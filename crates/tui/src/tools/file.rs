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
use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// === ReadFileTool ===

fn canonical_path_for_credential_guard(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    })
}

fn config_backup_path_for_credential_guard(config_path: &Path) -> PathBuf {
    let mut file_name = config_path
        .file_name()
        .map(std::ffi::OsString::from)
        .unwrap_or_else(|| std::ffi::OsString::from(codewhale_config::CONFIG_FILE_NAME));
    file_name.push(".bak");
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(file_name)
}

fn is_config_or_backup(candidate: &Path, config_path: &Path) -> bool {
    let config_path = canonical_path_for_credential_guard(config_path);
    let backup_path =
        canonical_path_for_credential_guard(&config_backup_path_for_credential_guard(&config_path));
    candidate == config_path || candidate == backup_path
}

/// Return whether `read_file` must refuse a CodeWhale-owned credential file.
///
/// This is deliberately scoped to the active config, the two conventional
/// config locations (including one-time backups), and CodeWhale's file-backed
/// secret-store directories. Other dotfiles remain readable. Model-bound
/// redaction is still required because shell tools can read these files and
/// arbitrary commands can print credentials without reading a file at all.
fn is_codewhale_credential_path(path: &Path) -> bool {
    let candidate = canonical_path_for_credential_guard(path);

    if let Ok(active_config) = codewhale_config::resolve_config_path(None)
        && is_config_or_backup(&candidate, &active_config)
    {
        return true;
    }

    let roots = [
        codewhale_config::codewhale_home(),
        codewhale_config::legacy_deepseek_home(),
    ];
    for root in roots.into_iter().flatten() {
        if is_config_or_backup(&candidate, &root.join(codewhale_config::CONFIG_FILE_NAME)) {
            return true;
        }

        let secrets_dir = canonical_path_for_credential_guard(&root.join("secrets"));
        if candidate.starts_with(secrets_dir) {
            return true;
        }
    }

    false
}

/// Tool for reading UTF-8 files from the workspace.
pub struct ReadFileTool;

#[async_trait]
impl ToolSpec for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn model_visible(&self) -> bool {
        false
    }

    fn description(&self) -> &'static str {
        "Read a UTF-8 file from the workspace. Use this instead of `cat`, `head`, `tail`, or `sed -n '..p'` in `exec_shell` — it's faster, sandbox-aware, and skips the approval prompt. Plain text is returned as-is and records the file snapshot required before `edit_file` will make a narrow in-place edit. CodeWhale config files and file-backed credential stores cannot be read with this tool; use `codewhale config list` or `codewhale auth status` for safe inspection. PDFs are text-extracted when the optional `pdftotext` executable (Poppler) is installed. Image screenshots are OCR-extracted when local OCR is available. Cannot read other non-PDF binaries.\n\nFor large files, use `start_line` and `max_lines` to read in chunks. By default, returns at most 200 lines (~16KB). If `truncated=\"true\"` in the response, use `next_start_line` to continue reading. For PDFs, use `pages` instead — `start_line`/`max_lines` only apply to text files."
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
        let path_str = required_str(&input, "path")?;
        let file_path = context.resolve_path(path_str)?;
        if is_codewhale_credential_path(&file_path) {
            return Err(ToolError::permission_denied(
                "read_file cannot expose CodeWhale configuration or credential-store files; use `codewhale config list` or `codewhale auth status` for safe inspection",
            ));
        }
        let pages = optional_str(&input, "pages");

        if let Some(result) = read_pdf_if_detected(
            &file_path,
            pages,
            super::pdf::PdfTextCommand::system(context.cancel_token.as_ref()),
        )
        .await?
        {
            return Ok(result);
        }
        if is_image_for_ocr(&file_path) {
            return read_image_via_ocr(&file_path, path_str);
        }

        // Open before parameter parsing so a missing file keeps the
        // historical "Failed to read …" error shape regardless of the other
        // arguments.
        let file = fs::File::open(&file_path).map_err(|e| {
            ToolError::execution_failed(format!("Failed to read {}: {}", file_path.display(), e))
        })?;
        let file_bytes = file.metadata().map(|meta| meta.len()).unwrap_or(u64::MAX);

        let explicit_range = input
            .get("start_line")
            .or_else(|| input.get("max_lines"))
            .is_some();

        // Small-file fast path. Only applies when the caller didn't pass an
        // explicit range — otherwise an explicit `start_line = 5` on a
        // tiny file would silently ignore the request.
        if !explicit_range && file_bytes <= SMALL_FILE_BYTES as u64 {
            drop(file);
            let contents = fs::read_to_string(&file_path).map_err(|e| {
                ToolError::execution_failed(format!(
                    "Failed to read {}: {}",
                    file_path.display(),
                    e
                ))
            })?;
            context.note_file_read(&file_path);

            let total_lines = contents.lines().count();
            if total_lines <= SMALL_FILE_LINES {
                return Ok(ToolResult::success(contents));
            }

            // Small in bytes but too many lines: render the default window
            // straight from the in-memory contents.
            let window: Vec<String> = contents
                .lines()
                .take(DEFAULT_READ_LINES)
                .map(str::to_string)
                .collect();
            return Ok(render_line_window(
                path_str,
                &window,
                total_lines,
                1,
                DEFAULT_READ_LINES,
            ));
        }

        let start_line = match input.get("start_line").and_then(Value::as_u64) {
            Some(0) => {
                return Err(ToolError::invalid_input(
                    "start_line must be 1-based and greater than 0".to_string(),
                ));
            }
            Some(v) => usize::try_from(v).map_err(|_| {
                ToolError::invalid_input(
                    "start_line exceeds platform addressable range".to_string(),
                )
            })?,
            None => 1,
        };

        let max_lines = match input.get("max_lines").and_then(Value::as_u64) {
            Some(0) => {
                return Err(ToolError::invalid_input(
                    "max_lines must be greater than 0".to_string(),
                ));
            }
            Some(v) => {
                let converted = usize::try_from(v).map_err(|_| {
                    ToolError::invalid_input(
                        "max_lines exceeds platform addressable range".to_string(),
                    )
                })?;
                std::cmp::min(converted, HARD_MAX_READ_LINES)
            }
            None => DEFAULT_READ_LINES,
        };

        // Bounded read for ranged/large files: skip and take lines through a
        // BufReader instead of materializing the whole file. The stream still
        // runs to EOF so the total line count and whole-file UTF-8 validation
        // match the historical read_to_string behavior.
        let (window, total_lines) =
            read_window_streaming(file, start_line, max_lines).map_err(|e| {
                ToolError::execution_failed(format!(
                    "Failed to read {}: {}",
                    file_path.display(),
                    e
                ))
            })?;
        context.note_file_read(&file_path);

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

        Ok(render_line_window(
            path_str,
            &window,
            total_lines,
            start_line,
            max_lines,
        ))
    }
}

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

/// Stream a line window out of `file`: skip `start_line - 1` lines, collect
/// up to `max_lines`, then keep counting (and validating UTF-8) to EOF.
/// Returns the collected window plus the total line count. Only the window
/// is ever held in memory.
fn read_window_streaming(
    file: fs::File,
    start_line: usize,
    max_lines: usize,
) -> std::io::Result<(Vec<String>, usize)> {
    use std::io::BufRead;

    let mut reader = std::io::BufReader::new(file);
    let mut raw: Vec<u8> = Vec::new();
    let mut window: Vec<String> = Vec::new();
    let mut total_lines = 0usize;
    let start_idx = start_line - 1;

    loop {
        raw.clear();
        let n = reader.read_until(b'\n', &mut raw)?;
        if n == 0 {
            break;
        }
        // Mirror `str::lines`: strip the trailing '\n', and a '\r' only when
        // it directly precedes that '\n'.
        let mut end = raw.len();
        if raw[..end].ends_with(b"\n") {
            end -= 1;
            if raw[..end].ends_with(b"\r") {
                end -= 1;
            }
        }
        // Validate every line so invalid UTF-8 anywhere in the file fails
        // exactly like the previous whole-file read_to_string did.
        let line = std::str::from_utf8(&raw[..end]).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            )
        })?;
        if total_lines >= start_idx && window.len() < max_lines {
            window.push(line.to_string());
        }
        total_lines += 1;
    }

    Ok((window, total_lines))
}

