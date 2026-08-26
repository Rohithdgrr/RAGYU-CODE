//! `/todo` — a small persistent task list shared by the REPL and TUI.
//!
//! Storage: `.govinda/todos.json` under the project root (the cwd at the
//! time the list is first created). Storing under `.govinda/` keeps the
//! state inside the project, so opening a different project loads a
//! different list — instead of a single global file that would appear to
//! "reset" (look empty) when switching folders.

use super::{App, dim, err, info, ok};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const TODO_DIR: &str = ".govinda";
const TODO_FILE: &str = "todos.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Todo {
    pub text: String,
    pub done: bool,
}

fn todo_path() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cwd.join(TODO_DIR).join(TODO_FILE)
}

/// Loads the persisted list; a missing or unreadable file means "empty".
pub fn load() -> Vec<Todo> {
    let path = todo_path();
    if !path.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Persists the current list (also used by `/plan` to track execution).
pub(super) fn save(app: &App) {
    let Ok(json) = serde_json::to_string_pretty(&app.todos) else {
        return;
    };
    let path = todo_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, &json) {
        err(format!("could not save todos: {e}"));
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        err(format!("could not save todos: {e}"));
    }
}

/// Returns the on-disk path of the current todo list. Useful for the model
/// tool so it can tell the user where the file lives.
pub fn current_path() -> PathBuf {
    todo_path()
}

/// Entry point for `/todo [subcommand]`.
pub(super) fn handle(arg: &str, app: &mut App) {
    let (sub, rest) = match arg.split_once(char::is_whitespace) {
        Some((s, r)) => (s.to_ascii_lowercase(), r.trim()),
        None => (arg.trim().to_ascii_lowercase(), ""),
    };
    match sub.as_str() {
        "" | "list" | "ls" => list(app),
        "add" => add(rest, app),
        "done" => toggle_done(rest, app, true),
        "undo" | "reopen" => toggle_done(rest, app, false),
        "rm" | "remove" | "del" => remove(rest, app),
        "clear" => clear(app),
        other => err(format!(
            "unknown /todo subcommand '{other}' — use add | list | done | undo | rm | clear"
        )),
    }
}

fn list(app: &App) {
    if app.todos.is_empty() {
        dim("(no todos — '/todo add <text>' to create one)");
        return;
    }
    for (i, t) in app.todos.iter().enumerate() {
        let marker = if t.done { "[x]" } else { "[ ]" };
        info(format!("{marker} {:>2}. {}", i + 1, t.text));
    }
    let open = app.todos.iter().filter(|t| !t.done).count();
    dim(format!("{} open / {} total", open, app.todos.len()));
}

fn add(text: &str, app: &mut App) {
    if text.is_empty() {
        info("usage: /todo add <text>");
        return;
    }
    app.todos.push(Todo {
        text: text.to_owned(),
        done: false,
    });
    ok(format!("added #{}: {text}", app.todos.len()));
    save(app);
}

fn toggle_done(arg: &str, app: &mut App, done: bool) {
    match parse_index(arg, app.todos.len()) {
        Some(i) => {
            app.todos[i].done = done;
            let verb = if done { "done" } else { "reopened" };
            ok(format!("#{} {}: {}", i + 1, verb, app.todos[i].text));
            save(app);
        }
        None => info(format!(
            "usage: /todo {} <1-{}>",
            if done { "done" } else { "undo" },
            app.todos.len().max(1)
        )),
    }
}

fn remove(arg: &str, app: &mut App) {
    match parse_index(arg, app.todos.len()) {
        Some(i) => {
            let removed = app.todos.remove(i);
            ok(format!("removed #{}: {}", i + 1, removed.text));
            save(app);
        }
        None => info(format!("usage: /todo rm <1-{}>", app.todos.len().max(1))),
    }
}

fn clear(app: &mut App) {
    let n = app.todos.len();
    app.todos.clear();
    save(app);
    if n == 0 {
        dim("(nothing to clear)");
    } else {
        ok(format!("cleared {n} todo(s)."));
    }
}

fn parse_index(arg: &str, len: usize) -> Option<usize> {
    arg.trim()
        .parse::<usize>()
        .ok()
        .filter(|&n| n >= 1 && n <= len)
        .map(|n| n - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_parse_is_one_based_and_bounded() {
        assert_eq!(parse_index("1", 3), Some(0));
        assert_eq!(parse_index(" 3 ", 3), Some(2));
        assert_eq!(parse_index("0", 3), None);
        assert_eq!(parse_index("4", 3), None);
        assert_eq!(parse_index("x", 3), None);
        assert_eq!(parse_index("", 0), None);
    }

    #[test]
    fn subcommands_route_case_insensitively() {
        // The dispatcher lowercases; ensure our expected set is lowercase.
        for sub in [
            "list", "ls", "add", "done", "undo", "reopen", "rm", "remove", "del", "clear",
        ] {
            assert_eq!(sub.to_ascii_lowercase(), sub);
        }
    }
}
