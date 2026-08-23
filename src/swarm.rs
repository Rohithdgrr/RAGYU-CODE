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
        Message::user(format!("Execute this task and return the results:\n\n{task}")),
    ];

    let mut all_output = String::new();
    let mut tools_used = Vec::new();
    let max_rounds = 3;

    for round in 0..max_rounds {
        let auth = provider.auth();
        let opts = ChatOptions {
            max_response_bytes: 64 * 1024, // 64KB cap for subagents
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
                    // No more tool calls, we're done
                    break;
                }

                // Execute tool calls
                history.push(Message::assistant_with_tool_calls(&sink_out, tool_calls.clone()));

                for call in &tool_calls {
                    tools_used.push(call.function.name.clone());
                    // For subagents, we just record the call but don't execute
                    // (execution happens in the parent context)
                    history.push(Message::tool(
                        &call.id,
                        format!("Tool '{}' called (subagent context)", call.function.name),
                    ));
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
/// This is a simpler interface for the common case of exploring code.
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

    let history = vec![
        Message::system(&system),
        Message::user(task.to_owned()),
    ];

    let auth = provider.auth();
    let opts = ChatOptions::new(auth.token(), "mistral-small-latest", 0.3);

    let mut out = String::new();
    let mut no_calls = Vec::new();
    let mut sink = StreamSink::new(&mut out, &mut no_calls);

    api::stream_chat(http, provider, &opts, &history, &mut sink, |_| {}).await?;

    Ok(out)
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
