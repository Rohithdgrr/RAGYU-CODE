//! Checkpointing + Session Rewind
//!
//! Provides automatic checkpoints of session state at key moments:
//! - Before each agent turn
//! - Before applying edits
//! - On user request (`/checkpoint`)
//!
//! Users can rewind to any previous checkpoint (`/rewind <n>`).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// A snapshot of session state at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Monotonically increasing ID.
    pub id: usize,
    /// Human-readable label (e.g., "before turn 5" or "before /apply").
    pub label: String,
    /// ISO-8601 timestamp.
    pub timestamp: String,
    /// Serialized session messages.
    pub messages: Vec<crate::api::Message>,
    /// Number of messages at checkpoint time.
    pub message_count: usize,
}

/// Collection of checkpoints for the current session.
#[derive(Debug, Default)]
pub struct CheckpointStore {
    checkpoints: Vec<Checkpoint>,
    next_id: usize,
    /// Maximum checkpoints to keep (FIFO eviction).
    max_checkpoints: usize,
}

impl CheckpointStore {
    pub fn new() -> Self {
        Self {
            checkpoints: Vec::new(),
            next_id: 1,
            max_checkpoints: 50,
        }
    }

    /// Creates a checkpoint with the given label.
    #[allow(clippy::expect_used)] // safe: we just pushed to the vec
    pub fn checkpoint(&mut self, label: &str, messages: &[crate::api::Message]) -> &Checkpoint {
        let cp = Checkpoint {
            id: self.next_id,
            label: label.to_owned(),
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            messages: messages.to_vec(),
            message_count: messages.len(),
        };
        self.next_id += 1;
        self.checkpoints.push(cp);
        // Evict oldest if over limit.
        if self.checkpoints.len() > self.max_checkpoints {
            self.checkpoints.remove(0);
        }
        self.checkpoints.last().expect("just pushed checkpoint")
    }

    /// Returns all checkpoints (newest last).
    pub fn list(&self) -> &[Checkpoint] {
        &self.checkpoints
    }

    /// Returns the checkpoint with the given ID, or the most recent one if `None`.
    pub fn get(&self, id: Option<usize>) -> Option<&Checkpoint> {
        match id {
            Some(id) => self.checkpoints.iter().find(|c| c.id == id),
            None => self.checkpoints.last(),
        }
    }

    /// Rewinds to a specific checkpoint, returning its messages.
    pub fn rewind_to(&mut self, id: usize) -> Option<Vec<crate::api::Message>> {
        let idx = self.checkpoints.iter().position(|c| c.id == id)?;
        let cp = &self.checkpoints[idx];
        let messages = cp.messages.clone();
        // Remove all checkpoints after this one.
        self.checkpoints.truncate(idx + 1);
        Some(messages)
    }

    /// Clears all checkpoints.
    pub fn clear(&mut self) {
        self.checkpoints.clear();
        self.next_id = 1;
    }

    /// Returns the number of stored checkpoints.
    pub fn len(&self) -> usize {
        self.checkpoints.len()
    }

    /// Returns true if there are no checkpoints.
    pub fn is_empty(&self) -> bool {
        self.checkpoints.is_empty()
    }
}

/// Persisted checkpoint on disk.
#[derive(Debug, Serialize, Deserialize)]
struct PersistedCheckpoint {
    id: usize,
    label: String,
    timestamp: String,
    messages: Vec<crate::api::Message>,
}

/// Saves a checkpoint to the `.govinda/checkpoints/` directory.
pub fn save_checkpoint(workspace: &Path, cp: &Checkpoint) -> Result<PathBuf> {
    let dir = workspace.join(".govinda").join("checkpoints");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("cp-{}.json", cp.id));
    let data = PersistedCheckpoint {
        id: cp.id,
        label: cp.label.clone(),
        timestamp: cp.timestamp.clone(),
        messages: cp.messages.clone(),
    };
    let json = serde_json::to_string_pretty(&data).context("failed to serialize checkpoint")?;
    // Atomic write to avoid corruption on crash.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &json)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Loads persisted checkpoints from the `.govinda/checkpoints/` directory.
