use chrono::{DateTime, Duration, Utc};
use codewhale_config::route::{
    LimitField, LogicalModelRef, OverrideSource, ReadyRouteCandidate, RouteLimits, RouteRequest,
    RouteResolver, SourcedLimitOverride, WireModelId,
};
use serde::Serialize;

use crate::client::DeepSeekClient;
use crate::codex_model_cache::{CodexModelCacheFreshness, model_roster};
use crate::config::{
    ApiProvider, Config, DEFAULT_NVIDIA_NIM_BASE_URL, KIMI_CODE_K3_CONTEXT_WINDOW_TOKENS,
    ProviderIdentity, is_exact_direct_moonshot_k3_route, is_exact_kimi_code_bare_k3_route,
    validate_kimi_code_api_model_id,
};
use crate::models::DIRECT_KIMI_K3_MAX_OUTPUT_TOKENS;

/// Why a route is using its effective context-window value.  Keep this
/// receipt separate from the numeric route limits so every consumer can state
/// whether the number is operator-configured, freshly provider-reported, a
/// Kimi Code safety floor, catalog data, or a conservative fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContextWindowSource {
    Configured,
    ProviderReported,
    StaticKimiCodeSafeFloor,
    Catalog,
    /// Parsed from a vendor-agnostic `_Nk` suffix in the model name
    /// (#5441). Optimistic, unlike the conservative [`Self::Fallback`]: a
    /// serving engine may ignore its own naming convention, so the number
    /// drives real budgets but is never evidence about the route.
    NameSuffixHint,
    Fallback,
}

impl ContextWindowSource {
    /// Every rung, in precedence order. The name-suffix hint sits between
    /// catalog data and the conservative fallback: any concrete fact about
    /// the route beats a naming convention.
    pub(crate) const ALL: [Self; 6] = [
        Self::Configured,
        Self::ProviderReported,
        Self::StaticKimiCodeSafeFloor,
        Self::Catalog,
        Self::NameSuffixHint,
        Self::Fallback,
    ];

    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::ProviderReported => "provider-reported",
            Self::StaticKimiCodeSafeFloor => "static Kimi Code safe floor",
            Self::Catalog => "catalog",
            Self::NameSuffixHint => "model-name hint",
            Self::Fallback => "fallback",
        }
    }

    /// Recover the rung a serialized report wrote, so a surface holding only
    /// the label still reads verification off the enum instead of matching
    /// strings.  An unrecognized label is nobody's rung.
    #[must_use]
    pub(crate) fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|rung| rung.label() == label)
    }

    /// Whether the window rests on evidence about this exact route.  The
    /// name-suffix hint and the fallback rung are guesses — one parsed from a
    /// naming convention, one made because nothing described the model — so
    /// no surface may present either as a capability we checked (#5239,
    /// #5441).
    #[must_use]
    pub(crate) const fn is_verified(self) -> bool {
        !matches!(self, Self::NameSuffixHint | Self::Fallback)
    }

    /// Suffix every rendered window carries: verified rungs stay bare,
    /// guesses say so next to the number that drives the budget.
    #[must_use]
    pub(crate) const fn honesty_suffix(self) -> &'static str {
        if self.is_verified() {
            ""
        } else {
            " (unverified)"
        }
    }

    /// [`Self::label`] plus [`Self::honesty_suffix`], ready for inline
    /// rendering (status line, `/status`, `/config` rows).
    #[must_use]
    pub(crate) fn display_label(self) -> String {
        format!("{}{}", self.label(), self.honesty_suffix())
    }
}

/// Context window carried alongside an exact runtime route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct ContextWindowResolution {
    pub(crate) tokens: u32,
    pub(crate) source: ContextWindowSource,
}

/// Resolve the effective context window for a host holding no fully resolved
/// route candidate: an `auto` selection, a model switch that keeps the current
/// endpoint, or a route resolution that failed.
///
/// Only the rungs derivable without an endpoint-scoped candidate are reachable
/// here — operator config, then offering/catalog limits, then the conservative
/// capability fallback.  The provider-reported and Kimi Code safe-floor rungs
/// need a resolved candidate and stay in [`plan_limit_overrides`].
///
/// The catalog predicate must stay identical to the one in
/// [`crate::route_budget::route_context_window_tokens`]: the pressure meter and
/// compaction trigger read their number from there, so any divergence would
/// print one rung's number under another rung's label.
#[must_use]
pub(crate) fn resolve_context_window(
    provider: ApiProvider,
    model: &str,
    route_limits: Option<RouteLimits>,
    context_window_override: Option<u32>,
) -> ContextWindowResolution {
    if let Some(tokens) = context_window_override.filter(|tokens| *tokens > 0) {
        return ContextWindowResolution {
            tokens,
            source: ContextWindowSource::Configured,
        };
    }
    if let Some(tokens) = route_limits
        .and_then(|limits| limits.context_tokens)
        .and_then(|tokens| u32::try_from(tokens).ok())
        .filter(|tokens| *tokens > 0)
    {
        return ContextWindowResolution {
            tokens,
            source: ContextWindowSource::Catalog,
        };
    }
    let tokens = crate::route_budget::route_context_window_tokens(provider, model, None);
    ContextWindowResolution {
        tokens,
        source: classify_capability_fallback_window(model, tokens),
    }
}

/// Classify a window the provider/model capability fallback produced, so the
/// receipt names the rung the number actually came from (#5239, #5441).
///
/// A value parsed from an `_Nk` model-name suffix is its own optimistic rung:
/// the serving engine may not honor its own naming convention. Everything
/// else the fallback produced — vendor-family heuristics, provider floors,
/// the conservative default — is the plain fallback rung. Both are
/// unverified; the ladder keeps them apart because they fail differently
/// (a hint that overstates the window delays compaction past the provider's
/// real limit).
fn classify_capability_fallback_window(model: &str, tokens: u32) -> ContextWindowSource {
    if crate::models::name_suffix_context_window_hint(model) == Some(tokens) {
        ContextWindowSource::NameSuffixHint
    } else {
        ContextWindowSource::Fallback
    }
}

/// Authenticated Kimi Code `/models` metadata that a caller has already
/// validated.  This is intentionally route-scoped: generic Moonshot metadata
/// can never promote a bare `k3` route.  The current runtime has no implicit
/// network probe; an authenticated model-listing consumer may pass this value
/// to [`resolve_route_candidate_with_context_metadata`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderReportedKimiCodeContext {
    pub(crate) context_tokens: u32,
    pub(crate) observed_at: DateTime<Utc>,
}

const KIMI_CODE_REPORTED_CONTEXT_MAX_AGE_HOURS: i64 = 24;

#[derive(Debug)]
pub(crate) struct RouteCandidateResolution {
    pub(crate) candidate: ReadyRouteCandidate,
    pub(crate) context_window: ContextWindowResolution,
}

