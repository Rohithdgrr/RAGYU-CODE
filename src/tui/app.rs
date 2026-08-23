//! TUI application: state machine, key handling, and the agent turn runner.
//!
//! The REPL's stdout-printing turn loop cannot run under ratatui (any stray
//! print corrupts the alternate screen), so this module re-implements the
//! turn pipeline against the TUI:
//!
//! - streaming deltas land in a shared live buffer rendered by the chat pane
//! - tool rounds execute sequentially; confirmation-gated tools are declined
//!   with a notice until Review mode lands (Phase C)
//! - results flow back over an mpsc channel so the event loop keeps reacting
//!   to keys (scroll, cancel) while the model streams
//! - the turn future owns `&mut App`; status-bar data is snapshotted before
//!   the turn starts so drawing never fights that borrow

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use super::widgets::chat_pane::ChatEntry;
use super::widgets::file_tree::FileTree;
use super::{draw, theme};
use crate::api::{self, ChatOptions, StreamSink};
use crate::commands::{self, App};

/// Upper bound on model↔tool round trips per user turn (mirrors the REPL).
const MAX_TOOL_ROUNDS: usize = 5;
/// Cap applied *before* a tool result enters session history.
const MAX_TOOL_RESULT_CHARS: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Normal,
    Agent,
    /// Reserved for Phase C (pending-edit review); not reachable yet.
    Review,
    Plan,
}

impl AppMode {
    pub fn label(self) -> &'static str {
        match self {
            AppMode::Normal => "NORMAL",
            AppMode::Agent => "AGENT",
            AppMode::Review => "REVIEW",
            AppMode::Plan => "PLAN",
        }
    }

    /// Validated transition per the workflow spec.
    fn transition_to(&mut self, next: AppMode) -> bool {
        let allowed = *self == next
            || matches!((*self, next), (AppMode::Normal, AppMode::Agent)
                | (AppMode::Agent, AppMode::Normal));
        if allowed {
            *self = next;
        }
        allowed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Input,
    Chat,
    Tree,
}

/// Live update pushed from the turn runner back to the UI thread.
pub enum TurnUpdate {
    AssistantProse(String),
    ToolStart { name: String, args: String },
    ToolEnd { name: String, args: String, ok: bool },
    /// A workspace-mutating call needs explicit approval (Review mode).
    ConfirmNeeded { name: String, args: String },
    Answer(String),
    Notice(String),
    Error(String),
}

/// User's answer to a gated-tool prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmDecision {
    Approve,
    ApproveAll,
    Decline,
}

/// Shared live-streaming buffer (written by the delta callback inside the
/// turn future, read by the draw loop). Single-task polling makes `Rc`
/// sound here.
pub type SharedStream = Rc<RefCell<String>>;

pub struct Tui {
    pub mode: AppMode,
    /// Mode to restore after a Review-mode confirmation resolves.
    prev_mode: AppMode,
    pub focus: Focus,
    pub entries: Vec<ChatEntry>,
    pub input: String,
    /// Cursor position measured in characters (not bytes/columns).
    pub input_cursor: usize,
    pub history: Vec<String>,
    /// Index of the history entry currently shown; `None` = live draft.
    history_idx: Option<usize>,
    draft: String,
    /// Lines scrolled up from the bottom; 0 = auto-follow.
    pub scroll_from_bottom: usize,
    pub streaming: SharedStream,
    pub cancel: Arc<AtomicBool>,
    pub busy: bool,
    pub quit: bool,
    pub show_tree: bool,
    pub show_tools: bool,
    /// Project sidebar, created lazily when the tree is first shown.
    pub tree: Option<FileTree>,
    pending_clear: bool,
    pending_prompt: Option<String>,
    /// Gated-tool approval channel (Some while a turn runs).
    confirm_tx: Option<tokio::sync::mpsc::UnboundedSender<ConfirmDecision>>,
    /// True while the input gate is up for a workspace-mutating call.
    pub confirm_pending: bool,
    /// Files pinned via the tree sidebar; injected into every turn's context.
    pub pinned_files: Vec<PathBuf>,
    // ── Plan mode ──
    pending_plan_task: Option<String>,
    plan_title: String,
    plan_steps: Vec<(String, bool)>,
    plan_cursor: usize,
    plan_awaiting: bool,
    plan_executing: bool,
    /// Transcript index of the live checklist entry (replaced on progress).
    plan_entry_idx: Option<usize>,
}

impl Default for Tui {
    fn default() -> Self {
        Self::new()
    }
}

impl Tui {
    pub fn new() -> Self {
        Self {
            mode: AppMode::Normal,
            prev_mode: AppMode::Normal,
            focus: Focus::Input,
            entries: Vec::new(),
            input: String::new(),
            input_cursor: 0,
            history: Vec::new(),
            history_idx: None,
            draft: String::new(),
            scroll_from_bottom: 0,
            streaming: Rc::new(RefCell::new(String::new())),
            cancel: Arc::new(AtomicBool::new(false)),
            busy: false,
            quit: false,
            show_tree: false,
            show_tools: true,
            tree: None,
            pending_clear: false,
            pending_prompt: None,
            confirm_tx: None,
            confirm_pending: false,
            pinned_files: Vec::new(),
            pending_plan_task: None,
            plan_title: String::new(),
            plan_steps: Vec::new(),
            plan_cursor: 0,
            plan_awaiting: false,
            plan_executing: false,
            plan_entry_idx: None,
        }
    }

    pub fn notice(&mut self, text: impl Into<String>) {
        self.entries.push(ChatEntry::Notice(text.into()));
        self.scroll_from_bottom = 0;
    }

    /// Consumes a queued user prompt (Enter on a plain message).
    pub fn take_submission(&mut self) -> Option<String> {
        self.pending_prompt.take()
    }

