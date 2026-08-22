use anyhow::{Context, Result};
use govinda_cli::api::{self, ChatOptions};
use govinda_cli::clock;
use govinda_cli::commands::{self, App, Outcome};
use govinda_cli::config::Config;
use govinda_cli::render::{Renderer, Spinner, accent, paint};
use govinda_cli::session::Session;
use govinda_cli::sessions;
use reedline::{FileBackedHistory, Prompt, PromptEditMode, PromptHistorySearch, Reedline, Signal};
use std::borrow::Cow;
use std::io::Write;
use std::time::Instant;

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

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    let config = Config::load().context("startup failed")?;
    let http = Config::http_client().context("startup failed")?;
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

    let history_path = std::env::current_dir()?.join(".govinda_history");
    let history =
        FileBackedHistory::with_file(1000, history_path).context("could not open history file")?;
    let mut rl = Reedline::create().with_history(Box::new(history));

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
}

fn parse_args() -> Result<Args> {
    let mut argv = std::env::args().skip(1);
    let mut resume = None;
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--resume" | "-r" => {
                let name = argv
                    .next()
                    .filter(|n| !n.starts_with('-'))
                    .ok_or_else(|| anyhow::anyhow!("--resume needs a session name"))?;
                resume = Some(name);
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument '{other}' — try --help"),
        }
    }
    Ok(Args { resume })
}

fn print_usage() {
    println!(
        "{}\n\nusage: govinda [options]\n\noptions:\n  --resume, -r <name>  continue a saved session (see /sessions)\n  --help, -h           show this help",
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
        }
    } else {
        run_turn(app, line).await;
    }
    Ok(false)
}

/// Upper bound on model↔tool round trips per user turn, so a confused model
/// can never loop forever.
const MAX_TOOL_ROUNDS: usize = 5;

async fn run_turn(app: &mut App, input: &str) {
    app.session.push_user(input);
    let raw = !app.renderer.markdown_enabled();

    for _round in 0..MAX_TOOL_ROUNDS {
        let history = app.session.window(app.config.context_tokens);
        let auth = app.config.provider.auth();
        let tools = app
            .tool_executor
            .as_ref()
            .map_or_else(Vec::new, |t| t.specs());
        let opts = ChatOptions {
            max_response_bytes: app.max_response_bytes,
            read_timeout: app.read_timeout,
            tools,
            ..ChatOptions::new(
                auth.token(),
                app.config.model.as_str(),
                app.config.temperature,
            )
        };

        let spinner = Spinner::start("thinking…", !raw);
        let mut out = String::new();
        let mut tool_calls = Vec::new();
        let started = Instant::now();
        // Everything the session held before this stream attempt; an error
        // with nothing emitted rolls back to exactly this state.
        let resume_len = app.session.messages().len();

        let result = {
            let http = &app.http;
            let provider = app.config.provider.clone();
            tokio::select! {
                res = api::stream_chat(http, provider.as_ref(), &opts, &history, &mut out, &mut tool_calls, |delta| {
                    if raw {
                        print!("{delta}");
                        let _ = std::io::stdout().flush();
                    }
                }) => res,
                _ = tokio::signal::ctrl_c() => Err(anyhow::anyhow!("interrupted")),
            }
        };
        spinner.stop().await;

        match result {
            Ok(()) if !tool_calls.is_empty() => {
                app.record_turn(started.elapsed());
                run_tool_round(app, &tool_calls);
                continue; // stream again so the model sees the results
            }
            Ok(()) => break_final_answer(app, raw, started, out),
            Err(e) => {
                app.record_error();
                println!();
                if !out.is_empty() {
                    // Keep what was already generated; mark it clearly.
                    let kept = format!("{out}\n\n*(interrupted)*");
                    app.session.push_assistant(kept.clone());
                    if raw {
                        println!("{out}");
                    } else {
                        app.renderer.render_answer(&kept);
                    }
                    eprintln!(
                        "{}",
                        paint(format!("error: {e:#}"), govinda_cli::render::err_color())
                    );
                } else {
                    // Roll back to the pre-round state, then drop the trailing
                    // user prompt (only present before any tool rounds ran).
                    app.session.truncate_messages(resume_len);
                    app.session.pop_user();
                    eprintln!(
                        "{}",
                        paint(format!("error: {e:#}"), govinda_cli::render::err_color())
                    );
                }
            }
        }
        return;
    }
    println!(
        "{}",
        paint(
            format!(
                "stopped after {MAX_TOOL_ROUNDS} tool rounds — ask again to continue."
            ),
            govinda_cli::render::dim_color()
        )
    );
}

/// Executes each requested call locally and commits the assistant tool-call
/// message plus one `tool` result per call to the session.
fn run_tool_round(app: &mut App, calls: &[api::ToolCall]) {
    for call in calls {
        println!(
            "{}",
            paint(
                format!("→ {}({})", call.function.name, call.function.arguments),
                govinda_cli::render::dim_color()
            )
        );
    }
    app.session.push_tool_calls(calls.to_vec());
    for call in calls {
        let outcome = match app.tool_executor.as_ref() {
            Some(executor) => executor.execute(&call.function.name, &call.function.arguments),
            None => Err(anyhow::anyhow!("no tool executor configured")),
        };
        let output = match outcome {
            Ok(value) => value,
            Err(e) => format!("error: {e:#}"),
        };
        println!(
            "{}",
            paint(
                format!("← {}", truncate_line(&output, 200)),
                govinda_cli::render::dim_color()
            )
        );
        app.session.push_tool_result(call.id.clone(), output);
    }
}

fn break_final_answer(app: &mut App, raw: bool, started: Instant, out: String) {
    app.record_turn(started.elapsed());
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
