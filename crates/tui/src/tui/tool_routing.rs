//! Active tool-card routing helpers for the TUI loop.

use std::path::PathBuf;
use std::time::Instant;

use crate::hooks::HookEvent;
use crate::tools::ReviewOutput;
use crate::tools::plan::PlanSnapshot;
use crate::tools::spec::{ToolError, ToolResult};
use crate::tui::active_cell::ActiveCell;
use crate::tui::app::{App, ToolDetailRecord, ToolEvidence};
use crate::tui::history::{
    DiffPreviewCell, ExecCell, ExecSource, ExploringEntry, GenericToolCell, HistoryCell,
    McpToolCell, PatchSummaryCell, PlanUpdateCell, ReviewCell, ToolCell, ToolStatus, ViewImageCell,
    WebSearchCell, output_looks_like_diff, summarize_mcp_output, summarize_tool_args,
    summarize_tool_output,
};
use crate::tui::workspace_context;

#[allow(clippy::too_many_lines)]
pub(super) fn handle_tool_call_started(
    app: &mut App,
    id: &str,
    name: &str,
    input: &serde_json::Value,
) {
    // #2511: ToolCallBefore gate moved to turn-loop planning loop
    // (Engine::handle_deepseek_turn). Removing observer-only firing
    // here to avoid double-firing hooks for each tool call.
    // Hooks that need observation can configure ToolCallBefore on
    // the turn-loop gate — it processes the denial (exit code 2).

    let id = id.to_string();

    // All in-flight tool work for the current turn lives in `app.active_cell`
    // until the turn completes. This mirrors Codex's contract: ONE active cell
    // mutates in place; finalized history isn't touched until flush. This
    // keeps the transcript stable while parallel completions arrive in any
    // order.
    if app.active_cell.is_none() {
        app.active_cell = Some(ActiveCell::new());
    }

    if is_exploring_tool(name) {
        let label = exploring_label(name, input);
        // ensure_exploring + append_to_exploring keeps all parallel exploring
        // starts in a single ExploringCell entry.
        let active = app.active_cell.as_mut().expect("active_cell just ensured");
        let entry_idx = active.ensure_exploring();
        app.active_tool_entry_completed_at.remove(&entry_idx);
        let inner = active
            .append_to_exploring(
                id.clone(),
                ExploringEntry {
                    label,
                    status: ToolStatus::Running,
                },
            )
            .map_or(0, |(_, inner)| inner);
        app.exploring_cell = Some(entry_idx);
        let virtual_index = app.history.len() + entry_idx;
        app.exploring_entries
            .insert(id.clone(), (virtual_index, inner));
        register_tool_cell(app, &id, name, input, virtual_index);
        app.mark_history_updated();
        return;
    }

    // Non-exploring tool: each is its own entry inside the active cell. We
    // intentionally do NOT clear `exploring_cell` here — the active cell can
    // hold both an exploring aggregate AND independent tool entries
    // simultaneously, which is exactly the case CX#7 fixes.

    if is_exec_tool(name) {
        let command = exec_target_from_input(input);
        let source = exec_source_from_input(input);
        let interaction = exec_interaction_summary(name, input);
        let mut is_wait = false;

        if let Some((summary, wait)) = interaction.as_ref() {
            is_wait = *wait;
            if is_wait
                && app
                    .last_exec_wait_command
                    .as_ref()
                    .is_some_and(|last| last == &command)
            {
                app.ignored_tool_calls.insert(id);
                return;
            }
            if is_wait {
                app.last_exec_wait_command = Some(command.clone());
            }

            push_active_tool_cell(
                app,
                &id,
                name,
                input,
                HistoryCell::Tool(ToolCell::Exec(ExecCell {
                    command,
                    status: ToolStatus::Running,
                    output: None,
                    live_output: None,
                    shell_task_id: None,
                    owner_agent_id: None,
                    owner_agent_name: None,
                    started_at: Some(Instant::now()),
                    duration_ms: None,
                    source,
                    interaction: Some(summary.clone()),
                    output_summary: None,
                })),
            );
            return;
        }

        if exec_is_background(input)
            && app
                .last_exec_wait_command
                .as_ref()
                .is_some_and(|last| last == &command)
        {
            app.ignored_tool_calls.insert(id);
            return;
        }
        if exec_is_background(input) && !is_wait {
            app.last_exec_wait_command = Some(command.clone());
        }

        push_active_tool_cell(
            app,
            &id,
            name,
            input,
            HistoryCell::Tool(ToolCell::Exec(ExecCell {
                command,
                status: ToolStatus::Running,
                output: None,
                live_output: None,
                shell_task_id: None,
                owner_agent_id: None,
                owner_agent_name: None,
                started_at: Some(Instant::now()),
                duration_ms: None,
                source,
                interaction: None,
                output_summary: None,
            })),
        );
        return;
    }

    if name == "update_plan" {
        let snapshot = parse_plan_input(input);
        push_active_tool_cell(
            app,
            &id,
            name,
            input,
            HistoryCell::Tool(ToolCell::PlanUpdate(PlanUpdateCell {
                snapshot,
                status: ToolStatus::Running,
            })),
        );
        return;
    }

    if name == "apply_patch" {
        let (path, summary) = parse_patch_summary(input);
        push_active_tool_cell(
            app,
            &id,
            name,
            input,
            HistoryCell::Tool(ToolCell::PatchSummary(PatchSummaryCell {
                path,
                summary,
                status: ToolStatus::Running,
                error: None,
            })),
        );
        return;
    }

    if name == "review" {
        let target = review_target_label(input);
        push_active_tool_cell(
            app,
            &id,
            name,
            input,
            HistoryCell::Tool(ToolCell::Review(ReviewCell {
                target,
                status: ToolStatus::Running,
                output: None,
                error: None,
            })),
        );
        return;
    }

    if is_mcp_tool(name) {
        push_active_tool_cell(
            app,
            &id,
            name,
            input,
            HistoryCell::Tool(ToolCell::Mcp(McpToolCell {
                tool: name.to_string(),
                status: ToolStatus::Running,
                content: None,
                is_image: false,
            })),
        );
        return;
    }

    if is_view_image_tool(name) {
        if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
            let raw_path = PathBuf::from(path);
            let display_path = raw_path
                .strip_prefix(&app.workspace)
                .unwrap_or(&raw_path)
                .to_path_buf();
            push_active_tool_cell(
                app,
                &id,
                name,
                input,
                HistoryCell::Tool(ToolCell::ViewImage(ViewImageCell { path: display_path })),
            );
        }
        return;
    }

    if is_web_search_tool(name) {
        let query = web_search_query(input);
        push_active_tool_cell(
            app,
            &id,
            name,
            input,
            HistoryCell::Tool(ToolCell::WebSearch(WebSearchCell {
                query,
                status: ToolStatus::Running,
                summary: None,
            })),
        );
        return;
    }

    let mut input_summary = summarize_tool_args(input);
    // Lead the `agent` args summary with the non-default action so renderers
    // can tell inspections (peek/status/wait) apart from spawns without a
    // schema change — a peek must not draw the same "delegate done" line as
    // a launch (#4112, dogfood A5).
    if name == "agent"
        && let Some(action) = input.get("action").and_then(serde_json::Value::as_str)
    {
        let action = action.trim().to_ascii_lowercase();
        let already_leads = input_summary
            .as_deref()
            .is_some_and(|summary| summary.starts_with("action:"));
        if !action.is_empty()
            && !already_leads
            && action != "start"
            && action != "spawn"
            && action != "run"
        {
            input_summary = Some(match input_summary {
                Some(rest) => format!("action: {action} {rest}"),
                None => format!("action: {action}"),
            });
        }
    }
    push_active_tool_cell(
        app,
        &id,
        name,
        input,
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: name.to_string(),
            status: ToolStatus::Running,
            input_summary,
            output: None,
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })),
    );
}

