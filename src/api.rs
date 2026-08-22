use crate::provider::Provider;
use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

/// Hard cap on a single streamed answer (protects memory from runaway output).
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
/// Cap on a single buffered SSE line; a "server" that never sends a newline
/// must not balloon memory past this before the parser gives up.
const MAX_SSE_LINE_BYTES: usize = 1024 * 1024;
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(120);

/// Default per-request read-stall timeout (`/timeout` tunes a copy of this).
pub const fn default_read_timeout() -> Duration {
    DEFAULT_READ_TIMEOUT
}
const MAX_RETRIES: u32 = 3;
const RETRYABLE_STATUS: [reqwest::StatusCode; 5] = [
    reqwest::StatusCode::REQUEST_TIMEOUT,
    reqwest::StatusCode::TOO_MANY_REQUESTS,
    reqwest::StatusCode::BAD_GATEWAY,
    reqwest::StatusCode::SERVICE_UNAVAILABLE,
    reqwest::StatusCode::GATEWAY_TIMEOUT,
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: String,
    pub content: String,
    /// Tool calls requested by an assistant turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// For `role == "tool"`: id of the call this message answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    fn with_role(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_owned(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }
    pub fn system(content: impl Into<String>) -> Self {
        Self::with_role("system", content)
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self::with_role("user", content)
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::with_role("assistant", content)
    }
    /// Assistant turn that asks the model's caller to run tools.
    pub fn assistant_with_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".into(),
            content: String::new(),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    }
    /// Result of one executed tool call (`role == "tool"`).
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    pub fn has_tool_calls(&self) -> bool {
        self.tool_calls.as_ref().is_some_and(|c| !c.is_empty())
    }
}

fn default_tool_kind() -> String {
    "function".to_owned()
}

/// One requested invocation; `arguments` is a raw JSON object string, exactly
/// as the wire format carries it (streamed in fragments we reassemble).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_tool_kind")]
    pub kind: String,
    pub function: ToolCallFunction,
}

impl ToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: default_tool_kind(),
            function: ToolCallFunction {
                name: name.into(),
                arguments: arguments.into(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallFunction {
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

/// A function-calling tool advertised to the model. `parameters` is a JSON
/// Schema object describing the accepted arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

impl Tool {
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    fn to_wire(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            }
        })
    }
}

#[derive(Debug, PartialEq)]
pub enum SseEvent {
    Delta(String),
    /// One or more fully reassembled tool calls (arrive after all argument
    /// fragments for that index have been buffered).
    ToolCalls(Vec<ToolCall>),
    Done,
    ApiError(String),
}

/// Byte-safe SSE parser.
///
/// Network chunks can split anywhere — including in the middle of a multi-byte
/// UTF-8 character. Decoding `from_utf8_lossy` per chunk corrupts those
/// characters, so we buffer raw bytes and only decode *complete* lines.
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    /// Partial tool calls, indexed by the `index` field of streamed
    /// `delta.tool_calls` fragments; id/name arrive once, arguments append.
    pending_tools: Vec<Option<PartialToolCall>>,
}

#[derive(Default, Clone)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl SseParser {
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > MAX_SSE_LINE_BYTES {
            // No newline in sight: refuse to buffer a runaway line.
            self.buf.clear();
            return vec![SseEvent::ApiError(format!(
                "SSE line exceeded {} KB without a newline",
                MAX_SSE_LINE_BYTES / 1024
            ))];
        }
        let mut events = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = self.buf.drain(..=pos).collect();
            events.extend(self.parse_line(&String::from_utf8_lossy(&line_bytes)));
        }
        events
    }

    /// Emits buffered tool calls (if any) and clears the accumulator.
    fn take_tool_calls(&mut self) -> Option<SseEvent> {
        if self.pending_tools.iter().all(Option::is_none) {
            return None;
        }
        let calls: Vec<ToolCall> = std::mem::take(&mut self.pending_tools)
            .into_iter()
            .flatten()
            .map(|p| ToolCall::new(p.id, p.name, p.arguments))
            .filter(|c| !(c.id.is_empty() && c.function.name.is_empty()))
            .collect();
        (!calls.is_empty()).then_some(SseEvent::ToolCalls(calls))
    }

    fn parse_line(&mut self, line: &str) -> Vec<SseEvent> {
        let Some(data) = line.trim().strip_prefix("data:") else {
            return Vec::new();
        };
        let data = data.trim();
        if data == "[DONE]" {
            return match self.take_tool_calls() {
                Some(calls) => vec![calls, SseEvent::Done],
                None => vec![SseEvent::Done],
            };
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        if let Some(err) = v.get("error") {
            let msg = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown API error");
            return vec![SseEvent::ApiError(msg.to_owned())];
        }
        let choice = &v["choices"][0];
        let mut events = Vec::new();
        if let Some(delta) = choice["delta"].as_object() {
            if let Some(text) = delta.get("content").and_then(Value::as_str)
                && !text.is_empty()
            {
                events.push(SseEvent::Delta(text.to_owned()));
            }
            if let Some(fragments) = delta.get("tool_calls").and_then(Value::as_array) {
                for frag in fragments {
                    self.apply_tool_fragment(frag);
                }
            }
        }
        if choice["finish_reason"].as_str() == Some("tool_calls")
            && let Some(calls) = self.take_tool_calls()
        {
            events.push(calls);
        }
        events
    }

    fn apply_tool_fragment(&mut self, frag: &Value) {
        let idx = frag["index"].as_u64().unwrap_or(0) as usize;
        if self.pending_tools.len() <= idx {
            self.pending_tools.resize(idx + 1, None);
        }
        let slot = self.pending_tools[idx].get_or_insert_with(PartialToolCall::default);
        if let Some(id) = frag["id"].as_str()
            && !id.is_empty()
        {
            slot.id = id.to_owned();
        }
        if let Some(name) = frag["function"]["name"].as_str()
            && !name.is_empty()
        {
            slot.name = name.to_owned();
        }
        if let Some(args) = frag["function"]["arguments"].as_str() {
            slot.arguments.push_str(args);
        }
    }
}

