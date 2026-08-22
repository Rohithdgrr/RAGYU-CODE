# Features

## Core chat
- **Streaming answers** with live token-by-token output (`/raw`) or spinner +
  rendered terminal markdown (default, via termimad)
- **Multi-turn memory** with tokenizer-based context-window trimming — the
  newest turns that fit the budget are sent, always aligned to a user turn
- **Real token accounting** (`/tokens`) using cl100k BPE counts, framing included

## Function calling
- OpenAI-style tools advertised to any compatible model; built-ins execute
  locally in the REPL (`current_time`, `count_words`)
- Multi-round agent loop (up to 5 rounds per turn), parallel tool calls supported
- Streamed argument fragments reassembled byte-safely; prose before calls is
  preserved and shown
- Safety rails: `/tools` on/off switch, ≤64 parallel calls, 256 KB argument cap,
  8 K-char result cap, sanitized error lines sent to the model
- Tool rounds move atomically through undo/history/export

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
- ~26 slash commands; `/config`, `/stats`, `/theme`, `/system` introspection
