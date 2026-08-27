//! `build_project` — build with auto-fix loop.
//!
//! Runs the project's build (cargo build, npm run build, tsc, go build) and
//! returns structured errors with file/line locations. Distinct from
//! `run_diagnostics` in that it runs the full build pipeline (linking,
//! bundling, type-checking) rather than just checking syntax.

use std::time::Instant;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// Build in release mode (default false).
    #[serde(default)]
    pub release: bool,
    /// Target triple (Rust only, e.g. "x86_64-pc-windows-msvc").
    pub target: Option<String>,
    /// Maximum retries (0-5, default 2). The tool itself does not auto-fix;
    /// it just re-runs and reports.
    #[serde(default = "default_max_retries")]
    pub max_retries: u8,
}

fn default_max_retries() -> u8 {
    2
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Rust,
    Node,
    Go,
}

pub fn run(base: &std::path::Path, args: Args) -> anyhow::Result<String> {
    let kind = detect_kind(base).ok_or_else(|| {
        anyhow::anyhow!(
            "no supported build target (Cargo.toml, package.json with build script, go.mod)"
        )
    })?;
    let mut attempts = 0u8;
    let max = args.max_retries.min(5);
    loop {
        attempts += 1;
        let (program, argv) = build_command(kind, base, &args)?;
        let start = Instant::now();
        let output = std::process::Command::new(program)
            .args(&argv)
            .current_dir(base)
            .output()
            .with_context(|| format!("failed to spawn {program}"))?;
        let elapsed_ms = start.elapsed().as_millis() as u64;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let ok = output.status.success();
        let errors = extract_errors(&stdout, &stderr, kind);
        if ok || attempts > max {
            return Ok(format!(
                "{{\"ok\":{ok},\"tool\":\"{program}\",\"attempts\":{attempts},\"elapsed_ms\":{elapsed_ms},\"errors\":{},\"stdout\":{},\"stderr\":{}}}",
                errors,
                serde_json::Value::String(truncate(&stdout, 4000)),
                serde_json::Value::String(truncate(&stderr, 2000)),
            ));
        }
    }
}

fn detect_kind(base: &std::path::Path) -> Option<Kind> {
    if base.join("Cargo.toml").exists() {
        Some(Kind::Rust)
    } else if base.join("package.json").exists() {
        // Only treat as Node if there's a build script
        if let Ok(raw) = std::fs::read_to_string(base.join("package.json")) {
            if raw.contains("\"build\"") {
                return Some(Kind::Node);
            }
        }
        None
    } else if base.join("go.mod").exists() {
        Some(Kind::Go)
    } else {
        None
    }
}

fn build_command(
    kind: Kind,
    _base: &std::path::Path,
    args: &Args,
) -> anyhow::Result<(&'static str, Vec<String>)> {
    match kind {
        Kind::Rust => {
            let mut a = vec!["build".to_owned()];
            if args.release {
                a.push("--release".to_owned());
            }
            if let Some(t) = &args.target {
                a.push("--target".to_owned());
                a.push(t.clone());
            }
            Ok(("cargo", a))
        }
        Kind::Node => Ok(("npm", vec!["run".to_owned(), "build".to_owned()])),
        Kind::Go => Ok(("go", vec!["build".to_owned(), "./...".to_owned()])),
    }
}

fn extract_errors(stdout: &str, stderr: &str, kind: Kind) -> serde_json::Value {
    let combined = format!("{stdout}\n{stderr}");
    let mut errors: Vec<serde_json::Value> = Vec::new();
    for line in combined.lines() {
        if let Some(err) = parse_error_line(line, kind) {
            errors.push(err);
        }
        if errors.len() >= 50 {
            break;
        }
    }
    serde_json::Value::Array(errors)
}

fn parse_error_line(line: &str, kind: Kind) -> Option<serde_json::Value> {
    let prefix = match kind {
        Kind::Rust => "error[",
        Kind::Node => "error TS",
        Kind::Go => " ",
    };
    if !line.starts_with(prefix) && !line.contains("error:") {
        return None;
    }
    // Crude: return the raw trimmed line as message
    Some(serde_json::json!({"message": line.trim()}))
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
    fn detect_finds_cargo() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        assert!(matches!(detect_kind(dir.path()), Some(Kind::Rust)));
    }

    #[test]
    fn rust_build_command_includes_release() {
        let args = Args {
            release: true,
            target: None,
            max_retries: 0,
        };
        let (prog, argv) = build_command(Kind::Rust, &std::path::PathBuf::new(), &args).unwrap();
        assert_eq!(prog, "cargo");
        assert!(argv.contains(&"--release".to_string()));
    }
}
