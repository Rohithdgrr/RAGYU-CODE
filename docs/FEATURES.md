# Features

## Core chat
- **Streaming answers** with live token-by-token output (`/raw`) or spinner +
  rendered terminal markdown (default, via termimad)
- **Multi-turn memory** with tokenizer-based context-window trimming — the
  newest turns that fit the budget are sent, always aligned to a user turn
- **Real token accounting** (`/tokens`) using cl100k BPE counts, framing included

## Function calling
- OpenAI-style tools advertised to any compatible model; built-ins execute
  locally in the REPL (20 built-ins — see below)
- **Workspace tools** — `read_file`, `write_file`, `list_files`, `grep`
  (regex) turn Govinda into a coding agent; all paths are sandboxed to the
  working directory (absolute paths and `..` rejected), reads are capped at
  2 MB/60 K chars, writes at 1 MB, and every write requires an interactive
  y/N confirmation with a preview of the arguments
- **Surgical editing with staged applies** — `edit_file` (exact search/
  replace, fails unless the match is unique), `insert_after`, and
  `insert_before` never write directly: they validate against the current
  file contents and queue a staged edit. `view_diff` returns a unified diff
  of everything pending; the user reviews it with `/diff` and commits with
  `/apply` (atomic batch — overlapping or ambiguous edits abort the whole
  batch with nothing written) or discards with `/reject`
- **Execution tools** — `run_shell` runs a command in the project directory
  through the platform shell with confirmation, timeout (default 60 s,
  max 600 s) and output caps; `run_test` wraps the detected test runner
  (Rust → cargo test, Python → pytest, JS → npm test) with an optional name
  filter; `check_project` runs compile/lint validation (cargo check,
  tsc --noEmit, mypy) so errors flow back into the conversation
- **User-defined shell tools** — `[[tools]]` blocks in config.toml
  (`name`, `description`, `command`, `args_template`, optional
  `timeout_secs` / `max_output_bytes`) run external commands without a shell:
  argv templates with `{placeholder}` substitution only, mandatory per-call
  confirmation, hard timeout (default 30 s, max 600 s), and output caps
  (default 64 KiB, max 1 MiB); invalid definitions are rejected at startup
- **Per-tool toggles** — `/tools enable|disable <name>` excludes individual
  tools from what the model sees, persisted in `.govinda_tools.json`;
  `/tools` lists the registry with on/disabled markers
- **Async executor** — tool calls execute as boxed futures concurrently
  under the REPL (confirmations stay sequential so prompts never interleave)
- Multi-round agent loop with **self-correction**: failed tool rounds (non-zero exit codes, declined calls, executor errors) grant up to 3 extra fix rounds so the model can react to its own failures
- Parallel tool calls supported
- Streamed argument fragments reassembled byte-safely; prose before calls is
  preserved and shown
- Safety rails: `/tools` on/off switch, ≤64 parallel calls, 256 KB argument cap,
  8 K-char result cap, sanitized error lines sent to the model
- Tool rounds move atomically through undo/history/export

## Code intelligence
- **Symbol index** — built at startup (refreshed by `/scan` or any
  `scan_project` call): functions, structs, enums, unions, traits, impls,
  modules, and macros mapped to `file:line` across the workspace, using the
  same zero-dependency regex extraction as `read_file` outlines; respects
  `.govindaignore`, skips binaries and files over 1 MB, capped at 20 K symbols
- **`find_symbol`** — locates definitions by name with kind filters
  (`function`, `struct`, `trait`, …); exact matches rank first, then
  case-insensitive and substring hits, so searching `Runner` also finds
  `impl Runner for Config`
- **`explain_code`** — read-only helper that returns one symbol's source
  block (or an outline plus the head of a file) so the model can explain it
- **Context-aware windowing** — when a prompt mentions workspace paths
  (`src/api.rs`), the relevant files ride along in the context window even
  if they only appeared in old messages: mentioned file + manifest +
  same-dir siblings (capped at 6 files / 12 K chars), folded into the system
  message by `Session::window_with`
- **Agent system-prompt specialization** — with tools enabled the system
  prompt gains coding-agent guidance (stage edits, verify with cargo check,
  locate symbols instead of guessing line numbers)

## Agent planning
- **`/plan <task>`** — decomposes a task via one planning call (workspace
  overview attached for grounding) into at most 10 concrete steps, stores
  them as the `/todo` list, and after an explicit y/N confirmation executes
  each step autonomously through the normal agent loop; destructive calls
  keep their per-call confirmations

## Providers
- One binary for Mistral, OpenAI, Groq, Ollama presets + any custom
  OpenAI-compatible server via `base_url` (LM Studio, vLLM, llama.cpp…)
- Runtime model switching with API validation: `/model <name|next|prev>`,
  fuzzy matching, cached `/models` listing

## Conversation management
- Named sessions: `/save`, `/load`, `/sessions`, `--resume <name>`,
  autosave on exit (`auto-<epoch>`), path-traversal-safe names
- `/undo` (atomic exchange removal), `/retry`, `/clear`
- `/compact` — folds history into one API-generated summary to free context
- `/variants [1-5]` + `/pick <n>` — concurrent alternate answers, commit one
- `/search` — case-insensitive history search; `/history`; `/fork` snapshots
- `/export md|txt` — Markdown or plain-text export incl. tool rounds

- **Git tools** — `git_diff` (uncommitted changes vs HEAD, read-only),
  `git_log` (bounded `--oneline` history), `git_branch` (list/create/switch;
  mutations confirmation-gated), and `git_commit` (`add -A` + commit with a
  model-proposed message, always confirmed); all spawn git directly via argv
  with a 30 s timeout and capped output

## Resilience & safety
- Byte-safe SSE parsing (chunks split mid-multi-byte-character survive)
- Retries with backoff on 408/429/502/503/504 and transport errors,
  honoring `Retry-After`; never duplicates emitted output on retry
- Connect timeout (10 s), read-stall timeout (tunable `/timeout` 1–600 s),
  response size cap (`/limit` 1–64 MB), 1 MB SSE-line guard
- Ctrl+C cancels the current reply but keeps partial output marked
  *(interrupted)*; failed turns roll back cleanly
- Secrets only via environment / `.env` (zeroized in memory), never in TOML;
  TLS certificate validation always on (rustls)

## Interface
- reedline input: persistent history (`.govinda_history`), vi/emacs editing
- Five color themes (`default`, `mono`, `dracula`, `solarized`, `ocean`)
- ~35 slash commands; `/config`, `/stats`, `/theme`, `/system` introspection;
  `/diff` / `/apply` / `/reject` review-commit workflow for staged edits;
  `/scan` symbol index + workspace overview; `/plan` guided task execution
