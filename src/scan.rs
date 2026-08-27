//! `scan_project`: builds a structured overview of the workspace so the
//! model can orient itself before reading files — project type, entry
//! points, dependencies, file statistics, and git state.
//!
//! Everything here is read-only. Dependency manifests are parsed with plain
//! data-model access (no full schema validation); git state comes from
//! reading `.git/HEAD` directly plus one internal, argv-direct
//! `git status --porcelain` spawn that fails gracefully when git is absent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Dependencies reported per ecosystem, at most.
const MAX_DEPS: usize = 60;
/// Dirty-file lines kept in the result.
const MAX_DIRTY_LINES: usize = 50;
/// Extension histogram entries kept.
const MAX_EXTENSIONS: usize = 15;
/// Wall-clock cap for the internal `git status` spawn.
const GIT_STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Builds the overview JSON for the workspace rooted at `base`.
pub async fn scan(base: &Path) -> String {
    let mut project_types: Vec<&str> = Vec::new();
    let mut entry_points: Vec<String> = Vec::new();
    let mut dependencies: BTreeMap<String, Value> = BTreeMap::new();

    // -- Rust ---------------------------------------------------------------
    if base.join("Cargo.toml").is_file() {
        project_types.push("rust");
        if let Ok(text) = tokio::fs::read_to_string(base.join("Cargo.toml")).await {
            let deps = toml_deps(&text);
            if !deps.is_empty() {
                dependencies.insert("cargo".into(), Value::Object(deps));
            }
        }
        for ep in ["src/main.rs", "src/lib.rs"] {
            if base.join(ep).is_file() {
                entry_points.push(ep.into());
            }
        }
    }

    // -- Node -----------------------------------------------------------------
    if base.join("package.json").is_file() {
        project_types.push("node");
        // Malformed package.json: skip silently, the type is still detected.
        if let Ok(pkg) = tokio::fs::read_to_string(base.join("package.json"))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|raw| serde_json::from_str::<Value>(&raw).map_err(anyhow::Error::from))
        {
            let deps = object_deps(pkg.get("dependencies"));
            let dev = object_deps(pkg.get("devDependencies"));
            let mut merged = deps;
            merged.extend(dev);
            if !merged.is_empty() {
                dependencies.insert("npm".into(), Value::Object(merged));
            }
            if let Some(Value::String(m)) = pkg.get("main") {
                entry_points.push(m.clone());
            }
            if let Some(Value::String(b)) = pkg.get("bin") {
                entry_points.push(b.clone());
            }
        }
    }

    // -- Python ---------------------------------------------------------------
    let requirements = base.join("requirements.txt").is_file();
    let pyproject = base.join("pyproject.toml").is_file();
    if pyproject || requirements {
        project_types.push("python");
        if requirements {
            let deps: Vec<Value> = std::fs::read_to_string(base.join("requirements.txt"))
                .map(|text| {
                    text.lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty() && !l.starts_with('#'))
                        .filter_map(|line| {
                            line.split(['=', '<', '>', '!', ';', '['])
                                .next()
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .map(|name| Value::String(name.to_owned()))
                        })
                        .take(MAX_DEPS)
                        .collect()
                })
                .unwrap_or_default();
            if !deps.is_empty() {
                dependencies.insert("pip".into(), Value::Array(deps));
            }
        }
        for ep in ["main.py", "app.py", "src/main.py"] {
            if base.join(ep).is_file() {
                entry_points.push(ep.into());
            }
        }
    }

    // -- Go -------------------------------------------------------------------
    if base.join("go.mod").is_file() {
        project_types.push("go");
        for ep in ["main.go"] {
            if base.join(ep).is_file() {
                entry_points.push(ep.into());
            }
        }
    }

    // -- File statistics (respects .govindaignore via tools::walk_files) ------
    let files = crate::tools::walk_files(base, base);
    let total_files = files.len();
    let mut ext_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut extensionless = 0usize;
    for f in &files {
        match f.extension().and_then(|e| e.to_str()) {
            Some(ext) => *ext_counts.entry(ext.to_ascii_lowercase()).or_default() += 1,
            None => extensionless += 1,
        }
    }
    let mut by_count: Vec<(String, usize)> = ext_counts.into_iter().collect();
    by_count.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    by_count.truncate(MAX_EXTENSIONS);
    let extensions: BTreeMap<String, Value> = by_count
        .into_iter()
        .map(|(ext, n)| (format!(".{ext}"), Value::from(n)))
        .collect();

    // -- Git ------------------------------------------------------------------
    let branch = read_git_branch(base);
    let dirty_files = git_status_dirty(base).await;
    let git = if branch.is_some() || dirty_files.is_some() {
        Some(serde_json::json!({
            "branch": branch,
            "dirty_files": dirty_files,
        }))
    } else {
        None
    };

    let result = serde_json::json!({
        "project_types": project_types,
        "entry_points": entry_points,
        "dependencies": dependencies,
        "files": {
            "total": total_files,
            "extensions": extensions,
            "extensionless": extensionless,
        },
        "git": git,
    });
    result.to_string()
}

