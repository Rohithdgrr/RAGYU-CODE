//! Function-calling: the registry of tools the model may invoke, and the
//! built-in implementations executed locally by the REPL.

use crate::api::Tool;
use crate::clock;
use anyhow::{Context, Result, bail};

/// Executes tool calls requested by the model.
///
/// Implementations own their tools' JSON-Schema specs and behavior; the agent
/// loop in the REPL only sees names, argument JSON, and result strings.
pub trait ToolExecutor: Send + Sync {
    /// Tools advertised to the model for each turn.
    fn specs(&self) -> Vec<Tool>;

    /// Runs one call. `arguments_json` is the raw arguments object string
    /// from the model — malformed input must surface as an error string,
    /// never a panic.
    fn execute(&self, name: &str, arguments_json: &str) -> Result<String>;
}

/// The default executor: safe, dependency-free local tools.
pub struct BuiltinTools;

impl ToolExecutor for BuiltinTools {
    fn specs(&self) -> Vec<Tool> {
        vec![
            Tool::new(
                "current_time",
                "Returns the user's current local date and time in ISO-8601 format.",
                serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "count_words",
                "Counts words and characters in the given text.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "text": {"type": "string", "description": "Text to measure"}
                    },
                    "required": ["text"],
                    "additionalProperties": false
                }),
            ),
        ]
    }

    fn execute(&self, name: &str, arguments_json: &str) -> Result<String> {
        match name {
            "current_time" => Ok(clock::now_iso8601()),
            "count_words" => {
                let args: serde_json::Value = serde_json::from_str(arguments_json)
                    .context("invalid JSON arguments for count_words")?;
                let text = args
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .context("count_words requires a string 'text' argument")?;
                Ok(format!(
                    "{{\"words\":{},\"characters\":{}}}",
                    text.split_whitespace().count(),
                    text.chars().count()
                ))
            }
            other => bail!("unknown tool '{other}'"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_time_returns_timestamp() {
        let out = BuiltinTools.execute("current_time", "{}").unwrap();
        assert!(out.contains('T'), "expected ISO-8601, got {out}");
    }

    #[test]
    fn count_words_measures_text() {
        let out = BuiltinTools
            .execute("count_words", r#"{"text":"hello big world"}"#)
            .unwrap();
        assert_eq!(out, r#"{"words":3,"characters":15}"#);
    }

    #[test]
    fn malformed_arguments_error_cleanly() {
        assert!(BuiltinTools.execute("count_words", "{oops").is_err());
        assert!(BuiltinTools.execute("count_words", "{}").is_err());
    }

    #[test]
    fn unknown_tool_errors() {
        assert!(BuiltinTools.execute("rm_rf", "{}").is_err());
    }

    #[test]
    fn specs_have_unique_names_and_object_schemas() {
        let specs = BuiltinTools.specs();
        let mut names: Vec<_> = specs.iter().map(|t| t.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), specs.len());
        for s in &specs {
            assert_eq!(s.parameters["type"], "object");
        }
    }
}
