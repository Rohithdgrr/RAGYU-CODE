//! Unified agent turn loop shared by every frontend.
//!
//! The REPL, the TUI, `-q` one-shot mode and `--build` pipelines all run the
//! SAME pipeline here; frontends only adapt presentation and confirmation:
//!
//! - streaming deltas land via [`AgentUi::stream_delta`]
//! - tool rounds execute concurrently (`FuturesUnordered`) like the REPL
//! - a failed round grants self-correction rounds (up to [`MAX_FIX_ROUNDS`])
//! - gated tools follow the caller's [`GatePolicy`] (ask / auto-run / decline)
//! - errors salvage partial answers or roll back the trailing user prompt
//! - staged-edit diffs stream to the UI and focus-file breadcrumbs update

use std::time::{Duration, Instant};

use futures_util::StreamExt;

use crate::api::{self, ChatOptions};
use crate::commands::App;

/// Upper bound on model↔tool round trips per user turn.
pub const MAX_TOOL_ROUNDS: usize = 5;
/// Extra rounds granted after a failed tool round (self-correction loop).
pub const MAX_FIX_ROUNDS: usize = 3;
/// Upper bound when the GOVINDA protocol is enforcing. The master prompt
/// demands 10k+ lines of production output, so the default 5-round cap
/// would strangle the model — bump it to 50.
pub const MAX_TOOL_ROUNDS_ENFORCED: usize = 50;
/// Extra fix rounds granted when the protocol is enforcing. The default
/// 3 isn't enough when every file needs tests, docs, and a design-system
/// audit.
pub const MAX_FIX_ROUNDS_ENFORCED: usize = 6;
/// Cap applied *before* a tool result enters the session history.
pub const MAX_TOOL_RESULT_CHARS: usize = 8 * 1024;
/// Characters of a tool result shown on screen.
const TOOL_RESULT_DISPLAY_CHARS: usize = 200;

// Old tool-result compression is implemented in `session.rs` as
// `Session::messages_compressed` (and the free function
// `compress_old_tool_results`); the agent loop calls that helper
// before `window_with` to keep the prompt lean.

/// How confirmation-gated tools are handled during a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatePolicy {
    /// Ask via [`AgentUi::confirm`] (+ batch offer for multiple gates).
    Interactive,
    /// Run everything without asking (`--build`, TUI auto-run).
    AutoRun,
    /// Auto-decline gated calls (`-q` one-shot with nobody watching).
    DeclineAll,
}

/// Answer to one interactive confirmation gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    Approved,
    ApprovedAll,
    Declined,
}

/// Frontend adapter for the shared turn loop. All methods take `&self`;
/// implement interior mutability where needed (senders, `RefCell`s, stdout).
pub trait AgentUi {
    /// Whether streaming deltas should surface live as they arrive.
    fn raw_stream(&self) -> bool {
        false
    }

    /// One streamed delta (only called when [`AgentUi::raw_stream`] is true).
    fn stream_delta(&self, _delta: &str) {}

    /// Prose the model streamed before requesting tool calls.
    fn prose(&self, _text: &str) {}

    /// Final text answer closing the turn.
    fn answer(&self, _text: &str) {}

    /// A tool call is about to execute.
    fn tool_start(&self, _name: &str, _args: &str) {}

    /// A tool call settled; `snippet` carries a short result preview.
    fn tool_end(&self, _name: &str, _args: &str, _ok: bool, _snippet: &str) {}

    /// A unified diff produced by a staged edit.
    fn diff(&self, _diff: &str) {}

    /// A file worth showing as the prompt breadcrumb focus.
    fn focus_file(&self, _path: &str) {}

    /// Informational line.
    fn notice(&self, _text: &str) {}

    /// Error line.
    fn error(&self, _text: &str) {}

    /// Footer after a completed answer (model, elapsed).
    fn timeline(&self, _model: &str, _elapsed: Duration) {}

    /// Interactive gate for one workspace-mutating call. Only called under
    /// [`GatePolicy::Interactive`]. When later gated calls follow this one,
    /// `allow_all` lets the user approve the rest in one stroke.
    fn confirm(&self, _name: &str, _args: &str, _allow_all: bool) -> Confirm {
        Confirm::Declined
    }

    /// Batch offer covering every gated call in the round; `true` approves
    /// all of them without further prompts.
    fn confirm_batch(&self, _gated_count: usize) -> bool {
        false
    }

    /// Resolves when the user aborts the running turn; the default never
    /// resolves. Polled alongside the stream request.
    fn cancel_wait<'a>(&'a self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>> {
        Box::pin(std::future::pending())
    }

    /// When true, hard failures propagate as `Err` instead of being printed
    /// locally (`-q` exits non-zero on any failure).
    fn fail_fast(&self) -> bool {
        false
    }
}

/// Outcome of a full turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnResult {
    /// The model produced a final text answer.
    Answered,
    /// Stopped at the round/fix limit without a final answer.
    RoundLimit,
}

