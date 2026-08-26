//! Local static-file preview server for the `open_preview` agent tool.
//!
//! Serves workspace files over `127.0.0.1` on a random free port so HTML
//! previews (with relative CSS/JS/images) work exactly like a dev server,
//! then opens the user's default browser at the requested path. One server
//! is shared per process: subsequent calls reuse the port. The task is
//! detached — it terminates with the process, which is the right lifetime
//! for a TUI session.

use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Server handle shared across `open_preview` calls in one process.
struct PreviewServer {
    addr: SocketAddr,
}

static PREVIEW: Mutex<Option<PreviewServer>> = Mutex::new(None);

/// Extension → `Content-Type` map for the file types previews actually use.
fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "txt" | "md" | "log" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// Resolves a URL path to a workspace file, rejecting traversal outside the
/// workspace root and absolute paths.
fn resolve_safe(root: &Path, url_path: &str) -> Option<PathBuf> {
    let stripped = url_path.strip_prefix('/').unwrap_or(url_path);
    // Full percent-decode instead of only %20.
    let decoded = urlencoding::decode(stripped).ok().unwrap_or_default().into_owned();
    let rel = Path::new(&decoded);
    let mut safe = true;
    for comp in rel.components() {
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            _ => safe = false,
        }
    }
    if !safe || rel.as_os_str().is_empty() {
        return None;
    }
    let full = root.join(rel);
    if full.is_file() {
        Some(full)
    } else {
        None
    }
}

/// Reads one HTTP request's first line (`GET /path HTTP/1.1`) from the socket.
async fn read_request(stream: &mut TcpStream) -> Option<String> {
    let mut buf = [0u8; 2048];
    let n = stream.read(&mut buf).await.ok()?;
    let req = String::from_utf8_lossy(&buf[..n]);
    req.lines().next().map(str::to_owned)
}

/// Handles one connection: parse path, serve file or 404, close.
async fn handle_conn(mut stream: TcpStream, root: PathBuf) {
    let Some(request_line) = read_request(&mut stream).await else {
        return;
    };
    let mut parts = request_line.split_whitespace();
    let (_method, url_path) = (parts.next(), parts.next());
    let Some(url_path) = url_path else {
        return;
    };
    // Strip query string (`?v=1` cache busters) before resolving.
    let url_path = url_path.split('?').next().unwrap_or(url_path);
    let (status, body, ctype) = match resolve_safe(&root, url_path) {
        Some(file) => match tokio::fs::read(&file).await {
            Ok(bytes) => ("200 OK".to_owned(), bytes, content_type(&file).to_owned()),
            Err(_) => (
                "500 Internal Server Error".to_owned(),
                b"cannot read file\n".to_vec(),
                "text/plain".to_owned(),
            ),
        },
        None => (
            "404 Not Found".to_owned(),
            b"not found\n".to_vec(),
            "text/plain".to_owned(),
        ),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(&body).await;
    let _ = stream.flush().await;
}

/// Starts the static server on a random free port, if not already running.
/// Returns the bound address.
async fn ensure_server() -> Result<SocketAddr> {
    if let Some(existing) = PREVIEW
        .lock()
        .map_err(|_| anyhow::anyhow!("preview state poisoned"))?
        .as_ref()
        .map(|s| s.addr)
    {
        return Ok(existing);
    }
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("cannot bind preview server")?;
    let addr = listener.local_addr().context("cannot read preview port")?;
    let root = std::env::current_dir().context("cannot resolve working directory")?;
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(handle_conn(stream, root.clone()));
                }
                Err(_) => continue,
            }
        }
    });
    if let Ok(mut guard) = PREVIEW.lock() {
        *guard = Some(PreviewServer { addr });
    }
    Ok(addr)
}

/// Opens `url` in the OS default browser without flashing a console window.
fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .context("cannot launch default browser")?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .context("cannot launch default browser")?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .context("cannot launch default browser")?;
    }
    Ok(())
}

/// Ensures the preview server is running and opens `path` (workspace-relative,
/// default `index.html`) in the browser. Returns a JSON result for the model.
pub async fn open(path: Option<&str>) -> Result<String> {
    let path = path.unwrap_or("index.html");
    anyhow::ensure!(
        !path.is_empty(),
        "path must not be empty — pass a workspace-relative file like index.html"
    );
    let addr = ensure_server().await?;
    let url = format!("http://{addr}/{}", path.trim_start_matches('/'));
    open_browser(&url)?;
    Ok(serde_json::json!({
        "url": url,
        "opened": path,
        "note": "Preview server is running for this session; relative assets (css/js/images) resolve against the workspace root."
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("govinda-preview-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("index.html"), "<h1>hi</h1>").unwrap();
        std::fs::write(dir.join("sub").join("style.css"), "body{}").unwrap();
        dir
    }

    #[test]
    fn resolves_files_inside_root() {
        let root = ws();
        assert!(resolve_safe(&root, "/index.html").is_some());
        assert!(resolve_safe(&root, "sub/style.css").is_some());
        assert!(resolve_safe(&root, "/missing.html").is_none());
        assert!(resolve_safe(&root, "/../secret.txt").is_none());
        assert!(resolve_safe(&root, "a/../../b.txt").is_none());
        assert!(resolve_safe(&root, "/").is_none());
    }

    #[test]
    fn mime_types_map_by_extension() {
        assert_eq!(content_type(Path::new("a.html")), "text/html; charset=utf-8");
        assert_eq!(content_type(Path::new("a.PNG")), "image/png");
        assert_eq!(content_type(Path::new("a.unknown")), "application/octet-stream");
    }

    #[tokio::test]
    async fn server_serves_workspace_files() {
        let root = ws();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let r = root.clone();
                tokio::spawn(handle_conn(stream, r));
            }
        });
        let mut conn = TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /index.html HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let mut buf = Vec::new();
        conn.read_to_end(&mut buf).await.unwrap();
        let body = String::from_utf8_lossy(&buf);
        assert!(body.contains("200 OK"), "{body}");
        assert!(body.contains("<h1>hi</h1>"), "{body}");
        assert!(body.contains("text/html"), "{body}");
    }
}
