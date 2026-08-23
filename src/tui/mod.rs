//! Rich terminal UI: a lightweight IDE layout around the same
//! agent core the REPL drives.
//!
//! - status bar, chat pane with code blocks, input bar with slash completion
//!   and history, streaming answers (Phase A+B)
//! - project tree sidebar with git marks and context pinning (Ctrl+T)
//! - tool panel sidebar: registry, live activity, session stats (Ctrl+P)
//! - gated tools pause in [REVIEW] mode: y approve · n decline · a all
//! - `/plan <task>` generates a step checklist and executes it turn-by-turn

pub mod app;
pub mod draw;
pub mod layout;
pub mod theme;
pub mod widgets;

use anyhow::Result;

use crate::commands::App;

/// Launches the interactive TUI. Terminal state is always restored, even on
/// error paths.
pub async fn run(app: &mut App) -> Result<()> {
    app::run(app).await
}
