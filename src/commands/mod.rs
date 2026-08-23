mod display;
mod edits;
mod generation;
mod persistence;
mod plan;
mod project;
mod todo;

use crate::config::Config;
use crate::render::{Renderer, accent, dim_color, err_color, ok_color, paint, theme_names};
use crate::session::Session;
use crate::tools::{BuiltinTools, PendingEdits, ToolExecutor};
use display::{
    print_history, search_history, set_limit, set_or_show_system, set_or_show_theme,
    set_temperature, set_timeout, show_config, show_stats, show_tools,
};
use edits::{
    apply as apply_edits, reject as reject_edits, review as review_edits, view as view_diff,
};
use generation::{compact, generate_variants, models, pick_variant, retry, set_model};
use persistence::{export, fork_session, list_named_sessions, load_session, save_session};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use todo::Todo;

/// Step parser shared with the TUI planner.
pub use plan::parse_steps;

/// Every slash command the REPL accepts. Drives the reedline completer and
/// shell-completion scripts; keep in sync with `dispatch()` / `help()`.
pub const SLASH_COMMANDS: [&str; 37] = [
    "/help",
    "/exit",
    "/quit",
    "/clear",
    "/reset",
    "/models",
    "/model",
    "/temp",
    "/system",
    "/history",
    "/undo",
    "/retry",
    "/variants",
    "/pick",
    "/compact",
    "/search",
    "/save",
    "/load",
    "/sessions",
    "/fork",
    "/export",
    "/stats",
    "/theme",
    "/tokens",
    "/raw",
    "/config",
    "/timeout",
    "/limit",
    "/tools",
    "/todo",
    "/diff",
    "/apply",
    "/reject",
    "/review",
    "/scan",
    "/plan",
    "/project",
];

/// Shared mutable state for the REPL and command handlers.
pub struct App {
    pub config: Config,
    pub http: reqwest::Client,
    pub session: Session,
    pub renderer: Renderer,
    /// Name of the current named session (drives auto-save on exit).
    pub session_name: Option<String>,
    pub models_cache: Option<Arc<Vec<String>>>,
    /// Per-request read-stall timeout (tuned at runtime via `/timeout`).
    pub read_timeout: Duration,
    /// Per-response size cap in bytes (tuned at runtime via `/limit`).
    pub max_response_bytes: usize,
    pub stats: Stats,
    /// Alternates produced by `/variants`, awaiting a `/pick`.
    pub pending_variants: Vec<String>,
    /// Executes model-requested tool calls (`None` disables function calling).
    /// Shared so the tool round can clone it out and stream results while
    /// still mutating `App` between completions.
    pub tool_executor: Option<Arc<dyn ToolExecutor>>,
    /// Master switch for function calling (toggled via `/tools`); when off,
    /// no tools are advertised and any calls a rogue server sends are ignored.
    pub tools_enabled: bool,
    /// Individually disabled tool names (`/tools disable <name>`), persisted
    /// to `.govinda_tools.json` and excluded from the advertised specs.
    pub disabled_tools: HashSet<String>,
    /// Tool schemas built once at startup (JSON construction per request
    /// would be pure waste — they never change mid-session).
    pub tool_specs: Vec<crate::api::Tool>,
    /// Session-scoped task list (`/todo`), persisted to `.govinda_todo.json`.
    pub todos: Vec<Todo>,
    /// True in `-q` one-shot mode: no interactive prompts (confirmation-
    /// gated tools are auto-declined) and no interactive-only output.
    pub non_interactive: bool,
    /// Staged (not yet applied) edits from the surgical editing tools,
    /// shared with the executor; committed via `/apply`, dropped by
    /// `/reject`.
    pub pending_edits: Arc<Mutex<PendingEdits>>,
    /// Current "focus" file shown in the prompt breadcrumb: the last
    /// workspace file the user mentioned or the agent edited.
    pub focus_file: Option<String>,
}