/// Push a tool cell as a new entry in `active_cell`, register the tool id,
/// and write a stub detail record so the pager / Ctrl+O can find it.
fn push_active_tool_cell(
    app: &mut App,
    tool_id: &str,
    tool_name: &str,
    input: &serde_json::Value,
    cell: HistoryCell,
) {
    if app.active_cell.is_none() {
        app.active_cell = Some(ActiveCell::new());
    }
    let active = app.active_cell.as_mut().expect("active_cell just ensured");
    let entry_idx = active.push_tool(tool_id.to_string(), cell);
    app.active_tool_entry_completed_at.remove(&entry_idx);
    let virtual_index = app.history.len() + entry_idx;
    register_tool_cell(app, tool_id, tool_name, input, virtual_index);
    app.mark_history_updated();
}

fn register_tool_cell(
    app: &mut App,
    tool_id: &str,
    tool_name: &str,
    input: &serde_json::Value,
    cell_index: usize,
) {
    app.tool_cells.insert(tool_id.to_string(), cell_index);
    let record = ToolDetailRecord {
        tool_id: tool_id.to_string(),
        tool_name: tool_name.to_string(),
        input: input.clone(),
        output: None,
    };
    if cell_index < app.history.len() {
        app.tool_details_by_cell.insert(cell_index, record);
    } else {
        // Active-cell entry: keep the detail record in `active_tool_details`
        // until the active cell flushes. `flush_active_cell` migrates these
        // records into `tool_details_by_cell` keyed by the eventual real
        // cell index.
        app.active_tool_details.insert(tool_id.to_string(), record);
    }
}

fn store_tool_detail_output(
    app: &mut App,
    tool_id: &str,
    cell_index: usize,
    result: &Result<ToolResult, ToolError>,
) {
    let payload = Some(match result {
        Ok(tool_result) => tool_result.content.clone(),
        Err(err) => err.to_string(),
    });
    if cell_index < app.history.len()
        && let Some(detail) = app.tool_details_by_cell.get_mut(&cell_index)
    {
        detail.output = payload.clone();
    }
    // Also write to the active table while the entry might still live there;
    // some callsites pre-rewrite cell_index but the active_tool_details map is
    // the canonical source for in-flight outputs.
    if let Some(detail) = app.active_tool_details.get_mut(tool_id) {
        detail.output = payload;
    }
}

#[allow(clippy::too_many_lines)]
/// Inspect a tool's success metadata for the `child_*` token-usage
/// fields that tools spawning their own LLM calls populate (e.g.
/// `rlm`). Roll any reported child-token cost into the session's
/// running sub-agent cost counter so the footer total reflects all
/// tokens the user is actually billed for, not just the parent turn's
/// tokens.
///
/// Without this hook, an RLM-heavy session shows a fraction of the
/// real spend because the parent turn's `Usage` only counts the
/// orchestrator's tokens, not the dozens of `deepseek-v4-flash` child
/// rounds RLM fans out under the hood (#524).
fn accrue_child_token_cost_if_any(app: &mut App, result: &Result<ToolResult, ToolError>) {
    let Ok(tool_result) = result else { return };
    let Some(metadata) = tool_result.metadata.as_ref() else {
        return;
    };
    let Some(model) = metadata
        .get("child_model")
        .and_then(serde_json::Value::as_str)
    else {
        return;
    };
    let input_tokens = metadata
        .get("child_input_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let output_tokens = metadata
        .get("child_output_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if input_tokens == 0 && output_tokens == 0 {
        return;
    }
    let prompt_cache_hit_tokens = metadata
        .get("child_prompt_cache_hit_tokens")
        .and_then(serde_json::Value::as_u64)
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX));
    let prompt_cache_miss_tokens = metadata
        .get("child_prompt_cache_miss_tokens")
        .and_then(serde_json::Value::as_u64)
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX));
    let usage = crate::models::Usage {
        input_tokens: u32::try_from(input_tokens).unwrap_or(u32::MAX),
        output_tokens: u32::try_from(output_tokens).unwrap_or(u32::MAX),
        prompt_cache_hit_tokens,
        prompt_cache_miss_tokens,
        prompt_cache_write_tokens: None,
        reasoning_tokens: None,
        reasoning_replay_tokens: None,
        server_tool_use: None,
    };
    let provider = metadata
        .get("child_provider")
        .or_else(|| metadata.get("resolved_provider"))
        .and_then(serde_json::Value::as_str)
        .and_then(crate::config::ApiProvider::parse)
        .unwrap_or(app.api_provider);
    if let Some(cost) =
        crate::pricing::calculate_turn_cost_estimate_for_provider(provider, model, &usage)
    {
        app.accrue_subagent_cost_estimate(cost);
    }
}

