//! Tool dispatch — plan/execute helpers for the per-turn tool batch.
//!
//! Extracted from `core/engine.rs` (P1.3). The high-level ordering still
//! lives in `Engine::handle_deepseek_turn`; this module owns:
//!
//! * Streaming-buffer parsing into a finalized `serde_json::Value` tool input
//!   (`final_tool_input`, `parse_tool_input`, fenced/JSON segment helpers).
//! * The `multi_tool_use.parallel` payload parser.
//! * Policy predicates the turn loop consults — when a batch can run in
//!   parallel, when an `update_plan` step should stop the turn, when a Plan
//!   prompt should force a plan-first hop, and the small set of read-only
//!   MCP tools that are safe to run in parallel.
//! * The tool execution plan/outcome types the batch driver passes around.
//!
//! All items are `pub(super)`-only: the public engine surface (Op/Event,
//! `EngineHandle`, `spawn_engine`) stays in `core/engine.rs`.

use serde_json::json;

use crate::models::{Tool, ToolCaller};
use crate::tools::spec::{ResourceClaim, ToolError, ToolResult, schedule_non_conflicting};
use crate::tui::app::AppMode;

use super::ToolUseState;

// === Types ============================================================

#[allow(dead_code)] // `index` mirrors batch order for diagnostic ergonomics.
pub(super) struct ToolExecOutcome {
    pub(super) index: usize,
    pub(super) id: String,
    pub(super) name: String,
    pub(super) input: serde_json::Value,
    pub(super) started_at: std::time::Instant,
    pub(super) result: Result<ToolResult, ToolError>,
}

#[derive(Debug, Clone)]
pub(super) struct ToolExecutionPlan {
    pub(super) index: usize,
    pub(super) id: String,
    pub(super) name: String,
    pub(super) input: serde_json::Value,
    pub(super) caller: Option<ToolCaller>,
    pub(super) interactive: bool,
    pub(super) approval_required: bool,
    pub(super) approval_description: String,
    pub(super) approval_force_prompt: bool,
    pub(super) supports_parallel: bool,
    pub(super) read_only: bool,
    pub(super) detached_start: bool,
    pub(super) resources: Vec<ResourceClaim>,
    pub(super) blocked_error: Option<ToolError>,
    pub(super) guard_result: Option<ToolResult>,
}

pub(super) enum ToolExecutionBatch {
    Parallel(Vec<ToolExecutionPlan>),
    Serial(Box<ToolExecutionPlan>),
}

#[derive(Debug, serde::Serialize)]
pub(super) struct ParallelToolResultEntry {
    pub(super) tool_name: String,
    pub(super) success: bool,
    pub(super) content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct ParallelToolResult {
    pub(super) results: Vec<ParallelToolResultEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolApprovalStamp {
    ApprovedByUser,
    ApprovedWithPolicy,
}

impl ToolApprovalStamp {
    fn decision(self) -> &'static str {
        match self {
            Self::ApprovedByUser => "approved_by_user",
            Self::ApprovedWithPolicy => "approved_with_policy",
        }
    }

    fn model_visible_note(self) -> &'static str {
        match self {
            Self::ApprovedByUser => {
                "[approval] This tool call required approval and was approved by the user before execution."
            }
            Self::ApprovedWithPolicy => {
                "[approval] This tool call required approval and was approved by the user with an adjusted execution policy before execution."
            }
        }
    }
}

pub(super) fn stamp_tool_result_approval(result: &mut ToolResult, approval: ToolApprovalStamp) {
    let approval_metadata = json!({
        "required": true,
        "decision": approval.decision(),
        "model_visible": true,
    });
    let metadata = result.metadata.get_or_insert_with(|| json!({}));
    if let Some(object) = metadata.as_object_mut() {
        object.insert("approval".to_string(), approval_metadata);
    } else {
        let prior = std::mem::replace(metadata, json!({}));
        if let Some(object) = metadata.as_object_mut() {
            object.insert("_prior".to_string(), prior);
            object.insert("approval".to_string(), approval_metadata);
        }
    }

    let note = approval.model_visible_note();
    if result.content.starts_with("[approval] ") {
        return;
    }
    if result.content.is_empty() {
        result.content = note.to_string();
    } else {
        result.content = format!("{note}\n\n{}", result.content);
    }
}

