//! In-memory workspace symbol index: functions, structs, enums, traits,
//! impls, modules, and macros mapped to `file:line` locations.
//!
//! Built by walking the workspace (respecting `.govindaignore`) and running
//! the lightweight regex extraction in [`crate::outline`] over every source
//! file — the same zero-dependency approach as `read_file` outlines. The
//! index is a navigation aid, not a compiler: stale entries after an edit
//! are acceptable, and `/scan` (or any `scan_project` tool call) refreshes
//! it on demand.

use crate::outline;
use std::path::Path;
use std::sync::{Arc, RwLock};

/// Largest source file parsed into the index; anything bigger is skipped
/// (generated blobs would slow the walk without helping navigation).
const MAX_INDEX_FILE_BYTES: u64 = 1024 * 1024;
/// Hard cap on indexed symbols so pathological workspaces can't balloon
/// memory or query results.
const MAX_SYMBOLS: usize = 20_000;
/// Most hits returned by a single `find_symbol` query.
pub const MAX_FIND_RESULTS: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// function | struct | enum | union | trait | module | macro | impl |
    /// class
    pub kind: &'static str,
    pub name: String,
    /// Workspace-relative path with '/' separators.
    pub file: String,
    /// 1-based definition line.
    pub line: usize,
}

#[derive(Debug, Default)]
pub struct SymbolIndex {
    pub symbols: Vec<Symbol>,
}

impl SymbolIndex {
    /// Walks `base` and extracts symbols from every recognized source file.
    pub fn build(base: &Path) -> Self {
        let mut symbols = Vec::new();
        for file in crate::tools::walk_files(base, base) {
            if symbols.len() >= MAX_SYMBOLS {
                break;
            }
            let Some(lang) = file.to_str().and_then(outline::detect_language) else {
                continue;
            };
            let Ok(meta) = std::fs::metadata(&file) else {
                continue;
            };
            if !meta.is_file() || meta.len() > MAX_INDEX_FILE_BYTES {
                continue;
            }
            let Ok(bytes) = std::fs::read(&file) else {
                continue;
            };
            if bytes.contains(&0) {
                continue; // binary
            }
            let rel = rel_path(base, &file);
            for sym in outline::symbols(lang, &String::from_utf8_lossy(&bytes)) {
                symbols.push(Symbol {
                    kind: sym.kind,
                    name: sym.label,
                    file: rel.clone(),
                    line: sym.line,
                });
                if symbols.len() >= MAX_SYMBOLS {
                    break;
                }
            }
        }
        Self { symbols }
    }

    /// Queries the index: exact (case-sensitive) matches first, then
    /// case-insensitive exact, then substring containment — e.g. searching
    /// `Runner` also finds `impl Runner for Config`. `kind` filters by one
    /// of the symbol kinds (or `"any"` / `None` for everything).
    pub fn find(&self, name: &str, kind: Option<&str>) -> Vec<&Symbol> {
        let needle = name.trim();
        if needle.is_empty() {
            return Vec::new();
        }
        let kind = kind.map(str::trim).filter(|k| !k.is_empty() && *k != "any");
        let candidates = self
            .symbols
            .iter()
            .filter(|s| kind.is_none_or(|k| s.kind == k));

        let mut lower: Option<String> = None;
        let mut ranked: Vec<(u8, &Symbol)> = Vec::new();
        for s in candidates {
            let rank = if s.name == needle {
                0
            } else {
                let l = lower.get_or_insert_with(|| needle.to_lowercase());
                if s.name.to_lowercase() == *l {
                    1
                } else if s.name.to_lowercase().contains(l.as_str()) || s.name.contains(needle) {
                    2
                } else {
                    continue;
                }
            };
            ranked.push((rank, s));
        }
        ranked.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.1.file.cmp(&b.1.file))
                .then(a.1.line.cmp(&b.1.line))
        });
        ranked.truncate(MAX_FIND_RESULTS);
        ranked.into_iter().map(|(_, s)| s).collect()
    }
}

fn rel_path(base: &Path, p: &Path) -> String {
    p.strip_prefix(base)
        .map(|r| r.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| p.to_string_lossy().replace('\\', "/"))
}

