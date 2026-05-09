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

use codewhale_execpolicy::{
    ExecPolicyEngine, PermissionDecision, ToolPermissionCheck, ToolPermissionContext,
};
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};

use crate::models::{Tool, ToolCaller};
use crate::tools::spec::{ToolError, ToolResult};
use crate::tui::app::AppMode;

use super::ToolUseState;
use super::tool_catalog::MULTI_TOOL_PARALLEL_NAME;

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
    pub(super) supports_parallel: bool,
    pub(super) read_only: bool,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ToolPermissionOverride {
    Unmatched,
    Allow { reason: String },
    Ask { reason: String },
    Deny { reason: String },
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

// === Typed permission rules ==========================================

pub(super) fn tool_permission_override_for_call(
    engine: &ExecPolicyEngine,
    tool_name: &str,
    input: &Value,
    workspace: &Path,
) -> ToolPermissionOverride {
    if tool_name == MULTI_TOOL_PARALLEL_NAME {
        return permission_override_for_parallel_call(engine, input, workspace);
    }

    if is_shell_permission_tool(tool_name) {
        let command = input.get("command").and_then(Value::as_str);
        return permission_override_for_context(engine, tool_name, command, None);
    }

    if !is_file_permission_tool(tool_name) {
        return ToolPermissionOverride::Unmatched;
    }

    let paths = permission_paths_for_file_tool(tool_name, input);
    if paths.is_empty() {
        return permission_override_for_context(engine, tool_name, None, None);
    }

    let mut saw_allow = false;
    let mut saw_unmatched = false;
    let mut ask_reason: Option<String> = None;

    for path in paths {
        match permission_override_for_path(engine, tool_name, &path, workspace) {
            ToolPermissionOverride::Deny { reason } => {
                return ToolPermissionOverride::Deny { reason };
            }
            ToolPermissionOverride::Ask { reason } => {
                ask_reason.get_or_insert(reason);
            }
            ToolPermissionOverride::Allow { .. } => {
                saw_allow = true;
            }
            ToolPermissionOverride::Unmatched => {
                saw_unmatched = true;
            }
        }
    }

    if let Some(reason) = ask_reason {
        ToolPermissionOverride::Ask { reason }
    } else if saw_allow && !saw_unmatched {
        ToolPermissionOverride::Allow {
            reason: format!("Tool '{tool_name}' allowed by permission rule"),
        }
    } else {
        ToolPermissionOverride::Unmatched
    }
}

fn permission_override_for_parallel_call(
    engine: &ExecPolicyEngine,
    input: &Value,
    workspace: &Path,
) -> ToolPermissionOverride {
    let Ok(calls) = parse_parallel_tool_calls(input) else {
        return ToolPermissionOverride::Unmatched;
    };

    let mut saw_allow = false;
    let mut saw_unmatched = false;
    let mut ask_reason: Option<String> = None;

    for (nested_tool_name, nested_input) in calls {
        match tool_permission_override_for_call(engine, &nested_tool_name, &nested_input, workspace)
        {
            ToolPermissionOverride::Deny { reason } => {
                return ToolPermissionOverride::Deny { reason };
            }
            ToolPermissionOverride::Ask { reason } => {
                ask_reason.get_or_insert(reason);
            }
            ToolPermissionOverride::Allow { .. } => {
                saw_allow = true;
            }
            ToolPermissionOverride::Unmatched => {
                saw_unmatched = true;
            }
        }
    }

    if let Some(reason) = ask_reason {
        ToolPermissionOverride::Ask { reason }
    } else if saw_allow && !saw_unmatched {
        ToolPermissionOverride::Allow {
            reason: format!("Tool '{MULTI_TOOL_PARALLEL_NAME}' allowed by permission rule"),
        }
    } else {
        ToolPermissionOverride::Unmatched
    }
}

fn permission_override_for_path(
    engine: &ExecPolicyEngine,
    tool_name: &str,
    path: &str,
    workspace: &Path,
) -> ToolPermissionOverride {
    let mut override_result = ToolPermissionOverride::Unmatched;
    for candidate in permission_path_candidates(path, workspace) {
        let next = permission_override_for_context(engine, tool_name, None, Some(&candidate));
        override_result = combine_permission_overrides(override_result, next);
        if matches!(override_result, ToolPermissionOverride::Deny { .. }) {
            break;
        }
    }
    override_result
}

fn permission_override_for_context(
    engine: &ExecPolicyEngine,
    tool_name: &str,
    command: Option<&str>,
    path: Option<&str>,
) -> ToolPermissionOverride {
    let mut override_result = ToolPermissionOverride::Unmatched;
    for policy_tool_name in
        std::iter::once(tool_name).chain(permission_policy_tool_aliases(tool_name).iter().copied())
    {
        let next = permission_override_from_check(
            tool_name,
            engine.check_tool_permission(ToolPermissionContext {
                tool: policy_tool_name,
                command,
                path,
                workspace_root: None,
            }),
        );
        override_result = combine_permission_overrides(override_result, next);
        if matches!(override_result, ToolPermissionOverride::Deny { .. }) {
            break;
        }
    }
    override_result
}

fn combine_permission_overrides(
    current: ToolPermissionOverride,
    next: ToolPermissionOverride,
) -> ToolPermissionOverride {
    match (current, next) {
        (ToolPermissionOverride::Deny { reason }, _) => ToolPermissionOverride::Deny { reason },
        (_, ToolPermissionOverride::Deny { reason }) => ToolPermissionOverride::Deny { reason },
        (ToolPermissionOverride::Ask { reason }, _) => ToolPermissionOverride::Ask { reason },
        (_, ToolPermissionOverride::Ask { reason }) => ToolPermissionOverride::Ask { reason },
        (ToolPermissionOverride::Allow { reason }, _) => ToolPermissionOverride::Allow { reason },
        (_, ToolPermissionOverride::Allow { reason }) => ToolPermissionOverride::Allow { reason },
        (ToolPermissionOverride::Unmatched, ToolPermissionOverride::Unmatched) => {
            ToolPermissionOverride::Unmatched
        }
    }
}

fn permission_override_from_check(
    tool_name: &str,
    check: ToolPermissionCheck,
) -> ToolPermissionOverride {
    let Some(decision) = check.decision else {
        return ToolPermissionOverride::Unmatched;
    };
    let label = check
        .matched_rule
        .as_ref()
        .map(|matched| matched.rule.pattern_label())
        .unwrap_or_else(|| format!("tool '{tool_name}'"));
    match decision {
        PermissionDecision::Allow => ToolPermissionOverride::Allow {
            reason: format!("Tool '{tool_name}' allowed by permission rule ({label})"),
        },
        PermissionDecision::Ask => ToolPermissionOverride::Ask {
            reason: format!("Approval required by permission rule ({label})"),
        },
        PermissionDecision::Deny => ToolPermissionOverride::Deny {
            reason: format!("Tool '{tool_name}' denied by permission rule ({label})"),
        },
    }
}

fn is_shell_permission_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "exec_shell"
            | "exec_shell_wait"
            | "exec_shell_interact"
            | "exec_shell_cancel"
            | "exec_wait"
            | "exec_interact"
    )
}

