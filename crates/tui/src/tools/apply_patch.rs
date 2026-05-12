//! Patch tools: `apply_patch` for unified diff patching
//!
//! This tool provides precise file modifications using unified diff format,
//! supporting multi-hunk patches and fuzzy matching.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    lsp_diagnostics_for_paths, optional_bool, optional_str, optional_u64, required_str,
};

/// Maximum lines of context for fuzzy matching (increased for better tolerance)
const MAX_FUZZ: usize = 50;
/// Limit how much context we print in error messages.
const HUNK_PREVIEW_LINES: usize = 4;
const SNIPPET_RADIUS: usize = 2;
const FILE_LIST_LIMIT: usize = 6;

// === Types ===

/// Result of applying a patch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchResult {
    pub success: bool,
    pub files_applied: usize,
    pub files_total: usize,
    pub hunks_applied: usize,
    pub hunks_total: usize,
    pub fuzz_used: usize,
    #[serde(default)]
    pub hunks_with_fuzz: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touched_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_summaries: Vec<FileSummary>,
    pub message: String,
}

/// Per-file summary for patch application output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSummary {
    pub path: String,
    pub hunks: usize,
    pub hunks_applied: usize,
    pub fuzz_used: usize,
    pub hunks_with_fuzz: usize,
    pub created: bool,
    pub deleted: bool,
}

/// A single hunk in a unified diff
#[derive(Debug, Clone)]
pub struct Hunk {
    pub old_start: usize,
    #[allow(dead_code)]
    pub old_count: usize,
    #[allow(dead_code)]
    pub new_start: usize,
    #[allow(dead_code)]
    pub new_count: usize,
    pub lines: Vec<HunkLine>,
}

/// A line in a hunk
#[derive(Debug, Clone)]
pub enum HunkLine {
    Context(String),
    Add(String),
    Remove(String),
}

/// Tool for applying unified diff patches to files
pub struct ApplyPatchTool;

#[derive(Debug, Clone)]
struct FilePatch {
    path: String,
    hunks: Vec<Hunk>,
    delete_after: bool,
    create_if_missing: bool,
}

#[derive(Debug, Clone)]
struct PendingWrite {
    path: PathBuf,
    content: Option<String>,
    original: Option<String>,
}

#[derive(Debug, Default, Clone, Copy)]
struct PatchStats {
    files_applied: usize,
    files_total: usize,
    hunks_applied: usize,
    hunks_total: usize,
    fuzz_used: usize,
    hunks_with_fuzz: usize,
}

