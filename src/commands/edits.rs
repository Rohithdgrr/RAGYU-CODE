//! `/diff`, `/apply`, `/reject` — review and commit the staged edits the
//! agent's surgical tools (`edit_file`, `insert_after`, `insert_before`)
//! have queued. Nothing touches the disk until `/apply`.

use super::{App, dim, err, ok, paint};
use crate::render::{dim_color, err_color};
use crate::tools::{EditOp, PendingEdits, apply_ops_to_content, resolve_in, staged_diff};
use anyhow::Context as _;
use crossterm::style::Color;

/// `/diff` — show the unified diff of everything staged.
pub(super) fn view(app: &App) {
    let ops = snapshot(app);
    if ops.is_empty() {
        dim("no staged edits — nothing to diff.");
        return;
    }
    let Ok(cwd) = std::env::current_dir() else {
        err("cannot resolve working directory.");
        return;
    };
    for (i, op) in ops.iter().enumerate() {
        println!(
            "{}",
            paint(format!("  {}. {}", i + 1, op.describe()), dim_color())
        );
    }
    match staged_diff(&cwd, &ops) {
        Ok(diff) if diff.trim().is_empty() => dim("(edits cancel out — empty diff)"),
        Ok(diff) => println!("{diff}"),
        Err(e) => err(&format!("cannot build diff: {e:#}")),
    }
}

/// `/apply` — validate every staged edit against current file contents,
/// then write all files atomically: any validation failure aborts the whole
/// batch with the queue intact.
pub(super) fn apply(app: &mut App) {
    let ops = snapshot(app);
    if ops.is_empty() {
        dim("no staged edits to apply.");
        return;
    }

    println!(
        "{}",
        paint(
            format!("applying {} staged edit(s):", ops.len()),
            Color::Yellow
        )
    );
    for (i, op) in ops.iter().enumerate() {
        println!(
            "{}",
            paint(format!("  {}. {}", i + 1, op.describe()), dim_color())
        );
    }

    // Group by target path, preserving first-seen order.
    let mut grouped: Vec<(String, Vec<&EditOp>)> = Vec::new();
    for op in &ops {
        match grouped.iter_mut().find(|(p, _)| *p == op.path()) {
            Some((_, group)) => group.push(op),
            None => grouped.push((op.path().to_owned(), vec![op])),
        }
    }

    // Validate and transform in memory first; nothing is written until every
    // file has been processed successfully.
    let mut writes: Vec<(std::path::PathBuf, String)> = Vec::new();
    let Ok(cwd) = std::env::current_dir() else {
        err("cannot resolve working directory.");
        return;
    };
    for (path, group) in &grouped {
        let outcome = transform_file(&cwd, path, group).map(|updated| {
            writes.push((resolve_in(&cwd, path).unwrap_or_default(), updated));
        });
        if let Err(e) = outcome {
            err(&format!("apply aborted (nothing written): {e:#}"));
            return;
        }
    }

    // All validated — commit.
    let mut failed = false;
    for (full, content) in &writes {
        if let Err(e) = std::fs::write(full, content) {
            failed = true;
            eprintln!(
                "{}",
                paint(
                    format!("write failed for {}: {e}", full.display()),
                    err_color()
                )
            );
        }
    }
    if failed {
        err("some writes failed — inspect your files before retrying.");
        return;
    }
    if let Ok(mut q) = app.pending_edits.lock() {
        q.clear();
    }
    ok(&format!(
        "applied {} edit(s) across {} file(s).",
        ops.len(),
        grouped.len()
    ));
}

/// `/reject` — discard everything staged without touching any files.
pub(super) fn reject(app: &mut App) {
    let n = snapshot(app).len();
    match app.pending_edits.lock() {
        Ok(mut q) => {
            q.clear();
            if n == 0 {
                dim("nothing staged to reject.");
            } else {
                ok(&format!(
                    "discarded {n} staged edit(s); no files were changed."
                ));
            }
        }
        Err(_) => err("staged-edit queue poisoned."),
    }
}

fn snapshot(app: &App) -> Vec<EditOp> {
    app.pending_edits
        .lock()
        .map(|q: std::sync::MutexGuard<'_, PendingEdits>| q.ops().to_vec())
        .unwrap_or_default()
}

fn transform_file(cwd: &std::path::Path, path: &str, group: &[&EditOp]) -> anyhow::Result<String> {
    let full = resolve_in(cwd, path)?;
    let bytes = std::fs::read(&full).with_context(|| format!("cannot read '{path}'"))?;
    anyhow::ensure!(!bytes.contains(&0), "'{path}' looks binary");
    let original = String::from_utf8_lossy(&bytes).to_string();
    apply_ops_to_content(&original, path, group)
}
