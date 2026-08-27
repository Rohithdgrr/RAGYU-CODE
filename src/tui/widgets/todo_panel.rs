//! Todo list pane — Frosted Daylight glass, sharp edges.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::commands::todo::Todo;
use crate::tui::theme;

/// Build pane lines for the todo list. `focused` tints the header accent.
pub fn build_lines(todos: &[Todo], focused: bool, width: u16) -> Vec<Line<'static>> {
    let t = theme::active();
    let w = width.saturating_sub(2) as usize;
    let mut out = Vec::new();

    // header is rendered by draw::Block title, so start with filter / count
    let open = todos.iter().filter(|x| !x.done).count();
    let total = todos.len();
    let count_style = if open == 0 && total > 0 {
        Style::default()
            .fg(t.accent_success)
            .bg(t.bg_secondary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.text_muted).bg(t.bg_secondary)
    };
    out.push(Line::from(vec![
        Span::styled(format!(" {open} left "), count_style),
        Span::styled(
            format!("· {total} total"),
            Style::default().fg(t.text_muted).bg(t.bg_secondary),
        ),
    ]));
    out.push(Line::styled(
        "─".repeat(w.min(28)),
        Style::default().fg(t.border_default).bg(t.bg_secondary),
    ));

    if todos.is_empty() {
        out.push(Line::styled(
            "  No tasks — /todo add <text>",
            Style::default()
                .fg(t.text_muted)
                .bg(t.bg_secondary)
                .add_modifier(Modifier::ITALIC),
        ));
        out.push(Line::default());
        out.push(Line::from(vec![
            Span::styled(
                "  /todo add ",
                Style::default()
                    .fg(t.accent_primary)
                    .bg(t.bg_secondary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "create",
                Style::default().fg(t.text_muted).bg(t.bg_secondary),
            ),
        ]));
        return out;
    }

    for (idx, todo) in todos.iter().enumerate() {
        let is_done = todo.done;
        let num = format!("{:>2}.", idx + 1);
        let check = if is_done { "✓" } else { " " };
        let box_style = if is_done {
            Style::default().fg(t.accent_success).bg(t.bg_secondary)
        } else {
            Style::default().fg(t.text_muted).bg(t.bg_secondary)
        };
        // wrap text to pane width - 8 (num + box + gaps)
        let inner = w.saturating_sub(8);
        let wrapped = crate::tui::widgets::chat_pane::wrap(&todo.text, inner);
        for (j, line) in wrapped.into_iter().enumerate() {
            if j == 0 {
                let text_style = if is_done {
                    Style::default()
                        .fg(t.text_muted)
                        .bg(t.bg_secondary)
                        .add_modifier(Modifier::CROSSED_OUT)
                } else {
                    Style::default()
                        .fg(if focused {
                            t.text_primary
                        } else {
                            t.text_secondary
                        })
                        .bg(t.bg_secondary)
                };
                out.push(Line::from(vec![
                    Span::styled(
                        format!(" {num} "),
                        Style::default().fg(t.text_muted).bg(t.bg_secondary),
                    ),
                    Span::styled(format!("[{check}] "), box_style),
                    Span::styled(line, text_style),
                ]));
            } else {
                let text_style = if is_done {
                    Style::default()
                        .fg(t.text_muted)
                        .bg(t.bg_secondary)
                        .add_modifier(Modifier::CROSSED_OUT)
                } else {
                    Style::default().fg(t.text_secondary).bg(t.bg_secondary)
                };
                out.push(Line::from(vec![
                    Span::styled("      ", Style::default().bg(t.bg_secondary)),
                    Span::styled(line, text_style),
                ]));
            }
        }
    }

    out.push(Line::default());
    // footer hints
    out.push(Line::from(vec![
        Span::styled(
            "  /todo ",
            Style::default()
                .fg(t.accent_primary)
                .bg(t.bg_secondary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "add/done/rm/clear",
            Style::default().fg(t.text_muted).bg(t.bg_secondary),
        ),
    ]));
    out.push(Line::from(vec![
        Span::styled(
            "  ? ",
            Style::default()
                .fg(t.accent_secondary)
                .bg(t.bg_secondary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "shortcuts",
            Style::default().fg(t.text_muted).bg(t.bg_secondary),
        ),
    ]));

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::todo::Todo;

    #[test]
    fn empty_shows_hint() {
        let lines = build_lines(&[], false, 40);
        let flat: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.clone())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(flat.contains("No tasks"));
    }

    #[test]
    fn renders_mixed() {
        let todos = vec![
            Todo {
                text: "a".into(),
                done: false,
            },
            Todo {
                text: "b".into(),
                done: true,
            },
        ];
        let lines = build_lines(&todos, true, 40);
        let flat: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(flat.contains("1."));
        assert!(flat.contains("2."));
        assert!(flat.contains("1 left"));
    }
}