fn record_spillover_artifact_if_any(
    app: &mut App,
    id: &str,
    name: &str,
    result: &Result<ToolResult, ToolError>,
) {
    let Ok(tool_result) = result else { return };
    if !tool_result.success {
        return;
    }
    let Some(path) = tool_result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("spillover_path"))
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
    else {
        return;
    };
    let metadata = tool_result.metadata.as_ref();
    let session_id = metadata
        .and_then(|metadata| metadata.get("artifact_session_id"))
        .and_then(serde_json::Value::as_str)
        .or(app.current_session_id.as_deref())
        .unwrap_or("");
    let storage_path = metadata
        .and_then(|metadata| metadata.get("artifact_relative_path"))
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| path.clone());
    let content_for_preview = metadata
        .and_then(|metadata| metadata.get("artifact_preview"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&tool_result.content);
    let byte_size = metadata
        .and_then(|metadata| metadata.get("artifact_byte_size"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_else(|| {
            std::fs::metadata(&storage_path)
                .map(|metadata| metadata.len())
                .unwrap_or(tool_result.content.len() as u64)
        });
    if app
        .session_artifacts
        .iter()
        .any(|artifact| artifact.tool_call_id == id && artifact.storage_path == storage_path)
    {
        return;
    }
    app.session_artifacts
        .push(crate::artifacts::record_tool_output_artifact_with_size(
            session_id,
            id,
            name,
            storage_path,
            byte_size,
            content_for_preview,
        ));
}

/// #3031: shell/tasks tools embed the literal `"(no output)"` into successful
/// `ToolResult` content (the model-facing transcript needs a non-empty tool
/// result). Treat it as no output on the TUI side so the compact-mode
/// suppression gate in `history.rs` actually fires; the raw content remains
/// available through the tool-detail store.
fn visible_tool_output(content: &str) -> Option<String> {
    if content.trim() == "(no output)" {
        None
    } else {
        Some(content.to_string())
    }
}

pub(super) fn handle_tool_call_complete(
    app: &mut App,
    id: &str,
    name: &str,
    result: &Result<ToolResult, ToolError>,
) {
    if app.ignored_tool_calls.remove(id) {
        return;
    }
    // Roll any child-LLM token usage the tool reports into the
    // session-cost counter. Runs unconditionally so future tools that
    // spawn their own LLM calls (RLM, summarizers, retrieval helpers)
    // get accrued without needing a per-tool hook (#524).
    accrue_child_token_cost_if_any(app, result);
    record_spillover_artifact_if_any(app, id, name, result);

    // Exploring entries land in the per-tool map regardless of whether they
    // live in the active cell or in finalized history; the path is the same.
    if let Some((cell_index, entry_index)) = app.exploring_entries.remove(id) {
        app.tool_cells.remove(id);
        store_tool_detail_output(app, id, cell_index, result);
        if let Some(HistoryCell::Tool(ToolCell::Exploring(cell))) =
            app.cell_at_virtual_index_mut(cell_index)
            && let Some(entry) = cell.entries.get_mut(entry_index)
        {
            entry.status = tool_status_from_result(result);
            app.mark_history_updated();
            // Mutating the in-flight exploring cell needs an active-cell
            // revision bump so the transcript cache invalidates the synthetic
            // tail row.
            if cell_index >= app.history.len() {
                app.active_cell_revision = app.active_cell_revision.wrapping_add(1);
                if let Some(active) = app.active_cell.as_mut() {
                    active.bump_revision();
                }
            }
        }
        refresh_active_tool_completion_timestamp(app, cell_index);
        return;
    }

    // Look up the cell by tool id. If the id isn't registered, that's an
    // orphan completion (race condition where the started event was lost or
    // a tool result arrived after the active cell was already flushed). Build
    // a finalized standalone cell from the result so the user can still see
    // the output, but DO NOT touch the active cell.
    let Some(cell_index) = app.tool_cells.remove(id) else {
        push_orphan_tool_completion(app, id, name, result);
        return;
    };

    store_tool_detail_output(app, id, cell_index, result);
    let in_active = cell_index >= app.history.len();

    let status = tool_status_from_result(result);
    let mut workflow_panel_output: Option<String> = None;

    if let Some(cell) = app.cell_at_virtual_index_mut(cell_index) {
        match cell {
            HistoryCell::Tool(ToolCell::Exec(exec)) => {
                exec.status = status;
                if let Ok(tool_result) = result.as_ref() {
                    let shell_task_id = tool_result
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("task_id"))
                        .and_then(serde_json::Value::as_str)
                        .filter(|task_id| !task_id.trim().is_empty())
                        .map(str::to_string);
                    if shell_task_id.is_some() {
                        exec.shell_task_id = shell_task_id;
                    }
                    exec.owner_agent_id = tool_result
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("owner_agent_id"))
                        .and_then(serde_json::Value::as_str)
                        .filter(|agent_id| !agent_id.trim().is_empty())
                        .map(str::to_string);
                    exec.owner_agent_name = tool_result
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("owner_agent_name"))
                        .and_then(serde_json::Value::as_str)
                        .filter(|agent_name| !agent_name.trim().is_empty())
                        .map(str::to_string);
                    if let Some(meta_command) = tool_result
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("command"))
                        .and_then(serde_json::Value::as_str)
                        && !meta_command.trim().is_empty()
                        && (exec.command == "command" || exec.command.starts_with("command "))
                    {
                        exec.command = meta_command.to_string();
                        if exec.interaction.as_deref().is_some_and(|interaction| {
                            interaction.starts_with("Waiting for command")
                        }) {
                            let task_suffix = tool_result
                                .metadata
                                .as_ref()
                                .and_then(|m| m.get("task_id"))
                                .and_then(serde_json::Value::as_str)
                                .map(|task_id| format!(" ({task_id})"))
                                .unwrap_or_default();
                            exec.interaction =
                                Some(format!("Waiting for \"{meta_command}\"{task_suffix}"));
                        }
                    }
                    exec.duration_ms = tool_result
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("duration_ms"))
                        .and_then(serde_json::Value::as_u64);
                    if status != ToolStatus::Running && exec.interaction.is_none() {
                        exec.output = visible_tool_output(&tool_result.content);
                        exec.output_summary = exec
                            .output
                            .as_deref()
                            .map(super::history::summarize_tool_output);
                        exec.live_output = None;
                    } else if status == ToolStatus::Running
                        && exec.interaction.is_none()
                        && !tool_result.content.is_empty()
                    {
                        exec.live_output = Some(tool_result.content.clone());
                    }
                } else if let Err(err) = result.as_ref()
                    && exec.interaction.is_none()
                {
                    exec.output = Some(err.to_string());
                    exec.output_summary =
                        Some(super::history::summarize_tool_output(&err.to_string()));
                }
                app.mark_history_updated();
            }
            HistoryCell::Tool(ToolCell::PlanUpdate(plan)) => {
                plan.status = status;
                app.mark_history_updated();
            }
            HistoryCell::Tool(ToolCell::PatchSummary(patch)) => {
                patch.status = status;
                match result.as_ref() {
                    Ok(tool_result) => {
                        if let Ok(json) =
                            serde_json::from_str::<serde_json::Value>(&tool_result.content)
                            && let Some(message) = json.get("message").and_then(|v| v.as_str())
                        {
                            patch.summary = message.to_string();
                        }
                    }
                    Err(err) => {
                        patch.error = Some(err.to_string());
                    }
                }
                app.mark_history_updated();
            }
            HistoryCell::Tool(ToolCell::Review(review)) => {
                review.status = status;
                match result.as_ref() {
                    Ok(tool_result) => {
                        if tool_result.success {
                            review.output = Some(ReviewOutput::from_str(&tool_result.content));
                        } else {
                            review.error = Some(tool_result.content.clone());
                        }
                    }
                    Err(err) => {
                        review.error = Some(err.to_string());
                    }
                }
                app.mark_history_updated();
            }
            HistoryCell::Tool(ToolCell::Mcp(mcp)) => {
                match result.as_ref() {
                    Ok(tool_result) => {
                        let summary = summarize_mcp_output(&tool_result.content);
                        if status == ToolStatus::Hydrated {
                            mcp.status = status;
                        } else if summary.is_error == Some(true) {
                            mcp.status = ToolStatus::Failed;
                        } else {
                            mcp.status = status;
                        }
                        mcp.is_image = summary.is_image;
                        mcp.content = summary.content;
                    }
                    Err(err) => {
                        mcp.status = status;
                        mcp.content = Some(err.to_string());
                    }
                }
                app.mark_history_updated();
            }
            HistoryCell::Tool(ToolCell::WebSearch(search)) => {
                search.status = status;
                match result.as_ref() {
                    Ok(tool_result) => {
                        search.summary = Some(summarize_tool_output(&tool_result.content));
                    }
                    Err(err) => {
                        search.summary = Some(err.to_string());
                    }
                }
                app.mark_history_updated();
            }
            HistoryCell::Tool(ToolCell::Generic(generic)) => {
                generic.status = status;
                match result.as_ref() {
                    Ok(tool_result) => {
                        generic.output = visible_tool_output(&tool_result.content);
                        generic.output_summary =
                            generic.output.as_deref().map(summarize_tool_output);
                        generic.is_diff = output_looks_like_diff(&tool_result.content);
                    }
                    Err(err) => {
                        generic.output = Some(err.to_string());
                        generic.output_summary = Some(summarize_tool_output(&err.to_string()));
                        generic.is_diff = false;
                    }
                }
                // #4121: capture workflow JSON before releasing the cell borrow
                // so we can hydrate the panel without overlapping borrows.
                if generic.name == "workflow" {
                    workflow_panel_output = generic.output.clone();
                }
                app.mark_history_updated();
            }
            _ => {}
        }
    }

    // #4121 / #4122: feed typed workflow events into the panel *and* keep the
    // history card snapshot in sync. Live streaming also arrives via
    // `Event::WorkflowUi`; this path covers tool-complete hydration.
    if let Some(output) = workflow_panel_output.as_deref() {
        apply_workflow_output_to_panel(app, output);
    }

    // If the mutated cell lived inside the active group, bump the active-cell
    // revision so the transcript cache re-renders the synthetic tail row.
    if in_active {
        app.active_cell_revision = app.active_cell_revision.wrapping_add(1);
        if let Some(active) = app.active_cell.as_mut() {
            active.bump_revision();
        }
        refresh_active_tool_completion_timestamp(app, cell_index);
    }

    if refreshes_workspace_context_on_completion(name) && status != ToolStatus::Running {
        workspace_context::refresh_now(app, Instant::now());
    }

    // #455 (observer-only): fire `tool_call_after` hooks once the
    // result has settled. Hooks see tool_name + the result content
    // (or error message) + success flag. Read-only — they cannot
    // mutate the result that goes back to the model. Mutation
    // remains a v0.8.9 follow-up. Fast-path skip avoids the
    // result.content.clone() and HookContext allocation when no
    // hooks are configured.
    if app.hooks.has_hooks_for_event(HookEvent::ToolCallAfter) {
        let (result_text, success): (String, bool) = match result.as_ref() {
            Ok(tool_result) => (tool_result.content.clone(), tool_result.success),
            Err(err) => (err.to_string(), false),
        };
        let context = app
            .base_hook_context()
            .with_tool_name(name)
            .with_tool_result(&result_text, success, None);
        let _ = app.execute_hooks(HookEvent::ToolCallAfter, &context);
    }

    // Collect evidence for the post-turn receipt.
    let evidence_summary = match result.as_ref() {
        Ok(tool_result) => {
            if tool_result.success {
                summarize_tool_output(&tool_result.content)
            } else {
                format!("failed: {}", summarize_tool_output(&tool_result.content))
            }
        }
        Err(err) => format!("error: {err}"),
    };
    app.tool_evidence.push(ToolEvidence {
        tool_name: name.to_string(),
        summary: evidence_summary,
    });
}