#[derive(Default)]
pub struct Stats {
    pub started: Option<Instant>,
    pub turns: u32,
    pub errors: u32,
    pub total_latency_ms: u128,
}

impl Stats {
    pub fn start() -> Self {
        Self {
            started: Some(Instant::now()),
            ..Default::default()
        }
    }
}

impl App {
    pub fn new(
        config: Config,
        http: reqwest::Client,
        session: Session,
        renderer: Renderer,
    ) -> Self {
        let builtin = BuiltinTools::new(config.shell_tools.clone());
        let tool_specs = builtin.specs();
        let pending_edits = builtin.pending_edits();
        let tool_executor: Option<Arc<dyn ToolExecutor>> = Some(Arc::new(builtin));
        Self {
            read_timeout: Duration::from_secs(config.timeout_secs),
            max_response_bytes: (config.limit_mb as usize) * 1024 * 1024,
            models_cache: None,
            session_name: None,
            stats: Stats::start(),
            pending_variants: Vec::new(),
            tool_executor,
            tools_enabled: true,
            disabled_tools: crate::tools::load_disabled_tools(),
            tool_specs,
            todos: todo::load(),
            non_interactive: false,
            pending_edits,
            focus_file: None,
            config,
            http,
            session,
            renderer,
        }
    }

    pub fn record_turn(&mut self, latency: Duration) {
        self.stats.turns += 1;
        self.stats.total_latency_ms += latency.as_millis();
    }

    pub fn record_error(&mut self) {
        self.stats.errors += 1;
    }
}

/// Persists the session todo list; used by `/plan` execution in main.
pub fn persist_todos(app: &mut App) {
    todo::save(app);
}

/// Replaces the session todo list and persists it (TUI plan tracking).
pub fn set_todos(app: &mut App, texts: &[String]) {
    app.todos = texts
        .iter()
        .map(|text| Todo {
            text: text.clone(),
            done: false,
        })
        .collect();
    persist_todos(app);
}

pub enum Outcome {
    Handled,
    Exit,
    /// Send this text as a fresh user turn (powers `/retry`).
    Resend(String),
    /// A confirmed plan awaiting autonomous execution (powers `/plan`).
    Plan(Vec<String>),
}

impl std::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Outcome::Handled => write!(f, "Handled"),
            Outcome::Exit => write!(f, "Exit"),
            Outcome::Resend(t) => write!(f, "Resend({t:?})"),
            Outcome::Plan(steps) => write!(f, "Plan({} steps)", steps.len()),
        }
    }
}

/// Splits a `/command rest` line; the command is lowercased so `/HELP`,
/// `/Help` and `/help` all work. Returns `None` for non-commands and bare
/// commands without arguments.
fn split_command(line: &str) -> Option<(String, &str)> {
    if !line.starts_with('/') {
        return None;
    }
    let (cmd, rest) = line.split_once(char::is_whitespace)?;
    Some((cmd.to_ascii_lowercase(), rest.trim()))
}