/// Explicit state machine for the agent turn loop. The original
/// procedural `loop { stream → tools → continue }` is now tracked via
/// this enum so observers, tests, and future UI can reason about the
/// current phase without parsing logs. States map 1:1 to the loop's
/// branches: streaming deltas, executing tool calls, self-correcting
/// after a failure, compacting history, terminal completed/cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnState {
    /// Waiting for streamed deltas from the provider.
    Streaming,
    /// Running the tool round that the last stream requested.
    ExecutingTools,
    /// Granted an extra round after a failed tool result (self-correction).
    SelfCorrecting,
    /// Auto-compact triggered (history summarized or hard reset).
    Compacting,
    /// Turn finished with a final answer.
    Completed,
    /// Turn was cancelled via `AgentUi::cancel_wait()` or Ctrl+C.
    Cancelled,
    /// Hit the round/fix limit without a final answer.
    RoundLimited,
}

impl TurnState {
    pub fn as_str(self) -> &'static str {
        match self {
            TurnState::Streaming => "streaming",
            TurnState::ExecutingTools => "executing_tools",
            TurnState::SelfCorrecting => "self_correcting",
            TurnState::Compacting => "compacting",
            TurnState::Completed => "completed",
            TurnState::Cancelled => "cancelled",
            TurnState::RoundLimited => "round_limited",
        }
    }
}