// Hold the lock guard for the duration of a tool execution.
// The inner guards are held for RAII purposes (dropped when the guard is dropped).
pub(super) enum ToolExecGuard<'a> {
    Read(#[allow(dead_code)] tokio::sync::RwLockReadGuard<'a, ()>),
    Write(#[allow(dead_code)] tokio::sync::RwLockWriteGuard<'a, ()>),
}

// === Caller policy and errors ========================================

pub(super) fn caller_type_for_tool_use(caller: Option<&ToolCaller>) -> &str {
    caller.map_or("direct", |c| c.caller_type.as_str())
}

pub(super) fn caller_allowed_for_tool(
    caller: Option<&ToolCaller>,
    tool_def: Option<&Tool>,
) -> bool {
    let requested = caller_type_for_tool_use(caller);
    if let Some(def) = tool_def
        && let Some(allowed) = &def.allowed_callers
    {
        if allowed.is_empty() {
            return requested == "direct";
        }
        return allowed.iter().any(|item| item == requested);
    }
    requested == "direct"
}

/// Whole-word check for "mode"/"modes" — a plain `contains("mode")` also
/// matched "model", letting provider model errors skip the actionable-hint
/// suffix (#3020).
fn mentions_mode_word(lower: &str) -> bool {
    lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|word| word == "mode" || word == "modes")
}

#[cfg(test)]
pub(super) fn format_tool_error(err: &ToolError, tool_name: &str) -> String {
    format_tool_error_with_schema(err, tool_name, None)
}

pub(super) fn format_tool_error_with_schema(
    err: &ToolError,
    tool_name: &str,
    input_schema: Option<&serde_json::Value>,
) -> String {
    let message = match err {
        ToolError::InvalidInput { message } => {
            format!("Invalid input for tool '{tool_name}': {message}")
        }
        ToolError::MissingField { field } => {
            format!("Tool '{tool_name}' is missing required field '{field}'")
        }
        ToolError::PathEscape { path } => format!(
            "Path escapes workspace: {}. Use a workspace-relative path or enable trust mode.",
            path.display()
        ),
        ToolError::ExecutionFailed { message } => message.clone(),
        ToolError::Timeout { seconds } => format!(
            "Tool '{tool_name}' timed out after {seconds}s. Try a narrower scope or a longer timeout."
        ),
        ToolError::NotAvailable { message } => {
            let lower = message.to_ascii_lowercase();
            // #3020: Pass through self-explanatory messages that already name the
            // cause (mode switch, allow_shell, feature flag).  Avoids appending a
            // conflicting "Check mode, feature flags" suffix on top of
            // "switch to Act mode" which already gives the recovery path.
            if lower.contains("current tool catalog")
                || lower.contains("did you mean:")
                || mentions_mode_word(&lower)
                || lower.contains("allow_shell")
                || lower.contains("feature flag")
            {
                message.clone()
            } else {
                format!(
                    "Tool '{tool_name}' is not available: {message}. Check mode, feature flags, or tool name."
                )
            }
        }
        ToolError::PermissionDenied { message } => {
            let lower = message.to_ascii_lowercase();
            // #3020: Pass through messages that already name the denial cause.
            if mentions_mode_word(&lower)
                || lower.contains("allow_shell")
                || lower.contains("denied by user")
            {
                message.clone()
            } else {
                format!(
                    "Tool '{tool_name}' was denied: {message}. Adjust approval mode or request permission."
                )
            }
        }
    };

    let message = with_transient_tool_fallback_hint(message, err, tool_name);
    let (category, bad_field) = match err {
        ToolError::InvalidInput { .. } => ("invalid_input", None),
        ToolError::MissingField { field } => ("missing_field", Some(field.as_str())),
        ToolError::PathEscape { .. } => ("path_escape", Some("path")),
        ToolError::NotAvailable { .. } => ("tool_not_available", Some("tool_name")),
        _ => return message,
    };
    let valid_shape = input_schema.cloned().unwrap_or_else(|| {
        serde_json::json!({
            "type": "object",
            "guidance": format!("Use the advertised input schema for '{tool_name}'")
        })
    });
    let feedback = serde_json::json!({
        "category": category,
        "bad_field": bad_field,
        "valid_shape": valid_shape,
        "retryable": true,
        "side_effect_status": "not_started"
    });
    format!("{message}\nTool validation feedback: {feedback}")
}

