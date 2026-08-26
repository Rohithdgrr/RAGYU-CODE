//! `html` — render HTML: Markdown → HTML, wrap Markdown in a styled HTML
//! document, or open an existing HTML file in the system browser.

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Convert Markdown text to HTML.
    MdToHtml,
    /// Wrap Markdown body in a standalone styled HTML document.
    WrapHtml,
    /// Open a saved HTML file in the system browser.
    OpenHtml,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Args {
    pub action: Action,
    /// Source Markdown text (for md_to_html / wrap_html).
    pub text: Option<String>,
    /// Source file path (for open_html).
    pub source: Option<String>,
    /// Output file path (for md_to_html / wrap_html).
    pub output_path: Option<String>,
    /// Page title (for wrap_html; default "Document").
    pub title: Option<String>,
}

pub fn run(_base: &Path, args: Args) -> anyhow::Result<String> {
    match args.action {
        Action::MdToHtml => {
            let text = args.text.ok_or_else(|| anyhow::anyhow!("text required for md_to_html"))?;
            let html = markdown_to_html(&text);
            let out = args.output_path.ok_or_else(|| anyhow::anyhow!("output_path required"))?;
            std::fs::write(&out, &html)?;
            Ok(format!("{{\"ok\":true,\"output\":\"{out}\",\"bytes\":{}}}", html.len()))
        }
        Action::WrapHtml => {
            let text = args.text.ok_or_else(|| anyhow::anyhow!("text required for wrap_html"))?;
            let title = args.title.unwrap_or_else(|| "Document".to_string());
            let body = markdown_to_html(&text);
            let html = format!(
                "<!doctype html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\"><title>{}</title><style>body{{font-family:system-ui;max-width:760px;margin:2rem auto;padding:0 1rem;line-height:1.6}}pre{{background:#f5f5f5;padding:0.75rem;border-radius:4px;overflow:auto}}code{{background:#f5f5f5;padding:0.1rem 0.3rem;border-radius:3px}}</style></head>\n<body>\n{}\n</body>\n</html>\n",
                html_escape(&title), body
            );
            let out = args.output_path.ok_or_else(|| anyhow::anyhow!("output_path required"))?;
            std::fs::write(&out, &html)?;
            Ok(format!("{{\"ok\":true,\"output\":\"{out}\",\"bytes\":{}}}", html.len()))
        }
        Action::OpenHtml => {
            let source = args.source.ok_or_else(|| anyhow::anyhow!("source required for open_html"))?;
            let path = std::path::Path::new(&source);
            anyhow::ensure!(path.exists(), "file not found: {source}");
            #[cfg(windows)]
            std::process::Command::new("cmd").args(["/C", "start", "", &source]).spawn().ok();
            #[cfg(target_os = "macos")]
            std::process::Command::new("open").arg(&source).spawn().ok();
            #[cfg(all(unix, not(target_os = "macos")))]
            std::process::Command::new("xdg-open").arg(&source).spawn().ok();
            Ok(format!("{{\"ok\":true,\"opened\":\"{source}\"}}"))
        }
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn markdown_to_html(md: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    let mut code_buf = String::new();
    let mut list_type: Option<char> = None;
    for line in md.lines() {
        if line.starts_with("```") {
            if in_code {
                out.push_str(&format!("<pre><code>{}</code></pre>\n", html_escape(&code_buf)));
                code_buf.clear();
            }
            in_code = !in_code;
            continue;
        }
        if in_code {
            code_buf.push_str(line);
            code_buf.push('\n');
            continue;
        }
        if let Some(rest) = line.strip_prefix("# ") {
            out.push_str(&format!("<h1>{}</h1>\n", process_inline(rest)));
            list_type = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("## ") {
            out.push_str(&format!("<h2>{}</h2>\n", process_inline(rest)));
            list_type = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("### ") {
            out.push_str(&format!("<h3>{}</h3>\n", process_inline(rest)));
            list_type = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("- ") {
            if list_type != Some('-') { out.push_str("<ul>\n"); list_type = Some('-'); }
            out.push_str(&format!("<li>{}</li>\n", process_inline(rest)));
            continue;
        }
        if line.starts_with("1. ") || line.starts_with("2. ") || line.starts_with("3. ") {
            if list_type != Some('1') { out.push_str("<ol>\n"); list_type = Some('1'); }
            let rest = &line[3..];
            out.push_str(&format!("<li>{}</li>\n", process_inline(rest)));
            continue;
        }
        if line.trim().is_empty() {
            if list_type == Some('-') { out.push_str("</ul>\n"); list_type = None; }
            if list_type == Some('1') { out.push_str("</ol>\n"); list_type = None; }
            out.push('\n');
            continue;
        }
        out.push_str(&format!("<p>{}</p>\n", process_inline(line)));
        list_type = None;
    }
    if in_code { out.push_str(&format!("<pre><code>{}</code></pre>\n", html_escape(&code_buf))); }
    if list_type == Some('-') { out.push_str("</ul>\n"); }
    if list_type == Some('1') { out.push_str("</ol>\n"); }
    out
}

fn process_inline(s: &str) -> String {
    let s = html_escape(s);
    let s = regex_inline(&s);
    s
}

fn regex_inline(s: &str) -> String {
    use std::sync::OnceLock;
    static RE_LINK: OnceLock<regex::Regex> = OnceLock::new();
    static RE_BOLD: OnceLock<regex::Regex> = OnceLock::new();
    static RE_ITAL: OnceLock<regex::Regex> = OnceLock::new();
    static RE_CODE: OnceLock<regex::Regex> = OnceLock::new();
    let link = RE_LINK.get_or_init(|| regex::Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap());
    let bold = RE_BOLD.get_or_init(|| regex::Regex::new(r"\*\*([^*]+)\*\*").unwrap());
    let ital = RE_ITAL.get_or_init(|| regex::Regex::new(r"\*([^*]+)\*").unwrap());
    let code = RE_CODE.get_or_init(|| regex::Regex::new(r"`([^`]+)`").unwrap());
    let mut s = link.replace_all(s, "<a href=\"$2\">$1</a>").to_string();
    s = bold.replace_all(&s, "<strong>$1</strong>").to_string();
    s = ital.replace_all(&s, "<em>$1</em>").to_string();
    s = code.replace_all(&s, "<code>$1</code>").to_string();
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_simple_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let args = Args {
            action: Action::MdToHtml,
            text: Some("# Title\n\nA paragraph.".into()),
            source: None,
            output_path: Some(dir.path().join("out.html").to_string_lossy().to_string()),
            title: None,
        };
        run(dir.path(), args).unwrap();
        let content = std::fs::read_to_string(dir.path().join("out.html")).unwrap();
        assert!(content.contains("<h1>Title</h1>"));
        assert!(content.contains("<p>A paragraph.</p>"));
    }
}
