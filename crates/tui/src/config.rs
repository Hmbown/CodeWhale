//! Configuration loading and defaults for codewhale.

use std::collections::HashMap;
use std::fmt::Write;
use std::fs;
#[cfg(unix)]
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use codewhale_execpolicy::ExecPolicyEngine;
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::audit::log_sensitive_event;
use crate::features::{Feature, Features, FeaturesToml, is_known_feature_key};
use crate::hooks::HooksConfig;

// Sub-agent concurrency/timeout limit constants and their clamp resolvers live
// in the `subagent_limits` leaf module. The constants are re-exported (keeping
// each item's visibility) so `crate::config::<CONST>` paths resolve unchanged;
// the private resolvers are pulled back in without widening external surface
// (#3311).
mod subagent_limits;
pub use subagent_limits::*;
use subagent_limits::{resolve_subagent_api_timeout_secs, resolve_subagent_heartbeat_timeout_secs};

// Provider model-name and base-URL constants live in the `models` leaf module
// and are re-exported below so every `crate::config::<CONST>` path is unchanged
// (#3311).
mod models;
pub use models::*;

const API_KEYRING_SENTINEL: &str = "__KEYRING__";
pub const DEFAULT_ZAI_PROVIDER_MAX_CONCURRENCY: usize = 3;
pub const MAX_PROVIDER_REQUEST_CONCURRENCY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiProvider {
    Deepseek,
    DeepseekCN,
    DeepseekAnthropic,
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
    SiliconflowCn,
    Arcee,
    Moonshot,
    Sglang,
    Vllm,
    Ollama,
    Huggingface,
    Together,
    Qianfan,
    OpenaiCodex,
    Anthropic,
    Openmodel,
    Zai,
    Stepfun,
    Minimax,
    Deepinfra,
    Sakana,
    OpenCodeZen,
    /// User-defined OpenAI-compatible endpoint (#1519).
    ///
    /// Selected when `provider = "<name>"` names a `[providers.<name>]
    /// kind="openai-compatible"` table. A single dynamic identity that maps to
    /// [`codewhale_config::ProviderKind::Custom`] and routes via the OpenAI Chat
    /// Completions wire protocol; the concrete endpoint/model/auth come from the
    /// named config table, not from this variant.
    Custom,
}

impl ApiProvider {
    #[must_use]
    pub fn names_hint() -> String {
        let mut names = Vec::with_capacity(Self::all().len() + 1);
        names.push(Self::Deepseek.as_str());
        names.push(Self::DeepseekCN.as_str());
        names.extend(
            Self::all()
                .iter()
                .filter(|provider| !matches!(provider, Self::Deepseek))
                .map(|provider| provider.as_str()),
        );
        names.join(", ")
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        // ApiProvider-specific: "deepseek-cn" is a legacy variant here,
        // while ProviderKind treats it as a Deepseek alias.
        if trimmed.eq_ignore_ascii_case("deepseek-cn")
            || trimmed.eq_ignore_ascii_case("deepseek_china")
            || trimmed.eq_ignore_ascii_case("deepseekcn")
            || trimmed.eq_ignore_ascii_case("deepseek-china")
        {
            return Some(Self::DeepseekCN);
        }
        codewhale_config::ProviderKind::parse(value).map(Self::from_kind)
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self.kind() {
            Some(kind) => kind.as_str(),
            None => "deepseek-cn",
        }
    }

    /// Human-friendly label for picker UIs / status chips.
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self.kind() {
            Some(kind) => kind.provider().display_name(),
            None => "DeepSeek (legacy alias)",
        }
    }

    /// Provider metadata from the shared config crate.
    ///
    /// Returns `None` only for the TUI-only legacy `DeepseekCN` variant, which
    /// intentionally keeps its own config table while sharing DeepSeek auth envs.
    #[must_use]
    pub fn metadata(self) -> Option<&'static dyn codewhale_config::provider::Provider> {
        self.kind().map(|kind| kind.provider())
    }

    /// Environment variable candidates for this provider's API key.
    #[must_use]
    pub fn env_vars(self) -> &'static [&'static str] {
        self.metadata().map_or(
            codewhale_config::ProviderKind::Deepseek
                .provider()
                .env_vars(),
            |provider| provider.env_vars(),
        )
    }

    /// Environment variable candidates formatted for UI copy.
    #[must_use]
    pub fn env_vars_label(self) -> String {
        self.env_vars().join(" / ")
    }

    /// Providers ordered for picker/browsing surfaces.
    #[must_use]
    pub fn sorted_for_display() -> Vec<Self> {
        codewhale_config::provider::providers_sorted_for_display()
            .iter()
            .map(|provider| Self::from_kind(provider.kind()))
            .collect()
    }

    /// Default base URL for this provider.
    #[must_use]
    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::DeepseekCN => DEFAULT_DEEPSEEKCN_BASE_URL,
            _ => self
                .metadata()
                .expect("ApiProvider variant missing ProviderKind metadata")
                .default_base_url(),
        }
    }

    /// Official provider page for creating or locating credentials.
    #[must_use]
    pub fn credential_url(self) -> Option<&'static str> {
        Some(match self {
            Self::Deepseek | Self::DeepseekCN | Self::DeepseekAnthropic => {
                "https://platform.deepseek.com/api_keys"
            }
            Self::NvidiaNim => "https://build.nvidia.com/settings/api-keys",
            Self::Openai => "https://platform.openai.com/api-keys",
            Self::Atlascloud => "https://atlascloud.ai/docs/en/api-keys",
            Self::WanjieArk => "https://docs.wanjiedata.com/maas/maas-openapi-v1.html",
            Self::Volcengine => "https://console.volcengine.com/ark",
            Self::Openrouter => "https://openrouter.ai/settings/keys",
            Self::XiaomiMimo => "https://platform.xiaomimimo.com/token-plan",
            Self::Novita => "https://novita.ai/docs/guides/quickstart",
            Self::Fireworks => "https://fireworks.ai/account/api-keys",
            Self::Siliconflow | Self::SiliconflowCn => "https://cloud.siliconflow.com/account/ak",
            Self::Arcee => "https://docs.arcee.ai/other/create-your-first-api-key",
            Self::Moonshot => "https://platform.kimi.ai/",
            Self::Huggingface => "https://huggingface.co/settings/tokens",
            Self::Together => "https://api.together.ai/settings/api-keys",
            Self::Qianfan => "https://console.bce.baidu.com/iam/#/iam/accesslist",
            Self::Anthropic => "https://console.anthropic.com/settings/keys",
            Self::Openmodel => "https://docs.openmodel.ai/en/docs/guides/api-key",
            Self::Zai => "https://z.ai/model-api",
            Self::Stepfun => "https://platform.stepfun.ai/",
            Self::Minimax => "https://platform.minimax.io/docs/guides/quickstart-preparation",
            Self::Deepinfra => "https://deepinfra.com/dash/api_keys",
            Self::Sakana => "https://api.sakana.ai/",
            Self::OpenCodeZen => "https://opencode.ai/zen",
            Self::OpenaiCodex | Self::Sglang | Self::Vllm | Self::Ollama => return None,
            // Custom endpoints have no canonical credential page; the user
            // supplies the key via their own `api_key_env`.
            Self::Custom => return None,
        })
    }

    /// All providers in stable `ProviderKind::ALL` order.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &Self::FROM_KIND_LOOKUP
    }

    /// `ApiProvider` discriminant → `ProviderKind` lookup.
    /// Index 1 is `None` for the legacy `DeepseekCN` variant.
    const KIND_LOOKUP: [Option<codewhale_config::ProviderKind>; 32] = [
        Some(codewhale_config::ProviderKind::Deepseek),
        None, // DeepseekCN
        Some(codewhale_config::ProviderKind::DeepseekAnthropic),
        Some(codewhale_config::ProviderKind::NvidiaNim),
        Some(codewhale_config::ProviderKind::Openai),
        Some(codewhale_config::ProviderKind::Atlascloud),
        Some(codewhale_config::ProviderKind::WanjieArk),
        Some(codewhale_config::ProviderKind::Volcengine),
        Some(codewhale_config::ProviderKind::Openrouter),
        Some(codewhale_config::ProviderKind::XiaomiMimo),
        Some(codewhale_config::ProviderKind::Novita),
        Some(codewhale_config::ProviderKind::Fireworks),
        Some(codewhale_config::ProviderKind::Siliconflow),
        Some(codewhale_config::ProviderKind::SiliconflowCN),
        Some(codewhale_config::ProviderKind::Arcee),
        Some(codewhale_config::ProviderKind::Moonshot),
        Some(codewhale_config::ProviderKind::Sglang),
        Some(codewhale_config::ProviderKind::Vllm),
        Some(codewhale_config::ProviderKind::Ollama),
        Some(codewhale_config::ProviderKind::Huggingface),
        Some(codewhale_config::ProviderKind::Together),
        Some(codewhale_config::ProviderKind::Qianfan),
        Some(codewhale_config::ProviderKind::OpenaiCodex),
        Some(codewhale_config::ProviderKind::Anthropic),
        Some(codewhale_config::ProviderKind::Openmodel),
        Some(codewhale_config::ProviderKind::Zai),
        Some(codewhale_config::ProviderKind::Stepfun),
        Some(codewhale_config::ProviderKind::Minimax),
        Some(codewhale_config::ProviderKind::Deepinfra),
        Some(codewhale_config::ProviderKind::Sakana),
        Some(codewhale_config::ProviderKind::OpenCodeZen),
        Some(codewhale_config::ProviderKind::Custom),
    ];

    /// `ProviderKind` discriminant → `ApiProvider` lookup.
    const FROM_KIND_LOOKUP: [Self; 31] = [
        Self::Deepseek,
        Self::DeepseekAnthropic,
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
        Self::Arcee,
        Self::SiliconflowCn,
        Self::Moonshot,
        Self::Sglang,
        Self::Vllm,
        Self::Ollama,
        Self::Huggingface,
        Self::Together,
        Self::Qianfan,
        Self::OpenaiCodex,
        Self::Anthropic,
        Self::Openmodel,
        Self::Zai,
        Self::Stepfun,
        Self::Minimax,
        Self::Deepinfra,
        Self::Sakana,
        Self::OpenCodeZen,
        Self::Custom,
    ];

    /// Map to the config-level `ProviderKind`.
    /// Returns `None` for the legacy `DeepseekCN` variant.
    #[must_use]
    pub fn kind(self) -> Option<codewhale_config::ProviderKind> {
        Self::KIND_LOOKUP[self as usize]
    }

    /// Construct from a config-level `ProviderKind`.
    #[must_use]
    pub fn from_kind(kind: codewhale_config::ProviderKind) -> Self {
        Self::FROM_KIND_LOOKUP[kind as usize]
    }

    /// Whether this provider is a self-hosted / local runtime.
    ///
    /// These run without hosted authentication and keep traffic on the user's
    /// own infrastructure, so they carry a local/private posture. Used by the
    /// fallback chain to avoid silently routing a local/private primary out to
    /// a cloud provider (#2574) and by the `/provider` dashboard's self-hosted
    /// hint (#3083). Update this list whenever adding a provider whose runtime
    /// is hosted on the user's own infrastructure.
    #[must_use]
    pub fn is_self_hosted(self) -> bool {
        matches!(self, Self::Sglang | Self::Vllm | Self::Ollama)
    }
}

fn normalize_subagent_provider_key(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| match ch {
            '-' | '_' | '.' | ' ' => '_',
            _ => ch,
        })
        .collect()
}