/// Hydrate or advance the WorkflowPanel from a workflow tool JSON payload.
/// Accepts a single run record (with optional `events` array) or a status
/// list. Log-only events are filtered by the panel itself so the transcript
/// stays free of progress spam (#4121). Also keeps the matching history card
/// snapshot aligned (#4122).
fn apply_workflow_output_to_panel(app: &mut App, output: &str) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(output) else {
        return;
    };

    // Prefer the typed event stream when present.
    if let Some(events) = value.get("events").and_then(|e| e.as_array()) {
        // Ensure a panel exists before applying — seed from run_id/goal if needed.
        if app.workflow_panel.is_none() {
            let run_id = value
                .get("run_id")
                .and_then(|v| v.as_str())
                .unwrap_or("workflow")
                .to_string();
            let label = value
                .get("workflow_goal")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("workflow_id").and_then(|v| v.as_str()))
                .unwrap_or("workflow")
                .to_string();
            let at_ms = value
                .get("started_at_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            app.workflow_panel = Some(crate::tui::widgets::workflow_panel::WorkflowPanel::new(
                run_id, label, at_ms,
            ));
        }
        if let Some(panel) = app.workflow_panel.as_mut() {
            let run_id = value
                .get("run_id")
                .and_then(|v| v.as_str())
                .unwrap_or(&panel.run_id)
                .to_string();
            let mut injected = Vec::with_capacity(events.len());
            for event in events {
                let mut event = event.clone();
                if let Some(obj) = event.as_object_mut() {
                    obj.entry("run_id".to_string())
                        .or_insert_with(|| serde_json::Value::String(run_id.clone()));
                }
                injected.push(event);
            }
            panel.apply_json_events(&injected);
            // Carry final result / source into panel for expanded history card.
            if let Some(summary) = value
                .get("result")
                .map(|v| v.to_string())
                .filter(|s| s != "null")
            {
                panel.result_summary = Some(summary);
            }
            if let Some(path) = value.get("source_path").and_then(|v| v.as_str()) {
                panel.source_path = Some(PathBuf::from(path));
            }
            app.needs_redraw = true;
        }
        sync_workflow_history_card_from_panel(app);
        return;
    }

    // Fallback: status list — show the most recent run as a shell panel.
    if value.get("action").and_then(|v| v.as_str()) == Some("status") {
        if let Some(runs) = value.get("runs").and_then(|r| r.as_array())
            && let Some(run) = runs.last()
        {
            apply_workflow_output_to_panel(app, &run.to_string());
        }
        return;
    }

    // Prefer full panel hydration from summary/phases snapshot when present.
    if let Some(panel) = crate::tui::widgets::workflow_panel::WorkflowPanel::from_run_json(&value) {
        app.workflow_panel = Some(panel);
        app.needs_redraw = true;
        sync_workflow_history_card_from_panel(app);
        return;
    }

    // Fallback: bare run record without events — at least surface header state.
    if let Some(run_id) = value.get("run_id").and_then(|v| v.as_str()) {
        use crate::tui::widgets::workflow_panel::{WorkflowPanelEvent, WorkflowPanelLifecycle};
        let label = value
            .get("workflow_goal")
            .and_then(|v| v.as_str())
            .or_else(|| value.get("workflow_id").and_then(|v| v.as_str()))
            .unwrap_or(run_id)
            .to_string();
        let at_ms = value
            .get("started_at_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let status = value
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("running");
        app.apply_workflow_panel_event(WorkflowPanelEvent::RunStarted {
            run_id: run_id.to_string(),
            workflow_id: value
                .get("workflow_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            workflow_goal: Some(label),
            source_path: value
                .get("source_path")
                .and_then(|v| v.as_str())
                .map(PathBuf::from),
            token_budget: value.get("token_budget").and_then(|v| v.as_u64()),
            at_ms,
        });
        if status != "running" {
            let life = match status {
                "completed" | "succeeded" => WorkflowPanelLifecycle::Succeeded,
                "failed" => WorkflowPanelLifecycle::Failed,
                "cancelled" | "canceled" => WorkflowPanelLifecycle::Cancelled,
                _ => WorkflowPanelLifecycle::Running,
            };
            if life != WorkflowPanelLifecycle::Running {
                app.apply_workflow_panel_event(WorkflowPanelEvent::RunCompleted {
                    status: life,
                    error: value
                        .get("error")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    at_ms: value
                        .get("completed_at_ms")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(at_ms),
                });
            }
        }
        sync_workflow_history_card_from_panel(app);
    }
}

/// Apply one live `WorkflowUi` engine event to the panel and history card.
pub(super) fn apply_workflow_ui_event(app: &mut App, run_id: &str, event: &serde_json::Value) {
    use crate::tui::widgets::workflow_panel::WorkflowPanelEvent;

    let mut event = event.clone();
    if let Some(obj) = event.as_object_mut() {
        obj.entry("run_id".to_string())
            .or_insert_with(|| serde_json::Value::String(run_id.to_string()));
    }
    if let Some(panel_event) = WorkflowPanelEvent::from_json_value(&event) {
        app.apply_workflow_panel_event(panel_event);
    }
    sync_workflow_history_card_from_panel(app);
}

/// Mirror the live WorkflowPanel snapshot into the in-flight (or most recent)
/// workflow history tool cell so compact/expanded cards stay current.
fn sync_workflow_history_card_from_panel(app: &mut App) {
    let Some(panel) = app.workflow_panel.as_ref() else {
        return;
    };
    let run_id = panel.run_id.clone();
    let snapshot = panel.to_run_json().to_string();

    // Prefer an in-flight Generic(workflow) cell whose output already carries
    // this run_id, else the newest running workflow cell, else any workflow
    // cell (tool-complete path already wrote the final output).
    let mut target: Option<usize> = None;
    let history_len = app.history.len();
    let total = history_len
        + app
            .active_cell
            .as_ref()
            .map(|a| a.entries().len())
            .unwrap_or(0);

    for idx in (0..total).rev() {
        let Some(cell) = app.cell_at_virtual_index(idx) else {
            continue;
        };
        let HistoryCell::Tool(ToolCell::Generic(generic)) = cell else {
            continue;
        };
        if generic.name != "workflow" {
            continue;
        }
        let matches_run = generic
            .output
            .as_deref()
            .and_then(|out| serde_json::from_str::<serde_json::Value>(out).ok())
            .and_then(|v| {
                v.get("run_id")
                    .and_then(|id| id.as_str())
                    .map(|id| id == run_id)
            })
            .unwrap_or(false);
        let is_running = generic.status == ToolStatus::Running;
        if matches_run || (is_running && target.is_none()) {
            target = Some(idx);
            if matches_run {
                break;
            }
        }
    }

    let Some(idx) = target else {
        return;
    };
    if let Some(HistoryCell::Tool(ToolCell::Generic(generic))) = app.cell_at_virtual_index_mut(idx)
    {
        // Preserve a richer final output if the tool completion already wrote
        // a full run record with an events array longer than the snapshot.
        let replace = match generic.output.as_deref() {
            None => true,
            Some(existing) => {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(existing) else {
                    return;
                };
                let existing_run = value.get("run_id").and_then(|v| v.as_str()).unwrap_or("");
                if !existing_run.is_empty() && existing_run != run_id {
                    return;
                }
                // Prefer full event-bearing records when the tool has completed.
                if generic.status == ToolStatus::Running {
                    true
                } else {
                    value
                        .get("events")
                        .and_then(|e| e.as_array())
                        .is_none_or(|e| e.is_empty())
                }
            }
        };
        if replace {
            generic.output = Some(snapshot);
            generic.output_summary = Some(format!("workflow {}", run_id));
            app.mark_history_updated();
        }
    }
}

fn refresh_active_tool_completion_timestamp(app: &mut App, cell_index: usize) {
    if cell_index < app.history.len() {
        return;
    }
    let entry_idx = cell_index - app.history.len();
    let Some(cell) = app.cell_at_virtual_index(cell_index) else {
        app.active_tool_entry_completed_at.remove(&entry_idx);
        return;
    };

    if history_cell_has_running_tool(cell) {
        app.active_tool_entry_completed_at.remove(&entry_idx);
    } else {
        app.active_tool_entry_completed_at
            .entry(entry_idx)
            .or_insert_with(Instant::now);
    }
}

fn history_cell_has_running_tool(cell: &HistoryCell) -> bool {
    let HistoryCell::Tool(tool) = cell else {
        return false;
    };
    match tool {
        ToolCell::Exec(exec) => exec.status == ToolStatus::Running,
        ToolCell::Exploring(explore) => explore
            .entries
            .iter()
            .any(|entry| entry.status == ToolStatus::Running),
        ToolCell::PlanUpdate(plan) => plan.status == ToolStatus::Running,
        ToolCell::PatchSummary(patch) => patch.status == ToolStatus::Running,
        ToolCell::Review(review) => review.status == ToolStatus::Running,
        ToolCell::DiffPreview(_) => false,
        ToolCell::Mcp(mcp) => mcp.status == ToolStatus::Running,
        ToolCell::ViewImage(_) => false,
        ToolCell::WebSearch(search) => search.status == ToolStatus::Running,
        ToolCell::Generic(generic) => generic.status == ToolStatus::Running,
    }
}

/// Build a finalized standalone history cell for a tool completion whose
/// start was never registered (orphan). This preserves the contract that
/// every tool result is visible somewhere; the alternative (silently
/// dropping it) hides errors and breaks debuggability.
///
/// Choice of cell type: we use `GenericToolCell` because we have no input
/// payload to reconstruct a more specific cell. The pager remains usable —
/// `tool_details_by_cell` is populated with the result text.
///
/// ## Index drift
///
/// If an active cell is in flight when the orphan arrives, pushing the
/// orphan into `app.history` shifts every active-cell virtual index forward
/// by 1. We must rewrite `tool_cells` / `exploring_entries` accordingly so
/// later completion lookups still find the right entries.
fn push_orphan_tool_completion(
    app: &mut App,
    tool_id: &str,
    name: &str,
    result: &Result<ToolResult, ToolError>,
) {
    let status = tool_status_from_result(result);
    let output = match result.as_ref() {
        Ok(tool_result) => Some(summarize_tool_output(&tool_result.content)),
        Err(err) => Some(err.to_string()),
    };
    let history_threshold_before_push = app.history.len();
    let active_in_flight = app.active_cell.is_some();
    let spillover_path = result
        .as_ref()
        .ok()
        .and_then(|r| r.metadata.as_ref())
        .and_then(|m| m.get("spillover_path"))
        .and_then(serde_json::Value::as_str)
        .map(std::path::PathBuf::from);
    let output_summary = output.as_deref().map(summarize_tool_output);
    let is_diff = output.as_deref().is_some_and(output_looks_like_diff);
    app.add_message(HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
        name: name.to_string(),
        status,
        input_summary: None,
        output,
        prompts: None,
        spillover_path,
        output_summary,
        is_diff,
    })));
    let cell_index = app.history.len().saturating_sub(1);
    app.tool_details_by_cell.insert(
        cell_index,
        ToolDetailRecord {
            tool_id: tool_id.to_string(),
            tool_name: name.to_string(),
            input: serde_json::Value::Null,
            output: match result.as_ref() {
                Ok(tool_result) => Some(tool_result.content.clone()),
                Err(err) => Some(err.to_string()),
            },
        },
    );

    // Shift active-cell virtual indices forward by 1 to absorb the new
    // history cell. Without this, the next completion would address the
    // wrong entry.
    if active_in_flight {
        let threshold = history_threshold_before_push;
        for idx in app.tool_cells.values_mut() {
            if *idx >= threshold {
                *idx = idx.wrapping_add(1);
            }
        }
        for (cell_idx, _) in app.exploring_entries.values_mut() {
            if *cell_idx >= threshold {
                *cell_idx = cell_idx.wrapping_add(1);
            }
        }
        if let Some(idx) = app.exploring_cell.as_mut()
            && *idx >= threshold
        {
            *idx = idx.wrapping_add(1);
        }
    }
}

