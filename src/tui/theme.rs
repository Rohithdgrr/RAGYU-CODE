//! TUI color themes — frosted-glass (glassmorphism), sharp edges.
//!
//! The design language: layered translucent-looking surfaces over a deep
//! backdrop, low-contrast hairline borders that read as glass edges, one
//! vivid accent glow on the focused surface. All corners are square —
//! sharpness is part of the identity ("cut glass", not "bubbles").
//!
//! Typography the palette targets (set these in your terminal):
//! - Space Grotesk / Outfit for display, DM Sans / Manrope for UI text,
//!   JetBrains Mono for numerals and code.
//!
//! The active theme lives in a process-wide `RwLock` so any pane can ask for
//! colors without threading a `Theme` through every call; a poisoned lock
//! falls back to the last-known value instead of panicking mid-frame.

use std::sync::RwLock;

use ratatui::style::{Color, Modifier, Style};

/// Every color role the TUI renders through. Widgets never hard-code
/// `Color::*`; they go through this struct so `/theme` can swap palettes
/// live.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    // Backgrounds — backdrop → frosted panel → raised glass card → hover sheen
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

    // Borders — hairline glass edges + focused glow
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

    /// Base text style for a pane's body (backdrop surface).
    pub fn text(&self) -> Style {
        Style::default().fg(self.text_primary).bg(self.bg_primary)
    }

    /// Frosted-panel text (sidebars, code blocks).
    pub fn text_on_panel(&self) -> Style {
        Style::default().fg(self.text_primary).bg(self.bg_secondary)
    }

    /// Dimmed/secondary text.
    pub fn text_dim(&self) -> Style {
        Style::default().fg(self.text_secondary).bg(self.bg_primary)
    }

    /// Border style; focused panes get the glowing accent edge.
    pub fn border_style(&self, focused: bool) -> Style {
        Style::default()
            .fg(if focused { self.border_focus } else { self.border_default })
            .bg(self.bg_primary)
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

    /// Sidebar background (file tree, tool panel) — frosted panel layer.
    pub fn sidebar_bg(&self) -> Style {
        Style::default().fg(self.text_primary).bg(self.bg_secondary)
    }
}

/// "Frosted Daylight" — misty cool-white layers with a cobalt glow.
pub const LIGHT_THEME: Theme = Theme {
    bg_primary: Color::Rgb(236, 240, 247),   // mist backdrop
    bg_secondary: Color::Rgb(224, 230, 241), // frosted panel
    bg_tertiary: Color::Rgb(248, 250, 253),  // raised glass card
    bg_hover: Color::Rgb(214, 222, 236),     // hover sheen

    accent_primary: Color::Rgb(63, 118, 227),  // glass blue
    accent_secondary: Color::Rgb(124, 92, 255), // violet
    accent_success: Color::Rgb(28, 176, 118),  // mint
    accent_warning: Color::Rgb(235, 156, 32),  // amber
    accent_error: Color::Rgb(232, 74, 74),     // rose

    text_primary: Color::Rgb(24, 30, 44),
    text_secondary: Color::Rgb(78, 90, 112),
    text_muted: Color::Rgb(140, 152, 174),
    text_inverse: Color::Rgb(248, 250, 253),

    border_default: Color::Rgb(190, 200, 218),
    border_focus: Color::Rgb(63, 118, 227),

    syntax_keyword: Color::Rgb(146, 58, 219),
    syntax_string: Color::Rgb(197, 62, 56),
    syntax_comment: Color::Rgb(96, 132, 100),
    syntax_function: Color::Rgb(122, 82, 178),
    syntax_type: Color::Rgb(20, 108, 180),
};

/// "Midnight Glass" — deep navy layers with neon edge-lighting.
pub const DARK_THEME: Theme = Theme {
    bg_primary: Color::Rgb(13, 17, 27),   // midnight backdrop
    bg_secondary: Color::Rgb(21, 27, 42), // frosted panel
    bg_tertiary: Color::Rgb(29, 37, 56),  // raised glass card
    bg_hover: Color::Rgb(38, 48, 72),     // hover sheen

    accent_primary: Color::Rgb(88, 148, 255),   // neon blue
    accent_secondary: Color::Rgb(158, 130, 255), // violet
    accent_success: Color::Rgb(52, 211, 153),   // mint neon
    accent_warning: Color::Rgb(251, 191, 36),   // amber neon
    accent_error: Color::Rgb(248, 113, 113),    // coral

    text_primary: Color::Rgb(228, 234, 245),
    text_secondary: Color::Rgb(150, 162, 186),
    text_muted: Color::Rgb(96, 108, 134),
    text_inverse: Color::Rgb(13, 17, 27),

    border_default: Color::Rgb(52, 64, 92),
    border_focus: Color::Rgb(88, 148, 255),

    syntax_keyword: Color::Rgb(255, 116, 218),
    syntax_string: Color::Rgb(255, 138, 128),
    syntax_comment: Color::Rgb(120, 168, 120),
    syntax_function: Color::Rgb(196, 181, 253),
    syntax_type: Color::Rgb(103, 202, 255),
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
