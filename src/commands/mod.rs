mod display;
mod edits;
mod generation;
pub mod output;
mod persistence;
mod plan;
mod project;
pub mod todo;

use crate::config::Config;
use crate::render::{Renderer, dim_color, err_color, ok_color, paint};
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
use output::{CommandOutput, Effect};
use persistence::{export, fork_session, list_named_sessions, load_session, save_session};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use todo::Todo;

/// Step parser shared with the TUI planner.
pub use plan::parse_steps;
/// Shared pipeline planning (phase-tagged steps) used by `--build`.
pub use plan::{Phase, PipelineStep, generate_pipeline, parse_pipeline_steps};

/// Every slash command the REPL accepts. Drives the reedline completer and
/// shell-completion scripts; keep in sync with `dispatch()` / `help()`.
pub const SLASH_COMMANDS: [&str; 46] = [
    "/help",
    "/exit",
    "/quit",
    "/clear",
    "/reset",
    "/agent",
    "/pin",
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
    "/checkpoint",
    "/rewind",
    "/memory",
    "/skills",
    "/commit",
    "/pr",
    "/auto-compact",
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
    /// `--build` pipeline mode: after the single pipeline confirmation every
    /// gated tool call is approved automatically.
    pub auto_approve: bool,
    /// Reset at the start of each turn, set by any failed tool round
    /// (errored call, declined gate, non-zero exit) — the `--build` loop
    /// uses it to decide whether a step succeeded and whether to grant a
    /// fix attempt.
    pub last_turn_had_failure: bool,
    /// Staged (not yet applied) edits from the surgical editing tools,
    /// shared with the executor; committed via `/apply`, dropped by
    /// `/reject`.
    pub pending_edits: Arc<Mutex<PendingEdits>>,
    /// Current "focus" file shown in the prompt breadcrumb: the last
    /// workspace file the user mentioned or the agent edited.
    pub focus_file: Option<String>,
    /// Session checkpoints for rewind functionality.
    pub checkpoints: crate::checkpoint::CheckpointStore,
    /// Project memory loaded from AGENTS.md / CLAUDE.md / .govinda/memory.md.
    pub project_memory: crate::memory::ProjectMemory,
    /// Loaded skills from ~/.config/govinda/skills/*.md.
    pub skills: Vec<crate::skills::Skill>,
    /// Whether auto-compact is enabled (context-aware session management).
    pub auto_compact_enabled: bool,
    /// Track when the last auto-compact happened (message count).
    pub last_auto_compact_count: usize,
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
        let workspace = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let project_memory = crate::memory::ProjectMemory::load(&workspace);
        let skills = crate::skills::load_skills();
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
            auto_approve: false,
            last_turn_had_failure: false,
            pending_edits,
            focus_file: None,
            checkpoints: crate::checkpoint::CheckpointStore::new(),
            project_memory,
            skills,
            auto_compact_enabled: true,
            last_auto_compact_count: 0,
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

/// Applies all staged edits to disk (`/apply` semantics). Used by the
/// `--build` pipeline after each autonomous step — headless mode has no one
/// to type `/apply`. Returns `true` when edits were written.
pub fn apply_pending_edits(app: &mut App) -> bool {
    edits::apply(app)
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
    /// The last exchange was removed (`/undo`); UIs drop matching entries.
    Undo,
    /// Session history was replaced (`/load`, `/rewind`); UIs rebuild the
    /// transcript from the session.
    Reloaded,
}

impl std::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Outcome::Handled => write!(f, "Handled"),
            Outcome::Exit => write!(f, "Exit"),
            Outcome::Resend(t) => write!(f, "Resend({t:?})"),
            Outcome::Plan(steps) => write!(f, "Plan({} steps)", steps.len()),
            Outcome::Undo => write!(f, "Undo"),
            Outcome::Reloaded => write!(f, "Reloaded"),
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
                Outcome::Undo
            } else {
                dim("nothing to undo.");
                Outcome::Handled
            }
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
            if load_session(rest, app) {
                Outcome::Reloaded
            } else {
                Outcome::Handled
            }
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
            info(format!(
                "~{} tokens · context budget {} tokens (real BPE count) · {} messages · provider {}",
                app.session.approx_tokens(),
                app.config.context_tokens,
                app.session.messages().len(),
                app.config.provider.id(),
            ));
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
            set_or_show_theme(rest, app);
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
        }        "/timeout" => {
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
            ok(format!("workspace scanned · {symbols} symbols indexed"));
            dim("overview:");
            info(crate::scan::scan(&cwd).await);
            // Remember the scanned HEAD so future runs can detect a stale index.
            match crate::git::run_git(&cwd, &["rev-parse", "HEAD"]).await {
                Ok(head) => {
                    let hash = head.trim();
                    match crate::project::record_scan_commit(&cwd, hash) {
                        Ok(()) => dim(format!(
                            "recorded scan at commit {hash} in .govinda_project.json"
                        )),
                        Err(e) => err(format!("could not update project memory: {e:#}")),
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
        "/checkpoint" => {
            let label = rest.trim();
            let label = if label.is_empty() {
                format!("turn {}", app.stats.turns + 1)
            } else {
                label.to_owned()
            };
            let cp = app.checkpoints.checkpoint(&label, app.session.messages()).to_owned();
            let workspace = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            match crate::checkpoint::save_checkpoint(&workspace, &cp) {
                Ok(path) => ok(format!("checkpoint #{} saved ({} msgs) → {}", cp.id, cp.message_count, path.display())),
                Err(e) => ok(format!("checkpoint #{} created ({} msgs) — disk save failed: {e:#}", cp.id, cp.message_count)),
            }
            Outcome::Handled
        }
        "/rewind" => {
            let arg = rest.trim();
            let id = if arg.is_empty() { None } else { arg.parse::<usize>().ok() };
            match app.checkpoints.rewind_to(id.unwrap_or(0)) {
                Some(messages) => {
                    app.session.clear();
                    for m in messages {
                        match m.role.as_str() {
                            "user" => app.session.push_user(m.content),
                            "assistant" => app.session.push_assistant(m.content),
                            "system" => app.session.set_system(m.content),
                            _ => {}
                        }
                    }
                    ok(format!("rewound to checkpoint — {} messages restored", app.session.messages().len()));
                    Outcome::Reloaded
                }
                None => {
                    err("no checkpoint found with that id");
                    Outcome::Handled
                }
            }
        }
        "/memory" => {
            let arg = rest.trim();
            if arg.starts_with("add ") || arg.starts_with("note ") {
                let note = arg.split_once(char::is_whitespace).map_or("", |(_, r)| r);
                let workspace = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                match crate::memory::ProjectMemory::append_note(&workspace, note) {
                    Ok(()) => ok("note added to .govinda/memory.md"),
                    Err(e) => err(format!("failed to add note: {e:#}")),
                }
            } else if app.project_memory.has_content() {
                ok(format!("project memory loaded: {}", app.project_memory.to_system_suffix().unwrap_or_default().len()));
            } else {
                dim("no project memory found — create AGENTS.md, CLAUDE.md, or .govinda/memory.md in the workspace root");
            }
            Outcome::Handled
        }
        "/skills" => {
            if app.skills.is_empty() {
                dim("no skills found — create .md files in ~/.config/govinda/skills/");
            } else {
                ok(format!("{} skill(s) loaded:", app.skills.len()));
                for s in &app.skills {
                    println!("  {} — {}{}", s.name, s.description, if s.requires_args { " (args required)" } else { "" });
                }
            }
            Outcome::Handled
        }
        "/commit" => {
            let msg = rest.trim();
            if msg.is_empty() {
                err("usage: /commit <message>");
                return Outcome::Handled;
            }
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            match crate::git::run_git(&cwd, &["add", "-A"]).await {
                Ok(_) => match crate::git::run_git(&cwd, &["commit", "-m", msg]).await {
                    Ok(out) => ok(format!("committed:\n{out}")),
                    Err(e) => err(format!("commit failed: {e:#}")),
                },
                Err(e) => err(format!("git add failed: {e:#}")),
            }
            Outcome::Handled
        }
        "/pr" => {
            let arg = rest.trim();
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            if arg == "create" || arg.is_empty() {
                // Create a new branch and show instructions
                let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
                let branch = format!("govinda/{timestamp}");
                match crate::git::run_git(&cwd, &["checkout", "-b", &branch]).await {
                    Ok(_) => ok(format!("created branch {branch}\n  → Make changes, then: /commit <message>\n  → Push: run_shell(\"git push -u origin {branch}\")")),
                    Err(e) => err(format!("branch creation failed: {e:#}")),
                }
            } else if arg == "list" {
                match crate::git::run_git(&cwd, &["branch", "--list"]).await {
                    Ok(branches) => ok(format!("branches:\n{branches}")),
                    Err(e) => err(format!("git branch failed: {e:#}")),
                }
            } else {
                // Switch to branch
                match crate::git::run_git(&cwd, &["checkout", arg]).await {
                    Ok(out) => ok(format!("switched to {arg}:\n{out}")),
                    Err(e) => err(format!("branch switch failed: {e:#}")),
                }
            }
            Outcome::Handled
        }
        "/agent" => {
            // TUI-only concept (NORMAL/AGENT mode badge). The REPL is always
            // agent-capable; function calling toggles with /tools.
            dim("mode switching is a TUI feature — run `govinda` (TUI) and use /agent <on|off>. In the REPL, tools toggle with /tools on|off.");
            Outcome::Handled
        }
        "/pin" => {
            dim("pinning files to context is a TUI feature — open the explorer (Ctrl+P), select a file, /pin. In the REPL, @-mention files in your prompt instead.");
            Outcome::Handled
        }
        "/auto-compact" => {
            let arg = rest.trim();
            if arg == "on" {
                app.auto_compact_enabled = true;
                ok("auto-compact enabled — context will be refreshed when nearing the token limit");
            } else if arg == "off" {
                app.auto_compact_enabled = false;
                ok("auto-compact disabled");
            } else {
                let state = if app.auto_compact_enabled { "on" } else { "off" };
                ok(format!("auto-compact is {state} (use: /auto-compact on|off)"));
            }
            Outcome::Handled
        }
        "/config" => {
            if rest.trim().eq_ignore_ascii_case("save") {
                match persistence::save_runtime_config(app) {
                    Ok(path) => ok(format!("settings saved to {}", path.display())),
                    Err(e) => err(format!("config save failed: {e:#}")),
                }
            } else {
                show_config(app);
                dim("use '/config save' to persist model/theme/timeout/limit settings.");
            }
            Outcome::Handled
        }
        unknown => {
            // Custom skills (~/.config/govinda/skills/*.md) execute as plain
            // prompts in BOTH frontends — one implementation here.
            if let Some(skill) = app
                .skills
                .iter()
                .find(|s| s.name.eq_ignore_ascii_case(unknown))
                .cloned()
            {
                let body = if rest.trim().is_empty() && skill.requires_args {
                    format!("{}\n\n{}", skill.body, "(This skill requires arguments)")
                } else if rest.trim().is_empty() {
                    skill.body.clone()
                } else {
                    format!("{}\n\nUser input: {}", skill.body, rest.trim())
                };
                ok(format!("executing skill: {}", skill.name));
                Outcome::Resend(body)
            } else {
                err(format!(
                    "unknown command '{unknown}' — type /help{}",
                    if app.skills.is_empty() {
                        String::new()
                    } else {
                        " or /skills".to_owned()
                    }
                ));
                Outcome::Handled
            }
        }
    }
}

fn help(app: &App) {
    info(format!(
        "{} v{} — OpenAI-compatible CLI chatbot ({})",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        app.config.provider.id()
    ));
    info(
        "  /help              this help\n\
         \x20 /exit, /quit       quit\n\
         \x20 /clear, /reset     wipe conversation history\n\
         \x20 /models            list models available to your key\n\
         \x20 /model <name>      switch model; `next`/`prev` cycle, partial ids match\n\
         \x20 /temp <0.0-1.0>    sampling temperature\n\
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
         \x20 /theme <name>      color theme\n\
         \x20 /tokens            token usage vs the context budget\n\
         \x20 /raw               toggle markdown rendering vs live streaming\n\
         \x20 /timeout <secs>    per-request read-stall timeout\n\
         \x20 /limit <mb>        response size cap in MB\n\
           \x20 /tools [on|off]    toggle function calling, or list the registry\n\
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
    );
}

// ---------------------------------------------------------------------------
// Output helpers — the single print path for every command.
//
// By default they write straight to stdout/stderr (REPL). While a structured
// dispatch is capturing, the same calls are buffered as role-tagged messages
// so the TUI can render them itself. All command code must go through these
// helpers; raw println! inside a handler would corrupt the TUI screen.
// ---------------------------------------------------------------------------

/// Renders one message the REPL way. Used when not capturing.
pub(crate) fn print_msg(role: output::Role, text: &str) {
    match role {
        output::Role::Ok => println!("{}", paint(text.to_owned(), ok_color())),
        output::Role::Err => eprintln!("{}", paint(text.to_owned(), err_color())),
        output::Role::Warn => println!(
            "{}",
            paint(text.to_owned(), crossterm::style::Color::Yellow)
        ),
        output::Role::Dim => println!("{}", paint(text.to_owned(), dim_color())),
        output::Role::Info | output::Role::Markdown => println!("{text}"),
    }
}

fn ok(msg: impl AsRef<str>) {
    output::emit(output::Role::Ok, msg.as_ref());
}

fn dim(msg: impl AsRef<str>) {
    output::emit(output::Role::Dim, msg.as_ref());
}

fn err(msg: impl AsRef<str>) {
    output::emit(output::Role::Err, msg.as_ref());
}

/// Plain informational line (replaces direct `println!` in handlers).
fn info(msg: impl AsRef<str>) {
    output::emit(output::Role::Info, msg.as_ref());
}

/// Markdown that should render as an assistant message (`/pick`).
fn markdown(msg: impl AsRef<str>) {
    output::emit(output::Role::Markdown, msg.as_ref());
}

/// Appended to the system prompt whenever function calling is available:
/// steers the model toward the workspace tools instead of guessing.
pub const AGENT_SYSTEM_ADDENDUM: &str = "\n\nYou are a coding agent working inside the user's project \
workspace. You use edit_file/insert_after/insert_before for changes (staged for review via \
view_diff), run_shell or check_project to verify compilation, find_symbol to locate definitions, \
and never guess line numbers — read files or query the symbol index before editing. \
You CAN run arbitrary shell commands: start dev servers, open files and URLs in the browser, \
launch programs. When the user asks to run, preview, or open something, do it with run_shell \
or open_preview instead of saying you cannot.";

/// Applies agent specialization when tools are on; plain chat keeps the
/// user's configured system prompt untouched. Shared by REPL startup,
/// `--build`, and `/system` so a custom prompt never strips the addendum.
pub fn specialize_system(app: &mut App) {
    if app.tools_enabled {
        let mut specialized = format!("{}{AGENT_SYSTEM_ADDENDUM}", app.session.system());
        // Inject project memory (AGENTS.md / CLAUDE.md / .govinda/memory.md)
        if let Some(suffix) = app.project_memory.to_system_suffix() {
            specialized.push_str(&format!("\n\n{suffix}"));
        }
        // Inject custom skills as available commands
        if !app.skills.is_empty() {
            specialized.push_str("\n\n## Custom Skills\n\nAvailable custom slash commands:\n");
            for s in &app.skills {
                let args_hint = if s.requires_args { " (requires args)" } else { "" };
                specialized.push_str(&format!("- `{}` — {}{}\n", s.name, s.description, args_hint));
            }
        }
        app.session.set_system(specialized);
    }
}

/// Dispatches with capture enabled and returns structured output for
/// non-stdout frontends (the TUI). Theme changes are auto-detected by
/// comparing the active theme name across the dispatch.
pub async fn dispatch_structured(line: &str, app: &mut App) -> CommandOutput {
    let theme_before = crate::render::active_theme().name;
    output::begin_capture();
    let outcome = dispatch(line, app).await;
    let msgs = output::take_captured();

    let effect = match outcome {
        Outcome::Handled => {
            let now = crate::render::active_theme().name;
            if now != theme_before {
                Effect::ThemeChanged(now.to_owned())
            } else {
                Effect::None
            }
        }
        Outcome::Exit => Effect::ExitRequested,
        Outcome::Resend(text) => Effect::Resend(text),
        Outcome::Plan(steps) => Effect::Plan(steps),
        Outcome::Undo => Effect::PopExchange,
        Outcome::Reloaded => Effect::ReloadTranscript,
    };
    CommandOutput { msgs, effect }
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
