//! Activity Detail, raw tool-detail, and pager-text helpers extracted from
//! `ui.rs` (issue #4103).
//!
//! Behavior-preserving move: these helpers build the Ctrl+O "Activity Detail" /
//! "Reasoning Timeline" pager, the `v` raw tool-details pager (including #500
//! spillover folding), the copy-cell actions, and the footer detail labels.
//! No logic changes were made during the extraction.

use crate::snapshot::SnapshotRepo;
use crate::tui::app::App;
use crate::tui::footer_ui::one_line_summary;
use crate::tui::history::{HistoryCell, ToolCell, ToolStatus};
use crate::tui::key_shortcuts;
use crate::tui::pager::PagerView;
use crate::tui::ui_text::{history_cell_to_text, truncate_line_to_width};
// Only the test-gated single-cell Activity Detail renderer needs these
// (Ctrl+O now opens the Turn Inspector, #4104).
#[cfg(test)]
use crate::tui::history::TranscriptRenderOptions;
#[cfg(test)]
use crate::tui::ui_text::line_to_plain;

/// Open a pager for the activity the user is most likely asking about.
///
/// Ctrl+O uses this path. It prefers an explicitly selected activity cell,
/// then a live activity in the current turn, then the most recent meaningful
/// activity across history + active cells. Tool activity is intentionally
/// rendered through the compact live view so Activity Detail does not become
/// an accidental raw-output dump; `v` remains the direct full tool-detail
/// surface.
///
/// Ctrl+O now opens the whole-turn Turn Inspector (#4104), so this single-cell
/// pager and its private helper chain are retained for tests (and potential
/// reuse by the #4106/#4107/#4108 follow-ups) but are no longer bound to a key.
#[cfg(test)]
pub(super) fn open_activity_detail_pager(app: &mut App) -> bool {
    let Some(idx) = activity_target_cell_index(app) else {
        app.status_message = Some("No activity detail available".to_string());
        return true;
    };

    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80);
    let Some(text) = activity_detail_text(app, idx, width) else {
        app.status_message = Some("No activity detail available".to_string());
        return true;
    };
    let title = if matches!(
        app.cell_at_virtual_index(idx),
        Some(HistoryCell::Thinking { .. })
    ) {
        "Reasoning Timeline"
    } else {
        "Activity Detail"
    };
    app.view_stack
        .push(PagerView::from_text(title, &text, width.saturating_sub(2)));
    true
}

fn activity_target_cell_index(app: &App) -> Option<usize> {
    if let Some(selected) = selected_transcript_cell_index(app)
        && app
            .cell_at_virtual_index(selected)
            .is_some_and(is_meaningful_activity_cell)
    {
        return Some(selected);
    }

    current_activity_cell_index(app).or_else(|| {
        (0..app.virtual_cell_count()).rev().find(|&idx| {
            app.cell_at_virtual_index(idx)
                .is_some_and(is_meaningful_activity_cell)
        })
    })
}

fn selected_transcript_cell_index(app: &App) -> Option<usize> {
    app.viewport
        .transcript_selection
        .ordered_endpoints()
        .and_then(|(start, _)| {
            app.viewport
                .transcript_cache
                .line_meta()
                .get(start.line_index)
                .and_then(|meta| meta.cell_line())
                .map(|(cell_index, _)| app.original_cell_index_for_rendered(cell_index))
        })
}

fn current_activity_cell_index(app: &App) -> Option<usize> {
    let active = app.active_cell.as_ref()?;
    let base = app.history.len();
    for desired_rank in [0, 1, 2] {
        if let Some((entry_idx, _)) = active
            .entries()
            .iter()
            .enumerate()
            .rev()
            .find(|(_, cell)| activity_cell_rank(cell) == Some(desired_rank))
        {
            return Some(base + entry_idx);
        }
    }
    None
}

fn is_meaningful_activity_cell(cell: &HistoryCell) -> bool {
    activity_cell_rank(cell).is_some()
}

fn activity_cell_rank(cell: &HistoryCell) -> Option<u8> {
    match cell {
        HistoryCell::Thinking {
            streaming: true, ..
        } => Some(0),
        HistoryCell::Tool(tool) => match tool_status_for_activity(tool) {
            Some(ToolStatus::Running) => Some(0),
            Some(ToolStatus::Failed) => Some(1),
            Some(ToolStatus::Hydrated) => Some(2),
            Some(ToolStatus::Success) => Some(2),
            None => Some(2),
        },
        HistoryCell::SubAgent(_) => Some(0),
        HistoryCell::Error { .. } => Some(1),
        HistoryCell::Thinking { .. } => Some(2),
        _ => None,
    }
}

#[cfg(test)]
fn activity_detail_text(app: &App, cell_index: usize, width: u16) -> Option<String> {
    let cell = app.cell_at_virtual_index(cell_index)?;
    if matches!(cell, HistoryCell::Thinking { .. }) {
        return reasoning_timeline_text(app, cell_index);
    }

    let mut sections = Vec::new();

    if let Some(turn_id) = app.runtime_turn_id.as_ref() {
        let status = humanized_turn_status(app);
        sections.push(format!("Turn {} \u{00B7} {status}", short_turn_id(turn_id)));
    }

    sections.push(format!(
        "Activity: {}",
        activity_cell_label(app, cell_index, cell)
    ));

    if let Some(status) = activity_status_line(cell) {
        sections.push(status);
    }

    let activity_indices = activity_indices(app);
    if let Some(position) = activity_indices.iter().position(|&idx| idx == cell_index) {
        sections.push(format!(
            "Activity chunk: {} of {}",
            position + 1,
            activity_indices.len()
        ));
        sections.extend(activity_navigation_lines(app, position, &activity_indices));
    }

    if let Some(handle) = activity_detail_handle_line(app, cell_index, cell) {
        sections.push(handle);
    }
    if let Some(summary) = activity_input_summary_line(cell) {
        sections.push(summary);
    }

    sections.push(String::new());
    sections.push(activity_cell_to_text(cell, width));
    Some(sections.join("\n"))
}