fn with_transient_tool_fallback_hint(message: String, err: &ToolError, tool_name: &str) -> String {
    if message_already_has_recovery_hint(&message) {
        return message;
    }

    let Some(hint) = transient_tool_fallback_hint(err, tool_name, &message) else {
        return message;
    };

    format!("{message} Fallback: {hint}")
}

fn message_already_has_recovery_hint(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("recovery:") || lower.contains("fallback:")
}

fn transient_tool_fallback_hint(
    err: &ToolError,
    tool_name: &str,
    formatted_message: &str,
) -> Option<&'static str> {
    if !is_transient_tool_failure(err, formatted_message) {
        return None;
    }

    let lower_tool = tool_name.to_ascii_lowercase();
    if lower_tool.contains("web_search")
        || lower_tool.contains("web_run")
        || lower_tool == "web.run"
    {
        return Some(
            "after one retry, switch to a direct URL/open/fetch path or cached context instead of repeating the same search.",
        );
    }

    if lower_tool.contains("fetch_url") {
        return Some(
            "after one retry, try a narrower URL/source, use search results or cached context, or state the access limit instead of repeating the same request.",
        );
    }

    if lower_tool.contains("file_search") || lower_tool.contains("grep") {
        return Some(
            "after one retry, narrow the query/path or inspect likely files directly instead of repeating the same search unchanged.",
        );
    }

    if lower_tool.contains("exec_shell")
        || lower_tool.contains("run_tests")
        || lower_tool.contains("run_verifiers")
    {
        return Some(
            "after one retry, narrow the command/scope, increase timeout only for expected long runs, or switch to file-level evidence.",
        );
    }

    if lower_tool.contains("agent") {
        return Some(
            "after one retry, reduce delegated scope or continue in the parent context instead of repeatedly spawning the same agent.",
        );
    }

    Some(
        "after one retry, choose a different tool or narrower strategy instead of repeating the same call unchanged.",
    )
}