fn tool_status_from_result(result: &Result<ToolResult, ToolError>) -> ToolStatus {
    match result.as_ref() {
        Ok(tool_result) if is_deferred_schema_hydration(tool_result) => ToolStatus::Hydrated,
        Ok(tool_result) => match tool_result.metadata.as_ref() {
            Some(meta)
                if meta
                    .get("status")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s == "Running") =>
            {
                ToolStatus::Running
            }
            _ => {
                if tool_result.success {
                    ToolStatus::Success
                } else {
                    ToolStatus::Failed
                }
            }
        },
        Err(_) => ToolStatus::Failed,
    }
}

fn is_deferred_schema_hydration(tool_result: &ToolResult) -> bool {
    if !tool_result.success {
        return false;
    }
    let Some(metadata) = tool_result.metadata.as_ref() else {
        return false;
    };
    metadata
        .get("event")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|event| event == "tool.schema_hydrated")
        && metadata
            .get("executed")
            .and_then(serde_json::Value::as_bool)
            .is_some_and(|executed| !executed)
}

fn is_exploring_tool(name: &str) -> bool {
    matches!(name, "read_file" | "list_dir" | "grep_files" | "list_files")
}

fn is_exec_tool(name: &str) -> bool {
    matches!(
        name,
        "exec_shell" | "exec_shell_wait" | "exec_shell_interact" | "exec_wait" | "exec_interact"
    )
}

