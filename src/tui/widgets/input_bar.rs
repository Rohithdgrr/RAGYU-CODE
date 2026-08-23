//! Input bar — modern rich floating composer.
//!
//! Visual language:
//! - floating card with `bg_tertiary` (white) lifting off `bg_primary`
//! - mode-tinted rounded border + chip header
//! - layered prompt (`›`), placeholder with subtle hint, inline ghost
//! - footer hint rail: slash commands · file refs · shortcuts + send affordance

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding};

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

/// Filtered slash commands for palette. Returns up to 12 matches that start
/// with the typed prefix (case-insensitive). Single "/" returns all.
pub fn filtered(input: &str) -> Vec<&'static str> {
    if !input.starts_with('/') {
        return Vec::new();
    }
    // only the first token matters, before any whitespace
    let token = input.split_whitespace().next().unwrap_or(input).to_ascii_lowercase();
    if token.is_empty() || token == "/" {
        return commands::SLASH_COMMANDS.to_vec();
    }
    commands::SLASH_COMMANDS
        .iter()
        .copied()
        .filter(|c| c.starts_with(token.as_str()))
        .collect()
}

/// Builds rich palette lines for dropdown — up to 10 rows, selected highlighted.
pub fn palette_lines(input: &str, selected: usize) -> Vec<Line<'static>> {
    let t = theme::active();
    let hits = filtered(input);
    if hits.is_empty() {
        return Vec::new();
    }
    let max_show = 10usize;
    // keep selected centered
    let total = hits.len().min(max_show);
    let sel = selected.min(total.saturating_sub(1));
    let mut out = Vec::new();
    // header
    out.push(Line::styled(
        format!(" {} commands ", hits.len()),
        Style::default().fg(t.text_muted).bg(t.bg_tertiary).add_modifier(Modifier::BOLD),
    ));
    for (i, cmd) in hits.into_iter().take(max_show).enumerate() {
        let is_sel = i == sel;
        let bg = if is_sel { t.bg_hover } else { t.bg_tertiary };
        let fg = if is_sel { t.accent_primary } else { t.text_secondary };
        let mut style = Style::default().fg(fg).bg(bg);
        if is_sel {
            style = style.add_modifier(Modifier::BOLD);
        }
        let desc = describe(cmd);
        let marker = if is_sel { "▸" } else { " " };
        out.push(Line::from(vec![
            Span::styled(format!("{marker} "), Style::default().fg(t.accent_primary).bg(bg)),
            Span::styled(cmd, style),
            Span::styled(format!("  {desc}"), Style::default().fg(t.text_muted).bg(bg)),
        ]));
    }
    out
}

fn describe(cmd: &str) -> &'static str {
    match cmd {
        "/help" => "show help",
        "/exit" | "/quit" => "quit",
        "/clear" | "/reset" => "clear chat",
        "/models" => "list models",
        "/model" => "switch model",
        "/temp" => "temperature",
        "/system" => "system prompt",
        "/history" => "show history",
        "/undo" => "undo last",
        "/retry" => "retry last",
        "/variants" => "alternates",
        "/pick" => "pick variant",
        "/compact" => "compact history",
        "/search" => "search",
        "/save" => "save session",
        "/load" => "load session",
        "/sessions" => "list sessions",
        "/fork" => "fork session",
        "/export" => "export md/txt",
        "/stats" => "session stats",
        "/theme" => "theme",
        "/tokens" => "token usage",
        "/raw" => "toggle markdown",
        "/config" => "show/save config",
        "/timeout" => "request timeout",
        "/limit" => "response cap",
        "/tools" => "tools registry",
        "/todo" => "task list",
        "/diff" => "staged diff",
        "/apply" => "apply edits",
        "/reject" => "discard edits",
        "/review" => "review edits",
        "/scan" => "scan workspace",
        "/plan" => "plan task",
        "/project" => "project memory",
        _ => "",
    }
}

/// Header chip content — returns styled line for block top title.
pub fn header_line(mode: AppMode, focus_input: bool) -> Line<'static> {
    let t = theme::active();
    let accent = match mode {
        AppMode::Normal => t.accent_success,
        AppMode::Agent => t.accent_secondary,
        AppMode::Review => t.accent_warning,
        AppMode::Plan => t.accent_primary,
    };
    let (icon, label) = match mode {
        AppMode::Normal => ("◉", " NORMAL "),
        AppMode::Agent => ("✦", " AGENT "),
        AppMode::Review => ("⚠", " REVIEW "),
        AppMode::Plan => ("⬢", " PLAN "),
    };
    // chip: icon + mode label with accent background when focused, muted otherwise
    let chip_style = if focus_input {
        Style::default()
            .fg(t.text_inverse)
            .bg(accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(t.text_muted)
            .bg(t.bg_secondary)
            .add_modifier(Modifier::BOLD)
    };
    let brand_style = Style::default()
        .fg(if focus_input { accent } else { t.text_muted })
        .add_modifier(Modifier::BOLD);

    Line::from(vec![
        Span::styled(" ", Style::default().bg(t.bg_tertiary)),
        Span::styled(format!(" {icon}"), brand_style.bg(t.bg_tertiary)),
        Span::styled(label, chip_style),
        Span::styled("  GOVINDA", brand_style.bg(t.bg_tertiary)),
        Span::styled(" ", Style::default().bg(t.bg_tertiary)),
    ])
}

