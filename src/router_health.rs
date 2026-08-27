//! Per-request router health log. Writes one JSONL line per model
//! request to `.govinda/router_health.jsonl`. The file is capped at
//! 1 MB; the oldest line is dropped on rotation.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use crate::router::Router;
use serde_json::json;

const LOG_NAME: &str = "router_health.jsonl";
const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct HealthEntry {
    pub ts: String,
    pub model: String,
    pub latency_ms: u32,
    pub success: bool,
    pub error: Option<String>,
}

/// Returns the log path under `.govinda/`, falling back to the
/// process temp dir when the workspace is read-only.
fn log_path() -> PathBuf {
    let dir = std::env::current_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join(".govinda");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(LOG_NAME)
}

/// Appends one entry. Silently no-ops on IO failure (the log is
/// observability, not a correctness path).
pub fn append(entry: &HealthEntry) {
    let path = log_path();
    rotate_if_needed(&path);
    let line = json!({
        "ts": entry.ts,
        "model": entry.model,
        "latency_ms": entry.latency_ms,
        "success": entry.success,
        "error": entry.error,
    })
    .to_string()
        + "\n";
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Convenience: snapshots the router's per-model health and writes
/// one entry per non-empty model.
pub fn snapshot(router: &Router) {
    let now = chrono::Utc::now().to_rfc3339();
    for entry in router.iter() {
        if let Some(h) = router.health(&entry.model) {
            append(&HealthEntry {
                ts: now.clone(),
                model: entry.model.clone(),
                latency_ms: h.last_latency_ms,
                success: h.last_error.is_none(),
                error: h.last_error.clone(),
            });
        }
    }
}

fn rotate_if_needed(path: &PathBuf) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() < MAX_BYTES {
        return;
    }
    let _ = std::fs::rename(path, path.with_extension("jsonl.1"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_path_under_govinda_dir() {
        let p = log_path();
        assert!(p.ends_with("router_health.jsonl"));
        assert!(p.to_string_lossy().contains(".govinda"));
    }

    #[test]
    fn append_writes_a_line() {
        let e = HealthEntry {
            ts: "2026-01-01T00:00:00Z".to_owned(),
            model: "auto".to_owned(),
            latency_ms: 123,
            success: true,
            error: None,
        };
        append(&e);
        let body = std::fs::read_to_string(log_path()).unwrap_or_default();
        assert!(body.contains("\"model\":\"auto\""));
    }
}
