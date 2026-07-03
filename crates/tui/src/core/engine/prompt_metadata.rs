//! Prompt-turn metadata assembly.
//!
//! Pure projection from resolved policy + turn facts to model-visible
//! `<turn_meta>` block, mode instructions, and resource lines. This module
//! does **not** mutate or decide mode, approval, trust, shell, sandbox, or
//! tool authority.

use crate::core::ops::UserInputProvenance;
use crate::core::session::SessionUsage;
use crate::models::ContentBlock;
use crate::resource_telemetry::ResourceTelemetry;
use crate::tools::goal::GoalSnapshot;
use crate::tui::app::AppMode;

/// Mode-specific runtime instructions for the model-visible prompt deltas.
///
/// These are the short mode-policy overlays (e.g. `## Mode: Agent`)
/// injected into the `<turn_meta>` block so the model sees the current
/// permissions without mutating the byte-stable system-prompt prefix.
pub fn mode_runtime_instructions(mode: AppMode) -> &'static str {
    match mode {
        AppMode::Agent | AppMode::Auto => crate::prompts::AGENT_MODE,
        AppMode::Plan => crate::prompts::PLAN_MODE,
        AppMode::Yolo => crate::prompts::YOLO_MODE,
    }
    .trim()
}

/// Renders a session token usage summary line for the `<turn_meta>` block.
pub fn session_token_usage_line(usage: &SessionUsage) -> Option<String> {
    let total = usage.input_tokens.saturating_add(usage.output_tokens);
    if total == 0 {
        return None;
    }

    let mut line = format!(
        "Session token usage: {total} total ({} input, {} output)",
        usage.input_tokens, usage.output_tokens,
    );
    if let Some(hit_tokens) = usage.cache_read_input_tokens {
        line.push_str(&format!(", cache hits {hit_tokens}"));
    }
    if let Some(miss_tokens) = usage.cache_creation_input_tokens {
        line.push_str(&format!(", cache misses {miss_tokens}"));
    }
    Some(line)
}

/// Renders an active-goal resource telemetry line for the `<turn_meta>` block.
pub fn goal_resource_line(snapshot: &GoalSnapshot) -> Option<String> {
    if !snapshot.is_active() {
        return None;
    }

    let mut telemetry = ResourceTelemetry::new(snapshot.tokens_used, snapshot.time_used_seconds);
    if let Some(token_budget) = snapshot.token_budget {
        telemetry = telemetry.with_token_budget(u64::from(token_budget));
    }

    let mut line = format!("Active goal resource usage: {}", telemetry.human_summary());
    if snapshot.tokens_used > 0 && snapshot.time_used_seconds > 0 {
        let rate = snapshot.tokens_used as f64 / snapshot.time_used_seconds as f64;
        line.push_str(&format!("; {rate:.1} tok/s"));
    }
    line.push_str(&format!("; {} continuations", snapshot.continuation_count));
    Some(line)
}

/// Fully assembled turn-metadata context.
///
/// All fields are pure facts resolved *before* this struct is built — no
/// policy decision happens here.
#[derive(Debug, Clone)]
pub struct TurnMetadataInput {
    /// Current operating mode (already resolved).
    pub mode: AppMode,
    /// Who sent this input.
    pub provenance: UserInputProvenance,
    /// Resolved model route id.
    pub routed_model: String,
    /// Whether the model was auto-selected.
    pub auto_model: bool,
    /// Resolved reasoning effort, if any.
    pub reasoning_effort: Option<String>,
    /// Whether reasoning effort was auto-selected.
    pub reasoning_effort_auto: bool,
    /// Current workspace path.
    pub workspace_path: std::path::PathBuf,
    /// Working-set summary block, if any.
    pub working_set_summary: Option<String>,
    /// Pre-computed context pressure line, if available.
    pub context_pressure_line: Option<String>,
    /// Pre-computed session token usage line.
    pub token_usage_line: Option<String>,
    /// Pre-computed active-goal resource line.
    pub goal_resource_line: Option<String>,
}