/// Runs one full agent turn: stream → optional concurrent tool rounds with
/// self-correction → final answer.
///
/// Session mutation happens here; presentation flows through `ui`. Returns
/// `Err` only when `ui.fail_fast()` is set (one-shot mode); otherwise errors
/// are salvaged locally and reported through [`AgentUi::error`].
#[allow(unused_assignments)]
pub async fn run_turn(
    app: &mut App,
    ui: &dyn AgentUi,
    gate: GatePolicy,
    input: &str,
) -> anyhow::Result<TurnResult> {
    let started = Instant::now();
    // GOVINDA protocol: prepend any pending per-turn header (set by
    // /plan) to the user message so the model sees the reminder even
    // when enforcement_mode is off. The header is consumed once.
    let effective_input;
    let input_ref: &str = if let Some(header) = app.pending_protocol_header.take() {
        effective_input = format!("{header}\n\n---\n\n{input}");
        &effective_input
    } else {
        input
    };
    app.session.push_user(input_ref);
    app.last_turn_had_failure = false;
    app.opencode_fallback_attempted = false;

    // Protocol-driven round caps: when enforcement is on, allow the model
    // enough rounds to actually reach the 10k-line target the master
    // prompt requires. Without this, the loop would short-circuit long
    // before the model could finish.
    let protocol_on = app.config.protocol.enforcement_mode;
    let max_rounds = if protocol_on {
        MAX_TOOL_ROUNDS_ENFORCED
    } else {
        MAX_TOOL_ROUNDS
    };
    let max_fixes = if protocol_on {
        MAX_FIX_ROUNDS_ENFORCED
    } else {
        MAX_FIX_ROUNDS
    };

    // Persistent per-session router: sync to the current active
    // provider/model, preserving strike counters and quarantine set
    // across turns. 3 strikes on the active model promote to the
    // next non-quarantined entry.
    {
        let provider_key = app.config.provider.key().to_string();
        let model = app.config.model.clone();
        app.router.sync_active(&provider_key, &model);
        app.router.set_failover(app.router_failover);
    }
    let mut failover_attempts: u8 = 0;

    // Context-aware windowing: files the prompt mentions ride along even if
    // they only appeared in old messages; the first becomes the breadcrumb.
    {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        if let Some(first) = crate::context::mentioned_files(input, &cwd).first() {
            let rel = first
                .strip_prefix(&cwd)
                .unwrap_or(first)
                .to_string_lossy()
                .replace('\\', "/");
            app.focus_file = Some(rel.clone());
            ui.focus_file(&rel);
        }
    }
    let injection = {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let files = crate::context::relevant_files(input, &cwd);
        crate::context::build_injection(&files, &cwd)
    };

    // Self-correction budget: rounds are capped at MAX_TOOL_ROUNDS, but a
    // failed tool round grants extra turns so the model can react.
    let mut fixes_granted = 0usize;
    let mut round_no = 0usize;
    // State machine tracking — the original procedural loop now has an
    // explicit `TurnState` so the current phase is inspectable.
    let mut turn_state = TurnState::Streaming;

    // Most recent chat error from a streamed round. When the round
    // eventually returns `Ok(())` with an empty body (e.g. the model
    // returned zero deltas, or every retry returned 200 with an empty
    // `choices[0]`) the user used to see only the generic
    // "(empty response)" placeholder. We now surface the last error
    // so the real reason is not hidden.
    let mut last_round_error: Option<String> = None;
    loop {
        // Round counter is bumped below, only when the model actually
        // produced output (non-empty response or tool calls). Failover
        // retries and self-correction continues do NOT count as rounds
        // because the model never produced a usable turn in those cases.
        if round_no > max_rounds + fixes_granted {
            turn_state = TurnState::RoundLimited;
            ui.notice(&format!(
                "stopped after {} tool rounds{} — ask again to continue. [state={}]",
                max_rounds + fixes_granted,
                if fixes_granted > 0 {
                    format!(" (+{fixes_granted} self-correction)")
                } else {
                    String::new()
                },
                turn_state.as_str()
            ));
            return Ok(TurnResult::RoundLimit);
        }

        let history = {
            let compressed = app.session.messages_compressed();
            app.session.window_with_messages(
                &compressed,
                app.config.context_tokens,
                injection.as_deref(),
            )
        };
        let auth = app.config.provider.auth();
        let provider_key_for_tokens = app.config.provider.key().to_string();
        let model_for_tokens = app.config.model.clone();
        let max_out = crate::provider::max_output_for(&provider_key_for_tokens, &model_for_tokens);
        let opts = ChatOptions {
            max_response_bytes: app.max_response_bytes,
            read_timeout: app.read_timeout,
            max_tokens: Some(max_out as u32),
            tools: if app.tools_enabled {
                app.tool_specs
                    .iter()
                    .filter(|t| !app.disabled_tools.contains(&t.name))
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            },
            ..ChatOptions::new(
                auth.token(),
                app.config.model.as_str(),
                app.config.temperature,
            )
        };

        // Everything the session held before this stream attempt; an error
        // with nothing emitted rolls back to exactly this state.
        let resume_len = app.session.messages().len();
        let mut out = String::new();
        let mut tool_calls = Vec::new();
        let result = {
            let http = &app.http;
            let provider = app.config.provider.clone();
            let mut sink = api::StreamSink::new(&mut out, &mut tool_calls);
            let cancel = ui.cancel_wait();
            tokio::select! {
                res = api::stream_chat(http, provider.as_ref(), &opts, &history, &mut sink, |delta| {
                    if ui.raw_stream() {
                        ui.stream_delta(delta);
                    }
                }) => res,
                _ = cancel => Err(anyhow::anyhow!("interrupted")),
            }
        };

        match result {
            Ok(()) if !tool_calls.is_empty() && app.tools_enabled => {
                round_no += 1;
                turn_state = TurnState::ExecutingTools;
                show_prose(ui, &out);
                // GOVINDA protocol: track the current phase from the
                // model's [Phase N] markers so we can spot the assistant
                // prematurely claiming completion.
                if let Some(phase) = crate::govinda_protocol::detect_phase(&out) {
                    app.current_phase = Some(phase);
                }
                let had_failure = run_tool_round(app, ui, gate, &out, &tool_calls).await;
                if had_failure {
                    app.last_turn_had_failure = true;
                }
                if had_failure && fixes_granted < max_fixes {
                    fixes_granted += 1;
                    turn_state = TurnState::SelfCorrecting;
                    ui.notice(&format!(
                        "↻ failure detected — granting self-correction round ({fixes_granted}/{max_fixes} max) [state={}]",
                        turn_state.as_str()
                    ));
                }
                // Back to streaming for the next round.
                turn_state = TurnState::Streaming;
                continue; // stream again so the model sees the results
            }
            Ok(()) => {
                // Defense-in-depth: api.rs now treats empty 200s as
                // Retryable errors, but older gateways or future code paths
                // may still surface Ok(()) with empty content. Treat it as a
                // retryable Empty failure rather than a silent success so the
                // user never sees bare "(empty response)" without diagnostics.
                if out.trim().is_empty() {
                    let empty_err = anyhow::anyhow!(
                        "empty response from model — gateway returned no content or tool calls (model may be quota-exhausted or mis-routed)"
                    );
                    let msg = format!("{empty_err:#}");
                    last_round_error = Some(msg.clone());
                    app.record_error();
                    let active = app.config.model.clone();
                    let kind = crate::router::FailureKind::Empty;
                    app.router.record_failure(&active, kind, &msg);
                    crate::router_health::append(&crate::router_health::HealthEntry {
                        ts: chrono::Utc::now().to_rfc3339(),
                        model: active.clone(),
                        latency_ms: started.elapsed().as_millis() as u32,
                        success: false,
                        error: Some(msg.clone()),
                    });
                    let should_promote = app.router.should_promote(&active, kind);
                    if !should_promote {
                        let strikes = app.router.health(&active).map(|h| h.strikes).unwrap_or(1) as u64;
                        let wait = Duration::from_millis(700 * strikes);
                        ui.notice(&format!(
                            "router: empty response strike {}/{} on {} — backing off {:.1}s before retry",
                            strikes,
                            crate::router::STRIKES_TO_QUARANTINE,
                            active,
                            wait.as_secs_f32()
                        ));
                        tokio::time::sleep(wait).await;
                        app.session
                            .truncate_messages(resume_len.min(app.session.messages().len()));
                        turn_state = TurnState::Streaming;
                        continue;
                    }
                    let max_failovers = app.router.iter().count().saturating_sub(1).max(5) as u8;
                    if failover_attempts < max_failovers {
                        let http_clone = app.http.clone();
                        let mut promoted: Option<String> = None;
                        let mut attempts_probe = 0u8;
                        let candidates = app.router.iter().count();
                        for _ in 0..candidates {
                            let candidate = {
                                let r = &mut app.router;
                                r.promote().map(|e| e.model.clone())
                            };
                            let Some(cand) = candidate else { break };
                            attempts_probe += 1;
                            let probe = crate::preflight::probe_active(
                                &http_clone,
                                app.config.provider.as_ref(),
                                &cand,
                            )
                            .await;
                            match probe.status {
                                crate::preflight::ProbeStatus::Ok
                                | crate::preflight::ProbeStatus::Warn(_) => {
                                    promoted = Some(cand);
                                    break;
                                }
                                crate::preflight::ProbeStatus::Err(ref reason) => {
                                    ui.notice(&format!(
                                        "router: candidate {cand} preflight failed ({reason}) — trying next"
                                    ));
                                    app.router.record_failure(
                                        &cand,
                                        crate::router::FailureKind::Server,
                                        reason,
                                    );
                                    continue;
                                }
                            }
                        }
                        if let Some(next) = promoted {
                            failover_attempts = failover_attempts.saturating_add(attempts_probe);
                            let active_clone = active.clone();
                            app.config.set_model(next.clone());
                            app.router.sync_active(&app.config.provider.key().to_string(), &app.config.model);
                            ui.notice(&format!(
                                "router: failover to {} after empty response on {} ({}/{} attempts)",
                                next, active_clone, failover_attempts, max_failovers
                            ));
                            app.session
                                .truncate_messages(resume_len.min(app.session.messages().len()));
                            turn_state = TurnState::Streaming;
                            continue;
                        } else if attempts_probe > 0 {
                            failover_attempts = failover_attempts.saturating_add(attempts_probe);
                        }
                    }
                    // No failover available — try OpenCode as last resort.
                    if let Some(next) = try_opencode_fallback(app, &app.http.clone()).await {
                        failover_attempts += 1;
                        let active_clone = active.clone();
                        app.config.set_model(next.clone());
                        ui.notice(&format!(
                            "router: failover to opencode:{} after empty response on {} ({} failovers tried)",
                            next, active_clone, failover_attempts
                        ));
                        app.session
                            .truncate_messages(resume_len.min(app.session.messages().len()));
                        turn_state = TurnState::Streaming;
                        continue;
                    }
                    // No failover available — surface the empty with diagnostics.
                    finish_answer(app, ui, out, last_round_error.as_deref());
                    // Emit a structured error card with suggestion so TUI/CLI
                    // users see an actionable fix instead of a silent notice.
                    ui.error(&format!(
                        "empty response from {} — all fallback models exhausted ({} failovers tried)\nModel: {} · Provider: {}\nSuggestion: run /models to list live models, then /model <id> to switch (e.g. /model auto/smart or /model auto/coding)",
                        active,
                        failover_attempts,
                        active,
                        app.config.provider.key()
                    ));
                    ui.timeline(app.config.model.as_str(), started.elapsed());
                    // Do NOT record success for empty — count as RoundLimit so
                    // callers can retry and metrics stay honest.
                    if turn_state != TurnState::Cancelled {
                        turn_state = TurnState::RoundLimited;
                    }
                    return Ok(TurnResult::RoundLimit);
                }
                // GOVINDA protocol: if the model claims completion without
                // reaching FINAL_VALIDATION, push back and grant a fix
                // round. This is the "self-correction pressure" the
                // master prompt demands. Only active in enforcement mode
                // so plain chat is unaffected.
                if protocol_on
                    && app.config.protocol.require_quality_gates
                    && app.current_phase
                        != Some(crate::govinda_protocol::ProjectPhase::FinalValidation)
                    && crate::govinda_protocol::looks_like_premature_completion(&out)
                {
                    if fixes_granted < max_fixes {
                        fixes_granted += 1;
                        turn_state = TurnState::SelfCorrecting;
                        let phase_str = app
                            .current_phase
                            .map(|p| p.as_str())
                            .unwrap_or("INSTRUCTION_INGESTION");
                        ui.notice(&format!(
                            "GOVINDA PROTOCOL: premature completion detected (phase={phase_str}). \
                             Granting self-correction round {fixes_granted}/{max_fixes} [state={}] . \
                             Continue with the next phase and call quality_gate_check before \
                             claiming completion.",
                            turn_state.as_str()
                        ));
                        // Inject the pressure as an extra user turn so
                        // the model sees it in the next stream.
                        app.session.push_user(
                            "[GOVINDA PROTOCOL] You have NOT completed the protocol. \
                             Current phase is not FINAL_VALIDATION and the quality_gate_check \
                             tool has not been satisfied. Continue the work — emit your next \
                             [Phase N] marker, then either keep implementing or call \
                             quality_gate_check with phase=FINAL_VALIDATION.",
                        );
                        turn_state = TurnState::Streaming;
                        continue;
                    } else {
                        ui.notice(
                            "GOVINDA PROTOCOL: self-correction budget exhausted; \
                             accepting the current answer.",
                        );
                    }
                }
                round_no += 1;
                turn_state = TurnState::Completed;
                finish_answer(app, ui, out, last_round_error.as_deref());
                ui.timeline(app.config.model.as_str(), started.elapsed());
                app.record_turn(started.elapsed());
                {
                    let model = app.config.model.clone();
                    let latency = started.elapsed().as_millis() as u32;
                    app.router.record_success(&model, latency);
                }
                crate::router_health::append(&crate::router_health::HealthEntry {
                    ts: chrono::Utc::now().to_rfc3339(),
                    model: app.config.model.clone(),
                    latency_ms: started.elapsed().as_millis() as u32,
                    success: true,
                    error: None,
                });
                let router_snapshot = app.router.clone();
                match crate::auto_compact::check_and_run(
                    app,
                    &router_snapshot,
                    crate::auto_compact::SOFT_COMPACT_PCT,
                    crate::auto_compact::HARD_COMPACT_PCT,
                )
                .await
                {
                    crate::auto_compact::Outcome::Noop => {}
                    crate::auto_compact::Outcome::SoftCompacted => {
                        turn_state = TurnState::Compacting;
                        ui.notice(&format!(
                            "auto-compact: history summarized (>= 90% fill) [state={}]",
                            turn_state.as_str()
                        ));
                        turn_state = TurnState::Completed;
                    }
                    crate::auto_compact::Outcome::HardReset => {
                        turn_state = TurnState::Compacting;
                        ui.notice(&format!(
                            "auto-compact: hard reset (>= 98% fill) [state={}]",
                            turn_state.as_str()
                        ));
                        turn_state = TurnState::Completed;
                    }
                }
                return Ok(TurnResult::Answered);
            }
            Err(e) => {
                // State tracking: interruptions are explicit Cancelled,
                // other errors are treated as RoundLimited for now.
                if e.to_string().contains("interrupted") {
                    turn_state = TurnState::Cancelled;
                } else {
                    // Preserve previous state; will become RoundLimited if we bail.
                }
                last_round_error = Some(format!("{e:#}"));
                app.record_error();
                // Record the failure against the active model and
                // try the next non-quarantined entry once before
                // giving up.
                let active = app.config.model.clone();
                let err_str = format!("{e:#}");
                let lower = err_str.to_ascii_lowercase();
                let kind = if lower.contains("empty response") {
                    crate::router::FailureKind::Empty
                } else if lower.contains("structure_limit")
                    || lower.contains("chat_admission_busy")
                    || lower.contains("admission busy")
                    || lower.contains("overloaded")
                    || lower.contains("capacity")
                    || lower.contains("busy")
                {
                    crate::router::FailureKind::Busy
                } else if lower.contains("rate limit")
                    || lower.contains("429")
                    || lower.contains("too many requests")
                {
                    crate::router::FailureKind::RateLimit
                } else if lower.contains("401")
                    || lower.contains("unauthorized")
                    || lower.contains("auth")
                {
                    crate::router::FailureKind::Auth
                } else if lower.contains("timeout") {
                    crate::router::FailureKind::Timeout
                } else if lower.contains("404")
                    || lower.contains("not found")
                    || lower.contains("bad model")
                {
                    crate::router::FailureKind::BadModel
                } else {
                    crate::router::FailureKind::Server
                };
                app.router.record_failure(&active, kind, &err_str);
                crate::router_health::append(&crate::router_health::HealthEntry {
                    ts: chrono::Utc::now().to_rfc3339(),
                    model: active.clone(),
                    latency_ms: started.elapsed().as_millis() as u32,
                    success: false,
                    error: Some(format!("{e:#}")),
                });
                // Three-strike gate: retry same model with backoff until
                // strikes reach quarantine threshold, unless Auth/BadModel.
                let should_promote = app.router.should_promote(&active, kind);
                if !should_promote && kind.is_retryable_on_same_model() {
                    // Backoff before retrying same model. Busy gets longer sleep.
                    let strikes = app.router.health(&active).map(|h| h.strikes).unwrap_or(1) as u64;
                    let base_ms = if kind == crate::router::FailureKind::Busy { 1500 } else { 600 };
                    let wait = Duration::from_millis(base_ms * strikes);
                    let msg = if kind == crate::router::FailureKind::Busy {
                        format!(
                            "router: {active} busy (strike {}/{}) — backing off {:.1}s before retry",
                            strikes,
                            crate::router::STRIKES_TO_QUARANTINE,
                            wait.as_secs_f32()
                        )
                    } else {
                        format!(
                            "router: strike {}/{} on {} ({kind:?}) — backing off {:.1}s before retry",
                            strikes,
                            crate::router::STRIKES_TO_QUARANTINE,
                            active,
                            wait.as_secs_f32()
                        )
                    };
                    ui.notice(&msg);
                    tokio::time::sleep(wait).await;
                    app.session
                        .truncate_messages(resume_len.min(app.session.messages().len()));
                    turn_state = TurnState::Streaming;
                    continue;
                }
                // Promote with preflight check: skip candidates that fail a probe.
                let max_failovers = app.router.iter().count().saturating_sub(1).max(5) as u8;
                if failover_attempts < max_failovers {
                    let http_clone = app.http.clone();
                    let mut promoted: Option<String> = None;
                    let mut attempts_probe = 0u8;
                    let candidates = app.router.iter().count();
                    for _ in 0..candidates {
                        let candidate = {
                            let r = &mut app.router;
                            r.promote().map(|e| e.model.clone())
                        };
                        let Some(cand) = candidate else { break };
                        attempts_probe += 1;
                        // Preflight probe the candidate before spending a full turn.
                        let probe = crate::preflight::probe_active(
                            &http_clone,
                            app.config.provider.as_ref(),
                            &cand,
                        )
                        .await;
                        match probe.status {
                            crate::preflight::ProbeStatus::Ok => {
                                promoted = Some(cand);
                                break;
                            }
                            crate::preflight::ProbeStatus::Warn(_) => {
                                // Warn still usable — accept it.
                                promoted = Some(cand);
                                break;
                            }
                            crate::preflight::ProbeStatus::Err(ref reason) => {
                                ui.notice(&format!(
                                    "router: candidate {cand} preflight failed ({reason}) — trying next"
                                ));
                                // Record a soft strike so we don't loop forever on this candidate.
                                app.router.record_failure(
                                    &cand,
                                    crate::router::FailureKind::Server,
                                    reason,
                                );
                                continue;
                            }
                        }
                    }
                    if let Some(next) = promoted {
                        // Account for any probed skips.
                        failover_attempts = failover_attempts.saturating_add(attempts_probe);
                        let active_clone = active.clone();
                        app.config.set_model(next.clone());
                        app.router.sync_active(&app.config.provider.key().to_string(), &app.config.model);
                        ui.notice(&format!(
                            "router: failover to {} after {} strikes on {} ({}/{} attempts)",
                            next, app.router.health(&active).map(|h| h.strikes).unwrap_or(0), active_clone, failover_attempts, max_failovers
                        ));
                        // Roll back the partial assistant turn so the
                        // next request starts cleanly.
                        app.session
                            .truncate_messages(resume_len.min(app.session.messages().len()));
                        turn_state = TurnState::Streaming;
                        continue;
                    } else if attempts_probe > 0 {
                        // We exhausted candidates via preflight, count them.
                        failover_attempts = failover_attempts.saturating_add(attempts_probe);
                    }
                }
                // No more router entries — try OpenCode as last resort.
                if let Some(next) = try_opencode_fallback(app, &app.http.clone()).await {
                    failover_attempts += 1;
                    let active_clone = active.clone();
                    app.config.set_model(next.clone());
                    ui.notice(&format!(
                        "router: failover to opencode:{} after strike on {} ({} failovers tried)",
                        next, active_clone, failover_attempts
                    ));
                    app.session
                        .truncate_messages(resume_len.min(app.session.messages().len()));
                    turn_state = TurnState::Streaming;
                    continue;
                }
                let fail_fast = ui.fail_fast();
                handle_round_error(app, ui, out, resume_len, e);
                if fail_fast {
                    anyhow::bail!("turn failed");
                }
                // Only set RoundLimited if we weren't already Cancelled.
                if turn_state != TurnState::Cancelled {
                    turn_state = TurnState::RoundLimited;
                }
                return Ok(TurnResult::RoundLimit);
            }
        }
    }
}

