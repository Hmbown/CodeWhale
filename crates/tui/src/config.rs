//! Configuration loading and defaults for codewhale.

use std::collections::HashMap;
use std::fmt::Write;
use std::fs;
#[cfg(unix)]
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use codewhale_execpolicy::{ExecPolicyEngine, Ruleset, ToolPermissionRule};
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::audit::log_sensitive_event;
use crate::features::{Features, FeaturesToml, is_known_feature_key};
use crate::hooks::HooksConfig;

pub const DEFAULT_MAX_SUBAGENTS: usize = 10;
pub const MAX_SUBAGENTS: usize = 20;
/// Default per-step DeepSeek API timeout for sub-agent requests, in seconds.
/// Matches the legacy hardcoded value so existing configs keep their old
/// behavior when `[subagents] api_timeout_secs` is unset (#1806, #1808).
pub const DEFAULT_SUBAGENT_API_TIMEOUT_SECS: u64 = 120;
/// Minimum accepted `[subagents] api_timeout_secs`. Anything lower (including
/// `0`, which would otherwise produce an immediate timeout footgun) clamps
/// up to this value before the runtime sees it.
pub const MIN_SUBAGENT_API_TIMEOUT_SECS: u64 = 1;
/// Maximum accepted `[subagents] api_timeout_secs` (30 minutes). The cap
/// keeps a misconfigured per-step timeout from masking real model/network
/// hangs forever.
pub const MAX_SUBAGENT_API_TIMEOUT_SECS: u64 = 1800;
pub const DEFAULT_TEXT_MODEL: &str = "deepseek-v4-pro";
pub const DEFAULT_DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/beta";
pub const DEFAULT_NVIDIA_NIM_MODEL: &str = "deepseek-ai/deepseek-v4-pro";
pub const DEFAULT_NVIDIA_NIM_FLASH_MODEL: &str = "deepseek-ai/deepseek-v4-flash";
pub const DEFAULT_NVIDIA_NIM_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
pub const DEFAULT_OPENAI_MODEL: &str = "deepseek-v4-pro";
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_ATLASCLOUD_MODEL: &str = "deepseek-ai/deepseek-v4-flash";
pub const DEFAULT_ATLASCLOUD_BASE_URL: &str = "https://api.atlascloud.ai/v1";
pub const DEFAULT_WANJIE_ARK_MODEL: &str = "deepseek-reasoner";
pub const DEFAULT_VOLCENGINE_MODEL: &str = "DeepSeek-V4-Pro";
pub const DEFAULT_VOLCENGINE_FLASH_MODEL: &str = "DeepSeek-V4-Flash";
pub const DEFAULT_VOLCENGINE_BASE_URL: &str = "https://ark.cn-beijing.volces.com/api/coding/v3";
pub const DEFAULT_WANJIE_ARK_BASE_URL: &str = "https://maas-openapi.wanjiedata.com/api/v1";
pub const DEFAULT_OPENROUTER_MODEL: &str = "deepseek/deepseek-v4-pro";
pub const DEFAULT_OPENROUTER_FLASH_MODEL: &str = "deepseek/deepseek-v4-flash";
pub const DEFAULT_OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const DEFAULT_XIAOMI_MIMO_MODEL: &str = "mimo-v2.5-pro";
pub const DEFAULT_XIAOMI_MIMO_BASE_URL: &str = "https://api.xiaomimimo.com/v1";
pub const DEFAULT_NOVITA_MODEL: &str = "deepseek/deepseek-v4-pro";
pub const DEFAULT_NOVITA_FLASH_MODEL: &str = "deepseek/deepseek-v4-flash";
pub const DEFAULT_NOVITA_BASE_URL: &str = "https://api.novita.ai/v1";
pub const DEFAULT_FIREWORKS_MODEL: &str = "accounts/fireworks/models/deepseek-v4-pro";
pub const DEFAULT_FIREWORKS_BASE_URL: &str = "https://api.fireworks.ai/inference/v1";
pub const DEFAULT_SILICONFLOW_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub const DEFAULT_SILICONFLOW_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub const DEFAULT_SILICONFLOW_BASE_URL: &str = "https://api.siliconflow.com/v1";
pub const DEFAULT_MOONSHOT_MODEL: &str = "kimi-k2.6";
pub const DEFAULT_MOONSHOT_BASE_URL: &str = "https://api.moonshot.ai/v1";
pub const DEFAULT_KIMI_CODE_MODEL: &str = "kimi-for-coding";
pub const DEFAULT_KIMI_CODE_BASE_URL: &str = "https://api.kimi.com/coding/v1";
pub const DEFAULT_SGLANG_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub const DEFAULT_SGLANG_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub const DEFAULT_SGLANG_BASE_URL: &str = "http://localhost:30000/v1";
pub const DEFAULT_VLLM_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub const DEFAULT_VLLM_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub const DEFAULT_VLLM_BASE_URL: &str = "http://localhost:8000/v1";
pub const DEFAULT_OLLAMA_MODEL: &str = "deepseek-coder:1.3b";
pub const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";
/// Legacy `deepseek-cn` provider alias.
///
/// DeepSeek's official API host is the same worldwide. Keep this alias for
/// old configs, but route it through the normal beta-enabled DeepSeek default.
/// Legacy typo hostname `api.deepseeki.com` remains recognized in URL
/// heuristics for backward compatibility.
pub const DEFAULT_DEEPSEEKCN_BASE_URL: &str = DEFAULT_DEEPSEEK_BASE_URL;
const API_KEYRING_SENTINEL: &str = "__KEYRING__";
pub const COMMON_DEEPSEEK_MODELS: &[&str] = &[
    "deepseek-v4-pro",
    "deepseek-v4-flash",
    "deepseek-ai/deepseek-v4-pro",
    "deepseek-ai/deepseek-v4-flash",
    "deepseek/deepseek-v4-pro",
    "deepseek/deepseek-v4-flash",
];
pub const OFFICIAL_DEEPSEEK_MODELS: &[&str] = &["deepseek-v4-pro", "deepseek-v4-flash"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiProvider {
    Deepseek,
    DeepseekCN,
    NvidiaNim,
    Openai,
    Atlascloud,
    WanjieArk,
    Volcengine,
    Openrouter,
    XiaomiMimo,
    Novita,
    Fireworks,
    Siliconflow,
    Moonshot,
    Sglang,
    Vllm,
    Ollama,
}

impl ApiProvider {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "deepseek" | "deep-seek" => Some(Self::Deepseek),
            "deepseek-cn" | "deepseek_china" | "deepseekcn" | "deepseek-china" => {
                Some(Self::DeepseekCN)
            }
            "nvidia" | "nvidia-nim" | "nvidia_nim" | "nim" => Some(Self::NvidiaNim),
            "openai" | "open-ai" => Some(Self::Openai),
            "atlascloud" | "atlas-cloud" | "atlas_cloud" | "atlas" => Some(Self::Atlascloud),
            "wanjie" | "wanjie-ark" | "wanjie_ark" | "ark-wanjie" | "ark_wanjie" | "wanjieark"
            | "wanjie-maas" | "wanjie_maas" | "wanjiemaas" => Some(Self::WanjieArk),
            "volcengine" | "volcengine-ark" | "volcengine_ark" | "ark" | "volc-ark"
            | "volcengineark" => Some(Self::Volcengine),
            "openrouter" | "open_router" => Some(Self::Openrouter),
            "xiaomi-mimo" | "xiaomi_mimo" | "xiaomimimo" | "mimo" | "xiaomi" => {
                Some(Self::XiaomiMimo)
            }
            "novita" => Some(Self::Novita),
            "fireworks" | "fireworks-ai" => Some(Self::Fireworks),
            "siliconflow" | "silicon-flow" | "silicon_flow" => Some(Self::Siliconflow),
            "moonshot" | "moonshot-ai" | "kimi" | "kimi-k2" => Some(Self::Moonshot),
            "sglang" | "sg-lang" => Some(Self::Sglang),
            "vllm" | "v-llm" => Some(Self::Vllm),
            "ollama" | "ollama-local" => Some(Self::Ollama),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deepseek => "deepseek",
            Self::DeepseekCN => "deepseek-cn",
            Self::NvidiaNim => "nvidia-nim",
            Self::Openai => "openai",
            Self::Atlascloud => "atlascloud",
            Self::WanjieArk => "wanjie-ark",
            Self::Volcengine => "volcengine",
            Self::Openrouter => "openrouter",
            Self::XiaomiMimo => "xiaomi-mimo",
            Self::Novita => "novita",
            Self::Fireworks => "fireworks",
            Self::Siliconflow => "siliconflow",
            Self::Moonshot => "moonshot",
            Self::Sglang => "sglang",
            Self::Vllm => "vllm",
            Self::Ollama => "ollama",
        }
    }

    /// Human-friendly label for picker UIs / status chips.
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Deepseek => "DeepSeek",
            Self::DeepseekCN => "DeepSeek (legacy alias)",
            Self::NvidiaNim => "NVIDIA NIM",
            Self::Openai => "OpenAI-compatible",
            Self::Atlascloud => "AtlasCloud",
            Self::WanjieArk => "Wanjie Ark",
            Self::Volcengine => "Volcengine Ark",
            Self::Openrouter => "OpenRouter",
            Self::XiaomiMimo => "Xiaomi MiMo",
            Self::Novita => "Novita AI",
            Self::Fireworks => "Fireworks AI",
            Self::Siliconflow => "SiliconFlow",
            Self::Moonshot => "Moonshot/Kimi",
            Self::Sglang => "SGLang",
            Self::Vllm => "vLLM",
            Self::Ollama => "Ollama",
        }
    }

    /// All providers, in the order shown in the picker.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::Deepseek,
            Self::NvidiaNim,
            Self::Openai,
            Self::Atlascloud,
            Self::WanjieArk,
            Self::Volcengine,
            Self::Openrouter,
            Self::XiaomiMimo,
            Self::Novita,
            Self::Fireworks,
            Self::Siliconflow,
            Self::Moonshot,
            Self::Sglang,
            Self::Vllm,
            Self::Ollama,
        ]
    }
}

// ============================================================================
// Provider Capability Matrix
// ============================================================================

/// Known capabilities for a provider + resolved-model combination.
///
/// Returned by [`provider_capability`] to describe what a given provider
/// supports for the resolved model string.  All fields are derived from
/// static knowledge (release docs, API guides) rather than live API probes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ProviderCapability {
    /// Canonical provider identifier.
    pub provider: ApiProvider,
    /// Resolved model identifier that will be sent in the API payload.
    pub resolved_model: String,
    /// Context window in tokens (the maximum input the model can accept).
    pub context_window: u32,
    /// Official maximum output tokens for this combo.
    ///
    /// This is model metadata for diagnostics and CI policy. Normal turns use
    /// a separate, more conservative request cap in the engine.
    pub max_output: u32,
    /// Whether the provider+model supports thinking/reasoning mode.
    pub thinking_supported: bool,
    /// Whether the provider returns prompt-cache telemetry fields.
    pub cache_telemetry_supported: bool,
    /// Which request-payload dialect the provider uses.
    pub request_payload_mode: RequestPayloadMode,
    /// Deprecation metadata for compatibility aliases that are still accepted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias_deprecation: Option<ModelAliasDeprecation>,
}

pub const DEEPSEEK_ALIAS_RETIREMENT_DATE: &str = "2026-07-24";
pub const DEEPSEEK_ALIAS_RETIREMENT_UTC: &str = "2026-07-24T15:59:00Z";
pub const DEEPSEEK_ALIAS_REPLACEMENT: &str = "deepseek-v4-flash";

/// Upstream retirement metadata for a model alias that remains compatible.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ModelAliasDeprecation {
    pub alias: String,
    pub replacement: String,
    pub retirement_date: String,
    pub retirement_utc: String,
    pub notice: String,
}

/// Which request-payload dialect the provider speaks.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum RequestPayloadMode {
    /// Standard OpenAI-compatible `/v1/chat/completions` payload.
    ChatCompletions,
}

/// Resolve the provider capability for a given [`ApiProvider`] and resolved
/// model string.
///
/// The `resolved_model` should be the final model identifier that will appear
/// in the API payload (after normalization / provider-specific mapping).
#[must_use]
pub fn provider_capability(provider: ApiProvider, resolved_model: &str) -> ProviderCapability {
    if matches!(
        provider,
        ApiProvider::Openai | ApiProvider::Atlascloud | ApiProvider::Moonshot
    ) {
        return ProviderCapability {
            provider,
            resolved_model: resolved_model.to_string(),
            context_window: crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS,
            max_output: 4096,
            thinking_supported: false,
            cache_telemetry_supported: false,
            request_payload_mode: RequestPayloadMode::ChatCompletions,
            alias_deprecation: None,
        };
    }

    if matches!(provider, ApiProvider::XiaomiMimo) {
        return ProviderCapability {
            provider,
            resolved_model: resolved_model.to_string(),
            context_window: 1_000_000,
            max_output: 128_000,
            thinking_supported: true,
            cache_telemetry_supported: false,
            request_payload_mode: RequestPayloadMode::ChatCompletions,
            alias_deprecation: None,
        };
    }

    if matches!(provider, ApiProvider::Ollama) {
        return ProviderCapability {
            provider,
            resolved_model: resolved_model.to_string(),
            context_window: 8192,
            max_output: 4096,
            thinking_supported: false,
            cache_telemetry_supported: false,
            request_payload_mode: RequestPayloadMode::ChatCompletions,
            alias_deprecation: None,
        };
    }

    let model_lower = resolved_model.to_ascii_lowercase();
    let alias_deprecation = if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        deepseek_alias_deprecation(&model_lower)
    } else {
        None
    };
    let is_v4_pro = model_lower.contains("v4-pro") || model_lower == "deepseek-v4pro";
    let is_v4_flash = model_lower.contains("v4-flash")
        || model_lower == "deepseek-v4flash"
        || model_lower == "deepseek-v4"
        || alias_deprecation.is_some();
    let is_reasoner = matches!(provider, ApiProvider::WanjieArk)
        && (model_lower.contains("reasoner") || model_lower.contains("r1"));

    // Context window: V4-class models get 1M, everything else falls through
    // to the model's own lookup or a default.
    let context_window = if is_v4_pro || is_v4_flash {
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    } else {
        crate::models::context_window_for_model(resolved_model)
            .unwrap_or(crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS)
    };

    // Max output tokens: official DeepSeek V4 API metadata lists 384K;
    // runtime request caps remain separate and more conservative.
    let max_output = if is_v4_pro || is_v4_flash {
        384_000
    } else {
        4096
    };

    // Thinking support: V4 models support thinking on all providers, but
    // only when the model name matches the V4 family.
    let thinking_supported = is_v4_pro || is_v4_flash || is_reasoner;

    // Cache telemetry: returned only by DeepSeek-native and NVIDIA NIM endpoints.
    let cache_telemetry_supported = matches!(
        provider,
        ApiProvider::Deepseek
            | ApiProvider::DeepseekCN
            | ApiProvider::NvidiaNim
            | ApiProvider::Volcengine
    );

    // Request payload mode: all current providers use chat completions.
    let request_payload_mode = RequestPayloadMode::ChatCompletions;

    ProviderCapability {
        provider,
        resolved_model: resolved_model.to_string(),
        context_window,
        max_output,
        thinking_supported,
        cache_telemetry_supported,
        request_payload_mode,
        alias_deprecation,
    }
}

fn deepseek_alias_deprecation(model_lower: &str) -> Option<ModelAliasDeprecation> {
    match model_lower {
        "deepseek-chat" | "deepseek-reasoner" => Some(ModelAliasDeprecation {
            alias: model_lower.to_string(),
            replacement: DEEPSEEK_ALIAS_REPLACEMENT.to_string(),
            retirement_date: DEEPSEEK_ALIAS_RETIREMENT_DATE.to_string(),
            retirement_utc: DEEPSEEK_ALIAS_RETIREMENT_UTC.to_string(),
            notice: format!(
                "{model_lower} is a compatibility alias for {DEEPSEEK_ALIAS_REPLACEMENT} and is scheduled to retire on {DEEPSEEK_ALIAS_RETIREMENT_DATE}."
            ),
        }),
        _ => None,
    }
}

/// Canonicalize compact DeepSeek model aliases to stable IDs.
///
/// Already-valid model IDs pass through unchanged. Only the compact
/// `v4pro`/`v4flash` spellings are rewritten to their hyphenated forms.
#[must_use]
pub fn canonical_model_name(model: &str) -> Option<&'static str> {
    match model.trim().to_ascii_lowercase().as_str() {
        "deepseek-v4pro" => Some("deepseek-v4-pro"),
        "deepseek-v4flash" => Some("deepseek-v4-flash"),
        _ => None,
    }
}

/// Normalize a configured/runtime model name.
///
/// Trims whitespace, preserves caller-provided case for already-valid model
/// IDs, and only canonicalizes compact aliases like `deepseek-v4pro`.
/// Non-DeepSeek or malformed names return `None`; DeepSeek's `/v1/models`
/// endpoint is the authority on valid model IDs.
#[must_use]
pub fn normalize_model_name(model: &str) -> Option<String> {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(canonical) = canonical_model_name(trimmed) {
        return Some(canonical.to_string());
    }

    let normalized = trimmed.to_ascii_lowercase();
    if !normalized.starts_with("deepseek") && !normalized.contains("/deepseek") {
        return None;
    }

    if trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '/'))
    {
        return Some(trimmed.to_string());
    }

    None
}

fn canonical_official_deepseek_model_id(model: &str) -> Option<&'static str> {
    match model.trim().to_ascii_lowercase().as_str() {
        "deepseek-v4-pro"
        | "deepseek-v4pro"
        | "deepseek-ai/deepseek-v4-pro"
        | "deepseek-ai/deepseek-v4pro"
        | "deepseek/deepseek-v4-pro"
        | "deepseek/deepseek-v4pro" => Some("deepseek-v4-pro"),
        "deepseek-v4-flash"
        | "deepseek-v4flash"
        | "deepseek-ai/deepseek-v4-flash"
        | "deepseek-ai/deepseek-v4flash"
        | "deepseek/deepseek-v4-flash"
        | "deepseek/deepseek-v4flash" => Some("deepseek-v4-flash"),
        _ => None,
    }
}

/// Normalize a model selected through the TUI for the active provider.
///
/// Official DeepSeek endpoints require bare model IDs. Provider-prefixed
/// aliases are valid for some compatible backends, but sending them to
/// DeepSeek's own API causes a 400. Keep the generic normalizer permissive for
/// config/back-compat, and canonicalize only when the active provider is known.
///
/// Preserves the caller's casing when the model is already a recognised
/// DeepSeek id (e.g. `DeepSeek-V4-Flash` stays as-is). Only rewrites compact
/// aliases like `deepseek-v4pro` → `deepseek-v4-pro`.
#[must_use]
pub fn normalize_model_name_for_provider(provider: ApiProvider, model: &str) -> Option<String> {
    let normalized = normalize_model_name(model)?;
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        && let Some(canonical) = canonical_official_deepseek_model_id(&normalized)
    {
        // When the user's input already matches a known model id
        // case-insensitively, keep their original casing; only rewrite
        // compact aliases (e.g. v4pro → v4-pro).
        if canonical.eq_ignore_ascii_case(&normalized)
            || normalized.to_ascii_lowercase() == canonical
        {
            return Some(normalized);
        }
        return Some(canonical.to_string());
    }
    if matches!(provider, ApiProvider::Siliconflow) {
        let provider_model = model_for_provider(provider, normalized.clone());
        if provider_model != normalized {
            return Some(provider_model);
        }
    }
    if let Some(canonical) = canonical_official_deepseek_model_id(&normalized) {
        return Some(model_for_provider(provider, canonical.to_string()));
    }
    Some(normalized)
}

#[must_use]
pub fn model_completion_names_for_provider(provider: ApiProvider) -> Vec<&'static str> {
    match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => OFFICIAL_DEEPSEEK_MODELS.to_vec(),
        ApiProvider::NvidiaNim => vec![DEFAULT_NVIDIA_NIM_MODEL, DEFAULT_NVIDIA_NIM_FLASH_MODEL],
        ApiProvider::Openrouter => vec![DEFAULT_OPENROUTER_MODEL, DEFAULT_OPENROUTER_FLASH_MODEL],
        ApiProvider::XiaomiMimo => vec![DEFAULT_XIAOMI_MIMO_MODEL, "mimo-v2.5"],
        ApiProvider::Novita => vec![DEFAULT_NOVITA_MODEL, DEFAULT_NOVITA_FLASH_MODEL],
        ApiProvider::Fireworks => vec![DEFAULT_FIREWORKS_MODEL],
        ApiProvider::Siliconflow => {
            vec![DEFAULT_SILICONFLOW_MODEL, DEFAULT_SILICONFLOW_FLASH_MODEL]
        }
        ApiProvider::Moonshot => vec![DEFAULT_MOONSHOT_MODEL],
        ApiProvider::WanjieArk => vec![DEFAULT_WANJIE_ARK_MODEL],
        ApiProvider::Sglang => vec![DEFAULT_SGLANG_MODEL, DEFAULT_SGLANG_FLASH_MODEL],
        ApiProvider::Vllm => vec![DEFAULT_VLLM_MODEL, DEFAULT_VLLM_FLASH_MODEL],
        ApiProvider::Volcengine => vec![DEFAULT_VOLCENGINE_MODEL, DEFAULT_VOLCENGINE_FLASH_MODEL],
        ApiProvider::Openai | ApiProvider::Atlascloud | ApiProvider::Ollama => {
            OFFICIAL_DEEPSEEK_MODELS.to_vec()
        }
    }
}

// === Types ===

/// Raw retry configuration loaded from config files.
#[derive(Debug, Clone, Deserialize)]
pub struct RetryConfig {
    pub enabled: Option<bool>,
    pub max_retries: Option<u32>,
    pub initial_delay: Option<f64>,
    pub max_delay: Option<f64>,
    pub exponential_base: Option<f64>,
}

/// Deserialize `status_items` tolerantly: skip keys unknown to this build
/// instead of erroring with "unknown variant".  This lets a dev build write
/// `"balance"` (or any future item) while the stable build still parses the
/// config file successfully.
fn deser_status_items<'de, D>(deserializer: D) -> Result<Option<Vec<StatusItem>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<Vec<String>> = Option::deserialize(deserializer)?;
    Ok(raw.map(|strings| {
        strings
            .into_iter()
            .filter_map(|s| {
                StatusItem::from_key(&s).or_else(|| {
                    tracing::warn!("ignoring unknown status item {s:?} in config");
                    None
                })
            })
            .collect()
    }))
}

/// UI configuration loaded from config files.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TuiConfig {
    pub alternate_screen: Option<String>,
    pub mouse_capture: Option<bool>,
    /// Timeout for startup terminal mode/probe calls in milliseconds.
    /// Defaults to 500ms when omitted.
    pub terminal_probe_timeout_ms: Option<u64>,
    /// Ordered list of footer items the user wants visible. `None` (the field
    /// missing from `config.toml`) means "use the built-in default order"; an
    /// empty `Some(vec![])` means "show nothing in the footer".
    ///
    /// Edited interactively via `/statusline`; persisted to `tui.status_items`
    /// in `~/.deepseek/config.toml`.
    #[serde(deserialize_with = "deser_status_items")]
    pub status_items: Option<Vec<StatusItem>>,
    /// Emit OSC 8 hyperlink escape sequences around URLs in the transcript so
    /// supporting terminals (iTerm2, Terminal.app 13+, Ghostty, Kitty,
    /// WezTerm, Alacritty, recent gnome-terminal/konsole) make them
    /// Cmd+click-openable. Terminals without OSC 8 support render the plain
    /// label and ignore the escape. Defaults to `true`; set `false` for
    /// terminals that misrender the sequence.
    pub osc8_links: Option<bool>,
    /// High-level notification trigger condition. When set, overrides the
    /// `[notifications].threshold_secs` gate from the lower-level
    /// `[notifications]` block:
    ///
    /// - `Always` — fire a turn-completion notification on every successful
    ///   turn regardless of duration. The configured `[notifications].method`
    ///   and `include_summary` flag are still respected.
    /// - `Never` — suppress all turn-completion notifications.
    /// - Unset (default) — fall back to the `[notifications]` defaults.
    pub notification_condition: Option<NotificationCondition>,
    /// When `true`, plain Up/Down on an empty composer scroll the
    /// transcript instead of recalling input history. Useful for
    /// terminals that map mouse-wheel gestures to arrow keys. Default:
    /// `true` only when mouse capture is off; otherwise `false`.
    #[serde(default)]
    pub composer_arrows_scroll: Option<bool>,
}

/// High-level notification trigger override. See
/// [`TuiConfig::notification_condition`].
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationCondition {
    /// Notify on every successful turn (no duration threshold).
    Always,
    /// Suppress notifications entirely.
    Never,
}

/// Notification delivery method (mirrors `tui::notifications::Method`).
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationMethod {
    /// Auto-detect: picks the best protocol for the current terminal
    /// (OSC 9, Kitty OSC 99, Ghostty OSC 777, or Bel).
    #[default]
    Auto,
    /// OSC 9 escape.
    Osc9,
    /// Plain BEL character.
    Bel,
    /// Kitty notification protocol (OSC 99).
    Kitty,
    /// Ghostty notification protocol (OSC 777).
    Ghostty,
    /// Disable notifications.
    Off,
}

fn default_threshold_secs() -> u64 {
    30
}

/// Completion sound options.
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CompletionSound {
    /// No sound on turn completion.
    Off,
    /// System notification beep (default). On Windows uses `MessageBeep`.
    #[default]
    Beep,
    /// Terminal BEL character (`\x07`).
    Bell,
}

/// Desktop-notification configuration (OSC 9 / BEL on turn completion).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct NotificationsConfig {
    /// Delivery method: `auto` | `osc9` | `bel` | `off`. Default: `auto`.
    /// `auto` resolves to OSC 9 for iTerm.app / Ghostty / WezTerm / Cmux
    /// (detected via `$TERM_PROGRAM` then `$LC_TERMINAL`); on macOS / Linux
    /// it falls back to BEL, and on Windows it falls back to `Off` so the
    /// post-turn notification doesn't ring the system error chime (#583).
    /// Use `method = "osc9"` explicitly when your terminal is OSC-9 capable
    /// but sets neither env var (e.g. Cmux without `LC_TERMINAL`).
    #[serde(default)]
    pub method: NotificationMethod,
    /// Only notify when the turn took at least this many seconds. Default: 30.
    #[serde(default = "default_threshold_secs")]
    pub threshold_secs: u64,
    /// Include a short summary (elapsed time + cost) in the notification body.
    /// Default: `false`.
    #[serde(default)]
    pub include_summary: bool,

    /// Completion sound: `"off"` | `"beep"` | `"bell"`. Default: `"beep"`.
    /// Plays a sound when every turn finishes (alongside the ✅ marker).
    #[serde(default)]
    pub completion_sound: CompletionSound,
}

fn default_snapshots_enabled() -> bool {
    true
}

fn default_snapshot_max_age_days() -> u64 {
    crate::snapshot::DEFAULT_MAX_AGE.as_secs() / (24 * 60 * 60)
}

fn default_snapshot_max_workspace_gb() -> u64 {
    crate::snapshot::DEFAULT_MAX_WORKSPACE_BYTES_FOR_SNAPSHOT / (1024 * 1024 * 1024)
}

/// Workspace side-git snapshot configuration (#137).
#[derive(Debug, Clone, Deserialize)]
pub struct SnapshotsConfig {
    /// Snapshot the workspace before and after each interactive agent turn.
    #[serde(default = "default_snapshots_enabled")]
    pub enabled: bool,
    /// Prune side-git snapshots older than this many days at session boot.
    #[serde(default = "default_snapshot_max_age_days")]
    pub max_age_days: u64,
    /// Maximum non-excluded workspace size (in GB) before the snapshot
    /// feature self-disables on first use. Set to `0` to disable the cap
    /// and snapshot regardless of size (the v0.8.31 behavior). The walk
    /// honors `.gitignore` and the snapshot module's built-in excludes
    /// (`node_modules/`, `target/`, ...) so the measured size reflects
    /// what would actually land in a snapshot commit.
    #[serde(default = "default_snapshot_max_workspace_gb")]
    pub max_workspace_gb: u64,
}

impl Default for SnapshotsConfig {
    fn default() -> Self {
        Self {
            enabled: default_snapshots_enabled(),
            max_age_days: default_snapshot_max_age_days(),
            max_workspace_gb: default_snapshot_max_workspace_gb(),
        }
    }
}

/// User-level memory configuration (#489).
///
/// Default is opt-in: when this table is absent or `enabled = false`, the
/// memory file is neither read nor written, and `# foo` quick-adds in the
/// composer fall through to the normal turn-submission path.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MemoryConfig {
    /// When `true`, load the user memory file at `Config::memory_path()`
    /// into the system prompt as a `<user_memory>` block, and intercept
    /// `# foo` typed in the composer to append to that file. Default `false`.
    #[serde(default)]
    pub enabled: Option<bool>,
}

impl SnapshotsConfig {
    #[must_use]
    pub fn max_age(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.max_age_days.saturating_mul(24 * 60 * 60))
    }
}

/// Search provider enumeration — selects which backend `web_search` uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchProvider {
    /// Bing HTML scraping. No API key needed.
    Bing,
    /// DuckDuckGo HTML scraping with Bing fallback. No API key needed.
    #[default]
    #[serde(alias = "duckduckgo")]
    DuckDuckGo,
    /// Tavily AI Search API (<https://tavily.com>). Requires api_key.
    Tavily,
    /// Bocha AI Search API (<https://bochaai.com>). Requires api_key.
    Bocha,
    /// Metaso AI Search API (<https://metaso.cn>). Uses built-in default key
    /// or `METASO_API_KEY` env var; configurable via `[search] api_key`.
    #[serde(alias = "metaso")]
    Metaso,
    /// Baidu AI Search API (<https://qianfan.baidubce.com>). Requires api_key.
    #[serde(
        alias = "baidu-search",
        alias = "baidu_ai_search",
        alias = "baidu_search",
        alias = "baidu-ai-search"
    )]
    Baidu,
}

impl SearchProvider {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "bing" => Some(Self::Bing),
            "duckduckgo" | "duck-duck-go" | "duck_duck_go" | "ddg" => Some(Self::DuckDuckGo),
            "tavily" => Some(Self::Tavily),
            "bocha" => Some(Self::Bocha),
            "metaso" => Some(Self::Metaso),
            "baidu" | "baidu-search" | "baidu_search" | "baidu-ai-search" | "baidu_ai_search" => {
                Some(Self::Baidu)
            }
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bing => "bing",
            Self::DuckDuckGo => "duckduckgo",
            Self::Tavily => "tavily",
            Self::Bocha => "bocha",
            Self::Metaso => "metaso",
            Self::Baidu => "baidu",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchProviderSource {
    Default,
    Config,
    EnvOverride,
}

impl SearchProviderSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Config => "config",
            Self::EnvOverride => "env override",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchProviderResolution {
    pub provider: SearchProvider,
    pub source: SearchProviderSource,
}

/// Web search provider configuration (`[search]` table in config.toml).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SearchConfig {
    /// Search provider: `bing` | `duckduckgo` | `tavily` | `bocha` | `metaso` | `baidu`. Default: `duckduckgo`.
    #[serde(default)]
    pub provider: Option<SearchProvider>,
    /// API key for Tavily, Bocha, Metaso, or Baidu. Not required for Bing or DuckDuckGo.
    /// Metaso also falls back to `METASO_API_KEY` env var, then a built-in default.
    /// Baidu also falls back to `BAIDU_SEARCH_API_KEY` env var.
    #[serde(default)]
    pub api_key: Option<String>,
}

/// Model-visible tool catalog controls (`[tools]` table in config.toml).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ToolsConfig {
    /// Native tool names to keep loaded even when they are outside the small
    /// default core catalog. Unknown names are harmless and simply never match.
    #[serde(default)]
    pub always_load: Vec<String>,
}

/// One configurable footer item.
///
/// Order in the user's `Vec<StatusItem>` is preserved: items in the left
/// cluster (`Mode`, `Model`, `Cost`, `Status`) render in the order given;
/// right-cluster chips (`Coherence`, `Agents`, `ReasoningReplay`,
/// `PrefixStability`, `Cache`, `ContextPercent`, `GitBranch`,
/// `LastToolElapsed`, `RateLimit`) likewise honour ordering inside their
/// cluster. The split between left and right is deliberate — left holds steady
/// identity (mode/model/cost), right holds transient signals — so we route
/// each variant to the correct side rather than letting users reorder across
/// the spacer.
///
/// Variants without a current data source (`RateLimit`, `LastToolElapsed`)
/// are intentionally exposed today so the picker is forward-compatible; they
/// render empty until the supporting fields land. Empty spans don't take
/// up footer width, so the user sees no visual artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum StatusItem {
    /// "agent" / "yolo" / "plan" chip.
    Mode,
    /// Model identifier (e.g. `deepseek-v4-pro`).
    Model,
    /// Session cost in the configured display currency.
    Cost,
    /// Activity label: "ready" / "draft" / "working".
    Status,
    /// Coherence intervention label: "refreshing context" / "verifying" / "resetting plan".
    Coherence,
    /// Sub-agent count chip ("3 agents").
    Agents,
    /// Reasoning-replay token count ("rsn 12.3k").
    ReasoningReplay,
    /// Prefix stability ("cache prefix 100%").
    PrefixStability,
    /// Cache hit rate ("cache 73%").
    Cache,
    /// Context-window utilisation percent ("48%").
    ContextPercent,
    /// Current git branch name.
    GitBranch,
    /// Elapsed time of the most recent tool call (placeholder until wired).
    LastToolElapsed,
    /// Remaining rate-limit budget (placeholder until wired).
    RateLimit,
    /// Session token usage: input / cache-hit / output.
    Tokens,
    /// DeepSeek account balance, refreshed once per turn completion.
    Balance,
}

