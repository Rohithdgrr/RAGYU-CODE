# OmniRouter Hardening & Intelligence — Implementation Plan

## Decisions (resolved)

- **Auto-compact summarizer model:** cheapest healthy router entry
  (`/fast` → `/cheap` → active). The active model is only used if no
  router entry is healthy.
- **Auto-compact fallback path:** API-only. The existing
  `commands/generation.rs::compact` flow is reused verbatim; no local
  summarizer. If the summarizer call fails, auto-compact is a no-op for
  that turn and the user is notified.
- **3-strike failover scope:** per-session permanent. A model that
  accumulates 3 strikes is marked `quarantined` for the rest of the
  session; `/model <id>` re-enables it.
- **Pre-flight test scope:** active model only on startup (1–2 token
  ping, 8 s timeout). Failure surfaces a clear error and the user can
  `/model` before retrying.
- **Top-models ranking source:** static `KnownModel` registry + local
  router health log. No live `/v1/models` parser change.

## Current OmniRouter Flow (read, not changed)

`src/omniroute.rs` is a bootstrap/launcher only.

Call site: `src/main.rs:111-157`. When no provider is explicit and no
key is set, the app tries OpenCode first; on failure it calls
`omniroute::ensure_running(&http)`:

1. Probe `http://localhost:20128/v1/models` (PROBE_TIMEOUT 600 ms).
2. If down: check `omniroute --version`; if missing and npm exists, run
   `npm i -g omniroute --no-audit --no-fund --no-optional --prefer-offline`
   with 300 s timeout, streaming npm output.
3. Spawn `omniroute` detached (stdout/stderr to null, Windows
   `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP`); drop the child handle.
4. Poll every 1 s up to 60 s for the gateway to answer.

The router does not exist yet. `src/provider.rs:182-189` declares 6
hard-coded combo models (`auto`, `/coding`, `/fast`, `/cheap`,
`/offline`, `/smart`) all with context_window = 1_048_576. `api.rs::list_models`
only parses `id` (`api.rs:572-605`); no `context_window`, no
`capabilities`. `api.rs::stream_chat` retries with `MAX_RETRIES = 3`
*per HTTP request* (`api.rs:35, 428-451`), not per model. `agent_loop.rs::run_turn`
calls `api::stream_chat` once and returns on `Err`
(`agent_loop.rs:200-247`). `commands/mod.rs::compact` is the only
existing summarization path (`commands/generation.rs:140-187`).

## Bugs / Dead Code (confirmed by grep)

1. `auto_compact_enabled` and `last_auto_compact_count` in
   `commands/mod.rs:114-117, 202-203` are never read or updated.
   `/auto-compact` is listed in the dispatcher as removed
   (`commands/mod.rs:480`).
2. No 90% / 98% threshold check against the model's true
   `context_window_for(provider, model)`. Trimming is a single
   `session::window_with(context_tokens, ...)` pass
   (`agent_loop.rs:178-180`) with no budget guard.
3. No model failover after persistent failures — `api.rs` retries 3× per
   HTTP request, then gives up; `agent_loop.rs:238-247` returns
   `RoundLimit` with no model swap.
4. `omniroute` combo context_window is hard-coded to 1_048_576
   regardless of upstream; can silently exceed the real limit.
5. `spawn_server` swallows stderr (`omniroute.rs:152-153`) — boot
   failures are invisible to the user.
6. `list_models` is a thin id-only parser (`api.rs:572-605`); too thin
   for a real top-models view.
7. `/temp` is reported as removed in the dispatcher
   (`commands/mod.rs:460`) but `temperature` is still consumed from
   config and the per-provider env var still exists; the docs and the
   runtime are out of sync.

## Hardcoded Fields (audit)

The following string and numeric fields are baked into source and need
to become data so the router and registry can extend without code
changes:

### Provider identifiers (string literals)