    /// Consumes a pending conversation-clear request (Ctrl+L, `/clear`).
    pub fn take_clear_request(&mut self) -> bool {
        std::mem::take(&mut self.pending_clear)
    }

    /// Applies Enter in the input bar. Slash commands are handled locally
    /// (the full dispatcher prints to stdout and would corrupt the screen);
    /// plain prompts queue up as agent turns.
    fn submit(&mut self) {
        let line = self.input.trim().to_owned();
        if line.is_empty() {
            return;
        }
        self.input.clear();
        self.input_cursor = 0;

        if line.starts_with('/') {
            self.local_command(&line);
            return;
        }
        self.history.push(line.clone());
        self.history_idx = None;
        self.draft.clear();
        self.entries.push(ChatEntry::User(line.clone()));
        self.scroll_from_bottom = 0;
        self.pending_prompt = Some(line);
    }

    fn local_command(&mut self, line: &str) {
        let (cmd, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        match cmd.to_ascii_lowercase().as_str() {
            "/quit" | "/exit" | "/q" => self.quit = true,
            "/clear" | "/reset" => self.pending_clear = true,
            "/help" => {
                self.notice(
                    "keys: Tab focus · ↑/↓ history|scroll|tree · Space expand dir · Enter \
                     open/pin file · F5 refresh tree · Esc clear/cancel · Ctrl+C cancel stream \
                     · Ctrl+L clear · Ctrl+T left tree · Ctrl+P explorer · Ctrl+Q quit\n\
                     gated calls pause in [REVIEW]: y approve · n decline · a all\n\
                     cmds: /help /clear /theme /tokens /agent <on|off> /plan <task>",
                );
            }
            "/theme" => {
                let t = theme::toggle();
                self.notice(format!("switched to the {} theme", t.name()));
            }
            "/tokens" => {} // rendered live in the status bar
            "/plan" => {
                let task = rest.trim();
                if task.is_empty() {
                    self.notice("usage: /plan <task> — decompose a task and execute step by step");
                } else {
                    self.pending_plan_task = Some(task.to_owned());
                    self.notice("planning…");
                }
            }
            "/pin" => {
                if let Some(tree) = &self.tree {
                    if let Some(path) = tree
                        .selected_node()
                        .filter(|n| !n.is_dir)
                        .map(|n| n.rel.clone())
                        .map(|rel| {
                            std::env::current_dir()
                                .unwrap_or_else(|_| PathBuf::from("."))
                                .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR))
                        })
                    {
                        self.pin_file(path);
                    } else {
                        self.notice("select a file in the explorer first (Ctrl+T / Ctrl+P).");
                    }
                } else {
                    self.notice("open explorer with Ctrl+T or Ctrl+P, then Enter pins the selected file.");
                }
            }
            "/agent" => {
                let requested = match rest.trim() {
                    "on" => Some(AppMode::Agent),
                    "off" => Some(AppMode::Normal),
                    "" => None,
                    _ => {
                        self.notice("usage: /agent <on|off>");
                        None
                    }
                };
                match requested {
                    Some(next) => {
                        if self.mode.transition_to(next) {
                            self.notice(format!("mode: {}", self.mode.label()));
                        } else {
                            self.notice(format!(
                                "cannot enter {} from {}",
                                next.label(),
                                self.mode.label()
                            ));
                        }
                    }
                    None => self.notice(format!("mode: {}", self.mode.label())),
                }
            }
            other => {
                self.notice(format!(
                    "'{other}' is not wired into the TUI yet — run with --repl for the full \
                     REPL command set"
                ));
            }
        }
    }

    pub fn handle_event(&mut self, ev: Event) {
        if let Event::Key(key) = ev
            && key.kind == crossterm::event::KeyEventKind::Press
        {
            self.handle_key(key);
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // Review-mode prompt intercepts everything except quit.
        if self.confirm_pending {
            match (key.modifiers, key.code) {
                (KeyModifiers::CONTROL, KeyCode::Char('q')) => self.quit = true,
                (_, KeyCode::Char('y')) => self.answer_confirm(ConfirmDecision::Approve),
                (_, KeyCode::Char('n')) | (_, KeyCode::Esc) => {
                    self.answer_confirm(ConfirmDecision::Decline)
                }
                (_, KeyCode::Char('a')) => self.answer_confirm(ConfirmDecision::ApproveAll),
                _ => {}
            }
            return;
        }

        // Global bindings.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('q') => {
                    self.quit = true;
                    return;
                }
                KeyCode::Char('c') => {
                    if self.busy {
                        self.cancel.store(true, Ordering::Relaxed);
                    } else {
                        self.input.clear();
                        self.input_cursor = 0;
                    }
                    return;
                }
                KeyCode::Char('l') => {
                    self.pending_clear = true;
                    return;
                }
                KeyCode::Char('t') => {
                    self.toggle_tree();
                    return;
                }
                KeyCode::Char('p') => {
                    self.toggle_explorer();
                    return;
                }
                _ => {}
            }
        }

        // Plan confirmation gate: y executes, anything else aborts.
        if self.plan_awaiting {
            match key.code {
                KeyCode::Char('y') | KeyCode::Enter => self.approve_plan(),
                _ => self.cancel_plan(),
            }
            return;
        }

        match key.code {
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Input => Focus::Chat,
                    Focus::Chat if self.tree.is_some() && (self.show_tree || self.show_tools) => Focus::Tree,
                    Focus::Chat => Focus::Input,
                    Focus::Tree => Focus::Input,
                };
            }
            KeyCode::F(5) => {
                if let Some(tree) = &mut self.tree {
                    tree.refresh();
                }
            }
            KeyCode::Esc => {
                if !self.input.is_empty() {
                    self.input.clear();
                    self.input_cursor = 0;
                } else if self.busy {
                    self.cancel.store(true, Ordering::Relaxed);
                } else if let Some(tree) = &mut self.tree {
                    // Collapse back to a fresh selection at the root.
                    tree.refresh();
                }
            }
            _ => match self.focus {
                Focus::Input => self.handle_input_key(key),
                Focus::Chat => self.handle_chat_key(key),
                Focus::Tree => self.handle_tree_key(key),
            },
        }
    }

    /// Sends the user's answer for the pending gated call and restores the
    /// pre-review mode.
    fn answer_confirm(&mut self, decision: ConfirmDecision) {
        if let Some(tx) = &self.confirm_tx {
            let _ = tx.send(decision);
        }
        self.confirm_pending = false;
        self.mode = self.prev_mode;
    }

    fn ensure_tree(&mut self) {
        if self.tree.is_none() {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            self.tree = Some(FileTree::open(&cwd));
            if !self.pinned_files.is_empty() {
                self.notice(format!(
                    "📎 {} pinned file(s) still ride along in every turn",
                    self.pinned_files.len()
                ));
            }
        }
    }

    /// Ctrl+T: open/close left project tree.
    fn toggle_tree(&mut self) {
        if self.tree.is_none() {
            self.ensure_tree();
            self.show_tree = true;
            return;
        }
        self.show_tree = !self.show_tree;
        if self.show_tree {
            self.ensure_tree();
        }
    }

    /// Ctrl+P: open/close right file explorer (replaced tools panel).
    fn toggle_explorer(&mut self) {
        if self.tree.is_none() {
            self.ensure_tree();
            self.show_tools = true;
            return;
        }
        self.show_tools = !self.show_tools;
        if self.show_tools {
            self.ensure_tree();
        }
    }

    fn handle_tree_key(&mut self, key: KeyEvent) {
        let Some(tree) = &mut self.tree else { return };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => tree.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => tree.move_selection(1),
            KeyCode::PageUp => tree.move_selection(-10),
            KeyCode::PageDown => tree.move_selection(10),
            KeyCode::Home => tree.move_selection(-1_000_000),
            KeyCode::End => tree.move_selection(1_000_000),
            KeyCode::Char(' ') => tree.toggle_selected(),
            KeyCode::Enter => {
                if let Some(path) = tree.activate_selected() {
                    self.pin_file(path);
                }
            }
            KeyCode::Char('g') => tree.refresh_git(),
            _ => {}
        }
    }

    /// Pins a workspace file so it rides along in every future turn's
    /// context injection.
    pub fn pin_file(&mut self, path: PathBuf) {
        if self.pinned_files.contains(&path) {
            self.notice("already pinned.");
            return;
        }
        let rel = path
            .strip_prefix(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        self.pinned_files.push(path);
        self.notice(format!("📎 pinned {rel} to context"));
    }

    pub fn take_plan_request(&mut self) -> Option<String> {
        self.pending_plan_task.take()
    }

    fn handle_input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.submit(),
            KeyCode::Char(c) => {
                let byte = self.cursor_byte();
                self.input.insert(byte, c);
                self.input_cursor += 1;
            }
            KeyCode::Backspace if self.input_cursor > 0 => {
                let byte = self.cursor_byte();
                let prev = self.input[..byte]
                    .char_indices()
                    .next_back()
                    .map_or(0, |(i, _)| i);
                self.input.replace_range(prev..byte, "");
                self.input_cursor -= 1;
            }
            KeyCode::Delete if self.input_cursor < self.input.chars().count() => {
                let byte = self.cursor_byte();
                let next = self.input[byte..]
                    .char_indices()
                    .nth(1)
                    .map_or(self.input.len(), |(i, _)| byte + i);
                self.input.replace_range(byte..next, "");
            }
            KeyCode::Left if self.input_cursor > 0 => self.input_cursor -= 1,
            KeyCode::Right if self.input_cursor < self.input.chars().count() => {
                self.input_cursor += 1;
            }
            KeyCode::Home => self.input_cursor = 0,
            KeyCode::End => self.input_cursor = self.input.chars().count(),
            KeyCode::Up => self.history_prev(),
            KeyCode::Down => self.history_next(),
            _ => {}
        }
    }

    fn handle_chat_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::PageUp => {
                let step = usize::from(key.code == KeyCode::PageUp) * 9 + 1;
                self.scroll_from_bottom += step;
            }
            KeyCode::Down | KeyCode::PageDown => {
                let step = usize::from(key.code == KeyCode::PageDown) * 9 + 1;
                self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(step);
            }
            KeyCode::End => self.scroll_from_bottom = 0,
            _ => {}
        }
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.history_idx {
            None => {
                self.draft = self.input.clone();
                self.history.len() - 1
            }
            Some(idx) => idx.saturating_sub(1),
        };
        self.history_idx = Some(next);
        let text = self.history[next].clone();
        self.set_input(text);
    }

    fn history_next(&mut self) {
        let Some(idx) = self.history_idx else {
            return;
        };
        if idx + 1 < self.history.len() {
            self.history_idx = Some(idx + 1);
            let text = self.history[idx + 1].clone();
            self.set_input(text);
        } else {
            self.history_idx = None;
            let draft = std::mem::take(&mut self.draft);
            self.set_input(draft);
        }
    }

    fn set_input(&mut self, text: String) {
        self.input_cursor = text.chars().count();
        self.input = text;
    }

    fn cursor_byte(&self) -> usize {
        self.input
            .char_indices()
            .nth(self.input_cursor)
            .map_or(self.input.len(), |(i, _)| i)
    }

    pub fn apply_update(&mut self, upd: TurnUpdate) {
        match upd {
            TurnUpdate::AssistantProse(text) => {
                self.entries.push(ChatEntry::Assistant(text));
            }
            TurnUpdate::ToolStart { name, args } => {
                self.entries.push(ChatEntry::Tool {
                    name,
                    args,
                    ok: None,
                });
                self.scroll_from_bottom = 0;
            }
            TurnUpdate::ToolEnd { name, args, ok } => {
                if let Some(ChatEntry::Tool { ok: slot, .. }) = self
                    .entries
                    .iter_mut()
                    .rev()
                    .find(|e| matches!(e, ChatEntry::Tool { ok: None, .. }))
                {
                    *slot = Some(ok);
                } else {
                    self.entries.push(ChatEntry::Tool {
                        name,
                        args,
                        ok: Some(ok),
                    });
                }
            }
            TurnUpdate::ConfirmNeeded { name, args } => {
                // Back-to-back gates keep the original pre-review mode.
                if !self.confirm_pending {
                    self.prev_mode = self.mode;
                }
                self.mode = AppMode::Review;
                self.confirm_pending = true;
                self.entries.push(ChatEntry::Tool {
                    name,
                    args,
                    ok: None,
                });
                self.entries.push(ChatEntry::Notice(
                    "⚠ workspace change — [y] approve · [n] decline · [a] approve all remaining"
                        .into(),
                ));
                self.scroll_from_bottom = 0;
            }
            TurnUpdate::Answer(text) => {
                self.streaming.borrow_mut().clear();
                self.entries.push(ChatEntry::Assistant(text));
                self.scroll_from_bottom = 0;
            }
            TurnUpdate::Notice(text) => self.notice(text),
            TurnUpdate::Error(text) => {
                self.entries
                    .push(ChatEntry::Notice(format!("error: {text}")));
                self.scroll_from_bottom = 0;
            }
        }
    }

    /// Called when a turn future resolves: resets transient state and
    /// salvages anything left in the live buffer (interrupted streams).
    pub fn finish_turn(&mut self, interrupted: bool) {
        self.busy = false;
        self.cancel.store(false, Ordering::Relaxed);
        self.confirm_pending = false;
        if self.mode == AppMode::Review {
            self.mode = self.prev_mode;
        }
        let leftover = std::mem::take(&mut *self.streaming.borrow_mut());
        if interrupted && !leftover.trim().is_empty() {
            self.entries
                .push(ChatEntry::Assistant(format!("{leftover}\n\n*(interrupted)*")));
        }
    }

    // ── Plan mode ────────────────────────────────────────────────────────

    /// Presents a freshly generated plan and waits for y/N.
    pub fn start_plan(&mut self, title: &str, steps: Vec<String>) {
        self.plan_title = title.to_owned();
        self.plan_steps = steps.iter().map(|s| (s.clone(), false)).collect();
        self.plan_cursor = 0;
        self.plan_awaiting = true;
        self.plan_executing = false;
        self.push_checklist();
        self.notice("proceed? [y] execute step by step · anything else aborts");
    }

    fn push_checklist(&mut self) {
        let entry = ChatEntry::Checklist {
            title: self.plan_title.clone(),
            steps: self.plan_steps.clone(),
        };
        match self.plan_entry_idx {
            Some(idx) if idx < self.entries.len() => self.entries[idx] = entry,
            _ => {
                self.entries.push(entry);
                self.plan_entry_idx = Some(self.entries.len() - 1);
            }
        }
        self.scroll_from_bottom = 0;
    }

    fn approve_plan(&mut self) {
        self.plan_awaiting = false;
        if self.plan_steps.is_empty() {
            return;
        }
        self.plan_executing = true;
        let (step, _) = self.plan_steps[self.plan_cursor].clone();
        self.pending_prompt =
            Some(format!("[plan step {}/{}] {}", self.plan_cursor + 1, self.plan_steps.len(), step));
        self.focus = Focus::Input;
    }

    fn cancel_plan(&mut self) {
        self.plan_awaiting = false;
        self.plan_executing = false;
        self.plan_entry_idx = None;
        self.plan_steps.clear();
        self.notice("plan aborted — steps kept visible in the transcript.");
    }

    /// Called after each completed turn while a plan is executing: marks the
    /// finished step, queues the next one, or closes out the plan.
    ///
    /// Returns the index of the step that just completed so the caller can
    /// sync `/todo` state (`App` is owned by the caller).
    pub fn advance_plan(&mut self) -> Option<usize> {
        if !self.plan_executing || self.plan_awaiting {
            return None;
        }
        let mut completed = None;
        if self.plan_cursor < self.plan_steps.len() {
            self.plan_steps[self.plan_cursor].1 = true;
            completed = Some(self.plan_cursor);
            self.push_checklist();
            self.plan_cursor += 1;
        }
        if self.plan_cursor < self.plan_steps.len() {
            let (step, _) = self.plan_steps[self.plan_cursor].clone();
            self.pending_prompt = Some(format!(
                "[plan step {}/{}] {}",
                self.plan_cursor + 1,
                self.plan_steps.len(),
                step
            ));
        } else {
            self.notice("✓ plan complete.");
            self.plan_executing = false;
            self.plan_entry_idx = None;
        }
        completed
    }

    /// True while a plan is mid-execution and more steps are queued.
    pub fn plan_in_flight(&self) -> bool {
        self.plan_executing
    }
}