pub async fn dispatch(line: &str, app: &mut App) -> Outcome {
    let (cmd, rest) = match split_command(line) {
        Some(pair) => pair,
        None => (line.trim().to_ascii_lowercase(), ""),
    };
    // Any real turn invalidates un-picked variants.
    if !matches!(cmd.as_str(), "/pick" | "/variants") && !app.pending_variants.is_empty() {
        app.pending_variants.clear();
    }
    match cmd.as_str() {
        "/help" | "/?" => {
            help(app);
            Outcome::Handled
        }
        "/exit" | "/quit" => Outcome::Exit,
        "/clear" | "/reset" => {
            app.session.clear();
            dim("conversation cleared.");
            Outcome::Handled
        }
        "/models" => {
            models(app).await;
            Outcome::Handled
        }
        "/model" => {
            set_model(rest, app).await;
            Outcome::Handled
        }
        "/temp" => {
            set_temperature(rest, app);
            Outcome::Handled
        }
        "/system" => {
            set_or_show_system(rest, app);
            Outcome::Handled
        }
        "/history" => {
            print_history(app);
            Outcome::Handled
        }
        "/undo" => {
            if app.session.undo() {
                dim("removed last exchange.");
            } else {
                dim("nothing to undo.");
            }
            Outcome::Handled
        }
        "/retry" => match retry(app) {
            Some(text) => {
                dim("regenerating…");
                Outcome::Resend(text)
            }
            None => {
                dim("nothing to retry yet.");
                Outcome::Handled
            }
        },
        "/save" => {
            save_session(rest, app);
            Outcome::Handled
        }
        "/load" => {
            load_session(rest, app);
            Outcome::Handled
        }
        "/sessions" => {
            list_named_sessions(app);
            Outcome::Handled
        }
        "/fork" => {
            fork_session(rest, app);
            Outcome::Handled
        }
        "/tokens" => {
            println!(
                "~{} tokens · context budget {} tokens (real BPE count) · {} messages · provider {}",
                app.session.approx_tokens(),
                app.config.context_tokens,
                app.session.messages().len(),
                app.config.provider.id(),
            );
            Outcome::Handled
        }
        "/raw" => {
            let next = !app.renderer.markdown_enabled();
            app.renderer.set_markdown(next);
            dim(if next {
                "markdown rendering on."
            } else {
                "raw streaming output."
            });
            Outcome::Handled
        }
        "/theme" => {
            set_or_show_theme(rest);
            Outcome::Handled
        }
        "/export" => {
            export(rest, app);
            Outcome::Handled
        }
        "/stats" => {
            show_stats(app);
            Outcome::Handled
        }
        "/search" => {
            search_history(rest, app);
            Outcome::Handled
        }
        "/compact" => {
            compact(app).await;
            Outcome::Handled
        }
        "/variants" => {
            generate_variants(rest, app).await;
            Outcome::Handled
        }
        "/pick" => {
            pick_variant(rest, app);
            Outcome::Handled
        }
        "/timeout" => {
            set_timeout(rest, app);
            Outcome::Handled
        }
        "/tools" => {
            show_tools(rest, app);
            Outcome::Handled
        }
        "/todo" => {
            todo::handle(rest, app);
            Outcome::Handled
        }
        "/diff" => {
            view_diff(app);
            Outcome::Handled
        }
        "/apply" => {
            apply_edits(app);
            Outcome::Handled
        }
        "/reject" => {
            reject_edits(app);
            Outcome::Handled
        }
        "/review" => {
            review_edits(app);
            Outcome::Handled
        }
        "/limit" => {
            set_limit(rest, app);
            Outcome::Handled
        }
        "/scan" => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let symbols = crate::symbols::rebuild(&cwd);
            ok(&format!("workspace scanned · {symbols} symbols indexed"));
            dim("overview:");
            println!("{}", crate::scan::scan(&cwd).await);
            // Remember the scanned HEAD so future runs can detect a stale index.
            match crate::git::run_git(&cwd, &["rev-parse", "HEAD"]).await {
                Ok(head) => {
                    let hash = head.trim();
                    match crate::project::record_scan_commit(&cwd, hash) {
                        Ok(()) => dim(&format!(
                            "recorded scan at commit {hash} in .govinda_project.json"
                        )),
                        Err(e) => err(&format!("could not update project memory: {e:#}")),
                    }
                }
                Err(_) => dim("not a git repository — scan commit not recorded."),
            }
            Outcome::Handled
        }
        "/plan" => plan::handle(rest, app).await,
        "/project" => {
            project::handle(rest);
            Outcome::Handled
        }
        "/config" => {
            if rest.trim().eq_ignore_ascii_case("save") {
                match persistence::save_runtime_config(app) {
                    Ok(path) => ok(&format!("settings saved to {}", path.display())),
                    Err(e) => err(&format!("config save failed: {e:#}")),
                }
            } else {
                show_config(app);
                dim("use '/config save' to persist model/theme/timeout/limit settings.");
            }
            Outcome::Handled
        }
        unknown => {
            err(&format!("unknown command '{unknown}' — type /help"));
            Outcome::Handled
        }
    }
}

