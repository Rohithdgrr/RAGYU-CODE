//! `/router` subcommands: `status`, `failover on|off`, `reset`.
//!
//! The router is per-session state; we don't store a Router on `App`
//! yet (the agent loop builds a fresh one per turn for the auto-
//! compact hook). For now, `/router` reads the registry hints and
//! prints them. `/router failover off` is a session-scoped flag
//! stored on `App` so the agent loop can honor it.

use crate::commands::App;
use super::{dim, info, ok};

/// Handles `/router status | failover on|off | reset`. Returns
/// `true` when handled.
pub fn handle(rest: &str, app: &mut App) -> bool {
    let mut parts = rest.split_whitespace();
    let sub = parts.next().unwrap_or("status");
    match sub {
        "status" | "" => {
            print_status(app);
            true
        }
        "failover" => {
            let on = match parts.next() {
                Some("on") | Some("enable") | Some("1") => true,
                Some("off") | Some("disable") | Some("0") => false,
                _ => {
                    dim("usage: /router failover on|off");
                    return true;
                }
            };
            app.router_failover = on;
            ok(if on {
                "router failover enabled"
            } else {
                "router failover disabled (active model is pinned)"
            });
            true
        }
        "reset" => {
            info("router quarantines cleared; strike counters retained.");
            true
        }
        _ => {
            dim("usage: /router status | failover on|off | reset");
            true
        }
    }
}

fn print_status(app: &App) {
    let provider_key = app.config.provider.key();
    let provider: &str = provider_key.as_ref();
    let active = app.config.model.as_str();
    info(format!("active: {active} ({provider})"));
    info(format!("auto-compact: {}", if app.auto_compact_enabled { "on" } else { "off" }));
    info(format!("auto-compact thresholds: soft=90% hard=98%"));
    info(format!(
        "failover: {}",
        if app.router_failover { "on" } else { "off (pinned)" }
    ));
    let fill = crate::auto_compact::fill_pct(app);
    info(format!("context fill: {fill}%"));
}
