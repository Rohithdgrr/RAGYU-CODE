//! Main render pass: status bar → chat → tree/tools sidebars → input.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Fill, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::app::{AppMode, Focus, Tui};
use super::layout::PaneLayout;
use super::theme;
use super::widgets::{chat_pane, input_bar, status_bar, todo_panel};

/// Fills an area with the backdrop color to make modals fully opaque.
fn clear_area(f: &mut Frame<'_>, area: Rect, theme: &theme::Theme) {
    f.render_widget(
        Paragraph::new(""),
        area,
    );
    // Use Fill to paint every cell with the backdrop background.
    f.render_widget(Fill::new(" ").style(Style::default().bg(theme.bg_tertiary)), area);
}

/// Per-frame snapshot of session facts. Owned data so a running turn (which
/// holds `&mut App`) can't fight the draw loop.
#[derive(Clone)]
pub struct StatusInfo {
    pub provider: String,
    pub model: String,
    pub provider_name: String,
    pub tokens: usize,
    /// CLI token budget (from `context_tokens` in TOML or auto-detected).
    /// This is what the trimmer uses when sending messages to the API.
    pub budget: usize,
    /// Model's *actual* input context window (from the provider's
    /// `/v1/models` endpoint or the static registry). When this differs
    /// significantly from `budget`, the bar shows both so the user can
    /// see they're not actually running out of room.
    pub model_context: usize,
    pub turns: u32,
    pub errors: u32,
    pub avg_latency_ms: u64,
    pub tools: Vec<ToolRowInfo>,
    pub todos: Vec<crate::commands::todo::Todo>,
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
        render_todo(f, rect, info, tui.focus == Focus::Tree);
    }

    render_input(f, layout.input, tui);

    // Focused pane glow beam — 1px cobalt top-edge that follows mouse X.
    let beam_pane = match tui.focus {
        Focus::Chat => Some(layout.chat),
        Focus::Tree => layout.tree.or(layout.tools),
        Focus::Input => Some(layout.input),
    };
    if let Some(area) = beam_pane {
        render_focus_beam(f, area, tui.mouse_x);
    }

    // Centered dialog for slash command args (after clicking palette)
    if let Some(dialog) = &tui.slash_dialog {
        render_slash_dialog(f, f.area(), dialog);
    }
    // Provider setup workflow modal.
    if let Some(ref wf) = tui.provider_workflow {
        render_provider_workflow(f, f.area(), wf);
    }

    // Cost dashboard modal (Ctrl+I)
    if tui.show_cost_dashboard {
        render_cost_dashboard(f, f.area(), info);
    }
    // Settings modal (Ctrl+,)
    if tui.show_settings {
        render_settings(f, f.area());
    }
    // Shortcuts modal (?)
    if tui.show_shortcuts {
        render_shortcuts(f, f.area());
    }
}

fn render_status(f: &mut Frame<'_>, area: Rect, mode: AppMode, info: &StatusInfo) {
    let t = theme::active();
    let line = status_bar::build_rich(mode, info, area.width.saturating_sub(2));
    if area.height >= 3 {
        // Frosted glass rail — sharp edges, solid glass with hairline border
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.border_default).bg(t.bg_tertiary))
            .style(Style::default().bg(t.bg_tertiary));
        let inner = block.inner(area);
        f.render_widget(block, area);
        // center vertically inside 3-row rail (inner height =1)
        let y = inner.y + inner.height.saturating_sub(1) / 2;
        let row = Rect { x: inner.x, y, width: inner.width, height: 1 };
        f.render_widget(Paragraph::new(line).style(Style::default().bg(t.bg_tertiary)), row);
    } else {
        // compact single-row fallback — still glassy
        f.render_widget(Paragraph::new(line).style(Style::default().bg(t.bg_tertiary)), area);
    }
}

