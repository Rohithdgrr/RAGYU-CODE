# Future Scope

Ideas beyond the current release, roughly in priority order. Items marked ⚠ need a security
design before implementation.

## Tools & extensibility
- **Incremental symbol index** — update entries for files touched by staged
  applies instead of waiting for the next `/scan`
- **MCP-style external tool servers** — spawn/attach a helper process and map
  its tools into the registry

## Context & content
- **File attachments / RAG** — drop files into context with chunking +
  tokenizer budgeting; optional local embedding index for retrieval (the
  path-mention injection in `context.rs` is the lightweight first step)
- **Streaming markdown** — render incrementally instead of at end-of-turn
- **Smarter compaction** — keep recent tool rounds verbatim, summarize only
  older exchanges; preserve open questions list

## Platform
- **Full-TUI mode** — alternate screen, scrollback pane, mouse support
- **Packaging** — cargo-dist releases, Homebrew/Scoop/winget manifests,
  install script, CI badge + release automation

## Model features
- Structured outputs (`response_format`) command support
- Vision inputs where providers support them
- Local conversation embeddings for `/search` semantic fallback

## Quality
- Fuzzing the SSE parser (cargo-fuzz) against random chunk splits
- Property tests for window trimming/undo group invariants (proptest)
- Benchmarks (criterion) for parser throughput and window computation
