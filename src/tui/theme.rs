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
    /// Palette name (matches the REPL theme names).
    pub name: &'static str,

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
    pub syntax_number: Color,
}

impl Theme {
    pub fn name(&self) -> &'static str {
        self.name
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
pub const LIGHT_THEME: Theme = glass_light("light", GLASS_BLUE, VIOLET, MINT, AMBER, ROSE);

/// "Midnight Glass" — deep navy layers with neon edge-lighting.
pub const DARK_THEME: Theme = glass_dark("dark", NEON_BLUE, VIOLET, MINT, AMBER, CORAL);

static ACTIVE: RwLock<Theme> = RwLock::new(LIGHT_THEME);
const GLASS_BLUE: Color = Color::Rgb(63, 118, 227);
const NEON_BLUE: Color = Color::Rgb(88, 148, 255);
const VIOLET: Color = Color::Rgb(124, 92, 255);
const MINT: Color = Color::Rgb(52, 211, 153);
const AMBER: Color = Color::Rgb(251, 191, 36);
const CORAL: Color = Color::Rgb(248, 113, 113);
const ROSE: Color = Color::Rgb(232, 74, 74);
const LAVENDER: Color = Color::Rgb(167, 139, 250);
const PINK: Color = Color::Rgb(244, 114, 182);

/// Builds a light glass palette: shared misty surfaces + accent set.
const fn glass_light(
    name: &'static str,
    accent_primary: Color,
    accent_secondary: Color,
    accent_success: Color,
    accent_warning: Color,
    accent_error: Color,
) -> Theme {
    Theme {
        name,
        bg_primary: Color::Rgb(236, 240, 247),
        bg_secondary: Color::Rgb(224, 230, 241),
        bg_tertiary: Color::Rgb(248, 250, 253),
        bg_hover: Color::Rgb(214, 222, 236),
        accent_primary,
        accent_secondary,
        accent_success,
        accent_warning,
        accent_error,
        text_primary: Color::Rgb(24, 30, 44),
        text_secondary: Color::Rgb(78, 90, 112),
        text_muted: Color::Rgb(140, 152, 174),
        text_inverse: Color::Rgb(248, 250, 253),
        border_default: Color::Rgb(190, 200, 218),
        border_focus: accent_primary,
        syntax_keyword: Color::Rgb(146, 58, 219),
        syntax_string: Color::Rgb(197, 62, 56),
        syntax_comment: Color::Rgb(96, 132, 100),
        syntax_function: Color::Rgb(122, 82, 178),
        syntax_type: Color::Rgb(20, 108, 180),
        syntax_number: Color::Rgb(180, 80, 80),
    }
}

/// Builds a dark glass palette: shared midnight surfaces + accent set.
const fn glass_dark(
    name: &'static str,
    accent_primary: Color,
    accent_secondary: Color,
    accent_success: Color,
    accent_warning: Color,
    accent_error: Color,
) -> Theme {
    Theme {
        name,
        bg_primary: Color::Rgb(13, 17, 27),
        bg_secondary: Color::Rgb(21, 27, 42),
        bg_tertiary: Color::Rgb(29, 37, 56),
        bg_hover: Color::Rgb(38, 48, 72),
        accent_primary,
        accent_secondary,
        accent_success,
        accent_warning,
        accent_error,
        text_primary: Color::Rgb(228, 234, 245),
        text_secondary: Color::Rgb(150, 162, 186),
        text_muted: Color::Rgb(96, 108, 134),
        text_inverse: Color::Rgb(13, 17, 27),
        border_default: Color::Rgb(52, 64, 92),
        border_focus: accent_primary,
        syntax_keyword: Color::Rgb(255, 116, 218),
        syntax_string: Color::Rgb(255, 138, 128),
        syntax_comment: Color::Rgb(120, 168, 120),
        syntax_function: Color::Rgb(196, 181, 253),
        syntax_type: Color::Rgb(103, 202, 255),
        syntax_number: Color::Rgb(255, 199, 116),
    }
}

/// All named palettes — the same names as the REPL's `render::THEMES`, each
/// mapped onto one of the two glass bases with its own accent set.
pub const NAMED_THEMES: &[Theme] = &[
    // "default" — midnight glass with the classic cyan glow.
    glass_dark("default", Color::Rgb(34, 211, 238), Color::Rgb(125, 211, 252), MINT, AMBER, CORAL),
    // "mono" — frosted daylight, greyscale accents.
    glass_light("mono", Color::Rgb(60, 60, 68), Color::Rgb(120, 120, 128), Color::Rgb(40, 40, 44), Color::Rgb(110, 110, 116), Color::Rgb(160, 30, 30)),
    // "dracula" — dark base, signature violet/pink.
    glass_dark("dracula", Color::Rgb(189, 147, 249), Color::Rgb(255, 121, 198), Color::Rgb(80, 250, 123), Color::Rgb(241, 250, 140), Color::Rgb(255, 85, 85)),
    // "solarized" — daylight base, warm gold over blue.
    glass_light("solarized", Color::Rgb(181, 137, 0), Color::Rgb(38, 139, 210), Color::Rgb(133, 153, 0), Color::Rgb(203, 75, 22), Color::Rgb(220, 50, 47)),
    // "ocean" — deep teal glass.
    glass_dark("ocean", Color::Rgb(0, 180, 216), Color::Rgb(0, 119, 182), Color::Rgb(2, 195, 154), AMBER, Color::Rgb(214, 40, 40)),
    // "nord" — frosty aurora blues.
    glass_dark("nord", Color::Rgb(136, 192, 208), Color::Rgb(129, 161, 193), Color::Rgb(163, 190, 140), Color::Rgb(235, 203, 139), Color::Rgb(191, 97, 106)),
    // "gruvbox" — warm retro amber/orange.
    glass_dark("gruvbox", Color::Rgb(250, 189, 47), Color::Rgb(254, 128, 25), Color::Rgb(184, 187, 38), Color::Rgb(250, 189, 47), Color::Rgb(251, 73, 52)),
    // "tokyo-night" — indigo/neon city glow.
    glass_dark("tokyo-night", Color::Rgb(122, 162, 247), Color::Rgb(187, 154, 247), Color::Rgb(115, 218, 202), Color::Rgb(224, 175, 104), Color::Rgb(247, 118, 142)),
    // "catppuccin" — pastel mocha.
    glass_dark("catppuccin", Color::Rgb(203, 166, 247), Color::Rgb(245, 194, 231), Color::Rgb(166, 227, 161), Color::Rgb(249, 226, 175), Color::Rgb(243, 139, 168)),
    // "rose" — dusky rose glass.
    glass_dark("rose", Color::Rgb(235, 111, 146), Color::Rgb(196, 167, 231), Color::Rgb(156, 207, 176), Color::Rgb(240, 200, 130), Color::Rgb(235, 111, 146)),
];

/// Switches to a named theme; returns `false` for unknown names.
pub fn set_by_name(name: &str) -> bool {
    match NAMED_THEMES
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(name.trim()))
    {
        Some(t) => {
            set(*t);
            true
        }
        None => false,
    }
}

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

