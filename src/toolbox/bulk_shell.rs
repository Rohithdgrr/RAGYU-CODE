//! `bulk_shell` — run multiple shell commands in one call.
//!
//! Set `parallel: true` to run them concurrently (e.g. for a build+test+lint
//! pipeline). Returns per-command status, exit code, elapsed_ms, stdout, stderr.

use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Command {
    pub name: Option<String>,
    pub command: String,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Args {
    pub commands: Vec<Command>,
    /// Run commands in parallel (default false = sequential).
    #[serde(default)]
    pub parallel: bool,
    /// Stop on first failure (default true).
    #[serde(default = "default_true")]
    pub stop_on_error: bool,
}

fn default_true() -> bool { true }

pub fn run(base: &Path, args: Args) -> anyhow::Result<String> {
    let results: Vec<serde_json::Value> = if args.parallel {
        run_parallel(base, &args.commands)
    } else {
        run_sequential(base, &args.commands, args.stop_on_error)
    };
    let n = results.len();
    let n_ok = results.iter().filter(|r| r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false)).count();
    Ok(format!(
        "{{\"total\":{n},\"ok\":{n_ok},\"err\":{},\"parallel\":{},\"results\":{}}}",
        n - n_ok,
        args.parallel,
        serde_json::to_string(&results).unwrap_or_default()
    ))
}

fn run_sequential(base: &Path, commands: &[Command], stop_on_error: bool) -> Vec<serde_json::Value> {
    let mut results = Vec::new();
    for cmd in commands {
        let r = run_one(base, cmd);
        let ok = r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        results.push(r);
        if !ok && stop_on_error { break; }
    }
    results
}

fn run_parallel(base: &Path, commands: &[Command]) -> Vec<serde_json::Value> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    let base = base.to_path_buf();
    // Clone the commands so we can move them into spawned threads.
    let owned: Vec<Command> = commands.to_vec();
    let handles: Vec<_> = owned.into_iter().map(|cmd| {
        let tx = tx.clone();
        let base = base.clone();
        std::thread::spawn(move || {
            let r = run_one(&base, &cmd);
            let _ = tx.send(r);
        })
    }).collect();
    drop(tx);
    let mut results: Vec<serde_json::Value> = Vec::with_capacity(handles.len());
    for _ in 0..handles.len() {
        if let Ok(r) = rx.recv() { results.push(r); }
    }
    results
}

fn run_one(base: &Path, cmd: &Command) -> serde_json::Value {
    let name = cmd.name.clone().unwrap_or_else(|| cmd.command.clone());
    let timeout = Duration::from_secs(cmd.timeout_secs.unwrap_or(60));
    let started = std::time::Instant::now();
    let output = std::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" })
        .args(if cfg!(windows) { vec!["/C", &cmd.command] } else { vec!["-c", &cmd.command] })
        .current_dir(base)
        .output();
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let timeout_reached = started.elapsed() > timeout;
    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let ok = out.status.success() && !timeout_reached;
            serde_json::json!({
                "name": name,
                "ok": ok,
                "exit_code": out.status.code(),
                "elapsed_ms": elapsed_ms,
                "stdout": stdout,
                "stderr": stderr,
            })
        }
        Err(e) => serde_json::json!({
            "name": name,
            "ok": false,
            "error": e.to_string(),
            "elapsed_ms": elapsed_ms,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_sequential() {
        let dir = tempfile::tempdir().unwrap();
        let args = Args {
            commands: vec![
                Command { name: Some("a".into()), command: "echo a".into(), timeout_secs: Some(5) },
                Command { name: Some("b".into()), command: "echo b".into(), timeout_secs: Some(5) },
            ],
            parallel: false,
            stop_on_error: true,
        };
        let result = run(dir.path(), args).unwrap();
        assert!(result.contains("\"a\""));
        assert!(result.contains("\"b\""));
    }

    #[test]
    fn runs_parallel() {
        let dir = tempfile::tempdir().unwrap();
        let args = Args {
            commands: vec![
                Command { name: Some("a".into()), command: "echo a".into(), timeout_secs: Some(5) },
                Command { name: Some("b".into()), command: "echo b".into(), timeout_secs: Some(5) },
            ],
            parallel: true,
            stop_on_error: false,
        };
        let result = run(dir.path(), args).unwrap();
        assert!(result.contains("\"parallel\":true"));
    }
}
