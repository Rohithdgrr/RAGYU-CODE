use anyhow::{Context, Result};
use std::borrow::Cow;
use std::sync::Arc;
use zeroize::Zeroizing;

/// Default context budget for token-aware trimming (`context_tokens` in TOML).
/// Raised from 8192 → 128_000 so the CLI never under-utilizes large-context
/// models (Gemini 1M, Claude 200k, OpenCode-routed 200k+, etc.) when no
/// explicit `context_tokens` is set in `config.toml`. Users can still set
/// a smaller value in TOML to force aggressive history trimming.
pub const DEFAULT_CONTEXT_TOKENS: usize = 128_000;
const MIN_CONTEXT_TOKENS: usize = 256;
const MAX_CONTEXT_TOKENS: usize = 1_000_000;

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
///
/// `Bearer` holds a `Zeroizing<String>` so the key is wiped from memory on
/// drop. `Debug` is redacted to prevent accidental logging of secrets; use
/// `Auth::token()` to borrow the key only when building a request. Never
/// log or serialize the key.
#[derive(Clone, PartialEq)]
pub enum Auth {
    /// Local servers (Ollama, LM Studio…) need nothing.
    None,
    Bearer(Zeroizing<String>),
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Auth::None => write!(f, "None"),
            Auth::Bearer(_) => write!(f, "Bearer([REDACTED])"),
        }
    }
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
    /// Static family id (`mistral`, `ollama`, `opencode`…). Dynamic
    /// backends that need a per-instance identity override [`Provider::key`]
    /// instead of widening this signature.
    fn id(&self) -> &'static str;
    /// Stable unique key used for config persistence and display.
    /// Defaults to `id()`; dynamic providers return e.g. `opencode-<pid>`.
    fn key(&self) -> Cow<'static, str> {
        Cow::Borrowed(self.id())
    }
    fn chat_url(&self) -> String;
    /// The configured API root (for display and config persistence).
    fn base_url(&self) -> &str;
    /// `None` for backends without a model-listing endpoint.
    fn models_url(&self) -> Option<String>;
    fn auth(&self) -> Auth;
}

/// The role a model plays in the routing pipeline. Combo gateways
/// (OmniRoute) tag every model they expose; non-combo providers can
/// optionally declare a role so the router can pick fallbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterRole {
    Primary,
    Coding,
    Fast,
    Cheap,
    Smart,
    Offline,
    Generic,
}

pub struct Preset {
    pub id: &'static str,
    pub base_url: &'static str,
    pub api_key_env: Option<&'static str>,
    /// Optional role tag. `None` means "no router role"; the provider is
    /// usable as the active model but contributes no fallbacks.
    pub role: Option<RouterRole>,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "omniroute",
        // Local AI gateway (`npm i -g omniroute`); keyless `auto` model
        // works on a fresh install via pre-wired free providers.
        base_url: "http://localhost:20128/v1",
        api_key_env: None,
        role: Some(RouterRole::Smart),
    },
    Preset {
        id: "mistral",
        base_url: "https://api.mistral.ai/v1",
        api_key_env: Some("MISTRAL_API_KEY"),
        role: None,
    },
    Preset {
        id: "openai",
        base_url: "https://api.openai.com/v1",
        api_key_env: Some("OPENAI_API_KEY"),
        role: None,
    },
    Preset {
        id: "openrouter",
        base_url: "https://openrouter.ai/api/v1",
        api_key_env: Some("OPENROUTER_API_KEY"),
        role: None,
    },
    Preset {
        id: "nvidia",
        base_url: "https://integrate.api.nvidia.com/v1",
        api_key_env: Some("NVIDIA_API_KEY"),
        role: None,
    },
    Preset {
        id: "deepseek",
        base_url: "https://api.deepseek.com/v1",
        api_key_env: Some("DEEPSEEK_API_KEY"),
        role: None,
    },
    Preset {
        id: "kimi",
        base_url: "https://api.moonshot.cn/v1",
        api_key_env: Some("KIMI_API_KEY"),
        role: None,
    },
    Preset {
        id: "glm",
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        api_key_env: Some("GLM_API_KEY"),
        role: None,
    },
    Preset {
        id: "minimax",
        base_url: "https://api.minimax.chat/v1",
        api_key_env: Some("MINIMAX_API_KEY"),
        role: None,
    },
    Preset {
        id: "groq",
        base_url: "https://api.groq.com/openai/v1",
        api_key_env: Some("GROQ_API_KEY"),
        role: None,
    },
    Preset {
        id: "bytez",
        base_url: "https://api.bytez.com/v1",
        api_key_env: Some("BYTEZ_API_KEY"),
        role: None,
    },
    Preset {
        id: "gemini",
        base_url: "https://generativelanguage.googleapis.com/v1beta",
        api_key_env: Some("GEMINI_API_KEY"),
        role: None,
    },
    Preset {
        id: "ollama",
        base_url: "http://localhost:11434/v1",
        api_key_env: None,
        role: Some(RouterRole::Offline),
    },
];

