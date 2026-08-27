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

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::OnceCell;

/// Server handle shared across `open_preview` calls in one process.
struct PreviewServer {
    addr: SocketAddr,
    token: String,
}

static PREVIEW: OnceCell<PreviewServer> = OnceCell::const_new();

/// Per-process preview token — 64-bit hex, generated once. The browser URL
/// includes `?token=<this>`; the server rejects any request without a valid
/// token or a same-origin `Origin`/`Referer` header. The server still binds
/// only to `127.0.0.1`, but the token prevents a malicious page from
/// embedding the preview URL in an `<img>` or `<iframe>` and reading other
/// workspace files via a different origin.
fn preview_token() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut hasher = DefaultHasher::new();
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    // Mix in a random stack address for extra entropy without external crates.
    let x: u8 = 0;
    std::ptr::from_ref(&x).hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

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
/// workspace root, absolute paths, and symlink escapes.
fn resolve_safe(root: &Path, url_path: &str) -> Option<PathBuf> {
    let stripped = url_path.strip_prefix('/').unwrap_or(url_path);
    // Full percent-decode instead of only %20.
    let decoded = urlencoding::decode(stripped).ok()?.into_owned();
    let rel = Path::new(&decoded);
    for comp in rel.components() {
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            _ => return None,
        }
    }
    if rel.as_os_str().is_empty() {
        return None;
    }
    let full = root.join(rel);
    // Avoid following a symlink that escapes the workspace.
    let canonical_root = root.canonicalize().ok()?;
    let canonical_full = full.canonicalize().ok()?;
    if !canonical_full.starts_with(&canonical_root) {
        return None;
    }
    if canonical_full.is_file() {
        Some(canonical_full)
    } else {
        None
    }
}

/// Reads one HTTP request (first line + headers) from the socket.
/// Returns the raw request text (up to 8 KiB) so the caller can inspect
/// both the path and headers (`Origin`, `Referer`, etc.).
async fn read_request(stream: &mut TcpStream) -> Option<String> {
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf).await.ok()?;
    if n == 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&buf[..n]).into_owned())
}