/// Render a collected line window into the `<file …>` wrapper used for
/// ranged/large reads. `window` must hold the lines for
/// `start_line..start_line + max_lines` (clamped to EOF).
fn render_line_window(
    path_str: &str,
    window: &[String],
    total_lines: usize,
    start_line: usize,
    max_lines: usize,
) -> ToolResult {
    let zero_based_start = start_line - 1;
    let zero_based_end = std::cmp::min(zero_based_start + max_lines, total_lines);
    let shown_first = start_line;
    let shown_last = zero_based_end; // 1-based inclusive line number of the last shown line

    let mut numbered = String::new();
    for (offset, line) in window.iter().enumerate() {
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

    ToolResult::success(output)
}

fn read_image_via_ocr(path: &Path, requested_path: &str) -> Result<ToolResult, ToolError> {
    let text = crate::tools::image_ocr::ocr_image_path(path)?;
    Ok(ToolResult::success(format!(
        "<image_ocr path=\"{requested_path}\">\n{text}\n</image_ocr>"
    )))
}

/// Detect an existing PDF by extension or by sniffing `%PDF` magic bytes.
fn is_pdf(path: &Path) -> Result<bool, ToolError> {
    let extension_matches = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
    let mut file = fs::File::open(path).map_err(|error| {
        ToolError::execution_failed(format!("Failed to read {}: {error}", path.display()))
    })?;
    if extension_matches {
        return Ok(true);
    }
    let mut buf = [0u8; 4];
    use std::io::Read;
    Ok(file.read_exact(&mut buf).is_ok() && &buf == b"%PDF")
}

fn is_image_for_ocr(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "tif" | "tiff" | "bmp"
            )
        })
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

/// Clean PDF-extracted text for TUI display: collapse consecutive blank
/// lines (more than 1 becomes 1), replace NUL bytes with U+FFFD, replace
/// non-breaking spaces with regular spaces, and trim trailing whitespace
/// on each line. Produces output that won't clutter the transcript with
/// vertical gaps or invisible control characters.
fn clean_pdf_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut blank_run = 0usize;
    let mut any_content = false;
    for line in raw.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_run = blank_run.saturating_add(1);
            if blank_run <= 1 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            any_content = true;
            // Push cleaned characters directly — avoids a per-line
            // temporary String allocation.
            for c in trimmed.chars() {
                match c {
                    '\0' => out.push('\u{FFFD}'),
                    '\u{A0}' => out.push(' '),
                    other => out.push(other),
                }
            }
            out.push('\n');
        }
    }
    // Trim leading blank lines only — don't use str::trim() which
    // would also strip intentional indentation (e.g. centred titles).
    if any_content {
        let start = out.find(|c: char| c != '\n').unwrap_or(0);
        // Walk back from end to find the last non-newline character.
        let end = out.rfind(|c: char| c != '\n').map_or(out.len(), |i| {
            i + out[i..].chars().next().map_or(1, |c| c.len_utf8())
        });
        out[start..end].to_string()
    } else {
        String::new()
    }
}

async fn read_pdf_if_detected(
    path: &Path,
    pages: Option<&str>,
    command: super::pdf::PdfTextCommand<'_>,
) -> Result<Option<ToolResult>, ToolError> {
    if !is_pdf(path)? {
        return Ok(None);
    }
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

    read_pdf_with_command(path, page_range, command)
        .await
        .map(Some)
}

async fn read_pdf_with_command(
    path: &Path,
    page_range: Option<(u32, u32)>,
    command: super::pdf::PdfTextCommand<'_>,
) -> Result<ToolResult, ToolError> {
    let text = super::pdf::extract_path(path, page_range, command)
        .await
        .map_err(super::pdf::into_tool_error)?;
    Ok(ToolResult::success(clean_pdf_text(&text)))
}

// === WriteFileTool ===

/// Tool for writing UTF-8 files to the workspace.
pub struct WriteFileTool;

#[async_trait]
impl ToolSpec for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn model_visible(&self) -> bool {
        false
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

        crate::utils::write_atomic_workspace(&file_path, file_content.as_bytes()).map_err(|e| {
            ToolError::execution_failed(format!("Failed to write {}: {}", file_path.display(), e))
        })?;
        context.note_file_read(&file_path);

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

        let outcome = if existed_before { "updated" } else { "created" };
        // Keep the execution-owned receipt workspace-relative even though the
        // legacy model-facing output above retains its resolved-path wording.
        let receipt_diff = make_unified_diff(path_str, &prior_contents, file_content);
        Ok(ToolResult::success(full_body).with_metadata(json!({
            "event": "file.mutation",
            "mutation": {
                "diff": receipt_diff,
                "files": [{ "path": path_str, "outcome": outcome }],
                "renames": []
            }
        })))
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

    fn model_visible(&self) -> bool {
        false
    }

    fn description(&self) -> &'static str {
        "Replace text in a single file via exact search/replace after the file has been read with `read_file` in this session. Use this instead of `sed -i` in `exec_shell` for one unambiguous in-place edit. `search` must match exactly one location by default; when no exact match is found the tool retries with leading-whitespace-tolerant fuzzy matching automatically. The optional `fuzz` parameter is accepted for backward compatibility and is no longer needed. Returns a compact unified diff, not the full file. For structural, multi-block, or cross-file changes, use `apply_patch` or `write_file` instead."
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
                    "description": "Deprecated: fuzzy fallback is now automatic. Accepted for backward compatibility but ignored."
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
        let _fuzz = optional_bool(&input, "fuzz", false);

        if search == replace {
            // #5003 — long-text edits repeatedly failed here because the model
            // generated a `replace` identical to `search`. A bare "no change"
            // message gave no hint of the root cause, so the model retried the
            // same broken call. Spell out the failure and the recovery path.
            let char_count = search.chars().count();
            let line_count = search.lines().count();
            return Err(ToolError::invalid_input(format!(
                "search and replace are identical ({char_count} chars, {line_count} lines), so no change is possible. This usually means `replace` was copied verbatim from `search` instead of carrying the intended edits. Recovery: re-read the file with read_file, then retry with a `replace` that is genuinely different from `search`; for large multi-line rewrites prefer apply_patch with a unified diff."
            )));
        }
        if search.is_empty() {
            return Err(ToolError::invalid_input("search must not be empty"));
        }
        if let Some(reason) = edit_payload_looks_corrupted(search, replace) {
            return Err(ToolError::invalid_input(format!(
                "edit_file refused corrupted payload: {reason}. Recovery: re-read the file and retry with a complete replace (or use apply_patch for brace-heavy multi-line edits)."
            )));
        }

        let file_path = context.resolve_path(path_str)?;
        context.require_fresh_file_read(&file_path, path_str)?;

        let contents = fs::read_to_string(&file_path).map_err(|e| {
            ToolError::execution_failed(format!("Failed to read {}: {}", file_path.display(), e))
        })?;

        // Models provide LF newlines even when the file on disk uses CRLF.
        // Match in a newline-normalized view, while retaining the sparse
        // positions where CR bytes were removed so only the original span is
        // replaced and the rest of the file stays byte-for-byte untouched.
        let (normalized_contents, crlf_positions) = normalize_crlf_with_positions(&contents);
        let normalized_search = normalize_crlf(search);
        let mut exact_ranges = normalized_contents
            .match_indices(normalized_search.as_ref())
            .map(|(start, matched)| (start, start + matched.len()));
        let first_exact_match = exact_ranges
            .next()
            .map(|range| map_normalized_range(range, crlf_positions.as_deref()));
        let exact_count = usize::from(first_exact_match.is_some()) + exact_ranges.count();

        let ((match_start, match_end), fuzz_kind) = if exact_count == 0 {
            // First fallback: tolerate indentation differences.
            let indent_matches = map_normalized_ranges(
                leading_whitespace_fuzzy_matches(
                    normalized_contents.as_ref(),
                    normalized_search.as_ref(),
                ),
                crlf_positions.as_deref(),
            );
            match indent_matches.as_slice() {
                [(start, end)] => ((*start, *end), Some("indentation")),
                [] => {
                    // Second fallback: tolerate typographic-punctuation
                    // drift (smart quotes, em-dashes, NBSP). Picks up the
                    // copy-paste failure mode where a browser/chat client
                    // silently substituted Unicode punctuation in for the
                    // ASCII the file actually contains.
                    let punct_matches = map_normalized_ranges(
                        punctuation_normalized_matches(
                            normalized_contents.as_ref(),
                            normalized_search.as_ref(),
                        ),
                        crlf_positions.as_deref(),
                    );
                    match punct_matches.as_slice() {
                        [] => {
                            // #5003 — the model could not tell why its search
                            // missed; show the first lines of the search text
                            // so it can compare against the file's contents.
                            return Err(ToolError::execution_failed(format!(
                                "Search string not found in {}. The search text starts with:\n{}\nRecovery: call read_file with path=\"{path_str}\" to inspect the current contents, then retry with a search string copied from the file.",
                                file_path.display(),
                                preview_search_for_error(search),
                            )));
                        }
                        [(start, end)] => ((*start, *end), Some("punctuation")),
                        _ => {
                            return Err(ToolError::execution_failed(format!(
                                "edit_file search is non-unique after punctuation normalization: matched {} locations in {}. Recovery: call read_file with path=\"{path_str}\" and retry with surrounding lines that make the search unique.",
                                punct_matches.len(),
                                file_path.display()
                            )));
                        }
                    }
                }
                _ => {
                    return Err(ToolError::execution_failed(format!(
                        "edit_file search is non-unique after indentation normalization: matched {} locations in {}. Recovery: call read_file with path=\"{path_str}\" and retry with surrounding lines that make the search unique.",
                        indent_matches.len(),
                        file_path.display()
                    )));
                }
            }
        } else if exact_count > 1 {
            return Err(ToolError::execution_failed(format!(
                "edit_file search is non-unique: matched {} locations in {}. \
                 Recovery: call read_file with path=\"{path_str}\" and retry with surrounding lines that make the search unique.",
                exact_count,
                file_path.display()
            )));
        } else {
            let Some((start, end)) = first_exact_match else {
                return Err(ToolError::execution_failed(
                    "edit_file internal range accounting failed — refusing write",
                ));
            };
            let fuzz_kind = (&contents[start..end] != search).then_some("line endings");
            ((start, end), fuzz_kind)
        };