pub fn preset_names() -> impl Iterator<Item = &'static str> {
    PRESETS.iter().map(|p| p.id)
}

/// A typed row of the OmniRoute combo table. Combos are the gateway's
/// pre-bundled routing strategies (`auto`, `/smart`, `/coding`, …).
/// `context_window` is the combo's advertised limit; real upstreams may
/// differ and the agent loop will fall back to the registry value when
/// the live `/v1/models` answer disagrees.
pub struct OmniRouteCombo {
    pub id: &'static str,
    pub role: RouterRole,
    pub free: bool,
    pub description: &'static str,
    pub context_window: usize,
}

pub const OMNIROUTE_COMBOS: &[OmniRouteCombo] = &[
    OmniRouteCombo {
        id: "auto",
        role: RouterRole::Smart,
        free: true,
        description: "smart router across all connected providers",
        context_window: 1_048_576,
    },
    OmniRouteCombo {
        id: "/smart",
        role: RouterRole::Smart,
        free: true,
        description: "quality-optimized combo",
        context_window: 1_048_576,
    },
    OmniRouteCombo {
        id: "/coding",
        role: RouterRole::Coding,
        free: true,
        description: "coding-optimized combo",
        context_window: 1_048_576,
    },
    OmniRouteCombo {
        id: "/fast",
        role: RouterRole::Fast,
        free: true,
        description: "speed-optimized combo",
        context_window: 1_048_576,
    },
    OmniRouteCombo {
        id: "/cheap",
        role: RouterRole::Cheap,
        free: true,
        description: "cost-optimized combo",
        context_window: 1_048_576,
    },
    OmniRouteCombo {
        id: "/offline",
        role: RouterRole::Offline,
        free: true,
        description: "local-only combo",
        context_window: 32_768,
    },
];

/// Returns the OmniRoute combo row for an id, or `None` if `id` is not
/// a known combo. Strips a leading `<vendor>/` prefix so callers can
/// pass either `auto` or `oc/auto` interchangeably. Leading `/` on
/// combo ids (e.g. `/smart`) is normalized so `"/smart"` and `"smart"`
/// both resolve.
pub fn omniroute_combo(id: &str) -> Option<&'static OmniRouteCombo> {
    let bare = id.rsplit_once('/').map(|(_, tail)| tail).unwrap_or(id);
    let bare_norm = bare.trim_start_matches('/');
    OMNIROUTE_COMBOS
        .iter()
        .find(|c| c.id.trim_start_matches('/') == bare_norm || c.id == id || c.id == bare)
}

/// A known model with metadata. `context_window` is the model's actual
/// input-token limit (from the provider's `/v1/models` or documented specs).
/// When 0, no metadata is known and the CLI falls back to
/// [`DEFAULT_CONTEXT_TOKENS`].
pub struct KnownModel {
    pub id: &'static str,
    pub free: bool,
    pub description: &'static str,
    /// Model's true input context window in tokens. `0` = unknown.
    pub context_window: usize,
}

/// Looks up the context window for a specific model on a given provider.
/// Returns 0 when no metadata is available (caller should then use
/// [`DEFAULT_CONTEXT_TOKENS`]).
///
/// Strips a leading `<vendor>/` prefix from the model name so callers can
/// pass either `hy3-free` or `oc/hy3-free` interchangeably.
/// OmniRoute combos are resolved via `OMNIROUTE_COMBOS` first so the
/// combo table is the single source of truth for their windows.
pub fn context_window_for(provider: &str, model: &str) -> usize {
    let bare = model
        .rsplit_once('/')
        .map(|(_, tail)| tail)
        .unwrap_or(model);
    if provider == "omniroute" {
        if let Some(c) = omniroute_combo(model).or_else(|| omniroute_combo(bare)) {
            return c.context_window;
        }
    }
    for km in known_models(provider) {
        if km.id == model || km.id == bare {
            return km.context_window;
        }
    }
    0
}

