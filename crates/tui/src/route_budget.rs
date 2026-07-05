use codewhale_config::HarnessCompactionStrategy;
use codewhale_config::route::RouteLimits;

use crate::config::{ApiProvider, provider_capability};
use crate::context_budget::ContextBudget;
use crate::models::{
    DEFAULT_AUTO_COMPACT_MAX_CONTEXT_WINDOW_TOKENS, DEFAULT_COMPACTION_TOKEN_THRESHOLD,
    context_window_for_model,
};

/// Percentage points added to the base auto-compact threshold for
/// [`HarnessCompactionStrategy::PrefixCache`] (compact later, preserve prefix cache).
pub(crate) const PREFIX_CACHE_PERCENT_DELTA: f64 = 8.0;
/// Percentage points subtracted from the base threshold for
/// [`HarnessCompactionStrategy::Aggressive`] (compact earlier).
pub(crate) const AGGRESSIVE_PERCENT_DELTA: f64 = 12.0;
/// Upper clamp for posture-adjusted compaction trigger percentages.
pub(crate) const COMPACTION_PCT_CEILING: f64 = 95.0;
/// Lower clamp for posture-adjusted compaction trigger percentages.
pub(crate) const COMPACTION_PCT_FLOOR: f64 = 50.0;

/// Shift the compaction trigger percent for a harness compaction strategy.
///
/// `Default` returns `base_pct` unchanged (zero-regression identity).
/// `PrefixCache` raises it (compact later, preserve the V4 prefix cache);
/// `Aggressive` lowers it (compact earlier). Deltas are named constants and
/// the result is clamped to a safe band; the downstream trigger is already
/// anchored to the input ceiling, so a higher percent never overflows.
#[must_use]
pub(crate) fn threshold_pct_for_strategy(
    strategy: HarnessCompactionStrategy,
    base_pct: f64,
) -> f64 {
    match strategy {
        HarnessCompactionStrategy::Default => base_pct,
        HarnessCompactionStrategy::PrefixCache => {
            (base_pct + PREFIX_CACHE_PERCENT_DELTA).min(COMPACTION_PCT_CEILING)
        }
        HarnessCompactionStrategy::Aggressive => {
            (base_pct - AGGRESSIVE_PERCENT_DELTA).max(COMPACTION_PCT_FLOOR)
        }
    }
}

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

/// Max output tokens requested for normal agent turns. Generous on purpose:
/// V4 thinking models can produce tens of thousands of reasoning tokens on
/// hard prompts before the visible reply, and DeepSeek V4 ships with a 1M
/// context window. The cap is fixed instead of silently lowered near pressure;
/// hard-cycle/preflight checks reserve this budget plus safety headroom before
/// sending the next request.
pub(crate) const TURN_MAX_OUTPUT_TOKENS: u32 = 262_144;

/// Safe max output tokens sent in the API request. This must be low enough to
/// work with providers that have smaller context limits than the model's native
/// window (e.g. self-hosted vLLM/SGLang with `--max-model-len 131072`). DeepSeek's
/// API still produces as many tokens as needed for thinking; this cap just
/// prevents HTTP 400 from providers with tight limits.
const API_MAX_OUTPUT_TOKENS: u32 = 65_536;

/// Context windows at or above this size reserve the full
/// [`TURN_MAX_OUTPUT_TOKENS`] (262K), leaving room for V4-class interleaved
/// thinking. Below it, the reservation falls back to
/// [`effective_max_output_tokens`] so a smaller self-hosted window does not
/// underflow to a negative budget.
const INTERNAL_BUDGET_LARGE_WINDOW_THRESHOLD: u32 = 500_000;

/// Compute the effective `max_tokens` to send in the API request for a given
/// model. Uses [`API_MAX_OUTPUT_TOKENS`] (64K) which fits within common provider
/// limits; for non-V4 models with smaller windows, caps at half the window.
///
/// Override: when `DEEPSEEK_MAX_OUTPUT_TOKENS` is set to a positive integer this
/// returns that value directly, for self-hosted providers whose `max-model-len`
/// is tight and where the model-table heuristic would over-allocate.
#[must_use]
pub(crate) fn effective_max_output_tokens(model: &str) -> u32 {
    if let Ok(raw) = std::env::var("DEEPSEEK_MAX_OUTPUT_TOKENS")
        && let Ok(n) = raw.trim().parse::<u32>()
        && n > 0
    {
        return n;
    }
    let window = context_window_for_model(model).unwrap_or(128_000);
    if window >= 500_000 {
        // V4-class models on large-context providers: 64K is safe for most
        // deployments while still allowing substantial output.
        API_MAX_OUTPUT_TOKENS
    } else {
        // Smaller models: cap at half the context window (leave room for input).
        let capped = window / 2;
        capped.min(API_MAX_OUTPUT_TOKENS)
    }
}

