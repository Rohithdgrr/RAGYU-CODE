use anyhow::{Context, Result};
use govinda_cli::api::{self, ChatOptions};
use govinda_cli::clock;
use govinda_cli::commands::{self, App, Outcome};
use govinda_cli::config::Config;
use govinda_cli::render::{Renderer, Spinner, accent, paint};
use govinda_cli::session::Session;
use govinda_cli::sessions;
use reedline::{
    FileBackedHistory, Prompt, PromptEditMode, PromptHistorySearch, Reedline, Signal, Span,
};
use std::borrow::Cow;
use std::io::Write;
use std::time::{Duration, Instant};

/// Completes slash commands as the user types `/mod` → `/models`, `/model`.
struct SlashCompleter;

impl reedline::Completer for SlashCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> reedline::CompletionResult {
        let start = line[..pos].rfind(char::is_whitespace).map_or(0, |i| i + 1);
        let partial = &line[start..pos];
        if start != 0 || !partial.starts_with('/') {
            return reedline::CompletionResult::fresh(Vec::new());
        }
        let suggestions: Vec<reedline::Suggestion> = commands::SLASH_COMMANDS
            .iter()
            .filter(|c| c.starts_with(partial))
            .map(|c| reedline::Suggestion {
                value: (*c).to_owned(),
                span: Span::new(start, pos),
                ..Default::default()
            })
            .collect();
        reedline::CompletionResult::fresh(suggestions)
    }
}

struct CliPrompt;

impl Prompt for CliPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Owned(paint("❯ ".to_owned(), accent()))
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, mode: PromptEditMode) -> Cow<'_, str> {
        match mode {
            PromptEditMode::Default | PromptEditMode::Emacs | PromptEditMode::Helix(_) => {
                Cow::Borrowed("")
            }
            PromptEditMode::Vi(_) => Cow::Borrowed(": "),
            PromptEditMode::Custom(prog) => Cow::Owned(format!(":{prog} ")),
        }
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("… ")
    }

    fn render_prompt_history_search_indicator(&self, search: PromptHistorySearch) -> Cow<'_, str> {
        Cow::Owned(format!("(search `{}`) ", search.term))
    }
}

/// Appended to the system prompt whenever function calling is available:
/// steers the model toward the workspace tools instead of guessing.
const AGENT_SYSTEM_ADDENDUM: &str = "\n\nYou are a coding agent working inside the user's project \
workspace. You use edit_file/insert_after/insert_before for changes (staged for review via \
view_diff), run_shell or check_project to verify compilation, find_symbol to locate definitions, \
and never guess line numbers — read files or query the symbol index before editing.";

/// Extra rounds granted after a failed tool round (self-correction loop):
/// a failing `cargo check` goes back to the model as-is so it can fix it.
const MAX_FIX_ROUNDS: usize = 3;

