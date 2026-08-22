# TODO List

## Done (v0.1)
- [x] Streaming chat client, byte-safe SSE parser, retry/backoff
- [x] Multi-provider presets + custom `base_url`
- [x] Tokenizer-based context window + `/tokens`
- [x] Session save/load/resume/fork/autosave/export with path guards
- [x] Slash command surface (35 commands incl. runtime tuning), themes, stats
- [x] Function calling: wire types, fragment reassembly, agent loop,
      built-in tools, caps (64 calls / 256 KB args / 8 K results)
- [x] `/tools` listing + on/off switch; sanitized model-facing errors
- [x] Session v2 format; atomic tool-round undo/window groups
- [x] Review fixes P0+P1+P2 (see PROBLEM-WITH-SOLUTION.md)
- [x] Two-phase wiremock tool-loop test; CI gate green

## Done (workspace & extensibility)
- [x] `scan_project` overview; `.govindaignore`; symbol outlines in `read_file`
- [x] Async executor trait with concurrent tool execution
- [x] TOML-defined shell tools (argv templates) + per-call confirmation ⚠
- [x] Per-tool enable/disable (`/tools en|dis <name>`, persisted)
- [x] Staged editing workflow: `edit_file`/`insert_*` → `view_diff` → `/apply`
- [x] Execution tools: `run_shell`, `run_test`, `check_project`
- [x] Pipe mode (`govinda -q`), `/config save`, session todo list

## Done (code intelligence & planning)
- [x] In-memory symbol index built at startup (`/scan` refreshes)
- [x] `find_symbol` + passive `explain_code` tools
- [x] Context-aware windowing: mentioned files injected into the window
- [x] Agent system-prompt specialization
- [x] Self-correction loop (up to 3 extra fix rounds on failures)
- [x] `/plan <task>`: decompose → y/N confirm → autonomous execution
- [x] Git tools: `git_diff`, `git_log`, `git_branch`, `git_commit`

## In progress
- [ ] (nothing — tree is clean pending commit/push of docs)

## Next up (Phase 10)
- [ ] File attachments / local RAG
- [ ] Incremental symbol-index updates after `/apply`
- [ ] Smarter compaction preserving recent tool rounds

## Backlog (Phase 11+)
- [ ] Streaming markdown rendering
- [ ] Full-TUI mode
- [ ] Release packaging: cargo-dist, winget/Scoop/Homebrew
- [ ] SSE parser fuzzing; property tests for window/undo invariants