// ─── Entry point ─────────────────────────────────────────────────────────────

/// Runs the TUI until quit; restores the terminal on any exit path.
pub async fn run(app: &mut App) -> Result<()> {
    let mut stdout = std::io::stdout();
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = event_loop(app, &mut terminal).await;

    // Restore unconditionally — a leaked alternate screen ruins the shell.
    let _ = crossterm::terminal::disable_raw_mode();
    let _ =
        crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
    result
}

async fn event_loop(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
) -> Result<()> {
    let mut tui = Tui::new();
    tui.notice(format!(
        "govinda-cli v{} — {} mode · type /help for keys",
        env!("CARGO_PKG_VERSION"),
        tui.mode.label()
    ));

    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));

    'outer: while !tui.quit {
        // ── Idle phase: draw, react to keys, wait for a submission. ──
        draw_frame(terminal, &status_info(app, &tui), &tui)?;
        let prompt = loop {
            tokio::select! {
                ev = events.next() => match ev {
                    Some(Ok(e)) => tui.handle_event(e),
                    Some(Err(e)) => return Err(anyhow::anyhow!("input error: {e}")),
                    None => break 'outer,
                },
                _ = tick.tick() => {}
            }
            if tui.take_clear_request() {
                app.session.clear();
                tui.entries.clear();
                tui.plan_entry_idx = None;
                tui.notice("conversation cleared.");
            }
            // A queued /plan task is generated once, then gated on y/N.
            if let Some(task) = tui.take_plan_request() {
                match generate_plan(app, &task).await {
                    Ok(steps) if steps.is_empty() => {
                        tui.notice("the model returned no parseable steps — try rephrasing.");
                    }
                    Ok(steps) => {
                        // The todo list doubles as the plan's tracker (REPL parity).
                        commands::set_todos(app, &steps);
                        tui.start_plan(&task, steps);
                    }
                    Err(e) => tui.notice(format!("plan generation failed: {e:#}")),
                }
                continue;
            }
            if tui.quit {
                break 'outer;
            }
            if let Some(p) = tui.take_submission() {
                break p;
            }
            draw_frame(terminal, &status_info(app, &tui), &tui)?;
        };
        // ── Busy phase: run the turn, keep reacting to keys. ──
        // Snapshot before the turn takes `&mut App`.
        let info = status_info(app, &tui);
        let pinned = tui.pinned_files.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnUpdate>();
        let (confirm_tx, confirm_rx) =
            tokio::sync::mpsc::unbounded_channel::<ConfirmDecision>();
        tui.confirm_tx = Some(confirm_tx);
        tui.busy = true;
        tui.scroll_from_bottom = 0;

        // Everything owning the `&mut App` borrow lives in this block; the
        // boxed turn future is guaranteed dropped before we touch session or
        // todos again.
        enum Exit {
            TurnDone,
            Quit,
            Eof,
        }
        #[allow(clippy::expect_used)] // sound: the clearing arm also breaks
        let exit = {
            // Option lets us release the borrow the moment the turn resolves.
            let mut turn = Some(Box::pin(run_turn(
                app,
                tui.streaming.clone(),
                tui.cancel.clone(),
                prompt,
                pinned,
                tx,
                confirm_rx,
            )));

            loop {
                if draw_frame(terminal, &info, &tui).is_err() {
                    break Exit::Quit;
                }
                tokio::select! {
                    biased;

                    upd = rx.recv() => {
                        if let Some(u) = upd {
                            tui.apply_update(u);
                        }
                    }
                    ev = events.next() => match ev {
                        Some(Ok(e)) => tui.handle_event(e),
                        Some(Err(e)) => return Err(anyhow::anyhow!("input error: {e}")),
                        None => break Exit::Eof,
                    },
                    _ = tick.tick() => {}

                    // Drives the stream forward.
                    interrupted = turn.as_mut().expect("turn alive while polling").as_mut() => {
                        drop(turn.take());
                        tui.finish_turn(interrupted);
                        break Exit::TurnDone;
                    }
                }
                if tui.quit {
                    // Dropping `turn` cancels the stream mid-flight.
                    break Exit::Quit;
                }
            }
        };

        tui.confirm_tx = None;
        // Drain anything the runner sent right before finishing.
        while let Ok(u) = rx.try_recv() {
            tui.apply_update(u);
        }
        match exit {
            Exit::TurnDone => {}
            Exit::Quit => break 'outer,
            Exit::Eof => break 'outer,
        }

        // Sync the finished plan step into the /todo tracker.
        if let Some(step_idx) = tui.advance_plan() {
            if let Some(todo) = app.todos.get_mut(step_idx) {
                todo.done = true;
            }
            commands::persist_todos(app);
        }
        draw_frame(terminal, &status_info(app, &tui), &tui)?;
    }
    Ok(())
}