/// Applies agent specialization when tools are on; plain chat keeps the
/// user's configured system prompt untouched.
fn specialize_system(app: &mut App) {
    if app.tools_enabled {
        let specialized = format!("{}{AGENT_SYSTEM_ADDENDUM}", app.session.system());
        app.session.set_system(specialized);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    if let Some(shell) = &args.completion {
        govinda_cli::completions::emit(shell)?;
        return Ok(());
    }
    let config = Config::load().context("startup failed")?;
    let http = Config::http_client().context("startup failed")?;
    if let Some(theme) = &config.theme {
        govinda_cli::render::set_theme(theme);
    }
    let renderer = Renderer::new(config.render_markdown);

    // Resume a named session, or start fresh.
    let mut app = match &args.resume {
        Some(name) => {
            let session = sessions::load_named(name)
                .with_context(|| format!("cannot resume session '{name}'"))?;
            println!(
                "{}",
                paint(
                    format!(
                        "resumed '{name}' · {} messages · last saved {}",
                        session.messages().len(),
                        session.updated_at().unwrap_or("unknown")
                    ),
                    accent()
                )
            );
            App::new(config, http, session, renderer)
        }
        None => {
            let session = Session::new(config.system_prompt.clone());
            App::new(config, http, session, renderer)
        }
    };
    if let Some(name) = &args.resume {
        app.session_name = Some(name.clone());
    }
    specialize_system(&mut app);

    // One-shot mode: answer the prompt (plus any piped stdin), then exit.
    // No banner, no REPL, no session autosave.
    if let Some(prompt) = args.query {
        return run_query(&mut app, &prompt).await;
    }

    println!(
        "{}",
        paint(
            format!("govinda-cli v{}", env!("CARGO_PKG_VERSION")),
            accent()
        )
    );
    println!(
        "{}",
        paint(
            "type /help for commands · Ctrl+C cancels a reply · Ctrl+D exits".to_owned(),
            govinda_cli::render::dim_color()
        )
    );

    // Phase-4 symbol index: built once at startup, refreshed by /scan or
    // any scan_project tool call. Failures never block startup.
    if let Ok(cwd) = std::env::current_dir() {
        let n = govinda_cli::symbols::rebuild(&cwd);
        if n > 0 {
            println!(
                "{}",
                paint(
                    format!("indexed {n} workspace symbols (/scan refreshes)"),
                    govinda_cli::render::dim_color()
                )
            );
        }
    }

    let history_path = std::env::current_dir()?.join(".govinda_history");
    let history =
        FileBackedHistory::with_file(1000, history_path).context("could not open history file")?;
    let mut rl = Reedline::create()
        .with_history(Box::new(history))
        .with_completer(Box::new(SlashCompleter));

    loop {
        match rl.read_line(&CliPrompt) {
            Ok(Signal::Success(line)) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if line.starts_with('/') && line.trim().len() == 1 {
                    continue;
                }
                match handle_line(line, &mut app).await {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(e) => eprintln!(
                        "{}",
                        paint(format!("error: {e:#}"), govinda_cli::render::err_color())
                    ),
                }
            }
            Ok(Signal::CtrlC) => {} // clears the input line
            Ok(Signal::CtrlD) => break,
            Ok(_) => {} // future reedline signals: treat as no-op
            Err(e) => {
                eprintln!("input error: {e}");
                break;
            }
        }
    }

    println!("{}", paint("bye.", govinda_cli::render::dim_color()));
    autosave(&mut app);
    Ok(())
}

struct Args {
    resume: Option<String>,
    query: Option<String>,
    completion: Option<String>,
}

fn parse_args() -> Result<Args> {
    let mut argv = std::env::args().skip(1);
    let mut resume = None;
    let mut query = None;
    let mut completion = None;
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--resume" | "-r" => {
                let name = argv
                    .next()
                    .filter(|n| !n.starts_with('-'))
                    .ok_or_else(|| anyhow::anyhow!("--resume needs a session name"))?;
                resume = Some(name);
            }
            "--query" | "-q" => {
                let prompt = argv
                    .next()
                    .filter(|p| !p.starts_with('-'))
                    .ok_or_else(|| anyhow::anyhow!("-q needs a prompt (quote it)"))?;
                query = Some(prompt);
            }
            "--completion" => {
                let shell = argv
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--completion needs a shell name"))?;
                completion = Some(shell);
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument '{other}' — try --help"),
        }
    }
    Ok(Args {
        resume,
        query,
        completion,
    })
}

fn print_usage() {
    println!(
        "{}\n\nusage: govinda [options]\n\noptions:\n  --resume, -r <name>  continue a saved session (see /sessions)\n  --query, -q <prompt> one-shot mode: answer and exit; piped stdin is appended\n                       to the prompt, e.g. cat file.rs | govinda -q \"review\"\n  --completion <shell> print a completion script (bash, zsh, fish, powershell)\n  --help, -h           show this help",
        paint(
            format!("govinda-cli v{}", env!("CARGO_PKG_VERSION")),
            accent()
        )
    );
}