#[derive(Clone)]
pub(crate) struct ResolvedRuntimeRoute {
    pub(crate) identity: ProviderIdentity,
    pub(crate) candidate: ReadyRouteCandidate,
    pub(crate) config: Box<Config>,
    pub(crate) model: String,
    pub(crate) context_window: ContextWindowResolution,
    preflighted_client: Option<DeepSeekClient>,
}

impl std::fmt::Debug for ResolvedRuntimeRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedRuntimeRoute")
            .field("provider_identity", &self.identity.key)
            .field("provider", &self.identity.provider)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

/// One exact provider route, fully resolved and client-preflighted before a
/// host mutates session/runtime state. The config and client may contain
/// credentials, so diagnostics intentionally expose only non-secret receipt
/// fields.
#[derive(Clone)]
pub(crate) struct ValidatedRuntimeRoute {
    pub(crate) identity: ProviderIdentity,
    pub(crate) candidate: ReadyRouteCandidate,
    pub(crate) config: Box<Config>,
    pub(crate) model: String,
    pub(crate) context_window: ContextWindowResolution,
    pub(crate) client: DeepSeekClient,
}

impl std::fmt::Debug for ValidatedRuntimeRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidatedRuntimeRoute")
            .field("provider_identity", &self.identity.key)
            .field("provider", &self.identity.provider)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl ResolvedRuntimeRoute {
    pub(crate) fn preflight(mut self) -> Result<Self, String> {
        if self.preflighted_client.is_none() {
            self.preflighted_client = Some(
                DeepSeekClient::from_candidate(&self.config, &self.candidate).map_err(|err| {
                    format_provider_route_preflight_error(&self.identity.key, &self.model, &err)
                })?,
            );
        }
        Ok(self)
    }

    pub(crate) fn validate(mut self) -> Result<ValidatedRuntimeRoute, String> {
        let client = match self.preflighted_client.take() {
            Some(client) => client,
            None => {
                DeepSeekClient::from_candidate(&self.config, &self.candidate).map_err(|err| {
                    format_provider_route_preflight_error(&self.identity.key, &self.model, &err)
                })?
            }
        };
        Ok(ValidatedRuntimeRoute {
            identity: self.identity,
            candidate: self.candidate,
            config: self.config,
            model: self.model,
            context_window: self.context_window,
            client,
        })
    }

    pub(crate) fn take_preflighted_client(&mut self) -> Option<DeepSeekClient> {
        self.preflighted_client.take()
    }
}

fn format_provider_route_preflight_error(
    identity_key: &str,
    model: &str,
    err: &anyhow::Error,
) -> String {
    let reason = err.to_string().trim().to_string();
    let mut message = format!(
        "{}. Failed to configure provider route {} / {}.",
        reason, identity_key, model
    );
    if let Some(next_step) = classify_provider_route_preflight_next_step(identity_key, &reason) {
        message.push_str(" Next step: ");
        message.push_str(&next_step);
    }
    message
}

fn classify_provider_route_preflight_next_step(identity_key: &str, reason: &str) -> Option<String> {
    let lower = reason.to_ascii_lowercase();
    if lower
        .contains("codex oauth credentials are only available on the official openai codex route")
    {
        return Some(format!(
            "Run /provider setup {identity_key} and remove its custom base URL; Codex OAuth only works on the official route."
        ));
    }
    if lower.contains("openai codex oauth credentials are unavailable")
        || lower.contains("codex access token")
    {
        return Some(format!(
            "Run `codewhale auth chatgpt` or /provider setup {identity_key} to Sign in with ChatGPT; Codex CLI import remains an explicit alternative."
        ));
    }
    if lower.contains("api key not found")
        || lower.contains("access token")
        || (lower.contains("credential")
            && (lower.contains("not found")
                || lower.contains("missing")
                || lower.contains("unsupported")))
    {
        return Some(format!(
            "Run /auth or /provider setup {identity_key} to configure credentials."
        ));
    }
    if lower.contains("tls certificate")
        || lower.contains("ssl_cert_file")
        || lower.contains("certificate verification")
        || lower.contains("insecure_skip_tls_verify")
        || lower.contains("base url")
        || lower.contains("invalid url")
    {
        return Some(format!(
            "Run /provider setup {identity_key} to fix base URL/TLS settings."
        ));
    }
    if lower.contains("provider")
        && lower.contains("model")
        && (lower.contains("pin")
            || lower.contains("mismatch")
            || lower.contains("unknown")
            || lower.contains("not found"))
    {
        return Some(
            "Run /models (or open the model picker) and choose a model valid for this provider."
                .to_string(),
        );
    }
    if lower.contains("fleet") || lower.contains("profile") || lower.contains("partial route") {
        return Some(
            "Review Fleet profile provider/model overrides; keep route fields atomic (#5042)."
                .to_string(),
        );
    }
    Some(format!(
        "Run /provider setup {identity_key} to review this route configuration."
    ))
}

impl ValidatedRuntimeRoute {
    /// Preserve the preflighted client with the exact resolved route receipt
    /// so the engine does not repeat environment-sensitive client discovery.
    pub(crate) fn into_resolved(self) -> ResolvedRuntimeRoute {
        ResolvedRuntimeRoute {
            identity: self.identity,
            candidate: self.candidate,
            config: self.config,
            model: self.model,
            context_window: self.context_window,
            preflighted_client: Some(self.client),
        }
    }
}

pub(crate) fn resolve_route_candidate(
    provider: ApiProvider,
    model_selector: Option<&str>,
    saved_provider_model: Option<&str>,
    base_url_override: Option<String>,
    context_window_override: Option<u32>,
) -> Result<ReadyRouteCandidate, String> {
    resolve_route_candidate_with_context_metadata(
        provider,
        model_selector,
        saved_provider_model,
        base_url_override,
        context_window_override,
        None,
    )
    .map(|resolution| resolution.candidate)
}

/// Reject only a provider-less model mismatch that existing route knowledge
/// proves foreign. Partial catalogs are not allowlists: unknown ids, local
/// runtimes, gateways, and custom endpoints remain provider-authoritative.
pub(crate) fn validate_unpinned_model_provider(
    provider: ApiProvider,
    model: &str,
    base_url: &str,
) -> Result<(), String> {
    let Some(kind) = provider.kind() else {
        return Ok(());
    };
    let Some(owner) = codewhale_config::known_foreign_model_owner(kind, model, base_url) else {
        return Ok(());
    };
    Err(format!(
        "Model `{}` was supplied without an explicit provider pin, but the resolved route is `{}` and the owning provider is `{}`. Pin the provider together with the model, or inherit the session route.",
        model.trim(),
        provider.as_str(),
        owner.as_str()
    ))
}

/// Resolve a provider-less fixed model to the provider's exact wire id before
/// child admission. This shares the runtime resolver used by Fleet receipts,
/// including aggregator alias translation, without making a live request.
pub(crate) fn resolve_unpinned_model_candidate(
    provider: ApiProvider,
    model: &str,
    base_url: &str,
) -> Result<ReadyRouteCandidate, String> {
    validate_unpinned_model_provider(provider, model, base_url)?;
    resolve_route_candidate(
        provider,
        Some(model),
        None,
        Some(base_url.to_string()),
        None,
    )
}

