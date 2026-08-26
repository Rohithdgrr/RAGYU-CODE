//! Pre-flight probe of the active model before the first chat.
//!
//! Sends a single 1-token `chat/completions` request with an 8 s
//! total timeout and reports `Ok` / `Warn` / `Err` so the CLI can
//! surface a clean error before the user invests in a long prompt.
//!
//! Only the active model is probed; fallbacks are not.

use crate::provider::Provider;
use anyhow::Context as _;
use std::time::{Duration, Instant};

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const PROBE_PROMPT: &str = "ping";
const PROBE_MAX_TOKENS: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeStatus {
    Ok,
    Warn(String),
    Err(String),
}

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub model: String,
    pub latency_ms: u32,
    pub status: ProbeStatus,
}

pub async fn probe_active(
    http: &reqwest::Client,
    provider: &dyn Provider,
    model: &str,
) -> ProbeResult {
    let started = Instant::now();
    let url = provider.chat_url();
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
    let resp = match tokio::time::timeout(PROBE_TIMEOUT, req.json(&body).send()).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            return ProbeResult {
                model: model.to_owned(),
                latency_ms: started.elapsed().as_millis() as u32,
                status: ProbeStatus::Err(format!("transport: {e}")),
            };
        }
        Err(_) => {
            return ProbeResult {
                model: model.to_owned(),
                latency_ms: started.elapsed().as_millis() as u32,
                status: ProbeStatus::Err(format!("timeout after {PROBE_TIMEOUT:?}")),
            };
        }
    };
    let latency_ms = started.elapsed().as_millis() as u32;
    if !resp.status().is_success() {
        let status = resp.status();
        return ProbeResult {
            model: model.to_owned(),
            latency_ms,
            status: ProbeStatus::Err(format!("HTTP {status}")),
        };
    }
    let value: serde_json::Value = match resp.json().await.context("probe: parse body") {
        Ok(v) => v,
        Err(e) => {
            return ProbeResult {
                model: model.to_owned(),
                latency_ms,
                status: ProbeStatus::Err(format!("body: {e}")),
            };
        }
    };
    let content = value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    if content.is_empty() {
        ProbeResult {
            model: model.to_owned(),
            latency_ms,
            status: ProbeStatus::Warn("empty reply body".to_owned()),
        }
    } else {
        ProbeResult {
            model: model.to_owned(),
            latency_ms,
            status: ProbeStatus::Ok,
        }
    }
}
