//! Smart "auto" model selection for the OmniRoute provider.
//!
//! The provider's gateway exposes roughly 100 model ids
//! (Claude Opus 4.8 / Sonnet 5, GPT-5.6, Gemini 3.1 Pro, GLM 5.2, Kimi
//! K2.7, DeepSeek V4, plus the gateway's own `auto/*` smart-routing
//! combos). Most of those are paid or rate-limited at any given
//! moment, so the CLI must:
//!
//!   1. know which models are even worth trying (a hard-coded
//!      preference list ordered by capability), and
//!   2. ask the live gateway which of those are currently served, and
//!   3. probe them in order, picking the first that actually answers.
//!
//! The first reachable model becomes the active one and seeds the
//! router's fallback chain. `auto` therefore works for every kind of
//! task (chat, coding, debugging, documenting, agent work) without the
//! user ever naming a model.
//!
//! The preference list is also the source for `/model auto` and the
//! initial router entries so the existing 3-strike failover walks the
//! same chain in the same order.

use crate::provider::{self, Provider, RouterRole};
use std::time::{Duration, Instant};

/// Tier 1 — high-end models to try first. Order is "best first" based
/// on observed quality and gateway-availability during development.
/// The list is a static preference, not a guarantee; the live
/// `/v1/models` endpoint decides what is actually served, and the
/// preflight probe decides what is actually reachable.
///
/// We keep ids in the exact form the gateway serves them
/// (`aug/opus4.8`, `auto/coding:pro`, …). The router already strips
/// any `<vendor>/` prefix when matching context windows.
pub const TIER1_PREFERENCE: &[&str] = &[
    // Gateway's own smart combos (best signal of "the best currently
    // available" because they re-rank across all connected providers).
    "auto/smart",
    "auto/best-chat",
    "auto/best-coding",
    "auto/best-reasoning",
    "auto/coding:pro",
    "auto/coding:reliable",
    // Direct high-end model ids exposed by the gateway's vendors.
    "aug/opus4.8",
    "aug/sonnet5-high",
    "aug/sonnet5-500k",
    "aug/opus4.7-500k",
    "aug/opus4.7",
    "aug/opus4.6-500k",
    "aug/sonnet4.6-500k",
    "aug/sonnet4.6",
    "aug/opus4.6",
    "aug/opus4.5",
    "aug/sonnet4.5",
    "aug/gpt5.6-luna",
    "aug/gpt5.6-sol",
    "aug/gpt5.6-terra",
    "aug/gpt5.5",
    "aug/gpt5.4",
    "aug/gemini-3.1-pro-preview",
    "aug/glm-5.2",
    "aug/kimi-k2.7",
    "aug/kimi-k2.6",
    "aug/prism-a",
    "aug/prism-b",
    "oc/deepseek-v4-flash-free",
];

/// Tier 2 — broad-coverage fallbacks. These are gateway smart-combos
/// that re-route at request time, so they are almost always reachable
/// even when specific models above are rate-limited. We use them as
/// the last line of defence before giving up.
pub const TIER2_PREFERENCE: &[&str] = &[
    "auto/coding",
    "auto/chat",
    "auto/fast",
    "auto/cheap",
    "auto/best-free",
    "auto/best-fast",
    "auto/best-coding-fast",
    "auto/best-vision",
    "auto/chaos",
    "auto/best-chaos",
    "auto/reasoning",
    "auto/reasoning:pro",
    "auto/coding:fast",
    "auto/coding:cheap",
    "auto/coding:free",
    "auto/multimodal",
    "auto/vision",
    "auto/glm",
    "auto/gemini",
    "auto/gemma",
    "auto/llama",
    "auto/mimo",
    "auto/zai",
    "auto/minimax",
    "auto/offline",
];

/// Probe timeout per candidate during auto-selection. Short on
/// purpose: if a model is rate-limited or down, we want to move on
/// quickly, not block the user for 8 seconds per candidate.
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const PROBE_PROMPT: &str = "ping";
const PROBE_MAX_TOKENS: u32 = 4;

/// HTTP-side timeout for fetching the live model list.
const MODELS_TIMEOUT: Duration = Duration::from_secs(3);

/// Result of picking the "auto" model.
#[derive(Debug, Clone)]
pub struct AutoPick {
    /// The model id that answered the probe (active model).
    pub model: String,
    /// Latency of the winning probe, in ms.
    pub latency_ms: u32,
    /// Number of candidates probed before the winner (0 = first try).
    pub tried: usize,
    /// Total candidates considered (after filtering against the live
    /// `/v1/models` listing). Used for the user-facing
    /// "1st of N tried" message.
    pub total: usize,
    /// All preference candidates that the live gateway serves, in
    /// preference order. The router seeds its fallback list with this
    /// so `promote()` walks the same chain.
    pub chain: Vec<String>,
}