fn subagent_provider_key_matches(key: &str, provider: ApiProvider) -> bool {
    if ApiProvider::parse(key).is_some_and(|candidate| candidate == provider) {
        return true;
    }

    let normalized = normalize_subagent_provider_key(key);
    if normalized == normalize_subagent_provider_key(provider.as_str()) {
        return true;
    }

    match provider {
        ApiProvider::Deepseek => matches!(
            normalized.as_str(),
            "deepseek" | "deepseek_api" | "deepseek_official"
        ),
        ApiProvider::DeepseekCN => matches!(
            normalized.as_str(),
            "deepseek_cn" | "deepseek_china" | "deepseekcn"
        ),
        ApiProvider::DeepseekAnthropic => matches!(
            normalized.as_str(),
            "deepseek_anthropic" | "deepseek_claude" | "deepseek_anthropic_api"
        ),
        ApiProvider::Openrouter => matches!(normalized.as_str(), "openrouter" | "open_router"),
        ApiProvider::OpenaiCodex => matches!(
            normalized.as_str(),
            "openai_codex" | "codex" | "chatgpt" | "openai_chatgpt"
        ),
        ApiProvider::OpenCodeZen => matches!(
            normalized.as_str(),
            "opencode_zen" | "opencode-zen" | "zen" | "opencode"
        ),
        ApiProvider::Anthropic => {
            matches!(
                normalized.as_str(),
                "anthropic" | "claude" | "anthropic_api"
            )
        }
        ApiProvider::Zai => matches!(
            normalized.as_str(),
            "zai"
                | "z_ai"
                | "glm"
                | "zai_glm"
                | "z_glm"
                | "zhipu"
                | "zhipuai"
                | "bigmodel"
                | "big_model"
                | "zhipu_glm"
        ),
        _ => false,
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
    /// OpenAI Responses API payload.
    Responses,
    /// Native Anthropic Messages API `/v1/messages` payload (#3014).
    AnthropicMessages,
}

/// Resolve the provider capability for a given [`ApiProvider`] and resolved
/// model string.
///
/// The `resolved_model` should be the final model identifier that will appear
/// in the API payload (after normalization / provider-specific mapping).
#[must_use]
pub fn provider_capability(provider: ApiProvider, resolved_model: &str) -> ProviderCapability {
    if matches!(provider, ApiProvider::Anthropic | ApiProvider::Openmodel) {
        return ProviderCapability {
            provider,
            resolved_model: resolved_model.to_string(),
            // 200K is the conservative Anthropic floor; 4.6+ models resolve
            // their 1M windows from models.rs rows (#3014).
            context_window: crate::models::context_window_for_model(resolved_model)
                .unwrap_or(200_000),
            max_output: crate::models::max_output_tokens_for_model(resolved_model)
                .unwrap_or(64_000),
            thinking_supported: crate::models::model_supports_reasoning(resolved_model),
            cache_telemetry_supported: matches!(provider, ApiProvider::Anthropic),
            request_payload_mode: RequestPayloadMode::AnthropicMessages,
            alias_deprecation: None,
        };
    }

    if matches!(
        provider,
        ApiProvider::OpenaiCodex | ApiProvider::OpenCodeZen
    ) {
        let ctx_window = if provider == ApiProvider::OpenaiCodex {
            OPENAI_CODEX_EFFECTIVE_CONTEXT_WINDOW_TOKENS
        } else {
            crate::models::context_window_for_model(resolved_model).unwrap_or(200_000)
        };
        return ProviderCapability {
            provider,
            resolved_model: resolved_model.to_string(),
            context_window: ctx_window,
            max_output: crate::models::max_output_tokens_for_model(resolved_model).unwrap_or(4096),
            thinking_supported: true,
            cache_telemetry_supported: false,
            request_payload_mode: RequestPayloadMode::Responses,
            alias_deprecation: None,
        };
    }

    // #3023: Delete the Openai/Atlascloud/Moonshot early-return so these
    // providers use the generic model-based path below, which correctly
    // resolves context windows, output limits, and thinking support from
    // models.rs lookups.  Ollama also falls through to model-based lookups
    // with 8192 as the last-resort fallback instead of a hardcoded floor.
    if matches!(provider, ApiProvider::XiaomiMimo) {
        return ProviderCapability {
            provider,
            resolved_model: resolved_model.to_string(),
            context_window: crate::models::context_window_for_model(resolved_model)
                .unwrap_or(crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS),
            max_output: crate::models::max_output_tokens_for_model(resolved_model).unwrap_or(4096),
            thinking_supported: crate::models::model_supports_reasoning(resolved_model),
            cache_telemetry_supported: false,
            request_payload_mode: RequestPayloadMode::ChatCompletions,
            alias_deprecation: None,
        };
    }

    if matches!(provider, ApiProvider::Arcee) {
        return ProviderCapability {
            provider,
            resolved_model: resolved_model.to_string(),
            context_window: crate::models::context_window_for_model(resolved_model)
                .unwrap_or(crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS),
            max_output: crate::models::max_output_tokens_for_model(resolved_model).unwrap_or(4096),
            thinking_supported: crate::models::model_supports_reasoning(resolved_model),
            cache_telemetry_supported: false,
            request_payload_mode: RequestPayloadMode::ChatCompletions,
            alias_deprecation: None,
        };
    }

    let model_lower = resolved_model.to_ascii_lowercase();
    let alias_deprecation = if matches!(
        provider,
        ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic
    ) {
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
    // to the model's own lookup or a default.  Ollama defaults to 8192
    // (conservative for small local models) instead of 128K.
    let context_window = if is_v4_pro || is_v4_flash {
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    } else if let Some(window) = crate::models::context_window_for_model(resolved_model) {
        window
    } else if matches!(provider, ApiProvider::Ollama) {
        8192
    } else {
        crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS
    };

    // Max output tokens: official DeepSeek V4 API metadata lists 384K;
    // runtime request caps remain separate and more conservative.
    let max_output = if is_v4_pro || is_v4_flash {
        384_000
    } else {
        crate::models::max_output_tokens_for_model(resolved_model).unwrap_or(4096)
    };

    // Thinking support: V4 models support thinking on all providers, but
    // only when the model name matches the V4 family.
    let thinking_supported = is_v4_pro
        || is_v4_flash
        || is_reasoner
        || crate::models::model_supports_reasoning(resolved_model);

    // Cache telemetry: returned only by DeepSeek-native and NVIDIA NIM endpoints.
    let cache_telemetry_supported = matches!(
        provider,
        ApiProvider::Deepseek
            | ApiProvider::DeepseekCN
            | ApiProvider::NvidiaNim
            | ApiProvider::Volcengine
    );

    let request_payload_mode = if matches!(
        provider,
        ApiProvider::DeepseekAnthropic | ApiProvider::Openmodel
    ) {
        RequestPayloadMode::AnthropicMessages
    } else {
        RequestPayloadMode::ChatCompletions
    };

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
        "pro" | "deepseek-v4pro" => Some("deepseek-v4-pro"),
        "flash" | "deepseek-v4flash" => Some("deepseek-v4-flash"),
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

#[must_use]
pub(crate) fn normalize_custom_model_id(model: &str) -> Option<String> {
    let trimmed = model.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Validate a user-requested model id against the active provider (#3018).
///
/// DeepSeek providers use the strict `normalize_model_name` gate (official
/// API only accepts DeepSeek IDs).  All other providers pass any non-empty,
/// non-control-character string through — the provider API is the authority.
#[must_use]
pub fn requested_model_for_provider(provider: ApiProvider, model: &str) -> Option<String> {
    match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic => {
            normalize_model_name(model)
        }
        _ => normalize_custom_model_id(model),
    }
}

/// Reject a provider/model tuple that we can be confident is invalid *before*
/// it reaches the network (#3227).
///
/// The route-isolation bug paired a model picked under one provider with a
/// different provider's route (model chip `deepseek-v4-pro`, provider badge
/// `Z.ai`), producing a `400 Unknown Model` from the upstream. This guard
/// catches that locally and names the incompatible pair instead.
///
/// We only reject tuples that are *known* to be wrong so legitimate custom
/// routing (self-hosted endpoints, OpenAI-compatible aggregators that proxy
/// DeepSeek weights, etc.) keeps working:
///
/// 1. A DeepSeek-native provider (`deepseek` / `deepseek-cn`) accepts only
///    DeepSeek model IDs or `auto` — same gate as [`normalize_model_name`].
/// 2. A non-DeepSeek *native* provider (e.g. Z.ai, which serves GLM) must not
///    be handed a DeepSeek-only model ID. This reuses the same
///    "foreign to a direct provider" classification the model resolver uses,
///    so DeepSeek aggregators (NVIDIA NIM, OpenRouter, Fireworks, …) stay
///    permissive.
///
/// Returns `Ok(())` for any tuple we cannot confidently reject (the provider
/// API remains the final authority for those).
#[cfg(test)]
pub fn validate_route(provider: ApiProvider, model: &str) -> Result<(), String> {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return Err(format!(
            "No model selected for provider '{}'.",
            provider.as_str()
        ));
    }
    if trimmed.eq_ignore_ascii_case("auto") {
        return Ok(());
    }

    // Providers whose model id is passed through verbatim (OpenAI-compatible,
    // Ollama tags, custom base URLs, …) are validated by the upstream service.
    if provider_passes_model_through(provider) {
        return Ok(());
    }

    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        if normalize_model_name(trimmed).is_some() {
            return Ok(());
        }
        return Err(format!(
            "Model '{trimmed}' is not a DeepSeek model, but the active provider is '{}'. \
             Use a DeepSeek model id (for example {}) or switch providers together with the model.",
            provider.as_str(),
            COMMON_DEEPSEEK_MODELS.join(", ")
        ));
    }

    // A non-DeepSeek native provider was handed a DeepSeek-only model id: this
    // is the exact contamination from #3227 (Z.ai + deepseek-v4-pro).
    if root_deepseek_model_is_foreign_to_direct_provider(provider, trimmed) {
        return Err(format!(
            "Model '{trimmed}' is a DeepSeek model and is not compatible with provider '{}'. \
             Switch the provider and model together, or pick a model this provider serves.",
            provider.as_str()
        ));
    }

    Ok(())
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

fn canonical_openrouter_recent_model_id(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    match normalized.as_str() {
        OPENROUTER_ARCEE_TRINITY_LARGE_THINKING_MODEL
        | "trinity"
        | "trinity-large-thinking"
        | "arcee-trinity"
        | "arcee-trinity-large-thinking" => Some(OPENROUTER_ARCEE_TRINITY_LARGE_THINKING_MODEL),
        OPENROUTER_GEMMA_4_31B_MODEL | "gemma-4-31b" | "gemma-4-31b-it" => {
            Some(OPENROUTER_GEMMA_4_31B_MODEL)
        }
        OPENROUTER_GEMMA_4_26B_A4B_MODEL | "gemma-4-26b-a4b" | "gemma-4-26b-a4b-it" => {
            Some(OPENROUTER_GEMMA_4_26B_A4B_MODEL)
        }
        OPENROUTER_GLM_5_1_MODEL | "glm-5.1" | "glm-5-1" | "zai-glm-5.1" | "zai-glm-5-1" => {
            Some(OPENROUTER_GLM_5_1_MODEL)
        }
        OPENROUTER_GLM_5_2_MODEL | "glm-5.2" | "glm-5-2" | "zai-glm-5.2" | "zai-glm-5-2" => {
            Some(OPENROUTER_GLM_5_2_MODEL)
        }
        OPENROUTER_GLM_5_TURBO_MODEL | "glm-5-turbo" | "glm-5turbo" | "zai-glm-5-turbo" => {
            Some(OPENROUTER_GLM_5_TURBO_MODEL)
        }
        OPENROUTER_KIMI_K2_7_CODE_MODEL
        | "kimi"
        | "kimi-k2"
        | "kimi-k2.7"
        | "kimi-k2-7"
        | "kimi-k2.7-code"
        | "kimi-k2-7-code"
        | "kimi-code"
        | "moonshot-kimi-k2.7-code"
        | "openrouter-kimi-k2.7-code" => Some(OPENROUTER_KIMI_K2_7_CODE_MODEL),
        OPENROUTER_KIMI_K2_6_MODEL | "kimi-k2.6" | "kimi-k2-6" | "moonshot-kimi-k2.6" => {
            Some(OPENROUTER_KIMI_K2_6_MODEL)
        }
        OPENROUTER_MINIMAX_M3_MODEL | "minimax-m3" | "minimax-m-3" => {
            Some(OPENROUTER_MINIMAX_M3_MODEL)
        }
        OPENROUTER_MINIMAX_M2_7_MODEL
        | "minimax-2.7"
        | "minimax-2-7"
        | "minimax-m2.7"
        | "minimax-m2-7"
        | "minimax-m-2.7"
        | "minimax-m-2-7" => Some(OPENROUTER_MINIMAX_M2_7_MODEL),
        OPENROUTER_NEMOTRON_3_NANO_OMNI_MODEL
        | "nemotron-3-nano-omni"
        | "nemotron-3-nano-omni-reasoning" => Some(OPENROUTER_NEMOTRON_3_NANO_OMNI_MODEL),
        OPENROUTER_NEMOTRON_3_ULTRA_MODEL
        | "nvidia/nemotron-3-ultra"
        | "nemotron-3-ultra"
        | "nemotron-3-ultra-550b-a55b"
        | "nvidia-nemotron-3-ultra"
        | "nvidia-nemotron-3-ultra-550b-a55b" => Some(OPENROUTER_NEMOTRON_3_ULTRA_MODEL),
        OPENROUTER_QWEN_3_6_35B_A3B_MODEL
        | "qwen3.6-35b-a3b"
        | "qwen-3.6-35b-a3b"
        | "qwen3-6-35b-a3b" => Some(OPENROUTER_QWEN_3_6_35B_A3B_MODEL),
        OPENROUTER_QWEN_3_6_FLASH_MODEL | "qwen3.6-flash" | "qwen-3.6-flash" => {
            Some(OPENROUTER_QWEN_3_6_FLASH_MODEL)
        }
        OPENROUTER_QWEN_3_6_MAX_PREVIEW_MODEL
        | "qwen3.6-max-preview"
        | "qwen-3.6-max-preview"
        | "qwen-max-preview" => Some(OPENROUTER_QWEN_3_6_MAX_PREVIEW_MODEL),
        OPENROUTER_QWEN_3_6_27B_MODEL | "qwen3.6-27b" | "qwen-3.6-27b" | "qwen3-6-27b" => {
            Some(OPENROUTER_QWEN_3_6_27B_MODEL)
        }
        OPENROUTER_QWEN_3_6_PLUS_MODEL | "qwen3.6-plus" | "qwen-3.6-plus" => {
            Some(OPENROUTER_QWEN_3_6_PLUS_MODEL)
        }
        OPENROUTER_QWEN_3_7_MAX_MODEL | "qwen3.7-max" | "qwen-3.7-max" => {
            Some(OPENROUTER_QWEN_3_7_MAX_MODEL)
        }
        OPENROUTER_TENCENT_HY3_PREVIEW_MODEL | "hy3-preview" | "tencent-hy3-preview" => {
            Some(OPENROUTER_TENCENT_HY3_PREVIEW_MODEL)
        }
        OPENROUTER_XIAOMI_MIMO_V2_5_PRO_MODEL
        | "mimo-v2.5-pro"
        | "mimo-v2-5-pro"
        | "xiaomi-mimo-v2.5-pro"
        | "xiaomi-mimo-v2-5-pro" => Some(OPENROUTER_XIAOMI_MIMO_V2_5_PRO_MODEL),
        OPENROUTER_XIAOMI_MIMO_V2_5_MODEL
        | "mimo-v2.5"
        | "mimo-v2-5"
        | "xiaomi-mimo-v2.5"
        | "xiaomi-mimo-v2-5" => Some(OPENROUTER_XIAOMI_MIMO_V2_5_MODEL),
        _ => None,
    }
}

fn canonical_xiaomi_mimo_model_id(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    match normalized.as_str() {
        "mimo"
        | DEFAULT_XIAOMI_MIMO_MODEL
        | "mimo-v2-5-pro"
        | "xiaomi-mimo-v2.5-pro"
        | "xiaomi-mimo-v2-5-pro" => Some(DEFAULT_XIAOMI_MIMO_MODEL),
        XIAOMI_MIMO_V2_5_PRO_ULTRASPEED_MODEL
        | "mimo-v2-5-pro-ultraspeed"
        | "xiaomi-mimo-v2.5-pro-ultraspeed"
        | "xiaomi-mimo-v2-5-pro-ultraspeed"
        | "ultraspeed"
        | "pro-ultraspeed" => Some(XIAOMI_MIMO_V2_5_PRO_ULTRASPEED_MODEL),
        "omni"
        | "mimo-omni"
        | "v2.5-omni"
        | "v25-omni"
        | "mimo-v2.5"
        | "mimo-v25"
        | "mimo-v2-5"
        | "mimo-v2.5-omni"
        | "mimo-v25-omni"
        | "mimo-v2-5-omni"
        | "xiaomi-mimo-v2.5"
        | "xiaomi-mimo-v2-5"
        | "xiaomi-mimo-v2.5-omni"
        | "xiaomi-mimo-v2-5-omni" => Some(XIAOMI_MIMO_V2_5_OMNI_MODEL),
        "asr" | "mimo-asr" | "mimo-v2.5-asr" | "speech-to-text" | "transcribe" => {
            Some(XIAOMI_MIMO_ASR_MODEL)
        }
        "mimo-tts" | "mimo-v25-tts" | "mimo-v2.5-tts" | "tts" | "speech" => {
            Some(XIAOMI_MIMO_TTS_MODEL)
        }
        "mimo-tts-voicedesign"
        | "mimo-voice-design"
        | "mimo-v25-tts-voicedesign"
        | "mimo-v2.5-tts-voicedesign"
        | "voicedesign"
        | "voice-design" => Some(XIAOMI_MIMO_TTS_VOICE_DESIGN_MODEL),
        "mimo-tts-voiceclone"
        | "mimo-voice-clone"
        | "mimo-v25-tts-voiceclone"
        | "mimo-v2.5-tts-voiceclone"
        | "voiceclone"
        | "voice-clone" => Some(XIAOMI_MIMO_TTS_VOICE_CLONE_MODEL),
        "mimo-v2-tts" => Some(XIAOMI_MIMO_V2_TTS_MODEL),
        _ => None,
    }
}

fn canonical_arcee_model_id(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    match normalized.as_str() {
        "trinity" | "arcee-trinity" | "trinity-large-thinking" | "arcee-trinity-large-thinking" => {
            Some(DEFAULT_ARCEE_MODEL)
        }
        "arcee-trinity-mini" | ARCEE_TRINITY_MINI_MODEL => Some(ARCEE_TRINITY_MINI_MODEL),
        "arcee-trinity-large-preview" | ARCEE_TRINITY_LARGE_PREVIEW_MODEL => {
            Some(ARCEE_TRINITY_LARGE_PREVIEW_MODEL)
        }
        _ => None,
    }
}

fn canonical_moonshot_model_id(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    match normalized.as_str() {
        "kimi"
        | "kimi-k2"
        | "kimi-k2.7"
        | "kimi-k2-7"
        | "kimi-k2.7-code"
        | "kimi-k2-7-code"
        | "kimi-code"
        | "moonshot-kimi-k2.7-code" => Some(DEFAULT_MOONSHOT_MODEL),
        "kimi-k2.6" | "kimi-k2-6" | "moonshot-kimi-k2.6" => Some(MOONSHOT_KIMI_K2_6_MODEL),
        _ => None,
    }
}

fn canonical_zai_model_id(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    match normalized.as_str() {
        "glm-5.1" | "glm-5-1" | "zai-glm-5.1" | "zai-glm-5-1" => Some(ZAI_GLM_5_1_MODEL),
        "glm-5.2" | "glm-5-2" | "zai-glm-5.2" | "zai-glm-5-2" => Some(DEFAULT_ZAI_MODEL),
        "glm-5-turbo" | "glm-5turbo" | "zai-glm-5-turbo" => Some(ZAI_GLM_5_TURBO_MODEL),
        _ => None,
    }
}

fn canonical_minimax_model_id(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    match normalized.as_str() {
        "minimax" | "minimax-m3" | "minimax-m-3" | "minimax-m-3-thinking" => {
            Some(DEFAULT_MINIMAX_MODEL)
        }
        "minimax-m2.7" | "minimax-m2-7" | "minimax-m-2.7" | "minimax-m-2-7" => {
            Some(MINIMAX_M2_7_MODEL)
        }
        "minimax-m2.7-highspeed"
        | "minimax-m2-7-highspeed"
        | "minimax-m-2.7-highspeed"
        | "minimax-m-2-7-highspeed" => Some(MINIMAX_M2_7_HIGHSPEED_MODEL),
        "minimax-m2.5" | "minimax-m2-5" | "minimax-m-2.5" | "minimax-m-2-5" => {
            Some(MINIMAX_M2_5_MODEL)
        }
        "minimax-m2.5-highspeed"
        | "minimax-m2-5-highspeed"
        | "minimax-m-2.5-highspeed"
        | "minimax-m-2-5-highspeed" => Some(MINIMAX_M2_5_HIGHSPEED_MODEL),
        "minimax-m2.1" | "minimax-m2-1" | "minimax-m-2.1" | "minimax-m-2-1" => {
            Some(MINIMAX_M2_1_MODEL)
        }
        "minimax-m2.1-highspeed"
        | "minimax-m2-1-highspeed"
        | "minimax-m-2.1-highspeed"
        | "minimax-m-2-1-highspeed" => Some(MINIMAX_M2_1_HIGHSPEED_MODEL),
        "minimax-m2" | "minimax-m-2" => Some(MINIMAX_M2_MODEL),
        _ => None,
    }
}

/// Resolve a user-entered model id to the canonical family id a provider
/// understands, without any wire-id translation.
///
/// Model families are treated equally: every provider-owned family (GLM via
/// Z.ai/Zhipu, Kimi, Xiaomi MiMo, MiniMax, Arcee, OpenRouter slugs, …)
/// resolves through the same "apply the family's canonical map, else pass the
/// input through" path. Nothing is rejected just because it is not a
/// DeepSeek id — the upstream API remains the final authority, mirroring how
/// the models.dev catalog (the route resolver's source of truth) carries one
/// authoritative id per offering regardless of vendor.
///
/// This is the canonicalization half of what [`normalize_model_name_for_provider`]
/// used to fuse together. Wire-id translation (e.g. `deepseek-v4-pro` → an
/// aggregator's `accounts/…/deepseek-v4-pro` slug) belongs to the route
/// resolver at request time, not to a name typed into `/provider`, so it is
/// deliberately kept out of here.
///
/// Returns `None` only for empty or control-character input; every other id
/// passes through so a custom/self-hosted endpoint is never wrongly rejected.
#[must_use]
pub fn canonical_model_id_for_provider(provider: ApiProvider, model: &str) -> Option<String> {
    let trimmed = model.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return None;
    }

    // Provider-owned model families resolve through their own canonical map,
    // which defines the authoritative casing (`glm-5.1` → `GLM-5.1`,
    // `minimax-m2.7` → `MiniMax-M2.7`). Each map recognizes only *its own*
    // aliases, so an unknown id falls through to passthrough — no family acts
    // as a gate against any other.
    let family_canonical: Option<&'static str> = match provider {
        ApiProvider::Openrouter => canonical_openrouter_recent_model_id(trimmed),
        ApiProvider::XiaomiMimo => canonical_xiaomi_mimo_model_id(trimmed),
        ApiProvider::Arcee => canonical_arcee_model_id(trimmed),
        ApiProvider::Moonshot => canonical_moonshot_model_id(trimmed),
        ApiProvider::Zai => canonical_zai_model_id(trimmed),
        ApiProvider::Minimax => canonical_minimax_model_id(trimmed),
        _ => None,
    };
    if let Some(canonical) = family_canonical {
        return Some(canonical.to_string());
    }

    // The official DeepSeek API is the one legitimate per-family gate: it serves
    // only its own ids (and 400s anything else), so reject an id it does not
    // recognize. Compact aliases are rewritten (deepseek-v4pro → deepseek-v4-pro)
    // and the caller's casing is kept for an already-valid id (`DeepSeek-V4-Flash`
    // stays as-is). Custom/self-hosted DeepSeek endpoints take the
    // accepts-custom-model-ids path, so they never reach this gate.
    if matches!(
        provider,
        ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic
    ) {
        let normalized = normalize_model_name(trimmed)?;
        if let Some(canonical) = canonical_official_deepseek_model_id(&normalized) {
            if canonical.eq_ignore_ascii_case(&normalized)
                || normalized.to_ascii_lowercase() == canonical
            {
                return Some(normalized);
            }
            return Some(canonical.to_string());
        }
        return Some(normalized);
    }

    // Aggregators that host DeepSeek (NIM, Novita, Fireworks, SiliconFlow, SGLang,
    // vLLM, DeepInfra, Wanjie Ark, Volcengine) canonicalize recognized DeepSeek
    // ids but pass everything else through — they serve more than DeepSeek, so
    // the upstream API stays the authority. A name is never rejected here.
    if matches!(
        provider,
        ApiProvider::NvidiaNim
            | ApiProvider::Novita
            | ApiProvider::Fireworks
            | ApiProvider::Siliconflow
            | ApiProvider::SiliconflowCn
            | ApiProvider::Sglang
            | ApiProvider::Vllm
            | ApiProvider::Deepinfra
            | ApiProvider::WanjieArk
            | ApiProvider::Volcengine
    ) && let Some(canonical) = canonical_official_deepseek_model_id(
        &normalize_model_name(trimmed).unwrap_or_else(|| trimmed.to_string()),
    ) {
        return Some(canonical.to_string());
    }

    // Everything else (HuggingFace, OpenAI-compatible, Qianfan, StepFun, Codex,
    // Anthropic) owns no canonical map — the id the user typed is authoritative.
    Some(trimmed.to_string())
}

/// Normalize a model selected through the TUI for the active provider, applying
/// the provider's wire-slug translation on top of the canonical family id.
///
/// This is the wire-id half of the split (canonicalization lives in
/// [`canonical_model_id_for_provider`]). Used by config-file normalization,
/// where vendor-prefixed ids (e.g. `deepseek-ai/DeepSeek-V4-Pro` on SiliconFlow)
/// are the stored form. `/provider` deliberately uses the canonical half instead.
#[must_use]
pub fn normalize_model_name_for_provider(provider: ApiProvider, model: &str) -> Option<String> {
    let canonical = canonical_model_id_for_provider(provider, model)?;
    // Translate the canonical family id to the provider's wire slug when the
    // provider's API uses vendor-prefixed ids (Together, Siliconflow, NIM, …).
    // `model_for_provider` is a no-op for providers without a wire-slug map, so
    // this is one uniform layer over the equal-treatment canonical resolver.
    Some(model_for_provider(provider, canonical))
}

#[must_use]
pub fn wire_model_for_provider(provider: ApiProvider, model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return trimmed.to_string();
    }
    if matches!(provider, ApiProvider::XiaomiMimo) {
        return normalize_model_name_for_provider(provider, trimmed)
            .unwrap_or_else(|| trimmed.to_string());
    }
    if provider_passes_model_through(provider) {
        return trimmed.to_string();
    }
    normalize_model_name_for_provider(provider, trimmed).unwrap_or_else(|| trimmed.to_string())
}

