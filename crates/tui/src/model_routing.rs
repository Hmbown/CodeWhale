//! Model selection and auto-routing.
//!
//! The CLI, TUI, runtime threads, subagents, and command handlers all need
//! this behavior, so it intentionally lives outside the command tree.

use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::client::DeepSeekClient;
use crate::config::{ApiProvider, Config, normalize_model_name_for_provider};
use crate::llm_client::LlmClient;
use crate::model_inventory::ModelInventory;
use crate::models::Role;
use crate::models::{ContentBlock, Message, MessageRequest, MessageResponse, SystemPrompt};
use crate::tui::app::ReasoningEffort;

/// Big/cheap model pair the auto-router may choose between for the active
/// provider (#3018).
///
/// `cheap == None` means the provider has no known cheap tier: heuristics
/// stay on the current model (only thinking effort varies) and the network
/// router is skipped entirely (#1549).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouterCandidates {
    pub(crate) big: String,
    pub(crate) cheap: Option<String>,
}

impl RouterCandidates {
    pub(crate) fn deepseek() -> Self {
        Self {
            big: "deepseek-v4-pro".to_string(),
            cheap: Some("deepseek-v4-flash".to_string()),
        }
    }

    /// The cheap-tier id, falling back to `big` when no cheap tier exists.
    pub(crate) fn cheap_or_big(&self) -> &str {
        self.cheap.as_deref().unwrap_or(&self.big)
    }
}

/// Return a provider-owned strong/fast pair for model families whose catalog
/// exposes more than one tier.  The ids here are deliberately explicit: a
/// model name alone is not evidence that another provider can serve its
/// sibling, so unknown providers and unknown families remain single-tier.
fn catalog_family_candidates(
    provider: ApiProvider,
    current_model: &str,
) -> Option<RouterCandidates> {
    let normalized = normalize_model_name_for_provider(provider, current_model)
        .unwrap_or_else(|| current_model.trim().to_string());
    let lower = normalized.to_ascii_lowercase();

    let cheap = match provider {
        ApiProvider::Openai | ApiProvider::OpenaiCodex
            if matches!(lower.as_str(), "gpt-5.6" | "gpt-5.6-sol" | "gpt-5.6-terra") =>
        {
            Some("gpt-5.6-luna".to_string())
        }
        ApiProvider::Anthropic
            if matches!(
                lower.as_str(),
                "claude-opus-4-8" | "claude-sonnet-4-6" | "claude-sonnet-5"
            ) =>
        {
            Some("claude-haiku-4-5".to_string())
        }
        ApiProvider::XiaomiMimo if lower == "mimo-v2.5-pro" => Some("mimo-v2.5".to_string()),
        ApiProvider::Arcee
            if matches!(
                lower.as_str(),
                "trinity-large-thinking" | "trinity-large-preview"
            ) =>
        {
            Some("trinity-mini".to_string())
        }
        ApiProvider::Moonshot if lower == "kimi-k2.7-code" => Some("kimi-k2.6".to_string()),
        ApiProvider::Minimax | ApiProvider::MinimaxAnthropic if lower == "minimax-m2.7" => {
            Some("MiniMax-M2.7-highspeed".to_string())
        }
        ApiProvider::OpencodeGo if lower == "kimi-k3" => Some("kimi-k2.7-code".to_string()),
        ApiProvider::Openrouter
            if lower == "qwen/qwen3.6-max-preview"
                || lower == "qwen/qwen3.6-plus"
                || lower == "qwen/qwen3.6-27b"
                || lower == "qwen/qwen3.6-35b-a3b" =>
        {
            Some("qwen/qwen3.6-flash".to_string())
        }
        ApiProvider::Openrouter if lower == "xiaomi/mimo-v2.5-pro" => {
            Some("xiaomi/mimo-v2.5".to_string())
        }
        ApiProvider::Openrouter
            if matches!(
                lower.as_str(),
                "arcee-ai/trinity-large-thinking" | "arcee-ai/trinity-large-preview"
            ) =>
        {
            Some("arcee-ai/trinity-mini".to_string())
        }
        ApiProvider::Openrouter if lower == "moonshotai/kimi-k2.7-code" => {
            Some("moonshotai/kimi-k2.6".to_string())
        }
        ApiProvider::Openrouter
            if lower == "anthropic/claude-opus-4-8"
                || lower == "anthropic/claude-sonnet-4-6"
                || lower == "anthropic/claude-sonnet-5" =>
        {
            Some("anthropic/claude-haiku-4-5".to_string())
        }
        _ => None,
    }?;

    Some(RouterCandidates {
        big: normalized,
        cheap: Some(cheap),
    })
}

/// Derive the auto-router's candidate pair for the active provider (#3018).
///
/// DeepSeek providers route between the canonical pro/flash pair. Hosted
/// routes with known wire ids for that pair (NVIDIA NIM, OpenRouter, Novita,
/// SiliconFlow, SGLang, vLLM, Wanjie Ark, Volcengine) use their provider
/// spellings. Every other provider has no known cheap tier: `big` is the
/// session model and `cheap` is `None`, so auto mode never fabricates a
/// DeepSeek id for a provider that cannot serve it.
pub(crate) fn provider_router_candidates(
    provider: crate::config::ApiProvider,
    current_model: &str,
) -> RouterCandidates {
    use crate::config::ApiProvider;
    if let Some(candidates) = catalog_family_candidates(provider, current_model) {
        return candidates;
    }

    if provider == ApiProvider::Zai {
        let normalized = crate::config::normalize_model_name_for_provider(provider, current_model)
            .unwrap_or_else(|| current_model.to_string());
        return RouterCandidates {
            // GLM-5.3 routes faster/explore children to GLM-5.3-Flash.
            // GLM-5.2 still uses GLM-5-Turbo. Flash, Turbo, and 5.1 have no
            // cheaper tier and keep children on the parent model.
            cheap: if normalized == crate::config::ZAI_GLM_5_3_MODEL {
                Some(crate::config::ZAI_GLM_5_3_FLASH_MODEL.to_string())
            } else if normalized == crate::config::ZAI_GLM_5_2_MODEL {
                Some(crate::config::ZAI_GLM_5_TURBO_MODEL.to_string())
            } else {
                None
            },
            big: normalized,
        };
    }

    if provider == ApiProvider::Openrouter
        && let Some(normalized) =
            crate::config::normalize_model_name_for_provider(provider, current_model)
        && matches!(
            normalized.as_str(),
            crate::config::OPENROUTER_GLM_5_1_MODEL
                | crate::config::OPENROUTER_GLM_5_2_MODEL
                | crate::config::OPENROUTER_GLM_5_3_MODEL
                | crate::config::OPENROUTER_GLM_5_3_FLASH_MODEL
                | crate::config::OPENROUTER_GLM_5_TURBO_MODEL
        )
    {
        return RouterCandidates {
            // z-ai/glm-5.3 routes faster children to z-ai/glm-5.3-flash;
            // z-ai/glm-5.2 still uses z-ai/glm-5-turbo. Flash, turbo, and 5.1
            // have no cheaper tier and keep children on parent.
            cheap: if normalized == crate::config::OPENROUTER_GLM_5_3_MODEL {
                Some(crate::config::OPENROUTER_GLM_5_3_FLASH_MODEL.to_string())
            } else if normalized == crate::config::OPENROUTER_GLM_5_2_MODEL {
                Some(crate::config::OPENROUTER_GLM_5_TURBO_MODEL.to_string())
            } else {
                None
            },
            big: normalized,
        };
    }

    match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => RouterCandidates::deepseek(),
        ApiProvider::NvidiaNim
        | ApiProvider::Openrouter
        | ApiProvider::Novita
        | ApiProvider::Siliconflow
        | ApiProvider::SiliconflowCn
        | ApiProvider::Sglang
        | ApiProvider::Vllm
        | ApiProvider::WanjieArk
            if current_model.to_ascii_lowercase().contains("deepseek") =>
        {
            RouterCandidates {
                big: crate::config::wire_model_for_provider(provider, "deepseek-v4-pro"),
                cheap: Some(crate::config::wire_model_for_provider(
                    provider,
                    "deepseek-v4-flash",
                )),
            }
        }
        ApiProvider::Volcengine if current_model.to_ascii_lowercase().contains("deepseek") => {
            RouterCandidates {
                big: crate::config::DEFAULT_VOLCENGINE_MODEL.to_string(),
                cheap: Some(crate::config::DEFAULT_VOLCENGINE_FLASH_MODEL.to_string()),
            }
        }
        _ => RouterCandidates {
            big: current_model.to_string(),
            cheap: None,
        },
    }
}

/// Auto-select a model based on request complexity.
///
/// Short messages (<100 chars) go to the cheap tier. Long messages and
/// requests with complex keywords go to the big tier. The fallback is cheap.
/// This DeepSeek-candidate wrapper keeps legacy callers and tests intact;
/// provider-aware callers use [`auto_model_heuristic_for_candidates`].
pub(crate) fn auto_model_heuristic(input: &str, current_model: &str) -> String {
    auto_model_heuristic_for_candidates(input, current_model, &RouterCandidates::deepseek())
}

/// Candidate-aware variant of [`auto_model_heuristic`] (#3018).
pub(crate) fn auto_model_heuristic_for_candidates(
    input: &str,
    current_model: &str,
    candidates: &RouterCandidates,
) -> String {
    auto_model_heuristic_with_bias_for_candidates(input, current_model, false, candidates).model
}

