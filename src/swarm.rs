//! Subagents / Background Explore Agents
//!
//! Provides a `delegate_task` tool that spawns background agents for
//! parallel exploration. Each subagent gets its own context window and
//! can run tools independently.
//!
//! Usage: The model calls `delegate_task` with a task description.
//! The subagent runs in the background and returns results when done.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::api::{self, ChatOptions, Message, StreamSink};
use crate::tools::ToolExecutor;

/// Default temperature for swarm workers. Low enough that parallel
/// workers on the same prompt produce consistent answers.
const SWARM_TEMPERATURE: f32 = 0.3;

/// Result from a subagent task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentResult {
    pub task_id: String,
    pub status: String,
    pub output: String,
    pub tools_used: Vec<String>,
}

/// Configuration for a subagent.
#[derive(Debug, Clone)]
pub struct SubagentConfig {
    pub model: String,
    pub temperature: f32,
    pub max_tokens: usize,
    pub system_prompt: String,
    pub tools: Vec<api::Tool>,
}

/// Spawns a subagent to execute a task in the background.
///
/// Returns a channel receiver that will receive the result when done.
pub fn spawn_subagent(
    config: SubagentConfig,
    task: String,
    http: reqwest::Client,
    provider: Arc<dyn crate::provider::Provider>,
    tools: Vec<api::Tool>,
) -> mpsc::UnboundedReceiver<SubagentResult> {
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        let result = run_subagent(config, task, http, provider, tools).await;
        let _ = tx.send(result);
    });

    rx
}

/// Runs a subagent task and returns the result.
///
/// Each tool call requested by the model is actually executed against the
/// workspace using a read-only `BuiltinTools` executor. Mutating tools that
/// require confirmation (`write_file`, `run_shell`, `apply_edits`, …) are
/// rejected with an error result so the model can self-correct instead of
/// silently succeeding with a fake result.
async fn run_subagent(
    config: SubagentConfig,
    task: String,
    http: reqwest::Client,
    provider: Arc<dyn crate::provider::Provider>,
    tools: Vec<api::Tool>,
) -> SubagentResult {
    let task_id = format!("sub-{}", chrono::Local::now().format("%H%M%S"));

    let mut history = vec![
        Message::system(&config.system_prompt),
        Message::user(format!(
            "Execute this task and return the results:\n\n{task}"
        )),
    ];

    let mut all_output = String::new();
    let mut tools_used = Vec::new();
    let max_rounds = 5;
    // Read-only executor for the subagent sandbox.
    let executor = crate::tools::BuiltinTools::default();

    for round in 0..max_rounds {
        let auth = provider.auth();
        let max_tokens = if config.max_tokens > 0 {
            Some(config.max_tokens as u32)
        } else {
            Some(4096)
        };
        let opts = ChatOptions {
            max_response_bytes: 64 * 1024,
            max_tokens,
            tools: tools.clone(),
            ..ChatOptions::new(auth.token(), &config.model, config.temperature)
        };

        let mut sink_out = String::new();
        let mut tool_calls = Vec::new();
        let mut sink = StreamSink::new(&mut sink_out, &mut tool_calls);

        match api::stream_chat(&http, provider.as_ref(), &opts, &history, &mut sink, |_| {}).await {
            Ok(()) => {
                all_output.push_str(&sink_out);

                if tool_calls.is_empty() {
                    break;
                }

                history.push(Message::assistant_with_tool_calls(
                    &sink_out,
                    tool_calls.clone(),
                ));

                for call in &tool_calls {
                    tools_used.push(call.function.name.clone());
                    let exec_result = if executor.requires_confirmation(&call.function.name) {
                        Err(anyhow::anyhow!(
                            "tool '{}' is not allowed in subagent (read-only sandbox — requires confirmation)",
                            call.function.name
                        ))
                    } else {
                        executor
                            .execute(&call.function.name, &call.function.arguments)
                            .await
                    };
                    let raw = match exec_result {
                        Ok(v) => v,
                        Err(e) => format!("error: {e:#}"),
                    };
                    // Cap per-tool result so one huge file doesn't blow the 128k history budget.
                    let truncated = if raw.chars().count() > 8000 {
                        let cut: String = raw.chars().take(8000).collect();
                        format!("{cut}\n…(truncated at 8000 chars)")
                    } else {
                        raw
                    };
                    history.push(Message::tool(&call.id, truncated));
                }
            }
            Err(e) => {
                all_output.push_str(&format!("\n[Error in round {round}: {e}]"));
                break;
            }
        }
    }

    SubagentResult {
        task_id,
        status: "completed".to_owned(),
        output: all_output,
        tools_used,
    }
}

/// Runs a parallel exploration task and returns the result.
///
/// This is a simpler interface for the common case of exploring code. Unlike
/// the previous single-shot implementation, this now loops over tool calls
/// (read-only) so `grep`/`read_file`/`find_symbol` actually return data.
pub async fn explore(
    task: &str,
    http: &reqwest::Client,
    provider: &dyn crate::provider::Provider,
    context: &str,
) -> Result<String> {
    let system = format!(
        "You are a code exploration agent. Analyze the codebase and answer the question. \
         Be thorough but concise. Focus on:\n\
         1. Finding relevant files and functions\n\
         2. Understanding the architecture\n\
         3. Identifying patterns and conventions\n\
         4. Providing actionable insights\n\n\
         Context:\n{context}"
    );

    let mut history = vec![Message::system(&system), Message::user(task.to_owned())];
    let auth = provider.auth();
    let executor = crate::tools::BuiltinTools::default();
    // Advertise only read-only tools to the explore worker.
    let all_specs = executor.specs();
    let tools: Vec<api::Tool> = all_specs
        .into_iter()
        .filter(|t| !executor.requires_confirmation(&t.name))
        .collect();

    let max_rounds = 4;
    let mut final_out = String::new();

    for _ in 0..max_rounds {
        let opts = ChatOptions {
            max_response_bytes: 64 * 1024,
            max_tokens: Some(4096),
            tools: tools.clone(),
            ..ChatOptions::new(
                auth.token(),
                crate::config::DEFAULT_MODEL,
                SWARM_TEMPERATURE,
            )
        };
        let mut out = String::new();
        let mut pending_calls = Vec::new();
        let mut sink = StreamSink::new(&mut out, &mut pending_calls);
        api::stream_chat(http, provider, &opts, &history, &mut sink, |_| {}).await?;
        final_out.push_str(&out);
        if pending_calls.is_empty() {
            return Ok(final_out);
        }
        history.push(Message::assistant_with_tool_calls(
            &out,
            pending_calls.clone(),
        ));
        for call in &pending_calls {
            let raw = if executor.requires_confirmation(&call.function.name) {
                format!(
                    "error: tool '{}' not allowed in explore (read-only)",
                    call.function.name
                )
            } else {
                match executor
                    .execute(&call.function.name, &call.function.arguments)
                    .await
                {
                    Ok(v) => v,
                    Err(e) => format!("error: {e:#}"),
                }
            };
            let truncated = if raw.chars().count() > 8000 {
                let cut: String = raw.chars().take(8000).collect();
                format!("{cut}\n…(truncated)")
            } else {
                raw
            };
            history.push(Message::tool(&call.id, truncated));
        }
        final_out.push('\n');
    }
    Ok(final_out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagent_result_serializes() {
        let result = SubagentResult {
            task_id: "test-123".into(),
            status: "completed".into(),
            output: "Found 5 relevant files".into(),
            tools_used: vec!["grep".into(), "read_file".into()],
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("test-123"));
        assert!(json.contains("grep"));
    }
}
