//! Auto-compact at soft and hard context-window thresholds.
//!
//! The two thresholds are:
//!   * `soft_pct` (default 90) — fold the history into a single
//!     summary by routing the summarizer call to the cheapest healthy
//!     router entry. Keeps the conversation going with a stable
//!     budget headroom.
//!   * `hard_pct` (default 98) — drop everything except the system
//!     prompt and the last four turns. A last-resort measure when
//!     the soft compact is not enough (the summarizer call itself
//!     fails, or two consecutive soft compactions have not reduced
//!     the fill percentage).
//!
//! The summarizer model is selected by the router so the active
//! "smart" model never has to spend its token budget on a routine
//! fold. If the active model is the only healthy entry, we fall
//! back to it.

use crate::commands::App;
use crate::provider;
use crate::router::Router;

pub const SOFT_COMPACT_PCT: u8 = 90;
pub const HARD_COMPACT_PCT: u8 = 98;
/// Number of recent turns the hard reset keeps.
pub const HARD_KEEP_TURNS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Noop,
    SoftCompacted,
    HardReset,
}

#[derive(Debug, Clone, Copy, Default)]
struct State {
    soft_streak: u8,
    last_soft_pct: u8,
}

static STATE: std::sync::Mutex<State> = std::sync::Mutex::new(State {
    soft_streak: 0,
    last_soft_pct: 0,
});

/// Computes the model's true context window for the active provider +
/// model pair, falling back to the configured `context_tokens` when
/// the registry returns 0.
pub fn context_window_for(app: &App) -> usize {
    let provider_key = app.config.provider.key();
    let provider_id: &str = provider_key.as_ref();
    let model: &str = app.config.model.as_str();
    let from_registry = provider::context_window_for(provider_id, model);
    if from_registry == 0 {
        app.config.context_tokens
    } else {
        from_registry
    }
}

/// Returns the current fill percentage (0..=100). Saturates at 100.
pub fn fill_pct(app: &App) -> u8 {
    let window = context_window_for(app);
    if window == 0 {
        return 0;
    }
    let used = app.session.approx_tokens();
    ((used * 100) / window).min(100) as u8
}

/// Decides whether to run a soft compact, a hard reset, or nothing.
pub async fn check_and_run(
    app: &mut App,
    router: &Router,
    soft_pct: u8,
    hard_pct: u8,
) -> Outcome {
    let pct = fill_pct(app);
    if pct < soft_pct {
        reset_streak();
        return Outcome::Noop;
    }
    if !app.auto_compact_enabled {
        return Outcome::Noop;
    }
    if pct >= hard_pct {
        hard_reset(app);
        reset_streak();
        return Outcome::HardReset;
    }
    // Soft path. The streak counter forces a hard reset when two
    // soft compactions in a row did not move the needle.
    let mut state = STATE.lock().unwrap_or_else(|p| p.into_inner());
    let drifted = state.soft_streak >= 1
        && pct.abs_diff(state.last_soft_pct) < 5
        && pct >= soft_pct;
    state.last_soft_pct = pct;
    state.soft_streak = state.soft_streak.saturating_add(1);
    drop(state);
    if drifted {
        hard_reset(app);
        reset_streak();
        return Outcome::HardReset;
    }
    if soft_compact(app, router).await {
        Outcome::SoftCompacted
    } else {
        Outcome::Noop
    }
}

fn reset_streak() {
    let mut state = STATE.lock().unwrap_or_else(|p| p.into_inner());
    state.soft_streak = 0;
    state.last_soft_pct = 0;
}

/// Routes a one-shot summarization call through the router's
/// cheapest healthy entry, then folds the history into a single
/// assistant turn. Returns `false` if the summarizer call fails;
/// the caller treats that as a no-op and lets the next turn decide.
async fn soft_compact(app: &mut App, router: &Router) -> bool {
    let summarizer = router.next_summarizer().model.clone();
    let used_before = app.session.approx_tokens();
    let original_model = std::mem::replace(&mut app.config.model, summarizer);
    crate::commands::generation::compact(app).await;
    app.config.model = original_model;
    let used_after = app.session.approx_tokens();
    if used_after < used_before {
        app.last_auto_compact_count = app.session.messages().len();
        true
    } else {
        false
    }
}

/// Keeps the system prompt + the last `HARD_KEEP_TURNS` turns and
/// drops everything else. Persists a one-line marker to
/// `.govinda/compaction.log` for observability.
fn hard_reset(app: &mut App) {
    use crate::api::Message;
    let keep = HARD_KEEP_TURNS;
    let len = app.session.messages().len();
    if len <= keep {
        return;
    }
    let drop_count = len - keep;
    let start = len - keep;
    let mut tail: Vec<Message> = app.session.messages()[start..].to_vec();
    let note = format!(
        "Earlier context was reset at {} to recover from overflow (dropped {drop_count} messages).",
        chrono::Utc::now().to_rfc3339()
    );
    let mut new_msgs = Vec::with_capacity(tail.len() + 1);
    new_msgs.push(Message::system(note));
    new_msgs.append(&mut tail);
    app.session.replace_messages(new_msgs);
    let _ = std::fs::create_dir_all(".govinda");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(".govinda/compaction.log")
        .and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "hard_reset ts={} dropped={drop_count}", chrono::Utc::now().to_rfc3339())
        });
    app.last_auto_compact_count = app.session.messages().len();
}