#[derive(Debug, Default, Clone)]
struct PatchStatsExt {
    stats: PatchStats,
    touched_files: Vec<String>,
    file_summaries: Vec<FileSummary>,
    header_path_mismatch: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct PatchShape {
    has_hunks: bool,
    header_files: Vec<String>,
}

impl PatchShape {
    fn file_count(&self) -> usize {
        self.header_files.len()
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct HunkApplyStats {
    hunks_applied: usize,
    fuzz_used: usize,
    hunks_with_fuzz: usize,
}

// === Errors ===

#[derive(Debug, Error)]
enum ApplyHunkError {
    #[error(
        "Failed to find matching location for hunk (expected at line {expected_line}, adjusted to {adjusted_line} with offset {offset:+})"
    )]
    NoMatch {
        expected_line: usize,
        adjusted_line: usize,
        offset: isize,
    },
}

#[async_trait]
impl ToolSpec for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "apply_patch"
    }

    fn description(&self) -> &'static str {
        "Apply a unified-diff patch (multi-hunk, multi-file). Use this instead of `git apply`, `patch`, or repeated `edit_file` calls in `exec_shell` — single transactional change with fuzzy matching and a rendered diff."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to patch (relative to workspace)"
                },
                "patch": {
                    "type": "string",
                    "description": "Unified diff patch content"
                },
                "changes": {
                    "type": "array",
                    "description": "Optional full file replacements (path + content).",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "content": { "type": "string" }
                        },
                        "required": ["path", "content"]
                    }
                },
                "fuzz": {
                    "type": "integer",
                    "description": "Maximum fuzz factor for fuzzy matching (default: 3)"
                },
                "create_if_missing": {
                    "type": "boolean",
                    "description": "Create the file if it doesn't exist (for new file patches)"
                }
            },
            "oneOf": [
                { "required": ["patch"] },
                { "required": ["changes"] }
            ]
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
        let fuzz = optional_u64(&input, "fuzz", MAX_FUZZ as u64).min(MAX_FUZZ as u64);
        let fuzz = usize::try_from(fuzz).unwrap_or(MAX_FUZZ);
        let create_if_missing = optional_bool(&input, "create_if_missing", false);

        if let Some(changes_value) = input.get("changes") {
            let (pending, stats) = build_pending_writes_from_changes(changes_value, context)?;
            apply_pending_writes(&pending)?;
            // Resolve absolute paths for LSP diagnostics query.
            let abs_paths: Vec<PathBuf> = pending.iter().map(|p| p.path.clone()).collect();
            let diag_block = lsp_diagnostics_for_paths(context, &abs_paths).await;
            let result = PatchResult {
                success: true,
                files_applied: stats.stats.files_applied,
                files_total: stats.stats.files_total,
                hunks_applied: stats.stats.hunks_applied,
                hunks_total: stats.stats.hunks_total,
                fuzz_used: stats.stats.fuzz_used,
                hunks_with_fuzz: stats.stats.hunks_with_fuzz,
                touched_files: stats.touched_files.clone(),
                file_summaries: stats.file_summaries.clone(),
                message: build_summary_message(&stats),
            };
            let mut tool_result = ToolResult::json(&result)
                .map_err(|e| ToolError::execution_failed(e.to_string()))?;
            if !diag_block.is_empty() {
                tool_result.content.push('\n');
                tool_result.content.push_str(&diag_block);
            }
            return Ok(tool_result);
        }

        let patch_text = required_str(&input, "patch")?;
        let path_override = optional_str(&input, "path");
        let patch_shape = inspect_patch_shape(patch_text);
        validate_patch_shape(&patch_shape, path_override)?;
        let mismatch_note = path_override.and_then(|path| diff_header_mismatch(path, &patch_shape));
        let file_patches = if let Some(path) = path_override {
            let hunks = parse_unified_diff(patch_text)?;
            if hunks.is_empty() {
                return Err(ToolError::invalid_input(
                    "Patch did not contain any hunks (`@@ ... @@`). Provide a unified diff hunk.",
                ));
            }
            vec![FilePatch {
                path: path.to_string(),
                hunks,
                delete_after: false,
                create_if_missing,
            }]
        } else {
            let file_patches = parse_unified_diff_files(patch_text, create_if_missing)?;
            if file_patches.is_empty() {
                return Err(ToolError::invalid_input(
                    "No valid file patches found. Ensure the patch includes `---`/`+++` headers or provide `path`.",
                ));
            }
            file_patches
        };

        let (pending, mut stats) = build_pending_writes_from_patches(file_patches, context, fuzz)?;
        if stats.header_path_mismatch.is_none() {
            stats.header_path_mismatch = mismatch_note;
        }
        apply_pending_writes(&pending)?;
        // Resolve absolute paths for LSP diagnostics query.
        let abs_paths: Vec<PathBuf> = pending
            .iter()
            .filter(|p| p.content.is_some()) // skip deleted files
            .map(|p| p.path.clone())
            .collect();
        let diag_block = lsp_diagnostics_for_paths(context, &abs_paths).await;
        let result = PatchResult {
            success: true,
            files_applied: stats.stats.files_applied,
            files_total: stats.stats.files_total,
            hunks_applied: stats.stats.hunks_applied,
            hunks_total: stats.stats.hunks_total,
            fuzz_used: stats.stats.fuzz_used,
            hunks_with_fuzz: stats.stats.hunks_with_fuzz,
            touched_files: stats.touched_files.clone(),
            file_summaries: stats.file_summaries.clone(),
            message: build_summary_message(&stats),
        };
        let mut tool_result =
            ToolResult::json(&result).map_err(|e| ToolError::execution_failed(e.to_string()))?;
        if !diag_block.is_empty() {
            tool_result.content.push('\n');
            tool_result.content.push_str(&diag_block);
        }
        Ok(tool_result)
    }
}

/// Parse a unified diff into hunks
fn parse_unified_diff(patch: &str) -> Result<Vec<Hunk>, ToolError> {
    let mut hunks = Vec::new();
    let mut lines = patch.lines().peekable();

    // Skip header lines (---, +++ etc)
    while let Some(line) = lines.peek() {
        if line.starts_with("@@") {
            break;
        }
        lines.next();
    }

    // Parse hunks
    while let Some(line) = lines.next() {
        if line.starts_with("@@") {
            let hunk = parse_hunk_header(line, &mut lines)?;
            hunks.push(hunk);
        }
    }

    Ok(hunks)
}

fn parse_unified_diff_files(
    patch: &str,
    create_if_missing: bool,
) -> Result<Vec<FilePatch>, ToolError> {
    let mut files = Vec::new();
    let mut lines = patch.lines().peekable();
    let mut current: Option<FilePatch> = None;
    let mut old_path: Option<String> = None;

    while let Some(line) = lines.next() {
        if line.starts_with("diff --git ") {
            if let Some(file) = current.take() {
                files.push(file);
            }
            old_path = None;
            continue;
        }

        if let Some(stripped) = line.strip_prefix("--- ") {
            old_path = Some(stripped.trim().to_string());
            continue;
        }

        if let Some(stripped) = line.strip_prefix("+++ ") {
            let new_path = Some(stripped.trim().to_string());
            let (path, delete_after, create_flag) =
                resolve_diff_paths(old_path.as_deref(), new_path.as_deref(), create_if_missing)?;
            old_path = None;
            if let Some(file) = current.take() {
                files.push(file);
            }
            current = Some(FilePatch {
                path,
                hunks: Vec::new(),
                delete_after,
                create_if_missing: create_flag,
            });
            continue;
        }

        if line.starts_with("@@") {
            let Some(file) = current.as_mut() else {
                if let Some(path) = old_path.as_deref() {
                    return Err(ToolError::invalid_input(format!(
                        "Patch hunk encountered after `--- {path}` but before a matching `+++` header. Each file section must include both headers."
                    )));
                }
                return Err(ToolError::invalid_input(
                    "Patch hunk encountered before any file header. Add `---`/`+++` headers or provide `path`.",
                ));
            };
            let hunk = parse_hunk_header(line, &mut lines)?;
            file.hunks.push(hunk);
        }
    }

    if let Some(file) = current {
        files.push(file);
    }

    Ok(files)
}

