//! Input bar: bordered 3-row block with mode title, editable line, and
//! slash-command ghost completion.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};

use super::super::app::AppMode;
use super::super::theme;
use crate::commands;

/// Returns the slash command that `input` is a prefix of, if any.
pub fn completion(input: &str) -> Option<&'static str> {
    if !input.starts_with('/') || input.contains(char::is_whitespace) {
        return None;
    }
    commands::SLASH_COMMANDS
        .iter()
        .copied()
        .find(|c| c.starts_with(input) && *c != input)
}

/// Builds the bordered input block plus the styled input line.
///
/// Returns `(block_title_spans, input_line, completion_ghost)`.
pub fn build(mode: AppMode, focus_input: bool, input: &str) -> (String, Line<'static>, Option<String>) {
    let t = theme::active();

    let border_color = if !focus_input {
        t.border_default
    } else {
        match mode {
            AppMode::Normal => t.accent_primary,
            AppMode::Agent => t.accent_secondary,
            AppMode::Review => t.accent_warning,
            AppMode::Plan => t.accent_success,
        }
    };

    let placeholder = if input.is_empty() && focus_input {
        Some(Span::styled(
            "Ask me to code, debug, or explain…",
            Style::default().fg(t.text_muted),
        ))
    } else {
        None
    };

    let ghost = if focus_input {
        completion(input).map(|c| c[input.len()..].to_owned())
    } else {
        None
    };

    let mut spans = vec![Span::styled(
        "❯ ",
        Style::default()
            .fg(border_color)
            .add_modifier(Modifier::BOLD),
    )];
    match placeholder {
        Some(p) => spans.push(p),
        None => spans.push(Span::styled(input.to_owned(), t.text())),
    }
    if let Some(g) = &ghost {
        spans.push(Span::styled(g.clone(), Style::default().fg(t.text_muted)));
    }

    (
        format!(" {} ", mode.label()),
        Line::from(spans),
        ghost,
    )
}

/// The bordered block to render the input inside.
pub fn block(focus_input: bool, mode: AppMode) -> Block<'static> {
    let t = theme::active();
    let border_color = if !focus_input {
        t.border_default
    } else {
        match mode {
            AppMode::Normal => t.accent_primary,
            AppMode::Agent => t.accent_secondary,
            AppMode::Review => t.accent_warning,
            AppMode::Plan => t.accent_success,
        }
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_slash_prefixes() {
        assert_eq!(completion("/hel"), Some("/help"));
        assert_eq!(completion("/help"), None); // exact match → no ghost
        assert_eq!(completion("hello"), None);
        // A space means the command is already typed; no completion.
        assert_eq!(completion("/tools "), None);
    }

    #[test]
    fn ghost_is_the_untyped_remainder() {
        let (_, _, ghost) = build(AppMode::Normal, true, "/cle");
        assert_eq!(ghost.as_deref(), Some("ar"));
    }
}