#[cfg(test)]
fn reasoning_timeline_text(app: &App, selected_cell_index: usize) -> Option<String> {
    let thinking_indices: Vec<usize> = (0..app.virtual_cell_count())
        .filter(|&idx| {
            matches!(
                app.cell_at_virtual_index(idx),
                Some(HistoryCell::Thinking { .. })
            )
        })
        .collect();
    if thinking_indices.is_empty() {
        return None;
    }

    let selected_position = thinking_indices
        .iter()
        .position(|&idx| idx == selected_cell_index)
        .map(|idx| idx + 1);
    let total = thinking_indices.len();
    let running = thinking_indices.iter().any(|&idx| {
        matches!(
            app.cell_at_virtual_index(idx),
            Some(HistoryCell::Thinking {
                streaming: true,
                ..
            })
        )
    });

    let mut sections = Vec::new();
    if let Some(turn_id) = app.runtime_turn_id.as_ref() {
        let status = humanized_turn_status(app);
        sections.push(format!("Turn {} \u{00B7} {status}", short_turn_id(turn_id)));
    }
    sections.push("Activity: reasoning timeline".to_string());
    sections.push(format!(
        "Status: {} · {total} chunk{}",
        if running { "running" } else { "done" },
        if total == 1 { "" } else { "s" }
    ));
    if let Some(position) = selected_position {
        sections.push(format!("Selected chunk: {position} of {total}"));
        if position > 1 {
            let previous_index = thinking_indices[position - 2];
            let preview = thinking_chunk_preview(app, previous_index);
            sections.push(format!(
                "Previous chunk: {} of {total} - {preview}",
                position - 1
            ));
        }
        if position < total {
            let next_index = thinking_indices[position];
            let preview = thinking_chunk_preview(app, next_index);
            sections.push(format!(
                "Next chunk: {} of {total} - {preview}",
                position + 1
            ));
        }
    }
    sections.push(String::new());

    for (position, cell_index) in thinking_indices.iter().copied().enumerate() {
        let Some(HistoryCell::Thinking {
            content,
            streaming,
            duration_secs,
        }) = app.cell_at_virtual_index(cell_index)
        else {
            continue;
        };
        let position = position + 1;
        let marker = if Some(position) == selected_position {
            " (selected)"
        } else {
            ""
        };
        let mut status = if *streaming {
            "running".to_string()
        } else {
            "done".to_string()
        };
        if let Some(duration_secs) = duration_secs {
            status.push_str(" · ");
            status.push_str(&format!("{duration_secs:.1}s"));
        }
        sections.push(format!("Thinking chunk {position} of {total}{marker}"));
        sections.push(format!("Status: {status}"));
        let body = content.trim();
        if body.is_empty() {
            sections.push("(no reasoning text recorded)".to_string());
        } else {
            sections.push(body.to_string());
        }
        sections.push(String::new());
    }

    Some(sections.join("\n"))
}

#[cfg(test)]
fn thinking_chunk_preview(app: &App, cell_index: usize) -> String {
    let Some(HistoryCell::Thinking { content, .. }) = app.cell_at_virtual_index(cell_index) else {
        return "thinking".to_string();
    };
    let preview = one_line_summary(content, 64);
    if preview.is_empty() {
        "thinking".to_string()
    } else {
        preview
    }
}

fn activity_cell_label(app: &App, cell_index: usize, cell: &HistoryCell) -> String {
    match cell {
        HistoryCell::Thinking { .. } => "thinking".to_string(),
        HistoryCell::Error { .. } => "error".to_string(),
        HistoryCell::SubAgent(_) => "sub-agent".to_string(),
        HistoryCell::Tool(ToolCell::Generic(generic)) => {
            crate::tui::widgets::tool_card::tool_activity_label_for_name(
                &generic.name,
                app.ui_locale,
            )
        }
        HistoryCell::Tool(_) => {
            detail_target_label(app, cell_index).unwrap_or_else(|| "tool activity".to_string())
        }
        _ => "message".to_string(),
    }
}

#[cfg(test)]
fn activity_status_line(cell: &HistoryCell) -> Option<String> {
    match cell {
        HistoryCell::Thinking {
            streaming,
            duration_secs,
            ..
        } => {
            let mut line = if *streaming {
                "Status: running".to_string()
            } else {
                "Status: done".to_string()
            };
            if let Some(duration_secs) = duration_secs {
                line.push_str(" · ");
                line.push_str(&format!("{duration_secs:.1}s"));
            }
            Some(line)
        }
        HistoryCell::Tool(tool) => {
            let status = tool_status_for_activity(tool)?;
            let mut line = format!("Status: {}", activity_status_label(status));
            if let Some(duration_ms) = tool_duration_for_activity(tool) {
                line.push_str(" · ");
                line.push_str(&format_activity_duration_ms(duration_ms));
            }
            Some(line)
        }
        HistoryCell::Error { severity, .. } => Some(format!("Status: {severity:?}")),
        HistoryCell::SubAgent(_) => None,
        _ => None,
    }
}

fn tool_status_for_activity(tool: &ToolCell) -> Option<ToolStatus> {
    match tool {
        ToolCell::Exec(cell) => Some(cell.status),
        ToolCell::Exploring(cell) => {
            if cell
                .entries
                .iter()
                .any(|entry| entry.status == ToolStatus::Running)
            {
                Some(ToolStatus::Running)
            } else if cell
                .entries
                .iter()
                .any(|entry| entry.status == ToolStatus::Failed)
            {
                Some(ToolStatus::Failed)
            } else if cell
                .entries
                .iter()
                .any(|entry| entry.status == ToolStatus::Hydrated)
            {
                Some(ToolStatus::Hydrated)
            } else {
                Some(ToolStatus::Success)
            }
        }
        ToolCell::PlanUpdate(cell) => Some(cell.status),
        ToolCell::PatchSummary(cell) => Some(cell.status),
        ToolCell::Review(cell) => Some(cell.status),
        ToolCell::DiffPreview(_) => Some(ToolStatus::Success),
        ToolCell::Mcp(cell) => Some(cell.status),
        ToolCell::ViewImage(_) => Some(ToolStatus::Success),
        ToolCell::WebSearch(cell) => Some(cell.status),
        ToolCell::Generic(cell) => Some(cell.status),
    }
}

fn tool_duration_for_activity(tool: &ToolCell) -> Option<u64> {
    match tool {
        ToolCell::Exec(cell) => cell.duration_ms.or_else(|| {
            (cell.status == ToolStatus::Running).then(|| {
                u64::try_from(
                    cell.started_at
                        .map(|started| started.elapsed().as_millis())
                        .unwrap_or_default(),
                )
                .unwrap_or(u64::MAX)
            })
        }),
        _ => None,
    }
}

fn activity_status_label(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Running => "running",
        ToolStatus::Success => "done",
        ToolStatus::Hydrated => "tool loaded - retry required",
        ToolStatus::Failed => "failed",
    }
}

fn format_activity_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

#[cfg(test)]
fn activity_indices(app: &App) -> Vec<usize> {
    (0..app.virtual_cell_count())
        .filter(|&idx| {
            app.cell_at_virtual_index(idx)
                .is_some_and(is_meaningful_activity_cell)
        })
        .collect()
}