impl StatusItem {
    /// Default footer composition for the always-on status line. Used when
    /// `tui.status_items` is missing from `config.toml` so upgraders see a
    /// concise footer by default; diagnostic chips remain available via
    /// `/statusline` without crowding the main UI.
    #[must_use]
    pub fn default_footer() -> Vec<StatusItem> {
        vec![
            StatusItem::Mode,
            StatusItem::Model,
            StatusItem::Cost,
            StatusItem::Status,
            StatusItem::Coherence,
            StatusItem::Agents,
            StatusItem::ReasoningReplay,
            StatusItem::Cache,
            StatusItem::GitBranch,
            StatusItem::Tokens,
        ]
    }

    /// Stable canonical name used in TOML and the picker label.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            StatusItem::Mode => "mode",
            StatusItem::Model => "model",
            StatusItem::Cost => "cost",
            StatusItem::Status => "status",
            StatusItem::Coherence => "coherence",
            StatusItem::Agents => "agents",
            StatusItem::ReasoningReplay => "reasoning_replay",
            StatusItem::PrefixStability => "prefix_stability",
            StatusItem::Cache => "cache",
            StatusItem::ContextPercent => "context_percent",
            StatusItem::GitBranch => "git_branch",
            StatusItem::LastToolElapsed => "last_tool_elapsed",
            StatusItem::RateLimit => "rate_limit",
            StatusItem::Tokens => "tokens",
            StatusItem::Balance => "balance",
        }
    }

    /// Reverse of [`key`](Self::key): parse a config string back to a variant.
    /// Returns `None` for unknown keys so the config parser can silently skip
    /// items added by newer versions rather than crashing with "unknown variant".
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "mode" => Some(Self::Mode),
            "model" => Some(Self::Model),
            "cost" => Some(Self::Cost),
            "status" => Some(Self::Status),
            "coherence" => Some(Self::Coherence),
            "agents" => Some(Self::Agents),
            "reasoning_replay" => Some(Self::ReasoningReplay),
            "prefix_stability" => Some(Self::PrefixStability),
            "cache" => Some(Self::Cache),
            "context_percent" => Some(Self::ContextPercent),
            "git_branch" => Some(Self::GitBranch),
            "last_tool_elapsed" => Some(Self::LastToolElapsed),
            "rate_limit" => Some(Self::RateLimit),
            "tokens" => Some(Self::Tokens),
            "balance" => Some(Self::Balance),
            _ => None,
        }
    }

    /// Human-readable label for the picker.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            StatusItem::Mode => "Mode",
            StatusItem::Model => "Model",
            StatusItem::Cost => "Session cost",
            StatusItem::Status => "Activity (ready/draft/working)",
            StatusItem::Coherence => "Coherence interventions",
            StatusItem::Agents => "Sub-agents in flight",
            StatusItem::ReasoningReplay => "Reasoning replay tokens",
            StatusItem::PrefixStability => "Prefix stability",
            StatusItem::Cache => "Prompt cache hit rate",
            StatusItem::ContextPercent => "Context window %",
            StatusItem::GitBranch => "Git branch",
            StatusItem::LastToolElapsed => "Last tool elapsed",
            StatusItem::RateLimit => "Rate-limit remaining",
            StatusItem::Tokens => "Session tokens",
            StatusItem::Balance => "Account balance",
        }
    }

    /// One-line hint shown beside the label so the user knows what each item
    /// surfaces without having to toggle it on first.
    #[must_use]
    pub fn hint(self) -> &'static str {
        match self {
            StatusItem::Mode => "agent · yolo · plan",
            StatusItem::Model => "the model id you'll send to",
            StatusItem::Cost => "running total for this session",
            StatusItem::Status => "what the agent is doing right now",
            StatusItem::Coherence => "shown only when the engine intervenes",
            StatusItem::Agents => "agents or RLM work in progress",
            StatusItem::ReasoningReplay => "thinking tokens replayed each turn",
            StatusItem::PrefixStability => "whether system/tools stayed cacheable",
            StatusItem::Cache => "% of prompt served from cache",
            StatusItem::ContextPercent => "tokens used / model context window",
            StatusItem::GitBranch => "current workspace branch",
            StatusItem::LastToolElapsed => "ms of the most recent tool call (placeholder)",
            StatusItem::RateLimit => "remaining requests in the budget (placeholder)",
            StatusItem::Tokens => "input / cache-hit / output token totals",
            StatusItem::Balance => "topped-up + granted balance from DeepSeek",
        }
    }

    /// Every variant in display order — used by the picker to enumerate rows.
    #[must_use]
    pub fn all() -> &'static [StatusItem] {
        &[
            StatusItem::Mode,
            StatusItem::Model,
            StatusItem::Cost,
            StatusItem::Balance,
            StatusItem::Status,
            StatusItem::Coherence,
            StatusItem::Agents,
            StatusItem::ReasoningReplay,
            StatusItem::PrefixStability,
            StatusItem::Cache,
            StatusItem::ContextPercent,
            StatusItem::GitBranch,
            StatusItem::LastToolElapsed,
            StatusItem::RateLimit,
            StatusItem::Tokens,
        ]
    }

    /// Items that belong in the footer's left cluster (steady identity).
    #[must_use]
    pub fn is_left_cluster(self) -> bool {
        matches!(
            self,
            StatusItem::Mode
                | StatusItem::Model
                | StatusItem::Cost
                | StatusItem::Status
                | StatusItem::Balance
        )
    }

    /// Whether this item is relevant for `provider`.  Provider-specific
    /// items return `false` for unsupported providers so the picker doesn't
    /// offer toggles that can never show useful data.
    #[must_use]
    pub fn is_available_for(self, provider: ApiProvider) -> bool {
        match self {
            StatusItem::Balance => {
                matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
            }
            _ => true,
        }
    }
}

/// Resolved retry policy with defaults applied.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub enabled: bool,
    pub max_retries: u32,
    pub initial_delay: f64,
    pub max_delay: f64,
    pub exponential_base: f64,
}

/// Capacity-controller config loaded from config files/environment.
#[derive(Debug, Clone, Deserialize)]
pub struct CapacityConfig {
    pub enabled: Option<bool>,
    pub low_risk_max: Option<f64>,
    pub medium_risk_max: Option<f64>,
    pub severe_min_slack: Option<f64>,
    pub severe_violation_ratio: Option<f64>,
    pub refresh_cooldown_turns: Option<u64>,
    pub replan_cooldown_turns: Option<u64>,
    pub max_replay_per_turn: Option<usize>,
    pub min_turns_before_guardrail: Option<u64>,
    pub profile_window: Option<usize>,
    pub deepseek_v3_2_chat_prior: Option<f64>,
    pub deepseek_v3_2_reasoner_prior: Option<f64>,
    pub deepseek_v4_pro_prior: Option<f64>,
    pub deepseek_v4_flash_prior: Option<f64>,
    pub fallback_default_prior: Option<f64>,
}

impl RetryPolicy {
    /// Compute the backoff delay for a retry attempt.
    #[must_use]
    #[allow(dead_code)] // used by runtime_api; will be wired into client retry loop
    pub fn delay_for_attempt(&self, attempt: u32) -> std::time::Duration {
        let exponent = i32::try_from(attempt).unwrap_or(i32::MAX);
        let delay = self.initial_delay * self.exponential_base.powi(exponent);
        let delay = delay.min(self.max_delay);
        // Clamp to a sane range to guard against NaN/negative from misconfigured values
        let delay = delay.clamp(0.0, 300.0);
        std::time::Duration::from_secs_f64(delay)
    }
}

/// Context management configuration (append-only layered context with Flash seams).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ContextConfig {
    /// Master enable for layered context management. Default: false while
    /// v0.7.5 audits V4 prefix-cache behavior.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Include a deterministic project context pack in the stable prompt
    /// prefix. Default: true; set `[context] project_pack = false` to disable.
    #[serde(default)]
    pub project_pack: Option<bool>,
    /// Verbatim window: last N turns never summarized. Default: 16.
    #[serde(default)]
    pub verbatim_window_turns: Option<usize>,
    /// Soft seam thresholds based on the active request input estimate.
    #[serde(default)]
    pub l1_threshold: Option<usize>,
    #[serde(default)]
    pub l2_threshold: Option<usize>,
    #[serde(default)]
    pub l3_threshold: Option<usize>,
    /// Hard cycle boundary. Default: 768000.
    #[serde(default)]
    pub cycle_threshold: Option<usize>,
    /// Model used for seam/briefing work. Default: "deepseek-v4-flash".
    #[serde(default)]
    pub seam_model: Option<String>,
}

/// Sub-agent model overrides. Keys in `models` can be role names (`worker`,
/// `explorer`, `awaiter`) or type names (`general`, `explore`, `plan`,
/// `review`, `custom`). Per-call explicit model choices still win.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SubagentsConfig {
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub worker_model: Option<String>,
    #[serde(default)]
    pub explorer_model: Option<String>,
    #[serde(default)]
    pub awaiter_model: Option<String>,
    #[serde(default)]
    pub review_model: Option<String>,
    #[serde(default)]
    pub custom_model: Option<String>,
    #[serde(default)]
    pub models: Option<HashMap<String, String>>,
    /// Maximum concurrent sub-agents. Overrides the top-level max_subagents
    /// setting. Clamped to [1, MAX_SUBAGENTS].
    #[serde(default)]
    pub max_concurrent: Option<usize>,
    /// Per-step DeepSeek API timeout for sub-agent requests, in seconds. The
    /// timeout wraps `client.create_message` so a stuck single step cannot
    /// pin the parent's parent-completion wakeup channel indefinitely.
    /// Defaults to `DEFAULT_SUBAGENT_API_TIMEOUT_SECS` (120) and is clamped
    /// to `MIN_SUBAGENT_API_TIMEOUT_SECS..=MAX_SUBAGENT_API_TIMEOUT_SECS`
    /// (1..=1800). Zero or unset uses the legacy 120s default (#1806, #1808).
    #[serde(default)]
    pub api_timeout_secs: Option<u64>,
}

/// `[auto]` table — knobs for the `--model auto` / `/model auto` router.
///
/// `cost_saving` (#1207): when `true`, the auto-mode router prefers
/// `deepseek-v4-flash` for ambiguous requests, only escalating to
/// `deepseek-v4-pro` when the task clearly benefits from deeper reasoning.
/// Default is `false` (balanced — match the existing routing voice).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AutoConfig {
    #[serde(default)]
    pub cost_saving: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PermissionsConfig {
    #[serde(default)]
    pub rules: Vec<ToolPermissionRule>,
}

/// Resolved CLI configuration, including defaults and environment overrides.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    pub provider: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    /// Optional extra HTTP headers sent to model API requests.
    pub http_headers: Option<HashMap<String, String>>,
    pub default_text_model: Option<String>,
    pub auth_mode: Option<String>,
    /// DeepSeek reasoning-effort tier: `"off" | "low" | "medium" | "high" | "max"`.
    /// Defaults to `"max"` at runtime if unset.
    pub reasoning_effort: Option<String>,
    pub tools_file: Option<String>,
    /// Native tool catalog controls. `tools_file` is the legacy external
    /// schema path; this table controls built-in tool loading policy.
    #[serde(default)]
    pub tools: Option<ToolsConfig>,
    pub skills_dir: Option<String>,
    pub mcp_config_path: Option<String>,
    pub notes_path: Option<String>,
    pub memory_path: Option<String>,
    /// When true, set `tool_choice: "required"` and opt compatible function
    /// schemas into DeepSeek beta strict mode. Schemas with root alternatives
    /// stay non-strict to avoid changing optional/one-of tool semantics.
    pub strict_tool_mode: Option<bool>,
    /// Additional system-prompt sources concatenated in declared order
    /// (#454). Paths are expanded via `expand_path` so `~` and env
    /// vars work. Project config overrides user config (replace, not
    /// merge) — that's the typical "this repo needs X plus everything
    /// I already have" pattern, where users put `~/global.md` in the
    /// project's array if they want both. Each file is loaded, capped
    /// at 100 KiB, and skipped (with a warning) on read errors so a
    /// missing optional file doesn't fail the launch.
    pub instructions: Option<Vec<String>>,
    pub allow_shell: Option<bool>,
    pub approval_policy: Option<String>,
    pub sandbox_mode: Option<String>,
    pub yolo: Option<bool>,
    /// Legacy shell command permission rules. These are translated into typed
    /// `exec_shell` allow/deny rules before tool execution.
    #[serde(default)]
    pub auto_allow: Vec<String>,
    #[serde(default)]
    pub auto_deny: Vec<String>,
    /// Typed, persistent tool permission rules.
    #[serde(default)]
    pub permissions: PermissionsConfig,
    /// External sandbox backend: `"none"` or `"opensandbox"`.
    /// When set, exec_shell routes commands through the backend's HTTP API
    /// instead of spawning a local process.
    pub sandbox_backend: Option<String>,
    /// Base URL for the external sandbox backend (default: `"http://localhost:8080"`).
    pub sandbox_url: Option<String>,
    /// Optional API key for the external sandbox backend (sent as Bearer token).
    pub sandbox_api_key: Option<String>,
    /// When true and `/usr/bin/bwrap` is present on Linux, route exec_shell
    /// through bubblewrap instead of relying solely on Landlock (#2184).
    /// Defaults to false. Requires the `bubblewrap` package to be installed
    /// separately — we do NOT vendor bwrap.
    pub prefer_bwrap: Option<bool>,
    pub managed_config_path: Option<String>,
    pub requirements_path: Option<String>,
    pub max_subagents: Option<usize>,
    pub retry: Option<RetryConfig>,
    pub capacity: Option<CapacityConfig>,
    pub features: Option<FeaturesToml>,

    /// TUI configuration (alternate screen, etc.)
    pub tui: Option<TuiConfig>,

    /// Lifecycle hooks configuration
    #[serde(default)]
    pub hooks: Option<HooksConfig>,

    /// Provider-specific credentials and defaults shared with the `codewhale` facade.
    #[serde(default)]
    pub providers: Option<ProvidersConfig>,

    /// Desktop notification settings (OSC 9 / BEL on long turn completion).
    #[serde(default)]
    pub notifications: Option<NotificationsConfig>,

    /// Per-domain network policy (#135). When absent, network tools fall back
    /// to a permissive default that mirrors pre-v0.7.0 behavior.
    #[serde(default)]
    pub network: Option<NetworkPolicyToml>,

    /// Community skill installer settings (#140). When absent, installer
    /// commands fall back to the bundled defaults
    /// ([`crate::skills::install::DEFAULT_REGISTRY_URL`] +
    /// [`crate::skills::install::DEFAULT_MAX_SIZE_BYTES`]).
    #[serde(default)]
    pub skills: Option<SkillsConfig>,

    /// Workspace side-git snapshots (#137). Defaults to enabled with 7-day
    /// retention when the table is absent.
    #[serde(default)]
    pub snapshots: Option<SnapshotsConfig>,

    /// Web search provider configuration. When absent, defaults to DuckDuckGo.
    /// Set `provider` to `bing`, `tavily`, or `bocha` to use those services
    /// instead; Tavily and Bocha also require an `api_key`.
    #[serde(default)]
    pub search: Option<SearchConfig>,

    /// User-level memory file (#489). Default behaviour is **opt-in**:
    /// loading + injection happens only when `[memory] enabled = true` or
    /// `DEEPSEEK_MEMORY=on` is set.
    #[serde(default)]
    pub memory: Option<MemoryConfig>,

    /// Tunables for `--model auto` (#1207). When absent, the auto router
    /// keeps its existing balanced behaviour.
    #[serde(default)]
    pub auto: Option<AutoConfig>,

    /// Post-edit LSP diagnostics injection (#136). When absent, the engine
    /// applies the defaults documented in [`LspConfigToml`].
    #[serde(default)]
    pub lsp: Option<LspConfigToml>,

    /// Append-only layered context management with Flash seam manager (#159).
    #[serde(default)]
    pub context: ContextConfig,

    /// Sub-agent model overrides.
    #[serde(default)]
    pub subagents: Option<SubagentsConfig>,

    /// Runtime API server tuning (`codewhale serve --http`). Currently only
    /// hosts the CORS allow-list extension (whalescale#255 / #561). When the
    /// table is absent, the daemon ships with localhost:3000 / localhost:1420
    /// / tauri://localhost as the only allowed dev origins.
    #[serde(default)]
    pub runtime_api: Option<RuntimeApiConfig>,

    /// Workshop / large-tool-output routing (#548). When absent, the global
    /// default threshold of 4 096 tokens applies and routing is active.
    #[serde(default)]
    pub workshop: Option<crate::tools::large_output_router::WorkshopConfig>,

    /// Vision model configuration for the `image_analyze` tool.
    #[serde(default)]
    pub vision_model: Option<VisionModelConfig>,
}

/// Vision model configuration for the `image_analyze` tool.
/// Uses an OpenAI-compatible vision model API.
#[derive(Debug, Clone, Deserialize)]
pub struct VisionModelConfig {
    /// Model identifier (e.g., "gemini-3.1-flash-lite-preview").
    pub model: String,
    /// API key for the vision model. Inherits from main config if not specified.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Base URL for the vision model API. Defaults to OpenAI.
    #[serde(default)]
    pub base_url: Option<String>,
}

/// `[runtime_api]` table — knobs for the local HTTP/SSE daemon.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct RuntimeApiConfig {
    /// Additional CORS origins to allow on top of the built-in defaults
    /// (`http://localhost:{3000,1420}`, `http://127.0.0.1:{3000,1420}`,
    /// `tauri://localhost`). Useful when developing a UI against a non-default
    /// dev server port (e.g. Vite's default `:5173`).
    ///
    /// Resolution order (highest priority first): `--cors-origin` CLI flag,
    /// `DEEPSEEK_CORS_ORIGINS` env var (comma-separated), this field. Whalescale#255 / #561.
    #[serde(default)]
    pub cors_origins: Option<Vec<String>>,
}

/// `[skills]` table — knobs for the community-skill installer.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SkillsConfig {
    /// Curated registry index. `/skill install <name>` looks up the spec here.
    /// Defaults to [`crate::skills::install::DEFAULT_REGISTRY_URL`].
    #[serde(default)]
    pub registry_url: Option<String>,
    /// Per-skill maximum *uncompressed* size in bytes. Tarballs that exceed
    /// this limit are rejected during validation. Defaults to 5 MiB.
    #[serde(default)]
    pub max_install_size_bytes: Option<u64>,
}

impl SkillsConfig {
    /// Resolve the registry URL with the bundled default.
    #[must_use]
    pub fn registry_url(&self) -> String {
        self.registry_url
            .clone()
            .unwrap_or_else(|| crate::skills::install::DEFAULT_REGISTRY_URL.to_string())
    }

    /// Resolve the max install size with the bundled default.
    #[must_use]
    pub fn max_install_size_bytes(&self) -> u64 {
        self.max_install_size_bytes
            .unwrap_or(crate::skills::install::DEFAULT_MAX_SIZE_BYTES)
    }
}

/// `[network]` table — mirrors `codewhale_config::NetworkPolicyToml` so the live
/// TUI runtime can construct a [`crate::network_policy::NetworkPolicy`]
/// without reaching into the workspace config crate. See `config.example.toml`
/// for documentation.
#[derive(Debug, Clone, Deserialize)]
pub struct NetworkPolicyToml {
    /// Decision for hosts that are not in `allow` or `deny`. One of
    /// `"allow" | "deny" | "prompt"`. Defaults to `"prompt"`.
    #[serde(default = "default_network_decision")]
    pub default: String,
    /// Hosts that are always allowed. Subdomain rules: a leading dot
    /// (`.example.com`) matches subdomains but not the apex.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Hosts that are always denied. Deny entries win over allow entries.
    #[serde(default)]
    pub deny: Vec<String>,
    /// Hostnames whose DNS may resolve to fake-IP/private proxy ranges in an
    /// explicitly trusted proxy setup. Literal IP URLs remain blocked.
    #[serde(default)]
    pub proxy: Vec<String>,
    /// Whether to record one audit-log line per outbound network call.
    #[serde(default = "default_network_audit")]
    pub audit: bool,
}

fn default_network_decision() -> String {
    "prompt".to_string()
}

fn default_network_audit() -> bool {
    true
}

impl Default for NetworkPolicyToml {
    fn default() -> Self {
        Self {
            default: default_network_decision(),
            allow: Vec::new(),
            deny: Vec::new(),
            proxy: Vec::new(),
            audit: default_network_audit(),
        }
    }
}

impl NetworkPolicyToml {
    /// Build a runtime [`crate::network_policy::NetworkPolicy`] from the
    /// on-disk schema.
    #[must_use]
    pub fn into_runtime(self) -> crate::network_policy::NetworkPolicy {
        crate::network_policy::NetworkPolicy {
            default: crate::network_policy::Decision::parse(&self.default).into(),
            allow: self.allow,
            deny: self.deny,
            proxy: self.proxy,
            audit: self.audit,
        }
    }
}

/// `[lsp]` table — mirrors [`crate::lsp::LspConfig`]. Documented in
/// `config.example.toml`. When omitted, defaults from `LspConfig::default()`
/// apply (enabled, 5 s poll, 20 diagnostics/file, errors only, no overrides).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct LspConfigToml {
    /// Master switch. Defaults to `true`.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// How long to wait for the LSP server to publish diagnostics after a
    /// `didOpen`/`didChange`. Defaults to 5000 ms.
    #[serde(default)]
    pub poll_after_edit_ms: Option<u64>,
    /// Cap on diagnostics surfaced per file. Defaults to 20.
    #[serde(default)]
    pub max_diagnostics_per_file: Option<usize>,
    /// Whether to surface warnings in addition to errors. Defaults to `false`.
    #[serde(default)]
    pub include_warnings: Option<bool>,
    /// Optional override for the `Language -> [cmd, ...args]` table. Keys
    /// are language slugs (`"rust"`, `"go"`, etc.).
    #[serde(default)]
    pub servers: Option<HashMap<String, Vec<String>>>,
}