#[cfg(test)]
fn auto_model_heuristic_with_bias(input: &str, current_model: &str, cost_saving: bool) -> String {
    auto_model_heuristic_with_bias_for_candidates(
        input,
        current_model,
        cost_saving,
        &RouterCandidates::deepseek(),
    )
    .model
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AutoRouteHeuristicDecision {
    model: String,
    reason: AutoRouteHeuristicReason,
}

fn auto_model_heuristic_with_bias_for_candidates(
    input: &str,
    _current_model: &str,
    cost_saving: bool,
    candidates: &RouterCandidates,
) -> AutoRouteHeuristicDecision {
    let len = input.chars().count();
    let lower = input.to_lowercase();
    let borderline_pro_keywords: &[&str] = &[
        "implement",
        "analyze",
        "\u{5b9e}\u{73b0}",
        "\u{5206}\u{6790}",
        "\u{5be6}\u{73fe}",
    ];
    let strong_match = COMPLEX_KEYWORDS
        .iter()
        .any(|kw| !borderline_pro_keywords.contains(kw) && lower.contains(kw));
    let borderline_match = borderline_pro_keywords.iter().any(|kw| lower.contains(kw));
    let pro_match = strong_match || (!cost_saving && borderline_match);
    if pro_match {
        return AutoRouteHeuristicDecision {
            model: candidates.big.clone(),
            reason: AutoRouteHeuristicReason::ComplexRequest,
        };
    }
    if len < 100 {
        return AutoRouteHeuristicDecision {
            model: candidates.cheap_or_big().to_string(),
            reason: if cost_saving && borderline_match {
                AutoRouteHeuristicReason::CostSavingPolicy
            } else {
                AutoRouteHeuristicReason::ShortRequest
            },
        };
    }
    let long_threshold = if cost_saving { 1_000 } else { 500 };
    if len > long_threshold {
        return AutoRouteHeuristicDecision {
            model: candidates.big.clone(),
            reason: AutoRouteHeuristicReason::LongRequest,
        };
    }

    AutoRouteHeuristicDecision {
        model: candidates.cheap_or_big().to_string(),
        reason: if cost_saving && borderline_match {
            AutoRouteHeuristicReason::CostSavingPolicy
        } else {
            AutoRouteHeuristicReason::RoutineRequest
        },
    }
}

const COMPLEX_KEYWORDS: &[&str] = &[
    "refactor",
    "architecture",
    "design",
    "debug",
    "security",
    "review",
    "audit",
    "migrate",
    "optimize",
    "rewrite",
    "implement",
    "analyze",
    "\u{91cd}\u{6784}",
    "\u{67b6}\u{6784}",
    "\u{8bbe}\u{8ba1}",
    "\u{8c03}\u{8bd5}",
    "\u{5b89}\u{5168}",
    "\u{5ba1}\u{67e5}",
    "\u{5ba1}\u{8ba1}",
    "\u{8fc1}\u{79fb}",
    "\u{4f18}\u{5316}",
    "\u{91cd}\u{5199}",
    "\u{5b9e}\u{73b0}",
    "\u{5206}\u{6790}",
    "\u{91cd}\u{69cb}",
    "\u{67b6}\u{69cb}",
    "\u{8a2d}\u{8a08}",
    "\u{8abf}\u{8a66}",
    "\u{5be9}\u{67e5}",
    "\u{5be9}\u{8a08}",
    "\u{9077}\u{79fb}",
    "\u{512a}\u{5316}",
    "\u{91cd}\u{5beb}",
    "\u{5be6}\u{73fe}",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutoRouteSource {
    FlashRouter,
    Heuristic,
}

impl AutoRouteSource {
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            AutoRouteSource::FlashRouter => "classifier",
            AutoRouteSource::Heuristic => "heuristic",
        }
    }
}

/// Provider-safe tier reported for the concrete Auto route.
///
/// `Selected` is deliberately neutral: a classifier may choose a runnable
/// inventory model that is not part of a known strong/fast pair, and the UI
/// must not invent a tier from the model id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AutoRouteTier {
    Strong,
    Fast,
    Only,
    Selected,
}

impl AutoRouteTier {
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Fast => "fast",
            Self::Only => "only model",
            Self::Selected => "selected",
        }
    }
}

/// Scope from which the concrete Auto route was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AutoRouteScope {
    /// The network classifier could choose any runnable provider/model pair in
    /// the redacted inventory. Only reachable under the persisted
    /// `[auto] cross_provider = true` opt-in (#4411).
    RunnableProviders,
    /// The network classifier saw only the active provider's runnable routes —
    /// the default Auto scope (#4411).
    ActiveProvider,
    /// The provider-aware local heuristic selected within one resolved route.
    ResolvedProvider,
}

impl AutoRouteScope {
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::RunnableProviders => "runnable providers",
            Self::ActiveProvider => "active provider only",
            Self::ResolvedProvider => "resolved provider",
        }
    }
}

/// Non-secret data path used to make an Auto decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AutoRouteDataPath {
    LocalHeuristic,
    Classifier {
        provider: ApiProvider,
        model: String,
    },
}

impl AutoRouteDataPath {
    #[must_use]
    pub(crate) fn label(&self) -> String {
        match self {
            Self::LocalHeuristic => "local only (no router request)".to_string(),
            Self::Classifier { provider, model } => format!(
                "latest request + bounded recent context -> {} / {model}",
                provider.display_name()
            ),
        }
    }
}

/// Local signal that selected the provider-safe strong/fast candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AutoRouteHeuristicReason {
    ComplexRequest,
    ShortRequest,
    LongRequest,
    CostSavingPolicy,
    RoutineRequest,
    NoFastSibling,
    NoRunnableCandidate,
}

impl AutoRouteHeuristicReason {
    #[must_use]
    fn label(self) -> &'static str {
        match self {
            Self::ComplexRequest => "complex request",
            Self::ShortRequest => "short request",
            Self::LongRequest => "long request",
            Self::CostSavingPolicy => "cost-saving policy",
            Self::RoutineRequest => "routine request",
            Self::NoFastSibling => "no runnable fast sibling",
            Self::NoRunnableCandidate => "no runnable inventory candidate",
        }
    }
}

/// Why the route was selected. Classifier failures are intentionally
/// collapsed to a non-secret reason; provider errors and response bodies must
/// never enter diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AutoRouteReason {
    ClassifierRecommendation,
    LocalHeuristic(AutoRouteHeuristicReason),
    ClassifierFallback(AutoRouteHeuristicReason),
}

impl AutoRouteReason {
    #[must_use]
    pub(crate) fn label(self) -> String {
        match self {
            Self::ClassifierRecommendation => "classifier recommendation".to_string(),
            Self::LocalHeuristic(reason) => format!("local heuristic: {}", reason.label()),
            Self::ClassifierFallback(reason) => {
                format!("classifier fallback: {}", reason.label())
            }
        }
    }
}

/// Effective provider-scoped model pair used to classify the selected tier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AutoRoutePair {
    pub(crate) strong: String,
    pub(crate) fast: Option<String>,
}

/// Per-turn Auto routing diagnostics. Provider/model identity remains owned by
/// the authoritative runtime `TurnRoute`; this receipt only records how the
/// concrete route was chosen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AutoRouteReceipt {
    pub(crate) tier: AutoRouteTier,
    pub(crate) pair: AutoRoutePair,
    pub(crate) scope: AutoRouteScope,
    pub(crate) data_path: AutoRouteDataPath,
    pub(crate) reason: AutoRouteReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutoRouteSelection {
    pub(crate) provider: ApiProvider,
    pub(crate) model: String,
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
    pub(crate) source: AutoRouteSource,
    /// Present for Auto decisions; explicit inventory lookups intentionally do
    /// not pretend to be Auto routing receipts.
    pub(crate) receipt: Option<AutoRouteReceipt>,
}

fn extract_first_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    (end >= start).then_some(&raw[start..=end])
}

fn parse_auto_route_reasoning_effort(effort: &str) -> Option<ReasoningEffort> {
    ReasoningEffort::parse_strict(effort).ok()
}

#[must_use]
pub(crate) fn normalize_auto_route_effort_for_provider(
    provider: ApiProvider,
    effort: ReasoningEffort,
) -> ReasoningEffort {
    if provider == ApiProvider::OpenaiCodex {
        return effort.normalize_for_provider(provider);
    }
    match effort {
        ReasoningEffort::Low | ReasoningEffort::Medium => ReasoningEffort::High,
        other => other,
    }
}

/// Select the reasoning request that accompanies an Auto-model route.
///
/// Model routing and reasoning routing are independent. An explicit fixed
/// preference wins over the classifier's suggestion; an absent preference or
/// explicit `Auto` keeps reasoning under per-prompt control.
#[must_use]
pub(crate) fn resolve_auto_model_reasoning(
    preference: Option<ReasoningEffort>,
    routed: Option<ReasoningEffort>,
) -> (Option<ReasoningEffort>, bool) {
    match preference {
        Some(
            effort @ (ReasoningEffort::Off
            | ReasoningEffort::Minimal
            | ReasoningEffort::Low
            | ReasoningEffort::Medium
            | ReasoningEffort::High
            | ReasoningEffort::XHigh
            | ReasoningEffort::Ultra
            | ReasoningEffort::Max),
        ) => (Some(effort), false),
        None | Some(ReasoningEffort::Auto) => (routed, true),
    }
}

