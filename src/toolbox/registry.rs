//! Registry that wires the toolbox tools into the BuiltinTools executor.

use std::path::Path;

/// Dispatch a toolbox tool by name. Returns `Some(...)` if handled,
/// `None` if the name is not a toolbox tool.
///
/// Async tools (http, git_diff_apply, screenshot, git_tools) are spawned via
/// `tokio::runtime::Handle::block_on` so the executor can stay sync.
pub fn dispatch(
    name: &str,
    arguments_json: &str,
    cwd: &Path,
) -> Option<anyhow::Result<String>> {
    let value: serde_json::Value = match serde_json::from_str(arguments_json) {
        Ok(v) => v,
        Err(e) => return Some(Err(anyhow::anyhow!("invalid JSON: {e}"))),
    };
    macro_rules! sync_with_cwd {
        ($mod:ident, $args_ty:ty) => {{
            let args: $args_ty = match serde_json::from_value(value) {
                Ok(a) => a,
                Err(e) => return Some(Err(anyhow::anyhow!("invalid args: {e}"))),
            };
            Some(super::$mod::run(cwd, args))
        }};
    }
    macro_rules! sync_no_cwd {
        ($mod:ident, $args_ty:ty) => {{
            let args: $args_ty = match serde_json::from_value(value) {
                Ok(a) => a,
                Err(e) => return Some(Err(anyhow::anyhow!("invalid args: {e}"))),
            };
            Some(super::$mod::run(args))
        }};
    }
    match name {
        // Original 18 tools
        "clipboard" => sync_with_cwd!(clipboard, super::clipboard::Args),
        "template" => sync_with_cwd!(template, super::template::Args),
        "package_install" => sync_with_cwd!(package, super::package::Args),
        "lint" => sync_with_cwd!(lint, super::lint::Args),
        "format" => sync_with_cwd!(formatter, super::formatter::Args),
        "build_project" => sync_with_cwd!(build, super::build::Args),
        "regex_search" => sync_with_cwd!(regex_search, super::regex_search::Args),
        "diff_apply" => sync_with_cwd!(diff_apply, super::diff_apply::Args),
        "template_fill" => sync_with_cwd!(template_fill, super::template_fill::Args),
        "scaffold_test" => sync_with_cwd!(scaffold_test, super::scaffold_test::Args),
        "memory_search" => sync_with_cwd!(memory_search, super::memory_search::Args),
        "image_view" => sync_with_cwd!(image_view, super::image_view::Args),
        "bulk_read" => sync_with_cwd!(bulk_read, super::bulk_read::Args),
        "bulk_crud" => sync_with_cwd!(bulk_crud, super::bulk_crud::Args),
        "bulk_shell" => sync_with_cwd!(bulk_shell, super::bulk_shell::Args),
        "docs" => sync_with_cwd!(docs, super::docs::Args),
        "format_setter" => sync_with_cwd!(format_setter, super::format_setter::Args),
        "code_format" => sync_with_cwd!(code_format, super::code_format::Args),
        "find_issues" => sync_with_cwd!(find_issues, super::find_issues::Args),
        "process_manager" => sync_no_cwd!(process, super::process::Args),
        "env" => sync_no_cwd!(env, super::env::Args),
        "json_query" => sync_no_cwd!(jsonquery, super::jsonquery::Args),
        "html" => sync_with_cwd!(html, super::html::Args),
        "parallel" => sync_with_cwd!(parallel, super::parallel::Args),
        // Async tools
        "http_request" => {
            let args: super::http::Args = match serde_json::from_value(value) {
                Ok(a) => a,
                Err(e) => return Some(Err(anyhow::anyhow!("invalid args: {e}"))),
            };
            Some(block_on(super::http::run(args)))
        }
        "git_diff_apply" => {
            let args: super::git_diff_apply::Args = match serde_json::from_value(value) {
                Ok(a) => a,
                Err(e) => return Some(Err(anyhow::anyhow!("invalid args: {e}"))),
            };
            Some(block_on(super::git_diff_apply::run(cwd, args)))
        }
        "screenshot" => {
            let args: super::screenshot::Args = match serde_json::from_value(value) {
                Ok(a) => a,
                Err(e) => return Some(Err(anyhow::anyhow!("invalid args: {e}"))),
            };
            Some(block_on(super::screenshot::run(cwd, args)))
        }
        "screenrec" => {
            let args: super::screenrec::Args = match serde_json::from_value(value) {
                Ok(a) => a,
                Err(e) => return Some(Err(anyhow::anyhow!("invalid args: {e}"))),
            };
            Some(block_on(super::screenrec::run(cwd, args)))
        }
        "git_tools" => {
            let args: super::git_tools::Args = match serde_json::from_value(value) {
                Ok(a) => a,
                Err(e) => return Some(Err(anyhow::anyhow!("invalid args: {e}"))),
            };
            Some(block_on(super::git_tools::run(cwd, args)))
        }
        _ => None,
    }
}

/// Block on an async function that returns `anyhow::Result<String>` using
/// the current tokio runtime, or spin up a one-shot runtime if we're outside one.
fn block_on(
    fut: impl std::future::Future<Output = anyhow::Result<String>>,
) -> anyhow::Result<String> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.block_on(fut)
    } else {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow::anyhow!("failed to create tokio runtime: {e}"))?;
        rt.block_on(fut)
    }
}

/// Tools that require user confirmation before execution.
pub const CONFIRM_REQUIRED: &[&str] = &[
    "delete_file", "move_file", "copy_file", "write_file",
    "run_shell", "run_test", "run_diagnostics", "open_preview",
    "git_commit", "git_branch", "apply_edits", "forget",
    "template", "package_install", "format", "build_project",
    "process_manager", "template_fill", "git_diff_apply", "scaffold_test",
    "bulk_crud", "format_setter", "git_tools",
];

/// Names of all toolbox tools (for BUILTIN_TOOL_NAMES list).
pub const TOOLBOX_NAMES: &[&str] = &[
    "clipboard", "template", "package_install", "lint", "format",
    "build_project", "http_request", "process_manager", "env",
    "json_query", "regex_search", "diff_apply", "template_fill",
    "git_diff_apply", "scaffold_test", "memory_search", "screenshot",
    "image_view",
    "bulk_read", "bulk_shell", "bulk_crud", "docs", "html",
    "screenrec", "format_setter", "code_format", "git_tools",
    "find_issues", "parallel",
];
