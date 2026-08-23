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
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use super::widgets::chat_pane::ChatEntry;
use super::widgets::file_tree::FileTree;
use super::{draw, icons, theme};
use crate::api::{self, ChatOptions, StreamSink};
use crate::commands::{self, App};

/// Upper bound on model↔tool round trips per user turn (mirrors the REPL).
const MAX_TOOL_ROUNDS: usize = 5;
/// Cap applied *before* a tool result enters session history.
const MAX_TOOL_RESULT_CHARS: usize = 8 * 1024;

/// Slash commands that never take an argument — Enter on the palette runs
/// them directly instead of opening the args dialog.
const ZERO_ARG_SLASH: [&str; 21] = [
    "/help", "/exit", "/quit", "/q", "/clear", "/reset", "/sessions", "/stats", "/history",
    "/models", "/tools", "/config", "/tokens", "/undo", "/retry", "/compact", "/raw", "/scan",
    "/pin", "/variants", "/pick",
];

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

#[derive(Debug, Clone)]
pub struct SlashDialog {
    pub command: String,
    pub desc: String,
    pub arg_input: String,
    pub arg_cursor: usize,
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
    /// Slash palette selection (when input starts with '/')
    pub slash_selected: usize,
    /// Palette row under the mouse (hover sheen), absolute hit index
    pub palette_hover: Option<usize>,
    /// File-tree row under the mouse (hover sheen), absolute flat index
    pub tree_hover: Option<usize>,
    /// Queued slash for full dispatch needing App (all 37 commands)
    pending_slash: Option<String>,
    /// Dialog shown after clicking a slash command (mouse or Tab) — args input
    pub slash_dialog: Option<SlashDialog>,
}

impl Default for Tui {
    fn default() -> Self {
        Self::new()
    }
}

impl Tui {
    pub fn new() -> Self {
        let mut tui = Self {
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
            slash_selected: 0,
            palette_hover: None,
            tree_hover: None,
            pending_slash: None,
            slash_dialog: None,
        };
        // Eagerly open explorer so "No files yet" never shows on startup when
        // the right pane is visible by default (width≥100). Fail silently in tests.
        if tui.show_tools {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            // Only auto-open if cwd looks like a real project (avoid temp test dirs pollution)
            // but FileTree::open is cheap even for temp dirs, so just open.
            tui.tree = Some(FileTree::open(&cwd));
        }
        tui
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
    pub fn take_pending_slash(&mut self) -> Option<String> {
        self.pending_slash.take()
    }

    fn submit(&mut self) {
        let line = self.input.trim().to_owned();
        if line.is_empty() {
            return;
        }
        self.input.clear();
        self.input_cursor = 0;
        self.slash_selected = 0;

        if line.starts_with('/') {
            // Try local fast path; if not handled, queue for App-aware dispatch
            if !self.local_command(&line) {
                self.pending_slash = Some(line);
            }
            return;
        }
        self.history.push(line.clone());
        self.history_idx = None;
        self.draft.clear();
        self.entries.push(ChatEntry::User(line.clone()));
        self.scroll_from_bottom = 0;
        self.pending_prompt = Some(line);
    }

    fn apply_slash_completion(&mut self) {
        let input = self.input.clone();
        let hits = crate::tui::widgets::input_bar::filtered(&input);
        if hits.is_empty() {
            return;
        }
        let idx = self.slash_selected.min(hits.len() - 1);
        let chosen = hits[idx];
        // replace first token with chosen
        if let Some(space) = input.find(char::is_whitespace) {
            let rest = &input[space..];
            self.input = format!("{chosen}{rest}");
        } else {
            self.input = chosen.to_owned();
        }
        self.input_cursor = self.input.chars().count();
        self.slash_selected = 0;
    }

    pub fn open_slash_dialog(&mut self, cmd: &str) {
        let desc = crate::tui::widgets::input_bar::describe(cmd);
        // Clear the palette input so the dropdown hides behind the dialog.
        self.input.clear();
        self.input_cursor = 0;
        self.slash_selected = 0;
        self.slash_dialog = Some(SlashDialog {
            command: cmd.to_owned(),
            desc: desc.to_owned(),
            arg_input: String::new(),
            arg_cursor: 0,
        });
    }

    fn close_slash_dialog(&mut self) {
        self.slash_dialog = None;
    }

    /// Confirms the dialog: queues `command [args]` for App-aware dispatch.
    fn confirm_slash_dialog(&mut self) {
        if let Some(d) = self.slash_dialog.take() {
            let full = if d.arg_input.trim().is_empty() {
                d.command.clone()
            } else {
                format!("{} {}", d.command, d.arg_input.trim())
            };
            self.pending_slash = Some(full);
            self.input.clear();
            self.input_cursor = 0;
            self.slash_selected = 0;
        }
    }

    fn handle_dialog_key(&mut self, key: KeyEvent) -> bool {
        if self.slash_dialog.is_none() {
            return false;
        }
        match key.code {
            KeyCode::Esc => {
                self.close_slash_dialog();
            }
            KeyCode::Enter => {
                self.confirm_slash_dialog();
            }
            KeyCode::Char(c) => {
                if let Some(dialog) = &mut self.slash_dialog {
                    let byte = dialog
                        .arg_input
                        .char_indices()
                        .nth(dialog.arg_cursor)
                        .map_or(dialog.arg_input.len(), |(i, _)| i);
                    dialog.arg_input.insert(byte, c);
                    dialog.arg_cursor += 1;
                }
            }
            KeyCode::Backspace => {
                if let Some(dialog) = &mut self.slash_dialog {
                    if dialog.arg_cursor > 0 {
                        let byte = dialog
                            .arg_input
                            .char_indices()
                            .nth(dialog.arg_cursor)
                            .map_or(dialog.arg_input.len(), |(i, _)| i);
                        let prev = dialog.arg_input[..byte]
                            .char_indices()
                            .next_back()
                            .map_or(0, |(i, _)| i);
                        dialog.arg_input.replace_range(prev..byte, "");
                        dialog.arg_cursor -= 1;
                    } else {
                        return false;
                    }
                }
            }
            KeyCode::Delete => {
                if let Some(dialog) = &mut self.slash_dialog {
                    if dialog.arg_cursor < dialog.arg_input.chars().count() {
                        let byte = dialog
                            .arg_input
                            .char_indices()
                            .nth(dialog.arg_cursor)
                            .map_or(dialog.arg_input.len(), |(i, _)| i);
                        let next = dialog.arg_input[byte..]
                            .char_indices()
                            .nth(1)
                            .map_or(dialog.arg_input.len(), |(i, _)| byte + i);
                        dialog.arg_input.replace_range(byte..next, "");
                    } else {
                        return false;
                    }
                }
            }
            KeyCode::Left => {
                if let Some(dialog) = &mut self.slash_dialog {
                    if dialog.arg_cursor > 0 { dialog.arg_cursor -= 1; } else { return false; }
                }
            }
            KeyCode::Right => {
                if let Some(dialog) = &mut self.slash_dialog {
                    if dialog.arg_cursor < dialog.arg_input.chars().count() { dialog.arg_cursor += 1; } else { return false; }
                }
            }
            KeyCode::Home => {
                if let Some(dialog) = &mut self.slash_dialog { dialog.arg_cursor = 0; }
            }
            KeyCode::End => {
                if let Some(dialog) = &mut self.slash_dialog { dialog.arg_cursor = dialog.arg_input.chars().count(); }
            }
            _ => return false,
        }
        true
    }

    fn local_command(&mut self, line: &str) -> bool {
        let (cmd, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let cmd_lc = cmd.to_ascii_lowercase();
        match cmd_lc.as_str() {
            "/quit" | "/exit" | "/q" => {
                self.quit = true;
                return true;
            }
            "/clear" | "/reset" => {
                self.pending_clear = true;
                return true;
            }
            "/help" => {
                self.notice(
                    "keys: Tab focus · ↑/↓ palette/history · Space expand dir · Enter \
                     open/pin file · F5 refresh · Esc clear/cancel · Ctrl+C cancel stream \
                     · Ctrl+L clear · Ctrl+T left tree · Ctrl+P explorer · Ctrl+Q quit\n\
                     gated calls pause in [REVIEW]: y approve · n decline · a all\n\
                     cmds: /help /clear /theme /tokens /agent <on|off> /plan <task> /model /temp /system /history /undo /retry /variants /pick /compact /search /save /load /sessions /fork /export /stats /raw /config /timeout /limit /tools /todo /diff /apply /reject /review /scan /project\n\
                      Tip: type \"/\" to see the palette — Enter/↑↓/click open the args dialog, Tab completes.",
                );
                return true;
            }
            "/theme" => {
                // "/theme" alone handled locally; "/theme <name>" needs App for persistence check but toggle is fine
                if rest.trim().is_empty() {
                    let t = theme::toggle();
                    self.notice(format!("switched to the {} theme", t.name()));
                    return true;
                }
                // fall through to App-aware dispatch for "/theme <name>"
                return false;
            }
            "/tokens" => {
                self.notice("tokens — see top bar for live usage ( /tokens for full BPE count )");
                return true;
            }
            "/plan" => {
                let task = rest.trim();
                if task.is_empty() {
                    self.notice("usage: /plan <task> — decompose a task and execute step by step");
                } else {
                    self.pending_plan_task = Some(task.to_owned());
                    self.notice("planning…");
                }
                return true;
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
                return true;
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
                return true;
            }
            _ => {}
        }
        // If it's a known slash command, queue for App-aware handling
        if crate::commands::SLASH_COMMANDS.contains(&cmd_lc.as_str()) {
            return false;
        }
        // Unknown command
        self.notice(format!(
            "unknown command '{}' — type \"/\" to see palette or /help",
            cmd
        ));
        true
    }

    pub     fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Key(key) if key.kind == crossterm::event::KeyEventKind::Press => {
                self.handle_key(key);
            }
            Event::Mouse(me) => {
                // Mouse is handled in event_loop with layout context; fallback scroll-only here
                self.handle_mouse_fallback(me);
            }
            _ => {}
        }
    }