        let effective_replace =
            normalize_replacement_line_endings(replace, crlf_positions.is_some());
        let mut updated = contents.clone();
        updated.replace_range(match_start..match_end, &effective_replace);
        if updated == contents {
            return Err(ToolError::invalid_input(
                "search and replace resolve to identical file contents after line-ending normalization, no change intended",
            ));
        }

        if let Some(reason) = invalid_preprocessor_edit(&file_path, &contents, &updated) {
            return Err(ToolError::invalid_input(format!(
                "edit_file refused corrupted payload: {reason}. Recovery: re-read the file and retry with a complete replace (or use apply_patch for brace-heavy multi-line edits)."
            )));
        }

        // Fidelity: the intended replace text must appear in the updated buffer
        // (empty replace is a valid deletion). Catches host/tool bridges that
        // claim success after mangling the payload.
        if !effective_replace.is_empty() && !updated.contains(&effective_replace) {
            return Err(ToolError::execution_failed(
                "edit_file internal fidelity check failed: replace text missing from updated buffer — refusing write",
            ));
        }

        crate::utils::write_atomic_workspace(&file_path, updated.as_bytes()).map_err(|e| {
            ToolError::execution_failed(format!("Failed to write {}: {}", file_path.display(), e))
        })?;
        context.note_file_read(&file_path);

        let display = file_path.display().to_string();
        let diff = make_unified_diff(&display, &contents, &updated);
        let fuzz_note = match fuzz_kind {
            Some("indentation") => " (fuzzy indentation match)",
            Some("punctuation") => {
                " (fuzzy punctuation match — typographic quotes/dashes normalized)"
            }
            Some("line endings") => " (CRLF/LF-normalized match)",
            Some(other) => other,
            None => "",
        };
        let summary = format!("Replaced 1 occurrence in {display}{fuzz_note}");
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

        // The structured receipt uses the requested workspace path instead of
        // the resolved host path retained by the legacy model-facing body.
        let receipt_diff = make_unified_diff(path_str, &contents, &updated);
        Ok(ToolResult::success(full_body).with_metadata(json!({
            "event": "file.mutation",
            "mutation": {
                "diff": receipt_diff,
                "files": [{ "path": path_str, "outcome": "updated" }],
                "renames": []
            }
        })))
    }
}

/// Detect catastrophic argument corruption of brace-structured edits.
///
/// Models (and some host XML/JSON bridges) occasionally deliver a `replace`
/// payload where a multi-line `{ ... }` block collapsed to empty `[]` or `{}`
/// while `search` still contains the full structured original. Writing that
/// would brick Rust match arms / JSON objects. Fail closed with recovery text
/// instead of applying the mangled payload (dogfood 2026-07-24).
fn edit_payload_looks_corrupted(search: &str, replace: &str) -> Option<&'static str> {
    let search_curly_open = search.matches('{').count();
    let search_curly_close = search.matches('}').count();
    let replace_curly_open = replace.matches('{').count();
    let replace_curly_close = replace.matches('}').count();
    let replace_square_open = replace.matches('[').count();
    let replace_square_close = replace.matches(']').count();

    if replace_curly_open != replace_curly_close {
        return Some(
            "replace has unbalanced `{`/`}` braces — the tool-call arguments were likely truncated or mangled before apply",
        );
    }
    if search_curly_open != search_curly_close {
        return Some(
            "search has unbalanced `{`/`}` braces — copy the exact file span again with balanced braces",
        );
    }
    if replace_square_open != replace_square_close {
        return Some(
            "replace has unbalanced `[`/`]` brackets — the tool-call arguments were likely truncated or mangled before apply",
        );
    }

    // Dogfood 2026-07-24: multi-line Rust `{ ... }` search collapsed into an
    // empty `[ ... ]` placeholder (host/XML arg bridge ate the brace body).
    // Count non-whitespace, non-bracket payload chars; a near-empty bracket
    // husk with a tiny tail like `=> {},` is the signature of that failure.
    if search_curly_open >= 1 && replace_square_open >= 1 {
        let significant = replace
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '[' && *c != ']')
            .count();
        if significant <= 12 {
            return Some(
                "replace collapsed a brace-structured search block into an empty/placeholder bracket span — refusing to brick the file; re-send the full replace text (prefer apply_patch for multi-line match arms)",
            );
        }
    }

    // Extreme shrinkage with lost braces (e.g. 200-char match arm -> tiny stub).
    if search.len() >= 80
        && replace.len() * 8 < search.len()
        && search_curly_open >= 1
        && replace_curly_open < search_curly_open
    {
        return Some(
            "replace is drastically shorter than search and lost brace structure — likely argument mangling; refuse apply",
        );
    }

    None
}

const PREPROCESSOR_CONDITIONAL_ERROR: &str = "replace would change the C/C++ preprocessor conditional balance (#if/#ifdef/#ifndef vs #endif) — the search or replace text is missing a matching directive; copy the complete block including both its opening and closing directives";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PreprocessorConditionalDebt {
    orphaned_closes: usize,
    unclosed_opens: usize,
}

impl PreprocessorConditionalDebt {
    fn total(self) -> usize {
        self.orphaned_closes + self.unclosed_opens
    }
}

/// Reject an edit only when it introduces new conditional-structure damage in
/// a file whose extension identifies it as C-family source. The whole file is
/// checked before and after the edit: complete block insertion/removal is safe,
/// while an orphaned opener or closer increases the structural debt. Existing
/// debt may be preserved or reduced so this guard never prevents a repair.
fn invalid_preprocessor_edit(path: &Path, before: &str, after: &str) -> Option<&'static str> {
    if !is_c_family_source(path) {
        return None;
    }

    let before_debt = preprocessor_conditional_debt(before);
    let after_debt = preprocessor_conditional_debt(after);
    let safe = after_debt == before_debt
        || after_debt.total() == 0
        || after_debt.total() < before_debt.total();

    (!safe).then_some(PREPROCESSOR_CONDITIONAL_ERROR)
}

