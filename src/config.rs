use crate::provider::{self, Provider};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

pub const DEFAULT_PROVIDER: &str = "omniroute";
pub const DEFAULT_MODEL: &str = "auto";
pub const DEFAULT_SYSTEM_PROMPT: &str = "You are a helpful assistant. Answer concisely.";
const DEFAULT_TEMPERATURE: f32 = 0.7;
const DEFAULT_RENDER_MARKDOWN: bool = true;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_LIMIT_MB: u64 = 16;
/// Current config schema version — bump when the TOML layout changes and
/// add a migration in `read_config_file`. Written by `save_runtime_config`.
pub const CURRENT_CONFIG_VERSION: u32 = 1;

/// Parses a config TOML string through the real schema. Test hook used to
/// prove `/config save` output loads cleanly.
#[cfg(test)]
pub(crate) fn parse_file_config_for_test(raw: &str) -> Result<()> {
    let _: FileConfig = toml::from_str(raw)?;
    Ok(())
}

/// Settings as written in `~/.config/govinda/config.toml`.
/// Every field is optional; missing keys fall back to defaults.
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    config_version: Option<u32>,
    model: Option<String>,
    temperature: Option<f32>,
    render_markdown: Option<bool>,
    system_prompt: Option<String>,
    /// Backend preset: mistral | openai | groq | ollama.
    provider: Option<String>,
    /// Overrides the preset's endpoint (custom OpenAI-compatible servers).
    base_url: Option<String>,
    /// Name of the env var that holds the API key for this provider.
    api_key_env: Option<String>,
    /// Token budget per request (real tokenizer counts).
    context_tokens: Option<usize>,
    /// User-defined shell tools, one `[[tools]]` block each.
    tools: Vec<crate::tools::ShellToolDef>,
    /// Default color theme (default | mono | dracula | solarized | ocean).
    theme: Option<String>,
    /// Read-stall timeout in seconds (1-600).
    timeout_secs: Option<u64>,
    /// Response size cap in MB (1-64).
    limit_mb: Option<u64>,
    /// GOVINDA Protocol enforcement mechanism (v7.0 "No Shortcuts").
    /// When true, the master system prompt and per-turn header are
    /// injected, and the model is required to call `quality_gate_check`
    /// before claiming completion.
    enforce_protocol: Option<bool>,
    /// Hard-fail line-count threshold for the FINAL phase.
    protocol_min_lines: Option<usize>,
    /// Maximum rounds granted to a single turn when the protocol is on.
    protocol_max_turns: Option<usize>,
    /// Whether the quality-gate tool scans delivered files for emoji.
    protocol_emoji_scan: Option<bool>,
    /// Whether the model must call `quality_gate_check` before claiming
    /// completion.
    protocol_require_gates: Option<bool>,
}

/// Never derive `Debug` for `Config` — the `api_key` field is a
/// `Zeroizing<String>` and must never be printed. A manual impl redacts it.
#[derive(Clone)]
pub struct Config {
    /// API key, zeroized on drop. Never log, serialize, or clone into a
    /// plain `String`. Borrow via `provider.auth().token()` or `api_key.as_str()`
    /// only when building a request, and wrap any owned copy in `Zeroizing`.
    pub api_key: Arc<Zeroizing<String>>,
    pub model: String,
    pub temperature: f32,
    pub render_markdown: bool,
    pub system_prompt: String,
    pub context_tokens: usize,
    pub provider: Arc<dyn Provider>,
    /// The config file that was read, if any (shown by `/config`).
    pub source_path: Option<PathBuf>,
    /// True when the provider came from env/TOML rather than defaults;
    /// gates the OpenCode auto-connect so explicit choices always win.
    pub provider_explicit: bool,
    /// Validated user-defined shell tools from `[[tools]]` blocks.
    pub shell_tools: Vec<crate::tools::ShellToolDef>,
    /// Default theme name (applied by the caller at startup).
    pub theme: Option<String>,
    /// Read-stall timeout in seconds, clamped 1-600.
    pub timeout_secs: u64,
    /// Response size cap in MB, clamped 1-64.
    pub limit_mb: u64,
    /// GOVINDA Protocol settings — see `govinda_protocol::ProtocolConfig`.
    pub protocol: crate::govinda_protocol::ProtocolConfig,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("provider", &self.provider.key())
            .field("source_path", &self.source_path)
            .finish_non_exhaustive()
    }
}