pub(super) fn refreshes_workspace_context_on_completion(name: &str) -> bool {
    matches!(
        name,
        "exec_shell"
            | "exec_shell_wait"
            | "exec_shell_interact"
            | "exec_wait"
            | "exec_interact"
            | "task_shell_start"
            | "task_shell_wait"
    )
}

pub(super) fn exploring_label(name: &str, input: &serde_json::Value) -> String {
    let fallback = format!("{name} tool");
    let obj = input.as_object();
    match name {
        "read_file" => obj
            .and_then(|o| o.get("path"))
            .and_then(|v| v.as_str())
            .map_or(fallback, |path| format!("Reading {path}")),
        "list_dir" => obj
            .and_then(|o| o.get("path"))
            .and_then(|v| v.as_str())
            .map_or("Listing directory".to_string(), |path| {
                format!("Listing {path}")
            }),
        "grep_files" => {
            let pattern = obj
                .and_then(|o| o.get("pattern"))
                .and_then(|v| v.as_str())
                .unwrap_or("pattern");
            format!("Searching for `{pattern}`")
        }
        "list_files" => "Listing files".to_string(),
        _ => fallback,
    }
}

fn is_mcp_tool(name: &str) -> bool {
    name.starts_with("mcp_")
}

fn is_view_image_tool(name: &str) -> bool {
    matches!(name, "view_image" | "view_image_file" | "view_image_tool")
}