#[must_use]
pub fn model_completion_names_for_provider(provider: ApiProvider) -> Vec<&'static str> {
    match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic => {
            OFFICIAL_DEEPSEEK_MODELS.to_vec()
        }
        ApiProvider::NvidiaNim => vec![DEFAULT_NVIDIA_NIM_MODEL, DEFAULT_NVIDIA_NIM_FLASH_MODEL],
        ApiProvider::Openrouter => {
            let mut models = vec![DEFAULT_OPENROUTER_MODEL, DEFAULT_OPENROUTER_FLASH_MODEL];
            models.extend_from_slice(RECENT_OPENROUTER_LARGE_MODELS);
            models
        }
        ApiProvider::XiaomiMimo => vec![
            DEFAULT_XIAOMI_MIMO_MODEL,
            XIAOMI_MIMO_V2_5_PRO_ULTRASPEED_MODEL,
            XIAOMI_MIMO_V2_5_OMNI_MODEL,
        ],
        ApiProvider::Novita => vec![DEFAULT_NOVITA_MODEL, DEFAULT_NOVITA_FLASH_MODEL],
        ApiProvider::Fireworks => vec![DEFAULT_FIREWORKS_MODEL],
        ApiProvider::Siliconflow | ApiProvider::SiliconflowCn => {
            vec![DEFAULT_SILICONFLOW_MODEL, DEFAULT_SILICONFLOW_FLASH_MODEL]
        }
        ApiProvider::Arcee => vec![DEFAULT_ARCEE_MODEL, ARCEE_TRINITY_LARGE_PREVIEW_MODEL],
        ApiProvider::Moonshot => vec![DEFAULT_MOONSHOT_MODEL],
        ApiProvider::Huggingface => {
            vec![DEFAULT_HUGGINGFACE_MODEL, DEFAULT_HUGGINGFACE_FLASH_MODEL]
        }
        ApiProvider::Deepinfra => vec![DEFAULT_DEEPINFRA_MODEL, DEFAULT_DEEPINFRA_FLASH_MODEL],
        ApiProvider::WanjieArk => {
            vec![
                DEFAULT_WANJIE_ARK_MODEL,
                "deepseek-v4-pro",
                "deepseek-v4-flash",
            ]
        }
        ApiProvider::Sglang => vec![DEFAULT_SGLANG_MODEL, DEFAULT_SGLANG_FLASH_MODEL],
        ApiProvider::Vllm => vec![DEFAULT_VLLM_MODEL, DEFAULT_VLLM_FLASH_MODEL],
        ApiProvider::Volcengine => vec![DEFAULT_VOLCENGINE_MODEL, DEFAULT_VOLCENGINE_FLASH_MODEL],
        ApiProvider::Ollama => Vec::new(),
        ApiProvider::Openai | ApiProvider::Atlascloud => OFFICIAL_DEEPSEEK_MODELS.to_vec(),
        ApiProvider::Together => vec![DEFAULT_TOGETHER_MODEL, DEFAULT_TOGETHER_FLASH_MODEL],
        ApiProvider::Qianfan => vec![DEFAULT_QIANFAN_MODEL],
        ApiProvider::OpenaiCodex => vec![DEFAULT_OPENAI_CODEX_MODEL],
        ApiProvider::OpenCodeZen => vec![DEFAULT_OPENCODE_ZEN_MODEL],
        ApiProvider::Openmodel => vec![DEFAULT_OPENMODEL_MODEL],
        ApiProvider::Zai => vec![DEFAULT_ZAI_MODEL, ZAI_GLM_5_1_MODEL, ZAI_GLM_5_TURBO_MODEL],
        ApiProvider::Stepfun => vec![DEFAULT_STEPFUN_MODEL],
        ApiProvider::Anthropic => vec![
            ANTHROPIC_OPUS_MODEL,
            DEFAULT_ANTHROPIC_MODEL,
            ANTHROPIC_HAIKU_MODEL,
        ],
        ApiProvider::Minimax => vec![
            DEFAULT_MINIMAX_MODEL,
            MINIMAX_M2_7_MODEL,
            MINIMAX_M2_7_HIGHSPEED_MODEL,
            MINIMAX_M2_5_MODEL,
            MINIMAX_M2_5_HIGHSPEED_MODEL,
            MINIMAX_M2_1_MODEL,
            MINIMAX_M2_1_HIGHSPEED_MODEL,
            MINIMAX_M2_MODEL,
        ],
        ApiProvider::Sakana => vec![DEFAULT_SAKANA_MODEL, SAKANA_FUGU_ULTRA_MODEL],
        // Custom endpoints expose no built-in completion names; the user
        // supplies their own model id (#1519).
        ApiProvider::Custom => Vec::new(),
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
    /// Per-SSE-chunk idle timeout in seconds. Defaults to 300 seconds when
    /// omitted. `0` maps to the default; values clamp to `1..=3600`.
    pub stream_chunk_timeout_secs: Option<u64>,
    /// Ordered list of footer items the user wants visible. `None` (the field
    /// missing from `config.toml`) means "use the built-in default order"; an
    /// empty `Some(vec![])` means "show nothing in the footer".
    ///
    /// Edited interactively via `/statusline`; persisted to `tui.status_items`
    /// in `~/.deepseek/config.toml`.
    #[serde(default, deserialize_with = "deser_status_items")]
    pub status_items: Option<Vec<StatusItem>>,
    /// Emit OSC 8 hyperlink escape sequences around URLs in the transcript so
    /// supporting terminals (iTerm2, Terminal.app 13+, Ghostty, Kitty,
    /// WezTerm, Alacritty, recent gnome-terminal/konsole) make them
    /// Cmd+click-openable. Terminals without OSC 8 support render the plain
    /// label and ignore the escape. Defaults to on for macOS/Linux and off for
    /// Windows legacy consoles; set `false` to suppress everywhere (e.g. for a
    /// terminal that misrenders the sequence). OSC 8 escapes are emitted
    /// out-of-band, so buffer-column corruption is not a concern.
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
    /// Play a configured WAV sound file.
    File,
}

/// Desktop-notification configuration (OSC 9 / BEL on turn completion).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct NotificationsConfig {
    /// Delivery method: `auto` | `osc9` | `bel` | `off`. Default: `auto`.
    /// `auto` resolves to OSC 9 for iTerm.app / Ghostty / WezTerm / Cmux
    /// (detected via `$TERM_PROGRAM` then `$LC_TERMINAL`); otherwise it
    /// falls back to BEL. On Windows the BEL path is routed through
    /// `MessageBeep(MB_OK)`.
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

    /// Completion sound: `"off"` | `"beep"` | `"bell"` | `"file"`. Default: `"beep"`.
    /// Plays a sound when every turn finishes (alongside the ✅ marker).
    #[serde(default)]
    pub completion_sound: CompletionSound,

    /// Path to the WAV sound file used when `completion_sound = "file"`.
    #[serde(default)]
    pub sound_file: Option<PathBuf>,
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
    /// When `true`, deprecate the in-repo `memory.rs` push/inject path
    /// (`<user_memory>` block + `remember` tool + `# foo` quick-add) in
    /// favor of Moraine pull/recall via its MCP tools. The old path is
    /// skipped even when `enabled = true`. Default `false`.
    #[serde(default)]
    pub moraine_fallback: Option<bool>,
}

/// Xiaomi MiMo speech/TTS output configuration.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SpeechConfig {
    /// Default directory for generated speech/TTS files when no explicit
    /// output path is provided.
    #[serde(default)]
    pub output_dir: Option<String>,
}

impl SnapshotsConfig {
    #[must_use]
    pub fn max_age(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.max_age_days.saturating_mul(24 * 60 * 60))
    }
}

// Web-search `[search]` table types live in the `search` leaf module and are
// re-exported below so `crate::config::SearchProvider` (and siblings) resolve
// unchanged (#3311).
mod search;
pub use search::*;

/// Model-visible tool catalog controls (`[tools]` table in config.toml).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ToolsConfig {
    /// Native tool names to keep loaded even when they are outside the small
    /// default core catalog. Unknown names are harmless and simply never match.
    #[serde(default)]
    pub always_load: Vec<String>,

    /// Optional directory to scan for plugin tool scripts. Scripts with a
    /// frontmatter header (`# name:`, `# description:`, `# schema:`) are
    /// auto-discovered and registered as tools.
    ///
    /// Defaults to `~/.codewhale/tools/` when `None`.
    #[serde(default)]
    pub plugin_dir: Option<String>,

    /// Per-tool overrides keyed by built-in tool name.
    /// Each override replaces or disables the named tool.
    #[serde(default)]
    pub overrides: Option<HashMap<String, ToolOverride>>,
}

/// One configurable footer item.
///
/// Order in the user's `Vec<StatusItem>` is preserved: items in the left
/// cluster (`Mode`, `Model`, `Cost`, `Status`) render in the order given;
/// right-cluster chips (`Agents`, `ReasoningReplay`, `PrefixStability`,
/// `Cache`, `ContextPercent`, `GitBranch`, `LastToolElapsed`, `RateLimit`)
/// likewise honour ordering inside their cluster. The split between left and right is deliberate — left holds steady
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
    /// Activity label: "idle" / "busy" / "draft" / "working".
    Status,
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
            StatusItem::Status => "Activity (idle/busy/draft/working)",
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
            StatusItem::Agents => "agents or RLM work in progress",
            StatusItem::ReasoningReplay => "thinking tokens replayed each turn",
            StatusItem::PrefixStability => "whether system/tools stayed cacheable",
            StatusItem::Cache => "% of prompt served from cache",
            StatusItem::ContextPercent => "tokens used / model context window",
            StatusItem::GitBranch => "current workspace branch",
            StatusItem::LastToolElapsed => "ms of the most recent tool call (reserved)",
            StatusItem::RateLimit => "remaining requests in the budget (reserved)",
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
    /// Model used for seam/briefing work. Default: "deepseek-v4-flash".
    #[serde(default)]
    pub seam_model: Option<String>,
}

/// Sub-agent model overrides. Keys in `models` can be role names (`worker`,
/// `explorer`, `awaiter`) or type names (`general`, `explore`, `plan`,
/// `review`, `custom`). Per-call explicit model choices still win.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SubagentsConfig {
    /// Top-level switch for the model-facing `agent` tool. `None` preserves
    /// the feature-flag default; `false` hides/refuses sub-agent spawning
    /// without changing the numeric queue/depth knobs.
    #[serde(default)]
    pub enabled: Option<bool>,
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
    /// How many levels of nested sub-agents the interactive `agent` tool may
    /// spawn. `0` blocks the model-facing `agent` tool at this runtime depth;
    /// use `[subagents] enabled = false` for the clearer durable off switch.
    /// `1` allows one level, `2` two, and so on. When unset, defaults to
    /// [`codewhale_config::DEFAULT_SPAWN_DEPTH`]; any value is clamped to
    /// [`codewhale_config::MAX_SPAWN_DEPTH_CEILING`]. Fleet workers are
    /// governed separately by `[fleet.exec] max_spawn_depth`; both share the
    /// same default and ceiling so the limit cannot drift.
    #[serde(default)]
    pub max_depth: Option<u32>,
    /// Number of direct (depth-1) sub-agents that may execute concurrently
    /// before further launches queue for a launch slot (#3095). When unset,
    /// defaults to the full resolved `max_subagents()` (no artificial
    /// throttle); explicit values are clamped to [1, max_subagents].
    #[serde(default)]
    pub launch_concurrency: Option<usize>,
    /// Maximum queued + running sub-agents admitted for one session. Defaults
    /// to a large bounded queue while `launch_concurrency` keeps instantaneous
    /// execution bounded.
    #[serde(default, alias = "max_total", alias = "admission_limit")]
    pub max_admitted: Option<usize>,
    /// Optional aggregate token budget shared by a root `agent` run and its
    /// descendants. When unset or 0, sub-agents keep legacy unlimited spend
    /// behavior unless an individual `agent` call supplies a per-run override.
    #[serde(default)]
    pub token_budget: Option<u64>,
    /// Deprecated pre-v0.8.61 alias for `launch_concurrency`. Honored only
    /// when `launch_concurrency` is unset, so the new key always wins.
    #[serde(default, rename = "interactive_max_launch")]
    pub interactive_max_launch_legacy: Option<usize>,
    /// Per-step DeepSeek API timeout for sub-agent requests, in seconds. The
    /// timeout wraps `client.create_message` so a stuck single step cannot
    /// pin the parent's parent-completion wakeup channel indefinitely.
    /// Defaults to `DEFAULT_SUBAGENT_API_TIMEOUT_SECS` (120) and is clamped
    /// to `MIN_SUBAGENT_API_TIMEOUT_SECS..=MAX_SUBAGENT_API_TIMEOUT_SECS`
    /// (1..=1800). Zero or unset uses the legacy 120s default (#1806, #1808).
    #[serde(default)]
    pub api_timeout_secs: Option<u64>,
    /// Wall-clock timeout for a running sub-agent that stops making
    /// manager-visible progress. Defaults to 5 minutes and is kept above the
    /// per-step API timeout so slow but legitimate model calls are not
    /// cancelled before their request timeout can fire (#2614).
    #[serde(default)]
    pub heartbeat_timeout_secs: Option<u64>,
    /// Per-provider overrides for sub-agent fanout and budget knobs. Keys are
    /// provider names such as `deepseek`, `zai`, `openrouter`, or `anthropic`.
    #[serde(default)]
    pub providers: Option<HashMap<String, SubagentProviderConfig>>,
}

/// Provider-specific sub-agent limit overrides.
///
/// Every field inherits from `[subagents]` when unset, so a provider profile
/// can tighten only the knobs that matter for that API's rate limits.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SubagentProviderConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub max_concurrent: Option<usize>,
    #[serde(default)]
    pub max_depth: Option<u32>,
    #[serde(default)]
    pub launch_concurrency: Option<usize>,
    #[serde(default, alias = "max_total", alias = "admission_limit")]
    pub max_admitted: Option<usize>,
    #[serde(default)]
    pub token_budget: Option<u64>,
    #[serde(default)]
    pub api_timeout_secs: Option<u64>,
    #[serde(default)]
    pub heartbeat_timeout_secs: Option<u64>,
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

fn default_update_check_for_updates() -> bool {
    true
}

/// Startup update-check configuration (`[update]` table in config.toml).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct UpdateConfig {
    /// When false, skip the TUI startup background update check entirely.
    #[serde(default = "default_update_check_for_updates")]
    pub check_for_updates: bool,
    /// Optional GitHub-compatible latest-release JSON endpoint.
    #[serde(default)]
    pub update_uri: Option<String>,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            check_for_updates: true,
            update_uri: None,
        }
    }
}

impl UpdateConfig {
    #[must_use]
    pub fn update_uri(&self) -> Option<&str> {
        self.update_uri
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }
}

