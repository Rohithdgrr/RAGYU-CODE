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
use crate::provider;

/// Slash commands that never take an argument — Enter on the palette or a
/// click on the palette row runs them directly instead of opening the args
/// dialog. These mirror the always-runnable subset of
/// `commands::SLASH_COMMANDS`.
const ZERO_ARG_SLASH: [&str; 10] = [
    "/help",
    "/exit",
    "/quit",
    "/q",
    "/clear",
    "/reset",
    "/models",
    "/tokens",
    "/retry",
    "/history",
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
    /// Settings modal (theme / provider)
    pub show_settings: bool,
    /// Shortcuts modal (single entry for all hints)
    pub show_shortcuts: bool,
    /// `/raw` — render assistant output as plain text instead of markdown
    /// (mirrors `app.renderer.markdown_enabled()`, synced after commands).
    pub raw_mode: bool,
    /// Last known mouse X position (for focused-pane beam effect).
    pub mouse_x: u16,
    /// Current provider name — drives accent color remapping.
    pub provider_name: String,
    /// Multi-step provider setup workflow state.
    pub provider_workflow: Option<ProviderWorkflow>,
    /// Signals event loop to fetch models for (provider, api_key) and open SelectModel step.
    pub pending_setup_models: Option<(String, String)>,
    /// Signals event loop to run connection test for the current workflow.
    pub pending_setup_test: bool,
}

