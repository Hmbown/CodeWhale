//! File system tools: `read_file`, `write_file`, `edit_file`, `list_dir`
//!
//! These tools provide safe file system operations within the workspace,
//! with path validation to prevent escaping the workspace boundary.

use super::diff_format::make_unified_diff;
use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    lsp_diagnostics_for_paths, optional_str, required_str,
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
        "Read a UTF-8 file from the workspace. Use this instead of `cat`, `head`, `tail`, or `sed -n '..p'` in `exec_shell` — it's faster, sandbox-aware, and skips the approval prompt. Plain text is returned as-is; PDFs are auto-extracted via `pdftotext` (poppler) when available. Cannot read images or non-PDF binaries."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file (relative to workspace or absolute)"
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
        let pages = optional_str(&input, "pages");

        if is_pdf(&file_path)? {
            return read_pdf(&file_path, pages);
        }

        let contents = fs::read_to_string(&file_path).map_err(|e| {
            ToolError::execution_failed(format!("Failed to read {}: {}", file_path.display(), e))
        })?;

        Ok(ToolResult::success(contents))
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
    // Try pdftotext (from the poppler suite). Other extractors (mutool,
    // pdfminer) could be added later behind the same dispatch.
    let mut cmd = Command::new("pdftotext");
    cmd.arg("-layout");

    if let Some(spec) = pages {
        match parse_pages_arg(spec) {
            Some((start, end)) => {
                cmd.arg("-f").arg(start.to_string());
                cmd.arg("-l").arg(end.to_string());
            }
            None => {
                return Err(ToolError::invalid_input(format!(
                    "invalid `pages` value `{spec}` (expected `N` or `N-M`, e.g. `1-5`)"
                )));
            }
        }
    }

    cmd.arg(path).arg("-"); // output to stdout
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Structured "binary unavailable" — caller knows what to suggest.
            return ToolResult::json(&json!({
                "type": "binary_unavailable",
                "path": path.display().to_string(),
                "kind": "pdf",
                "reason": "pdftotext not installed",
                "hint": "install poppler (macOS: `brew install poppler`; Debian/Ubuntu: `apt install poppler-utils`)"
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
        "Replace text in a single file via exact search/replace. Use this instead of `sed -i` in `exec_shell` for in-place edits. For multi-hunk or cross-file changes, use `apply_patch` instead. Required: 'path', 'search' (exact text to find), 'replace' (text to substitute)."
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
                    "description": "Text to search for"
                },
                "replace": {
                    "type": "string",
                    "description": "Text to replace with"
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

        let file_path = context.resolve_path(path_str)?;

        let contents = fs::read_to_string(&file_path).map_err(|e| {
            ToolError::execution_failed(format!("Failed to read {}: {}", file_path.display(), e))
        })?;

        let count = contents.matches(search).count();
        if count == 0 {
            return Err(ToolError::execution_failed(format!(
                "Search string not found in {}",
                file_path.display()
            )));
        }

        let updated = contents.replace(search, replace);

        fs::write(&file_path, &updated).map_err(|e| {
            ToolError::execution_failed(format!("Failed to write {}: {}", file_path.display(), e))
        })?;

        let display = file_path.display().to_string();
        let diff = make_unified_diff(&display, &contents, &updated);
        let summary = format!("Replaced {count} occurrence(s) in {display}");
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

    #[tokio::test]
    async fn test_read_file_not_found() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let tool = ReadFileTool;
        let result = tool.execute(json!({"path": "nonexistent.txt"}), &ctx).await;

        assert!(result.is_err());
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

    #[tokio::test]
    async fn read_file_returns_binary_unavailable_when_pdftotext_missing() {
        // We can't reliably remove pdftotext from $PATH in a test, but if
        // it's missing on the runner this test exercises that branch. If
        // it's installed, the test exits early — covered by the parse_pages
        // and is_pdf tests above.
        if Command::new("pdftotext")
            .arg("-v")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
        {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("doc.pdf");
        fs::write(&path, b"%PDF-1.7\n%%EOF").unwrap();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let result = ReadFileTool
            .execute(json!({"path": "doc.pdf"}), &ctx)
            .await
            .expect("structured response, not error");
        assert!(result.success);
        assert!(result.content.contains("binary_unavailable"));
        assert!(result.content.contains("pdftotext"));
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

        let list_schema = ListDirTool.input_schema();
        let required = list_schema
            .get("required")
            .and_then(|value| value.as_array())
            .expect("list schema should include required array");
        assert!(required.is_empty()); // path is optional
    }
}
