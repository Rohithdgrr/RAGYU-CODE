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
/// (`BadModel`, `Auth`). `Busy` covers gateway capacity errors
/// (`structure_limit`, `chat_admission_busy`, `overloaded`) that should
/// back off on the same gateway before consuming a failover slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Auth,
    RateLimit,
    Busy,
    Server,
    Timeout,
    BadModel,
    Empty,
    Other,
}

impl FailureKind {
    /// Whether this kind is worth retrying on the same model before failover.
    pub fn is_retryable_on_same_model(self) -> bool {
        matches!(
            self,
            FailureKind::RateLimit | FailureKind::Busy | FailureKind::Server | FailureKind::Timeout | FailureKind::Empty | FailureKind::Other
        )
    }

    /// Whether failover should happen immediately even before 3 strikes.
    pub fn should_promote_immediately(self) -> bool {
        matches!(self, FailureKind::Auth | FailureKind::BadModel)
    }
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

#[derive(Debug, Clone)]
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
            // Combo order for failover: smart, coding, chat, fast, cheap, offline.
            // Skip the active model if it is itself a combo. The ids
            // must match what the gateway actually serves (e.g.
            // "auto/coding", not "/coding") so a failover lands on a
            // real model instead of a 429.
            const COMBO_ORDER: &[&str] = &[
                "auto/smart",
                "auto/coding",
                "auto/chat",
                "auto/fast",
                "auto/cheap",
                "auto/offline",
            ];
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

    /// Re-syncs the router when the active provider/model changes.
    /// Preserves strike counters and quarantine state, rebuilds the
    /// fallback list for the new provider, and re-enables the new
    /// active model if it was previously quarantined (per spec:
    /// `/model <id>` re-enables a quarantined model).
    pub fn sync_active(&mut self, provider: &str, model: &str) {
        let needs_rebuild = self.active_model() != model
            || (provider == "omniroute") != (self.entries.len() > 1)
            || (provider != "omniroute" && self.entries.len() != 1);
        if !needs_rebuild {
            return;
        }
        let old_health = std::mem::take(&mut self.health);
        let old_quarantined = std::mem::take(&mut self.quarantined);
        let failover = self.failover_enabled;
        let mut new = Self::for_active(provider, model);
        new.health = old_health;
        new.quarantined = old_quarantined;
        new.failover_enabled = failover;
        new.quarantined.remove(model);
        *self = new;
    }

    /// Replaces the fallback list with `chain` while preserving the
    /// active model, all health counters, all quarantines, and the
    /// `failover_enabled` flag. The active model is kept at index 0;
    /// the chain (which already contains the active id in most cases)
    /// is de-duplicated and the active id is removed from any other
    /// position so it can't be promoted to itself. Existing health
    /// entries for models that are no longer in the chain are kept
    /// in `health` (harmless) but quarantines are dropped for models
    /// the chain no longer references so a stale quarantine cannot
    /// permanently hide a fresh candidate.
    ///
    /// `role_for` lets the caller attach a `RouterRole` to each model
    /// id. `context_window_for` resolves the input-token limit; pass
    /// 0 to leave the default 0 (the router does not require a
    /// non-zero window for failover).
    pub fn seed_entries(
        &mut self,
        chain: impl IntoIterator<Item = String>,
        role_for: impl Fn(&str) -> crate::provider::RouterRole,
        context_window_for: impl Fn(&str) -> usize,
    ) {
        let active = self.active_model().to_owned();
        let mut seen = std::collections::HashSet::new();
        let mut entries: Vec<RouterEntry> = Vec::new();
        // Active first, with the role the caller assigns to it.
        seen.insert(active.clone());
        entries.push(RouterEntry {
            model: active.clone(),
            role: role_for(&active),
            context_window: context_window_for(&active),
        });
        for m in chain {
            if seen.insert(m.clone()) {
                entries.push(RouterEntry {
                    role: role_for(&m),
                    context_window: context_window_for(&m),
                    model: m,
                });
            }
        }
        // Drop quarantines for models that are no longer in the
        // chain so we don't permanently lock out a candidate that
        // just got re-introduced.
        let keep: std::collections::HashSet<String> =
            entries.iter().map(|e| e.model.clone()).collect();
        self.quarantined.retain(|m| keep.contains(m));
        self.entries = entries;
        self.active_idx = 0;
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
                eprintln!("router: promoted {} → {}", self.entries[start].model, m);
                return Some(&self.entries[idx]);
            }
        }
        None
    }

    /// Returns true if model has enough strikes to warrant promotion.
    /// `Auth`/`BadModel` promote immediately; others need 3 strikes.
    pub fn should_promote(&self, model: &str, kind: FailureKind) -> bool {
        if kind.should_promote_immediately() {
            return true;
        }
        self.health
            .get(model)
            .map(|h| h.strikes >= STRIKES_TO_QUARANTINE)
            .unwrap_or(false)
    }

    /// Number of non-quarantined candidates (including active).
    pub fn healthy_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| !self.quarantined.contains(&e.model))
            .count()
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
        // Promote should land on the first non-quarantined entry: auto/smart.
        let next = r.promote().expect("promote succeeds");
        assert_eq!(next.model, "auto/smart");
        assert_eq!(r.active().model, "auto/smart");
    }

    #[test]
    fn promote_returns_none_when_all_quarantined() {
        let mut r = fixture();
        let models: Vec<String> = r.iter().map(|e| e.model.clone()).collect();
        for model in &models {
            for _ in 0..STRIKES_TO_QUARANTINE {
                r.record_failure(model, FailureKind::Server, "x");
            }
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
        // No quarantines yet: first Fast wins. auto/fast is at index 3
        // in the fixture (active=auto, auto/smart, auto/coding, auto/fast, …).
        assert_eq!(r.next_summarizer().model, "auto/fast");
        let mut r = fixture();
        // Quarantine auto/fast, auto/coding, auto/smart: Cheap wins.
        r.record_failure("auto/fast", FailureKind::Server, "x");
        r.record_failure("auto/fast", FailureKind::Server, "x");
        r.record_failure("auto/fast", FailureKind::Server, "x");
        r.record_failure("auto/coding", FailureKind::Server, "x");
        r.record_failure("auto/coding", FailureKind::Server, "x");
        r.record_failure("auto/coding", FailureKind::Server, "x");
        r.record_failure("auto/smart", FailureKind::Server, "x");
        r.record_failure("auto/smart", FailureKind::Server, "x");
        r.record_failure("auto/smart", FailureKind::Server, "x");
        assert_eq!(r.next_summarizer().model, "auto/cheap");
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