fn render_chat(f: &mut Frame<'_>, area: Rect, tui: &Tui) {
    let t = theme::active();
    let streaming = tui.streaming.borrow().clone();
    let lines = chat_pane::build_lines(
        &tui.entries,
        Some(&streaming),
        tui.busy,
        area.width.saturating_sub(2),
        tui.raw_mode,
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
        .title(format!(" {} CHAT ", super::icons::CHAT))
        .title_style(
            Style::default()
                .fg(t.text_primary)
                .bg(t.bg_primary)
                .add_modifier(Modifier::BOLD),
        )
        // right-aligned hint rail, like the web mock
        .title(
            ratatui::text::Line::styled(
                " Ctrl+K focus · / commands · @ files ",
                Style::default()
                    .fg(t.text_muted)
                    .bg(t.bg_primary)
                    .add_modifier(Modifier::DIM),
            )
            .right_aligned(),
        )
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
    // file count badge — “▸ 24 files” on the right, like the web mock
    let file_count = tree.flat().iter().filter(|n| !n.is_dir).count();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} {} ", super::icons::TREE_TITLE, title.trim()))
        .title_style(Style::default().fg(t.accent_secondary).bg(t.bg_primary).add_modifier(Modifier::BOLD))
        .title(
            ratatui::text::Line::styled(
                format!(" ▸ {file_count} files "),
                Style::default().fg(t.text_muted).bg(t.bg_primary),
            )
            .right_aligned(),
        )
        .border_style(t.border_style(focused))
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

fn render_todo(f: &mut Frame<'_>, area: Rect, info: &StatusInfo, focused: bool) {
    let t = theme::active();
    let lines = todo_panel::build_lines(&info.todos, focused, area.width);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} TODO ", super::icons::TREE_TITLE))
        .border_style(t.border_style(focused))
        .title_style(Style::default().fg(t.accent_secondary).bg(t.bg_primary).add_modifier(Modifier::BOLD))
        .style(t.sidebar_bg());
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(lines).style(t.sidebar_bg()), inner);
}