/// Route-aware equivalent of [`normalize_auto_route_effort_for_provider`].
/// The inventory knows the selected provider/model, and the route resolver
/// supplies the endpoint needed to distinguish Kimi Code's official bare-K3
/// contract from generic Moonshot.
#[must_use]
pub(crate) fn normalize_auto_route_effort_for_configured_route(
    config: &Config,
    provider: ApiProvider,
    model: &str,
    effort: ReasoningEffort,
) -> ReasoningEffort {
    crate::route_runtime::resolve_runtime_route(config, provider, Some(model))
        .map(|route| {
            effort.normalize_for_route(provider, &route.candidate.endpoint().base_url, &route.model)
        })
        .unwrap_or_else(|_| normalize_auto_route_effort_for_provider(provider, effort))
}

fn normalize_auto_route_selection_for_config(
    config: &Config,
    mut selection: AutoRouteSelection,
) -> AutoRouteSelection {
    selection.reasoning_effort = selection.reasoning_effort.map(|effort| {
        normalize_auto_route_effort_for_configured_route(
            config,
            selection.provider,
            &selection.model,
            effort,
        )
    });
    selection
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InventoryAutoRouteRecommendation {
    provider: ApiProvider,
    model: String,
    reasoning_effort: Option<ReasoningEffort>,
}

pub(crate) async fn resolve_auto_route_with_inventory(
    config: &Config,
    latest_request: &str,
    recent_context: &str,
    selected_model_mode: &str,
    selected_thinking_mode: &str,
) -> Result<AutoRouteSelection> {
    resolve_auto_route_with_inventory_for_session(
        config,
        latest_request,
        recent_context,
        "agent",
        selected_model_mode,
        selected_thinking_mode,
    )
    .await
}

pub(crate) async fn resolve_auto_route_with_inventory_for_session(
    config: &Config,
    latest_request: &str,
    recent_context: &str,
    session_mode: &str,
    selected_model_mode: &str,
    selected_thinking_mode: &str,
) -> Result<AutoRouteSelection> {
    resolve_auto_route_with_inventory_for_session_and_cache_policy(
        config,
        latest_request,
        recent_context,
        session_mode,
        selected_model_mode,
        selected_thinking_mode,
        true,
    )
    .await
}

pub(crate) async fn resolve_auto_route_with_inventory_for_session_and_cache_policy(
    config: &Config,
    latest_request: &str,
    recent_context: &str,
    session_mode: &str,
    selected_model_mode: &str,
    selected_thinking_mode: &str,
    allow_response_cache: bool,
) -> Result<AutoRouteSelection> {
    let inventory = ModelInventory::from_config(config);
    if !inventory.router_available {
        // Fall back to heuristic-only auto routing when the flash router
        // is unavailable (e.g. non-DeepSeek providers like wanjie-ark).
        return Ok(normalize_auto_route_selection_for_config(
            config,
            auto_route_from_inventory_heuristic(config, latest_request, &inventory),
        ));
    }

    let heuristic = auto_route_from_inventory_heuristic(config, latest_request, &inventory);
    if cfg!(test) {
        return Ok(normalize_auto_route_selection_for_config(config, heuristic));
    }

    let selection = match auto_route_inventory_recommendation(
        config,
        &inventory,
        latest_request,
        recent_context,
        session_mode,
        selected_model_mode,
        selected_thinking_mode,
        allow_response_cache,
    )
    .await
    {
        Ok(Some(recommendation)) => auto_route_from_classifier(&inventory, recommendation),
        Ok(None) | Err(_) => auto_route_classifier_fallback(heuristic, &inventory),
    };
    Ok(normalize_auto_route_selection_for_config(config, selection))
}

pub(crate) fn resolve_explicit_route_with_inventory(
    config: &Config,
    requested_model: &str,
) -> Option<AutoRouteSelection> {
    let requested_model = requested_model.trim();
    if requested_model.is_empty() || requested_model.eq_ignore_ascii_case("auto") {
        return None;
    }

    let inventory = ModelInventory::from_config(config);
    let active_provider = config.api_provider();

    if let Some(candidate) = inventory.candidates.iter().find(|candidate| {
        candidate.provider == active_provider
            && explicit_model_matches_candidate(candidate, requested_model)
    }) {
        return Some(AutoRouteSelection {
            provider: candidate.provider,
            model: candidate.model.clone(),
            reasoning_effort: config.reasoning_effort().map(|setting| {
                normalize_auto_route_effort_for_configured_route(
                    config,
                    candidate.provider,
                    &candidate.model,
                    ReasoningEffort::from_setting(setting),
                )
            }),
            source: AutoRouteSource::Heuristic,
            receipt: None,
        });
    }

    let mut matches = inventory
        .candidates
        .iter()
        .filter(|candidate| explicit_model_matches_candidate(candidate, requested_model));
    let candidate = matches.next()?;
    if matches.next().is_some() {
        return None;
    }

    Some(AutoRouteSelection {
        provider: candidate.provider,
        model: candidate.model.clone(),
        reasoning_effort: config.reasoning_effort().map(|setting| {
            normalize_auto_route_effort_for_configured_route(
                config,
                candidate.provider,
                &candidate.model,
                ReasoningEffort::from_setting(setting),
            )
        }),
        source: AutoRouteSource::Heuristic,
        receipt: None,
    })
}

pub(crate) fn explicit_route_candidate_providers(
    config: &Config,
    requested_model: &str,
) -> Vec<ApiProvider> {
    let requested_model = requested_model.trim();
    if requested_model.is_empty() || requested_model.eq_ignore_ascii_case("auto") {
        return Vec::new();
    }

    let inventory = ModelInventory::from_config(config);
    let mut providers = Vec::new();
    for candidate in inventory
        .candidates
        .iter()
        .filter(|candidate| explicit_model_matches_candidate(candidate, requested_model))
    {
        if !providers.contains(&candidate.provider) {
            providers.push(candidate.provider);
        }
    }
    providers
}

fn explicit_model_matches_candidate(
    candidate: &crate::model_inventory::ModelRouteCandidate,
    requested_model: &str,
) -> bool {
    candidate.model.eq_ignore_ascii_case(requested_model)
        || normalize_model_name_for_provider(candidate.provider, requested_model)
            .is_some_and(|model| candidate.model.eq_ignore_ascii_case(&model))
}

fn auto_route_from_inventory_heuristic(
    config: &Config,
    latest_request: &str,
    inventory: &ModelInventory,
) -> AutoRouteSelection {
    let Some(active) = inventory.active_default() else {
        let model = config.default_model();
        return AutoRouteSelection {
            provider: config.api_provider(),
            receipt: Some(auto_route_receipt(
                inventory,
                config.api_provider(),
                &model,
                AutoRouteScope::ResolvedProvider,
                AutoRouteDataPath::LocalHeuristic,
                AutoRouteReason::LocalHeuristic(AutoRouteHeuristicReason::NoRunnableCandidate),
            )),
            model,
            reasoning_effort: Some(crate::auto_reasoning::select(false, latest_request)),
            source: AutoRouteSource::Heuristic,
        };
    };
    // Use the candidates' cheap/big info for complexity-based routing.
    let router_candidates = provider_router_candidates(active.provider, &active.model);
    let fast_is_runnable = router_candidates.cheap.as_deref().is_some_and(|model| {
        inventory
            .candidate(active.provider, model)
            .is_some_and(|candidate| candidate.readiness.can_attempt())
    });
    let decision = if fast_is_runnable {
        auto_model_heuristic_with_bias_for_candidates(
            latest_request,
            &active.model,
            config.auto_cost_saving(),
            &router_candidates,
        )
    } else {
        AutoRouteHeuristicDecision {
            model: active.model.clone(),
            reason: AutoRouteHeuristicReason::NoFastSibling,
        }
    };
    AutoRouteSelection {
        provider: active.provider,
        receipt: Some(auto_route_receipt(
            inventory,
            active.provider,
            &decision.model,
            AutoRouteScope::ResolvedProvider,
            AutoRouteDataPath::LocalHeuristic,
            AutoRouteReason::LocalHeuristic(decision.reason),
        )),
        model: decision.model,
        reasoning_effort: Some(crate::auto_reasoning::select(false, latest_request)),
        source: AutoRouteSource::Heuristic,
    }
}

fn auto_route_from_classifier(
    inventory: &ModelInventory,
    recommendation: InventoryAutoRouteRecommendation,
) -> AutoRouteSelection {
    let data_path = AutoRouteDataPath::Classifier {
        provider: inventory.router_provider,
        model: inventory.router_model.to_string(),
    };
    // Report the scope the classifier actually had, not the widest one it
    // could ever have (#4411).
    let scope = if inventory.cross_provider_auto {
        AutoRouteScope::RunnableProviders
    } else {
        AutoRouteScope::ActiveProvider
    };
    AutoRouteSelection {
        provider: recommendation.provider,
        receipt: Some(auto_route_receipt(
            inventory,
            recommendation.provider,
            &recommendation.model,
            scope,
            data_path,
            AutoRouteReason::ClassifierRecommendation,
        )),
        model: recommendation.model,
        reasoning_effort: recommendation.reasoning_effort,
        source: AutoRouteSource::FlashRouter,
    }
}

fn auto_route_classifier_fallback(
    mut heuristic: AutoRouteSelection,
    inventory: &ModelInventory,
) -> AutoRouteSelection {
    if let Some(receipt) = heuristic.receipt.as_mut() {
        let heuristic_reason = match receipt.reason {
            AutoRouteReason::LocalHeuristic(reason)
            | AutoRouteReason::ClassifierFallback(reason) => reason,
            AutoRouteReason::ClassifierRecommendation => AutoRouteHeuristicReason::RoutineRequest,
        };
        receipt.data_path = AutoRouteDataPath::Classifier {
            provider: inventory.router_provider,
            model: inventory.router_model.to_string(),
        };
        receipt.reason = AutoRouteReason::ClassifierFallback(heuristic_reason);
    }
    heuristic
}

fn auto_route_receipt(
    inventory: &ModelInventory,
    provider: ApiProvider,
    selected_model: &str,
    scope: AutoRouteScope,
    data_path: AutoRouteDataPath,
    reason: AutoRouteReason,
) -> AutoRouteReceipt {
    let pair = auto_route_pair(inventory, provider, selected_model);
    let tier = if pair
        .fast
        .as_deref()
        .is_some_and(|fast| fast.eq_ignore_ascii_case(selected_model))
    {
        AutoRouteTier::Fast
    } else if pair.strong.eq_ignore_ascii_case(selected_model) {
        if pair.fast.is_some() {
            AutoRouteTier::Strong
        } else {
            AutoRouteTier::Only
        }
    } else {
        AutoRouteTier::Selected
    };
    AutoRouteReceipt {
        tier,
        pair,
        scope,
        data_path,
        reason,
    }
}

fn auto_route_pair(
    inventory: &ModelInventory,
    provider: ApiProvider,
    selected_model: &str,
) -> AutoRoutePair {
    // A provider can expose several unrelated model families. Derive the pair
    // from a runnable candidate that actually contains the selected model,
    // preferring a cheap-tier match before a strong-tier match. Falling back
    // to the provider default would report a truthful provider with a false
    // model family (for example OpenRouter GLM reported as DeepSeek).
    let matching_pair = inventory
        .candidates
        .iter()
        .filter(|candidate| candidate.provider == provider && candidate.readiness.can_attempt())
        .map(|candidate| provider_router_candidates(provider, &candidate.model))
        .find(|pair| {
            pair.cheap
                .as_deref()
                .is_some_and(|fast| fast.eq_ignore_ascii_case(selected_model))
        })
        .or_else(|| {
            inventory
                .candidates
                .iter()
                .filter(|candidate| {
                    candidate.provider == provider && candidate.readiness.can_attempt()
                })
                .map(|candidate| provider_router_candidates(provider, &candidate.model))
                .find(|pair| pair.big.eq_ignore_ascii_case(selected_model))
        });
    let Some(candidates) = matching_pair else {
        return AutoRoutePair {
            strong: selected_model.to_string(),
            fast: None,
        };
    };
    let Some(strong) = inventory
        .candidate(provider, &candidates.big)
        .filter(|candidate| candidate.readiness.can_attempt())
        .map(|candidate| candidate.model.clone())
    else {
        return AutoRoutePair {
            strong: selected_model.to_string(),
            fast: None,
        };
    };
    let fast = candidates.cheap.as_deref().and_then(|model| {
        inventory
            .candidate(provider, model)
            .filter(|candidate| candidate.readiness.can_attempt())
            .map(|candidate| candidate.model.clone())
    });
    AutoRoutePair { strong, fast }
}

#[allow(clippy::too_many_arguments)]
async fn auto_route_inventory_recommendation(
    config: &Config,
    inventory: &ModelInventory,
    latest_request: &str,
    recent_context: &str,
    session_mode: &str,
    selected_model_mode: &str,
    selected_thinking_mode: &str,
    allow_response_cache: bool,
) -> Result<Option<InventoryAutoRouteRecommendation>> {
    let mut router_config = config.clone();
    // The classifier runs on the inventory's router route: the explicit
    // [auto.router] route when configured, else the DeepSeek flash default.
    router_config.provider = Some(inventory.router_provider.as_str().to_string());
    router_config.default_text_model = Some(inventory.router_model.clone());

    let client = DeepSeekClient::new(&router_config)?;
    let router_system = inventory_auto_router_system_prompt(inventory, config.auto_cost_saving());
    let router_prompt = classifier_prompt(
        &client,
        latest_request,
        recent_context,
        session_mode,
        selected_model_mode,
        selected_thinking_mode,
    );
    let request_route =
        client.effective_route_envelope(&inventory.router_model, chrono::Utc::now());
    let request = MessageRequest {
        model: inventory.router_model.to_string(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: router_prompt,
                cache_control: None,
            }],
        }],
        max_tokens: client.effective_max_output_tokens(&request_route.model),
        system: Some(SystemPrompt::Text(router_system)),
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort: Some(
            inventory
                .router_thinking
                .clone()
                .unwrap_or_else(|| "off".to_string()),
        ),
        stream: Some(false),
        temperature: None,
        top_p: None,
    };

    let response = if allow_response_cache {
        tokio::time::timeout(
            Duration::from_secs(inventory.router_timeout_secs),
            client.create_message(request),
        )
        .await??
    } else {
        tokio::time::timeout(
            Duration::from_secs(inventory.router_timeout_secs),
            client.create_message_without_response_cache(request),
        )
        .await??
    };
    if crate::models::is_incomplete_stop_reason(response.stop_reason.as_deref()) {
        anyhow::bail!(
            "auto-route classifier response incomplete: provider stop reason `{}`",
            crate::models::stop_reason_detail(response.stop_reason.as_deref())
        );
    }
    Ok(parse_inventory_auto_route_recommendation(
        &message_response_text(&response),
        inventory,
    ))
}