/// Resolved CLI configuration, including defaults and environment overrides.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    pub provider: Option<String>,
    #[serde(alias = "apiKey")]
    pub api_key: Option<String>,
    #[serde(alias = "baseUrl")]
    pub base_url: Option<String>,
    /// Optional extra HTTP headers sent to model API requests.
    #[serde(alias = "httpHeaders")]
    pub http_headers: Option<HashMap<String, String>>,
    #[serde(alias = "defaultTextModel")]
    pub default_text_model: Option<String>,
    #[serde(alias = "authMode")]
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
    pub mcp_oauth_callback_port: Option<u16>,
    pub mcp_oauth_callback_url: Option<String>,
    pub notes_path: Option<String>,
    pub memory_path: Option<String>,
    /// When true, set `tool_choice: "required"` and opt compatible function
    /// schemas into DeepSeek beta strict mode. Schemas with root alternatives
    /// stay non-strict to avoid changing optional/one-of tool semantics.
    pub strict_tool_mode: Option<bool>,
    /// Additional user-owned system-prompt sources concatenated in declared
    /// order (#454). Paths are expanded via `expand_path` so `~` and env vars
    /// work. Project-scope config is not allowed to set this field; the TUI
    /// project overlay ignores `instructions` so a cloned repo cannot choose
    /// arbitrary local files to place into the prompt. Each configured file is
    /// loaded, capped at 100 KiB, and skipped (with a warning) on read errors so
    /// a missing optional file doesn't fail the launch.
    pub instructions: Option<Vec<String>>,
    pub allow_shell: Option<bool>,
    /// Opt-in ghost-text follow-up prompt suggestion after each completed turn.
    /// Default: false — the user must explicitly set this to true to enable.
    pub prompt_suggestion: Option<bool>,
    #[serde(alias = "approvalPolicy")]
    pub approval_policy: Option<String>,
    #[serde(alias = "sandboxMode")]
    pub sandbox_mode: Option<String>,
    #[serde(default, alias = "fallbackProviders")]
    pub fallback_providers: Vec<codewhale_config::ProviderKind>,
    pub yolo: Option<bool>,
    pub verbosity: Option<String>,
    /// External sandbox backend: `"none"` or `"opensandbox"`.
    /// When set, exec_shell routes commands through the backend's HTTP API
    /// instead of spawning a local process.
    #[serde(alias = "sandboxBackend")]
    pub sandbox_backend: Option<String>,
    /// Base URL for the external sandbox backend (default: `"http://localhost:8080"`).
    #[serde(alias = "sandboxUrl")]
    pub sandbox_url: Option<String>,
    /// Optional API key for the external sandbox backend (sent as Bearer token).
    #[serde(alias = "sandboxApiKey")]
    pub sandbox_api_key: Option<String>,
    /// When true and `/usr/bin/bwrap` is present on Linux, route exec_shell
    /// through bubblewrap instead of relying solely on Landlock (#2184).
    /// Defaults to false. Requires the `bubblewrap` package to be installed
    /// separately — we do NOT vendor bwrap.
    #[serde(alias = "preferBwrap")]
    pub prefer_bwrap: Option<bool>,
    #[serde(alias = "managedConfigPath")]
    pub managed_config_path: Option<String>,
    #[serde(alias = "requirementsPath")]
    pub requirements_path: Option<String>,
    #[serde(alias = "maxSubagents")]
    pub max_subagents: Option<usize>,
    pub retry: Option<RetryConfig>,
    pub features: Option<FeaturesToml>,

    /// Deterministic user-level auto-review policy for tool calls. The engine
    /// applies these rules after built-in safety floors, so config cannot
    /// bypass publish/destructive-background holds.
    #[serde(default)]
    pub auto_review: Option<AutoReviewConfig>,

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

    /// Verifier-preview behavior (#2093). When absent, automatic verifier
    /// preview stays off and verifier verdicts use the hunt policy.
    #[serde(default)]
    pub verifier: Option<codewhale_config::VerifierConfigToml>,

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
    /// Set `provider` to another supported backend such as `bing`, `tavily`,
    /// `bocha`, `metaso`, `searxng`, `baidu`, `volcengine`, or `sofya`.
    /// API-backed services require provider-specific credentials; SearXNG
    /// requires a trusted `base_url`.
    #[serde(default)]
    pub search: Option<SearchConfig>,

    /// User-level memory file (#489). Default behaviour is **opt-in**:
    /// loading + injection happens only when `[memory] enabled = true` or
    /// `DEEPSEEK_MEMORY=on` is set.
    ///
    /// v0.8.66 deprecates this in favour of Moraine MCP recall. Set
    /// `[memory] moraine_fallback = true` to skip the legacy push/inject
    /// path while keeping Moraine's pull/recall tools.
    #[serde(default)]
    pub memory: Option<MemoryConfig>,

    /// Xiaomi MiMo speech/TTS defaults.
    #[serde(default)]
    pub speech: Option<SpeechConfig>,

    /// Tunables for `--model auto` (#1207). When absent, the auto router
    /// keeps its existing balanced behaviour.
    #[serde(default)]
    pub auto: Option<AutoConfig>,

    /// Optional 1-8 hotbar slot bindings (#2064). When absent, hotbar UI and
    /// dispatch layers use the built-in defaults from `codewhale_config`.
    #[serde(default)]
    pub hotbar: Option<Vec<codewhale_config::HotbarBindingToml>>,

    /// Startup update-check behavior. When absent, the TUI keeps the default
    /// fire-and-forget latest-release check.
    #[serde(default)]
    pub update: Option<UpdateConfig>,

    /// Post-edit LSP diagnostics injection (#136). When absent, the engine
    /// applies the defaults documented in [`LspConfigToml`].
    #[serde(default)]
    pub lsp: Option<LspConfigToml>,

    /// Append-only layered context management with Flash seam manager (#159).
    #[serde(default)]
    pub context: ContextConfig,

    /// Agent Fleet trust/security/role/exec config.
    #[serde(default)]
    pub fleet: Option<codewhale_config::FleetConfigToml>,

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

    /// Sibling `permissions.toml` ask-rules compiled for runtime checks.
    ///
    /// This is deliberately not part of `config.toml`; it is loaded from the
    /// companion permissions file after profile/env/managed config resolution.
    #[serde(skip)]
    pub exec_policy_engine: ExecPolicyEngine,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AutoReviewConfig {
    #[serde(default, alias = "guidance", alias = "naturalLanguageGuidance")]
    pub natural_language_guidance: Option<String>,
    #[serde(default)]
    pub allow: Vec<AutoReviewRuleConfig>,
    #[serde(default)]
    pub block: Vec<AutoReviewRuleConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AutoReviewRuleConfig {
    pub id: Option<String>,
    #[serde(default, alias = "toolName", alias = "tool_name")]
    pub tool: Option<String>,
    #[serde(default, alias = "actionKind", alias = "action_kind")]
    pub action_kind: Option<String>,
    #[serde(default, alias = "textContains", alias = "text_contains")]
    pub text_contains: Option<String>,
    pub reason: Option<String>,
}

impl AutoReviewConfig {
    fn to_runtime_policy(&self) -> crate::tui::auto_review::AutoReviewPolicy {
        crate::tui::auto_review::AutoReviewPolicy {
            allow_rules: self
                .allow
                .iter()
                .enumerate()
                .map(|(index, rule)| {
                    rule.to_runtime_rule(index, crate::tui::auto_review::AutoReviewAction::Allow)
                })
                .collect(),
            block_rules: self
                .block
                .iter()
                .enumerate()
                .map(|(index, rule)| {
                    rule.to_runtime_rule(index, crate::tui::auto_review::AutoReviewAction::Block)
                })
                .collect(),
            natural_language_guidance: self
                .natural_language_guidance
                .as_ref()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        }
    }

    fn validate(&self) -> Result<()> {
        validate_auto_review_rules("allow", &self.allow)?;
        validate_auto_review_rules("block", &self.block)?;
        Ok(())
    }
}

impl AutoReviewRuleConfig {
    fn to_runtime_rule(
        &self,
        index: usize,
        action: crate::tui::auto_review::AutoReviewAction,
    ) -> crate::tui::auto_review::AutoReviewRule {
        let id_prefix = match action {
            crate::tui::auto_review::AutoReviewAction::Allow => "allow",
            crate::tui::auto_review::AutoReviewAction::Block => "block",
            crate::tui::auto_review::AutoReviewAction::AskUser => "ask",
            crate::tui::auto_review::AutoReviewAction::HoldForReview => "hold",
        };
        let id = self
            .id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("config-{id_prefix}-{index}"));
        let reason = self
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("configured auto-review {id_prefix} rule"));
        let mut rule = match action {
            crate::tui::auto_review::AutoReviewAction::Allow => {
                crate::tui::auto_review::AutoReviewRule::allow(id, reason)
            }
            crate::tui::auto_review::AutoReviewAction::Block => {
                crate::tui::auto_review::AutoReviewRule::block(id, reason)
            }
            crate::tui::auto_review::AutoReviewAction::AskUser
            | crate::tui::auto_review::AutoReviewAction::HoldForReview => {
                crate::tui::auto_review::AutoReviewRule::block(id, reason)
            }
        };

        if let Some(tool) = self
            .tool
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            rule = rule.tool_name(tool.to_string());
        }
        if let Some(action_kind) = self
            .action_kind
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(parse_auto_review_action_kind)
        {
            rule = rule.action_kind(action_kind);
        }
        if let Some(text) = self
            .text_contains
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            rule = rule.text_contains(text.to_string());
        }

        rule
    }

    fn has_matcher(&self) -> bool {
        self.tool
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || self
                .action_kind
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            || self
                .text_contains
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    }
}

fn validate_auto_review_rules(kind: &str, rules: &[AutoReviewRuleConfig]) -> Result<()> {
    for (index, rule) in rules.iter().enumerate() {
        if !rule.has_matcher() {
            anyhow::bail!(
                "Invalid auto_review.{kind}[{index}]: set at least one of tool, action_kind, or text_contains."
            );
        }
        if let Some(action_kind) = rule.action_kind.as_deref()
            && parse_auto_review_action_kind(action_kind.trim()).is_none()
        {
            anyhow::bail!(
                "Invalid auto_review.{kind}[{index}].action_kind '{action_kind}': expected read, write, shell, network, git, mcp_read, mcp_action, browser, secret, publish, destructive, or unknown."
            );
        }
    }
    Ok(())
}

fn parse_auto_review_action_kind(raw: &str) -> Option<crate::tui::auto_review::ToolActionKind> {
    match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "read" => Some(crate::tui::auto_review::ToolActionKind::Read),
        "write" => Some(crate::tui::auto_review::ToolActionKind::Write),
        "shell" => Some(crate::tui::auto_review::ToolActionKind::Shell),
        "network" => Some(crate::tui::auto_review::ToolActionKind::Network),
        "git" => Some(crate::tui::auto_review::ToolActionKind::Git),
        "mcp_read" => Some(crate::tui::auto_review::ToolActionKind::McpRead),
        "mcp_action" => Some(crate::tui::auto_review::ToolActionKind::McpAction),
        "browser" => Some(crate::tui::auto_review::ToolActionKind::Browser),
        "secret" => Some(crate::tui::auto_review::ToolActionKind::Secret),
        "publish" => Some(crate::tui::auto_review::ToolActionKind::Publish),
        "destructive" => Some(crate::tui::auto_review::ToolActionKind::Destructive),
        "unknown" => Some(crate::tui::auto_review::ToolActionKind::Unknown),
        _ => None,
    }
}

/// How a user wants to replace or disable a built-in tool.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOverride {
    /// Run a local script file. The script receives the tool's JSON input
    /// on stdin and must return a JSON `ToolResult` on stdout.
    Script {
        /// Path to the script (absolute, or relative to `~/.codewhale/tools/`).
        path: String,
        /// Optional static arguments prepended before the tool's JSON input.
        #[serde(default)]
        args: Option<Vec<String>>,
    },
    /// Run an external command. The command receives the tool's JSON input
    /// on stdin and must return a JSON `ToolResult` on stdout.
    Command {
        /// The command to run (binary name or absolute path).
        command: String,
        /// Optional static arguments prepended before the tool's JSON input.
        #[serde(default)]
        args: Option<Vec<String>>,
    },
    /// Completely disable a built-in tool. The tool will not appear in the
    /// model-visible catalog and cannot be called.
    Disabled,
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
    /// When true, skill discovery scans only CodeWhale-owned skill roots
    /// (plus any explicit `skills_dir`) instead of importing compatible
    /// directories from other AI tools such as Claude, OpenCode, or Cursor.
    #[serde(default, alias = "scanCodewhaleOnly")]
    pub scan_codewhale_only: Option<bool>,
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

    /// Resolve whether session-time discovery should ignore cross-tool skill
    /// directories. Defaults to the compatibility-preserving broad scan.
    #[must_use]
    pub fn scan_codewhale_only(&self) -> bool {
        self.scan_codewhale_only.unwrap_or(false)
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
    /// User-defined LSP servers for file extensions not in the built-in
    /// registry. Keyed by extension (e.g. `"php"`, `"rb"`).
    #[serde(default)]
    pub custom: Option<HashMap<String, crate::lsp::CustomLspDef>>,
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
            custom: self.custom.unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProviderConfig {
    #[serde(alias = "apiKey")]
    pub api_key: Option<String>,
    #[serde(alias = "baseUrl")]
    pub base_url: Option<String>,
    pub model: Option<String>,
    #[serde(
        default,
        alias = "contextWindow",
        alias = "context_window_tokens",
        alias = "contextWindowTokens",
        alias = "context_length",
        alias = "contextLength"
    )]
    pub context_window: Option<u32>,
    pub mode: Option<String>,
    #[serde(alias = "authMode")]
    pub auth_mode: Option<String>,
    #[serde(alias = "insecureSkipTlsVerify")]
    pub insecure_skip_tls_verify: Option<bool>,
    #[serde(alias = "httpHeaders")]
    pub http_headers: Option<HashMap<String, String>>,
    #[serde(alias = "pathSuffix")]
    pub path_suffix: Option<String>,
    #[serde(alias = "reasoningStyle", alias = "reasoningStreamStyle")]
    pub reasoning_stream_style: Option<String>,
    #[serde(
        default,
        alias = "max-concurrency",
        alias = "maxConcurrency",
        alias = "concurrency"
    )]
    pub max_concurrency: Option<usize>,
    pub auth: Option<codewhale_config::ProviderAuthSourceToml>,
    /// Wire-protocol selector for a custom `[providers.<name>]` entry (#1519).
    ///
    /// Only `"openai-compatible"` is accepted for now; any other value is
    /// rejected at selection time so unsupported wire formats fail loudly rather
    /// than silently routing as OpenAI. Built-in providers leave this unset.
    #[serde(default)]
    pub kind: Option<String>,
    /// Name of the environment variable holding this custom provider's API key
    /// (#1519), e.g. `api_key_env = "EXAMPLE_API_KEY"`. The key value itself is
    /// never stored in config; only the env var name is.
    #[serde(default, alias = "apiKeyEnv")]
    pub api_key_env: Option<String>,
}

impl ProviderConfig {
    /// True when this entry selects the OpenAI-compatible custom wire protocol.
    ///
    /// `kind` is matched case-insensitively against `openai-compatible` (and the
    /// `openai_compatible` underscore spelling). Returns `false` when `kind` is
    /// unset (built-in providers) or names any other value.
    #[must_use]
    pub fn is_openai_compatible_custom(&self) -> bool {
        self.kind.as_deref().is_some_and(|kind| {
            let normalized = kind.trim().to_ascii_lowercase().replace('_', "-");
            normalized == "openai-compatible"
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProvidersConfig {
    #[serde(default)]
    pub deepseek: ProviderConfig,
    #[serde(default, alias = "deepseekCn")]
    pub deepseek_cn: ProviderConfig,
    #[serde(
        default,
        alias = "deepseek-anthropic",
        alias = "deepseekAnthropic",
        alias = "deepseek-claude",
        alias = "deepseek_claude"
    )]
    pub deepseek_anthropic: ProviderConfig,
    #[serde(default, alias = "nvidiaNim")]
    pub nvidia_nim: ProviderConfig,
    #[serde(default)]
    pub openai: ProviderConfig,
    #[serde(default)]
    pub atlascloud: ProviderConfig,
    #[serde(default, alias = "wanjieArk")]
    pub wanjie_ark: ProviderConfig,
    #[serde(default)]
    pub volcengine: ProviderConfig,
    #[serde(default)]
    pub openrouter: ProviderConfig,
    #[serde(
        default,
        alias = "xiaomi",
        alias = "mimo",
        alias = "xiaomimimo",
        alias = "xiaomiMimo"
    )]
    pub xiaomi_mimo: ProviderConfig,
    #[serde(default)]
    pub novita: ProviderConfig,
    #[serde(default)]
    pub fireworks: ProviderConfig,
    #[serde(default)]
    pub siliconflow: ProviderConfig,
    #[serde(
        default,
        alias = "siliconflow-CN",
        alias = "siliconflow-cn",
        alias = "siliconflowCn"
    )]
    pub siliconflow_cn: ProviderConfig,
    #[serde(default)]
    pub arcee: ProviderConfig,
    #[serde(default)]
    pub moonshot: ProviderConfig,
    #[serde(default)]
    pub sglang: ProviderConfig,
    #[serde(default)]
    pub vllm: ProviderConfig,
    #[serde(default)]
    pub ollama: ProviderConfig,
    #[serde(default, alias = "hugging-face", alias = "hf")]
    pub huggingface: ProviderConfig,
    #[serde(default, alias = "deep-infra", alias = "deep_infra")]
    pub deepinfra: ProviderConfig,
    #[serde(default, alias = "together-ai")]
    pub together: ProviderConfig,
    #[serde(
        default,
        alias = "baidu-qianfan",
        alias = "baidu_qianfan",
        alias = "baidu"
    )]
    pub qianfan: ProviderConfig,
    #[serde(
        default,
        alias = "openai-codex",
        alias = "openaiCodex",
        alias = "codex",
        alias = "chatgpt"
    )]
    pub openai_codex: ProviderConfig,
    #[serde(default, alias = "claude")]
    pub anthropic: ProviderConfig,
    #[serde(default, alias = "open-model", alias = "open_model")]
    pub openmodel: ProviderConfig,
    #[serde(
        default,
        alias = "zhipu",
        alias = "zhipuai",
        alias = "bigmodel",
        alias = "big-model"
    )]
    pub zai: ProviderConfig,
    #[serde(default)]
    pub stepfun: ProviderConfig,
    #[serde(default)]
    pub minimax: ProviderConfig,
    #[serde(default, alias = "sakana-ai", alias = "sakana_ai", alias = "fugu")]
    pub sakana: ProviderConfig,
    #[serde(
        default,
        alias = "opencode-zen",
        alias = "opencode_zen",
        alias = "opencodezen",
        alias = "zen",
        alias = "opencode"
    )]
    pub opencode_zen: ProviderConfig,
    /// Arbitrary user-named custom providers (#1519).
    ///
    /// Captures every `[providers.<name>]` table whose key is not one of the
    /// built-in providers above. Each entry is an OpenAI-compatible custom
    /// endpoint selected via `provider = "<name>"`; routing reads its
    /// `base_url` / `model` / `api_key_env` through [`ApiProvider::Custom`].
    #[serde(flatten, default)]
    pub custom: HashMap<String, ProviderConfig>,
}

impl ProvidersConfig {
    /// Look up a user-defined custom provider table by its `[providers.<name>]`
    /// key (#1519). Returns `None` when no entry with that exact name exists.
    #[must_use]
    pub fn custom_provider_config(&self, name: &str) -> Option<&ProviderConfig> {
        self.custom.get(name)
    }

    fn validate(&self) -> Result<()> {
        let builtins = [
            ("providers.deepseek", &self.deepseek),
            ("providers.deepseek_cn", &self.deepseek_cn),
            ("providers.deepseek_anthropic", &self.deepseek_anthropic),
            ("providers.nvidia_nim", &self.nvidia_nim),
            ("providers.openai", &self.openai),
            ("providers.atlascloud", &self.atlascloud),
            ("providers.wanjie_ark", &self.wanjie_ark),
            ("providers.volcengine", &self.volcengine),
            ("providers.openrouter", &self.openrouter),
            ("providers.xiaomi_mimo", &self.xiaomi_mimo),
            ("providers.novita", &self.novita),
            ("providers.fireworks", &self.fireworks),
            ("providers.siliconflow", &self.siliconflow),
            ("providers.siliconflow_cn", &self.siliconflow_cn),
            ("providers.arcee", &self.arcee),
            ("providers.moonshot", &self.moonshot),
            ("providers.sglang", &self.sglang),
            ("providers.vllm", &self.vllm),
            ("providers.ollama", &self.ollama),
            ("providers.huggingface", &self.huggingface),
            ("providers.deepinfra", &self.deepinfra),
            ("providers.together", &self.together),
            ("providers.qianfan", &self.qianfan),
            ("providers.openai_codex", &self.openai_codex),
            ("providers.anthropic", &self.anthropic),
            ("providers.openmodel", &self.openmodel),
            ("providers.zai", &self.zai),
            ("providers.stepfun", &self.stepfun),
            ("providers.minimax", &self.minimax),
            ("providers.sakana", &self.sakana),
            ("providers.opencode_zen", &self.opencode_zen),
        ];
        for (name, config) in builtins {
            validate_provider_context_window(name, config.context_window)?;
        }
        for (name, config) in &self.custom {
            validate_provider_context_window(&format!("providers.{name}"), config.context_window)?;
        }
        Ok(())
    }
}

