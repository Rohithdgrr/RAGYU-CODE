//! `lint` — style + lint checker.
//!
//! Runs the project's linter (clippy, eslint, ruff, golangci-lint) with
//! structured output. Distinguishes errors from warnings and supports
//! apply-fixes for auto-fixable violations.

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// Scope to a file or directory.
    pub path: Option<String>,
    /// Apply auto-fixes (default false = check-only).
    #[serde(default)]
    pub fix: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Rust,
    Node,
    Python,
    Go,
}

pub fn run(base: &std::path::Path, args: Args) -> anyhow::Result<String> {
    let scope = args.path.as_deref().unwrap_or(".");
    let kind = detect_kind(base).ok_or_else(|| {
        anyhow::anyhow!(
            "no supported project found (Cargo.toml, package.json, pyproject.toml, go.mod)"
        )
    })?;
    let (program, argv) = lint_command(kind, scope, args.fix)?;
    let output = std::process::Command::new(program)
        .args(&argv)
        .current_dir(base)
        .output()
        .with_context(|| format!("failed to spawn {program}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let ok = output.status.success();
    let (errors, warnings) = count_violations(&stdout, &stderr);
    Ok(format!(
        "{{\"ok\":{},\"tool\":\"{program}\",\"scope\":\"{scope}\",\"fix\":{},\"errors\":{errors},\"warnings\":{warnings},\"stdout\":{},\"stderr\":{}}}",
        ok,
        args.fix,
        serde_json::Value::String(truncate(&stdout, 4000)),
        serde_json::Value::String(truncate(&stderr, 2000)),
    ))
}

fn detect_kind(base: &std::path::Path) -> Option<Kind> {
    if base.join("Cargo.toml").exists() {
        Some(Kind::Rust)
    } else if base.join("package.json").exists() {
        Some(Kind::Node)
    } else if base.join("pyproject.toml").exists() || base.join("requirements.txt").exists() {
        Some(Kind::Python)
    } else if base.join("go.mod").exists() {
        Some(Kind::Go)
    } else {
        None
    }
}

fn lint_command(kind: Kind, scope: &str, fix: bool) -> anyhow::Result<(&'static str, Vec<String>)> {
    let program = match kind {
        Kind::Rust => "cargo",
        Kind::Node => "npx",
        Kind::Python => "ruff",
        Kind::Go => "golangci-lint",
    };
    let argv: Vec<String> = match kind {
        Kind::Rust => {
            let mut a = vec!["clippy".to_owned(), "--message-format=short".to_owned()];
            if fix {
                a.push("--fix".to_owned());
            }
            a.push("--".to_owned());
            a.push("-D".to_owned());
            a.push("warnings".to_owned());
            a
        }
        Kind::Node => {
            let mut a = vec!["eslint".to_owned(), scope.to_owned()];
            if fix {
                a.push("--fix".to_owned());
            }
            a
        }
        Kind::Python => {
            let mut a = vec!["check".to_owned(), scope.to_owned()];
            if fix {
                a.push("--fix".to_owned());
            }
            a
        }
        Kind::Go => vec!["run".to_owned(), scope.to_owned()],
    };
    Ok((program, argv))
}

fn count_violations(stdout: &str, stderr: &str) -> (usize, usize) {
    let combined = format!("{stdout}\n{stderr}");
    let errors = combined.matches("error:").count() + combined.matches("Error:").count();
    let warnings = combined.matches("warning:").count() + combined.matches("Warning:").count();
    (errors, warnings)
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

use anyhow::Context;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_finds_rust_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        assert_eq!(detect_kind(dir.path()), Some(Kind::Rust));
    }

    #[test]
    fn count_violations_finds_errors_and_warnings() {
        let (e, w) = count_violations("error: foo\nwarning: bar\nError: baz", "");
        assert_eq!(e, 2);
        assert_eq!(w, 1);
    }
}