/// Executes each requested call locally and commits the assistant turn plus
/// one tool result per call to the session. Gated calls are approved per the
/// [`GatePolicy`]; approved calls run concurrently via `FuturesUnordered`,
/// results surfacing the moment each settles while session state commits in
/// call order.
///
/// Returns `true` when any result signals failure (errored call, declined
/// gate, non-zero exit) — feeding the self-correction loop.
async fn run_tool_round(
    app: &mut App,
    ui: &dyn AgentUi,
    gate: GatePolicy,
    prose: &str,
    calls: &[api::ToolCall],
) -> bool {
    for call in calls {
        ui.notice(&format!(
            "→ {}({})",
            call.function.name, call.function.arguments
        ));
    }

    let executor: Option<std::sync::Arc<dyn crate::tools::ToolExecutor>> =
        app.tool_executor.clone();
    let gated: Vec<bool> = calls
        .iter()
        .map(|call| {
            executor
                .as_ref()
                .is_some_and(|e| e.requires_confirmation(&call.function.name))
        })
        .collect();
    let gated_count = gated.iter().filter(|g| **g).count();

    // Approval pass — interactive prompts never interleave execution.
    let mut approve_rest = gate == GatePolicy::AutoRun;
    if gated_count > 1 && gate == GatePolicy::Interactive && ui.confirm_batch(gated_count) {
        approve_rest = true;
    }

    let mut allowed = Vec::with_capacity(calls.len());
    for (i, call) in calls.iter().enumerate() {
        let approved = match gate {
            GatePolicy::AutoRun => true,
            GatePolicy::DeclineAll => !gated[i],
            GatePolicy::Interactive => {
                if !gated[i] || approve_rest {
                    true
                } else {
                    let more_gated_after = gated[i + 1..].iter().filter(|g| **g).count() > 0;
                    match ui.confirm(
                        &call.function.name,
                        &call.function.arguments,
                        more_gated_after,
                    ) {
                        Confirm::ApprovedAll => {
                            approve_rest = true;
                            true
                        }
                        Confirm::Approved => true,
                        Confirm::Declined => false,
                    }
                }
            }
        };
        allowed.push(approved);
    }

    // Concurrent execution pass; results print as they settle.
    let mut futures: futures_util::stream::FuturesUnordered<_> = calls
        .iter()
        .enumerate()
        .map(|(i, call)| {
            let approved = allowed[i];
            let name = call.function.name.clone();
            let args = call.function.arguments.clone();
            let executor = executor.clone();
            async move {
                let outcome = match (approved, executor) {
                    (false, _) => Err(anyhow::anyhow!("declined")),
                    (true, Some(executor)) => executor.execute(&name, &args).await,
                    (true, None) => Err(anyhow::anyhow!("no tool executor configured")),
                };
                (i, outcome)
            }
        })
        .collect();

    let mut outcomes: Vec<Option<anyhow::Result<String>>> =
        (0..calls.len()).map(|_| None).collect();
    let mut had_failure = false;
    while let Some((i, outcome)) = futures.next().await {
        let ok = outcome.is_ok();
        let snippet = match &outcome {
            Ok(value) => truncate_chars(first_line(value), TOOL_RESULT_DISPLAY_CHARS),
            Err(e) if e.to_string() == "declined" => String::new(),
            Err(_) => String::new(),
        };
        ui.tool_end(
            &calls[i].function.name,
            &calls[i].function.arguments,
            ok,
            &snippet,
        );
        if outcome.is_err() || outcome.as_deref().is_ok_and(result_signals_failure) {
            had_failure = true;
        }
        outcomes[i] = Some(outcome);
    }

    // Session commit stays in call order regardless of completion order; the
    // focus file and diffs from staged edits surface to both UI and model.
    let mut results = Vec::with_capacity(calls.len());
    for (call, outcome) in calls.iter().zip(outcomes) {
        let Some(outcome) = outcome else {
            results.push((
                call.id.clone(),
                format!("error: tool '{}' produced no result", call.function.name),
            ));
            had_failure = true;
            continue;
        };
        match outcome {
            Ok(value) => {
                if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&value) {
                    if let Some(path) = payload.get("path").and_then(|p| p.as_str()) {
                        app.focus_file = Some(path.to_owned());
                        ui.focus_file(path);
                    }
                    if let Some(diff) = payload.get("diff").and_then(|d| d.as_str())
                        && !diff.trim().is_empty()
                    {
                        ui.diff(diff);
                    }
                }
                results.push((call.id.clone(), truncate_result(&value)));
            }
            Err(e) if e.to_string() == "declined" => {
                results.push((
                    call.id.clone(),
                    "error: user declined this operation — ask how to proceed before retrying"
                        .to_owned(),
                ));
            }
            Err(_) => {
                results.push((
                    call.id.clone(),
                    format!("error: tool '{}' failed", call.function.name),
                ));
            }
        }
    }
    app.session.commit_tool_round(prose, calls, &results);
    had_failure
}