/// Parses `[dependencies]` / `[dev-dependencies]` from a Cargo.toml string.
fn toml_deps(cargo_toml: &str) -> serde_json::Map<String, Value> {
    use toml::Value as Tv;
    let mut out = serde_json::Map::new();
    let parsed: Option<Tv> = cargo_toml.parse().ok();
    if let Some(Tv::Table(tables)) = parsed {
        for section in ["dependencies", "dev-dependencies"] {
            if let Some(Tv::Table(deps)) = tables.get(section) {
                for (name, spec) in deps.iter().take(MAX_DEPS - out.len()) {
                    let version = match spec {
                        Tv::String(v) => v.clone(),
                        Tv::Table(t) => t
                            .get("version")
                            .and_then(Tv::as_str)
                            .unwrap_or("path/git")
                            .to_owned(),
                        _ => continue,
                    };
                    out.insert(name.clone(), Value::String(version));
                }
            }
        }
    }
    out
}

/// Flattens a `dependencies`-style JSON object into `"name": "version"` pairs.
fn object_deps(value: Option<&Value>) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    if let Some(Value::Object(map)) = value {
        for (name, version) in map.iter().take(MAX_DEPS) {
            out.insert(
                name.clone(),
                Value::String(version.as_str().unwrap_or("*").to_owned()),
            );
        }
    }
    out
}

fn git_dir(base: &Path) -> Option<PathBuf> {
    let git_path = base.join(".git");
    if git_path.is_dir() {
        return Some(git_path);
    }
    if git_path.is_file() {
        let content = std::fs::read_to_string(&git_path).ok()?;
        let rest = content.strip_prefix("gitdir:")?.trim();
        let p = Path::new(rest);
        return Some(if p.is_absolute() {
            p.to_path_buf()
        } else {
            base.join(p)
        });
    }
    None
}

/// Reads the current branch from `.git/HEAD`; handles detached HEAD and worktrees.
fn read_git_branch(base: &Path) -> Option<String> {
    let git_dir = git_dir(base)?;
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(rest) = head.strip_prefix("ref: refs/heads/") {
        return (!rest.is_empty()).then(|| rest.to_owned());
    }
    // Detached: HEAD holds a raw commit hash.
    let short: String = head.chars().take(7).collect();
    (!short.is_empty()).then(|| format!("(detached) {short}"))
}

