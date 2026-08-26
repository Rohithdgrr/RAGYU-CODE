# GOVINDA SYSTEM_INSTRUCTIONS

This file is the single entry point the GOVINDA Protocol enforcement
mechanism reads when `enforce_protocol = true` is set in
`~/.config/govinda/config.toml` (or `--enforce` is passed on the
command line).

## What the CLI does with this file

When protocol enforcement is on, `specialize_system()` in
`src/commands/mod.rs` appends three blocks to the system message that
the model sees on every request:

1. **GOVINDA PROTOCOL (ENFORCED)** — the inlined master system prompt
   (`src/govinda_protocol::MASTER_SYSTEM_PROMPT`). Self-contained, so a
   missing file never silently disables enforcement.
2. **ACTIVE PLAN TEMPLATE** — the body of `PLAN_TEMPLATE.md` at the
   repo root, falling back to a built-in stub when missing.
3. **ACTIVE SYSTEM INSTRUCTIONS** — this file's body, when present.

If you want to inject your own per-project rules without recompiling
GOVINDA, edit this file. The protocol reads it on every session
startup; no restart or recompile is required.

## Per-project-type instruction documents

The repo also has a `SYSTEM_INSTRUCTIONS/` directory that contains
the per-project-type rule sets. They are not auto-injected; point
the protocol at them by adding a `## Per-project rules` section to
this file, like so:

```markdown
## Per-project rules

The project is a web app. Apply these additional rules:
$(cat SYSTEM_INSTRUCTIONS/WEB-DEVELOPMENT-INSTRUCTIONS.MD)
```

(Or just paste the contents of the relevant document into this file.)

The available rule sets are:

- `SYSTEM_INSTRUCTIONS/GENERAL_RULES&INSTRUCTIONS` — universal baseline
- `SYSTEM_INSTRUCTIONS/WEB-DEVELOPMENT-INSTRUCTIONS.MD` — websites / web apps
- `SYSTEM_INSTRUCTIONS/MOBILE-DEVELOPMENT-INSTRUCITONS.MD` — iOS / Android
- `SYSTEM_INSTRUCTIONS/DESKTOP-APPLICATION-INSTRUCTIONS.MD` — Tauri / Electron
- `SYSTEM_INSTRUCTIONS/BOTS-EXTENSION-API_SERVICE-CLI_TOOL-RULES&INSTRUCTIONS.MD` —
  bots, browser extensions, API services, CLI tools

## How the protocol tracks progress

The model is told to emit `[Phase N]` markers in every response. The
agent loop parses these with `govinda_protocol::detect_phase` and
stores the most recent phase on `App::current_phase`. When the model
claims completion ("That's it!", "Here's the complete solution", etc.)
before reaching `FINAL_VALIDATION`, the agent loop injects a synthetic
user message asking the model to keep going and grants a self-correction
round. This is the only way the protocol enforces the "no early
termination" rule at the host level — everything else is the master
prompt steering the model.

## What to put in this file

- Project-specific quality bars (LCP targets, test coverage minimums,
  etc.) the master prompt doesn't know about.
- Hard rules that must override the master prompt (e.g. "this org
  forbids using emojis anywhere, even in comments").
- Links to external design systems, style guides, or compliance
  documents the model should respect.
- A short list of pre-approved third-party services (auth, payments,
  email) so the model doesn't propose alternatives that have to be
  re-justified.

Keep it short — every token in this file is a token the model will
re-read on every turn. Aim for fewer than 5,000 tokens.