fn is_web_search_tool(name: &str) -> bool {
    matches!(name, "web_search" | "search_web" | "search" | "web.run")
        || name.ends_with("_web_search")
}

fn web_search_query(input: &serde_json::Value) -> String {
    if let Some(searches) = input.get("search_query").and_then(|v| v.as_array())
        && let Some(first) = searches.first()
        && let Some(q) = first.get("q").and_then(|v| v.as_str())
    {
        return q.to_string();
    }

    input
        .get("query")
        .or_else(|| input.get("q"))
        .or_else(|| input.get("search"))
        .and_then(|v| v.as_str())
        .unwrap_or("Web search")
        .to_string()
}

fn review_target_label(input: &serde_json::Value) -> String {
    let target = input
        .get("target")
        .and_then(|v| v.as_str())
        .unwrap_or("review")
        .trim();
    let kind = input
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let staged = input
        .get("staged")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let target_lower = target.to_ascii_lowercase();

    if kind == "diff"
        || target_lower == "diff"
        || target_lower == "git diff"
        || target_lower == "staged"
        || target_lower == "cached"
    {
        if staged || target_lower == "staged" || target_lower == "cached" {
            return "git diff --cached".to_string();
        }
        return "git diff".to_string();
    }

    target.to_string()
}

fn parse_plan_input(input: &serde_json::Value) -> PlanSnapshot {
    PlanSnapshot::from_tool_input(input)
}

fn parse_patch_summary(input: &serde_json::Value) -> (String, String) {
    if let Some(changes) = input.get("changes").and_then(|v| v.as_array()) {
        let count = changes.len();
        let path = changes
            .first()
            .and_then(|c| c.get("path"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| "<file>".to_string());
        let label = if count <= 1 {
            path
        } else {
            format!("{count} files")
        };
        let summary = format!("Changes: {count} file(s)");
        return (label, summary);
    }

    let patch_text = input.get("patch").and_then(|v| v.as_str()).unwrap_or("");
    let paths = extract_patch_paths(patch_text);
    let path = input
        .get("path")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            if paths.len() == 1 {
                paths.first().cloned()
            } else if paths.is_empty() {
                None
            } else {
                Some(format!("{} files", paths.len()))
            }
        })
        .unwrap_or_else(|| "<file>".to_string());

    let (adds, removes) = count_patch_changes(patch_text);
    let summary = if adds == 0 && removes == 0 {
        "Patch applied".to_string()
    } else {
        format!("Changes: +{adds} / -{removes}")
    };
    (path, summary)
}

fn extract_patch_paths(patch: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            let raw = rest.trim();
            if raw == "/dev/null" || raw == "dev/null" {
                continue;
            }
            let raw = raw.strip_prefix("b/").unwrap_or(raw);
            if !paths.contains(&raw.to_string()) {
                paths.push(raw.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("diff --git ") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if let Some(path) = parts.get(1).or_else(|| parts.first()) {
                let raw = path.trim();
                let raw = raw
                    .strip_prefix("b/")
                    .or_else(|| raw.strip_prefix("a/"))
                    .unwrap_or(raw);
                if !paths.contains(&raw.to_string()) {
                    paths.push(raw.to_string());
                }
            }
        }
    }
    paths
}

pub(super) fn maybe_add_patch_preview(app: &mut App, input: &serde_json::Value) {
    if let Some(patch) = input.get("patch").and_then(|v| v.as_str()) {
        app.add_message(HistoryCell::Tool(ToolCell::DiffPreview(DiffPreviewCell {
            title: "Patch Preview".to_string(),
            diff: patch.to_string(),
        })));
        app.mark_history_updated();
        return;
    }

    if let Some(changes) = input.get("changes").and_then(|v| v.as_array()) {
        let preview = format_changes_preview(changes);
        if !preview.trim().is_empty() {
            app.add_message(HistoryCell::Tool(ToolCell::DiffPreview(DiffPreviewCell {
                title: "Changes Preview".to_string(),
                diff: preview,
            })));
            app.mark_history_updated();
        }
    }
}

fn format_changes_preview(changes: &[serde_json::Value]) -> String {
    let mut out = String::new();
    for change in changes {
        let path = change
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("<file>");
        let content = change.get("content").and_then(|v| v.as_str()).unwrap_or("");

        out.push_str(&format!("diff --git a/{path} b/{path}\n"));
        out.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
        out.push_str("@@ -0,0 +1,1 @@\n");

        let mut count = 0usize;
        for line in content.lines() {
            out.push('+');
            out.push_str(line);
            out.push('\n');
            count += 1;
            if count >= 20 {
                out.push_str("+... (truncated)\n");
                break;
            }
        }
        if content.is_empty() {
            out.push_str("+\n");
        }
    }
    out
}

fn count_patch_changes(patch: &str) -> (usize, usize) {
    let mut adds = 0;
    let mut removes = 0;
    for line in patch.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            adds += 1;
        } else if line.starts_with('-') {
            removes += 1;
        }
    }
    (adds, removes)
}

