use super::{App, dim, err, ok};
use crate::clock;
use crate::render::{accent, paint};
use crate::session::Session;
use crate::sessions;
use anyhow::Context;
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

// ---------------------------------------------------------------------------
// /config save — persist runtime settings back into config.toml
// ---------------------------------------------------------------------------

/// Runtime values worth persisting; split out from file I/O so the merge is
/// unit-testable.
pub(super) struct RuntimeSnapshot {
    pub model: String,
    pub temperature: f32,
    pub system_prompt: String,
    pub render_markdown: bool,
    pub theme: String,
    pub timeout_secs: u64,
    pub limit_mb: u64,
}

impl RuntimeSnapshot {
    pub fn from_app(app: &App) -> Self {
        Self {
            model: app.config.model.clone(),
            temperature: app.config.temperature,
            system_prompt: app.session.system().to_owned(),
            render_markdown: app.renderer.markdown_enabled(),
            theme: crate::render::active_theme().name.to_owned(),
            timeout_secs: app.read_timeout.as_secs(),
            limit_mb: (app.max_response_bytes / (1024 * 1024)) as u64,
        }
    }
}

/// Updates the keys Govinda owns inside an existing TOML table, leaving every
/// other key (including `[[tools]]` blocks) untouched.
fn merge_snapshot(table: &mut toml::Table, s: &RuntimeSnapshot) {
    table.insert("model".into(), toml::Value::from(s.model.clone()));
    table.insert("temperature".into(), toml::Value::from(s.temperature));
    table.insert(
        "system_prompt".into(),
        toml::Value::from(s.system_prompt.clone()),
    );
    table.insert(
        "render_markdown".into(),
        toml::Value::from(s.render_markdown),
    );
    table.insert("theme".into(), toml::Value::from(s.theme.clone()));
    table.insert(
        "timeout_secs".into(),
        toml::Value::from(s.timeout_secs as i64),
    );
    table.insert("limit_mb".into(), toml::Value::from(s.limit_mb as i64));
}

/// Resolves where `/config save` writes: `GOVINDA_CONFIG` > the file that was
/// loaded > the default location.
fn save_target_path(app: &App) -> anyhow::Result<PathBuf> {
    if let Some(p) = std::env::var_os("GOVINDA_CONFIG") {
        return Ok(PathBuf::from(p));
    }
    if let Some(p) = &app.config.source_path {
        return Ok(p.clone());
    }
    crate::config::default_config_path()
        .ok_or_else(|| anyhow::anyhow!("cannot determine a config path (no HOME set?)"))
}

/// Writes current runtime settings to config.toml. The existing file is
/// parsed generically and re-serialized, so unknown keys survive.
pub(super) fn save_runtime_config(app: &App) -> anyhow::Result<PathBuf> {
    let path = save_target_path(app)?;
    let mut table: toml::Table = match std::fs::read_to_string(&path) {
        Ok(raw) => {
            toml::from_str(&raw).with_context(|| format!("cannot parse {}", path.display()))?
        }
        Err(_) => toml::Table::new(),
    };
    let snapshot = RuntimeSnapshot::from_app(app);
    merge_snapshot(&mut table, &snapshot);
    std::fs::write(&path, toml::to_string_pretty(&table)?)
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod save_config_tests {
    use super::*;

    #[test]
    fn merge_updates_owned_keys_only() {
        let mut table: toml::Table = toml::from_str(
            r#"
model = "old-model"
temperature = 0.9
custom_key = "keep me"

[[tools]]
name = "gh_pr"
description = "d"
command = "gh"
args_template = ["pr", "list"]
"#,
        )
        .unwrap();
        let snapshot = RuntimeSnapshot {
            model: "new-model".into(),
            temperature: 0.2,
            system_prompt: "be brief".into(),
            render_markdown: false,
            theme: "dracula".into(),
            timeout_secs: 45,
            limit_mb: 8,
        };
        merge_snapshot(&mut table, &snapshot);
        let out = toml::to_string_pretty(&table).unwrap();

        assert!(out.contains("model = \"new-model\""), "{out}");
        assert!(out.contains("temperature = 0.2"), "{out}");
        assert!(out.contains("theme = \"dracula\""), "{out}");
        assert!(out.contains("timeout_secs = 45"), "{out}");
        assert!(out.contains("limit_mb = 8"), "{out}");
        // unknown keys and [[tools]] survive the round-trip
        assert!(out.contains("custom_key = \"keep me\""), "{out}");
        assert!(out.contains("[[tools]]"), "{out}");
        assert!(out.contains("name = \"gh_pr\""), "{out}");
    }

    #[test]
    fn merge_into_empty_table_yields_valid_config() {
        let mut table = toml::Table::new();
        let snapshot = RuntimeSnapshot {
            model: "m".into(),
            temperature: 0.5,
            system_prompt: "s".into(),
            render_markdown: true,
            theme: "default".into(),
            timeout_secs: 30,
            limit_mb: 16,
        };
        merge_snapshot(&mut table, &snapshot);
        let out = toml::to_string_pretty(&table).unwrap();
        crate::config::parse_file_config_for_test(&out).expect("saved config should load cleanly");
    }
}
