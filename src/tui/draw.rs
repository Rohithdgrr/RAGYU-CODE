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
        render_tree(f, rect, tree, tui.focus == Focus::Tree, " PROJECT ", tui.tree_hover);
    }
    if let Some(rect) = layout.tools {
        if let Some(tree) = &tui.tree {
            // Right sidebar now hosts file explorer (replaced tools panel)
            render_tree(f, rect, tree, tui.focus == Focus::Tree, " FILES ", tui.tree_hover);
        } else {
            render_explorer_placeholder(f, rect);
        }
    }

    render_input(f, layout.input, tui);

    // Centered dialog for slash command args (after clicking palette)
    if let Some(dialog) = &tui.slash_dialog {
        render_slash_dialog(f, f.area(), dialog);
    }
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
    hovered: Option<usize>,
) {
    let t = theme::active();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} {} ", super::icons::TREE_TITLE, title.trim()))
        .border_style(t.border_style(focused))
        .title_style(Style::default().fg(t.accent_secondary).bg(t.bg_primary).add_modifier(Modifier::BOLD))
        .style(t.sidebar_bg());
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Tell the tree how tall it can draw so scrolling stays exact.
    tree.set_view_height(inner.height);
    f.render_widget(
        Paragraph::new(tree.render_lines_hover(focused, inner.width, hovered)),
        inner,
    );
}