#[cfg(test)]
fn activity_navigation_lines(
    app: &App,
    position: usize,
    activity_indices: &[usize],
) -> Vec<String> {
    let total = activity_indices.len();
    let mut lines = Vec::new();
    if position > 0 {
        let previous_idx = activity_indices[position - 1];
        if let Some(cell) = app.cell_at_virtual_index(previous_idx) {
            let label = activity_cell_label(app, previous_idx, cell);
            lines.push(format!(
                "Previous activity: {} of {total} - {}",
                position,
                truncate_line_to_width(&label, 56)
            ));
        }
    }
    if position + 1 < total {
        let next_idx = activity_indices[position + 1];
        if let Some(cell) = app.cell_at_virtual_index(next_idx) {
            let label = activity_cell_label(app, next_idx, cell);
            lines.push(format!(
                "Next activity: {} of {total} - {}",
                position + 2,
                truncate_line_to_width(&label, 56)
            ));
        }
    }
    lines
}

#[cfg(test)]
fn activity_detail_handle_line(app: &App, cell_index: usize, cell: &HistoryCell) -> Option<String> {
    if let Some(detail) = app.tool_detail_record_for_cell(cell_index) {
        if let Some(artifact) = app
            .session_artifacts
            .iter()
            .find(|artifact| artifact.tool_call_id == detail.tool_id)
        {
            return Some(format!(
                "Detail handle: {} (retrieve_tool_result ref={}; v raw details)",
                artifact.id, artifact.id
            ));
        }
        return Some(format!(
            "Detail handle: tool:{} (v raw details)",
            detail.tool_id
        ));
    }

    match cell {
        HistoryCell::Tool(_) => Some("Detail handle: v details".to_string()),
        HistoryCell::SubAgent(_) => Some("Detail handle: v details".to_string()),
        _ => None,
    }
}

#[cfg(test)]
fn activity_input_summary_line(cell: &HistoryCell) -> Option<String> {
    let HistoryCell::Tool(ToolCell::Generic(generic)) = cell else {
        return None;
    };
    let summary = generic.input_summary.as_deref()?.trim();
    if summary.is_empty() {
        None
    } else {
        Some(format!("Input: {summary}"))
    }
}

#[cfg(test)]
fn activity_cell_to_text(cell: &HistoryCell, width: u16) -> String {
    let lines = match cell {
        HistoryCell::Tool(_) => cell.lines_with_options(
            width,
            TranscriptRenderOptions {
                calm_mode: true,
                low_motion: true,
                ..TranscriptRenderOptions::default()
            },
        ),
        _ => cell.transcript_lines(width),
    };
    lines
        .iter()
        .map(line_to_plain)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Empty-state hint shown when the selection has no raw leaf detail to open.
/// `v` / `Alt+V` only ever surface the raw detail of the ONE selected
/// tool/card/leaf, so when there is nothing leaf-level to show we point the
/// user at Ctrl+O for the whole-turn context instead of failing silently
/// (#4105).
const NO_RAW_DETAIL_HINT: &str =
    "No raw detail for this item — press Ctrl+O for the turn overview.";

/// Intro line prepended to the raw tool-detail pager body so the surface reads
/// as the raw detail of the single selected item — not the whole turn. Ctrl+O
/// remains the whole-turn Turn Inspector (#4105).
const RAW_DETAIL_PAGER_INTRO: &str =
    "Raw detail for the selected item — press Ctrl+O for the whole-turn overview.";

pub(super) fn open_tool_details_pager(app: &mut App) -> bool {
    let target_cell = detail_target_cell_index(app);

    let Some(cell_index) = target_cell else {
        app.status_message = Some(NO_RAW_DETAIL_HINT.to_string());
        return false;
    };
    open_details_pager_for_cell(app, cell_index)
}

/// Build the trailing "Spillover" section for the tool-details pager
/// (#500). Returns `None` when the cell at `cell_index` is not a
/// `GenericToolCell` with a recorded spillover path, or when the
/// spillover file is missing or unreadable. Failures fall back to a
/// short notice in the section so the user understands why the full
/// content can't be loaded — better than silent truncation.
pub(super) fn spillover_pager_section(app: &App, cell_index: usize) -> Option<String> {
    use crate::tui::history::{GenericToolCell, HistoryCell, ToolCell};

    let cell = app.cell_at_virtual_index(cell_index)?;
    let HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
        spillover_path: Some(path),
        ..
    })) = cell
    else {
        return None;
    };
    let path_str = path.display().to_string();
    let body = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => format!("(could not read spillover file: {err})"),
    };
    Some(format!(
        "── Full output (spillover) ──\nFile: {path_str}\n\n{body}"
    ))
}

pub(crate) fn open_details_pager_for_cell(app: &mut App, cell_index: usize) -> bool {
    if let Some(detail) = app.tool_detail_record_for_cell(cell_index) {
        let input = serde_json::to_string_pretty(&detail.input)
            .unwrap_or_else(|_| detail.input.to_string());
        let output = detail.output.as_deref().map_or(
            "(not available)".to_string(),
            std::string::ToString::to_string,
        );

        // #500: when the tool result was spilled to disk, fold the full
        // file content into the pager body so the user can see what was
        // elided (the model only ever saw the head). The truncated head
        // stays above as `Output:` so the user can compare what the
        // model received against the full payload.
        let spillover_section = spillover_pager_section(app, cell_index);

        // Frame the body as leaf-level raw detail for the selected item. The
        // Tool ID / Input / Output / spillover content below is unchanged — only
        // the leading intro line is new, so existing raw-output visibility is
        // preserved (#4105).
        let content = if let Some(section) = spillover_section {
            format!(
                "{RAW_DETAIL_PAGER_INTRO}\n\nTool ID: {}\nTool: {}\n\nInput:\n{}\n\nOutput:\n{}\n\n{}",
                detail.tool_id, detail.tool_name, input, output, section
            )
        } else {
            format!(
                "{RAW_DETAIL_PAGER_INTRO}\n\nTool ID: {}\nTool: {}\n\nInput:\n{}\n\nOutput:\n{}",
                detail.tool_id, detail.tool_name, input, output
            )
        };

        let width = app
            .viewport
            .last_transcript_area
            .map(|area| area.width)
            .unwrap_or(80);
        app.view_stack.push(PagerView::from_text(
            format!("Raw detail — {}", detail.tool_name),
            &content,
            width.saturating_sub(2),
        ));
        return true;
    }

    let Some(cell) = app.cell_at_virtual_index(cell_index) else {
        app.status_message = Some(NO_RAW_DETAIL_HINT.to_string());
        return false;
    };
    let title = match cell {
        HistoryCell::User { .. } => "You".to_string(),
        HistoryCell::Assistant { .. } => "Assistant".to_string(),
        HistoryCell::System { .. } => "Note".to_string(),
        HistoryCell::Error { .. } => "Error".to_string(),
        HistoryCell::Thinking { .. } => "Reasoning".to_string(),
        HistoryCell::Tool(_) => "Message".to_string(),
        HistoryCell::SubAgent(_) => "Sub-agent".to_string(),
        HistoryCell::ArchivedContext { .. } => "Archived Context".to_string(),
    };
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80);
    let content = history_cell_to_text(cell, width);
    app.view_stack.push(PagerView::from_text(
        title,
        &content,
        width.saturating_sub(2),
    ));
    true
}