| Location | Field | Suggested action |
|---|---|---|
| `config.rs:9` | `DEFAULT_PROVIDER = "omniroute"` | keep as a const; nothing to do |
| `config.rs:10` | `DEFAULT_MODEL = "auto"` | keep; references the combo model id |
| `provider.rs:78-143` | 12 `Preset { id, base_url, api_key_env }` rows | already data; the **role tags** are not — add a `roles: &'static [Role]` to `Preset` so the router can pick fallbacks without hard-coding "omniroute" |
| `provider.rs:182-189` | 6 omniroute combo rows, all `context_window: 1_048_576` | move per-combo window into a new `OmniRouteCombo { id, role, context_window, description }` table; add a free-tier cap-aware window (combo /offline is currently 32_768 — keep that, the rest are 1_048_576) |
| `opencode.rs:129-141` | 7 hardcoded `base_url` strings for `openai`, `openrouter`, `groq`, `deepseek`, `google`, `ollama` | consolidate into a single map keyed by provider id; both `provider.rs::PRESETS` and `opencode.rs` should reference the same map |
| `tui/theme.rs:261-287` | per-provider color RGBs | keep, but ensure every `id` in `PRESETS` has a color (currently `kimi` is missing in the primary map at `tui/theme.rs:262-281` — add it) |
| `tui/draw.rs:798` | help text `mistral · kimi · groq · ollama` | build from `provider::preset_names()` instead of a hardcoded list |
| `tui/app.rs:572` | `if chosen == "ollama"` | switch on `provider.requires_key()` to pick the next step generically |
| `commands/mod.rs:648` | `provider_id != "ollama"` | same — use `!provider.requires_key()` |
| `commands/mod.rs:1019, 1030, 1034, 1039, 1077, 144, 145, 545, 546, tui/app.rs:2495, 2561` | repeated `resolve("ollama", ...)` for tests | fine for tests, but extract a `test_provider() -> ResolvedProvider` helper to avoid drift if a new keyless preset is added |

### Numeric / timeout / capacity constants (no config knob today)

| Location | Field | Suggested action |
|---|---|---|
| `omniroute.rs:18-21` | `INSTALL_TIMEOUT=300s`, `BOOT_TIMEOUT=60s`, `POLL_INTERVAL=1s`, `PROBE_TIMEOUT=600ms` | promote to `Config` (TOML keys `omniroute_install_timeout_secs`, etc.) with current values as defaults |
| `omniroute.rs:26-28` | `CREATE_NO_WINDOW`, `CREATE_NEW_PROCESS_GROUP` | keep Windows-only; comment "intentionally not configurable" |
| `agent_loop.rs:21` | `MAX_TOOL_ROUNDS=5` | keep |
| `agent_loop.rs:28` | `TOOL_RESULT_DISPLAY_CHARS=200` | reuse as the new `compress_old_tool_results` excerpt size |
| `api.rs:28-29` | `MAX_SSE_LINE_BYTES=1MB`, `DEFAULT_READ_TIMEOUT=120s` | keep |
| `api.rs:35` | `MAX_RETRIES=3` (per HTTP request) | keep — orthogonal to the per-model 3-strike rule |
| `api.rs:36` | `RETRYABLE_STATUS: 5 codes` | keep |
| `api.rs:197, 199` | `MAX_PARALLEL_TOOL_CALLS=64`, `MAX_TOOL_ARGUMENTS_BYTES=256KB` | keep |
| `config.rs:12-15` | `DEFAULT_TEMPERATURE=0.7`, `DEFAULT_RENDER_MARKDOWN=true`, `DEFAULT_TIMEOUT_SECS=30`, `DEFAULT_LIMIT_MB=16` | keep |
| `provider.rs:12-13` | `MIN_CONTEXT_TOKENS=256`, `MAX_CONTEXT_TOKENS=1_000_000` | keep |
| `swarm.rs:152` | `temperature = 0.3` literal | pull from `Config` (config key `swarm_temperature`); same for `Config::DEFAULT_MODEL` if a swarm preset deserves its own |
| `tools.rs:25-49` | 14 hardcoded size/time caps | keep; these are the safety rails |
| `tokens.rs:7, 9` | `PER_MESSAGE_OVERHEAD=4`, `PER_TOOL_CALL_OVERHEAD=4` | keep |
| `tui/app.rs:37` | `ZERO_ARG_SLASH: [&str; 13]` | build from the command registry; the literal list drifts |
| `context.rs:19-27` | `MAX_FILES=6`, `MAX_SIBLINGS=2`, `SOURCE_EXTS`, `MANIFESTS` | keep |
| `outline.rs:13-14` | `MAX_SYMBOLS=150`, `MAX_IMPORTS=50` | keep |

