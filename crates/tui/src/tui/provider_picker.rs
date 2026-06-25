//! `/provider` picker modal — pick a provider (DeepSeek / NVIDIA NIM /
//! hosted providers / self-hosted providers) and, if it lacks credentials, type the API key
//! inline before completing the switch (#52).
//!
//! The picker is intentionally a single modal with two visible states:
//!
//! 1. **List** — pick a provider; each row shows the active provider arrow
//!    and an "API key configured" / "needs API key" hint. Enter on a
//!    configured provider applies the switch immediately
//!    ([`ViewEvent::ProviderPickerApplied`]). Enter on an un-configured one
//!    transitions the same modal into the key-entry state.
//! 2. **Key entry** — masked input box pre-filled with the provider's
//!    canonical env-var name as a hint. Enter submits
//!    [`ViewEvent::ProviderPickerApiKeySubmitted`], which the UI handler
//!    persists via `save_api_key_for` before switching.
//!
//! Pressing Esc backs out: from key entry returns to the list; from the
//! list closes the modal without changes.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};

use crate::config::{ApiProvider, Config, has_api_key_for, kimi_cli_credentials_present};
use crate::core::ops::ProviderRuntimeStatus;
use crate::model_profile::{SupportState, resolved_capability_profile};
use crate::palette;
use crate::tui::app::ReasoningEffort;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};
use codewhale_config::catalog::{CatalogOffering, CatalogSnapshot};
use codewhale_config::provider::WireFormat;
use codewhale_config::route::{
    LogicalModelRef, PricingSku, RequestProtocol, RouteRequest, RouteResolver, bundled_offerings,
};
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    List,
    KeyEntry,
}

