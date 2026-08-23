//! Persistent project memory (`.govinda_project.json` in the repo root).
//!
//! Remembers workspace-scoped facts across runs so the agent does not have
//! to rediscover them every session:
//!   - the commit hash of the last `/scan` (lets future work detect a stale
//!     symbol index without re-walking the tree),
//!   - the user's preferred test command,
//!   - the user's preferred build/check command.
//!
//! The file is plain JSON in the workspace root; it is read-only for every
//! tool and only ever written by the explicit `/project` command or a
//! successful scan.

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const PROJECT_FILE: &str = ".govinda_project.json";

/// Workspace-scoped preferences persisted across sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectMemory {
    /// Commit HEAD pointed at during the last full workspace scan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_scan_commit: Option<String>,
    /// User-configured test runner (`/project set test …`); preferred over
    /// auto-detection by `run_test`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_command: Option<String>,
    /// User-configured build/validation command; preferred by
    /// `check_project`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_command: Option<String>,
}

impl ProjectMemory {
    /// Splits a stored command into `(program, argv)` — whitespace only,
    /// never a shell — rejecting empty input.
    pub fn argv(command: &str) -> Option<(String, Vec<String>)> {
        let mut words = command.split_whitespace().map(str::to_owned);
        let program = words.next()?;
        Some((program, words.collect()))
    }
}

fn path_in(base: &Path) -> PathBuf {
    base.join(PROJECT_FILE)
}

/// Loads memory from `base/.govinda_project.json`; missing or malformed
/// files mean "no memory yet" rather than an error.
pub fn load_from(base: &Path) -> ProjectMemory {
    std::fs::read_to_string(path_in(base))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save_to(base: &Path, mem: &ProjectMemory) -> Result<()> {
    let json = serde_json::to_string_pretty(mem).context("cannot serialize project memory")?;
    std::fs::write(path_in(base), json)
        .with_context(|| format!("cannot write {}", path_in(base).display()))
}

/// Convenience wrappers over the process working directory (the workspace
/// root) for callers outside command handling.
pub fn load() -> ProjectMemory {
    std::env::current_dir()
        .map(|cwd| load_from(&cwd))
        .unwrap_or_default()
}

/// Records `commit` as the last scanned HEAD and persists immediately.
/// A failed save degrades to a warning-level error for the caller to show;
/// scanning itself never fails because of this.
pub fn record_scan_commit(base: &Path, commit: &str) -> Result<()> {
    let mut mem = load_from(base);
    mem.last_scan_commit = Some(commit.to_owned());
    save_to(base, &mem)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "govinda-project-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn roundtrip_preserves_preferences() {
        let ws = TempDir::new("roundtrip");
        assert_eq!(load_from(&ws.0), ProjectMemory::default());

        let mem = ProjectMemory {
            last_scan_commit: Some("abc123".into()),
            test_command: Some("cargo nextest run".into()),
            ..Default::default()
        };
        save_to(&ws.0, &mem).unwrap();

        let loaded = load_from(&ws.0);
        assert_eq!(loaded, mem);
        assert!(ws.0.join(".govinda_project.json").is_file());

        // record_scan_commit updates just the hash field.
        record_scan_commit(&ws.0, "def456").unwrap();
        let loaded = load_from(&ws.0);
        assert_eq!(loaded.last_scan_commit.as_deref(), Some("def456"));
        assert_eq!(loaded.test_command.as_deref(), Some("cargo nextest run"));
    }

    #[test]
    fn malformed_memory_file_loads_as_empty() {
        let ws = TempDir::new("malformed");
        std::fs::write(ws.0.join(".govinda_project.json"), "{not json").unwrap();
        assert_eq!(load_from(&ws.0), ProjectMemory::default());
    }

    #[test]
    fn argv_splits_program_from_arguments() {
        let (prog, args) = ProjectMemory::argv("cargo test --lib").unwrap();
        assert_eq!(prog, "cargo");
        assert_eq!(args, vec!["test", "--lib"]);
        assert!(ProjectMemory::argv("").is_none());
        assert!(ProjectMemory::argv("   \t ").is_none());
    }
}
