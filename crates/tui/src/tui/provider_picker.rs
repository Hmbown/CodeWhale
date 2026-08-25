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

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

use crate::config::{ApiProvider, Config, base_url_uses_local_host, provider_is_configured};
use crate::core::ops::ProviderRuntimeStatus;
use crate::localization::{Locale, MessageId, tr};
use crate::model_profile::{
    SupportState, resolved_capability_profile, resolved_capability_profile_for_route,
};
use crate::models_dev_live::{self, ModelsDevFreshness};
use crate::palette;
use crate::provider_lake::{catalog_model_count_for_provider, catalog_offering_for_model};
use crate::provider_readiness::{
    CredentialState, ProviderReadinessSnapshot, ProviderRouteIdentity, ResolvedProviderReadiness,
    credential_state_for_provider, route_identity_for_model,
};
use crate::tui::app::ReasoningEffort;
use crate::tui::menu_style;
use crate::tui::views::{
    ActionHint, EmptyState, ListDetailLayout, ModalKind, ModalView, ViewAction, ViewEvent,
    centered_modal_area, render_modal_footer, render_modal_surface,
};
use codewhale_config::catalog::{CatalogOffering, CatalogSnapshot};
use codewhale_config::provider::{CredentialAcquisition, WireFormat};
use codewhale_config::route::{PricingSku, RequestProtocol};
use codewhale_config::{
    AGNES_TEMPLATE_ID, ProviderSetupApply, ProviderSetupTemplate, SENSENOVA_TEMPLATE_ID,
    provider_setup_template, provider_setup_templates,
};
use serde_json::Value;
use std::borrow::Cow;
use std::cell::RefCell;
use std::sync::OnceLock;

const DS4_PROVIDER_ID: &str = "ds4";
const DS4_BASE_URL: &str = "http://127.0.0.1:8000/v1";
const DS4_DEFAULT_MODEL: &str = "deepseek-v4-flash";
const LM_STUDIO_PROVIDER_ID: &str = "lm_studio";
const LM_STUDIO_BASE_URL: &str = "http://127.0.0.1:1234/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    List,
    /// Explicit xAI acquisition choice. xAI supports both an API key and the
    /// Codewhale-owned device OAuth flow; neither path may impersonate the other.
    XaiAuthChoice,
    KeyEntry,
    /// Explicit disabled/read-only/managed external-credential policy choice.
    ExternalConsentChoice,
    /// Full owner/path/side-effect disclosure before a read grant is saved.
    ExternalConsentConfirm,
    /// Default model pick after a key has been live-validated (#3875).
    ModelPick,
    /// Kimi Code membership plan selection for the exact `api.kimi.com` route.
    PlanTier,
    /// StepFun pay-as-you-go vs Step Plan endpoint choice, asked before key
    /// entry so the selected route is the one that gets live-validated (#4526).
    StepfunBillingRoute,
    /// Confirmation summary before any secret or model is persisted (#3875).
    Confirm,
    CustomForm,
    /// Beginner template catalog (#5350).
    TemplateList,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalConsentChoice {
    Disabled,
    ReadOnly,
    ManagedUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XaiAuthChoice {
    ApiKey,
    DeviceOAuth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KimiCodePlanTier {
    Safe262k,
    OneMillion,
}

/// StepFun's two billing tracks. They are separate endpoints, not separate
/// keys, so the setup wizard has to pick one before a key can be validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepfunBillingRoute {
    PayAsYouGo,
    StepPlan,
}

impl StepfunBillingRoute {
    fn base_url(self) -> &'static str {
        match self {
            Self::PayAsYouGo => crate::config::DEFAULT_STEPFUN_BASE_URL,
            Self::StepPlan => crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL,
        }
    }
}

