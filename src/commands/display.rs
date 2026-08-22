use super::{App, dim, err, ok};
use crate::render::{self, paint, theme_names};
use crossterm::style::Color;
use std::time::Duration;

pub(super) fn set_temperature(arg: &str, app: &mut App) {
    match parse_temperature(arg) {
        Some(t) => {
            app.config.temperature = t;
            ok(&format!("temperature set to {t:.2}"));
        }
        None => println!(
            "usage: /temp <0.0-1.0>   (current: {:.2})",
            app.config.temperature
        ),
    }
}

pub(super) fn parse_temperature(arg: &str) -> Option<f32> {
    arg.trim()
        .parse::<f32>()
        .ok()
        .filter(|t| (0.0..=1.0).contains(t))
}

pub(super) fn set_or_show_system(prompt: &str, app: &mut App) {
    if prompt.is_empty() {
        println!("system prompt: {}", app.session.system());
        return;
    }
    app.session.set_system(prompt);
    ok("system prompt updated (applies to the next turn).");
}

pub(super) fn set_or_show_theme(name: &str) {
    if name.is_empty() {
        println!(
            "theme: {} (available: {})",
            render::active_theme().name,
            theme_names().collect::<Vec<_>>().join(", ")
        );
        return;
    }
    if render::set_theme(name) {
        ok(&format!("theme set to {name}"));
    } else {
        err(&format!(
            "unknown theme '{name}' — available: {}",
            theme_names().collect::<Vec<_>>().join(", ")
        ));
    }
}

pub(super) fn set_timeout(arg: &str, app: &mut App) {
    match arg
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|s| (1..=600).contains(s))
    {
        Some(secs) => {
            app.read_timeout = Duration::from_secs(secs);
            ok(&format!("read timeout set to {secs}s"));
        }
        None => println!(
            "usage: /timeout <1-600>   (current: {}s)",
            app.read_timeout.as_secs()
        ),
    }
}

pub(super) fn set_limit(arg: &str, app: &mut App) {
    match arg
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|mb| (1..=64).contains(mb))
    {
        Some(mb) => {
            app.max_response_bytes = (mb as usize) * 1024 * 1024;
            ok(&format!("response cap set to {mb} MB"));
        }
        None => println!(
            "usage: /limit <1-64>   (current: {} MB)",
            app.max_response_bytes / (1024 * 1024)
        ),
    }
}

pub(super) fn show_tools(arg: &str, app: &mut App) {
    match arg.trim() {
        "on" | "off" => {
            let next = arg.trim() == "on";
            if !next && app.tool_executor.is_none() {
                err("tools are unavailable (no executor configured).");
                return;
            }
            app.tools_enabled = next;
            ok(&format!(
                "function calling {}.",
                if next { "enabled" } else { "disabled" }
            ));
        }
        "" => {
            if !app.tools_enabled {
                err("function calling is off — '/tools on' to re-enable.");
                return;
            }
            println!("function calling: on");
            match &app.tool_executor {
                Some(_) => {
                    for tool in &app.tool_specs {
                        println!(
                            "  {} — {}",
                            paint(&tool.name, Color::Green),
                            tool.description
                        );
                    }
                }
                None => err("no tool executor configured."),
            }
        }
        _ => println!("usage: /tools [on|off]   (currently shows the registry)"),
    }
}

pub(super) fn print_history(app: &App) {
    let msgs = app.session.messages();
    if msgs.is_empty() {
        dim("(empty)");
        return;
    }
    for m in msgs {
        let (label, color) = if m.role == "user" {
            ("you", Color::Green)
        } else if m.role == "tool" {
            ("tool", render::dim_color())
        } else if m.has_tool_calls() {
            ("bot·tools", render::bot_color())
        } else {
            ("bot", render::bot_color())
        };
        println!("{} {}", paint(format!("[{label}]"), color), m.content);
        if let Some(calls) = &m.tool_calls {
            for c in calls {
                println!(
                    "  {} {}({})",
                    paint("→", color),
                    c.function.name,
                    c.function.arguments
                );
            }
        }
    }
}

pub(super) fn search_history(needle: &str, app: &App) {
    if needle.is_empty() {
        println!("usage: /search <text>");
        return;
    }
    let hits = app.session.search(needle);
    if hits.is_empty() {
        dim(&format!("no matches for '{needle}'."));
        return;
    }
    for (idx, role, content) in &hits {
        let label = if *role == "user" { "you" } else { "bot" };
        println!("{}", paint(format!("[#{idx} {label}]"), Color::Green));
        println!("{content}");
    }
    ok(&format!("{} match(es).", hits.len()));
}

pub(super) fn show_stats(app: &App) {
    let elapsed = app.stats.started.map_or(Duration::ZERO, |s| s.elapsed());
    let avg = if app.stats.turns > 0 {
        app.stats.total_latency_ms / u128::from(app.stats.turns)
    } else {
        0
    };
    println!(
        "session        {}\nturns          {}\navg latency    {} ms\nerrors         {}\nhistory        {} messages (~{} tokens)",
        format_duration(elapsed),
        app.stats.turns,
        avg,
        app.stats.errors,
        app.session.messages().len(),
        app.session.approx_tokens(),
    );
}

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
    println!(
        "config file    {}\nprovider       {} ({})\nmodel          {}\ntemperature    {:.2}\ncontext budget {} tokens (tokenizer-trimmed)\nrendering      {}\ntheme          {}\ntimeout        {}s\nresponse cap   {} MB\nhistory        {} messages (~{} tokens)",
        config_file,
        app.config.provider.id(),
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
    );
}
