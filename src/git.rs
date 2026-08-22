//! Git integration helpers backing the `git_*` tools.
//!
//! Every operation spawns `git` directly (argv, never a shell) inside the
//! workspace root, with a hard timeout and capped output — mirroring the
//! safety posture of the shell tools. Mutating operations (`git_commit`,
//! `git_branch`) are confirmation-gated at the tool-registry layer; this
//! module only builds argv and captures results.

use std::time::Duration;

use anyhow::{Context, Result};

/// Wall-clock cap per git invocation.
const GIT_TIMEOUT: Duration = Duration::from_secs(30);
/// Combined output kept per stream.
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
/// Default number of commits returned by `git_log`.
pub const DEFAULT_LOG_COMMITS: usize = 20;
const MAX_LOG_COMMITS: usize = 200;

/// Runs one git command in `base`, returning combined stdout/stderr text.
/// A non-zero exit is an `Ok` carrying the output — the model should see
/// the failure verbatim (minus internals) rather than a bare error.
pub async fn run_git(base: &std::path::Path, argv: &[&str]) -> Result<String> {
    let spawned = tokio::process::Command::new("git")
        .arg("-C")
        .arg(base)
        .args(argv)
        .output();
    let output = match tokio::time::timeout(GIT_TIMEOUT, spawned).await {
        Err(_) => anyhow::bail!("git timed out after {}s", GIT_TIMEOUT.as_secs()),
        Ok(res) => res.with_context(|| format!("cannot spawn 'git {}'", argv.join(" ")))?,
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut text = stdout.to_string();
    if !stderr.trim().is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&stderr);
    }
    if text.len() > MAX_OUTPUT_BYTES {
        text.truncate(MAX_OUTPUT_BYTES);
        text.push_str("\n…(truncated)");
    }
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed with {}: {}",
            argv.join(" "),
            output.status,
            text.trim()
        );
    }
    Ok(text)
}

/// Builds argv for `git_diff`: working-tree + staged changes vs HEAD,
/// with a stat summary first.
pub fn diff_argv() -> Vec<&'static str> {
    vec!["diff", "HEAD"]
}

/// Builds argv for `git_log` (`--oneline`, bounded).
pub fn log_argv(max: Option<usize>) -> Vec<String> {
    let n = max.unwrap_or(DEFAULT_LOG_COMMITS).clamp(1, MAX_LOG_COMMITS);
    vec![
        "log".to_owned(),
        "--oneline".to_owned(),
        "-n".to_owned(),
        n.to_string(),
    ]
}

/// What `git_branch` should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchAction {
    List,
    Create,
    Switch,
}

impl BranchAction {
    /// Parses the tool's `action` argument.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "list" | "" => Some(Self::List),
            "create" => Some(Self::Create),
            "switch" | "checkout" => Some(Self::Switch),
            _ => None,
        }
    }

    /// argv for the action; `name` is ignored for List.
    pub fn argv(&self, name: &str) -> Result<Vec<String>> {
        match self {
            Self::List => Ok(vec!["branch".to_owned()]),
            Self::Create => {
                anyhow::ensure!(!name.trim().is_empty(), "create needs a branch name");
                Ok(vec!["branch".to_owned(), name.trim().to_owned()])
            }
            Self::Switch => {
                anyhow::ensure!(!name.trim().is_empty(), "switch needs a branch name");
                Ok(vec!["switch".to_owned(), name.trim().to_owned()])
            }
        }
    }

    /// Human label used in confirmations and errors.
    pub fn label(&self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Create => "create",
            Self::Switch => "switch",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_argv_bounds_commit_count() {
        assert_eq!(log_argv(None), vec!["log", "--oneline", "-n", "20"]);
        assert_eq!(log_argv(Some(5)), vec!["log", "--oneline", "-n", "5"]);
        assert_eq!(
            log_argv(Some(10_000)),
            vec!["log", "--oneline", "-n", "200"]
        );
        assert_eq!(log_argv(Some(0)), vec!["log", "--oneline", "-n", "1"]);
    }

    #[test]
    fn branch_actions_build_expected_argv() {
        assert_eq!(
            BranchAction::parse("list").unwrap().argv("").unwrap(),
            vec!["branch"]
        );
        assert_eq!(BranchAction::parse("CREATE").unwrap().label(), "create");
        assert_eq!(
            BranchAction::parse("checkout")
                .unwrap()
                .argv("feat")
                .unwrap(),
            vec!["switch", "feat"]
        );
        assert_eq!(
            BranchAction::parse("switch").unwrap().argv("main").unwrap(),
            vec!["switch", "main"]
        );
        assert!(BranchAction::parse("rebase").is_none());
        // create/switch demand a name
        assert!(BranchAction::Create.argv(" ").is_err());
        assert!(BranchAction::Switch.argv("").is_err());
    }

    #[tokio::test]
    async fn run_git_reports_missing_repo_cleanly() {
        let dir = std::env::temp_dir().join(format!("govinda-git-norepo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = run_git(&dir, &["status"]).await;
        assert!(err.is_err(), "expected failure outside a repo");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn run_git_works_in_a_real_repo() {
        // The GOVINDA workspace itself is a git repo when tests run from it.
        let base = std::env::current_dir().unwrap();
        if !base.join(".git").exists() {
            return;
        }
        let out = run_git(&base, &["rev-parse", "--is-inside-work-tree"])
            .await
            .unwrap();
        assert_eq!(out.trim(), "true");
    }
}