fn help(app: &App) {
    println!(
        "{}",
        paint(
            format!(
                "{} v{} — OpenAI-compatible CLI chatbot ({})",
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
                app.config.provider.id()
            ),
            accent()
        )
    );
    println!(
        "  /help              this help\n\
         \x20 /exit, /quit       quit\n\
         \x20 /clear, /reset     wipe conversation history\n\
         \x20 /models            list models available to your key\n\
         \x20 /model <name>      switch model; `next`/`prev` cycle, partial ids match (current: {})\n\
         \x20 /temp <0.0-1.0>    sampling temperature (current: {:.2})\n\
         \x20 /system [text]     view or set the system prompt\n\
         \x20 /history           print the conversation so far\n\
         \x20 /undo              remove the last exchange\n\
         \x20 /retry             regenerate the last answer\n\
         \x20 /variants [n]      generate n alternate answers concurrently\n\
         \x20 /pick <n>          commit variant n as the answer\n\
         \x20 /compact           fold history into one summary turn to free context\n\
         \x20 /search <text>     find text anywhere in the conversation\n\
         \x20 /save [name]       save conversation to JSON (sessions/)\n\
         \x20 /load <name>       load a saved conversation\n\
         \x20 /sessions          list saved sessions (resume with --resume <name>)\n\
         \x20 /fork [file]       snapshot the conversation without leaving it\n\
         \x20 /export md|txt     export conversation as Markdown or plain text\n\
         \x20 /stats             session statistics (turns, latency, errors)\n\
         \x20 /theme <name>      color theme ({})\n\
         \x20 /tokens            token usage vs the {}-token context budget\n\
         \x20 /raw               toggle markdown rendering vs live streaming\n\
         \x20 /timeout <secs>    per-request read-stall timeout (current: {}s)\n\
         \x20 /limit <mb>        response size cap in MB (current: {})\n\
          \x20 /tools [on|off]    toggle function calling, or list the registry (currently {})\n\
          \x20 /tools en|dis <n>  enable/disable a single tool (persisted across runs)\n\
           \x20 /todo [sub]        task list: list | add <text> | done <n> | undo <n> | rm <n> | clear\n\
           \x20 /diff              show staged edits as a unified diff (nothing applied yet)\n\
         \x20 /apply             commit all staged edits to disk (atomic batch)\n\
         \x20 /reject            discard all staged edits\n\
         \x20 /review            per-file +N/-M summary of staged edits\n\
         \x20 /scan              rebuild the symbol index and print a workspace overview\n\
         \x20 /plan <task>       decompose a task into steps, confirm, execute autonomously\n\
         \x20 /project [sub]     project memory: show | set test|build <cmd> | clear test|build\n\
         \x20 /config [save]     show settings; `save` persists model/theme/timeout/limit",
        app.config.model,
        app.config.temperature,
        theme_names().collect::<Vec<_>>().join(", "),
        app.config.context_tokens,
        app.read_timeout.as_secs(),
        app.max_response_bytes / (1024 * 1024),
        if app.tools_enabled { "on" } else { "off" },
    );
}

fn ok(msg: &str) {
    println!("{}", paint(msg.to_owned(), ok_color()));
}

fn dim(msg: &str) {
    println!("{}", paint(msg.to_owned(), dim_color()));
}

fn err(msg: &str) {
    eprintln!("{}", paint(msg.to_owned(), err_color()));
}

#[cfg(test)]
mod tests {
    use super::{App, Outcome, dispatch, split_command};
    use crate::commands::display::parse_temperature;
    use crate::commands::persistence::safe_session_path;
    use crate::config::Config;
    use crate::provider;
    use crate::render::Renderer;
    use crate::session::Session;
    use std::path::PathBuf;
    use std::sync::Arc;
    use zeroize::Zeroizing;

