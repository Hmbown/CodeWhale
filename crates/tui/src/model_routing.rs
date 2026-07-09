//! Model selection and auto-routing.
//!
//! The CLI, TUI, runtime threads, subagents, and command handlers all need
//! this behavior, so it intentionally lives outside the command tree.

use std::time::Duration;

use anyhow::Result;

use crate::client::DeepSeekClient;
use crate::config::{ApiProvider, Config, normalize_model_name_for_provider};
use crate::llm_client::LlmClient;
use crate::model_inventory::ModelInventory;
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
    if provider == ApiProvider::Zai {
        let normalized = crate::config::normalize_model_name_for_provider(provider, current_model)
            .unwrap_or_else(|| current_model.to_string());
        return RouterCandidates {
            // GLM-5.2 (the default) routes faster/explore children to GLM-5-Turbo,
            // the same-family fast sibling. GLM-5.1 and GLM-5-Turbo itself have no
            // cheaper tier and keep children on the parent model.
            cheap: if normalized == crate::config::ZAI_GLM_5_2_MODEL {
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
                | crate::config::OPENROUTER_GLM_5_TURBO_MODEL
        )
    {
        return RouterCandidates {
            // z-ai/glm-5.2 routes faster children to z-ai/glm-5-turbo; the 5.1
            // and turbo ids have no cheaper tier and keep children on parent.
            cheap: if normalized == crate::config::OPENROUTER_GLM_5_2_MODEL {
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
        | ApiProvider::WanjieArk => RouterCandidates {
            big: crate::config::wire_model_for_provider(provider, "deepseek-v4-pro"),
            cheap: Some(crate::config::wire_model_for_provider(
                provider,
                "deepseek-v4-flash",
            )),
        },
        ApiProvider::Volcengine => RouterCandidates {
            big: crate::config::DEFAULT_VOLCENGINE_MODEL.to_string(),
            cheap: Some(crate::config::DEFAULT_VOLCENGINE_FLASH_MODEL.to_string()),
        },
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
    auto_model_heuristic_with_bias_for_candidates(input, current_model, false, candidates)
}

#[cfg(test)]
fn auto_model_heuristic_with_bias(input: &str, current_model: &str, cost_saving: bool) -> String {
    auto_model_heuristic_with_bias_for_candidates(
        input,
        current_model,
        cost_saving,
        &RouterCandidates::deepseek(),
    )
}

fn auto_model_heuristic_with_bias_for_candidates(
    input: &str,
    _current_model: &str,
    cost_saving: bool,
    candidates: &RouterCandidates,
) -> String {
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
        return candidates.big.clone();
    }
    if len < 100 {
        return candidates.cheap_or_big().to_string();
    }
    let long_threshold = if cost_saving { 1_000 } else { 500 };
    if len > long_threshold {
        return candidates.big.clone();
    }

    candidates.cheap_or_big().to_string()
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
            AutoRouteSource::FlashRouter => "flash-router",
            AutoRouteSource::Heuristic => "heuristic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutoRouteSelection {
    pub(crate) provider: ApiProvider,
    pub(crate) model: String,
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
    pub(crate) source: AutoRouteSource,
}

fn extract_first_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    (end >= start).then_some(&raw[start..=end])
}

fn parse_auto_route_reasoning_effort(effort: &str) -> Option<ReasoningEffort> {
    match effort.trim().to_ascii_lowercase().as_str() {
        "off" | "disabled" | "none" | "false" => Some(ReasoningEffort::Off),
        "low" | "minimal" | "medium" | "mid" => Some(ReasoningEffort::High),
        "high" => Some(ReasoningEffort::High),
        "max" | "maximum" | "xhigh" | "ultracode" => Some(ReasoningEffort::Max),
        _ => None,
    }
}

#[must_use]
pub(crate) fn normalize_auto_route_effort(effort: ReasoningEffort) -> ReasoningEffort {
    normalize_auto_route_effort_for_provider(ApiProvider::Deepseek, effort)
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
    let inventory = ModelInventory::from_config(config);
    if !inventory.router_available {
        // Fall back to heuristic-only auto routing when the flash router
        // is unavailable (e.g. non-DeepSeek providers like wanjie-ark).
        return Ok(auto_route_from_inventory_heuristic(
            config,
            latest_request,
            &inventory,
        ));
    }

    let heuristic = auto_route_from_inventory_heuristic(config, latest_request, &inventory);
    if cfg!(test) {
        return Ok(heuristic);
    }

    match auto_route_inventory_recommendation(
        config,
        &inventory,
        latest_request,
        recent_context,
        session_mode,
        selected_model_mode,
        selected_thinking_mode,
    )
    .await
    {
        Ok(Some(recommendation)) => Ok(AutoRouteSelection {
            provider: recommendation.provider,
            model: recommendation.model,
            reasoning_effort: recommendation.reasoning_effort,
            source: AutoRouteSource::FlashRouter,
        }),
        Ok(None) | Err(_) => Ok(heuristic),
    }
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
                normalize_auto_route_effort_for_provider(
                    candidate.provider,
                    ReasoningEffort::from_setting(setting),
                )
            }),
            source: AutoRouteSource::Heuristic,
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
            normalize_auto_route_effort_for_provider(
                candidate.provider,
                ReasoningEffort::from_setting(setting),
            )
        }),
        source: AutoRouteSource::Heuristic,
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
        return AutoRouteSelection {
            provider: config.api_provider(),
            model: config.default_model(),
            reasoning_effort: Some(crate::auto_reasoning::select(false, latest_request)),
            source: AutoRouteSource::Heuristic,
        };
    };
    // Use the candidates' cheap/big info for complexity-based routing.
    let router_candidates = provider_router_candidates(config.api_provider(), &active.model);
    let chosen = if router_candidates.cheap.is_some() {
        auto_model_heuristic_for_candidates(latest_request, &active.model, &router_candidates)
    } else {
        active.model.clone()
    };
    AutoRouteSelection {
        provider: active.provider,
        model: chosen,
        reasoning_effort: Some(crate::auto_reasoning::select(false, latest_request)),
        source: AutoRouteSource::Heuristic,
    }
}