pub fn load_checkpoints(workspace: &Path) -> Result<Vec<Checkpoint>> {
    let dir = workspace.join(".govinda").join("checkpoints");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut checkpoints = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .context("cannot read checkpoints dir")?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .collect();
    entries.sort_by_key(|e| {
        e.file_name()
            .to_string_lossy()
            .strip_prefix("cp-")
            .and_then(|s| s.strip_suffix(".json"))
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0)
    });
    for entry in entries {
        let raw = fs::read_to_string(entry.path())?;
        if let Ok(data) = serde_json::from_str::<PersistedCheckpoint>(&raw) {
            checkpoints.push(Checkpoint {
                id: data.id,
                label: data.label,
                timestamp: data.timestamp,
                messages: data.messages.clone(),
                message_count: data.messages.len(),
            });
        }
    }
    Ok(checkpoints)
}

/// Prunes old checkpoints on disk, keeping only the most recent N.
pub fn prune_checkpoints(workspace: &Path, keep: usize) -> Result<()> {
    let dir = workspace.join(".govinda").join("checkpoints");
    if !dir.is_dir() {
        return Ok(());
    }
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .context("cannot read checkpoints dir")?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .collect();
    entries.sort_by_key(|e| {
        e.file_name()
            .to_string_lossy()
            .strip_prefix("cp-")
            .and_then(|s| s.strip_suffix(".json"))
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0)
    });
    // Remove oldest (first) entries beyond `keep`.
    let to_remove = entries.len().saturating_sub(keep);
    for entry in entries.into_iter().take(to_remove) {
        let _ = fs::remove_file(entry.path());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Message;

    fn test_messages() -> Vec<Message> {
        vec![
            Message::user("hello"),
            Message::assistant("hi there"),
            Message::user("how are you"),
        ]
    }

    #[test]
    fn checkpoint_store_basic_flow() {
        let mut store = CheckpointStore::new();
        assert!(store.is_empty());

        let msgs = test_messages();
        store.checkpoint("before turn 1", &msgs);
        store.checkpoint("before turn 2", &msgs);
        assert_eq!(store.len(), 2);

        let cp = store.get(Some(1)).unwrap();
        assert_eq!(cp.label, "before turn 1");
        assert_eq!(cp.message_count, 3);

        // Rewind to checkpoint 1
        let rewound = store.rewind_to(1).unwrap();
        assert_eq!(rewound.len(), 3);
        assert_eq!(store.len(), 1); // checkpoint 2 removed
    }

    #[test]
    fn prune_removes_oldest() {
        let dir = std::env::temp_dir().join("govinda-cp-prune");
        let _ = fs::remove_dir_all(&dir);
        let cp1 = Checkpoint {
            id: 1,
            label: "first".into(),
            timestamp: "10:00:00".into(),
            messages: vec![],
            message_count: 0,
        };
        save_checkpoint(&dir, &cp1).unwrap();
        let cp2 = Checkpoint {
            id: 2,
            label: "second".into(),
            timestamp: "10:01:00".into(),
            messages: vec![],
            message_count: 0,
        };
        save_checkpoint(&dir, &cp2).unwrap();

        prune_checkpoints(&dir, 1).unwrap();
        let loaded = load_checkpoints(&dir).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].label, "second");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_sorts_numerically_not_lexically() {
        // cp-10.json must sort after cp-2.json, not before
        let dir = std::env::temp_dir().join("govinda-cp-sort");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Manually create checkpoint files with IDs that would sort
        // incorrectly lexicographically (cp-10 < cp-2)
        for id in [1, 2, 10, 3] {
            let cp = Checkpoint {
                id,
                label: format!("cp-{id}"),
                timestamp: format!("10:0{id}:00"),
                messages: vec![],
                message_count: 0,
            };
            save_checkpoint(&dir, &cp).unwrap();
        }

        // Keep only 2 — should remove the oldest (1 and 2), not 10
        prune_checkpoints(&dir, 2).unwrap();
        let loaded = load_checkpoints(&dir).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, 3);
        assert_eq!(loaded[1].id, 10);
        let _ = fs::remove_dir_all(&dir);
    }
}
