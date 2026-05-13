//! Deferred tool catalog and built-in advanced tool helpers.
//!
//! The streaming turn loop owns when tools are offered or executed. This module
//! owns the catalog-level policy around deferred loading, tool search, missing
//! tool suggestions, and the small set of built-in advanced tools that are not
//! registered by the normal runtime tool registry.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::models::Tool;
use crate::tools::spec::{ToolError, ToolResult, required_str};
use crate::tui::app::AppMode;

pub(super) const MULTI_TOOL_PARALLEL_NAME: &str = "multi_tool_use.parallel";
pub(super) const REQUEST_USER_INPUT_NAME: &str = "request_user_input";
pub(super) const CODE_EXECUTION_TOOL_NAME: &str = "code_execution";
const CODE_EXECUTION_TOOL_TYPE: &str = "code_execution_20250825";
pub(super) use crate::tools::js_execution::JS_EXECUTION_TOOL_NAME;
const TOOL_SEARCH_REGEX_NAME: &str = "tool_search_tool_regex";
const TOOL_SEARCH_REGEX_TYPE: &str = "tool_search_tool_regex_20251119";
pub(super) const TOOL_SEARCH_BM25_NAME: &str = "tool_search_tool_bm25";
const TOOL_SEARCH_BM25_TYPE: &str = "tool_search_tool_bm25_20251119";

pub(super) fn is_tool_search_tool(name: &str) -> bool {
    matches!(name, TOOL_SEARCH_REGEX_NAME | TOOL_SEARCH_BM25_NAME)
}

pub(super) fn should_default_defer_tool(name: &str, mode: AppMode) -> bool {
    if mode == AppMode::Yolo {
        return false;
    }

    // Shell exec tools are kept active in Agent so the model can run
    // verification commands (build/test/git/cargo) without first having to
    // discover them through ToolSearch. Plan mode does not register shell
    // execution tools.
    let always_loaded_in_action_modes = matches!(mode, AppMode::Agent)
        && matches!(
            name,
            "exec_shell"
                | "exec_shell_wait"
                | "exec_shell_interact"
                | "exec_wait"
                | "exec_interact"
        );
    if always_loaded_in_action_modes {
        return false;
    }

    !matches!(
        name,
        "read_file"
            | "list_dir"
            | "grep_files"
            | "file_search"
            | "diagnostics"
            | "rlm_open"
            | "rlm_eval"
            | "rlm_configure"
            | "rlm_close"
            | "handle_read"
            | "recall_archive"
            | "notify"
            | MULTI_TOOL_PARALLEL_NAME
            | "update_plan"
            | "checklist_write"
            | "todo_write"
            | "task_create"
            | "task_list"
            | "task_read"
            | "task_gate_run"
            | "task_shell_start"
            | "task_shell_wait"
            | "github_issue_context"
            | "github_pr_context"
            | REQUEST_USER_INPUT_NAME
    )
}

pub(super) fn apply_native_tool_deferral(catalog: &mut [Tool], mode: AppMode) {
    for tool in catalog {
        tool.defer_loading = Some(should_default_defer_tool(&tool.name, mode));
    }
}

fn should_keep_mcp_tool_loaded(name: &str) -> bool {
    matches!(
        name,
        "list_mcp_resources"
            | "list_mcp_resource_templates"
            | "mcp_read_resource"
            | "read_mcp_resource"
            | "mcp_get_prompt"
    )
}

pub(super) fn apply_mcp_tool_deferral(catalog: &mut [Tool], mode: AppMode) {
    for tool in catalog {
        tool.defer_loading =
            Some(mode != AppMode::Yolo && !should_keep_mcp_tool_loaded(&tool.name));
    }
}

pub(super) fn build_model_tool_catalog(
    mut native_tools: Vec<Tool>,
    mut mcp_tools: Vec<Tool>,
    mode: AppMode,
) -> Vec<Tool> {
    apply_native_tool_deferral(&mut native_tools, mode);
    apply_mcp_tool_deferral(&mut mcp_tools, mode);
    // Sort each partition by name for prefix-cache stability (#263). The
    // upstream `to_api_tools()` already sorts the registry's HashMap output;
    // this catalog is built from caller-supplied Vecs which the test harness
    // and (future) caller refactors may not pre-sort. Built-ins stay as a
    // contiguous prefix ahead of MCP tools so adding/removing an MCP tool
    // never shifts a built-in's position.
    native_tools.sort_by(|a, b| a.name.cmp(&b.name));
    mcp_tools.sort_by(|a, b| a.name.cmp(&b.name));
    native_tools.extend(mcp_tools);
    native_tools
}