fn resolve_diff_paths(
    old_path: Option<&str>,
    new_path: Option<&str>,
    create_if_missing: bool,
) -> Result<(String, bool, bool), ToolError> {
    let old_norm = old_path.and_then(normalize_diff_path);
    let new_norm = new_path.and_then(normalize_diff_path);
    let delete_after = new_norm.is_none();
    let create_flag = create_if_missing || old_norm.is_none();
    let path = new_norm
        .or(old_norm)
        .ok_or_else(|| ToolError::invalid_input("Patch is missing both old and new file paths"))?;
    Ok((path, delete_after, create_flag))
}

fn normalize_diff_path(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw == "/dev/null" || raw == "dev/null" {
        return None;
    }
    let raw = raw
        .strip_prefix("a/")
        .or_else(|| raw.strip_prefix("b/"))
        .unwrap_or(raw);
    Some(raw.to_string())
}

/// Parse a hunk header and its content
fn parse_hunk_header<'a, I>(
    header: &str,
    lines: &mut std::iter::Peekable<I>,
) -> Result<Hunk, ToolError>
where
    I: Iterator<Item = &'a str>,
{
    // Parse @@ -old_start,old_count +new_start,new_count @@
    let parts: Vec<&str> = header.split_whitespace().collect();
    if parts.len() < 3 {
        return Err(ToolError::invalid_input(format!(
            "Invalid hunk header: {header}. Expected `@@ -start,count +start,count @@`."
        )));
    }

    let old_range = parts[1].trim_start_matches('-');
    let new_range = parts[2].trim_start_matches('+');

    let (old_start, old_count) = parse_range(old_range)?;
    let (new_start, new_count) = parse_range(new_range)?;

    // Parse hunk lines
    let mut hunk_lines = Vec::new();
    let expected_lines = old_count.max(new_count) + old_count.min(new_count);

    for _ in 0..expected_lines * 2 {
        // Allow for more lines than expected
        match lines.peek() {
            Some(line) if line.starts_with("@@") => break,
            Some(line) if line.starts_with('-') => {
                hunk_lines.push(HunkLine::Remove(line[1..].to_string()));
                lines.next();
            }
            Some(line) if line.starts_with('+') => {
                hunk_lines.push(HunkLine::Add(line[1..].to_string()));
                lines.next();
            }
            Some(line) if line.starts_with(' ') || line.is_empty() => {
                let content = if line.is_empty() { "" } else { &line[1..] };
                hunk_lines.push(HunkLine::Context(content.to_string()));
                lines.next();
            }
            Some(line)
                if line.starts_with("diff ")
                    || line.starts_with("--- ")
                    || line.starts_with("+++ ") =>
            {
                // Start of a new file patch - don't consume, let outer loop handle it
                break;
            }
            Some(line) if !line.starts_with('\\') => {
                // Treat as context line without leading space
                hunk_lines.push(HunkLine::Context((*line).to_string()));
                lines.next();
            }
            Some(_) => {
                lines.next(); // Skip "\ No newline at end of file" etc
            }
            None => break,
        }
    }

    Ok(Hunk {
        old_start,
        old_count,
        new_start,
        new_count,
        lines: hunk_lines,
    })
}

/// Parse a range like "10,5" or "10" into (start, count)
fn parse_range(range: &str) -> Result<(usize, usize), ToolError> {
    let parts: Vec<&str> = range.split(',').collect();
    let start = parts[0].parse::<usize>().map_err(|_| {
        ToolError::invalid_input(format!(
            "Invalid line number `{}` in hunk header. Use positive integers like `12` or `12,3`.",
            parts[0]
        ))
    })?;
    let count = if parts.len() > 1 {
        parts[1].parse::<usize>().map_err(|_| {
            ToolError::invalid_input(format!(
                "Invalid line count `{}` in hunk header. Use positive integers like `3`.",
                parts[1]
            ))
        })?
    } else {
        1
    };
    Ok((start, count))
}

fn inspect_patch_shape(patch: &str) -> PatchShape {
    let mut shape = PatchShape::default();
    let mut seen = HashSet::new();
    let mut old_path: Option<String> = None;

    for line in patch.lines() {
        if line.starts_with("@@") {
            shape.has_hunks = true;
        }

        if let Some(stripped) = line.strip_prefix("--- ") {
            old_path = normalize_diff_path(stripped);
            continue;
        }

        if let Some(stripped) = line.strip_prefix("+++ ") {
            let new_path = normalize_diff_path(stripped);
            let resolved = new_path.or(old_path.clone());
            if let Some(path) = resolved
                && seen.insert(path.clone())
            {
                shape.header_files.push(path);
            }
            old_path = None;
        }
    }

    shape
}