#[allow(dead_code)]
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
            Line::from(vec![
                Span::styled("  /", Style::default().fg(t.accent_primary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD)),
                Span::styled(" commands", Style::default().fg(t.text_muted).bg(t.bg_tertiary)),
                Span::styled(" · @", Style::default().fg(t.accent_secondary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD)),
                Span::styled(" files", Style::default().fg(t.text_muted).bg(t.bg_tertiary)),
                Span::styled(" · ? shortcuts", Style::default().fg(t.text_muted).bg(t.bg_tertiary).add_modifier(Modifier::ITALIC)),
            ])
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
                clear_area(f, pal_rect, &t);
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

    // — @-mention file picker dropdown —
    if tui.at_picker_active && !tui.at_picker_files.is_empty() {
        let files = &tui.at_picker_files;
        let sel = tui.at_picker_selected;
        let total = files.len();
        let max_show = 8.min(total);
        let start = sel.saturating_sub(max_show / 2).min(total.saturating_sub(max_show));
        let end = (start + max_show).min(total);

        let mut picker_lines: Vec<Line<'static>> = Vec::new();
        // Header
        picker_lines.push(Line::styled(
            format!(" {} file(s) matching '@{}' ", total, tui.at_picker_query),
            Style::default().fg(t.text_muted).bg(t.bg_tertiary).add_modifier(Modifier::BOLD),
        ));
        // Files
        for (idx, file) in files.iter().enumerate().take(end).skip(start) {
            let is_sel = idx == sel;
            let marker = if is_sel { "▸" } else { " " };
            let bg = if is_sel { t.bg_hover } else { t.bg_tertiary };
            let fg = if is_sel { t.accent_primary } else { t.text_secondary };
            let mut style = Style::default().fg(fg).bg(bg);
            if is_sel {
                style = style.add_modifier(Modifier::BOLD);
            }
            picker_lines.push(Line::from(vec![
                ratatui::text::Span::styled(format!("{marker} "), Style::default().fg(t.accent_primary).bg(bg)),
                ratatui::text::Span::styled(format!(" {} ", super::icons::FILE_CODE), Style::default().fg(t.text_muted).bg(bg)),
                ratatui::text::Span::styled(file.clone(), style),
            ]));
        }
        if end < total {
            picker_lines.push(Line::styled(
                format!("  {} more", total - end),
                Style::default().fg(t.text_muted).bg(t.bg_tertiary).add_modifier(Modifier::ITALIC),
            ));
        }
        // Render
        let picker_h = (picker_lines.len() as u16 + 2).min(12);
        let picker_w = area.width.saturating_sub(2).min(50);
        let picker_x = area.x;
        let picker_y = area.y.saturating_sub(picker_h);
        let picker_rect = Rect {
            x: picker_x,
            y: picker_y.max(1),
            width: picker_w,
            height: picker_h,
        };
        clear_area(f, picker_rect, &t);
        let picker_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.accent_secondary).bg(t.bg_tertiary))
            .title(format!(" {} FILES ", super::icons::FILES_TITLE))
            .title_style(
                Style::default()
                    .fg(t.accent_secondary)
                    .bg(t.bg_tertiary)
                    .add_modifier(Modifier::BOLD),
            )
            .style(Style::default().bg(t.bg_tertiary));
        let picker_inner = picker_block.inner(picker_rect);
        f.render_widget(picker_block, picker_rect);
        let slice: Vec<Line<'static>> = picker_lines.into_iter().take(picker_inner.height as usize).collect();
        f.render_widget(Paragraph::new(slice).style(Style::default().bg(t.bg_tertiary)), picker_inner);
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
    // Clear the area behind the modal for fully opaque rendering.
    clear_area(f, area, &t);

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
            let mut model_style = Style::default().fg(fg).bg(bg);
            if is_sel {
                model_style = model_style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            }
            let mut spans: Vec<Span<'static>> = vec![
                ratatui::text::Span::styled(
                    format!("{marker} "),
                    Style::default().fg(t.accent_primary).bg(bg),
                ),
                ratatui::text::Span::styled(name.clone(), model_style),
            ];
            // Show [FREE] tag if this model is in the known registry as free.
            // We check all providers' known models (small constant data).
            let is_free = crate::provider::preset_names().any(|pid| {
                crate::provider::known_models(pid).iter().any(|km| km.id == name.as_str() && km.free)
            });
            if is_free {
                spans.push(ratatui::text::Span::styled(
                    " [FREE]",
                    Style::default()
                        .fg(t.accent_success)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            lines.push(Line::from(spans));
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

/// Renders the usage/cost dashboard modal (Ctrl+I).
fn render_cost_dashboard(f: &mut Frame<'_>, area: Rect, info: &StatusInfo) {
    let t = theme::active();
    let w: u16 = 56;
    let h: u16 = 18;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect::new(x, y, w.min(area.width.saturating_sub(2)), h.min(area.height.saturating_sub(2)));
    clear_area(f, area, &t);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.border_focus).bg(t.bg_tertiary))
        .title(format!(" {} USAGE & COST ", super::icons::INFO))
        .title_style(Style::default().fg(t.accent_primary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(t.bg_tertiary));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let muted = Style::default().fg(t.text_muted).bg(t.bg_tertiary);
    let label = Style::default().fg(t.text_secondary).bg(t.bg_tertiary);
    let value = Style::default().fg(t.text_primary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD);
    let accent = Style::default().fg(t.accent_primary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD);

    // Token usage
    lines.push(Line::from(vec![
        Span::styled(" Token Usage", accent),
    ]));
    lines.push(Line::default());

    let used_s = format!("{:.1}k", info.tokens as f64 / 1000.0);
    let total_s = format!("{:.1}k", info.budget as f64 / 1000.0);
    let pct = info.tokens.checked_mul(100).and_then(|t| t.checked_div(info.budget)).unwrap_or(0);

    lines.push(Line::from(vec![
        Span::styled("  Used:     ", label),
        Span::styled(used_s, value),
        Span::styled(" / ", muted),
        Span::styled(total_s, value),
        Span::styled(format!(" ({pct}%)"), if pct > 80 { Style::default().fg(t.accent_error).bg(t.bg_tertiary).add_modifier(Modifier::BOLD) } else { value }),
    ]));

    // Progress bar
    let bar_len = 30;
    let filled = (pct.min(100) * bar_len) / 100;
    let bar = "█".repeat(filled) + &"░".repeat(bar_len - filled);
    let bar_fg = if pct > 80 { t.accent_error } else if pct > 60 { t.accent_warning } else { t.accent_success };
    lines.push(Line::from(vec![
        Span::styled("             ", label),
        Span::styled(bar, Style::default().fg(bar_fg).bg(t.bg_tertiary)),
    ]));

    lines.push(Line::default());

    // Session stats
    lines.push(Line::from(vec![
        Span::styled(" Session Stats", accent),
    ]));
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled("  Turns:     ", label),
        Span::styled(format!("{}", info.turns), value),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Errors:    ", label),
        Span::styled(format!("{}", info.errors), if info.errors > 0 { Style::default().fg(t.accent_error).bg(t.bg_tertiary) } else { value }),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Avg Latency: ", label),
        Span::styled(format!("{}ms", info.avg_latency_ms), value),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Provider:  ", label),
        Span::styled(info.provider.clone(), value),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Model:     ", label),
        Span::styled(info.model.clone(), value),
    ]));

    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled("  Esc ", Style::default().fg(t.text_secondary).bg(t.bg_secondary).add_modifier(Modifier::BOLD)),
        Span::styled(" close", muted),
    ]));

    f.render_widget(Paragraph::new(lines).style(Style::default().bg(t.bg_tertiary)), inner);
}

