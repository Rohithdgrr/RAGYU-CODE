#![allow(clippy::unwrap_used, clippy::expect_used)]

use govinda_cli::api::{ChatOptions, Message, StreamSink, Tool, ToolCall, stream_chat_at};
use std::sync::{Arc, Mutex};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SSE_BODY: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
    "data: [DONE]\n\n",
);

fn opts<'a>(key: &'a str) -> ChatOptions<'a> {
    ChatOptions::new(Some(key), "test-model", 0.7)
}

fn chat_url(server: &MockServer) -> String {
    format!("{}/v1/chat/completions", server.uri())
}

async fn mount_sse(server: &MockServer, body: &'static str) {
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn streams_deltas_and_accumulates_full_text() {
    let server = MockServer::start().await;
    mount_sse(&server, SSE_BODY).await;

    let http = reqwest::Client::new();
    let history = vec![Message::user("hi")];
    let mut out = String::new();
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink_seen = seen.clone();

    stream_chat_at(
        &http,
        &chat_url(&server),
        Some("k"),
        &opts("k"),
        &history,
        &mut StreamSink::new(&mut out, &mut Vec::new()),
        move |d| {
            sink_seen.lock().unwrap().push(d.to_owned());
        },
    )
    .await
    .expect("stream should succeed");

    assert_eq!(out, "Hello");
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["Hel".to_owned(), "lo".to_owned()]
    );
}

#[tokio::test]
async fn retries_transient_5xx_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_sse(&server, SSE_BODY).await;

    let http = reqwest::Client::new();
    let history = vec![Message::user("hi")];
    let mut out = String::new();

    stream_chat_at(
        &http,
        &chat_url(&server),
        Some("k"),
        &opts("k"),
        &history,
        &mut StreamSink::new(&mut out, &mut Vec::new()),
        |_| {},
    )
    .await
    .expect("retry after 503 should succeed");

    assert_eq!(out, "Hello");
}

#[tokio::test]
async fn auth_errors_fail_fast_without_retry() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"message":"bad key"}"#))
        .expect(1)
        .mount(&server)
        .await;

    let http = reqwest::Client::new();
    let history = vec![Message::user("hi")];
    let mut out = String::new();

    let result = stream_chat_at(
        &http,
        &chat_url(&server),
        Some("k"),
        &opts("k"),
        &history,
        &mut StreamSink::new(&mut out, &mut Vec::new()),
        |_| {},
    )
    .await;
    let err = result.expect_err("401 must fail");
    assert!(err.to_string().contains("401"), "got: {err}");
    assert!(out.is_empty());
}

#[tokio::test]
async fn reassembles_tool_calls_from_fragments() {
    // Arguments arrive split across three SSE lines, as real providers do.
    const TOOL_BODY: &str = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"weather\",\"arguments\":\"{\\\"ci\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"ty\\\": \\\"Paris\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    let server = MockServer::start().await;
    mount_sse(&server, TOOL_BODY).await;

    let http = reqwest::Client::new();
    let history = vec![Message::user("weather in Paris?")];
    let mut out = String::new();
    let mut tool_calls = Vec::new();

    stream_chat_at(
        &http,
        &chat_url(&server),
        Some("k"),
        &opts("k"),
        &history,
        &mut StreamSink::new(&mut out, &mut tool_calls),
        |_| {},
    )
    .await
    .expect("stream should succeed");

    assert_eq!(out, "");
    assert_eq!(
        tool_calls,
        vec![ToolCall::new("call_1", "weather", r#"{"city": "Paris"}"#)]
    );
}

#[tokio::test]
async fn tools_are_sent_in_request_body_when_configured() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(serde_json::json!({
            "tools": [{
                "type": "function",
                "function": {"name": "current_time"}
            }]
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(SSE_BODY),
        )
        .expect(1)
        .mount(&server)
        .await;

    let http = reqwest::Client::new();
    let history = vec![Message::user("hi")];
    let mut out = String::new();

    let mut o = opts("k");
    o.tools = vec![Tool::new(
        "current_time",
        "returns the time",
        serde_json::json!({"type": "object", "properties": {}}),
    )];

    stream_chat_at(
        &http,
        &chat_url(&server),
        Some("k"),
        &o,
        &history,
        &mut StreamSink::new(&mut out, &mut Vec::new()),
        |_| {},
    )
    .await
    .expect("request with tools should succeed");
}
