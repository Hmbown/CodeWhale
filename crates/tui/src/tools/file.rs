//! File system tools: `read_file`, `write_file`, `edit_file`, `list_dir`
//!
//! These tools provide safe file system operations within the workspace,
//! with path validation to prevent escaping the workspace boundary.

use super::diff_format::make_unified_diff;
use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    lsp_diagnostics_for_paths, optional_bool, optional_str, required_str,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

// === ReadFileTool ===

/// Tool for reading UTF-8 files from the workspace.
pub struct ReadFileTool;

#[async_trait]
impl ToolSpec for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Read a UTF-8 file from the workspace. Use this instead of `cat`, `head`, `tail`, or `sed -n '..p'` in `exec_shell` — it's faster, sandbox-aware, and skips the approval prompt. Plain text is returned as-is; PDFs are auto-extracted via the bundled pure-Rust extractor (no Poppler install required). Cannot read images or non-PDF binaries.\n\nFor large files, use `start_line` and `max_lines` to read in chunks. By default, returns at most 200 lines (~16KB). If `truncated=\"true\"` in the response, use `next_start_line` to continue reading. For PDFs, use `pages` instead — `start_line`/`max_lines` only apply to text files."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file (relative to workspace or absolute)"
                },
                "start_line": {
                    "type": "integer",
                    "description": "Starting line (1-based, default 1)"
                },
                "max_lines": {
                    "type": "integer",
                    "description": "Maximum lines to return (default 200, max 500)"
                },
                "pages": {
                    "type": "string",
                    "description": "PDF only: page range to extract, e.g. \"1-5\" or \"10\". Ignored for non-PDF files."
                }
            },
            "required": ["path"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        // Bounded output for large files. The small-file fast path keeps the
        // historical "return contents unchanged" behavior so existing flows
        // (small configs, single source files, etc.) don't suddenly start
        // seeing wrapped output. Once a file is large or the caller asks
        // for an explicit range, we switch to a numbered, line-tagged
        // window with continuation hints so the model can page through
        // without re-loading the entire file on every turn. Harvested
        // from PR #1451 by @Oliver-ZPLiu, closes part of #1450.
        const DEFAULT_READ_LINES: usize = 200;
        const HARD_MAX_READ_LINES: usize = 500;
        const MAX_VISIBLE_BYTES: usize = 16 * 1024;
        const SMALL_FILE_LINES: usize = 200;
        const SMALL_FILE_BYTES: usize = 16 * 1024;

        let path_str = required_str(&input, "path")?;
        let file_path = context.resolve_path(path_str)?;
        let pages = optional_str(&input, "pages");

        if is_pdf(&file_path)? {
            return read_pdf(&file_path, pages);
        }

        let contents = fs::read_to_string(&file_path).map_err(|e| {
            ToolError::execution_failed(format!("Failed to read {}: {}", file_path.display(), e))
        })?;

        let total_lines = contents.lines().count();
        let total_bytes = contents.len();
        let explicit_range = input
            .get("start_line")
            .or_else(|| input.get("max_lines"))
            .is_some();

        // Small-file fast path. Only applies when the caller didn't pass an
        // explicit range — otherwise an explicit `start_line = 5` on a
        // tiny file would silently ignore the request.
        if !explicit_range && total_lines <= SMALL_FILE_LINES && total_bytes <= SMALL_FILE_BYTES {
            return Ok(ToolResult::success(contents));
        }

        let start_line = match input.get("start_line").and_then(Value::as_u64) {
            Some(0) => {
                return Err(ToolError::invalid_input(
                    "start_line must be 1-based and greater than 0".to_string(),
                ));
            }
            Some(v) => v as usize,
            None => 1,
        };

        let max_lines = match input.get("max_lines").and_then(Value::as_u64) {
            Some(0) => {
                return Err(ToolError::invalid_input(
                    "max_lines must be greater than 0".to_string(),
                ));
            }
            Some(v) => std::cmp::min(v as usize, HARD_MAX_READ_LINES),
            None => DEFAULT_READ_LINES,
        };

        // `start_line > total_lines` is not an error — it lets the model
        // page past the end without raising. Returns an empty-content
        // sentinel so subsequent reads can stop.
        if start_line > total_lines {
            let output = format!(
                "<file path=\"{path_str}\" total_lines=\"{total_lines}\" shown_lines=\"none\" truncated=\"false\">\n\
                 \n\
                 [NO CONTENT] start_line {start_line} is beyond total_lines {total_lines}.\n\
                 </file>"
            );
            return Ok(ToolResult::success(output));
        }

        let lines: Vec<&str> = contents.lines().collect();
        let zero_based_start = start_line - 1;
        let zero_based_end = std::cmp::min(zero_based_start + max_lines, total_lines);
        let shown_first = start_line;
        let shown_last = zero_based_end; // 1-based inclusive line number of the last shown line

        let mut numbered = String::new();
        for (offset, line) in lines[zero_based_start..zero_based_end].iter().enumerate() {
            let line_no = start_line + offset;
            numbered.push_str(&format!("{line_no:>6}│ {line}\n"));
        }

        // UTF-8-safe byte truncation of the rendered range.
        let truncated_by_bytes = numbered.len() > MAX_VISIBLE_BYTES;
        let shown_content = if truncated_by_bytes {
            let mut end = MAX_VISIBLE_BYTES;
            while end > 0 && !numbered.is_char_boundary(end) {
                end -= 1;
            }
            &numbered[..end]
        } else {
            &numbered
        };

        let truncated_by_lines = zero_based_end < total_lines;
        let truncated = truncated_by_lines || truncated_by_bytes;
        let next_start = zero_based_end + 1;

        let mut attrs = format!(
            "path=\"{path_str}\" total_lines=\"{total_lines}\" shown_lines=\"{shown_first}-{shown_last}\" truncated=\"{truncated}\""
        );
        if truncated_by_lines {
            attrs.push_str(&format!(" next_start_line=\"{next_start}\""));
        }

        let mut output = format!("<file {attrs}>\n{shown_content}");
        if truncated_by_lines {
            output.push_str(&format!(
                "\n[TRUNCATED] Showing lines {shown_first}-{shown_last} of {total_lines}. To continue, call read_file with path=\"{path_str}\" start_line={next_start} max_lines={max_lines}\n"
            ));
        }
        if truncated_by_bytes {
            output.push_str(
                "\n[TRUNCATED] The selected range exceeded 16KB. Continue with a smaller max_lines value.\n",
            );
        }
        output.push_str("</file>");

        Ok(ToolResult::success(output))
    }
}