/// Copy the "focused" transcript cell to the system clipboard.
/// The focused cell is determined by the detail-target heuristic
/// (viewport centre or most recent cell). Returns true when text
/// was actually copied.
pub(super) fn copy_focused_cell(app: &mut App) -> bool {
    let cell_index = detail_target_cell_index(app);
    let Some(index) = cell_index else {
        return false;
    };
    copy_cell_to_clipboard(app, index)
}

pub(crate) fn copy_cell_to_clipboard(app: &mut App, cell_index: usize) -> bool {
    let Some(cell) = app.cell_at_virtual_index(cell_index) else {
        app.status_message = Some("No message at that line".to_string());
        return false;
    };
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80);
    let text = history_cell_to_text(cell, width);
    if text.trim().is_empty() {
        app.status_message = Some("Message is empty".to_string());
        return false;
    }
    if app.clipboard.write_text(&text).is_ok() {
        app.status_message = Some("Message copied".to_string());
        true
    } else {
        app.status_message = Some("Copy failed".to_string());
        false
    }
}

pub(super) fn detail_target_cell_index(app: &App) -> Option<usize> {
    if let Some((start, _)) = app.viewport.transcript_selection.ordered_endpoints() {
        return app
            .viewport
            .transcript_cache
            .line_meta()
            .get(start.line_index)
            .and_then(|meta| meta.cell_line())
            .map(|(cell_index, _)| app.original_cell_index_for_rendered(cell_index));
    }

    app.detail_cell_index_for_viewport(
        app.viewport.last_transcript_top,
        app.viewport.last_transcript_visible.max(1),
        app.viewport.transcript_cache.line_meta(),
    )
    .or_else(|| app.history.len().checked_sub(1))
}

pub(crate) fn selected_detail_footer_label(app: &App) -> Option<String> {
    if app.viewport.transcript_selection.is_active() {
        return None;
    }
    let cell_index = activity_footer_target_cell_index(app)?;
    let cell = app.cell_at_virtual_index(cell_index)?;
    let label = truncate_line_to_width(&activity_cell_label(app, cell_index, cell), 30);
    let detail_hint = if app.cell_has_detail_target(cell_index) {
        let noun = if matches!(cell, HistoryCell::SubAgent(_)) {
            "details"
        } else {
            "raw details"
        };
        format!(
            " · {}",
            key_shortcuts::tool_details_shortcut_action_hint(noun)
        )
    } else {
        String::new()
    };
    Some(format!(
        "{} Turn Inspector · {label}{detail_hint}",
        key_shortcuts::activity_shortcut_label()
    ))
}

fn activity_footer_target_cell_index(app: &App) -> Option<usize> {
    let line_meta = app.viewport.transcript_cache.line_meta();
    let start = app
        .viewport
        .last_transcript_top
        .min(line_meta.len().saturating_sub(1));
    let end = start
        .saturating_add(app.viewport.last_transcript_visible.max(1))
        .min(line_meta.len());
    for meta in line_meta.iter().take(end).skip(start) {
        let Some((cell_index, _)) = meta.cell_line() else {
            continue;
        };
        let cell_index = app.original_cell_index_for_rendered(cell_index);
        if app
            .cell_at_virtual_index(cell_index)
            .is_some_and(is_meaningful_activity_cell)
        {
            return Some(cell_index);
        }
    }

    activity_target_cell_index(app)
}

pub(crate) fn detail_target_label(app: &App, cell_index: usize) -> Option<String> {
    if let Some(detail) = app.tool_detail_record_for_cell(cell_index) {
        return Some(detail.tool_name.clone());
    }
    let cell = app.cell_at_virtual_index(cell_index)?;
    match cell {
        HistoryCell::Tool(ToolCell::Exec(exec)) => {
            Some(format!("run {}", one_line_summary(&exec.command, 80)))
        }
        HistoryCell::Tool(ToolCell::Exploring(explore)) => Some(format!(
            "workspace {} item{}",
            explore.entries.len(),
            if explore.entries.len() == 1 { "" } else { "s" }
        )),
        HistoryCell::Tool(ToolCell::PlanUpdate(_)) => Some("update plan".to_string()),
        HistoryCell::Tool(ToolCell::PatchSummary(patch)) => Some(format!("patch {}", patch.path)),
        HistoryCell::Tool(ToolCell::Review(review)) => {
            let target = one_line_summary(&review.target, 80);
            Some(if target.is_empty() {
                "review".to_string()
            } else {
                format!("review {target}")
            })
        }
        HistoryCell::Tool(ToolCell::DiffPreview(diff)) => Some(format!("diff {}", diff.title)),
        HistoryCell::Tool(ToolCell::Mcp(mcp)) => Some(format!("tool {}", mcp.tool)),
        HistoryCell::Tool(ToolCell::ViewImage(image)) => {
            Some(format!("image {}", image.path.display()))
        }
        HistoryCell::Tool(ToolCell::WebSearch(search)) => Some(format!("search {}", search.query)),
        HistoryCell::Tool(ToolCell::Generic(generic)) => Some(
            crate::tui::widgets::tool_card::tool_activity_label_for_name(
                &generic.name,
                app.ui_locale,
            ),
        ),
        HistoryCell::SubAgent(_) => Some("sub-agent".to_string()),
        _ => None,
    }
}

pub(super) fn extract_reasoning_header(text: &str) -> Option<String> {
    let start = text.find("**")?;
    let rest = &text[start + 2..];
    let end = rest.find("**")?;
    let header = rest[..end].trim().trim_end_matches(':');
    if header.is_empty() {
        None
    } else {
        Some(header.to_string())
    }
}

