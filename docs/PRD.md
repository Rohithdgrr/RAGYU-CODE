# Product Requirements Document — govinda-cli

## 1. Vision
A fast, trustworthy, single-binary terminal chatbot that works with **any**
OpenAI-compatible backend, keeps conversations safe on disk, and lets the
model act through locally executed tools without giving up user control.

## 2. Problem statement
Existing CLI chatbots are either locked to one vendor, lose history between
runs, render output poorly in terminals, or execute model-requested actions
with no guardrails. Developers want a hackable, dependency-light client they
can read the entire source of.

## 3. Goals / non-goals

**Goals**
- G1: Stream answers from any OpenAI-compatible API with correct UTF-8 handling.
- G2: Multi-turn memory bounded by a real tokenizer budget, persisted across runs.
- G3: Function calling with local execution, parallel calls, and hard resource caps.
- G4: Never lose generated content: partial answers survive Ctrl+C/errors.
- G5: Secrets never touch config files, logs, or session JSON.
- G6: Fully testable core (library crate + wiremock), CI-enforced quality gates.

**Non-goals (for v1)**
- Full-TUI interface, image/file attachments, RAG
- Non-OpenAI wire protocols (Anthropic native, Gemini)
- Multi-user/server deployment

## 4. Personas
- **Developer Dana** — wants a scriptable REPL against local Ollama models.
- **Privacy-first Pia** — refuses cloud; needs local-only operation and plaintext transparency.
- **Tinkerer Theo** — reads the source, adds custom tools via a small trait.

## 5. Functional requirements

| ID | Requirement | Status |
|---|---|---|
| FR-1 | Streamed completions with markdown or raw live rendering | ✅ |
| FR-2 | Multi-provider presets + `base_url` override | ✅ |
| FR-3 | Tokenizer-trimmed context window aligned to user turns | ✅ |
| FR-4 | Named sessions: save/load/resume/list/fork/autosave/export | ✅ |
| FR-5 | Slash-command surface (35 commands incl. runtime tuning) | ✅ |
| FR-6 | Function calling: advertise tools, reassemble streamed calls, execute locally, loop ≤5 rounds (+3 self-correction), commit results atomically | ✅ |
| FR-7 | `/tools` registry view + master on/off switch + per-tool toggles | ✅ |
| FR-8 | Retry with backoff honoring `Retry-After`; no duplicated output after partial emission | ✅ |
| FR-9 | Caps: response size (`/limit`), SSE line, parallel calls (64), arguments (256 KB), stored results (8 K chars) | ✅ |
| FR-10 | Undo/retry semantics that never split tool rounds | ✅ |
| FR-11 | Concurrent answer variants with pick-to-commit | ✅ |
| FR-12 | History compaction via summary turn | ✅ |
| FR-13 | Workspace tools: sandboxed read/write/list/grep, staged surgical edits with diff review and atomic apply | ✅ |
| FR-14 | Symbol index (`/scan`) with `find_symbol` / `explain_code` lookups | ✅ |
| FR-15 | Context-aware windowing: prompt-mentioned files injected into the window | ✅ |
| FR-16 | Execution tools: `run_shell`, `run_test`, `check_project` with confirmation gates | ✅ |
| FR-17 | Git tools: `git_diff`, `git_log`, `git_branch`, `git_commit` (mutations gated) | ✅ |
| FR-18 | `/plan <task>`: decompose → confirm → autonomous step execution tracked in `/todo` | ✅ |

## 6. Non-functional requirements
- NFR-1 Performance: first token rendered as soon as it arrives; tool specs
  built once per session; startup < 1 s release build.
- NFR-2 Security: rustls TLS validation always on; keys only from env,
  zeroized; path-traversal-proof session names; sanitized model-facing errors.
- NFR-3 Reliability: byte-safe SSE under arbitrary chunk splits; graceful
  degradation when tokenizer vocab fails to load.
- NFR-4 Quality: clippy `-D warnings`, fmt check, 60+ tests including two-phase
  wiremock protocol tests — enforced by CI on every push/PR.

## 7. Success metrics
- Zero known data-loss paths for interrupted turns (kept text marked).
- All review findings P0/P1/P2 closed (see PROBLEM-WITH-SOLUTION.md).
- CI green on main at all times.

## 8. Release criteria (v0.1)
All FR/NFR above met; docs complete (this set); pushed to GitHub with CI badge workflow active.
