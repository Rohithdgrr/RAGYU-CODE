# GOVINDA Enforcement Mechanism — Implementation Plan

## Goal
Wire the user's "GOVINDA Protocol v7.0" master system prompt, planning template, and self-verification loop into the existing Rust CLI so that **every** user prompt is treated as production-grade, and the model cannot short-circuit to a stub.

Two activation modes are needed:
1. **Always-on default** (config opt-in) — every prompt gets the protocol header appended.
2. **Explicit `/plan` slash command** — forces a planning cycle before any tool use.

## Current state (verified in code)
- `src/commands/mod.rs:1` — REPL slash-command dispatcher; `AGENT_SYSTEM_ADDENDUM` is appended in `specialize_system()` at line 884. This is the single injection point for system-prompt tweaks.
- `src/agent_loop.rs:132` — `run_turn()` is the per-turn pipeline. Round cap at line 21 is `MAX_TOOL_ROUNDS = 5`; fail-correction at line 23 is `MAX_FIX_ROUNDS = 3`. Per-round tool results are truncated at line 25 (`MAX_TOOL_RESULT_CHARS = 8192`).
- `src/tools.rs:54` — `BUILTIN_TOOL_NAMES` is the reserved-name list for user shell tools; any new built-in must be added here.
- `src/lib.rs:9` — module registry; new modules must be `pub mod`'d here.
- `src/commands/mod.rs:142` — `App::new_for_test()` is the canonical minimal `App` builder (used by tests and by tool executors that need `App`).
- No existing `SYSTEM_INSTRUCTIONS.md` or `PLAN_TEMPLATE.md` file exists at the repo root (verified via glob — only `SYSTEM_INSTRUCTIONS/` *folder* with sub-instruction documents).

## Architecture decision: where the protocol lives
- A new `govinda_protocol` module owns the master prompt text, the plan template text, and the phase enum. It is the single source of truth.
- A new built-in tool `quality_gate_check` enforces the self-verification loop (the model calls it itself; the host also re-checks results on its side).
- A new `/plan` slash command prepends the protocol header to the *next* turn only, and stores the resulting plan in the session todo list so the model can track phases.
- `App.enforcement_mode: bool` toggles always-on injection. Default: `false`. Config key: `enforce_protocol = true` in `config.toml`.

## Module: `src/govinda_protocol.rs` (NEW)

Public surface:
```rust
pub const MASTER_SYSTEM_PROMPT: &str;      // full v7.0 prompt
pub const PLAN_TEMPLATE: &str;              // PLAN_TEMPLATE.md body
pub const SYSTEM_INSTRUCTIONS_PATH: &str;   // "SYSTEM_INSTRUCTIONS/GENERAL_RULES&INSTRUCTIONS"
pub const PLAN_TEMPLATE_PATH: &str;         // "PLAN_TEMPLATE.md"

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectPhase {
    InstructionIngestion,
    ProjectAnalysis,
    ArchitectureRoadmap,
    DesignSystem,
    DevelopmentPlan,
    Implementation,
    SelfVerification,
    FinalValidation,
}

pub struct ProtocolConfig {
    pub enforcement_mode: bool,      // master switch
    pub min_line_count: usize,       // default 10_000
    pub max_turns: usize,            // default 50
    pub emoji_scan: bool,            // default true
    pub require_quality_gates: bool, // default true
}

impl Default for ProtocolConfig { /* reads from crate::config */ }

pub fn build_protocol_header(user_prompt: &str) -> String;       // header for next turn
pub fn detect_phase(text: &str) -> Option<ProjectPhase>;          // parses model output for "[Phase N]"
pub fn scan_emojis(text: &str) -> Vec<(usize, char)>;             // Unicode emoji ranges
pub fn quality_gate_payload(phase: ProjectPhase, files: &[String], line_count: usize) -> serde_json::Value;
```

