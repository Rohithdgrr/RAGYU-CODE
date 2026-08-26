use super::{App, dim, err, info, ok};
use crate::render::{self, theme_names};
use crate::tools::save_disabled_tools;
use std::time::Duration;

#[allow(dead_code)]
pub(super) fn set_temperature(arg: &str, app: &mut App) {
    match parse_temperature(arg) {
        Some(t) => {
            app.config.temperature = t;
            ok(format!("temperature set to {t:.2}"));
        }
        None => info(format!(
            "usage: /temp <0.0-1.0>   (current: {:.2})",
            app.config.temperature
        )),
    }
}

#[allow(dead_code)]
pub(super) fn parse_temperature(arg: &str) -> Option<f32> {
    arg.trim()
        .parse::<f32>()
        .ok()
        .filter(|t| (0.0..=1.0).contains(t))
}

#[allow(dead_code)]
pub(super) fn set_or_show_system(prompt: &str, app: &mut App) {
    if prompt.is_empty() {
        info(format!("system prompt: {}", app.session.system()));
        return;
    }
    app.session.set_system(prompt);
    // Re-apply agent specialization so a custom prompt keeps the tool
    // addendum + project memory + skills (REPL/TUI parity).
    super::specialize_system(app);
    ok("system prompt updated (applies to the next turn).");
}

pub(super) fn set_or_show_theme(name: &str, app: &App) {
    if name.is_empty() {
        info(format!(
            "theme: {} (available: {})",
            render::active_theme().name,
            theme_names().collect::<Vec<_>>().join(", ")
        ));
        return;
    }
    if render::set_theme(name) {
        // Persist the choice when the config came from a real file.
        if let Err(e) = super::persistence::save_runtime_config(app) {
            dim(format!("(theme not persisted: {e:#})"));
        }
        ok(format!("theme set to {name}"));
    } else {
        err(format!(
            "unknown theme '{name}' — available: {}",
            theme_names().collect::<Vec<_>>().join(", ")
        ));
    }
}

#[allow(dead_code)]
pub(super) fn set_timeout(arg: &str, app: &mut App) {
    match arg
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|s| (1..=600).contains(s))
    {
        Some(secs) => {
            app.read_timeout = Duration::from_secs(secs);
            ok(format!("read timeout set to {secs}s"));
        }
        None => info(format!(
            "usage: /timeout <1-600>   (current: {}s)",
            app.read_timeout.as_secs()
        )),
    }
}

#[allow(dead_code)]
pub(super) fn set_limit(arg: &str, app: &mut App) {
    match arg
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|mb| (1..=64).contains(mb))
    {
        Some(mb) => {
            app.max_response_bytes = (mb as usize) * 1024 * 1024;
            ok(format!("response cap set to {mb} MB"));
        }
        None => info(format!(
            "usage: /limit <1-64>   (current: {} MB)",
            app.max_response_bytes / (1024 * 1024)
        )),
    }
}