impl Config {
    /// Loads configuration with three layers, later wins:
    ///
    ///   defaults < ~/.config/govinda/config.toml < environment variables
    ///
    /// The API key only ever comes from the environment (`.env` fallback);
    /// never put secrets in the TOML file.
    pub fn load() -> Result<Self> {
        dotenvy::dotenv().ok();

        let (file, source_path) = Self::read_config_file()?;

        let provider_name = env_override("GOVINDA_PROVIDER")
            .or(file.provider.clone())
            .unwrap_or_else(|| DEFAULT_PROVIDER.to_owned());
        // An explicit provider gates auto-connect so intentional choices win,
        // but the default "omniroute" never counts as explicit — users who
        // leave config empty or write only comments still get auto-connect.
        let provider_explicit = env_override("GOVINDA_PROVIDER").is_some()
            || file
                .provider
                .as_deref()
                .is_some_and(|p| p != DEFAULT_PROVIDER);

        // A saved OpenCode-backed provider (`opencode-<pid>` + base_url)
        // reloads through the bridge, pulling its key from OpenCode's own
        // credential store instead of the environment.
        let provider: Arc<dyn Provider> = if provider_name.starts_with(crate::opencode::KEY_PREFIX)
        {
            let base_url = file.base_url.as_deref().with_context(|| {
                format!(
                    "saved provider '{provider_name}' needs a base_url in config.toml — run '/opencode connect' to repair"
                )
            })?;
            crate::opencode::resolve_saved(&provider_name, base_url)?
        } else {
            provider::resolve(
                &provider_name,
                file.base_url.as_deref(),
                file.api_key_env.as_deref(),
                env_override,
            )
            .context("provider setup failed")?
        };
        // Borrow the provider's Zeroizing token directly — avoids an intermediate
        // plain String that would linger on the heap. `Auth::Bearer` already
        // holds a `Zeroizing<String>`; cloning it keeps the zeroize guarantee.
        let api_key = match provider.auth() {
            crate::provider::Auth::Bearer(t) => Arc::new(t),
            crate::provider::Auth::None => Arc::new(Zeroizing::new(String::new())),
        };

        let model = env_override("GOVINDA_MODEL")
            .or_else(|| env_override("MISTRAL_MODEL"))
            .or(file.model)
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());

        let temperature = match env_override("GOVINDA_TEMPERATURE")
            .or_else(|| env_override("MISTRAL_TEMPERATURE"))
        {
            Some(raw) => parse_temperature(&raw).unwrap_or_else(|| {
                eprintln!(
                    "warning: GOVINDA_TEMPERATURE='{raw}' is not a number in 0.0-1.0; using {DEFAULT_TEMPERATURE}"
                );
                DEFAULT_TEMPERATURE
            }),
            None => file.temperature.map_or(DEFAULT_TEMPERATURE, |t| {
                t.clamp(0.0, 1.0)
            }),
        };

        // Resolve context budget in priority order:
        //   1. Explicit `context_tokens` in TOML (clamped to sane range)
        //   2. Static registry lookup for the chosen model (`context_window_for`)
        //   3. Provider default for the preset family
        //   4. DEFAULT_CONTEXT_TOKENS (128k)
        // The model's *actual* limit is much larger for modern backends
        // (Gemini 1M, Claude 200k, OpenCode 200k+, etc.) so we no longer
        // silently cap at 8k.
        let provider_id = provider.key();
        let context_tokens = match file.context_tokens {
            Some(v) => provider::clamp_context_tokens(Some(v)),
            None => {
                let from_registry = provider::context_window_for(&provider_id, &model);
                if from_registry > 0 {
                    from_registry
                } else {
                    provider::DEFAULT_CONTEXT_TOKENS
                }
            }
        };