Behavior:
- `build_protocol_header` returns the **header line** only (the "GOVINDA PROTOCOL ACTIVATED…" preamble plus the relevant instruction section pointers), so it can be prepended without blowing the context window. The full master prompt stays in the system message; the header is an additional reminder per turn.
- `scan_emojis` checks Unicode ranges `U+1F300..=U+1F9FF`, `U+2600..=U+27BF`, `U+1F000..=U+1F02F`, plus the variation selectors `U+FE0F` and ZWJ `U+200D`. Returns offending `(offset, char)` pairs.
- `detect_phase` regexes for `\[Phase (\d)\]` and `PHASE \d:` markers.

## Changes to `src/commands/mod.rs`

1. Add `pub mod govinda_protocol;` (re-exported via `lib.rs`).
2. New field on `App`:
   ```rust
   pub protocol_config: govinda_protocol::ProtocolConfig,
   pub pending_protocol_header: Option<String>,  // set by /plan
   pub current_phase: Option<govinda_protocol::ProjectPhase>,
   pub total_lines_emitted: usize,               // rough estimate from write_file payloads
   ```
3. In `specialize_system()` (line 884), if `protocol_config.enforcement_mode` is true, append the master prompt **plus** any loaded instruction files (`SYSTEM_INSTRUCTIONS/GENERAL_RULES&INSTRUCTIONS`, `WEBSITE_INSTRUCTIONS`, `CLI_TOOL_INSTRUCTIONS`, etc.) **plus** the plan template. Instruction files are read once at startup (cached on `App`).
4. In `dispatch()`, add a `/plan <prompt…>` handler:
   - Stores the prompt in `app.pending_protocol_header = Some(build_protocol_header(&prompt))`.
   - Resets `current_phase = Some(InstructionIngestion)`.
   - Returns `Outcome::Resend(prompt)` so the next turn runs through `run_turn` with the header prepended.
5. In `dispatch()` unknown-command fallback (line 472), detect `/plan` first (it was removed in the cleanup at line 438 — restore it).

## Changes to `src/agent_loop.rs`

1. After `app.session.push_user(input);` at line 139, prepend `app.pending_protocol_header.take()` to the user message instead of the raw input. Or — cleaner — append it to the system message *for this turn only* by extending `ChatOptions` with an optional `system_suffix: Option<String>`. **Recommended**: add a new `app.session.push_user_with_header(input, header)` method on `Session` that records the header as a separate system-style message kept in-window for that turn.
2. In the round loop, every 3rd round, after the model stream, call `govinda_protocol::quality_gate_payload(...)` and append a synthetic tool result to the session reminding the model to run `quality_gate_check` before claiming completion. This is the "self-correction pressure" the spec requires.
3. Detect premature completion: if the assistant text contains "That's it", "complete solution", or "fully implemented" AND `current_phase != FinalValidation`, inject a system-style reminder and continue the loop (already have the `fixes_granted` mechanism at line 243 — reuse it, bump `MAX_FIX_ROUNDS` from 3 to 6 when `enforcement_mode` is on).
4. Bump `MAX_TOOL_ROUNDS` from 5 to 50 when `enforcement_mode` is on (the spec calls for `max_turns = 50`).

## New built-in tool: `quality_gate_check`

In `src/tools.rs`:
1. Add `"quality_gate_check"` to `BUILTIN_TOOL_NAMES` (line 54).
2. Register the spec in `BuiltinTools::specs()` with the JSON schema from the spec (phase enum, files_delivered array, line_count_estimate integer, checks object with booleans).
3. Implement the executor branch in `BuiltinTools::execute()`. Pure local — no I/O, no confirmation, no shell.
4. On call:
   - Parse `checks` booleans. Any `false` returns a structured error string with the violation text.
   - For `phase == "FINAL"`, fail if `line_count_estimate < min_line_count` and instruct the model to expand.
   - Re-run `govinda_protocol::scan_emojis` on `files_delivered` (the executor reads them off disk, capped at 2MB per file via existing `MAX_INPUT_FILE_BYTES`) — any hits add a violation.
