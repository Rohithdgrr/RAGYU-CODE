//! Main render pass: status bar → chat → tree/tools sidebars → input.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::app::{AppMode, Focus, Tui};
use super::layout::PaneLayout;
use super::theme;
use super::widgets::{chat_pane, input_bar, status_bar};

/// Per-frame snapshot of session facts. Owned data so a running turn (which
/// holds `&mut App`) can't fight the draw loop.
#[derive(Clone)]
pub struct StatusInfo {
    pub provider: String,
    pub model: String,
    pub tokens: usize,
    pub budget: usize,
    pub turns: u32,
    pub errors: u32,
    pub avg_latency_ms: u64,
    pub tools: Vec<ToolRowInfo>,
    pub git_branch: Option<String>,
    pub git_dirty: bool,
    pub pinned: usize,
    pub focus: Focus,
    pub busy: bool,
}

#[derive(Clone)]
pub struct ToolRowInfo {
    pub name: String,
    pub enabled: bool,
    pub gated: bool,
}

pub fn draw(f: &mut Frame<'_>, tui: &Tui, info: &StatusInfo) {
    let layout = PaneLayout::compute(f.area(), tui.show_tree, tui.show_tools);

    render_status(f, layout.status, tui.mode, info);
    render_chat(f, layout.chat, tui);

    if let Some(rect) = layout.tree
        && let Some(tree) = &tui.tree
    {
        render_tree(f, rect, tree, tui.focus == Focus::Tree, " PROJECT ");
    }
    if let Some(rect) = layout.tools {
        if let Some(tree) = &tui.tree {
            // Right sidebar now hosts file explorer (replaced tools panel)
            render_tree(f, rect, tree, tui.focus == Focus::Tree, " FILES ");
        } else {
            render_explorer_placeholder(f, rect);
        }
    }

    render_input(f, layout.input, tui);
}

fn render_status(f: &mut Frame<'_>, area: Rect, mode: AppMode, info: &StatusInfo) {
    // Rich top nav — passes full info + width for responsive truncation.
    let line = status_bar::build_rich(mode, info, area.width);
    f.render_widget(Paragraph::new(line), area);
}

fn render_chat(f: &mut Frame<'_>, area: Rect, tui: &Tui) {
    let t = theme::active();
    let streaming = tui.streaming.borrow().clone();
    let lines = chat_pane::build_lines(
        &tui.entries,
        Some(&streaming),
        tui.busy,
        area.width.saturating_sub(2),
    );

    // Clamp scroll so we never show blank space above the first line.
    let visible = area.height as usize;
    let max_scroll = lines.len().saturating_sub(visible);
    let scroll = tui.scroll_from_bottom.min(max_scroll);
    let start = lines.len().saturating_sub(visible + scroll);
    let slice: Vec<Line<'static>> =
        lines[start..start + visible.min(lines.len() - start)].to_vec();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(t.border_style(tui.focus == Focus::Chat))
        .border_type(ratatui::widgets::BorderType::Rounded)
        .style(t.text());
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(slice), inner);
}

fn render_tree(
    f: &mut Frame<'_>,
    area: Rect,
    tree: &crate::tui::widgets::file_tree::FileTree,
    focused: bool,
    title: &str,
) {
    let t = theme::active();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(t.border_style(focused))
        .border_type(ratatui::widgets::BorderType::Rounded)
        .title_style(Style::default().fg(t.accent_secondary).add_modifier(Modifier::BOLD))
        .style(t.sidebar_bg());
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Tell the tree how tall it can draw so scrolling stays exact.
    tree.set_view_height(inner.height);
    f.render_widget(
        Paragraph::new(tree.render_lines_with_width(focused, inner.width)),
        inner,
    );
}

fn render_explorer_placeholder(f: &mut Frame<'_>, area: Rect) {
    let t = theme::active();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" FILES ")
        .border_style(t.border_style(false))
        .border_type(ratatui::widgets::BorderType::Rounded)
        .title_style(Style::default().fg(t.accent_secondary).add_modifier(Modifier::BOLD))
        .style(t.sidebar_bg());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let hint = vec![
        Line::styled("  No files yet", Style::default().fg(t.text_muted).bg(t.bg_secondary)),
        Line::default(),
        Line::styled("  Press Ctrl+P", Style::default().fg(t.text_secondary).bg(t.bg_secondary).add_modifier(Modifier::BOLD)),
        Line::styled("  or Ctrl+T to", Style::default().fg(t.text_muted).bg(t.bg_secondary)),
        Line::styled("  open explorer", Style::default().fg(t.text_muted).bg(t.bg_secondary)),
    ];
    f.render_widget(Paragraph::new(hint), inner);
}

