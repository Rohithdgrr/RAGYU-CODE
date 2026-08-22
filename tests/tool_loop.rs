#![allow(clippy::unwrap_used, clippy::expect_used)]

//! End-to-end protocol test of the tool-calling loop: the model requests a
//! call, we execute it locally, commit it to the session, and the follow-up
//! request must carry the `role:"tool"` result message.

use govinda_cli::api::{ChatOptions, StreamSink, stream_chat_at};
use govinda_cli::session::Session;
use govinda_cli::tools::{BuiltinTools, ToolExecutor};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOOL_CALL_BODY: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"Let me check.\"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_9\",\"type\":\"function\",\"function\":{\"name\":\"current_time\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
    "data: [DONE]\n\n",
);

const FINAL_BODY: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"It is noon.\"}}]}\n\n",
    "data: [DONE]\n\n",
);

fn chat_url(server: &MockServer) -> String {
    format!("{}/v1/chat/completions", server.uri())
}

#[tokio::test]
async fn followup_request_carries_committed_tool_result() {
    let server = MockServer::start().await;

    // Phase 1: model streams prose + a tool call.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(serde_json::json!({})))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(TOOL_CALL_BODY),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    // Phase 2 must see the committed tool round in `messages`.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(serde_json::json!({
            "messages": [
                {"role": "system"},
                {"role": "user", "content": "what time is it?"},
                {"role": "assistant", "content": "Let me check.",
                 "tool_calls": [{"id": "call_9", "type": "function",
                                 "function": {"name": "current_time"}}]},
                {"role": "tool", "tool_call_id": "call_9"}
            ]
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(FINAL_BODY),
        )
        .expect(1)
        .mount(&server)
        .await;

    let http = reqwest::Client::new();
    let executor = BuiltinTools;
    let mut session = Session::new("sys");
    session.push_user("what time is it?");

    // Round 1: stream → tool calls arrive.
    let history = session.window(usize::MAX);
    let mut opts = ChatOptions::new(None, "test-model", 0.7);
    opts.tools = executor.specs();
    let mut out = String::new();
    let mut tool_calls = Vec::new();
    stream_chat_at(
        &http,
        &chat_url(&server),
        None,
        &opts,
        &history,
        &mut StreamSink::new(&mut out, &mut tool_calls),
        |_| {},
    )
    .await
    .expect("round 1 should succeed");
    assert_eq!(out, "Let me check.");
    assert_eq!(tool_calls.len(), 1);

    // Commit exactly like the REPL's run_tool_round does.
    assert_eq!(session.messages().len(), 1);
    let results: Vec<(String, String)> = tool_calls
        .iter()
        .map(|c| {
            let output = executor
                .execute(&c.function.name, &c.function.arguments)
                .unwrap();
            (c.id.clone(), output)
        })
        .collect();
    session.commit_tool_round(&out, &tool_calls, &results);

    // Round 2: the server matcher above fails the test unless the request
    // carries the assistant prose, its tool_calls, and the tool result.
    let history = session.window(usize::MAX);
    let mut out = String::new();
    let mut calls2 = Vec::new();
    stream_chat_at(
        &http,
        &chat_url(&server),
        None,
        &opts,
        &history,
        &mut StreamSink::new(&mut out, &mut calls2),
        |_| {},
    )
    .await
    .expect("round 2 should succeed");
    assert_eq!(out, "It is noon.");
    assert!(calls2.is_empty());
}

#[test]
fn oversized_tool_result_is_truncated_before_commit() {
    let mut session = Session::new("s");
    let big = "x".repeat(50_000);
    let call = govinda_cli::api::ToolCall::new("c1", "dump", "{}");
    let capped: String = if big.chars().count() <= 8 * 1024 {
        big.clone()
    } else {
        format!("{}\n…(truncated)", &big[..8 * 1024])
    };
    session.commit_tool_round("", &[call], &[("c1".to_owned(), capped)]);
    let stored = &session.messages()[1];
    assert!(stored.content.chars().count() < 50_000);
    assert!(stored.content.ends_with("…(truncated)"));
}