fn is_file_permission_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "read_file" | "write_file" | "edit_file" | "list_dir" | "apply_patch"
    )
}

fn permission_policy_tool_aliases(tool_name: &str) -> &'static [&'static str] {
    match tool_name {
        "exec_shell_wait"
        | "exec_shell_interact"
        | "exec_shell_cancel"
        | "exec_wait"
        | "exec_interact" => &["exec_shell"],
        "read_file" => &["file_read"],
        "write_file" => &["file_write"],
        "edit_file" => &["file_edit"],
        _ => &[],
    }
}

fn permission_paths_for_file_tool(tool_name: &str, input: &Value) -> Vec<String> {
    match tool_name {
        "read_file" | "write_file" | "edit_file" => input
            .get("path")
            .and_then(Value::as_str)
            .map(|path| vec![path.to_string()])
            .unwrap_or_default(),
        "list_dir" => vec![
            input
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or(".")
                .to_string(),
        ],
        "apply_patch" => apply_patch_permission_paths(input),
        _ => Vec::new(),
    }
}

fn apply_patch_permission_paths(input: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(path) = input.get("path").and_then(Value::as_str) {
        push_unique_path(&mut paths, path);
    }
    for key in ["changes", "files"] {
        if let Some(entries) = input.get(key).and_then(Value::as_array) {
            for entry in entries {
                if let Some(path) = entry.get("path").and_then(Value::as_str) {
                    push_unique_path(&mut paths, path);
                }
            }
        }
    }
    match input.get("patch").and_then(Value::as_str) {
        Some(patch) if paths.is_empty() => {
            for path in parse_unified_diff_permission_paths(patch) {
                push_unique_path(&mut paths, &path);
            }
        }
        _ => {}
    }
    paths
}