fn git_branch_and_dirty() -> (Option<String>, bool) {
    // Cheap, sync read of .git/HEAD; no process spawn per frame.
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let head = cwd.join(".git").join("HEAD");
    let dirty = false; // keep cheap; file-tree handles precise git marks
    let branch = std::fs::read_to_string(&head).ok().and_then(|s| {
        let t = s.trim();
        if let Some(rest) = t.strip_prefix("ref: refs/heads/") {
            Some(rest.to_owned())
        } else if t.len() >= 7 {
            // detached HEAD — show short sha
            Some(format!("⨂ {}", &t[..7]))
        } else {
            None
        }
    });
    (branch, dirty)
}

fn status_info(app: &App, tui: &Tui) -> draw::StatusInfo {
    let (git_branch, git_dirty) = git_branch_and_dirty();
    draw::StatusInfo {
        provider: app.config.provider.id().to_owned(),
        model: app.config.model.to_string(),
        tokens: app.session.approx_tokens(),
        budget: app.config.context_tokens,
        turns: app.stats.turns,
        errors: app.stats.errors,
        avg_latency_ms: if app.stats.turns > 0 {
            (app.stats.total_latency_ms / u128::from(app.stats.turns)) as u64
        } else {
            0
        },
        tools: app
            .tool_specs
            .iter()
            .map(|t| draw::ToolRowInfo {
                name: t.name.clone(),
                enabled: !app.disabled_tools.contains(&t.name),
                gated: app
                    .tool_executor
                    .as_ref()
                    .is_some_and(|e| e.requires_confirmation(&t.name)),
            })
            .collect(),
        git_branch,
        git_dirty,
        pinned: tui.pinned_files.len(),
        focus: tui.focus,
        busy: tui.busy,
    }
}

