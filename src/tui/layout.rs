//! Responsive pane layout.
//!
//! Terminal real estate is split top-to-bottom into a 1-row status bar,
//! the main content band (chat center, optional tree/tools flanks), and a
//! 3-row input bar. Narrow terminals drop side panes instead of squeezing
//! the chat; short terminals compact the input bar to one row.

use ratatui::layout::{Constraint, Layout, Rect};

/// Computed pane rects for one frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct PaneLayout {
    pub status: Rect,
    pub chat: Rect,
    pub input: Rect,
    /// Left sidebar; `None` when hidden or too narrow to show.
    pub tree: Option<Rect>,
    /// Right sidebar; `None` when hidden or too narrow to show.
    pub tools: Option<Rect>,
}

impl PaneLayout {
    /// Splits `area` per the current visibility flags.
    ///
    /// Responsive breakpoints:
    /// - width < 100 → tool panel collapses
    /// - width < 60  → project tree collapses too (chat goes full-width)
    /// - height < 20 → input bar shrinks from 3 rows to 1
    pub fn compute(area: Rect, show_tree: bool, show_tools: bool) -> Self {
        let input_rows = if area.height < 20 { 1 } else { 3 };

        let [status, body, input] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(input_rows),
        ])
        .areas(area);

        let want_tree = show_tree && area.width >= 60;
        let want_tools = show_tools && area.width >= 100;

        let mut chat = body;
        let mut tree = None;
        let mut tools = None;

        // Carve side panes off the body left-to-right so the chat keeps a
        // predictable center position regardless of what is visible.
        if want_tree || want_tools {
            let cols = Layout::horizontal([
                Constraint::Length(if want_tree { 24 } else { 0 }),
                Constraint::Min(20), // chat floor — never squeezed below this
                Constraint::Length(if want_tools { 28 } else { 0 }),
            ])
            .split(body);
            chat = cols[1];
            if want_tree {
                tree = Some(cols[0]);
            }
            if want_tools {
                tools = Some(cols[2]);
            }
        }

        Self {
            status,
            chat,
            input,
            tree,
            tools,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    #[test]
    fn full_layout_shows_both_sidebars() {
        let l = PaneLayout::compute(rect(140, 40), true, true);
        assert!(l.tree.is_some());
        assert!(l.tools.is_some());
        assert_eq!(l.status.height, 1);
        assert_eq!(l.input.height, 3);
        // Chat never drops below its floor.
        assert!(l.chat.width >= 20);
    }

    #[test]
    fn narrow_terminal_hides_tools() {
        let l = PaneLayout::compute(rect(80, 30), true, true);
        assert!(l.tree.is_some());
        assert!(l.tools.is_none());
    }

    #[test]
    fn very_narrow_terminal_hides_both() {
        let l = PaneLayout::compute(rect(50, 30), true, true);
        assert!(l.tree.is_none());
        assert!(l.tools.is_none());
        assert_eq!(l.chat.width, 50);
    }

    #[test]
    fn short_terminal_compacts_input() {
        let l = PaneLayout::compute(rect(120, 15), false, false);
        assert_eq!(l.input.height, 1);
    }

    #[test]
    fn visibility_flags_respected_on_wide_terminal() {
        let l = PaneLayout::compute(rect(140, 40), false, false);
        assert!(l.tree.is_none());
        assert!(l.tools.is_none());
    }
}