/// Detect a PDF by extension OR by sniffing the `%PDF-` magic bytes.
/// Files without an extension are still recognized as PDFs when the header
/// matches.
fn is_pdf(path: &Path) -> Result<bool, ToolError> {
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    {
        return Ok(true);
    }
    // Sniff first 4 bytes. Don't error if the file doesn't exist — let the
    // caller's `read_to_string` produce the canonical not-found error.
    let mut buf = [0u8; 4];
    let result = match fs::File::open(path) {
        Ok(mut f) => {
            use std::io::Read;
            f.read_exact(&mut buf).map(|_| buf)
        }
        Err(_) => return Ok(false),
    };
    Ok(matches!(result, Ok(b) if &b == b"%PDF"))
}

fn parse_pages_arg(spec: &str) -> Option<(u32, u32)> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some((a, b)) = trimmed.split_once('-') {
        let start: u32 = a.trim().parse().ok()?;
        let end: u32 = b.trim().parse().ok()?;
        if start == 0 || end < start {
            return None;
        }
        Some((start, end))
    } else {
        let n: u32 = trimmed.parse().ok()?;
        if n == 0 {
            return None;
        }
        Some((n, n))
    }
}

fn read_pdf(path: &Path, pages: Option<&str>) -> Result<ToolResult, ToolError> {
    // Validate the `pages` spec once, up front, so both extractor paths
    // surface the same error shape on bad input.
    let page_range = match pages {
        Some(spec) => match parse_pages_arg(spec) {
            Some((start, end)) => Some((start, end)),
            None => {
                return Err(ToolError::invalid_input(format!(
                    "invalid `pages` value `{spec}` (expected `N` or `N-M`, e.g. `1-5`)"
                )));
            }
        },
        None => None,
    };

    // Default to the bundled pure-Rust `pdf-extract` reader: it removes
    // the install-poppler prerequisite that bit every new user, and the
    // crate is already a workspace dep (used by `web_run`'s URL fetch
    // path). Users with column-heavy / complex-table PDFs (academic
    // papers, financial filings) can opt into the historical
    // `pdftotext -layout` route by setting
    // `prefer_external_pdftotext = true` in `~/.config/deepseek/settings.toml`.
    let prefer_external = crate::settings::Settings::load()
        .map(|s| s.prefer_external_pdftotext)
        .unwrap_or(false);

    if prefer_external {
        read_pdf_via_pdftotext(path, page_range)
    } else {
        read_pdf_via_pdf_extract(path, page_range)
    }
}

fn read_pdf_via_pdf_extract(
    path: &Path,
    page_range: Option<(u32, u32)>,
) -> Result<ToolResult, ToolError> {
    let text = if let Some((start, end)) = page_range {
        // Page-by-page extraction so we can slice the requested window
        // without dragging every page through the caller's context.
        // pdf-extract returns pages in document order; `start`/`end` are
        // 1-indexed inclusive (validated above), so we convert to a
        // 0-indexed half-open slice with bounds clamping.
        let pages = pdf_extract::extract_text_by_pages(path).map_err(|e| {
            ToolError::execution_failed(format!(
                "pdf-extract failed on {}: {e} (set `prefer_external_pdftotext = true` in settings.toml to retry via pdftotext)",
                path.display()
            ))
        })?;
        let total = pages.len();
        if total == 0 {
            String::new()
        } else {
            let start_idx = (start as usize).saturating_sub(1).min(total);
            let end_idx = (end as usize).min(total);
            if start_idx >= end_idx {
                String::new()
            } else {
                pages[start_idx..end_idx].join("\n")
            }
        }
    } else {
        pdf_extract::extract_text(path).map_err(|e| {
            ToolError::execution_failed(format!(
                "pdf-extract failed on {}: {e} (set `prefer_external_pdftotext = true` in settings.toml to retry via pdftotext)",
                path.display()
            ))
        })?
    };
    Ok(ToolResult::success(text))
}

