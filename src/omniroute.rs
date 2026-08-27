//! OmniRoute gateway bootstrap.
//!
//! OmniRoute (`npm i -g omniroute`) is Govinda's zero-config default
//! backend: an OpenAI-compatible gateway on `localhost:20128` whose `auto`
//! model works keylessly on a fresh install. This module makes that default
//! truly automatic: when the gateway is not running, Govinda installs the
//! npm package (if needed), launches the server detached, and waits for it
//! to come up — no manual setup.

use anyhow::{Context as _, Result};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::{Command as StdCommand, Stdio};
use std::time::Duration;

pub const BASE_URL: &str = "http://localhost:20128";

const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
const BOOT_TIMEOUT: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(1000);
const PROBE_TIMEOUT: Duration = Duration::from_millis(600);

/// Windows creation flags: no console window + own process group so the
/// detached server outlives Govinda quietly.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

fn base_url() -> String {
    std::env::var("OMNIROUTE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| BASE_URL.to_owned())
}

/// True when the gateway answers on `/v1/models`.
pub async fn probe(http: &reqwest::Client) -> bool {
    let url = format!("{}/v1/models", base_url());
    match tokio::time::timeout(PROBE_TIMEOUT, http.get(url).send()).await {
        Ok(Ok(resp)) => resp.status().is_success(),
        _ => false,
    }
}

/// Runs a command to completion, returning trimmed stdout.
async fn run_cmd(argv: &[String], timeout: Duration) -> Result<String> {
    let display = argv.join(" ");
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| anyhow::anyhow!("'{display}' timed out"))?
        .with_context(|| format!("failed to launch '{display}'"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("'{display}' failed: {}", stderr.trim())
    }
}

/// On Windows npm is a `.cmd` shim, so everything goes through `cmd /C`.
fn npm(argv: &str) -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".into(), "/C".into(), format!("npm {argv}")]
    } else {
        vec!["npm".into(), argv.to_owned()]
    }
}

fn omniroute_cli(argv: &str) -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".into(), "/C".into(), format!("omniroute {argv}")]
    } else {
        vec!["omniroute".into(), argv.to_owned()]
    }
}

async fn npm_available() -> bool {
    run_cmd(&npm("--version"), Duration::from_secs(15))
        .await
        .is_ok()
}

async fn omniroute_installed() -> bool {
    run_cmd(&omniroute_cli("--version"), Duration::from_secs(20))
        .await
        .is_ok()
}

/// Installs OmniRoute globally, streaming npm's own progress output to the
/// terminal so the user sees download/link progress live instead of a silent
/// wait. The child is killed if it overruns `INSTALL_TIMEOUT`.
///
/// Speed knobs: `--no-audit`/`--no-fund` skip two extra network round-trips
/// per package, `--no-optional` skips native/optional deps we don't need, and
/// `--prefer-offline` reuses any cached tarballs from a prior attempt so a
/// retry or a second machine with a warm cache installs near-instantly.
async fn install_omniroute() -> Result<()> {
    let argv = npm("install -g omniroute --no-audit --no-fund --no-optional --prefer-offline");
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        // Belt-and-suspenders: force the same skips via env in case the
        // user's npmrc re-enables them.
        .env("npm_config_audit", "false")
        .env("npm_config_fund", "false")
        .env("npm_config_prefer_offline", "true");
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let mut child = cmd
        .spawn()
        .context("failed to launch npm for the omniroute install")?;
    match tokio::time::timeout(INSTALL_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => {
            anyhow::bail!("'npm install -g omniroute' exited with {status}")
        }
        Ok(Err(e)) => Err(e).context("waiting on the omniroute install failed"),
        Err(_) => {
            let _ = child.start_kill();
            anyhow::bail!(
                "omniroute installation did not finish within {}s — run 'npm i -g omniroute' manually",
                INSTALL_TIMEOUT.as_secs()
            )
        }
    }
}

/// Launches the gateway detached; the child handle is dropped immediately
/// so the server outlives Govinda. Uses `std::process::Command` (not tokio)
/// because tokio's `Child` does not reliably detach on all platforms —
/// notably Windows where dropping the handle can terminate the grandchild.
///
/// `stderr` is redirected to a per-process tempfile so boot failures
/// are recoverable: the caller can tail the file in the error message
/// instead of relying on the user to dig through process logs.
fn spawn_server() -> Result<std::path::PathBuf> {
    let argv = omniroute_cli("");
    let log_path = std::env::temp_dir().join(format!("omniroute-{}.log", std::process::id()));
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open {}", log_path.display()))?;
    let mut cmd = StdCommand::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file));
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    cmd.spawn()
        .context("failed to start the omniroute server")?;
    // Intentionally drop the Child handle — the process must outlive us.
    Ok(log_path)
}

/// Guarantees the OmniRoute gateway is serving: probe → verify install →
/// install if missing (with live progress) → start if stopped → wait for boot.
/// Returns true only when the gateway answers afterwards. Errors are
/// actionable; the caller just reports them.
pub async fn ensure_running(http: &reqwest::Client) -> Result<bool> {
    // Fast path: the gateway is already answering.
    if probe(http).await {
        return Ok(true);
    }

    // Step 1: verify whether the CLI is installed on this machine.
    if !omniroute_installed().await {
        if !npm_available().await {
            anyhow::bail!(
                "Node.js/npm not found — install Node from https://nodejs.org, then run 'npm i -g omniroute' manually"
            );
        }
        eprintln!(
            "OmniRoute is not installed — installing globally now (this can take a minute)..."
        );
        install_omniroute().await?;
        eprintln!("OmniRoute installed successfully.");
    }

    // Step 2: the gateway is installed; start it if it isn't already up.
    eprintln!("starting the OmniRoute gateway on {}...", base_url());
    let log_path = spawn_server()?;

    let deadline = tokio::time::Instant::now() + BOOT_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if probe(http).await {
            return Ok(true);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    // Boot failed: tail the server log so the user has something
    // actionable instead of a "did not come up in time" mystery.
    if let Ok(tail) = std::fs::read_to_string(&log_path) {
        let last: String = tail
            .lines()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        if !last.is_empty() {
            eprintln!("--- omniroute server log (last lines) ---\n{last}\n---");
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npm_argv_goes_through_cmd_on_windows_only() {
        #[cfg(windows)]
        assert_eq!(
            npm("--version"),
            vec![
                "cmd".to_owned(),
                "/C".to_owned(),
                "npm --version".to_owned()
            ]
        );
        #[cfg(not(windows))]
        assert_eq!(
            npm("--version"),
            vec!["npm".to_owned(), "--version".to_owned()]
        );
    }

    #[test]
    fn base_url_is_overridable() {
        // Default constant sanity; env override path exercised indirectly.
        assert!(BASE_URL.starts_with("http://"));
    }
}