pub struct ChatOptions<'a> {
    /// Bearer token; `None` for unauthenticated local runtimes.
    pub bearer: Option<&'a str>,
    pub model: &'a str,
    pub temperature: f32,
    /// Per-request cap on the streamed answer (defaults to `MAX_RESPONSE_BYTES`).
    pub max_response_bytes: usize,
    /// Per-request read-stall timeout (defaults to `DEFAULT_READ_TIMEOUT`).
    pub read_timeout: Duration,
    /// Function-calling tools advertised to the model (empty = omitted).
    pub tools: Vec<Tool>,
    /// Optional `tool_choice` override (`"auto"`, `"none"`, `"required"` or
    /// a specific-function object).
    pub tool_choice: Option<Value>,
}

impl<'a> ChatOptions<'a> {
    pub fn new(bearer: Option<&'a str>, model: &'a str, temperature: f32) -> Self {
        Self {
            bearer,
            model,
            temperature,
            max_response_bytes: MAX_RESPONSE_BYTES,
            read_timeout: DEFAULT_READ_TIMEOUT,
            tools: Vec::new(),
            tool_choice: None,
        }
    }
}

enum Attempt {
    Ok,
    Fatal(anyhow::Error),
    Retryable {
        error: anyhow::Error,
        retry_after: Option<Duration>,
    },
}

