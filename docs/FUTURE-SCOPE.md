# Future Scope

Ideas beyond v0.1, roughly in priority order. Items marked ⚠ need a security
design before implementation.

## Tools & extensibility
- **User-defined shell tools** (TOML-configured commands) ⚠
  - schema: `[[tools]] name, description, command, args_template`
  - mandatory per-call confirmation prompt; output size caps; timeout;
    no secrets interpolated from the environment into arguments
- **Per-tool toggles** — `/tools enable|disable <name>`; persisted preference
- **Async `ToolExecutor`** — boxed futures so slow tools (HTTP lookups) can run
  concurrently with `join_all` while the spinner runs
- **MCP-style external tool servers** — spawn/attach a helper process and map
  its tools into the registry

## Context & content
- **File attachments / RAG** — drop files into context with chunking +
  tokenizer budgeting; optional local embedding index for retrieval
- **Streaming markdown** — render incrementally instead of at end-of-turn
- **Smarter compaction** — keep recent tool rounds verbatim, summarize only
  older exchanges; preserve open questions list

## Platform
- **Full-TUI mode** — alternate screen, scrollback pane, mouse support
- **`/config save`** — write runtime changes back to config.toml safely
- **Packaging** — cargo-dist releases, Homebrew/Scoop/winget manifests,
  install script, CI badge + release automation
- **Shell completions & pipe mode** — `govinda -q "prompt"` non-interactive
  query for scripting; stdin piping of prompts/files

## Model features
- Structured outputs (`response_format`) command support
- Vision inputs where providers support them
- Local conversation embeddings for `/search` semantic fallback

## Quality
- Fuzzing the SSE parser (cargo-fuzz) against random chunk splits
- Property tests for window trimming/undo group invariants (proptest)
- Benchmarks (criterion) for parser throughput and window computation