### Role tags (new, replaces stringly-typed combo ids)

The router and the top-models view need to know which combo is
"fast" vs "cheap" vs "smart". Today that information is only in the
human-readable `description` field. Add a typed `RouterRole` enum and
attach it to both `OmniRouteCombo` rows and any `Preset` row that is
itself a combo gateway.

```rust
// src/provider.rs
pub enum RouterRole { Primary, Coding, Fast, Cheap, Smart, Offline, Generic }

pub struct OmniRouteCombo {
    pub id: &'static str,
    pub role: RouterRole,
    pub free: bool,
    pub description: &'static str,
    pub context_window: usize,
}

pub const OMNIROUTE_COMBOS: &[OmniRouteCombo] = &[
    OmniRouteCombo { id: "auto",     role: RouterRole::Smart,   free: true, description: "smart router across all connected providers", context_window: 1_048_576 },
    OmniRouteCombo { id: "/smart",   role: RouterRole::Smart,   free: true, description: "quality-optimized combo",                       context_window: 1_048_576 },
    OmniRouteCombo { id: "/coding",  role: RouterRole::Coding,  free: true, description: "coding-optimized combo",                        context_window: 1_048_576 },
    OmniRouteCombo { id: "/fast",    role: RouterRole::Fast,    free: true, description: "speed-optimized combo",                         context_window: 1_048_576 },
    OmniRouteCombo { id: "/cheap",   role: RouterRole::Cheap,   free: true, description: "cost-optimized combo",                          context_window: 1_048_576 },
    OmniRouteCombo { id: "/offline", role: RouterRole::Offline, free: true, description: "local-only combo",                              context_window: 32_768 },
];
```

The `Preset` struct gets an optional `role: Option<RouterRole>` so a
non-omniroute preset can also declare its role (e.g. `ollama` →
`Offline`).

`context_window_for("omniroute", combo_id)` is rewritten to consult
`OMNIROUTE_COMBOS` instead of the inline `&[KnownModel]` table for
combo rows. The `KnownModel` table becomes a pure registry for
non-combo models, removing the duplicated hard-coded `1_048_576`.

## Implementation Tasks (ordered, one per file/block)

### Task 1 — `src/provider.rs` — Add `RouterRole`, `OMNIROUTE_COMBOS`, roles on `Preset`

- Introduce `RouterRole` enum and `OMNIROUTE_COMBOS` table above.
- Add `pub role: Option<RouterRole>` to `Preset` and fill it for the
  presets that map cleanly (`omniroute: Some(Smart)`, `ollama:
  Some(Offline)`, others `None`).
- Refactor `known_models("omniroute")` to look up combos via
  `OMNIROUTE_COMBOS` so combo context windows stop being a duplicated
  literal.
- `context_window_for` is unchanged in signature; its body now walks
  the new combo table first.
- Tests: cover each role row, and confirm the previously-asserted
  values (`context_window_for("omniroute", "auto")` = 1_048_576,
  `context_window_for("omniroute", "/offline")` = 32_768) still pass.

### Task 2 — `src/router.rs` (new) — Router + strikes + failover

Public surface:

