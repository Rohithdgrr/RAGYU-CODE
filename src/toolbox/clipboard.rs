//! `clipboard` — internal copy/paste/cut for project data.
//!
//! Lets the model hold intermediate text state without re-reading files.
//! The clipboard is an in-memory buffer scoped to the process; it is NOT
//! the OS clipboard. Reduces token usage for multi-step edits.

use std::sync::{Mutex, OnceLock};

static CLIPBOARD: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn clipboard() -> &'static Mutex<Option<String>> {
    CLIPBOARD.get_or_init(|| Mutex::new(None))
}

/// What to do with the clipboard.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Read a file (or a line range) into the clipboard.
    Copy,
    /// Write the clipboard to a file.
    Paste,
    /// Read a file (or a line range) into the clipboard AND blank it in place.
    Cut,
    /// Show the current clipboard contents (or "(empty)").
    View,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    pub action: Action,
    /// File to read from (copy/cut) or write to (paste). Required for copy/cut/paste.
    pub source_path: Option<String>,
    /// 1-based start line. Defaults to 1.
    pub line_start: Option<usize>,
    /// 1-based end line (inclusive). Defaults to last line.
    pub line_end: Option<usize>,
    /// Inline text to paste. When provided, source_path is ignored.
    pub text: Option<String>,
}

pub fn run(base: &std::path::Path, args: Args) -> anyhow::Result<String> {
    match args.action {
        Action::Copy => {
            let path = args
                .source_path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("source_path required for copy"))?;
            let content = read_range(base, path, args.line_start, args.line_end)?;
            let n = content.chars().count();
            *clipboard().lock().unwrap_or_else(|e| e.into_inner()) = Some(content);
            Ok(format!(
                "{{\"action\":\"copy\",\"source\":\"{}\",\"chars\":{}}}",
                path, n
            ))
        }
        Action::Paste => {
            let text = if let Some(t) = args.text {
                t
            } else {
                clipboard()
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("clipboard is empty"))?
            };
            let path = args
                .source_path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("source_path required for paste"))?;
            crate::tools::write_file(
                base,
                &crate::tools::WriteFileArgs {
                    path: path.to_owned(),
                    content: text.clone(),
                },
            )?;
            Ok(format!(
                "{{\"action\":\"paste\",\"target\":\"{}\",\"chars\":{}}}",
                path,
                text.chars().count()
            ))
        }
        Action::Cut => {
            let path = args
                .source_path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("source_path required for cut"))?;
            let content = read_range(base, path, args.line_start, args.line_end)?;
            let n = content.chars().count();
            *clipboard().lock().unwrap_or_else(|e| e.into_inner()) = Some(content.clone());
            // Blank the range in the source file.
            let full = std::fs::read_to_string(base.join(path))?;
            let lines: Vec<&str> = full.lines().collect();
            let start = args.line_start.unwrap_or(1).saturating_sub(1);
            let end = args.line_end.unwrap_or(lines.len()).min(lines.len());
            let mut new_lines: Vec<String> = lines
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    if i >= start && i < end {
                        String::new()
                    } else {
                        (*l).to_owned()
                    }
                })
                .collect();
            // Remove trailing blanks introduced by the cut.
            while new_lines.last().map(|l| l.is_empty()).unwrap_or(false) {
                new_lines.pop();
            }
            std::fs::write(base.join(path), format!("{}\n", new_lines.join("\n")))?;
            Ok(format!(
                "{{\"action\":\"cut\",\"source\":\"{}\",\"chars\":{},\"lines\":{}}}",
                path,
                n,
                end - start
            ))
        }
        Action::View => {
            let text = clipboard().lock().unwrap_or_else(|e| e.into_inner());
            match text.as_deref() {
                Some(t) => Ok(format!(
                    "{{\"chars\":{},\"preview\":{}}}",
                    t.chars().count(),
                    serde_json::Value::String(truncate(t, 200))
                )),
                None => Ok("{\"empty\":true}".to_owned()),
            }
        }
    }
}

fn read_range(
    base: &std::path::Path,
    path: &str,
    start: Option<usize>,
    end: Option<usize>,
) -> anyhow::Result<String> {
    let full = std::fs::read_to_string(base.join(path))
        .map_err(|e| anyhow::anyhow!("cannot read '{path}': {e}"))?;
    let lines: Vec<&str> = full.lines().collect();
    let s = start.unwrap_or(1).saturating_sub(1);
    let e = end.unwrap_or(lines.len()).min(lines.len());
    anyhow::ensure!(s < lines.len(), "line_start {s} out of range");
    Ok(lines[s..e].join("\n"))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_paste_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "line1\nline2\nline3\n").unwrap();
        let copy_args = Args {
            action: Action::Copy,
            source_path: Some("a.txt".into()),
            line_start: Some(2),
            line_end: Some(2),
            text: None,
        };
        run(dir.path(), copy_args).unwrap();
        let paste_args = Args {
            action: Action::Paste,
            source_path: Some("b.txt".into()),
            line_start: None,
            line_end: None,
            text: None,
        };
        let result = run(dir.path(), paste_args).unwrap();
        assert!(result.contains("\"chars\":5"));
        let pasted = std::fs::read_to_string(dir.path().join("b.txt")).unwrap();
        assert_eq!(pasted.trim(), "line2");
    }

    #[test]
    fn view_reports_empty() {
        clipboard().lock().unwrap_or_else(|e| e.into_inner()).take();
        let args = Args {
            action: Action::View,
            source_path: None,
            line_start: None,
            line_end: None,
            text: None,
        };
        let result = run(&std::path::PathBuf::new(), args).unwrap();
        assert!(result.contains("empty"));
    }
}