/// Build the `<turn_meta>` content block from resolved policy and turn facts.
///
/// The block is placed at the tail of each user/runtime message so the
/// leading bytes stay stable for DeepSeek's KV prefix cache. The caller is
/// responsible for computing context pressure, token usage, and goal
/// resource lines before calling this function.
pub fn build_turn_metadata(input: &TurnMetadataInput) -> ContentBlock {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    let mut lines = vec![
        format!("Current local date: {today}"),
        // Workspace path lives here (not in the static `## Environment`
        // block) so the static system-prompt prefix stays byte-stable
        // across sessions.
        format!("Current workspace: {}", input.workspace_path.display()),
        format!("Current model: {}", input.routed_model),
        format!("Current mode: {}", input.mode.as_setting()),
        "Current mode policy source: runtime".to_string(),
        format!(
            "Current mode policy:\n{}",
            mode_runtime_instructions(input.mode)
        ),
        format!("Input provenance: {}", input.provenance.as_str()),
        format!(
            "Input authority: {}",
            if input.provenance.can_authorize_work() {
                "external_current_turn"
            } else {
                "non_authoritative"
            }
        ),
    ];
    if input.auto_model {
        lines.push(format!("Auto model route: {}", input.routed_model));
    }
    if input.reasoning_effort_auto {
        if let Some(ref reasoning_effort) = input.reasoning_effort {
            lines.push(format!("Auto reasoning effort: {reasoning_effort}"));
        }
    }
    if let Some(ref context_pressure_line) = input.context_pressure_line {
        lines.push(context_pressure_line.clone());
    }
    if let Some(ref token_usage_line) = input.token_usage_line {
        lines.push(token_usage_line.clone());
    }
    if let Some(ref goal_resource_line) = input.goal_resource_line {
        lines.push(goal_resource_line.clone());
    }
    if let Some(ref working_set_summary) = input.working_set_summary {
        lines.push(working_set_summary.clone());
    }
    let summary = lines.join("\n");

    ContentBlock::Text {
        text: format!("<turn_meta>\n{summary}\n</turn_meta>"),
        cache_control: None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::tools::goal::{GoalSnapshot, GoalStatus};

    fn text_content(block: ContentBlock) -> String {
        match block {
            ContentBlock::Text { text, .. } => text,
            _ => panic!("expected text content block"),
        }
    }

    fn metadata_input(provenance: UserInputProvenance) -> TurnMetadataInput {
        TurnMetadataInput {
            mode: AppMode::Agent,
            provenance,
            routed_model: "deepseek-reasoner".to_string(),
            auto_model: false,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            workspace_path: PathBuf::from("/tmp/codewhale"),
            working_set_summary: None,
            context_pressure_line: None,
            token_usage_line: None,
            goal_resource_line: None,
        }
    }

    #[test]
    fn generated_turn_metadata_reports_non_authoritative_provenance() {
        let text = text_content(build_turn_metadata(&metadata_input(
            UserInputProvenance::SubAgentHandoff,
        )));

        assert!(text.starts_with("<turn_meta>\n"));
        assert!(text.ends_with("\n</turn_meta>"));
        assert!(text.contains("Current workspace: /tmp/codewhale"));
        assert!(text.contains("Current model: deepseek-reasoner"));
        assert!(text.contains("Current mode: agent"));
        assert!(text.contains("Current mode policy source: runtime"));
        assert!(text.contains("Input provenance: subagent_handoff"));
        assert!(text.contains("Input authority: non_authoritative"));
        assert!(text.contains("## Mode: Agent"));
    }

    #[test]
    fn generated_turn_metadata_reports_external_turn_authority_only_as_fact() {
        let text = text_content(build_turn_metadata(&metadata_input(
            UserInputProvenance::ExternalUser,
        )));

        assert!(text.contains("Input provenance: external_user"));
        assert!(text.contains("Input authority: external_current_turn"));
        assert!(!text.contains("subagent_handoff"));
    }

    #[test]
    fn generated_turn_metadata_appends_auto_and_resource_lines() {
        let mut input = metadata_input(UserInputProvenance::ExternalUser);
        input.auto_model = true;
        input.reasoning_effort_auto = true;
        input.reasoning_effort = Some("medium".to_string());
        input.context_pressure_line = Some(
            "Context pressure: comfortable (10.0% used, 100 / 1000 tokens; 900 input tokens available)"
                .to_string(),
        );
        input.token_usage_line =
            Some("Session token usage: 42 total (20 input, 22 output)".to_string());
        input.goal_resource_line =
            Some("Active goal resource usage: 100 tokens; 1 continuations".to_string());
        input.working_set_summary =
            Some("Working set:\n- crates/tui/src/core/engine.rs".to_string());

        let text = text_content(build_turn_metadata(&input));

        assert!(text.contains("Auto model route: deepseek-reasoner"));
        assert!(text.contains("Auto reasoning effort: medium"));
        assert!(text.contains("Context pressure: comfortable"));
        assert!(text.contains("Session token usage: 42 total"));
        assert!(text.contains("Active goal resource usage: 100 tokens"));
        assert!(text.contains("Working set:\n- crates/tui/src/core/engine.rs"));
    }

    #[test]
    fn session_token_usage_line_includes_prompt_cache_fields() {
        let usage = SessionUsage {
            input_tokens: 12,
            output_tokens: 8,
            cache_read_input_tokens: Some(5),
            cache_creation_input_tokens: Some(7),
        };

        assert_eq!(
            session_token_usage_line(&usage).as_deref(),
            Some(
                "Session token usage: 20 total (12 input, 8 output), cache hits 5, cache misses 7"
            )
        );
        assert_eq!(session_token_usage_line(&SessionUsage::default()), None);
    }

    #[test]
    fn goal_resource_line_only_renders_active_goal_facts() {
        let inactive = GoalSnapshot {
            objective: Some("ship it".to_string()),
            status: GoalStatus::Paused.as_str().to_string(),
            token_budget: Some(1_000),
            tokens_used: 500,
            time_used_seconds: 50,
            continuation_count: 2,
            elapsed_seconds: None,
            evidence: None,
            blocker: None,
            completion_verification: None,
        };
        assert_eq!(goal_resource_line(&inactive), None);

        let active = GoalSnapshot {
            status: GoalStatus::Active.as_str().to_string(),
            ..inactive
        };
        let line = goal_resource_line(&active).expect("active goal line");
        assert!(line.contains("Active goal resource usage:"));
        assert!(line.contains("10.0 tok/s"));
        assert!(line.contains("2 continuations"));
    }
}