// -- Global store -------------------------------------------------------------

static INDEX: RwLock<Option<Arc<SymbolIndex>>> = RwLock::new(None);
/// When the index was last rebuilt (wall time) + the max mtime of files at
/// that moment. Used by `ensure` to avoid rebuilding when nothing changed,
/// and to debounce rapid calls within the same turn.
static INDEX_META: RwLock<Option<(std::time::Instant, std::time::SystemTime)>> = RwLock::new(None);

/// Rebuilds the index from disk and installs it as the current snapshot.
/// Returns the number of symbols indexed.
pub fn rebuild(base: &Path) -> usize {
    let built = Arc::new(SymbolIndex::build(base));
    let n = built.symbols.len();
    if let Ok(mut slot) = INDEX.write() {
        *slot = Some(built);
    }
    // Record build time + current wall time for mtime checks.
    if let Ok(mut meta) = INDEX_META.write() {
        *meta = Some((std::time::Instant::now(), std::time::SystemTime::now()));
    }
    n
}

/// The current snapshot, if one has been built.
pub fn current() -> Option<Arc<SymbolIndex>> {
    INDEX.read().ok().and_then(|slot| slot.clone())
}

/// Returns the latest file mtime in the workspace (quick metadata walk,
/// no file contents read). `None` if the workspace cannot be scanned.
fn max_mtime(base: &Path) -> Option<std::time::SystemTime> {
    let mut max: Option<std::time::SystemTime> = None;
    for file in crate::tools::walk_files(base, base) {
        if let Ok(meta) = std::fs::metadata(&file) {
            if let Ok(m) = meta.modified() {
                max = Some(match max {
                    Some(cur) if cur >= m => cur,
                    _ => m,
                });
            }
        }
    }
    max
}

/// Current snapshot, building one from `base` on first use so `find_symbol`
/// works even when no explicit scan happened yet.
///
/// Perf: `ensure` does not rebuild every call. It caches the last build
/// timestamp and the max file mtime at build time. A fast metadata scan
/// checks whether any file is newer than the cached mtime; if not, the
/// existing index is reused. Rapid calls within 2 seconds are debounced
/// entirely without even scanning.
pub fn ensure(base: &Path) -> Arc<SymbolIndex> {
    if let Some(idx) = current() {
        // Debounce: if we just built, reuse without scanning.
        let should_recheck = {
            if let Ok(meta) = INDEX_META.read() {
                if let Some((instant, _)) = *meta {
                    instant.elapsed() > std::time::Duration::from_secs(2)
                } else {
                    true
                }
            } else {
                true
            }
        };
        if !should_recheck {
            return idx;
        }
        // Check max mtime: if no file is newer than the build time, reuse.
        let build_time = {
            INDEX_META
                .read()
                .ok()
                .and_then(|m| m.as_ref().map(|(_, t)| *t))
        };
        if let Some(build_time) = build_time {
            if let Some(max) = max_mtime(base) {
                if max <= build_time {
                    return idx;
                }
            } else {
                // Cannot scan — be conservative and reuse.
                return idx;
            }
        } else {
            return idx;
        }
    }
    rebuild(base);
    current().unwrap_or_default()
}