impl LspConfigToml {
    /// Build a runtime [`crate::lsp::LspConfig`] from the on-disk schema,
    /// falling back to defaults for any unset fields.
    #[must_use]
    pub fn into_runtime(self) -> crate::lsp::LspConfig {
        let defaults = crate::lsp::LspConfig::default();
        crate::lsp::LspConfig {
            enabled: self.enabled.unwrap_or(defaults.enabled),
            poll_after_edit_ms: self
                .poll_after_edit_ms
                .unwrap_or(defaults.poll_after_edit_ms),
            max_diagnostics_per_file: self
                .max_diagnostics_per_file
                .unwrap_or(defaults.max_diagnostics_per_file),
            include_warnings: self.include_warnings.unwrap_or(defaults.include_warnings),
            servers: self.servers.unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProviderConfig {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub auth_mode: Option<String>,
    pub http_headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProvidersConfig {
    #[serde(default)]
    pub deepseek: ProviderConfig,
    #[serde(default)]
    pub deepseek_cn: ProviderConfig,
    #[serde(default)]
    pub nvidia_nim: ProviderConfig,
    #[serde(default)]
    pub openai: ProviderConfig,
    #[serde(default)]
    pub atlascloud: ProviderConfig,
    #[serde(default)]
    pub wanjie_ark: ProviderConfig,
    #[serde(default)]
    pub volcengine: ProviderConfig,
    #[serde(default)]
    pub openrouter: ProviderConfig,
    #[serde(default)]
    pub xiaomi_mimo: ProviderConfig,
    #[serde(default)]
    pub novita: ProviderConfig,
    #[serde(default)]
    pub fireworks: ProviderConfig,
    #[serde(default)]
    pub siliconflow: ProviderConfig,
    #[serde(default)]
    pub moonshot: ProviderConfig,
    #[serde(default)]
    pub sglang: ProviderConfig,
    #[serde(default)]
    pub vllm: ProviderConfig,
    #[serde(default)]
    pub ollama: ProviderConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ConfigFile {
    #[serde(flatten)]
    base: Config,
    profiles: Option<HashMap<String, Config>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RequirementsFile {
    #[serde(default)]
    allowed_approval_policies: Vec<String>,
    #[serde(default)]
    allowed_sandbox_modes: Vec<String>,
}

// === Config Loading ===

impl Config {
    #[must_use]
    pub fn search_provider_resolution(&self) -> SearchProviderResolution {
        if let Ok(raw) = std::env::var("DEEPSEEK_SEARCH_PROVIDER")
            && let Some(provider) = SearchProvider::parse(&raw)
        {
            return SearchProviderResolution {
                provider,
                source: SearchProviderSource::EnvOverride,
            };
        }

        if let Some(provider) = self.search.as_ref().and_then(|search| search.provider) {
            return SearchProviderResolution {
                provider,
                source: SearchProviderSource::Config,
            };
        }

        SearchProviderResolution {
            provider: SearchProvider::default(),
            source: SearchProviderSource::Default,
        }
    }

    #[must_use]
    pub fn search_provider(&self) -> SearchProvider {
        self.search_provider_resolution().provider
    }

    /// Return `true` if the `[auto] cost_saving = true` opt-in is set
    /// (#1207). When true, the auto-mode router biases toward
    /// `deepseek-v4-flash` for ambiguous requests instead of escalating to
    /// `deepseek-v4-pro`. Default: `false` (balanced behaviour).
    #[must_use]
    pub fn auto_cost_saving(&self) -> bool {
        self.auto
            .as_ref()
            .and_then(|a| a.cost_saving)
            .unwrap_or(false)
    }

    #[must_use]
    pub fn tools_always_load(&self) -> std::collections::HashSet<String> {
        self.tools
            .as_ref()
            .map(|tools| {
                tools
                    .always_load
                    .iter()
                    .map(|name| name.trim())
                    .filter(|name| !name.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Load configuration from disk and merge with environment overrides.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// # use crate::config::Config;
    /// let config = Config::load(None, None)?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn load(path: Option<PathBuf>, profile: Option<&str>) -> Result<Self> {
        let path = resolve_load_config_path(path);
        let mut config = if let Some(path) = path.as_ref() {
            if path.exists() {
                let contents = fs::read_to_string(path)
                    .with_context(|| format!("Failed to read config file: {}", path.display()))?;
                let parsed: ConfigFile = toml::from_str(&contents)
                    .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
                apply_profile(parsed, profile)?
            } else {
                Config::default()
            }
        } else {
            Config::default()
        };

        apply_user_permissions_file(&mut config, path.as_deref())?;
        apply_env_overrides(&mut config);
        apply_managed_overrides(&mut config)?;
        apply_requirements(&mut config)?;
        normalize_model_config(&mut config);
        config.validate()?;
        config.warn_on_misplaced_root_base_url();
        Ok(config)
    }

    /// Surface a one-line warning when the user has set the legacy root
    /// `base_url` field but their active provider is not DeepSeek (the only
    /// provider that actually reads that field, plus an NvidiaNim back-compat
    /// sniff). Common confusion: users add `base_url = "..."` at the top of
    /// `~/.deepseek/config.toml` for ollama / vllm / openai-compat servers
    /// and wonder why it's silently ignored (#1308).
    fn warn_on_misplaced_root_base_url(&self) {
        let Some(root_base) = self.base_url.as_deref().map(str::trim) else {
            return;
        };
        if root_base.is_empty() {
            return;
        }
        let provider = self.api_provider();
        if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
            return;
        }
        if matches!(provider, ApiProvider::NvidiaNim)
            && root_base.contains("integrate.api.nvidia.com")
        {
            return;
        }
        // Only warn if the per-provider table doesn't have an explicit
        // `base_url`, because if it does, the per-provider one wins and the
        // root field is just dead config — no behavior surprise.
        let has_provider_base = self
            .provider_config_for(provider)
            .and_then(|p| p.base_url.as_deref().map(str::trim))
            .is_some_and(|s| !s.is_empty());
        if has_provider_base {
            return;
        }
        let table = match provider {
            ApiProvider::Openai => "providers.openai",
            ApiProvider::Atlascloud => "providers.atlascloud",
            ApiProvider::WanjieArk => "providers.wanjie_ark",
            ApiProvider::Openrouter => "providers.openrouter",
            ApiProvider::XiaomiMimo => "providers.xiaomi_mimo",
            ApiProvider::Novita => "providers.novita",
            ApiProvider::Fireworks => "providers.fireworks",
            ApiProvider::Siliconflow => "providers.siliconflow",
            ApiProvider::Moonshot => "providers.moonshot",
            ApiProvider::Sglang => "providers.sglang",
            ApiProvider::Vllm => "providers.vllm",
            ApiProvider::Ollama => "providers.ollama",
            ApiProvider::Volcengine => "providers.volcengine",
            ApiProvider::NvidiaNim => "providers.nvidia_nim",
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => return,
        };
        tracing::warn!(
            "Top-level `base_url = \"{root_base}\"` is ignored for the {provider:?} provider. \
             Move it under `[{table}]` (e.g. `[{table}]\\nbase_url = \"...\"`) \
             or set the corresponding `*_BASE_URL` env var. (#1308)"
        );
    }

    /// Validate that critical config fields are present.
    pub fn validate(&self) -> Result<()> {
        if let Some(provider) = self.provider.as_deref()
            && ApiProvider::parse(provider).is_none()
        {
            anyhow::bail!(
                "Invalid provider '{provider}': expected deepseek, deepseek-cn, nvidia-nim, openai, atlascloud, wanjie-ark, openrouter, xiaomi-mimo, novita, fireworks, siliconflow, moonshot, sglang, vllm, or ollama."
            );
        }
        if let Some(ref key) = self.api_key
            && key.trim().is_empty()
        {
            anyhow::bail!("api_key cannot be empty string");
        }
        if let Some(features) = &self.features {
            for key in features.entries.keys() {
                if !is_known_feature_key(key) {
                    anyhow::bail!("Unknown feature flag: {key}");
                }
            }
        }
        if let Some(model) = self.default_text_model.as_deref()
            && !model.trim().eq_ignore_ascii_case("auto")
            && !provider_passes_model_through(self.api_provider())
            && !self.active_provider_preserves_custom_base_url_model()
            && normalize_model_name(model).is_none()
        {
            anyhow::bail!(
                "Invalid default_text_model '{model}': expected auto or a DeepSeek model ID (for example: deepseek-v4-pro, deepseek-v4-flash, deepseek-ai/deepseek-v4-pro)."
            );
        }
        if let Some(policy) = self.approval_policy.as_deref() {
            let normalized = policy.trim().to_ascii_lowercase();
            if !matches!(
                normalized.as_str(),
                "on-request" | "untrusted" | "never" | "auto" | "suggest"
            ) {
                anyhow::bail!(
                    "Invalid approval_policy '{policy}': expected on-request, untrusted, never, auto, or suggest."
                );
            }
        }
        if let Some(mode) = self.sandbox_mode.as_deref() {
            let normalized = mode.trim().to_ascii_lowercase();
            if !matches!(
                normalized.as_str(),
                "read-only" | "workspace-write" | "danger-full-access" | "external-sandbox"
            ) {
                anyhow::bail!(
                    "Invalid sandbox_mode '{mode}': expected read-only, workspace-write, danger-full-access, or external-sandbox."
                );
            }
        }
        if let Some(tui) = &self.tui
            && let Some(mode) = tui.alternate_screen.as_deref()
        {
            let mode = mode.to_ascii_lowercase();
            if !matches!(mode.as_str(), "auto" | "always" | "never") {
                anyhow::bail!(
                    "Invalid tui.alternate_screen '{mode}': expected auto, always, or never."
                );
            }
        }
        if let Some(capacity) = &self.capacity {
            if let Some(v) = capacity.low_risk_max
                && !(0.0..=1.0).contains(&v)
            {
                anyhow::bail!(
                    "Invalid capacity.low_risk_max '{v}': expected a value in [0.0, 1.0]."
                );
            }
            if let Some(v) = capacity.medium_risk_max
                && !(0.0..=1.0).contains(&v)
            {
                anyhow::bail!(
                    "Invalid capacity.medium_risk_max '{v}': expected a value in [0.0, 1.0]."
                );
            }
            if let (Some(low), Some(medium)) = (capacity.low_risk_max, capacity.medium_risk_max)
                && low > medium
            {
                anyhow::bail!(
                    "Invalid capacity thresholds: low_risk_max ({low}) must be <= medium_risk_max ({medium})."
                );
            }
            if let Some(v) = capacity.severe_violation_ratio
                && !(0.0..=1.0).contains(&v)
            {
                anyhow::bail!(
                    "Invalid capacity.severe_violation_ratio '{v}': expected a value in [0.0, 1.0]."
                );
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn api_provider(&self) -> ApiProvider {
        self.provider
            .as_deref()
            .and_then(ApiProvider::parse)
            .unwrap_or_else(|| {
                self.base_url
                    .as_deref()
                    .filter(|base| base.contains("integrate.api.nvidia.com"))
                    .map(|_| ApiProvider::NvidiaNim)
                    .or_else(|| {
                        self.base_url
                            .as_deref()
                            .filter(|base| base.contains("api.deepseeki.com"))
                            .map(|_| ApiProvider::DeepseekCN)
                    })
                    .unwrap_or(ApiProvider::Deepseek)
            })
    }

    pub(crate) fn provider_config_for(&self, provider: ApiProvider) -> Option<&ProviderConfig> {
        let providers = self.providers.as_ref()?;
        Some(match provider {
            ApiProvider::Deepseek => &providers.deepseek,
            ApiProvider::DeepseekCN => &providers.deepseek_cn,
            ApiProvider::NvidiaNim => &providers.nvidia_nim,
            ApiProvider::Openai => &providers.openai,
            ApiProvider::Atlascloud => &providers.atlascloud,
            ApiProvider::WanjieArk => &providers.wanjie_ark,
            ApiProvider::Openrouter => &providers.openrouter,
            ApiProvider::XiaomiMimo => &providers.xiaomi_mimo,
            ApiProvider::Novita => &providers.novita,
            ApiProvider::Fireworks => &providers.fireworks,
            ApiProvider::Siliconflow => &providers.siliconflow,
            ApiProvider::Moonshot => &providers.moonshot,
            ApiProvider::Sglang => &providers.sglang,
            ApiProvider::Vllm => &providers.vllm,
            ApiProvider::Ollama => &providers.ollama,
            ApiProvider::Volcengine => &providers.volcengine,
        })
    }

    pub(crate) fn provider_config(&self) -> Option<&ProviderConfig> {
        self.provider_config_for(self.api_provider())
    }

    #[must_use]
    pub fn http_headers(&self) -> HashMap<String, String> {
        let mut headers = self.http_headers.clone().unwrap_or_default();
        if let Some(provider_headers) = self
            .provider_config()
            .and_then(|provider| provider.http_headers.as_ref())
        {
            headers.extend(provider_headers.clone());
        }
        headers.retain(|name, value| !name.trim().is_empty() && !value.trim().is_empty());
        headers
    }

    #[must_use]
    pub fn default_model(&self) -> String {
        let provider = self.api_provider();
        if let Some(model) = self
            .provider_config()
            .and_then(|provider| provider.model.as_deref())
        {
            if provider_passes_model_through(provider)
                || self.active_provider_preserves_custom_base_url_model()
            {
                return model.trim().to_string();
            }
            if let Some(normalized) = normalize_model_for_provider(provider, model) {
                return normalized;
            }
            // An explicit provider-scoped model that is not a recognized
            // DeepSeek alias is a deliberate custom choice for a non-DeepSeek
            // provider (e.g. `MiniMax-M2.7` on an OpenAI-compatible endpoint).
            // It must pass through verbatim rather than fall back to a
            // DeepSeek/provider default (issue #1714).
            if !matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
                let trimmed = model.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
        let moonshot_config = (provider == ApiProvider::Moonshot)
            .then(|| self.provider_config())
            .flatten();
        let moonshot_uses_kimi_code = moonshot_config.is_some_and(|config| {
            provider_config_uses_kimi_oauth(config)
                || config
                    .base_url
                    .as_deref()
                    .is_some_and(moonshot_base_url_uses_kimi_code)
        });
        if moonshot_uses_kimi_code {
            return DEFAULT_KIMI_CODE_MODEL.to_string();
        }
        if let Some(model) = self.default_text_model.as_deref()
            && (provider_passes_model_through(provider)
                || self.active_provider_preserves_custom_base_url_model())
        {
            return model.trim().to_string();
        }
        if let Some(model) = self.default_text_model.as_deref()
            && model.trim().eq_ignore_ascii_case("auto")
        {
            return "auto".to_string();
        }
        if let Some(model) = self.default_text_model.as_deref()
            && let Some(normalized) = normalize_model_name(model)
        {
            return model_for_provider(provider, normalized);
        }

        match provider {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => DEFAULT_TEXT_MODEL,
            ApiProvider::NvidiaNim => DEFAULT_NVIDIA_NIM_MODEL,
            ApiProvider::Openai => DEFAULT_OPENAI_MODEL,
            ApiProvider::Atlascloud => DEFAULT_ATLASCLOUD_MODEL,
            ApiProvider::WanjieArk => DEFAULT_WANJIE_ARK_MODEL,
            ApiProvider::Openrouter => DEFAULT_OPENROUTER_MODEL,
            ApiProvider::XiaomiMimo => DEFAULT_XIAOMI_MIMO_MODEL,
            ApiProvider::Novita => DEFAULT_NOVITA_MODEL,
            ApiProvider::Fireworks => DEFAULT_FIREWORKS_MODEL,
            ApiProvider::Siliconflow => DEFAULT_SILICONFLOW_MODEL,
            ApiProvider::Moonshot => DEFAULT_MOONSHOT_MODEL,
            ApiProvider::Sglang => DEFAULT_SGLANG_MODEL,
            ApiProvider::Vllm => DEFAULT_VLLM_MODEL,
            ApiProvider::Ollama => DEFAULT_OLLAMA_MODEL,
            ApiProvider::Volcengine => DEFAULT_VOLCENGINE_MODEL,
        }
        .to_string()
    }

    /// Return the configured API base URL (normalized).
    #[must_use]
    pub fn deepseek_base_url(&self) -> String {
        let provider = self.api_provider();
        let provider_base = self
            .provider_config_for(provider)
            .and_then(|provider| provider.base_url.clone());
        // Root `base_url` is the legacy DeepSeek field; only NvidiaNim has a
        // back-compat sniff (integrate.api.nvidia.com). OpenRouter / Novita
        // were added in v0.6.7 and require explicit `[providers.<name>]`
        // entries or the corresponding `*_BASE_URL` env var.
        let root_base = match provider {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => self.base_url.clone(),
            ApiProvider::NvidiaNim => self
                .base_url
                .as_ref()
                .filter(|base| base.contains("integrate.api.nvidia.com"))
                .cloned(),
            ApiProvider::Openai
            | ApiProvider::Atlascloud
            | ApiProvider::WanjieArk
            | ApiProvider::Openrouter
            | ApiProvider::XiaomiMimo
            | ApiProvider::Novita
            | ApiProvider::Fireworks
            | ApiProvider::Siliconflow
            | ApiProvider::Moonshot
            | ApiProvider::Sglang
            | ApiProvider::Vllm
            | ApiProvider::Ollama
            | ApiProvider::Volcengine => None,
        };
        let base = provider_base.or(root_base).unwrap_or_else(|| {
            match provider {
                ApiProvider::Deepseek => DEFAULT_DEEPSEEK_BASE_URL,
                ApiProvider::DeepseekCN => DEFAULT_DEEPSEEKCN_BASE_URL,
                ApiProvider::NvidiaNim => DEFAULT_NVIDIA_NIM_BASE_URL,
                ApiProvider::Openai => DEFAULT_OPENAI_BASE_URL,
                ApiProvider::Atlascloud => DEFAULT_ATLASCLOUD_BASE_URL,
                ApiProvider::WanjieArk => DEFAULT_WANJIE_ARK_BASE_URL,
                ApiProvider::Openrouter => DEFAULT_OPENROUTER_BASE_URL,
                ApiProvider::XiaomiMimo => DEFAULT_XIAOMI_MIMO_BASE_URL,
                ApiProvider::Novita => DEFAULT_NOVITA_BASE_URL,
                ApiProvider::Fireworks => DEFAULT_FIREWORKS_BASE_URL,
                ApiProvider::Siliconflow => DEFAULT_SILICONFLOW_BASE_URL,
                ApiProvider::Moonshot => {
                    if self
                        .provider_config()
                        .is_some_and(provider_config_uses_kimi_oauth)
                    {
                        DEFAULT_KIMI_CODE_BASE_URL
                    } else {
                        DEFAULT_MOONSHOT_BASE_URL
                    }
                }
                ApiProvider::Sglang => DEFAULT_SGLANG_BASE_URL,
                ApiProvider::Vllm => DEFAULT_VLLM_BASE_URL,
                ApiProvider::Ollama => DEFAULT_OLLAMA_BASE_URL,
                ApiProvider::Volcengine => DEFAULT_VOLCENGINE_BASE_URL,
            }
            .to_string()
        });
        normalize_base_url(&base)
    }

    fn active_provider_preserves_custom_base_url_model(&self) -> bool {
        let provider = self.api_provider();
        provider_preserves_custom_base_url_model(provider, &self.deepseek_base_url())
    }

    /// Read the API key.
    ///
    /// Precedence: **explicit in-memory override → provider/root config
    /// → environment**.
    ///
    /// The in-memory `self.api_key` override is only honored when the user
    /// explicitly set the field (not the legacy `API_KEYRING_SENTINEL`
    /// placeholder, not empty whitespace).
    pub fn deepseek_api_key(&self) -> Result<String> {
        let provider = self.api_provider();
        let slot = match provider {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => "deepseek",
            ApiProvider::NvidiaNim => "nvidia-nim",
            ApiProvider::Openai => "openai",
            ApiProvider::Atlascloud => "atlascloud",
            ApiProvider::WanjieArk => "wanjie-ark",
            ApiProvider::Openrouter => "openrouter",
            ApiProvider::XiaomiMimo => "xiaomi-mimo",
            ApiProvider::Novita => "novita",
            ApiProvider::Fireworks => "fireworks",
            ApiProvider::Siliconflow => "siliconflow",
            ApiProvider::Moonshot => "moonshot",
            ApiProvider::Sglang => "sglang",
            ApiProvider::Vllm => "vllm",
            ApiProvider::Ollama => "ollama",
            ApiProvider::Volcengine => "volcengine",
        };

        // 0. DeepSeek compatibility slot. The legacy top-level `api_key`
        // belongs to DeepSeek only; provider-specific keys below must win for
        // NIM/OpenRouter/etc. so a stale DeepSeek key is not sent elsewhere.
        if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
            && let Some(configured) = self.api_key.as_ref()
            && !configured.trim().is_empty()
            && configured != API_KEYRING_SENTINEL
        {
            return Ok(configured.clone());
        }

        if provider == ApiProvider::Moonshot
            && self
                .provider_config_for(provider)
                .is_some_and(provider_config_uses_kimi_oauth)
        {
            return kimi_cli_oauth_access_token();
        }

        // 1. Config file (provider-scoped slot). This intentionally wins
        // over ambient env so `codewhale auth set` fixes stale shell exports.
        if let Some(configured) = self
            .provider_config_for(provider)
            .and_then(|provider| provider.api_key.clone())
            && !configured.trim().is_empty()
        {
            return Ok(configured);
        }

        // 2. Environment variables. Do not query platform credential stores
        // here; routine startup and doctor checks must stay prompt-free.
        if let Some(value) = codewhale_secrets::env_for(slot)
            && !value.trim().is_empty()
        {
            return Ok(value);
        }

        if base_url_uses_local_host(&self.deepseek_base_url()) {
            return Ok(String::new());
        }

        match provider {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => anyhow::bail!(
                "DeepSeek API key not found.\n\
                 \n\
                 1. Get a key:  https://platform.deepseek.com/api_keys\n\
                 2. Save it (works in every folder, no OS prompts):\n\
                        codewhale auth set --provider deepseek\n\
                 \n\
                 Alternatives:\n\
                   • export DEEPSEEK_API_KEY=<your-key>      (current shell only;\n\
                     also note: zsh users — exports in ~/.zshrc only reach interactive\n\
                     shells, prefer ~/.zshenv for everything)\n\
                   • api_key = \"<your-key>\"  in ~/.deepseek/config.toml"
            ),
            ApiProvider::NvidiaNim => anyhow::bail!(
                "NVIDIA NIM API key not found. Run 'codewhale auth set --provider nvidia-nim', \
                 set NVIDIA_API_KEY/NVIDIA_NIM_API_KEY, or save api_key in ~/.deepseek/config.toml \
                 with provider = \"nvidia-nim\"."
            ),
            ApiProvider::Openai => anyhow::bail!(
                "OpenAI-compatible API key not found. Run 'codewhale auth set --provider openai', \
                 set OPENAI_API_KEY, or add [providers.openai] api_key in ~/.deepseek/config.toml."
            ),
            ApiProvider::Atlascloud => anyhow::bail!(
                "AtlasCloud API key not found. Run 'codewhale auth set --provider atlascloud', \
                 set ATLASCLOUD_API_KEY, or add [providers.atlascloud] api_key in ~/.deepseek/config.toml."
            ),
            ApiProvider::WanjieArk => anyhow::bail!(
                "Wanjie Ark API key not found. Run 'codewhale auth set --provider wanjie-ark', \
                 set WANJIE_ARK_API_KEY/WANJIE_API_KEY/WANJIE_MAAS_API_KEY, or add \
                 [providers.wanjie_ark] api_key in ~/.deepseek/config.toml."
            ),
            ApiProvider::Openrouter => anyhow::bail!(
                "OpenRouter API key not found. Run 'codewhale auth set --provider openrouter', \
                 set OPENROUTER_API_KEY, or add [providers.openrouter] api_key in ~/.deepseek/config.toml."
            ),
            ApiProvider::XiaomiMimo => anyhow::bail!(
                "Xiaomi MiMo API key not found. Run 'codewhale auth set --provider xiaomi-mimo', \
                 set XIAOMI_MIMO_API_KEY/MIMO_API_KEY, or add [providers.xiaomi_mimo] api_key in ~/.deepseek/config.toml."
            ),
            ApiProvider::Novita => anyhow::bail!(
                "Novita API key not found. Run 'codewhale auth set --provider novita', \
                 set NOVITA_API_KEY, or add [providers.novita] api_key in ~/.deepseek/config.toml."
            ),
            ApiProvider::Fireworks => anyhow::bail!(
                "Fireworks AI API key not found. Run 'codewhale auth set --provider fireworks', \
                 set FIREWORKS_API_KEY, or add [providers.fireworks] api_key in ~/.deepseek/config.toml."
            ),
            ApiProvider::Siliconflow => anyhow::bail!(
                "SiliconFlow API key not found. Run 'codewhale auth set --provider siliconflow', \
                 set SILICONFLOW_API_KEY, or add [providers.siliconflow] api_key in ~/.deepseek/config.toml."
            ),
            ApiProvider::Moonshot => anyhow::bail!(
                "Moonshot/Kimi API key not found. Run 'codewhale auth set --provider moonshot', \
                 set MOONSHOT_API_KEY/KIMI_API_KEY, or add [providers.moonshot] api_key. \
                 For a Kimi Code plan key, set [providers.moonshot] base_url = \
                 \"https://api.kimi.com/coding/v1\" and model = \"kimi-for-coding\"."
            ),
            // Self-hosted deployments commonly run without auth on localhost.
            // Return an empty key and let the client omit the Authorization header.
            ApiProvider::Sglang
            | ApiProvider::Vllm
            | ApiProvider::Ollama
            | ApiProvider::Volcengine => Ok(String::new()),
        }
    }

    /// Resolve the skills directory path.
    #[must_use]
    pub fn skills_dir(&self) -> PathBuf {
        self.skills_dir
            .as_deref()
            .map(expand_path)
            .or_else(default_skills_dir)
            .unwrap_or_else(|| PathBuf::from("./skills"))
    }

    /// Resolve the MCP config path.
    #[must_use]
    pub fn mcp_config_path(&self) -> PathBuf {
        self.mcp_config_path
            .as_deref()
            .map(expand_path)
            .or_else(default_mcp_config_path)
            .unwrap_or_else(|| PathBuf::from("./mcp.json"))
    }

    /// Resolve the notes file path.
    #[must_use]
    pub fn notes_path(&self) -> PathBuf {
        self.notes_path
            .as_deref()
            .map(expand_path)
            .or_else(default_notes_path)
            .unwrap_or_else(|| PathBuf::from("./notes.txt"))
    }

    /// Resolve the memory file path.
    #[must_use]
    pub fn memory_path(&self) -> PathBuf {
        self.memory_path
            .as_deref()
            .map(expand_path)
            .or_else(default_memory_path)
            .unwrap_or_else(|| PathBuf::from("./memory.md"))
    }

    /// Resolve the configured `instructions = [...]` array (#454)
    /// to absolute paths, in declared order. Empty when unset or
    /// when every entry is empty after trimming. Each entry runs
    /// through `expand_path` so `~` and env vars are honoured.
    #[must_use]
    pub fn instructions_paths(&self) -> Vec<PathBuf> {
        self.instructions
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(String::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(expand_path)
            .collect()
    }

    /// Whether the user-memory feature is enabled. The default is **off**
    /// to preserve zero-overhead behavior for users who haven't opted in.
    /// Flips to `true` when `[memory] enabled = true` in `config.toml` or
    /// `DEEPSEEK_MEMORY=on` is set in the environment.
    #[must_use]
    pub fn memory_enabled(&self) -> bool {
        self.memory
            .as_ref()
            .and_then(|m| m.enabled)
            .unwrap_or(false)
    }

    /// Return the configured vision model config, inheriting api_key from main config.
    #[must_use]
    pub fn vision_model_config(&self) -> Option<VisionModelConfig> {
        let mut config = self.vision_model.clone()?;
        if config.api_key.is_none() {
            config.api_key = self.api_key.clone();
        }
        Some(config)
    }

    #[must_use]
    pub fn project_context_pack_enabled(&self) -> bool {
        self.context.project_pack.unwrap_or(true)
    }

    /// Return whether shell execution is allowed. Defaults to `false`: shell
    /// access must be opted into explicitly (GHSA-72w5-pf8h-xfp4).
    #[must_use]
    pub fn allow_shell(&self) -> bool {
        self.allow_shell.unwrap_or(false)
    }

    /// Build the user-layer permission ruleset used by the tool execution
    /// approval path.
    #[must_use]
    pub fn permission_ruleset(&self) -> Ruleset {
        Ruleset::user(self.auto_allow.clone(), self.auto_deny.clone())
            .with_rules(self.permissions.rules.clone())
    }

    /// Build the permission engine for shell/file tool approval decisions.
    #[must_use]
    pub fn exec_policy_engine(&self) -> ExecPolicyEngine {
        ExecPolicyEngine::with_rulesets(vec![self.permission_ruleset()])
    }

    /// Return the maximum number of concurrent sub-agents.
    /// Checks `[subagents] max_concurrent` first, then top-level `max_subagents`,
    /// then falls back to `DEFAULT_MAX_SUBAGENTS`.
    #[must_use]
    pub fn max_subagents(&self) -> usize {
        // Check [subagents] max_concurrent first
        if let Some(subagents_cfg) = self.subagents.as_ref()
            && let Some(max) = subagents_cfg.max_concurrent
        {
            return max.clamp(1, MAX_SUBAGENTS);
        }
        // Fall back to top-level max_subagents
        self.max_subagents
            .unwrap_or(DEFAULT_MAX_SUBAGENTS)
            .clamp(1, MAX_SUBAGENTS)
    }

    /// Resolved per-step DeepSeek API timeout for sub-agents, in seconds.
    ///
    /// Reads `[subagents] api_timeout_secs` and clamps to
    /// `[MIN_SUBAGENT_API_TIMEOUT_SECS, MAX_SUBAGENT_API_TIMEOUT_SECS]`
    /// (1..=1800). `None` or `0` resolve to the legacy
    /// `DEFAULT_SUBAGENT_API_TIMEOUT_SECS` (120) so existing configs keep
    /// their old behavior; explicit `1` is honored, useful only in fast
    /// fail-fast tests, not production (#1806, #1808).
    #[must_use]
    pub fn subagent_api_timeout_secs(&self) -> u64 {
        let raw = self
            .subagents
            .as_ref()
            .and_then(|cfg| cfg.api_timeout_secs)
            .unwrap_or(DEFAULT_SUBAGENT_API_TIMEOUT_SECS);
        if raw == 0 {
            return DEFAULT_SUBAGENT_API_TIMEOUT_SECS;
        }
        raw.clamp(MIN_SUBAGENT_API_TIMEOUT_SECS, MAX_SUBAGENT_API_TIMEOUT_SECS)
    }

    /// Raw sub-agent model override map. Values are validated at spawn time
    /// so an invalid role/type model fails before any partial agent spawn.
    #[must_use]
    pub fn subagent_model_overrides(&self) -> HashMap<String, String> {
        let mut overrides = HashMap::new();
        let Some(cfg) = self.subagents.as_ref() else {
            return overrides;
        };

        let mut insert = |key: &str, value: &Option<String>| {
            if let Some(model) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                overrides.insert(key.to_string(), model.to_string());
            }
        };
        insert("default", &cfg.default_model);
        insert("worker", &cfg.worker_model);
        insert("general", &cfg.worker_model);
        insert("explorer", &cfg.explorer_model);
        insert("explore", &cfg.explorer_model);
        insert("awaiter", &cfg.awaiter_model);
        insert("plan", &cfg.awaiter_model);
        insert("review", &cfg.review_model);
        insert("custom", &cfg.custom_model);

        if let Some(models) = cfg.models.as_ref() {
            for (key, model) in models {
                let key = key.trim();
                let model = model.trim();
                if !key.is_empty() && !model.is_empty() {
                    overrides.insert(key.to_ascii_lowercase(), model.to_string());
                }
            }
        }

        overrides
    }

    /// Return the configured DeepSeek reasoning-effort tier, if any.
    #[must_use]
    pub fn reasoning_effort(&self) -> Option<&str> {
        self.reasoning_effort.as_deref()
    }

    /// Get hooks configuration, returning default if not configured.
    pub fn hooks_config(&self) -> HooksConfig {
        self.hooks.clone().unwrap_or_default()
    }

    /// Resolve the notifications configuration with defaults applied.
    #[must_use]
    pub fn notifications_config(&self) -> NotificationsConfig {
        self.notifications.clone().unwrap_or_default()
    }

    /// Resolve workspace side-git snapshot settings with defaults applied.
    #[must_use]
    pub fn snapshots_config(&self) -> SnapshotsConfig {
        self.snapshots.clone().unwrap_or_default()
    }

    /// Resolve enabled features from defaults and config entries.
    #[must_use]
    pub fn features(&self) -> Features {
        let mut features = Features::with_defaults();
        if let Some(table) = &self.features {
            features.apply_map(&table.entries);
        }
        features
    }

    /// Override a feature flag in memory (used by CLI overrides).
    pub fn set_feature(&mut self, key: &str, enabled: bool) -> Result<()> {
        if !is_known_feature_key(key) {
            anyhow::bail!("Unknown feature flag: {key}");
        }
        let table = self.features.get_or_insert_with(FeaturesToml::default);
        table.entries.insert(key.to_string(), enabled);
        Ok(())
    }

    /// Resolve the effective retry policy with defaults applied.
    #[must_use]
    pub fn retry_policy(&self) -> RetryPolicy {
        let defaults = RetryPolicy {
            enabled: true,
            max_retries: 3,
            initial_delay: 1.0,
            max_delay: 60.0,
            exponential_base: 2.0,
        };

        let Some(cfg) = &self.retry else {
            return defaults;
        };

        RetryPolicy {
            enabled: cfg.enabled.unwrap_or(defaults.enabled),
            max_retries: cfg.max_retries.unwrap_or(defaults.max_retries),
            initial_delay: cfg.initial_delay.unwrap_or(defaults.initial_delay),
            max_delay: cfg.max_delay.unwrap_or(defaults.max_delay),
            exponential_base: cfg.exponential_base.unwrap_or(defaults.exponential_base),
        }
    }
}

// === Defaults ===

fn default_config_path() -> Option<PathBuf> {
    env_config_path().or_else(home_config_path)
}

pub(crate) fn effective_home_dir() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("HOME") {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }

    if let Some(path) = std::env::var_os("USERPROFILE") {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }

    #[cfg(windows)]
    {
        if let (Some(drive), Some(homepath)) =
            (std::env::var_os("HOMEDRIVE"), std::env::var_os("HOMEPATH"))
        {
            let mut path = PathBuf::from(drive);
            path.push(homepath);
            if !path.as_os_str().is_empty() {
                return Some(path);
            }
        }
    }

    dirs::home_dir()
}

fn home_config_path() -> Option<PathBuf> {
    effective_home_dir().map(|home| {
        let primary = home.join(".codewhale").join("config.toml");
        if primary.exists() {
            return primary;
        }
        home.join(".deepseek").join("config.toml")
    })
}

#[must_use]
pub(crate) fn is_workspace_trusted(workspace: &Path) -> bool {
    let Some(config_path) = default_config_path() else {
        return false;
    };
    let Ok(raw) = fs::read_to_string(config_path) else {
        return false;
    };
    let Ok(doc) = toml::from_str::<toml::Value>(&raw) else {
        return false;
    };
    workspace_trust_level_from_doc(&doc, workspace).is_some_and(is_trusted_level)
}

pub(crate) fn save_workspace_trust(workspace: &Path) -> Result<PathBuf> {
    let config_path = default_config_path()
        .context("Failed to resolve config path: home directory not found.")?;
    ensure_parent_dir(&config_path)?;

    let mut doc = if config_path.exists() {
        let raw = fs::read_to_string(&config_path)?;
        toml::from_str::<toml::Value>(&raw)
            .with_context(|| format!("Failed to parse config at {}", config_path.display()))?
    } else {
        toml::Value::Table(toml::value::Table::new())
    };

    let root = doc
        .as_table_mut()
        .context("Config root must be a TOML table.")?;
    let projects = root
        .entry("projects".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()))
        .as_table_mut()
        .context("`projects` must be a table.")?;
    let project = projects
        .entry(workspace_config_key(workspace))
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()))
        .as_table_mut()
        .context("Project entry must be a table.")?;
    project.insert(
        "trust_level".to_string(),
        toml::Value::String("trusted".to_string()),
    );

    let serialized = toml::to_string_pretty(&doc).context("failed to serialize updated config")?;
    write_config_file_secure(&config_path, &serialized)
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    Ok(config_path)
}

fn workspace_trust_level_from_doc<'a>(doc: &'a toml::Value, workspace: &Path) -> Option<&'a str> {
    let workspace = canonicalize_or_keep(workspace);
    let projects = doc.get("projects")?.as_table()?;
    for (raw_path, project) in projects {
        let project_path = canonicalize_or_keep(&expand_path(raw_path));
        if project_path == workspace {
            return project.get("trust_level").and_then(toml::Value::as_str);
        }
    }
    None
}

fn is_trusted_level(level: &str) -> bool {
    level.trim().eq_ignore_ascii_case("trusted")
}

fn workspace_config_key(workspace: &Path) -> String {
    canonicalize_or_keep(workspace)
        .to_string_lossy()
        .into_owned()
}

fn canonicalize_or_keep(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn env_config_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("DEEPSEEK_CONFIG_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(expand_path(trimmed));
        }
    }
    None
}

fn expand_pathbuf(path: PathBuf) -> PathBuf {
    if let Some(raw) = path.to_str() {
        return expand_path(raw);
    }
    path
}

fn resolve_load_config_path(path: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(path) = path {
        return Some(expand_pathbuf(path));
    }

    if let Some(path) = env_config_path() {
        if path.exists() {
            return Some(path);
        }

        if let Some(home_path) = home_config_path()
            && home_path.exists()
        {
            return Some(home_path);
        }

        return Some(path);
    }

    home_config_path()
}

/// Create an inspectable config file on first interactive launch.
///
/// The file intentionally omits `api_key`; onboarding or `codewhale auth set`
/// writes that field after the user supplies a key.
pub fn ensure_config_file_exists(path: Option<PathBuf>) -> Result<Option<PathBuf>> {
    let config_path = path
        .map(expand_pathbuf)
        .or_else(default_config_path)
        .context("Failed to resolve config path: home directory not found.")?;
    if config_path.exists() {
        return Ok(None);
    }

    ensure_parent_dir(&config_path)?;
    let content = format!(
        r#"# codewhale Configuration
# Get your API key from https://platform.deepseek.com
# Save it with: codewhale auth set --provider deepseek

# Base URL (default: https://api.deepseek.com/beta)
# Set https://api.deepseek.com to opt out of beta features.
# base_url = "https://api.deepseek.com/beta"

# Default model
default_text_model = "{DEFAULT_TEXT_MODEL}"

# Thinking mode (DeepSeek V4 reasoning effort):
# "auto" | "off" | "low" | "medium" | "high" | "max"
# Shift+Tab in the TUI cycles between off / high / max.
reasoning_effort = "auto"
"#
    );
    write_config_file_secure(&config_path, &content)
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    Ok(Some(config_path))
}

fn default_managed_config_path() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        Some(PathBuf::from("/etc/deepseek/managed_config.toml"))
    }
    #[cfg(not(unix))]
    {
        effective_home_dir().map(|home| {
            let primary = home.join(".codewhale").join("managed_config.toml");
            if primary.exists() {
                return primary;
            }
            home.join(".deepseek").join("managed_config.toml")
        })
    }
}

fn default_requirements_path() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        Some(PathBuf::from("/etc/deepseek/requirements.toml"))
    }
    #[cfg(not(unix))]
    {
        effective_home_dir().map(|home| {
            let primary = home.join(".codewhale").join("requirements.toml");
            if primary.exists() {
                return primary;
            }
            home.join(".deepseek").join("requirements.toml")
        })
    }
}

pub(crate) fn expand_path(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix('~')
        && (stripped.is_empty() || stripped.starts_with('/') || stripped.starts_with('\\'))
        && let Some(mut home) = effective_home_dir()
    {
        let suffix = stripped.trim_start_matches(['/', '\\']);
        if !suffix.is_empty() {
            home.push(suffix);
        }
        return home;
    }

    let expanded = shellexpand::tilde(path);
    PathBuf::from(expanded.as_ref())
}

fn default_skills_dir() -> Option<PathBuf> {
    effective_home_dir().map(|home| home.join(".codewhale").join("skills"))
}

fn default_mcp_config_path() -> Option<PathBuf> {
    effective_home_dir().map(|home| {
        let primary = home.join(".codewhale").join("mcp.json");
        if primary.exists() {
            return primary;
        }
        home.join(".deepseek").join("mcp.json")
    })
}

fn default_notes_path() -> Option<PathBuf> {
    effective_home_dir().map(|home| {
        let primary = home.join(".codewhale").join("notes.txt");
        if primary.exists() {
            return primary;
        }
        home.join(".deepseek").join("notes.txt")
    })
}

fn default_memory_path() -> Option<PathBuf> {
    effective_home_dir().map(|home| {
        let primary = home.join(".codewhale").join("memory.md");
        if primary.exists() {
            return primary;
        }
        home.join(".deepseek").join("memory.md")
    })
}

// === Environment Overrides ===

/// Read a CodeWhale env var, preferring the `CODEWHALE_*` form over the
/// legacy `DEEPSEEK_*` form. Empty values are ignored so a blank shell export
/// does not erase configured provider settings.
fn codewhale_env_var(
    codewhale_name: &str,
    legacy_name: &str,
) -> Result<String, std::env::VarError> {
    std::env::var(codewhale_name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var(legacy_name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .ok_or(std::env::VarError::NotPresent)
}

fn apply_env_overrides(config: &mut Config) {
    if let Ok(value) = codewhale_env_var("CODEWHALE_PROVIDER", "DEEPSEEK_PROVIDER") {
        config.provider = Some(value);
    }
    if let Ok(value) = codewhale_env_var("CODEWHALE_BASE_URL", "DEEPSEEK_BASE_URL") {
        match config.api_provider() {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => {
                config.base_url = Some(value);
            }
            ApiProvider::NvidiaNim => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .nvidia_nim
                    .base_url = Some(value);
            }
            ApiProvider::Openai => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .openai
                    .base_url = Some(value);
            }
            ApiProvider::Openrouter => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .openrouter
                    .base_url = Some(value);
            }
            ApiProvider::XiaomiMimo => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .xiaomi_mimo
                    .base_url = Some(value);
            }
            ApiProvider::WanjieArk => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .wanjie_ark
                    .base_url = Some(value);
            }
            ApiProvider::Novita => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .novita
                    .base_url = Some(value);
            }
            ApiProvider::Fireworks => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .fireworks
                    .base_url = Some(value);
            }
            ApiProvider::Siliconflow => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .siliconflow
                    .base_url = Some(value);
            }
            ApiProvider::Moonshot => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .moonshot
                    .base_url = Some(value);
            }
            ApiProvider::Sglang => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .sglang
                    .base_url = Some(value);
            }
            ApiProvider::Vllm => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .vllm
                    .base_url = Some(value);
            }
            ApiProvider::Ollama => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .ollama
                    .base_url = Some(value);
            }
            ApiProvider::Volcengine => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .volcengine
                    .base_url = Some(value);
            }
            ApiProvider::Atlascloud => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .atlascloud
                    .base_url = Some(value);
            }
        }
    }
    if matches!(config.api_provider(), ApiProvider::NvidiaNim)
        && let Ok(value) = std::env::var("NVIDIA_NIM_BASE_URL")
            .or_else(|_| std::env::var("NIM_BASE_URL"))
            .or_else(|_| std::env::var("NVIDIA_BASE_URL"))
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .nvidia_nim
            .base_url = Some(value);
    }
    // OpenAI-compatible and non-DeepSeek hosted providers are scoped only on
    // their own provider entry — the legacy root `base_url` keeps DeepSeek-only
    // semantics.
    if matches!(config.api_provider(), ApiProvider::Openai)
        && let Ok(value) = std::env::var("OPENAI_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .openai
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Atlascloud)
        && let Ok(value) = std::env::var("ATLASCLOUD_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .atlascloud
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Openrouter)
        && let Ok(value) = std::env::var("OPENROUTER_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .openrouter
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::XiaomiMimo)
        && let Ok(value) =
            std::env::var("XIAOMI_MIMO_BASE_URL").or_else(|_| std::env::var("MIMO_BASE_URL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .xiaomi_mimo
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::WanjieArk)
        && let Ok(value) = std::env::var("WANJIE_ARK_BASE_URL")
            .or_else(|_| std::env::var("WANJIE_BASE_URL"))
            .or_else(|_| std::env::var("WANJIE_MAAS_BASE_URL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .wanjie_ark
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Novita)
        && let Ok(value) = std::env::var("NOVITA_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .novita
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Fireworks)
        && let Ok(value) = std::env::var("FIREWORKS_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .fireworks
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Siliconflow)
        && let Ok(value) = std::env::var("SILICONFLOW_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .siliconflow
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Moonshot)
        && let Ok(value) =
            std::env::var("MOONSHOT_BASE_URL").or_else(|_| std::env::var("KIMI_BASE_URL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .moonshot
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Sglang)
        && let Ok(value) = std::env::var("SGLANG_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .sglang
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Vllm)
        && let Ok(value) = std::env::var("VLLM_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .vllm
            .base_url = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_HTTP_HEADERS")
        && let Ok(headers) = parse_http_headers(&value)
        && !headers.is_empty()
    {
        let mut root_headers = config.http_headers.clone().unwrap_or_default();
        root_headers.extend(headers.clone());
        config.http_headers = Some(root_headers);

        let provider = config.api_provider();
        let providers = config
            .providers
            .get_or_insert_with(ProvidersConfig::default);
        let entry = match provider {
            ApiProvider::Deepseek => &mut providers.deepseek,
            ApiProvider::DeepseekCN => &mut providers.deepseek_cn,
            ApiProvider::NvidiaNim => &mut providers.nvidia_nim,
            ApiProvider::Openai => &mut providers.openai,
            ApiProvider::Atlascloud => &mut providers.atlascloud,
            ApiProvider::WanjieArk => &mut providers.wanjie_ark,
            ApiProvider::Openrouter => &mut providers.openrouter,
            ApiProvider::XiaomiMimo => &mut providers.xiaomi_mimo,
            ApiProvider::Novita => &mut providers.novita,
            ApiProvider::Fireworks => &mut providers.fireworks,
            ApiProvider::Siliconflow => &mut providers.siliconflow,
            ApiProvider::Moonshot => &mut providers.moonshot,
            ApiProvider::Sglang => &mut providers.sglang,
            ApiProvider::Vllm => &mut providers.vllm,
            ApiProvider::Ollama => &mut providers.ollama,
            ApiProvider::Volcengine => &mut providers.volcengine,
        };
        let mut provider_headers = entry.http_headers.clone().unwrap_or_default();
        provider_headers.extend(headers);
        entry.http_headers = Some(provider_headers);
    }
    if matches!(config.api_provider(), ApiProvider::Ollama)
        && let Ok(value) = std::env::var("OLLAMA_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .ollama
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Sglang)
        && let Ok(value) = std::env::var("SGLANG_MODEL")
    {
        config.default_text_model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Vllm)
        && let Ok(value) = std::env::var("VLLM_MODEL")
    {
        config.default_text_model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Ollama)
        && let Ok(value) = std::env::var("OLLAMA_MODEL")
    {
        config.default_text_model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Openai)
        && let Ok(value) = std::env::var("OPENAI_MODEL")
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .openai
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::XiaomiMimo)
        && let Ok(value) =
            std::env::var("XIAOMI_MIMO_MODEL").or_else(|_| std::env::var("MIMO_MODEL"))
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .xiaomi_mimo
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Atlascloud)
        && let Ok(value) = std::env::var("ATLASCLOUD_MODEL")
    {
        config.default_text_model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::WanjieArk)
        && let Ok(value) = std::env::var("WANJIE_ARK_MODEL")
            .or_else(|_| std::env::var("WANJIE_MODEL"))
            .or_else(|_| std::env::var("WANJIE_MAAS_MODEL"))
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .wanjie_ark
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Moonshot)
        && let Ok(value) = std::env::var("MOONSHOT_MODEL")
            .or_else(|_| std::env::var("KIMI_MODEL_NAME"))
            .or_else(|_| std::env::var("KIMI_MODEL"))
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .moonshot
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Siliconflow)
        && let Ok(value) = std::env::var("SILICONFLOW_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .siliconflow
            .model = Some(value);
    }
    if let Some(value) = codewhale_env_var("CODEWHALE_MODEL", "DEEPSEEK_MODEL")
        .ok()
        .or_else(|| {
            std::env::var("DEEPSEEK_DEFAULT_TEXT_MODEL")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
    {
        // The CLI `--model` handoff always sets DEEPSEEK_MODEL, never the
        // provider-specific *_MODEL var. The legacy root `default_text_model`
        // is a DeepSeek-only slot (the validator rejects non-DeepSeek IDs
        // there). For a non-DeepSeek provider the explicit model must land in
        // the provider-scoped slot instead so the verbatim-passthrough path
        // honors it rather than falling back to a DeepSeek/provider default
        // (issue #1714). Mirror the OPENAI_MODEL branch above for every
        // non-DeepSeek provider.
        let provider = config.api_provider();
        if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
            config.default_text_model = Some(value);
        } else {
            let providers = config
                .providers
                .get_or_insert_with(ProvidersConfig::default);
            let entry = match provider {
                ApiProvider::Deepseek | ApiProvider::DeepseekCN => unreachable!(
                    "DeepSeek providers are handled in the if branch above (issue #1714)"
                ),
                ApiProvider::NvidiaNim => &mut providers.nvidia_nim,
                ApiProvider::Openai => &mut providers.openai,
                ApiProvider::Atlascloud => &mut providers.atlascloud,
                ApiProvider::WanjieArk => &mut providers.wanjie_ark,
                ApiProvider::Openrouter => &mut providers.openrouter,
                ApiProvider::XiaomiMimo => &mut providers.xiaomi_mimo,
                ApiProvider::Novita => &mut providers.novita,
                ApiProvider::Fireworks => &mut providers.fireworks,
                ApiProvider::Siliconflow => &mut providers.siliconflow,
                ApiProvider::Moonshot => &mut providers.moonshot,
                ApiProvider::Sglang => &mut providers.sglang,
                ApiProvider::Vllm => &mut providers.vllm,
                ApiProvider::Ollama => &mut providers.ollama,
                ApiProvider::Volcengine => &mut providers.volcengine,
            };
            entry.model = Some(value);
        }
    }
    if matches!(config.api_provider(), ApiProvider::NvidiaNim)
        && let Ok(value) = std::env::var("NVIDIA_NIM_MODEL")
    {
        config.default_text_model = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_SKILLS_DIR") {
        config.skills_dir = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_MCP_CONFIG") {
        config.mcp_config_path = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_NOTES_PATH") {
        config.notes_path = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_MEMORY_PATH") {
        config.memory_path = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_MEMORY") {
        let on = matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "on" | "true" | "yes" | "y" | "enabled"
        );
        config
            .memory
            .get_or_insert_with(MemoryConfig::default)
            .enabled = Some(on);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_ALLOW_SHELL") {
        config.allow_shell = Some(value == "1" || value.eq_ignore_ascii_case("true"));
    }
    if let Ok(value) = std::env::var("DEEPSEEK_APPROVAL_POLICY") {
        config.approval_policy = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_SANDBOX_MODE") {
        config.sandbox_mode = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_YOLO") {
        config.yolo = Some(value == "1" || value.eq_ignore_ascii_case("true"));
    }
    if let Ok(value) = std::env::var("DEEPSEEK_SANDBOX_BACKEND") {
        config.sandbox_backend = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_SANDBOX_URL") {
        config.sandbox_url = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_SANDBOX_API_KEY") {
        config.sandbox_api_key = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_MANAGED_CONFIG_PATH") {
        config.managed_config_path = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_SEARCH_API_KEY")
        && !value.trim().is_empty()
    {
        config
            .search
            .get_or_insert_with(SearchConfig::default)
            .api_key = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_REQUIREMENTS_PATH") {
        config.requirements_path = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_MAX_SUBAGENTS")
        && let Ok(parsed) = value.parse::<usize>()
    {
        config.max_subagents = Some(parsed.clamp(1, MAX_SUBAGENTS));
    }

    let capacity = config.capacity.get_or_insert(CapacityConfig {
        enabled: None,
        low_risk_max: None,
        medium_risk_max: None,
        severe_min_slack: None,
        severe_violation_ratio: None,
        refresh_cooldown_turns: None,
        replan_cooldown_turns: None,
        max_replay_per_turn: None,
        min_turns_before_guardrail: None,
        profile_window: None,
        deepseek_v3_2_chat_prior: None,
        deepseek_v3_2_reasoner_prior: None,
        deepseek_v4_pro_prior: None,
        deepseek_v4_flash_prior: None,
        fallback_default_prior: None,
    });

    if let Ok(value) = std::env::var("DEEPSEEK_CAPACITY_ENABLED") {
        let val = value.trim().to_ascii_lowercase();
        capacity.enabled = Some(matches!(val.as_str(), "1" | "true" | "yes" | "on"));
    }
    if let Ok(value) = std::env::var("DEEPSEEK_CAPACITY_LOW_RISK_MAX")
        && let Ok(parsed) = value.parse::<f64>()
    {
        capacity.low_risk_max = Some(parsed);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_CAPACITY_MEDIUM_RISK_MAX")
        && let Ok(parsed) = value.parse::<f64>()
    {
        capacity.medium_risk_max = Some(parsed);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_CAPACITY_SEVERE_MIN_SLACK")
        && let Ok(parsed) = value.parse::<f64>()
    {
        capacity.severe_min_slack = Some(parsed);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_CAPACITY_SEVERE_VIOLATION_RATIO")
        && let Ok(parsed) = value.parse::<f64>()
    {
        capacity.severe_violation_ratio = Some(parsed);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_CAPACITY_REFRESH_COOLDOWN_TURNS")
        && let Ok(parsed) = value.parse::<u64>()
    {
        capacity.refresh_cooldown_turns = Some(parsed);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_CAPACITY_REPLAN_COOLDOWN_TURNS")
        && let Ok(parsed) = value.parse::<u64>()
    {
        capacity.replan_cooldown_turns = Some(parsed);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_CAPACITY_MAX_REPLAY_PER_TURN")
        && let Ok(parsed) = value.parse::<usize>()
    {
        capacity.max_replay_per_turn = Some(parsed);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_CAPACITY_MIN_TURNS_BEFORE_GUARDRAIL")
        && let Ok(parsed) = value.parse::<u64>()
    {
        capacity.min_turns_before_guardrail = Some(parsed);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_CAPACITY_PROFILE_WINDOW")
        && let Ok(parsed) = value.parse::<usize>()
    {
        capacity.profile_window = Some(parsed);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_CAPACITY_PRIOR_CHAT")
        && let Ok(parsed) = value.parse::<f64>()
    {
        capacity.deepseek_v3_2_chat_prior = Some(parsed);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_CAPACITY_PRIOR_REASONER")
        && let Ok(parsed) = value.parse::<f64>()
    {
        capacity.deepseek_v3_2_reasoner_prior = Some(parsed);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_CAPACITY_PRIOR_V4_PRO")
        && let Ok(parsed) = value.parse::<f64>()
    {
        capacity.deepseek_v4_pro_prior = Some(parsed);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_CAPACITY_PRIOR_V4_FLASH")
        && let Ok(parsed) = value.parse::<f64>()
    {
        capacity.deepseek_v4_flash_prior = Some(parsed);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_CAPACITY_PRIOR_FALLBACK")
        && let Ok(parsed) = value.parse::<f64>()
    {
        capacity.fallback_default_prior = Some(parsed);
    }

    if config.capacity.as_ref().is_some_and(|c| {
        c.enabled.is_none()
            && c.low_risk_max.is_none()
            && c.medium_risk_max.is_none()
            && c.severe_min_slack.is_none()
            && c.severe_violation_ratio.is_none()
            && c.refresh_cooldown_turns.is_none()
            && c.replan_cooldown_turns.is_none()
            && c.max_replay_per_turn.is_none()
            && c.min_turns_before_guardrail.is_none()
            && c.profile_window.is_none()
            && c.deepseek_v3_2_chat_prior.is_none()
            && c.deepseek_v3_2_reasoner_prior.is_none()
            && c.deepseek_v4_pro_prior.is_none()
            && c.deepseek_v4_flash_prior.is_none()
            && c.fallback_default_prior.is_none()
    }) {
        config.capacity = None;
    }
}

fn normalize_model_config(config: &mut Config) {
    if let Some(model) = config.default_text_model.as_deref()
        && !provider_passes_model_through(config.api_provider())
        && !config.active_provider_preserves_custom_base_url_model()
        && let Some(normalized) = normalize_model_for_provider(config.api_provider(), model)
    {
        config.default_text_model = Some(normalized);
    }

    if let Some(providers) = config.providers.as_mut() {
        if let Some(model) = providers.deepseek.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::Deepseek, &providers.deepseek)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Deepseek, model)
        {
            providers.deepseek.model = Some(normalized);
        }
        if let Some(model) = providers.deepseek_cn.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::DeepseekCN, &providers.deepseek_cn)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::DeepseekCN, model)
        {
            providers.deepseek_cn.model = Some(normalized);
        }
        if let Some(model) = providers.nvidia_nim.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::NvidiaNim, &providers.nvidia_nim)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::NvidiaNim, model)
        {
            providers.nvidia_nim.model = Some(normalized);
        }
        if let Some(model) = providers.openrouter.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::Openrouter, &providers.openrouter)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Openrouter, model)
        {
            providers.openrouter.model = Some(normalized);
        }
        if let Some(model) = providers.novita.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::Novita, &providers.novita)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Novita, model)
        {
            providers.novita.model = Some(normalized);
        }
        if let Some(model) = providers.fireworks.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::Fireworks, &providers.fireworks)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Fireworks, model)
        {
            providers.fireworks.model = Some(normalized);
        }
        if let Some(model) = providers.siliconflow.model.as_deref()
            && !provider_entry_uses_custom_base_url(
                ApiProvider::Siliconflow,
                &providers.siliconflow,
            )
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Siliconflow, model)
        {
            providers.siliconflow.model = Some(normalized);
        }
        if let Some(model) = providers.moonshot.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::Moonshot, &providers.moonshot)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Moonshot, model)
        {
            providers.moonshot.model = Some(normalized);
        }
        if let Some(model) = providers.sglang.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::Sglang, &providers.sglang)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Sglang, model)
        {
            providers.sglang.model = Some(normalized);
        }
        if let Some(model) = providers.vllm.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::Vllm, &providers.vllm)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Vllm, model)
        {
            providers.vllm.model = Some(normalized);
        }
    }
}

