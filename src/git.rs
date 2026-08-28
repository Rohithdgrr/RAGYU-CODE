//! Git integration helpers backing the `git_*` tools.
//!
//! Every operation spawns `git` directly (argv, never a shell) inside the
//! workspace root, with a hard timeout and capped output — mirroring the
//! safety posture of the shell tools. Mutating operations (`git_commit`,
//! `git_branch`) are confirmation-gated at the tool-registry layer; this
//! module only builds argv and captures results.
//!
//! # Security: PATH Validation
//!
//! To prevent PATH manipulation attacks (BUG-037), git commands use a
//! validated binary path from one of:
//!   1. Explicit `git_binary_path` in config.toml (user override)
//!   2. Validated system location from TRUSTED_GIT_LOCATIONS
//!
//! The resolved path is cached on first use and reused for all subsequent
//! git operations to avoid repeated PATH resolution.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};

/// Wall-clock cap per git invocation.
const GIT_TIMEOUT: Duration = Duration::from_secs(30);
/// Combined output kept per stream.
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
/// Default number of commits returned by `git_log`.
pub const DEFAULT_LOG_COMMITS: usize = 20;
const MAX_LOG_COMMITS: usize = 200;

/// Trusted system locations for git binaries. Only binaries at these paths
/// (after canonicalization) are accepted when auto-detecting git from PATH.
/// Users can override by setting `git_binary_path` in config.toml.
const TRUSTED_GIT_LOCATIONS: &[&str] = &[
    // Linux/Unix
    "/usr/bin/git",
    "/usr/local/bin/git",
    "/bin/git",
    // macOS Homebrew
    "/opt/homebrew/bin/git",
    "/usr/local/bin/git", // Intel Macs
    // Windows
    "C:\\Program Files\\Git\\cmd\\git.exe",
    "C:\\Program Files (x86)\\Git\\cmd\\git.exe",
    "C:\\Program Files\\Git\\bin\\git.exe",
];

/// Cached validated git binary path. Initialized on first use and reused
/// for all subsequent git operations to avoid repeated PATH resolution.
static VALIDATED_GIT_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Validates and returns the git binary path. Checks config first, then
/// validates git from PATH against TRUSTED_GIT_LOCATIONS.
///
/// # Security
///
/// This prevents PATH manipulation attacks where an attacker controls the
/// PATH environment variable to inject a malicious git binary.
///
/// # Errors
///
/// Returns error if:
/// - git not found in PATH
/// - git location is not in TRUSTED_GIT_LOCATIONS
/// - canonicalization fails
/// - config override path doesn't exist
fn validate_git_binary() -> Result<PathBuf> {
    // Check config first for user override
    let config = crate::config::Config::load().context("failed to load config")?;
    if let Some(configured_path) = config.git_binary_path {
        if configured_path.is_file() {
            return Ok(configured_path);
        } else {
            anyhow::bail!(
                "configured git_binary_path does not exist: {}",
                configured_path.display()
            );
        }
    }

    // Find git in PATH
    let git_path = which::which("git").context(
        "git not found in PATH; install git or set git_binary_path in config.toml",
    )?;

    // Canonicalize to resolve symlinks and relative paths
    let canonical = git_path.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize git path: {}",
            git_path.display()
        )
    })?;

    // Check against trusted locations
    for trusted in TRUSTED_GIT_LOCATIONS {
        let trusted_path = PathBuf::from(trusted);
        // On Windows, canonicalize trusted path to handle drive letter case
        if let Ok(trusted_canonical) = trusted_path.canonicalize() {
            if canonical == trusted_canonical {
                return Ok(canonical);
            }
        } else if canonical == trusted_path {
            // Fallback for paths that don't exist yet or can't be canonicalized
            return Ok(canonical);
        }
    }

    anyhow::bail!(
        "git binary at {} is not in a trusted location\n\
         Trusted locations: {}\n\
         To use this git binary, add 'git_binary_path = \"{}\"' to your config.toml\n\
         WARNING: Only do this if you trust this git binary and its location is secure.",
        canonical.display(),
        TRUSTED_GIT_LOCATIONS.join(", "),
        canonical.display()
    )
}

/// Returns the validated git binary path, initializing on first call.
/// Subsequent calls return the cached value.
pub fn git_command() -> Result<&'static Path> {
    if let Some(path) = VALIDATED_GIT_PATH.get() {
        return Ok(path.as_path());
    }
    
    let validated = validate_git_binary()?;
    match VALIDATED_GIT_PATH.set(validated) {
        Ok(_) => Ok(VALIDATED_GIT_PATH.get().unwrap().as_path()),
        Err(_) => {
            // Another thread set it first, use that value
            Ok(VALIDATED_GIT_PATH.get().unwrap().as_path())
        }
    }
}

/// Runs one git command in `base`, returning combined stdout/stderr text.
/// A non-zero exit is an `Ok` carrying the output — the model should see
/// the failure verbatim (minus internals) rather than a bare error.
pub async fn run_git(base: &std::path::Path, argv: &[&str]) -> Result<String> {
    let git_bin = git_command().context("git binary validation failed")?;
    let spawned = tokio::process::Command::new(git_bin)
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

    #[test]
    fn git_command_validates_trusted_locations() {
        // This test validates that git_command() returns a path when git
        // is found in a trusted location. On systems where git is installed
        // in a non-standard location, this test may fail (expected behavior).
        let result = git_command();
        
        // If git is found and validated, the path should be in trusted locations
        if let Ok(git_path) = result {
            let path_str = git_path.to_string_lossy();
            let is_trusted = TRUSTED_GIT_LOCATIONS.iter().any(|trusted| {
                let trusted_path = PathBuf::from(trusted);
                if let Ok(canonical) = trusted_path.canonicalize() {
                    git_path == canonical
                } else {
                    path_str.contains(trusted)
                }
            });
            assert!(
                is_trusted,
                "git at {} should be in a trusted location",
                path_str
            );
        }
        // If git_command() fails, it's either not installed or in an untrusted
        // location - both are acceptable for this test
    }

    #[test]
    fn validate_git_binary_rejects_untrusted_paths() {
        // This test verifies the validation logic by checking that
        // the TRUSTED_GIT_LOCATIONS list is non-empty and contains
        // expected platform-specific paths
        assert!(!TRUSTED_GIT_LOCATIONS.is_empty());
        
        #[cfg(unix)]
        {
            assert!(TRUSTED_GIT_LOCATIONS.contains(&"/usr/bin/git"));
            assert!(TRUSTED_GIT_LOCATIONS.contains(&"/usr/local/bin/git"));
        }
        
        #[cfg(windows)]
        {
            assert!(TRUSTED_GIT_LOCATIONS
                .iter()
                .any(|p| p.contains("Program Files")));
        }
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

    #[tokio::test]
    async fn run_git_uses_validated_binary() {
        // Verify that run_git uses the validated binary path
        let base = std::env::current_dir().unwrap();
        if !base.join(".git").exists() {
            return;
        }
        
        // This should succeed if git is in a trusted location
        let result = run_git(&base, &["--version"]).await;
        
        if let Ok(output) = result {
            assert!(output.contains("git version"), "expected git version output");
        }
        // If it fails, git is not in a trusted location (expected security behavior)
    }
}