/// Handles one connection: parse path, enforce preview token + Origin check,
/// serve file or error, close.
async fn handle_conn(mut stream: TcpStream, root: PathBuf, token: String, addr: SocketAddr) {
    let Some(raw_req) = read_request(&mut stream).await else {
        return;
    };
    let mut lines = raw_req.lines();
    let request_line = match lines.next() {
        Some(l) => l,
        None => return,
    };
    let mut parts = request_line.split_whitespace();
    let (_method, url_path_full) = (parts.next(), parts.next());
    let Some(url_path_full) = url_path_full else {
        return;
    };
    // Collect headers (lowercased) for Origin/Referer checks.
    let mut origin: Option<String> = None;
    let mut referer: Option<String> = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "origin" => origin = Some(v.trim().to_owned()),
                "referer" => referer = Some(v.trim().to_owned()),
                _ => {}
            }
        }
    }
    // --- Preview auth: token or same-origin check ---
    // The URL is `http://127.0.0.1:PORT/path?token=...`. We allow the request
    // if any of:
    //  1. `?token=<expected>` (or `&token=`) is present and matches, OR
    //  2. `Origin` header is `http://127.0.0.1:PORT` / `http://localhost:PORT`, OR
    //  3. `Referer` header starts with the same origin (covers <link>/<script> fetches
    //     that the browser makes after the initial page load — they carry Referer, not Origin).
    let has_valid_token = url_path_full.contains(&format!("token={token}"));
    let expected_origin_1 = format!("http://127.0.0.1:{}", addr.port());
    let expected_origin_2 = format!("http://localhost:{}", addr.port());
    let has_valid_origin = origin
        .as_deref()
        .is_some_and(|o| o.starts_with(&expected_origin_1) || o.starts_with(&expected_origin_2));
    let has_valid_referer = referer
        .as_deref()
        .is_some_and(|r| r.starts_with(&expected_origin_1) || r.starts_with(&expected_origin_2));
    if !has_valid_token && !has_valid_origin && !has_valid_referer {
        // No token and no same-origin header — likely a cross-origin probe.
        // The initial browser navigation always has the token, so this is safe.
        // Audit the rejection (V-009: forensic trail of all preview access).
        if let Ok(cwd) = std::env::current_dir() {
            crate::audit::record(
                &cwd,
                crate::audit::AuditKind::Preview,
                false,
                &format!("rejected (no token, no same-origin): {url_path_full}"),
            );
        }
        let body = b"403 Forbidden: invalid preview token or origin\n";
        let head = format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes()).await;
        let _ = stream.write_all(body).await;
        let _ = stream.flush().await;
        return;
    }
    // Strip query string (`?token=...` / `?v=1` cache busters) before resolving.
    let url_path = url_path_full.split('?').next().unwrap_or(url_path_full);
    // Audit every successful preview access for the forensic trail (V-009).
    if let Ok(cwd) = std::env::current_dir() {
        crate::audit::record(
            &cwd,
            crate::audit::AuditKind::Preview,
            true,
            url_path,
        );
    }
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
/// Returns the bound address + token. Uses `OnceCell` to eliminate the
/// check-lock-await-write TOCTOU that previously allowed two concurrent
/// callers to bind two listeners and leak one.
///
/// The token is generated once per process and required as `?token=` on the
/// URL; same-origin `Origin`/`Referer` also passes so sub-resources (css/js)
/// load without re-adding the token.
async fn ensure_server() -> Result<(SocketAddr, String)> {
    let server = PREVIEW
        .get_or_try_init(|| async {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .await
                .context("cannot bind preview server")?;
            let addr = listener.local_addr().context("cannot read preview port")?;
            let token = preview_token();
            let root = std::env::current_dir().context("cannot resolve working directory")?;
            let token_clone = token.clone();
            tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            let r = root.clone();
                            let t = token_clone.clone();
                            let a = addr;
                            tokio::spawn(handle_conn(stream, r, t, a));
                        }
                        Err(_) => continue,
                    }
                }
            });
            Ok::<PreviewServer, anyhow::Error>(PreviewServer { addr, token })
        })
        .await?;
    Ok((server.addr, server.token.clone()))
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
///
/// The URL includes `?token=<per-process secret>` so only the browser
/// that was launched from this process can fetch the workspace files. The
/// server also checks `Origin`/`Referer` for same-origin requests, so
/// sub-resources (css/js) load without extra tokens.
pub async fn open(path: Option<&str>) -> Result<String> {
    let path = path.unwrap_or("index.html");
    anyhow::ensure!(
        !path.is_empty(),
        "path must not be empty — pass a workspace-relative file like index.html"
    );
    let (addr, token) = ensure_server().await?;
    // Append ?token= for auth; `?v=1` style cache busters are handled
    // separately in `handle_conn` (query string stripped before resolving).
    let sep = if path.contains('?') { '&' } else { '?' };
    let url = format!(
        "http://{addr}/{}{sep}token={token}",
        path.trim_start_matches('/')
    );
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
        assert_eq!(
            content_type(Path::new("a.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("a.PNG")), "image/png");
        assert_eq!(
            content_type(Path::new("a.unknown")),
            "application/octet-stream"
        );
    }

    #[tokio::test]
    async fn server_serves_workspace_files() {
        let root = ws();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let token = "testtoken123".to_owned();
        let tok_clone = token.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let r = root.clone();
                let t = tok_clone.clone();
                let a = addr;
                tokio::spawn(handle_conn(stream, r, t, a));
            }
        });
        // Valid token passes
        let mut conn = TcpStream::connect(addr).await.unwrap();
        let req = format!("GET /index.html?token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        conn.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        conn.read_to_end(&mut buf).await.unwrap();
        let body = String::from_utf8_lossy(&buf);
        assert!(body.contains("200 OK"), "{body}");
        assert!(body.contains("<h1>hi</h1>"), "{body}");
        assert!(body.contains("text/html"), "{body}");
        // Missing token without valid Origin/Referer is rejected
        let mut conn2 = TcpStream::connect(addr).await.unwrap();
        conn2
            .write_all(b"GET /index.html HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let mut buf2 = Vec::new();
        conn2.read_to_end(&mut buf2).await.unwrap();
        let body2 = String::from_utf8_lossy(&buf2);
        assert!(body2.contains("403 Forbidden"), "{body2}");
        // Same-origin Origin header without token is allowed (sub-resource)
        let mut conn3 = TcpStream::connect(addr).await.unwrap();
        let req3 = format!(
            "GET /sub/style.css HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://127.0.0.1:{}\r\n\r\n",
            addr.port()
        );
        conn3.write_all(req3.as_bytes()).await.unwrap();
        let mut buf3 = Vec::new();
        conn3.read_to_end(&mut buf3).await.unwrap();
        let body3 = String::from_utf8_lossy(&buf3);
        assert!(body3.contains("200 OK"), "{body3}");
    }
}
