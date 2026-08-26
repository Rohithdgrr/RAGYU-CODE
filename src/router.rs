//! Router: per-session ordered list of model entries with strike
//! tracking and promotion to the next healthy entry on persistent
//! failures.
//!
//! The router is the runtime brain behind three policies:
//!   1. **Pre-flight on the active model** (`preflight.rs`).
//!   2. **Three-strike failover**: a model that fails three times in
//!      a row is quarantined for the rest of the session; the next
//!      healthy entry is promoted transparently.
//!   3. **Cheapest healthy summarizer** for auto-compact
//!      (`auto_compact.rs`).
//!
//! The router itself is a small, dependency-free struct so it can be
//! unit-tested without spinning up an HTTP client.

use std::collections::{HashMap, HashSet};

use crate::provider::{self, RouterRole};

/// Number of consecutive failures before a model is quarantined for
/// the rest of the session.
pub const STRIKES_TO_QUARANTINE: u8 = 3;

/// Categories of failure. Some kinds are recoverable on the same
/// model (e.g. `RateLimit` after backoff), some are not
/// (`BadModel`, `Auth`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Auth,
    RateLimit,
    Server,
    Timeout,
    BadModel,
    Empty,
    Other,
}

#[derive(Debug, Clone, Default)]
pub struct Health {
    pub strikes: u8,
    pub last_latency_ms: u32,
    pub last_error: Option<String>,
    pub total_requests: u32,
    pub total_failures: u32,
}

#[derive(Debug, Clone)]
pub struct RouterEntry {
    pub model: String,
    pub role: RouterRole,
    pub context_window: usize,
}

#[derive(Debug)]
pub struct Router {
    entries: Vec<RouterEntry>,
    active_idx: usize,
    health: HashMap<String, Health>,
    quarantined: HashSet<String>,
    failover_enabled: bool,
}

impl Router {
    /// Builds the router for the given active provider/model. The
    /// active model is always `entries[0]`. Subsequent entries are
    /// fallbacks drawn from the OmniRoute combo table for the
    /// omniroute provider, or empty otherwise.
    pub fn for_active(provider: &str, model: &str) -> Self {
        let active_window = provider::context_window_for(provider, model);
        let mut entries = Vec::with_capacity(provider::OMNIROUTE_COMBOS.len() + 1);
        entries.push(RouterEntry {
            model: model.to_owned(),
            role: RouterRole::Primary,
            context_window: active_window,
        });
        if provider == "omniroute" {
            // Combo order for failover: smart, coding, fast, cheap, offline.
            // Skip the active model if it is itself a combo.
            const COMBO_ORDER: &[&str] = &["/smart", "/coding", "/fast", "/cheap", "/offline"];
            for id in COMBO_ORDER {
                if *id == model {
                    continue;
                }
                if let Some(c) = provider::omniroute_combo(id) {
                    entries.push(RouterEntry {
                        model: c.id.to_owned(),
                        role: c.role,
                        context_window: c.context_window,
                    });
                }
            }
        }
        Self {
            entries,
            active_idx: 0,
            health: HashMap::new(),
            quarantined: HashSet::new(),
            failover_enabled: true,
        }
    }

    pub fn active(&self) -> &RouterEntry {
        &self.entries[self.active_idx]
    }

    pub fn active_model(&self) -> &str {
        self.active().model.as_str()
    }

    pub fn failover_enabled(&self) -> bool {
        self.failover_enabled
    }

    pub fn set_failover(&mut self, on: bool) {
        self.failover_enabled = on;
    }

    /// Returns the cheapest healthy router entry. The summarizer
    /// runs are token-heavy but not quality-sensitive, so
    /// `Fast > Cheap > active` is the preference order. Returns the
    /// active entry when nothing healthier exists.
    pub fn next_summarizer(&self) -> &RouterEntry {
        let want_first = |r: RouterRole| matches!(r, RouterRole::Fast);
        let want_second = |r: RouterRole| matches!(r, RouterRole::Cheap);
        if let Some(e) = self
            .entries
            .iter()
            .find(|e| !self.quarantined.contains(&e.model) && want_first(e.role))
        {
            return e;
        }
        if let Some(e) = self
            .entries
            .iter()
            .find(|e| !self.quarantined.contains(&e.model) && want_second(e.role))
        {
            return e;
        }
        self.active()
    }

    /// Increments the strike counter for `model` and quarantines it
    /// when it reaches `STRIKES_TO_QUARANTINE`. Quarantining is
    /// sticky for the session; use [`Self::clear_quarantines`] to
    /// re-enable.
    pub fn record_failure(&mut self, model: &str, kind: FailureKind, msg: &str) {
        let h = self.health.entry(model.to_owned()).or_default();
        h.strikes = h.strikes.saturating_add(1);
        h.last_error = Some(format!("{kind:?}: {msg}"));
        h.total_failures = h.total_failures.saturating_add(1);
        h.total_requests = h.total_requests.saturating_add(1);
        if h.strikes >= STRIKES_TO_QUARANTINE {
            self.quarantine(model);
        }
    }

