//! `regex_search` — regex search with named group capture and JSON output.
//!
//! Like `grep` but returns structured `{file, line, match, group1, group2, ...}`
//! so the model doesn't have to parse raw grep output.

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Args {
    /// Regex pattern (Rust syntax). Supports named groups (?P<name>...).
    pub pattern: String,
    /// File or directory to search (default ".").
    pub path: Option<String>,
    /// Include only files matching these glob patterns.
    pub include_globs: Option<Vec<String>>,
    /// Case-sensitive (default true).
    pub case_sensitive: Option<bool>,
    /// Max matches to return (default 100).
    pub max_matches: Option<usize>,
}

pub fn run(base: &Path, args: Args) -> anyhow::Result<String> {
    let target = base.join(args.path.as_deref().unwrap_or("."));
    let max = args.max_matches.unwrap_or(100);
    let case_sensitive = args.case_sensitive.unwrap_or(true);
    let re = regex::RegexBuilder::new(&args.pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|e| anyhow::anyhow!("invalid regex: {e}"))?;
    let names: Vec<String> = re.capture_names().filter_map(|n| n.map(String::from)).collect();
    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut count = 0usize;
        for entry in walk_files(&target)? {
            let Some(name_os) = entry.file_name() else { continue; };
            if let Some(globs) = &args.include_globs {
                let name = name_os.to_string_lossy();
                if !globs.iter().any(|g| glob_match(g, &name)) { continue; }
            }
        let Ok(raw) = std::fs::read_to_string(&entry) else { continue; };
        if raw.contains('\0') { continue; }
        for (lineno, line) in raw.lines().enumerate() {
            if let Some(caps) = re.captures(line) {
                let mut obj = serde_json::json!({
                    "file": entry.strip_prefix(base).unwrap_or(&entry).display().to_string(),
                    "line": lineno + 1,
                    "match": caps.get(0).map(|m| m.as_str()).unwrap_or(""),
                });
                for name in &names {
                    if let Some(m) = caps.name(name) {
                        obj.as_object_mut().unwrap().insert(name.clone(), serde_json::Value::String(m.as_str().to_owned()));
                    }
                }
                results.push(obj);
                count += 1;
                if count >= max { break; }
            }
        }
        if count >= max { break; }
    }
    Ok(format!("{{\"count\":{},\"matches\":{}}}", results.len(), serde_json::to_string(&results).unwrap_or_default()))
}

fn walk_files(target: &Path) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    if target.is_file() {
        out.push(target.to_path_buf());
        return Ok(out);
    }
    let skip = [".git", "target", "node_modules", "dist", "build", ".next", "__pycache__"];
    for entry in std::fs::read_dir(target)? {
        let e = entry?;
        let name = e.file_name().to_string_lossy().to_string();
        if skip.contains(&name.as_str()) { continue; }
        if e.file_type()?.is_dir() {
            out.extend(walk_files(&e.path())?);
        } else if e.file_type()?.is_file() {
            out.push(e.path());
        }
    }
    Ok(out)
}

fn glob_match(pattern: &str, name: &str) -> bool {
    // Simple glob: * matches any, ? matches one
    if let Ok(re) = glob_to_regex(pattern) {
        re.is_match(name)
    } else {
        pattern == name
    }
}

fn glob_to_regex(pattern: &str) -> Result<regex::Regex, regex::Error> {
    let mut r = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => r.push_str(".*"),
            '?' => r.push('.'),
            '.' | '(' | ')' | '+' | '|' | '^' | '$' | '{' | '}' | '[' | ']' | '\\' => { r.push('\\'); r.push(ch); }
            _ => r.push(ch),
        }
    }
    r.push('$');
    regex::Regex::new(&r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_to_regex_star_matches_anything() {
        let re = glob_to_regex("*.rs").unwrap();
        assert!(re.is_match("main.rs"));
        assert!(!re.is_match("main.go"));
    }

    #[test]
    fn glob_to_regex_handles_special_chars() {
        let re = glob_to_regex("a.b.c").unwrap();
        assert!(re.is_match("a.b.c"));
        assert!(!re.is_match("axbxc"));
    }
}