// ============================================================================
// Turn Inspector (issue #4104)
//
// Ctrl+O opens a *turn-level* overview of the current in-flight turn — or the
// latest completed turn when idle — rather than the single-cell Activity
// Detail. `v` / `Alt+V` remain the raw leaf-detail command for the selected
// item; this surface never dumps a single tool's raw output.
//
// Each of the nine overview sections renders from whatever turn/cell/app state
// is cleanly reachable and DEGRADES the rest gracefully to a short "none"/"—"
// line — never a mysterious blank. The thinner sections (diagnostics loop,
// tests/verifier) are intentionally heuristic in this first pass; the leaf
// issues #4106/#4107/#4108 flesh them out with structured data later.
// ============================================================================

/// Open the whole-turn Turn Inspector pager (Ctrl+O).
///
/// Reuses the same `PagerView` text-section machinery as the Activity Detail
/// pager — no new modal system. Always succeeds: an empty transcript still
/// yields a coherent (degraded) overview rather than a dead keypress.
pub(super) fn open_turn_inspector_pager(app: &mut App) -> bool {
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80);
    let text = turn_inspector_text(app);
    // Precompute the compact Markdown handoff (#4108) and attach it so the
    // pager's `e` key can copy a pasteable artifact without reaching back into
    // `app`. Reuses the same turn scope + section data as the overview above.
    let handoff = turn_handoff_markdown(app);
    app.view_stack.push(
        PagerView::from_text("Turn Inspector", &text, width.saturating_sub(2))
            .with_export_markdown(handoff),
    );
    true
}

/// Virtual-cell range `[start, end)` of the turn under inspection.
///
/// The turn is the run of cells from the last user prompt through the end of
/// the transcript. Because `virtual_cell_count()` includes still-in-flight
/// `active_cell` entries, this scopes to the current in-flight turn during a
/// turn, and to the latest completed turn once the active cell has flushed to
/// history. When no user prompt exists yet the whole transcript is used.
fn current_turn_range(app: &App) -> (usize, usize) {
    let end = app.virtual_cell_count();
    let start = (0..end)
        .rev()
        .find(|&idx| {
            matches!(
                app.cell_at_virtual_index(idx),
                Some(HistoryCell::User { .. })
            )
        })
        .unwrap_or(0);
    (start, end)
}

/// Human form of the runtime turn status — raw enum-ish values like
/// "in_progress" must never reach the inspector (dogfood A6, #4102).
fn humanized_turn_status(app: &App) -> &str {
    match app.runtime_turn_status.as_deref() {
        Some("in_progress") | None => "in progress",
        Some(other) => other,
    }
}

/// Short display form of a runtime turn id. The full UUID reads as internal
/// state in the inspector header (dogfood A6); twelve characters is plenty
/// to correlate with logs.
fn short_turn_id(turn_id: &str) -> &str {
    turn_id.get(..12).unwrap_or(turn_id)
}

/// Assemble the Turn Inspector overview text from all available turn data.
pub(super) fn turn_inspector_text(app: &App) -> String {
    let (start, end) = current_turn_range(app);
    let mut out: Vec<String> = Vec::new();

    // Turn identity header. Lead with the human turn number and status; the
    // id is a short correlation suffix, never a raw UUID dump (dogfood A6).
    let status = humanized_turn_status(app);
    if app.turn_counter > 0 {
        let mut line = format!("Turn #{} \u{00B7} {status}", app.turn_counter);
        if let Some(turn_id) = app.runtime_turn_id.as_ref() {
            line.push_str(&format!(" \u{00B7} id {}", short_turn_id(turn_id)));
        }
        out.push(line);
    } else if let Some(turn_id) = app.runtime_turn_id.as_ref() {
        out.push(format!("Turn {} \u{00B7} {status}", short_turn_id(turn_id)));
    } else {
        out.push("Turn: \u{2014} (no turn recorded yet)".to_string());
    }
    // Restate the Ctrl+O (overview) vs. `v` (raw leaf detail) contract so the
    // two surfaces never get confused.
    out.push(
        "Overview of the current/latest turn · press v for the selected item's raw detail"
            .to_string(),
    );

    push_section(&mut out, "Intent", vec![turn_intent_line(app, start)]);

    if let Some(line) = selected_item_context_line(app) {
        push_section(&mut out, "Selected item", vec![line]);
    }

    push_section(&mut out, "Plan / checklist", turn_plan_lines(app));
    push_section(
        &mut out,
        "Turn timeline",
        turn_timeline_lines(app, start, end),
    );
    push_section(
        &mut out,
        "Files changed",
        turn_files_changed(app, start, end),
    );
    push_section(&mut out, "Diagnostics loop", turn_diagnostics_lines(app));
    push_section(
        &mut out,
        "Tests / verifier",
        turn_verifier_lines(app, start, end),
    );
    push_section(&mut out, "Approvals / denials", turn_approvals_lines(app));
    push_section(&mut out, "Model route + tokens/cost", turn_route_lines(app));
    push_section(
        &mut out,
        "Final result / status",
        turn_result_lines(app, start, end),
    );

    out.join("\n")
}

/// Build a compact, pasteable Markdown handoff of the current/latest turn
/// (issue #4108).
///
/// Reuses the exact same turn scope (`current_turn_range`) and the same
/// per-section data helpers as the Turn Inspector (#4104), so the handoff can
/// never drift from what Ctrl+O shows — it only re-renders that data as
/// Markdown headings + bullets instead of the inspector's box-drawn rules.
/// Unavailable sections degrade to a short `—` (and the optional Plan section
/// is dropped entirely when empty) so the artifact stays paste-ready without
/// leaving a heading over a blank void — the same graceful-degrade contract the
/// inspector already follows.
pub(crate) fn turn_handoff_markdown(app: &App) -> String {
    let (start, end) = current_turn_range(app);
    let mut out: Vec<String> = Vec::new();

    // Title + identity — turn id when known, else the turn counter, else a
    // bare heading so an empty transcript still yields a coherent artifact.
    let heading = if app.turn_counter > 0 {
        format!("# Turn handoff — Turn #{}", app.turn_counter)
    } else if let Some(turn_id) = app.runtime_turn_id.as_ref() {
        format!("# Turn handoff — {}", short_turn_id(turn_id))
    } else {
        "# Turn handoff".to_string()
    };
    out.push(heading);

    let status = match app.runtime_turn_status.as_deref() {
        Some("in_progress") => "in progress",
        Some(other) => other,
        None => "idle",
    };
    out.push(format!(
        "_Status: {status} · generated {}_",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ));

    push_md_section(&mut out, "Intent", vec![turn_intent_line(app, start)]);

    // Plan/checklist is optional context: include it only when a plan or todo
    // list actually ran, to keep the handoff compact.
    let plan = turn_plan_lines(app);
    if !plan.is_empty() {
        push_md_section(&mut out, "Plan / checklist", md_bullets(plan));
    }

    push_md_section(
        &mut out,
        "Files changed",
        md_bullets(turn_files_changed(app, start, end)),
    );
    push_md_section(
        &mut out,
        "Turn timeline",
        md_bullets(turn_timeline_lines(app, start, end)),
    );
    push_md_section(
        &mut out,
        "Tests / verifier",
        md_bullets(turn_verifier_lines(app, start, end)),
    );
    push_md_section(
        &mut out,
        "Model route + tokens/cost",
        md_bullets(turn_route_lines(app)),
    );
    push_md_section(
        &mut out,
        "Result / status",
        md_bullets(turn_result_lines(app, start, end)),
    );

    // Trailing newline keeps the artifact clean when pasted into a PR body.
    out.push(String::new());
    out.join("\n")
}

