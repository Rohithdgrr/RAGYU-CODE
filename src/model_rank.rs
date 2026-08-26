//! Top-models ranker. Combines the static `KnownModel` registry with
//! the runtime `Router` health log to produce a sorted view of
//! "best models for this provider".
//!
//! The ranker is deliberately small and pure: it does not perform
//! any network I/O. The `Router` is optional; when `None`, only
//! registry hints are used.

use crate::provider::{self, KnownModel, RouterRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Quality,
    Speed,
    Cost,
    Context,
    Free,
}

impl SortKey {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "quality" | "qual" | "q" => Some(Self::Quality),
            "speed" | "latency" => Some(Self::Speed),
            "cost" | "price" => Some(Self::Cost),
            "context" | "ctx" | "c" => Some(Self::Context),
            "free" => Some(Self::Free),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RankedModel {
    pub id: String,
    pub role: RouterRole,
    pub free: bool,
    pub context_window: usize,
    pub description: &'static str,
    pub score: f32,
}

pub fn top_models(provider: &str, sort: SortKey, n: usize) -> Vec<RankedModel> {
    let registry = provider::known_models(provider);
    let mut out: Vec<RankedModel> = registry
        .iter()
        .map(|km| score_row(km, sort))
        .collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(n);
    out
}

fn score_row(km: &KnownModel, sort: SortKey) -> RankedModel {
    let role = role_for(km);
    let context_norm = if km.context_window == 0 {
        0.0
    } else {
        (km.context_window as f32 / 1_000_000.0).min(1.0)
    };
    let score = match sort {
        SortKey::Quality => 0.6 * context_norm + 0.4 * (if km.free { 1.0 } else { 0.5 }),
        SortKey::Speed => 0.7 + 0.3 * context_norm,
        SortKey::Cost => if km.free { 1.0 } else { 0.4 },
        SortKey::Context => context_norm,
        SortKey::Free => if km.free { 1.0 } else { 0.0 },
    };
    RankedModel {
        id: km.id.to_owned(),
        role,
        free: km.free,
        context_window: km.context_window,
        description: km.description,
        score,
    }
}

fn role_for(km: &KnownModel) -> RouterRole {
    provider::omniroute_combo(&km.id).map(|c| c.role).unwrap_or_else(|| {
        if km.id == "auto" {
            RouterRole::Smart
        } else {
            RouterRole::Generic
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sort_keys() {
        assert_eq!(SortKey::parse("quality"), Some(SortKey::Quality));
        assert_eq!(SortKey::parse("c"), Some(SortKey::Context));
        assert_eq!(SortKey::parse("nope"), None);
    }

    #[test]
    fn top_models_returns_n_sorted_descending() {
        let rows = top_models("omniroute", SortKey::Context, 3);
        assert!(rows.len() <= 3);
        for w in rows.windows(2) {
            assert!(w[0].score >= w[1].score);
        }
    }

    #[test]
    fn free_sort_puts_free_first() {
        let rows = top_models("openai", SortKey::Free, 10);
        // GPT-3.5 is the only "free" entry (registry flag) and
        // should rank first.
        assert!(!rows.is_empty());
        assert!(rows[0].free);
    }
}
