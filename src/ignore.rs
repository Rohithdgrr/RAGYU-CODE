//! `.govindaignore` support: a gitignore-flavored exclusion list that governs
//! which files the agent may see.
//!
//! The file lives at the workspace root and uses a pragmatic subset of
//! gitignore syntax: `#` comments, blank lines, trailing `/` for
//! directory-only rules, leading or interior `/` to anchor at the root, and
//! `*`, `?`, `**` globs elsewhere. Negation (`!`) is intentionally not
//! supported — rules are additive so behavior stays predictable; anything a
//! rule matches stays hidden.

use regex::Regex;
use std::path::Path;

/// Parsed ignore rules, matched against workspace-relative paths using
/// forward slashes (the same normalization `display_rel` produces).
#[derive(Debug, Default)]
pub struct IgnoreRules {
    rules: Vec<Rule>,
}

#[derive(Debug)]
struct Rule {
    re: Regex,
    /// Rule only applies to directories (pattern ended with `/`). Because an
    /// ignored directory is never descended into, matching the directory
    /// itself is enough — children are never visited.
    dir_only: bool,
}

impl IgnoreRules {
    /// Loads `<root>/.govindaignore`; a missing or unreadable file means "no
    /// exclusions".
    pub fn load(root: &Path) -> Self {
        match std::fs::read_to_string(root.join(".govindaignore")) {
            Ok(text) => Self::parse(&text),
            Err(_) => Self::default(),
        }
    }

    /// Parses ignore-file text. Malformed lines are skipped silently rather
    /// than erroring — a broken ignore file must never break the tools.
    pub fn parse(text: &str) -> Self {
        let mut rules = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            let dir_only = line.ends_with('/');
            let mut pattern = line.trim_end_matches('/');
            // Interior or leading slash anchors the pattern at the root.
            let anchored = pattern.contains('/');
            pattern = pattern.trim_start_matches('/');
            if pattern.is_empty() {
                continue;
            }
            let body = glob_body(pattern);
            let anchored_re = if anchored {
                format!("^{body}(/.*)?$")
            } else {
                format!("^(?:.*/)?{body}(/.*)?$")
            };
            if let Ok(re) = Regex::new(&anchored_re) {
                rules.push(Rule { re, dir_only });
            }
        }
        Self { rules }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Whether `rel_path` (workspace-relative, forward slashes) is excluded.
    /// Directory-only rules also exclude everything beneath the directory
    /// they match, mirroring gitignore. Ancestors are checked so callers can
    /// test a file directly without walking top-down.
    pub fn matches(&self, rel_path: &str, is_dir: bool) -> bool {
        let mut current = Some(rel_path);
        while let Some(path) = current {
            let treat_as_dir = path != rel_path || is_dir;
            if self
                .rules
                .iter()
                .any(|r| (!r.dir_only || treat_as_dir) && r.re.is_match(path))
            {
                return true;
            }
            current = path.rsplit_once('/').map(|(parent, _)| parent);
        }
        false
    }
}

/// Translates the glob portion of a pattern into a regex fragment:
/// `**` becomes any path segments, `*`/`?` stay within one segment,
/// everything else is escaped literally.
fn glob_body(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                if chars.peek() == Some(&'/') {
                    chars.next();
                }
                out.push_str(".*");
            }
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            other => out.push_str(&regex::escape(&other.to_string())),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matched(rules_text: &str, rel: &str, is_dir: bool) -> bool {
        IgnoreRules::parse(rules_text).matches(rel, is_dir)
    }

    #[test]
    fn comments_blanks_and_negation_are_skipped() {
        assert!(IgnoreRules::parse("# comment\n\n   \n!keep").is_empty());
    }

    #[test]
    fn unanchored_patterns_match_at_any_depth() {
        let text = "*.log\ntemp\n";
        assert!(matched(text, "debug.log", false));
        assert!(matched(text, "logs/sub/debug.log", false));
        assert!(matched(text, "temp", true));
        assert!(matched(text, "src/temp", true));
        assert!(
            !matched(text, "template", true),
            "basename must match whole segment"
        );
        assert!(!matched(text, "src/main.rs", false));
    }

    #[test]
    fn anchored_patterns_only_match_from_root() {
        let text = "/dist\nsrc/*.tmp\n";
        assert!(matched(text, "dist", true));
        assert!(matched(text, "dist/x.js", false));
        assert!(!matched(text, "pkg/dist/x.js", false));
        assert!(matched(text, "src/a.tmp", false));
        assert!(!matched(text, "other/src/a.tmp", false));
    }

    #[test]
    fn directory_only_rules_exclude_their_subtrees() {
        let text = "build/\n";
        assert!(matched(text, "build", true));
        // Files under an ignored dir are excluded via the ancestor check…
        assert!(matched(text, "build/out.o", false));
        assert!(matched(text, "vendor/build/out.o", false));
        // …while a *file* named 'build' at the root survives.
        assert!(!matched(text, "build", false));
    }

    #[test]
    fn double_star_spans_path_segments() {
        let text = "**/fixtures/**\n";
        assert!(matched(text, "tests/fixtures/data.json", false));
        assert!(matched(text, "fixtures/deep/dir/x.txt", false));
        assert!(!matched(text, "tests/fixture/data.json", false));

        let deep = "docs/**/*.md\n";
        assert!(matched(deep, "docs/guide/intro.md", false));
        // gitignore: `**/` matches *zero or more* directories
        assert!(matched(deep, "docs/intro.md", false));
    }

    #[test]
    fn question_mark_matches_single_char_within_segment() {
        assert!(matched("file?.txt", "file1.txt", false));
        assert!(!matched("file?.txt", "file10.txt", false));
        assert!(!matched("file?.txt", "file/.txt", false));
    }

    #[test]
    fn special_regex_chars_are_literal() {
        let text = "a+b.c\n";
        assert!(matched(text, "a+b.c", false));
        assert!(!matched(text, "aabbc", false));
    }
}
