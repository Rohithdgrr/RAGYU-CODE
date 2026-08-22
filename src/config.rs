use crate::provider::{self, Provider};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

pub const DEFAULT_MODEL: &str = "mistral-small-latest";
pub const DEFAULT_SYSTEM_PROMPT: &str = "You are a helpful assistant. Answer concisely.";
const DEFAULT_TEMPERATURE: f32 = 0.7;
const DEFAULT_RENDER_MARKDOWN: bool = true;

/// Settings as written in `~/.config/govinda/config.toml`.
/// Every field is optional; missing keys fall back to defaults.
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
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
}

#[derive(Clone)]
pub struct Config {
    pub api_key: Zeroizing<String>,
    pub model: String,
    pub temperature: f32,
    pub render_markdown: bool,
    pub system_prompt: String,
    pub context_tokens: usize,
    pub provider: Arc<dyn Provider>,
    /// The config file that was read, if any (shown by `/config`).
    pub source_path: Option<PathBuf>,
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
            .unwrap_or_else(|| "mistral".to_owned());
        let provider = provider::resolve(
            &provider_name,
            file.base_url.as_deref(),
            file.api_key_env.as_deref(),
            env_override,
        )
        .context("provider setup failed")?;
        let api_key = Zeroizing::new(provider.auth().token().unwrap_or_default().to_owned());

        let model = env_override("MISTRAL_MODEL")
            .or(file.model)
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());

        let temperature = match env_override("MISTRAL_TEMPERATURE") {
            Some(raw) => parse_temperature(&raw).unwrap_or_else(|| {
                eprintln!(
                    "warning: MISTRAL_TEMPERATURE='{raw}' is not a number in 0.0-1.0; using {DEFAULT_TEMPERATURE}"
                );
                DEFAULT_TEMPERATURE
            }),
            None => file.temperature.map_or(DEFAULT_TEMPERATURE, |t| {
                t.clamp(0.0, 1.0)
            }),
        };

        let context_tokens = provider::clamp_context_tokens(file.context_tokens);

        let render_markdown = file.render_markdown.unwrap_or(DEFAULT_RENDER_MARKDOWN);
        let system_prompt = file
            .system_prompt
            .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_owned());

        Ok(Self {
            api_key,
            model,
            temperature,
            render_markdown,
            system_prompt,
            context_tokens,
            provider,
            source_path,
        })
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
        let parsed: FileConfig =
            toml::from_str(&raw).with_context(|| format!("invalid TOML in {}", path.display()))?;
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
    fn empty_and_partial_toml_fall_back_to_defaults() {
        let empty: FileConfig = toml::from_str("").unwrap();
        assert!(empty.model.is_none());
        let partial: FileConfig = toml::from_str("# just a comment\n").unwrap();
        assert!(partial.temperature.is_none());
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
