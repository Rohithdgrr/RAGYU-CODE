//! LSP Diagnostics + Go-to-Definition Overlay
//!
//! Provides basic LSP-like diagnostics by running language-specific
//! checkers (cargo check, tsc, mypy) and parsing their output into
//! structured diagnostics. Also supports go-to-definition by querying
//! the symbol index.
//!
//! This is a lightweight LSP substitute that doesn't require a running
//! language server — it parses compiler/linter output directly.

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Severity level for a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

impl Severity {
    pub fn icon(&self) -> &'static str {
        match self {
            Severity::Error => "✖",
            Severity::Warning => "⚠",
            Severity::Info => "ℹ",
            Severity::Hint => "💡",
        }
    }
}

/// A single diagnostic message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub severity: Severity,
    pub message: String,
    pub code: Option<String>,
}

impl Diagnostic {
    pub fn display(&self) -> String {
        match self.code.as_deref() {
            Some(code) if !code.is_empty() => format!(
                "{}:{}:{}: {} {} [{}]",
                self.file, self.line, self.column, self.severity.icon(), self.message, code
            ),
            _ => format!(
                "{}:{}:{}: {} {}",
                self.file, self.line, self.column, self.severity.icon(), self.message
            ),
        }
    }
}

/// A symbol definition from the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolDef {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub signature: Option<String>,
}

/// Parses Rust compiler diagnostics from cargo check output.
pub fn parse_rust_diagnostics(output: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    // Pattern: error[E0001]: message\n  --> file.rs:line:col
    // or: warning: message\n  --> file.rs:line:col
    let re = match Regex::new(r"(error|warning|note|help)\[?(\w*)\]?: (.+?)\n\s+--> (.+?):(\d+):(\d+)") {
        Ok(r) => r,
        Err(_) => return diagnostics,
    };

    for cap in re.captures_iter(output) {
        let severity = match cap.get(1).map(|m| m.as_str()) {
            Some("error") => Severity::Error,
            Some("warning") => Severity::Warning,
            Some("note") => Severity::Info,
            Some("help") => Severity::Hint,
            _ => Severity::Info,
        };
        let code = cap.get(2).map(|m| m.as_str().to_owned()).filter(|s| !s.is_empty());
        let message = cap.get(3).map(|m| m.as_str().to_owned()).unwrap_or_default();
        let file = cap.get(4).map(|m| m.as_str().to_owned()).unwrap_or_default();
        let line = cap.get(5).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        let column = cap.get(6).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);

        diagnostics.push(Diagnostic {
            file,
            line,
            column,
            severity,
            message,
            code,
        });
    }
    diagnostics
}

/// Parses TypeScript diagnostics from tsc output.
pub fn parse_typescript_diagnostics(output: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    // Pattern: file.ts(line,col): error TS1234: message
    let re = match Regex::new(r"(.+?)\((\d+),(\d+)\): (error|warning|info) (TS\d+): (.+)") {
        Ok(r) => r,
        Err(_) => return diagnostics,
    };

    for cap in re.captures_iter(output) {
        let severity = match cap.get(4).map(|m| m.as_str()) {
            Some("error") => Severity::Error,
            Some("warning") => Severity::Warning,
            _ => Severity::Info,
        };
        diagnostics.push(Diagnostic {
            file: cap.get(1).map(|m| m.as_str().to_owned()).unwrap_or_default(),
            line: cap.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0),
            column: cap.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(0),
            severity,
            message: cap.get(6).map(|m| m.as_str().to_owned()).unwrap_or_default(),
            code: cap.get(5).map(|m| m.as_str().to_owned()),
        });
    }
    diagnostics
}

/// Parses Python diagnostics from mypy output.
pub fn parse_python_diagnostics(output: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    // Pattern: file.py:line: error: message
    let re = match Regex::new(r"(.+?):(\d+): (error|warning|note|info): (.+)") {
        Ok(r) => r,
        Err(_) => return diagnostics,
    };

    for cap in re.captures_iter(output) {
        let severity = match cap.get(3).map(|m| m.as_str()) {
            Some("error") => Severity::Error,
            Some("warning") => Severity::Warning,
            Some("note") => Severity::Info,
            _ => Severity::Info,
        };
        diagnostics.push(Diagnostic {
            file: cap.get(1).map(|m| m.as_str().to_owned()).unwrap_or_default(),
            line: cap.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0),
            column: 0,
            severity,
            message: cap.get(4).map(|m| m.as_str().to_owned()).unwrap_or_default(),
            code: None,
        });
    }
    diagnostics
}