```rust
pub struct Router {
    entries: Vec<RouterEntry>,        // ordered primary + fallbacks
    health: HashMap<String, Health>,   // per-model strike counter
    quarantined: HashSet<String>,      // struck-out for the session
    failover_enabled: bool,            // toggled by /router failover
}

pub struct RouterEntry {
    pub model: String,
    pub role: RouterRole,
    pub context_window: usize,
}

#[derive(Default)]
pub struct Health {
    pub strikes: u8,
    pub last_latency_ms: u32,
    pub last_error: Option<String>,
}

pub const STRIKES_TO_QUARANTINE: u8 = 3;

impl Router {
    pub fn for_active(provider: &str, model: &str) -> Self;
    pub fn active(&self) -> &RouterEntry;
    pub fn next_summarizer(&self) -> &RouterEntry; // Fast > Cheap > active
    pub fn record_failure(&mut self, model: &str, kind: FailureKind, msg: &str);
    pub fn record_success(&mut self, model: &str, latency_ms: u32);
    pub fn promote(&mut self) -> Option<&RouterEntry>;
    pub fn quarantine(&mut self, model: &str);
    pub fn is_quarantined(&self, model: &str) -> bool;
    pub fn set_failover(&mut self, on: bool);
    pub fn iter_active(&self) -> impl Iterator<Item = &RouterEntry>;
}

pub enum FailureKind { Auth, RateLimit, Server, Timeout, BadModel, Empty, Other }
```

Behavior:
- `Router::for_active`: builds entries from the active model + the
  OmniRoute combos whose `RouterRole` is non-`Offline`, plus
  `Offline` last. For `provider == "omniroute"` the default order is
  `[active, /smart, /coding, /fast, /cheap, /offline]`. For other
  providers, fallbacks are `[active]` plus an "auto" entry when the
  provider is an OmniRoute-family combo gateway.
- `record_failure` increments `strikes`; if `strikes >= 3` and the model
  is not already quarantined, quarantine it and log via `tracing::warn!`.
- `promote` returns the next non-quarantined entry, or `None` if all
  are exhausted. The caller uses this to retry the same
  `api::stream_chat` call with the new model.
- `next_summarizer` is read-only: cheapest role wins
  (`Fast > Cheap > active`); does not consume strikes.
- `set_failover(false)` makes `promote` always return `None`.

### Task 3 — `src/preflight.rs` (new) — Active-model probe

```rust
pub struct ProbeResult {
    pub model: String,
    pub latency_ms: u32,
    pub status: ProbeStatus,
}
pub enum ProbeStatus { Ok, Warn(String), Err(String) }

pub async fn probe_active(
    http: &reqwest::Client,
    provider: &dyn Provider,
    model: &str,
) -> ProbeResult;
```

- Sends a `chat/completions` request with `max_tokens = 4`,
  `temperature = 0`, messages = single `user: "ping"`.
- 8 s total timeout (configurable later via `Config::probe_timeout`,
  default 8 s — this replaces the currently hard-coded
  `omniroute::PROBE_TIMEOUT` only for the model probe; the bootstrap
  probe keeps its 600 ms).
- Returns `Ok` on 200 + non-empty body, `Warn` on 200 with empty
  body, `Err` on any non-2xx or transport failure.
- One call only (active model). No pre-flight for fallbacks.

Wire into `main.rs`:
- After `Config::load` and before the `auto_connect` block, if the
  provider has a base URL and a model, run `probe_active` and on
  `Err` print the error and continue (do not abort — the user can
  `/model`). On `Ok` print latency to the existing dim-color line so
  the user sees that probing happened.
- Initialize `Router::for_active(provider, model)` and stash it on
  the `App` (or pass through the agent loop) for use by the auto-
  compact hook and the failure path.

### Task 4 — `src/auto_compact.rs` (new) — Threshold + soft/hard reset

```rust
pub fn check_and_run(
    app: &mut App,
    router: &Router,
) -> Outcome;
```

- Computes `used = app.session.approx_tokens()` and
  `window = context_window_for(provider, model)` (falling back to
  `app.config.context_tokens`).
