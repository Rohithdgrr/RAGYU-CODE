//! `/diff`, `/apply`, `/reject` — review and commit the staged edits the
//! agent's surgical tools (`edit_file`, `insert_after`, `insert_before`)
//! have queued. Nothing touches the disk until `/apply`.

use super::{App, dim, err, info, ok};
use crate::tools::{EditOp, PendingEdits, apply_ops_to_content, resolve_in, staged_diff};
use anyhow::Context as _;

/// `/diff` — show the unified diff of everything staged.
#[allow(dead_code)]
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
        dim(format!("  {}. {}", i + 1, op.describe()));
    }
    match staged_diff(&cwd, &ops) {
        Ok(diff) if diff.trim().is_empty() => dim("(edits cancel out — empty diff)"),
        Ok(diff) => info(diff),
        Err(e) => err(format!("cannot build diff: {e:#}")),
    }
}

/// `/apply` — validate every staged edit against current file contents,
/// then write all files atomically: any validation failure aborts the whole
/// batch with the queue intact. Returns `true` when all edits were written.
#[allow(dead_code)]
pub(super) fn apply(app: &mut App) -> bool {
    let ops = snapshot(app);
    if ops.is_empty() {
        dim("no staged edits to apply.");
        return false;
    }

    ok(format!("applying {} staged edit(s):", ops.len()));
    for (i, op) in ops.iter().enumerate() {
        dim(format!("  {}. {}", i + 1, op.describe()));
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
        return false;
    };
    for (path, group) in &grouped {
        let outcome = transform_file(&cwd, path, group).map(|updated| {
            writes.push((resolve_in(&cwd, path).unwrap_or_default(), updated));
        });
        if let Err(e) = outcome {
            err(format!("apply aborted (nothing written): {e:#}"));
            return false;
        }
    }

    // All validated — commit.
    let mut failed = false;
    for (full, content) in &writes {
        if let Err(e) = std::fs::write(full, content) {
            failed = true;
            err(format!("write failed for {}: {e}", full.display()));
        }
    }
    if failed {
        err("some writes failed — inspect your files before retrying.");
        return false;
    }
    if let Some(mut q) = app.pending_edits.try_lock() {
        q.clear();
    }
    ok(format!(
        "applied {} edit(s) across {} file(s).",
        ops.len(),
        grouped.len()
    ));
    true
}

/// `/reject` — discard everything staged without touching any files.
#[allow(dead_code)]
pub(super) fn reject(app: &mut App) {
    let n = snapshot(app).len();
    match app.pending_edits.try_lock() {
        Some(mut q) => {
            q.clear();
            if n == 0 {
                dim("nothing staged to reject.");
            } else {
                ok(format!(
                    "discarded {n} staged edit(s); no files were changed."
                ));
            }
        }
        None => err("staged-edit queue is busy; try again."),
    }
}

/// `/review` — batch summary after a run of edits: per-file `+N/-M` counts
/// computed from each file's staged unified diff.
#[allow(dead_code)]
pub(super) fn review(app: &App) {
    let ops = snapshot(app);
    if ops.is_empty() {
        dim("no staged edits — nothing to review.");
        return;
    }
    // Group by target path, preserving first-seen order (same as /apply).
    let mut grouped: Vec<(String, Vec<&EditOp>)> = Vec::new();
    for op in &ops {
        match grouped.iter_mut().find(|(p, _)| *p == op.path()) {
            Some((_, group)) => group.push(op),
            None => grouped.push((op.path().to_owned(), vec![op])),
        }
    }
    let Ok(cwd) = std::env::current_dir() else {
        err("cannot resolve working directory.");
        return;
    };

    ok(format!("{} file(s) modified:", grouped.len()));
    let mut totals = (0usize, 0usize);
    for (path, group) in &grouped {
        let (added, removed) = {
            let owned: Vec<EditOp> = group.iter().map(|op| (*op).clone()).collect();
            match staged_diff(&cwd, &owned) {
                Ok(diff) => crate::diff::count_changes(&diff),
                Err(e) => {
                    err(format!("cannot diff '{path}': {e:#}"));
                    continue;
                }
            }
        };
        totals.0 += added;
        totals.1 += removed;
        ok(format!("  {path}: +{added}/-{removed}"));
    }
    dim(format!(
        "total: +{}/-{} across {} staged edit(s)",
        totals.0,
        totals.1,
        ops.len()
    ));
    dim("run /apply to confirm, /reject to discard, or /diff for full diffs.");
}

