//! `code_format` — normalize source-code formatting per language: enforce LF
//! or CRLF line endings, strip trailing whitespace, ensure final newline,
//! and convert between spaces and tabs (2/4-space indent).

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    /// LF endings, strip trailing whitespace, ensure final newline.
    Unix,
    /// CRLF endings, strip trailing whitespace, ensure final newline.
    Windows,
    /// 2-space indent (after normalization).
    TwoSpace,
    /// 4-space indent (after normalization).
    FourSpace,
    /// Tab indent (after normalization).
    Tabs,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct FileSpec {
    pub path: String,
    pub style: Style,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Args {
    pub files: Vec<FileSpec>,
}

pub fn run(base: &Path, args: Args) -> anyhow::Result<String> {
    let mut results: Vec<serde_json::Value> = Vec::new();
    for spec in &args.files {
        let full = base.join(&spec.path);
        match std::fs::read_to_string(&full) {
            Err(e) => results.push(serde_json::json!({"path":spec.path, "ok":false, "error":e.to_string()})),
            Ok(content) => {
                let before = content.len();
                let updated = apply_style(&content, &spec.style);
                match std::fs::write(&full, &updated) {
                    Ok(()) => results.push(serde_json::json!({
                        "path": spec.path, "ok": true, "style": format!("{:?}", spec.style).to_lowercase(),
                        "before": before, "after": updated.len(),
                    })),
                    Err(e) => results.push(serde_json::json!({"path":spec.path, "ok":false, "error":e.to_string()})),
                }
            }
        }
    }
    let n = results.len();
    let n_ok = results.iter().filter(|r| r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false)).count();
    Ok(format!("{{\"total\":{n},\"ok\":{n_ok},\"err\":{},\"results\":{}}}",
        n - n_ok,
        serde_json::to_string(&results).unwrap_or_default()
    ))
}

fn apply_style(content: &str, style: &Style) -> String {
    let mut s = content.replace("\r\n", "\n");
    let normalized: String = s.lines().map(|l| l.trim_end().to_string()).collect::<Vec<_>>().join("\n");
    s = normalized;
    if !s.ends_with('\n') { s.push('\n'); }
    match style {
        Style::Unix => s,
        Style::Windows => s.replace("\n", "\r\n"),
        Style::TwoSpace => reindent(&s, 2, " "),
        Style::FourSpace => reindent(&s, 4, " "),
        Style::Tabs => reindent(&s, 1, "\t"),
    }
}

fn reindent(content: &str, target: usize, ch: &str) -> String {
    let mut out_lines: Vec<String> = Vec::new();
    for line in content.lines() {
        let stripped = line.trim_start();
        let leading_ws_len = line.len() - stripped.len();
        let leading_ws = line[..leading_ws_len].chars().filter(|c| c.is_whitespace()).count();
        let new_indent = if leading_ws > 0 { leading_ws * target } else { 0 };
        out_lines.push(format!("{}{}", ch.repeat(new_indent), stripped));
    }
    format!("{}\n", out_lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_lf_and_strips_trailing_ws() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a  \nb\nc  \n").unwrap();
        let args = Args { files: vec![FileSpec { path: "a.txt".into(), style: Style::Unix }] };
        run(dir.path(), args).unwrap();
        let content = std::fs::read_to_string(dir.path().join("a.txt")).unwrap();
        assert!(!content.contains("  \n"));
        assert!(content.ends_with('\n'));
    }
}