fn show_prose(ui: &dyn AgentUi, text: &str) {
    if !text.trim().is_empty() {
        ui.prose(text);
    }
}

/// Commits a final text answer. Empty answers render a placeholder
/// and, when available, forward the most recent round error so the
/// user sees the real reason (HTTP 429, transport failure, etc.)
/// instead of a silent "(empty response)" that hides the cause.
fn finish_answer(
    app: &mut App,
    ui: &dyn AgentUi,
    out: String,
    last_round_error: Option<&str>,
) {
    if out.trim().is_empty() {
        ui.notice("(empty response)");
        if let Some(reason) = last_round_error {
            ui.error(&format!("empty response: {reason}"));
        }
        return;
    }
    ui.answer(&out);
    app.session.push_assistant(out);
}

/// Error policy: keep any partially generated answer (marked interrupted),
/// otherwise roll back to the pre-round state minus the trailing prompt.
fn handle_round_error(
    app: &mut App,
    ui: &dyn AgentUi,
    out: String,
    resume_len: usize,
    e: anyhow::Error,
) {
    let error_chain: Vec<String> = e.chain().map(|c| format!("{c}")).collect();
    let primary = error_chain.first().cloned().unwrap_or_default();

    if !out.is_empty() {
        let kept = format!("{out}\n\n*(interrupted)*");
        app.session.push_assistant(kept.clone());
        ui.answer(&kept);
    }
    // Build a detailed error message with context chain.
    let mut detail = String::new();
    if error_chain.len() > 1 {
        detail.push_str("Caused by:\n");
        for (i, cause) in error_chain.iter().enumerate().skip(1) {
            detail.push_str(&format!("  {i}. {cause}\n"));
        }
    }
    // Add model/provider context.
    detail.push_str(&format!(
        "Model: {} · Provider: {}\n",
        app.config.model,
        app.config.provider.key()
    ));
    let msg = if detail.is_empty() {
        primary
    } else {
        format!("{primary}\n{detail}")
    };
    ui.error(&msg);
    app.session.truncate_messages(resume_len);
    app.session.pop_user();
}