fn is_transient_tool_failure(err: &ToolError, formatted_message: &str) -> bool {
    if matches!(err, ToolError::Timeout { .. }) {
        return true;
    }

    if !matches!(err, ToolError::ExecutionFailed { .. }) {
        return false;
    }

    let lower = formatted_message.to_ascii_lowercase();
    [
        "timeout",
        "timed out",
        "request failed",
        "connection",
        "network",
        "http 429",
        "rate limit",
        "http 5",
        "anti-bot",
        "captcha",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

// === Streaming-buffer parsing =========================================

/// Promote a streaming `ToolUseState` to a finalized JSON input.
///
/// Order of preference:
///
///   1. `input_buffer` (the raw streamed delta concatenation) — parsed as
///      JSON. This is the most authoritative because it's what the model
///      actually emitted.
///   2. `input` (the per-delta best-effort parse mirror) — used when the
///      buffer is empty (pre-streaming tool calls take this path).
///   3. `input_buffer` non-empty but unparseable → fall back to `input`
///      (the per-delta parser has already mirrored the most recent valid
///      partial parse into `tool_state.input`).
pub(super) fn final_tool_input(state: &ToolUseState) -> serde_json::Value {
    if state.input_parse_error.is_some() {
        return malformed_tool_arguments_input(&state.input_buffer);
    }
    if !state.input_buffer.trim().is_empty()
        && let Some(parsed) = parse_tool_input(&state.input_buffer)
    {
        return parsed;
    }
    state.input.clone()
}

pub(super) fn parse_tool_input(buffer: &str) -> Option<serde_json::Value> {
    let trimmed = buffer.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Try the deterministic arg-repair ladder first (handles trailing commas,
    // unclosed braces, embedded control chars, etc.)
    if let Ok(value) = crate::tools::arg_repair::repair(trimmed) {
        return Some(value);
    }
    // Fall back to existing strategies for code-fenced, double-encoded, and
    // segment-extraction patterns that the repair ladder doesn't cover.
    if let Some(stripped) = strip_code_fences(trimmed)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&stripped)
    {
        return Some(value);
    }
    if let Ok(serde_json::Value::String(inner)) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&inner)
    {
        return Some(value);
    }
    extract_json_segment(trimmed)
        .and_then(|segment| serde_json::from_str::<serde_json::Value>(&segment).ok())
}

pub(super) fn malformed_tool_arguments_input(buffer: &str) -> serde_json::Value {
    json!({ "raw_arguments": buffer })
}

pub(super) fn malformed_tool_arguments_error(buffer: &str) -> String {
    format!("malformed tool arguments from model: expected valid JSON, got {buffer:?}")
}

fn strip_code_fences(text: &str) -> Option<String> {
    if !text.contains("```") {
        return None;
    }
    let line_count = text.lines().count();
    let mut lines = Vec::with_capacity(line_count);
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            continue;
        }
        lines.push(line);
    }
    let stripped = lines.join("\n");
    let stripped = stripped.trim();
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

fn extract_json_segment(text: &str) -> Option<String> {
    extract_balanced_segment(text, '{', '}').or_else(|| extract_balanced_segment(text, '[', ']'))
}

fn extract_balanced_segment(text: &str, open: char, close: char) -> Option<String> {
    let start = text.find(open)?;
    let mut depth = 0i32;
    let mut end = None;
    for (offset, ch) in text[start..].char_indices() {
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                end = Some(start + offset + ch.len_utf8());
                break;
            }
        }
    }
    end.map(|end_idx| text[start..end_idx].to_string())
}

fn normalize_parallel_tool_name(raw: &str) -> String {
    let mut name = raw.trim();
    for prefix in ["functions.", "tools.", "tool."] {
        if let Some(stripped) = name.strip_prefix(prefix) {
            name = stripped;
            break;
        }
    }
    name.to_string()
}

