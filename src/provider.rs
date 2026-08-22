use anyhow::{Context, Result};
use std::sync::Arc;
use zeroize::Zeroizing;

/// Default context budget for token-aware trimming (`context_tokens` in TOML).
pub const DEFAULT_CONTEXT_TOKENS: usize = 8192;
const MIN_CONTEXT_TOKENS: usize = 256;
const MAX_CONTEXT_TOKENS: usize = 200_000;

/// Clamps a user-supplied context budget into the sane range.
pub fn clamp_context_tokens(raw: Option<usize>) -> usize {
    match raw {
        Some(t) if (MIN_CONTEXT_TOKENS..=MAX_CONTEXT_TOKENS).contains(&t) => t,
        Some(t) => {
            eprintln!(
                "warning: context_tokens={t} out of range {MIN_CONTEXT_TOKENS}-{MAX_CONTEXT_TOKENS}; using {DEFAULT_CONTEXT_TOKENS}"
            );
            DEFAULT_CONTEXT_TOKENS
        }
        None => DEFAULT_CONTEXT_TOKENS,
    }
}

/// How a provider authenticates requests.
#[derive(Clone, Debug, PartialEq)]
pub enum Auth {
    /// Local servers (Ollama, LM Studio…) need nothing.
    None,
    Bearer(Zeroizing<String>),
}

impl Auth {
    pub fn token(&self) -> Option<&str> {
        match self {
            Auth::None => None,
            Auth::Bearer(t) => Some(t.as_str()),
        }
    }
}

/// Everything `api.rs` needs to know to talk to a backend.
///
/// All OpenAI-compatible servers share one wire format (SSE with
/// `choices[0].delta.content` and a `[DONE]` sentinel), so a single
/// streaming implementation serves Mistral, OpenAI, Ollama, LM Studio,
/// vLLM and friends; only endpoints and auth differ.
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;
    fn chat_url(&self) -> String;
    /// `None` for backends without a model-listing endpoint.
    fn models_url(&self) -> Option<String>;
    fn auth(&self) -> Auth;
}

struct Preset {
    id: &'static str,
    base_url: &'static str,
    api_key_env: Option<&'static str>,
}

const PRESETS: &[Preset] = &[
    Preset {
        id: "mistral",
        base_url: "https://api.mistral.ai/v1",
        api_key_env: Some("MISTRAL_API_KEY"),
    },
    Preset {
        id: "openai",
        base_url: "https://api.openai.com/v1",
        api_key_env: Some("OPENAI_API_KEY"),
    },
    Preset {
        id: "groq",
        base_url: "https://api.groq.com/openai/v1",
        api_key_env: Some("GROQ_API_KEY"),
    },
    Preset {
        id: "ollama",
        base_url: "http://localhost:11434/v1",
        api_key_env: None,
    },
];

pub fn preset_names() -> impl Iterator<Item = &'static str> {
    PRESETS.iter().map(|p| p.id)
}

/// A resolved provider: preset (or custom `base_url`) + optional key.
#[derive(Clone)]
struct ResolvedProvider {
    id: &'static str,
    base_url: String,
    key: Option<Zeroizing<String>>,
}

impl Provider for ResolvedProvider {
    fn id(&self) -> &'static str {
        self.id
    }

    fn chat_url(&self) -> String {
        format!("{}/chat/completions", trim_base(&self.base_url))
    }

    fn models_url(&self) -> Option<String> {
        // A bare localhost URL without /v1 usually means an OpenAI-compatible
        // server was configured by hand; still try the standard path.
        Some(format!("{}/models", trim_base(&self.base_url)))
    }

    fn auth(&self) -> Auth {
        match &self.key {
            Some(k) => Auth::Bearer(k.clone()),
            None => Auth::None,
        }
    }
}

fn trim_base(base: &str) -> &str {
    base.trim_end_matches('/')
}

/// Builds the provider from layered config:
///   preset defaults < `base_url`/`api_key_env` overrides < environment key.
///
/// `api_key_from_env` is the already-loaded env value (MISTRAL_API_KEY-style),
/// kept separate so `.env` handling stays in config.rs.
pub fn resolve(
    name: &str,
    base_url_override: Option<&str>,
    api_key_env_override: Option<&str>,
    env_lookup: impl Fn(&str) -> Option<String>,
) -> Result<Arc<dyn Provider>> {
    let preset = PRESETS.iter().find(|p| p.id == name);
    let base_url = base_url_override
        .map(str::to_owned)
        .or_else(|| preset.map(|p| p.base_url.to_owned()))
        .with_context(|| {
            format!(
                "unknown provider '{name}' — known: {} (or set base_url for a custom OpenAI-compatible server)",
                preset_names().collect::<Vec<_>>().join(", ")
            )
        })?;

    let env_name = api_key_env_override.or(preset.and_then(|p| p.api_key_env));
    let key = match env_name {
        Some(var) => env_lookup(var).and_then(|k| {
            let k = k.trim().to_owned();
            (!k.is_empty()).then(|| Zeroizing::new(k))
        }),
        None => None,
    };

    if key.is_none() && preset.is_some_and(|p| p.api_key_env.is_some()) {
        anyhow::bail!(
            "{} is not set.\nAdd it to a .env file in this directory, or export it, then restart.",
            env_name.unwrap_or("the API key variable")
        );
    }

    Ok(Arc::new(ResolvedProvider {
        id: preset.map_or("custom", |p| p.id),
        base_url,
        key,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(var: &str) -> Option<String> {
        match var {
            "TEST_KEY" | "MISTRAL_API_KEY" => Some("secret".to_owned()),
            _ => None,
        }
    }

    #[test]
    fn mistral_preset_resolves_with_key() {
        let p = resolve("mistral", None, None, lookup).expect("mistral resolves");
        assert_eq!(p.id(), "mistral");
        assert_eq!(p.chat_url(), "https://api.mistral.ai/v1/chat/completions");
        assert_eq!(
            p.models_url().as_deref(),
            Some("https://api.mistral.ai/v1/models")
        );
        assert_eq!(p.auth().token(), Some("secret"));
    }

    #[test]
    fn ollama_needs_no_key() {
        let p = resolve("ollama", None, None, |_| None).expect("ollama resolves");
        assert_eq!(p.chat_url(), "http://localhost:11434/v1/chat/completions");
        assert_eq!(p.auth(), Auth::None);
    }

    #[test]
    fn missing_key_for_cloud_provider_is_an_error_naming_the_var() {
        let err = match resolve("openai", None, None, |_| None) {
            Err(e) => e,
            Ok(_) => panic!("openai without a key must not resolve"),
        };
        assert!(err.to_string().contains("OPENAI_API_KEY"), "{err}");
    }

    #[test]
    fn overrides_retarget_any_preset() {
        let p = resolve(
            "ollama",
            Some("http://192.168.1.10:8080/v0/"),
            Some("TEST_KEY"),
            lookup,
        )
        .expect("resolves");
        assert_eq!(p.chat_url(), "http://192.168.1.10:8080/v0/chat/completions");
        assert_eq!(p.auth().token(), Some("secret"));
    }

    #[test]
    fn unknown_name_without_base_url_fails() {
        assert!(resolve("wat", None, None, lookup).is_err());
    }
}
