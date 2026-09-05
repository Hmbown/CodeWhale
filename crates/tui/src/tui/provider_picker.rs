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
use crate::provider_lake::{
    all_catalog_models_for_provider, catalog_model_count_for_provider, catalog_offering_for_model,
};
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
    /// Explicit ChatGPT/Codex acquisition choice. Native PKCE subscription
    /// sign-in is first-class; Codex CLI import remains an alternative.
    ChatgptAuthChoice,
    KeyEntry,
    /// Explicit disabled/read-only/managed external-credential policy choice.
    ExternalConsentChoice,
    /// Full owner/path/side-effect disclosure before a read grant is saved.
    ExternalConsentConfirm,
    /// Explicit confirmation before revoking external access; revocation
    /// clears only Codewhale-owned consent state (#5772).
    ExternalConsentRevokeConfirm,
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
enum ChatgptAuthChoice {
    SignInWithChatgpt,
    ImportCodexCli,
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
/// normal `/provider` default; first-run onboarding opens `Catalog` so a user
/// can pick a hosted API or a local runtime immediately. `A` still exposes the
/// full catalog and `L` filters to local routes.
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
    chatgpt_auth_choice: ChatgptAuthChoice,
    external_consent_choice: ExternalConsentChoice,
    /// Where Esc returns from the revoke confirmation. Revocation is reachable
    /// both from the list (`x`) and from the policy choice, and "back" has to
    /// mean the step the user actually came from.
    external_revoke_return: Stage,
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
    /// Pointer geometry for the two-pane picker (Slice D): provider-strip
    /// rows on the left, model rows on the right/under, recorded during
    /// render like the template hitboxes above.
    list_row_hitboxes: RefCell<Vec<(Rect, usize)>>,
    model_row_hitboxes: RefCell<Vec<(Rect, usize)>>,
    consent_row_hitboxes: RefCell<Vec<(Rect, usize)>>,
    /// Pointer hover positions. Advisory only — hover never moves the
    /// keyboard selection; it renders with the shared
    /// [`crate::tui::menu_style::hovered_row_style`] primitive.
    hovered_list_idx: Option<usize>,
    hovered_model_idx: Option<usize>,
    hovered_consent_idx: Option<usize>,
    /// Last clicked row per pane for single-click-select /
    /// double-click-activate rhythm (mirrors the model picker).
    last_list_mouse_selected: Option<usize>,
    last_model_mouse_selected: Option<usize>,
    hovered_template_idx: Option<usize>,
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
        // #5772: normal picker copy never carries an external file path —
        // the exact path is disclosed only inside the explicit reuse
        // confirmation.
        let credential_source = match &credential_resolution.source {
            crate::credentials::CredentialSource::ExternalGrant { cli, .. } => {
                format!("{cli} credentials (read-only)")
            }
            other => other.label().into_owned(),
        };
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
        // Slice D: cost lives at the model level (per-model $/mtok in the
        // models pane and the model-pick stage), never on the provider row.
        // Auth guidance that used to ride the provider meter (e.g. the Kimi
        // imported-token hint) travels on `messages` via
        // `missing_auth_message` below instead.
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
                    false,
                    None,
                    None,
                )
            }
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
        // #5772: the status projection needs an ambient candidate path to
        // report `ambient_path_changed`, so resolving it for a provider the
        // user never consented to would derive (and then render) another CLI's
        // credential location during ordinary browsing. Ask for it only once a
        // consent record exists.
        let external_credential_status = configured
            .and_then(|entry| entry.external_credentials.as_ref())
            .and_then(|_| config.external_credential_consent_status(provider));

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
        // Slice D: no provider-level cost — per-model $/mtok lives in the
        // models pane and the model-pick stage.
        format!(
            "{} | {} | {} | base:{}{} | route:{}{} origin:{} | {} | {}{} | catalog:{}{}",
            self.readiness.label(),
            self.auth_status.label(),
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

/// Slice D: cost lives at the model level. Per-model $/mtok in/out projected
/// from the merged catalog offering, with honest non-token fallbacks. `None`
/// pricing is an unknown, never a fabricated zero.
///
/// Provider-agnostic fallbacks keep the label truthful when the catalog has
/// no row: self-hosted routes are local, Codex rides OAuth quota, and
/// everything else is honestly unknown.
fn model_cost_label(provider: ApiProvider, model: &str) -> String {
    // OpenCode Go spends a subscription allowance, not per-token dollars, so
    // a catalog token price would misreport it as metered spend (#4526).
    if provider == ApiProvider::OpencodeGo {
        return "plan".to_string();
    }
    let pricing =
        catalog_offering_for_model(provider, model).map(|offering| offering.to_offering().pricing);
    model_cost_label_for_pricing(provider, pricing.as_ref())
}

/// Slice D two-pane picker: `(model, per-model cost, is_default_route)` rows
/// for the models pane beside/under the provider strip. The default route's
/// model sorts first so the eye lands on what Enter would use; the rest are
/// alphabetical. Falls back to the default route when the catalog has no rows
/// for the provider, so the pane never renders empty.
fn provider_pane_models(row: &ProviderDashboardRow, limit: usize) -> Vec<(String, String, bool)> {
    let mut models = all_catalog_models_for_provider(row.provider);
    if models.is_empty() && !row.default_route.logical_model.trim().is_empty() {
        models.push(row.default_route.logical_model.clone());
    }
    models.sort_by_key(|model| model.to_ascii_lowercase());
    models.dedup_by_key(|model| model.to_ascii_lowercase());
    let default = row.default_route.logical_model.clone();
    let wire = row.default_route.wire_model.clone();
    models.sort_by_key(|model| {
        (!model.eq_ignore_ascii_case(&default) && !model.eq_ignore_ascii_case(&wire)) as u8
    });
    models
        .into_iter()
        .take(limit.max(1))
        .map(|model| {
            let is_default =
                model.eq_ignore_ascii_case(&default) || model.eq_ignore_ascii_case(&wire);
            let price = model_cost_label(row.provider, &model);
            (model, price, is_default)
        })
        .collect()
}

fn model_cost_label_for_pricing(provider: ApiProvider, pricing: Option<&PricingSku>) -> String {
    // OpenCode Go spends a subscription allowance, not per-token dollars, so
    // a catalog token price would misreport it as metered spend (#4526).
    if provider == ApiProvider::OpencodeGo {
        return "plan".to_string();
    }
    match pricing {
        Some(PricingSku::Token {
            input_per_mtok,
            output_per_mtok,
        }) => match (input_per_mtok, output_per_mtok) {
            (Some(input), Some(output)) => format!("${input:.2}/${output:.2} mtok"),
            _ => "token-priced".to_string(),
        },
        Some(PricingSku::SubscriptionQuota { .. }) => "plan".to_string(),
        Some(PricingSku::AccountCredits { .. }) => "credits".to_string(),
        Some(PricingSku::LocalOrNotApplicable) => "local".to_string(),
        Some(PricingSku::UnknownOrStale) | None => match provider {
            ApiProvider::Ollama | ApiProvider::Sglang | ApiProvider::Vllm => "local".to_string(),
            ApiProvider::OpenaiCodex => "oauth quota".to_string(),
            ApiProvider::OpencodeZen => "pay-as-you-go".to_string(),
            _ => "price ?".to_string(),
        },
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

/// Whether a provider has a supported external credential owner at all.
///
/// Pure provider metadata. Unlike [`external_consent_target_for_provider`] it
/// resolves no path, so deciding whether to *offer* the reuse action never
/// derives a HOME-based candidate location (#5772).
#[must_use]
pub(crate) fn provider_supports_external_consent(provider: ApiProvider) -> bool {
    matches!(provider, ApiProvider::OpenaiCodex | ApiProvider::Xai)
}

/// Resolve the external credential target for a provider that supports
/// read-only external consent. This is the same lower-level fact the
/// provider picker uses to build its consent flow; Fleet setup reuses it
/// for route-scoped activation without switching the parent session.
///
/// #5772: only the disclosure and confirmation steps may call this. Ordinary
/// browsing and key entry must use [`provider_supports_external_consent`],
/// because the resolved candidate is exactly what the confirmation exists to
/// disclose.
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
            chatgpt_auth_choice: ChatgptAuthChoice::SignInWithChatgpt,
            external_consent_choice: ExternalConsentChoice::Disabled,
            external_revoke_return: Stage::List,
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
            list_row_hitboxes: RefCell::new(Vec::new()),
            model_row_hitboxes: RefCell::new(Vec::new()),
            consent_row_hitboxes: RefCell::new(Vec::new()),
            hovered_list_idx: None,
            hovered_model_idx: None,
            hovered_consent_idx: None,
            last_list_mouse_selected: None,
            last_model_mouse_selected: None,
            hovered_template_idx: None,
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
        } else if self.selected_provider() == ApiProvider::OpenaiCodex {
            self.enter_chatgpt_auth_choice();
        } else if self.stepfun_billing_route_applies() {
            self.enter_stepfun_billing_route();
        } else {
            self.enter_key_entry();
        }
    }

    fn enter_chatgpt_auth_choice(&mut self) {
        self.chatgpt_auth_choice = ChatgptAuthChoice::SignInWithChatgpt;
        self.stage = Stage::ChatgptAuthChoice;
        self.api_key_input.clear();
        self.key_entry_error = None;
        self.pending_api_key = None;
    }

    fn move_chatgpt_auth_choice(&mut self) {
        self.chatgpt_auth_choice = match self.chatgpt_auth_choice {
            ChatgptAuthChoice::SignInWithChatgpt => ChatgptAuthChoice::ImportCodexCli,
            ChatgptAuthChoice::ImportCodexCli => ChatgptAuthChoice::SignInWithChatgpt,
        };
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
        // Capability only (#5772): the candidate path stays unresolved until
        // the user picks ReadOnly and the confirmation discloses it.
        if provider_supports_external_consent(self.selected_provider()) {
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
        crate::config::is_exact_kimi_code_bare_k3_route(
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

    fn enter_ds4_form(&mut self) {
        self.stage = Stage::CustomForm;
        self.custom_provider_field = CustomProviderField::ApiKeyEnv;
        self.custom_provider_id = DS4_PROVIDER_ID.to_string();
        self.custom_provider_base_url = DS4_BASE_URL.to_string();
        self.custom_provider_model = DS4_DEFAULT_MODEL.to_string();
        self.custom_provider_api_key_env.clear();
    }

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

    fn enter_sensenova_form(&mut self) {
        if let Some(template) = provider_setup_template(SENSENOVA_TEMPLATE_ID) {
            self.apply_template(template);
        }
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
                    .fg(palette::WHALE_ACTION)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_BG));
        let inner = outer.inner(area);
        outer.render(area, buf);

        let view_action = match self.view {
            ProviderListView::Configured => self.tr(MessageId::PickerActionBrowseAll),
            ProviderListView::Catalog => self.tr(MessageId::PickerActionConfigured),
            ProviderListView::Local => self.tr(MessageId::PickerActionBrowseAll),
        };
        let search_active = !self.query.trim().is_empty();
        // The action footer moves into the body so it wraps instead of clipping
        // at narrow widths (#3732); the provider list renders above it.
        let content = if self.onboarding_mode {
            let mut hints = vec![
                ActionHint::new("↑↓", self.tr(MessageId::PickerActionMove)),
                ActionHint::new("Enter", enter_action),
            ];
            if self.view == ProviderListView::Local {
                hints.push(ActionHint::new(
                    "A",
                    self.tr(MessageId::PickerActionBrowseAll),
                ));
            }
            hints.extend([
                ActionHint::new("Ctrl+O", self.tr(MessageId::OnboardProviderOffline)),
                ActionHint::new("Esc", self.tr(MessageId::OnboardActionBack)),
            ]);
            render_modal_footer(inner, buf, &hints)
        } else if search_active {
            render_modal_footer(
                inner,
                buf,
                &[
                    // Two-stage Esc (clear the query, then cancel) reads as one
                    // hint instead of a duplicated key.
                    ActionHint::new(
                        "Esc",
                        format!(
                            "{} / {}",
                            self.tr(MessageId::PickerActionClear),
                            self.tr(MessageId::PickerActionCancel)
                        ),
                    ),
                    ActionHint::new("↑↓", self.tr(MessageId::PickerActionMove)),
                    ActionHint::new("Enter", enter_action),
                    ActionHint::new("A", view_action.clone()),
                    ActionHint::new("L", "local only"),
                    ActionHint::new("I", "LM Studio"),
                    ActionHint::new("C", self.tr(MessageId::PickerActionCustom)),
                    ActionHint::new("D", "DS4"),
                    ActionHint::new("S", "SenseNova"),
                ],
            )
        } else {
            render_modal_footer(
                inner,
                buf,
                &[
                    ActionHint::new("↑↓", self.tr(MessageId::PickerActionMove)),
                    ActionHint::new("a-z", self.tr(MessageId::PickerActionJump)),
                    ActionHint::new("Enter", enter_action),
                    ActionHint::new("A", view_action),
                    ActionHint::new("L", "local only"),
                    ActionHint::new("I", "LM Studio"),
                    ActionHint::new("C", self.tr(MessageId::PickerActionCustom)),
                    ActionHint::new("D", "DS4"),
                    ActionHint::new("S", "SenseNova"),
                    ActionHint::new("P", self.tr(MessageId::PickerActionTemplates)),
                    ActionHint::new("C-t", self.tr(MessageId::PickerActionTestConnection)),
                    ActionHint::new("R", self.tr(MessageId::PickerActionEditKey)),
                    ActionHint::new("E", self.tr(MessageId::ProviderExternalActionChoices)),
                    ActionHint::new("X", self.tr(MessageId::ProviderExternalActionRevoke)),
                    ActionHint::new("M", self.tr(MessageId::PickerActionModels)),
                    ActionHint::new("Esc", self.tr(MessageId::PickerActionCancel)),
                ],
            )
        };

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
        // Slice D two-pane picker: the provider strip lives on the left and
        // every visible row is clickable, so record this frame's geometry for
        // hover + click handling (mirrors the model picker hitboxes).
        self.list_row_hitboxes.borrow_mut().clear();
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
            let is_hovered = self.hovered_list_idx == Some(*idx);
            let label_style = if is_selected {
                menu_style::selected_row_style_with_fg(palette::SELECTION_TEXT)
            } else if is_hovered {
                menu_style::hovered_row_style()
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
            let row_y = layout.list.y.saturating_add(lines.len() as u16);
            self.list_row_hitboxes
                .borrow_mut()
                .push((Rect::new(layout.list.x, row_y, layout.list.width, 1), *idx));
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
                format!("Protocol: {}", row.supported_protocols.join("+")),
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
        // #5772: the external block exists only for a persisted consent record
        // (see the row constructor), and it names the owning CLI without the
        // file path. The exact path is disclosed solely inside the explicit
        // reuse confirmation; a dormant candidate must never leak HOME-derived
        // locations into ordinary browsing.
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
            let owner = self
                .tr(MessageId::ProviderExternalOwnerOnly)
                .replace("{owner}", status.owner);
            let mut owner_spans = vec![Span::styled(
                owner,
                Style::default().fg(palette::TEXT_MUTED),
            )];
            if status.ambient_path_changed {
                let warning = self
                    .tr(MessageId::ProviderExternalPinnedPathChanged)
                    .replace("{owner}", status.owner);
                owner_spans.push(Span::styled(
                    " | ",
                    Style::default().fg(palette::TEXT_MUTED),
                ));
                owner_spans.push(Span::styled(
                    warning,
                    Style::default().fg(palette::STATUS_WARNING),
                ));
            }
            lines.push(Line::from(owner_spans));
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
        // Slice D two-pane picker: the selected provider's models live
        // beside (wide) or under (narrow) the provider strip, each with its
        // own $/mtok in/out from the catalog. Last on purpose: when the pane
        // is short, clipping eats models — never the consent block above.
        // Display-only — choosing a model happens in the model picker (`M`)
        // or the guided setup flow.
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Models · $in/$out per mtok",
            Style::default()
                .fg(palette::TEXT_PRIMARY)
                .add_modifier(Modifier::BOLD),
        )));
        let pane_models = provider_pane_models(row, 8);
        let name_budget = usize::from(inner.width).saturating_sub(22).max(8);
        for (model, price, is_default) in &pane_models {
            let name = crate::tui::ui_text::truncate_line_to_width(model, name_budget);
            let mut spans = vec![
                Span::styled("  ", Style::default()),
                Span::styled(name, Style::default().fg(palette::TEXT_PRIMARY)),
                Span::styled(
                    format!("  {price}"),
                    Style::default().fg(palette::TEXT_MUTED),
                ),
            ];
            if *is_default {
                spans.push(Span::styled(
                    "  (default)",
                    Style::default().fg(palette::WHALE_ACTION),
                ));
            }
            lines.push(Line::from(spans));
        }
        let total_models = all_catalog_models_for_provider(row.provider).len();
        if total_models > pane_models.len() {
            lines.push(Line::from(Span::styled(
                format!("  +{} more · M for all", total_models - pane_models.len()),
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
                    .fg(palette::WHALE_ACTION)
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

    fn render_chatgpt_auth_choice(&self, area: Rect, buf: &mut Buffer) {
        let outer = Block::default()
            .title(Line::from(Span::styled(
                self.tr(MessageId::ChatgptAuthChoiceTitle),
                Style::default()
                    .fg(palette::WHALE_ACTION)
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
                ActionHint::new("E", self.tr(MessageId::ProviderExternalActionReuseCodex)),
                ActionHint::new("Esc", self.tr(MessageId::SetupActionBack)),
            ],
        );
        let marker =
            |choice| crate::tui::glyphs::selection_marker(self.chatgpt_auth_choice == choice);
        Paragraph::new(vec![
            Line::from(self.tr(MessageId::ChatgptAuthChoiceIntro)),
            Line::from(""),
            Line::from(format!(
                "{} 1. {}",
                marker(ChatgptAuthChoice::SignInWithChatgpt),
                self.tr(MessageId::ChatgptAuthChoicePkceOption),
            )),
            Line::from(format!(
                "{} 2. {}",
                marker(ChatgptAuthChoice::ImportCodexCli),
                self.tr(MessageId::ChatgptAuthChoiceImportOption),
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
                    .fg(palette::WHALE_ACTION)
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
                    .fg(palette::WHALE_ACTION)
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
        // Slice D explicit-consent gate, Gate 1 of 2: choose access. The
        // consent backend (#5779) and every localized string are unchanged —
        // only the gate framing and the clickable rows are new.
        self.consent_row_hitboxes.borrow_mut().clear();
        let selected = self.external_consent_choice;
        let options = [
            (
                ExternalConsentChoice::Disabled,
                '1',
                self.tr(MessageId::ProviderExternalDisabledLabel),
                self.tr(MessageId::ProviderExternalDisabledDetail),
            ),
            (
                ExternalConsentChoice::ReadOnly,
                '2',
                self.tr(MessageId::ProviderExternalReadOnlyLabel),
                self.tr(MessageId::ProviderExternalReadOnlyDetail),
            ),
            (
                ExternalConsentChoice::ManagedUnavailable,
                '3',
                self.tr(MessageId::ProviderExternalManagedLabel),
                self.tr(MessageId::ProviderExternalManagedDetail),
            ),
        ];
        // Options render first at fixed rows (header + one line per
        // option) so hitboxes stay exact; the wrapping explainer lines live
        // below where wrapping cannot disturb pointer geometry.
        let mut lines = vec![Line::from(Span::styled(
            "Gate 1 of 2 · choose access",
            Style::default()
                .fg(palette::TEXT_PRIMARY)
                .add_modifier(Modifier::BOLD),
        ))];
        for (slot, (choice, digit, label, detail)) in options.iter().enumerate() {
            let is_selected = selected == *choice;
            let is_hovered = self.hovered_consent_idx == Some(slot);
            let marker = crate::tui::glyphs::selection_marker(is_selected);
            let label_style = if is_selected {
                menu_style::selected_row_style_with_fg(palette::WHALE_ACTION)
            } else if is_hovered {
                menu_style::hovered_row_style()
            } else {
                Style::default().fg(palette::TEXT_PRIMARY)
            };
            let mut label_line = Line::from(vec![
                Span::styled(format!("{marker} {digit}. {label}"), label_style),
                Span::styled(
                    format!(" · {detail}"),
                    Style::default().fg(palette::TEXT_MUTED),
                ),
            ]);
            if is_selected {
                label_line.style = menu_style::selected_row_bg_style();
            }
            lines.push(label_line);
            let row_y = content.y.saturating_add(1 + slot as u16);
            self.consent_row_hitboxes
                .borrow_mut()
                .push((Rect::new(content.x, row_y, content.width, 1), slot));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            self.tr(MessageId::ProviderExternalChoiceIntro),
            Style::default().fg(palette::TEXT_MUTED),
        )));
        lines.push(Line::from(Span::styled(
            "Enter continues · nothing is read or saved until Gate 2 confirms.",
            Style::default().fg(palette::TEXT_MUTED),
        )));
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
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
                    .fg(palette::WHALE_ACTION)
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
        let route_label = self.tr(MessageId::ProviderExternalRouteLabel);
        let owner_label = self.tr(MessageId::ProviderExternalOwnerLabel);
        let exact_path_label = self.tr(MessageId::ProviderExternalExactPathLabel);
        let semantics_label = self.tr(MessageId::ProviderExternalSemanticsLabel);
        let revoke_label = self.tr(MessageId::ProviderExternalRevokeLabel);
        // #5772: the disclosure names the exact route (endpoint + default
        // model), the source adapter, the custody boundary (local device
        // only), the credential/billing owner, and the local-only revoke
        // consequence before any validate/read/persist may run.
        let row = &self.rows[self.selected_idx];
        Paragraph::new(vec![
            // Slice D explicit-consent gate, Gate 2 of 2: review the exact
            // disclosure before granting. Backend (#5779) unchanged.
            Line::from(Span::styled(
                "Gate 2 of 2 · review before granting",
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(format!("{provider_label}: {}", provider.as_str())),
            Line::from(format!(
                "{route_label}: {} · {}",
                row.base_url, row.default_route.logical_model
            )),
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
            Line::from(self.tr(MessageId::ProviderExternalCustodyLine).into_owned()),
            Line::from(
                self.tr(MessageId::ProviderExternalBillingLine)
                    .replace("{owner}", source.owner_label()),
            ),
            Line::from(self.tr(MessageId::ProviderExternalRejectUnsafe)),
            Line::from(
                self.tr(MessageId::ProviderExternalRevokeScope)
                    .replace("{owner}", source.owner_label()),
            ),
            Line::from(format!(
                "{revoke_label}: codewhale auth external-revoke --provider {}",
                provider.as_str()
            )),
        ])
        .wrap(Wrap { trim: false })
        .render(content, buf);
    }

    /// Revocation disclosure (#5772): names what is cleared (only the
    /// Codewhale-owned consent record on this device) and what is never
    /// touched (the external CLI's file). Enter revokes; Esc goes back.
    fn render_external_consent_revoke_confirm(&self, area: Rect, buf: &mut Buffer) {
        let provider_name = self.rows[self.selected_idx].display_name.clone();
        let outer = Block::default()
            .title(Line::from(Span::styled(
                self.tr(MessageId::ProviderExternalRevokeConfirmTitle),
                Style::default()
                    .fg(palette::WHALE_ACTION)
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
                ActionHint::new("Enter", self.tr(MessageId::ProviderExternalActionRevoke)),
                ActionHint::new("Esc", self.tr(MessageId::SetupActionBack)),
            ],
        );
        let provider_label = self.tr(MessageId::RouteProviderLabel);
        let owner = self
            .rows
            .get(self.selected_idx)
            .and_then(|row| row.external_credential_status.as_ref())
            .map(|status| status.owner)
            .unwrap_or(self.selected_provider().display_name());
        Paragraph::new(vec![
            Line::from(format!("{provider_label}: {provider_name}")),
            Line::from(
                self.tr(MessageId::ProviderExternalRevokeScope)
                    .replace("{owner}", owner),
            ),
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
                    .fg(palette::WHALE_ACTION)
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
        // Slice D: every model row carries its own $/mtok from the catalog
        // and every visible row is clickable, so record this frame's geometry
        // for hover + click handling.
        self.model_row_hitboxes.borrow_mut().clear();
        let model_provider = self.rows[self.selected_idx].provider;
        let mut lines: Vec<Line> = Vec::with_capacity(visible_rows);
        for (idx, model) in self
            .model_options
            .iter()
            .enumerate()
            .skip(visible_start)
            .take(visible_rows)
        {
            let is_selected = idx == self.model_selected_idx;
            let is_hovered = self.hovered_model_idx == Some(idx);
            let arrow = crate::tui::glyphs::selection_marker(is_selected);
            let label_style = if is_selected {
                menu_style::selected_row_style_with_fg(palette::SELECTION_TEXT)
            } else if is_hovered {
                menu_style::hovered_row_style()
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
            // Slice D: cost moved off the provider level down to the model.
            let price = model_cost_label(model_provider, model);
            let mut spans = vec![
                Span::styled(format!(" {arrow} {model}"), label_style),
                Span::styled(
                    format!("  {price}"),
                    if is_selected {
                        menu_style::selected_row_style_with_fg(palette::TEXT_MUTED)
                    } else {
                        Style::default().fg(palette::TEXT_MUTED)
                    },
                ),
            ];
            if !default_tag.is_empty() {
                spans.push(Span::styled(
                    format!("  ({default_tag})"),
                    if is_selected {
                        menu_style::selected_row_style_with_fg(palette::TEXT_MUTED)
                    } else {
                        Style::default().fg(palette::TEXT_MUTED)
                    },
                ));
            }
            let mut line = Line::from(spans);
            if is_selected {
                line.style = menu_style::selected_row_bg_style();
            }
            let row_y = list_area.y.saturating_add(lines.len() as u16);
            self.model_row_hitboxes
                .borrow_mut()
                .push((Rect::new(list_area.x, row_y, list_area.width, 1), idx));
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
                    .fg(palette::WHALE_ACTION)
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
                    .fg(palette::WHALE_ACTION)
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
                    .fg(palette::WHALE_ACTION)
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
                    .fg(palette::WHALE_ACTION)
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
                    .fg(palette::WHALE_ACTION)
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
            // Slice D hover rule: template rows are clickable, so they
            // hover-respond with the shared primitive like every other row.
            let style = if selected_row {
                menu_style::selected_row_style_with_fg(palette::SELECTION_TEXT)
            } else if self.hovered_template_idx == Some(idx) {
                menu_style::hovered_row_style()
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
            menu_style::selected_row_style_with_fg(palette::WHALE_ACTION)
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

    /// Slice D two-pane pointer support: hover is advisory (it never moves
    /// the keyboard selection) and renders with the shared
    /// [`crate::tui::menu_style::hovered_row_style`] primitive; click selects
    /// and a second click activates, mirroring the model picker rhythm.
    fn list_hit_at(&self, mouse: MouseEvent) -> Option<usize> {
        let pos = Position::new(mouse.column, mouse.row);
        self.list_row_hitboxes
            .borrow()
            .iter()
            .find_map(|(rect, idx)| rect.contains(pos).then_some(*idx))
    }

    fn model_hit_at(&self, mouse: MouseEvent) -> Option<usize> {
        let pos = Position::new(mouse.column, mouse.row);
        self.model_row_hitboxes
            .borrow()
            .iter()
            .find_map(|(rect, idx)| rect.contains(pos).then_some(*idx))
    }

    fn consent_hit_at(&self, mouse: MouseEvent) -> Option<usize> {
        let pos = Position::new(mouse.column, mouse.row);
        self.consent_row_hitboxes
            .borrow()
            .iter()
            .find_map(|(rect, slot)| rect.contains(pos).then_some(*slot))
    }

    /// Enter on the list stage, shared by keyboard and double-click so both
    /// paths apply, set up, or route to the custom form identically.
    fn activate_selected_row(&mut self) -> ViewAction {
        if !self.row_visible(self.selected_idx) {
            return ViewAction::None;
        }
        let provider = self.selected_provider();
        let provider_id = self.selected_provider_id();
        if provider == ApiProvider::Custom && !self.rows[self.selected_idx].is_configured {
            self.enter_custom_form();
            ViewAction::None
        } else if !self.selected_route_is_valid() {
            ViewAction::None
        } else if self.selected_has_key() {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied {
                provider,
                provider_id,
            })
        } else {
            // #5772: plain activation never inspects or adopts an external
            // CLI credential. Reuse starts only from the explicit `e` action,
            // which discloses the exact path and requires its own
            // confirmation.
            self.begin_setup();
            ViewAction::None
        }
    }

    /// Enter on the model-pick stage, shared by keyboard and double-click.
    fn advance_from_model_pick(&mut self) -> ViewAction {
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

    fn click_list_row(&mut self, mouse: MouseEvent) -> ViewAction {
        let Some(idx) = self.list_hit_at(mouse) else {
            return ViewAction::None;
        };
        let activate = self.last_list_mouse_selected == Some(idx) && self.selected_idx == idx;
        self.selected_idx = idx;
        self.last_list_mouse_selected = Some(idx);
        if activate {
            self.activate_selected_row()
        } else {
            ViewAction::None
        }
    }

    fn click_model_row(&mut self, mouse: MouseEvent) -> ViewAction {
        let Some(idx) = self.model_hit_at(mouse) else {
            return ViewAction::None;
        };
        let advance = self.last_model_mouse_selected == Some(idx) && self.model_selected_idx == idx;
        self.model_selected_idx = idx.min(self.model_options.len().saturating_sub(1));
        self.selected_model = self.model_options.get(self.model_selected_idx).cloned();
        self.last_model_mouse_selected = Some(idx);
        if advance {
            self.advance_from_model_pick()
        } else {
            ViewAction::None
        }
    }

    fn click_consent_row(&mut self, mouse: MouseEvent) {
        let Some(slot) = self.consent_hit_at(mouse) else {
            return;
        };
        // Single click chooses; Enter still commits, so a stray click can
        // never grant or revoke access by itself.
        self.external_consent_choice = match slot {
            0 => ExternalConsentChoice::Disabled,
            1 => ExternalConsentChoice::ReadOnly,
            _ => ExternalConsentChoice::ManagedUnavailable,
        };
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
            | Stage::ChatgptAuthChoice
            | Stage::ExternalConsentChoice
            | Stage::ExternalConsentConfirm
            | Stage::ExternalConsentRevokeConfirm
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
                // Keyboard and double-click share `activate_selected_row`
                // (Slice D) so both paths behave identically.
                KeyCode::Enter => self.activate_selected_row(),
                KeyCode::Char(c)
                    if key.modifiers.is_empty()
                        && self.query.is_empty()
                        && c.eq_ignore_ascii_case(&'x')
                        && self.row_visible(self.selected_idx)
                        && self.rows[self.selected_idx].credential_state
                            == CredentialState::ExternalConsent =>
                {
                    // #5772: revoking external access asks first; only an
                    // affirmative Enter on the revoke confirmation clears the
                    // Codewhale-owned consent record.
                    self.external_revoke_return = Stage::List;
                    self.stage = Stage::ExternalConsentRevokeConfirm;
                    ViewAction::None
                }
                KeyCode::Char(c)
                    if key.modifiers.is_empty()
                        && c.eq_ignore_ascii_case(&'e')
                        && self.query.is_empty()
                        && self.row_visible(self.selected_idx)
                        && provider_supports_external_consent(self.selected_provider()) =>
                {
                    // #5772: `e` is the one explicit "use external CLI
                    // credentials" action. It only opens the policy choice;
                    // the exact path is disclosed at the confirmation step
                    // and nothing is validated, read, or persisted before an
                    // affirmative Enter there.
                    self.enter_external_consent_choice();
                    ViewAction::None
                }
                KeyCode::Char(c)
                    if key.modifiers.is_empty()
                        && c.eq_ignore_ascii_case(&'r')
                        && self.query.is_empty()
                        && self.row_visible(self.selected_idx) =>
                {
                    self.begin_setup();
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
                        && c.eq_ignore_ascii_case(&'l') =>
                {
                    self.show_local_routes();
                    ViewAction::None
                }
                KeyCode::Char(c)
                    if key.modifiers.is_empty()
                        && self.query.is_empty()
                        && c.eq_ignore_ascii_case(&'i') =>
                {
                    self.enter_lm_studio_form();
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
                KeyCode::Char(c)
                    if key.modifiers.is_empty()
                        && self.query.is_empty()
                        && c.eq_ignore_ascii_case(&'d') =>
                {
                    self.enter_ds4_form();
                    ViewAction::None
                }
                KeyCode::Char(c)
                    if key.modifiers.is_empty()
                        && self.query.is_empty()
                        && c.eq_ignore_ascii_case(&'s') =>
                {
                    self.enter_sensenova_form();
                    ViewAction::None
                }
                KeyCode::Char(c)
                    if key.modifiers.is_empty()
                        && self.query.is_empty()
                        && c.eq_ignore_ascii_case(&'p') =>
                {
                    self.enter_template_list();
                    ViewAction::None
                }
                KeyCode::Char(c)
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && c.eq_ignore_ascii_case(&'t')
                        && self.row_visible(self.selected_idx) =>
                {
                    ViewAction::EmitAndClose(ViewEvent::ProviderPickerTestConnection {
                        provider: self.selected_provider(),
                        provider_id: self.selected_provider_id(),
                        catalog_view: self.view == ProviderListView::Catalog,
                    })
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
            Stage::ChatgptAuthChoice => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::List;
                    ViewAction::None
                }
                KeyCode::Up | KeyCode::Down => {
                    self.move_chatgpt_auth_choice();
                    ViewAction::None
                }
                KeyCode::Char('1') => {
                    self.chatgpt_auth_choice = ChatgptAuthChoice::SignInWithChatgpt;
                    ViewAction::None
                }
                KeyCode::Char('2') => {
                    self.chatgpt_auth_choice = ChatgptAuthChoice::ImportCodexCli;
                    ViewAction::None
                }
                KeyCode::Char(c) if key.modifiers.is_empty() && c.eq_ignore_ascii_case(&'e') => {
                    self.enter_external_consent_choice();
                    ViewAction::None
                }
                KeyCode::Enter => match self.chatgpt_auth_choice {
                    ChatgptAuthChoice::SignInWithChatgpt => {
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerChatgptOAuthRequested)
                    }
                    ChatgptAuthChoice::ImportCodexCli => {
                        self.enter_external_consent_choice();
                        ViewAction::None
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
                    } else if self.selected_provider() == ApiProvider::OpenaiCodex {
                        Stage::ChatgptAuthChoice
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
                    } else if self.selected_provider() == ApiProvider::OpenaiCodex {
                        Stage::ChatgptAuthChoice
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
                        // #5772: revocation is destructive to Codewhale-owned
                        // state, so it gets its own confirmation instead of a
                        // one-key commit.
                        self.external_revoke_return = Stage::ExternalConsentChoice;
                        self.stage = Stage::ExternalConsentRevokeConfirm;
                        ViewAction::None
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
            Stage::ExternalConsentRevokeConfirm => match key.code {
                KeyCode::Esc => {
                    self.stage = self.external_revoke_return;
                    ViewAction::None
                }
                KeyCode::Enter => {
                    ViewAction::EmitAndClose(ViewEvent::ProviderPickerExternalConsentRevoked {
                        provider: self.selected_provider(),
                    })
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
                // Keyboard and double-click share `advance_from_model_pick`
                // (Slice D) so both paths behave identically.
                KeyCode::Enter => self.advance_from_model_pick(),
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
                MouseEventKind::ScrollUp => {
                    self.last_list_mouse_selected = None;
                    self.move_up();
                }
                MouseEventKind::ScrollDown => {
                    self.last_list_mouse_selected = None;
                    self.move_down();
                }
                MouseEventKind::Moved => {
                    self.hovered_list_idx = self.list_hit_at(mouse);
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    return self.click_list_row(mouse);
                }
                _ => {}
            },
            Stage::ModelPick => match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.last_model_mouse_selected = None;
                    self.move_model_selection(-1);
                }
                MouseEventKind::ScrollDown => {
                    self.last_model_mouse_selected = None;
                    self.move_model_selection(1);
                }
                MouseEventKind::Moved => {
                    self.hovered_model_idx = self.model_hit_at(mouse);
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    return self.click_model_row(mouse);
                }
                _ => {}
            },
            Stage::ExternalConsentChoice => match mouse.kind {
                MouseEventKind::Moved => {
                    self.hovered_consent_idx = self.consent_hit_at(mouse);
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    self.click_consent_row(mouse);
                }
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
                    MouseEventKind::Moved => {
                        let pos = Position::new(mouse.column, mouse.row);
                        self.hovered_template_idx = self
                            .template_row_hitboxes
                            .borrow()
                            .iter()
                            .find_map(|(rect, idx)| rect.contains(pos).then_some(*idx));
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
            | Stage::ChatgptAuthChoice
            | Stage::KeyEntry
            | Stage::ExternalConsentConfirm
            | Stage::ExternalConsentRevokeConfirm
            | Stage::Confirm
            | Stage::CustomForm => {}
        }
        ViewAction::None
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let preferred_height = match self.stage {
            Stage::List => (self.rows.len() as u16).saturating_add(2),
            Stage::XaiAuthChoice => 12,
            Stage::ChatgptAuthChoice => 13,
            // Key/OAuth help is intentionally multi-line and wraps at narrow
            // widths. One shared height keeps every provider's final guidance
            // visible instead of special-casing whichever route clipped last.
            Stage::KeyEntry => 14,
            Stage::ExternalConsentChoice => 12,
            // The disclosure is deliberately long (route, owner, exact path,
            // semantics, custody, billing owner, revoke scope) and every line
            // wraps; it gets the height it needs so nothing is clipped.
            Stage::ExternalConsentConfirm => 20,
            Stage::ExternalConsentRevokeConfirm => 12,
            Stage::ModelPick => 12,
            Stage::PlanTier => 10,
            Stage::StepfunBillingRoute => 11,
            Stage::Confirm => 10,
            Stage::CustomForm => 12,
            // Catalog rows + descriptor detail (URL, env, default, docs).
            Stage::TemplateList => 18,
        };
        let popup_area = centered_modal_area(area, 120, preferred_height, 64, 8);

        render_modal_surface(area, popup_area, buf);

        match self.stage {
            Stage::List => self.render_list(popup_area, buf),
            Stage::XaiAuthChoice => self.render_xai_auth_choice(popup_area, buf),
            Stage::ChatgptAuthChoice => self.render_chatgpt_auth_choice(popup_area, buf),
            Stage::KeyEntry => self.render_key_entry(popup_area, buf),
            Stage::ExternalConsentChoice => self.render_external_consent_choice(popup_area, buf),
            Stage::ExternalConsentConfirm => self.render_external_consent_confirm(popup_area, buf),
            Stage::ExternalConsentRevokeConfirm => {
                self.render_external_consent_revoke_confirm(popup_area, buf)
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::has_api_key_for;
    use crate::test_support::EnvVarGuard;
    use crossterm::event::{KeyEvent, KeyModifiers};

    // Environment-mutating tests in this module hold the process-wide
    // `lock_test_env()` (via `crate::test_support`), the same barrier every
    // other module's env tests use. A module-private mutex cannot serialize
    // against the rest of the suite, so sibling tests raced on shared
    // provider env vars (EXAMPLE_API_KEY, OPENROUTER_API_KEY, ...) and a panic
    // while holding it cascaded PoisonError failures into unrelated tests.

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
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
    fn failed_live_catalog_refresh_names_the_working_fallback() {
        assert_eq!(
            catalog_freshness_title_suffix_for(ModelsDevFreshness::Failed),
            " · refresh failed; catalog available"
        );
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

        // Catalog surface: one identity per vendor (not dual-wire / plan kinds).
        assert_eq!(names.len(), ApiProvider::catalog().len());
        assert!(names.contains(&"DeepSeek"));
        assert!(names.contains(&"Alibaba Cloud Model Studio"));
        // Dialect is wire config — no second MiniMax / Model Studio rows.
        assert_eq!(
            names
                .iter()
                .filter(|name| name.contains("Alibaba Cloud Model Studio"))
                .count(),
            1
        );
        assert_eq!(names.iter().filter(|name| **name == "MiniMax").count(), 1);
        assert_eq!(names.iter().filter(|name| **name == "DeepSeek").count(), 1);

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
            "NVIDIA_API_KEY / NVIDIA_NIM_API_KEY"
        );
    }

    #[test]
    fn key_entry_hint_includes_provider_credential_url() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::NvidiaNim);
        picker.handle_key(key(KeyCode::Enter));

        let rendered = render_text(&picker, 120, 20);

        assert!(rendered.contains("NVIDIA_API_KEY / NVIDIA_NIM_API_KEY"));
        assert!(rendered.contains("https://build.nvidia.com/settings/api-keys"));
    }

    #[test]
    fn zai_key_entry_wraps_long_environment_guidance_without_hiding_credentials_url() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Zai);
        picker.handle_key(key(KeyCode::Enter));

        // Reproduce the width from the dogfood screenshot: the old renderer
        // allocated one row per logical line, so the long env-var sentence
        // clipped and displaced the credentials URL.
        let rendered = render_text(&picker, 100, 20);

        for name in [
            "ZAI_API_KEY",
            "Z_AI_API_KEY",
            "ZHIPU_API_KEY",
            "GLM_API_KEY",
        ] {
            assert!(rendered.contains(name), "missing {name}:\n{rendered}");
        }
        assert!(rendered.contains("re-open /provider."), "{rendered}");
        assert!(
            rendered.contains("Credentials: https://z.ai/model-api"),
            "{rendered}"
        );
    }

    #[test]
    fn kimi_key_entry_uses_the_direct_api_key_console_without_oauth_copy() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Moonshot);
        picker.handle_key(key(KeyCode::Enter));

        let rendered = render_text(&picker, 120, 20);

        assert!(rendered.contains("https://platform.kimi.ai/console/api-keys"));
        assert!(rendered.contains("paste key here"));
        assert!(!rendered.contains("OAuth"));
        assert!(!rendered.contains("device login"));
    }

    #[test]
    fn kimi_code_plan_key_entry_uses_membership_route_guidance() {
        let config = Config {
            provider: Some("moonshot".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                moonshot: crate::config::ProviderConfig {
                    base_url: Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                    model: Some(crate::config::KIMI_CODE_K3_MODEL.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut picker = ProviderPickerView::new(ApiProvider::Moonshot, &config);
        assert_eq!(picker.selected_provider(), ApiProvider::Moonshot);
        picker.handle_key(key(KeyCode::Enter));

        let rendered = render_text(&picker, 120, 24);

        assert!(rendered.contains("https://www.kimi.com/code/console"));
        assert!(rendered.contains("api.kimi.com/coding/v1"));
        assert!(rendered.contains("does not import Kimi CLI credentials"));
        assert!(!rendered.contains("https://platform.kimi.ai/console/api-keys"));
        assert!(!rendered.contains("OAuth"));
    }

    #[test]
    fn recovery_picker_keeps_active_route_and_esc_makes_no_change() {
        let config = Config {
            provider: Some("moonshot".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                moonshot: crate::config::ProviderConfig {
                    base_url: Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                    model: Some(crate::config::KIMI_CODE_K3_MODEL.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut picker = ProviderPickerView::new(ApiProvider::Moonshot, &config);

        assert_eq!(picker.stage, Stage::List);
        assert_eq!(picker.selected_provider(), ApiProvider::Moonshot);
        assert!(matches!(
            picker.handle_key(key(KeyCode::Esc)),
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerDismissed { .. })
        ));
        assert_eq!(config.provider.as_deref(), Some("moonshot"));
        assert_eq!(
            config
                .provider_config_for(ApiProvider::Moonshot)
                .and_then(|entry| entry.base_url.as_deref()),
            Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL)
        );
    }

    #[test]
    fn recovery_model_pick_restores_exact_kimi_code_k3_without_catalog_leakage() {
        let mut config = Config {
            provider: Some("moonshot".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                moonshot: crate::config::ProviderConfig {
                    base_url: Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                    model: Some(crate::config::KIMI_CODE_K3_MODEL.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let picker = ProviderPickerView::new_for_model_pick_after_validation(
            ApiProvider::Moonshot,
            ApiProvider::Moonshot,
            &config,
            None,
            "validated-key".to_string(),
            None,
        )
        .expect("Kimi route row");

        assert_eq!(picker.selected_model.as_deref(), Some("k3"));
        assert_eq!(
            picker
                .model_options
                .iter()
                .filter(|model| model.eq_ignore_ascii_case("k3"))
                .count(),
            1,
            "the current wire model must be appended once, case-insensitively"
        );

        config
            .providers
            .as_mut()
            .expect("providers")
            .moonshot
            .base_url = Some(crate::config::DEFAULT_MOONSHOT_BASE_URL.to_string());
        let generic = ProviderPickerView::new_for_model_pick_after_validation(
            ApiProvider::Moonshot,
            ApiProvider::Moonshot,
            &config,
            None,
            "validated-key".to_string(),
            None,
        )
        .expect("generic Moonshot row");
        assert!(
            !generic
                .model_options
                .iter()
                .any(|model| model.eq_ignore_ascii_case("k3")),
            "bare K3 stays route-local and must not be added to generic Moonshot"
        );
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
        assert_eq!(codex.stage, Stage::ChatgptAuthChoice);
        assert_eq!(codex.selected_provider(), ApiProvider::OpenaiCodex);
        let codex_text = render_text(&codex, 120, 20);
        assert!(codex_text.contains("Sign in with ChatGPT"), "{codex_text}");
        assert!(codex_text.contains("subscription"), "{codex_text}");
        assert!(codex_text.contains("openai"), "{codex_text}");
        assert!(codex_text.contains("API-key"), "{codex_text}");
        assert!(codex_text.contains("Import Codex CLI"), "{codex_text}");
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
        // Slice D: cost is model-level — a local model prices as local.
        assert_eq!(model_cost_label(ApiProvider::Ollama, "llama3"), "local");
        assert!(row.base_url.contains("localhost:11434"));
        assert!(row.is_active);
    }

    #[test]
    fn ollama_cloud_row_requires_credentials_and_is_not_labeled_local() {
        let _env_lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("isolated credential home");
        let _home = EnvVarGuard::set("CODEWHALE_HOME", temp.path());
        let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
        let _ollama_cloud_key = EnvVarGuard::remove("OLLAMA_CLOUD_API_KEY");
        let _ollama_key = EnvVarGuard::remove("OLLAMA_API_KEY");
        let _cli_source = EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let _cli_key = EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");

        let mut config = Config {
            provider: Some("ollama".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                ollama: crate::config::ProviderConfig {
                    base_url: Some(codewhale_config::provider::OLLAMA_CLOUD_BASE_URL.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        assert_eq!(config.api_provider(), ApiProvider::OllamaCloud);
        let missing = ProviderDashboardRow::from_config(
            ApiProvider::OllamaCloud,
            ApiProvider::OllamaCloud,
            &config,
        );
        assert_eq!(missing.auth_status, ProviderAuthStatus::Missing);
        assert_eq!(missing.readiness, ResolvedProviderReadiness::MissingKey);
        // Slice D: no provider-level cost leaks into the catalog hint.
        assert!(!missing.compact_hint().contains("cost:"));
        assert!(!missing.compact_hint().contains("(self-hosted)"));
        assert!(
            missing
                .messages
                .iter()
                .any(|message| message.contains("OLLAMA_API_KEY")),
            "missing Cloud key guidance: {:?}",
            missing.messages
        );

        config.providers.as_mut().expect("providers").ollama.api_key =
            Some("ollama-cloud-key".to_string());
        let configured = ProviderDashboardRow::from_config(
            ApiProvider::OllamaCloud,
            ApiProvider::OllamaCloud,
            &config,
        );
        assert_eq!(configured.auth_status, ProviderAuthStatus::Configured);
        assert_eq!(
            configured.readiness,
            ResolvedProviderReadiness::SavedUnchecked
        );
        assert!(!configured.compact_hint().contains("(self-hosted)"));
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
    fn provider_dashboard_row_surfaces_modelstudio_structured_thinking() {
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                modelstudio_token_plan: crate::config::ProviderConfig {
                    api_key: Some("modelstudio-key".to_string()),
                    model: Some("qwen3.8-max".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let row = ProviderDashboardRow::from_config(
            ApiProvider::ModelstudioTokenPlan,
            ApiProvider::ModelstudioTokenPlan,
            &config,
        );

        assert_eq!(row.reasoning.support, ProviderReasoningSupport::Supported);
        assert_eq!(
            row.reasoning.stream_visibility,
            ProviderReasoningStreamVisibility::StructuredThinking
        );
        assert!(row.compact_hint().contains("stream:structured"));
    }

    #[test]
    fn provider_dashboard_row_surfaces_kimi_code_k3_reasoning_only_on_exact_route() {
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                moonshot: crate::config::ProviderConfig {
                    api_key: Some("kimi-code-key".to_string()),
                    base_url: Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                    model: Some(crate::config::KIMI_CODE_K3_MODEL.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let row = ProviderDashboardRow::from_config(
            ApiProvider::Moonshot,
            ApiProvider::Moonshot,
            &config,
        );

        assert_eq!(
            row.default_route.wire_model,
            crate::config::KIMI_CODE_K3_MODEL
        );
        assert_eq!(row.reasoning.support, ProviderReasoningSupport::Supported);
        assert_eq!(
            row.reasoning.stream_visibility,
            ProviderReasoningStreamVisibility::StructuredThinking
        );
        assert_eq!(
            row.reasoning.controls,
            vec!["low".to_string(), "high".to_string(), "max".to_string()]
        );
        assert_eq!(
            row.capabilities.context_window,
            Some(262_144),
            "the picker must show the route-effective K3 baseline, not the generic fallback"
        );
        assert_eq!(
            row.capabilities.context_window_source.as_deref(),
            Some("static Kimi Code safe floor"),
            "the picker must name the provenance instead of presenting a bare limit as provider fact"
        );
        assert!(
            row.compact_hint()
                .contains("ctx:262K(static Kimi Code safe floor)"),
            "the compact picker receipt must retain context provenance"
        );

        let mut direct = config.clone();
        direct
            .providers
            .as_mut()
            .expect("providers")
            .moonshot
            .base_url = Some(crate::config::DEFAULT_MOONSHOT_BASE_URL.to_string());
        let direct_row = ProviderDashboardRow::from_config(
            ApiProvider::Moonshot,
            ApiProvider::Moonshot,
            &direct,
        );
        assert_ne!(
            direct_row.reasoning.support,
            ProviderReasoningSupport::Supported,
            "generic Moonshot k3 must not inherit Kimi Code's route-owned capability"
        );
        // The generic model-facts table now carries the same conservative
        // number for bare `k3`, so the route-ownership distinction lives in
        // provenance: the direct Moonshot row must never claim the Kimi Code
        // route-owned floor as its source.
        assert_ne!(
            direct_row.capabilities.context_window_source.as_deref(),
            Some("static Kimi Code safe floor")
        );
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
        let _lock = crate::test_support::lock_test_env();
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
        let _lock = crate::test_support::lock_test_env();
        let _example_key = EnvVarGuard::remove("EXAMPLE_API_KEY");
        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "my_thing".to_string(),
            crate::config::ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("https://api.example.com/v1".to_string()),
                model: Some("vendor/custom-model-v1".to_string()),
                api_key: Some(crate::config::API_KEYRING_SENTINEL.to_string()),
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
        let _lock = crate::test_support::lock_test_env();
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
    fn sensenova_preset_fills_published_openai_host() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('s'))),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::CustomForm);
        assert_eq!(picker.custom_provider_id, "sensenova");
        assert_eq!(
            picker.custom_provider_base_url,
            codewhale_config::SENSENOVA_BASE_URL
        );
        assert_eq!(
            picker.custom_provider_model,
            codewhale_config::SENSENOVA_DEFAULT_MODEL
        );
        assert_eq!(
            picker.custom_provider_api_key_env,
            codewhale_config::SENSENOVA_API_KEY_ENV
        );
        let rendered = render_text(&picker, 100, 20);
        assert!(rendered.contains("SenseNova"), "{rendered}");
        assert!(
            !rendered.contains("Custom provider"),
            "built-in template must not look like a blank custom form: {rendered}"
        );
    }

    #[test]
    fn p_opens_template_list_with_catalog_rows() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('p'))),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::TemplateList);
        let rendered = render_text(&picker, 100, 24);
        assert!(rendered.contains("OpenCode Zen"), "{rendered}");
        assert!(rendered.contains("OpenCode Go"), "{rendered}");
        assert!(rendered.contains("SenseNova"), "{rendered}");
        assert!(rendered.contains("Command Code"), "{rendered}");
        assert!(rendered.contains("Agnes"), "{rendered}");
        assert!(
            rendered.contains("no published") || rendered.contains("unpublished"),
            "{rendered}"
        );
        assert!(
            rendered.contains("https://opencode.ai/zen/v1"),
            "descriptor endpoint missing: {rendered}"
        );
        assert!(
            rendered.contains("https://opencode.ai/docs/zen/"),
            "{rendered}"
        );
        assert!(
            !rendered.contains("minimax-m2.7"),
            "template detail must not dump the Zen roster: {rendered}"
        );
    }

    #[test]
    fn template_list_enter_on_unpublished_agnes_does_not_invent_a_url() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new_for_template_setup(
            ApiProvider::Deepseek,
            "agnes",
            &config,
            None,
        )
        .expect("agnes template");
        assert_eq!(picker.stage, Stage::TemplateList);
        let action = picker.handle_key(key(KeyCode::Enter));
        match action {
            ViewAction::Emit(ViewEvent::StatusMessage { message }) => {
                assert!(
                    message.to_ascii_lowercase().contains("no published"),
                    "{message}"
                );
            }
            other => panic!("expected unpublished status, got {other:?}"),
        }
        assert!(picker.custom_provider_base_url.is_empty());
    }

    fn template_list_click(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Slice D hover rule: template rows are clickable, so hover must
    /// respond visibly without moving the keyboard selection.
    #[test]
    fn template_list_hover_tracks_pointer_without_moving_selection() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('p'))),
            ViewAction::None
        ));
        let area = Rect::new(0, 0, 100, 24);
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);
        let (rect, idx) = picker
            .template_row_hitboxes
            .borrow()
            .iter()
            .copied()
            .find(|(_, row_idx)| *row_idx != picker.template_selected_idx)
            .expect("a non-selected template row");
        let selected_before = picker.template_selected_idx;
        picker.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(picker.hovered_template_idx, Some(idx));
        assert_eq!(picker.template_selected_idx, selected_before);
    }

    /// Slice D two-pane picker at narrow widths: the provider strip stays on
    /// top and the priced models pane renders under it (stacked layout).
    /// Ollama carries no auth notes, so the pane fits the short detail area;
    /// note-heavy rows degrade by clipping, as detail panes always have.
    #[test]
    fn narrow_list_stage_stacks_models_pane_under_provider_strip() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Ollama);
        let rendered = render_text(&picker, 80, 52);
        assert!(
            rendered.contains("Models · $in/$out per mtok"),
            "{rendered}"
        );
        assert!(rendered.contains("(default)"), "{rendered}");
        assert!(rendered.contains("local"), "{rendered}");
        assert!(!rendered.contains("cost:"), "{rendered}");
    }

    #[test]
    fn template_list_mouse_selects_row_and_second_click_activates() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('p'))),
            ViewAction::None
        ));
        let area = Rect::new(0, 0, 100, 24);
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);
        let (rect, idx) = picker
            .template_row_hitboxes
            .borrow()
            .iter()
            .copied()
            .find(|(_, row_idx)| *row_idx == 2)
            .expect("SenseNova row hitbox");
        assert_eq!(
            provider_setup_templates()[idx].id,
            codewhale_config::SENSENOVA_TEMPLATE_ID
        );
        let click = template_list_click(rect.x, rect.y);
        assert!(matches!(picker.handle_mouse(click), ViewAction::None));
        assert_eq!(picker.template_selected_idx, idx);
        assert_eq!(picker.stage, Stage::TemplateList);
        picker.handle_mouse(click);
        assert_eq!(picker.stage, Stage::CustomForm);
        assert_eq!(picker.custom_provider_id, "sensenova");
        assert_eq!(
            picker.custom_provider_base_url,
            codewhale_config::SENSENOVA_BASE_URL
        );
    }

    #[test]
    fn template_list_mouse_second_click_on_unpublished_does_not_invent_a_url() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('p'))),
            ViewAction::None
        ));
        let area = Rect::new(0, 0, 100, 24);
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);
        let (rect, idx) = picker
            .template_row_hitboxes
            .borrow()
            .iter()
            .copied()
            .find(|(_, row_idx)| provider_setup_templates()[*row_idx].is_unpublished())
            .expect("Agnes row hitbox");
        let click = template_list_click(rect.x, rect.y);
        assert!(matches!(picker.handle_mouse(click), ViewAction::None));
        assert_eq!(picker.template_selected_idx, idx);
        match picker.handle_mouse(click) {
            ViewAction::Emit(ViewEvent::StatusMessage { message }) => {
                assert!(
                    message.to_ascii_lowercase().contains("no published"),
                    "{message}"
                );
            }
            other => panic!("expected unpublished status, got {other:?}"),
        }
        assert!(picker.custom_provider_base_url.is_empty());
        assert_eq!(picker.stage, Stage::TemplateList);
    }

    #[test]
    fn template_list_compact_40x12_keeps_selection_without_clipping() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('p'))),
            ViewAction::None
        ));

        for selected in [0usize, provider_setup_templates().len().saturating_sub(1)] {
            picker.template_selected_idx = selected;
            let area = Rect::new(0, 0, 40, 12);
            let mut buf = Buffer::empty(area);
            picker.render(area, &mut buf);
            let rendered = render_text(&picker, 40, 12);
            let selected_template = &provider_setup_templates()[selected];
            assert!(
                rendered.contains(selected_template.display_name),
                "40x12 must keep selected {} visible:\n{rendered}",
                selected_template.display_name
            );
            assert!(
                rendered.contains(crate::tui::glyphs::SELECTION),
                "40x12 must show the selection marker:\n{rendered}"
            );
            for (idx, line) in rendered.lines().enumerate() {
                assert!(
                    crate::tui::ui_text::text_display_width(line) <= 40,
                    "40x12 line {idx} clips: {line:?}\n{rendered}"
                );
            }
            let hitboxes = picker.template_row_hitboxes.borrow().clone();
            assert!(
                !hitboxes.is_empty(),
                "40x12 must register template hitboxes:\n{rendered}"
            );
            assert!(
                hitboxes.iter().any(|(_, idx)| *idx == selected),
                "40x12 hitboxes must include selected {selected}: {hitboxes:?}\n{rendered}"
            );
            for (rect, idx) in &hitboxes {
                assert!(
                    rect.y < 12 && rect.x < 40,
                    "hitbox for {idx} is outside 40x12: {rect:?}"
                );
                let row = (0..40)
                    .map(|x| buf[(x, rect.y)].symbol())
                    .collect::<String>();
                assert!(
                    row.contains(provider_setup_templates()[*idx].display_name)
                        || row.contains(provider_setup_templates()[*idx].id),
                    "40x12 hitbox y={} should map to {}: {row:?}",
                    rect.y,
                    provider_setup_templates()[*idx].display_name
                );
            }
        }
    }

    #[test]
    fn template_list_uses_locale_for_kinds_labels_and_guidance() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config)
            .with_locale(crate::localization::Locale::ZhHans);
        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('p'))),
            ViewAction::None
        ));
        let rendered = render_text(&picker, 100, 24);
        // TestBackend stores the continuation cell of each wide CJK glyph
        // as a space. Collapse whitespace for language-copy assertions
        // while retaining the original cell dump for English-leak checks.
        let compact: String = rendered.chars().filter(|ch| !ch.is_whitespace()).collect();
        assert!(rendered.contains("OpenCode Zen"), "{rendered}");
        assert!(rendered.contains("SenseNova"), "{rendered}");
        assert!(rendered.contains("Agnes"), "{rendered}");
        assert!(compact.contains("仅密钥"), "{rendered}");
        assert!(compact.contains("兼容"), "{rendered}");
        assert!(
            compact.contains("基础URL"),
            "localized Base URL label missing: {rendered}"
        );
        assert!(
            compact.contains("模型：") || compact.contains("模型:"),
            "localized Model label missing: {rendered}"
        );
        assert!(
            !rendered.contains("key-only"),
            "English kind leaked: {rendered}"
        );
        assert!(
            !rendered.contains("Base URL:"),
            "English Base URL leaked: {rendered}"
        );
        assert!(
            !rendered.contains("Create or copy an OpenCode Zen API key"),
            "English guidance leaked: {rendered}"
        );
        picker.template_selected_idx = provider_setup_templates()
            .iter()
            .position(|template| template.is_unpublished())
            .expect("agnes");
        let unpublished = render_text(&picker, 100, 24);
        let unpublished_compact: String = unpublished
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect();
        assert!(
            unpublished_compact.contains("未公布") || unpublished_compact.contains("没有"),
            "{unpublished}"
        );
        assert!(
            !unpublished.contains("unpublished"),
            "English unpublished kind leaked: {unpublished}"
        );
    }

    #[test]
    fn t_emits_test_connection_for_the_selected_row() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let expected_catalog = picker.view == ProviderListView::Catalog;
        let action = picker.handle_key(ctrl(KeyCode::Char('t')));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerTestConnection {
                provider,
                provider_id,
                catalog_view,
            }) => {
                assert_eq!(provider, picker.selected_provider());
                assert_eq!(provider_id, picker.selected_provider_id());
                assert_eq!(catalog_view, expected_catalog);
            }
            other => panic!("expected test-connection event, got {other:?}"),
        }
    }

    #[test]
    fn t_on_configured_view_does_not_force_the_full_catalog() {
        let _lock = crate::test_support::lock_test_env();
        let _key = crate::test_support::EnvVarGuard::set("DEEPSEEK_API_KEY", "sk-test");
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        assert_eq!(picker.view, ProviderListView::Configured);
        match picker.handle_key(ctrl(KeyCode::Char('t'))) {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerTestConnection {
                catalog_view,
                ..
            }) => {
                assert!(!catalog_view);
            }
            other => panic!("expected test-connection event, got {other:?}"),
        }
    }

    #[test]
    fn plain_t_stays_type_ahead_and_does_not_probe() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        picker.toggle_view();
        let action = picker.handle_key(key(KeyCode::Char('t')));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(picker.query, "t");
        assert_eq!(picker.stage, Stage::List);
    }

    #[test]
    fn lm_studio_preset_is_loopback_keyless_and_requests_the_loaded_model() {
        let config = Config::default();
        let mut picker =
            ProviderPickerView::new_for_setup(ApiProvider::Deepseek, None, &config, None);

        let rendered = render_text(&picker, 100, 28);
        assert!(rendered.contains("I LM Studio"), "{rendered}");
        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('i'))),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::CustomForm);
        assert_eq!(picker.custom_provider_field, CustomProviderField::Model);
        assert_eq!(picker.custom_provider_id, "lm_studio");
        assert_eq!(picker.custom_provider_base_url, "http://127.0.0.1:1234/v1");
        assert!(picker.custom_provider_model.is_empty());
        assert!(picker.custom_provider_api_key_env.is_empty());

        for ch in "local-code-model".chars() {
            picker.handle_key(key(KeyCode::Char(ch)));
        }
        picker.handle_key(key(KeyCode::Enter));
        let action = picker.handle_key(key(KeyCode::Enter));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerCustomProviderSubmitted {
                provider_id,
                base_url,
                model,
                api_key_env,
            }) => {
                assert_eq!(provider_id, "lm_studio");
                assert_eq!(base_url, "http://127.0.0.1:1234/v1");
                assert_eq!(model.as_deref(), Some("local-code-model"));
                assert_eq!(api_key_env, None);
            }
            other => panic!("expected LM Studio custom-provider submit event, got {other:?}"),
        }
    }

    #[test]
    fn ds4_preset_is_keyless_and_ready_to_save() {
        let mut picker =
            ProviderPickerView::new_for_ds4_setup(ApiProvider::Deepseek, &Config::default(), None);

        assert_eq!(picker.stage, Stage::CustomForm);
        assert_eq!(picker.custom_provider_id, "ds4");
        assert_eq!(picker.custom_provider_base_url, "http://127.0.0.1:8000/v1");
        assert_eq!(picker.custom_provider_model, "deepseek-v4-flash");
        assert!(picker.custom_provider_api_key_env.is_empty());

        match picker.handle_key(key(KeyCode::Enter)) {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerCustomProviderSubmitted {
                provider_id,
                base_url,
                model,
                api_key_env,
            }) => {
                assert_eq!(provider_id, "ds4");
                assert_eq!(base_url, "http://127.0.0.1:8000/v1");
                assert_eq!(model.as_deref(), Some("deepseek-v4-flash"));
                assert_eq!(api_key_env, None);
            }
            other => panic!("expected DS4 custom-provider submit event, got {other:?}"),
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
        let _lock = crate::test_support::lock_test_env();
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
        let _lock = crate::test_support::lock_test_env();
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

    /// The visible payoff of the sourced resolver. Before this change the row
    /// said "missing OPENROUTER_API_KEY" and nothing else, so a user could not
    /// tell whether the durable slot had been read and found empty, skipped, or
    /// never consulted. The note must now name the places that were probed and
    /// the command that fixes the first of them.
    #[test]
    fn missing_key_note_names_the_places_that_were_checked() {
        let _lock = crate::test_support::lock_test_env();
        let _openrouter_key = EnvVarGuard::remove("OPENROUTER_API_KEY");
        let config = Config::default();
        let row = ProviderDashboardRow::from_config(
            ApiProvider::Openrouter,
            ApiProvider::Deepseek,
            &config,
        );

        let note = row
            .messages
            .iter()
            .find(|message| message.contains("missing OPENROUTER_API_KEY"))
            .expect("missing-key note");
        assert!(
            note.contains("checked "),
            "the note must say where it looked: {note}"
        );
        assert!(
            note.contains("secret store \"openrouter\""),
            "the durable slot must be named: {note}"
        );
        assert!(
            note.contains("fix: "),
            "the note must offer an action: {note}"
        );
        assert!(
            !note.contains("sk-"),
            "a credential note must never carry key material: {note}"
        );
    }

    /// Every row states which place its credential came from, so a
    /// "key:configured" row can be reconciled with a request that used a
    /// different source.
    #[test]
    fn every_row_states_its_credential_source() {
        let _lock = crate::test_support::lock_test_env();
        let _openrouter_key = EnvVarGuard::remove("OPENROUTER_API_KEY");
        let config = Config::default();
        let missing = ProviderDashboardRow::from_config(
            ApiProvider::Openrouter,
            ApiProvider::Deepseek,
            &config,
        );
        assert_eq!(missing.credential_source, "not found");

        let _key = EnvVarGuard::set("OPENROUTER_API_KEY", "test-value");
        let configured = ProviderDashboardRow::from_config(
            ApiProvider::Openrouter,
            ApiProvider::Deepseek,
            &config,
        );
        assert_eq!(configured.credential_source, "OPENROUTER_API_KEY");
        assert!(
            !configured.credential_source.contains("test-value"),
            "the source is a label, never the value"
        );
    }

    #[test]
    fn modelstudio_family_key_marks_all_variants_configured() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let _home = crate::test_support::EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", tmp.path());
        let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", tmp.path());
        let _backend = crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
        let _ms_key = crate::test_support::EnvVarGuard::remove("MODELSTUDIO_API_KEY");
        let _dashscope_key = crate::test_support::EnvVarGuard::remove("DASHSCOPE_API_KEY");
        let _cli_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let _cli_key = crate::test_support::EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");

        // One saved key on the Token Plan variant, marked by the save path.
        codewhale_secrets::Secrets::auto_detect()
            .set("modelstudio-token-plan", "ms-family-key")
            .expect("seed family slot");
        let config = Config {
            provider: Some("deepseek".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                modelstudio_token_plan: crate::config::ProviderConfig {
                    auth_mode: Some("api_key".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };

        for variant in [
            ApiProvider::ModelstudioTokenPlan,
            ApiProvider::ModelstudioTokenPlanAnthropic,
            ApiProvider::ModelstudioCodingPlan,
            ApiProvider::ModelstudioCodingPlanAnthropic,
        ] {
            let row = ProviderDashboardRow::from_config(variant, ApiProvider::Deepseek, &config);
            assert_eq!(
                row.auth_status,
                ProviderAuthStatus::Configured,
                "{variant:?} must resolve the family's one saved key"
            );
        }
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

        let rendered = render_text(&picker, 124, 24);

        assert!(rendered.contains("key:configured"));
        assert!(!rendered.contains("auth:configured"));
        assert!(rendered.contains("Route: custom-model"));
        assert!(rendered.contains("chat"));
        // Slice D: provider detail carries no cost; the models pane does.
        assert!(!rendered.contains("cost:"));
        assert!(!rendered.contains("Usage:"));
        assert!(rendered.contains("Models · $in/$out per mtok"));
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
        let mut listed = picker
            .rows
            .iter()
            .map(|row| row.provider)
            .collect::<Vec<_>>();
        // With no configured custom providers, the catalog keeps the Custom
        // entry so a custom endpoint can still be created from setup. The
        // canonical universe is the user-facing catalog (one identity per
        // vendor): dual-wire dialects are `wire` config and plan variants are
        // `mode`/base_url, not picker rows.
        let mut expected = ApiProvider::catalog().to_vec();
        listed.sort_by_key(|provider| provider.as_str());
        expected.sort_by_key(|provider| provider.as_str());
        assert_eq!(
            listed, expected,
            "setup must use the canonical provider universe"
        );
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

    /// #4763: onboarding focuses the persisted route but must still open on
    /// the navigable list. Jumping straight into key/OAuth entry hid the
    /// provider catalog from returning users with a missing key.
    #[test]
    fn onboarding_catalog_focuses_missing_provider_without_leaving_the_list() {
        let _lock = crate::test_support::lock_test_env();
        let _anthropic_key = crate::test_support::EnvVarGuard::remove("ANTHROPIC_API_KEY");
        let config = Config::default();
        let picker = ProviderPickerView::new_for_onboarding(
            ApiProvider::Deepseek,
            Some(ApiProvider::Anthropic),
            &config,
            None,
        );

        assert_eq!(picker.stage, Stage::List);
        assert_eq!(picker.view, ProviderListView::Catalog);
        assert_eq!(picker.selected_provider(), ApiProvider::Anthropic);
        assert_eq!(
            picker.visible_row_count(),
            picker.rows.len(),
            "onboarding must show the whole provider catalog"
        );
    }

    #[test]
    fn first_run_onboarding_shows_all_providers_including_hosted() {
        let _lock = crate::test_support::lock_test_env();
        let config = Config::default();
        let picker =
            ProviderPickerView::new_for_onboarding(ApiProvider::Deepseek, None, &config, None);

        // First-run setup opens on the full catalog with the active provider
        // (DeepSeek) selected — hosted APIs are visible immediately, not
        // hidden behind a keypress (#n3onr1ft feedback, 2026-08-23).
        assert_eq!(picker.stage, Stage::List);
        assert_eq!(picker.view, ProviderListView::Catalog);
        assert_eq!(picker.selected_provider(), ApiProvider::Deepseek);

        let visible = picker
            .filtered_rows()
            .into_iter()
            .map(|(_, row)| row.provider)
            .collect::<Vec<_>>();
        assert!(!visible.is_empty());
        // Hosted providers AND local runtimes are both present up front.
        assert!(visible.contains(&ApiProvider::Deepseek));
        assert!(visible.contains(&ApiProvider::Ollama));
        assert!(
            visible.iter().any(|provider| !provider.is_self_hosted()),
            "hosted providers must be visible on first run: {visible:?}"
        );

        let rendered = render_text(&picker, 40, 12);
        assert!(
            rendered.contains(crate::tui::glyphs::SELECTION),
            "{rendered}"
        );
        for (idx, line) in rendered.lines().enumerate() {
            assert!(
                crate::tui::ui_text::text_display_width(line) <= 40,
                "40x12 line {idx} clips: {line:?}\n{rendered}"
            );
        }
    }

    #[test]
    fn local_shortcut_filters_cloud_rows_from_the_catalog() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new_for_onboarding(
            ApiProvider::Deepseek,
            Some(ApiProvider::Deepseek),
            &config,
            None,
        );
        assert_eq!(picker.view, ProviderListView::Catalog);

        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('l'))),
            ViewAction::None
        ));
        assert_eq!(picker.view, ProviderListView::Local);
        assert_eq!(picker.selected_provider(), ApiProvider::Ollama);
        assert!(
            picker
                .filtered_rows()
                .into_iter()
                .all(|(_, row)| row.provider.is_self_hosted())
        );
    }

    #[test]
    fn onboarding_catalog_honors_typed_credentials_for_every_builtin_provider() {
        use codewhale_config::provider::CredentialAcquisition;

        let _global_env = crate::test_support::lock_test_env();
        let home = tempfile::tempdir().expect("isolated provider catalog home");
        let _home = EnvVarGuard::set("HOME", home.path().to_string_lossy().as_ref());
        let _codewhale_home =
            EnvVarGuard::set("CODEWHALE_HOME", home.path().to_string_lossy().as_ref());
        let _codex_home = EnvVarGuard::set("CODEX_HOME", home.path().to_string_lossy().as_ref());
        let _secret_backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
        let mut key_envs = ApiProvider::all()
            .iter()
            .flat_map(|provider| provider.env_vars().iter().copied())
            .collect::<Vec<_>>();
        key_envs.sort_unstable();
        key_envs.dedup();
        let _missing_keys = key_envs
            .into_iter()
            .map(EnvVarGuard::remove)
            .collect::<Vec<_>>();
        let config = Config::default();

        // Every provider the catalog actually lists. Hidden dual-wire/plan
        // variants share their vendor primary's row and credential metadata,
        // so `ApiProvider::all()` cannot be driven through the visible list.
        for provider in ApiProvider::catalog().iter().copied() {
            let mut picker = ProviderPickerView::new_for_onboarding(
                ApiProvider::Deepseek,
                Some(provider),
                &config,
                None,
            );
            assert_eq!(picker.selected_provider(), provider, "{provider:?}");
            assert_eq!(picker.stage, Stage::List, "{provider:?}");

            let action = picker.handle_key(key(KeyCode::Enter));
            match provider.credential_help().acquisition {
                CredentialAcquisition::ApiKey => {
                    assert!(matches!(action, ViewAction::None), "{provider:?}");
                    assert!(
                        matches!(picker.stage, Stage::KeyEntry | Stage::StepfunBillingRoute),
                        "{provider:?} entered {:?}",
                        picker.stage
                    );
                }
                CredentialAcquisition::ApiKeyOrOAuth => {
                    assert_eq!(provider, ApiProvider::Xai, "{provider:?}");
                    assert!(matches!(action, ViewAction::None), "{provider:?}");
                    let choices = render_text(&picker, 80, 24);
                    assert!(choices.contains("API key"), "{choices}");
                    assert!(choices.contains("device OAuth"), "{choices}");

                    // Choice 1 is an ordinary API-key path. Text remains a key;
                    // it is never reinterpreted as an OAuth bearer token.
                    assert!(matches!(
                        picker.handle_key(key(KeyCode::Char('1'))),
                        ViewAction::None
                    ));
                    assert!(matches!(
                        picker.handle_key(key(KeyCode::Enter)),
                        ViewAction::None
                    ));
                    assert_eq!(picker.stage, Stage::KeyEntry);
                    for ch in "violet-".chars() {
                        picker.handle_key(key(KeyCode::Char(ch)));
                    }
                    assert!(picker.handle_paste("otter-key"));
                    let key_text = "violet-otter-key";
                    assert_eq!(picker.api_key_input, key_text);
                    for (width, height) in [(80, 24), (120, 32)] {
                        let rendered = render_text(&picker, width, height);
                        assert!(!rendered.contains(key_text), "{width}x{height}: {rendered}");
                        assert!(rendered.contains('*'), "{width}x{height}: {rendered}");
                    }
                    assert!(matches!(
                        picker.handle_key(key(KeyCode::Enter)),
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerApiKeySubmitted {
                            provider: ApiProvider::Xai,
                            provider_id: None,
                            api_key,
                            base_url: None,
                        }) if api_key == key_text
                    ));

                    // Choice 2 is the provider-native device flow and emits only
                    // the request event; the picker never manufactures a token.
                    let mut oauth = ProviderPickerView::new_for_onboarding(
                        ApiProvider::Deepseek,
                        Some(ApiProvider::Xai),
                        &config,
                        None,
                    );
                    assert!(matches!(
                        oauth.handle_key(key(KeyCode::Enter)),
                        ViewAction::None
                    ));
                    assert!(matches!(
                        oauth.handle_key(key(KeyCode::Char('2'))),
                        ViewAction::None
                    ));
                    assert!(matches!(
                        oauth.handle_key(key(KeyCode::Enter)),
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerXaiOAuthRequested)
                    ));
                }
                CredentialAcquisition::LocalOptional => assert!(matches!(
                    action,
                    ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied {
                        provider: applied,
                        provider_id: None,
                    }) if applied == provider
                )),
                CredentialAcquisition::OAuth => {
                    assert!(matches!(action, ViewAction::None), "{provider:?}");
                    if provider == ApiProvider::OpenaiCodex {
                        assert_eq!(picker.stage, Stage::ChatgptAuthChoice, "{provider:?}");
                        assert!(!picker.handle_paste("fixture-oauth-paste"));
                        assert!(
                            picker.api_key_input.is_empty(),
                            "{provider:?} must reject key paste"
                        );
                        assert!(matches!(
                            picker.handle_key(key(KeyCode::Enter)),
                            ViewAction::EmitAndClose(
                                ViewEvent::ProviderPickerChatgptOAuthRequested
                            )
                        ));
                    } else {
                        assert_eq!(picker.stage, Stage::KeyEntry, "{provider:?}");
                        assert!(picker.handle_paste("fixture-oauth-paste"));
                        assert!(
                            picker.api_key_input.is_empty(),
                            "{provider:?} must reject key paste"
                        );
                    }
                }
                CredentialAcquisition::Configuration => {
                    assert_eq!(provider, ApiProvider::Custom);
                    assert!(matches!(action, ViewAction::None));
                    assert_eq!(picker.stage, Stage::CustomForm);
                    assert!(picker.api_key_input.is_empty());
                }
            }
        }
    }

    #[test]
    fn credential_draft_is_masked_and_escape_drops_it_without_persistence() {
        let _global_env = crate::test_support::lock_test_env();
        let home = tempfile::tempdir().expect("isolated credential draft home");
        let _home = EnvVarGuard::set("HOME", home.path().to_string_lossy().as_ref());
        let _codewhale_home =
            EnvVarGuard::set("CODEWHALE_HOME", home.path().to_string_lossy().as_ref());
        let _secret_backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
        let _openrouter_key = EnvVarGuard::remove("OPENROUTER_API_KEY");
        let config = Config::default();
        let draft = ["violet", "otter", "draft", "7361"].join("-");
        let mut picker = ProviderPickerView::new_for_missing_auth(
            ApiProvider::Deepseek,
            ApiProvider::Openrouter,
            &config,
            None,
        )
        .expect("OpenRouter key editor");

        let ctrl_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert!(matches!(picker.handle_key(ctrl_v), ViewAction::None));
        assert!(
            picker.api_key_input.is_empty(),
            "shortcut must not type `v`"
        );
        let shifted_v = KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT);
        assert!(matches!(picker.handle_key(shifted_v), ViewAction::None));
        assert_eq!(
            picker.api_key_input, "V",
            "shifted credential text is valid"
        );
        assert!(matches!(
            picker.handle_key(key(KeyCode::Backspace)),
            ViewAction::None
        ));
        assert!(picker.handle_paste(&draft));
        assert_eq!(picker.api_key_input, draft);
        for (width, height) in [(80, 24), (120, 32)] {
            let rendered = render_text(&picker, width, height);
            assert!(!rendered.contains(&draft), "{width}x{height}: {rendered}");
            assert!(rendered.contains('*'), "{width}x{height}: {rendered}");
        }

        assert!(matches!(
            picker.handle_key(key(KeyCode::Esc)),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::List);
        assert!(picker.api_key_input.is_empty());
        assert_eq!(
            std::fs::read_dir(home.path())
                .expect("isolated home remains readable")
                .count(),
            0,
            "Esc must not create config or credential-backend files"
        );
    }

    /// #4763: Escape backs out one stage at a time — key entry returns to the
    /// list, and only the list dismisses the picker.
    #[test]
    fn onboarding_escape_walks_key_entry_back_to_the_list_then_dismisses() {
        let _lock = crate::test_support::lock_test_env();
        let _anthropic_key = crate::test_support::EnvVarGuard::remove("ANTHROPIC_API_KEY");
        let config = Config::default();
        let mut picker = ProviderPickerView::new_for_onboarding(
            ApiProvider::Deepseek,
            Some(ApiProvider::Anthropic),
            &config,
            None,
        );
        assert_eq!(picker.stage, Stage::List);

        picker.enter_key_entry();
        assert_eq!(picker.stage, Stage::KeyEntry);

        assert!(matches!(
            picker.handle_key(key(KeyCode::Esc)),
            ViewAction::None
        ));
        assert_eq!(
            picker.stage,
            Stage::List,
            "Escape from key entry returns to the provider list"
        );

        assert!(
            matches!(
                picker.handle_key(key(KeyCode::Esc)),
                ViewAction::EmitAndClose(ViewEvent::ProviderPickerDismissed { .. })
            ),
            "Escape from the list dismisses the picker"
        );
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
            None,
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
            None,
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
                ..
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
    fn exact_kimi_code_setup_asks_for_plan_and_emits_selected_context_window() {
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                moonshot: crate::config::ProviderConfig {
                    base_url: Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                    model: Some(crate::config::KIMI_CODE_K3_MODEL.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut picker = ProviderPickerView::new_for_model_pick_after_validation(
            ApiProvider::Deepseek,
            ApiProvider::Moonshot,
            &config,
            None,
            "sk-kimi-plan".to_string(),
            None,
        )
        .expect("Moonshot has a picker row");

        assert_eq!(picker.stage, Stage::ModelPick);
        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::PlanTier);
        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('2'))),
            ViewAction::None
        ));
        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::Confirm);
        match picker.handle_key(key(KeyCode::Enter)) {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerSetupConfirmed {
                context_window,
                model,
                ..
            }) => {
                assert_eq!(model, crate::config::KIMI_CODE_K3_MODEL);
                assert_eq!(context_window, Some(1_048_576));
            }
            other => panic!("expected Kimi Code setup confirmation, got {other:?}"),
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
            None,
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

    fn stepfun_config(base_url: Option<&str>) -> Config {
        Config {
            providers: Some(crate::config::ProvidersConfig {
                stepfun: crate::config::ProviderConfig {
                    base_url: base_url.map(str::to_string),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// #4526: StepFun's two billing tracks are two endpoints. Setup asks which
    /// one the key belongs to, and the choice reaches key entry as a pending —
    /// not yet persisted — endpoint.
    #[test]
    fn stepfun_setup_asks_for_billing_route_before_key_entry() {
        let config = stepfun_config(None);
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Stepfun);

        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('r'))),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::StepfunBillingRoute);
        assert_eq!(
            picker.stepfun_billing_route,
            StepfunBillingRoute::PayAsYouGo
        );

        // The endpoints are the whole difference between the two tracks, so
        // both have to be legible at the narrow terminal size too.
        for (w, h) in [(80u16, 24u16), (120u16, 32u16)] {
            let rendered = render_text(&picker, w, h);
            assert!(
                rendered.contains(crate::config::DEFAULT_STEPFUN_BASE_URL)
                    && rendered.contains(crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL),
                "{w}x{h} must show both StepFun endpoints:\n{rendered}"
            );
            for (idx, line) in rendered.lines().enumerate() {
                assert!(
                    crate::tui::ui_text::text_display_width(line) <= w as usize,
                    "{w}x{h} billing-route line {idx} overflows: {line:?}"
                );
            }
        }

        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('2'))),
            ViewAction::None
        ));
        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::KeyEntry);
        assert_eq!(
            picker.pending_base_url.as_deref(),
            Some(crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL)
        );
    }

    /// The chosen endpoint rides on the key-submit event so the live check in
    /// `ui.rs` probes the Step Plan route, not the pay-as-you-go default.
    #[test]
    fn stepfun_plan_choice_travels_with_the_key_for_validation() {
        let config = stepfun_config(None);
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Stepfun);
        picker.handle_key(key(KeyCode::Char('r')));
        picker.handle_key(key(KeyCode::Char('2')));
        picker.handle_key(key(KeyCode::Enter));
        for c in "step-plan-key".chars() {
            picker.handle_key(key(KeyCode::Char(c)));
        }

        match picker.handle_key(key(KeyCode::Enter)) {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApiKeySubmitted {
                provider,
                api_key,
                base_url,
                ..
            }) => {
                assert_eq!(provider, ApiProvider::Stepfun);
                assert_eq!(api_key, "step-plan-key");
                assert_eq!(
                    base_url.as_deref(),
                    Some(crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL)
                );
            }
            other => panic!("expected ProviderPickerApiKeySubmitted, got {other:?}"),
        }
    }

    /// Confirm carries exactly the validated endpoint, and nothing else about
    /// the route, so the handler writes only `[providers.stepfun] base_url`.
    #[test]
    fn stepfun_confirm_emits_only_the_validated_endpoint() {
        let config = stepfun_config(None);
        let mut picker = ProviderPickerView::new_for_model_pick_after_validation(
            ApiProvider::Deepseek,
            ApiProvider::Stepfun,
            &config,
            None,
            "step-plan-key".to_string(),
            Some(crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL.to_string()),
        )
        .expect("StepFun has a picker row");

        assert_eq!(picker.stage, Stage::ModelPick);
        picker.handle_key(key(KeyCode::Enter));
        assert_eq!(picker.stage, Stage::Confirm);
        match picker.handle_key(key(KeyCode::Enter)) {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerSetupConfirmed {
                provider,
                base_url,
                context_window,
                ..
            }) => {
                assert_eq!(provider, ApiProvider::Stepfun);
                assert_eq!(
                    base_url.as_deref(),
                    Some(crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL)
                );
                assert_eq!(context_window, None);
            }
            other => panic!("expected ProviderPickerSetupConfirmed, got {other:?}"),
        }
    }

    /// A hand-configured StepFun endpoint is a deliberate choice. The wizard
    /// skips the billing-route stage entirely and emits no endpoint, so the
    /// custom value is never silently rewritten (#4526).
    #[test]
    fn stepfun_custom_base_url_survives_the_wizard_untouched() {
        let custom = "https://stepfun.internal.example/v1";
        let config = stepfun_config(Some(custom));
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Stepfun);
        assert_eq!(picker.rows[picker.selected_idx].base_url, custom);

        picker.handle_key(key(KeyCode::Char('r')));
        assert_eq!(picker.stage, Stage::KeyEntry);
        assert_eq!(picker.pending_base_url, None);
        assert_eq!(picker.rows[picker.selected_idx].base_url, custom);

        for c in "custom-key".chars() {
            picker.handle_key(key(KeyCode::Char(c)));
        }
        match picker.handle_key(key(KeyCode::Enter)) {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApiKeySubmitted {
                base_url, ..
            }) => assert_eq!(base_url, None, "custom endpoint must not be rewritten"),
            other => panic!("expected ProviderPickerApiKeySubmitted, got {other:?}"),
        }
    }

    /// A StepFun route already on Step Plan re-opens preselected there rather
    /// than defaulting the user back onto pay-as-you-go.
    #[test]
    fn stepfun_plan_route_reopens_preselected() {
        let config = stepfun_config(Some(crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL));
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Stepfun);
        picker.handle_key(key(KeyCode::Char('r')));

        assert_eq!(picker.stage, Stage::StepfunBillingRoute);
        assert_eq!(picker.stepfun_billing_route, StepfunBillingRoute::StepPlan);
    }

    /// #4526: OpenCode Go (subscription allowance) and OpenCode Zen
    /// (pay-as-you-go) are separate billing tracks and must not present as the
    /// same generic meter. Slice D: the distinction now lives on the
    /// per-model cost label, not on a provider row.
    #[test]
    fn opencode_go_and_zen_read_as_distinct_billing_tracks() {
        let token = PricingSku::Token {
            input_per_mtok: Some(1.0),
            output_per_mtok: Some(2.0),
        };
        let go = model_cost_label_for_pricing(ApiProvider::OpencodeGo, Some(&token));
        let zen = model_cost_label_for_pricing(ApiProvider::OpencodeZen, Some(&token));
        assert_ne!(go, zen);
        assert_eq!(go, "plan", "Go label was {go:?}");
        assert_eq!(zen, "$1.00/$2.00 mtok", "Zen label was {zen:?}");
        assert_ne!(
            go,
            model_cost_label_for_pricing(ApiProvider::Openrouter, None)
        );

        // Go never reports catalog token prices: its allowance is not spend.
        assert_eq!(model_cost_label(ApiProvider::OpencodeGo, "some-model"), go);
    }

    /// Slice D: per-model $/mtok in/out from the catalog, with honest
    /// non-token fallbacks and never a fabricated rate.
    #[test]
    fn model_cost_label_spells_out_mtok_in_and_out() {
        let token = PricingSku::Token {
            input_per_mtok: Some(1.5),
            output_per_mtok: Some(6.0),
        };
        assert_eq!(
            model_cost_label_for_pricing(ApiProvider::Deepseek, Some(&token)),
            "$1.50/$6.00 mtok"
        );
        // Partial token pricing never fabricates the missing leg.
        let partial = PricingSku::Token {
            input_per_mtok: Some(1.5),
            output_per_mtok: None,
        };
        assert_eq!(
            model_cost_label_for_pricing(ApiProvider::Deepseek, Some(&partial)),
            "token-priced"
        );
        assert_eq!(
            model_cost_label_for_pricing(
                ApiProvider::Deepseek,
                Some(&PricingSku::SubscriptionQuota {
                    used_pct: None,
                    resets_at: None,
                }),
            ),
            "plan"
        );
        assert_eq!(
            model_cost_label_for_pricing(
                ApiProvider::Deepseek,
                Some(&PricingSku::AccountCredits { balance: None }),
            ),
            "credits"
        );
        assert_eq!(
            model_cost_label_for_pricing(
                ApiProvider::Deepseek,
                Some(&PricingSku::LocalOrNotApplicable),
            ),
            "local"
        );
        // Unknown pricing falls back honestly per provider posture.
        assert_eq!(
            model_cost_label_for_pricing(ApiProvider::Ollama, None),
            "local"
        );
        assert_eq!(
            model_cost_label_for_pricing(ApiProvider::OpenaiCodex, None),
            "oauth quota"
        );
        assert_eq!(
            model_cost_label_for_pricing(ApiProvider::Deepseek, None),
            "price ?"
        );
    }

    /// Slice D two-pane picker: the models pane leads with the default route
    /// and every row carries a non-empty cost label.
    #[test]
    fn provider_pane_models_lead_with_default_route() {
        let config = Config::default();
        let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let row = picker
            .rows
            .iter()
            .find(|row| row.provider == ApiProvider::Deepseek)
            .expect("DeepSeek has a picker row");
        let models = provider_pane_models(row, 8);
        assert!(!models.is_empty(), "models pane must never render empty");
        assert!(models.len() <= 8);
        let (first, _, first_default) = &models[0];
        assert!(
            *first_default,
            "default route model must sort first, got {first:?}"
        );
        assert!(
            first.eq_ignore_ascii_case(&row.default_route.logical_model)
                || first.eq_ignore_ascii_case(&row.default_route.wire_model),
            "first pane model {first:?} is not the default route"
        );
        for (model, price, _) in &models {
            assert!(!price.trim().is_empty(), "model {model:?} needs a price");
        }
    }

    /// Slice D: the list stage pairs the provider strip with a models pane —
    /// no provider-level cost, per-model $/mtok beside it.
    #[test]
    fn list_stage_pairs_provider_strip_with_priced_models_pane() {
        let config = Config::default();
        let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let rendered = render_text(&picker, 124, 24);
        assert!(
            rendered.contains("Models · $in/$out per mtok"),
            "{rendered}"
        );
        assert!(rendered.contains("(default)"), "{rendered}");
        assert!(!rendered.contains("cost:"), "{rendered}");
        assert!(!rendered.contains("Usage:"), "{rendered}");
    }

    /// Slice D: provider-strip rows are clickable and hover visibly without
    /// disturbing the keyboard selection.
    #[test]
    fn provider_strip_rows_are_clickable_and_hoverable() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Ollama);
        // Render first so this frame's hitboxes exist.
        let _ = render_text(&picker, 120, 24);
        assert!(
            !picker.list_row_hitboxes.borrow().is_empty(),
            "list rows must record hitboxes"
        );
        let ollama_idx = picker
            .rows
            .iter()
            .position(|row| row.provider == ApiProvider::Ollama)
            .expect("Ollama has a picker row");
        let ollama_hit = picker
            .list_row_hitboxes
            .borrow()
            .iter()
            .find(|(_, idx)| *idx == ollama_idx)
            .map(|(rect, _)| *rect)
            .expect("Ollama row must be visible");
        let other_hit = picker
            .list_row_hitboxes
            .borrow()
            .iter()
            .find(|(_, idx)| *idx != ollama_idx)
            .map(|(rect, idx)| (*rect, *idx))
            .expect("a second visible row is needed");
        // Hover tracks the pointer and leaves the selection alone.
        let selected_before = picker.selected_idx;
        picker.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: other_hit.0.x,
            row: other_hit.0.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(picker.hovered_list_idx, Some(other_hit.1));
        assert_eq!(picker.selected_idx, selected_before);
        // Moving off every row clears the hover.
        picker.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        // Single click selects; a second click activates like Enter.
        // (Ollama needs no key, so activation applies immediately.)
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: ollama_hit.x,
            row: ollama_hit.y,
            modifiers: KeyModifiers::NONE,
        };
        assert!(matches!(picker.handle_mouse(click), ViewAction::None));
        assert_eq!(picker.selected_idx, ollama_idx);
        match picker.handle_mouse(click) {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied { provider, .. }) => {
                assert_eq!(provider, ApiProvider::Ollama);
            }
            other => panic!("double-click must apply Ollama, got {other:?}"),
        }
    }

    /// Slice D: model-pick rows carry per-model cost and are clickable.
    #[test]
    fn model_pick_rows_show_per_model_cost_and_click_selects() {
        let config = Config::default();
        // Codex prices deterministically off-catalog ("oauth quota").
        let mut picker = ProviderPickerView::new_for_model_pick_after_validation(
            ApiProvider::Deepseek,
            ApiProvider::OpenaiCodex,
            &config,
            None,
            "[REDACTED]".to_string(),
            None,
        )
        .expect("Codex has a picker row");
        assert_eq!(picker.stage, Stage::ModelPick);
        let rendered = render_text(&picker, 100, 24);
        assert!(rendered.contains("oauth quota"), "{rendered}");
        assert!(
            !picker.model_row_hitboxes.borrow().is_empty(),
            "model rows must record hitboxes"
        );
        let target = picker.model_row_hitboxes.borrow()[0];
        picker.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: target.0.x,
            row: target.0.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(picker.model_selected_idx, target.1);
    }

    /// Slice D explicit-consent gate: Gate 1 chooses (clickable), Gate 2
    /// discloses, and a click alone never grants anything.
    #[test]
    fn external_consent_gate_chooses_then_discloses() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Xai);
        picker.handle_key(key(KeyCode::Char('e')));
        assert_eq!(picker.stage, Stage::ExternalConsentChoice);
        let rendered = render_text(&picker, 100, 24);
        assert!(rendered.contains("Gate 1 of 2"), "{rendered}");
        assert_eq!(picker.consent_row_hitboxes.borrow().len(), 3);
        // Click the read-only option: chosen, not granted.
        let readonly_hit = picker.consent_row_hitboxes.borrow()[1];
        picker.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: readonly_hit.0.x,
            row: readonly_hit.0.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            picker.external_consent_choice,
            ExternalConsentChoice::ReadOnly
        );
        assert_eq!(picker.stage, Stage::ExternalConsentChoice);
        // Enter advances to the Gate 2 disclosure.
        picker.handle_key(key(KeyCode::Enter));
        assert_eq!(picker.stage, Stage::ExternalConsentConfirm);
        let confirm = render_text(&picker, 100, 24);
        assert!(confirm.contains("Gate 2 of 2"), "{confirm}");
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
            None,
        )
        .expect("OpenRouter has a picker row");
        let mut confirm = ProviderPickerView::new_for_model_pick_after_validation(
            ApiProvider::Deepseek,
            ApiProvider::Openrouter,
            &config,
            None,
            "sk-validated-key".to_string(),
            None,
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
    fn configured_api_key_editors_acknowledge_saved_credentials_across_providers() {
        for (provider, config, secret) in [
            (
                ApiProvider::Zai,
                Config {
                    providers: Some(crate::config::ProvidersConfig {
                        zai: crate::config::ProviderConfig {
                            api_key: Some("stored-zai-key".to_string()),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                    ..Config::default()
                },
                "stored-zai-key",
            ),
            (
                ApiProvider::Openrouter,
                Config {
                    providers: Some(crate::config::ProvidersConfig {
                        openrouter: crate::config::ProviderConfig {
                            api_key: Some("stored-openrouter-key".to_string()),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                    ..Config::default()
                },
                "stored-openrouter-key",
            ),
        ] {
            let mut picker = ProviderPickerView::new(provider, &config);
            move_to_provider(&mut picker, provider);
            picker.handle_key(key(KeyCode::Char('r')));

            let rendered = render_text(&picker, 100, 20);

            assert!(
                rendered.contains("Saved credential configured"),
                "{provider:?}:\n{rendered}"
            );
            assert!(rendered.contains("stored credential"), "{rendered}");
            assert!(rendered.contains("replace saved key"), "{rendered}");
            assert!(rendered.contains("keep current key"), "{rendered}");
            assert!(!rendered.contains("paste key here"), "{rendered}");
            assert!(!rendered.contains(secret), "{rendered}");
        }
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
                base_url,
            }) => {
                assert_eq!(provider, ApiProvider::Novita);
                assert_eq!(provider_id, None);
                assert_eq!(api_key, "novita-key");
                assert_eq!(base_url, None);
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
        assert_eq!(picker.stage, Stage::ChatgptAuthChoice);

        let rendered = render_text(&picker, 96, 20);
        assert!(rendered.contains("Sign in with ChatGPT"), "{rendered}");
        assert!(rendered.contains("subscription"), "{rendered}");
        assert!(rendered.contains("billing"), "{rendered}");
        assert!(rendered.contains("Import Codex CLI"), "{rendered}");
        assert!(!rendered.contains("save & switch"));
        assert!(!rendered.contains("(paste key here)"));
        assert!(!rendered.contains("Credentials:"));

        assert!(!picker.handle_paste("codex-token"));
        assert!(picker.api_key_input.is_empty());
        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerChatgptOAuthRequested)
        ));

        let mut picker = ProviderPickerView::new_for_missing_auth(
            ApiProvider::Deepseek,
            ApiProvider::OpenaiCodex,
            &config,
            None,
        )
        .expect("OpenAI Codex has a picker row");
        picker.handle_key(key(KeyCode::Char('2')));
        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::ExternalConsentChoice);
        let choices = render_text(&picker, 100, 20);
        assert!(choices.contains("Disabled (default)"), "{choices}");
        assert!(
            choices.contains("Use external CLI credentials (read-only)"),
            "{choices}"
        );
        assert!(choices.contains("Managed (unavailable)"), "{choices}");
        // #5772: the choice stage must not disclose the candidate path; only
        // the confirmation step may.
        assert!(!choices.contains("Exact resolved path:"), "{choices}");

        picker.handle_key(key(KeyCode::Char('2')));
        picker.handle_key(key(KeyCode::Enter));
        assert_eq!(picker.stage, Stage::ExternalConsentConfirm);
        let confirm = render_text(&picker, 120, 22);
        assert!(confirm.contains("Owning CLI: Codex CLI"), "{confirm}");
        assert!(confirm.contains("Exact resolved path:"), "{confirm}");
        assert!(confirm.contains("Route:"), "{confirm}");
        assert!(confirm.contains("local device only"), "{confirm}");
        assert!(confirm.contains("billing owner"), "{confirm}");
        assert!(confirm.contains("no refresh, identity-provider or discovery requests"));
        assert!(confirm.contains("normal requests to the selected provider"));
        assert!(
            confirm.contains("clears only Codewhale's consent record"),
            "{confirm}"
        );
        assert!(confirm.contains("external-revoke --provider openai-codex"));
        // Confirmation is escapable without any event being emitted.
        assert!(matches!(
            picker.handle_key(key(KeyCode::Esc)),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::ExternalConsentChoice);
        picker.handle_key(key(KeyCode::Char('2')));
        picker.handle_key(key(KeyCode::Enter));
        assert_eq!(picker.stage, Stage::ExternalConsentConfirm);
        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerExternalConsentConfirmed {
                provider: ApiProvider::OpenaiCodex,
                consent_provider: codewhale_config::ProviderKind::OpenaiCodex,
                source: codewhale_config::ExternalCredentialSource::CodexCli,
                ..
            })
        ));
    }

    #[test]
    fn external_consent_surface_uses_the_selected_locale() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new_for_missing_auth(
            ApiProvider::Deepseek,
            ApiProvider::OpenaiCodex,
            &config,
            None,
        )
        .expect("OpenAI Codex has a picker row")
        .with_locale(crate::localization::Locale::ZhHans);

        picker.handle_key(key(KeyCode::Char('2')));
        picker.handle_key(key(KeyCode::Enter));
        let choices = render_text(&picker, 100, 20);
        let compact = choices
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>();
        assert!(compact.contains("外部凭据访问"), "{choices}");
        assert!(compact.contains("禁用（默认）"), "{choices}");
        assert!(compact.contains("托管（不可用）"), "{choices}");
    }

    #[test]
    fn xai_auth_choice_keeps_api_key_device_oauth_and_external_reuse_distinct() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new_for_missing_auth(
            ApiProvider::Deepseek,
            ApiProvider::Xai,
            &config,
            None,
        )
        .expect("xAI has a picker row");
        assert_eq!(picker.stage, Stage::XaiAuthChoice);

        let rendered = render_text(&picker, 96, 20);
        assert!(rendered.contains("xAI API key"));
        assert!(rendered.contains("Native device OAuth"));
        assert!(rendered.contains("Codewhale-owned storage"));
        picker.handle_key(key(KeyCode::Char('2')));
        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerXaiOAuthRequested)
        ));

        let mut external = ProviderPickerView::new_for_missing_auth(
            ApiProvider::Deepseek,
            ApiProvider::Xai,
            &config,
            None,
        )
        .expect("xAI has a picker row");
        assert!(matches!(
            external.handle_key(key(KeyCode::Char('e'))),
            ViewAction::None
        ));
        assert_eq!(external.stage, Stage::ExternalConsentChoice);
        let rendered = render_text(&external, 100, 20);
        assert!(rendered.contains("Managed (unavailable)"), "{rendered}");
    }

    #[test]
    fn xai_auth_choice_uses_the_selected_locale() {
        let config = Config::default();
        let picker = ProviderPickerView::new_for_missing_auth(
            ApiProvider::Deepseek,
            ApiProvider::Xai,
            &config,
            None,
        )
        .expect("xAI has a picker row")
        .with_locale(crate::localization::Locale::ZhHans);

        let rendered = render_text(&picker, 100, 24);
        let compact = rendered
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>();
        for translated in [
            "xAI身份验证",
            "请选择一个明确的凭据来源",
            "xAIAPI密钥",
            "原生设备OAuth",
        ] {
            assert!(compact.contains(translated), "{translated}: {rendered}");
        }
        assert!(!rendered.contains("Choose one explicit credential source"));
        assert!(!rendered.contains("Native device OAuth"));
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

        let fallback_key = crate::config::ProviderConfig {
            auth_mode: Some("oauth".to_string()),
            api_key: Some("xai-api-key".to_string()),
            ..Default::default()
        };
        assert_eq!(
            xai_oauth_status(Some(&fallback_key), false),
            Some(ProviderAuthStatus::Configured)
        );
        for sentinel in [crate::config::API_KEYRING_SENTINEL, "  __KEYRING__  "] {
            let placeholder = crate::config::ProviderConfig {
                auth_mode: Some("oauth".to_string()),
                api_key: Some(sentinel.to_string()),
                ..Default::default()
            };
            assert_eq!(
                xai_oauth_status(Some(&placeholder), false),
                Some(ProviderAuthStatus::OAuthMissing)
            );
        }
    }

    #[test]
    fn inactive_external_consents_are_visible_without_io_and_never_enter_routing_inventory() {
        let _env = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("external consent fixtures");
        let codex_path = temp.path().join("codex-auth.json");
        let grok_path = temp.path().join("grok-auth.json");
        let codex_raw = "codex-external-file-must-not-be-read";
        let grok_raw = "grok-external-file-must-not-be-read";
        std::fs::write(&codex_path, codex_raw).expect("write Codex trap");
        std::fs::write(&grok_path, grok_raw).expect("write Grok trap");
        let owned_home = temp.path().join("codewhale-owned");

        let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &owned_home);
        let _codex_path =
            crate::test_support::EnvVarGuard::set("OPENAI_CODEX_AUTH_FILE", &codex_path);
        let _grok_path = crate::test_support::EnvVarGuard::set("GROK_AUTH_PATH", &grok_path);
        let _codex_access = crate::test_support::EnvVarGuard::remove("OPENAI_CODEX_ACCESS_TOKEN");
        let _legacy_codex_access = crate::test_support::EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
        let _xai_key = crate::test_support::EnvVarGuard::remove("XAI_API_KEY");
        let _cli_key = crate::test_support::EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");
        let _cli_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");

        let config = Config {
            provider: Some(ApiProvider::Deepseek.as_str().to_string()),
            providers: Some(crate::config::ProvidersConfig {
                openai_codex: crate::config::ProviderConfig {
                    auth_mode: Some("oauth".to_string()),
                    external_credentials: Some(
                        codewhale_config::ExternalCredentialConsentToml::read_only(
                            codewhale_config::ProviderKind::OpenaiCodex,
                            codewhale_config::ExternalCredentialSource::CodexCli,
                            codex_path.clone(),
                        ),
                    ),
                    ..Default::default()
                },
                xai: crate::config::ProviderConfig {
                    auth_mode: Some("oauth".to_string()),
                    external_credentials: Some(
                        codewhale_config::ExternalCredentialConsentToml::read_only(
                            codewhale_config::ProviderKind::Xai,
                            codewhale_config::ExternalCredentialSource::GrokCli,
                            grok_path.clone(),
                        ),
                    ),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        crate::external_credentials::reset_side_effect_trap();
        assert!(!has_api_key_for(&config, ApiProvider::OpenaiCodex));
        assert!(!has_api_key_for(&config, ApiProvider::Xai));

        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        for provider in [ApiProvider::OpenaiCodex, ApiProvider::Xai] {
            let index = picker
                .rows
                .iter()
                .position(|row| row.provider == provider)
                .expect("consented provider row");
            let row = &picker.rows[index];
            assert_eq!(row.credential_state, CredentialState::ExternalConsent);
            assert_eq!(row.auth_status, ProviderAuthStatus::OAuthConsented);
            let structural = row
                .external_credential_status
                .as_ref()
                .expect("external status");
            assert_eq!(structural.access.as_str(), "read_only");
            assert_eq!(structural.route_state, "dormant");
            assert!(structural.revoke_command.contains(provider.as_str()));
            assert_eq!(
                row.readiness,
                ResolvedProviderReadiness::ExternalConsentPendingSelection
            );
            assert!(!row.readiness.can_attempt());
            picker.selected_idx = index;
            let visible = render_text(&picker, 140, 32);
            assert!(visible.contains("External: access=read_only"), "{visible}");
            // #5772: the row names the owning CLI but never the file path —
            // the exact path is disclosed only inside the explicit reuse
            // confirmation.
            assert!(visible.contains("Owner:"), "{visible}");
            assert!(!visible.contains("Owner/path:"), "{visible}");
            let pinned = codewhale_config::quote_os_path(match provider {
                ApiProvider::OpenaiCodex => &codex_path,
                _ => &grok_path,
            });
            assert!(
                !visible.contains(&pinned),
                "ordinary browsing must not disclose {pinned}: {visible}"
            );
            assert!(
                visible.contains("revoke: codewhale auth external-revoke"),
                "{visible}"
            );
            assert!(
                picker.selected_has_key(),
                "selecting {provider:?} should activate the consented route before checking it"
            );
            assert!(matches!(
                picker.handle_key(key(KeyCode::Enter)),
                ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied {
                    provider: selected,
                    ..
                }) if selected == provider
            ));
        }
        // #5772: revocation requires its own confirmation and clears only
        // Codewhale-owned consent state.
        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('x'))),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::ExternalConsentRevokeConfirm);
        let revoke_render = render_text(&picker, 120, 16);
        assert!(
            revoke_render.contains("clears only Codewhale's consent record"),
            "{revoke_render}"
        );
        assert!(
            !revoke_render.contains(&codewhale_config::quote_os_path(&grok_path)),
            "revocation never inspects or names the external file: {revoke_render}"
        );
        assert!(matches!(
            picker.handle_key(key(KeyCode::Esc)),
            ViewAction::None
        ));
        // `x` was pressed from the list, so Esc returns to the list — "back"
        // means the step the user actually came from (#5772).
        assert_eq!(picker.stage, Stage::List);
        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('x'))),
            ViewAction::None
        ));
        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerExternalConsentRevoked {
                provider: ApiProvider::Xai
            })
        ));

        let inventory = crate::model_inventory::ModelInventory::from_config(&config);
        assert!(
            inventory.candidates.iter().all(|candidate| !matches!(
                candidate.provider,
                ApiProvider::OpenaiCodex | ApiProvider::Xai
            )),
            "dormant external-only routes must not reach auto-routing inventory"
        );
        assert_eq!(
            crate::route_billing::for_route(&config, ApiProvider::Xai),
            crate::route_billing::BillingPresentation::Metered
        );
        assert_eq!(
            crate::external_credentials::side_effect_trap_counts(),
            (0, 0),
            "picker, readiness, billing, and model inventory must not inspect inactive external files"
        );
        assert_eq!(
            std::fs::read_to_string(&codex_path).expect("Codex trap unchanged"),
            codex_raw
        );
        assert_eq!(
            std::fs::read_to_string(&grok_path).expect("Grok trap unchanged"),
            grok_raw
        );
        assert!(!owned_home.join("credentials/xai-auth.json").exists());
    }

    /// Modal body text with layout removed: ratatui word-wraps a long path
    /// across rows and pads each row to the modal edge, so a `contains` check
    /// against a full path is a check on the terminal width, not on what was
    /// disclosed. Strip whitespace and box-drawing glyphs so the assertion is
    /// about the content.
    fn unwrapped_modal_text(rendered: &str) -> String {
        rendered
            .chars()
            .filter(|ch| !ch.is_whitespace() && !('\u{2500}'..='\u{257f}').contains(ch))
            .collect()
    }

    /// #5772: with reuse off, ordinary browsing and plain Enter perform zero
    /// external I/O and never mint, persist, or reveal an external credential
    /// grant; only the explicit `e` action discloses the exact path, and only
    /// its confirmation emits the grant event.
    #[test]
    fn unconsented_external_row_performs_no_io_and_grants_only_after_confirmation() {
        // Constructing and rendering the full provider picker is intentionally
        // broad: the regression has to prove that ordinary catalog browsing,
        // key entry, and both consent modals all preserve the same no-I/O
        // boundary. That state is larger than libtest's default thread stack,
        // while the product TUI runs on a deliberately larger stack. Mirror
        // the product/test precedent instead of requiring RUST_MIN_STACK in CI.
        std::thread::Builder::new()
            .name("provider-consent-no-io".to_string())
            .stack_size(16 * 1024 * 1024)
            .spawn(unconsented_external_row_performs_no_io_on_sized_stack)
            .expect("spawn provider-consent regression thread")
            .join()
            .expect("provider-consent regression thread");
    }

    fn unconsented_external_row_performs_no_io_on_sized_stack() {
        let _env = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("external reuse fixtures");
        let codex_path = temp
            .path()
            .canonicalize()
            .expect("canonical temp root")
            .join("auth.json");
        // A fresh-looking token file exists on disk; nothing may look at it.
        let codex_raw = "{\"tokens\":{\"access_token\":\"header.eyJleHAiOjk5OTk5OTk5OTl9.sig\"}}";
        std::fs::write(&codex_path, codex_raw).expect("write Codex trap");

        let _codex_home =
            crate::test_support::EnvVarGuard::set("OPENAI_CODEX_AUTH_FILE", &codex_path);
        let _codex_access = crate::test_support::EnvVarGuard::remove("OPENAI_CODEX_ACCESS_TOKEN");
        let _legacy_access = crate::test_support::EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
        let _cli_key = crate::test_support::EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");

        let config = Config {
            provider: Some(ApiProvider::Deepseek.as_str().to_string()),
            ..Default::default()
        };

        crate::external_credentials::reset_side_effect_trap();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::OpenaiCodex);
        let quoted = codewhale_config::quote_os_path(&codex_path);
        let quoted_unwrapped = unwrapped_modal_text(&quoted);

        // Ordinary browsing reveals neither the path nor the file's existence.
        let visible = render_text(&picker, 140, 32);
        assert!(
            !unwrapped_modal_text(&visible).contains(&quoted_unwrapped),
            "ordinary browsing must not disclose {quoted}: {visible}"
        );

        // Plain Enter begins ordinary setup; it must not read the external
        // file, mint a grant, or emit the consent-confirmed event. #5778 puts
        // openai-codex behind the ChatGPT auth choice first; entering that
        // stage stays inside the no-I/O boundary.
        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::ChatgptAuthChoice);
        assert_eq!(
            crate::external_credentials::complete_side_effect_trap_counts(),
            (0, 0, 0, 0, 0),
            "ordinary selection must not touch external credential state"
        );

        // The explicit reuse flow: choose "Import from Codex CLI", then Enter;
        // the choice stage still hides the path and performs no I/O.
        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('2'))),
            ViewAction::None
        ));
        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::ExternalConsentChoice);
        let choices = render_text(&picker, 100, 20);
        assert!(
            choices.contains("Use external CLI credentials (read-only)"),
            "{choices}"
        );
        assert!(
            !unwrapped_modal_text(&choices).contains(&quoted_unwrapped),
            "the choice stage must not disclose {quoted}: {choices}"
        );

        // …the confirmation stage discloses the exact candidate path…
        picker.handle_key(key(KeyCode::Char('2')));
        picker.handle_key(key(KeyCode::Enter));
        assert_eq!(picker.stage, Stage::ExternalConsentConfirm);
        let confirm = render_text(&picker, 120, 30);
        assert!(
            unwrapped_modal_text(&confirm).contains(&quoted_unwrapped),
            "confirmation must name the exact path: {confirm}"
        );
        // …and even rendering the confirmation performed no external I/O.
        assert_eq!(
            crate::external_credentials::complete_side_effect_trap_counts(),
            (0, 0, 0, 0, 0),
            "disclosure must not validate or read before affirmative confirmation"
        );

        // Only the affirmative Enter on the confirmation emits the grant event.
        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerExternalConsentConfirmed {
                provider: ApiProvider::OpenaiCodex,
                ..
            })
        ));
        assert_eq!(
            std::fs::read_to_string(&codex_path).expect("Codex trap unchanged"),
            codex_raw
        );
    }

    /// #5772: revoking external access requires its own confirmation; a bare
    /// `x` chord only opens the disclosure.
    #[test]
    fn revoke_requires_confirmation() {
        let _env = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("revoke fixtures");
        let codex_path = temp
            .path()
            .canonicalize()
            .expect("canonical temp root")
            .join("auth.json");
        std::fs::write(&codex_path, "codex-external-file-must-not-be-read").expect("trap");
        let _codex_home =
            crate::test_support::EnvVarGuard::set("OPENAI_CODEX_AUTH_FILE", &codex_path);
        let _codex_access = crate::test_support::EnvVarGuard::remove("OPENAI_CODEX_ACCESS_TOKEN");
        let _legacy_access = crate::test_support::EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
        let _cli_key = crate::test_support::EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");

        let config = Config {
            provider: Some(ApiProvider::Deepseek.as_str().to_string()),
            providers: Some(crate::config::ProvidersConfig {
                openai_codex: crate::config::ProviderConfig {
                    auth_mode: Some("oauth".to_string()),
                    external_credentials: Some(
                        codewhale_config::ExternalCredentialConsentToml::read_only(
                            codewhale_config::ProviderKind::OpenaiCodex,
                            codewhale_config::ExternalCredentialSource::CodexCli,
                            codex_path.clone(),
                        ),
                    ),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        crate::external_credentials::reset_side_effect_trap();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::OpenaiCodex);

        assert!(matches!(
            picker.handle_key(key(KeyCode::Char('x'))),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::ExternalConsentRevokeConfirm);
        assert_eq!(
            crate::external_credentials::complete_side_effect_trap_counts(),
            (0, 0, 0, 0, 0),
            "revocation disclosure must not inspect the external file"
        );
        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerExternalConsentRevoked {
                provider: ApiProvider::OpenaiCodex
            })
        ));
    }

    #[test]
    fn kimi_cli_token_is_never_auto_enabled_without_explicit_legacy_auth_mode() {
        let _env = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("Kimi import fixture root");
        let kimi_home = temp.path().join("kimi-code");
        std::fs::create_dir_all(kimi_home.join("credentials"))
            .expect("Kimi import credential directory");
        let expires_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_secs_f64()
            + 3600.0;
        std::fs::write(
            kimi_home.join("credentials/kimi-code.json"),
            serde_json::json!({
                "access_token": "unexpired-user-owned-token",
                "refresh_token": "must-not-be-used",
                "expires_at": expires_at,
            })
            .to_string(),
        )
        .expect("write Kimi import fixture");
        let _kimi_home = crate::test_support::EnvVarGuard::set(
            "KIMI_CODE_HOME",
            kimi_home.to_str().expect("utf8 path"),
        );
        let _moonshot_key = crate::test_support::EnvVarGuard::remove("MOONSHOT_API_KEY");
        let _kimi_key = crate::test_support::EnvVarGuard::remove("KIMI_API_KEY");

        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &Config::default());
        move_to_provider(&mut picker, ApiProvider::Moonshot);
        let row = &picker.rows[picker.selected_idx];
        assert_eq!(row.auth_status, ProviderAuthStatus::Missing);
        assert_eq!(row.credential_state, CredentialState::MissingKey);

        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::None
        ));
        assert_eq!(
            picker.stage,
            Stage::KeyEntry,
            "a stray Kimi CLI credential must lead to API-key setup, not import activation"
        );
    }

    #[test]
    fn explicit_legacy_kimi_import_is_unavailable_and_routes_to_api_key_setup() {
        let _env = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("Kimi import fixture root");
        let kimi_home = temp.path().join("kimi-code");
        std::fs::create_dir_all(kimi_home.join("credentials"))
            .expect("Kimi import credential directory");
        let expires_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_secs_f64()
            + 3600.0;
        std::fs::write(
            kimi_home.join("credentials/kimi-code.json"),
            serde_json::json!({
                "access_token": "unexpired-user-owned-token",
                "refresh_token": "must-not-be-used",
                "expires_at": expires_at,
            })
            .to_string(),
        )
        .expect("write Kimi import fixture");
        let _kimi_home = crate::test_support::EnvVarGuard::set(
            "KIMI_CODE_HOME",
            kimi_home.to_str().expect("utf8 path"),
        );
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                moonshot: crate::config::ProviderConfig {
                    auth_mode: Some("kimi_oauth".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Moonshot);
        let row = &picker.rows[picker.selected_idx];
        assert_eq!(
            row.auth_status,
            ProviderAuthStatus::ImportedTokenUnavailable
        );
        assert_eq!(row.credential_state, CredentialState::MissingKey);
        assert_eq!(row.base_url, crate::config::DEFAULT_KIMI_CODE_BASE_URL);
        assert_eq!(
            row.default_route.logical_model,
            crate::config::DEFAULT_KIMI_CODE_MODEL
        );
        // Slice D: the Kimi key guidance travels on messages, not on a
        // provider-level meter.
        assert!(
            row.messages
                .iter()
                .any(|message| message.contains("Kimi API key")),
            "missing Kimi key guidance: {:?}",
            row.messages
        );
        assert_eq!(row.readiness, ResolvedProviderReadiness::MissingKey);
        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::None
        ));
        assert_eq!(picker.stage, Stage::KeyEntry);
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
                cell.bg == palette::SELECTION_BG
            })
            .count();
        assert!(
            highlighted_cells >= 32,
            "selected provider row should use a visible continuous highlight"
        );
    }

    #[test]
    fn search_footer_shows_two_stage_esc_as_a_single_hint() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        picker.query = "deep".to_string();
        let area = Rect::new(0, 0, 100, 24);
        let mut buf = Buffer::empty(area);

        picker.render(area, &mut buf);

        let text = area
            .positions()
            .map(|position| buf[position].symbol())
            .collect::<String>();
        // The key appears once, with both stages spelled out in its label.
        assert_eq!(
            text.matches(" Esc ").count(),
            1,
            "search footer must not duplicate the Esc key: {text}"
        );
        assert!(text.contains("clear / cancel"), "{text}");
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
