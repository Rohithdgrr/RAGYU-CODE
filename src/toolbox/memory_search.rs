//! `memory_search` — semantic search over `.govinda/memory.md` and past sessions.
//!
//! Returns relevant prior decisions, gotchas, and facts. Cheap and
//! high-leverage: surfaces context the model would otherwise miss in
//! long sessions or after a `/clear`.

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// Natural-language query.
    pub query: String,
    /// Max results (default 5).
    pub max_results: Option<usize>,
}

pub fn run(base: &Path, args: Args) -> anyhow::Result<String> {
    let max = args.max_results.unwrap_or(5);
    let query_lower = args.query.to_lowercase();
    let query_tokens: Vec<&str> = query_lower.split_whitespace().collect();
    let mem_path = base.join(".govinda").join("memory.md");
    let Ok(raw) = std::fs::read_to_string(&mem_path) else {
        return Ok("{\"results\":[],\"note\":\"no memory file yet — use the 'remember' tool to add notes\"}".to_owned());
    };
    // Split memory into sections (separated by ## headers)
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current_header = String::new();
    let mut current_body = String::new();
    for line in raw.lines() {
        if line.starts_with("## ") {
            if !current_header.is_empty() || !current_body.trim().is_empty() {
                sections.push((current_header.clone(), current_body.trim().to_owned()));
            }
            current_header = line[3..].to_owned();
            current_body.clear();
        } else {
            current_body.push_str(line);
            current_body.push('\n');
        }
    }
    if !current_header.is_empty() || !current_body.trim().is_empty() {
        sections.push((current_header, current_body.trim().to_owned()));
    }
    // Score by token overlap (Jaccard-like)
    let mut scored: Vec<(usize, &(String, String))> = sections
        .iter()
        .map(|s| (score(&query_tokens, s.1.to_lowercase().as_str()), s))
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    let results: Vec<serde_json::Value> = scored
        .into_iter()
        .take(max)
        .map(|(_, (h, b))| {
            serde_json::json!({
                "header": h,
                "body": truncate(b, 500),
            })
        })
        .collect();
    Ok(format!(
        "{{\"count\":{},\"results\":{}}}",
        results.len(),
        serde_json::to_string(&results).unwrap_or_default()
    ))
}

fn score(query_tokens: &[&str], text: &str) -> usize {
    query_tokens.iter().filter(|t| text.contains(**t)).count()
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
    fn score_counts_overlapping_tokens() {
        assert_eq!(score(&["cargo", "build"], "cargo build error"), 2);
        assert_eq!(score(&["cargo", "test"], "completely unrelated"), 0);
    }

    #[test]
    fn returns_empty_when_no_memory_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = run(
            dir.path(),
            Args {
                query: "anything".into(),
                max_results: Some(5),
            },
        )
        .unwrap();
        assert!(result.contains("\"results\":[]"));
    }
}