fn render_settings(f: &mut Frame<'_>, area: Rect) {
    let t = theme::active();
    let w: u16 = 56;
    let h: u16 = 16;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect::new(x, y, w.min(area.width.saturating_sub(2)), h.min(area.height.saturating_sub(2)));
    clear_area(f, area, &t);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.border_focus).bg(t.bg_tertiary))
        .title(format!(" {} SETTINGS ", super::icons::TOOLS))
        .title_style(Style::default().fg(t.accent_primary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(t.bg_tertiary));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let muted = Style::default().fg(t.text_muted).bg(t.bg_tertiary);
    let label = Style::default().fg(t.text_secondary).bg(t.bg_tertiary);
    let value = Style::default().fg(t.text_primary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD);
    let lines = vec![
        Line::from(vec![Span::styled(" Theme", Style::default().fg(t.accent_primary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD))]),
        Line::default(),
        Line::from(vec![Span::styled("  /theme <name>  ", label), Span::styled("switch palette", muted)]),
        Line::from(vec![Span::styled("  mono / default / dracula / nord …", value)]),
        Line::default(),
        Line::from(vec![Span::styled(" Provider", Style::default().fg(t.accent_primary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD))]),
        Line::default(),
        Line::from(vec![Span::styled("  /provider <name>  ", label), Span::styled("mistral · kimi · groq · ollama", muted)]),
        Line::from(vec![Span::styled("  /model <name>  ", label), Span::styled("partial match", muted)]),
        Line::default(),
        Line::from(vec![Span::styled("  Esc ", Style::default().fg(t.text_secondary).bg(t.bg_secondary).add_modifier(Modifier::BOLD)), Span::styled(" close", muted)]),
    ];
    f.render_widget(Paragraph::new(lines).style(Style::default().bg(t.bg_tertiary)), inner);
}

fn render_shortcuts(f: &mut Frame<'_>, area: Rect) {
    let t = theme::active();
    let w: u16 = 58;
    let h: u16 = 18;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect::new(x, y, w.min(area.width.saturating_sub(2)), h.min(area.height.saturating_sub(2)));
    clear_area(f, area, &t);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.border_focus).bg(t.bg_tertiary))
        .title(" ? SHORTCUTS ")
        .title_style(Style::default().fg(t.accent_primary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(t.bg_tertiary));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let muted = Style::default().fg(t.text_muted).bg(t.bg_tertiary);
    let key = Style::default().fg(t.text_secondary).bg(t.bg_secondary).add_modifier(Modifier::BOLD);
    let val = Style::default().fg(t.text_primary).bg(t.bg_tertiary);
    let row = |k: &str, v: &str| Line::from(vec![Span::styled(format!("  {k} "), key), Span::styled(v.to_owned(), muted)]);
    let lines = vec![
        row("/", "commands palette"),
        row("@", "file mention"),
        row("Tab", "cycle palette"),
        row("↑↓", "history / picker"),
        row("Enter", "send / select"),
        row("Esc", "clear / close"),
        row("Ctrl+L", "clear chat"),
        row("Ctrl+T", "toggle project"),
        row("Ctrl+P", "toggle todo"),
        row("Ctrl+I", "usage"),
        row("Ctrl+,", "settings"),
        row("?", "this help"),
        Line::default(),
        Line::from(vec![Span::styled("  Esc ", Style::default().fg(t.text_secondary).bg(t.bg_secondary).add_modifier(Modifier::BOLD)), Span::styled(" close", muted), Span::styled("  ·  click file to pin", val)]),
    ];
    f.render_widget(Paragraph::new(lines).style(Style::default().bg(t.bg_tertiary)), inner);
}