    pub(crate) fn smoke_app() -> App {
        let provider = provider::resolve("ollama", None, None, |_| None).expect("ollama preset");
        let config = Config {
            api_key: Arc::new(Zeroizing::new(String::new())),
            model: "test-model".to_owned(),
            temperature: 0.5,
            render_markdown: false,
            system_prompt: "sys".to_owned(),
            context_tokens: 2048,
            provider,
            source_path: None,
            shell_tools: Vec::new(),
            theme: None,
            timeout_secs: 30,
            limit_mb: 16,
        };
        App::new(
            config,
            reqwest::Client::new(),
            Session::new("sys"),
            Renderer::new(false),
        )
    }

    #[tokio::test]
    async fn every_slash_command_dispatches_without_panic() {
        // Fresh session keeps the network paths on early-return branches
        // (/compact, /variants); /models and /model fail fast against an
        // unstarted local Ollama and degrade gracefully.
        let cases = [
            "/help",
            "/?",
            "/clear",
            "/history",
            "/tokens",
            "/stats",
            "/config",
            "/system",
            "/system new prompt",
            "/temp 0.3",
            "/timeout 30",
            "/limit 8",
            "/raw",
            "/theme mono",
            "/theme nord",
            "/tools",
            "/tools off",
            "/tools on",
            "/todo",
            "/diff",
            "/apply",
            "/reject",
            "/review",
            "/project",
            "/project set test cargo test",
            "/project clear test",
            "/todo add write more tests",
            "/todo list",
            "/search hi",
            "/sessions",
            "/undo",
            "/retry",
            "/pick",
            "/fork",
            "/save dispatch-smoke",
            "/load definitely-missing-session",
            "/export md",
        ];
        for cmd in cases {
            let outcome = dispatch(cmd, &mut smoke_app()).await;
            assert!(matches!(outcome, Outcome::Handled), "{cmd} -> {outcome:?}");
        }
        let _ = std::fs::remove_file("sessions/dispatch-smoke.json");
        let _ = std::fs::remove_file(".govinda_project.json");
    }

    #[tokio::test]
    async fn commands_are_case_insensitive() {
        let mut app = smoke_app();
        for cmd in ["/HELP", "/Help", "/STATS", "/Tokens", "/CLEAR"] {
            let outcome = dispatch(cmd, &mut app).await;
            assert!(matches!(outcome, Outcome::Handled), "{cmd} -> {outcome:?}");
        }
        assert!(matches!(dispatch("/EXIT", &mut app).await, Outcome::Exit));
    }

    #[test]
    fn split_command_lowercases_and_splits() {
        assert_eq!(
            split_command("/HELP  write tests "),
            Some(("/help".to_owned(), "write tests"))
        );
        assert_eq!(split_command("/exit"), None);
        assert_eq!(split_command("hello world"), None);
    }

    #[test]
    fn temperature_parses_and_clamps_range() {
        assert_eq!(parse_temperature("0"), Some(0.0));
        assert_eq!(parse_temperature(" 0.5 "), Some(0.5));
        assert_eq!(parse_temperature("1"), Some(1.0));
        assert!(parse_temperature("-0.1").is_none());
        assert!(parse_temperature("1.01").is_none());
        assert!(parse_temperature("hot").is_none());
        assert!(parse_temperature("").is_none());
    }

    #[test]
    fn safe_paths_stay_under_sessions() {
        assert_eq!(
            safe_session_path("chat.json").unwrap(),
            PathBuf::from("sessions/chat.json")
        );
        assert_eq!(
            safe_session_path("a/b/chat.json").unwrap(),
            PathBuf::from("sessions/a/b/chat.json")
        );
    }

    #[test]
    fn unsafe_paths_are_rejected() {
        assert!(safe_session_path("/etc/passwd").is_err());
        assert!(safe_session_path("../secrets.json").is_err());
        assert!(safe_session_path("a/../../x.json").is_err());
        assert!(safe_session_path(r"C:\tmp\x.json").is_err());
    }
}