/// One non-interactive model call that decomposes a task into steps.
/// Mirrors the REPL's `/plan` without any stdout output.
async fn generate_plan(app: &mut App, task: &str) -> Result<Vec<String>> {
    anyhow::ensure!(app.tools_enabled, "planning needs function calling — /tools on");
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let overview = crate::scan::scan(&cwd).await;
    const PLAN_SYSTEM: &str = "You are a planning assistant for a coding agent that can scan, \
read, edit, and verify code in the user's workspace. Decompose the given task into short, \
concrete, self-contained steps (at most 10), ordered so each builds on the last. Prefer steps \
that name specific files or commands. Reply ONLY with a markdown numbered list — one step per \
line, no prose before or after.";
    let ctx = [
        api::Message::system(PLAN_SYSTEM),
        api::Message::user(format!(
            "Task:\n{task}\n\nWorkspace overview:\n{overview}\n\nProduce the plan now."
        )),
    ];
    let auth = app.config.provider.auth();
    // Planning must not recurse into tool calls.
    let opts = ChatOptions::new(auth.token(), &app.config.model, app.config.temperature);
    let mut out = String::new();
    let mut no_calls = Vec::new();
    {
        let mut sink = StreamSink::new(&mut out, &mut no_calls);
        api::stream_chat(
            &app.http,
            app.config.provider.as_ref(),
            &opts,
            &ctx,
            &mut sink,
            |_| {},
        )
        .await?;
    }
    Ok(commands::parse_steps(&out))
}

