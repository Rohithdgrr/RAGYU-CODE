# govinda-cli

A minimal, pure-Rust CLI chatbot with streaming responses, conversation memory, and markdown rendering. Works with any OpenAI-compatible chat API: Mistral, OpenAI, Groq, Ollama, LM Studio, vLLM…

## Features

- **Multi-provider** — one binary for any OpenAI-compatible backend (`provider = "ollama"` in the config and you're talking to localhost)
- **Function calling** — the model can invoke locally executed tools (`current_time`, `count_words`); multi-round tool loops with parallel calls, streamed fragment reassembly, and hard caps on call count, argument size, and result size
- **Token-aware context** — history is trimmed with a real BPE tokenizer against a configurable token budget
- **Streaming + rendered output** — spinner while generating, then the answer is rendered as terminal markdown (or `/raw` for live token-by-token plain text)
- **Concurrent variants** — `/variants 3` fires three parallel requests and races them
- **Conversation history** — multi-turn memory with automatic context-window trimming
- **Resilient** — byte-safe SSE parsing (survives chunks split mid-UTF-8-character), retries with backoff on 429/5xx, connect/read timeouts, 4 MB response cap
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

   or as an environment variable (`setx MISTRAL_API_KEY "your-key"` on Windows). Environment variables take precedence over `.env`. Each provider preset has its own key variable (`MISTRAL_API_KEY`, `OPENAI_API_KEY`, `GROQ_API_KEY`); local runtimes need none.

Optional env vars: `MISTRAL_MODEL` (default `mistral-small-latest`), `MISTRAL_TEMPERATURE` (default `0.7`, clamped to 0–1).

### Providers

Set `provider` in the config file to switch backends:

| Provider | Endpoint | API key env |
|---|---|---|
| `mistral` (default) | api.mistral.ai | `MISTRAL_API_KEY` |
| `openai` | api.openai.com | `OPENAI_API_KEY` |
| `groq` | api.groq.com | `GROQ_API_KEY` |
| `ollama` | localhost:11434 | none |

Any other OpenAI-compatible server works via `base_url`:

```toml
provider = "ollama"
base_url = "http://192.168.1.10:8080/v0"   # LM Studio / vLLM / llama.cpp server
```

### Config file

Settings can also live in a TOML file (default `~/.config/govinda/config.toml`, override the location with `GOVINDA_CONFIG`):

```toml
provider = "mistral"        # mistral | openai | groq | ollama
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
cargo run --release                 # start a new conversation
cargo run --release -- --resume work   # continue a saved session
```

## Commands

| Command | Description |
|---|---|
| `/help` | Show all commands |
| `/exit`, `/quit` | Quit (Ctrl+D also exits) |
| `/clear`, `/reset` | Wipe conversation history |
| `/models` | List models available to your key |
| `/model <name>` | Switch model; `next`/`prev` cycle, partial ids match (validated against the API) |
| `/temp <0.0-1.0>` | Set sampling temperature |
| `/system [text]` | View or set the system prompt |
| `/history` | Print the conversation so far |
| `/undo` | Remove the last exchange |
| `/retry` | Regenerate the last answer |
| `/variants [1-5]` | Generate alternate answers concurrently for the last question |
| `/pick <n>` | Commit one of the generated variants |
| `/compact` | Fold history into one API-generated summary to free context |
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
| `/todo [sub]` | Persistent task list: `list` · `add <text>` · `done <n>` · `undo <n>` · `rm <n>` · `clear` |
| `/config` | Show current settings |

Input line supports up/down history recall (persisted to `.govinda_history`) and standard editing keys.

Sessions are saved with real ISO-8601 `created_at` / `updated_at` timestamps, and the current conversation is auto-saved on exit (named sessions keep their name; unnamed ones become `auto-<epoch>`).

## Function calling

Models that support OpenAI-style tools can call built-ins, which execute locally in the REPL:

- `current_time` — the user's local date/time (ISO-8601)
- `count_words` — word/character counts for a given text

The agent loop runs up to 5 model↔tool rounds per turn: each requested call executes immediately (its invocation and truncated result print dimmed), results go back to the model as `role: "tool"` messages, and tool rounds move atomically in history (`/undo` never splits one). Safety rails:

- `/tools off` stops advertising tools entirely — nothing can be invoked
- at most 64 parallel calls per turn, 256 KB of arguments per call, 8 K chars per stored result
- executor failures send only a sanitized error line back to the model

New Rust tools plug in by implementing the `ToolExecutor` trait (`src/tools.rs`) and registering it on `App`.

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

## Roadmap

User-defined shell tools · file attachments/RAG · full-TUI mode · config-file writing (`/config save`)
