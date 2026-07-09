//! Preserved tool-output rendering and line selection.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::palette;

use super::constants::{TOOL_OUTPUT_HEAD_LINES, TOOL_OUTPUT_TAIL_LINES, TOOL_TEXT_LIMIT};
use super::{
    RenderMode, details_affordance_line, looks_like_file_path, render_card_detail_line,
    render_card_detail_line_single, tool_value_style, truncate_text,
};

pub(super) fn render_tool_output_mode(
    output: &str,
    width: u16,
    line_limit: usize,
    mode: RenderMode,
) -> Vec<Line<'static>> {
    render_preserved_output_mode(output, width, line_limit, mode, "result")
}

pub(super) fn render_exec_output_mode(
    output: &str,
    width: u16,
    line_limit: usize,
    mode: RenderMode,
) -> Vec<Line<'static>> {
    render_preserved_output_mode(output, width, line_limit, mode, "output")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputRow {
    pub text: String,
    pub intact: bool,
}

/// Heuristic: does the output look like a unified diff? Returns true when
/// the output contains at least one hunk header (`@@`) or a `diff --git`
/// line, which are reliable markers of unified diff content (#380).
pub(crate) fn output_looks_like_diff(output: &str) -> bool {
    let mut lines = output.lines();
    // Check first 5 lines for diff markers
    for _ in 0..5 {
        let Some(line) = lines.next() else { break };
        let trimmed = line.trim();
        if trimmed.starts_with("@@") || trimmed.starts_with("diff --git") {
            return true;
        }
    }
    false
}

fn summarize_string_value(text: &str, max_len: usize, count_only: bool) -> String {
    let trimmed = text.trim();
    let len = trimmed.chars().count();
    if count_only || len > max_len {
        return format!("<{len} chars>");
    }
    truncate_text(trimmed, max_len)
}

fn summarize_inline_value(value: &Value, max_len: usize, count_only: bool) -> String {
    match value {
        Value::String(s) => summarize_string_value(s, max_len, count_only),
        Value::Array(items) => format!("<{} items>", items.len()),
        Value::Object(map) => format!("<{} keys>", map.len()),
        Value::Bool(b) => b.to_string(),
        Value::Number(num) => num.to_string(),
        Value::Null => "null".to_string(),
    }
}

fn is_noisy_tool_arg_key(key: &str) -> bool {
    matches!(
        key,
        "limit"
            | "max_count"
            | "max_output_tokens"
            | "offset"
            | "page"
            | "page_size"
            | "per_page"
            | "response_length"
            | "timeout_ms"
            | "yield_time_ms"
    )
}

