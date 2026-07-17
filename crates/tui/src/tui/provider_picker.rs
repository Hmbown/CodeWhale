//! `/provider` picker modal — pick a provider (DeepSeek / NVIDIA NIM /
//! hosted providers / self-hosted providers) and, if it lacks credentials, type the API key
//! inline before completing the switch (#52).
//!
//! The picker is intentionally a single modal with guided stages (#3875):
//!
//! 1. **List** — pick a provider; each row shows the active provider arrow
//!    and an "API key configured" / "needs API key" hint. Enter on a
//!    configured provider applies the switch immediately
//!    ([`ViewEvent::ProviderPickerApplied`]). Enter on an un-configured one
//!    transitions the same modal into the key-entry state.
//! 2. **Key entry** — masked input box pre-filled with the provider's
//!    canonical env-var name as a hint. Enter submits
//!    [`ViewEvent::ProviderPickerApiKeySubmitted`] for live validation.
//!    Failed verification reopens this stage with the provider error and
//!    never persists the rejected secret.
//! 3. **Model pick** — after a key validates, choose a default model from
//!    the provider catalog (provider default pre-selected).
//! 4. **Confirm** — summary of provider + masked key + model. Enter emits
//!    [`ViewEvent::ProviderPickerSetupConfirmed`], which the UI handler
//!    persists (comment-preserving) before switching.
//! 5. **Custom form** — a named OpenAI-compatible endpoint form. Enter submits
//!    [`ViewEvent::ProviderPickerCustomProviderSubmitted`], which persists a
//!    `[providers.<name>]` table without storing raw secrets.
//!
//! Pressing Esc backs out one stage at a time; from the list it closes the
//! modal without changes.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

use crate::config::{
    ApiProvider, Config, base_url_uses_local_host, has_api_key_for, provider_is_configured,
};
use crate::core::ops::ProviderRuntimeStatus;
use crate::model_profile::{SupportState, resolved_capability_profile};
use crate::models_dev_live::{self, ModelsDevFreshness};
use crate::palette;
use crate::provider_lake::catalog_model_count_for_provider;
use crate::provider_readiness::{
    CredentialState, ProviderReadinessSnapshot, ProviderRouteIdentity, ResolvedProviderReadiness,
    credential_state_for_provider, route_identity_for_model,
};
use crate::tui::app::ReasoningEffort;
use crate::tui::views::{
    ActionHint, EmptyState, ListDetailLayout, ModalKind, ModalView, ViewAction, ViewEvent,
    centered_modal_area, render_modal_footer, render_modal_surface,
};
use codewhale_config::catalog::{CatalogOffering, CatalogSnapshot};
use codewhale_config::provider::WireFormat;
use codewhale_config::route::{
    LogicalModelRef, PricingSku, RequestProtocol, RouteRequest, RouteResolver,
};
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    List,
    KeyEntry,
    /// Default model pick after a key has been live-validated (#3875).
    ModelPick,
    /// Confirmation summary before any secret or model is persisted (#3875).
    Confirm,
    CustomForm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CustomProviderField {
    Name,
    BaseUrl,
    Model,
    ApiKeyEnv,
}

/// Which subset of `rows` the list stage shows (#3830). `Configured` is the
/// default; `A` toggles to `Catalog` to add a new provider or look at one
/// that hasn't been set up yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderListView {
    Configured,
    Catalog,
}

pub struct ProviderPickerView {
    rows: Vec<ProviderDashboardRow>,
    selected_idx: usize,
    stage: Stage,
    view: ProviderListView,
    setup_mode: bool,
    query: String,
    api_key_input: String,
    /// An error surfaced after a failed key verification, shown inline
    /// in the key-entry stage. Cleared when the user edits the input.
    key_entry_error: Option<String>,
    /// Validated key held only in memory until the confirm stage persists it.
    pending_api_key: Option<String>,
    /// Catalog models offered during the model-pick stage.
    model_options: Vec<String>,
    model_selected_idx: usize,
    /// Model chosen on the model-pick stage (and shown on confirm).
    selected_model: Option<String>,
    custom_provider_field: CustomProviderField,
    custom_provider_id: String,
    custom_provider_base_url: String,
    custom_provider_model: String,
    custom_provider_api_key_env: String,
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
    pub(crate) readiness: ResolvedProviderReadiness,
    pub maturity: ProviderMaturity,
    pub messages: Vec<String>,
    pub is_active: bool,
    has_key: bool,
    credential_state: CredentialState,
    route_identity: ProviderRouteIdentity,
    route_ok: bool,
    /// Whether this provider should appear in the default `/provider`
    /// manager view (#3830) without the user explicitly browsing the full
    /// catalog: the active provider, one with working credentials/OAuth, a
    /// custom provider entry, or any provider with a non-default
    /// `[providers.<name>]` table entry. A self-hosted provider type
    /// (Ollama/Sglang/Vllm) does *not* auto-qualify just because its auth is
    /// optional — that would clutter the default view with every untouched
    /// local-provider slot.
    pub is_configured: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAuthStatus {
    Configured,
    Missing,
    NoAuth,
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
        Self::from_config_with_provider_id(
            provider,
            active,
            config,
            None,
            config.provider.as_deref(),
            runtime_status,
        )
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
            config.provider.as_deref(),
            runtime_status,
        )
    }

    fn from_config_with_provider_id(
        provider: ApiProvider,
        active: ApiProvider,
        config: &Config,
        provider_id_override: Option<&str>,
        active_provider_id: Option<&str>,
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
        let auth_mode = config.auth_mode_for_provider(provider);
        let no_auth = crate::config::auth_mode_disables_api_key(auth_mode.as_deref());
        let api_key_required = crate::config::auth_mode_requires_api_key(auth_mode.as_deref());
        let official_endpoint = !config.provider_uses_custom_endpoint(provider);
        let auth_status = auth_status_for(
            provider,
            has_key,
            configured,
            no_auth,
            api_key_required,
            official_endpoint,
        );
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
                    Some(id) => active_provider_id == Some(id),
                    None => true,
                }
        } else {
            provider == active
        };
        let request_concurrency =
            ProviderRequestConcurrencySummary::for_row(provider, config, runtime_status, is_active);

        let compatibility_kind = (provider == ApiProvider::DeepseekCN)
            .then_some(codewhale_config::ProviderKind::Deepseek);
        let Some(kind) = provider.kind().or(compatibility_kind) else {
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
                        .clone()
                        .unwrap_or_else(|| "deepseek-v4-pro".to_string()),
                    wire_model: "legacy alias".to_string(),
                },
                request_concurrency,
                usage_meter,
                reasoning: ProviderReasoningSummary::unknown(provider, config),
                capabilities: ProviderCapabilityBadges::unknown(),
                model_origin,
                readiness: ResolvedProviderReadiness::Legacy,
                maturity: ProviderMaturity::for_provider(provider),
                messages: vec![
                    "legacy DeepSeek China alias; routing maps through DeepSeek compatibility"
                        .to_string(),
                ],
                is_active,
                has_key,
                credential_state: CredentialState::Legacy,
                route_identity: route_identity_for_model(
                    config,
                    provider,
                    configured_model.as_deref().unwrap_or("deepseek-v4-pro"),
                ),
                route_ok: true,
                is_configured: provider_is_configured(
                    provider,
                    is_active,
                    has_key,
                    configured,
                    provider == ApiProvider::Custom && provider_id_override.is_some(),
                ),
            };
        };

        let available_model_count = catalog_model_count_for_provider(provider);
        let catalog_status = if available_model_count == 0 {
            ProviderCatalogStatus::DefaultOnly
        } else {
            ProviderCatalogStatus::Bundled
        };
        let route_request = RouteRequest {
            explicit_provider: Some(kind),
            model_selector: configured_model.clone().map(LogicalModelRef::from),
            saved_provider_model: None,
            // The legacy CN alias shares DeepSeek's strict model contract.
            // Passing its endpoint as a generic override would classify the
            // route as custom and accidentally accept foreign model ids.
            base_url_override: (provider != ApiProvider::DeepseekCN)
                .then(|| configured_base_url.clone())
                .flatten(),
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
                    if provider == ApiProvider::DeepseekCN {
                        configured_base_url
                            .clone()
                            .unwrap_or_else(|| provider.default_base_url().to_string())
                    } else {
                        candidate.endpoint.base_url
                    },
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
                            .and_then(|metadata| metadata.wire_policy().fixed())
                            .map(|protocol| protocol_label(protocol).to_string())
                            .unwrap_or_else(|| "model-aware".to_string()),
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

        let credential_state = credential_state_for_provider(config, provider);
        let route_identity =
            route_identity_for_model(config, provider, &default_route.logical_model);
        let readiness = readiness_for(
            &route_identity,
            credential_state,
            route_ok,
            &ProviderReadinessSnapshot::default(),
        );
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
            credential_state,
            route_identity,
            route_ok,
            is_configured: provider_is_configured(
                provider,
                is_active,
                has_key,
                configured,
                provider == ApiProvider::Custom && provider_id_override.is_some(),
            ),
        }
    }

    fn list_row_hint(&self, view: ProviderListView) -> String {
        match view {
            ProviderListView::Configured => {
                format!("{} | {}", self.readiness.label(), self.auth_status.label())
            }
            ProviderListView::Catalog => self.compact_hint(),
        }
    }

    fn compact_hint(&self) -> String {
        // Self-hosted providers carry a local/private posture; surface it next
        // to the base URL so the row reads correctly without a key (#3083).
        let self_hosted = if self.provider.is_self_hosted()
            || matches!(
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
            "{} | {} | {} | {} | base:{}{} | route:{}{} origin:{} | {} | {}{} | catalog:{}{}",
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

    /// Cross-field search (#3830 P1, #4141): match a query against the provider
    /// name (display name, provider id, kind, provider key), the base URL, and
    /// the default route's display model name and wire model id. Matching the
    /// route means a model name or wire id surfaces the provider that serves it,
    /// keeping this picker consistent with the model picker's cross-field search
    /// (`model_row_matches_query`).
    fn matches_query(&self, query: &str) -> bool {
        let query = query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return true;
        }
        self.display_name.to_ascii_lowercase().contains(&query)
            || self.provider_id.to_ascii_lowercase().contains(&query)
            || self.kind.to_ascii_lowercase().contains(&query)
            || self.base_url.to_ascii_lowercase().contains(&query)
            || self.provider.as_str().to_ascii_lowercase().contains(&query)
            || self
                .default_route
                .logical_model
                .to_ascii_lowercase()
                .contains(&query)
            || self
                .default_route
                .wire_model
                .to_ascii_lowercase()
                .contains(&query)
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
            Self::Configured => "key:configured",
            Self::Missing => "key:not-set",
            Self::NoAuth => "auth:none",
            Self::Optional => "key:optional",
            Self::OAuthReady => "auth:oauth-ready",
            Self::OAuthMissing => "auth:oauth-missing",
            Self::Local => "local",
            Self::Legacy => "legacy",
        }
    }
}

/// Compact Models.dev freshness chip for the provider picker chrome (#4139).
fn catalog_freshness_title_suffix() -> &'static str {
    match models_dev_live::status().freshness {
        ModelsDevFreshness::Stale => " · stale",
        ModelsDevFreshness::Failed => " · cache failed",
        ModelsDevFreshness::Bundled | ModelsDevFreshness::Live => "",
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
        | ApiProvider::MinimaxAnthropic
        | ApiProvider::Sglang
        | ApiProvider::Vllm
        | ApiProvider::Zai
        | ApiProvider::Xai
        | ApiProvider::Moonshot => ProviderReasoningStreamVisibility::StructuredThinking,
        _ => ProviderReasoningStreamVisibility::Unknown,
    }
}

