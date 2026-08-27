//! `parallel` — run multiple tool invocations concurrently.
//!
//! With `wait: true` (default) returns when all calls complete. With
//! `wait: false` the calls fire-and-forget and return immediately.
//! `max_concurrency` caps parallelism (default 4, max 16).

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SubCall {
    /// Name of the tool to invoke.
    pub tool: String,
    /// JSON-encoded arguments string for the tool.
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    pub calls: Vec<SubCall>,
    /// Wait for all to complete (default true). If false, return early.
    pub wait: Option<bool>,
    /// Max concurrency (default 4, max 16).
    pub max_concurrency: Option<usize>,
}

pub fn run(base: &Path, args: Args) -> anyhow::Result<String> {
    let max = args.max_concurrency.unwrap_or(4).clamp(1, 16);
    let wait = args.wait.unwrap_or(true);
    if !wait {
        for c in &args.calls {
            let tool = c.tool.clone();
            let value = c.args.clone();
            let base = base.to_path_buf();
            std::thread::spawn(move || {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if let Some(v) = value.as_str() {
                        let _ = crate::toolbox::registry::dispatch(&tool, v, &base);
                    }
                }));
            });
        }
        return Ok(format!(
            "{{\"ok\":true,\"fired\":{},\"wait\":false}}",
            args.calls.len()
        ));
    }
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    let results: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    // Simple counting semaphore implemented with a Mutex<u8> (max 256).
    let sem = std::sync::Arc::new(std::sync::Mutex::new(max as u8));
    let handles: Vec<_> = args
        .calls
        .iter()
        .map(|c| {
            let tx = tx.clone();
            let results = std::sync::Arc::clone(&results);
            let sem = std::sync::Arc::clone(&sem);
            let tool = c.tool.clone();
            let value = c.args.clone();
            let base = base.to_path_buf();
            std::thread::spawn(move || {
                // Acquire permit (block until one is available).
                {
                    loop {
                        let mut guard = sem.lock().unwrap();
                        if *guard > 0 {
                            *guard -= 1;
                            break;
                        }
                        drop(guard);
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
                let res = if let Some(v) = value.as_str() {
                    crate::toolbox::registry::dispatch(&tool, v, &base)
                } else {
                    Some(Err(anyhow::anyhow!("args must be a JSON string")))
                };
                let payload = match res {
                    Some(Ok(s)) => serde_json::json!({"tool":tool, "ok":true, "result":s}),
                    Some(Err(e)) => {
                        serde_json::json!({"tool":tool, "ok":false, "error":e.to_string()})
                    }
                    None => serde_json::json!({"tool":tool, "ok":false, "error":"unknown tool"}),
                };
                results.lock().unwrap().push(payload.clone());
                // Release permit.
                *sem.lock().unwrap() += 1;
                let _ = tx.send(());
            })
        })
        .collect();
    let _ = rx.recv_timeout(std::time::Duration::from_secs(120));
    drop(tx);
    for h in handles {
        let _ = h.join();
    }
    let results = std::sync::Arc::try_unwrap(results)
        .ok()
        .and_then(|m| Some(m.into_inner().unwrap_or_default()))
        .unwrap_or_default();
    Ok(format!(
        "{{\"ok\":true,\"concurrent\":{max},\"count\":{},\"results\":{}}}",
        results.len(),
        serde_json::to_string(&results).unwrap_or_default()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatches_to_registry() {
        let dir = tempfile::tempdir().unwrap();
        let calls = vec![SubCall {
            tool: "json_query".into(),
            args: serde_json::Value::String(r#"{"source":"raw","query":"42"}"#.into()),
        }];
        let args = Args {
            calls,
            wait: Some(true),
            max_concurrency: Some(2),
        };
        let result = run(dir.path(), args).unwrap();
        assert!(result.contains("\"json_query\""));
    }
}