fn parse_unified_diff_permission_paths(patch: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut old_path: Option<String> = None;

    for line in patch.lines() {
        if let Some(stripped) = line.strip_prefix("--- ") {
            old_path = normalize_diff_permission_path(stripped);
            continue;
        }
        if let Some(stripped) = line.strip_prefix("+++ ") {
            let new_path = normalize_diff_permission_path(stripped);
            if let Some(path) = new_path.or_else(|| old_path.clone()) {
                push_unique_path(&mut paths, &path);
            }
            old_path = None;
        }
    }

    paths
}

fn normalize_diff_permission_path(raw: &str) -> Option<String> {
    let raw = diff_permission_path_token(raw)?;
    if raw.is_empty() || raw == "/dev/null" || raw == "dev/null" {
        return None;
    }
    let raw = raw
        .strip_prefix("a/")
        .or_else(|| raw.strip_prefix("b/"))
        .unwrap_or(&raw);
    Some(raw.to_string())
}

fn diff_permission_path_token(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with('"') {
        return parse_quoted_diff_path(raw);
    }
    raw.split('\t')
        .next()
        .and_then(|path| path.split_whitespace().next())
        .map(ToString::to_string)
}

fn parse_quoted_diff_path(raw: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut chars = raw.strip_prefix('"')?.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(String::from_utf8_lossy(&bytes).to_string()),
            '\\' => {
                let escaped = chars.next()?;
                match escaped {
                    '0'..='7' => {
                        let mut value = escaped.to_digit(8).unwrap_or(0);
                        for _ in 0..2 {
                            let Some(next) = chars.peek().copied() else {
                                break;
                            };
                            let Some(digit) = next.to_digit(8) else {
                                break;
                            };
                            value = value * 8 + digit;
                            chars.next();
                        }
                        bytes.push(value.min(u8::MAX as u32) as u8);
                    }
                    'a' => bytes.push(0x07),
                    'b' => bytes.push(0x08),
                    'f' => bytes.push(0x0c),
                    'n' => bytes.push(b'\n'),
                    'r' => bytes.push(b'\r'),
                    't' => bytes.push(b'\t'),
                    'v' => bytes.push(0x0b),
                    other => {
                        let mut buf = [0; 4];
                        bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
                    }
                }
            }
            other => {
                let mut buf = [0; 4];
                bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
        }
    }

    None
}

fn push_unique_path(paths: &mut Vec<String>, path: &str) {
    if !paths.iter().any(|existing| existing == path) {
        paths.push(path.to_string());
    }
}

fn permission_path_candidates(path: &str, workspace: &Path) -> Vec<String> {
    let mut candidates = Vec::new();
    push_unique_path(&mut candidates, path);

    let workspace_bases = permission_workspace_bases(workspace);
    let workspace_base = workspace_bases
        .first()
        .cloned()
        .unwrap_or_else(|| normalize_permission_path(workspace));
    let raw_path = Path::new(path);
    let candidate_path = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        workspace_base.join(raw_path)
    };
    let candidate_path = normalize_permission_path(&candidate_path);

    for workspace_base in &workspace_bases {
        if let Some(relative) = workspace_relative_permission_path(&candidate_path, workspace_base)
        {
            push_unique_path(&mut candidates, &relative);
        }
    }

    if let Some(canonical_path) = canonicalize_permission_path(&candidate_path) {
        for workspace_base in &workspace_bases {
            if let Some(relative) =
                workspace_relative_permission_path(&canonical_path, workspace_base)
            {
                push_unique_path(&mut candidates, &relative);
            }
        }
    }

    candidates
}

fn permission_workspace_bases(workspace: &Path) -> Vec<PathBuf> {
    let mut bases = Vec::new();
    let workspace = if workspace.is_absolute() {
        workspace.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current_dir| current_dir.join(workspace))
            .unwrap_or_else(|_| workspace.to_path_buf())
    };
    push_unique_path_buf(&mut bases, normalize_permission_path(&workspace));
    if let Ok(canonical) = workspace.canonicalize() {
        push_unique_path_buf(&mut bases, normalize_permission_path(&canonical));
    }
    bases
}

