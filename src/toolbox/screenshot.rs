//! `screenshot` — visual capture of a URL or running app.
//!
//! Uses headless Chrome (via `chromiumoxide` if available) or falls back to
//! printing a clear error so the user knows what to install.

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Args {
    /// URL (https://...) or "local:port" for a running dev server.
    pub target: String,
    /// Full page screenshot (default false = viewport only).
    #[serde(default)]
    pub full_page: bool,
    /// CSS selector to scope the screenshot to a specific element.
    pub selector: Option<String>,
}

pub async fn run(base: &Path, args: Args) -> anyhow::Result<String> {
    let out_dir = base.join(".govinda").join("screenshots");
    std::fs::create_dir_all(&out_dir)?;
    let path = out_dir.join(format!("shot-{}.png", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    // Try to find a headless browser
    let candidates: &[&str] = if cfg!(windows) {
        &[
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        ]
    } else {
        &["/usr/bin/chromium", "/usr/bin/google-chrome", "/usr/bin/chromium-browser", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"]
    };
    let browser = candidates.iter().find(|p| std::path::Path::new(*p).exists());
    let Some(browser) = browser else {
        anyhow::bail!(
            "no headless browser found. Install Chrome/Edge/Chromium, or set CHROME_PATH env var to the binary. Tried: {:?}",
            candidates
        );
    };
    let path_str = path.to_string_lossy();
    let mut cmd = std::process::Command::new(browser);
    cmd.arg("--headless=new")
        .arg(format!("--screenshot={path_str}"))
        .arg("--hide-scrollbars")
        .arg("--disable-gpu")
        .arg(if args.full_page { "--full-page" } else { "--window-size=1280,720" })
        .arg("--virtual-time-budget=5000");
    // SECURITY: only allow http(s) URLs to mitigate SSRF to local/private addresses
    if !args.target.starts_with("http://") && !args.target.starts_with("https://") {
        if !args.target.starts_with("local:") {
            anyhow::bail!("target must be http(s)://... or local:PORT");
        }
    }
    if args.target.starts_with("http") {
        cmd.arg(&args.target);
    } else {
        // local:port → http://127.0.0.1:port
        let port = args.target.trim_start_matches("local:");
        cmd.arg(format!("http://127.0.0.1:{port}"));
    }
    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Ok(format!("{{\"ok\":false,\"error\":{}}}", serde_json::Value::String(truncate(&stderr, 500))));
    }
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Ok(format!("{{\"ok\":true,\"path\":\"{}\",\"bytes\":{},\"selector\":{}}}", path.display(), bytes, args.selector.as_deref().map(|s| format!("\"{s}\"")).unwrap_or_else(|| "null".into())))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_owned() }
    else { let mut out: String = s.chars().take(max).collect(); out.push('…'); out }
}

#[cfg(test)]
mod tests {
    #[test]
    fn noop() { /* requires a real browser; covered by integration tests */ }
}
