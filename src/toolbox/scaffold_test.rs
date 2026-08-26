//! `scaffold_test` — generate a starter test file for a function.
//!
//! Given a source file and a function name, produce a runnable test stub
//! with imports, mocks, and assertion scaffolding. The model then refines
//! the generated tests.

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Args {
    /// Source file containing the function.
    pub source_path: String,
    /// Function name to test.
    pub function_name: String,
    /// Test framework override (auto-detect by default).
    pub framework: Option<String>,
}

pub fn run(base: &Path, args: Args) -> anyhow::Result<String> {
    let _source = std::fs::read_to_string(base.join(&args.source_path))
        .map_err(|e| anyhow::anyhow!("cannot read source: {e}"))?;
    let kind = detect_kind(base);
    let framework = args.framework.unwrap_or_else(|| kind.to_string());
    let (test_dir, test_path, scaffold) = match kind {
        Kind::Rust => {
            let dir = "tests";
            let stem = std::path::Path::new(&args.source_path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "module".to_string());
            let path = format!("{dir}/{stem}_test.rs");
            let scaffold = rust_scaffold(&args.function_name);
            (dir, path, scaffold)
        }
        Kind::Node => {
            let stem = std::path::Path::new(&args.source_path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "module".to_string());
            let path = format!("{stem}.test.js");
            let scaffold = node_scaffold(&args.function_name);
            (".", path, scaffold)
        }
        Kind::Python => {
            let stem = args.source_path.trim_end_matches(".py").to_owned();
            let path = format!("test_{stem}.py");
            let scaffold = python_scaffold(&args.function_name);
            (".", path, scaffold)
        }
        Kind::Go => {
            let stem = std::path::Path::new(&args.source_path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "module".to_string());
            let path = format!("{stem}_test.go");
            let scaffold = go_scaffold(&args.function_name);
            (".", path, scaffold)
        }
    };
    let full = base.join(&test_path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&full, &scaffold)?;
    Ok(format!(
        "{{\"ok\":true,\"function\":\"{}\",\"framework\":\"{framework}\",\"test_dir\":\"{test_dir}\",\"test_path\":\"{test_path}\",\"bytes\":{}}}",
        args.function_name,
        scaffold.len()
    ))
}

#[derive(Clone, Copy, Debug)]
enum Kind { Rust, Node, Python, Go }

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Kind::Rust => write!(f, "rust"),
            Kind::Node => write!(f, "node"),
            Kind::Python => write!(f, "python"),
            Kind::Go => write!(f, "go"),
        }
    }
}

fn detect_kind(base: &Path) -> Kind {
    if base.join("Cargo.toml").exists() { Kind::Rust }
    else if base.join("package.json").exists() { Kind::Node }
    else if base.join("pyproject.toml").exists() || base.join("requirements.txt").exists() { Kind::Python }
    else if base.join("go.mod").exists() { Kind::Go }
    else { Kind::Rust } // sensible default
}

fn rust_scaffold(fn_name: &str) -> String {
    format!(
        "use super::*;\n\n#[cfg(test)]\nmod tests {{\n    use super::*;\n\n    #[test]\n    fn {fn_name}_works() {{\n        // TODO: call {fn_name} with a known input and assert the output\n        // let result = {fn_name}(/* args */);\n        // assert_eq!(result, /* expected */);\n    }}\n\n    #[test]\n    fn {fn_name}_handles_empty_input() {{\n        // TODO: edge case — empty / zero / None input\n    }}\n\n    #[test]\n    fn {fn_name}_handles_error_case() {{\n        // TODO: edge case — invalid input that should return an error\n    }}\n}}\n"
    )
}

fn node_scaffold(fn_name: &str) -> String {
    format!(
        "import {{ describe, it, expect }} from 'vitest';\nimport {{ {fn_name} }} from './path-to-module';\n\ndescribe('{fn_name}', () => {{\n  it('works with a known input', () => {{\n    // const result = {fn_name}(/* args */);\n    // expect(result).toBe(/* expected */);\n  }});\n\n  it('handles empty input', () => {{\n    // edge case\n  }});\n\n  it('handles error case', () => {{\n    // edge case\n  }});\n}});\n"
    )
}

fn python_scaffold(fn_name: &str) -> String {
    format!(
        "import pytest\nfrom .module import {fn_name}\n\n\ndef test_{fn_name}_works():\n    # result = {fn_name}(/* args */)\n    # assert result == /* expected */\n    pass\n\n\ndef test_{fn_name}_handles_empty_input():\n    pass\n\n\ndef test_{fn_name}_handles_error_case():\n    with pytest.raises(ValueError):\n        {fn_name}(/* invalid */)\n"
    )
}

fn go_scaffold(fn_name: &str) -> String {
    format!(
        "package main\n\nimport \"testing\"\n\nfunc Test{FnName}(t *testing.T) {{\n\t// t.Run(\"works\", func(t *testing.T) {{ ... }})\n}}\n",
        FnName = capitalize(fn_name)
    )
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_scaffold_includes_function_name() {
        let s = rust_scaffold("parse_input");
        assert!(s.contains("parse_input_works"));
        assert!(s.contains("use super::*"));
    }

    #[test]
    fn capitalize_works() {
        assert_eq!(capitalize("hello"), "Hello");
        assert_eq!(capitalize(""), "");
    }
}
