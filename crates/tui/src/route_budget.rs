use codewhale_config::route::RouteLimits;

use crate::config::{ApiProvider, provider_capability};
use crate::context_budget::ContextBudget;
use crate::models::{
    DEFAULT_AUTO_COMPACT_MAX_CONTEXT_WINDOW_TOKENS, DEFAULT_COMPACTION_TOKEN_THRESHOLD,
    context_window_for_model,
};

/// Output room reserved by the internal budget for large-context reasoning
/// models. This is deliberately larger than the ordinary API request cap so
/// interleaved thinking cannot exhaust the turn budget.
pub(crate) const TURN_MAX_OUTPUT_TOKENS: u32 = 262_144;

/// Safe ordinary API request cap across provider routes.
const API_MAX_OUTPUT_TOKENS: u32 = 65_536;

/// Large windows reserve the full internal reasoning allowance. Smaller
/// windows reserve their route-effective request cap instead.
const INTERNAL_BUDGET_LARGE_WINDOW_THRESHOLD: u32 = 500_000;

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

/// Effective `max_tokens` for a model before provider/route caps are applied.
#[must_use]
pub(crate) fn effective_max_output_tokens(model: &str) -> u32 {
    if let Ok(raw) = std::env::var("DEEPSEEK_MAX_OUTPUT_TOKENS")
        && let Ok(tokens) = raw.trim().parse::<u32>()
        && tokens > 0
    {
        return tokens;
    }

    let window = context_window_for_model(model).unwrap_or(128_000);
    if window >= INTERNAL_BUDGET_LARGE_WINDOW_THRESHOLD {
        API_MAX_OUTPUT_TOKENS
    } else {
        (window / 2).min(API_MAX_OUTPUT_TOKENS)
    }
}

/// Effective request output cap for a fully resolved provider/model route.
#[must_use]
pub(crate) fn effective_max_output_tokens_for_route(
    provider: ApiProvider,
    model: &str,
    route_limits: Option<RouteLimits>,
) -> u32 {
    let cap =
        effective_max_output_tokens(model).min(provider_capability(provider, model).max_output);
    let cap = route_output_limit_tokens(route_limits).map_or(cap, |route_cap| cap.min(route_cap));
    let Some(window) = route_limits
        .and_then(|limits| limits.context_tokens)
        .and_then(|tokens| u32::try_from(tokens).ok())
        .filter(|tokens| *tokens > 0)
    else {
        return cap;
    };

    u32::try_from(ContextBudget::new(u64::from(window), 0, u64::from(cap)).output_cap_tokens)
        .unwrap_or(cap)
        .max(1)
}

/// Output reservation used by the internal input budget for a route.
#[must_use]
pub(crate) fn route_output_reservation_for_window(
    provider: ApiProvider,
    model: &str,
    window_tokens: u32,
    route_limits: Option<RouteLimits>,
) -> u32 {
    if window_tokens >= INTERNAL_BUDGET_LARGE_WINDOW_THRESHOLD {
        route_output_limit_tokens(route_limits).map_or(TURN_MAX_OUTPUT_TOKENS, |route_cap| {
            route_cap.min(TURN_MAX_OUTPUT_TOKENS)
        })
    } else {
        effective_max_output_tokens_for_route(provider, model, route_limits)
    }
}

#[must_use]
pub(crate) fn route_context_budget(
    provider: ApiProvider,
    model: &str,
    route_limits: Option<RouteLimits>,
    input_tokens: usize,
) -> Option<ContextBudget> {
    let window = route_context_window_tokens(provider, model, route_limits);
    let output_cap = route_output_reservation_for_window(provider, model, window, route_limits);
    Some(ContextBudget::new(
        u64::from(window),
        u64::try_from(input_tokens).ok()?,
        u64::from(output_cap),
    ))
}

#[must_use]
pub(crate) fn compaction_threshold_for_route_at_percent(
    provider: ApiProvider,
    model: &str,
    route_limits: Option<RouteLimits>,
    percent: f64,
) -> usize {
    route_context_budget(provider, model, route_limits, 0)
        .and_then(|budget| {
            usize::try_from(budget.compaction_trigger_for_percent(percent.clamp(10.0, 100.0))).ok()
        })
        .unwrap_or(DEFAULT_COMPACTION_TOKEN_THRESHOLD)
}

#[must_use]
pub(crate) fn auto_compact_default_for_route(
    provider: ApiProvider,
    model: &str,
    route_limits: Option<RouteLimits>,
) -> bool {
    route_context_window_tokens(provider, model, route_limits)
        <= DEFAULT_AUTO_COMPACT_MAX_CONTEXT_WINDOW_TOKENS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_missing_route_metadata_uses_provider_context_floor() {
        assert_eq!(
            route_context_window_tokens(ApiProvider::OpenaiCodex, "gpt-5.5", None),
            128_000
        );
        assert_eq!(
            compaction_threshold_for_route_at_percent(
                ApiProvider::OpenaiCodex,
                "gpt-5.5",
                None,
                80.0,
            ),
            98_304
        );
        assert!(auto_compact_default_for_route(
            ApiProvider::OpenaiCodex,
            "gpt-5.5",
            None,
        ));
    }

    #[test]
    fn v4_trigger_is_anchored_to_spendable_input() {
        let budget = route_context_budget(ApiProvider::Deepseek, "deepseek-v4-pro", None, 0)
            .expect("V4 route budget");

        assert_eq!(budget.window_tokens, 1_000_000);
        assert_eq!(budget.output_cap_tokens, u64::from(TURN_MAX_OUTPUT_TOKENS));
        assert_eq!(budget.input_budget_ceiling, 736_832);
        assert_eq!(
            compaction_threshold_for_route_at_percent(
                ApiProvider::Deepseek,
                "deepseek-v4-pro",
                None,
                80.0,
            ),
            589_466
        );
    }

    #[test]
    fn kimi_catalog_output_ceiling_preserves_input_budget() {
        let _lock = crate::test_support::lock_test_env();
        let _max_output = crate::test_support::EnvVarGuard::remove("DEEPSEEK_MAX_OUTPUT_TOKENS");
        // #4368/#4378: Models.dev may report Kimi's full 262K context as both
        // context and output ceilings. On a sub-500K window, reserve the
        // route-effective 32K request cap rather than treating that catalog
        // maximum as the amount every turn will emit.
        let limits = RouteLimits {
            context_tokens: Some(262_144),
            output_tokens: Some(262_144),
            ..RouteLimits::default()
        };
        let budget = route_context_budget(ApiProvider::Moonshot, "kimi-k2.7-code", Some(limits), 0)
            .expect("Kimi route budget");
        let trigger = compaction_threshold_for_route_at_percent(
            ApiProvider::Moonshot,
            "kimi-k2.7-code",
            Some(limits),
            80.0,
        );

        assert_eq!(budget.output_cap_tokens, 32_768);
        assert_eq!(budget.input_budget_ceiling, 228_352);
        assert_eq!(trigger, 182_682);
        assert!(trigger as u64 <= budget.input_budget_ceiling);
        assert!(
            trigger < 209_715,
            "must fire before the old window-relative trigger"
        );
    }
}