5. **Does not require confirmation** (read-only verification).

## Config: `src/config.rs`

Add fields to `Config`:
```rust
pub enforce_protocol: bool,          // default false
pub protocol_min_lines: usize,       // default 10_000
pub protocol_max_turns: usize,       // default 50
pub protocol_emoji_scan: bool,       // default true
pub protocol_require_gates: bool,    // default true
```

Deserialize from `[default]` or `[profile.<name>]` in `config.toml`. Add a CLI flag `--enforce` / `-E` in `src/main.rs` to flip it for one session.

## Files created
- `src/govinda_protocol.rs` — module
- `SYSTEM_INSTRUCTIONS.md` (repo root) — pointer file listing the per-project-type instruction files under `SYSTEM_INSTRUCTIONS/`, so the protocol loader can find them.
- `PLAN_TEMPLATE.md` (repo root) — verbatim from the spec.

## Files modified
- `src/lib.rs` — add `pub mod govinda_protocol;`
- `src/commands/mod.rs` — `App` fields, `/plan` handler, system-prompt injection
- `src/agent_loop.rs` — header prepend, quality-gate pressure, completion detection, round-cap bump
- `src/tools.rs` — `quality_gate_check` built-in
- `src/config.rs` — new `Config` fields
- `src/main.rs` — `--enforce` flag

## Validation plan
1. `cargo build` (with `--features` if any) and `cargo test` both pass.
2. New unit tests in `src/govinda_protocol.rs`:
   - `scan_emojis` flags a known emoji and ignores ASCII.
   - `detect_phase` parses both `[Phase 2]` and `PHASE 3:` markers.
   - `build_protocol_header` includes the user's prompt verbatim.
3. New test in `src/tools.rs` invoking `quality_gate_check` through `BuiltinTools::execute`:
   - `no_emojis: false` → returns violation.
   - `line_count_estimate: 500` with `phase: "FINAL"` → returns line-count violation.
   - All booleans `true` + `line_count: 15000` → returns `ALL QUALITY GATES PASSED`.
4. End-to-end smoke: a synthetic `App` with `enforce_protocol = true`, send `user_input = "build a simple todo list"`, assert `specialize_system()` contains the master prompt string.

## Open questions
- **Q1: Should the protocol be opt-in or on by default?**
  Recommendation: **opt-in via `[default] enforce_protocol = true` in `config.toml`**, with a `--enforce` flag as a runtime override. Default `false` to preserve existing users' workflows.
- **Q2: Where do the per-project-type instruction files live?**
  Recommendation: keep them under `SYSTEM_INSTRUCTIONS/` (the existing folder) and add `SYSTEM_INSTRUCTIONS.md` at the repo root as a manifest pointing at them. The protocol loader reads whichever one matches the project's detected type via `crate::project::detect_type()`.
- **Q3: Is the 10,000-line minimum enforced as a hard fail, or as a warning that can be overridden?**
  Recommendation: hard fail at the `FINAL` phase (the spec's own text says "If not, EXPAND"). Earlier phases only warn, since the model is mid-stream.
- **Q4: Where do `MASTER_SYSTEM_PROMPT` and `PLAN_TEMPLATE` live — as Rust string constants, or as files loaded at startup?**
  Recommendation: keep the master prompt as a Rust `const` (it's literally the system prompt the model sees) but make `PLAN_TEMPLATE` a runtime file at `PLAN_TEMPLATE.md` so the user can edit it without recompiling.

## Out of scope (explicitly)
- Implementing the actual production-grade output for any specific user prompt — the mechanism only enforces the protocol; it does not generate the 15,000-line todo app.
- Changing the existing TUI panels — the protocol runs in `run_turn`, which the TUI already drives.
- Cross-session persistence of `current_phase` — phase resets per session.