pub struct ProviderPickerView {
    rows: Vec<ProviderDashboardRow>,
    selected_idx: usize,
    stage: Stage,
    api_key_input: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDashboardRow {
    pub provider: ApiProvider,
    pub provider_id: String,
    pub display_name: String,
    pub kind: String,
    pub base_url: String,
    pub auth_status: ProviderAuthStatus,
    pub catalog_status: ProviderCatalogStatus,
    pub supported_protocols: Vec<String>,
    pub available_model_count: usize,
    pub default_route: ProviderDefaultRoute,
    pub request_concurrency: ProviderRequestConcurrencySummary,
    pub usage_meter: String,
    pub reasoning: ProviderReasoningSummary,
    pub capabilities: ProviderCapabilityBadges,
    pub model_origin: ProviderModelOrigin,
    pub readiness: ProviderReadiness,
    pub maturity: ProviderMaturity,
    pub messages: Vec<String>,
    pub is_active: bool,
    has_key: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAuthStatus {
    Configured,
    Missing,
    Optional,
    OAuthReady,
    OAuthMissing,
    Local,
    Legacy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderCatalogStatus {
    Bundled,
    DefaultOnly,
    Legacy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDefaultRoute {
    pub logical_model: String,
    pub wire_model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderRequestConcurrencySummary {
    pub limit: Option<usize>,
    pub active: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderReadiness {
    Ready,
    NeedsAuth,
    LocalReady,
    Legacy,
    Invalid,
}

/// How battle-tested a provider integration is, independent of whether the
/// user has credentials configured (which `ProviderReadiness` already tracks).
/// Kept intentionally minimal — the only two honest states today are an
/// experimental integration and a supported one (#2984).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderMaturity {
    Experimental,
    Supported,
}

impl ProviderMaturity {
    /// Maturity is seeded from a small table keyed by provider. Only the
    /// OpenAI Codex bridge is experimental today; everything else is supported.
    fn for_provider(provider: ApiProvider) -> Self {
        match provider {
            ApiProvider::OpenaiCodex => Self::Experimental,
            _ => Self::Supported,
        }
    }

    /// Compact tag for the picker hint. Returns `None` when the integration is
    /// supported so the common case stays noise-free (#2984).
    fn tag(self) -> Option<&'static str> {
        match self {
            Self::Experimental => Some("experimental"),
            Self::Supported => None,
        }
    }
}

/// Where the row's current model came from, so the dashboard can distinguish a
/// provider default from a saved override or a custom pass-through id (#3083).
/// Live-catalog/static origins are not yet distinguishable here; they arrive
/// with the #3385 live-fetch layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderModelOrigin {
    Default,
    Saved,
    Custom,
}

impl ProviderModelOrigin {
    fn for_provider(provider: ApiProvider, has_saved_model: bool) -> Self {
        if has_saved_model {
            Self::Saved
        } else if provider == ApiProvider::Custom {
            Self::Custom
        } else {
            Self::Default
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Saved => "saved",
            Self::Custom => "custom",
        }
    }
}

/// Capability + metadata badges projected from the resolved capability profile
/// (#3083). Tri-state so "unknown" stays distinct from "unsupported"; metadata
/// is `None` when not resolvable. Reasoning is tracked separately in
/// [`ProviderReasoningSummary`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCapabilityBadges {
    pub context_window: Option<u32>,
    pub max_output: Option<u32>,
    pub tools: SupportState,
    pub structured: SupportState,
    pub streaming: SupportState,
    pub cache: SupportState,
}

impl ProviderCapabilityBadges {
    fn for_route(provider: ApiProvider, wire_model: &str) -> Self {
        let cap = resolved_capability_profile(provider, wire_model);
        Self {
            context_window: cap.context_window,
            max_output: cap.max_output,
            tools: cap.native_tool_calls,
            structured: cap.structured_output,
            streaming: cap.streaming,
            cache: cap.prompt_caching,
        }
    }

    fn unknown() -> Self {
        Self {
            context_window: None,
            max_output: None,
            tools: SupportState::Unknown,
            structured: SupportState::Unknown,
            streaming: SupportState::Unknown,
            cache: SupportState::Unknown,
        }
    }

    /// Compact, never-fabricating badge cluster. Metadata and each capability
    /// render `?` when unknown rather than being silently dropped.
    fn label(&self) -> String {
        format!(
            "ctx:{} out:{} tools:{} json:{} stream:{} cache:{}",
            humanize_token_count(self.context_window),
            humanize_token_count(self.max_output),
            support_glyph(self.tools),
            support_glyph(self.structured),
            support_glyph(self.streaming),
            support_glyph(self.cache),
        )
    }
}

fn support_glyph(state: SupportState) -> &'static str {
    match state {
        SupportState::Supported => "y",
        SupportState::Unsupported => "n",
        SupportState::Unknown => "?",
    }
}

fn humanize_token_count(value: Option<u32>) -> String {
    match value {
        None => "?".to_string(),
        Some(v) if v >= 1_000_000 && v % 1_000_000 == 0 => format!("{}M", v / 1_000_000),
        Some(v) if v >= 1_000_000 => format!("{:.1}M", f64::from(v) / 1_000_000.0),
        Some(v) if v >= 1_000 => format!("{}K", v / 1_000),
        Some(v) => v.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderReasoningSummary {
    pub support: ProviderReasoningSupport,
    pub controls: Vec<String>,
    pub stream_visibility: ProviderReasoningStreamVisibility,
    pub selected_control: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderReasoningSupport {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderReasoningStreamVisibility {
    StructuredThinking,
    InlineTags,
    SummaryOnly,
    NotExposed,
    Unknown,
}

impl ProviderDashboardRow {
    #[cfg(test)]
    fn from_config(provider: ApiProvider, active: ApiProvider, config: &Config) -> Self {
        Self::from_config_with_runtime_status(provider, active, config, None)
    }

    fn from_config_with_runtime_status(
        provider: ApiProvider,
        active: ApiProvider,
        config: &Config,
        runtime_status: Option<&ProviderRuntimeStatus>,
    ) -> Self {
        Self::from_config_with_provider_id(provider, active, config, None, runtime_status)
    }

    fn from_custom_config_with_runtime_status(
        provider_id: &str,
        active: ApiProvider,
        config: &Config,
        runtime_status: Option<&ProviderRuntimeStatus>,
    ) -> Self {
        let mut scoped = config.clone();
        scoped.provider = Some(provider_id.to_string());
        Self::from_config_with_provider_id(
            ApiProvider::Custom,
            active,
            &scoped,
            Some(provider_id),
            runtime_status,
        )
    }

    fn from_config_with_provider_id(
        provider: ApiProvider,
        active: ApiProvider,
        config: &Config,
        provider_id_override: Option<&str>,
        runtime_status: Option<&ProviderRuntimeStatus>,
    ) -> Self {
        let configured = config.provider_config_for(provider);
        let configured_base_url = configured
            .and_then(|entry| entry.base_url.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let configured_model = configured
            .and_then(|entry| entry.model.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let has_configured_model = configured_model.is_some();
        let model_origin = ProviderModelOrigin::for_provider(provider, has_configured_model);
        let has_key = if provider == ApiProvider::Custom {
            custom_provider_has_auth(configured)
        } else {
            has_api_key_for(config, provider)
        };
        let auth_status = auth_status_for(provider, has_key, configured);
        let usage_meter = usage_meter_for(provider);
        let provider_id = provider_id_override
            .map(str::to_string)
            .unwrap_or_else(|| provider.as_str().to_string());
        let display_name = provider_id_override
            .map(|id| format!("{id} (custom)"))
            .unwrap_or_else(|| provider.display_name().to_string());
        let is_active = if provider == ApiProvider::Custom {
            active == ApiProvider::Custom
                && match provider_id_override {
                    Some(id) => config.provider.as_deref() == Some(id),
                    None => true,
                }
        } else {
            provider == active
        };
        let request_concurrency =
            ProviderRequestConcurrencySummary::for_row(provider, config, runtime_status, is_active);

        let Some(kind) = provider.kind() else {
            return Self {
                provider,
                provider_id,
                display_name,
                kind: "legacy".to_string(),
                base_url: configured_base_url
                    .unwrap_or_else(|| provider.default_base_url().to_string()),
                auth_status: ProviderAuthStatus::Legacy,
                catalog_status: ProviderCatalogStatus::Legacy,
                supported_protocols: vec![protocol_label(WireFormat::ChatCompletions).to_string()],
                available_model_count: 0,
                default_route: ProviderDefaultRoute {
                    logical_model: configured_model
                        .unwrap_or_else(|| "deepseek-v4-pro".to_string()),
                    wire_model: "legacy alias".to_string(),
                },
                request_concurrency,
                usage_meter,
                reasoning: ProviderReasoningSummary::unknown(provider, config),
                capabilities: ProviderCapabilityBadges::unknown(),
                model_origin,
                readiness: ProviderReadiness::Legacy,
                maturity: ProviderMaturity::for_provider(provider),
                messages: vec![
                    "legacy DeepSeek China alias; routing maps through DeepSeek compatibility"
                        .to_string(),
                ],
                is_active,
                has_key,
            };
        };

        let available_model_count = bundled_offerings()
            .iter()
            .filter(|offering| offering.provider.as_str() == kind.as_str())
            .count();
        let catalog_status = if available_model_count == 0 {
            ProviderCatalogStatus::DefaultOnly
        } else {
            ProviderCatalogStatus::Bundled
        };
        let route_request = RouteRequest {
            explicit_provider: Some(kind),
            model_selector: configured_model.clone().map(LogicalModelRef::from),
            saved_provider_model: None,
            base_url_override: configured_base_url.clone(),
        };

        let mut messages = Vec::new();
        let route = RouteResolver::new().resolve(&route_request);
        let (base_url, supported_protocols, default_route, resolved_pricing, route_ok) = match route
        {
            Ok(candidate) => {
                if !candidate.validation.messages.is_empty() {
                    messages.extend(candidate.validation.messages.clone());
                }
                (
                    candidate.endpoint.base_url,
                    vec![protocol_label(candidate.protocol).to_string()],
                    ProviderDefaultRoute {
                        logical_model: candidate.logical_model.raw().to_string(),
                        wire_model: candidate.wire_model_id.as_str().to_string(),
                    },
                    pricing_label(provider, candidate.pricing.as_ref()),
                    candidate.validation.ok,
                )
            }
            Err(error) => {
                messages.push(format!("route validation failed: {error}"));
                (
                    configured_base_url.unwrap_or_else(|| provider.default_base_url().to_string()),
                    vec![
                        provider
                            .metadata()
                            .map(|metadata| protocol_label(metadata.wire()).to_string())
                            .unwrap_or_else(|| {
                                protocol_label(WireFormat::ChatCompletions).to_string()
                            }),
                    ],
                    ProviderDefaultRoute {
                        logical_model: configured_model.unwrap_or_else(|| "invalid".to_string()),
                        wire_model: "unresolved".to_string(),
                    },
                    usage_meter.clone(),
                    false,
                )
            }
        };

        if matches!(
            auth_status,
            ProviderAuthStatus::Missing | ProviderAuthStatus::OAuthMissing
        ) {
            messages.push(missing_auth_message(provider, configured, &provider_id));
        }
        if catalog_status == ProviderCatalogStatus::DefaultOnly {
            messages.push("catalog snapshot missing; using provider default".to_string());
        }

        let readiness = readiness_for(provider, auth_status, route_ok);
        let reasoning = ProviderReasoningSummary::for_route(provider, &default_route, config);
        let capabilities = ProviderCapabilityBadges::for_route(provider, &default_route.wire_model);

        Self {
            provider,
            provider_id,
            display_name,
            kind: configured
                .and_then(|entry| entry.kind.as_deref())
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("{kind:?}")),
            base_url,
            auth_status,
            catalog_status,
            supported_protocols,
            available_model_count,
            default_route,
            request_concurrency,
            usage_meter: resolved_pricing,
            reasoning,
            capabilities,
            model_origin,
            readiness,
            maturity: ProviderMaturity::for_provider(provider),
            messages,
            is_active,
            has_key,
        }
    }

    fn compact_hint(&self) -> String {
        // Self-hosted providers carry a local/private posture; surface it next
        // to the base URL so the row reads correctly without a key (#3083).
        let self_hosted = if matches!(
            self.auth_status,
            ProviderAuthStatus::Local | ProviderAuthStatus::Optional
        ) {
            " (self-hosted)"
        } else {
            ""
        };
        let request_concurrency = self
            .request_concurrency
            .label()
            .map(|label| format!(" | {label}"))
            .unwrap_or_default();
        format!(
            "{} | auth:{} | {} | {} | base:{}{} | route:{}{} origin:{} | {} | {}{} | catalog:{}{}",
            self.readiness.label(),
            self.auth_status.label(),
            self.usage_meter,
            self.supported_protocols.join("+"),
            compact_base_url(&self.base_url),
            self_hosted,
            self.default_route.logical_model,
            route_wire_suffix(&self.default_route),
            self.model_origin.label(),
            self.capabilities.label(),
            self.reasoning.label(),
            request_concurrency,
            self.catalog_label(),
            // Only experimental integrations add a tag; supported ones stay
            // noise-free (#2984).
            self.maturity
                .tag()
                .map(|tag| format!(" | {tag}"))
                .unwrap_or_default(),
        )
    }

    fn catalog_label(&self) -> String {
        match self.catalog_status {
            ProviderCatalogStatus::Bundled => format!("{} bundled", self.available_model_count),
            ProviderCatalogStatus::DefaultOnly => "default-only".to_string(),
            ProviderCatalogStatus::Legacy => "legacy".to_string(),
        }
    }
}

impl ProviderRequestConcurrencySummary {
    fn for_row(
        provider: ApiProvider,
        config: &Config,
        runtime_status: Option<&ProviderRuntimeStatus>,
        is_active: bool,
    ) -> Self {
        let mut summary = Self {
            limit: config.provider_max_concurrency(provider),
            active: None,
        };
        if is_active
            && let Some(status) = runtime_status
            && status.provider == provider
        {
            summary.limit = status.request_concurrency_limit;
            summary.active = Some(status.active_provider_requests);
        }
        summary
    }

    fn label(self) -> Option<String> {
        match (self.limit, self.active) {
            (Some(limit), Some(active)) => Some(format!("req:{active}/{limit}")),
            (Some(limit), None) => Some(format!("req:cap {limit}")),
            (None, Some(active)) if active > 0 => Some(format!("req:{active}/uncapped")),
            _ => None,
        }
    }
}

impl ProviderReasoningSummary {
    fn for_route(provider: ApiProvider, route: &ProviderDefaultRoute, config: &Config) -> Self {
        if provider == ApiProvider::OpenaiCodex {
            return Self {
                support: ProviderReasoningSupport::Supported,
                controls: codex_reasoning_controls(),
                stream_visibility: ProviderReasoningStreamVisibility::StructuredThinking,
                selected_control: selected_reasoning_control(provider, config),
            };
        }

        if let Some(offering) = reasoning_catalog_offering(provider, route) {
            let support = match offering.reasoning {
                Some(true) => ProviderReasoningSupport::Supported,
                Some(false) => ProviderReasoningSupport::Unsupported,
                None => ProviderReasoningSupport::Unknown,
            };
            let controls = reasoning_controls_from_options(&offering.reasoning_options);
            return Self {
                support,
                controls,
                stream_visibility: configured_or_default_stream_visibility(
                    provider, config, support,
                ),
                selected_control: selected_reasoning_control(provider, config),
            };
        }

        Self::unknown(provider, config)
    }

    fn unknown(provider: ApiProvider, config: &Config) -> Self {
        Self {
            support: ProviderReasoningSupport::Unknown,
            controls: Vec::new(),
            stream_visibility: configured_or_default_stream_visibility(
                provider,
                config,
                ProviderReasoningSupport::Unknown,
            ),
            selected_control: selected_reasoning_control(provider, config),
        }
    }

    fn label(&self) -> String {
        let support = match self.support {
            ProviderReasoningSupport::Supported if !self.controls.is_empty() => {
                format!("reasoning:{}", self.controls.join("/"))
            }
            ProviderReasoningSupport::Supported => "reasoning:yes".to_string(),
            ProviderReasoningSupport::Unsupported => "reasoning:no".to_string(),
            ProviderReasoningSupport::Unknown => "reasoning:unknown".to_string(),
        };
        let mut parts = vec![
            support,
            format!("stream:{}", self.stream_visibility.label()),
        ];
        if let Some(selected) = &self.selected_control {
            parts.push(format!("ctrl:{selected}"));
        }
        parts.join(" ")
    }
}

impl ProviderReasoningStreamVisibility {
    fn label(self) -> &'static str {
        match self {
            Self::StructuredThinking => "structured",
            Self::InlineTags => "inline-tags",
            Self::SummaryOnly => "summary-only",
            Self::NotExposed => "not-exposed",
            Self::Unknown => "unknown",
        }
    }
}

impl ProviderAuthStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Missing => "missing",
            Self::Optional => "optional",
            Self::OAuthReady => "oauth-ready",
            Self::OAuthMissing => "oauth-missing",
            Self::Local => "local",
            Self::Legacy => "legacy",
        }
    }
}

impl ProviderReadiness {
    fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NeedsAuth => "needs-auth",
            Self::LocalReady => "local-ready",
            Self::Legacy => "legacy",
            Self::Invalid => "invalid",
        }
    }
}