/// Append a `## Title` Markdown section. An empty body degrades to a single
/// `—` line so a heading is never followed by a void — the Markdown analogue
/// of [`push_section`]'s `none` degrade.
fn push_md_section(out: &mut Vec<String>, title: &str, body: Vec<String>) {
    out.push(String::new());
    out.push(format!("## {title}"));
    if body.is_empty() {
        out.push("—".to_string());
    } else {
        out.extend(body);
    }
}

/// Convert Turn Inspector section lines into Markdown bullet rows. Inspector
/// list helpers prefix rows with `• `; swap that for `- `, and bullet the
/// key/value rows (route, tokens, status) too so the whole section is valid
/// Markdown.
fn md_bullets(lines: Vec<String>) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| {
            let body = line.strip_prefix("• ").unwrap_or(line.as_str());
            format!("- {body}")
        })
        .collect()
}

/// Append a `── Title ──` section. An empty body degrades to a single
/// `none` line so the section header is never followed by a blank void.
fn push_section(out: &mut Vec<String>, title: &str, body: Vec<String>) {
    out.push(String::new());
    out.push(format!("── {title} ──"));
    if body.is_empty() {
        out.push("none".to_string());
    } else {
        out.extend(body);
    }
}

/// Section 1 — intent / user-prompt summary for the turn.
fn turn_intent_line(app: &App, start: usize) -> String {
    if let Some(HistoryCell::User { content }) = app.cell_at_virtual_index(start) {
        let summary = one_line_summary(content, 240);
        if !summary.is_empty() {
            return summary;
        }
    }
    if let Some(prompt) = app.last_submitted_prompt.as_deref() {
        let summary = one_line_summary(prompt, 240);
        if !summary.is_empty() {
            return summary;
        }
    }
    "—".to_string()
}

/// Optional selected-item context. The first view is the turn overview, but
/// when the user has an activity cell selected we surface it plus the `v`
/// affordance so the Ctrl+O / `v` split stays discoverable.
fn selected_item_context_line(app: &App) -> Option<String> {
    let idx = selected_transcript_cell_index(app)?;
    let cell = app.cell_at_virtual_index(idx)?;
    let label = truncate_line_to_width(&activity_cell_label(app, idx, cell), 48);
    let hint = if app.cell_has_detail_target(idx) {
        " · v opens its raw detail"
    } else {
        ""
    };
    Some(format!("{label}{hint}"))
}

/// Section 2 — plan and/or checklist state, when a plan/todo tool has run.
fn turn_plan_lines(app: &App) -> Vec<String> {
    let mut lines = Vec::new();

    if let Ok(plan) = app.plan_state.try_lock()
        && !plan.is_empty()
    {
        let snapshot = plan.snapshot();
        let headline = snapshot
            .title
            .as_deref()
            .or(snapshot.objective.as_deref())
            .map(str::trim)
            .filter(|s: &&str| !s.is_empty());
        if let Some(headline) = headline {
            lines.push(format!("Plan: {}", truncate_line_to_width(headline, 64)));
        }
        let (pending, in_progress, completed) = plan.counts();
        let total = pending + in_progress + completed;
        if total > 0 {
            lines.push(format!(
                "Steps: {completed}/{total} done ({}%)",
                plan.progress_percent()
            ));
        }
        for item in &snapshot.items {
            lines.push(format!(
                "{} {}",
                step_status_glyph(&item.status),
                truncate_line_to_width(&item.step, 72)
            ));
        }
    }

    if let Ok(todos) = app.todos.try_lock() {
        let snapshot = todos.snapshot();
        if !snapshot.items.is_empty() {
            lines.push(format!("Checklist: {}% complete", snapshot.completion_pct));
            for item in &snapshot.items {
                lines.push(format!(
                    "{} {}",
                    todo_status_glyph(&item.status),
                    truncate_line_to_width(&item.content, 72)
                ));
            }
        }
    }

    lines
}

fn step_status_glyph(status: &crate::tools::plan::StepStatus) -> &'static str {
    match status {
        crate::tools::plan::StepStatus::Completed => "[x]",
        crate::tools::plan::StepStatus::InProgress => "[~]",
        crate::tools::plan::StepStatus::Pending => "[ ]",
    }
}

fn todo_status_glyph(status: &crate::tools::todo::TodoStatus) -> &'static str {
    match status {
        crate::tools::todo::TodoStatus::Completed => "[x]",
        crate::tools::todo::TodoStatus::InProgress => "[~]",
        crate::tools::todo::TodoStatus::Pending => "[ ]",
    }
}