/// Streams one chat completion from `provider`; appends deltas to `out` as
/// they arrive and any requested tool calls to `tool_calls`.
///
/// Because `out` is caller-owned and filled incrementally, text already shown
/// survives any error — the REPL marks it "(interrupted)" instead of losing it.
pub async fn stream_chat(
    http: &reqwest::Client,
    provider: &dyn Provider,
    opts: &ChatOptions<'_>,
    history: &[Message],
    out: &mut String,
    tool_calls: &mut Vec<ToolCall>,
    on_delta: impl FnMut(&str),
) -> Result<()> {
    stream_chat_at(
        http,
        &provider.chat_url(),
        provider.auth().token(),
        opts,
        history,
        out,
        tool_calls,
        on_delta,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn stream_chat_at(
    http: &reqwest::Client,
    url: &str,
    bearer: Option<&str>,
    opts: &ChatOptions<'_>,
    history: &[Message],
    out: &mut String,
    tool_calls: &mut Vec<ToolCall>,
    mut on_delta: impl FnMut(&str),
) -> Result<()> {
    let mut body = json!({
        "model": opts.model,
        "temperature": opts.temperature,
        "stream": true,
        "messages": history,
    });
    if !opts.tools.is_empty() {
        body["tools"] = Value::Array(opts.tools.iter().map(Tool::to_wire).collect());
    }
    if let Some(choice) = &opts.tool_choice {
        body["tool_choice"] = choice.clone();
    }

    for attempt in 1..=MAX_RETRIES {
        // Retries are only safe before anything has been emitted, otherwise we
        // would duplicate text the user already saw.
        match attempt_once(http, url, bearer, &body, opts, out, tool_calls, &mut on_delta).await? {
            Attempt::Ok => return Ok(()),
            Attempt::Fatal(e) => return Err(e),
            Attempt::Retryable { error, retry_after } => {
                if !out.is_empty() || !tool_calls.is_empty() || attempt == MAX_RETRIES {
                    return Err(error);
                }
                let wait = retry_after.unwrap_or(Duration::from_millis(500 * u64::from(attempt)));
                eprintln!(
                    "transient error ({error:#}); retrying in {:.1}s…",
                    wait.as_secs_f32()
                );
                tokio::time::sleep(wait).await;
            }
        }
    }
    unreachable!("retry loop always returns within MAX_RETRIES iterations")
}

#[allow(clippy::too_many_arguments)]
async fn attempt_once(
    http: &reqwest::Client,
    url: &str,
    bearer: Option<&str>,
    body: &Value,
    opts: &ChatOptions<'_>,
    out: &mut String,
    tool_calls: &mut Vec<ToolCall>,
    on_delta: &mut impl FnMut(&str),
) -> Result<Attempt> {
    let mut req = http.post(url).json(body);
    if let Some(token) = bearer {
        req = req.bearer_auth(token);
    }
    let resp = match req
        // Per-request read-stall protection; runtime-tunable via /timeout.
        .timeout(opts.read_timeout)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            return Ok(transport_outcome(
                anyhow::Error::new(e).context("request failed (check your connection)"),
            ));
        }
    };

    let status = resp.status();
    if !status.is_success() {
        let retryable = RETRYABLE_STATUS.contains(&status);
        let retry_after = if retryable {
            resp.headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(parse_retry_after)
        } else {
            None
        };
        let snippet = truncate(&resp.text().await.unwrap_or_default(), 300);
        let error = anyhow::anyhow!("API error {status}: {snippet}");
        return Ok(if retryable {
            Attempt::Retryable { error, retry_after }
        } else {
            Attempt::Fatal(error)
        });
    }

    let mut stream = resp.bytes_stream();
    let mut parser = SseParser::default();

    loop {
        let chunk = match stream.next().await {
            Some(Ok(chunk)) => chunk,
            Some(Err(e)) => {
                return Ok(Attempt::Retryable {
                    error: anyhow::Error::new(e).context("connection dropped mid-stream"),
                    retry_after: None,
                });
            }
            None => return Ok(Attempt::Ok), // server closed without [DONE]; fine
        };

        for event in parser.feed(&chunk) {
            match event {
                SseEvent::Delta(text) => {
                    if out.len() + text.len() > opts.max_response_bytes {
                        return Ok(Attempt::Fatal(anyhow::anyhow!(
                            "response exceeded {} MB cap",
                            opts.max_response_bytes / (1024 * 1024)
                        )));
                    }
                    out.push_str(&text);
                    on_delta(&text);
                }
                SseEvent::ToolCalls(calls) => tool_calls.extend(calls),
                SseEvent::Done => return Ok(Attempt::Ok),
                SseEvent::ApiError(msg) => {
                    return Ok(Attempt::Fatal(anyhow::anyhow!("API error: {msg}")));
                }
            }
        }
    }
}

fn transport_outcome(error: anyhow::Error) -> Attempt {
    // Timeouts/connect/request-level failures are worth retrying.
    let retryable = error
        .chain()
        .filter_map(|c| c.downcast_ref::<reqwest::Error>())
        .any(|e| e.is_timeout() || e.is_connect() || e.is_request());
    if retryable {
        Attempt::Retryable {
            error,
            retry_after: None,
        }
    } else {
        Attempt::Fatal(error)
    }
}

