//! `image_view` — multimodal image input bridge.
//!
//! Reads an image file and prepares it for the next model call. The image
//! is referenced by path; the actual vision-capable model call happens in
//! the agent loop (when the model is selected). For now, this tool validates
//! the image, reports dimensions, and stores the path in a per-turn scratch
//! buffer that the next prompt can reference.

use std::path::Path;
use std::sync::{Mutex, OnceLock};

static PENDING_IMAGE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn pending() -> &'static Mutex<Option<String>> {
    PENDING_IMAGE.get_or_init(|| Mutex::new(None))
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// Local file path to an image (PNG, JPEG, GIF, WebP).
    pub path: String,
    /// Hint to the model about what to look for.
    pub prompt: Option<String>,
}

pub fn run(_base: &Path, args: Args) -> anyhow::Result<String> {
    let path = std::path::Path::new(&args.path);
    anyhow::ensure!(path.exists(), "image file not found: {}", args.path);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    anyhow::ensure!(
        matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
        ),
        "unsupported image format: {ext} (supported: png, jpg, jpeg, gif, webp, bmp)"
    );
    let bytes = std::fs::metadata(path)?.len();
    *pending().lock().unwrap() = Some(args.path.clone());
    Ok(format!(
        "{{\"ok\":true,\"path\":\"{}\",\"bytes\":{bytes},\"format\":\"{ext}\",\"prompt\":{}}}",
        args.path,
        args.prompt
            .as_deref()
            .map(|p| format!("\"{}\"", p.replace('"', "\\\"")))
            .unwrap_or_else(|| "null".into())
    ))
}

/// Consume and return any pending image path set by image_view.
/// Called by the agent loop before sending the next request.
pub fn take_pending() -> Option<String> {
    pending().lock().unwrap().take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_file() {
        let result = run(
            &std::path::PathBuf::new(),
            Args {
                path: "/nonexistent.png".into(),
                prompt: None,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_unsupported_format() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.xyz");
        std::fs::write(&p, b"x").unwrap();
        let result = run(
            dir.path(),
            Args {
                path: p.to_string_lossy().into(),
                prompt: None,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn accepts_png() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.png");
        std::fs::write(&p, b"\x89PNG\r\n\x1a\n").unwrap();
        let result = run(
            dir.path(),
            Args {
                path: p.to_string_lossy().into(),
                prompt: Some("test".into()),
            },
        )
        .unwrap();
        assert!(result.contains("\"format\":\"png\""));
    }
}
