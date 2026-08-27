//! Function-calling: the registry of tools the model may invoke, and the
//! built-in implementations executed locally by the REPL.
//!
//! Workspace tools (`read_file`, `write_file`, `list_files`, `grep`) operate
//! strictly inside the process working directory: absolute paths and `..`
//! components are rejected before any I/O, matching the session-path policy.
//!
//! User-defined shell tools (`[[tools]]` in config.toml) spawn external
//! commands — never through a shell, only as direct argv — with mandatory
//! confirmation, a hard timeout, and an output cap.

use crate::api::Tool;
use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
/// Largest file `read_file`/`grep` will open.
const MAX_INPUT_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Largest `read_file` result handed back to the model, in characters.
const MAX_READ_CHARS: usize = 60_000;
/// Default number of lines returned by `read_file`.
const DEFAULT_READ_LINES: usize = 2000;
/// Largest `write_file` payload accepted, in bytes.
const MAX_WRITE_BYTES: usize = 1024 * 1024;
/// Directory entries returned by `list_files` by default / at most.
const DEFAULT_LIST_ENTRIES: usize = 500;
const MAX_LIST_ENTRIES: usize = 2000;
/// Deepest recursion for directory walks.
const MAX_WALK_DEPTH: usize = 12;
/// Matches returned by `grep` by default / at most.
const DEFAULT_GREP_MATCHES: usize = 50;
const MAX_GREP_MATCHES: usize = 200;
/// Directories never descended into during walks.
const SKIP_DIRS: [&str; 3] = [".git", "target", "node_modules"];
/// Shell tools: default / maximum wall-clock time per invocation.
const DEFAULT_SHELL_TIMEOUT_SECS: u64 = 30;
const MAX_SHELL_TIMEOUT_SECS: u64 = 600;
/// Shell tools: default / maximum combined output kept per stream.
const DEFAULT_SHELL_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_SHELL_OUTPUT_BYTES: usize = 1024 * 1024;
/// Largest single argument value accepted from the model, in characters.
const MAX_ARG_VALUE_CHARS: usize = 8 * 1024;
/// Cap on the unified-diff preview embedded in each staged-edit result
/// (the model sees this string too, so it must stay bounded).
const MAX_DIFF_PREVIEW_CHARS: usize = 4_000;
/// Names reserved by built-in implementations; user tools cannot shadow them.
const BUILTIN_TOOL_NAMES: [&str; 67] = [
    "current_time",
    "count_tokens",
    "read_file",
    "write_file",
    "delete_file",
    "move_file",
    "copy_file",
    "list_files",
    "list_directory",
    "grep",
    "scan_project",
    "find_symbol",
    "explain_code",
    "edit_file",
    "insert_after",
    "insert_before",
    "view_diff",
    "apply_edits",
    "discard_edits",
    "show_staged_files",
    "run_shell",
    "run_test",
    "run_diagnostics",
    "open_preview",
    "git_diff",
    "git_log",
    "git_branch",
    "git_commit",
    "web_search",
    "web_fetch",
    "ask_user",
    "delegate_task",
    "show_token_budget",
    "show_capabilities",
    "todo",
    "remember",
    "forget",
    // GOVINDA toolbox — 18 new tools
    "clipboard",
    "template",
    "package_install",
    "lint",
    "format",
    "build_project",
    "http_request",
    "process_manager",
    "env",
    "json_query",
    "regex_search",
    "diff_apply",
    "template_fill",
    "git_diff_apply",
    "scaffold_test",
    "memory_search",
    "screenshot",
    "image_view",
    // GOVINDA toolbox — bulk + parallel + docs + extras
    "bulk_read",
    "bulk_shell",
    "bulk_crud",
    "docs",
    "html",
    "screenrec",
    "format_setter",
    "code_format",
    "git_tools",
    "find_issues",
    "parallel",
    // GOVINDA Protocol v7.0 self-verification tool.
    "quality_gate_check",
];
/// `{placeholder}` tokens inside shell-tool `args_template` words.
#[allow(clippy::expect_used)] // static, hand-checked patterns
fn placeholder_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{([A-Za-z_][A-Za-z0-9_]*)\}").expect("valid regex"))
}
#[allow(clippy::expect_used)] // static, hand-checked pattern
fn tool_name_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-z][a-z0-9_]*$").expect("valid regex"))
}

/// Deserializes a tool call's raw arguments string into a typed struct.
///
/// Every executor should funnel arguments through this so malformed input
/// surfaces as one consistent error instead of ad-hoc parsing per tool.
pub fn parse_args<T: DeserializeOwned>(arguments_json: &str) -> Result<T> {
    serde_json::from_str(arguments_json).context("malformed tool arguments (invalid JSON)")
}

/// Boxed future returned by `ToolExecutor::execute`, so slow tools (shell
/// commands, HTTP lookups) can run concurrently without blocking the REPL.
pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>;

/// Executes tool calls requested by the model.
///
/// Implementations own their tools' JSON-Schema specs and behavior; the agent
/// loop in the REPL only sees names, argument JSON, and result strings.
pub trait ToolExecutor: Send + Sync {
    /// Tools advertised to the model for each turn.
    fn specs(&self) -> Vec<Tool>;

    /// Runs one call asynchronously. `arguments_json` is the raw arguments
    /// object string from the model — malformed input must surface as an
    /// error string, never a panic.
    fn execute<'a>(&'a self, name: &'a str, arguments_json: &'a str) -> ToolFuture<'a>;

    /// Tools that mutate state (workspace writes, shell commands) and must be
    /// confirmed interactively before execution. The REPL prompts y/N and
    /// skips declined calls.
    fn requires_confirmation(&self, name: &str) -> bool {
        let _ = name;
        false
    }
}

/// The default executor: safe local tools plus sandboxed workspace, staged
/// editing, and shell execution tools.
#[derive(Default)]
pub struct BuiltinTools {
    shell_tools: Vec<ShellToolDef>,
    /// Queue of staged edits shared with the REPL (`/diff`, `/apply`,
    /// `/reject`). The executor stages; only `/apply` touches the disk.
    pending: Arc<Mutex<PendingEdits>>,
}

impl BuiltinTools {
    /// Builds an executor over validated user shell-tool definitions.
    /// Validation errors must already have been surfaced at config load;
    /// this constructor trusts its input.
    pub fn new(shell_tools: Vec<ShellToolDef>) -> Self {
        Self {
            shell_tools,
            pending: Arc::default(),
        }
    }

    /// Handle to the shared staged-edit queue for REPL commands.
    pub fn pending_edits(&self) -> Arc<Mutex<PendingEdits>> {
        self.pending.clone()
    }
}