fn normalize_model_for_provider(provider: ApiProvider, model: &str) -> Option<String> {
    if provider_passes_model_through(provider) {
        return None;
    }
    normalize_model_name(model).map(|normalized| model_for_provider(provider, normalized))
}

pub(crate) fn provider_passes_model_through(provider: ApiProvider) -> bool {
    matches!(
        provider,
        ApiProvider::Openai
            | ApiProvider::Atlascloud
            | ApiProvider::WanjieArk
            | ApiProvider::Volcengine
            | ApiProvider::XiaomiMimo
            | ApiProvider::Moonshot
            | ApiProvider::Ollama
    )
}

fn provider_entry_uses_custom_base_url(provider: ApiProvider, entry: &ProviderConfig) -> bool {
    entry
        .base_url
        .as_deref()
        .is_some_and(|base_url| provider_preserves_custom_base_url_model(provider, base_url))
}

fn default_base_url_for_provider(provider: ApiProvider) -> &'static str {
    match provider {
        ApiProvider::Deepseek => DEFAULT_DEEPSEEK_BASE_URL,
        ApiProvider::DeepseekCN => DEFAULT_DEEPSEEKCN_BASE_URL,
        ApiProvider::NvidiaNim => DEFAULT_NVIDIA_NIM_BASE_URL,
        ApiProvider::Openai => DEFAULT_OPENAI_BASE_URL,
        ApiProvider::Atlascloud => DEFAULT_ATLASCLOUD_BASE_URL,
        ApiProvider::WanjieArk => DEFAULT_WANJIE_ARK_BASE_URL,
        ApiProvider::Openrouter => DEFAULT_OPENROUTER_BASE_URL,
        ApiProvider::XiaomiMimo => DEFAULT_XIAOMI_MIMO_BASE_URL,
        ApiProvider::Novita => DEFAULT_NOVITA_BASE_URL,
        ApiProvider::Fireworks => DEFAULT_FIREWORKS_BASE_URL,
        ApiProvider::Siliconflow => DEFAULT_SILICONFLOW_BASE_URL,
        ApiProvider::Moonshot => DEFAULT_MOONSHOT_BASE_URL,
        ApiProvider::Sglang => DEFAULT_SGLANG_BASE_URL,
        ApiProvider::Vllm => DEFAULT_VLLM_BASE_URL,
        ApiProvider::Ollama => DEFAULT_OLLAMA_BASE_URL,
        ApiProvider::Volcengine => DEFAULT_VOLCENGINE_BASE_URL,
    }
}

fn base_url_is_custom_for_provider(provider: ApiProvider, base_url: &str) -> bool {
    if provider == ApiProvider::Siliconflow && siliconflow_base_url_is_official(base_url) {
        return false;
    }
    normalize_base_url(base_url) != normalize_base_url(default_base_url_for_provider(provider))
}

fn provider_preserves_custom_base_url_model(provider: ApiProvider, base_url: &str) -> bool {
    base_url_is_custom_for_provider(provider, base_url)
}

fn siliconflow_base_url_is_official(base_url: &str) -> bool {
    matches!(
        normalize_base_url(base_url).to_ascii_lowercase().as_str(),
        "https://api.siliconflow.com/v1" | "https://api.siliconflow.cn/v1"
    )
}

fn moonshot_base_url_uses_kimi_code(base_url: &str) -> bool {
    let normalized = normalize_base_url(base_url).to_ascii_lowercase();
    normalized == DEFAULT_KIMI_CODE_BASE_URL
        || normalized == "https://api.kimi.com/coding"
        || normalized.starts_with("https://api.kimi.com/coding/")
}

fn provider_config_uses_kimi_oauth(config: &ProviderConfig) -> bool {
    config
        .auth_mode
        .as_deref()
        .is_some_and(auth_mode_uses_kimi_oauth)
}

fn auth_mode_uses_kimi_oauth(mode: &str) -> bool {
    matches!(
        normalize_auth_mode(mode).as_str(),
        "kimi" | "kimi_oauth" | "kimi_cli" | "oauth"
    )
}

fn normalize_auth_mode(mode: &str) -> String {
    mode.trim().to_ascii_lowercase().replace(['-', ' '], "_")
}

fn base_url_uses_local_host(base_url: &str) -> bool {
    let Some(host) = base_url_host(base_url) else {
        return false;
    };
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if matches!(host.as_str(), "localhost" | "0.0.0.0") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|addr| addr.is_loopback() || addr.is_unspecified())
}

fn base_url_host(base_url: &str) -> Option<&str> {
    let without_scheme = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    let authority = without_scheme.split('/').next()?.rsplit('@').next()?;
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split_once(']').map(|(host, _)| host);
    }
    authority.split(':').next().filter(|host| !host.is_empty())
}

fn model_for_provider(provider: ApiProvider, normalized: String) -> String {
    let lowered = normalized.to_ascii_lowercase();
    match (provider, lowered.as_str()) {
        (ApiProvider::NvidiaNim, "deepseek-v4-pro") => DEFAULT_NVIDIA_NIM_MODEL.to_string(),
        (ApiProvider::NvidiaNim, "deepseek-v4-flash") => DEFAULT_NVIDIA_NIM_FLASH_MODEL.to_string(),
        (ApiProvider::Openrouter, "deepseek-v4-pro") => DEFAULT_OPENROUTER_MODEL.to_string(),
        (ApiProvider::Openrouter, "deepseek-v4-flash") => {
            DEFAULT_OPENROUTER_FLASH_MODEL.to_string()
        }
        (ApiProvider::Novita, "deepseek-v4-pro") => DEFAULT_NOVITA_MODEL.to_string(),
        (ApiProvider::Novita, "deepseek-v4-flash") => DEFAULT_NOVITA_FLASH_MODEL.to_string(),
        (ApiProvider::Fireworks, "deepseek-v4-pro") => DEFAULT_FIREWORKS_MODEL.to_string(),
        (ApiProvider::Fireworks, "deepseek-v4-flash") => {
            // Flash not yet available on Fireworks; fall through to normalized name
            "accounts/fireworks/models/deepseek-v4-flash".to_string()
        }
        (ApiProvider::Siliconflow, "deepseek-v4-pro" | "deepseek-reasoner" | "deepseek-r1") => {
            DEFAULT_SILICONFLOW_MODEL.to_string()
        }
        (ApiProvider::Siliconflow, "deepseek-v4-flash" | "deepseek-chat" | "deepseek-v3") => {
            DEFAULT_SILICONFLOW_FLASH_MODEL.to_string()
        }
        (ApiProvider::Sglang, "deepseek-v4-pro") => DEFAULT_SGLANG_MODEL.to_string(),
        (ApiProvider::Sglang, "deepseek-v4-flash") => DEFAULT_SGLANG_FLASH_MODEL.to_string(),
        (ApiProvider::Vllm, "deepseek-v4-pro") => DEFAULT_VLLM_MODEL.to_string(),
        (ApiProvider::Vllm, "deepseek-v4-flash") => DEFAULT_VLLM_FLASH_MODEL.to_string(),
        _ => normalized,
    }
}

fn normalize_base_url(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    let deepseek_domains = ["api.deepseek.com", "api.deepseeki.com"];
    if deepseek_domains
        .iter()
        .any(|domain| trimmed.contains(domain))
    {
        return trimmed.trim_end_matches("/v1").to_string();
    }
    trimmed.to_string()
}

fn parse_http_headers(raw: &str) -> Result<HashMap<String, String>> {
    let mut headers = HashMap::new();
    for pair in raw.trim().split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let Some((name, value)) = pair.split_once('=') else {
            anyhow::bail!("invalid header pair '{pair}', expected name=value");
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() {
            anyhow::bail!("header name cannot be empty");
        }
        if value.is_empty() {
            continue;
        }
        headers.insert(name.to_string(), value.to_string());
    }
    Ok(headers)
}

fn apply_profile(config: ConfigFile, profile: Option<&str>) -> Result<Config> {
    if let Some(profile_name) = profile {
        let profiles = config.profiles.as_ref();
        match profiles.and_then(|profiles| profiles.get(profile_name)) {
            Some(override_cfg) => Ok(merge_config(config.base, override_cfg.clone())),
            None => {
                let available = profiles
                    .map(|profiles| {
                        let mut keys = profiles.keys().cloned().collect::<Vec<_>>();
                        keys.sort();
                        if keys.is_empty() {
                            "none".to_string()
                        } else {
                            keys.join(", ")
                        }
                    })
                    .unwrap_or_else(|| "none".to_string());
                anyhow::bail!("Profile '{profile_name}' not found. Available profiles: {available}")
            }
        }
    } else {
        Ok(config.base)
    }
}

fn merge_config(base: Config, override_cfg: Config) -> Config {
    Config {
        provider: override_cfg.provider.or(base.provider),
        api_key: override_cfg.api_key.or(base.api_key),
        base_url: override_cfg.base_url.or(base.base_url),
        http_headers: override_cfg.http_headers.or(base.http_headers),
        default_text_model: override_cfg.default_text_model.or(base.default_text_model),
        auth_mode: override_cfg.auth_mode.or(base.auth_mode),
        reasoning_effort: override_cfg.reasoning_effort.or(base.reasoning_effort),
        tools_file: override_cfg.tools_file.or(base.tools_file),
        tools: override_cfg.tools.or(base.tools),
        skills_dir: override_cfg.skills_dir.or(base.skills_dir),
        mcp_config_path: override_cfg.mcp_config_path.or(base.mcp_config_path),
        notes_path: override_cfg.notes_path.or(base.notes_path),
        memory_path: override_cfg.memory_path.or(base.memory_path),
        vision_model: override_cfg.vision_model.or(base.vision_model),
        // #454: project's instructions array replaces user's array
        // wholesale. The typical "merge" pattern is for users who want
        // both — they list `~/global.md` inside the project array.
        instructions: override_cfg.instructions.or(base.instructions),
        allow_shell: override_cfg.allow_shell.or(base.allow_shell),
        yolo: override_cfg.yolo.or(base.yolo),
        approval_policy: override_cfg.approval_policy.or(base.approval_policy),
        sandbox_mode: override_cfg.sandbox_mode.or(base.sandbox_mode),
        auto_allow: merge_non_empty_vec(base.auto_allow, override_cfg.auto_allow),
        auto_deny: merge_non_empty_vec(base.auto_deny, override_cfg.auto_deny),
        permissions: merge_permissions_config(base.permissions, override_cfg.permissions),
        sandbox_backend: override_cfg.sandbox_backend.or(base.sandbox_backend),
        sandbox_url: override_cfg.sandbox_url.or(base.sandbox_url),
        sandbox_api_key: override_cfg.sandbox_api_key.or(base.sandbox_api_key),
        prefer_bwrap: override_cfg.prefer_bwrap.or(base.prefer_bwrap),
        managed_config_path: override_cfg
            .managed_config_path
            .or(base.managed_config_path),
        requirements_path: override_cfg.requirements_path.or(base.requirements_path),
        max_subagents: override_cfg.max_subagents.or(base.max_subagents),
        retry: override_cfg.retry.or(base.retry),
        capacity: override_cfg.capacity.or(base.capacity),
        tui: override_cfg.tui.or(base.tui),
        hooks: override_cfg.hooks.or(base.hooks),
        providers: merge_providers(base.providers, override_cfg.providers),
        features: merge_features(base.features, override_cfg.features),
        notifications: override_cfg.notifications.or(base.notifications),
        network: override_cfg.network.or(base.network),
        skills: override_cfg.skills.or(base.skills),
        snapshots: override_cfg.snapshots.or(base.snapshots),
        search: override_cfg.search.or(base.search),
        memory: override_cfg.memory.or(base.memory),
        auto: override_cfg.auto.or(base.auto),
        lsp: override_cfg.lsp.or(base.lsp),
        context: ContextConfig {
            enabled: override_cfg.context.enabled.or(base.context.enabled),
            project_pack: override_cfg
                .context
                .project_pack
                .or(base.context.project_pack),
            verbatim_window_turns: override_cfg
                .context
                .verbatim_window_turns
                .or(base.context.verbatim_window_turns),
            l1_threshold: override_cfg
                .context
                .l1_threshold
                .or(base.context.l1_threshold),
            l2_threshold: override_cfg
                .context
                .l2_threshold
                .or(base.context.l2_threshold),
            l3_threshold: override_cfg
                .context
                .l3_threshold
                .or(base.context.l3_threshold),
            cycle_threshold: override_cfg
                .context
                .cycle_threshold
                .or(base.context.cycle_threshold),
            seam_model: override_cfg.context.seam_model.or(base.context.seam_model),
        },
        subagents: override_cfg.subagents.or(base.subagents),
        strict_tool_mode: override_cfg.strict_tool_mode.or(base.strict_tool_mode),
        runtime_api: override_cfg.runtime_api.or(base.runtime_api),
        workshop: override_cfg.workshop.or(base.workshop),
    }
}

fn merge_non_empty_vec<T>(base: Vec<T>, override_cfg: Vec<T>) -> Vec<T> {
    if override_cfg.is_empty() {
        base
    } else {
        override_cfg
    }
}

fn merge_permissions_config(
    mut base: PermissionsConfig,
    override_cfg: PermissionsConfig,
) -> PermissionsConfig {
    base.rules.extend(override_cfg.rules);
    base
}

fn merge_provider_config(base: ProviderConfig, override_cfg: ProviderConfig) -> ProviderConfig {
    ProviderConfig {
        api_key: override_cfg.api_key.or(base.api_key),
        base_url: override_cfg.base_url.or(base.base_url),
        model: override_cfg.model.or(base.model),
        auth_mode: override_cfg.auth_mode.or(base.auth_mode),
        http_headers: override_cfg.http_headers.or(base.http_headers),
    }
}

fn merge_providers(
    base: Option<ProvidersConfig>,
    override_cfg: Option<ProvidersConfig>,
) -> Option<ProvidersConfig> {
    match (base, override_cfg) {
        (None, None) => None,
        (Some(base), None) => Some(base),
        (None, Some(override_cfg)) => Some(override_cfg),
        (Some(base), Some(override_cfg)) => Some(ProvidersConfig {
            deepseek: merge_provider_config(base.deepseek, override_cfg.deepseek),
            deepseek_cn: merge_provider_config(base.deepseek_cn, override_cfg.deepseek_cn),
            nvidia_nim: merge_provider_config(base.nvidia_nim, override_cfg.nvidia_nim),
            openai: merge_provider_config(base.openai, override_cfg.openai),
            atlascloud: merge_provider_config(base.atlascloud, override_cfg.atlascloud),
            wanjie_ark: merge_provider_config(base.wanjie_ark, override_cfg.wanjie_ark),
            openrouter: merge_provider_config(base.openrouter, override_cfg.openrouter),
            xiaomi_mimo: merge_provider_config(base.xiaomi_mimo, override_cfg.xiaomi_mimo),
            novita: merge_provider_config(base.novita, override_cfg.novita),
            fireworks: merge_provider_config(base.fireworks, override_cfg.fireworks),
            siliconflow: merge_provider_config(base.siliconflow, override_cfg.siliconflow),
            moonshot: merge_provider_config(base.moonshot, override_cfg.moonshot),
            sglang: merge_provider_config(base.sglang, override_cfg.sglang),
            vllm: merge_provider_config(base.vllm, override_cfg.vllm),
            ollama: merge_provider_config(base.ollama, override_cfg.ollama),
            volcengine: merge_provider_config(base.volcengine, override_cfg.volcengine),
        }),
    }
}

fn load_single_config_file(path: &Path) -> Result<Config> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;
    let parsed: ConfigFile = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
    Ok(parsed.base)
}

fn apply_user_permissions_file(config: &mut Config, config_path: Option<&Path>) -> Result<()> {
    let Some(config_path) = config_path else {
        return Ok(());
    };
    let permissions_path = codewhale_config::permissions_path_for_config_path(config_path);
    if !permissions_path.exists() {
        return Ok(());
    }
    let contents = fs::read_to_string(&permissions_path).with_context(|| {
        format!(
            "Failed to read permissions file: {}",
            permissions_path.display()
        )
    })?;
    let permissions: PermissionsConfig = toml::from_str(&contents).with_context(|| {
        format!(
            "Failed to parse permissions file: {}",
            permissions_path.display()
        )
    })?;
    config.permissions.rules.extend(permissions.rules);
    Ok(())
}

fn apply_managed_overrides(config: &mut Config) -> Result<()> {
    let path = config
        .managed_config_path
        .as_deref()
        .map(expand_path)
        .or_else(default_managed_config_path);
    let Some(path) = path else {
        return Ok(());
    };
    if !path.exists() {
        return Ok(());
    }
    let managed = load_single_config_file(&path)?;
    *config = merge_config(config.clone(), managed);
    Ok(())
}

fn apply_requirements(config: &mut Config) -> Result<()> {
    let path = config
        .requirements_path
        .as_deref()
        .map(expand_path)
        .or_else(default_requirements_path);
    let Some(path) = path else {
        return Ok(());
    };
    if !path.exists() {
        return Ok(());
    }
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read requirements file: {}", path.display()))?;
    let requirements: RequirementsFile = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse requirements file: {}", path.display()))?;

    if !requirements.allowed_approval_policies.is_empty()
        && let Some(policy) = config.approval_policy.as_ref()
    {
        let policy = policy.to_ascii_lowercase();
        if !requirements
            .allowed_approval_policies
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&policy))
        {
            anyhow::bail!(
                "approval_policy '{policy}' is not allowed by requirements ({})",
                requirements.allowed_approval_policies.join(", ")
            );
        }
    }
    if !requirements.allowed_sandbox_modes.is_empty()
        && let Some(mode) = config.sandbox_mode.as_ref()
    {
        let mode = mode.to_ascii_lowercase();
        if !requirements
            .allowed_sandbox_modes
            .iter()
            .any(|m| m.eq_ignore_ascii_case(&mode))
        {
            anyhow::bail!(
                "sandbox_mode '{mode}' is not allowed by requirements ({})",
                requirements.allowed_sandbox_modes.join(", ")
            );
        }
    }

    Ok(())
}

fn merge_features(
    base: Option<FeaturesToml>,
    override_cfg: Option<FeaturesToml>,
) -> Option<FeaturesToml> {
    match (base, override_cfg) {
        (None, None) => None,
        (Some(mut base), Some(override_cfg)) => {
            for (key, value) in override_cfg.entries {
                base.entries.insert(key, value);
            }
            Some(base)
        }
        (Some(base), None) => Some(base),
        (None, Some(override_cfg)) => Some(override_cfg),
    }
}

