use codewhale_config::route::RouteLimits;

use crate::config::{ApiProvider, provider_capability};
use crate::context_budget::ContextBudget;
use crate::models::{
    DEFAULT_AUTO_COMPACT_MAX_CONTEXT_WINDOW_TOKENS, DEFAULT_COMPACTION_TOKEN_THRESHOLD,
    compaction_threshold_for_model_at_percent,
};

/// Preserve only route limits that came from a concrete offering.
#[must_use]
pub(crate) fn known_route_limits(limits: RouteLimits) -> Option<RouteLimits> {
    limits.has_known_limit().then_some(limits)
}

/// Context window for a resolved runtime route.
///
/// Route/offering facts win when known; otherwise this falls back to the
/// existing provider+model capability matrix so startup and custom/local
/// routes keep their previous conservative behavior.
#[must_use]
pub(crate) fn route_context_window_tokens(
    provider: ApiProvider,
    model: &str,
    route_limits: Option<RouteLimits>,
) -> u32 {
    route_limits
        .and_then(|limits| limits.context_tokens)
        .and_then(|tokens| u32::try_from(tokens).ok())
        .filter(|tokens| *tokens > 0)
        .unwrap_or_else(|| provider_capability(provider, model).context_window)
}

/// Provider/offering output cap, when the resolved route reports one.
#[must_use]
pub(crate) fn route_output_limit_tokens(route_limits: Option<RouteLimits>) -> Option<u32> {
    route_limits
        .and_then(|limits| limits.output_tokens)
        .and_then(|tokens| u32::try_from(tokens).ok())
        .filter(|tokens| *tokens > 0)
}

#[must_use]
pub(crate) fn route_context_budget(
    provider: ApiProvider,
    model: &str,
    route_limits: Option<RouteLimits>,
    input_tokens: usize,
    configured_output_cap: u32,
) -> Option<ContextBudget> {
    let window = route_context_window_tokens(provider, model, route_limits);
    Some(ContextBudget::new(
        u64::from(window),
        u64::try_from(input_tokens).ok()?,
        u64::from(configured_output_cap),
    ))
}

#[must_use]
pub(crate) fn compaction_threshold_for_route_at_percent(
    provider: ApiProvider,
    model: &str,
    route_limits: Option<RouteLimits>,
    percent: f64,
) -> usize {
    if route_limits
        .and_then(|limits| limits.context_tokens)
        .is_some()
    {
        let window = route_context_window_tokens(provider, model, route_limits);
        let percent = percent.clamp(10.0, 100.0);
        let threshold = (f64::from(window) * percent / 100.0).round();
        let threshold = if threshold.is_finite() && threshold > 0.0 {
            threshold as u64
        } else {
            return DEFAULT_COMPACTION_TOKEN_THRESHOLD;
        };
        return usize::try_from(threshold).unwrap_or(DEFAULT_COMPACTION_TOKEN_THRESHOLD);
    }

    compaction_threshold_for_model_at_percent(model, percent)
}

#[must_use]
pub(crate) fn auto_compact_default_for_route(
    provider: ApiProvider,
    model: &str,
    route_limits: Option<RouteLimits>,
) -> bool {
    if route_limits
        .and_then(|limits| limits.context_tokens)
        .is_some()
    {
        return route_context_window_tokens(provider, model, route_limits)
            <= DEFAULT_AUTO_COMPACT_MAX_CONTEXT_WINDOW_TOKENS;
    }

    crate::models::auto_compact_default_for_model(model)
}