/// Saves the conversation on the way out so nothing is ever lost.
/// Named sessions keep their name; unnamed ones get `auto-<epoch>`.
fn autosave(app: &mut App) {
    if app.session.messages().is_empty() {
        return;
    }
    let name = app
        .session_name
        .clone()
        .unwrap_or_else(|| format!("auto-{}", clock::epoch_secs()));
    let path = match sessions::named_session_path(&name) {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "{}",
                paint(
                    format!("autosave skipped ({e:#})"),
                    govinda_cli::render::err_color()
                )
            );
            return;
        }
    };
    match app.session.save_to(&path) {
        Ok(()) => println!(
            "{}",
            paint(
                format!("session saved to {}", path.display()),
                govinda_cli::render::dim_color()
            )
        ),
        Err(e) => eprintln!(
            "{}",
            paint(
                format!("could not save session: {e:#}"),
                govinda_cli::render::err_color()
            )
        ),
    }
}

/// Returns `true` when the REPL should exit.
async fn handle_line(line: &str, app: &mut App) -> Result<bool> {
    if line.starts_with('/') {
        match commands::dispatch(line, app).await {
            Outcome::Exit => return Ok(true),
            Outcome::Handled => {}
            Outcome::Resend(text) => run_turn(app, &text).await,
            Outcome::Plan(steps) => execute_plan(app, steps).await,
        }
    } else {
        run_turn(app, line).await;
    }
    Ok(false)
}

/// Executes a confirmed `/plan` step by step. Each step runs through the
/// normal agent loop — tool calls, confirmations, and the self-correction
/// loop all stay active — and progress is tracked in `/todo`.
async fn execute_plan(app: &mut App, steps: Vec<String>) {
    println!(
        "{}",
        paint(
            format!("execute {} step(s) autonomously now?", steps.len()),
            crossterm::style::Color::Yellow
        )
    );
    print!(
        "{}",
        paint("proceed? [y/N] ", crossterm::style::Color::Yellow)
    );
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    let read_ok = std::io::stdin().read_line(&mut answer).is_ok();
    let confirmed = read_ok && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes");
    if !confirmed {
        println!(
            "{}",
            paint(
                "plan kept in /todo — nothing executed.",
                govinda_cli::render::dim_color()
            )
        );
        return;
    }

    let total = steps.len();
    for (i, step) in steps.into_iter().enumerate() {
        println!();
        println!(
            "{}",
            paint(
                format!("── plan step {}/{}: {step}", i + 1, total),
                accent()
            )
        );
        run_turn(app, &format!("[plan step {}/{}] {step}", i + 1, total)).await;
        if let Some(todo) = app.todos.get_mut(i) {
            todo.done = true;
        }
        govinda_cli::commands::persist_todos(app);
    }
    println!(
        "{}",
        paint("plan complete.", govinda_cli::render::dim_color())
    );
}

/// One-shot `-q` mode: streams the answer to stdout as plain text, runs
/// tool rounds with confirmation-gated calls auto-declined (no user is
/// watching), and never autosaves. Errors go to stderr with a non-zero exit.
async fn run_query(app: &mut App, prompt: &str) -> Result<()> {
    app.non_interactive = true;

    // Piped stdin becomes context appended after the typed prompt.
    use std::io::IsTerminal;
    let mut full = prompt.to_owned();
    if !std::io::stdin().is_terminal() {
        match std::io::read_to_string(std::io::stdin()) {
            Ok(piped) if !piped.trim().is_empty() => {
                full.push_str("\n\n---\n\n");
                full.push_str(piped.trim_end());
            }
            Ok(_) => {}
            Err(e) => eprintln!("warning: could not read piped stdin ({e})"),
        }
    }

    // Context-aware windowing works in one-shot mode too.
    let injection = {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let files = govinda_cli::context::relevant_files(&full, &cwd);
        govinda_cli::context::build_injection(&files, &cwd)
    };

    for _round in 0..MAX_TOOL_ROUNDS {
        let history = app
            .session
            .window_with(app.config.context_tokens, injection.as_deref());
        let auth = app.config.provider.auth();
        let opts = chat_options(app, &auth);
        let mut out = String::new();
        let mut tool_calls = Vec::new();
        let result = {
            let http = &app.http;
            let provider = app.config.provider.clone();
            let mut sink = api::StreamSink::new(&mut out, &mut tool_calls);
            tokio::select! {
                res = api::stream_chat(http, provider.as_ref(), &opts, &history, &mut sink, |delta| {
                    print!("{delta}");
                    let _ = std::io::stdout().flush();
                }) => res,
                _ = tokio::signal::ctrl_c() => Err(anyhow::anyhow!("interrupted")),
            }
        };
        result?;
        println!();

        if tool_calls.is_empty() || !app.tools_enabled {
            app.record_turn(Duration::ZERO);
            return Ok(());
        }
        run_tool_round(app, &out, &tool_calls).await;
    }
    anyhow::bail!("stopped after {MAX_TOOL_ROUNDS} tool rounds without a final answer")
}