fn read_pdf_via_pdftotext(
    path: &Path,
    page_range: Option<(u32, u32)>,
) -> Result<ToolResult, ToolError> {
    let mut cmd = Command::new("pdftotext");
    cmd.arg("-layout");

    if let Some((start, end)) = page_range {
        cmd.arg("-f").arg(start.to_string());
        cmd.arg("-l").arg(end.to_string());
    }

    cmd.arg(path).arg("-"); // output to stdout
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Structured "binary unavailable" — only reachable when the
            // user explicitly opted into the external path. Hints back at
            // both the install command and the in-tree default.
            return ToolResult::json(&json!({
                "type": "binary_unavailable",
                "path": path.display().to_string(),
                "kind": "pdf",
                "reason": "pdftotext not installed (prefer_external_pdftotext = true in settings)",
                "hint": "install poppler (macOS: `brew install poppler`; Debian/Ubuntu: `apt install poppler-utils`) — or unset `prefer_external_pdftotext` to use the bundled pure-Rust extractor"
            }))
            .map_err(|e| {
                ToolError::execution_failed(format!("failed to serialize response: {e}"))
            });
        }
        Err(e) => {
            return Err(ToolError::execution_failed(format!(
                "failed to launch pdftotext: {e}"
            )));
        }
    };

    let output = child
        .wait_with_output()
        .map_err(|e| ToolError::execution_failed(format!("pdftotext failed to complete: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(ToolError::execution_failed(format!(
            "pdftotext failed (exit {:?}): {stderr}",
            output.status.code()
        )));
    }

    let text = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(ToolResult::success(text))
}

// === WriteFileTool ===

/// Tool for writing UTF-8 files to the workspace.
pub struct WriteFileTool;

#[async_trait]
impl ToolSpec for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Write content to a UTF-8 file in the workspace. Use this instead of heredocs (`cat <<EOF > file`) or `echo > file` in `exec_shell` — diffs render inline and approval is handled cleanly. Creates or overwrites; parent directories are auto-created."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write"
                }
            },
            "required": ["path", "content"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::WritesFiles,
            ToolCapability::Sandboxable,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Suggest
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let path_str = required_str(&input, "path")?;
        let file_content = required_str(&input, "content")?;

        let file_path = context.resolve_path(path_str)?;

        // Snapshot the existing contents (if any) before we overwrite — used
        // to render an inline diff in the tool result.
        let existed_before = file_path.exists();
        let prior_contents = if existed_before {
            fs::read_to_string(&file_path).unwrap_or_default()
        } else {
            String::new()
        };

        // Create parent directories if needed
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                ToolError::execution_failed(format!(
                    "Failed to create directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        fs::write(&file_path, file_content).map_err(|e| {
            ToolError::execution_failed(format!("Failed to write {}: {}", file_path.display(), e))
        })?;

        let display = file_path.display().to_string();
        let diff = make_unified_diff(&display, &prior_contents, file_content);
        let summary = if existed_before {
            format!("Wrote {} bytes to {}", file_content.len(), display)
        } else {
            format!("Created {} ({} bytes)", display, file_content.len())
        };
        let body = if diff.is_empty() {
            format!("{summary}\n(no changes)")
        } else {
            format!("{diff}\n{summary}")
        };

        // Append LSP diagnostics for the written file when enabled (#428).
        let diag_block = lsp_diagnostics_for_paths(context, &[file_path]).await;
        let full_body = if diag_block.is_empty() {
            body
        } else {
            format!("{body}\n{diag_block}")
        };

        Ok(ToolResult::success(full_body))
    }
}

// === EditFileTool ===

/// Tool for search/replace editing of files.
pub struct EditFileTool;

#[async_trait]
impl ToolSpec for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "Replace text in a single file via exact search/replace. Use this instead of `sed -i` in `exec_shell` for one unambiguous in-place edit. `search` matches exactly by default, including whitespace and indentation; set `fuzz: true` to tolerate leading-indentation differences. Returns a compact unified diff, not the full file. For structural, multi-block, or cross-file changes, use `apply_patch` or `write_file` instead."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file"
                },
                "search": {
                    "type": "string",
                    "description": "Exact text to search for, including whitespace, indentation, and newlines"
                },
                "replace": {
                    "type": "string",
                    "description": "Text to replace with"
                },
                "fuzz": {
                    "type": "boolean",
                    "description": "When true, tolerate leading whitespace differences on each searched line (default false)"
                }
            },
            "required": ["path", "search", "replace"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::WritesFiles,
            ToolCapability::Sandboxable,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Suggest
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let path_str = required_str(&input, "path")?;
        let search = required_str(&input, "search")?;
        let replace = required_str(&input, "replace")?;
        let fuzz = optional_bool(&input, "fuzz", false);

        if search == replace {
            return Err(ToolError::invalid_input(
                "search and replace are identical, no change intended",
            ));
        }

        let file_path = context.resolve_path(path_str)?;

        let contents = fs::read_to_string(&file_path).map_err(|e| {
            ToolError::execution_failed(format!("Failed to read {}: {}", file_path.display(), e))
        })?;

        let count = contents.matches(search).count();
        let (updated, count, fuzz_used) = if count == 0 && fuzz {
            let matches = leading_whitespace_fuzzy_matches(&contents, search);
            match matches.as_slice() {
                [] => {
                    return Err(ToolError::execution_failed(format!(
                        "Search string not found in {}",
                        file_path.display()
                    )));
                }
                [(start, end)] => {
                    let mut updated = contents.clone();
                    updated.replace_range(*start..*end, replace);
                    (updated, 1, true)
                }
                _ => {
                    return Err(ToolError::execution_failed(format!(
                        "Fuzzy search matched {} locations in {}; refine search text",
                        matches.len(),
                        file_path.display()
                    )));
                }
            }
        } else if count == 0 {
            return Err(ToolError::execution_failed(format!(
                "Search string not found in {}",
                file_path.display()
            )));
        } else {
            (contents.replace(search, replace), count, false)
        };

        fs::write(&file_path, &updated).map_err(|e| {
            ToolError::execution_failed(format!("Failed to write {}: {}", file_path.display(), e))
        })?;

        let display = file_path.display().to_string();
        let diff = make_unified_diff(&display, &contents, &updated);
        let summary = if count > 1 {
            format!(
                "Replaced {count} occurrence(s) in {display}\n\
                 Warning: multiple matches were replaced with the same substitution. \
                 Verify the result with read_file before proceeding."
            )
        } else {
            let fuzz_note = if fuzz_used {
                " (fuzzy indentation match)"
            } else {
                ""
            };
            format!("Replaced 1 occurrence in {display}{fuzz_note}")
        };
        let body = if diff.is_empty() {
            format!("{summary}\n(no textual changes)")
        } else {
            format!("{diff}\n{summary}")
        };

        // Append LSP diagnostics for the edited file when enabled (#428).
        let diag_block = lsp_diagnostics_for_paths(context, &[file_path]).await;
        let full_body = if diag_block.is_empty() {
            body
        } else {
            format!("{body}\n{diag_block}")
        };

        Ok(ToolResult::success(full_body))
    }
}