fn inventory_auto_router_system_prompt(inventory: &ModelInventory, cost_saving: bool) -> String {
    let mut prompt = if inventory.cross_provider_auto {
        String::new()
    } else {
        // The inventory JSON below is already scoped to the active provider
        // (#4411); say so, so the classifier does not try to name one it was
        // never shown.
        format!(
            "Auto routing is scoped to the active provider `{}`. Every model in the inventory \
below belongs to it; never select another provider.\n\n",
            inventory.active_provider.as_str()
        )
    };
    prompt.push_str(&format!(
        "You are the codewhale model-routing classifier. Return only compact JSON: \
{{\"provider\":\"<provider>\",\"model\":\"<model>\",\"thinking\":\"off|high|max\"}}.\n\
Choose only provider/model pairs present in the inventory JSON. Use off only for trivial no-tool answers, \
high for ordinary reasoning, and max for agentic, coding, multi-file, release, architecture, debugging, \
security, tool-heavy, or uncertain work.\n\nInventory JSON:\n{}",
        inventory.router_context_json()
    ));

    if cost_saving {
        let active_pair = inventory.active_default().and_then(|active| {
            let candidates = provider_router_candidates(active.provider, &active.model);
            let fast = candidates.cheap.as_deref()?;
            (inventory
                .candidate(active.provider, &candidates.big)
                .is_some_and(|candidate| candidate.readiness.can_attempt())
                && inventory
                    .candidate(active.provider, fast)
                    .is_some_and(|candidate| candidate.readiness.can_attempt()))
            .then_some((active.provider, candidates.big, fast.to_string()))
        });

        if let Some((provider, strong, fast)) = active_pair {
            prompt.push_str(&format!(
                "\n\nCost-saving mode is ON. For the active provider `{}`, `{fast}` is the fast tier \
and `{strong}` is the strong tier. Prefer `{fast}` for ambiguous, routine, or single-step work. \
Select `{strong}` only when the request is unmistakably agentic, multi-step, architecture/design, \
security review, debugging, or otherwise clearly beyond the fast tier. Keep the selected model paired \
with provider `{}`.",
                provider.as_str(),
                provider.as_str()
            ));
        } else {
            prompt.push_str(
                "\n\nCost-saving mode is ON, but the active provider has no known runnable fast sibling. \
Do not invent a model or cross-provider downgrade solely to save cost.",
            );
        }
    }

    prompt
}

fn parse_inventory_auto_route_recommendation(
    raw: &str,
    inventory: &ModelInventory,
) -> Option<InventoryAutoRouteRecommendation> {
    let json = extract_first_json_object(raw)?;
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let provider = value
        .get("provider")
        .and_then(serde_json::Value::as_str)
        .and_then(ApiProvider::parse)?;
    // Defense in depth for #4411: the payload already hides other providers,
    // but a hallucinated (or stale) provider must not become a route either.
    if !inventory.auto_scope_allows(provider) {
        return None;
    }
    let model = value.get("model").and_then(serde_json::Value::as_str)?;
    let candidate = inventory
        .candidate(provider, model)
        .filter(|candidate| candidate.readiness.can_attempt())?;
    let reasoning_effort = value
        .get("thinking")
        .or_else(|| value.get("reasoning_effort"))
        .or_else(|| value.get("effort"))
        .and_then(serde_json::Value::as_str)
        .and_then(parse_auto_route_reasoning_effort);

    Some(InventoryAutoRouteRecommendation {
        provider,
        model: candidate.model.clone(),
        reasoning_effort,
    })
}

fn auto_route_prompt(
    latest_request: &str,
    recent_context: &str,
    session_mode: &str,
    selected_model_mode: &str,
    selected_thinking_mode: &str,
) -> String {
    format!(
        "Session mode: {}\nSelected model mode: {}\nSelected thinking mode: {}\n\nRecent context:\n{}\n\nLatest user request:\n{}\n\nReturn JSON only.",
        session_mode,
        selected_model_mode,
        selected_thinking_mode,
        if recent_context.trim().is_empty() {
            "No prior context."
        } else {
            recent_context
        },
        truncate_for_auto_router(latest_request, 4_000)
    )
}