/// Upper bound on model↔tool round trips per user turn, so a confused model
/// can never loop forever.
const MAX_TOOL_ROUNDS: usize = 5;
/// Cap on a single tool result stored in history (display truncation is
/// separate); a huge result would otherwise wreck the context budget.
const MAX_TOOL_RESULT_CHARS: usize = 8 * 1024;
/// Characters of a tool result shown on screen.
const TOOL_RESULT_DISPLAY_CHARS: usize = 200;

async fn run_turn(app: &mut App, input: &str) {
    app.session.push_user(input);
    let raw = !app.renderer.markdown_enabled();
    let mut rounds_elapsed = std::time::Duration::ZERO;

    // Context-aware windowing: files the prompt mentions (plus their
    // manifest and same-dir siblings) ride along even if they only appeared
    // in old messages — computed once from this turn's input.
    let injection = {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let files = govinda_cli::context::relevant_files(input, &cwd);
        govinda_cli::context::build_injection(&files, &cwd)
    };

    // Self-correction budget: rounds are capped at MAX_TOOL_ROUNDS, but a
    // failed tool round (compile error, declined check…) grants extra turns
    // — up to MAX_FIX_ROUNDS — so the model can react to its own failures.
    let mut fixes_granted = 0usize;
    let mut round_no = 0usize;

    loop {
        round_no += 1;
        if round_no > MAX_TOOL_ROUNDS + fixes_granted {
            app.record_turn(rounds_elapsed);
            show_timeline(app, rounds_elapsed);
            println!(
                "{}",
                paint(
                    format!(
                        "stopped after {} tool rounds{} — ask again to continue.",
                        MAX_TOOL_ROUNDS + fixes_granted,
                        if fixes_granted > 0 {
                            format!(" (+{fixes_granted} self-correction)")
                        } else {
                            String::new()
                        }
                    ),
                    govinda_cli::render::dim_color()
                )
            );
            return;
        }

        let history = app
            .session
            .window_with(app.config.context_tokens, injection.as_deref());
        let auth = app.config.provider.auth();
        let opts = chat_options(app, &auth);
        let started = Instant::now();
        // Everything the session held before this stream attempt; an error
        // with nothing emitted rolls back to exactly this state.
        let resume_len = app.session.messages().len();
        let (result, out, tool_calls) = stream_round(app, &history, &opts).await;
        rounds_elapsed += started.elapsed();

        match result {
            Ok(()) if !tool_calls.is_empty() && app.tools_enabled => {
                show_round_prose(app, raw, &out);
                let had_failure = run_tool_round(app, &out, &tool_calls).await;
                if had_failure && fixes_granted < MAX_FIX_ROUNDS {
                    fixes_granted += 1;
                    println!(
                        "{}",
                        paint(
                            format!(
                                "↻ failure detected — granting self-correction round ({}/{} max)",
                                fixes_granted, MAX_FIX_ROUNDS
                            ),
                            govinda_cli::render::dim_color()
                        )
                    );
                }
                continue; // stream again so the model sees the results
            }
            Ok(()) => {
                finish_text_answer(app, raw, out);
                show_timeline(app, rounds_elapsed);
                app.record_turn(rounds_elapsed);
            }
            Err(e) => handle_round_error(app, raw, out, resume_len, e),
        }
        return;
    }
}

/// Per-request options from current settings; tool schemas come from the
/// specs cached at startup.
fn chat_options<'a>(app: &'a App, auth: &'a govinda_cli::provider::Auth) -> ChatOptions<'a> {
    ChatOptions {
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
        ..ChatOptions::new(
            auth.token(),
            app.config.model.as_str(),
            app.config.temperature,
        )
    }
}