fn strip_line_leading_whitespace_with_map(input: &str) -> (String, Vec<usize>) {
    let mut normalized = String::with_capacity(input.len());
    let mut byte_map = Vec::with_capacity(input.len());
    let mut at_line_start = true;
    for (idx, ch) in input.char_indices() {
        if at_line_start && matches!(ch, ' ' | '\t') {
            continue;
        }
        normalized.push(ch);
        for _ in 0..ch.len_utf8() {
            byte_map.push(idx);
        }
        at_line_start = ch == '\n';
    }
    (normalized, byte_map)
}

fn line_start_before(input: &str, idx: usize) -> usize {
    input[..idx]
        .rfind('\n')
        .map_or(0, |newline| newline.saturating_add(1))
}

fn leading_whitespace_fuzzy_matches(contents: &str, search: &str) -> Vec<(usize, usize)> {
    let (normalized_contents, byte_map) = strip_line_leading_whitespace_with_map(contents);
    let (normalized_search, _) = strip_line_leading_whitespace_with_map(search);
    if normalized_search.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    let mut cursor = 0;
    while let Some(rel_idx) = normalized_contents[cursor..].find(&normalized_search) {
        let norm_start = cursor + rel_idx;
        let norm_end = norm_start + normalized_search.len();
        let Some(&mapped_start) = byte_map.get(norm_start) else {
            break;
        };
        let original_start = line_start_before(contents, mapped_start);
        let original_end = byte_map.get(norm_end).copied().unwrap_or(contents.len());
        matches.push((original_start, original_end));
        cursor = norm_start.saturating_add(1);
    }
    matches
}

// === ListDirTool ===

/// Tool for listing directory contents.
pub struct ListDirTool;

#[async_trait]
impl ToolSpec for ListDirTool {
    fn name(&self) -> &'static str {
        "list_dir"
    }

    fn description(&self) -> &'static str {
        "List entries in a directory relative to the workspace. Use this instead of `ls`, `ls -la`, or `find . -maxdepth 1` in `exec_shell` for directory listings."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path (default: .)"
                }
            },
            "required": []
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let path_str = optional_str(&input, "path").unwrap_or(".");
        let dir_path = context.resolve_path(path_str)?;

        let mut entries = Vec::new();

        for entry in fs::read_dir(&dir_path).map_err(|e| {
            ToolError::execution_failed(format!(
                "Failed to read directory {}: {}",
                dir_path.display(),
                e
            ))
        })? {
            let entry = entry.map_err(|e| ToolError::execution_failed(e.to_string()))?;
            let file_type = entry
                .file_type()
                .map_err(|e| ToolError::execution_failed(e.to_string()))?;

            entries.push(json!({
                "name": entry.file_name().to_string_lossy().to_string(),
                "is_dir": file_type.is_dir(),
            }));
        }

        ToolResult::json(&entries).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

