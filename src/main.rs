use anyhow::{Context, Result};
use govinda_cli::clock;
use govinda_cli::commands::{self, App, Outcome};
use govinda_cli::config::Config;
use govinda_cli::render::{Renderer, accent, paint};
use govinda_cli::session::Session;
use govinda_cli::sessions;
use reedline::{
    FileBackedHistory, Prompt, PromptEditMode, PromptHistorySearch, Reedline, Signal, Span,
};
use std::borrow::Cow;
use std::io::{IsTerminal, Write};


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

/// Prompt with an optional file breadcrumb (Phase 6.2):
/// `govinda-cli v0.5.0  📄 src/api.rs  🦀 Rust\n❯ `
struct CliPrompt {
    left: String,
}

impl CliPrompt {
    fn new(focus: Option<&str>) -> Self {
        let dim = govinda_cli::render::dim_color();
        let mut left = paint(
            format!("govinda-cli v{}", env!("CARGO_PKG_VERSION")),
            accent(),
        );
        if let Some(file) = focus.map(str::trim).filter(|f| !f.is_empty()) {
            left.push_str(&paint(format!("  📄 {file}"), accent()));
            if let Some(badge) = govinda_cli::render::language_badge(file) {
                left.push_str(&paint(format!("  {badge}"), dim));
            }
        }
        left.push_str(&paint("\n❯ ".to_owned(), accent()));
        Self { left }
    }
}

impl Default for CliPrompt {
    fn default() -> Self {
        Self::new(None)
    }
}

impl Prompt for CliPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.left)
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
    commands::specialize_system(&mut app);

    // One-prompt build pipeline: plan phases, confirm once, execute
    // autonomously, verify, report. Non-TUI by definition.
    if let Some(prompt) = args.build {
        return run_build(&mut app, &prompt).await;
    }

    // Rich TUI mode: IDE-like panes around the same agent core. Session
    // autosave still applies on exit.
    if args.tui {
        let result = govinda_cli::tui::run(&mut app).await;
        autosave(&mut app);
        return result;
    }

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
        match rl.read_line(&CliPrompt::new(app.focus_file.as_deref())) {
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
    build: Option<String>,
    completion: Option<String>,
    tui: bool,
}

fn parse_args() -> Result<Args> {
    let mut argv = std::env::args().skip(1);
    let mut resume = None;
    let mut query = None;
    let mut build = None;
    let mut completion = None;
    let mut force_repl = false;
    let mut force_tui = false;
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
            "--build" | "-b" => {
                let prompt = argv
                    .next()
                    .filter(|p| !p.starts_with('-'))
                    .ok_or_else(|| anyhow::anyhow!("--build needs a prompt (quote it)"))?;
                build = Some(prompt);
            }
            "--completion" => {
                let shell = argv
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--completion needs a shell name"))?;
                completion = Some(shell);
            }
            "--tui" => force_tui = true,
            "--repl" => force_repl = true,
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument '{other}' — try --help"),
        }
    }
    // TUI is the default for interactive sessions; --repl forces the legacy
    // REPL, --query/-q and --build/-b imply non-TUI, and piped stdout always
    // falls back to the plain REPL.
    let tui = if force_repl || query.is_some() || build.is_some() || !std::io::stdout().is_terminal()
    {
        false
    } else {
        force_tui || true
    };
    Ok(Args {
        resume,
        query,
        build,
        completion,
        tui,
    })
}

