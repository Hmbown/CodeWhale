//! The single shared turn-route planner (#1004).
//!
//! One function decides which provider, model, route identity, client, limits,
//! compaction policy, and reasoning tier a turn will use.
//! `spawned_dispatch_inner` calls it to *send* a turn; `/preview-request`
//! calls it with a hypothetical prompt to *describe* one. Because there is a
//! single implementation, a preview cannot report a route different from the
//! one dispatch would pick for the same prompt — which is the whole point of
//! previewing a route before spending anything on it.
//!
//! It lives outside the TUI module so the engine-side preview tests can drive
//! the same planner the UI drives, provider-free.
//!
//! The planner mutates no engine or session state. Its one outbound call is
//! the auto-router classifier, which only runs when auto model routing is on.
//! Production may use the deterministic response cache for that call;
//! `/preview-request` explicitly bypasses it so inspection does not perturb
//! later routing.

use crate::compaction::CompactionConfig;
use crate::config::{ApiProvider, Config, ProviderIdentity};
use crate::route_runtime::{
    ResolvedRuntimeRoute, resolve_runtime_route, resolve_runtime_route_for_identity,
};
use crate::tui::app::{AppMode, ReasoningEffort};

/// Everything the shared turn-route planner needs.
///
/// Borrowed rather than owned so the dispatch path can pass its already
/// captured `UserDispatchPrepare` fields and `/preview-request` can pass a
/// hypothetical prompt, without either one duplicating the other's logic.
pub(crate) struct TurnRoutePlanRequest<'a> {
    pub(crate) route_config: &'a Config,
    pub(crate) app_route_identity: &'a ProviderIdentity,
    pub(crate) api_provider: ApiProvider,
    pub(crate) app_model: &'a str,
    pub(crate) auto_model: bool,
    pub(crate) reasoning_effort: ReasoningEffort,
    pub(crate) mode: AppMode,
    /// Model-facing content of the next user message (file mentions and skill
    /// wrapping already resolved). This is what the auto router classifies.
    pub(crate) content: &'a str,
    /// The user's display text, used by the heuristic and auto-reasoning
    /// fallbacks exactly as production does.
    pub(crate) display_text: &'a str,
    pub(crate) auto_router_context: &'a str,
    pub(crate) should_auto_resolve: bool,
    /// Production dispatch may use the deterministic response cache for the
    /// auxiliary Auto classifier. Read-only previews must set this to false.
    pub(crate) allow_auto_router_response_cache: bool,
    pub(crate) preflight_required: bool,
    pub(crate) auto_compact_user_configured: bool,
    pub(crate) auto_compact: bool,
    pub(crate) auto_compact_threshold_percent: f64,
}

/// The exact route, limits, compaction policy, and reasoning normalization one
/// turn would use.
pub(crate) struct PlannedTurnRoute {
    pub(crate) route: ResolvedRuntimeRoute,
    pub(crate) compaction: CompactionConfig,
    pub(crate) effective_provider: ApiProvider,
    pub(crate) effective_model: String,
    pub(crate) effective_provider_identity: String,
    pub(crate) effective_provider_label: String,
    pub(crate) selected_reasoning_effort: Option<ReasoningEffort>,
    /// Normalized api value for the resolved route — the string that reaches
    /// the wire.
    pub(crate) effective_reasoning_effort: Option<String>,
    pub(crate) auto_controls_reasoning: bool,
    pub(crate) auto_selection: Option<crate::model_routing::AutoRouteSelection>,
    /// Why this concrete route was selected. This is captured by the planner,
    /// not inferred later from the resulting provider/model pair.
    pub(crate) routing_source: TurnRoutingSource,
}

/// Durable provenance for the route selected for one turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnRoutingSource {
    /// The active fixed route was used unchanged. This intentionally does not
    /// guess whether an earlier UI action or persisted config installed it.
    ActiveFixedRoute,
    /// Auto model routing used its provider-backed classifier.
    AutoProviderClassifier,
    /// Auto model routing used the local deterministic fallback heuristic.
    AutoLocalHeuristic,
}

