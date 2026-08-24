//! Structured command output — the bridge that lets ONE dispatcher serve two
//! frontends.
//!
//! `commands::dispatch` writes human-readable lines via the `ok`/`dim`/`err`
//! helpers. In the REPL those print straight to stdout (zero behavior
//! change). In the TUI, printing to stdout would corrupt the alternate
//! screen, so [`dispatch_structured`] flips on capture first: every helper
//! call is buffered into role-tagged [`Msg`]s and returned as a
//! [`CommandOutput`] for the TUI to render as chat entries/notices.

/// Visual role of one output line; each frontend maps roles to its own style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Plain informational line.
    Info,
    /// Success confirmation.
    Ok,
    /// Error (frontend decides stderr vs red notice).
    Err,
    /// Warning / attention.
    Warn,
    /// Dimmed secondary detail.
    Dim,
    /// Pre-formatted markdown the frontend should render as an assistant
    /// message (e.g. a committed `/pick` variant).
    Markdown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Msg {
    pub role: Role,
    pub text: String,
}

impl Msg {
    fn new(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
        }
    }
}

/// Side-channel effects a command requests beyond printed lines.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Effect {
    #[default]
    None,
    /// Quit the frontend (`/exit`).
    ExitRequested,
    /// Send this text as a fresh user turn (`/retry`). The TUI additionally
    /// drops the trailing assistant entries so the transcript stays in sync.
    Resend(String),
    /// A confirmed plan awaiting autonomous execution (`/plan`).
    Plan(Vec<String>),
    /// The active theme changed; payload is the new theme name.
    ThemeChanged(String),
    /// Session history was replaced (`/load`, `/rewind`) — the TUI must
    /// rebuild its transcript entries from `app.session`.
    ReloadTranscript,
    /// The last exchange was removed (`/undo`) — the TUI drops the matching
    /// chat entries.
    PopExchange,
}

/// Everything a dispatched command produced: printable lines plus an effect.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CommandOutput {
    pub msgs: Vec<Msg>,
    pub effect: Effect,
}

impl CommandOutput {
    pub fn is_silent(&self) -> bool {
        self.msgs.is_empty() && self.effect == Effect::None
    }
}

use std::cell::{Cell, RefCell};

thread_local! {
    /// `Some` while a structured dispatch is capturing helper output.
    static CAPTURE: RefCell<Vec<Msg>> = const { RefCell::new(Vec::new()) };
}

/// Starts buffering helper output on this thread. Must be paired with
/// [`take_captured`].
pub fn begin_capture() {
    CAPTURE.with(|c| c.borrow_mut().clear());
    // Marker: capture is "on" whenever dispatch_structured is between begin
    // and take; we model that with a sentinel flag below.
    IN_CAPTURE.with(|f| f.set(true));
}

thread_local! {
    static IN_CAPTURE: Cell<bool> = const { Cell::new(false) };
}

/// Ends buffering and returns everything captured since [`begin_capture`].
pub fn take_captured() -> Vec<Msg> {
    IN_CAPTURE.with(|f| f.set(false));
    CAPTURE.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// True when helpers should buffer instead of printing.
pub fn capturing() -> bool {
    IN_CAPTURE.with(Cell::get)
}

/// Internal emit path used by the `ok`/`dim`/`err`/… helpers.
pub(crate) fn emit(role: Role, text: impl Into<String>) {
    let text = text.into();
    if capturing() {
        CAPTURE.with(|c| c.borrow_mut().push(Msg::new(role, text)));
    } else {
        crate::commands::print_msg(role, &text);
    }
}