pub fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        #[cfg(unix)]
        {
            // Tighten group/other bits on the parent dir as a hardening pass.
            // The dir lives under the user's home, so the chmod is best-effort:
            // filesystems that don't accept Unix permission bits (Docker
            // bind-mounts of NTFS, network shares, FAT, certain CI volumes —
            // see #897) return EPERM/ENOTSUP. The dir already exists by the
            // time we get here, so failing the whole save just because we
            // couldn't tighten perms strands the user mid-onboarding. Warn
            // loudly so a security-sensitive operator can still notice via
            // `RUST_LOG=warn`, then continue.
            if let Ok(meta) = fs::metadata(parent) {
                let mode = meta.permissions().mode();
                if mode & 0o077 != 0 {
                    let mut perms = meta.permissions();
                    perms.set_mode(mode & !0o077);
                    if let Err(err) = fs::set_permissions(parent, perms) {
                        tracing::warn!(
                            target: "codewhale::config",
                            path = %parent.display(),
                            error = %err,
                            "could not tighten parent dir permissions; \
                             filesystem may not support Unix chmod \
                             (Docker bind-mount, NTFS, network share). \
                             Continuing — the file will still be written."
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

/// Write content to a config file with restrictive permissions (owner-only read/write).
/// On Unix this sets mode 0o600 before writing.
fn write_config_file_secure(path: &Path, content: &str) -> Result<()> {
    #[cfg(unix)]
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(content.as_bytes())?;
        // The file was already opened with mode 0o600; the explicit
        // set_permissions re-asserts that on filesystems where mode-at-open
        // didn't take effect (or where the file already existed with broader
        // bits). Filesystems that don't accept Unix chmod at all (Docker
        // bind-mounts of NTFS, network shares — #897) return EPERM. Treat
        // that as a warning rather than failing the whole save: the file
        // contents are written, and on Windows/macOS hosts the parent file
        // system's native ACL model is doing the access control.
        if let Err(err) = file.set_permissions(fs::Permissions::from_mode(0o600)) {
            tracing::warn!(
                target: "codewhale::config",
                path = %path.display(),
                error = %err,
                "could not enforce 0o600 on config file; filesystem may \
                 not support Unix chmod. File contents written; rely on \
                 host ACLs for access control."
            );
        }
    }
    #[cfg(not(unix))]
    {
        fs::write(path, content)?;
    }
    Ok(())
}

/// Where a saved credential ended up. Returned by [`save_api_key`] so
/// the caller can show a confirmation message without leaking the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedCredential {
    /// Stored in **both** the OS keyring and the codewhale config file.
    /// This is the default outcome on platforms with a working keyring
    /// backend: writing both layers defeats the
    /// `keyring → env → config-file` resolution-order shadow that
    /// would otherwise let a stale OS-keyring entry from a previous
    /// install hide the freshly-entered key (#593). The `backend`
    /// label is the value of [`codewhale_secrets::Secrets::backend_name`]
    /// at write time so the toast text can name the actual backend
    /// (`"system keyring"`, `"file-based (~/.deepseek/secrets/)"`).
    KeyringAndConfigFile {
        /// `Secrets::backend_name()` at write time.
        backend: String,
        /// Absolute path to the config file that was also updated.
        path: PathBuf,
    },
    /// Stored in the codewhale config file only. Fallback when no
    /// keyring backend is reachable, or under `cfg(test)` so unit
    /// tests don't pollute the host keyring.
    ConfigFile(PathBuf),
}

impl SavedCredential {
    /// Human-readable description for status / log output. Never
    /// includes the key value.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::KeyringAndConfigFile { backend, path } => {
                format!("OS keyring ({backend}) and {}", path.display())
            }
            Self::ConfigFile(path) => path.display().to_string(),
        }
    }
}

/// Save the active provider's API key.
///
/// **Dual-write strategy (#593):** writes to `~/.deepseek/config.toml`
/// (always) and to the OS keyring via [`codewhale_secrets::Secrets`]
/// (when a backend is reachable). The runtime resolves credentials in
/// `keyring → env → config-file` order; writing to the config file
/// alone — as v0.8.8 through v0.8.10 did — let a stale keyring entry
/// from a prior install silently shadow the fresh value the user just
/// typed during in-TUI onboarding, producing the "no response" symptom
/// reported in #593.
///
/// The config file remains the inspectable durable record (works in
/// npm installs, IDE terminals, and headless boxes alike), and the
/// keyring acts as the layered override that defeats stale-shadow on
/// the resolution path. When the keyring write fails (no backend, OS
/// permission denied, etc.) the config-file write still stands and
/// the function reports a [`SavedCredential::ConfigFile`] outcome —
/// callers should not treat that as a failure.
///
/// Skipped under `cfg(test)` so the suite never touches the host
/// keyring. The `secrets` crate has its own test coverage for
/// keyring set/get.
pub fn save_api_key(api_key: &str) -> Result<SavedCredential> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Refusing to save an empty API key.");
    }

    // Always write the inspectable copy first. The config file is the
    // durable record everyone — including macOS Keychain-prompted
    // first-run, headless CI, and IDE terminals — can rely on.
    let path = save_api_key_to_config_file(trimmed)?;

    // Then mirror to the OS keyring when one is reachable. This
    // overwrites any stale entry from a prior install so
    // `Secrets::resolve` (keyring → env → config-file) no longer
    // shadows the fresh key. Skipped under `cfg(test)` so unit tests
    // can't pollute the host keyring (macOS Always-Allow prompts,
    // cross-test contamination).
    #[cfg(not(test))]
    {
        let secrets = codewhale_secrets::Secrets::auto_detect();
        match secrets.set("deepseek", trimmed) {
            Ok(()) => {
                let backend = secrets.backend_name().to_string();
                log_sensitive_event(
                    "credential.save",
                    json!({
                        "backend": backend.clone(),
                        "config_path": path.display().to_string(),
                        "dual_write": true,
                    }),
                );
                return Ok(SavedCredential::KeyringAndConfigFile { backend, path });
            }
            Err(err) => {
                tracing::warn!("OS keyring write failed; key saved to config.toml only: {err}");
                // Fall through to the ConfigFile-only outcome below.
            }
        }
    }

    Ok(SavedCredential::ConfigFile(path))
}

/// Write the `api_key` slot directly to `config.toml`.
fn save_api_key_to_config_file(api_key: &str) -> Result<PathBuf> {
    fn is_api_key_assignment(line: &str) -> bool {
        let trimmed = line.trim_start();
        trimmed
            .strip_prefix("api_key")
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    }

    let config_path = default_config_path()
        .context("Failed to resolve config path: home directory not found.")?;

    ensure_parent_dir(&config_path)?;

    let key_to_write = api_key.to_string();

    let content = if config_path.exists() {
        // Read existing config and update the api_key line
        let existing = fs::read_to_string(&config_path)?;
        if existing.contains("api_key") {
            // Replace existing api_key line
            let mut result = String::new();
            for line in existing.lines() {
                if is_api_key_assignment(line) {
                    let _ = writeln!(result, "api_key = \"{key_to_write}\"");
                } else {
                    result.push_str(line);
                    result.push('\n');
                }
            }
            result
        } else {
            // Prepend api_key to existing config
            format!("api_key = \"{key_to_write}\"\n{existing}")
        }
    } else {
        // Create new minimal config
        format!(
            r#"# codewhale Configuration
# Get your API key from https://platform.deepseek.com
# Or set DEEPSEEK_API_KEY environment variable

api_key = "{key_to_write}"

# Base URL (default: https://api.deepseek.com/beta)
# Set https://api.deepseek.com to opt out of beta features.
# base_url = "https://api.deepseek.com/beta"

# Default model
default_text_model = "{DEFAULT_TEXT_MODEL}"

# Thinking mode (DeepSeek V4 reasoning effort):
# "off" | "low" | "medium" | "high" | "max"
# Shift+Tab in the TUI cycles between off / high / max.
reasoning_effort = "max"
"#
        )
    };

    write_config_file_secure(&config_path, &content)
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    log_sensitive_event(
        "credential.save",
        json!({
            "backend": "config_file",
            "config_path": config_path.display().to_string(),
        }),
    );

    Ok(config_path)
}

/// Check if the active provider has any API key configured anywhere the
/// runtime can resolve it.
///
/// Platform credential stores are intentionally not queried here.
/// Startup/onboarding checks must be cheap and prompt-free, so v0.8.8
/// keeps the default auth path to environment variables and
/// `~/.deepseek/config.toml`.
///
/// Used by [`crate::tui::app::App::new`] to decide whether to gate
/// the user behind the in-TUI api-key onboarding screen — getting
/// this wrong made users get prompted for credentials in situations
/// where normal env/config auth was already available.
pub fn has_api_key(config: &Config) -> bool {
    has_api_key_for(config, config.api_provider())
}

#[must_use]
pub fn active_provider_has_config_api_key(config: &Config) -> bool {
    let provider = config.api_provider();

    if provider == ApiProvider::Moonshot
        && config
            .provider_config_for(provider)
            .is_some_and(provider_config_uses_kimi_oauth)
    {
        return kimi_cli_credentials_present();
    }

    if config
        .provider_config_for(provider)
        .and_then(|entry| entry.api_key.as_ref())
        .is_some_and(|k| !k.trim().is_empty() && k != API_KEYRING_SENTINEL)
    {
        return true;
    }

    matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        && config
            .api_key
            .as_ref()
            .is_some_and(|k| !k.trim().is_empty() && k != API_KEYRING_SENTINEL)
}

#[must_use]
pub fn active_provider_has_env_api_key(config: &Config) -> bool {
    match config.api_provider() {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => {
            std::env::var("DEEPSEEK_API_KEY").is_ok_and(|k| !k.trim().is_empty())
        }
        ApiProvider::NvidiaNim => {
            std::env::var("NVIDIA_API_KEY").is_ok_and(|k| !k.trim().is_empty())
                || std::env::var("NVIDIA_NIM_API_KEY").is_ok_and(|k| !k.trim().is_empty())
        }
        ApiProvider::Openai => std::env::var("OPENAI_API_KEY").is_ok_and(|k| !k.trim().is_empty()),
        ApiProvider::Atlascloud => {
            std::env::var("ATLASCLOUD_API_KEY").is_ok_and(|k| !k.trim().is_empty())
        }
        ApiProvider::WanjieArk => {
            std::env::var("WANJIE_ARK_API_KEY").is_ok_and(|k| !k.trim().is_empty())
                || std::env::var("WANJIE_API_KEY").is_ok_and(|k| !k.trim().is_empty())
                || std::env::var("WANJIE_MAAS_API_KEY").is_ok_and(|k| !k.trim().is_empty())
        }
        ApiProvider::Openrouter => {
            std::env::var("OPENROUTER_API_KEY").is_ok_and(|k| !k.trim().is_empty())
        }
        ApiProvider::XiaomiMimo => {
            std::env::var("XIAOMI_MIMO_API_KEY").is_ok_and(|k| !k.trim().is_empty())
                || std::env::var("MIMO_API_KEY").is_ok_and(|k| !k.trim().is_empty())
        }
        ApiProvider::Novita => std::env::var("NOVITA_API_KEY").is_ok_and(|k| !k.trim().is_empty()),
        ApiProvider::Fireworks => {
            std::env::var("FIREWORKS_API_KEY").is_ok_and(|k| !k.trim().is_empty())
        }
        ApiProvider::Siliconflow => {
            std::env::var("SILICONFLOW_API_KEY").is_ok_and(|k| !k.trim().is_empty())
        }
        ApiProvider::Moonshot => {
            std::env::var("MOONSHOT_API_KEY").is_ok_and(|k| !k.trim().is_empty())
                || std::env::var("KIMI_API_KEY").is_ok_and(|k| !k.trim().is_empty())
        }
        ApiProvider::Sglang => std::env::var("SGLANG_API_KEY").is_ok_and(|k| !k.trim().is_empty()),
        ApiProvider::Vllm => std::env::var("VLLM_API_KEY").is_ok_and(|k| !k.trim().is_empty()),
        ApiProvider::Ollama => std::env::var("OLLAMA_API_KEY").is_ok_and(|k| !k.trim().is_empty()),
        ApiProvider::Volcengine => {
            std::env::var("VOLCENGINE_API_KEY").is_ok_and(|k| !k.trim().is_empty())
                || std::env::var("VOLCENGINE_ARK_API_KEY").is_ok_and(|k| !k.trim().is_empty())
                || std::env::var("ARK_API_KEY").is_ok_and(|k| !k.trim().is_empty())
        }
    }
}

#[must_use]
pub fn active_provider_uses_env_only_api_key(config: &Config) -> bool {
    active_provider_has_env_api_key(config) && !active_provider_has_config_api_key(config)
}

/// Check whether the given provider has any usable API key — via env var,
/// provider/root config. Used by the `/provider` picker to decide whether to
/// prompt for a key inline.
#[must_use]
pub fn has_api_key_for(config: &Config, provider: ApiProvider) -> bool {
    let env_var = match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => "DEEPSEEK_API_KEY",
        ApiProvider::NvidiaNim => "NVIDIA_API_KEY",
        ApiProvider::Openai => "OPENAI_API_KEY",
        ApiProvider::Atlascloud => "ATLASCLOUD_API_KEY",
        ApiProvider::WanjieArk => "WANJIE_ARK_API_KEY",
        ApiProvider::Openrouter => "OPENROUTER_API_KEY",
        ApiProvider::XiaomiMimo => "XIAOMI_MIMO_API_KEY",
        ApiProvider::Novita => "NOVITA_API_KEY",
        ApiProvider::Fireworks => "FIREWORKS_API_KEY",
        ApiProvider::Siliconflow => "SILICONFLOW_API_KEY",
        ApiProvider::Moonshot => "MOONSHOT_API_KEY",
        ApiProvider::Sglang => "SGLANG_API_KEY",
        ApiProvider::Vllm => "VLLM_API_KEY",
        ApiProvider::Ollama => "OLLAMA_API_KEY",
        ApiProvider::Volcengine => "VOLCENGINE_API_KEY",
    };
    if std::env::var(env_var).is_ok_and(|k| !k.trim().is_empty()) {
        return true;
    }
    if matches!(provider, ApiProvider::NvidiaNim)
        && std::env::var("NVIDIA_NIM_API_KEY").is_ok_and(|k| !k.trim().is_empty())
    {
        return true;
    }
    if matches!(provider, ApiProvider::WanjieArk)
        && (std::env::var("WANJIE_API_KEY").is_ok_and(|k| !k.trim().is_empty())
            || std::env::var("WANJIE_MAAS_API_KEY").is_ok_and(|k| !k.trim().is_empty()))
    {
        return true;
    }
    if matches!(provider, ApiProvider::XiaomiMimo)
        && std::env::var("MIMO_API_KEY").is_ok_and(|k| !k.trim().is_empty())
    {
        return true;
    }
    if matches!(provider, ApiProvider::Moonshot)
        && std::env::var("KIMI_API_KEY").is_ok_and(|k| !k.trim().is_empty())
    {
        return true;
    }

    if provider == ApiProvider::Moonshot
        && config
            .provider_config_for(provider)
            .is_some_and(provider_config_uses_kimi_oauth)
    {
        return kimi_cli_credentials_present();
    }

    // Self-hosted providers typically run without authentication.
    if matches!(
        provider,
        ApiProvider::Sglang | ApiProvider::Vllm | ApiProvider::Ollama
    ) {
        return true;
    }

    if provider == config.api_provider() && base_url_uses_local_host(&config.deepseek_base_url()) {
        return true;
    }

    if config
        .provider_config_for(provider)
        .and_then(|entry| entry.api_key.as_ref())
        .is_some_and(|k| !k.trim().is_empty() && k != API_KEYRING_SENTINEL)
    {
        return true;
    }

    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        && config
            .api_key
            .as_ref()
            .is_some_and(|k| !k.trim().is_empty() && k != API_KEYRING_SENTINEL)
    {
        return true;
    }

    false
}

/// Save an API key to the appropriate place for the given provider.
/// DeepSeek goes through [`save_api_key`]. Other providers write
/// `[providers.<name>] api_key = "..."` to `~/.deepseek/config.toml`.
/// Returns the config file path.
pub fn save_api_key_for(provider: ApiProvider, api_key: &str) -> Result<PathBuf> {
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        return match save_api_key(api_key)? {
            SavedCredential::KeyringAndConfigFile { path, .. }
            | SavedCredential::ConfigFile(path) => Ok(path),
        };
    }

    let config_path = default_config_path()
        .context("Failed to resolve config path: home directory not found.")?;
    ensure_parent_dir(&config_path)?;

    let table_name = match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => {
            return Err(anyhow::anyhow!(
                "save_api_key_for: DeepSeek variants must use the root api_key field, not provider-specific storage"
            ));
        }
        ApiProvider::NvidiaNim => "providers.nvidia_nim",
        ApiProvider::Openai => "providers.openai",
        ApiProvider::Atlascloud => "providers.atlascloud",
        ApiProvider::WanjieArk => "providers.wanjie_ark",
        ApiProvider::Openrouter => "providers.openrouter",
        ApiProvider::XiaomiMimo => "providers.xiaomi_mimo",
        ApiProvider::Novita => "providers.novita",
        ApiProvider::Fireworks => "providers.fireworks",
        ApiProvider::Siliconflow => "providers.siliconflow",
        ApiProvider::Moonshot => "providers.moonshot",
        ApiProvider::Sglang => "providers.sglang",
        ApiProvider::Vllm => "providers.vllm",
        ApiProvider::Ollama => "providers.ollama",
        ApiProvider::Volcengine => "providers.volcengine",
    };

    // Parse existing TOML (or start fresh) so we can edit the right table
    // without disturbing other sections.
    let mut doc: toml::Value = if config_path.exists() {
        let raw = fs::read_to_string(&config_path)?;
        toml::from_str(&raw)
            .with_context(|| format!("Failed to parse config at {}", config_path.display()))?
    } else {
        toml::Value::Table(toml::value::Table::new())
    };

    let table = doc
        .as_table_mut()
        .context("Config root must be a TOML table.")?;
    let providers = table
        .entry("providers".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()))
        .as_table_mut()
        .context("`providers` must be a table.")?;
    let key_inside = match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => {
            return Err(anyhow::anyhow!(
                "save_api_key_for: DeepSeek variants must use the root api_key field, not provider-specific storage"
            ));
        }
        ApiProvider::NvidiaNim => "nvidia_nim",
        ApiProvider::Openai => "openai",
        ApiProvider::Atlascloud => "atlascloud",
        ApiProvider::WanjieArk => "wanjie_ark",
        ApiProvider::Openrouter => "openrouter",
        ApiProvider::XiaomiMimo => "xiaomi_mimo",
        ApiProvider::Novita => "novita",
        ApiProvider::Fireworks => "fireworks",
        ApiProvider::Siliconflow => "siliconflow",
        ApiProvider::Moonshot => "moonshot",
        ApiProvider::Sglang => "sglang",
        ApiProvider::Vllm => "vllm",
        ApiProvider::Ollama => "ollama",
        ApiProvider::Volcengine => "volcengine",
    };
    let entry = providers
        .entry(key_inside.to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()))
        .as_table_mut()
        .with_context(|| format!("`{table_name}` must be a table."))?;
    entry.insert(
        "api_key".to_string(),
        toml::Value::String(api_key.to_string()),
    );

    let serialized = toml::to_string_pretty(&doc).context("failed to serialize updated config")?;
    write_config_file_secure(&config_path, &serialized)
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    log_sensitive_event(
        "credential.save",
        json!({
            "backend": "config_file",
            "provider": provider.as_str(),
            "config_path": config_path.display().to_string(),
        }),
    );

    Ok(config_path)
}

pub fn save_provider_auth_mode_for(provider: ApiProvider, auth_mode: &str) -> Result<PathBuf> {
    let config_path = default_config_path()
        .context("Failed to resolve config path: home directory not found.")?;
    ensure_parent_dir(&config_path)?;

    let mut doc: toml::Value = if config_path.exists() {
        let raw = fs::read_to_string(&config_path)?;
        toml::from_str(&raw)
            .with_context(|| format!("Failed to parse config at {}", config_path.display()))?
    } else {
        toml::Value::Table(toml::value::Table::new())
    };

    let table = doc
        .as_table_mut()
        .context("Config root must be a TOML table.")?;
    let providers = table
        .entry("providers".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()))
        .as_table_mut()
        .context("`providers` must be a table.")?;
    let key_inside = provider_config_key(provider).context("provider auth mode key")?;
    let entry = providers
        .entry(key_inside.to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()))
        .as_table_mut()
        .with_context(|| format!("`providers.{key_inside}` must be a table."))?;
    entry.insert(
        "auth_mode".to_string(),
        toml::Value::String(auth_mode.to_string()),
    );

    let serialized = toml::to_string_pretty(&doc).context("failed to serialize updated config")?;
    write_config_file_secure(&config_path, &serialized)
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    log_sensitive_event(
        "credential.auth_mode.set",
        json!({
            "backend": "config_file",
            "provider": provider.as_str(),
            "auth_mode": auth_mode,
            "config_path": config_path.display().to_string(),
        }),
    );
    Ok(config_path)
}

fn provider_config_key(provider: ApiProvider) -> Result<&'static str> {
    match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => {
            anyhow::bail!("DeepSeek stores auth at the root config level")
        }
        ApiProvider::NvidiaNim => Ok("nvidia_nim"),
        ApiProvider::Openai => Ok("openai"),
        ApiProvider::Atlascloud => Ok("atlascloud"),
        ApiProvider::WanjieArk => Ok("wanjie_ark"),
        ApiProvider::Volcengine => Ok("volcengine"),
        ApiProvider::Openrouter => Ok("openrouter"),
        ApiProvider::XiaomiMimo => Ok("xiaomi_mimo"),
        ApiProvider::Novita => Ok("novita"),
        ApiProvider::Fireworks => Ok("fireworks"),
        ApiProvider::Siliconflow => Ok("siliconflow"),
        ApiProvider::Moonshot => Ok("moonshot"),
        ApiProvider::Sglang => Ok("sglang"),
        ApiProvider::Vllm => Ok("vllm"),
        ApiProvider::Ollama => Ok("ollama"),
    }
}

const KIMI_CODE_CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const KIMI_CODE_CREDENTIAL_FILE: &str = "kimi-code.json";

#[derive(Debug, Clone, Deserialize, Serialize)]
struct KimiOAuthCredential {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_at: Option<f64>,
    expires_in: Option<f64>,
    scope: Option<String>,
    token_type: Option<String>,
}

fn kimi_cli_oauth_access_token() -> Result<String> {
    let path = kimi_cli_oauth_credentials_path()?;
    let raw = fs::read_to_string(&path).with_context(|| {
        format!(
            "Kimi OAuth credentials not found at {}. Run `kimi login`, then set \
             [providers.moonshot] auth_mode = \"kimi_oauth\".",
            path.display()
        )
    })?;
    let mut credential: KimiOAuthCredential =
        serde_json::from_str(&raw).context("Failed to parse Kimi OAuth credentials")?;

    if kimi_oauth_access_token_is_fresh(&credential) {
        return credential
            .access_token
            .filter(|token| !token.trim().is_empty())
            .context("Kimi OAuth access token is empty");
    }

    let refresh_token = credential
        .refresh_token
        .as_deref()
        .filter(|token| !token.trim().is_empty())
        .context("Kimi OAuth refresh token is empty. Run `kimi login` again.")?;
    credential = refresh_kimi_oauth_token(refresh_token)?;
    write_kimi_oauth_credential(&path, &credential)?;
    credential
        .access_token
        .filter(|token| !token.trim().is_empty())
        .context("Kimi OAuth refresh returned an empty access token")
}

fn kimi_oauth_access_token_is_fresh(credential: &KimiOAuthCredential) -> bool {
    let Some(now) = now_unix_secs() else {
        return false;
    };

    credential
        .access_token
        .as_deref()
        .is_some_and(|token| !token.trim().is_empty())
        && credential
            .expires_at
            .is_some_and(|expires_at| expires_at - now > 60.0)
}

fn refresh_kimi_oauth_token(refresh_token: &str) -> Result<KimiOAuthCredential> {
    let oauth_host = std::env::var("KIMI_CODE_OAUTH_HOST")
        .or_else(|_| std::env::var("KIMI_OAUTH_HOST"))
        .unwrap_or_else(|_| "https://auth.kimi.com".to_string());
    let url = format!("{}/api/oauth/token", oauth_host.trim_end_matches('/'));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("Failed to build Kimi OAuth refresh client")?;
    let params = [
        ("client_id", KIMI_CODE_CLIENT_ID),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];
    let response = client
        .post(url)
        .header("X-Msh-Platform", "kimi_cli")
        .header("X-Msh-Version", env!("CARGO_PKG_VERSION"))
        .form(&params)
        .send()
        .context("Kimi OAuth refresh request failed")?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("Kimi OAuth refresh failed with HTTP {status}. Run `kimi login` again.");
    }

    let mut refreshed: KimiOAuthCredential = response
        .json()
        .context("Failed to parse Kimi OAuth refresh response")?;
    if let Some(expires_in) = refreshed.expires_in
        && let Some(now) = now_unix_secs()
    {
        refreshed.expires_at = Some(now + expires_in);
    }
    Ok(refreshed)
}

fn kimi_cli_oauth_credentials_path() -> Result<PathBuf> {
    let share_dir = std::env::var("KIMI_SHARE_DIR")
        .map(PathBuf::from)
        .or_else(|_| {
            effective_home_dir()
                .map(|home| home.join(".kimi"))
                .ok_or(std::env::VarError::NotPresent)
        })
        .context("Failed to resolve Kimi share directory")?;
    Ok(share_dir
        .join("credentials")
        .join(KIMI_CODE_CREDENTIAL_FILE))
}

fn write_kimi_oauth_credential(path: &Path, credential: &KimiOAuthCredential) -> Result<()> {
    let serialized = serde_json::to_vec_pretty(credential)
        .context("Failed to serialize Kimi OAuth credentials")?;
    crate::utils::write_atomic(path, &serialized).with_context(|| {
        format!(
            "Failed to write Kimi OAuth credentials to {}",
            path.display()
        )
    })?;
    #[cfg(unix)]
    if let Err(err) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        tracing::warn!(
            target: "codewhale::config",
            path = %path.display(),
            error = %err,
            "could not enforce 0o600 on Kimi OAuth credentials; relying on host ACLs"
        );
    }
    Ok(())
}

fn now_unix_secs() -> Option<f64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .ok()
}

#[must_use]
pub fn kimi_cli_credentials_present() -> bool {
    kimi_cli_oauth_credentials_path().is_ok_and(|path| path.exists())
}