fn auth_status_for(
    provider: ApiProvider,
    has_key: bool,
    configured: Option<&crate::config::ProviderConfig>,
    no_auth: bool,
    api_key_required: bool,
    official_endpoint: bool,
) -> ProviderAuthStatus {
    if no_auth {
        return ProviderAuthStatus::NoAuth;
    }
    if provider.is_self_hosted() {
        if api_key_required {
            return if has_key {
                ProviderAuthStatus::Configured
            } else {
                ProviderAuthStatus::Missing
            };
        }
        if provider == ApiProvider::Ollama {
            return ProviderAuthStatus::Local;
        }
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
    if provider == ApiProvider::Moonshot
        && official_endpoint
        && configured.is_some_and(config_uses_kimi_oauth)
    {
        return if crate::config::kimi_cli_credentials_valid() {
            ProviderAuthStatus::OAuthReady
        } else {
            ProviderAuthStatus::OAuthMissing
        };
    }
    if provider == ApiProvider::OpenaiCodex && official_endpoint {
        return if has_key {
            ProviderAuthStatus::OAuthReady
        } else {
            ProviderAuthStatus::OAuthMissing
        };
    }
    if provider == ApiProvider::Xai
        && official_endpoint
        && let Some(status) = xai_oauth_status(configured, crate::xai_oauth::credentials_valid())
    {
        return status;
    }
    if has_key {
        ProviderAuthStatus::Configured
    } else {
        ProviderAuthStatus::Missing
    }
}

fn xai_oauth_status(
    configured: Option<&crate::config::ProviderConfig>,
    credentials_present: bool,
) -> Option<ProviderAuthStatus> {
    let oauth_selected = configured
        .and_then(|entry| entry.auth_mode.as_deref())
        .is_some_and(crate::xai_oauth::auth_mode_uses_xai_oauth);
    if !oauth_selected {
        return None;
    }
    Some(if credentials_present {
        ProviderAuthStatus::OAuthReady
    } else {
        ProviderAuthStatus::OAuthMissing
    })
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
    })
}

fn custom_provider_auth_is_optional(configured: Option<&crate::config::ProviderConfig>) -> bool {
    configured.is_some_and(|entry| {
        entry
            .auth_mode
            .as_deref()
            .is_some_and(|mode| crate::config::auth_mode_disables_api_key(Some(mode)))
            || entry
                .base_url
                .as_deref()
                .is_some_and(base_url_uses_local_host)
    })
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
        matches!(
            normalized.as_str(),
            "kimi" | "kimi_oauth" | "kimi_cli" | "oauth"
        )
    })
}

fn readiness_for(
    identity: &ProviderRouteIdentity,
    credential: CredentialState,
    route_ok: bool,
    health: &ProviderReadinessSnapshot,
) -> ResolvedProviderReadiness {
    crate::provider_readiness::resolve_with_identity(identity, credential, route_ok, health)
}