/// Renders the multi-step provider setup workflow modal.
fn render_provider_workflow(f: &mut Frame<'_>, area: Rect, wf: &super::app::ProviderWorkflow) {
    use super::app::ProviderWorkflow;
    let t = theme::active();

    match wf {
        ProviderWorkflow::SelectProvider { providers, selected } => {
            let w: u16 = 50;
            let h = (providers.len() as u16 + 6).min(area.height.saturating_sub(4));
            let x = area.x + area.width.saturating_sub(w) / 2;
            let y = area.y + area.height.saturating_sub(h) / 2;
            let rect = Rect::new(x, y, w.min(area.width.saturating_sub(2)), h);
            clear_area(f, area, &t);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(t.border_focus).bg(t.bg_tertiary))
                .title(" \u{f013} SETUP — SELECT PROVIDER ")
                .title_style(Style::default().fg(t.text_inverse).bg(t.accent_primary).add_modifier(Modifier::BOLD))
                .style(Style::default().bg(t.bg_tertiary));
            let inner = block.inner(rect);
            f.render_widget(block, rect);
            let mut lines: Vec<Line<'static>> = Vec::new();
            lines.push(Line::styled(
                " Choose your AI provider:",
                Style::default().fg(t.text_muted).bg(t.bg_tertiary).add_modifier(Modifier::ITALIC),
            ));
            lines.push(Line::default());
            for (i, prov) in providers.iter().enumerate() {
                let is_sel = i == *selected;
                let marker = if is_sel { "\u{25b8}" } else { " " };
                let bg = if is_sel { t.bg_hover } else { t.bg_tertiary };
                let fg = if is_sel { t.accent_primary } else { t.text_secondary };
                let mut style = Style::default().fg(fg).bg(bg);
                if is_sel { style = style.add_modifier(Modifier::BOLD); }
                // Check if provider has a free tier
                let known = crate::provider::known_models(prov);
                let has_free = known.iter().any(|m| m.free);
                let mut spans = vec![
                    ratatui::text::Span::styled(format!("{marker} "), Style::default().fg(t.accent_primary).bg(bg)),
                    ratatui::text::Span::styled(prov.clone(), style),
                ];
                if has_free {
                    spans.push(ratatui::text::Span::styled(
                        "  [FREE models]",
                        Style::default().fg(t.accent_success).bg(bg).add_modifier(Modifier::BOLD),
                    ));
                }
                lines.push(Line::from(spans));
            }
            lines.push(Line::default());
            lines.push(Line::from(vec![
                ratatui::text::Span::styled(" Enter ", Style::default().fg(t.text_inverse).bg(t.accent_success).add_modifier(Modifier::BOLD)),
                ratatui::text::Span::styled(" select  ", Style::default().fg(t.text_secondary).bg(t.bg_tertiary)),
                ratatui::text::Span::styled(" Esc ", Style::default().fg(t.text_inverse).bg(t.border_default).add_modifier(Modifier::BOLD)),
                ratatui::text::Span::styled(" cancel", Style::default().fg(t.text_muted).bg(t.bg_tertiary)),
            ]));
            f.render_widget(Paragraph::new(lines).style(Style::default().bg(t.bg_tertiary)), inner);
        }
        ProviderWorkflow::EnterApiKey { provider, key_input, cursor } => {
            let w: u16 = 56;
            let h: u16 = 14;
            let x = area.x + area.width.saturating_sub(w) / 2;
            let y = area.y + area.height.saturating_sub(h) / 2;
            let rect = Rect::new(x, y, w.min(area.width.saturating_sub(2)), h);
            clear_area(f, area, &t);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(t.border_focus).bg(t.bg_tertiary))
                .title(format!(" \u{f023} SETUP — {provider} API KEY "))
                .title_style(Style::default().fg(t.text_inverse).bg(t.accent_primary).add_modifier(Modifier::BOLD))
                .style(Style::default().bg(t.bg_tertiary));
            let inner = block.inner(rect);
            f.render_widget(block, rect);
            let mut lines: Vec<Line<'static>> = Vec::new();
            lines.push(Line::styled(
                format!(" Enter your {provider} API key:"),
                Style::default().fg(t.text_muted).bg(t.bg_tertiary).add_modifier(Modifier::ITALIC),
            ));
            lines.push(Line::default());
            // Masked input display
            let masked: String = key_input.chars().map(|_| '\u{2022}').collect();
            let before: String = key_input.chars().take(*cursor).map(|_| '\u{2022}').collect();
            let after: String = key_input.chars().skip(*cursor).map(|_| '\u{2022}').collect();
            lines.push(Line::from(vec![
                ratatui::text::Span::styled(" > ", Style::default().fg(t.accent_primary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD)),
                ratatui::text::Span::styled(before, Style::default().fg(t.text_primary).bg(t.bg_tertiary)),
                ratatui::text::Span::styled(
                    if after.is_empty() {
                        "\u{258c}".to_owned()
                    } else {
                        #[allow(clippy::unwrap_used)] // safe: after is not empty
                        after.chars().next().unwrap().to_string()
                    },
                    Style::default().fg(t.accent_primary).bg(t.bg_tertiary),
                ),
                ratatui::text::Span::styled(
                    after.chars().skip(1).collect::<String>(),
                    Style::default().fg(t.text_primary).bg(t.bg_tertiary),
                ),
            ]));
            lines.push(Line::default());
            if masked.is_empty() {
                lines.push(Line::styled(
                    "  Paste or type your API key, then press Enter.",
                    Style::default().fg(t.text_muted).bg(t.bg_tertiary),
                ));
            } else {
                lines.push(Line::styled(
                    format!("  {} characters entered", key_input.chars().count()),
                    Style::default().fg(t.text_muted).bg(t.bg_tertiary),
                ));
            }
            lines.push(Line::styled(
                "  Key is used only for this session. For ollama, press Esc.",
                Style::default().fg(t.text_muted).bg(t.bg_tertiary).add_modifier(Modifier::DIM),
            ));
            lines.push(Line::default());
            lines.push(Line::from(vec![
                ratatui::text::Span::styled(" Enter ", Style::default().fg(t.text_inverse).bg(t.accent_success).add_modifier(Modifier::BOLD)),
                ratatui::text::Span::styled(" continue  ", Style::default().fg(t.text_secondary).bg(t.bg_tertiary)),
                ratatui::text::Span::styled(" Esc ", Style::default().fg(t.text_inverse).bg(t.border_default).add_modifier(Modifier::BOLD)),
                ratatui::text::Span::styled(" cancel", Style::default().fg(t.text_muted).bg(t.bg_tertiary)),
            ]));
            f.render_widget(Paragraph::new(lines).style(Style::default().bg(t.bg_tertiary)), inner);
            // cursor
            let prompt_w: u16 = 3; // "> " width
            let before_w = before_width(key_input, *cursor);
            let cx = inner.x + prompt_w + before_w;
            let cy = inner.y + 3;
            f.set_cursor_position((cx.min(inner.x + inner.width.saturating_sub(1)), cy));
        }
        ProviderWorkflow::SelectModel { provider, api_key: _, models, selected } => {
            let w: u16 = 56;
            let visible = models.len().min(10) as u16;
            let h = visible + 8;
            let x = area.x + area.width.saturating_sub(w) / 2;
            let y = area.y + area.height.saturating_sub(h) / 2;
            let rect = Rect::new(x, y, w.min(area.width.saturating_sub(2)), h);
            clear_area(f, area, &t);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(t.border_focus).bg(t.bg_tertiary))
                .title(format!(" \u{f2db} SETUP — SELECT MODEL for {provider} "))
                .title_style(Style::default().fg(t.text_inverse).bg(t.accent_primary).add_modifier(Modifier::BOLD))
                .style(Style::default().bg(t.bg_tertiary));
            let inner = block.inner(rect);
            f.render_widget(block, rect);
            let mut lines: Vec<Line<'static>> = Vec::new();
            lines.push(Line::styled(
                " Choose a model:",
                Style::default().fg(t.text_muted).bg(t.bg_tertiary).add_modifier(Modifier::ITALIC),
            ));
            lines.push(Line::default());
            // Windowed scroll
            let total = models.len();
            let sel = *selected;
            let start = sel.saturating_sub(visible as usize / 2).min(total.saturating_sub(visible as usize));
            let end = (start + visible as usize).min(total);
            for (i, name) in models.iter().enumerate().take(end).skip(start) {
                let is_sel = i == sel;
                let marker = if is_sel { "\u{25b8}" } else { " " };
                let bg = if is_sel { t.bg_hover } else { t.bg_tertiary };
                let fg = if is_sel { t.accent_primary } else { t.text_secondary };
                let mut style = Style::default().fg(fg).bg(bg);
                if is_sel { style = style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED); }
                let mut spans = vec![
                    ratatui::text::Span::styled(format!("{marker} "), Style::default().fg(t.accent_primary).bg(bg)),
                    ratatui::text::Span::styled(name.clone(), style),
                ];
                // Show [FREE] tag
                let is_free = crate::provider::known_models(provider).iter().any(|km| km.id == name.as_str() && km.free);
                if is_free {
                    spans.push(ratatui::text::Span::styled(
                        " [FREE]",
                        Style::default().fg(t.accent_success).bg(bg).add_modifier(Modifier::BOLD),
                    ));
                }
                lines.push(Line::from(spans));
            }
            lines.push(Line::default());
            lines.push(Line::from(vec![
                ratatui::text::Span::styled(" Enter ", Style::default().fg(t.text_inverse).bg(t.accent_success).add_modifier(Modifier::BOLD)),
                ratatui::text::Span::styled(" select  ", Style::default().fg(t.text_secondary).bg(t.bg_tertiary)),
                ratatui::text::Span::styled(" Esc ", Style::default().fg(t.text_inverse).bg(t.border_default).add_modifier(Modifier::BOLD)),
                ratatui::text::Span::styled(" cancel", Style::default().fg(t.text_muted).bg(t.bg_tertiary)),
            ]));
            f.render_widget(Paragraph::new(lines).style(Style::default().bg(t.bg_tertiary)), inner);
        }
        ProviderWorkflow::Testing { provider, model, .. } => {
            let w: u16 = 50;
            let h: u16 = 8;
            let x = area.x + area.width.saturating_sub(w) / 2;
            let y = area.y + area.height.saturating_sub(h) / 2;
            let rect = Rect::new(x, y, w.min(area.width.saturating_sub(2)), h);
            clear_area(f, area, &t);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(t.accent_warning).bg(t.bg_tertiary))
                .title(" \u{f017} TESTING CONNECTION ")
                .title_style(Style::default().fg(t.text_inverse).bg(t.accent_warning).add_modifier(Modifier::BOLD))
                .style(Style::default().bg(t.bg_tertiary));
            let inner = block.inner(rect);
            f.render_widget(block, rect);
            let frame = tick_frame(4);
            let spinner = ['\u{25f7}', '\u{25f4}', '\u{25fa}', '\u{25fb}'][frame];
            f.render_widget(Paragraph::new(vec![
                Line::default(),
                Line::from(vec![
                    ratatui::text::Span::styled(format!(" {spinner} "), Style::default().fg(t.accent_warning).bg(t.bg_tertiary).add_modifier(Modifier::BOLD)),
                    ratatui::text::Span::styled("Testing connection...", Style::default().fg(t.text_primary).bg(t.bg_tertiary)),
                ]),
                Line::default(),
                Line::from(vec![
                    ratatui::text::Span::styled("  provider: ", Style::default().fg(t.text_muted).bg(t.bg_tertiary)),
                    ratatui::text::Span::styled(provider.clone(), Style::default().fg(t.text_primary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD)),
                ]),
                Line::from(vec![
                    ratatui::text::Span::styled("  model:    ", Style::default().fg(t.text_muted).bg(t.bg_tertiary)),
                    ratatui::text::Span::styled(model.clone(), Style::default().fg(t.text_primary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD)),
                ]),
            ]).style(Style::default().bg(t.bg_tertiary)), inner);
        }
        ProviderWorkflow::Result { provider, model, ok, message, .. } => {
            let w: u16 = 56;
            let h: u16 = 12;
            let x = area.x + area.width.saturating_sub(w) / 2;
            let y = area.y + area.height.saturating_sub(h) / 2;
            let rect = Rect::new(x, y, w.min(area.width.saturating_sub(2)), h);
            clear_area(f, area, &t);
            let border_color = if *ok { t.accent_success } else { t.accent_error };
            let title = if *ok { " \u{f058} SETUP COMPLETE " } else { " \u{f057} SETUP FAILED " };
            let title_bg = if *ok { t.accent_success } else { t.accent_error };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color).bg(t.bg_tertiary))
                .title(title)
                .title_style(Style::default().fg(t.text_inverse).bg(title_bg).add_modifier(Modifier::BOLD))
                .style(Style::default().bg(t.bg_tertiary));
            let inner = block.inner(rect);
            f.render_widget(block, rect);
            let status_color = if *ok { t.accent_success } else { t.accent_error };
            let status_text = if *ok { "PASSED" } else { "FAILED" };
            f.render_widget(Paragraph::new(vec![
                Line::default(),
                Line::from(vec![
                    ratatui::text::Span::styled("  Status:  ", Style::default().fg(t.text_muted).bg(t.bg_tertiary)),
                    ratatui::text::Span::styled(status_text, Style::default().fg(status_color).bg(t.bg_tertiary).add_modifier(Modifier::BOLD)),
                ]),
                Line::from(vec![
                    ratatui::text::Span::styled("  Provider: ", Style::default().fg(t.text_muted).bg(t.bg_tertiary)),
                    ratatui::text::Span::styled(provider.clone(), Style::default().fg(t.text_primary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD)),
                ]),
                Line::from(vec![
                    ratatui::text::Span::styled("  Model:   ", Style::default().fg(t.text_muted).bg(t.bg_tertiary)),
                    ratatui::text::Span::styled(model.clone(), Style::default().fg(t.text_primary).bg(t.bg_tertiary).add_modifier(Modifier::BOLD)),
                ]),
                Line::default(),
                Line::styled(
                    format!("  {}", message),
                    Style::default().fg(if *ok { t.text_secondary } else { t.accent_error }).bg(t.bg_tertiary),
                ),
                Line::default(),
                Line::from(vec![
                    ratatui::text::Span::styled("  Press any key to close", Style::default().fg(t.text_muted).bg(t.bg_tertiary).add_modifier(Modifier::DIM)),
                ]),
            ]).style(Style::default().bg(t.bg_tertiary)), inner);
        }
    }
}

/// Renders the focused-pane glow beam: a 1px cobalt/violet top-edge
/// whose bright center follows the mouse X position (chamfer effect).
fn render_focus_beam(f: &mut Frame<'_>, area: Rect, mouse_x: u16) {
    let t = theme::active();
    if area.width < 4 {
        return;
    }
    // Beam width is half the pane width, centered around mouse X.
    let beam_width = area.width / 2;
    let mouse_rel = mouse_x.saturating_sub(area.x);
    let offset = mouse_rel
        .saturating_sub(beam_width / 2)
        .min(area.width.saturating_sub(beam_width));
    let beam_area = Rect::new(area.x + offset, area.y, beam_width.min(area.width), 1);
    // Solid accent bar — in the terminal, this reads as a subtle top-edge glow.
    let beam = ratatui::widgets::Paragraph::new("")
        .style(Style::default().fg(t.accent_primary).bg(t.accent_primary));
    f.render_widget(beam, beam_area);
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