pub(super) fn show_tools(arg: &str, app: &mut App) {
    let arg = arg.trim();
    match arg {
        "on" | "off" => {
            let next = arg == "on";
            if !next && app.tool_executor.is_none() {
                err("tools are unavailable (no executor configured).");
                return;
            }
            app.tools_enabled = next;
            ok(format!(
                "function calling {}.",
                if next { "enabled" } else { "disabled" }
            ));
        }
        "" => {
            if !app.tools_enabled {
                err("function calling is off — '/tools on' to re-enable.");
                return;
            }
            info("function calling: on");
            match &app.tool_executor {
                Some(_) => {
                    for tool in &app.tool_specs {
                        let state = if app.disabled_tools.contains(&tool.name) {
                            "[disabled]"
                        } else {
                            "[on]"
                        };
                        info(format!("  {state} {} — {}", tool.name, tool.description));
                    }
                    if !app.disabled_tools.is_empty() {
                        dim("re-enable with '/tools enable <name>'.");
                    }
                }
                None => err("no tool executor configured."),
            }
        }
        _ => {
            // /tools enable|disable <name>
            let Some((verb, name)) = arg.split_once(char::is_whitespace) else {
                info("usage: /tools [on|off] | /tools enable|disable <name>");
                return;
            };
            let name = name.trim();
            let disable = match verb {
                "enable" | "en" => false,
                "disable" | "dis" => true,
                _ => {
                    info("usage: /tools [on|off] | /tools enable|disable <name>");
                    return;
                }
            };
            if !app.tool_specs.iter().any(|t| t.name == name) {
                err(format!("unknown tool '{name}' — run /tools to see the registry"));
                return;
            }
            if disable {
                app.disabled_tools.insert(name.to_owned());
                ok(format!(
                    "'{name}' disabled — the model can no longer call it."
                ));
            } else if app.disabled_tools.remove(name) {
                ok(format!("'{name}' re-enabled."));
            } else {
                ok(format!("'{name}' was already enabled."));
            }
            if let Err(e) = save_disabled_tools(&app.disabled_tools) {
                err(format!("{e:#}"));
            }
        }
    }
}

pub(super) fn print_history(app: &App) {
    let msgs = app.session.messages();
    if msgs.is_empty() {
        dim("(empty)");
        return;
    }
    for m in msgs {
        let label = if m.role == "user" {
            "you"
        } else if m.role == "tool" {
            "tool"
        } else if m.has_tool_calls() {
            "bot·tools"
        } else {
            "bot"
        };
        info(format!("[{label}] {}", m.content));
        if let Some(calls) = &m.tool_calls {
            for c in calls {
                info(format!("  → {}({})", c.function.name, c.function.arguments));
            }
        }
    }
}

#[allow(dead_code)]
pub(super) fn search_history(needle: &str, app: &App) {
    if needle.is_empty() {
        info("usage: /search <text>");
        return;
    }
    let hits = app.session.search(needle);
    if hits.is_empty() {
        dim(format!("no matches for '{needle}'."));
        return;
    }
    for (idx, role, content) in &hits {
        let label = if *role == "user" { "you" } else { "bot" };
        info(format!("[#{idx} {label}] {content}"));
    }
    ok(format!("{} match(es).", hits.len()));
}

#[allow(dead_code)]
pub(super) fn show_stats(app: &App) {
    let elapsed = app.stats.started.map_or(Duration::ZERO, |s| s.elapsed());
    let avg = if app.stats.turns > 0 {
        app.stats.total_latency_ms / u128::from(app.stats.turns)
    } else {
        0
    };
    info(format!(
        "session        {}\nturns          {}\navg latency    {} ms\nerrors         {}\nhistory        {} messages (~{} tokens)",
        format_duration(elapsed),
        app.stats.turns,
        avg,
        app.stats.errors,
        app.session.messages().len(),
        app.session.approx_tokens(),
    ));
}

#[allow(dead_code)]
fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

pub(super) fn show_config(app: &App) {
    let config_file = app
        .config
        .source_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(none found — defaults)".to_owned());
    info(format!(
        "config file    {}\nprovider       {} ({})\nmodel          {}\ntemperature    {:.2}\ncontext budget {} tokens (tokenizer-trimmed)\nrendering      {}\ntheme          {}\ntimeout        {}s\nresponse cap   {} MB\nhistory        {} messages (~{} tokens)",
        config_file,
        app.config.provider.key(),
        app.config.provider.chat_url(),
        app.config.model,
        app.config.temperature,
        app.config.context_tokens,
        if app.renderer.markdown_enabled() {
            "markdown"
        } else {
            "raw streaming"
        },
        render::active_theme().name,
        app.read_timeout.as_secs(),
        app.max_response_bytes / (1024 * 1024),
        app.session.messages().len(),
        app.session.approx_tokens(),
    ));
}
