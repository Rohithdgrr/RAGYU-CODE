use crate::session::Session;
use anyhow::Result;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const SESSIONS_DIR: &str = "sessions";

/// Metadata for one saved session file, for `/sessions` listings.
pub struct SessionEntry {
    pub name: String,
    pub path: PathBuf,
    pub messages: usize,
    pub updated_at: Option<String>,
    pub modified: Option<SystemTime>,
}

/// Maps a bare session name to `sessions/<name>.json`, rejecting anything
/// that could escape the sessions directory.
pub fn named_session_path(name: &str) -> Result<PathBuf> {
    valid_name(name)?;
    Ok(PathBuf::from(SESSIONS_DIR).join(format!("{name}.json")))
}

/// Extracts the session name from a path inside `sessions/`, if it is a
/// plain top-level `<name>.json` (the shape named sessions use).
pub fn name_from_path(path: &Path) -> Option<String> {
    if path.parent()?.to_str()? != SESSIONS_DIR {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    let ext = path.extension()?.to_str()?;
    (ext == "json" && valid_name(stem).is_ok()).then(|| stem.to_owned())
}

fn valid_name(name: &str) -> Result<()> {
    anyhow::ensure!(
        !name.is_empty()
            && !name.starts_with('.')
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_')),
        "invalid session name '{name}' — use letters, digits, '-' or '_'"
    );
    Ok(())
}

/// Lists every `*.json` in `sessions/`, newest-modified first.
/// Files that are not valid session JSON still appear, with 0 messages.
pub fn list() -> Vec<SessionEntry> {
    list_in(Path::new(SESSIONS_DIR))
}

pub fn list_in(dir: &Path) -> Vec<SessionEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<SessionEntry> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            let name = p.file_stem()?.to_str()?.to_owned();
            let meta = std::fs::metadata(&p).ok();
            let modified = meta.as_ref().and_then(|m| m.modified().ok());
            Some(SessionEntry {
                name,
                path: p,
                messages: 0,
                updated_at: None,
                modified,
            })
        })
        .collect();
    for entry in &mut out {
        if let Ok((messages, updated_at)) = peek(&entry.path) {
            entry.messages = messages;
            entry.updated_at = updated_at;
        }
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.modified));
    out
}

#[derive(Deserialize)]
struct SessionMeta {
    #[serde(default)]
    messages: Vec<serde_json::Value>,
    #[serde(default)]
    updated_at: Option<String>,
}

fn peek(path: &Path) -> Result<(usize, Option<String>)> {
    // Guard against very large session files.
    const MAX_PEEK_BYTES: u64 = 10 * 1024 * 1024; // 10 MiB
    let meta = std::fs::metadata(path)?;
    anyhow::ensure!(
        meta.len() <= MAX_PEEK_BYTES,
        "session file too large ({} bytes, limit {})",
        meta.len(),
        MAX_PEEK_BYTES
    );
    let raw = std::fs::read_to_string(path)?;
    let meta: SessionMeta = serde_json::from_str(&raw)?;
    Ok((meta.messages.len(), meta.updated_at))
}

/// Loads a named session, restoring its conversation and timestamps.
pub fn load_named(name: &str) -> Result<Session> {
    let path = named_session_path(name)?;
    Session::load_from(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_map_into_sessions_dir() {
        assert_eq!(
            named_session_path("work").unwrap(),
            PathBuf::from("sessions/work.json")
        );
        assert_eq!(
            named_session_path("my-chat_2").unwrap(),
            PathBuf::from("sessions/my-chat_2.json")
        );
    }

    #[test]
    fn bad_names_are_rejected() {
        assert!(named_session_path("").is_err());
        assert!(named_session_path("..").is_err());
        assert!(named_session_path(".hidden").is_err());
        assert!(named_session_path("a/b").is_err());
        assert!(named_session_path(r"a\b").is_err());
        assert!(named_session_path("a b").is_err());
    }

    #[test]
    fn name_from_path_roundtrips() {
        let p = PathBuf::from("sessions/work.json");
        assert_eq!(name_from_path(&p).as_deref(), Some("work"));
        assert_eq!(name_from_path(Path::new("other/x.json")), None);
        assert_eq!(name_from_path(Path::new("sessions/../etc.json")), None);
        assert_eq!(name_from_path(Path::new("sessions/x.txt")), None);
    }

    #[test]
    fn list_reads_a_temp_dir() {
        let dir = std::env::temp_dir().join(format!("govinda-sessions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::new("sys");
        s.save_to(&dir.join("b.json")).unwrap();
        s.save_to(&dir.join("a.json")).unwrap();
        std::fs::write(dir.join("notes.txt"), "not a session").unwrap();
        let entries = list_in(&dir);
        assert_eq!(entries.len(), 2, "only *.json files");
        assert_eq!(entries[0].messages, 0);
        assert!(entries[0].updated_at.is_some(), "timestamp recorded");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_named_roundtrips() {
        let mut s = Session::new("sys");
        s.push_user("hi");
        let path = named_session_path("test-load").unwrap();
        s.save_to(&path).unwrap();
        let loaded = load_named("test-load").unwrap();
        assert_eq!(loaded.messages().len(), 1);
        assert!(loaded.updated_at().is_some());
        let _ = std::fs::remove_file(path);
    }
}
