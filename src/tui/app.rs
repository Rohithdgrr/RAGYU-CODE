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
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use super::widgets::chat_pane::ChatEntry;
use super::widgets::file_tree::FileTree;
use super::{draw, icons, theme};
use crate::api;
use crate::commands::{self, App};

/// Slash commands that never take an argument — Enter on the palette runs
/// them directly instead of opening the args dialog.
const ZERO_ARG_SLASH: [&str; 24] = [
    "/help", "/exit", "/quit", "/q", "/clear", "/reset", "/sessions", "/stats", "/history",
    "/models", "/tools", "/config", "/tokens", "/undo", "/retry", "/compact", "/raw", "/scan",
    "/pin", "/variants", "/pick", "/agent", "/skills", "/auto-compact",
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
    /// Available model IDs (populated for `/model` dialog).
    pub models: Vec<String>,
    /// Index of the currently highlighted model in the list.
    pub models_selected: usize,
}

/// Live update pushed from the turn runner back to the UI thread.
pub enum TurnUpdate {
    AssistantProse(String),
    ToolStart { name: String, args: String },
    /// `snippet` is a short result preview (first line, truncated).
    ToolEnd { name: String, args: String, ok: bool, snippet: String },
    Answer(String),
    Notice(String),
    Error(String),
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
    /// True while the input gate is up for a workspace-mutating call.
    pub confirm_pending: bool,
    /// Files pinned via the tree sidebar; injected into every turn's context.
    pub pinned_files: Vec<PathBuf>,
    // ── Plan mode ──
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
    /// Cached model list fetched from the API (used by `/model` dialog).
    pub models_cache: Vec<String>,
    /// Set when `/model` is selected from palette — triggers async model fetch
    /// in the event loop before opening the dialog.
    pending_model_fetch: bool,
    /// @-mention file picker state
    pub at_picker_active: bool,
    pub at_picker_query: String,
    pub at_picker_files: Vec<String>,
    pub at_picker_selected: usize,
    /// Usage/cost dashboard modal state
    pub show_cost_dashboard: bool,
    /// `/raw` — render assistant output as plain text instead of markdown
    /// (mirrors `app.renderer.markdown_enabled()`, synced after commands).
    pub raw_mode: bool,
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
            confirm_pending: false,
            pinned_files: Vec::new(),
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
            models_cache: Vec::new(),
            pending_model_fetch: false,
            at_picker_active: false,
            at_picker_query: String::new(),
            at_picker_files: Vec::new(),
            at_picker_selected: 0,
            show_cost_dashboard: false,
            raw_mode: false,
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

    /// Consumes a pending model-fetch request (triggered when `/model` is
    /// selected from the palette). Returns `true` once.
    pub fn take_model_fetch_request(&mut self) -> bool {
        std::mem::take(&mut self.pending_model_fetch)
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
            // Bare "/" (or "/ " …): restore it so the palette dropdown
            // reopens instead of failing as an unknown command.
            if line.chars().all(|c| c == '/' || c == ' ') {
                self.input.push('/');
                self.input_cursor = 1;
                self.slash_selected = 0;
                return;
            }
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
        // For `/model`, populate the dialog with available models from cache.
        let models = if cmd == "/model" && !self.models_cache.is_empty() {
            self.models_cache.clone()
        } else {
            Vec::new()
        };
        self.slash_dialog = Some(SlashDialog {
            command: cmd.to_owned(),
            desc: desc.to_owned(),
            arg_input: String::new(),
            arg_cursor: 0,
            models,
            models_selected: 0,
        });
    }

    fn close_slash_dialog(&mut self) {
        self.slash_dialog = None;
    }

