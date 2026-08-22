# Architecture

Single-crate Rust application: a thin binary (`src/main.rs`) over a library
crate (`govinda_cli`) so everything is unit- and integration-testable.

## Module map

```
src/
├── main.rs            REPL loop, agent turn loop (run_turn: injections +
│                      self-correction budget), plan execution, display policy
├── lib.rs             crate root; test-only lint relaxations
├── api.rs             wire protocol: Message/Tool/ToolCall types, ChatOptions,
│                      StreamSink, byte-safe SseParser, stream_chat(_at),
│                      list_models; all hard caps live here
├── session.rs         Session state machine: history, window/window_with
│                      trimming (context injection folded into the system
│                      message), atomic tool-round commits, undo groups,
│                      save/load v2
├── sessions.rs        sessions/ directory: naming validation (anti-traversal),
│                      listing metadata, autosave naming
├── tokens.rs          cl100k BPE token counting + per-message overhead model
├── tools.rs           ToolExecutor trait, BuiltinTools (workspace / staged
│                      editing / execution / symbol / git tools), parse_args<T>,
│                      PendingEdits staged-edit queue, sandboxed path resolution
├── symbols.rs         in-memory workspace symbol index (kind/name/file/line),
│                      global snapshot store, rebuild/find
├── context.rs         context-aware windowing: detects mentioned paths,
│                      renders the file-injection block for window_with
├── git.rs             git tool helpers: direct argv spawns, argv builders
├── scan.rs            scan_project overview: project types, deps, stats, git
├── outline.rs         regex-based per-file symbol outlines (display + index)
├── ignore.rs          .govindaignore rule matching
├── diff.rs            unified-diff rendering for staged edits
├── config.rs          3-layer config merge (defaults < TOML < env), HTTP client
├── provider.rs        Provider trait + mistral/openai/groq/ollama presets,
│                      Auth enum (Zeroizing<String>), context-token clamping
├── clock.rs           injectable time helpers (ISO-8601, epoch secs)
├── render.rs          themes, paint(), markdown Renderer (termimad), Spinner
├── completions.rs     shell completion script emission
└── commands/
    ├── mod.rs         App state, dispatch() for 35 slash commands, help
    ├── display.rs     introspection commands (history/search/stats/config/tools…)
    ├── generation.rs  models/model switching, retry, compact, variants/pick
    ├── persistence.rs save/load/sessions/fork/export with path guards
    ├── edits.rs       /diff · /apply · /reject staged-edit workflow
    ├── todo.rs        persistent task list (also tracks /plan progress)
    └── plan.rs        /plan <task>: decompose → confirm → Outcome::Plan(steps)
```

## Layering rules

- `main.rs` owns **policy** (what to show, what survives an error, when a
  failure grants a self-correction round);
  `api.rs` owns **transport**; `session.rs` owns **state**;
  `commands/*` own **mutations** of App.
- `api` never touches `Session`; the caller passes an immutable
  `&[Message]` window and receives output via `StreamSink`.
- `tools::ToolExecutor` is the only extension point for new capabilities —
  the agent loop knows nothing about specific tools. It only inspects
  `requires_confirmation(name)` for the y/N gate.
- The symbol index is a global snapshot (`symbols::rebuild`/`ensure`);
  consumers never hold references across rebuilds — they clone the `Arc`.

## Key invariants

1. **No duplicate output on retry** — retries abort once any text/tool call
   was emitted (`StreamSink::has_output()`).
2. **Atomic tool rounds** — assistant prose+calls and their results are
   committed together (`commit_tool_round`) and move as one group under
   undo/window trimming.
3. **Window alignment** — the API never sees a window opening on an
   assistant/tool turn; trimming always re-aligns to the next user message.
4. **Bounded memory** — every untrusted input has a cap: 4 MB response,
   1 MB SSE line, 64 parallel calls, 256 KB arguments, 8 K-char stored result.
5. **Secrets never logged or serialized** — keys are `Zeroizing<String>` from
   env only.

## Data flow (one streamed round)

```
reqwest (rustls) ─► bytes_stream ─► SseParser.feed() ─► Vec<SseEvent>
      Delta(text) ─► StreamSink.out (+ on_delta → raw stdout)
      ToolCalls   ─► StreamSink.tool_calls       │
      ApiError    ─► Attempt::Fatal              │ spinner / Ctrl+C select
      Done        ─► Attempt::Ok                 ▼
                              run_tool_round → BuiltinTools.execute()
                              Session.commit_tool_round → next round request
```

`run_turn` wraps this loop with two policies: the user prompt's mentioned
files are injected into the window once per turn (`context.rs` →
`Session::window_with`), and any failed tool result grants up to
`MAX_FIX_ROUNDS = 3` extra rounds beyond `MAX_TOOL_ROUNDS = 5`.

## Concurrency model

- Single async runtime (`tokio`, multi-thread); the REPL itself is
  synchronous reedline, one `run_turn` at a time.
- Per round: `tokio::select!` races the stream against `ctrl_c()`.
- `/variants` fires up to 5 concurrent streams via `futures_util::join_all`,
  cloning only what each future needs so `App` is never borrowed across awaits.
- Tool execution is async by design: `execute` returns boxed futures, and
  confirmed calls in one round run concurrently under `join_all` while
  results stay ordered by call index (confirmations themselves stay
  sequential so prompts never interleave).
- Git and shell tools spawn processes directly (argv, never a shell) under
  `tokio::time::timeout` with capped output.

## Testing strategy

- Unit tests co-located per module (parser edge cases, window/undo grouping,
  config parsing, path safety).
- Integration tests use **wiremock** to serve scripted SSE:
  `tests/streaming.rs` (protocol) and `tests/tool_loop.rs` (two-phase tool
  round asserting the follow-up request carries the committed `role:"tool"`
  messages).

## Persistence formats

- Session JSON v2: `{version, created_at, updated_at, system, messages[]}`
  where messages may carry `tool_calls` / `tool_call_id`. v1 files load
  unchanged (serde defaults + role filter keeps user/assistant/tool).
- `.govinda_history` — reedline FileBackedHistory (1000 lines).
- `.govinda_todo.json` — session todo list; `/plan` steps land here and are
  ticked off as autonomous execution progresses.
- `.govinda_tools.json` — per-tool enable/disable toggles.
- The symbol index is memory-only by design: rebuilt at startup (or
  `/scan`) rather than persisted, so it can never go stale on disk.