/// One stream attempt (spinner + Ctrl+C race included). Nothing is committed
/// here; the caller decides what survives.
async fn stream_round(
    app: &App,
    history: &[api::Message],
    opts: &ChatOptions<'_>,
) -> (anyhow::Result<()>, String, Vec<api::ToolCall>) {
    let raw = !app.renderer.markdown_enabled();
    let mut out = String::new();
    let mut tool_calls = Vec::new();

    let spinner = Spinner::start("thinking…", !raw);
    let result = {
        let http = &app.http;
        let provider = app.config.provider.clone();
        let mut sink = api::StreamSink::new(&mut out, &mut tool_calls);
        tokio::select! {
            res = api::stream_chat(http, provider.as_ref(), opts, history, &mut sink, |delta| {
                if raw {
                    print!("{delta}");
                    let _ = std::io::stdout().flush();
                }
            }) => res,
            _ = tokio::signal::ctrl_c() => Err(anyhow::anyhow!("interrupted")),
        }
    };
    spinner.stop().await;
    (result, out, tool_calls)
}

/// Shows prose the model streamed before requesting tool calls. Raw mode has
/// already printed it live — only the separator is added there.
fn show_round_prose(app: &App, raw: bool, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    println!();
    if !raw {
        app.renderer.render_answer(text);
    }
}

/// Error policy for one failed round: keep any partially generated answer
/// (marked interrupted), or roll the session back to the pre-round state.
fn handle_round_error(app: &mut App, raw: bool, out: String, resume_len: usize, e: anyhow::Error) {
    app.record_error();
    println!();
    let err_paint = || paint(format!("error: {e:#}"), govinda_cli::render::err_color());
    if !out.is_empty() {
        // Keep what was already generated; mark it clearly. Raw mode has
        // already streamed the text live, so only markdown re-renders.
        let kept = format!("{out}\n\n*(interrupted)*");
        app.session.push_assistant(kept.clone());
        if !raw {
            app.renderer.render_answer(&kept);
        }
        eprintln!("{}", err_paint());
    } else {
        // Roll back, then drop the trailing user prompt (only present
        // before any tool rounds ran).
        app.session.truncate_messages(resume_len);
        app.session.pop_user();
        eprintln!("{}", err_paint());
    }
}