fn draw_frame(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    info: &draw::StatusInfo,
    tui: &Tui,
) -> Result<()> {
    terminal.draw(|f| draw::draw(f, tui, info))?;
    Ok(())
}

// ─── Turn runner ─────────────────────────────────────────────────────────────

/// One full agent turn: stream → optional tool rounds → final answer.
///
/// Returns `true` when the turn was interrupted (Ctrl+C/Esc) so the caller
/// can salvage the partial stream. Session mutation happens here; UI
/// feedback flows through `tx`, and gated tools wait on `confirm_rx`.
async fn run_turn(
    app: &mut App,
    streaming: SharedStream,
    cancel: Arc<AtomicBool>,
    prompt: String,
    pinned: Vec<PathBuf>,
    tx: tokio::sync::mpsc::UnboundedSender<TurnUpdate>,
    mut confirm_rx: tokio::sync::mpsc::UnboundedReceiver<ConfirmDecision>,
) -> bool {
    let started = Instant::now();
    streaming.borrow_mut().clear();

    // Context injection mirrors the REPL (mentioned/relevant files) plus any
    // files pinned from the tree sidebar.
    let injection = {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut files = crate::context::relevant_files(&prompt, &cwd);
        for p in &pinned {
            if !files.contains(p) {
                files.push(p.clone());
            }
        }
        crate::context::build_injection(&files, &cwd)
    };

    app.session.push_user(prompt);

    for round in 1..=MAX_TOOL_ROUNDS {
        if cancel.load(Ordering::Relaxed) {
            abort_turn(app, &streaming, &tx).await;
            return true;
        }

        let history = app
            .session
            .window_with(app.config.context_tokens, injection.as_deref());
        let auth = app.config.provider.auth();
        let opts = ChatOptions {
            max_response_bytes: app.max_response_bytes,
            read_timeout: app.read_timeout,
            tools: if app.tools_enabled {
                app.tool_specs
                    .iter()
                    .filter(|t| !app.disabled_tools.contains(&t.name))
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            },
            ..ChatOptions::new(auth.token(), app.config.model.as_str(), app.config.temperature)
        };

        let mut sink_out = String::new();
        let mut tool_calls = Vec::new();
        let result = {
            let http = &app.http;
            let provider = app.config.provider.clone();
            let mut sink = StreamSink::new(&mut sink_out, &mut tool_calls);
            let stream_shared = streaming.clone();
            let streamed = api::stream_chat(
                http,
                provider.as_ref(),
                &opts,
                &history,
                &mut sink,
                move |delta| {
                    stream_shared.borrow_mut().push_str(delta);
                },
            );
            tokio::select! {
                r = streamed => r.map(|_| sink_out),
                _ = wait_for_cancel(cancel.clone()) => Err(anyhow::anyhow!("interrupted")),
            }
        };

        match result {
            Err(e) => {
                let interrupted = e.to_string() == "interrupted";
                app.record_error();
                salvage_failed_round(app, &streaming, &tx, interrupted, &e).await;
                return interrupted;
            }
            Ok(prose) if tool_calls.is_empty() || !app.tools_enabled => {
                app.record_turn(started.elapsed());
                let _ = tx.send(TurnUpdate::Answer(prose));
                return false;
            }
            Ok(prose) => {
                // Tool round: surface prose, then execute calls one by one.
                if !prose.trim().is_empty() {
                    let _ = tx.send(TurnUpdate::AssistantProse(prose.clone()));
                }
                let executor = app.tool_executor.clone();
                let mut results = Vec::with_capacity(tool_calls.len());
                let mut approve_all_remaining = false;
                for call in &tool_calls {
                    if cancel.load(Ordering::Relaxed) {
                        results.push((
                            call.id.clone(),
                            "error: turn cancelled before this tool ran".to_owned(),
                        ));
                        continue;
                    }
                    let _ = tx.send(TurnUpdate::ToolStart {
                        name: call.function.name.clone(),
                        args: call.function.arguments.clone(),
                    });
                    let gated = executor
                        .as_ref()
                        .is_some_and(|e| e.requires_confirmation(&call.function.name));

                    // Approval gate: workspace-mutating calls pause the turn
                    // until the user answers y/n/a in Review mode.
                    let approved = !gated || approve_all_remaining || {
                        let _ = tx.send(TurnUpdate::ConfirmNeeded {
                            name: call.function.name.clone(),
                            args: call.function.arguments.clone(),
                        });
                        match wait_for_decision(&mut confirm_rx, &cancel).await {
                            ConfirmDecision::Approve => true,
                            ConfirmDecision::ApproveAll => {
                                approve_all_remaining = true;
                                true
                            }
                            ConfirmDecision::Decline => false,
                        }
                    };

                    let outcome = if !approved {
                        Err(anyhow::anyhow!("declined"))
                    } else {
                        match executor.as_ref() {
                            Some(e) => {
                                e.execute(&call.function.name, &call.function.arguments)
                                    .await
                            }
                            None => Err(anyhow::anyhow!("no tool executor configured")),
                        }
                    };
                    let stored = match &outcome {
                        Ok(value) => truncate_chars(value, MAX_TOOL_RESULT_CHARS),
                        Err(e) if e.to_string() == "declined" => {
                            "error: user declined this operation — ask how to proceed before \
                             retrying"
                                .to_owned()
                        }
                        Err(_) => format!("error: tool '{}' failed", call.function.name),
                    };
                    let failed =
                        outcome.is_err() || outcome.as_deref().is_ok_and(result_signals_failure);
                    let _ = tx.send(TurnUpdate::ToolEnd {
                        name: call.function.name.clone(),
                        args: call.function.arguments.clone(),
                        ok: !failed,
                    });
                    results.push((call.id.clone(), stored));
                }
                app.session.commit_tool_round(&prose, &tool_calls, &results);
                streaming.borrow_mut().clear();
                if round == MAX_TOOL_ROUNDS {
                    app.record_turn(started.elapsed());
                    let _ = tx.send(TurnUpdate::Notice(format!(
                        "stopped after {MAX_TOOL_ROUNDS} tool rounds — ask again to continue."
                    )));
                    return false;
                }
            }
        }
    }
    app.record_turn(started.elapsed());
    false
}