/// Clear the API key from config-file storage.
///
/// `/logout` calls this to wipe credentials so the next request can't
/// silently use a stale config key (#343). The function strips the legacy
/// root `api_key = ...` line *and* every `api_key` line nested in a
/// `[providers.<name>]` table.
///
/// Environment variables (`DEEPSEEK_API_KEY`, etc.) are intentionally
/// **not** unset — they are managed by the user's shell and outside the
/// CLI's purview. `Config::deepseek_api_key`'s explicit-override path
/// (Path 0) ensures a freshly-entered key still wins over a stale env
/// var that lingers from a previous session.
pub fn clear_api_key() -> Result<()> {
    // Strip api_key lines from config.toml, including provider-scoped nested
    // entries. Clearing a config file must not trigger platform credential
    // prompts.
    let config_path = default_config_path()
        .context("Failed to resolve config path: home directory not found.")?;

    if !config_path.exists() {
        return Ok(());
    }

    let existing = fs::read_to_string(&config_path)?;
    let mut result = String::new();

    for line in existing.lines() {
        // Match `api_key`, `api_key =`, `  api_key=`, etc. — anywhere it
        // appears as the leading non-whitespace token.
        let trimmed = line.trim_start();
        if trimmed.strip_prefix("api_key").is_some_and(|rest| {
            let rest = rest.trim_start();
            rest.is_empty() || rest.starts_with('=')
        }) {
            continue;
        }
        result.push_str(line);
        result.push('\n');
    }

    write_config_file_secure(&config_path, &result)
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    log_sensitive_event(
        "credential.clear",
        json!({
            "backend": "config_file",
            "config_path": config_path.display().to_string(),
            "scope": "root_and_provider_keys",
        }),
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::lock_test_env;
    use std::collections::HashMap;
    use std::env;
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    // GHSA-72w5-pf8h-xfp4 — regression: `allow_shell` must be opt-in.
    #[test]
    fn allow_shell_defaults_to_false_when_unset() {
        let config = Config::default();
        assert_eq!(config.allow_shell, None, "default Config has no opt-in set");
        assert!(
            !config.allow_shell(),
            "Config::allow_shell() must default to false when no opt-in is recorded"
        );
    }

    #[test]
    fn network_policy_toml_maps_proxy_hosts_to_runtime_policy() {
        let policy: NetworkPolicyToml = toml::from_str(
            r#"
            default = "allow"
            proxy = ["github.com", ".githubusercontent.com"]
            "#,
        )
        .expect("network policy toml");

        let runtime = policy.into_runtime();

        assert_eq!(runtime.proxy, ["github.com", ".githubusercontent.com"]);
        assert!(runtime.trusts_proxy_fakeip_host("github.com"));
        assert!(runtime.trusts_proxy_fakeip_host("raw.githubusercontent.com"));
    }

    #[test]
    fn search_provider_defaults_to_duckduckgo() {
        assert_eq!(SearchProvider::default(), SearchProvider::DuckDuckGo);
    }

    #[test]
    fn tools_always_load_parses_and_trims_names() {
        let parsed: ConfigFile = toml::from_str(
            r#"
            [tools]
            always_load = ["git_show", " notify ", ""]
            "#,
        )
        .expect("tools config");

        let names = parsed.base.tools_always_load();

        assert!(names.contains("git_show"));
        assert!(names.contains("notify"));
        assert!(!names.contains(""));
    }

    #[test]
    fn explicit_duckduckgo_search_provider_is_preserved() {
        let config: Config = toml::from_str(
            r#"
            [search]
            provider = "duckduckgo"
            "#,
        )
        .expect("search config");

        assert_eq!(
            config.search.and_then(|search| search.provider),
            Some(SearchProvider::DuckDuckGo)
        );
    }

    #[test]
    fn explicit_baidu_search_provider_is_preserved() {
        let config: Config = toml::from_str(
            r#"
            [search]
            provider = "baidu"
            "#,
        )
        .expect("search config");

        assert_eq!(
            config.search.and_then(|search| search.provider),
            Some(SearchProvider::Baidu)
        );
    }

    #[test]
    fn baidu_search_provider_aliases_parse() {
        assert_eq!(SearchProvider::parse("baidu"), Some(SearchProvider::Baidu));
        assert_eq!(
            SearchProvider::parse("baidu-search"),
            Some(SearchProvider::Baidu)
        );
        assert_eq!(
            SearchProvider::parse("baidu_ai_search"),
            Some(SearchProvider::Baidu)
        );
    }

    #[test]
    fn search_provider_resolution_reports_default_source() {
        let _guard = lock_test_env();
        let prev = env::var_os("DEEPSEEK_SEARCH_PROVIDER");
        unsafe { env::remove_var("DEEPSEEK_SEARCH_PROVIDER") };

        let resolution = Config::default().search_provider_resolution();

        unsafe { EnvGuard::restore_var("DEEPSEEK_SEARCH_PROVIDER", prev) };
        assert_eq!(resolution.provider, SearchProvider::DuckDuckGo);
        assert_eq!(resolution.source, SearchProviderSource::Default);
    }

    #[test]
    fn search_provider_resolution_reports_config_source() {
        let _guard = lock_test_env();
        let prev = env::var_os("DEEPSEEK_SEARCH_PROVIDER");
        unsafe { env::remove_var("DEEPSEEK_SEARCH_PROVIDER") };
        let config: Config = toml::from_str(
            r#"
            [search]
            provider = "tavily"
            "#,
        )
        .expect("search config");

        let resolution = config.search_provider_resolution();

        unsafe { EnvGuard::restore_var("DEEPSEEK_SEARCH_PROVIDER", prev) };
        assert_eq!(resolution.provider, SearchProvider::Tavily);
        assert_eq!(resolution.source, SearchProviderSource::Config);
    }

    #[test]
    fn search_provider_resolution_reports_env_override_source() {
        let _guard = lock_test_env();
        let prev = env::var_os("DEEPSEEK_SEARCH_PROVIDER");
        unsafe { env::set_var("DEEPSEEK_SEARCH_PROVIDER", "bocha") };
        let config: Config = toml::from_str(
            r#"
            [search]
            provider = "duckduckgo"
            "#,
        )
        .expect("search config");

        let resolution = config.search_provider_resolution();

        unsafe { EnvGuard::restore_var("DEEPSEEK_SEARCH_PROVIDER", prev) };
        assert_eq!(resolution.provider, SearchProvider::Bocha);
        assert_eq!(resolution.source, SearchProviderSource::EnvOverride);
    }

    #[test]
    fn search_provider_env_override_accepts_baidu() {
        let _guard = lock_test_env();
        let prev = env::var_os("DEEPSEEK_SEARCH_PROVIDER");
        unsafe { env::set_var("DEEPSEEK_SEARCH_PROVIDER", "baidu") };
        let config: Config = toml::from_str(
            r#"
            [search]
            provider = "duckduckgo"
            "#,
        )
        .expect("search config");

        let resolution = config.search_provider_resolution();

        unsafe { EnvGuard::restore_var("DEEPSEEK_SEARCH_PROVIDER", prev) };
        assert_eq!(resolution.provider, SearchProvider::Baidu);
        assert_eq!(resolution.source, SearchProviderSource::EnvOverride);
    }

    #[test]
    fn apply_env_overrides_sets_search_api_key() {
        let _guard = lock_test_env();
        let prev = env::var_os("DEEPSEEK_SEARCH_API_KEY");
        unsafe { env::set_var("DEEPSEEK_SEARCH_API_KEY", "search-env-key") };
        let mut config = Config::default();

        apply_env_overrides(&mut config);

        unsafe { EnvGuard::restore_var("DEEPSEEK_SEARCH_API_KEY", prev) };
        assert_eq!(
            config.search.and_then(|search| search.api_key),
            Some("search-env-key".to_string())
        );
    }

    #[test]
    fn search_provider_resolution_ignores_invalid_env_override() {
        let _guard = lock_test_env();
        let prev = env::var_os("DEEPSEEK_SEARCH_PROVIDER");
        unsafe { env::set_var("DEEPSEEK_SEARCH_PROVIDER", "not-a-provider") };
        let config: Config = toml::from_str(
            r#"
            [search]
            provider = "tavily"
            "#,
        )
        .expect("search config");

        let resolution = config.search_provider_resolution();

        unsafe { EnvGuard::restore_var("DEEPSEEK_SEARCH_PROVIDER", prev) };
        assert_eq!(resolution.provider, SearchProvider::Tavily);
        assert_eq!(resolution.source, SearchProviderSource::Config);
    }

    struct EnvGuard {
        home: Option<OsString>,
        userprofile: Option<OsString>,
        deepseek_config_path: Option<OsString>,
        deepseek_provider: Option<OsString>,
        deepseek_api_key: Option<OsString>,
        deepseek_base_url: Option<OsString>,
        deepseek_http_headers: Option<OsString>,
        deepseek_model: Option<OsString>,
        deepseek_default_text_model: Option<OsString>,
        codewhale_provider: Option<OsString>,
        codewhale_model: Option<OsString>,
        codewhale_base_url: Option<OsString>,
        nvidia_api_key: Option<OsString>,
        nvidia_nim_api_key: Option<OsString>,
        nim_base_url: Option<OsString>,
        nvidia_base_url: Option<OsString>,
        nvidia_nim_base_url: Option<OsString>,
        nvidia_nim_model: Option<OsString>,
        openai_api_key: Option<OsString>,
        openai_base_url: Option<OsString>,
        openai_model: Option<OsString>,
        atlascloud_api_key: Option<OsString>,
        atlascloud_base_url: Option<OsString>,
        atlascloud_model: Option<OsString>,
        wanjie_ark_api_key: Option<OsString>,
        wanjie_api_key: Option<OsString>,
        wanjie_maas_api_key: Option<OsString>,
        wanjie_ark_base_url: Option<OsString>,
        wanjie_base_url: Option<OsString>,
        wanjie_maas_base_url: Option<OsString>,
        wanjie_ark_model: Option<OsString>,
        wanjie_model: Option<OsString>,
        wanjie_maas_model: Option<OsString>,
        openrouter_api_key: Option<OsString>,
        openrouter_base_url: Option<OsString>,
        xiaomi_mimo_api_key: Option<OsString>,
        mimo_api_key: Option<OsString>,
        xiaomi_mimo_base_url: Option<OsString>,
        mimo_base_url: Option<OsString>,
        xiaomi_mimo_model: Option<OsString>,
        mimo_model: Option<OsString>,
        novita_api_key: Option<OsString>,
        novita_base_url: Option<OsString>,
        fireworks_api_key: Option<OsString>,
        fireworks_base_url: Option<OsString>,
        siliconflow_api_key: Option<OsString>,
        siliconflow_base_url: Option<OsString>,
        siliconflow_model: Option<OsString>,
        moonshot_api_key: Option<OsString>,
        moonshot_base_url: Option<OsString>,
        moonshot_model: Option<OsString>,
        kimi_api_key: Option<OsString>,
        kimi_base_url: Option<OsString>,
        kimi_model: Option<OsString>,
        kimi_model_name: Option<OsString>,
        kimi_share_dir: Option<OsString>,
        kimi_code_oauth_host: Option<OsString>,
        kimi_oauth_host: Option<OsString>,
        sglang_api_key: Option<OsString>,
        sglang_base_url: Option<OsString>,
        sglang_model: Option<OsString>,
        vllm_api_key: Option<OsString>,
        vllm_base_url: Option<OsString>,
        vllm_model: Option<OsString>,
        ollama_api_key: Option<OsString>,
        ollama_base_url: Option<OsString>,
        ollama_model: Option<OsString>,
    }

    impl EnvGuard {
        fn new(home: &Path) -> Self {
            let home_str = OsString::from(home.as_os_str());
            let config_path = home.join(".deepseek").join("config.toml");
            let config_str = OsString::from(config_path.as_os_str());
            let home_prev = env::var_os("HOME");
            let userprofile_prev = env::var_os("USERPROFILE");
            let deepseek_config_prev = env::var_os("DEEPSEEK_CONFIG_PATH");
            let deepseek_provider_prev = env::var_os("DEEPSEEK_PROVIDER");
            let api_key_prev = env::var_os("DEEPSEEK_API_KEY");
            let base_url_prev = env::var_os("DEEPSEEK_BASE_URL");
            let http_headers_prev = env::var_os("DEEPSEEK_HTTP_HEADERS");
            let model_prev = env::var_os("DEEPSEEK_MODEL");
            let default_text_model_prev = env::var_os("DEEPSEEK_DEFAULT_TEXT_MODEL");
            let codewhale_provider_prev = env::var_os("CODEWHALE_PROVIDER");
            let codewhale_model_prev = env::var_os("CODEWHALE_MODEL");
            let codewhale_base_url_prev = env::var_os("CODEWHALE_BASE_URL");
            let nvidia_api_key_prev = env::var_os("NVIDIA_API_KEY");
            let nvidia_nim_api_key_prev = env::var_os("NVIDIA_NIM_API_KEY");
            let nim_base_url_prev = env::var_os("NIM_BASE_URL");
            let nvidia_base_url_prev = env::var_os("NVIDIA_BASE_URL");
            let nvidia_nim_base_url_prev = env::var_os("NVIDIA_NIM_BASE_URL");
            let nvidia_nim_model_prev = env::var_os("NVIDIA_NIM_MODEL");
            let openai_api_key_prev = env::var_os("OPENAI_API_KEY");
            let openai_base_url_prev = env::var_os("OPENAI_BASE_URL");
            let openai_model_prev = env::var_os("OPENAI_MODEL");
            let atlascloud_api_key_prev = env::var_os("ATLASCLOUD_API_KEY");
            let atlascloud_base_url_prev = env::var_os("ATLASCLOUD_BASE_URL");
            let atlascloud_model_prev = env::var_os("ATLASCLOUD_MODEL");
            let wanjie_ark_api_key_prev = env::var_os("WANJIE_ARK_API_KEY");
            let wanjie_api_key_prev = env::var_os("WANJIE_API_KEY");
            let wanjie_maas_api_key_prev = env::var_os("WANJIE_MAAS_API_KEY");
            let wanjie_ark_base_url_prev = env::var_os("WANJIE_ARK_BASE_URL");
            let wanjie_base_url_prev = env::var_os("WANJIE_BASE_URL");
            let wanjie_maas_base_url_prev = env::var_os("WANJIE_MAAS_BASE_URL");
            let wanjie_ark_model_prev = env::var_os("WANJIE_ARK_MODEL");
            let wanjie_model_prev = env::var_os("WANJIE_MODEL");
            let wanjie_maas_model_prev = env::var_os("WANJIE_MAAS_MODEL");
            let openrouter_api_key_prev = env::var_os("OPENROUTER_API_KEY");
            let openrouter_base_url_prev = env::var_os("OPENROUTER_BASE_URL");
            let xiaomi_mimo_api_key_prev = env::var_os("XIAOMI_MIMO_API_KEY");
            let mimo_api_key_prev = env::var_os("MIMO_API_KEY");
            let xiaomi_mimo_base_url_prev = env::var_os("XIAOMI_MIMO_BASE_URL");
            let mimo_base_url_prev = env::var_os("MIMO_BASE_URL");
            let xiaomi_mimo_model_prev = env::var_os("XIAOMI_MIMO_MODEL");
            let mimo_model_prev = env::var_os("MIMO_MODEL");
            let novita_api_key_prev = env::var_os("NOVITA_API_KEY");
            let novita_base_url_prev = env::var_os("NOVITA_BASE_URL");
            let fireworks_api_key_prev = env::var_os("FIREWORKS_API_KEY");
            let fireworks_base_url_prev = env::var_os("FIREWORKS_BASE_URL");
            let siliconflow_api_key_prev = env::var_os("SILICONFLOW_API_KEY");
            let siliconflow_base_url_prev = env::var_os("SILICONFLOW_BASE_URL");
            let siliconflow_model_prev = env::var_os("SILICONFLOW_MODEL");
            let moonshot_api_key_prev = env::var_os("MOONSHOT_API_KEY");
            let moonshot_base_url_prev = env::var_os("MOONSHOT_BASE_URL");
            let moonshot_model_prev = env::var_os("MOONSHOT_MODEL");
            let kimi_api_key_prev = env::var_os("KIMI_API_KEY");
            let kimi_base_url_prev = env::var_os("KIMI_BASE_URL");
            let kimi_model_prev = env::var_os("KIMI_MODEL");
            let kimi_model_name_prev = env::var_os("KIMI_MODEL_NAME");
            let kimi_share_dir_prev = env::var_os("KIMI_SHARE_DIR");
            let kimi_code_oauth_host_prev = env::var_os("KIMI_CODE_OAUTH_HOST");
            let kimi_oauth_host_prev = env::var_os("KIMI_OAUTH_HOST");
            let sglang_api_key_prev = env::var_os("SGLANG_API_KEY");
            let sglang_base_url_prev = env::var_os("SGLANG_BASE_URL");
            let sglang_model_prev = env::var_os("SGLANG_MODEL");
            let vllm_api_key_prev = env::var_os("VLLM_API_KEY");
            let vllm_base_url_prev = env::var_os("VLLM_BASE_URL");
            let vllm_model_prev = env::var_os("VLLM_MODEL");
            let ollama_api_key_prev = env::var_os("OLLAMA_API_KEY");
            let ollama_base_url_prev = env::var_os("OLLAMA_BASE_URL");
            let ollama_model_prev = env::var_os("OLLAMA_MODEL");
            // Safety: test-only environment mutation guarded by a global mutex.
            unsafe {
                env::set_var("HOME", &home_str);
                env::set_var("USERPROFILE", &home_str);
                env::set_var("DEEPSEEK_CONFIG_PATH", &config_str);
                env::remove_var("DEEPSEEK_PROVIDER");
                env::remove_var("DEEPSEEK_API_KEY");
                env::remove_var("DEEPSEEK_BASE_URL");
                env::remove_var("DEEPSEEK_HTTP_HEADERS");
                env::remove_var("DEEPSEEK_MODEL");
                env::remove_var("DEEPSEEK_DEFAULT_TEXT_MODEL");
                env::remove_var("CODEWHALE_PROVIDER");
                env::remove_var("CODEWHALE_MODEL");
                env::remove_var("CODEWHALE_BASE_URL");
                env::remove_var("NVIDIA_API_KEY");
                env::remove_var("NVIDIA_NIM_API_KEY");
                env::remove_var("NIM_BASE_URL");
                env::remove_var("NVIDIA_BASE_URL");
                env::remove_var("NVIDIA_NIM_BASE_URL");
                env::remove_var("NVIDIA_NIM_MODEL");
                env::remove_var("OPENAI_API_KEY");
                env::remove_var("OPENAI_BASE_URL");
                env::remove_var("OPENAI_MODEL");
                env::remove_var("ATLASCLOUD_API_KEY");
                env::remove_var("ATLASCLOUD_BASE_URL");
                env::remove_var("ATLASCLOUD_MODEL");
                env::remove_var("WANJIE_ARK_API_KEY");
                env::remove_var("WANJIE_API_KEY");
                env::remove_var("WANJIE_MAAS_API_KEY");
                env::remove_var("WANJIE_ARK_BASE_URL");
                env::remove_var("WANJIE_BASE_URL");
                env::remove_var("WANJIE_MAAS_BASE_URL");
                env::remove_var("WANJIE_ARK_MODEL");
                env::remove_var("WANJIE_MODEL");
                env::remove_var("WANJIE_MAAS_MODEL");
                env::remove_var("OPENROUTER_API_KEY");
                env::remove_var("OPENROUTER_BASE_URL");
                env::remove_var("XIAOMI_MIMO_API_KEY");
                env::remove_var("MIMO_API_KEY");
                env::remove_var("XIAOMI_MIMO_BASE_URL");
                env::remove_var("MIMO_BASE_URL");
                env::remove_var("XIAOMI_MIMO_MODEL");
                env::remove_var("MIMO_MODEL");
                env::remove_var("NOVITA_API_KEY");
                env::remove_var("NOVITA_BASE_URL");
                env::remove_var("FIREWORKS_API_KEY");
                env::remove_var("FIREWORKS_BASE_URL");
                env::remove_var("SILICONFLOW_API_KEY");
                env::remove_var("SILICONFLOW_BASE_URL");
                env::remove_var("SILICONFLOW_MODEL");
                env::remove_var("MOONSHOT_API_KEY");
                env::remove_var("MOONSHOT_BASE_URL");
                env::remove_var("MOONSHOT_MODEL");
                env::remove_var("KIMI_API_KEY");
                env::remove_var("KIMI_BASE_URL");
                env::remove_var("KIMI_MODEL");
                env::remove_var("KIMI_MODEL_NAME");
                env::remove_var("KIMI_SHARE_DIR");
                env::remove_var("KIMI_CODE_OAUTH_HOST");
                env::remove_var("KIMI_OAUTH_HOST");
                env::remove_var("SGLANG_API_KEY");
                env::remove_var("SGLANG_BASE_URL");
                env::remove_var("SGLANG_MODEL");
                env::remove_var("VLLM_API_KEY");
                env::remove_var("VLLM_BASE_URL");
                env::remove_var("VLLM_MODEL");
                env::remove_var("OLLAMA_API_KEY");
                env::remove_var("OLLAMA_BASE_URL");
                env::remove_var("OLLAMA_MODEL");
            }
            Self {
                home: home_prev,
                userprofile: userprofile_prev,
                deepseek_config_path: deepseek_config_prev,
                deepseek_provider: deepseek_provider_prev,
                deepseek_api_key: api_key_prev,
                deepseek_base_url: base_url_prev,
                deepseek_http_headers: http_headers_prev,
                deepseek_model: model_prev,
                deepseek_default_text_model: default_text_model_prev,
                codewhale_provider: codewhale_provider_prev,
                codewhale_model: codewhale_model_prev,
                codewhale_base_url: codewhale_base_url_prev,
                nvidia_api_key: nvidia_api_key_prev,
                nvidia_nim_api_key: nvidia_nim_api_key_prev,
                nim_base_url: nim_base_url_prev,
                nvidia_base_url: nvidia_base_url_prev,
                nvidia_nim_base_url: nvidia_nim_base_url_prev,
                nvidia_nim_model: nvidia_nim_model_prev,
                openai_api_key: openai_api_key_prev,
                openai_base_url: openai_base_url_prev,
                openai_model: openai_model_prev,
                atlascloud_api_key: atlascloud_api_key_prev,
                atlascloud_base_url: atlascloud_base_url_prev,
                atlascloud_model: atlascloud_model_prev,
                wanjie_ark_api_key: wanjie_ark_api_key_prev,
                wanjie_api_key: wanjie_api_key_prev,
                wanjie_maas_api_key: wanjie_maas_api_key_prev,
                wanjie_ark_base_url: wanjie_ark_base_url_prev,
                wanjie_base_url: wanjie_base_url_prev,
                wanjie_maas_base_url: wanjie_maas_base_url_prev,
                wanjie_ark_model: wanjie_ark_model_prev,
                wanjie_model: wanjie_model_prev,
                wanjie_maas_model: wanjie_maas_model_prev,
                openrouter_api_key: openrouter_api_key_prev,
                openrouter_base_url: openrouter_base_url_prev,
                xiaomi_mimo_api_key: xiaomi_mimo_api_key_prev,
                mimo_api_key: mimo_api_key_prev,
                xiaomi_mimo_base_url: xiaomi_mimo_base_url_prev,
                mimo_base_url: mimo_base_url_prev,
                xiaomi_mimo_model: xiaomi_mimo_model_prev,
                mimo_model: mimo_model_prev,
                novita_api_key: novita_api_key_prev,
                novita_base_url: novita_base_url_prev,
                fireworks_api_key: fireworks_api_key_prev,
                fireworks_base_url: fireworks_base_url_prev,
                siliconflow_api_key: siliconflow_api_key_prev,
                siliconflow_base_url: siliconflow_base_url_prev,
                siliconflow_model: siliconflow_model_prev,
                moonshot_api_key: moonshot_api_key_prev,
                moonshot_base_url: moonshot_base_url_prev,
                moonshot_model: moonshot_model_prev,
                kimi_api_key: kimi_api_key_prev,
                kimi_base_url: kimi_base_url_prev,
                kimi_model: kimi_model_prev,
                kimi_model_name: kimi_model_name_prev,
                kimi_share_dir: kimi_share_dir_prev,
                kimi_code_oauth_host: kimi_code_oauth_host_prev,
                kimi_oauth_host: kimi_oauth_host_prev,
                sglang_api_key: sglang_api_key_prev,
                sglang_base_url: sglang_base_url_prev,
                sglang_model: sglang_model_prev,
                vllm_api_key: vllm_api_key_prev,
                vllm_base_url: vllm_base_url_prev,
                vllm_model: vllm_model_prev,
                ollama_api_key: ollama_api_key_prev,
                ollama_base_url: ollama_base_url_prev,
                ollama_model: ollama_model_prev,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // Safety: test-only environment mutation guarded by a global mutex.
            unsafe {
                Self::restore_var("HOME", self.home.take());
                Self::restore_var("USERPROFILE", self.userprofile.take());
                Self::restore_var("DEEPSEEK_CONFIG_PATH", self.deepseek_config_path.take());
                Self::restore_var("DEEPSEEK_PROVIDER", self.deepseek_provider.take());
                Self::restore_var("DEEPSEEK_API_KEY", self.deepseek_api_key.take());
                Self::restore_var("DEEPSEEK_BASE_URL", self.deepseek_base_url.take());
                Self::restore_var("DEEPSEEK_HTTP_HEADERS", self.deepseek_http_headers.take());
                Self::restore_var("DEEPSEEK_MODEL", self.deepseek_model.take());
                Self::restore_var(
                    "DEEPSEEK_DEFAULT_TEXT_MODEL",
                    self.deepseek_default_text_model.take(),
                );
                Self::restore_var("CODEWHALE_PROVIDER", self.codewhale_provider.take());
                Self::restore_var("CODEWHALE_MODEL", self.codewhale_model.take());
                Self::restore_var("CODEWHALE_BASE_URL", self.codewhale_base_url.take());
                Self::restore_var("NVIDIA_API_KEY", self.nvidia_api_key.take());
                Self::restore_var("NVIDIA_NIM_API_KEY", self.nvidia_nim_api_key.take());
                Self::restore_var("NIM_BASE_URL", self.nim_base_url.take());
                Self::restore_var("NVIDIA_BASE_URL", self.nvidia_base_url.take());
                Self::restore_var("NVIDIA_NIM_BASE_URL", self.nvidia_nim_base_url.take());
                Self::restore_var("NVIDIA_NIM_MODEL", self.nvidia_nim_model.take());
                Self::restore_var("OPENAI_API_KEY", self.openai_api_key.take());
                Self::restore_var("OPENAI_BASE_URL", self.openai_base_url.take());
                Self::restore_var("OPENAI_MODEL", self.openai_model.take());
                Self::restore_var("ATLASCLOUD_API_KEY", self.atlascloud_api_key.take());
                Self::restore_var("ATLASCLOUD_BASE_URL", self.atlascloud_base_url.take());
                Self::restore_var("ATLASCLOUD_MODEL", self.atlascloud_model.take());
                Self::restore_var("WANJIE_ARK_API_KEY", self.wanjie_ark_api_key.take());
                Self::restore_var("WANJIE_API_KEY", self.wanjie_api_key.take());
                Self::restore_var("WANJIE_MAAS_API_KEY", self.wanjie_maas_api_key.take());
                Self::restore_var("WANJIE_ARK_BASE_URL", self.wanjie_ark_base_url.take());
                Self::restore_var("WANJIE_BASE_URL", self.wanjie_base_url.take());
                Self::restore_var("WANJIE_MAAS_BASE_URL", self.wanjie_maas_base_url.take());
                Self::restore_var("WANJIE_ARK_MODEL", self.wanjie_ark_model.take());
                Self::restore_var("WANJIE_MODEL", self.wanjie_model.take());
                Self::restore_var("WANJIE_MAAS_MODEL", self.wanjie_maas_model.take());
                Self::restore_var("OPENROUTER_API_KEY", self.openrouter_api_key.take());
                Self::restore_var("OPENROUTER_BASE_URL", self.openrouter_base_url.take());
                Self::restore_var("XIAOMI_MIMO_API_KEY", self.xiaomi_mimo_api_key.take());
                Self::restore_var("MIMO_API_KEY", self.mimo_api_key.take());
                Self::restore_var("XIAOMI_MIMO_BASE_URL", self.xiaomi_mimo_base_url.take());
                Self::restore_var("MIMO_BASE_URL", self.mimo_base_url.take());
                Self::restore_var("XIAOMI_MIMO_MODEL", self.xiaomi_mimo_model.take());
                Self::restore_var("MIMO_MODEL", self.mimo_model.take());
                Self::restore_var("NOVITA_API_KEY", self.novita_api_key.take());
                Self::restore_var("NOVITA_BASE_URL", self.novita_base_url.take());
                Self::restore_var("FIREWORKS_API_KEY", self.fireworks_api_key.take());
                Self::restore_var("FIREWORKS_BASE_URL", self.fireworks_base_url.take());
                Self::restore_var("SILICONFLOW_API_KEY", self.siliconflow_api_key.take());
                Self::restore_var("SILICONFLOW_BASE_URL", self.siliconflow_base_url.take());
                Self::restore_var("SILICONFLOW_MODEL", self.siliconflow_model.take());
                Self::restore_var("MOONSHOT_API_KEY", self.moonshot_api_key.take());
                Self::restore_var("MOONSHOT_BASE_URL", self.moonshot_base_url.take());
                Self::restore_var("MOONSHOT_MODEL", self.moonshot_model.take());
                Self::restore_var("KIMI_API_KEY", self.kimi_api_key.take());
                Self::restore_var("KIMI_BASE_URL", self.kimi_base_url.take());
                Self::restore_var("KIMI_MODEL", self.kimi_model.take());
                Self::restore_var("KIMI_MODEL_NAME", self.kimi_model_name.take());
                Self::restore_var("KIMI_SHARE_DIR", self.kimi_share_dir.take());
                Self::restore_var("KIMI_CODE_OAUTH_HOST", self.kimi_code_oauth_host.take());
                Self::restore_var("KIMI_OAUTH_HOST", self.kimi_oauth_host.take());
                Self::restore_var("SGLANG_API_KEY", self.sglang_api_key.take());
                Self::restore_var("SGLANG_BASE_URL", self.sglang_base_url.take());
                Self::restore_var("SGLANG_MODEL", self.sglang_model.take());
                Self::restore_var("VLLM_API_KEY", self.vllm_api_key.take());
                Self::restore_var("VLLM_BASE_URL", self.vllm_base_url.take());
                Self::restore_var("VLLM_MODEL", self.vllm_model.take());
                Self::restore_var("OLLAMA_API_KEY", self.ollama_api_key.take());
                Self::restore_var("OLLAMA_BASE_URL", self.ollama_base_url.take());
                Self::restore_var("OLLAMA_MODEL", self.ollama_model.take());
            }
        }
    }

    impl EnvGuard {
        /// Restore an env var to its prior value (or remove it if it was unset).
        ///
        /// # Safety
        /// Must only be called from test code guarded by a global mutex.
        unsafe fn restore_var(key: &str, prev: Option<OsString>) {
            if let Some(value) = prev {
                unsafe { env::set_var(key, value) };
            } else {
                unsafe { env::remove_var(key) };
            }
        }
    }

    #[test]
    fn max_subagents_defaults_to_ten() {
        assert_eq!(Config::default().max_subagents(), DEFAULT_MAX_SUBAGENTS);
        assert_eq!(DEFAULT_MAX_SUBAGENTS, 10);
    }

    #[test]
    fn subagents_max_concurrent_overrides_top_level_cap() {
        let config = Config {
            max_subagents: Some(3),
            subagents: Some(SubagentsConfig {
                max_concurrent: Some(12),
                ..SubagentsConfig::default()
            }),
            ..Config::default()
        };

        assert_eq!(config.max_subagents(), 12);
    }

    #[test]
    fn max_subagents_clamps_subagents_max_concurrent() {
        let low = Config {
            subagents: Some(SubagentsConfig {
                max_concurrent: Some(0),
                ..SubagentsConfig::default()
            }),
            ..Config::default()
        };
        assert_eq!(low.max_subagents(), 1);

        let high = Config {
            subagents: Some(SubagentsConfig {
                max_concurrent: Some(MAX_SUBAGENTS + 10),
                ..SubagentsConfig::default()
            }),
            ..Config::default()
        };
        assert_eq!(high.max_subagents(), MAX_SUBAGENTS);
    }

    #[test]
    fn subagent_api_timeout_defaults_and_clamps() {
        assert_eq!(
            Config::default().subagent_api_timeout_secs(),
            DEFAULT_SUBAGENT_API_TIMEOUT_SECS
        );

        let zero = Config {
            subagents: Some(SubagentsConfig {
                api_timeout_secs: Some(0),
                ..SubagentsConfig::default()
            }),
            ..Config::default()
        };
        assert_eq!(
            zero.subagent_api_timeout_secs(),
            DEFAULT_SUBAGENT_API_TIMEOUT_SECS
        );

        let explicit_min = Config {
            subagents: Some(SubagentsConfig {
                api_timeout_secs: Some(MIN_SUBAGENT_API_TIMEOUT_SECS),
                ..SubagentsConfig::default()
            }),
            ..Config::default()
        };
        assert_eq!(explicit_min.subagent_api_timeout_secs(), 1);

        let high = Config {
            subagents: Some(SubagentsConfig {
                api_timeout_secs: Some(MAX_SUBAGENT_API_TIMEOUT_SECS + 60),
                ..SubagentsConfig::default()
            }),
            ..Config::default()
        };
        assert_eq!(
            high.subagent_api_timeout_secs(),
            MAX_SUBAGENT_API_TIMEOUT_SECS
        );
    }

    #[test]
    fn config_loads_typed_permission_rules_for_exec_policy_engine() -> Result<()> {
        let _guard = lock_test_env();
        let temp = tempfile::tempdir()?;
        let _env = EnvGuard::new(temp.path());
        let config_path = temp.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
auto_allow = ["git status"]
auto_deny = ["rm -rf"]

[[permissions.rules]]
tool = "read_file"
decision = "ask"
path = "secrets/**"
"#,
        )?;

        let config = Config::load(Some(config_path), None)?;
        assert_eq!(config.auto_allow, ["git status"]);
        assert_eq!(config.auto_deny, ["rm -rf"]);

        let decision = config.exec_policy_engine().check_tool_permission(
            codewhale_execpolicy::ToolPermissionContext {
                tool: "read_file",
                command: None,
                path: Some("secrets/token.txt"),
                workspace_root: None,
            },
        );

        assert_eq!(
            decision.decision,
            Some(codewhale_execpolicy::PermissionDecision::Ask)
        );
        Ok(())
    }

    #[test]
    fn merge_permissions_config_appends_rules_from_override() {
        let base = PermissionsConfig {
            rules: vec![ToolPermissionRule::file_path(
                "read_file",
                codewhale_execpolicy::PermissionDecision::Deny,
                "secrets/**",
            )],
        };
        let override_cfg = PermissionsConfig {
            rules: vec![ToolPermissionRule::file_path(
                "write_file",
                codewhale_execpolicy::PermissionDecision::Ask,
                "src/**",
            )],
        };

        let merged = merge_permissions_config(base, override_cfg);

        assert_eq!(merged.rules.len(), 2);
        assert_eq!(
            merged.rules[0],
            ToolPermissionRule::file_path(
                "read_file",
                codewhale_execpolicy::PermissionDecision::Deny,
                "secrets/**",
            )
        );
        assert_eq!(
            merged.rules[1],
            ToolPermissionRule::file_path(
                "write_file",
                codewhale_execpolicy::PermissionDecision::Ask,
                "src/**",
            )
        );
    }

    #[test]
    fn config_loads_sibling_permissions_toml_for_exec_policy_engine() -> Result<()> {
        let _guard = lock_test_env();
        let temp = tempfile::tempdir()?;
        let _env = EnvGuard::new(temp.path());
        let config_path = temp.path().join("config.toml");
        fs::write(&config_path, "auto_allow = [\"git status\"]\n")?;
        fs::write(
            temp.path().join("permissions.toml"),
            r#"
[[rules]]
tool = "read_file"
decision = "ask"
path = "secrets/**"
"#,
        )?;

        let config = Config::load(Some(config_path), None)?;
        assert_eq!(config.auto_allow, ["git status"]);

        let decision = config.exec_policy_engine().check_tool_permission(
            codewhale_execpolicy::ToolPermissionContext {
                tool: "read_file",
                command: None,
                path: Some("secrets/token.txt"),
                workspace_root: None,
            },
        );

        assert_eq!(
            decision.decision,
            Some(codewhale_execpolicy::PermissionDecision::Ask)
        );
        Ok(())
    }

    #[test]
    fn save_api_key_writes_config_file_under_cfg_test() -> Result<()> {
        // `save_api_key` writes to the shared user config file. This
        // pins the boring v0.8.8 setup path and avoids platform
        // credential prompts during onboarding.
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let saved = save_api_key("test-key")?;
        let expected = temp_root.join(".deepseek").join("config.toml");
        assert_eq!(saved, SavedCredential::ConfigFile(expected.clone()));
        assert_eq!(saved.describe(), expected.display().to_string());

        let contents = fs::read_to_string(&expected)?;
        assert!(contents.contains("api_key = \""));

        #[cfg(unix)]
        {
            assert_eq!(fs::metadata(&expected)?.permissions().mode() & 0o777, 0o600);
            let parent = expected.parent().expect("config has parent dir");
            assert_eq!(fs::metadata(parent)?.permissions().mode() & 0o077, 0);

            fs::set_permissions(&expected, fs::Permissions::from_mode(0o644))?;
            save_api_key("second-test-key")?;
            assert_eq!(fs::metadata(&expected)?.permissions().mode() & 0o777, 0o600);
        }
        Ok(())
    }

    #[test]
    fn ensure_config_file_exists_creates_first_run_template() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-first-run-config-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let created = ensure_config_file_exists(None)?.expect("should create config");
        let content = fs::read_to_string(&created)?;

        assert_eq!(created, temp_root.join(".deepseek").join("config.toml"));
        assert!(content.contains("default_text_model = \"deepseek-v4-pro\""));
        assert!(content.contains("reasoning_effort = \"auto\""));
        assert!(!content.contains("api_key ="));
        assert!(ensure_config_file_exists(None)?.is_none());
        Ok(())
    }

    #[test]
    fn workspace_trust_round_trips_through_global_config() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-workspace-trust-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);
        let workspace = temp_root.join("project");
        fs::create_dir_all(&workspace)?;

        assert!(!is_workspace_trusted(&workspace));
        let saved = save_workspace_trust(&workspace)?;

        assert_eq!(saved, temp_root.join(".deepseek").join("config.toml"));
        assert!(is_workspace_trusted(&workspace));
        assert!(!crate::tui::onboarding::needs_trust(&workspace));
        assert!(
            !workspace.join(".deepseek").exists(),
            "trust persistence must not create a project-local .deepseek directory"
        );

        let parsed: toml::Value = toml::from_str(&fs::read_to_string(saved)?)?;
        assert_eq!(
            workspace_trust_level_from_doc(&parsed, &workspace),
            Some("trusted")
        );
        Ok(())
    }

    #[test]
    fn workspace_trust_reads_existing_projects_table() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-existing-project-trust-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);
        let workspace = temp_root.join("project");
        fs::create_dir_all(&workspace)?;
        let config_path = temp_root.join(".deepseek").join("config.toml");
        fs::create_dir_all(config_path.parent().unwrap())?;
        fs::write(
            &config_path,
            format!(
                "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
                workspace_config_key(&workspace)
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
            ),
        )?;

        assert!(is_workspace_trusted(&workspace));
        assert!(!crate::tui::onboarding::needs_trust(&workspace));
        Ok(())
    }

    #[test]
    fn save_api_key_rejects_empty_input() {
        let _lock = lock_test_env();
        let err = save_api_key("   ").expect_err("empty should bail");
        assert!(
            err.to_string().contains("empty"),
            "expected error to mention empty, got: {err}"
        );
    }

    #[test]
    fn saved_credential_describe_returns_config_file_path() {
        let cf = SavedCredential::ConfigFile(PathBuf::from("/tmp/x.toml"));
        assert_eq!(cf.describe(), "/tmp/x.toml");
    }

    /// #593: the dual-write outcome describes both targets so the
    /// onboarding toast (`API key saved to {describe}`) tells the user
    /// the key landed in *both* the keyring and the config file —
    /// which is the whole point of the fix (defeats stale-keyring
    /// shadow while keeping the config file inspectable).
    #[test]
    fn saved_credential_describe_lists_both_targets_for_keyring_and_config() {
        let dual = SavedCredential::KeyringAndConfigFile {
            backend: "system keyring".to_string(),
            path: PathBuf::from("/tmp/x.toml"),
        };
        assert_eq!(
            dual.describe(),
            "OS keyring (system keyring) and /tmp/x.toml"
        );
    }

    #[test]
    fn has_api_key_detects_in_memory_override_and_env_var() -> Result<()> {
        // Pins the v0.8.8 contract: `has_api_key` covers the prompt-free
        // sources used by `Config::deepseek_api_key` (in-memory override,
        // env var, config-file slot).
        let _lock = lock_test_env();
        // Explicit in-memory key wins over every other source per
        // `Config::deepseek_api_key`'s "Path 0" override.
        let cfg = Config {
            api_key: Some("sk-in-memory-override".to_string()),
            ..Default::default()
        };
        assert!(
            has_api_key(&cfg),
            "in-memory override must be detected as a usable key"
        );

        // Env var path.
        let env_cfg = Config::default();
        unsafe {
            std::env::set_var("DEEPSEEK_API_KEY", "sk-test-from-env");
        }
        assert!(
            has_api_key(&env_cfg),
            "env-var key must be detected even with empty config"
        );
        unsafe {
            std::env::remove_var("DEEPSEEK_API_KEY");
        }
        Ok(())
    }

    fn config_with_provider_scoped_key(provider: &str, api_key: &str) -> Config {
        let mut providers = ProvidersConfig::default();
        match provider {
            "deepseek" | "deepseek-cn" => {
                providers.deepseek.api_key = Some(api_key.to_string());
            }
            "nvidia-nim" => {
                providers.nvidia_nim.api_key = Some(api_key.to_string());
            }
            "openai" => {
                providers.openai.api_key = Some(api_key.to_string());
            }
            "wanjie-ark" => {
                providers.wanjie_ark.api_key = Some(api_key.to_string());
            }
            "openrouter" => {
                providers.openrouter.api_key = Some(api_key.to_string());
            }
            "novita" => {
                providers.novita.api_key = Some(api_key.to_string());
            }
            "fireworks" => {
                providers.fireworks.api_key = Some(api_key.to_string());
            }
            "siliconflow" => {
                providers.siliconflow.api_key = Some(api_key.to_string());
            }
            "sglang" => {
                providers.sglang.api_key = Some(api_key.to_string());
            }
            "vllm" => {
                providers.vllm.api_key = Some(api_key.to_string());
            }
            "ollama" => {
                providers.ollama.api_key = Some(api_key.to_string());
            }
            _ => panic!("unexpected provider {provider}"),
        }

        Config {
            provider: Some(provider.to_string()),
            providers: Some(providers),
            ..Config::default()
        }
    }

    #[test]
    fn has_api_key_uses_active_provider_scoped_config_key() {
        for provider in [
            "openai",
            "wanjie-ark",
            "openrouter",
            "novita",
            "fireworks",
            "siliconflow",
        ] {
            let config = config_with_provider_scoped_key(provider, "provider-config-key");

            assert!(
                has_api_key(&config),
                "active provider config key must satisfy onboarding auth check for {provider}"
            );
        }
    }

    #[test]
    fn has_api_key_uses_active_provider_env_key() -> Result<()> {
        let _lock = lock_test_env();
        for (provider, env_var) in [
            ("openai", "OPENAI_API_KEY"),
            ("wanjie-ark", "WANJIE_ARK_API_KEY"),
            ("openrouter", "OPENROUTER_API_KEY"),
            ("novita", "NOVITA_API_KEY"),
            ("fireworks", "FIREWORKS_API_KEY"),
            ("siliconflow", "SILICONFLOW_API_KEY"),
        ] {
            unsafe {
                std::env::set_var(env_var, "provider-env-key");
            }

            let config = Config {
                provider: Some(provider.to_string()),
                ..Config::default()
            };

            assert!(
                has_api_key(&config),
                "active provider env key must satisfy onboarding auth check for {provider}"
            );

            unsafe {
                std::env::remove_var(env_var);
            }
        }
        Ok(())
    }

    #[test]
    fn has_api_key_uses_root_config_key_for_deepseek_variants() {
        for provider in ["deepseek", "deepseek-cn"] {
            let config = Config {
                provider: Some(provider.to_string()),
                api_key: Some("root-config-key".to_string()),
                ..Config::default()
            };

            assert!(
                has_api_key(&config),
                "root config api_key must satisfy onboarding auth check for {provider}"
            );
        }
    }

    /// Regression for #343: clear_api_key strips both the root `api_key`
    /// and any nested `[providers.<name>].api_key` lines from config.toml
    /// so a stale credential can't shadow a fresh login.
    #[test]
    fn clear_api_key_strips_root_and_provider_scoped_keys() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-clear-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_dir = temp_root.join(".deepseek");
        fs::create_dir_all(&config_dir)?;
        let config_path = config_dir.join("config.toml");
        fs::write(
            &config_path,
            r#"api_key = "old-root-key"
default_text_model = "deepseek-v4-flash"

[providers.deepseek]
api_key = "old-provider-key"
base_url = "https://api.deepseek.com"

[providers.openrouter]
api_key = "old-openrouter-key"
"#,
        )?;

        clear_api_key()?;

        let after = fs::read_to_string(&config_path)?;
        assert!(
            !after.contains("old-root-key"),
            "root api_key must be stripped: {after}"
        );
        assert!(
            !after.contains("old-provider-key"),
            "provider-scoped codewhale key must be stripped: {after}"
        );
        assert!(
            !after.contains("old-openrouter-key"),
            "provider-scoped openrouter key must be stripped: {after}"
        );
        // Non-credential lines must survive.
        assert!(after.contains("default_text_model"));
        assert!(after.contains("base_url"));
        Ok(())
    }

    /// Regression for #343: explicit in-memory `api_key` (non-empty,
    /// non-sentinel) wins over env/config so a freshly-typed onboarding
    /// key takes effect immediately.
    #[test]
    fn deepseek_api_key_prefers_explicit_in_memory_override() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-override-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config = Config {
            api_key: Some("freshly-typed-key".to_string()),
            ..Config::default()
        };
        let resolved = config
            .deepseek_api_key()
            .expect("explicit override must resolve");
        assert_eq!(resolved, "freshly-typed-key");
        Ok(())
    }

    #[test]
    fn deepseek_api_key_prefers_saved_config_over_stale_env() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-config-over-env-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        unsafe {
            env::set_var("DEEPSEEK_API_KEY", "stale-env-key");
        }
        let config = Config {
            api_key: Some("fresh-config-key".to_string()),
            ..Config::default()
        };
        assert_eq!(config.deepseek_api_key()?, "fresh-config-key");
        unsafe {
            env::remove_var("DEEPSEEK_API_KEY");
        }
        Ok(())
    }

    #[test]
    fn active_provider_detects_env_only_api_key() -> Result<()> {
        let _lock = lock_test_env();
        let temp_root =
            env::temp_dir().join(format!("codewhale-tui-env-only-key-{}", std::process::id()));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        unsafe {
            env::set_var("DEEPSEEK_API_KEY", "env-only-key");
        }
        let mut config = Config::default();
        assert!(active_provider_has_env_api_key(&config));
        assert!(!active_provider_has_config_api_key(&config));
        assert!(active_provider_uses_env_only_api_key(&config));

        config.api_key = Some("config-key".to_string());
        assert!(active_provider_has_config_api_key(&config));
        assert!(!active_provider_uses_env_only_api_key(&config));

        unsafe {
            env::remove_var("DEEPSEEK_API_KEY");
        }
        Ok(())
    }

    #[test]
    fn deepseek_api_key_ignores_sentinel_placeholder() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-sentinel-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config = Config {
            api_key: Some(API_KEYRING_SENTINEL.to_string()),
            ..Config::default()
        };
        // Sentinel must not be treated as a real key — the resolver should
        // fall through to env / config-provider and ultimately bail out
        // with a "key not found" error.
        let _err = config
            .deepseek_api_key()
            .expect_err("sentinel placeholder must not satisfy the API key check");
        Ok(())
    }

    #[test]
    fn test_tilde_expansion_in_paths() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-tilde-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config = Config {
            skills_dir: Some("~/.deepseek/skills".to_string()),
            ..Default::default()
        };
        let expected_skills = temp_root.join(".deepseek").join("skills");
        let actual_skills = config.skills_dir();
        assert_eq!(
            actual_skills.components().collect::<Vec<_>>(),
            expected_skills.components().collect::<Vec<_>>()
        );

        Ok(())
    }

    #[test]
    fn test_load_uses_tilde_expanded_deepseek_config_path() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-load-tilde-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".custom-deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(&config_path, "api_key = \"test-key\"\n")?;

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_CONFIG_PATH", "~/.custom-deepseek/config.toml");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_key.as_deref(), Some("test-key"));
        Ok(())
    }

    #[test]
    fn test_load_falls_back_to_home_config_when_env_path_missing() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-load-fallback-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let home_config = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&home_config)?;
        fs::write(&home_config, "api_key = \"home-key\"\n")?;

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var(
                "DEEPSEEK_CONFIG_PATH",
                temp_root.join("missing-config.toml").as_os_str(),
            );
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_key.as_deref(), Some("home-key"));
        Ok(())
    }

    #[test]
    fn test_nonexistent_profile_error() {
        let mut profiles = HashMap::new();
        profiles.insert("work".to_string(), Config::default());
        let config = ConfigFile {
            base: Config::default(),
            profiles: Some(profiles),
        };

        let err = apply_profile(config, Some("nonexistent")).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("Profile 'nonexistent' not found"));
        assert!(message.contains("Available profiles"));
        assert!(message.contains("work"));
    }

    #[test]
    fn test_profile_with_no_profiles_section() {
        let config = ConfigFile {
            base: Config::default(),
            profiles: None,
        };

        let err = apply_profile(config, Some("missing")).unwrap_err();
        assert!(err.to_string().contains("Available profiles: none"));
    }

    #[test]
    fn test_save_api_key_doesnt_match_similar_keys() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-api-key-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            "api_key_backup = \"old\"\napi_key = \"current\"\n",
        )?;

        let saved = save_api_key("new-key")?;
        assert_eq!(saved, SavedCredential::ConfigFile(config_path.clone()));

        let contents = fs::read_to_string(&config_path)?;
        assert!(contents.contains("api_key_backup = \"old\""));
        assert!(contents.contains("api_key = \""));
        Ok(())
    }

    #[test]
    fn test_empty_api_key_rejected() {
        let config = Config {
            api_key: Some("   ".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_missing_api_key_allowed() -> Result<()> {
        let config = Config::default();
        config.validate()?;
        Ok(())
    }

    #[test]
    fn apply_env_overrides_ignores_empty_api_key() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-empty-key-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Simulate a fresh user who copied .env.example to .env without
        // filling in DEEPSEEK_API_KEY: dotenv loads it as the empty string.
        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_API_KEY", "");
        }

        let mut config = Config {
            api_key: Some("from-config-file".to_string()),
            ..Default::default()
        };
        apply_env_overrides(&mut config);

        assert_eq!(config.api_key.as_deref(), Some("from-config-file"));
        config.validate()?;
        Ok(())
    }

    #[test]
    fn apply_env_overrides_does_not_copy_api_key_into_config() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-env-key-not-config-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        unsafe {
            env::set_var("DEEPSEEK_API_KEY", "env-key");
        }
        let mut config = Config::default();
        apply_env_overrides(&mut config);

        assert_eq!(config.api_key, None);
        assert_eq!(config.deepseek_api_key()?, "env-key");
        unsafe {
            env::remove_var("DEEPSEEK_API_KEY");
        }
        Ok(())
    }

    #[test]
    fn normalize_model_name_preserves_v_series_snapshots() {
        // v4 canonical forms still resolve
        assert_eq!(
            normalize_model_name("deepseek-v4-pro").as_deref(),
            Some("deepseek-v4-pro")
        );
        assert_eq!(
            normalize_model_name("deepseek-v4pro").as_deref(),
            Some("deepseek-v4-pro")
        );
        // v-series dated snapshots pass through unchanged
        assert_eq!(
            normalize_model_name("deepseek-v4-flash-20260423").as_deref(),
            Some("deepseek-v4-flash-20260423")
        );
        // future v-series identities pass through
        assert_eq!(
            normalize_model_name("deepseek-v5-pro-20270101").as_deref(),
            Some("deepseek-v5-pro-20270101")
        );
        // legacy names pass through unchanged — server decides
        assert_eq!(
            normalize_model_name("deepseek-chat").as_deref(),
            Some("deepseek-chat")
        );
        // cross-provider names still normalize
        assert_eq!(
            normalize_model_name("deepseek-ai/deepseek-v4-pro").as_deref(),
            Some("deepseek-ai/deepseek-v4-pro")
        );
        // preserve exact case for providers that require case-sensitive model IDs
        assert_eq!(
            normalize_model_name("DeepSeek-V4-Pro").as_deref(),
            Some("DeepSeek-V4-Pro")
        );
        assert_eq!(
            normalize_model_name("deepseek-ai/DeepSeek-V4-Pro").as_deref(),
            Some("deepseek-ai/DeepSeek-V4-Pro")
        );
    }

    #[test]
    fn normalize_model_for_provider_keeps_provider_remaps_when_case_is_preserved() {
        assert_eq!(
            normalize_model_for_provider(ApiProvider::Deepseek, "DeepSeek-V4-Pro").as_deref(),
            Some("DeepSeek-V4-Pro")
        );
        assert_eq!(
            normalize_model_for_provider(ApiProvider::NvidiaNim, "DeepSeek-V4-Pro").as_deref(),
            Some(DEFAULT_NVIDIA_NIM_MODEL)
        );
    }

    #[test]
    fn normalize_model_name_for_provider_canonicalizes_deepseek_api_variants() {
        assert_eq!(
            normalize_model_name_for_provider(ApiProvider::Deepseek, "deepseek-ai/DeepSeek-V4-Pro")
                .as_deref(),
            Some("deepseek-v4-pro")
        );
        assert_eq!(
            normalize_model_name_for_provider(ApiProvider::Deepseek, "deepseek/deepseek-v4-flash")
                .as_deref(),
            Some("deepseek-v4-flash")
        );
    }

    #[test]
    fn normalize_model_name_for_provider_keeps_provider_specific_ids() {
        assert_eq!(
            normalize_model_name_for_provider(ApiProvider::NvidiaNim, "deepseek-v4-pro").as_deref(),
            Some(DEFAULT_NVIDIA_NIM_MODEL)
        );
        assert_eq!(
            normalize_model_name_for_provider(ApiProvider::Openrouter, "deepseek-v4-flash")
                .as_deref(),
            Some(DEFAULT_OPENROUTER_FLASH_MODEL)
        );
        assert_eq!(
            normalize_model_name_for_provider(ApiProvider::Siliconflow, "deepseek-v4-pro")
                .as_deref(),
            Some(DEFAULT_SILICONFLOW_MODEL)
        );
        assert_eq!(
            normalize_model_name_for_provider(ApiProvider::Siliconflow, "deepseek-reasoner")
                .as_deref(),
            Some(DEFAULT_SILICONFLOW_MODEL)
        );
        assert_eq!(
            normalize_model_name_for_provider(ApiProvider::Siliconflow, "deepseek-r1").as_deref(),
            Some(DEFAULT_SILICONFLOW_MODEL)
        );
        assert_eq!(
            normalize_model_name_for_provider(ApiProvider::Siliconflow, "deepseek-chat").as_deref(),
            Some(DEFAULT_SILICONFLOW_FLASH_MODEL)
        );
        assert_eq!(
            normalize_model_name_for_provider(ApiProvider::Siliconflow, "deepseek-v3").as_deref(),
            Some(DEFAULT_SILICONFLOW_FLASH_MODEL)
        );
        assert_eq!(
            normalize_model_name_for_provider(ApiProvider::Siliconflow, "deepseek-v3.2").as_deref(),
            Some("deepseek-v3.2")
        );
    }

    #[test]
    fn model_completion_names_for_deepseek_api_are_deduplicated_bare_ids() {
        assert_eq!(
            model_completion_names_for_provider(ApiProvider::Deepseek),
            vec!["deepseek-v4-pro", "deepseek-v4-flash"]
        );
    }

    #[test]
    fn model_completion_names_for_moonshot_excludes_oauth_only_kimi_code_model() {
        assert_eq!(
            model_completion_names_for_provider(ApiProvider::Moonshot),
            vec![DEFAULT_MOONSHOT_MODEL]
        );
    }

    #[test]
    fn normalize_model_name_rejects_invalid_or_non_deepseek_ids() {
        assert!(normalize_model_name("qwen3-coder").is_none());
        assert!(normalize_model_name("codewhale v4").is_none());
        assert!(normalize_model_name("").is_none());
    }

    #[test]
    fn normalize_model_name_accepts_provider_prefixed_deepseek_ids() {
        assert_eq!(
            normalize_model_name("accounts/fireworks/models/deepseek-v4-flash").as_deref(),
            Some("accounts/fireworks/models/deepseek-v4-flash")
        );
        assert_eq!(
            normalize_model_name("provider/deepseek-ai/deepseek-v4-pro").as_deref(),
            Some("provider/deepseek-ai/deepseek-v4-pro")
        );
    }

    #[test]
    fn default_context_seams_are_opt_in() {
        let config = Config::default();
        assert!(!config.context.enabled.unwrap_or(false));
        assert_eq!(config.context.l1_threshold.unwrap_or(192_000), 192_000);
        assert_eq!(config.context.cycle_threshold.unwrap_or(768_000), 768_000);
        assert_eq!(
            config
                .context
                .seam_model
                .as_deref()
                .unwrap_or("deepseek-v4-flash"),
            "deepseek-v4-flash"
        );
    }

    #[test]
    fn profile_without_context_does_not_disable_base_context() {
        let mut profiles = HashMap::new();
        profiles.insert("work".to_string(), Config::default());
        let config = ConfigFile {
            base: Config {
                context: ContextConfig {
                    enabled: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            },
            profiles: Some(profiles),
        };

        let merged = apply_profile(config, Some("work")).expect("profile");
        assert_eq!(merged.context.enabled, Some(true));
    }

    #[test]
    fn removed_context_per_model_table_is_ignored_for_compatibility() -> Result<()> {
        let parsed: ConfigFile = toml::from_str(
            r#"
            [context]
            enabled = true

            [context.per_model.deepseek-v4-pro]
            l1_threshold = 111
            l2_threshold = 222
            l3_threshold = 333
            cycle_threshold = 444
            "#,
        )?;

        assert_eq!(parsed.base.context.enabled, Some(true));
        Ok(())
    }

    #[test]
    fn project_context_pack_defaults_on_and_can_be_disabled() {
        let mut config = Config::default();
        assert!(config.project_context_pack_enabled());

        config.context.project_pack = Some(false);
        assert!(!config.project_context_pack_enabled());
    }

    #[test]
    fn validate_accepts_future_deepseek_model_id() -> Result<()> {
        let config = Config {
            default_text_model: Some("deepseek-v4".to_string()),
            ..Default::default()
        };
        config.validate()?;
        Ok(())
    }

    #[test]
    fn validate_accepts_auto_default_text_model() -> Result<()> {
        let config = Config {
            default_text_model: Some("auto".to_string()),
            ..Default::default()
        };
        config.validate()?;
        assert_eq!(config.default_model(), "auto");
        Ok(())
    }

    #[test]
    fn deepseek_provider_defaults_to_beta_endpoint() {
        let config = Config::default();

        assert_eq!(config.api_provider(), ApiProvider::Deepseek);
        assert_eq!(config.deepseek_base_url(), DEFAULT_DEEPSEEK_BASE_URL);
    }

    #[test]
    fn explicit_deepseek_base_url_overrides_beta_default() {
        let config = Config {
            base_url: Some("https://api.deepseek.com".to_string()),
            ..Default::default()
        };

        assert_eq!(config.api_provider(), ApiProvider::Deepseek);
        assert_eq!(config.deepseek_base_url(), "https://api.deepseek.com");
    }

    #[test]
    fn loopback_deepseek_base_url_runs_without_api_key() -> Result<()> {
        let _lock = lock_test_env();
        let config = Config {
            base_url: Some("http://127.0.0.1:8000/v1".to_string()),
            ..Default::default()
        };

        assert_eq!(config.api_provider(), ApiProvider::Deepseek);
        assert!(has_api_key(&config));
        assert_eq!(config.deepseek_api_key()?, "");
        Ok(())
    }

    #[test]
    fn deepseek_model_env_overrides_default_text_model() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-model-env-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_MODEL", "deepseek-v4-flash-20260423");
        }

        let config = Config::load(None, None)?;
        // v-series snapshots pass through unchanged — no alias folding
        assert_eq!(
            config.default_text_model.as_deref(),
            Some("deepseek-v4-flash-20260423")
        );
        Ok(())
    }

    #[test]
    fn http_headers_load_from_root_config() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-http-headers-root-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"