fn print_usage() {
    println!(
        "{}\n\nusage: govinda [options]\n\noptions:\n  --resume, -r <name>  continue a saved session (see /sessions)\n  --query, -q <prompt> one-shot mode: answer and exit; piped stdin is appended\n                       to the prompt, e.g. cat file.rs | govinda -q \"review\"\n  --build, -b <prompt> one-prompt pipeline: plan phases (docs → code → deps →\n                       run → preview → verify), confirm once, then execute\n                       autonomously; exit code reflects verification\n  --repl               use the legacy plain-text REPL instead of the rich TUI\n  --completion <shell> print a completion script (bash, zsh, fish, powershell)\n  --help, -h           show this help\n\nthe rich TUI launches by default in interactive terminals.",
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
            Outcome::Handled | Outcome::Undo | Outcome::Reloaded => {}
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

/// Extra fix turns granted when the pipeline's VERIFY phase keeps failing.
const MAX_BUILD_FIX_TURNS: usize = 3;

/// `--build <prompt>` one-prompt pipeline:
///
/// plan (phase-tagged steps) → single confirmation → autonomous execution
/// with all tool gates auto-approved → staged edits applied per step →
/// guaranteed VERIFY phase → fix loop on persistent failure → report.
/// Exit status reflects the final verification result.
async fn run_build(app: &mut App, prompt: &str) -> Result<()> {
    println!(
        "{}",
        paint(format!("build pipeline: {prompt}"), accent())
    );

    let steps = match commands::generate_pipeline(app, prompt).await {
        Ok(steps) if !steps.is_empty() => steps,
        Ok(_) => anyhow::bail!("the model returned no parseable pipeline steps"),
        Err(e) => anyhow::bail!("pipeline planning failed ({e:#})"),
    };

    // The todo list doubles as the pipeline progress tracker.
    app.todos = steps
        .iter()
        .map(|(_, text)| govinda_cli::commands::todo::Todo {
            text: text.clone(),
            done: false,
        })
        .collect();
    commands::persist_todos(app);

    println!();
    println!("{}", paint("proposed pipeline:", accent()));
    for (i, (phase, step)) in steps.iter().enumerate() {
        println!(
            "  {} [{}] {}",
            paint(format!("{:>2}.", i + 1), crossterm::style::Color::DarkGrey),
            phase.tag(),
            step
        );
    }
    print!(
        "{}",
        paint(
            format!(
                "execute {} step(s) autonomously — writes, installs, and runs will be auto-approved? [y/N] ",
                steps.len()
            ),
            crossterm::style::Color::Yellow
        )
    );
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    let confirmed = std::io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes");
    if !confirmed {
        println!(
            "{}",
            paint(
                "pipeline cancelled — plan kept in /todo, nothing executed.",
                govinda_cli::render::dim_color()
            )
        );
        return Ok(());
    }

    app.auto_approve = true;
    let total = steps.len();
    // Per-step report rows: (phase tag, description, succeeded).
    let mut report: Vec<(String, String, bool)> = Vec::new();

    for (i, (phase, step)) in steps.into_iter().enumerate() {
        println!();
        println!(
            "{}",
            paint(
                format!("── [{}] step {}/{}: {}", phase.tag(), i + 1, total, step),
                accent()
            )
        );
        run_turn_auto(
            app,
            &format!("[{} step {}/{}] {}\n\nPhase guidance: {}", phase.tag(), i + 1, total, step, phase.hint()),
        )
        .await;
        // Headless mode has no one to type /apply: commit staged edits now,
        // so RUN/VERIFY operate on the code the model just wrote.
        commands::apply_pending_edits(app);
        report.push((phase.tag().to_owned(), step, !app.last_turn_had_failure));
        if let Some(todo) = app.todos.get_mut(i) {
            todo.done = true;
        }
        commands::persist_todos(app);
    }

    // The verify phase failed? Grant explicit fix attempts before giving up.
    let mut fixes = 0usize;
    while app.last_turn_had_failure && fixes < MAX_BUILD_FIX_TURNS {
        fixes += 1;
        println!();
        println!(
            "{}",
            paint(
                format!("── verification failed — fix attempt {fixes}/{MAX_BUILD_FIX_TURNS}"),
                crossterm::style::Color::Yellow
            )
        );
        run_turn_auto(
            app,
            "[VERIFY] The previous step reported failures (errors, non-zero exits, or failing \
             tests). Diagnose the root cause, fix it, and re-run the relevant tests/checks.",
        )
        .await;
        commands::apply_pending_edits(app);
        report.push((
            "VERIFY".to_owned(),
            format!("fix attempt {fixes}/{MAX_BUILD_FIX_TURNS}"),
            !app.last_turn_had_failure,
        ));
    }

    // Final report; exit code mirrors verification.
    let success = !app.last_turn_had_failure;
    println!();
    println!("{}", paint("── build report ─────────────────────", accent()));
    let width = report.iter().map(|(t, _, _)| t.len()).max().unwrap_or(0);
    for (tag, text, ok) in &report {
        let glyph = if *ok {
            paint("✓", govinda_cli::render::ok_color())
        } else {
            paint("✗", govinda_cli::render::err_color())
        };
        let pad = " ".repeat(width - tag.chars().count());
        println!("  {glyph} [{tag}{pad}] {text}");
    }
    if success {
        println!(
            "{}",
            paint("pipeline complete.", govinda_cli::render::dim_color())
        );
        Ok(())
    } else {
        anyhow::bail!("pipeline finished with failures after {MAX_BUILD_FIX_TURNS} fix attempt(s)")
    }
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

    let ui = CliUi::one_shot();
    match govinda_cli::agent_loop::run_turn(
        app,
        &ui,
        govinda_cli::agent_loop::GatePolicy::DeclineAll,
        &full,
    )
    .await
    {
        Ok(govinda_cli::agent_loop::TurnResult::Answered) => Ok(()),
        _ => anyhow::bail!(
            "stopped after {} tool rounds without a final answer",
            govinda_cli::agent_loop::MAX_TOOL_ROUNDS
        ),
    }
}

// ---------------------------------------------------------------------------
// CLI frontend adapter for the shared agent loop (src/agent_loop.rs).
// ---------------------------------------------------------------------------

/// REPL presentation: live raw streaming, markdown rendering for answers,
/// stdin-based confirmation gates. One implementation of `AgentUi` serves
/// the interactive REPL, `--build`, and `-q`.
struct CliUi {
    /// Snapshot of `renderer.markdown_enabled()` taken at turn start.
    markdown: bool,
    /// `-q` mode: stream deltas always, decline gated calls, fail fast.
    one_shot: bool,
}

impl CliUi {
    fn interactive(markdown: bool) -> Self {
        Self {
            markdown,
            one_shot: false,
        }
    }

    fn one_shot() -> Self {
        Self {
            markdown: false,
            one_shot: true,
        }
    }

    fn raw(&self) -> bool {
        self.one_shot || !self.markdown
    }
}

impl govinda_cli::agent_loop::AgentUi for CliUi {
    fn raw_stream(&self) -> bool {
        self.raw()
    }

    fn stream_delta(&self, delta: &str) {
        print!("{delta}");
        let _ = std::io::stdout().flush();
    }

    fn prose(&self, text: &str) {
        println!();
        if !self.raw() {
            govinda_cli::render::Renderer::new(self.markdown).render_answer(text);
        }
    }

    fn answer(&self, text: &str) {
        if self.raw() {
            println!("\n");
        } else {
            println!();
            govinda_cli::render::Renderer::new(self.markdown).render_answer(text);
        }
    }

    fn tool_start(&self, name: &str, args: &str) {
        println!(
            "{}",
            paint(
                format!("→ {name}({args})"),
                govinda_cli::render::dim_color()
            )
        );
    }

    fn tool_end(&self, _name: &str, _args: &str, ok: bool, snippet: &str) {
        if !ok && snippet.is_empty() {
            return;
        }
        println!(
            "{}",
            paint(
                format!("← {snippet}"),
                govinda_cli::render::dim_color()
            )
        );
    }

    fn diff(&self, diff: &str) {
        govinda_cli::render::render_diff(diff);
    }

    fn notice(&self, text: &str) {
        println!(
            "{}",
            paint(
                text.to_owned(),
                govinda_cli::render::dim_color()
            )
        );
    }

    fn error(&self, text: &str) {
        eprintln!(
            "{}",
            paint(format!("error: {text}"), govinda_cli::render::err_color())
        );
    }

    fn timeline(&self, model: &str, elapsed: std::time::Duration) {
        println!(
            "{}",
            paint(
                format!("── {model} · {:.1}s", elapsed.as_secs_f32()),
                govinda_cli::render::dim_color()
            )
        );
    }

    fn confirm_batch(&self, gated_count: usize) -> bool {
        // The call list preview is printed by the loop via tool_start lines
        // already; here we only ask once for all of them.
        println!();
        println!(
            "{}",
            paint(
                format!("⚠ {gated_count} tools modify your workspace:"),
                crossterm::style::Color::Yellow
            )
        );
        print!(
            "{}",
            paint("approve all? [y/N] ", crossterm::style::Color::Yellow)
        );
        read_confirmation_answer()
    }

    fn confirm(&self, name: &str, arguments_json: &str, allow_all: bool) -> govinda_cli::agent_loop::Confirm {
        use govinda_cli::agent_loop::Confirm;
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
        let hint = if allow_all { "[y/N/a(ll)]" } else { "[y/N]" };
        print!(
            "{}",
            paint(format!("proceed? {hint} "), crossterm::style::Color::Yellow)
        );
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        match std::io::stdin().read_line(&mut answer) {
            Ok(0) | Err(_) => return Confirm::Declined,
            Ok(_) => {}
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => Confirm::Approved,
            "a" | "all" if allow_all => Confirm::ApprovedAll,
            _ => Confirm::Declined,
        }
    }

    fn fail_fast(&self) -> bool {
        self.one_shot
    }
}

fn read_confirmation_answer() -> bool {
    use std::io::Write as _;
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

/// Interactive REPL turn through the shared loop.
async fn run_turn(app: &mut App, input: &str) {
    let ui = CliUi::interactive(app.renderer.markdown_enabled());
    match govinda_cli::agent_loop::run_turn(app, &ui, govinda_cli::agent_loop::GatePolicy::Interactive, input)
        .await
    {
        Ok(_) => {}
        Err(e) => eprintln!(
            "{}",
            paint(format!("error: {e:#}"), govinda_cli::render::err_color())
        ),
    }
}

/// Auto-run turn used by the `--build` pipeline: gated tools execute
/// without per-call prompts (the pipeline was confirmed once up front).
async fn run_turn_auto(app: &mut App, input: &str) {
    let ui = CliUi::interactive(app.renderer.markdown_enabled());
    let _ = govinda_cli::agent_loop::run_turn(
        app,
        &ui,
        govinda_cli::agent_loop::GatePolicy::AutoRun,
        input,
    )
    .await;
}