fn validate_patch_shape(shape: &PatchShape, path_override: Option<&str>) -> Result<(), ToolError> {
    if !shape.has_hunks {
        return Err(ToolError::invalid_input(
            "Patch must include at least one hunk header (`@@ -start,count +start,count @@`).",
        ));
    }

    match path_override {
        Some(_) if shape.file_count() > 1 => Err(ToolError::invalid_input(format!(
            "Patch references multiple files ({}) but `path` was provided. Remove `path` to apply a multi-file patch, or provide a single-file patch.",
            format_file_list(&shape.header_files),
        ))),
        None if shape.file_count() == 0 => Err(ToolError::invalid_input(
            "Patch contains hunks but no file headers (`---`/`+++`). Provide `path` or add headers.",
        )),
        _ => Ok(()),
    }
}

fn diff_header_mismatch(path_override: &str, shape: &PatchShape) -> Option<String> {
    if shape.file_count() != 1 {
        return None;
    }
    let header_path = &shape.header_files[0];
    let override_norm = normalize_diff_path(path_override).unwrap_or_else(|| path_override.into());
    if &override_norm == header_path {
        None
    } else {
        Some(format!(
            "Note: patch headers reference `{header_path}` but `path` overrides to `{override_norm}`."
        ))
    }
}

fn build_summary_message(stats: &PatchStatsExt) -> String {
    let mut parts = Vec::new();
    if stats.stats.hunks_total > 0 {
        parts.push(format!(
            "Applied {}/{} hunks across {} file(s).",
            stats.stats.hunks_applied, stats.stats.hunks_total, stats.stats.files_applied
        ));
    } else {
        parts.push(format!(
            "Applied {} file change(s).",
            stats.stats.files_applied
        ));
    }

    if !stats.touched_files.is_empty() {
        parts.push(format!(
            "Files: {}.",
            format_file_list(&stats.touched_files)
        ));
    }

    if stats.stats.fuzz_used > 0 {
        parts.push(format!(
            "Fuzz used on {} hunk(s) (total fuzz: {}).",
            stats.stats.hunks_with_fuzz, stats.stats.fuzz_used
        ));
    }

    if let Some(note) = stats.header_path_mismatch.as_deref() {
        parts.push(note.to_string());
    }

    parts.join(" ")
}

fn format_file_list(files: &[String]) -> String {
    if files.is_empty() {
        return "<none>".to_string();
    }
    let mut shown: Vec<String> = files.iter().take(FILE_LIST_LIMIT).cloned().collect();
    let remaining = files.len().saturating_sub(shown.len());
    if remaining > 0 {
        shown.push(format!("... (+{remaining} more)"));
    }
    shown.join(", ")
}

fn push_unique(target: &mut Vec<String>, value: String) {
    if !target.iter().any(|existing| existing == &value) {
        target.push(value);
    }
}

fn build_pending_writes_from_changes(
    changes_value: &Value,
    context: &ToolContext,
) -> Result<(Vec<PendingWrite>, PatchStatsExt), ToolError> {
    let changes = changes_value.as_array().ok_or_else(|| {
        ToolError::invalid_input("`changes` must be an array of objects like {path, content}")
    })?;
    if changes.is_empty() {
        return Err(ToolError::invalid_input("`changes` cannot be empty"));
    }

    let mut pending = Vec::new();
    let mut stats = PatchStatsExt::default();
    for change in changes {
        let path = change
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::missing_field("changes[].path"))?;
        let content = change
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::missing_field("changes[].content"))?;

        let resolved = context.resolve_path(path)?;
        let original = if resolved.exists() {
            Some(read_file_content(&resolved)?)
        } else {
            None
        };
        let created = original.is_none();

        pending.push(PendingWrite {
            path: resolved,
            content: Some(content.to_string()),
            original,
        });

        stats.stats.files_total += 1;
        stats.stats.files_applied += 1;
        push_unique(&mut stats.touched_files, path.to_string());
        stats.file_summaries.push(FileSummary {
            path: path.to_string(),
            hunks: 0,
            hunks_applied: 0,
            fuzz_used: 0,
            hunks_with_fuzz: 0,
            created,
            deleted: false,
        });
    }

    Ok((pending, stats))
}

