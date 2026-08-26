//! `/cd` / `/cwd` / `/folder` / `/open` — change working directory.
//! Updates process cwd, project memory, todo list, and symbol index.
//! Used by both REPL and TUI (TUI also refreshes its FileTree).

use std::path::PathBuf;

use super::{dim, err, info, ok, App, Outcome};

fn resolve_target(raw: &str) -> PathBuf {
    let trimmed = raw.trim().trim_matches('"').trim_matches('\'');
    if trimmed.is_empty() {
        return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    }
    let mut p = PathBuf::from(trimmed);
    // Expand ~ to home
    if let Some(stripped) = trimmed.strip_prefix("~/").or_else(|| trimmed.strip_prefix("~\\")) {
        if let Some(home) = dirs_home() {
            p = home.join(stripped);
        }
    } else if trimmed == "~"
        && let Some(home) = dirs_home() {
            p = home;
        }
    p
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

pub async fn handle(arg: &str, app: &mut App) -> Outcome {
    let raw = arg.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("show") || raw.eq_ignore_ascii_case("pwd") {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        info(format!("cwd: {}", cwd.display()));
        return Outcome::Handled;
    }
    let target = resolve_target(raw);
    // Resolve relative against current cwd
    let abs = if target.is_absolute() {
        target
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(&target)
    };
    let canon = match abs.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            err(format!("cannot open '{}': {e}", abs.display()));
            return Outcome::Handled;
        }
    };
    if !canon.is_dir() {
        err(format!("not a directory: {}", canon.display()));
        return Outcome::Handled;
    }
    let prev = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Err(e) = std::env::set_current_dir(&canon) {
        err(format!("cd failed: {e}"));
        return Outcome::Handled;
    }
    // Refresh state that is per-workspace
    app.todos = crate::commands::todo::load();
    app.project_memory = crate::memory::ProjectMemory::load(&canon);
    // Rebuild symbol index for new workspace (best-effort)
    let count = crate::symbols::rebuild(&canon);
    ok(format!("opened: {}", canon.display()));
    if prev != canon {
        dim(format!("previous: {}", prev.display()));
    }
    info(format!("{count} symbols indexed · {} todos", app.todos.len()));
    // Signal TUI to rebuild tree (it watches Effect::ReloadTranscript to rebuild transcript; we use custom)
    // For REPL, just handled. For TUI, caller will refresh FileTree via effect.
    Outcome::Handled
}

#[allow(dead_code)] // used by TUI
pub fn show_current() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}