fn validate_provider_context_window(name: &str, value: Option<u32>) -> Result<()> {
    if value == Some(0) {
        anyhow::bail!("{name}.context_window must be greater than 0");
    }
    Ok(())
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

    #[must_use]
    pub fn auto_review_policy(&self) -> crate::tui::auto_review::AutoReviewPolicy {
        self.auto_review
            .as_ref()
            .map(AutoReviewConfig::to_runtime_policy)
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
                if let Some(msg) = warn_on_misplaced_top_level_keys(&contents) {
                    tracing::warn!("{msg}");
                }
                apply_profile(parsed, profile)?
            } else {
                Config::default()
            }
        } else {
            Config::default()
        };

        apply_env_overrides(&mut config);
        apply_managed_overrides(&mut config)?;
        apply_requirements(&mut config)?;
        normalize_model_config(&mut config);
        config.exec_policy_engine = load_sibling_exec_policy_engine(path.as_deref())?;
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
        let Ok(table) = provider_config_table_name(provider) else {
            return;
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
                "Invalid provider '{provider}': expected {}.",
                ApiProvider::names_hint()
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
        if let Some(v) = self.verbosity.as_deref() {
            let normalized = v.trim().to_ascii_lowercase();
            if !matches!(normalized.as_str(), "normal" | "concise") {
                anyhow::bail!("Invalid verbosity '{v}': expected normal or concise.");
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
        if let Some(auto_review) = &self.auto_review {
            auto_review.validate()?;
        }
        if let Some(providers) = &self.providers {
            providers.validate()?;
        }
        Ok(())
    }

    #[must_use]
    pub fn api_provider(&self) -> ApiProvider {
        if let Some(provider) = self.provider.as_deref().and_then(ApiProvider::parse) {
            return provider;
        }
        // #1519 safety fix: when `provider = "<name>"` is not a built-in provider
        // but names a `[providers.<name>]` custom table, route as the dynamic
        // custom identity. This MUST precede the DeepSeek fallback below so an
        // arbitrary custom name can never silently misroute to DeepSeek.
        if let Some(name) = self.provider.as_deref()
            && self
                .providers
                .as_ref()
                .and_then(|providers| providers.custom_provider_config(name))
                .is_some()
        {
            return ApiProvider::Custom;
        }
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
    }

    pub(crate) fn provider_config_for(&self, provider: ApiProvider) -> Option<&ProviderConfig> {
        let providers = self.providers.as_ref()?;
        // The custom provider's config lives in the flatten map, keyed by the
        // selected `provider = "<name>"` value, not in a fixed field (#1519).
        // Resolve it by name so every existing reader (auth, headers, base_url)
        // transparently sees the named table.
        if provider == ApiProvider::Custom {
            return self
                .provider
                .as_deref()
                .and_then(|name| providers.custom_provider_config(name));
        }
        Some(match provider {
            ApiProvider::Deepseek => &providers.deepseek,
            ApiProvider::DeepseekCN => &providers.deepseek_cn,
            ApiProvider::DeepseekAnthropic => &providers.deepseek_anthropic,
            ApiProvider::NvidiaNim => &providers.nvidia_nim,
            ApiProvider::Openai => &providers.openai,
            ApiProvider::Atlascloud => &providers.atlascloud,
            ApiProvider::WanjieArk => &providers.wanjie_ark,
            ApiProvider::Openrouter => &providers.openrouter,
            ApiProvider::XiaomiMimo => &providers.xiaomi_mimo,
            ApiProvider::Novita => &providers.novita,
            ApiProvider::Fireworks => &providers.fireworks,
            ApiProvider::Siliconflow => &providers.siliconflow,
            ApiProvider::SiliconflowCn => &providers.siliconflow_cn,
            ApiProvider::Arcee => &providers.arcee,
            ApiProvider::Moonshot => &providers.moonshot,
            ApiProvider::Sglang => &providers.sglang,
            ApiProvider::Vllm => &providers.vllm,
            ApiProvider::Ollama => &providers.ollama,
            ApiProvider::Volcengine => &providers.volcengine,
            ApiProvider::Huggingface => &providers.huggingface,
            ApiProvider::Deepinfra => &providers.deepinfra,
            ApiProvider::Together => &providers.together,
            ApiProvider::Qianfan => &providers.qianfan,
            ApiProvider::OpenaiCodex => &providers.openai_codex,
            ApiProvider::OpenCodeZen => &providers.opencode_zen,
            ApiProvider::Anthropic => &providers.anthropic,
            ApiProvider::Openmodel => &providers.openmodel,
            ApiProvider::Zai => &providers.zai,
            ApiProvider::Stepfun => &providers.stepfun,
            ApiProvider::Minimax => &providers.minimax,
            ApiProvider::Sakana => &providers.sakana,
            // Handled by the name-keyed early return above (#1519).
            ApiProvider::Custom => unreachable!("custom provider resolved by name above"),
        })
    }

    pub(crate) fn subagent_provider_config(
        &self,
        provider: ApiProvider,
    ) -> Option<&SubagentProviderConfig> {
        let providers = self.subagents.as_ref()?.providers.as_ref()?;
        providers.iter().find_map(|(key, config)| {
            subagent_provider_key_matches(key, provider).then_some(config)
        })
    }

    pub(crate) fn provider_config_for_mut(&mut self, provider: ApiProvider) -> &mut ProviderConfig {
        // The custom provider's mutable slot is keyed by the selected
        // `provider = "<name>"` value in the flatten map (#1519). Capture the
        // name before borrowing `providers` mutably; fall back to a private
        // sentinel key so the accessor stays total when no name is set.
        let custom_key = (provider == ApiProvider::Custom).then(|| {
            self.provider
                .clone()
                .unwrap_or_else(|| "__custom__".to_string())
        });
        let providers = self.providers.get_or_insert_with(ProvidersConfig::default);
        if let Some(key) = custom_key {
            return providers.custom.entry(key).or_default();
        }
        match provider {
            ApiProvider::Deepseek => &mut providers.deepseek,
            ApiProvider::DeepseekCN => &mut providers.deepseek_cn,
            ApiProvider::DeepseekAnthropic => &mut providers.deepseek_anthropic,
            ApiProvider::NvidiaNim => &mut providers.nvidia_nim,
            ApiProvider::Openai => &mut providers.openai,
            ApiProvider::Atlascloud => &mut providers.atlascloud,
            ApiProvider::WanjieArk => &mut providers.wanjie_ark,
            ApiProvider::Openrouter => &mut providers.openrouter,
            ApiProvider::XiaomiMimo => &mut providers.xiaomi_mimo,
            ApiProvider::Novita => &mut providers.novita,
            ApiProvider::Fireworks => &mut providers.fireworks,
            ApiProvider::Siliconflow => &mut providers.siliconflow,
            ApiProvider::SiliconflowCn => &mut providers.siliconflow_cn,
            ApiProvider::Arcee => &mut providers.arcee,
            ApiProvider::Moonshot => &mut providers.moonshot,
            ApiProvider::Sglang => &mut providers.sglang,
            ApiProvider::Vllm => &mut providers.vllm,
            ApiProvider::Ollama => &mut providers.ollama,
            ApiProvider::Volcengine => &mut providers.volcengine,
            ApiProvider::Huggingface => &mut providers.huggingface,
            ApiProvider::Deepinfra => &mut providers.deepinfra,
            ApiProvider::Together => &mut providers.together,
            ApiProvider::Qianfan => &mut providers.qianfan,
            ApiProvider::OpenaiCodex => &mut providers.openai_codex,
            ApiProvider::OpenCodeZen => &mut providers.opencode_zen,
            ApiProvider::Anthropic => &mut providers.anthropic,
            ApiProvider::Openmodel => &mut providers.openmodel,
            ApiProvider::Zai => &mut providers.zai,
            ApiProvider::Stepfun => &mut providers.stepfun,
            ApiProvider::Minimax => &mut providers.minimax,
            ApiProvider::Sakana => &mut providers.sakana,
            // Handled by the name-keyed early return above (#1519).
            ApiProvider::Custom => unreachable!("custom provider resolved by name above"),
        }
    }

    /// Return the configured provider request concurrency cap.
    ///
    /// `None` means the client does not apply an extra in-flight request
    /// semaphore. Z.ai/GLM gets a conservative default because its SSE endpoint
    /// times out under sustained parallel stream opens well below the advertised
    /// service concurrency (#3496). Operators can raise it with
    /// `[providers.zai] max_concurrency = N`; `0` explicitly disables the
    /// client-side cap for that provider.
    #[must_use]
    pub fn provider_max_concurrency(&self, provider: ApiProvider) -> Option<usize> {
        let configured = self
            .provider_config_for(provider)
            .and_then(|entry| entry.max_concurrency);
        match configured {
            Some(0) => None,
            Some(limit) => Some(limit.clamp(1, MAX_PROVIDER_REQUEST_CONCURRENCY)),
            None if provider == ApiProvider::Zai => Some(DEFAULT_ZAI_PROVIDER_MAX_CONCURRENCY),
            None => None,
        }
    }

    pub(crate) fn provider_config(&self) -> Option<&ProviderConfig> {
        self.provider_config_for(self.api_provider())
    }

    fn provider_config_string_with_runtime_fallback<F>(
        &self,
        provider: ApiProvider,
        get: F,
    ) -> Option<String>
    where
        F: Fn(&ProviderConfig) -> Option<String>,
    {
        if let Some(value) = self.provider_config_for(provider).and_then(&get) {
            return Some(value);
        }
        if provider == ApiProvider::SiliconflowCn {
            return self
                .provider_config_for(ApiProvider::Siliconflow)
                .and_then(get);
        }
        None
    }

    #[must_use]
    pub fn insecure_skip_tls_verify(&self) -> bool {
        self.provider_config()
            .and_then(|provider| provider.insecure_skip_tls_verify)
            .unwrap_or(false)
    }

    #[must_use]
    pub(crate) fn context_window_for_provider_config(&self, provider: ApiProvider) -> Option<u32> {
        if let Some(window) = self
            .provider_config_for(provider)
            .and_then(|entry| entry.context_window)
            .filter(|window| *window > 0)
        {
            return Some(window);
        }
        if provider == ApiProvider::SiliconflowCn {
            return self
                .provider_config_for(ApiProvider::Siliconflow)
                .and_then(|entry| entry.context_window)
                .filter(|window| *window > 0);
        }
        None
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
        if let Some(model) =
            self.provider_config_string_with_runtime_fallback(provider, |entry| entry.model.clone())
        {
            let model = model.trim();
            if provider_passes_model_through(provider)
                || self.active_provider_preserves_custom_base_url_model()
            {
                return model.to_string();
            }
            if let Some(normalized) = normalize_model_for_provider(provider, model) {
                return normalized;
            }
            // An explicit provider-scoped model that is not a recognized
            // DeepSeek alias is a deliberate custom choice for a non-DeepSeek
            // provider (e.g. `MiniMax-M2.7` on an OpenAI-compatible endpoint).
            // It must pass through verbatim rather than fall back to a
            // DeepSeek/provider default (issue #1714).
            if !matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
                && !model.is_empty()
            {
                return model.to_string();
            }
        }
        // The Codex Responses backend only serves its own model family, and a
        // global `default_text_model` is constrained to DeepSeek IDs or "auto"
        // by validation — so it can never name a Codex-compatible model. Fall
        // back to the Codex default here instead of letting a DeepSeek default
        // leak through and be rejected by the backend. An explicit
        // `[providers.openai_codex] model` is honored by the block above.
        if provider == ApiProvider::OpenaiCodex {
            return DEFAULT_OPENAI_CODEX_MODEL.to_string();
        }
        if provider == ApiProvider::OpenCodeZen {
            return DEFAULT_OPENCODE_ZEN_MODEL.to_string();
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
            && model.trim().eq_ignore_ascii_case("auto")
        {
            return "auto".to_string();
        }
        if provider == ApiProvider::XiaomiMimo
            && let Some(model) = self.default_text_model.as_deref()
            && let Some(canonical) = canonical_xiaomi_mimo_model_id(model)
        {
            return canonical.to_string();
        }
        if provider == ApiProvider::XiaomiMimo {
            return DEFAULT_XIAOMI_MIMO_MODEL.to_string();
        }
        if let Some(model) = self.default_text_model.as_deref()
            && (provider_passes_model_through(provider)
                || self.active_provider_preserves_custom_base_url_model())
        {
            return model.trim().to_string();
        }
        if let Some(model) = self.default_text_model.as_deref()
            && !root_deepseek_model_is_foreign_to_direct_provider(provider, model)
            && let Some(normalized) = normalize_model_name_for_provider(provider, model)
        {
            return normalized;
        }

        match provider {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => DEFAULT_TEXT_MODEL,
            ApiProvider::DeepseekAnthropic => DEFAULT_DEEPSEEK_ANTHROPIC_MODEL,
            ApiProvider::NvidiaNim => DEFAULT_NVIDIA_NIM_MODEL,
            ApiProvider::Openai => DEFAULT_OPENAI_MODEL,
            ApiProvider::Atlascloud => DEFAULT_ATLASCLOUD_MODEL,
            ApiProvider::WanjieArk => DEFAULT_WANJIE_ARK_MODEL,
            ApiProvider::Openrouter => DEFAULT_OPENROUTER_MODEL,
            ApiProvider::XiaomiMimo => DEFAULT_XIAOMI_MIMO_MODEL,
            ApiProvider::Novita => DEFAULT_NOVITA_MODEL,
            ApiProvider::Fireworks => DEFAULT_FIREWORKS_MODEL,
            ApiProvider::Siliconflow | ApiProvider::SiliconflowCn => DEFAULT_SILICONFLOW_MODEL,
            ApiProvider::Arcee => DEFAULT_ARCEE_MODEL,
            ApiProvider::Moonshot => DEFAULT_MOONSHOT_MODEL,
            ApiProvider::Sglang => DEFAULT_SGLANG_MODEL,
            ApiProvider::Vllm => DEFAULT_VLLM_MODEL,
            ApiProvider::Ollama => DEFAULT_OLLAMA_MODEL,
            ApiProvider::Volcengine => DEFAULT_VOLCENGINE_MODEL,
            ApiProvider::Huggingface => DEFAULT_HUGGINGFACE_MODEL,
            ApiProvider::Deepinfra => DEFAULT_DEEPINFRA_MODEL,
            ApiProvider::Together => DEFAULT_TOGETHER_MODEL,
            ApiProvider::Qianfan => DEFAULT_QIANFAN_MODEL,
            ApiProvider::OpenaiCodex => DEFAULT_OPENAI_CODEX_MODEL,
            ApiProvider::OpenCodeZen => DEFAULT_OPENCODE_ZEN_MODEL,
            ApiProvider::Openmodel => DEFAULT_OPENMODEL_MODEL,
            ApiProvider::Zai => DEFAULT_ZAI_MODEL,
            ApiProvider::Stepfun => DEFAULT_STEPFUN_MODEL,
            ApiProvider::Anthropic => DEFAULT_ANTHROPIC_MODEL,
            ApiProvider::Minimax => DEFAULT_MINIMAX_MODEL,
            ApiProvider::Sakana => DEFAULT_SAKANA_MODEL,
            // Custom endpoints have no built-in default model; pass through the
            // descriptor placeholder when nothing is configured (#1519).
            ApiProvider::Custom => codewhale_config::ProviderKind::Custom
                .provider()
                .default_model(),
        }
        .to_string()
    }

    /// Return the configured API base URL (normalized).
    #[must_use]
    pub fn deepseek_base_url(&self) -> String {
        let provider = self.api_provider();
        let provider_base = self
            .provider_config_string_with_runtime_fallback(provider, |entry| entry.base_url.clone());
        // Root `base_url` is the legacy DeepSeek field; only NvidiaNim has a
        // back-compat sniff (integrate.api.nvidia.com). OpenRouter / Novita
        // were added in v0.6.7 and require explicit `[providers.<name>]`
        // entries or the corresponding `*_BASE_URL` env var.
        let root_base = match provider {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => self.base_url.clone(),
            ApiProvider::DeepseekAnthropic => None,
            ApiProvider::NvidiaNim => self
                .base_url
                .as_ref()
                .filter(|base| base.contains("integrate.api.nvidia.com"))
                .cloned(),
            ApiProvider::Openai
            | ApiProvider::Anthropic
            | ApiProvider::Openmodel
            | ApiProvider::Atlascloud
            | ApiProvider::WanjieArk
            | ApiProvider::Openrouter
            | ApiProvider::XiaomiMimo
            | ApiProvider::Novita
            | ApiProvider::Fireworks
            | ApiProvider::Siliconflow
            | ApiProvider::SiliconflowCn
            | ApiProvider::Arcee
            | ApiProvider::Moonshot
            | ApiProvider::Sglang
            | ApiProvider::Vllm
            | ApiProvider::Ollama
            | ApiProvider::Volcengine
            | ApiProvider::Huggingface
            | ApiProvider::Deepinfra
            | ApiProvider::Together
            | ApiProvider::Qianfan
            | ApiProvider::OpenaiCodex
            | ApiProvider::OpenCodeZen
            | ApiProvider::Zai
            | ApiProvider::Stepfun
            | ApiProvider::Minimax
            | ApiProvider::Sakana
            // Custom reads its base_url from the named `[providers.<name>]`
            // table (via provider_base), never from the legacy root field.
            | ApiProvider::Custom => None,
        };
        let configured_base_url = provider_base.or(root_base);
        let base = if provider == ApiProvider::XiaomiMimo {
            let config_api_key = self
                .provider_config_for(provider)
                .and_then(|provider| provider.api_key.as_deref());
            let mode = self
                .provider_config_for(provider)
                .and_then(|provider| provider.mode.as_deref());
            let env_api_key =
                xiaomi_mimo_env_api_key_for_runtime(mode, configured_base_url.as_deref());
            let api_key = config_api_key.or(env_api_key.as_deref());
            resolve_xiaomi_mimo_base_url(configured_base_url, api_key, mode)
        } else {
            configured_base_url
                .or_else(env_base_url_override)
                .unwrap_or_else(|| {
                    match provider {
                        ApiProvider::Deepseek => DEFAULT_DEEPSEEK_BASE_URL,
                        ApiProvider::DeepseekCN => DEFAULT_DEEPSEEKCN_BASE_URL,
                        ApiProvider::DeepseekAnthropic => DEFAULT_DEEPSEEK_ANTHROPIC_BASE_URL,
                        ApiProvider::NvidiaNim => DEFAULT_NVIDIA_NIM_BASE_URL,
                        ApiProvider::Openai => DEFAULT_OPENAI_BASE_URL,
                        ApiProvider::Atlascloud => DEFAULT_ATLASCLOUD_BASE_URL,
                        ApiProvider::WanjieArk => DEFAULT_WANJIE_ARK_BASE_URL,
                        ApiProvider::Openrouter => DEFAULT_OPENROUTER_BASE_URL,
                        ApiProvider::XiaomiMimo => DEFAULT_XIAOMI_MIMO_BASE_URL,
                        ApiProvider::Novita => DEFAULT_NOVITA_BASE_URL,
                        ApiProvider::Fireworks => DEFAULT_FIREWORKS_BASE_URL,
                        ApiProvider::Siliconflow => DEFAULT_SILICONFLOW_BASE_URL,
                        ApiProvider::SiliconflowCn => DEFAULT_SILICONFLOW_CN_BASE_URL,
                        ApiProvider::Arcee => DEFAULT_ARCEE_BASE_URL,
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
                        ApiProvider::Huggingface => DEFAULT_HUGGINGFACE_BASE_URL,
                        ApiProvider::Deepinfra => DEFAULT_DEEPINFRA_BASE_URL,
                        ApiProvider::Together => DEFAULT_TOGETHER_BASE_URL,
                        ApiProvider::Qianfan => DEFAULT_QIANFAN_BASE_URL,
                        ApiProvider::OpenaiCodex => DEFAULT_OPENAI_CODEX_BASE_URL,
                        ApiProvider::OpenCodeZen => DEFAULT_OPENCODE_ZEN_BASE_URL,
                        ApiProvider::Openmodel => DEFAULT_OPENMODEL_BASE_URL,
                        ApiProvider::Zai => DEFAULT_ZAI_BASE_URL,
                        ApiProvider::Stepfun => DEFAULT_STEPFUN_BASE_URL,
                        ApiProvider::Anthropic => DEFAULT_ANTHROPIC_BASE_URL,
                        ApiProvider::Minimax => DEFAULT_MINIMAX_BASE_URL,
                        ApiProvider::Sakana => DEFAULT_SAKANA_BASE_URL,
                        // No built-in endpoint; descriptor placeholder keeps the
                        // fallback total. A real custom route configures
                        // `[providers.<name>] base_url` which wins above (#1519).
                        ApiProvider::Custom => codewhale_config::ProviderKind::Custom
                            .provider()
                            .default_base_url(),
                    }
                    .to_string()
                })
        };
        normalize_base_url(&base)
    }

    fn active_provider_preserves_custom_base_url_model(&self) -> bool {
        let provider = self.api_provider();
        provider_preserves_custom_base_url_model(provider, &self.deepseek_base_url())
    }

    pub(crate) fn model_ids_pass_through(&self) -> bool {
        let provider = self.api_provider();
        provider_passes_model_through(provider)
            || self.active_provider_preserves_custom_base_url_model()
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

        // 0. DeepSeek compatibility slot. The legacy top-level `api_key`
        // belongs to DeepSeek only; provider-specific keys below must win for
        // NIM/OpenRouter/etc. so a stale DeepSeek key is not sent elsewhere.
        //
        // However, when the CLI dispatcher forwards an explicit `--api-key`
        // through `DEEPSEEK_API_KEY` with the dispatcher source marker, that
        // intentional override must win over the saved root key. This is
        // essential for DeepSeek-compatible subscription endpoints where the
        // user runs something like:
        //   codewhale --provider deepseek --api-key ark-... --base-url ... --model auto
        if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
            && std::env::var("DEEPSEEK_API_KEY_SOURCE").as_deref() == Ok("cli")
            && let Some(env_key) = provider_env_api_key(provider)
            && !env_key.trim().is_empty()
        {
            return Ok(env_key);
        }
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

        // OpenAI Codex (ChatGPT) reuses the existing Codex CLI OAuth login.
        // The access token lives in ~/.codex/auth.json (refreshed on demand)
        // rather than a stored API key, so resolve it before the config-file
        // and env slots. Explicit env overrides are handled inside
        // `get_credentials`.
        if provider == ApiProvider::OpenaiCodex {
            return Ok(crate::oauth::get_credentials()?.access_token);
        }

        // 1. Config file (provider-scoped slot). This intentionally wins
        // over ambient env so `codewhale auth set` fixes stale shell exports.
        if let Some(configured) = self
            .provider_config_string_with_runtime_fallback(provider, |entry| entry.api_key.clone())
            && !configured.trim().is_empty()
        {
            return Ok(configured);
        }

        // 1b. Custom providers (#1519) name their auth env var per-entry via
        // `[providers.<name>] api_key_env = "..."`. Resolve it before the
        // generic env step, since the custom identity declares no built-in env
        // var. The env var NAME is read from config; the secret value is read
        // from the process environment and never persisted.
        if provider == ApiProvider::Custom
            && let Some(env_name) = self
                .provider_config_for(provider)
                .and_then(|entry| entry.api_key_env.as_deref())
                .map(str::trim)
                .filter(|name| !name.is_empty())
            && let Ok(value) = std::env::var(env_name)
            && !value.trim().is_empty()
        {
            return Ok(value);
        }

        // 2. Environment variables. Do not query platform credential stores
        // here; routine startup and doctor checks must stay prompt-free.
        if provider == ApiProvider::XiaomiMimo {
            let mode = self
                .provider_config_for(provider)
                .and_then(|provider| provider.mode.as_deref());
            if let Some(value) =
                xiaomi_mimo_env_api_key_for_runtime(mode, Some(&self.deepseek_base_url()))
                && !value.trim().is_empty()
            {
                return Ok(value);
            }
        }
        if let Some(value) = provider_env_api_key(provider) {
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
                   • api_key = \"<your-key>\"  in ~/.codewhale/config.toml"
            ),
            ApiProvider::SiliconflowCn => anyhow::bail!(
                "SiliconFlow China API key not found. Get a key: {}. Run 'codewhale auth set --provider siliconflow-CN', \
                 set {}, or add [{}] api_key in ~/.codewhale/config.toml. \
                 [providers.siliconflow] remains a fallback when the CN table omits api_key.",
                provider
                    .credential_url()
                    .unwrap_or("https://cloud.siliconflow.com/account/ak"),
                provider.env_vars_label(),
                provider_config_table_name(provider)?
            ),
            ApiProvider::Moonshot => anyhow::bail!(
                "Moonshot/Kimi API key not found. Get a key: {}. Run 'codewhale auth set --provider moonshot', \
                 set {}, or add [{}] api_key. \
                 For a Kimi Code plan key, set [providers.moonshot] base_url = \
                 \"https://api.kimi.com/coding/v1\" and model = \"kimi-for-coding\".",
                provider
                    .credential_url()
                    .unwrap_or("https://platform.kimi.ai/"),
                provider.env_vars_label(),
                provider_config_table_name(provider)?
            ),
            ApiProvider::Anthropic | ApiProvider::Openmodel => {
                anyhow::bail!("{}", missing_provider_api_key_message(provider)?)
            }
            ApiProvider::OpenaiCodex | ApiProvider::OpenCodeZen => anyhow::bail!(
                "OpenAI Codex OAuth credentials not found.\n\
                 \n\
                 CodeWhale uses your existing ChatGPT/Codex login.\n\
                 1. Run: codex login      (or use the Codex CLI to authenticate)\n\
                 2. CodeWhale will read credentials from ~/.codex/auth.json\n\
                 \n\
                 Env overrides:\n\
                   OPENAI_CODEX_ACCESS_TOKEN  or  CODEX_ACCESS_TOKEN"
            ),
            // Self-hosted deployments commonly run without auth on localhost.
            // Return an empty key and let the client omit the Authorization header.
            ApiProvider::Sglang | ApiProvider::Vllm | ApiProvider::Ollama => Ok(String::new()),
            // Custom OpenAI-compatible endpoints (#1519): the key comes from the
            // env var named by `[providers.<name>] api_key_env`. If we reached
            // here it is unset/empty (and the endpoint is not loopback).
            ApiProvider::Custom => {
                let provider_name = self.provider.as_deref().unwrap_or("<name>");
                match self
                    .provider_config_for(provider)
                    .and_then(|entry| entry.api_key_env.as_deref())
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                {
                    Some(env_name) => anyhow::bail!(
                        "Custom provider '{provider_name}' API key not found.\n\
                         Set the environment variable {env_name} to your key, \
                         or add api_key to [providers.{provider_name}]."
                    ),
                    None => anyhow::bail!(
                        "Custom provider '{provider_name}' has no auth configured.\n\
                         Add api_key_env = \"YOUR_ENV_VAR\" (or api_key) to \
                         [providers.{provider_name}] in ~/.codewhale/config.toml."
                    ),
                }
            }
            _ => anyhow::bail!("{}", missing_provider_api_key_message(provider)?),
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

    /// Resolve the default speech/TTS output directory, if configured.
    #[must_use]
    pub fn speech_output_dir(&self) -> Option<PathBuf> {
        std::env::var("XIAOMI_MIMO_SPEECH_OUTPUT_DIR")
            .or_else(|_| std::env::var("MIMO_SPEECH_OUTPUT_DIR"))
            .or_else(|_| std::env::var("XIAOMIMIMO_SPEECH_OUTPUT_DIR"))
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(|value| expand_path(&value))
            .or_else(|| {
                self.speech
                    .as_ref()
                    .and_then(|speech| speech.output_dir.as_deref())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(expand_path)
            })
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

    /// Whether the legacy `memory.rs` push/inject path is deprecated in
    /// favor of Moraine MCP recall. When `true`, the `<user_memory>`
    /// block is skipped, the `remember` tool is not registered, and
    /// `# foo` quick-add falls through to normal turn submission, even
    /// when `memory_enabled()` returns `true`. Default `false`.
    #[must_use]
    pub fn moraine_fallback(&self) -> bool {
        self.memory
            .as_ref()
            .and_then(|m| m.moraine_fallback)
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

    /// Return whether shell execution is allowed for noninteractive and
    /// durable-task profiles. Defaults to `false`: in headless, app-server, and
    /// background-task contexts there is no human to approve commands, so shell
    /// access must be opted into explicitly (GHSA-72w5-pf8h-xfp4).
    #[must_use]
    pub fn allow_shell(&self) -> bool {
        self.allow_shell.unwrap_or(false)
    }

    /// Return whether shell execution is allowed for an *interactive* TUI Agent
    /// session. Defaults to `true`: the interactive composer always gates each
    /// shell command behind an approval prompt, so the catalog can expose shell
    /// by default while still preserving consent (GHSA-72w5-pf8h-xfp4). An
    /// explicit `allow_shell = false` still hides shell tools. This is the
    /// single source of truth for the interactive default; both startup
    /// (`run_interactive`) and the durable Agent permission baseline read it so
    /// the default cannot drift between them.
    #[must_use]
    pub fn interactive_allow_shell(&self) -> bool {
        self.allow_shell.unwrap_or(true)
    }

    /// Whether ghost-text prompt suggestion is enabled (opt-in, default off).
    pub fn prompt_suggestion_enabled(&self) -> bool {
        self.prompt_suggestion.unwrap_or(false)
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

    /// Return the provider-specific maximum number of concurrent sub-agents.
    /// `[subagents.providers.<provider>] max_concurrent` inherits from the
    /// global `[subagents]` value when unset.
    #[must_use]
    pub fn max_subagents_for_provider(&self, provider: ApiProvider) -> usize {
        self.subagent_provider_config(provider)
            .and_then(|cfg| cfg.max_concurrent)
            .map(|max| max.clamp(1, MAX_SUBAGENTS))
            .unwrap_or_else(|| self.max_subagents())
    }

    /// Whether the model-facing `agent` tool is available after applying the
    /// feature flag, explicit `[subagents] enabled` switch, and legacy
    /// zero-valued opt-outs.
    #[must_use]
    pub fn subagents_enabled(&self) -> bool {
        self.subagents_disabled_reason().is_none()
    }

    /// Whether the model-facing `agent` tool is available for this provider
    /// after applying global and provider-specific sub-agent controls.
    #[must_use]
    pub fn subagents_enabled_for_provider(&self, provider: ApiProvider) -> bool {
        if !self.subagents_enabled() {
            return false;
        }
        let Some(provider_cfg) = self.subagent_provider_config(provider) else {
            return true;
        };
        provider_cfg.enabled != Some(false)
            && provider_cfg.max_concurrent != Some(0)
            && provider_cfg.max_depth != Some(0)
    }

    /// Machine-readable reason sub-agents are disabled, in precedence order.
    #[must_use]
    pub fn subagents_disabled_reason(&self) -> Option<&'static str> {
        if !self.features().enabled(Feature::Subagents) {
            return Some("features.subagents=false");
        }
        let subagents_cfg = self.subagents.as_ref()?;
        if subagents_cfg.enabled == Some(false) {
            return Some("subagents.enabled=false");
        }
        if subagents_cfg.max_concurrent == Some(0) {
            return Some("subagents.max_concurrent=0");
        }
        if subagents_cfg.max_depth == Some(0) {
            return Some("subagents.max_depth=0");
        }
        None
    }

    /// How many levels of nested sub-agents the interactive `agent` tool may
    /// spawn. Reads `[subagents] max_depth`; when unset it defaults to
    /// [`codewhale_config::DEFAULT_SPAWN_DEPTH`]. `0` is a valid value that
    /// blocks the `agent` tool at this runtime depth. Any value is clamped to
    /// [`codewhale_config::MAX_SPAWN_DEPTH_CEILING`] so the operator's choice
    /// can never exceed the hard recursion ceiling.
    #[must_use]
    pub fn subagent_max_spawn_depth(&self) -> u32 {
        self.subagents
            .as_ref()
            .and_then(|cfg| cfg.max_depth)
            .unwrap_or(codewhale_config::DEFAULT_SPAWN_DEPTH)
            .min(codewhale_config::MAX_SPAWN_DEPTH_CEILING)
    }

    /// Return the provider-specific maximum sub-agent recursion depth.
    #[must_use]
    pub fn subagent_max_spawn_depth_for_provider(&self, provider: ApiProvider) -> u32 {
        self.subagent_provider_config(provider)
            .and_then(|cfg| cfg.max_depth)
            .unwrap_or_else(|| self.subagent_max_spawn_depth())
            .min(codewhale_config::MAX_SPAWN_DEPTH_CEILING)
    }

    /// Number of direct (depth-1) sub-agents that may execute concurrently
    /// before further launches queue for a launch slot (#3095). Reads
    /// `[subagents] launch_concurrency` (or the deprecated
    /// `interactive_max_launch` alias); when unset it defaults to the full
    /// resolved `max_subagents()` (no artificial throttle), and any explicit
    /// value is clamped to `[1, max_subagents]`.
    #[must_use]
    pub fn launch_concurrency(&self) -> usize {
        let max = self.max_subagents();
        self.subagents
            .as_ref()
            .and_then(|cfg| cfg.launch_concurrency.or(cfg.interactive_max_launch_legacy))
            .unwrap_or(max)
            .clamp(1, max)
    }

    /// Return the provider-specific direct launch throttle. Children above
    /// this limit queue for a launch slot instead of starting immediately.
    #[must_use]
    pub fn launch_concurrency_for_provider(&self, provider: ApiProvider) -> usize {
        let max = self.max_subagents_for_provider(provider);
        self.subagent_provider_config(provider)
            .and_then(|cfg| cfg.launch_concurrency)
            .or_else(|| {
                self.subagents
                    .as_ref()
                    .and_then(|cfg| cfg.launch_concurrency.or(cfg.interactive_max_launch_legacy))
            })
            .unwrap_or(max)
            .clamp(1, max)
    }

    /// Maximum queued + running sub-agents admitted for the session.
    ///
    /// Defaults to [`MAX_SUBAGENT_ADMISSION`] so distinct `agent` calls can
    /// queue and drain through `launch_concurrency` instead of being rejected
    /// at the instantaneous concurrency cap. Explicit values are clamped to
    /// `[max_subagents, MAX_SUBAGENT_ADMISSION]`.
    #[must_use]
    pub fn max_admitted_subagents(&self) -> usize {
        let max_concurrent = self.max_subagents();
        self.subagents
            .as_ref()
            .and_then(|cfg| cfg.max_admitted)
            .unwrap_or(MAX_SUBAGENT_ADMISSION)
            .clamp(max_concurrent, MAX_SUBAGENT_ADMISSION)
    }

    /// Return the provider-specific queued + running admission cap.
    #[must_use]
    pub fn max_admitted_subagents_for_provider(&self, provider: ApiProvider) -> usize {
        let max_concurrent = self.max_subagents_for_provider(provider);
        self.subagent_provider_config(provider)
            .and_then(|cfg| cfg.max_admitted)
            .or_else(|| self.subagents.as_ref().and_then(|cfg| cfg.max_admitted))
            .unwrap_or(MAX_SUBAGENT_ADMISSION)
            .clamp(max_concurrent, MAX_SUBAGENT_ADMISSION)
    }

    /// Optional aggregate token budget for each root `agent` run.
    ///
    /// Reads `[subagents] token_budget`. `None` and `0` both mean unlimited,
    /// preserving legacy behavior until a budget is explicitly configured.
    #[must_use]
    pub fn subagent_token_budget(&self) -> Option<u64> {
        self.subagents
            .as_ref()
            .and_then(|cfg| cfg.token_budget)
            .filter(|budget| *budget > 0)
    }

    /// Return the provider-specific aggregate token budget for each root
    /// `agent` run.
    #[must_use]
    pub fn subagent_token_budget_for_provider(&self, provider: ApiProvider) -> Option<u64> {
        self.subagent_provider_config(provider)
            .and_then(|cfg| cfg.token_budget)
            .or_else(|| self.subagents.as_ref().and_then(|cfg| cfg.token_budget))
            .filter(|budget| *budget > 0)
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
        resolve_subagent_api_timeout_secs(
            self.subagents.as_ref().and_then(|cfg| cfg.api_timeout_secs),
        )
    }

    /// Return the provider-specific per-step API timeout for sub-agents.
    #[must_use]
    pub fn subagent_api_timeout_secs_for_provider(&self, provider: ApiProvider) -> u64 {
        resolve_subagent_api_timeout_secs(
            self.subagent_provider_config(provider)
                .and_then(|cfg| cfg.api_timeout_secs)
                .or_else(|| self.subagents.as_ref().and_then(|cfg| cfg.api_timeout_secs)),
        )
    }

    /// Resolved no-progress heartbeat timeout for running sub-agents.
    ///
    /// Reads `[subagents] heartbeat_timeout_secs` and clamps to
    /// `[MIN_SUBAGENT_HEARTBEAT_TIMEOUT_SECS, MAX_SUBAGENT_HEARTBEAT_TIMEOUT_SECS]`.
    /// `None` or `0` resolve to the default 300 seconds. The final value is
    /// also kept at least 30 seconds above `subagent_api_timeout_secs()` so a
    /// configured long model request is not pre-empted by heartbeat cleanup.
    #[must_use]
    pub fn subagent_heartbeat_timeout_secs(&self) -> u64 {
        resolve_subagent_heartbeat_timeout_secs(
            self.subagents
                .as_ref()
                .and_then(|cfg| cfg.heartbeat_timeout_secs),
            self.subagent_api_timeout_secs(),
        )
    }

    /// Return the provider-specific no-progress heartbeat timeout.
    #[must_use]
    pub fn subagent_heartbeat_timeout_secs_for_provider(&self, provider: ApiProvider) -> u64 {
        let api_timeout = self.subagent_api_timeout_secs_for_provider(provider);
        resolve_subagent_heartbeat_timeout_secs(
            self.subagent_provider_config(provider)
                .and_then(|cfg| cfg.heartbeat_timeout_secs)
                .or_else(|| {
                    self.subagents
                        .as_ref()
                        .and_then(|cfg| cfg.heartbeat_timeout_secs)
                }),
            api_timeout,
        )
    }

    /// Resolved per-SSE-chunk idle timeout in seconds.
    ///
    /// Reads `[tui].stream_chunk_timeout_secs`, falling back to the legacy
    /// `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS` env var when the config key is
    /// omitted. `None` or `0` resolve to the default 300 seconds; explicit
    /// values are clamped to `1..=3600`.
    #[must_use]
    pub fn stream_chunk_timeout_secs(&self) -> u64 {
        let raw = self
            .tui
            .as_ref()
            .and_then(|cfg| cfg.stream_chunk_timeout_secs)
            .or_else(|| {
                std::env::var(STREAM_CHUNK_TIMEOUT_ENV)
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
            })
            .unwrap_or(DEFAULT_STREAM_CHUNK_TIMEOUT_SECS);
        if raw == 0 {
            return DEFAULT_STREAM_CHUNK_TIMEOUT_SECS;
        }
        raw.clamp(MIN_STREAM_CHUNK_TIMEOUT_SECS, MAX_STREAM_CHUNK_TIMEOUT_SECS)
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

    /// Resolve community skill settings with defaults applied.
    #[must_use]
    pub fn skills_config(&self) -> SkillsConfig {
        self.skills.clone().unwrap_or_default()
    }

    /// Resolve startup update-check settings with defaults applied.
    #[must_use]
    pub fn update_config(&self) -> UpdateConfig {
        self.update.clone().unwrap_or_default()
    }

    /// Resolve durable hotbar bindings for render/dispatch layers.
    #[must_use]
    pub fn resolve_hotbar_bindings(
        &self,
        known_action_ids: &[&str],
    ) -> codewhale_config::HotbarConfigResolution {
        codewhale_config::resolve_hotbar_bindings(self.hotbar.as_deref(), known_action_ids)
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

fn root_deepseek_model_is_foreign_to_direct_provider(provider: ApiProvider, model: &str) -> bool {
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        || provider_passes_model_through(provider)
    {
        return false;
    }
    if matches!(
        provider,
        ApiProvider::NvidiaNim
            | ApiProvider::Openrouter
            | ApiProvider::Novita
            | ApiProvider::Fireworks
            | ApiProvider::Siliconflow
            | ApiProvider::SiliconflowCn
            | ApiProvider::Deepinfra
            | ApiProvider::Together
            | ApiProvider::Sglang
            | ApiProvider::Vllm
            | ApiProvider::Volcengine
            | ApiProvider::Atlascloud
            | ApiProvider::WanjieArk
    ) {
        return false;
    }
    normalize_model_name(model).is_some()
}

// === Defaults ===

// Pure filesystem path helpers live in the `paths` leaf module. The two
// `pub(crate)` entry points are re-exported so external `crate::config::`
// callers resolve unchanged; the remaining helpers are imported privately for
// the workspace-trust/config-load logic that stays in this file (#3311).
mod paths;
use paths::{
    canonicalize_or_keep, codewhale_home_dir, default_config_path, default_managed_config_path,
    default_mcp_config_path, default_memory_path, default_notes_path, default_requirements_path,
    default_skills_dir, env_config_path, expand_pathbuf, home_config_path, workspace_config_key,
};
pub(crate) use paths::{effective_home_dir, expand_path};

pub(crate) fn workspace_trust_config_candidate_paths() -> Vec<PathBuf> {
    if let Some(path) = env_config_path() {
        return vec![path];
    }

    if let Some(codewhale_home) = codewhale_home_dir() {
        return vec![codewhale_home.join("config.toml")];
    }

    let Some(home) = effective_home_dir() else {
        return Vec::new();
    };
    vec![
        home.join(".codewhale").join("config.toml"),
        home.join(".deepseek").join("config.toml"),
    ]
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

pub(crate) fn resolve_load_config_path(path: Option<PathBuf>) -> Option<PathBuf> {
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

# Startup update check
[update]
check_for_updates = true
# update_uri = "https://internal.mirror.example/codewhale/releases/latest"
"#
    );
    write_config_file_secure(&config_path, &content)
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    Ok(Some(config_path))
}

// === Environment Overrides ===

/// Read the `DEEPSEEK_BASE_URL` / `CODEWHALE_BASE_URL` env var that the CLI
/// dispatcher forwards from `--base-url`.  Returns `None` when the var is
/// absent or empty so that provider-specific defaults still apply.
fn env_base_url_override() -> Option<String> {
    codewhale_env_var("CODEWHALE_BASE_URL", "DEEPSEEK_BASE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
}

/// Resolve an env var, preferring the `CODEWHALE_*` form over the
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
            ApiProvider::DeepseekAnthropic => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .deepseek_anthropic
                    .base_url = Some(value);
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
            ApiProvider::Anthropic => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .anthropic
                    .base_url = Some(value);
            }
            ApiProvider::Openmodel => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .openmodel
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
            ApiProvider::SiliconflowCn => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .siliconflow_cn
                    .base_url = Some(value);
            }
            ApiProvider::Arcee => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .arcee
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
            ApiProvider::Huggingface => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .huggingface
                    .base_url = Some(value);
            }
            ApiProvider::Deepinfra => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .deepinfra
                    .base_url = Some(value);
            }
            ApiProvider::Together => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .together
                    .base_url = Some(value);
            }
            ApiProvider::Qianfan => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .qianfan
                    .base_url = Some(value);
            }
            ApiProvider::OpenaiCodex => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .openai_codex
                    .base_url = Some(value);
            }
            ApiProvider::OpenCodeZen => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .opencode_zen
                    .base_url = Some(value);
            }
            ApiProvider::Zai => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .zai
                    .base_url = Some(value);
            }
            ApiProvider::Stepfun => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .stepfun
                    .base_url = Some(value);
            }
            ApiProvider::Minimax => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .minimax
                    .base_url = Some(value);
            }
            ApiProvider::Sakana => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .sakana
                    .base_url = Some(value);
            }
            // Custom resolves to the named `[providers.<name>]` table; route the
            // override through the name-keyed mutable accessor (#1519).
            ApiProvider::Custom => {
                config.provider_config_for_mut(ApiProvider::Custom).base_url = Some(value);
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
    if matches!(config.api_provider(), ApiProvider::XiaomiMimo)
        && let Ok(value) = std::env::var("XIAOMI_MIMO_MODE").or_else(|_| std::env::var("MIMO_MODE"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .xiaomi_mimo
            .mode = Some(value);
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
    if matches!(config.api_provider(), ApiProvider::Volcengine)
        && let Ok(value) = std::env::var("VOLCENGINE_BASE_URL")
            .or_else(|_| std::env::var("VOLCENGINE_ARK_BASE_URL"))
            .or_else(|_| std::env::var("ARK_BASE_URL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .volcengine
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
    let active_provider = config.api_provider();
    if matches!(
        active_provider,
        ApiProvider::Siliconflow | ApiProvider::SiliconflowCn
    ) && let Ok(value) = std::env::var("SILICONFLOW_BASE_URL")
        && !value.trim().is_empty()
    {
        config.provider_config_for_mut(active_provider).base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Arcee)
        && let Ok(value) = std::env::var("ARCEE_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .arcee
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Huggingface)
        && let Ok(value) =
            std::env::var("HUGGINGFACE_BASE_URL").or_else(|_| std::env::var("HF_BASE_URL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .huggingface
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
        // Capture the custom entry key (the selected provider name) before the
        // mutable borrow of `providers` below (#1519).
        let custom_key = (provider == ApiProvider::Custom).then(|| {
            config
                .provider
                .clone()
                .unwrap_or_else(|| "__custom__".to_string())
        });
        let providers = config
            .providers
            .get_or_insert_with(ProvidersConfig::default);
        let entry = match provider {
            ApiProvider::Deepseek => &mut providers.deepseek,
            ApiProvider::DeepseekCN => &mut providers.deepseek_cn,
            ApiProvider::DeepseekAnthropic => &mut providers.deepseek_anthropic,
            ApiProvider::NvidiaNim => &mut providers.nvidia_nim,
            ApiProvider::Openai => &mut providers.openai,
            ApiProvider::Atlascloud => &mut providers.atlascloud,
            ApiProvider::WanjieArk => &mut providers.wanjie_ark,
            ApiProvider::Openrouter => &mut providers.openrouter,
            ApiProvider::XiaomiMimo => &mut providers.xiaomi_mimo,
            ApiProvider::Novita => &mut providers.novita,
            ApiProvider::Fireworks => &mut providers.fireworks,
            ApiProvider::Siliconflow => &mut providers.siliconflow,
            ApiProvider::SiliconflowCn => &mut providers.siliconflow_cn,
            ApiProvider::Arcee => &mut providers.arcee,
            ApiProvider::Moonshot => &mut providers.moonshot,
            ApiProvider::Sglang => &mut providers.sglang,
            ApiProvider::Vllm => &mut providers.vllm,
            ApiProvider::Ollama => &mut providers.ollama,
            ApiProvider::Volcengine => &mut providers.volcengine,
            ApiProvider::Huggingface => &mut providers.huggingface,
            ApiProvider::Deepinfra => &mut providers.deepinfra,
            ApiProvider::Together => &mut providers.together,
            ApiProvider::Qianfan => &mut providers.qianfan,
            ApiProvider::OpenaiCodex => &mut providers.openai_codex,
            ApiProvider::OpenCodeZen => &mut providers.opencode_zen,
            ApiProvider::Anthropic => &mut providers.anthropic,
            ApiProvider::Openmodel => &mut providers.openmodel,
            ApiProvider::Zai => &mut providers.zai,
            ApiProvider::Stepfun => &mut providers.stepfun,
            ApiProvider::Minimax => &mut providers.minimax,
            ApiProvider::Sakana => &mut providers.sakana,
            ApiProvider::Custom => providers
                .custom
                .entry(custom_key.expect("custom key captured for custom provider"))
                .or_default(),
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
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .wanjie_ark
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Openrouter)
        && let Ok(value) = std::env::var("OPENROUTER_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .openrouter
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Volcengine)
        && let Ok(value) =
            std::env::var("VOLCENGINE_MODEL").or_else(|_| std::env::var("VOLCENGINE_ARK_MODEL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .volcengine
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Novita)
        && let Ok(value) = std::env::var("NOVITA_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .novita
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Fireworks)
        && let Ok(value) = std::env::var("FIREWORKS_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .fireworks
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Moonshot)
        && let Ok(value) = std::env::var("MOONSHOT_MODEL")
            .or_else(|_| std::env::var("KIMI_MODEL_NAME"))
            .or_else(|_| std::env::var("KIMI_MODEL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .moonshot
            .model = Some(value);
    }
    let active_provider = config.api_provider();
    if matches!(
        active_provider,
        ApiProvider::Siliconflow | ApiProvider::SiliconflowCn
    ) && let Ok(value) = std::env::var("SILICONFLOW_MODEL")
        && !value.trim().is_empty()
    {
        config.provider_config_for_mut(active_provider).model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Arcee)
        && let Ok(value) = std::env::var("ARCEE_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .arcee
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Huggingface)
        && let Ok(value) = std::env::var("HUGGINGFACE_MODEL").or_else(|_| std::env::var("HF_MODEL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .huggingface
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
        // Capture the custom entry key before the mutable borrow below (#1519).
        let custom_key = (provider == ApiProvider::Custom).then(|| {
            config
                .provider
                .clone()
                .unwrap_or_else(|| "__custom__".to_string())
        });
        if matches!(
            provider,
            ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic
        ) {
            config.default_text_model = Some(value);
        } else {
            let providers = config
                .providers
                .get_or_insert_with(ProvidersConfig::default);
            let entry = match provider {
                ApiProvider::Deepseek
                | ApiProvider::DeepseekCN
                | ApiProvider::DeepseekAnthropic => unreachable!(
                    "DeepSeek providers are handled in the if branch above (issue #1714)"
                ),
                ApiProvider::Custom => providers
                    .custom
                    .entry(custom_key.expect("custom key captured for custom provider"))
                    .or_default(),
                ApiProvider::NvidiaNim => &mut providers.nvidia_nim,
                ApiProvider::Openai => &mut providers.openai,
                ApiProvider::Atlascloud => &mut providers.atlascloud,
                ApiProvider::WanjieArk => &mut providers.wanjie_ark,
                ApiProvider::Openrouter => &mut providers.openrouter,
                ApiProvider::XiaomiMimo => &mut providers.xiaomi_mimo,
                ApiProvider::Novita => &mut providers.novita,
                ApiProvider::Fireworks => &mut providers.fireworks,
                ApiProvider::Siliconflow => &mut providers.siliconflow,
                ApiProvider::SiliconflowCn => &mut providers.siliconflow_cn,
                ApiProvider::Arcee => &mut providers.arcee,
                ApiProvider::Moonshot => &mut providers.moonshot,
                ApiProvider::Sglang => &mut providers.sglang,
                ApiProvider::Vllm => &mut providers.vllm,
                ApiProvider::Ollama => &mut providers.ollama,
                ApiProvider::Volcengine => &mut providers.volcengine,
                ApiProvider::Huggingface => &mut providers.huggingface,
                ApiProvider::Deepinfra => &mut providers.deepinfra,
                ApiProvider::Together => &mut providers.together,
                ApiProvider::Qianfan => &mut providers.qianfan,
                ApiProvider::OpenaiCodex => &mut providers.openai_codex,
                ApiProvider::OpenCodeZen => &mut providers.opencode_zen,
                ApiProvider::Anthropic => &mut providers.anthropic,
                ApiProvider::Openmodel => &mut providers.openmodel,
                ApiProvider::Zai => &mut providers.zai,
                ApiProvider::Stepfun => &mut providers.stepfun,
                ApiProvider::Minimax => &mut providers.minimax,
                ApiProvider::Sakana => &mut providers.sakana,
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
    if let Ok(value) =
        std::env::var("CODEWHALE_VERBOSITY").or_else(|_| std::env::var("DEEPSEEK_VERBOSITY"))
    {
        config.verbosity = Some(value);
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
    if let Ok(value) = codewhale_env_var("CODEWHALE_SEARCH_BASE_URL", "DEEPSEEK_SEARCH_BASE_URL") {
        config
            .search
            .get_or_insert_with(SearchConfig::default)
            .base_url = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_REQUIREMENTS_PATH") {
        config.requirements_path = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_MAX_SUBAGENTS")
        && let Ok(parsed) = value.parse::<usize>()
    {
        config.max_subagents = Some(parsed.clamp(1, MAX_SUBAGENTS));
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
        if let Some(model) = providers.siliconflow_cn.model.as_deref()
            && !provider_entry_uses_custom_base_url(
                ApiProvider::SiliconflowCn,
                &providers.siliconflow_cn,
            )
            && let Some(normalized) =
                normalize_model_for_provider(ApiProvider::SiliconflowCn, model)
        {
            providers.siliconflow_cn.model = Some(normalized);
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
        if let Some(model) = providers.deepinfra.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::Deepinfra, &providers.deepinfra)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Deepinfra, model)
        {
            providers.deepinfra.model = Some(normalized);
        }
    }
}

fn normalize_model_for_provider(provider: ApiProvider, model: &str) -> Option<String> {
    if matches!(provider, ApiProvider::XiaomiMimo)
        && let Some(canonical) = canonical_xiaomi_mimo_model_id(model)
    {
        return Some(canonical.to_string());
    }
    if provider_passes_model_through(provider) {
        return None;
    }
    normalize_model_name_for_provider(provider, model)
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
            | ApiProvider::Qianfan
            | ApiProvider::Openmodel
            | ApiProvider::Ollama
            | ApiProvider::Huggingface
            // Custom OpenAI-compatible endpoints preserve user-supplied model
            // ids verbatim (#1519); never normalize/rewrite them.
            | ApiProvider::Custom
    )
}

fn provider_entry_uses_custom_base_url(provider: ApiProvider, entry: &ProviderConfig) -> bool {
    entry
        .base_url
        .as_deref()
        .is_some_and(|base_url| provider_preserves_custom_base_url_model(provider, base_url))
}

fn default_base_url_for_provider(provider: ApiProvider) -> &'static str {
    provider.default_base_url()
}

fn xiaomi_mimo_base_url_for_mode(mode: &str) -> Option<&'static str> {
    let normalized = mode.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    if normalized.is_empty() || xiaomi_mimo_mode_uses_standard_endpoint(&normalized) {
        return None;
    }
    Some(match normalized.as_str() {
        "token-plan" | "tokenplan" | "subscription" | "subscribed" | "plan" => {
            DEFAULT_XIAOMI_MIMO_BASE_URL
        }
        "token-plan-cn"
        | "token-plan-china"
        | "token-plan-mainland"
        | "token-plan-mainland-china"
        | "cn"
        | "china" => XIAOMI_MIMO_TOKEN_PLAN_CN_BASE_URL,
        "token-plan-sgp"
        | "token-plan-sg"
        | "token-plan-singapore"
        | "sgp"
        | "sg"
        | "singapore" => XIAOMI_MIMO_TOKEN_PLAN_SGP_BASE_URL,
        "token-plan-ams"
        | "token-plan-eu"
        | "token-plan-europe"
        | "token-plan-amsterdam"
        | "ams"
        | "eu"
        | "europe"
        | "amsterdam" => XIAOMI_MIMO_TOKEN_PLAN_AMS_BASE_URL,
        _ => DEFAULT_XIAOMI_MIMO_BASE_URL,
    })
}

fn xiaomi_mimo_mode_uses_standard_endpoint(normalized_mode: &str) -> bool {
    matches!(
        normalized_mode,
        "standard" | "default" | "payg" | "paygo" | "pay-as-you-go" | "pay-as-go"
    )
}

fn xiaomi_mimo_base_url_uses_token_plan(base_url: &str) -> bool {
    let normalized = normalize_base_url(base_url).to_ascii_lowercase();
    normalized == XIAOMI_MIMO_TOKEN_PLAN_CN_BASE_URL
        || normalized == XIAOMI_MIMO_TOKEN_PLAN_SGP_BASE_URL
        || normalized == XIAOMI_MIMO_TOKEN_PLAN_AMS_BASE_URL
}

fn xiaomi_mimo_env_var(candidates: &[&str]) -> Option<String> {
    candidates.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

fn xiaomi_mimo_env_api_key_for_runtime(
    mode: Option<&str>,
    base_url: Option<&str>,
) -> Option<String> {
    const TOKEN_PLAN_ENV_VARS: &[&str] =
        &["XIAOMI_MIMO_TOKEN_PLAN_API_KEY", "MIMO_TOKEN_PLAN_API_KEY"];
    const STANDARD_ENV_VARS: &[&str] = &["XIAOMI_MIMO_API_KEY", "XIAOMI_API_KEY", "MIMO_API_KEY"];

    let normalized_mode =
        mode.map(|value| value.trim().to_ascii_lowercase().replace(['_', ' '], "-"));
    let standard_selected = normalized_mode
        .as_deref()
        .is_some_and(xiaomi_mimo_mode_uses_standard_endpoint)
        || base_url.is_some_and(xiaomi_mimo_base_url_is_pay_as_you_go);
    if standard_selected {
        return xiaomi_mimo_env_var(STANDARD_ENV_VARS);
    }

    let token_plan_selected = normalized_mode
        .as_deref()
        .and_then(xiaomi_mimo_base_url_for_mode)
        .is_some()
        || base_url.is_some_and(xiaomi_mimo_base_url_uses_token_plan);
    if token_plan_selected {
        return xiaomi_mimo_env_var(TOKEN_PLAN_ENV_VARS);
    }

    xiaomi_mimo_env_var(TOKEN_PLAN_ENV_VARS).or_else(|| xiaomi_mimo_env_var(STANDARD_ENV_VARS))
}

fn resolve_xiaomi_mimo_base_url(
    configured: Option<String>,
    api_key: Option<&str>,
    mode: Option<&str>,
) -> String {
    let normalized_mode =
        mode.map(|value| value.trim().to_ascii_lowercase().replace(['_', ' '], "-"));
    let uses_standard_mode = normalized_mode
        .as_deref()
        .is_some_and(xiaomi_mimo_mode_uses_standard_endpoint);
    let mode_base_url = normalized_mode
        .as_deref()
        .and_then(xiaomi_mimo_base_url_for_mode);
    let uses_token_plan = xiaomi_mimo_api_key_uses_token_plan(api_key);
    match configured {
        Some(base_url) if uses_standard_mode => base_url,
        Some(base_url) if uses_token_plan && xiaomi_mimo_base_url_is_pay_as_you_go(&base_url) => {
            mode_base_url
                .unwrap_or(DEFAULT_XIAOMI_MIMO_BASE_URL)
                .to_string()
        }
        Some(base_url) => base_url,
        None => {
            if let Some(base_url) = mode_base_url {
                base_url.to_string()
            } else if uses_standard_mode {
                XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL.to_string()
            } else if uses_token_plan || api_key.is_none() {
                DEFAULT_XIAOMI_MIMO_BASE_URL.to_string()
            } else {
                XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL.to_string()
            }
        }
    }
}

fn xiaomi_mimo_api_key_uses_token_plan(api_key: Option<&str>) -> bool {
    api_key.is_some_and(|key| key.trim_start().starts_with("tp-"))
}

fn xiaomi_mimo_base_url_is_pay_as_you_go(base_url: &str) -> bool {
    matches!(
        normalize_base_url(base_url).to_ascii_lowercase().as_str(),
        "https://api.xiaomimimo.com" | "https://api.xiaomimimo.com/v1"
    )
}

fn base_url_is_custom_for_provider(provider: ApiProvider, base_url: &str) -> bool {
    if (provider == ApiProvider::Siliconflow || provider == ApiProvider::SiliconflowCn)
        && siliconflow_base_url_is_official(base_url)
    {
        return false;
    }
    if provider == ApiProvider::XiaomiMimo
        && (xiaomi_mimo_base_url_uses_token_plan(base_url)
            || xiaomi_mimo_base_url_is_pay_as_you_go(base_url))
    {
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
        (
            ApiProvider::Siliconflow | ApiProvider::SiliconflowCn,
            "deepseek-v4-pro" | "deepseek-reasoner" | "deepseek-r1",
        ) => DEFAULT_SILICONFLOW_MODEL.to_string(),
        (
            ApiProvider::Siliconflow | ApiProvider::SiliconflowCn,
            "deepseek-v4-flash" | "deepseek-chat" | "deepseek-v3",
        ) => DEFAULT_SILICONFLOW_FLASH_MODEL.to_string(),
        (ApiProvider::Sglang, "deepseek-v4-pro") => DEFAULT_SGLANG_MODEL.to_string(),
        (ApiProvider::Sglang, "deepseek-v4-flash") => DEFAULT_SGLANG_FLASH_MODEL.to_string(),
        (ApiProvider::Vllm, "deepseek-v4-pro") => DEFAULT_VLLM_MODEL.to_string(),
        (ApiProvider::Vllm, "deepseek-v4-flash") => DEFAULT_VLLM_FLASH_MODEL.to_string(),
        (ApiProvider::Deepinfra, "deepseek-v4-pro" | "deepseek-v4pro") => {
            DEFAULT_DEEPINFRA_MODEL.to_string()
        }
        (ApiProvider::Deepinfra, "deepseek-v4-flash" | "deepseek-chat" | "deepseek-reasoner") => {
            DEFAULT_DEEPINFRA_FLASH_MODEL.to_string()
        }
        (ApiProvider::Together, "deepseek-v4-pro" | "deepseek-v4pro") => {
            DEFAULT_TOGETHER_MODEL.to_string()
        }
        (
            ApiProvider::Together,
            "deepseek-v4-flash" | "deepseek-v4flash" | "deepseek-chat" | "deepseek-reasoner",
        ) => DEFAULT_TOGETHER_FLASH_MODEL.to_string(),
        (
            ApiProvider::Moonshot,
            "kimi"
            | "kimi-k2"
            | "kimi-k2.7"
            | "kimi-k2-7"
            | "kimi-k2.7-code"
            | "kimi-k2-7-code"
            | "kimi-code"
            | "moonshot-kimi-k2.7-code",
        ) => DEFAULT_MOONSHOT_MODEL.to_string(),
        (ApiProvider::Moonshot, "kimi-k2.6" | "kimi-k2-6" | "moonshot-kimi-k2.6") => {
            MOONSHOT_KIMI_K2_6_MODEL.to_string()
        }
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
        mcp_oauth_callback_port: override_cfg
            .mcp_oauth_callback_port
            .or(base.mcp_oauth_callback_port),
        mcp_oauth_callback_url: override_cfg
            .mcp_oauth_callback_url
            .or(base.mcp_oauth_callback_url),
        notes_path: override_cfg.notes_path.or(base.notes_path),
        memory_path: override_cfg.memory_path.or(base.memory_path),
        vision_model: override_cfg.vision_model.or(base.vision_model),
        // #454: user-owned overlays such as profiles and managed config may
        // replace the instruction array. Project-scope config is filtered in
        // main.rs and cannot set instruction paths.
        instructions: override_cfg.instructions.or(base.instructions),
        allow_shell: override_cfg.allow_shell.or(base.allow_shell),
        prompt_suggestion: override_cfg.prompt_suggestion.or(base.prompt_suggestion),
        yolo: override_cfg.yolo.or(base.yolo),
        verbosity: override_cfg.verbosity.or(base.verbosity),
        approval_policy: override_cfg.approval_policy.or(base.approval_policy),
        sandbox_mode: override_cfg.sandbox_mode.or(base.sandbox_mode),
        fallback_providers: if override_cfg.fallback_providers.is_empty() {
            base.fallback_providers
        } else {
            override_cfg.fallback_providers
        },
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
        auto_review: override_cfg.auto_review.or(base.auto_review),
        tui: override_cfg.tui.or(base.tui),
        hooks: override_cfg.hooks.or(base.hooks),
        providers: merge_providers(base.providers, override_cfg.providers),
        features: merge_features(base.features, override_cfg.features),
        notifications: override_cfg.notifications.or(base.notifications),
        network: override_cfg.network.or(base.network),
        verifier: override_cfg.verifier.or(base.verifier),
        skills: merge_skills_config(base.skills, override_cfg.skills),
        snapshots: override_cfg.snapshots.or(base.snapshots),
        search: override_cfg.search.or(base.search),
        memory: override_cfg.memory.or(base.memory),
        speech: override_cfg.speech.or(base.speech),
        auto: override_cfg.auto.or(base.auto),
        hotbar: override_cfg.hotbar.or(base.hotbar),
        update: override_cfg.update.or(base.update),
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
            seam_model: override_cfg.context.seam_model.or(base.context.seam_model),
        },
        fleet: override_cfg.fleet.or(base.fleet),
        subagents: override_cfg.subagents.or(base.subagents),
        strict_tool_mode: override_cfg.strict_tool_mode.or(base.strict_tool_mode),
        runtime_api: override_cfg.runtime_api.or(base.runtime_api),
        workshop: override_cfg.workshop.or(base.workshop),
        exec_policy_engine: override_cfg.exec_policy_engine,
    }
}

fn load_sibling_exec_policy_engine(config_path: Option<&Path>) -> Result<ExecPolicyEngine> {
    let Some(config_path) = config_path else {
        return Ok(ExecPolicyEngine::new(Vec::new(), Vec::new()));
    };
    let permissions_path = codewhale_config::permissions_path_for_config_path(config_path);
    if !permissions_path.exists() {
        return Ok(ExecPolicyEngine::new(Vec::new(), Vec::new()));
    }

    let raw = fs::read_to_string(&permissions_path).with_context(|| {
        format!(
            "Failed to read permissions file: {}",
            permissions_path.display()
        )
    })?;
    let permissions: codewhale_config::PermissionsToml =
        toml::from_str(&raw).with_context(|| {
            format!(
                "Failed to parse permissions file: {}",
                permissions_path.display()
            )
        })?;
    if permissions.is_empty() {
        Ok(ExecPolicyEngine::new(Vec::new(), Vec::new()))
    } else {
        Ok(ExecPolicyEngine::with_rulesets(vec![permissions.ruleset()]))
    }
}

fn merge_skills_config(
    base: Option<SkillsConfig>,
    override_cfg: Option<SkillsConfig>,
) -> Option<SkillsConfig> {
    match (base, override_cfg) {
        (None, None) => None,
        (Some(base), None) => Some(base),
        (None, Some(override_cfg)) => Some(override_cfg),
        (Some(base), Some(override_cfg)) => Some(SkillsConfig {
            registry_url: override_cfg.registry_url.or(base.registry_url),
            max_install_size_bytes: override_cfg
                .max_install_size_bytes
                .or(base.max_install_size_bytes),
            scan_codewhale_only: override_cfg
                .scan_codewhale_only
                .or(base.scan_codewhale_only),
        }),
    }
}

fn merge_provider_config(base: ProviderConfig, override_cfg: ProviderConfig) -> ProviderConfig {
    ProviderConfig {
        api_key: override_cfg.api_key.or(base.api_key),
        base_url: override_cfg.base_url.or(base.base_url),
        model: override_cfg.model.or(base.model),
        context_window: override_cfg.context_window.or(base.context_window),
        mode: override_cfg.mode.or(base.mode),
        auth_mode: override_cfg.auth_mode.or(base.auth_mode),
        insecure_skip_tls_verify: override_cfg
            .insecure_skip_tls_verify
            .or(base.insecure_skip_tls_verify),
        http_headers: override_cfg.http_headers.or(base.http_headers),
        path_suffix: override_cfg.path_suffix.or(base.path_suffix),
        reasoning_stream_style: override_cfg
            .reasoning_stream_style
            .or(base.reasoning_stream_style),
        max_concurrency: override_cfg.max_concurrency.or(base.max_concurrency),
        auth: override_cfg.auth.or(base.auth),
        kind: override_cfg.kind.or(base.kind),
        api_key_env: override_cfg.api_key_env.or(base.api_key_env),
    }
}

/// Merge the per-name custom provider maps (#1519): the union of both key sets,
/// with each shared key deep-merged via [`merge_provider_config`] (override
/// wins field-by-field). Keys present in only one map are carried through as-is.
fn merge_custom_providers(
    mut base: HashMap<String, ProviderConfig>,
    override_cfg: HashMap<String, ProviderConfig>,
) -> HashMap<String, ProviderConfig> {
    for (name, entry) in override_cfg {
        let merged = match base.remove(&name) {
            Some(base_entry) => merge_provider_config(base_entry, entry),
            None => entry,
        };
        base.insert(name, merged);
    }
    base
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
            deepseek_anthropic: merge_provider_config(
                base.deepseek_anthropic,
                override_cfg.deepseek_anthropic,
            ),
            nvidia_nim: merge_provider_config(base.nvidia_nim, override_cfg.nvidia_nim),
            openai: merge_provider_config(base.openai, override_cfg.openai),
            anthropic: merge_provider_config(base.anthropic, override_cfg.anthropic),
            openmodel: merge_provider_config(base.openmodel, override_cfg.openmodel),
            atlascloud: merge_provider_config(base.atlascloud, override_cfg.atlascloud),
            wanjie_ark: merge_provider_config(base.wanjie_ark, override_cfg.wanjie_ark),
            openrouter: merge_provider_config(base.openrouter, override_cfg.openrouter),
            xiaomi_mimo: merge_provider_config(base.xiaomi_mimo, override_cfg.xiaomi_mimo),
            novita: merge_provider_config(base.novita, override_cfg.novita),
            fireworks: merge_provider_config(base.fireworks, override_cfg.fireworks),
            siliconflow: merge_provider_config(base.siliconflow, override_cfg.siliconflow),
            siliconflow_cn: merge_provider_config(base.siliconflow_cn, override_cfg.siliconflow_cn),
            arcee: merge_provider_config(base.arcee, override_cfg.arcee),
            moonshot: merge_provider_config(base.moonshot, override_cfg.moonshot),
            sglang: merge_provider_config(base.sglang, override_cfg.sglang),
            vllm: merge_provider_config(base.vllm, override_cfg.vllm),
            ollama: merge_provider_config(base.ollama, override_cfg.ollama),
            volcengine: merge_provider_config(base.volcengine, override_cfg.volcengine),
            huggingface: merge_provider_config(base.huggingface, override_cfg.huggingface),
            deepinfra: merge_provider_config(base.deepinfra, override_cfg.deepinfra),
            together: merge_provider_config(base.together, override_cfg.together),
            qianfan: merge_provider_config(base.qianfan, override_cfg.qianfan),
            openai_codex: merge_provider_config(base.openai_codex, override_cfg.openai_codex),
            zai: merge_provider_config(base.zai, override_cfg.zai),
            stepfun: merge_provider_config(base.stepfun, override_cfg.stepfun),
            minimax: merge_provider_config(base.minimax, override_cfg.minimax),
            sakana: merge_provider_config(base.sakana, override_cfg.sakana),
            opencode_zen: merge_provider_config(base.opencode_zen, override_cfg.opencode_zen),
            custom: merge_custom_providers(base.custom, override_cfg.custom),
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

/// Build a one-line warning when top-level-only keys are nested under a section
/// CodeWhale does not define (`[general]` / `[sandbox]`). TOML silently drops
/// those keys, so e.g. `[general]\nallow_shell = true` never takes effect and
/// the shell tools (`exec_shell`, `task_shell_start`, …) are absent from the
/// catalog with no explanation. Returns `None` when nothing is misplaced.
///
/// This is the exact confusion behind #2589: `allow_shell` and `sandbox_mode`
/// belong at the top of the file, above any `[section]` header.
fn warn_on_misplaced_top_level_keys(raw: &str) -> Option<String> {
    let doc = toml::from_str::<toml::Value>(raw).ok()?;
    // Sections CodeWhale does not recognize but users nest settings under.
    const UNKNOWN_SECTIONS: &[&str] = &["general", "sandbox"];
    // Keys that are only ever read from the top level of the config.
    const TOP_LEVEL_KEYS: &[&str] = &[
        "allow_shell",
        "sandbox_mode",
        "approval_policy",
        "verbosity",
    ];

    let mut hits: Vec<String> = Vec::new();
    for section in UNKNOWN_SECTIONS {
        let Some(table) = doc.get(*section).and_then(toml::Value::as_table) else {
            continue;
        };
        for key in TOP_LEVEL_KEYS {
            if table.contains_key(*key) {
                hits.push(format!("`{section}.{key}`"));
            }
        }
    }
    if hits.is_empty() {
        return None;
    }
    Some(format!(
        "Ignoring {} — CodeWhale has no `[general]` or `[sandbox]` section, so these \
         keys are silently dropped. Move them to the TOP of the config file (above any \
         `[section]` header), e.g. `allow_shell = true`. Until then, shell tools stay \
         disabled. (#2589)",
        hits.join(", ")
    ))
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
    /// (`"system keyring"`, `"file-based (~/.codewhale/secrets/)"`).
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
/// **Dual-write strategy (#593):** writes to `~/.codewhale/config.toml`
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
/// `~/.codewhale/config.toml`.
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
    if provider == ApiProvider::OpenaiCodex {
        // The persistent Codex login is the OAuth credential file, analogous to
        // a stored config key. Token env overrides are scored separately by
        // active_provider_has_env_api_key.
        return crate::oauth::auth_file_path().exists();
    }
    if provider == ApiProvider::OpenCodeZen {
        return config
            .provider_config_for(provider)
            .and_then(|c| c.api_key.as_deref())
            .is_some_and(|k| !k.is_empty());
    }
    if matches!(provider, ApiProvider::Huggingface)
        && std::env::var("HUGGINGFACE_API_KEY")
            .or_else(|_| std::env::var("HF_TOKEN"))
            .is_ok_and(|k| !k.trim().is_empty())
    {
        return true;
    }

    if config
        .provider_config_string_with_runtime_fallback(provider, |entry| entry.api_key.clone())
        .is_some_and(|k| !k.trim().is_empty() && k != API_KEYRING_SENTINEL)
    {
        return true;
    }
    if config
        .provider_config_for(provider)
        .and_then(|entry| entry.auth.as_ref())
        .is_some_and(|auth| auth.validate().is_ok())
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
    provider_env_api_key(config.api_provider()).is_some()
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
    if provider
        .env_vars()
        .iter()
        .any(|var| std::env::var(var).is_ok_and(|k| !k.trim().is_empty()))
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
    if provider == ApiProvider::OpenaiCodex {
        // Token env overrides are checked above; also honor the Codex CLI OAuth
        // login on disk.
        return crate::oauth::auth_file_path().exists();
    }
    if provider == ApiProvider::OpenCodeZen {
        return config
            .provider_config_for(provider)
            .and_then(|c| c.api_key.as_deref())
            .is_some_and(|k| !k.is_empty());
    }

    // Self-hosted providers typically run without authentication.
    if provider.is_self_hosted() {
        return true;
    }

    if provider == config.api_provider() && base_url_uses_local_host(&config.deepseek_base_url()) {
        return true;
    }

    if config
        .provider_config_string_with_runtime_fallback(provider, |entry| entry.api_key.clone())
        .is_some_and(|k| !k.trim().is_empty() && k != API_KEYRING_SENTINEL)
    {
        return true;
    }
    if config
        .provider_config_for(provider)
        .and_then(|entry| entry.auth.as_ref())
        .is_some_and(|auth| auth.validate().is_ok())
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
/// `[providers.<name>] api_key = "..."` to `~/.codewhale/config.toml`.
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

    let key_inside = provider_config_key(provider).context("provider api key table")?;
    let table_name = format!("providers.{key_inside}");

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
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        anyhow::bail!("DeepSeek stores auth at the root config level");
    }
    provider
        .metadata()
        .map(|metadata| metadata.provider_config_key())
        .context("provider config key")
}

fn provider_config_table_name(provider: ApiProvider) -> Result<String> {
    Ok(format!("providers.{}", provider_config_key(provider)?))
}

fn provider_env_api_key(provider: ApiProvider) -> Option<String> {
    if provider == ApiProvider::Huggingface {
        return std::env::var("HUGGINGFACE_API_KEY")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                std::env::var("HF_TOKEN")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            });
    }

    provider.env_vars().iter().find_map(|var| {
        std::env::var(var)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

fn missing_provider_api_key_message(provider: ApiProvider) -> Result<String> {
    let credential_hint = provider
        .credential_url()
        .map(|url| format!(" Get a key: {url}."))
        .unwrap_or_default();
    Ok(format!(
        "{} API key not found.{} Run 'codewhale auth set --provider {}', set {}, or add [{}] api_key in ~/.codewhale/config.toml.",
        provider.display_name(),
        credential_hint,
        provider.as_str(),
        provider.env_vars_label(),
        provider_config_table_name(provider)?
    ))
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
    let client = crate::tls::reqwest_blocking_client_builder()
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
    if let Some(kimi_code_home) = kimi_code_home_override() {
        return Ok(kimi_oauth_credential_path(kimi_code_home));
    }

    let modern_path = effective_home_dir()
        .map(|home| kimi_oauth_credential_path(home.join(".kimi-code")))
        .context("Failed to resolve Kimi Code home directory")?;
    if modern_path.exists() {
        return Ok(modern_path);
    }

    if let Some(legacy_share_dir) = kimi_legacy_share_dir_override() {
        return Ok(kimi_oauth_credential_path(legacy_share_dir));
    }

    if let Some(legacy_path) = effective_home_dir()
        .map(|home| kimi_oauth_credential_path(home.join(".kimi")))
        .filter(|path| path.exists())
    {
        return Ok(legacy_path);
    }

    Ok(modern_path)
}

fn kimi_code_home_override() -> Option<PathBuf> {
    std::env::var_os("KIMI_CODE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn kimi_legacy_share_dir_override() -> Option<PathBuf> {
    std::env::var_os("KIMI_SHARE_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn kimi_oauth_credential_path(home: PathBuf) -> PathBuf {
    home.join("credentials").join(KIMI_CODE_CREDENTIAL_FILE)
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

/// Clear only the active provider's API key from the config file.
/// Unlike `clear_api_key()` which strips ALL api_key lines, this
/// removes only the key for the specified provider section.
pub fn clear_active_provider_api_key(provider: &str) -> Result<()> {
    let config_path = default_config_path()
        .context("Failed to resolve config path: home directory not found.")?;

    if !config_path.exists() {
        return Ok(());
    }

    let existing = fs::read_to_string(&config_path)?;
    let mut result = String::new();
    let target_section = format!("[providers.{provider}]");
    let mut in_target_section = false;

    for line in existing.lines() {
        let trimmed = line.trim();

        // Track which [providers.X] section we're in.
        if trimmed.starts_with("[providers.") {
            in_target_section = trimmed == target_section;
        } else if trimmed.starts_with('[') {
            in_target_section = false;
        }

        // For the root section (before any [headers]), clear api_key
        // only if the provider is "deepseek" (root-level key).
        let is_root_key = !in_target_section
            && provider == "deepseek"
            && trimmed.strip_prefix("api_key").is_some_and(|rest| {
                let rest = rest.trim_start();
                rest.is_empty() || rest.starts_with('=')
            });

        // For a provider section, clear api_key if we're in the target section.
        let is_provider_key = in_target_section
            && trimmed.strip_prefix("api_key").is_some_and(|rest| {
                let rest = rest.trim_start();
                rest.is_empty() || rest.starts_with('=')
            });

        if is_root_key || is_provider_key {
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
            "scope": provider,
        }),
    );

    Ok(())
}

#[cfg(test)]
mod tests;
