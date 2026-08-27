//! File Attachments + RAG Chunking / @file Injection v2
//!
//! Provides intelligent file chunking for RAG (Retrieval-Augmented Generation)
//! and the @-mention file injection system. Files are chunked into meaningful
//! segments that preserve context while staying within token limits.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Maximum size of a file to include in context (bytes).
const MAX_FILE_SIZE: usize = 512 * 1024; // 512KB
/// Target chunk size in tokens (was 2000 chars / 80 heuristic).
/// 700 tokens ≈ 2800 chars of code, safely fits the model's window when
/// several chunks are injected. The overlap is ~20% of this budget.
const DEFAULT_CHUNK_TOKENS: usize = 700;
const CHUNK_OVERLAP_TOKENS: usize = 140;

/// A chunk of a file with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChunk {
    /// Relative path to the file.
    pub path: String,
    /// Start line number (1-based).
    pub start_line: usize,
    /// End line number (1-based).
    pub end_line: usize,
    /// The chunk content.
    pub content: String,
    /// Total lines in the file.
    pub total_lines: usize,
    /// Language/file type.
    pub language: Option<String>,
}

/// Metadata about a file attachment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAttachment {
    /// Relative path to the file.
    pub path: String,
    /// File size in bytes.
    pub size: usize,
    /// Number of lines.
    pub lines: usize,
    /// Language/file type.
    pub language: Option<String>,
    /// Whether the file was fully included or chunked.
    pub fully_included: bool,
    /// Number of chunks created.
    pub chunk_count: usize,
}

/// Chunks a file into segments suitable for context injection.
pub fn chunk_file(path: &Path, base: &Path) -> Result<Vec<FileChunk>> {
    let meta = fs::metadata(path).context(format!("cannot stat {}", path.display()))?;
    anyhow::ensure!(meta.is_file(), "not a regular file");
    anyhow::ensure!(
        meta.len() <= MAX_FILE_SIZE as u64,
        "file too large ({} bytes, max {})",
        meta.len(),
        MAX_FILE_SIZE
    );

    let bytes = fs::read(path).context(format!("cannot read {}", path.display()))?;
    anyhow::ensure!(!bytes.contains(&0), "binary file");
    let text = String::from_utf8_lossy(&bytes).to_string();
    let total_lines = text.lines().count();
    let rel = path
        .strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let language = detect_language(&rel);

    let lines: Vec<&str> = text.lines().collect();
    let mut chunks = Vec::new();

    // Fast path: short files stay as one chunk when measured in real tokens.
    let total_tokens = crate::tokens::count(&text);
    if total_tokens <= DEFAULT_CHUNK_TOKENS || lines.len() <= 12 {
        chunks.push(FileChunk {
            path: rel,
            start_line: 1,
            end_line: total_lines.max(1),
            content: text,
            total_lines: total_lines.max(1),
            language,
        });
    } else {
        // Token-aware sliding window: grow each chunk until the token budget
        // is hit, then rewind by overlap tokens so context is preserved.
        let mut start = 0usize;
        while start < lines.len() {
            let mut tokens = 0usize;
            let mut end = start;
            while end < lines.len() && tokens < DEFAULT_CHUNK_TOKENS {
                // +1 for the newline that join("\n") will insert
                tokens += crate::tokens::count(lines[end]) + 1;
                end += 1;
                // If a single very long line already exceeds the budget, keep it
                // as its own chunk to guarantee progress.
                if end == start + 1 && tokens >= DEFAULT_CHUNK_TOKENS {
                    break;
                }
            }
            if end == start {
                end = (start + 1).min(lines.len());
            }
            let chunk_content: String = lines[start..end].join("\n");
            chunks.push(FileChunk {
                path: rel.clone(),
                start_line: start + 1,
                end_line: end,
                content: chunk_content,
                total_lines,
                language: language.clone(),
            });
            if end >= lines.len() {
                break;
            }
            // Rewind by overlap tokens.
            let mut overlap_tokens = 0usize;
            let mut overlap_start = end;
            while overlap_start > start && overlap_tokens < CHUNK_OVERLAP_TOKENS {
                overlap_start -= 1;
                overlap_tokens += crate::tokens::count(lines[overlap_start]) + 1;
            }
            // Guarantee forward progress: at least one new line per chunk.
            let next_start = overlap_start.max(start + 1).min(end - 1);
            // Safety: if rewound to the same start, force advance by one chunk's end.
            if next_start <= start {
                start = end;
            } else {
                start = next_start;
            }
            if start >= lines.len() {
                break;
            }
        }
    }

    Ok(chunks)
}

