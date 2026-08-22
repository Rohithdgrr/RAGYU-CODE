# API Reference

govinda-cli is an end-user CLI, but its library crate (`govinda_cli`) exposes a
small, documented Rust API. This page covers the public surface and the
provider wire protocol it targets.

## Provider wire protocol (external)

Any **OpenAI-compatible** chat-completions backend works:

- `POST {chat_url}` with JSON body:
  ```json
  {
    "model": "...",
    "temperature": 0.7,
    "stream": true,
    "messages": [ {"role": "...", "content": "..."} ],
    "tools":   [ {"type":"function","function":{ ... } } ],
    "tool_choice": "auto"
  }
  ```
  `tools` / `tool_choice` are only sent when non-empty/set.
- Response is **SSE**: `data: {...delta...}\n\n` lines ending with
  `data: [DONE]`. Tool calls arrive as incremental fragments under
  `choices[0].delta.tool_calls`, keyed by `index`.
- `GET {models_url}` (optional per provider) returns `{ "data": [ {"id": ...} ] }`.
- Auth: `Authorization: Bearer <key>` when the provider needs one.

## Core types — `govinda_cli::api`

### Message

```rust
pub struct Message {
    pub role: String,                       // system | user | assistant | tool
    pub content: String,
    pub tool_calls: Option<Vec<ToolCall>>,  // assistant turns that request calls
    pub tool_call_id: Option<String>,       // role=="tool" answers this call id
}
```

Constructors: `Message::system/user/assistant(content)`,
`assistant_with_tool_calls(content, calls)` (carries prose streamed before the
calls), `tool(id, output)`, plus `has_tool_calls()`.
All extra fields are serde-skipped when `None`, so plain v1 session files
remain valid.

### ToolCall / Tool

```rust
ToolCall::new(id, name, arguments)   // arguments = raw JSON object string
Tool::new(name, description, parameters_json_schema)
```

`Tool` serializes on the wire as
`{"type":"function","function":{"name","description","parameters"}}`.

### ChatOptions<'a>

| Field | Default | Meaning |
|---|---|---|
| `bearer` | — | token; `None` for local runtimes |
| `model` | — | model id |
| `temperature` | — | 0.0–1.0 |
| `max_response_bytes` | 4 MB | cap on one streamed answer |
| `read_timeout` | 120 s | read-stall timeout |
| `tools` | `vec![]` | advertised tools (empty = omitted) |
| `tool_choice` | `None` | `"auto"`, `"none"`, `"required"` or function object |

### Streaming

```rust
pub struct StreamSink<'a> {
    pub out: &'a mut String,
    pub tool_calls: &'a mut Vec<ToolCall>,
}

stream_chat(http, provider, opts, history, sink, on_delta) -> Result<()>
stream_chat_at(http, url, bearer, opts, history, sink, on_delta) -> Result<()>
```

Behavior:

- Byte-safe SSE parsing (chunks may split mid-UTF-8-character).
- Retries transient failures (408/429/502/503/504, timeouts, connect drops)
  up to 3 attempts with backoff, honoring `Retry-After`; never retries once
  any text or tool call has been emitted.
- Tool-call fragments are reassembled by `index`; the completed set is
  appended to `sink.tool_calls` on `finish_reason == "tool_calls"` or `[DONE]`.
- Hard caps: `MAX_PARALLEL_TOOL_CALLS = 64` slots,
  `MAX_TOOL_ARGUMENTS_BYTES = 256 KB` per call — exceeding either aborts the
  stream with an error instead of allocating unbounded memory.
- `on_delta(&str)` fires per text fragment (used for raw-mode live printing).

Also: `list_models(http, url, bearer) -> Result<Vec<String>>` (sorted ids).

## Tools — `govinda_cli::tools`

```rust
pub trait ToolExecutor: Send + Sync {
    fn specs(&self) -> Vec<Tool>;
    fn execute(&self, name: &str, arguments_json: &str) -> Result<String>;
}
```

- `BuiltinTools`: `current_time` (local ISO-8601), `count_words`
  (`{"text": ...}` → `{"words":n,"characters":n}`).
- `parse_args<T: DeserializeOwned>(json) -> Result<T>` standardizes argument
  decoding for every executor.

## Session — `govinda_cli::session`

Key operations on `Session`:

- `push_user / push_assistant / push_tool_calls(content, calls) /
  push_tool_result(id, output)`
- `commit_tool_round(prose, calls, &[(id, output)])` — atomic round commit
- `window(budget_tokens)` — tokenizer-trimmed context; always opens on a user
  turn, never splits an assistant-tool-call group from its results
- `undo()` — removes the last exchange; tool rounds move atomically
- `approx_tokens()`, `search()`, `compact_with_summary()`,
  `save_to(path)` / `load_from(path)` (JSON, format version 2; version 1 files load unchanged)

## Persistence — `sessions/`

Named sessions live at `sessions/<name>.json`. Names are validated to reject
path traversal (`..`, absolute paths, drive letters). Autosave on exit writes
the current conversation as `auto-<epoch>` unless named.

## Error handling

All fallible APIs return `anyhow::Result`. Errors carry context chains
(`error: {e:#}` prints them fully); model-facing tool failures are sanitized
to `error: tool '<name>' failed` while details print locally only.