pub(super) fn ensure_advanced_tooling(catalog: &mut Vec<Tool>, mode: AppMode) {
    // code_execution depends on a locally-installed Python interpreter
    // (python3 / python / py -3). Before v0.8.31, the tool was always
    // advertised and would fail at execution time on Windows where
    // `python3` isn't on PATH — the model treated the tool as reliable
    // once it appeared in the catalog. We now probe at catalog-build
    // time and only advertise when an interpreter resolves. See
    // `crate::dependencies::resolve_python_interpreter` for the probe.
    if mode != AppMode::Plan
        && !catalog.iter().any(|t| t.name == CODE_EXECUTION_TOOL_NAME)
        && crate::dependencies::resolve_python_interpreter().is_some()
    {
        catalog.push(Tool {
            tool_type: Some(CODE_EXECUTION_TOOL_TYPE.to_string()),
            name: CODE_EXECUTION_TOOL_NAME.to_string(),
            description: "Execute Python code in a local sandboxed runtime and return stdout/stderr/return_code as JSON.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "code": { "type": "string", "description": "Python source code to execute." }
                },
                "required": ["code"]
            }),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(false),
            input_examples: None,
            strict: None,
            cache_control: None,
        });
    }

    // js_execution mirrors code_execution: gate on Node.js being
    // present locally so the model never sees a runtime it can't
    // actually use. Plan mode hides shell/exec surfaces (including
    // both interpreter tools) by construction; Agent / YOLO advertise
    // the tool only when `resolve_node()` succeeds.
    if mode != AppMode::Plan
        && !catalog.iter().any(|t| t.name == JS_EXECUTION_TOOL_NAME)
        && crate::dependencies::resolve_node().is_some()
    {
        catalog.push(crate::tools::js_execution::js_execution_tool_definition());
    }

    if !catalog.iter().any(|t| t.name == TOOL_SEARCH_REGEX_NAME) {
        catalog.push(Tool {
            tool_type: Some(TOOL_SEARCH_REGEX_TYPE.to_string()),
            name: TOOL_SEARCH_REGEX_NAME.to_string(),
            description: "Search deferred tool definitions using a regex query and return matching tool references.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Regex pattern to search tool names/descriptions/schema." }
                },
                "required": ["query"]
            }),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(false),
            input_examples: None,
            strict: None,
            cache_control: None,
        });
    }

    if !catalog.iter().any(|t| t.name == TOOL_SEARCH_BM25_NAME) {
        catalog.push(Tool {
            tool_type: Some(TOOL_SEARCH_BM25_TYPE.to_string()),
            name: TOOL_SEARCH_BM25_NAME.to_string(),
            description: "Search deferred tool definitions using natural-language matching and return matching tool references.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural language query for tool discovery." }
                },
                "required": ["query"]
            }),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(false),
            input_examples: None,
            strict: None,
            cache_control: None,
        });
    }
}

pub(super) fn initial_active_tools(catalog: &[Tool]) -> HashSet<String> {
    let mut active = HashSet::new();
    for tool in catalog {
        if !tool.defer_loading.unwrap_or(false) || is_tool_search_tool(&tool.name) {
            active.insert(tool.name.clone());
        }
    }
    if active.is_empty()
        && !catalog.is_empty()
        && let Some(first) = catalog.first()
    {
        active.insert(first.name.clone());
    }
    active
}

fn active_tool_list_from_catalog(catalog: &[Tool], active: &HashSet<String>) -> Vec<Tool> {
    // Two-pass for prefix-cache stability (#263). Always-loaded tools come
    // first in their stable catalog order; tools that started life deferred
    // and were activated mid-conversation by ToolSearch get appended at the
    // tail. Otherwise activating a deferred tool shifts every later tool's
    // byte offset and busts the cached prefix from that point onwards.
    let mut head: Vec<Tool> = Vec::new();
    let mut tail: Vec<Tool> = Vec::new();
    for tool in catalog {
        if !active.contains(&tool.name) {
            continue;
        }
        if tool.defer_loading.unwrap_or(false) {
            tail.push(tool.clone());
        } else {
            head.push(tool.clone());
        }
    }
    head.extend(tail);
    head
}