api_key = "test-key"
http_headers = { "X-Model-Provider-Id" = "tongyi" }
"#,
        )?;

        let config = Config::load(None, None)?;
        assert_eq!(
            config
                .http_headers()
                .get("X-Model-Provider-Id")
                .map(String::as_str),
            Some("tongyi")
        );
        Ok(())
    }

    #[test]
    fn provider_http_headers_extend_and_override_root_config() {
        let mut providers = ProvidersConfig::default();
        providers.deepseek.http_headers = Some(HashMap::from([
            ("X-Model-Provider-Id".to_string(), "tongyi".to_string()),
            ("X-Shared".to_string(), "provider".to_string()),
        ]));
        let config = Config {
            http_headers: Some(HashMap::from([
                ("X-Root".to_string(), "root".to_string()),
                ("X-Shared".to_string(), "root".to_string()),
            ])),
            providers: Some(providers),
            ..Default::default()
        };

        let headers = config.http_headers();
        assert_eq!(
            headers.get("X-Model-Provider-Id").map(String::as_str),
            Some("tongyi")
        );
        assert_eq!(headers.get("X-Root").map(String::as_str), Some("root"));
        assert_eq!(
            headers.get("X-Shared").map(String::as_str),
            Some("provider")
        );
    }

    #[test]
    fn http_headers_env_overrides_config() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-http-headers-env-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"
api_key = "test-key"
http_headers = { "X-Model-Provider-Id" = "from-file" }
"#,
        )?;
        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_HTTP_HEADERS", "X-Model-Provider-Id=from-env");
        }

        let config = Config::load(None, None)?;
        assert_eq!(
            config
                .http_headers()
                .get("X-Model-Provider-Id")
                .map(String::as_str),
            Some("from-env")
        );
        Ok(())
    }

    #[test]
    fn nvidia_nim_provider_uses_nim_defaults() -> Result<()> {
        let config = Config {
            provider: Some("nvidia-nim".to_string()),
            ..Default::default()
        };

        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::NvidiaNim);
        assert_eq!(config.default_model(), DEFAULT_NVIDIA_NIM_MODEL);
        assert_eq!(config.deepseek_base_url(), DEFAULT_NVIDIA_NIM_BASE_URL);
        Ok(())
    }

    #[test]
    fn nvidia_nim_provider_normalizes_deepseek_v4_pro_alias() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-nim-model-alias-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            "provider = \"nvidia-nim\"\ndefault_text_model = \"deepseek-v4-pro\"\napi_key = \"nim-key\"\n",
        )?;

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::NvidiaNim);
        assert_eq!(
            config.default_text_model.as_deref(),
            Some(DEFAULT_NVIDIA_NIM_MODEL)
        );
        Ok(())
    }

    #[test]
    fn nvidia_nim_provider_normalizes_deepseek_v4_flash_alias() -> Result<()> {
        let config = Config {
            provider: Some("nvidia-nim".to_string()),
            default_text_model: Some("deepseek-v4-flash".to_string()),
            ..Default::default()
        };

        config.validate()?;
        assert_eq!(config.default_model(), DEFAULT_NVIDIA_NIM_FLASH_MODEL);
        Ok(())
    }

    #[test]
    fn nvidia_nim_env_overrides_provider_and_credentials() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-nim-env-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "nvidia-nim");
            env::set_var("NVIDIA_API_KEY", "nim-env-key");
            env::set_var("NVIDIA_NIM_MODEL", "deepseek-ai/deepseek-v4-pro");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::NvidiaNim);
        assert_eq!(config.deepseek_api_key()?, "nim-env-key");
        assert_eq!(config.default_model(), DEFAULT_NVIDIA_NIM_MODEL);
        Ok(())
    }

    #[test]
    fn nvidia_nim_env_accepts_short_nim_base_url_alias() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-nim-base-url-alias-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "nvidia-nim");
            env::set_var("NIM_BASE_URL", "https://short-nim.example/v1");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::NvidiaNim);
        assert_eq!(config.deepseek_base_url(), "https://short-nim.example/v1");
        Ok(())
    }

    #[test]
    fn nvidia_nim_env_accepts_facade_base_url_forwarding() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-nim-forwarded-base-url-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "nvidia-nim");
            env::set_var("DEEPSEEK_BASE_URL", "https://forwarded-nim.example/v1");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::NvidiaNim);
        assert_eq!(
            config.deepseek_base_url(),
            "https://forwarded-nim.example/v1"
        );
        Ok(())
    }

    #[test]
    fn openai_provider_uses_openai_compatible_defaults() -> Result<()> {
        let config = Config {
            provider: Some("openai".to_string()),
            ..Default::default()
        };

        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::Openai);
        assert_eq!(config.default_model(), DEFAULT_OPENAI_MODEL);
        assert_eq!(config.deepseek_base_url(), DEFAULT_OPENAI_BASE_URL);
        Ok(())
    }

    #[test]
    fn xiaomi_mimo_provider_uses_documented_defaults() -> Result<()> {
        let config = Config {
            provider: Some("xiaomi-mimo".to_string()),
            ..Default::default()
        };

        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::XiaomiMimo);
        assert_eq!(config.default_model(), DEFAULT_XIAOMI_MIMO_MODEL);
        assert_eq!(config.deepseek_base_url(), DEFAULT_XIAOMI_MIMO_BASE_URL);
        Ok(())
    }

    #[test]
    fn xiaomi_mimo_env_overrides_provider_base_url_model_and_key() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-xiaomi-mimo-env-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "mimo");
            env::set_var("MIMO_API_KEY", "mimo-env-key");
            env::set_var("MIMO_BASE_URL", "https://mimo-gateway.example/v1");
            env::set_var("MIMO_MODEL", "mimo-v2.5");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::XiaomiMimo);
        assert_eq!(config.deepseek_api_key()?, "mimo-env-key");
        assert_eq!(
            config.deepseek_base_url(),
            "https://mimo-gateway.example/v1"
        );
        assert_eq!(config.default_model(), "mimo-v2.5");
        Ok(())
    }

    #[test]
    fn atlascloud_provider_uses_documented_defaults() -> Result<()> {
        let config = Config {
            provider: Some("atlascloud".to_string()),
            ..Default::default()
        };

        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::Atlascloud);
        assert_eq!(config.default_model(), DEFAULT_ATLASCLOUD_MODEL);
        assert_eq!(config.deepseek_base_url(), DEFAULT_ATLASCLOUD_BASE_URL);
        Ok(())
    }

    #[test]
    fn atlascloud_env_overrides_provider_base_url_and_model() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-atlascloud-env-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "atlascloud");
            env::set_var("ATLASCLOUD_API_KEY", "atlascloud-env-key");
            env::set_var("ATLASCLOUD_BASE_URL", "https://api.atlascloud.ai/v1");
            env::set_var("ATLASCLOUD_MODEL", "deepseek-ai/deepseek-v4-flash");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Atlascloud);
        assert_eq!(config.deepseek_api_key()?, "atlascloud-env-key");
        assert_eq!(config.deepseek_base_url(), "https://api.atlascloud.ai/v1");
        assert_eq!(config.default_model(), "deepseek-ai/deepseek-v4-flash");
        Ok(())
    }

    #[test]
    fn wanjie_ark_provider_uses_documented_defaults() -> Result<()> {
        let config = Config {
            provider: Some("wanjie-ark".to_string()),
            ..Default::default()
        };

        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::WanjieArk);
        assert_eq!(config.default_model(), DEFAULT_WANJIE_ARK_MODEL);
        assert_eq!(config.deepseek_base_url(), DEFAULT_WANJIE_ARK_BASE_URL);
        Ok(())
    }

    #[test]
    fn wanjie_ark_env_overrides_provider_base_url_model_and_key() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-wanjie-env-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "ark-wanjie");
            env::set_var("WANJIE_ARK_API_KEY", "wanjie-env-key");
            env::set_var("WANJIE_ARK_BASE_URL", "https://wanjie.example/api/v1");
            env::set_var("WANJIE_ARK_MODEL", "wanjie-model-id");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::WanjieArk);
        assert_eq!(config.deepseek_api_key()?, "wanjie-env-key");
        assert_eq!(config.deepseek_base_url(), "https://wanjie.example/api/v1");
        assert_eq!(config.default_model(), "wanjie-model-id");
        Ok(())
    }

    #[test]
    fn wanjie_ark_provider_accepts_custom_model_and_table_key() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-wanjie-table-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"provider = "wanjie-ark"

[providers.wanjie_ark]
api_key = "wanjie-table-key"
base_url = "https://maas-openapi.wanjiedata.com/api/v1"
model = "account-model-id"
"#,
        )?;

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::WanjieArk);
        assert_eq!(config.deepseek_api_key()?, "wanjie-table-key");
        assert_eq!(
            config.deepseek_base_url(),
            "https://maas-openapi.wanjiedata.com/api/v1"
        );
        assert_eq!(config.default_model(), "account-model-id");
        Ok(())
    }

    #[test]
    fn openai_provider_accepts_custom_model_and_base_url() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-openai-table-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"provider = "openai"

