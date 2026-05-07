//! `retrieve_tool_result` - selective retrieval for spilled tool outputs.
//!
//! Large successful tool results are spilled to
//! `~/.deepseek/tool_outputs/<tool-call-id>.txt` by `tools::truncate`. This
//! tool gives the model a read-only, directory-scoped way to fetch summaries or
//! slices of those historical outputs without replaying the entire file into
//! every subsequent request.

use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::spec::{
    ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, optional_str, optional_u64,
    required_str,
};

const DEFAULT_MAX_BYTES: usize = 8 * 1024;
const HARD_MAX_BYTES: usize = 128 * 1024;
const DEFAULT_LINE_COUNT: usize = 40;
const HARD_LINE_COUNT: usize = 500;
const DEFAULT_MAX_MATCHES: usize = 20;
const HARD_MAX_MATCHES: usize = 100;
const DEFAULT_CONTEXT_LINES: usize = 1;
const HARD_CONTEXT_LINES: usize = 5;

/// Retrieve summaries or slices of a prior spilled tool result.
pub struct RetrieveToolResultTool;

#[async_trait]
impl ToolSpec for RetrieveToolResultTool {
    fn name(&self) -> &'static str {
        "retrieve_tool_result"
    }

    fn description(&self) -> &'static str {
        "Retrieve a previously spilled large tool result from ~/.deepseek/tool_outputs by tool call id, filename, or spillover path. Supports summary, head, tail, lines, and query modes so you can fetch only the needed historical output."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "ref": {
                    "type": "string",
                    "description": "Tool call id, tool_result:<id>, spillover filename, or absolute spillover path."
                },
                "mode": {
                    "type": "string",
                    "enum": ["summary", "head", "tail", "lines", "query"],
                    "description": "Retrieval mode. Defaults to summary."
                },
                "query": {
                    "type": "string",
                    "description": "Case-insensitive substring to search for when mode=query."
                },
                "lines": {
                    "type": "string",
                    "description": "Line selector for mode=lines, e.g. \"10\" or \"10-40\"."
                },
                "start_line": {
                    "type": "integer",
                    "description": "1-based first line for mode=lines."
                },
                "end_line": {
                    "type": "integer",
                    "description": "1-based final line for mode=lines."
                },
                "line_count": {
                    "type": "integer",
                    "description": "Number of lines for head/tail modes. Default 40, hard cap 500."
                },
                "max_bytes": {
                    "type": "integer",
                    "description": "Maximum bytes of excerpt text returned. Default 8192, hard cap 131072."
                },
                "max_matches": {
                    "type": "integer",
                    "description": "Maximum query matches or signal lines returned. Default 20, hard cap 100."
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Extra lines around each query match. Default 1, hard cap 5."
                }
            },
            "required": ["ref"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let reference = required_str(&input, "ref")?.trim();
        if reference.is_empty() {
            return Err(ToolError::invalid_input("ref cannot be empty"));
        }

        let mode = optional_str(&input, "mode")
            .unwrap_or("summary")
            .trim()
            .to_ascii_lowercase();
        let max_bytes = clamp_u64(
            optional_u64(&input, "max_bytes", DEFAULT_MAX_BYTES as u64),
            1,
            HARD_MAX_BYTES,
        );
        let path = resolve_spillover_reference(reference)?;
        let content = fs::read_to_string(&path).map_err(|err| {
            ToolError::execution_failed(format!("failed to read {}: {err}", path.display()))
        })?;

        let lines: Vec<&str> = content.lines().collect();
        let payload = match mode.as_str() {
            "summary" => {
                build_summary_payload(reference, &path, &content, &lines, &input, max_bytes)
            }
            "head" => build_head_tail_payload(reference, &path, "head", &lines, &input, max_bytes),
            "tail" => build_head_tail_payload(reference, &path, "tail", &lines, &input, max_bytes),
            "lines" => build_lines_payload(reference, &path, &lines, &input, max_bytes)?,
            "query" => build_query_payload(reference, &path, &lines, &input, max_bytes)?,
            other => {
                return Err(ToolError::invalid_input(format!(
                    "unsupported mode `{other}` (expected summary, head, tail, lines, or query)"
                )));
            }
        };

        ToolResult::json(&payload).map_err(|err| {
            ToolError::execution_failed(format!("failed to serialize result: {err}"))
        })
    }
}

