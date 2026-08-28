//! OpenCode bridge: detect a local OpenCode installation and borrow its
//! connected providers/models so Govinda can chat through them directly.
//!
//! Two discovery paths, tried in order:
//!   1. A running OpenCode server (`opencode serve` or the TUI's embedded
//!      server) answers `GET /global/health`; its `/config/providers`
//!      endpoint lists everything the user connected.
//!   2. Otherwise OpenCode's own files are parsed directly: `opencode.json`
//!      for configured endpoints and `auth.json` for credentials.
//!
//! Only OpenAI-compatible backends are surfaced; Govinda speaks one wire
//! format (see `src/api.rs`). Credentials never leave memory un-zeroized and
//! are never written to Govinda's own config.

use crate::provider::{Auth, Provider};
use anyhow::{Context as _, Result};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

const DEFAULT_SERVER: &str = "http://127.0.0.1:4096";
const PROBE_TIMEOUT: Duration = Duration::from_millis(750);
const CATALOG_TIMEOUT: Duration = Duration::from_secs(5);

/// Prefix used for every borrowed provider's stable key (`opencode-<pid>`),
/// both for config persistence and `/provider` round-trips.
pub const KEY_PREFIX: &str = "opencode-";

// ---------------------------------------------------------------------------
// Provider impl
// ---------------------------------------------------------------------------

/// One borrowed OpenCode backend. Wire-compatible with every other preset:
/// `{base}/chat/completions` over SSE, bearer auth from OpenCode's own
/// credential store.
#[derive(Clone)]
pub struct OcProvider {
    /// The underlying OpenCode provider id (e.g. `openai`, `zen`, a custom id).
    pub pid: String,
    base_url: String,
    auth: Auth,
}

impl OcProvider {
    pub fn new(pid: impl Into<String>, base_url: impl Into<String>, auth: Auth) -> Self {
        Self {
            pid: pid.into(),
            base_url: base_url.into(),
            auth,
        }
    }
}

impl Provider for OcProvider {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn key(&self) -> Cow<'static, str> {
        Cow::Owned(format!("{KEY_PREFIX}{}", self.pid))
    }

    fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn models_url(&self) -> Option<String> {
        Some(format!("{}/models", self.base_url.trim_end_matches('/')))
    }

    fn auth(&self) -> Auth {
        self.auth.clone()
    }
}

// ---------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------

/// One connectable backend discovered from OpenCode.
pub struct OcEntry {
    pub pid: String,
    pub base_url: String,
    pub auth: Auth,
    /// Model ids advertised up front; may be empty (live `/models` still works).
    pub models: Vec<String>,
}

pub struct OcCatalog {
    pub entries: Vec<OcEntry>,
    /// `(providerID, modelID)` OpenCode itself would use by default.
    pub default: Option<(String, String)>,
}