/// Output tokens reserved when computing a route's input budget ceiling. The
/// reserved term is window-dependent so a tight self-hosted window does not
/// reserve more than it has:
///   * a resolved route output cap wins (clamped to [`TURN_MAX_OUTPUT_TOKENS`]);
///   * `window >= 500K` (V4-class) reserves the full [`TURN_MAX_OUTPUT_TOKENS`];
///   * `window < 500K` reserves [`effective_max_output_tokens`], i.e. what the
///     API actually caps output at — reserving the full 262K on a 256K window
///     would underflow the budget and silently disable preflight/recovery.
#[must_use]
pub(crate) fn route_output_reservation_for_window(
    model: &str,
    window_tokens: u32,
    route_limits: Option<RouteLimits>,
) -> u32 {
    if let Some(route_cap) = route_output_limit_tokens(route_limits) {
        return route_cap.min(TURN_MAX_OUTPUT_TOKENS);
    }
    if window_tokens >= INTERNAL_BUDGET_LARGE_WINDOW_THRESHOLD {
        TURN_MAX_OUTPUT_TOKENS
    } else {
        effective_max_output_tokens(model)
    }
}

/// Compaction trigger (input tokens) for a fully-resolved provider/model route.
///
/// Builds the route's [`ContextBudget`] — reserving output first — and reads the
/// ceiling-anchored trigger, so the threshold can never sit above the room input
/// may actually occupy. This is the single source of truth for the route trigger;
/// callers no longer compute their own `percent × window`.
#[must_use]
pub(crate) fn compaction_threshold_for_route_at_percent(
    provider: ApiProvider,
    model: &str,
    route_limits: Option<RouteLimits>,
    percent: f64,
) -> usize {
    let window = route_context_window_tokens(provider, model, route_limits);
    if window == 0 {
        return DEFAULT_COMPACTION_TOKEN_THRESHOLD;
    }
    let reservation = route_output_reservation_for_window(model, window, route_limits);
    let budget = ContextBudget::new(u64::from(window), 0, u64::from(reservation));
    usize::try_from(budget.compaction_trigger_for_percent(percent))
        .unwrap_or(DEFAULT_COMPACTION_TOKEN_THRESHOLD)
}

