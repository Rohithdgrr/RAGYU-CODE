mod display;
mod edits;
mod folder;
mod generation;
pub mod output;
mod persistence;
mod plan;
mod project;
mod router_cmd;
pub mod todo;

use crate::config::Config;
use crate::render::{Renderer, dim_color, err_color, ok_color, paint};
use crate::session::Session;
use crate::tools::{BuiltinTools, PendingEdits, ToolExecutor};
use display::{
    print_history, set_or_show_theme, show_config, show_tools,
};
use generation::{compact, models, retry, set_model};
use output::{CommandOutput, Effect};
use persistence::{load_session, save_session};
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
///
/// Curated to the commands that are wired through and working â€” the old
/// list had 55 entries, many of which were stubbed, broken, or duplicated
/// TUI-only behaviour. Anything removed here is still discoverable via
/// `/help` (which lists a short summary of each).
pub const SLASH_COMMANDS: [&str; 14] = [
    "/help",
    "/exit",
    "/clear",
    "/provider",
    "/model",
    "/models",
    "/router",
    "/theme",
    "/tokens",
    "/todo",
    "/cd",
    "/save",
    "/load",
    "/history",
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
    /// would be pure waste â€” they never change mid-session).
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
    /// (errored call, declined gate, non-zero exit) â€” the `--build` loop
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
    /// Whether the router is allowed to promote on persistent
    /// failures. Defaults to `true`; toggled via
    /// `/router failover on|off`.
    pub router_failover: bool,
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
    /// Builds a minimal `App` for use by tool executors and slash-command
    /// helpers that need access to the same `pending_edits` queue and
    /// `tools_enabled` state. No model call, no session.
    pub fn new_for_test() -> Self {
        use std::sync::Arc;
        use zeroize::Zeroizing;
        let provider = crate::provider::resolve("ollama", None, None, |_| None)
            .expect("ollama preset");
        let config = Config {
            api_key: Arc::new(Zeroizing::new(String::new())),
            model: "test-model".to_owned(),
            temperature: 0.5,
            render_markdown: false,
            system_prompt: String::new(),
            context_tokens: crate::provider::DEFAULT_CONTEXT_TOKENS,
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
            Session::new(""),
            Renderer::new(false),
        )
    }

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
            router_failover: true,
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
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let pending = app.pending_edits.clone();
    let mut guard = pending.lock().unwrap();
    let ops = guard.ops().to_vec();
    if ops.is_empty() {
        return false;
    }
    let result = crate::tools::apply_ops_to_disk(&cwd, &ops);
    if result.is_ok() {
        guard.clear();
        true
    } else {
        false
    }
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
/// `/Help` and `/help` all work. Returns `None` for non-commands. Bare
/// commands without arguments are allowed (rest defaults to "").
fn split_command(line: &str) -> Option<(String, &str)> {
    if !line.starts_with('/') {
        return None;
    }
    let (cmd, rest) = match line.split_once(char::is_whitespace) {
        Some((c, r)) => (c, r.trim()),
        None => (line, ""),
    };
    Some((cmd.to_ascii_lowercase(), rest))
}

pub async fn dispatch(line: &str, app: &mut App) -> Outcome {
    let (cmd, rest) = match split_command(line) {
        Some(parts) => parts,
        None => return Outcome::Handled,
    };
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
            models(rest, app).await;
            Outcome::Handled
        }
        "/provider" => {
            return provider_dispatch(rest, app).await;
        }
        "/model" => {
            set_model(rest, app).await;
            Outcome::Handled
        }
        "/history" => {
            print_history(app);
            Outcome::Handled
        }
        "/retry" => match retry(app) {
            Some(text) => {
                dim("regeneratingâ€¦");
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
        "/tokens" => {
            let used = app.session.approx_tokens();
            let budget = app.config.context_tokens;
            let provider_id: &str = app.config.provider.key().as_ref();
            let model: &str = app.config.model.as_str();
            let registry_window = crate::provider::context_window_for(provider_id, model);
            let headroom = budget.saturating_sub(used);
            let pct = if budget > 0 { (used * 100) / budget } else { 0 };
            // Render the token report as a Markdown message so the TUI
            // shows it in the chat pane (the previous `info()` notice
            // was easy to miss on long transcripts).
            let body = format!(
                "### Token usage\n\
                 \n\
                 - **used**: ~{used} tokens ({pct}% of budget)\n\
                 - **budget**: {budget} tokens (CLI trim cap, `context_tokens` in `config.toml`)\n\
                 - **headroom**: ~{headroom} tokens\n\
                 - **messages**: {}\n\
                 - **model**: `{model}` ({provider_id})\n\
                 - **model limit**: {} tokens{}",
                app.session.messages().len(),
                if registry_window > 0 { registry_window } else { budget },
                if registry_window == 0 {
                    " (registry unknown -- set `context_tokens` in TOML)"
                } else {
                    " (from registry)"
                },
            );
            markdown(body);
            Outcome::Handled
        }
        "/theme" => {
            set_or_show_theme(rest, app);
            Outcome::Handled
        }
        "/compact" => {
            compact(app).await;
            Outcome::Handled
        }
        "/router" => {
            router_cmd::handle(rest, app);
            Outcome::Handled
        }
        "/todo" => {
            todo::handle(rest, app);
            Outcome::Handled
        }
        "/cd" | "/open" => {
            return folder::handle(rest, app).await;
        }
        unknown => {
            // Map removed/broken old commands to the closest live command.
            // Keeps muscle memory working without silently doing nothing.
            match unknown {
                "/opencode" => {
                    dim("`/opencode` was merged into `/provider` â€” try `/provider` to see options, or `/provider oc auto` to borrow OpenCode's connected providers.");
                }
                "/pin" => {
                    dim("`/pin` is a TUI-only feature â€” in the REPL, @-mention files in your prompt instead.");
                }
                "/system" => {
                    dim("`/system` was removed; edit the system prompt in config.toml (`system_prompt = \"â€¦\"`).");
                }
                "/temp" => {
                    dim("`/temp` was removed; set temperature in config.toml (`temperature = 0.3`).");
                }
                "/timeout" => {
                    dim("`/timeout` was removed; set in config.toml (`timeout_secs = 120`).");
                }
                "/limit" => {
                    dim("`/limit` was removed; set in config.toml (`limit_mb = 16`).");
                }
                "/raw" => {
                    dim("`/raw` was removed; markdown rendering is controlled by the TUI/REPL mode.");
                }
                "/search" => {
                    dim("`/search` was removed; use Ctrl+R in the REPL or the history panel in the TUI.");
                }
                "/sessions" | "/fork" | "/export" | "/stats" => {
                    dim(format!("`{unknown}` was removed; saved sessions are still readable from `sessions/` but the slash command was cut."));
                }
                "/diff" | "/apply" | "/reject" | "/review" => {
                    dim("edit staging was removed; the model now writes directly via tools.");
                }
                "/scan" | "/plan" | "/project" | "/checkpoint" | "/rewind" | "/memory" | "/skills" | "/commit" | "/pr" | "/auto-compact" => {
                    dim(format!("`{unknown}` was removed in the cleanup. Use the TUI tools panel or `run_shell` for ad-hoc tasks."));
                }
                "/apikey" => {
                    let provider = app.config.provider.key().to_string();
                    info(format!(
                        "provider: {provider} — set the API key via env ({}_API_KEY=...) or .env, not via slash command.",
                        provider.to_uppercase()
                    ));
                }
                "/undo" => {
                    dim("`/undo` was removed; restart the chat with `/clear` if you want a fresh history.");
                }
                "/context" => {
                    dim("`/context` was merged into `/tokens` — run `/tokens` for the full usage report.");
                }
                "/tools" => {
                    dim("`/tools` was removed; tools are always available to the model unless `/router failover off` pins the active model.");
                }
                "/agent" => {
                    dim("`/agent` was removed; function calling is on by default and toggled via `tools_enabled` in `config.toml`.");
                }
                "/config" => {
                    dim("`/config` was removed; edit `~/.config/govinda/config.toml` directly (TOML is the source of truth).");
                }
                }
                "/test" | "/setup" => {
                    dim("`/test` / `/setup` were removed; run a model call directly to verify the provider.");
                }
                "/variants" | "/pick" => {
                    dim("variants/pick were removed; ask the model to regenerate instead.");
                }
                "/cwd" | "/folder" => {
                    return folder::handle(rest, app).await;
                }
                _ => {
                    // Custom skills (~/.config/govinda/skills/*.md) execute as
                    // plain prompts in BOTH frontends — one implementation
                    // here. Only check skills when the unknown command wasn't
                    // a removed alias.
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
                        return Outcome::Resend(body);
                    } else {
                        err(format!(
                            "unknown command '{unknown}' — type /help{}",
                            if app.skills.is_empty() {
                                String::new()
                            } else {
                                " or /skills".to_owned()
                            }
                        ));
                        return Outcome::Handled;
                    }
                }
            }
            Outcome::Handled
        }
    }
}