- `pct = used * 100 / window`.
- `soft_pct = 90` (configurable via `app.session.soft_compact_pct`,
  default 90). `hard_pct = 98` (configurable, default 98). Default
  thresholds are `const SOFT_COMPACT_PCT: u8 = 90;` and
  `const HARD_COMPACT_PCT: u8 = 98;` in this file — previously the
  percent thresholds did not exist in source at all.
- Triggers:
  - `pct >= hard_pct` → hard reset.
  - `pct >= soft_pct` AND two consecutive soft compactions have not
    reduced `used` below `soft_pct - 5` → hard reset.
  - `pct >= soft_pct` (otherwise) → soft compact.
- **Hard reset path:** keep `system` + last 4 turns, drop everything
  else, push a system note `Earlier context was reset at <ts> to
  recover from overflow.`, set
  `last_compaction = HardReset`, `last_auto_compact_count =
  history.len()`. Persist a one-line entry to
  `.govinda/compaction.log`.
- **Soft compact path:** call the existing `generation::compact(app)`,
  but route the summarizer call through `Router::next_summarizer` (set
  `app.config.model` to the summarizer model for the duration of the
  call, then restore). If `next_summarizer` is itself quarantined, use
  the active model. The summarizer call uses `max_tokens = 512` and a
  30 s timeout.
- Respects `app.auto_compact_enabled`. The dead fields
  `auto_compact_enabled` and `last_auto_compact_count` now have one
  read site and two write sites each.

Wire into `agent_loop.rs::run_turn`:
- After `finish_answer` succeeds and before `record_turn`
  (`agent_loop.rs:232-237`), call `check_and_run`. A `Handled`
  outcome means compaction ran this turn; nothing else changes.

### Task 5 — `src/commands/router_cmd.rs` (new) — `/router status | failover | reset`

- `/router status` — print primary + fallbacks + quarantined list +
  per-model `strikes / last_latency_ms / last_error`.
- `/router failover off|on` — toggles `Router::set_failover`.
- `/router reset` — clears `quarantined` (keeps strike counters).
- Wire into `commands/mod.rs` dispatcher and TUI command list
  (`tui/app.rs:39, 681, 1138` — append `"/router"`).

### Task 6 — `src/model_rank.rs` (new) — Top-models view

```rust
pub struct RankedModel {
    pub provider: String,
    pub id: String,
    pub role: RouterRole,
    pub free: bool,
    pub context_window: usize,
    pub description: &'static str,
    pub score: f32,
    pub health: Option<&'static Health>,
}

pub fn top_models(
    provider: &str,
    sort: SortKey,
    n: usize,
    health: Option<&Router>,
) -> Vec<RankedModel>;
pub enum SortKey { Quality, Speed, Cost, Context, Free }
```

Sources:
- Static `provider::known_models(provider)` (now a pure registry
  after Task 1) for `id`, `free`, `context_window`, `description`,
  `role`.
- `Router` health log: average latency (Speed) and success rate
  (Quality) when present; otherwise the registry hint.
- Scoring weights:
  - Quality: `0.5 * success_rate + 0.3 * (1 - strikes/3) + 0.2 * context_norm`.
  - Speed: `1 / (1 + avg_latency_ms/1000)`.
  - Cost: `free ? 1.0 : 0.4`.
  - Context: `min(1, context_window / 1_000_000)`.
  - Free: `free ? 1.0 : 0.0`.

Wire into existing `/models`:
- Extend the `models` command (`commands/generation.rs:60-70`) with a
  `top` subcommand: `/models top [N] [--sort=quality|speed|cost|context]`.
- Default sort = `Quality`. Default N = 5. No live `/v1/models` call;
  registry + health only.

### Task 7 — Token-reduction techniques (in-place edits)

- `src/agent_loop.rs:178-180` — before `window_with`, run a new
  helper `compress_old_tool_results(history, keep_recent=3,
  excerpt_chars=200)` (the `200` matches the existing
  `TOOL_RESULT_DISPLAY_CHARS=200` constant on `agent_loop.rs:28`).
