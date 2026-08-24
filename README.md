# govinda-cli

A minimal, pure-Rust CLI chatbot with streaming responses, conversation memory, and markdown rendering. Works with any OpenAI-compatible chat API: Mistral, OpenAI, Groq, Ollama, OpenRouter, NVIDIA, DeepSeek, and more.

## Features

- **Multi-provider** — one binary for any OpenAI-compatible backend (`provider = "ollama"` in the config and you're talking to localhost)
- **Runtime provider switching** — `/provider groq` or `/provider custom https://your-server/v1` swaps backends mid-session; the model cache resets and `/config save` persists the choice
- **11 provider presets** — Mistral, OpenAI, OpenRouter, NVIDIA, DeepSeek, Kimi (Moonshot), GLM (Zhipu), MiniMax, Groq, ByteZ, Ollama
- **Coding agent** — the model can read, grep, scan, stage surgical edits (`edit_file`/`insert_after`/`insert_before` reviewed via `/diff`, applied atomically with `/apply`), run shell commands/tests/compile checks, and use git tools — all sandboxed to the working directory with confirmation gates on anything destructive
- **Web tools** — `web_search` (DuckDuckGo) and `web_fetch` (URL content extraction) for internet access
- **AskUserQuestion tool** — clarification gate that pauses the agent to ask the user a question
- **Project memory** — loads instructions from `AGENTS.md`, `CLAUDE.md`, and `.govinda/memory.md` at startup and injects them into the system prompt
- **Checkpointing + rewind** — `/checkpoint` saves session state, `/rewind [id]` restores; auto-checkpoints before every turn
- **Custom skills** — drop `.md` files in `~/.config/govinda/skills/` to create custom slash commands with frontmatter metadata
- **Auto-compact** — automatically compacts session history when approaching the token budget limit (`/auto-compact on|off`)
- **Branch/PR workflow** — `/commit <message>` for git commits, `/pr create|list|branch` for branching
- **Symbol index** — an in-memory `file:line` index of functions, structs, enums, traits, impls, and macros; rebuilt automatically after `/apply`
- **Context-aware window** — files your prompt mentions ride along in the context window (plus manifest + same-dir siblings) even if they only appeared in old messages
- **Self-correction loop** — failed verifications (non-zero exit codes, compile errors) grant up to 3 extra agent rounds so the model can fix its own mistakes
- **Plan mode** — `/plan <task>` decomposes a task into steps, confirms with y/N, then executes them autonomously
- **Build pipeline** — `govinda -b "<prompt>"` runs docs → code → deps → run → preview → verify from a single prompt, with a guaranteed verify phase, fix retries, and an exit code that reflects verification
- **Function calling** — multi-round tool loops with parallel calls, streamed fragment reassembly, and hard caps on call count, argument size, and result size
- **Token-aware context** — history is trimmed with a real BPE tokenizer against a configurable token budget
- **Streaming + rendered output** — spinner while generating, then the answer is rendered as terminal markdown (or `/raw` for live token-by-token plain text)
- **Concurrent variants** — `/variants 3` fires three parallel requests and races them
- **Conversation history** — multi-turn memory with automatic context-window trimming
- **Resilient** — byte-safe SSE parsing (survives chunks split mid-UTF-8-character), retries with backoff on 429/5xx, connect/read timeouts, response size cap
- **Ctrl+C safe** — cancels the current reply and keeps whatever was generated so far
- **Pure-Rust TLS** — rustls engine; OS certificate store via `rustls-native-certs` (no OpenSSL)

## Setup

1. Install Rust: https://rustup.rs
2. Pick a provider and get a key if it needs one (e.g. https://console.mistral.ai for Mistral, https://ollama.com for a local Ollama server — no key needed)
3. Provide the key (either works):

   ```
   # .env file in this directory
   MISTRAL_API_KEY=your-key
   ```

   or as an environment variable (`setx MISTRAL_API_KEY "your-key"` on Windows). Environment variables take precedence over `.env`. Each provider preset has its own key variable; local runtimes need none.

Optional env vars: `MISTRAL_MODEL` (default `mistral-small-latest`), `MISTRAL_TEMPERATURE` (default `0.7`, clamped to 0–1).

### Providers

Set `provider` in the config file to switch backends:

| Provider | Endpoint | API key env |
|---|---|---|
| `mistral` (default) | api.mistral.ai | `MISTRAL_API_KEY` |
| `openai` | api.openai.com | `OPENAI_API_KEY` |
| `openrouter` | openrouter.ai/api/v1 | `OPENROUTER_API_KEY` |
| `nvidia` | integrate.api.nvidia.com/v1 | `NVIDIA_API_KEY` |
| `deepseek` | api.deepseek.com/v1 | `DEEPSEEK_API_KEY` |
| `kimi` | api.moonshot.cn/v1 | `KIMI_API_KEY` |
| `glm` | open.bigmodel.cn/api/paas/v4 | `GLM_API_KEY` |
| `minimax` | api.minimax.chat/v1 | `MINIMAX_API_KEY` |
| `groq` | api.groq.com/openai/v1 | `GROQ_API_KEY` |
| `bytez` | api.bytez.com/v1 | `BYTEZ_API_KEY` |
| `ollama` | localhost:11434 | none |

Any other OpenAI-compatible server works via `base_url`:

```toml
provider = "ollama"
base_url = "http://192.168.1.10:8080/v0"   # LM Studio / vLLM / llama.cpp server
```

### Config file

Settings can also live in a TOML file (default `~/.config/govinda/config.toml`, override the location with `GOVINDA_CONFIG`):

```toml
provider = "mistral"        # mistral | openai | groq | ollama | openrouter | nvidia | deepseek | kimi | glm | minimax | bytez
base_url = ""               # optional override for custom servers
api_key_env = "MY_KEY_VAR"  # optional env-var name for the API key
model = "mistral-large-latest"
temperature = 0.3
context_tokens = 16384      # tokenizer-based history budget per request
render_markdown = true
system_prompt = "You are a concise coding assistant."
```

Precedence: defaults < config.toml < environment variables. Unknown keys are rejected at startup. The API key is only ever read from the environment / `.env` — never put secrets in the TOML file.

## Run

```
cargo run --release                      # rich TUI (default in a terminal)
cargo run --release -- --repl            # legacy plain-text REPL
cargo run --release -- --resume work     # continue a saved session
cargo run --release -- -q "explain this" # one-shot: answer and exit
cargo run --release -- -b "make a todo REST API with tests"
                                         # one-prompt build pipeline (see below)
```

### Build pipeline (`--build` / `-b`)

One prompt runs the whole loop — docs, code, dependencies, run, preview, verify:

```
prompt ──► PLAN ──► [DOCS] → [CODE] → [DEPS] → [RUN] → [PREVIEW] → [VERIFY]
                        ▲                                       │
                        └────────── fix attempts ◄──────────────┘
```

1. One model call decomposes your prompt into phase-tagged steps (`[DOCS]`, `[CODE]`, `[DEPS]`, `[RUN]`, `[PREVIEW]`, `[VERIFY]`), each with per-phase tool guidance.
2. You confirm **once**; every step then executes autonomously with writes, installs, and runs auto-approved, staged edits committed per step.
3. The pipeline always ends in a `[VERIFY]` phase (tests/project checks). Failures grant up to 3 extra fix turns before giving up.
4. A ✓/✗ report prints at the end; the exit code reflects verification.

## TUI design (glassmorphism)

The terminal UI uses a **frosted-glass design with sharp edges** — layered
misty surfaces, hairline glass borders, and one glowing accent on the focused
pane. For the intended look install:

- **A Nerd Font** (required for icons — the UI never uses emoji). Any patched
  font works; e.g. *JetBrainsMono Nerd Font* or *SymbolsNFMono*.
- Recommended typography stack:
  | Role | Font |
  |---|---|
  | Headings / display | **Space Grotesk** |
  | Body / UI text | **DM Sans**, Manrope, or Inter |
  | Code / numbers | **JetBrains Mono** |

  Terminals render with the configured font, so set these in your terminal
  profile — headings are uppercase + bold, numerals land in the mono face.
- Light ("Frosted Daylight") and dark ("Midnight Glass") glass bases ship
  built-in, and all 10 named themes (`/theme <name>`) map onto them with
  matching accent sets.

### One command engine, two frontends

The TUI and the REPL share the **same command dispatcher** and the **same
agent loop** (`src/commands/`, `src/agent_loop.rs`). Every slash command
behaves identically in both — output is captured into structured messages in
the TUI (never printed over the alternate screen) and streamed to stdout in
the REPL. Tool rounds run concurrently with self-correction rounds, inline
diffs, and result previews in both.

## Commands

| Command | Description |
|---|---|
| `/help` | Show all commands |
| `/exit`, `/quit` | Quit (Ctrl+D also exits) |
| `/clear`, `/reset` | Wipe conversation history |
| `/models` | List models available to your key |
| `/provider` | List provider presets and show the active one |
| `/provider <name>` | Switch provider at runtime: `mistral` `openai` `openrouter` `nvidia` `deepseek` `kimi` `glm` `minimax` `groq` `bytez` `ollama` (key resolves from its env var) |
| `/provider <name> <base-url>` | Switch with a custom endpoint override (any OpenAI-compatible server; unknown name = keyless custom provider). Persist with `/config save` |
| `/model <name>` | Switch model; `next`/`prev` cycle, partial ids match (validated against the API) |
| `/temp <0.0-1.0>` | Set sampling temperature |
| `/system [text]` | View or set the system prompt |
| `/history` | Print the conversation so far |
| `/undo` | Remove the last exchange |
| `/retry` | Regenerate the last answer |
| `/variants [1-5]` | Generate alternate answers concurrently for the last question |
| `/pick <n>` | Commit one of the generated variants |
| `/compact` | Fold history into one API-generated summary to free context |
| `/auto-compact [on\|off]` | Toggle automatic context compaction when nearing token budget |
| `/search <text>` | Case-insensitive search through the conversation |
| `/save [name]` | Save conversation to JSON (default `sessions/`) |
| `/load <name>` | Load a saved conversation (paths restricted to `sessions/`) |
| `/sessions` | List saved sessions, newest first |
| `/fork [file]` | Snapshot the conversation without leaving it |
| `/export md\|txt [file]` | Export the conversation as Markdown or plain text |
| `/stats` | Session statistics (turns, avg latency, errors) |
| `/theme <name>` | Color theme (`default`, `mono`, `dracula`, `solarized`, `ocean`, `nord`, `gruvbox`, `tokyo-night`, `catppuccin`, `rose`) |
| `/tokens` | Token usage vs the context budget (real BPE count) |
| `/raw` | Toggle markdown rendering vs live streaming |
| `/timeout <secs>` | Per-request read-stall timeout (1–600 s) |
| `/limit <mb>` | Response size cap in MB (1–64) |
| `/tools [on\|off]` | List tools the model may call, or toggle function calling |
| `/tools en\|dis <name>` | Enable/disable a single tool (persisted in `.govinda_tools.json`) |
| `/todo [sub]` | Persistent task list: `list` · `add <text>` · `done <n>` · `undo <n>` · `rm <n>` · `clear` |
| `/diff` | Show staged edits as a unified diff (nothing applied yet) |
| `/apply` | Commit all staged edits to disk (atomic batch) + rebuild symbol index |
| `/reject` | Discard all staged edits |
| `/scan` | Rebuild the symbol index and print a workspace overview |
| `/plan <task>` | Decompose a task into steps, confirm, execute autonomously |
| `/config [save]` | Show current settings; `save` persists them to the config file |
| `/checkpoint [label]` | Save a session checkpoint for later rewind |
| `/rewind [id]` | Rewind to a saved checkpoint (most recent if no id given) |
| `/memory [add <note>]` | View or append to `.govinda/memory.md` project memory |
| `/skills` | List loaded custom skills from `~/.config/govinda/skills/` |
| `/commit <message>` | Stage all changes and git commit |
| `/pr [create\|list\|branch]` | Branch/PR workflow: create `govinda/<timestamp>` branch, list, or switch |

Input line supports up/down history recall (persisted to `.govinda_history`), slash-command completion as you type, and standard editing keys.

Sessions are saved with real ISO-8601 `created_at` / `updated_at` timestamps, and the current conversation is auto-saved on exit (named sessions keep their name; unnamed ones become `auto-<epoch>`).

## Project Memory

Govinda loads project-specific instructions from these files at startup (if present) and injects them into the system prompt:

- **`AGENTS.md`** — general agent instructions for the project
- **`CLAUDE.md`** — Claude-style instructions (also supported for compatibility)
- **`.govinda/memory.md`** — persistent memory notes that survive across sessions

Use `/memory add <note>` to append a timestamped note to `.govinda/memory.md`.

## Custom Skills

Create custom slash commands by placing `.md` files in `~/.config/govinda/skills/`:

```markdown
---
name: /review-pr
description: Review a pull request
args: required
---
Review the current pull request for code quality, bugs, and improvements.
Focus on security issues, performance, and adherence to project conventions.
```

- The `name` field becomes the slash command (e.g., `/review-pr`)
- The `body` is sent to the model as the prompt when the skill is invoked
- Skills with `args: required` show a hint when invoked without arguments
- Skills are listed with `/skills` and appear in the command dispatch

## Checkpointing

Session state is automatically checkpointed before every turn. You can also manually save checkpoints:

```
/checkpoint              # save with auto-generated label
/checkpoint before-refactor  # save with custom label
/rewind                  # rewind to most recent checkpoint
/rewind 3                # rewind to checkpoint #3
```

Checkpoints are persisted to `.govinda/checkpoints/` and survive restarts.

## Function calling

Models that support OpenAI-style tools can invoke built-ins, which execute locally in the REPL:

- `current_time`, `count_words` — simple utilities
- **Workspace**: `read_file` (with symbol outlines), `write_file`, `list_files`, `grep` — all paths sandboxed to the working directory; reads capped at 2 MB, writes confirmation-gated
- **Web**: `web_search` (DuckDuckGo search), `web_fetch` (URL content extraction)
- **Staged editing**: `edit_file` (unique-match replace), `insert_after`, `insert_before` queue edits that are reviewed via `view_diff`/`/diff` and committed atomically by `/apply`
- **Code intelligence**: `find_symbol` (kind-filtered definition lookup in the startup-built index), `explain_code` (one symbol's source block), `scan_project` (workspace overview + index refresh)
- **Execution**: `run_shell` (confirmation, timeout, output caps), `run_test` (detected runner: cargo test / pytest / npm test), `check_project` (cargo check / tsc / mypy)
- **Git**: `git_diff`, `git_log` (read-only), `git_branch`, `git_commit` (mutations confirmation-gated)
- **Interaction**: `ask_user` (clarification gate — pauses to ask the user a question)
- **User-defined** `[[tools]]` blocks in config.toml spawn external commands via argv templates with `{placeholder}` substitution — never a shell

The agent loop runs up to 5 model↔tool rounds per turn; failed rounds (non-zero exit codes, compile errors) grant up to 3 self-correction rounds. Safety rails:

- `/tools off` stops advertising tools entirely — nothing can be invoked
- at most 64 parallel calls per turn, 256 KB of arguments per call, 8 K chars per stored result
- executor failures send only a sanitized error line back to the model

## Development

```
cargo test          # unit + wiremock integration tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Notes on privacy & security

- The API key lives only in memory (zeroized where practical) and is never logged.
- Conversations saved via `/save` are plaintext JSON on disk — treat them accordingly.
- TLS certificate validation is always on.
- Checkpoints stored in `.govinda/checkpoints/` contain full conversation snapshots.

## Roadmap

File attachments/RAG · full-TUI mode · PTY panel for live command output · enhanced streaming markdown in TUI