/// Builds a minimal [`App`] the `todo` tool can dispatch through.
///
/// The tool only needs the todo list to read/write and the slash dispatcher
/// to format the output. We avoid pulling in HTTP / session state because
/// the tool runs while the user's session is already active.
pub(crate) fn todo_app_for_tools() -> App {
    use std::sync::Arc;
    use zeroize::Zeroizing;
    let provider = crate::provider::resolve("ollama", None, None, |_| None)
        .expect("ollama preset");
    let config = Config {
        api_key: Arc::new(Zeroizing::new(String::new())),
        model: "test-model".to_owned(),
        temperature: 0.5,
        render_markdown: false,
        system_prompt: String::new(),
        context_tokens: crate::provider::DEFAULT_CONTEXT_TOKENS,
        provider,
        source_path: None,
        provider_explicit: false,
        shell_tools: Vec::new(),
        theme: None,
        timeout_secs: 30,
        limit_mb: 16,
    };
    let mut app = App::new(
        config,
        reqwest::Client::new(),
        Session::new(""),
        Renderer::new(false),
    );
    // Load the project's todo list so the tool sees the same state the user does.
    app.todos = todo::load();
    app
}


/// `/provider` handler extracted so the dispatch match stays compact.
async fn provider_dispatch(arg: &str, app: &mut App) -> Outcome {
    if arg.is_empty() {
        info(format!(
            "current provider: {} ({}){}",
            app.config.provider.key(),
            app.config.provider.base_url(),
            if app.config.provider.auth().token().is_some() {
                " Â· API key loaded"
            } else {
                " Â· no API key (local/custom)"
            },
        ));
        info(format!(
            "available: {}",
            crate::provider::preset_names().collect::<Vec<_>>().join(", ")
        ));
        dim("switch with /provider <name>, or a custom endpoint with /provider <name> <base-url>");
        return Outcome::Handled;
    }

    // Check for interactive mode flag
    let (arg_part, _interactive) = if arg.ends_with(" -i") || arg.ends_with(" --interactive") {
        (
            arg.trim_end_matches(" -i")
                .trim_end_matches(" --interactive")
                .trim(),
            true,
        )
    } else {
        (arg, false)
    };

    let (name, base_url) = match arg_part.split_once(char::is_whitespace) {
        Some((n, u)) => (n.trim(), Some(u.trim())),
        None => (arg_part, None),
    };

    // Direct switch.
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
                    " Â· API key loaded"
                } else {
                    ""
                },
            ));
            dim("pick a model for this provider with /model <name> (or /models to list). Persist the switch with /config save.");
        }
        Err(e) => err(format!("{e:#}")),
    }
    Outcome::Handled
}
/// Test the current provider and model configuration
#[allow(dead_code)]
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
#[allow(dead_code)]
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
        "{} v{} â€” OpenAI-compatible CLI chatbot ({})",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        app.config.provider.key()
    ));
    info(
        "  /help              this help\n\
         \x20 /exit, /quit       quit\n\
         \x20 /clear             wipe conversation history\n\
         \x20 /agent [on|off]    toggle function calling (tools)\n\
         \x20 /provider [name]   list or switch AI provider (add -i for interactive setup)\n\
         \x20 /models            list models available to your key\n\
         \x20 /model <name>      switch model; `next`/`prev` cycle, partial ids match\n\
         \x20 /theme <name>      color theme\n\
         \x20 /tokens            token usage vs the context budget\n\
         \x20 /context           detailed token breakdown + model limit\n\
         \x20 /todo [sub]        task list: list | add <text> | done <n> | undo <n> | rm <n> | clear\n\
         \x20 /tools [on|off]    toggle function calling, or list the registry\n\
         \x20 /cd, /open <path>  change folder â€” open workspace (Ctrl+O)\n\
         \x20 /save [name]       save conversation to JSON (sessions/)\n\
         \x20 /load <name>       load a saved conversation\n\
         \x20 /history           print the conversation so far\n\
         \x20 /undo              remove the last exchange\n\
         \x20 /retry             regenerate the last answer\n\
         \x20 /compact           fold history into one summary turn to free context\n\
         \x20 /config [save]     show settings; `save` persists model/theme/timeout/limit",
    );
    dim("");
    dim("Provider setup workflow:");
    dim("  1. /provider <name> -i    # Interactive setup for a provider");
    dim("  2. /apikey <key>           # Set API key if needed");
    dim("  3. /models                 # List available models");
    dim("  4. /model <name>           # Select a model");
    dim("  5. /config save            # Persist settings");
}