/// Resolve a candidate together with a non-secret context-window provenance
/// receipt.  `provider_reported_context` is accepted only for the exact Kimi
/// Code bare-K3 endpoint, only at the documented 1M entitlement, and only
/// while fresh; this prevents generic Moonshot or stale metadata from being
/// inherited by a membership-plan route.
pub(crate) fn resolve_route_candidate_with_context_metadata(
    provider: ApiProvider,
    model_selector: Option<&str>,
    saved_provider_model: Option<&str>,
    base_url_override: Option<String>,
    context_window_override: Option<u32>,
    provider_reported_context: Option<ProviderReportedKimiCodeContext>,
) -> Result<RouteCandidateResolution, String> {
    let effective_base_url = base_url_override
        .as_deref()
        .unwrap_or_else(|| provider.default_base_url());
    if let Some(model) = model_selector.or(saved_provider_model) {
        validate_kimi_code_api_model_id(provider, effective_base_url, model)?;
    }
    let resolver = RouteResolver::new();
    let base_request = RouteRequest {
        explicit_provider: provider.kind(),
        model_selector: model_selector.map(|model| LogicalModelRef::from(model.to_string())),
        saved_provider_model: saved_provider_model
            .map(|model| WireModelId::from(model.to_string())),
        base_url_override,
        limit_overrides: Vec::new(),
    };
    // First pass: resolve the route without overrides to learn the effective
    // endpoint, wire model id, and catalog limits. Candidates are immutable, so
    // limit adjustments are planned from this read-only resolution and then
    // requested through `RouteRequest::limit_overrides` on a second pass; the
    // resolver applies them BEFORE minting the final candidate and records
    // their provenance on it.
    let resolved = resolver
        .resolve(&base_request)
        .map_err(|err| err.to_string())?;
    let plan = plan_limit_overrides(
        provider,
        &resolved,
        context_window_override,
        provider_reported_context,
    );
    let candidate = if plan.overrides.is_empty() {
        resolved
    } else {
        resolver
            .resolve(&RouteRequest {
                limit_overrides: plan.overrides,
                ..base_request
            })
            .map_err(|err| err.to_string())?
    };
    Ok(RouteCandidateResolution {
        candidate,
        context_window: plan.context_window,
    })
}

/// The sourced limit overrides a route needs, plus the context-window receipt
/// describing the effective context value they produce.
struct LimitOverridePlan {
    overrides: Vec<SourcedLimitOverride>,
    context_window: ContextWindowResolution,
}

/// Plan the limit overrides for a resolved route.
///
/// Precedence (unchanged from the previous post-hoc mutation order):
/// provider-scoped roster/API corrections and exact-route documented output
/// facts first, then operator-configured context, then fresh route-scoped
/// provider-reported context, then the membership-plan safe floor, then
/// catalog data, then the conservative fallback.
fn plan_limit_overrides(
    provider: ApiProvider,
    resolved: &ReadyRouteCandidate,
    context_window_override: Option<u32>,
    provider_reported_context: Option<ProviderReportedKimiCodeContext>,
) -> LimitOverridePlan {
    let mut overrides = Vec::new();
    let configured = context_window_override.filter(|window| *window > 0);
    let mut effective_context = resolved.limits().context_tokens;
    if is_exact_direct_moonshot_k3_route(
        provider,
        &resolved.endpoint().base_url,
        resolved.wire_model_id().as_str(),
    ) {
        overrides.push(SourcedLimitOverride {
            field: LimitField::OutputTokens,
            value: Some(u64::from(DIRECT_KIMI_K3_MAX_OUTPUT_TOKENS)),
            source: OverrideSource::DocumentedRouteOutputMaximum,
        });
    }
    if provider == ApiProvider::OpenaiCodex {
        // Models.dev describes the public API offering, not the account-scoped
        // ChatGPT OAuth route. Strip API-only limits, then carry the fresh
        // Codex roster's per-model context into every runtime consumer.
        overrides.push(SourcedLimitOverride {
            field: LimitField::InputTokens,
            value: None,
            source: OverrideSource::CodexPublicApiLimitStrip,
        });
        overrides.push(SourcedLimitOverride {
            field: LimitField::OutputTokens,
            value: None,
            source: OverrideSource::CodexPublicApiLimitStrip,
        });
        if configured.is_none() {
            let roster = model_roster();
            let roster_context = if roster.freshness == CodexModelCacheFreshness::Fresh {
                roster
                    .metadata_for(resolved.wire_model_id().as_str())
                    .and_then(|metadata| metadata.context_window)
                    .map(u64::from)
            } else {
                None
            };
            effective_context = roster_context;
            overrides.push(SourcedLimitOverride {
                field: LimitField::ContextTokens,
                value: roster_context,
                source: OverrideSource::CodexRosterCorrection,
            });
        }
    }

    if let Some(context_window) = configured {
        overrides.push(SourcedLimitOverride {
            field: LimitField::ContextTokens,
            value: Some(u64::from(context_window)),
            source: OverrideSource::UserContextWindow,
        });
        return LimitOverridePlan {
            overrides,
            context_window: ContextWindowResolution {
                tokens: context_window,
                source: ContextWindowSource::Configured,
            },
        };
    }

    let is_exact_kimi_code_k3 = is_exact_kimi_code_bare_k3_route(
        provider,
        &resolved.endpoint().base_url,
        resolved.wire_model_id().as_str(),
    );
    let now = Utc::now();
    if is_exact_kimi_code_k3
        && provider_reported_context.is_some_and(|reported| {
            reported.context_tokens == 1_048_576
                && reported.observed_at <= now
                && now.signed_duration_since(reported.observed_at)
                    <= Duration::hours(KIMI_CODE_REPORTED_CONTEXT_MAX_AGE_HOURS)
        })
    {
        let reported = provider_reported_context.expect("checked above");
        overrides.push(SourcedLimitOverride {
            field: LimitField::ContextTokens,
            value: Some(u64::from(reported.context_tokens)),
            source: OverrideSource::ProviderReportedContextWindow,
        });
        return LimitOverridePlan {
            overrides,
            context_window: ContextWindowResolution {
                tokens: reported.context_tokens,
                source: ContextWindowSource::ProviderReported,
            },
        };
    }

    // Kimi Code's bare `k3` is a membership-plan route, not an alias for
    // Moonshot's public `kimi-k3` catalog entry.  The safe all-plan floor is
    // the route's next precedence after an explicit config or fresh, scoped
    // provider report.
    if is_exact_kimi_code_k3 {
        overrides.push(SourcedLimitOverride {
            field: LimitField::ContextTokens,
            value: Some(u64::from(KIMI_CODE_K3_CONTEXT_WINDOW_TOKENS)),
            source: OverrideSource::MembershipPlanSafeFloor,
        });
        return LimitOverridePlan {
            overrides,
            context_window: ContextWindowResolution {
                tokens: KIMI_CODE_K3_CONTEXT_WINDOW_TOKENS,
                source: ContextWindowSource::StaticKimiCodeSafeFloor,
            },
        };
    }

    if let Some(tokens) = effective_context.and_then(|tokens| u32::try_from(tokens).ok()) {
        return LimitOverridePlan {
            overrides,
            context_window: ContextWindowResolution {
                tokens,
                source: ContextWindowSource::Catalog,
            },
        };
    }

    let fallback_tokens =
        crate::config::provider_capability(provider, resolved.wire_model_id().as_str())
            .context_window;
    LimitOverridePlan {
        overrides,
        context_window: ContextWindowResolution {
            tokens: fallback_tokens,
            source: classify_capability_fallback_window(
                resolved.wire_model_id().as_str(),
                fallback_tokens,
            ),
        },
    }
}