impl AutoPick {
    fn empty() -> Self {
        Self {
            model: String::new(),
            latency_ms: 0,
            tried: 0,
            total: 0,
            chain: Vec::new(),
        }
    }
}

/// Fetches the live `/v1/models` listing for `provider` and returns the
/// served model ids, sorted alphabetically. Returns an empty `Vec` on
/// transport/parse failure (we treat that as "the gateway did not
/// answer" and fall back to the static preference list as-is).
pub async fn live_model_ids(http: &reqwest::Client, provider: &dyn Provider) -> Vec<String> {
    let Some(url) = provider.models_url() else {
        return Vec::new();
    };
    // Bind `auth` to a local so the `&str` returned by `.token()`
    // outlives the borrow of the temporary `Auth` value.
    let auth = provider.auth();
    let bearer = auth.token();
    let mut req = http.get(&url);
    if let Some(t) = bearer {
        req = req.bearer_auth(t);
    }
    let resp = match tokio::time::timeout(MODELS_TIMEOUT, req.send()).await {
        Ok(Ok(r)) if r.status().is_success() => r,
        _ => return Vec::new(),
    };
    // Parse in-line to avoid a second GET (old impl called list_models which re-fetched).
    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let Some(arr) = body.get("data").and_then(|d| d.as_array()) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = arr
        .iter()
        .filter_map(|e| e.get("id").and_then(|v| v.as_str()).map(|s| s.to_owned()))
        .collect();
    ids.sort();
    ids
}

/// Filters a static preference list against the live `/v1/models` list.
/// `tier` is the slice of preference ids. `live` is the served model
/// ids. The returned `Vec` preserves the preference order.
///
/// If `live` is empty (gateway did not answer), returns `tier` items
/// as-is so the user still gets sensible defaults.
pub fn filter_against_live(tier: &[&'static str], live: &[String]) -> Vec<String> {
    if live.is_empty() {
        return tier.iter().map(|s| s.to_string()).collect();
    }
    tier.iter()
        .filter(|id| live.iter().any(|served| served == *id))
        .map(|s| s.to_string())
        .collect()
}

/// Builds the full preference chain (tier 1 + tier 2) filtered against
/// the live model list. De-duplicates while preserving order. Returns
/// an empty `Vec` only if both tiers are empty (which never happens
/// given the constants above).
pub fn build_chain(live: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for id in TIER1_PREFERENCE
        .iter()
        .chain(TIER2_PREFERENCE.iter())
        .copied()
    {
        if !live.is_empty() && !live.iter().any(|s| s == id) {
            continue;
        }
        if seen.insert(id.to_string()) {
            out.push(id.to_string());
        }
    }
    out
}

/// Sends a single non-streaming `chat/completions` request with a tiny
/// prompt and 4-token cap. Returns `Some(latency_ms)` on HTTP 200 with
/// a **non-empty** `choices[0].message.content`, `None` otherwise.
/// The preflight module has the same purpose but is intentionally
/// minimal; this helper is preference-aware and tolerates more failure
/// modes because the caller iterates over many candidates and a 4xx on
/// one is expected.
///
/// Crucially we validate body content — a gateway that returns 200
/// with `{"choices":[{"message":{"content":""}}]}` or empty `choices`
/// (quota-exhausted shim) must NOT be treated as healthy, otherwise
/// `pick_best` would elect a broken `auto/chat` combo that later
/// yields the user-visible `(empty response)` placeholder.
async fn probe_one(
    http: &reqwest::Client,
    provider: &dyn Provider,
    model: &str,
) -> Option<u32> {
    let url = provider.chat_url();
    // Bind `auth` to a local so the `&str` returned by `.token()`
    // outlives the borrow of the temporary `Auth` value.
    let auth = provider.auth();
    let bearer = auth.token();
    let body = serde_json::json!({
        "model": model,
        "temperature": 0,
        "max_tokens": PROBE_MAX_TOKENS,
        "stream": false,
        "messages": [{ "role": "user", "content": PROBE_PROMPT }],
    });
    let mut req = http.post(&url);
    if let Some(t) = bearer {
        req = req.bearer_auth(t);
    }
    let started = Instant::now();
    let resp = match tokio::time::timeout(PROBE_TIMEOUT, req.json(&body).send()).await {
        Ok(Ok(r)) => r,
        _ => return None,
    };
    if !resp.status().is_success() {
        return None;
    }
    // Validate body contains non-empty assistant content — reject
    // quota-shimmed 200s that carry empty choices.
    let value: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return None,
    };
    if let Some(err) = value.get("error") {
        // Gateway forwarded an upstream error inside 200 — treat as unhealthy.
        let _ = err;
        return None;
    }
    let content = value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    if content.trim().is_empty() {
        // Also accept non-empty tool_calls as proof of liveness for
        // models that answer the probe with a function call, though the
        // "ping" prompt should never trigger that.
        let has_tool_calls = value
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("tool_calls"))
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty());
        if !has_tool_calls {
            return None;
        }
    }
    Some(started.elapsed().as_millis() as u32)
}

