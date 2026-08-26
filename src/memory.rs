//! Project Memory: AGENTS.md / CLAUDE.md + .govinda/memory.md
//!
//! At session startup, loads project-specific instructions from these files
//! (if present) and injects them into the system prompt:
//!
//! - `AGENTS.md` (project root) — general agent instructions
//! - `CLAUDE.md` (project root) — Claude-style instructions  
//! - `.govinda/memory.md` (project root) — persistent memory/notes
//!
//! These files follow a simple convention: the entire content is used as
//! system context. No special parsing — just raw markdown text.

use anyhow::Result;
use std::fs;
use std::path::Path;

/// Maximum size of a memory file to prevent context overflow.
const MAX_MEMORY_FILE_BYTES: usize = 32 * 1024;

/// Loaded project memory from all known sources.
#[derive(Debug, Clone, Default)]
pub struct ProjectMemory {
    /// Content from `AGENTS.md`.
    pub agents_md: Option<String>,
    /// Content from `CLAUDE.md`.
    pub claude_md: Option<String>,
    /// Content from `.govinda/memory.md`.
    pub govinda_memory: Option<String>,
}

impl ProjectMemory {
    /// Loads all project memory files from the given workspace root.
    pub fn load(workspace: &Path) -> Self {
        Self {
            agents_md: load_memory_file(workspace, "AGENTS.md"),
            claude_md: load_memory_file(workspace, "CLAUDE.md"),
            govinda_memory: load_memory_file(
                &workspace.join(".govinda"),
                "memory.md",
            ),
        }
    }

    /// Returns true if any memory was loaded.
    pub fn has_content(&self) -> bool {
        self.agents_md.is_some() || self.claude_md.is_some() || self.govinda_memory.is_some()
    }

    /// Combines all loaded memory into a single system prompt suffix.
    /// Returns `None` if no memory files were found.
    pub fn to_system_suffix(&self) -> Option<String> {
        if !self.has_content() {
            return None;
        }
        let mut parts = Vec::new();

        if let Some(ref agents) = self.agents_md {
            parts.push(format!(
                "# Project Instructions (AGENTS.md)\n\n{}",
                agents.trim()
            ));
        }
        if let Some(ref claude) = self.claude_md {
            parts.push(format!(
                "# Project Instructions (CLAUDE.md)\n\n{}",
                claude.trim()
            ));
        }
        if let Some(ref memory) = self.govinda_memory {
            parts.push(format!(
                "# Project Memory (.govinda/memory.md)\n\n{}",
                memory.trim()
            ));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n---\n\n"))
        }
    }

    /// Appends a note to `.govinda/memory.md`.
    pub fn append_note(workspace: &Path, note: &str) -> Result<()> {
        let dir = workspace.join(".govinda");
        fs::create_dir_all(&dir)?;
        let path = dir.join("memory.md");
        let existing = fs::read_to_string(&path).unwrap_or_default();
        let separator = if existing.trim().is_empty() {
            String::new()
        } else {
            "\n\n".to_owned()
        };
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M");
        let content = format!("{existing}{separator}## {timestamp}\n\n{note}\n");
        fs::write(&path, content)?;
        Ok(())
    }
}

/// Free-function wrappers that operate on the process current working
/// directory. Used by the `remember` / `forget` tools so the agent can
/// curate the project memory without going through the slash command.
pub fn append_note(note: &str) {
    if let Ok(cwd) = std::env::current_dir() {
        let _ = ProjectMemory::append_note(&cwd, note);
    }
}

/// Removes all sections of `.govinda/memory.md` whose body (case-insensitive)
/// contains `needle`. Returns the number of sections removed.
pub fn remove_note(needle: &str) -> usize {
    let Ok(cwd) = std::env::current_dir() else { return 0 };
    let path = cwd.join(".govinda").join("memory.md");
    let Ok(content) = std::fs::read_to_string(&path) else { return 0 };
    let lower = needle.to_lowercase();
    let mut kept = Vec::new();
    let mut removed = 0usize;
    // Sections are separated by `## YYYY-MM-DD HH:MM` headers.
    let mut current: Vec<&str> = Vec::new();
    let mut current_matches = false;
    for line in content.lines() {
        if line.starts_with("## ") {
            // Flush the previous section.
            if !current.is_empty() {
                if current_matches {
                    removed += 1;
                } else {
                    kept.extend(current.iter().copied());
                    kept.push(""); // blank between sections
                }
            }
            current.clear();
            current.push(line);
            current_matches = line.to_lowercase().contains(&lower);
        } else {
            if !current.is_empty() {
                current.push(line);
                if !current_matches && line.to_lowercase().contains(&lower) {
                    current_matches = true;
                }
            } else {
                kept.push(line);
            }
        }
    }
    if !current.is_empty() {
        if current_matches {
            removed += 1;
        } else {
            kept.extend(current.iter().copied());
        }
    }
    let new_content = kept.join("\n");
    let _ = std::fs::write(&path, new_content);
    removed
}

/// Loads a single memory file from the workspace, returning `None` if missing.
fn load_memory_file(workspace: &Path, relative: &str) -> Option<String> {
    let path = workspace.join(relative);
    if !path.is_file() {
        return None;
    }
    let meta = fs::metadata(&path).ok()?;
    if meta.len() > MAX_MEMORY_FILE_BYTES as u64 {
        eprintln!(
            "warning: {} is too large ({} bytes), skipping",
            path.display(),
            meta.len()
        );
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    let trimmed = content.trim().to_owned();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_memory_from_workspace() {
        let dir = std::env::temp_dir().join("govinda-memory-test");
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("AGENTS.md"), "# Instructions\nBe helpful.").unwrap();
        fs::write(dir.join("CLAUDE.md"), "# Claude Rules\nBe concise.").unwrap();
        let govinda = dir.join(".govinda");
        let _ = fs::create_dir_all(&govinda);
        fs::write(govinda.join("memory.md"), "Remember: tests pass.").unwrap();

        let mem = ProjectMemory::load(&dir);
        assert!(mem.has_content());
        assert!(mem.agents_md.as_deref().unwrap().contains("Be helpful"));
        assert!(mem.claude_md.as_deref().unwrap().contains("Be concise"));
        assert!(mem
            .govinda_memory
            .as_deref()
            .unwrap()
            .contains("tests pass"));

        let suffix = mem.to_system_suffix().unwrap();
        assert!(suffix.contains("AGENTS.md"));
        assert!(suffix.contains("CLAUDE.md"));
        assert!(suffix.contains("memory.md"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_when_no_files() {
        let dir = std::env::temp_dir().join("govinda-memory-empty");
        let _ = fs::create_dir_all(&dir);
        let mem = ProjectMemory::load(&dir);
        assert!(!mem.has_content());
        assert!(mem.to_system_suffix().is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_note_creates_file() {
        let dir = std::env::temp_dir().join("govinda-memory-append");
        let _ = fs::remove_dir_all(&dir);
        ProjectMemory::append_note(&dir, "first note").unwrap();
        ProjectMemory::append_note(&dir, "second note").unwrap();
        let content = fs::read_to_string(dir.join(".govinda/memory.md")).unwrap();
        assert!(content.contains("first note"));
        assert!(content.contains("second note"));
        let _ = fs::remove_dir_all(&dir);
    }
}
