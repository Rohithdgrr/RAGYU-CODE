# Phased Plan

Delivery history and forward plan. Phases 1–5 are **shipped**; 6+ are planned.

## Phase 1 — Core chat (done)
- reqwest + rustls streaming client, byte-safe SSE parser
- Message model, ChatOptions, retry/backoff with Retry-After
- reedline REPL, markdown rendering, spinner, Ctrl+C cancel keeping partial text

## Phase 2 — Memory & sessions (done)
- Session state machine, cl100k token counting, context window trimming
- Session JSON persistence (v1), named sessions, autosave, path-traversal guards
- `/history`, `/search`, `/undo`, `/retry`, `/compact`, `/fork`, `/export`

## Phase 3 — Multi-provider & config (done)
- Provider presets (mistral/openai/groq/ollama) + `base_url` custom servers
- 3-layer config merge (defaults < TOML < env), unknown-key rejection
- `/models`, `/model next|prev` with API validation, `/variants` + `/pick`
- Themes, `/stats`, runtime tunables (`/temp`, `/timeout`, `/limit`, `/raw`)

## Phase 4 — Function calling (done)
- Message model extension: `tool_calls` / `tool_call_id` (serde-compatible
  with v1 files); Tool/ToolCall wire types
- SSE fragment reassembly by index incl. parallel calls;
  `finish_reason:"tool_calls"` and `[DONE]` flush paths
- `ChatOptions.tools` / `tool_choice`; request body includes tools when set

## Phase 5 — Agent loop + hardening (done)
- `run_turn` agent loop: stream → execute → commit atomically → re-stream,
  max 5 rounds; prose-before-calls preserved
- `ToolExecutor` trait + `BuiltinTools` (`current_time`, `count_words`)
  + typed `parse_args<T>` helper
- Caps: 64 parallel calls, 256 KB arguments, 8 K results; index-cap DoS fix
- Session v2 format; atomic undo groups; window never splits tool rounds
- Security pass: sanitized model errors, `/tools` on/off switch
- Review-driven quality pass: `StreamSink`, per-turn stats, raw-mode
  double-print fix, two-phase wiremock tool-loop test, full CI gate green

## Phase 6 — Extensibility (planned)
- User-defined shell/file tools via TOML with per-tool confirmation prompts
- Async executor trait for slow tools (HTTP lookups) with concurrent execution
- `/tools enable|disable <name>` per-tool granularity

## Phase 7 — Context & content (planned)
- File attachments / RAG over a local folder
- Streaming markdown renderer (render-as-it-arrives in markdown mode)
- Smarter compaction: preserve recent tool rounds verbatim

## Phase 8 — Productization (planned)
- Full-TUI mode, config writing (`/config save`), release packaging
  (cargo-dist), install script, badges
