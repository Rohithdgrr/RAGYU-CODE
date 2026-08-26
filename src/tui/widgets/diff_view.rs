//! Interactive Diff Viewer Widget
//!
//! Renders unified diffs with hunk-level approval/discard controls.
//! Supports split view and unified view modes.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::super::theme;

/// A parsed hunk from a unified diff.
#[derive(Debug, Clone)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<HunkLine>,
    pub approved: bool,
    pub file: String,
}

/// A single line within a hunk.
#[derive(Debug, Clone)]
pub struct HunkLine {
    pub kind: HunkLineKind,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkLineKind {
    Context,
    Added,
    Removed,
    Header,
}

/// State for the interactive diff viewer.
#[derive(Debug, Clone)]
pub struct DiffState {
    pub hunks: Vec<Hunk>,
    pub selected_hunk: usize,
    pub unified: bool,
    pub scroll_offset: usize,
}

impl Default for DiffState {
    fn default() -> Self {
        Self::new()
    }
}

impl DiffState {
    pub fn new() -> Self {
        Self {
            hunks: Vec::new(),
            selected_hunk: 0,
            unified: true,
            scroll_offset: 0,
        }
    }

    /// Parses a unified diff string into hunks.
    pub fn parse_diff(diff: &str) -> Self {
        let mut hunks = Vec::new();
        let mut current_file = String::new();
        let mut current_header = String::new();
        let mut current_lines = Vec::new();

        for line in diff.lines() {
            if line.starts_with("--- ") || line.starts_with("+++ ") {
                // File header
                if !current_header.is_empty() && !current_lines.is_empty() {
                    hunks.push(Hunk {
                        header: current_header.clone(),
                        lines: current_lines.clone(),
                        approved: false,
                        file: current_file.clone(),
                    });
                    current_lines.clear();
                }
                if line.starts_with("+++ ") {
                    current_file = line[4..].to_owned();
                }
            } else if line.starts_with("@@ ") {
                // Hunk header
                if !current_header.is_empty() && !current_lines.is_empty() {
                    hunks.push(Hunk {
                        header: current_header.clone(),
                        lines: current_lines.clone(),
                        approved: false,
                        file: current_file.clone(),
                    });
                    current_lines.clear();
                }
                current_header = line.to_owned();
            } else if line.starts_with('+') {
                current_lines.push(HunkLine {
                    kind: HunkLineKind::Added,
                    content: line[1..].to_owned(),
                });
            } else if line.starts_with('-') {
                current_lines.push(HunkLine {
                    kind: HunkLineKind::Removed,
                    content: line[1..].to_owned(),
                });
            } else if line.starts_with(' ') {
                current_lines.push(HunkLine {
                    kind: HunkLineKind::Context,
                    content: line[1..].to_owned(),
                });
            }
        }

        // Push last hunk
        if !current_header.is_empty() || !current_lines.is_empty() {
            hunks.push(Hunk {
                header: current_header,
                lines: current_lines,
                approved: false,
                file: current_file,
            });
        }

        Self {
            hunks,
            selected_hunk: 0,
            unified: true,
            scroll_offset: 0,
        }
    }

    /// Toggle approval of the selected hunk.
    pub fn toggle_selected(&mut self) {
        if let Some(hunk) = self.hunks.get_mut(self.selected_hunk) {
            hunk.approved = !hunk.approved;
        }
    }

    /// Approve all hunks.
    pub fn approve_all(&mut self) {
        for hunk in &mut self.hunks {
            hunk.approved = true;
        }
    }

    /// Discard all hunks.
    pub fn discard_all(&mut self) {
        for hunk in &mut self.hunks {
            hunk.approved = false;
        }
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected_hunk > 0 {
            self.selected_hunk -= 1;
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        if self.selected_hunk + 1 < self.hunks.len() {
            self.selected_hunk += 1;
        }
    }

    /// Toggle between unified and split view.
    pub fn toggle_view(&mut self) {
        self.unified = !self.unified;
    }

    /// Returns the number of approved hunks.
    pub fn approved_count(&self) -> usize {
        self.hunks.iter().filter(|h| h.approved).count()
    }

    /// Returns the total number of hunks.
    pub fn total_count(&self) -> usize {
        self.hunks.len()
    }
}

/// Renders the diff state as styled lines for the TUI.
pub fn render_diff_lines(state: &DiffState, width: u16) -> Vec<Line<'static>> {
    let t = theme::active();
    let inner = width.saturating_sub(4) as usize;
    let mut lines = Vec::new();

