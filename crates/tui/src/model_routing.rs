//! Model selection and auto-routing.
//!
//! This module owns runtime model-routing decisions. It is used by the CLI,
//! TUI, runtime threads, and subagent tools, so it intentionally lives outside
//! the command tree.

use crate::client::DeepSeekClient;
use crate::config::Config;
use crate::llm_client::LlmClient;
use crate::models::{ContentBlock, Message, MessageRequest, MessageResponse, SystemPrompt};
use crate::tui::app::ReasoningEffort;
use std::time::Duration;

pub(crate) fn auto_model_heuristic(input: &str, _current_model: &str) -> String {
    auto_model_heuristic_with_bias(input, _current_model, false)
}

/// `auto_model_heuristic` parameterised by the `[auto] cost_saving` opt-in
/// (#1207). When `cost_saving` is `true` the keyword set drops the borderline
/// triggers (`implement`, `analyze`) and the long-message length threshold
/// goes from 500 to 1000 — both shifts let "looks involved but might be a
/// one-liner" requests stay on Flash unless they actually look agentic.
fn auto_model_heuristic_with_bias(input: &str, _current_model: &str, cost_saving: bool) -> String {
    auto_model_heuristic_selection_with_bias(input, _current_model, cost_saving).model
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoModelHeuristicConfidence {
    Decisive,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AutoModelHeuristicSelection {
    model: String,
    confidence: AutoModelHeuristicConfidence,
}

fn auto_model_heuristic_selection_with_bias(
    input: &str,
    _current_model: &str,
    cost_saving: bool,
) -> AutoModelHeuristicSelection {
    let len = input.chars().count();
    let lower = input.to_lowercase();
    let borderline_pro_keywords: &[&str] = &[
        "implement",
        "analyze",
        "\u{5b9e}\u{73b0}", // 实现
        "\u{5206}\u{6790}", // 分析
        "\u{5be6}\u{73fe}", // 實現
    ];
    let strong_match = COMPLEX_KEYWORDS
        .iter()
        .any(|kw| !borderline_pro_keywords.contains(kw) && lower.contains(kw));
    let borderline_match = borderline_pro_keywords.iter().any(|kw| lower.contains(kw));
    let pro_match = strong_match || (!cost_saving && borderline_match);
    if pro_match {
        return AutoModelHeuristicSelection {
            model: "deepseek-v4-pro".to_string(),
            confidence: AutoModelHeuristicConfidence::Decisive,
        };
    }
    // Short messages → Flash
    if len < 100 {
        return AutoModelHeuristicSelection {
            model: "deepseek-v4-flash".to_string(),
            confidence: AutoModelHeuristicConfidence::Decisive,
        };
    }
    // Long complex requests → Pro. Cost-saving raises the threshold so that
    // long-but-routine requests (pasted logs, CSV-style data) don't escalate.
    let long_threshold = if cost_saving { 1_000 } else { 500 };
    if len > long_threshold {
        return AutoModelHeuristicSelection {
            model: "deepseek-v4-pro".to_string(),
            confidence: AutoModelHeuristicConfidence::Decisive,
        };
    }
    // Grey-zone default branch: Flash is the deterministic fallback, but the
    // Flash router can still add value here
    AutoModelHeuristicSelection {
        model: "deepseek-v4-flash".to_string(),
        confidence: AutoModelHeuristicConfidence::Ambiguous,
    }
}

const COMPLEX_KEYWORDS: &[&str] = &[
    // English (unchanged from the original list).
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
    // Simplified Chinese.
    "\u{91cd}\u{6784}", // 重构
    "\u{67b6}\u{6784}", // 架构
    "\u{8bbe}\u{8ba1}", // 设计
    "\u{8c03}\u{8bd5}", // 调试
    "\u{5b89}\u{5168}", // 安全
    "\u{5ba1}\u{67e5}", // 审查
    "\u{5ba1}\u{8ba1}", // 审计
    "\u{8fc1}\u{79fb}", // 迁移
    "\u{4f18}\u{5316}", // 优化
    "\u{91cd}\u{5199}", // 重写
    "\u{5b9e}\u{73b0}", // 实现
    "\u{5206}\u{6790}", // 分析
    // Traditional Chinese variants where they differ.
    "\u{91cd}\u{69cb}", // 重構
    "\u{67b6}\u{69cb}", // 架構
    "\u{8a2d}\u{8a08}", // 設計
    "\u{8abf}\u{8a66}", // 調試
    "\u{5be9}\u{67e5}", // 審查
    "\u{5be9}\u{8a08}", // 審計
    "\u{9077}\u{79fb}", // 遷移
    "\u{512a}\u{5316}", // 優化
    "\u{91cd}\u{5beb}", // 重寫
    "\u{5be6}\u{73fe}", // 實現
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutoRouteRecommendation {
    pub(crate) model: String,
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
}

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
    pub(crate) model: String,
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
    pub(crate) source: AutoRouteSource,
}

const AUTO_MODEL_ROUTER_SYSTEM_PROMPT: &str = "\
You are the codewhale auto-routing classifier. Return only compact JSON: \
{\"model\":\"deepseek-v4-flash|deepseek-v4-pro\",\"thinking\":\"off|high|max\"}. \
Use deepseek-v4-flash for trivial, conversational, status, or single-step work. \
Use deepseek-v4-pro for coding, debugging, release work, multi-step tasks, high-risk decisions, \
tool-heavy work, ambiguous requests, or anything that benefits from deeper reasoning. \
Use thinking off only for trivial no-tool answers, high for ordinary reasoning, and max for \
agentic, coding, multi-file, release, architecture, debugging, security, tool-heavy, or uncertain work.";

/// Addendum appended to the auto-router system prompt when the user has opted in
/// to cost-saving mode. It nudges the LLM toward Flash for faintly-pro-keyword
/// requests that might otherwise look ambiguous but aren't genuinely complex.
const AUTO_MODEL_ROUTER_COST_SAVING_ADDENDUM: &str = "\
\n\nCost-saving mode is ON. Prefer deepseek-v4-flash for any request that is \
not unmistakably agentic, multi-step, architecture/design, security review, \
or involves significant code generation or bug hunting. Do not escalate to \
deepseek-v4-pro just because the user says \"implement\", \"analyze\", or sends \
a very long message — those are weak signals and Flash can handle them. Reserve \
Pro for genuinely complex, multi-file, multi-tool, or high-stakes work.";

pub(crate) fn parse_auto_route_recommendation(raw: &str) -> Option<AutoRouteRecommendation> {
    let value = extract_first_json_object(raw)?;
    let model = value.get("model").and_then(serde_json::Value::as_str)?;
    let model = normalize_auto_route_model(model)?;
    let reasoning_effort = value
        .get("thinking")
        .or_else(|| value.get("reasoning_effort"))
        .or_else(|| value.get("effort"))
        .and_then(serde_json::Value::as_str)
        .and_then(parse_auto_route_reasoning_effort);

    Some(AutoRouteRecommendation {
        model: model.to_string(),
        reasoning_effort,
    })
}

fn extract_first_json_object(s: &str) -> Option<serde_json::Value> {
    let bytes = s.as_bytes();
    let mut depth = 0usize;
    let mut start: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'{' => {
                depth += 1;
                if depth == 1 {
                    start = Some(i);
                }
            }
            b'}' => {
                if depth == 1 {
                    if let Some(start) = start {
                        let json_str = &s[start..=i];
                        return serde_json::from_str(json_str).ok();
                    }
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    None
}

fn normalize_auto_route_model(model: &str) -> Option<&'static str> {
    match model.trim().to_ascii_lowercase().as_str() {
        "deepseek-v4-flash" | "flash" | "v4-flash" => Some("deepseek-v4-flash"),
        "deepseek-v4-pro" | "pro" | "v4-pro" | "deepseek-v4" | "v4" => Some("deepseek-v4-pro"),
        _ => None,
    }
}

fn parse_auto_route_reasoning_effort(effort: &str) -> Option<ReasoningEffort> {
    match effort.trim().to_ascii_lowercase().as_str() {
        "on" | "max" | "high" | "deep" | "3" => Some(ReasoningEffort::High),
        "medium" | "moderate" | "2" => Some(ReasoningEffort::Medium),
        "off" | "low" | "1" | "none" | "minimum" | "0" => Some(ReasoningEffort::Low),
        _ => None,
    }
}

pub(crate) fn normalize_auto_route_effort(effort: ReasoningEffort) -> ReasoningEffort {
    effort
}

fn auto_route_from_heuristic(
    _latest_request: &str,
    heuristic: AutoModelHeuristicSelection,
) -> AutoRouteSelection {
    AutoRouteSelection {
        model: heuristic.model,
        reasoning_effort: None,
        source: AutoRouteSource::Heuristic,
    }
}

async fn auto_route_flash_recommendation(
    config: &Config,
    latest_request: &str,
    recent_context: &str,
    selected_model_mode: &str,
    selected_thinking_mode: &str,
) -> anyhow::Result<Option<AutoRouteRecommendation>> {
    if cfg!(test) {
        return Ok(None);
    }

    let client = DeepSeekClient::new(config)?;
    let mut router_system = AUTO_MODEL_ROUTER_SYSTEM_PROMPT.to_string();
    if config.auto_cost_saving() {
        router_system.push_str(AUTO_MODEL_ROUTER_COST_SAVING_ADDENDUM);
    }
    let request = MessageRequest {
        model: "deepseek-v4-flash".to_string(),
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: auto_route_prompt(
                    latest_request,
                    recent_context,
                    selected_model_mode,
                    selected_thinking_mode,
                ),
                cache_control: None,
            }],
        }],
        max_tokens: 96,
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
    Ok(parse_auto_route_recommendation(&message_response_text(
        &response,
    )))
}