pub(super) fn active_tools_for_step(
    catalog: &[Tool],
    active: &HashSet<String>,
    force_update_plan: bool,
) -> Vec<Tool> {
    // DeepSeek reasoning models reject explicit named tool_choice forcing here,
    // so for obvious quick-plan asks we narrow the first-step tool surface to
    // update_plan instead.
    if force_update_plan {
        let forced: Vec<_> = catalog
            .iter()
            .filter(|tool| tool.name == "update_plan")
            .cloned()
            .collect();
        if !forced.is_empty() {
            return forced;
        }
    }

    active_tool_list_from_catalog(catalog, active)
}

fn tool_search_haystack(tool: &Tool) -> String {
    format!(
        "{}\n{}\n{}",
        tool.name.to_lowercase(),
        tool.description.to_lowercase(),
        tool.input_schema.to_string().to_lowercase()
    )
}

fn discover_tools_with_regex(catalog: &[Tool], query: &str) -> Result<Vec<String>, ToolError> {
    let regex = regex::Regex::new(query)
        .map_err(|err| ToolError::invalid_input(format!("Invalid regex query: {err}")))?;

    let mut matches = Vec::new();
    for tool in catalog {
        if is_tool_search_tool(&tool.name) {
            continue;
        }
        let hay = tool_search_haystack(tool);
        if regex.is_match(&hay) {
            matches.push(tool.name.clone());
        }
        if matches.len() >= 5 {
            break;
        }
    }
    Ok(matches)
}

fn discover_tools_with_bm25_like(catalog: &[Tool], query: &str) -> Vec<String> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|term| term.trim().to_lowercase())
        .filter(|term| !term.is_empty())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(i64, String)> = Vec::new();
    for tool in catalog {
        if is_tool_search_tool(&tool.name) {
            continue;
        }
        let hay = tool_search_haystack(tool);
        let mut score = 0i64;
        for term in &terms {
            if hay.contains(term) {
                score += 1;
            }
            if tool.name.to_lowercase().contains(term) {
                score += 2;
            }
        }
        if score > 0 {
            scored.push((score, tool.name.clone()));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().take(5).map(|(_, name)| name).collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    if a.is_empty() {
        return b.chars().count();
    }
    if b.is_empty() {
        return a.chars().count();
    }

    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    let mut curr = vec![0usize; b_chars.len() + 1];

    for (i, a_ch) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, b_ch) in b_chars.iter().enumerate() {
            let cost = if a_ch == *b_ch { 0 } else { 1 };
            let delete = prev[j + 1] + 1;
            let insert = curr[j] + 1;
            let substitute = prev[j] + cost;
            curr[j + 1] = delete.min(insert).min(substitute);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_chars.len()]
}

fn suggest_tool_names(catalog: &[Tool], requested: &str, limit: usize) -> Vec<String> {
    let requested = requested.trim().to_ascii_lowercase();
    if requested.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut candidates: Vec<(u8, usize, String)> = Vec::new();
    for tool in catalog {
        let candidate = tool.name.to_ascii_lowercase();
        let prefix_match = candidate.starts_with(&requested) || requested.starts_with(&candidate);
        let contains_match = candidate.contains(&requested) || requested.contains(&candidate);
        let distance = edit_distance(&candidate, &requested);
        let close_typo = distance <= 3;

        if !(prefix_match || contains_match || close_typo) {
            continue;
        }

        let rank = if prefix_match {
            0
        } else if contains_match {
            1
        } else {
            2
        };
        candidates.push((rank, distance, tool.name.clone()));
    }

    candidates.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    candidates.dedup_by(|a, b| a.2 == b.2);
    candidates
        .into_iter()
        .take(limit)
        .map(|(_, _, name)| name)
        .collect()
}