/// Executes each requested call locally and commits the assistant turn
/// (prose included) plus one `tool` result per call to the session.
///
/// Confirmation-gated tools (workspace writes, shell commands) are approved
/// sequentially first so prompts never interleave; the approved calls then
/// execute concurrently via boxed futures, with results printed in call
/// order once all settle. Declined calls report a sanitized decline line
/// back to the model.
///
/// The model only ever sees a sanitized failure line; the detailed error
/// chain is printed locally so it never leaks file paths or internals.
///
/// Returns `true` when any result in the round signals failure — an errored
/// call, a declined gate, or a command that exited non-zero — which feeds
/// the self-correction loop.
async fn run_tool_round(app: &mut App, prose: &str, calls: &[api::ToolCall]) -> bool {
    for call in calls {
        println!(
            "{}",
            paint(
                format!("→ {}({})", call.function.name, call.function.arguments),
                govinda_cli::render::dim_color()
            )
        );
    }

    // Sequential approval pass — user prompts must not interleave. In `-q`
    // mode nobody can approve, so gated calls are auto-declined.
    let mut allowed = Vec::with_capacity(calls.len());
    for call in calls {
        let needs_confirmation = app
            .tool_executor
            .as_ref()
            .is_some_and(|e| e.requires_confirmation(&call.function.name));
        let approved = needs_confirmation
            && !app.non_interactive
            && confirm_tool_call(&call.function.name, &call.function.arguments);
        if !approved && !app.non_interactive {
            println!("{}", paint("✗ declined", govinda_cli::render::err_color()));
        }
        allowed.push(approved);
    }

    // Concurrent execution pass; results stay ordered by call index.
    let executor: Option<&dyn govinda_cli::tools::ToolExecutor> = app.tool_executor.as_deref();
    let futures = calls.iter().enumerate().map(|(i, call)| {
        let approved = allowed[i];
        let name = call.function.name.as_str();
        let args = call.function.arguments.as_str();
        async move {
            match (approved, executor) {
                (false, _) => Err(anyhow::anyhow!("declined")),
                (true, Some(executor)) => executor.execute(name, args).await,
                (true, None) => Err(anyhow::anyhow!("no tool executor configured")),
            }
        }
    });
    let outcomes = futures_util::future::join_all(futures).await;

    let mut results = Vec::with_capacity(calls.len());
    let mut had_failure = false;
    for (call, outcome) in calls.iter().zip(outcomes) {
        match outcome {
            Ok(value) => {
                println!(
                    "{}",
                    paint(
                        format!("← {}", truncate_line(&value, TOOL_RESULT_DISPLAY_CHARS)),
                        govinda_cli::render::dim_color()
                    )
                );
                if result_signals_failure(&value) {
                    had_failure = true;
                }
                results.push((call.id.clone(), truncate_result(&value)));
            }
            Err(e) if e.to_string() == "declined" => {
                println!("{}", paint("✗ declined", govinda_cli::render::err_color()));
                had_failure = true;
                results.push((
                    call.id.clone(),
                    "error: user declined this operation — ask how to proceed before retrying"
                        .to_owned(),
                ));
            }
            Err(e) => {
                eprintln!(
                    "{}",
                    paint(
                        format!("tool '{}' failed: {e:#}", call.function.name),
                        govinda_cli::render::err_color()
                    )
                );
                had_failure = true;
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

/// Heuristic over a committed tool-result string: `error:` prefixes from
/// the executor, or a structured JSON payload with a non-zero exit code
/// (`run_shell`, `check_project`…) count as failures.
fn result_signals_failure(value: &str) -> bool {
    if value.starts_with("error:") {
        return true;
    }
    serde_json::from_str::<serde_json::Value>(value)
        .ok()
        .and_then(|v| v.get("exit_code").and_then(serde_json::Value::as_i64))
        .is_some_and(|code| code != 0)
}

/// Interactive y/N gate for workspace-mutating tools. Shows a truncated
/// pretty-print of the arguments so the user can see exactly what would run.
fn confirm_tool_call(name: &str, arguments_json: &str) -> bool {
    println!();
    println!(
        "{}",
        paint(
            format!("⚠ tool '{name}' modifies your workspace:"),
            crossterm::style::Color::Yellow
        )
    );
    match serde_json::from_str::<serde_json::Value>(arguments_json) {
        Ok(value) => {
            let pretty = serde_json::to_string_pretty(&value).unwrap_or_default();
            let preview = truncate_chars(&pretty, 2000);
            for line in preview.lines().take(40) {
                println!("  {line}");
            }
        }
        Err(_) => println!("  {}", truncate_chars(arguments_json, 2000)),
    }
    print!(
        "{}",
        paint("proceed? [y/N] ", crossterm::style::Color::Yellow)
    );
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    match std::io::stdin().read_line(&mut answer) {
        Ok(0) | Err(_) => false,
        Ok(_) => matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
    }
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_owned()
    } else {
        let cut: String = s.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}

/// Dimmed footer printed after every completed answer: model, wall time,
/// and how many agent rounds it took — a mini timeline for the turn.
fn show_timeline(app: &App, elapsed: std::time::Duration) {
    println!(
        "{}",
        paint(
            format!("── {} · {:.1}s", app.config.model, elapsed.as_secs_f32()),
            govinda_cli::render::dim_color()
        )
    );
}

fn finish_text_answer(app: &mut App, raw: bool, out: String) {
    if out.trim().is_empty() {
        println!(
            "{}",
            paint("(empty response)", govinda_cli::render::dim_color())
        );
        return;
    }
    if raw {
        println!("\n");
    } else {
        println!();
        app.renderer.render_answer(&out);
    }
    app.session.push_assistant(out);
}

fn truncate_line(s: &str, max_chars: usize) -> String {
    let first = s.lines().next().unwrap_or("");
    if first.chars().count() <= max_chars {
        first.to_owned()
    } else {
        let cut: String = first.chars().take(max_chars).collect();
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