/// One internal, read-only `git status --porcelain` spawn; any failure
/// (missing git, not a repo, timeout) yields `None` rather than an error.
async fn git_status_dirty(base: &Path) -> Option<Vec<String>> {
    let output = tokio::time::timeout(
        GIT_STATUS_TIMEOUT,
        tokio::process::Command::new("git")
            .arg("-C")
            .arg(base)
            .args(["status", "--porcelain"])
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None; // not a repo, or git refused
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Porcelain lines are `XY <path>`: drop the 3-char status column.
    Some(
        stdout
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.get(3..).unwrap_or(l).to_owned())
            .take(MAX_DIRTY_LINES)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TempWs(PathBuf);
    impl TempWs {
        fn new(tag: &str) -> Self {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "govinda-scan-{tag}-{}-{}",
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

    #[tokio::test]
    async fn scan_detects_rust_project_with_deps_and_entries() {
        let ws = TempWs::new("rust");
        std::fs::write(
            ws.0.join("Cargo.toml"),
            "[package]\nname = \"x\"\n\n[dependencies]\nserde = \"1\"\nanyhow = \"1\"\n\n[dev-dependencies]\nwiremock = \"0.6\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(ws.0.join("src")).unwrap();
        std::fs::write(ws.0.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(ws.0.join("README.md"), "# x\n").unwrap();

        let out = scan(&ws.0).await;
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["project_types"][0], "rust");
        assert!(
            parsed["entry_points"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e == "src/main.rs")
        );
        assert_eq!(parsed["dependencies"]["cargo"]["serde"], "1");
        assert_eq!(parsed["dependencies"]["cargo"]["wiremock"], "0.6");
        assert_eq!(parsed["files"]["total"], 3);
        assert_eq!(parsed["files"]["extensions"][".rs"], 1);
        assert_eq!(parsed["files"]["extensions"][".md"], 1);
    }

    #[tokio::test]
    async fn scan_respects_govindaignore_in_stats() {
        let ws = TempWs::new("ignore");
        std::fs::write(ws.0.join(".govindaignore"), "*.log\n/generated/\n").unwrap();
        std::fs::write(ws.0.join("keep.rs"), "fn a() {}\n").unwrap();
        std::fs::write(ws.0.join("noise.log"), "x\n").unwrap();
        std::fs::create_dir_all(ws.0.join("generated")).unwrap();
        std::fs::write(ws.0.join("generated/out.rs"), "fn b() {}\n").unwrap();

        let out = scan(&ws.0).await;
        let parsed: Value = serde_json::from_str(&out).unwrap();
        // .govindaignore itself + keep.rs; noise.log and generated/ excluded
        assert_eq!(parsed["files"]["total"], 2);
        assert_eq!(parsed["files"]["extensions"][".rs"], 1);
        assert_eq!(parsed["files"]["extensionless"], 1);
    }

    #[tokio::test]
    async fn scan_handles_empty_and_multi_language_workspaces() {
        let ws = TempWs::new("multi");
        std::fs::create_dir_all(ws.0.join("src")).unwrap();
        std::fs::write(
            ws.0.join("package.json"),
            "{\"main\":\"dist/index.js\",\"dependencies\":{\"left-pad\":\"^1\"}}",
        )
        .unwrap();
        std::fs::write(
            ws.0.join("requirements.txt"),
            "flask==2.0.0\n# comment\nclick\n",
        )
        .unwrap();
        std::fs::write(
            ws.0.join("Cargo.toml"),
            "[package]\nname = \"x\"\n\n[dependencies]\nserde = \"1\"\n",
        )
        .unwrap();
        std::fs::write(ws.0.join("src/main.rs"), "fn main() {}\n").unwrap();

        let out = scan(&ws.0).await;
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let types: Vec<&str> = parsed["project_types"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(types.contains(&"node"));
        assert!(types.contains(&"python"));
        assert!(types.contains(&"rust"));
        assert_eq!(parsed["dependencies"]["npm"]["left-pad"], "^1");
        let pip = parsed["dependencies"]["pip"].as_array().unwrap();
        assert!(pip.iter().any(|d| d == "flask"));
        assert!(pip.iter().any(|d| d == "click"));
        assert!(
            parsed["entry_points"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e == "dist/index.js")
        );
    }

    #[tokio::test]
    async fn scan_malformed_package_json_still_reports_type() {
        let ws = TempWs::new("brokenpkg");
        std::fs::write(ws.0.join("package.json"), "{not json").unwrap();
        let out = scan(&ws.0).await;
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["project_types"][0], "node");
        assert!(parsed["dependencies"]["npm"].is_null());
    }

    #[test]
    fn branch_reading_supports_attached_detached_and_missing() {
        let ws = TempWs::new("branch");
        assert_eq!(read_git_branch(&ws.0), None);

        let git_dir = ws.0.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(read_git_branch(&ws.0).as_deref(), Some("main"));

        std::fs::write(git_dir.join("HEAD"), "abc1234def5678\n").unwrap();
        assert_eq!(
            read_git_branch(&ws.0).as_deref(),
            Some("(detached) abc1234")
        );
    }

    #[tokio::test]
    async fn git_status_returns_none_without_a_repo() {
        let ws = TempWs::new("norepo");
        assert!(git_status_dirty(&ws.0).await.is_none());
    }
}
