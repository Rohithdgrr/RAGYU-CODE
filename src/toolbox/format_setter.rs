//! `format_setter` — bulk-convert file formats: normalize line endings to
//! CRLF/LF/CR, or uppercase filenames. Useful when cross-platform contributors
//! produce inconsistent line endings.

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConvertTo {
    Crlf,
    Lf,
    Cr,
    /// Uppercase the filename (no body change).
    Upper,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct FileSpec {
    pub path: String,
    pub to: ConvertTo,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    pub files: Vec<FileSpec>,
}

pub fn run(base: &Path, args: Args) -> anyhow::Result<String> {
    let mut results: Vec<serde_json::Value> = Vec::new();
    for spec in &args.files {
        let full = base.join(&spec.path);
        match std::fs::read_to_string(&full) {
            Err(e) => {
                results.push(serde_json::json!({
                    "path": spec.path, "ok": false, "error": e.to_string(),
                }));
                continue;
            }
            Ok(content) => {
                let bytes = content.len();
                let new_content = match spec.to {
                    ConvertTo::Crlf => content
                        .replace("\r\n", "\n")
                        .replace("\r", "\n")
                        .replace("\n", "\r\n"),
                    ConvertTo::Lf => content.replace("\r\n", "\n").replace("\r", "\n"),
                    ConvertTo::Cr => content.replace("\r\n", "\r").replace("\n", "\r"),
                    ConvertTo::Upper => {
                        if let Some(parent) = full.parent() {
                            let new_name = spec.path.to_uppercase();
                            let new_full = parent.join(&new_name);
                            if new_full != full {
                                let _ = std::fs::rename(&full, &new_full);
                            }
                            results.push(serde_json::json!({
                                "path": spec.path, "ok": true, "renamed_to": new_name, "bytes": bytes,
                            }));
                            continue;
                        }
                        content.clone()
                    }
                };
                match std::fs::write(&full, &new_content) {
                    Ok(()) => results.push(serde_json::json!({
                        "path": spec.path, "ok": true, "to": format!("{:?}", spec.to).to_lowercase(), "bytes": new_content.len(),
                    })),
                    Err(e) => results.push(serde_json::json!({
                        "path": spec.path, "ok": false, "error": e.to_string(),
                    })),
                }
            }
        }
    }
    let n = results.len();
    let n_ok = results
        .iter()
        .filter(|r| r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false))
        .count();
    Ok(format!(
        "{{\"total\":{n},\"ok\":{n_ok},\"err\":{},\"results\":{}}}",
        n - n_ok,
        serde_json::to_string(&results).unwrap_or_default()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_lf_to_crlf() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a\nb\nc\n").unwrap();
        let args = Args {
            files: vec![FileSpec {
                path: "a.txt".into(),
                to: ConvertTo::Crlf,
            }],
        };
        run(dir.path(), args).unwrap();
        let content = std::fs::read_to_string(dir.path().join("a.txt")).unwrap();
        assert!(content.contains("\r\n"));
    }
}