    if state.hunks.is_empty() {
        lines.push(Line::styled(
            "  No staged edits to review",
            Style::default().fg(t.text_muted),
        ));
        return lines;
    }

    // Header
    let approved = state.approved_count();
    let total = state.total_count();
    lines.push(Line::from(vec![
        Span::styled(
            format!(" {} DIFF ", super::super::icons::INFO),
            Style::default()
                .fg(t.text_inverse)
                .bg(t.accent_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {approved}/{total} hunks approved"),
            Style::default().fg(t.text_secondary),
        ),
        Span::styled(
            if state.unified { " [unified]" } else { " [split]" },
            Style::default().fg(t.text_muted),
        ),
    ]));
    lines.push(Line::default());

    // Render each hunk
    for (idx, hunk) in state.hunks.iter().enumerate() {
        let is_selected = idx == state.selected_hunk;
        let status_icon = if hunk.approved {
            super::super::icons::CHECK
        } else {
            "·"
        };
        let status_style = if hunk.approved {
            t.success()
        } else if is_selected {
            t.warning()
        } else {
            t.text_dim()
        };

        // Hunk header line
        let header_bg = if is_selected { t.bg_hover } else { t.bg_secondary };
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {status_icon} "),
                status_style,
            ),
            Span::styled(
                format!("[{}] ", idx + 1),
                Style::default().fg(t.text_muted).bg(header_bg),
            ),
            Span::styled(
                truncate(&hunk.header, inner.saturating_sub(10)),
                Style::default()
                    .fg(if is_selected { t.accent_primary } else { t.text_secondary })
                    .bg(header_bg)
                    .add_modifier(if is_selected { Modifier::BOLD } else { Modifier::empty() }),
            ),
        ]));

        // Hunk lines (only if selected or unified view shows all)
        if is_selected || state.unified {
            for hunk_line in &hunk.lines {
                let (prefix, style) = match hunk_line.kind {
                    HunkLineKind::Added => (
                        "+",
                        Style::default().fg(t.accent_success).bg(t.bg_primary),
                    ),
                    HunkLineKind::Removed => (
                        "-",
                        Style::default().fg(t.accent_error).bg(t.bg_primary),
                    ),
                    HunkLineKind::Context => (
                        " ",
                        Style::default().fg(t.text_muted).bg(t.bg_primary),
                    ),
                    HunkLineKind::Header => (
                        "@",
                        Style::default().fg(t.accent_primary).bg(t.bg_primary),
                    ),
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("   {prefix}"),
                        style,
                    ),
                    Span::styled(
                        truncate(&hunk_line.content, inner.saturating_sub(5)),
                        style,
                    ),
                ]));
            }
            if !state.unified || is_selected {
                lines.push(Line::default());
            }
        }
    }

    // Footer
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled(
            " j/k ",
            Style::default()
                .fg(t.text_secondary)
                .bg(t.bg_secondary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" navigate ", Style::default().fg(t.text_muted)),
        Span::styled(
            " Space ",
            Style::default()
                .fg(t.text_secondary)
                .bg(t.bg_secondary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" toggle ", Style::default().fg(t.text_muted)),
        Span::styled(
            " a ",
            Style::default()
                .fg(t.text_secondary)
                .bg(t.bg_secondary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" approve all ", Style::default().fg(t.text_muted)),
        Span::styled(
            " Enter ",
            Style::default()
                .fg(t.text_inverse)
                .bg(t.accent_success)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" apply ", Style::default().fg(t.text_muted)),
    ]));

    lines
}

fn truncate(s: &str, max: usize) -> String {
    if UnicodeWidthStr::width(s) <= max {
        s.to_owned()
    } else {
        let mut out = String::new();
        let mut w = 0;
        for ch in s.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
            if w + cw > max.saturating_sub(1) {
                out.push('…');
                break;
            }
            out.push(ch);
            w += cw;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_unified_diff() {
        let diff = "--- a/main.rs\n+++ b/main.rs\n@@ -1,3 +1,4 @@\n use std::io;\n+use std::fs;\n \n fn main() {}\n";
        let state = DiffState::parse_diff(diff);
        assert_eq!(state.hunks.len(), 1);
        assert_eq!(state.hunks[0].lines.len(), 4);
        assert!(state.hunks[0].lines.iter().any(|l| l.kind == HunkLineKind::Added));
    }

    #[test]
    fn toggle_approval() {
        let mut state = DiffState::parse_diff("@@ -1 +1 @@\n-old\n+new\n");
        assert!(!state.hunks[0].approved);
        state.toggle_selected();
        assert!(state.hunks[0].approved);
        assert_eq!(state.approved_count(), 1);
    }
}