fn workspace_relative_permission_path(path: &Path, workspace: &Path) -> Option<String> {
    let normalized_path = normalize_permission_path(path);
    normalized_path
        .strip_prefix(workspace)
        .ok()
        .map(permission_path_to_string)
}

fn canonicalize_permission_path(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return path
            .canonicalize()
            .ok()
            .map(|path| normalize_permission_path(&path));
    }

    let mut existing_ancestor = path.to_path_buf();
    let mut suffix_parts: Vec<std::ffi::OsString> = Vec::new();

    while !existing_ancestor.exists() {
        if let Some(file_name) = existing_ancestor.file_name() {
            suffix_parts.push(file_name.to_owned());
        }
        match existing_ancestor.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => {
                existing_ancestor = parent.to_path_buf();
            }
            _ => return None,
        }
    }

    let mut canonical = existing_ancestor
        .canonicalize()
        .unwrap_or(existing_ancestor);
    for part in suffix_parts.into_iter().rev() {
        canonical.push(part);
    }
    Some(normalize_permission_path(&canonical))
}

fn normalize_permission_path(path: &Path) -> PathBuf {
    let mut prefix: Option<std::ffi::OsString> = None;
    let mut is_root = false;
    let mut stack: Vec<std::ffi::OsString> = Vec::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix_component) => {
                prefix = Some(prefix_component.as_os_str().to_owned());
            }
            Component::RootDir => {
                is_root = true;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                let parent = Component::ParentDir.as_os_str();
                if let Some(last) = stack.pop() {
                    if last == parent {
                        stack.push(last);
                        stack.push(parent.to_owned());
                    }
                } else if !is_root {
                    stack.push(parent.to_owned());
                }
            }
            Component::Normal(part) => {
                stack.push(part.to_owned());
            }
        }
    }

    let mut normalized = PathBuf::new();
    if let Some(prefix) = prefix {
        normalized.push(prefix);
    }
    if is_root {
        normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR));
    }
    for part in stack {
        normalized.push(part);
    }
    normalized
}

fn permission_path_to_string(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        ".".to_string()
    } else {
        path.to_string_lossy().replace('\\', "/")
    }
}

fn push_unique_path_buf(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

pub(super) fn format_tool_error(err: &ToolError, tool_name: &str) -> String {
    match err {
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
            if lower.contains("current tool catalog") || lower.contains("did you mean:") {
                message.clone()
            } else {
                format!(
                    "Tool '{tool_name}' is not available: {message}. Check mode, feature flags, or tool name."
                )
            }
        }
        ToolError::PermissionDenied { message } => format!(
            "Tool '{tool_name}' was denied: {message}. Adjust approval mode or request permission."
        ),
    }
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

fn strip_code_fences(text: &str) -> Option<String> {
    if !text.contains("```") {
        return None;
    }
    let mut lines = Vec::new();
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
    !plans.is_empty() && plans.iter().all(tool_plan_is_parallel_safe)
}

pub(super) fn tool_plan_is_parallel_safe(plan: &ToolExecutionPlan) -> bool {
    plan.read_only && plan.supports_parallel && !plan.approval_required && !plan.interactive
}

pub(super) fn plan_tool_execution_batches(
    plans: Vec<ToolExecutionPlan>,
) -> Vec<ToolExecutionBatch> {
    let mut batches = Vec::new();
    let mut parallel_chunk = Vec::new();

    for plan in plans {
        if tool_plan_is_parallel_safe(&plan) {
            parallel_chunk.push(plan);
            continue;
        }

        if !parallel_chunk.is_empty() {
            batches.push(ToolExecutionBatch::Parallel(std::mem::take(
                &mut parallel_chunk,
            )));
        }
        batches.push(ToolExecutionBatch::Serial(Box::new(plan)));
    }

    if !parallel_chunk.is_empty() {
        batches.push(ToolExecutionBatch::Parallel(parallel_chunk));
    }

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
        "give me a plan",
        "make a plan",
        "outline a plan",
        "draft a plan",
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
        "look through",
        "understand the current",
        "ground it in the codebase",
        "based on the codebase",
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