fn exec_command_from_input(input: &serde_json::Value) -> Option<String> {
    input
        .get("command")
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string)
}

fn exec_target_from_input(input: &serde_json::Value) -> String {
    exec_command_from_input(input).unwrap_or_else(|| {
        input
            .get("task_id")
            .or_else(|| input.get("id"))
            .and_then(|v| v.as_str())
            .map(|task_id| format!("command {task_id}"))
            .unwrap_or_else(|| "command".to_string())
    })
}

fn exec_source_from_input(input: &serde_json::Value) -> ExecSource {
    match input.get("source").and_then(|v| v.as_str()) {
        Some(source) if source.eq_ignore_ascii_case("user") => ExecSource::User,
        _ => ExecSource::Assistant,
    }
}

fn exec_interaction_summary(name: &str, input: &serde_json::Value) -> Option<(String, bool)> {
    let command = exec_target_from_input(input);
    let command_display = format!("\"{command}\"");
    let interaction_input = input
        .get("input")
        .or_else(|| input.get("stdin"))
        .or_else(|| input.get("data"))
        .and_then(|v| v.as_str());

    let is_wait_tool = matches!(name, "exec_shell_wait" | "exec_wait");
    let is_interact_tool = matches!(name, "exec_shell_interact" | "exec_interact");

    if is_interact_tool || interaction_input.is_some() {
        let preview = interaction_input.map(summarize_interaction_input);
        let summary = if let Some(preview) = preview {
            format!("Interacted with {command_display}, sent {preview}")
        } else {
            format!("Interacted with {command_display}")
        };
        return Some((summary, false));
    }

    if is_wait_tool || input.get("wait").and_then(serde_json::Value::as_bool) == Some(true) {
        if exec_command_from_input(input).is_none()
            && let Some(task_id) = input
                .get("task_id")
                .or_else(|| input.get("id"))
                .and_then(|v| v.as_str())
        {
            return Some((format!("Waiting for command {task_id}"), true));
        }
        return Some((format!("Waited for {command_display}"), true));
    }

    None
}

fn summarize_interaction_input(input: &str) -> String {
    let mut single_line = input.replace('\r', "");
    single_line = single_line.replace('\n', "\\n");
    single_line = single_line.replace('\"', "'");
    let max_len = 80;
    if single_line.chars().count() <= max_len {
        return format!("\"{single_line}\"");
    }
    let mut out = String::new();
    for ch in single_line.chars().take(max_len.saturating_sub(3)) {
        out.push(ch);
    }
    out.push_str("...");
    format!("\"{out}\"")
}

fn exec_is_background(input: &serde_json::Value) -> bool {
    input
        .get("background")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::plan::StepStatus;
    use serde_json::json;

    #[test]
    fn parse_plan_input_accepts_legacy_payload() {
        let snapshot = parse_plan_input(&json!({
            "explanation": "Legacy explanation",
            "plan": [
                { "step": "inspect", "status": "completed" },
                { "step": "patch", "status": "in_progress" }
            ]
        }));

        assert_eq!(snapshot.explanation.as_deref(), Some("Legacy explanation"));
        assert_eq!(snapshot.items.len(), 2);
        assert_eq!(snapshot.items[0].status, StepStatus::Completed);
        assert_eq!(snapshot.items[1].status, StepStatus::InProgress);
    }

    #[test]
    fn parse_plan_input_extracts_rich_artifact_fields() {
        let snapshot = parse_plan_input(&json!({
            "title": " PlanArtifact ",
            "objective": "Make Plan mode reviewable",
            "context_summary": "Grounded in issue #2691",
            "sources_used": [" gh issue view 2691 ", ""],
            "critical_files": ["crates/tui/src/tools/plan.rs"],
            "constraints": ["No secrets"],
            "recommended_approach": "Enrich update_plan",
            "verification_plan": "Run focused tests",
            "risks_and_unknowns": "Replay may drift",
            "handoff_packet": "Continue with session replay",
            "plan": [
                { "step": " ", "status": "completed" },
                { "step": "render all fields", "status": "weird" }
            ]
        }));

        assert_eq!(snapshot.title.as_deref(), Some("PlanArtifact"));
        assert_eq!(snapshot.sources_used, vec!["gh issue view 2691"]);
        assert_eq!(
            snapshot.critical_files,
            vec!["crates/tui/src/tools/plan.rs"]
        );
        assert_eq!(snapshot.constraints, vec!["No secrets"]);
        assert_eq!(
            snapshot.verification_plan.as_deref(),
            Some("Run focused tests")
        );
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.items[0].step, "render all fields");
        assert_eq!(snapshot.items[0].status, StepStatus::Pending);
    }

    // ── #3031: "(no output)" placeholder must not defeat compact rendering ─

    #[test]
    fn visible_tool_output_maps_no_output_placeholder_to_none() {
        assert_eq!(visible_tool_output("(no output)"), None);
        assert_eq!(visible_tool_output("  (no output)\n"), None);
    }

    #[test]
    fn visible_tool_output_preserves_real_content() {
        assert_eq!(
            visible_tool_output("compiled 3 crates").as_deref(),
            Some("compiled 3 crates")
        );
        // Output that merely CONTAINS the placeholder is real output.
        assert_eq!(
            visible_tool_output("step 1: (no output) — continuing").as_deref(),
            Some("step 1: (no output) — continuing")
        );
        assert_eq!(visible_tool_output("").as_deref(), Some(""));
    }

    #[test]
    fn exec_cell_without_output_suppresses_placeholder_in_live_mode() {
        use crate::tui::history::{ExecCell, ExecSource, ToolCell, ToolStatus};

        let cell = ToolCell::Exec(ExecCell {
            command: "true".to_string(),
            status: ToolStatus::Success,
            output: None,
            live_output: None,
            shell_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
            started_at: None,
            duration_ms: Some(120),
            source: ExecSource::Assistant,
            interaction: None,
            output_summary: None,
        });

        let live: String = cell
            .lines(80)
            .iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(
            !live.contains("(no output)"),
            "Live mode must suppress the placeholder: {live:?}"
        );

        let transcript: String = cell
            .transcript_lines(80)
            .iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(
            transcript.contains("(no output)"),
            "Transcript mode still records the placeholder: {transcript:?}"
        );
    }
}