/// Whether the StepFun billing-route choice applies to `base_url`.
///
/// Only the two endpoints Codewhale can classify are offered. A hand-edited
/// endpoint (regional proxy, gateway, anything unrecognized) is a deliberate
/// user choice, so the stage is skipped rather than silently rewriting it.
fn stepfun_route_is_selectable(provider: ApiProvider, base_url: &str) -> bool {
    provider == ApiProvider::Stepfun
        && matches!(
            crate::pricing::billing_surface_for_route(provider, Some(base_url)),
            Some(crate::pricing::STEPFUN_PAYG_BILLING_SURFACE)
                | Some(crate::pricing::STEPFUN_PLAN_BILLING_SURFACE)
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CustomProviderField {
    Name,
    BaseUrl,
    Model,
    ApiKeyEnv,
}

/// Which subset of `rows` the list stage shows (#3830). `Configured` is the
/// normal `/provider` default; first-run onboarding opens `Local` so a user
/// can start with Ollama, SGLang, or vLLM without walking through cloud-key
/// setup. `A` still exposes the full catalog and `L` returns to local routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderListView {
    Configured,
    Catalog,
    Local,
}

pub struct ProviderPickerView {
    rows: Vec<ProviderDashboardRow>,
    selected_idx: usize,
    stage: Stage,
    view: ProviderListView,
    setup_mode: bool,
    /// First-run/recovery keeps the canonical provider engine but removes
    /// the advanced management hotkeys from the decision surface. They remain
    /// available from `/provider` after onboarding.
    onboarding_mode: bool,
    query: String,
    api_key_input: String,
    /// An error surfaced after a failed key verification, shown inline
    /// in the key-entry stage. Cleared when the user edits the input.
    key_entry_error: Option<String>,
    locale: Locale,
    xai_auth_choice: XaiAuthChoice,
    external_consent_choice: ExternalConsentChoice,
    /// Validated key held only in memory until the confirm stage persists it.
    pending_api_key: Option<String>,
    /// Catalog models offered during the model-pick stage.
    model_options: Vec<String>,
    model_selected_idx: usize,
    /// Model chosen on the model-pick stage (and shown on confirm).
    selected_model: Option<String>,
    selected_context_window: Option<u32>,
    kimi_code_plan_tier: KimiCodePlanTier,
    stepfun_billing_route: StepfunBillingRoute,
    /// Endpoint chosen in the setup wizard, carried unpersisted through key
    /// validation and only written on confirm (#4526).
    pending_base_url: Option<String>,
    custom_provider_field: CustomProviderField,
    custom_provider_id: String,
    custom_provider_base_url: String,
    custom_provider_model: String,
    custom_provider_api_key_env: String,
    template_selected_idx: usize,
    template_row_hitboxes: RefCell<Vec<(Rect, usize)>>,
    last_template_mouse_selected: Option<usize>,
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
    external_credential_status: Option<codewhale_config::ExternalCredentialConsentStatus>,
    pub is_active: bool,
    has_key: bool,
    /// Human-readable name of the place this row's credential resolved from,
    /// or "not found". Ported from pi-mono's `AuthResult.source`; a label
    /// only, never secret material.
    pub(crate) credential_source: String,
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
    OAuthConsented,
    OAuthMissing,
    ImportedTokenUnavailable,
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
    /// Source receipt for the route-effective context-window badge.
    pub context_window_source: Option<String>,
    pub max_output: Option<u32>,
    pub tools: SupportState,
    pub structured: SupportState,
    pub streaming: SupportState,
    pub cache: SupportState,
    pub vision: SupportState,
}

impl ProviderCapabilityBadges {
    fn for_route(provider: ApiProvider, wire_model: &str) -> Self {
        let cap = catalog_offering_for_model(provider, wire_model).map_or_else(
            || resolved_capability_profile(provider, wire_model),
            |offering| {
                let route_offering = offering.to_offering();
                resolved_capability_profile_for_route(
                    provider,
                    wire_model,
                    route_offering.capabilities,
                    route_offering.limits,
                )
            },
        );
        Self {
            context_window: cap.context_window,
            context_window_source: None,
            max_output: cap.max_output,
            tools: cap.native_tool_calls,
            structured: cap.structured_output,
            streaming: cap.streaming,
            cache: cap.prompt_caching,
            vision: cap.image_input,
        }
    }

    fn unknown() -> Self {
        Self {
            context_window: None,
            context_window_source: None,
            max_output: None,
            tools: SupportState::Unknown,
            structured: SupportState::Unknown,
            streaming: SupportState::Unknown,
            cache: SupportState::Unknown,
            vision: SupportState::Unknown,
        }
    }

    /// Compact, never-fabricating badge cluster. Metadata and each capability
    /// render `?` when unknown rather than being silently dropped.
    fn label(&self) -> String {
        format!(
            "ctx:{}({}) out:{} tools:{} json:{} stream:{} cache:{} vision:{}",
            humanize_token_count(self.context_window),
            self.context_window_source.as_deref().unwrap_or("?"),
            humanize_token_count(self.max_output),
            support_glyph(self.tools),
            support_glyph(self.structured),
            support_glyph(self.streaming),
            support_glyph(self.cache),
            support_glyph(self.vision),
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
        let uses_kimi_imported_token = provider == ApiProvider::Moonshot
            && configured.is_some_and(crate::config::provider_config_uses_kimi_imported_token);
        let configured_base_url = configured_base_url.or_else(|| {
            uses_kimi_imported_token.then(|| crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string())
        });
        let explicitly_configured_model = configured
            .and_then(|entry| entry.model.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let has_configured_model = explicitly_configured_model.is_some();
        let configured_model = explicitly_configured_model.or_else(|| {
            uses_kimi_imported_token.then(|| crate::config::DEFAULT_KIMI_CODE_MODEL.to_string())
        });
        let model_origin = ProviderModelOrigin::for_provider(provider, has_configured_model);
        // One sourced resolution per row: the picker must be able to say WHERE
        // it looked, not just that it found nothing (pi-mono's `AuthResult`
        // source, ported in `crate::credentials`).
        let credential_resolution = crate::config::resolve_credential_source(config, provider);
        let has_key = if provider == ApiProvider::Custom {
            custom_provider_has_auth(configured)
        } else {
            credential_resolution.is_present()
        };
        let credential_source = credential_resolution.source.label().into_owned();
        let credential_state = credential_state_for_provider(config, provider);
        let auth_mode = config.auth_mode_for_provider(provider);
        let no_auth = crate::config::auth_mode_disables_api_key(auth_mode.as_deref());
        let api_key_required = crate::config::auth_mode_requires_api_key(auth_mode.as_deref());
        let official_endpoint = !config.provider_uses_custom_endpoint(provider);
        let auth_base_url = config.base_url_for_route(provider);
        let xai_oauth_ready = provider == ApiProvider::Xai
            && official_endpoint
            && crate::xai_oauth::credentials_valid(config);
        let auth_status = if credential_state == CredentialState::ExternalConsent {
            ProviderAuthStatus::OAuthConsented
        } else {
            auth_status_for(
                provider,
                &auth_base_url,
                has_key,
                configured,
                no_auth,
                api_key_required,
                official_endpoint,
                xai_oauth_ready,
            )
        };
        let usage_meter = if matches!(auth_status, ProviderAuthStatus::ImportedTokenUnavailable) {
            "usage: Kimi API key required".to_string()
        } else {
            usage_meter_for(provider)
        };
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
                external_credential_status: None,
                is_active,
                has_key,
                credential_source,
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
        let mut messages = Vec::new();
        // Use the same route-effective resolver as the active runtime. In
        // particular, Kimi Code's bare K3 model has a conservative 262K
        // membership-plan baseline (or an explicit configured override), not
        // the generic catalog's unknown-model fallback.
        let route = crate::route_runtime::resolve_route_candidate_with_context_metadata(
            provider,
            configured_model.as_deref(),
            None,
            // The legacy CN alias shares DeepSeek's strict model contract.
            // Passing its endpoint as a generic override would classify the
            // route as custom and accidentally accept foreign model ids.
            (provider != ApiProvider::DeepseekCN)
                .then(|| configured_base_url.clone())
                .flatten(),
            config.context_window_for_provider_config(provider),
            None,
        );
        let (
            base_url,
            supported_protocols,
            default_route,
            resolved_pricing,
            route_ok,
            route_context_window,
            route_context_window_source,
        ) = match route {
            Ok(resolution) => {
                let candidate = resolution.candidate;
                if !candidate.validation().messages.is_empty() {
                    messages.extend(candidate.validation().messages.clone());
                }
                (
                    if provider == ApiProvider::DeepseekCN {
                        configured_base_url
                            .clone()
                            .unwrap_or_else(|| provider.default_base_url().to_string())
                    } else {
                        candidate.endpoint().base_url.clone()
                    },
                    vec![protocol_label(candidate.protocol()).to_string()],
                    ProviderDefaultRoute {
                        logical_model: candidate.logical_model().raw().to_string(),
                        wire_model: candidate.wire_model_id().as_str().to_string(),
                    },
                    pricing_label(provider, candidate.pricing()),
                    candidate.validation().ok,
                    Some(resolution.context_window.tokens),
                    Some(resolution.context_window.source.label().to_string()),
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
                    None,
                    None,
                )
            }
        };
        let resolved_pricing =
            if matches!(auth_status, ProviderAuthStatus::ImportedTokenUnavailable) {
                usage_meter
            } else if provider == ApiProvider::Ollama
                && !crate::config::provider_route_is_keyless_self_hosted(provider, &base_url)
                && resolved_pricing == "cost: local"
            {
                "cost: unknown".to_string()
            } else {
                resolved_pricing
            };

        if matches!(
            auth_status,
            ProviderAuthStatus::Missing
                | ProviderAuthStatus::OAuthMissing
                | ProviderAuthStatus::ImportedTokenUnavailable
        ) {
            messages.push(missing_auth_message(
                provider,
                configured,
                &provider_id,
                &credential_resolution,
            ));
        }
        if catalog_status == ProviderCatalogStatus::DefaultOnly {
            messages.push("catalog snapshot missing; using provider default".to_string());
        }

        let route_identity =
            route_identity_for_model(config, provider, &default_route.logical_model);
        let readiness = readiness_for(
            &route_identity,
            credential_state,
            route_ok,
            &ProviderReadinessSnapshot::default(),
        );
        let reasoning =
            ProviderReasoningSummary::for_route(provider, &base_url, &default_route, config);
        let mut capabilities =
            ProviderCapabilityBadges::for_route(provider, &default_route.wire_model);
        if let Some(context_window) = route_context_window {
            capabilities.context_window = Some(context_window);
        }
        capabilities.context_window_source = route_context_window_source;
        let external_credential_status = config.external_credential_consent_status(provider);

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
            external_credential_status,
            is_active,
            has_key,
            credential_source,
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
            ProviderListView::Local => format!(
                "local · no cloud key · {} · {}",
                compact_base_url(&self.base_url),
                self.default_route.logical_model
            ),
        }
    }

    fn compact_hint(&self) -> String {
        // Self-hosted providers carry a local/private posture; surface it next
        // to the base URL so the row reads correctly without a key (#3083).
        let self_hosted =
            if crate::config::provider_route_is_keyless_self_hosted(self.provider, &self.base_url)
                || matches!(
                    self.auth_status,
                    ProviderAuthStatus::Local | ProviderAuthStatus::Optional
                )
            {
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
    fn for_route(
        provider: ApiProvider,
        base_url: &str,
        route: &ProviderDefaultRoute,
        config: &Config,
    ) -> Self {
        if provider == ApiProvider::OpenaiCodex {
            return Self {
                support: ProviderReasoningSupport::Supported,
                controls: codex_reasoning_controls(),
                stream_visibility: ProviderReasoningStreamVisibility::StructuredThinking,
                selected_control: selected_reasoning_control(provider, config),
            };
        }

        // The bare `k3` ID is deliberately not listed as a generic Moonshot
        // model. Kimi Code owns this reasoning contract only at its exact
        // membership-plan endpoint, so surface the capability before key
        // entry without attributing it to neighboring Moonshot routes.
        if crate::config::is_exact_kimi_code_k3_route(provider, base_url, &route.wire_model) {
            return Self {
                support: ProviderReasoningSupport::Supported,
                controls: vec!["low".to_string(), "high".to_string(), "max".to_string()],
                stream_visibility: configured_or_default_stream_visibility(
                    provider,
                    config,
                    ProviderReasoningSupport::Supported,
                ),
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
            Self::OAuthConsented => "auth:oauth-consented-select-to-check",
            Self::OAuthMissing => "auth:oauth-missing",
            Self::ImportedTokenUnavailable => "auth:imported-token-unavailable",
            Self::Local => "local",
            Self::Legacy => "legacy",
        }
    }
}

/// Compact Models.dev freshness chip for the provider picker chrome (#4139).
fn catalog_freshness_title_suffix() -> &'static str {
    catalog_freshness_title_suffix_for(models_dev_live::status().freshness)
}

fn catalog_freshness_title_suffix_for(freshness: ModelsDevFreshness) -> &'static str {
    match freshness {
        ModelsDevFreshness::Stale => " · stale",
        // A failed optional refresh keeps prior or bundled rows available.
        // Say what the picker is using instead of implying the catalog broke.
        ModelsDevFreshness::Failed => " · refresh failed; catalog available",
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
        // Model Studio surfaces reasoning as structured Thinking on both
        // dialects: `delta.reasoning_content` on the OpenAI-compatible
        // routes, thinking blocks on the Anthropic-compatible routes.
        | ApiProvider::ModelstudioTokenPlan
        | ApiProvider::ModelstudioTokenPlanAnthropic
        | ApiProvider::ModelstudioCodingPlan
        | ApiProvider::ModelstudioCodingPlanAnthropic
        | ApiProvider::Moonshot => ProviderReasoningStreamVisibility::StructuredThinking,
        _ => ProviderReasoningStreamVisibility::Unknown,
    }
}

#[allow(clippy::too_many_arguments)]
fn auth_status_for(
    provider: ApiProvider,
    base_url: &str,
    has_key: bool,
    configured: Option<&crate::config::ProviderConfig>,
    no_auth: bool,
    api_key_required: bool,
    official_endpoint: bool,
    xai_oauth_ready: bool,
) -> ProviderAuthStatus {
    if no_auth {
        return ProviderAuthStatus::NoAuth;
    }
    if crate::config::provider_route_is_keyless_self_hosted(provider, base_url) {
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
        && configured.is_some_and(crate::config::provider_config_uses_kimi_imported_token)
    {
        return ProviderAuthStatus::ImportedTokenUnavailable;
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
        && let Some(status) = xai_oauth_status(configured, xai_oauth_ready)
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
    oauth_credentials_present: bool,
) -> Option<ProviderAuthStatus> {
    let oauth_selected = configured
        .and_then(|entry| entry.auth_mode.as_deref())
        .is_some_and(crate::xai_oauth::auth_mode_uses_xai_oauth);
    if !oauth_selected {
        return None;
    }
    Some(if oauth_credentials_present {
        ProviderAuthStatus::OAuthReady
    } else if has_explicit_credential(ApiProvider::Xai, configured) {
        ProviderAuthStatus::Configured
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
            entry.api_key.as_deref().is_some_and(|value| {
                crate::config::classify_config_api_key_value(value)
                    == crate::config::ConfigApiKeyValueKind::Literal
            })
        })
}

fn custom_provider_has_auth(configured: Option<&crate::config::ProviderConfig>) -> bool {
    if custom_provider_auth_is_optional(configured) {
        return true;
    }
    configured.is_some_and(|entry| {
        entry.api_key.as_deref().is_some_and(|value| {
            crate::config::classify_config_api_key_value(value)
                == crate::config::ConfigApiKeyValueKind::Literal
        }) || entry
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

/// The actionable half of a failed credential resolution.
///
/// "missing DEEPSEEK_API_KEY" told a user nothing about *where* the picker
/// looked, which is why a key sitting in the secret store could read as
/// "missing key" while the request path found it. Every message now carries
/// the ordered places that were probed and the one command that fixes the
/// first of them. Places and fixes are labels only — never secret material.
fn missing_auth_message(
    provider: ApiProvider,
    configured: Option<&crate::config::ProviderConfig>,
    provider_id: &str,
    resolution: &crate::credentials::CredentialResolution,
) -> String {
    if provider == ApiProvider::Moonshot
        && configured.is_some_and(crate::config::provider_config_uses_kimi_imported_token)
    {
        return "Kimi OAuth is unavailable; configure a Kimi API key".to_string();
    }
    let headline = if provider == ApiProvider::Custom {
        match configured
            .and_then(|entry| entry.api_key_env.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            Some(env_name) => format!("missing {env_name} for custom provider {provider_id}"),
            None => format!("missing custom provider auth for {provider_id}"),
        }
    } else {
        format!("missing {}", provider.env_vars_label())
    };
    let mut message = headline;
    let checked = resolution.checked_places();
    if !checked.is_empty() {
        message.push_str(" · checked ");
        message.push_str(&checked);
    }
    if let Some(fix) = resolution.first_fix() {
        message.push_str(" · fix: ");
        message.push_str(fix);
    }
    message
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
        ApiProvider::XiaomiMimo => "cost: token-plan".to_string(),
        // OpenCode ships two billing tracks off one account; the rows must not
        // both read as generic metering (#4526).
        ApiProvider::OpencodeGo => "usage: OpenCode Go subscription".to_string(),
        ApiProvider::OpencodeZen => "cost: OpenCode Zen pay-as-you-go".to_string(),
        _ => "cost: unknown".to_string(),
    }
}

fn pricing_label(provider: ApiProvider, pricing: Option<&PricingSku>) -> String {
    // OpenCode Go spends a subscription allowance, not per-token dollars, so a
    // catalog token price would misreport it as metered spend.
    if provider == ApiProvider::OpencodeGo {
        return usage_meter_for(provider);
    }
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

/// Resolve the external credential target for a provider that supports
/// read-only external consent. This is the same lower-level fact the
/// provider picker uses to build its consent flow; Fleet setup reuses it
/// for route-scoped activation without switching the parent session.
#[must_use]
pub(crate) fn external_consent_target_for_provider(
    provider: ApiProvider,
) -> Option<(
    codewhale_config::ProviderKind,
    codewhale_config::ExternalCredentialSource,
    std::path::PathBuf,
)> {
    let (consent_provider, source, path) = match provider {
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
        _ => return None,
    };
    let path = codewhale_config::resolve_external_credential_path(path).ok()?;
    Some((consent_provider, source, path))
}

/// #5243: grant-time validation — does the external file that the user wants
/// to read actually exist and hold a fresh, usable token? The old picker only
/// lexically normalized the path (`resolve_external_credential_path`) and
/// deferred the check to the first request, which produced
/// `auth:oauth-consented-select-to-check` and required a second `e` trip after
/// a just-minted OAuth. Validating here fails fast and, when the check passes,
/// the token is adopted automatically as part of the same grant.
pub(crate) fn external_consent_target_is_grantable(provider: ApiProvider) -> bool {
    let Some((_, _, path)) = external_consent_target_for_provider(provider) else {
        return false;
    };
    match provider {
        ApiProvider::Xai => crate::xai_oauth::external_file_is_fresh(&path),
        ApiProvider::OpenaiCodex => codex_external_file_is_fresh(&path),
        _ => false,
    }
}

fn codex_external_file_is_fresh(path: &std::path::Path) -> bool {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let value: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let token = value
        .get("tokens")
        .and_then(|t| t.get("access_token"))
        .and_then(|v| v.as_str())
        .filter(|t| !t.trim().is_empty());
    let Some(token) = token else {
        return false;
    };
    // Reuse the same 60s skew the runtime uses: token with valid JWT exp is fresh.
    if let Some(exp) = codex_jwt_expiry(token) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        return now + 60 < exp;
    }
    // If we cannot parse expiry, fail closed — external Codex credentials are
    // never refreshed by Codewhale, so an opaque token must be treated as stale.
    false
}

fn codex_jwt_expiry(token: &str) -> Option<u64> {
    use base64::Engine as _;
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: Value = serde_json::from_slice(&decoded).ok()?;
    claims.get("exp")?.as_u64()
}

impl ProviderPickerView {
    /// OAuth-only providers never collect a typed credential: the key entry
    /// stage for them is a routing step (device flow / external consent), so
    /// typed input and pastes are accepted events but never stored.
    fn key_entry_is_oauth_locked(&self) -> bool {
        self.selected_provider().credential_help().acquisition == CredentialAcquisition::OAuth
    }
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
        // Build the setup/catalog universe directly from ApiProvider::all so
        // first-run and recovery use the same canonical provider surface as
        // the runtime, not a historical onboarding shortlist. The active
        // provider is highlighted via `selected_idx` below, so it is never
        // lost in the list.
        let runtime_status = runtime_status.as_ref();
        let custom_rows = custom_provider_dashboard_rows(active, config, runtime_status);
        // Catalog surface = ProviderKind::ALL (one identity per vendor). Dual
        // dialect / plan-variant kinds stay resolvable but are not separate
        // rows; plan is mode/base_url and dialect is providers.<id>.wire.
        let catalog_active = active.catalog_identity();
        let mut rows: Vec<ProviderDashboardRow> = ApiProvider::catalog()
            .iter()
            .copied()
            .filter(|provider| *provider != ApiProvider::Custom || custom_rows.is_empty())
            .map(|p| {
                ProviderDashboardRow::from_config_with_runtime_status(
                    p,
                    catalog_active,
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
            onboarding_mode: false,
            query: String::new(),
            api_key_input: String::new(),
            key_entry_error: None,
            locale: Locale::En,
            xai_auth_choice: XaiAuthChoice::ApiKey,
            external_consent_choice: ExternalConsentChoice::Disabled,
            pending_api_key: None,
            model_options: Vec::new(),
            model_selected_idx: 0,
            selected_model: None,
            selected_context_window: None,
            kimi_code_plan_tier: KimiCodePlanTier::Safe262k,
            stepfun_billing_route: StepfunBillingRoute::PayAsYouGo,
            pending_base_url: None,
            custom_provider_field: CustomProviderField::Name,
            custom_provider_id: String::new(),
            custom_provider_base_url: String::new(),
            custom_provider_model: String::new(),
            custom_provider_api_key_env: String::new(),
            template_selected_idx: 0,
            template_row_hitboxes: RefCell::new(Vec::new()),
            last_template_mouse_selected: None,
        };
        picker.restore_memory(memory);
        picker
    }

    #[must_use]
    pub(crate) fn with_locale(mut self, locale: Locale) -> Self {
        self.locale = locale;
        self
    }

    fn tr(&self, id: MessageId) -> Cow<'static, str> {
        tr(self.locale, id)
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
        Self::new_for_setup_inner(active, target, config, runtime_status, true)
    }

    /// Open the named OpenAI-compatible form with DS4's keyless local
    /// defaults filled in. DS4 uses the existing transport, not a new adapter.
    #[must_use]
    pub fn new_for_ds4_setup(
        active: ApiProvider,
        config: &Config,
        runtime_status: Option<ProviderRuntimeStatus>,
    ) -> Self {
        let mut picker = Self::new_with_runtime_status(active, config, runtime_status);
        picker.setup_mode = true;
        picker.enter_ds4_form();
        picker
    }

    /// Open the named OpenAI-compatible form with LM Studio's loopback
    /// defaults filled in. Same transport as any custom OpenAI-compatible
    /// row; the model field stays empty because it depends on what the user
    /// has loaded.
    #[must_use]
    pub fn new_for_lm_studio_setup(
        active: ApiProvider,
        config: &Config,
        runtime_status: Option<ProviderRuntimeStatus>,
    ) -> Self {
        let mut picker = Self::new_with_runtime_status(active, config, runtime_status);
        picker.setup_mode = true;
        picker.enter_lm_studio_form();
        picker
    }

    /// Open the beginner template list (`/provider templates`, Settings).
    #[must_use]
    pub fn new_for_template_list(
        active: ApiProvider,
        config: &Config,
        runtime_status: Option<ProviderRuntimeStatus>,
    ) -> Self {
        let mut picker = Self::new_with_runtime_status(active, config, runtime_status);
        picker.setup_mode = true;
        picker.enter_template_list();
        picker
    }

    /// Apply one catalog template: first-class key-only setup, compatible
    /// custom form, or unpublished guidance.
    #[must_use]
    pub fn new_for_template_setup(
        active: ApiProvider,
        template_id: &str,
        config: &Config,
        runtime_status: Option<ProviderRuntimeStatus>,
    ) -> Option<Self> {
        let template = provider_setup_template(template_id)?;
        let mut picker = Self::new_with_runtime_status(active, config, runtime_status);
        picker.setup_mode = true;
        picker.apply_template(template);
        Some(picker)
    }

    /// Open the setup catalog for first-run/recovery onboarding (#4763).
    /// Identical to [`Self::new_for_setup`] except that a missing-auth
    /// `target` is only *focused*: onboarding must show the navigable
    /// provider list before it asks for a secret, so key/OAuth entry is
    /// reached by picking a row, never by opening straight into it.
    #[must_use]
    pub fn new_for_onboarding(
        active: ApiProvider,
        target: Option<ApiProvider>,
        config: &Config,
        runtime_status: Option<ProviderRuntimeStatus>,
    ) -> Self {
        let mut picker = Self::new_for_setup_inner(active, target, config, runtime_status, false);
        picker.onboarding_mode = true;
        picker
    }

    fn new_for_setup_inner(
        active: ApiProvider,
        target: Option<ApiProvider>,
        config: &Config,
        runtime_status: Option<ProviderRuntimeStatus>,
        key_entry_for_missing_auth: bool,
    ) -> Self {
        let mut picker = Self::new_with_runtime_status(active, config, runtime_status);
        // First-run setup shows the full provider catalog (hosted + local)
        // so a user can pick DeepSeek or any other API immediately. Local-only
        // is opt-in via "explore offline" and the L toggle. A local-first
        // default (introduced 2026-08-15) hid hosted providers behind a
        // keypress, which read as "only local models are supported."
        let _ = key_entry_for_missing_auth;
        picker.view = ProviderListView::Catalog;
        picker.setup_mode = true;
        if let Some(target) = target
            && let Some(idx) = picker.rows.iter().position(|row| row.provider == target)
        {
            picker.selected_idx = idx;
            if key_entry_for_missing_auth && !picker.selected_has_key() {
                picker.begin_setup();
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
        picker.begin_setup();
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
            ProviderListView::Local => self.rows[idx].provider.is_self_hosted(),
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
            ProviderListView::Local => ProviderListView::Catalog,
        };
        if !self.rows.is_empty() && !self.row_visible(self.selected_idx) {
            self.selected_idx = (0..self.rows.len())
                .find(|idx| self.row_visible(*idx))
                .unwrap_or(0);
        }
    }

    /// Show only the built-in keyless/self-hosted routes. Kept separate from
    /// `Configured`: merely supporting a local route does not mean the user
    /// configured it, while first-run should still make those routes obvious.
    fn show_local_routes(&mut self) {
        self.view = ProviderListView::Local;
        self.query.clear();
        if !self.rows.is_empty() && !self.row_visible(self.selected_idx) {
            self.selected_idx = self
                .rows
                .iter()
                .position(|row| row.provider == ApiProvider::Ollama)
                .or_else(|| (0..self.rows.len()).find(|idx| self.row_visible(*idx)))
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
                | CredentialState::ExternalConsent
                | CredentialState::ImportedToken
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
        self.pending_base_url = None;
        self.model_options.clear();
        self.model_selected_idx = 0;
        self.selected_model = None;
    }

    /// Start guided setup for the selected row. Providers that bill on more
    /// than one endpoint choose the route first so the key is validated
    /// against the endpoint it will actually be saved for (#4526).
    fn begin_setup(&mut self) {
        if self.selected_provider() == ApiProvider::Xai {
            self.enter_xai_auth_choice();
        } else if self.stepfun_billing_route_applies() {
            self.enter_stepfun_billing_route();
        } else {
            self.enter_key_entry();
        }
    }

    fn enter_xai_auth_choice(&mut self) {
        self.xai_auth_choice = XaiAuthChoice::ApiKey;
        self.stage = Stage::XaiAuthChoice;
        self.api_key_input.clear();
        self.key_entry_error = None;
        self.pending_api_key = None;
    }

    fn move_xai_auth_choice(&mut self) {
        self.xai_auth_choice = match self.xai_auth_choice {
            XaiAuthChoice::ApiKey => XaiAuthChoice::DeviceOAuth,
            XaiAuthChoice::DeviceOAuth => XaiAuthChoice::ApiKey,
        };
    }

    fn stepfun_billing_route_applies(&self) -> bool {
        self.rows
            .get(self.selected_idx)
            .is_some_and(|row| stepfun_route_is_selectable(row.provider, &row.base_url))
    }

    fn enter_stepfun_billing_route(&mut self) {
        // Preselect whatever the row already resolves to so re-running setup
        // on a configured Step Plan route does not default back to PAYG.
        self.stepfun_billing_route = if crate::pricing::billing_surface_for_route(
            ApiProvider::Stepfun,
            Some(&self.rows[self.selected_idx].base_url),
        ) == Some(crate::pricing::STEPFUN_PLAN_BILLING_SURFACE)
        {
            StepfunBillingRoute::StepPlan
        } else {
            StepfunBillingRoute::PayAsYouGo
        };
        self.stage = Stage::StepfunBillingRoute;
    }

    fn apply_stepfun_billing_route(&mut self) {
        let base_url = self.stepfun_billing_route.base_url().to_string();
        self.enter_key_entry();
        self.rows[self.selected_idx].base_url.clone_from(&base_url);
        self.pending_base_url = Some(base_url);
    }

    fn selected_external_consent_target(
        &self,
    ) -> Option<(
        codewhale_config::ProviderKind,
        codewhale_config::ExternalCredentialSource,
        std::path::PathBuf,
    )> {
        external_consent_target_for_provider(self.selected_provider())
    }

    fn enter_external_consent_choice(&mut self) {
        if self.selected_external_consent_target().is_some() {
            self.external_consent_choice = ExternalConsentChoice::Disabled;
            self.stage = Stage::ExternalConsentChoice;
        }
    }

    fn move_external_consent_choice(&mut self, delta: isize) {
        let index = match self.external_consent_choice {
            ExternalConsentChoice::Disabled => 0,
            ExternalConsentChoice::ReadOnly => 1,
            ExternalConsentChoice::ManagedUnavailable => 2,
        };
        self.external_consent_choice = match (index as isize + delta).rem_euclid(3) {
            0 => ExternalConsentChoice::Disabled,
            1 => ExternalConsentChoice::ReadOnly,
            _ => ExternalConsentChoice::ManagedUnavailable,
        };
    }

    fn build_external_consent_event(&self) -> Option<ViewEvent> {
        let (provider, source, path) = self.selected_external_consent_target()?;
        Some(ViewEvent::ProviderPickerExternalConsentConfirmed {
            provider: self.selected_provider(),
            consent_provider: provider,
            source,
            path,
        })
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
        base_url: Option<String>,
    ) -> Option<Self> {
        let mut picker = Self::new_with_runtime_status(active, config, runtime_status);
        let idx = picker.rows.iter().position(|row| row.provider == target)?;
        picker.selected_idx = idx;
        picker.view = ProviderListView::Catalog;
        picker.pending_api_key = Some(api_key);
        // The wizard's endpoint choice survives the validation round-trip so
        // confirm persists exactly the route the key was verified against.
        if let Some(base_url) = base_url {
            picker.rows[idx].base_url.clone_from(&base_url);
            picker.pending_base_url = Some(base_url);
        }
        picker.api_key_input.clear();
        picker.key_entry_error = None;
        picker.enter_model_pick();
        Some(picker)
    }

    fn enter_model_pick(&mut self) {
        self.stage = Stage::ModelPick;
        self.selected_context_window = None;
        let provider = self.selected_provider();
        let route = &self.rows[self.selected_idx].default_route;
        let kimi_code_k3 = crate::config::is_exact_kimi_code_k3_route(
            provider,
            &self.rows[self.selected_idx].base_url,
            &route.wire_model,
        );
        // Recovery must restore the configured wire route, not replace bare
        // K3 with whichever generic Moonshot catalog entry happens to sort
        // first. Keep this route-local; `k3` is intentionally not added to
        // the global Moonshot catalog.
        let preferred = if kimi_code_k3 {
            route.wire_model.clone()
        } else {
            route.logical_model.clone()
        };
        let mut models = crate::provider_lake::all_catalog_models_for_provider(provider);
        if kimi_code_k3
            && !preferred.trim().is_empty()
            && !models
                .iter()
                .any(|model| model.eq_ignore_ascii_case(preferred.trim()))
        {
            models.push(preferred.clone());
        }
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

    fn selected_kimi_code_k3(&self) -> bool {
        let Some(model) = self.selected_model.as_deref() else {
            return false;
        };
        crate::config::is_exact_kimi_code_k3_route(
            self.selected_provider(),
            &self.rows[self.selected_idx].base_url,
            model,
        )
    }

    fn enter_plan_tier(&mut self) {
        self.stage = Stage::PlanTier;
        self.kimi_code_plan_tier = KimiCodePlanTier::Safe262k;
    }

    fn apply_plan_tier(&mut self) {
        self.selected_context_window = Some(match self.kimi_code_plan_tier {
            KimiCodePlanTier::Safe262k => crate::models::KIMI_CODE_K3_CONTEXT_WINDOW_TOKENS,
            KimiCodePlanTier::OneMillion => 1_048_576,
        });
        self.enter_confirm();
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
            context_window: self.selected_context_window,
            base_url: self.pending_base_url.clone(),
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

    /// Filled-in custom form. Not a list-stage hotkey — `d` is type-ahead.
    fn enter_ds4_form(&mut self) {
        self.stage = Stage::CustomForm;
        self.custom_provider_field = CustomProviderField::ApiKeyEnv;
        self.custom_provider_id = DS4_PROVIDER_ID.to_string();
        self.custom_provider_base_url = DS4_BASE_URL.to_string();
        self.custom_provider_model = DS4_DEFAULT_MODEL.to_string();
        self.custom_provider_api_key_env.clear();
    }

    /// Filled-in custom form. Not a list-stage hotkey — `i` is type-ahead.
    fn enter_lm_studio_form(&mut self) {
        self.stage = Stage::CustomForm;
        self.custom_provider_field = CustomProviderField::Model;
        self.custom_provider_id = LM_STUDIO_PROVIDER_ID.to_string();
        self.custom_provider_base_url = LM_STUDIO_BASE_URL.to_string();
        // LM Studio model identifiers depend on what the user has loaded, so
        // leave the model editable instead of guessing a stale default.
        self.custom_provider_model.clear();
        self.custom_provider_api_key_env.clear();
    }

    fn enter_template_list(&mut self) {
        self.stage = Stage::TemplateList;
        self.template_selected_idx = self
            .template_selected_idx
            .min(provider_setup_templates().len().saturating_sub(1));
        self.last_template_mouse_selected = None;
        self.template_row_hitboxes.borrow_mut().clear();
    }

    fn selected_template(&self) -> Option<&'static ProviderSetupTemplate> {
        provider_setup_templates().get(self.template_selected_idx)
    }

    fn move_template_selection(&mut self, delta: isize) {
        let total = provider_setup_templates().len();
        if total == 0 {
            return;
        }
        self.template_selected_idx =
            crate::tui::list_nav::wrap_index(self.template_selected_idx, total, delta);
        self.last_template_mouse_selected = None;
    }

    fn template_kind_label(&self, template: &ProviderSetupTemplate) -> Cow<'static, str> {
        self.tr(match template.apply {
            ProviderSetupApply::FirstClass(_) => MessageId::ProviderTemplateKindKeyOnly,
            ProviderSetupApply::Compatible => MessageId::ProviderTemplateKindCompatible,
            ProviderSetupApply::Unpublished => MessageId::ProviderTemplateKindUnpublished,
        })
    }

    fn template_guidance_text(&self, template: &ProviderSetupTemplate) -> Cow<'static, str> {
        match template.id {
            "opencode-zen" => self.tr(MessageId::ProviderTemplateGuidanceOpencodeZen),
            "opencode-go" => self.tr(MessageId::ProviderTemplateGuidanceOpencodeGo),
            id if id == SENSENOVA_TEMPLATE_ID => {
                self.tr(MessageId::ProviderTemplateGuidanceSenseNova)
            }
            id if id == AGNES_TEMPLATE_ID => self.tr(MessageId::ProviderTemplateGuidanceAgnes),
            _ => Cow::Borrowed(template.guidance()),
        }
    }

    fn activate_selected_template(&mut self) -> ViewAction {
        if let Some(template) = self.selected_template() {
            if template.is_unpublished() {
                ViewAction::Emit(ViewEvent::StatusMessage {
                    message: self.tr(MessageId::ProviderTemplateUnpublished).into_owned(),
                })
            } else {
                self.apply_template(template);
                ViewAction::None
            }
        } else {
            ViewAction::None
        }
    }

    fn handle_template_list_click(&mut self, mouse: MouseEvent) -> ViewAction {
        let clicked = self
            .template_row_hitboxes
            .borrow()
            .iter()
            .find_map(|(rect, idx)| {
                rect.contains(Position::new(mouse.column, mouse.row))
                    .then_some(*idx)
            });
        let Some(idx) = clicked else {
            return ViewAction::None;
        };
        let activate =
            self.last_template_mouse_selected == Some(idx) && self.template_selected_idx == idx;
        self.template_selected_idx = idx;
        self.last_template_mouse_selected = Some(idx);
        if activate {
            self.activate_selected_template()
        } else {
            ViewAction::None
        }
    }

    fn apply_template(&mut self, template: &'static ProviderSetupTemplate) {
        match template.apply {
            ProviderSetupApply::FirstClass(kind) => {
                let provider = ApiProvider::from_kind(kind);
                if !self.rows.iter().any(|row| row.provider == provider)
                    || (self
                        .rows
                        .iter()
                        .position(|row| row.provider == provider)
                        .is_some_and(|idx| !self.row_visible(idx)))
                {
                    self.view = ProviderListView::Catalog;
                }
                if let Some(idx) = self.rows.iter().position(|row| row.provider == provider) {
                    self.selected_idx = idx;
                    self.stage = Stage::List;
                    if !self.selected_has_key() {
                        self.begin_setup();
                    }
                }
            }
            ProviderSetupApply::Compatible => self.enter_compatible_form(template),
            ProviderSetupApply::Unpublished => {
                if let Some(idx) = provider_setup_templates()
                    .iter()
                    .position(|candidate| candidate.id == template.id)
                {
                    self.template_selected_idx = idx;
                }
                self.enter_template_list();
            }
        }
    }

    fn enter_compatible_form(&mut self, template: &'static ProviderSetupTemplate) {
        self.stage = Stage::CustomForm;
        self.custom_provider_field = CustomProviderField::ApiKeyEnv;
        self.custom_provider_id = template.id.to_string();
        self.custom_provider_base_url = template.base_url().unwrap_or("").to_string();
        self.custom_provider_model = template.default_model().unwrap_or("").to_string();
        self.custom_provider_api_key_env = template.api_key_env().unwrap_or("").to_string();
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

    fn render_list(&self, area: Rect, buf: &mut Buffer) {
        let enter_action = if !self.selected_route_is_valid() {
            self.tr(MessageId::PickerActionUnavailable)
        } else if self.selected_has_key() {
            self.tr(MessageId::PickerActionApply)
        } else {
            self.tr(MessageId::PickerActionSetKey)
        };
        let title = if self.onboarding_mode {
            format!(" {} ", self.tr(MessageId::OnboardProviderTitle))
        } else {
            match (self.setup_mode, self.view) {
                (true, ProviderListView::Configured) => {
                    format!(" Provider setup{} ", catalog_freshness_title_suffix())
                }
                (true, ProviderListView::Catalog) => {
                    format!(" Provider setup · all{} ", catalog_freshness_title_suffix())
                }
                (true, ProviderListView::Local) => " Local models · no cloud key ".to_string(),
                (false, ProviderListView::Configured) => {
                    format!(" Provider{} ", catalog_freshness_title_suffix())
                }
                (false, ProviderListView::Catalog) => {
                    format!(" Provider · all{} ", catalog_freshness_title_suffix())
                }
                (false, ProviderListView::Local) => " Provider · local only ".to_string(),
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

        let search_active = !self.query.trim().is_empty();
        // The action footer moves into the body so it wraps instead of clipping
        // at narrow widths (#3732); the provider list renders above it.
        let hints = self.list_stage_action_hints(enter_action);
        let content = render_modal_footer(inner, buf, &hints);

        let filtered = self.filtered_rows();
        if filtered.is_empty() {
            if search_active {
                EmptyState::new(
                    self.tr(MessageId::ProviderNoMatchesTitle),
                    self.tr(MessageId::ProviderNoMatchesHint),
                )
                .primary_action("Esc", self.tr(MessageId::PickerActionClearSearch))
                .render(content, buf);
            } else {
                EmptyState::new(
                    self.tr(MessageId::ProviderNoConfiguredTitle),
                    self.tr(MessageId::ProviderNoConfiguredHint),
                )
                .primary_action("A", self.tr(MessageId::PickerActionBrowseAll))
                .secondary_action("C", self.tr(MessageId::PickerActionCustom))
                .render(content, buf);
            }
            return;
        }

        // Onboarding asks one question. The ordinary provider manager keeps
        // its technical detail pane, but first-run gives the available rows
        // the whole body so 40x12 still has room to choose and proceed.
        let layout = if self.onboarding_mode {
            ListDetailLayout {
                list: content,
                detail: Rect::new(content.x, content.y, 0, 0),
                stacked: false,
            }
        } else {
            ListDetailLayout::split(content, 34)
        };
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
            let arrow = crate::tui::glyphs::selection_marker(is_selected);
            let active_dot = if is_active { " *" } else { "  " };
            let spacer_style = if is_selected {
                menu_style::selected_row_bg_style()
            } else {
                Style::default()
            };
            let label_style = if is_selected {
                menu_style::selected_row_style_with_fg(palette::SELECTION_TEXT)
            } else {
                Style::default().fg(palette::TEXT_PRIMARY)
            };
            let has_usable_auth = matches!(
                row.credential_state,
                CredentialState::Saved
                    | CredentialState::ImportedToken
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
                menu_style::selected_row_style_with_fg(hint_fg)
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
                line.style = menu_style::selected_row_bg_style();
                let target_width = usize::from(layout.list.width);
                let line_width = line.width();
                if line_width < target_width {
                    line.spans.push(Span::styled(
                        " ".repeat(target_width - line_width),
                        menu_style::selected_row_bg_style(),
                    ));
                }
            }
            lines.push(line);
        }
        Paragraph::new(lines).render(layout.list, buf);
        if !self.onboarding_mode {
            self.render_provider_detail(layout.detail, buf, &self.rows[self.selected_idx]);
        }
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
            // Which place the credential actually came from. A row can read
            // "key:configured" for four different reasons; naming the one that
            // won is what lets a user reconcile the picker with a request that
            // succeeded (or didn't).
            Line::from(Span::styled(
                format!("Credential: {}", row.credential_source),
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
        if let Some(status) = row.external_credential_status.as_ref() {
            let state = if status.route_state == "active" {
                self.tr(MessageId::CtxInspActive)
            } else {
                self.tr(MessageId::ProviderExternalDormant)
            };
            let scope = self
                .tr(MessageId::ProviderExternalDetailScope)
                .replace("{access}", status.access.as_str())
                .replace("{provider}", &status.provider)
                .replace("{source}", status.source.as_str())
                .replace("{version}", &status.consent_version.to_string())
                .replace("{state}", &state);
            lines.push(Line::from(Span::styled(
                scope,
                Style::default().fg(palette::TEXT_MUTED),
            )));
            let owner_path = self
                .tr(MessageId::ProviderExternalOwnerPath)
                .replace("{owner}", status.owner)
                .replace("{path}", &codewhale_config::quote_os_path(&status.path));
            let mut owner_path_spans = vec![Span::styled(
                owner_path,
                Style::default().fg(palette::TEXT_MUTED),
            )];
            if status.ambient_path_changed {
                let warning = self
                    .tr(MessageId::ProviderExternalPinnedPathWarning)
                    .replace("{owner}", status.owner)
                    .replace("{path}", &codewhale_config::quote_os_path(&status.path));
                owner_path_spans.push(Span::styled(
                    " | ",
                    Style::default().fg(palette::TEXT_MUTED),
                ));
                owner_path_spans.push(Span::styled(
                    warning,
                    Style::default().fg(palette::STATUS_WARNING),
                ));
            }
            lines.push(Line::from(owner_path_spans));
            let semantics = match status.access {
                codewhale_config::ExternalCredentialAccess::Disabled => {
                    self.tr(MessageId::ProviderExternalDisabledDetail)
                }
                codewhale_config::ExternalCredentialAccess::ReadOnly => {
                    self.tr(MessageId::ProviderExternalReadOnlySemantics)
                }
                codewhale_config::ExternalCredentialAccess::Managed => {
                    self.tr(MessageId::ProviderExternalManagedDetail)
                }
            };
            lines.push(Line::from(Span::styled(
                semantics,
                Style::default().fg(palette::TEXT_MUTED),
            )));
            let revoke = self
                .tr(MessageId::ProviderExternalRevoke)
                .replace("{revoke}", &status.revoke_command);
            lines.push(Line::from(Span::styled(
                revoke,
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .render(inner, buf);
    }

    fn render_xai_auth_choice(&self, area: Rect, buf: &mut Buffer) {
        let outer = Block::default()
            .title(Line::from(Span::styled(
                self.tr(MessageId::XaiAuthChoiceTitle),
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
                ActionHint::new("↑↓/1-2", self.tr(MessageId::ProviderExternalActionChoose)),
                ActionHint::new("Enter", self.tr(MessageId::SetupActionContinue)),
                ActionHint::new("E", self.tr(MessageId::ProviderExternalActionReuseGrok)),
                ActionHint::new("Esc", self.tr(MessageId::SetupActionBack)),
            ],
        );
        let marker = |choice| crate::tui::glyphs::selection_marker(self.xai_auth_choice == choice);
        Paragraph::new(vec![
            Line::from(self.tr(MessageId::XaiAuthChoiceIntro)),
            Line::from(""),
            Line::from(format!(
                "{} 1. {}",
                marker(XaiAuthChoice::ApiKey),
                self.tr(MessageId::XaiAuthChoiceApiKeyOption),
            )),
            Line::from(format!(
                "{} 2. {}",
                marker(XaiAuthChoice::DeviceOAuth),
                self.tr(MessageId::XaiAuthChoiceDeviceOAuthOption),
            )),
        ])
        .wrap(Wrap { trim: false })
        .render(content, buf);
    }

    fn render_key_entry(&self, area: Rect, buf: &mut Buffer) {
        let row = &self.rows[self.selected_idx];
        let codex_oauth = row.provider == ApiProvider::OpenaiCodex;
        let oauth_provider = codex_oauth;
        let saved_credential = !oauth_provider && row.has_key;
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
            render_modal_footer(
                inner,
                buf,
                &[
                    ActionHint::new("Enter", self.tr(MessageId::ProviderExternalActionChoices)),
                    ActionHint::new("Esc", self.tr(MessageId::SetupActionBack)),
                ],
            )
        } else if saved_credential && self.api_key_input.trim().is_empty() {
            render_modal_footer(
                inner,
                buf,
                &[
                    ActionHint::new("Type/paste", "replace saved key"),
                    ActionHint::new("Esc", "keep current key"),
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
            "(run codex login; then explicitly grant read-only access)".to_string()
        } else if masked.is_empty() && saved_credential {
            "Saved credential configured".to_string()
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
            vec![
                Line::from(Span::styled(
                    self.tr(MessageId::ProviderExternalHintCodexReview)
                        .replace("{login}", "codex login"),
                    Style::default().fg(palette::TEXT_MUTED),
                )),
                Line::from(Span::styled(
                    format!(
                        "Or set {} / CODEX_ACCESS_TOKEN and re-open {reopen_command}.",
                        self.env_var_for_selected_row(),
                    ),
                    Style::default().fg(palette::TEXT_MUTED),
                )),
                Line::from(Span::styled(
                    "CLI: codewhale auth external-consent --provider openai-codex; no token is stored here.",
                    Style::default().fg(palette::TEXT_MUTED),
                )),
            ]
        } else if saved_credential && self.api_key_input.trim().is_empty() {
            vec![Line::from(Span::styled(
                "This terminal can use the stored credential. Type or paste only to replace it; Esc keeps it unchanged.",
                Style::default().fg(palette::TEXT_MUTED),
            ))]
        } else if saved_credential {
            vec![Line::from(Span::styled(
                "The replacement is validated before it replaces the stored credential.",
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
        if !oauth_provider {
            if row.provider == ApiProvider::Moonshot
                && crate::config::moonshot_base_url_is_exact_kimi_code(&row.base_url)
            {
                hint_lines.extend([
                    Line::from(Span::styled(
                        self.tr(MessageId::KimiCodePlanApiKeyHint).replace(
                            "{console}",
                            crate::config::KIMI_CODE_MEMBERSHIP_PLAN_CONSOLE_URL,
                        ),
                        Style::default().fg(palette::TEXT_MUTED),
                    )),
                    Line::from(Span::styled(
                        self.tr(MessageId::KimiCodePlanRouteHint)
                            .replace("{route}", crate::config::DEFAULT_KIMI_CODE_BASE_URL),
                        Style::default().fg(palette::TEXT_MUTED),
                    )),
                    Line::from(Span::styled(
                        self.tr(MessageId::KimiCodePlanNoImportHint),
                        Style::default().fg(palette::TEXT_MUTED),
                    )),
                ]);
            } else {
                let help = row.provider.credential_help();
                hint_lines.push(Line::from(Span::styled(
                    help.credential_url.map_or_else(
                        || format!("Credentials: {}", help.guidance),
                        |url| format!("Credentials: {url}"),
                    ),
                    Style::default().fg(palette::TEXT_MUTED),
                )));
                if let Some(url) = help.docs_url {
                    hint_lines.push(Line::from(Span::styled(
                        self.tr(MessageId::ProviderTemplateDocs)
                            .replace("{url}", url),
                        Style::default().fg(palette::TEXT_MUTED),
                    )));
                }
            }
        };

        if let Some(ref error) = self.key_entry_error {
            hint_lines.push(Line::from(Span::styled(
                format!("Verification failed: {error}"),
                Style::default().fg(palette::STATUS_ERROR),
            )));
        }

        // `Line` count is not rendered row count: long environment-variable
        // guidance can wrap to two or three terminal rows. Ask ratatui for the
        // exact wrapped height instead of duplicating its layout arithmetic.
        let hint = Paragraph::new(hint_lines).wrap(Wrap { trim: true });
        let hint_height = u16::try_from(hint.line_count(content.width.max(1)))
            .unwrap_or(u16::MAX)
            .clamp(1, 6);
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(hint_height),
                Constraint::Min(1),
            ])
            .split(content);

        Paragraph::new(key_lines).render(layout[0], buf);
        hint.render(layout[1], buf);
    }

    fn render_external_consent_choice(&self, area: Rect, buf: &mut Buffer) {
        let provider_name = self.rows[self.selected_idx].display_name.clone();
        let outer = Block::default()
            .title(Line::from(Span::styled(
                self.tr(MessageId::ProviderExternalChoiceTitle)
                    .replace("{provider}", &provider_name),
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
                ActionHint::new("↑↓", self.tr(MessageId::ProviderExternalActionChoose)),
                ActionHint::new("Enter", self.tr(MessageId::SetupActionContinue)),
                ActionHint::new("Esc", self.tr(MessageId::SetupActionBack)),
            ],
        );
        let selected = self.external_consent_choice;
        let row = |choice, label: Cow<'static, str>, detail: Cow<'static, str>| {
            let marker = crate::tui::glyphs::selection_marker(selected == choice);
            Line::from(vec![
                Span::styled(
                    format!("{marker} {label}"),
                    Style::default().fg(if selected == choice {
                        palette::WHALE_INFO
                    } else {
                        palette::TEXT_PRIMARY
                    }),
                ),
                Span::styled(
                    format!(" · {detail}"),
                    Style::default().fg(palette::TEXT_MUTED),
                ),
            ])
        };
        Paragraph::new(vec![
            Line::from(self.tr(MessageId::ProviderExternalChoiceIntro)),
            Line::from(""),
            row(
                ExternalConsentChoice::Disabled,
                self.tr(MessageId::ProviderExternalDisabledLabel),
                self.tr(MessageId::ProviderExternalDisabledDetail),
            ),
            row(
                ExternalConsentChoice::ReadOnly,
                self.tr(MessageId::ProviderExternalReadOnlyLabel),
                self.tr(MessageId::ProviderExternalReadOnlyDetail),
            ),
            row(
                ExternalConsentChoice::ManagedUnavailable,
                self.tr(MessageId::ProviderExternalManagedLabel),
                self.tr(MessageId::ProviderExternalManagedDetail),
            ),
        ])
        .wrap(Wrap { trim: false })
        .render(content, buf);
    }

    fn render_external_consent_confirm(&self, area: Rect, buf: &mut Buffer) {
        let Some((provider, source, path)) = self.selected_external_consent_target() else {
            return;
        };
        let outer = Block::default()
            .title(Line::from(Span::styled(
                self.tr(MessageId::ProviderExternalConfirmTitle),
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
                ActionHint::new("Enter", self.tr(MessageId::ProviderExternalActionGrant)),
                ActionHint::new("Esc", self.tr(MessageId::SetupActionCancel)),
            ],
        );
        let provider_label = self.tr(MessageId::RouteProviderLabel);
        let owner_label = self.tr(MessageId::ProviderExternalOwnerLabel);
        let exact_path_label = self.tr(MessageId::ProviderExternalExactPathLabel);
        let semantics_label = self.tr(MessageId::ProviderExternalSemanticsLabel);
        let revoke_label = self.tr(MessageId::ProviderExternalRevokeLabel);
        Paragraph::new(vec![
            Line::from(format!("{provider_label}: {}", provider.as_str())),
            Line::from(format!(
                "{owner_label}: {} ({})",
                source.owner_label(),
                source.as_str()
            )),
            Line::from(format!(
                "{exact_path_label}: {}",
                codewhale_config::quote_os_path(&path)
            )),
            Line::from(""),
            Line::from(format!(
                "{semantics_label}: {}.",
                self.tr(MessageId::ProviderExternalReadOnlySemantics)
            )),
            Line::from(self.tr(MessageId::ProviderExternalRejectUnsafe)),
            Line::from(format!(
                "{revoke_label}: codewhale auth external-revoke --provider {}",
                provider.as_str()
            )),
        ])
        .wrap(Wrap { trim: false })
        .render(content, buf);
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
            self.tr(MessageId::ProviderConnectionCheckedPickModel),
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
            let arrow = crate::tui::glyphs::selection_marker(is_selected);
            let label_style = if is_selected {
                menu_style::selected_row_style_with_fg(palette::SELECTION_TEXT)
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
                            menu_style::selected_row_style_with_fg(palette::TEXT_MUTED)
                        } else {
                            Style::default().fg(palette::TEXT_MUTED)
                        },
                    )
                },
            ]);
            if is_selected {
                line.style = menu_style::selected_row_bg_style();
            }
            lines.push(line);
        }
        if lines.is_empty() {
            lines.push(Line::from(Span::styled(
                self.tr(MessageId::ProviderNoCatalogModels),
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }
        Paragraph::new(lines).render(list_area, buf);
    }

    fn render_plan_tier(&self, area: Rect, buf: &mut Buffer) {
        let outer = Block::default()
            .title(Line::from(Span::styled(
                " Kimi Code plan tier ",
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
                ActionHint::new("↑↓", "choose"),
                ActionHint::new("Enter", "continue"),
                ActionHint::new("Esc", "back"),
            ],
        );
        let selected = self.kimi_code_plan_tier;
        let marker = |tier| crate::tui::glyphs::selection_marker(selected == tier);
        Paragraph::new(vec![
            Line::from("Kimi Code plan limits determine the context window used for k3."),
            Line::from("Choose the tier you actually have; the safe floor is selected by default."),
            Line::from(""),
            Line::from(format!(
                "{} 1. 262K context (safe default)",
                marker(KimiCodePlanTier::Safe262k)
            )),
            Line::from(format!(
                "{} 2. 1M context (only with an eligible plan)",
                marker(KimiCodePlanTier::OneMillion)
            )),
        ])
        .wrap(Wrap { trim: false })
        .render(content, buf);
    }

    fn render_stepfun_billing_route(&self, area: Rect, buf: &mut Buffer) {
        let outer = Block::default()
            .title(Line::from(Span::styled(
                format!(" {} ", self.tr(MessageId::StepfunBillingRouteTitle)),
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
                ActionHint::new("↑↓", "choose"),
                ActionHint::new("Enter", "continue"),
                ActionHint::new("Esc", "back"),
            ],
        );
        let selected = self.stepfun_billing_route;
        let marker = |route| crate::tui::glyphs::selection_marker(selected == route);
        // The endpoint is shown next to each choice: it is the whole
        // difference between the two billing tracks, and it is what gets
        // written to `[providers.stepfun] base_url` on confirm.
        Paragraph::new(vec![
            Line::from(self.tr(MessageId::StepfunBillingRouteIntro).to_string()),
            Line::from(""),
            Line::from(format!(
                "{} 1. {} — {}",
                marker(StepfunBillingRoute::PayAsYouGo),
                self.tr(MessageId::StepfunBillingRoutePaygOption),
                StepfunBillingRoute::PayAsYouGo.base_url(),
            )),
            Line::from(format!(
                "{} 2. {} — {}",
                marker(StepfunBillingRoute::StepPlan),
                self.tr(MessageId::StepfunBillingRoutePlanOption),
                StepfunBillingRoute::StepPlan.base_url(),
            )),
        ])
        .wrap(Wrap { trim: false })
        .render(content, buf);
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
            if let Some(context_window) = self.selected_context_window {
                Line::from(format!("Context:  {} tokens", context_window))
            } else {
                Line::from("")
            },
        ];
        Paragraph::new(lines).render(content, buf);
    }

    fn render_custom_form(&self, area: Rect, buf: &mut Buffer) {
        let title = provider_setup_template(&self.custom_provider_id)
            .filter(|template| template.is_compatible())
            .map(|template| format!(" {} ", template.display_name))
            .unwrap_or_else(|| " Custom provider ".to_string());
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

        let hint = provider_setup_template(&self.custom_provider_id)
            .filter(|template| template.is_compatible())
            .map(|template| {
                let mut parts = vec![self.template_guidance_text(template).into_owned()];
                if let Some(url) = template.docs_url() {
                    parts.push(
                        self.tr(MessageId::ProviderTemplateDocs)
                            .replace("{url}", url),
                    );
                }
                parts.join(" ")
            })
            .unwrap_or_else(|| self.tr(MessageId::ProviderCustomFormHint).into_owned());
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(palette::TEXT_MUTED),
        )))
        .wrap(Wrap { trim: true })
        .render(layout[0], buf);

        self.render_custom_form_field(layout[1], buf, CustomProviderField::Name, "Name", "acme_ai");
        self.render_custom_form_field(
            layout[2],
            buf,
            CustomProviderField::BaseUrl,
            &self.tr(MessageId::ProviderCustomFormBaseUrl),
            "https://api.example.com/v1",
        );
        self.render_custom_form_field(
            layout[3],
            buf,
            CustomProviderField::Model,
            &self.tr(MessageId::ProviderCustomFormModel),
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

    fn render_template_list(&self, area: Rect, buf: &mut Buffer) {
        self.template_row_hitboxes.borrow_mut().clear();
        let outer = Block::default()
            .title(Line::from(Span::styled(
                format!(" {} ", self.tr(MessageId::ProviderTemplatesTitle)),
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
                ActionHint::new("↑↓", self.tr(MessageId::PickerActionMove)),
                ActionHint::new("Enter", self.tr(MessageId::PickerActionApply)),
                ActionHint::new("Esc", self.tr(MessageId::PickerActionCancel)),
            ],
        );
        let templates = provider_setup_templates();
        let intro_height = if content.height >= 12 { 2 } else { 0 };
        let remaining = content.height.saturating_sub(intro_height);
        let detail_reserve = if remaining >= 6 {
            3
        } else if remaining >= 4 {
            2
        } else if remaining >= 3 {
            1
        } else {
            0
        };
        let list_budget = remaining.saturating_sub(detail_reserve).max(1);
        let visible_count = templates
            .len()
            .min(usize::from(list_budget))
            .max(usize::from(remaining > 0));
        let list_height = u16::try_from(visible_count).unwrap_or(u16::MAX).max(1);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(intro_height),
                Constraint::Length(list_height),
                Constraint::Min(detail_reserve),
            ])
            .split(content);
        if intro_height > 0 {
            Paragraph::new(Line::from(Span::styled(
                self.tr(MessageId::ProviderTemplatesIntro),
                Style::default().fg(palette::TEXT_MUTED),
            )))
            .wrap(Wrap { trim: true })
            .render(chunks[0], buf);
        }
        let selected = self
            .template_selected_idx
            .min(templates.len().saturating_sub(1));
        let max_start = templates.len().saturating_sub(visible_count);
        let start = selected
            .saturating_sub(visible_count.saturating_sub(1))
            .min(max_start);
        let list_area = chunks[1];
        for (offset, (idx, template)) in templates
            .iter()
            .enumerate()
            .skip(start)
            .take(visible_count)
            .enumerate()
        {
            let row_y = list_area.y.saturating_add(offset as u16);
            if row_y >= list_area.bottom() {
                break;
            }
            let row = Rect::new(list_area.x, row_y, list_area.width, 1);
            self.template_row_hitboxes.borrow_mut().push((row, idx));
            let selected_row = idx == self.template_selected_idx;
            let marker = crate::tui::glyphs::selection_marker(selected_row);
            let kind = self.template_kind_label(template);
            let style = if selected_row {
                menu_style::selected_row_style_with_fg(palette::SELECTION_TEXT)
            } else {
                Style::default().fg(palette::TEXT_PRIMARY)
            };
            let label = format!(
                "{marker} {} ({}) · {kind}",
                template.display_name, template.id
            );
            Paragraph::new(Line::from(Span::styled(
                crate::tui::ui_text::truncate_line_to_width(&label, usize::from(row.width)),
                style,
            )))
            .render(row, buf);
        }

        let mut detail = Vec::new();
        if let Some(template) = self.selected_template() {
            if template.is_unpublished() {
                detail.push(Line::from(Span::styled(
                    self.tr(MessageId::ProviderTemplateUnpublished),
                    Style::default().fg(palette::TEXT_MUTED),
                )));
            } else if let Some(url) = template.base_url() {
                detail.push(Line::from(Span::styled(
                    self.tr(MessageId::ProviderTemplateBaseUrl)
                        .replace("{url}", url),
                    Style::default().fg(palette::TEXT_MUTED),
                )));
            }
            if let Some(env) = template.api_key_env() {
                detail.push(Line::from(Span::styled(
                    env.to_string(),
                    Style::default().fg(palette::TEXT_MUTED),
                )));
            }
            if let Some(model) = template.default_model() {
                detail.push(Line::from(Span::styled(
                    self.tr(MessageId::ProviderTemplateModel)
                        .replace("{model}", model),
                    Style::default().fg(palette::TEXT_MUTED),
                )));
            }
            if let Some(url) = template.docs_url() {
                detail.push(Line::from(Span::styled(
                    self.tr(MessageId::ProviderTemplateDocs)
                        .replace("{url}", url),
                    Style::default().fg(palette::TEXT_MUTED),
                )));
            }
            if let Some(url) = template.credential_url() {
                detail.push(Line::from(Span::styled(
                    self.tr(MessageId::ProviderTemplateCredentials)
                        .replace("{url}", url),
                    Style::default().fg(palette::TEXT_MUTED),
                )));
            }
            detail.push(Line::from(Span::styled(
                self.template_guidance_text(template),
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }
        Paragraph::new(detail)
            .wrap(Wrap { trim: true })
            .render(chunks[2], buf);
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
        let marker = crate::tui::glyphs::selection_marker(selected);
        let value = self.custom_form_field_value(field);
        let display = if value.is_empty() { placeholder } else { value };
        let value_style = if selected {
            menu_style::selected_row_style_with_fg(palette::SELECTION_TEXT)
        } else if value.is_empty() {
            Style::default().fg(palette::TEXT_MUTED)
        } else {
            Style::default().fg(palette::TEXT_PRIMARY)
        };
        let label_style = if selected {
            menu_style::selected_row_style_with_fg(palette::WHALE_INFO)
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
            line.style = menu_style::selected_row_bg_style();
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
                if self.key_entry_is_oauth_locked() {
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
            Stage::List
            | Stage::XaiAuthChoice
            | Stage::ExternalConsentChoice
            | Stage::ExternalConsentConfirm
            | Stage::ModelPick
            | Stage::PlanTier
            | Stage::StepfunBillingRoute
            | Stage::Confirm
            | Stage::TemplateList => false,
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
                    if provider == ApiProvider::Custom
                        && !self.rows[self.selected_idx].is_configured
                    {
                        self.enter_custom_form();
                        ViewAction::None
                    } else if !self.selected_route_is_valid() {
                        ViewAction::None
                    } else if self.selected_has_key() {
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied {
                            provider,
                            provider_id,
                        })
                    } else if external_consent_target_is_grantable(provider) {
                        // #5243: token already stored externally and the user
                        // pressed Enter on the provider (says they want it
                        // read) — adopt it automatically in the same chord,
                        // no second `e` trip. Validated at grant time.
                        if let Some(event) = self.build_external_consent_event() {
                            ViewAction::EmitAndClose(event)
                        } else {
                            self.begin_setup();
                            ViewAction::None
                        }
                    } else {
                        self.begin_setup();
                        ViewAction::None
                    }
                }
                KeyCode::Backspace if !self.query.is_empty() => {
                    let mut query = self.query.clone();
                    query.pop();
                    self.update_query(query);
                    ViewAction::None
                }
                KeyCode::Char(c) => self.handle_list_char(c, key.modifiers),
                _ => ViewAction::None,
            },
            Stage::XaiAuthChoice => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::List;
                    ViewAction::None
                }
                KeyCode::Up | KeyCode::Down => {
                    self.move_xai_auth_choice();
                    ViewAction::None
                }
                KeyCode::Char('1') => {
                    self.xai_auth_choice = XaiAuthChoice::ApiKey;
                    ViewAction::None
                }
                KeyCode::Char('2') => {
                    self.xai_auth_choice = XaiAuthChoice::DeviceOAuth;
                    ViewAction::None
                }
                KeyCode::Char(c) if key.modifiers.is_empty() && c.eq_ignore_ascii_case(&'e') => {
                    self.enter_external_consent_choice();
                    ViewAction::None
                }
                KeyCode::Enter => match self.xai_auth_choice {
                    XaiAuthChoice::ApiKey => {
                        self.enter_key_entry();
                        ViewAction::None
                    }
                    XaiAuthChoice::DeviceOAuth => {
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerXaiOAuthRequested)
                    }
                },
                _ => ViewAction::None,
            },
            Stage::KeyEntry => match key.code {
                KeyCode::Esc => {
                    // Back to the route choice when one was made, so Esc undoes
                    // one wizard step instead of discarding the whole flow.
                    self.stage = if self.selected_provider() == ApiProvider::Xai {
                        Stage::XaiAuthChoice
                    } else if self.pending_base_url.is_some() {
                        Stage::StepfunBillingRoute
                    } else {
                        Stage::List
                    };
                    self.api_key_input.clear();
                    self.key_entry_error = None;
                    self.pending_api_key = None;
                    self.model_options.clear();
                    self.model_selected_idx = 0;
                    self.selected_model = None;
                    ViewAction::None
                }
                KeyCode::Backspace => {
                    if !self.key_entry_is_oauth_locked() {
                        self.api_key_input.pop();
                        self.key_entry_error = None;
                    }
                    ViewAction::None
                }
                KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if !self.key_entry_is_oauth_locked() {
                        self.api_key_input.pop();
                        self.key_entry_error = None;
                    }
                    ViewAction::None
                }
                KeyCode::Enter => {
                    if self.selected_provider() == ApiProvider::OpenaiCodex {
                        self.enter_external_consent_choice();
                        return ViewAction::None;
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
                            base_url: self.pending_base_url.clone(),
                        })
                    }
                }
                KeyCode::Char(c)
                    if !key.modifiers.intersects(
                        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                    ) =>
                {
                    if self.key_entry_is_oauth_locked() {
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
            Stage::ExternalConsentChoice => match key.code {
                KeyCode::Esc => {
                    self.stage = if self.selected_provider() == ApiProvider::Xai {
                        Stage::XaiAuthChoice
                    } else {
                        Stage::KeyEntry
                    };
                    ViewAction::None
                }
                KeyCode::Up => {
                    self.move_external_consent_choice(-1);
                    ViewAction::None
                }
                KeyCode::Down => {
                    self.move_external_consent_choice(1);
                    ViewAction::None
                }
                KeyCode::Char('1') => {
                    self.external_consent_choice = ExternalConsentChoice::Disabled;
                    ViewAction::None
                }
                KeyCode::Char('2') => {
                    self.external_consent_choice = ExternalConsentChoice::ReadOnly;
                    ViewAction::None
                }
                KeyCode::Char('3') => {
                    self.external_consent_choice = ExternalConsentChoice::ManagedUnavailable;
                    ViewAction::None
                }
                KeyCode::Enter => match self.external_consent_choice {
                    ExternalConsentChoice::Disabled => {
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerExternalConsentRevoked {
                            provider: self.selected_provider(),
                        })
                    }
                    ExternalConsentChoice::ReadOnly => {
                        self.stage = Stage::ExternalConsentConfirm;
                        ViewAction::None
                    }
                    ExternalConsentChoice::ManagedUnavailable => ViewAction::None,
                },
                _ => ViewAction::None,
            },
            Stage::ExternalConsentConfirm => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::ExternalConsentChoice;
                    ViewAction::None
                }
                KeyCode::Enter => self
                    .build_external_consent_event()
                    .map(ViewAction::EmitAndClose)
                    .unwrap_or(ViewAction::None),
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
                    if self.selected_kimi_code_k3() {
                        self.enter_plan_tier();
                    } else {
                        self.enter_confirm();
                    }
                    ViewAction::None
                }
                _ => ViewAction::None,
            },
            Stage::StepfunBillingRoute => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::List;
                    self.pending_base_url = None;
                    ViewAction::None
                }
                KeyCode::Up | KeyCode::Down => {
                    self.stepfun_billing_route = match self.stepfun_billing_route {
                        StepfunBillingRoute::PayAsYouGo => StepfunBillingRoute::StepPlan,
                        StepfunBillingRoute::StepPlan => StepfunBillingRoute::PayAsYouGo,
                    };
                    ViewAction::None
                }
                KeyCode::Char('1') => {
                    self.stepfun_billing_route = StepfunBillingRoute::PayAsYouGo;
                    ViewAction::None
                }
                KeyCode::Char('2') => {
                    self.stepfun_billing_route = StepfunBillingRoute::StepPlan;
                    ViewAction::None
                }
                KeyCode::Enter => {
                    self.apply_stepfun_billing_route();
                    ViewAction::None
                }
                _ => ViewAction::None,
            },
            Stage::PlanTier => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::ModelPick;
                    ViewAction::None
                }
                KeyCode::Up | KeyCode::Down => {
                    self.kimi_code_plan_tier = match self.kimi_code_plan_tier {
                        KimiCodePlanTier::Safe262k => KimiCodePlanTier::OneMillion,
                        KimiCodePlanTier::OneMillion => KimiCodePlanTier::Safe262k,
                    };
                    ViewAction::None
                }
                KeyCode::Char('1') => {
                    self.kimi_code_plan_tier = KimiCodePlanTier::Safe262k;
                    ViewAction::None
                }
                KeyCode::Char('2') => {
                    self.kimi_code_plan_tier = KimiCodePlanTier::OneMillion;
                    ViewAction::None
                }
                KeyCode::Enter => {
                    self.apply_plan_tier();
                    ViewAction::None
                }
                _ => ViewAction::None,
            },
            Stage::Confirm => match key.code {
                KeyCode::Esc => {
                    self.stage = if self.selected_kimi_code_k3() {
                        Stage::PlanTier
                    } else {
                        Stage::ModelPick
                    };
                    ViewAction::None
                }
                KeyCode::Enter => self
                    .build_setup_confirmed_event()
                    .map(ViewAction::EmitAndClose)
                    .unwrap_or(ViewAction::None),
                _ => ViewAction::None,
            },
            Stage::TemplateList => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::List;
                    ViewAction::None
                }
                KeyCode::Up => {
                    self.move_template_selection(-1);
                    ViewAction::None
                }
                KeyCode::Down => {
                    self.move_template_selection(1);
                    ViewAction::None
                }
                KeyCode::Enter => self.activate_selected_template(),
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
            Stage::TemplateList => {
                return match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        self.move_template_selection(-1);
                        ViewAction::None
                    }
                    MouseEventKind::ScrollDown => {
                        self.move_template_selection(1);
                        ViewAction::None
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        self.handle_template_list_click(mouse)
                    }
                    _ => ViewAction::None,
                };
            }
            Stage::PlanTier
            | Stage::StepfunBillingRoute
            | Stage::XaiAuthChoice
            | Stage::KeyEntry
            | Stage::ExternalConsentChoice
            | Stage::ExternalConsentConfirm
            | Stage::Confirm
            | Stage::CustomForm => {}
        }
        ViewAction::None
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let preferred_height = match self.stage {
            Stage::List => (self.rows.len() as u16).saturating_add(2),
            Stage::XaiAuthChoice => 12,
            // Key/OAuth help is intentionally multi-line and wraps at narrow
            // widths. One shared height keeps every provider's final guidance
            // visible instead of special-casing whichever route clipped last.
            Stage::KeyEntry => 14,
            Stage::ExternalConsentChoice => 12,
            Stage::ExternalConsentConfirm => 13,
            Stage::ModelPick => 12,
            Stage::PlanTier => 10,
            Stage::StepfunBillingRoute => 11,
            Stage::Confirm => 10,
            Stage::CustomForm => 12,
            Stage::TemplateList => 16,
        };
        let popup_area = centered_modal_area(area, 120, preferred_height, 64, 8);

        render_modal_surface(area, popup_area, buf);

        match self.stage {
            Stage::List => self.render_list(popup_area, buf),
            Stage::XaiAuthChoice => self.render_xai_auth_choice(popup_area, buf),
            Stage::KeyEntry => self.render_key_entry(popup_area, buf),
            Stage::ExternalConsentChoice => self.render_external_consent_choice(popup_area, buf),
            Stage::ExternalConsentConfirm => self.render_external_consent_confirm(popup_area, buf),
            Stage::ModelPick => self.render_model_pick(popup_area, buf),
            Stage::PlanTier => self.render_plan_tier(popup_area, buf),
            Stage::StepfunBillingRoute => self.render_stepfun_billing_route(popup_area, buf),
            Stage::Confirm => self.render_confirm(popup_area, buf),
            Stage::CustomForm => self.render_custom_form(popup_area, buf),
            Stage::TemplateList => self.render_template_list(popup_area, buf),
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

mod list_keys;

#[cfg(test)]
mod tests;
