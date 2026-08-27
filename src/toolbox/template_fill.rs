//! `template_fill` — fill `{{var}}` placeholders in a template file.
//!
//! Separates template structure from data. Reuse templates across projects.

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// Template file path (with {{var}} placeholders).
    pub template_path: String,
    /// Variable values.
    pub variables: BTreeMap<String, String>,
    /// Where to write the rendered output.
    pub output_path: String,
}

pub fn run(base: &Path, args: Args) -> anyhow::Result<String> {
    let template = std::fs::read_to_string(base.join(&args.template_path))
        .map_err(|e| anyhow::anyhow!("cannot read template '{}': {e}", args.template_path))?;
    let mut rendered = template;
    for (k, v) in &args.variables {
        rendered = rendered.replace(&format!("{{{{{k}}}}}"), v);
    }
    // Report any unfilled placeholders
    let mut unfilled: Vec<String> = Vec::new();
    let mut rest = rendered.as_str();
    while let Some(start) = rest.find("{{") {
        if let Some(end) = rest[start + 2..].find("}}") {
            let var = &rest[start + 2..start + 2 + end];
            unfilled.push(var.to_owned());
            rest = &rest[start + 2 + end + 2..];
        } else {
            break;
        }
    }
    let out = base.join(&args.output_path);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = rendered.len();
    std::fs::write(&out, &rendered)?;
    Ok(format!(
        "{{\"ok\":true,\"template\":\"{}\",\"output\":\"{}\",\"bytes\":{bytes},\"variables\":{},\"unfilled\":{}}}",
        args.template_path,
        args.output_path,
        args.variables.len(),
        serde_json::to_string(&unfilled).unwrap_or_default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_simple_variables() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("t.txt"), "Hello, {{name}}!").unwrap();
        let mut vars = BTreeMap::new();
        vars.insert("name".into(), "World".into());
        let args = Args {
            template_path: "t.txt".into(),
            variables: vars,
            output_path: "out.txt".into(),
        };
        run(dir.path(), args).unwrap();
        let result = std::fs::read_to_string(dir.path().join("out.txt")).unwrap();
        assert_eq!(result, "Hello, World!");
    }

    #[test]
    fn reports_unfilled_placeholders() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("t.txt"), "{{a}} and {{b}}").unwrap();
        let mut vars = BTreeMap::new();
        vars.insert("a".into(), "X".into());
        let args = Args {
            template_path: "t.txt".into(),
            variables: vars,
            output_path: "out.txt".into(),
        };
        let result = run(dir.path(), args).unwrap();
        assert!(result.contains("\"b\""));
    }
}