/// Compaction trigger (input tokens) for a model id without a resolved route
/// (e.g. background runtime threads).
///
/// Mirrors [`compaction_threshold_for_route_at_percent`] using the model's known
/// window and a no-route reservation, so headless paths anchor the trigger to
/// the same input budget ceiling. Falls back to the conservative default when
/// the model's window is unknown.
#[must_use]
pub fn compaction_threshold_for_model_at_percent(model: &str, percent: f64) -> usize {
    let Some(window) = context_window_for_model(model) else {
        return DEFAULT_COMPACTION_TOKEN_THRESHOLD;
    };
    let reservation = route_output_reservation_for_window(model, window, None);
    let budget = ContextBudget::new(u64::from(window), 0, u64::from(reservation));
    usize::try_from(budget.compaction_trigger_for_percent(percent))
        .unwrap_or(DEFAULT_COMPACTION_TOKEN_THRESHOLD)
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

    /// Input budget ceiling for a window/reservation pair, mirroring
    /// `ContextBudget`'s internal `window - reserved - headroom`.
    fn ceiling(window: u32, reservation: u32) -> u64 {
        u64::from(window)
            .saturating_sub(u64::from(reservation))
            .saturating_sub(1_024)
    }

    /// V4's 1M window reserves the full 262K output, so the usable input ceiling
    /// is ~736K. Anchoring the trigger to that ceiling drops the 80% threshold
    /// from the old 800K (80% of the *window*, which input can never reach once
    /// output is reserved) to ~589K. This is the core dimensional fix, and it is
    /// env-independent because the reservation comes from `TURN_MAX_OUTPUT_TOKENS`
    /// for large windows rather than `effective_max_output_tokens`.
    #[test]
    fn route_trigger_anchors_to_input_ceiling_on_v4() {
        let provider = ApiProvider::Deepseek;
        let model = "deepseek-v4-pro";
        let window = route_context_window_tokens(provider, model, None);
        assert_eq!(window, 1_000_000);
        let reservation = route_output_reservation_for_window(model, window, None);
        assert_eq!(reservation, TURN_MAX_OUTPUT_TOKENS);

        let expected_ceiling = ceiling(window, reservation);
        let trigger = compaction_threshold_for_route_at_percent(provider, model, None, 80.0);
        assert_eq!(trigger as u64, (expected_ceiling as f64 * 0.8 + 0.5) as u64);
        assert!(
            trigger < 800_000,
            "ceiling-anchored trigger {trigger} must be below the old 80%-of-window 800K"
        );
        assert!(trigger as u64 <= expected_ceiling);
    }

    /// Tight self-hosted window with a large output reservation: the old
    /// `percent × window` put the 80% trigger at ~205K — above the ~141K the
    /// input can actually occupy — so compaction never fired before the provider
    /// hard-rejected on length. The ceiling-anchored trigger must sit at or below
    /// the ceiling and strictly below the old window-relative value.
    #[test]
    fn route_trigger_does_not_exceed_ceiling_on_tight_window() {
        let provider = ApiProvider::Deepseek;
        let model = "custom-local-256k";
        let limits = RouteLimits {
            context_tokens: Some(262_144),
            output_tokens: Some(120_000),
            ..Default::default()
        };
        let window = route_context_window_tokens(provider, model, Some(limits));
        assert_eq!(window, 262_144);
        let reservation = route_output_reservation_for_window(model, window, Some(limits));
        assert_eq!(reservation, 120_000);

        let expected_ceiling = ceiling(window, reservation);
        let trigger =
            compaction_threshold_for_route_at_percent(provider, model, Some(limits), 80.0);
        let old_window_relative = (f64::from(window) * 0.8) as usize;
        assert!(
            trigger as u64 <= expected_ceiling,
            "trigger {trigger} must not exceed input ceiling {expected_ceiling}"
        );
        assert!(
            trigger < old_window_relative,
            "trigger {trigger} must fire earlier than the buggy window-relative {old_window_relative}"
        );
    }

    /// The model-only headless path (background runtime threads) must anchor to
    /// the same ceiling as the routed path. Asserted as an invariant rather than
    /// a literal so the case stays robust to `DEEPSEEK_MAX_OUTPUT_TOKENS`.
    #[test]
    fn model_trigger_never_exceeds_ceiling() {
        for model in [
            "deepseek-v4-pro",
            "trinity-large-thinking",
            "deepseek-v3.2-128k",
            "unknown-model",
        ] {
            let trigger = compaction_threshold_for_model_at_percent(model, 80.0);
            let Some(window) = context_window_for_model(model) else {
                continue;
            };
            let reservation = route_output_reservation_for_window(model, window, None);
            assert!(
                trigger as u64 <= ceiling(window, reservation),
                "model {model}: trigger {trigger} exceeded ceiling"
            );
            assert!(trigger > 0, "model {model}: trigger must be positive");
        }
    }

    /// Unknown windows fall back to the conservative default without panicking.
    #[test]
    fn unknown_window_uses_default_threshold() {
        // A model id with no known window and no route limits resolves through
        // `context_window_for_model`'s legacy fallback; the result is always a
        // positive, ceiling-bounded threshold.
        let trigger = compaction_threshold_for_model_at_percent("totally-unknown-xyz", 80.0);
        assert!(trigger > 0);
    }

    #[test]
    fn threshold_pct_for_strategy_default_is_identity() {
        assert_eq!(
            threshold_pct_for_strategy(HarnessCompactionStrategy::Default, 80.0),
            80.0
        );
    }

    #[test]
    fn threshold_pct_for_strategy_prefix_cache_is_later() {
        let adjusted = threshold_pct_for_strategy(HarnessCompactionStrategy::PrefixCache, 80.0);
        assert!(adjusted > 80.0);
        assert!(adjusted <= COMPACTION_PCT_CEILING);
    }

    #[test]
    fn threshold_pct_for_strategy_aggressive_is_earlier() {
        let adjusted = threshold_pct_for_strategy(HarnessCompactionStrategy::Aggressive, 80.0);
        assert!(adjusted < 80.0);
        assert!(adjusted >= COMPACTION_PCT_FLOOR);
    }

    #[test]
    fn default_strategy_compact_threshold_matches_legacy_percent() {
        let provider = ApiProvider::Deepseek;
        let model = "deepseek-v4-pro";
        let base = 80.0;
        let legacy = compaction_threshold_for_route_at_percent(provider, model, None, base);
        let adjusted = compaction_threshold_for_route_at_percent(
            provider,
            model,
            None,
            threshold_pct_for_strategy(HarnessCompactionStrategy::Default, base),
        );
        assert_eq!(legacy, adjusted);
    }

    #[test]
    fn headless_model_trigger_respects_compaction_strategy() {
        let model = "deepseek-v4-pro";
        let base = 80.0;
        let default_trigger = compaction_threshold_for_model_at_percent(
            model,
            threshold_pct_for_strategy(HarnessCompactionStrategy::Default, base),
        );
        let aggressive_trigger = compaction_threshold_for_model_at_percent(
            model,
            threshold_pct_for_strategy(HarnessCompactionStrategy::Aggressive, base),
        );
        assert!(aggressive_trigger < default_trigger);
    }
}
