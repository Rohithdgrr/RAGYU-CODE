//! Tool panel sidebar: live tool registry, execution activity, and session
//! metrics. Data is snapshotted per frame (the turn runner owns `&mut App`
//! while streaming), so values lag at most one turn.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::super::theme;
use crate::tui::widgets::chat_pane::ChatEntry;

/// One registry row.
pub struct ToolRow {
    pub name: String,
    pub enabled: bool,
    /// Whether this tool asks before touching the workspace.
    pub gated: bool,
}

/// Compact activity record for the most recent tool calls.
#[derive(Clone, Copy)]
pub struct Activity {
    pub ok: Option<bool>,
}

pub fn build_lines(
    tools: &[ToolRow],
    activity: &[Activity],
    turns: u32,
    errors: u32,
    avg_latency_ms: u64,
) -> Vec<Line<'static>> {
    let t = theme::active();
    let mut lines = Vec::new();

    // ── Registry ──
    lines.push(header("TOOLS", &t));
    for tool in tools {
        let (flag, color) = if tool.enabled {
            ("on", t.accent_success)
        } else {
            ("off", t.accent_error)
        };
        let gate = if tool.gated { " 🔒" } else { "" };
        lines.push(Line::from(vec![
            Span::styled(
                format!("[{flag}] "),
                Style::default().fg(color),
            ),
            Span::styled(format!("{name}{gate}", name = tool.name), t.sidebar_bg()),
        ]));
    }

    // ── Live activity ──
    lines.push(Line::default());
    lines.push(header("ACTIVITY", &t));
    if activity.is_empty() {
        lines.push(Line::styled("no calls this session", muted(&t)));
    } else {
        let running = activity.iter().filter(|a| a.ok.is_none()).count();
        let failed = activity.iter().filter(|a| a.ok == Some(false)).count();
        let done = activity.iter().filter(|a| a.ok == Some(true)).count();
        lines.push(Line::from(vec![
            Span::styled(format!("✓ {done} "), t.success()),
            Span::styled(format!("✗ {failed} "), t.error()),
            Span::styled(
                format!("… {running}"),
                if running > 0 { t.warning() } else { muted(&t) },
            ),
        ]));
        // Most recent calls, newest last.
        for a in activity.iter().rev().take(6) {
            let (icon, style) = match a.ok {
                None => ("…", t.warning()),
                Some(true) => ("✓", t.success()),
                Some(false) => ("✗", t.error()),
            };
            lines.push(Span::styled(icon.to_owned(), style).into());
        }
    }

    // ── Session stats ──
    lines.push(Line::default());
    lines.push(header("STATUS", &t));
    for (label, value) in [
        ("Turns", turns.to_string()),
        ("Errors", errors.to_string()),
        ("Avg", format!("{avg_latency_ms}ms")),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!("{label}: "), muted(&t)),
            Span::styled(value, t.sidebar_bg()),
        ]));
    }

    lines
}

fn header(text: &str, t: &theme::Theme) -> Line<'static> {
    Line::styled(
        format!("── {text} "),
        Style::default()
            .fg(t.accent_secondary)
            .add_modifier(Modifier::BOLD),
    )
}

fn muted(t: &theme::Theme) -> Style {
    Style::default().fg(t.text_muted)
}

/// Extracts the tail of tool activity from the transcript.
pub fn activity_from_entries(entries: &[ChatEntry], limit: usize) -> Vec<Activity> {
    entries
        .iter()
        .rev()
        .filter_map(|e| match e {
            ChatEntry::Tool { ok, .. } => Some(Activity { ok: *ok }),
            _ => None,
        })
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_extracts_recent_tool_entries_in_order() {
        use ChatEntry::{Assistant, Notice, Tool};
        let entries = vec![
            Assistant("hi".into()),
            Tool {
                name: "a".into(),
                args: String::new(),
                ok: Some(true),
            },
            Notice("x".into()),
            Tool {
                name: "b".into(),
                args: String::new(),
                ok: None,
            },
            Tool {
                name: "c".into(),
                args: String::new(),
                ok: Some(false),
            },
        ];
        let acts = activity_from_entries(&entries, 10);
        assert_eq!(acts.len(), 3);
        assert_eq!(acts[0].ok, Some(true));
        assert_eq!(acts[2].ok, Some(false));

        let limited = activity_from_entries(&entries, 2);
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].ok, None); // keeps chronological order
    }

    #[test]
    fn lines_render_registry_and_stats() {
        let tools = vec![ToolRow {
            name: "read_file".into(),
            enabled: true,
            gated: false,
        }];
        let lines = build_lines(&tools, &[], 4, 1, 250);
        let joined: String = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.clone()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("[on] read_file"));
        assert!(joined.contains("Turns: 4"));
        assert!(joined.contains("Avg: 250ms"));
    }
}