/// Section 3 — chronological turn timeline with compact action affordances.
fn turn_timeline_lines(app: &App, start: usize, end: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for idx in start..end {
        let Some(cell) = app.cell_at_virtual_index(idx) else {
            continue;
        };
        match cell {
            HistoryCell::User { content } => {
                let summary = one_line_summary(content, 96);
                rows.push(timeline_row("user prompt", &summary, None, None, &[]));
            }
            HistoryCell::Thinking {
                content,
                streaming,
                duration_secs,
            } => {
                let summary = one_line_summary(content, 88);
                let status = streaming.then_some("running").unwrap_or("done");
                let duration = duration_secs.map(|secs| format!("{secs:.1}s"));
                let actions = timeline_cell_actions(app, idx, cell);
                rows.push(timeline_row(
                    "reasoning",
                    &summary,
                    Some(status),
                    duration.as_deref(),
                    &actions,
                ));
            }
            HistoryCell::Tool(tool) => {
                let (kind, summary) = timeline_tool_summary(app, idx, tool);
                let duration = tool_duration_for_activity(tool).map(format_activity_duration_ms);
                let status = tool_status_for_activity(tool).map(activity_status_label);
                let actions = timeline_cell_actions(app, idx, cell);
                rows.push(timeline_row(
                    kind,
                    &summary,
                    status,
                    duration.as_deref(),
                    &actions,
                ));
            }
            HistoryCell::SubAgent(_) => {
                let summary = detail_target_label(app, idx).unwrap_or_else(|| "sub-agent".into());
                let actions = timeline_cell_actions(app, idx, cell);
                rows.push(timeline_row("sub-agent", &summary, None, None, &actions));
            }
            HistoryCell::Assistant { content, streaming } => {
                let summary = one_line_summary(content, 96);
                let status = streaming.then_some("streaming").unwrap_or("done");
                rows.push(timeline_row(
                    "assistant result",
                    &summary,
                    Some(status),
                    None,
                    &[],
                ));
            }
            HistoryCell::Error { message, severity } => {
                let summary = one_line_summary(message, 96);
                let status = severity.to_string();
                rows.push(timeline_row("error", &summary, Some(&status), None, &[]));
            }
            HistoryCell::System { content }
            | HistoryCell::ArchivedContext {
                summary: content, ..
            } => {
                let summary = one_line_summary(content, 96);
                rows.push(timeline_row("system note", &summary, None, None, &[]));
            }
        }
    }
    rows.push(turn_checkpoint_timeline_row(app));
    rows.into_iter()
        .enumerate()
        .map(|(idx, row)| format!("{}. {row}", idx + 1))
        .collect()
}

fn timeline_tool_summary(app: &App, idx: usize, tool: &ToolCell) -> (&'static str, String) {
    match tool {
        ToolCell::Exec(exec) if command_looks_like_verifier(&exec.command) => {
            ("test/verifier", truncate_line_to_width(&exec.command, 88))
        }
        ToolCell::Exec(exec) => ("shell command", truncate_line_to_width(&exec.command, 88)),
        ToolCell::Exploring(explore) => (
            "read/search",
            format!(
                "{} item{}",
                explore.entries.len(),
                if explore.entries.len() == 1 { "" } else { "s" }
            ),
        ),
        ToolCell::PlanUpdate(_) => ("plan update", "plan/checklist changed".to_string()),
        ToolCell::PatchSummary(patch) => {
            let summary = one_line_summary(&patch.summary, 72);
            if summary.is_empty() {
                ("edit", truncate_line_to_width(&patch.path, 88))
            } else {
                (
                    "edit",
                    truncate_line_to_width(&format!("{} — {summary}", patch.path), 88),
                )
            }
        }
        ToolCell::Review(review) => {
            let target = one_line_summary(&review.target, 88);
            (
                "review",
                if target.is_empty() {
                    "code review".to_string()
                } else {
                    target
                },
            )
        }
        ToolCell::DiffPreview(diff) => ("diff", truncate_line_to_width(&diff.title, 88)),
        ToolCell::Mcp(mcp) => ("MCP tool", truncate_line_to_width(&mcp.tool, 88)),
        ToolCell::ViewImage(image) => (
            "image",
            truncate_line_to_width(&image.path.display().to_string(), 88),
        ),
        ToolCell::WebSearch(search) => ("web search", truncate_line_to_width(&search.query, 88)),
        ToolCell::Generic(generic) => {
            let mut label =
                detail_target_label(app, idx).unwrap_or_else(|| generic.name.replace('_', " "));
            if let Some(input) = generic.input_summary.as_deref().map(str::trim)
                && !input.is_empty()
            {
                label.push_str(" · ");
                label.push_str(input);
            }
            (
                generic_tool_timeline_kind(generic),
                truncate_line_to_width(&label, 88),
            )
        }
    }
}

fn generic_tool_timeline_kind(generic: &crate::tui::history::GenericToolCell) -> &'static str {
    let name = generic.name.as_str();
    if generic.is_diff || name.contains("diff") {
        "diff"
    } else if matches!(name, "read_file" | "list_files" | "glob" | "grep_files")
        || name.contains("read")
        || name.contains("search")
        || name.contains("grep")
    {
        "read/search"
    } else if matches!(name, "apply_patch" | "edit_file" | "write_file")
        || name.contains("patch")
        || name.contains("edit")
        || name.contains("write")
    {
        "edit"
    } else if name.contains("approval") {
        "approval"
    } else if name.contains("diagnostic") || name.contains("lsp") {
        "diagnostics"
    } else {
        "tool"
    }
}

fn timeline_cell_actions(app: &App, idx: usize, cell: &HistoryCell) -> Vec<&'static str> {
    let mut actions = Vec::new();
    if app.cell_has_detail_target(idx) {
        actions.push("v raw detail");
    }
    match cell {
        HistoryCell::Tool(ToolCell::DiffPreview(_)) => actions.push("d diff"),
        HistoryCell::Tool(ToolCell::PatchSummary(_)) => actions.push("d diff"),
        HistoryCell::Tool(ToolCell::Generic(generic)) if generic.is_diff => actions.push("d diff"),
        _ => {}
    }
    actions
}

fn timeline_row(
    kind: &str,
    summary: &str,
    status: Option<&str>,
    duration: Option<&str>,
    actions: &[&str],
) -> String {
    let mut line = if summary.trim().is_empty() {
        kind.to_string()
    } else {
        format!("{kind}: {}", summary.trim())
    };
    if let Some(status) = status.filter(|s| !s.trim().is_empty()) {
        line.push_str(" — ");
        line.push_str(status);
    }
    if let Some(duration) = duration.filter(|s| !s.trim().is_empty()) {
        line.push_str(" · ");
        line.push_str(duration);
    }
    if !actions.is_empty() {
        line.push_str(" · actions: ");
        line.push_str(&actions.join(", "));
    }
    line
}