pub(crate) fn resolve_runtime_route(
    config: &Config,
    provider: ApiProvider,
    model_selector: Option<&str>,
) -> Result<ResolvedRuntimeRoute, String> {
    let identity = if provider == ApiProvider::Custom {
        config.active_provider_identity(provider)?
    } else {
        config
            .resolve_persisted_provider_identity(Some(provider.as_str()), Some(provider.as_str()))?
    };
    resolve_runtime_route_for_identity(config, &identity, model_selector)
}

/// Resolve one persisted/live identity into a scoped runtime config and route
/// candidate. Identity is revalidated against the live registry before any
/// endpoint, model, credential, or client material is read.
pub(crate) fn resolve_runtime_route_for_identity(
    config: &Config,
    identity: &ProviderIdentity,
    model_selector: Option<&str>,
) -> Result<ResolvedRuntimeRoute, String> {
    let identity = config.resolve_persisted_provider_identity(
        Some(identity.provider.as_str()),
        identity.persisted_id(),
    )?;
    let provider = identity.provider;
    let mut route_config = prepared_route_config(config, &identity, model_selector);
    let saved_provider_model = configured_model_for_route(&route_config, provider);
    // #5034: with no explicit selector and no saved model, a Codex route
    // would fall back to the resolver's static seed offering. Prefer the
    // live Codex roster head so a provider switch lands on the current
    // flagship model; a missing/stale roster keeps the seed offering.
    let roster_preferred = (provider == ApiProvider::OpenaiCodex
        && model_selector.is_none()
        && saved_provider_model.is_none())
    .then(|| model_roster().preferred_model_id().map(str::to_string))
    .flatten();
    let model_selector = model_selector.or(roster_preferred.as_deref());
    let resolution = resolve_route_candidate_with_context_metadata(
        provider,
        model_selector,
        saved_provider_model,
        Some(route_config.deepseek_base_url()),
        route_config.context_window_for_provider_config(provider),
        None,
    )?;
    let candidate = resolution.candidate;
    let model = candidate.wire_model_id().as_str().to_string();
    set_model_for_route(&mut route_config, provider, &model);

    Ok(ResolvedRuntimeRoute {
        identity,
        candidate,
        config: Box::new(route_config),
        model,
        context_window: resolution.context_window,
        preflighted_client: None,
    })
}

fn prepared_route_config(
    config: &Config,
    identity: &ProviderIdentity,
    model_selector: Option<&str>,
) -> Config {
    let mut route_config = config.clone();
    route_config.scope_to_provider_identity(identity);
    let provider = identity.provider;
    if matches!(provider, ApiProvider::NvidiaNim)
        && route_config
            .base_url
            .as_deref()
            .map(|base| !base.contains("integrate.api.nvidia.com"))
            .unwrap_or(true)
    {
        route_config.base_url = Some(DEFAULT_NVIDIA_NIM_BASE_URL.to_string());
    }
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        && route_config
            .base_url
            .as_deref()
            .map(root_base_url_belongs_to_non_deepseek_provider)
            .unwrap_or(false)
    {
        route_config.base_url = None;
    }
    if let Some(model) = model_selector {
        set_model_for_route(&mut route_config, provider, model);
    }
    route_config
}

fn configured_model_for_route(config: &Config, provider: ApiProvider) -> Option<&str> {
    if provider == ApiProvider::Custom && config.uses_legacy_literal_custom_route() {
        return config.default_text_model.as_deref();
    }
    config
        .provider_config_for(provider)
        .and_then(|provider| provider.model.as_deref())
}

fn set_model_for_route(config: &mut Config, provider: ApiProvider, model: &str) {
    config.set_provider_model_override(provider, Some(model.to_string()));
}