pub(super) fn missing_tool_error_message(tool_name: &str, catalog: &[Tool]) -> String {
    let suggestions = suggest_tool_names(catalog, tool_name, 3);
    if suggestions.is_empty() {
        return format!(
            "Tool '{tool_name}' is not available in the current tool catalog. \
             Verify mode/feature flags, or use {TOOL_SEARCH_BM25_NAME} with a short query."
        );
    }

    format!(
        "Tool '{tool_name}' is not available in the current tool catalog. \
         Did you mean: {}? You can also use {TOOL_SEARCH_BM25_NAME} to discover tools.",
        suggestions.join(", ")
    )
}

#[cfg(test)]
pub(super) fn maybe_activate_requested_deferred_tool(
    tool_name: &str,
    catalog: &[Tool],
    active_tools: &mut HashSet<String>,
) -> bool {
    let Some(def) = catalog.iter().find(|def| def.name == tool_name) else {
        return false;
    };

    if !def.defer_loading.unwrap_or(false) || active_tools.contains(tool_name) {
        return false;
    }

    active_tools.insert(tool_name.to_string())
}

pub(super) fn maybe_hydrate_requested_deferred_tool(
    tool_name: &str,
    tool_input: &Value,
    catalog: &[Tool],
    active_tools_at_batch_start: &HashSet<String>,
    hydrated_tools_this_batch: &mut HashSet<String>,
) -> Option<ToolResult> {
    let def = catalog.iter().find(|def| def.name == tool_name)?;

    if !def.defer_loading.unwrap_or(false) || active_tools_at_batch_start.contains(tool_name) {
        return None;
    }

    hydrated_tools_this_batch.insert(tool_name.to_string());
    Some(deferred_tool_schema_hydration_result(def, tool_input))
}

