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
use crate::clock;
use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::OnceLock;
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
/// Names reserved by built-in implementations; user tools cannot shadow them.
const BUILTIN_TOOL_NAMES: [&str; 6] = [
    "current_time",
    "count_words",
    "read_file",
    "write_file",
    "list_files",
    "grep",
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

/// The default executor: safe local tools plus sandboxed workspace and
/// user-defined shell tools.
#[derive(Default)]
pub struct BuiltinTools {
    shell_tools: Vec<ShellToolDef>,
}

impl BuiltinTools {
    /// Builds an executor over validated user shell-tool definitions.
    /// Validation errors must already have been surfaced at config load;
    /// this constructor trusts its input.
    pub fn new(shell_tools: Vec<ShellToolDef>) -> Self {
        Self { shell_tools }
    }
}

impl ToolExecutor for BuiltinTools {
    fn specs(&self) -> Vec<Tool> {
        let mut specs = vec![
            Tool::new(
                "current_time",
                "Returns the user's current local date and time in ISO-8601 format.",
                serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "count_words",
                "Counts words and characters in the given text.",
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
                "read_file",
                "Reads a text file from the workspace with line numbers. Workspace-relative \
                 paths only; absolute paths and '..' are rejected.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Workspace-relative file path"},
                        "offset_line": {"type": "integer", "description": "1-based first line to return (default 1)"},
                        "max_lines": {"type": "integer", "description": "Maximum lines to return (default 2000)"}
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
                "list_files",
                "Lists files and directories under a workspace path (recursive). Common build \
                 dirs (.git, target, node_modules) are skipped.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Workspace-relative directory (default '.'); directories get a trailing '/'"},
                        "max_entries": {"type": "integer", "description": "Maximum entries to list (default 500)"}
                    },
                    "additionalProperties": false
                }),
            ),
            Tool::new(
                "grep",
                "Regex search across text files in the workspace; returns 'path:line: text' matches.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string", "description": "Rust regex pattern"},
                        "path": {"type": "string", "description": "Workspace-relative directory or file to search (default '.')"},
                        "max_matches": {"type": "integer", "description": "Maximum matches to return (default 50)"}
                    },
                    "required": ["pattern"],
                    "additionalProperties": false
                }),
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
        name == "write_file" || self.shell_tools.iter().any(|t| t.name == name)
    }

    fn execute<'a>(&'a self, name: &'a str, arguments_json: &'a str) -> ToolFuture<'a> {
        Box::pin(async move {
            match name {
                "current_time" => Ok(clock::now_iso8601()),
                "count_words" => {
                    let args: CountWordsArgs = parse_args(arguments_json)?;
                    Ok(format!(
                        "{{\"words\":{},\"characters\":{}}}",
                        args.text.split_whitespace().count(),
                        args.text.chars().count()
                    ))
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
                "list_files" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: ListFilesArgs = parse_args(arguments_json)?;
                    list_files(&cwd, &args)
                }
                "grep" => {
                    let cwd =
                        std::env::current_dir().context("cannot resolve working directory")?;
                    let args: GrepArgs = parse_args(arguments_json)?;
                    grep(&cwd, &args)
                }
                other => match self.shell_tools.iter().find(|t| t.name == other) {
                    Some(def) => run_shell_tool(def, arguments_json).await,
                    None => bail!("unknown tool '{other}'"),
                },
            }
        })
    }
}

#[derive(serde::Deserialize)]
struct CountWordsArgs {
    text: String,
}

#[derive(serde::Deserialize)]
struct ReadFileArgs {
    path: String,
    offset_line: Option<usize>,
    max_lines: Option<usize>,
}

#[derive(serde::Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

#[derive(serde::Deserialize)]
struct ListFilesArgs {
    path: Option<String>,
    max_entries: Option<usize>,
}

#[derive(serde::Deserialize)]
struct GrepArgs {
    pattern: String,
    path: Option<String>,
    max_matches: Option<usize>,
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
// Sandbox
// ---------------------------------------------------------------------------

/// Anchors a workspace-relative path under `base`, rejecting absolute paths,
/// rooted paths, and any `..` component. This is the single gate every
/// workspace tool passes through before touching the filesystem.
fn resolve_in(base: &Path, raw: &str) -> Result<PathBuf> {
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

/// Collects files up to `depth` levels below `root`, skipping build dirs.
fn walk_files(root: &Path) -> Vec<PathBuf> {
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
            let name = entry.file_name().to_string_lossy().to_string();
            if ft.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push((entry.path(), depth + 1));
                }
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

    let mut out = selected.join("\n");
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

fn write_file(base: &Path, args: &WriteFileArgs) -> Result<String> {
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

fn list_files(base: &Path, args: &ListFilesArgs) -> Result<String> {
    let root_arg = args.path.as_deref().unwrap_or(".");
    let root = resolve_in(base, root_arg)?;
    anyhow::ensure!(root.is_dir(), "'{}' is not a directory", root_arg);
    let max_entries = args
        .max_entries
        .unwrap_or(DEFAULT_LIST_ENTRIES)
        .clamp(1, MAX_LIST_ENTRIES);

    let mut lines = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        if lines.len() >= max_entries {
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
                    "…(output capped at {max_entries} entries — narrow the path)"
                ));
                break;
            }
            let Ok(ft) = entry.file_type() else { continue };
            let name = entry.file_name().to_string_lossy().to_string();
            if ft.is_dir() {
                if SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                lines.push(format!("{}/", display_rel(base, &entry.path())));
                stack.push(entry.path());
            } else if ft.is_file() {
                lines.push(display_rel(base, &entry.path()));
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
        walk_files(&root)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn current_time_returns_timestamp() {
        let out = BuiltinTools::default()
            .execute("current_time", "{}")
            .await
            .unwrap();
        assert!(out.contains('T'), "expected ISO-8601, got {out}");
    }

    #[tokio::test]
    async fn count_words_measures_text() {
        let out = BuiltinTools::default()
            .execute("count_words", r#"{"text":"hello big world"}"#)
            .await
            .unwrap();
        assert_eq!(out, r#"{"words":3,"characters":15}"#);
    }

    #[tokio::test]
    async fn malformed_arguments_error_cleanly() {
        let tools = BuiltinTools::default();
        assert!(tools.execute("count_words", "{oops").await.is_err());
        assert!(tools.execute("count_words", "{}").await.is_err());
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
                },
            )
            .is_err()
        );
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
}