fn render_explorer_placeholder(f: &mut Frame<'_>, area: Rect) {
    let t = theme::active();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} FILES ", super::icons::FILES_TITLE))
        .border_style(t.border_style(false))
        .title_style(Style::default().fg(t.accent_secondary).bg(t.bg_primary).add_modifier(Modifier::BOLD))
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
            // animated pulse: cycling braille spinner + LIVE chip
            let frame = tick_frame(4);
            let dots = ["   ", ".  ", ".. ", "..."][frame];
            Line::from(vec![
                ratatui::text::Span::styled(
                    format!("  {} streaming{dots}  ", super::icons::LIVE),
                    Style::default().fg(t.accent_primary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD),
                ),
                ratatui::text::Span::styled(
                    format!("{} {} pinned", super::icons::PINNED, tui.pinned_files.len()),
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
            let lines = input_bar::palette_lines(&tui.input, tui.slash_selected, tui.palette_hover);
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
                    .border_style(Style::default().fg(t.border_focus).bg(t.bg_tertiary))
                    .title(format!(" {} COMMANDS ", super::icons::COMMANDS_TITLE))
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

fn render_slash_dialog(f: &mut Frame<'_>, area: Rect, dialog: &crate::tui::app::SlashDialog) {
    let t = theme::active();
    let has_models = !dialog.models.is_empty();
    // Dynamic height: base 9 rows + model list (up to 8 visible).
    let model_visible = if has_models { dialog.models.len().min(8) as u16 } else { 0 };
    let w: u16 = 60;
    let h: u16 = if has_models { 9 + model_visible + 1 } else { 9 };
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect::new(
        x,
        y,
        w.min(area.width.saturating_sub(2)),
        h.min(area.height.saturating_sub(2)),
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.border_focus).bg(t.bg_tertiary))
        .title(format!(
            " {} {} ",
            super::icons::command(&dialog.command),
            dialog.command
        ))
        .title_style(
            Style::default()
                .fg(t.text_inverse)
                .bg(t.accent_primary)
                .add_modifier(Modifier::BOLD),
        )
        .style(Style::default().bg(t.bg_tertiary));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    // desc
    let desc_line = Line::styled(
        format!(" {}", dialog.desc),
        Style::default()
            .fg(t.text_muted)
            .bg(t.bg_tertiary)
            .add_modifier(Modifier::ITALIC),
    );
    let input_line = {
        let prompt = "> ";
        let before: String = dialog.arg_input.chars().take(dialog.arg_cursor).collect();
        let after: String = dialog.arg_input.chars().skip(dialog.arg_cursor).collect();
        let base = Style::default().fg(t.text_primary).bg(t.bg_tertiary);
        if after.is_empty() {
            Line::from(vec![
                ratatui::text::Span::styled(
                    prompt,
                    Style::default()
                        .fg(t.accent_primary)
                        .bg(t.bg_tertiary)
                        .add_modifier(Modifier::BOLD),
                ),
                ratatui::text::Span::styled(before, base),
                ratatui::text::Span::styled(
                    "\u{258c}",
                    Style::default()
                        .fg(t.accent_primary)
                        .bg(t.bg_tertiary),
                ),
                ratatui::text::Span::styled(
                    if has_models {
                        "  type to filter or use \u{2195}\u{2191}"
                    } else {
                        "  args (optional)"
                    },
                    Style::default()
                        .fg(t.text_muted)
                        .bg(t.bg_tertiary)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        } else {
            Line::from(vec![
                ratatui::text::Span::styled(
                    prompt,
                    Style::default()
                        .fg(t.accent_primary)
                        .bg(t.bg_tertiary)
                        .add_modifier(Modifier::BOLD),
                ),
                ratatui::text::Span::styled(before, base),
                ratatui::text::Span::styled(
                    after.chars()
                        .next()
                        .map(|c| c.to_string())
                        .unwrap_or_default(),
                    Style::default()
                        .fg(t.text_primary)
                        .bg(t.bg_hover)
                        .add_modifier(Modifier::BOLD),
                ),
                ratatui::text::Span::styled(
                    after.chars().skip(1).collect::<String>(),
                    base,
                ),
            ])
        }
    };
    let footer = Line::from(vec![
        ratatui::text::Span::styled(
            " Enter ",
            Style::default()
                .fg(t.text_inverse)
                .bg(t.accent_success)
                .add_modifier(Modifier::BOLD),
        ),
        ratatui::text::Span::styled(
            " execute  ",
            Style::default()
                .fg(t.text_secondary)
                .bg(t.bg_tertiary),
        ),
        ratatui::text::Span::styled(
            " Esc ",
            Style::default()
                .fg(t.text_inverse)
                .bg(t.border_default)
                .add_modifier(Modifier::BOLD),
        ),
        ratatui::text::Span::styled(
            " cancel ",
            Style::default()
                .fg(t.text_muted)
                .bg(t.bg_tertiary),
        ),
    ]);
    let mut lines: Vec<Line<'static>> = vec![Line::default(), desc_line, Line::default(), input_line];
    // Model list section (scrollable, up to 8 visible rows).
    if has_models {
        let total = dialog.models.len();
        let max_show = 8usize.min(total);
        let sel = dialog.models_selected.min(total.saturating_sub(1));
        // Window so the selected model is always visible.
        let start = sel
            .saturating_sub(max_show / 2)
            .min(total.saturating_sub(max_show));
        let end = (start + max_show).min(total);
        lines.push(Line::default());
        // Header with scroll info
        let header = if total > max_show {
            format!(" {start}\u{2013}{end} / {total} models ")
        } else {
            format!(" {total} model(s) ")
        };
        lines.push(Line::styled(
            header,
            Style::default()
                .fg(t.text_muted)
                .bg(t.bg_tertiary)
                .add_modifier(Modifier::BOLD),
        ));
        if start > 0 {
            lines.push(Line::styled(
                format!("  \u{f077} {start} more"),
                Style::default()
                    .fg(t.text_muted)
                    .bg(t.bg_tertiary)
                    .add_modifier(Modifier::ITALIC),
            ));
        }
        for idx in start..end {
            let is_sel = idx == sel;
            let name = &dialog.models[idx];
            let marker = if is_sel { "\u{25b8}" } else { " " };
            let bg = if is_sel { t.bg_hover } else { t.bg_tertiary };
            let fg = if is_sel {
                t.accent_primary
            } else {
                t.text_secondary
            };
            let mut style = Style::default().fg(fg).bg(bg);
            if is_sel {
                style = style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            }
            lines.push(Line::from(vec![
                ratatui::text::Span::styled(
                    format!("{marker} "),
                    Style::default().fg(t.accent_primary).bg(bg),
                ),
                ratatui::text::Span::styled(name.clone(), style),
            ]));
        }
        if end < total {
            lines.push(Line::styled(
                format!("  \u{f078} {} more", total - end),
                Style::default()
                    .fg(t.text_muted)
                    .bg(t.bg_tertiary)
                    .add_modifier(Modifier::ITALIC),
            ));
        }
    }
    lines.push(Line::default());
    lines.push(footer);
    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(t.bg_tertiary)),
        inner,
    );
    // place cursor inside dialog input
    let prompt_w: u16 = 2; // "> " width
    let before_w = before_width(&dialog.arg_input, dialog.arg_cursor);
    let cx = inner.x + prompt_w + before_w;
    let cy = inner.y + 3; // input line is 4th line (0-indexed 3)
    f.set_cursor_position((cx.min(inner.x + inner.width.saturating_sub(1)), cy));
}

fn before_width(s: &str, cursor: usize) -> u16 {
    let before: String = s.chars().take(cursor).collect();
    before.width() as u16
}

/// Positions the terminal cursor after the `❯ ` prompt at `x`.
fn place_cursor(f: &mut Frame<'_>, x: u16, y: u16, input: &str, cursor_chars: usize) {
    let before: String = input.chars().take(cursor_chars).collect();
    let col = x.saturating_add(before.width().min(200) as u16);
    f.set_cursor_position((col, y));
}

/// Frame counter for subtle animations — driven by the 250ms event-loop tick.
pub fn tick_frame(modulo: usize) -> usize {
    if modulo == 0 {
        return 0;
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| (d.as_millis() / 250) as usize % modulo)
}