fn reasoning_catalog_offering(
    provider: ApiProvider,
    route: &ProviderDefaultRoute,
) -> Option<&'static CatalogOffering> {
    let provider_id = provider.kind()?.as_str();
    bundled_reasoning_catalog()
        .offerings
        .iter()
        .find(|offering| {
            offering.provider == provider_id
                && offering
                    .wire_model_id
                    .eq_ignore_ascii_case(&route.wire_model)
        })
}

fn bundled_reasoning_catalog() -> &'static CatalogSnapshot {
    static CATALOG: OnceLock<CatalogSnapshot> = OnceLock::new();
    CATALOG.get_or_init(|| CatalogSnapshot {
        // Source reasoning descriptors from the single bundled Models.dev
        // snapshot (the same data #3385's catalog layer uses) rather than a
        // hand-maintained per-row seed, so provider reasoning rows (GLM-5.2,
        // etc.) cannot drift from the catalog and every bundled provider with
        // reasoning facts is covered, not just GLM.
        offerings: codewhale_config::catalog::bundled_catalog_offerings(),
    })
}

fn codex_reasoning_controls() -> Vec<String> {
    [
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::Max,
    ]
    .iter()
    .map(|effort| {
        effort
            .display_label_for_provider(ApiProvider::OpenaiCodex)
            .to_string()
    })
    .collect()
}

fn reasoning_controls_from_options(options: &[Value]) -> Vec<String> {
    let mut controls = Vec::new();
    for option in options {
        collect_reasoning_controls(option, &mut controls);
    }
    controls
}

fn collect_reasoning_controls(value: &Value, controls: &mut Vec<String>) {
    match value {
        Value::String(text) => push_reasoning_control(controls, text),
        Value::Array(items) => {
            for item in items {
                collect_reasoning_controls(item, controls);
            }
        }
        Value::Object(map) => {
            if let Some(values) = map.get("values") {
                collect_reasoning_controls(values, controls);
            }
        }
        _ => {}
    }
}

fn push_reasoning_control(controls: &mut Vec<String>, value: &str) {
    let normalized = value.trim();
    if normalized.is_empty() || controls.iter().any(|item| item == normalized) {
        return;
    }
    controls.push(normalized.to_string());
}

fn selected_reasoning_control(provider: ApiProvider, config: &Config) -> Option<String> {
    let effort = ReasoningEffort::from_setting_for_provider(config.reasoning_effort()?, provider);
    Some(effort.display_label_for_provider(provider).to_string())
}

fn configured_or_default_stream_visibility(
    provider: ApiProvider,
    config: &Config,
    support: ProviderReasoningSupport,
) -> ProviderReasoningStreamVisibility {
    if let Some(configured) = config
        .provider_config_for(provider)
        .and_then(|entry| entry.reasoning_stream_style.as_deref())
        && let Some(visibility) = parse_reasoning_stream_visibility(configured)
    {
        return visibility;
    }

    match support {
        ProviderReasoningSupport::Unsupported => ProviderReasoningStreamVisibility::NotExposed,
        ProviderReasoningSupport::Unknown => ProviderReasoningStreamVisibility::Unknown,
        ProviderReasoningSupport::Supported => default_reasoning_stream_visibility(provider),
    }
}

fn parse_reasoning_stream_visibility(value: &str) -> Option<ProviderReasoningStreamVisibility> {
    match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "separate_field" | "separate" | "field" | "structured" | "structured_thinking" => {
            Some(ProviderReasoningStreamVisibility::StructuredThinking)
        }
        "inline_tags" | "inline" | "think_tags" | "thinking_tags" => {
            Some(ProviderReasoningStreamVisibility::InlineTags)
        }
        "summary" | "summary_only" => Some(ProviderReasoningStreamVisibility::SummaryOnly),
        "none" | "text" | "disabled" | "off" | "not_exposed" => {
            Some(ProviderReasoningStreamVisibility::NotExposed)
        }
        _ => None,
    }
}

fn default_reasoning_stream_visibility(provider: ApiProvider) -> ProviderReasoningStreamVisibility {
    match provider {
        ApiProvider::OpenaiCodex
        | ApiProvider::Deepseek
        | ApiProvider::DeepseekCN
        | ApiProvider::NvidiaNim
        | ApiProvider::Openrouter
        | ApiProvider::XiaomiMimo
        | ApiProvider::Novita
        | ApiProvider::Fireworks
        | ApiProvider::Siliconflow
        | ApiProvider::SiliconflowCn
        | ApiProvider::Volcengine
        | ApiProvider::Arcee
        | ApiProvider::Minimax
        | ApiProvider::Sglang
        | ApiProvider::Vllm
        | ApiProvider::Zai
        | ApiProvider::Moonshot => ProviderReasoningStreamVisibility::StructuredThinking,
        _ => ProviderReasoningStreamVisibility::Unknown,
    }
}

fn auth_status_for(
    provider: ApiProvider,
    has_key: bool,
    configured: Option<&crate::config::ProviderConfig>,
) -> ProviderAuthStatus {
    if matches!(provider, ApiProvider::Ollama) {
        return ProviderAuthStatus::Local;
    }
    if matches!(provider, ApiProvider::Sglang | ApiProvider::Vllm) {
        return if has_explicit_credential(provider, configured) {
            ProviderAuthStatus::Configured
        } else {
            ProviderAuthStatus::Optional
        };
    }
    if provider == ApiProvider::Custom {
        return if custom_provider_auth_is_optional(configured) {
            ProviderAuthStatus::Optional
        } else if has_key {
            ProviderAuthStatus::Configured
        } else {
            ProviderAuthStatus::Missing
        };
    }
    if provider == ApiProvider::Moonshot && configured.is_some_and(config_uses_kimi_oauth) {
        return if has_key {
            ProviderAuthStatus::OAuthReady
        } else {
            ProviderAuthStatus::OAuthMissing
        };
    }
    if provider == ApiProvider::OpenaiCodex {
        return if has_key {
            ProviderAuthStatus::OAuthReady
        } else {
            ProviderAuthStatus::OAuthMissing
        };
    }
    if has_key {
        ProviderAuthStatus::Configured
    } else {
        ProviderAuthStatus::Missing
    }
}

fn has_explicit_credential(
    provider: ApiProvider,
    configured: Option<&crate::config::ProviderConfig>,
) -> bool {
    provider
        .env_vars()
        .iter()
        .any(|var| std::env::var(var).is_ok_and(|value| !value.trim().is_empty()))
        || configured.is_some_and(|entry| {
            entry
                .api_key
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                || entry
                    .auth
                    .as_ref()
                    .is_some_and(|auth| auth.validate().is_ok())
        })
}

fn custom_provider_has_auth(configured: Option<&crate::config::ProviderConfig>) -> bool {
    if custom_provider_auth_is_optional(configured) {
        return true;
    }
    configured.is_some_and(|entry| {
        entry
            .api_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || entry
                .api_key_env
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .is_some_and(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()))
            || entry
                .auth
                .as_ref()
                .is_some_and(|auth| auth.validate().is_ok())
    })
}

fn custom_provider_auth_is_optional(configured: Option<&crate::config::ProviderConfig>) -> bool {
    configured.is_some_and(|entry| {
        entry
            .auth_mode
            .as_deref()
            .is_some_and(auth_mode_disables_api_key)
            || entry
                .base_url
                .as_deref()
                .is_some_and(base_url_uses_local_host)
    })
}

fn auth_mode_disables_api_key(mode: &str) -> bool {
    matches!(
        mode.trim()
            .to_ascii_lowercase()
            .replace(['-', ' '], "_")
            .as_str(),
        "none" | "off" | "disabled" | "no_auth" | "noapi" | "no_api_key" | "anonymous"
    )
}

