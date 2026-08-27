//! `format` — run the project's formatter (rustfmt, prettier, black, gofmt).

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// Scope to a file or directory (default = all).
    pub path: Option<String>,
    /// Check-only mode (default false = write).
    #[serde(default)]
    pub check: bool,
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
    let (program, argv) = format_command(kind, scope, args.check);
    let output = std::process::Command::new(program)
        .args(&argv)
        .current_dir(base)
        .output()
        .with_context(|| format!("failed to spawn {program}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let ok = output.status.success();
    Ok(format!(
        "{{\"ok\":{},\"tool\":\"{program}\",\"scope\":\"{scope}\",\"check\":{},\"stdout\":{},\"stderr\":{}}}",
        ok,
        args.check,
        serde_json::Value::String(truncate(&stdout, 2000)),
        serde_json::Value::String(truncate(&stderr, 1000)),
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

fn format_command(kind: Kind, scope: &str, check: bool) -> (&'static str, Vec<String>) {
    match kind {
        Kind::Rust => (
            "cargo",
            if check {
                vec!["fmt".into(), "--check".into()]
            } else {
                vec!["fmt".into(), "--".into(), scope.into()]
            },
        ),
        Kind::Node => (
            "npx",
            vec![
                "prettier".into(),
                if check {
                    "--check".into()
                } else {
                    "--write".into()
                },
                scope.into(),
            ],
        ),
        Kind::Python => (
            "black",
            if check {
                vec!["--check".into(), scope.into()]
            } else {
                vec![scope.into()]
            },
        ),
        Kind::Go => (
            "gofmt",
            if check {
                vec!["-l".into(), scope.into()]
            } else {
                vec!["-w".into(), scope.into()]
            },
        ),
    }
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
    fn detect_finds_python_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pyproject.toml"), "[tool.black]").unwrap();
        assert_eq!(detect_kind(dir.path()), Some(Kind::Python));
    }

    #[test]
    fn format_command_rust_check_flag() {
        let (_, argv) = format_command(Kind::Rust, ".", true);
        assert!(argv.contains(&"--check".to_string()));
    }
}