async fn auto_route_inventory_recommendation(
    config: &Config,
    inventory: &ModelInventory,
    latest_request: &str,
    recent_context: &str,
    session_mode: &str,
    selected_model_mode: &str,
    selected_thinking_mode: &str,
) -> Result<Option<InventoryAutoRouteRecommendation>> {
    let mut router_config = config.clone();
    router_config.provider = Some(ApiProvider::Deepseek.as_str().to_string());
    router_config.default_text_model = Some(inventory.router_model.to_string());

    let client = DeepSeekClient::new(&router_config)?;
    let router_system = inventory_auto_router_system_prompt(inventory);
    let request = MessageRequest {
        model: inventory.router_model.to_string(),
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: auto_route_prompt(
                    latest_request,
                    recent_context,
                    session_mode,
                    selected_model_mode,
                    selected_thinking_mode,
                ),
                cache_control: None,
            }],
        }],
        max_tokens: 128,
        system: Some(SystemPrompt::Text(router_system)),
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort: Some("off".to_string()),
        stream: Some(false),
        temperature: Some(0.0),
        top_p: None,
    };

    let response =
        tokio::time::timeout(Duration::from_secs(4), client.create_message(request)).await??;
    Ok(parse_inventory_auto_route_recommendation(
        &message_response_text(&response),
        inventory,
    ))
}

fn inventory_auto_router_system_prompt(inventory: &ModelInventory) -> String {
    format!(
        "You are the codewhale model-routing classifier. Return only compact JSON: \
{{\"provider\":\"<provider>\",\"model\":\"<model>\",\"thinking\":\"off|high|max\"}}.\n\
Choose only provider/model pairs present in the inventory JSON. Use off only for trivial no-tool answers, \
high for ordinary reasoning, and max for agentic, coding, multi-file, release, architecture, debugging, \
security, tool-heavy, or uncertain work.\n\nInventory JSON:\n{}",
        inventory.router_context_json()
    )
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
    let model = value.get("model").and_then(serde_json::Value::as_str)?;
    let candidate = inventory.candidate(provider, model)?;
    let reasoning_effort = value
        .get("thinking")
        .or_else(|| value.get("reasoning_effort"))
        .or_else(|| value.get("effort"))
        .and_then(serde_json::Value::as_str)
        .and_then(parse_auto_route_reasoning_effort)
        .map(|effort| normalize_auto_route_effort_for_provider(provider, effort));

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
        assert_eq!(wrapped.reasoning_effort, Some(ReasoningEffort::High));
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
        assert_eq!(route.model, crate::config::ZAI_GLM_5_TURBO_MODEL);
        assert_eq!(route.source, AutoRouteSource::Heuristic);
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
    fn heuristic_without_cheap_tier_always_returns_current_model() {
        // #3018 AC: Ollama + auto must never fabricate a DeepSeek id.
        let candidates = RouterCandidates {
            big: "qwen3:32b".to_string(),
            cheap: None,
        };
        for prompt in [
            "hi",
            "please refactor the auth module for security",
            &"long filler sentence. ".repeat(60),
        ] {
            let model = auto_model_heuristic_for_candidates(prompt, "qwen3:32b", &candidates);
            assert_eq!(model, "qwen3:32b", "prompt {prompt:?}");
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
            }),
            ..Default::default()
        };
        assert!(cfg.auto_cost_saving());
    }
}
