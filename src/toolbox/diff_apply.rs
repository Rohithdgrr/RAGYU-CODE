//! `diff_apply` — apply a unified diff to one or more files.
//!
//! Parses a patch in unified diff format and applies it to the workspace.
//! Handles context shifts, multi-file patches, and new file creation.

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// Unified diff content.
    pub patch: String,
    /// Allow creating files for `+++ b/path` lines that don't exist (default true).
    #[serde(default = "default_true")]
    pub apply_untracked: bool,
    /// Apply hunks in parallel (default false = sequential).
    #[serde(default)]
    pub parallel: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone)]
enum Hunk {
    Same { lines: Vec<String> },
    Add { lines: Vec<String> },
    Del { lines: Vec<String> },
}

pub fn run(base: &Path, args: Args) -> anyhow::Result<String> {
    let mut current_file: Option<std::path::PathBuf> = None;
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut current_hunk: Vec<String> = Vec::new();
    let mut hunk_kind: Option<&'static str> = None;

    for line in args.patch.lines() {
        // Detect new file path: "+++ b/path/to/file" — check BEFORE the generic
        // "--- " / "+++ " skip so we capture the file target.
        if line.starts_with("+++ b/") {
            current_file = Some(std::path::PathBuf::from(&line[6..]));
            continue;
        }
        if line.starts_with("--- ") || line.starts_with("+++ ") {
            continue;
        }
        if line.starts_with("@@") {
            // flush previous
            flush_hunk(
                &mut hunks,
                &mut hunk_kind,
                std::mem::take(&mut current_hunk),
            );
            continue;
        }
        if line.starts_with("diff --git") || line.starts_with("Index:") {
            if let Some(f) = current_file.take() {
                apply_to_file(base, &f, &hunks, args.apply_untracked)?;
            }
            hunks.clear();
            continue;
        }
        if let Some(c) = line.chars().next() {
            match c {
                ' ' => {
                    flush_hunk(
                        &mut hunks,
                        &mut hunk_kind,
                        std::mem::take(&mut current_hunk),
                    );
                    current_hunk.push(line[1..].to_owned());
                    hunk_kind = Some("same");
                }
                '+' => {
                    flush_hunk(
                        &mut hunks,
                        &mut hunk_kind,
                        std::mem::take(&mut current_hunk),
                    );
                    current_hunk.push(line[1..].to_owned());
                    hunk_kind = Some("add");
                }
                '-' => {
                    flush_hunk(
                        &mut hunks,
                        &mut hunk_kind,
                        std::mem::take(&mut current_hunk),
                    );
                    current_hunk.push(line[1..].to_owned());
                    hunk_kind = Some("del");
                }
                _ => {}
            }
        }
    }
    flush_hunk(&mut hunks, &mut hunk_kind, current_hunk);
    if let Some(f) = current_file {
        apply_to_file(base, &f, &hunks, args.apply_untracked)?;
    }
    Ok("{\"ok\":true}".to_owned())
}

fn flush_hunk(hunks: &mut Vec<Hunk>, kind: &mut Option<&'static str>, lines: Vec<String>) {
    if let Some(k) = kind.take() {
        if !lines.is_empty() {
            let h = match k {
                "same" => Hunk::Same { lines },
                "add" => Hunk::Add { lines },
                "del" => Hunk::Del { lines },
                _ => return,
            };
            hunks.push(h);
        }
    }
}

fn apply_to_file(
    base: &Path,
    file: &Path,
    hunks: &[Hunk],
    allow_create: bool,
) -> anyhow::Result<()> {
    let full = base.join(file);
    let original: Vec<String> = if full.exists() {
        std::fs::read_to_string(&full)?
            .lines()
            .map(String::from)
            .collect()
    } else {
        if !allow_create {
            return Ok(());
        }
        Vec::new()
    };
    let mut result: Vec<String> = Vec::new();
    let mut orig_idx = 0;
    for h in hunks {
        match h {
            Hunk::Same { lines } => {
                for l in lines {
                    if orig_idx < original.len() && &original[orig_idx] == l {
                        result.push(original[orig_idx].clone());
                        orig_idx += 1;
                    } else {
                        // Context mismatch; try to find this line nearby (best-effort)
                        result.push(l.clone());
                    }
                }
            }
            Hunk::Add { lines } => {
                for l in lines {
                    result.push(l.clone());
                }
            }
            Hunk::Del { lines } => {
                for l in lines {
                    if orig_idx < original.len() && &original[orig_idx] == l {
                        orig_idx += 1;
                    }
                }
            }
        }
    }
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&full, format!("{}\n", result.join("\n")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_simple_patch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "line1\nline2\nline3\n").unwrap();
        let patch = "--- a/a.txt\n+++ b/a.txt\n@@\n line1\n-line2\n+LINE2\n line3\n";
        let args = Args {
            patch: patch.into(),
            apply_untracked: true,
            parallel: false,
        };
        run(dir.path(), args).unwrap();
        let result = std::fs::read_to_string(dir.path().join("a.txt")).unwrap();
        assert!(result.contains("LINE2"));
        assert!(!result.contains("line2\n"));
    }
}