fn base_url_uses_local_host(base_url: &str) -> bool {
    let without_scheme = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    let authority = without_scheme.split('/').next().unwrap_or_default();
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or(authority)
        .trim_start_matches('[')
        .split(']')
        .next()
        .unwrap_or(authority)
        .split(':')
        .next()
        .unwrap_or(authority)
        .to_ascii_lowercase();
    matches!(host.as_str(), "localhost" | "0.0.0.0")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|addr| addr.is_loopback() || addr.is_unspecified())
}

fn missing_auth_message(
    provider: ApiProvider,
    configured: Option<&crate::config::ProviderConfig>,
    provider_id: &str,
) -> String {
    if provider == ApiProvider::Custom {
        if let Some(env_name) = configured
            .and_then(|entry| entry.api_key_env.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            return format!("missing {env_name} for custom provider {provider_id}");
        }
        return format!("missing custom provider auth for {provider_id}");
    }
    format!("missing {}", provider.env_vars_label())
}

fn config_uses_kimi_oauth(config: &crate::config::ProviderConfig) -> bool {
    config.auth_mode.as_deref().is_some_and(|mode| {
        let normalized = mode.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        matches!(normalized.as_str(), "kimi_oauth" | "kimi_cli" | "kimi_code")
    })
}

fn readiness_for(
    provider: ApiProvider,
    auth_status: ProviderAuthStatus,
    route_ok: bool,
) -> ProviderReadiness {
    if provider.kind().is_none() {
        return ProviderReadiness::Legacy;
    }
    if !route_ok {
        return ProviderReadiness::Invalid;
    }
    match auth_status {
        ProviderAuthStatus::Local | ProviderAuthStatus::Optional => ProviderReadiness::LocalReady,
        ProviderAuthStatus::Configured | ProviderAuthStatus::OAuthReady => ProviderReadiness::Ready,
        ProviderAuthStatus::Legacy => ProviderReadiness::Legacy,
        ProviderAuthStatus::Missing | ProviderAuthStatus::OAuthMissing => {
            ProviderReadiness::NeedsAuth
        }
    }
}

fn usage_meter_for(provider: ApiProvider) -> String {
    match provider {
        ApiProvider::Ollama | ApiProvider::Sglang | ApiProvider::Vllm => "cost: local".to_string(),
        ApiProvider::OpenaiCodex => "usage: Codex OAuth quota".to_string(),
        ApiProvider::Moonshot if kimi_cli_credentials_present() => {
            "usage: Kimi OAuth quota".to_string()
        }
        ApiProvider::XiaomiMimo => "cost: token-plan".to_string(),
        _ => "cost: unknown".to_string(),
    }
}

fn pricing_label(provider: ApiProvider, pricing: Option<&PricingSku>) -> String {
    match pricing {
        Some(PricingSku::Token {
            input_per_mtok,
            output_per_mtok,
        }) => match (input_per_mtok, output_per_mtok) {
            (Some(input), Some(output)) => format!("cost: ${input:.2}/${output:.2} mtok"),
            _ => "cost: token".to_string(),
        },
        Some(PricingSku::SubscriptionQuota { used_pct, .. }) => used_pct.map_or_else(
            || "usage: subscription quota".to_string(),
            |pct| format!("usage: subscription {pct:.0}%"),
        ),
        Some(PricingSku::AccountCredits { balance }) => balance.map_or_else(
            || "usage: account credits".to_string(),
            |balance| format!("usage: ${balance:.2} credits"),
        ),
        Some(PricingSku::LocalOrNotApplicable) => "cost: local".to_string(),
        Some(PricingSku::UnknownOrStale) | None => usage_meter_for(provider),
    }
}

fn protocol_label(protocol: RequestProtocol) -> &'static str {
    match protocol {
        WireFormat::ChatCompletions => "chat",
        WireFormat::Responses => "responses",
        WireFormat::AnthropicMessages => "anthropic",
    }
}

fn route_wire_suffix(route: &ProviderDefaultRoute) -> String {
    if route.logical_model == route.wire_model {
        String::new()
    } else {
        format!(" -> {}", route.wire_model)
    }
}

/// Strip the scheme and trailing slash, then cap the length so one long base
/// URL can't dominate (and overflow) the provider hint row. Capped values get
/// an ellipsis; short URLs pass through unchanged.
fn compact_base_url(base_url: &str) -> String {
    let stripped = base_url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    crate::tui::ui_text::truncate_line_to_width(stripped, 24)
}

impl ProviderPickerView {
    #[cfg(test)]
    #[must_use]
    pub fn new(active: ApiProvider, config: &Config) -> Self {
        Self::new_with_runtime_status(active, config, None)
    }

    #[must_use]
    pub fn new_with_runtime_status(
        active: ApiProvider,
        config: &Config,
        runtime_status: Option<ProviderRuntimeStatus>,
    ) -> Self {
        // Present providers in the shared metadata display order (#3076). The
        // active provider is highlighted via `selected_idx` below, so it is
        // never lost in the list.
        let runtime_status = runtime_status.as_ref();
        let custom_rows = custom_provider_dashboard_rows(active, config, runtime_status);
        let mut rows: Vec<ProviderDashboardRow> = ApiProvider::sorted_for_display()
            .into_iter()
            .filter(|provider| *provider != ApiProvider::Custom || custom_rows.is_empty())
            .map(|p| {
                ProviderDashboardRow::from_config_with_runtime_status(
                    p,
                    active,
                    config,
                    runtime_status,
                )
            })
            .collect();
        rows.extend(custom_rows);
        rows.sort_by(|a, b| {
            a.display_name
                .to_ascii_lowercase()
                .cmp(&b.display_name.to_ascii_lowercase())
                .then_with(|| a.provider_id.cmp(&b.provider_id))
        });
        let selected_idx = rows
            .iter()
            .position(|row| row.is_active)
            .or_else(|| rows.iter().position(|row| row.provider == active))
            .unwrap_or(0);
        Self {
            rows,
            selected_idx,
            stage: Stage::List,
            api_key_input: String::new(),
        }
    }

    fn move_up(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        if self.selected_idx == 0 {
            self.selected_idx = self.rows.len() - 1;
        } else {
            self.selected_idx -= 1;
        }
    }

    fn move_down(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        if self.selected_idx + 1 == self.rows.len() {
            self.selected_idx = 0;
        } else {
            self.selected_idx += 1;
        }
    }

    /// Type-ahead: move the selection to the next provider whose display name
    /// starts with the given character (case-insensitive), wrapping so repeated
    /// presses cycle through matches — e.g. pressing `z` jumps to "Z.ai".
    fn jump_to_letter(&mut self, c: char) {
        let count = self.rows.len();
        if count == 0 {
            return;
        }
        let target = c.to_ascii_lowercase();
        for offset in 1..=count {
            let idx = (self.selected_idx + offset) % count;
            if self.rows[idx]
                .display_name
                .to_ascii_lowercase()
                .starts_with(target)
            {
                self.selected_idx = idx;
                return;
            }
        }
    }

    fn selected_provider(&self) -> ApiProvider {
        self.rows[self.selected_idx].provider
    }

    fn selected_has_key(&self) -> bool {
        self.rows[self.selected_idx].has_key
    }

    fn enter_key_entry(&mut self) {
        self.stage = Stage::KeyEntry;
        self.api_key_input.clear();
    }

    fn env_var_for(provider: ApiProvider) -> String {
        provider.env_vars_label()
    }

    fn env_var_for_selected_row(&self) -> String {
        let row = &self.rows[self.selected_idx];
        if row.provider == ApiProvider::Custom {
            return row
                .messages
                .iter()
                .find_map(|message| {
                    message
                        .strip_prefix("missing ")
                        .and_then(|rest| rest.split_once(" for custom provider"))
                        .map(|(env_name, _)| env_name.to_string())
                })
                .unwrap_or_else(|| format!("[providers.{}] api_key", row.provider_id));
        }
        Self::env_var_for(row.provider)
    }

    fn visible_start(&self, visible_rows: usize) -> usize {
        if visible_rows == 0 {
            return 0;
        }
        let max_start = self.rows.len().saturating_sub(visible_rows);
        self.selected_idx
            .saturating_add(1)
            .saturating_sub(visible_rows)
            .min(max_start)
    }

    fn selected_row_style(fg: Color) -> Style {
        Style::default()
            .fg(fg)
            .bg(palette::SURFACE_ELEVATED)
            .add_modifier(Modifier::BOLD)
    }

    fn selected_row_bg_style() -> Style {
        Style::default().bg(palette::SURFACE_ELEVATED)
    }