impl OcCatalog {
    pub fn find(&self, pid: &str) -> Option<&OcEntry> {
        self.entries.iter().find(|e| e.pid == pid)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The backend + model to activate: OpenCode's own default when present,
    /// otherwise the first entry's first model.
    pub fn pick_default(&self) -> Option<(&OcEntry, &str)> {
        if let Some((pid, mid)) = &self.default {
            if let Some(entry) = self.find(pid) {
                return Some((entry, mid.as_str()));
            }
        }
        let entry = self.entries.first()?;
        let model = entry.models.first()?;
        Some((entry, model.as_str()))
    }
}

/// Endpoints Govinda knows how to speak for well-known OpenCode provider ids.
fn known_endpoint(pid: &str) -> Option<&'static str> {
    match pid {
        "openai" => Some("https://api.openai.com/v1"),
        "openrouter" => Some("https://openrouter.ai/api/v1"),
        "groq" => Some("https://api.groq.com/openai/v1"),
        "deepseek" => Some("https://api.deepseek.com/v1"),
        "mistral" => Some("https://api.mistral.ai/v1"),
        "xai" => Some("https://api.x.ai/v1"),
        "together" => Some("https://api.together.xyz/v1"),
        "fireworks" => Some("https://api.fireworks.ai/inference/v1"),
        "opencode" | "zen" => Some("https://opencode.ai/zen/v1"),
        // Gemini's OpenAI-compatibility surface, not the native generate API.
        "google" => Some("https://generativelanguage.googleapis.com/v1beta/openai"),
        "lmstudio" => Some("http://127.0.0.1:1234/v1"),
        "ollama" => Some("http://localhost:11434/v1"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// File locations
// ---------------------------------------------------------------------------

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// OpenCode's credential store: `{providerID: {type, key?, access?, ...}}`.
pub fn auth_file() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let p = PathBuf::from(xdg).join("opencode").join("auth.json");
        if p.exists() {
            return Some(p);
        }
    }
    if let Some(home) = home_dir() {
        let p = home
            .join(".local")
            .join("share")
            .join("opencode")
            .join("auth.json");
        if p.exists() {
            return Some(p);
        }
    }
    for var in ["APPDATA", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(var) {
            let p = PathBuf::from(base).join("opencode").join("auth.json");
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

/// Candidate locations of OpenCode's `opencode.json`.
fn config_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = home_dir() {
        out.push(home.join(".config").join("opencode").join("opencode.json"));
        out.push(home.join(".opencode").join("opencode.json"));
    }
    for var in ["APPDATA", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(var) {
            out.push(PathBuf::from(base).join("opencode").join("opencode.json"));
        }
    }
    out.into_iter().filter(|p| p.exists()).collect()
}

// ---------------------------------------------------------------------------
// Pure parsing helpers (unit-tested without HTTP or real files)
// ---------------------------------------------------------------------------

/// Pulls a bearer credential out of one `auth.json` entry. Only plain keys
/// and OAuth access tokens are usable here; anything else (refresh-only,
/// wellknown flows) is ignored rather than guessed at.
pub(crate) fn extract_auth(entry: &Value) -> Option<Auth> {
    for field in ["key", "access"] {
        if let Some(raw) = entry.get(field).and_then(Value::as_str) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return Some(Auth::Bearer(Zeroizing::new(trimmed.to_owned())));
            }
        }
    }
    None
}

/// Reads `auth.json` into `providerID → Auth`.
pub(crate) fn load_auth_map() -> BTreeMap<String, Auth> {
    let mut map = BTreeMap::new();
    let Some(path) = auth_file() else {
        return map;
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return map;
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
        return map;
    };
    let Some(obj) = parsed.as_object() else {
        return map;
    };
    for (pid, entry) in obj {
        if let Some(auth) = extract_auth(entry) {
            map.insert(pid.clone(), auth);
        }
    }
    map
}

/// Parses a `/config/providers` response body:
/// `{providers: [{id, models}], default: {pid: model}}`.
/// Models may be an object map or a list; both are handled leniently.
pub(crate) fn parse_catalog_response(body: &Value) -> OcCatalog {
    let mut entries = Vec::new();
    let auth_map = load_auth_map();
    if let Some(providers) = body.get("providers").and_then(Value::as_array) {
        for p in providers {
            let Some(pid) = p.get("id").and_then(Value::as_str) else {
                continue;
            };
            let models = parse_model_list(p.get("models"));
            let Some((base_url, auth)) = resolve_endpoint(pid, p, &auth_map) else {
                continue;
            };
            entries.push(OcEntry {
                pid: pid.to_owned(),
                base_url,
                auth,
                models,
            });
        }
    }
    let mut default = None;
    if let Some(defs) = body.get("default").and_then(Value::as_object) {
        for (pid, mid) in defs {
            if let Some(mid) = mid.as_str() {
                default = Some((pid.clone(), mid.to_owned()));
                break;
            }
        }
    }
    OcCatalog { entries, default }
}

/// Models appear as an object map (`{id: {...}}`) or occasionally a plain
/// array; normalize both into sorted ids for stable display.
fn parse_model_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Object(map)) => {
            let mut ids: Vec<String> = map.keys().cloned().collect();
            ids.sort();
            ids
        }
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(s) => Some(s.clone()),
                Value::Object(o) => o.get("id").and_then(Value::as_str).map(str::to_owned),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Endpoint resolution for one provider entry from the catalog:
