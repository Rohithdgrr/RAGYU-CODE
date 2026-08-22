use super::{App, dim, err, ok};
use crate::clock;
use crate::render::{accent, paint};
use crate::session::Session;
use crate::sessions;
use std::path::{Path, PathBuf};

/// Rejects absolute paths and any `..` component, then anchors relative paths
/// under `sessions/` so save/load/fork can never escape the workspace.
pub(super) fn safe_session_path(arg: &str) -> anyhow::Result<PathBuf> {
    let p = Path::new(arg);
    anyhow::ensure!(
        !p.is_absolute() && !p.has_root(),
        "absolute paths are not allowed"
    );
    anyhow::ensure!(
        !p.components().any(|c| c == std::path::Component::ParentDir),
        "'..' components are not allowed"
    );
    Ok(PathBuf::from(sessions::SESSIONS_DIR).join(p))
}

pub(super) fn save_session(arg: &str, app: &mut App) {
    let path = match arg.is_empty() {
        true => match app.session_name.as_deref() {
            Some(name) => match sessions::named_session_path(name) {
                Ok(p) => p,
                Err(e) => {
                    err(&format!("{e:#}"));
                    return;
                }
            },
            None => default_session_path(),
        },
        false => match safe_session_path(arg) {
            Ok(p) => p,
            Err(e) => {
                err(&format!("{e:#}"));
                return;
            }
        },
    };
    match app.session.save_to(&path) {
        Ok(()) => {
            if let Some(name) = sessions::name_from_path(&path) {
                app.session_name = Some(name);
            }
            ok(&format!(
                "saved {} messages to {}",
                app.session.messages().len(),
                path.display()
            ));
        }
        Err(e) => err(&format!("{e:#}")),
    }
}

fn default_session_path() -> PathBuf {
    PathBuf::from(format!(
        "{}/chat-{}.json",
        sessions::SESSIONS_DIR,
        clock::epoch_secs()
    ))
}

pub(super) fn load_session(arg: &str, app: &mut App) {
    if arg.is_empty() {
        println!("usage: /load <name>");
        return;
    }
    let path = match safe_session_path(arg) {
        Ok(p) => p,
        Err(e) => {
            err(&format!("{e:#}"));
            return;
        }
    };
    match Session::load_from(&path) {
        Ok(session) => {
            let n = session.messages().len();
            let saved_at = session.updated_at().map(str::to_owned);
            app.session_name = sessions::name_from_path(&path);
            app.session = session;
            ok(&format!(
                "loaded {n} messages from {}{}",
                path.display(),
                saved_at
                    .map(|t| format!(" (last saved {t})"))
                    .unwrap_or_default()
            ));
        }
        Err(e) => err(&format!("{e:#}")),
    }
}

/// Lists saved sessions, newest first, marking the one currently in use.
pub(super) fn list_named_sessions(app: &App) {
    let entries = sessions::list();
    if entries.is_empty() {
        dim("no saved sessions yet — /save <name> creates one.");
        return;
    }
    println!("saved sessions (newest first):");
    for e in entries {
        let current = app.session_name.as_deref() == Some(e.name.as_str());
        let ts = e.updated_at.clone().unwrap_or_else(|| "-".to_owned());
        println!(
            "  {}{}  {} msgs  {ts}",
            paint(&e.name, accent()),
            if current { " ←" } else { "" },
            e.messages,
        );
    }
}

/// Saves a snapshot of the current conversation without touching the live one.
pub(super) fn fork_session(arg: &str, app: &mut App) {
    let path = match arg.is_empty() {
        true => PathBuf::from(format!(
            "{}/fork-{}.json",
            sessions::SESSIONS_DIR,
            clock::epoch_secs()
        )),
        false => match safe_session_path(arg) {
            Ok(p) => p,
            Err(e) => {
                err(&format!("{e:#}"));
                return;
            }
        },
    };
    match app.session.save_to(&path) {
        Ok(()) => ok(&format!(
            "forked snapshot with {} messages to {} (live conversation unchanged)",
            app.session.messages().len(),
            path.display()
        )),
        Err(e) => err(&format!("{e:#}")),
    }
}

pub(super) fn export(arg: &str, app: &App) {
    let (fmt, file) = match arg.split_once(char::is_whitespace) {
        Some((f, r)) => (f.to_ascii_lowercase(), Some(r.trim().to_owned())),
        None => (arg.to_ascii_lowercase(), None),
    };
    let path = match file {
        Some(f) => PathBuf::from(f),
        None => PathBuf::from(format!(
            "{}/export-{}.{}",
            sessions::SESSIONS_DIR,
            clock::epoch_secs(),
            if fmt == "md" { "md" } else { "txt" }
        )),
    };
    let content = match fmt.as_str() {
        "md" => export_markdown(app),
        "txt" => export_text(app),
        other => {
            err(&format!(
                "unknown format '{other}' — usage: /export md|txt [file]"
            ));
            return;
        }
    };
    match std::fs::write(&path, content) {
        Ok(()) => ok(&format!("exported to {}", path.display())),
        Err(e) => err(&format!("export failed: {e}")),
    }
}

fn export_markdown(app: &App) -> String {
    let mut out = format!("# Conversation ({})\n\n", clock::now_iso8601());
    out.push_str(&format!(
        "**Model:** {} · **Provider:** {}\n\n",
        app.config.model,
        app.config.provider.id()
    ));
    for m in app.session.messages() {
        match m.role.as_str() {
            "user" => out.push_str(&format!("## You\n\n{}\n\n", m.content)),
            "tool" => out.push_str(&format!(
                "## Tool result ({})\n\n{}\n\n",
                m.tool_call_id.as_deref().unwrap_or("unknown id"),
                m.content
            )),
            _ => {
                out.push_str("## Assistant\n\n");
                if !m.content.is_empty() {
                    out.push_str(&format!("{}\n\n", m.content));
                }
                if let Some(calls) = &m.tool_calls {
                    for c in calls {
                        out.push_str(&format!(
                            "- `{}`({}) → id {}\n",
                            c.function.name, c.function.arguments, c.id
                        ));
                    }
                    out.push('\n');
                }
            }
        }
    }
    out
}

fn export_text(app: &App) -> String {
    let mut out = String::new();
    for m in app.session.messages() {
        let label = match m.role.as_str() {
            "user" => "You",
            "tool" => "Tool",
            _ => "Assistant",
        };
        out.push_str(&format!("{label}: {}\n\n", m.content));
    }
    out
}