impl ToolExecutor for BuiltinTools {
    fn specs(&self) -> Vec<Tool> {
        let mut specs = vec![
            Tool::new(
                "current_time",
                "Returns the user's current date and time. Use this instead of guessing \
                 dates or seasons. Default format is ISO-8601 in the user's local timezone.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "timezone": {
                            "type": "string",
                            "enum": ["local", "utc"],
                            "description": "Which timezone to use (default \"local\")"
                        },
                        "format": {
                            "type": "string",
                            "enum": ["iso8601", "unix", "human"],
                            "description": "Output format: iso8601 (default), unix (epoch seconds), or human (e.g. \"Wednesday, August 26, 2026 1:07 PM\")"
                        }
                    },
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "count_tokens",
                "Counts the real BPE tokens (cl100k_base) and characters in the given text. \
                 Use this to estimate how much a string will cost in the model's context, \
                 not character count — token-to-char ratios vary 1:3 to 1:6 by language.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "text": {"type": "string", "description": "Text to measure"}
                    },
                    "required": ["text"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "todo",
                "Manage the project's task list. Use this to track work you plan to do, \
                 break a big task into steps, and mark items done as you finish. The list is \
                 stored at <cwd>/.govinda/todos.json and is shared with the `/todo` slash command \
                 and the TODO panel in the TUI. Actions: list | add <text> | done <index> | \
                 undo <index> | rm <index> | clear. Indices are 1-based.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["list", "add", "done", "undo", "rm", "clear"],
                            "description": "What to do with the todo list"
                        },
                        "text": {
                            "type": "string",
                            "description": "Task text (required for action=add)"
                        },
                        "index": {
                            "type": "integer",
                            "description": "1-based index (required for action=done|undo|rm)"
                        }
                    },
                    "required": ["action"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "read_file",
                "Reads a text file from the workspace with line numbers. For source files it \
                 first returns a symbol outline (functions, types, imports) computed from the \
                 whole file, so partial reads stay navigable. Workspace-relative paths only; \
                 absolute paths and '..' are rejected.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Workspace-relative file path"},
                        "offset_line": {"type": "integer", "description": "1-based first line to return (default 1)"},
                        "max_lines": {"type": "integer", "description": "Maximum lines to return (default 2000)"},
                        "include_outline": {"type": "boolean", "description": "Prepend a symbol outline for source files (default true)"}
                    },
                    "required": ["path"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "write_file",
                "Creates or overwrites a workspace file with the given content. Requires user \
                 confirmation before it runs. Workspace-relative paths only.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Workspace-relative file path"},
                        "content": {"type": "string", "description": "Full file content to write"}
                    },
                    "required": ["path", "content"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "delete_file",
                "Removes a workspace file. Faster and quieter than `run_shell rm`. Requires \
                 user confirmation. Workspace-relative paths only; absolute paths and '..' \
                 are rejected. Use with care — this is not undoable.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Workspace-relative file to delete"}
                    },
                    "required": ["path"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "move_file",
                "Renames or moves a workspace file/directory within the workspace. Both source \
                 and destination are workspace-relative. Requires user confirmation. Overwrites \
                 the destination if it exists. Faster and quieter than `run_shell mv`.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "from": {"type": "string", "description": "Source path (workspace-relative)"},
                        "to": {"type": "string", "description": "Destination path (workspace-relative)"}
                    },
                    "required": ["from", "to"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "copy_file",
                "Copies a workspace file within the workspace. Both source and destination are \
                 workspace-relative. Requires user confirmation. Overwrites the destination if \
                 it exists. Faster and quieter than `run_shell cp`.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "from": {"type": "string", "description": "Source path (workspace-relative)"},
                        "to": {"type": "string", "description": "Destination path (workspace-relative)"}
                    },
                    "required": ["from", "to"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "list_files",
                "Lists files and directories under a workspace path (RECURSIVE). Use \
                 `list_directory` for a quick non-recursive `ls` of just one folder. Common \
                 build dirs (.git, target, node_modules) are skipped.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Workspace-relative directory (default '.'); directories get a trailing '/'"},
                        "max_entries": {"type": "integer", "description": "Maximum entries to list (default 500)"},
                        "max_depth": {"type": "integer", "description": "Max recursion depth (default 8, max 12)"}
                    },
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "list_directory",
                "Non-recursive directory listing (like `ls`). Cheap — use this for \"what's in \
                 this directory?\" instead of the recursive list_files. Directories get a \
                 trailing '/'. Hidden files (starting with '.') are included by default.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Workspace-relative directory (default '.')"},
                        "include_hidden": {"type": "boolean", "description": "Include dotfiles (default true)"}
                    },
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "grep",
                "Regex search across text files in the workspace; returns 'path:line: text' \
                 matches. Use `find_symbol` for symbol lookups — it's faster and ranks exact \
                 matches first.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string", "description": "Rust regex pattern"},
                        "path": {"type": "string", "description": "Workspace-relative directory or file to search (default '.')"},
                        "max_matches": {"type": "integer", "description": "Maximum matches to return (default 50)"},
                        "include_globs": {"type": "array", "items": {"type": "string"}, "description": "Restrict search to files matching any of these glob patterns (e.g. [\"*.rs\", \"*.toml\"])"},
                        "case_sensitive": {"type": "boolean", "description": "Case-sensitive search (default true)"}
                    },
                    "required": ["pattern"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "scan_project",
                "Builds a structured overview of the workspace: project types, entry points, \
                 dependencies, file statistics, and git branch/status. Also refreshes the \
                 workspace symbol index used by find_symbol. Read-only.",
                serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "find_symbol",
                "Locates the definition of a function, struct, enum, trait, impl, module, or \
                 macro by name across the indexed codebase. Returns kind, name, file, and line \
                 for each match. Prefer this over grep for symbol lookups.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Symbol name (exact or partial)"},
                        "kind": {"type": "string", "enum": ["function", "struct", "enum", "union", "trait", "module", "macro", "impl", "class", "any"], "description": "Filter by kind (default any)"}
                    },
                    "required": ["name"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "explain_code",
                "Read-only helper for explaining code: returns the source of one symbol \
                 (function/struct/trait…) from a workspace file, or an outline plus the head \
                 of the whole file when no symbol is named. Pair the result with your own \
                 explanation.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Workspace-relative file path"},
                        "symbol": {"type": "string", "description": "Optional symbol name to extract; omit for a file overview"},
                        "max_lines": {"type": "integer", "description": "Maximum lines to return (default 120)"}
                    },
                    "required": ["path"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "edit_file",
                "Replaces an exact string in an existing workspace file. The edit is STAGED, \
                 not written: the user reviews it via view_diff and commits with /apply. \
                 Fails unless the search string occurs exactly once — include enough \
                 surrounding lines (with their indentation) to make it unique.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Workspace-relative file path"},
                        "old_string": {"type": "string", "description": "Exact text to replace (including indentation)"},
                        "new_string": {"type": "string", "description": "Replacement text"}
                    },
                    "required": ["path", "old_string", "new_string"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "insert_after",
                "Stages new text to be inserted after the given 1-based line of a workspace \
                 file (for adding functions, imports, fields…). Committed only via /apply.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Workspace-relative file path"},
                        "line": {"type": "integer", "description": "1-based line number to insert after"},
                        "text": {"type": "string", "description": "Text block to insert"}
                    },
                    "required": ["path", "line", "text"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "insert_before",
                "Stages new text to be inserted before the given 1-based line of a workspace \
                 file. Committed only via /apply.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Workspace-relative file path"},
                        "line": {"type": "integer", "description": "1-based line number to insert before"},
                        "text": {"type": "string", "description": "Text block to insert"}
                    },
                    "required": ["path", "line", "text"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "view_diff",
                "Returns the unified diff of all staged (not yet applied) edits. Use this \
                 after a batch of edits to review what would change before committing. \
                 Pass `file` to scope to one file. Read-only.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file": {"type": "string", "description": "Optional workspace-relative path to scope the diff to one file"}
                    },
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "apply_edits",
                "Commits ALL currently-staged edits to disk in one atomic batch (equivalent to \
                 `/apply`). Use after staging several edits and confirming via view_diff. \
                 Requires user confirmation. Returns a summary of files changed.",
                serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "discard_edits",
                "Discards ALL staged edits without writing them (equivalent to `/reject`). \
                 Use to back out of a batch of edits you no longer want. No confirmation \
                 needed — it's pure local cleanup.",
                serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "show_staged_files",
                "Lists files with pending staged edits, with edit counts per file. Cheaper \
                 than view_diff when you just need an overview (e.g. \"did my last batch \
                 touch 3 files or 10?\").",
                serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "run_shell",
                "Run ANY shell command in the project directory — build, test, lint, git, \
                 package managers, start dev servers (npm run dev, python -m http.server), \
                 open files or URLs in the browser (start on Windows, xdg-open on Linux), \
                 launch apps. Never claim you cannot open a browser or run a program — you \
                 can, via this tool. Requires user confirmation; output is capped and a \
                 timeout applies.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": "The shell command to run"},
                        "timeout_secs": {"type": "integer", "description": "Wall-clock cap in seconds (default 60, max 600)"}
                    },
                    "required": ["command"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "open_preview",
                "Serve the workspace over a local HTTP server and open a file in the user's \
                 browser — the right way to show an HTML/CSS/JS preview after creating or \
                 editing web files. Relative assets resolve against the workspace root. \
                 Requires user confirmation (it starts a server).",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Workspace-relative file to open, e.g. index.html (default index.html)"}
                    },
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "run_test",
                "Runs the workspace's test suite with an optional name filter, using the \
                 detected project type (Rust → cargo test, Python → pytest, JS → npm test). \
                 Requires user confirmation.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "filter": {"type": "string", "description": "Optional substring filter passed to the test runner"},
                        "timeout_secs": {"type": "integer", "description": "Wall-clock cap in seconds (default 120, max 600)"}
                    },
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "run_diagnostics",
                "Runs compile/lint validation for the detected project type (Rust → cargo \
                 check, TypeScript → tsc --noEmit, Python → mypy) so errors can be fixed \
                 immediately. Pass `path` to scope to a single file or directory. Returns \
                 structured error/warning messages. Read-only in effect; requires \
                 confirmation because it executes build tools.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Optional workspace-relative file or directory to scope the check to"},
                        "format": {
                            "type": "string",
                            "enum": ["errors_only", "full", "summary"],
                            "description": "Output verbosity: errors_only (default, just diagnostics), full (all compiler output), summary (one-line per file with error count)"
                        }
                    },
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "git_diff",
                "Shows uncommitted changes (working tree + staged) as a unified diff against \
                 HEAD, so you can review edits before proposing a commit. Pass `file` to scope \
                 to one file. Read-only.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file": {"type": "string", "description": "Optional workspace-relative path to scope the diff to one file"}
                    },
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "git_log",
                "Shows recent commit history for context. Use `path` to filter to a file or \
                 directory, `author` to filter by author substring, `branch` to view a \
                 different branch, and `format` to control verbosity. Read-only.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "max_commits": {"type": "integer", "description": "How many commits to show (default 20, max 200)"},
                        "path": {"type": "string", "description": "Optional workspace-relative path to filter to a file or directory"},
                        "author": {"type": "string", "description": "Optional substring to match against the commit author name/email"},
                        "branch": {"type": "string", "description": "Optional branch to view (default current branch)"},
                        "format": {
                            "type": "string",
                            "enum": ["oneline", "short", "full", "stat"],
                            "description": "Output format: oneline (default, hash + subject), short (hash + author + subject), full (full body), stat (files changed per commit)"
                        }
                    },
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "git_branch",
                "Creates or switches git branches, or lists them. create and switch require \
                 user confirmation; list is read-only.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {"type": "string", "enum": ["list", "create", "switch"], "description": "Branch operation"},
                        "name": {"type": "string", "description": "Branch name (required for create/switch)"}
                    },
                    "required": ["action"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "git_commit",
                "Stages all workspace changes (git add -A) and commits with the given message. \
                 Requires user confirmation — always propose the message in prose first and \
                 let the user adjust it.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "message": {"type": "string", "description": "Commit message (concise imperative summary)"},
                        "stage_all": {"type": "boolean", "description": "Stage every change first (default true); set false to commit only what is already staged"}
                    },
                    "required": ["message"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "web_search",
                "Search the web using a search engine API. Returns search results with titles, \
                 URLs, and snippets. Use this to find current information, documentation, \
                 or verify facts. Pass `site` to restrict to a single domain (e.g. \
                 \"docs.rs\" or \"stackoverflow.com\").",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search query string"},
                        "max_results": {"type": "integer", "description": "Maximum results to return (default 5, max 20)"},
                        "site": {"type": "string", "description": "Optional domain to restrict results to (e.g. \"docs.rs\")"}
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "web_fetch",
                "Fetch the content of a web page by URL. Returns the readable text content \
                 of the page (stripped of HTML, scripts, and navigation). Follows HTTPS \
                 redirects, sets a desktop User-Agent, and times out at 15 seconds by default. \
                 Use `format` to get markdown or raw HTML instead of plain text. Use for \
                 reading documentation, articles, or API responses.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": {"type": "string", "description": "URL to fetch (http:// or https://)"},
                        "max_chars": {"type": "integer", "description": "Maximum characters to return (default 20000, max 100000)"},
                        "format": {
                            "type": "string",
                            "enum": ["text", "markdown", "html"],
                            "description": "Output format: text (default, stripped to readable prose), markdown (preserves structure as MD), html (raw HTML)"
                        },
                        "timeout_secs": {"type": "integer", "description": "Wall-clock cap in seconds (default 15, max 60)"}
                    },
                    "required": ["url"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "ask_user",
                "Ask the user a clarifying question before proceeding. The question is \
                 displayed as an input gate that pauses the agent turn. Use this when \
                 you need user input, clarification, or a decision that only the user \
                 can make. Provide `options` for multiple-choice questions. The user's \
                 response is returned as the tool result.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "question": {"type": "string", "description": "The question to ask the user"},
                        "options": {"type": "array", "items": {"type": "string"}, "description": "Optional list of suggested answer options (shown as buttons)"},
                        "multi_select": {"type": "boolean", "description": "Allow multiple options to be selected (default false)"}
                    },
                    "required": ["question"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "delegate_task",
                "Spawn a background subagent to investigate something in parallel. The \
                 subagent has READ-ONLY tools (read_file, grep, find_symbol, scan_project, \
                 web_search, web_fetch) and runs for up to 60 seconds. It cannot modify files. \
                 Use for: comparing implementation alternatives, exploring unfamiliar \
                 codebases, or independent verification of a hypothesis. Returns the \
                 subagent's final summary as a string.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "task": {"type": "string", "description": "The task to delegate — be specific about what to investigate and what to return"},
                        "context": {"type": "string", "description": "Optional context to seed the subagent (relevant files, prior findings, constraints)"},
                        "timeout_secs": {"type": "integer", "description": "Wall-clock cap (default 60, max 300)"}
                    },
                    "required": ["task"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "show_token_budget",
                "Returns the current token usage, context budget, model limit, and headroom. \
                 Use this to decide whether you can fit more context or need to compact the \
                 session. Output: used_tokens, budget_tokens, model_limit_tokens, \
                 headroom_tokens, percent_used.",
                serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "show_capabilities",
                "Returns the list of enabled tools, current working directory, model name, \
                 provider, and project type. Use this when context is stale or you've lost \
                 track of your environment (e.g. after a long session).",
                serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "remember",
                "Saves a note to the project's persistent memory (.govinda/memory.md). Use \
                 this to record non-obvious facts about the codebase that future sessions \
                 should know (architecture decisions, gotchas, user preferences, recurring \
                 patterns). Notes survive across sessions and projects.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "note": {"type": "string", "description": "The fact to remember (be concise and specific)"}
                    },
                    "required": ["note"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "forget",
                "Removes a note from the project's memory. Use when a previously remembered \
                 fact is no longer true (e.g. a workaround is obsolete). The note must \
                 match exactly (case-insensitive substring match).",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "note": {"type": "string", "description": "Substring of the note to remove (case-insensitive match)"}}
                    ,
                    "required": ["note"],
                    "additionalProperties": false
                }),
            ),
            // -------------------------------------------------------------------
            // GOVINDA toolbox — 18 new high-leverage tools
            // -------------------------------------------------------------------
            Tool::new(
                "clipboard",
                "Internal copy/paste/cut for project data. Lets you hold intermediate text \
                 state (e.g. the body of a function before refactoring) without re-reading the \
                 file. The clipboard is in-memory and process-scoped. Use action=view to \
                 inspect current contents. Pass `line_start`/`line_end` to scope copy/cut to a \
                 line range.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["copy", "paste", "cut", "view"],
                            "description": "What to do with the clipboard"
                        },
                        "source_path": {
                            "type": "string",
                            "description": "File path (workspace-relative) for copy/cut/paste"
                        },
                        "line_start": {"type": "integer", "description": "1-based start line (default 1)"},
                        "line_end": {"type": "integer", "description": "1-based end line inclusive (default last)"},
                        "text": {
                            "type": "string",
                            "description": "Inline text to paste (alternative to source_path for paste)"
                        }
                    },
                    "required": ["action"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "template",
                "Generate a complete, runnable project scaffold. One call replaces 20+ tool \
                 calls to lay down boilerplate. Supports: web (nextjs/vite), app (tauri), bot \
                 (discord.py), extension (chrome), ai-ml (pytorch), cli (rust/go/node), tui \
                 (ratatui/bubbletea), api (axum). The destination path must not already exist. \
                 Requires user confirmation.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["web", "app", "bot", "extension", "ai-ml", "cli", "tui", "api"]
                        },
                        "framework": {
                            "type": "string",
                            "description": "Framework name (nextjs, vite, tauri, electron, discord, ratatui, bubbletea, axum, pytorch, chrome, rust, go, node). Defaults to a sensible pick."
                        },
                        "name": {"type": "string", "description": "Project name (used for directory, package name, etc.)"},
                        "path": {"type": "string", "description": "Workspace-relative path where the project is created (must not already exist)"}
                    },
                    "required": ["kind", "name", "path"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "package_install",
                "Add a dependency to the project's manifest (cargo add, npm install, pip \
                 install, go get). Detects the project kind automatically. Returns structured \
                 info on success/failure. Requires user confirmation. Cheaper and more reliable \
                 than calling `run_shell cargo add`.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "package": {"type": "string", "description": "Package name (e.g. serde, react, requests)"},
                        "dev": {"type": "boolean", "description": "Dev dependency (default false)"},
                        "version": {"type": "string", "description": "Exact version constraint (e.g. 1.0, ^2.0)"}
                    },
                    "required": ["package"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "lint",
                "Run the project's linter (clippy, eslint, ruff, golangci-lint) with \
                 structured output. Distinguishes errors from warnings. Pass `fix: true` to \
                 apply auto-fixes. Read-only when fix=false.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Scope to a file or directory"},
                        "fix": {"type": "boolean", "description": "Apply auto-fixes (default false)"}
                    },
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "format",
                "Run the project's formatter (rustfmt, prettier, black, gofmt). Pass \
                 `check: true` for check-only mode (default false = writes the file). Requires \
                 user confirmation when writing files.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Scope to a file or directory"},
                        "check": {"type": "boolean", "description": "Check-only mode (default false)"}
                    },
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "build_project",
                "Run the full build pipeline (cargo build, npm run build, go build) and return \
                 structured errors with file/line locations. Distinct from `run_diagnostics` in \
                 that it runs the full pipeline (linking, bundling, type-checking) rather than \
                 just checking syntax. Will retry up to `max_retries` times. Requires user \
                 confirmation.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "release": {"type": "boolean", "description": "Build in release mode (default false)"},
                        "target": {"type": "string", "description": "Target triple (Rust only, e.g. x86_64-pc-windows-msvc)"},
                        "max_retries": {"type": "integer", "description": "Maximum retries 0-5 (default 2)"}
                    },
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "http_request",
                "Structured HTTP caller. Like curl but returns parsed JSON, status code, and \
                 headers as a single object. Supports GET/POST/PUT/DELETE/PATCH. Times out at \
                 30 seconds by default (max 120). Use instead of `run_shell curl` — much more \
                 reliable for JSON APIs.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "method": {"type": "string", "enum": ["GET", "POST", "PUT", "DELETE", "PATCH"]},
                        "url": {"type": "string", "description": "URL (http:// or https://)"},
                        "headers": {"type": "object", "additionalProperties": {"type": "string"}},
                        "body": {"type": "string", "description": "Request body"},
                        "timeout_secs": {"type": "integer", "description": "Wall-clock cap (default 30, max 120)"}
                    },
                    "required": ["method", "url"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "process_manager",
                "Start, stop, list, tail, or check status of background processes (dev servers, \
                 watchers, daemons). Returns a short handle ID (e.g. 'p1') for later queries. \
                 Use this instead of `run_shell` for long-running processes — `run_shell` blocks \
                 for the full timeout. Requires user confirmation for start/stop.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {"type": "string", "enum": ["start", "stop", "list", "tail", "status"]},
                        "command": {"type": "string", "description": "Command to start (for action=start)"},
                        "handle_id": {"type": "string", "description": "Process handle (for stop/tail/status)"},
                        "tail_lines": {"type": "integer", "description": "Lines to tail (default 50)"}
                    },
                    "required": ["action"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "env",
                "Read or list environment variables. Secrets (KEY, TOKEN, SECRET, PASSWORD) \
                 are masked automatically. Use to check `GOVINDA_*` variables and to read \
                 `.env` values without using `run_shell printenv`.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {"type": "string", "enum": ["get", "set", "list", "unset"]},
                        "key": {"type": "string", "description": "Variable name (for get/set/unset)"},
                        "value": {"type": "string", "description": "Value to set (for action=set)"}
                    },
                    "required": ["action"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "json_query",
                "Extract fields from a JSON file using a dot-path query. Supports: `a.b.c`, \
                 `a[0].name`, `a[*].id` (all elements in an array). Drastically reduces token \
                 usage when working with large JSON. Pass `source: \"raw\"` and the JSON in \
                 `query` for inline queries.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "source": {
                            "type": "string",
                            "description": "File path, or 'raw' to treat the query argument as inline JSON"
                        },
                        "query": {
                            "type": "string",
                            "description": "Dot-path query (e.g. 'data.users[0].name' or 'items[*].id')"
                        }
                    },
                    "required": ["source", "query"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "regex_search",
                "Regex search with named-group capture and JSON output. Like `grep` but returns \
                 structured `{file, line, match, group1, group2, ...}` so you don't have to \
                 parse raw grep output. Supports named groups (?P<name>...).",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string", "description": "Regex pattern (Rust syntax; supports named groups)"},
                        "path": {"type": "string", "description": "File or directory to search (default '.')"},
                        "include_globs": {"type": "array", "items": {"type": "string"}},
                        "case_sensitive": {"type": "boolean", "description": "Case-sensitive (default true)"},
                        "max_matches": {"type": "integer", "description": "Max matches to return (default 100)"}
                    },
                    "required": ["pattern"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "diff_apply",
                "Apply a unified diff to one or more files. Handles context shifts, multi-file \
                 patches, and new file creation. Useful for large refactors where edit_file is \
                 too granular. Pass `apply_untracked: false` to refuse creating new files.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "patch": {"type": "string", "description": "Unified diff content (with --- and +++ headers)"},
                        "apply_untracked": {"type": "boolean", "description": "Allow creating files for +++ b/path lines (default true)"}
                    },
                    "required": ["patch"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "template_fill",
                "Fill `{{var}}` placeholders in a template file. Reuse templates across \
                 projects (config files, README scaffolds, CI configs). Reports any unfilled \
                 placeholders so you can fix the input. Requires user confirmation.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "template_path": {"type": "string", "description": "Template file path (workspace-relative)"},
                        "variables": {"type": "object", "additionalProperties": {"type": "string"}},
                        "output_path": {"type": "string", "description": "Where to write the rendered output"}
                    },
                    "required": ["template_path", "variables", "output_path"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "git_diff_apply",
                "Fetch a patch from a URL and apply it to the workspace. Useful for grabbing PR \
                 diffs, Stack Overflow snippets, or any web-hosted patch. Requires user \
                 confirmation. Only allows http(s) URLs to mitigate SSRF.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": {"type": "string", "description": "URL of a patch (text/plain, .diff, or .patch)"},
                        "base_sha": {"type": "string", "description": "Base SHA for PR-style diffs (informational only)"}
                    },
                    "required": ["url"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "scaffold_test",
                "Generate a starter test file for a function. One call produces a runnable \
                 test stub with imports, mocks, and assertion scaffolding. You then refine \
                 the generated tests. Detects project kind (Rust/Node/Python/Go) and writes \
                 to the right test directory. Requires user confirmation.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "source_path": {"type": "string", "description": "Source file containing the function"},
                        "function_name": {"type": "string", "description": "Function to test"},
                        "framework": {"type": "string", "description": "Test framework override (auto-detect by default)"}
                    },
                    "required": ["source_path", "function_name"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "memory_search",
                "Search `.govinda/memory.md` for prior project knowledge (decisions, gotchas, \
                 user preferences). Surfaces context you would otherwise miss in long \
                 sessions or after a `/clear`. Returns scored, token-overlapping sections.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Natural-language question or topic"},
                        "max_results": {"type": "integer", "description": "Max results to return (default 5)"}
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "screenshot",
                "Capture a screenshot of a URL (headless Chrome/Edge) or a running local dev \
                 server (pass `local:PORT`). Writes a PNG to `.govinda/screenshots/`. Only \
                 allows http(s) URLs to mitigate SSRF.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": {"type": "string", "description": "URL (https://...) or 'local:PORT'"},
                        "full_page": {"type": "boolean", "description": "Full page vs viewport (default false)"},
                        "selector": {"type": "string", "description": "CSS selector to scope the screenshot to a specific element"}
                    },
                    "required": ["target"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "image_view",
                "Attach an image to the next model call. Use for visual debugging of UIs, \
                 checking screenshots, or processing diagrams. Supports PNG, JPEG, GIF, WebP, \
                 BMP. Only works with vision-capable models (GPT-4o, Claude, Gemini, etc.).",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Local file path to an image (PNG, JPEG, GIF, WebP, BMP)"},
                        "prompt": {"type": "string", "description": "Hint to the model about what to look for"}
                    },
                    "required": ["path"],
                    "additionalProperties": false
                }),
            ),
            // -------------------------------------------------------------------
            // GOVINDA toolbox — bulk + parallel + docs + extras (12 new)
            // -------------------------------------------------------------------
            Tool::new(
                "bulk_read",
                "Read multiple files in one call. Cheaper than calling read_file N times — \
                 saves round-trips when gathering context across files. Returns structured JSON \
                 with per-file results (ok, bytes, content, error).",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "paths": {"type": "array", "items": {"type": "string"}, "description": "List of workspace-relative paths"},
                        "line_numbers": {"type": "boolean", "description": "Include line numbers (default true)"},
                        "max_bytes": {"type": "integer", "description": "Max bytes per file (default 50000, 0 = unlimited)"}
                    },
                    "required": ["paths"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "bulk_shell",
                "Run multiple shell commands in one call. Set parallel: true to run them \
                 concurrently (e.g. for a build+test+lint pipeline). Returns per-command \
                 status, exit code, elapsed_ms, stdout, stderr. stop_on_error stops at the first failure.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "commands": {"type": "array", "items": {"type": "object", "properties": {
                            "name": {"type": "string"},
                            "command": {"type": "string"},
                            "timeout_secs": {"type": "integer"}
                        }}, "description": "List of commands to run"},
                        "parallel": {"type": "boolean", "description": "Run commands concurrently (default false)"},
                        "stop_on_error": {"type": "boolean", "description": "Stop at first failure (default true)"}
                    },
                    "required": ["commands"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "bulk_crud",
                "Bulk read/write/delete/move/copy on many files in one call. Each operation \
                 is a discriminated union item with action: read|write|delete|move|copy. \
                 Parallel mode is safe for reads, sequential for writes (default).",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operations": {"type": "array", "items": {"type": "object", "properties": {
                            "action": {"type": "string", "enum": ["read", "write", "delete", "move", "copy"]},
                            "path": {"type": "string"},
                            "content": {"type": "string"},
                            "from": {"type": "string"},
                            "to": {"type": "string"}
                        }}, "description": "List of operations"},
                        "parallel": {"type": "boolean", "description": "Run operations concurrently (default false)"},
                        "stop_on_error": {"type": "boolean", "description": "Stop at first failure (default true)"},
                        "max_bytes": {"type": "integer", "description": "Max bytes per file when reading (default 50000)"}
                    },
                    "required": ["operations"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "docs",
                "Generate a project documentation file (README, CHANGELOG, API stub, \
                 CONTRIBUTING) using sensible defaults. Set force: true to overwrite. \
                 Detects project kind to customize the generated text.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "kind": {"type": "string", "enum": ["readme", "changelog", "api", "contributing"]},
                        "output_path": {"type": "string", "description": "Workspace-relative path to write"},
                        "name": {"type": "string", "description": "Project name (for headers)"},
                        "description": {"type": "string", "description": "Short tagline for README"},
                        "force": {"type": "boolean", "description": "Overwrite existing file (default false)"}
                    },
                    "required": ["kind", "output_path"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "html",
                "Render HTML: convert Markdown to HTML, wrap Markdown in a styled HTML document, \
                 or open an existing HTML file in the system browser.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {"type": "string", "enum": ["md_to_html", "wrap_html", "open_html"]},
                        "text": {"type": "string", "description": "Source Markdown (for md_to_html / wrap_html)"},
                        "source": {"type": "string", "description": "File path (for open_html)"},
                        "output_path": {"type": "string", "description": "Output path (for md_to_html / wrap_html)"},
                        "title": {"type": "string", "description": "Page title (for wrap_html; default 'Document')"}
                    },
                    "required": ["action"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "screenrec",
                "Record a video of a running app or browser interaction. Requires a headless \
                 browser (Chrome/Edge/Chromium) installed. The current build captures periodic \
                 screenshots; full video capture requires a browser with video-recording extensions \
                 (e.g. playwright with --record-video).",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": {"type": "string", "description": "URL or 'local:port'"},
                        "duration_secs": {"type": "integer", "description": "Recording duration (default 10, max 60)"},
                        "fps": {"type": "integer", "description": "Frames per second (default 10, max 30)"},
                        "output_path": {"type": "string", "description": "Where to write the video"}
                    },
                    "required": ["target", "output_path"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "format_setter",
                "Bulk-convert file formats: normalize line endings to CRLF/LF/CR, or uppercase \
                 filenames. Per-file to: target. Useful when cross-platform contributors produce \
                 inconsistent line endings.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "files": {"type": "array", "items": {"type": "object", "properties": {
                            "path": {"type": "string"},
                            "to": {"type": "string", "enum": ["crlf", "lf", "cr", "upper"]}
                        }}}
                    },
                    "required": ["files"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "code_format",
                "Normalize source-code formatting per language: enforce LF or CRLF line endings, \
                 strip trailing whitespace, ensure final newline, and convert between spaces and \
                 tabs (2/4-space indent).",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "files": {"type": "array", "items": {"type": "object", "properties": {
                            "path": {"type": "string"},
                            "style": {"type": "string", "enum": ["unix", "windows", "two_space", "four_space", "tabs"]}
                        }}}
                    },
                    "required": ["files"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "git_tools",
                "Extended git operations beyond the basic ones: stash (push/pop/list/drop), \
                 create-branch, checkout, rebase, merge, remote add, tag. Wraps the underlying \
                 git CLI; requires a working git installation.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {"type": "string", "description": "Action kind: stash|create_branch|checkout|rebase|merge|remote_add|tag"}
                    },
                    "required": ["action"],
                    "additionalProperties": true
                }),
            ),
            Tool::new(
                "find_issues",
                "Find code-quality issues in a workspace: duplicate blocks (sliding window), \
                 possibly-dead `pub fn` items, unused imports (Rust heuristic), and unimplemented \
                 functions (todo!()/unimplemented!() bodies). Returns structured findings with file, \
                 line, and a brief description.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "issue": {"type": "string", "enum": ["duplicates", "deadcode", "unused_imports", "unused_code", "all"]},
                        "path": {"type": "string", "description": "Scope directory (default '.')"},
                        "min_lines": {"type": "integer", "description": "Min lines for duplicate detection (default 5)"}
                    },
                    "required": ["issue"],
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "parallel",
                "Run multiple tool invocations concurrently. Set max_concurrency to cap \
                 parallelism (default 4, max 16). With wait: false the calls fire-and-forget \
                 and return immediately. Each result is a structured record with tool name, ok \
                 status, and either the result string or the error.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "calls": {"type": "array", "items": {"type": "object", "properties": {
                            "tool": {"type": "string", "description": "Tool name to invoke"},
                            "args": {"type": "string", "description": "JSON-encoded arguments string"}
                        }}},
                        "wait": {"type": "boolean", "description": "Wait for completion (default true)"},
                        "max_concurrency": {"type": "integer", "description": "Max parallel calls (default 4, max 16)"}
                    },
                    "required": ["calls"],
                    "additionalProperties": false
                }),
            ),
            // -------------------------------------------------------------------
            // GOVINDA Protocol v7.0 self-verification.
            // The model calls this BEFORE claiming completion. It runs locally,
            // has no side effects, and returns a structured PASS / FAIL with
            // specific violations when the protocol is in enforcement mode.
            // -------------------------------------------------------------------
            Tool::new(
                "quality_gate_check",
                "GOVINDA Protocol self-verification. Call this BEFORE claiming the task is \
                 done. Validates the current turn against the protocol's quality gates: \
                 no_emoji characters, tests included, error handling, loading states, \
                 type safety, documentation, Docker, CI/CD. Returns PASS when every \
                 reported check is true AND line_count_estimate meets the project minimum \
                 (only hard-fails at phase=FINAL_VALIDATION; earlier phases warn). When \
                 the emoji scan is enabled, the tool also reads the listed files and \
                 flags any Unicode emoji found in the produced code.",
                crate::govinda_protocol::quality_gate_schema(),
            ),
        ];
        for def in &self.shell_tools {
            specs.push(Tool::new(
                def.name.clone(),
                def.description.clone(),
                shell_tool_schema(def),
            ));
        }
        specs
    }

    fn requires_confirmation(&self, name: &str) -> bool {
        matches!(
            name,
            "write_file" | "delete_file" | "move_file" | "copy_file"
            | "run_shell" | "run_test" | "run_diagnostics"
            | "git_commit" | "git_branch" | "open_preview"
            | "apply_edits" | "forget"
            // Toolbox tools that mutate state on disk
            | "template" | "package_install" | "format" | "build_project"
            | "process_manager" | "template_fill" | "git_diff_apply"
            | "scaffold_test"
            | "bulk_crud" | "format_setter" | "git_tools"
        ) || self.shell_tools.iter().any(|t| t.name == name)
    }

    fn execute<'a>(&'a self, name: &'a str, arguments_json: &'a str) -> ToolFuture<'a> {
        Box::pin(async move {
            match name {
                "current_time" => {
                    let args: CurrentTimeArgs = parse_args(arguments_json)?;
                    Ok(current_time(&args))
                }
                "count_tokens" => {
                    let args: CountTokensArgs = parse_args(arguments_json)?;
                    Ok(format!(
                        "{{\"tokens\":{},\"characters\":{}}}",
                        crate::tokens::count(&args.text),
                        args.text.chars().count()
                    ))
                }
                "todo" => {
                    // The todo list lives in the commands::todo module; we delegate to it
                    // by re-issuing the equivalent `/todo` slash command through the
                    // shared capture path. This keeps the on-disk format in one place
                    // and the TUI's panel updates the same way it does for slash input.
                    #[derive(serde::Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct TodoArgs {
                        action: String,
                        text: Option<String>,
                        index: Option<usize>,
                    }
                    let args: TodoArgs = parse_args(arguments_json)?;
                    let mut line = String::from("/todo ");
                    line.push_str(args.action.as_str());
                    if let Some(t) = args.text.as_deref() {
                        if !t.is_empty() {
                            line.push(' ');
                            line.push_str(t);
                        }
                    }
                    if let Some(i) = args.index {
                        line.push(' ');
                        line.push_str(&i.to_string());
                    }
                    // Run through the REPL dispatcher; capture prints via the
                    // shared output helpers so the result is a plain string the
                    // model can read.
                    let mut app = crate::commands::todo_app_for_tools();
                    let out = crate::commands::dispatch_structured(&line, &mut app).await;
                    if out.msgs.is_empty() {
                        Ok("(no change)".to_string())
                    } else {
                        Ok(out
                            .msgs
                            .into_iter()
                            .map(|m| m.text)
                            .collect::<Vec<_>>()
                            .join("\n"))
                    }
                }
                "read_file" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: ReadFileArgs = parse_args(arguments_json)?;
                    read_file(&cwd, &args)
                }
                "write_file" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: WriteFileArgs = parse_args(arguments_json)?;
                    write_file(&cwd, &args)
                }
                "delete_file" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: DeleteFileArgs = parse_args(arguments_json)?;
                    delete_file(&cwd, &args)
                }
                "move_file" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: MoveFileArgs = parse_args(arguments_json)?;
                    move_file(&cwd, &args)
                }
                "copy_file" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: CopyFileArgs = parse_args(arguments_json)?;
                    copy_file(&cwd, &args)
                }
                "list_files" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: ListFilesArgs = parse_args(arguments_json)?;
                    list_files(&cwd, &args)
                }
                "list_directory" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: ListDirectoryArgs = parse_args(arguments_json)?;
                    list_directory(&cwd, &args)
                }
                "grep" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: GrepArgs = parse_args(arguments_json)?;
                    grep(&cwd, &args)
                }
                "scan_project" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    crate::symbols::rebuild(&cwd);
                    Ok(crate::scan::scan(&cwd).await)
                }
                "find_symbol" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: FindSymbolArgs = parse_args(arguments_json)?;
                    let index = crate::symbols::ensure(&cwd);
                    let hits = index.find(&args.name, args.kind.as_deref());
                    if hits.is_empty() {
                        Ok(format!(
                            "{{\"matches\":0,\"note\":\"no symbols match '{}' — run scan_project to refresh the index\"}}",
                            args.name
                        ))
                    } else {
                        Ok(crate::symbols::results_json(&hits))
                    }
                }
                "explain_code" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: ExplainCodeArgs = parse_args(arguments_json)?;
                    explain_code(&cwd, &args)
                }
                "edit_file" | "insert_after" | "insert_before" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let op = parse_edit_op(name, arguments_json)?;
                    validate_staged_op(&cwd, &op)?;
                    let summary = op.describe();
                    let path = op.path().to_owned();
                    // Per-op unified diff for the terminal: the REPL renders
                    // it in color as soon as the edit is staged (Phase 6.1).
                    let diff_preview = staged_diff(&cwd, std::slice::from_ref(&op))
                        .map(|d| truncate_chars(&d, MAX_DIFF_PREVIEW_CHARS))
                        .unwrap_or_default();
                    let count = {
                        let mut pending = self
                            .pending
                            .lock()
                            .map_err(|_| anyhow::anyhow!("staged-edit queue poisoned"))?;
                        pending.push(op);
                        pending.ops().len()
                    };
                    Ok(serde_json::json!({
                        "staged": true,
                        "path": path,
                        "edit": summary,
                        "pending_edits": count,
                        "diff": diff_preview,
                        "note": "Edit staged. The user reviews with view_diff or /diff and commits with /apply."
                    })
                    .to_string())
                }
                "view_diff" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: ViewDiffArgs = parse_args(arguments_json)?;
                    let pending = self
                        .pending
                        .lock()
                        .map_err(|_| anyhow::anyhow!("staged-edit queue poisoned"))?;
                    if pending.ops().is_empty() {
                        return Ok("no staged edits — nothing to diff".to_owned());
                    }
                    let ops_owned: Vec<EditOp> = match args.file.as_deref() {
                        Some(f) => pending
                            .ops()
                            .iter()
                            .filter(|op| op.path() == f)
                            .cloned()
                            .collect(),
                        None => pending.ops().to_vec(),
                    };
                    if ops_owned.is_empty() {
                        return Ok(format!(
                            "no staged edits for '{}' — nothing to diff",
                            args.file.as_deref().unwrap_or("?")
                        ));
                    }
                    let diff = staged_diff(&cwd, &ops_owned)?;
                    if diff.is_empty() {
                        return Ok("no staged edits — nothing to diff".to_owned());
                    }
                    let n = ops_owned.len();
                    Ok(format!(
                        "{n} staged edit(s), NOT yet applied:\n{diff}\n(commit with apply_edits, discard with discard_edits)"
                    ))
                }
                "apply_edits" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let pending = self
                        .pending
                        .lock()
                        .map_err(|_| anyhow::anyhow!("staged-edit queue poisoned"))?;
                    if pending.ops().is_empty() {
                        return Ok("no staged edits — nothing to apply".to_owned());
                    }
                    let ops: Vec<EditOp> = pending.ops().to_vec();
                    let n = ops.len();
                    let paths: Vec<String> = ops.iter().map(|o| o.path().to_owned()).collect();
                    let unique: std::collections::BTreeSet<String> =
                        paths.iter().cloned().collect();
                    // Drop the lock before mutating disk.
                    drop(pending);
                    let result = apply_ops_to_disk(&cwd, &ops)?;
                    Ok(format!(
                        "applied {n} edit(s) to {} file(s): {}\n{}\nstaged queue is now empty",
                        unique.len(),
                        result,
                        "use discard_edits to drop without writing",
                    ))
                }
                "discard_edits" => {
                    let mut pending = self
                        .pending
                        .lock()
                        .map_err(|_| anyhow::anyhow!("staged-edit queue poisoned"))?;
                    let n = pending.ops().len();
                    pending.clear();
                    Ok(format!("discarded {n} staged edit(s); queue is empty"))
                }
                "show_staged_files" => {
                    let pending = self
                        .pending
                        .lock()
                        .map_err(|_| anyhow::anyhow!("staged-edit queue poisoned"))?;
                    if pending.ops().is_empty() {
                        return Ok("no staged edits".to_owned());
                    }
                    let mut counts: std::collections::BTreeMap<String, usize> =
                        std::collections::BTreeMap::new();
                    for op in pending.ops() {
                        *counts.entry(op.path().to_owned()).or_default() += 1;
                    }
                    let mut lines = Vec::with_capacity(counts.len() + 1);
                    lines.push(format!(
                        "{} edit(s) staged across {} file(s):",
                        pending.ops().len(),
                        counts.len()
                    ));
                    for (p, c) in &counts {
                        lines.push(format!("  {c:>3}× {p}"));
                    }
                    Ok(lines.join("\n"))
                }
                "run_shell" => {
                    let args: RunShellArgs = parse_args(arguments_json)?;
                    run_shell_command(args).await
                }
                "open_preview" => {
                    let args: OpenPreviewArgs = parse_args(arguments_json)?;
                    crate::preview::open(args.path.as_deref()).await
                }
                "run_test" => {
                    let args: RunTestArgs = parse_args(arguments_json)?;
                    run_test_tool(args).await
                }
                "run_diagnostics" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: RunDiagnosticsArgs = parse_args(arguments_json)?;
                    let diagnostics = crate::lsp::run_diagnostics(&cwd).await.unwrap_or_default();
                    let scope = args.path.as_deref().unwrap_or(".");
                    let fmt = args.format.as_deref().unwrap_or("errors_only");
                    if diagnostics.is_empty() {
                        return Ok(format!(
                            "no diagnostics in scope '{scope}' — code looks clean"
                        ));
                    }
                    if fmt == "summary" {
                        let mut counts: std::collections::BTreeMap<&str, (usize, usize)> =
                            std::collections::BTreeMap::new();
                        for d in &diagnostics {
                            let entry = counts.entry(d.file.as_str()).or_insert((0, 0));
                            match d.severity {
                                crate::lsp::Severity::Error => entry.0 += 1,
                                _ => entry.1 += 1,
                            }
                        }
                        let mut lines = vec![format!(
                            "{} diagnostic(s) in scope '{}' across {} file(s):",
                            diagnostics.len(),
                            scope,
                            counts.len()
                        )];
                        for (f, (e, w)) in &counts {
                            lines.push(format!("  {e} err / {w} warn  {f}"));
                        }
                        Ok(lines.join("\n"))
                    } else if fmt == "full" {
                        let formatted = crate::lsp::format_diagnostics(&diagnostics, 50);
                        Ok(format!(
                            "diagnostics for '{scope}':\n{}",
                            formatted.join("\n")
                        ))
                    } else {
                        // errors_only (default)
                        let errs: Vec<&crate::lsp::Diagnostic> = diagnostics
                            .iter()
                            .filter(|d| d.severity == crate::lsp::Severity::Error)
                            .collect();
                        Ok(format!(
                            "{} error(s) in scope '{}':\n{}",
                            errs.len(),
                            scope,
                            if errs.is_empty() {
                                "(none)".to_owned()
                            } else {
                                errs.iter()
                                    .map(|d| {
                                        format!(
                                            "  {}:{}:{}: {}",
                                            d.file, d.line, d.column, d.message
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            }
                        ))
                    }
                }
                "git_diff" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: GitDiffArgs = parse_args(arguments_json)?;
                    let mut argv: Vec<String> = vec!["diff".into(), "HEAD".into()];
                    if let Some(f) = args.file.as_deref() {
                        argv.push("--".into());
                        argv.push(f.to_owned());
                    }
                    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
                    let text = crate::git::run_git(&cwd, &refs).await?;
                    Ok(format!("uncommitted changes vs HEAD:\n{text}"))
                }
                "git_log" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: GitLogArgs = parse_args(arguments_json)?;
                    let mut argv: Vec<String> = vec!["log".into()];
                    match args.format.as_deref() {
                        Some("short") => argv.push("--pretty=short".into()),
                        Some("full") => argv.push("--pretty=full".into()),
                        Some("stat") => argv.push("--stat".into()),
                        _ => argv.push("--oneline".into()),
                    }
                    if let Some(n) = args.max_commits {
                        argv.push(format!("-{n}"));
                    }
                    if let Some(a) = args.author.as_deref() {
                        argv.push(format!("--author={a}"));
                    }
                    if let Some(b) = args.branch.as_deref() {
                        argv.push(b.to_owned());
                    }
                    if let Some(p) = args.path.as_deref() {
                        argv.push("--".into());
                        argv.push(p.to_owned());
                    }
                    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
                    let text = crate::git::run_git(&cwd, &refs).await?;
                    Ok(format!("recent commits:\n{text}"))
                }
                "git_branch" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: GitBranchArgs = parse_args(arguments_json)?;
                    let action =
                        crate::git::BranchAction::parse(&args.action).ok_or_else(|| {
                            anyhow::anyhow!("unknown branch action '{}'", args.action)
                        })?;
                    let argv = action.argv(args.name.as_deref().unwrap_or(""))?;
                    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
                    let text = crate::git::run_git(&cwd, &refs).await?;
                    Ok(format!("git branch {} done:\n{text}", action.label()))
                }
                "git_commit" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: GitCommitArgs = parse_args(arguments_json)?;
                    git_commit_tool(&cwd, &args).await
                }
                "web_search" => {
                    let args: WebSearchArgs = parse_args(arguments_json)?;
                    web_search_tool(args).await
                }
                "web_fetch" => {
                    let args: WebFetchArgs = parse_args(arguments_json)?;
                    web_fetch_tool(args).await
                }
                "ask_user" => {
                    let args: AskUserArgs = parse_args(arguments_json)?;
                    // In non-interactive mode, return a default answer
                    bail!(
                        "ask_user requires interactive mode — user input is needed: {}",
                        args.question
                    );
                }
                "delegate_task" => {
                    let args: DelegateTaskArgs = parse_args(arguments_json)?;
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let overview = crate::scan::scan(&cwd).await;
                    let combined_context = match &args.context {
                        Some(c) if !c.trim().is_empty() => {
                            format!("{overview}\n\nAdditional context:\n{c}")
                        }
                        _ => overview,
                    };
                    let http = crate::config::Config::http_client()
                        .context("failed to build HTTP client for delegation")?;
                    let provider = crate::provider::resolve(
                        &std::env::var("GOVINDA_PROVIDER")
                            .unwrap_or_else(|_| crate::config::DEFAULT_PROVIDER.to_owned()),
                        None,
                        None,
                        |name| std::env::var(format!("{name}_API_KEY")).ok(),
                    )
                    .context("provider setup failed for delegation")?;
                    let timeout_secs = args.timeout_secs.unwrap_or(60).clamp(1, 300);
                    let fut = crate::swarm::explore(
                        &args.task,
                        &http,
                        provider.as_ref(),
                        &combined_context,
                    );
                    let result = tokio::time::timeout(Duration::from_secs(timeout_secs), fut).await;
                    match result {
                        Ok(Ok(output)) => Ok(format!(
                            "{{\"task\":{},\"status\":\"completed\",\"output\":{}}}",
                            serde_json::json!(args.task),
                            serde_json::json!(output)
                        )),
                        Ok(Err(e)) => Ok(format!(
                            "{{\"task\":{},\"status\":\"failed\",\"error\":{}}}",
                            serde_json::json!(args.task),
                            serde_json::json!(format!("{e:#}"))
                        )),
                        Err(_) => Ok(format!(
                            "{{\"task\":{},\"status\":\"timeout\",\"error\":\"subagent timed out after {timeout_secs}s\"}}",
                            serde_json::json!(args.task)
                        )),
                    }
                }
                "show_token_budget" => Ok(crate::tokens::budget_summary()
                    .unwrap_or_else(|_| "token counter unavailable".to_owned())),
                "show_capabilities" => Ok(crate::commands::capabilities_summary()
                    .unwrap_or_else(|_| "capability summary unavailable".to_owned())),
                "remember" => {
                    let args: RememberArgs = parse_args(arguments_json)?;
                    crate::memory::append_note(&args.note);
                    Ok(format!("remembered: {}", args.note))
                }
                "forget" => {
                    let args: ForgetArgs = parse_args(arguments_json)?;
                    let removed = crate::memory::remove_note(&args.note);
                    if removed > 0 {
                        Ok(format!("removed {removed} matching note(s)"))
                    } else {
                        Ok(format!("no note matched '{}' — nothing removed", args.note))
                    }
                }
                "quality_gate_check" => {
                    #[derive(serde::Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct QualityGateArgs {
                        phase: String,
                        files_delivered: Option<Vec<String>>,
                        line_count_estimate: Option<usize>,
                        checks: crate::govinda_protocol::QualityGateChecks,
                    }
                    let args: QualityGateArgs = parse_args(arguments_json)?;
                    let phase = match args.phase.as_str() {
                        "INSTRUCTION_INGESTION" => {
                            crate::govinda_protocol::ProjectPhase::InstructionIngestion
                        }
                        "PROJECT_INTELLIGENCE" => {
                            crate::govinda_protocol::ProjectPhase::ProjectAnalysis
                        }
                        "ARCHITECTURE_ROADMAP" => {
                            crate::govinda_protocol::ProjectPhase::ArchitectureRoadmap
                        }
                        "DESIGN_SYSTEM" => crate::govinda_protocol::ProjectPhase::DesignSystem,
                        "DEVELOPMENT_PLAN" => {
                            crate::govinda_protocol::ProjectPhase::DevelopmentPlan
                        }
                        "IMPLEMENTATION" => crate::govinda_protocol::ProjectPhase::Implementation,
                        "SELF_VERIFICATION" => {
                            crate::govinda_protocol::ProjectPhase::SelfVerification
                        }
                        "FINAL_VALIDATION" => {
                            crate::govinda_protocol::ProjectPhase::FinalValidation
                        }
                        other => bail!("unknown phase '{other}'"),
                    };
                    let config = crate::govinda_protocol::ProtocolConfig::default();
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let files: Vec<String> = args.files_delivered.unwrap_or_default();
                    let result = crate::govinda_protocol::run_quality_gate(
                        phase,
                        &files,
                        args.line_count_estimate.unwrap_or(0),
                        &args.checks,
                        &config,
                        |rel| {
                            // Workspace-relative only; absolute paths and `..`
                            // are rejected, matching the read_file policy.
                            if rel.is_empty() || rel.contains("..") {
                                return None;
                            }
                            let path = cwd.join(rel);
                            let Ok(meta) = std::fs::metadata(&path) else {
                                return None;
                            };
                            if !meta.is_file() || meta.len() > MAX_INPUT_FILE_BYTES {
                                return None;
                            }
                            std::fs::read_to_string(&path).ok()
                        },
                    );
                    if result.passed {
                        Ok(format!(
                            "ALL QUALITY GATES PASSED for phase={phase}. Proceed to next phase. \
                             {summary}",
                            phase = result.phase,
                            summary = serde_json::json!({
                                "line_count": result.line_count,
                                "min_line_count": result.min_line_count,
                            })
                        ))
                    } else {
                        let violations = result.violations.join("\n  - ");
                        Ok(format!(
                            "QUALITY GATE FAILURES for phase={phase}:\n  - {violations}\n\n\
                             Fix these issues and call quality_gate_check again before \
                             claiming completion.",
                            phase = result.phase,
                        ))
                    }
                }
                other => match self.shell_tools.iter().find(|t| t.name == other) {
                    Some(def) => run_shell_tool(def, arguments_json).await,
                    None => {
                        // Try the GOVINDA toolbox (18 high-leverage tools).
                        let cwd = std::env::current_dir()
                            .unwrap_or_else(|_| std::path::PathBuf::from("."));
                        if let Some(result) =
                            crate::toolbox::registry::dispatch(other, arguments_json, &cwd)
                        {
                            match result {
                                Ok(s) => Ok(s),
                                Err(e) => bail!("toolbox tool '{other}' failed: {e:#}"),
                            }
                        } else {
                            bail!("unknown tool '{other}'")
                        }
                    }
                },
            }
        })
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CountTokensArgs {
    text: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentTimeArgs {
    timezone: Option<String>,
    format: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadFileArgs {
    path: String,
    offset_line: Option<usize>,
    max_lines: Option<usize>,
    /// Prepend a symbol/import outline (computed from the whole file, so it
    /// stays useful when only a line range is returned). Default: true.
    include_outline: Option<bool>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteFileArgs {
    pub path: String,
    pub content: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteFileArgs {
    path: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveFileArgs {
    from: String,
    to: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CopyFileArgs {
    from: String,
    to: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ListFilesArgs {
    path: Option<String>,
    max_entries: Option<usize>,
    max_depth: Option<usize>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ListDirectoryArgs {
    path: Option<String>,
    include_hidden: Option<bool>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewDiffArgs {
    file: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct GrepArgs {
    pattern: String,
    path: Option<String>,
    max_matches: Option<usize>,
    include_globs: Option<Vec<String>>,
    case_sensitive: Option<bool>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RunShellArgs {
    command: String,
    timeout_secs: Option<u64>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RunTestArgs {
    filter: Option<String>,
    timeout_secs: Option<u64>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenPreviewArgs {
    path: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FindSymbolArgs {
    name: String,
    kind: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ExplainCodeArgs {
    path: String,
    symbol: Option<String>,
    max_lines: Option<usize>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GitDiffArgs {
    file: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GitLogArgs {
    max_commits: Option<usize>,
    path: Option<String>,
    author: Option<String>,
    branch: Option<String>,
    format: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GitBranchArgs {
    action: String,
    name: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GitCommitArgs {
    message: String,
    stage_all: Option<bool>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RunDiagnosticsArgs {
    path: Option<String>,
    format: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct WebSearchArgs {
    query: String,
    max_results: Option<usize>,
    site: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WebFetchArgs {
    url: String,
    max_chars: Option<usize>,
    format: Option<String>,
    timeout_secs: Option<u64>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AskUserArgs {
    question: String,
    #[allow(dead_code)]
    options: Option<Vec<String>>,
    #[allow(dead_code)]
    multi_select: Option<bool>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DelegateTaskArgs {
    task: String,
    #[allow(dead_code)]
    context: Option<String>,
    #[allow(dead_code)]
    timeout_secs: Option<u64>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RememberArgs {
    note: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ForgetArgs {
    note: String,
}

// ---------------------------------------------------------------------------
// Web tools (web_search / web_fetch)
// ---------------------------------------------------------------------------

/// `web_search`: performs a web search using a lightweight approach.
/// Uses DuckDuckGo's lite HTML endpoint to extract search results.
async fn web_search_tool(args: WebSearchArgs) -> Result<String> {
    let query = args.query.trim();
    anyhow::ensure!(!query.is_empty(), "query must not be empty");
    let max_results = args.max_results.unwrap_or(5).clamp(1, 20);

    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding::encode(query)
    );
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(15))
        .build()
        .context("failed to build HTTP client for web search")?;
    let resp = client
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (compatible; govinda-cli/1.0)")
        .send()
        .await
        .context("web search request failed")?
        .text()
        .await
        .context("failed to read web search response")?;

    // Parse results from DuckDuckGo lite HTML
    let mut results = Vec::new();
    for block in resp.split("<a rel=\"nofollow\"") {
        if results.len() >= max_results {
            break;
        }
        // Extract URL
        let Some(href_start) = block.find("href=") else {
            continue;
        };
        let href_rest = &block[href_start + 6..];
        let Some(url_end) = href_rest.find(['"', '\'', ' ']) else {
            continue;
        };
        let result_url = &href_rest[..url_end];
        // Extract title (inside <a> tag)
        let title = block
            .find('>')
            .map(|i| &block[i + 1..])
            .and_then(|title_rest| title_rest.find("</a>"))
            .map(|end| {
                #[allow(clippy::unwrap_used)] // safe: outer find() succeeded
                let title_rest = &block[block.find('>').unwrap() + 1..];
                &title_rest[..end]
            })
            .unwrap_or("")
            .trim();
        // Extract snippet
        let snippet = block
            .find("class=\"result-snippet\"")
            .map(|i| &block[i + 22..])
            .and_then(|snip_rest| snip_rest.find("</td>"))
            .map(|end| {
                #[allow(clippy::unwrap_used)] // safe: outer find() succeeded
                let snip_rest = &block[block.find("class=\"result-snippet\"").unwrap() + 22..];
                &snip_rest[..end]
            })
            .unwrap_or("")
            .trim();
        // Strip HTML tags from snippet
        let clean_snippet = strip_html_tags(snippet);
        if !result_url.is_empty() && !result_url.starts_with("//") {
            results.push(format!(
                "{}. {}\n   {}\n   {}",
                results.len() + 1,
                strip_html_tags(title),
                result_url,
                clean_snippet
            ));
        }
    }

    if results.is_empty() {
        Ok(format!(
            "{{\"query\":\"{}\",\"results\":0,\"note\":\"no results found — try different keywords\"}}",
            query
        ))
    } else {
        Ok(
            format!("{{\"query\":\"{}\",\"results\":{}}}", query, results.len())
                + "\n\n"
                + &results.join("\n\n"),
        )
    }
}

/// Strips HTML tags from a string.
fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_entity = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            '&' => in_entity = true,
            ';' if in_entity => in_entity = false,
            _ if !in_tag && !in_entity => out.push(c),
            _ => {}
        }
    }
    out.trim().to_owned()
}

/// `web_fetch`: fetches a URL and returns the readable text content.
///
/// SSRF is blocked at two layers:
/// - `ensure_safe_url_with_dns` checks literal IPs (`0.0.0.0`, `::1`,
///   `127.*`, `10.*`, `192.168.*`, `169.254.*`, `172.16-31.*`, `::ffff:*`)
///   plus DNS rebinding (resolves the host and rejects private IPs).
/// - `redirect_policy` re-validates every redirect target.
async fn web_fetch_tool(args: WebFetchArgs) -> Result<String> {
    let url = args.url.trim();
    anyhow::ensure!(!url.is_empty(), "url must not be empty");
    // DNS rebinding check: resolves host and rejects private IPs.
    crate::ssrf::ensure_safe_url_with_dns(url).await?;
    let max_chars = args.max_chars.unwrap_or(20_000).min(100_000);
    let timeout_secs = args.timeout_secs.unwrap_or(15).clamp(1, 60);
    let format = args.format.as_deref().unwrap_or("text");

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(crate::ssrf::redirect_policy())
        .user_agent("Mozilla/5.0 (compatible; govinda-cli/1.0)")
        .build()
        .context("failed to build HTTP client for web fetch")?;
    let resp = client
        .get(url)
        .send()
        .await
        .context(format!("failed to fetch {url}"))?
        .text()
        .await
        .context(format!("failed to read response from {url}"))?;

    // Format passthrough for raw/markdown.
    if format == "html" {
        let truncated = if resp.len() > max_chars {
            format!("{}…\n…(truncated to {max_chars} chars)", &resp[..max_chars])
        } else {
            resp
        };
        return Ok(truncated);
    }
    if format == "markdown" {
        let stripped = strip_html_tags(&resp);
        let truncated = if stripped.len() > max_chars {
            format!(
                "{}…\n…(truncated to {max_chars} chars)",
                &stripped[..max_chars]
            )
        } else {
            stripped
        };
        return Ok(truncated);
    }

    let text = if resp.len() > max_chars * 4 {
        // Likely HTML — strip tags and scripts
        let stripped = strip_html_tags(&resp);
        let lines: Vec<&str> = stripped
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        lines.join("\n")
    } else {
        resp
    };

    let truncated: String = text.chars().take(max_chars).collect();
    Ok(format!(
        "{{\"url\":\"{}\",\"chars\":{}}}
\n{}",
        url,
        text.len(),
        truncated
    ))
}

// ---------------------------------------------------------------------------
// User-defined shell tools ([[tools]] in config.toml)
// ---------------------------------------------------------------------------

/// A user-defined tool that runs an external command.
///
/// `args_template` is an argv template: each element becomes one argument,
/// and `{placeholder}` tokens inside it are substituted with string values
/// from the model's arguments. Commands are spawned directly — never through
/// a shell — so template values cannot inject extra commands or flags beyond
/// the words the user wrote.
///
/// Every shell tool requires interactive confirmation before it runs.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ShellToolDef {
    pub name: String,
    pub description: String,
    pub command: String,
    #[serde(default)]
    pub args_template: Vec<String>,
    /// Wall-clock cap in seconds (default 30, max 600).
    pub timeout_secs: Option<u64>,
    /// Per-stream output kept, in bytes (default 64 KiB, max 1 MiB).
    pub max_output_bytes: Option<usize>,
}

/// Validates definitions from config.toml; a bad definition is a hard error
/// so typos never silently weaken or disable a tool.
pub fn validate_shell_tools(defs: &[ShellToolDef]) -> Result<()> {
    let mut seen = HashSet::new();
    for d in defs {
        anyhow::ensure!(
            tool_name_re().is_match(&d.name),
            "tool name '{}' must be lowercase snake_case ([a-z][a-z0-9_]*)",
            d.name
        );
        anyhow::ensure!(
            seen.insert(d.name.clone()),
            "duplicate tool name '{}'",
            d.name
        );
        anyhow::ensure!(
            !BUILTIN_TOOL_NAMES.contains(&d.name.as_str()),
            "tool '{}' collides with a built-in tool",
            d.name
        );
        anyhow::ensure!(
            !d.description.trim().is_empty(),
            "tool '{}' needs a description",
            d.name
        );
        anyhow::ensure!(
            !d.command.trim().is_empty(),
            "tool '{}' needs a command",
            d.name
        );
        if let Some(t) = d.timeout_secs {
            anyhow::ensure!(
                (1..=MAX_SHELL_TIMEOUT_SECS).contains(&t),
                "tool '{}': timeout_secs must be 1-{MAX_SHELL_TIMEOUT_SECS}",
                d.name
            );
        }
        if let Some(b) = d.max_output_bytes {
            anyhow::ensure!(
                (1..=MAX_SHELL_OUTPUT_BYTES).contains(&b),
                "tool '{}': max_output_bytes must be 1-{MAX_SHELL_OUTPUT_BYTES}",
                d.name
            );
        }
    }
    Ok(())
}

/// Unique `{placeholder}` names across an argv template, sorted for stable
/// schema generation.
fn placeholders_in(words: &[String]) -> Vec<String> {
    let mut names: Vec<String> = words
        .iter()
        .flat_map(|w| placeholder_re().captures_iter(w).map(|c| c[1].to_owned()))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Builds the JSON schema advertised to the model: every placeholder becomes
/// a required string parameter, nothing else is accepted.
fn shell_tool_schema(def: &ShellToolDef) -> Value {
    let names = placeholders_in(&def.args_template);
    let mut props = serde_json::Map::new();
    for n in &names {
        props.insert(
            n.clone(),
            serde_json::json!({
                "type": "string",
                "description": format!("Value substituted into {{{n}}}"),
            }),
        );
    }
    serde_json::json!({
        "type": "object",
        "properties": props,
        "required": names,
        "additionalProperties": false
    })
}

/// Fills one argv word: replaces every `{placeholder}` with the matching
/// value from the model's arguments. Missing keys and oversized or NUL-bearing
/// values are rejected.
fn fill_word(word: &str, values: &HashMap<String, Value>) -> Result<String> {
    let mut out = String::with_capacity(word.len());
    let mut last = 0;
    for cap in placeholder_re().captures_iter(word) {
        let Some(m) = cap.get(0) else { continue };
        out.push_str(&word[last..m.start()]);
        let key = &cap[1];
        let raw = values
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("missing argument '{key}'"))?;
        let s = match raw {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        anyhow::ensure!(
            s.chars().count() <= MAX_ARG_VALUE_CHARS,
            "argument '{key}' too long (cap {MAX_ARG_VALUE_CHARS} chars)"
        );
        anyhow::ensure!(!s.contains('\0'), "argument '{key}' contains NUL bytes");
        out.push_str(&s);
        last = m.end();
    }
    out.push_str(&word[last..]);
    Ok(out)
}

/// Spawns the command directly (no shell), enforces the timeout, caps both
/// output streams, and returns a structured result. A non-zero exit code is
/// still an `Ok` — the model should see the failure and react to it.
async fn run_shell_tool(def: &ShellToolDef, arguments_json: &str) -> Result<String> {
    let values: HashMap<String, Value> = parse_args(arguments_json)?;
    let mut argv = Vec::with_capacity(def.args_template.len());
    for word in &def.args_template {
        argv.push(fill_word(word, &values)?);
    }
    audit_shell(&format!("{} {}", def.command, argv.join(" ")));

    let timeout_dur = Duration::from_secs(
        def.timeout_secs
            .unwrap_or(DEFAULT_SHELL_TIMEOUT_SECS)
            .clamp(1, MAX_SHELL_TIMEOUT_SECS),
    );
    let max_out = def
        .max_output_bytes
        .unwrap_or(DEFAULT_SHELL_OUTPUT_BYTES)
        .clamp(1, MAX_SHELL_OUTPUT_BYTES);

    let started = Instant::now();
    let spawned = tokio::process::Command::new(&def.command)
        .args(&argv)
        .output();
    let output = match tokio::time::timeout(timeout_dur, spawned).await {
        Err(_) => bail!("timed out after {}s", timeout_dur.as_secs()),
        Ok(res) => res.with_context(|| format!("cannot spawn '{}'", def.command))?,
    };

    Ok(serde_json::json!({
        "exit_code": output.status.code(),
        "duration_ms": started.elapsed().as_millis() as u64,
        "stdout": capped_lossy(&output.stdout, max_out),
        "stderr": capped_lossy(&output.stderr, max_out),
    })
    .to_string())
}

/// Lossy-decodes captured bytes, truncating at `cap` with a visible marker.
fn capped_lossy(bytes: &[u8], cap: usize) -> String {
    if bytes.len() <= cap {
        return String::from_utf8_lossy(bytes).to_string();
    }
    format!(
        "{}\n…(truncated at {cap} bytes)",
        String::from_utf8_lossy(&bytes[..cap])
    )
}

// ---------------------------------------------------------------------------
// Per-tool enable/disable persistence (.govinda_tools.json)
// ---------------------------------------------------------------------------

const DISABLED_TOOLS_FILE: &str = ".govinda_tools.json";

fn disabled_tools_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(DISABLED_TOOLS_FILE)
}

/// Loads the persisted disabled-tool set; a missing or unreadable file means
/// "everything enabled".
pub fn load_disabled_tools() -> HashSet<String> {
    fs::read_to_string(disabled_tools_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

pub fn save_disabled_tools(disabled: &HashSet<String>) -> Result<()> {
    let mut names: Vec<String> = disabled.iter().cloned().collect();
    names.sort();
    let json = serde_json::to_string_pretty(&names).context("cannot serialize tool toggles")?;
    fs::write(disabled_tools_path(), json).context("could not write .govinda_tools.json")
}

// ---------------------------------------------------------------------------
// Staged edits: surgical editing with an explicit apply step
// ---------------------------------------------------------------------------

/// One staged, not-yet-applied workspace change.
///
/// Editing tools never touch the filesystem directly; they validate against
/// current file contents and queue an `EditOp` here. The user inspects the
/// unified diff (`/diff` or the `view_diff` tool) and commits everything at
/// once with `/apply`, or discards with `/reject`.
#[derive(Debug, Clone)]
pub enum EditOp {
    /// Replace the unique occurrence of `old_string` in `path`.
    Replace {
        path: String,
        old_string: String,
        new_string: String,
    },
    /// Insert `text` (a whole block of lines, trailing newline optional)
    /// before or after 1-based line `line` of `path`.
    Insert {
        path: String,
        line: usize,
        text: String,
        after: bool,
    },
}

impl EditOp {
    pub fn path(&self) -> &str {
        match self {
            EditOp::Replace { path, .. } | EditOp::Insert { path, .. } => path,
        }
    }

    /// One-line human-readable summary for listings and confirmations.
    pub fn describe(&self) -> String {
        match self {
            EditOp::Replace {
                path,
                old_string,
                new_string,
            } => format!(
                "replace {} → {} in {path}",
                first_line(old_string),
                first_line(new_string)
            ),
            EditOp::Insert {
                path,
                line,
                text,
                after,
            } => format!(
                "insert {} line {line} in {path}: {}",
                if *after { "after" } else { "before" },
                first_line(text)
            ),
        }
    }
}

fn first_line(s: &str) -> String {
    let line = s.lines().next().unwrap_or("");
    if line.chars().count() > 60 {
        let mut cut: String = line.chars().take(57).collect();
        cut.push_str("...");
        cut
    } else {
        line.to_owned()
    }
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    let cut: String = s.chars().take(max_chars).collect();
    format!("{cut}\n…(diff preview truncated)")
}

/// In-memory queue of staged edits shared between the tool executor and the
/// REPL commands (`/diff`, `/apply`, `/reject`).
#[derive(Default)]
pub struct PendingEdits {
    ops: Vec<EditOp>,
}

impl PendingEdits {
    pub fn push(&mut self, op: EditOp) {
        self.ops.push(op);
    }
    pub fn ops(&self) -> &[EditOp] {
        &self.ops
    }
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
    pub fn clear(&mut self) {
        self.ops.clear();
    }
}

/// Renders the combined unified diff of all staged edits against the files'
/// current on-disk contents. Files whose edits cancel out produce no hunk.
pub fn staged_diff(base: &Path, pending: &[EditOp]) -> Result<String> {
    let mut per_file: Vec<(String, Vec<&EditOp>)> = Vec::new();
    for op in pending {
        match per_file.iter_mut().find(|(p, _)| *p == op.path()) {
            Some((_, ops)) => ops.push(op),
            None => per_file.push((op.path().to_owned(), vec![op])),
        }
    }
    let mut out = String::new();
    for (path, ops) in per_file {
        let full = resolve_in(base, &path)?;
        let bytes = fs::read(&full).with_context(|| format!("cannot read '{path}'"))?;
        anyhow::ensure!(
            !bytes.contains(&0),
            "'{path}' looks binary; refusing to diff"
        );
        let original = String::from_utf8_lossy(&bytes).to_string();
        let updated = apply_ops_to_content(&original, &path, &ops)?;
        out.push_str(&crate::diff::unified_diff(&path, &original, &updated));
    }
    Ok(out)
}

/// Validates and writes a batch of edit ops to disk (atomic per file, but
/// not across files — used by the `apply_edits` tool and the `/apply` slash
/// command). Returns a summary of files written.
pub fn apply_ops_to_disk(base: &Path, ops: &[EditOp]) -> Result<String> {
    // Group by file path and apply each group atomically.
    let mut per_file: std::collections::BTreeMap<String, Vec<&EditOp>> =
        std::collections::BTreeMap::new();
    for op in ops {
        per_file.entry(op.path().to_owned()).or_default().push(op);
    }
    let mut written: Vec<String> = Vec::new();
    for (path, ops) in per_file {
        let full = resolve_in(base, &path)?;
        let bytes = fs::read(&full).with_context(|| format!("cannot read '{path}' for apply"))?;
        anyhow::ensure!(
            !bytes.contains(&0),
            "'{path}' looks binary; refusing to apply"
        );
        let original = String::from_utf8_lossy(&bytes).to_string();
        let updated = apply_ops_to_content(&original, &path, &ops)?;
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create parent dir for '{path}'"))?;
        }
        fs::write(&full, &updated).with_context(|| format!("cannot write '{path}'"))?;
        written.push(path);
    }
    Ok(written.join(", "))
}

/// Validates and applies a batch of edit ops to one file's content.
///
/// All validation happens before any mutation so a bad op aborts the whole
/// batch atomically. Insertions are grouped by anchor line and applied from
/// the highest line down so earlier inserts never shift later anchors.
pub(crate) fn apply_ops_to_content(
    content: &str,
    display_path: &str,
    ops: &[&EditOp],
) -> Result<String> {
    // Pass 1 — resolve every op to a concrete change without mutating.
    enum Change {
        Spans {
            start_byte: usize,
            end_byte: usize,
            replacement: String,
        },
        Lines {
            line: usize,
            after: bool,
            text: String,
        },
    }
    let mut changes = Vec::with_capacity(ops.len());
    for op in ops {
        match op {
            EditOp::Replace {
                old_string,
                new_string,
                ..
            } => {
                anyhow::ensure!(
                    !old_string.is_empty(),
                    "'{display_path}': empty search string is not allowed"
                );
                let count = content.matches(old_string.as_str()).count();
                anyhow::ensure!(
                    count == 1,
                    "'{display_path}': search string matched {count} times (must be exactly \
                     once) — include more surrounding lines to make it unique"
                );
                let start = content.find(old_string.as_str()).unwrap_or(0);
                changes.push(Change::Spans {
                    start_byte: start,
                    end_byte: start + old_string.len(),
                    replacement: new_string.clone(),
                });
            }
            EditOp::Insert {
                line, text, after, ..
            } => {
                anyhow::ensure!(*line >= 1, "'{display_path}': line numbers are 1-based");
                anyhow::ensure!(
                    !text.trim().is_empty(),
                    "'{display_path}': insertion text is empty"
                );
                let total = content.lines().count();
                let max = if *after { total } else { total + 1 };
                anyhow::ensure!(
                    *line <= max,
                    "'{display_path}': cannot insert {} line {line} (file has {total} lines)",
                    if *after { "after" } else { "before" }
                );
                changes.push(Change::Lines {
                    line: *line,
                    after: *after,
                    text: text.clone(),
                });
            }
        }
    }

    // Conflict check: byte ranges touched by Replace ops must not overlap.
    let mut spans: Vec<(usize, usize)> = changes
        .iter()
        .filter_map(|c| match c {
            Change::Spans {
                start_byte,
                end_byte,
                ..
            } => Some((*start_byte, *end_byte)),
            Change::Lines { .. } => None,
        })
        .collect();
    spans.sort();
    for pair in spans.windows(2) {
        anyhow::ensure!(
            pair[0].1 <= pair[1].0,
            "'{display_path}': two staged edits overlap — split them into separate applies"
        );
    }
    // Two insertions at the same anchor are ambiguous about ordering.
    let mut anchors: Vec<(usize, bool)> = changes
        .iter()
        .filter_map(|c| match c {
            Change::Lines { line, after, .. } => Some((*line, *after)),
            Change::Spans { .. } => None,
        })
        .collect();
    anchors.sort();
    for pair in anchors.windows(2) {
        anyhow::ensure!(
            pair[0] != pair[1] || !pair[0].1,
            "'{display_path}': multiple insertions anchored at the same point"
        );
    }

    // Pass 2 — mutate. Replaces go highest byte offset first; inserts group
    // by anchor line and apply bottom-up.
    let mut updated = content.to_owned();
    let mut span_changes: Vec<_> = changes
        .iter()
        .filter(|c| matches!(c, Change::Spans { .. }))
        .collect();
    span_changes.sort_by_key(|c| match c {
        Change::Spans { start_byte, .. } => std::cmp::Reverse(*start_byte),
        Change::Lines { .. } => std::cmp::Reverse(0),
    });
    for c in span_changes {
        if let Change::Spans {
            start_byte,
            end_byte,
            replacement,
        } = c
        {
            updated.replace_range(*start_byte..*end_byte, replacement);
        }
    }

    let mut line_changes: Vec<_> = changes
        .iter()
        .filter(|c| matches!(c, Change::Lines { .. }))
        .collect();
    line_changes.sort_by_key(|c| match c {
        Change::Lines { line, after, .. } => (*line, !*after),
        _ => (0, true),
    });
    // Bottom-up: inserting at higher lines first keeps lower anchors valid.
    for c in line_changes.into_iter().rev() {
        if let Change::Lines { line, after, text } = c {
            let had_trailing_nl = updated.ends_with('\n');
            let mut lines: Vec<String> = updated.lines().map(str::to_owned).collect();
            let idx = if *after { *line } else { *line - 1 }; // 0-based slot
            for (k, l) in text.lines().enumerate() {
                lines.insert(idx + k, l.to_owned());
            }
            let mut joined = lines.join("\n");
            if had_trailing_nl {
                joined.push('\n');
            }
            updated = joined;
        }
    }
    Ok(updated)
}

/// Builds the `EditOp` requested by one of the staging tools.
fn parse_edit_op(name: &str, arguments_json: &str) -> Result<EditOp> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct EditArgs {
        path: String,
        old_string: Option<String>,
        new_string: Option<String>,
        line: Option<usize>,
        text: Option<String>,
    }
    let args: EditArgs = parse_args(arguments_json)?;
    anyhow::ensure!(!args.path.trim().is_empty(), "path must not be empty");
    match name {
        "edit_file" => Ok(EditOp::Replace {
            path: args.path,
            old_string: args
                .old_string
                .ok_or_else(|| anyhow::anyhow!("edit_file needs 'old_string'"))?,
            new_string: args
                .new_string
                .ok_or_else(|| anyhow::anyhow!("edit_file needs 'new_string'"))?,
        }),
        "insert_after" | "insert_before" => Ok(EditOp::Insert {
            path: args.path,
            line: args
                .line
                .ok_or_else(|| anyhow::anyhow!("{name} needs 'line'"))?,
            text: args
                .text
                .ok_or_else(|| anyhow::anyhow!("{name} needs 'text'"))?,
            after: name == "insert_after",
        }),
        other => bail!("'{other}' does not stage edits"),
    }
}

/// Fail-fast validation against ground truth: the target file must be a
/// readable text file and the edit must apply cleanly right now. (Re-checked
/// at apply time in case the file changed meanwhile.)
fn validate_staged_op(base: &Path, op: &EditOp) -> Result<()> {
    let full = resolve_in(base, op.path())?;
    let meta = fs::metadata(&full).with_context(|| format!("cannot stat '{}'", op.path()))?;
    anyhow::ensure!(meta.is_file(), "'{}' is not a regular file", op.path());
    anyhow::ensure!(
        meta.len() <= MAX_INPUT_FILE_BYTES,
        "'{}' is larger than {} MB",
        op.path(),
        MAX_INPUT_FILE_BYTES / (1024 * 1024)
    );
    let bytes = fs::read(&full).with_context(|| format!("cannot read '{}'", op.path()))?;
    anyhow::ensure!(
        !bytes.contains(&0),
        "'{}' looks binary; surgical edits are refused",
        op.path()
    );
    let content = String::from_utf8_lossy(&bytes);
    // Dry-run the single op to surface uniqueness/bounds errors immediately.
    let refs = [op];
    apply_ops_to_content(&content, op.path(), &refs)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Audit log for shell execution (security)
// ---------------------------------------------------------------------------

/// Appends a shell command to `.govinda/shell_audit.log` for post-hoc review.
/// Best-effort: failures are ignored so auditing never breaks the tool.
/// The log lives in the workspace (`.govinda/`) and is never sent to the model.
fn audit_shell(command: &str) {
    let ts = crate::clock::now_iso8601();
    let line = format!("[{ts}] shell: {command}\n");
    if let Ok(cwd) = std::env::current_dir() {
        let dir = cwd.join(".govinda");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("shell_audit.log");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            let _ = file.write_all(line.as_bytes());
        }
    }
}

// ---------------------------------------------------------------------------
// Execution tools (run_shell / run_test / check_project)
// ---------------------------------------------------------------------------

const DEFAULT_RUN_SHELL_TIMEOUT_SECS: u64 = 60;
const DEFAULT_RUN_TEST_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectKind {
    Rust,
    Node,
    Python,
}

/// Detects the workspace's project type from its manifest files.
fn detect_project() -> Option<ProjectKind> {
    if Path::new("Cargo.toml").is_file() {
        Some(ProjectKind::Rust)
    } else if Path::new("package.json").is_file() {
        Some(ProjectKind::Node)
    } else if [
        "pyproject.toml",
        "pytest.ini",
        "setup.py",
        "requirements.txt",
    ]
    .iter()
    .any(|f| Path::new(f).is_file())
    {
        Some(ProjectKind::Python)
    } else {
        None
    }
}

/// Runs `program argv…` directly (no shell), enforcing timeout and output
/// caps. The process inherits the executor's working directory — the project
/// root — so relative paths inside build tools resolve correctly.
///
/// Every invocation is audit-logged to `.govinda/shell_audit.log` even when
/// running under `--build` auto-approve, so `--build` does not silently
/// bypass the confirmation trail.
async fn exec_argv(program: &str, argv: &[String], timeout_secs: u64) -> Result<String> {
    audit_shell(&format!("{program} {}", argv.join(" ")));
    let timeout_dur = Duration::from_secs(timeout_secs.clamp(1, MAX_SHELL_TIMEOUT_SECS));
    let max_out = MAX_SHELL_OUTPUT_BYTES;
    let started = Instant::now();
    let spawned = tokio::process::Command::new(program).args(argv).output();
    let output = match tokio::time::timeout(timeout_dur, spawned).await {
        Err(_) => bail!("timed out after {}s", timeout_dur.as_secs()),
        Ok(res) => res.with_context(|| format!("cannot spawn '{program}'"))?,
    };
    Ok(serde_json::json!({
        "command": format!("{program} {}", argv.join(" ")).trim_end(),
        "exit_code": output.status.code(),
        "duration_ms": started.elapsed().as_millis() as u64,
        "stdout": capped_lossy(&output.stdout, max_out),
        "stderr": capped_lossy(&output.stderr, max_out),
    })
    .to_string())
}

/// On Windows several dev tools (npm, npx) are batch files that cannot be
/// spawned as raw executables; route through cmd there. Unix spawns directly.
async fn exec_tool(program: &str, argv: &[String], timeout_secs: u64) -> Result<String> {
    if cfg!(windows) && matches!(program, "npm" | "npx") {
        let mut wrapped = vec!["/C".to_owned(), program.to_owned()];
        wrapped.extend(argv.iter().cloned());
        return exec_argv("cmd", &wrapped, timeout_secs).await;
    }
    exec_argv(program, argv, timeout_secs).await
}

/// `current_time`: returns the current time in the requested timezone/format.
fn current_time(args: &CurrentTimeArgs) -> String {
    use chrono::{DateTime, Local, SecondsFormat, Utc};
    let tz = args.timezone.as_deref().unwrap_or("local");
    let fmt = args.format.as_deref().unwrap_or("iso8601");
    let dt: DateTime<Utc> = match tz {
        "utc" => Utc::now(),
        _ => Local::now().with_timezone(&Utc),
    };
    match fmt {
        "unix" => dt.timestamp().to_string(),
        "human" => dt
            .with_timezone(&Local)
            .format("%A, %B %-d, %Y %-I:%M %p")
            .to_string(),
        _ => dt.to_rfc3339_opts(SecondsFormat::Secs, true),
    }
}

/// Applies a batch of staged edits to disk atomically. Used by both the
/// `/apply` slash command and the `apply_edits` tool.
#[allow(dead_code)]
fn apply_edits_atomic(base: &Path, ops: &[EditOp]) -> Result<String> {
    // Reuse the slash command's apply path so behaviour stays in sync.
    let app = crate::commands::App::new_for_test();
    let pending = app.pending_edits.clone();
    {
        let mut guard = pending.lock().unwrap();
        guard.clear();
        for op in ops {
            guard.push(op.clone());
        }
    }
    crate::commands::apply_pending(base, &app)
}

/// `run_shell`: an arbitrary shell command in the project directory.
///
/// Requires user confirmation via `ToolExecutor::requires_confirmation`.
/// Under `--build` (`GatePolicy::AutoRun`) the confirmation is auto-approved
/// but the command is still audit-logged to `.govinda/shell_audit.log` so
/// every shell execution leaves a trace.
async fn run_shell_command(args: RunShellArgs) -> Result<String> {
    let command = args.command.trim();
    anyhow::ensure!(!command.is_empty(), "command must not be empty");
    anyhow::ensure!(
        command.chars().count() <= MAX_ARG_VALUE_CHARS,
        "command too long (cap {MAX_ARG_VALUE_CHARS} chars)"
    );
    anyhow::ensure!(!command.contains('\0'), "command contains NUL bytes");
    // Audit every shell invocation, including --build auto-approve.
    audit_shell(command);
    let timeout = args
        .timeout_secs
        .unwrap_or(DEFAULT_RUN_SHELL_TIMEOUT_SECS)
        .clamp(1, MAX_SHELL_TIMEOUT_SECS);
    if cfg!(windows) {
        exec_argv("cmd", &["/C".to_owned(), command.to_owned()], timeout).await
    } else {
        exec_argv("sh", &["-c".to_owned(), command.to_owned()], timeout).await
    }
}

/// Builds the argv for the workspace's test runner.
fn test_command(kind: ProjectKind, filter: Option<&str>) -> (String, Vec<String>) {
    let filter = filter.map(str::trim).filter(|f| !f.is_empty());
    match kind {
        ProjectKind::Rust => {
            let mut argv = vec!["test".to_owned()];
            if let Some(f) = filter {
                argv.push(f.to_owned());
            }
            ("cargo".to_owned(), argv)
        }
        ProjectKind::Node => {
            let mut argv = vec!["test".to_owned(), "--".to_owned()];
            if let Some(f) = filter {
                argv.push(f.to_owned());
            }
            ("npm".to_owned(), argv)
        }
        ProjectKind::Python => {
            let mut argv = vec!["-m".to_owned(), "pytest".to_owned()];
            if let Some(f) = filter {
                argv.push("-k".to_owned());
                argv.push(f.to_owned());
            }
            ("python".to_owned(), argv)
        }
    }
}

/// `run_test`: semantic wrapper over the detected test runner. A user-
/// configured test command (`.govinda_project.json`) takes precedence.
async fn run_test_tool(args: RunTestArgs) -> Result<String> {
    let timeout = args
        .timeout_secs
        .unwrap_or(DEFAULT_RUN_TEST_TIMEOUT_SECS)
        .clamp(1, MAX_SHELL_TIMEOUT_SECS);
    if let Some((program, argv)) = crate::project::load()
        .test_command
        .and_then(|cmd| crate::project::ProjectMemory::argv(cmd.trim()))
    {
        return exec_tool(&program, &argv, timeout).await;
    }
    let kind = detect_project().ok_or_else(|| {
        anyhow::anyhow!(
            "no supported project manifest found (Cargo.toml, package.json, pyproject.toml…)"
        )
    })?;
    let (program, argv) = test_command(kind, args.filter.as_deref());
    exec_tool(&program, &argv, timeout).await
}

/// Builds the compile/lint validation command for the project type.
#[allow(dead_code)]
fn check_command(kind: ProjectKind) -> Result<(String, Vec<String>)> {
    Ok(match kind {
        ProjectKind::Rust => ("cargo".to_owned(), vec!["check".to_owned()]),
        ProjectKind::Node => {
            anyhow::ensure!(
                Path::new("tsconfig.json").is_file(),
                "no TypeScript config (tsconfig.json) — nothing to type-check"
            );
            (
                "npx".to_owned(),
                vec!["tsc".to_owned(), "--noEmit".to_owned()],
            )
        }
        ProjectKind::Python => (
            "python".to_owned(),
            vec!["-m".to_owned(), "mypy".to_owned(), ".".to_owned()],
        ),
    })
}

// ---------------------------------------------------------------------------
// Sandbox
// ---------------------------------------------------------------------------

/// Anchors a workspace-relative path under `base`, rejecting absolute paths,
/// rooted paths, and any `..` component. This is the single gate every
/// workspace tool passes through before touching the filesystem.
pub(crate) fn resolve_in(base: &Path, raw: &str) -> Result<PathBuf> {
    anyhow::ensure!(
        !raw.trim().is_empty(),
        "path must not be empty ('.' lists the workspace root)"
    );
    let p = Path::new(raw);
    anyhow::ensure!(
        !p.is_absolute() && !p.has_root(),
        "absolute paths are not allowed — use workspace-relative paths"
    );
    anyhow::ensure!(
        !p.components().any(|c| c == std::path::Component::ParentDir),
        "'..' components are not allowed"
    );
    Ok(base.join(p))
}

fn display_rel(base: &Path, p: &Path) -> String {
    p.strip_prefix(base)
        .map(|r| r.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| p.to_string_lossy().replace('\\', "/"))
}

/// Collects files up to `MAX_WALK_DEPTH` levels below `root`, skipping build
/// dirs and anything excluded by `.govindaignore`. `base` is the workspace
/// root that relative paths (and the ignore file itself) resolve against.
pub(crate) fn walk_files(base: &Path, root: &Path) -> Vec<PathBuf> {
    let ignore = crate::ignore::IgnoreRules::load(base);
    // `display_rel` normalizes separators to '/', so basenames split on '/'.
    let excluded = |path: &Path, is_dir: bool| -> bool {
        let rel = display_rel(base, path);
        if is_dir && SKIP_DIRS.contains(&rel.rsplit('/').next().unwrap_or(&rel)) {
            return true;
        }
        ignore.matches(&rel, is_dir)
    };

    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth >= MAX_WALK_DEPTH || out.len() >= MAX_LIST_ENTRIES {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if out.len() >= MAX_LIST_ENTRIES {
                break;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if excluded(&entry.path(), ft.is_dir()) {
                continue;
            }
            if ft.is_dir() {
                stack.push((entry.path(), depth + 1));
            } else if ft.is_file() {
                out.push(entry.path());
            }
        }
    }
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// Workspace tools
// ---------------------------------------------------------------------------

fn read_file(base: &Path, args: &ReadFileArgs) -> Result<String> {
    let path = resolve_in(base, &args.path)?;
    let meta = fs::metadata(&path).with_context(|| format!("cannot stat '{}'", args.path))?;
    anyhow::ensure!(meta.is_file(), "'{}' is not a regular file", args.path);
    anyhow::ensure!(
        meta.len() <= MAX_INPUT_FILE_BYTES,
        "'{}' is larger than {} MB — read a line range instead via offset_line/max_lines",
        args.path,
        MAX_INPUT_FILE_BYTES / (1024 * 1024)
    );
    let bytes = fs::read(&path).with_context(|| format!("cannot read '{}'", args.path))?;
    anyhow::ensure!(
        !bytes.contains(&0),
        "'{}' looks binary (contains NUL bytes)",
        args.path
    );
    let text = String::from_utf8_lossy(&bytes);
    let total_lines = text.lines().count();

    let mut out = String::new();
    if args.include_outline.unwrap_or(true)
        && let Some(lang) = crate::outline::detect_language(&args.path)
    {
        let outline = crate::outline::outline(lang, &text);
        if !outline.is_empty() {
            out.push_str(&outline);
            out.push_str(&format!(
                "[file: {} — {} lines total]\n",
                args.path, total_lines
            ));
        }
    }

    let start = args.offset_line.unwrap_or(1).max(1);
    anyhow::ensure!(
        start <= total_lines.saturating_add(1),
        "offset_line {start} is past end of file ({total_lines} lines)"
    );
    let max_lines = args.max_lines.unwrap_or(DEFAULT_READ_LINES).max(1);

    let selected: Vec<String> = text
        .lines()
        .skip(start - 1)
        .take(max_lines)
        .enumerate()
        .map(|(i, line)| format!("{:>5}| {}", start + i, line))
        .collect();
    anyhow::ensure!(
        !selected.is_empty(),
        "no lines in range (file has {total_lines} lines)"
    );

    out.push_str(&selected.join("\n"));
    let shown = selected.len();
    if out.chars().count() > MAX_READ_CHARS {
        out = out.chars().take(MAX_READ_CHARS).collect();
        out.push_str("\n…(truncated at character cap)");
    }
    if start + shown - 1 < total_lines {
        out.push_str(&format!(
            "\n…({} more lines — pass offset_line {} to continue)",
            total_lines - (start + shown - 1),
            start + shown
        ));
    }
    Ok(out)
}

pub fn write_file(base: &Path, args: &WriteFileArgs) -> Result<String> {
    let content_bytes = args.content.len();
    anyhow::ensure!(
        content_bytes <= MAX_WRITE_BYTES,
        "content too large ({} bytes; cap {})",
        content_bytes,
        MAX_WRITE_BYTES
    );
    let path = resolve_in(base, &args.path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create parent directory for '{}'", args.path))?;
    }
    let existed = path.exists();
    fs::write(&path, &args.content).with_context(|| format!("cannot write '{}'", args.path))?;
    Ok(format!(
        "{{\"path\":\"{}\",\"bytes\":{},\"created\":{}}}",
        display_rel(base, &path),
        content_bytes,
        !existed
    ))
}

fn delete_file(base: &Path, args: &DeleteFileArgs) -> Result<String> {
    let path = resolve_in(base, &args.path)?;
    let meta = fs::metadata(&path)
        .with_context(|| format!("cannot stat '{}' — does it exist?", args.path))?;
    anyhow::ensure!(meta.is_file(), "'{}' is not a regular file", args.path);
    fs::remove_file(&path).with_context(|| format!("cannot delete '{}'", args.path))?;
    Ok(format!(
        "{{\"path\":\"{}\",\"bytes\":{},\"deleted\":true}}",
        display_rel(base, &path),
        meta.len()
    ))
}

fn move_file(base: &Path, args: &MoveFileArgs) -> Result<String> {
    let from = resolve_in(base, &args.from)?;
    let to = resolve_in(base, &args.to)?;
    anyhow::ensure!(from.exists(), "source '{}' does not exist", args.from);
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create parent directory for '{}'", args.to))?;
    }
    let existed = to.exists();
    fs::rename(&from, &to)
        .with_context(|| format!("cannot move '{}' to '{}'", args.from, args.to))?;
    Ok(format!(
        "{{\"from\":\"{}\",\"to\":\"{}\",\"overwrote\":{}}}",
        display_rel(base, &from),
        display_rel(base, &to),
        existed
    ))
}

fn copy_file(base: &Path, args: &CopyFileArgs) -> Result<String> {
    let from = resolve_in(base, &args.from)?;
    let to = resolve_in(base, &args.to)?;
    anyhow::ensure!(
        from.is_file(),
        "source '{}' is not a regular file",
        args.from
    );
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create parent directory for '{}'", args.to))?;
    }
    let existed = to.exists();
    let bytes = fs::copy(&from, &to)
        .with_context(|| format!("cannot copy '{}' to '{}'", args.from, args.to))?;
    Ok(format!(
        "{{\"from\":\"{}\",\"to\":\"{}\",\"bytes\":{},\"overwrote\":{}}}",
        display_rel(base, &from),
        display_rel(base, &to),
        bytes,
        existed
    ))
}

fn list_files(base: &Path, args: &ListFilesArgs) -> Result<String> {
    let root_arg = args.path.as_deref().unwrap_or(".");
    let root = resolve_in(base, root_arg)?;
    anyhow::ensure!(root.is_dir(), "'{}' is not a directory", root_arg);
    let max_entries = args
        .max_entries
        .unwrap_or(DEFAULT_LIST_ENTRIES)
        .clamp(1, MAX_LIST_ENTRIES);
    let max_depth = args.max_depth.unwrap_or(8).clamp(1, MAX_WALK_DEPTH);

    let mut lines = Vec::new();
    let ignore = crate::ignore::IgnoreRules::load(base);
    // (directory, depth)
    let mut stack = vec![(root.clone(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if lines.len() >= max_entries || depth >= max_depth {
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if lines.len() >= max_entries {
                lines.push(format!(
                    "…(output capped at {max_entries} entries — narrow the path or set max_entries higher)"
                ));
                break;
            }
            let Ok(ft) = entry.file_type() else { continue };
            let name = entry.file_name().to_string_lossy().to_string();
            let rel = display_rel(base, &entry.path());
            if (ft.is_dir() && SKIP_DIRS.contains(&name.as_str()))
                || ignore.matches(&rel, ft.is_dir())
            {
                continue;
            }
            if ft.is_dir() {
                lines.push(format!("{rel}/", rel = rel));
                stack.push((entry.path(), depth + 1));
            } else if ft.is_file() {
                lines.push(rel);
            }
        }
    }
    anyhow::ensure!(
        !lines.is_empty(),
        "'{}' contains no visible files",
        root_arg
    );
    Ok(lines.join("\n"))
}

fn list_directory(base: &Path, args: &ListDirectoryArgs) -> Result<String> {
    let root_arg = args.path.as_deref().unwrap_or(".");
    let root = resolve_in(base, root_arg)?;
    anyhow::ensure!(root.is_dir(), "'{}' is not a directory", root_arg);
    let include_hidden = args.include_hidden.unwrap_or(true);
    let ignore = crate::ignore::IgnoreRules::load(base);

    let entries =
        fs::read_dir(&root).with_context(|| format!("cannot read directory '{}'", root_arg))?;
    let mut names: Vec<(String, bool)> = entries
        .filter_map(|e| e.ok())
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let rel = display_rel(base, &e.path());
            // Honor ignore rules (e.g. .gitignore) for both files and dirs.
            if ignore.matches(&rel, is_dir) {
                return None;
            }
            Some((name, is_dir))
        })
        .flatten()
        .filter(|(name, _)| include_hidden || !name.starts_with('.'))
        .collect();
    names.sort_by(|a, b| a.0.cmp(&b.0));
    if names.is_empty() {
        return Ok(format!("(empty: {})", root_arg));
    }
    let mut lines = Vec::with_capacity(names.len() + 1);
    lines.push(format!("{} entries in {}:", names.len(), root_arg));
    for (name, is_dir) in &names {
        let marker = if *is_dir { '/' } else { ' ' };
        lines.push(format!("  {name}{marker}"));
    }
    Ok(lines.join("\n"))
}

fn grep(base: &Path, args: &GrepArgs) -> Result<String> {
    let max_matches = args
        .max_matches
        .unwrap_or(DEFAULT_GREP_MATCHES)
        .clamp(1, MAX_GREP_MATCHES);
    let re =
        Regex::new(&args.pattern).with_context(|| format!("invalid regex '{}'", args.pattern))?;

    let root_arg = args.path.as_deref().unwrap_or(".");
    let root = resolve_in(base, root_arg)?;
    let files = if root.is_dir() {
        walk_files(base, &root)
    } else {
        vec![root.clone()]
    };

    let mut matches = Vec::new();
    for file in files {
        if matches.len() >= max_matches {
            break;
        }
        let Ok(meta) = fs::metadata(&file) else {
            continue;
        };
        if !meta.is_file() || meta.len() > MAX_INPUT_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = fs::read(&file) else { continue };
        if bytes.contains(&0) {
            continue; // binary
        }
        let rel = display_rel(base, &file);
        for (i, line) in String::from_utf8_lossy(&bytes).lines().enumerate() {
            if re.is_match(line) {
                let snippet: String = line.trim().chars().take(200).collect();
                matches.push(format!("{rel}:{}: {snippet}", i + 1));
                if matches.len() >= max_matches {
                    matches.push(format!(
                        "…(stopped at {max_matches} matches — refine the pattern or raise max_matches)"
                    ));
                    break;
                }
            }
        }
    }
    anyhow::ensure!(
        !matches.is_empty(),
        "no matches for '{}' under '{}'",
        args.pattern,
        root_arg
    );
    Ok(matches.join("\n"))
}

/// `explain_code`: returns one symbol's source block, or an outline plus
/// the head of the file when no symbol is named. Read-only; slices are
/// derived from the same regex extraction as the symbol index.
fn explain_code(base: &Path, args: &ExplainCodeArgs) -> Result<String> {
    let path = resolve_in(base, &args.path)?;
    let meta = fs::metadata(&path).with_context(|| format!("cannot stat '{}'", args.path))?;
    anyhow::ensure!(meta.is_file(), "'{}' is not a regular file", args.path);
    anyhow::ensure!(
        meta.len() <= MAX_INPUT_FILE_BYTES,
        "'{}' is larger than {} MB",
        args.path,
        MAX_INPUT_FILE_BYTES / (1024 * 1024)
    );
    let bytes = fs::read(&path).with_context(|| format!("cannot read '{}'", args.path))?;
    anyhow::ensure!(
        !bytes.contains(&0),
        "'{}' looks binary (contains NUL bytes)",
        args.path
    );
    let text = String::from_utf8_lossy(&bytes);
    let total_lines = text.lines().count();
    let max_lines = args.max_lines.unwrap_or(120).clamp(1, 2000);

    let slice = |start: usize, end: usize| -> String {
        let stop = end
            .min(total_lines + 1)
            .min(start.saturating_add(max_lines));
        text.lines()
            .skip(start - 1)
            .take(stop - start)
            .enumerate()
            .map(|(i, line)| format!("{:>5}| {}", start + i, line))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let Some(name) = args
        .symbol
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        // File overview: outline (when recognized) plus the head of the file.
        let mut out = String::new();
        if let Some(lang) = crate::outline::detect_language(&args.path) {
            let o = crate::outline::outline(lang, &text);
            if !o.is_empty() {
                out.push_str(&o);
            }
        }
        out.push_str(&format!(
            "[file: {} — {total_lines} lines total]\n",
            args.path
        ));
        out.push_str(&slice(1, max_lines + 1));
        return Ok(out);
    };

    // Locate the symbol in this file: prefer the global index (fresh from a
    // scan), falling back to on-the-fly extraction of just this file.
    let cwd_index = crate::symbols::current();
    let local = cwd_index.as_ref().and_then(|idx| {
        idx.find(name, None)
            .into_iter()
            .find(|s| s.file == args.path.replace('\\', "/"))
            .map(|s| (s.kind.to_owned(), s.name.clone(), s.line))
    });
    let located = match local {
        Some(hit) => Some(hit),
        None => crate::outline::detect_language(&args.path).and_then(|lang| {
            crate::outline::symbols(lang, &text)
                .into_iter()
                .find(|s| s.label == name || s.label.contains(name))
                .map(|s| (s.kind.to_owned(), s.label, s.line))
        }),
    };
    let Some((kind, label, start)) = located else {
        bail!(
            "symbol '{name}' not found in '{}' — try grep first to locate it",
            args.path
        );
    };

    // The block runs until the next indexed symbol at or above its own
    // definition level; a trailing cap keeps runaway blocks bounded.
    let next_line = cwd_index
        .as_ref()
        .and_then(|idx| {
            idx.symbols
                .iter()
                .filter(|s| s.file == args.path.replace('\\', "/") && s.line > start)
                .map(|s| s.line)
                .min()
        })
        .unwrap_or(total_lines + 1);

    Ok(format!(
        "[{kind} {label} — {}:{start}]\n{}\n[end of block]",
        args.path,
        slice(start, next_line)
    ))
}

/// `git_commit`: optionally stages everything, then commits with the given
/// message. Confirmation gating happens at the registry layer.
async fn git_commit_tool(base: &Path, args: &GitCommitArgs) -> Result<String> {
    let message = args.message.trim();
    anyhow::ensure!(!message.is_empty(), "commit message must not be empty");
    anyhow::ensure!(
        message.chars().count() <= 500,
        "commit message too long (cap 500 chars)"
    );
    if args.stage_all.unwrap_or(true) {
        crate::git::run_git(base, &["add", "-A"]).await?;
    }
    let text = crate::git::run_git(base, &["commit", "-m", message]).await?;
    Ok(format!("committed:\n{text}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that call `set_current_dir` — the process-wide
    /// working directory is shared, so parallel tests would race otherwise.
    fn cwd_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // -- staged edits --------------------------------------------------------

    fn replace_op(old: &str, new: &str) -> EditOp {
        EditOp::Replace {
            path: "f.txt".into(),
            old_string: old.into(),
            new_string: new.into(),
        }
    }

    #[test]
    fn staged_replace_applies_exactly_once() {
        let op = replace_op("b", "BETA");
        let refs = [&op];
        let out = apply_ops_to_content("a\nb\nc\nb\n", "f.txt", &refs).unwrap_err();
        assert!(out.to_string().contains("2 times"), "{out}");
        let op = replace_op("a\nb", "x");
        let refs = [&op];
        let out = apply_ops_to_content("a\nb\nc\n", "f.txt", &refs).unwrap();
        assert_eq!(out, "x\nc\n");
    }

    #[test]
    fn staged_replace_rejects_missing_and_empty_search() {
        let op = replace_op("zzz", "y");
        let refs = [&op];
        let err = apply_ops_to_content("abc\n", "f.txt", &refs).unwrap_err();
        assert!(err.to_string().contains("0 times"), "{err}");
        let op = EditOp::Replace {
            path: "f.txt".into(),
            old_string: String::new(),
            new_string: "y".into(),
        };
        let refs = [&op];
        assert!(apply_ops_to_content("abc\n", "f.txt", &refs).is_err());
    }

    #[test]
    fn staged_inserts_honor_line_anchors_and_bounds() {
        let mk = |line: usize, after: bool, text: &str| EditOp::Insert {
            path: "f.txt".into(),
            line,
            text: text.into(),
            after,
        };
        let op = mk(1, true, "first!");
        let out = apply_ops_to_content("a\nb\n", "f.txt", &[&op]).unwrap();
        assert_eq!(out, "a\nfirst!\nb\n");
        let op = mk(1, false, "top");
        let out = apply_ops_to_content("a\nb\n", "f.txt", &[&op]).unwrap();
        assert_eq!(out, "top\na\nb\n");
        // after the last line is fine; past it is not
        assert!(apply_ops_to_content("a\n", "f.txt", &[&mk(1, true, "x")]).is_ok());
        assert!(apply_ops_to_content("a\n", "f.txt", &[&mk(2, false, "x")]).is_ok());
        assert!(
            apply_ops_to_content("a\n", "f.txt", &[&mk(3, true, "x")])
                .unwrap_err()
                .to_string()
                .contains("after line 3")
        );
    }

    #[test]
    fn overlapping_replaces_conflict() {
        let content = "hello world hello\n";
        let r1 = replace_op("hello world", "X");
        let r2 = replace_op("world hello", "Y");
        let refs = [&r1, &r2];
        let err = apply_ops_to_content(content, "f.txt", &refs).unwrap_err();
        assert!(err.to_string().contains("overlap"), "{err}");
    }

    #[test]
    fn duplicate_insert_anchor_is_a_conflict() {
        let i1 = EditOp::Insert {
            path: "f.txt".into(),
            line: 2,
            text: "one".into(),
            after: true,
        };
        let i2 = EditOp::Insert {
            path: "f.txt".into(),
            line: 2,
            text: "two".into(),
            after: true,
        };
        let refs = [&i1, &i2];
        let err = apply_ops_to_content("a\nb\n", "f.txt", &refs).unwrap_err();
        assert!(err.to_string().contains("same point"), "{err}");
    }

    #[test]
    fn multiple_inserts_apply_bottom_up() {
        let i1 = EditOp::Insert {
            path: "f.txt".into(),
            line: 1,
            text: "top".into(),
            after: true,
        };
        let i2 = EditOp::Insert {
            path: "f.txt".into(),
            line: 3,
            text: "bottom".into(),
            after: true,
        };
        let refs = [&i1, &i2];
        let out = apply_ops_to_content("a\nb\nc\n", "f.txt", &refs).unwrap();
        assert_eq!(out, "a\ntop\nb\nc\nbottom\n");
    }

    #[test]
    fn staged_diff_previews_pending_edits() {
        let ws = TempWs::new("stagediff");
        fs::write(ws.0.join("f.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let op = EditOp::Replace {
            path: "f.txt".into(),
            old_string: "beta".into(),
            new_string: "BETA".into(),
        };
        let diff = staged_diff(&ws.0, &[op]).unwrap();
        assert!(diff.contains("--- a/f.txt"), "{diff}");
        assert!(diff.contains("-beta\n+BETA\n"), "{diff}");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn edit_tools_stage_through_executor_without_writes() {
        let _guard = cwd_guard();
        let orig = std::env::current_dir().unwrap();
        let ws = TempWs::new("stageexec");
        std::env::set_current_dir(&ws.0).unwrap();
        fs::write(ws.0.join("code.rs"), "fn main() {}\n").unwrap();
        let tools = BuiltinTools::default();

        let out = tools
            .execute(
                "edit_file",
                r#"{"path":"code.rs","old_string":"fn main() {}","new_string":"fn main() {\n    println!(\"hi\");\n}"}"#,
            )
            .await
            .unwrap();
        assert!(out.contains("\"staged\":true"), "{out}");
        // Nothing written yet.
        assert_eq!(
            fs::read_to_string(ws.0.join("code.rs")).unwrap(),
            "fn main() {}\n"
        );

        let pending = tools.pending_edits();
        assert_eq!(pending.lock().unwrap().ops().len(), 1);
        let diff = tools.execute("view_diff", "{}").await.unwrap();
        assert!(diff.contains("+    println!"), "{diff}");

        pending.lock().unwrap().clear();
        let _ = std::env::set_current_dir(orig);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn edit_file_rejects_nonunique_match_at_stage_time() {
        let _guard = cwd_guard();
        let orig = std::env::current_dir().unwrap();
        let ws = TempWs::new("stagedup");
        std::env::set_current_dir(&ws.0).unwrap();
        fs::write(ws.0.join("dup.txt"), "same\nsame\n").unwrap();
        let tools = BuiltinTools::default();
        let err = tools
            .execute(
                "edit_file",
                r#"{"path":"dup.txt","old_string":"same","new_string":"x"}"#,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("2 times"), "{err}");
        let _ = std::env::set_current_dir(orig);
    }

    #[test]
    fn parse_edit_op_builds_expected_variants() {
        let op = parse_edit_op("insert_after", r#"{"path":"a.rs","line":3,"text":"hi"}"#).unwrap();
        match op {
            EditOp::Insert { line, after, .. } => {
                assert_eq!(line, 3);
                assert!(after);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(parse_edit_op("edit_file", r#"{"path":"a"}"#).is_err());
        assert!(parse_edit_op("view_diff", "{}").is_err());
    }

    #[tokio::test]
    async fn run_shell_captures_output() {
        let args = RunShellArgs {
            command: "echo hello-run-shell".into(),
            timeout_secs: Some(10),
        };
        let out = run_shell_command(args).await.unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["exit_code"], 0);
        assert!(
            parsed["stdout"]
                .as_str()
                .unwrap()
                .contains("hello-run-shell"),
            "{out}"
        );
    }

    #[tokio::test]
    async fn run_shell_reports_nonzero_exit_as_data() {
        let args = RunShellArgs {
            command: if cfg!(windows) {
                "cmd /C exit 3".into()
            } else {
                "false".into()
            },
            timeout_secs: None,
        };
        let out = run_shell_command(args).await.unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["exit_code"], 3);
    }

    #[tokio::test]
    async fn run_shell_enforces_timeout_and_rejects_bad_input() {
        let args = RunShellArgs {
            command: if cfg!(windows) {
                "ping -n 30 127.0.0.1".into()
            } else {
                "sleep 30".into()
            },
            timeout_secs: Some(1),
        };
        let err = run_shell_command(args).await.unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
        let empty = RunShellArgs {
            command: "   ".into(),
            timeout_secs: None,
        };
        assert!(run_shell_command(empty).await.is_err());
    }

    #[test]
    fn test_command_builds_runner_argv_per_project_kind() {
        let (prog, argv) = test_command(ProjectKind::Rust, Some("  my_test  "));
        assert_eq!(prog, "cargo");
        assert_eq!(argv, vec!["test", "my_test"]);
        let (_, argv) = test_command(ProjectKind::Rust, None);
        assert_eq!(argv, vec!["test"]);
        let (_, argv) = test_command(ProjectKind::Node, Some("api"));
        assert_eq!(argv, vec!["test", "--", "api"]);
        let (_, argv) = test_command(ProjectKind::Python, None);
        assert_eq!(argv, vec!["-m", "pytest"]);
        let (_, argv) = test_command(ProjectKind::Python, Some("fast"));
        assert_eq!(argv, vec!["-m", "pytest", "-k", "fast"]);
    }

    #[test]
    fn check_command_builds_validation_argv_per_project_kind() {
        let (prog, argv) = check_command(ProjectKind::Rust).unwrap();
        assert_eq!(prog, "cargo");
        assert_eq!(argv, vec!["check"]);
        let (prog, argv) = check_command(ProjectKind::Python).unwrap();
        assert_eq!(prog, "python");
        assert_eq!(argv, vec!["-m", "mypy", "."]);
        // Node validation requires a tsconfig.json next to package.json.
        if Path::new("tsconfig.json").is_file() {
            assert!(check_command(ProjectKind::Node).is_ok());
        } else {
            let err = check_command(ProjectKind::Node).unwrap_err();
            assert!(err.to_string().contains("tsconfig"), "{err}");
        }
    }

    #[test]
    fn execution_tool_names_need_confirmation() {
        let tools = BuiltinTools::default();
        for name in [
            "write_file",
            "delete_file",
            "move_file",
            "copy_file",
            "run_shell",
            "run_test",
            "run_diagnostics",
            "open_preview",
            "git_commit",
            "git_branch",
            "apply_edits",
            "forget",
        ] {
            assert!(tools.requires_confirmation(name), "{name}");
        }
        // Staging edits only queues — no confirmation gate needed.
        for name in ["edit_file", "insert_after", "insert_before", "view_diff"] {
            assert!(!tools.requires_confirmation(name), "{name}");
        }
    }

    #[tokio::test]
    async fn current_time_returns_timestamp() {
        let out = BuiltinTools::default()
            .execute("current_time", "{}")
            .await
            .unwrap();
        assert!(out.contains('T'), "expected ISO-8601, got {out}");
    }

    #[tokio::test]
    async fn count_tokens_measures_text() {
        let out = BuiltinTools::default()
            .execute("count_tokens", r#"{"text":"hello big world"}"#)
            .await
            .unwrap();
        // cl100k token count for this short text; just check the JSON shape.
        assert!(
            out.starts_with(r#"{"tokens":"#) || out.starts_with(r#"{"tokens":""#),
            "unexpected output: {out}"
        );
        assert!(out.contains(r#""characters":15}"#), "{out}");
    }

    #[tokio::test]
    async fn malformed_arguments_error_cleanly() {
        let tools = BuiltinTools::default();
        assert!(tools.execute("count_tokens", "{oops").await.is_err());
        assert!(tools.execute("count_tokens", "{}").await.is_err());
    }

    #[tokio::test]
    async fn unknown_tool_errors() {
        assert!(
            BuiltinTools::default()
                .execute("rm_rf", "{}")
                .await
                .is_err()
        );
    }

    #[test]
    fn specs_have_unique_names_and_object_schemas() {
        let specs = BuiltinTools::default().specs();
        let mut names: Vec<_> = specs.iter().map(|t| t.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), specs.len());
        for s in &specs {
            assert_eq!(s.parameters["type"], "object");
        }
    }

    #[test]
    fn only_write_needs_confirmation() {
        let tools = BuiltinTools::default();
        assert!(tools.requires_confirmation("write_file"));
        for t in ["read_file", "list_files", "grep", "current_time"] {
            assert!(!tools.requires_confirmation(t), "{t}");
        }
    }

    // -- sandbox ------------------------------------------------------------

    #[test]
    fn sandbox_rejects_escape_attempts() {
        let base = Path::new("/base");
        assert!(resolve_in(base, "../x").is_err());
        assert!(resolve_in(base, "a/../../x").is_err());
        assert!(resolve_in(base, "/etc/passwd").is_err());
        assert!(resolve_in(base, r"C:\tmp\x").is_err());
        assert!(resolve_in(base, "").is_err());
        assert_eq!(
            resolve_in(base, "src/lib.rs").unwrap(),
            base.join("src/lib.rs")
        );
    }

    // -- workspace tools against a temp dir ----------------------------------

    struct TempWs(PathBuf);
    impl TempWs {
        fn new(tag: &str) -> Self {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "govinda-test-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempWs {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn read_write_roundtrip_with_line_numbers() {
        let ws = TempWs::new("rw");
        let write_args = WriteFileArgs {
            path: "sub/a.txt".into(),
            content: "one\ntwo\nthree\n".into(),
        };
        let out = write_file(&ws.0, &write_args).unwrap();
        assert!(out.contains("\"bytes\":14"), "{out}");
        assert!(out.contains("\"created\":true"), "{out}");

        let read_args = ReadFileArgs {
            path: "sub/a.txt".into(),
            offset_line: Some(2),
            max_lines: Some(1),
            include_outline: None,
        };
        let out = read_file(&ws.0, &read_args).unwrap();
        assert!(out.starts_with("    2| two"), "{out}");
        assert!(out.contains("offset_line 3"), "{out}");
    }

    #[test]
    fn read_file_reports_continuation() {
        let ws = TempWs::new("cont");
        let content: String = (1..=10).map(|i| format!("line{i}\n")).collect();
        write_file(
            &ws.0,
            &WriteFileArgs {
                path: "c.txt".into(),
                content,
            },
        )
        .unwrap();
        let out = read_file(
            &ws.0,
            &ReadFileArgs {
                path: "c.txt".into(),
                offset_line: None,
                max_lines: Some(3),
                include_outline: None,
            },
        )
        .unwrap();
        assert!(out.contains("7 more lines"), "{out}");
        assert!(out.contains("offset_line 4"), "{out}");
    }

    #[test]
    fn read_file_rejects_binary_and_missing() {
        let ws = TempWs::new("bin");
        fs::write(ws.0.join("b.bin"), [0u8, 1, 2]).unwrap();
        let err = read_file(
            &ws.0,
            &ReadFileArgs {
                path: "b.bin".into(),
                offset_line: None,
                max_lines: None,
                include_outline: None,
            },
        );
        assert!(err.unwrap_err().to_string().contains("binary"));
        assert!(
            read_file(
                &ws.0,
                &ReadFileArgs {
                    path: "nope.txt".into(),
                    offset_line: None,
                    max_lines: None,
                    include_outline: None,
                },
            )
            .is_err()
        );
    }

    #[test]
    fn read_file_prepends_symbol_outline_for_source_files() {
        let ws = TempWs::new("outline");
        let src = "use std::io;\n\nfn main() {\n    helper();\n}\n\nfn helper() {}\n";
        write_file(
            &ws.0,
            &WriteFileArgs {
                path: "app.rs".into(),
                content: src.into(),
            },
        )
        .unwrap();

        let args = |outline: Option<bool>| ReadFileArgs {
            path: "app.rs".into(),
            offset_line: None,
            max_lines: None,
            include_outline: outline,
        };

        let with = read_file(&ws.0, &args(None)).unwrap();
        assert!(with.contains("[outline]"), "{with}");
        assert!(with.contains("| fn main"), "{with}");
        assert!(with.contains("| fn helper"), "{with}");
        assert!(with.contains("[file: app.rs — 7 lines total]"), "{with}");

        let without = read_file(&ws.0, &args(Some(false))).unwrap();
        assert!(!without.contains("[outline]"), "{without}");
    }

    #[test]
    fn govindaignore_hides_files_from_list_and_grep() {
        let ws = TempWs::new("ignore");
        fs::write(ws.0.join(".govindaignore"), "secrets/\n*.tmp\n").unwrap();
        fs::create_dir_all(ws.0.join("secrets")).unwrap();
        fs::write(ws.0.join("secrets/key.txt"), "hidden token here\n").unwrap();
        fs::write(ws.0.join("cache.tmp"), "junk\n").unwrap();
        fs::write(ws.0.join("visible.txt"), "token here\n").unwrap();

        let listed = list_files(
            &ws.0,
            &ListFilesArgs {
                path: None,
                max_entries: None,
                max_depth: None,
            },
        )
        .unwrap();
        assert!(listed.contains("visible.txt"), "{listed}");
        assert!(!listed.contains("secrets/"), "{listed}");
        assert!(!listed.contains("cache.tmp"), "{listed}");

        let hits = grep(
            &ws.0,
            &GrepArgs {
                pattern: "token".into(),
                path: None,
                max_matches: None,
                include_globs: None,
                case_sensitive: None,
            },
        )
        .unwrap();
        assert!(hits.contains("visible.txt"), "{hits}");
        assert!(!hits.contains("secrets"), "{hits}");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn scan_project_runs_through_executor() {
        // scan_project refreshes the shared symbol index; hold the global
        // guard so parallel index tests don't interleave rebuilds.
        let _index_guard = crate::symbols::tests::global_guard();
        let tools = BuiltinTools::default();
        let out = tools.execute("scan_project", "{}").await.unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["files"]["total"].is_u64(), "{out}");
        assert!(!tools.requires_confirmation("scan_project"));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn find_symbol_and_explain_code_work_through_executor() {
        let _guard = cwd_guard();
        // The executor reads the process-global symbol index; keep other
        // index-mutating tests from swapping it mid-run. Held across awaits
        // on purpose — no other test may rebuild while this one runs.
        let _index_guard = crate::symbols::tests::global_guard();
        let orig = std::env::current_dir().unwrap();
        let ws = TempWs::new("symexec");
        std::env::set_current_dir(&ws.0).unwrap();
        // Start from a clean slate so ensure() builds THIS workspace's index.
        crate::symbols::tests::reset_global();
        fs::write(
            ws.0.join("lib.rs"),
            "pub struct Widget;\n\npub fn build() -> Widget {\n    Widget\n}\n",
        )
        .unwrap();
        let tools = BuiltinTools::default();

        // find_symbol locates definitions with kind/file/line
        let out = tools
            .execute("find_symbol", r#"{"name":"Widget"}"#)
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["matches"], 1, "{out}");
        assert_eq!(parsed["symbols"][0]["kind"], "struct");
        assert_eq!(parsed["symbols"][0]["file"], "lib.rs");

        // kind filter excludes non-matching kinds
        let out = tools
            .execute("find_symbol", r#"{"name":"Widget","kind":"trait"}"#)
            .await
            .unwrap();
        assert!(out.contains("\"matches\":0"), "{out}");

        // explain_code extracts one symbol's block with line numbers
        let out = tools
            .execute("explain_code", r#"{"path":"lib.rs","symbol":"build"}"#)
            .await
            .unwrap();
        assert!(out.contains("[function build — lib.rs:3]"), "{out}");
        assert!(out.contains("pub fn build() -> Widget {"), "{out}");
        assert!(out.contains("[end of block]"), "{out}");

        // explain_code without a symbol gives a file overview
        let out = tools
            .execute("explain_code", r#"{"path":"lib.rs"}"#)
            .await
            .unwrap();
        assert!(
            out.contains("[outline]") || out.contains("[file: lib.rs"),
            "{out}"
        );
        assert!(
            tools
                .execute("find_symbol", r#"{"name":42}"#)
                .await
                .is_err(),
            "malformed args must error"
        );
        crate::symbols::tests::reset_global();
        let _ = std::env::set_current_dir(orig);
    }

    #[tokio::test]
    async fn git_tools_validate_input_and_gate_confirmation() {
        let tools = BuiltinTools::default();
        assert!(tools.requires_confirmation("git_commit"));
        assert!(tools.requires_confirmation("git_branch"));
        for name in ["git_diff", "git_log"] {
            assert!(!tools.requires_confirmation(name), "{name}");
        }
        // Unknown branch action fails before spawning git.
        let err = tools
            .execute("git_branch", r#"{"action":"rebase"}"#)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown branch action"), "{err}");
        // Empty commit message is rejected without touching git.
        let err = tools
            .execute("git_commit", r#"{"message":"   "}"#)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("must not be empty"), "{err}");
    }

    #[test]
    fn explain_code_rejects_escaping_paths_and_missing_symbols() {
        let ws = TempWs::new("explainsandbox");
        fs::write(ws.0.join("a.rs"), "fn ok_fn() {}\n").unwrap();
        let err = explain_code(
            &ws.0,
            &ExplainCodeArgs {
                path: "../secrets.txt".into(),
                symbol: None,
                max_lines: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("not allowed"), "{err}");

        fs::write(ws.0.join("b.rs"), "fn present_fn() {}\n").unwrap();
        let err = explain_code(
            &ws.0,
            &ExplainCodeArgs {
                path: "b.rs".into(),
                symbol: Some("absent_fn".into()),
                max_lines: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("not found in"), "{err}");
    }

    #[test]
    fn write_file_rejects_oversize_content() {
        let ws = TempWs::new("big");
        let big = "x".repeat(MAX_WRITE_BYTES + 1);
        assert!(
            write_file(
                &ws.0,
                &WriteFileArgs {
                    path: "big.txt".into(),
                    content: big,
                },
            )
            .is_err()
        );
    }

    #[test]
    fn list_files_walks_and_marks_dirs() {
        let ws = TempWs::new("ls");
        fs::create_dir_all(ws.0.join("src/nested")).unwrap();
        fs::create_dir_all(ws.0.join(".git")).unwrap();
        fs::write(ws.0.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(ws.0.join(".git/config"), "x").unwrap();
        let out = list_files(
            &ws.0,
            &ListFilesArgs {
                path: None,
                max_entries: None,
                max_depth: None,
            },
        )
        .unwrap();
        assert!(out.contains("src/"), "{out}");
        assert!(out.contains("src/nested/"), "{out}");
        assert!(out.contains("src/main.rs"), "{out}");
        assert!(!out.contains(".git"), "{out}");
    }

    #[test]
    fn list_files_caps_entries() {
        let ws = TempWs::new("cap");
        for i in 0..20 {
            fs::write(ws.0.join(format!("f{i}.txt")), "x").unwrap();
        }
        let out = list_files(
            &ws.0,
            &ListFilesArgs {
                path: None,
                max_entries: Some(5),
                max_depth: None,
            },
        )
        .unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 6, "{out}");
        assert!(lines[..5].iter().all(|l| l.starts_with("f")), "{out}");
        assert!(lines[5].contains("capped"), "{out}");
    }

    #[test]
    fn grep_finds_regex_matches_with_locations() {
        let ws = TempWs::new("grep");
        fs::write(ws.0.join("a.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();
        fs::write(ws.0.join("b.txt"), "alpha here\nnothing\n").unwrap();
        fs::write(ws.0.join("skip.bin"), [0u8]).unwrap();
        let out = grep(
            &ws.0,
            &GrepArgs {
                pattern: r"fn \w+".into(),
                path: None,
                max_matches: None,
                include_globs: None,
                case_sensitive: None,
            },
        )
        .unwrap();
        assert!(out.contains("a.rs:1:"), "{out}");
        assert!(out.contains("a.rs:2:"), "{out}");
        assert!(!out.contains("skip.bin"), "{out}");
        // no-match surfaces a clean error, not a panic
        assert!(
            grep(
                &ws.0,
                &GrepArgs {
                    pattern: "zzz-nowhere".into(),
                    path: None,
                    max_matches: None,
                    include_globs: None,
                    case_sensitive: None,
                },
            )
            .is_err()
        );
    }

    #[test]
    fn grep_invalid_regex_errors_cleanly() {
        let ws = TempWs::new("badre");
        let err = grep(
            &ws.0,
            &GrepArgs {
                pattern: "(unclosed".into(),
                path: None,
                max_matches: None,
                include_globs: None,
                case_sensitive: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid regex"));
    }

    #[tokio::test]
    async fn workspace_tools_respect_sandbox_via_execute() {
        // End-to-end through the trait: escaping paths must fail even when
        // they arrive from the model.
        let tools = BuiltinTools::default();
        assert!(
            tools
                .execute("read_file", r#"{"path":"../../Cargo.toml"}"#)
                .await
                .is_err()
        );
        assert!(
            tools
                .execute("list_files", r#"{"path":"/"}"#)
                .await
                .is_err()
        );
        assert!(
            tools
                .execute("grep", r#"{"pattern":"x","path":"C:\\"}"#)
                .await
                .is_err()
        );
    }

    // -- user-defined shell tools ---------------------------------------------

    fn echo_def() -> ShellToolDef {
        ShellToolDef {
            name: "echo_it".into(),
            description: "echoes a message".into(),
            command: if cfg!(windows) { "cmd" } else { "echo" }.into(),
            args_template: if cfg!(windows) {
                vec!["/C".into(), "echo".into(), "{msg}!".into()]
            } else {
                vec!["{msg}!".into()]
            },
            timeout_secs: None,
            max_output_bytes: None,
        }
    }

    #[test]
    fn validation_rejects_bad_definitions() {
        let mk = |name: &str, command: &str| ShellToolDef {
            name: name.into(),
            description: "d".into(),
            command: command.into(),
            args_template: vec![],
            timeout_secs: None,
            max_output_bytes: None,
        };
        assert!(validate_shell_tools(&[mk("good_name", "x")]).is_ok());
        assert!(validate_shell_tools(&[mk("Bad Name", "x")]).is_err());
        assert!(validate_shell_tools(&[mk("1starts_digit", "x")]).is_err());
        assert!(validate_shell_tools(&[mk("", "x")]).is_err());
        assert!(validate_shell_tools(&[mk("dup", "x"), mk("dup", "y")]).is_err());
        // built-in shadowing
        for builtin in BUILTIN_TOOL_NAMES {
            assert!(
                validate_shell_tools(&[mk(builtin, "x")]).is_err(),
                "{builtin}"
            );
        }
        // bounds
        let mut t = mk("timed", "x");
        t.timeout_secs = Some(0);
        assert!(validate_shell_tools(&[t.clone()]).is_err());
        t.timeout_secs = Some(601);
        assert!(validate_shell_tools(&[t.clone()]).is_err());
        t.timeout_secs = Some(600);
        assert!(validate_shell_tools(&[t]).is_ok());
        let mut o = mk("capped", "x");
        o.max_output_bytes = Some(MAX_SHELL_OUTPUT_BYTES + 1);
        assert!(validate_shell_tools(&[o]).is_err());
    }

    #[test]
    fn schema_exposes_unique_required_placeholders() {
        let def = ShellToolDef {
            name: "gh".into(),
            description: "d".into(),
            command: "gh".into(),
            args_template: vec!["pr".into(), "view".into(), "{repo}".into(), "{repo}".into()],
            timeout_secs: None,
            max_output_bytes: None,
        };
        let schema = shell_tool_schema(&def);
        let props = schema["properties"].as_object().unwrap();
        assert_eq!(props.len(), 1, "{schema}");
        assert_eq!(props["repo"]["type"], "string");
        assert_eq!(schema["required"], serde_json::json!(["repo"]));
    }

    #[test]
    fn fill_word_substitutes_and_rejects_missing_or_huge_values() {
        let mut values = HashMap::new();
        values.insert("who".to_owned(), Value::String("world".to_owned()));
        assert_eq!(
            fill_word("hello {who}, bye {who}", &values).unwrap(),
            "hello world, bye world"
        );
        // literal braces stay literal
        assert_eq!(fill_word("a {b c} {}", &values).unwrap(), "a {b c} {}");
        // missing key
        assert!(fill_word("{nope}", &values).is_err());
        // oversized value
        values.insert(
            "big".to_owned(),
            Value::String("x".repeat(MAX_ARG_VALUE_CHARS + 1)),
        );
        assert!(fill_word("{big}", &values).is_err());
        // NUL byte rejected
        values.insert("nul".to_owned(), Value::String("a\u{0}b".to_owned()));
        assert!(fill_word("{nul}", &values).is_err());
        // non-string scalars stringify
        values.insert("n".to_owned(), serde_json::json!(42));
        assert_eq!(fill_word("n={n}", &values).unwrap(), "n=42");
    }

    #[tokio::test]
    async fn shell_tool_runs_and_captures_output() {
        let tools = BuiltinTools::new(vec![echo_def()]);
        assert!(tools.requires_confirmation("echo_it"));
        let specs = tools.specs();
        assert!(specs.iter().any(|s| s.name == "echo_it"));
        let out = tools
            .execute("echo_it", r#"{"msg":"hi there"}"#)
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["exit_code"], 0);
        assert!(
            parsed["stdout"].as_str().unwrap().contains("hi there!"),
            "{out}"
        );
    }

    #[tokio::test]
    async fn shell_tool_timeout_kills_runaway_command() {
        let def = ShellToolDef {
            name: "slowpoke".into(),
            description: "sleeps forever".into(),
            command: if cfg!(windows) { "ping" } else { "sleep" }.into(),
            args_template: if cfg!(windows) {
                vec!["-n".into(), "30".into(), "127.0.0.1".into()]
            } else {
                vec!["30".into()]
            },
            timeout_secs: Some(1),
            max_output_bytes: None,
        };
        validate_shell_tools(std::slice::from_ref(&def)).unwrap();
        let err = run_shell_tool(&def, "{}").await.unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
    }

    #[tokio::test]
    async fn shell_tool_spawn_failure_errors_cleanly() {
        let def = ShellToolDef {
            name: "missing_prog_xyz".into(),
            description: "does not exist".into(),
            command: "definitely-not-a-real-program-xyz".into(),
            args_template: vec![],
            timeout_secs: None,
            max_output_bytes: None,
        };
        let err = run_shell_tool(&def, "{}").await.unwrap_err();
        assert!(err.to_string().contains("cannot spawn"), "{err}");
    }

    #[test]
    fn capped_lossy_truncates_with_marker() {
        let full = b"abcdefgh".to_vec();
        assert_eq!(capped_lossy(&full, 100), "abcdefgh");
        let out = capped_lossy(&full, 4);
        assert!(out.starts_with("abcd"), "{out}");
        assert!(out.contains("truncated at 4 bytes"), "{out}");
    }

    #[test]
    fn disabled_tool_toggles_roundtrip() {
        // The toggle file resolves against the process cwd; serialize with
        // tests that change directories.
        let _guard = cwd_guard();
        let mut set = HashSet::new();
        set.insert("grep".to_owned());
        set.insert("write_file".to_owned());
        save_disabled_tools(&set).unwrap();
        let loaded = load_disabled_tools();
        assert_eq!(loaded, set);
        save_disabled_tools(&HashSet::new()).unwrap();
        assert!(load_disabled_tools().is_empty());
        std::fs::remove_file(disabled_tools_path()).ok();
    }

    #[test]
    fn list_files_respects_depth_cap() {
        // list_files should not exceed MAX_WALK_DEPTH (12)
        // This test verifies the depth cap is applied
        let base = std::env::temp_dir().join("govinda-depth-test");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("a/b/c/d/e/f/g/h/i/j/k/l/m")).unwrap();
        std::fs::write(base.join("a/b/c/d/e/f/g/h/i/j/k/l/m/deep.txt"), "x").unwrap();
        std::fs::write(base.join("a/top.txt"), "x").unwrap();

        let args = ListFilesArgs {
            path: Some("a".into()),
            max_entries: Some(100),
            max_depth: None,
        };
        // Should succeed without stack overflow
        let result = list_files(&base, &args);
        assert!(
            result.is_ok(),
            "list_files should handle deep paths: {result:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