/// Recommended `max_tokens` (output limit) for a model. Returns 0 when no
/// tailored value is known — caller should fall back to a safe default.
/// Values are conservative mid-range defaults per provider family; they
/// prevent the silent `finish_reason: length` truncation that otherwise
/// makes the model appear to "stop" mid-answer.
pub fn max_output_for(provider: &str, model: &str) -> usize {
    let bare = model
        .rsplit_once('/')
        .map(|(_, tail)| tail)
        .unwrap_or(model);
    let lower = bare.to_ascii_lowercase();
    // Reasoning / long-output families get a higher cap.
    if lower.contains("o1-")
        || lower.contains("o1_")
        || lower == "o1"
        || lower.contains("reasoner")
        || lower.contains("deepseek-r1")
        || lower.contains("r1")
    {
        return 16384;
    }
    if lower.contains("gemini") {
        return 8192;
    }
    if lower.contains("claude") {
        return 8192;
    }
    // Provider-scoped known limits for small-context models.
    let ctx = context_window_for(provider, model);
    if ctx > 0 && ctx <= 8192 {
        return 2048.min(ctx / 2).max(1024);
    }
    if ctx > 0 && ctx <= 32768 {
        return 4096;
    }
    // Default: 4096 is safe for every OpenAI-compatible API; large-context
    // models (128k+, 1M) still need an explicit cap or the server defaults
    // to 4k and truncates longer answers with `finish_reason: length`.
    8192
}