    pub fn record_success(&mut self, model: &str, latency_ms: u32) {
        let h = self.health.entry(model.to_owned()).or_default();
        h.strikes = 0;
        h.last_latency_ms = latency_ms;
        h.last_error = None;
        h.total_requests = h.total_requests.saturating_add(1);
    }

    pub fn quarantine(&mut self, model: &str) {
        if self.quarantined.insert(model.to_owned()) {
            eprintln!("router: model {model} quarantined after {STRIKES_TO_QUARANTINE} strikes");
        }
    }

    pub fn is_quarantined(&self, model: &str) -> bool {
        self.quarantined.contains(model)
    }

    pub fn clear_quarantines(&mut self) {
        self.quarantined.clear();
    }

    /// Promotes to the next non-quarantined entry. Returns `None` if
    /// every entry is quarantined or `failover_enabled` is `false`.
    /// The caller is expected to call this only once per turn.
    pub fn promote(&mut self) -> Option<&RouterEntry> {
        if !self.failover_enabled {
            return None;
        }
        let start = self.active_idx;
        let n = self.entries.len();
        for step in 1..=n {
            let idx = (start + step) % n;
            let m = &self.entries[idx].model;
            if !self.quarantined.contains(m) {
                self.active_idx = idx;
                eprintln!(
                    "router: promoted {} → {}",
                    self.entries[start].model, m
                );
                return Some(&self.entries[idx]);
            }
        }
        None
    }

    pub fn health(&self, model: &str) -> Option<&Health> {
        self.health.get(model)
    }

    pub fn quarantined(&self) -> impl Iterator<Item = &str> {
        self.quarantined.iter().map(|s| s.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = &RouterEntry> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Router {
        Router::for_active("omniroute", "auto")
    }

    #[test]
    fn active_is_first_entry() {
        let r = fixture();
        assert_eq!(r.active().model, "auto");
        assert_eq!(r.active().role, RouterRole::Primary);
    }

    #[test]
    fn three_strikes_quarantines_and_promote_skips() {
        let mut r = fixture();
        for msg in ["a", "b", "c"] {
            r.record_failure("auto", FailureKind::Server, msg);
        }
        assert!(r.is_quarantined("auto"));
        // Promote should land on the first non-quarantined entry: /smart.
        let next = r.promote().expect("promote succeeds");
        assert_eq!(next.model, "/smart");
        assert_eq!(r.active().model, "/smart");
    }

    #[test]
    fn promote_returns_none_when_all_quarantined() {
        let mut r = fixture();
        for entry in r.iter() {
            r.record_failure(&entry.model, FailureKind::Server, "x");
        }
        assert!(r.promote().is_none());
    }

    #[test]
    fn promote_respects_failover_off() {
        let mut r = fixture();
        r.set_failover(false);
        for _ in 0..3 {
            r.record_failure("auto", FailureKind::Server, "x");
        }
        assert!(r.promote().is_none());
    }

    #[test]
    fn success_resets_strike_counter() {
        let mut r = fixture();
        r.record_failure("auto", FailureKind::Server, "x");
        r.record_failure("auto", FailureKind::Server, "x");
        r.record_success("auto", 250);
        assert_eq!(r.health("auto").unwrap().strikes, 0);
    }

    #[test]
    fn next_summarizer_prefers_fast_then_cheap_then_active() {
        let r = fixture();
        // No quarantines yet: first Fast wins. /fast is at index 3
        // in the fixture (active=auto, /smart, /coding, /fast, …).
        assert_eq!(r.next_summarizer().model, "/fast");
        let mut r = fixture();
        // Quarantine /fast, /coding, /smart: Cheap wins.
        r.record_failure("/fast", FailureKind::Server, "x");
        r.record_failure("/fast", FailureKind::Server, "x");
        r.record_failure("/fast", FailureKind::Server, "x");
        r.record_failure("/coding", FailureKind::Server, "x");
        r.record_failure("/coding", FailureKind::Server, "x");
        r.record_failure("/coding", FailureKind::Server, "x");
        r.record_failure("/smart", FailureKind::Server, "x");
        r.record_failure("/smart", FailureKind::Server, "x");
        r.record_failure("/smart", FailureKind::Server, "x");
        assert_eq!(r.next_summarizer().model, "/cheap");
    }

    #[test]
    fn clear_quarantines_re_enables() {
        let mut r = fixture();
        for _ in 0..3 {
            r.record_failure("auto", FailureKind::Server, "x");
        }
        r.clear_quarantines();
        assert!(!r.is_quarantined("auto"));
    }

    #[test]
    fn non_omniroute_provider_has_no_fallbacks() {
        let r = Router::for_active("mistral", "mistral-small-latest");
        assert_eq!(r.iter().count(), 1);
    }
}