fn usage_meter_for(provider: ApiProvider) -> String {
    match provider {
        ApiProvider::Ollama | ApiProvider::Sglang | ApiProvider::Vllm => "cost: local".to_string(),
        ApiProvider::OpenaiCodex => "usage: Codex OAuth quota".to_string(),
        ApiProvider::Moonshot if crate::config::kimi_cli_credentials_valid() => {
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
        Self::new_with_runtime_status_and_memory(active, config, runtime_status, None)
    }

    #[must_use]
    pub fn new_with_runtime_status_and_memory(
        active: ApiProvider,
        config: &Config,
        runtime_status: Option<ProviderRuntimeStatus>,
        memory: Option<&crate::tui::app::ProviderPickerMemory>,
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
        // Default to the configured-only view (#3830); if nothing is
        // configured yet (a fresh install), open straight on the full
        // catalog instead of an empty list with no obvious next step.
        let view = if rows.iter().any(|row| row.is_configured) {
            ProviderListView::Configured
        } else {
            ProviderListView::Catalog
        };
        let mut picker = Self {
            rows,
            selected_idx,
            stage: Stage::List,
            view,
            setup_mode: false,
            query: String::new(),
            api_key_input: String::new(),
            key_entry_error: None,
            pending_api_key: None,
            model_options: Vec::new(),
            model_selected_idx: 0,
            selected_model: None,
            custom_provider_field: CustomProviderField::Name,
            custom_provider_id: String::new(),
            custom_provider_base_url: String::new(),
            custom_provider_model: String::new(),
            custom_provider_api_key_env: String::new(),
        };
        picker.restore_memory(memory);
        picker
    }

    /// Apply session-local request evidence after the static catalog rows are
    /// built. Saved credentials stay "not checked" until this snapshot proves
    /// success; a failed check remains visible and retryable with its reason.
    #[must_use]
    pub(crate) fn with_provider_health(mut self, health: &ProviderReadinessSnapshot) -> Self {
        for row in &mut self.rows {
            row.readiness = readiness_for(
                &row.route_identity,
                row.credential_state,
                row.route_ok,
                health,
            );
            if let Some(detail) = row.readiness.detail()
                && !row.messages.iter().any(|message| message == detail)
            {
                row.messages.push(detail.to_string());
            }
        }
        self
    }

    /// Restore browsing context from the last dismissed `/provider` picker.
    fn restore_memory(&mut self, memory: Option<&crate::tui::app::ProviderPickerMemory>) {
        let Some(memory) = memory else {
            return;
        };
        if memory.catalog_view {
            self.view = ProviderListView::Catalog;
        }
        if let Some(remembered_id) = memory.selected_provider_id.as_deref()
            && let Some(idx) = self
                .rows
                .iter()
                .position(|row| row.provider_id == remembered_id)
            && (self.row_visible(idx) || memory.catalog_view)
        {
            if memory.catalog_view {
                self.view = ProviderListView::Catalog;
            }
            self.selected_idx = idx;
        }
        if !self.rows.is_empty() && !self.row_visible(self.selected_idx) {
            self.selected_idx = (0..self.rows.len())
                .find(|idx| self.row_visible(*idx))
                .unwrap_or(0);
        }
    }

    /// Open the picker as a first-run/setup catalog: every built-in provider is
    /// visible, and an optional target is focused. Missing-auth targets jump
    /// straight to the existing masked key-entry stage; configured/local
    /// targets stay on the list so Enter applies them normally.
    #[must_use]
    pub fn new_for_setup(
        active: ApiProvider,
        target: Option<ApiProvider>,
        config: &Config,
        runtime_status: Option<ProviderRuntimeStatus>,
    ) -> Self {
        let mut picker = Self::new_with_runtime_status(active, config, runtime_status);
        picker.view = ProviderListView::Catalog;
        picker.setup_mode = true;
        if let Some(target) = target
            && let Some(idx) = picker.rows.iter().position(|row| row.provider == target)
        {
            picker.selected_idx = idx;
            if !picker.selected_has_key() {
                picker.enter_key_entry();
            }
        }
        picker
    }

    /// Open the picker already focused on `target` in its key-entry stage —
    /// the missing-auth handoff (#3830): when a route switch is rejected for
    /// want of a key, drop the user straight onto that provider's key prompt
    /// instead of dead-ending with an error. Falls back to the normal list
    /// if the target has no row (e.g. an unknown custom id).
    #[must_use]
    /// Returns `None` when `target` has no picker row (an unknown/custom
    /// provider we could not focus or key-enter) so the caller can keep its
    /// honest error instead of opening a dead-end picker.
    pub fn new_for_missing_auth(
        active: ApiProvider,
        target: ApiProvider,
        config: &Config,
        runtime_status: Option<ProviderRuntimeStatus>,
    ) -> Option<Self> {
        let mut picker = Self::new_with_runtime_status(active, config, runtime_status);
        let idx = picker.rows.iter().position(|row| row.provider == target)?;
        picker.selected_idx = idx;
        // The target may be an unconfigured catalog row; show the catalog so
        // it is visible, then jump into key entry for it.
        picker.view = ProviderListView::Catalog;
        picker.enter_key_entry();
        Some(picker)
    }

    fn row_visible(&self, idx: usize) -> bool {
        let query = self.query.trim();
        if !query.is_empty() {
            return self.rows[idx].matches_query(query);
        }
        match self.view {
            ProviderListView::Catalog => true,
            ProviderListView::Configured => self.rows[idx].is_configured,
        }
    }

    fn visible_row_count(&self) -> usize {
        (0..self.rows.len())
            .filter(|idx| self.row_visible(*idx))
            .count()
    }

    /// Toggle between the configured-only and full-catalog views (#3830),
    /// keeping the current selection if it stays visible and otherwise
    /// jumping to the first visible row (`rows` is sorted alphabetically by
    /// display name, so this lands on the alphabetically-first match, not
    /// necessarily the row positionally nearest the old selection).
    fn toggle_view(&mut self) {
        self.view = match self.view {
            ProviderListView::Configured => ProviderListView::Catalog,
            ProviderListView::Catalog => ProviderListView::Configured,
        };
        if !self.rows.is_empty() && !self.row_visible(self.selected_idx) {
            self.selected_idx = (0..self.rows.len())
                .find(|idx| self.row_visible(*idx))
                .unwrap_or(0);
        }
    }

    /// Update the search query and clamp the selection to the first visible row.
    fn update_query(&mut self, next: String) {
        self.query = next;
        self.selected_idx = (0..self.rows.len())
            .find(|idx| self.row_visible(*idx))
            .unwrap_or(0);
    }

    /// Move the selection one visible row forward (`step = 1`) or backward
    /// (`step = -1`), skipping rows hidden by the current `view` filter
    /// (#3830) and wrapping at the ends.
    fn move_selection(&mut self, step: i64) {
        let count = self.rows.len();
        if count == 0 || self.visible_row_count() == 0 {
            return;
        }
        let mut idx = self.selected_idx;
        loop {
            idx = ((idx as i64 + step).rem_euclid(count as i64)) as usize;
            if self.row_visible(idx) {
                self.selected_idx = idx;
                return;
            }
        }
    }

    fn move_up(&mut self) {
        self.move_selection(-1);
    }

    fn move_down(&mut self) {
        self.move_selection(1);
    }

    fn selected_provider(&self) -> ApiProvider {
        self.rows[self.selected_idx].provider
    }

    fn selected_provider_id(&self) -> Option<String> {
        let row = &self.rows[self.selected_idx];
        (row.provider == ApiProvider::Custom).then(|| row.provider_id.clone())
    }

    fn selected_has_key(&self) -> bool {
        matches!(
            self.rows[self.selected_idx].credential_state,
            CredentialState::Saved
                | CredentialState::NoAuth
                | CredentialState::Local
                | CredentialState::Legacy
        )
    }

    fn selected_route_is_valid(&self) -> bool {
        self.rows[self.selected_idx].route_ok
    }

    fn enter_key_entry(&mut self) {
        self.stage = Stage::KeyEntry;
        self.api_key_input.clear();
        self.key_entry_error = None;
        self.pending_api_key = None;
        self.model_options.clear();
        self.model_selected_idx = 0;
        self.selected_model = None;
    }

    /// Open the picker already focused on `target` in its key-entry stage
    /// with a validation error message - the verify-then-persist handoff
    /// (#3875): when a submitted key fails live validation, drop the user
    /// back on that provider's key prompt with the provider's actual error
    /// instead of dead-ending with a status toast.
    #[must_use]
    pub fn new_for_key_entry_with_error(
        active: ApiProvider,
        target: ApiProvider,
        config: &Config,
        runtime_status: Option<ProviderRuntimeStatus>,
        error: String,
    ) -> Option<Self> {
        let mut picker = Self::new_with_runtime_status(active, config, runtime_status);
        let idx = picker.rows.iter().position(|row| row.provider == target)?;
        picker.selected_idx = idx;
        picker.view = ProviderListView::Catalog;
        picker.stage = Stage::KeyEntry;
        picker.key_entry_error = Some(error);
        Some(picker)
    }

    /// Open the guided flow on the model-pick stage after a key has been
    /// live-validated (#3875). The key stays in memory only until confirm.
    #[must_use]
    pub fn new_for_model_pick_after_validation(
        active: ApiProvider,
        target: ApiProvider,
        config: &Config,
        runtime_status: Option<ProviderRuntimeStatus>,
        api_key: String,
    ) -> Option<Self> {
        let mut picker = Self::new_with_runtime_status(active, config, runtime_status);
        let idx = picker.rows.iter().position(|row| row.provider == target)?;
        picker.selected_idx = idx;
        picker.view = ProviderListView::Catalog;
        picker.pending_api_key = Some(api_key);
        picker.api_key_input.clear();
        picker.key_entry_error = None;
        picker.enter_model_pick();
        Some(picker)
    }

    fn enter_model_pick(&mut self) {
        self.stage = Stage::ModelPick;
        let provider = self.selected_provider();
        let preferred = self.rows[self.selected_idx]
            .default_route
            .logical_model
            .clone();
        let mut models = crate::provider_lake::all_catalog_models_for_provider(provider);
        if models.is_empty() && !preferred.trim().is_empty() {
            models.push(preferred.clone());
        }
        if models.is_empty() {
            // Last-resort so the guided flow never dead-ends without a choice.
            models.push(provider.as_str().to_string());
        }
        let selected = models
            .iter()
            .position(|model| model.eq_ignore_ascii_case(preferred.trim()))
            .unwrap_or(0);
        self.model_options = models;
        self.model_selected_idx = selected.min(self.model_options.len().saturating_sub(1));
        self.selected_model = self.model_options.get(self.model_selected_idx).cloned();
    }

    fn enter_confirm(&mut self) {
        if self.selected_model.is_none() {
            self.selected_model = self.model_options.get(self.model_selected_idx).cloned();
        }
        self.stage = Stage::Confirm;
    }

    fn move_model_selection(&mut self, delta: isize) {
        let len = self.model_options.len();
        if len == 0 {
            return;
        }
        let current = self.model_selected_idx as isize;
        let next = (current + delta).rem_euclid(len as isize) as usize;
        self.model_selected_idx = next;
        self.selected_model = self.model_options.get(next).cloned();
    }

    fn build_setup_confirmed_event(&self) -> Option<ViewEvent> {
        let api_key = self.pending_api_key.as_ref()?.trim();
        if api_key.is_empty() {
            return None;
        }
        let model = self
            .selected_model
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())?;
        Some(ViewEvent::ProviderPickerSetupConfirmed {
            provider: self.selected_provider(),
            provider_id: self.selected_provider_id(),
            api_key: api_key.to_string(),
            model: model.to_string(),
        })
    }

    fn enter_custom_form(&mut self) {
        self.stage = Stage::CustomForm;
        self.custom_provider_field = CustomProviderField::Name;
        self.custom_provider_id.clear();
        self.custom_provider_base_url.clear();
        self.custom_provider_model.clear();
        self.custom_provider_api_key_env.clear();
    }

    fn custom_form_field_mut(&mut self) -> &mut String {
        match self.custom_provider_field {
            CustomProviderField::Name => &mut self.custom_provider_id,
            CustomProviderField::BaseUrl => &mut self.custom_provider_base_url,
            CustomProviderField::Model => &mut self.custom_provider_model,
            CustomProviderField::ApiKeyEnv => &mut self.custom_provider_api_key_env,
        }
    }

    fn custom_form_field_value(&self, field: CustomProviderField) -> &str {
        match field {
            CustomProviderField::Name => &self.custom_provider_id,
            CustomProviderField::BaseUrl => &self.custom_provider_base_url,
            CustomProviderField::Model => &self.custom_provider_model,
            CustomProviderField::ApiKeyEnv => &self.custom_provider_api_key_env,
        }
    }

    fn advance_custom_field(&mut self) {
        self.custom_provider_field = match self.custom_provider_field {
            CustomProviderField::Name => CustomProviderField::BaseUrl,
            CustomProviderField::BaseUrl => CustomProviderField::Model,
            CustomProviderField::Model => CustomProviderField::ApiKeyEnv,
            CustomProviderField::ApiKeyEnv => CustomProviderField::ApiKeyEnv,
        };
    }

    fn retreat_custom_field(&mut self) {
        self.custom_provider_field = match self.custom_provider_field {
            CustomProviderField::Name => CustomProviderField::Name,
            CustomProviderField::BaseUrl => CustomProviderField::Name,
            CustomProviderField::Model => CustomProviderField::BaseUrl,
            CustomProviderField::ApiKeyEnv => CustomProviderField::Model,
        };
    }

    fn build_custom_provider_event(&self) -> Option<ViewEvent> {
        let provider_id = self.custom_provider_id.trim();
        let base_url = self.custom_provider_base_url.trim();
        if provider_id.is_empty() || base_url.is_empty() {
            return None;
        }
        let model = non_empty_string(&self.custom_provider_model);
        let api_key_env = non_empty_string(&self.custom_provider_api_key_env);
        Some(ViewEvent::ProviderPickerCustomProviderSubmitted {
            provider_id: provider_id.to_string(),
            base_url: base_url.to_string(),
            model,
            api_key_env,
        })
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

    /// Rows visible under the current `view` filter (#3830), as
    /// `(original_index, row)` pairs so callers can still compare against
    /// `self.selected_idx`.
    fn filtered_rows(&self) -> Vec<(usize, &ProviderDashboardRow)> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(idx, _)| self.row_visible(*idx))
            .collect()
    }

    fn visible_start(selected_pos: usize, total: usize, visible_rows: usize) -> usize {
        if visible_rows == 0 {
            return 0;
        }
        let max_start = total.saturating_sub(visible_rows);
        selected_pos
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
        let enter_action = if !self.selected_route_is_valid() {
            "unavailable"
        } else if self.selected_has_key() {
            "apply"
        } else {
            "set key"
        };
        let title = match (self.setup_mode, self.view) {
            (true, ProviderListView::Configured) => {
                format!(" Provider setup{} ", catalog_freshness_title_suffix())
            }
            (true, ProviderListView::Catalog) => {
                format!(" Provider setup · all{} ", catalog_freshness_title_suffix())
            }
            (false, ProviderListView::Configured) => {
                format!(" Provider{} ", catalog_freshness_title_suffix())
            }
            (false, ProviderListView::Catalog) => {
                format!(" Provider · all{} ", catalog_freshness_title_suffix())
            }
        };
        let outer = Block::default()
            .title(Line::from(Span::styled(
                title,
                Style::default()
                    .fg(palette::WHALE_INFO)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_BG));
        let inner = outer.inner(area);
        outer.render(area, buf);

        let view_action = match self.view {
            ProviderListView::Configured => "browse all",
            ProviderListView::Catalog => "configured",
        };
        let search_active = !self.query.trim().is_empty();
        // The action footer moves into the body so it wraps instead of clipping
        // at narrow widths (#3732); the provider list renders above it.
        let content = if search_active {
            render_modal_footer(
                inner,
                buf,
                &[
                    ActionHint::new("Esc", "clear"),
                    ActionHint::new("↑↓", "move"),
                    ActionHint::new("Enter", enter_action),
                    ActionHint::new("A", view_action),
                    ActionHint::new("C", "custom"),
                    ActionHint::new("Esc", "cancel"),
                ],
            )
        } else {
            render_modal_footer(
                inner,
                buf,
                &[
                    ActionHint::new("↑↓", "move"),
                    ActionHint::new("a-z", "jump"),
                    ActionHint::new("Enter", enter_action),
                    ActionHint::new("A", view_action),
                    ActionHint::new("C", "custom"),
                    ActionHint::new("R", "edit key"),
                    ActionHint::new("M", "models"),
                    ActionHint::new("Esc", "cancel"),
                ],
            )
        };

        let filtered = self.filtered_rows();
        if filtered.is_empty() {
            if search_active {
                EmptyState::new(
                    "No providers match",
                    "Try a different search term or clear to browse.",
                )
                .primary_action("Esc", "clear search")
                .render(content, buf);
            } else {
                EmptyState::new(
                    "No providers configured yet",
                    "Browse every supported provider or create a custom endpoint.",
                )
                .primary_action("A", "browse all")
                .secondary_action("C", "custom")
                .render(content, buf);
            }
            return;
        }

        let layout = ListDetailLayout::split(content, 34);
        let selected_pos = filtered
            .iter()
            .position(|(idx, _)| *idx == self.selected_idx)
            .unwrap_or(0);
        let visible_rows = usize::from(layout.list.height);
        let visible_start = Self::visible_start(selected_pos, filtered.len(), visible_rows);
        let mut lines: Vec<Line> = Vec::with_capacity(visible_rows);
        for (pos, (idx, row)) in filtered
            .iter()
            .enumerate()
            .skip(visible_start)
            .take(visible_rows)
        {
            let is_selected = *idx == self.selected_idx;
            debug_assert_eq!(is_selected, pos == selected_pos);
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
            let has_usable_auth = matches!(
                row.credential_state,
                CredentialState::Saved
                    | CredentialState::NoAuth
                    | CredentialState::Local
                    | CredentialState::Legacy
            );
            let hint_style = if is_selected {
                let hint_fg = if has_usable_auth {
                    palette::TEXT_MUTED
                } else {
                    palette::STATUS_WARNING
                };
                Self::selected_row_style(hint_fg)
            } else if has_usable_auth {
                Style::default().fg(palette::TEXT_MUTED)
            } else {
                Style::default().fg(palette::STATUS_WARNING)
            };
            let prefix = format!(" {arrow} {}{active_dot}  ", row.display_name);
            let hint = crate::tui::ui_text::semantic_truncate_between_affixes(
                &prefix,
                &row.list_row_hint(self.view),
                "",
                usize::from(layout.list.width),
            );
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
                let target_width = usize::from(layout.list.width);
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
        Paragraph::new(lines).render(layout.list, buf);
        self.render_provider_detail(layout.detail, buf, &self.rows[self.selected_idx]);
    }

    fn render_provider_detail(&self, area: Rect, buf: &mut Buffer, row: &ProviderDashboardRow) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let block = Block::default()
            .title(Line::from(Span::styled(
                " Details ",
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default());
        let inner = block.inner(area);
        block.render(area, buf);

        let route = if row.default_route.logical_model == row.default_route.wire_model {
            row.default_route.logical_model.clone()
        } else {
            format!(
                "{} -> {}",
                row.default_route.logical_model, row.default_route.wire_model
            )
        };
        let mut lines = vec![
            Line::from(Span::styled(
                row.display_name.clone(),
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                format!(
                    "{} | {} | {}",
                    row.readiness.label(),
                    row.auth_status.label(),
                    row.catalog_label()
                ),
                Style::default().fg(palette::TEXT_MUTED),
            )),
            Line::from(Span::styled(
                format!("Route: {route}"),
                Style::default().fg(palette::TEXT_PRIMARY),
            )),
            Line::from(Span::styled(
                format!("Endpoint: {}", row.base_url),
                Style::default().fg(palette::TEXT_MUTED),
            )),
            Line::from(Span::styled(
                format!(
                    "Protocol: {} | Usage: {}",
                    row.supported_protocols.join("+"),
                    row.usage_meter
                ),
                Style::default().fg(palette::TEXT_MUTED),
            )),
            Line::from(Span::styled(
                format!("Capabilities: {}", row.capabilities.label()),
                Style::default().fg(palette::TEXT_MUTED),
            )),
            Line::from(Span::styled(
                format!("Reasoning: {}", row.reasoning.label()),
                Style::default().fg(palette::TEXT_MUTED),
            )),
        ];
        if let Some(concurrency) = row.request_concurrency.label() {
            lines.push(Line::from(Span::styled(
                concurrency,
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }
        for message in row.messages.iter().take(2) {
            lines.push(Line::from(Span::styled(
                format!("Note: {message}"),
                Style::default().fg(palette::STATUS_WARNING),
            )));
        }
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .render(inner, buf);
    }

    fn render_key_entry(&self, area: Rect, buf: &mut Buffer) {
        let row = &self.rows[self.selected_idx];
        let codex_oauth = row.provider == ApiProvider::OpenaiCodex;
        let xai_oauth = row.provider == ApiProvider::Xai;
        let oauth_provider = codex_oauth || xai_oauth;
        let outer = Block::default()
            .title(Line::from(Span::styled(
                if oauth_provider {
                    format!(" OAuth login — {} ", row.display_name)
                } else {
                    format!(" API key — {} ", row.display_name)
                },
                Style::default()
                    .fg(palette::WHALE_INFO)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_BG));
        let inner = outer.inner(area);
        outer.render(area, buf);

        // The action footer moves into the body so it wraps instead of clipping
        // at narrow widths (#3732); the key-entry fields render above it.
        let content = if codex_oauth {
            render_modal_footer(inner, buf, &[ActionHint::new("Esc", "back")])
        } else if xai_oauth {
            render_modal_footer(
                inner,
                buf,
                &[
                    ActionHint::new("Enter", "device login"),
                    ActionHint::new("Esc", "back"),
                ],
            )
        } else {
            render_modal_footer(
                inner,
                buf,
                &[
                    ActionHint::new("Enter", "continue"),
                    ActionHint::new("Esc", "back"),
                ],
            )
        };

        let masked = mask_key(&self.api_key_input);
        let display = if codex_oauth {
            "(run codex login; no token is stored here)".to_string()
        } else if xai_oauth {
            "(browser/device-code sign-in; tokens stay in ~/.grok/auth.json)".to_string()
        } else if masked.is_empty() {
            "(paste key here)".to_string()
        } else {
            masked
        };
        let key_lines = vec![Line::from(vec![
            Span::styled(
                if oauth_provider { "Auth: " } else { "Key: " },
                Style::default().fg(palette::TEXT_MUTED),
            ),
            Span::styled(
                display,
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
        ])];
        let reopen_command = if self.setup_mode {
            "/setup provider"
        } else {
            "/provider"
        };
        let mut hint_lines = if codex_oauth {
            vec![Line::from(Span::styled(
                format!(
                    "Run `codex login`, or set {} / CODEX_ACCESS_TOKEN and re-open {reopen_command}.",
                    self.env_var_for_selected_row(),
                ),
                Style::default().fg(palette::TEXT_MUTED),
            ))]
        } else if xai_oauth {
            vec![Line::from(Span::styled(
                "Press Enter for xAI device login, or use XAI_API_KEY and re-open this picker.",
                Style::default().fg(palette::TEXT_MUTED),
            ))]
        } else {
            vec![Line::from(Span::styled(
                format!(
                    "Or set the {} environment variable and re-open {reopen_command}.",
                    self.env_var_for_selected_row(),
                ),
                Style::default().fg(palette::TEXT_MUTED),
            ))]
        };
        if !oauth_provider && let Some(url) = row.provider.credential_url() {
            hint_lines.push(Line::from(Span::styled(
                format!("Credentials: {url}"),
                Style::default().fg(palette::TEXT_MUTED),
            )));
        };

        if let Some(ref error) = self.key_entry_error {
            hint_lines.push(Line::from(Span::styled(
                format!("Verification failed: {error}"),
                Style::default().fg(palette::STATUS_ERROR),
            )));
        }

        let hint_height = hint_lines.len().clamp(1, 3) as u16;
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(hint_height),
                Constraint::Min(1),
            ])
            .split(content);

        Paragraph::new(key_lines).render(layout[0], buf);
        Paragraph::new(hint_lines).render(layout[1], buf);
    }

    fn render_model_pick(&self, area: Rect, buf: &mut Buffer) {
        let provider_name = self.rows[self.selected_idx].display_name.clone();
        let outer = Block::default()
            .title(Line::from(Span::styled(
                format!(" Default model · {provider_name} "),
                Style::default()
                    .fg(palette::WHALE_INFO)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_BG));
        let inner = outer.inner(area);
        outer.render(area, buf);

        let content = render_modal_footer(
            inner,
            buf,
            &[
                ActionHint::new("↑↓", "move"),
                ActionHint::new("Enter", "continue"),
                ActionHint::new("Esc", "back"),
            ],
        );

        let header = Paragraph::new(Line::from(Span::styled(
            "Key verified. Pick a default model for this provider.",
            Style::default().fg(palette::TEXT_MUTED),
        )));
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(content);
        header.render(layout[0], buf);

        let list_area = layout[1];
        let visible_rows = usize::from(list_area.height);
        let visible_start = Self::visible_start(
            self.model_selected_idx,
            self.model_options.len(),
            visible_rows,
        );
        let mut lines: Vec<Line> = Vec::with_capacity(visible_rows);
        for (idx, model) in self
            .model_options
            .iter()
            .enumerate()
            .skip(visible_start)
            .take(visible_rows)
        {
            let is_selected = idx == self.model_selected_idx;
            let arrow = if is_selected { "▸" } else { " " };
            let label_style = if is_selected {
                Self::selected_row_style(palette::TEXT_PRIMARY)
            } else {
                Style::default().fg(palette::TEXT_PRIMARY)
            };
            let default_tag = if self.rows[self.selected_idx]
                .default_route
                .logical_model
                .eq_ignore_ascii_case(model)
            {
                "default"
            } else {
                ""
            };
            let mut line = Line::from(vec![
                Span::styled(format!(" {arrow} {model}"), label_style),
                if default_tag.is_empty() {
                    Span::raw("")
                } else {
                    Span::styled(
                        format!("  ({default_tag})"),
                        if is_selected {
                            Self::selected_row_style(palette::TEXT_MUTED)
                        } else {
                            Style::default().fg(palette::TEXT_MUTED)
                        },
                    )
                },
            ]);
            if is_selected {
                line.style = Self::selected_row_bg_style();
            }
            lines.push(line);
        }
        if lines.is_empty() {
            lines.push(Line::from(Span::styled(
                "No catalog models available.",
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }
        Paragraph::new(lines).render(list_area, buf);
    }

    fn render_confirm(&self, area: Rect, buf: &mut Buffer) {
        let row = &self.rows[self.selected_idx];
        let outer = Block::default()
            .title(Line::from(Span::styled(
                " Confirm provider setup ",
                Style::default()
                    .fg(palette::WHALE_INFO)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_BG));
        let inner = outer.inner(area);
        outer.render(area, buf);

        let content = render_modal_footer(
            inner,
            buf,
            &[
                ActionHint::new("Enter", "save & switch"),
                ActionHint::new("Esc", "back"),
            ],
        );

        let masked = self
            .pending_api_key
            .as_deref()
            .map(mask_key)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "(none)".to_string());
        let model = self
            .selected_model
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("(none)");
        let lines = vec![
            Line::from(Span::styled(
                "Review before saving. Nothing is written until you confirm.",
                Style::default().fg(palette::TEXT_MUTED),
            )),
            Line::from(vec![
                Span::styled("Provider: ", Style::default().fg(palette::TEXT_MUTED)),
                Span::styled(
                    row.display_name.clone(),
                    Style::default()
                        .fg(palette::TEXT_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("API key:  ", Style::default().fg(palette::TEXT_MUTED)),
                Span::styled(masked, Style::default().fg(palette::TEXT_PRIMARY)),
            ]),
            Line::from(vec![
                Span::styled("Model:    ", Style::default().fg(palette::TEXT_MUTED)),
                Span::styled(
                    model.to_string(),
                    Style::default()
                        .fg(palette::TEXT_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ];
        Paragraph::new(lines).render(content, buf);
    }

    fn render_custom_form(&self, area: Rect, buf: &mut Buffer) {
        let outer = Block::default()
            .title(Line::from(Span::styled(
                " Custom provider ",
                Style::default()
                    .fg(palette::WHALE_INFO)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_BG));
        let inner = outer.inner(area);
        outer.render(area, buf);

        let content = render_modal_footer(
            inner,
            buf,
            &[
                ActionHint::new("Tab/↑↓", "field"),
                ActionHint::new("Enter", "next/save"),
                ActionHint::new("Esc", "back"),
            ],
        );
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(content);

        Paragraph::new(Line::from(Span::styled(
            "OpenAI-compatible endpoint. Store an env var name here, not a raw key.",
            Style::default().fg(palette::TEXT_MUTED),
        )))
        .render(layout[0], buf);

        self.render_custom_form_field(layout[1], buf, CustomProviderField::Name, "Name", "acme_ai");
        self.render_custom_form_field(
            layout[2],
            buf,
            CustomProviderField::BaseUrl,
            "Base URL",
            "https://api.example.com/v1",
        );
        self.render_custom_form_field(
            layout[3],
            buf,
            CustomProviderField::Model,
            "Default model",
            "optional",
        );
        self.render_custom_form_field(
            layout[4],
            buf,
            CustomProviderField::ApiKeyEnv,
            "API key env",
            "optional",
        );
    }

    fn render_custom_form_field(
        &self,
        area: Rect,
        buf: &mut Buffer,
        field: CustomProviderField,
        label: &str,
        placeholder: &str,
    ) {
        let selected = self.custom_provider_field == field;
        let marker = if selected { "▸" } else { " " };
        let value = self.custom_form_field_value(field);
        let display = if value.is_empty() { placeholder } else { value };
        let value_style = if selected {
            Self::selected_row_style(palette::TEXT_PRIMARY)
        } else if value.is_empty() {
            Style::default().fg(palette::TEXT_MUTED)
        } else {
            Style::default().fg(palette::TEXT_PRIMARY)
        };
        let label_style = if selected {
            Self::selected_row_style(palette::WHALE_INFO)
        } else {
            Style::default().fg(palette::TEXT_MUTED)
        };
        let mut line = Line::from(vec![
            Span::styled(marker, label_style),
            Span::styled(" ", label_style),
            Span::styled(format!("{label}: "), label_style),
            Span::styled(
                crate::tui::ui_text::truncate_line_to_width(
                    display,
                    usize::from(area.width).saturating_sub(18),
                ),
                value_style,
            ),
        ]);
        if selected {
            line.style = Self::selected_row_bg_style();
        }
        Paragraph::new(line).render(area, buf);
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
        match self.stage {
            Stage::KeyEntry => {
                if matches!(
                    self.selected_provider(),
                    ApiProvider::OpenaiCodex | ApiProvider::Xai
                ) {
                    return true;
                }
                let sanitized: String = text.chars().filter(|c| !c.is_whitespace()).collect();
                if !sanitized.is_empty() {
                    self.api_key_input.push_str(&sanitized);
                    self.key_entry_error = None;
                }
                true
            }
            Stage::CustomForm => {
                let sanitized = text.replace(['\r', '\n', '\t'], " ");
                self.custom_form_field_mut().push_str(sanitized.trim());
                true
            }
            Stage::List | Stage::ModelPick | Stage::Confirm => false,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match self.stage {
            Stage::List => match key.code {
                KeyCode::Esc if !self.query.is_empty() => {
                    self.update_query(String::new());
                    ViewAction::None
                }
                KeyCode::Esc => ViewAction::EmitAndClose(ViewEvent::ProviderPickerDismissed {
                    catalog_view: self.view == ProviderListView::Catalog,
                    selected_provider_id: self
                        .rows
                        .get(self.selected_idx)
                        .map(|row| row.provider_id.clone()),
                }),
                KeyCode::Up => {
                    self.move_up();
                    ViewAction::None
                }
                KeyCode::Down => {
                    self.move_down();
                    ViewAction::None
                }
                // Row-dependent actions are no-ops when the current filter
                // (#3830) hides every row — e.g. a fresh Configured view
                // with nothing configured yet shows the empty state and
                // `selected_idx` doesn't point at anything on screen.
                KeyCode::Enter if self.row_visible(self.selected_idx) => {
                    let provider = self.selected_provider();
                    let provider_id = self.selected_provider_id();
                    if !self.selected_route_is_valid() {
                        ViewAction::None
                    } else if self.selected_has_key() {
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied {
                            provider,
                            provider_id,
                        })
                    } else if provider == ApiProvider::Moonshot
                        && crate::config::kimi_cli_credentials_valid()
                    {
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerKimiOAuthEnabled {
                            provider,
                        })
                    } else {
                        self.enter_key_entry();
                        ViewAction::None
                    }
                }
                KeyCode::Char(c)
                    if key.modifiers.is_empty()
                        && c.eq_ignore_ascii_case(&'r')
                        && self.query.is_empty()
                        && self.row_visible(self.selected_idx) =>
                {
                    self.enter_key_entry();
                    ViewAction::None
                }
                // Toggle between the configured-only default view and the
                // full provider catalog (#3830). Handled before the
                // type-ahead arm so `a`/`A` always toggles instead of
                // seeking a provider whose name starts with "a".
                KeyCode::Char(c)
                    if key.modifiers.is_empty()
                        && self.query.is_empty()
                        && c.eq_ignore_ascii_case(&'a') =>
                {
                    self.toggle_view();
                    ViewAction::None
                }
                KeyCode::Char(c)
                    if key.modifiers.is_empty()
                        && self.query.is_empty()
                        && c.eq_ignore_ascii_case(&'c') =>
                {
                    self.enter_custom_form();
                    ViewAction::None
                }
                // Jump to the `/model` picker pre-filtered to this provider
                // (#3083). Handled before the type-ahead arm so `m`/`M` opens
                // models instead of seeking a provider whose name starts with m.
                KeyCode::Char(c)
                    if key.modifiers.is_empty()
                        && self.query.is_empty()
                        && c.eq_ignore_ascii_case(&'m')
                        && self.row_visible(self.selected_idx) =>
                {
                    let provider = self.selected_provider();
                    let provider_id = self.selected_provider_id();
                    ViewAction::EmitAndClose(ViewEvent::ProviderPickerOpenModels {
                        provider,
                        provider_id,
                    })
                }
                KeyCode::Backspace if !self.query.is_empty() => {
                    let mut query = self.query.clone();
                    query.pop();
                    self.update_query(query);
                    ViewAction::None
                }
                KeyCode::Char(ch)
                    if key.modifiers.is_empty()
                        && !key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    let mut query = self.query.clone();
                    query.push(ch);
                    self.update_query(query);
                    ViewAction::None
                }
                _ => ViewAction::None,
            },
            Stage::KeyEntry => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::List;
                    self.api_key_input.clear();
                    self.key_entry_error = None;
                    self.pending_api_key = None;
                    self.model_options.clear();
                    self.model_selected_idx = 0;
                    self.selected_model = None;
                    ViewAction::None
                }
                KeyCode::Backspace => {
                    if !matches!(
                        self.selected_provider(),
                        ApiProvider::OpenaiCodex | ApiProvider::Xai
                    ) {
                        self.api_key_input.pop();
                        self.key_entry_error = None;
                    }
                    ViewAction::None
                }
                KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if !matches!(
                        self.selected_provider(),
                        ApiProvider::OpenaiCodex | ApiProvider::Xai
                    ) {
                        self.api_key_input.pop();
                        self.key_entry_error = None;
                    }
                    ViewAction::None
                }
                KeyCode::Enter => {
                    if self.selected_provider() == ApiProvider::OpenaiCodex {
                        return ViewAction::None;
                    }
                    if self.selected_provider() == ApiProvider::Xai {
                        return ViewAction::EmitAndClose(
                            ViewEvent::ProviderPickerXaiOAuthRequested,
                        );
                    }
                    let key = self.api_key_input.trim().to_string();
                    if key.is_empty() {
                        // Stay in key-entry; the user can press Esc to abort.
                        ViewAction::None
                    } else {
                        let provider = self.selected_provider();
                        let provider_id = self.selected_provider_id();
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerApiKeySubmitted {
                            provider,
                            provider_id,
                            api_key: key,
                        })
                    }
                }
                KeyCode::Char(c) => {
                    if matches!(
                        self.selected_provider(),
                        ApiProvider::OpenaiCodex | ApiProvider::Xai
                    ) {
                        return ViewAction::None;
                    }
                    // Reject ASCII whitespace so a stray space/tab doesn't slip
                    // into a credential; bracketed paste happens via the input
                    // path that already trims on submit.
                    if !c.is_whitespace() {
                        self.api_key_input.push(c);
                        self.key_entry_error = None;
                    }
                    ViewAction::None
                }
                _ => ViewAction::None,
            },
            Stage::ModelPick => match key.code {
                KeyCode::Esc => {
                    // Back to key entry with the validated key pre-filled so the
                    // user can retype without losing progress.
                    self.stage = Stage::KeyEntry;
                    if let Some(pending) = self.pending_api_key.clone() {
                        self.api_key_input = pending;
                    }
                    self.key_entry_error = None;
                    ViewAction::None
                }
                KeyCode::Up => {
                    self.move_model_selection(-1);
                    ViewAction::None
                }
                KeyCode::Down => {
                    self.move_model_selection(1);
                    ViewAction::None
                }
                KeyCode::Enter => {
                    if self.model_options.is_empty() {
                        return ViewAction::None;
                    }
                    self.selected_model = self.model_options.get(self.model_selected_idx).cloned();
                    self.enter_confirm();
                    ViewAction::None
                }
                _ => ViewAction::None,
            },
            Stage::Confirm => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::ModelPick;
                    ViewAction::None
                }
                KeyCode::Enter => self
                    .build_setup_confirmed_event()
                    .map(ViewAction::EmitAndClose)
                    .unwrap_or(ViewAction::None),
                _ => ViewAction::None,
            },
            Stage::CustomForm => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::List;
                    ViewAction::None
                }
                KeyCode::Tab | KeyCode::Down => {
                    self.advance_custom_field();
                    ViewAction::None
                }
                KeyCode::BackTab | KeyCode::Up => {
                    self.retreat_custom_field();
                    ViewAction::None
                }
                KeyCode::Backspace => {
                    self.custom_form_field_mut().pop();
                    ViewAction::None
                }
                KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.custom_form_field_mut().pop();
                    ViewAction::None
                }
                KeyCode::Enter if self.custom_provider_field != CustomProviderField::ApiKeyEnv => {
                    self.advance_custom_field();
                    ViewAction::None
                }
                KeyCode::Enter => self
                    .build_custom_provider_event()
                    .map(ViewAction::EmitAndClose)
                    .unwrap_or(ViewAction::None),
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    self.custom_form_field_mut().push(c);
                    ViewAction::None
                }
                _ => ViewAction::None,
            },
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        match self.stage {
            Stage::List => match mouse.kind {
                MouseEventKind::ScrollUp => self.move_up(),
                MouseEventKind::ScrollDown => self.move_down(),
                _ => {}
            },
            Stage::ModelPick => match mouse.kind {
                MouseEventKind::ScrollUp => self.move_model_selection(-1),
                MouseEventKind::ScrollDown => self.move_model_selection(1),
                _ => {}
            },
            Stage::KeyEntry | Stage::Confirm | Stage::CustomForm => {}
        }
        ViewAction::None
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let preferred_height = match self.stage {
            Stage::List => (self.rows.len() as u16).saturating_add(2),
            Stage::KeyEntry => 10,
            Stage::ModelPick => 12,
            Stage::Confirm => 10,
            Stage::CustomForm => 12,
        };
        let popup_area = centered_modal_area(area, 120, preferred_height, 64, 8);

        render_modal_surface(area, popup_area, buf);

        match self.stage {
            Stage::List => self.render_list(popup_area, buf),
            Stage::KeyEntry => self.render_key_entry(popup_area, buf),
            Stage::ModelPick => self.render_model_pick(popup_area, buf),
            Stage::Confirm => self.render_confirm(popup_area, buf),
            Stage::CustomForm => self.render_custom_form(popup_area, buf),
        }
    }
}

fn non_empty_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
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
        // The target may be hidden by the default configured-only view
        // (#3830); switch to the full catalog so navigation can still reach
        // it, matching what a user pressing `A` would do.
        if let Some(idx) = picker.rows.iter().position(|row| row.provider == provider)
            && !picker.row_visible(idx)
        {
            picker.toggle_view();
        }
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
    fn provider_picker_semantically_truncates_dense_rows_at_narrow_width() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        picker.toggle_view();

        let text = render_text(&picker, 64, 16);
        assert!(text.contains('…'), "{text}");
        for (idx, line) in text.lines().enumerate() {
            assert!(
                crate::tui::ui_text::text_display_width(line) <= 64,
                "line {idx} overflows: {line:?}"
            );
        }
    }

    #[test]
    fn type_ahead_jumps_to_provider_by_first_letter() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        // Z.ai isn't configured, so it's hidden by the default view (#3830);
        // browse the full catalog like a user pressing `A` would.
        picker.toggle_view();
        // Search for "zai" — unique enough to match only Z.ai.
        for c in "zai".chars() {
            picker.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(picker.query, "zai");
        let filtered = picker.filtered_rows();
        assert!(!filtered.is_empty(), "search for 'zai' must match Z.ai");
        assert!(
            filtered
                .iter()
                .any(|(_, row)| row.provider == ApiProvider::Zai),
            "Z.ai must be in filtered results: {:?}",
            filtered
                .iter()
                .map(|(_, r)| &r.display_name)
                .collect::<Vec<_>>()
        );
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
        // Scroll across the full catalog (#3830), not just the configured
        // subset, which would only contain the active provider here.
        picker.toggle_view();
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
    fn default_view_shows_only_configured_providers() {
        // #3830: with nothing but the active provider set up, the default
        // list view excludes the unconfigured catalog noise — even though
        // `rows` (the underlying data) still has every provider, per
        // `picker_lists_all_providers` above. Doesn't assert an exact count:
        // `OpenaiCodex` reads a real OAuth file from disk in
        // `has_api_key_for`, so it's legitimately "configured" on a machine
        // with a prior Codex login and must not make this test host-dependent.
        let config = Config::default();
        let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);

        assert_eq!(picker.view, ProviderListView::Configured);
        let visible: Vec<ApiProvider> = picker
            .filtered_rows()
            .iter()
            .map(|(_, row)| row.provider)
            .collect();
        assert!(visible.contains(&ApiProvider::Deepseek), "{visible:?}");
        assert!(
            !visible.contains(&ApiProvider::Custom),
            "the unused custom-provider placeholder slot isn't \"configured\": {visible:?}"
        );
        for unconfigured in [
            ApiProvider::Zai,
            ApiProvider::Openrouter,
            ApiProvider::Novita,
            ApiProvider::Ollama,
        ] {
            assert!(
                !visible.contains(&unconfigured),
                "{unconfigured:?} has no credentials and isn't active: {visible:?}"
            );
        }
        assert!(
            picker.rows.len() > visible.len(),
            "underlying data keeps every provider"
        );
    }

    #[test]
    fn explicit_provider_config_marks_provider_configured_without_active_or_key() {
        // #3830: a non-default `[providers.<name>]` entry (here just a base
        // URL override, no key) counts as "configured" even though the
        // provider is neither active nor has working credentials.
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                openrouter: crate::config::ProviderConfig {
                    base_url: Some("https://custom.openrouter.example/v1".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let row = picker
            .rows
            .iter()
            .find(|row| row.provider == ApiProvider::Openrouter)
            .expect("openrouter row");
        assert!(row.is_configured);
        assert!(!row.has_key, "explicit config doesn't imply a working key");
    }

    #[test]
    fn empty_provider_headers_do_not_mark_provider_configured() {
        let _env = crate::test_support::lock_test_env();
        let _anthropic_key = crate::test_support::EnvVarGuard::remove("ANTHROPIC_API_KEY");
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                anthropic: crate::config::ProviderConfig {
                    http_headers: Some(std::collections::HashMap::new()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let anthropic = picker
            .rows
            .iter()
            .find(|row| row.provider == ApiProvider::Anthropic)
            .expect("anthropic row");

        assert!(
            !anthropic.is_configured,
            "an empty deserialized header table is default state, not setup"
        );
    }

    #[test]
    fn non_empty_provider_headers_mark_provider_configured() {
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                anthropic: crate::config::ProviderConfig {
                    http_headers: Some(std::collections::HashMap::from([(
                        "X-Route".to_string(),
                        "custom".to_string(),
                    )])),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let anthropic = picker
            .rows
            .iter()
            .find(|row| row.provider == ApiProvider::Anthropic)
            .expect("anthropic row");

        assert!(
            anthropic.is_configured,
            "a user-authored header is meaningful explicit provider setup"
        );
    }

    #[test]
    fn blank_provider_header_entries_do_not_mark_provider_configured() {
        let _env = crate::test_support::lock_test_env();
        let _anthropic_key = crate::test_support::EnvVarGuard::remove("ANTHROPIC_API_KEY");
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                anthropic: crate::config::ProviderConfig {
                    http_headers: Some(std::collections::HashMap::from([
                        (" ".to_string(), "value".to_string()),
                        ("X-Blank".to_string(), "   ".to_string()),
                    ])),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        assert!(!crate::config::provider_is_configured_for_active(
            &config,
            ApiProvider::Anthropic,
            ApiProvider::Deepseek,
        ));
    }

    #[test]
    fn self_hosted_provider_not_auto_configured_without_explicit_setup() {
        // #3830: `has_api_key_for` always reports `true` for self-hosted
        // providers (no auth required to route to them) — that must not, on
        // its own, make Ollama/Sglang/Vllm show up in the default
        // configured-only view for every user regardless of setup.
        let config = Config::default();
        let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let ollama = picker
            .rows
            .iter()
            .find(|row| row.provider == ApiProvider::Ollama)
            .expect("ollama row");
        assert!(
            ollama.has_key,
            "self-hosted providers report has_key unconditionally"
        );
        assert!(
            !ollama.is_configured,
            "but that alone must not mark them configured"
        );

        // Active self-hosted provider still counts as configured.
        let active_picker = ProviderPickerView::new(ApiProvider::Ollama, &config);
        let active_ollama = active_picker
            .rows
            .iter()
            .find(|row| row.provider == ApiProvider::Ollama)
            .expect("ollama row");
        assert!(active_ollama.is_configured);
    }

    #[test]
    fn toggle_view_reveals_full_catalog_and_back() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let configured_count = picker.filtered_rows().len();
        assert_eq!(picker.view, ProviderListView::Configured);

        let action = picker.handle_key(key(KeyCode::Char('a')));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(picker.view, ProviderListView::Catalog);
        assert_eq!(picker.filtered_rows().len(), picker.rows.len());
        assert!(picker.filtered_rows().len() > configured_count);

        picker.handle_key(key(KeyCode::Char('A')));
        assert_eq!(picker.view, ProviderListView::Configured);
        assert_eq!(picker.filtered_rows().len(), configured_count);
    }

    #[test]
    fn key_entry_hint_uses_metadata_env_vars() {
        assert_eq!(
            ProviderPickerView::env_var_for(ApiProvider::NvidiaNim),
            "NVIDIA_API_KEY / NVIDIA_NIM_API_KEY / DEEPSEEK_API_KEY"
        );
    }

    #[test]
    fn key_entry_hint_includes_provider_credential_url() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::NvidiaNim);
        picker.handle_key(key(KeyCode::Enter));

        let rendered = render_text(&picker, 120, 20);

        assert!(rendered.contains("NVIDIA_API_KEY / NVIDIA_NIM_API_KEY / DEEPSEEK_API_KEY"));
        assert!(rendered.contains("https://build.nvidia.com/settings/api-keys"));
    }

    #[test]
    fn setup_provider_key_entry_matrix_keeps_hosted_codex_and_local_hints_distinct() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let codewhale_home = tmp.path().join(".codewhale");
        let _home = crate::test_support::EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", tmp.path());
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let _deepseek_key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _deepseek_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let _codex_key = crate::test_support::EnvVarGuard::remove("OPENAI_CODEX_ACCESS_TOKEN");
        let _codex_legacy_key = crate::test_support::EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
        let config = Config::default();

        let hosted = ProviderPickerView::new_for_setup(
            ApiProvider::Openai,
            Some(ApiProvider::Deepseek),
            &config,
            None,
        );
        assert_eq!(hosted.stage, Stage::KeyEntry);
        assert_eq!(hosted.selected_provider(), ApiProvider::Deepseek);
        let hosted_text = render_text(&hosted, 120, 20);
        assert!(hosted_text.contains("DEEPSEEK_API_KEY"), "{hosted_text}");
        assert!(
            hosted_text.contains("Credentials: https://platform.deepseek.com/api_keys"),
            "{hosted_text}"
        );
        assert!(!hosted_text.contains("OAuth login"), "{hosted_text}");

        let codex = ProviderPickerView::new_for_setup(
            ApiProvider::Deepseek,
            Some(ApiProvider::OpenaiCodex),
            &config,
            None,
        );
        assert_eq!(codex.stage, Stage::KeyEntry);
        assert_eq!(codex.selected_provider(), ApiProvider::OpenaiCodex);
        let codex_text = render_text(&codex, 120, 20);
        assert!(codex_text.contains("OAuth login"), "{codex_text}");
        assert!(
            codex_text.contains("OPENAI_CODEX_ACCESS_TOKEN"),
            "{codex_text}"
        );
        assert!(!codex_text.contains("Credentials:"), "{codex_text}");
        assert!(!codex_text.contains("(paste key here)"), "{codex_text}");

        let local = ProviderPickerView::new_for_setup(
            ApiProvider::Deepseek,
            Some(ApiProvider::Ollama),
            &config,
            None,
        );
        assert_eq!(local.stage, Stage::List);
        assert_eq!(local.selected_provider(), ApiProvider::Ollama);
        let local_text = render_text(&local, 120, 20);
        assert!(!local_text.contains("Credentials:"), "{local_text}");

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
        let _custom_key = crate::test_support::EnvVarGuard::remove("EXAMPLE_API_KEY");
        let custom_config = Config {
            provider: Some("my_thing".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Config::default()
        };
        let custom_picker =
            ProviderPickerView::new_for_setup(ApiProvider::Custom, None, &custom_config, None);
        let custom_row = &custom_picker.rows[custom_picker.selected_idx];
        assert_eq!(custom_row.provider, ApiProvider::Custom);
        assert_eq!(custom_row.provider_id, "my_thing");
        assert!(
            custom_row
                .messages
                .iter()
                .any(|message| message.contains("EXAMPLE_API_KEY")),
            "custom setup row should name its configured auth env var: {:?}",
            custom_row.messages
        );
        let custom_text = render_text(&custom_picker, 120, 20);
        assert!(custom_text.contains("my_thing"), "{custom_text}");
        assert!(custom_text.contains("EXAMPLE_API_KEY"), "{custom_text}");
        assert!(!custom_text.contains("Credentials:"), "{custom_text}");
    }

    #[test]
    fn provider_dashboard_row_models_local_readiness_without_rendering() {
        let config = Config::default();
        let row =
            ProviderDashboardRow::from_config(ApiProvider::Ollama, ApiProvider::Ollama, &config);

        assert_eq!(row.provider_id, "ollama");
        assert_eq!(row.auth_status, ProviderAuthStatus::Local);
        assert_eq!(row.readiness, ResolvedProviderReadiness::LocalUnchecked);
        assert_eq!(row.supported_protocols, vec!["chat".to_string()]);
        assert_eq!(row.usage_meter, "cost: local");
        assert!(row.base_url.contains("localhost:11434"));
        assert!(row.is_active);
    }

    #[test]
    fn deepseek_cn_row_uses_shared_readiness_and_strict_model_validation() {
        let _lock = crate::test_support::lock_test_env();
        let _key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let missing = Config {
            provider: Some("deepseek-cn".to_string()),
            ..Default::default()
        };
        let missing_row = ProviderDashboardRow::from_config(
            ApiProvider::DeepseekCN,
            ApiProvider::DeepseekCN,
            &missing,
        );
        assert_eq!(missing_row.readiness, ResolvedProviderReadiness::MissingKey);
        assert_ne!(missing_row.auth_status, ProviderAuthStatus::Legacy);

        let configured = Config {
            provider: Some("deepseek-cn".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                deepseek_cn: crate::config::ProviderConfig {
                    api_key: Some("deepseek-cn-test-key".to_string()),
                    model: Some("deepseek-v4-pro".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let configured_row = ProviderDashboardRow::from_config(
            ApiProvider::DeepseekCN,
            ApiProvider::DeepseekCN,
            &configured,
        );
        assert_eq!(
            configured_row.readiness,
            ResolvedProviderReadiness::SavedUnchecked
        );

        let mut invalid = configured;
        invalid
            .providers
            .as_mut()
            .expect("providers")
            .deepseek_cn
            .model = Some("anthropic/claude-foreign".to_string());
        let invalid_row = ProviderDashboardRow::from_config(
            ApiProvider::DeepseekCN,
            ApiProvider::DeepseekCN,
            &invalid,
        );
        assert_eq!(
            invalid_row.readiness,
            ResolvedProviderReadiness::InvalidRoute
        );
    }

    #[test]
    fn provider_health_requires_observed_success_and_keeps_failure_reason() {
        let config = Config {
            api_key: Some("saved-key".to_string()),
            ..Config::default()
        };
        let unchecked = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let row = unchecked
            .rows
            .iter()
            .find(|row| row.provider == ApiProvider::Deepseek)
            .expect("DeepSeek row");
        assert_eq!(row.readiness, ResolvedProviderReadiness::SavedUnchecked);

        let mut health = ProviderReadinessSnapshot::default();
        health.record_success(&config, ApiProvider::Deepseek, "deepseek-v4-pro");
        let ready =
            ProviderPickerView::new(ApiProvider::Deepseek, &config).with_provider_health(&health);
        assert_eq!(
            ready
                .rows
                .iter()
                .find(|row| row.provider == ApiProvider::Deepseek)
                .unwrap()
                .readiness,
            ResolvedProviderReadiness::Ready
        );

        health.record_failure_message(
            &config,
            ApiProvider::Deepseek,
            "deepseek-v4-pro",
            crate::error_taxonomy::ErrorCategory::Authentication,
            "credential rejected",
        );
        let failed =
            ProviderPickerView::new(ApiProvider::Deepseek, &config).with_provider_health(&health);
        let row = failed
            .rows
            .iter()
            .find(|row| row.provider == ApiProvider::Deepseek)
            .unwrap();
        assert!(row.readiness.label().contains("last check failed"));
        assert!(
            row.messages
                .iter()
                .any(|message| message == "credential rejected")
        );
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
    fn provider_row_query_matches_default_route_model_and_wire_id() {
        // #4141: cross-field search must also match the default route's display
        // model name and wire model id, keeping this picker consistent with the
        // model picker (`model_row_matches_query`). Z.ai's provider key,
        // display name, kind, and base URL contain no "glm", so a "glm" match
        // can only come from the route's model/wire fields.
        let config = Config {
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

        // Wire model id + display model name, case-insensitively.
        assert!(row.matches_query("glm-5.2"));
        assert!(row.matches_query("GLM"));
        // Provider name still matches, and an unrelated token still does not.
        assert!(row.matches_query("zhipu"));
        assert!(!row.matches_query("anthropic"));
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
        let _env_lock = crate::test_support::lock_test_env();
        let _sglang_key = crate::test_support::EnvVarGuard::remove("SGLANG_API_KEY");
        let _sglang_base_url = crate::test_support::EnvVarGuard::remove("SGLANG_BASE_URL");
        let _vllm_key = crate::test_support::EnvVarGuard::remove("VLLM_API_KEY");
        let _vllm_base_url = crate::test_support::EnvVarGuard::remove("VLLM_BASE_URL");
        let _ollama_key = crate::test_support::EnvVarGuard::remove("OLLAMA_API_KEY");
        let _ollama_base_url = crate::test_support::EnvVarGuard::remove("OLLAMA_BASE_URL");

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
    fn protected_self_hosted_row_requires_its_configured_auth_mode() {
        let _env_lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("isolated credential home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", temp.path());
        let _backend = crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
        let _vllm_key = crate::test_support::EnvVarGuard::remove("VLLM_API_KEY");
        let _vllm_base_url = crate::test_support::EnvVarGuard::remove("VLLM_BASE_URL");
        let config = Config {
            provider: Some("vllm".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                vllm: crate::config::ProviderConfig {
                    auth_mode: Some("api_key".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };

        let row = ProviderDashboardRow::from_config(ApiProvider::Vllm, ApiProvider::Vllm, &config);

        assert_eq!(row.auth_status, ProviderAuthStatus::Missing);
        assert_eq!(row.credential_state, CredentialState::MissingKey);
        assert_eq!(row.readiness, ResolvedProviderReadiness::MissingKey);
        assert!(row.compact_hint().contains("(self-hosted)"));
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
        assert_eq!(row.readiness, ResolvedProviderReadiness::SavedUnchecked);
        assert_eq!(row.base_url, "http://localhost:9000/v1");
        assert_eq!(row.default_route.logical_model, "custom-model");
        assert_eq!(row.default_route.wire_model, "custom-model");
        assert_eq!(row.supported_protocols, vec!["chat".to_string()]);
    }

    #[test]
    fn custom_endpoint_cannot_claim_official_xai_oauth_readiness() {
        let _lock = ENV_LOCK.lock().expect("env lock poisoned");
        let temp = tempfile::tempdir().expect("isolated oauth home");
        let _xai_key = EnvVarGuard::remove("XAI_API_KEY");
        let missing_grok_auth = temp.path().join("missing.json");
        let _grok_auth = EnvVarGuard::set(
            "GROK_AUTH_PATH",
            missing_grok_auth.to_str().expect("utf8 test path"),
        );
        let config = Config {
            provider: Some("xai".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                xai: crate::config::ProviderConfig {
                    base_url: Some("https://gateway.example.test/v1".to_string()),
                    model: Some("private-grok".to_string()),
                    auth_mode: Some("oauth".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };

        let row = ProviderDashboardRow::from_config(ApiProvider::Xai, ApiProvider::Xai, &config);

        assert_eq!(row.auth_status, ProviderAuthStatus::Missing);
        assert_eq!(row.credential_state, CredentialState::MissingKey);
        assert_eq!(row.readiness, ResolvedProviderReadiness::MissingKey);
        assert!(!row.compact_hint().contains("oauth"));
    }

    #[test]
    fn explicit_no_auth_custom_row_is_distinct_and_usable() {
        let custom = std::collections::HashMap::from([(
            "no-auth-gateway".to_string(),
            crate::config::ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("https://gateway.example.test/v1".to_string()),
                model: Some("private-model".to_string()),
                auth_mode: Some("no-auth".to_string()),
                ..Default::default()
            },
        )]);
        let config = Config {
            provider: Some("no-auth-gateway".to_string()),
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
            .find(|row| row.provider_id == "no-auth-gateway")
            .expect("configured no-auth row");

        assert_eq!(row.auth_status, ProviderAuthStatus::NoAuth);
        assert_eq!(row.credential_state, CredentialState::NoAuth);
        assert_eq!(row.readiness, ResolvedProviderReadiness::NoAuthUnchecked);
        assert!(picker.selected_has_key());
        assert!(row.compact_hint().contains("auth:none"));
    }

    #[test]
    fn unresolved_custom_auth_metadata_does_not_mark_picker_row_configured() {
        let custom = std::collections::HashMap::from([(
            "metadata-only".to_string(),
            crate::config::ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("https://gateway.example.test/v1".to_string()),
                model: Some("private-model".to_string()),
                auth: Some(codewhale_config::ProviderAuthSourceToml {
                    source: codewhale_config::AuthSourceKind::Command,
                    command: vec!["secret-tool".to_string(), "lookup".to_string()],
                    timeout_ms: Some(2_000),
                    secret_id: None,
                }),
                ..Default::default()
            },
        )]);
        let config = Config {
            provider: Some("metadata-only".to_string()),
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
            .find(|row| row.provider_id == "metadata-only")
            .expect("metadata-only row remains visible for repair");

        assert_eq!(row.auth_status, ProviderAuthStatus::Missing);
        assert_eq!(row.credential_state, CredentialState::MissingKey);
        assert_eq!(row.readiness, ResolvedProviderReadiness::MissingKey);
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
        assert_eq!(row.readiness, ResolvedProviderReadiness::MissingKey);
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
    fn provider_picker_marks_only_exact_active_custom_row() {
        let custom = std::collections::HashMap::from([
            (
                "custom-a".to_string(),
                crate::config::ProviderConfig {
                    kind: Some("openai-compatible".to_string()),
                    base_url: Some("http://127.0.0.1:18181/v1".to_string()),
                    model: Some("model-a".to_string()),
                    api_key: Some("test-key-a".to_string()),
                    ..Default::default()
                },
            ),
            (
                "custom-b".to_string(),
                crate::config::ProviderConfig {
                    kind: Some("openai-compatible".to_string()),
                    base_url: Some("http://127.0.0.1:18182/v1".to_string()),
                    model: Some("model-b".to_string()),
                    api_key: Some("test-key-b".to_string()),
                    ..Default::default()
                },
            ),
        ]);
        let config = Config {
            provider: Some("custom-a".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Config::default()
        };

        let rows = custom_provider_dashboard_rows(ApiProvider::Custom, &config, None);
        let active_ids: Vec<_> = rows
            .iter()
            .filter(|row| row.is_active)
            .map(|row| row.provider_id.as_str())
            .collect();

        assert_eq!(active_ids, vec!["custom-a"]);
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
        assert_eq!(row.readiness, ResolvedProviderReadiness::SavedUnchecked);
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
    fn custom_provider_form_emits_named_provider_without_secret_value() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);

        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('c'))),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::CustomForm);
        for ch in "acme_ai".chars() {
            picker.handle_key(key(KeyCode::Char(ch)));
        }
        picker.handle_key(key(KeyCode::Enter));
        for ch in "https://api.acme.example/v1".chars() {
            picker.handle_key(key(KeyCode::Char(ch)));
        }
        picker.handle_key(key(KeyCode::Enter));
        for ch in "acme/code-1".chars() {
            picker.handle_key(key(KeyCode::Char(ch)));
        }
        picker.handle_key(key(KeyCode::Enter));
        for ch in "ACME_API_KEY".chars() {
            picker.handle_key(key(KeyCode::Char(ch)));
        }

        let action = picker.handle_key(key(KeyCode::Enter));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerCustomProviderSubmitted {
                provider_id,
                base_url,
                model,
                api_key_env,
            }) => {
                assert_eq!(provider_id, "acme_ai");
                assert_eq!(base_url, "https://api.acme.example/v1");
                assert_eq!(model.as_deref(), Some("acme/code-1"));
                assert_eq!(api_key_env.as_deref(), Some("ACME_API_KEY"));
            }
            other => panic!("expected custom provider submit event, got {other:?}"),
        }
    }

    #[test]
    fn named_custom_provider_selection_preserves_provider_id() {
        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "local_acme".to_string(),
            crate::config::ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("http://localhost:9000/v1".to_string()),
                model: Some("acme/code-1".to_string()),
                ..Default::default()
            },
        );
        let config = Config {
            provider: Some("local_acme".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Config::default()
        };
        let mut picker = ProviderPickerView::new(ApiProvider::Custom, &config);

        let action = picker.handle_key(key(KeyCode::Enter));

        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied {
                provider,
                provider_id,
            }) => {
                assert_eq!(provider, ApiProvider::Custom);
                assert_eq!(provider_id.as_deref(), Some("local_acme"));
            }
            other => panic!("expected named custom provider apply, got {other:?}"),
        }
    }

    #[test]
    fn named_custom_provider_model_shortcut_preserves_provider_id() {
        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "local_acme".to_string(),
            crate::config::ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("http://localhost:9000/v1".to_string()),
                model: Some("acme/code-1".to_string()),
                ..Default::default()
            },
        );
        let config = Config {
            provider: Some("local_acme".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Config::default()
        };
        let mut picker = ProviderPickerView::new(ApiProvider::Custom, &config);

        let action = picker.handle_key(key(KeyCode::Char('m')));

        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerOpenModels {
                provider,
                provider_id,
            }) => {
                assert_eq!(provider, ApiProvider::Custom);
                assert_eq!(provider_id.as_deref(), Some("local_acme"));
            }
            other => panic!("expected named custom provider model shortcut, got {other:?}"),
        }
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
        assert_eq!(row.catalog_status, ProviderCatalogStatus::Bundled);
        assert!(row.available_model_count >= 3);
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
        assert_eq!(row.readiness, ResolvedProviderReadiness::MissingKey);
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
    fn provider_dashboard_row_marks_missing_api_key_as_needs_key() {
        let _lock = ENV_LOCK.lock().expect("env lock poisoned");
        let _openrouter_key = EnvVarGuard::remove("OPENROUTER_API_KEY");
        let config = Config::default();
        let row = ProviderDashboardRow::from_config(
            ApiProvider::Openrouter,
            ApiProvider::Deepseek,
            &config,
        );

        assert_eq!(row.auth_status, ProviderAuthStatus::Missing);
        assert_eq!(row.readiness, ResolvedProviderReadiness::MissingKey);
        assert_eq!(row.readiness.label(), "missing key");
        let hint = row.compact_hint();
        assert!(hint.contains("key:not-set"));
        assert!(!hint.contains("needs-auth"));
        assert!(!hint.contains("auth:missing"));
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
        assert_eq!(row.readiness, ResolvedProviderReadiness::InvalidRoute);
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

        assert!(rendered.contains("key:configured"));
        assert!(!rendered.contains("auth:configured"));
        assert!(rendered.contains("Route: custom-model"));
        assert!(rendered.contains("chat"));
        assert!(rendered.contains("cost: unknown"));
        assert!(rendered.contains("Endpoint: http://localhost:9000/v1"));
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
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied {
                provider,
                provider_id,
            }) => {
                assert_eq!(provider, ApiProvider::Ollama);
                assert_eq!(provider_id, None);
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
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerOpenModels {
                provider,
                provider_id,
            }) => {
                assert_eq!(provider, ApiProvider::Openrouter);
                assert_eq!(provider_id, None);
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
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerOpenModels {
                provider,
                provider_id,
            }) => {
                assert_eq!(provider, ApiProvider::Deepseek);
                assert_eq!(provider_id, None);
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
        // Wrap across the full catalog (#3830), not just the configured
        // subset, which would only contain the active provider here.
        picker.toggle_view();
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
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied {
                provider,
                provider_id,
            }) => {
                assert_eq!(provider, ApiProvider::Deepseek);
                assert_eq!(provider_id, None);
            }
            other => panic!("expected ProviderPickerApplied, got {other:?}"),
        }
    }

    #[test]
    fn new_for_missing_auth_opens_key_entry_focused_on_target() {
        // #3830: the missing-auth handoff drops the user onto the target
        // provider's key prompt, not a dead-end error.
        let config = Config::default();
        let picker = ProviderPickerView::new_for_missing_auth(
            ApiProvider::Deepseek,
            ApiProvider::Anthropic,
            &config,
            None,
        )
        .expect("Anthropic has a picker row");
        assert_eq!(picker.stage, Stage::KeyEntry);
        assert_eq!(picker.selected_provider(), ApiProvider::Anthropic);
    }

    #[test]
    fn setup_catalog_shows_all_providers_from_configured_view() {
        let config = Config::default();
        let picker = ProviderPickerView::new_for_setup(ApiProvider::Deepseek, None, &config, None);

        assert_eq!(picker.stage, Stage::List);
        assert_eq!(picker.view, ProviderListView::Catalog);
        assert_eq!(picker.visible_row_count(), picker.rows.len());
    }

    #[test]
    fn setup_catalog_focuses_missing_provider_key_entry() {
        let _lock = crate::test_support::lock_test_env();
        let _anthropic_key = crate::test_support::EnvVarGuard::remove("ANTHROPIC_API_KEY");
        let config = Config::default();
        let picker = ProviderPickerView::new_for_setup(
            ApiProvider::Deepseek,
            Some(ApiProvider::Anthropic),
            &config,
            None,
        );

        assert_eq!(picker.view, ProviderListView::Catalog);
        assert_eq!(picker.stage, Stage::KeyEntry);
        assert_eq!(picker.selected_provider(), ApiProvider::Anthropic);
        assert!(picker.api_key_input.is_empty());
    }

    #[test]
    fn setup_catalog_uses_setup_title() {
        let config = Config::default();
        let picker = ProviderPickerView::new_for_setup(ApiProvider::Deepseek, None, &config, None);

        let rendered = render_text(&picker, 96, 20);

        assert!(rendered.contains("Provider setup"));
    }

    #[test]
    fn setup_catalog_key_entry_uses_setup_reopen_hint() {
        let config = Config::default();
        let picker = ProviderPickerView::new_for_setup(
            ApiProvider::Deepseek,
            Some(ApiProvider::Anthropic),
            &config,
            None,
        );

        let rendered = render_text(&picker, 96, 20);

        assert!(rendered.contains("API key"));
        assert!(rendered.contains("/setup provider"));
        assert!(!rendered.contains("re-open /provider."));
    }

    #[test]
    fn default_provider_picker_keeps_provider_reopen_hint() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Anthropic);
        picker.handle_key(key(KeyCode::Enter));

        let rendered = render_text(&picker, 96, 20);

        assert!(rendered.contains("API key"));
        assert!(rendered.contains("re-open /provider."));
        assert!(!rendered.contains("/setup provider"));
    }

    #[test]
    fn setup_catalog_focuses_configured_provider_without_rekeying() {
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                openai: crate::config::ProviderConfig {
                    api_key: Some("openai-key".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let picker = ProviderPickerView::new_for_setup(
            ApiProvider::Deepseek,
            Some(ApiProvider::Openai),
            &config,
            None,
        );

        assert_eq!(picker.view, ProviderListView::Catalog);
        assert_eq!(picker.stage, Stage::List);
        assert_eq!(picker.selected_provider(), ApiProvider::Openai);
    }

    #[test]
    fn new_for_key_entry_with_error_opens_prompt_and_renders_reason() {
        let config = Config::default();
        let picker = ProviderPickerView::new_for_key_entry_with_error(
            ApiProvider::Deepseek,
            ApiProvider::Openrouter,
            &config,
            None,
            "HTTP 401: unauthorized".to_string(),
        )
        .expect("OpenRouter has a picker row");

        assert_eq!(picker.stage, Stage::KeyEntry);
        assert_eq!(picker.selected_provider(), ApiProvider::Openrouter);
        let rendered = render_text(&picker, 90, 14);
        assert!(rendered.contains("Verification failed: HTTP 401: unauthorized"));
    }

    #[test]
    fn new_for_model_pick_after_validation_opens_model_stage() {
        let config = Config::default();
        let picker = ProviderPickerView::new_for_model_pick_after_validation(
            ApiProvider::Deepseek,
            ApiProvider::Openrouter,
            &config,
            None,
            "sk-validated".to_string(),
        )
        .expect("OpenRouter has a picker row");

        assert_eq!(picker.stage, Stage::ModelPick);
        assert_eq!(picker.selected_provider(), ApiProvider::Openrouter);
        assert_eq!(picker.pending_api_key.as_deref(), Some("sk-validated"));
        assert!(!picker.model_options.is_empty());
        assert!(picker.selected_model.is_some());
    }

    #[test]
    fn model_pick_enter_advances_to_confirm_and_confirm_emits_setup() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new_for_model_pick_after_validation(
            ApiProvider::Deepseek,
            ApiProvider::Openrouter,
            &config,
            None,
            "sk-validated".to_string(),
        )
        .expect("OpenRouter has a picker row");

        assert_eq!(picker.stage, Stage::ModelPick);
        let action = picker.handle_key(key(KeyCode::Enter));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(picker.stage, Stage::Confirm);

        let selected_model = picker
            .selected_model
            .clone()
            .expect("model selected on confirm");
        let action = picker.handle_key(key(KeyCode::Enter));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerSetupConfirmed {
                provider,
                provider_id,
                api_key,
                model,
            }) => {
                assert_eq!(provider, ApiProvider::Openrouter);
                assert_eq!(provider_id, None);
                assert_eq!(api_key, "sk-validated");
                assert_eq!(model, selected_model);
            }
            other => panic!("expected ProviderPickerSetupConfirmed, got {other:?}"),
        }
    }

    #[test]
    fn model_pick_and_confirm_esc_backs_out_without_emitting() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new_for_model_pick_after_validation(
            ApiProvider::Deepseek,
            ApiProvider::Openrouter,
            &config,
            None,
            "sk-validated".to_string(),
        )
        .expect("OpenRouter has a picker row");

        picker.handle_key(key(KeyCode::Enter));
        assert_eq!(picker.stage, Stage::Confirm);
        assert!(matches!(
            picker.handle_key(key(KeyCode::Esc)),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::ModelPick);

        assert!(matches!(
            picker.handle_key(key(KeyCode::Esc)),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::KeyEntry);
        assert_eq!(picker.api_key_input, "sk-validated");
        assert!(picker.pending_api_key.is_some());
    }

    #[test]
    fn guided_flow_stages_render_at_80x24_and_120x32() {
        let config = Config::default();
        let model_pick = ProviderPickerView::new_for_model_pick_after_validation(
            ApiProvider::Deepseek,
            ApiProvider::Openrouter,
            &config,
            None,
            "sk-validated-key".to_string(),
        )
        .expect("OpenRouter has a picker row");
        let mut confirm = ProviderPickerView::new_for_model_pick_after_validation(
            ApiProvider::Deepseek,
            ApiProvider::Openrouter,
            &config,
            None,
            "sk-validated-key".to_string(),
        )
        .expect("OpenRouter has a picker row");
        confirm.handle_key(key(KeyCode::Enter));
        assert_eq!(confirm.stage, Stage::Confirm);

        for (w, h) in [(80u16, 24u16), (120u16, 32u16)] {
            let model_text = render_text(&model_pick, w, h);
            assert!(
                model_text.contains("Default model") || model_text.contains("default model"),
                "{w}x{h} model pick missing title:\n{model_text}"
            );
            assert!(
                model_text.contains("continue") || model_text.contains("Enter"),
                "{w}x{h} model pick missing continue affordance:\n{model_text}"
            );
            for (idx, line) in model_text.lines().enumerate() {
                assert!(
                    crate::tui::ui_text::text_display_width(line) <= w as usize,
                    "{w}x{h} model pick line {idx} overflows: {line:?}"
                );
            }

            let confirm_text = render_text(&confirm, w, h);
            assert!(
                confirm_text.contains("Confirm"),
                "{w}x{h} confirm missing title:\n{confirm_text}"
            );
            assert!(
                confirm_text.contains("Provider:") || confirm_text.contains("OpenRouter"),
                "{w}x{h} confirm missing provider summary:\n{confirm_text}"
            );
            assert!(
                confirm_text.contains("Model:") || confirm_text.contains("model"),
                "{w}x{h} confirm missing model summary:\n{confirm_text}"
            );
            // Masked key only — never the raw secret.
            assert!(
                !confirm_text.contains("sk-validated-key"),
                "{w}x{h} confirm leaked raw key:\n{confirm_text}"
            );
            for (idx, line) in confirm_text.lines().enumerate() {
                assert!(
                    crate::tui::ui_text::text_display_width(line) <= w as usize,
                    "{w}x{h} confirm line {idx} overflows: {line:?}"
                );
            }
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

        let rendered = render_text(&picker, 80, 14);

        assert!(rendered.contains("Enter"), "rendered: {rendered}");
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
                provider_id,
                api_key,
            }) => {
                assert_eq!(provider, ApiProvider::Novita);
                assert_eq!(provider_id, None);
                assert_eq!(api_key, "novita-key");
            }
            other => panic!("expected ProviderPickerApiKeySubmitted, got {other:?}"),
        }
    }

    #[test]
    fn openai_codex_key_entry_is_oauth_only() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new_for_missing_auth(
            ApiProvider::Deepseek,
            ApiProvider::OpenaiCodex,
            &config,
            None,
        )
        .expect("OpenAI Codex has a picker row");
        assert_eq!(picker.stage, Stage::KeyEntry);

        let rendered = render_text(&picker, 96, 20);
        assert!(rendered.contains("OAuth login"));
        assert!(rendered.contains("no token is stored here"));
        assert!(!rendered.contains("save & switch"));
        assert!(!rendered.contains("(paste key here)"));
        assert!(!rendered.contains("Credentials:"));

        assert!(picker.handle_paste("codex-token"));
        for c in "codex-token".chars() {
            picker.handle_key(key(KeyCode::Char(c)));
        }
        assert!(picker.api_key_input.is_empty());
        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::None
        ));
    }

    #[test]
    fn xai_key_entry_launches_device_oauth() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new_for_missing_auth(
            ApiProvider::Deepseek,
            ApiProvider::Xai,
            &config,
            None,
        )
        .expect("xAI has a picker row");
        assert_eq!(picker.stage, Stage::KeyEntry);

        let rendered = render_text(&picker, 96, 20);
        assert!(rendered.contains("OAuth login"));
        assert!(rendered.contains("device login"));
        assert!(rendered.contains("~/.grok/auth.json"));
        assert!(!rendered.contains("(paste key here)"));

        assert!(picker.handle_paste("xai-token"));
        assert!(picker.api_key_input.is_empty());
        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerXaiOAuthRequested)
        ));
    }

    #[test]
    fn xai_auth_status_distinguishes_oauth_from_api_key_auth() {
        let oauth_config = crate::config::ProviderConfig {
            auth_mode: Some("oauth".to_string()),
            ..Default::default()
        };
        assert_eq!(
            xai_oauth_status(Some(&oauth_config), false),
            Some(ProviderAuthStatus::OAuthMissing)
        );
        assert_eq!(
            xai_oauth_status(Some(&oauth_config), true),
            Some(ProviderAuthStatus::OAuthReady)
        );
        assert_eq!(xai_oauth_status(None, true), None);
        assert_eq!(xai_oauth_status(None, false), None);
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
    fn list_esc_emits_dismiss_memory() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let action = picker.handle_key(key(KeyCode::Esc));
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerDismissed { .. })
        ));
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
    fn tall_catalog_render_shows_selected_provider_details() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        // "All providers" means the full catalog (#3830), not just configured.
        picker.toggle_view();

        let rendered = render_text(&picker, 80, 23);

        assert!(rendered.contains("DeepSeek *"));
        assert!(rendered.contains("Details"));
        assert!(rendered.contains("Route:"));
    }

    /// The four terminal sizes the v0.8.66 modal blocker (#3732) requires every
    /// overlay to remain readable and fully operable at.
    const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

    #[test]
    fn provider_picker_is_usable_and_opaque_at_blocker_sizes() {
        use crate::tui::views::ViewStack;
        // Provider display names contain capital X/Q (Xiaomi MiMo, Qianfan), so
        // use a glyph that can never appear in the modal content as the
        // bleed-through sentinel.
        const SENTINEL: &str = "\u{2592}"; // ▒
        let config = Config::default();
        // Make the first provider in the sorted list active so its highlighted
        // row sits at the top of the list, never on the vertical center cell
        // that must read as the opaque modal ink.
        let active = ProviderPickerView::new(ApiProvider::Deepseek, &config).rows[0].provider;

        for (w, h) in BLOCKER_SIZES {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            for y in 0..h {
                for x in 0..w {
                    buf[(x, y)].set_symbol(SENTINEL);
                }
            }
            // Render through the ViewStack so the shared opaque backdrop is
            // painted exactly as it is in production.
            let mut stack = ViewStack::new();
            stack.push(ProviderPickerView::new(active, &config));
            stack.render(area, &mut buf);

            let rows: Vec<String> = (0..h)
                .map(|y| {
                    (0..w)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect();
            let text = rows.join("\n");

            // Footer keeps every action (it wraps instead of clipping).
            for label in ["move", "jump", "edit key", "models", "cancel"] {
                assert!(text.contains(label), "{w}x{h}: missing '{label}' hint");
            }
            // The Enter action label is dynamic (apply vs set key); one shows.
            assert!(
                text.contains("apply") || text.contains("set key"),
                "{w}x{h}: missing Enter action label"
            );
            // Composited frame is fully opaque: no sentinel survives and the
            // center cell carries the modal ink background.
            assert!(
                !text.contains(SENTINEL),
                "{w}x{h}: background bleed-through into modal surface"
            );
            assert_eq!(
                buf[(w / 2, h / 2)].bg,
                palette::WHALE_BG,
                "{w}x{h}: modal interior must be opaque"
            );
            // No row exceeds the frame width (no horizontal overflow).
            for (y, row) in rows.iter().enumerate() {
                assert!(
                    unicode_width::UnicodeWidthStr::width(row.trim_end()) <= w as usize,
                    "{w}x{h}: row {y} overflows width: {row:?}"
                );
            }
        }
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

    #[test]
    fn esc_reports_browsing_context_and_reopen_restores_it() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        // Browse full catalog and move highlight.
        picker.handle_key(key(KeyCode::Char('a')));
        picker.handle_key(key(KeyCode::Down));
        let remembered_id = picker.rows[picker.selected_idx].provider_id.clone();
        let action = picker.handle_key(key(KeyCode::Esc));
        let ViewAction::EmitAndClose(ViewEvent::ProviderPickerDismissed {
            catalog_view,
            selected_provider_id,
        }) = action
        else {
            panic!("expected ProviderPickerDismissed");
        };
        assert!(catalog_view);
        assert_eq!(
            selected_provider_id.as_deref(),
            Some(remembered_id.as_str())
        );

        let memory = crate::tui::app::ProviderPickerMemory {
            catalog_view,
            selected_provider_id,
        };
        let reopened = ProviderPickerView::new_with_runtime_status_and_memory(
            ApiProvider::Deepseek,
            &config,
            None,
            Some(&memory),
        );
        assert_eq!(reopened.view, ProviderListView::Catalog);
        assert_eq!(
            reopened.rows[reopened.selected_idx].provider_id,
            remembered_id
        );
    }
}