- `src/api.rs:415-426` — drop `temperature` from the body when
  `temperature == 1.0` (many providers default to 1.0 and treat
  custom values as a deviation). Don't send fields the model ignores.
- `src/api.rs` — add a `max_tokens` field to `ChatOptions` (default
  `min(2048, window/4)`). When `finish_reason == "length"` is observed
  twice in a row for the same `ChatOptions`, increase by 512.
- `src/prompt_cache.rs` (new) — small LRU (32 entries) keyed by
  `(model, sha256(system || last_4_turns))`. Used by `/variants` and
  `/retry` only; never in the main agent loop to avoid staleness.

### Task 8 — Observability and tests

- `src/router_health.rs` (new) — `append(entry: HealthEntry)` writes
  one line to `.govinda/router_health.jsonl` (JSONL). Cap the file
  at 1 MB; rotate.
- Extend `/stats` to print a router health summary (success rate,
  p50/p95 latency, quarantines).
- Unit tests:
  - `Router::record_failure` increments; 3rd strike quarantines;
    `promote` skips quarantined.
  - `preflight::probe_active` returns `Ok` for 200+body, `Err` for
    500, `Err` for timeout.
  - `auto_compact::check_and_run` at 89.9 % no-ops, 90.0 % soft,
    98.0 % hard, second soft at 92 % without recovery triggers hard.
  - `model_rank::top_models` returns sorted output for each
    `SortKey`.
  - `compress_old_tool_results` keeps the last 3 untouched,
    truncates older.
  - `provider::known_models("omniroute")` returns the same id list
    as before (combo ids), with the right role tag and context
    window.
- Integration test in `tests/` (wiremock): three responses in order
  (200, 500, 500) → `Router::record_failure` x2 quarantines → next
  call uses fallback model. 200 from fallback → success.

### Task 9 — Small fixes to OmniRoute bootstrap

- `src/omniroute.rs:152-153` — redirect `stderr` to a tempfile
  `std::env::temp_dir().join("omniroute-<pid>.log")` and on
  boot-timeout print the tail of that file in the error message.
- `src/main.rs:111-157` — second-chance probe: if
  `ensure_running` returns `Ok(false)`, schedule a single background
  retry 10 s later that re-runs `ensure_running` and updates a
  `pending_reconnect: bool` on the app. The first chat after a
  successful reconnect prints a dim "OmniRoute reconnected" line.

### Task 10 — Drift fixes for already-removed commands / providers

- `commands/mod.rs:460, 1116` — `/temp` is reported as removed but
  `Config.temperature` is still the active value; add a clear
  dispatcher branch that prints the help pointer instead of the
  removed notice, and remove the `"/temp 0.3"` example from the
  help text.
- `tui/app.rs:39, 681, 1138` — `ZERO_ARG_SLASH` and the
  inline command list both duplicate the dispatcher table. Build
  them from a single `ZERO_ARG_COMMANDS: &[&str]` constant in
  `commands/mod.rs` and import in both call sites.
- `tui/draw.rs:798` — replace the hardcoded help footer
  `mistral · kimi · groq · ollama` with
  `provider::preset_names().take(4).collect::<Vec<_>>().join(" · ")`.
- `tui/theme.rs:262-281` — add a color for `kimi` (missing on the
  primary map).

## Affected Files (summary)