fn is_c_family_source(path: &Path) -> bool {
    const EXTENSIONS: &[&str] = &[
        "c", "cc", "cp", "cpp", "cxx", "h", "h++", "hh", "hpp", "hxx", "inl", "ipp", "ixx", "m",
        "mm", "tpp", "cu", "cuh", "cppm",
    ];

    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            EXTENSIONS
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

/// Measure unmatched preprocessor conditionals across an entire source file.
/// Tracking nesting (instead of comparing span-level tuple counts) also catches
/// an `#endif` moved before its opener. Whitespace between `#` and the directive
/// name is accepted, as it is by C preprocessors.
fn preprocessor_conditional_debt(text: &str) -> PreprocessorConditionalDebt {
    let mut depth = 0usize;
    let mut orphaned_closes = 0usize;

    for line in text.lines() {
        match preprocessor_directive(line) {
            Some("if" | "ifdef" | "ifndef") => depth += 1,
            Some("endif") if depth == 0 => orphaned_closes += 1,
            Some("endif") => depth -= 1,
            _ => {}
        }
    }

    PreprocessorConditionalDebt {
        orphaned_closes,
        unclosed_opens: depth,
    }
}

fn preprocessor_directive(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix('#')?.trim_start();
    let name_end = rest
        .find(|character: char| !character.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    (name_end > 0).then_some(&rest[..name_end])
}

/// Build a short, line-truncated preview of a (possibly very long) search
/// payload for error messages, so the model can compare what it searched for
/// against the file's actual contents without the error message ballooning.
fn preview_search_for_error(search: &str) -> String {
    const MAX_PREVIEW_LINES: usize = 3;
    const MAX_PREVIEW_LINE_LEN: usize = 80;
    search
        .lines()
        .take(MAX_PREVIEW_LINES)
        .map(|line| {
            if line.chars().count() > MAX_PREVIEW_LINE_LEN {
                let mut truncated: String = line.chars().take(MAX_PREVIEW_LINE_LEN).collect();
                truncated.push_str("...");
                truncated
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Normalize Windows CRLF pairs to LF while retaining the normalized byte
/// positions where a `\r` was removed. Lone carriage returns are preserved.
/// Inputs without CRLF are borrowed and use identity offsets.
///
/// A normalized boundary maps back to the original by adding the number of
/// removed CR bytes strictly before it. At the normalized newline itself that
/// excludes the current CR, so the start maps to `\r`; after the newline (or
/// at EOF) it includes that CR and spans the full pair.
fn normalize_crlf(input: &str) -> Cow<'_, str> {
    if input.contains("\r\n") {
        Cow::Owned(input.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(input)
    }
}

fn normalize_crlf_with_positions(input: &str) -> (Cow<'_, str>, Option<Vec<usize>>) {
    if !input.contains("\r\n") {
        return (Cow::Borrowed(input), None);
    }

    let mut normalized = String::with_capacity(input.len());
    let mut crlf_positions = Vec::new();
    let mut chars = input.char_indices().peekable();

    while let Some((_, ch)) = chars.next() {
        if ch == '\r' && matches!(chars.peek(), Some((_, '\n'))) {
            let _ = chars.next();
            crlf_positions.push(normalized.len());
            normalized.push('\n');
            continue;
        }

        normalized.push(ch);
    }

    (Cow::Owned(normalized), Some(crlf_positions))
}

fn map_normalized_range(
    (start, end): (usize, usize),
    crlf_positions: Option<&[usize]>,
) -> (usize, usize) {
    let Some(crlf_positions) = crlf_positions else {
        return (start, end);
    };
    let map_boundary =
        |offset| offset + crlf_positions.partition_point(|position| *position < offset);
    (map_boundary(start), map_boundary(end))
}

fn map_normalized_ranges(
    ranges: impl IntoIterator<Item = (usize, usize)>,
    crlf_positions: Option<&[usize]>,
) -> Vec<(usize, usize)> {
    ranges
        .into_iter()
        .map(|range| map_normalized_range(range, crlf_positions))
        .collect()
}

/// Convert model-provided replacement newlines to the base file's convention.
/// Fold CRLF first so an already-CRLF payload never becomes `\r\r\n`.
fn normalize_replacement_line_endings(replace: &str, use_crlf: bool) -> String {
    let lf = replace.replace("\r\n", "\n");
    if use_crlf {
        lf.replace('\n', "\r\n")
    } else {
        lf
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

fn next_char_boundary(input: &str, idx: usize) -> usize {
    if idx >= input.len() {
        return input.len();
    }

    let mut next = idx.saturating_add(1);
    while next < input.len() && !input.is_char_boundary(next) {
        next = next.saturating_add(1);
    }
    next
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
        // Use the actual match start position, expanding to line start only
        // when the match begins at a line boundary in the normalized text.
        // This prevents destroying preceding text on the same line when
        // the match starts mid-line after whitespace stripping.
        let original_start =
            if norm_start == 0 || normalized_contents.as_bytes()[norm_start - 1] == b'\n' {
                // Match starts at a line boundary — use line start for full-line replacement.
                line_start_before(contents, mapped_start)
            } else {
                // Match starts mid-line — use the exact mapped position.
                mapped_start
            };
        let original_end = byte_map.get(norm_end).copied().unwrap_or(contents.len());
        matches.push((original_start, original_end));
        cursor = next_char_boundary(&normalized_contents, norm_start);
    }
    matches
}

/// Normalize typographic punctuation to its ASCII counterpart:
///
/// * `"` `"` / U+201C U+201D → `"`
/// * `'` `'` / U+2018 U+2019 → `'`
/// * `–` `—` / U+2013 U+2014 → `-`
/// * U+00A0 (non-breaking space) → ASCII space
///
/// Returns the normalized string plus a byte-map sized to
/// `normalized.len()` whose i-th entry is the original byte offset of
/// the character that produced normalized byte i. Used to recover the
/// original-byte range after finding a match in normalized space.
fn punctuation_normalized_with_map(input: &str) -> (String, Vec<usize>) {
    let mut normalized = String::with_capacity(input.len());
    let mut byte_map = Vec::with_capacity(input.len());
    for (idx, ch) in input.char_indices() {
        let replacement: Option<char> = match ch {
            '\u{201C}' | '\u{201D}' => Some('"'),
            '\u{2018}' | '\u{2019}' => Some('\''),
            '\u{2013}' | '\u{2014}' => Some('-'),
            '\u{00A0}' => Some(' '),
            _ => None,
        };
        let written = replacement.unwrap_or(ch);
        normalized.push(written);
        for _ in 0..written.len_utf8() {
            byte_map.push(idx);
        }
    }
    (normalized, byte_map)
}

/// Try to find `search` inside `contents` after normalizing typographic
/// punctuation in both. Catches the copy-paste failure mode where a
/// browser, word processor, or chat client silently converted ASCII
/// quotes/dashes to their Unicode "pretty" forms.
fn punctuation_normalized_matches(contents: &str, search: &str) -> Vec<(usize, usize)> {
    let (norm_contents, byte_map) = punctuation_normalized_with_map(contents);
    let (norm_search, _) = punctuation_normalized_with_map(search);
    if norm_search.is_empty() {
        return Vec::new();
    }
    // If normalization didn't change anything, the exact-match pass
    // already considered this case — skip to avoid double-reporting.
    if norm_contents == contents && norm_search == search {
        return Vec::new();
    }

    let mut matches = Vec::new();
    let mut cursor = 0;
    while let Some(rel_idx) = norm_contents[cursor..].find(&norm_search) {
        let norm_start = cursor + rel_idx;
        let norm_end = norm_start + norm_search.len();
        let Some(&original_start) = byte_map.get(norm_start) else {
            break;
        };
        let original_end = byte_map.get(norm_end).copied().unwrap_or(contents.len());
        matches.push((original_start, original_end));
        cursor = next_char_boundary(&norm_contents, norm_start);
    }
    matches
}

// === ListDirTool ===

/// Tool for listing directory contents.
pub struct ListDirTool;

const LIST_DIR_TIMEOUT: Duration = Duration::from_secs(30);

/// Cap on entries returned by a single `list_dir` call so a huge directory
/// (node_modules, build output, photo dumps) can't balloon the tool result.
/// Mirrors the bounded-output idiom of `read_file`'s `HARD_MAX_READ_LINES`.
/// Directories at or under the cap keep the historical plain-array response;
/// larger ones return an object with truncation metadata.
const LIST_DIR_MAX_ENTRIES: usize = 500;

#[async_trait]
impl ToolSpec for ListDirTool {
    fn name(&self) -> &'static str {
        "list_dir"
    }

    fn model_visible(&self) -> bool {
        false
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

        let entries =
            list_dir_entries_async(dir_path, context.cancel_token.clone(), LIST_DIR_TIMEOUT)
                .await?;

        ToolResult::json(&entries).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

async fn list_dir_entries_async(
    dir_path: PathBuf,
    cancel_token: Option<CancellationToken>,
    timeout: Duration,
) -> Result<Value, ToolError> {
    let worker_cancel_token = cancel_token.clone();
    run_blocking_list_dir(timeout, cancel_token, move || {
        list_dir_entries(&dir_path, worker_cancel_token.as_ref())
    })
    .await
}

async fn run_blocking_list_dir<F>(
    timeout: Duration,
    cancel_token: Option<CancellationToken>,
    list_dir: F,
) -> Result<Value, ToolError>
where
    F: FnOnce() -> Result<Value, ToolError> + Send + 'static,
{
    if cancel_token
        .as_ref()
        .is_some_and(CancellationToken::is_cancelled)
    {
        return Err(list_dir_cancelled());
    }

    let task = tokio::task::spawn_blocking(list_dir);
    let result = match cancel_token {
        Some(token) => {
            tokio::select! {
                biased;
                () = token.cancelled() => return Err(list_dir_cancelled()),
                result = tokio::time::timeout(timeout, task) => result,
            }
        }
        None => tokio::time::timeout(timeout, task).await,
    };

    let joined = result.map_err(|_| list_dir_timeout(timeout))?;
    joined.map_err(|err| {
        ToolError::execution_failed(format!("list_dir worker failed before completion: {err}"))
    })?
}

fn list_dir_entries(
    dir_path: &Path,
    cancel_token: Option<&CancellationToken>,
) -> Result<Value, ToolError> {
    check_list_dir_cancelled(cancel_token)?;

    let mut entries = Vec::new();
    let mut total_entries = 0usize;

    for entry in fs::read_dir(dir_path).map_err(|e| {
        ToolError::execution_failed(format!(
            "Failed to read directory {}: {}",
            dir_path.display(),
            e
        ))
    })? {
        check_list_dir_cancelled(cancel_token)?;

        let entry = entry.map_err(|e| ToolError::execution_failed(e.to_string()))?;
        total_entries += 1;
        // Past the cap, keep counting for the truncation metadata but stop
        // materializing entries.
        if entries.len() >= LIST_DIR_MAX_ENTRIES {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;

        entries.push(json!({
            "name": entry.file_name().to_string_lossy().to_string(),
            "is_dir": file_type.is_dir(),
        }));
    }

    if total_entries > entries.len() {
        Ok(json!({
            "entries": entries,
            "listed_entries": LIST_DIR_MAX_ENTRIES,
            "total_entries": total_entries,
            "truncated": true,
        }))
    } else {
        Ok(Value::Array(entries))
    }
}

fn check_list_dir_cancelled(cancel_token: Option<&CancellationToken>) -> Result<(), ToolError> {
    if cancel_token.is_some_and(CancellationToken::is_cancelled) {
        return Err(list_dir_cancelled());
    }
    Ok(())
}

fn list_dir_cancelled() -> ToolError {
    ToolError::cancelled("list_dir cancelled before completion")
}

fn list_dir_timeout(timeout: Duration) -> ToolError {
    ToolError::Timeout {
        seconds: timeout.as_secs().max(1),
    }
}

// === Unit Tests ===

#[cfg(test)]
#[path = "file/tests.rs"]
mod pdf_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn read_before_edit(ctx: &ToolContext, path: &str) {
        ReadFileTool
            .execute(json!({"path": path}), ctx)
            .await
            .expect("read before edit");
    }

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

    // This test deliberately serializes process-global environment changes
    // while awaiting the tool path.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn read_file_denies_codewhale_config_backups_and_secret_store() {
        let _env_lock = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", tmp.path());
        let _config_path = crate::test_support::EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
        let _legacy_config_path = crate::test_support::EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");

        fs::write(tmp.path().join("config.toml"), "api_key = \"secret\"\n").expect("write config");
        fs::write(
            tmp.path().join("config.toml.bak"),
            "api_key = \"old-secret\"\n",
        )
        .expect("write config backup");
        fs::create_dir_all(tmp.path().join("secrets")).expect("create secrets dir");
        fs::write(
            tmp.path().join("secrets").join("secrets.json"),
            r#"{"provider":"secret"}"#,
        )
        .expect("write file keyring");
        fs::write(tmp.path().join("notes.txt"), "ordinary workspace data")
            .expect("write ordinary file");

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        for path in ["config.toml", "config.toml.bak", "secrets/secrets.json"] {
            let err = ReadFileTool
                .execute(json!({"path": path}), &ctx)
                .await
                .expect_err("credential-bearing CodeWhale file must be denied");
            let message = err.to_string();
            assert!(message.contains("cannot expose CodeWhale"), "{message}");
            assert!(message.contains("codewhale config list"), "{message}");
        }

        let ordinary = ReadFileTool
            .execute(json!({"path": "notes.txt"}), &ctx)
            .await
            .expect("ordinary workspace file should remain readable");
        assert_eq!(ordinary.content, "ordinary workspace data");
    }

    #[tokio::test]
    async fn read_file_ocr_extracts_text_from_image_when_backend_exists() {
        if !crate::tools::image_ocr::ocr_available() {
            return;
        }
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ocr_hello.png");
        if !fixture.exists() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        fs::copy(&fixture, tmp.path().join("ocr_hello.png")).expect("copy fixture");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let result = match ReadFileTool
            .execute(json!({"path": "ocr_hello.png"}), &ctx)
            .await
        {
            Ok(result) => result,
            Err(err) => {
                // Name is when_backend_exists — skip if live OCR fails after
                // the availability probe (restricted Vision, etc.).
                let msg = err.to_string();
                let _skip_reason =
                    format!("OCR backend probe passed but read_file OCR failed: {msg}");
                let _ = &_skip_reason;
                return;
            }
        };

        assert!(result.success);
        assert!(result.content.contains("<image_ocr"));
        let normalized = result.content.to_uppercase();
        assert!(
            normalized.contains("HELLO") && normalized.contains("OCR"),
            "expected OCR text in read_file result, got {:?}",
            result.content
        );
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
    async fn read_file_streamed_range_on_large_file_matches_windowed_contract() {
        // Over 16KB forces the streamed BufRead path even without an
        // explicit range; assert the ranged output stays byte-compatible
        // with the historical full-read implementation.
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let file = tmp.path().join("large.txt");
        let body: String = (1..=2000)
            .map(|n| format!("line {n} {}\n", "x".repeat(20)))
            .collect();
        assert!(body.len() > 16 * 1024, "fixture must exceed 16KB");
        fs::write(&file, &body).expect("write");

        let tool = ReadFileTool;
        let result = tool
            .execute(
                json!({ "path": "large.txt", "start_line": 1500, "max_lines": 10 }),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("total_lines=\"2000\""));
        assert!(result.content.contains("shown_lines=\"1500-1509\""));
        assert!(result.content.contains("next_start_line=\"1510\""));
        assert!(result.content.contains("  1500│ line 1500"));
        assert!(result.content.contains("  1509│ line 1509"));
        assert!(!result.content.contains("  1510│"));
        assert!(result.content.contains(
            "[TRUNCATED] Showing lines 1500-1509 of 2000. To continue, call read_file with path=\"large.txt\" start_line=1510 max_lines=10"
        ));

        // Default window (no range) on the same large file starts at line 1.
        let default_window = tool
            .execute(json!({ "path": "large.txt" }), &ctx)
            .await
            .expect("execute");
        assert!(default_window.content.contains("shown_lines=\"1-200\""));
        assert!(default_window.content.contains("next_start_line=\"201\""));
        assert!(default_window.content.contains("     1│ line 1"));

        // Paging past EOF returns the no-content sentinel, not an error.
        let past_end = tool
            .execute(json!({ "path": "large.txt", "start_line": 5000 }), &ctx)
            .await
            .expect("execute");
        assert!(past_end.content.contains("[NO CONTENT]"));
        assert!(past_end.content.contains("shown_lines=\"none\""));
    }

    #[tokio::test]
    async fn read_file_streamed_range_rejects_invalid_utf8_like_full_read() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let file = tmp.path().join("mixed.bin");
        // Valid first lines, invalid bytes later: the streamed path must
        // still fail the whole read like read_to_string did.
        let mut bytes = b"good line\n".repeat(5);
        bytes.extend_from_slice(&[0xFF, 0xFE, b'\n']);
        fs::write(&file, &bytes).expect("write");

        let err = ReadFileTool
            .execute(
                json!({ "path": "mixed.bin", "start_line": 1, "max_lines": 2 }),
                &ctx,
            )
            .await
            .expect_err("invalid UTF-8 must error");
        let message = err.to_string();
        assert!(message.contains("Failed to read"), "{message}");
        assert!(message.contains("valid UTF-8"), "{message}");
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
    fn clean_pdf_text_collapses_consecutive_blank_lines() {
        let raw = "line1\n\n\n\n\nline2\n\n\nline3";
        let cleaned = super::clean_pdf_text(raw);
        assert_eq!(cleaned, "line1\n\nline2\n\nline3");
    }

    #[test]
    fn clean_pdf_text_replaces_nul_bytes_with_replacement_char() {
        let raw = "hello\0world";
        let cleaned = super::clean_pdf_text(raw);
        assert!(!cleaned.contains('\0'));
        assert!(cleaned.contains('\u{FFFD}'));
    }

    #[test]
    fn clean_pdf_text_replaces_non_breaking_spaces() {
        let raw = "hello\u{A0}world";
        let cleaned = super::clean_pdf_text(raw);
        assert!(!cleaned.contains('\u{A0}'));
        assert_eq!(cleaned, "hello world");
    }

    #[test]
    fn clean_pdf_text_trims_trailing_whitespace() {
        let raw = "hello   ";
        let cleaned = super::clean_pdf_text(raw);
        assert_eq!(cleaned, "hello");
    }

    #[test]
    fn clean_pdf_text_preserves_leading_indentation() {
        let raw = "   indented line\nregular line";
        let cleaned = super::clean_pdf_text(raw);
        assert_eq!(cleaned, "   indented line\nregular line");
    }

    #[tokio::test]
    async fn read_file_pdf_path_uses_optional_pdftotext_adapter() {
        if !sample_pdf_present() || crate::dependencies::resolve_pdftotext().is_none() {
            return;
        }
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
        let mutation = &result.metadata.as_ref().expect("metadata")["mutation"];
        assert_eq!(
            mutation["files"],
            json!([{ "path": "output.txt", "outcome": "created" }])
        );
        assert!(
            mutation["diff"]
                .as_str()
                .is_some_and(|diff| diff.contains("--- a/output.txt")),
            "{mutation}"
        );
        assert!(
            !mutation["diff"]
                .as_str()
                .unwrap_or_default()
                .contains(&tmp.path().display().to_string()),
            "receipt headers must not expose the resolved host path: {mutation}"
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

    #[cfg(unix)]
    #[tokio::test]
    async fn write_file_tool_new_file_matches_standard_creation_mode() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let control = tmp.path().join("control.txt");
        fs::write(&control, b"control").expect("write control");

        WriteFileTool
            .execute(
                json!({"path": "created.txt", "content": "from write_file"}),
                &ctx,
            )
            .await
            .expect("execute");

        let control_mode = fs::metadata(&control)
            .expect("control metadata")
            .permissions()
            .mode()
            & 0o777;
        let created_mode = fs::metadata(tmp.path().join("created.txt"))
            .expect("created metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(created_mode, control_mode);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_file_tool_preserves_existing_mode() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let path = tmp.path().join("shared.txt");
        fs::write(&path, b"before").expect("initial write");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o664))
            .expect("set shared permissions");

        WriteFileTool
            .execute(json!({"path": "shared.txt", "content": "after"}), &ctx)
            .await
            .expect("execute");

        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o664);
        assert_eq!(fs::read_to_string(&path).expect("read"), "after");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn edit_file_tool_preserves_executable_bits() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let path = tmp.path().join("script.sh");
        fs::write(&path, b"#!/bin/sh\nexit 0\n").expect("initial write");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("set executable permissions");
        read_before_edit(&ctx, "script.sh").await;

        EditFileTool
            .execute(
                json!({
                    "path": "script.sh",
                    "search": "exit 0",
                    "replace": "exit 1"
                }),
                &ctx,
            )
            .await
            .expect("execute");

        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "#!/bin/sh\nexit 1\n"
        );
    }

    #[tokio::test]
    async fn edit_file_refuses_brace_collapsed_match_arm_payload() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let path = tmp.path().join("arm.rs");
        let original = r#"match outcome {
            SendMessageOutcome::Finished {
                status: TurnOutcomeStatus::Interrupted,
                ..
            } => self.pause_goal_after_interruption().await,
            SendMessageOutcome::Finished {
                status: TurnOutcomeStatus::Completed,
                ..
            } => {}
        }
"#;
        fs::write(&path, original).expect("write");
        read_before_edit(&ctx, "arm.rs").await;

        let search = r#"SendMessageOutcome::Finished {
                status: TurnOutcomeStatus::Interrupted,
                ..
            } => self.pause_goal_after_interruption().await,"#;
        // Corrupted host payload: brace block collapsed to empty brackets.
        let replace = "[
                
            ] => {},";
        let err = EditFileTool
            .execute(
                json!({
                    "path": "arm.rs",
                    "search": search,
                    "replace": replace,
                }),
                &ctx,
            )
            .await
            .expect_err("corrupted brace collapse must fail closed");
        let msg = err.to_string();
        assert!(
            msg.contains("corrupted") || msg.contains("collapsed") || msg.contains("unbalanced"),
            "unexpected error: {msg}"
        );
        assert_eq!(fs::read_to_string(&path).expect("read"), original);
    }

    #[tokio::test]
    async fn edit_file_preserves_rust_match_arm_braces() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let path = tmp.path().join("arm.rs");
        let original = r#"match outcome {
            SendMessageOutcome::Finished {
                status: TurnOutcomeStatus::Interrupted,
                ..
            } => self.pause_goal_after_interruption().await,
            other => {}
        }
"#;
        fs::write(&path, original).expect("write");
        read_before_edit(&ctx, "arm.rs").await;

        let search = r#"SendMessageOutcome::Finished {
                status: TurnOutcomeStatus::Interrupted,
                ..
            } => self.pause_goal_after_interruption().await,"#;
        let replace = r#"SendMessageOutcome::Finished {
                status: TurnOutcomeStatus::Interrupted,
                ..
            } => {
                // stay active
                let _ = self.tx_event.send(Event::status("ok".into())).await;
            }"#;
        EditFileTool
            .execute(
                json!({
                    "path": "arm.rs",
                    "search": search,
                    "replace": replace,
                }),
                &ctx,
            )
            .await
            .expect("brace-heavy replace must apply");
        let updated = fs::read_to_string(&path).expect("read");
        assert!(updated.contains("stay active"), "{updated}");
        assert!(
            updated.contains("SendMessageOutcome::Finished"),
            "{updated}"
        );
        assert!(
            !updated.contains("pause_goal_after_interruption"),
            "{updated}"
        );
    }

    #[tokio::test]
    async fn test_edit_file_tool() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // Create a file to edit
        let test_file = tmp.path().join("edit_me.txt");
        fs::write(&test_file, "hello world").expect("write");
        read_before_edit(&ctx, "edit_me.txt").await;

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({"path": "edit_me.txt", "search": "hello", "replace": "hi"}),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("Replaced 1 occurrence"));
        // Inline diff (#505) — the unified diff lands above the summary
        // line so the TUI's diff-aware renderer kicks in.
        assert!(result.content.contains("--- a/"), "{}", result.content);
        assert!(
            result.content.contains("-hello world"),
            "{}",
            result.content
        );
        assert!(result.content.contains("+hi world"), "{}", result.content);
        let mutation = &result.metadata.as_ref().expect("metadata")["mutation"];
        assert_eq!(
            mutation["files"],
            json!([{ "path": "edit_me.txt", "outcome": "updated" }])
        );
        let receipt_diff = mutation["diff"].as_str().expect("receipt diff");
        assert!(receipt_diff.contains("--- a/edit_me.txt"), "{receipt_diff}");
        assert!(receipt_diff.contains("-hello world"), "{receipt_diff}");
        assert!(receipt_diff.contains("+hi world"), "{receipt_diff}");
        assert!(
            !receipt_diff.contains(&tmp.path().display().to_string()),
            "receipt headers must not expose the resolved host path: {receipt_diff}"
        );

        // Verify edit was applied
        let edited = fs::read_to_string(&test_file).expect("read");
        assert_eq!(edited, "hi world");
    }

    #[tokio::test]
    async fn edit_file_matches_lf_search_in_crlf_file_and_preserves_crlf() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let test_file = tmp.path().join("crlf.py");
        fs::write(
            &test_file,
            b"def greet(name):\r\n    print(name)\r\n\r\ndef add(a, b):\r\n    return a + b\r\n",
        )
        .expect("write");
        read_before_edit(&ctx, "crlf.py").await;

        let result = EditFileTool
            .execute(
                json!({
                    "path": "crlf.py",
                    "search": "def add(a, b):\n    return a + b",
                    "replace": "def add(a, b):\n    return a * b",
                }),
                &ctx,
            )
            .await
            .expect("LF model input should edit a CRLF file");

        assert!(result.success, "{}", result.content);
        assert_eq!(
            fs::read(&test_file).expect("read"),
            b"def greet(name):\r\n    print(name)\r\n\r\ndef add(a, b):\r\n    return a * b\r\n",
        );
    }

    #[test]
    fn edit_file_sparse_crlf_positions_map_utf8_range_through_eof() {
        let original = "前\r\n尾";
        let (normalized, crlf_positions) = normalize_crlf_with_positions(original);

        assert_eq!(normalized, "前\n尾");
        assert_eq!(crlf_positions.as_deref(), Some(&[3][..]));
        assert_eq!(
            map_normalized_range((0, normalized.len()), crlf_positions.as_deref()),
            (0, original.len()),
        );
    }

    #[tokio::test]
    async fn edit_file_maps_utf8_crlf_match_ending_at_eof() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let test_file = tmp.path().join("utf8-eof-crlf.txt");
        fs::write(&test_file, "前\r\n尾").expect("write");
        read_before_edit(&ctx, "utf8-eof-crlf.txt").await;

        EditFileTool
            .execute(
                json!({
                    "path": "utf8-eof-crlf.txt",
                    "search": "前\n尾",
                    "replace": "始\n终",
                }),
                &ctx,
            )
            .await
            .expect("UTF-8 CRLF match should map through EOF");

        assert_eq!(fs::read(&test_file).expect("read"), "始\r\n终".as_bytes(),);
    }

    #[tokio::test]
    async fn edit_file_normalizes_multiline_replacement_for_single_line_crlf_match() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let test_file = tmp.path().join("single-line-crlf.txt");
        fs::write(&test_file, b"alpha\r\nomega\r\n").expect("write");
        read_before_edit(&ctx, "single-line-crlf.txt").await;

        EditFileTool
            .execute(
                json!({
                    "path": "single-line-crlf.txt",
                    "search": "omega",
                    "replace": "beta\ngamma",
                }),
                &ctx,
            )
            .await
            .expect("replacement should follow the file's CRLF style");

        assert_eq!(
            fs::read(&test_file).expect("read"),
            b"alpha\r\nbeta\r\ngamma\r\n",
        );
    }

    #[tokio::test]
    async fn edit_file_normalizes_crlf_and_mixed_replacement_for_lf_file() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let test_file = tmp.path().join("lf.txt");
        fs::write(&test_file, b"alpha\nomega\n").expect("write");
        read_before_edit(&ctx, "lf.txt").await;

        EditFileTool
            .execute(
                json!({
                    "path": "lf.txt",
                    "search": "omega",
                    "replace": "beta\r\ngamma\nfinal",
                }),
                &ctx,
            )
            .await
            .expect("replacement should follow the file's LF style");

        assert_eq!(
            fs::read(&test_file).expect("read"),
            b"alpha\nbeta\ngamma\nfinal\n",
        );
    }

    #[tokio::test]
    async fn edit_file_rejects_logical_duplicate_across_lf_and_crlf() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let test_file = tmp.path().join("mixed.txt");
        let original = b"same\nblock\r\nsame\r\nblock\r\n";
        fs::write(&test_file, original).expect("write");
        read_before_edit(&ctx, "mixed.txt").await;

        let error = EditFileTool
            .execute(
                json!({
                    "path": "mixed.txt",
                    "search": "same\nblock",
                    "replace": "changed",
                }),
                &ctx,
            )
            .await
            .expect_err("logical duplicates must remain non-unique");

        assert!(error.to_string().contains("matched 2"), "{error}");
        assert_eq!(fs::read(&test_file).expect("read"), original);
    }

    #[tokio::test]
    async fn edit_file_combines_crlf_and_indentation_fuzzy_matching() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let test_file = tmp.path().join("fuzzy-crlf.txt");
        fs::write(&test_file, "前言\r\n    数据 = 1\r\n").expect("write");
        read_before_edit(&ctx, "fuzzy-crlf.txt").await;

        let result = EditFileTool
            .execute(
                json!({
                    "path": "fuzzy-crlf.txt",
                    "search": "前言\n        数据 = 1",
                    "replace": "前言\n    数据 = 2",
                }),
                &ctx,
            )
            .await
            .expect("indentation fallback should compose with CRLF normalization");

        assert!(
            result.content.contains("fuzzy indentation match"),
            "{}",
            result.content
        );
        assert_eq!(
            fs::read(&test_file).expect("read"),
            "前言\r\n    数据 = 2\r\n".as_bytes(),
        );
    }

    #[tokio::test]
    async fn edit_file_combines_crlf_and_punctuation_fuzzy_matching() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let test_file = tmp.path().join("punctuation-crlf.txt");
        fs::write(&test_file, "前言\r\n数据 \"x\"\r\n").expect("write");
        read_before_edit(&ctx, "punctuation-crlf.txt").await;

        let result = EditFileTool
            .execute(
                json!({
                    "path": "punctuation-crlf.txt",
                    "search": "前言\n数据 \u{201C}x\u{201D}",
                    "replace": "前言\r\n数据 y\n下一行",
                }),
                &ctx,
            )
            .await
            .expect("punctuation fallback should compose with CRLF normalization");

        assert!(
            result.content.contains("fuzzy punctuation match"),
            "{}",
            result.content
        );
        assert_eq!(
            fs::read(&test_file).expect("read"),
            "前言\r\n数据 y\r\n下一行\r\n".as_bytes(),
        );
    }

    #[tokio::test]
    async fn edit_file_rejects_line_ending_normalized_noop() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let test_file = tmp.path().join("noop-crlf.txt");
        let original = b"alpha\r\nbeta\r\n";
        fs::write(&test_file, original).expect("write");
        read_before_edit(&ctx, "noop-crlf.txt").await;

        let error = EditFileTool
            .execute(
                json!({
                    "path": "noop-crlf.txt",
                    "search": "alpha\nbeta",
                    "replace": "alpha\r\nbeta",
                }),
                &ctx,
            )
            .await
            .expect_err("normalized no-op should be rejected");

        assert!(error.to_string().contains("no change intended"), "{error}");
        assert_eq!(fs::read(&test_file).expect("read"), original);
    }

    #[tokio::test]
    async fn edit_file_requires_prior_read() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("blind.txt");
        fs::write(&test_file, "hello world").expect("write");

        let err = EditFileTool
            .execute(
                json!({"path": "blind.txt", "search": "hello", "replace": "hi"}),
                &ctx,
            )
            .await
            .expect_err("edit without read should fail");
        let message = err.to_string();
        assert!(message.contains("not been read"), "{message}");
        assert!(message.contains("read_file"), "{message}");

        let unchanged = fs::read_to_string(&test_file).expect("read");
        assert_eq!(unchanged, "hello world");
    }

    #[tokio::test]
    async fn edit_file_rejects_stale_prior_read() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("stale.txt");
        fs::write(&test_file, "alpha beta").expect("write");
        read_before_edit(&ctx, "stale.txt").await;
        fs::write(&test_file, "alpha beta gamma").expect("external write");

        let err = EditFileTool
            .execute(
                json!({"path": "stale.txt", "search": "alpha", "replace": "omega"}),
                &ctx,
            )
            .await
            .expect_err("stale read should fail");
        let message = err.to_string();
        assert!(message.contains("changed since"), "{message}");
        assert!(message.contains("read_file"), "{message}");

        let unchanged = fs::read_to_string(&test_file).expect("read");
        assert_eq!(unchanged, "alpha beta gamma");
    }

    #[tokio::test]
    async fn edit_file_rejects_non_unique_exact_match() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("multi.txt");
        fs::write(&test_file, "hello world hello").expect("write");
        read_before_edit(&ctx, "multi.txt").await;

        let err = EditFileTool
            .execute(
                json!({"path": "multi.txt", "search": "hello", "replace": "hi"}),
                &ctx,
            )
            .await
            .expect_err("non-unique exact match should fail");
        let message = err.to_string();
        assert!(message.contains("non-unique"), "{message}");
        assert!(message.contains("matched 2"), "{message}");
        assert!(message.contains("read_file"), "{message}");

        let unchanged = fs::read_to_string(&test_file).expect("read");
        assert_eq!(unchanged, "hello world hello");
    }

    #[tokio::test]
    async fn test_edit_file_accepts_omitted_and_explicit_fuzz() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = EditFileTool;

        for (file_name, fuzz) in [
            ("fuzz_omitted.txt", None),
            ("fuzz_false.txt", Some(false)),
            ("fuzz_true.txt", Some(true)),
        ] {
            let test_file = tmp.path().join(file_name);
            fs::write(&test_file, "hello world").expect("write");
            read_before_edit(&ctx, file_name).await;

            let mut input = serde_json::Map::from_iter([
                ("path".to_string(), json!(file_name)),
                ("search".to_string(), json!("hello")),
                ("replace".to_string(), json!("hi")),
            ]);
            if let Some(fuzz) = fuzz {
                input.insert("fuzz".to_string(), json!(fuzz));
            }

            let result = tool
                .execute(Value::Object(input), &ctx)
                .await
                .expect("execute");

            assert!(result.success, "{file_name}: {}", result.content);
            assert!(result.content.contains("Replaced 1 occurrence"));
            let edited = fs::read_to_string(&test_file).expect("read");
            assert_eq!(edited, "hi world");
        }
    }

    #[tokio::test]
    async fn test_edit_file_single_match_has_no_multi_match_warning() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("single.txt");
        fs::write(&test_file, "hello world").expect("write");
        read_before_edit(&ctx, "single.txt").await;

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
        read_before_edit(&ctx, "fuzzy.txt").await;

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
    async fn test_edit_file_fuzz_tolerates_leading_whitespace_after_multibyte_start() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("fuzzy_cjk.txt");
        fs::write(&test_file, "数据\n").expect("write");
        read_before_edit(&ctx, "fuzzy_cjk.txt").await;

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({
                    "path": "fuzzy_cjk.txt",
                    "search": "    数据",
                    "replace": "记录",
                    "fuzz": true
                }),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success, "{}", result.content);
        assert!(result.content.contains("fuzzy indentation match"));
        let edited = fs::read_to_string(&test_file).expect("read");
        assert_eq!(edited, "记录\n");
    }

    #[tokio::test]
    async fn test_edit_file_fuzz_tolerates_smart_quote_substitution() {
        // The file on disk has ASCII quotes. The search comes from a
        // browser paste with curly quotes. Exact match fails; the
        // punctuation-normalized fallback should still land the edit.
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("smart.rs");
        fs::write(&test_file, "let s = \"hello world\";\n").expect("write");
        read_before_edit(&ctx, "smart.rs").await;

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({
                    "path": "smart.rs",
                    // \u{201C} \u{201D} are the curly double-quote pair.
                    "search": "let s = \u{201C}hello world\u{201D};",
                    "replace": "let s = \"hello universe\";",
                    "fuzz": true
                }),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success, "fuzzy punctuation edit should succeed");
        assert!(
            result.content.contains("fuzzy punctuation match"),
            "expected punctuation-fuzz note, got: {}",
            result.content
        );
        let edited = fs::read_to_string(&test_file).expect("read");
        assert_eq!(edited, "let s = \"hello universe\";\n");
    }

    #[tokio::test]
    async fn test_edit_file_fuzz_tolerates_smart_quote_after_multibyte_start() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("smart_cjk.md");
        fs::write(&test_file, "数据 \"x\"\n").expect("write");
        read_before_edit(&ctx, "smart_cjk.md").await;

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({
                    "path": "smart_cjk.md",
                    "search": "数据 \u{201C}x\u{201D}",
                    "replace": "数据 y",
                    "fuzz": true
                }),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success, "{}", result.content);
        assert!(result.content.contains("fuzzy punctuation match"));
        let edited = fs::read_to_string(&test_file).expect("read");
        assert_eq!(edited, "数据 y\n");
    }

    #[tokio::test]
    async fn test_edit_file_fuzz_tolerates_em_dash_and_nbsp() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("dash.md");
        // File has an ASCII hyphen and ASCII space.
        fs::write(&test_file, "alpha - beta\n").expect("write");
        read_before_edit(&ctx, "dash.md").await;

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({
                    "path": "dash.md",
                    // Search uses em-dash + NBSP, common after a copy-paste
                    // from a styled document.
                    "search": "alpha\u{00A0}\u{2014}\u{00A0}beta",
                    "replace": "alpha - gamma",
                    "fuzz": true
                }),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);
        let edited = fs::read_to_string(&test_file).expect("read");
        assert_eq!(edited, "alpha - gamma\n");
    }

    #[tokio::test]
    async fn test_edit_file_not_found() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // Create a file without the search string
        let test_file = tmp.path().join("no_match.txt");
        fs::write(&test_file, "foo bar baz").expect("write");
        read_before_edit(&ctx, "no_match.txt").await;

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
        assert!(err.to_string().contains("read_file"));
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
        // #5003 - the diagnostic must help the model self-correct: it should
        // size the payload and point at the root cause instead of a bare
        // "no change intended".
        assert!(
            err.contains("10 chars"),
            "error should size the payload: {err}"
        );
        assert!(
            err.contains("Recovery"),
            "error should offer recovery: {err}"
        );
        let unchanged = fs::read_to_string(&test_file).expect("read");
        assert_eq!(unchanged, "a := \"foo\"");
    }

    #[test]
    fn test_c_preprocessor_rejects_missing_close() {
        let before = "#if FEATURE\nold code\n#endif\n";
        let after = "#if FEATURE\nnew code\n";
        assert_eq!(
            invalid_preprocessor_edit(Path::new("source.c"), before, after),
            Some(PREPROCESSOR_CONDITIONAL_ERROR)
        );
    }

    #[test]
    fn test_c_preprocessor_rejects_extra_close() {
        let before = "#if FEATURE\nold code\n#endif\n";
        let after = "#if FEATURE\nnew code\n#endif\n#endif\n";
        assert_eq!(
            invalid_preprocessor_edit(Path::new("source.hpp"), before, after),
            Some(PREPROCESSOR_CONDITIONAL_ERROR)
        );
    }

    #[test]
    fn test_c_preprocessor_allows_balanced_block_removal_and_insertion() {
        let block = "#ifdef FEATURE\nfeature();\n#endif\n";
        assert!(invalid_preprocessor_edit(Path::new("source.cc"), block, "").is_none());
        assert!(invalid_preprocessor_edit(Path::new("source.cc"), "", block).is_none());
    }

    #[test]
    fn test_c_preprocessor_allows_in_block_edit() {
        let before = "#if FEATURE\nold_call();\n#endif\n";
        let after = "#if FEATURE\nnew_call();\n#endif\n";
        assert!(invalid_preprocessor_edit(Path::new("source.cxx"), before, after).is_none());
    }

    #[test]
    fn test_non_c_directive_prose_is_not_validated() {
        let before = "#if this example is enabled\nexplanation\n#endif\n";
        let after = "#if this example is enabled\nupdated explanation\n";
        assert!(invalid_preprocessor_edit(Path::new("guide.md"), before, after).is_none());
    }

    #[test]
    fn test_preview_search_for_error_truncates() {
        let long_line = "x".repeat(200);
        let search = format!("{long_line}\nsecond line\nthird line\nfourth line\n");
        let preview = preview_search_for_error(&search);
        assert!(preview.lines().count() <= 3);
        assert!(preview.contains("..."));
        assert!(!preview.contains("fourth line"));
    }

    #[tokio::test]
    async fn test_edit_file_not_found_shows_search_preview() {
        // #5003 - when search misses, the error should preview the search text
        // so the model can compare what it searched for against the file.
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let test_file = tmp.path().join("preview.txt");
        fs::write(&test_file, "foo bar baz").expect("write");
        read_before_edit(&ctx, "preview.txt").await;

        let tool = EditFileTool;
        let result = tool
            .execute(
                json!({
                    "path": "preview.txt",
                    "search": "first line\nsecond line\n",
                    "replace": "changed"
                }),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Search string not found"));
        assert!(
            err.contains("first line"),
            "error should preview search text: {err}"
        );
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
        let entries: Value = serde_json::from_str(&result.content).expect("list_dir json");
        assert!(entries.as_array().expect("entries").iter().any(|entry| {
            entry.get("name").and_then(Value::as_str) == Some("subdir")
                && entry.get("is_dir").and_then(Value::as_bool) == Some(true)
        }));
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

    #[tokio::test]
    async fn test_list_dir_small_dir_keeps_plain_array_response() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        fs::write(tmp.path().join("only.txt"), "").expect("write");

        let tool = ListDirTool;
        let result = tool.execute(json!({}), &ctx).await.expect("execute");

        let parsed: Value = serde_json::from_str(&result.content).expect("json");
        assert!(
            parsed.is_array(),
            "small dirs must keep the historical array shape: {parsed}"
        );
        assert_eq!(parsed.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_list_dir_caps_entries_with_truncation_metadata() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let extra = 7;
        for i in 0..LIST_DIR_MAX_ENTRIES + extra {
            fs::write(tmp.path().join(format!("f{i:04}.txt")), "").expect("write");
        }

        let tool = ListDirTool;
        let result = tool.execute(json!({}), &ctx).await.expect("execute");

        let parsed: Value = serde_json::from_str(&result.content).expect("json");
        assert!(parsed.is_object(), "oversized dirs return an object");
        assert_eq!(parsed["truncated"], json!(true));
        assert_eq!(
            parsed["listed_entries"].as_u64().unwrap() as usize,
            LIST_DIR_MAX_ENTRIES
        );
        assert_eq!(
            parsed["total_entries"].as_u64().unwrap() as usize,
            LIST_DIR_MAX_ENTRIES + extra
        );
        assert_eq!(
            parsed["entries"].as_array().unwrap().len(),
            LIST_DIR_MAX_ENTRIES
        );
    }

    #[tokio::test]
    async fn test_list_dir_respects_cancel_token() {
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("file.txt"), "").expect("write");
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();
        let ctx = ToolContext::new(tmp.path().to_path_buf()).with_cancel_token(cancel_token);

        let tool = ListDirTool;
        let err = tool
            .execute(json!({}), &ctx)
            .await
            .expect_err("cancelled list_dir should return an error");

        assert!(
            format!("{err:?}").contains("cancelled"),
            "unexpected error: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_list_dir_blocking_wrapper_reports_timeout() {
        let err = run_blocking_list_dir(Duration::from_millis(1), None, || {
            std::thread::sleep(Duration::from_millis(50));
            Ok(Value::Array(Vec::new()))
        })
        .await
        .expect_err("slow list_dir worker should time out");

        assert!(
            matches!(err, ToolError::Timeout { seconds: 1 }),
            "unexpected error: {err:?}"
        );
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
        let required_fields: Vec<_> = required.iter().filter_map(|value| value.as_str()).collect();
        assert_eq!(required_fields, vec!["path", "search", "replace"]);
        assert!(!required_fields.contains(&"fuzz"));
        assert_eq!(
            edit_schema["properties"]["fuzz"]["type"].as_str(),
            Some("boolean")
        );
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
