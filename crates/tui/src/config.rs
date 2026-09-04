//! Configuration loading and defaults for codewhale.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::{Context, Result};
use codewhale_execpolicy::ExecPolicyEngine;
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::audit::log_sensitive_event;
use crate::credentials::CredentialStore;
use crate::features::{Feature, Features, FeaturesToml, is_known_feature_key};
use crate::hooks::HooksConfig;

// Sub-agent concurrency/timeout limit constants and their clamp resolvers live
// in the `subagent_limits` leaf module. The constants are re-exported (keeping
// each item's visibility) so `crate::config::<CONST>` paths resolve unchanged;
// the private resolvers are pulled back in without widening external surface
// (#3311).
#[cfg(test)]
mod scope_tests;
// The single place provider credential precedence is decided. Lives inside
// `config` so it can walk the private probe helpers without widening their
// visibility; `has_api_key_for` is a thin wrapper over it (#pi-auth-port).
mod credential_resolve;
pub(crate) use credential_resolve::resolve_credential_source;
mod subagent_limits;
pub use subagent_limits::*;
use subagent_limits::{resolve_subagent_api_timeout_secs, resolve_subagent_heartbeat_timeout_secs};

// Provider model-name and base-URL constants live in the `models` leaf module
// and are re-exported below so every `crate::config::<CONST>` path is unchanged
// (#3311).
mod models;
pub use models::*;

#[cfg(test)]
pub(crate) use codewhale_config::API_KEYRING_SENTINEL;
pub(crate) use codewhale_config::{ConfigApiKeyValueKind, classify_config_api_key_value};

pub const DEFAULT_ZAI_PROVIDER_MAX_CONCURRENCY: usize = 3;
pub const MAX_PROVIDER_REQUEST_CONCURRENCY: usize = 64;

/// Default maximum number of automatic re-requests when a reasoning model
/// returns only hidden thinking without any answer text or tool call.
pub const DEFAULT_REASONING_ONLY_REPROMPTS: u32 = 2;

/// Default custom message sent to the model on each re-request.
/// Used when the user does not set `[reasoning_only] reprompt_message`.
pub const DEFAULT_REASONING_ONLY_REPROMPT_MESSAGE: &str = "So, what's up ? Keep running !";

pub fn default_stop_words() -> Vec<String> {
    ["stop", "wait", "pause"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

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
    Orcarouter,
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
    OllamaCloud,
    Huggingface,
    Together,
    Qianfan,
    OpenaiCodex,
    Anthropic,
    Openmodel,
    Zai,
    Stepfun,
    Minimax,
    MinimaxAnthropic,
    Deepinfra,
    Sakana,
    LongCat,
    OpencodeGo,
    OpencodeZen,
    Meta,
    Xai,
    /// Mistral AI — la Plateforme (OpenAI-compatible Chat Completions).
    Mistral,
    /// Google Gemini — official OpenAI-compatible endpoint. A distinct
    /// backend, not an OpenAI alias: thought signatures on tool calls are
    /// captured and replayed per Google's contract.
    Google,
    /// Google Antigravity (`agy`). Consent-gated read-only import of the
    /// official CLI's login, then a text-only cloud-code stream
    /// (`/v1internal:streamGenerateContent`). Tools and non-text parts
    /// fail closed.
    Antigravity,
    /// Jiangsu Telecom TokenHub — OpenAI-compatible AI gateway.
    Telecomjs,
    /// Eden AI — OpenAI-compatible AI gateway (aggregator).
    Edenai,
    /// Concentrate — OpenAI Responses-compatible AI gateway (aggregator; BYOK only).
    Concentrate,
    /// Alibaba Cloud Model Studio — Token Plan (OpenAI-compatible Chat Completions).
    ModelstudioTokenPlan,
    /// Alibaba Cloud Model Studio — Token Plan Anthropic-compatible endpoint.
    ModelstudioTokenPlanAnthropic,
    /// Alibaba Cloud Model Studio — Coding Plan (OpenAI-compatible Chat Completions).
    ModelstudioCodingPlan,
    /// Alibaba Cloud Model Studio — Coding Plan Anthropic-compatible endpoint.
    ModelstudioCodingPlanAnthropic,
    /// User-defined OpenAI-compatible endpoint (#1519).
    ///
    /// Selected when `provider = "<name>"` names a `[providers.<name>]
    /// kind="openai-compatible"` table. A single dynamic identity that maps to
    /// [`codewhale_config::ProviderKind::Custom`] and routes via the OpenAI Chat
    /// Completions wire protocol; the concrete endpoint/model/auth come from the
    /// named config table, not from this variant.
    Custom,
}

/// Exact, non-secret provider identity resolved from live configuration.
///
/// Built-ins use their canonical slug (for example `openrouter`). Dynamic
/// custom providers keep the user-owned `[providers.<name>]` key so session
/// persistence never collapses `lm-studio` into the generic `custom` kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderIdentity {
    pub(crate) provider: ApiProvider,
    pub(crate) key: String,
    /// Additive exact configured provider id written by current persistence
    /// schemas. `None` is meaningful: it identifies the released legacy
    /// root-level `provider = "custom"` route and must never be upgraded to an
    /// exact `[providers.custom]` table merely because one exists later.
    pub(crate) exact_id: Option<String>,
    /// Runtime provenance for the released `ollama` + exact Cloud route.
    /// Persistence writes the canonical Cloud kind plus the original `ollama`
    /// id, then reconstructs this flag on resume; the flag itself is not
    /// serialized.
    pub(crate) migrated_legacy_ollama_cloud_route: bool,
}

impl ProviderIdentity {
    #[must_use]
    pub(crate) fn persisted_id(&self) -> Option<&str> {
        self.exact_id.as_deref()
    }
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
        // Legacy dual-wire slugs keep their own `[providers.<slug>]` tables,
        // credential slots, and default models even though catalog surfaces
        // collapse them onto the vendor primary (`ProviderKind::ALL`, and
        // `catalog_identity` for UI). `ProviderKind::parse` resolves these
        // spellings as primary aliases, which would orphan the legacy table a
        // pre-0.9.4 config actually selects: credentials, base_url, and model
        // pinned under `[providers.deepseek-anthropic]` /
        // `[providers.minimax-anthropic]` must keep resolving for
        // `provider = "deepseek-anthropic"` / `"minimax-anthropic"`.
        if trimmed.eq_ignore_ascii_case("deepseek-anthropic")
            || trimmed.eq_ignore_ascii_case("deepseek_anthropic")
            || trimmed.eq_ignore_ascii_case("deepseek-claude")
            || trimmed.eq_ignore_ascii_case("deepseek_claude")
        {
            return Some(Self::DeepseekAnthropic);
        }
        if trimmed.eq_ignore_ascii_case("minimax-anthropic")
            || trimmed.eq_ignore_ascii_case("minimax_anthropic")
            || trimmed.eq_ignore_ascii_case("mini-max-anthropic")
            || trimmed.eq_ignore_ascii_case("mini_max_anthropic")
        {
            return Some(Self::MinimaxAnthropic);
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
            // Mirror credential_help()/env_vars(): a variant without
            // registered metadata falls back to the DeepSeek defaults
            // instead of panicking at startup/render. The
            // all_provider_variants_have_metadata test guards the table.
            _ => self.metadata().map_or_else(
                || {
                    codewhale_config::ProviderKind::Deepseek
                        .provider()
                        .default_base_url()
                },
                |provider| provider.default_base_url(),
            ),
        }
    }

    /// Canonical credential acquisition metadata shared by provider surfaces.
    #[must_use]
    pub fn credential_help(self) -> codewhale_config::provider::CredentialHelp {
        self.metadata().map_or_else(
            || {
                codewhale_config::provider::provider_for_kind(
                    codewhale_config::ProviderKind::Deepseek,
                )
                .credential_help()
            },
            codewhale_config::provider::Provider::credential_help,
        )
    }

    /// Official provider page for creating or locating credentials.
    #[must_use]
    pub fn credential_url(self) -> Option<&'static str> {
        self.credential_help().credential_url
    }

    /// All providers including legacy dual-wire / plan-variant kinds.
    ///
    /// Prefer [`Self::catalog`] for pickers and other user-facing lists.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &Self::FROM_KIND_LOOKUP
    }

    /// User-facing catalog surface: one identity per vendor.
    ///
    /// Matches `ProviderKind::ALL` — dialect is `providers.<id>.wire`, plan is
    /// `mode` / base_url (Z.ai / Xiaomi shape), not extra ProviderKinds.
    #[must_use]
    pub fn catalog() -> &'static [Self] {
        static CATALOG: std::sync::OnceLock<Vec<ApiProvider>> = std::sync::OnceLock::new();
        CATALOG
            .get_or_init(|| {
                codewhale_config::ProviderKind::ALL
                    .iter()
                    .copied()
                    .map(Self::from_kind)
                    .collect()
            })
            .as_slice()
    }

    /// Collapse legacy dialect/plan kinds onto the vendor primary for UI.
    #[must_use]
    pub fn catalog_identity(self) -> Self {
        match self {
            Self::DeepseekAnthropic => Self::Deepseek,
            Self::MinimaxAnthropic => Self::Minimax,
            Self::ModelstudioTokenPlanAnthropic
            | Self::ModelstudioCodingPlan
            | Self::ModelstudioCodingPlanAnthropic => Self::ModelstudioTokenPlan,
            other => other,
        }
    }

    /// `ApiProvider` discriminant → `ProviderKind` lookup.
    /// Index 1 is `None` for the legacy `DeepseekCN` variant.
    const KIND_LOOKUP: [Option<codewhale_config::ProviderKind>; 49] = [
        Some(codewhale_config::ProviderKind::Deepseek),
        None, // DeepseekCN
        Some(codewhale_config::ProviderKind::DeepseekAnthropic),
        Some(codewhale_config::ProviderKind::NvidiaNim),
        Some(codewhale_config::ProviderKind::Openai),
        Some(codewhale_config::ProviderKind::Atlascloud),
        Some(codewhale_config::ProviderKind::WanjieArk),
        Some(codewhale_config::ProviderKind::Volcengine),
        Some(codewhale_config::ProviderKind::Openrouter),
        Some(codewhale_config::ProviderKind::Orcarouter),
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
        Some(codewhale_config::ProviderKind::OllamaCloud),
        Some(codewhale_config::ProviderKind::Huggingface),
        Some(codewhale_config::ProviderKind::Together),
        Some(codewhale_config::ProviderKind::Qianfan),
        Some(codewhale_config::ProviderKind::OpenaiCodex),
        Some(codewhale_config::ProviderKind::Anthropic),
        Some(codewhale_config::ProviderKind::Openmodel),
        Some(codewhale_config::ProviderKind::Zai),
        Some(codewhale_config::ProviderKind::Stepfun),
        Some(codewhale_config::ProviderKind::Minimax),
        Some(codewhale_config::ProviderKind::MinimaxAnthropic),
        Some(codewhale_config::ProviderKind::Deepinfra),
        Some(codewhale_config::ProviderKind::Sakana),
        Some(codewhale_config::ProviderKind::LongCat),
        Some(codewhale_config::ProviderKind::OpencodeGo),
        Some(codewhale_config::ProviderKind::OpencodeZen),
        Some(codewhale_config::ProviderKind::Meta),
        Some(codewhale_config::ProviderKind::Xai),
        Some(codewhale_config::ProviderKind::Mistral),
        Some(codewhale_config::ProviderKind::Google),
        Some(codewhale_config::ProviderKind::Antigravity),
        Some(codewhale_config::ProviderKind::Telecomjs),
        Some(codewhale_config::ProviderKind::Edenai),
        Some(codewhale_config::ProviderKind::Concentrate),
        Some(codewhale_config::ProviderKind::ModelstudioTokenPlan),
        Some(codewhale_config::ProviderKind::ModelstudioTokenPlanAnthropic),
        Some(codewhale_config::ProviderKind::ModelstudioCodingPlan),
        Some(codewhale_config::ProviderKind::ModelstudioCodingPlanAnthropic),
        Some(codewhale_config::ProviderKind::Custom),
    ];

    /// `ProviderKind` discriminant → `ApiProvider` lookup.
    const FROM_KIND_LOOKUP: [Self; 48] = [
        Self::Deepseek,
        Self::DeepseekAnthropic,
        Self::NvidiaNim,
        Self::Openai,
        Self::Atlascloud,
        Self::WanjieArk,
        Self::Volcengine,
        Self::Openrouter,
        Self::Orcarouter,
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
        Self::OllamaCloud,
        Self::Huggingface,
        Self::Together,
        Self::Qianfan,
        Self::OpenaiCodex,
        Self::Anthropic,
        Self::Openmodel,
        Self::Zai,
        Self::Stepfun,
        Self::Minimax,
        Self::MinimaxAnthropic,
        Self::Deepinfra,
        Self::Sakana,
        Self::LongCat,
        Self::OpencodeGo,
        Self::OpencodeZen,
        Self::Meta,
        Self::Xai,
        Self::Mistral,
        Self::Telecomjs,
        Self::ModelstudioTokenPlan,
        Self::ModelstudioTokenPlanAnthropic,
        Self::ModelstudioCodingPlan,
        Self::ModelstudioCodingPlanAnthropic,
        Self::Antigravity,
        Self::Google,
        Self::Edenai,
        Self::Concentrate,
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
        ApiProvider::Orcarouter => matches!(normalized.as_str(), "orcarouter" | "orca_router"),
        ApiProvider::Edenai => matches!(normalized.as_str(), "edenai" | "eden_ai"),
        ApiProvider::Concentrate => matches!(
            normalized.as_str(),
            "concentrate" | "concentrate_ai" | "concentrateai"
        ),
        ApiProvider::OpenaiCodex => matches!(
            normalized.as_str(),
            "openai_codex" | "codex" | "chatgpt" | "openai_chatgpt"
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
        ApiProvider::LongCat => matches!(
            normalized.as_str(),
            "longcat" | "long_cat" | "meituan_longcat" | "meituan"
        ),
        ApiProvider::OpencodeGo => {
            matches!(normalized.as_str(), "opencode_go" | "opencodego")
        }
        ApiProvider::OpencodeZen => matches!(
            normalized.as_str(),
            "opencode_zen" | "opencodezen" | "zen" | "opencode"
        ),
        ApiProvider::Meta => matches!(
            normalized.as_str(),
            "meta" | "meta_ai" | "meta_model_api" | "muse" | "muse_spark"
        ),
        ApiProvider::Xai => matches!(normalized.as_str(), "xai" | "x_ai" | "grok"),
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
    /// Known output ceiling for this provider/model metadata path, when one is
    /// actually known.
    ///
    /// `None` means "this route publishes no output maximum we can stand
    /// behind" — for example the Kimi Code membership ids, whose limits live in
    /// the membership catalog rather than the static model catalogue. Unknown
    /// must stay unknown: callers may **not** substitute a placeholder ceiling,
    /// and in particular [`crate::route_budget`] does not clamp a requested
    /// `max_tokens` against an unknown compatibility cap.
    ///
    /// When `Some`, the value is a documented exact-route maximum or a
    /// deliberately conservative provider ceiling (Anthropic's 64K floor, the
    /// Codex OAuth route). It is metadata for diagnostics and CI policy; normal
    /// turns use a separate, more conservative request cap in the engine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u32>,
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
    provider_capability_with_wire(provider, resolved_model, None)
}

/// Wire-aware variant of [`provider_capability`] that respects
/// `wire = "responses" | "anthropic" | "chat"` for `Custom` providers.
///
/// Built-ins keep their fixed policy; `Custom` defaults to `Chat` when `wire`
/// is absent so existing configs stay compatible. Mirrors
/// `crates/tui/src/client.rs::provider_wire_format_for_config` and the
/// `Custom` comment in `crates/config/src/provider.rs`.
#[must_use]
pub fn provider_capability_with_wire(
    provider: ApiProvider,
    resolved_model: &str,
    wire: Option<&str>,
) -> ProviderCapability {
    // Custom wire overrides must be checked before the generic fallback so
    // `[providers.<name>] wire = "responses"` / `"anthropic"` is honored.
    if provider == ApiProvider::Custom {
        if wire_config_prefers_anthropic(wire) {
            return ProviderCapability {
                provider,
                resolved_model: resolved_model.to_string(),
                context_window: crate::models::context_window_for_model(resolved_model)
                    .unwrap_or(crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS),
                max_output: crate::models::max_output_tokens_for_model(resolved_model),
                thinking_supported: crate::models::model_supports_reasoning(resolved_model),
                cache_telemetry_supported: false,
                request_payload_mode: RequestPayloadMode::AnthropicMessages,
                alias_deprecation: None,
            };
        }
        if wire_config_prefers_responses(wire) {
            return ProviderCapability {
                provider,
                resolved_model: resolved_model.to_string(),
                context_window: crate::models::context_window_for_model(resolved_model)
                    .unwrap_or(crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS),
                max_output: crate::models::max_output_tokens_for_model(resolved_model),
                thinking_supported: crate::models::model_supports_reasoning(resolved_model),
                cache_telemetry_supported: false,
                request_payload_mode: RequestPayloadMode::Responses,
                alias_deprecation: None,
            };
        }
    }

    if matches!(
        provider,
        ApiProvider::Anthropic | ApiProvider::MinimaxAnthropic | ApiProvider::Openmodel
    ) {
        return ProviderCapability {
            provider,
            resolved_model: resolved_model.to_string(),
            // 200K is the conservative Anthropic floor; 4.6+ models resolve
            // their 1M windows from models.rs rows (#3014).
            context_window: crate::models::context_window_for_model(resolved_model)
                .unwrap_or(200_000),
            // 64K is the documented Anthropic Messages floor. For a model
            // the catalogue describes this carries its documented ceiling;
            // for an unknown one it is an *assumed* floor, and
            // `route_budget::output_ceiling_source` labels it unverified so
            // no receipt renders it as "documented" (#5440).
            max_output: Some(
                crate::models::max_output_tokens_for_model(resolved_model).unwrap_or(64_000),
            ),
            thinking_supported: crate::models::model_supports_reasoning(resolved_model),
            cache_telemetry_supported: matches!(provider, ApiProvider::Anthropic),
            request_payload_mode: RequestPayloadMode::AnthropicMessages,
            alias_deprecation: None,
        };
    }

    if matches!(provider, ApiProvider::OpenaiCodex) {
        return ProviderCapability {
            provider,
            resolved_model: resolved_model.to_string(),
            context_window: OPENAI_CODEX_EFFECTIVE_CONTEXT_WINDOW_TOKENS,
            // The OAuth cache does not publish an output ceiling. This 4K is a
            // deliberate, long-standing product decision for the Codex route
            // (not a fallback): keep the compatibility capability conservative
            // instead of inheriting the public API model's output limit. It is
            // an assumption, not a documented fact — receipts label it
            // unverified (#5440).
            max_output: Some(4096),
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
            // No documented output maximum for these routes: stay unknown so
            // no compatibility clamp is applied downstream.
            max_output: crate::models::max_output_tokens_for_model(resolved_model),
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
            // No documented output maximum for these routes: stay unknown so
            // no compatibility clamp is applied downstream.
            max_output: crate::models::max_output_tokens_for_model(resolved_model),
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
    //
    // Everything else answers from the static model catalogue, and answers
    // `None` when the catalogue has no row. That is the truthful state for
    // membership routes such as the `kimi-for-coding` family, whose ceilings
    // are owned by the membership catalog. It must not become a placeholder
    // number: a fabricated 4K here silently clamped offline membership routes
    // to 4K output via `route_budget`.
    let max_output = if is_v4_pro || is_v4_flash {
        Some(384_000)
    } else {
        crate::models::max_output_tokens_for_model(resolved_model)
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
        ApiProvider::DeepseekAnthropic | ApiProvider::MinimaxAnthropic | ApiProvider::Openmodel
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
/// `v4pro`/`v4flash` spellings and the experimental vision shorthand are
/// rewritten to their hyphenated forms.
#[must_use]
pub fn canonical_model_name(model: &str) -> Option<&'static str> {
    match model.trim().to_ascii_lowercase().as_str() {
        "pro" | "deepseek-v4pro" => Some("deepseek-v4-pro"),
        "flash" | "deepseek-v4flash" => Some("deepseek-v4-flash"),
        "flash-vision" | "deepseek-v4flashvisionexp" => Some("deepseek-v4-flash-vision-exp"),
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
/// DeepSeek providers use the strict `normalize_model_name` gate (the official
/// API only accepts DeepSeek IDs). OpenCode Go uses its documented Chat
/// Completions allowlist because the shared Go roster also contains
/// Messages-only models. Other providers pass any non-empty,
/// non-control-character string through — the provider API is the authority.
#[must_use]
pub fn requested_model_for_provider(provider: ApiProvider, model: &str) -> Option<String> {
    match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic => {
            normalize_model_name(model)
        }
        ApiProvider::OpencodeGo => opencode_go_chat_model_id(model).map(str::to_string),
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
/// 3. OpenCode Go accepts only models documented for its Chat Completions
///    endpoint; models served only over Anthropic Messages are rejected.
///
/// Returns `Ok(())` for any tuple we cannot confidently reject (the provider
/// API remains the final authority for those).
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

    if provider == ApiProvider::OpencodeGo {
        return if opencode_go_chat_model_id(trimmed).is_some() {
            Ok(())
        } else {
            Err(format!(
                "Model '{trimmed}' is not available through OpenCode Go Chat Completions. \
                 Choose one of: {}.",
                OPENCODE_GO_CHAT_MODELS.join(", ")
            ))
        };
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

/// Resolve model names accepted by DeepSeek's first-party endpoints.
///
/// The legacy aliases are intentionally handled only in this direct-provider
/// layer. Aggregators and custom endpoints own their model namespaces; for
/// example, Wanjie Ark still documents `deepseek-reasoner` as its native id.
fn canonical_direct_deepseek_model_id(model: &str) -> Option<&'static str> {
    match model.trim().to_ascii_lowercase().as_str() {
        "deepseek-chat" | "deepseek-reasoner" => Some(DEEPSEEK_ALIAS_REPLACEMENT),
        _ => canonical_official_deepseek_model_id(model),
    }
}

fn legacy_deepseek_alias_reasoning_effort(model: &str) -> Option<&'static str> {
    match model.trim().to_ascii_lowercase().as_str() {
        // DeepSeek documents these retired aliases as the non-thinking and
        // thinking modes of V4 Flash, respectively. Keep that intent only
        // when the user has not already chosen an explicit reasoning tier.
        "deepseek-chat" => Some("off"),
        "deepseek-reasoner" => Some("high"),
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
        OPENROUTER_GLM_5_3_FLASH_MODEL
        | "glm-5.3-flash"
        | "glm-5-3-flash"
        | "zai-glm-5.3-flash"
        | "zai-glm-5-3-flash" => Some(OPENROUTER_GLM_5_3_FLASH_MODEL),
        OPENROUTER_GLM_5_3_MODEL | "glm-5.3" | "glm-5-3" | "zai-glm-5.3" | "zai-glm-5-3" => {
            Some(OPENROUTER_GLM_5_3_MODEL)
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
        OPENROUTER_QWEN_3_7_PLUS_MODEL | "qwen3.7-plus" | "qwen-3.7-plus" => {
            Some(OPENROUTER_QWEN_3_7_PLUS_MODEL)
        }
        OPENROUTER_QWEN_3_7_MAX_MODEL | "qwen3.7-max" | "qwen-3.7-max" => {
            Some(OPENROUTER_QWEN_3_7_MAX_MODEL)
        }
        OPENROUTER_QWEN_3_8_FLASH_MODEL | "qwen3.8-flash" | "qwen-3.8-flash" => {
            Some(OPENROUTER_QWEN_3_8_FLASH_MODEL)
        }
        OPENROUTER_TENCENT_HY3_PREVIEW_MODEL
        | "hy3-preview"
        | "tencent-hy3-preview"
        | "hy3"
        | "hunyuan"
        | "tencent-hunyuan"
        | "hunyuan-hy3" => Some(OPENROUTER_TENCENT_HY3_PREVIEW_MODEL),
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

pub(crate) fn opencode_go_chat_model_id(model: &str) -> Option<&'static str> {
    codewhale_config::opencode_go_chat_model_id(model)
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
        // Each alias resolves to its own constant, never through
        // `DEFAULT_ZAI_MODEL`: moving the default (now GLM-5.3) must not
        // silently re-point an explicit GLM-5.2 request.
        "glm-5.2" | "glm-5-2" | "zai-glm-5.2" | "zai-glm-5-2" => Some(ZAI_GLM_5_2_MODEL),
        "glm-5.3-flash" | "glm-5-3-flash" | "zai-glm-5.3-flash" | "zai-glm-5-3-flash" => {
            Some(ZAI_GLM_5_3_FLASH_MODEL)
        }
        "glm-5.3" | "glm-5-3" | "zai-glm-5.3" | "zai-glm-5-3" => Some(ZAI_GLM_5_3_MODEL),
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
/// Most provider-owned families (GLM via Z.ai/Zhipu, Kimi, Xiaomi MiMo,
/// MiniMax, Arcee, OpenRouter slugs, …) resolve through the same "apply the
/// family's canonical map, else pass the input through" path. OpenCode Go is
/// deliberately stricter because one provider roster spans two incompatible
/// wire protocols; only its Chat Completions rows may resolve here.
///
/// This is the canonicalization half of what [`normalize_model_name_for_provider`]
/// used to fuse together. Wire-id translation (e.g. `deepseek-v4-pro` → an
/// aggregator's `accounts/…/deepseek-v4-pro` slug) belongs to the route
/// resolver at request time, not to a name typed into `/provider`, so it is
/// deliberately kept out of here.
///
/// Returns `None` for empty or control-character input and for ids outside the
/// OpenCode Go Chat Completions allowlist. Other provider ids pass through so a
/// custom/self-hosted endpoint is never wrongly rejected.
#[must_use]
pub fn canonical_model_id_for_provider(provider: ApiProvider, model: &str) -> Option<String> {
    let trimmed = model.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return None;
    }

    // OpenCode Go is a strict protocol slice: its live `/models` response also
    // advertises Anthropic-Messages-only models, but this provider sends OpenAI
    // Chat Completions. Unknown and Messages-only ids must stop here rather
    // than falling through to the generic pass-through path below.
    if provider == ApiProvider::OpencodeGo {
        return opencode_go_chat_model_id(trimmed).map(str::to_string);
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
        ApiProvider::Minimax | ApiProvider::MinimaxAnthropic => canonical_minimax_model_id(trimmed),
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
        if let Some(canonical) = canonical_direct_deepseek_model_id(&normalized) {
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
    if provider == ApiProvider::OpencodeGo {
        // Canonicalize known Chat Completions ids only. Never substitute a
        // different model for an unknown/Messages-only id — that silently
        // changes the request. Keep the caller's spelling so validate_route /
        // the route resolver can reject it by name.
        return opencode_go_chat_model_id(trimmed)
            .map(str::to_string)
            .unwrap_or_else(|| trimmed.to_string());
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

/// Resolve the final request model while respecting custom endpoint
/// namespaces. Provider-only normalization cannot distinguish DeepSeek's
/// first-party API from a self-hosted OpenAI-compatible endpoint configured
/// under the legacy `deepseek` provider name, so actual HTTP clients use this
/// route-aware boundary.
#[must_use]
pub fn wire_model_for_provider_route(provider: ApiProvider, base_url: &str, model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return trimmed.to_string();
    }
    // OpenCode Go's provider identity is the Chat Completions protocol
    // boundary even when its base URL is overridden. Do not let the generic
    // custom-endpoint passthrough re-admit a Messages-only model.
    if provider == ApiProvider::OpencodeGo {
        return wire_model_for_provider(provider, trimmed);
    }
    if base_url_is_custom_for_provider(provider, base_url) {
        return trimmed.to_string();
    }
    wire_model_for_provider(provider, trimmed)
}

/// Reconcile a remembered `/model` pick with the model the config file names.
///
/// `provider_models` in `settings.toml` remembers the last `/model` (or model
/// picker) selection and outranks `config.toml` on the next launch. The picker
/// offers catalog spellings, which are lowercase, so a user whose config names
/// `DeepSeek-V4-Flash` can end up relaunching into `deepseek-v4-flash` — the
/// wrong id for a self-hosted OpenAI-compatible gateway whose model names are
/// case-sensitive, and the wrong id in the header.
///
/// When the two strings name the *same* model in a different ASCII case, the
/// config file owns the spelling. A remembered pick that names a genuinely
/// different model still wins, so `/model` persistence is unchanged: only the
/// spelling defers, never the selection.
#[must_use]
pub(crate) fn prefer_configured_model_spelling(configured: &str, remembered: String) -> String {
    let configured = configured.trim();
    if remembered != configured && remembered.eq_ignore_ascii_case(configured) {
        return configured.to_string();
    }
    remembered
}

/// Recover the behavioral intent of a retiring alias only when the selected
/// route is a first-party DeepSeek endpoint. Custom endpoints own both the id
/// and its semantics, so they deliberately return `None` here.
pub(crate) fn legacy_deepseek_alias_effort_for_route(
    provider: ApiProvider,
    base_url: &str,
    model: &str,
) -> Option<&'static str> {
    if !matches!(
        provider,
        ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic
    ) {
        return None;
    }
    let effort = legacy_deepseek_alias_reasoning_effort(model)?;
    (wire_model_for_provider_route(provider, base_url, model) != model.trim()).then_some(effort)
}

/// Hardcoded per-provider model id list used **only as a compatibility
/// fallback** (#4188).
///
/// Preferred sources are the live Models.dev catalog and the offline bundled
/// snapshot via [`crate::provider_lake`]. Call this directly only for
/// Codewhale-only / local providers Models.dev does not represent, or when
/// probing the fallback table in tests. Picker, inventory, and subagent
/// surfaces must go through the provider lake.
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
        ApiProvider::Orcarouter => {
            vec![DEFAULT_ORCAROUTER_MODEL, DEFAULT_ORCAROUTER_FLASH_MODEL]
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
        // Moonshot's direct platform API (the provider's default route) serves
        // `kimi-k3`; advertising only `kimi-k2.7-code` is half of why a
        // dogfood user reported "I can't find k3" on v0.9.1.
        //
        // The bare `k3` id and `kimi-for-coding` deliberately stay out: they
        // belong to the Kimi Code coding-plan endpoint
        // (api.kimi.com/coding/v1), which `validate_kimi_code_api_model_id`
        // enforces. A completion list is a per-provider fallback with no
        // base-URL context, so offering an id this route would reject would
        // just move the surprise later. Kimi Code routes surface their own
        // ids through the configured model and the route-aware picker rows.
        ApiProvider::Moonshot => vec![
            DEFAULT_MOONSHOT_MODEL,
            MOONSHOT_KIMI_K3_MODEL,
            MOONSHOT_KIMI_K2_6_MODEL,
        ],
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
        ApiProvider::Ollama | ApiProvider::OllamaCloud => Vec::new(),
        ApiProvider::Openai => OFFICIAL_OPENAI_MODELS.to_vec(),
        ApiProvider::Atlascloud => vec![DEFAULT_ATLASCLOUD_MODEL],
        ApiProvider::Together => vec![DEFAULT_TOGETHER_MODEL, DEFAULT_TOGETHER_FLASH_MODEL],
        ApiProvider::Qianfan => vec![DEFAULT_QIANFAN_MODEL],
        ApiProvider::OpenaiCodex => vec![DEFAULT_OPENAI_CODEX_MODEL],
        ApiProvider::Openmodel => vec![DEFAULT_OPENMODEL_MODEL],
        ApiProvider::Zai => vec![
            DEFAULT_ZAI_MODEL,
            ZAI_GLM_5_3_FLASH_MODEL,
            ZAI_GLM_5_2_MODEL,
            ZAI_GLM_5_1_MODEL,
            ZAI_GLM_5_TURBO_MODEL,
        ],
        ApiProvider::Stepfun => vec![DEFAULT_STEPFUN_MODEL],
        ApiProvider::Anthropic => vec![
            ANTHROPIC_OPUS_MODEL,
            DEFAULT_ANTHROPIC_MODEL,
            ANTHROPIC_HAIKU_MODEL,
        ],
        ApiProvider::Minimax | ApiProvider::MinimaxAnthropic => vec![
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
        ApiProvider::LongCat => vec![DEFAULT_LONGCAT_MODEL],
        ApiProvider::OpencodeGo => OPENCODE_GO_CHAT_MODELS.to_vec(),
        ApiProvider::OpencodeZen => codewhale_config::route::opencode_zen_picker_models(),
        ApiProvider::Meta => vec![
            DEFAULT_META_MODEL,
            "muse-spark-1.1",
            "muse-spark-1.2-contributor",
        ],
        ApiProvider::Xai => vec![
            DEFAULT_XAI_MODEL,
            XAI_GROK_4_5_MODEL,
            XAI_GROK_4_3_MODEL,
            XAI_GROK_BUILD_MODEL,
            XAI_GROK_COMPOSER_2_5_FAST_MODEL,
            XAI_GROK_4_20_0309_REASONING_MODEL,
            XAI_GROK_4_20_0309_NON_REASONING_MODEL,
        ],
        // Frozen pre-refresh gateway snapshot: these are the rows TelecomJS
        // TokenHub advertised when the provider landed (note the still-listed
        // `GLM-5.0`). It is only a conservative fallback — a configured key
        // replaces it wholesale with the authenticated live `/models` catalog
        // (docs/PROVIDERS.md, `telecomjs` row). Do not hand-add newer model
        // ids here; refresh the whole snapshot from the gateway instead.
        ApiProvider::Telecomjs => vec![
            DEFAULT_TELECOMJS_MODEL,
            "deepseek-v4-flash",
            "DeepSeek-R1",
            "qwen3.7-plus",
            "qwen3-max",
            "glm-5.2",
            "glm-5.1",
            "GLM-5.0",
            "Minimax-M2.5",
            "kimi-k2.7-code",
            "Doubao-Seed-2.0-Pro",
        ],
        ApiProvider::ModelstudioTokenPlan
        | ApiProvider::ModelstudioTokenPlanAnthropic
        | ApiProvider::ModelstudioCodingPlan
        | ApiProvider::ModelstudioCodingPlanAnthropic => vec![
            DEFAULT_MODELSTUDIO_TOKEN_PLAN_MODEL,
            "qwen3.8-max-preview",
            "qwen3.7-plus",
            "qwen3.7-max",
            "qwen3.6-flash",
            "deepseek-v4-pro",
            "deepseek-v4-flash-0731",
            // No glm-5.3: Model Studio publishes no such row (2026-08-03).
            "glm-5.2",
        ],
        ApiProvider::Mistral => vec![
            DEFAULT_MISTRAL_MODEL,
            "mistral-medium-latest",
            "mistral-small-latest",
            "mistral-large-latest",
        ],
        ApiProvider::Google => vec![
            DEFAULT_GOOGLE_MODEL,
            "gemini-3-pro-preview",
            "gemini-3.7-flash",
            "gemini-3.6-flash",
            "gemini-3.5-flash",
            "gemini-3.5-flash-lite",
            "gemini-2.5-pro",
            "gemini-2.5-flash",
        ],
        // The cloud-code wire protocol is not implemented; no model is
        // advertised for the credential-import-only route.
        ApiProvider::Antigravity => Vec::new(),
        ApiProvider::Edenai => vec![DEFAULT_EDENAI_MODEL],
        ApiProvider::Concentrate => vec![DEFAULT_CONCENTRATE_MODEL],
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

/// Deserialize `header_items` tolerantly: skip keys unknown to this build
/// instead of failing with an "unknown variant" error.
///
/// This keeps configuration files forward-compatible. For example, a newer
/// CodeWhale build may write a header item that an older build does not yet
/// understand; the older build will ignore that item while preserving the
/// remaining supported entries.
fn deser_header_items<'de, D>(deserializer: D) -> Result<Option<Vec<HeaderItem>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<Vec<String>> = Option::deserialize(deserializer)?;
    Ok(raw.map(|strings| {
        strings
            .into_iter()
            .filter_map(|s| {
                HeaderItem::from_key(&s).or_else(|| {
                    tracing::warn!("ignoring unknown header item {s:?} in config");
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
    /// Per-SSE-chunk idle timeout in seconds. Defaults to 900 seconds when
    /// omitted. `0` maps to the default; values clamp to `1..=3600`.
    pub stream_chunk_timeout_secs: Option<u64>,
    /// R1: ceiling on model steps in a single turn. Omitted or `0` resolve
    /// to the finite default (200); explicit values clamp to
    /// `1..=100_000`. There is deliberately no "unlimited" value — `0` is
    /// an invalid setting, not a sentinel that disables the cap.
    pub max_model_steps: Option<u32>,
    /// R1: cumulative wall-clock budget for a single turn, in seconds.
    /// Omitted or `0` resolve to the finite default (3600); explicit values
    /// clamp to `30..=86_400`. Time blocked on a human approval decision is
    /// excluded from the measurement.
    pub turn_wall_clock_secs: Option<u64>,
    /// R1: per-step cap on accumulated streamed content, in megabytes.
    /// Omitted or `0` resolve to the default (10 MB); explicit values clamp
    /// to `64 KiB..=512 MiB`.
    pub stream_max_content_mb: Option<u64>,
    /// R1: per-step cap on a single stream's wall-clock duration, in
    /// seconds. Omitted or `0` resolve to the default (1800); explicit
    /// values clamp to `10..=86_400`.
    pub stream_max_duration_secs: Option<u64>,
    /// Ordered list of footer items the user wants visible. `None` (the field
    /// missing from `config.toml`) means "use the built-in default order"; an
    /// empty `Some(vec![])` means "show nothing in the footer".
    ///
    /// Edited interactively via `/statusline`; persisted to `tui.status_items`
    /// in `~/.deepseek/config.toml`.
    #[serde(default, deserialize_with = "deser_status_items")]
    pub status_items: Option<Vec<StatusItem>>,
    /// Ordered list of optional header items the user wants visible.
    ///
    /// `None` (the field missing from `config.toml`) preserves the built-in
    /// header unchanged. An empty `Some(vec![])` likewise enables no additional
    /// header items, while configured entries enable their corresponding
    /// optional header content.
    ///
    /// The existing context-utilisation display remains part of the built-in
    /// header and is not controlled by this list.
    ///
    /// Unknown items are ignored during deserialization so configurations written
    /// by newer CodeWhale versions remain loadable by older versions.
    ///
    /// Persisted to `tui.header_items` in `~/.deepseek/config.toml`.
    #[serde(default, deserialize_with = "deser_header_items")]
    pub header_items: Option<Vec<HeaderItem>>,
    /// Emit OSC 8 hyperlink escape sequences around URLs in the transcript so
    /// supporting terminals (iTerm2, Terminal.app 13+, Ghostty, Kitty,
    /// WezTerm, Alacritty, recent gnome-terminal/konsole) make them clickable
    /// with the terminal's link gesture (usually Cmd-click on macOS and
    /// Ctrl-click on Linux/Windows). Terminals without OSC 8 support render the
    /// plain label and ignore the escape. Defaults to on for macOS/Linux and
    /// off for Windows legacy consoles; set `false` to suppress everywhere
    /// (e.g. for a terminal that misrenders the sequence). OSC 8 escapes are
    /// emitted out-of-band, so buffer-column corruption is not a concern.
    pub osc8_links: Option<bool>,
    /// High-level notification trigger condition. When set, controls whether
    /// operator notifications may interrupt the current terminal and, for
    /// `Always`, overrides the `[notifications].threshold_secs` gate from the
    /// lower-level `[notifications]` block:
    ///
    /// - `Always` — allow configured operator notifications even while the
    ///   terminal is focused. Successful turn completion also ignores the
    ///   duration threshold. Method, category, and quiet gates still apply.
    /// - `Unfocused` — notify only after the terminal has remained unfocused
    ///   for the built-in attention grace period. The normal duration
    ///   threshold still applies.
    /// - `Never` — suppress all operator notifications.
    /// - Unset (default) — behave like `Unfocused` and fall back to the
    ///   `[notifications]` duration threshold.
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
    /// Allow configured operator notifications in the foreground; completed
    /// turns have no duration threshold.
    Always,
    /// Notify only while the terminal is genuinely in the background.
    Unfocused,
    /// Suppress all operator notifications.
    Never,
}

/// Notification delivery method (mirrors `tui::notifications::Method`).
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
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

impl NotificationMethod {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "osc9" | "osc-9" | "osc_9" => Some(Self::Osc9),
            "bel" | "bell" => Some(Self::Bel),
            "kitty" => Some(Self::Kitty),
            "ghostty" => Some(Self::Ghostty),
            "off" | "none" | "disable" | "disabled" => Some(Self::Off),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Osc9 => "osc9",
            Self::Bel => "bel",
            Self::Kitty => "kitty",
            Self::Ghostty => "ghostty",
            Self::Off => "off",
        }
    }

    #[must_use]
    pub fn names_hint() -> &'static str {
        "auto, osc9, bel, kitty, ghostty, off"
    }
}

fn default_threshold_secs() -> u64 {
    30
}

/// Completion sound options.
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CompletionSound {
    /// No sound on turn completion.
    #[default]
    Off,
    /// System notification beep. On Windows uses `MessageBeep`.
    Beep,
    /// Terminal BEL character (`\x07`).
    Bell,
    /// Play a configured WAV sound file.
    File,
}

impl CompletionSound {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "disable" | "disabled" => Some(Self::Off),
            "beep" => Some(Self::Beep),
            "bell" | "bel" => Some(Self::Bell),
            "file" => Some(Self::File),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Beep => "beep",
            Self::Bell => "bell",
            Self::File => "file",
        }
    }

    #[must_use]
    pub fn names_hint() -> &'static str {
        "off, beep, bell, file"
    }
}

/// Controls when per-subagent completion notifications fire during fleet /
/// workflow runs. Turn-completion notifications are unaffected.
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SubagentCompletionNotification {
    /// Notify on every subagent completion.
    Always,
    /// Notify only when the last subagent in a batch finishes — no other
    /// subagents running and no workflow run in progress. Default: stays quiet
    /// mid-run and fires once when the fleet drains.
    #[default]
    FinalOnly,
    /// Never fire a subagent-completion notification.
    Off,
}

impl SubagentCompletionNotification {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "always" => Some(Self::Always),
            "final-only" | "finalonly" | "final" => Some(Self::FinalOnly),
            "off" | "none" | "never" | "disable" | "disabled" => Some(Self::Off),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::FinalOnly => "final-only",
            Self::Off => "off",
        }
    }

    #[must_use]
    pub fn names_hint() -> &'static str {
        "always, final-only, off"
    }
}

/// Operator notification configuration (native and terminal transports).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct NotificationsConfig {
    /// Delivery method: `auto` | `osc9` | `kitty` | `ghostty` | `bel` |
    /// `off`. Default: `auto`.
    /// `auto` resolves to OSC 9 for iTerm.app / Ghostty / WezTerm / Cmux
    /// (detected via `$TERM_PROGRAM` then `$LC_TERMINAL`) and the native macOS
    /// transport where appropriate; unknown terminals fail closed to `off`.
    /// Audible BEL is explicit only. On Windows explicit BEL is routed through
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

    /// When to fire per-subagent completion notifications during fleet /
    /// workflow runs: `always` | `final-only` | `off`. Default: `final-only`
    /// (quiet mid-run, one notification when the batch drains). Set `off` to
    /// silence subagent notifications entirely.
    #[serde(default)]
    pub subagent_completion: SubagentCompletionNotification,

    /// Completion sound: `"off"` | `"beep"` | `"bell"` | `"file"`. Default: `"off"`.
    /// This is opt-in and follows the same foreground/quiet attention policy
    /// as desktop notifications.
    #[serde(default)]
    pub completion_sound: CompletionSound,

    /// Path to the WAV sound file used when `completion_sound = "file"`.
    #[serde(default)]
    pub sound_file: Option<PathBuf>,

    /// Opt-in per-event sound policy (`[notifications.event_sound]`).
    /// Disabled by default; see `tui::sound_policy` for the decision rules.
    #[serde(default)]
    pub event_sound: EventSoundConfig,

    /// Quiet mode: suppress every desktop notification (all categories, all
    /// delivery methods) and the paired `[notifications.event_sound]` cues,
    /// without editing `method`, `completion_sound`, or the per-category
    /// switches under `[notifications.events]`. Default: `false`.
    #[serde(default)]
    pub quiet: bool,

    /// Per-category desktop-notification switches
    /// (`[notifications.events]`). Every category defaults to enabled; set
    /// one to `false` to silence that event kind without touching the rest.
    #[serde(default)]
    pub events: NotificationEventsConfig,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            method: NotificationMethod::default(),
            threshold_secs: default_threshold_secs(),
            include_summary: false,
            subagent_completion: SubagentCompletionNotification::default(),
            completion_sound: CompletionSound::default(),
            sound_file: None,
            event_sound: EventSoundConfig::default(),
            quiet: false,
            events: NotificationEventsConfig::default(),
        }
    }
}

/// One live `[notifications]` scalar edit.
///
/// Keeping edits as deltas prevents a later session-only command from
/// replacing earlier live changes with a freshly loaded disk snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationConfigUpdate {
    Method(NotificationMethod),
    ThresholdSecs(u64),
    IncludeSummary(bool),
    Quiet(bool),
    CompletionSound(CompletionSound),
    SubagentCompletion(SubagentCompletionNotification),
}

impl NotificationsConfig {
    pub fn apply_update(&mut self, update: NotificationConfigUpdate) {
        match update {
            NotificationConfigUpdate::Method(value) => self.method = value,
            NotificationConfigUpdate::ThresholdSecs(value) => self.threshold_secs = value,
            NotificationConfigUpdate::IncludeSummary(value) => self.include_summary = value,
            NotificationConfigUpdate::Quiet(value) => self.quiet = value,
            NotificationConfigUpdate::CompletionSound(value) => self.completion_sound = value,
            NotificationConfigUpdate::SubagentCompletion(value) => {
                self.subagent_completion = value;
            }
        }
    }
}

fn default_notification_event_enabled() -> bool {
    true
}

/// Per-category desktop-notification switches (`[notifications.events]`).
///
/// Categories mirror the closed set of notification kinds in
/// `tui::notification_payload::NotificationKind`. Each defaults to `true`;
/// a disabled category is suppressed across every delivery mechanism
/// (OSC 9, Kitty OSC 99, Ghostty OSC 777, BEL, macOS Notification Center).
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct NotificationEventsConfig {
    /// An agent turn finished successfully. Default: `true`.
    #[serde(default = "default_notification_event_enabled")]
    pub turn_complete: bool,
    /// A sub-agent reached a terminal status. Default: `true`.
    #[serde(default = "default_notification_event_enabled")]
    pub subagent_terminal: bool,
    /// A tool call is blocked waiting for approval. Default: `true`.
    #[serde(default = "default_notification_event_enabled")]
    pub approval_needed: bool,
    /// The agent asked a question and is blocked on the answer.
    /// Default: `true`.
    #[serde(default = "default_notification_event_enabled")]
    pub input_needed: bool,
    /// The sandbox denied an operation and the user must decide.
    /// Default: `true`.
    #[serde(default = "default_notification_event_enabled")]
    pub elevation_needed: bool,
    /// The model called the `notify` tool. Default: `true`.
    #[serde(default = "default_notification_event_enabled")]
    pub model_notify: bool,
}

impl Default for NotificationEventsConfig {
    fn default() -> Self {
        Self {
            turn_complete: true,
            subagent_terminal: true,
            approval_needed: true,
            input_needed: true,
            elevation_needed: true,
            model_notify: true,
        }
    }
}

fn default_event_sound_events() -> Vec<String> {
    vec!["turn-complete".to_string(), "approval-needed".to_string()]
}

fn default_event_sound_min_interval_ms() -> u64 {
    2000
}

/// Opt-in, deterministic per-event sound policy (#4817). Terminal-bell
/// level only: cues are BEL (`\x07`) bytes, a platform-safe no-op on
/// terminals that ignore them. Off by default.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct EventSoundConfig {
    /// Master switch. Default: `false` (nothing is emitted unless opted in).
    #[serde(default)]
    pub enabled: bool,
    /// Allow-list of event names, kebab-case (`"turn-complete"`,
    /// `"subagent-terminal"`, `"approval-needed"`, `"input-needed"`,
    /// `"elevation-needed"`, `"model-notify"`). Unknown names are ignored.
    /// Default: `["turn-complete", "approval-needed"]`.
    #[serde(default = "default_event_sound_events")]
    pub events: Vec<String>,
    /// Minimum milliseconds between two plays of the same event. Default: 2000.
    #[serde(default = "default_event_sound_min_interval_ms")]
    pub min_interval_ms: u64,
    /// Quiet mode: suppress all event sounds without editing the allow-list.
    /// Default: `false`.
    #[serde(default)]
    pub quiet: bool,
}

impl Default for EventSoundConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            events: default_event_sound_events(),
            min_interval_ms: default_event_sound_min_interval_ms(),
            quiet: false,
        }
    }
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryBackend {
    Native,
    #[default]
    Off,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MemoryConfig {
    /// When `true`, load the user memory file at `Config::memory_path()`
    /// into the system prompt as a `<user_memory>` block, and intercept
    /// `# foo` typed in the composer to append to that file. Default `false`.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Explicit backend selection for the v0.9.2 memory lifecycle.
    /// `None` preserves the pre-native opt-in behavior for old configs.
    #[serde(default)]
    pub backend: Option<MemoryBackend>,
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

/// Persistent-goal loop controls (`[goal]` table in config.toml, #5052).
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
pub struct GoalConfig {
    /// Optional safety backstop on automatic goal continuation passes.
    /// Goals are unlimited by default; token/time budgets are telemetry only.
    ///
    /// `None` uses the built-in default
    /// ([`crate::goal_loop::DEFAULT_MAX_GOAL_CONTINUATIONS`], currently `0`);
    /// `0` disables the backstop entirely so only terminal status or user
    /// control ends the run.
    #[serde(default)]
    pub max_continuations: Option<u32>,
    /// Optional quiet period between successful cross-turn continuations.
    /// `0` preserves immediate continuation. Positive values make long-lived
    /// coordinator goals yield visibly between turns instead of sleeping
    /// inside a provider turn.
    #[serde(default)]
    pub continuation_delay_seconds: Option<u64>,
}

/// Reasoning-only recovery controls (`[reasoning_only]` table in config.toml).
/// When the model returns only hidden reasoning (thinking) without any answer
/// text or tool call, the engine can re-request the answer automatically.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct ReasoningOnlyConfig {
    /// Maximum number of automatic re-requests when the model returns only
    /// reasoning without any answer or tool call.
    /// Defaults to 2. Set to 0 to disable automatic recovery.
    pub max_reprompts: Option<u32>,
    /// Optional custom message sent to the model on each re-request.
    /// When set, the engine inserts this as a user message before re-issuing
    /// the request, nudging the model to produce an actual answer.
    /// When unset (default), the engine uses the built-in default message
    /// ("So, what's up ? Keep running !").
    pub reprompt_message: Option<String>,
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
    /// "act" / "plan" / "operate" chip.
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
    /// Prepaid remaining credit, refreshed once per turn completion.
    Balance,
    /// Session metrics strip: turns · steps │ LLM · tools │ TTFT · tok/s │
    /// cache │ in — sourced from engine timings and provider usage.
    SessionMetrics,
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
            StatusItem::SessionMetrics,
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
            StatusItem::SessionMetrics => "session_metrics",
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
            "session_metrics" => Some(Self::SessionMetrics),
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
            StatusItem::SessionMetrics => "Session metrics",
        }
    }

    /// One-line hint shown beside the label so the user knows what each item
    /// surfaces without having to toggle it on first.
    #[must_use]
    pub fn hint(self) -> &'static str {
        match self {
            StatusItem::Mode => "plan · act · operate",
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
            StatusItem::Balance => "remaining prepaid credit from the active provider",
            StatusItem::SessionMetrics => "turns · steps · LLM/tool time · TTFT · tok/s · input",
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
            StatusItem::SessionMetrics,
        ]
    }

    /// Whether this item is relevant for `provider`.  Provider-specific
    /// items return `false` for unsupported providers so the picker doesn't
    /// offer toggles that can never show useful data.
    #[must_use]
    pub fn is_available_for(self, provider: ApiProvider) -> bool {
        match self {
            StatusItem::Balance => provider_has_balance_api(provider),
            _ => true,
        }
    }
}

/// Prepaid providers that publish a remaining-credit endpoint Codewhale
/// can fetch. Local runtimes and invoice-only vendors stay out.
#[must_use]
pub fn provider_has_balance_api(provider: ApiProvider) -> bool {
    matches!(
        provider,
        ApiProvider::Deepseek
            | ApiProvider::DeepseekCN
            | ApiProvider::Openrouter
            | ApiProvider::Siliconflow
            | ApiProvider::SiliconflowCn
    )
}

/// One configurable header item

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HeaderItem {
    /// Session token usage: input / cache-hit / output.
    Tokens,
}

impl HeaderItem {
    /// Default header composition for the always-on status line. Used when
    /// `tui.header_items` is missing from `config.toml` so upgraders see a
    /// concise header by default; diagnostic chips remain available through
    /// explicit configuration without crowding the main UI.
    #[must_use]
    pub fn default_header() -> Vec<HeaderItem> {
        Vec::new()
    }

    /// Stable canonical name used in TOML.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            HeaderItem::Tokens => "tokens",
        }
    }

    /// Parse a config string while ignoring unknown items.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "tokens" => Some(Self::Tokens),
            _ => None,
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

/// Context management configuration.
///
/// The append-only "Flash seam" layered-context system (#159) was removed on
/// 2026-07-23 — it never left its opt-in default and compaction owns context
/// reduction now. Its keys remain parsed-but-ignored so existing config files
/// keep loading; `project_pack` is the only live setting.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ContextConfig {
    /// Ignored (was: master enable for the removed layered-context system).
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Include a deterministic project context pack in the stable prompt
    /// prefix. Default: false — the pack is a large pretty-printed directory
    /// listing the model can rebuild with one `File` call (#4781). Set
    /// `[context] project_pack = true` to opt in (useful for weak tool-calling
    /// models).
    #[serde(default)]
    pub project_pack: Option<bool>,
    /// Ignored (was: seam verbatim window).
    #[serde(default)]
    pub verbatim_window_turns: Option<usize>,
    /// Ignored (was: seam thresholds).
    #[serde(default)]
    pub l1_threshold: Option<usize>,
    #[serde(default)]
    pub l2_threshold: Option<usize>,
    #[serde(default)]
    pub l3_threshold: Option<usize>,
    /// Ignored (was: seam model).
    #[serde(default)]
    pub seam_model: Option<String>,
}

/// Fleet-role model overrides for delegated workers. Canonical keys in
/// `models` are `worker`, `scout`, `planner`, `reviewer`, `builder`,
/// `verifier`, and `custom`. Legacy sub-agent type names remain accepted for
/// v0.9.x compatibility. Per-call explicit model choices still win.
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
    #[serde(default, rename = "scout_model", alias = "explorer_model")]
    pub explorer_model: Option<String>,
    #[serde(default, rename = "planner_model", alias = "awaiter_model")]
    pub awaiter_model: Option<String>,
    #[serde(default, rename = "reviewer_model", alias = "review_model")]
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
    /// Defaults to `DEFAULT_SUBAGENT_API_TIMEOUT_SECS` (600) and is clamped
    /// to `MIN_SUBAGENT_API_TIMEOUT_SECS..=MAX_SUBAGENT_API_TIMEOUT_SECS`
    /// (1..=3600). Zero or unset uses the 600s default (#1806, #1808).
    #[serde(default)]
    pub api_timeout_secs: Option<u64>,
    /// Wall-clock timeout for a running sub-agent that stops making
    /// manager-visible progress. Defaults to 5 minutes and is kept above the
    /// per-step API timeout so slow but legitimate model calls are not
    /// cancelled before their request timeout can fire (#2614).
    #[serde(default)]
    pub heartbeat_timeout_secs: Option<u64>,
    /// Default per-child model-turn budget applied when an `agent` start
    /// carries no explicit `max_steps` (#5324). Unset or zero remains
    /// unbounded for every Fleet role; positive values are clamped to the
    /// runtime ceiling (2000) at resolution.
    #[serde(default)]
    pub default_max_steps: Option<u32>,
    /// Default per-child wall-clock budget in seconds applied when an
    /// `agent` start carries no explicit `wall_time_secs` (#5324). When
    /// unset, children get 1800s; values are clamped to 1..=86400 at
    /// resolution.
    #[serde(default)]
    pub default_wall_time_secs: Option<u64>,
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
/// `cost_saving` (#1207): when `true`, the auto-mode router prefers the
/// active provider's known fast sibling for ambiguous requests, only using
/// its strong tier when the task clearly benefits from deeper reasoning.
/// Providers without a validated sibling stay on the active model. Default
/// is `false` (balanced — match the existing routing voice).
///
/// `cross_provider` (#4411): Auto routing is scoped to the active provider
/// unless this persisted opt-in is set to `true`. Without it, neither the
/// classifier inventory nor the local heuristic may leave the provider the
/// session is actually configured to use.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AutoConfig {
    #[serde(default)]
    pub cost_saving: Option<bool>,
    /// Persisted opt-in for cross-provider Auto routing (`[auto]
    /// cross_provider = true`). Default `false`: active provider only.
    #[serde(default)]
    pub cross_provider: Option<bool>,
    /// Optional explicit auto-router classifier route (`[auto.router]`).
    #[serde(default)]
    pub router: Option<AutoRouterConfig>,
}

/// Default classifier call timeout for `[auto.router]` (seconds).
pub(crate) const DEFAULT_AUTO_ROUTER_TIMEOUT_SECS: u64 = 4;
/// Upper clamp for a configured classifier timeout: a hung local router must
/// not stall an Auto turn forever.
pub(crate) const MAX_AUTO_ROUTER_TIMEOUT_SECS: u64 = 300;

/// Explicit classifier route for Auto model mode (`[auto.router]`).
///
/// When `provider` + `model` are set, Auto mode's classifier call goes to that
/// route. When unset, Auto stays local and free: it uses the heuristic and
/// makes no classifier call at all.
///
/// There is deliberately no implicit default. Holding a DeepSeek key used to
/// elect `deepseek-v4-flash` as the classifier for every Auto turn, which spent
/// a user's tokens on a route they never chose and privileged one provider over
/// the rest. Electing a network classifier is now something the operator writes
/// down.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AutoRouterConfig {
    /// Provider id for the classifier route (e.g. `"deepseek"`, `"zai"`).
    #[serde(default)]
    pub provider: Option<String>,
    /// Model id on that provider (e.g. `"deepseek-v4-flash"`).
    #[serde(default)]
    pub model: Option<String>,
    /// Thinking tier for the classifier call (e.g. `"off"`). Defaults to off.
    #[serde(default)]
    pub thinking: Option<String>,
    /// Classifier call timeout in seconds. Defaults to
    /// [`DEFAULT_AUTO_ROUTER_TIMEOUT_SECS`] (4); `0` means "use the default".
    /// Values above [`MAX_AUTO_ROUTER_TIMEOUT_SECS`] (300) are clamped so a
    /// hung local router cannot stall a turn indefinitely.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

fn default_update_check_for_updates() -> bool {
    true
}

fn default_update_check_interval_hours() -> u64 {
    codewhale_release::check::DEFAULT_CHECK_INTERVAL_HOURS
}

/// Startup update-check configuration (`[update]` table in config.toml).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct UpdateConfig {
    /// When false, skip the TUI startup background update check entirely.
    #[serde(default = "default_update_check_for_updates")]
    pub check_for_updates: bool,
    /// Hours between network checks. The answer is cached on disk in between,
    /// so the notice still appears on every launch — only the request is
    /// throttled. `0` disables caching and checks on every launch.
    #[serde(default = "default_update_check_interval_hours")]
    pub check_interval_hours: u64,
    /// Optional GitHub-compatible latest-release JSON endpoint.
    #[serde(default)]
    pub update_uri: Option<String>,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            check_for_updates: true,
            check_interval_hours: default_update_check_interval_hours(),
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

/// Which approval option a freshly rendered approval card highlights.
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDefaultSelection {
    /// Highlight the deny option, so a reflexive Enter refuses the call.
    #[default]
    Deny,
    /// Highlight "allow once", restoring the pre-v0.9.6 Enter-to-approve flow.
    AllowOnce,
}

/// Approval-card presentation (`[approval]` table in config.toml). Approval
/// *policy* stays the top-level `approval_policy` key; this table only governs
/// how the card is presented once a prompt is already required.
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
pub struct ApprovalConfig {
    /// Option highlighted when an approval card first appears (#5293).
    /// Default: `deny`.
    #[serde(default)]
    pub default_selection: ApprovalDefaultSelection,
}

/// `transcript.prose_measure` exactly as written in `config.toml`.
///
/// Parsed permissively (any scalar shape) so an invalid value can surface as
/// a targeted `transcript.prose_measure` config error via [`Config::validate`]
/// instead of a generic whole-file parse failure that names no key.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum RawProseMeasure {
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Text(String),
}

impl RawProseMeasure {
    /// Render the raw file value for config diagnostics.
    fn describe(&self) -> String {
        match self {
            Self::Integer(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Boolean(value) => value.to_string(),
            Self::Text(value) => format!("'{value}'"),
        }
    }
}

/// Transcript rendering controls (`[transcript]` table in config.toml).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptConfig {
    /// Wrap cap, in terminal columns, for prose cells — user messages,
    /// assistant answers, and reasoning/thinking blocks — in the live
    /// transcript (#5436). Absent or `0` spends the full content width,
    /// matching tool/status cells and the #5322 wide-frame decision. A
    /// positive integer caps prose at that many columns for owners who
    /// want a bounded reading measure on ultrawide displays. Tool, diff,
    /// and status cells never inherit this cap.
    #[serde(default)]
    pub(crate) prose_measure: Option<RawProseMeasure>,
}

impl TranscriptConfig {
    /// Resolve the raw file value into a prose wrap cap.
    ///
    /// `Ok(None)` means full content width. Errors describe the raw value so
    /// [`Config::validate`] can name the offending key and setting.
    fn prose_measure_columns(&self) -> Result<Option<u16>, String> {
        match &self.prose_measure {
            None | Some(RawProseMeasure::Integer(0)) => Ok(None),
            Some(RawProseMeasure::Integer(columns)) if *columns > 0 => {
                Ok(Some((*columns).min(i64::from(u16::MAX)) as u16))
            }
            Some(raw) => Err(format!(
                "expected a positive whole number of columns \
                 (0 or absent = full width), got {}",
                raw.describe()
            )),
        }
    }
}

/// Resolved CLI configuration, including defaults and environment overrides.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    /// Single-token inputs that cancel the active turn before dispatch.
    #[serde(default)]
    pub stop_words: Option<Vec<String>>,
    pub provider: Option<String>,
    #[serde(alias = "apiKey")]
    pub api_key: Option<String>,
    #[serde(alias = "baseUrl")]
    pub base_url: Option<String>,
    /// Optional extra HTTP headers sent to model API requests.
    #[serde(alias = "httpHeaders")]
    pub http_headers: Option<HashMap<String, String>>,
    /// Optional user-facing tab/window title shown as `[title] …` in front of
    /// the terminal window title (the `Codewhale` / `reasoning…` / `done.`
    /// states). This is the default for every session in this config scope;
    /// the `/title` command overrides it per session, and `/config title …
    /// --save` persists a new default here. Multi-window setups can point each
    /// workspace at its own `--config` file (or profile) so alt-tabbed
    /// sessions are identifiable at a glance.
    pub title: Option<String>,
    #[serde(alias = "defaultTextModel")]
    pub default_text_model: Option<String>,
    #[serde(alias = "authMode")]
    pub auth_mode: Option<String>,
    /// DeepSeek reasoning-effort tier: `"off" | "low" | "medium" | "high" | "max"`.
    /// Defaults to `"max"` at runtime if unset.
    pub reasoning_effort: Option<String>,
    /// True only when compatibility migration inferred `reasoning_effort`
    /// from a retiring DeepSeek alias. This distinguishes that inferred value
    /// from a user override during an in-session provider switch.
    #[serde(skip)]
    pub(crate) reasoning_effort_inferred_from_legacy_alias: bool,
    /// Runtime-only receipt that a fresh launch adopted the selected Fleet's
    /// operator provider/model pair. App initialization uses it to prevent
    /// generic remembered `/model` preferences from replacing that selected
    /// Fleet route later in the same launch.
    #[serde(skip)]
    pub(crate) fleet_operator_route_applied: bool,
    /// Runtime-only receipt that the selected Fleet also supplied a reasoning
    /// tier. Kept separate because an operator with no tier deliberately
    /// inherits the ordinary session/settings reasoning preference.
    #[serde(skip)]
    pub(crate) fleet_operator_reasoning_applied: bool,
    /// Original first-party DeepSeek alias captured before model normalization.
    /// This runtime-only receipt lets diagnostics explain why the resolved
    /// model changed without persisting compatibility state back to config.
    #[serde(skip)]
    pub(crate) migrated_deepseek_model_alias: Option<String>,
    /// Runtime-only receipt that the released `ollama` + exact
    /// `https://ollama.com/v1` tuple was upgraded to `ollama-cloud` in memory.
    ///
    /// This survives route-scoped config clones so old provider-table and
    /// secret-slot reads remain available to that exact migrated route. It is
    /// never serialized and is never set for an explicit `ollama-cloud`
    /// selection.
    #[serde(skip)]
    pub(crate) migrated_legacy_ollama_cloud_route: bool,
    /// Runtime-only isolation boundary for account-owned managed Chat.
    ///
    /// This is never user-configurable or serialized. The Runtime Chat relay
    /// sets it on its private config clone so the Engine suppresses all host
    /// workspace metadata and constructs no model-visible MCP, sub-agent, or
    /// native tool surface for those turns.
    #[serde(skip)]
    pub(crate) runtime_chat_isolated: bool,
    /// Runtime-only marker for an independent RuntimeThreadManager store.
    ///
    /// Those threads are not projected into the interactive TUI's attached
    /// CWC run, so their provider requests do not participate in that run's
    /// exclusive Chat ownership gate. RuntimeThreadManager sets this marker on
    /// its private config clone; it is never user-configurable or serialized.
    #[serde(skip)]
    pub(crate) runtime_thread_inference_unrelated: bool,
    /// Native tool catalog controls. This table controls built-in
    /// tool loading policy.
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
    /// Whether a workspace-write sandbox also grants the shell outbound
    /// network access. Defaults to `false`: editing the workspace is not a
    /// reason to be able to reach the internet. Network comes from an explicit
    /// opt-in here, from a `danger-full-access` posture, or from the
    /// post-denial elevation prompt. `yolo`/`Bypass` is unaffected — it
    /// resolves to `danger-full-access`, which is unsandboxed by definition.
    #[serde(alias = "sandboxNetworkAccess")]
    pub sandbox_network_access: Option<bool>,
    /// Foreign-agent instruction formats to import as project instructions.
    /// Empty by default: a `CLAUDE.md`, `.cursorrules`, or
    /// `.github/copilot-instructions.md` written as law for another tool is
    /// not silently treated as law for this one. Accepts `claude`, `cursor`,
    /// `cline`, `windsurf`, `gemini`, `copilot`, `muse`, or `all`.
    #[serde(default, alias = "projectInstructionImports")]
    pub project_instruction_imports: Vec<String>,
    /// `telemetry` as written to the config file, before environment and
    /// default resolution. Kept so doctor and config displays can state the
    /// *resolved* consent with its source (default | env | config) instead of
    /// reading "unset" while batches ship (#5441).
    #[serde(default)]
    pub telemetry: Option<bool>,
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
    /// When true and `/usr/bin/bwrap` is executable on Linux, route exec_shell
    /// through bubblewrap (#2184).
    /// Defaults to false. Requires the `bubblewrap` package to be installed
    /// separately — we do NOT vendor bwrap.
    #[serde(alias = "preferBwrap")]
    pub prefer_bwrap: Option<bool>,
    /// Additional host paths to bind read-only inside the bubblewrap sandbox
    /// (Linux, `prefer_bwrap = true`, #5410). The default root bind already
    /// exposes the host filesystem read-only; these cover setups where a
    /// policy or future default narrows it. Non-existent paths are skipped.
    #[serde(default, alias = "bwrapRoRoots")]
    pub bwrap_ro_roots: Vec<std::path::PathBuf>,
    /// Host device nodes to bind read-write inside the bubblewrap sandbox
    /// (#5410), e.g. `/dev/null` for shell redirection against the host node.
    /// Only character/block devices are honored — never directories — so
    /// this key cannot become a writable-root escape hatch. Non-existent or
    /// non-device paths are skipped. The default private `/dev` already
    /// provides fresh device nodes, so most users never need this.
    #[serde(default, alias = "bwrapDevRoots")]
    pub bwrap_dev_roots: Vec<std::path::PathBuf>,
    /// Opt-in sandbox read deny-list (S1, #5568). Listed subpaths are
    /// unreadable inside sandboxed shell commands even though the sandbox
    /// otherwise grants full-disk read: Seatbelt appends last-match-wins
    /// deny rules; bubblewrap masks each existing path with an empty tmpfs
    /// (directories) or a /dev/null bind (files). A leading `~` expands to
    /// the user's home. Empty (the default) preserves current behavior.
    /// Example: `sandbox_denied_read_paths = ["~/.ssh", "~/.aws"]`.
    #[serde(default, alias = "sandboxDeniedReadPaths")]
    pub sandbox_denied_read_paths: Vec<std::path::PathBuf>,
    /// Apply the built-in credential-store read deny-list (S1). Defaults to
    /// `true`: `~/.ssh`, cloud credential dirs, keychains, browser profiles,
    /// `.env` files, and Codewhale's own secret stores are unreadable by the
    /// file-reading tools and by sandboxed shell commands. Set to `false` to
    /// restore the pre-S1 full-disk-read behavior. See
    /// `sandbox::read_guard` for the exact list and its honest limits — it is
    /// defense-in-depth, not a security boundary.
    #[serde(default, alias = "sandboxReadDenylistDefaults")]
    pub sandbox_read_denylist_defaults: Option<bool>,
    /// Subtract paths from the *built-in* deny-list defaults when a project
    /// genuinely needs one of them. Has no effect on
    /// `sandbox_denied_read_paths`: an explicit deny always wins.
    #[serde(default, alias = "sandboxReadDenylistExempt")]
    pub sandbox_read_denylist_exempt: Vec<std::path::PathBuf>,
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

    /// Transcript rendering controls (`[transcript]` table). Absent means
    /// prose uses the full content width (#5436).
    #[serde(default)]
    pub transcript: Option<TranscriptConfig>,

    /// Lifecycle hooks configuration
    #[serde(default)]
    pub hooks: Option<HooksConfig>,

    /// Lifecycle event outbox (`[lifecycle_outbox]`). Opt-in: an unset or
    /// empty `path` disables the feature and leaves behavior unchanged.
    /// Fires for interactive TUI sessions and headless `codewhale exec` runs.
    #[serde(default)]
    pub lifecycle_outbox: Option<codewhale_config::LifecycleOutboxToml>,

    /// Per-session control socket (`[control_socket]`). Opt-in: an absent
    /// table or `enabled = false` (the default) leaves the feature off.
    /// When enabled, the interactive TUI binds a unix socket per running
    /// session (see `crate::tui::control_socket`).
    #[serde(default)]
    pub control_socket: Option<codewhale_config::ControlSocketToml>,

    /// Provider-specific credentials and defaults shared with the `codewhale` facade.
    #[serde(default)]
    pub providers: Option<ProvidersConfig>,

    /// Desktop notification settings (OSC 9 / BEL on long turn completion).
    #[serde(default)]
    pub notifications: Option<NotificationsConfig>,

    /// Approval-card presentation (`[approval]`). Absent means deny-by-default
    /// preselection.
    #[serde(default)]
    pub approval: Option<ApprovalConfig>,

    /// Per-domain network policy (#135). When absent, network tools fall back
    /// to a permissive default that mirrors pre-v0.7.0 behavior.
    #[serde(default)]
    pub network: Option<NetworkPolicyToml>,

    /// Verifier-preview behavior (#2093). When absent, automatic verifier
    /// preview stays off and verifier verdicts use the hunt policy.
    #[serde(default)]
    pub verifier: Option<codewhale_config::VerifierConfigToml>,

    /// Background advisor watcher (#3982). When absent, the advisor is off
    /// by default. Enable with `[advisor] enabled = true` or `/advisor on`.
    #[serde(default)]
    pub advisor: Option<codewhale_config::AdvisorConfigToml>,

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

    /// Web search provider configuration. When absent, defaults to keyless
    /// Firecrawl. Other API services require credentials; SearXNG requires a
    /// trusted `base_url`.
    #[serde(default)]
    pub search: Option<SearchConfig>,

    /// Persistent-goal loop controls (#5052). When absent, goals have no
    /// continuation ceiling. Users can opt into one with
    /// `[goal] max_continuations`.
    #[serde(default)]
    pub goal: Option<GoalConfig>,

    /// Reasoning-only recovery controls. When absent, the engine uses the
    /// built-in default (2 retries, no custom message). Configure with
    /// `[reasoning_only] max_reprompts` and/or `[reasoning_only] reprompt_message`.
    #[serde(default)]
    pub reasoning_only: Option<ReasoningOnlyConfig>,

    /// User-level memory (#489). Default behaviour is **opt-in**:
    /// loading + injection happens only when `[memory] enabled = true` or
    /// `DEEPSEEK_MEMORY=on` is set. The surviving store is the native
    /// Markdown + SQLite FTS5 system (`memory/global/MEMORY.md`).
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

    /// Context configuration (project context pack; legacy seam keys are
    /// parsed but ignored since the 2026-07-23 removal).
    #[serde(default)]
    pub context: ContextConfig,

    /// Agent Fleet trust/security/role/exec config.
    #[serde(default)]
    pub fleet: Option<codewhale_config::FleetConfigToml>,

    /// Workflow automatic-launch, approval, isolation, and activity
    /// persistence knobs (#4128). When absent, consumers use
    /// [`codewhale_config::WorkflowConfigToml::default`] via
    /// [`Self::workflow_config`].
    #[serde(default)]
    pub workflow: Option<codewhale_config::WorkflowConfigToml>,

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

    /// Receipt describing what the environment layer did to this config's
    /// effective base URL.
    ///
    /// This provenance cannot be reconstructed from the merged provider table:
    /// environment overrides are written into the same `base_url` field as
    /// file-owned routes. Keep the receipt so a saved provider/root key (or a
    /// configured `api_key_env`) cannot silently follow an env-selected custom
    /// host, and so a cross-provider child cannot borrow an ambient generic
    /// host that was never addressed to it.
    #[serde(skip)]
    pub(crate) base_url_env_receipt: BaseUrlEnvReceipt,

    /// Who owns the legacy root `base_url` field.
    ///
    /// `Deepseek` and `DeepseekCN` are two identities that share one legacy
    /// root field, so the field alone cannot say whether it is a user's
    /// file-owned endpoint (shared by both, as it always has been) or a
    /// `CODEWHALE_BASE_URL`/`DEEPSEEK_BASE_URL` value that
    /// [`apply_env_overrides`] addressed to exactly one of them.
    ///
    /// [`BaseUrlEnvReceipt::Unrecorded`] is the file-owned case and keeps the
    /// legacy shared behavior.
    #[serde(skip)]
    pub(crate) root_base_url_owner: BaseUrlEnvReceipt,

    /// Mini-window (pinned, always-on-top) mode layout preferences
    /// (`[mini_window]` in config.toml). When the host terminal window is
    /// pinned into its small always-on-top form, the TUI switches to a
    /// compact layout that keeps only the elements listed here.
    #[serde(default)]
    pub mini_window: Option<MiniWindowConfig>,
}

/// Layout preferences for the pinned (always-on-top) mini-window mode.
///
/// When the host window is shrunk into its mini form (the right-click
/// "弹出置顶小窗" action), the TUI hides the shell chrome and keeps only the
/// message stream plus whatever the user opted to keep here.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MiniWindowConfig {
    /// Keep the composer (message input box) visible in mini mode.
    /// Default: true — the mini window stays interactive.
    #[serde(default = "mini_default_keep_input")]
    pub keep_input: bool,
    /// Keep the Tasks + To-do strip visible in mini mode. Default: false.
    #[serde(default)]
    pub keep_todo: bool,
    /// Keep the side work rail (work surface side panel) visible in mini
    /// mode. Default: false.
    #[serde(default)]
    pub keep_sidebar: bool,
    /// Keep the bottom phase strip visible in mini mode. Default: false.
    #[serde(default)]
    pub keep_footer: bool,
    /// Keep the top status bar (route/mode/effort/permission header) visible
    /// in mini mode. Default: false.
    #[serde(default)]
    pub keep_header: bool,
}

impl Default for MiniWindowConfig {
    fn default() -> Self {
        Self {
            keep_input: true,
            keep_todo: false,
            keep_sidebar: false,
            keep_footer: false,
            keep_header: false,
        }
    }
}

fn mini_default_keep_input() -> bool {
    true
}

/// What the environment layer decided about the generic
/// `CODEWHALE_BASE_URL` / `DEEPSEEK_BASE_URL` override.
///
/// The distinction that matters is between "no receipt" and "a receipt saying
/// nobody owns it". They are not the same state and must not collapse: a
/// missing receipt is a config that never passed through the environment
/// layer, while [`BaseUrlEnvReceipt::NoOwner`] is a positive statement that a
/// higher-precedence layer took the endpoint away from the environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum BaseUrlEnvReceipt {
    /// The environment layer never ran for this config — directly constructed
    /// configs, embedded profiles, and unit-test fixtures. These keep the
    /// established global fallback: the generic override applies to whatever
    /// route is asked about.
    #[default]
    Unrecorded,
    /// The environment layer ran and no route owns the generic override —
    /// either it was absent, or a higher-precedence file layer (a managed
    /// overlay) supplied/reselected the effective route's endpoint. No route,
    /// active or pinned, may borrow the ambient generic host.
    NoOwner,
    /// The environment layer ran and addressed the override to exactly this
    /// `(provider, identity)`. Only that route resolves it; every other route
    /// falls through to its own default.
    Route(ApiProvider, String),
}

impl BaseUrlEnvReceipt {
    /// Whether `(provider, identity)` is the route this receipt names.
    fn owns(&self, provider: ApiProvider, identity: &str) -> bool {
        match self {
            Self::Route(owner, owner_identity) => *owner == provider && owner_identity == identity,
            Self::Unrecorded | Self::NoOwner => false,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AutoReviewConfig {
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
    #[serde(default, alias = "textContains")]
    pub(crate) text_contains: Option<String>,
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
            crate::tui::auto_review::AutoReviewAction::AskUser => {
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
    }
}

fn validate_auto_review_rules(kind: &str, rules: &[AutoReviewRuleConfig]) -> Result<()> {
    for (index, rule) in rules.iter().enumerate() {
        if rule
            .text_contains
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            anyhow::bail!(
                "Invalid auto_review.{kind}[{index}].text_contains: user-intent matching was retired; scope the rule with tool and/or action_kind."
            );
        }
        if !rule.has_matcher() {
            anyhow::bail!(
                "Invalid auto_review.{kind}[{index}]: set at least one of tool or action_kind."
            );
        }
        if let Some(action_kind) = rule.action_kind.as_deref() {
            let normalized = action_kind.trim().to_ascii_lowercase().replace('-', "_");
            if parse_auto_review_action_kind(&normalized).is_none() {
                anyhow::bail!(
                    "Invalid auto_review.{kind}[{index}].action_kind '{action_kind}': expected read, write, shell, external, publish, or destructive."
                );
            }
            if kind == "allow"
                && !matches!(
                    normalized.as_str(),
                    "read" | "write" | "shell" | "external" | "publish" | "destructive"
                )
            {
                anyhow::bail!(
                    "Invalid auto_review.allow[{index}].action_kind '{action_kind}': this retired narrow kind cannot safely widen to a v0.9.8 decision class; replace it with an exact tool rule or a current action_kind."
                );
            }
        }
    }
    Ok(())
}

fn parse_auto_review_action_kind(raw: &str) -> Option<crate::tui::auto_review::ToolActionKind> {
    match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "read" | "mcp_read" => Some(crate::tui::auto_review::ToolActionKind::Read),
        "write" => Some(crate::tui::auto_review::ToolActionKind::Write),
        "shell" => Some(crate::tui::auto_review::ToolActionKind::Shell),
        "external" | "network" | "git" | "mcp_action" | "browser" | "unknown" => {
            Some(crate::tui::auto_review::ToolActionKind::External)
        }
        "publish" => Some(crate::tui::auto_review::ToolActionKind::Publish),
        "destructive" | "secret" => Some(crate::tui::auto_review::ToolActionKind::Destructive),
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
    /// When true, skill discovery scans only Codewhale-owned skill roots
    /// (plus any explicit `skills_dir`) instead of importing compatible
    /// directories from other AI tools such as Claude, OpenCode, or Cursor.
    #[serde(default, alias = "scanCodewhaleOnly")]
    pub scan_codewhale_only: Option<bool>,
}

impl SkillsConfig {
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
    /// Explicit fake-IP placeholder CIDRs for those proxy hosts. Only subnets
    /// within `198.18.0.0/15` are accepted by the runtime SSRF guard.
    #[serde(default)]
    pub proxy_fake_ip_cidrs: Vec<String>,
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
            proxy_fake_ip_cidrs: Vec::new(),
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
            proxy_fake_ip_cidrs: self.proxy_fake_ip_cidrs,
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
    /// Dual-wire dialect toggle: `openai` (default) or `anthropic`.
    /// Not a separate catalog provider — config only (DeepSeek / MiniMax /
    /// Model Studio).
    #[serde(
        default,
        alias = "apiStyle",
        alias = "api_style",
        alias = "protocol",
        alias = "wire_format",
        alias = "wireFormat",
        alias = "dialect"
    )]
    pub wire: Option<String>,
    #[serde(alias = "authMode")]
    pub auth_mode: Option<String>,
    /// Validated basename of the active Codewhale-owned xAI OAuth generation.
    /// The file always lives below Codewhale's private credentials directory.
    #[serde(default, alias = "oauthCredentialGeneration")]
    pub oauth_credential_generation: Option<String>,
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
    /// Explicit, provider-scoped consent for one credential file owned by
    /// another CLI. Absence is the disabled default.
    #[serde(default, alias = "externalCredentials")]
    pub external_credentials: Option<codewhale_config::ExternalCredentialConsentToml>,
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
    #[serde(default, alias = "orca_router", alias = "orca")]
    pub orcarouter: ProviderConfig,
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
    #[serde(default, alias = "ollama-cloud", alias = "ollamaCloud")]
    pub ollama_cloud: ProviderConfig,
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
    #[serde(
        default,
        alias = "minimax-anthropic",
        alias = "minimaxAnthropic",
        alias = "mini-max-anthropic",
        alias = "mini_max_anthropic"
    )]
    pub minimax_anthropic: ProviderConfig,
    #[serde(default, alias = "sakana-ai", alias = "sakana_ai", alias = "fugu")]
    pub sakana: ProviderConfig,
    #[serde(
        default,
        alias = "long-cat",
        alias = "meituan-longcat",
        alias = "meituan"
    )]
    pub longcat: ProviderConfig,
    #[serde(default, alias = "opencode-go", alias = "opencodego")]
    pub opencode_go: ProviderConfig,
    #[serde(
        default,
        alias = "opencode-zen",
        alias = "opencodezen",
        alias = "zen",
        alias = "opencode"
    )]
    pub opencode_zen: ProviderConfig,
    #[serde(
        default,
        alias = "meta-ai",
        alias = "meta_ai",
        alias = "meta-model-api",
        alias = "meta_model_api",
        alias = "muse",
        alias = "muse-spark"
    )]
    pub meta: ProviderConfig,
    #[serde(default, alias = "x-ai", alias = "x_ai", alias = "grok")]
    pub xai: ProviderConfig,
    #[serde(
        default,
        alias = "mistral-ai",
        alias = "mistral_ai",
        alias = "mistralai",
        alias = "la-plateforme",
        alias = "la_plateforme"
    )]
    pub mistral: ProviderConfig,
    #[serde(
        default,
        alias = "google-gemini",
        alias = "google_gemini",
        alias = "gemini"
    )]
    pub google: ProviderConfig,
    #[serde(default, alias = "agy")]
    pub antigravity: ProviderConfig,
    #[serde(
        default,
        alias = "telecom-js",
        alias = "telecom_js",
        alias = "telecomjs-cn",
        alias = "tokenhub"
    )]
    pub telecomjs: ProviderConfig,
    /// Eden AI — OpenAI-compatible AI gateway (aggregator).
    #[serde(default, alias = "eden-ai", alias = "eden_ai")]
    pub edenai: ProviderConfig,
    /// Concentrate — OpenAI Responses-compatible AI gateway (aggregator).
    #[serde(
        default,
        alias = "concentrate-ai",
        alias = "concentrate_ai",
        alias = "concentrateai"
    )]
    pub concentrate: ProviderConfig,
    /// Alibaba Cloud Model Studio — Token Plan (OpenAI-compatible Chat Completions).
    #[serde(default, alias = "modelstudio-token-plan")]
    pub modelstudio_token_plan: ProviderConfig,
    /// Alibaba Cloud Model Studio — Token Plan Anthropic-compatible endpoint.
    #[serde(default, alias = "modelstudio-token-plan-anthropic")]
    pub modelstudio_token_plan_anthropic: ProviderConfig,
    /// Alibaba Cloud Model Studio — Coding Plan (OpenAI-compatible Chat Completions).
    #[serde(default, alias = "modelstudio-coding-plan")]
    pub modelstudio_coding_plan: ProviderConfig,
    /// Alibaba Cloud Model Studio — Coding Plan Anthropic-compatible endpoint.
    #[serde(default, alias = "modelstudio-coding-plan-anthropic")]
    pub modelstudio_coding_plan_anthropic: ProviderConfig,
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
            ("providers.ollama_cloud", &self.ollama_cloud),
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
            ("providers.minimax_anthropic", &self.minimax_anthropic),
            ("providers.sakana", &self.sakana),
            ("providers.opencode_go", &self.opencode_go),
            ("providers.opencode_zen", &self.opencode_zen),
            ("providers.meta", &self.meta),
            ("providers.xai", &self.xai),
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

/// The highest-precedence source that can currently own approval policy.
///
/// The resolved [`Config`] historically retained only the final string, which
/// made an in-session editor unable to distinguish a user-owned root key from
/// a profile, environment, managed, requirements, or project constraint. The
/// destructive Full Access preset uses this classification to fail closed
/// unless it can prove that removing the root key is the operation requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApprovalPolicyControl {
    Unset,
    RootConfig,
    Profile,
    Environment,
    ManagedConfig,
    ProjectConfig,
    Requirements,
    Ambiguous,
}

impl ApprovalPolicyControl {
    #[must_use]
    pub(crate) fn editable_root(self) -> bool {
        matches!(self, Self::Unset | Self::RootConfig)
    }

    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unset => "saved TUI posture",
            Self::RootConfig => "the root config.toml approval_policy",
            Self::Profile => "the active config profile",
            Self::Environment => "DEEPSEEK_APPROVAL_POLICY",
            Self::ManagedConfig => "managed configuration",
            Self::ProjectConfig => "project configuration",
            Self::Requirements => "managed approval requirements",
            Self::Ambiguous => "an unresolved configuration source",
        }
    }
}

/// Highest-precedence source that owns the interactive shell availability
/// switch. Project/profile/environment/managed constraints are intentionally
/// read-only from the root settings editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellAccessControl {
    Unset,
    RootConfig,
    Profile,
    Environment,
    ManagedConfig,
    ProjectConfig,
    Ambiguous,
}

impl ShellAccessControl {
    #[must_use]
    pub(crate) fn editable_root(self) -> bool {
        matches!(self, Self::Unset | Self::RootConfig)
    }

    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unset => "the session default",
            Self::RootConfig => "the root config.toml allow_shell",
            Self::Profile => "the active config profile",
            Self::Environment => "DEEPSEEK_ALLOW_SHELL",
            Self::ManagedConfig => "managed configuration",
            Self::ProjectConfig => "project configuration",
            Self::Ambiguous => "an unresolved configuration source",
        }
    }
}

fn approval_policy_env_is_set() -> bool {
    let read = || {
        std::env::var_os("CODEWHALE_APPROVAL_POLICY").is_some()
            || std::env::var_os("DEEPSEEK_APPROVAL_POLICY").is_some()
    };
    #[cfg(test)]
    {
        crate::test_support::with_test_env_lock(read)
    }
    #[cfg(not(test))]
    {
        read()
    }
}

fn allow_shell_env_is_set() -> bool {
    let read = || {
        std::env::var_os("CODEWHALE_ALLOW_SHELL").is_some()
            || std::env::var_os("DEEPSEEK_ALLOW_SHELL").is_some()
    };
    #[cfg(test)]
    {
        crate::test_support::with_test_env_lock(read)
    }
    #[cfg(not(test))]
    {
        read()
    }
}

fn project_config_root_bool(workspace: &Path, key: &str) -> Option<bool> {
    [
        workspace
            .join(codewhale_config::CODEWHALE_APP_DIR)
            .join("config.toml"),
        workspace
            .join(codewhale_config::LEGACY_APP_DIR)
            .join("config.toml"),
    ]
    .into_iter()
    .find(|path| path.exists())
    .and_then(|path| std::fs::read_to_string(path).ok())
    .and_then(|raw| toml::from_str::<toml::Value>(&raw).ok())
    .and_then(|document| document.get(key).and_then(toml::Value::as_bool))
}

/// Map the saved TUI permission posture onto the approval-policy ordering used
/// by project config. Full Access is looser than every project policy, so its
/// baseline is the loosest ranked policy (`auto`).
#[must_use]
pub(crate) fn approval_policy_baseline_from_permission_posture(
    posture: Option<&str>,
) -> Option<&'static str> {
    posture.and_then(
        |posture| match posture.trim().to_ascii_lowercase().as_str() {
            "ask" | "suggest" | "on-request" | "untrusted" => Some("on-request"),
            "auto" | "auto-review" | "auto_review" => Some("auto"),
            "full" | "full-access" | "full_access" | "bypass" => Some("auto"),
            _ => None,
        },
    )
}

// === Config Loading ===

impl Config {
    /// The read deny-list in force for this config (S1).
    ///
    /// Unions the built-in credential-store defaults (unless
    /// `sandbox_read_denylist_defaults = false`) with
    /// `sandbox_denied_read_paths`, minus `sandbox_read_denylist_exempt`.
    /// Feeds both the in-process file-reading tools and the OS sandbox
    /// wrappers, so a shell `cat` and a `read_file` call refuse the same paths.
    #[must_use]
    pub fn read_denylist(&self) -> crate::sandbox::read_guard::ReadDenylist {
        crate::sandbox::read_guard::ReadDenylist::build(
            self.sandbox_read_denylist_defaults.unwrap_or(true),
            &self.sandbox_denied_read_paths,
            &self.sandbox_read_denylist_exempt,
        )
    }

    /// Every path the OS sandbox wrappers should deny reads under — the
    /// deny-list's subtree rules, home-expanded and normalized.
    #[must_use]
    pub fn effective_sandbox_denied_read_paths(&self) -> Vec<std::path::PathBuf> {
        self.read_denylist().subtree_paths()
    }

    #[must_use]
    pub fn stop_words(&self) -> Vec<String> {
        self.stop_words.clone().unwrap_or_else(default_stop_words)
    }

    /// Structural external-credential status for user-facing inventory. This
    /// resolves only environment/config strings and performs no filesystem or
    /// network access.
    pub(crate) fn external_credential_consent_status(
        &self,
        provider: ApiProvider,
    ) -> Option<codewhale_config::ExternalCredentialConsentStatus> {
        let (kind, source, path) = match provider {
            ApiProvider::OpenaiCodex => (
                codewhale_config::ProviderKind::OpenaiCodex,
                codewhale_config::ExternalCredentialSource::CodexCli,
                crate::oauth::auth_file_path(),
            ),
            ApiProvider::Xai => (
                codewhale_config::ProviderKind::Xai,
                codewhale_config::ExternalCredentialSource::GrokCli,
                crate::xai_oauth::auth_file_path(),
            ),
            ApiProvider::Deepseek => (
                codewhale_config::ProviderKind::Deepseek,
                codewhale_config::ExternalCredentialSource::DshCli,
                codewhale_config::default_dsh_credentials_path(),
            ),
            ApiProvider::DeepseekAnthropic => (
                codewhale_config::ProviderKind::DeepseekAnthropic,
                codewhale_config::ExternalCredentialSource::DshCli,
                codewhale_config::default_dsh_credentials_path(),
            ),
            _ => return None,
        };
        let active_kind = self
            .api_provider()
            .kind()
            .unwrap_or(codewhale_config::ProviderKind::Deepseek);
        let consent = self
            .provider_config_for(provider)
            .and_then(|entry| entry.external_credentials.as_ref());
        Some(codewhale_config::external_credential_consent_status(
            consent,
            kind,
            source,
            &path,
            active_kind,
        ))
    }

    /// Return the non-root source that prevents an interactive runtime preset
    /// from safely rewriting approval, shell, and sandbox posture. Presets may
    /// edit user-owned root keys, but must never overwrite a profile, env,
    /// managed, requirements, or project constraint in the live merged Config.
    #[must_use]
    pub(crate) fn runtime_preset_blocker(
        &self,
        config_path: Option<&Path>,
        profile: Option<&str>,
        workspace: &Path,
    ) -> Option<&'static str> {
        let requirements_path = self
            .requirements_path
            .as_deref()
            .map(expand_path)
            .or_else(default_requirements_path);
        if let Some(path) = requirements_path
            && path.exists()
        {
            let controlled = std::fs::read_to_string(path)
                .ok()
                .and_then(|raw| toml::from_str::<RequirementsFile>(&raw).ok())
                .is_none_or(|requirements| {
                    !requirements.allowed_approval_policies.is_empty()
                        || !requirements.allowed_sandbox_modes.is_empty()
                });
            if controlled {
                return Some("managed runtime requirements");
            }
        }

        let workspace_is_home = effective_home_dir().is_some_and(|home| {
            let workspace = workspace
                .canonicalize()
                .unwrap_or_else(|_| workspace.to_path_buf());
            let home = home.canonicalize().unwrap_or(home);
            workspace == home
        });
        let project_controls_runtime = || {
            let saved_approval_baseline = crate::settings::Settings::load_persisted()
                .ok()
                .and_then(|settings| settings.permission_posture)
                .and_then(|posture| {
                    approval_policy_baseline_from_permission_posture(Some(&posture))
                });
            let approval_baseline = self.approval_policy.as_deref().or(saved_approval_baseline);
            let parsed_controls =
                codewhale_config::load_project_config(workspace).is_some_and(|project| {
                    project.approval_policy.as_deref().is_some_and(|policy| {
                        codewhale_config::project_approval_policy_is_allowed(
                            approval_baseline,
                            policy,
                        )
                    }) || project.sandbox_mode.as_deref().is_some_and(|sandbox| {
                        codewhale_config::project_sandbox_mode_is_allowed(
                            self.sandbox_mode.as_deref(),
                            sandbox,
                        )
                    })
                });
            parsed_controls || project_config_root_bool(workspace, "allow_shell") == Some(false)
        };
        if !workspace_is_home && project_controls_runtime() {
            return Some("project runtime configuration");
        }

        let managed_path = self
            .managed_config_path
            .as_deref()
            .map(expand_path)
            .or_else(default_managed_config_path);
        if let Some(path) = managed_path
            && path.exists()
        {
            match load_single_config_file(&path) {
                Ok(managed)
                    if managed.approval_policy.is_some()
                        || managed.sandbox_mode.is_some()
                        || managed.allow_shell.is_some() =>
                {
                    return Some("managed runtime configuration");
                }
                Err(_) => return Some("an unreadable managed runtime configuration"),
                Ok(_) => {}
            }
        }

        if [
            "DEEPSEEK_APPROVAL_POLICY",
            "DEEPSEEK_SANDBOX_MODE",
            "DEEPSEEK_ALLOW_SHELL",
        ]
        .into_iter()
        .any(|name| std::env::var_os(name).is_some())
        {
            return Some("environment-controlled runtime posture");
        }

        if let Some(profile) = profile {
            let path = match resolve_load_config_path(config_path.map(Path::to_path_buf)) {
                Ok(Some(path)) => path,
                Ok(None) => return Some("an unresolved active config profile"),
                Err(_) => return Some("an invalid active config path override"),
            };
            let Some(parsed) = std::fs::read_to_string(path)
                .ok()
                .and_then(|raw| toml::from_str::<ConfigFile>(&raw).ok())
            else {
                return Some("an unreadable active config profile");
            };
            if parsed
                .profiles
                .as_ref()
                .and_then(|profiles| profiles.get(profile))
                .is_some_and(|profile| {
                    profile.approval_policy.is_some()
                        || profile.sandbox_mode.is_some()
                        || profile.allow_shell.is_some()
                })
            {
                return Some("the active config profile");
            }
        }

        None
    }

    /// Identify whether the effective approval policy can safely be edited by
    /// changing the root user config. Sources applied later in the load chain
    /// are deliberately treated as controlling even when their value happens
    /// to equal the root value; equality is not provenance.
    #[must_use]
    pub(crate) fn approval_policy_control(
        &self,
        config_path: Option<&Path>,
        profile: Option<&str>,
        workspace: &Path,
    ) -> ApprovalPolicyControl {
        if self.approval_policy_is_requirements_managed() {
            return ApprovalPolicyControl::Requirements;
        }

        let workspace_is_home = effective_home_dir().is_some_and(|home| {
            let workspace = workspace
                .canonicalize()
                .unwrap_or_else(|_| workspace.to_path_buf());
            let home = home.canonicalize().unwrap_or(home);
            workspace == home
        });
        if !workspace_is_home {
            let saved_approval_baseline = crate::settings::Settings::load_persisted()
                .ok()
                .and_then(|settings| settings.permission_posture)
                .and_then(|posture| {
                    approval_policy_baseline_from_permission_posture(Some(&posture))
                });
            let approval_baseline = self.approval_policy.as_deref().or(saved_approval_baseline);
            if codewhale_config::load_project_config(workspace)
                .and_then(|project| project.approval_policy)
                .is_some_and(|policy| {
                    codewhale_config::project_approval_policy_is_allowed(approval_baseline, &policy)
                })
            {
                return ApprovalPolicyControl::ProjectConfig;
            }
        }

        let managed_path = self
            .managed_config_path
            .as_deref()
            .map(expand_path)
            .or_else(default_managed_config_path);
        if let Some(path) = managed_path
            && path.exists()
        {
            match load_single_config_file(&path) {
                Ok(managed) if managed.approval_policy.is_some() => {
                    return ApprovalPolicyControl::ManagedConfig;
                }
                Err(_) => return ApprovalPolicyControl::Ambiguous,
                Ok(_) => {}
            }
        }

        if approval_policy_env_is_set() {
            return ApprovalPolicyControl::Environment;
        }

        let path = match resolve_load_config_path(config_path.map(Path::to_path_buf)) {
            Ok(Some(path)) => path,
            Ok(None) | Err(_) => {
                return if self.approval_policy.is_some() {
                    ApprovalPolicyControl::Ambiguous
                } else {
                    ApprovalPolicyControl::Unset
                };
            }
        };
        let parsed = std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| toml::from_str::<ConfigFile>(&raw).ok());
        let Some(parsed) = parsed else {
            return if self.approval_policy.is_some() {
                ApprovalPolicyControl::Ambiguous
            } else {
                ApprovalPolicyControl::Unset
            };
        };
        if let Some(profile) = profile
            && parsed
                .profiles
                .as_ref()
                .and_then(|profiles| profiles.get(profile))
                .is_some_and(|profile| profile.approval_policy.is_some())
        {
            return ApprovalPolicyControl::Profile;
        }
        if parsed.base.approval_policy.is_some() {
            ApprovalPolicyControl::RootConfig
        } else if self.approval_policy.is_some() {
            ApprovalPolicyControl::Ambiguous
        } else {
            ApprovalPolicyControl::Unset
        }
    }

    /// Identify whether shell availability can safely be edited through the
    /// user-owned root config. Later sources are controlling even when their
    /// effective value happens to match the root value.
    #[must_use]
    pub(crate) fn allow_shell_control(
        &self,
        config_path: Option<&Path>,
        profile: Option<&str>,
        workspace: &Path,
    ) -> ShellAccessControl {
        let workspace_is_home = effective_home_dir().is_some_and(|home| {
            let workspace = workspace
                .canonicalize()
                .unwrap_or_else(|_| workspace.to_path_buf());
            let home = home.canonicalize().unwrap_or(home);
            workspace == home
        });
        if !workspace_is_home && project_config_root_bool(workspace, "allow_shell") == Some(false) {
            return ShellAccessControl::ProjectConfig;
        }

        let managed_path = self
            .managed_config_path
            .as_deref()
            .map(expand_path)
            .or_else(default_managed_config_path);
        if let Some(path) = managed_path
            && path.exists()
        {
            match load_single_config_file(&path) {
                Ok(managed) if managed.allow_shell.is_some() => {
                    return ShellAccessControl::ManagedConfig;
                }
                Err(_) => return ShellAccessControl::Ambiguous,
                Ok(_) => {}
            }
        }

        if allow_shell_env_is_set() {
            return ShellAccessControl::Environment;
        }

        let path = match resolve_load_config_path(config_path.map(Path::to_path_buf)) {
            Ok(Some(path)) => path,
            Ok(None) | Err(_) => {
                return if self.allow_shell.is_some() {
                    ShellAccessControl::Ambiguous
                } else {
                    ShellAccessControl::Unset
                };
            }
        };
        let parsed = std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| toml::from_str::<ConfigFile>(&raw).ok());
        let Some(parsed) = parsed else {
            return if self.allow_shell.is_some() {
                ShellAccessControl::Ambiguous
            } else {
                ShellAccessControl::Unset
            };
        };
        if let Some(profile) = profile
            && parsed
                .profiles
                .as_ref()
                .and_then(|profiles| profiles.get(profile))
                .is_some_and(|profile| profile.allow_shell.is_some())
        {
            return ShellAccessControl::Profile;
        }
        if parsed.base.allow_shell.is_some() {
            ShellAccessControl::RootConfig
        } else if self.allow_shell.is_some() {
            ShellAccessControl::Ambiguous
        } else {
            ShellAccessControl::Unset
        }
    }

    /// Whether an explicit config or requirements file owns approval posture.
    /// TUI preferences may supply a default only when this is false.
    #[must_use]
    pub fn approval_policy_is_managed(&self) -> bool {
        if self.approval_policy.is_some() {
            return true;
        }
        self.approval_policy_is_requirements_managed()
    }

    /// Whether organization requirements, rather than a user-editable config
    /// key, own approval posture. User config still outranks TUI settings, but
    /// `/config approval_mode ... --save` may edit that user-owned key.
    #[must_use]
    pub fn approval_policy_is_requirements_managed(&self) -> bool {
        let path = self
            .requirements_path
            .as_deref()
            .map(expand_path)
            .or_else(default_requirements_path);
        let Some(path) = path else {
            return false;
        };
        if !path.exists() {
            return false;
        }
        // Fail closed if a present requirements file becomes unreadable or
        // malformed between Config::load and App::new.
        std::fs::read_to_string(path)
            .ok()
            .and_then(|contents| toml::from_str::<RequirementsFile>(&contents).ok())
            .is_none_or(|requirements| !requirements.allowed_approval_policies.is_empty())
    }

    #[must_use]
    pub fn search_provider_resolution(&self) -> SearchProviderResolution {
        if let Ok(raw) = std::env::var("CODEWHALE_SEARCH_PROVIDER")
            .or_else(|_| std::env::var("DEEPSEEK_SEARCH_PROVIDER"))
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

    /// Store a session/config provider choice and return the effective runtime
    /// provider after applying the documented environment precedence.
    pub fn set_search_provider(&mut self, provider: SearchProvider) -> SearchProvider {
        self.search
            .get_or_insert_with(SearchConfig::default)
            .provider = Some(provider);
        self.search_provider()
    }

    /// Return `true` if the `[auto] cost_saving = true` opt-in is set
    /// (#1207). When true, the auto-mode router biases toward the active
    /// provider's validated fast sibling for ambiguous requests instead of
    /// its strong tier. Providers without a known sibling stay on the active
    /// model. Default: `false` (balanced behaviour).
    #[must_use]
    pub fn auto_cost_saving(&self) -> bool {
        self.auto
            .as_ref()
            .and_then(|a| a.cost_saving)
            .unwrap_or(false)
    }

    /// Return `true` only when `[auto] cross_provider = true` is persisted in
    /// config (#4411). Auto mode otherwise stays on the active provider: the
    /// classifier never sees other providers' routes, and the local heuristic
    /// never selects one. There is no interactive toggle — enabling
    /// cross-provider Auto is an explicit, durable config edit.
    #[must_use]
    pub fn auto_cross_provider(&self) -> bool {
        self.auto
            .as_ref()
            .and_then(|a| a.cross_provider)
            .unwrap_or(false)
    }

    /// Classifier call timeout for `[auto.router]` in seconds. Defaults to
    /// [`DEFAULT_AUTO_ROUTER_TIMEOUT_SECS`] (4); `0` means "use the default".
    /// Values above [`MAX_AUTO_ROUTER_TIMEOUT_SECS`] (300) are clamped so a
    /// hung local router cannot stall a turn indefinitely.
    #[must_use]
    pub fn auto_router_timeout_secs(&self) -> u64 {
        self.auto
            .as_ref()
            .and_then(|a| a.router.as_ref())
            .and_then(|r| r.timeout_secs)
            .filter(|secs| *secs > 0)
            .unwrap_or(DEFAULT_AUTO_ROUTER_TIMEOUT_SECS)
            .min(MAX_AUTO_ROUTER_TIMEOUT_SECS)
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
        Self::load_with_environment_policy(path, profile, ConfigEnvironmentPolicy::Runtime)
    }

    /// Load configuration for a structural diagnostic without materializing
    /// secret-bearing environment values into the returned configuration.
    ///
    /// This still applies the ordinary safe routing, model, and policy
    /// overrides so doctor describes the runtime the user selected. Provider
    /// credentials are resolved only inside an explicit live-probe boundary.
    pub(crate) fn load_structural(path: Option<PathBuf>, profile: Option<&str>) -> Result<Self> {
        Self::load_with_environment_policy(
            path,
            profile,
            ConfigEnvironmentPolicy::StructuralDiagnostic,
        )
    }

    fn load_with_environment_policy(
        path: Option<PathBuf>,
        profile: Option<&str>,
        environment_policy: ConfigEnvironmentPolicy,
    ) -> Result<Self> {
        let path = resolve_load_config_path(path)?;
        let mut config = if let Some(path) = path.as_ref() {
            if path.exists() {
                let contents = fs::read_to_string(path)
                    .with_context(|| format!("Failed to read config file: {}", path.display()))?;
                let parsed: ConfigFile = toml::from_str(&contents).map_err(|_| {
                    anyhow::anyhow!(
                        "Failed to parse config file {}; file contents were omitted",
                        codewhale_config::quote_os_path(path)
                    )
                })?;
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

        apply_env_overrides(&mut config, environment_policy);
        apply_managed_overrides(&mut config)?;
        apply_requirements(&mut config)?;
        normalize_model_config(&mut config);
        config.exec_policy_engine = load_sibling_exec_policy_engine(path.as_deref())?;
        config.validate()?;
        config.warn_on_misplaced_root_base_url();
        Ok(config)
    }

    /// Surface a one-line warning when the user has set the legacy root
    /// `base_url` field but their active provider does not read it. DeepSeek,
    /// the NvidiaNim compatibility sniff, and the literal legacy `custom`
    /// route are the exceptions. Common confusion: users add a top-level
    /// `base_url = "..."` to `~/.deepseek/config.toml` for ollama / vllm /
    /// named OpenAI-compatible servers and wonder why it is ignored (#1308).
    fn warn_on_misplaced_root_base_url(&self) {
        let Some(root_base) = self.base_url.as_deref().map(str::trim) else {
            return;
        };
        if root_base.is_empty() {
            return;
        }
        let provider = self.api_provider();
        if matches!(
            provider,
            ApiProvider::Deepseek
                | ApiProvider::DeepseekCN
                | ApiProvider::XiaomiMimo
                | ApiProvider::OpenaiCodex
        ) {
            return;
        }
        if matches!(provider, ApiProvider::NvidiaNim)
            && root_base.contains("integrate.api.nvidia.com")
        {
            return;
        }
        if provider == ApiProvider::Custom && self.uses_legacy_literal_custom_route() {
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
            && self
                .providers
                .as_ref()
                .and_then(|providers| providers.custom_provider_config(provider))
                .is_none()
        {
            anyhow::bail!(
                "Invalid provider '{provider}': expected {}.",
                ApiProvider::names_hint()
            );
        }
        let active_provider = self.api_provider();
        match validate_kimi_code_api_model_id(
            active_provider,
            &self.deepseek_base_url(),
            &self.default_model(),
        ) {
            Err(error) if error == KIMI_CODE_CLAUDE_ALIAS_GUIDANCE => {
                return Err(SafeConfigDiagnostic::KimiCodeClaudeAlias.into());
            }
            result => result.map_err(anyhow::Error::msg)?,
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
        // Validate the model against the *active provider's* name space, not
        // against DeepSeek's. `canonical_model_id_for_provider` is the
        // equal-treatment resolver: it applies each family's own canonical map
        // (GLM via Z.ai, Kimi, MiniMax, …) and passes unknown ids through, so
        // it rejects only what the provider genuinely cannot serve. Validating
        // with the DeepSeek-only `normalize_model_name` bricked every config
        // whose provider owns a non-DeepSeek family — including ones our own
        // setup wizard writes (`provider = "zai"`, `GLM-5.2`). (#4829)
        if let Some(model) = self.default_text_model.as_deref()
            && !model.trim().eq_ignore_ascii_case("auto")
            && !provider_passes_model_through(self.api_provider())
            && !self.active_provider_preserves_custom_base_url_model()
            && canonical_model_id_for_provider(self.api_provider(), model).is_none()
        {
            let provider = self.api_provider();
            let known = model_completion_names_for_provider(provider);
            let hint = if known.is_empty() {
                String::new()
            } else {
                format!(" (for example: {})", known.join(", "))
            };
            anyhow::bail!(
                "Invalid default_text_model '{model}' for provider '{}': expected auto or a model ID this provider serves{hint}.",
                provider.as_str()
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
        if let Some(transcript) = &self.transcript
            && let Err(detail) = transcript.prose_measure_columns()
        {
            anyhow::bail!("Invalid transcript.prose_measure: {detail}.");
        }
        if let Some(auto_review) = &self.auto_review {
            auto_review.validate()?;
        }
        if let Some(providers) = &self.providers {
            providers.validate()?;
        }
        Ok(())
    }

    /// Resolved prose wrap cap from `[transcript] prose_measure` (#5436).
    ///
    /// `None` (absent or `0`) means prose uses the full content width,
    /// consistent with tool/status cells. Invalid values are rejected by
    /// [`Config::validate`], which every load path runs, so this resolver
    /// cannot fail here.
    #[must_use]
    pub fn prose_measure(&self) -> Option<u16> {
        self.transcript
            .as_ref()
            .and_then(|transcript| transcript.prose_measure_columns().ok().flatten())
    }

    #[must_use]
    pub fn api_provider(&self) -> ApiProvider {
        // #1519 safety fix: when `provider = "<name>"` is not a built-in provider
        // but names a `[providers.<name>]` custom table, route as the dynamic
        // custom identity. Exact configured keys win even when their spelling
        // collides case-insensitively with a built-in slug.
        if let Some(name) = self.provider.as_deref()
            && self
                .providers
                .as_ref()
                .and_then(|providers| providers.custom_provider_config(name))
                .is_some()
        {
            return ApiProvider::Custom;
        }
        if let Some(provider) = self.provider.as_deref().and_then(ApiProvider::parse) {
            if provider == ApiProvider::Ollama && self.selects_legacy_ollama_cloud_route() {
                return ApiProvider::OllamaCloud;
            }
            return provider;
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

    /// Whether the live config uses the released route-sensitive Ollama Cloud
    /// shape. This is a pure in-memory compatibility check: no config or
    /// secret state is rewritten, and only the exact official `/v1` endpoint
    /// upgrades from `ollama` to `ollama-cloud`.
    fn selects_legacy_ollama_cloud_route(&self) -> bool {
        if self.migrated_legacy_ollama_cloud_route {
            return true;
        }
        if self.provider.as_deref().and_then(ApiProvider::parse) != Some(ApiProvider::Ollama) {
            return false;
        }
        self.legacy_ollama_cloud_route_configured()
    }

    /// Whether the legacy Ollama table itself names the exact hosted route,
    /// independent of which provider the parent session currently selects.
    /// Fleet and subagent pins need this route-scoped form.
    fn legacy_ollama_cloud_route_configured(&self) -> bool {
        let base_url = self
            .providers
            .as_ref()
            .and_then(|providers| providers.ollama.base_url.as_deref())
            .map(str::to_string)
            .or_else(|| first_nonempty_env(&["OLLAMA_BASE_URL"]));
        base_url.is_some_and(|base_url| {
            codewhale_config::provider::migrates_legacy_ollama_cloud_route(
                codewhale_config::ProviderKind::Ollama,
                &base_url,
            )
        })
    }

    /// Return the exact non-secret key for an active provider route.
    #[must_use]
    pub(crate) fn provider_identity_for(&self, provider: ApiProvider) -> String {
        if provider == ApiProvider::Custom
            && let Some(name) = self
                .provider
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
            && (self
                .providers
                .as_ref()
                .and_then(|providers| providers.custom_provider_config(name))
                .is_some()
                || ApiProvider::parse(name).is_none())
        {
            return name.to_string();
        }
        provider.as_str().to_string()
    }

    /// Resolve the currently selected live route while retaining whether the
    /// literal custom key came from the legacy root fields or an exact table.
    pub(crate) fn active_provider_identity(
        &self,
        provider: ApiProvider,
    ) -> std::result::Result<ProviderIdentity, String> {
        if provider == ApiProvider::OllamaCloud
            && (self.migrated_legacy_ollama_cloud_route
                || self.provider.as_deref().and_then(ApiProvider::parse)
                    == Some(ApiProvider::Ollama))
            && self.legacy_ollama_cloud_route_configured()
        {
            return self.resolve_provider_identity(ApiProvider::Ollama.as_str());
        }
        self.resolve_provider_identity(&self.provider_identity_for(provider))
    }

    /// Resolve a persisted provider key against the current live config.
    ///
    /// Named custom providers are exact and fail closed: a removed, renamed,
    /// or malformed table can never fall through to DeepSeek or whichever
    /// provider happens to be selected now. The literal legacy value `custom`
    /// remains loadable only for the old root-field config shape where the live
    /// provider is also literally `custom` and both `base_url` and
    /// `default_text_model` identify one valid route.
    pub(crate) fn resolve_provider_identity(
        &self,
        persisted: &str,
    ) -> std::result::Result<ProviderIdentity, String> {
        let key = persisted.trim();
        if key.is_empty() {
            return Err(
                "saved session has an empty provider identity; choose a valid session or repair its `metadata.model_provider` field"
                    .to_string(),
            );
        }

        let has_exact_custom_table = self
            .providers
            .as_ref()
            .and_then(|providers| providers.custom_provider_config(key))
            .is_some();

        if !has_exact_custom_table
            && let Some(mut provider) = ApiProvider::parse(key)
            && provider != ApiProvider::Custom
        {
            let migrated_legacy_ollama_cloud_route =
                provider == ApiProvider::Ollama && self.legacy_ollama_cloud_route_configured();
            if provider == ApiProvider::Ollama && migrated_legacy_ollama_cloud_route {
                provider = ApiProvider::OllamaCloud;
            }
            return Ok(ProviderIdentity {
                provider,
                key: provider.as_str().to_string(),
                exact_id: Some(if migrated_legacy_ollama_cloud_route {
                    ApiProvider::Ollama.as_str().to_string()
                } else {
                    provider.as_str().to_string()
                }),
                migrated_legacy_ollama_cloud_route,
            });
        }

        if !has_exact_custom_table && key.eq_ignore_ascii_case(ApiProvider::Custom.as_str()) {
            if self.selects_literal_custom_provider() {
                // The historical literal `provider = "custom"` can mean
                // either the legacy root-field route or an exact
                // `[providers.custom]` table. Prefer the table when it exists;
                // otherwise validate the legacy root shape. This keeps old
                // save/resume records deterministic without treating the
                // literal key as a wildcard for some other named provider.
                if !has_exact_custom_table {
                    self.validate_legacy_literal_custom_route()?;
                    return Ok(ProviderIdentity {
                        provider: ApiProvider::Custom,
                        key: ApiProvider::Custom.as_str().to_string(),
                        exact_id: None,
                        migrated_legacy_ollama_cloud_route: false,
                    });
                }
            }

            // Pre-exact releases persisted every named custom route as the
            // generic literal `custom`. Migrate that record only when the live
            // config selects the sole valid named custom table; otherwise the
            // old value is genuinely ambiguous and must fail closed.
            if !self.selects_literal_custom_provider() {
                let selected = self.provider.as_deref().map(str::trim).unwrap_or_default();
                let valid_named = self
                    .providers
                    .as_ref()
                    .map(|providers| {
                        providers
                            .custom
                            .keys()
                            .filter(|name| {
                                !name.eq_ignore_ascii_case(ApiProvider::Custom.as_str())
                                    && ApiProvider::parse(name).is_none()
                                    && self.resolve_provider_identity(name).is_ok()
                            })
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if let [name] = valid_named.as_slice()
                    && selected == name
                {
                    return self.resolve_provider_identity(name);
                }
                return Err(format!(
                    "legacy session records only the generic `custom` provider kind, but the live config does not select exactly one valid named custom route (selected '{}', valid named routes: {}). Restore the original single `[providers.<name>]` route or repair the saved provider identity; Codewhale will not guess or fall back",
                    if selected.is_empty() {
                        "<unset>"
                    } else {
                        selected
                    },
                    valid_named.len()
                ));
            }
        }

        let exact_key = key;

        let entry = self
            .providers
            .as_ref()
            .and_then(|providers| providers.custom_provider_config(exact_key))
            .ok_or_else(|| {
                format!(
                    "saved session requires custom provider '{exact_key}', but `[providers.{exact_key}]` is missing from the live config. Restore that exact table and retry; Codewhale will not fall back"
                )
            })?;
        if !entry.is_openai_compatible_custom() {
            return Err(format!(
                "saved session requires custom provider '{exact_key}', but `[providers.{exact_key}]` must set `kind = \"openai-compatible\"`. Fix the live config and retry; Codewhale will not fall back"
            ));
        }
        let base_url = entry
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|base_url| !base_url.is_empty())
            .ok_or_else(|| {
                format!(
                    "saved session requires custom provider '{exact_key}', but `[providers.{exact_key}]` has no `base_url`. Fix the live config and retry; Codewhale will not fall back"
                )
            })?;
        let parsed = reqwest::Url::parse(base_url).map_err(|err| {
            format!(
                "saved session requires custom provider '{exact_key}', but `[providers.{exact_key}].base_url` is invalid: {err}. Fix the live config and retry; Codewhale will not fall back"
            )
        })?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(format!(
                "saved session requires custom provider '{exact_key}', but `[providers.{exact_key}].base_url` must be an http(s) URL with a host. Fix the live config and retry; Codewhale will not fall back"
            ));
        }

        Ok(ProviderIdentity {
            provider: ApiProvider::Custom,
            key: exact_key.to_string(),
            exact_id: Some(exact_key.to_string()),
            migrated_legacy_ollama_cloud_route: false,
        })
    }

    /// Resolve a provider explicitly pinned by a current Fleet/subagent
    /// declaration.
    ///
    /// A scoped legacy Ollama Cloud config retains its migration marker so the
    /// active client can keep reading `[providers.ollama]` and the old secret
    /// slot. That marker is provenance for the active route, not an alias for a
    /// newly declared `ollama-cloud` pin: the explicit pin must bind the
    /// first-class table and credential slot even when it is declared by a
    /// child of the migrated route.
    pub(crate) fn resolve_provider_pin_identity(
        &self,
        provider_id: &str,
    ) -> std::result::Result<ProviderIdentity, String> {
        let mut identity = self.resolve_provider_identity(provider_id)?;
        if identity.provider == ApiProvider::OllamaCloud
            && ApiProvider::parse(provider_id.trim()) == Some(ApiProvider::OllamaCloud)
        {
            identity.migrated_legacy_ollama_cloud_route = false;
        }
        Ok(identity)
    }

    /// Resolve an additive exact provider id. Unlike raw selector resolution,
    /// this never interprets the literal id `custom` as the legacy root route:
    /// an id means the record requires that exact `[providers.<id>]` table.
    fn resolve_exact_provider_identity(
        &self,
        persisted: &str,
    ) -> std::result::Result<ProviderIdentity, String> {
        let id = persisted.trim();
        if id.is_empty() {
            return Err(
                "persisted provider route has an empty exact provider id; Codewhale will not guess or fall back"
                    .to_string(),
            );
        }
        let has_exact_custom_table = self
            .providers
            .as_ref()
            .and_then(|providers| providers.custom_provider_config(id))
            .is_some();
        if id.eq_ignore_ascii_case(ApiProvider::Custom.as_str()) && !has_exact_custom_table {
            return Err(format!(
                "persisted provider route requires exact custom provider '{id}', but `[providers.{id}]` is missing from the live config. Restore that exact table and retry; Codewhale will not fall back"
            ));
        }

        let identity = self.resolve_provider_identity(id)?;
        if identity.provider == ApiProvider::Custom && identity.persisted_id() != Some(id) {
            return Err(format!(
                "persisted provider route requires exact custom provider '{id}', but the live config only provides the legacy root-level custom route. Restore `[providers.{id}]` and retry; Codewhale will not fall back"
            ));
        }
        Ok(identity)
    }

    /// Resolve the two-field provider route written by current session/thread
    /// schemas without erasing which field supplied the identity.
    ///
    /// `provider_kind` is the generic wire/provider class (`custom` for every
    /// named OpenAI-compatible endpoint); `provider_id` is the additive exact
    /// configured key. Older records have no id and may have overloaded the
    /// kind field with an exact custom name. Keeping those cases distinct is
    /// security-sensitive: a legacy built-in record must never be captured by
    /// a later same-key custom table, while a current `custom` + exact-id pair
    /// must retain that user-owned table identity.
    pub(crate) fn resolve_persisted_provider_identity(
        &self,
        provider_kind: Option<&str>,
        provider_id: Option<&str>,
    ) -> std::result::Result<ProviderIdentity, String> {
        let kind = provider_kind
            .map(str::trim)
            .filter(|value| !value.is_empty());
        // Missing and malformed are different security states. An explicitly
        // persisted empty id must reach `resolve_exact_provider_identity` so
        // it fails closed instead of being reinterpreted as an id-less legacy
        // root route.
        let id = provider_id.map(str::trim);

        let Some(kind) = kind else {
            return id.map_or_else(
                || {
                    Err(
                        "persisted provider route has neither a provider kind nor an exact provider id; Codewhale will not guess or fall back"
                            .to_string(),
                    )
                },
                |id| self.resolve_exact_provider_identity(id),
            );
        };

        let Some(mut provider) = ApiProvider::parse(kind) else {
            // Pre-additive releases sometimes wrote an exact named custom key
            // into `model_provider`. Preserve that shape, but reject a
            // contradictory additive id instead of silently choosing one.
            if let Some(id) = id
                && id != kind
            {
                return Err(format!(
                    "persisted provider route has legacy identity '{kind}' but exact provider id '{id}'; repair the mismatched fields because Codewhale will not guess or fall back"
                ));
            }
            return match id {
                Some(id) => self.resolve_exact_provider_identity(id),
                None => self.resolve_provider_identity(kind),
            };
        };
        let migrated_legacy_ollama_cloud = (provider == ApiProvider::Ollama
            && self.legacy_ollama_cloud_route_configured())
            || (provider == ApiProvider::OllamaCloud
                && id.and_then(ApiProvider::parse) == Some(ApiProvider::Ollama)
                && self.legacy_ollama_cloud_route_configured());
        if migrated_legacy_ollama_cloud {
            provider = ApiProvider::OllamaCloud;
        }

        if provider == ApiProvider::Custom {
            if let Some(id) = id {
                let identity = self.resolve_exact_provider_identity(id)?;
                if identity.provider != ApiProvider::Custom {
                    return Err(format!(
                        "persisted provider route declares generic kind 'custom' but exact provider id '{id}' resolves as built-in '{}'; use the matching built-in kind or restore `[providers.{id}]`. Codewhale will not guess or fall back",
                        identity.provider.as_str()
                    ));
                }
                return Ok(identity);
            }

            // The absence of the additive id is itself provenance. Released
            // id-less `custom` records belong to the root-level route only;
            // they must not be captured by a table added under the same key.
            self.validate_legacy_literal_custom_root_route()?;
            return Ok(ProviderIdentity {
                provider: ApiProvider::Custom,
                key: ApiProvider::Custom.as_str().to_string(),
                exact_id: None,
                migrated_legacy_ollama_cloud_route: false,
            });
        }

        if let Some(id) = id
            && ApiProvider::parse(id) != Some(provider)
            && !(migrated_legacy_ollama_cloud
                && ApiProvider::parse(id) == Some(ApiProvider::Ollama))
        {
            return Err(format!(
                "persisted provider route declares built-in kind '{}' but exact provider id '{id}' names a different route; repair the mismatched fields because Codewhale will not guess or fall back",
                provider.as_str()
            ));
        }

        // Exact custom keys normally win raw string resolution. A persisted
        // built-in kind is stronger evidence than that raw key, but Config's
        // single selector cannot represent both routes simultaneously. Fail
        // closed instead of constructing a descriptor whose client would read
        // credentials/settings from the shadowing custom table.
        if self
            .providers
            .as_ref()
            .and_then(|providers| providers.custom_provider_config(provider.as_str()))
            .is_some()
        {
            return Err(format!(
                "persisted provider route requires built-in '{}', but an exact `[providers.{}]` custom route shadows the same selector. Rename the custom route or update the saved provider kind/id pair; Codewhale will not guess or fall back",
                provider.as_str(),
                provider.as_str()
            ));
        }

        Ok(ProviderIdentity {
            provider,
            key: provider.as_str().to_string(),
            exact_id: Some(if migrated_legacy_ollama_cloud {
                ApiProvider::Ollama.as_str().to_string()
            } else {
                provider.as_str().to_string()
            }),
            migrated_legacy_ollama_cloud_route: migrated_legacy_ollama_cloud,
        })
    }

    /// Scope a cloned runtime config to one already-resolved identity. This is
    /// required only for the root-literal custom route: when a later
    /// `[providers.custom]` table coexists, ordinary selector lookup would
    /// otherwise capture the table. Removing it from the scoped clone keeps
    /// the root endpoint authoritative without mutating the live registry.
    pub(crate) fn scope_to_provider_identity(&mut self, identity: &ProviderIdentity) {
        self.migrated_legacy_ollama_cloud_route = identity.migrated_legacy_ollama_cloud_route;
        self.provider = Some(identity.key.clone());
        if identity.provider == ApiProvider::Custom
            && identity.persisted_id().is_none()
            && let Some(providers) = self.providers.as_mut()
        {
            providers.custom.retain(|name, _| {
                !name
                    .trim()
                    .eq_ignore_ascii_case(ApiProvider::Custom.as_str())
            });
        }
    }

    fn validate_legacy_literal_custom_route(&self) -> std::result::Result<(), String> {
        if self.has_literal_custom_provider_table() {
            return Err(
                "legacy `provider = \"custom\"` is ambiguous because `[providers.custom]` is also present. Move the route to one named `[providers.<name>]` table and update the saved provider identity; Codewhale will not guess or fall back"
                    .to_string(),
            );
        }

        self.validate_legacy_literal_custom_root_route()
    }

    fn validate_legacy_literal_custom_root_route(&self) -> std::result::Result<(), String> {
        let selected = self.provider.as_deref().map(str::trim).unwrap_or_default();
        if !self.selects_literal_custom_provider() {
            return Err(format!(
                "legacy session records only the generic `custom` provider kind, but the live config selects '{}'. Only an unchanged legacy config with `provider = \"custom\"` and root-level `base_url`/`default_text_model` can load this session; Codewhale will not guess or fall back",
                if selected.is_empty() {
                    "<unset>"
                } else {
                    selected
                }
            ));
        }

        let base_url = self
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|base_url| !base_url.is_empty())
            .ok_or_else(|| {
                "legacy `provider = \"custom\"` requires a non-empty root-level `base_url` to load a saved session; Codewhale will not use the custom-provider placeholder or fall back"
                    .to_string()
            })?;
        let parsed = reqwest::Url::parse(base_url).map_err(|err| {
            format!(
                "legacy `provider = \"custom\"` has an invalid root-level `base_url`: {err}. Fix the live config and retry; Codewhale will not fall back"
            )
        })?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(
                "legacy `provider = \"custom\"` requires a root-level `base_url` with an http(s) scheme and host; Codewhale will not fall back"
                    .to_string(),
            );
        }

        let model = self
            .default_text_model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .ok_or_else(|| {
                "legacy `provider = \"custom\"` requires a non-empty root-level `default_text_model` to load a saved session; Codewhale will not guess or fall back"
                    .to_string()
            })?;
        if model.eq_ignore_ascii_case("auto") || normalize_custom_model_id(model).is_none() {
            return Err(
                "legacy `provider = \"custom\"` requires one explicit, valid root-level `default_text_model` (not `auto`) to load a saved session; Codewhale will not guess or fall back"
                    .to_string(),
            );
        }

        Ok(())
    }

    fn selects_literal_custom_provider(&self) -> bool {
        self.provider
            .as_deref()
            .map(str::trim)
            .is_some_and(|name| name.eq_ignore_ascii_case(ApiProvider::Custom.as_str()))
    }

    fn has_literal_custom_provider_table(&self) -> bool {
        self.providers.as_ref().is_some_and(|providers| {
            providers.custom.keys().any(|name| {
                name.trim()
                    .eq_ignore_ascii_case(ApiProvider::Custom.as_str())
            })
        })
    }

    pub(crate) fn uses_legacy_literal_custom_route(&self) -> bool {
        self.selects_literal_custom_provider() && !self.has_literal_custom_provider_table()
    }

    /// Whether `identity` names a custom route that this config can resolve.
    ///
    /// Either an exact `[providers.<name>]` custom table, or the legacy
    /// root-field literal `custom` route. Anything else — an empty key, a
    /// removed table, a built-in provider name — is an unresolvable custom
    /// identity and endpoint resolution must fail closed on it.
    ///
    /// The predicate that pins that contract for the regression suite; the
    /// resolver itself fails closed without consulting it.
    #[cfg(test)]
    pub(crate) fn custom_identity_is_resolvable(&self, identity: &str) -> bool {
        self.custom_provider_entry_for_identity(identity).is_some()
            || (identity_is_literal_custom(identity) && self.uses_legacy_literal_custom_route())
    }

    /// Trimmed, non-empty `wire` dialect preference for `provider`'s config
    /// table (`[providers.<name>] wire = "responses" | "anthropic" | "chat"`).
    ///
    /// Single source for the client wire resolver and the capability reporter
    /// so the two cannot drift. `None` means "no preference" — the provider's
    /// static policy applies.
    pub(crate) fn provider_wire_dialect(&self, provider: ApiProvider) -> Option<&str> {
        self.provider_config_for(provider)?
            .wire
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
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
            ApiProvider::Orcarouter => &providers.orcarouter,
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
            ApiProvider::OllamaCloud if self.selects_legacy_ollama_cloud_route() => {
                &providers.ollama
            }
            ApiProvider::OllamaCloud => &providers.ollama_cloud,
            ApiProvider::Volcengine => &providers.volcengine,
            ApiProvider::Huggingface => &providers.huggingface,
            ApiProvider::Deepinfra => &providers.deepinfra,
            ApiProvider::Together => &providers.together,
            ApiProvider::Qianfan => &providers.qianfan,
            ApiProvider::OpenaiCodex => &providers.openai_codex,
            ApiProvider::Anthropic => &providers.anthropic,
            ApiProvider::Openmodel => &providers.openmodel,
            ApiProvider::Zai => &providers.zai,
            ApiProvider::Stepfun => &providers.stepfun,
            ApiProvider::Minimax => &providers.minimax,
            ApiProvider::MinimaxAnthropic => &providers.minimax_anthropic,
            ApiProvider::Sakana => &providers.sakana,
            ApiProvider::LongCat => &providers.longcat,
            ApiProvider::OpencodeGo => &providers.opencode_go,
            ApiProvider::OpencodeZen => &providers.opencode_zen,
            ApiProvider::Meta => &providers.meta,
            ApiProvider::Xai => &providers.xai,
            ApiProvider::Mistral => &providers.mistral,
            ApiProvider::Google => &providers.google,
            ApiProvider::Antigravity => &providers.antigravity,
            ApiProvider::Telecomjs => &providers.telecomjs,
            ApiProvider::Edenai => &providers.edenai,
            ApiProvider::Concentrate => &providers.concentrate,
            ApiProvider::ModelstudioTokenPlan => &providers.modelstudio_token_plan,
            ApiProvider::ModelstudioTokenPlanAnthropic => {
                &providers.modelstudio_token_plan_anthropic
            }
            ApiProvider::ModelstudioCodingPlan => &providers.modelstudio_coding_plan,
            ApiProvider::ModelstudioCodingPlanAnthropic => {
                &providers.modelstudio_coding_plan_anthropic
            }
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
        let legacy_ollama_cloud = self.selects_legacy_ollama_cloud_route();
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
            ApiProvider::Orcarouter => &mut providers.orcarouter,
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
            ApiProvider::OllamaCloud if legacy_ollama_cloud => &mut providers.ollama,
            ApiProvider::OllamaCloud => &mut providers.ollama_cloud,
            ApiProvider::Volcengine => &mut providers.volcengine,
            ApiProvider::Huggingface => &mut providers.huggingface,
            ApiProvider::Deepinfra => &mut providers.deepinfra,
            ApiProvider::Together => &mut providers.together,
            ApiProvider::Qianfan => &mut providers.qianfan,
            ApiProvider::OpenaiCodex => &mut providers.openai_codex,
            ApiProvider::Anthropic => &mut providers.anthropic,
            ApiProvider::Openmodel => &mut providers.openmodel,
            ApiProvider::Zai => &mut providers.zai,
            ApiProvider::Stepfun => &mut providers.stepfun,
            ApiProvider::Minimax => &mut providers.minimax,
            ApiProvider::MinimaxAnthropic => &mut providers.minimax_anthropic,
            ApiProvider::Sakana => &mut providers.sakana,
            ApiProvider::LongCat => &mut providers.longcat,
            ApiProvider::OpencodeGo => &mut providers.opencode_go,
            ApiProvider::OpencodeZen => &mut providers.opencode_zen,
            ApiProvider::Meta => &mut providers.meta,
            ApiProvider::Xai => &mut providers.xai,
            ApiProvider::Mistral => &mut providers.mistral,
            ApiProvider::Google => &mut providers.google,
            ApiProvider::Antigravity => &mut providers.antigravity,
            ApiProvider::Telecomjs => &mut providers.telecomjs,
            ApiProvider::Edenai => &mut providers.edenai,
            ApiProvider::Concentrate => &mut providers.concentrate,
            ApiProvider::ModelstudioTokenPlan => &mut providers.modelstudio_token_plan,
            ApiProvider::ModelstudioTokenPlanAnthropic => {
                &mut providers.modelstudio_token_plan_anthropic
            }
            ApiProvider::ModelstudioCodingPlan => &mut providers.modelstudio_coding_plan,
            ApiProvider::ModelstudioCodingPlanAnthropic => {
                &mut providers.modelstudio_coding_plan_anthropic
            }
            // Handled by the name-keyed early return above (#1519).
            ApiProvider::Custom => unreachable!("custom provider resolved by name above"),
        }
    }

    /// Apply a runtime model override without migrating a released
    /// root-literal custom route into an ambiguous `[providers.custom]` table.
    pub(crate) fn set_provider_model_override(
        &mut self,
        provider: ApiProvider,
        model: Option<String>,
    ) {
        if provider == ApiProvider::Custom && self.uses_legacy_literal_custom_route() {
            self.default_text_model = model;
        } else {
            self.provider_config_for_mut(provider).model = model;
        }
    }

    /// Apply a runtime endpoint override while preserving the storage shape of
    /// a released root-literal custom route.
    pub(crate) fn set_provider_base_url_override(
        &mut self,
        provider: ApiProvider,
        base_url: Option<String>,
    ) {
        if provider == ApiProvider::Custom && self.uses_legacy_literal_custom_route() {
            self.base_url = base_url;
        } else {
            self.provider_config_for_mut(provider).base_url = base_url;
        }
    }

    /// Apply an in-memory credential update without creating a named custom
    /// table for the legacy root-literal route.
    pub(crate) fn set_provider_api_key_override(
        &mut self,
        provider: ApiProvider,
        api_key: Option<String>,
    ) {
        if provider == ApiProvider::Custom && self.uses_legacy_literal_custom_route() {
            self.api_key = api_key;
        } else {
            self.provider_config_for_mut(provider).api_key = api_key;
        }
    }

    /// Mirror a successful native xAI login into the live route config.
    /// Codewhale-owned OAuth storage supersedes any dormant Grok CLI consent.
    pub(crate) fn mark_codewhale_owned_xai_oauth(&mut self, generation: String) {
        let entry = self.provider_config_for_mut(ApiProvider::Xai);
        entry.auth_mode = Some("oauth".to_string());
        entry.oauth_credential_generation = Some(generation);
        entry.external_credentials = None;
    }

    /// Mirror a successful native ChatGPT PKCE login into the live Codex route.
    /// Codewhale-owned OAuth storage supersedes any dormant Codex CLI consent.
    pub(crate) fn mark_codewhale_owned_chatgpt_oauth(&mut self, generation: String) {
        let entry = self.provider_config_for_mut(ApiProvider::OpenaiCodex);
        entry.auth_mode = Some("oauth".to_string());
        entry.oauth_credential_generation = Some(generation);
        entry.external_credentials = None;
    }

    pub(crate) fn clear_codewhale_owned_chatgpt_oauth(&mut self) {
        let entry = self.provider_config_for_mut(ApiProvider::OpenaiCodex);
        if entry
            .oauth_credential_generation
            .as_deref()
            .is_some_and(codewhale_config::is_valid_chatgpt_oauth_generation)
        {
            entry.oauth_credential_generation = None;
            if entry.auth_mode.as_deref() == Some("oauth") {
                entry.auth_mode = None;
            }
        }
    }

    /// Refresh only model-provider route material from a newly loaded disk
    /// snapshot. The receiver is the already-effective interactive Config,
    /// including CLI feature toggles and workspace/project permission overlays;
    /// replacing it wholesale during `/load` could silently loosen those
    /// controls. Provider tables carry their endpoint, auth, headers, TLS,
    /// model-passthrough, and per-route limits as one atomic registry.
    pub(crate) fn refresh_provider_routes_from(&mut self, fresh: &Self) {
        self.provider.clone_from(&fresh.provider);
        self.api_key.clone_from(&fresh.api_key);
        self.base_url.clone_from(&fresh.base_url);
        self.http_headers.clone_from(&fresh.http_headers);
        self.default_text_model
            .clone_from(&fresh.default_text_model);
        self.auth_mode.clone_from(&fresh.auth_mode);
        self.fallback_providers
            .clone_from(&fresh.fallback_providers);
        self.retry.clone_from(&fresh.retry);
        self.providers.clone_from(&fresh.providers);
        self.base_url_env_receipt
            .clone_from(&fresh.base_url_env_receipt);
        self.root_base_url_owner
            .clone_from(&fresh.root_base_url_owner);
        self.reasoning_effort_inferred_from_legacy_alias =
            fresh.reasoning_effort_inferred_from_legacy_alias;
        self.migrated_deepseek_model_alias
            .clone_from(&fresh.migrated_deepseek_model_alias);
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
        let provider = self.api_provider();
        let mut headers = self.http_headers.clone().unwrap_or_default();
        if let Some(provider_headers) = self
            .provider_config_for(provider)
            .and_then(|provider| provider.http_headers.as_ref())
        {
            headers.extend(provider_headers.clone());
        }
        headers.retain(|name, value| !name.trim().is_empty() && !value.trim().is_empty());
        if auth_mode_disables_api_key(self.auth_mode_for_provider(provider).as_deref()) {
            headers.retain(|name, _| !codewhale_config::is_upstream_auth_header(name));
        }
        headers
    }

    fn active_configured_model_id(&self) -> Option<&str> {
        self.provider_config_for(self.api_provider())
            .and_then(|entry| entry.model.as_deref())
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .or_else(|| {
                self.default_text_model
                    .as_deref()
                    .map(str::trim)
                    .filter(|model| !model.is_empty())
            })
    }

    /// Describe a first-party DeepSeek alias that was migrated for the active
    /// route. Custom endpoints retain ownership of the same model strings and
    /// must not receive DeepSeek's deprecation claim.
    pub(crate) fn active_deepseek_alias_deprecation(&self) -> Option<ModelAliasDeprecation> {
        let provider = self.api_provider();
        if !matches!(
            provider,
            ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic
        ) {
            return None;
        }

        let alias = self
            .migrated_deepseek_model_alias
            .as_deref()
            .or_else(|| self.active_configured_model_id())?
            .trim()
            .to_ascii_lowercase();
        let base_url = self.deepseek_base_url();
        if wire_model_for_provider_route(provider, &base_url, &alias) == alias {
            return None;
        }

        deepseek_alias_deprecation(&alias)
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
        let moonshot_config = (provider == ApiProvider::Moonshot)
            .then(|| self.provider_config())
            .flatten();
        let moonshot_uses_kimi_code = moonshot_config.is_some_and(|config| {
            provider_config_uses_kimi_imported_token(config)
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
        // A root DeepSeek-family default must not leak onto a vendor-locked
        // official endpoint that can never serve it (the provider then
        // rejects every request, e.g. `deepseek-v4-pro` on api.x.ai). Custom
        // base URLs keep full pass-through: a compatible proxy may
        // legitimately serve any model id.
        let foreign_root_default = |model: &str| {
            !self.active_provider_preserves_custom_base_url_model()
                && matches!(
                    provider,
                    ApiProvider::Xai | ApiProvider::Openai | ApiProvider::Moonshot
                )
                && normalize_model_name(model).is_some()
        };
        // Xiaomi MiMo: honour a root `default_text_model` that names a MiMo id
        // (canonical aliases or a custom account id). Do not silently drop it
        // for the provider seed default.
        if provider == ApiProvider::XiaomiMimo
            && let Some(model) = self.default_text_model.as_deref()
        {
            if let Some(canonical) = canonical_xiaomi_mimo_model_id(model) {
                return canonical.to_string();
            }
            // Non-empty root value that is not a known foreign DeepSeek id is
            // a deliberate custom MiMo choice — apply it. A stale DeepSeek id
            // still falls through to the provider default below rather than
            // being forwarded to Xiaomi's endpoint.
            let trimmed = model.trim();
            if !trimmed.is_empty() && normalize_model_name(trimmed).is_none() {
                return trimmed.to_string();
            }
        }
        if let Some(model) = self.default_text_model.as_deref()
            && (provider_passes_model_through(provider)
                || self.active_provider_preserves_custom_base_url_model())
            && !foreign_root_default(model)
            // Xiaomi was handled above so a stale DeepSeek root id does not
            // pass through merely because the provider is pass-through.
            && provider != ApiProvider::XiaomiMimo
        {
            return model.trim().to_string();
        }
        if let Some(model) = self.default_text_model.as_deref()
            && provider != ApiProvider::XiaomiMimo
            && !root_deepseek_model_is_foreign_to_direct_provider(provider, model)
            && let Some(normalized) = normalize_model_name_for_provider(provider, model)
            // A wire-slug translation (e.g. the Moonshot map) resolves the
            // foreign default to a native model; an identity result does not.
            && (!foreign_root_default(model) || !normalized.eq_ignore_ascii_case(model.trim()))
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
            ApiProvider::Orcarouter => DEFAULT_ORCAROUTER_MODEL,
            ApiProvider::XiaomiMimo => DEFAULT_XIAOMI_MIMO_MODEL,
            ApiProvider::Novita => DEFAULT_NOVITA_MODEL,
            ApiProvider::Fireworks => DEFAULT_FIREWORKS_MODEL,
            ApiProvider::Siliconflow | ApiProvider::SiliconflowCn => DEFAULT_SILICONFLOW_MODEL,
            ApiProvider::Arcee => DEFAULT_ARCEE_MODEL,
            ApiProvider::Moonshot => DEFAULT_MOONSHOT_MODEL,
            ApiProvider::Sglang => DEFAULT_SGLANG_MODEL,
            ApiProvider::Vllm => DEFAULT_VLLM_MODEL,
            ApiProvider::Ollama => DEFAULT_OLLAMA_MODEL,
            ApiProvider::OllamaCloud => DEFAULT_OLLAMA_CLOUD_MODEL,
            ApiProvider::Volcengine => DEFAULT_VOLCENGINE_MODEL,
            ApiProvider::Huggingface => DEFAULT_HUGGINGFACE_MODEL,
            ApiProvider::Deepinfra => DEFAULT_DEEPINFRA_MODEL,
            ApiProvider::Together => DEFAULT_TOGETHER_MODEL,
            ApiProvider::Qianfan => DEFAULT_QIANFAN_MODEL,
            // Prefer the live Codex roster head over the static seed so a
            // provider switch lands on the current flagship model instead of
            // a stale constant (#5034). Missing/stale/invalid rosters keep
            // the seed default. An explicit root `default_text_model` that is
            // not a foreign DeepSeek id is honoured above this fallback.
            ApiProvider::OpenaiCodex => {
                if let Some(preferred) =
                    crate::codex_model_cache::model_roster().preferred_model_id()
                {
                    return preferred.to_string();
                }
                DEFAULT_OPENAI_CODEX_MODEL
            }
            ApiProvider::Openmodel => DEFAULT_OPENMODEL_MODEL,
            ApiProvider::Zai => DEFAULT_ZAI_MODEL,
            ApiProvider::Stepfun => DEFAULT_STEPFUN_MODEL,
            ApiProvider::Anthropic => DEFAULT_ANTHROPIC_MODEL,
            ApiProvider::Minimax | ApiProvider::MinimaxAnthropic => DEFAULT_MINIMAX_MODEL,
            ApiProvider::Sakana => DEFAULT_SAKANA_MODEL,
            ApiProvider::LongCat => DEFAULT_LONGCAT_MODEL,
            ApiProvider::OpencodeGo => DEFAULT_OPENCODE_GO_MODEL,
            ApiProvider::OpencodeZen => DEFAULT_OPENCODE_ZEN_MODEL,
            ApiProvider::Meta => DEFAULT_META_MODEL,
            ApiProvider::Xai => DEFAULT_XAI_MODEL,
            ApiProvider::Mistral => DEFAULT_MISTRAL_MODEL,
            ApiProvider::Google => DEFAULT_GOOGLE_MODEL,
            ApiProvider::Antigravity => DEFAULT_ANTIGRAVITY_MODEL,
            ApiProvider::Telecomjs => DEFAULT_TELECOMJS_MODEL,
            ApiProvider::Edenai => DEFAULT_EDENAI_MODEL,
            ApiProvider::Concentrate => DEFAULT_CONCENTRATE_MODEL,
            ApiProvider::ModelstudioTokenPlan
            | ApiProvider::ModelstudioTokenPlanAnthropic
            | ApiProvider::ModelstudioCodingPlan
            | ApiProvider::ModelstudioCodingPlanAnthropic => DEFAULT_MODELSTUDIO_TOKEN_PLAN_MODEL,
            // Custom endpoints have no built-in default model; pass through the
            // descriptor placeholder when nothing is configured (#1519).
            ApiProvider::Custom => codewhale_config::ProviderKind::Custom
                .provider()
                .default_model(),
        }
        .to_string()
    }

    /// Return the configured API base URL (normalized) for the selected route.
    #[must_use]
    pub fn deepseek_base_url(&self) -> String {
        self.base_url_for_route(self.api_provider())
    }

    /// Resolve `provider`'s endpoint from the layers that provider actually
    /// owns, in precedence order:
    ///
    /// 1. its own `[providers.<table>]` entry (including in-memory runtime
    ///    overrides), plus the legacy root `base_url` where that field still
    ///    belongs to the route;
    /// 2. its provider-specific environment contract (`MOONSHOT_BASE_URL`,
    ///    `OPENAI_BASE_URL`, ...), which names exactly one provider and is
    ///    therefore sound to read for a route that is not the session's;
    /// 3. the generic `CODEWHALE_BASE_URL` / `DEEPSEEK_BASE_URL` override, but
    ///    only when this config is still the route that override selected;
    /// 4. the provider's canonical default endpoint.
    ///
    /// Step 3 is why this is identity-aware instead of a bare env read.
    /// `CODEWHALE_BASE_URL` is documented as "base URL for the active
    /// provider", and [`apply_env_overrides`] writes it onto exactly one
    /// provider entry. Every cross-provider construction seam — a pinned
    /// subagent/fleet child, the per-turn auto-router, tool routing, a picker
    /// preview — works by cloning the session config and re-pointing
    /// `provider`, so without the ownership check a Moonshot/Z.ai/MiniMax
    /// child in a DeepSeek session would silently inherit the DeepSeek host
    /// and dispatch a pinned model to the wrong vendor.
    pub(crate) fn base_url_for_route(&self, provider: ApiProvider) -> String {
        self.base_url_for_route_identity(provider, &self.provider_identity_for(provider))
    }

    /// [`Config::base_url_for_route`] for an explicitly named identity.
    ///
    /// Named custom routes are resolved by this `identity` — the
    /// `[providers.<name>]` table key — and never by whichever custom route
    /// the session happens to be on. An identity that names no custom table
    /// fails closed to the descriptor placeholder rather than borrowing the
    /// active custom host.
    pub(crate) fn base_url_for_route_identity(
        &self,
        provider: ApiProvider,
        identity: &str,
    ) -> String {
        let provider_base = if provider == ApiProvider::Custom {
            self.custom_provider_entry_for_identity(identity)
                .and_then(|entry| entry.base_url.clone())
        } else {
            self.provider_config_string_with_runtime_fallback(provider, |entry| {
                entry.base_url.clone()
            })
        };
        // Root `base_url` is normally the legacy DeepSeek field. Xiaomi MiMo
        // also reads it when its table has no endpoint. OpenAI Codex must not:
        // a legacy DeepSeek endpoint would otherwise turn a normal Codex OAuth
        // switch into a custom route and make the saved Codex CLI login unusable.
        // NvidiaNim has a back-compat sniff (integrate.api.nvidia.com), and the
        // literal `provider = "custom"` legacy shape retains its root endpoint.
        // Named custom providers always read their own `[providers.<name>]`
        // table.
        let root_base = match provider {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => {
                self.route_owned_root_base_url(provider, identity)
            }
            // Xiaomi MiMo honours a root `base_url` when the per-provider table
            // has none — otherwise a minimal top-level config silently falls
            // back to the official host.
            ApiProvider::XiaomiMimo => self.route_owned_root_base_url(provider, identity),
            ApiProvider::DeepseekAnthropic => None,
            ApiProvider::NvidiaNim => self
                .route_owned_root_base_url(provider, identity)
                .filter(|base| base.contains("integrate.api.nvidia.com")),
            ApiProvider::Openai
            | ApiProvider::Anthropic
            | ApiProvider::Openmodel
            | ApiProvider::Atlascloud
            | ApiProvider::WanjieArk
            | ApiProvider::Openrouter
            | ApiProvider::Orcarouter
            | ApiProvider::OpenaiCodex
            | ApiProvider::Novita
            | ApiProvider::Fireworks
            | ApiProvider::Siliconflow
            | ApiProvider::SiliconflowCn
            | ApiProvider::Arcee
            | ApiProvider::Moonshot
            | ApiProvider::Sglang
            | ApiProvider::Vllm
            | ApiProvider::Ollama
            | ApiProvider::OllamaCloud
            | ApiProvider::Volcengine
            | ApiProvider::Huggingface
            | ApiProvider::Deepinfra
            | ApiProvider::Together
            | ApiProvider::Qianfan
            | ApiProvider::Zai
            | ApiProvider::Stepfun
            | ApiProvider::Minimax
            | ApiProvider::MinimaxAnthropic
            | ApiProvider::Sakana
            | ApiProvider::LongCat
            | ApiProvider::OpencodeGo
            | ApiProvider::OpencodeZen
            | ApiProvider::Meta
            | ApiProvider::Xai
            | ApiProvider::Mistral
            | ApiProvider::Google
            | ApiProvider::Antigravity
            | ApiProvider::Telecomjs
            | ApiProvider::Edenai
            | ApiProvider::Concentrate
            | ApiProvider::ModelstudioTokenPlan
            | ApiProvider::ModelstudioTokenPlanAnthropic
            | ApiProvider::ModelstudioCodingPlan
            | ApiProvider::ModelstudioCodingPlanAnthropic => None,
            // The legacy root endpoint belongs to the literal `custom`
            // identity only. A named custom child asking about its own table
            // must not inherit it.
            ApiProvider::Custom
                if identity_is_literal_custom(identity)
                    && self.uses_legacy_literal_custom_route() =>
            {
                self.route_owned_root_base_url(provider, identity)
            }
            // Named custom routes read their base URL from `provider_base`.
            ApiProvider::Custom => None,
        };
        // A provider-scoped endpoint variable names exactly one provider, so it
        // resolves for the selected identity whether or not that identity is
        // the session route. `apply_env_overrides` only merges these into the
        // active provider's table, which is why a non-active route has to read
        // them here instead of relying on the merged config.
        let configured_base_url = provider_base
            .or(root_base)
            .or_else(|| provider_env_base_url_override(provider));
        let entry = self.provider_config_for(provider);
        let mode = entry.and_then(|e| e.mode.as_deref());
        let wire = entry.and_then(|e| e.wire.as_deref());
        let base = if provider == ApiProvider::XiaomiMimo {
            let config_api_key = entry.and_then(|e| e.api_key.as_deref()).filter(|value| {
                classify_config_api_key_value(value) == ConfigApiKeyValueKind::Literal
            });
            let env_api_key =
                xiaomi_mimo_env_api_key_for_runtime(mode, configured_base_url.as_deref());
            let api_key = config_api_key.or(env_api_key.as_deref());
            resolve_xiaomi_mimo_base_url(configured_base_url, api_key, mode)
        } else if matches!(
            provider,
            ApiProvider::ModelstudioTokenPlan
                | ApiProvider::ModelstudioTokenPlanAnthropic
                | ApiProvider::ModelstudioCodingPlan
                | ApiProvider::ModelstudioCodingPlanAnthropic
        ) {
            resolve_modelstudio_base_url_for_tui(configured_base_url, provider, mode, wire)
        } else if matches!(
            provider,
            ApiProvider::Minimax | ApiProvider::MinimaxAnthropic
        ) {
            resolve_minimax_base_url_for_tui(configured_base_url, provider, wire)
        } else if matches!(
            provider,
            ApiProvider::Deepseek | ApiProvider::DeepseekAnthropic
        ) {
            resolve_deepseek_base_url_for_tui(configured_base_url, provider, wire)
        } else {
            configured_base_url
                .or_else(|| self.route_owned_generic_env_base_url(provider, identity))
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
                        ApiProvider::Orcarouter => DEFAULT_ORCAROUTER_BASE_URL,
                        ApiProvider::XiaomiMimo => DEFAULT_XIAOMI_MIMO_BASE_URL,
                        ApiProvider::Novita => DEFAULT_NOVITA_BASE_URL,
                        ApiProvider::Fireworks => DEFAULT_FIREWORKS_BASE_URL,
                        ApiProvider::Siliconflow => DEFAULT_SILICONFLOW_BASE_URL,
                        ApiProvider::SiliconflowCn => DEFAULT_SILICONFLOW_CN_BASE_URL,
                        ApiProvider::Arcee => DEFAULT_ARCEE_BASE_URL,
                        ApiProvider::Moonshot => {
                            if self
                                .provider_config_for(provider)
                                .is_some_and(provider_config_uses_kimi_imported_token)
                            {
                                DEFAULT_KIMI_CODE_BASE_URL
                            } else {
                                DEFAULT_MOONSHOT_BASE_URL
                            }
                        }
                        ApiProvider::Sglang => DEFAULT_SGLANG_BASE_URL,
                        ApiProvider::Vllm => DEFAULT_VLLM_BASE_URL,
                        ApiProvider::Ollama => DEFAULT_OLLAMA_BASE_URL,
                        ApiProvider::OllamaCloud => DEFAULT_OLLAMA_CLOUD_BASE_URL,
                        ApiProvider::Volcengine => DEFAULT_VOLCENGINE_BASE_URL,
                        ApiProvider::Huggingface => DEFAULT_HUGGINGFACE_BASE_URL,
                        ApiProvider::Deepinfra => DEFAULT_DEEPINFRA_BASE_URL,
                        ApiProvider::Together => DEFAULT_TOGETHER_BASE_URL,
                        ApiProvider::Qianfan => DEFAULT_QIANFAN_BASE_URL,
                        ApiProvider::OpenaiCodex => DEFAULT_OPENAI_CODEX_BASE_URL,
                        ApiProvider::Openmodel => DEFAULT_OPENMODEL_BASE_URL,
                        ApiProvider::Zai => DEFAULT_ZAI_BASE_URL,
                        ApiProvider::Stepfun => DEFAULT_STEPFUN_BASE_URL,
                        ApiProvider::Anthropic => DEFAULT_ANTHROPIC_BASE_URL,
                        ApiProvider::Minimax => DEFAULT_MINIMAX_BASE_URL,
                        ApiProvider::MinimaxAnthropic => DEFAULT_MINIMAX_ANTHROPIC_BASE_URL,
                        ApiProvider::Sakana => DEFAULT_SAKANA_BASE_URL,
                        ApiProvider::LongCat => DEFAULT_LONGCAT_BASE_URL,
                        ApiProvider::OpencodeGo => DEFAULT_OPENCODE_GO_BASE_URL,
                        ApiProvider::OpencodeZen => DEFAULT_OPENCODE_ZEN_BASE_URL,
                        ApiProvider::Meta => DEFAULT_META_BASE_URL,
                        ApiProvider::Xai => DEFAULT_XAI_BASE_URL,
                        ApiProvider::Mistral => DEFAULT_MISTRAL_BASE_URL,
                        ApiProvider::Google => DEFAULT_GOOGLE_BASE_URL,
                        ApiProvider::Antigravity => DEFAULT_ANTIGRAVITY_BASE_URL,
                        ApiProvider::Telecomjs => DEFAULT_TELECOMJS_BASE_URL,
                        ApiProvider::Edenai => DEFAULT_EDENAI_BASE_URL,
                        ApiProvider::Concentrate => DEFAULT_CONCENTRATE_BASE_URL,
                        ApiProvider::ModelstudioTokenPlan
                        | ApiProvider::ModelstudioTokenPlanAnthropic
                        | ApiProvider::ModelstudioCodingPlan
                        | ApiProvider::ModelstudioCodingPlanAnthropic => {
                            DEFAULT_MODELSTUDIO_TOKEN_PLAN_BASE_URL
                        }
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

    /// The generic `CODEWHALE_BASE_URL` / `DEEPSEEK_BASE_URL` override, but
    /// only for the route that override actually selected.
    ///
    /// [`apply_env_overrides`] records the owning `(provider, identity)` in
    /// [`Config::base_url_env_receipt`] at load time and writes the value onto
    /// that provider's own entry. A config later re-pointed at another identity
    /// is a different route: it must fall through to that provider's own
    /// default rather than borrow the session host.
    fn route_owned_generic_env_base_url(
        &self,
        provider: ApiProvider,
        identity: &str,
    ) -> Option<String> {
        match &self.base_url_env_receipt {
            // Never went through the environment layer: keep the established
            // global fallback so directly constructed configs are unaffected.
            BaseUrlEnvReceipt::Unrecorded => env_base_url_override(),
            // A positive "nobody owns it" — a managed overlay took the
            // endpoint. No route may borrow the ambient generic host.
            BaseUrlEnvReceipt::NoOwner => None,
            BaseUrlEnvReceipt::Route(..) => self
                .base_url_env_receipt
                .owns(provider, identity)
                .then(env_base_url_override)
                .flatten(),
        }
    }

    /// The legacy root `base_url`, unless an environment write addressed it to
    /// a different route.
    ///
    /// `Deepseek` and `DeepseekCN` share this one field. A user who writes
    /// `base_url` in their config file still means it for both identities —
    /// that legacy compatibility is preserved by `None` ownership. But when
    /// [`apply_env_overrides`] wrote the value, it wrote it for exactly the
    /// identity that was active, and a pinned child of the sibling identity
    /// must not inherit it.
    fn route_owned_root_base_url(&self, provider: ApiProvider, identity: &str) -> Option<String> {
        let root = self.base_url.clone()?;
        match &self.root_base_url_owner {
            // File-owned legacy root: shared by every route that reads it, as
            // it always has been.
            BaseUrlEnvReceipt::Unrecorded => Some(root),
            // An environment write that a higher-precedence layer has since
            // taken authority over. It belongs to no route.
            BaseUrlEnvReceipt::NoOwner => None,
            BaseUrlEnvReceipt::Route(..) => self
                .root_base_url_owner
                .owns(provider, identity)
                .then_some(root),
        }
    }

    /// Resolve a named custom provider's table by explicit identity.
    ///
    /// Fails closed: an empty identity, or one that names no
    /// `[providers.<name>]` custom table, resolves to nothing instead of
    /// falling back to whichever custom route the session is currently on.
    fn custom_provider_entry_for_identity(&self, identity: &str) -> Option<&ProviderConfig> {
        let key = identity.trim();
        if key.is_empty() {
            return None;
        }
        self.providers.as_ref()?.custom_provider_config(key)
    }

    fn active_provider_preserves_custom_base_url_model(&self) -> bool {
        self.provider_uses_custom_endpoint(self.api_provider())
    }

    /// Whether `provider`'s effective endpoint is a custom host rather than its
    /// shipped one. Resolved through the same identity-aware resolver the
    /// client is built from, so this predicate cannot disagree with the URL the
    /// request will actually be sent to.
    pub(crate) fn provider_uses_custom_endpoint(&self, provider: ApiProvider) -> bool {
        provider_preserves_custom_base_url_model(provider, &self.base_url_for_route(provider))
    }

    /// Whether file-owned credential slots are bound to `provider`'s
    /// effective endpoint.
    ///
    /// The environment can replace the active route's base URL after config
    /// parsing. In that case, a root/provider `api_key` or configured
    /// `api_key_env` still belongs to the file-owned endpoint and must not
    /// follow a newly selected custom host. An explicit source-marked CLI key
    /// remains a deliberate endpoint override and is handled before this
    /// predicate by the runtime resolver.
    pub(crate) fn config_credentials_are_bound_to_provider_endpoint(
        &self,
        provider: ApiProvider,
    ) -> bool {
        provider != self.api_provider()
            || !self.active_base_url_is_environment_owned(provider)
            || !self.provider_uses_custom_endpoint(provider)
    }

    fn active_base_url_is_environment_owned(&self, provider: ApiProvider) -> bool {
        if provider != self.api_provider() {
            return false;
        }
        let identity = self.provider_identity_for(provider);
        if self.base_url_env_receipt.owns(provider, &identity) {
            return true;
        }

        // Below the receipt, the environment can still supply the endpoint for
        // a route that has none of its own. A provider-scoped variable names
        // exactly one provider, so it always owns that route's endpoint. The
        // generic variable only does so while no receipt has said otherwise —
        // once a receipt exists and does not name this route,
        // `route_owned_generic_env_base_url` refuses it, so claiming env
        // ownership here would contradict the URL actually resolved.
        if self.configured_base_url_for_provider(provider).is_some() {
            return false;
        }
        provider_env_base_url_override(provider).is_some()
            || (matches!(self.base_url_env_receipt, BaseUrlEnvReceipt::Unrecorded)
                && env_base_url_override().is_some())
    }

    /// The endpoint `provider` owns through a file or in-memory layer, before
    /// the environment layer is consulted.
    ///
    /// The legacy root field is read through
    /// [`Config::route_owned_root_base_url`] so an environment write addressed
    /// to one identity is not mistaken for the sibling identity's configured
    /// endpoint. DeepSeek and Xiaomi MiMo honour root `base_url` when their
    /// per-provider table has none. OpenAI Codex does not: its OAuth login is
    /// valid only for the official route, not an inherited legacy endpoint.
    fn configured_base_url_for_provider(&self, provider: ApiProvider) -> Option<String> {
        let identity = self.provider_identity_for(provider);
        let provider_base = self
            .provider_config_string_with_runtime_fallback(provider, |entry| entry.base_url.clone());
        match provider {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::XiaomiMimo => {
                provider_base.or_else(|| self.route_owned_root_base_url(provider, &identity))
            }
            ApiProvider::NvidiaNim => provider_base.or_else(|| {
                self.route_owned_root_base_url(provider, &identity)
                    .filter(|base| base.contains("integrate.api.nvidia.com"))
            }),
            ApiProvider::Custom if self.uses_legacy_literal_custom_route() => {
                provider_base.or_else(|| self.route_owned_root_base_url(provider, &identity))
            }
            _ => provider_base,
        }
        .filter(|base| !base.trim().is_empty())
    }

    /// Whether model ids for `provider` belong to the configured endpoint.
    ///
    /// Every route — active or pinned — is judged on the endpoint it will
    /// actually be dispatched to, so a pinned child cannot canonicalize model
    /// ids for a host that owns its own namespace (or pass through ids on a
    /// route that resolves to a canonical endpoint). The resolver behind
    /// [`Config::provider_uses_custom_endpoint`] is identity-aware, so this no
    /// longer risks attributing the session's endpoint to another provider.
    pub(crate) fn model_ids_pass_through_for_provider(&self, provider: ApiProvider) -> bool {
        provider_passes_model_through(provider) || self.provider_uses_custom_endpoint(provider)
    }

    pub(crate) fn model_ids_pass_through(&self) -> bool {
        self.model_ids_pass_through_for_provider(self.api_provider())
    }

    pub(crate) fn auth_mode_for_provider(&self, provider: ApiProvider) -> Option<String> {
        self.provider_config_string_with_runtime_fallback(provider, |entry| entry.auth_mode.clone())
            .or_else(|| {
                (provider == self.api_provider())
                    .then(|| self.auth_mode.clone())
                    .flatten()
            })
    }

    /// Mint a read capability for the exact external credential path selected
    /// when consent was granted.
    ///
    /// Path resolution itself is side-effect free. The returned capability is
    /// required by every external credential adapter before it may stat or
    /// read the selected file. `suggested_path` is used only in disabled-mode
    /// guidance; an existing grant remains pinned to its persisted path even
    /// if ambient CLI-home environment variables change later.
    pub(crate) fn external_credential_read_grant(
        &self,
        provider: ApiProvider,
        source: codewhale_config::ExternalCredentialSource,
        suggested_path: &Path,
    ) -> Result<codewhale_config::ExternalCredentialReadGrant> {
        if provider != self.api_provider() {
            anyhow::bail!(
                "external credential access for {} is dormant until that provider is explicitly selected",
                provider.display_name()
            );
        }
        let kind = provider
            .metadata()
            .map(codewhale_config::provider::Provider::kind)
            .context("external credentials are unsupported for this provider")?;
        let consent = self
            .provider_config_for(provider)
            .and_then(|entry| entry.external_credentials.as_ref())
            .with_context(|| {
                format!(
                    "External credentials owned by {} are disabled for {}. To allow read-only access to this exact file, run:\n  codewhale auth external-consent --provider {} --mode read-only --path {}",
                    source.as_str(),
                    provider.display_name(),
                    kind.as_str(),
                    codewhale_config::quote_os_path(suggested_path)
                )
            })?;
        consent
            .read_grant(kind, source, &consent.path)
            .map_err(|error| {
                anyhow::anyhow!(
                    "external credential consent for {}: {error}",
                    provider.display_name()
                )
            })
    }

    /// Whether a structurally valid read-only consent record exists for an
    /// external credential source. This never stats or reads the selected
    /// file and never mints the capability required to do so.
    pub(crate) fn external_credential_read_consent_configured(
        &self,
        provider: ApiProvider,
        source: codewhale_config::ExternalCredentialSource,
    ) -> bool {
        let Some(kind) = provider
            .metadata()
            .map(codewhale_config::provider::Provider::kind)
        else {
            return false;
        };
        let Some(consent) = self
            .provider_config_for(provider)
            .and_then(|entry| entry.external_credentials.as_ref())
        else {
            return false;
        };
        consent
            .validate_read_scope(kind, source, &consent.path)
            .is_ok()
    }

    pub(crate) fn should_skip_secret_store_for_provider(&self, provider: ApiProvider) -> bool {
        // The CLI's durable credential namespace has one compatibility slot
        // named `custom`; it cannot identify an arbitrary named custom route.
        // Reusing that slot for `[providers.<name>]` could send endpoint A's
        // bearer token to endpoint B. Named routes therefore resolve only
        // their own config/auth/api_key_env sources. The generic slot remains
        // valid solely for the literal legacy root-field custom route.
        if provider == ApiProvider::Custom && !self.uses_legacy_literal_custom_route() {
            return true;
        }

        let auth_mode = self.auth_mode_for_provider(provider);
        if auth_mode_disables_api_key(auth_mode.as_deref()) {
            return true;
        }
        if self.provider_uses_custom_endpoint(provider) {
            // An explicitly authenticated loopback runtime may intentionally
            // use the durable provider slot (for example a protected local
            // vLLM server). Remote custom endpoints must never inherit an
            // official provider's saved credential.
            let explicitly_authenticated_loopback = provider == self.api_provider()
                && auth_mode_requires_api_key(auth_mode.as_deref())
                && base_url_uses_local_host(&self.deepseek_base_url());
            if !explicitly_authenticated_loopback {
                return true;
            }
        }
        if auth_mode_requires_api_key(auth_mode.as_deref()) {
            return false;
        }

        provider_route_is_keyless_self_hosted(provider, &self.base_url_for_route(provider))
            || (provider == self.api_provider()
                && base_url_uses_local_host(&self.deepseek_base_url()))
    }

    /// Read the API key.
    ///
    /// Precedence: **route-specific explicitly consented OAuth token → source-marked explicit CLI key →
    /// provider/root config → configured custom-provider environment →
    /// secret store → ambient provider environment**.
    ///
    /// The in-memory `self.api_key` override is only honored when the user
    /// explicitly set the field (not the legacy `API_KEYRING_SENTINEL`
    /// placeholder, not empty whitespace).
    pub fn deepseek_api_key(&self) -> Result<String> {
        self.deepseek_api_key_with_secret_store_mode(false)
    }

    /// Resolve an API key for a diagnostic without migrating a legacy secret
    /// store or opening a write-capable secret backend.
    ///
    /// This retains ordinary credential precedence, including a legacy
    /// file-backed secret as a fallback, but it must only be used by static
    /// diagnostic/reporting paths. Normal runtime and authentication paths use
    /// [`Self::deepseek_api_key`] and preserve their existing migration
    /// behavior.
    pub(crate) fn deepseek_api_key_read_only(&self) -> Result<String> {
        self.deepseek_api_key_with_secret_store_mode(true)
    }

    /// Clone this route with a diagnostic-only credential in its in-memory
    /// provider slot.
    ///
    /// A live `doctor` probe still needs to construct the ordinary client. By
    /// materializing the credential on an isolated clone first, that client
    /// never reaches the normal migrating secret-store resolver while it is
    /// only checking connectivity. The clone is process-local and is never
    /// persisted.
    pub(crate) fn with_read_only_api_key_for_diagnostic(&self) -> Result<Self> {
        let provider = self.api_provider();
        let api_key = self.deepseek_api_key_read_only()?;
        let mut diagnostic = self.clone();
        diagnostic.set_provider_api_key_override(provider, Some(api_key));
        Ok(diagnostic)
    }

    fn deepseek_api_key_with_secret_store_mode(&self, read_only: bool) -> Result<String> {
        let provider = self.api_provider();
        let auth_mode = self.auth_mode_for_provider(provider);
        if auth_mode_disables_api_key(auth_mode.as_deref()) {
            return Ok(String::new());
        }
        let custom_endpoint = self.provider_uses_custom_endpoint(provider);
        let explicit_cli_key = explicit_cli_api_key_override();

        // 0. Legacy root compatibility slot. The top-level `api_key` belongs
        // to DeepSeek, plus the literal root-field `provider = "custom"`
        // compatibility route. Provider-specific keys below must win for all
        // named/custom-table routes so a stale root key is not sent elsewhere.
        //
        // However, when the CLI dispatcher forwards an explicit `--api-key`
        // through the provider-neutral CLI bridge with its source marker, that
        // intentional override must win over the saved root key. This is
        // essential for DeepSeek-compatible subscription endpoints where the
        // user runs something like:
        //   codewhale --provider deepseek --api-key ark-... --base-url ... --model auto
        if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
            && cli_api_key_source().as_deref() == Some("cli")
            && let Some(env_key) = explicit_cli_key
                .as_ref()
                .cloned()
                .or_else(|| provider_env_api_key(provider))
            && !env_key.trim().is_empty()
        {
            return Ok(env_key);
        }
        if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
            && self.config_credentials_are_bound_to_provider_endpoint(provider)
            && let Some(configured) = self.api_key.as_ref()
            && classify_config_api_key_value(configured) == ConfigApiKeyValueKind::Literal
        {
            warn_on_config_api_key_shadowing(self, provider, "the root api_key");
            return Ok(configured.clone());
        }

        if provider == ApiProvider::Moonshot
            && !custom_endpoint
            && self
                .provider_config_for(provider)
                .is_some_and(provider_config_uses_kimi_imported_token)
        {
            let credential_help =
                credential_help_for_provider_route(provider, &self.deepseek_base_url());
            anyhow::bail!(
                "Kimi CLI credential import is unsupported. Codewhale does not impersonate or reuse Kimi OAuth clients; configure an API key from {} instead.",
                credential_help
                    .credential_url
                    .unwrap_or("the selected provider's API-key console")
            );
        }

        // xAI OAuth prefers Codewhale-owned device-login storage. An existing
        // Grok CLI file is considered only with provider/path-scoped read-only
        // consent. Activated by [providers.xai] auth_mode = "oauth".
        if provider == ApiProvider::Xai
            && !custom_endpoint
            && self
                .provider_config_for(provider)
                .is_some_and(provider_config_uses_xai_oauth)
            && crate::xai_oauth::credentials_present(self)
        {
            return crate::xai_oauth::get_access_token(self);
        }

        // OpenAI Codex (ChatGPT) can read an existing Codex CLI OAuth login
        // only after exact read-only consent. Codewhale never refreshes or
        // rewrites that file. Explicit env overrides remain process-scoped.
        if provider == ApiProvider::OpenaiCodex && !custom_endpoint {
            if let Some(credentials) = crate::oauth::credentials_from_env() {
                return Ok(credentials.access_token);
            }
            let path = crate::oauth::auth_file_path();
            let grant = self.external_credential_read_grant(
                provider,
                codewhale_config::ExternalCredentialSource::CodexCli,
                &path,
            )?;
            return Ok(crate::oauth::get_credentials(&grant)?.access_token);
        }

        // The dispatcher cannot know the effective provider until the TUI
        // applies `--profile`. A provider-neutral, source-marked CLI override
        // therefore wins over saved API-key slots here, after OAuth routes
        // have made their own credential decision.
        if let Some(value) = explicit_cli_key {
            return Ok(value);
        }

        // 1. Config file (provider-scoped slot). This intentionally wins
        // over ambient env so `codewhale auth set` fixes stale shell exports.
        if self.config_credentials_are_bound_to_provider_endpoint(provider)
            && let Some(configured) = self
                .provider_config_string_with_runtime_fallback(provider, |entry| {
                    entry.api_key.clone()
                })
            && classify_config_api_key_value(&configured) == ConfigApiKeyValueKind::Literal
        {
            let config_source = match provider_config_table_name(provider) {
                Ok(table) => format!("`{table}` api_key"),
                Err(_) => "the provider config-table api_key".to_string(),
            };
            warn_on_config_api_key_shadowing(self, provider, &config_source);
            return Ok(configured);
        }
        if provider == ApiProvider::Custom
            && self.uses_legacy_literal_custom_route()
            && self.config_credentials_are_bound_to_provider_endpoint(provider)
            && let Some(configured) = self.api_key.as_ref()
            && classify_config_api_key_value(configured) == ConfigApiKeyValueKind::Literal
        {
            warn_on_config_api_key_shadowing(self, provider, "the root api_key");
            return Ok(configured.clone());
        }

        // 1b. A route can explicitly bind an environment variable by name via
        // `[providers.<name>] api_key_env = "..."`. This remains safe for a
        // custom endpoint because the binding belongs to that route; ambient
        // provider variables below do not.
        //
        // For a custom provider, a binding that names an unset (or empty)
        // variable is a broken credential contract, not a keyless route: fail
        // loudly with the route-scoped fix instead of silently degrading to
        // the self-hosted loopback keyless fallback below (#5104). Without
        // this, an `api_key_env` route on a loopback host dispatched
        // unauthenticated while the operator believed credentials were wired,
        // and the composer-side preflight recovery never saw an error.
        if provider == ApiProvider::Custom
            && let Some(env_name) = bound_provider_api_key_env_name(self, provider)
        {
            return match std::env::var(&env_name) {
                Ok(value) if !value.trim().is_empty() => Ok(value),
                _ => {
                    let route_name = self.provider.as_deref().unwrap_or("<name>");
                    Err(anyhow::anyhow!(
                        "Custom provider '{route_name}' API key not found: the route binds \
                         api_key_env = \"{env_name}\" but that environment variable is not set. \
                         Set {env_name} to your key, or remove api_key_env from \
                         [providers.{route_name}] to run the endpoint without credentials."
                    ))
                }
            };
        }
        if let Some(value) = provider_config_env_api_key(self, provider) {
            return Ok(value);
        }

        // 2. The dispatcher resolves this same provider slot before launching
        // the TUI. Standalone `codewhale-tui` launches must see the identical
        // durable credential. Auto-detection is file-backed and prompt-free by
        // default; the OS keyring is queried only when the user explicitly
        // selects the system backend.
        if !self.should_skip_secret_store_for_provider(provider)
            && let Some(value) = provider_secret_store_api_key_with_mode(self, provider, read_only)
        {
            return Ok(value);
        }

        // 3. Ambient provider environment variables are scoped to official
        // endpoints. Never send an official-provider export to a custom host.
        if !self.should_skip_secret_store_for_provider(provider)
            && provider == ApiProvider::XiaomiMimo
        {
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
        if !self.should_skip_secret_store_for_provider(provider)
            && let Some(value) = provider_env_api_key(provider)
        {
            return Ok(value);
        }

        // Official DeepSeek Harness credentials, only after explicit
        // read-only consent to one exact `$DSH_HOME/.credentials.yaml`.
        if matches!(
            provider,
            ApiProvider::Deepseek | ApiProvider::DeepseekAnthropic
        ) && !custom_endpoint
        {
            let path = codewhale_config::default_dsh_credentials_path();
            if let Ok(grant) = self.external_credential_read_grant(
                provider,
                codewhale_config::ExternalCredentialSource::DshCli,
                &path,
            ) && let Some(value) = crate::dsh_credentials::deepseek_api_key_from_grant(&grant)?
            {
                return Ok(value);
            }
        }

        // Official Antigravity (`agy`) login. `ANTIGRAVITY_API_KEY` config
        // and env slots were already checked above; here the process's own
        // `AGY_ADC_AUTH` wins over the consented `state.vscdb`, which is
        // imported read-only from the one pinned path. The token is then
        // used on the cloud-code stream; it is never logged.
        if provider == ApiProvider::Antigravity && !custom_endpoint {
            let grant = self
                .external_credential_read_grant(
                    provider,
                    codewhale_config::ExternalCredentialSource::AgyCli,
                    &codewhale_config::default_agy_credentials_path(),
                )
                .ok();
            let process_env: std::collections::HashMap<String, String> = std::env::vars().collect();
            match crate::agy_credentials::antigravity_credential_precedence(
                None,
                &process_env,
                grant.as_ref(),
            ) {
                crate::agy_credentials::AntigravityCredential::ProcessEnv(token) => {
                    return Ok(token);
                }
                crate::agy_credentials::AntigravityCredential::ExternalFile(token) => {
                    return Ok(token);
                }
                other => {
                    tracing::debug!(
                        target: "config",
                        source = other.source_label(),
                        "antigravity credential plane did not yield a sendable token"
                    );
                }
            }
        }

        if !auth_mode_requires_api_key(auth_mode.as_deref())
            && (provider_route_is_keyless_self_hosted(provider, &self.deepseek_base_url())
                || base_url_uses_local_host(&self.deepseek_base_url()))
        {
            return Ok(String::new());
        }

        if custom_endpoint {
            let route_name = self
                .provider
                .as_deref()
                .unwrap_or_else(|| provider.as_str());
            anyhow::bail!(
                "Custom endpoint credentials for {route_name} must be bound explicitly. Ambient provider credentials are not sent to {}. Add api_key or api_key_env to this provider route, or pass --api-key with --base-url.",
                self.deepseek_base_url()
            );
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
                   • api_key = \"<your-key>\"  in ~/.codewhale/config.toml\n\
                   • already configured DeepSeek Harness? grant read-only access:\n\
                        codewhale auth external-consent --provider deepseek --mode read-only"
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
            ApiProvider::Moonshot => {
                let credential_help =
                    credential_help_for_provider_route(provider, &self.deepseek_base_url());
                if moonshot_base_url_is_exact_kimi_code(&self.deepseek_base_url()) {
                    anyhow::bail!(
                        "Kimi Code membership-plan API key not found. Get a plan key: {}. This route uses api.kimi.com/coding/v1 and does not import Kimi CLI credentials. Run 'codewhale auth set --provider moonshot', set {}, or add [{}] api_key.",
                        credential_help
                            .credential_url
                            .unwrap_or(KIMI_CODE_MEMBERSHIP_PLAN_CONSOLE_URL),
                        provider.env_vars_label(),
                        provider_config_table_name(provider)?
                    );
                }
                anyhow::bail!(
                    "Moonshot/Kimi API key not found. Get a key: {}. Run 'codewhale auth set --provider moonshot', \
                     set {}, or add [{}] api_key. \
                     For a Kimi Code plan key, set [providers.moonshot] base_url = \
                     \"https://api.kimi.com/coding/v1\" and model = \"kimi-for-coding\".",
                    credential_help
                        .credential_url
                        .unwrap_or("https://platform.kimi.ai/console/api-keys"),
                    provider.env_vars_label(),
                    provider_config_table_name(provider)?
                );
            }
            ApiProvider::Anthropic | ApiProvider::Openmodel => {
                anyhow::bail!("{}", missing_provider_api_key_message(provider)?)
            }
            ApiProvider::OpencodeZen => {
                anyhow::bail!("{}", missing_provider_api_key_message(provider)?)
            }
            ApiProvider::OpenaiCodex => anyhow::bail!("{}", crate::oauth::missing_auth_message()),
            ApiProvider::Xai => {
                // Prefer OAuth guidance when auth_mode requests it or Grok CLI
                // tokens already exist; otherwise show both API-key and OAuth.
                if self
                    .provider_config_for(provider)
                    .is_some_and(provider_config_uses_xai_oauth)
                    || crate::xai_oauth::credentials_present(self)
                {
                    anyhow::bail!("{}", crate::xai_oauth::missing_auth_message());
                }
                anyhow::bail!(
                    "xAI API key not found. Get a key: https://console.x.ai/\n\
                     Run 'codewhale auth set --provider xai', set XAI_API_KEY, or add \
                     [providers.xai] api_key.\n\
                     OAuth alternative: run `codewhale auth xai-device` for \
                     Codewhale-owned storage and set [providers.xai] auth_mode = \"oauth\"."
                );
            }
            // Self-hosted deployments commonly run without auth on localhost.
            // Return an empty key and let the client omit the Authorization header.
            ApiProvider::Sglang | ApiProvider::Vllm => Ok(String::new()),
            ApiProvider::Ollama
                if provider_route_is_keyless_self_hosted(provider, &self.deepseek_base_url()) =>
            {
                Ok(String::new())
            }
            ApiProvider::Ollama => {
                let help = credential_help_for_provider_route(provider, &self.deepseek_base_url());
                anyhow::bail!(
                    "Ollama Cloud API key not found. Get a key: {}. Run 'codewhale auth set --provider ollama', set OLLAMA_API_KEY, or add [providers.ollama] api_key in ~/.codewhale/config.toml.",
                    help.credential_url
                        .unwrap_or(codewhale_config::provider::OLLAMA_CLOUD_API_KEY_URL)
                )
            }
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
        let configured = self.mcp_config_path.as_deref().map(expand_path);
        match configured {
            Some(path) if path.is_absolute() => path,
            Some(path) => {
                tracing::warn!(
                    configured_path = %path.display(),
                    "relative mcp_config_path is not stable across launch directories; using the user-global MCP config"
                );
                default_mcp_config_path().unwrap_or_else(|| PathBuf::from("./mcp.json"))
            }
            None => default_mcp_config_path().unwrap_or_else(|| PathBuf::from("./mcp.json")),
        }
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
        let legacy_path = self
            .memory_path
            .as_deref()
            .map(expand_path)
            .or_else(default_memory_path)
            .unwrap_or_else(|| PathBuf::from("./memory.md"));
        if self.memory_backend() == MemoryBackend::Native {
            // The configured value is historically a *legacy single-file*
            // path (`$CODEWHALE_HOME/memory.md`), and the native store lives
            // beside it. Deriving from the parent is therefore right for the
            // default and for anyone still carrying the old setting.
            //
            // But someone who points `memory_path` at a native store — the
            // obvious reading of the name — used to get a second one nested
            // inside it (`…/memory/global/memory/global/MEMORY.md`), silently
            // writing somewhere other than the file they named. Honour an
            // already-native path as itself.
            if crate::native_memory::NativeMemoryStore::from_global_path(&legacy_path).is_some() {
                return legacy_path;
            }
            return legacy_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("memory")
                .join("global")
                .join("MEMORY.md");
        }
        legacy_path
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
        if let Some(backend) = self.memory.as_ref().and_then(|memory| memory.backend) {
            return backend != MemoryBackend::Off;
        }
        self.memory
            .as_ref()
            .and_then(|m| m.enabled)
            .unwrap_or(false)
    }

    /// Effective safety backstop on automatic goal continuation passes
    /// (#5052). Goals are unlimited by default. `[goal] max_continuations`
    /// opts into a ceiling; `0` disables it so only terminal status or user
    /// control stops an operate-mode goal run.
    #[must_use]
    pub fn goal_max_continuations(&self) -> u32 {
        self.goal
            .as_ref()
            .and_then(|goal| goal.max_continuations)
            .unwrap_or(crate::goal_loop::DEFAULT_MAX_GOAL_CONTINUATIONS)
    }

    /// Quiet period between successful interactive goal turns (#5508).
    /// Absent/zero keeps the existing immediate-continuation behavior.
    #[must_use]
    pub fn goal_continuation_delay_seconds(&self) -> u64 {
        self.goal
            .as_ref()
            .and_then(|goal| goal.continuation_delay_seconds)
            .unwrap_or(0)
            .min(crate::goal_loop::MAX_GOAL_CONTINUATION_DELAY_SECONDS)
    }

    /// Maximum number of automatic re-requests when the model returns only
    /// reasoning without any answer or tool call. Defaults to 2.
    /// Set via `[reasoning_only] max_reprompts` in config.toml.
    #[must_use]
    pub fn reasoning_only_max_reprompts(&self) -> u32 {
        self.reasoning_only
            .as_ref()
            .and_then(|cfg| cfg.max_reprompts)
            .unwrap_or(DEFAULT_REASONING_ONLY_REPROMPTS)
    }

    /// Optional custom message sent to the model on each re-request when the
    /// model returns only reasoning without any answer or tool call.
    /// Set via `[reasoning_only] reprompt_message` in config.toml.
    /// When unset, the built-in default is used.
    #[must_use]
    pub fn reasoning_only_reprompt_message(&self) -> &str {
        self.reasoning_only
            .as_ref()
            .and_then(|cfg| cfg.reprompt_message.as_deref())
            .unwrap_or(DEFAULT_REASONING_ONLY_REPROMPT_MESSAGE)
    }

    /// Resolve the explicit local-memory backend.
    #[must_use]
    pub fn memory_backend(&self) -> MemoryBackend {
        self.memory
            .as_ref()
            .and_then(|memory| memory.backend)
            .unwrap_or_else(|| {
                let Some(memory) = self.memory.as_ref() else {
                    return MemoryBackend::Off;
                };
                if memory.enabled.unwrap_or(false) {
                    MemoryBackend::Native
                } else {
                    MemoryBackend::Off
                }
            })
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
        self.context.project_pack.unwrap_or(false)
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

    /// Default per-child model-turn budget from `[subagents]
    /// default_max_steps`, applied when an `agent` start carries no explicit
    /// `max_steps` (#5324). `None` or `0` mean unbounded; a positive value is
    /// clamped to the runtime ceiling when applied.
    #[must_use]
    pub fn subagent_default_max_steps(&self) -> Option<u32> {
        self.subagents
            .as_ref()
            .and_then(|cfg| cfg.default_max_steps)
            .filter(|steps| *steps > 0)
    }

    /// Default per-child wall-clock budget in seconds from `[subagents]
    /// default_wall_time_secs`, applied when an `agent` start carries no
    /// explicit `wall_time_secs` (#5324). `None` or `0` keep the 1800s
    /// default; the resolved value is clamped to 1..=86400 when applied.
    #[must_use]
    pub fn subagent_default_wall_time_secs(&self) -> Option<u64> {
        self.subagents
            .as_ref()
            .and_then(|cfg| cfg.default_wall_time_secs)
            .filter(|secs| *secs > 0)
    }

    /// Resolved per-step DeepSeek API timeout for sub-agents, in seconds.
    ///
    /// Reads `[subagents] api_timeout_secs` and clamps to
    /// `[MIN_SUBAGENT_API_TIMEOUT_SECS, MAX_SUBAGENT_API_TIMEOUT_SECS]`
    /// (1..=3600). `None` or `0` resolve to
    /// `DEFAULT_SUBAGENT_API_TIMEOUT_SECS` (600); explicit `1` is honored,
    /// useful only in fast fail-fast tests, not production (#1806, #1808).
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
    /// configured long model request is not pre-empted by heartbeat cleanup,
    /// and at least 30 seconds above the sub-agent tool timeout so a single
    /// long tool execution is not cancelled as "no progress" (2026-08-04
    /// sub-agent hunt, finding 4).
    #[must_use]
    pub fn subagent_heartbeat_timeout_secs(&self) -> u64 {
        resolve_subagent_heartbeat_timeout_secs(
            self.subagents
                .as_ref()
                .and_then(|cfg| cfg.heartbeat_timeout_secs),
            self.subagent_api_timeout_secs(),
            DEFAULT_SUBAGENT_TOOL_TIMEOUT_SECS,
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
            DEFAULT_SUBAGENT_TOOL_TIMEOUT_SECS,
        )
    }

    /// Resolved per-SSE-chunk idle timeout in seconds.
    ///
    /// Reads `[tui].stream_chunk_timeout_secs`, falling back to the
    /// `CODEWHALE_STREAM_IDLE_TIMEOUT_SECS` env var (legacy alias:
    /// `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS`) when the config key is
    /// omitted. `None` or `0` resolve to the default 900 seconds; explicit
    /// values are clamped to `1..=3600`.
    #[must_use]
    pub fn stream_chunk_timeout_secs(&self) -> u64 {
        let raw = self
            .tui
            .as_ref()
            .and_then(|cfg| cfg.stream_chunk_timeout_secs)
            .or_else(|| {
                std::env::var(STREAM_CHUNK_TIMEOUT_ENV)
                    .or_else(|_| std::env::var(LEGACY_STREAM_CHUNK_TIMEOUT_ENV))
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
            })
            .unwrap_or(DEFAULT_STREAM_CHUNK_TIMEOUT_SECS);
        if raw == 0 {
            return DEFAULT_STREAM_CHUNK_TIMEOUT_SECS;
        }
        raw.clamp(MIN_STREAM_CHUNK_TIMEOUT_SECS, MAX_STREAM_CHUNK_TIMEOUT_SECS)
    }

    /// R1: resolved ceiling on model steps in a single turn.
    ///
    /// Reads `[tui].max_model_steps`, falling back to the
    /// `CODEWHALE_MAX_MODEL_STEPS` env var, then to the finite default.
    /// `0` — from either source — is treated as an invalid value and
    /// resolves to the default; it never means "unlimited".
    #[must_use]
    pub fn max_model_steps(&self) -> u32 {
        let raw = self
            .tui
            .as_ref()
            .and_then(|cfg| cfg.max_model_steps)
            .or_else(|| {
                std::env::var(MAX_MODEL_STEPS_ENV)
                    .ok()
                    .and_then(|value| value.trim().parse::<u32>().ok())
            });
        crate::core::engine::turn_budget::resolve_max_model_steps(raw)
    }

    /// R1: resolved cumulative per-turn wall-clock budget.
    ///
    /// Reads `[tui].turn_wall_clock_secs`, falling back to the
    /// `CODEWHALE_TURN_WALL_CLOCK_SECS` env var, then to the finite
    /// default. `0` resolves to the default; it never means "unlimited".
    #[must_use]
    pub fn turn_wall_clock(&self) -> std::time::Duration {
        let raw = self
            .tui
            .as_ref()
            .and_then(|cfg| cfg.turn_wall_clock_secs)
            .or_else(|| {
                std::env::var(TURN_WALL_CLOCK_ENV)
                    .ok()
                    .and_then(|value| value.trim().parse::<u64>().ok())
            });
        crate::core::engine::turn_budget::resolve_turn_wall_clock(raw)
    }

    /// R1: resolved per-step cap on accumulated streamed content, in bytes.
    #[must_use]
    pub fn stream_max_content_bytes(&self) -> usize {
        crate::core::engine::turn_budget::resolve_stream_max_content_bytes(
            self.tui.as_ref().and_then(|cfg| cfg.stream_max_content_mb),
        )
    }

    /// R1: resolved per-step cap on a single stream's wall-clock duration.
    #[must_use]
    pub fn stream_max_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(
            crate::core::engine::turn_budget::resolve_stream_max_duration_secs(
                self.tui
                    .as_ref()
                    .and_then(|cfg| cfg.stream_max_duration_secs),
            ),
        )
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
        insert("scout", &cfg.explorer_model);
        insert("explorer", &cfg.explorer_model);
        insert("explore", &cfg.explorer_model);
        insert("planner", &cfg.awaiter_model);
        insert("awaiter", &cfg.awaiter_model);
        insert("plan", &cfg.awaiter_model);
        insert("reviewer", &cfg.review_model);
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

    /// Parsed `[fleet]` table, or defaults when the table is absent
    /// (#fleet-roster cutover (v0.8.67)).
    #[must_use]
    pub fn fleet_config(&self) -> codewhale_config::FleetConfigToml {
        self.fleet.clone().unwrap_or_default()
    }

    /// The person's fleet as models (design MODEL-ROUTING-CATALOG §10 F1):
    /// every exact provider + model route in the selected fleet for
    /// `workspace`, with the roles each fills. `Ok(empty)` = the session
    /// model only; `Err` = a selected fleet that cannot be read. This is the
    /// seam the operator-awareness slice (F2) reads.
    pub fn fleet_members(
        &self,
        workspace: &Path,
    ) -> Result<Vec<crate::fleet::members::FleetModel>, crate::fleet::store::FleetStoreError> {
        crate::fleet::members::fleet_models(workspace)
    }

    /// Parsed `[workflow]` table, or product defaults when the table is absent
    /// (#4128 / Section 2.11). Automatic launch, approval, isolation, and
    /// activity-persistence consumers should read through this accessor so
    /// omitted keys share one model.
    #[must_use]
    pub fn workflow_config(&self) -> codewhale_config::WorkflowConfigToml {
        self.workflow.clone().unwrap_or_default()
    }

    /// Return the configured DeepSeek reasoning-effort tier, if any.
    #[must_use]
    pub fn reasoning_effort(&self) -> Option<&str> {
        self.reasoning_effort.as_deref()
    }

    pub(crate) fn reasoning_effort_is_explicit(&self) -> bool {
        self.reasoning_effort.is_some() && !self.reasoning_effort_inferred_from_legacy_alias
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

    /// Resolve which approval option a fresh card highlights (#5293).
    #[must_use]
    pub fn approval_default_selection(&self) -> ApprovalDefaultSelection {
        self.approval.unwrap_or_default().default_selection
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

/// Controls whether configuration loading may copy secret-bearing environment
/// values into the in-memory configuration.
///
/// Structural diagnostics intentionally retain safe environment routing and
/// policy fields while refusing values that could be secrets when later
/// rendered or included in an error path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigEnvironmentPolicy {
    Runtime,
    StructuralDiagnostic,
}

impl ConfigEnvironmentPolicy {
    const fn permits_secret_bearing_values(self) -> bool {
        matches!(self, Self::Runtime)
    }
}

fn root_deepseek_model_is_foreign_to_direct_provider(provider: ApiProvider, model: &str) -> bool {
    if matches!(
        provider,
        ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic
    ) || provider_passes_model_through(provider)
    {
        return false;
    }
    if matches!(
        provider,
        ApiProvider::NvidiaNim
            | ApiProvider::Openrouter
            | ApiProvider::Orcarouter
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
            | ApiProvider::OpencodeGo
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
mod home;
mod paths;
use paths::{
    canonicalize_or_keep, codewhale_home_dir, default_config_path, default_managed_config_path,
    default_mcp_config_path, default_memory_path, default_notes_path, default_requirements_path,
    default_skills_dir, env_config_path, expand_pathbuf, home_config_path, try_default_config_path,
    workspace_config_key,
};
pub(crate) use paths::{effective_home_dir, expand_path};

pub(crate) fn workspace_trust_config_candidate_paths() -> Vec<PathBuf> {
    #[cfg(test)]
    {
        if !crate::test_support::guarded_environment_provides_state_paths() {
            return vec![
                crate::test_support::unsealed_test_state_root()
                    .join(codewhale_config::CONFIG_FILE_NAME),
            ];
        }
    }

    match env_config_path() {
        Ok(Some(path)) => return vec![path],
        Ok(None) => {}
        Err(error) => {
            tracing::error!(
                error = %error,
                "invalid config path override; refusing workspace-trust fallback"
            );
            return Vec::new();
        }
    }

    match codewhale_home_dir() {
        Ok(Some(codewhale_home)) => return vec![codewhale_home.join("config.toml")],
        Ok(None) => {}
        Err(error) => {
            tracing::error!(
                error = %error,
                "invalid Codewhale home override; refusing workspace-trust fallback"
            );
            return Vec::new();
        }
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
    let config_path = match default_config_path() {
        Ok(path) => path,
        Err(error) => {
            tracing::error!(
                error = %error,
                "failed to resolve workspace-trust config; treating workspace as untrusted"
            );
            return false;
        }
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
    let config_path =
        try_default_config_path().context("Failed to resolve config path for workspace trust.")?;
    ensure_parent_dir(&config_path)?;

    let project_key = workspace_config_key(workspace);
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        crate::config_persistence::set_document_value(
            doc,
            &["projects", project_key.as_str(), "trust_level"],
            "trusted",
        )
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    Ok(config_path)
}

fn workspace_trust_level_from_doc<'a>(doc: &'a toml::Value, workspace: &Path) -> Option<&'a str> {
    let workspace = canonicalize_or_keep(workspace);
    // Trust records may sit at the top level or — from the historic
    // extras-nesting write bug (healed on mutation since 2026-07-23) — under
    // one or more literal `extras` tables. Read tolerantly so a not-yet-
    // healed config file still recognizes its trusted workspaces.
    let mut scope = Some(doc);
    while let Some(current) = scope {
        if let Some(projects) = current.get("projects").and_then(toml::Value::as_table) {
            for (raw_path, project) in projects {
                let project_path = canonicalize_or_keep(&expand_path(raw_path));
                if project_path == workspace {
                    return project.get("trust_level").and_then(toml::Value::as_str);
                }
            }
        }
        scope = current.get("extras");
    }
    None
}

fn is_trusted_level(level: &str) -> bool {
    level.trim().eq_ignore_ascii_case("trusted")
}

pub(crate) fn resolve_load_config_path(path: Option<PathBuf>) -> Result<Option<PathBuf>> {
    if let Some(path) = path {
        return Ok(Some(expand_pathbuf(path)));
    }

    try_default_config_path().map(Some)
}

/// Create an inspectable config file on first interactive launch.
///
/// The file intentionally omits `api_key`; onboarding or `codewhale auth set`
/// writes that field after the user supplies a key.
pub fn ensure_config_file_exists(path: Option<PathBuf>) -> Result<Option<PathBuf>> {
    let config_path = match path {
        Some(path) => expand_pathbuf(path),
        None => default_config_path().context("Failed to resolve config path.")?,
    };
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
# check_interval_hours = 1
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

fn first_nonempty_env(names: &[&str]) -> Option<String> {
    let read = || {
        names.iter().find_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
    };
    #[cfg(test)]
    {
        crate::test_support::with_test_env_lock(read)
    }
    #[cfg(not(test))]
    {
        read()
    }
}

/// Return the provider-scoped endpoint override that `apply_env_overrides`
/// will apply to the active route. This is intentionally kept beside the
/// mutation code: after the write, a provider-table `base_url` no longer
/// carries enough information to distinguish a file-owned route from an
/// environment-selected host.
fn provider_env_base_url_override(provider: ApiProvider) -> Option<String> {
    let names: &[&str] = match provider {
        ApiProvider::NvidiaNim => &["NVIDIA_NIM_BASE_URL", "NIM_BASE_URL", "NVIDIA_BASE_URL"],
        ApiProvider::Openai => &["OPENAI_BASE_URL"],
        ApiProvider::Atlascloud => &["ATLASCLOUD_BASE_URL"],
        ApiProvider::Openrouter => &["OPENROUTER_BASE_URL"],
        ApiProvider::Orcarouter => &["ORCAROUTER_BASE_URL"],
        ApiProvider::XiaomiMimo => &["XIAOMI_MIMO_BASE_URL", "MIMO_BASE_URL"],
        ApiProvider::WanjieArk => &[
            "WANJIE_ARK_BASE_URL",
            "WANJIE_BASE_URL",
            "WANJIE_MAAS_BASE_URL",
        ],
        ApiProvider::Volcengine => &[
            "VOLCENGINE_BASE_URL",
            "VOLCENGINE_ARK_BASE_URL",
            "ARK_BASE_URL",
        ],
        ApiProvider::Novita => &["NOVITA_BASE_URL"],
        ApiProvider::Fireworks => &["FIREWORKS_BASE_URL"],
        ApiProvider::Siliconflow | ApiProvider::SiliconflowCn => &["SILICONFLOW_BASE_URL"],
        ApiProvider::Arcee => &["ARCEE_BASE_URL"],
        ApiProvider::Moonshot => &["MOONSHOT_BASE_URL", "KIMI_BASE_URL"],
        ApiProvider::Sglang => &["SGLANG_BASE_URL"],
        ApiProvider::Vllm => &["VLLM_BASE_URL"],
        ApiProvider::Ollama => &["OLLAMA_BASE_URL"],
        ApiProvider::OllamaCloud => &["OLLAMA_CLOUD_BASE_URL"],
        ApiProvider::Huggingface => &["HUGGINGFACE_BASE_URL", "HF_BASE_URL"],
        ApiProvider::Meta => &["META_MODEL_API_BASE_URL", "MODEL_API_BASE_URL"],
        ApiProvider::Xai => &["XAI_BASE_URL"],
        ApiProvider::Mistral => &["MISTRAL_BASE_URL"],
        ApiProvider::Google => &["GOOGLE_BASE_URL", "GEMINI_BASE_URL"],
        ApiProvider::Antigravity => &["ANTIGRAVITY_BASE_URL"],
        ApiProvider::Telecomjs => &["TELECOMJS_BASE_URL"],
        ApiProvider::Edenai => &["EDENAI_BASE_URL"],
        ApiProvider::Concentrate => &["CONCENTRATE_BASE_URL"],
        ApiProvider::ModelstudioTokenPlan | ApiProvider::ModelstudioTokenPlanAnthropic => {
            &["MODELSTUDIO_TOKEN_PLAN_BASE_URL"]
        }
        ApiProvider::ModelstudioCodingPlan | ApiProvider::ModelstudioCodingPlanAnthropic => {
            &["MODELSTUDIO_CODING_PLAN_BASE_URL"]
        }
        ApiProvider::OpencodeGo => &["OPENCODE_GO_BASE_URL"],
        ApiProvider::OpencodeZen => &["OPENCODE_ZEN_BASE_URL"],
        ApiProvider::Deepseek
        | ApiProvider::DeepseekCN
        | ApiProvider::DeepseekAnthropic
        | ApiProvider::Anthropic
        | ApiProvider::Openmodel
        | ApiProvider::Deepinfra
        | ApiProvider::Together
        | ApiProvider::Qianfan
        | ApiProvider::OpenaiCodex
        | ApiProvider::Zai
        | ApiProvider::Stepfun
        | ApiProvider::Minimax
        | ApiProvider::MinimaxAnthropic
        | ApiProvider::Sakana
        | ApiProvider::LongCat
        | ApiProvider::Custom => &[],
    };
    first_nonempty_env(names)
}

/// Resolve an env var, preferring the `CODEWHALE_*` form over the
/// legacy `DEEPSEEK_*` form. Empty values are ignored so a blank shell export
/// does not erase configured provider settings.
fn codewhale_env_var(
    codewhale_name: &str,
    legacy_name: &str,
) -> Result<String, std::env::VarError> {
    let read = || {
        std::env::var(codewhale_name)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                std::env::var(legacy_name)
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            })
            .ok_or(std::env::VarError::NotPresent)
    };
    #[cfg(test)]
    {
        crate::test_support::with_test_env_lock(read)
    }
    #[cfg(not(test))]
    {
        read()
    }
}

fn apply_env_overrides(config: &mut Config, policy: ConfigEnvironmentPolicy) {
    #[cfg(test)]
    {
        crate::test_support::with_test_env_lock(|| {
            apply_env_overrides_unlocked(config, policy);
        })
    }
    #[cfg(not(test))]
    {
        apply_env_overrides_unlocked(config, policy);
    }
}

fn apply_env_overrides_unlocked(config: &mut Config, policy: ConfigEnvironmentPolicy) {
    if let Ok(value) = codewhale_env_var("CODEWHALE_PROVIDER", "DEEPSEEK_PROVIDER") {
        config.provider = Some(value);
    }
    let active_base_url_from_env = env_base_url_override().is_some()
        || provider_env_base_url_override(config.api_provider()).is_some()
        || (config.selects_legacy_ollama_cloud_route()
            && first_nonempty_env(&["OLLAMA_BASE_URL"]).is_some());
    if let Ok(value) = codewhale_env_var("CODEWHALE_BASE_URL", "DEEPSEEK_BASE_URL") {
        match config.api_provider() {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => {
                // DeepSeek and DeepSeek-CN share this one legacy root field.
                // Record which of them the environment addressed so the
                // sibling identity cannot inherit the value, while a
                // file-owned root (no owner recorded) stays shared.
                config.base_url = Some(value);
                // Resolve the owner *after* the write: the root value is one
                // of the inputs `api_provider()` sniffs, so the effective
                // identity is the post-write one, matching the receipt
                // recorded at the end of this function.
                let owner = config.api_provider();
                config.root_base_url_owner =
                    BaseUrlEnvReceipt::Route(owner, config.provider_identity_for(owner));
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
            ApiProvider::Orcarouter => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .orcarouter
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
            ApiProvider::OllamaCloud => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .ollama_cloud
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
            ApiProvider::MinimaxAnthropic => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .minimax_anthropic
                    .base_url = Some(value);
            }
            ApiProvider::Sakana => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .sakana
                    .base_url = Some(value);
            }
            ApiProvider::LongCat => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .longcat
                    .base_url = Some(value);
            }
            ApiProvider::OpencodeGo => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .opencode_go
                    .base_url = Some(value);
            }
            ApiProvider::OpencodeZen => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .opencode_zen
                    .base_url = Some(value);
            }
            ApiProvider::Meta => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .meta
                    .base_url = Some(value);
            }
            ApiProvider::Xai => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .xai
                    .base_url = Some(value);
            }
            ApiProvider::Mistral => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .mistral
                    .base_url = Some(value);
            }
            ApiProvider::Google => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .google
                    .base_url = Some(value);
            }
            ApiProvider::Antigravity => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .antigravity
                    .base_url = Some(value);
            }
            ApiProvider::Telecomjs => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .telecomjs
                    .base_url = Some(value);
            }
            ApiProvider::Edenai => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .edenai
                    .base_url = Some(value);
            }
            ApiProvider::Concentrate => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .concentrate
                    .base_url = Some(value);
            }
            ApiProvider::ModelstudioTokenPlan => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .modelstudio_token_plan
                    .base_url = Some(value);
            }
            ApiProvider::ModelstudioTokenPlanAnthropic => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .modelstudio_token_plan_anthropic
                    .base_url = Some(value);
            }
            ApiProvider::ModelstudioCodingPlan => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .modelstudio_coding_plan
                    .base_url = Some(value);
            }
            ApiProvider::ModelstudioCodingPlanAnthropic => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .modelstudio_coding_plan_anthropic
                    .base_url = Some(value);
            }
            // Custom resolves to the named `[providers.<name>]` table; route the
            // override through the exact route while retaining the released
            // root-literal custom storage shape (#1519, #4334).
            ApiProvider::Custom => {
                config.set_provider_base_url_override(ApiProvider::Custom, Some(value));
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
    if matches!(config.api_provider(), ApiProvider::Meta)
        && let Ok(value) = std::env::var("META_MODEL_API_BASE_URL")
            .or_else(|_| std::env::var("MODEL_API_BASE_URL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .meta
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Xai)
        && let Ok(value) = std::env::var("XAI_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .xai
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Mistral)
        && let Ok(value) = std::env::var("MISTRAL_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .mistral
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Telecomjs)
        && let Ok(value) = std::env::var("TELECOMJS_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .telecomjs
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Edenai)
        && let Ok(value) = std::env::var("EDENAI_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .edenai
            .base_url = Some(value);
    }
    // Concentrate has no inline block here on purpose: CONCENTRATE_BASE_URL
    // is already served by `provider_env_base_url_override`, which the route
    // resolver consults — a second inline assignment was a duplicate.
    if matches!(
        config.api_provider(),
        ApiProvider::ModelstudioTokenPlan | ApiProvider::ModelstudioTokenPlanAnthropic
    ) && let Ok(value) = std::env::var("MODELSTUDIO_TOKEN_PLAN_BASE_URL")
        && !value.trim().is_empty()
    {
        let field = if config.api_provider() == ApiProvider::ModelstudioTokenPlanAnthropic {
            &mut config
                .providers
                .get_or_insert_with(ProvidersConfig::default)
                .modelstudio_token_plan_anthropic
                .base_url
        } else {
            &mut config
                .providers
                .get_or_insert_with(ProvidersConfig::default)
                .modelstudio_token_plan
                .base_url
        };
        *field = Some(value);
    }
    if matches!(
        config.api_provider(),
        ApiProvider::ModelstudioCodingPlan | ApiProvider::ModelstudioCodingPlanAnthropic
    ) && let Ok(value) = std::env::var("MODELSTUDIO_CODING_PLAN_BASE_URL")
        && !value.trim().is_empty()
    {
        let field = if config.api_provider() == ApiProvider::ModelstudioCodingPlanAnthropic {
            &mut config
                .providers
                .get_or_insert_with(ProvidersConfig::default)
                .modelstudio_coding_plan_anthropic
                .base_url
        } else {
            &mut config
                .providers
                .get_or_insert_with(ProvidersConfig::default)
                .modelstudio_coding_plan
                .base_url
        };
        *field = Some(value);
    }
    if policy.permits_secret_bearing_values()
        && let Ok(value) = std::env::var("CODEWHALE_HTTP_HEADERS")
            .or_else(|_| std::env::var("DEEPSEEK_HTTP_HEADERS"))
        && let Ok(headers) = parse_http_headers(&value)
        && !headers.is_empty()
    {
        let mut root_headers = config.http_headers.clone().unwrap_or_default();
        root_headers.extend(headers.clone());
        config.http_headers = Some(root_headers);

        let provider = config.api_provider();
        // Root headers are the canonical header slot for a released literal
        // custom route. Creating `[providers.custom]` here would make the route
        // ambiguous and disconnect its root endpoint, model, and credential.
        if !(provider == ApiProvider::Custom && config.uses_legacy_literal_custom_route()) {
            // Capture the custom entry key (the selected provider name) before
            // the mutable borrow of `providers` below (#1519).
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
                ApiProvider::Orcarouter => &mut providers.orcarouter,
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
                ApiProvider::OllamaCloud => &mut providers.ollama_cloud,
                ApiProvider::Volcengine => &mut providers.volcengine,
                ApiProvider::Huggingface => &mut providers.huggingface,
                ApiProvider::Deepinfra => &mut providers.deepinfra,
                ApiProvider::Together => &mut providers.together,
                ApiProvider::Qianfan => &mut providers.qianfan,
                ApiProvider::OpenaiCodex => &mut providers.openai_codex,
                ApiProvider::Anthropic => &mut providers.anthropic,
                ApiProvider::Openmodel => &mut providers.openmodel,
                ApiProvider::Zai => &mut providers.zai,
                ApiProvider::Stepfun => &mut providers.stepfun,
                ApiProvider::Minimax => &mut providers.minimax,
                ApiProvider::MinimaxAnthropic => &mut providers.minimax_anthropic,
                ApiProvider::Sakana => &mut providers.sakana,
                ApiProvider::LongCat => &mut providers.longcat,
                ApiProvider::OpencodeGo => &mut providers.opencode_go,
                ApiProvider::OpencodeZen => &mut providers.opencode_zen,
                ApiProvider::Meta => &mut providers.meta,
                ApiProvider::Xai => &mut providers.xai,
                ApiProvider::Mistral => &mut providers.mistral,
                ApiProvider::Google => &mut providers.google,
                ApiProvider::Antigravity => &mut providers.antigravity,
                ApiProvider::Telecomjs => &mut providers.telecomjs,
                ApiProvider::Edenai => &mut providers.edenai,
                ApiProvider::Concentrate => &mut providers.concentrate,
                ApiProvider::ModelstudioTokenPlan => &mut providers.modelstudio_token_plan,
                ApiProvider::ModelstudioTokenPlanAnthropic => {
                    &mut providers.modelstudio_token_plan_anthropic
                }
                ApiProvider::ModelstudioCodingPlan => &mut providers.modelstudio_coding_plan,
                ApiProvider::ModelstudioCodingPlanAnthropic => {
                    &mut providers.modelstudio_coding_plan_anthropic
                }
                ApiProvider::Custom => providers
                    .custom
                    .entry(custom_key.unwrap_or_else(|| "__custom__".to_string()))
                    .or_default(),
            };
            let mut provider_headers = entry.http_headers.clone().unwrap_or_default();
            provider_headers.extend(headers);
            entry.http_headers = Some(provider_headers);
        }
    }
    if config.provider.as_deref().and_then(ApiProvider::parse) == Some(ApiProvider::Ollama)
        && let Ok(value) = std::env::var("OLLAMA_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .ollama
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::OllamaCloud)
        && config.provider.as_deref().and_then(ApiProvider::parse) == Some(ApiProvider::OllamaCloud)
        && let Ok(value) = std::env::var("OLLAMA_CLOUD_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .ollama_cloud
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
    if matches!(
        config.api_provider(),
        ApiProvider::Ollama | ApiProvider::OllamaCloud
    ) && let Ok(value) = std::env::var("OLLAMA_MODEL")
    {
        config.default_text_model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::OllamaCloud)
        && let Ok(value) = std::env::var("OLLAMA_CLOUD_MODEL")
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
    if matches!(config.api_provider(), ApiProvider::Meta)
        && let Ok(value) =
            std::env::var("META_MODEL_API_MODEL").or_else(|_| std::env::var("MODEL_API_MODEL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .meta
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Xai)
        && let Ok(value) = std::env::var("XAI_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .xai
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Mistral)
        && let Ok(value) = std::env::var("MISTRAL_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .mistral
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::OpencodeGo)
        && let Ok(value) = std::env::var("OPENCODE_GO_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .opencode_go
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Telecomjs)
        && let Ok(value) = std::env::var("TELECOMJS_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .telecomjs
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Concentrate)
        && let Ok(value) = std::env::var("CONCENTRATE_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .concentrate
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Edenai)
        && let Ok(value) = std::env::var("EDENAI_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .edenai
            .model = Some(value);
    }
    if matches!(
        config.api_provider(),
        ApiProvider::ModelstudioTokenPlan | ApiProvider::ModelstudioTokenPlanAnthropic
    ) && let Ok(value) = std::env::var("MODELSTUDIO_TOKEN_PLAN_MODEL")
        && !value.trim().is_empty()
    {
        let field = if config.api_provider() == ApiProvider::ModelstudioTokenPlanAnthropic {
            &mut config
                .providers
                .get_or_insert_with(ProvidersConfig::default)
                .modelstudio_token_plan_anthropic
                .model
        } else {
            &mut config
                .providers
                .get_or_insert_with(ProvidersConfig::default)
                .modelstudio_token_plan
                .model
        };
        *field = Some(value);
    }
    if matches!(
        config.api_provider(),
        ApiProvider::ModelstudioCodingPlan | ApiProvider::ModelstudioCodingPlanAnthropic
    ) && let Ok(value) = std::env::var("MODELSTUDIO_CODING_PLAN_MODEL")
        && !value.trim().is_empty()
    {
        let field = if config.api_provider() == ApiProvider::ModelstudioCodingPlanAnthropic {
            &mut config
                .providers
                .get_or_insert_with(ProvidersConfig::default)
                .modelstudio_coding_plan_anthropic
                .model
        } else {
            &mut config
                .providers
                .get_or_insert_with(ProvidersConfig::default)
                .modelstudio_coding_plan
                .model
        };
        *field = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::OpencodeZen)
        && let Ok(value) = std::env::var("OPENCODE_ZEN_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .opencode_zen
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
        if (provider == ApiProvider::Custom && config.uses_legacy_literal_custom_route())
            || matches!(
                provider,
                ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic
            )
        {
            config.default_text_model = Some(value);
        } else {
            // Capture the custom entry key before the mutable borrow below (#1519).
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
                ApiProvider::Deepseek
                | ApiProvider::DeepseekCN
                | ApiProvider::DeepseekAnthropic => unreachable!(
                    "DeepSeek providers are handled in the if branch above (issue #1714)"
                ),
                ApiProvider::Custom => providers
                    .custom
                    .entry(custom_key.unwrap_or_else(|| "__custom__".to_string()))
                    .or_default(),
                ApiProvider::NvidiaNim => &mut providers.nvidia_nim,
                ApiProvider::Openai => &mut providers.openai,
                ApiProvider::Atlascloud => &mut providers.atlascloud,
                ApiProvider::WanjieArk => &mut providers.wanjie_ark,
                ApiProvider::Openrouter => &mut providers.openrouter,
                ApiProvider::Orcarouter => &mut providers.orcarouter,
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
                ApiProvider::OllamaCloud => &mut providers.ollama_cloud,
                ApiProvider::Volcengine => &mut providers.volcengine,
                ApiProvider::Huggingface => &mut providers.huggingface,
                ApiProvider::Deepinfra => &mut providers.deepinfra,
                ApiProvider::Together => &mut providers.together,
                ApiProvider::Qianfan => &mut providers.qianfan,
                ApiProvider::OpenaiCodex => &mut providers.openai_codex,
                ApiProvider::Anthropic => &mut providers.anthropic,
                ApiProvider::Openmodel => &mut providers.openmodel,
                ApiProvider::Zai => &mut providers.zai,
                ApiProvider::Stepfun => &mut providers.stepfun,
                ApiProvider::Minimax => &mut providers.minimax,
                ApiProvider::MinimaxAnthropic => &mut providers.minimax_anthropic,
                ApiProvider::Sakana => &mut providers.sakana,
                ApiProvider::LongCat => &mut providers.longcat,
                ApiProvider::OpencodeGo => &mut providers.opencode_go,
                ApiProvider::OpencodeZen => &mut providers.opencode_zen,
                ApiProvider::Meta => &mut providers.meta,
                ApiProvider::Xai => &mut providers.xai,
                ApiProvider::Mistral => &mut providers.mistral,
                ApiProvider::Google => &mut providers.google,
                ApiProvider::Antigravity => &mut providers.antigravity,
                ApiProvider::Telecomjs => &mut providers.telecomjs,
                ApiProvider::Edenai => &mut providers.edenai,
                ApiProvider::Concentrate => &mut providers.concentrate,
                ApiProvider::ModelstudioTokenPlan => &mut providers.modelstudio_token_plan,
                ApiProvider::ModelstudioTokenPlanAnthropic => {
                    &mut providers.modelstudio_token_plan_anthropic
                }
                ApiProvider::ModelstudioCodingPlan => &mut providers.modelstudio_coding_plan,
                ApiProvider::ModelstudioCodingPlanAnthropic => {
                    &mut providers.modelstudio_coding_plan_anthropic
                }
            };
            entry.model = Some(value);
        }
    }
    if matches!(config.api_provider(), ApiProvider::NvidiaNim)
        && let Ok(value) = std::env::var("NVIDIA_NIM_MODEL")
    {
        config.default_text_model = Some(value);
    }
    if let Ok(value) =
        std::env::var("CODEWHALE_SKILLS_DIR").or_else(|_| std::env::var("DEEPSEEK_SKILLS_DIR"))
    {
        config.skills_dir = Some(value);
    }
    if let Ok(value) =
        std::env::var("CODEWHALE_MCP_CONFIG").or_else(|_| std::env::var("DEEPSEEK_MCP_CONFIG"))
    {
        config.mcp_config_path = Some(value);
    }
    if let Ok(value) =
        std::env::var("CODEWHALE_NOTES_PATH").or_else(|_| std::env::var("DEEPSEEK_NOTES_PATH"))
    {
        config.notes_path = Some(value);
    }
    if let Ok(value) =
        std::env::var("CODEWHALE_MEMORY_PATH").or_else(|_| std::env::var("DEEPSEEK_MEMORY_PATH"))
    {
        config.memory_path = Some(value);
    }
    if let Ok(value) =
        std::env::var("CODEWHALE_MEMORY").or_else(|_| std::env::var("DEEPSEEK_MEMORY"))
    {
        let on = matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "on" | "true" | "yes" | "y" | "enabled"
        );
        config
            .memory
            .get_or_insert_with(MemoryConfig::default)
            .enabled = Some(on);
    }
    if let Ok(value) =
        std::env::var("CODEWHALE_ALLOW_SHELL").or_else(|_| std::env::var("DEEPSEEK_ALLOW_SHELL"))
    {
        config.allow_shell = Some(value == "1" || value.eq_ignore_ascii_case("true"));
    }
    if let Ok(value) = std::env::var("CODEWHALE_APPROVAL_POLICY")
        .or_else(|_| std::env::var("DEEPSEEK_APPROVAL_POLICY"))
    {
        config.approval_policy = Some(value);
    }
    if let Ok(value) =
        std::env::var("CODEWHALE_SANDBOX_MODE").or_else(|_| std::env::var("DEEPSEEK_SANDBOX_MODE"))
    {
        config.sandbox_mode = Some(value);
    }
    if let Ok(value) = std::env::var("CODEWHALE_SANDBOX_NETWORK_ACCESS")
        .or_else(|_| std::env::var("DEEPSEEK_SANDBOX_NETWORK_ACCESS"))
    {
        config.sandbox_network_access = Some(value == "1" || value.eq_ignore_ascii_case("true"));
    }
    if let Ok(value) = std::env::var("CODEWHALE_PROJECT_INSTRUCTION_IMPORTS") {
        config.project_instruction_imports = value
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect();
    }
    // `DEEPSEEK_YOLO` is a read-only deprecated alias of `CODEWHALE_YOLO`
    // (removable in 0.10 per issue #5443); `CODEWHALE_YOLO` wins when both
    // are set.
    if let Ok(value) = std::env::var("CODEWHALE_YOLO").or_else(|_| std::env::var("DEEPSEEK_YOLO")) {
        config.yolo = Some(value == "1" || value.eq_ignore_ascii_case("true"));
    }
    if let Ok(value) =
        std::env::var("CODEWHALE_VERBOSITY").or_else(|_| std::env::var("DEEPSEEK_VERBOSITY"))
    {
        config.verbosity = Some(value);
    }
    if let Ok(value) = std::env::var("CODEWHALE_SANDBOX_BACKEND")
        .or_else(|_| std::env::var("DEEPSEEK_SANDBOX_BACKEND"))
    {
        config.sandbox_backend = Some(value);
    }
    if let Ok(value) = codewhale_env_var("CODEWHALE_PREFER_BWRAP", "DEEPSEEK_PREFER_BWRAP") {
        let primary_is_set = std::env::var("CODEWHALE_PREFER_BWRAP")
            .ok()
            .is_some_and(|value| !value.trim().is_empty());
        let legacy_is_set = std::env::var("DEEPSEEK_PREFER_BWRAP")
            .ok()
            .is_some_and(|value| !value.trim().is_empty());
        if !primary_is_set && legacy_is_set {
            tracing::warn!(
                "DEEPSEEK_PREFER_BWRAP is deprecated; use CODEWHALE_PREFER_BWRAP (the legacy alias is removed in 0.10.0)"
            );
        }
        config.prefer_bwrap = Some(value == "1" || value.eq_ignore_ascii_case("true"));
    }
    if let Ok(value) =
        std::env::var("CODEWHALE_SANDBOX_URL").or_else(|_| std::env::var("DEEPSEEK_SANDBOX_URL"))
    {
        config.sandbox_url = Some(value);
    }
    if policy.permits_secret_bearing_values()
        && let Ok(value) = std::env::var("CODEWHALE_SANDBOX_API_KEY")
            .or_else(|_| std::env::var("DEEPSEEK_SANDBOX_API_KEY"))
    {
        config.sandbox_api_key = Some(value);
    }
    if let Ok(value) = std::env::var("CODEWHALE_MANAGED_CONFIG_PATH")
        .or_else(|_| std::env::var("DEEPSEEK_MANAGED_CONFIG_PATH"))
    {
        config.managed_config_path = Some(value);
    }
    if policy.permits_secret_bearing_values()
        && let Ok(value) = std::env::var("CODEWHALE_SEARCH_API_KEY")
            .or_else(|_| std::env::var("DEEPSEEK_SEARCH_API_KEY"))
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
    if let Ok(value) = std::env::var("CODEWHALE_REQUIREMENTS_PATH")
        .or_else(|_| std::env::var("DEEPSEEK_REQUIREMENTS_PATH"))
    {
        config.requirements_path = Some(value);
    }
    if let Ok(value) = std::env::var("CODEWHALE_MAX_SUBAGENTS")
        .or_else(|_| std::env::var("DEEPSEEK_MAX_SUBAGENTS"))
        && let Ok(parsed) = value.parse::<usize>()
    {
        config.max_subagents = Some(parsed.clamp(1, MAX_SUBAGENTS));
    }
    // Always leave a receipt: "the environment layer ran and nobody owns the
    // base URL" is a different, stronger statement than "no receipt", and only
    // the explicit form stops a pinned cross-provider child from treating the
    // ambient generic host as a global fallback.
    config.base_url_env_receipt = if active_base_url_from_env {
        let provider = config.api_provider();
        BaseUrlEnvReceipt::Route(provider, config.provider_identity_for(provider))
    } else {
        BaseUrlEnvReceipt::NoOwner
    };
}

fn normalize_model_config(config: &mut Config) {
    let provider = config.api_provider();
    let base_url = config.deepseek_base_url();
    config.migrated_deepseek_model_alias = if matches!(
        provider,
        ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic
    ) {
        config
            .active_configured_model_id()
            .map(str::to_ascii_lowercase)
            .filter(|model| deepseek_alias_deprecation(model).is_some())
            .filter(|model| {
                wire_model_for_provider_route(provider, &base_url, model) != model.as_str()
            })
    } else {
        None
    };

    // Preserve the behavioral half of DeepSeek's retired aliases while
    // migrating their model id to V4 Flash. An explicit reasoning setting is
    // authoritative; this compatibility default only fills an omitted value.
    // Custom endpoints retain both their model id and their own semantics.
    if config.reasoning_effort.is_none() {
        let alias_effort = config
            .migrated_deepseek_model_alias
            .as_deref()
            .and_then(legacy_deepseek_alias_reasoning_effort);
        if let Some(effort) = alias_effort {
            config.reasoning_effort = Some(effort.to_string());
            config.reasoning_effort_inferred_from_legacy_alias = true;
        }
    }

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
        if let Some(model) = providers.deepseek_anthropic.model.as_deref()
            && !provider_entry_uses_custom_base_url(
                ApiProvider::DeepseekAnthropic,
                &providers.deepseek_anthropic,
            )
            && let Some(normalized) =
                normalize_model_for_provider(ApiProvider::DeepseekAnthropic, model)
        {
            providers.deepseek_anthropic.model = Some(normalized);
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

#[cfg(test)]
pub(crate) fn normalize_model_config_for_test(config: &mut Config) {
    normalize_model_config(config);
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
            | ApiProvider::OllamaCloud
            | ApiProvider::Huggingface
            | ApiProvider::Meta
            | ApiProvider::Xai
            | ApiProvider::Telecomjs
            | ApiProvider::Edenai
            // Concentrate ids are gateway-owned (plain, `provider/model`, or the
            // gateway's own `auto`); the resolver strips only `concentrate/`.
            | ApiProvider::Concentrate
            | ApiProvider::ModelstudioTokenPlan
            | ApiProvider::ModelstudioTokenPlanAnthropic
            | ApiProvider::ModelstudioCodingPlan
            | ApiProvider::ModelstudioCodingPlanAnthropic
            // Custom OpenAI-compatible endpoints preserve user-supplied model
            // ids verbatim (#1519); never normalize/rewrite them.
            | ApiProvider::Custom
    )
}

/// Whether a provider identity key is the historical literal `custom`.
fn identity_is_literal_custom(identity: &str) -> bool {
    identity
        .trim()
        .eq_ignore_ascii_case(ApiProvider::Custom.as_str())
}

fn provider_entry_uses_custom_base_url(provider: ApiProvider, entry: &ProviderConfig) -> bool {
    entry
        .base_url
        .as_deref()
        .is_some_and(|base_url| provider_preserves_custom_base_url_model(provider, base_url))
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

fn wire_config_prefers_anthropic(wire: Option<&str>) -> bool {
    let Some(raw) = wire.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let normalized = raw.to_ascii_lowercase().replace(['_', ' '], "-");
    matches!(
        normalized.as_str(),
        "anthropic"
            | "anthropic-messages"
            | "messages"
            | "claude"
            | "anthropic-compatible"
            | "anthropic-compat"
    )
}

fn wire_config_prefers_responses(wire: Option<&str>) -> bool {
    let Some(raw) = wire.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let normalized = raw.to_ascii_lowercase().replace(['_', ' '], "-");
    matches!(
        normalized.as_str(),
        "responses"
            | "responses-api"
            | "openai-responses"
            | "openai-responses-api"
            | "response"
            | "response-api"
            | "openai-responses-compat"
            | "responses-compat"
    )
}

fn modelstudio_mode_is_coding_plan(provider: ApiProvider, mode: Option<&str>) -> bool {
    if matches!(
        provider,
        ApiProvider::ModelstudioCodingPlan | ApiProvider::ModelstudioCodingPlanAnthropic
    ) {
        return true;
    }
    let Some(raw) = mode.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let normalized = raw.to_ascii_lowercase().replace(['_', ' '], "-");
    matches!(
        normalized.as_str(),
        "coding-plan" | "coding" | "codingplan" | "dashscope-coding" | "code"
    )
}

fn resolve_modelstudio_base_url_for_tui(
    configured: Option<String>,
    provider: ApiProvider,
    mode: Option<&str>,
    wire: Option<&str>,
) -> String {
    if let Some(url) = configured.filter(|value| !value.trim().is_empty()) {
        return url;
    }
    let coding = modelstudio_mode_is_coding_plan(provider, mode);
    let anthropic = matches!(
        provider,
        ApiProvider::ModelstudioTokenPlanAnthropic | ApiProvider::ModelstudioCodingPlanAnthropic
    ) || wire_config_prefers_anthropic(wire);
    match (coding, anthropic) {
        (true, true) => MODELSTUDIO_CODING_PLAN_ANTHROPIC_BASE_URL.to_string(),
        (true, false) => DEFAULT_MODELSTUDIO_CODING_PLAN_BASE_URL.to_string(),
        (false, true) => MODELSTUDIO_TOKEN_PLAN_ANTHROPIC_BASE_URL.to_string(),
        (false, false) => DEFAULT_MODELSTUDIO_TOKEN_PLAN_BASE_URL.to_string(),
    }
}

fn resolve_minimax_base_url_for_tui(
    configured: Option<String>,
    provider: ApiProvider,
    wire: Option<&str>,
) -> String {
    if let Some(url) = configured.filter(|value| !value.trim().is_empty()) {
        return url;
    }
    if matches!(provider, ApiProvider::MinimaxAnthropic) || wire_config_prefers_anthropic(wire) {
        DEFAULT_MINIMAX_ANTHROPIC_BASE_URL.to_string()
    } else {
        DEFAULT_MINIMAX_BASE_URL.to_string()
    }
}

fn resolve_deepseek_base_url_for_tui(
    configured: Option<String>,
    provider: ApiProvider,
    wire: Option<&str>,
) -> String {
    if let Some(url) = configured.filter(|value| !value.trim().is_empty()) {
        return url;
    }
    if matches!(provider, ApiProvider::DeepseekAnthropic) || wire_config_prefers_anthropic(wire) {
        DEFAULT_DEEPSEEK_ANTHROPIC_BASE_URL.to_string()
    } else {
        DEFAULT_DEEPSEEK_BASE_URL.to_string()
    }
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
    let kind = provider
        .kind()
        .unwrap_or(codewhale_config::ProviderKind::Deepseek);
    codewhale_config::provider_preserves_custom_base_url_model(kind, base_url)
}

/// Whether this concrete route is a self-hosted endpoint whose credentials
/// are optional by default.
///
/// Ollama is local; the released exact `ollama` + `https://ollama.com/v1`
/// tuple is upgraded to `OllamaCloud` before this helper runs. Cloud is never
/// self-hosted, while neighboring remote Ollama URLs remain custom and are
/// rejected before they can inherit ambient or saved credentials.
pub(crate) fn provider_route_is_keyless_self_hosted(provider: ApiProvider, base_url: &str) -> bool {
    if provider == ApiProvider::Ollama {
        return base_url_uses_local_host(base_url);
    }
    provider.is_self_hosted()
}

fn provider_preserves_custom_base_url_model(provider: ApiProvider, base_url: &str) -> bool {
    base_url_is_custom_for_provider(provider, base_url)
}

fn moonshot_base_url_uses_kimi_code(base_url: &str) -> bool {
    let normalized = normalize_base_url(base_url).to_ascii_lowercase();
    normalized == DEFAULT_KIMI_CODE_BASE_URL
        || normalized == "https://api.kimi.com/coding"
        || normalized.starts_with("https://api.kimi.com/coding/")
}

/// The Kimi Code API endpoint, normalized only for insignificant trailing
/// slashes. This must stay stricter than `moonshot_base_url_uses_kimi_code`:
/// route-specific K3 capability and request shaping are not safe for arbitrary
/// Kimi-hosted paths.
pub(crate) fn moonshot_base_url_is_exact_kimi_code(base_url: &str) -> bool {
    codewhale_config::provider::is_exact_kimi_code_route(
        codewhale_config::ProviderKind::Moonshot,
        base_url,
    )
}

/// The exact Moonshot direct-API endpoint, normalized only for an
/// insignificant trailing slash. Custom gateways must retain their own wire
/// contract even when they expose a `kimi-k3` model id.
pub(crate) fn moonshot_base_url_is_exact_direct_platform(base_url: &str) -> bool {
    codewhale_config::provider::is_exact_moonshot_platform_route(
        codewhale_config::ProviderKind::Moonshot,
        base_url,
    )
}

/// Whether a route is exactly Moonshot's direct pay-as-you-go K3 route.
pub(crate) fn is_exact_direct_moonshot_k3_route(
    provider: ApiProvider,
    base_url: &str,
    model: &str,
) -> bool {
    provider == ApiProvider::Moonshot
        && moonshot_base_url_is_exact_direct_platform(base_url)
        && model.trim().eq_ignore_ascii_case(MOONSHOT_KIMI_K3_MODEL)
}

/// Whether a route is exactly xAI's first-party Grok 4.6 endpoint.
#[must_use]
pub(crate) fn is_exact_xai_grok_4_6_route(
    provider: ApiProvider,
    base_url: &str,
    model: &str,
) -> bool {
    provider == ApiProvider::Xai
        && codewhale_config::provider::is_exact_xai_platform_route(
            codewhale_config::ProviderKind::Xai,
            base_url,
        )
        && model.trim().eq_ignore_ascii_case(XAI_GROK_4_6_MODEL)
}

/// Whether a route uses either official Kimi Code K3 membership model.
pub(crate) fn is_exact_kimi_code_k3_route(
    provider: ApiProvider,
    base_url: &str,
    model: &str,
) -> bool {
    provider == ApiProvider::Moonshot
        && moonshot_base_url_is_exact_kimi_code(base_url)
        && [KIMI_CODE_K3_MODEL, KIMI_CODE_K3_256K_MODEL]
            .iter()
            .any(|id| model.trim().eq_ignore_ascii_case(id))
}

/// Whether a route uses plan-tier-dependent bare `k3`.
///
/// Keep entitlement handling separate from `k3-256k`, whose window is fixed.
#[must_use]
pub(crate) fn is_exact_kimi_code_bare_k3_route(
    provider: ApiProvider,
    base_url: &str,
    model: &str,
) -> bool {
    provider == ApiProvider::Moonshot
        && moonshot_base_url_is_exact_kimi_code(base_url)
        && model.trim().eq_ignore_ascii_case(KIMI_CODE_K3_MODEL)
}

/// Whether a route is one of Z.ai's exact first-party Chat endpoints.
#[must_use]
pub(crate) fn is_exact_zai_chat_route(provider: ApiProvider, base_url: &str) -> bool {
    provider == ApiProvider::Zai
        && codewhale_config::provider::is_exact_zai_chat_route(
            codewhale_config::ProviderKind::Zai,
            base_url,
        )
}

/// Whether a route is an exact first-party Z.ai model that exposes **tiered**
/// reasoning effort (`reasoning_effort: high | max`) rather than only the
/// generic thinking toggle.
///
/// GLM-5.2 is the verified member. GLM-5.3 and GLM-5.3-Flash inherit it
/// because their catalog rows inherit GLM-5.2's `reasoning_options`
/// wholesale. If Z.ai publishes different reasoning controls, this
/// predicate is where they split.
#[must_use]
pub(crate) fn is_exact_zai_tiered_effort_route(
    provider: ApiProvider,
    base_url: &str,
    model: &str,
) -> bool {
    is_exact_zai_chat_route(provider, base_url)
        && (model.trim().eq_ignore_ascii_case(ZAI_GLM_5_2_MODEL)
            || model.trim().eq_ignore_ascii_case(ZAI_GLM_5_3_MODEL)
            || model.trim().eq_ignore_ascii_case(ZAI_GLM_5_3_FLASH_MODEL))
}

/// Whether a route is exactly first-party Z.ai GLM-5-Turbo.
#[must_use]
pub(crate) fn is_exact_zai_glm_5_turbo_route(
    provider: ApiProvider,
    base_url: &str,
    model: &str,
) -> bool {
    is_exact_zai_chat_route(provider, base_url)
        && model.trim().eq_ignore_ascii_case(ZAI_GLM_5_TURBO_MODEL)
}

/// Whether a route is an exact first-party Z.ai model with a verified
/// reasoning control. GLM-5.2, GLM-5.3, and GLM-5.3-Flash have tiered
/// effort; GLM-5.1 and GLM-5-Turbo only expose the generic thinking toggle.
#[must_use]
pub(crate) fn is_exact_known_zai_reasoning_route(
    provider: ApiProvider,
    base_url: &str,
    model: &str,
) -> bool {
    is_exact_zai_tiered_effort_route(provider, base_url, model)
        || is_exact_zai_glm_5_turbo_route(provider, base_url, model)
        || (is_exact_zai_chat_route(provider, base_url)
            && model.trim().eq_ignore_ascii_case(ZAI_GLM_5_1_MODEL))
}

/// MiniMax's own hosted routes, for both wire dialects.
///
/// Kept as a pure string predicate so a dispatch receipt can be judged without
/// a `Config`, and shared with billing classification so a MiniMax-compatible
/// gateway cannot inherit the first-party PAYG/Token Plan duality. Both the
/// `.io` and `.com` hosts are first-party; anything else is a gateway.
#[must_use]
pub(crate) fn minimax_base_url_is_supported_direct(base_url: &str) -> bool {
    codewhale_config::provider::is_exact_minimax_chat_route(
        codewhale_config::ProviderKind::Minimax,
        base_url,
    ) || codewhale_config::provider::is_exact_minimax_anthropic_route(
        codewhale_config::ProviderKind::MinimaxAnthropic,
        base_url,
    )
}

/// Whether a route is exactly MiniMax-M3 on the first-party OpenAI-compatible
/// Chat API. Compatible gateways and the Anthropic Messages route retain
/// their own token-limit dialects.
#[must_use]
pub(crate) fn is_exact_minimax_m3_route(
    provider: ApiProvider,
    base_url: &str,
    model: &str,
) -> bool {
    provider == ApiProvider::Minimax
        && codewhale_config::provider::is_exact_minimax_chat_route(
            codewhale_config::ProviderKind::Minimax,
            base_url,
        )
        && model.trim().eq_ignore_ascii_case(DEFAULT_MINIMAX_MODEL)
}

/// Whether a route is exactly MiniMax-M3 on a first-party Anthropic-compatible
/// Messages endpoint. The wire supports adaptive/disabled thinking, but no
/// distinct effort tier.
#[must_use]
pub(crate) fn is_exact_minimax_anthropic_m3_route(
    provider: ApiProvider,
    base_url: &str,
    model: &str,
) -> bool {
    provider == ApiProvider::MinimaxAnthropic
        && codewhale_config::provider::is_exact_minimax_anthropic_route(
            codewhale_config::ProviderKind::MinimaxAnthropic,
            base_url,
        )
        && model.trim().eq_ignore_ascii_case(DEFAULT_MINIMAX_MODEL)
}

#[must_use]
pub(crate) fn minimax_m3_route_uses_max_completion_tokens(
    provider: ApiProvider,
    base_url: &str,
    model: &str,
) -> bool {
    is_exact_minimax_m3_route(provider, base_url, model)
}

/// The Kimi Code membership roster, as one fact.
///
/// The picker offers these ids, `validate_kimi_code_api_model_id` accepts them
/// on the membership endpoint and rejects them on the direct platform, and the
/// model picker labels them as plan routes. Those sites previously kept
/// independent literal lists and had already drifted (`kimi-for-coding` was
/// missing from the picker label), so the roster lives here and nowhere else.
pub(crate) const KIMI_CODE_MEMBERSHIP_MODELS: [&str; 4] = [
    KIMI_CODE_K3_MODEL,
    KIMI_CODE_K3_256K_MODEL,
    DEFAULT_KIMI_CODE_MODEL,
    KIMI_CODE_HIGHSPEED_MODEL,
];

/// Whether `model` is a Kimi Code membership model id.
///
/// The single membership-roster predicate. Callers that need to name the
/// product — output-ceiling provenance, picker rosters, setup validation, and
/// the model picker's route label — must use this rather than re-listing ids.
#[must_use]
pub(crate) fn is_kimi_code_membership_model(model: &str) -> bool {
    let model = model.trim();
    KIMI_CODE_MEMBERSHIP_MODELS
        .iter()
        .any(|id| model.eq_ignore_ascii_case(id))
}

/// The Moonshot direct-platform roster, as one fact. Mirror of
/// [`KIMI_CODE_MEMBERSHIP_MODELS`] for the pay-as-you-go product.
pub(crate) const MOONSHOT_DIRECT_PLATFORM_MODELS: [&str; 3] = [
    MOONSHOT_KIMI_K3_MODEL,
    DEFAULT_MOONSHOT_MODEL,
    MOONSHOT_KIMI_K2_6_MODEL,
];

pub(crate) const KIMI_CODE_CLAUDE_ALIAS_GUIDANCE: &str = "Kimi Code model `k3[1m]` is a Claude Code environment convention, not an API model id. Use model = \"k3\". If your Kimi Code plan includes 1M context, also set context_window = 1048576; otherwise keep the 262144 safe default.";

#[derive(Debug, thiserror::Error)]
pub(crate) enum SafeConfigDiagnostic {
    #[error("{}", KIMI_CODE_CLAUDE_ALIAS_GUIDANCE)]
    KimiCodeClaudeAlias,
}

/// Fail closed on known-bad model/endpoint pairings (#4687).
///
/// Canonical endpoints reject `k3[1m]` and known membership/direct cross-pairings.
/// Unknown IDs and custom Moonshot-compatible gateways remain pass-through.
pub(crate) fn validate_kimi_code_api_model_id(
    provider: ApiProvider,
    base_url: &str,
    model: &str,
) -> std::result::Result<(), String> {
    if provider != ApiProvider::Moonshot {
        return Ok(());
    }
    let model = model.trim();
    if model.is_empty() {
        return Ok(());
    }

    if moonshot_base_url_is_exact_kimi_code(base_url) {
        if model.eq_ignore_ascii_case("k3[1m]") {
            return Err(KIMI_CODE_CLAUDE_ALIAS_GUIDANCE.to_string());
        }
        for direct_id in MOONSHOT_DIRECT_PLATFORM_MODELS {
            if model.eq_ignore_ascii_case(direct_id) {
                return Err(format!(
                    "Kimi Code membership route (api.kimi.com/coding/v1) does not accept model = \"{model}\": it is a direct Moonshot platform id. Use a Kimi Code membership model (\"k3\", \"k3-256k\", \"kimi-for-coding\", or \"kimi-for-coding-highspeed\") for this base_url. Direct Moonshot pay-as-you-go uses base_url = \"https://api.moonshot.ai/v1\" with model = \"{direct_id}\"."
                ));
            }
        }
        return Ok(());
    }

    if moonshot_base_url_is_exact_direct_platform(base_url) {
        for membership_id in KIMI_CODE_MEMBERSHIP_MODELS {
            if model.eq_ignore_ascii_case(membership_id) {
                return Err(format!(
                    "Moonshot direct route (api.moonshot.ai/v1) does not accept model = \"{model}\": it is a Kimi Code membership model id, not a direct-platform catalog model. Kimi Code membership uses base_url = \"https://api.kimi.com/coding/v1\" with model = \"{membership_id}\"; direct Moonshot pay-as-you-go K3 uses model = \"kimi-k3\"."
                ));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod kimi_code_pairing_tests {
    use super::*;

    #[test]
    fn membership_roster_passes_on_kimi_code_endpoint() {
        for model in [
            KIMI_CODE_K3_MODEL,
            KIMI_CODE_K3_256K_MODEL,
            DEFAULT_KIMI_CODE_MODEL,
            KIMI_CODE_HIGHSPEED_MODEL,
        ] {
            assert!(
                validate_kimi_code_api_model_id(
                    ApiProvider::Moonshot,
                    DEFAULT_KIMI_CODE_BASE_URL,
                    model,
                )
                .is_ok(),
                "{model} must be accepted on the exact Kimi Code membership endpoint"
            );
        }
    }

    #[test]
    fn direct_platform_ids_fail_on_kimi_code_endpoint() {
        for model in [
            MOONSHOT_KIMI_K3_MODEL,
            DEFAULT_MOONSHOT_MODEL,
            MOONSHOT_KIMI_K2_6_MODEL,
        ] {
            let err = validate_kimi_code_api_model_id(
                ApiProvider::Moonshot,
                DEFAULT_KIMI_CODE_BASE_URL,
                model,
            )
            .expect_err("direct-platform ids are not Kimi Code membership roster models");
            assert!(err.contains(model), "{err}");
            assert!(err.contains("api.moonshot.ai/v1"), "{err}");
        }
    }

    #[test]
    fn membership_ids_fail_on_direct_moonshot_endpoint() {
        for model in [
            KIMI_CODE_K3_MODEL,
            KIMI_CODE_K3_256K_MODEL,
            DEFAULT_KIMI_CODE_MODEL,
            KIMI_CODE_HIGHSPEED_MODEL,
        ] {
            let err = validate_kimi_code_api_model_id(
                ApiProvider::Moonshot,
                DEFAULT_MOONSHOT_BASE_URL,
                model,
            )
            .expect_err("membership ids are not direct-platform catalog models");
            assert!(err.contains(model), "{err}");
            assert!(err.contains("api.kimi.com/coding/v1"), "{err}");
        }
    }

    #[test]
    fn canonical_pairs_pass_and_custom_gateways_are_untouched() {
        // Canonical pairs pass on both endpoints.
        for (base_url, model) in [
            (DEFAULT_KIMI_CODE_BASE_URL, KIMI_CODE_K3_MODEL),
            (DEFAULT_KIMI_CODE_BASE_URL, KIMI_CODE_K3_256K_MODEL),
            (DEFAULT_KIMI_CODE_BASE_URL, DEFAULT_KIMI_CODE_MODEL),
            (DEFAULT_KIMI_CODE_BASE_URL, KIMI_CODE_HIGHSPEED_MODEL),
            (DEFAULT_MOONSHOT_BASE_URL, MOONSHOT_KIMI_K3_MODEL),
            (DEFAULT_MOONSHOT_BASE_URL, DEFAULT_MOONSHOT_MODEL),
            (DEFAULT_MOONSHOT_BASE_URL, MOONSHOT_KIMI_K2_6_MODEL),
        ] {
            assert!(
                validate_kimi_code_api_model_id(ApiProvider::Moonshot, base_url, model).is_ok(),
                "{base_url} / {model}"
            );
        }
        // The pre-existing cross-pairings still fail closed.
        assert!(
            validate_kimi_code_api_model_id(
                ApiProvider::Moonshot,
                DEFAULT_KIMI_CODE_BASE_URL,
                MOONSHOT_KIMI_K3_MODEL,
            )
            .is_err()
        );
        assert!(
            validate_kimi_code_api_model_id(
                ApiProvider::Moonshot,
                DEFAULT_MOONSHOT_BASE_URL,
                KIMI_CODE_K3_MODEL,
            )
            .is_err()
        );
        // Custom gateways keep their own wire contract, membership ids
        // included: only the two canonical endpoints enforce pairings.
        for model in [
            KIMI_CODE_K3_MODEL,
            KIMI_CODE_K3_256K_MODEL,
            DEFAULT_KIMI_CODE_MODEL,
            KIMI_CODE_HIGHSPEED_MODEL,
            MOONSHOT_KIMI_K3_MODEL,
        ] {
            assert!(
                validate_kimi_code_api_model_id(
                    ApiProvider::Moonshot,
                    "https://proxy.example/v1",
                    model,
                )
                .is_ok(),
                "{model} on a custom gateway"
            );
        }
    }
}

/// Short route label for header/diagnostics without credentials (#4687).
pub(crate) fn moonshot_k3_route_display_name(base_url: &str, model: &str) -> Option<&'static str> {
    if is_exact_kimi_code_bare_k3_route(ApiProvider::Moonshot, base_url, model) {
        return Some("Kimi Code membership / k3");
    }
    if is_exact_kimi_code_k3_route(ApiProvider::Moonshot, base_url, model) {
        return Some("Kimi Code membership / k3-256k");
    }
    if is_exact_direct_moonshot_k3_route(ApiProvider::Moonshot, base_url, model) {
        return Some("Moonshot direct / kimi-k3");
    }
    None
}

/// Credential help for a concrete provider route.
///
/// `ProviderKind::Moonshot` intentionally retains its generic direct-API
/// metadata in `codewhale-config`: that remains correct for Moonshot's own
/// platform route. The Kimi Code membership endpoint is a distinct route and
/// must not send its users to the generic API console or imply CLI credential
/// import support.
pub(crate) fn credential_help_for_provider_route(
    provider: ApiProvider,
    base_url: &str,
) -> codewhale_config::provider::CredentialHelp {
    provider.kind().map_or_else(
        || provider.credential_help(),
        |kind| codewhale_config::provider::credential_help_for_route(kind, base_url),
    )
}

pub(crate) fn provider_config_uses_kimi_imported_token(config: &ProviderConfig) -> bool {
    config
        .auth_mode
        .as_deref()
        .is_some_and(auth_mode_uses_kimi_imported_token)
}

pub(crate) use codewhale_config::{
    auth_mode_disables_api_key, auth_mode_requires_api_key, auth_mode_uses_kimi_imported_token,
};

fn provider_config_uses_xai_oauth(config: &ProviderConfig) -> bool {
    config
        .auth_mode
        .as_deref()
        .is_some_and(crate::xai_oauth::auth_mode_uses_xai_oauth)
}

/// Whether a base URL points at a loopback/unspecified host, i.e. a local
/// runtime rather than a hosted endpoint. Shared by the active-provider
/// local-base-url check above and the `/provider` picker's custom-provider
/// auth-optionality heuristic (#3830).
pub(crate) fn base_url_uses_local_host(base_url: &str) -> bool {
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
        (ApiProvider::Together, "inkling" | "together-inkling" | "thinkingmachines/inkling") => {
            TOGETHER_INKLING_MODEL.to_string()
        }
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
    // Captured before the struct literal moves the field out of `override_cfg`.
    let override_defines_root_base_url = override_cfg.base_url.is_some();
    Config {
        provider: override_cfg.provider.or(base.provider),
        telemetry: override_cfg.telemetry.or(base.telemetry),
        api_key: override_cfg.api_key.or(base.api_key),
        base_url: override_cfg.base_url.or(base.base_url),
        http_headers: override_cfg.http_headers.or(base.http_headers),
        default_text_model: override_cfg.default_text_model.or(base.default_text_model),
        auth_mode: override_cfg.auth_mode.or(base.auth_mode),
        reasoning_effort: override_cfg.reasoning_effort.or(base.reasoning_effort),
        reasoning_effort_inferred_from_legacy_alias: override_cfg
            .reasoning_effort_inferred_from_legacy_alias
            || base.reasoning_effort_inferred_from_legacy_alias,
        fleet_operator_route_applied: override_cfg.fleet_operator_route_applied
            || base.fleet_operator_route_applied,
        fleet_operator_reasoning_applied: override_cfg.fleet_operator_reasoning_applied
            || base.fleet_operator_reasoning_applied,
        migrated_legacy_ollama_cloud_route: override_cfg.migrated_legacy_ollama_cloud_route
            || base.migrated_legacy_ollama_cloud_route,
        migrated_deepseek_model_alias: override_cfg
            .migrated_deepseek_model_alias
            .or(base.migrated_deepseek_model_alias),
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
        stop_words: override_cfg.stop_words.or(base.stop_words),
        allow_shell: override_cfg.allow_shell.or(base.allow_shell),
        prompt_suggestion: override_cfg.prompt_suggestion.or(base.prompt_suggestion),
        yolo: override_cfg.yolo.or(base.yolo),
        verbosity: override_cfg.verbosity.or(base.verbosity),
        approval_policy: override_cfg.approval_policy.or(base.approval_policy),
        sandbox_mode: override_cfg.sandbox_mode.or(base.sandbox_mode),
        sandbox_network_access: override_cfg
            .sandbox_network_access
            .or(base.sandbox_network_access),
        project_instruction_imports: if override_cfg.project_instruction_imports.is_empty() {
            base.project_instruction_imports
        } else {
            override_cfg.project_instruction_imports
        },
        fallback_providers: if override_cfg.fallback_providers.is_empty() {
            base.fallback_providers
        } else {
            override_cfg.fallback_providers
        },
        sandbox_backend: override_cfg.sandbox_backend.or(base.sandbox_backend),
        sandbox_url: override_cfg.sandbox_url.or(base.sandbox_url),
        sandbox_api_key: override_cfg.sandbox_api_key.or(base.sandbox_api_key),
        prefer_bwrap: override_cfg.prefer_bwrap.or(base.prefer_bwrap),
        bwrap_ro_roots: if override_cfg.bwrap_ro_roots.is_empty() {
            base.bwrap_ro_roots
        } else {
            override_cfg.bwrap_ro_roots
        },
        bwrap_dev_roots: if override_cfg.bwrap_dev_roots.is_empty() {
            base.bwrap_dev_roots
        } else {
            override_cfg.bwrap_dev_roots
        },
        sandbox_denied_read_paths: if override_cfg.sandbox_denied_read_paths.is_empty() {
            base.sandbox_denied_read_paths
        } else {
            override_cfg.sandbox_denied_read_paths
        },
        sandbox_read_denylist_defaults: override_cfg
            .sandbox_read_denylist_defaults
            .or(base.sandbox_read_denylist_defaults),
        sandbox_read_denylist_exempt: if override_cfg.sandbox_read_denylist_exempt.is_empty() {
            base.sandbox_read_denylist_exempt
        } else {
            override_cfg.sandbox_read_denylist_exempt
        },
        managed_config_path: override_cfg
            .managed_config_path
            .or(base.managed_config_path),
        requirements_path: override_cfg.requirements_path.or(base.requirements_path),
        max_subagents: override_cfg.max_subagents.or(base.max_subagents),
        retry: override_cfg.retry.or(base.retry),
        auto_review: override_cfg.auto_review.or(base.auto_review),
        tui: override_cfg.tui.or(base.tui),
        transcript: override_cfg.transcript.or(base.transcript),
        hooks: override_cfg.hooks.or(base.hooks),
        lifecycle_outbox: override_cfg.lifecycle_outbox.or(base.lifecycle_outbox),
        control_socket: override_cfg.control_socket.or(base.control_socket),
        providers: merge_providers(base.providers, override_cfg.providers),
        features: merge_features(base.features, override_cfg.features),
        notifications: override_cfg.notifications.or(base.notifications),
        approval: override_cfg.approval.or(base.approval),
        network: override_cfg.network.or(base.network),
        verifier: override_cfg.verifier.or(base.verifier),
        advisor: override_cfg.advisor.or(base.advisor),
        skills: merge_skills_config(base.skills, override_cfg.skills),
        snapshots: override_cfg.snapshots.or(base.snapshots),
        search: override_cfg.search.or(base.search),
        goal: override_cfg.goal.or(base.goal),
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
        workflow: override_cfg.workflow.or(base.workflow),
        subagents: override_cfg.subagents.or(base.subagents),
        strict_tool_mode: override_cfg.strict_tool_mode.or(base.strict_tool_mode),
        runtime_api: override_cfg.runtime_api.or(base.runtime_api),
        workshop: override_cfg.workshop.or(base.workshop),
        exec_policy_engine: override_cfg.exec_policy_engine,
        base_url_env_receipt: match override_cfg.base_url_env_receipt {
            BaseUrlEnvReceipt::Unrecorded => base.base_url_env_receipt,
            recorded => recorded,
        },
        // A layer that supplies its own root `base_url` replaces the
        // environment's write, so that layer's ownership wins outright.
        root_base_url_owner: if override_defines_root_base_url {
            override_cfg.root_base_url_owner
        } else {
            match override_cfg.root_base_url_owner {
                BaseUrlEnvReceipt::Unrecorded => base.root_base_url_owner,
                recorded => recorded,
            }
        },
        runtime_chat_isolated: override_cfg.runtime_chat_isolated || base.runtime_chat_isolated,
        runtime_thread_inference_unrelated: override_cfg.runtime_thread_inference_unrelated
            || base.runtime_thread_inference_unrelated,
        mini_window: override_cfg.mini_window.or(base.mini_window),
        reasoning_only: override_cfg.reasoning_only.or(base.reasoning_only),
        title: override_cfg.title.or(base.title),
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
    let permissions: codewhale_config::PermissionsToml = toml::from_str(&raw).map_err(|_| {
        anyhow::anyhow!(
            "Failed to parse permissions file {}; file contents were omitted",
            codewhale_config::quote_os_path(&permissions_path)
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
        wire: override_cfg.wire.or(base.wire),
        auth_mode: override_cfg.auth_mode.or(base.auth_mode),
        oauth_credential_generation: override_cfg
            .oauth_credential_generation
            .or(base.oauth_credential_generation),
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
        external_credentials: override_cfg
            .external_credentials
            .or(base.external_credentials),
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
            orcarouter: merge_provider_config(base.orcarouter, override_cfg.orcarouter),
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
            ollama_cloud: merge_provider_config(base.ollama_cloud, override_cfg.ollama_cloud),
            volcengine: merge_provider_config(base.volcengine, override_cfg.volcengine),
            huggingface: merge_provider_config(base.huggingface, override_cfg.huggingface),
            deepinfra: merge_provider_config(base.deepinfra, override_cfg.deepinfra),
            together: merge_provider_config(base.together, override_cfg.together),
            qianfan: merge_provider_config(base.qianfan, override_cfg.qianfan),
            openai_codex: merge_provider_config(base.openai_codex, override_cfg.openai_codex),
            zai: merge_provider_config(base.zai, override_cfg.zai),
            stepfun: merge_provider_config(base.stepfun, override_cfg.stepfun),
            minimax: merge_provider_config(base.minimax, override_cfg.minimax),
            minimax_anthropic: merge_provider_config(
                base.minimax_anthropic,
                override_cfg.minimax_anthropic,
            ),
            sakana: merge_provider_config(base.sakana, override_cfg.sakana),
            longcat: merge_provider_config(base.longcat, override_cfg.longcat),
            opencode_go: merge_provider_config(base.opencode_go, override_cfg.opencode_go),
            opencode_zen: merge_provider_config(base.opencode_zen, override_cfg.opencode_zen),
            meta: merge_provider_config(base.meta, override_cfg.meta),
            xai: merge_provider_config(base.xai, override_cfg.xai),
            mistral: merge_provider_config(base.mistral, override_cfg.mistral),
            google: merge_provider_config(base.google, override_cfg.google),
            antigravity: merge_provider_config(base.antigravity, override_cfg.antigravity),
            telecomjs: merge_provider_config(base.telecomjs, override_cfg.telecomjs),
            edenai: merge_provider_config(base.edenai, override_cfg.edenai),
            concentrate: merge_provider_config(base.concentrate, override_cfg.concentrate),
            modelstudio_token_plan: merge_provider_config(
                base.modelstudio_token_plan,
                override_cfg.modelstudio_token_plan,
            ),
            modelstudio_token_plan_anthropic: merge_provider_config(
                base.modelstudio_token_plan_anthropic,
                override_cfg.modelstudio_token_plan_anthropic,
            ),
            modelstudio_coding_plan: merge_provider_config(
                base.modelstudio_coding_plan,
                override_cfg.modelstudio_coding_plan,
            ),
            modelstudio_coding_plan_anthropic: merge_provider_config(
                base.modelstudio_coding_plan_anthropic,
                override_cfg.modelstudio_coding_plan_anthropic,
            ),
            custom: merge_custom_providers(base.custom, override_cfg.custom),
        }),
    }
}

fn load_single_config_file(path: &Path) -> Result<Config> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;
    let parsed: ConfigFile = toml::from_str(&contents).map_err(|_| {
        anyhow::anyhow!(
            "Failed to parse config file {}; file contents were omitted",
            codewhale_config::quote_os_path(path)
        )
    })?;
    Ok(parsed.base)
}

/// Build a one-line warning when top-level-only keys are nested under a section
/// Codewhale does not define (`[general]` / `[sandbox]`). TOML silently drops
/// those keys, so e.g. `[general]\nallow_shell = true` never takes effect and
/// the shell tools (`exec_shell`, `task_shell_start`, …) are absent from the
/// catalog with no explanation. Returns `None` when nothing is misplaced.
///
/// This is the exact confusion behind #2589: `allow_shell` and `sandbox_mode`
/// belong at the top of the file, above any `[section]` header.
fn warn_on_misplaced_top_level_keys(raw: &str) -> Option<String> {
    let doc = toml::from_str::<toml::Value>(raw).ok()?;
    // Sections Codewhale does not recognize but users nest settings under.
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
        "Ignoring {} — Codewhale has no `[general]` or `[sandbox]` section, so these \
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
    let mut managed = load_single_config_file(&path)?;
    strip_external_credential_consent(&mut managed);
    let prior_route = (
        config.api_provider(),
        config.provider_identity_for(config.api_provider()),
    );
    let mut merged = merge_config(config.clone(), managed.clone());
    let merged_route = (
        merged.api_provider(),
        merged.provider_identity_for(merged.api_provider()),
    );
    if prior_route != merged_route || config_defines_base_url_for_effective_route(&managed, &merged)
    {
        // Managed configuration is a higher-precedence file layer. If it
        // selects a different route or supplies that route's endpoint, the
        // lower environment layer no longer owns the effective base URL.
        //
        // Record that as an explicit "nobody owns it" rather than clearing the
        // receipt. Clearing it would read as "this config never met the
        // environment layer", which re-enables the generic
        // `CODEWHALE_BASE_URL` fallback for every route — including pinned
        // cross-provider children, which would then borrow an ambient host
        // that managed routing had just taken authority over.
        merged.base_url_env_receipt = BaseUrlEnvReceipt::NoOwner;
        // The shared legacy root field is the same ambient host by another
        // name. If the environment wrote it, managed authority takes it from
        // every route rather than leaving it addressed to the identity that
        // was active before the overlay. A *file*-owned root is left alone:
        // managed did not override it, so it stays the user's value.
        if matches!(merged.root_base_url_owner, BaseUrlEnvReceipt::Route(..)) {
            merged.root_base_url_owner = BaseUrlEnvReceipt::NoOwner;
        }
    }
    *config = merged;
    Ok(())
}

/// Organization-managed overlays may constrain routing and policy, but they
/// cannot consent on a user's behalf to credential files owned by another
/// CLI. Only the user config/profile loaded before this layer may carry these
/// grants. A managed `disabled` record is a tightening tombstone and is kept
/// so a lower-precedence user grant cannot survive an administrator deny.
fn strip_external_credential_consent(config: &mut Config) {
    if config.providers.is_none() {
        return;
    }
    for provider in ApiProvider::all()
        .iter()
        .copied()
        .filter(|provider| *provider != ApiProvider::Custom)
    {
        let external = &mut config
            .provider_config_for_mut(provider)
            .external_credentials;
        if external.as_ref().is_some_and(|consent| {
            consent.access != codewhale_config::ExternalCredentialAccess::Disabled
        }) {
            *external = None;
        }
    }
    if let Some(providers) = config.providers.as_mut() {
        for provider in providers.custom.values_mut() {
            if provider
                .external_credentials
                .as_ref()
                .is_some_and(|consent| {
                    consent.access != codewhale_config::ExternalCredentialAccess::Disabled
                })
            {
                provider.external_credentials = None;
            }
        }
    }
}

fn config_defines_base_url_for_effective_route(source: &Config, effective: &Config) -> bool {
    let provider = effective.api_provider();
    let mut source = source.clone();
    source.provider.clone_from(&effective.provider);
    let provider_base = source
        .provider_config_string_with_runtime_fallback(provider, |entry| entry.base_url.clone());
    let configured = match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => provider_base.or(source.base_url),
        ApiProvider::NvidiaNim => provider_base.or_else(|| {
            source
                .base_url
                .filter(|base| base.contains("integrate.api.nvidia.com"))
        }),
        ApiProvider::Custom if effective.uses_legacy_literal_custom_route() => source.base_url,
        _ => provider_base,
    };
    configured.is_some_and(|base| !base.trim().is_empty())
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
    let requirements: RequirementsFile = toml::from_str(&contents).map_err(|_| {
        anyhow::anyhow!(
            "Failed to parse requirements file {}; file contents were omitted",
            codewhale_config::quote_os_path(&path)
        )
    })?;

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
    codewhale_config::create_config_document(path, content)
}

/// Where a saved credential ended up. Returned by [`save_api_key`] so
/// the caller can show a confirmation message without leaking the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedCredential {
    /// Stored in the durable secret store. The config file contains only
    /// non-secret provider metadata and has any matching plaintext `api_key`
    /// entry removed. The `backend` label is the value of
    /// [`codewhale_secrets::Secrets::backend_name`] at write time so the toast
    /// text can name the actual backend (`"system keyring"`,
    /// `"file-based (~/.codewhale/secrets/)"`).
    KeyringAndConfigFile {
        /// `Secrets::backend_name()` at write time.
        backend: String,
        /// Absolute path to the credential-free config metadata file.
        path: PathBuf,
    },
    /// Stored in the Codewhale config file only under `cfg(test)` so unit tests
    /// without an explicitly isolated secret backend do not pollute the host
    /// credential store. Production save flows never automatically downgrade
    /// a failed secret-store write to plaintext.
    ConfigFile(PathBuf),
}

impl SavedCredential {
    /// Human-readable description for status / log output. Never
    /// includes the key value.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::KeyringAndConfigFile { backend, path } => {
                format!(
                    "secret store ({backend}); credential-free config metadata in {}",
                    path.display()
                )
            }
            Self::ConfigFile(path) => path.display().to_string(),
        }
    }
}

/// Resolve the config document for CREDENTIAL writes: api_key values,
/// `auth_mode` markers, and oauth/external-credential pointers.
///
/// Credentials are user-global — a key saved while working in one repo must be
/// visible from every other repo (#5045, #5193). The ambient
/// `CODEWHALE_CONFIG_PATH`/`DEEPSEEK_CONFIG_PATH` override can point at a
/// workspace-scoped document (`<repo>/.codewhale/config.toml`, plaintext and
/// easy to commit by accident), so credential writes that would land there are
/// rescoped to the user-global config instead. Non-credential settings keep
/// the ambient scoping, and callers that pass an explicit config path never
/// consult this resolver; a per-workspace destination stays possible only as
/// that kind of explicit opt-in.
fn credential_config_path() -> anyhow::Result<PathBuf> {
    let resolved = try_default_config_path()?;
    if !codewhale_config::config_path_is_workspace_scoped(&resolved) {
        return Ok(resolved);
    }
    let global = home_config_path()
        .context("Failed to resolve user-global config path: home directory not found.")?;
    tracing::info!(
        ambient = %resolved.display(),
        global = %global.display(),
        "rescoping credential write from workspace config to user-global config"
    );
    Ok(global)
}

/// Save the active provider's API key.
///
/// The selected durable secret backend is attempted first. On success the
/// config keeps only non-secret auth metadata and any older plaintext copy is
/// removed. When the secret-store write fails (OS permission denied, corrupt
/// or read-only file backend, etc.), the save fails loudly rather than writing
/// the key to plaintext `config.toml`.
///
/// Under `cfg(test)` the secret-store path is enabled only when the test sets
/// both an isolated `CODEWHALE_HOME` and an explicit backend, preventing unit
/// tests from touching the developer's real credential store.
pub fn save_api_key(api_key: &str) -> Result<SavedCredential> {
    save_root_api_key_for_secret_slot(api_key, "deepseek", true)
}

fn save_root_api_key_for_secret_slot(
    api_key: &str,
    secret_slot: &str,
    clear_deepseek_provider_slot: bool,
) -> Result<SavedCredential> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Refusing to save an empty API key.");
    }

    let path = credential_config_path().context("Failed to resolve config path for API key.")?;

    if let Some(secrets) = credential_secret_store() {
        // Same read-modify-write as the per-provider save below; hold the slot's
        // write lock across snapshot, store write, config write, and rollback.
        return crate::credentials::store::with_provider_write_lock(secret_slot, || {
            let prior_secret = secrets.get(secret_slot);
            match prior_secret.as_ref() {
                Ok(prior) => match secrets.set(secret_slot, trimmed) {
                    Ok(()) => {
                        if let Err(error) = save_root_api_key_metadata_without_plaintext(
                            &path,
                            clear_deepseek_provider_slot,
                        ) {
                            let current = secrets.get(secret_slot).map_err(|rollback| {
                        anyhow::anyhow!(
                            "{error}; additionally could not verify secret-store rollback for {secret_slot}: {rollback}"
                        )
                    })?;
                            if current.as_deref() == Some(trimmed) {
                                match prior {
                            Some(previous) => secrets.set(secret_slot, previous),
                            None => secrets.delete(secret_slot),
                        }
                        .map_err(|rollback| {
                            anyhow::anyhow!(
                                "{error}; additionally failed to restore prior secret-store state for {secret_slot}: {rollback}"
                            )
                        })?;
                            }
                            return Err(error);
                        }
                        codewhale_config::scrub_plaintext_api_keys_from_config_backup(&path)?;
                        let backend = secrets.backend_name().to_string();
                        log_sensitive_event(
                            "credential.save",
                            json!({
                                "backend": backend.clone(),
                                "config_path": path.display().to_string(),
                                "plaintext_config_fallback": false,
                            }),
                        );
                        Ok(SavedCredential::KeyringAndConfigFile { backend, path })
                    }
                    Err(err) => Err(plaintext_credential_fallback_refused("write", &path, &err)),
                },
                Err(error) => Err(plaintext_credential_fallback_refused(
                    "snapshot", &path, &error,
                )),
            }
        });
    }

    let path = save_api_key_to_config_file(trimmed)?;
    codewhale_config::scrub_plaintext_api_keys_from_config_backup(&path)?;
    Ok(SavedCredential::ConfigFile(path))
}

fn plaintext_credential_fallback_refused(
    operation: &str,
    config_path: &Path,
    failure: &dyn std::fmt::Display,
) -> anyhow::Error {
    anyhow::anyhow!(
        "Secret storage {operation} failed: {failure}. Refusing to write the API key in plaintext to {}. Fix the configured secret backend and retry; Codewhale did not change that file.",
        codewhale_config::quote_os_path(config_path)
    )
}

/// The durable secret store for credential saves and logout-time deletes.
///
/// Under `cfg(test)` the store is only exposed when the test set both an
/// isolated `CODEWHALE_HOME` and an explicit backend, so unit tests can never
/// touch the developer's real credential store.
#[cfg(not(test))]
fn credential_secret_store() -> Option<codewhale_secrets::Secrets> {
    Some(codewhale_secrets::Secrets::auto_detect())
}

#[cfg(test)]
fn credential_secret_store() -> Option<codewhale_secrets::Secrets> {
    let isolated_home = codewhale_paths::codewhale_home_is_explicit();
    let explicit_backend = std::env::var_os("CODEWHALE_SECRET_BACKEND")
        .or_else(|| std::env::var_os("DEEPSEEK_SECRET_BACKEND"))
        .is_some_and(|value| !value.is_empty());
    (isolated_home && explicit_backend).then(codewhale_secrets::Secrets::auto_detect)
}

fn save_root_api_key_metadata_without_plaintext(
    config_path: &Path,
    clear_deepseek_provider_slot: bool,
) -> Result<()> {
    ensure_parent_dir(config_path)?;
    crate::config_persistence::mutate_config_document(config_path, |doc| {
        crate::config_persistence::set_document_value(doc, &["auth_mode"], "api_key")?;
        if !doc.contains_key("default_text_model") {
            crate::config_persistence::set_document_value(
                doc,
                &["default_text_model"],
                DEFAULT_TEXT_MODEL,
            )?;
        }
        if !doc.contains_key("reasoning_effort") {
            crate::config_persistence::set_document_value(doc, &["reasoning_effort"], "max")?;
        }
        crate::config_persistence::unset_document_value(doc, &["api_key"])?;
        if clear_deepseek_provider_slot {
            crate::config_persistence::unset_document_value(
                doc,
                &["providers", "deepseek", "api_key"],
            )?;
            crate::config_persistence::unset_document_value(
                doc,
                &["providers", "deepseek-cn", "api_key"],
            )?;
        }
        Ok(())
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))
}

/// Write the `api_key` slot directly to `config.toml`.
fn save_api_key_to_config_file(api_key: &str) -> Result<PathBuf> {
    let config_path =
        credential_config_path().context("Failed to resolve config path for API key.")?;

    ensure_parent_dir(&config_path)?;

    if config_path.exists() {
        // TOML-aware upsert. The old line scan keyed off
        // `existing.contains("api_key")`, so a comment that merely mentioned
        // api_key made it skip the insert entirely; editing the document
        // replaces or inserts the real key and keeps user comments.
        crate::config_persistence::mutate_config_document(&config_path, |doc| {
            crate::config_persistence::set_document_value(doc, &["api_key"], api_key)?;
            crate::config_persistence::set_document_value(doc, &["auth_mode"], "api_key")
        })
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    } else {
        // Create new minimal config
        let content = format!(
            r#"# codewhale Configuration
# Set provider credentials in this file or via environment variables.
# See /links in the TUI for provider-specific credential pages.

api_key = "{api_key}"
auth_mode = "api_key"

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
        );
        crate::config_persistence::write_config_toml_atomic(&config_path, &content)
            .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    }

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
/// The default secret store is file-backed and prompt-free. An OS credential
/// store is queried only when the user explicitly selects the system backend.
///
/// Used by the TUI app constructor to decide whether to gate
/// the user behind the in-TUI api-key onboarding screen — getting
/// this wrong made users get prompted for credentials in situations
/// where normal env/config auth was already available.
pub fn has_api_key(config: &Config) -> bool {
    has_api_key_for(config, config.api_provider())
}

fn provider_uses_oauth_credentials(config: &Config, provider: ApiProvider) -> bool {
    !auth_mode_disables_api_key(config.auth_mode_for_provider(provider).as_deref())
        && !config.provider_uses_custom_endpoint(provider)
        && (provider == ApiProvider::OpenaiCodex
            || (provider == ApiProvider::Moonshot
                && config
                    .provider_config_for(provider)
                    .is_some_and(provider_config_uses_kimi_imported_token))
            || (provider == ApiProvider::Xai
                && config
                    .provider_config_for(provider)
                    .is_some_and(provider_config_uses_xai_oauth)))
}

/// The environment variable name a provider route explicitly binds via
/// `[providers.<name>] api_key_env`, when credentials are bound to the active
/// endpoint. `None` when the route declares no binding.
fn bound_provider_api_key_env_name(config: &Config, provider: ApiProvider) -> Option<String> {
    if !config.config_credentials_are_bound_to_provider_endpoint(provider) {
        return None;
    }
    config
        .provider_config_for(provider)
        .and_then(|entry| entry.api_key_env.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn provider_config_env_api_key(config: &Config, provider: ApiProvider) -> Option<String> {
    let env_name = bound_provider_api_key_env_name(config, provider)?;
    std::env::var(env_name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

#[must_use]
pub fn active_provider_has_config_api_key(config: &Config) -> bool {
    let provider = config.api_provider();
    if auth_mode_disables_api_key(config.auth_mode_for_provider(provider).as_deref()) {
        return false;
    }
    let custom_endpoint = config.provider_uses_custom_endpoint(provider);

    if provider == ApiProvider::Moonshot
        && !custom_endpoint
        && config
            .provider_config_for(provider)
            .is_some_and(provider_config_uses_kimi_imported_token)
    {
        return false;
    }
    if provider == ApiProvider::OpenaiCodex && !custom_endpoint {
        // A native ChatGPT PKCE login is a Codewhale-owned credential and
        // stands on its own, before any external Codex CLI consent is
        // considered.
        if crate::chatgpt_oauth::credentials_valid(config) {
            return true;
        }
        // The persistent Codex login is the OAuth credential file, analogous to
        // a stored config key. Token env overrides are scored separately by
        // active_provider_has_env_api_key. #5772: a consent record alone is not
        // a credential — the exact consented path (never an ambient candidate)
        // is read through the secure adapter and must still hold a live token.
        let Some(consent) = config
            .provider_config_for(provider)
            .and_then(|entry| entry.external_credentials.as_ref())
        else {
            return false;
        };
        return config
            .external_credential_read_grant(
                provider,
                codewhale_config::ExternalCredentialSource::CodexCli,
                &consent.path,
            )
            .is_ok_and(|grant| crate::oauth::stored_credentials_present(&grant));
    }
    if !custom_endpoint
        && matches!(provider, ApiProvider::Huggingface)
        && std::env::var("HUGGINGFACE_API_KEY")
            .or_else(|_| std::env::var("HF_TOKEN"))
            .is_ok_and(|k| !k.trim().is_empty())
    {
        return true;
    }

    if config.config_credentials_are_bound_to_provider_endpoint(provider)
        && config
            .provider_config_string_with_runtime_fallback(provider, |entry| entry.api_key.clone())
            .is_some_and(|key| {
                classify_config_api_key_value(&key) == ConfigApiKeyValueKind::Literal
            })
    {
        return true;
    }
    if !config.should_skip_secret_store_for_provider(provider)
        && provider_secret_store_api_key(config, provider).is_some()
    {
        return true;
    }

    matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        && config.config_credentials_are_bound_to_provider_endpoint(provider)
        && config
            .api_key
            .as_ref()
            .is_some_and(|key| classify_config_api_key_value(key) == ConfigApiKeyValueKind::Literal)
}

#[must_use]
pub fn active_provider_has_env_api_key(config: &Config) -> bool {
    let provider = config.api_provider();
    if auth_mode_disables_api_key(config.auth_mode_for_provider(provider).as_deref()) {
        return false;
    }
    (!provider_uses_oauth_credentials(config, provider)
        && explicit_cli_api_key_override().is_some())
        || provider_config_env_api_key(config, provider).is_some()
        || (!config.should_skip_secret_store_for_provider(provider)
            && provider_env_api_key(provider).is_some())
}

#[must_use]
pub fn active_provider_uses_env_only_api_key(config: &Config) -> bool {
    active_provider_has_env_api_key(config) && !active_provider_has_config_api_key(config)
}

/// A key saved in the user-global config file stays visible even when this
/// process loaded a DIFFERENT config (e.g. an explicit workspace `--config`
/// path). Credentials are user-global: a workspace override may select a
/// different route, but it must never make a global credential appear locked.
///
/// Bounded, read-only, non-migrating: parses the default config file's raw
/// provider table directly (never runs legacy migration, never opens a
/// write-capable backend). Returns the key only when it reads as a real
/// literal, not a placeholder.
struct UserGlobalConfigCache {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
    json: serde_json::Value,
}

fn user_global_config_json() -> Option<serde_json::Value> {
    static CACHE: Mutex<Option<UserGlobalConfigCache>> = Mutex::new(None);
    let path = codewhale_config::default_config_path().ok()?;
    let meta = fs::metadata(&path).ok()?;
    let modified = meta.modified().ok();
    let len = meta.len();
    let mut guard = CACHE.lock().ok()?;
    if let Some(cached) = guard.as_ref()
        && cached.path == path
        && cached.modified == modified
        && cached.len == len
    {
        return Some(cached.json.clone());
    }
    let text = fs::read_to_string(&path).ok()?;
    let doc: codewhale_config::ConfigToml = toml::from_str(&text).ok()?;
    let json = serde_json::to_value(&doc).ok()?;
    *guard = Some(UserGlobalConfigCache {
        path,
        modified,
        len,
        json: json.clone(),
    });
    Some(json)
}

fn user_global_config_api_key(provider: ApiProvider) -> Option<String> {
    if provider == ApiProvider::Custom {
        // Custom providers are per-config by nature; the probe applies to
        // built-in ids whose keys are saved under the user-global file.
        return None;
    }
    let json = user_global_config_json()?;
    let provider_config_key = provider.metadata().map_or_else(
        || provider.as_str(),
        |metadata| metadata.provider_config_key(),
    );
    let key = json
        .get("providers")?
        .get(provider_config_key)?
        .get("api_key")?
        .as_str()?;
    let key = key.trim();
    if key.is_empty() || classify_config_api_key_value(key) != ConfigApiKeyValueKind::Literal {
        return None;
    }
    Some(key.to_string())
}

/// Check whether the given provider has any usable API key — via env var,
/// provider/root config. Used by the `/provider` picker to decide whether to
/// prompt for a key inline.
#[must_use]
pub fn has_api_key_for(config: &Config, provider: ApiProvider) -> bool {
    credential_resolve::resolve_credential_source(config, provider).is_present()
}

impl Config {
    /// Resolve one coherent Codex OAuth snapshot. The bearer and account id
    /// must come from the same secure file handle; opening the external JSON a
    /// second time could pair identities across an atomic owner refresh or a
    /// hostile path swap.
    pub(crate) fn codex_credentials(&self) -> Result<crate::oauth::CodexCredentials> {
        if let Some(credentials) = crate::oauth::credentials_from_env() {
            return Ok(credentials);
        }
        anyhow::ensure!(
            self.api_provider() == ApiProvider::OpenaiCodex
                && !self.provider_uses_custom_endpoint(ApiProvider::OpenaiCodex),
            "Codex OAuth credentials are only available on the official OpenAI Codex route"
        );
        if crate::chatgpt_oauth::credentials_valid(self) {
            let owned = crate::chatgpt_oauth::get_owned_credentials(self)?;
            return Ok(crate::oauth::CodexCredentials {
                access_token: owned.access_token,
                account_id: owned.account_id,
            });
        }
        let path = crate::oauth::auth_file_path();
        let grant = self.external_credential_read_grant(
            ApiProvider::OpenaiCodex,
            codewhale_config::ExternalCredentialSource::CodexCli,
            &path,
        )?;
        crate::oauth::get_credentials(&grant)
    }

    /// ChatGPT account id for the already-selected Codex route. Environment
    /// metadata remains independent; the external file is read only when the
    /// exact provider/source/path consent tuple is valid.
    #[cfg(test)]
    pub(crate) fn codex_account_id(&self) -> Option<String> {
        self.codex_credentials()
            .ok()
            .and_then(|credentials| credentials.account_id)
    }
}

/// Whether a provider counts as "configured" for the default `/provider`
/// and `/model` manager views (#3830). Shared by both pickers so "what shows
/// up without browsing the full catalog" stays a single definition.
/// Self-hosted providers (Ollama/Sglang/Vllm) report `has_key = true`
/// unconditionally in [`has_api_key_for`] since they don't require auth to
/// route to — that's correct for routing, but wrong for "did the user set
/// this up," so a self-hosted provider only qualifies via an explicit
/// `[providers.<name>]` entry or being active, never via `has_key` alone
/// (otherwise every self-hosted provider type would always show up).
#[must_use]
pub(crate) fn provider_is_configured(
    provider: ApiProvider,
    is_active: bool,
    has_key: bool,
    configured: Option<&ProviderConfig>,
    is_named_custom_entry: bool,
) -> bool {
    // A *named* custom provider entry (one the user actually added) always
    // counts. The unconfigured `Custom` placeholder row that fills the slot
    // when no custom provider exists yet is not itself "configured" — it's
    // the catalog's invitation to add one.
    if is_active || is_named_custom_entry {
        return true;
    }
    if configured.is_some_and(provider_config_is_explicit) {
        return true;
    }
    if provider.is_self_hosted() {
        return false;
    }
    has_key
}

/// Convenience wrapper around [`provider_is_configured`] for callers that
/// just want "is this provider configured given the active one," without
/// the provider picker's multi-row named-custom-provider bookkeeping
/// (`is_named_custom_entry`) — e.g. the `/model` picker (#3830), which only
/// ever resolves the single, currently-selected `Custom` slot via
/// [`Config::provider_config_for`], the same way model/route resolution
/// does everywhere else.
#[must_use]
pub(crate) fn provider_is_configured_for_active(
    config: &Config,
    provider: ApiProvider,
    active: ApiProvider,
) -> bool {
    provider_is_configured(
        provider,
        provider == active,
        has_api_key_for(config, provider),
        config.provider_config_for(provider),
        false,
    )
}

/// True when a `[providers.<name>]` table entry has any field the user would
/// have had to set explicitly — base URL, model, auth, etc. Used by
/// [`provider_is_configured`]: merely existing in the
/// (always-`Some`-once-any-provider-is-configured) `ProvidersConfig` struct
/// isn't enough, since untouched providers still resolve to a
/// `ProviderConfig::default()` there.
fn provider_config_is_explicit(entry: &ProviderConfig) -> bool {
    let non_empty = |value: Option<&String>| value.is_some_and(|value| !value.trim().is_empty());

    non_empty(entry.api_key.as_ref())
        || non_empty(entry.base_url.as_ref())
        || non_empty(entry.model.as_ref())
        || non_empty(entry.auth_mode.as_ref())
        || entry
            .auth
            .as_ref()
            .is_some_and(|auth| auth.validate().is_ok())
        || entry.context_window.is_some()
        || non_empty(entry.mode.as_ref())
        || entry.max_concurrency.is_some()
        || entry.http_headers.as_ref().is_some_and(|headers| {
            headers
                .iter()
                .any(|(name, value)| !name.trim().is_empty() && !value.trim().is_empty())
        })
        || non_empty(entry.path_suffix.as_ref())
        || non_empty(entry.reasoning_stream_style.as_ref())
        || entry.insecure_skip_tls_verify.is_some()
        || non_empty(entry.kind.as_ref())
        || non_empty(entry.api_key_env.as_ref())
        || entry.external_credentials.is_some()
        || non_empty(entry.oauth_credential_generation.as_ref())
}

/// Save an API key to the appropriate place for the given provider.
/// DeepSeek goes through [`save_api_key`]. Other providers write
/// `[providers.<name>] api_key = "..."` to `~/.codewhale/config.toml`.
/// Returns the config file path.
#[cfg(test)]
pub fn save_api_key_for(provider: ApiProvider, api_key: &str) -> Result<PathBuf> {
    match save_api_key_for_identity(
        &ProviderIdentity {
            provider,
            key: provider.as_str().to_string(),
            exact_id: Some(provider.as_str().to_string()),
            migrated_legacy_ollama_cloud_route: false,
        },
        &Config {
            provider: Some(provider.as_str().to_string()),
            ..Config::default()
        },
        api_key,
    )? {
        SavedCredential::KeyringAndConfigFile { path, .. } | SavedCredential::ConfigFile(path) => {
            Ok(path)
        }
    }
}

/// Save an API key for the given provider identity and return where the
/// credential actually landed ([`SavedCredential`]) so callers can state the
/// true destination — the durable secret store plus credential-free config
/// metadata, or (tests only) the plaintext config file (#5195).
pub(crate) fn save_api_key_for_identity(
    identity: &ProviderIdentity,
    route_config: &Config,
    api_key: &str,
) -> Result<SavedCredential> {
    if identity.provider == ApiProvider::Xai {
        return codewhale_config::with_xai_oauth_revocation_transaction(|| {
            save_api_key_for_identity_unlocked(identity, route_config, api_key)
        });
    }
    save_api_key_for_identity_unlocked(identity, route_config, api_key)
}

fn save_api_key_for_identity_unlocked(
    identity: &ProviderIdentity,
    route_config: &Config,
    api_key: &str,
) -> Result<SavedCredential> {
    let provider = identity.provider;
    if provider == ApiProvider::OpenaiCodex {
        anyhow::bail!(
            "OpenAI Codex uses OAuth. Sign in with ChatGPT via `codewhale auth chatgpt` (subscription billing, Codewhale-owned tokens). The openai API-key route is a different billing owner. Alternatively run `codex login`, then grant exact read-only access with `codewhale auth external-consent --provider openai-codex --mode read-only`, or set OPENAI_CODEX_ACCESS_TOKEN for this process; Codewhale does not store an API key for this provider."
        );
    }
    let is_legacy_literal_custom = provider == ApiProvider::Custom
        && identity.key.trim() == ApiProvider::Custom.as_str()
        && identity.persisted_id().is_none();
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        return save_api_key(api_key);
    }
    if is_legacy_literal_custom {
        return save_root_api_key_for_secret_slot(api_key, "custom", false);
    }

    let api_key = api_key.trim();
    anyhow::ensure!(!api_key.is_empty(), "Refusing to save an empty API key.");

    let config_path =
        credential_config_path().context("Failed to resolve config path for provider API key.")?;
    ensure_parent_dir(&config_path)?;

    let key_inside = if provider == ApiProvider::Custom {
        let key = identity.key.trim();
        anyhow::ensure!(!key.is_empty(), "custom provider id cannot be empty");
        key
    } else {
        provider_config_key(provider).context("provider api key table")?
    };
    // A legacy, manually-selected Kimi CLI import implicitly routed Moonshot
    // traffic to Kimi Code. Once the user replaces that import with the
    // supported API-key route, persist the endpoint before changing auth_mode
    // so the key is not silently sent to the ordinary Moonshot endpoint.
    // Respect an explicit user-owned endpoint.
    let pin_kimi_code_base_url = provider == ApiProvider::Moonshot
        && route_config
            .provider_config_for(provider)
            .is_some_and(|entry| {
                provider_config_uses_kimi_imported_token(entry)
                    && entry
                        .base_url
                        .as_deref()
                        .is_none_or(|base_url| base_url.trim().is_empty())
            });

    if !route_config.should_skip_secret_store_for_provider(provider)
        && let Some(secrets) = credential_secret_store()
    {
        let secret_slot = provider_secret_store_slot(provider);
        // Snapshot -> write -> config-write -> rollback is a read-modify-write.
        // Hold this provider's credential write lock across the whole sequence
        // so a concurrent save or logout on the same slot cannot interleave and
        // leave the secret store and the config document disagreeing. This is
        // the `modify`-is-the-only-write-path rule ported from pi-mono; see
        // `crate::credentials::store`.
        return crate::credentials::store::with_provider_write_lock(secret_slot, || {
            let prior_secret = secrets.get(secret_slot);
            match prior_secret.as_ref() {
                Ok(prior) => match secrets.set(secret_slot, api_key) {
                    Ok(()) => {
                        let config_result = crate::config_persistence::mutate_config_document(
                            &config_path,
                            |doc| {
                                if pin_kimi_code_base_url {
                                    crate::config_persistence::set_document_value(
                                        doc,
                                        &["providers", key_inside, "base_url"],
                                        DEFAULT_KIMI_CODE_BASE_URL,
                                    )?;
                                }
                                crate::config_persistence::set_document_value(
                                    doc,
                                    &["providers", key_inside, "auth_mode"],
                                    "api_key",
                                )?;
                                crate::config_persistence::unset_document_value(
                                    doc,
                                    &["providers", key_inside, "external_credentials"],
                                )?;
                                if provider == ApiProvider::Xai {
                                    crate::config_persistence::unset_document_value(
                                        doc,
                                        &["providers", key_inside, "oauth_credential_generation"],
                                    )?;
                                }
                                crate::config_persistence::unset_document_value(
                                    doc,
                                    &["providers", key_inside, "api_key"],
                                )?;
                                Ok(())
                            },
                        )
                        .with_context(|| {
                            format!("Failed to write config to {}", config_path.display())
                        });
                        if let Err(error) = config_result {
                            let current = secrets.get(secret_slot).map_err(|rollback| {
                        anyhow::anyhow!(
                            "{error}; additionally could not verify secret-store rollback for {secret_slot}: {rollback}"
                        )
                    })?;
                            if current.as_deref() == Some(api_key) {
                                match prior {
                            Some(previous) => secrets.set(secret_slot, previous),
                            None => secrets.delete(secret_slot),
                        }
                        .map_err(|rollback| {
                            anyhow::anyhow!(
                                "{error}; additionally failed to restore prior secret-store state for {secret_slot}: {rollback}"
                            )
                        })?;
                            }
                            return Err(error);
                        }
                        codewhale_config::scrub_plaintext_api_keys_from_config_backup(
                            &config_path,
                        )?;
                        let backend = secrets.backend_name().to_string();
                        log_sensitive_event(
                            "credential.save",
                            json!({
                                "backend": backend.clone(),
                                "provider": identity.key,
                                "config_path": config_path.display().to_string(),
                                "plaintext_config_fallback": false,
                            }),
                        );
                        Ok(SavedCredential::KeyringAndConfigFile {
                            backend,
                            path: config_path,
                        })
                    }
                    Err(err) => Err(plaintext_credential_fallback_refused(
                        "write",
                        &config_path,
                        &err,
                    )),
                },
                Err(error) => Err(plaintext_credential_fallback_refused(
                    "snapshot",
                    &config_path,
                    &error,
                )),
            }
        });
    }

    // Edit the `[providers.<name>]` table in place so unrelated sections,
    // comments, and formatting survive the write.
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        if pin_kimi_code_base_url {
            crate::config_persistence::set_document_value(
                doc,
                &["providers", key_inside, "base_url"],
                DEFAULT_KIMI_CODE_BASE_URL,
            )?;
        }
        crate::config_persistence::set_document_value(
            doc,
            &["providers", key_inside, "auth_mode"],
            "api_key",
        )?;
        crate::config_persistence::unset_document_value(
            doc,
            &["providers", key_inside, "external_credentials"],
        )?;
        if provider == ApiProvider::Xai {
            crate::config_persistence::unset_document_value(
                doc,
                &["providers", key_inside, "oauth_credential_generation"],
            )?;
        }
        crate::config_persistence::set_document_value(
            doc,
            &["providers", key_inside, "api_key"],
            api_key,
        )
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    log_sensitive_event(
        "credential.save",
        json!({
            "backend": "config_file",
            "provider": identity.key,
            "config_path": config_path.display().to_string(),
        }),
    );
    codewhale_config::scrub_plaintext_api_keys_from_config_backup(&config_path)?;

    Ok(SavedCredential::ConfigFile(config_path))
}

/// Persist a default model for `provider` via the comment-preserving config
/// path used by guided provider setup (#3875). DeepSeek writes root
/// `default_text_model`; other hosted providers write `[providers.<name>] model`.
pub(crate) fn save_provider_model_for_identity(
    identity: &ProviderIdentity,
    _route_config: &Config,
    model: &str,
) -> Result<PathBuf> {
    let provider = identity.provider;
    let model = model.trim();
    anyhow::ensure!(!model.is_empty(), "model cannot be empty");

    let config_path =
        try_default_config_path().context("Failed to resolve config path for provider model.")?;
    ensure_parent_dir(&config_path)?;

    let is_legacy_literal_custom = provider == ApiProvider::Custom
        && identity.key.trim() == ApiProvider::Custom.as_str()
        && identity.persisted_id().is_none();
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        || is_legacy_literal_custom
    {
        crate::config_persistence::mutate_config_document(&config_path, |doc| {
            crate::config_persistence::set_document_value(doc, &["default_text_model"], model)
        })
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
        return Ok(config_path);
    }

    let key_inside = if provider == ApiProvider::Custom {
        let key = identity.key.trim();
        anyhow::ensure!(!key.is_empty(), "custom provider id cannot be empty");
        key
    } else {
        provider_config_key(provider).context("provider model table")?
    };
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        crate::config_persistence::set_document_value(
            doc,
            &["providers", key_inside, "model"],
            model,
        )
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    Ok(config_path)
}

/// Persist a guided-setup endpoint choice into the provider's own
/// `[providers.<name>] base_url` (#4526).
///
/// Deliberately narrow: it never touches the root `base_url`, another
/// provider's table, or any other key, so a billing-route choice cannot
/// repoint an unrelated route.
pub(crate) fn save_provider_base_url_for_identity(
    identity: &ProviderIdentity,
    _route_config: &Config,
    base_url: &str,
) -> Result<PathBuf> {
    let base_url = base_url.trim();
    anyhow::ensure!(!base_url.is_empty(), "base URL cannot be empty");
    let config_path = try_default_config_path()
        .context("Failed to resolve config path for provider base URL.")?;
    ensure_parent_dir(&config_path)?;
    let key_inside = if identity.provider == ApiProvider::Custom {
        let key = identity.key.trim();
        anyhow::ensure!(!key.is_empty(), "custom provider id cannot be empty");
        key
    } else {
        provider_config_key(identity.provider).context("provider base URL table")?
    };
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        crate::config_persistence::set_document_value(
            doc,
            &["providers", key_inside, "base_url"],
            base_url,
        )
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    Ok(config_path)
}

/// Persist a guided-setup context-window choice without replacing the user's
/// surrounding TOML comments or formatting.
pub(crate) fn save_provider_context_window_for_identity(
    identity: &ProviderIdentity,
    _route_config: &Config,
    context_window: u32,
) -> Result<PathBuf> {
    anyhow::ensure!(context_window > 0, "context window must be greater than 0");
    let config_path = try_default_config_path()
        .context("Failed to resolve config path for provider context window.")?;
    ensure_parent_dir(&config_path)?;
    let key_inside = if identity.provider == ApiProvider::Custom {
        let key = identity.key.trim();
        anyhow::ensure!(!key.is_empty(), "custom provider id cannot be empty");
        key
    } else {
        provider_config_key(identity.provider).context("provider context window table")?
    };
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        crate::config_persistence::set_document_value(
            doc,
            &["providers", key_inside, "context_window"],
            i64::from(context_window),
        )
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    Ok(config_path)
}

/// Grant-time validation (#5772): read the exact file the user just confirmed,
/// through the same secure adapter the request path uses, and require it to
/// hold a usable credential.
///
/// This runs *after* the confirmation disclosure and *before* any consent
/// record is written, which is the whole ordering the consent model depends
/// on. Persisting first would leave a record claiming a credential exists for
/// a file that is missing, malformed, or expired — and every status surface
/// downstream would then have to trust it. Nothing here refreshes, rewrites,
/// or makes a network request, and no credential value escapes this function.
fn validate_external_credential_before_consent(
    consent_provider: codewhale_config::ProviderKind,
    source: codewhale_config::ExternalCredentialSource,
    path: &Path,
) -> Result<()> {
    let grant = codewhale_config::ExternalCredentialConsentToml::read_only(
        consent_provider,
        source,
        path.to_path_buf(),
    )
    .read_grant(consent_provider, source, path)?;
    match source {
        codewhale_config::ExternalCredentialSource::CodexCli => {
            crate::oauth::get_credentials(&grant).map(|_| ())
        }
        codewhale_config::ExternalCredentialSource::GrokCli => {
            crate::xai_oauth::validate_external_credentials(&grant)
        }
        codewhale_config::ExternalCredentialSource::DshCli => {
            crate::dsh_credentials::deepseek_api_key_from_grant(&grant)?
                .map(|_| ())
                .context("the DeepSeek Harness credentials file holds no DEEPSEEK_API_KEY")
        }
        codewhale_config::ExternalCredentialSource::AgyCli => {
            crate::agy_credentials::antigravity_oauth_token_from_grant(&grant)?
                .map(|_| ())
                .context("the Antigravity credential store holds no OAuth token")
        }
        codewhale_config::ExternalCredentialSource::KimiCodeCli => anyhow::bail!(
            "Kimi CLI credentials are never imported; configure a Kimi API key instead"
        ),
    }
}

/// Persist an explicitly confirmed read-only external credential grant and
/// update the live mirror only after the comment-preserving disk mutation
/// succeeds.
///
/// Order is load-bearing (#5772): the caller has already shown the
/// confirmation disclosure, this function then reads and validates the exact
/// consented file, and only a usable credential is allowed to produce a
/// persisted consent record.
pub(crate) fn persist_external_credential_consent_for_at(
    config_path: Option<&Path>,
    live_config: &mut Config,
    provider: ApiProvider,
    consent_provider: codewhale_config::ProviderKind,
    source: codewhale_config::ExternalCredentialSource,
    path: &Path,
) -> Result<PathBuf> {
    let expected = match provider {
        ApiProvider::OpenaiCodex => (
            codewhale_config::ProviderKind::OpenaiCodex,
            codewhale_config::ExternalCredentialSource::CodexCli,
        ),
        ApiProvider::Xai => (
            codewhale_config::ProviderKind::Xai,
            codewhale_config::ExternalCredentialSource::GrokCli,
        ),
        _ => anyhow::bail!(
            "{} has no supported external credential owner",
            provider.as_str()
        ),
    };
    anyhow::ensure!(
        (consent_provider, source) == expected,
        "external credential owner does not match provider {}",
        provider.as_str()
    );
    let path = codewhale_config::resolve_external_credential_path(path)?;
    let path_value = path.to_str().context(
        "external credential path cannot be persisted losslessly because it is not valid UTF-8",
    )?;
    validate_external_credential_before_consent(consent_provider, source, &path).with_context(
        || {
            format!(
                "no usable {} credential was found, so read-only consent was not saved",
                source.owner_label()
            )
        },
    )?;
    let config_path = match config_path {
        Some(path) => path.to_path_buf(),
        None => credential_config_path()
            .context("Failed to resolve config path for external credential consent.")?,
    };
    ensure_parent_dir(&config_path)?;
    let key_inside = provider_config_key(provider).context("external credential provider key")?;
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        crate::config_persistence::set_document_value(
            doc,
            &["providers", key_inside, "auth_mode"],
            "oauth",
        )?;
        let prefix = &["providers", key_inside, "external_credentials"];
        crate::config_persistence::set_document_value(
            doc,
            &[prefix[0], prefix[1], prefix[2], "access"],
            "read_only",
        )?;
        crate::config_persistence::set_document_value(
            doc,
            &[prefix[0], prefix[1], prefix[2], "provider"],
            consent_provider.as_str(),
        )?;
        crate::config_persistence::set_document_value(
            doc,
            &[prefix[0], prefix[1], prefix[2], "source"],
            source.as_str(),
        )?;
        crate::config_persistence::set_document_value(
            doc,
            &[prefix[0], prefix[1], prefix[2], "path"],
            path_value,
        )?;
        crate::config_persistence::set_document_value(
            doc,
            &[prefix[0], prefix[1], prefix[2], "consent_version"],
            i64::from(codewhale_config::EXTERNAL_CREDENTIAL_CONSENT_VERSION),
        )
    })
    .with_context(|| {
        format!(
            "Failed to write config to {}",
            codewhale_config::quote_os_path(&config_path)
        )
    })?;
    live_config
        .providers
        .get_or_insert_with(ProvidersConfig::default);
    let entry = live_config.provider_config_for_mut(provider);
    entry.auth_mode = Some("oauth".to_string());
    entry.external_credentials = Some(codewhale_config::ExternalCredentialConsentToml::read_only(
        consent_provider,
        source,
        path,
    ));
    Ok(config_path)
}

/// Revoke one provider's external-file access without inspecting that file.
pub(crate) fn revoke_external_credential_consent_for_at(
    config_path: Option<&Path>,
    live_config: &mut Config,
    provider: ApiProvider,
) -> Result<PathBuf> {
    anyhow::ensure!(
        matches!(provider, ApiProvider::OpenaiCodex | ApiProvider::Xai),
        "{} has no supported external credential owner",
        provider.as_str()
    );
    let config_path = match config_path {
        Some(path) => path.to_path_buf(),
        None => credential_config_path()
            .context("Failed to resolve config path for external credential consent.")?,
    };
    ensure_parent_dir(&config_path)?;
    let key_inside = provider_config_key(provider).context("external credential provider key")?;
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        crate::config_persistence::unset_document_value(
            doc,
            &["providers", key_inside, "external_credentials"],
        )?;
        Ok(())
    })
    .with_context(|| {
        format!(
            "Failed to write config to {}",
            codewhale_config::quote_os_path(&config_path)
        )
    })?;
    live_config
        .provider_config_for_mut(provider)
        .external_credentials = None;
    Ok(config_path)
}

pub(crate) fn provider_config_key(provider: ApiProvider) -> Result<&'static str> {
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

/// Canonical durable-credential slot shared with the CLI dispatcher.
fn provider_secret_store_slot(provider: ApiProvider) -> &'static str {
    match provider {
        // TUI compatibility variants share the canonical CLI provider slots.
        ApiProvider::DeepseekCN => "deepseek",
        // Shared-account families (SiliconFlow China, the four Model Studio
        // variants) collapse onto one slot via ProviderKind::secret_store_slot.
        _ => provider
            .kind()
            .map_or_else(|| provider.as_str(), |kind| kind.secret_store_slot()),
    }
}

/// Whether the secret-store save marker (`auth_mode = "api_key"` with no
/// config literal, written by the save path) exists for `provider` or for any
/// provider sharing its durable credential slot.
///
/// One Model Studio account authenticates all four plan/dialect variants, so
/// saving a key on `modelstudio-token-plan` marks only that variant's config
/// table; the sibling variants must still treat the family slot as saved.
fn secret_slot_save_marker_on_shared_slot(config: &Config, provider: ApiProvider) -> bool {
    let slot = provider_secret_store_slot(provider);
    ApiProvider::all()
        .iter()
        .copied()
        .chain(std::iter::once(ApiProvider::DeepseekCN))
        .filter(|candidate| provider_secret_store_slot(*candidate) == slot)
        .any(|candidate| {
            config
                .provider_config_for(candidate)
                .is_some_and(|entry| auth_mode_requires_api_key(entry.auth_mode.as_deref()))
        })
}

/// Read only the durable secret-store layer (no environment fallback).
///
/// This keeps `config -> secret store -> env` precedence explicit in the TUI
/// and lets status surfaces distinguish a saved key from an ambient export.
pub(crate) fn provider_secret_store_api_key(
    config: &Config,
    provider: ApiProvider,
) -> Option<String> {
    provider_secret_store_api_key_with_mode(config, provider, false)
}

fn provider_secret_store_api_key_with_mode(
    config: &Config,
    provider: ApiProvider,
    read_only: bool,
) -> Option<String> {
    // Keep the named-custom exclusion at the credential boundary itself.
    // Callers also use this policy to avoid unnecessary keyring probes, but a
    // future caller must not be able to read the legacy `custom` slot for an
    // arbitrary `[providers.<name>]` endpoint by omitting that outer guard.
    if config.should_skip_secret_store_for_provider(provider) {
        return None;
    }

    // Unit tests must never inspect the developer's real credential store.
    // Secret-store regressions opt in with an isolated CODEWHALE_HOME and an
    // explicit backend, matching the secrets crate's own test discipline.
    #[cfg(test)]
    if !codewhale_paths::codewhale_home_is_explicit()
        || std::env::var_os("CODEWHALE_SECRET_BACKEND").is_none()
    {
        return None;
    }

    let secrets = if read_only {
        codewhale_secrets::Secrets::auto_detect_read_only()
    } else {
        codewhale_secrets::Secrets::auto_detect()
    };
    // Read through the credential-store trait so every read of a durable slot
    // goes through one adapter (`crate::credentials::store`), and the value is
    // carried as a type-tagged `Credential` rather than a bare String that can
    // drift into a log line.
    let store =
        crate::credentials::store::SecretStoreCredentials::new(secrets, known_secret_store_slots());
    let primary = store
        .read(provider_secret_store_slot(provider))
        .ok()
        .flatten()
        .map(|credential| credential.expose_secret().to_string());
    if primary.is_some() {
        return primary;
    }

    // The old local identity owned the hosted slot only when the live config
    // selected the exact Ollama Cloud route. Never apply this fallback to a
    // neighboring/custom endpoint or to an explicit new `ollama-cloud`
    // selection, and never write/copy/delete either slot while resolving.
    (provider == ApiProvider::OllamaCloud && config.selects_legacy_ollama_cloud_route())
        .then(|| {
            store
                .read(ApiProvider::Ollama.as_str())
                .ok()
                .flatten()
                .map(|credential| credential.expose_secret().to_string())
        })
        .flatten()
}

/// Every durable credential slot CodeWhale knows how to write.
///
/// The backing keyring exposes no key enumeration, so
/// [`crate::credentials::store::SecretStoreCredentials::list`] is given the
/// slot names to probe. Deduplicated because shared-account families collapse
/// several providers onto one slot.
fn known_secret_store_slots() -> Vec<String> {
    let mut slots: Vec<String> = ApiProvider::all()
        .iter()
        .copied()
        .chain(std::iter::once(ApiProvider::DeepseekCN))
        .map(|provider| provider_secret_store_slot(provider).to_string())
        .collect();
    slots.sort();
    slots.dedup();
    slots
}

/// The shadowing warning for a config-file `api_key` that wins over a live
/// secret-store credential, if both exist (#5194).
///
/// The config file intentionally outranks the secret store in the read
/// chain, but a shadowed slot is invisible: the user rotates the key with
/// `codewhale auth set` and nothing changes, because the stale plaintext
/// copy still wins. Mirror the fleet-roster shadowing rule (#5098):
/// precedence is normal, but it must be VISIBLE. The message names both
/// sources, which one won, and the command that resolves the shadow.
/// Split from [`warn_on_config_api_key_shadowing`] so the decision is
/// testable without capturing tracing output.
fn config_api_key_shadow_warning(
    config: &Config,
    provider: ApiProvider,
    config_source: &str,
) -> Option<String> {
    if config.should_skip_secret_store_for_provider(provider) {
        return None;
    }
    provider_secret_store_api_key_with_mode(config, provider, true).map(|_| {
        let slot = provider_secret_store_slot(provider);
        let id = provider.as_str();
        format!(
            "both {config_source} in the config file and secret-store slot \"{slot}\" \
             hold a credential for provider {id}; the config-file key won. Run \
             `codewhale auth set --provider {id}` to move the key into the secret store \
             and strip the plaintext copy, or remove the config-file api_key."
        )
    })
}

/// Emit the #5194 shadowing warning at most once per provider slot per
/// process: credential resolution runs on every request, and a repeating
/// warning is noise, not signal.
fn warn_on_config_api_key_shadowing(config: &Config, provider: ApiProvider, config_source: &str) {
    let Some(message) = config_api_key_shadow_warning(config, provider, config_source) else {
        return;
    };
    static WARNED_SLOTS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashSet<&'static str>>,
    > = std::sync::OnceLock::new();
    let mut warned = WARNED_SLOTS
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !warned.insert(provider_secret_store_slot(provider)) {
        return;
    }
    drop(warned);
    tracing::warn!("{message}");
}

/// The model this launch was explicitly asked for, if any.
///
/// The `codewhale` dispatcher forwards `--model` to this binary as
/// `CODEWHALE_MODEL` (with the legacy `DEEPSEEK_MODEL` alias), so an explicit
/// flag and an explicit shell export are the same signal here: *the user named
/// a model for this run*. That has to outrank the remembered per-provider
/// selection in `settings.toml`, which is a convenience memory of the last
/// `/model` pick — never a reason to run something the user did not ask for
/// (v0.9.1 kimi-k3 dogfood report).
pub(crate) fn explicit_launch_model_override() -> Option<String> {
    codewhale_env_var("CODEWHALE_MODEL", "DEEPSEEK_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// The provider this launch was explicitly asked for, if any.
///
/// An environment/CLI override is a one-run instruction and must outrank the
/// user's saved startup default. A provider merely named in config.toml is a
/// seed instead: the user can deliberately replace that seed from `/model`.
pub(crate) fn explicit_launch_provider_override() -> Option<String> {
    codewhale_env_var("CODEWHALE_PROVIDER", "DEEPSEEK_PROVIDER")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn explicit_cli_api_key_override() -> Option<String> {
    (cli_api_key_source().as_deref() == Some("cli"))
        .then(|| {
            std::env::var(codewhale_config::CLI_API_KEY_ENV)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .flatten()
}

pub(crate) fn cli_api_key_source() -> Option<String> {
    codewhale_env_var(
        codewhale_config::CLI_API_KEY_SOURCE_ENV,
        codewhale_config::LEGACY_CLI_API_KEY_SOURCE_ENV,
    )
    .ok()
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

/// Clear every saved API key from config-file storage AND the durable
/// secret store.
///
/// The full-wipe logout path (`codewhale-tui --logout`, `auth logout`)
/// calls this to remove credentials so the next request can't
/// silently use a stale config key (#343). The function removes the legacy
/// root `api_key` entry *and* every `api_key` entry nested in a
/// `[providers.<name>]` table, leaving keys like `api_key_env`, comments,
/// and formatting untouched, then deletes every provider's secret-store
/// slot — symmetric with CLI logout (#5159) — so a stored credential cannot
/// survive logout and reappear through the read chain (#5196). The TUI
/// `/logout` command stays single-provider and goes through
/// [`clear_active_provider_api_key`] instead.
///
/// Environment variables (`DEEPSEEK_API_KEY`, etc.) are intentionally
/// **not** unset — they are managed by the user's shell and outside the
/// CLI's purview. `Config::deepseek_api_key`'s explicit-override path
/// (Path 0) ensures a freshly-entered key still wins over a stale env
/// var that lingers from a previous session.
pub fn clear_api_key() -> Result<()> {
    codewhale_config::with_xai_oauth_revocation_transaction(clear_api_key_unlocked)
}

fn clear_api_key_unlocked() -> Result<()> {
    // Same read-modify-write as the saves: hold every durable slot's write
    // lock across the config-document mutation and the store deletes so a
    // concurrent save cannot interleave and leave the two disagreeing.
    crate::credentials::store::with_provider_write_locks(
        known_secret_store_slots(),
        clear_api_key_under_slot_locks,
    )
}

fn clear_api_key_under_slot_locks() -> Result<()> {
    // Strip api_key entries from config.toml, including provider-scoped
    // nested entries. Clearing a config file must not trigger platform
    // credential prompts. Clears target the same user-global document that
    // credential saves write, so logout removes what login stored (#5045).
    let config_path = credential_config_path()
        .context("Failed to resolve config path while clearing API keys.")?;

    if config_path.exists() {
        crate::config_persistence::mutate_config_document(&config_path, |doc| {
            crate::config_persistence::remove_document_key_recursive(doc.as_table_mut(), "api_key");
            crate::config_persistence::unset_document_value(
                doc,
                &["providers", "xai", "oauth_credential_generation"],
            )?;
            crate::config_persistence::unset_document_value(
                doc,
                &["providers", "xai", "auth_mode"],
            )?;
            crate::config_persistence::unset_document_value(
                doc,
                &["providers", "xai", "external_credentials"],
            )?;
            Ok(())
        })
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
        log_sensitive_event(
            "credential.clear",
            json!({
                "backend": "config_file",
                "config_path": config_path.display().to_string(),
                "scope": "root_and_provider_keys",
            }),
        );
    }

    // The config scrub alone leaves the durable secret-store credential
    // alive, and the read chain prefers the secret store over the file, so a
    // "cleared" key silently came back on the next launch (#5196). Delete
    // every provider slot too, symmetric with CLI logout (#5159). This runs
    // even when the config file is absent: the slot survives independently
    // of the file.
    if let Some(secrets) = credential_secret_store() {
        let failures = clear_all_provider_api_keys_from_secret_store(secrets);
        if !failures.is_empty() {
            anyhow::bail!(
                "failed to delete stored credentials for: {}",
                failures.join(", ")
            );
        }
    }

    Ok(())
}

/// Delete the credential slot of every provider that has one stored.
///
/// Mirrors the CLI logout helper (#5159): each slot is probed first so
/// backends that error on deleting a missing item stay quiet, slots shared
/// by several providers (e.g. the historical `siliconflow` slot) are deleted
/// once, and every deletion failure is returned as a human-readable entry so
/// the caller can fail loudly instead of claiming a clean logout while
/// credentials linger in the store (#5196).
fn clear_all_provider_api_keys_from_secret_store(
    secrets: codewhale_secrets::Secrets,
) -> Vec<String> {
    let mut failures = Vec::new();
    let store = crate::credentials::store::SecretStoreCredentials::new(
        secrets.clone(),
        known_secret_store_slots(),
    );
    // `list` enumerates the slots that actually hold something, without
    // exposing any value — the deduplication that used to live here is now the
    // slot table's job.
    let stored: Vec<crate::credentials::CredentialInfo> = match store.list() {
        Ok(stored) => stored,
        Err(error) => {
            failures.push(format!("secret store enumeration: {error}"));
            return failures;
        }
    };
    for entry in stored {
        // The caller already holds this slot's write lock for the whole
        // logout. Delete through the backend rather than `store.delete`,
        // which would re-acquire the same non-reentrant mutex and deadlock.
        if let Err(error) = secrets.delete(&entry.provider_id) {
            failures.push(format!("{}: {error}", entry.provider_id));
        }
    }
    failures
}

/// Clear only the active provider's API key from the config file and delete
/// that provider's durable secret-store slot (#5196).
/// Unlike `clear_api_key()` which strips ALL api_key entries, this
/// removes only the key for the specified provider section (plus the
/// legacy root `api_key` when the provider is DeepSeek).
pub fn clear_active_provider_api_key(provider: &str) -> Result<()> {
    if provider == ApiProvider::Xai.as_str() {
        return codewhale_config::with_xai_oauth_revocation_transaction(|| {
            clear_active_provider_api_key_unlocked(provider)
        });
    }
    clear_active_provider_api_key_unlocked(provider)
}

fn clear_active_provider_api_key_unlocked(provider: &str) -> Result<()> {
    let slot = ApiProvider::all()
        .iter()
        .find(|candidate| candidate.as_str() == provider)
        .map(|candidate| provider_secret_store_slot(*candidate));
    match slot {
        Some(slot) => crate::credentials::store::with_provider_write_lock(slot, || {
            clear_active_provider_api_key_under_lock(provider)
        }),
        None => clear_active_provider_api_key_under_lock(provider),
    }
}

fn clear_active_provider_api_key_under_lock(provider: &str) -> Result<()> {
    let config_path = credential_config_path()
        .context("Failed to resolve config path while clearing API keys.")?;

    if config_path.exists() {
        // `custom` is both the legacy root-shaped route id and a valid exact
        // `[providers.custom]` table key. Inspect the persisted shape before the
        // mutation so logout clears exactly one credential scope.
        let persisted = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config from {}", config_path.display()))?;
        let persisted_config: Config = toml::from_str(&persisted).map_err(|_| {
            anyhow::anyhow!(
                "Failed to parse config from {}; file contents were omitted",
                codewhale_config::quote_os_path(&config_path)
            )
        })?;
        let exact_literal_custom_table = provider == ApiProvider::Custom.as_str()
            && persisted_config
                .providers
                .as_ref()
                .and_then(|providers| providers.custom_provider_config(provider))
                .is_some();

        crate::config_persistence::mutate_config_document(&config_path, |doc| {
            // The root-level api_key is shared by the legacy DeepSeek and released
            // literal-custom config shapes. Exact named custom ids remain scoped
            // to their own table.
            if matches!(
                provider,
                value if value == ApiProvider::Deepseek.as_str()
                    || value == ApiProvider::DeepseekCN.as_str()
            ) || (provider == ApiProvider::Custom.as_str() && !exact_literal_custom_table)
            {
                crate::config_persistence::unset_document_value(doc, &["api_key"])?;
            }
            if provider != ApiProvider::Custom.as_str() || exact_literal_custom_table {
                crate::config_persistence::unset_document_value(
                    doc,
                    &["providers", provider, "api_key"],
                )?;
            }
            if provider == ApiProvider::Xai.as_str() {
                crate::config_persistence::unset_document_value(
                    doc,
                    &["providers", "xai", "oauth_credential_generation"],
                )?;
                crate::config_persistence::unset_document_value(
                    doc,
                    &["providers", "xai", "auth_mode"],
                )?;
                crate::config_persistence::unset_document_value(
                    doc,
                    &["providers", "xai", "external_credentials"],
                )?;
            }
            Ok(())
        })
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
        log_sensitive_event(
            "credential.clear",
            json!({
                "backend": "config_file",
                "config_path": config_path.display().to_string(),
                "scope": provider,
            }),
        );
    }

    // The durable secret-store slot survives a config-file scrub and the
    // read chain prefers it, so the cleared key would silently come back
    // (#5196). Delete the provider's slot too — even when the config file
    // itself is absent. Exact named custom providers have no secret-store
    // slot, so an unmatched provider string skips this step.
    if let Some(secrets) = credential_secret_store()
        && let Some(slot) = ApiProvider::all()
            .iter()
            .find(|candidate| candidate.as_str() == provider)
            .map(|candidate| provider_secret_store_slot(*candidate))
    {
        let has_value = secrets
            .get(slot)
            .ok()
            .flatten()
            .is_some_and(|value| !value.trim().is_empty());
        if has_value {
            secrets
                .delete(slot)
                .with_context(|| format!("failed to delete stored credential for {slot}"))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;

/// #5045 regression coverage: credential writes must never land in a
/// workspace-scoped `.codewhale/config.toml`.
#[cfg(test)]
mod credential_scope_tests {
    use super::*;
    use crate::test_support::{EnvVarGuard, lock_test_env};

    /// With the ambient config path pointing at a workspace-local
    /// `.codewhale/config.toml` (a checkout the user works in), saving an
    /// API key must write the user-global config under the isolated
    /// `CODEWHALE_HOME`, never the project file. The `.git` marker stands in
    /// for cwd-inside-the-workspace: chdir is process-global and unsafe in a
    /// parallel test binary, and production classifies on either signal.
    #[test]
    fn api_key_save_rescopes_workspace_config_to_user_global() -> Result<()> {
        let _lock = lock_test_env();
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().join("repo");
        fs::create_dir_all(workspace.join(".git"))?;
        let project_dir = workspace.join(".codewhale");
        fs::create_dir_all(&project_dir)?;
        let project_config = project_dir.join("config.toml");
        fs::write(&project_config, "approval_policy = \"never\"\n")?;

        let user_home = temp.path().join("user-global-home");
        let _home = EnvVarGuard::set("CODEWHALE_HOME", user_home.as_os_str());
        let _config = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", project_config.as_os_str());
        let _legacy_config = EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");
        // No explicit secret backend: under cfg(test) the save takes the
        // plaintext config-file path, which is exactly the surface this
        // regression guards.
        let _backend = EnvVarGuard::remove("CODEWHALE_SECRET_BACKEND");
        let _legacy_backend = EnvVarGuard::remove("DEEPSEEK_SECRET_BACKEND");

        let saved = save_api_key("workspace-rescope-test-key")?;

        let global_config = user_home.join("config.toml");
        // Compare canonicalized paths: the resolved config path runs through
        // `normalize_config_file_path`, which canonicalizes the parent, so on
        // macOS the lexical `/var/folders/…` tempdir and its canonical
        // `/private/var/folders/…` form are the same file. A lexical compare
        // both false-fails and false-passes on that symlink.
        let saved_path = match saved {
            SavedCredential::ConfigFile(path) => path,
            other => panic!("expected a config-file save, got {}", other.describe()),
        };
        assert_eq!(
            canonicalize_or_keep(&saved_path),
            canonicalize_or_keep(&global_config),
            "credential save must surface the user-global destination"
        );
        let global = fs::read_to_string(&global_config)?;
        assert!(
            global.contains("workspace-rescope-test-key"),
            "user-global config must hold the saved key: {global}"
        );
        let project = fs::read_to_string(&project_config)?;
        assert!(
            !project.contains("workspace-rescope-test-key"),
            "credential leaked into workspace config: {project}"
        );
        assert!(
            !project.contains("api_key"),
            "workspace config must stay credential-free: {project}"
        );
        Ok(())
    }

    /// Provider-table saves go through the same resolver: an OpenRouter key
    /// saved with a workspace-scoped ambient config path must land in the
    /// user-global document.
    #[test]
    fn provider_api_key_save_rescopes_workspace_config_to_user_global() -> Result<()> {
        let _lock = lock_test_env();
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().join("repo");
        fs::create_dir_all(workspace.join(".git"))?;
        let project_dir = workspace.join(".codewhale");
        fs::create_dir_all(&project_dir)?;
        let project_config = project_dir.join("config.toml");
        fs::write(&project_config, "approval_policy = \"never\"\n")?;

        let user_home = temp.path().join("user-global-home");
        let _home = EnvVarGuard::set("CODEWHALE_HOME", user_home.as_os_str());
        let _config = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", project_config.as_os_str());
        let _legacy_config = EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");
        let _backend = EnvVarGuard::remove("CODEWHALE_SECRET_BACKEND");
        let _legacy_backend = EnvVarGuard::remove("DEEPSEEK_SECRET_BACKEND");

        let path = save_api_key_for(ApiProvider::Openrouter, "workspace-rescope-openrouter-key")?;

        // Canonicalized comparison: see the root-key test above.
        assert_eq!(
            canonicalize_or_keep(&path),
            canonicalize_or_keep(&user_home.join("config.toml")),
            "provider save must report the user-global destination"
        );
        let global = fs::read_to_string(&path)?;
        assert!(global.contains("workspace-rescope-openrouter-key"));
        let project = fs::read_to_string(&project_config)?;
        assert!(
            !project.contains("workspace-rescope-openrouter-key"),
            "credential leaked into workspace config: {project}"
        );
        Ok(())
    }
}