/// Detects project type and runs the appropriate checker.
pub async fn run_diagnostics(workspace: &Path) -> Result<Vec<Diagnostic>> {
    let output = if workspace.join("Cargo.toml").is_file() {
        run_cargo_check(workspace).await?
    } else if workspace.join("tsconfig.json").is_file() {
        run_tsc_check(workspace).await?
    } else if [
        "pyproject.toml",
        "setup.py",
        "requirements.txt",
    ]
    .iter()
    .any(|f| workspace.join(f).is_file())
    {
        run_mypy_check(workspace).await?
    } else {
        return Ok(Vec::new());
    };

    let diagnostics = if workspace.join("Cargo.toml").is_file() {
        parse_rust_diagnostics(&output)
    } else if workspace.join("tsconfig.json").is_file() {
        parse_typescript_diagnostics(&output)
    } else {
        parse_python_diagnostics(&output)
    };

    Ok(diagnostics)
}

async fn run_cargo_check(workspace: &Path) -> Result<String> {
    let output = tokio::process::Command::new("cargo")
        .arg("check")
        .arg("--message-format=short")
        .current_dir(workspace)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .context("failed to run cargo check")?;

    let mut result = String::from_utf8_lossy(&output.stdout).to_string();
    result.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(result)
}

async fn run_tsc_check(workspace: &Path) -> Result<String> {
    let output = tokio::process::Command::new("npx")
        .args(["tsc", "--noEmit", "--pretty", "false"])
        .current_dir(workspace)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .context("failed to run tsc")?;

    let mut result = String::from_utf8_lossy(&output.stdout).to_string();
    result.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(result)
}

async fn run_mypy_check(workspace: &Path) -> Result<String> {
    let output = tokio::process::Command::new("python")
        .args(["-m", "mypy", ".", "--no-error-summary"])
        .current_dir(workspace)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .context("failed to run mypy")?;

    let mut result = String::from_utf8_lossy(&output.stdout).to_string();
    result.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(result)
}

/// Looks up a symbol definition by name using the symbol index.
pub fn go_to_definition(
    name: &str,
    workspace: &Path,
) -> Option<SymbolDef> {
    let index = crate::symbols::ensure(workspace);
    let hits = index.find(name, None);
    hits.into_iter().next().map(|hit| SymbolDef {
        name: hit.name.clone(),
        kind: hit.kind.to_string(),
        file: hit.file.clone(),
        line: hit.line,
        column: 0,
        signature: None,
    })
}

/// Formats diagnostics for display in the TUI.
pub fn format_diagnostics(diagnostics: &[Diagnostic], max_display: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let errors = diagnostics.iter().filter(|d| d.severity == Severity::Error).count();
    let warnings = diagnostics.iter().filter(|d| d.severity == Severity::Warning).count();

    if errors > 0 || warnings > 0 {
        lines.push(format!("{} error(s), {} warning(s)", errors, warnings));
    }

    for diag in diagnostics.iter().take(max_display) {
        lines.push(diag.display());
    }

    if diagnostics.len() > max_display {
        lines.push(format!("... and {} more", diagnostics.len() - max_display));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rust_error() {
        let output = "error[E0308]: mismatched types\n  --> src/main.rs:10:5\n   |\n10 |     let x: i32 = \"hello\";\n   |                 ^^^^^^^ expected `i32`, found `&str`\n";
        let diagnostics = parse_rust_diagnostics(output);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].file, "src/main.rs");
        assert_eq!(diagnostics[0].line, 10);
    }

    #[test]
    fn parse_typescript_error() {
        let output = "src/app.ts(15,3): error TS2322: Type 'string' is not assignable to type 'number'.\n";
        let diagnostics = parse_typescript_diagnostics(output);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].code.as_deref(), Some("TS2322"));
    }

    #[test]
    fn parse_python_error() {
        let output = "src/main.py:42: error: Name 'undefined_var' is not defined  [name-defined]\n";
        let diagnostics = parse_python_diagnostics(output);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Severity::Error);
    }
}