    fn render_list(&self, area: Rect, buf: &mut Buffer) {
        let enter_action = if self.selected_has_key() {
            "apply"
        } else {
            "set key"
        };
        let outer = Block::default()
            .title(Line::from(Span::styled(
                " Provider ",
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            )))
            .title_bottom(Line::from(vec![
                Span::styled(" ↑↓ ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("move "),
                Span::styled(" a-z ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("jump "),
                Span::styled(" Enter ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw(format!("{enter_action} ")),
                Span::styled(" R ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("edit key "),
                Span::styled(" M ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("models "),
                Span::styled(" Esc ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("cancel "),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default());
        let inner = outer.inner(area);
        outer.render(area, buf);

        let visible_rows = usize::from(inner.height);
        let visible_start = self.visible_start(visible_rows);
        let mut lines: Vec<Line> = Vec::with_capacity(visible_rows);
        for (idx, row) in self
            .rows
            .iter()
            .enumerate()
            .skip(visible_start)
            .take(visible_rows)
        {
            let is_selected = idx == self.selected_idx;
            let is_active = row.is_active;
            let arrow = if is_selected { "▸" } else { " " };
            let active_dot = if is_active { " *" } else { "  " };
            let spacer_style = if is_selected {
                Self::selected_row_bg_style()
            } else {
                Style::default()
            };
            let label_style = if is_selected {
                Self::selected_row_style(palette::TEXT_PRIMARY)
            } else {
                Style::default().fg(palette::TEXT_PRIMARY)
            };
            let hint_style = if is_selected {
                let hint_fg = if row.has_key {
                    palette::TEXT_MUTED
                } else {
                    palette::STATUS_WARNING
                };
                Self::selected_row_style(hint_fg)
            } else if row.has_key {
                Style::default().fg(palette::TEXT_MUTED)
            } else {
                Style::default().fg(palette::STATUS_WARNING)
            };
            let hint = row.compact_hint();
            let mut line = Line::from(vec![
                Span::styled(" ", spacer_style),
                Span::styled(arrow, label_style),
                Span::styled(" ", spacer_style),
                Span::styled(row.display_name.as_str(), label_style),
                Span::styled(active_dot, label_style),
                Span::styled("  ", spacer_style),
                Span::styled(hint, hint_style),
            ]);
            if is_selected {
                line.style = Self::selected_row_bg_style();
                let target_width = usize::from(inner.width);
                let line_width = line.width();
                if line_width < target_width {
                    line.spans.push(Span::styled(
                        " ".repeat(target_width - line_width),
                        Self::selected_row_bg_style(),
                    ));
                }
            }
            lines.push(line);
        }
        Paragraph::new(lines).render(inner, buf);
    }

    fn render_key_entry(&self, area: Rect, buf: &mut Buffer) {
        let row = &self.rows[self.selected_idx];
        let outer = Block::default()
            .title(Line::from(Span::styled(
                format!(" API key — {} ", row.display_name),
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            )))
            .title_bottom(Line::from(vec![
                Span::styled(" Enter ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("save & switch "),
                Span::styled(" Esc ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("back "),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default());
        let inner = outer.inner(area);
        outer.render(area, buf);

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .split(inner);

        let masked = mask_key(&self.api_key_input);
        let display = if masked.is_empty() {
            "(paste key here)".to_string()
        } else {
            masked
        };
        let key_lines = vec![Line::from(vec![
            Span::styled("Key: ", Style::default().fg(palette::TEXT_MUTED)),
            Span::styled(
                display,
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
        ])];
        Paragraph::new(key_lines).render(layout[0], buf);

        let hint = format!(
            "Or set the {} environment variable and re-open /provider.",
            self.env_var_for_selected_row(),
        );
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(palette::TEXT_MUTED),
        )))
        .render(layout[1], buf);
    }
}

fn mask_key(input: &str) -> String {
    let trimmed = input.trim();
    let len = trimmed.chars().count();
    if len == 0 {
        return String::new();
    }
    if len <= 4 {
        return "*".repeat(len);
    }
    let visible: String = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{}{}", "*".repeat(len - 4), visible)
}

impl ModalView for ProviderPickerView {
    fn kind(&self) -> ModalKind {
        ModalKind::ProviderPicker
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        if self.stage == Stage::KeyEntry {
            let sanitized: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            if !sanitized.is_empty() {
                self.api_key_input.push_str(&sanitized);
            }
            true
        } else {
            false
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match self.stage {
            Stage::List => match key.code {
                KeyCode::Esc => ViewAction::Close,
                KeyCode::Up => {
                    self.move_up();
                    ViewAction::None
                }
                KeyCode::Down => {
                    self.move_down();
                    ViewAction::None
                }
                KeyCode::Enter => {
                    let provider = self.selected_provider();
                    if self.selected_has_key() {
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied { provider })
                    } else if provider == ApiProvider::Moonshot && kimi_cli_credentials_present() {
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerKimiOAuthEnabled {
                            provider,
                        })
                    } else {
                        self.enter_key_entry();
                        ViewAction::None
                    }
                }
                KeyCode::Char(c) if key.modifiers.is_empty() && c.eq_ignore_ascii_case(&'r') => {
                    self.enter_key_entry();
                    ViewAction::None
                }
                // Jump to the `/model` picker pre-filtered to this provider
                // (#3083). Handled before the type-ahead arm so `m`/`M` opens
                // models instead of seeking a provider whose name starts with m.
                KeyCode::Char(c) if key.modifiers.is_empty() && c.eq_ignore_ascii_case(&'m') => {
                    let provider = self.selected_provider();
                    ViewAction::EmitAndClose(ViewEvent::ProviderPickerOpenModels { provider })
                }
                // Type-ahead: any other letter jumps to the next provider whose
                // name starts with it (e.g. `z` -> "Z.ai").
                KeyCode::Char(c) if key.modifiers.is_empty() && c.is_ascii_alphabetic() => {
                    self.jump_to_letter(c);
                    ViewAction::None
                }
                _ => ViewAction::None,
            },
            Stage::KeyEntry => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::List;
                    self.api_key_input.clear();
                    ViewAction::None
                }
                KeyCode::Backspace => {
                    self.api_key_input.pop();
                    ViewAction::None
                }
                KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.api_key_input.pop();
                    ViewAction::None
                }
                KeyCode::Enter => {
                    let key = self.api_key_input.trim().to_string();
                    if key.is_empty() {
                        // Stay in key-entry; the user can press Esc to abort.
                        ViewAction::None
                    } else {
                        let provider = self.selected_provider();
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerApiKeySubmitted {
                            provider,
                            api_key: key,
                        })
                    }
                }
                KeyCode::Char(c) => {
                    // Reject ASCII whitespace so a stray space/tab doesn't slip
                    // into a credential; bracketed paste happens via the input
                    // path that already trims on submit.
                    if !c.is_whitespace() {
                        self.api_key_input.push(c);
                    }
                    ViewAction::None
                }
                _ => ViewAction::None,
            },
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        if self.stage == Stage::List {
            match mouse.kind {
                MouseEventKind::ScrollUp => self.move_up(),
                MouseEventKind::ScrollDown => self.move_down(),
                _ => {}
            }
        }
        ViewAction::None
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_width = 120.min(area.width.saturating_sub(4)).max(64);
        let popup_height = match self.stage {
            Stage::List => (self.rows.len() as u16).saturating_add(2),
            Stage::KeyEntry => 10,
        }
        .min(area.height.saturating_sub(4))
        .max(8);
        let popup_area = Rect {
            x: area.x + (area.width.saturating_sub(popup_width)) / 2,
            y: area.y + (area.height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };

        Clear.render(popup_area, buf);

        match self.stage {
            Stage::List => self.render_list(popup_area, buf),
            Stage::KeyEntry => self.render_key_entry(popup_area, buf),
        }
    }
}

fn custom_provider_dashboard_rows(
    active: ApiProvider,
    config: &Config,
    runtime_status: Option<&ProviderRuntimeStatus>,
) -> Vec<ProviderDashboardRow> {
    let Some(providers) = config.providers.as_ref() else {
        return Vec::new();
    };
    let mut ids: Vec<_> = providers.custom.keys().cloned().collect();
    ids.sort_by_key(|id| id.to_ascii_lowercase());
    ids.into_iter()
        .filter(|id| {
            providers
                .custom_provider_config(id)
                .is_some_and(|entry| entry.is_openai_compatible_custom())
        })
        .map(|id| {
            ProviderDashboardRow::from_custom_config_with_runtime_status(
                &id,
                active,
                config,
                runtime_status,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use std::env;
    use std::ffi::OsString;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn remove(key: &'static str) -> Self {
            let previous = env::var_os(key);
            // SAFETY: provider-picker tests that mutate environment variables
            // hold ENV_LOCK for the whole guard lifetime, so no sibling test in
            // this module can concurrently mutate/read this provider key.
            unsafe {
                env::remove_var(key);
            }
            Self { key, previous }
        }

        fn set(key: &'static str, value: &str) -> Self {
            let previous = env::var_os(key);
            // SAFETY: provider-picker tests that mutate environment variables
            // hold ENV_LOCK for the whole guard lifetime, so no sibling test in
            // this module can concurrently mutate/read this provider key.
            unsafe {
                env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: EnvVarGuard is used while ENV_LOCK is held; declaration
            // order in the test drops the guard before releasing the lock.
            unsafe {
                match self.previous.take() {
                    Some(value) => env::set_var(self.key, value),
                    None => env::remove_var(self.key),
                }
            }
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn move_to_provider(picker: &mut ProviderPickerView, provider: ApiProvider) {
        let max_steps = picker.rows.len();
        for _ in 0..max_steps {
            if picker.selected_provider() == provider {
                return;
            }
            picker.handle_key(key(KeyCode::Down));
        }
        panic!("provider {provider:?} not found in picker");
    }

    fn render_text(picker: &ProviderPickerView, width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn type_ahead_jumps_to_provider_by_first_letter() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        // `z` is unique to Z.ai among provider display names.
        picker.handle_key(key(KeyCode::Char('z')));
        assert_eq!(picker.selected_provider(), ApiProvider::Zai);
    }

    #[test]
    fn compact_base_url_strips_scheme_and_caps_length() {
        // Short URLs pass through unchanged (scheme + trailing slash stripped).
        assert_eq!(
            compact_base_url("https://api.deepseek.com/"),
            "api.deepseek.com"
        );
        assert_eq!(
            compact_base_url("http://localhost:9000/v1"),
            "localhost:9000/v1"
        );
        // A long URL is capped so it can't dominate the hint row.
        let long = compact_base_url("https://api-us-west-2.example-region.company.com/v1/openai");
        assert!(long.ends_with("..."), "expected an ellipsis, got {long:?}");
        assert!(
            long.chars().count() <= 24,
            "capped to 24 cols, got {long:?}"
        );
    }

    #[test]
    fn mouse_scroll_moves_selection_in_list_stage() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let before = picker.selected_idx;
        picker.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_ne!(
            picker.selected_idx, before,
            "scroll down should advance the selection"
        );
    }

    #[test]
    fn picker_lists_all_providers() {
        let config = Config::default();
        let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let names: Vec<_> = picker
            .rows
            .iter()
            .map(|row| row.display_name.as_str())
            .collect();

        // Every built-in provider is present, none dropped (#3076 reorders, it
        // does not filter).
        assert_eq!(names.len(), ApiProvider::all().len());
        assert!(names.contains(&"DeepSeek"));

        // Providers are presented in neutral case-insensitive alphabetical
        // order by display name (#3076), not `ApiProvider::all()` order.
        let mut expected = names.clone();
        expected.sort_by_key(|name| name.to_ascii_lowercase());
        assert_eq!(
            names, expected,
            "provider picker must list providers in case-insensitive alphabetical order"
        );
        // DeepSeek is no longer hard-coded first.
        assert_ne!(names.first(), Some(&"DeepSeek"));
    }

    #[test]
    fn key_entry_hint_uses_metadata_env_vars() {
        assert_eq!(
            ProviderPickerView::env_var_for(ApiProvider::NvidiaNim),
            "NVIDIA_API_KEY / NVIDIA_NIM_API_KEY / DEEPSEEK_API_KEY"
        );
    }

    #[test]
    fn provider_dashboard_row_models_local_readiness_without_rendering() {
        let config = Config::default();
        let row =
            ProviderDashboardRow::from_config(ApiProvider::Ollama, ApiProvider::Ollama, &config);

        assert_eq!(row.provider_id, "ollama");
        assert_eq!(row.auth_status, ProviderAuthStatus::Local);
        assert_eq!(row.readiness, ProviderReadiness::LocalReady);
        assert_eq!(row.supported_protocols, vec!["chat".to_string()]);
        assert_eq!(row.usage_meter, "cost: local");
        assert!(row.base_url.contains("localhost:11434"));
        assert!(row.is_active);
    }

    #[test]
    fn openai_codex_row_is_experimental_and_tagged_in_hint() {
        let config = Config::default();
        let row = ProviderDashboardRow::from_config(
            ApiProvider::OpenaiCodex,
            ApiProvider::Deepseek,
            &config,
        );

        // #2984: maturity is a separate axis from auth/readiness.
        assert_eq!(row.maturity, ProviderMaturity::Experimental);
        assert!(
            row.compact_hint().contains("experimental"),
            "experimental maturity must surface in the hint, got {:?}",
            row.compact_hint()
        );
    }

    #[test]
    fn mainstream_provider_is_supported_without_experimental_tag() {
        let config = Config::default();
        let row = ProviderDashboardRow::from_config(
            ApiProvider::Deepseek,
            ApiProvider::Deepseek,
            &config,
        );

        // #2984: supported integrations stay noise-free (no tag).
        assert_eq!(row.maturity, ProviderMaturity::Supported);
        assert!(
            !row.compact_hint().contains("experimental"),
            "supported providers must omit the experimental tag, got {:?}",
            row.compact_hint()
        );
    }

    #[test]
    fn provider_dashboard_row_surfaces_glm_reasoning_controls() {
        let config = Config {
            reasoning_effort: Some("max".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                zai: crate::config::ProviderConfig {
                    api_key: Some("zai-key".to_string()),
                    model: Some("GLM-5.2".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let row = ProviderDashboardRow::from_config(ApiProvider::Zai, ApiProvider::Zai, &config);

        assert_eq!(row.default_route.wire_model, "GLM-5.2");
        assert_eq!(row.reasoning.support, ProviderReasoningSupport::Supported);
        assert_eq!(
            row.reasoning.controls,
            vec!["high".to_string(), "max".to_string()]
        );
        assert_eq!(
            row.reasoning.stream_visibility,
            ProviderReasoningStreamVisibility::StructuredThinking
        );
        assert_eq!(row.reasoning.selected_control.as_deref(), Some("max"));
        assert!(row.compact_hint().contains("reasoning:high/max"));
        assert!(row.compact_hint().contains("stream:structured"));
    }

    #[test]
    fn provider_dashboard_row_surfaces_zai_concurrency_cap() {
        let config = Config::default();
        let row =
            ProviderDashboardRow::from_config(ApiProvider::Zai, ApiProvider::Deepseek, &config);

        assert_eq!(
            row.request_concurrency.limit,
            Some(crate::config::DEFAULT_ZAI_PROVIDER_MAX_CONCURRENCY)
        );
        assert_eq!(row.request_concurrency.active, None);
        assert!(
            row.compact_hint().contains("req:cap 3"),
            "Z.ai's effective default cap must surface in /provider, got {:?}",
            row.compact_hint()
        );
    }

    #[test]
    fn provider_dashboard_row_surfaces_active_provider_requests() {
        let config = Config::default();
        let runtime_status = ProviderRuntimeStatus {
            provider: ApiProvider::Zai,
            request_concurrency_limit: Some(crate::config::DEFAULT_ZAI_PROVIDER_MAX_CONCURRENCY),
            active_provider_requests: 2,
        };
        let mut picker = ProviderPickerView::new_with_runtime_status(
            ApiProvider::Zai,
            &config,
            Some(runtime_status),
        );

        move_to_provider(&mut picker, ApiProvider::Zai);
        let row = &picker.rows[picker.selected_idx];

        assert_eq!(
            row.request_concurrency.limit,
            Some(crate::config::DEFAULT_ZAI_PROVIDER_MAX_CONCURRENCY)
        );
        assert_eq!(row.request_concurrency.active, Some(2));
        assert!(
            row.compact_hint().contains("req:2/3"),
            "active runtime concurrency must surface in /provider, got {:?}",
            row.compact_hint()
        );
    }

    #[test]
    fn provider_dashboard_row_surfaces_codex_reasoning_scale() {
        let config = Config {
            reasoning_effort: Some("max".to_string()),
            ..Config::default()
        };
        let row = ProviderDashboardRow::from_config(
            ApiProvider::OpenaiCodex,
            ApiProvider::OpenaiCodex,
            &config,
        );

        assert_eq!(row.reasoning.support, ProviderReasoningSupport::Supported);
        assert_eq!(
            row.reasoning.controls,
            vec![
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
                "xhigh".to_string(),
            ]
        );
        assert_eq!(
            row.reasoning.stream_visibility,
            ProviderReasoningStreamVisibility::StructuredThinking
        );
        assert_eq!(row.reasoning.selected_control.as_deref(), Some("xhigh"));
        assert!(
            row.compact_hint()
                .contains("reasoning:low/medium/high/xhigh")
        );
    }

    #[test]
    fn provider_dashboard_row_surfaces_capability_and_metadata_badges() {
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                deepseek: crate::config::ProviderConfig {
                    api_key: Some("deepseek-key".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let row = ProviderDashboardRow::from_config(
            ApiProvider::Deepseek,
            ApiProvider::Deepseek,
            &config,
        );

        // Metadata badges are projected from the resolved capability profile,
        // never hardcoded per UI surface.
        assert!(row.capabilities.context_window.is_some());
        assert!(row.capabilities.max_output.is_some());
        let hint = row.compact_hint();
        assert!(hint.contains("ctx:"), "metadata badge missing: {hint}");
        assert!(hint.contains("out:"), "metadata badge missing: {hint}");
        // Capability cluster present (tri-state; unknown renders `?`, never
        // silently omitted).
        for badge in ["tools:", "json:", "stream:", "cache:"] {
            assert!(
                hint.contains(badge),
                "capability badge {badge} missing: {hint}"
            );
        }
    }

    #[test]
    fn provider_dashboard_row_classifies_model_origin() {
        // Default: no configured model override.
        let config = Config::default();
        let row = ProviderDashboardRow::from_config(
            ApiProvider::Deepseek,
            ApiProvider::Deepseek,
            &config,
        );
        assert_eq!(row.model_origin, ProviderModelOrigin::Default);
        assert!(row.compact_hint().contains("origin:default"));

        // Saved: a configured model override for the provider.
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                deepseek: crate::config::ProviderConfig {
                    api_key: Some("k".to_string()),
                    model: Some("deepseek-v4-flash".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let row = ProviderDashboardRow::from_config(
            ApiProvider::Deepseek,
            ApiProvider::Deepseek,
            &config,
        );
        assert_eq!(row.model_origin, ProviderModelOrigin::Saved);
        assert!(row.compact_hint().contains("origin:saved"));
    }

    #[test]
    fn model_origin_classifier_covers_default_saved_custom() {
        assert_eq!(
            ProviderModelOrigin::for_provider(ApiProvider::Deepseek, false),
            ProviderModelOrigin::Default
        );
        assert_eq!(
            ProviderModelOrigin::for_provider(ApiProvider::Deepseek, true),
            ProviderModelOrigin::Saved
        );
        assert_eq!(
            ProviderModelOrigin::for_provider(ApiProvider::Custom, false),
            ProviderModelOrigin::Custom
        );
        // An explicit saved model still wins for a custom provider.
        assert_eq!(
            ProviderModelOrigin::for_provider(ApiProvider::Custom, true),
            ProviderModelOrigin::Saved
        );
    }

    #[test]
    fn self_hosted_provider_row_marks_self_hosted_in_hint() {
        let config = Config::default();
        let row =
            ProviderDashboardRow::from_config(ApiProvider::Ollama, ApiProvider::Ollama, &config);
        assert_eq!(row.auth_status, ProviderAuthStatus::Local);
        assert!(
            row.compact_hint().contains("(self-hosted)"),
            "self-hosted hint missing: {}",
            row.compact_hint()
        );

        let sglang =
            ProviderDashboardRow::from_config(ApiProvider::Sglang, ApiProvider::Sglang, &config);
        assert_eq!(sglang.auth_status, ProviderAuthStatus::Optional);
        assert!(
            sglang.compact_hint().contains("(self-hosted)"),
            "self-hosted hint missing for SGLang: {}",
            sglang.compact_hint()
        );
    }

    #[test]
    fn self_hosted_reasoning_visibility_covers_vllm() {
        assert_eq!(
            default_reasoning_stream_visibility(ApiProvider::Sglang),
            ProviderReasoningStreamVisibility::StructuredThinking
        );
        assert_eq!(
            default_reasoning_stream_visibility(ApiProvider::Vllm),
            ProviderReasoningStreamVisibility::StructuredThinking
        );
    }

    #[test]
    fn humanize_token_count_is_compact_and_marks_unknown() {
        assert_eq!(humanize_token_count(None), "?");
        assert_eq!(humanize_token_count(Some(1_000_000)), "1M");
        assert_eq!(humanize_token_count(Some(1_500_000)), "1.5M");
        assert_eq!(humanize_token_count(Some(131_072)), "131K");
        assert_eq!(humanize_token_count(Some(512)), "512");
    }

    #[test]
    fn provider_dashboard_row_uses_route_resolver_for_custom_openai_endpoint() {
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                openai: crate::config::ProviderConfig {
                    api_key: Some("openai-key".to_string()),
                    base_url: Some("http://localhost:9000/v1".to_string()),
                    model: Some("custom-model".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let row =
            ProviderDashboardRow::from_config(ApiProvider::Openai, ApiProvider::Openai, &config);

        assert_eq!(row.provider_id, "openai");
        assert_eq!(row.auth_status, ProviderAuthStatus::Configured);
        assert_eq!(row.readiness, ProviderReadiness::Ready);
        assert_eq!(row.base_url, "http://localhost:9000/v1");
        assert_eq!(row.default_route.logical_model, "custom-model");
        assert_eq!(row.default_route.wire_model, "custom-model");
        assert_eq!(row.supported_protocols, vec!["chat".to_string()]);
    }

    #[test]
    fn provider_picker_lists_configured_custom_provider_readiness() {
        let _lock = ENV_LOCK.lock().expect("env lock poisoned");
        let _example_key = EnvVarGuard::remove("EXAMPLE_API_KEY");
        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "my_thing".to_string(),
            crate::config::ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("https://api.example.com/v1".to_string()),
                model: Some("vendor/custom-model-v1".to_string()),
                api_key_env: Some("EXAMPLE_API_KEY".to_string()),
                ..Default::default()
            },
        );
        let config = Config {
            provider: Some("my_thing".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Config::default()
        };

        let picker = ProviderPickerView::new(ApiProvider::Custom, &config);
        let row = picker
            .rows
            .iter()
            .find(|row| row.provider_id == "my_thing")
            .expect("configured custom provider row");

        assert_eq!(row.provider, ApiProvider::Custom);
        assert_eq!(row.display_name, "my_thing (custom)");
        assert_eq!(row.kind, "openai-compatible");
        assert!(row.is_active);
        assert_eq!(row.auth_status, ProviderAuthStatus::Missing);
        assert_eq!(row.readiness, ProviderReadiness::NeedsAuth);
        assert_eq!(row.base_url, "https://api.example.com/v1");
        assert_eq!(row.supported_protocols, vec!["chat".to_string()]);
        assert_eq!(row.default_route.logical_model, "vendor/custom-model-v1");
        assert_eq!(row.default_route.wire_model, "vendor/custom-model-v1");
        assert_eq!(row.model_origin, ProviderModelOrigin::Saved);
        assert!(
            row.messages
                .iter()
                .any(|message| message.contains("EXAMPLE_API_KEY")),
            "custom row should name the configured auth env var: {:?}",
            row.messages
        );
        assert_eq!(picker.rows[picker.selected_idx].provider_id, "my_thing");
    }

    #[test]
    fn provider_picker_marks_custom_provider_ready_when_env_auth_is_set() {
        let _lock = ENV_LOCK.lock().expect("env lock poisoned");
        let _example_key = EnvVarGuard::set("EXAMPLE_API_KEY", "sk-test");
        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "my_thing".to_string(),
            crate::config::ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("https://api.example.com/v1".to_string()),
                model: Some("custom-model-v1".to_string()),
                api_key_env: Some("EXAMPLE_API_KEY".to_string()),
                ..Default::default()
            },
        );
        let config = Config {
            provider: Some("my_thing".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Config::default()
        };

        let picker = ProviderPickerView::new(ApiProvider::Custom, &config);
        let row = picker
            .rows
            .iter()
            .find(|row| row.provider_id == "my_thing")
            .expect("configured custom provider row");

        assert_eq!(row.auth_status, ProviderAuthStatus::Configured);
        assert_eq!(row.readiness, ProviderReadiness::Ready);
        assert!(row.has_key);
        assert!(
            !row.messages
                .iter()
                .any(|message| message.contains("EXAMPLE_API_KEY")),
            "configured custom auth should not report missing env var: {:?}",
            row.messages
        );
    }

    #[test]
    fn provider_dashboard_row_surfaces_anthropic_wire_protocol() {
        let config = Config::default();
        let row = ProviderDashboardRow::from_config(
            ApiProvider::Anthropic,
            ApiProvider::Deepseek,
            &config,
        );

        assert_eq!(row.provider_id, "anthropic");
        assert_eq!(row.supported_protocols, vec!["anthropic".to_string()]);
        assert_eq!(row.catalog_status, ProviderCatalogStatus::DefaultOnly);
        assert!(
            row.messages
                .iter()
                .any(|message| message.contains("catalog"))
        );
    }

    #[test]
    fn provider_dashboard_row_surfaces_openmodel_messages_route() {
        let _lock = ENV_LOCK.lock().expect("env lock poisoned");
        let _openmodel_key = EnvVarGuard::remove("OPENMODEL_API_KEY");
        let config = Config::default();
        let row = ProviderDashboardRow::from_config(
            ApiProvider::Openmodel,
            ApiProvider::Deepseek,
            &config,
        );

        assert_eq!(row.provider_id, "openmodel");
        assert_eq!(row.display_name, "OpenModel");
        assert_eq!(row.auth_status, ProviderAuthStatus::Missing);
        assert_eq!(row.readiness, ProviderReadiness::NeedsAuth);
        assert_eq!(row.supported_protocols, vec!["anthropic".to_string()]);
        assert_eq!(row.base_url, crate::config::DEFAULT_OPENMODEL_BASE_URL);
        assert_eq!(row.default_route.logical_model, "deepseek-v4-flash");
        assert_eq!(row.default_route.wire_model, "deepseek-v4-flash");
        assert!(
            row.messages
                .iter()
                .any(|message| message.contains("missing OPENMODEL_API_KEY"))
        );
    }

    #[test]
    fn provider_dashboard_row_marks_missing_hosted_auth_as_needs_auth() {
        let _lock = ENV_LOCK.lock().expect("env lock poisoned");
        let _openrouter_key = EnvVarGuard::remove("OPENROUTER_API_KEY");
        let config = Config::default();
        let row = ProviderDashboardRow::from_config(
            ApiProvider::Openrouter,
            ApiProvider::Deepseek,
            &config,
        );

        assert_eq!(row.auth_status, ProviderAuthStatus::Missing);
        assert_eq!(row.readiness, ProviderReadiness::NeedsAuth);
        assert!(
            row.messages
                .iter()
                .any(|message| message.contains("missing OPENROUTER_API_KEY"))
        );
    }

    #[test]
    fn provider_dashboard_row_marks_route_resolver_errors_as_invalid() {
        let config = Config {
            api_key: Some("deepseek-key".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                deepseek: crate::config::ProviderConfig {
                    model: Some("anthropic/claude-foreign".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let row = ProviderDashboardRow::from_config(
            ApiProvider::Deepseek,
            ApiProvider::Deepseek,
            &config,
        );

        assert_eq!(row.auth_status, ProviderAuthStatus::Configured);
        assert_eq!(row.readiness, ProviderReadiness::Invalid);
        assert_eq!(row.default_route.wire_model, "unresolved");
        assert!(
            row.messages
                .iter()
                .any(|message| message.contains("route validation failed"))
        );
    }

    #[test]
    fn provider_dashboard_render_includes_route_protocol_usage_and_base_url() {
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                openai: crate::config::ProviderConfig {
                    api_key: Some("openai-key".to_string()),
                    base_url: Some("http://localhost:9000/v1".to_string()),
                    model: Some("custom-model".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let picker = ProviderPickerView::new(ApiProvider::Openai, &config);

        let rendered = render_text(&picker, 124, 18);

        assert!(rendered.contains("auth:configured"));
        assert!(rendered.contains("route:custom-model"));
        assert!(rendered.contains("chat"));
        assert!(rendered.contains("cost: unknown"));
        assert!(rendered.contains("localhost:9000/v1"));
    }

    #[test]
    fn ollama_is_selectable_without_key() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Ollama);
        assert_eq!(picker.selected_provider(), ApiProvider::Ollama);
        assert!(picker.selected_has_key());
        let action = picker.handle_key(key(KeyCode::Enter));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied { provider }) => {
                assert_eq!(provider, ApiProvider::Ollama);
            }
            other => panic!("expected ProviderPickerApplied, got {other:?}"),
        }
    }

    #[test]
    fn pressing_m_opens_models_for_selected_provider() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Openrouter);

        let action = picker.handle_key(key(KeyCode::Char('m')));

        // #3083: `m` jumps to the model picker scoped to the highlighted
        // provider rather than acting as a type-ahead seek.
        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerOpenModels { provider }) => {
                assert_eq!(provider, ApiProvider::Openrouter);
            }
            other => panic!("expected ProviderPickerOpenModels, got {other:?}"),
        }
    }

    #[test]
    fn pressing_uppercase_m_also_opens_models() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);

        // Case-insensitive like the `R` edit-key affordance: a bare `M` works.
        let action = picker.handle_key(key(KeyCode::Char('M')));

        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerOpenModels { provider }) => {
                assert_eq!(provider, ApiProvider::Deepseek);
            }
            other => panic!("expected ProviderPickerOpenModels, got {other:?}"),
        }
    }

    #[test]
    fn picker_marks_active_provider_as_initial_selection() {
        let config = Config::default();
        let picker = ProviderPickerView::new(ApiProvider::Openrouter, &config);
        assert_eq!(picker.selected_provider(), ApiProvider::Openrouter);
        assert!(picker.rows[picker.selected_idx].is_active);
    }

    #[test]
    fn list_navigation_wraps_between_first_and_last_provider() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let first = picker.rows.first().expect("non-empty list").provider;
        let last = picker.rows.last().expect("non-empty list").provider;

        // Order-independent: jump to the first entry, wrap up to the last, back down.
        picker.selected_idx = 0;
        picker.handle_key(key(KeyCode::Up));
        assert_eq!(picker.selected_provider(), last);

        picker.handle_key(key(KeyCode::Down));
        assert_eq!(picker.selected_provider(), first);
    }

    #[test]
    fn enter_with_no_key_transitions_to_key_entry_stage() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        // Move to OpenRouter, which has no key in default config.
        move_to_provider(&mut picker, ApiProvider::Openrouter);
        assert_eq!(picker.selected_provider(), ApiProvider::Openrouter);
        let action = picker.handle_key(key(KeyCode::Enter));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(picker.stage, Stage::KeyEntry);
    }

    #[test]
    fn enter_with_existing_key_emits_apply_and_closes() {
        let config = Config {
            api_key: Some("existing-deepseek-key".to_string()),
            ..Config::default()
        };
        let mut picker = ProviderPickerView::new(ApiProvider::NvidiaNim, &config);
        // Navigate to DeepSeek, which has a key from the top-level config.
        move_to_provider(&mut picker, ApiProvider::Deepseek);
        let action = picker.handle_key(key(KeyCode::Enter));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied { provider }) => {
                assert_eq!(provider, ApiProvider::Deepseek);
            }
            other => panic!("expected ProviderPickerApplied, got {other:?}"),
        }
    }

    #[test]
    fn configured_provider_can_reenter_key_entry_with_r() {
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                xiaomi_mimo: crate::config::ProviderConfig {
                    api_key: Some("mimo-key".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::XiaomiMimo);

        let action = picker.handle_key(key(KeyCode::Char('r')));

        assert!(matches!(action, ViewAction::None));
        assert_eq!(picker.stage, Stage::KeyEntry);
        assert!(picker.api_key_input.is_empty());
    }

    #[test]
    fn ctrl_r_does_not_trigger_key_entry() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);

        let action = picker.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));

        assert!(matches!(action, ViewAction::None));
        assert_eq!(picker.stage, Stage::List);
    }

    #[test]
    fn configured_provider_footer_mentions_edit_key() {
        let config = Config {
            api_key: Some("existing-deepseek-key".to_string()),
            ..Config::default()
        };
        let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);

        let rendered = render_text(&picker, 80, 12);

        assert!(rendered.contains("Enter"));
        assert!(rendered.contains("apply"));
        assert!(rendered.contains("edit key"));
    }

    #[test]
    fn key_entry_enter_submits_after_typing() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        // Navigate to Novita and trigger key entry.
        move_to_provider(&mut picker, ApiProvider::Novita);
        picker.handle_key(key(KeyCode::Enter));
        assert_eq!(picker.stage, Stage::KeyEntry);
        for c in "novita-key".chars() {
            picker.handle_key(key(KeyCode::Char(c)));
        }
        let action = picker.handle_key(key(KeyCode::Enter));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApiKeySubmitted {
                provider,
                api_key,
            }) => {
                assert_eq!(provider, ApiProvider::Novita);
                assert_eq!(api_key, "novita-key");
            }
            other => panic!("expected ProviderPickerApiKeySubmitted, got {other:?}"),
        }
    }

    #[test]
    fn key_entry_esc_returns_to_list_without_emitting() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Openrouter);
        picker.handle_key(key(KeyCode::Enter));
        assert_eq!(picker.stage, Stage::KeyEntry);
        picker.handle_key(key(KeyCode::Char('a')));
        let action = picker.handle_key(key(KeyCode::Esc));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(picker.stage, Stage::List);
        assert!(picker.api_key_input.is_empty());
    }

    #[test]
    fn list_esc_closes_without_emitting() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let action = picker.handle_key(key(KeyCode::Esc));
        assert!(matches!(action, ViewAction::Close));
    }

    #[test]
    fn key_entry_strips_whitespace_chars() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Openrouter);
        picker.handle_key(key(KeyCode::Enter));
        assert_eq!(picker.stage, Stage::KeyEntry);
        for c in "abc def".chars() {
            picker.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(picker.api_key_input, "abcdef");
    }