/// Parses a `Retry-After` header value: plain seconds only (HTTP-date form is
/// intentionally unsupported — APIs that matter here send seconds).
fn parse_retry_after(v: &str) -> Option<Duration> {
    v.trim().parse::<u64>().ok().map(Duration::from_secs)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

pub async fn list_models(
    http: &reqwest::Client,
    url: &str,
    bearer: Option<&str>,
) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Resp {
        data: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        id: String,
    }

    let mut req = http.get(url);
    if let Some(token) = bearer {
        req = req.bearer_auth(token);
    }
    let resp: Resp = req
        .send()
        .await
        .context("failed to list models")?
        .error_for_status()
        .context("the provider rejected the models request")?
        .json()
        .await
        .context("could not parse models response")?;

    let mut ids: Vec<String> = resp.data.into_iter().map(|e| e.id).collect();
    ids.sort();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(parser: &mut SseParser, chunks: &[&[u8]]) -> Vec<SseEvent> {
        let mut events = Vec::new();
        for c in chunks {
            events.extend(parser.feed(c));
        }
        events
    }

    fn deltas(events: &[SseEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                SseEvent::Delta(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn parses_simple_stream() {
        let mut p = SseParser::default();
        let events = feed_all(
            &mut p,
            &[b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\ndata: [DONE]\n\n"],
        );
        assert_eq!(deltas(&events), vec!["Hi"]);
        assert_eq!(events.last(), Some(&SseEvent::Done));
    }

    #[test]
    fn handles_chunks_split_mid_json() {
        let line = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n";
        let mid = 17;
        let mut p = SseParser::default();
        let events = feed_all(&mut p, &[&line[..mid], &line[mid..]]);
        assert_eq!(deltas(&events), vec!["Hello"]);
    }

    #[test]
    fn survives_multibyte_char_split_across_chunks() {
        // "héllo" — é = C3 A9, split right between the two bytes.
        let mut line = b"data: {\"choices\":[{\"delta\":{\"content\":\"h".to_vec();
        line.extend_from_slice(&[0xC3]);
        let tail = {
            let mut t = vec![0xA9];
            t.extend_from_slice(b"llo\"}}]}\n");
            t
        };
        let mut p = SseParser::default();
        assert!(p.feed(&line).is_empty());
        let events = p.feed(&tail);
        assert_eq!(deltas(&events), vec!["h\u{e9}llo"]);
    }

    #[test]
    fn tolerates_crlf_and_noise_lines() {
        let mut p = SseParser::default();
        let events = feed_all(
            &mut p,
            &[
                b": keep-alive\r\n",
                b"event: ping\r\n\r\n",
                b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\r\n",
                b"data: {\"choices\":[{\"delta\":{\"content\":\"\"}}]}\r\n", // empty delta dropped
                b"data: [DONE]\r\n",
            ],
        );
        assert_eq!(deltas(&events), vec!["a"]);
        assert_eq!(events.len(), 2); // delta + done; noise skipped entirely
    }

    #[test]
    fn surfaces_api_error_payloads() {
        let mut p = SseParser::default();
        let events = feed_all(
            &mut p,
            &[b"data: {\"error\":{\"message\":\"rate limited\",\"type\":\"too_many\"}}\n"],
        );
        assert_eq!(events[0], SseEvent::ApiError("rate limited".to_owned()));
    }

    #[test]
    fn aborts_on_runaway_line_without_newline() {
        let mut p = SseParser::default();
        // Feed >1 MB with no newline anywhere.
        let chunk = vec![b'x'; 256 * 1024];
        let mut events = Vec::new();
        for _ in 0..6 {
            events.extend(p.feed(&chunk));
        }
        assert!(
            matches!(events.as_slice(), [SseEvent::ApiError(msg)] if msg.contains("newline")),
            "expected overflow error, got {events:?}"
        );
    }

    #[test]
    fn parses_retry_after_seconds() {
        assert_eq!(parse_retry_after(" 30 "), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after("abc"), None);
        assert_eq!(parse_retry_after("-1"), None);
    }

    fn tool_call_events(events: &[SseEvent]) -> Vec<Vec<ToolCall>> {
        events
            .iter()
            .filter_map(|e| match e {
                SseEvent::ToolCalls(c) => Some(c.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn reassembles_fragmented_tool_calls() {
        let mut p = SseParser::default();
        let events = feed_all(
            &mut p,
            &[
                b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"weather\",\"arguments\":\"{\\\"ci\"}}]}}]}\n",
                b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"ty\\\":\\\"Paris\\\"}\"}}]}}]}\n",
                b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n",
                b"data: [DONE]\n",
            ],
        );
        let calls = tool_call_events(&events);
        assert_eq!(
            calls,
            vec![vec![ToolCall::new("c1", "weather", r#"{"city":"Paris"}"#)]]
        );
        // ToolCalls arrives before Done, no stray deltas.
        assert_eq!(events.last(), Some(&SseEvent::Done));
    }

    #[test]
    fn reassembles_parallel_tool_calls_by_index() {
        let mut p = SseParser::default();
        let events = feed_all(
            &mut p,
            &[
                b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"a\",\"function\":{\"name\":\"f\",\"arguments\":\"1\"}},{\"index\":1,\"id\":\"b\",\"function\":{\"name\":\"g\",\"arguments\":\"2\"}}]}}]}\n",
                b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"function\":{\"arguments\":\"3\"}}]},\"finish_reason\":\"tool_calls\"}]}\n",
                b"data: [DONE]\n",
            ],
        );
        let calls = tool_call_events(&events);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0][0].function.name, "f");
        assert_eq!(calls[0][0].function.arguments, "1");
        assert_eq!(calls[0][1], ToolCall::new("b", "g", "23"));
        assert!(!matches!(events.last(), Some(&SseEvent::ToolCalls(_))));
    }

    #[test]
    fn flushes_pending_tool_calls_on_done_without_finish_reason() {
        let mut p = SseParser::default();
        let events = feed_all(
            &mut p,
            &[
                b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"x\",\"function\":{\"name\":\"t\",\"arguments\":\"{}\"}}]}}]}\n",
                b"data: [DONE]\n",
            ],
        );
        let calls = tool_call_events(&events);
        assert_eq!(calls.len(), 1, "flushed on [DONE]: {calls:?}");
        assert_eq!(events.last(), Some(&SseEvent::Done));
    }
}
