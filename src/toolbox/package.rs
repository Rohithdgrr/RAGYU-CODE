//! `package_install` — add a dependency to the project's manifest.
//!
//! Detects the project kind (cargo, npm, pip, go) and runs the right
//! command, returning structured info (added version, manifest diff).

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// Package name (e.g. "serde", "react", "requests").
    pub package: String,
    /// Dev dependency (default false).
    #[serde(default)]
    pub dev: bool,
    /// Exact version constraint (e.g. "1.0", "^2.0").
    pub version: Option<String>,
}

pub fn run(base: &std::path::Path, args: Args) -> anyhow::Result<String> {
    let kind = detect_kind(base);
    let Some(kind) = kind else {
        anyhow::bail!(
            "no supported manifest (Cargo.toml, package.json, requirements.txt, go.mod) found in workspace"
        );
    };
    let spec = match &args.version {
        Some(v) => format!("{}@{}", args.package, v),
        None => args.package.clone(),
    };
    let (program, argv) = match (kind, args.dev) {
        (Kind::Cargo, false) => ("cargo", vec!["add".into(), spec.clone()]),
        (Kind::Cargo, true) => ("cargo", vec!["add".into(), "--dev".into(), spec.clone()]),
        (Kind::Npm, false) => (
            if cfg!(windows) { "npm.cmd" } else { "npm" },
            vec!["install".into(), spec.clone()],
        ),
        (Kind::Npm, true) => (
            if cfg!(windows) { "npm.cmd" } else { "npm" },
            vec!["install".into(), "--save-dev".into(), spec.clone()],
        ),
        (Kind::Pip, _) => (
            if cfg!(windows) { "pip.cmd" } else { "pip" },
            vec!["install".into(), spec.clone()],
        ),
        (Kind::Go, _) => ("go", vec!["get".into(), spec.clone()]),
    };
    let output = std::process::Command::new(program)
        .args(&argv)
        .current_dir(base)
        .output()
        .with_context(|| format!("failed to spawn {program}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let ok = output.status.success();
    Ok(format!(
        "{{\"ok\":{},\"manifest\":\"{}\",\"added\":\"{}\",\"stdout\":{},\"stderr\":{}}}",
        ok,
        manifest_path(kind),
        spec,
        serde_json::Value::String(truncate(&stdout, 2000)),
        serde_json::Value::String(truncate(&stderr, 2000)),
    ))
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Cargo,
    Npm,
    Pip,
    Go,
}

fn detect_kind(base: &std::path::Path) -> Option<Kind> {
    if base.join("Cargo.toml").exists() {
        Some(Kind::Cargo)
    } else if base.join("package.json").exists() {
        Some(Kind::Npm)
    } else if base.join("requirements.txt").exists() || base.join("pyproject.toml").exists() {
        Some(Kind::Pip)
    } else if base.join("go.mod").exists() {
        Some(Kind::Go)
    } else {
        None
    }
}

fn manifest_path(kind: Kind) -> &'static str {
    match kind {
        Kind::Cargo => "Cargo.toml",
        Kind::Npm => "package.json",
        Kind::Pip => "requirements.txt",
        Kind::Go => "go.mod",
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
    fn detect_finds_cargo_manifest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        assert!(matches!(detect_kind(dir.path()), Some(Kind::Cargo)));
    }

    #[test]
    fn detect_finds_npm_manifest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert!(matches!(detect_kind(dir.path()), Some(Kind::Npm)));
    }

    #[test]
    fn detect_finds_python_manifest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("requirements.txt"), "x").unwrap();
        assert!(matches!(detect_kind(dir.path()), Some(Kind::Pip)));
    }

    #[test]
    fn detect_returns_none_for_empty_workspace() {
        let dir = tempfile::tempdir().unwrap();
        assert!(detect_kind(dir.path()).is_none());
    }
}