/// Chunks multiple files and returns them with metadata.
pub fn chunk_files(paths: &[&Path], base: &Path) -> Result<(Vec<FileChunk>, Vec<FileAttachment>)> {
    let mut all_chunks = Vec::new();
    let mut attachments = Vec::new();

    for path in paths {
        match chunk_file(path, base) {
            Ok(chunks) => {
                let rel = path
                    .strip_prefix(base)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let language = detect_language(&rel);
                let total_lines = chunks.last().map(|c| c.total_lines).unwrap_or(0);
                attachments.push(FileAttachment {
                    path: rel,
                    size: chunks.iter().map(|c| c.content.len()).sum(),
                    lines: total_lines,
                    language,
                    fully_included: chunks.len() == 1,
                    chunk_count: chunks.len(),
                });
                all_chunks.extend(chunks);
            }
            Err(e) => {
                eprintln!("warning: failed to chunk {}: {e}", path.display());
            }
        }
    }

    Ok((all_chunks, attachments))
}

/// Formats chunks for injection into the system prompt.
pub fn format_chunks_for_prompt(chunks: &[FileChunk]) -> String {
    if chunks.is_empty() {
        return String::new();
    }

    let mut output = String::from("# Attached Files\n\n");

    for chunk in chunks {
        output.push_str(&format!(
            "## {} (lines {}-{}/{})\n\n```\n{}\n```\n\n",
            chunk.path, chunk.start_line, chunk.end_line, chunk.total_lines, chunk.content
        ));
    }

    output
}

/// Detects the programming language from a file extension.
fn detect_language(filename: &str) -> Option<String> {
    let ext = filename.rsplit('.').next()?.to_lowercase();
    match ext.as_str() {
        "rs" => Some("rust".into()),
        "js" | "jsx" => Some("javascript".into()),
        "ts" | "tsx" => Some("typescript".into()),
        "py" => Some("python".into()),
        "go" => Some("go".into()),
        "java" => Some("java".into()),
        "c" | "h" => Some("c".into()),
        "cpp" | "cc" | "cxx" | "hpp" => Some("cpp".into()),
        "rb" => Some("ruby".into()),
        "php" => Some("php".into()),
        "swift" => Some("swift".into()),
        "kt" | "kts" => Some("kotlin".into()),
        "cs" => Some("csharp".into()),
        "html" | "htm" => Some("html".into()),
        "css" => Some("css".into()),
        "scss" | "sass" => Some("css".into()),
        "json" => Some("json".into()),
        "yaml" | "yml" => Some("yaml".into()),
        "toml" => Some("toml".into()),
        "md" | "markdown" => Some("markdown".into()),
        "sh" | "bash" | "zsh" => Some("shell".into()),
        "sql" => Some("sql".into()),
        "xml" => Some("xml".into()),
        _ => None,
    }
}

/// Searches for files matching a query pattern (for @-mention).
pub fn search_files(query: &str, base: &Path, max_results: usize) -> Vec<String> {
    let ignore = crate::ignore::IgnoreRules::load(base);
    let mut results = Vec::new();

    fn walk(
        dir: &Path,
        base: &Path,
        ignore: &crate::ignore::IgnoreRules,
        query: &str,
        results: &mut Vec<String>,
        max: usize,
        depth: usize,
    ) {
        if results.len() >= max || depth > 8 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if results.len() >= max {
                break;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = path.is_dir();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if ignore.matches(&rel, is_dir) {
                continue;
            }
            if !is_dir
                && (query.is_empty()
                    || name
                        .to_ascii_lowercase()
                        .contains(&query.to_ascii_lowercase()))
            {
                results.push(rel);
            }
            if is_dir {
                walk(&path, base, ignore, query, results, max, depth + 1);
            }
        }
    }

    walk(base, base, &ignore, query, &mut results, max_results, 0);
    results.sort();
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn chunk_small_file() {
        let dir = std::env::temp_dir().join("govinda-rag-test");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("small.rs");
        fs::write(&path, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();

        let chunks = chunk_file(&path, &dir).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[0].end_line, 3);
        assert!(chunks[0].content.contains("fn main"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_language_works() {
        assert_eq!(detect_language("main.rs"), Some("rust".into()));
        assert_eq!(detect_language("app.ts"), Some("typescript".into()));
        assert_eq!(detect_language("style.css"), Some("css".into()));
        assert!(detect_language("readme").is_none());
    }
}