[providers.openai]
api_key = "openai-table-key"
base_url = "https://openai-compatible.example/api/coding/paas/v4"
model = "glm-5"
"#,
        )?;

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Openai);
        assert_eq!(config.deepseek_api_key()?, "openai-table-key");
        assert_eq!(
            config.deepseek_base_url(),
            "https://openai-compatible.example/api/coding/paas/v4"
        );
        assert_eq!(config.default_model(), "glm-5");
        Ok(())
    }

    // Regression for issue #1714: `codewhale --provider openai --model
    // MiniMax-M2.7` forwards the choice via DEEPSEEK_MODEL (never
    // OPENAI_MODEL) and uses the DEFAULT base_url. The explicit custom model
    // must pass through verbatim instead of silently becoming a
    // DeepSeek/provider default.
    #[test]
    fn deepseek_model_env_passes_custom_model_through_for_non_deepseek_providers() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-1714-passthrough-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;

        // (a) provider=openai + model="MiniMax-M2.7" via env, NO OPENAI_MODEL,
        // DEFAULT base_url.
        {
            let _guard = EnvGuard::new(&temp_root);
            // Safety: test-only environment mutation guarded by a global mutex.
            unsafe {
                env::set_var("DEEPSEEK_PROVIDER", "openai");
                env::set_var("OPENAI_API_KEY", "openai-env-key");
                env::set_var("DEEPSEEK_MODEL", "MiniMax-M2.7");
            }

            let config = Config::load(None, None)?;
            assert_eq!(config.api_provider(), ApiProvider::Openai);
            assert_eq!(config.deepseek_base_url(), DEFAULT_OPENAI_BASE_URL);
            assert_eq!(config.default_model(), "MiniMax-M2.7");
        }

        // (b) a non-passthrough provider (novita) with an unknown custom model
        // and the DEFAULT base_url must also be preserved verbatim — never
        // rewritten to DEFAULT_NOVITA_MODEL.
        {
            let _guard = EnvGuard::new(&temp_root);
            // Safety: test-only environment mutation guarded by a global mutex.
            unsafe {
                env::set_var("DEEPSEEK_PROVIDER", "novita");
                env::set_var("NOVITA_API_KEY", "novita-env-key");
                env::set_var("DEEPSEEK_MODEL", "MiniMax-M2.7");
            }

            let config = Config::load(None, None)?;
            assert_eq!(config.api_provider(), ApiProvider::Novita);
            assert_eq!(config.deepseek_base_url(), DEFAULT_NOVITA_BASE_URL);
            assert_ne!(config.default_model(), DEFAULT_NOVITA_MODEL);
            assert_eq!(config.default_model(), "MiniMax-M2.7");
        }

        Ok(())
    }

    #[test]
    fn openai_env_overrides_provider_base_url_and_model() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-openai-env-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "openai");
            env::set_var("OPENAI_API_KEY", "openai-env-key");
            env::set_var("OPENAI_BASE_URL", "https://openai-compatible.example/v4");
            env::set_var("OPENAI_MODEL", "glm-5");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Openai);
        assert_eq!(config.deepseek_api_key()?, "openai-env-key");
        assert_eq!(
            config.deepseek_base_url(),
            "https://openai-compatible.example/v4"
        );
        assert_eq!(config.default_model(), "glm-5");
        Ok(())
    }

    #[test]
    fn openai_env_accepts_facade_base_url_forwarding() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-openai-forwarded-base-url-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "openai");
            env::set_var("OPENAI_API_KEY", "forwarded-openai-key");
            env::set_var("DEEPSEEK_BASE_URL", "https://forwarded-openai.example/v4");
            env::set_var("DEEPSEEK_MODEL", "glm-5");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Openai);
        assert_eq!(config.deepseek_api_key()?, "forwarded-openai-key");
        assert_eq!(
            config.deepseek_base_url(),
            "https://forwarded-openai.example/v4"
        );
        assert_eq!(config.default_model(), "glm-5");
        Ok(())
    }

    #[test]
    fn openrouter_provider_uses_canonical_defaults() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-or-defaults-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config = Config {
            provider: Some("openrouter".to_string()),
            ..Default::default()
        };
        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::Openrouter);
        assert_eq!(config.default_model(), DEFAULT_OPENROUTER_MODEL);
        assert_eq!(config.deepseek_base_url(), DEFAULT_OPENROUTER_BASE_URL);
        Ok(())
    }

    #[test]
    fn novita_provider_uses_canonical_defaults() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-novita-defaults-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config = Config {
            provider: Some("novita".to_string()),
            ..Default::default()
        };
        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::Novita);
        assert_eq!(config.default_model(), DEFAULT_NOVITA_MODEL);
        assert_eq!(config.deepseek_base_url(), DEFAULT_NOVITA_BASE_URL);
        Ok(())
    }

    #[test]
    fn fireworks_provider_uses_canonical_defaults() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-fireworks-defaults-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config = Config {
            provider: Some("fireworks".to_string()),
            ..Default::default()
        };
        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::Fireworks);
        assert_eq!(config.default_model(), DEFAULT_FIREWORKS_MODEL);
        assert_eq!(config.deepseek_base_url(), DEFAULT_FIREWORKS_BASE_URL);
        Ok(())
    }

    #[test]
    fn siliconflow_provider_uses_canonical_defaults() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-siliconflow-defaults-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config = Config {
            provider: Some("siliconflow".to_string()),
            ..Default::default()
        };
        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::Siliconflow);
        assert_eq!(config.default_model(), DEFAULT_SILICONFLOW_MODEL);
        assert_eq!(config.deepseek_base_url(), DEFAULT_SILICONFLOW_BASE_URL);
        assert_eq!(
            model_completion_names_for_provider(ApiProvider::Siliconflow),
            vec![DEFAULT_SILICONFLOW_MODEL, DEFAULT_SILICONFLOW_FLASH_MODEL]
        );
        Ok(())
    }

    #[test]
    fn sglang_provider_works_without_api_key() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-sglang-defaults-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config = Config {
            provider: Some("sglang".to_string()),
            ..Default::default()
        };
        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::Sglang);
        assert_eq!(config.default_model(), DEFAULT_SGLANG_MODEL);
        assert_eq!(config.deepseek_base_url(), DEFAULT_SGLANG_BASE_URL);
        assert_eq!(config.deepseek_api_key()?, "");
        assert!(has_api_key_for(&config, ApiProvider::Sglang));
        Ok(())
    }

    #[test]
    fn ollama_provider_uses_local_defaults_without_api_key() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-ollama-defaults-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config = Config {
            provider: Some("ollama".to_string()),
            ..Default::default()
        };
        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::Ollama);
        assert_eq!(config.default_model(), DEFAULT_OLLAMA_MODEL);
        assert_eq!(config.deepseek_base_url(), DEFAULT_OLLAMA_BASE_URL);
        assert_eq!(config.deepseek_api_key()?, "");
        assert!(has_api_key_for(&config, ApiProvider::Ollama));
        Ok(())
    }

    #[test]
    fn ollama_model_is_passed_through_verbatim() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-ollama-model-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"provider = "ollama"

[providers.ollama]
base_url = "http://127.0.0.1:11434/v1"
model = "qwen2.5-coder:7b"
"#,
        )?;

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Ollama);
        assert_eq!(config.default_model(), "qwen2.5-coder:7b");
        assert_eq!(config.deepseek_base_url(), "http://127.0.0.1:11434/v1");
        Ok(())
    }

    #[test]
    fn deepseek_base_url_env_scopes_to_self_hosted_providers() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-self-hosted-base-url-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "ollama");
            env::set_var("DEEPSEEK_BASE_URL", "http://ollama.remote:11434/v1");
        }
        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Ollama);
        assert_eq!(config.deepseek_base_url(), "http://ollama.remote:11434/v1");

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "vllm");
            env::set_var("DEEPSEEK_BASE_URL", "http://vllm.remote:8000/v1");
        }
        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Vllm);
        assert_eq!(config.deepseek_base_url(), "http://vllm.remote:8000/v1");
        Ok(())
    }

    #[test]
    fn vllm_env_resolves_reported_lan_http_endpoint_and_model() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-vllm-lan-http-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "vllm");
            env::set_var("VLLM_BASE_URL", "http://192.168.0.110:8000/v1");
            env::set_var("DEEPSEEK_MODEL", "deepseek-v4-flash");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Vllm);
        assert_eq!(config.deepseek_base_url(), "http://192.168.0.110:8000/v1");
        assert_eq!(config.default_model(), "deepseek-v4-flash");
        Ok(())
    }

    #[test]
    fn ollama_env_overrides_base_url_and_model() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-ollama-env-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "ollama-local");
            env::set_var("OLLAMA_BASE_URL", "http://ollama.example/v1");
            env::set_var("OLLAMA_MODEL", "deepseek-coder-v2:16b");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Ollama);
        assert_eq!(config.deepseek_base_url(), "http://ollama.example/v1");
        assert_eq!(config.default_model(), "deepseek-coder-v2:16b");
        Ok(())
    }

    #[test]
    fn openrouter_env_api_key_resolves_via_deepseek_api_key() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-or-env-key-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "openrouter");
            env::set_var("OPENROUTER_API_KEY", "or-env-key");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Openrouter);
        assert_eq!(config.deepseek_api_key()?, "or-env-key");
        Ok(())
    }

    #[test]
    fn novita_env_api_key_resolves_via_deepseek_api_key() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-novita-env-key-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "novita");
            env::set_var("NOVITA_API_KEY", "novita-env-key");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Novita);
        assert_eq!(config.deepseek_api_key()?, "novita-env-key");
        Ok(())
    }

    #[test]
    fn siliconflow_env_overrides_key_base_url_and_model() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-siliconflow-env-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "siliconflow");
            env::set_var("SILICONFLOW_API_KEY", "sf-env-key");
            env::set_var("SILICONFLOW_BASE_URL", "https://sf-mirror.example/v1");
            env::set_var("SILICONFLOW_MODEL", "deepseek-v4-flash");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Siliconflow);
        assert_eq!(config.deepseek_api_key()?, "sf-env-key");
        assert_eq!(config.deepseek_base_url(), "https://sf-mirror.example/v1");
        assert_eq!(config.default_model(), "deepseek-v4-flash");
        Ok(())
    }

    #[test]
    fn siliconflow_cn_base_url_env_normalizes_model_aliases() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-siliconflow-cn-env-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "siliconflow");
            env::set_var("SILICONFLOW_API_KEY", "sf-env-key");
            env::set_var("SILICONFLOW_BASE_URL", "https://api.siliconflow.cn/v1");
            env::set_var("SILICONFLOW_MODEL", "deepseek-reasoner");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Siliconflow);
        assert_eq!(config.deepseek_api_key()?, "sf-env-key");
        assert_eq!(config.deepseek_base_url(), "https://api.siliconflow.cn/v1");
        assert_eq!(config.default_model(), DEFAULT_SILICONFLOW_MODEL);
        Ok(())
    }

    #[test]
    fn openrouter_base_url_env_overrides_default() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-or-base-url-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "openrouter");
            env::set_var("OPENROUTER_BASE_URL", "https://or-mirror.example/v1");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Openrouter);
        assert_eq!(config.deepseek_base_url(), "https://or-mirror.example/v1");
        Ok(())
    }

    #[test]
    fn openrouter_reads_provider_table_from_config_file() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-or-table-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"provider = "openrouter"

[providers.openrouter]
api_key = "or-table-key"
base_url = "https://or-table.example/v1"
"#,
        )?;

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Openrouter);
        assert_eq!(config.deepseek_api_key()?, "or-table-key");
        assert_eq!(config.deepseek_base_url(), "https://or-table.example/v1");
        Ok(())
    }

    #[test]
    fn siliconflow_reads_provider_table_from_config_file() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-siliconflow-table-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"provider = "siliconflow"

[providers.siliconflow]
api_key = "sf-table-key"
model = "deepseek-v4-flash"
"#,
        )?;

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Siliconflow);
        assert_eq!(config.deepseek_api_key()?, "sf-table-key");
        assert_eq!(config.deepseek_base_url(), DEFAULT_SILICONFLOW_BASE_URL);
        assert_eq!(config.default_model(), DEFAULT_SILICONFLOW_FLASH_MODEL);
        Ok(())
    }

    #[test]
    fn openrouter_custom_base_url_preserves_provider_model() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-or-custom-model-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"provider = "openrouter"

[providers.openrouter]
api_key = "or-table-key"
base_url = "https://gateway.example.com/v1"
model = "DeepSeek-V4-Pro"
"#,
        )?;

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Openrouter);
        assert_eq!(config.deepseek_api_key()?, "or-table-key");
        assert_eq!(config.deepseek_base_url(), "https://gateway.example.com/v1");
        assert_eq!(config.default_model(), "DeepSeek-V4-Pro");
        Ok(())
    }

    #[test]
    fn novita_reads_provider_table_from_config_file() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-novita-table-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"provider = "novita"

[providers.novita]
api_key = "novita-table-key"
"#,
        )?;

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Novita);
        assert_eq!(config.deepseek_api_key()?, "novita-table-key");
        assert_eq!(config.deepseek_base_url(), DEFAULT_NOVITA_BASE_URL);
        Ok(())
    }

    #[test]
    fn moonshot_kimi_oauth_reads_fresh_cli_credential() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-kimi-oauth-key-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let kimi_share_dir = temp_root.join(".kimi");
        let credential_dir = kimi_share_dir.join("credentials");
        fs::create_dir_all(&credential_dir)?;
        unsafe { env::set_var("KIMI_SHARE_DIR", &kimi_share_dir) };

        let expires_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            + 3600.0;
        let credential = json!({
            "access_token": "fresh-oauth-token",
            "refresh_token": "refresh-token",
            "expires_at": expires_at,
            "scope": "openid profile email",
            "token_type": "Bearer",
        });
        fs::write(
            credential_dir.join(KIMI_CODE_CREDENTIAL_FILE),
            serde_json::to_string(&credential)?,
        )?;

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"provider = "moonshot"

[providers.moonshot]
auth_mode = "kimi_oauth"
api_key = "stale-api-key"
"#,
        )?;

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Moonshot);
        assert_eq!(config.deepseek_base_url(), DEFAULT_KIMI_CODE_BASE_URL);
        assert_eq!(config.default_model(), DEFAULT_KIMI_CODE_MODEL);
        assert_eq!(config.deepseek_api_key()?, "fresh-oauth-token");
        assert!(has_api_key_for(&config, ApiProvider::Moonshot));
        Ok(())
    }

    #[test]
    fn moonshot_kimi_code_api_key_uses_coding_model() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-kimi-code-key-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"provider = "moonshot"

[providers.moonshot]
api_key = "kimi-code-key"
base_url = "https://api.kimi.com/coding/v1"
"#,
        )?;

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Moonshot);
        assert_eq!(config.deepseek_base_url(), DEFAULT_KIMI_CODE_BASE_URL);
        assert_eq!(config.default_model(), DEFAULT_KIMI_CODE_MODEL);
        assert_eq!(config.deepseek_api_key()?, "kimi-code-key");
        assert!(has_api_key_for(&config, ApiProvider::Moonshot));
        Ok(())
    }

    /// Env-var-only path: `CODEWHALE_BASE_URL=https://api.kimi.com/coding/v1`
    /// combined with `CODEWHALE_PROVIDER=moonshot` must trigger Kimi Code
    /// model selection even when the TOML has no `base_url`.
    #[test]
    fn moonshot_kimi_code_env_base_url_selects_coding_model() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-kimi-code-env-url-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"[providers.moonshot]
api_key = "kimi-code-env-key"
"#,
        )?;
        // Safety: test-only env mutation guarded by lock_test_env().
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "moonshot");
            env::set_var("CODEWHALE_BASE_URL", "https://api.kimi.com/coding/v1");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Moonshot);
        assert_eq!(config.deepseek_base_url(), DEFAULT_KIMI_CODE_BASE_URL);
        assert_eq!(config.default_model(), DEFAULT_KIMI_CODE_MODEL);
        assert_eq!(config.deepseek_api_key()?, "kimi-code-env-key");
        assert!(has_api_key_for(&config, ApiProvider::Moonshot));
        Ok(())
    }

    /// Regression for issue #2160: a stale root `default_text_model` carried
    /// over from a DeepSeek setup must not steer the Kimi Code endpoint to
    /// `deepseek-v4-pro`. The user-facing trigger here is the legacy
    /// `DEEPSEEK_PROVIDER` env var (still produced by the `codewhale
    /// --provider moonshot` dispatcher for compat); the test also has a
    /// `CODEWHALE_PROVIDER` twin below for the public env path.
    #[test]
    fn moonshot_kimi_code_model_overrides_root_deepseek_default() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-kimi-code-root-model-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"provider = "deepseek"
default_text_model = "deepseek-v4-pro"

[providers.moonshot]
api_key = "kimi-code-key"
base_url = "https://api.kimi.com/coding/v1"
"#,
        )?;
        // Safety: test-only env mutation guarded by lock_test_env().
        unsafe { env::set_var("DEEPSEEK_PROVIDER", "moonshot") };

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Moonshot);
        assert_eq!(config.deepseek_base_url(), DEFAULT_KIMI_CODE_BASE_URL);
        assert_eq!(config.default_model(), DEFAULT_KIMI_CODE_MODEL);
        Ok(())
    }

    /// Same regression as above, but driven by the public `CODEWHALE_PROVIDER`
    /// env var. Documents the recommended user-facing setup path: never
    /// `DEEPSEEK_PROVIDER=moonshot`, always `CODEWHALE_PROVIDER=moonshot`
    /// (or `codewhale --provider moonshot`, which also resolves through
    /// this code path internally).
    #[test]
    fn moonshot_kimi_code_model_resolves_via_codewhale_provider_env() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-kimi-code-cw-env-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"provider = "deepseek"
default_text_model = "deepseek-v4-pro"

[providers.moonshot]
api_key = "kimi-code-key"
base_url = "https://api.kimi.com/coding/v1"
"#,
        )?;
        // Safety: test-only env mutation guarded by lock_test_env().
        unsafe { env::set_var("CODEWHALE_PROVIDER", "moonshot") };

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Moonshot);
        assert_eq!(config.deepseek_base_url(), DEFAULT_KIMI_CODE_BASE_URL);
        assert_eq!(config.default_model(), DEFAULT_KIMI_CODE_MODEL);
        Ok(())
    }

    /// `CODEWHALE_PROVIDER` wins when both it and the legacy
    /// `DEEPSEEK_PROVIDER` are set, so a user adding the new alias to their
    /// shell isn't surprised by a stale legacy export.
    #[test]
    fn codewhale_provider_env_takes_precedence_over_deepseek_provider() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-cw-vs-ds-provider-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(&config_path, "provider = \"deepseek\"\n")?;
        // Safety: test-only env mutation guarded by lock_test_env().
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "moonshot");
            env::set_var("DEEPSEEK_PROVIDER", "openrouter");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Moonshot);
        Ok(())
    }

    /// Moonshot Platform path: when [providers.moonshot] is empty (or
    /// missing) and no Kimi Code endpoint is configured, the resolver
    /// defaults to the Moonshot Platform base URL and the `kimi-k2.6`
    /// model. This is the "I have a Moonshot Platform API key, not a
    /// Kimi Code plan key" path.
    #[test]
    fn moonshot_platform_defaults_to_kimi_k26() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-moonshot-platform-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"provider = "moonshot"

[providers.moonshot]
api_key = "moonshot-platform-key"
"#,
        )?;

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Moonshot);
        assert_eq!(config.deepseek_base_url(), DEFAULT_MOONSHOT_BASE_URL);
        assert_eq!(config.default_model(), DEFAULT_MOONSHOT_MODEL);
        assert_eq!(config.deepseek_api_key()?, "moonshot-platform-key");
        Ok(())
    }

    #[test]
    fn has_api_key_for_detects_env_and_config_per_provider() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-has-key-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let mut config = Config::default();
        assert!(!has_api_key_for(&config, ApiProvider::Openai));
        assert!(!has_api_key_for(&config, ApiProvider::WanjieArk));
        assert!(!has_api_key_for(&config, ApiProvider::Openrouter));
        assert!(!has_api_key_for(&config, ApiProvider::XiaomiMimo));
        assert!(!has_api_key_for(&config, ApiProvider::Siliconflow));
        assert!(
            has_api_key_for(&config, ApiProvider::Sglang),
            "SGLang is self-hosted and does not require a key by default"
        );
        assert!(
            has_api_key_for(&config, ApiProvider::Vllm),
            "vLLM is self-hosted and does not require a key by default"
        );

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("OPENROUTER_API_KEY", "or-env");
            env::set_var("OPENAI_API_KEY", "openai-env");
            env::set_var("WANJIE_API_KEY", "wanjie-env");
            env::set_var("MIMO_API_KEY", "mimo-env");
            env::set_var("SILICONFLOW_API_KEY", "sf-env");
        }
        assert!(has_api_key_for(&config, ApiProvider::Openai));
        assert!(has_api_key_for(&config, ApiProvider::WanjieArk));
        assert!(has_api_key_for(&config, ApiProvider::Openrouter));
        assert!(has_api_key_for(&config, ApiProvider::XiaomiMimo));
        assert!(has_api_key_for(&config, ApiProvider::Siliconflow));
        assert!(!has_api_key_for(&config, ApiProvider::Novita));

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::remove_var("OPENROUTER_API_KEY");
            env::remove_var("OPENAI_API_KEY");
            env::remove_var("WANJIE_API_KEY");
            env::remove_var("MIMO_API_KEY");
            env::remove_var("SILICONFLOW_API_KEY");
        }
        let mut providers = ProvidersConfig::default();
        providers.openai.api_key = Some("file-openai".to_string());
        providers.wanjie_ark.api_key = Some("file-wanjie".to_string());
        providers.xiaomi_mimo.api_key = Some("file-mimo".to_string());
        providers.novita.api_key = Some("file-novita".to_string());
        providers.siliconflow.api_key = Some("file-siliconflow".to_string());
        config.providers = Some(providers);
        assert!(has_api_key_for(&config, ApiProvider::Openai));
        assert!(has_api_key_for(&config, ApiProvider::WanjieArk));
        assert!(has_api_key_for(&config, ApiProvider::XiaomiMimo));
        assert!(has_api_key_for(&config, ApiProvider::Novita));
        assert!(has_api_key_for(&config, ApiProvider::Siliconflow));
        assert!(!has_api_key_for(&config, ApiProvider::Openrouter));
        Ok(())
    }

    #[test]
    fn has_api_key_for_uses_deepseek_cn_provider_table() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-has-key-cn-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let mut providers = ProvidersConfig::default();
        providers.deepseek_cn.api_key = Some("cn-file-key".to_string());
        let config = Config {
            providers: Some(providers),
            ..Config::default()
        };

        assert!(has_api_key_for(&config, ApiProvider::DeepseekCN));
        Ok(())
    }

    #[test]
    fn has_api_key_for_uses_root_config_key_for_deepseek_variants() {
        let config = Config {
            api_key: Some("root-config-key".to_string()),
            ..Config::default()
        };

        assert!(has_api_key_for(&config, ApiProvider::Deepseek));
        assert!(has_api_key_for(&config, ApiProvider::DeepseekCN));
    }

    #[test]
    fn save_api_key_for_openrouter_writes_provider_table() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-save-key-or-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);
        unsafe { std::env::set_var("DEEPSEEK_SECRET_BACKEND", "local") };

        let path = save_api_key_for(ApiProvider::Openrouter, "or-saved-key")?;
        let contents = fs::read_to_string(&path)?;
        let parsed: toml::Value = toml::from_str(&contents)?;
        assert_eq!(
            parsed
                .get("providers")
                .and_then(|p| p.get("openrouter"))
                .and_then(|t| t.get("api_key"))
                .and_then(toml::Value::as_str),
            Some("or-saved-key")
        );
        // Re-saving must not duplicate or wipe sibling tables.
        save_api_key_for(ApiProvider::Novita, "novita-saved-key")?;
        let contents = fs::read_to_string(&path)?;
        let parsed: toml::Value = toml::from_str(&contents)?;
        assert_eq!(
            parsed
                .get("providers")
                .and_then(|p| p.get("openrouter"))
                .and_then(|t| t.get("api_key"))
                .and_then(toml::Value::as_str),
            Some("or-saved-key")
        );
        assert_eq!(
            parsed
                .get("providers")
                .and_then(|p| p.get("novita"))
                .and_then(|t| t.get("api_key"))
                .and_then(toml::Value::as_str),
            Some("novita-saved-key")
        );
        save_api_key_for(ApiProvider::Openai, "openai-saved-key")?;
        save_api_key_for(ApiProvider::WanjieArk, "wanjie-saved-key")?;
        save_api_key_for(ApiProvider::Fireworks, "fireworks-saved-key")?;
        save_api_key_for(ApiProvider::XiaomiMimo, "mimo-saved-key")?;
        save_api_key_for(ApiProvider::Siliconflow, "sf-saved-key")?;
        save_api_key_for(ApiProvider::Sglang, "sglang-saved-key")?;
        let contents = fs::read_to_string(&path)?;
        let parsed: toml::Value = toml::from_str(&contents)?;
        assert_eq!(
            parsed
                .get("providers")
                .and_then(|p| p.get("openai"))
                .and_then(|t| t.get("api_key"))
                .and_then(toml::Value::as_str),
            Some("openai-saved-key")
        );
        assert_eq!(
            parsed
                .get("providers")
                .and_then(|p| p.get("wanjie_ark"))
                .and_then(|t| t.get("api_key"))
                .and_then(toml::Value::as_str),
            Some("wanjie-saved-key")
        );
        assert_eq!(
            parsed
                .get("providers")
                .and_then(|p| p.get("fireworks"))
                .and_then(|t| t.get("api_key"))
                .and_then(toml::Value::as_str),
            Some("fireworks-saved-key")
        );
        assert_eq!(
            parsed
                .get("providers")
                .and_then(|p| p.get("xiaomi_mimo"))
                .and_then(|t| t.get("api_key"))
                .and_then(toml::Value::as_str),
            Some("mimo-saved-key")
        );
        assert_eq!(
            parsed
                .get("providers")
                .and_then(|p| p.get("siliconflow"))
                .and_then(|t| t.get("api_key"))
                .and_then(toml::Value::as_str),
            Some("sf-saved-key")
        );
        assert_eq!(
            parsed
                .get("providers")
                .and_then(|p| p.get("sglang"))
                .and_then(|t| t.get("api_key"))
                .and_then(toml::Value::as_str),
            Some("sglang-saved-key")
        );
        Ok(())
    }

    #[test]
    fn save_api_key_for_deepseek_cn_uses_root_deepseek_storage() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-save-key-cn-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);
        unsafe { std::env::set_var("DEEPSEEK_SECRET_BACKEND", "local") };

        let path = save_api_key_for(ApiProvider::DeepseekCN, "cn-saved-key")?;
        let contents = fs::read_to_string(&path)?;
        let parsed: toml::Value = toml::from_str(&contents)?;

        assert_eq!(
            parsed.get("api_key").and_then(toml::Value::as_str),
            Some("cn-saved-key")
        );
        Ok(())
    }

    #[test]
    fn nvidia_nim_reads_facade_provider_table() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-nim-provider-table-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"provider = "nvidia-nim"
default_text_model = "deepseek-v4-flash"

[providers.nvidia_nim]
api_key = "nim-table-key"
base_url = "https://nim-table.example/v1"
model = "deepseek-v4-pro"
"#,
        )?;

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::NvidiaNim);
        assert_eq!(config.deepseek_api_key()?, "nim-table-key");
        assert_eq!(config.deepseek_base_url(), "https://nim-table.example/v1");
        // Custom base URL preserves the user-specified model name; normalisation
        // is skipped because the gateway expects the model name as-provided.
        assert_eq!(config.default_model(), "deepseek-v4-pro");
        Ok(())
    }

    #[test]
    fn nvidia_nim_provider_table_key_overrides_root_deepseek_key() -> Result<()> {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-nim-root-key-precedence-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        ensure_parent_dir(&config_path)?;
        fs::write(
            &config_path,
            r#"api_key = "codewhale-root-key"
provider = "nvidia-nim"

[providers.nvidia_nim]
api_key = "nim-table-key"
base_url = "https://integrate.api.nvidia.com/v1"
model = "deepseek-ai/deepseek-v4-pro"
"#,
        )?;

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::NvidiaNim);
        assert_eq!(config.deepseek_api_key()?, "nim-table-key");
        Ok(())
    }

    // ========================================================================
    // Provider Capability Matrix tests
    // ========================================================================

    #[test]
    fn provider_capability_deepseek_v4_pro_has_1m_window_and_thinking() {
        let cap = provider_capability(ApiProvider::Deepseek, "deepseek-v4-pro");
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, 384_000);
        assert!(cap.thinking_supported);
        assert!(cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }

    #[test]
    fn provider_capability_deepseek_v4_flash_has_1m_window_and_thinking() {
        let cap = provider_capability(ApiProvider::Deepseek, "deepseek-v4-flash");
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, 384_000);
        assert!(cap.thinking_supported);
        assert!(cap.cache_telemetry_supported);
    }

    #[test]
    fn provider_capability_deepseek_chat_alias_has_v4_flash_caps_and_metadata() {
        let cap = provider_capability(ApiProvider::Deepseek, "deepseek-chat");
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, 384_000);
        assert!(cap.thinking_supported);
        assert!(cap.cache_telemetry_supported);

        let deprecation = cap
            .alias_deprecation
            .as_ref()
            .expect("alias deprecation metadata");
        assert_eq!(deprecation.alias, "deepseek-chat");
        assert_eq!(deprecation.replacement, "deepseek-v4-flash");
        assert_eq!(deprecation.retirement_date, "2026-07-24");
        assert_eq!(deprecation.retirement_utc, "2026-07-24T15:59:00Z");
    }

    #[test]
    fn provider_capability_deepseek_reasoner_alias_has_v4_flash_caps_and_metadata() {
        let cap = provider_capability(ApiProvider::Deepseek, "deepseek-reasoner");
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, 384_000);
        assert!(cap.thinking_supported);
        assert!(cap.cache_telemetry_supported);

        let deprecation = cap
            .alias_deprecation
            .as_ref()
            .expect("alias deprecation metadata");
        assert_eq!(deprecation.alias, "deepseek-reasoner");
        assert_eq!(deprecation.replacement, "deepseek-v4-flash");
    }

    #[test]
    fn provider_capability_deepseek_v4_flash_has_no_alias_deprecation() {
        let cap = provider_capability(ApiProvider::Deepseek, "deepseek-v4-flash");
        assert!(cap.alias_deprecation.is_none());
    }

    #[test]
    fn provider_capability_nvidia_nim_v4_pro_maps_correctly() {
        let cap = provider_capability(ApiProvider::NvidiaNim, DEFAULT_NVIDIA_NIM_MODEL);
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, 384_000);
        assert!(cap.thinking_supported);
        assert!(cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }

    #[test]
    fn provider_capability_nvidia_nim_v4_flash_maps_correctly() {
        let cap = provider_capability(ApiProvider::NvidiaNim, DEFAULT_NVIDIA_NIM_FLASH_MODEL);
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, 384_000);
        assert!(cap.thinking_supported);
        assert!(cap.cache_telemetry_supported);
    }

    #[test]
    fn provider_capability_openrouter_v4_pro_has_thinking_no_cache() {
        let cap = provider_capability(ApiProvider::Openrouter, DEFAULT_OPENROUTER_MODEL);
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, 384_000);
        assert!(cap.thinking_supported);
        // OpenRouter does not return DeepSeek prompt-cache telemetry.
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }

    #[test]
    fn provider_capability_xiaomi_mimo_has_thinking_no_cache() {
        let cap = provider_capability(ApiProvider::XiaomiMimo, DEFAULT_XIAOMI_MIMO_MODEL);
        assert_eq!(cap.context_window, 1_000_000);
        assert_eq!(cap.max_output, 128_000);
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }

    #[test]
    fn provider_capability_novita_v4_pro_has_thinking_no_cache() {
        let cap = provider_capability(ApiProvider::Novita, DEFAULT_NOVITA_MODEL);
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, 384_000);
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
    }

    #[test]
    fn provider_capability_fireworks_v4_pro_has_thinking_no_cache() {
        let cap = provider_capability(ApiProvider::Fireworks, DEFAULT_FIREWORKS_MODEL);
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, 384_000);
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
    }

    #[test]
    fn provider_capability_siliconflow_v4_pro_has_thinking_no_cache() {
        let cap = provider_capability(ApiProvider::Siliconflow, DEFAULT_SILICONFLOW_MODEL);
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, 384_000);
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }

    #[test]
    fn provider_capability_sglang_v4_pro_has_thinking_no_cache() {
        let cap = provider_capability(ApiProvider::Sglang, DEFAULT_SGLANG_MODEL);
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, 384_000);
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
    }

    #[test]
    fn provider_capability_openai_custom_model_is_chat_completions_without_thinking() {
        let cap = provider_capability(ApiProvider::Openai, "glm-5");
        assert_eq!(
            cap.context_window,
            crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, 4096);
        assert!(!cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }

    #[test]
    fn provider_capability_atlascloud_custom_model_is_chat_completions_without_thinking() {
        let cap = provider_capability(ApiProvider::Atlascloud, "deepseek-ai/deepseek-v4-flash");
        assert_eq!(
            cap.context_window,
            crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, 4096);
        assert!(!cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }

    #[test]
    fn provider_capability_wanjie_ark_reasoner_has_thinking_no_cache() {
        let cap = provider_capability(ApiProvider::WanjieArk, DEFAULT_WANJIE_ARK_MODEL);
        assert_eq!(
            cap.context_window,
            crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, 4096);
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }

    #[test]
    fn provider_capability_ollama_is_openai_compatible_without_thinking() {
        let cap = provider_capability(ApiProvider::Ollama, "deepseek-v3.1:671b");
        assert_eq!(cap.context_window, 8192);
        assert_eq!(cap.max_output, 4096);
        assert!(!cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }

    #[test]
    fn provider_capability_non_v4_model_has_smaller_window() {
        let cap = provider_capability(ApiProvider::Deepseek, "deepseek-coder");
        assert_eq!(
            cap.context_window,
            crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, 4096);
        assert!(!cap.thinking_supported);
    }

    #[test]
    fn provider_capability_roundtrip_serialization() {
        let cap = provider_capability(ApiProvider::Deepseek, "deepseek-v4-pro");
        let json = serde_json::to_value(&cap).unwrap();
        let deserialized: ProviderCapability = serde_json::from_value(json).unwrap();
        assert_eq!(cap, deserialized);
    }

    #[test]
    fn status_item_balance_available_only_for_deepseek_providers() {
        // Balance item should only be offered for DeepSeek / DeepSeekCN.
        assert!(StatusItem::Balance.is_available_for(ApiProvider::Deepseek));
        assert!(StatusItem::Balance.is_available_for(ApiProvider::DeepseekCN));
        // Sanity: all other known providers should hide the Balance toggle.
        assert!(!StatusItem::Balance.is_available_for(ApiProvider::Openrouter));
        assert!(!StatusItem::Balance.is_available_for(ApiProvider::Novita));
        assert!(!StatusItem::Balance.is_available_for(ApiProvider::NvidiaNim));
        assert!(!StatusItem::Balance.is_available_for(ApiProvider::Fireworks));
        assert!(!StatusItem::Balance.is_available_for(ApiProvider::Sglang));
        assert!(!StatusItem::Balance.is_available_for(ApiProvider::Vllm));
        assert!(!StatusItem::Balance.is_available_for(ApiProvider::Ollama));
        assert!(!StatusItem::Balance.is_available_for(ApiProvider::Openai));
        assert!(!StatusItem::Balance.is_available_for(ApiProvider::Atlascloud));
        // Other StatusItem variants should be available everywhere.
        assert!(StatusItem::Mode.is_available_for(ApiProvider::Ollama));
    }

    #[test]
    fn status_items_deser_ignores_unknown_variants() {
        // Simulate a stable build reading config written by a dev build that
        // knows about items the stable build doesn't (e.g. "balance" or a
        // future "cost_saving" chip).
        let toml_str = r#"
            alternate_screen = "auto"
            status_items = ["mode", "model", "unknown_future_item", "cost", "another_unknown", "status"]
        "#;
        let tui: TuiConfig = toml::from_str(toml_str).expect("should parse without error");
        let items = tui.status_items.expect("status_items should be Some");
        assert_eq!(items.len(), 4, "unknown items should be silently dropped");
        assert_eq!(items[0], StatusItem::Mode);
        assert_eq!(items[1], StatusItem::Model);
        assert_eq!(items[2], StatusItem::Cost);
        assert_eq!(items[3], StatusItem::Status);
    }
}
