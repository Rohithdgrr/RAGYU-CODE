# Tech Stack

## Language & toolchain
- **Rust** (edition 2024), MSRV: current stable
- **cargo** for build/test; GitHub Actions CI on ubuntu-latest
- Lints: `rust_2018_idioms` + clippy with `unwrap_used`/`expect_used` denied
  outside tests; CI enforces `-D warnings`

## Runtime dependencies

| Crate | Version | Why |
|---|---|---|
| `reqwest` | 0.12 | HTTP client; features `json`, `stream` (SSE bytes), **rustls-tls** + native roots — pure-Rust TLS, no OpenSSL |
| `tokio` | 1 | async runtime (`macros`, `rt-multi-thread`, `signal`, `time`) — powers streaming, Ctrl+C select, retry sleeps |
| `futures-util` | 0.3 | `StreamExt` for SSE byte streams, `join_all` for `/variants` |
| `serde` / `serde_json` | 1 | Message/session/config serialization; JSON value surgery on the wire format |
| `toml` | 0.8 | config.toml parsing (`deny_unknown_fields`) |
| `dotenvy` | 0.15 | `.env` API-key loading |
| `reedline` | 0.50 | line editor: history, vi/emacs modes, Ctrl+C/D signals |
| `termimad` | 0.35 | terminal markdown rendering of answers/exports |
| `crossterm` | 0.29 | ANSI colors, raw-mode spinner animation |
| `tiktoken-rs` | 0.12 | real cl100k BPE counts for context budgeting (falls back to chars÷4 if vocab fails to load) |
| `chrono` | 0.4 | ISO-8601 session timestamps (clock feature only) |
| `anyhow` | 1 | error chains with context |
| `zeroize` | 1.9 | API keys held in `Zeroizing<String>` |

## Dev dependencies
- `wiremock` 0.6 — scripted SSE mock servers for integration tests

## External systems
- Any OpenAI-compatible chat-completions API (Mistral, OpenAI, Groq,
  Ollama, LM Studio, vLLM, llama.cpp server) over HTTPS or localhost
- Filesystem only otherwise: `sessions/*.json`, `.govinda_history`,
  optional `~/.config/govinda/config.toml`, optional `.env`
- No database, no telemetry

## Build & QA commands
```
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Platform notes
- Windows-first development (USERPROFILE config fallback), Unix-friendly via
  XDG_CONFIG_HOME/HOME; paths validated per-platform against traversal.