fn build_pending_writes_from_patches(
    file_patches: Vec<FilePatch>,
    context: &ToolContext,
    fuzz: usize,
) -> Result<(Vec<PendingWrite>, PatchStatsExt), ToolError> {
    let mut pending = Vec::new();
    let mut stats = PatchStatsExt::default();
    stats.stats.files_total = file_patches.len();

    for file_patch in file_patches {
        if file_patch.hunks.is_empty() {
            return Err(ToolError::invalid_input(format!(
                "Patch section for `{}` has no hunks (`@@ ... @@`).",
                file_patch.path
            )));
        }

        let resolved = context.resolve_path(&file_patch.path)?;
        let original = if resolved.exists() {
            Some(read_file_content(&resolved)?)
        } else {
            None
        };

        if original.is_none() && !file_patch.create_if_missing {
            return Err(ToolError::execution_failed(format!(
                "File `{}` does not exist at `{}`. Set create_if_missing=true for new files or include headers for file creation.",
                file_patch.path,
                resolved.display(),
            )));
        }

        if file_patch.delete_after && original.is_none() {
            return Err(ToolError::execution_failed(format!(
                "File `{}` does not exist at `{}` to delete.",
                file_patch.path,
                resolved.display(),
            )));
        }

        let base_content = original.clone().unwrap_or_default();
        let mut lines: Vec<String> = if base_content.is_empty() {
            Vec::new()
        } else {
            base_content.lines().map(String::from).collect()
        };

        let apply_stats =
            apply_hunks_to_lines(&mut lines, &file_patch.hunks, fuzz, &file_patch.path)?;
        stats.stats.hunks_applied += apply_stats.hunks_applied;
        stats.stats.hunks_total += file_patch.hunks.len();
        stats.stats.fuzz_used += apply_stats.fuzz_used;
        stats.stats.hunks_with_fuzz += apply_stats.hunks_with_fuzz;
        stats.stats.files_applied += 1;
        push_unique(&mut stats.touched_files, file_patch.path.clone());
        stats.file_summaries.push(FileSummary {
            path: file_patch.path.clone(),
            hunks: file_patch.hunks.len(),
            hunks_applied: apply_stats.hunks_applied,
            fuzz_used: apply_stats.fuzz_used,
            hunks_with_fuzz: apply_stats.hunks_with_fuzz,
            created: original.is_none() && !file_patch.delete_after,
            deleted: file_patch.delete_after,
        });

        if file_patch.delete_after {
            pending.push(PendingWrite {
                path: resolved,
                content: None,
                original,
            });
        } else {
            let new_content = lines.join("\n");
            pending.push(PendingWrite {
                path: resolved,
                content: Some(new_content),
                original,
            });
        }
    }

    Ok((pending, stats))
}

fn apply_pending_writes(pending: &[PendingWrite]) -> Result<(), ToolError> {
    let mut applied = Vec::new();

    for entry in pending {
        let result = if let Some(content) = entry.content.as_ref() {
            if let Some(parent) = entry.path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    ToolError::execution_failed(format!(
                        "Failed to create directory {}: {}",
                        parent.display(),
                        e
                    ))
                })?;
            }
            fs::write(&entry.path, content).map_err(|e| {
                ToolError::execution_failed(format!(
                    "Failed to write {}: {}",
                    entry.path.display(),
                    e
                ))
            })
        } else if entry.path.exists() {
            fs::remove_file(&entry.path).map_err(|e| {
                ToolError::execution_failed(format!(
                    "Failed to delete {}: {}",
                    entry.path.display(),
                    e
                ))
            })
        } else {
            Ok(())
        };

        if let Err(err) = result {
            rollback_pending_writes(&applied);
            return Err(err);
        }

        applied.push(entry.clone());
    }

    Ok(())
}

fn rollback_pending_writes(applied: &[PendingWrite]) {
    for entry in applied.iter().rev() {
        match entry.original.as_ref() {
            Some(content) => {
                let _ = fs::write(&entry.path, content);
            }
            None => {
                let _ = fs::remove_file(&entry.path);
            }
        }
    }
}

fn read_file_content(path: &PathBuf) -> Result<String, ToolError> {
    fs::read_to_string(path).map_err(|e| {
        ToolError::execution_failed(format!("Failed to read {}: {}", path.display(), e))
    })
}

fn preview_expected_lines(hunk: &Hunk, limit: usize) -> Vec<String> {
    let mut preview = Vec::new();
    for line in hunk.lines.iter().filter_map(|line| match line {
        HunkLine::Context(s) => Some((" ", s)),
        HunkLine::Remove(s) => Some(("-", s)),
        HunkLine::Add(_) => None,
    }) {
        if preview.len() >= limit {
            break;
        }
        preview.push(format!("  {}{}", line.0, line.1));
    }
    if preview.is_empty() {
        preview.push("  <no context lines in hunk>".to_string());
    }
    preview
}

fn snippet_around(lines: &[String], line_1_based: usize, radius: usize) -> Vec<String> {
    if lines.is_empty() {
        return vec!["  <empty file>".to_string()];
    }

    let center = line_1_based
        .saturating_sub(1)
        .min(lines.len().saturating_sub(1));
    let start = center.saturating_sub(radius);
    let end = (center + radius).min(lines.len().saturating_sub(1));

    lines[start..=end]
        .iter()
        .enumerate()
        .map(|(idx, line)| {
            let line_no = start + idx + 1;
            format!("  {line_no:>4}: {line}")
        })
        .collect()
}

fn format_hunk_no_match_error(
    lines: &[String],
    hunk: &Hunk,
    err: &ApplyHunkError,
    max_fuzz: usize,
) -> String {
    match err {
        ApplyHunkError::NoMatch {
            expected_line,
            adjusted_line,
            offset,
        } => {
            let expected_preview = preview_expected_lines(hunk, HUNK_PREVIEW_LINES).join("\n");
            let file_preview = snippet_around(lines, *adjusted_line, SNIPPET_RADIUS).join("\n");
            format!(
                "could not find matching context near line {expected_line} (searched around line {adjusted_line} with offset {offset:+} and fuzz up to {max_fuzz}). Expected context preview:\n{expected_preview}\nFile snippet near line {adjusted_line}:\n{file_preview}\nHints: ensure the patch matches the current file contents, increase `fuzz`, or regenerate the patch."
            )
        }
    }
}