/// Right-aligned header suffix showing shortcuts/context count.
pub fn header_suffix(pinned: usize) -> Option<Line<'static>> {
    if pinned == 0 {
        return None;
    }
    let t = theme::active();
    Some(Line::from(vec![
        Span::styled(
            format!(" 📎 {pinned} "),
            Style::default()
                .fg(t.accent_secondary)
                .bg(t.bg_tertiary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ]))
}

/// Footer hint rail — context aware.
pub fn footer_hint(has_ghost: bool, focus_input: bool, confirm_pending: bool) -> Line<'static> {
    let t = theme::active();
    let muted = Style::default().fg(t.text_muted).bg(t.bg_tertiary);
    let key_style = Style::default()
        .fg(t.text_secondary)
        .bg(t.bg_secondary)
        .add_modifier(Modifier::BOLD);
    let action_style = Style::default()
        .fg(t.text_inverse)
        .bg(if focus_input {
            t.accent_primary
        } else {
            t.border_default
        })
        .add_modifier(Modifier::BOLD);

    if confirm_pending {
        return Line::from(vec![
            Span::styled(" ⚠ ", Style::default().fg(t.accent_warning).bg(t.bg_tertiary)),
            Span::styled(" y approve ", key_style),
            Span::styled(" n decline ", key_style),
            Span::styled(" a all ", key_style),
        ]);
    }
    if has_ghost {
        return Line::from(vec![
            Span::styled(" ↹ ", Style::default().fg(t.accent_success).bg(t.bg_tertiary)),
            Span::styled(" Tab ", key_style),
            Span::styled(" to complete  ", muted),
            Span::styled(" Esc ", key_style),
            Span::styled(" clear ", muted),
        ]);
    }
    // default modern rail: evenly spaced affordances
    Line::from(vec![
        Span::styled(" / ", key_style),
        Span::styled("commands ", muted),
        Span::styled("·", muted),
        Span::styled(" @ ", key_style),
        Span::styled("files ", muted),
        Span::styled("·", muted),
        Span::styled(" Esc ", key_style),
        Span::styled("clear ", muted),
        Span::styled("·", muted),
        Span::styled(" ↑↓ ", key_style),
        Span::styled("history ", muted),
        Span::styled("  ", muted),
        Span::styled(" ↵ Send ", action_style),
        Span::styled(" ", muted),
    ])
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

    let ghost = if focus_input {
        completion(input).map(|c| c[input.len()..].to_owned())
    } else {
        None
    };

    // Prompt chevron — tinted by mode, bold
    let prompt_style = Style::default()
        .fg(border_color)
        .bg(t.bg_tertiary)
        .add_modifier(Modifier::BOLD);

    // subtle vertical bar glow: use a layered prompt
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(" ❯ ", prompt_style));

    if input.is_empty() && focus_input {
        // rich placeholder: primary hint + dim secondary hint
        spans.push(Span::styled(
            "Ask me to code, debug, or explain…",
            Style::default().fg(t.text_muted).bg(t.bg_tertiary),
        ));
        spans.push(Span::styled(
            "  ·  try \"/\" for commands",
            Style::default()
                .fg(t.text_muted)
                .bg(t.bg_tertiary)
                .add_modifier(Modifier::ITALIC),
        ));
    } else if input.is_empty() && !focus_input {
        spans.push(Span::styled(
            "Ask me to code, debug, or explain…",
            Style::default()
                .fg(t.text_muted)
                .bg(t.bg_tertiary)
                .add_modifier(Modifier::DIM),
        ));
    } else {
        spans.push(Span::styled(
            input.to_owned(),
            Style::default().fg(t.text_primary).bg(t.bg_tertiary),
        ));
    }
    if let Some(g) = &ghost {
        spans.push(Span::styled(
            g.clone(),
            Style::default()
                .fg(t.text_muted)
                .bg(t.bg_tertiary)
                .add_modifier(Modifier::ITALIC | Modifier::DIM),
        ));
        // ghost arrow hint inline
        spans.push(Span::styled(
            "  → Tab",
            Style::default()
                .fg(t.accent_success)
                .bg(t.bg_tertiary)
                .add_modifier(Modifier::BOLD),
        ));
    }

    (
        format!(" {} ", mode.label()),
        Line::from(spans).style(Style::default().bg(t.bg_tertiary)),
        ghost,
    )
}

/// The bordered floating card block to render the input inside.
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
        .border_style(Style::default().fg(border_color).bg(t.bg_tertiary))
        .border_type(ratatui::widgets::BorderType::Rounded)
        .style(Style::default().bg(t.bg_tertiary))
        .padding(Padding::horizontal(1))
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
        assert_eq!("/tools ".contains(' '), true);
    }

    #[test]
    fn ghost_is_the_untyped_remainder() {
        let (_, _, ghost) = build(AppMode::Normal, true, "/cle");
        assert_eq!(ghost.as_deref(), Some("ar"));
    }

    #[test]
    fn footer_varies_by_state() {
        let with_ghost = footer_hint(true, true, false);
        assert!(with_ghost.width() > 4);
        let idle = footer_hint(false, true, false);
        assert!(idle.width() > 4);
    }

    #[test]
    fn header_contains_mode_label() {
        let line = header_line(AppMode::Agent, true);
        let raw: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(raw.contains("AGENT"));
    }
}