#[must_use]
pub fn summarize_tool_args(input: &Value) -> Option<String> {
    let obj = input.as_object()?;
    if obj.is_empty() {
        return None;
    }

    let mut parts = Vec::new();

    if let Some(value) = obj.get("path") {
        parts.push(format!(
            "path: {}",
            summarize_inline_value(value, 80, false)
        ));
    }
    if let Some(value) = obj.get("command") {
        parts.push(format!(
            "command: {}",
            summarize_inline_value(value, 80, false)
        ));
    }
    if let Some(value) = obj.get("query") {
        parts.push(format!(
            "query: {}",
            summarize_inline_value(value, 80, false)
        ));
    }
    if let Some(value) = obj.get("prompt") {
        parts.push(format!(
            "prompt: {}",
            summarize_inline_value(value, 80, false)
        ));
    }
    if let Some(value) = obj.get("text") {
        parts.push(format!(
            "text: {}",
            summarize_inline_value(value, 80, false)
        ));
    }
    if let Some(value) = obj.get("pattern") {
        parts.push(format!(
            "pattern: {}",
            summarize_inline_value(value, 80, false)
        ));
    }
    if let Some(value) = obj.get("model") {
        parts.push(format!(
            "model: {}",
            summarize_inline_value(value, 40, false)
        ));
    }
    if let Some(value) = obj.get("profile") {
        parts.push(format!(
            "profile: {}",
            summarize_inline_value(value, 40, false)
        ));
    }
    if let Some(value) = obj.get("level") {
        parts.push(format!(
            "level: {}",
            summarize_inline_value(value, 40, false)
        ));
    }
    if let Some(value) = obj.get("file_id") {
        parts.push(format!(
            "file_id: {}",
            summarize_inline_value(value, 40, false)
        ));
    }
    if let Some(value) = obj.get("task_id") {
        parts.push(format!(
            "task_id: {}",
            summarize_inline_value(value, 40, false)
        ));
    }
    if let Some(value) = obj.get("voice_id") {
        parts.push(format!(
            "voice_id: {}",
            summarize_inline_value(value, 40, false)
        ));
    }
    if let Some(value) = obj.get("content") {
        parts.push(format!(
            "content: {}",
            summarize_inline_value(value, 0, true)
        ));
    }

    if parts.is_empty()
        && let Some((key, value)) = obj
            .iter()
            .find(|(key, _)| !is_noisy_tool_arg_key(key.as_str()))
    {
        return Some(format!(
            "{}: {}",
            key,
            summarize_inline_value(value, 80, false)
        ));
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

#[must_use]
pub fn summarize_tool_output(output: &str) -> String {
    if let Ok(json) = serde_json::from_str::<Value>(output) {
        if let Some(obj) = json.as_object() {
            if let Some(error) = obj.get("error").or(obj.get("status_msg")) {
                return format!("Error: {}", summarize_inline_value(error, 120, false));
            }

            let mut parts = Vec::new();

            if let Some(status) = obj.get("status").and_then(|v| v.as_str()) {
                parts.push(format!("status: {status}"));
            }
            if let Some(message) = obj.get("message").and_then(|v| v.as_str()) {
                parts.push(truncate_text(message, TOOL_TEXT_LIMIT));
            }
            if let Some(task_id) = obj.get("task_id").and_then(|v| v.as_str()) {
                parts.push(format!("task_id: {task_id}"));
            }
            if let Some(file_id) = obj.get("file_id").and_then(|v| v.as_str()) {
                parts.push(format!("file_id: {file_id}"));
            }
            if let Some(url) = obj
                .get("file_url")
                .or_else(|| obj.get("url"))
                .and_then(|v| v.as_str())
            {
                parts.push(format!("url: {}", truncate_text(url, 120)));
            }
            if let Some(data) = obj.get("data") {
                parts.push(format!("data: {}", summarize_inline_value(data, 80, true)));
            }

            if !parts.is_empty() {
                return parts.join(" | ");
            }

            if let Some(content) = obj
                .get("content")
                .or(obj.get("result"))
                .or(obj.get("output"))
            {
                return summarize_inline_value(content, TOOL_TEXT_LIMIT, false);
            }
        }

        return summarize_inline_value(&json, TOOL_TEXT_LIMIT, true);
    }

    truncate_text(output, TOOL_TEXT_LIMIT)
}

/// Summary information extracted from an MCP tool output payload.
pub struct McpOutputSummary {
    pub content: Option<String>,
    pub is_image: bool,
    pub is_error: Option<bool>,
}

/// Summarize raw MCP output into UI-friendly content.
#[must_use]
pub fn summarize_mcp_output(output: &str) -> McpOutputSummary {
    if let Ok(json) = serde_json::from_str::<Value>(output) {
        let is_error = json
            .get("isError")
            .and_then(serde_json::Value::as_bool)
            .or_else(|| json.get("is_error").and_then(serde_json::Value::as_bool));

        if let Some(blocks) = json.get("content").and_then(|v| v.as_array()) {
            let mut lines = Vec::new();
            let mut is_image = false;

            for block in blocks {
                let block_type = block
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                match block_type {
                    "text" => {
                        let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        if !text.is_empty() {
                            lines.push(format!("- text: {}", truncate_text(text, 200)));
                        }
                    }
                    "image" | "image_url" => {
                        is_image = true;
                        let url = block
                            .get("url")
                            .or_else(|| block.get("image_url"))
                            .and_then(|v| v.as_str());
                        if let Some(url) = url {
                            lines.push(format!("- image: {}", truncate_text(url, 200)));
                        } else {
                            lines.push("- image".to_string());
                        }
                    }
                    "resource" | "resource_link" => {
                        let uri = block
                            .get("uri")
                            .or_else(|| block.get("url"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("<resource>");
                        lines.push(format!("- resource: {}", truncate_text(uri, 200)));
                    }
                    other => {
                        lines.push(format!("- {other} content"));
                    }
                }
            }

            return McpOutputSummary {
                content: if lines.is_empty() {
                    None
                } else {
                    Some(lines.join("\n"))
                },
                is_image,
                is_error,
            };
        }
    }

    McpOutputSummary {
        content: Some(summarize_tool_output(output)),
        is_image: output_is_image(output),
        is_error: None,
    }
}

#[must_use]
pub fn output_is_image(output: &str) -> bool {
    let lower = output.to_lowercase();

    [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".tiff", ".ppm",
    ]
    .iter()
    .any(|ext| lower.contains(ext))
}

fn render_preserved_output_mode(
    output: &str,
    width: u16,
    line_limit: usize,
    mode: RenderMode,
    first_label: &str,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if output.trim().is_empty() {
        // #3031: In compact/Live mode, suppress "(no output)" — the tool
        // header already carries the success/failure status. Transcript
        // mode still records it for exports/clipboard/pager.
        if mode == RenderMode::Transcript {
            lines.push(Line::from(Span::styled(
                "  (no output)",
                Style::default().fg(palette::TEXT_MUTED).italic(),
            )));
        }
        return lines;
    }

    // Hash once; reuse for both the rows cache and the indices cache below.
    let content_hash = crate::tui::output_rows_cache::hash_str(output);
    let all_lines =
        crate::tui::output_rows_cache::get_or_compute_rows_with_hash(content_hash, width, || {
            output_rows(output, width)
        });

    if matches!(mode, RenderMode::Transcript) {
        // Full-content path: emit every wrapped line with no head/tail split,
        // no "+N more" affordance.
        for (idx, row) in all_lines.iter().enumerate() {
            render_output_row(
                &mut lines,
                if idx == 0 { Some(first_label) } else { None },
                row,
                width,
            );
        }
        return lines;
    }

    let selected = crate::tui::output_rows_cache::get_or_compute_indices(
        content_hash,
        width,
        line_limit,
        || selected_output_indices(&all_lines, line_limit),
    );
    let mut previous: Option<usize> = None;
    for (rendered_idx, idx) in selected.iter().copied().enumerate() {
        if let Some(prev) = previous {
            let omitted = idx.saturating_sub(prev + 1);
            if omitted > 0 {
                lines.push(details_affordance_line(
                    &format!(
                        "{omitted} lines omitted; {}",
                        crate::tui::key_shortcuts::tool_details_shortcut_action_hint("full output")
                    ),
                    Style::default().fg(palette::TEXT_MUTED),
                ));
            }
        }

        let row = &all_lines[idx];
        render_output_row(
            &mut lines,
            if rendered_idx == 0 {
                Some(first_label)
            } else {
                None
            },
            row,
            width,
        );
        previous = Some(idx);
    }

    lines
}

fn output_rows(output: &str, width: u16) -> Vec<OutputRow> {
    let wrap_width = width.saturating_sub(4).max(1) as usize;
    let mut rows = Vec::new();
    let mut sanitized = String::with_capacity(output.len());
    for line in output.lines() {
        sanitized.clear();
        crate::tui::osc8::strip_ansi_into(line, &mut sanitized);
        let intact = is_path_or_url_like(&sanitized);
        if intact {
            rows.push(OutputRow {
                text: sanitized.clone(),
                intact: true,
            });
        } else {
            for wrapped in wrap_text(&sanitized, wrap_width) {
                rows.push(OutputRow {
                    text: wrapped,
                    intact: false,
                });
            }
        }
    }
    if rows.is_empty() {
        rows.push(OutputRow {
            text: String::new(),
            intact: false,
        });
    }
    rows
}

fn selected_output_indices(rows: &[OutputRow], line_limit: usize) -> Vec<usize> {
    let total = rows.len();
    if total <= line_limit || line_limit == 0 {
        return (0..total).collect();
    }

    let head = TOOL_OUTPUT_HEAD_LINES.min(line_limit).min(total);
    let tail = TOOL_OUTPUT_TAIL_LINES
        .min(line_limit.saturating_sub(head))
        .min(total.saturating_sub(head));
    let mut selected = std::collections::BTreeSet::new();
    selected.extend(0..head);
    selected.extend(total.saturating_sub(tail)..total);

    let budget = line_limit.saturating_sub(selected.len());
    if budget > 0 {
        let mut important: Vec<(usize, usize)> = rows
            .iter()
            .enumerate()
            .skip(head)
            .take(total.saturating_sub(head + tail))
            .filter_map(|(idx, row)| output_importance_rank(&row.text).map(|rank| (idx, rank)))
            .collect();
        important.sort_by_key(|(idx, rank)| (*rank, *idx));
        for (idx, _) in important.into_iter().take(budget) {
            selected.insert(idx);
        }
    }

    selected.into_iter().collect()
}

fn output_importance_rank(line: &str) -> Option<usize> {
    let lower = line.to_ascii_lowercase();
    if [
        "error",
        "failed",
        "failure",
        "fatal",
        "panic",
        "exception",
        "traceback",
        "denied",
        "not found",
        "no such file",
        "cannot",
        "can't",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return Some(0);
    }
    if lower.contains("warning") || lower.contains("warn") {
        return Some(1);
    }
    if is_path_or_url_like(line) {
        return Some(2);
    }
    None
}

fn is_path_or_url_like(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.contains("://") || trimmed.starts_with("file:") {
        return true;
    }
    let has_separator = trimmed.contains('/') || trimmed.contains('\\');
    let has_extension = trimmed
        .split_whitespace()
        .any(|part| part.rsplit_once('.').is_some_and(|(_, ext)| ext.len() <= 8));
    has_separator && has_extension
}

/// Detect whether a line contains a `path:line` pattern that could be
/// opened by `try_open_file_at_line`. Returns a distinctive style
/// (underline + blue) when the pattern matches, or `None` otherwise.
/// The style is applied over the existing value style so the line
/// remains readable.
fn file_line_style(text: &str) -> Option<Style> {
    let trimmed = text.trim();
    if let Some((before, after)) = trimmed.rsplit_once(':')
        && !before.is_empty()
        && after.chars().all(|c| c.is_ascii_digit())
        && looks_like_file_path(before)
    {
        Some(
            Style::default()
                .fg(palette::WHALE_INFO)
                .add_modifier(ratatui::style::Modifier::UNDERLINED),
        )
    } else {
        None
    }
}

/// Apply inline diff highlighting to a single text line.
///
/// Returns the appropriate style for the line based on its prefix:
/// - Lines starting with `+` (after trimming) => `palette::DIFF_ADDED` (green)
/// - Lines starting with `-` (after trimming) => `palette::STATUS_ERROR` (red)
/// - Lines starting with `@@` => `palette::WHALE_INFO` (cyan/blue)
/// - All other lines => None (use default style)
fn diff_line_style(text: &str) -> Option<Style> {
    let trimmed = text.trim_start();
    if trimmed.starts_with("@@") {
        Some(Style::default().fg(palette::WHALE_ACCENT_PRIMARY))
    } else if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
        Some(Style::default().fg(palette::DIFF_ADDED))
    } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
        Some(Style::default().fg(palette::STATUS_ERROR))
    } else {
        None
    }
}

fn render_output_row(
    lines: &mut Vec<Line<'static>>,
    label: Option<&str>,
    row: &OutputRow,
    width: u16,
) {
    // #374: apply file:line highlighting when the row text contains
    // a `path:line` pattern. Diff style takes precedence (colored
    // prefix lines should stay colored), but if no diff style matched,
    // check for a file:line pattern and highlight it distinctively.
    let diff_style = diff_line_style(&row.text);
    let file_style = file_line_style(&row.text);
    let value_style = diff_style.or(file_style).unwrap_or_else(tool_value_style);
    if row.intact {
        lines.push(render_card_detail_line_single(
            label,
            &row.text,
            value_style,
        ));
    } else {
        lines.extend(render_card_detail_line(
            label,
            &row.text,
            value_style,
            width,
        ));
    }
}

pub(super) fn wrap_plain_line(line: &str, style: Style, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for part in wrap_text(line, width.max(1) as usize) {
        lines.push(Line::from(Span::styled(part, style)));
    }
    lines
}

pub(super) fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        let tentative = if current.is_empty() {
            ch.to_string()
        } else {
            let mut t = current.clone();
            t.push(ch);
            t
        };

        if UnicodeWidthStr::width(tentative.as_str()) > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }

        current.push(ch);
    }

    lines.push(current);

    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}