fn root_base_url_belongs_to_non_deepseek_provider(base_url: &str) -> bool {
    let lower = base_url.to_ascii_lowercase();
    [
        "integrate.api.nvidia.com",
        "api.openai.com",
        "api.atlascloud.ai",
        "maas-openapi.wanjiedata.com",
        "volces.com",
        "openrouter.ai",
        "xiaomimimo.com",
        "novita.ai",
        "fireworks.ai",
        "siliconflow",
        "arcee.ai",
        "moonshot.ai",
        "api.kimi.com",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DEFAULT_TEXT_MODEL, DEFAULT_ZAI_MODEL, ProviderConfig, ProvidersConfig};

    /// Every rung keeps its own label and round-trips through it, and only
    /// the guesses read as unverified.  Two rungs sharing a label would let a
    /// guess be displayed as evidence.
    #[test]
    fn every_context_window_rung_round_trips_its_own_label() {
        let mut seen = Vec::new();
        for source in ContextWindowSource::ALL {
            let label = source.label();
            assert!(!label.is_empty(), "{source:?} must carry a label");
            assert!(
                !seen.contains(&label),
                "{source:?} reuses the label {label}"
            );
            seen.push(label);
            assert_eq!(ContextWindowSource::from_label(label), Some(source));
            assert_eq!(
                source.is_verified(),
                !matches!(
                    source,
                    ContextWindowSource::Fallback | ContextWindowSource::NameSuffixHint
                ),
                "{source:?} misreports whether its window rests on route evidence"
            );
            assert_eq!(
                source.honesty_suffix(),
                if source.is_verified() {
                    ""
                } else {
                    " (unverified)"
                },
                "{source:?} must mark every guess it renders"
            );
        }
        assert_eq!(ContextWindowSource::from_label("configured "), None);
    }

    /// #5441: a window parsed from an `_Nk` model-name suffix is its own
    /// unverified rung — optimistic, unlike the conservative fallback — and
    /// any concrete fact about the route still beats it.
    #[test]
    fn name_suffix_hint_is_its_own_unverified_rung_below_catalog() {
        let resolved = resolve_context_window(ApiProvider::Custom, "qwen3-32b-256k", None, None);

        assert_eq!(resolved.tokens, 256_000);
        assert_eq!(resolved.source, ContextWindowSource::NameSuffixHint);
        assert!(!resolved.source.is_verified());
        assert_eq!(resolved.source.label(), "model-name hint");
        assert_eq!(
            resolved.source.display_label(),
            "model-name hint (unverified)"
        );

        // The ladder is positional and the hint sits below catalog data: an
        // offering that describes the same id wins.
        let offering = Some(RouteLimits {
            context_tokens: Some(131_072),
            ..RouteLimits::default()
        });
        let catalog = resolve_context_window(ApiProvider::Custom, "qwen3-32b-256k", offering, None);
        assert_eq!(catalog.tokens, 131_072);
        assert_eq!(catalog.source, ContextWindowSource::Catalog);
        assert!(catalog.source.is_verified());

        // An operator override beats both, exactly as before.
        let configured = resolve_context_window(
            ApiProvider::Custom,
            "qwen3-32b-256k",
            offering,
            Some(1_048_576),
        );
        assert_eq!(configured.source, ContextWindowSource::Configured);
    }

    /// #5239: an id nothing describes must land on the fallback rung and say
    /// so, rather than borrowing the configured rung's authority for a guess.
    #[test]
    fn unknown_model_resolves_to_the_honest_fallback_rung() {
        let resolved =
            resolve_context_window(ApiProvider::Custom, "private-1m-deployment-v9", None, None);

        assert_eq!(resolved.source, ContextWindowSource::Fallback);
        assert_eq!(resolved.source.label(), "fallback");
        assert!(!resolved.source.is_verified());
        assert_eq!(
            resolved.tokens,
            crate::route_budget::route_context_window_tokens(
                ApiProvider::Custom,
                "private-1m-deployment-v9",
                None,
            )
        );
    }

    /// The same unknown id with an operator override is a configured 1M route,
    /// not a 128K one — and the rung must say which of the two it is.
    #[test]
    fn configured_override_outranks_offering_limits_and_the_fallback() {
        let offering = Some(RouteLimits {
            context_tokens: Some(131_072),
            ..RouteLimits::default()
        });

        for limits in [None, offering] {
            let resolved = resolve_context_window(
                ApiProvider::Custom,
                "private-1m-deployment-v9",
                limits,
                Some(1_048_576),
            );
            assert_eq!(resolved.tokens, 1_048_576);
            assert_eq!(resolved.source, ContextWindowSource::Configured);
        }

        let catalog = resolve_context_window(
            ApiProvider::Custom,
            "private-1m-deployment-v9",
            offering,
            None,
        );
        assert_eq!(catalog.tokens, 131_072);
        assert_eq!(catalog.source, ContextWindowSource::Catalog);
    }

    /// A zero or absent override is not a configuration decision; it must not
    /// promote a guess to the configured rung.
    #[test]
    fn empty_override_and_empty_offering_stay_on_the_fallback_rung() {
        for (limits, over) in [
            (None, Some(0)),
            (
                Some(RouteLimits {
                    context_tokens: Some(0),
                    ..RouteLimits::default()
                }),
                None,
            ),
        ] {
            assert_eq!(
                resolve_context_window(
                    ApiProvider::Custom,
                    "private-1m-deployment-v9",
                    limits,
                    over
                )
                .source,
                ContextWindowSource::Fallback
            );
        }
    }

    #[test]
    fn resolved_runtime_route_keeps_large_config_off_async_stacks() {
        assert!(
            std::mem::size_of::<ResolvedRuntimeRoute>() <= 1024,
            "resolved routes cross several async boundaries and must keep Config boxed"
        );
        assert!(
            std::mem::size_of::<ResolvedRuntimeRoute>() < std::mem::size_of::<Config>(),
            "resolved routes must remain smaller than their scoped Config payload"
        );
    }

    #[test]
    fn provider_route_preflight_missing_key_error_surfaces_reason_and_auth_step() {
        let err = anyhow::anyhow!(
            "Custom provider 'lm-studio' API key not found. Run 'codewhale auth set --provider custom'."
        );
        let formatted = format_provider_route_preflight_error("lm-studio", "local-model", &err);

        assert!(formatted.starts_with("Custom provider 'lm-studio' API key not found."));
        assert!(formatted.contains("Failed to configure provider route lm-studio / local-model."));
        assert!(formatted.contains(
            "Next step: Run /auth or /provider setup lm-studio to configure credentials."
        ));
    }

    #[test]
    fn provider_route_preflight_codex_oauth_errors_surface_the_right_next_step() {
        let missing = anyhow::anyhow!("OpenAI Codex OAuth credentials are unavailable.");
        let missing_formatted =
            format_provider_route_preflight_error("openai-codex", "gpt-5.6-sol", &missing);
        assert!(missing_formatted.contains(
            "Next step: Run `codewhale auth chatgpt` or /provider setup openai-codex to Sign in with ChatGPT; Codex CLI import remains an explicit alternative."
        ));

        let custom = anyhow::anyhow!(
            "Codex OAuth credentials are only available on the official OpenAI Codex route"
        );
        let custom_formatted =
            format_provider_route_preflight_error("openai-codex", "gpt-5.6-sol", &custom);
        assert!(custom_formatted.contains(
            "Next step: Run /provider setup openai-codex and remove its custom base URL; Codex OAuth only works on the official route."
        ));
    }

    #[test]
    fn provider_route_preflight_tls_error_surfaces_route_and_setup_step() {
        let err = anyhow::anyhow!(
            "TLS certificate verification cannot be disabled for provider custom; configure SSL_CERT_FILE with a trusted custom CA bundle instead"
        );
        let formatted = format_provider_route_preflight_error("lm-studio", "local-model", &err);

        assert!(
            formatted
                .starts_with("TLS certificate verification cannot be disabled for provider custom")
        );
        assert!(formatted.contains("Failed to configure provider route lm-studio / local-model."));
        assert!(
            formatted
                .contains("Next step: Run /provider setup lm-studio to fix base URL/TLS settings.")
        );
    }

    #[test]
    fn codex_route_uses_fresh_account_context_and_drops_api_only_limits() {
        let _lock = crate::test_support::lock_test_env();
        let codex_home = tempfile::tempdir().expect("Codex home");
        let _home = crate::test_support::EnvVarGuard::set("CODEX_HOME", codex_home.path());
        std::fs::write(
            codex_home.path().join("models_cache.json"),
            serde_json::to_vec(&serde_json::json!({
                "fetched_at": chrono::Utc::now(),
                "models": [{
                    "slug": crate::config::DEFAULT_OPENAI_CODEX_MODEL,
                    "priority": 1,
                    "context_window": 128000,
                    "supported_reasoning_levels": [{"effort": "high"}]
                }]
            }))
            .expect("serialize cache"),
        )
        .expect("write cache");

        let candidate = resolve_route_candidate(
            ApiProvider::OpenaiCodex,
            Some(crate::config::DEFAULT_OPENAI_CODEX_MODEL),
            None,
            None,
            None,
        )
        .expect("Codex route");

        assert_eq!(candidate.limits().context_tokens, Some(128_000));
        assert_eq!(candidate.limits().input_tokens, None);
        assert_eq!(candidate.limits().output_tokens, None);
        assert_eq!(
            crate::route_budget::route_context_window_tokens(
                ApiProvider::OpenaiCodex,
                crate::config::DEFAULT_OPENAI_CODEX_MODEL,
                Some(candidate.limits()),
            ),
            128_000
        );
    }

    #[test]
    fn codex_switch_without_saved_model_prefers_fresh_roster_head() {
        // #5034: switching to openai-codex with no saved model must land on
        // the roster's current flagship, not the static seed constant.
        let _lock = crate::test_support::lock_test_env();
        let codex_home = tempfile::tempdir().expect("Codex home");
        let _home = crate::test_support::EnvVarGuard::set("CODEX_HOME", codex_home.path());
        std::fs::write(
            codex_home.path().join("models_cache.json"),
            serde_json::to_vec(&serde_json::json!({
                "fetched_at": chrono::Utc::now(),
                "models": [
                    {"slug": "gpt-test-flagship", "priority": 1, "context_window": 256000},
                    {"slug": crate::config::DEFAULT_OPENAI_CODEX_MODEL, "priority": 7}
                ]
            }))
            .expect("serialize cache"),
        )
        .expect("write cache");

        let config = crate::config::Config::default();
        let route = resolve_runtime_route(&config, ApiProvider::OpenaiCodex, None)
            .expect("codex route resolves");
        assert_eq!(route.model, "gpt-test-flagship");

        // An explicit selector or saved provider model still wins.
        let explicit = resolve_runtime_route(
            &config,
            ApiProvider::OpenaiCodex,
            Some(crate::config::DEFAULT_OPENAI_CODEX_MODEL),
        )
        .expect("explicit codex route resolves");
        assert_eq!(explicit.model, crate::config::DEFAULT_OPENAI_CODEX_MODEL);
    }

    #[test]
    fn opencode_go_kimi_k3_route_uses_1m_context() {
        // OpenCode Go may not own a models.dev row for kimi-k3; capability and
        // budget resolution still must use the 1M K3 contract, never the 128K
        // legacy fallback or the 131K max-output field.
        let cap = crate::config::provider_capability(ApiProvider::OpencodeGo, "kimi-k3");
        assert_eq!(cap.context_window, 1_048_576);
        assert_eq!(cap.max_output, Some(131_072));
        assert_ne!(Some(cap.context_window), cap.max_output);

        let candidate =
            resolve_route_candidate(ApiProvider::OpencodeGo, Some("kimi-k3"), None, None, None)
                .expect("OpenCode Go Kimi K3 route");
        assert_eq!(candidate.wire_model_id().as_str(), "kimi-k3");
        // Prefer catalog/route limits when present; otherwise the capability
        // path above is the source of truth for picker/budget display.
        if let Some(ctx) = candidate.limits().context_tokens {
            assert_eq!(ctx, 1_048_576);
        } else {
            assert_eq!(
                crate::route_budget::route_context_window_tokens(
                    ApiProvider::OpencodeGo,
                    "kimi-k3",
                    Some(candidate.limits()),
                ),
                1_048_576
            );
        }
    }

    #[test]
    fn direct_moonshot_k3_route_uses_documented_1m_limits_with_provenance() {
        let candidate =
            resolve_route_candidate(ApiProvider::Moonshot, Some("kimi-k3"), None, None, None)
                .expect("Moonshot Kimi K3 route");

        assert_eq!(candidate.wire_model_id().as_str(), "kimi-k3");
        assert_eq!(candidate.limits().context_tokens, Some(1_048_576));
        assert_eq!(candidate.limits().output_tokens, Some(1_048_576));
        assert!(candidate.applied_limit_overrides().contains(
            &codewhale_config::route::SourcedLimitOverride {
                field: codewhale_config::route::LimitField::OutputTokens,
                value: Some(1_048_576),
                source: codewhale_config::route::OverrideSource::DocumentedRouteOutputMaximum,
            }
        ));
        assert_eq!(
            crate::route_budget::route_context_window_tokens(
                ApiProvider::Moonshot,
                "kimi-k3",
                Some(candidate.limits()),
            ),
            1_048_576
        );
        assert_eq!(
            crate::route_budget::effective_max_output_tokens_for_route(
                ApiProvider::Moonshot,
                "kimi-k3",
                Some(candidate.limits()),
            ),
            65_536,
            "the documented catalogue output ceiling remains a ceiling; the safe default request must not reserve it in full"
        );
    }

    #[test]
    fn kimi_code_bare_k3_keeps_tier_safe_floor_not_legacy_128k() {
        // Bare `k3` membership context is plan-tier dependent (256K on lower
        // tiers, up to 1M on higher ones), so the static route baseline stays
        // the safe floor. Higher entitlements come from an explicit provider
        // `context_window` override — never from assuming the top tier, and
        // never from the 128K legacy default.
        let candidate = resolve_route_candidate(
            ApiProvider::Moonshot,
            Some("k3"),
            None,
            Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
            None,
        )
        .expect("Kimi Code K3 route");

        assert_eq!(candidate.wire_model_id().as_str(), "k3");
        assert_eq!(candidate.limits().context_tokens, Some(262_144));
        // Output remains a conservative generic default because the
        // membership API does not publish a distinct maximum. Never project
        // it as context or inherit the direct-platform 1M maximum.
        assert_ne!(
            candidate.limits().context_tokens,
            candidate.limits().output_tokens
        );
        assert_eq!(
            crate::config::provider_capability(
                ApiProvider::Moonshot,
                crate::config::KIMI_CODE_K3_MODEL
            )
            .context_window,
            262_144
        );
        assert_ne!(candidate.limits().output_tokens, Some(1_048_576));
    }

    #[test]
    fn kimi_code_context_resolution_records_precedence_and_rejects_bad_metadata() {
        let base = Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string());
        let static_floor = resolve_route_candidate_with_context_metadata(
            ApiProvider::Moonshot,
            Some("k3"),
            None,
            base.clone(),
            None,
            None,
        )
        .expect("Kimi Code route");
        assert_eq!(static_floor.context_window.tokens, 262_144);
        assert_eq!(
            static_floor.context_window.source,
            ContextWindowSource::StaticKimiCodeSafeFloor
        );

        let configured = resolve_route_candidate_with_context_metadata(
            ApiProvider::Moonshot,
            Some("k3"),
            None,
            base.clone(),
            Some(1_048_576),
            Some(ProviderReportedKimiCodeContext {
                context_tokens: 1_048_576,
                observed_at: Utc::now(),
            }),
        )
        .expect("configured route");
        assert_eq!(configured.context_window.tokens, 1_048_576);
        assert_eq!(
            configured.context_window.source,
            ContextWindowSource::Configured
        );

        let reported = resolve_route_candidate_with_context_metadata(
            ApiProvider::Moonshot,
            Some("k3"),
            None,
            base.clone(),
            None,
            Some(ProviderReportedKimiCodeContext {
                context_tokens: 1_048_576,
                observed_at: Utc::now(),
            }),
        )
        .expect("fresh documented provider metadata");
        assert_eq!(reported.context_window.tokens, 1_048_576);
        assert_eq!(
            reported.context_window.source,
            ContextWindowSource::ProviderReported
        );

        let stale = resolve_route_candidate_with_context_metadata(
            ApiProvider::Moonshot,
            Some("k3"),
            None,
            base,
            None,
            Some(ProviderReportedKimiCodeContext {
                context_tokens: 1_048_576,
                observed_at: Utc::now() - Duration::hours(25),
            }),
        )
        .expect("stale metadata falls back safely");
        assert_eq!(
            stale.context_window.source,
            ContextWindowSource::StaticKimiCodeSafeFloor
        );

        let generic_err = resolve_route_candidate_with_context_metadata(
            ApiProvider::Moonshot,
            Some("k3"),
            None,
            Some(crate::config::DEFAULT_MOONSHOT_BASE_URL.to_string()),
            None,
            Some(ProviderReportedKimiCodeContext {
                context_tokens: 1_048_576,
                observed_at: Utc::now(),
            }),
        )
        .expect_err("bare k3 is rejected on the direct Moonshot endpoint (#4687)");
        assert!(
            generic_err.contains("kimi-k3"),
            "error should guide the user to kimi-k3: {generic_err}"
        );
    }

    #[test]
    fn kimi_code_k3_context_override_wins_over_conservative_baseline() {
        let candidate = resolve_route_candidate(
            ApiProvider::Moonshot,
            Some("k3"),
            None,
            Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
            Some(1_048_576),
        )
        .expect("Kimi Code K3 route");

        assert_eq!(
            candidate.wire_model_id().as_str(),
            crate::config::KIMI_CODE_K3_MODEL,
            "the 1M entitlement changes limits, never the provider wire id"
        );
        assert!(crate::config::is_exact_kimi_code_bare_k3_route(
            ApiProvider::Moonshot,
            &candidate.endpoint().base_url,
            candidate.wire_model_id().as_str(),
        ));
        assert_eq!(candidate.limits().context_tokens, Some(1_048_576));
    }

    #[test]
    fn kimi_code_rejects_claude_only_k3_1m_alias_for_selected_and_saved_models() {
        for (selected, saved) in [(Some("k3[1m]"), None), (None, Some("k3[1m]"))] {
            let error = resolve_route_candidate(
                ApiProvider::Moonshot,
                selected,
                saved,
                Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                None,
            )
            .expect_err("Claude Code's context hint is not a Kimi Code API model id");

            assert!(error.contains("model = \"k3\""), "{error}");
            assert!(error.contains("context_window = 1048576"), "{error}");
            assert!(error.contains("plan includes 1M context"), "{error}");
            assert!(error.contains("262144 safe default"), "{error}");
        }
    }

    #[test]
    fn k3_route_rejects_cross_paired_model_ids_and_allows_canonical_pairs() {
        use crate::config::{
            DEFAULT_KIMI_CODE_BASE_URL, DEFAULT_MOONSHOT_BASE_URL, KIMI_CODE_K3_MODEL,
            MOONSHOT_KIMI_K3_MODEL, moonshot_k3_route_display_name,
            validate_kimi_code_api_model_id,
        };

        // Canonical pairs succeed.
        validate_kimi_code_api_model_id(
            ApiProvider::Moonshot,
            DEFAULT_KIMI_CODE_BASE_URL,
            KIMI_CODE_K3_MODEL,
        )
        .expect("kimi code + k3");
        validate_kimi_code_api_model_id(
            ApiProvider::Moonshot,
            DEFAULT_MOONSHOT_BASE_URL,
            MOONSHOT_KIMI_K3_MODEL,
        )
        .expect("direct + kimi-k3");

        // Trailing slash normalization still enforces.
        let err = validate_kimi_code_api_model_id(
            ApiProvider::Moonshot,
            "https://api.kimi.com/coding/v1/",
            "kimi-k3",
        )
        .expect_err("kimi code + kimi-k3");
        assert!(err.contains("k3"), "{err}");
        assert!(err.contains("kimi-k3"), "{err}");

        let err = validate_kimi_code_api_model_id(
            ApiProvider::Moonshot,
            "https://api.moonshot.ai/v1/",
            "k3",
        )
        .expect_err("direct + k3");
        assert!(err.contains("kimi-k3"), "{err}");

        // Custom gateway is not rejected for either model id.
        validate_kimi_code_api_model_id(
            ApiProvider::Moonshot,
            "https://gateway.example.com/v1",
            "k3",
        )
        .expect("custom + k3");
        validate_kimi_code_api_model_id(
            ApiProvider::Moonshot,
            "https://gateway.example.com/v1",
            "kimi-k3",
        )
        .expect("custom + kimi-k3");

        // Runtime resolve fails closed the same way.
        let err = resolve_route_candidate(
            ApiProvider::Moonshot,
            Some("kimi-k3"),
            None,
            Some(DEFAULT_KIMI_CODE_BASE_URL.to_string()),
            None,
        )
        .expect_err("resolve kimi code + kimi-k3");
        assert!(err.contains("k3"), "{err}");

        let err = resolve_route_candidate(
            ApiProvider::Moonshot,
            Some("k3"),
            None,
            Some(DEFAULT_MOONSHOT_BASE_URL.to_string()),
            None,
        )
        .expect_err("resolve direct + k3");
        assert!(err.contains("kimi-k3"), "{err}");

        assert_eq!(
            moonshot_k3_route_display_name(DEFAULT_KIMI_CODE_BASE_URL, "k3"),
            Some("Kimi Code membership / k3")
        );
        assert_eq!(
            moonshot_k3_route_display_name(DEFAULT_MOONSHOT_BASE_URL, "kimi-k3"),
            Some("Moonshot direct / kimi-k3")
        );
    }

    #[test]
    fn kimi_code_k3_baseline_does_not_leak_to_other_moonshot_routes() {
        let kimi_code_endpoint = Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string());
        let direct_moonshot = resolve_route_candidate(
            ApiProvider::Moonshot,
            Some(crate::config::MOONSHOT_KIMI_K3_MODEL),
            None,
            Some(crate::config::DEFAULT_MOONSHOT_BASE_URL.to_string()),
            None,
        )
        .expect("direct Moonshot K3 route");
        assert_eq!(direct_moonshot.limits().context_tokens, Some(1_048_576));

        // Bare k3 on the direct platform endpoint is fail-closed (#4687).
        let generic_err = resolve_route_candidate(
            ApiProvider::Moonshot,
            Some("k3"),
            None,
            Some(crate::config::DEFAULT_MOONSHOT_BASE_URL.to_string()),
            None,
        )
        .expect_err("bare k3 on direct Moonshot must fail closed");
        assert!(generic_err.contains("kimi-k3"), "{generic_err}");

        // A non-K3 direct model must not inherit the Kimi Code 262k floor.
        let generic_moonshot = resolve_route_candidate(
            ApiProvider::Moonshot,
            Some("moonshot-v1-128k"),
            None,
            Some(crate::config::DEFAULT_MOONSHOT_BASE_URL.to_string()),
            None,
        )
        .expect("generic Moonshot route");
        assert_ne!(generic_moonshot.limits().context_tokens, Some(262_144));

        let kimi_code_default = resolve_route_candidate(
            ApiProvider::Moonshot,
            Some(crate::config::DEFAULT_KIMI_CODE_MODEL),
            None,
            kimi_code_endpoint,
            None,
        )
        .expect("Kimi Code default route");
        assert_ne!(kimi_code_default.limits().context_tokens, Some(262_144));
    }

    #[test]
    fn runtime_route_without_model_uses_target_provider_default() {
        let config = Config {
            provider: Some("openrouter".to_string()),
            providers: Some(ProvidersConfig {
                openrouter: ProviderConfig {
                    model: Some("deepseek/deepseek-v4-pro".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let route = resolve_runtime_route(&config, ApiProvider::Zai, None)
            .expect("target provider default should resolve");

        assert_eq!(route.model, DEFAULT_ZAI_MODEL);
        assert_eq!(route.config.provider.as_deref(), Some("zai"));
        assert_eq!(
            route
                .config
                .providers
                .as_ref()
                .and_then(|providers| providers.zai.model.as_deref()),
            Some(DEFAULT_ZAI_MODEL)
        );
        assert_eq!(
            route
                .config
                .providers
                .as_ref()
                .and_then(|providers| providers.openrouter.model.as_deref()),
            Some("deepseek/deepseek-v4-pro")
        );
    }

    #[test]
    fn runtime_route_rejects_foreign_direct_model_before_config_snapshot() {
        let config = Config {
            provider: Some("deepseek".to_string()),
            providers: Some(ProvidersConfig {
                deepseek: ProviderConfig {
                    model: Some(DEFAULT_TEXT_MODEL.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let err = resolve_runtime_route(&config, ApiProvider::Zai, Some("deepseek-v4-pro"))
            .expect_err("foreign direct-provider model should reject");

        assert!(err.contains("not served by direct provider zai"));
        assert_eq!(config.provider.as_deref(), Some("deepseek"));
        assert_eq!(
            config
                .providers
                .as_ref()
                .and_then(|providers| providers.zai.model.as_deref()),
            None
        );
    }

    #[test]
    fn unpinned_spawn_route_is_conservative_and_returns_exact_wire_id() {
        let err = resolve_unpinned_model_candidate(
            ApiProvider::Moonshot,
            "deepseek-v4-pro",
            ApiProvider::Moonshot.default_base_url(),
        )
        .expect_err("official Moonshot cannot inherit a DeepSeek-owned pin");
        assert!(err.contains("deepseek-v4-pro"), "names model: {err}");
        assert!(err.contains("moonshot"), "names route: {err}");
        assert!(err.contains("deepseek"), "names owner: {err}");

        let openrouter = resolve_unpinned_model_candidate(
            ApiProvider::Openrouter,
            "deepseek-v4-pro",
            ApiProvider::Openrouter.default_base_url(),
        )
        .expect("aggregator alias should resolve offline");
        assert_eq!(
            openrouter.wire_model_id().as_str(),
            crate::config::DEFAULT_OPENROUTER_MODEL,
        );

        let vllm = resolve_unpinned_model_candidate(
            ApiProvider::Vllm,
            "deepseek-v4-pro",
            ApiProvider::Vllm.default_base_url(),
        )
        .expect("local runtime model ids stay provider-authoritative");
        assert!(!vllm.wire_model_id().as_str().is_empty());

        let custom = resolve_unpinned_model_candidate(
            ApiProvider::Moonshot,
            "deepseek-v4-pro",
            "https://gateway.example.test/v1",
        )
        .expect("a custom endpoint owns its model namespace");
        assert_eq!(custom.wire_model_id().as_str(), "deepseek-v4-pro");
    }

    fn custom_config(base_url: &str, model: &str) -> Config {
        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "my_thing".to_string(),
            ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some(base_url.to_string()),
                model: Some(model.to_string()),
                api_key_env: Some("EXAMPLE_API_KEY".to_string()),
                ..Default::default()
            },
        );
        Config {
            provider: Some("my_thing".to_string()),
            providers: Some(ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn custom_provider_resolves_to_custom_endpoint_and_verbatim_model() {
        use codewhale_config::route::RequestProtocol;

        let config = custom_config("https://api.example.com/v1", "vendor/custom-model-v1");
        let route = resolve_runtime_route(&config, ApiProvider::Custom, None)
            .expect("custom provider should resolve");

        // Endpoint + model come from the named table; the prefixed model id is
        // preserved verbatim as the wire id (no provider-prefix sniffing).
        assert_eq!(
            route.candidate.endpoint().base_url,
            "https://api.example.com/v1"
        );
        assert_eq!(
            route.candidate.wire_model_id().as_str(),
            "vendor/custom-model-v1"
        );
        assert_eq!(route.model, "vendor/custom-model-v1");
        assert_eq!(route.candidate.protocol(), RequestProtocol::ChatCompletions);
        // HTTPS endpoint: route is valid with no insecure-http advisory.
        assert!(route.candidate.validation().ok);
        assert!(route.candidate.validation().messages.is_empty());
        // The selected provider name is preserved (not overwritten with "custom").
        assert_eq!(route.config.provider.as_deref(), Some("my_thing"));
    }

    #[test]
    fn custom_provider_context_window_overrides_unknown_route_limit() {
        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "dashscope".to_string(),
            ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("https://dashscope.example.com/compatible-mode/v1".to_string()),
                model: Some("qwen3.7".to_string()),
                context_window: Some(1_000_000),
                api_key_env: Some("DASHSCOPE_API_KEY".to_string()),
                ..Default::default()
            },
        );
        let config = Config {
            provider: Some("dashscope".to_string()),
            providers: Some(ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Config::default()
        };

        let route = resolve_runtime_route(&config, ApiProvider::Custom, None)
            .expect("custom route should resolve");

        assert_eq!(route.model, "qwen3.7");
        assert_eq!(route.candidate.limits().context_tokens, Some(1_000_000));
    }

    #[test]
    fn custom_provider_http_non_loopback_fires_insecure_advisory() {
        let config = custom_config("http://gpu.internal.example:8000/v1", "custom-model-v1");
        let route = resolve_runtime_route(&config, ApiProvider::Custom, None)
            .expect("custom http provider should resolve");

        // Advisory only: the route still validates (ok == true) but warns that
        // credentials would be sent in plaintext over a non-loopback http URL.
        assert!(route.candidate.validation().ok);
        assert!(
            route
                .candidate
                .validation()
                .messages
                .iter()
                .any(|message| message.contains("insecure http")),
            "expected insecure-http advisory, got {:?}",
            route.candidate.validation().messages
        );
        assert_eq!(
            route.candidate.endpoint().base_url,
            "http://gpu.internal.example:8000/v1"
        );
    }
}