fn deferred_tool_schema_hydration_result(tool: &Tool, tool_input: &Value) -> ToolResult {
    let expected = schema_field_lines(tool);
    let received = received_field_names(tool_input);
    let corrections = likely_field_corrections(&tool.name, &received);

    let expected_text = if expected.is_empty() {
        "  (no declared fields)".to_string()
    } else {
        expected
            .iter()
            .map(|field| format!("  {field}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let received_text = if received.is_empty() {
        "  (none)".to_string()
    } else {
        format!("  {}", received.join(", "))
    };
    let correction_text = if corrections.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nLikely correction:\n{}",
            corrections
                .iter()
                .map(|field| format!("  {field}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    ToolResult::success(format!(
        "Tool `{}` was deferred and has now been loaded.\n\nExpected schema:\n{}\n\nReceived fields:\n{}{}\n\nThe tool was not executed. Retry the same operation with the loaded schema.",
        tool.name, expected_text, received_text, correction_text
    ))
    .with_metadata(json!({
        "event": "tool.schema_hydrated",
        "tool": tool.name,
        "executed": false,
        "retry_required": true,
        "reason": "deferred_tool_first_use",
    }))
}

fn schema_field_lines(tool: &Tool) -> Vec<String> {
    let mut required = Vec::new();
    if let Some(items) = tool.input_schema.get("required").and_then(Value::as_array) {
        for item in items {
            if let Some(field) = item.as_str() {
                required.push(field.to_string());
            }
        }
    }

    let Some(properties) = tool
        .input_schema
        .get("properties")
        .and_then(Value::as_object)
    else {
        return required;
    };

    let mut fields = Vec::new();
    let mut seen = HashSet::new();
    for field in &required {
        if let Some(schema) = properties.get(field) {
            fields.push(format!("{field}: {}", schema_type_label(schema)));
            seen.insert(field.as_str());
        } else {
            fields.push(field.clone());
        }
    }
    for (field, schema) in properties {
        if seen.contains(field.as_str()) {
            continue;
        }
        fields.push(format!("{field}: {} (optional)", schema_type_label(schema)));
    }
    fields
}

fn schema_type_label(schema: &Value) -> String {
    schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("value")
        .to_string()
}

fn received_field_names(input: &Value) -> Vec<String> {
    let mut fields = input
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    fields.sort();
    fields
}

fn likely_field_corrections(tool_name: &str, received: &[String]) -> Vec<String> {
    if tool_name != "edit_file" {
        return Vec::new();
    }

    let has = |name: &str| received.iter().any(|field| field == name);
    let mut corrections = Vec::new();
    if has("old_string") {
        corrections.push("old_string -> search".to_string());
    } else if has("old_str") {
        corrections.push("old_str -> search".to_string());
    }
    if has("new_string") {
        corrections.push("new_string -> replace".to_string());
    } else if has("new_str") {
        corrections.push("new_str -> replace".to_string());
    } else if has("replacement") {
        corrections.push("replacement -> replace".to_string());
    }
    corrections
}

pub(super) fn execute_tool_search(
    tool_name: &str,
    input: &serde_json::Value,
    catalog: &[Tool],
    active_tools: &mut HashSet<String>,
) -> Result<ToolResult, ToolError> {
    let query = required_str(input, "query")?;
    let discovered = if tool_name == TOOL_SEARCH_REGEX_NAME {
        discover_tools_with_regex(catalog, query)?
    } else {
        discover_tools_with_bm25_like(catalog, query)
    };

    for name in &discovered {
        active_tools.insert(name.clone());
    }

    let references = discovered
        .iter()
        .map(|name| json!({"type": "tool_reference", "tool_name": name}))
        .collect::<Vec<_>>();

    let payload = json!({
        "type": "tool_search_tool_search_result",
        "tool_references": references,
    });

    Ok(ToolResult {
        content: serde_json::to_string(&payload).unwrap_or_else(|_| payload.to_string()),
        success: true,
        metadata: Some(json!({
            "tool_references": discovered,
        })),
    })
}

pub(super) async fn execute_code_execution_tool(
    input: &serde_json::Value,
    workspace: &Path,
) -> Result<ToolResult, ToolError> {
    let code = required_str(input, "code")?;

    // Resolve the locally-installed Python interpreter we cached at
    // catalog-build time. If it's absent now (somehow registered but
    // disappeared between startup and this call — concurrent uninstall,
    // PATH change, etc.) we fail fast with a clear message rather than
    // dropping into `tokio::process::Command::new("python3")` and
    // surfacing the cryptic "program not found" the contributor
    // originally hit on Windows.
    let interpreter = crate::dependencies::resolve_python_interpreter().ok_or_else(|| {
        ToolError::execution_failed(format!(
            "code_execution: no Python interpreter found on PATH (tried {:?}). \
             Install Python 3 and ensure one of these is on PATH, then restart \
             deepseek-tui.",
            crate::dependencies::PYTHON_CANDIDATES,
        ))
    })?;
    let (program, args) = crate::dependencies::split_interpreter_spec(&interpreter);

    // Write the code to a temp file and execute it as a script rather
    // than passing it via `-c "<code>"`. Reasons:
    //   * `-c` has length limits (argv) on Windows.
    //   * Multiline code with quote nesting is brittle through `-c`.
    //   * Tracebacks reference a real filename instead of `<string>`,
    //     so the model can interpret line numbers correctly.
    // Tempfile lives only for the duration of this execution; Drop
    // removes it. We use `.py` so any shebang / encoding-sniffer
    // logic in the interpreter behaves normally.
    let temp_dir = tempfile::tempdir()
        .map_err(|e| ToolError::execution_failed(format!("tempdir failed: {e}")))?;
    let script_path = temp_dir.path().join("code_execution.py");
    tokio::fs::write(&script_path, code)
        .await
        .map_err(|e| ToolError::execution_failed(format!("tempfile write failed: {e}")))?;

    let mut cmd = tokio::process::Command::new(&program);
    for arg in &args {
        cmd.arg(arg);
    }
    cmd.arg(&script_path);
    cmd.current_dir(workspace);

    let output = tokio::time::timeout(Duration::from_secs(120), cmd.output())
        .await
        .map_err(|_| ToolError::Timeout { seconds: 120 })
        .and_then(|res| res.map_err(|e| ToolError::execution_failed(e.to_string())))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let return_code = output.status.code().unwrap_or(-1);
    let success = output.status.success();
    let payload = json!({
        "type": "code_execution_result",
        "stdout": stdout,
        "stderr": stderr,
        "return_code": return_code,
        "content": [],
    });

    Ok(ToolResult {
        content: serde_json::to_string(&payload).unwrap_or_else(|_| payload.to_string()),
        success,
        metadata: Some(payload),
    })
}