fn render_input(f: &mut Frame<'_>, area: Rect, tui: &Tui) {
    let t = theme::active();
    if area.height < 3 {
        // Compact single-row input for tiny terminals.
        let (_, line, _) = input_bar::build(tui.mode, true, &tui.input);
        // give compact a floating tint too
        let bg = Style::default().bg(t.bg_tertiary).fg(t.text_primary);
        f.render_widget(Paragraph::new(line).style(bg), area);
        place_cursor(f, area.x + 2, area.y, &tui.input, tui.input_cursor);
        return;
    }

    let (_, line, ghost) = input_bar::build(tui.mode, tui.focus == Focus::Input, &tui.input);
    let has_ghost = ghost.is_some();
    let focus_input = tui.focus == Focus::Input;

    // Modern floating card: header chip + footer rail + inner input line
    let header = input_bar::header_line(tui.mode, focus_input);
    let footer = input_bar::footer_hint(has_ghost, focus_input, tui.confirm_pending);

    // Cozy 3-row vs rich 5-row Composer
    // 3-row: header sits on top border, footer on bottom border.
    // 5-row: we add an extra inner gutter line and center the input vertically.
    let mut block = input_bar::block(focus_input, tui.mode)
        .title(header)
        .title_bottom(footer);

    // Right-side pinned badge when 5-row gives us room
    if area.height >= 5
        && let Some(suffix) = input_bar::header_suffix(tui.pinned_files.len())
    {
        block = block.title_top(suffix);
    }

    let inner = block.inner(area);
    f.render_widget(block, area);

    if area.height >= 5 {
        // Rich: input centered on inner row 1 of 3 (borders already take 2)
        // inner.height == 3 when area.height ==5
        let input_y = inner.y + 1;
        // light divider lines above/below input for depth (render as styled spans)
        let divider = ratatui::text::Line::styled(
            "─".repeat(inner.width as usize),
            Style::default().fg(t.border_default).bg(t.bg_tertiary),
        );
        // top gutter (row 0): subtle hint when idle — show context status
        let gutter_line = if tui.busy {
            Line::from(vec![
                ratatui::text::Span::styled(
                    "  ⏺ streaming…  ",
                    Style::default().fg(t.accent_primary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD),
                ),
                ratatui::text::Span::styled(
                    format!("{} pinned", tui.pinned_files.len()),
                    Style::default().fg(t.text_muted).bg(t.bg_tertiary),
                ),
            ])
        } else if !focus_input {
            Line::styled(
                format!("  {}", status_bar::focus_hint(tui.focus)),
                Style::default().fg(t.text_muted).bg(t.bg_tertiary),
            )
        } else {
            Line::styled(" ", Style::default().bg(t.bg_tertiary))
        };
        f.render_widget(Paragraph::new(gutter_line).style(Style::default().bg(t.bg_tertiary)), Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 });
        f.render_widget(Paragraph::new(line).style(Style::default().bg(t.bg_tertiary)), Rect { x: inner.x, y: input_y, width: inner.width, height: 1 });
        // bottom gutter already is the footer rail (block title), keep empty bottom line for breathing
        let bottom_gutter = Line::styled(" ", Style::default().bg(t.bg_tertiary));
        f.render_widget(Paragraph::new(bottom_gutter), Rect { x: inner.x, y: inner.y + 2, width: inner.width, height: 1 });
        let _ = divider; // reserved for future separator styling
        if focus_input && !tui.confirm_pending {
            place_cursor(f, inner.x, input_y, &tui.input, tui.input_cursor);
        }
    } else {
        // Cozy single-line within 3-row card
        f.render_widget(Paragraph::new(line).style(Style::default().bg(t.bg_tertiary)), inner);
        if focus_input && !tui.confirm_pending {
            place_cursor(f, inner.x, inner.y, &tui.input, tui.input_cursor);
        }
    }

    // — Slash palette dropdown: filtered "/" commands above input — scrollable, shows all
    if focus_input && tui.input.starts_with('/') && !tui.input.contains(' ') {
        let hits = input_bar::filtered(&tui.input);
        if !hits.is_empty() {
            let lines = input_bar::palette_lines(&tui.input, tui.slash_selected);
            if !lines.is_empty() {
                let pal_h = (lines.len() as u16 + 2).min(18);
                let pal_w = area.width.saturating_sub(2).min(56);
                let pal_x = area.x;
                // place just above input block
                let pal_y = area.y.saturating_sub(pal_h);
                let pal_rect = Rect {
                    x: pal_x,
                    y: pal_y.max(1),
                    width: pal_w,
                    height: pal_h,
                };
                let pal_block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(t.accent_primary).bg(t.bg_tertiary))
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .title(" ⌘ COMMANDS ")
                    .title_style(
                        Style::default()
                            .fg(t.accent_primary)
                            .bg(t.bg_tertiary)
                            .add_modifier(Modifier::BOLD),
                    )
                    .style(Style::default().bg(t.bg_tertiary));
                let pal_inner = pal_block.inner(pal_rect);
                f.render_widget(pal_block, pal_rect);
                // height may be less than lines if clamped, slice
                let slice: Vec<Line<'static>> = lines.into_iter().take(pal_inner.height as usize).collect();
                f.render_widget(Paragraph::new(slice).style(Style::default().bg(t.bg_tertiary)), pal_inner);
            }
        }
    }
}

/// Positions the terminal cursor after the `❯ ` prompt at `x`.
fn place_cursor(f: &mut Frame<'_>, x: u16, y: u16, input: &str, cursor_chars: usize) {
    let before: String = input.chars().take(cursor_chars).collect();
    let col = x.saturating_add(before.width().min(200) as u16);
    f.set_cursor_position((col, y));
}