// ---------------------------------------------------------------------------
// Output helpers â€” the single print path for every command.
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
#[allow(dead_code)]
fn markdown(msg: impl AsRef<str>) {
    output::emit(output::Role::Markdown, msg.as_ref());
}

/// Appended to the system prompt whenever function calling is available:
/// steers the model toward the workspace tools instead of guessing.
pub const AGENT_SYSTEM_ADDENDUM: &str = "\n\nYou are a coding agent working inside the user's project \
workspace. You use edit_file/insert_after/insert_before for changes (staged for review via \
view_diff), run_shell or check_project to verify compilation, find_symbol to locate definitions, \
and never guess line numbers â€” read files or query the symbol index before editing. \
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
                specialized.push_str(&format!("- `{}` â€” {}{}\n", s.name, s.description, args_hint));
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

/// Returns a human-readable summary of the current environment for the
/// `show_capabilities` tool: cwd, model, provider, enabled tools, project type.
pub fn capabilities_summary() -> anyhow::Result<String> {
    use anyhow::Context;
    let cwd = std::env::current_dir().context("cannot resolve cwd")?;
    let model = std::env::var("GOVINDA_MODEL")
        .ok()
        .or_else(|| Some("test-model".into()))
        .unwrap();
    let provider = std::env::var("GOVINDA_PROVIDER").unwrap_or_else(|_| "unknown".into());
    let enabled_tools: Vec<&str> = vec![
        "current_time", "count_tokens", "read_file", "write_file", "delete_file",
        "move_file", "copy_file", "list_files", "list_directory", "grep",
        "scan_project", "find_symbol", "explain_code", "edit_file", "insert_after",
        "insert_before", "view_diff", "apply_edits", "discard_edits",
        "show_staged_files", "run_shell", "run_test", "run_diagnostics",
        "open_preview", "git_diff", "git_log", "git_branch", "git_commit",
        "web_search", "web_fetch", "ask_user", "delegate_task", "todo",
        "show_token_budget", "show_capabilities", "remember", "forget",
    ];
    Ok(format!(
        "cwd: {}\nmodel: {}\nprovider: {}\nenabled tools ({}):\n  {}\ndisabled tools: (use the TUI's /tools to toggle)",
        cwd.display(),
        model,
        provider,
        enabled_tools.len(),
        enabled_tools.join(", "),
    ))
}

/// Applies the pending edit queue atomically; used by both the `/apply`
/// slash command and the `apply_edits` tool.
pub fn apply_pending(base: &std::path::Path, app: &App) -> anyhow::Result<String> {
    let pending = app.pending_edits.clone();
    let mut guard = pending.lock().unwrap();
    let ops = guard.ops().to_vec();
    if ops.is_empty() {
        return Ok("nothing to apply".to_owned());
    }
    let _ = crate::tools::apply_ops_to_disk(base, &ops)?;
    guard.clear();
    Ok(format!("applied {} edit(s)", ops.len()))
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
        // with the variable name â€” provider stays unchanged.
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
        for cmd in ["/HELP", "/Help", "/Tokens", "/CLEAR"] {
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
        // Bare commands without arguments are now allowed.
        assert_eq!(split_command("/exit"), Some(("/exit".to_owned(), "")));
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
