//! Security audit trail.
//!
//! Every tool invocation that has security relevance (shell commands, file
//! reads/writes, network fetches, preview-server access) is appended to
//! `.govinda/audit.log` as a single JSON line so a post-hoc review can
//! reconstruct who did what, when, and from which working directory. The
//! file is append-only from the application's perspective: govinda never
//! edits or deletes existing entries. Failures to write are swallowed
//! (audit logging must never break a tool).
//!
//! The schema is intentionally tiny and additive. Each line is a JSON
//! object with the following fields:
//!
//! - `ts`        — ISO-8601 timestamp
//! - `kind`      — one of: `shell`, `file_read`, `file_write`, `network`, `preview`
//! - `ok`        — bool (true unless explicitly failed)
//! - `detail`    — string with the redacted command/path/URL
//!
//! The file is local to the workspace (`.govinda/audit.log`) and is
//! never sent back to the model. It exists for the human operator.

use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

const AUDIT_FILE: &str = "audit.log";

/// Discriminator for the kind of event being recorded. New variants are
/// added when a new tool category needs auditing; old logs parse fine
/// because each line is independently JSON-encoded.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditKind {
    Shell,
    FileRead,
    FileWrite,
    Network,
    Preview,
}

#[derive(Debug, Serialize)]
struct AuditEntry<'a> {
    ts: String,
    kind: AuditKind,
    ok: bool,
    detail: &'a str,
}

/// Returns the audit log path for `workspace`, creating `.govinda/` if
/// needed. Returns `None` if creation fails so callers can no-op quietly.
fn audit_path(workspace: &Path) -> Option<PathBuf> {
    let dir = workspace.join(".govinda");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(AUDIT_FILE))
}

/// Append one entry to `.govinda/audit.log`. Best-effort: errors are
/// swallowed so audit logging never breaks a tool. Callers should pass
/// already-redacted detail (no raw API keys, no full HTTP bodies).
pub fn record(workspace: &Path, kind: AuditKind, ok: bool, detail: &str) {
    let Some(path) = audit_path(workspace) else {
        return;
    };
    let entry = AuditEntry {
        ts: crate::clock::now_iso8601(),
        kind,
        ok,
        detail,
    };
    let Ok(line) = serde_json::to_string(&entry) else {
        return;
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Convenience: record a shell execution. Keeps the legacy `audit_shell`
/// callers in `tools.rs` working with one line of forwarding.
pub fn shell(workspace: &Path, ok: bool, command: &str) {
    record(workspace, AuditKind::Shell, ok, command);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn ws() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("govinda-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn records_json_lines_for_each_kind() {
        let dir = ws();
        record(&dir, AuditKind::Shell, true, "echo hi");
        record(&dir, AuditKind::FileRead, true, "src/lib.rs");
        record(&dir, AuditKind::FileWrite, true, "src/lib.rs");
        record(&dir, AuditKind::Network, true, "https://example.com/");
        record(&dir, AuditKind::Preview, false, "missing-token");

        let raw = std::fs::read_to_string(dir.join(".govinda").join(AUDIT_FILE)).unwrap();
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 5);
        // Each line must be independently valid JSON with the expected
        // shape — guarantees the file is greppable and machine-parseable.
        let mut seen: HashMap<&str, bool> = HashMap::new();
        for line in &lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap_or_else(|e| {
                panic!("invalid JSON line: {line:?}: {e}");
            });
            let kind = v["kind"].as_str().unwrap().to_owned();
            seen.insert(Box::leak(kind.into_boxed_str()), v["ok"].as_bool().unwrap());
        }
        assert!(seen["shell"]);
        assert!(seen["file_read"]);
        assert!(!seen["preview"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failures_to_create_workspace_dir_silently_noop() {
        // A path with an invalid component must not panic; record() is
        // best-effort and returns silently.
        let bogus = Path::new("\0not\0a\0path");
        record(bogus, AuditKind::Shell, true, "x");
    }
}
