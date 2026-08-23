//! Main render pass: status bar → chat → tree/tools sidebars → input.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::app::{AppMode, Focus, Tui};
use super::layout::PaneLayout;
use super::theme;
use super::widgets::{chat_pane, input_bar, status_bar, tool_panel};

/// Per-frame snapshot of session facts. Owned data so a running turn (which
/// holds `&mut App`) can't fight the draw loop.
pub struct StatusInfo {
    pub provider: String,
    pub model: String,
    pub tokens: usize,
    pub budget: usize,
    pub turns: u32,
    pub errors: u32,
    pub avg_latency_ms: u64,
    pub tools: Vec<ToolRowInfo>,
}

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
        render_tree(f, rect, tree, tui.focus == Focus::Tree);
    }
    if let Some(rect) = layout.tools {
        render_tools(f, rect, info, tui);
    }

    render_input(f, layout.input, tui);
}

fn render_status(f: &mut Frame<'_>, area: Rect, mode: AppMode, info: &StatusInfo) {
    let line = status_bar::build_line(
        mode,
        &info.provider,
        &info.model,
        info.tokens,
        info.budget,
    );
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
        .style(t.text());
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(slice), inner);
}

fn render_tree(f: &mut Frame<'_>, area: Rect, tree: &crate::tui::widgets::file_tree::FileTree, focused: bool) {
    let t = theme::active();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" PROJECT ")
        .border_style(t.border_style(focused))
        .style(t.sidebar_bg());
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Tell the tree how tall it can draw so scrolling stays exact.
    tree.set_view_height(inner.height);
    f.render_widget(Paragraph::new(tree.render_lines(focused)), inner);
}

fn render_tools(f: &mut Frame<'_>, area: Rect, info: &StatusInfo, tui: &Tui) {
    let t = theme::active();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(if tui.confirm_pending {
            " TOOLS · REVIEW "
        } else {
            " TOOLS "
        })
        .border_style(t.border_style(false))
        .title_style(Style::default().fg(if tui.confirm_pending {
            t.accent_warning
        } else {
            t.text_muted
        }))
        .style(t.sidebar_bg());
    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows: Vec<tool_panel::ToolRow> = info
        .tools
        .iter()
        .map(|r| tool_panel::ToolRow {
            name: r.name.clone(),
            enabled: r.enabled,
            gated: r.gated,
        })
        .collect();
    let activity = tool_panel::activity_from_entries(&tui.entries, 12);
    let lines =
        tool_panel::build_lines(&rows, &activity, info.turns, info.errors, info.avg_latency_ms);
    f.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
        inner,
    );
}

fn render_input(f: &mut Frame<'_>, area: Rect, tui: &Tui) {
    let t = theme::active();
    if area.height < 3 {
        // Compact single-row input for tiny terminals.
        let (_, line, _) = input_bar::build(tui.mode, true, &tui.input);
        f.render_widget(Paragraph::new(line), area);
        place_cursor(f, area.x + 2, area.y, &tui.input, tui.input_cursor);
        return;
    }

    let (_, line, ghost) = input_bar::build(tui.mode, tui.focus == Focus::Input, &tui.input);
    // Ghost completion hint rides in the bottom border corner.
    let hint = match (&ghost, tui.focus) {
        (Some(_), Focus::Input) => " Tab complete ".to_owned(),
        _ => format!(" {} ", status_bar::focus_hint(tui.focus)),
    };
    let block = input_bar::block(tui.focus == Focus::Input, tui.mode)
        .title_bottom(hint)
        .title_style(Style::default().fg(t.text_muted));
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(line), inner);

    if tui.focus == Focus::Input && !tui.confirm_pending {
        place_cursor(f, inner.x, inner.y, &tui.input, tui.input_cursor);
    }
}

/// Positions the terminal cursor after the `❯ ` prompt at `x`.
fn place_cursor(f: &mut Frame<'_>, x: u16, y: u16, input: &str, cursor_chars: usize) {
    let before: String = input.chars().take(cursor_chars).collect();
    let col = x.saturating_add(before.width().min(200) as u16);
    f.set_cursor_position((col, y));
}