    fn handle_mouse_fallback(&mut self, me: MouseEvent) {
        match me.kind {
            MouseEventKind::ScrollUp => {
                if self.focus == Focus::Tree {
                    if let Some(tree) = &mut self.tree {
                        tree.move_selection(-1);
                    }
                } else if self.focus == Focus::Chat {
                    self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(1);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.focus == Focus::Tree {
                    if let Some(tree) = &mut self.tree {
                        tree.move_selection(1);
                    }
                } else if self.focus == Focus::Chat {
                    self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(1);
                }
            }
            _ => {}
        }
    }

    /// Mouse with layout — called from event_loop where pane rects are known.
    pub fn handle_mouse_with_layout(&mut self, me: MouseEvent, layout: &super::layout::PaneLayout) {
        let col = me.column;
        let row = me.row;
        // helper to test inside rect
        let inside = |r: ratatui::layout::Rect| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height;
        // Dialog captures mouse first
        if let Some(_dialog) = &self.slash_dialog {
            // centered 60x9 dialog — compute same as draw
            let full_w = layout.status.width;
            let full_h = layout.status.height + layout.chat.height + layout.input.height;
            let dw: u16 = 60;
            let dh: u16 = 9;
            let dx = full_w.saturating_sub(dw) / 2;
            let dy = full_h.saturating_sub(dh) / 2;
            let dlg = ratatui::layout::Rect::new(dx, dy, dw.min(full_w), dh.min(full_h));
            match me.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if inside(dlg) {
                        // click inside dialog — focus stays, maybe set cursor? For now do nothing.
                        return;
                    } else {
                        // click outside closes dialog (cancel)
                        self.close_slash_dialog();
                        return;
                    }
                }
                _ => return,
            }
        }
        // Palette click — open dialog for selected command
        if let MouseEventKind::Moved = me.kind {
            // default: outside any hoverable region
            self.palette_hover = None;
        }
        if let MouseEventKind::Moved = me.kind {
            // file-tree hover sheen (left tree or right explorer)
            let pane = layout
                .tree
                .filter(|r| inside(*r))
                .or_else(|| layout.tools.filter(|r| inside(*r)));
            self.tree_hover = pane.and_then(|r| {
                let Some(tree) = &self.tree else { return None };
                if r.width < 3 || r.height < 3 {
                    return None;
                }
                let inner_y = r.y + 1;
                let off = row.saturating_sub(inner_y) as usize;
                let view_h = (r.height.saturating_sub(2)) as usize;
                let len = tree.flat_len();
                if len == 0 {
                    return None;
                }
                let selected = tree.selected_index();
                let start = selected
                    .saturating_sub(view_h.saturating_sub(1))
                    .min(len.saturating_sub(view_h.min(len)));
                let idx = start + off;
                (idx < len).then_some(idx)
            });
        }
        if self.focus == Focus::Input && self.input.starts_with('/') && !self.input.contains(' ') {
            let hits = crate::tui::widgets::input_bar::filtered(&self.input);
            if !hits.is_empty() {
                let lines = crate::tui::widgets::input_bar::palette_lines(&self.input, self.slash_selected, self.palette_hover);
                let pal_h = (lines.len() as u16 + 2).min(18);
                let pal_w = layout.input.width.saturating_sub(2).min(56);
                let pal_x = layout.input.x;
                let pal_y = layout.input.y.saturating_sub(pal_h);
                let pal_rect = ratatui::layout::Rect::new(pal_x, pal_y.max(1), pal_w, pal_h);
                if inside(pal_rect) {
                    // Hover sheen: track which palette row the mouse is over.
                    if let MouseEventKind::Moved = me.kind {
                        let inner_y = pal_rect.y + 1;
                        let off = row.saturating_sub(inner_y) as usize;
                        let total = hits.len();
                        let max_show = 12.min(total);
                        let sel = self.slash_selected.min(total.saturating_sub(1));
                        let start = sel.saturating_sub(max_show / 2).min(total.saturating_sub(max_show));
                        self.palette_hover = if total > max_show && start > 0 {
                            (off >= 2 && off < 2 + max_show).then(|| start + (off - 2))
                        } else {
                            (off >= 1 && off < 1 + max_show).then(|| start + (off - 1))
                        };
                        return;
                    }
                    match me.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            // map click to row
                            let inner_y = pal_rect.y + 1;
                            let off = row.saturating_sub(inner_y) as usize;
                            // header is line 0, maybe "↑" line 1, then commands, then "↓"
                            let total = hits.len();
                            let max_show = 12.min(total);
                            let sel = self.slash_selected.min(total.saturating_sub(1));
                            let start = sel.saturating_sub(max_show / 2).min(total.saturating_sub(max_show));
                            let end = (start + max_show).min(total);
                            // determine which row was clicked
                            let mut cmd_idx: Option<usize> = None;
                            if total > max_show && start > 0 {
                                // line 1 is "↑"
                                if off == 1 {
                                    // clicked "↑" — page selection up
                                    self.slash_selected = sel.saturating_sub(max_show);
                                    return;
                                } else if off >= 2 && off < 2 + max_show {
                                    cmd_idx = Some(start + (off - 2));
                                } else if off == 2 + max_show && end < total {
                                    // clicked "↓" — page selection down
                                    self.slash_selected = (sel + max_show).min(total - 1);
                                    return;
                                }
                            } else {
                                if off == 1 + max_show && end < total {
                                    // clicked "↓" — page selection down
                                    self.slash_selected = (sel + max_show).min(total - 1);
                                    return;
                                }
                                if off >= 1 && off < 1 + max_show {
                                    cmd_idx = Some(start + (off - 1));
                                }
                            }
                            if let Some(idx) = cmd_idx {
                                if idx < total {
                                    self.slash_selected = idx;
                                    let cmd = hits[idx];
                                    self.open_slash_dialog(cmd);
                                    return;
                                }
                            } else if off == 0 {
                                // header click — ignore
                                return;
                            }
                            // click on palette but not on command — just keep
                            return;
                        }
                        MouseEventKind::ScrollUp => {
                            if self.slash_selected > 0 { self.slash_selected -= 1; }
                            return;
                        }
                        MouseEventKind::ScrollDown => {
                            if self.slash_selected + 1 < hits.len() { self.slash_selected += 1; }
                            return;
                        }
                        _ => {}
                    }
                }
            }
        }
        match me.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(r) = layout.tree
                    && inside(r)
                {
                    self.focus = Focus::Tree;
                    self.show_tree = true;
                    self.ensure_tree();
                    self.click_tree_at(col, row, r);
                    return;
                }
                if let Some(r) = layout.tools
                    && inside(r)
                {
                    self.focus = Focus::Tree;
                    self.show_tools = true;
                    self.ensure_tree();
                    self.click_tree_at(col, row, r);
                    return;
                }
                if inside(layout.chat) {
                    self.focus = Focus::Chat;
                    return;
                }
                if inside(layout.input) {
                    self.focus = Focus::Input;
                    return;
                }
                if inside(layout.status) {
                    // click top bar cycles theme
                    // not handling here; could toggle
                }
            }
            MouseEventKind::ScrollUp => {
                if let Some(r) = layout.tree
                    && inside(r)
                {
                    if let Some(tree) = &mut self.tree { tree.move_selection(-3); }
                } else if let Some(r) = layout.tools
                    && inside(r)
                {
                    if let Some(tree) = &mut self.tree { tree.move_selection(-3); }
                } else if inside(layout.chat) {
                    self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(3);
                } else {
                    self.handle_mouse_fallback(me);
                }
            }
            MouseEventKind::ScrollDown => {
                if let Some(r) = layout.tree
                    && inside(r)
                {
                    if let Some(tree) = &mut self.tree { tree.move_selection(3); }
                } else if let Some(r) = layout.tools
                    && inside(r)
                {
                    if let Some(tree) = &mut self.tree { tree.move_selection(3); }
                } else if inside(layout.chat) {
                    self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(3);
                } else {
                    self.handle_mouse_fallback(me);
                }
            }
            _ => {}
        }
    }

    fn click_tree_at(&mut self, _col: u16, row: u16, pane: ratatui::layout::Rect) {
        let Some(tree) = &mut self.tree else { return };
        // inner area (border 1)
        if pane.width < 3 || pane.height < 3 { return; }
        let inner_y = pane.y + 1;
        let clicked_offset = row.saturating_sub(inner_y) as usize;
        let view_h = (pane.height.saturating_sub(2)) as usize;
        // compute window start as render does
        let len = tree.flat_len();
        if len == 0 { return; }
        let selected = tree.selected_index();
        let start = selected.saturating_sub(view_h.saturating_sub(1)).min(len.saturating_sub(view_h.min(len)));
        let idx = start + clicked_offset;
        if idx >= len { return; }
        tree.set_selected(idx);
        // single click opens: dir toggles, file pins
        if let Some(node) = tree.selected_node() {
            if node.is_dir {
                tree.toggle_selected();
            } else if let Some(path) = tree.activate_selected() {
                self.pin_file(path);
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // Dialog captures all keys (Enter confirms, Esc cancels, typing edits arg)
        if self.slash_dialog.is_some() {
            // allow Ctrl+Q to quit even with dialog
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
            {
                self.quit = true;
                return;
            }
            self.handle_dialog_key(key);
            return;
        }
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

        // Global bindings. Handle case-insensitive for Ctrl.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if let KeyCode::Char(c) = key.code {
                match c.to_ascii_lowercase() {
                    'q' => {
                        self.quit = true;
                        return;
                    }
                    'c' => {
                        if self.busy {
                            self.cancel.store(true, Ordering::Relaxed);
                        } else {
                            self.input.clear();
                            self.input_cursor = 0;
                        }
                        return;
                    }
                    'l' => {
                        self.pending_clear = true;
                        return;
                    }
                    't' => {
                        self.toggle_tree();
                        return;
                    }
                    'p' => {
                        self.toggle_explorer();
                        return;
                    }
                    _ => {}
                }
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

        // Slash palette: Tab completes, Up/Down navigates filtered list
        if self.focus == Focus::Input && self.input.starts_with('/') {
            // palette is shown only while first token has no space
            if !self.input.contains(' ') {
                let hits = crate::tui::widgets::input_bar::filtered(&self.input);
                if !hits.is_empty() {
                    match key.code {
                        KeyCode::Tab | KeyCode::Right => {
                            self.apply_slash_completion();
                            return;
                        }
                        KeyCode::Up => {
                            if self.slash_selected == 0 {
                                self.slash_selected = hits.len() - 1;
                            } else {
                                self.slash_selected -= 1;
                            }
                            return;
                        }
                        KeyCode::Down => {
                            self.slash_selected = (self.slash_selected + 1) % hits.len();
                            return;
                        }
                        KeyCode::Enter => {
                            // Commands that take no argument run straight away;
                            // everything else opens the args dialog so required
                            // arguments are collected first (same as clicking a
                            // palette row). Running e.g. "/save" bare would only
                            // yield a usage notice.
                            let cmd = hits[self.slash_selected.min(hits.len() - 1)];
                            if ZERO_ARG_SLASH.contains(&cmd) {
                                // fall through to submit() below
                            } else {
                                self.open_slash_dialog(cmd);
                                return;
                            }
                        }
                        KeyCode::Esc => {
                            self.input.clear();
                            self.input_cursor = 0;
                            self.slash_selected = 0;
                            return;
                        }
                        _ => {}
                    }
                }
            }
        }

        match key.code {
            KeyCode::Tab => {
                // If slash palette is active, Tab already handled above; otherwise switch focus
                if self.focus == Focus::Input && self.input.starts_with('/') {
                    // fallback: complete first ghost
                    if let Some(ghost) = crate::tui::widgets::input_bar::completion(&self.input) {
                        self.input.push_str(&ghost);
                        self.input_cursor = self.input.chars().count();
                        self.slash_selected = 0;
                        return;
                    }
                }
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
                    "{} {} pinned file(s) still ride along in every turn",
                    icons::PINNED,
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
        self.notice(format!("{} pinned {rel} to context", crate::tui::icons::PINNED));
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
                self.slash_selected = 0;
            }
            KeyCode::Backspace if self.input_cursor > 0 => {
                let byte = self.cursor_byte();
                let prev = self.input[..byte]
                    .char_indices()
                    .next_back()
                    .map_or(0, |(i, _)| i);
                self.input.replace_range(prev..byte, "");
                self.input_cursor -= 1;
                self.slash_selected = 0;
            }
            KeyCode::Delete if self.input_cursor < self.input.chars().count() => {
                let byte = self.cursor_byte();
                let next = self.input[byte..]
                    .char_indices()
                    .nth(1)
                    .map_or(self.input.len(), |(i, _)| byte + i);
                self.input.replace_range(byte..next, "");
                self.slash_selected = 0;
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
                    "{} workspace change — [y] approve · [n] decline · [a] approve all remaining"
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
            self.notice(format!("{} plan complete.", icons::SUCCESS));
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
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = event_loop(app, &mut terminal).await;

    // Restore unconditionally — a leaked alternate screen ruins the shell.
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen
    );
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
                    Some(Ok(Event::Mouse(me))) => {
                        // layout-aware mouse: click to focus/open file tree, scroll
                        let area = terminal
                            .size()
                            .map(|r| ratatui::layout::Rect::new(0, 0, r.width, r.height))
                            .unwrap_or(ratatui::layout::Rect::new(0, 0, 80, 24));
                        let layout = crate::tui::layout::PaneLayout::compute(area, tui.show_tree, tui.show_tools);
                        tui.handle_mouse_with_layout(me, &layout);
                    }
                    Some(Ok(e)) => tui.handle_event(e),
                    Some(Err(e)) => return Err(anyhow::anyhow!("input error: {e}")),
                    None => break 'outer,
                },
                _ = tick.tick() => {
                    if let Some(tree) = tui.tree.as_mut() {
                        tree.maybe_auto_refresh();
                    }
                }
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
            if let Some(slash) = tui.take_pending_slash() {
                handle_tui_slash(app, &mut tui, &slash).await;
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
                        Some(Ok(Event::Mouse(me))) => {
                            let area = terminal
                                .size()
                                .map(|r| ratatui::layout::Rect::new(0, 0, r.width, r.height))
                                .unwrap_or(ratatui::layout::Rect::new(0, 0, 80, 24));
                            let layout = crate::tui::layout::PaneLayout::compute(area, tui.show_tree, tui.show_tools);
                            tui.handle_mouse_with_layout(me, &layout);
                        }
                        Some(Ok(e)) => tui.handle_event(e),
                        Some(Err(e)) => return Err(anyhow::anyhow!("input error: {e}")),
                        None => break Exit::Eof,
                    },
                    _ = tick.tick() => {
                        if let Some(tree) = tui.tree.as_mut() {
                            tree.maybe_auto_refresh();
                        }
                    }

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

async fn handle_tui_slash(app: &mut App, tui: &mut Tui, line: &str) {
    let (cmd, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let lc = cmd.to_ascii_lowercase();
    match lc.as_str() {
        "/models" => {
            tui.notice(format!("provider {} — current model {}", app.config.provider.id(), app.config.model));
            tui.notice("tip: /model <name> to switch, /model next|prev to cycle");
        }
        "/model" => {
            let arg = rest.trim();
            if arg.is_empty() {
                tui.notice(format!("current model: {} ({})", app.config.model, app.config.provider.id()));
            } else if ["next", "prev", "n", "p"].contains(&arg.to_ascii_lowercase().as_str()) {
                tui.notice(format!("model cycling requires REPL — use /model <name> to set directly; current {}", app.config.model));
            } else {
                app.config.model = arg.to_owned();
                tui.notice(format!("model set to {}", app.config.model));
            }
        }
        "/temp" => {
            let arg = rest.trim();
            let v = arg.parse::<f32>().ok().filter(|x| (0.0..=1.0).contains(x));
            if let Some(val) = v {
                app.config.temperature = val;
                tui.notice(format!("temperature set to {val:.2}"));
            } else {
                tui.notice(format!("usage: /temp <0.0-1.0> (current {:.2})", app.config.temperature));
            }
        }
        "/system" => {
            let p = rest.trim();
            if p.is_empty() {
                tui.notice(format!("system: {}", app.session.system()));
            } else {
                app.session.set_system(p);
                tui.notice("system prompt updated (next turn)");
            }
        }
        "/history" => {
            let msgs = app.session.messages();
            if msgs.is_empty() {
                tui.notice("(empty history)");
            } else {
                for (i, m) in msgs.iter().enumerate() {
                    let prefix = if m.role == "user" { "you" } else { "govinda" };
                    let txt: String = m.content.chars().take(120).collect();
                    tui.notice(format!("[{i} {prefix}] {txt}"));
                }
            }
        }
        "/undo" => {
            if app.session.undo() {
                tui.notice("removed last exchange");
            } else {
                tui.notice("nothing to undo");
            }
        }
        "/retry" => {
            // find last user message
            if let Some(last) = app.session.messages().iter().rev().find(|m| m.role == "user").map(|m| m.content.clone()) {
                tui.notice("regenerating last prompt…");
                tui.pending_prompt = Some(last);
            } else {
                tui.notice("nothing to retry");
            }
        }
        "/variants" | "/pick" => tui.notice("variants/pick are REPL-only"),
        "/compact" => {
            tui.notice("compacting history…");
            // best-effort: keep system + last 2 exchanges
            let msgs = app.session.messages().to_vec();
            if msgs.len() > 4 {
                let keep = 3;
                let tail = msgs[msgs.len() - keep..].to_vec();
                app.session.clear();
                // re-add tail (simplified)
                for m in tail {
                    if m.role == "user" { app.session.push_user(m.content); }
                    else if m.role == "assistant" { app.session.push_assistant(m.content); }
                }
                tui.notice(format!("compacted to {} messages", app.session.messages().len()));
            } else {
                tui.notice("history already compact");
            }
        }
        "/search" => {
            let needle = rest.trim();
            if needle.is_empty() {
                tui.notice("usage: /search <text>");
            } else {
                let hits = app.session.search(needle);
                if hits.is_empty() {
                    tui.notice(format!("no matches for '{needle}'"));
                } else {
                    for (idx, role, content) in hits.iter().take(5) {
                        let s: String = content.chars().take(100).collect();
                        tui.notice(format!("[{idx} {role}] {s}"));
                    }
                    tui.notice(format!("{} match(es)", hits.len()));
                }
            }
        }
        "/save" => {
            let name = rest.trim();
            match sanitize_session_name(name) {
                Some(n) => {
                    let dir = std::path::Path::new("sessions");
                    let _ = std::fs::create_dir_all(dir);
                    let path = dir.join(format!("{n}.json"));
                    match app.session.save_to(&path) {
                        Ok(()) => tui.notice(format!("saved session → {}", path.display())),
                        Err(e) => tui.notice(format!("save failed: {e:#}")),
                    }
                }
                None => tui.notice("usage: /save <name> (letters, digits, - and _ only)"),
            }
        }
        "/load" => {
            let name = rest.trim();
            match sanitize_session_name(name) {
                Some(n) => {
                    let path = std::path::Path::new("sessions").join(format!("{n}.json"));
                    match crate::session::Session::load_from(&path) {
                        Ok(s) => {
                            app.session = s;
                            // Rebuild transcript from loaded history.
                            tui.entries.clear();
                            for m in app.session.messages() {
                                if m.role == "user" {
                                    tui.entries.push(ChatEntry::User(m.content.clone()));
                                } else if m.role == "assistant" {
                                    tui.entries.push(ChatEntry::Assistant(m.content.clone()));
                                }
                            }
                            tui.scroll_from_bottom = 0;
                            tui.notice(format!("loaded session '{}' ({} messages)", n, app.session.messages().len()));
                        }
                        Err(e) => tui.notice(format!("load failed: {e:#}")),
                    }
                }
                None => tui.notice("usage: /load <name>"),
            }
        }
        "/sessions" => {
            let mut names: Vec<String> = std::fs::read_dir("sessions")
                .map(|rd| {
                    rd.flatten()
                        .filter_map(|e| {
                            let n = e.file_name().to_string_lossy().to_string();
                            n.strip_suffix(".json").map(str::to_owned)
                        })
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            if names.is_empty() {
                tui.notice("no saved sessions — /save <name> creates one");
            } else {
                tui.notice(format!("{} saved session(s):", names.len()));
                for n in names.iter().take(15) {
                    tui.notice(format!("  {n}"));
                }
                tui.notice("load with /load <name>");
            }
        }
        "/fork" => {
            let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
            let name = rest.trim();
            let n = if name.is_empty() {
                format!("fork-{stamp}")
            } else {
                format!("{}-{}", sanitize_session_name(name).unwrap_or_else(|| "fork".into()), stamp)
            };
            let dir = std::path::Path::new("sessions");
            let _ = std::fs::create_dir_all(dir);
            let path = dir.join(format!("{n}.json"));
            match app.session.save_to(&path) {
                Ok(()) => tui.notice(format!("forked snapshot → {}", path.display())),
                Err(e) => tui.notice(format!("fork failed: {e:#}")),
            }
        }
        "/export" => {
            let fmt = rest.trim().to_ascii_lowercase();
            let md = fmt != "txt";
            let mut out = String::new();
            for m in app.session.messages() {
                if md {
                    out.push_str(if m.role == "user" { "**You:** " } else { "**Govinda:** " });
                } else {
                    out.push_str(if m.role == "user" { "You: " } else { "Govinda: " });
                }
                out.push_str(&m.content);
                out.push_str("\n\n");
            }
            let ext = if md { "md" } else { "txt" };
            let path = std::path::Path::new("sessions").join(format!("export.{ext}"));
            match std::fs::write(&path, out) {
                Ok(()) => tui.notice(format!("exported {} message(s) → {}", app.session.messages().len(), path.display())),
                Err(e) => tui.notice(format!("export failed: {e:#}")),
            }
        }
        "/stats" => {
            let elapsed = app.stats.started.map_or(std::time::Duration::ZERO, |s| s.elapsed());
            let avg = if app.stats.turns > 0 { app.stats.total_latency_ms / u128::from(app.stats.turns) } else { 0 };
            tui.notice(format!("turns {} · errors {} · avg {avg}ms · tokens ~{} · uptime {}s", app.stats.turns, app.stats.errors, app.session.approx_tokens(), elapsed.as_secs()));
        }
        "/theme" => {
            let name = rest.trim();
            if name.is_empty() {
                tui.notice(format!("current theme: {} (light/dark)", crate::tui::theme::active().name()));
            } else {
                let lower = name.to_ascii_lowercase();
                let target = if lower.contains("dark") { crate::tui::theme::DARK_THEME } else { crate::tui::theme::LIGHT_THEME };
                crate::tui::theme::set(target);
                tui.notice(format!("theme set to {}", target.name()));
            }
        }
        "/raw" => {
            app.renderer.set_markdown(!app.renderer.markdown_enabled());
            tui.notice(if app.renderer.markdown_enabled() { "markdown on" } else { "raw streaming" });
        }
        "/config" => {
            let c = rest.trim();
            if c.eq_ignore_ascii_case("save") {
                tui.notice("config save — use REPL /config save for full persist");
            } else {
                tui.notice(format!("provider {} model {} temp {:.2} budget {} timeout {}s", app.config.provider.id(), app.config.model, app.config.temperature, app.config.context_tokens, app.read_timeout.as_secs()));
            }
        }
        "/timeout" => {
            if let Ok(s) = rest.trim().parse::<u64>() { if (1..=600).contains(&s) { app.read_timeout = std::time::Duration::from_secs(s); tui.notice(format!("timeout {s}s")); return; } }
            tui.notice(format!("usage: /timeout <1-600> (current {}s)", app.read_timeout.as_secs()));
        }
        "/limit" => {
            if let Ok(mb) = rest.trim().parse::<u64>() { if (1..=64).contains(&mb) { app.max_response_bytes = (mb as usize)*1024*1024; tui.notice(format!("limit {mb}MB")); return; } }
            tui.notice(format!("usage: /limit <1-64> (current {}MB)", app.max_response_bytes/(1024*1024)));
        }
        "/tools" => {
            let arg = rest.trim();
            if arg.is_empty() {
                let on = if app.tools_enabled { "on" } else { "off" };
                tui.notice(format!("tools {on} — {} specs, {} disabled", app.tool_specs.len(), app.disabled_tools.len()));
                for t in &app.tool_specs {
                    let state = if app.disabled_tools.contains(&t.name) { "off" } else { "on" };
                    tui.notice(format!(" {state} {} — {}", t.name, t.description));
                }
            } else if arg == "on" || arg == "off" {
                app.tools_enabled = arg == "on";
                tui.notice(format!("tools {}", arg));
            } else if let Some((verb, name)) = arg.split_once(char::is_whitespace) {
                let dis = matches!(verb, "disable"|"dis"|"off");
                if dis { app.disabled_tools.insert(name.trim().to_owned()); tui.notice(format!("disabled {name}")); }
                else { app.disabled_tools.remove(name.trim()); tui.notice(format!("enabled {name}")); }
                let _ = crate::tools::save_disabled_tools(&app.disabled_tools);
            } else {
                tui.notice("usage: /tools [on|off] | /tools enable|disable <name>");
            }
        }
        "/todo" => {
            let (sub, r2) = rest.split_once(char::is_whitespace).map(|(s,r)|(s.to_ascii_lowercase(), r.trim())).unwrap_or((rest.trim().to_ascii_lowercase(), ""));
            match sub.as_str() {
                "" | "list" | "ls" => {
                    if app.todos.is_empty() { tui.notice("(no todos — /todo add <text>)"); }
                    else { for (i, td) in app.todos.iter().enumerate() { tui.notice(format!("{}. {} [{}]", i+1, td.text, if td.done {"x"} else {" "})); } }
                }
                "add" => {
                    if r2.is_empty() { tui.notice("usage: /todo add <text>"); }
                    else { app.todos.push(crate::commands::todo::Todo{ text: r2.to_owned(), done:false }); crate::commands::persist_todos(app); tui.notice(format!("added #{}: {}", app.todos.len(), r2)); }
                }
                "done" => {
                    if let Ok(n) = r2.parse::<usize>() { if n>=1 && n<=app.todos.len() { app.todos[n-1].done=true; crate::commands::persist_todos(app); tui.notice(format!("#{n} done")); } else { tui.notice("usage: /todo done <n>"); } } else { tui.notice("usage: /todo done <n>"); }
                }
                "undo" | "reopen" => {
                    if let Ok(n) = r2.parse::<usize>() { if n>=1 && n<=app.todos.len() { app.todos[n-1].done=false; crate::commands::persist_todos(app); tui.notice(format!("#{n} reopened")); } else { tui.notice("usage: /todo undo <n>"); } } else { tui.notice("usage: /todo undo <n>"); }
                }
                "rm" | "remove" | "del" => {
                    if let Ok(n) = r2.parse::<usize>() { if n>=1 && n<=app.todos.len() { let rm=app.todos.remove(n-1); crate::commands::persist_todos(app); tui.notice(format!("removed #{}: {}", n, rm.text)); } else { tui.notice("usage: /todo rm <n>"); } } else { tui.notice("usage: /todo rm <n>"); }
                }
                "clear" => { let n=app.todos.len(); app.todos.clear(); crate::commands::persist_todos(app); tui.notice(format!("cleared {n} todo(s)")); }
                _ => tui.notice("usage: /todo add|list|done|undo|rm|clear"),
            }
        }
        "/diff" => {
            let ops: Vec<crate::tools::EditOp> = match app.pending_edits.lock() {
                Ok(q) => q.ops().to_vec(),
                Err(_) => {
                    tui.notice("staged-edit queue poisoned");
                    return;
                }
            };
            if ops.is_empty() {
                tui.notice("no staged edits — nothing to diff");
                return;
            }
            for (i, op) in ops.iter().enumerate().take(8) {
                tui.notice(format!("{}. {}", i + 1, op.describe()));
            }
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            match crate::tools::staged_diff(&cwd, &ops) {
                Ok(diff) if diff.trim().is_empty() => tui.notice("(edits cancel out — empty diff)"),
                Ok(diff) => {
                    for line in diff.lines().take(14) {
                        tui.notice(line.to_string());
                    }
                    tui.notice(format!("(+{} more lines) … /apply to commit", diff.lines().count().saturating_sub(14)));
                }
                Err(e) => tui.notice(format!("cannot build diff: {e:#}")),
            }
        }
        "/apply" => {
            let ops: Vec<crate::tools::EditOp> = match app.pending_edits.lock() {
                Ok(q) => q.ops().to_vec(),
                Err(_) => {
                    tui.notice("staged-edit queue poisoned");
                    return;
                }
            };
            if ops.is_empty() {
                tui.notice("no staged edits to apply");
                return;
            }
            // Group by path (first-seen order), validate all, then write atomically.
            let mut grouped: Vec<(String, Vec<&crate::tools::EditOp>)> = Vec::new();
            for op in &ops {
                match grouped.iter_mut().find(|(p, _)| *p == op.path()) {
                    Some((_, g)) => g.push(op),
                    None => grouped.push((op.path().to_owned(), vec![op])),
                }
            }
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let mut writes: Vec<(std::path::PathBuf, String)> = Vec::new();
            for (path, group) in &grouped {
                // Read current content, apply this group's ops.
                let full = match crate::tools::resolve_in(&cwd, path) {
                    Ok(f) => f,
                    Err(e) => {
                        tui.notice(format!("apply aborted (nothing written): {e:#}"));
                        return;
                    }
                };
                match std::fs::read_to_string(&full) {
                    Ok(content) => {
                        let refs: Vec<&crate::tools::EditOp> = group.iter().copied().collect();
                        match crate::tools::apply_ops_to_content(&content, path, &refs) {
                            Ok(updated) => writes.push((full, updated)),
                            Err(e) => {
                                tui.notice(format!("apply aborted (nothing written): {e:#}"));
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        tui.notice(format!("apply aborted: cannot read '{path}': {e}"));
                        return;
                    }
                }
            }
            let mut failed = 0;
            for (full, content) in &writes {
                if std::fs::write(full, content).is_err() {
                    failed += 1;
                }
            }
            if failed > 0 {
                tui.notice(format!("{failed} write(s) failed — inspect files before retrying"));
            } else if let Ok(mut q) = app.pending_edits.lock() {
                let n = ops.len();
                let f = grouped.len();
                q.clear();
                tui.notice(format!("{} applied {n} edit(s) across {f} file(s)", icons::CHECK));
                if let Some(tree) = tui.tree.as_mut() {
                    tree.mark_dirty();
                }
            }
        }
        "/reject" => {
            let n = app.pending_edits.lock().map(|q| q.ops().len()).unwrap_or(0);
            if let Ok(mut q) = app.pending_edits.lock() {
                q.clear();
            }
            if n == 0 {
                tui.notice("nothing staged to reject");
            } else {
                tui.notice(format!("discarded {n} staged edit(s); no files changed"));
            }
        }
        "/review" => {
            let ops: Vec<crate::tools::EditOp> = match app.pending_edits.lock() {
                Ok(q) => q.ops().to_vec(),
                Err(_) => {
                    tui.notice("staged-edit queue poisoned");
                    return;
                }
            };
            if ops.is_empty() {
                tui.notice("no staged edits — nothing to review");
                return;
            }
            let mut grouped: Vec<(String, Vec<&crate::tools::EditOp>)> = Vec::new();
            for op in &ops {
                match grouped.iter_mut().find(|(p, _)| *p == op.path()) {
                    Some((_, g)) => g.push(op),
                    None => grouped.push((op.path().to_owned(), vec![op])),
                }
            }
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let mut total_add = 0usize;
            let mut total_rm = 0usize;
            tui.notice(format!("{} file(s) modified:", grouped.len()));
            for (path, group) in &grouped {
                let owned: Vec<crate::tools::EditOp> = group.iter().map(|op| (*op).clone()).collect();
                match crate::tools::staged_diff(&cwd, &owned) {
                    Ok(diff) => {
                        let (a, r) = crate::diff::count_changes(&diff);
                        total_add += a;
                        total_rm += r;
                        tui.notice(format!("  {path}: +{a}/-{r}"));
                    }
                    Err(e) => tui.notice(format!("  {path}: diff failed ({e:#})")),
                }
            }
            tui.notice(format!("total +{total_add}/-{total_rm} — /apply to commit, /reject to discard"));
        }
        "/scan" => {
            tui.notice("scanning workspace…");
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let overview = crate::scan::scan(&cwd).await;
            let n = crate::symbols::rebuild(&cwd);
            tui.notice(format!("scanned {n} symbols"));
            for line in overview.lines().take(12) { tui.notice(line.to_string()); }
        }
        "/project" => {
            let arg = rest.trim();
            if arg.is_empty() {
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                match std::fs::read_to_string(cwd.join(".govinda_project.json")) {
                    Ok(raw) => {
                        for line in raw.lines().take(8) {
                            tui.notice(line.to_string());
                        }
                    }
                    Err(_) => tui.notice("no project memory yet — /project set test|build <cmd>"),
                }
            } else if let Some((verb, r)) = arg.split_once(char::is_whitespace) {
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let path = cwd.join(".govinda_project.json");
                let mut map: serde_json::Value = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|raw| serde_json::from_str(&raw).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                match verb {
                    "set" => {
                        if let Some((key, cmd)) = r.split_once(char::is_whitespace) {
                            map[key.trim()] = serde_json::json!(cmd.trim());
                            match serde_json::to_string_pretty(&map).map(|j| std::fs::write(&path, j)) {
                                Ok(Ok(())) => tui.notice(format!("project {key} = '{cmd}'")),
                                _ => tui.notice("could not write project memory"),
                            }
                        } else {
                            tui.notice("usage: /project set test|build <cmd>");
                        }
                    }
                    "clear" => {
                        if let Some(obj) = map.as_object_mut() {
                            obj.remove(r.trim());
                        }
                        match serde_json::to_string_pretty(&map).map(|j| std::fs::write(&path, j)) {
                            Ok(Ok(())) => tui.notice(format!("cleared project {r}")),
                            _ => tui.notice("could not write project memory"),
                        }
                    }
                    _ => tui.notice("usage: /project show | set test|build <cmd> | clear test|build"),
                }
            } else {
                tui.notice("usage: /project show | set test|build <cmd> | clear test|build");
            }
        }
        "/quit" | "/exit" | "/q" => { tui.quit = true; tui.notice("quitting…"); },
        "/clear" | "/reset" => { app.session.clear(); tui.entries.clear(); tui.notice("cleared"); },
        "/help" => {
            for l in [
                "keys: Tab focus · ↑/↓ palette · Enter opens args dialog · F5 refresh",
                "      dialog: type args · Enter execute · Esc cancel · click outside closes",
                "type \"/\" in the input to browse all 37 commands",
            ] {
                tui.notice(l.to_string());
            }
        }
        _ => tui.notice(format!("executed {line}")),
    }
}

/// Session-name sanitizer for /save, /load, /fork: lowercase word chars and
/// dashes only; anything else (paths, dots) is rejected.
fn sanitize_session_name(raw: &str) -> Option<String> {
    let n = raw.trim();
    if n.is_empty() || n.len() > 64 {
        return None;
    }
    if !n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    Some(n.to_ascii_lowercase())
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
                    let _gated = executor
                        .as_ref()
                        .is_some_and(|e| e.requires_confirmation(&call.function.name));

                    // Auto-accept for all tools — user requested realtime file ops
                    // without REVIEW prompts. The original gated logic is kept below
                    // as comment for reference; it would send ConfirmNeeded and wait
                    // for y/n/a. Now we auto-approve so files show instantly.
                    let approved = true;
                    let _ = approve_all_remaining; // keep variable for future use
                    /* original gated approval (now auto-accepted):
                    let approved = !_gated || approve_all_remaining || {
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
                    */

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

    fn type_str(t: &mut Tui, s: &str) {
        for c in s.chars() {
            t.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    #[test]
    fn zero_arg_command_enter_runs_directly() {
        let mut t = Tui::new();
        type_str(&mut t, "/history");
        t.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(t.slash_dialog.is_none(), "no dialog for zero-arg command");
        assert_eq!(t.take_pending_slash().as_deref(), Some("/history"));
    }

    #[test]
    fn exact_arg_command_enter_opens_dialog() {
        let mut t = Tui::new();
        type_str(&mut t, "/save");
        t.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        // Bare "/save" must NOT execute (it would only print a usage notice);
        // the args dialog collects the name instead.
        let d = t.slash_dialog.as_ref().expect("dialog should open for /save");
        assert_eq!(d.command, "/save");
        assert!(t.take_pending_slash().is_none());
        // Confirming with empty arg queues the bare command.
        t.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(t.slash_dialog.is_none());
        assert_eq!(t.take_pending_slash().as_deref(), Some("/save"));
    }

    #[test]
    fn partial_command_enter_opens_dialog() {
        let mut t = Tui::new();
        type_str(&mut t, "/sav");
        t.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let d = t.slash_dialog.as_ref().expect("dialog should open");
        assert_eq!(d.command, "/save");
    }

    #[test]
    fn dialog_accepts_args_and_confirms() {
        let mut t = Tui::new();
        type_str(&mut t, "/sav");
        t.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        type_str(&mut t, "mysession");
        t.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(t.slash_dialog.is_none());
        assert_eq!(t.take_pending_slash().as_deref(), Some("/save mysession"));
    }

    #[test]
    fn navigated_palette_enter_opens_selected() {
        let mut t = Tui::new();
        type_str(&mut t, "/");
        // Navigate down to "/model" (index 6) — an arg-taking command.
        for _ in 0..6 {
            t.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        t.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let d = t.slash_dialog.as_ref().expect("dialog opens on navigated enter");
        let hits = crate::tui::widgets::input_bar::filtered("/");
        assert_eq!(d.command, hits[6]);
        assert_eq!(d.command, "/model");
    }

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

    #[test]
    fn slash_palette_shows_all_and_filters() {
        use crate::tui::widgets::input_bar;
        assert_eq!(input_bar::filtered("/").len(), crate::commands::SLASH_COMMANDS.len());
        assert!(input_bar::filtered("/hel").contains(&"/help"));
        assert!(input_bar::filtered("/to").contains(&"/todo"));
        assert!(input_bar::filtered("/xyz").is_empty());
        let lines = input_bar::palette_lines("/", 0, None);
        assert!(lines.len() > 5, "palette should have header + rows");
        // selected highlighting
        let lines2 = input_bar::palette_lines("/hel", 1, Some(1));
        assert!(lines2.iter().any(|l| l.spans.iter().any(|s| s.content.contains("/help"))));
    }

    #[test]
    fn all_slash_commands_are_handled() {
        // Every SLASH_COMMANDS entry should either be handled locally (notice/quit/clear)
        // or queued as pending_slash for App-aware dispatch — never "not wired".
        for cmd in crate::commands::SLASH_COMMANDS {
            let mut tui = Tui::new();
            // use a basic arg where needed to avoid usage notices being mistaken for failure
            let input = match cmd {
                "/model" => "/model test-model".to_string(),
                "/temp" => "/temp 0.5".to_string(),
                "/system" => "/system test".to_string(),
                "/search" => "/search hello".to_string(),
                "/save" => "/save test".to_string(),
                "/load" => "/load test".to_string(),
                "/export" => "/export md".to_string(),
                "/timeout" => "/timeout 30".to_string(),
                "/limit" => "/limit 8".to_string(),
                "/tools" => "/tools".to_string(),
                "/todo" => "/todo list".to_string(),
                "/scan" => "/scan".to_string(),
                "/plan" => "/plan task".to_string(),
                "/project" => "/project show".to_string(),
                _ => cmd.to_string(),
            };
            tui.set_input(input.clone());
            tui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            // Arg-taking commands open the args dialog on Enter — confirm it.
            if tui.slash_dialog.is_some() {
                tui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            }
            let has_notice = tui.entries.iter().any(|e| matches!(e, ChatEntry::Notice(_)));
            let pending = tui.take_pending_slash().is_some();
            let is_local = has_notice || tui.quit || tui.take_clear_request() || pending;
            assert!(is_local, "slash {cmd} should be handled (notice/pending/quit/clear) but was not");
            // Ensure never the old "not wired" message
            let wired = tui.entries.iter().any(|e| matches!(e, ChatEntry::Notice(t) if t.contains("not wired")));
            assert!(!wired, "slash {cmd} still shows not wired");
        }
    }

    #[tokio::test]
    async fn tui_slash_dispatch_covers_all_commands() {
        // Smoke App for handle_tui_slash
        let provider = crate::provider::resolve("ollama", None, None, |_| None).expect("ollama preset");
        let config = crate::config::Config {
            api_key: std::sync::Arc::new(zeroize::Zeroizing::new(String::new())),
            model: "test-model".to_owned(),
            temperature: 0.5,
            render_markdown: false,
            system_prompt: "sys".to_owned(),
            context_tokens: 2048,
            provider,
            source_path: None,
            shell_tools: Vec::new(),
            theme: None,
            timeout_secs: 30,
            limit_mb: 16,
        };
        let mut app = crate::commands::App::new(
            config,
            reqwest::Client::new(),
            crate::session::Session::new("sys"),
            crate::render::Renderer::new(false),
        );
        for cmd in crate::commands::SLASH_COMMANDS {
            let mut tui = Tui::new();
            let line = match cmd {
                "/model" => "/model foo",
                "/temp" => "/temp 0.7",
                "/system" => "/system hi",
                "/search" => "/search hi",
                "/save" => "/save t",
                "/load" => "/load t",
                "/export" => "/export md",
                "/timeout" => "/timeout 30",
                "/limit" => "/limit 8",
                "/todo" => "/todo list",
                "/scan" => "/scan",
                "/project" => "/project show",
                "/plan" => "/plan do x",
                _ => cmd,
            };
            handle_tui_slash(&mut app, &mut tui, line).await;
            let handled = tui.entries.iter().any(|e| matches!(e, ChatEntry::Notice(_))) || tui.quit || tui.take_clear_request();
            assert!(
                handled,
                "handle_tui_slash for {cmd} should push a Notice or set quit/clear"
            );
        }
    }
}
