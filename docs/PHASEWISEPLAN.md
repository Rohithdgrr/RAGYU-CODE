# Phased Plan

Delivery history and forward plan. Phases 1–7 are **shipped**; 8+ are planned.

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

## Phase 6 — Workspace awareness (done)
- `scan_project` overview (project types, entry points, dependencies,
  file stats, git state); `.govindaignore` rule matching
- Regex-based symbol outlines in `read_file`; pipe mode (`govinda -q`)
- Async boxed-future executor; concurrent execution under sequential y/N gates

## Phase 7 — Extensibility & staged editing (done)
- User-defined shell tools via TOML argv templates (no shell), validated at startup
- Per-tool enable/disable persisted in `.govinda_tools.json`
- Staged surgical editing: `edit_file` / `insert_after` / `insert_before`,
  `view_diff` review, atomic `/apply`, `/reject`
- Execution tools: `run_shell`, `run_test`, `check_project`;
  `/config save`; session todo list; unified-diff module

## Phase 8 — Code intelligence & RAG (done)
- In-memory symbol index (kind/name/file/line) built at startup, refreshed
  by `/scan` or `scan_project`; zero-dep regex extraction
- `find_symbol` (kind-filtered definition lookup) and passive `explain_code`
- Context-aware windowing: mentioned files (+ manifest, siblings) injected
  into every round via `Session::window_with`
- Agent system-prompt specialization when function calling is enabled

## Phase 9 — Agent loop & planning (done)
- Self-correction loop: failed rounds grant up to `MAX_FIX_ROUNDS = 3`
  extra turns beyond the base 5
- `/plan <task>`: one planning call → ≤10 steps → y/N gate → autonomous
  step-by-step execution tracked in `/todo` (`Outcome::Plan`)
- Git tools: `git_diff`, `git_log` (read-only), `git_branch`, `git_commit`
  (mutations confirmation-gated; direct argv spawns)

## Phase 10 — Context & content (planned)
- File attachments / RAG over a local folder
- Incremental symbol-index updates on staged applies
- Streaming markdown renderer (render-as-it-arrives in markdown mode)
- Smarter compaction: preserve recent tool rounds verbatim

## Phase 11 — Productization (planned)
- Full-TUI mode, release packaging (cargo-dist), install script, badges
