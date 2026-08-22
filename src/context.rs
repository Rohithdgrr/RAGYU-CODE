//! Context-aware windowing: detects which workspace files a user prompt is
//! "about" and renders them as an injection block for the context window.
//!
//! Heuristics, in order:
//!   1. Path-like tokens in the input (`src/api.rs`, `tools.rs`) that resolve
//!      to real files inside the workspace.
//!   2. The workspace manifest (`Cargo.toml`, `package.json`, …), so the
//!      model always sees dependencies alongside mentioned source files.
//!   3. Sibling source files in the same directory (capped) — modules of the
//!      same subsystem usually travel together.
//!
//! Everything is capped: an injection must never crowd out conversation
//! history from the token budget.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Most files one injection may include (mentioned + manifest + siblings).
const MAX_FILES: usize = 6;
/// Sibling source files considered per mentioned file.
const MAX_SIBLINGS: usize = 2;
/// Character cap on the rendered injection block.
pub const MAX_INJECTION_CHARS: usize = 12_000;
/// Extensions eligible for sibling pickup.
const SOURCE_EXTS: [&str; 5] = ["rs", "py", "js", "ts", "tsx"];
/// Manifests added whenever any source file is mentioned.
const MANIFESTS: [&str; 4] = ["Cargo.toml", "package.json", "pyproject.toml", "go.mod"];

#[allow(clippy::expect_used)] // static, hand-checked pattern
fn path_like_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Tokens with path separators and/or a known-ish extension: matches
        // `src/api.rs`, `Cargo.toml`, `a/b/c.py`, but not bare words.
        regex::Regex::new(r"[A-Za-z0-9_][A-Za-z0-9_.\-]*(?:/[A-Za-z0-9_.\-]+|\.[A-Za-z]{1,5})")
            .expect("valid regex")
    })
}

fn is_source(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| SOURCE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Extracts workspace-relative paths mentioned in `input` that actually
/// exist on disk under `base`. Order preserved; duplicates removed.
pub fn mentioned_files(input: &str, base: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for tok in path_like_re().find_iter(input) {
        let raw = tok.as_str().trim_matches('.');
        if raw.len() < 4 {
            continue;
        }
        let candidate = base.join(raw.replace('/', std::path::MAIN_SEPARATOR_STR));
        if candidate.is_file() && !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
}

/// Computes the injection set for a user prompt: mentioned files first,
/// then manifests (when sources are involved), then same-dir siblings.
pub fn relevant_files(input: &str, base: &Path) -> Vec<PathBuf> {
    let mut files = mentioned_files(input, base);
    let has_source = files.iter().any(|p| is_source(p));

    if has_source {
        for m in MANIFESTS {
            let path = base.join(m);
            if path.is_file() && !files.contains(&path) {
                files.push(path);
                break; // one manifest is enough
            }
        }
    }

    // Siblings: same directory as each mentioned source file.
    let mut siblings: Vec<PathBuf> = Vec::new();
    for f in files.clone() {
        if !is_source(&f) {
            continue;
        }
        let Some(dir) = f.parent() else { continue };
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut near: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_source(p) && *p != f && !files.contains(p))
            .collect();
        near.sort();
        siblings.extend(near.into_iter().take(MAX_SIBLINGS));
    }
    for s in siblings {
        if files.len() >= MAX_FILES {
            break;
        }
        if !files.contains(&s) {
            files.push(s);
        }
    }
    files.truncate(MAX_FILES);
    files
}

/// Renders the injection block appended to the system message, or `None`
/// when nothing relevant was found. Each file appears as a fenced code
/// block with its workspace-relative path; oversized files are truncated
/// with a visible marker so partial content is never mistaken for whole.
pub fn build_injection(files: &[PathBuf], base: &Path) -> Option<String> {
    if files.is_empty() {
        return None;
    }
    let mut out =
        String::from("[workspace context] Files referenced by the user, provided for reference:\n");
    for f in files {
        let rel = f
            .strip_prefix(base)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| f.to_string_lossy().replace('\\', "/"));
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        if bytes.contains(&0) {
            continue; // binary
        }
        let text = String::from_utf8_lossy(&bytes);
        out.push_str(&format!("\n--- {rel} ---\n```\n"));
        let budget = MAX_INJECTION_CHARS.saturating_sub(out.chars().count());
        if text.chars().count() > budget {
            let cut: String = text.chars().take(budget).collect();
            out.push_str(&cut);
            out.push_str("\n…(truncated)\n```\n");
        } else {
            out.push_str(text.trim_end());
            out.push_str("\n```\n");
        }
        if out.chars().count() >= MAX_INJECTION_CHARS {
            break;
        }
    }
    (out.chars().count() > 60).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempWs(PathBuf);
    impl TempWs {
        fn new(tag: &str) -> Self {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "govinda-ctx-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempWs {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn mentions_resolve_only_existing_workspace_files() {
        let ws = TempWs::new("mentions");
        fs::write(ws.0.join("api.rs"), "fn a() {}\n").unwrap();
        let hits = mentioned_files("look at src/api.rs and docs/missing.md", &ws.0);
        assert!(hits.is_empty(), "{hits:?}");

        fs::create_dir_all(ws.0.join("src")).unwrap();
        fs::write(ws.0.join("src/api.rs"), "fn a() {}\n").unwrap();
        let hits = mentioned_files("look at src/api.rs and Cargo.toml", &ws.0);
        assert_eq!(hits, vec![ws.0.join("src").join("api.rs")]);
    }

    #[test]
    fn relevant_files_adds_manifest_and_siblings() {
        let ws = TempWs::new("relevant");
        fs::create_dir_all(ws.0.join("src")).unwrap();
        fs::write(ws.0.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        fs::write(ws.0.join("src/provider.rs"), "fn p() {}\n").unwrap();
        fs::write(ws.0.join("src/api.rs"), "fn a() {}\n").unwrap();
        fs::write(ws.0.join("src/main.rs"), "fn main() {}\n").unwrap();

        let files = relevant_files("fix bug in src/api.rs", &ws.0);
        let rels: Vec<String> = files
            .iter()
            .map(|f| {
                f.strip_prefix(&ws.0)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(rels[0], "src/api.rs");
        assert!(rels.contains(&"Cargo.toml".to_owned()), "{rels:?}");
        // up to two siblings picked deterministically (sorted)
        let siblings: Vec<&String> = rels[1..].iter().filter(|r| r.starts_with("src/")).collect();
        assert!(siblings.len() <= 2, "{rels:?}");
        assert!(!siblings.is_empty(), "{rels:?}");
    }

    #[test]
    fn plain_prompts_inject_nothing() {
        let ws = TempWs::new("plain");
        assert!(relevant_files("what is a mutex?", &ws.0).is_empty());
        assert!(build_injection(&[], &ws.0).is_none());
    }

    #[test]
    fn injection_truncates_and_labels_files() {
        let ws = TempWs::new("inject");
        fs::write(ws.0.join("big.txt"), "x".repeat(50_000)).unwrap();
        let files = vec![ws.0.join("big.txt")];
        let block = build_injection(&files, &ws.0).expect("block");
        assert!(block.contains("--- big.txt ---"), "{block}");
        assert!(block.contains("(truncated)"), "{block}");
        assert!(block.chars().count() <= MAX_INJECTION_CHARS + 200);
    }
}
