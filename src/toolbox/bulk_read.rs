//! `bulk_read` — read multiple files in one call.
//!
//! Cheaper than calling `read_file` N times when gathering context across
//! files. Returns structured JSON with per-file results.

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// List of workspace-relative paths.
    pub paths: Vec<String>,
    /// Include line numbers (default true).
    pub line_numbers: Option<bool>,
    /// Max bytes per file (default 50_000, 0 = unlimited).
    pub max_bytes: Option<usize>,
}

pub fn run(base: &Path, args: Args) -> anyhow::Result<String> {
    let line_numbers = args.line_numbers.unwrap_or(true);
    let max_bytes = args.max_bytes.unwrap_or(50_000);
    let mut results: Vec<serde_json::Value> = Vec::new();
    for path in &args.paths {
        let full = base.join(path);
        let entry = match std::fs::read_to_string(&full) {
            Ok(content) => {
                let original_len = content.len();
                let truncated = if max_bytes > 0 && content.len() > max_bytes {
                    format!(
                        "{}…(truncated to {} bytes)",
                        &content[..max_bytes],
                        max_bytes
                    )
                } else {
                    content
                };
                let formatted = if line_numbers {
                    truncated
                        .lines()
                        .enumerate()
                        .map(|(i, l)| format!("{:>4}  {}", i + 1, l))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    truncated
                };
                serde_json::json!({
                    "path": path,
                    "ok": true,
                    "bytes": original_len,
                    "content": formatted,
                })
            }
            Err(e) => serde_json::json!({
                "path": path,
                "ok": false,
                "error": e.to_string(),
            }),
        };
        results.push(entry);
    }
    Ok(format!(
        "{{\"count\":{},\"results\":{}}}",
        results.len(),
        serde_json::to_string(&results).unwrap_or_default()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
        std::fs::write(dir.path().join("b.txt"), "beta").unwrap();
        let args = Args {
            paths: vec!["a.txt".into(), "b.txt".into()],
            line_numbers: Some(false),
            max_bytes: None,
        };
        let result = run(dir.path(), args).unwrap();
        assert!(result.contains("\"alpha\""));
        assert!(result.contains("\"beta\""));
    }

    #[test]
    fn reports_missing_files_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let args = Args {
            paths: vec!["missing.txt".into()],
            line_numbers: Some(false),
            max_bytes: None,
        };
        let result = run(dir.path(), args).unwrap();
        assert!(result.contains("\"ok\":false"));
    }

    #[test]
    fn line_numbers_are_added() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x\ny\nz\n").unwrap();
        let args = Args {
            paths: vec!["a.txt".into()],
            line_numbers: Some(true),
            max_bytes: None,
        };
        let result = run(dir.path(), args).unwrap();
        assert!(result.contains("1  x"));
        assert!(result.contains("2  y"));
        assert!(result.contains("3  z"));
    }
}