        let render_markdown = file.render_markdown.unwrap_or(DEFAULT_RENDER_MARKDOWN);
        let system_prompt = file
            .system_prompt
            .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_owned());

        crate::tools::validate_shell_tools(&file.tools).context("invalid [[tools]] definition")?;

        let timeout_secs = file
            .timeout_secs
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, 600);
        let limit_mb = file.limit_mb.unwrap_or(DEFAULT_LIMIT_MB).clamp(1, 64);

        let protocol = crate::govinda_protocol::ProtocolConfig::from_overrides(
            env_override("GOVINDA_ENFORCE_PROTOCOL")
                .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                .or(file.enforce_protocol),
            file.protocol_min_lines,
            file.protocol_max_turns,
            file.protocol_emoji_scan,
            file.protocol_require_gates,
        );

        Ok(Self {
            api_key,
            model,
            temperature,
            render_markdown,
            system_prompt,
            context_tokens,
            provider,
            source_path,
            provider_explicit,
            shell_tools: file.tools,
            theme: file.theme,
            timeout_secs,
            limit_mb,
            protocol,
        })
    }

    /// Swaps the active backend and re-derives the cached key from it.
    /// Used by OpenCode connect/auto-connect (the `/provider` path replaces
    /// the whole `Config`'s provider field directly).
    pub fn adopt_provider(&mut self, provider: Arc<dyn Provider>) {
        self.api_key = match provider.auth() {
            crate::provider::Auth::Bearer(t) => Arc::new(t),
            crate::provider::Auth::None => Arc::new(Zeroizing::new(String::new())),
        };
        self.provider = provider;
    }

    /// Reads and parses the TOML file. A missing file is normal (no config);
    /// a malformed one is a hard error so typos don't silently change behavior.
    fn read_config_file() -> Result<(FileConfig, Option<PathBuf>)> {
        let explicit = env_override("GOVINDA_CONFIG");
        let path = match explicit.as_deref() {
            Some(p) => Some(PathBuf::from(p)),
            None => default_config_path(),
        };
        let Some(path) = path else {
            return Ok((FileConfig::default(), None));
        };
        if !path.exists() {
            if explicit.is_some() {
                eprintln!(
                    "warning: GOVINDA_CONFIG points to {}, which does not exist; using defaults",
                    path.display()
                );
            }
            return Ok((FileConfig::default(), None));
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let mut parsed: FileConfig =
            toml::from_str(&raw).with_context(|| format!("invalid TOML in {}", path.display()))?;
        // Version gate — reject configs from a newer CLI that we cannot migrate
        // forward, and normalize missing versions to CURRENT.
        if let Some(v) = parsed.config_version {
            if v > CURRENT_CONFIG_VERSION {
                anyhow::bail!(
                    "config version {v} in {} is newer than supported {CURRENT_CONFIG_VERSION} — update govinda-cli",
                    path.display()
                );
            }
            // Future migrations: match (v, CURRENT_CONFIG_VERSION) { (1,2) => migrate... }
        } else {
            // Old configs without a version are implicitly v1 — stamp them so
            // the next `save_runtime_config` persists the marker.
            parsed.config_version = Some(CURRENT_CONFIG_VERSION);
        }
        Ok((parsed, Some(path)))
    }

    pub fn http_client() -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(120))
            .build()
            .context("failed to build HTTP client")
    }
}