/// Returns known models for a provider. Used as fallback when the API
/// model-listing endpoint is unavailable, and to show free-tier tags.
pub fn known_models(provider: &str) -> &'static [KnownModel] {
    match provider {
        "omniroute" => &[
            KnownModel {
                id: "auto",
                free: true,
                description: "smart router across all connected providers",
                context_window: 1_048_576,
            },
            KnownModel {
                id: "/coding",
                free: true,
                description: "coding-optimized combo",
                context_window: 1_048_576,
            },
            KnownModel {
                id: "/fast",
                free: true,
                description: "speed-optimized combo",
                context_window: 1_048_576,
            },
            KnownModel {
                id: "/cheap",
                free: true,
                description: "cost-optimized combo",
                context_window: 1_048_576,
            },
            KnownModel {
                id: "/offline",
                free: true,
                description: "local-only combo",
                context_window: 32_768,
            },
            KnownModel {
                id: "/smart",
                free: true,
                description: "quality-optimized combo",
                context_window: 1_048_576,
            },
        ],
        "mistral" => &[
            KnownModel {
                id: "mistral-small-latest",
                free: true,
                description: "fast general (free tier)",
                context_window: 32_000,
            },
            KnownModel {
                id: "mistral-medium-latest",
                free: false,
                description: "balanced quality",
                context_window: 128_000,
            },
            KnownModel {
                id: "mistral-large-latest",
                free: false,
                description: "highest quality",
                context_window: 128_000,
            },
            KnownModel {
                id: "codestral-latest",
                free: true,
                description: "code specialist (free tier)",
                context_window: 32_000,
            },
            KnownModel {
                id: "open-mistral-nemo",
                free: true,
                description: "open model (free tier)",
                context_window: 128_000,
            },
            KnownModel {
                id: "open-codestral-mamba",
                free: true,
                description: "code Mamba (free tier)",
                context_window: 256_000,
            },
            KnownModel {
                id: "mistral-tiny-latest",
                free: true,
                description: "tiny/fast (free tier)",
                context_window: 32_000,
            },
        ],
        "openai" => &[
            KnownModel {
                id: "gpt-4o-mini",
                free: false,
                description: "fast & cheap",
                context_window: 128_000,
            },
            KnownModel {
                id: "gpt-4o",
                free: false,
                description: "flagship model",
                context_window: 128_000,
            },
            KnownModel {
                id: "gpt-3.5-turbo",
                free: false,
                description: "legacy fast",
                context_window: 16_385,
            },
            KnownModel {
                id: "o1-mini",
                free: false,
                description: "reasoning (small)",
                context_window: 128_000,
            },
            KnownModel {
                id: "o1-preview",
                free: false,
                description: "reasoning (full)",
                context_window: 128_000,
            },
        ],
        "openrouter" => &[
            KnownModel {
                id: "mistralai/mistral-7b-instruct:free",
                free: true,
                description: "Mistral 7B (free)",
                context_window: 32_000,
            },
            KnownModel {
                id: "meta-llama/llama-3.1-8b-instruct:free",
                free: true,
                description: "Llama 3.1 8B (free)",
                context_window: 131_000,
            },
            KnownModel {
                id: "meta-llama/llama-3.1-70b-instruct:free",
                free: true,
                description: "Llama 3.1 70B (free)",
                context_window: 131_000,
            },
            KnownModel {
                id: "google/gemma-2-9b-it:free",
                free: true,
                description: "Gemma 2 9B (free)",
                context_window: 8_192,
            },
            KnownModel {
                id: "qwen/qwen-2-7b-instruct:free",
                free: true,
                description: "Qwen 2 7B (free)",
                context_window: 32_000,
            },
            KnownModel {
                id: "huggingfaceh4/zephyr-7b-beta:free",
                free: true,
                description: "Zephyr 7B (free)",
                context_window: 32_000,
            },
            KnownModel {
                id: "openchat/openchat-7b:free",
                free: true,
                description: "OpenChat 7B (free)",
                context_window: 8_192,
            },
            KnownModel {
                id: "gryphe/mythomist-7b:free",
                free: true,
                description: "Mythomist 7B (free)",
                context_window: 32_000,
            },
            KnownModel {
                id: "nousresearch/hermes-3-llama-3.1-405b:free",
                free: true,
                description: "Hermes 3 405B (free)",
                context_window: 131_000,
            },
            KnownModel {
                id: "mistralai/mixtral-8x7b-instruct",
                free: false,
                description: "Mixtral 8x7B",
                context_window: 32_000,
            },
            KnownModel {
                id: "meta-llama/llama-3.1-405b-instruct",
                free: false,
                description: "Llama 3.1 405B",
                context_window: 131_000,
            },
            KnownModel {
                id: "meta-llama/llama-3.1-70b-instruct",
                free: false,
                description: "Llama 3.1 70B",
                context_window: 131_000,
            },
            KnownModel {
                id: "anthropic/claude-3.5-sonnet",
                free: false,
                description: "Claude 3.5 Sonnet",
                context_window: 200_000,
            },
            KnownModel {
                id: "anthropic/claude-3.5-sonnet:beta",
                free: false,
                description: "Claude 3.5 Sonnet (beta)",
                context_window: 200_000,
            },
            KnownModel {
                id: "openai/gpt-4o-mini",
                free: false,
                description: "GPT-4o Mini",
                context_window: 128_000,
            },
            KnownModel {
                id: "openai/gpt-4o",
                free: false,
                description: "GPT-4o",
                context_window: 128_000,
            },
            KnownModel {
                id: "google/gemini-pro-1.5",
                free: false,
                description: "Gemini Pro 1.5",
                context_window: 1_000_000,
            },
            KnownModel {
                id: "deepseek/deepseek-chat",
                free: false,
                description: "DeepSeek Chat",
                context_window: 64_000,
            },
        ],
        "kimi" => &[
            KnownModel {
                id: "moonshot-v1-8k",
                free: false,
                description: "8k context",
                context_window: 8_192,
            },
            KnownModel {
                id: "moonshot-v1-32k",
                free: false,
                description: "32k context",
                context_window: 32_000,
            },
            KnownModel {
                id: "moonshot-v1-128k",
                free: false,
                description: "128k context",
                context_window: 128_000,
            },
        ],
        "groq" => &[
            KnownModel {
                id: "llama-3.3-70b-versatile",
                free: true,
                description: "Llama 3.3 70B (free tier)",
                context_window: 131_000,
            },
            KnownModel {
                id: "llama-3.1-8b-instant",
                free: true,
                description: "Llama 3.1 8B fast (free tier)",
                context_window: 131_000,
            },
            KnownModel {
                id: "llama3-70b-8192",
                free: true,
                description: "Llama 3 70B (free tier)",
                context_window: 8_192,
            },
            KnownModel {
                id: "llama3-8b-8192",
                free: true,
                description: "Llama 3 8B (free tier)",
                context_window: 8_192,
            },
            KnownModel {
                id: "gemma2-9b-it",
                free: true,
                description: "Gemma 2 9B (free tier)",
                context_window: 8_192,
            },
            KnownModel {
                id: "mixtral-8x7b-32768",
                free: true,
                description: "Mixtral 8x7B (free tier)",
                context_window: 32_768,
            },
            KnownModel {
                id: "gemma-7b-it",
                free: true,
                description: "Gemma 7B (free tier)",
                context_window: 8_192,
            },
        ],
        "ollama" => &[
            KnownModel {
                id: "llama3.1",
                free: true,
                description: "Llama 3.1 (local)",
                context_window: 128_000,
            },
            KnownModel {
                id: "llama3.2",
                free: true,
                description: "Llama 3.2 (local)",
                context_window: 128_000,
            },
            KnownModel {
                id: "codellama",
                free: true,
                description: "Code Llama (local)",
                context_window: 16_000,
            },
            KnownModel {
                id: "mistral",
                free: true,
                description: "Mistral (local)",
                context_window: 32_000,
            },
            KnownModel {
                id: "mixtral",
                free: true,
                description: "Mixtral (local)",
                context_window: 32_000,
            },
            KnownModel {
                id: "phi3",
                free: true,
                description: "Phi-3 (local)",
                context_window: 128_000,
            },
            KnownModel {
                id: "gemma2",
                free: true,
                description: "Gemma 2 (local)",
                context_window: 8_192,
            },
            KnownModel {
                id: "qwen2.5",
                free: true,
                description: "Qwen 2.5 (local)",
                context_window: 128_000,
            },
            KnownModel {
                id: "deepseek-coder-v2",
                free: true,
                description: "DeepSeek Coder V2 (local)",
                context_window: 128_000,
            },
            KnownModel {
                id: "command-r",
                free: true,
                description: "Command R (local)",
                context_window: 128_000,
            },
            KnownModel {
                id: "vicuna",
                free: true,
                description: "Vicuna (local)",
                context_window: 4_096,
            },
            KnownModel {
                id: "neural-chat",
                free: true,
                description: "Neural Chat (local)",
                context_window: 4_096,
            },
        ],
        "deepseek" => &[
            KnownModel {
                id: "deepseek-chat",
                free: true,
                description: "DeepSeek Chat (free tier)",
                context_window: 64_000,
            },
            KnownModel {
                id: "deepseek-reasoner",
                free: true,
                description: "DeepSeek Reasoner/R1 (free tier)",
                context_window: 64_000,
            },
            KnownModel {
                id: "deepseek-coder",
                free: true,
                description: "DeepSeek Coder (free tier)",
                context_window: 64_000,
            },
        ],
        "nvidia" => &[
            KnownModel {
                id: "nvidia/llama-3.1-nemotron-70b-instruct",
                free: true,
                description: "Nemotron 70B (free tier)",
                context_window: 131_000,
            },
            KnownModel {
                id: "meta/llama-3.1-405b-instruct",
                free: true,
                description: "Llama 3.1 405B (free tier)",
                context_window: 131_000,
            },
            KnownModel {
                id: "meta/llama-3.1-70b-instruct",
                free: true,
                description: "Llama 3.1 70B (free tier)",
                context_window: 131_000,
            },
            KnownModel {
                id: "meta/llama-3.1-8b-instruct",
                free: true,
                description: "Llama 3.1 8B (free tier)",
                context_window: 131_000,
            },
            KnownModel {
                id: "mistralai/mistral-large-instruct",
                free: true,
                description: "Mistral Large (free tier)",
                context_window: 32_000,
            },
            KnownModel {
                id: "mistralai/mixtral-8x7b-instruct",
                free: true,
                description: "Mixtral 8x7B (free tier)",
                context_window: 32_000,
            },
            KnownModel {
                id: "google/gemma-2-27b-it",
                free: true,
                description: "Gemma 2 27B (free tier)",
                context_window: 8_192,
            },
            KnownModel {
                id: "google/gemma-2-9b-it",
                free: true,
                description: "Gemma 2 9B (free tier)",
                context_window: 8_192,
            },
            KnownModel {
                id: "ai21/jamba-1-5-large",
                free: true,
                description: "Jamba 1.5 Large (free tier)",
                context_window: 256_000,
            },
            KnownModel {
                id: "snowflake/snowflake-arctic-instruct",
                free: true,
                description: "Arctic Instruct (free tier)",
                context_window: 4_096,
            },
        ],
        "bytez" => &[
            KnownModel {
                id: "bytez-auto",
                free: true,
                description: "auto-select (free tier)",
                context_window: 0,
            },
            KnownModel {
                id: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
                free: true,
                description: "Llama 3.3 70B Turbo (free tier)",
                context_window: 131_000,
            },
            KnownModel {
                id: "meta-llama/Llama-3.1-70B-Instruct",
                free: true,
                description: "Llama 3.1 70B (free tier)",
                context_window: 131_000,
            },
            KnownModel {
                id: "meta-llama/Llama-3.1-8B-Instruct",
                free: true,
                description: "Llama 3.1 8B (free tier)",
                context_window: 131_000,
            },
            KnownModel {
                id: "mistralai/Mistral-7B-Instruct-v0.3",
                free: true,
                description: "Mistral 7B v0.3 (free tier)",
                context_window: 32_000,
            },
            KnownModel {
                id: "google/gemma-2-9b-it",
                free: true,
                description: "Gemma 2 9B (free tier)",
                context_window: 8_192,
            },
        ],
        "gemini" => &[
            KnownModel {
                id: "gemini-2.0-flash-exp",
                free: true,
                description: "Gemini 2.0 Flash Experimental (free tier)",
                context_window: 1_000_000,
            },
            KnownModel {
                id: "gemini-1.5-flash",
                free: true,
                description: "Gemini 1.5 Flash (free tier)",
                context_window: 1_000_000,
            },
            KnownModel {
                id: "gemini-1.5-flash-8b",
                free: true,
                description: "Gemini 1.5 Flash 8B (free tier)",
                context_window: 1_000_000,
            },
            KnownModel {
                id: "gemini-1.5-pro",
                free: true,
                description: "Gemini 1.5 Pro (free tier)",
                context_window: 1_000_000,
            },
            KnownModel {
                id: "gemini-1.0-pro",
                free: true,
                description: "Gemini 1.0 Pro (free tier)",
                context_window: 32_000,
            },
        ],
        "glm" => &[
            KnownModel {
                id: "glm-4-flash",
                free: true,
                description: "GLM-4 Flash (free tier)",
                context_window: 128_000,
            },
            KnownModel {
                id: "glm-4-air",
                free: false,
                description: "GLM-4 Air",
                context_window: 128_000,
            },
            KnownModel {
                id: "glm-4-airx",
                free: false,
                description: "GLM-4 AirX",
                context_window: 8_192,
            },
            KnownModel {
                id: "glm-4-long",
                free: false,
                description: "GLM-4 Long (128k)",
                context_window: 1_000_000,
            },
            KnownModel {
                id: "glm-4-plus",
                free: false,
                description: "GLM-4 Plus",
                context_window: 128_000,
            },
            KnownModel {
                id: "codegeex-4",
                free: true,
                description: "CodeGeeX 4 (free tier)",
                context_window: 128_000,
            },
        ],
        "minimax" => &[
            KnownModel {
                id: "MiniMax-Text-01",
                free: false,
                description: "MiniMax Text 01",
                context_window: 1_000_000,
            },
            KnownModel {
                id: "abab6.5s-chat",
                free: false,
                description: "Abab 6.5S Chat",
                context_window: 32_000,
            },
            KnownModel {
                id: "abab5.5-chat",
                free: false,
                description: "Abab 5.5 Chat",
                context_window: 16_000,
            },
        ],
        // OpenCode-routed models. Context windows match the
        // `oc/*` entries returned by OmniRoute's `/v1/models` (and OpenCode's
        // own Zen service). All 200k+ for the "h3" family / "hy3-free" /
        // big-pickle / nemotron / north / gemma4 31B; 1M for the free
        // mimo-v2.5 and deepseek-v4-flash-free variants.
        "opencode" => &[
            KnownModel {
                id: "big-pickle",
                free: true,
                description: "Big Pickle (200k)",
                context_window: 200_000,
            },
            KnownModel {
                id: "hy3-free",
                free: true,
                description: "hy3-free (200k)",
                context_window: 200_000,
            },
            KnownModel {
                id: "deepseek-v4-flash-free",
                free: true,
                description: "DeepSeek V4 Flash Free (1M)",
                context_window: 1_000_000,
            },
            KnownModel {
                id: "mimo-v2.5-free",
                free: true,
                description: "mimo-v2.5-free (1M)",
                context_window: 1_048_576,
            },
            KnownModel {
                id: "nemotron-3-ultra-free",
                free: true,
                description: "nemotron-3-ultra-free (200k)",
                context_window: 200_000,
            },
            KnownModel {
                id: "north-mini-code-free",
                free: true,
                description: "north-mini-code-free (200k)",
                context_window: 200_000,
            },
        ],
        _ => &[],
    }
}