/// Remaps the accent colors of the current theme to match a provider.
/// Called live when the user switches providers via `/provider`.
pub fn apply_provider_accent(provider: &str) {
    let accent = match provider {
        "omniroute" => Color::Rgb(124, 58, 237),
        "mistral" => Color::Rgb(63, 118, 227),
        "openai" => Color::Rgb(16, 163, 127),
        "kimi" => Color::Rgb(124, 92, 255),
        "groq" => Color::Rgb(249, 115, 22),
        "ollama" => Color::Rgb(107, 114, 128),
        "deepseek" => Color::Rgb(14, 165, 233),
        "nvidia" => Color::Rgb(118, 185, 0),
        "bytez" => Color::Rgb(225, 29, 72),
        "gemini" => Color::Rgb(66, 133, 244),
        "glm" => LAVENDER,
        "minimax" => PINK,
        _ => Color::Rgb(63, 118, 227), // fallback to cobalt
    };
    let accent2 = match provider {
        "omniroute" => Color::Rgb(167, 139, 250),
        "mistral" => Color::Rgb(88, 148, 255),
        "openai" => Color::Rgb(52, 211, 153),
        "kimi" => Color::Rgb(167, 139, 250),
        "groq" => Color::Rgb(251, 191, 36),
        "ollama" => Color::Rgb(156, 163, 175),
        "deepseek" => Color::Rgb(56, 189, 248),
        "nvidia" => Color::Rgb(163, 230, 53),
        "bytez" => Color::Rgb(251, 113, 133),
        "gemini" => Color::Rgb(66, 133, 244),
        "glm" => Color::Rgb(167, 139, 250),
        "minimax" => Color::Rgb(244, 114, 182),
        _ => Color::Rgb(88, 148, 255),
    };
    if let Ok(mut slot) = ACTIVE.write() {
        slot.accent_primary = accent;
        slot.accent_secondary = accent2;
        slot.border_focus = accent;
    }
}

/// Flips between the light and dark glass bases and returns the now-active
/// theme. Named palettes are preserved as targets: `mono` represents the
/// light base, `default` the dark one.
#[allow(clippy::expect_used)] // safe: "default" and "mono" are always in NAMED_THEMES
pub fn toggle() -> Theme {
    let next = if active().bg_primary == LIGHT_THEME.bg_primary {
        *NAMED_THEMES.iter().find(|t| t.name == "default").expect("default theme exists")
    } else {
        *NAMED_THEMES.iter().find(|t| t.name == "mono").expect("mono theme exists")
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

    #[test]
    fn all_ten_repl_theme_names_resolve() {
        for name in crate::render::theme_names() {
            assert!(
                NAMED_THEMES.iter().any(|t| t.name == name),
                "TUI palette missing for REPL theme '{name}'"
            );
            assert!(set_by_name(name), "set_by_name('{name}') failed");
        }
        assert!(!set_by_name("definitely-not-a-theme"));
    }

    #[test]
    fn named_palettes_keep_their_names() {
        for t in NAMED_THEMES {
            assert_eq!(t.name(), t.name);
        }
    }
}
