//! `bulk_crud` — read/write/delete/move/copy many files in one call.
//!
//! Each operation is a discriminated union item with `action: read|write|delete|move|copy`.
//! Parallel mode is safe for reads, sequential for writes (default).

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Op {
    /// Read a file.
    Read { path: String },
    /// Write content to a file.
    Write { path: String, content: String },
    /// Delete a file.
    Delete { path: String },
    /// Move a file.
    Move { from: String, to: String },
    /// Copy a file.
    Copy { from: String, to: String },
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    pub operations: Vec<Op>,
    /// Run operations in parallel (default false = sequential).
    #[serde(default)]
    pub parallel: bool,
    /// Stop on first failure (default true).
    #[serde(default = "default_true")]
    pub stop_on_error: bool,
    /// Max bytes per file when reading (default 50_000, 0 = unlimited).
    pub max_bytes: Option<usize>,
}

fn default_true() -> bool {
    true
}

pub fn run(base: &Path, args: Args) -> anyhow::Result<String> {
    let max_bytes = args.max_bytes.unwrap_or(50_000);
    let results: Vec<serde_json::Value> = if args.parallel {
        run_parallel(base, &args.operations, max_bytes)
    } else {
        run_sequential(base, &args.operations, max_bytes, args.stop_on_error)
    };
    let n = results.len();
    let n_ok = results
        .iter()
        .filter(|r| r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false))
        .count();
    Ok(format!(
        "{{\"total\":{n},\"ok\":{n_ok},\"err\":{},\"parallel\":{},\"results\":{}}}",
        n - n_ok,
        args.parallel,
        serde_json::to_string(&results).unwrap_or_default()
    ))
}

fn run_sequential(base: &Path, ops: &[Op], max_bytes: usize, stop: bool) -> Vec<serde_json::Value> {
    let mut results = Vec::new();
    for op in ops {
        let r = run_one(base, op, max_bytes);
        let ok = r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        results.push(r);
        if !ok && stop {
            break;
        }
    }
    results
}

fn run_parallel(base: &Path, ops: &[Op], max_bytes: usize) -> Vec<serde_json::Value> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    let base = base.to_path_buf();
    // Clone the operations so we can move them into spawned threads.
    let owned: Vec<Op> = ops.to_vec();
    let handles: Vec<_> = owned
        .into_iter()
        .map(|op| {
            let tx = tx.clone();
            let base = base.clone();
            std::thread::spawn(move || {
                let r = run_one(&base, &op, max_bytes);
                let _ = tx.send(r);
            })
        })
        .collect();
    drop(tx);
    let mut results: Vec<serde_json::Value> = Vec::with_capacity(handles.len());
    for _ in 0..handles.len() {
        if let Ok(r) = rx.recv() {
            results.push(r);
        }
    }
    results
}

fn run_one(base: &Path, op: &Op, max_bytes: usize) -> serde_json::Value {
    match op {
        Op::Read { path } => {
            let full = base.join(path);
            match std::fs::read_to_string(&full) {
                Ok(content) => {
                    let truncated = if max_bytes > 0 && content.len() > max_bytes {
                        let cut = &content[..max_bytes];
                        format!("{cut}\n…(truncated)")
                    } else {
                        content
                    };
                    serde_json::json!({"op":"read","path":path,"ok":true,"bytes":truncated.len(),"content":truncated})
                }
                Err(e) => {
                    serde_json::json!({"op":"read","path":path,"ok":false,"error":e.to_string()})
                }
            }
        }
        Op::Write { path, content } => {
            let full = base.join(path);
            if let Some(parent) = full.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&full, content) {
                Ok(()) => {
                    serde_json::json!({"op":"write","path":path,"ok":true,"bytes":content.len()})
                }
                Err(e) => {
                    serde_json::json!({"op":"write","path":path,"ok":false,"error":e.to_string()})
                }
            }
        }
        Op::Delete { path } => {
            let full = base.join(path);
            match std::fs::remove_file(&full) {
                Ok(()) => serde_json::json!({"op":"delete","path":path,"ok":true}),
                Err(e) => {
                    serde_json::json!({"op":"delete","path":path,"ok":false,"error":e.to_string()})
                }
            }
        }
        Op::Move { from, to } => {
            let f = base.join(from);
            let t = base.join(to);
            if let Some(parent) = t.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::rename(&f, &t) {
                Ok(()) => serde_json::json!({"op":"move","from":from,"to":to,"ok":true}),
                Err(e) => {
                    serde_json::json!({"op":"move","from":from,"to":to,"ok":false,"error":e.to_string()})
                }
            }
        }
        Op::Copy { from, to } => {
            let f = base.join(from);
            let t = base.join(to);
            if let Some(parent) = t.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::copy(&f, &t) {
                Ok(bytes) => {
                    serde_json::json!({"op":"copy","from":from,"to":to,"ok":true,"bytes":bytes})
                }
                Err(e) => {
                    serde_json::json!({"op":"copy","from":from,"to":to,"ok":false,"error":e.to_string()})
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulk_write_read() {
        let dir = tempfile::tempdir().unwrap();
        let args = Args {
            operations: vec![
                Op::Write {
                    path: "a.txt".into(),
                    content: "alpha".into(),
                },
                Op::Write {
                    path: "b.txt".into(),
                    content: "beta".into(),
                },
                Op::Read {
                    path: "a.txt".into(),
                },
                Op::Read {
                    path: "b.txt".into(),
                },
            ],
            parallel: false,
            stop_on_error: true,
            max_bytes: Some(1000),
        };
        let r = run(dir.path(), args).unwrap();
        assert!(r.contains("\"alpha\""));
        assert!(r.contains("\"beta\""));
    }
}
