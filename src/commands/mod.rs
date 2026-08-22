mod display;
mod generation;
mod persistence;

use crate::api;
use crate::config::Config;
use crate::render::{Renderer, accent, dim_color, err_color, ok_color, paint, theme_names};
use crate::session::Session;
use crate::tools::{BuiltinTools, ToolExecutor};
use display::{
    print_history, search_history, set_limit, set_or_show_system, set_or_show_theme,
    set_temperature, set_timeout, show_config, show_stats,
};
use generation::{compact, generate_variants, models, pick_variant, retry, set_model};
use persistence::{export, fork_session, list_named_sessions, load_session, save_session};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    pub tool_executor: Option<Box<dyn ToolExecutor>>,
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
        Self {
            read_timeout: api::default_read_timeout(),
            max_response_bytes: api::MAX_RESPONSE_BYTES,
            models_cache: None,
            session_name: None,
            stats: Stats::start(),
            pending_variants: Vec::new(),
            tool_executor: Some(Box::new(BuiltinTools)),
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

pub enum Outcome {
    Handled,
    Exit,
    /// Send this text as a fresh user turn (powers `/retry`).
    Resend(String),
}

pub async fn dispatch(line: &str, app: &mut App) -> Outcome {
    let (cmd, rest) = match line.split_once(char::is_whitespace) {
        Some((c, r)) => (c, r.trim()),
        None => (line, ""),
    };
    // Any real turn invalidates un-picked variants.
    if !matches!(cmd, "/pick" | "/variants") && !app.pending_variants.is_empty() {
        app.pending_variants.clear();
    }
    match cmd {
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
        "/limit" => {
            set_limit(rest, app);
            Outcome::Handled
        }
        "/config" => {
            show_config(app);
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
         \x20 /config            show current settings",
        app.config.model,
        app.config.temperature,
        theme_names().collect::<Vec<_>>().join(", "),
        app.config.context_tokens,
        app.read_timeout.as_secs(),
        app.max_response_bytes / (1024 * 1024),
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
    use crate::commands::display::parse_temperature;
    use crate::commands::persistence::safe_session_path;
    use std::path::PathBuf;

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
