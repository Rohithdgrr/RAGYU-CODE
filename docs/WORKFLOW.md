# Workflows

## 1. One user turn (the hot path)

```
user input
   │
   ▼
handle_line() ── starts with '/'? ──► commands::dispatch() ─► Outcome::{Handled, Exit,
   │ no                                                    │  Resend, Plan}
   ▼                                                       │ Resend(text) ─┐
run_turn(app, input)                                       ◄───────────────┘
   │  session.push_user(input)                              │ Plan(steps) ──┐
   │  injection = context::relevant_files(input)            ▼               │
   ▼             → build_injection()          execute_plan(): y/N gate, then
┌─ agent loop (5 rounds + up to 3 fix) ──────────────────────────────────┐
│ history = session.window_with(budget, injection) # trimmed + injected  │
│ opts    = chat_options(app)                   # cached tool specs     │
│ stream_round(app, &history, &opts)            # spinner + Ctrl+C race │
│      │                                                               │
│      ├─ text only ──► finish_text_answer(): render + push_assistant  │
│      ├─ tool calls ─► show prose → run_tool_round():                 │
│      │                   execute each call locally,                  │
│      │                   session.commit_tool_round(prose,calls,res)  │
│      │                   on failure: grant fix round (max 3)         │
│      │                   continue loop (model sees results next)     │
│      └─ error ───────► handle_round_error(): keep "(interrupted)"    │
│                         partial text OR roll back to pre-round state │
└───────────────────────────────────────────────────────────────────────┘
```

## 2. Tool execution round

1. Model streams `delta.tool_calls` fragments → `SseParser` reassembles by
   `index` → `SseEvent::ToolCalls`.
2. REPL prints each call dimmed: `→ name(arguments)`.
3. Session commits the assistant message (prose + calls) **first**.
4. Each call runs through `ToolExecutor::execute`; output prints truncated to
   one line (`← …`) and is stored capped at 8 K chars.
5. Failures print details locally; the model receives only
   `error: tool '<name>' failed`.
6. Loop streams again so the model sees the `role:"tool"` results.
7. If any round result signalled failure (`error:` prefix, non-zero
   `exit_code`, declined call), up to `MAX_FIX_ROUNDS = 3` extra rounds are
   granted beyond the base 5 so the model can self-correct.

## 3. Startup

```
main()
 ├─ parse args (--resume <name> | --help)
 ├─ Config::load()          defaults < config.toml < env vars (.env fallback)
 ├─ Config::http_client()   rustls, connect 10 s / read 120 s
 ├─ Renderer::new(markdown)
 ├─ Session::new(system_prompt)  or  sessions::load_named(name)
 ├─ App::new(...)           builds BuiltinTools + caches specs once
 ├─ specialize_system()     agent addendum appended when tools are on
 ├─ symbols::rebuild(cwd)   in-memory symbol index (/scan refreshes)
 └─ reedline REPL loop until Ctrl+D or /exit → autosave()
```

## 4. Plan workflow (`/plan <task>`)

1. One planning call: system prompt demands a bare numbered list (≤10 steps);
   `scan_project`'s workspace overview is attached for grounding.
2. Steps replace `/todo` contents and print; dispatch returns
   `Outcome::Plan(steps)`.
3. main.rs asks `proceed? [y/N]` — declining keeps the plan in `/todo`.
4. Each step runs through the normal `run_turn` loop (confirmations and
   self-correction stay active) and is ticked off in `/todo` on completion.

## 5. Persistence workflow

- `/save [name]` → `sessions/<name>.json` (v2 format, timestamps stamped)
- `/load <name>` / `--resume name` → foreign roles dropped except
  user/assistant/tool; legacy v1 files load unchanged
- Exit → autosave named session keeps its name; unnamed becomes `auto-<epoch>`
- `/export md|txt [file]` → human-readable transcript incl. tool ids

## 6. Developer workflow

```
cargo test                                   # unit + wiremock integration
cargo clippy --all-targets -- -D warnings    # CI gate
cargo fmt --check                            # CI gate
cargo run --release                          # try it against a provider
```

CI (GitHub Actions, on push to main / PRs): fmt check → clippy `-D warnings`
→ tests. All three must pass locally before pushing.

## 7. Configuration workflow

1. Put the API key in `.env` (`MISTRAL_API_KEY=...`) or environment — never in TOML.
2. Optional `~/.config/govinda/config.toml` (override path with `GOVINDA_CONFIG`);
   unknown keys are a hard startup error.
3. Env overrides win: `GOVINDA_PROVIDER`, `MISTRAL_MODEL`, `MISTRAL_TEMPERATURE`.
4. Runtime tuning via `/temp`, `/timeout`, `/limit`, `/model`, `/tools`, `/raw`, `/theme`.
