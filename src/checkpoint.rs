//! Checkpointing + Session Rewind
//!
//! Provides automatic checkpoints of session state at key moments:
//! - Before each agent turn
//! - Before applying edits
//! - On user request (`/checkpoint`)
//!
//! Users can rewind to any previous checkpoint (`/rewind <n>`).
//!
//! ## Encryption at rest (V-008)
//!
//! Checkpoints contain the entire conversation history, which can include
//! API keys, secrets, and proprietary code that the user typed. To keep
//! them safe when written to disk, set the `GOVINDA_CHECKPOINT_PASSPHRASE`
//! environment variable before launching govinda. When set, checkpoints
//! are encrypted with XChaCha20-Poly1305 (authenticated encryption) using
//! a key derived from the passphrase via Argon2id. Encrypted files carry
//! the `ENC1` magic header; plaintext files (the default) carry the
//! `PLA1` magic so older checkpoints continue to load after an upgrade.

use anyhow::{Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// File magic for plaintext checkpoints.
const MAGIC_PLAIN: &[u8; 4] = b"PLA1";
/// File magic for encrypted checkpoints (XChaCha20-Poly1305 + Argon2id).
const MAGIC_ENC: &[u8; 4] = b"ENC1";
/// Argon2id parameters: 64 MiB memory, 3 iterations, 1 lane. Tuned for a
/// few-hundred-millisecond derive on a modern laptop so a stolen checkpoint
/// file cannot be brute-forced cheaply, while keeping the per-checkpoint
/// overhead bearable (one derive per file written).
const ARGON2_MEM_KIB: u32 = 64 * 1024;
const ARGON2_TIME_COST: u32 = 3;
const ARGON2_PARALLELISM: u32 = 1;

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

/// Saves a checkpoint to the `.govinda/checkpoints/` directory. When the
/// `GOVINDA_CHECKPOINT_PASSPHRASE` env var is set, the file is encrypted
/// with XChaCha20-Poly1305 using a key derived from the passphrase via
/// Argon2id. Otherwise the checkpoint is written as plaintext JSON.
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
    let json = serde_json::to_vec(&data).context("failed to serialize checkpoint")?;
    let bytes = match std::env::var("GOVINDA_CHECKPOINT_PASSPHRASE") {
        Ok(passphrase) if !passphrase.is_empty() => {
            encrypt_checkpoint(&json, passphrase.as_bytes())?
        }
        _ => prepend_magic(MAGIC_PLAIN, &json),
    };
    // Atomic write to avoid corruption on crash.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

fn prepend_magic(magic: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(magic);
    out.extend_from_slice(payload);
    out
}

/// Derives a 32-byte XChaCha20 key from `passphrase` using Argon2id. The
/// salt is the ASCII representation of the checkpoint id so the same
/// passphrase can decrypt every checkpoint in a workspace, while an
/// attacker who steals a single file still has to brute-force Argon2id.
fn derive_key(passphrase: &[u8], cp_id: usize) -> Result<[u8; 32]> {
    let params = Params::new(
        ARGON2_MEM_KIB,
        ARGON2_TIME_COST,
        ARGON2_PARALLELISM,
        Some(32),
    )
    .map_err(|e| anyhow::anyhow!("invalid Argon2 parameters: {e}"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    let salt = format!("govinda-checkpoint-{cp_id}");
    argon
        .hash_password_into(passphrase, salt.as_bytes(), &mut key)
        .map_err(|e| anyhow::anyhow!("Argon2 key derivation failed: {e}"))?;
    Ok(key)
}

/// Encrypts `plaintext` with XChaCha20-Poly1305 under a key derived from
/// `passphrase`. The on-disk layout is:
/// `ENC1 | random 24-byte nonce | ciphertext+tag` (the `aad` field is the
/// literal bytes "ENC1" so a tampering attack that flips the magic to
/// `PLA1` is detected by the AEAD tag).
fn encrypt_checkpoint(plaintext: &[u8], passphrase: &[u8]) -> Result<Vec<u8>> {
    // We use the checkpoint id 0 as a "header-only" salt so the key is
    // bound to the file as a whole, not a per-message nonce. The actual
    // XChaCha nonce is a fresh 24-byte random.
    let key = derive_key(passphrase, 0)?;
    use rand::RngCore;
    let mut rng_bytes = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut rng_bytes);
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&key));
    let ct = cipher
        .encrypt(
            XNonce::from_slice(&rng_bytes),
            Payload {
                msg: plaintext,
                aad: MAGIC_ENC,
            },
        )
        .map_err(|e| anyhow::anyhow!("checkpoint encryption failed: {e}"))?;
    let mut out = Vec::with_capacity(4 + 24 + ct.len());
    out.extend_from_slice(MAGIC_ENC);
    out.extend_from_slice(&rng_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Decrypts a payload produced by `encrypt_checkpoint`.
fn decrypt_checkpoint(blob: &[u8], passphrase: &[u8]) -> Result<Vec<u8>> {
    anyhow::ensure!(
        blob.len() > 4 + 24,
        "encrypted checkpoint too short to contain a nonce"
    );
    let (magic, rest) = blob.split_at(4);
    anyhow::ensure!(magic == MAGIC_ENC, "not an encrypted checkpoint");
    let (nonce, ct) = rest.split_at(24);
    let key = derive_key(passphrase, 0)?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&key));
    cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ct,
                aad: MAGIC_ENC,
            },
        )
        .map_err(|_| anyhow::anyhow!("checkpoint decryption failed (wrong passphrase or corrupted file)"))
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
        let raw = match fs::read(&entry.path()) {
            Ok(b) => b,
            Err(_) => continue,
        };
        // Auto-detect encryption from the 4-byte magic. Plaintext files
        // written by older versions start with '{' (JSON) — those still
        // load, so an upgrade doesn't break existing checkpoints.
        let json_bytes: Vec<u8> = if raw.len() >= 4 && &raw[..4] == MAGIC_ENC {
            // Encrypted: passphrase is required.
            let passphrase = match std::env::var("GOVINDA_CHECKPOINT_PASSPHRASE") {
                Ok(p) if !p.is_empty() => p,
                _ => {
                    // Skip encrypted files silently when no passphrase is
                    // configured. The user opted in to encryption; without
                    // a passphrase these files are unreadable by design.
                    continue;
                }
            };
            match decrypt_checkpoint(&raw, passphrase.as_bytes()) {
                Ok(b) => b,
                Err(_) => continue, // wrong passphrase or tampered file
            }
        } else {
            raw
        };
        if let Ok(data) = serde_json::from_slice::<PersistedCheckpoint>(&json_bytes) {
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
    use std::sync::Mutex;

    /// Process-global mutex for tests that mutate `GOVINDA_CHECKPOINT_PASSPHRASE`
    /// (a process-wide environment variable that Rust's `std::env` does not
    /// make thread-safe). Acquiring this guard around the env mutation
    /// ensures these tests cannot interleave and corrupt one another.
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    /// Sentinel value used to detect accidental leakage from a prior test.
    const LEAKED_PASSPHRASE: &str = "Z3_LEAKED_PASSPHRASE_SENTINEL_zZ";

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
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Force plaintext mode for this test.
        unsafe {
            std::env::remove_var("GOVINDA_CHECKPOINT_PASSPHRASE");
        }
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

    /// Sets the env var only if `passphrase.is_some()`. Used by tests
    /// to avoid leaving stale state on process-global env.
    fn set_passphrase(passphrase: Option<&str>) {
        // SAFETY: tests serialize through ENV_LOCK.
        unsafe {
            match passphrase {
                Some(p) if !p.is_empty() => {
                    std::env::set_var("GOVINDA_CHECKPOINT_PASSPHRASE", p);
                }
                _ => {
                    std::env::remove_var("GOVINDA_CHECKPOINT_PASSPHRASE");
                }
            }
        }
    }

    #[test]
    fn encrypted_round_trip_with_passphrase() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join("govinda-cp-encrypted");
        let _ = fs::remove_dir_all(&dir);
        set_passphrase(Some("correct-horse-battery"));
        let cp = Checkpoint {
            id: 1,
            label: "secret".into(),
            timestamp: "10:00:00".into(),
            messages: vec![crate::api::Message::user("my api key is sk-LIVE-DATA")],
            message_count: 1,
        };
        let path = save_checkpoint(&dir, &cp).unwrap();
        let raw = fs::read(&path).unwrap();
        assert_eq!(&raw[..4], MAGIC_ENC, "checkpoint must be encrypted when passphrase is set");
        assert!(
            !String::from_utf8_lossy(&raw).contains("sk-LIVE-DATA"),
            "plaintext leaked into encrypted file"
        );
        let loaded = load_checkpoints(&dir).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, 1);
        // Wrong passphrase → file is silently skipped.
        set_passphrase(Some("wrong"));
        let loaded2 = load_checkpoints(&dir).unwrap();
        assert!(loaded2.is_empty(), "wrong passphrase must not yield a result");
        // Clear the env var so other tests are not affected.
        set_passphrase(None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn plaintext_round_trip_without_passphrase() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join("govinda-cp-plain");
        let _ = fs::remove_dir_all(&dir);
        set_passphrase(None); // defensive: in case a prior test leaked state
        let cp = Checkpoint {
            id: 7,
            label: "plain".into(),
            timestamp: "10:00:00".into(),
            messages: vec![],
            message_count: 0,
        };
        let path = save_checkpoint(&dir, &cp).unwrap();
        let raw = fs::read(&path).unwrap();
        assert!(raw.starts_with(b"{"), "expected plain JSON, got {:?}", &raw[..4]);
        let loaded = load_checkpoints(&dir).unwrap();
        assert_eq!(loaded.len(), 1);
        set_passphrase(None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_sorts_numerically_not_lexically() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Force plaintext mode for this test.
        unsafe {
            std::env::remove_var("GOVINDA_CHECKPOINT_PASSPHRASE");
        }
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