/// Multi-step guided provider setup: select provider → API key → model → test.
#[derive(Debug, Clone)]
pub enum ProviderWorkflow {
    /// Step 1: user is selecting a provider from the list.
    SelectProvider {
        providers: Vec<String>,
        selected: usize,
    },
    /// Step 2: user is entering an API key for the chosen provider.
    EnterApiKey {
        provider: String,
        key_input: String,
        cursor: usize,
    },
    /// Step 3: user is selecting a model from the provider.
    SelectModel {
        provider: String,
        api_key: String,
        models: Vec<String>,
        selected: usize,
    },
    /// Step 4: testing the connection with the chosen model.
    Testing {
        provider: String,
        api_key: String,
        model: String,
    },
    /// Step 5: showing the result.
    Result {
        provider: String,
        api_key: String,
        model: String,
        ok: bool,
        message: String,
    },
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
            show_settings: false,
            show_shortcuts: false,
            raw_mode: false,
            mouse_x: 0,
            provider_name: String::new(),
            provider_workflow: None,
            pending_setup_models: None,
            pending_setup_test: false,
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
        // Populate dialog lists: /model uses cached models, /provider uses static presets, /theme uses theme names.
        let models = if cmd == "/model" && !self.models_cache.is_empty() {
            self.models_cache.clone()
        } else if cmd == "/provider" {
            crate::provider::preset_names().map(|s| s.to_owned()).collect()
        } else if cmd == "/theme" {
            crate::tui::theme::NAMED_THEMES.iter().map(|t| t.name.to_owned()).collect()
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
                // For list-backed commands, use the highlighted entry.
                if (d.command == "/model" || d.command == "/provider" || d.command == "/theme") && !d.models.is_empty() {
                    let name = d
                        .models
                        .get(d.models_selected)
                        .cloned()
                        .unwrap_or_default();
                    if name.is_empty() {
                        d.command.clone()
                    } else {
                        format!("{} {}", d.command, name)
                    }
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

    /// Starts the multi-step provider setup workflow.
    pub fn start_provider_workflow(&mut self) {
        let providers: Vec<String> = crate::provider::preset_names().map(|s| s.to_owned()).collect();
        self.provider_workflow = Some(ProviderWorkflow::SelectProvider {
            providers,
            selected: 0,
        });
    }

    /// Handles key events during the multi-step provider setup workflow.
    fn handle_workflow_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.provider_workflow = None;
                self.notice("setup cancelled.");
            }
            _ => {
                if let Some(ref mut wf) = self.provider_workflow {
                    match wf {
                        ProviderWorkflow::SelectProvider { providers, selected } => {
                            match key.code {
                                KeyCode::Up | KeyCode::Char('k') => {
                                    *selected = selected.saturating_sub(1);
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    if *selected + 1 < providers.len() {
                                        *selected += 1;
                                    }
                                }
                                KeyCode::Enter => {
                                    let chosen = providers[*selected].clone();
                                    // Skip API key step for local providers.
                                    if chosen == "ollama" {
                                        let wf = std::mem::replace(wf, ProviderWorkflow::SelectProvider { providers: vec![], selected: 0 });
                                        if let ProviderWorkflow::SelectProvider { providers: _, selected: _ } = wf {
                                            // Fetch models for ollama and go to model selection.
                                            self.pending_setup_models = Some((chosen, String::new()));
                                        }
                                    } else {
                                        *wf = ProviderWorkflow::EnterApiKey {
                                            provider: chosen,
                                            key_input: String::new(),
                                            cursor: 0,
                                        };
                                    }
                                }
                                _ => {}
                            }
                        }
                        ProviderWorkflow::EnterApiKey { provider, key_input, cursor } => {
                            match key.code {
                                KeyCode::Char(c) => {
                                    let byte = key_input.char_indices().nth(*cursor).map_or(key_input.len(), |(i, _)| i);
                                    key_input.insert(byte, c);
                                    *cursor += 1;
                                }
                                KeyCode::Backspace if *cursor > 0 => {
                                    let byte = key_input.char_indices().nth(*cursor).map_or(key_input.len(), |(i, _)| i);
                                    let prev = key_input[..byte].char_indices().next_back().map_or(0, |(i, _)| i);
                                    key_input.replace_range(prev..byte, "");
                                    *cursor -= 1;
                                }
                                KeyCode::Left if *cursor > 0 => *cursor -= 1,
                                KeyCode::Right if *cursor < key_input.chars().count() => *cursor += 1,
                                KeyCode::Home => *cursor = 0,
                                KeyCode::End => *cursor = key_input.chars().count(),
                                KeyCode::Enter => {
                                    let key_val = key_input.clone();
                                    let prov = provider.clone();
                                    // Move to model selection.
                                    let wf = std::mem::replace(wf, ProviderWorkflow::SelectProvider { providers: vec![], selected: 0 });
                                    if let ProviderWorkflow::EnterApiKey { .. } = wf {
                                        self.pending_setup_models = Some((prov, key_val));
                                    }
                                }
                                _ => {}
                            }
                        }
                        ProviderWorkflow::SelectModel { provider, api_key, models, selected } => {
                            match key.code {
                                KeyCode::Up | KeyCode::Char('k') => {
                                    *selected = selected.saturating_sub(1);
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    if *selected + 1 < models.len() {
                                        *selected += 1;
                                    }
                                }
                                KeyCode::Enter => {
                                    let model = models[*selected].clone();
                                    let prov = provider.clone();
                                    let key = api_key.clone();
                                    // Move to testing step.
                                    *wf = ProviderWorkflow::Testing {
                                        provider: prov,
                                        api_key: key,
                                        model,
                                    };
                                    // Signal event loop to run the test.
                                    self.pending_setup_test = true;
                                }
                                _ => {}
                            }
                        }
                        ProviderWorkflow::Testing { .. } => {
                            // Allow Esc to cancel a hanging test.
                            if key.code == KeyCode::Esc {
                                let wf = self.provider_workflow.take();
                                if let Some(ProviderWorkflow::Testing { provider, .. }) = wf {
                                    self.notice(format!("setup test cancelled for {provider}."));
                                }
                            }
                            // Otherwise wait for async test to complete.
                        }
                        ProviderWorkflow::Result { .. } => {
                            // Any key closes the result.
                            let result = self.provider_workflow.take();
                            if let Some(ProviderWorkflow::Result { provider, api_key: _, model, ok, message }) = result {
                                if ok {
                                    self.notice(format!("setup complete: {provider} / {model}"));
                                } else {
                                    self.notice(format!("setup failed: {message}"));
                                }
                            }
                        }
                    }
                }
            }
        }
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
                     · Ctrl+L clear · Ctrl+T left tree · Ctrl+P explorer · Ctrl+O open folder · Ctrl+Q quit\n\
                     cmds: /help /clear /tokens /model /history /retry /save /load /theme /provider /models /router /todo /cd\n\
                      Tip: type \"/\" to see the palette — Enter/↑↓/click open the args dialog, Tab completes. Ctrl+O to change folder.",
                );
                return true;
            }
            "/theme" | "/tokens" | "/plan" => {
                // Theme switching, token counts and planning are handled by
                // the unified command dispatcher (commands::dispatch) so the
                // TUI and REPL always agree.
                return false;
            }
            "/setup" => {
                self.start_provider_workflow();
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
            // All other slash commands go through the unified dispatcher
            // (which calls App-aware handlers for `/router`, `/provider`,
            // `/save`, etc.). handle_tui_slash syncs derived state.
            _ => {}
        }        // Known slash commands and custom skills need App-aware handling
        if crate::commands::SLASH_COMMANDS.contains(&cmd_lc.as_str()) {
            return false;
        }

        // Unknown command — queue for handle_tui_slash which checks skills
        false
    }    pub fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Key(key) if key.kind == crossterm::event::KeyEventKind::Press => {
                self.handle_key(key);
            }
            Event::Mouse(me) => {
                // Track mouse X for the focused-pane beam effect.
                self.mouse_x = me.column;
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
        self.mouse_x = me.column;
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
                                    } else if ZERO_ARG_SLASH.contains(&cmd) {
                                        // Run immediately: fill the input
                                        // with the full command and submit
                                        // so `local_command` handles it
                                        // (`/exit` -> quit, `/clear` -> clear,
                                        // `/tokens` -> dispatch + render).
                                        self.input = cmd.to_owned();
                                        self.input_cursor = self.input.chars().count();
                                        self.slash_selected = 0;
                                        self.submit();
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
        // Provider workflow captures all keys when active.
        if self.provider_workflow.is_some() {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
            {
                self.quit = true;
                return;
            }
            self.handle_workflow_key(key);
            return;
        }
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
            if let (KeyModifiers::CONTROL, KeyCode::Char('q')) = (key.modifiers, key.code) { self.quit = true }
            return;
        }

        // Global bindings. Handle case-insensitive for Ctrl.
        // Modals capture Esc first
        if self.show_settings || self.show_shortcuts || self.show_cost_dashboard {
            if key.code == KeyCode::Esc {
                self.show_settings = false;
                self.show_shortcuts = false;
                self.show_cost_dashboard = false;
                return;
            }
            // any other key closes shortcuts/settings as well
            if self.show_shortcuts || self.show_settings {
                self.show_shortcuts = false;
                self.show_settings = false;
                return;
            }
        }
        // Single shortcut entry: "?" shows the shortcuts modal (when not typing)
        if key.code == KeyCode::Char('?') && !key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.focus == Focus::Input && !self.input.is_empty() {
                // let "?" be typed
            } else {
                self.show_shortcuts = !self.show_shortcuts;
                return;
            }
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && let KeyCode::Char(c) = key.code {
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
                    ',' => {
                        self.show_settings = !self.show_settings;
                        return;
                    }
                    'o' => {
                        // Ctrl+O: open folder — change working directory
                        self.open_slash_dialog("/open");
                        return;
                    }
                    'z' => {
                        // Ctrl+Z: zen mode (toggle sidebars) — moved from Ctrl+O
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
                                // Replace the partial input with the full
                                // command so `submit()` -> `local_command`
                                // sees the resolved slash and runs it
                                // immediately (`/exit` -> quit,
                                // `/clear` -> clear, `/tokens` -> dispatch).
                                self.input = cmd.to_owned();
                                self.input_cursor = self.input.chars().count();
                                self.slash_selected = 0;
                                // Fall through to submit() below.
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
                        self.input.push_str(ghost);
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
                        name: name.clone(),
                        args: args.clone(),
                        ok: Some(ok),
                    });
                }
                // Result preview — for shell-like tools use the terminal-styled Shell entry.
                if !snippet.trim().is_empty() {
                    if matches!(name.as_str(), "run_shell" | "run_test" | "check_project") {
                        self.entries.push(ChatEntry::Shell {
                            cmd: args.clone(),
                            output: snippet.chars().take(800).collect(),
                            ok,
                        });
                    } else {
                        let text: String = snippet.chars().take(160).collect();
                        self.entries
                            .push(ChatEntry::Notice(format!("↳ {text}")));
                    }
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
        } else if !leftover.trim().is_empty() {
            // Normal completion but streaming buffer has un-flushed content.
            // This can happen if the model's final text wasn't fully captured
            // by the Answer update. Preserve it to avoid losing output.
            self.entries
                .push(ChatEntry::Assistant(leftover));
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
    // Set initial provider name and remap accent color.
    tui.provider_name = app.config.provider.key().to_string();
    theme::apply_provider_accent(&tui.provider_name);
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
                let provider_id = app.config.provider.key().to_string();
                let api_models = if let Some(url) = app.config.provider.models_url() {
                    match api::list_models(&app.http, &url, app.config.provider.auth().token()).await {
                        Ok(list) => list,
                        Err(e) => {
                            tui.notice(format!("failed to fetch models: {e:#}"));
                            Vec::new()
                        }
                    }
                } else {
                    Vec::new()
                };
                // Merge API models with known static registry.
                let known = crate::provider::known_models(&provider_id);
                let mut models = api_models;
                let api_set: std::collections::HashSet<&str> = models.iter().map(|s| s.as_str()).collect();
                let extras: Vec<String> = known.iter()
                    .filter(|m| !api_set.contains(m.id))
                    .map(|m| m.id.to_owned())
                    .collect();
                models.extend(extras);
                if models.is_empty() {
                    tui.notice(format!(
                        "no models available for '{provider_id}' — try /model <name> manually"
                    ));
                }
                tui.models_cache = models;
                tui.open_slash_dialog("/model");
                continue;
            }
            // Provider setup workflow: fetch models for the chosen provider.
            if let Some((prov, key)) = tui.pending_setup_models.take() {
                let known = crate::provider::known_models(&prov);
                let mut models: Vec<String> = known.iter().map(|m| m.id.to_owned()).collect();
                // Try live API fetch to get real models.
                if let Ok(p) = crate::provider::resolve(&prov, None, None, {
                    let k = key.clone();
                    move |_: &str| Some(k.clone())
                })
                    && let Some(url) = p.models_url()
                        && let Ok(api_models) = api::list_models(&app.http, &url, p.auth().token()).await {
                    let known_set: std::collections::HashSet<&str> = models.iter().map(|s| s.as_str()).collect();
                    let extras: Vec<String> = api_models.into_iter().filter(|m| !known_set.contains(m.as_str())).collect();
                    models.extend(extras);
                        }
                if models.is_empty() {
                    models.push("default".to_owned());
                }
                tui.provider_workflow = Some(ProviderWorkflow::SelectModel {
                    provider: prov,
                    api_key: key,
                    models,
                    selected: 0,
                });
                continue;
            }
            // Provider setup workflow: test the connection by fetching models.
            if tui.pending_setup_test {
                tui.pending_setup_test = false;
                if let Some(ProviderWorkflow::Testing { provider, api_key, model }) = tui.provider_workflow.take() {
                    let test_result = {
                        match crate::provider::resolve(&provider, None, None, {
                            let k = api_key.clone();
                            move |_: &str| Some(k.clone())
                        }) {
                            Ok(p) => {
                                if let Some(url) = p.models_url() {
                                    match crate::api::list_models(&app.http, &url, p.auth().token()).await {
                                        Ok(list) => {
                                            let ok = !list.is_empty();
                                            let msg = if ok {
                                                format!("{} models available — connection verified", list.len())
                                            } else {
                                                "provider returned no models (may still work for direct calls)".to_owned()
                                            };
                                            (ok, msg)
                                        }
                                        Err(e) => (false, format!("{e:#}")),
                                    }
                                } else {
                                    // Provider has no models endpoint — trust the resolve worked.
                                    (true, "provider resolved successfully (no model listing endpoint)".to_owned())
                                }
                            }
                            Err(e) => (false, format!("{e:#}")),
                        }
                    };
                    let (ok, message) = test_result;
                    // Apply provider and model on success.
                    if ok
                        && let Ok(p) = crate::provider::resolve(&provider, None, None, {
                            let k = api_key.clone();
                            move |_: &str| Some(k.clone())
                        }) {
                            app.config.provider = p;
                            app.config.model = model.clone();
                        }
                    tui.provider_workflow = Some(ProviderWorkflow::Result {
                        provider,
                        api_key,
                        model,
                        ok,
                        message,
                    });
                    continue;
                }
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
    // Handles worktrees where .git is a file pointing to the main repo.
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let git_path = cwd.join(".git");
    let dirty = false; // keep cheap; file-tree handles precise git marks

    // In a worktree, .git is a file containing "gitdir: <path>".
    // We need to resolve the actual gitdir to find HEAD.
    let head_path = if git_path.is_file() {
        // Worktree: read the gitdir path from the .git file
        let content = std::fs::read_to_string(&git_path).ok().unwrap_or_default();
        let gitdir = content
            .strip_prefix("gitdir: ")
            .map(str::trim)
            .unwrap_or("");
        if gitdir.is_empty() {
            return (None, dirty);
        }
        // Resolve relative gitdir path against the worktree root
        let resolved = if std::path::Path::new(gitdir).is_absolute() {
            std::path::PathBuf::from(gitdir)
        } else {
            cwd.join(gitdir)
        };
        resolved.join("HEAD")
    } else {
        git_path.join("HEAD")
    };

    let branch = std::fs::read_to_string(&head_path).ok().and_then(|s| {
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
    // Pick the larger of the CLI budget and the model's true context
    // window so the bar always reflects real available room. When the
    // user set a small `context_tokens` in TOML, `budget` is the small
    // value (and the trimmer enforces it) — the bar still shows the
    // model's full limit so they can see the cap.
    let model_context = {
        let from_registry = provider::context_window_for(
            &app.config.provider.key(),
            &app.config.model,
        );
        if from_registry > app.config.context_tokens {
            from_registry
        } else {
            app.config.context_tokens
        }
    };
    draw::StatusInfo {
        provider: app.config.provider.key().to_string(),
        model: app.config.model.to_string(),
        provider_name: tui.provider_name.clone(),
        tokens: app.session.approx_tokens(),
        budget: app.config.context_tokens,
        model_context,
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
        todos: app.todos.clone(),
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
    // Sync provider name and remap accent color on provider switch.
    let new_provider = app.config.provider.key().to_string();
    if tui.provider_name != new_provider {
        tui.provider_name = new_provider;
        theme::apply_provider_accent(&tui.provider_name);
    }
    // Folder change: refresh tree to new cwd and prune pinned files
    let cmd_lc = line.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    if matches!(cmd_lc.as_str(), "/cd" | "/cwd" | "/folder" | "/open") {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        tui.tree = Some(FileTree::open(&cwd));
        tui.pinned_files.retain(|p| p.starts_with(&cwd));
        tui.show_tree = true;
        // Ensure layout shows at least one pane
        if !tui.show_tools {
            // keep todo pane visible
        }
    }
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
            // `/retry` semantics: drop the trailing assistant side so the
            // regenerated answer does not duplicate.
            while tui.entries.last().is_some_and(|e| matches!(e, ChatEntry::Notice(_))) {
                tui.entries.pop();
            }
            // Pop back until just before the last assistant-side block, but keep the User.
            while tui.entries.last().is_some_and(|e| matches!(e, ChatEntry::Assistant(_) | ChatEntry::Tool {..} | ChatEntry::Op(_) | ChatEntry::Shell {..} | ChatEntry::Code {..} | ChatEntry::Checklist {..})) {
                tui.entries.pop();
            }
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
    // Strip trailing notices first.
    while entries.last().is_some_and(|e| matches!(e, ChatEntry::Notice(_))) {
        entries.pop();
    }
    // Find last User and truncate to before it, removing the whole exchange
    // (User + any following assistant/tool/op/shell/code/checklist entries).
    if let Some(pos) = entries.iter().rposition(|e| matches!(e, ChatEntry::User(_))) {
        entries.truncate(pos);
    }
    while entries.last().is_some_and(|e| matches!(e, ChatEntry::Notice(_))) {
        entries.pop();
    }
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
        let cmds: Vec<&str> = crate::commands::SLASH_COMMANDS.to_vec();
        for cmd in cmds {
            let mut tui = Tui::new();
            // use a basic arg where needed to avoid usage notices being mistaken for failure
            let input: String = match cmd {
                "/model" => "/model test-model".to_string(),
                "/save" => "/save test".to_string(),
                "/load" => "/load test".to_string(),
                "/todo" => "/todo list".to_string(),
                other => other.to_string(),
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
            provider_explicit: false,
            shell_tools: Vec::new(),
            theme: None,
            timeout_secs: 30,
            limit_mb: 16,
            protocol: crate::govinda_protocol::ProtocolConfig::default(),
        };
        let mut app = crate::commands::App::new(
            config,
            reqwest::Client::new(),
            crate::session::Session::new("sys"),
            crate::render::Renderer::new(false),
        );
        let cmds: Vec<&str> = crate::commands::SLASH_COMMANDS.to_vec();
        for cmd in cmds {
            let mut tui = Tui::new();
            let line: &str = match cmd {
                "/model" => "/model foo",
                "/save" => "/save t",
                "/load" => "/load t",
                "/todo" => "/todo list",
                other => other,
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
            provider_explicit: false,
            shell_tools: Vec::new(),
            theme: None,
            timeout_secs: 30,
            limit_mb: 16,
            protocol: crate::govinda_protocol::ProtocolConfig::default(),
        };
        App::new(
            config,
            reqwest::Client::new(),
            crate::session::Session::new("sys"),
            crate::render::Renderer::new(false),
        )
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