/// Formats hits as compact JSON for the model: kind, name, file, line.
pub fn results_json(hits: &[&Symbol]) -> String {
    let items: Vec<serde_json::Value> = hits
        .iter()
        .map(|s| {
            serde_json::json!({
                "kind": s.kind,
                "name": s.name,
                "file": s.file,
                "line": s.line,
            })
        })
        .collect();
    serde_json::json!({ "matches": items.len(), "symbols": items }).to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Serializes tests that touch the process-global [`INDEX`] — parallel
    /// rebuilds would otherwise clobber each other's snapshots. Also taken
    /// by tools.rs executor tests that route through `rebuild`/`ensure`.
    pub(crate) fn global_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct TempWs(PathBuf);
    impl TempWs {
        fn new(tag: &str) -> Self {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "govinda-sym-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempWs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn build_indexes_rust_sources_with_locations() {
        let ws = TempWs::new("build");
        std::fs::create_dir_all(ws.0.join("src")).unwrap();
        std::fs::write(
            ws.0.join("src/api.rs"),
            "pub struct Client;\n\npub fn connect() {}\n\ntrait Transport {\n    fn send();\n}\n\nimpl Transport for Client {\n    fn send() {}\n}\n",
        )
        .unwrap();
        std::fs::write(ws.0.join("target junk.rs"), "fn noise() {}").unwrap();
        std::fs::create_dir_all(ws.0.join("target")).unwrap();

        let idx = SymbolIndex::build(&ws.0);
        let hits = idx.find("Client", None);
        assert_eq!(hits.len(), 2, "{hits:?}"); // struct + impl target
        assert_eq!(hits[0].file, "src/api.rs");
        assert_eq!(hits[0].line, 1);
        assert_eq!(hits[0].kind, "struct");

        // trait lookup by kind filter
        let traits = idx.find("Transport", Some("trait"));
        assert_eq!(traits.len(), 1);
        assert_eq!(traits[0].line, 5);

        // substring search finds the impl header too
        let impls = idx.find("transport", Some("impl"));
        assert_eq!(impls.len(), 1);
        assert!(impls[0].name.contains("Transport"));
    }

    #[test]
    fn find_prefers_exact_over_substring_and_caps_results() {
        let idx = SymbolIndex {
            symbols: vec![
                Symbol {
                    kind: "function",
                    name: "run".into(),
                    file: "a.rs".into(),
                    line: 1,
                },
                Symbol {
                    kind: "function",
                    name: "runtime".into(),
                    file: "b.rs".into(),
                    line: 2,
                },
                Symbol {
                    kind: "struct",
                    name: "Run".into(),
                    file: "c.rs".into(),
                    line: 3,
                },
            ],
        };
        let hits = idx.find("run", None);
        assert_eq!(hits[0].name, "run", "exact match ranks first: {hits:?}");
        assert!(hits.iter().any(|s| s.name == "Run"));
        assert!(hits.iter().any(|s| s.name == "runtime"));

        let fns = idx.find("run", Some("function"));
        assert_eq!(fns.len(), 2);
        assert!(idx.find("", None).is_empty());
    }

    #[test]
    fn build_skips_binary_and_huge_files() {
        let ws = TempWs::new("skip");
        std::fs::write(ws.0.join("bin.rs"), [0u8, 1]).unwrap();
        std::fs::write(
            ws.0.join("huge.rs"),
            "x".repeat(MAX_INDEX_FILE_BYTES as usize + 1),
        )
        .unwrap();
        let idx = SymbolIndex::build(&ws.0);
        assert!(idx.symbols.is_empty(), "{:?}", idx.symbols);
    }

    /// Clears the process-global snapshot. Test-only: lets tests install
    /// and remove their own indexes without leaking state.
    #[cfg(test)]
    pub(crate) fn reset_global() {
        if let Ok(mut slot) = INDEX.write() {
            *slot = None;
        }
        if let Ok(mut meta) = INDEX_META.write() {
            *meta = None;
        }
    }

    #[test]
    fn global_store_roundtrips_and_ensure_lazily_builds() {
        let _guard = global_guard();
        let ws = TempWs::new("global");
        std::fs::write(ws.0.join("lib.rs"), "pub fn govinda_index_fn() {}\n").unwrap();
        assert!(current().is_none());
        let n = rebuild(&ws.0);
        assert_eq!(n, 1);
        let idx = current().unwrap();
        assert_eq!(idx.find("govinda_index_fn", None).len(), 1);
        // ensure() returns a snapshot that sees the same symbols.
        let again = ensure(&ws.0);
        assert_eq!(again.find("govinda_index_fn", None).len(), 1);
        reset_global();
        assert!(current().is_none());
    }

    #[test]
    fn results_json_shapes_hits_for_the_model() {
        let idx = SymbolIndex {
            symbols: vec![Symbol {
                kind: "trait",
                name: "Runner".into(),
                file: "src/a.rs".into(),
                line: 9,
            }],
        };
        let hits = idx.find("Runner", None);
        let json = results_json(&hits);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["matches"], 1);
        assert_eq!(v["symbols"][0]["file"], "src/a.rs");
        assert_eq!(v["symbols"][0]["line"], 9);
    }
}