fn resolve_spillover_reference(reference: &str) -> Result<PathBuf, ToolError> {
    let root = crate::tools::truncate::spillover_root()
        .ok_or_else(|| ToolError::execution_failed("could not resolve ~/.deepseek/tool_outputs"))?;
    let root_canonical = root.canonicalize().map_err(|err| {
        ToolError::execution_failed(format!(
            "spillover directory {} is not readable: {err}",
            root.display()
        ))
    })?;

    let trimmed = reference.trim();
    let stripped = trimmed.strip_prefix("tool_result:").unwrap_or(trimmed);
    let raw_path = PathBuf::from(stripped);

    let candidate = if raw_path.is_absolute() {
        raw_path
    } else if stripped.ends_with(".txt")
        || stripped.contains('/')
        || (std::path::MAIN_SEPARATOR != '/' && stripped.contains(std::path::MAIN_SEPARATOR))
    {
        root.join(stripped)
    } else {
        crate::tools::truncate::spillover_path(stripped).ok_or_else(|| {
            ToolError::invalid_input(format!("invalid spilled tool-result ref `{reference}`"))
        })?
    };

    let canonical = candidate.canonicalize().map_err(|err| {
        ToolError::execution_failed(format!(
            "spilled tool result `{reference}` was not found at {}: {err}",
            candidate.display()
        ))
    })?;

    if !canonical.starts_with(&root_canonical) {
        return Err(ToolError::invalid_input(format!(
            "ref `{reference}` does not point inside {}",
            root_canonical.display()
        )));
    }
    if !canonical.is_file() {
        return Err(ToolError::invalid_input(format!(
            "ref `{reference}` does not point to a spillover file"
        )));
    }

    Ok(canonical)
}

fn build_summary_payload(
    reference: &str,
    path: &std::path::Path,
    content: &str,
    lines: &[&str],
    input: &Value,
    max_bytes: usize,
) -> Value {
    let max_matches = clamp_u64(
        optional_u64(input, "max_matches", DEFAULT_MAX_MATCHES as u64),
        1,
        HARD_MAX_MATCHES,
    );
    let signal_lines = collect_signal_lines(lines, max_matches);
    let head_count = DEFAULT_LINE_COUNT.min(lines.len());
    let tail_count = DEFAULT_LINE_COUNT.min(lines.len());
    let head = render_numbered_lines(
        lines
            .iter()
            .take(head_count)
            .enumerate()
            .map(|(idx, line)| (idx + 1, *line)),
        max_bytes / 2,
    );
    let tail_start = lines.len().saturating_sub(tail_count);
    let tail = render_numbered_lines(
        lines
            .iter()
            .enumerate()
            .skip(tail_start)
            .map(|(idx, line)| (idx + 1, *line)),
        max_bytes / 2,
    );

    json!({
        "ref": reference,
        "path": path.display().to_string(),
        "mode": "summary",
        "total_bytes": content.len(),
        "total_lines": lines.len(),
        "non_empty_lines": lines.iter().filter(|line| !line.trim().is_empty()).count(),
        "signal_lines": signal_lines,
        "head": head,
        "tail": tail,
        "hint": "Use mode=head, tail, lines, or query to retrieve a narrower slice."
    })
}

fn build_head_tail_payload(
    reference: &str,
    path: &std::path::Path,
    mode: &str,
    lines: &[&str],
    input: &Value,
    max_bytes: usize,
) -> Value {
    let count = clamp_u64(
        optional_u64(input, "line_count", DEFAULT_LINE_COUNT as u64),
        1,
        HARD_LINE_COUNT,
    );
    let selected: Vec<(usize, &str)> = if mode == "head" {
        lines
            .iter()
            .take(count)
            .enumerate()
            .map(|(idx, line)| (idx + 1, *line))
            .collect()
    } else {
        let start = lines.len().saturating_sub(count);
        lines
            .iter()
            .enumerate()
            .skip(start)
            .map(|(idx, line)| (idx + 1, *line))
            .collect()
    };
    let excerpt = render_numbered_lines(selected.iter().copied(), max_bytes);

    json!({
        "ref": reference,
        "path": path.display().to_string(),
        "mode": mode,
        "total_lines": lines.len(),
        "line_count": count,
        "excerpt": excerpt,
    })
}

