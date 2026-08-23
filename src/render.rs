use crossterm::{
    cursor::MoveToColumn,
    execute,
    style::{Color, Print, Stylize},
    terminal::{Clear, ClearType},
};
use std::io::{IsTerminal, Write};
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

pub fn stdout_is_tty() -> bool {
    std::io::stdout().is_terminal()
}

/// Named color themes for the prompt and UI accents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub accent: Color,
    pub bot: Color,
    pub ok: Color,
    pub err: Color,
    pub dim: Color,
}

pub const THEMES: &[Theme] = &[
    Theme {
        name: "default",
        accent: Color::Cyan,
        bot: Color::Cyan,
        ok: Color::Green,
        err: Color::Red,
        dim: Color::DarkGrey,
    },
    Theme {
        name: "mono",
        accent: Color::White,
        bot: Color::White,
        ok: Color::White,
        err: Color::Red,
        dim: Color::Grey,
    },
    Theme {
        name: "dracula",
        accent: Color::DarkMagenta,
        bot: Color::Magenta,
        ok: Color::Green,
        err: Color::Red,
        dim: Color::DarkGrey,
    },
    Theme {
        name: "solarized",
        accent: Color::Yellow,
        bot: Color::Blue,
        ok: Color::Green,
        err: Color::Red,
        dim: Color::DarkGrey,
    },
    Theme {
        name: "ocean",
        accent: Color::Blue,
        bot: Color::Cyan,
        ok: Color::Green,
        err: Color::Red,
        dim: Color::DarkGrey,
    },
    Theme {
        name: "nord",
        accent: Color::DarkBlue,
        bot: Color::Cyan,
        ok: Color::DarkGreen,
        err: Color::Red,
        dim: Color::Grey,
    },
    Theme {
        name: "gruvbox",
        accent: Color::Yellow,
        bot: Color::DarkYellow,
        ok: Color::Green,
        err: Color::DarkRed,
        dim: Color::DarkGrey,
    },
    Theme {
        name: "tokyo-night",
        accent: Color::Magenta,
        bot: Color::Blue,
        ok: Color::DarkGreen,
        err: Color::Red,
        dim: Color::DarkGrey,
    },
    Theme {
        name: "catppuccin",
        accent: Color::DarkMagenta,
        bot: Color::Blue,
        ok: Color::DarkGreen,
        err: Color::DarkRed,
        dim: Color::Grey,
    },
    Theme {
        name: "rose",
        accent: Color::DarkRed,
        bot: Color::Magenta,
        ok: Color::Green,
        err: Color::Red,
        dim: Color::DarkGrey,
    },
];

static THEME: RwLock<Theme> = RwLock::new(THEMES[0]);

// A poisoned lock here can only mean a panic while holding it; recovering to
// the default theme is safe and keeps the CLI alive.
fn theme_read() -> Theme {
    match THEME.read() {
        Ok(g) => *g,
        Err(_) => THEMES[0],
    }
}

/// Sets the active theme by name (case-insensitive); returns false when unknown.
pub fn set_theme(name: &str) -> bool {
    match THEMES.iter().find(|t| t.name.eq_ignore_ascii_case(name)) {
        Some(t) => {
            *match THEME.write() {
                Ok(mut g) => g,
                Err(e) => e.into_inner(),
            } = *t;
            true
        }
        None => false,
    }
}

pub fn active_theme() -> Theme {
    theme_read()
}

pub fn theme_names() -> impl Iterator<Item = &'static str> {
    THEMES.iter().map(|t| t.name)
}

fn themed(role: ThemeRole) -> Color {
    let t = active_theme();
    match role {
        ThemeRole::Accent => t.accent,
        ThemeRole::Bot => t.bot,
        ThemeRole::Ok => t.ok,
        ThemeRole::Err => t.err,
        ThemeRole::Dim => t.dim,
    }
}

#[derive(Clone, Copy)]
enum ThemeRole {
    Accent,
    Bot,
    Ok,
    Err,
    Dim,
}

/// Convenience accessors over the active theme.
pub fn accent() -> Color {
    themed(ThemeRole::Accent)
}
pub fn bot_color() -> Color {
    themed(ThemeRole::Bot)
}
pub fn ok_color() -> Color {
    themed(ThemeRole::Ok)
}
pub fn err_color() -> Color {
    themed(ThemeRole::Err)
}
pub fn dim_color() -> Color {
    themed(ThemeRole::Dim)
}