    #[test]
    fn small_list_render_keeps_selected_provider_visible_after_down_navigation() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Ollama);

        let rendered = render_text(&picker, 80, 12);

        assert!(rendered.contains("Ollama"));
        assert!(!rendered.contains("DeepSeek *"));
    }

    #[test]
    fn small_list_render_keeps_initial_active_provider_visible() {
        let config = Config::default();
        let picker = ProviderPickerView::new(ApiProvider::Ollama, &config);

        let rendered = render_text(&picker, 80, 12);

        assert!(rendered.contains("Ollama *"));
    }

    #[test]
    fn tall_list_render_shows_all_providers_without_scrolling() {
        let config = Config::default();
        let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);

        let rendered = render_text(&picker, 80, 23);

        assert!(rendered.contains("DeepSeek *"));
        assert!(rendered.contains("Ollama"));
    }

    #[test]
    fn selected_provider_row_uses_strong_highlight() {
        let config = Config::default();
        let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);

        picker.render(area, &mut buf);

        let highlighted_cells = area
            .positions()
            .filter(|position| {
                let cell = &buf[*position];
                cell.bg == palette::SURFACE_ELEVATED
            })
            .count();
        assert!(
            highlighted_cells >= 32,
            "selected provider row should use a visible continuous highlight"
        );
    }
}
