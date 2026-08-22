# TODO List

## Done (v0.1)
- [x] Streaming chat client, byte-safe SSE parser, retry/backoff
- [x] Multi-provider presets + custom `base_url`
- [x] Tokenizer-based context window + `/tokens`
- [x] Session save/load/resume/fork/autosave/export with path guards
- [x] Slash command surface (~26 commands), themes, stats
- [x] Function calling: wire types, fragment reassembly, agent loop,
      built-in tools, caps (64 calls / 256 KB args / 8 K results)
- [x] `/tools` listing + on/off switch; sanitized model-facing errors
- [x] Session v2 format; atomic tool-round undo/window groups
- [x] Review fixes P0+P1+P2 (see PROBLEM-WITH-SOLUTION.md)
- [x] Two-phase wiremock tool-loop test; CI gate green

## In progress
- [ ] (nothing — tree is clean pending commit/push of docs)

## Next up (Phase 6)
- [ ] TOML-defined shell tools + per-call confirmation prompt ⚠
- [ ] Per-tool enable/disable (`/tools enable <name>`)
- [ ] Async executor trait for slow tools, concurrent execution

## Backlog (Phases 7–8)
- [ ] File attachments / local RAG
- [ ] Streaming markdown rendering
- [ ] Smarter compaction preserving recent tool rounds
- [ ] Full-TUI mode
- [ ] `/config save` (write runtime changes back to TOML)
- [ ] Release packaging: cargo-dist, winget/Scoop/Homebrew
- [ ] Non-interactive mode (`govinda -q "..."`) + stdin piping
- [ ] SSE parser fuzzing; property tests for window/undo invariants