pub(super) fn parse_parallel_tool_calls(
    input: &serde_json::Value,
) -> Result<Vec<(String, serde_json::Value)>, ToolError> {
    let tool_uses = input
        .get("tool_uses")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ToolError::missing_field("tool_uses"))?;
    if tool_uses.is_empty() {
        return Err(ToolError::invalid_input(
            "multi_tool_use.parallel requires at least one tool call",
        ));
    }

    let mut calls = Vec::with_capacity(tool_uses.len());
    for item in tool_uses {
        let name = item
            .get("recipient_name")
            .or_else(|| item.get("tool_name"))
            .or_else(|| item.get("name"))
            .or_else(|| item.get("tool"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::missing_field("recipient_name"))?;
        let params = item
            .get("parameters")
            .or_else(|| item.get("input"))
            .or_else(|| item.get("args"))
            .or_else(|| item.get("arguments"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        calls.push((normalize_parallel_tool_name(name), params));
    }

    Ok(calls)
}

// === Dispatch policy ==================================================

#[cfg(test)]
pub(super) fn should_parallelize_tool_batch(plans: &[ToolExecutionPlan]) -> bool {
    if plans.is_empty() || !plans.iter().all(tool_plan_can_join_parallel_batch) {
        return false;
    }
    schedule_non_conflicting(
        plans
            .iter()
            .map(|plan| ((), plan.resources.clone()))
            .collect(),
    )
    .len()
        == 1
}

pub(super) fn tool_plan_is_parallel_safe(plan: &ToolExecutionPlan) -> bool {
    plan.read_only && plan.supports_parallel && !plan.approval_required && !plan.interactive
}

pub(super) fn tool_plan_can_join_parallel_batch(plan: &ToolExecutionPlan) -> bool {
    plan.blocked_error.is_none()
        && (tool_plan_is_parallel_safe(plan)
            || (plan.detached_start && !plan.approval_required && !plan.interactive))
}

pub(super) fn plan_tool_execution_batches(
    plans: Vec<ToolExecutionPlan>,
) -> Vec<ToolExecutionBatch> {
    let mut batches = Vec::new();
    let mut parallel_candidates = Vec::new();

    let flush_parallel = |parallel_candidates: &mut Vec<_>,
                          batches: &mut Vec<ToolExecutionBatch>| {
        for chunk in schedule_non_conflicting(std::mem::take(parallel_candidates)) {
            batches.push(ToolExecutionBatch::Parallel(chunk));
        }
    };

    for plan in plans {
        if tool_plan_can_join_parallel_batch(&plan) {
            let resources = plan.resources.clone();
            parallel_candidates.push((plan, resources));
            continue;
        }

        flush_parallel(&mut parallel_candidates, &mut batches);
        batches.push(ToolExecutionBatch::Serial(Box::new(plan)));
    }

    flush_parallel(&mut parallel_candidates, &mut batches);

    batches
}

pub(super) fn should_stop_after_plan_tool(
    mode: AppMode,
    tool_name: &str,
    result: &Result<ToolResult, ToolError>,
) -> bool {
    mode == AppMode::Plan && tool_name == "update_plan" && result.is_ok()
}

pub(super) fn should_force_update_plan_first(mode: AppMode, content: &str) -> bool {
    if mode != AppMode::Plan {
        return false;
    }

    let lower = content.to_ascii_lowercase();
    // Only shortcut genuinely lightweight plan asks. Bare "make a plan" wording
    // is often used for repo/version/build work where Plan mode still needs to
    // inspect available context before publishing the handoff artifact.
    let asks_for_direct_plan = [
        "quick plan",
        "short plan",
        "simple plan",
        "3-step plan",
        "3 step plan",
        "three-step plan",
        "three step plan",
        "high-level plan",
        "high level plan",
    ]
    .iter()
    .any(|needle| lower.contains(needle));

    if !asks_for_direct_plan {
        return false;
    }

    let asks_for_repo_exploration = [
        "inspect the repo",
        "inspect the code",
        "explore the repo",
        "search the repo",
        "read the code",
        "review the code",
        "analyze the code",
        "investigate",
        "figure out",
        "figuring out",
        "look through",
        "understand the current",
        "current state",
        "ground it in the codebase",
        "based on the codebase",
        "repo",
        "codebase",
        "version",
        "ver ",
        "release",
        "build",
        "benchmark",
        "api server",
        "github.com",
        "http://",
        "https://",
    ]
    .iter()
    .any(|needle| lower.contains(needle));

    !asks_for_repo_exploration
}

pub(super) fn mcp_tool_is_parallel_safe(name: &str) -> bool {
    matches!(
        name,
        "list_mcp_resources"
            | "list_mcp_resource_templates"
            | "mcp_read_resource"
            | "read_mcp_resource"
            | "mcp_get_prompt"
    )
}

pub(super) fn mcp_tool_is_read_only(name: &str) -> bool {
    matches!(
        name,
        "list_mcp_resources"
            | "list_mcp_resource_templates"
            | "mcp_read_resource"
            | "read_mcp_resource"
            | "mcp_get_prompt"
    )
}

pub(super) fn mcp_tool_approval_description(name: &str) -> String {
    if mcp_tool_is_read_only(name) {
        format!("Read-only MCP tool '{name}'")
    } else {
        format!("MCP tool '{name}' may have side effects")
    }
}