fn auto_route_prompt(
    latest_request: &str,
    recent_context: &str,
    selected_model_mode: &str,
    selected_thinking_mode: &str,
) -> String {
    format!(
        "Session mode: agent\nSelected model mode: {}\nSelected thinking mode: {}\n\nRecent context:\n{}\n\nLatest user request:\n{}\n\nReturn JSON only.",
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
            ContentBlock::Thinking { thinking } => {
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

/// Resolve auto-route — heuristic first, then flash router for ambiguous cases.
pub(crate) async fn resolve_auto_route_with_flash(
    config: &Config,
    latest_request: &str,
    recent_context: &str,
    selected_model_mode: &str,
    selected_thinking_mode: &str,
) -> AutoRouteSelection {
    let cost_saving = config.auto_cost_saving();
    let heuristic =
        auto_model_heuristic_selection_with_bias(latest_request, selected_model_mode, cost_saving);
    if heuristic.confidence == AutoModelHeuristicConfidence::Decisive {
        return auto_route_from_heuristic(latest_request, heuristic);
    }

    match auto_route_flash_recommendation(
        config,
        latest_request,
        recent_context,
        selected_model_mode,
        selected_thinking_mode,
    )
    .await
    {
        Ok(Some(recommendation)) => AutoRouteSelection {
            model: recommendation.model,
            reasoning_effort: recommendation.reasoning_effort,
            source: AutoRouteSource::FlashRouter,
        },
        Ok(None) | Err(_) => auto_route_from_heuristic(latest_request, heuristic),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_model_heuristic_chinese_keywords_route_to_pro() {
        // Without these keywords, a Chinese user typing
        // "帮我重构这个模块" (37 chars in chars().count() terms after
        // the leading helper text) fell through to the short-message
        // Flash branch even though the intent is obviously Pro-tier.
        for msg in [
            "\u{5e2e}\u{6211}\u{91cd}\u{6784}\u{8fd9}\u{4e2a}\u{6a21}\u{5757}", // 帮我重构这个模块
            "\u{8bbe}\u{8ba1}\u{6570}\u{636e}\u{5e93}\u{67b6}\u{6784}",         // 设计数据库架构
            "\u{8c03}\u{8bd5}\u{5d29}\u{6e83}\u{95ee}\u{9898}",                 // 调试崩溃问题
            "\u{5ba1}\u{8ba1}\u{5b89}\u{5168}\u{6f0f}\u{6d1e}",                 // 审计安全漏洞
            "\u{8fc1}\u{79fb}\u{5230}\u{65b0}\u{6846}\u{67b6}",                 // 迁移到新框架
            "\u{4f18}\u{5316}\u{6027}\u{80fd}\u{74f6}\u{9888}",                 // 优化性能瓶颈
            "\u{5206}\u{6790}\u{8fd9}\u{6bb5}\u{4ee3}\u{7801}",                 // 分析这段代码
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
            "\u{8acb}\u{91cd}\u{69cb}\u{6b64}\u{6a21}\u{7d44}", // 請重構此模組
            "\u{67b6}\u{69cb}\u{8a2d}\u{8a08}",                 // 架構設計
            "\u{4ee3}\u{78bc}\u{8abf}\u{8a66}",                 // 代碼調試
            "\u{5be9}\u{8a08}\u{6f0f}\u{6d1e}",                 // 審計漏洞
            "\u{9077}\u{79fb}\u{5230}\u{65b0}\u{67b6}\u{69cb}", // 遷移到新架構
            "\u{512a}\u{5316}\u{6027}\u{80fd}",                 // 優化性能
            "\u{91cd}\u{5beb}\u{4ee3}\u{78bc}",                 // 重寫代碼
            "\u{5be6}\u{73fe}\u{65b0}\u{529f}\u{80fd}",         // 實現新功能
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
        // Sanity: a short non-keyword Chinese message still falls
        // through to the cost-saving Flash branch.
        // "你好" (2 chars) — well under the 100-char Flash floor.
        assert_eq!(
            auto_model_heuristic("\u{4f60}\u{597d}", "auto"),
            "deepseek-v4-flash",
        );
    }

    #[test]
    fn auto_heuristic_selection_marks_short_and_complex_routes_decisive() {
        let short = auto_model_heuristic_selection_with_bias("yes", "auto", false);
        assert_eq!(short.model, "deepseek-v4-flash");
        assert_eq!(
            short.confidence,
            AutoModelHeuristicConfidence::Decisive,
            "trivial replies should skip the Flash router"
        );

        let complex = auto_model_heuristic_selection_with_bias(
            "Please review the auth migration",
            "auto",
            false,
        );
        assert_eq!(complex.model, "deepseek-v4-pro");
        assert_eq!(
            complex.confidence,
            AutoModelHeuristicConfidence::Decisive,
            "strong complexity keywords should skip the Flash router"
        );
    }

    #[test]
    fn auto_heuristic_selection_leaves_default_branch_ambiguous_for_router() {
        let request =
            "Please update the configuration notes so each option has a clearer label. ".repeat(3);
        assert!(
            (100..500).contains(&request.chars().count()),
            "test request must stay in the default grey zone"
        );

        let selection = auto_model_heuristic_selection_with_bias(&request, "auto", false);
        assert_eq!(selection.model, "deepseek-v4-flash");
        assert_eq!(
            selection.confidence,
            AutoModelHeuristicConfidence::Ambiguous,
            "only the grey-zone default branch should invoke the Flash router"
        );
    }

    #[test]
    fn auto_route_recommendation_parses_strict_json() {
        let rec =
            parse_auto_route_recommendation(r#"{"model":"deepseek-v4-pro","thinking":"max"}"#)
                .expect("valid router response should parse");

        assert_eq!(rec.model, "deepseek-v4-pro");
        assert_eq!(rec.reasoning_effort, Some(ReasoningEffort::Max));
    }

    #[test]
    fn auto_route_recommendation_accepts_wrapped_json_aliases() {
        let rec =
            parse_auto_route_recommendation(r#"route: {"model":"flash","reasoning_effort":"off"}"#)
                .expect("wrapped router response should parse");

        assert_eq!(rec.model, "deepseek-v4-flash");
        assert_eq!(rec.reasoning_effort, Some(ReasoningEffort::Off));
    }

    #[test]
    fn auto_route_recommendation_normalizes_legacy_low_medium_to_high() {
        let rec = parse_auto_route_recommendation(
            r#"{"model":"deepseek-v4-pro","reasoning_effort":"medium"}"#,
        )
        .expect("medium should parse for back-compat");

        assert_eq!(rec.model, "deepseek-v4-pro");
        assert_eq!(rec.reasoning_effort, Some(ReasoningEffort::High));
    }

    #[test]
    fn auto_route_recommendation_rejects_unknown_model() {
        assert!(
            parse_auto_route_recommendation(r#"{"model":"some-other-model","thinking":"max"}"#,)
                .is_none()
        );
    }

    #[test]
    fn auto_heuristic_default_routes_implement_to_pro() {
        // Default (no cost-saving): "implement" is one of the borderline
        // keywords that escalates to Pro.
        assert_eq!(
            auto_model_heuristic_with_bias("Please implement a binary search", "auto", false),
            "deepseek-v4-pro"
        );
    }

    #[test]
    fn auto_heuristic_cost_saving_keeps_borderline_keywords_on_flash() {
        // Cost-saving: "implement" / "analyze" are no longer enough to escalate.
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
        // Cost-saving must NOT swallow obviously Pro-grade work.
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
        // 600-char request is "long" by default (>500) → Pro,
        // but stays Flash under cost-saving (threshold 1000).
        let body = "filler sentence. ".repeat(40); // ~680 chars
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
    fn config_auto_cost_saving_defaults_to_false() {
        let cfg = crate::config::Config::default();
        assert!(!cfg.auto_cost_saving());
    }

    #[test]
    fn config_auto_cost_saving_reads_table() {
        let cfg = crate::config::Config {
            auto: Some(crate::config::AutoConfig {
                cost_saving: Some(true),
            }),
            ..Default::default()
        };
        assert!(cfg.auto_cost_saving());
    }
}