#[allow(dead_code)]
fn snapshot(app: &App) -> Vec<EditOp> {
    app.pending_edits.lock().ops().to_vec()
}

#[allow(dead_code)]
fn transform_file(cwd: &std::path::Path, path: &str, group: &[&EditOp]) -> anyhow::Result<String> {
    let full = resolve_in(cwd, path)?;
    let bytes = std::fs::read(&full).with_context(|| format!("cannot read '{path}'"))?;
    anyhow::ensure!(!bytes.contains(&0), "'{path}' looks binary");
    let original = String::from_utf8_lossy(&bytes).to_string();
    apply_ops_to_content(&original, path, group)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::PendingEdits;
    use parking_lot::Mutex as PlMutex;

    fn smoke_app() -> App {
        super::super::tests::smoke_app()
    }

    #[test]
    fn review_reports_per_file_change_counts() {
        let ws = TempDir::new("review");
        std::fs::write(ws.0.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(ws.0.join("b.txt"), "x\n").unwrap();
        let mut app = smoke_app();
        app.pending_edits = std::sync::Arc::new(PlMutex::new({
            let mut q = PendingEdits::default();
            q.push(EditOp::Replace {
                path: "a.txt".into(),
                old_string: "two".into(),
                new_string: "TWO\nextra".into(),
            });
            q.push(EditOp::Replace {
                path: "b.txt".into(),
                old_string: "x".into(),
                new_string: "".into(),
            });
            q
        }));
        // Must not panic and must not touch the disk.
        review(&app);
        assert_eq!(
            std::fs::read_to_string(ws.0.join("a.txt")).unwrap(),
            "one\ntwo\nthree\n"
        );
    }

    #[test]
    fn apply_writes_files_and_clears_queue() {
        let _guard = crate::TEST_CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ws = TempDir::new("apply");
        std::env::set_current_dir(&ws.0).unwrap();
        std::fs::write(ws.0.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let mut app = smoke_app();
        app.pending_edits = std::sync::Arc::new(PlMutex::new({
            let mut q = PendingEdits::default();
            q.push(EditOp::Replace {
                path: "a.txt".into(),
                old_string: "two".into(),
                new_string: "TWO".into(),
            });
            q
        }));
        assert!(apply(&mut app));
        assert_eq!(
            std::fs::read_to_string(ws.0.join("a.txt")).unwrap(),
            "one\nTWO\nthree\n"
        );
        assert!(snapshot(&app).is_empty());
    }

    #[test]
    fn apply_aborts_atomically_on_bad_op() {
        let _guard = crate::TEST_CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ws = TempDir::new("apply-abort");
        std::env::set_current_dir(&ws.0).unwrap();
        std::fs::write(ws.0.join("a.txt"), "content\n").unwrap();
        let mut app = smoke_app();
        app.pending_edits = std::sync::Arc::new(PlMutex::new({
            let mut q = PendingEdits::default();
            q.push(EditOp::Replace {
                path: "a.txt".into(),
                old_string: "missing text".into(),
                new_string: "nope".into(),
            });
            q
        }));
        assert!(!apply(&mut app));
        assert_eq!(
            std::fs::read_to_string(ws.0.join("a.txt")).unwrap(),
            "content\n",
            "file must be untouched when an op fails validation"
        );
    }

    // Local temp-workspace helper (mirrors tools::tests::TempWs).
    pub struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "govinda-edits-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
}