/// A resolved provider: preset (or custom `base_url`) + optional key.
///
/// `key` is a `Zeroizing<String>` so the API key is wiped on drop. This
/// struct never implements `Debug` to avoid accidental logging of the key.
#[derive(Clone)]
struct ResolvedProvider {
    id: &'static str,
    base_url: String,
    /// API key, zeroized on drop. Never log or serialize.
    key: Option<Zeroizing<String>>,
}

impl Provider for ResolvedProvider {
    fn id(&self) -> &'static str {
        self.id
    }

    fn chat_url(&self) -> String {
        format!("{}/chat/completions", trim_base(&self.base_url))
    }

    fn base_url(&self) -> &str {
        &self.base_url
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

/// Live-fetch the `context_length` (or `max_input_tokens`) of a model from
/// the provider's `/v1/models` endpoint. Returns the detected window, or
/// `0` if the request fails / the model is not listed.
///
/// This is the authoritative source: the static `known_models()` table is a
/// fallback for offline / keyless scenarios. The OpenAI-compatible spec
/// allows each entry to carry `context_length`, `max_input_tokens`, or
/// `max_tokens`; we accept any of them.
pub async fn fetch_model_context(
    http: &reqwest::Client,
    provider: &dyn Provider,
    model: &str,
) -> usize {
    let Some(url) = provider.models_url() else {
        return 0;
    };
    // Use Zeroizing for the cloned token so the temporary copy is wiped on drop.
    // The caller’s Auth already holds a Zeroizing<String>; we clone via
    // Zeroizing to avoid leaving a plain String on the heap.
    let token: Option<Zeroizing<String>> = match provider.auth() {
        Auth::Bearer(t) => Some(Zeroizing::new(t.to_string())),
        Auth::None => None,
    };
    let mut req = http.get(&url);
    if let Some(tok) = token.as_ref() {
        req = req.bearer_auth(tok.as_str());
    }
    let resp = match tokio::time::timeout(std::time::Duration::from_secs(3), req.send()).await {
        Ok(Ok(r)) => r,
        _ => return 0,
    };
    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return 0,
    };
    let entries = body.get("data").and_then(|d| d.as_array());
    let Some(arr) = entries else { return 0 };
    let bare = model
        .rsplit_once('/')
        .map(|(_, tail)| tail)
        .unwrap_or(model);
    for entry in arr {
        let id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if id == model || id == bare {
            // OpenAI-compat spec: prefer `context_length`, then
            // `max_input_tokens`, then `max_tokens`. Some providers
            // (e.g. Ollama) use `context_length` only.
            let n = entry
                .get("context_length")
                .and_then(|v| v.as_u64())
                .or_else(|| entry.get("max_input_tokens").and_then(|v| v.as_u64()))
                .or_else(|| entry.get("max_tokens").and_then(|v| v.as_u64()))
                .unwrap_or(0);
            if n > 0 {
                return n as usize;
            }
        }
    }
    0
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

    #[test]
    fn context_window_lookup_returns_registered_value() {
        assert_eq!(context_window_for("opencode", "hy3-free"), 200_000);
        assert_eq!(context_window_for("opencode", "mimo-v2.5-free"), 1_048_576);
        assert_eq!(context_window_for("opencode", "oc/hy3-free"), 200_000);
        assert_eq!(context_window_for("openai", "gpt-4o"), 128_000);
        assert_eq!(context_window_for("gemini", "gemini-1.5-pro"), 1_000_000);
        assert_eq!(context_window_for("groq", "llama3-8b-8192"), 8_192);
        assert_eq!(context_window_for("ollama", "llama3.1"), 128_000);
        assert_eq!(context_window_for("nope", "whatever"), 0);
    }

    #[test]
    fn context_window_for_handles_slash_prefixed_model() {
        // Users type "oc/hy3-free" but the registry stores "hy3-free".
        // The lookup must still resolve it.
        assert_eq!(context_window_for("opencode", "oc/hy3-free"), 200_000);
    }
}