/// Walks the preference chain and returns the first model that
/// answers the probe. Returns `AutoPick::empty()` if every candidate
/// fails or `chain` is empty. The router is **not** updated by this
/// function — the caller decides how to use the result.
///
/// The returned `chain` is always populated (when the live
/// `/v1/models` listing was reachable) so the caller can seed its
/// router entries from the same preference order without re-fetching
/// the model list.
pub async fn pick_best(
    http: &reqwest::Client,
    provider: &dyn Provider,
) -> AutoPick {
    let live = live_model_ids(http, provider).await;
    let chain = build_chain(&live);
    let total = chain.len();
    if chain.is_empty() {
        return AutoPick::empty();
    }
    for (i, model) in chain.iter().enumerate() {
        if let Some(latency_ms) = probe_one(http, provider, model).await {
            return AutoPick {
                model: model.clone(),
                latency_ms,
                tried: i,
                total,
                chain,
            };
        }
    }
    AutoPick {
        model: String::new(),
        latency_ms: 0,
        tried: chain.len(),
        total,
        chain,
    }
}

/// Router role for a preference-list model id. Smart combos get their
/// declared role; everything else is `Smart` (the best signal we
/// have without inspecting the live model's `capabilities` map).
pub fn role_for(model: &str) -> RouterRole {
    if let Some(c) = provider::omniroute_combo(model) {
        return c.role;
    }
    // Heuristics by id prefix / family so the summarizer picks
    // sensibly and the cost-aware failover still works.
    let m = model.to_ascii_lowercase();
    if m.contains("offline") {
        RouterRole::Offline
    } else if m.contains("fast") || m.contains("nano") || m.contains("mini") || m.contains("haiku") {
        RouterRole::Fast
    } else if m.contains("cheap") || m.contains("free") {
        RouterRole::Cheap
    } else {
        RouterRole::Smart
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live() -> Vec<String> {
        vec![
            "aug/opus4.8".into(),
            "aug/sonnet5-high".into(),
            "auto/smart".into(),
            "auto/coding".into(),
            "auto/chat".into(),
            "unrelated-model".into(),
        ]
    }

    #[test]
    fn filter_preserves_preference_order() {
        let live = live();
        let tier1_filtered = filter_against_live(TIER1_PREFERENCE, &live);
        // aug/opus4.8 is first in TIER1 and exists in live; it must
        // come before aug/sonnet5-high.
        let i_opus = tier1_filtered.iter().position(|s| s == "aug/opus4.8").unwrap();
        let i_sonnet = tier1_filtered
            .iter()
            .position(|s| s == "aug/sonnet5-high")
            .unwrap();
        assert!(i_opus < i_sonnet, "preference order must be preserved");
    }

    #[test]
    fn build_chain_dedupes_across_tiers() {
        let live = live();
        let chain = build_chain(&live);
        // No duplicates even if a model appears in both tiers.
        let mut sorted = chain.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), chain.len(), "chain must not contain duplicates");
    }

    #[test]
    fn build_chain_drops_unserved_when_live_known() {
        let live = live();
        let chain = build_chain(&live);
        // aug/opus4.7-500k is in TIER1 but not in our test live set.
        assert!(!chain.iter().any(|s| s == "aug/opus4.7-500k"));
        // auto/coding IS in live so it should be present.
        assert!(chain.iter().any(|s| s == "auto/coding"));
    }

    #[test]
    fn build_chain_falls_back_to_static_when_live_empty() {
        let chain = build_chain(&[]);
        assert!(!chain.is_empty(), "static fallback must produce a chain");
        // Order is preserved (tier 1 entries come before tier 2).
        let i_smart = chain.iter().position(|s| s == "auto/smart").unwrap();
        let i_chat = chain.iter().position(|s| s == "auto/chat").unwrap();
        assert!(i_smart < i_chat);
    }

    #[test]
    fn role_for_smart_combo_uses_omniroute_table() {
        assert_eq!(role_for("auto/smart"), RouterRole::Smart);
        assert_eq!(role_for("auto/coding"), RouterRole::Coding);
        assert_eq!(role_for("auto/fast"), RouterRole::Fast);
    }

    #[test]
    fn role_for_uses_heuristics_for_direct_ids() {
        assert_eq!(role_for("aug/opus4.8"), RouterRole::Smart);
        assert_eq!(role_for("aug/haiku4.5"), RouterRole::Fast);
        assert_eq!(role_for("oc/deepseek-v4-flash-free"), RouterRole::Cheap);
    }
}