/// explicit `options.baseURL` wins, then the static table; a credential is
/// required unless the endpoint is a known local server.
fn resolve_endpoint(
    pid: &str,
    entry: &Value,
    auth_map: &BTreeMap<String, Auth>,
) -> Option<(String, Auth)> {
    let options = entry.get("options");
    let explicit = options
        .and_then(|o| o.get("baseURL"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let auth = entry
        .get("options")
        .and_then(|o| o.get("apiKey"))
        .and_then(Value::as_str)
        .map(|k| Auth::Bearer(Zeroizing::new(k.trim().to_owned())))
        .or_else(|| auth_map.get(pid).cloned());

    if let Some(url) = explicit {
        let local = url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost");
        if auth.is_none() && !local {
            return None;
        }
        return Some((url, auth.unwrap_or(Auth::None)));
    }
    let endpoint = known_endpoint(pid)?;
    let local =
        endpoint.starts_with("http://127.0.0.1") || endpoint.starts_with("http://localhost");
    if auth.is_none() && !local {
        return None;
    }
    Some((endpoint.to_owned(), auth.unwrap_or(Auth::None)))
}

/// Parses `opencode.json` for the offline path: configured providers with
/// explicit OpenAI-compatible `baseURL`s, plus `model` as the default pick.
pub(crate) fn parse_config_body(body: &Value, auth_map: &BTreeMap<String, Auth>) -> OcCatalog {
    let mut entries = Vec::new();
    if let Some(providers) = body.get("provider").and_then(Value::as_object) {
        for (pid, p) in providers {
            let Some(options) = p.get("options") else {
                continue;
            };
            let Some(base_url) = options
                .get("baseURL")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let auth = options
                .get("apiKey")
                .and_then(Value::as_str)
                .map(|k| Auth::Bearer(Zeroizing::new(k.trim().to_owned())))
                .or_else(|| auth_map.get(pid).cloned())
                .unwrap_or(Auth::None);
            let local = base_url.starts_with("http://127.0.0.1")
                || base_url.starts_with("http://localhost");
            if matches!(auth, Auth::None) && !local {
                continue;
            }
            entries.push(OcEntry {
                pid: pid.clone(),
                base_url: base_url.to_owned(),
                auth,
                models: parse_model_list(p.get("models")),
            });
        }
    }
    // Providers authenticated in auth.json whose endpoint is statically known.
    for (pid, auth) in auth_map {
        if entries.iter().any(|e| e.pid == *pid) || known_endpoint(pid).is_none() {
            continue;
        }
        entries.push(OcEntry {
            pid: pid.clone(),
            base_url: known_endpoint(pid).unwrap_or_default().to_owned(),
            auth: auth.clone(),
            models: Vec::new(),
        });
    }
    let default = body
        .get("model")
        .and_then(Value::as_str)
        .and_then(split_model_ref);
    OcCatalog { entries, default }
}

/// Splits OpenCode's `provider/model` reference form.
pub(crate) fn split_model_ref(model_ref: &str) -> Option<(String, String)> {
    let (pid, mid) = model_ref.split_once('/')?;
    (!pid.is_empty() && !mid.is_empty()).then(|| (pid.to_owned(), mid.to_owned()))
}

/// Offline catalog: parse OpenCode's own files without touching the network.
pub(crate) fn local_catalog() -> OcCatalog {
    let auth_map = load_auth_map();
    for path in config_files() {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(body) = serde_json::from_str::<Value>(&raw) {
                return parse_config_body(&body, &auth_map);
            }
        }
    }
    parse_config_body(&Value::Null, &auth_map)
}

// ---------------------------------------------------------------------------
// Network paths
// ---------------------------------------------------------------------------

fn server_base() -> String {
    std::env::var("OPENCODE_SERVER_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_SERVER.to_owned())
}

fn authorized(mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    if let Ok(password) = std::env::var("OPENCODE_SERVER_PASSWORD") {
        if !password.trim().is_empty() {
            let user =
                std::env::var("OPENCODE_SERVER_USERNAME").unwrap_or_else(|_| "opencode".to_owned());
            req = req.basic_auth(user, Some(password));
        }
    }
    req
}

/// Probes for a running OpenCode server. Returns its version when found.
pub async fn probe(http: &reqwest::Client) -> Option<String> {
    let url = format!("{}/global/health", server_base());
    let response = tokio::time::timeout(PROBE_TIMEOUT, authorized(http.get(url)).send())
        .await
        .ok()?
        .ok()?;
    let body: Value = response.json().await.ok()?;
    let healthy = body.get("healthy").and_then(Value::as_bool).unwrap_or(true);
    healthy.then(|| {
        body.get("version")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned()
    })
}

/// Discovers every connectable backend: the running server's catalog when
/// available, OpenCode's config/auth files otherwise.
pub async fn fetch_catalog(http: &reqwest::Client) -> Result<OcCatalog> {
    if probe(http).await.is_some() {
        let url = format!("{}/config/providers", server_base());
        let response = tokio::time::timeout(CATALOG_TIMEOUT, authorized(http.get(url)).send())
            .await
            .map_err(|_| anyhow::anyhow!("opencode server catalog request timed out"))?
            .context("opencode server catalog request failed")?;
        let body: Value = response
            .json()
            .await
            .context("opencode server returned malformed catalog JSON")?;
        return Ok(parse_catalog_response(&body));
    }
    Ok(local_catalog())
}

/// Rebuilds a saved `opencode-<pid>` provider from disk state (sync — used
/// by `Config::load` when config.toml names one explicitly).
pub fn resolve_saved(key: &str, base_url: &str) -> Result<Arc<dyn Provider>> {
    let pid = key
        .strip_prefix(KEY_PREFIX)
        .filter(|p| !p.is_empty())
        .with_context(|| format!("invalid saved provider key '{key}'"))?;
    let auth = load_auth_map().remove(pid).unwrap_or(Auth::None);
    if matches!(auth, Auth::None)
        && !base_url.starts_with("http://127.0.0.1")
        && !base_url.starts_with("http://localhost")
    {
        anyhow::bail!(
            "no OpenCode credential found for '{pid}' — run '/opencode connect' or re-authenticate in OpenCode"
        );
    }
    Ok(Arc::new(OcProvider::new(pid, base_url, auth)))
}

/// Startup auto-connect: detect OpenCode, borrow its default backend.
/// Returns `Ok(None)` when nothing usable was found (never an error worth
/// surfacing at startup).
pub async fn auto_connect(
    http: &reqwest::Client,
    fallback_model: &str,
) -> Result<Option<(Arc<dyn Provider>, String)>> {
    let catalog = match fetch_catalog(http).await {
        Ok(catalog) => catalog,
        Err(_) => return Ok(None),
    };
    let Some((entry, model)) = catalog.pick_default() else {
        return Ok(None);
    };
    let model = if model.is_empty() {
        fallback_model.to_owned()
    } else {
        model.to_owned()
    };
    Ok(Some((
        Arc::new(OcProvider::new(
            entry.pid.clone(),
            entry.base_url.clone(),
            entry.auth.clone(),
        )),
        model,
    )))
}

/// Explicit `/opencode connect [pid]`: resolves one borrowed backend.
/// Returns `(provider, model, summary)` for the caller to activate.
pub async fn connect(
    http: &reqwest::Client,
    requested: Option<&str>,
) -> Result<(Arc<dyn Provider>, String, String)> {
    let catalog = fetch_catalog(http).await.context(
        "could not reach OpenCode (is it installed? start 'opencode' or check OPENCODE_SERVER_URL)",
    )?;
    if catalog.is_empty() {
        anyhow::bail!(
            "OpenCode is reachable but no compatible providers are connected — authenticate a provider inside OpenCode first"
        );
    }
    let (entry, model) = match requested {
        Some(pid) => {
            let entry = catalog.find(pid).with_context(|| {
                format!(
                    "provider '{pid}' not found among OpenCode's connected providers ({})",
                    catalog
                        .entries
                        .iter()
                        .map(|e| e.pid.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            (entry, entry.models.first().map(String::as_str))
        }
        None => catalog
            .pick_default()
            .map(|(e, m)| (e, Some(m)))
            .unwrap_or_else(|| {
                let e = &catalog.entries[0];
                (e, None)
            }),
    };
    let model = model.map(str::to_owned).unwrap_or_else(fallback_model_name);
    let provider = Arc::new(OcProvider::new(
        entry.pid.clone(),
        entry.base_url.clone(),
        entry.auth.clone(),
    ));
    let summary = format!(
        "connected via OpenCode: {} · {} · model {}",
        entry.pid, entry.base_url, model
    );
    Ok((provider, model, summary))
}

fn fallback_model_name() -> String {
    crate::config::DEFAULT_MODEL.to_owned()
}

/// Human-readable detection status for `/opencode status`.
pub async fn status_line(http: &reqwest::Client) -> String {
    match probe(http).await {
        Some(version) => format!("server detected at {} (v{version})", server_base()),
        None => {
            if auth_file().is_some() || !config_files().is_empty() {
                "installed (files found) but no server running — using stored credentials"
                    .to_owned()
            } else {
                "not detected (no server, no OpenCode files)".to_owned()
            }
        }
    }
}

/// Tries to start the OpenCode server if it's installed but not running.
/// Returns `true` if the server is now reachable, `false` otherwise.
pub async fn try_start_server(http: &reqwest::Client) -> bool {
    // Already running?
    if probe(http).await.is_some() {
        return true;
    }
    // Try `opencode serve` in the background.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("opencode")
            .arg("serve")
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("opencode")
            .arg("serve")
            .spawn();
    }
    // Wait up to 15 seconds for the server to come up.
    for _ in 0..15 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if probe(http).await.is_some() {
            return true;
        }
    }
    false
}

/// Ensures the opencode CLI is installed. Tries `npm install -g opencode`
/// if the binary is not found. Returns `true` if installed or successfully
/// installed, `false` if npm is unavailable or the install fails.
pub async fn ensure_installed() -> bool {
    // Already available?
    if tokio::process::Command::new("opencode")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return true;
    }
    // Try to install via npm.
    #[cfg(windows)]
    let argv = vec!["cmd".to_string(), "/C".to_string(), "npm install -g opencode --no-audit --no-fund".to_string()];
    #[cfg(not(windows))]
    let argv = vec!["npm".to_string(), "install".to_string(), "-g".to_string(), "opencode".to_string()];
    let status = tokio::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
    status.map(|s| s.success()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn auth_map(pairs: &[(&str, &str)]) -> BTreeMap<String, Auth> {
        pairs
            .iter()
            .map(|(k, v)| {
                (
                    (*k).to_owned(),
                    Auth::Bearer(Zeroizing::new((*v).to_owned())),
                )
            })
            .collect()
    }

    #[test]
    fn catalog_response_parses_providers_and_default() {
        let body = json!({
            "providers": [
                { "id": "openai", "models": { "gpt-4o": {}, "gpt-4o-mini": {} },
                  "options": { "baseURL": "https://api.openai.com/v1", "apiKey": "sk-test" } },
                { "id": "custom-local", "models": [{ "id": "qwen3" }],
                  "options": { "baseURL": "http://127.0.0.1:1337/v1" } },
                { "id": "anthropic", "models": { "claude-x": {} } }
            ],
            "default": { "openai": "gpt-4o-mini" }
        });
        let catalog = parse_catalog_response(&body);
        assert_eq!(
            catalog.entries.len(),
            2,
            "anthropic has no endpoint/key and must be skipped"
        );
        assert_eq!(
            catalog.default,
            Some(("openai".to_owned(), "gpt-4o-mini".to_owned()))
        );
        let openai = catalog.find("openai").unwrap();
        assert_eq!(openai.base_url, "https://api.openai.com/v1");
        assert_eq!(openai.models, vec!["gpt-4o", "gpt-4o-mini"]);
        let local = catalog.find("custom-local").unwrap();
        assert_eq!(local.auth, Auth::None);
        assert_eq!(local.models, vec!["qwen3"]);
    }

    #[test]
    fn known_endpoints_require_credentials_except_local_servers() {
        let none = BTreeMap::new();
        // No baseURL, no key, cloud endpoint → excluded.
        let entry = json!({ "id": "deepseek" });
        assert!(resolve_endpoint("deepseek", &entry, &none).is_none());
        // Key from auth map → included.
        let keyed = auth_map(&[("deepseek", "sk-test")]);
        let (url, auth) = resolve_endpoint("deepseek", &entry, &keyed).unwrap();
        assert_eq!(url, "https://api.deepseek.com/v1");
        assert!(matches!(auth, Auth::Bearer(_)));
        // Local servers need no key.
        let lmstudio = json!({ "id": "lmstudio" });
        let (url, auth) = resolve_endpoint("lmstudio", &lmstudio, &none).unwrap();
        assert_eq!(url, "http://127.0.0.1:1234/v1");
        assert_eq!(auth, Auth::None);
    }

    #[test]
    fn extract_auth_prefers_key_and_falls_back_to_access_token() {
        assert!(extract_auth(&json!({ "type": "wellknown" })).is_none());
        let api = extract_auth(&json!({ "type": "api", "key": " sk-live " }));
        assert!(matches!(api, Some(Auth::Bearer(_))));
        let oauth = extract_auth(&json!({ "type": "oauth", "access": "tok" }));
        assert!(matches!(oauth, Some(Auth::Bearer(_))));
    }

    #[test]
    fn config_body_yields_custom_providers_and_default_split() {
        let body = json!({
            "model": "atomic-chat/qwen3-coder",
            "provider": {
                "atomic-chat": {
                    "npm": "@ai-sdk/openai-compatible",
                    "options": { "baseURL": "http://127.0.0.1:1337/v1" },
                    "models": { "qwen3-coder": {} }
                }
            }
        });
        let catalog = parse_config_body(&body, &BTreeMap::new());
        assert_eq!(catalog.entries.len(), 1);
        assert_eq!(
            catalog.default,
            Some(("atomic-chat".to_owned(), "qwen3-coder".to_owned()))
        );
        assert_eq!(catalog.pick_default().unwrap().1, "qwen3-coder");
    }

    #[test]
    fn model_reference_splits_on_single_slash_only_when_both_sides_exist() {
        assert_eq!(
            split_model_ref("openai/gpt-4o"),
            Some(("openai".to_owned(), "gpt-4o".to_owned()))
        );
        assert_eq!(split_model_ref("gpt-4o"), None);
        assert_eq!(split_model_ref("openai/"), None);
    }

    #[test]
    fn oc_provider_wire_format_matches_openai_conventions() {
        let p = OcProvider::new("zen", "https://opencode.ai/zen/v1/", Auth::None);
        assert_eq!(p.id(), "opencode");
        assert_eq!(p.key(), "opencode-zen");
        assert_eq!(p.chat_url(), "https://opencode.ai/zen/v1/chat/completions");
        assert_eq!(
            p.models_url().as_deref(),
            Some("https://opencode.ai/zen/v1/models")
        );
    }

    #[test]
    fn catalog_pick_default_prefers_server_default_then_first_entry() {
        let mk = |pid: &str, models: &[&str]| OcEntry {
            pid: pid.to_owned(),
            base_url: format!("https://{pid}.example/v1"),
            auth: Auth::Bearer(Zeroizing::new("k".to_owned())),
            models: models.iter().map(|s| (*s).to_owned()).collect(),
        };
        let mut catalog = OcCatalog {
            entries: vec![mk("a", &["m1"]), mk("b", &["m2"])],
            default: Some(("b".to_owned(), "m2".to_owned())),
        };
        assert_eq!(catalog.pick_default().unwrap().0.pid, "b");
        catalog.default = Some(("missing".to_owned(), "x".to_owned()));
        assert_eq!(catalog.pick_default().unwrap().0.pid, "a");
        catalog.default = None;
        assert_eq!(catalog.pick_default().unwrap().1, "m1");
    }

    #[test]
    fn resolve_saved_rejects_unknown_prefix_and_missing_keys_for_cloud() {
        assert!(resolve_saved("mistral", "https://x/v1").is_err());
        // Cloud endpoint without any auth.json → actionable error.
        let err = match resolve_saved("opencode-nope", "https://api.example.com/v1") {
            Err(e) => e,
            Ok(_) => panic!("cloud provider without key must fail"),
        };
        assert!(err.to_string().contains("/opencode connect"), "{err}");
    }
}