fn turn_checkpoint_timeline_row(app: &App) -> String {
    if app.turn_counter == 0 {
        return "checkpoint: unavailable — no numbered turn snapshot yet · action: e export handoff"
            .to_string();
    }

    let repo = match SnapshotRepo::open_existing(&app.workspace) {
        Ok(Some(repo)) => repo,
        Ok(None) => {
            return "checkpoint: unavailable — no snapshot repo found · action: e export handoff"
                .to_string();
        }
        Err(err) => {
            return format!(
                "checkpoint: unknown — snapshot repo could not be opened ({}) · action: e export handoff",
                truncate_line_to_width(&err.to_string(), 72)
            );
        }
    };
    let snapshots = match repo.list(20) {
        Ok(snapshots) => snapshots,
        Err(err) => {
            return format!(
                "checkpoint: unknown — snapshot list failed ({}) · action: e export handoff",
                truncate_line_to_width(&err.to_string(), 72)
            );
        }
    };
    let prefix = format!("pre-turn:{}", app.turn_counter);
    let matching = snapshots
        .iter()
        .find(|snapshot| {
            snapshot.label == prefix || snapshot.label.starts_with(&format!("{prefix}:"))
        })
        .or_else(|| {
            snapshots
                .iter()
                .find(|snapshot| snapshot.label.starts_with("pre-turn:"))
        });
    if let Some(snapshot) = matching {
        let short = &snapshot.id.as_str()[..snapshot.id.as_str().len().min(8)];
        format!(
            "checkpoint: {} ({short}) available · actions: r restore via /restore (guarded), e export handoff",
            truncate_line_to_width(&snapshot.label, 72)
        )
    } else {
        "checkpoint: unavailable — no pre-turn snapshot found · action: e export handoff"
            .to_string()
    }
}

/// Section 4 — files touched by patch/diff tool cells in the turn.
fn turn_files_changed(app: &App, start: usize, end: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for idx in start..end {
        let Some(HistoryCell::Tool(tool)) = app.cell_at_virtual_index(idx) else {
            continue;
        };
        match tool {
            ToolCell::PatchSummary(patch) if seen.insert(patch.path.clone()) => {
                lines.push(format!(
                    "• {} — {}",
                    truncate_line_to_width(&patch.path, 60),
                    activity_status_label(patch.status)
                ));
            }
            ToolCell::DiffPreview(diff) if seen.insert(diff.title.clone()) => {
                lines.push(format!(
                    "• {} (diff)",
                    truncate_line_to_width(&diff.title, 60)
                ));
            }
            _ => {}
        }
    }
    lines
}

/// Section 5 — diagnostics / LSP repair loop.
///
/// Structured per-turn LSP repair state is not surfaced to the TUI yet
/// (issue #4106), so this degrades to the coarse enable flag rather than a
/// blank section.
fn turn_diagnostics_lines(app: &App) -> Vec<String> {
    if app.lsp_enabled {
        vec!["LSP enabled — no repair-loop details surfaced yet (see #4106)".to_string()]
    } else {
        vec!["LSP disabled".to_string()]
    }
}

/// Section 6 — tests / verifier results.
///
/// Heuristic first pass (issue #4107): scans the turn's exec/review tool cells
/// for verifier-shaped commands and reports their status. Degrades to `none`
/// when nothing test-shaped ran.
fn turn_verifier_lines(app: &App, start: usize, end: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for idx in start..end {
        let Some(HistoryCell::Tool(tool)) = app.cell_at_virtual_index(idx) else {
            continue;
        };
        match tool {
            ToolCell::Exec(exec) if command_looks_like_verifier(&exec.command) => {
                lines.push(format!(
                    "• {} — {}",
                    truncate_line_to_width(&exec.command, 56),
                    activity_status_label(exec.status)
                ));
            }
            ToolCell::Review(review) => {
                let target = truncate_line_to_width(review.target.trim(), 48);
                let target = if target.is_empty() {
                    "review".to_string()
                } else {
                    format!("review {target}")
                };
                lines.push(format!(
                    "• {target} — {}",
                    activity_status_label(review.status)
                ));
            }
            _ => {}
        }
    }
    lines
}

fn command_looks_like_verifier(command: &str) -> bool {
    let lower = command.to_lowercase();
    [
        "test",
        "pytest",
        "jest",
        "cargo check",
        "cargo clippy",
        "verif",
        "lint",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Section 7 — approvals / denials.
///
/// The approval allow/deny sets are session-scoped (not per-turn), so the
/// counts are labelled `(session)` to avoid implying turn precision.
fn turn_approvals_lines(app: &App) -> Vec<String> {
    let mut lines = Vec::new();
    let approved = app.approval_session_approved.len();
    let denied = app.approval_session_denied.len();
    if approved > 0 {
        lines.push(format!("Approved (session): {approved}"));
    }
    if denied > 0 {
        lines.push(format!("Denied (session): {denied}"));
    }
    lines
}

/// Section 8 — model route plus token/cost accounting.
fn turn_route_lines(app: &App) -> Vec<String> {
    let mut lines = Vec::new();

    let (provider, model) = match app.pending_turn_route.as_ref() {
        Some((provider, model, _passthrough)) => (provider.display_name(), model.clone()),
        None => (app.api_provider.display_name(), app.model.clone()),
    };
    lines.push(format!("Route: {provider} · {model}"));

    let session = &app.session;
    match (session.last_prompt_tokens, session.last_completion_tokens) {
        (Some(prompt), Some(completion)) => {
            lines.push(format!(
                "Tokens (last turn): {prompt} in · {completion} out"
            ));
        }
        (Some(prompt), None) => lines.push(format!("Tokens (last turn): {prompt} in")),
        (None, Some(completion)) => lines.push(format!("Tokens (last turn): {completion} out")),
        (None, None) => {
            if session.total_tokens > 0 {
                lines.push(format!("Tokens (session): {}", session.total_tokens));
            }
        }
    }

    let cost = app.displayed_session_cost_for_currency(app.cost_currency);
    if cost > 0.0 {
        lines.push(format!("Cost (session): {}", app.format_cost_amount(cost)));
    }

    lines
}

/// Section 9 — final result / current status.
fn turn_result_lines(app: &App, start: usize, end: usize) -> Vec<String> {
    let mut lines = Vec::new();

    let status = match app.runtime_turn_status.as_deref() {
        Some("in_progress") => "in progress",
        Some(other) => other,
        None => "idle",
    };
    lines.push(format!("Status: {status}"));

    let final_text = (start..end)
        .rev()
        .find_map(|idx| match app.cell_at_virtual_index(idx) {
            Some(HistoryCell::Assistant { content, .. }) => {
                let summary = one_line_summary(content, 200);
                (!summary.is_empty()).then_some(summary)
            }
            _ => None,
        });
    if let Some(text) = final_text {
        lines.push(format!("Result: {text}"));
    } else if status == "in progress" {
        lines.push("Result: turn still running".to_string());
    } else {
        lines.push("Result: —".to_string());
    }

    let error_text = (start..end)
        .rev()
        .find_map(|idx| match app.cell_at_virtual_index(idx) {
            Some(HistoryCell::Error { message, .. }) => {
                let summary = one_line_summary(message, 160);
                (!summary.is_empty()).then_some(summary)
            }
            _ => None,
        });
    if let Some(err) = error_text {
        lines.push(format!("Error: {err}"));
    }

    lines
}