/// Cancels a turn before any request went out: drops the trailing user
/// prompt and reports the interruption.
async fn abort_turn(
    app: &mut App,
    streaming: &SharedStream,
    tx: &tokio::sync::mpsc::UnboundedSender<TurnUpdate>,
) {
    streaming.borrow_mut().clear();
    pop_trailing_user(app);
    let _ = tx.send(TurnUpdate::Notice("turn cancelled.".into()));
}

/// Error policy mirroring the REPL: keep a partially generated answer
/// (marked interrupted), otherwise roll back the trailing user prompt.
async fn salvage_failed_round(
    app: &mut App,
    streaming: &SharedStream,
    tx: &tokio::sync::mpsc::UnboundedSender<TurnUpdate>,
    interrupted: bool,
    e: &anyhow::Error,
) {
    let partial = std::mem::take(&mut *streaming.borrow_mut());
    if !partial.trim().is_empty() {
        let kept = format!("{partial}\n\n*(interrupted)*");
        app.session.push_assistant(kept.clone());
        let _ = tx.send(TurnUpdate::Answer(kept));
    } else {
        pop_trailing_user(app);
    }
    if interrupted {
        let _ = tx.send(TurnUpdate::Notice("turn cancelled.".into()));
    } else {
        let _ = tx.send(TurnUpdate::Error(format!("{e:#}")));
    }
}

/// Removes the trailing user prompt when no assistant content survived —
/// otherwise history would start with an orphan question.
fn pop_trailing_user(app: &mut App) {
    if app
        .session
        .messages()
        .last()
        .is_some_and(|m| m.role == "user")
    {
        app.session.pop_user();
    }
}

