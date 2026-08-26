mod display;
mod edits;
mod folder;
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
pub const SLASH_COMMANDS: [&str; 55] = [
    "/help",
    "/exit",
    "/quit",
    "/clear",
    "/reset",
    "/agent",
    "/provider",
    "/opencode",
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
    "/cd",
    "/cwd",
    "/folder",
    "/open",
    "/apikey",
    "/setup",
    "/test",
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
        "/provider" => {
            let arg = rest.trim();
            if arg.is_empty() {
                // List presets and show the active provider.
                info(format!(
                    "current provider: {} ({}){}",
                    app.config.provider.key(),
                    app.config.provider.base_url(),
                    if app.config.provider.auth().token().is_some() {
                        " · API key loaded"
                    } else {
                        " · no API key (local/custom)"
                    },
                ));
                info(format!(
                    "available: {}",
                    crate::provider::preset_names().collect::<Vec<_>>().join(", ")
                ));
                dim("switch with /provider <name>, or a custom endpoint with /provider <name> <base-url>");
                dim("for interactive setup, use: /provider <name> -i");
                return Outcome::Handled;
            }
            
            // Check for interactive mode flag
            let (arg_part, interactive) = if arg.ends_with(" -i") || arg.ends_with(" --interactive") {
                (arg.trim_end_matches(" -i").trim_end_matches(" --interactive").trim(), true)
            } else {
                (arg, false)
            };
            
            let (name, base_url) = match arg_part.split_once(char::is_whitespace) {
                Some((n, u)) => (n.trim(), Some(u.trim())),
                None => (arg_part, None),
            };
            
            if interactive {
                // Interactive setup workflow
                if let Err(e) = interactive_provider_setup(name, base_url, app).await {
                    err(format!("setup failed: {e:#}"));
                }
            } else {
                // Direct switch (original behavior)
                let key_env_lookup = |var: &str| std::env::var(var).ok();
                match crate::provider::resolve(name, base_url, None, key_env_lookup) {
                    Ok(new_provider) => {
                        app.config.provider = new_provider;
                        // Cached model ids belong to the old backend.
                        app.models_cache = None;
                        ok(format!(
                            "provider switched to {} ({}){}",
                            app.config.provider.key(),
                            app.config.provider.chat_url(),
                            if app.config.provider.auth().token().is_some() {
                                " · API key loaded"
                            } else {
                                ""
                            },
                        ));
                        dim("pick a model for this provider with /model <name> (or /models to list). Persist the switch with /config save.");
                        dim("for interactive setup, use: /provider <name> -i");
                    }
                    Err(e) => err(format!("{e:#}")),
                }
            }
            Outcome::Handled
        }
        "/opencode" => {
            let arg = rest.trim();
            let (sub, target) = match arg.split_once(char::is_whitespace) {
                Some((s, t)) => (s, t.trim()),
                None => (arg, ""),
            };
            const USAGE: &str = "usage: /opencode [status|connect|models|disconnect] [provider]";
            match sub {
                "status" | "" => {
                    info(format!(
                        "opencode: {}",
                        crate::opencode::status_line(&app.http).await
                    ));
                    if app
                        .config
                        .provider
                        .key()
                        .starts_with(crate::opencode::KEY_PREFIX)
                    {
                        ok(format!(
                            "active backend borrowed from OpenCode · model {}",
                            app.config.model
                        ));
                    } else {
                        dim("not currently borrowing from OpenCode — try '/opencode connect'");
                    }
                }
                "connect" => {
                    let requested = target.split_whitespace().next().map(str::to_owned);
                    match crate::opencode::connect(&app.http, requested.as_deref()).await {
                        Ok((provider, model, summary)) => {
                            app.config.provider = provider;
                            app.config.model = model;
                            // Cached model ids belong to the old backend.
                            app.models_cache = None;
                            ok(summary);
                            dim("list models with /models · persist with /config save");
                        }
                        Err(e) => err(format!("{e:#}")),
                    }
                }
                "models" => match crate::opencode::fetch_catalog(&app.http).await {
                    Ok(catalog) if !catalog.is_empty() => {
                        for entry in &catalog.entries {
                            info(format!("{} ({})", entry.pid, entry.base_url));
                            for model in &entry.models {
                                dim(format!("  {model}"));
                            }
                        }
                        match catalog.default {
                            Some((pid, mid)) => {
                                dim(format!("default: {pid}/{mid}"));
                            }
                            None => {}
                        }
                    }
                    Ok(_) => err(
                        "OpenCode is reachable but no compatible providers are connected",
                    ),
                    Err(e) => err(format!("{e:#}")),
                },
                "disconnect" => {
                    dim("switch away with /provider <name> (e.g. /provider mistral)");
                }
                _ => err(USAGE),
            }
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
                app.config.provider.key(),
            ));
            Outcome::Handled
        }
        "/context" => {
            let used = app.session.approx_tokens();
            let budget = app.config.context_tokens;
            let provider_key = app.config.provider.key();
            let provider_id: &str = provider_key.as_ref();
            let model: &str = app.config.model.as_str();
            let registry_window = crate::provider::context_window_for(provider_id, model);
            let headroom = budget.saturating_sub(used);
            let pct = if budget > 0 { (used * 100) / budget } else { 0 };
            info(format!(
                "model: {}\nprovider: {}\nused: ~{} tokens ({pct}% of budget)\nbudget: {} tokens (CLI trim cap)\nheadroom: ~{} tokens\nmodel limit: {} tokens{}\nset budget in config.toml: `context_tokens = <N>`",
                model,
                provider_id,
                used,
                budget,
                headroom,
                if registry_window > 0 { registry_window } else { budget },
                if registry_window == 0 { " (registry unknown — set explicitly in TOML)" } else { " (from registry)" },
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
            // Agent mode = function calling on/off. One source of truth on
            // App so the model really gains/loses tools (the TUI additionally
            // switches its NORMAL/AGENT badge when it sees the change).
            match rest.trim() {
                "on" => {
                    app.tools_enabled = true;
                    ok("agent mode ON — function calling enabled.");
                }
                "off" => {
                    app.tools_enabled = false;
                    ok("agent mode OFF — function calling disabled.");
                }
                "" => info(format!(
                    "agent mode {} (function calling {})",
                    if app.tools_enabled { "ON" } else { "OFF" },
                    if app.tools_enabled { "enabled" } else { "disabled" },
                )),
                other => info(format!(
                    "usage: /agent <on|off> (unknown arg '{other}')"
                )),
            }
            Outcome::Handled
        }
        "/pin" => {
            dim("pinning files to context is a TUI feature — open the explorer (Ctrl+P), select a file, /pin. In the REPL, @-mention files in your prompt instead.");
            Outcome::Handled
        }
        "/cd" | "/cwd" | "/folder" | "/open" => {
            return folder::handle(rest, app).await;
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
        "/apikey" => {
            let key = rest.trim();
            if key.is_empty() {
                let provider = app.config.provider.key().to_string();
                let has_key = app.config.provider.auth().token().is_some();
                info(format!(
                    "provider: {provider} — API key: {}",
                    if has_key { "loaded" } else { "not set" },
                ));
                dim(format!(
                    "set with: /apikey <key>\nthen /config save to persist, or set {provider}_API_KEY in .env"
                ));
            } else {
                // Re-resolve the current provider with the new key.
                let name = app.config.provider.key().to_string();
                if name.starts_with(crate::opencode::KEY_PREFIX) {
                    err(
                        "OpenCode-backed providers use credentials from OpenCode's own store — re-authenticate inside OpenCode, or /provider <name> to switch first",
                    );
                    return Outcome::Handled;
                }
                let base_url = Some(app.config.provider.base_url().to_owned());
                let key_owned = key.to_owned();
                let key_env_lookup = |_: &str| Some(key_owned.clone());
                match crate::provider::resolve(&name, base_url.as_deref(), None, key_env_lookup) {
                    Ok(new_provider) => {
                        app.config.provider = new_provider;
                        app.config.api_key = std::sync::Arc::new(zeroize::Zeroizing::new(key.to_owned()));
                        ok(format!("API key set for {name} — model calls will authenticate now."));
                        dim("use /config save to persist, or /models to list available models.");
                    }
                    Err(e) => err(format!("could not set API key: {e:#}")),
                }
            }
            Outcome::Handled
        }
        "/test" => {
            test_provider(app).await;
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

/// Test the current provider and model configuration
async fn test_provider(app: &mut App) {
    info("Testing provider configuration...");
    
    let provider_id = app.config.provider.key();
    let model = &app.config.model;
    let has_key = app.config.provider.auth().token().is_some();
    
    info(format!("Provider: {}", provider_id));
    info(format!("Model: {}", model));
    info(format!("API Key: {}", if has_key { "loaded" } else { "not set" }));
    
    if !has_key && provider_id != "ollama" {
        err("API key is required for this provider");
        dim(format!("Set {}_API_KEY in .env or use /apikey <key>", provider_id.to_uppercase()));
        return;
    }
    
    dim("Sending test request...");
    
    let test_message = "Hello! Please respond with 'Test successful' if you can read this.";
    let ctx = vec![
        crate::api::Message::system("You are a helpful assistant. Keep responses brief."),
        crate::api::Message::user(test_message),
    ];
    
    let auth = app.config.provider.auth();
    let opts = crate::api::ChatOptions {
        max_response_bytes: app.max_response_bytes,
        read_timeout: app.read_timeout,
        ..crate::api::ChatOptions::new(auth.token(), model, app.config.temperature)
    };
    
    let mut out = String::new();
    let mut no_calls = Vec::new();
    let mut sink = crate::api::StreamSink::new(&mut out, &mut no_calls);
    
    match crate::api::stream_chat(
        &app.http,
        app.config.provider.as_ref(),
        &opts,
        &ctx,
        &mut sink,
        |_| {},
    )
    .await
    {
        Ok(()) if !out.trim().is_empty() => {
            ok("Test successful!");
            info(format!("Response: {}", out.trim()));
            dim("Your provider and model are working correctly.");
        }
        Ok(()) => {
            err("Test failed: empty response from API");
            dim("The API returned an empty response. Check your model name.");
        }
        Err(e) => {
            err(format!("Test failed: {e:#}"));
            dim("Possible issues:");
            dim("- Invalid API key");
            dim("- Incorrect model name");
            dim("- Network connectivity problems");
            dim("- Provider service outage");
            dim(format!("Try /models to see available models for {}", provider_id));
        }
    }
}

/// Interactive provider setup workflow: select provider -> API key -> model -> test -> load
async fn interactive_provider_setup(
    name: &str,
    base_url_override: Option<&str>,
    app: &mut App,
) -> anyhow::Result<()> {
    // Step 1: Select provider
    let provider_name = if name.is_empty() {
        info("Available providers:");
        for preset in crate::provider::PRESETS.iter() {
            info(format!("  - {}", preset.id));
        }
        dim("Enter provider name:");
        return Err(anyhow::anyhow!("Provider name required. Use: /provider <name> -i"));
    } else {
        name
    };
    
    info(format!("Setting up provider: {}", provider_name));
    
    // Step 2: Check if API key is needed
    let preset = crate::provider::PRESETS.iter().find(|p| p.id == provider_name);
    
    match preset.and_then(|p| p.api_key_env) {
        Some(env_var) => {
            let current_key = std::env::var(env_var).ok();
            
            if current_key.is_none() || current_key.as_ref().map_or(true, |k| k.trim().is_empty()) {
                info(format!("API key required for {}", provider_name));
                info(format!("Environment variable: {}", env_var));
                dim("Options:");
                dim(format!("1. Set {} in your .env file", env_var));
                dim(format!("2. Export it: export {}=your_key", env_var));
                dim(format!("3. Use /apikey <key> to set it interactively"));
                return Err(anyhow::anyhow!("API key not set. Please set {} and try again.", env_var));
            } else {
                ok(format!("API key found in {}", env_var));
            }
        }
        None => {
            ok("No API key required for this provider");
        }
    }
    
    // Step 3: Resolve provider
    let key_env_lookup = |var: &str| std::env::var(var).ok();
    let new_provider = crate::provider::resolve(provider_name, base_url_override, None, key_env_lookup)?;
    app.config.provider = new_provider.clone();
    app.models_cache = None;
    
    ok(format!("Provider configured: {} ({})", new_provider.key(), new_provider.base_url()));
    
    // Step 4: List available models
    info("Fetching available models...");
    let provider_id = new_provider.key().to_string();
    let known_models = crate::provider::known_models(&provider_id);
    
    if !known_models.is_empty() {
        info(format!("Known models for {}:", provider_id));
        for (i, model) in known_models.iter().enumerate() {
            let tag = if model.free { " [FREE]" } else { "" };
            info(format!("  {}. {}{} - {}", i + 1, model.id, tag, model.description));
        }
    }
    
    // Try to fetch live models
    if let Some(models_url) = new_provider.models_url() {
        match crate::api::list_models(&app.http, &models_url, new_provider.auth().token()).await {
            Ok(live_models) => {
                info(format!("Live models available: {}", live_models.len()));
                for (i, model) in live_models.iter().take(10).enumerate() {
                    info(format!("  {}. {}", i + 1, model));
                }
                if live_models.len() > 10 {
                    dim(format!("... and {} more", live_models.len() - 10));
                }
            }
            Err(e) => {
                dim(format!("Could not fetch live models: {e:#}"));
                dim("Using known models from registry");
            }
        }
    }
    
    // Step 5: Prompt for model selection
    info("Step 5: Select a model");
    dim("Enter model name or use first known model as default");
    
    // Auto-select first known model if available
    if let Some(first_model) = known_models.first() {
        app.config.model = first_model.id.to_string();
        ok(format!("Auto-selected model: {} - {}", first_model.id, first_model.description));
        dim("You can change this with /model <name>");
    } else {
        dim("No known models available. Please specify a model with /model <name>");
    }
    
    // Step 6: Test the configuration
    info("Step 6: Testing configuration...");
    dim("Run /test to verify your API key and model are working");
    
    ok("Provider setup complete!");
    dim("Use /config save to persist these settings");
    dim("Use /models to see all available models");
    dim("Use /model <name> to change the selected model");
    
    Ok(())
}

fn help(app: &App) {
    info(format!(
        "{} v{} — OpenAI-compatible CLI chatbot ({})",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        app.config.provider.key()
    ));
    info(
        "  /help              this help\n\
         \x20 /exit, /quit       quit\n\
         \x20 /clear, /reset     wipe conversation history\n\
         \x20 /provider [name]   list or switch AI provider (add -i for interactive setup)\n\
         \x20 /apikey [key]      view or set API key for current provider\n\
         \x20 /test              test current provider and model configuration\n\
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
          \x20 /cd, /open <path>  change folder — open workspace (Ctrl+O)\n\
         \x20 /project [sub]     project memory: show | set test|build <cmd> | clear test|build\n\
         \x20 /config [save]     show settings; `save` persists model/theme/timeout/limit",
    );
    dim("");
    dim("Provider setup workflow:");
    dim("  1. /provider <name> -i    # Interactive setup for a provider");
    dim("  2. /apikey <key>           # Set API key if needed");
    dim("  3. /models                 # List available models");
    dim("  4. /model <name>           # Select a model");
    dim("  5. /test                   # Test the configuration");
    dim("  6. /config save            # Persist settings");
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
    use super::{App, Outcome, dispatch, dispatch_structured, split_command};
    use crate::commands::display::parse_temperature;
    use crate::commands::persistence::safe_session_path;
    use crate::config::Config;
    use crate::provider;
    use crate::render::Renderer;
    use crate::session::Session;
    use std::path::PathBuf;
    use std::sync::Arc;
    use zeroize::Zeroizing;

    /// `/provider` lists presets, switches between them, and accepts custom
    /// OpenAI-compatible endpoints. Switching invalidates the model cache.
    #[tokio::test]
    async fn provider_command_lists_and_switches_presets() {
        let mut app = smoke_app();
        assert_eq!(app.config.provider.id(), "ollama");

        // Bare command lists presets + current provider.
        let out = dispatch_structured("/provider", &mut app).await;
        let text = out
            .msgs
            .iter()
            .map(|m| m.text.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("groq"), "{text}");
        assert!(text.contains("current provider: ollama"), "{text}");

        // Switch to a preset with a custom endpoint override.
        let out =
            dispatch_structured("/provider ollama http://10.0.0.5:11434/v1", &mut app).await;
        assert!(
            out.msgs.iter().any(|m| m.text.contains("switched to ollama")),
            "{out:?}"
        );
        assert_eq!(app.config.provider.id(), "ollama");
        assert_eq!(
            app.config.provider.chat_url(),
            "http://10.0.0.5:11434/v1/chat/completions"
        );
    }

    #[tokio::test]
    async fn provider_custom_endpoint_and_missing_key_error() {
        let mut app = smoke_app();

        // Unknown name + base URL = custom OpenAI-compatible endpoint.
        let out =
            dispatch_structured("/provider custom https://llm.corp.internal/v1", &mut app).await;
        assert!(
            out.msgs.iter().any(|m| m.text.contains("switched to custom")),
            "{out:?}"
        );
        assert_eq!(app.config.provider.id(), "custom");
        assert_eq!(
            app.config.provider.base_url(),
            "https://llm.corp.internal/v1"
        );

        // Cloud preset without its key in the environment must be rejected
        // with the variable name — provider stays unchanged.
        if std::env::var_os("GROQ_API_KEY").is_none() {
            let out = dispatch_structured("/provider groq", &mut app).await;
            assert!(
                out.msgs.iter().any(|m| m.role == super::output::Role::Err
                    && m.text.contains("GROQ_API_KEY")),
                "{out:?}"
            );
            assert_ne!(app.config.provider.id(), "groq");
        }
    }

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
            provider_explicit: false,
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