// === Unit Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_read_file_tool() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // Create a test file
        let test_file = tmp.path().join("test.txt");
        fs::write(&test_file, "hello world").expect("write");

        let tool = ReadFileTool;
        let result = tool
            .execute(json!({"path": "test.txt"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert_eq!(result.content, "hello world");
    }

    #[test]
    fn parse_pages_arg_accepts_single_page() {
        assert_eq!(parse_pages_arg("3"), Some((3, 3)));
        assert_eq!(parse_pages_arg("  7  "), Some((7, 7)));
    }

    #[test]
    fn parse_pages_arg_accepts_range() {
        assert_eq!(parse_pages_arg("1-5"), Some((1, 5)));
        assert_eq!(parse_pages_arg("10-20"), Some((10, 20)));
        // Whitespace around either side of the dash is tolerated so
        // hand-typed `pages: "1 - 5"` still works.
        assert_eq!(parse_pages_arg(" 1 - 5 "), Some((1, 5)));
    }

    #[test]
    fn parse_pages_arg_rejects_invalid_ranges() {
        // Caller would otherwise feed `pdftotext -f 5 -l 1`, which
        // prints nothing — fail loudly so the model can re-issue.
        assert!(parse_pages_arg("5-1").is_none(), "end < start must reject");
        // 0-indexed pages aren't a thing in pdftotext; reject so the
        // caller doesn't get a confusing "no output" silent fail.
        assert!(
            parse_pages_arg("0").is_none(),
            "zero single-page must reject"
        );
        assert!(parse_pages_arg("0-3").is_none(), "zero start must reject");
        // Empty / whitespace-only / non-numeric inputs must reject.
        assert!(parse_pages_arg("").is_none());
        assert!(parse_pages_arg("   ").is_none());
        assert!(parse_pages_arg("abc").is_none());
        assert!(parse_pages_arg("3.5").is_none(), "floats must reject");
    }

    #[test]
    fn parse_pages_arg_rejects_half_open_ranges() {
        // Half-open ranges like `1-` or `-5` are almost certainly a
        // typo for `1-N`/`N` rather than intentional input. Reject
        // them rather than silently extending to u32::MAX or 0.
        assert!(parse_pages_arg("1-").is_none());
        assert!(parse_pages_arg("-5").is_none());
        assert!(parse_pages_arg("-").is_none());
    }

    #[test]
    fn parse_pages_arg_rejects_negative_numbers() {
        // u32::parse on a negative literal returns Err, so the
        // function reports `None` rather than wrapping into a giant
        // positive number — defensive but worth pinning.
        assert!(parse_pages_arg("-3-5").is_none());
    }

    #[tokio::test]
    async fn test_read_file_not_found() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let tool = ReadFileTool;
        let result = tool.execute(json!({"path": "nonexistent.txt"}), &ctx).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_file_small_file_returns_unwrapped_contents() {
        // Small files (≤ 200 lines AND ≤ 16KB, no explicit range) keep
        // the historical "return contents unchanged" behavior so
        // existing prompts don't suddenly see <file> tags appear.
        // Harvested from #1451 — pin the fast-path contract.
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let file = tmp.path().join("small.txt");
        fs::write(&file, "line 1\nline 2\nline 3\n").expect("write");
        let tool = ReadFileTool;
        let result = tool
            .execute(json!({ "path": "small.txt" }), &ctx)
            .await
            .expect("execute");
        assert!(result.success);
        assert_eq!(result.content, "line 1\nline 2\nline 3\n");
        assert!(
            !result.content.contains("<file"),
            "small-file fast path must not wrap output"
        );
    }

    #[tokio::test]
    async fn read_file_explicit_range_wraps_in_file_tag_with_one_based_lines() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let file = tmp.path().join("ranged.txt");
        let body: String = (1..=10).map(|n| format!("line {n}\n")).collect();
        fs::write(&file, &body).expect("write");
        let tool = ReadFileTool;
        let result = tool
            .execute(
                json!({ "path": "ranged.txt", "start_line": 3, "max_lines": 4 }),
                &ctx,
            )
            .await
            .expect("execute");
        assert!(result.success);
        assert!(
            result.content.contains("shown_lines=\"3-6\""),
            "1-based inclusive range must be reflected in shown_lines: {}",
            result.content
        );
        assert!(
            result.content.contains("next_start_line=\"7\""),
            "next_start_line must point one past the last shown line: {}",
            result.content
        );
        assert!(
            result.content.contains("     3│ line 3"),
            "rendered lines must start at the requested line number"
        );
        assert!(
            result.content.contains("     6│ line 6"),
            "rendered lines must end at the last in-range line"
        );
        assert!(
            !result.content.contains("     7│ line 7"),
            "lines past max_lines must be excluded"
        );
        assert!(result.content.contains("truncated=\"true\""));
    }

    #[tokio::test]
    async fn read_file_range_beyond_total_returns_no_content_sentinel() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let file = tmp.path().join("short.txt");
        fs::write(&file, "only\nthree\nlines\n").expect("write");
        let tool = ReadFileTool;
        let result = tool
            .execute(json!({ "path": "short.txt", "start_line": 99 }), &ctx)
            .await
            .expect("execute");
        assert!(
            result.success,
            "out-of-range must not raise — it's a sentinel"
        );
        assert!(result.content.contains("[NO CONTENT]"));
        assert!(result.content.contains("shown_lines=\"none\""));
        assert!(result.content.contains("truncated=\"false\""));
    }

    #[tokio::test]
    async fn read_file_rejects_zero_start_line_and_zero_max_lines() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        fs::write(tmp.path().join("any.txt"), "x\n").expect("write");
        let tool = ReadFileTool;
        let zero_start = tool
            .execute(json!({ "path": "any.txt", "start_line": 0 }), &ctx)
            .await;
        assert!(zero_start.is_err(), "start_line=0 must error (1-based)");
        let zero_max = tool
            .execute(json!({ "path": "any.txt", "max_lines": 0 }), &ctx)
            .await;
        assert!(zero_max.is_err(), "max_lines=0 must error");
    }

    #[tokio::test]
    async fn read_file_clamps_max_lines_to_hard_cap() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let file = tmp.path().join("bigish.txt");
        let body: String = (1..=600).map(|n| format!("L{n}\n")).collect();
        fs::write(&file, &body).expect("write");
        let tool = ReadFileTool;
        let result = tool
            .execute(json!({ "path": "bigish.txt", "max_lines": 5000 }), &ctx)
            .await
            .expect("execute");
        // Hard cap is 500 lines; line 500 must appear, line 501 must not.
        assert!(
            result.content.contains("   500│ L500"),
            "line 500 should be in the window (max_lines clamped to 500)"
        );
        assert!(
            !result.content.contains("   501│ L501"),
            "line 501 must be outside the clamped window"
        );
        assert!(result.content.contains("next_start_line=\"501\""));
        assert!(result.content.contains("truncated=\"true\""));
    }

    #[tokio::test]
    async fn read_file_large_file_without_range_uses_default_window() {
        // A file over 200 lines / 16KB with no explicit range still
        // gets the default window, not the unbounded raw content —
        // this is the entire point of the patch (token-budget control).
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let file = tmp.path().join("big.txt");
        let body: String = (1..=250).map(|n| format!("row {n}\n")).collect();
        fs::write(&file, &body).expect("write");
        let tool = ReadFileTool;
        let result = tool
            .execute(json!({ "path": "big.txt" }), &ctx)
            .await
            .expect("execute");
        assert!(result.content.contains("<file "));
        assert!(result.content.contains("shown_lines=\"1-200\""));
        assert!(result.content.contains("next_start_line=\"201\""));
        assert!(result.content.contains("     1│ row 1"));
        assert!(result.content.contains("   200│ row 200"));
        assert!(
            !result.content.contains("   201│ row 201"),
            "default max_lines=200 must hold"
        );
    }

    #[tokio::test]
    async fn test_read_file_missing_path() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let tool = ReadFileTool;
        let result = tool.execute(json!({}), &ctx).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("Failed to validate input: missing required field 'path'")
        );
    }

    #[test]
    fn pdf_detected_by_extension() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("paper.PDF");
        fs::write(&path, b"not really a pdf, but extension says yes").unwrap();
        assert!(is_pdf(&path).unwrap());
    }

    #[test]
    fn pdf_detected_by_magic_bytes_without_extension() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("blob");
        fs::write(&path, b"%PDF-1.7\nrest of bytes").unwrap();
        assert!(is_pdf(&path).unwrap());
    }

    #[test]
    fn non_pdf_not_detected() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("notes.txt");
        fs::write(&path, "hello").unwrap();
        assert!(!is_pdf(&path).unwrap());
    }

    #[test]
    fn pages_arg_parses_single_and_range() {
        assert_eq!(parse_pages_arg("5"), Some((5, 5)));
        assert_eq!(parse_pages_arg("1-10"), Some((1, 10)));
        assert_eq!(parse_pages_arg(" 3 - 7 "), Some((3, 7)));
        assert_eq!(parse_pages_arg("0"), None);
        assert_eq!(parse_pages_arg("10-3"), None);
        assert_eq!(parse_pages_arg(""), None);
        assert_eq!(parse_pages_arg("abc"), None);
    }

    /// Sample PDF shipped with the repo for parity tests against the
    /// pure-Rust extractor. 38 pages, born-digital LaTeX (arXiv 2512.24601).
    /// Path is workspace-root-relative because the fixture lives outside
    /// the tui crate.
    const SAMPLE_PDF_PATH: &str = "../../docs/2512.24601v2.pdf";

    fn sample_pdf_present() -> bool {
        std::path::Path::new(SAMPLE_PDF_PATH).exists()
    }

    #[test]
    fn read_pdf_via_pdf_extract_finds_known_title() {
        // Skip when the fixture isn't checked out (sparse clones, shallow
        // worktrees). Local dev + CI both have it.
        if !sample_pdf_present() {
            // Fixture not present (sparse / shallow checkout). Silent
            // skip — `cargo test` reports the same `ok` either way.
            return;
        }
        let path = std::path::PathBuf::from(SAMPLE_PDF_PATH);
        let result = read_pdf_via_pdf_extract(&path, None).expect("extract whole PDF");
        assert!(result.success);
        assert!(
            result.content.contains("Recursive Language Models"),
            "pdf-extract should recover the document title; got prefix {:?}",
            &result.content.chars().take(200).collect::<String>()
        );
    }

    #[test]
    fn read_pdf_via_pdf_extract_respects_pages_window() {
        if !sample_pdf_present() {
            // Fixture not present (sparse / shallow checkout). Silent
            // skip — `cargo test` reports the same `ok` either way.
            return;
        }
        let path = std::path::PathBuf::from(SAMPLE_PDF_PATH);
        let single = read_pdf_via_pdf_extract(&path, Some((1, 1))).expect("single page");
        let two = read_pdf_via_pdf_extract(&path, Some((1, 2))).expect("two pages");
        assert!(single.success);
        assert!(two.success);
        // A two-page slice must be at least as long as the one-page slice
        // (most documents have non-trivial body text past page 1).
        assert!(
            two.content.len() >= single.content.len(),
            "expected pages 1-2 ({} bytes) >= page 1 ({} bytes)",
            two.content.len(),
            single.content.len()
        );
        // Title text lives on page 1 — must survive the window crop.
        assert!(single.content.contains("Recursive Language Models"));
    }

    #[tokio::test]
    async fn read_file_pdf_path_uses_pdf_extract_by_default() {
        if !sample_pdf_present() {
            // Fixture not present (sparse / shallow checkout). Silent
            // skip — `cargo test` reports the same `ok` either way.
            return;
        }
        // The fixture lives outside the tui crate, so we point ToolContext
        // at the workspace root and read by relative path. This exercises
        // the full ReadFileTool::execute → is_pdf → read_pdf dispatch on
        // the bundled extractor (no pdftotext required on the test host).
        let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../");
        let ctx = ToolContext::new(workspace);
        let result = ReadFileTool
            .execute(json!({"path": "docs/2512.24601v2.pdf", "pages": "1"}), &ctx)
            .await
            .expect("execute");
        assert!(result.success);
        assert!(
            result.content.contains("Recursive Language Models"),
            "page-1 extraction must surface the title"
        );
    }

    /// Serialises tests that mutate `DEEPSEEK_CONFIG_PATH` so they don't
    /// race against each other — env vars are process-global and the
    /// settings loader inspects this var on every call.
    static DS_CONFIG_PATH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct ConfigPathEnvGuard {
        prior: Option<std::ffi::OsString>,
    }
    impl ConfigPathEnvGuard {
        fn capture() -> Self {
            Self {
                prior: std::env::var_os("DEEPSEEK_CONFIG_PATH"),
            }
        }
    }
    impl Drop for ConfigPathEnvGuard {
        fn drop(&mut self) {
            // Safety: scoped to test process; reverts to the captured value.
            match &self.prior {
                Some(v) => unsafe { std::env::set_var("DEEPSEEK_CONFIG_PATH", v) },
                None => unsafe { std::env::remove_var("DEEPSEEK_CONFIG_PATH") },
            }
        }
    }

    #[test]
    fn read_pdf_routes_to_pdftotext_when_setting_opted_in() {
        // Two concerns in one test: with `prefer_external_pdftotext = true`
        // the dispatch must (a) call pdftotext when present, and (b) return
        // the structured `binary_unavailable` response when pdftotext is
        // missing.
        // Sync test (calls `read_pdf` directly, not the async ReadFileTool
        // wrapper) so the env-var lock is never held across an `.await`.
        let _lock = DS_CONFIG_PATH_LOCK.lock().unwrap();
        let _guard = ConfigPathEnvGuard::capture();

        let tmp = tempdir().expect("tempdir");
        let config_dir = tmp.path().join("cfg");
        fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("config.toml");
        fs::write(&config_path, "").unwrap();
        // The sibling settings.toml is what Settings::load() reads.
        fs::write(
            config_dir.join("settings.toml"),
            "prefer_external_pdftotext = true\n",
        )
        .unwrap();
        // Safety: serialised by DS_CONFIG_PATH_LOCK; reverted by guard.
        unsafe {
            std::env::set_var("DEEPSEEK_CONFIG_PATH", &config_path);
        }

        let pdf_path = tmp.path().join("doc.pdf");
        fs::write(&pdf_path, b"%PDF-1.7\n%%EOF").unwrap();
        let outcome = read_pdf(&pdf_path, None);

        let pdftotext_present = Command::new("pdftotext")
            .arg("-v")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok();

        if pdftotext_present {
            // pdftotext on a stub `%PDF-1.7\n%%EOF` cannot find a real
            // trailer/xref table and fails with `exit 1`. That failure
            // text mentions pdftotext explicitly — proof we routed
            // through Poppler rather than falling back to the bundled
            // extractor. Validate by inspecting the error message.
            let err = outcome.expect_err("malformed PDF must surface the pdftotext error");
            let msg = err.to_string();
            assert!(
                msg.contains("pdftotext"),
                "error message must reference pdftotext; got {msg}"
            );
        } else {
            let result = outcome.expect("binary_unavailable is a structured success, not an Err");
            assert!(result.success);
            assert!(result.content.contains("binary_unavailable"));
            assert!(result.content.contains("pdftotext"));
            assert!(
                result.content.contains("prefer_external_pdftotext"),
                "hint must reference the opt-in flag the user set"
            );
        }
    }

    #[tokio::test]
    async fn test_write_file_tool() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let tool = WriteFileTool;
        let result = tool
            .execute(
                json!({"path": "output.txt", "content": "test content"}),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);
        // New file → "Created …" summary; the unified diff above the summary
        // primes the TUI's diff-aware renderer (#505).
        assert!(result.content.contains("Created"), "{}", result.content);
        assert!(result.content.contains("--- a/"), "{}", result.content);
        assert!(
            result.content.contains("+test content"),
            "{}",
            result.content
        );

        // Verify file was written
        let written = fs::read_to_string(tmp.path().join("output.txt")).expect("read");
        assert_eq!(written, "test content");
    }

    #[tokio::test]
    async fn test_write_file_creates_dirs() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let tool = WriteFileTool;
        let result = tool
            .execute(
                json!({"path": "subdir/nested/file.txt", "content": "nested content"}),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);

        // Verify nested file was created
        let written = fs::read_to_string(tmp.path().join("subdir/nested/file.txt")).expect("read");
        assert_eq!(written, "nested content");
    }

    #[tokio::test]
    async fn test_edit_file_tool() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // Create a file to edit
        let test_file = tmp.path().join("edit_me.txt");
        fs::write(&test_file, "hello world hello").expect("write");

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({"path": "edit_me.txt", "search": "hello", "replace": "hi"}),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("2 occurrence(s)"));
        assert!(
            result.content.contains("multiple matches were replaced"),
            "{}",
            result.content
        );
        // Inline diff (#505) — the unified diff lands above the summary
        // line so the TUI's diff-aware renderer kicks in.
        assert!(result.content.contains("--- a/"), "{}", result.content);
        assert!(
            result.content.contains("-hello world hello"),
            "{}",
            result.content
        );
        assert!(
            result.content.contains("+hi world hi"),
            "{}",
            result.content
        );

        // Verify edit was applied
        let edited = fs::read_to_string(&test_file).expect("read");
        assert_eq!(edited, "hi world hi");
    }

    #[tokio::test]
    async fn test_edit_file_single_match_has_no_multi_match_warning() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("single.txt");
        fs::write(&test_file, "hello world").expect("write");

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({"path": "single.txt", "search": "hello", "replace": "hi"}),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("Replaced 1 occurrence"));
        assert!(!result.content.contains("multiple matches were replaced"));
    }

    #[tokio::test]
    async fn test_edit_file_fuzz_tolerates_leading_whitespace() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("fuzzy.txt");
        fs::write(
            &test_file,
            "fn main() {\n    if true {\n        let value = 1;\n    }\n}\n",
        )
        .expect("write");

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({
                    "path": "fuzzy.txt",
                    "search": "if true {\n    let value = 1;\n}",
                    "replace": "    if true {\n        let value = 2;\n    }",
                    "fuzz": true
                }),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("fuzzy indentation match"));
        let edited = fs::read_to_string(&test_file).expect("read");
        assert_eq!(
            edited,
            "fn main() {\n    if true {\n        let value = 2;\n    }\n}\n"
        );
    }

    #[tokio::test]
    async fn test_edit_file_not_found() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // Create a file without the search string
        let test_file = tmp.path().join("no_match.txt");
        fs::write(&test_file, "foo bar baz").expect("write");

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({"path": "no_match.txt", "search": "hello", "replace": "hi"}),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_edit_file_rejects_identical_search_and_replace() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("same.txt");
        fs::write(&test_file, "a := \"foo\"").expect("write");

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({
                    "path": "same.txt",
                    "search": "a := \"foo\"",
                    "replace": "a := \"foo\""
                }),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("search and replace are identical"),
            "error must explain the no-op input: {err}"
        );
        let unchanged = fs::read_to_string(&test_file).expect("read");
        assert_eq!(unchanged, "a := \"foo\"");
    }

    /// #157 — When the model uses `replacement` instead of `replace`,
    /// the error should name the provided fields so the model can
    /// self-correct without a second round-trip.
    #[tokio::test]
    async fn test_edit_file_wrong_param_name_shows_provided_fields() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("test.txt");
        fs::write(&test_file, "hello world").expect("write");

        let tool = EditFileTool;
        // Model uses `replacement` instead of `replace`.
        let result = tool
            .execute(
                json!({"path": "test.txt", "search": "hello", "replacement": "hi"}),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        // The error must name both the missing field AND the provided ones.
        assert!(
            err.contains("missing required field 'replace'"),
            "error must name the missing field: {err}"
        );
        assert!(
            err.contains("Input provided:") || err.contains("provided:"),
            "error must list the fields the model did supply: {err}"
        );
    }

    #[tokio::test]
    async fn test_list_dir_tool() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // Create some files and directories
        fs::write(tmp.path().join("file1.txt"), "").expect("write");
        fs::write(tmp.path().join("file2.txt"), "").expect("write");
        fs::create_dir(tmp.path().join("subdir")).expect("mkdir");

        let tool = ListDirTool;
        let result = tool.execute(json!({}), &ctx).await.expect("execute");

        assert!(result.success);
        assert!(result.content.contains("file1.txt"));
        assert!(result.content.contains("file2.txt"));
        assert!(result.content.contains("subdir"));
        assert!(result.content.contains("\"is_dir\": true"));
    }

    #[tokio::test]
    async fn test_list_dir_with_path() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // Create a subdirectory with files
        let subdir = tmp.path().join("mydir");
        fs::create_dir(&subdir).expect("mkdir");
        fs::write(subdir.join("nested.txt"), "").expect("write");

        let tool = ListDirTool;
        let result = tool
            .execute(json!({"path": "mydir"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("nested.txt"));
    }

    #[test]
    fn test_read_file_tool_properties() {
        let tool = ReadFileTool;
        assert_eq!(tool.name(), "read_file");
        assert!(tool.is_read_only());
        assert!(tool.is_sandboxable());
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Auto);
    }

    #[test]
    fn test_write_file_tool_properties() {
        let tool = WriteFileTool;
        assert_eq!(tool.name(), "write_file");
        assert!(!tool.is_read_only());
        assert!(tool.is_sandboxable());
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Suggest);
    }

    #[test]
    fn test_edit_file_tool_properties() {
        let tool = EditFileTool;
        assert_eq!(tool.name(), "edit_file");
        assert!(!tool.is_read_only());
        assert!(tool.is_sandboxable());
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Suggest);
        assert!(tool.description().contains("exact search/replace"));
        assert!(tool.description().contains("structural"));
    }

    #[test]
    fn test_list_dir_tool_properties() {
        let tool = ListDirTool;
        assert_eq!(tool.name(), "list_dir");
        assert!(tool.is_read_only());
        assert!(tool.is_sandboxable());
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Auto);
    }

    #[test]
    fn test_parallel_support_flags() {
        let read_tool = ReadFileTool;
        let list_tool = ListDirTool;
        let write_tool = WriteFileTool;

        assert!(read_tool.supports_parallel());
        assert!(list_tool.supports_parallel());
        assert!(!write_tool.supports_parallel());
    }

    #[test]
    fn test_input_schemas() {
        // Verify all tools have valid JSON schemas
        let read_schema = ReadFileTool.input_schema();
        assert!(read_schema.get("type").is_some());
        assert!(read_schema.get("properties").is_some());

        let write_schema = WriteFileTool.input_schema();
        let required = write_schema
            .get("required")
            .and_then(|value| value.as_array())
            .expect("write schema should include required array");
        assert!(required.iter().any(|v| v.as_str() == Some("path")));
        assert!(required.iter().any(|v| v.as_str() == Some("content")));

        let edit_schema = EditFileTool.input_schema();
        let required = edit_schema
            .get("required")
            .and_then(|value| value.as_array())
            .expect("edit schema should include required array");
        assert_eq!(required.len(), 3);
        let search_desc = edit_schema["properties"]["search"]["description"]
            .as_str()
            .expect("search description");
        assert!(search_desc.contains("Exact text"));
        assert!(search_desc.contains("whitespace"));

        let list_schema = ListDirTool.input_schema();
        let required = list_schema
            .get("required")
            .and_then(|value| value.as_array())
            .expect("list schema should include required array");
        assert!(required.is_empty()); // path is optional
    }
}