/// Colorizes text only when stdout is a TTY, so piped output stays clean.
pub fn paint(s: impl Into<String>, color: Color) -> String {
    let s = s.into();
    if stdout_is_tty() {
        s.with(color).to_string()
    } else {
        s
    }
}

/// Markdown renderer with plain-text fallback.
///
/// In rendered mode the finished answer is printed once, fully formatted.
/// In raw mode deltas stream straight to stdout as they arrive.
#[derive(Debug, Clone, Copy, Default)]
pub struct Renderer {
    markdown: bool,
}

impl Renderer {
    pub fn new(markdown: bool) -> Self {
        Self {
            markdown: markdown && stdout_is_tty(),
        }
    }

    pub fn markdown_enabled(&self) -> bool {
        self.markdown
    }

    pub fn set_markdown(&mut self, on: bool) {
        self.markdown = on && stdout_is_tty();
    }

    pub fn render_answer(&self, md: &str) {
        if md.trim().is_empty() {
            return;
        }
        if self.markdown {
            termimad::print_text(md);
        } else {
            println!("{md}");
        }
    }
}

/// Renders a unified diff with ANSI colors: `+` lines in the OK color,
/// `-` lines in the error color, hunk headers accented, everything else
/// dimmed. Piped output stays uncolored (see [`paint`]).
pub fn render_diff(diff: &str) {
    for line in diff.lines() {
        let color = if line.starts_with("+++") || line.starts_with("---") {
            dim_color()
        } else if line.starts_with('+') {
            ok_color()
        } else if line.starts_with('-') {
            err_color()
        } else if line.starts_with("@@") {
            accent()
        } else {
            dim_color()
        };
        println!("{}", paint(line.to_owned(), color));
    }
}

/// Emoji + display name badge for a source file, or `None` for files with no
/// recognized language (plain `📄` still shows in the breadcrumb).
pub fn language_badge(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?;
    match ext.to_ascii_lowercase().as_str() {
        "rs" => Some("🦀 Rust"),
        "py" => Some("🐍 Python"),
        "js" | "jsx" | "mjs" | "cjs" => Some("🟨 JavaScript"),
        "ts" | "tsx" => Some("🔷 TypeScript"),
        "go" => Some("🐹 Go"),
        _ => None,
    }
}

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Animated single-line spinner for rendered mode. Inert when disabled or
/// when stdout is not a terminal. Always call `.stop().await` after the
/// request settles; `Drop` also stops the animation as a safety net.
pub struct Spinner {
    stop: Arc<AtomicBool>,
    active: bool,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl Spinner {
    pub fn start(label: &str, enabled: bool) -> Self {
        if !enabled || !stdout_is_tty() {
            return Self {
                stop: Arc::new(AtomicBool::new(false)),
                active: false,
                handle: None,
            };
        }
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let label = label.to_owned();
        let handle = tokio::spawn(async move {
            let mut i = 0usize;
            while !flag.load(Ordering::Relaxed) {
                let _ = execute!(
                    std::io::stdout(),
                    MoveToColumn(0),
                    Clear(ClearType::CurrentLine),
                    Print(format!(
                        "{} {label}",
                        paint(SPINNER_FRAMES[i % SPINNER_FRAMES.len()], accent())
                    ))
                );
                i += 1;
                tokio::time::sleep(Duration::from_millis(80)).await;
            }
            let _ = execute!(
                std::io::stdout(),
                MoveToColumn(0),
                Clear(ClearType::CurrentLine)
            );
            let _ = std::io::stdout().flush();
        });
        Self {
            stop,
            active: true,
            handle: Some(handle),
        }
    }

    pub async fn stop(mut self) {
        self.shutdown().await;
    }

    async fn shutdown(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        if self.active {
            // No await in drop — flip the flag so the task exits cleanly, and
            // abort as a hard safety net in case it is blocked on sleep/IO.
            self.stop.store(true, Ordering::Relaxed);
            if let Some(handle) = self.handle.take() {
                handle.abort();
            }
        }
    }
}
