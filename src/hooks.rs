//! Hooks: pre_tool, post_tool, session_start, pre_compact
//!
//! Configurable hooks that run at key lifecycle points. Defined in
//! `~/.config/govinda/config.toml` as `[[hooks]]` blocks.
//!
//! Hook types:
//! - `session_start`: runs once when the session begins
//! - `pre_tool`: runs before every tool call (can veto by returning non-zero)
//! - `post_tool`: runs after every tool call (notification/logging)
//! - `pre_compact`: runs before auto-compact (can modify what's kept)

use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// A configured hook.
#[derive(Debug, Clone, Deserialize)]
pub struct Hook {
    /// Hook type: "session_start", "pre_tool", "post_tool", "pre_compact".
    pub event: String,
    /// Shell command to run.
    pub command: String,
    /// Optional timeout in seconds (default 10).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Optional environment variables.
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Result of running a hook.
#[derive(Debug)]
pub struct HookResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl HookResult {
    /// Returns true if the hook approved (exit code 0).
    pub fn approved(&self) -> bool {
        self.exit_code == 0
    }
}

/// Runs a hook command and returns the result.
pub async fn run_hook(hook: &Hook, context: &HashMap<String, String>) -> Result<HookResult> {
    let timeout = Duration::from_secs(hook.timeout_secs.unwrap_or(10).min(60));

    // Build the command with environment variable substitution
    let mut cmd_str = hook.command.clone();
    for (key, value) in context {
        let placeholder = format!("{{{key}}}");
        cmd_str = cmd_str.replace(&placeholder, value);
    }

    let future = async {
        if cfg!(windows) {
            Command::new("cmd")
                .args(["/C", &cmd_str])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .output()
                .await
        } else {
            Command::new("sh")
                .args(["-c", &cmd_str])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .output()
                .await
        }
    };

    match tokio::time::timeout(timeout, future).await {
        Ok(result) => match result {
            Ok(output) => Ok(HookResult {
                success: output.status.success(),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                exit_code: output.status.code().unwrap_or(-1),
            }),
            Err(e) => Ok(HookResult {
                success: false,
                stdout: String::new(),
                stderr: format!("hook spawn failed: {e}"),
                exit_code: -1,
            }),
        },
        Err(_) => Ok(HookResult {
            success: false,
            stdout: String::new(),
            stderr: "hook timed out".to_owned(),
            exit_code: -1,
        }),
    }
}

/// Runs all hooks matching a given event type.
pub async fn run_hooks(
    hooks: &[Hook],
    event: &str,
    context: &HashMap<String, String>,
) -> Vec<HookResult> {
    let mut results = Vec::new();
    for hook in hooks {
        if hook.event == event {
            match run_hook(hook, context).await {
                Ok(result) => results.push(result),
                Err(e) => {
                    results.push(HookResult {
                        success: false,
                        stdout: String::new(),
                        stderr: format!("hook error: {e}"),
                        exit_code: -1,
                    });
                }
            }
        }
    }
    results
}

/// Returns true if all hooks for the given event approved (exit code 0).
pub fn all_approved(results: &[HookResult]) -> bool {
    results.iter().all(|r| r.approved())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_hook_echo() {
        let hook = Hook {
            event: "test".into(),
            command: "echo hello-hook".into(),
            timeout_secs: Some(5),
            env: HashMap::new(),
        };
        let result = run_hook(&hook, &HashMap::new()).await.unwrap();
        assert!(result.success);
        assert!(result.stdout.contains("hello-hook"));
    }

    #[tokio::test]
    async fn run_hooks_filters_by_event() {
        let hooks = vec![
            Hook {
                event: "pre_tool".into(),
                command: "echo pre".into(),
                timeout_secs: Some(5),
                env: HashMap::new(),
            },
            Hook {
                event: "post_tool".into(),
                command: "echo post".into(),
                timeout_secs: Some(5),
                env: HashMap::new(),
            },
        ];
        let results = run_hooks(&hooks, "pre_tool", &HashMap::new()).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].stdout.contains("pre"));
    }
}
