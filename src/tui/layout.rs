//! Responsive pane layout.
//!
//! Terminal real estate is split top-to-bottom into a 1-row status bar,
//! the main content band (chat center, optional tree + explorer flanks), and a
//! 5-row rich floating composer (3-row cozy on medium screens).
//! Narrow terminals drop side panes instead of squeezing the chat;
//! short terminals compact the input bar to one row.
//! Right sidebar now hosts the file explorer (replaced former tools panel).

use ratatui::layout::{Constraint, Layout, Rect};

/// Computed pane rects for one frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct PaneLayout {
    pub status: Rect,
    pub chat: Rect,
    pub input: Rect,
    /// Left sidebar (project tree); `None` when hidden or too narrow to show.
    pub tree: Option<Rect>,
    /// Right sidebar (file explorer — formerly tools); `None` when hidden or too narrow.
    pub tools: Option<Rect>,
}

impl PaneLayout {
    /// Splits `area` per the current visibility flags.
    ///
    /// Responsive breakpoints:
    /// - width < 70 → file explorer (right) collapses
    /// - width < 60  → project tree (left) collapses too (chat goes full-width)
    /// - height < 20 → 1-row compact (tiny)
    /// - height < 28 → 3-row cozy
    /// - otherwise  → 5-row rich floating composer
    pub fn compute(area: Rect, show_tree: bool, show_tools: bool) -> Self {
        let input_rows = if area.height < 20 {
            1
        } else if area.height < 28 {
            3
        } else {
            5
        };

        let [status, body, input] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(input_rows),
        ])
        .areas(area);

        let want_tree = show_tree && area.width >= 60;
        let want_tools = show_tools && area.width >= 70;

        let mut chat = body;
        let mut tree = None;
        let mut tools = None;

        // Carve side panes off the body left-to-right so the chat keeps a
        // predictable center position regardless of what is visible.
        // When only one explorer is visible we give it extra width (34) for
        // perfect file-name visibility; when both are visible we keep 26+26
        // to preserve chat width. Chat never drops below 20 cols.
        if want_tree || want_tools {
            let (left_w, right_w) = match (want_tree, want_tools) {
                (true, true) => (26, 28),
                (true, false) => (34, 0),
                (false, true) => (0, 36),
                (false, false) => (0, 0),
            };
            let cols = Layout::horizontal([
                Constraint::Length(left_w),
                Constraint::Min(20),
                Constraint::Length(right_w),
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
        assert_eq!(l.input.height, 5);
        // Chat never drops below its floor.
        assert!(l.chat.width >= 20);
    }

    #[test]
    fn narrow_terminal_hides_tools() {
        let l = PaneLayout::compute(rect(65, 30), true, true);
        assert!(l.tree.is_some());
        assert!(l.tools.is_none());
    }

    #[test]
    fn mid_width_shows_explorer() {
        let l = PaneLayout::compute(rect(90, 30), true, true);
        assert!(l.tree.is_some());
        assert!(l.tools.is_some());
    }

    #[test]
    fn single_explorer_gets_wider_pane() {
        let l = PaneLayout::compute(rect(100, 30), false, true);
        assert!(l.tools.is_some());
        assert_eq!(l.tools.unwrap().width, 36);
        let l2 = PaneLayout::compute(rect(100, 30), true, false);
        assert_eq!(l2.tree.unwrap().width, 34);
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
    fn medium_terminal_uses_cozy_input() {
        let l = PaneLayout::compute(rect(120, 25), false, false);
        assert_eq!(l.input.height, 3);
    }

    #[test]
    fn tall_terminal_uses_rich_input() {
        let l = PaneLayout::compute(rect(120, 40), false, false);
        assert_eq!(l.input.height, 5);
    }

    #[test]
    fn visibility_flags_respected_on_wide_terminal() {
        let l = PaneLayout::compute(rect(140, 40), false, false);
        assert!(l.tree.is_none());
        assert!(l.tools.is_none());
    }
}
