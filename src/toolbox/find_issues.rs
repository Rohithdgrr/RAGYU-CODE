//! `find_issues` — find code-quality issues in a workspace: duplicate blocks
//! (sliding window), possibly-dead `pub fn` items, unused imports (Rust heuristic),
//! and unimplemented functions (todo!()/unimplemented!() bodies).

use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Issue {
    Duplicates,
    Deadcode,
    UnusedImports,
    UnusedCode,
    All,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    pub issue: Issue,
    /// Scope directory (default ".").
    pub path: Option<String>,
    /// Min lines for duplicate detection (default 5).
    pub min_lines: Option<usize>,
}

pub fn run(base: &Path, args: Args) -> anyhow::Result<String> {
    let scope = base.join(args.path.as_deref().unwrap_or("."));
    anyhow::ensure!(
        scope.is_dir(),
        "scope is not a directory: {}",
        scope.display()
    );
    let result = match args.issue {
        Issue::Duplicates => find_duplicates(&scope, args.min_lines.unwrap_or(5)),
        Issue::Deadcode => find_deadcode(&scope),
        Issue::UnusedImports => find_unused_imports(&scope),
        Issue::UnusedCode => find_unused_code(&scope),
        Issue::All => {
            let mut all: Vec<serde_json::Value> = Vec::new();
            all.extend(find_duplicates(&scope, args.min_lines.unwrap_or(5)));
            all.extend(find_deadcode(&scope));
            all.extend(find_unused_imports(&scope));
            all.extend(find_unused_code(&scope));
            all
        }
    };
    Ok(format!(
        "{{\"issue\":\"{:?}\",\"scope\":\"{}\",\"count\":{},\"findings\":{}}}",
        args.issue,
        scope.display(),
        result.len(),
        serde_json::to_string(&result).unwrap_or_default()
    ))
}

fn list_source_files(scope: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let skip = [
        ".git",
        "target",
        "node_modules",
        "dist",
        "build",
        ".next",
        "__pycache__",
    ];
    fn walk(dir: &Path, skip: &[&str], out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            if skip.contains(&e.file_name().to_string_lossy().as_ref()) {
                continue;
            }
            let path = e.path();
            if path.is_dir() {
                walk(&path, skip, out);
            } else if let Some(ext) = path.extension().and_then(|x| x.to_str()) {
                if matches!(ext, "rs" | "py" | "js" | "ts" | "go") {
                    out.push(path);
                }
            }
        }
    }
    walk(scope, &skip, &mut out);
    out
}

fn find_duplicates(scope: &Path, min_lines: usize) -> Vec<serde_json::Value> {
    let mut chunks: HashMap<String, Vec<(String, usize)>> = HashMap::new();
    for f in list_source_files(scope) {
        let Ok(content) = std::fs::read_to_string(&f) else {
            continue;
        };
        for (i, win) in content
            .lines()
            .collect::<Vec<_>>()
            .windows(min_lines)
            .enumerate()
        {
            let key: String = win.iter().map(|l| l.trim()).collect::<Vec<_>>().join("|");
            if key.is_empty() {
                continue;
            }
            chunks
                .entry(key)
                .or_default()
                .push((f.display().to_string(), i + 1));
        }
    }
    let mut out: Vec<serde_json::Value> = Vec::new();
    for (block, locs) in chunks {
        if locs.len() < 2 {
            continue;
        }
        let unique_files: HashSet<_> = locs.iter().map(|(f, _)| f.clone()).collect();
        if unique_files.len() < 2 {
            continue;
        }
        out.push(serde_json::json!({
            "issue": "duplicate",
            "lines": min_lines,
            "preview": block.lines().next().unwrap_or("").trim(),
            "occurrences": locs,
        }));
    }
    out
}