impl TurnRoutingSource {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::ActiveFixedRoute => "active-fixed-route",
            Self::AutoProviderClassifier => "auto-provider-classifier",
            Self::AutoLocalHeuristic => "auto-local-heuristic",
        }
    }
}

fn reasoning_effort_for_route_selection(
    auto_model: bool,
    provider: ApiProvider,
    effort: ReasoningEffort,
) -> &'static str {
    if auto_model {
        effort.as_setting()
    } else {
        effort.as_setting_for_provider(provider)
    }
}

/// Resolve the route for one turn.
///
/// This is *the* route planner (#1004). `spawned_dispatch_inner` calls it to
/// send a turn; `/preview-request` calls it with a hypothetical prompt to
/// describe one. Because there is a single implementation, a preview cannot
/// report a provider, model, route identity, client, reasoning tier, limit,
/// tool budget, billing basis, or endpoint different from the one dispatch
/// would pick for the same prompt.
///
/// It mutates no engine or session state: it reads config, resolves a route,
/// and returns a value. Its one outbound call is the auto-router classifier,
/// which is the same auxiliary call a real turn makes and only runs when auto
/// model routing is on. The caller chooses whether that auxiliary call may
/// touch the process-global deterministic response cache.
pub(crate) async fn plan_turn_route(
    request: TurnRoutePlanRequest<'_>,
) -> Result<PlannedTurnRoute, String> {
    let auto_selection = if request.should_auto_resolve {
        Some(
            crate::model_routing::resolve_auto_route_with_inventory_for_session_and_cache_policy(
                request.route_config,
                request.content,
                request.auto_router_context,
                request.mode.as_setting(),
                if request.auto_model { "auto" } else { "fixed" },
                reasoning_effort_for_route_selection(
                    request.auto_model,
                    request.api_provider,
                    request.reasoning_effort,
                ),
                request.allow_auto_router_response_cache,
            )
            .await
            .map_err(|err| err.to_string())?,
        )
    } else {
        None
    };

    let effective_provider = auto_selection
        .as_ref()
        .map(|selection| selection.provider)
        .unwrap_or(request.api_provider);

    let effective_model = if request.auto_model {
        auto_selection
            .as_ref()
            .map(|selection| selection.model.clone())
            .unwrap_or_else(|| {
                crate::model_routing::auto_model_heuristic(request.display_text, request.app_model)
            })
    } else {
        request.app_model.to_string()
    };

    let turn_route = if effective_provider == request.app_route_identity.provider {
        resolve_runtime_route_for_identity(
            request.route_config,
            request.app_route_identity,
            Some(&effective_model),
        )
    } else {
        resolve_runtime_route(
            request.route_config,
            effective_provider,
            Some(&effective_model),
        )
    };

    let turn_route = turn_route.map_err(|err| err.to_string())?;
    let turn_route = if request.preflight_required {
        turn_route.preflight()?
    } else {
        turn_route
    };

    let turn_route_limits = crate::route_budget::known_route_limits(turn_route.candidate.limits());
    let effective_provider_identity = turn_route.identity.key.clone();
    let effective_provider_label = if effective_provider == ApiProvider::Custom {
        effective_provider_identity.clone()
    } else {
        effective_provider.display_name().to_string()
    };

    let turn_compaction = CompactionConfig {
        enabled: if request.auto_compact_user_configured {
            request.auto_compact
        } else {
            crate::route_budget::auto_compact_default_for_route(
                turn_route.identity.provider,
                &turn_route.model,
                turn_route_limits,
            )
        },
        token_threshold: crate::route_budget::compaction_threshold_for_route_at_percent(
            turn_route.identity.provider,
            &turn_route.model,
            turn_route_limits,
            request.auto_compact_threshold_percent,
        ),
        model: turn_route.model.clone(),
        image_input: turn_route.candidate.capabilities().image_input,
        effective_context_window: Some(crate::route_budget::route_context_window_tokens(
            turn_route.identity.provider,
            &turn_route.model,
            turn_route_limits,
        )),
        ..Default::default()
    };

    // Model selection and reasoning selection are independent. A fixed
    // reasoning preference survives auto model routing and is normalized
    // against the concrete route below; only an explicit `auto` delegates the
    // tier to the classifier/heuristic.
    let auto_controls_reasoning = request.reasoning_effort == ReasoningEffort::Auto;
    let selected_reasoning_effort = if auto_controls_reasoning {
        Some(
            auto_selection
                .as_ref()
                .and_then(|selection| selection.reasoning_effort)
                .unwrap_or_else(|| crate::auto_reasoning::select(false, request.display_text)),
        )
    } else {
        None
    };

    let effective_reasoning_effort = selected_reasoning_effort
        .unwrap_or(request.reasoning_effort)
        .api_value_for_route(
            effective_provider,
            &turn_route.candidate.endpoint().base_url,
            &turn_route.model,
        )
        .map(str::to_string);

    let routing_source = if !request.auto_model {
        TurnRoutingSource::ActiveFixedRoute
    } else if auto_selection.is_some() {
        TurnRoutingSource::AutoProviderClassifier
    } else {
        TurnRoutingSource::AutoLocalHeuristic
    };

    Ok(PlannedTurnRoute {
        route: turn_route,
        compaction: turn_compaction,
        effective_provider,
        effective_model,
        effective_provider_identity,
        effective_provider_label,
        selected_reasoning_effort,
        effective_reasoning_effort,
        auto_controls_reasoning,
        auto_selection,
        routing_source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_TEXT_MODEL;

    fn deepseek_identity() -> ProviderIdentity {
        ProviderIdentity {
            provider: ApiProvider::Deepseek,
            key: ApiProvider::Deepseek.as_str().to_string(),
            exact_id: None,
            migrated_legacy_ollama_cloud_route: false,
        }
    }

    #[test]
    fn auto_model_route_selection_keeps_raw_reasoning_preference() {
        assert_eq!(
            reasoning_effort_for_route_selection(
                true,
                ApiProvider::OpenaiCodex,
                ReasoningEffort::Off,
            ),
            "off"
        );
        assert_eq!(
            reasoning_effort_for_route_selection(
                false,
                ApiProvider::OpenaiCodex,
                ReasoningEffort::Off,
            ),
            "low"
        );
    }

    #[tokio::test]
    async fn auto_model_route_respects_fixed_reasoning_preference() {
        let config = Config::default();
        let identity = deepseek_identity();

        let planned = plan_turn_route(TurnRoutePlanRequest {
            route_config: &config,
            app_route_identity: &identity,
            api_provider: ApiProvider::Deepseek,
            app_model: DEFAULT_TEXT_MODEL,
            auto_model: true,
            reasoning_effort: ReasoningEffort::Low,
            mode: AppMode::Agent,
            content: "explain this function",
            display_text: "explain this function",
            auto_router_context: "",
            should_auto_resolve: false,
            allow_auto_router_response_cache: false,
            preflight_required: false,
            auto_compact_user_configured: false,
            auto_compact: true,
            auto_compact_threshold_percent: 80.0,
        })
        .await
        .expect("plan auto-model turn");

        assert_eq!(
            planned.routing_source,
            TurnRoutingSource::AutoLocalHeuristic
        );
        assert!(!planned.auto_controls_reasoning);
        assert_eq!(planned.selected_reasoning_effort, None);
        // First-party DeepSeek routes carry low as the real wire tier
        // (`reasoning_effort` low/high/max are documented); the App keeps the
        // unresolved preference as Low either way.
        assert_eq!(planned.effective_reasoning_effort.as_deref(), Some("low"));
    }
}