fn build_lines_payload(
    reference: &str,
    path: &std::path::Path,
    lines: &[&str],
    input: &Value,
    max_bytes: usize,
) -> Result<Value, ToolError> {
    let (start, end) = parse_line_selector(input)?;
    let excerpt = if start > lines.len() {
        String::new()
    } else {
        let end = end.min(lines.len());
        render_numbered_lines(
            lines
                .iter()
                .enumerate()
                .skip(start - 1)
                .take(end.saturating_sub(start) + 1)
                .map(|(idx, line)| (idx + 1, *line)),
            max_bytes,
        )
    };

    Ok(json!({
        "ref": reference,
        "path": path.display().to_string(),
        "mode": "lines",
        "total_lines": lines.len(),
        "start_line": start,
        "end_line": end.min(lines.len()),
        "excerpt": excerpt,
    }))
}

fn build_query_payload(
    reference: &str,
    path: &std::path::Path,
    lines: &[&str],
    input: &Value,
    max_bytes: usize,
) -> Result<Value, ToolError> {
    let query = optional_str(input, "query")
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .ok_or_else(|| ToolError::invalid_input("query is required when mode=query"))?;
    let query_lower = query.to_lowercase();
    let max_matches = clamp_u64(
        optional_u64(input, "max_matches", DEFAULT_MAX_MATCHES as u64),
        1,
        HARD_MAX_MATCHES,
    );
    let context_lines = clamp_u64(
        optional_u64(input, "context_lines", DEFAULT_CONTEXT_LINES as u64),
        0,
        HARD_CONTEXT_LINES,
    );

    let mut matched_lines = 0usize;
    let mut results = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if !line.to_lowercase().contains(&query_lower) {
            continue;
        }
        matched_lines += 1;
        if results.len() >= max_matches {
            continue;
        }
        let start = idx.saturating_sub(context_lines);
        let end = (idx + context_lines).min(lines.len().saturating_sub(1));
        let excerpt = render_numbered_lines(
            lines
                .iter()
                .enumerate()
                .skip(start)
                .take(end.saturating_sub(start) + 1)
                .map(|(line_idx, text)| (line_idx + 1, *text)),
            max_bytes / max_matches.max(1),
        );
        results.push(json!({
            "line": idx + 1,
            "excerpt": excerpt,
        }));
    }

    Ok(json!({
        "ref": reference,
        "path": path.display().to_string(),
        "mode": "query",
        "query": query,
        "total_lines": lines.len(),
        "matched_lines": matched_lines,
        "matches_returned": results.len(),
        "results": results,
    }))
}

fn parse_line_selector(input: &Value) -> Result<(usize, usize), ToolError> {
    let explicit_start = input.get("start_line").and_then(Value::as_u64);
    let explicit_end = input.get("end_line").and_then(Value::as_u64);
    if explicit_start.is_some() || explicit_end.is_some() {
        let start = explicit_start.ok_or_else(|| {
            ToolError::invalid_input("start_line is required when end_line is supplied")
        })?;
        let end = explicit_end.unwrap_or(start);
        return validate_line_range(start as usize, end as usize);
    }

    let spec = optional_str(input, "lines")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ToolError::invalid_input(
                "mode=lines requires `lines` (for example \"10-40\") or start_line/end_line",
            )
        })?;

    if let Some((start, end)) = spec.split_once('-') {
        let start = parse_positive_line(start.trim(), "lines start")?;
        let end = parse_positive_line(end.trim(), "lines end")?;
        validate_line_range(start, end)
    } else {
        let line = parse_positive_line(spec, "lines")?;
        validate_line_range(line, line)
    }
}

fn validate_line_range(start: usize, end: usize) -> Result<(usize, usize), ToolError> {
    if start == 0 || end == 0 {
        return Err(ToolError::invalid_input("line numbers are 1-based"));
    }
    if end < start {
        return Err(ToolError::invalid_input(
            "end_line must be greater than or equal to start_line",
        ));
    }
    Ok((start, end))
}

fn parse_positive_line(raw: &str, field: &str) -> Result<usize, ToolError> {
    raw.parse::<usize>().map_err(|_| {
        ToolError::invalid_input(format!("{field} must be a positive integer line number"))
    })
}

fn collect_signal_lines(lines: &[&str], max_matches: usize) -> Vec<Value> {
    let mut out = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if !is_signal_line(line) {
            continue;
        }
        out.push(json!({
            "line": idx + 1,
            "text": truncate_line(line.trim(), 300),
        }));
        if out.len() >= max_matches {
            break;
        }
    }
    out
}