fn find_deadcode(scope: &Path) -> Vec<serde_json::Value> {
    let files = list_source_files(scope);
    let mut out = Vec::new();
    for f in &files {
        if !f.extension().map_or(false, |e| e == "rs") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(f) else {
            continue;
        };
        for (lineno, line) in content.lines().enumerate() {
            if let Some(idx) = line.find("pub fn ") {
                let rest = &line[idx + 7..];
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if name.is_empty() {
                    continue;
                }
                if files.len() > 1 {
                    let referenced_elsewhere = files.iter().any(|other| {
                        other != f
                            && std::fs::read_to_string(other)
                                .map(|c| c.contains(&name))
                                .unwrap_or(false)
                    });
                    if !referenced_elsewhere {
                        out.push(serde_json::json!({
                            "issue": "possibly_dead",
                            "name": name,
                            "file": f.display().to_string(),
                            "line": lineno + 1,
                        }));
                    }
                }
            }
        }
    }
    out
}

fn find_unused_imports(scope: &Path) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for f in list_source_files(scope) {
        if !f.extension().map_or(false, |e| e == "rs") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&f) else {
            continue;
        };
        for (lineno, line) in content.lines().enumerate() {
            if let Some(rest) = line.trim_start().strip_prefix("use ") {
                let last = rest
                    .rsplit("::")
                    .next()
                    .unwrap_or("")
                    .trim_end_matches(';')
                    .trim();
                let ident: String = last
                    .rsplit(" as ")
                    .next()
                    .unwrap_or(last)
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if ident.is_empty() || ident == "self" {
                    continue;
                }
                let usage = content.matches(&ident).count();
                if usage <= 1 {
                    out.push(serde_json::json!({
                        "issue": "unused_import",
                        "name": ident,
                        "file": f.display().to_string(),
                        "line": lineno + 1,
                    }));
                }
            }
        }
    }
    out
}

fn find_unused_code(scope: &Path) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for f in list_source_files(scope) {
        if !f.extension().map_or(false, |e| e == "rs") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&f) else {
            continue;
        };
        let mut in_fn = false;
        let mut depth = 0;
        let mut fn_name = String::new();
        let mut fn_start = 0;
        for (i, line) in content.lines().enumerate() {
            if !in_fn {
                if let Some(idx) = line.find("fn ") {
                    let after = &line[idx + 3..];
                    let mut chars = after.chars();
                    let first = chars.next();
                    if first.map_or(true, |c| !c.is_alphanumeric() && c != '_') {
                        continue;
                    }
                    let mut name = String::new();
                    name.push(first.unwrap());
                    for c in chars {
                        if c.is_alphanumeric() || c == '_' {
                            name.push(c);
                        } else {
                            break;
                        }
                    }
                    if name.is_empty() || name == "_" {
                        continue;
                    }
                    fn_name = name;
                    fn_start = i + 1;
                    depth = 0;
                    in_fn = true;
                }
            } else {
                depth += line.matches('{').count() as i32;
                depth -= line.matches('}').count() as i32;
                if depth <= 0 && line.contains('}') {
                    let body: String = content
                        .lines()
                        .take(i + 1)
                        .skip(fn_start)
                        .collect::<Vec<_>>()
                        .join("\n");
                    if body.contains("todo!()") || body.contains("unimplemented!()") {
                        out.push(serde_json::json!({
                            "issue": "unimplemented",
                            "name": fn_name,
                            "file": f.display().to_string(),
                            "line": fn_start,
                        }));
                    }
                    in_fn = false;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_duplicate_blocks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        let dup = "fn helper() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n    println!(\"{}{}{}\", x, y, z);\n}\n";
        std::fs::write(dir.path().join("src/a.rs"), dup).unwrap();
        std::fs::write(dir.path().join("src/b.rs"), dup).unwrap();
        let args = Args {
            issue: Issue::Duplicates,
            path: None,
            min_lines: Some(5),
        };
        let result = run(dir.path(), args).unwrap();
        assert!(result.contains("\"duplicate\""));
    }
}