/// Polls the cancel flag so a streaming request can be abandoned promptly.
async fn wait_for_cancel(cancel: Arc<AtomicBool>) {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Waits for the user's y/n/a answer; a cancelled turn counts as Decline.
async fn wait_for_decision(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<ConfirmDecision>,
    cancel: &Arc<AtomicBool>,
) -> ConfirmDecision {
    tokio::select! {
        biased;
        decision = rx.recv() => {
            // Channel closed → caller went away; decline is safest.
            decision.unwrap_or(ConfirmDecision::Decline)
        }
        _ = wait_for_cancel(cancel.clone()) => ConfirmDecision::Decline,
    }
}

/// Failure heuristic mirroring the REPL: sanitized `error:` prefixes and
/// structured payloads with a non-zero exit code.
fn result_signals_failure(value: &str) -> bool {
    if value.starts_with("error:") {
        return true;
    }
    serde_json::from_str::<serde_json::Value>(value)
        .ok()
        .and_then(|v| v.get("exit_code").and_then(serde_json::Value::as_i64))
        .is_some_and(|code| code != 0)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_owned()
    } else {
        let cut: String = s.chars().take(max_chars).collect();
        format!("{cut}\n…(truncated)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_transitions_follow_spec() {
        let mut m = AppMode::Normal;
        assert!(m.transition_to(AppMode::Agent));
        assert_eq!(m, AppMode::Agent);
        // Review/Plan are Phase C states; not reachable yet.
        assert!(!m.transition_to(AppMode::Review));
        assert!(!m.transition_to(AppMode::Plan));
        assert!(m.transition_to(AppMode::Normal));
        assert_eq!(m, AppMode::Normal);
    }

    #[test]
    fn slash_commands_are_handled_locally() {
        let mut tui = Tui::new();
        tui.set_input("/help".into());
        tui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(tui.entries.iter().any(|e| matches!(e, ChatEntry::Notice(_))));
        assert!(tui.take_submission().is_none());

        tui.set_input("/quit".into());
        tui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(tui.quit);

        tui.set_input("/clear".into());
        tui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(tui.take_clear_request());

        tui.set_input("/bogus".into());
        tui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(tui.take_submission().is_none());
        assert!(tui
            .entries
            .iter()
            .any(|e| matches!(e, ChatEntry::Notice(t) if t.contains("bogus"))));
    }

    #[test]
    fn prompts_queue_and_record_history() {
        let mut tui = Tui::new();
        tui.set_input("fix the bug".into());
        tui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(tui.take_submission().as_deref(), Some("fix the bug"));
        // History round-trip.
        tui.history_prev();
        assert_eq!(tui.input, "fix the bug");
        tui.history_next();
        assert_eq!(tui.input, "");
    }

    #[test]
    fn input_editing_respects_multibyte_characters() {
        let mut tui = Tui::new();
        for c in ['h', 'é', '日'] {
            tui.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(tui.input, "hé日");
        tui.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        tui.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(tui.input, "h日");
        // Cursor sits after the surviving 'h'.
        assert_eq!(tui.input_cursor, 1);
    }

    #[test]
    fn ctrl_c_cancels_only_when_busy() {
        let mut tui = Tui::new();
        tui.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!tui.cancel.load(Ordering::Relaxed));
        tui.busy = true;
        tui.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(tui.cancel.load(Ordering::Relaxed));
    }

    #[test]
    fn chat_scrolling_moves_both_ways() {
        let mut tui = Tui::new();
        tui.focus = Focus::Chat;
        tui.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(tui.scroll_from_bottom, 10);
        tui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(tui.scroll_from_bottom, 9);
        tui.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(tui.scroll_from_bottom, 0);
    }

    #[test]
    fn failure_heuristic_matches_repl_behavior() {
        assert!(result_signals_failure("error: declined"));
        assert!(result_signals_failure(r#"{"exit_code":101}"#));
        assert!(!result_signals_failure(r#"{"exit_code":0}"#));
        assert!(!result_signals_failure("plain output"));
    }

    #[test]
    fn tool_updates_pair_start_and_end() {
        let mut tui = Tui::new();
        tui.apply_update(TurnUpdate::ToolStart {
            name: "read_file".into(),
            args: "{}".into(),
        });
        tui.apply_update(TurnUpdate::ToolEnd {
            name: "read_file".into(),
            args: "{}".into(),
            ok: true,
        });
        assert!(matches!(
            tui.entries.last(),
            Some(ChatEntry::Tool { ok: Some(true), .. })
        ));
    }

    #[test]
    fn confirm_needed_enters_review_mode_and_y_resolves() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tui = Tui::new();
        tui.confirm_tx = Some(tx);
        tui.mode = AppMode::Agent;

        tui.apply_update(TurnUpdate::ConfirmNeeded {
            name: "write_file".into(),
            args: r#"{"path":"a.txt"}"#.into(),
        });
        assert_eq!(tui.mode, AppMode::Review);
        assert!(tui.confirm_pending);

        // While pending, keys route to the gate, not the input.
        tui.set_input("should not change".into());
        tui.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert_eq!(rx.try_recv().unwrap(), ConfirmDecision::Approve);
        assert_eq!(tui.mode, AppMode::Agent); // restored
        assert!(!tui.confirm_pending);
        assert_eq!(tui.input, "should not change");

        // Decline path restores too — a fresh TUI starting in Normal.
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
        let mut tui2 = Tui::new();
        tui2.confirm_tx = Some(tx2);
        tui2.apply_update(TurnUpdate::ConfirmNeeded {
            name: "run_shell".into(),
            args: "{}".into(),
        });
        assert_eq!(tui2.mode, AppMode::Review);
        tui2.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(rx2.try_recv().unwrap(), ConfirmDecision::Decline);
        assert_eq!(tui2.mode, AppMode::Normal);
    }

    #[test]
    fn approve_all_answers_a_for_every_remaining_gate() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tui = Tui::new();
        tui.confirm_tx = Some(tx);
        for _ in 0..3 {
            tui.apply_update(TurnUpdate::ConfirmNeeded {
                name: "write_file".into(),
                args: "{}".into(),
            });
            tui.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        }
        assert_eq!(
            std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>(),
            vec![ConfirmDecision::ApproveAll; 3]
        );
    }

    #[test]
    fn plan_lifecycle_awaits_confirms_executes_completes() {
        let mut tui = Tui::new();
        tui.start_plan("refactor", vec!["step one".into(), "step two".into()]);
        assert!(tui.plan_awaiting);
        assert!(tui.entries.iter().any(|e| matches!(
            e,
            ChatEntry::Checklist { steps, .. } if steps.len() == 2
        )));

        // Abort path.
        tui.cancel_plan();
        assert!(!tui.plan_awaiting && !tui.plan_in_flight());

        // Restart and drive through both steps.
        tui.start_plan("refactor", vec!["step one".into(), "step two".into()]);
        tui.approve_plan();
        assert!(tui.plan_in_flight());
        assert_eq!(
            tui.take_submission().as_deref(),
            Some("[plan step 1/2] step one")
        );
        // First turn finished → step 1 done, step 2 queued.
        assert_eq!(tui.advance_plan(), Some(0));
        assert_eq!(
            tui.take_submission().as_deref(),
            Some("[plan step 2/2] step two")
        );
        // Second turn finished → plan complete.
        assert_eq!(tui.advance_plan(), Some(1));
        assert!(!tui.plan_in_flight());
        // Nothing more to advance once done.
        assert_eq!(tui.advance_plan(), None);
    }

    #[test]
    fn pin_file_deduplicates() {
        let mut tui = Tui::new();
        let p = PathBuf::from("src/main.rs");
        tui.pin_file(p.clone());
        let notices_after_first = tui.entries.len();
        tui.pin_file(p);
        assert_eq!(tui.pinned_files.len(), 1);
        assert!(tui.entries.len() > notices_after_first, "duplicate notice shown");
    }
}