fn apply_hunks_to_lines(
    lines: &mut Vec<String>,
    hunks: &[Hunk],
    fuzz: usize,
    file_label: &str,
) -> Result<HunkApplyStats, ToolError> {
    let mut stats = HunkApplyStats::default();
    let mut cumulative_offset: isize = 0;

    for (idx, hunk) in hunks.iter().enumerate() {
        match apply_hunk(lines, hunk, fuzz, &mut cumulative_offset) {
            Ok(fuzz_used) => {
                stats.fuzz_used += fuzz_used;
                stats.hunks_applied += 1;
                if fuzz_used > 0 {
                    stats.hunks_with_fuzz += 1;
                }
            }
            Err(e) => {
                let detail = format_hunk_no_match_error(lines, hunk, &e, fuzz);
                return Err(ToolError::execution_failed(format!(
                    "Failed to apply hunk {}/{} for `{}`: {}",
                    idx + 1,
                    hunks.len(),
                    file_label,
                    detail
                )));
            }
        }
    }

    Ok(stats)
}

/// Apply a hunk to the file content with fuzzy matching
fn apply_hunk(
    lines: &mut Vec<String>,
    hunk: &Hunk,
    max_fuzz: usize,
    cumulative_offset: &mut isize,
) -> Result<usize, ApplyHunkError> {
    // Build expected old lines from hunk
    let old_lines: Vec<&str> = hunk
        .lines
        .iter()
        .filter_map(|line| match line {
            HunkLine::Context(s) | HunkLine::Remove(s) => Some(s.as_str()),
            HunkLine::Add(_) => None,
        })
        .collect();

    // Build new lines from hunk
    let new_lines: Vec<String> = hunk
        .lines
        .iter()
        .filter_map(|line| match line {
            HunkLine::Context(s) | HunkLine::Add(s) => Some(s.clone()),
            HunkLine::Remove(_) => None,
        })
        .collect();

    // Try to find the location with fuzzy matching
    // Apply cumulative offset from previous hunks
    let base_idx = if hunk.old_start > 0 {
        hunk.old_start - 1
    } else {
        0
    };
    let start_idx = ((base_idx as isize) + *cumulative_offset).max(0) as usize;

    for fuzz in 0..=max_fuzz {
        // Try at exact position first, then nearby
        let search_range = if fuzz == 0 {
            vec![start_idx]
        } else {
            let min = start_idx.saturating_sub(fuzz);
            let max = (start_idx + fuzz).min(lines.len());
            (min..=max).collect()
        };

        for pos in search_range {
            if matches_at_position(lines, &old_lines, pos) {
                // Apply the hunk
                let end_pos = pos + old_lines.len();
                lines.splice(pos..end_pos, new_lines.clone());

                // Update cumulative offset: new lines added minus old lines removed
                let delta = new_lines.len() as isize - old_lines.len() as isize;
                *cumulative_offset += delta;

                return Ok(fuzz);
            }
        }
    }

    // Special case: adding to empty file or new hunk at end
    if old_lines.is_empty() && (lines.is_empty() || start_idx >= lines.len()) {
        let delta = new_lines.len() as isize;
        lines.extend(new_lines);
        *cumulative_offset += delta;
        return Ok(0);
    }

    Err(ApplyHunkError::NoMatch {
        expected_line: hunk.old_start,
        adjusted_line: start_idx + 1, // Convert back to 1-indexed
        offset: *cumulative_offset,
    })
}

/// Check if `old_lines` match at the given position
fn matches_at_position(lines: &[String], old_lines: &[&str], pos: usize) -> bool {
    if pos + old_lines.len() > lines.len() {
        return false;
    }

    for (i, old_line) in old_lines.iter().enumerate() {
        // Normalize whitespace for comparison
        let file_line = lines[pos + i].trim_end();
        let expected = old_line.trim_end();
        if file_line != expected {
            return false;
        }
    }

    true
}

