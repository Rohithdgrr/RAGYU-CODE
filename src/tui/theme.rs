//! TUI color themes.
//!
//! The TUI ships a light-first palette (the whole point of the redesign —
//! every other coding-agent CLI is dark-only) plus a dark fallback. The
//! active theme lives in a process-wide `RwLock` so any pane can ask for
//! colors without threading a `Theme` through every call; a poisoned lock
//! falls back to the last-known value instead of panicking mid-frame.

use std::sync::RwLock;

use ratatui::style::{Color, Modifier, Style};

/// Every color role the TUI renders through. Widgets never hard-code
/// `Color::*`; they go through this struct so `/theme` can swap palettes
/// live.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    // Backgrounds
    pub bg_primary: Color,
    pub bg_secondary: Color,
    pub bg_tertiary: Color,
    pub bg_hover: Color,

    // Accents
    pub accent_primary: Color,
    pub accent_secondary: Color,
    pub accent_success: Color,
    pub accent_warning: Color,
    pub accent_error: Color,

    // Text
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub text_inverse: Color,

    // Borders
    pub border_default: Color,
    pub border_focus: Color,

    // Syntax highlighting (light-bg compatible)
    pub syntax_keyword: Color,
    pub syntax_string: Color,
    pub syntax_comment: Color,
    pub syntax_function: Color,
    pub syntax_type: Color,
}

impl Theme {
    pub fn name(&self) -> &'static str {
        if self.bg_primary == LIGHT_THEME.bg_primary {
            "light"
        } else {
            "dark"
        }
    }

    /// Base text style for a pane's body.
    pub fn text(&self) -> Style {
        Style::default().fg(self.text_primary).bg(self.bg_primary)
    }

    /// Dimmed/secondary text.
    pub fn text_dim(&self) -> Style {
        Style::default().fg(self.text_secondary).bg(self.bg_primary)
    }

    /// Border style; focused panes get the accent border per the spec.
    pub fn border_style(&self, focused: bool) -> Style {
        Style::default().fg(if focused {
            self.border_focus
        } else {
            self.border_default
        })
    }

    /// Accent-tinted text (titles, highlights).
    pub fn accent_text(&self, focused: bool) -> Style {
        Style::default()
            .fg(if focused {
                self.accent_primary
            } else {
                self.accent_secondary
            })
            .add_modifier(Modifier::BOLD)
    }

    /// User message marker / success states.
    pub fn success(&self) -> Style {
        Style::default().fg(self.accent_success)
    }

    pub fn warning(&self) -> Style {
        Style::default().fg(self.accent_warning)
    }

    pub fn error(&self) -> Style {
        Style::default().fg(self.accent_error)
    }

    /// Sidebar background (file tree, tool panel).
    pub fn sidebar_bg(&self) -> Style {
        Style::default().fg(self.text_primary).bg(self.bg_secondary)
    }
}

/// Apple-inspired light theme — the signature look of the redesign.
pub const LIGHT_THEME: Theme = Theme {
    bg_primary: Color::Rgb(250, 250, 252),
    bg_secondary: Color::Rgb(245, 245, 247),
    bg_tertiary: Color::Rgb(255, 255, 255),
    bg_hover: Color::Rgb(235, 235, 240),

    accent_primary: Color::Rgb(0, 122, 255),
    accent_secondary: Color::Rgb(88, 86, 214),
    accent_success: Color::Rgb(52, 199, 89),
    accent_warning: Color::Rgb(255, 149, 0),
    accent_error: Color::Rgb(255, 59, 48),

    text_primary: Color::Rgb(28, 28, 30),
    text_secondary: Color::Rgb(99, 99, 102),
    text_muted: Color::Rgb(142, 142, 147),
    text_inverse: Color::Rgb(255, 255, 255),

    border_default: Color::Rgb(200, 200, 205),
    border_focus: Color::Rgb(0, 122, 255),

    syntax_keyword: Color::Rgb(175, 0, 145),
    syntax_string: Color::Rgb(196, 26, 22),
    syntax_comment: Color::Rgb(0, 128, 0),
    syntax_function: Color::Rgb(121, 93, 163),
    syntax_type: Color::Rgb(0, 103, 163),
};

/// Dark counterpart for night sessions (`/theme dark`).
pub const DARK_THEME: Theme = Theme {
    bg_primary: Color::Rgb(22, 22, 26),
    bg_secondary: Color::Rgb(30, 30, 35),
    bg_tertiary: Color::Rgb(40, 40, 46),
    bg_hover: Color::Rgb(52, 52, 60),

    accent_primary: Color::Rgb(64, 156, 255),
    accent_secondary: Color::Rgb(125, 122, 255),
    accent_success: Color::Rgb(78, 201, 120),
    accent_warning: Color::Rgb(255, 179, 71),
    accent_error: Color::Rgb(255, 105, 97),

    text_primary: Color::Rgb(235, 235, 240),
    text_secondary: Color::Rgb(160, 160, 168),
    text_muted: Color::Rgb(110, 110, 118),
    text_inverse: Color::Rgb(22, 22, 26),

    border_default: Color::Rgb(64, 64, 72),
    border_focus: Color::Rgb(64, 156, 255),

    syntax_keyword: Color::Rgb(255, 121, 222),
    syntax_string: Color::Rgb(231, 106, 106),
    syntax_comment: Color::Rgb(106, 153, 85),
    syntax_function: Color::Rgb(220, 190, 255),
    syntax_type: Color::Rgb(79, 193, 233),
};

static ACTIVE: RwLock<Theme> = RwLock::new(LIGHT_THEME);

/// Snapshot of the active theme. On a poisoned lock we recover the value
/// from inside the guard rather than panic — a torn frame beats a crash.
pub fn active() -> Theme {
    *ACTIVE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn set(theme: Theme) {
    if let Ok(mut slot) = ACTIVE.write() {
        *slot = theme;
    }
}

/// Flips light ↔ dark and returns the now-active theme.
pub fn toggle() -> Theme {
    let next = if active().name() == "light" {
        DARK_THEME
    } else {
        LIGHT_THEME
    };
    set(next);
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_backgrounds() {
        assert_eq!(LIGHT_THEME.name(), "light");
        assert_eq!(DARK_THEME.name(), "dark");
    }

    #[test]
    fn toggle_round_trips() {
        let before = active().name();
        let after = toggle();
        assert_ne!(before, after.name());
        assert_eq!(active().name(), after.name());
        toggle(); // restore for other tests
    }

    #[test]
    fn focused_borders_use_focus_color() {
        let t = LIGHT_THEME;
        assert_eq!(t.border_style(true).fg, Some(t.border_focus));
        assert_eq!(t.border_style(false).fg, Some(t.border_default));
    }
}