fn is_signal_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    [
        "error",
        "failed",
        "failure",
        "panic",
        "warning",
        "exception",
        "traceback",
        "assertion",
        "exit code",
        "test result",
        "thread '",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn render_numbered_lines<'a>(
    lines: impl IntoIterator<Item = (usize, &'a str)>,
    max_bytes: usize,
) -> String {
    let mut rendered = String::new();
    for (line_no, line) in lines {
        rendered.push_str(&format!("{line_no}: {line}\n"));
        if rendered.len() > max_bytes {
            break;
        }
    }
    truncate_text(&rendered, max_bytes)
}

fn truncate_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.trim_end_matches('\n').to_string();
    }
    let note = "\n[truncated to max_bytes]";
    let budget = max_bytes.saturating_sub(note.len()).max(1);
    let cut = (0..=budget)
        .rev()
        .find(|idx| text.is_char_boundary(*idx))
        .unwrap_or(0);
    format!("{}{}", text[..cut].trim_end_matches('\n'), note)
}

fn truncate_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    let mut out: String = line.chars().take(max_chars.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

fn clamp_u64(value: u64, min: usize, max: usize) -> usize {
    (value as usize).clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;
    use tempfile::tempdir;

    struct SpilloverRootGuard {
        prior: Option<PathBuf>,
    }

    impl Drop for SpilloverRootGuard {
        fn drop(&mut self) {
            crate::tools::truncate::set_test_spillover_root(self.prior.take());
        }
    }

    fn set_spillover_root(path: PathBuf) -> SpilloverRootGuard {
        let prior = crate::tools::truncate::set_test_spillover_root(Some(path));
        SpilloverRootGuard { prior }
    }

    fn context() -> ToolContext {
        let tmp = tempdir().unwrap();
        ToolContext::new(tmp.path())
    }

    fn test_lock() -> MutexGuard<'static, ()> {
        crate::tools::truncate::TEST_SPILLOVER_GUARD
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    fn execute_tool(input: Value) -> Result<ToolResult, ToolError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(RetrieveToolResultTool.execute(input, &context()))
    }

    #[test]
    fn summary_reads_spillover_by_tool_call_id() {
        let _lock = test_lock();
        let tmp = tempdir().unwrap();
        let _guard = set_spillover_root(tmp.path().join("tool_outputs"));
        crate::tools::truncate::write_spillover(
            "call-abc",
            "checking crate\nerror[E0425]: missing value\nwarning: unused import\nfinished",
        )
        .unwrap();

        let result = execute_tool(json!({"ref": "call-abc"})).unwrap();

        assert!(result.success);
        let body: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(body["mode"], "summary");
        assert!(body["signal_lines"].to_string().contains("error[E0425]"));
        assert!(body["signal_lines"].to_string().contains("warning"));
    }

    #[test]
    fn query_returns_matching_line_with_context() {
        let _lock = test_lock();
        let tmp = tempdir().unwrap();
        let _guard = set_spillover_root(tmp.path().join("tool_outputs"));
        crate::tools::truncate::write_spillover(
            "call-query",
            "one\ntwo before\nneedle here\nafter\nlast",
        )
        .unwrap();

        let result = execute_tool(json!({
            "ref": "tool_result:call-query",
            "mode": "query",
            "query": "needle",
            "context_lines": 1
        }))
        .unwrap();

        let body: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(body["matched_lines"], 1);
        let rendered = body["results"].to_string();
        assert!(rendered.contains("2: two before"));
        assert!(rendered.contains("3: needle here"));
        assert!(rendered.contains("4: after"));
    }

    #[test]
    fn lines_mode_accepts_filename_inside_spillover_root() {
        let _lock = test_lock();
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("tool_outputs");
        let _guard = set_spillover_root(root.clone());
        crate::tools::truncate::write_spillover("call-lines", "a\nb\nc\nd").unwrap();

        let result = execute_tool(json!({
            "ref": "call-lines.txt",
            "mode": "lines",
            "lines": "2-3"
        }))
        .unwrap();

        let body: Value = serde_json::from_str(&result.content).unwrap();
        let excerpt = body["excerpt"].as_str().unwrap();
        assert!(excerpt.contains("2: b"));
        assert!(excerpt.contains("3: c"));
        assert!(!excerpt.contains("1: a"));
        assert!(!excerpt.contains("4: d"));
    }

    #[test]
    fn rejects_path_outside_spillover_root() {
        let _lock = test_lock();
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("tool_outputs");
        fs::create_dir_all(&root).unwrap();
        let outside = tmp.path().join("outside.txt");
        fs::write(&outside, "secret").unwrap();
        let _guard = set_spillover_root(root);

        let err = execute_tool(json!({"ref": outside.display().to_string()})).unwrap_err();

        assert!(
            err.to_string().contains("does not point inside"),
            "unexpected error: {err}"
        );
    }
}