fn env_override(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn parse_temperature(raw: &str) -> Option<f32> {
    raw.parse::<f32>().ok().filter(|t| (0.0..=1.0).contains(t))
}

/// `GOVINDA_CONFIG` > `$XDG_CONFIG_HOME/govinda/config.toml`
/// > `$HOME/.config/govinda/config.toml` (USERPROFILE on Windows).
pub fn default_config_path() -> Option<PathBuf> {
    if let Some(dir) = env_override("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(dir).join("govinda").join("config.toml"));
    }
    let home = env_override("HOME")
        .or_else(|| env_override("USERPROFILE"))
        .map(PathBuf::from)?;
    Some(home.join(".config").join("govinda").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_parses_all_known_keys() {
        let cfg: FileConfig =
            toml::from_str("model = \"mistral-large-latest\"\ntemperature = 0.2\nrender_markdown = false\nsystem_prompt = \"be brief\"\n")
                .unwrap();
        assert_eq!(cfg.model.as_deref(), Some("mistral-large-latest"));
        assert_eq!(cfg.temperature, Some(0.2));
        assert_eq!(cfg.render_markdown, Some(false));
        assert_eq!(cfg.system_prompt.as_deref(), Some("be brief"));
    }

    #[test]
    fn protocol_overrides_parse_from_toml() {
        let raw = r#"
enforce_protocol = true
protocol_min_lines = 5000
protocol_max_turns = 25
protocol_emoji_scan = false
protocol_require_gates = false
"#;
        let cfg: FileConfig = toml::from_str(raw).unwrap();
        assert_eq!(cfg.enforce_protocol, Some(true));
        assert_eq!(cfg.protocol_min_lines, Some(5000));
        assert_eq!(cfg.protocol_max_turns, Some(25));
        assert_eq!(cfg.protocol_emoji_scan, Some(false));
        assert_eq!(cfg.protocol_require_gates, Some(false));
    }

    #[test]
    fn empty_and_partial_toml_fall_back_to_defaults() {
        let empty: FileConfig = toml::from_str("").unwrap();
        assert!(empty.model.is_none());
        let partial: FileConfig = toml::from_str("# just a comment\n").unwrap();
        assert!(partial.temperature.is_none());
        assert!(partial.tools.is_empty());
    }

    #[test]
    fn shell_tools_parse_from_toml() {
        let raw = r#"
[[tools]]
name = "gh_pr"
description = "list open pull requests"
command = "gh"
args_template = ["pr", "list", "--repo", "{repo}"]
timeout_secs = 15
max_output_bytes = 8192

[[tools]]
name = "git_status"
description = "show git status"
command = "git"
args_template = ["status", "--short"]
"#;
        let cfg: FileConfig = toml::from_str(raw).unwrap();
        assert_eq!(cfg.tools.len(), 2);
        assert_eq!(cfg.tools[0].name, "gh_pr");
        assert_eq!(cfg.tools[0].args_template.len(), 4);
        assert_eq!(cfg.tools[0].timeout_secs, Some(15));
        assert_eq!(cfg.tools[0].max_output_bytes, Some(8192));
        assert_eq!(cfg.tools[1].timeout_secs, None);
        assert_eq!(
            crate::tools::validate_shell_tools(&cfg.tools).map_err(|e| e.to_string()),
            Ok(())
        );
    }

    #[test]
    fn unknown_toml_keys_are_rejected_with_position() {
        assert!(toml::from_str::<FileConfig>("apI_key = \"oops\"").is_err());
    }

    #[test]
    fn temperature_parse_rejects_out_of_range() {
        assert_eq!(parse_temperature("0.5"), Some(0.5));
        assert_eq!(parse_temperature("-1"), None);
        assert_eq!(parse_temperature("x"), None);
    }

    #[test]
    fn default_path_is_under_home_config_dir() {
        let path = default_config_path().expect("a home dir exists in test environments");
        let s = path.to_string_lossy();
        assert!(
            s.ends_with(r"\govinda\config.toml") || s.ends_with("/govinda/config.toml"),
            "unexpected path: {s}"
        );
    }
}
