//! Top status bar: version, agent mode chip, provider/model, token budget.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::super::app::{AppMode, Focus};
use super::super::theme;

/// Renders the 1-row status line content.
pub fn build_line(mode: AppMode, provider: &str, model: &str, tokens: usize, budget: usize) -> Line<'static> {
    let t = theme::active();
    let mode_style = Style::default()
        .fg(t.text_inverse)
        .bg(match mode {
            AppMode::Normal => t.accent_primary,
            AppMode::Agent => t.accent_secondary,
            AppMode::Review => t.accent_warning,
            AppMode::Plan => t.accent_success,
        })
        .add_modifier(Modifier::BOLD);

    let left = vec![
        Span::styled(
            format!(" govinda-cli v{}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(t.text_secondary),
        ),
        Span::raw("  "),
        Span::styled(format!(" {} ", mode.label()), mode_style),
    ];

    let usage = if budget > 0 {
        format!("~{tokens}/{budget} tok")
    } else {
        format!("~{tokens} tok")
    };
    let over = tokens > budget;
    let right = vec![
        Span::styled(
            format!("{provider} · {model} "),
            Style::default().fg(t.text_muted),
        ),
        Span::styled(
            format!("{usage} "),
            Style::default().fg(if over { t.accent_error } else { t.text_secondary }),
        ),
    ];

    let mut spans = left;
    spans.extend(right);
    Line::from(spans).style(Style::default().bg(t.bg_secondary))
}

/// Focus hint shown at far-left of the input bar title.
pub fn focus_hint(focus: Focus) -> &'static str {
    match focus {
        Focus::Input => "type · Tab focus chat",
        Focus::Chat => "scroll · Tab focus input",
        Focus::Tree => "↑↓ navigate · Enter pin file · Space expand",
    }
}