    /// Confirms the dialog: queues `command [args]` for App-aware dispatch.
    fn confirm_slash_dialog(&mut self) {
        if let Some(d) = self.slash_dialog.take() {
            let full = if d.arg_input.trim().is_empty() {
                // For `/model`, use the highlighted model from the list.
                if d.command == "/model" && !d.models.is_empty() {
                    let name = d
                        .models
                        .get(d.models_selected)
                        .cloned()
                        .unwrap_or_default();
                    format!("{} {}", d.command, name)
                } else {
                    d.command.clone()
                }
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
            KeyCode::Up => {
                if let Some(dialog) = &mut self.slash_dialog {
                    if !dialog.models.is_empty() {
                        dialog.models_selected = if dialog.models_selected == 0 {
                            dialog.models.len() - 1
                        } else {
                            dialog.models_selected - 1
                        };
                        // Pre-fill arg_input with the highlighted model name.
                        if let Some(name) = dialog.models.get(dialog.models_selected) {
                            dialog.arg_input = name.clone();
                            dialog.arg_cursor = dialog.arg_input.chars().count();
                        }
                    } else {
                        if dialog.arg_cursor > 0 { dialog.arg_cursor -= 1; } else { return false; }
                    }
                }
            }
            KeyCode::Down => {
                if let Some(dialog) = &mut self.slash_dialog {
                    if !dialog.models.is_empty() {
                        dialog.models_selected = (dialog.models_selected + 1) % dialog.models.len();
                        if let Some(name) = dialog.models.get(dialog.models_selected) {
                            dialog.arg_input = name.clone();
                            dialog.arg_cursor = dialog.arg_input.chars().count();
                        }
                    } else {
                        if dialog.arg_cursor < dialog.arg_input.chars().count() { dialog.arg_cursor += 1; } else { return false; }
                    }
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
        let (cmd, _rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
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
                     cmds: /help /clear /theme /tokens /agent <on|off> /plan <task> /model /temp /system /history /undo /retry /variants /pick /compact /search /save /load /sessions /fork /export /stats /raw /config /timeout /limit /tools /todo /diff /apply /reject /review /scan /project\n\
                      Tip: type \"/\" to see the palette — Enter/↑↓/click open the args dialog, Tab completes.",
                );
                return true;
            }
            "/theme" | "/tokens" | "/plan" => {
                // Theme switching, token counts and planning are handled by
                // the unified command dispatcher (commands::dispatch) so the
                // TUI and REPL always agree.
                return false;
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
            // "/agent" goes through the unified dispatcher, which toggles
            // app.tools_enabled for real; handle_tui_slash syncs the badge.
            _ => {}
        }        // Known slash commands and custom skills need App-aware handling
        if crate::commands::SLASH_COMMANDS.contains(&cmd_lc.as_str()) {
            return false;
        }

        // Unknown command — queue for handle_tui_slash which checks skills
        false
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
        if let Some(dialog) = &self.slash_dialog {
            // centered dialog — compute same as draw (dynamic height for model list)
            let full_w = layout.status.width;
            let full_h = layout.status.height + layout.chat.height + layout.input.height;
            let dw: u16 = 60;
            let model_visible = if !dialog.models.is_empty() { dialog.models.len().min(8) as u16 } else { 0 };
            let dh: u16 = if !dialog.models.is_empty() { 9 + model_visible + 1 } else { 9 };
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
                                    if cmd == "/model" {
                                        self.pending_model_fetch = true;
                                    } else {
                                        self.open_slash_dialog(cmd);
                                    }
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
        // Review-mode prompt intercepts everything except quit. (Kept for
        // future interactive gating; the shared agent loop currently runs
        // gated tools in AutoRun mode, so this never triggers.)
        if self.confirm_pending {
            match (key.modifiers, key.code) {
                (KeyModifiers::CONTROL, KeyCode::Char('q')) => self.quit = true,
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
                    'i' => {
                        self.show_cost_dashboard = !self.show_cost_dashboard;
                        return;
                    }
                    'o' => {
                        // Ctrl+O: zen mode (toggle sidebars)
                        if self.show_tree || self.show_tools {
                            self.show_tree = false;
                            self.show_tools = false;
                        } else {
                            self.show_tree = true;
                            self.show_tools = true;
                        }
                        return;
                    }
                    'f' => {
                        // Ctrl+F: find in chat (placeholder)
                        self.notice("find in chat: use /search <text>");
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
                            } else if cmd == "/model" {
                                // `/model` needs an async model list fetch before
                                // opening the dialog, so flag it for the event loop.
                                self.pending_model_fetch = true;
                                return;
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

    fn handle_input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                // If @-picker is active, insert selected file
                if self.at_picker_active && !self.at_picker_files.is_empty() {
                    self.insert_at_mention();
                    return;
                }
                self.submit();
            }
            KeyCode::Char(c) => {
                let byte = self.cursor_byte();
                self.input.insert(byte, c);
                self.input_cursor += 1;
                self.slash_selected = 0;
                self.update_at_picker();
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
                self.update_at_picker();
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
            KeyCode::Up => {
                if self.at_picker_active && !self.at_picker_files.is_empty() {
                    self.at_picker_selected = if self.at_picker_selected == 0 {
                        self.at_picker_files.len() - 1
                    } else {
                        self.at_picker_selected - 1
                    };
                } else {
                    self.history_prev();
                }
            }
            KeyCode::Down => {
                if self.at_picker_active && !self.at_picker_files.is_empty() {
                    self.at_picker_selected = (self.at_picker_selected + 1) % self.at_picker_files.len();
                } else {
                    self.history_next();
                }
            }
            KeyCode::Esc => {
                if self.show_cost_dashboard {
                    self.show_cost_dashboard = false;
                } else if self.at_picker_active {
                    self.at_picker_active = false;
                    self.at_picker_files.clear();
                }
            }
            _ => {}
        }
    }

    /// Updates the @-mention picker based on current input and cursor position.
    fn update_at_picker(&mut self) {
        if let Some(query) = crate::tui::widgets::input_bar::at_mention_query(&self.input, self.input_cursor) {
            self.at_picker_active = true;
            self.at_picker_query = query.clone();
            self.at_picker_files = crate::tui::widgets::input_bar::at_mention_files(&query);
            self.at_picker_selected = 0;
        } else {
            self.at_picker_active = false;
            self.at_picker_files.clear();
        }
    }

    /// Inserts the selected @-mention file into the input.
    fn insert_at_mention(&mut self) {
        if !self.at_picker_active || self.at_picker_files.is_empty() {
            return;
        }
        let file = self.at_picker_files[self.at_picker_selected].clone();
        // Find the @ position and replace query with file path
        let text: String = self.input.chars().take(self.input_cursor).collect();
        let chars: Vec<char> = text.chars().collect();
        let mut last_at = None;
        for (i, &c) in chars.iter().enumerate() {
            if c == '@' && (i == 0 || chars[i - 1].is_whitespace()) {
                last_at = Some(i);
            }
        }
        if let Some(at_pos) = last_at {
            // Replace from @ to cursor with @file_path
            let before: String = chars[..at_pos].iter().collect();
            let after: String = self.input.chars().skip(self.input_cursor).collect();
            self.input = format!("{before}@{file} {after}");
            self.input_cursor = before.chars().count() + 1 + file.chars().count() + 1;
        }
        self.at_picker_active = false;
        self.at_picker_files.clear();
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
            TurnUpdate::ToolEnd { name, args, ok, snippet } => {
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
                // Result preview so users can see what read_file/grep/…
                // returned without leaving the transcript.
                if !snippet.trim().is_empty() {
                    let text: String = snippet.chars().take(160).collect();
                    self.entries
                        .push(ChatEntry::Notice(format!("↳ {text}")));
                }
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
            // Fetch available models from the API, then open the /model dialog.
            if tui.take_model_fetch_request() {
                let models = if let Some(url) = app.config.provider.models_url() {
                    match api::list_models(&app.http, &url, app.config.provider.auth().token()).await {
                        Ok(list) => list,
                        Err(e) => {
                            tui.notice(format!("failed to fetch models: {e:#}"));
                            Vec::new()
                        }
                    }
                } else {
                    tui.notice(format!(
                        "provider '{}' has no model-listing endpoint",
                        app.config.provider.id()
                    ));
                    Vec::new()
                };
                tui.models_cache = models;
                tui.open_slash_dialog("/model");
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
            let ui = TuiUi {
                tx: tx.clone(),
                cancel: tui.cancel.clone(),
                streaming: tui.streaming.clone(),
                pinned,
                prompt,
            };
            let mut turn = Some(Box::pin(crate::agent_loop::run_turn(
                app,
                &ui,
                crate::agent_loop::GatePolicy::AutoRun,
                ui.prompt.as_str(),
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
                    _res = turn.as_mut().expect("turn alive while polling").as_mut() => {
                        drop(turn.take());
                        let interrupted = tui.cancel.load(Ordering::Relaxed);
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

async fn handle_tui_slash(
    app: &mut App,
    tui: &mut Tui,
    line: &str,
) -> commands::output::CommandOutput {
    // Unified command path: the same dispatcher the REPL uses, captured into
    // structured output instead of printing to stdout (which would corrupt
    // the alternate screen). One source of truth for every slash command.
    let agent_before = app.tools_enabled;
    let out = commands::dispatch_structured(line, app).await;
    // `/agent on|off` toggles function calling on App — mirror it in the
    // NORMAL/AGENT badge so the label reflects real capability.
    if app.tools_enabled != agent_before {
        let next = if app.tools_enabled {
            AppMode::Agent
        } else {
            AppMode::Normal
        };
        let _ = tui.mode.transition_to(next);
        tui.notice(format!("mode: {}", tui.mode.label()));
    }
    apply_command_output(app, tui, line, out.clone());
    // `/raw` toggles the renderer inside the dispatcher; keep the pane flag
    // in sync so the chat view switches live.
    tui.raw_mode = !app.renderer.markdown_enabled();
    out
}

/// Maps [`CommandOutput`] onto the TUI: messages become notices or assistant
/// entries, effects drive theme switches, transcript syncs, plan checklists.
fn apply_command_output(app: &mut App, tui: &mut Tui, line: &str, out: commands::output::CommandOutput) {
    use commands::output::{Effect, Role};

    for msg in &out.msgs {
        match msg.role {
            Role::Markdown => {
                tui.entries.push(ChatEntry::Assistant(msg.text.clone()));
                tui.scroll_from_bottom = 0;
            }
            Role::Err => tui.entries.push(ChatEntry::Notice(format!("error: {}", msg.text))),
            _ => tui.notice(msg.text.clone()),
        }
    }

    match out.effect {
        Effect::None => {}
        Effect::ExitRequested => {
            tui.quit = true;
        }
        Effect::Resend(text) => {
            // `/retry` semantics: drop the trailing assistant entries so the
            // regenerated answer does not duplicate the failed one.
            let mut end = tui.entries.len();
            while end > 0 {
                match tui.entries[end - 1] {
                    ChatEntry::Notice(_) => end -= 1,
                    ChatEntry::Assistant(_) => break,
                    _ => break,
                }
            }
            let mut keep = end;
            if keep > 0 && matches!(tui.entries[keep - 1], ChatEntry::Assistant(_)) {
                keep -= 1;
            }
            tui.entries.truncate(keep);
            tui.pending_prompt = Some(text);
        }
        Effect::Plan(steps) => {
            let task = line.split_once(char::is_whitespace).map_or("task", |(_, r)| r.trim());
            tui.start_plan(task, steps);
        }
        Effect::ThemeChanged(name) => {
            if theme::set_by_name(&name) {
                tui.notice(format!("theme set to {name}"));
            }
        }
        Effect::ReloadTranscript => {
            tui.entries.clear();
            for m in app.session.messages() {
                match m.role.as_str() {
                    "user" => tui.entries.push(ChatEntry::User(m.content.clone())),
                    "assistant" => tui.entries.push(ChatEntry::Assistant(m.content.clone())),
                    _ => {}
                }
            }
            tui.scroll_from_bottom = 0;
        }
        Effect::PopExchange => {
            pop_last_exchange(&mut tui.entries);
        }
    }
}

/// Drops the trailing user+assistant pair from the transcript, skipping any
/// notices printed alongside (the dispatcher emits them before the effect).
fn pop_last_exchange(entries: &mut Vec<ChatEntry>) {
    let mut end = entries.len();
    while end > 0 && matches!(entries[end - 1], ChatEntry::Notice(_)) {
        end -= 1;
    }
    let mut keep = end;
    if keep > 0 && matches!(entries[keep - 1], ChatEntry::Assistant(_)) {
        keep -= 1;
    }
    if keep > 0 && matches!(entries[keep - 1], ChatEntry::User(_)) {
        keep -= 1;
    }
    entries.truncate(keep);
}

// ─── Turn runner ─────────────────────────────────────────────────────────────

fn draw_frame(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    info: &draw::StatusInfo,
    tui: &Tui,
) -> Result<()> {
    terminal.draw(|f| draw::draw(f, tui, info))?;
    Ok(())
}

/// Bridge between the shared agent loop ([`crate::agent_loop`]) and the
/// TUI: every loop callback is forwarded as a [`TurnUpdate`] over the mpsc
/// channel so the event loop keeps drawing and reacting to keys while the
/// turn runs.
struct TuiUi {
    tx: tokio::sync::mpsc::UnboundedSender<TurnUpdate>,
    cancel: Arc<AtomicBool>,
    streaming: SharedStream,
    /// Files pinned from the tree sidebar (context injection).
    #[allow(dead_code)]
    pinned: Vec<PathBuf>,
    prompt: String,
}

impl crate::agent_loop::AgentUi for TuiUi {
    fn raw_stream(&self) -> bool {
        // The chat pane renders the shared live buffer, so deltas always
        // surface immediately.
        true
    }

    fn stream_delta(&self, delta: &str) {
        self.streaming.borrow_mut().push_str(delta);
    }

    fn prose(&self, text: &str) {
        let _ = self.tx.send(TurnUpdate::AssistantProse(text.to_owned()));
    }

    fn answer(&self, text: &str) {
        let _ = self.tx.send(TurnUpdate::Answer(text.to_owned()));
    }

    fn tool_start(&self, name: &str, args: &str) {
        let _ = self.tx.send(TurnUpdate::ToolStart {
            name: name.to_owned(),
            args: args.to_owned(),
        });
    }

    fn tool_end(&self, name: &str, args: &str, ok: bool, snippet: &str) {
        let _ = self.tx.send(TurnUpdate::ToolEnd {
            name: name.to_owned(),
            args: args.to_owned(),
            ok,
            snippet: snippet.to_owned(),
        });
    }

    fn diff(&self, diff: &str) {
        let _ = self.tx.send(TurnUpdate::Notice(diff.to_owned()));
    }

    fn notice(&self, text: &str) {
        let _ = self.tx.send(TurnUpdate::Notice(text.to_owned()));
    }

    fn error(&self, text: &str) {
        let _ = self.tx.send(TurnUpdate::Error(text.to_owned()));
    }

    fn timeline(&self, model: &str, elapsed: std::time::Duration) {
        let _ = self.tx.send(TurnUpdate::Notice(format!(
            "── {model} · {:.1}s",
            elapsed.as_secs_f32()
        )));
    }

    fn cancel_wait<'a>(&'a self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>> {
        Box::pin(async move {
            loop {
                if self.cancel.load(Ordering::Relaxed) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
    }
}

#[cfg(test)]#[cfg(test)]
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
    fn plan_via_dialog_reaches_plan_runner() {
        // Regression: /plan confirmed through the args dialog used to fall
        // into handle_tui_slash's dead fallback instead of queueing the task.
        let mut t = Tui::new();
        type_str(&mut t, "/plan");
        t.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(t.slash_dialog.is_some(), "/plan should open the args dialog");
        type_str(&mut t, "build the parser");
        t.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            t.take_pending_slash().as_deref(),
            Some("/plan build the parser")
        );
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
        // Navigate down to "/model" — an arg-taking command.
        let hits = crate::tui::widgets::input_bar::filtered("/");
        let idx = hits.iter().position(|c| *c == "/model").expect("/model listed");
        for _ in 0..idx {
            t.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        t.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        // `/model` triggers async model fetch, not a direct dialog open.
        assert!(t.take_model_fetch_request(), "/model should trigger model fetch");
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

        // Unknown commands are queued for handle_tui_slash (which checks skills)
        tui.set_input("/bogus".into());
        tui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(tui.take_submission().is_none());
        // The command is queued as pending_slash, not handled locally
        assert_eq!(tui.take_pending_slash().as_deref(), Some("/bogus"));
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
    fn tool_updates_pair_start_and_end_with_snippet() {
        let mut tui = Tui::new();
        tui.apply_update(TurnUpdate::ToolStart {
            name: "read_file".into(),
            args: "{}".into(),
        });
        tui.apply_update(TurnUpdate::ToolEnd {
            name: "read_file".into(),
            args: "{}".into(),
            ok: true,
            snippet: "fn main() {}".into(),
        });
        assert!(matches!(
            tui.entries.last(),
            Some(ChatEntry::Notice(n)) if n.starts_with("↳ fn main")
        ));
        // The tool row itself resolved to ok.
        assert!(tui.entries.iter().any(|e| matches!(
            e,
            ChatEntry::Tool { ok: Some(true), .. }
        )));
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
            let out = handle_tui_slash(&mut app, &mut tui, line).await;
            let handled = !out.is_silent()
                || tui.quit
                || tui.take_clear_request()
                || tui.entries.iter().any(|e| matches!(e, ChatEntry::Notice(_)));
            assert!(
                handled,
                "handle_tui_slash for {cmd} should print something or set a flag/effect"
            );
            // Stub detection: no unified command may defer to the REPL.
            let stubbed = tui.entries.iter().any(|e| matches!(e, ChatEntry::Notice(t)
                if t.contains("use REPL") || t.contains("REPL-only") || t.contains("requires REPL")));
            assert!(!stubbed, "handle_tui_slash for {cmd} still defers to the REPL");
        }
    }

    /// `/agent on|off` must toggle REAL capability (`app.tools_enabled`),
    /// not just a status-bar label — and the badge must follow.
    #[tokio::test]
    async fn agent_command_toggles_tools_and_badge() {
        let mut app = smoke_app();
        app.tools_enabled = true;
        let mut tui = Tui::new();

        handle_tui_slash(&mut app, &mut tui, "/agent off").await;
        assert!(!app.tools_enabled, "tools must be disabled");
        assert_eq!(tui.mode, AppMode::Normal);

        handle_tui_slash(&mut app, &mut tui, "/agent on").await;
        assert!(app.tools_enabled, "tools must be re-enabled");
        assert_eq!(tui.mode, AppMode::Agent);
    }

    fn smoke_app() -> App {
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
        App::new(
            config,
            reqwest::Client::new(),
            crate::session::Session::new("sys"),
            crate::render::Renderer::new(false),
        )
    }

    /// `/undo` must keep the TUI transcript in sync with the session: the
    /// last exchange disappears from BOTH.
    #[tokio::test]
    async fn undo_syncs_transcript_and_session() {
        use commands::output::Effect;
        let mut app = smoke_app();
        app.session.push_user("q1");
        app.session.push_assistant("a1");
        app.session.push_user("q2");
        app.session.push_assistant("a2");
        let mut tui = Tui::new();
        tui.entries.push(ChatEntry::User("q1".into()));
        tui.entries.push(ChatEntry::Assistant("a1".into()));
        tui.entries.push(ChatEntry::User("q2".into()));
        tui.entries.push(ChatEntry::Assistant("a2".into()));

        let out = handle_tui_slash(&mut app, &mut tui, "/undo").await;
        assert_eq!(out.effect, Effect::PopExchange);
        assert_eq!(app.session.messages().len(), 2);
        // Transcript now ends with the first exchange's assistant reply.
        assert!(matches!(
            tui.entries.last(),
            Some(ChatEntry::Assistant(t)) if t == "a1"
        ));
    }

    /// `/save` then `/load` must round-trip through disk with a rebuilt
    /// transcript (previously `/save` printed fake success).
    #[tokio::test]
    async fn save_and_load_round_trip_through_disk() {
        use commands::output::Effect;
        let _guard = crate::TEST_CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut app = smoke_app();
        app.session.push_user("hello");
        app.session.push_assistant("world");
        let mut tui = Tui::new();

        let out = handle_tui_slash(&mut app, &mut tui, "/save govinda-roundtrip-test").await;
        let path = std::path::Path::new("sessions").join("govinda-roundtrip-test");
        assert!(
            path.exists(),
            "session file must exist on disk; cwd={:?}; msgs={:?}",
            std::env::current_dir(),
            out.msgs
        );
        assert!(out.msgs.iter().any(|m| m.text.contains("saved")), "{out:?}");

        // Fresh state, then load.
        let mut app2 = smoke_app();
        let mut tui2 = Tui::new();
        assert!(app2.session.messages().is_empty());
        let out = handle_tui_slash(&mut app2, &mut tui2, "/load govinda-roundtrip-test").await;
        assert_eq!(out.effect, Effect::ReloadTranscript);
        assert_eq!(app2.session.messages().len(), 2);
        assert!(
            tui2.entries.iter().any(|e| matches!(e, ChatEntry::Assistant(t) if t == "world")),
            "transcript must be rebuilt from the loaded session"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// A committed variant (`/pick`) lands as an Assistant chat entry.
    #[tokio::test]
    async fn pick_commits_variant_as_assistant_entry() {
        let mut app = smoke_app();
        app.pending_variants.push("variant answer".to_owned());
        let mut tui = Tui::new();
        handle_tui_slash(&mut app, &mut tui, "/pick 1").await;
        assert!(app.pending_variants.is_empty());
        assert_eq!(app.session.messages().last().map(|m| m.content.as_str()), Some("variant answer"));
        assert!(tui.entries.iter().any(|e| matches!(e, ChatEntry::Assistant(t) if t == "variant answer")));
    }

    /// `/theme <name>` flows through the unified dispatcher and switches the
    /// TUI glass palette by name.
    #[tokio::test]
    async fn theme_switch_applies_glass_palette() {
        use commands::output::Effect;
        let mut app = smoke_app();
        let mut tui = Tui::new();
        let before = theme::active().name;
        let out = handle_tui_slash(&mut app, &mut tui, "/theme dracula").await;
        assert_eq!(out.effect, Effect::ThemeChanged("dracula".to_owned()));
        assert_eq!(theme::active().name, "dracula");
        assert_ne!(before, theme::active().name, "{before}");
        theme::set(theme::DARK_THEME); // restore
    }
}