/// Heuristic over a committed tool-result string: `error:` prefixes from
/// the executor, or structured payloads with a non-zero exit code
/// (`run_shell`, `check_project`…) count as failures.
pub fn result_signals_failure(value: &str) -> bool {
    if value.starts_with("error:") {
        return true;
    }
    serde_json::from_str::<serde_json::Value>(value)
        .ok()
        .and_then(|v| v.get("exit_code").and_then(serde_json::Value::as_i64))
        .is_some_and(|code| code != 0)
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_owned()
    } else {
        let cut: String = s.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}

/// Cap applied *before* a tool result enters the session history.
fn truncate_result(s: &str) -> String {
    if s.chars().count() <= MAX_TOOL_RESULT_CHARS {
        s.to_owned()
    } else {
        let cut: String = s.chars().take(MAX_TOOL_RESULT_CHARS).collect();
        format!("{cut}\n…(truncated)")
    }
}

/// Attempts to connect to OpenCode as a last-resort fallback when all
/// OmniRoute models have been exhausted. Probes for a running server
/// (or discovers credentials from disk), then seeds the router with
/// every connectable model. Returns the first available model name on
/// success, `None` if OpenCode is unreachable or has no providers.
///
/// When the server is not running, this function tries to start it.
/// When the CLI is not installed, this function tries to install it via npm.
async fn try_opencode_fallback(
    app: &mut App,
    http: &reqwest::Client,
) -> Option<String> {
    // Only try once per turn to avoid repeated expensive probes.
    if app.opencode_fallback_attempted {
        return None;
    }
    app.opencode_fallback_attempted = true;

    // Step 1: try the server, then local files.
    let catalog = match crate::opencode::fetch_catalog(http).await {
        Ok(c) if !c.is_empty() => c,
        _ => {
            // Server not reachable and no local files — try to start it.
            if crate::opencode::try_start_server(http).await {
                match crate::opencode::fetch_catalog(http).await {
                    Ok(c) if !c.is_empty() => c,
                    _ => return None,
                }
            } else {
                // Server didn't come up — try installing the CLI.
                if crate::opencode::ensure_installed().await {
                    // Installed but server didn't start yet — give it one more shot.
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    match crate::opencode::fetch_catalog(http).await {
                        Ok(c) if !c.is_empty() => c,
                        _ => return None,
                    }
                } else {
                    return None;
                }
            }
        }
    };

    // Step 2: prefer OpenCode's own default (respects user's chosen provider/model
    // and auth), otherwise pick the first entry with a usable credential and models.
    if let Some((entry, model)) = catalog.pick_default() {
        let is_local = entry.base_url.starts_with("http://127.0.0.1")
            || entry.base_url.starts_with("http://localhost");
        let has_auth = !matches!(entry.auth, crate::provider::Auth::None);
        if (has_auth || is_local) && !model.is_empty() {
            let provider = std::sync::Arc::new(crate::opencode::OcProvider::new(
                entry.pid.clone(),
                entry.base_url.clone(),
                entry.auth.clone(),
            ));
            app.config.adopt_provider(provider);
            app.config.set_model(model.to_owned());
            app.router.sync_active(&app.config.provider.key().to_string(), &app.config.model);
            return Some(model.to_owned());
        }
    }
    for entry in &catalog.entries {
        // Skip unauthenticated cloud endpoints (would fail with missing key).
        let is_local = entry.base_url.starts_with("http://127.0.0.1")
            || entry.base_url.starts_with("http://localhost");
        if matches!(entry.auth, crate::provider::Auth::None) && !is_local {
            continue;
        }
        if entry.models.is_empty() {
            continue;
        }
        let provider = std::sync::Arc::new(crate::opencode::OcProvider::new(
            entry.pid.clone(),
            entry.base_url.clone(),
            entry.auth.clone(),
        ));
        if let Some(first_model) = entry.models.first() {
            app.config.adopt_provider(provider);
            app.config.set_model(first_model.clone());
            app.router.sync_active(&app.config.provider.key().to_string(), &app.config.model);
            return Some(first_model.clone());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_detection_covers_errors_and_exit_codes() {
        assert!(result_signals_failure(
            "error: user declined this operation"
        ));
        assert!(result_signals_failure(
            r#"{"exit_code":101,"stdout":"compile error"}"#
        ));
        assert!(!result_signals_failure(r#"{"exit_code":0,"stdout":"ok"}"#));
        // Plain text results (read_file output…) are never failures.
        assert!(!result_signals_failure("[outline]\n    1| fn main()"));
        assert!(!result_signals_failure(""));
        // Malformed JSON without an error prefix: not a failure signal.
        assert!(!result_signals_failure("{not json"));
    }
}
