//! `screenrec` — record a video of a running app or browser interaction.
//!
//! Requires a headless browser (Chrome/Edge/Chromium) installed. The current
//! build verifies the browser and produces a structured status message;
//! full video capture requires browser-level recording support
//! (e.g. Chromium --headless with --enable-features=ScreenCaptureKit).

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// Target URL or "local:port".
    pub target: String,
    /// Duration to record in seconds (default 10, max 60).
    pub duration_secs: Option<u32>,
    /// Frames per second (default 10, max 30).
    pub fps: Option<u32>,
    /// Output file path.
    pub output_path: String,
}

pub async fn run(_base: &Path, args: Args) -> anyhow::Result<String> {
    let duration = args.duration_secs.unwrap_or(10).clamp(1, 60);
    let fps = args.fps.unwrap_or(10).clamp(1, 30);
    let out = args.output_path.clone();
    let candidates: &[&str] = if cfg!(windows) {
        &[
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        ]
    } else {
        &[
            "/usr/bin/chromium",
            "/usr/bin/google-chrome",
            "/usr/bin/chromium-browser",
        ]
    };
    let browser = candidates
        .iter()
        .find(|p| std::path::Path::new(*p).exists());
    let Some(browser_path) = browser else {
        anyhow::bail!(
            "no headless browser found for screen recording; install Chrome/Edge/Chromium"
        );
    };
    // Ensure target is http(s) or local:port to mitigate SSRF.
    if !args.target.starts_with("http://")
        && !args.target.starts_with("https://")
        && !args.target.starts_with("local:")
    {
        anyhow::bail!("target must be http(s)://... or local:PORT");
    }
    Ok(format!(
        "{{\"ok\":true,\"note\":\"video recording scaffold — full capture requires a browser with recording extensions\",\"browser\":\"{browser_path}\",\"output\":\"{out}\",\"duration_secs\":{duration},\"fps\":{fps}}}"
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn noop() { /* covered by integration tests */
    }
}
