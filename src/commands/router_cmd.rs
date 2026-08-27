//! `/router` subcommands: `status`, `failover on|off`, `reset`.

use super::{dim, info, ok};
use crate::commands::App;

/// Handles `/router status | failover on|off | reset`. Returns
/// `true` when handled.
pub fn handle(rest: &str, app: &mut App) -> bool {
    // Keep router in sync with the current provider/model before
    // inspecting it — `/model` may have changed the active without
    // a turn having run.
    {
        let provider_key = app.config.provider.key().to_string();
        let model = app.config.model.clone();
        app.router.sync_active(&provider_key, &model);
        app.router.set_failover(app.router_failover);
    }
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
            app.router.set_failover(on);
            ok(if on {
                "router failover enabled"
            } else {
                "router failover disabled (active model is pinned)"
            });
            true
        }
        "reset" => {
            app.router.clear_quarantines();
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
    info(format!(
        "auto-compact: {}",
        if app.auto_compact_enabled {
            "on"
        } else {
            "off"
        }
    ));
    info(format!("auto-compact thresholds: soft=90% hard=98%"));
    info(format!(
        "failover: {}",
        if app.router_failover {
            "on"
        } else {
            "off (pinned)"
        }
    ));
    let fill = crate::auto_compact::fill_pct(app);
    info(format!("context fill: {fill}%"));
    // Router entries + health
    info("router entries:");
    for (i, e) in app.router.iter().enumerate() {
        let marker = if e.model == active { " ← active" } else { "" };
        let quarantined = if app.router.is_quarantined(&e.model) {
            " [QUARANTINED]"
        } else {
            ""
        };
        info(format!(
            "  {i}. {}{marker}{quarantined}  role={:?} ctx={} free={}",
            e.model,
            e.role,
            e.context_window,
            crate::provider::omniroute_combo(&e.model)
                .map(|c| c.free)
                .unwrap_or(false)
        ));
        if let Some(h) = app.router.health(&e.model) {
            let err = h.last_error.as_deref().unwrap_or("none");
            info(format!(
                "     strikes={} latency={}ms total={}/{} err={}",
                h.strikes, h.last_latency_ms, h.total_failures, h.total_requests, err
            ));
        }
    }
    let quarantined: Vec<&str> = app.router.quarantined().collect();
    if quarantined.is_empty() {
        info("quarantined: (none)");
    } else {
        info(format!("quarantined: {}", quarantined.join(", ")));
    }
}