// === Unit Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn parse_patch_result(result: ToolResult) -> PatchResult {
        serde_json::from_str(&result.content).expect("patch result json")
    }

    #[test]
    fn test_parse_range() {
        assert_eq!(parse_range("10,5").unwrap(), (10, 5));
        assert_eq!(parse_range("10").unwrap(), (10, 1));
        assert_eq!(parse_range("1,0").unwrap(), (1, 0));
    }

    #[test]
    fn test_parse_unified_diff() {
        let patch = r"--- a/test.txt
+++ b/test.txt
@@ -1,3 +1,3 @@
 line1
-line2
+modified line2
 line3
";

        let hunks = parse_unified_diff(patch).unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_count, 3);
        assert_eq!(hunks[0].new_start, 1);
        assert_eq!(hunks[0].new_count, 3);
    }

    #[test]
    fn test_apply_hunk_simple() {
        let mut lines = vec![
            "line1".to_string(),
            "line2".to_string(),
            "line3".to_string(),
        ];

        let hunk = Hunk {
            old_start: 1,
            old_count: 3,
            new_start: 1,
            new_count: 3,
            lines: vec![
                HunkLine::Context("line1".to_string()),
                HunkLine::Remove("line2".to_string()),
                HunkLine::Add("modified".to_string()),
                HunkLine::Context("line3".to_string()),
            ],
        };

        let mut offset: isize = 0;
        let fuzz = apply_hunk(&mut lines, &hunk, 0, &mut offset).unwrap();
        assert_eq!(fuzz, 0);
        assert_eq!(lines, vec!["line1", "modified", "line3"]);
    }

    #[test]
    fn test_apply_hunk_with_fuzz() {
        let mut lines = vec![
            "line0".to_string(),
            "line1".to_string(),
            "line2".to_string(),
            "line3".to_string(),
        ];

        // Hunk expects to start at line 1, but content is at line 2
        let hunk = Hunk {
            old_start: 1, // Wrong position
            old_count: 2,
            new_start: 1,
            new_count: 2,
            lines: vec![
                HunkLine::Remove("line1".to_string()),
                HunkLine::Add("modified".to_string()),
                HunkLine::Context("line2".to_string()),
            ],
        };

        let mut offset: isize = 0;
        let fuzz = apply_hunk(&mut lines, &hunk, 3, &mut offset).unwrap();
        assert!(fuzz > 0);
        assert_eq!(lines, vec!["line0", "modified", "line2", "line3"]);
    }

    #[test]
    fn test_apply_hunk_no_match_returns_error() {
        let mut lines = vec!["line1".to_string(), "line2".to_string()];
        let hunk = Hunk {
            old_start: 5,
            old_count: 1,
            new_start: 5,
            new_count: 1,
            lines: vec![
                HunkLine::Context("missing".to_string()),
                HunkLine::Add("new".to_string()),
            ],
        };

        let mut offset: isize = 0;
        let err = apply_hunk(&mut lines, &hunk, 0, &mut offset).unwrap_err();
        assert!(matches!(
            err,
            ApplyHunkError::NoMatch {
                expected_line: 5,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn test_apply_patch_tool() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // Create a test file
        fs::write(tmp.path().join("test.txt"), "line1\nline2\nline3\n").expect("write");

        let patch = r"--- a/test.txt
+++ b/test.txt
@@ -1,3 +1,3 @@
 line1
-line2
+modified
 line3
";

        let tool = ApplyPatchTool;
        let result = tool
            .execute(json!({"path": "test.txt", "patch": patch}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        let patch_result = parse_patch_result(result);
        assert_eq!(patch_result.touched_files, vec!["test.txt"]);
        assert_eq!(patch_result.hunks_applied, 1);

        // Verify the patch was applied
        let content = fs::read_to_string(tmp.path().join("test.txt")).expect("read");
        assert!(content.contains("modified"));
        assert!(!content.contains("line2"));
    }

    #[tokio::test]
    async fn test_apply_patch_add_lines() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::write(tmp.path().join("test.txt"), "line1\nline3\n").expect("write");

        let patch = r"@@ -1,2 +1,3 @@
 line1
+line2
 line3
";

        let tool = ApplyPatchTool;
        let result = tool
            .execute(json!({"path": "test.txt", "patch": patch}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        let patch_result = parse_patch_result(result);
        assert_eq!(patch_result.touched_files, vec!["test.txt"]);

        let content = fs::read_to_string(tmp.path().join("test.txt")).expect("read");
        assert!(content.contains("line2"));
    }

    #[tokio::test]
    async fn test_apply_patch_create_new_file() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let patch = r"@@ -0,0 +1,3 @@
+line1
+line2
+line3
";

        let tool = ApplyPatchTool;
        let result = tool
            .execute(
                json!({"path": "new_file.txt", "patch": patch, "create_if_missing": true}),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);
        let patch_result = parse_patch_result(result);
        assert_eq!(patch_result.touched_files, vec!["new_file.txt"]);
        assert!(patch_result.file_summaries.first().unwrap().created);
        assert!(tmp.path().join("new_file.txt").exists());
    }

    #[tokio::test]
    async fn test_apply_patch_changes_list() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::write(tmp.path().join("one.txt"), "old\n").expect("write");

        let tool = ApplyPatchTool;
        let result = tool
            .execute(
                json!({
                    "changes": [
                        { "path": "one.txt", "content": "new\n" },
                        { "path": "two.txt", "content": "second\n" }
                    ]
                }),
                &ctx,
            )
            .await
            .expect("execute");

        assert!(result.success);
        let patch_result = parse_patch_result(result);
        let mut touched = patch_result.touched_files.clone();
        touched.sort();
        assert_eq!(touched, vec!["one.txt", "two.txt"]);
        assert_eq!(patch_result.hunks_total, 0);
        assert_eq!(
            fs::read_to_string(tmp.path().join("one.txt")).unwrap(),
            "new\n"
        );
        assert_eq!(
            fs::read_to_string(tmp.path().join("two.txt")).unwrap(),
            "second\n"
        );
    }

    #[tokio::test]
    async fn test_apply_patch_multi_file_diff() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::write(tmp.path().join("a.txt"), "line1\nline2\n").expect("write");
        fs::write(tmp.path().join("b.txt"), "alpha\nbeta\n").expect("write");

        let patch = r"diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,2 +1,2 @@
 line1
-line2
+line2-mod
diff --git a/b.txt b/b.txt
--- a/b.txt
+++ b/b.txt
@@ -1,2 +1,3 @@
 alpha
+beta2
 beta
";

        let tool = ApplyPatchTool;
        let result = tool
            .execute(json!({"patch": patch}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        let patch_result = parse_patch_result(result);
        let mut touched = patch_result.touched_files.clone();
        touched.sort();
        assert_eq!(touched, vec!["a.txt", "b.txt"]);
        assert_eq!(patch_result.files_applied, 2);
        let a = fs::read_to_string(tmp.path().join("a.txt")).unwrap();
        let b = fs::read_to_string(tmp.path().join("b.txt")).unwrap();
        assert!(a.contains("line2-mod"));
        assert!(b.contains("beta2"));
    }

    #[tokio::test]
    async fn test_apply_patch_requires_headers_without_path() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = ApplyPatchTool;

        let patch = r"@@ -1,1 +1,1 @@
-old
+new
";

        let err = tool
            .execute(json!({"patch": patch}), &ctx)
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidInput { message } => {
                assert!(message.contains("no file headers"));
                assert!(message.contains("Provide `path`"));
            }
            other => panic!("expected invalid input, got: {other}"),
        }
    }

    #[tokio::test]
    async fn test_path_override_rejects_multi_file_diff() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = ApplyPatchTool;

        let patch = r"diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,1 +1,1 @@
-one
+one-mod
diff --git a/b.txt b/b.txt
--- a/b.txt
+++ b/b.txt
@@ -1,1 +1,1 @@
-two
+two-mod
";

        let err = tool
            .execute(json!({"path": "a.txt", "patch": patch}), &ctx)
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidInput { message } => {
                assert!(message.contains("multiple files"));
                assert!(message.contains("a.txt"));
                assert!(message.contains("b.txt"));
            }
            other => panic!("expected invalid input, got: {other}"),
        }
    }

    #[tokio::test]
    async fn test_apply_patch_summary_reports_fuzz() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = ApplyPatchTool;

        fs::write(tmp.path().join("test.txt"), "line0\nline1\nline2\nline3\n").expect("write");

        let patch = r"@@ -1,2 +1,2 @@
-line1
+modified
 line2
";

        let result = tool
            .execute(json!({"path": "test.txt", "patch": patch, "fuzz": 3}), &ctx)
            .await
            .expect("execute");
        assert!(result.success);
        let patch_result = parse_patch_result(result);
        assert_eq!(patch_result.hunks_with_fuzz, 1);
        assert!(patch_result.fuzz_used > 0);
        assert!(patch_result.message.contains("Fuzz used"));
        let summary = patch_result.file_summaries.first().unwrap();
        assert_eq!(summary.hunks_with_fuzz, 1);
    }

    #[tokio::test]
    async fn test_path_override_header_mismatch_note() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = ApplyPatchTool;

        fs::write(tmp.path().join("override.txt"), "old\n").expect("write");

        let patch = r"--- a/other.txt