| File | Action |
|---|---|
| `src/omniroute.rs` | edit (Task 9) |
| `src/main.rs` | edit (Task 3 wire, Task 9) |
| `src/provider.rs` | edit (Task 1, foundation for Tasks 2 and 6) |
| `src/opencode.rs` | edit (consolidate base-url map referenced in audit) |
| `src/router.rs` | new (Task 2) |
| `src/preflight.rs` | new (Task 3) |
| `src/auto_compact.rs` | new (Task 4) |
| `src/commands/router_cmd.rs` | new (Task 5) |
| `src/model_rank.rs` | new (Task 6) |
| `src/prompt_cache.rs` | new (Task 7) |
| `src/router_health.rs` | new (Task 8) |
| `src/agent_loop.rs` | edit (Task 4 hook, Task 7 compression) |
| `src/api.rs` | edit (Task 7) |
| `src/commands/generation.rs` | edit (Task 6 wire into `/models top`) |
| `src/commands/mod.rs` | edit (Tasks 4, 5, 10) |
| `src/swarm.rs` | edit (Task — pull `0.3` literal into a `Config` knob) |
| `src/tui/app.rs` | edit (Task 5, Task 10) |
| `src/tui/draw.rs` | edit (Task 10) |
| `src/tui/theme.rs` | edit (Task 10) |
| `tests/router_tests.rs` | new (Task 8) |

## Data Flow

```
main.rs start
  └─ Config::load
  └─ preflight::probe_active ──────► dim "active model probed in N ms"
  └─ opencode::auto_connect (existing)
  └─ omniroute::ensure_running (existing) ─► temp log on failure (Task 9)
  └─ Router::for_active(provider, model)        ◄── uses OMNIROUTE_COMBOS
  └─ run REPL/TUI

turn
  └─ run_turn (agent_loop)
       └─ loop:
            ├─ window_with + compress_old_tool_results
            ├─ stream_chat (api)
            ├─ on Err: router.record_failure(model, kind, msg);
            │           if !quarantined and router.failover_enabled
            │              → router.promote → set model, retry once
            ├─ on Ok: finish_answer
            └─ after Answered: auto_compact::check_and_run
                  └─ soft  → generation::compact via router.next_summarizer
                  └─ hard  → keep system + last 4, log to .govinda/compaction.log
            └─ router_health::append(entry)
```

## Failure Modes (explicit)

- **Probe fails:** print error, continue to REPL; first chat will
  re-surface the same error and exit cleanly.
- **Summarizer call fails:** auto-compact is a no-op for that turn;
  emit a `notice` and bump a `soft_compact_streak` counter. The next
  turn that crosses 98 % triggers hard reset.
- **All fallbacks quarantined:** `Router::promote` returns `None`; the
  agent loop prints "all models quarantined — run `/router reset` to
  re-enable" and exits the turn cleanly.
- **Compaction log write fails:** swallow + warn; the history reset
  still happens in memory.
- **Failover disabled:** `Router::promote` always returns `None`; the
  agent loop surfaces the original error so the user can `/model`.

## Validation Plan

- `cargo test` — all new unit + integration tests pass.
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo fmt --check` — clean.
- Manual smoke:
  1. With OmniRoute up: a 100-turn session, observe auto-compact kicks
     in at 90 % and reset at 98 %.
  2. Block the active upstream: 3 consecutive errors → model swap →
     next call succeeds via fallback.
  3. Pre-flight: rename the model id to a non-existent one; observe
     the pre-flight error and that `/model <good>` recovers without
     restart.
  4. `/models top 5 --sort=context` lists models in context-window
     order with role tags.
  5. `/router failover off` then 3 failures → no model swap, error
     surfaced; `/router reset` then `/router failover on` restores
     promotion.
  6. Kill the network mid-session: graceful "router offline" message,
     no panic; recovery on reconnect prints the dim line.

## Out of Scope (explicit)

- Changing the OmniRoute npm package itself.
- New presets (existing registry already covers current upstreams).
- Cross-session memory beyond `.govinda/router_health.jsonl` and
  `.govinda/compaction.log`.
- Server-side rate-limit coordination.
- A live `/v1/models` parser with context_window/capabilities fields
  (the top-models view uses the static registry + health log per the
  resolved decision).
- Local (non-API) summarizer (per the resolved decision).
- Auto-reconnect of quarantined models within a session (per the
  resolved decision).
- New TUI widgets or panels (per the user's refinement).
- New slash commands beyond `/router status|off|on|reset` and
  `/models top` (per the user's refinement).
