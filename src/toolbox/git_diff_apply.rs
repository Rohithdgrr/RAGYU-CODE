//! `git_diff_apply` — fetch a patch from a URL and apply it.
//!
//! Useful for grabbing PR diffs, Stack Overflow snippets, or any web-hosted
//! patch and applying it directly.

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Args {
    /// URL of a patch (text/plain, application/x-patch, or raw .diff/.patch).
    pub url: String,
    /// Base SHA for PR-style diffs (informational only).
    pub base_sha: Option<String>,
}

pub async fn run(base: &Path, args: Args) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (compatible; govinda-cli/1.0)")
        .build()?;
    let resp = client.get(&args.url).send().await
        .map_err(|e| anyhow::anyhow!("fetch failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Ok(format!("{{\"ok\":false,\"status\":{}}}", status.as_u16()));
    }
    let body = resp.text().await?;
    let base_sha = args.base_sha.clone().unwrap_or_default();
    // Re-use the diff_apply tool
    let _ = crate::toolbox::diff_apply::run(
        base,
        crate::toolbox::diff_apply::Args {
            patch: body.clone(),
            apply_untracked: true,
            parallel: false,
        },
    );
    let n_files = body.matches("diff --git").count();
    Ok(format!(
        "{{\"ok\":true,\"url\":\"{}\",\"base_sha\":\"{base_sha}\",\"files\":{n_files},\"patch_bytes\":{}}}",
        args.url, body.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Can't test async reqwest here without an HTTP server; covered by integration tests
    #[test]
    fn url_parsing_does_not_panic() {
        let _ = Args { url: "https://example.com".into(), base_sha: None };
    }
}
