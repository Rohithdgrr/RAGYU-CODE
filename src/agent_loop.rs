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

/// Runs one full agent turn: stream → optional concurrent tool rounds with
/// self-correction → final answer.
///
/// Session mutation happens here; presentation flows through `ui`. Returns
/// `Err` only when `ui.fail_fast()` is set (one-shot mode); otherwise errors
/// are salvaged locally and reported through [`AgentUi::error`].
pub async fn run_turn(
    app: &mut App,
    ui: &dyn AgentUi,
    gate: GatePolicy,
    input: &str,
) -> anyhow::Result<TurnResult> {
    let started = Instant::now();
    app.session.push_user(input);
    app.last_turn_had_failure = false;

    // Per-turn router: 3 strikes on the active model promote to the
    // next non-quarantined entry. State is local to the turn — the
    // pre-flight probe is the cross-turn check.
    let mut router = crate::router::Router::for_active(
        app.config.provider.key().as_ref(),
        &app.config.model,
    );
    if !app.router_failover {
        router.set_failover(false);
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

    loop {
        round_no += 1;
        if round_no > MAX_TOOL_ROUNDS + fixes_granted {
            ui.notice(&format!(
                "stopped after {} tool rounds{} — ask again to continue.",
                MAX_TOOL_ROUNDS + fixes_granted,
                if fixes_granted > 0 {
                    format!(" (+{fixes_granted} self-correction)")
                } else {
                    String::new()
                }
            ));
            return Ok(TurnResult::RoundLimit);
        }

        let history = {
            let compressed = app.session.messages_compressed();
            app.session
                .window_with_messages(&compressed, app.config.context_tokens, injection.as_deref())
        };
        let auth = app.config.provider.auth();
        let opts = ChatOptions {
            max_response_bytes: app.max_response_bytes,
            read_timeout: app.read_timeout,
            tools: if app.tools_enabled {
                app.tool_specs
                    .iter()
                    .filter(|t| !app.disabled_tools.contains(&t.name))
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            },
            ..ChatOptions::new(auth.token(), app.config.model.as_str(), app.config.temperature)
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
                show_prose(ui, &out);
                let had_failure = run_tool_round(app, ui, gate, &out, &tool_calls).await;
                if had_failure {
                    app.last_turn_had_failure = true;
                }
                if had_failure && fixes_granted < MAX_FIX_ROUNDS {
                    fixes_granted += 1;
                    ui.notice(&format!(
                        "↻ failure detected — granting self-correction round ({fixes_granted}/{MAX_FIX_ROUNDS} max)"
                    ));
                }
                continue; // stream again so the model sees the results
            }
            Ok(()) => {
                finish_answer(app, ui, out);
                ui.timeline(app.config.model.as_str(), started.elapsed());
                app.record_turn(started.elapsed());
                router.record_success(&app.config.model, started.elapsed().as_millis() as u32);
                crate::router_health::append(&crate::router_health::HealthEntry {
                    ts: chrono::Utc::now().to_rfc3339(),
                    model: app.config.model.clone(),
                    latency_ms: started.elapsed().as_millis() as u32,
                    success: true,
                    error: None,
                });
                match crate::auto_compact::check_and_run(
                    app,
                    &mut router,
                    crate::auto_compact::SOFT_COMPACT_PCT,
                    crate::auto_compact::HARD_COMPACT_PCT,
                )
                .await
                {
                    crate::auto_compact::Outcome::Noop => {}
                    crate::auto_compact::Outcome::SoftCompacted => {
                        ui.notice("auto-compact: history summarized (>= 90% fill)");
                    }
                    crate::auto_compact::Outcome::HardReset => {
                        ui.notice("auto-compact: hard reset (>= 98% fill)");
                    }
                }
                return Ok(TurnResult::Answered);
            }
            Err(e) => {
                app.record_error();
                // Record the failure against the active model and
                // try the next non-quarantined entry once before
                // giving up.
                let active = app.config.model.clone();
                router.record_failure(
                    &active,
                    crate::router::FailureKind::Server,
                    &format!("{e:#}"),
                );
                crate::router_health::append(&crate::router_health::HealthEntry {
                    ts: chrono::Utc::now().to_rfc3339(),
                    model: active.clone(),
                    latency_ms: started.elapsed().as_millis() as u32,
                    success: false,
                    error: Some(format!("{e:#}")),
                });
                if failover_attempts < 1 {
                    if let Some(next) = router.promote() {
                        failover_attempts += 1;
                        app.config.model = next.model.clone();
                        ui.notice(&format!(
                            "router: failover to {} after strike on {}",
                            next.model, active
                        ));
                        // Roll back the partial assistant turn so the
                        // next request starts cleanly.
                        app.session
                            .truncate_messages(resume_len.min(app.session.messages().len()));
                        continue;
                    }
                }
                let fail_fast = ui.fail_fast();
                handle_round_error(app, ui, out, resume_len, e);
                if fail_fast {
                    anyhow::bail!("turn failed");
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
        ui.notice(&format!("→ {}({})", call.function.name, call.function.arguments));
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

    let mut outcomes: Vec<Option<anyhow::Result<String>>> = (0..calls.len()).map(|_| None).collect();
    let mut had_failure = false;
    while let Some((i, outcome)) = futures.next().await {
        let ok = outcome.is_ok();
        let snippet = match &outcome {
            Ok(value) => truncate_chars(first_line(value), TOOL_RESULT_DISPLAY_CHARS),
            Err(e) if e.to_string() == "declined" => String::new(),
            Err(_) => String::new(),
        };
        ui.tool_end(&calls[i].function.name, &calls[i].function.arguments, ok, &snippet);
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

/// Commits a final text answer (empty answers render a placeholder).
fn finish_answer(app: &mut App, ui: &dyn AgentUi, out: String) {
    if out.trim().is_empty() {
        ui.notice("(empty response)");
        return;
    }
    ui.answer(&out);
    app.session.push_assistant(out);
}

/// Error policy: keep any partially generated answer (marked interrupted),
/// otherwise roll back to the pre-round state minus the trailing prompt.
fn handle_round_error(app: &mut App, ui: &dyn AgentUi, out: String, resume_len: usize, e: anyhow::Error) {
    if !out.is_empty() {
        let kept = format!("{out}\n\n*(interrupted)*");
        app.session.push_assistant(kept.clone());
        ui.answer(&kept);
        ui.error(&format!("{e:#}"));
    } else {
        app.session.truncate_messages(resume_len);
        app.session.pop_user();
        ui.error(&format!("{e:#}"));
    }
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