fn classifier_prompt(
    client: &DeepSeekClient,
    latest_request: &str,
    recent_context: &str,
    session_mode: &str,
    selected_model_mode: &str,
    selected_thinking_mode: &str,
) -> String {
    client.redact_model_bound_text(&auto_route_prompt(
        latest_request,
        recent_context,
        session_mode,
        selected_model_mode,
        selected_thinking_mode,
    ))
}

fn message_response_text(response: &MessageResponse) -> String {
    let mut out = String::new();
    for block in &response.content {
        match block {
            ContentBlock::Text { text, .. } | ContentBlock::ToolResult { content: text, .. } => {
                append_router_text(&mut out, text);
            }
            ContentBlock::Thinking { thinking, .. } => {
                append_router_text(&mut out, thinking);
            }
            ContentBlock::ToolUse { name, .. } => {
                append_router_text(&mut out, &format!("[tool call: {name}]"));
            }
            _ => {}
        }
    }
    out
}

fn append_router_text(out: &mut String, text: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(text);
}

fn truncate_for_auto_router(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_model_reasoning_keeps_model_and_thinking_choices_independent() {
        assert_eq!(
            resolve_auto_model_reasoning(Some(ReasoningEffort::Low), Some(ReasoningEffort::Max)),
            (Some(ReasoningEffort::Low), false)
        );
        assert_eq!(
            resolve_auto_model_reasoning(Some(ReasoningEffort::Auto), Some(ReasoningEffort::Max)),
            (Some(ReasoningEffort::Max), true)
        );
        assert_eq!(
            resolve_auto_model_reasoning(None, Some(ReasoningEffort::High)),
            (Some(ReasoningEffort::High), true)
        );
    }

    #[test]
    fn auto_model_heuristic_chinese_keywords_route_to_pro() {
        for msg in [
            "\u{5e2e}\u{6211}\u{91cd}\u{6784}\u{8fd9}\u{4e2a}\u{6a21}\u{5757}",
            "\u{8bbe}\u{8ba1}\u{6570}\u{636e}\u{5e93}\u{67b6}\u{6784}",
            "\u{8c03}\u{8bd5}\u{5d29}\u{6e83}\u{95ee}\u{9898}",
            "\u{5ba1}\u{8ba1}\u{5b89}\u{5168}\u{6f0f}\u{6d1e}",
            "\u{8fc1}\u{79fb}\u{5230}\u{65b0}\u{6846}\u{67b6}",
            "\u{4f18}\u{5316}\u{6027}\u{80fd}\u{74f6}\u{9888}",
            "\u{5206}\u{6790}\u{8fd9}\u{6bb5}\u{4ee3}\u{7801}",
        ] {
            assert_eq!(
                auto_model_heuristic(msg, "auto"),
                "deepseek-v4-pro",
                "expected Pro for `{msg}`",
            );
        }
    }

    #[test]
    fn auto_model_heuristic_traditional_chinese_keywords_route_to_pro() {
        for msg in [
            "\u{8acb}\u{91cd}\u{69cb}\u{6b64}\u{6a21}\u{7d44}",
            "\u{67b6}\u{69cb}\u{8a2d}\u{8a08}",
            "\u{4ee3}\u{78bc}\u{8abf}\u{8a66}",
            "\u{5be9}\u{8a08}\u{6f0f}\u{6d1e}",
            "\u{9077}\u{79fb}\u{5230}\u{65b0}\u{67b6}\u{69cb}",
            "\u{512a}\u{5316}\u{6027}\u{80fd}",
            "\u{91cd}\u{5beb}\u{4ee3}\u{78bc}",
            "\u{5be6}\u{73fe}\u{65b0}\u{529f}\u{80fd}",
        ] {
            assert_eq!(
                auto_model_heuristic(msg, "auto"),
                "deepseek-v4-pro",
                "expected Pro for `{msg}`",
            );
        }
    }

    #[test]
    fn auto_model_heuristic_short_chinese_chat_stays_on_flash() {
        assert_eq!(
            auto_model_heuristic("\u{4f60}\u{597d}", "auto"),
            "deepseek-v4-flash",
        );
    }

    #[test]
    fn auto_route_prompt_uses_current_session_mode() {
        let prompt = auto_route_prompt(
            "Please explain the change before editing files.",
            "No prior context.",
            "plan",
            "auto",
            "auto",
        );

        assert!(
            prompt.starts_with("Session mode: plan\n"),
            "auto-route prompt should reflect the active session mode, got: {prompt}"
        );
    }

    #[test]
    fn classifier_prompt_redacts_secret_after_tool_result_flattening() {
        let secret = "cw-router-secret-should-never-leave-process";
        let config = Config {
            api_key: Some(secret.to_string()),
            ..Default::default()
        };
        let client = DeepSeekClient::new(&config).expect("classifier client");
        // `recent_auto_router_context` converts ToolResult blocks into ordinary
        // text before this boundary. Exercise that exact flattened shape.
        let recent_context = format!("assistant: [tool result] token={secret}");

        let prompt = classifier_prompt(
            &client,
            "continue the investigation",
            &recent_context,
            "agent",
            "auto",
            "auto",
        );

        assert!(
            !prompt.contains(secret),
            "flattened tool-result secret leaked"
        );
        assert!(
            prompt.contains(codewhale_config::persistence::REDACTED),
            "secret should be visibly redacted"
        );
        assert!(prompt.contains("continue the investigation"));
    }

    #[test]
    fn inventory_auto_router_prompt_names_cost_saving_zai_pair() {
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _zai = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-key");
        let config = Config {
            provider: Some("zai".to_string()),
            ..Default::default()
        };
        let inventory = ModelInventory::from_config(&config);

        let balanced = inventory_auto_router_system_prompt(&inventory, false);
        let cost_saving = inventory_auto_router_system_prompt(&inventory, true);

        assert!(!balanced.contains("Cost-saving mode is ON"));
        assert!(
            cost_saving.contains(
                "For the active provider `zai`, `GLM-5.3-Flash` is the fast tier and `GLM-5.3` is the strong tier"
            ),
            "cost-saving classifier policy must name the provider-safe pair: {cost_saving}"
        );
        assert!(
            cost_saving.contains("Keep the selected model paired with provider `zai`"),
            "cost-saving policy must preserve provider/model validation: {cost_saving}"
        );
    }

    #[test]
    fn auto_route_effort_normalization_is_provider_aware() {
        assert_eq!(
            normalize_auto_route_effort_for_provider(ApiProvider::Deepseek, ReasoningEffort::Low),
            ReasoningEffort::High
        );
        assert_eq!(
            normalize_auto_route_effort_for_provider(
                ApiProvider::Deepseek,
                ReasoningEffort::Medium
            ),
            ReasoningEffort::High
        );
        assert_eq!(
            normalize_auto_route_effort_for_provider(
                ApiProvider::OpenaiCodex,
                ReasoningEffort::Low
            ),
            ReasoningEffort::Low
        );
        assert_eq!(
            normalize_auto_route_effort_for_provider(
                ApiProvider::OpenaiCodex,
                ReasoningEffort::Medium
            ),
            ReasoningEffort::Medium
        );
        assert_eq!(
            normalize_auto_route_effort_for_provider(
                ApiProvider::OpenaiCodex,
                ReasoningEffort::Off
            ),
            ReasoningEffort::Low
        );
    }

    #[test]
    fn configured_route_effort_normalizer_keeps_kimi_code_low_medium_local() {
        let mut config = Config {
            provider: Some("moonshot".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                moonshot: crate::config::ProviderConfig {
                    base_url: Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                    model: Some("k3".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            normalize_auto_route_effort_for_configured_route(
                &config,
                ApiProvider::Moonshot,
                "k3",
                ReasoningEffort::Low,
            ),
            ReasoningEffort::Low
        );
        assert_eq!(
            normalize_auto_route_effort_for_configured_route(
                &config,
                ApiProvider::Moonshot,
                "k3",
                ReasoningEffort::Medium,
            ),
            ReasoningEffort::Medium
        );

        config
            .providers
            .as_mut()
            .expect("providers")
            .moonshot
            .base_url = Some(crate::config::DEFAULT_MOONSHOT_BASE_URL.to_string());
        assert_eq!(
            normalize_auto_route_effort_for_configured_route(
                &config,
                ApiProvider::Moonshot,
                "k3",
                ReasoningEffort::Low,
            ),
            ReasoningEffort::High
        );
    }

    #[test]
    fn inventory_auto_route_recommendation_requires_runnable_pair() {
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::set("DEEPSEEK_API_KEY", "ds-key");
        let _zai = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-key");
        let config = Config {
            provider: Some("zai".to_string()),
            default_text_model: Some(crate::config::DEFAULT_TEXT_MODEL.to_string()),
            ..Default::default()
        };
        let inventory = ModelInventory::from_config(&config);

        let route = parse_inventory_auto_route_recommendation(
            r#"{"provider":"zai","model":"GLM-5.2","thinking":"max"}"#,
            &inventory,
        )
        .expect("valid inventory route should parse");
        assert_eq!(route.provider, ApiProvider::Zai);
        assert_eq!(route.model, crate::config::ZAI_GLM_5_2_MODEL);
        assert_eq!(route.reasoning_effort, Some(ReasoningEffort::Max));

        assert!(
            parse_inventory_auto_route_recommendation(
                r#"{"provider":"zai","model":"deepseek-v4-pro","thinking":"max"}"#,
                &inventory,
            )
            .is_none(),
            "router must not pair a DeepSeek model with the Z.ai provider"
        );

        let wrapped = parse_inventory_auto_route_recommendation(
            r#"route: {"provider":"zai","model":"GLM-5-Turbo","reasoning_effort":"medium"}"#,
            &inventory,
        )
        .expect("wrapped inventory route should parse");
        assert_eq!(wrapped.provider, ApiProvider::Zai);
        assert_eq!(wrapped.model, crate::config::ZAI_GLM_5_TURBO_MODEL);
        // Parsing is strict and literal; the historic Medium->High coercion
        // is applied downstream by normalize_auto_route_selection_for_config
        // so route-specific contracts (Kimi Code K3) can keep Medium.
        assert_eq!(wrapped.reasoning_effort, Some(ReasoningEffort::Medium));
    }

    #[test]
    fn inventory_auto_route_recommendation_rejects_unready_candidate() {
        let _env_lock = crate::test_support::lock_test_env();
        let _zai = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-key");
        let config = Config {
            provider: Some("zai".to_string()),
            ..Default::default()
        };
        let mut inventory = ModelInventory::from_config(&config);
        let candidate = inventory
            .candidates
            .iter_mut()
            .find(|candidate| {
                candidate.provider == ApiProvider::Zai
                    && candidate.model == crate::config::ZAI_GLM_5_2_MODEL
            })
            .expect("Z.ai strong candidate");
        candidate.readiness = crate::provider_readiness::ResolvedProviderReadiness::InvalidRoute;

        assert!(
            parse_inventory_auto_route_recommendation(
                r#"{"provider":"zai","model":"GLM-5.2","thinking":"max"}"#,
                &inventory,
            )
            .is_none(),
            "classifier output must not revive an unsupported route"
        );
    }

    #[test]
    fn inventory_auto_route_recommendation_accepts_wanjie_v4_ids() {
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::set("DEEPSEEK_API_KEY", "ds-key");
        let _wanjie = crate::test_support::EnvVarGuard::set("WANJIE_ARK_API_KEY", "wanjie-key");
        let config = Config {
            provider: Some("wanjie-ark".to_string()),
            ..Default::default()
        };
        let inventory = ModelInventory::from_config(&config);

        let route = parse_inventory_auto_route_recommendation(
            r#"{"provider":"wanjie-ark","model":"deepseek-v4-pro","thinking":"max"}"#,
            &inventory,
        )
        .expect("Wanjie V4 Pro inventory route should parse");
        assert_eq!(route.provider, ApiProvider::WanjieArk);
        assert_eq!(route.model, "deepseek-v4-pro");
        assert_eq!(route.reasoning_effort, Some(ReasoningEffort::Max));

        let route = parse_inventory_auto_route_recommendation(
            r#"{"provider":"wanjie-ark","model":"deepseek-v4-flash","thinking":"off"}"#,
            &inventory,
        )
        .expect("Wanjie V4 Flash inventory route should parse");
        assert_eq!(route.provider, ApiProvider::WanjieArk);
        assert_eq!(route.model, "deepseek-v4-flash");
        assert_eq!(route.reasoning_effort, Some(ReasoningEffort::Off));
    }

    #[test]
    fn explicit_route_to_nonactive_provider_uses_that_providers_effort() {
        // Active provider is DeepSeek (whose effort floor is low/medium), but the
        // explicit model `GLM-5.2` only routes to Z.ai. The resolved effort must
        // be normalized for Z.ai — not left at DeepSeek's raw `low` setting.
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::set("DEEPSEEK_API_KEY", "ds-key");
        let _zai = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-key");
        let config = Config {
            provider: Some("deepseek".to_string()),
            reasoning_effort: Some("low".to_string()),
            ..Default::default()
        };

        let route = resolve_explicit_route_with_inventory(&config, "GLM-5.2")
            .expect("explicit GLM route should resolve to its provider");

        assert_eq!(
            route.provider,
            ApiProvider::Zai,
            "GLM-5.2 must route to Z.ai, not the active DeepSeek provider"
        );
        assert_eq!(
            route.reasoning_effort,
            Some(ReasoningEffort::High),
            "low must be normalized up to high for the Z.ai route, not passed through"
        );

        // GLM-5.3 is the default and a first-class route: same provider
        // ownership, same effort normalization, and it resolves to its own id.
        let route_53 = resolve_explicit_route_with_inventory(&config, "GLM-5.3")
            .expect("explicit GLM-5.3 route should resolve to its provider");
        assert_eq!(
            route_53.provider,
            ApiProvider::Zai,
            "GLM-5.3 must route to Z.ai, not the active DeepSeek provider"
        );
        assert_eq!(
            route_53.model,
            crate::config::ZAI_GLM_5_3_MODEL,
            "GLM-5.3 must resolve to its own id"
        );
        assert_eq!(route_53.reasoning_effort, Some(ReasoningEffort::High));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn inventory_auto_route_resolves_active_authenticated_provider() {
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::set("DEEPSEEK_API_KEY", "ds-key");
        let _zai = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-key");
        let config = Config {
            provider: Some("zai".to_string()),
            ..Default::default()
        };

        let route =
            resolve_auto_route_with_inventory(&config, "quick status check", "", "auto", "auto")
                .await
                .expect("inventory route should resolve with authenticated active provider");

        assert_eq!(route.provider, ApiProvider::Zai);
        assert_eq!(route.model, crate::config::ZAI_GLM_5_3_FLASH_MODEL);
        assert_eq!(route.source, AutoRouteSource::Heuristic);
        let receipt = route.receipt.expect("Auto route receipt");
        assert_eq!(receipt.tier, AutoRouteTier::Fast);
        assert_eq!(receipt.scope, AutoRouteScope::ResolvedProvider);
        assert_eq!(receipt.data_path, AutoRouteDataPath::LocalHeuristic);
        assert_eq!(
            receipt.reason,
            AutoRouteReason::LocalHeuristic(AutoRouteHeuristicReason::ShortRequest)
        );
        assert_eq!(receipt.pair.strong, crate::config::DEFAULT_ZAI_MODEL);
        assert_eq!(
            receipt.pair.fast.as_deref(),
            Some(crate::config::ZAI_GLM_5_3_FLASH_MODEL)
        );
    }

    #[test]
    fn classifier_receipt_discloses_active_provider_scope_and_data_path() {
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::set("DEEPSEEK_API_KEY", "ds-key");
        let _zai = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-key");
        let config = Config {
            provider: Some("zai".to_string()),
            ..Default::default()
        };
        let inventory = ModelInventory::from_config(&config);
        let recommendation = parse_inventory_auto_route_recommendation(
            r#"{"provider":"zai","model":"GLM-5-Turbo","thinking":"off"}"#,
            &inventory,
        )
        .expect("runnable classifier recommendation");

        let route = auto_route_from_classifier(&inventory, recommendation);

        assert_eq!(route.provider, ApiProvider::Zai);
        assert_eq!(route.model, crate::config::ZAI_GLM_5_TURBO_MODEL);
        assert_eq!(route.source, AutoRouteSource::FlashRouter);
        let receipt = route.receipt.expect("classifier receipt");
        assert_eq!(receipt.tier, AutoRouteTier::Fast);
        // #4411: the classifier only saw Z.ai routes, so the receipt says so
        // instead of claiming the wider runnable-providers scope.
        assert_eq!(receipt.scope, AutoRouteScope::ActiveProvider);
        assert_eq!(
            receipt.data_path,
            AutoRouteDataPath::Classifier {
                provider: ApiProvider::Deepseek,
                model: "deepseek-v4-flash".to_string(),
            }
        );
        assert_eq!(receipt.reason, AutoRouteReason::ClassifierRecommendation);
    }

    #[test]
    fn classifier_recommendation_for_another_provider_is_refused_by_default() {
        // #4411: the payload never named DeepSeek, but a classifier can still
        // emit one. The recommendation must not become a route unless the
        // persisted cross-provider opt-in is set.
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::set("DEEPSEEK_API_KEY", "ds-key");
        let _zai = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-key");
        let scoped = Config {
            provider: Some("zai".to_string()),
            ..Default::default()
        };
        let scoped_inventory = ModelInventory::from_config(&scoped);
        let raw = r#"{"provider":"deepseek","model":"deepseek-v4-flash","thinking":"off"}"#;

        assert!(
            parse_inventory_auto_route_recommendation(raw, &scoped_inventory).is_none(),
            "cross-provider classifier output must be refused by default"
        );
        // The same inventory still accepts an in-scope active-provider route.
        assert!(
            parse_inventory_auto_route_recommendation(
                r#"{"provider":"zai","model":"GLM-5.2","thinking":"max"}"#,
                &scoped_inventory,
            )
            .is_some()
        );

        let opted_in = Config {
            auto: Some(crate::config::AutoConfig {
                cost_saving: None,
                cross_provider: Some(true),
                router: None,
            }),
            ..scoped.clone()
        };
        let opted_in_route =
            parse_inventory_auto_route_recommendation(raw, &ModelInventory::from_config(&opted_in))
                .expect("opt-in admits the cross-provider recommendation");
        assert_eq!(opted_in_route.provider, ApiProvider::Deepseek);
    }

    #[test]
    fn classifier_prompt_declares_active_provider_scope_by_default() {
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::set("DEEPSEEK_API_KEY", "ds-key");
        let _zai = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-key");
        let config = Config {
            provider: Some("zai".to_string()),
            ..Default::default()
        };

        let prompt =
            inventory_auto_router_system_prompt(&ModelInventory::from_config(&config), false);

        assert!(
            prompt.contains("Auto routing is scoped to the active provider `zai`"),
            "{prompt}"
        );
        assert!(!prompt.contains("deepseek"), "{prompt}");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn active_provider_strong_fast_selection_survives_scoping() {
        // Same-provider tier selection is the behavior scoping must not
        // break: a complex request still reaches the active provider's strong
        // tier, a trivial one still reaches its fast tier (#4411).
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _zai = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-key");
        let config = Config {
            provider: Some("zai".to_string()),
            ..Default::default()
        };

        let strong = resolve_auto_route_with_inventory(
            &config,
            "refactor the routing module and audit its security boundaries",
            "",
            "auto",
            "auto",
        )
        .await
        .expect("strong-tier route");
        assert_eq!(strong.provider, ApiProvider::Zai);
        assert_eq!(strong.model, crate::config::DEFAULT_ZAI_MODEL);
        let strong_receipt = strong.receipt.expect("strong receipt");
        assert_eq!(strong_receipt.tier, AutoRouteTier::Strong);
        assert_eq!(strong_receipt.scope, AutoRouteScope::ResolvedProvider);

        let fast = resolve_auto_route_with_inventory(&config, "hi", "", "auto", "auto")
            .await
            .expect("fast-tier route");
        assert_eq!(fast.provider, ApiProvider::Zai);
        assert_eq!(fast.model, crate::config::ZAI_GLM_5_3_FLASH_MODEL);
        assert_eq!(
            fast.receipt.expect("fast receipt").tier,
            AutoRouteTier::Fast
        );
    }

    #[test]
    fn config_auto_cross_provider_defaults_to_false() {
        assert!(!Config::default().auto_cross_provider());
        let opted_in = Config {
            auto: Some(crate::config::AutoConfig {
                cost_saving: None,
                cross_provider: Some(true),
                router: None,
            }),
            ..Default::default()
        };
        assert!(opted_in.auto_cross_provider());
    }

    #[test]
    fn classifier_receipt_never_reports_openrouter_default_for_another_family() {
        let _env_lock = crate::test_support::lock_test_env();
        let _openrouter =
            crate::test_support::EnvVarGuard::set("OPENROUTER_API_KEY", "openrouter-key");
        let config = Config {
            provider: Some("openrouter".to_string()),
            ..Default::default()
        };
        let inventory = ModelInventory::from_config(&config);
        let recommendation = parse_inventory_auto_route_recommendation(
            r#"{"provider":"openrouter","model":"z-ai/glm-5.2","thinking":"max"}"#,
            &inventory,
        )
        .expect("runnable non-default OpenRouter family");

        let route = auto_route_from_classifier(&inventory, recommendation);
        let receipt = route.receipt.expect("classifier receipt");

        assert_eq!(route.model, crate::config::OPENROUTER_GLM_5_2_MODEL);
        assert_eq!(receipt.pair.strong, crate::config::OPENROUTER_GLM_5_2_MODEL);
        assert_ne!(
            receipt.pair.fast.as_deref(),
            Some(crate::config::DEFAULT_OPENROUTER_FLASH_MODEL),
            "a GLM selection must not be described as the DeepSeek default pair"
        );
        assert!(matches!(
            receipt.tier,
            AutoRouteTier::Strong | AutoRouteTier::Only
        ));
    }

    #[test]
    fn classifier_fallback_preserves_attempted_data_path_without_error_text() {
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::set("DEEPSEEK_API_KEY", "ds-key");
        let _zai = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-key");
        let config = Config {
            provider: Some("zai".to_string()),
            ..Default::default()
        };
        let inventory = ModelInventory::from_config(&config);
        let heuristic = auto_route_from_inventory_heuristic(&config, "quick status", &inventory);

        let route = auto_route_classifier_fallback(heuristic, &inventory);

        assert_eq!(route.source, AutoRouteSource::Heuristic);
        let receipt = route.receipt.expect("fallback receipt");
        assert_eq!(receipt.scope, AutoRouteScope::ResolvedProvider);
        assert!(matches!(
            receipt.data_path,
            AutoRouteDataPath::Classifier {
                provider: ApiProvider::Deepseek,
                ref model,
            } if model == "deepseek-v4-flash"
        ));
        assert_eq!(
            receipt.reason,
            AutoRouteReason::ClassifierFallback(AutoRouteHeuristicReason::ShortRequest)
        );
        assert!(!receipt.reason.label().contains("secret-provider-error"));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn inventory_auto_route_never_falls_back_across_providers_by_default() {
        // #4411: the active provider has no usable credential, but another
        // provider does. Auto must stay on the active provider and report a
        // no-runnable-candidate heuristic instead of silently spending the
        // other provider's key.
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::set("DEEPSEEK_API_KEY", "ds-key");
        let _zai = crate::test_support::EnvVarGuard::remove("ZAI_API_KEY");
        let config = Config {
            provider: Some("zai".to_string()),
            ..Default::default()
        };

        let route =
            resolve_auto_route_with_inventory(&config, "quick status check", "", "auto", "auto")
                .await
                .expect("inventory route should resolve without leaving the active provider");

        assert_eq!(route.provider, ApiProvider::Zai);
        assert_ne!(route.provider, ApiProvider::Deepseek);
        assert_eq!(route.source, AutoRouteSource::Heuristic);
        let receipt = route.receipt.expect("Auto route receipt");
        assert_eq!(receipt.scope, AutoRouteScope::ResolvedProvider);
        assert_eq!(
            receipt.reason,
            AutoRouteReason::LocalHeuristic(AutoRouteHeuristicReason::NoRunnableCandidate)
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn inventory_auto_route_crosses_providers_only_under_persisted_opt_in() {
        // The same configuration as above, plus the persisted
        // `[auto] cross_provider = true` opt-in (#4411).
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::set("DEEPSEEK_API_KEY", "ds-key");
        let _zai = crate::test_support::EnvVarGuard::remove("ZAI_API_KEY");
        let config = Config {
            provider: Some("zai".to_string()),
            auto: Some(crate::config::AutoConfig {
                cost_saving: None,
                cross_provider: Some(true),
                router: None,
            }),
            ..Default::default()
        };

        let route =
            resolve_auto_route_with_inventory(&config, "quick status check", "", "auto", "auto")
                .await
                .expect("opted-in route should fall back to an authenticated provider");

        assert_eq!(route.provider, ApiProvider::Deepseek);
        assert_eq!(route.model, "deepseek-v4-flash");
        assert_eq!(route.source, AutoRouteSource::Heuristic);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn inventory_auto_route_cost_saving_changes_borderline_zai_route() {
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _zai = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-key");
        let balanced = Config {
            provider: Some("zai".to_string()),
            ..Default::default()
        };
        let cost_saving = Config {
            auto: Some(crate::config::AutoConfig {
                cost_saving: Some(true),
                cross_provider: None,
                router: None,
            }),
            ..balanced.clone()
        };

        let balanced_route = resolve_auto_route_with_inventory(
            &balanced,
            "Please implement a binary search",
            "",
            "auto",
            "auto",
        )
        .await
        .expect("balanced Auto route should resolve");
        let cost_saving_route = resolve_auto_route_with_inventory(
            &cost_saving,
            "Please implement a binary search",
            "",
            "auto",
            "auto",
        )
        .await
        .expect("cost-saving Auto route should resolve");

        assert_eq!(balanced_route.provider, ApiProvider::Zai);
        assert_eq!(balanced_route.model, crate::config::DEFAULT_ZAI_MODEL);
        assert_eq!(cost_saving_route.provider, ApiProvider::Zai);
        assert_eq!(
            cost_saving_route.model,
            crate::config::ZAI_GLM_5_3_FLASH_MODEL
        );
        assert_eq!(cost_saving_route.source, AutoRouteSource::Heuristic);
        assert_eq!(
            balanced_route
                .receipt
                .as_ref()
                .map(|receipt| (receipt.tier, receipt.reason)),
            Some((
                AutoRouteTier::Strong,
                AutoRouteReason::LocalHeuristic(AutoRouteHeuristicReason::ComplexRequest),
            ))
        );
        assert_eq!(
            cost_saving_route
                .receipt
                .as_ref()
                .map(|receipt| (receipt.tier, receipt.reason)),
            Some((
                AutoRouteTier::Fast,
                AutoRouteReason::LocalHeuristic(AutoRouteHeuristicReason::CostSavingPolicy),
            ))
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn inventory_auto_route_uses_wanjie_v4_pair_without_deepseek_router() {
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _wanjie = crate::test_support::EnvVarGuard::set("WANJIE_ARK_API_KEY", "wanjie-key");
        let config = Config {
            provider: Some("wanjie-ark".to_string()),
            default_text_model: Some("auto".to_string()),
            ..Default::default()
        };

        let route =
            resolve_auto_route_with_inventory(&config, "quick status check", "", "auto", "auto")
                .await
                .expect("heuristic-only Wanjie route should resolve");
        assert_eq!(route.provider, ApiProvider::WanjieArk);
        assert_eq!(route.model, "deepseek-v4-flash");
        assert_eq!(route.source, AutoRouteSource::Heuristic);

        let route = resolve_auto_route_with_inventory(
            &config,
            "please refactor this architecture",
            "",
            "auto",
            "auto",
        )
        .await
        .expect("complex Wanjie route should resolve");
        assert_eq!(route.provider, ApiProvider::WanjieArk);
        assert_eq!(route.model, "deepseek-v4-pro");
        assert_eq!(route.source, AutoRouteSource::Heuristic);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn inventory_auto_route_uses_volcengine_v4_pair_without_deepseek_router() {
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _volcengine =
            crate::test_support::EnvVarGuard::set("VOLCENGINE_API_KEY", "volcengine-key");
        let config = Config {
            provider: Some("volcengine".to_string()),
            default_text_model: Some("auto".to_string()),
            ..Default::default()
        };

        let route =
            resolve_auto_route_with_inventory(&config, "quick status check", "", "auto", "auto")
                .await
                .expect("heuristic-only Volcengine route should resolve");
        assert_eq!(route.provider, ApiProvider::Volcengine);
        assert_eq!(route.model, "DeepSeek-V4-Flash");
        assert_eq!(route.source, AutoRouteSource::Heuristic);

        let route = resolve_auto_route_with_inventory(
            &config,
            "please refactor this architecture",
            "",
            "auto",
            "auto",
        )
        .await
        .expect("complex Volcengine route should resolve");
        assert_eq!(route.provider, ApiProvider::Volcengine);
        assert_eq!(route.model, "DeepSeek-V4-Pro");
        assert_eq!(route.source, AutoRouteSource::Heuristic);
    }

    #[test]
    fn auto_heuristic_default_routes_implement_to_pro() {
        assert_eq!(
            auto_model_heuristic_with_bias("Please implement a binary search", "auto", false),
            "deepseek-v4-pro"
        );
    }

    #[test]
    fn auto_heuristic_cost_saving_keeps_borderline_keywords_on_flash() {
        assert_eq!(
            auto_model_heuristic_with_bias("Please implement a binary search", "auto", true),
            "deepseek-v4-flash"
        );
        assert_eq!(
            auto_model_heuristic_with_bias("analyze this snippet", "auto", true),
            "deepseek-v4-flash"
        );
    }

    #[test]
    fn auto_heuristic_strong_keywords_still_route_to_pro_under_cost_saving() {
        for kw in [
            "refactor",
            "architecture",
            "design",
            "debug",
            "security",
            "review",
            "audit",
            "migrate",
            "optimize",
            "rewrite",
        ] {
            let req = format!("Please {kw} this module");
            assert_eq!(
                auto_model_heuristic_with_bias(&req, "auto", true),
                "deepseek-v4-pro",
                "expected Pro for strong keyword `{kw}` even in cost-saving mode"
            );
        }
    }

    #[test]
    fn auto_heuristic_cost_saving_raises_long_message_threshold() {
        let body = "filler sentence. ".repeat(40);
        assert_eq!(
            auto_model_heuristic_with_bias(&body, "auto", false),
            "deepseek-v4-pro"
        );
        assert_eq!(
            auto_model_heuristic_with_bias(&body, "auto", true),
            "deepseek-v4-flash"
        );
    }

    #[test]
    fn provider_router_candidates_cover_known_provider_classes() {
        use crate::config::ApiProvider;

        let deepseek = provider_router_candidates(ApiProvider::Deepseek, "deepseek-v4-pro");
        assert_eq!(deepseek.big, "deepseek-v4-pro");
        assert_eq!(deepseek.cheap.as_deref(), Some("deepseek-v4-flash"));

        let openrouter =
            provider_router_candidates(ApiProvider::Openrouter, "deepseek/deepseek-v4-pro");
        assert_eq!(openrouter.big, "deepseek/deepseek-v4-pro");
        assert_eq!(
            openrouter.cheap.as_deref(),
            Some("deepseek/deepseek-v4-flash")
        );

        let wanjie = provider_router_candidates(ApiProvider::WanjieArk, "deepseek-reasoner");
        assert_eq!(wanjie.big, "deepseek-v4-pro");
        assert_eq!(wanjie.cheap.as_deref(), Some("deepseek-v4-flash"));

        let volcengine = provider_router_candidates(ApiProvider::Volcengine, "DeepSeek-V4-Pro");
        assert_eq!(volcengine.big, "DeepSeek-V4-Pro");
        assert_eq!(volcengine.cheap.as_deref(), Some("DeepSeek-V4-Flash"));

        let zai = provider_router_candidates(ApiProvider::Zai, "GLM-5.2");
        assert_eq!(zai.big, "GLM-5.2");
        // GLM-5.2 faster/explore children route to GLM-5-Turbo (same-family fast
        // sibling), not back down to GLM-5.1.
        assert_eq!(zai.cheap.as_deref(), Some("GLM-5-Turbo"));

        let openrouter_glm = provider_router_candidates(ApiProvider::Openrouter, "z-ai/glm-5.2");
        assert_eq!(openrouter_glm.big, "z-ai/glm-5.2");
        assert_eq!(openrouter_glm.cheap.as_deref(), Some("z-ai/glm-5-turbo"));

        // GLM-5.3's fast sibling is Flash; GLM-5.2 still uses Turbo.
        let zai_53 = provider_router_candidates(ApiProvider::Zai, "GLM-5.3");
        assert_eq!(zai_53.big, "GLM-5.3");
        assert_eq!(zai_53.cheap.as_deref(), Some("GLM-5.3-Flash"));

        let openrouter_glm_53 = provider_router_candidates(ApiProvider::Openrouter, "z-ai/glm-5.3");
        assert_eq!(openrouter_glm_53.big, "z-ai/glm-5.3");
        assert_eq!(
            openrouter_glm_53.cheap.as_deref(),
            Some("z-ai/glm-5.3-flash")
        );

        let zai_flash = provider_router_candidates(ApiProvider::Zai, "GLM-5.3-Flash");
        assert_eq!(zai_flash.big, "GLM-5.3-Flash");
        assert_eq!(zai_flash.cheap, None);

        // GLM-5.1 has no cheaper tier; faster children stay on the parent.
        let zai_51 = provider_router_candidates(ApiProvider::Zai, "GLM-5.1");
        assert_eq!(zai_51.big, "GLM-5.1");
        assert_eq!(zai_51.cheap, None);

        // GLM-5-Turbo is itself the cheap tier; no further downgrade.
        let zai_turbo = provider_router_candidates(ApiProvider::Zai, "GLM-5-Turbo");
        assert_eq!(zai_turbo.big, "GLM-5-Turbo");
        assert_eq!(zai_turbo.cheap, None);

        // Providers without a known cheap tier: big = session model, no cheap.
        let ollama = provider_router_candidates(ApiProvider::Ollama, "qwen3:32b");
        assert_eq!(ollama.big, "qwen3:32b");
        assert_eq!(ollama.cheap, None);

        let moonshot = provider_router_candidates(ApiProvider::Moonshot, "kimi-k2.6");
        assert_eq!(moonshot.big, "kimi-k2.6");
        assert_eq!(moonshot.cheap, None);
    }

    #[test]
    fn provider_router_candidates_cover_catalog_fast_siblings() {
        use crate::config::ApiProvider;

        let cases = [
            (ApiProvider::OpenaiCodex, "gpt-5.6-sol", "gpt-5.6-luna"),
            (
                ApiProvider::Anthropic,
                "claude-sonnet-4-6",
                "claude-haiku-4-5",
            ),
            (ApiProvider::XiaomiMimo, "mimo-v2.5-pro", "mimo-v2.5"),
            (ApiProvider::Arcee, "trinity-large-thinking", "trinity-mini"),
            (ApiProvider::Moonshot, "kimi-k2.7-code", "kimi-k2.6"),
            (
                ApiProvider::Minimax,
                "MiniMax-M2.7",
                "MiniMax-M2.7-highspeed",
            ),
            (ApiProvider::OpencodeGo, "kimi-k3", "kimi-k2.7-code"),
            (
                ApiProvider::Openrouter,
                "qwen/qwen3.6-max-preview",
                "qwen/qwen3.6-flash",
            ),
            (
                ApiProvider::Openrouter,
                "anthropic/claude-sonnet-4-6",
                "anthropic/claude-haiku-4-5",
            ),
        ];

        for (provider, strong, fast) in cases {
            let candidates = provider_router_candidates(provider, strong);
            assert_eq!(candidates.big, strong);
            assert_eq!(candidates.cheap.as_deref(), Some(fast));
            assert_eq!(
                provider_router_candidates(provider, fast).cheap,
                None,
                "already-fast model must not downgrade again: {provider:?}/{fast}"
            );
        }

        for (provider, model) in [
            (ApiProvider::Ollama, "qwen3:32b"),
            (ApiProvider::Custom, "gpt-5.6-sol"),
            (ApiProvider::OpenaiCodex, "gpt-5.6-luna"),
        ] {
            assert_eq!(provider_router_candidates(provider, model).cheap, None);
        }
    }

    #[test]
    fn heuristic_without_cheap_tier_always_returns_current_model() {
        // #3018 AC: Ollama + auto must never fabricate a DeepSeek id.
        let candidates = RouterCandidates {
            big: "qwen3:32b".to_string(),
            cheap: None,
        };
        for cost_saving in [false, true] {
            for prompt in [
                "hi",
                "please refactor the auth module for security",
                &"long filler sentence. ".repeat(60),
            ] {
                let model = auto_model_heuristic_with_bias_for_candidates(
                    prompt,
                    "qwen3:32b",
                    cost_saving,
                    &candidates,
                )
                .model;
                assert_eq!(model, "qwen3:32b", "prompt {prompt:?}");
            }
        }
    }

    #[test]
    fn config_auto_cost_saving_defaults_to_false() {
        let cfg = Config::default();
        assert!(!cfg.auto_cost_saving());
    }

    #[test]
    fn config_auto_cost_saving_reads_table() {
        let cfg = Config {
            auto: Some(crate::config::AutoConfig {
                cost_saving: Some(true),
                cross_provider: None,
                router: None,
            }),
            ..Default::default()
        };
        assert!(cfg.auto_cost_saving());
    }
}