+++ b/other.txt
@@ -1,1 +1,1 @@
-old
+new
";

        let result = tool
            .execute(json!({"path": "override.txt", "patch": patch}), &ctx)
            .await
            .expect("execute");
        let patch_result = parse_patch_result(result);
        assert!(
            patch_result
                .message
                .contains("headers reference `other.txt`")
        );
        assert!(
            patch_result
                .message
                .contains("path` overrides to `override.txt`")
        );
    }

    #[test]
    fn test_apply_patch_tool_properties() {
        let tool = ApplyPatchTool;
        assert_eq!(tool.name(), "apply_patch");
        assert!(!tool.is_read_only());
        assert!(tool.is_sandboxable());
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Suggest);
    }

    #[test]
    fn test_multi_hunk_offset_tracking() {
        // File with 6 lines
        let mut lines: Vec<String> = vec![
            "line1".to_string(),
            "line2".to_string(),
            "line3".to_string(),
            "line4".to_string(),
            "line5".to_string(),
            "line6".to_string(),
        ];

        // Hunk 1: Add 2 lines after line1 (offset becomes +2)
        let hunk1 = Hunk {
            old_start: 1,
            old_count: 2,
            new_start: 1,
            new_count: 4,
            lines: vec![
                HunkLine::Context("line1".to_string()),
                HunkLine::Add("new_a".to_string()),
                HunkLine::Add("new_b".to_string()),
                HunkLine::Context("line2".to_string()),
            ],
        };

        // Hunk 2: Modify line5 (originally at position 5, now at position 7 due to +2 offset)
        let hunk2 = Hunk {
            old_start: 5, // Original position in the diff
            old_count: 1,
            new_start: 7,
            new_count: 1,
            lines: vec![
                HunkLine::Remove("line5".to_string()),
                HunkLine::Add("modified5".to_string()),
            ],
        };

        let mut offset: isize = 0;

        // Apply first hunk
        let fuzz1 = apply_hunk(&mut lines, &hunk1, 3, &mut offset).unwrap();
        assert_eq!(fuzz1, 0);
        assert_eq!(offset, 2); // Added 2 lines (4 new - 2 old)
        assert_eq!(
            lines,
            vec![
                "line1", "new_a", "new_b", "line2", "line3", "line4", "line5", "line6"
            ]
        );

        // Apply second hunk - this would fail without offset tracking!
        let fuzz2 = apply_hunk(&mut lines, &hunk2, 3, &mut offset).unwrap();
        assert_eq!(fuzz2, 0);
        assert!(lines.contains(&"modified5".to_string()));
        assert!(!lines.contains(&"line5".to_string()));
    }
}
