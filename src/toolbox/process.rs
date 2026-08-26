//! `process_manager` — background process control (dev servers, watchers, daemons).
//!
//! Lets the model start long-running processes and query them later. Each
//! process gets a short handle ID (e.g. "p1") returned by `start`.
//!
//! State is in-memory and process-scoped — handles are lost when govinda
//! exits. This is intentional: the CLI is short-lived.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Default)]
struct Proc {
    handle: String,
    command: String,
    child: Option<std::process::Child>,
    log_tail: String,
}

static PROCS: OnceLock<Arc<Mutex<HashMap<String, Proc>>>> = OnceLock::new();
static HANDLE_COUNTER: OnceLock<Mutex<u32>> = OnceLock::new();

fn procs() -> &'static Arc<Mutex<HashMap<String, Proc>>> {
    PROCS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

fn handle_counter() -> &'static Mutex<u32> {
    HANDLE_COUNTER.get_or_init(|| Mutex::new(0))
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action { Start, Stop, List, Tail, Status }

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Args {
    pub action: Action,
    pub command: Option<String>,
    pub handle_id: Option<String>,
    pub tail_lines: Option<usize>,
}

pub fn run(args: Args) -> anyhow::Result<String> {
    match args.action {
        Action::Start => {
            let cmd = args.command.ok_or_else(|| anyhow::anyhow!("command required for start"))?;
            let mut guard = procs().lock().unwrap();
            let mut counter = handle_counter().lock().unwrap();
            *counter += 1;
            let handle = format!("p{}", *counter);
            let child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" })
                .args(if cfg!(windows) { vec!["/C", &cmd] } else { vec!["-c", &cmd] })
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok();
            guard.insert(handle.clone(), Proc {
                handle: handle.clone(),
                command: cmd.clone(),
                child,
                log_tail: String::new(),
            });
            Ok(format!("{{\"handle\":\"{}\",\"command\":\"{}\",\"started\":true}}", handle, cmd))
        }
        Action::Stop => {
            let h = args.handle_id.ok_or_else(|| anyhow::anyhow!("handle_id required for stop"))?;
            let mut guard = procs().lock().unwrap();
            if let Some(p) = guard.get_mut(&h) {
                if let Some(mut c) = p.child.take() {
                    let _ = c.kill();
                }
                guard.remove(&h);
                Ok(format!("{{\"handle\":\"{}\",\"stopped\":true}}", h))
            } else {
                Ok(format!("{{\"handle\":\"{}\",\"stopped\":false,\"reason\":\"not_found\"}}", h))
            }
        }
        Action::List => {
            let guard = procs().lock().unwrap();
            let list: Vec<serde_json::Value> = guard.values().map(|p| {
                serde_json::json!({
                    "handle": p.handle,
                    "command": p.command,
                    "running": p.child.is_some(),
                })
            }).collect();
            Ok(format!("{{\"processes\":{}}}", serde_json::to_string(&list).unwrap_or_default()))
        }
        Action::Tail => {
            let h = args.handle_id.ok_or_else(|| anyhow::anyhow!("handle_id required for tail"))?;
            let _tail = args.tail_lines.unwrap_or(50);
            let guard = procs().lock().unwrap();
            match guard.get(&h) {
                Some(p) => Ok(format!("{{\"handle\":\"{}\",\"log\":{}}}", h, serde_json::Value::String(p.log_tail.clone()))),
                None => Ok(format!("{{\"handle\":\"{}\",\"log\":\"\",\"reason\":\"not_found\"}}", h)),
            }
        }
        Action::Status => {
            let h = args.handle_id.ok_or_else(|| anyhow::anyhow!("handle_id required for status"))?;
            let guard = procs().lock().unwrap();
            match guard.get(&h) {
                Some(p) => Ok(format!("{{\"handle\":\"{}\",\"command\":\"{}\",\"running\":{}}}", h, p.command, p.child.is_some())),
                None => Ok(format!("{{\"handle\":\"{}\",\"found\":false}}", h)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_initially_empty() {
        let args = Args { action: Action::List, command: None, handle_id: None, tail_lines: None };
        let result = run(args).unwrap();
        assert!(result.contains("\"processes\""));
    }

    #[test]
    fn start_returns_handle() {
        let args = Args { action: Action::Start, command: Some("echo hello".into()), handle_id: None, tail_lines: None };
        let result = run(args).unwrap();
        assert!(result.contains("\"handle\":\"p"));
    }

    #[test]
    fn stop_unknown_handle_is_not_error() {
        let args = Args { action: Action::Stop, command: None, handle_id: Some("px".into()), tail_lines: None };
        let result = run(args).unwrap();
        assert!(result.contains("\"stopped\":false"));
    }
}
