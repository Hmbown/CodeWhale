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

/// Build a [`ContextBudget`] from an already-resolved context window.
///
/// `window_tokens` must be the same value the caller used to derive
/// `configured_output_cap` (typically via
/// [`route_context_window_tokens`]); passing it in here avoids recomputing
/// the window a second time and keeps the output/input reservation
/// consistent with it.
#[must_use]
pub(crate) fn route_context_budget(
    window_tokens: u32,
    input_tokens: usize,
    configured_output_cap: u32,
) -> Option<ContextBudget> {
    Some(ContextBudget::new(
        u64::from(window_tokens),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::lock_test_env;

    #[test]
    fn known_route_limits_drops_empty_and_keeps_populated() {
        let _lock = lock_test_env();
        // No concrete limit at all -> not a known route.
        assert!(
            known_route_limits(RouteLimits {
                context_tokens: None,
                input_tokens: None,
                output_tokens: None,
            })
            .is_none()
        );
        // Any single known field is enough.
        assert!(
            known_route_limits(RouteLimits {
                context_tokens: None,
                input_tokens: None,
                output_tokens: Some(8_192),
            })
            .is_some()
        );
    }

    #[test]
    fn compaction_threshold_for_route_clamps_percent_and_scales_to_window() {
        let _lock = lock_test_env();
        let limits = RouteLimits {
            context_tokens: Some(200_000),
            input_tokens: None,
            output_tokens: None,
        };

        // 80% of a 200K window.
        assert_eq!(
            compaction_threshold_for_route_at_percent(
                ApiProvider::Deepseek,
                "deepseek-v4-pro",
                Some(limits),
                80.0,
            ),
            160_000
        );
        // Percent below the floor is clamped to 10%.
        assert_eq!(
            compaction_threshold_for_route_at_percent(
                ApiProvider::Deepseek,
                "deepseek-v4-pro",
                Some(limits),
                1.0,
            ),
            20_000
        );
        // Percent above the ceiling is clamped to 100%.
        assert_eq!(
            compaction_threshold_for_route_at_percent(
                ApiProvider::Deepseek,
                "deepseek-v4-pro",
                Some(limits),
                250.0,
            ),
            200_000
        );
    }

    #[test]
    fn auto_compact_default_for_route_splits_at_large_window_threshold() {
        let _lock = lock_test_env();
        // A route window under the threshold keeps auto-compaction on.
        let small = RouteLimits {
            context_tokens: Some(u64::from(DEFAULT_AUTO_COMPACT_MAX_CONTEXT_WINDOW_TOKENS)),
            input_tokens: None,
            output_tokens: None,
        };
        assert!(auto_compact_default_for_route(
            ApiProvider::Deepseek,
            "deepseek-v4-pro",
            Some(small),
        ));

        // A route window above the threshold turns auto-compaction off, even
        // though the bare model would otherwise default it on.
        let large = RouteLimits {
            context_tokens: Some(u64::from(DEFAULT_AUTO_COMPACT_MAX_CONTEXT_WINDOW_TOKENS) + 1),
            input_tokens: None,
            output_tokens: None,
        };
        assert!(!auto_compact_default_for_route(
            ApiProvider::Deepseek,
            "deepseek-v4-pro",
            Some(large),
        ));

        // No route facts -> falls back to the model matrix.
        assert_eq!(
            auto_compact_default_for_route(ApiProvider::Deepseek, "deepseek-v4-pro", None),
            crate::models::auto_compact_default_for_model("deepseek-v4-pro"),
        );
    }

    #[test]
    fn route_context_budget_uses_passed_window_directly() {
        let _lock = lock_test_env();
        let budget =
            route_context_budget(128_000, 60_000, 32_768).expect("budget should be constructed");
        assert_eq!(budget.window_tokens, 128_000);
        assert_eq!(budget.output_cap_tokens, 32_768);
        assert_eq!(budget.available_input_tokens, 34_208);
    }
}
