//! Tool approval system for `DeepSeek` CLI.
//!
//! Hosts the [`ApprovalRequest`] / [`ApprovalView`] pair the engine asks
//! the TUI to present whenever a tool needs human approval, plus the
//! sandbox elevation flow ([`ElevationRequest`] / [`ElevationView`]) that
//! follows a sandbox denial.
//!
//! ## v0.6.7: Codex-style takeover with stakes-based variants (#129)
//!
//! The modal now renders as a full-screen takeover (calm centered card
//! against the transcript area) and routes each request to one of two
//! stakes-based variants:
//!
//! - **Benign** (`RiskLevel::Benign`) — read-only ops, MCP discovery,
//!   query-only network. A single `Enter` / `1` / `y` approves once;
//!   `2` / `a` approves for the session.
//! - **Destructive** (`RiskLevel::Destructive`) — file writes, shell,
//!   patches, MCP actions, unclassified tools, and any "fetch arbitrary
//!   content" surface. The takeover keeps the destructive badge and
//!   impact summary visible, then lets `Enter` commit the highlighted
//!   option or `y` / `a` / `d` commit directly.
//!
//! The decision events emitted upstream are unchanged
//! (`ViewEvent::ApprovalDecision`), so `ui.rs` and the engine handle
//! both variants without modification. Auto-approve / YOLO bypasses
//! happen *before* the view is constructed (see `tui/ui.rs`); this
//! module always assumes the user is being asked.

use crate::localization::Locale;
use crate::palette;
use crate::sandbox::SandboxPolicy;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};
use crate::tui::widgets::{ApprovalWidget, ElevationWidget, Renderable};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const DIFF_PREVIEW_MAX_BYTES: u64 = 512 * 1024;

/// Determines when tool executions require user approval
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApprovalMode {
    /// Auto-approve all tools (YOLO mode / --yolo flag)
    Auto,
    /// Suggest approval for non-safe tools (non-YOLO modes)
    #[default]
    Suggest,
    /// Never execute tools requiring approval
    Never,
}

impl ApprovalMode {
    pub fn label(self) -> &'static str {
        match self {
            ApprovalMode::Auto => "AUTO",
            ApprovalMode::Suggest => "SUGGEST",
            ApprovalMode::Never => "NEVER",
        }
    }

    pub fn from_config_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(ApprovalMode::Auto),
            "suggest" | "suggested" | "on-request" | "untrusted" => Some(ApprovalMode::Suggest),
            "never" | "deny" | "denied" => Some(ApprovalMode::Never),
            _ => None,
        }
    }
}

/// User's decision for a pending approval
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewDecision {
    /// Execute this tool once
    Approved,
    /// Approve and don't ask again for this tool type this session
    ApprovedForSession,
    /// Reject the tool execution
    Denied,
    /// Abort the entire turn
    Abort,
}

/// Categorizes tools by cost/risk level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCategory {
    /// Free, read-only operations (`list_dir`, `read_file`, todo_*)
    Safe,
    /// File modifications (`write_file`, `edit_file`)
    FileWrite,
    /// Shell execution (`exec_shell`)
    Shell,
    /// Network-oriented built-in tools
    Network,
    /// Read-only MCP discovery and resource access
    McpRead,
    /// MCP actions that may change remote state
    McpAction,
    /// Unknown or unclassified tool surface
    Unknown,
}

/// Stakes-based variant for the takeover modal.
///
/// `RiskLevel::Benign` lets a single keystroke commit the approval.
/// `RiskLevel::Destructive` keeps stronger warning copy and styling
/// around approvals that can touch files, shell, or remote state.
///
/// Routing rules live in [`classify_risk`] — when in doubt, route to
/// `Destructive`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Benign,
    Destructive,
}

/// Cached diff preview for file-modification tools.
///
/// Built once at `ApprovalRequest` construction time so the modal doesn't
/// re-read the file every render frame. The variants make the "nothing to
/// show" cases explicit instead of collapsing to `None` and silently hiding
/// the preview panel (#1638).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDiffPreview {
    /// Normal unified diff against an existing file (or apply_patch content).
    Diff {
        text: String,
        added: usize,
        deleted: usize,
    },
    /// `write_file` against a path that doesn't exist yet — there's no old
    /// content to diff against, so we show the proposed content as additions.
    NewFile { path: String, content: String },
    /// Content matches the file already — render a calm "no changes" hint
    /// instead of swallowing the whole preview area.
    NoChange { path: String },
    /// `edit_file` search string not present in the file — render a warning
    /// plus a search→replace fallback diff so the user still sees intent.
    MissingMatch {
        path: String,
        text: String,
        match_count: usize,
    },
    /// Existing file is large enough that building a full in-memory diff would
    /// risk a visible pause in the TUI event loop.
    SkippedLargeFile { path: String, size: u64, limit: u64 },
}

impl ApprovalDiffPreview {
    /// Plain unified-diff text suitable for the pager / detail view.
    /// Returns an empty string for non-diff variants.
    #[must_use]
    pub fn diff_text(&self) -> &str {
        match self {
            Self::Diff { text, .. } | Self::MissingMatch { text, .. } => text,
            Self::NewFile { content, .. } => content,
            Self::NoChange { .. } | Self::SkippedLargeFile { .. } => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalDetail {
    pub label: String,
    pub value: String,
    pub shell_lines: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct RenderedDiffPanel {
    pub header: String,
    pub lines: Vec<Line<'static>>,
}

#[derive(Debug, Clone)]
struct DiffPanelCache {
    width: u16,
    locale: Locale,
    panel: RenderedDiffPanel,
}

/// Request for user approval of a tool execution
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// Unique ID for this tool use
    pub id: String,
    /// Tool being executed
    pub tool_name: String,
    /// Tool category
    pub category: ToolCategory,
    /// Stakes-based routing for the takeover modal
    pub risk: RiskLevel,
    /// Tool parameters (for display)
    pub params: Value,
    /// Exact-argument fingerprint, used to scope *denials* (#1617).
    pub approval_key: String,
    /// Lossy / arity-aware fingerprint, used to scope *approvals* so an
    /// "approve for session" covers later flag variants (v0.8.37).
    pub approval_grouping_key: String,
    /// Snapshot of the diff/preview state, built once at construction so
    /// renderers never re-read the filesystem mid-render.
    diff_preview: Option<ApprovalDiffPreview>,
    /// Precomputed popup details, including parsed shell command lines.
    prominent_details: Vec<ApprovalDetail>,
    /// Workspace path used to resolve relative paths for diff previews.
    workspace: Option<String>,
    /// The model's explanation of intent before invoking write tools (#2381).
    /// Displayed in the approval view so users understand *why* the change
    /// is being made before reviewing *what* will change.
    intent_summary: Option<String>,
}

impl ApprovalRequest {
    #[cfg(test)]
    pub fn new(
        id: &str,
        tool_name: &str,
        description: &str,
        params: &Value,
        approval_key: &str,
    ) -> Self {
        let mut req =
            Self::new_with_intent(id, tool_name, description, params, approval_key, None, None);
        req.build_and_set_diff_preview();
        req
    }

    pub fn new_with_intent(
        id: &str,
        tool_name: &str,
        _description: &str,
        params: &Value,
        approval_key: &str,
        workspace: Option<String>,
        intent_summary: Option<&str>,
    ) -> Self {
        let category = get_tool_category(tool_name);
        let risk = classify_risk(tool_name, category, params);
        let approval_grouping_key =
            crate::tools::approval_cache::build_approval_grouping_key(tool_name, params).0;
        let prominent_details = build_prominent_details(category, params, workspace.as_deref());
        // diff_preview is NOT built here — callers should call
        // `build_and_set_diff_preview` separately so the I/O can be
        // offloaded to a blocking thread in async contexts.

        Self {
            id: id.to_string(),
            tool_name: tool_name.to_string(),
            category,
            risk,
            params: params.clone(),
            approval_key: approval_key.to_string(),
            approval_grouping_key,
            diff_preview: None,
            prominent_details,
            workspace,
            intent_summary: intent_summary.and_then(|summary| {
                let summary = summary.trim();
                if summary.is_empty() {
                    None
                } else {
                    Some(summary.to_string())
                }
            }),
        }
    }

    /// Convenience constructor that passes `None` for `intent_summary`.
    /// Prefer `new_with_intent` when the model's reasoning is available.
    pub fn new_with_workspace(
        id: &str,
        tool_name: &str,
        description: &str,
        params: &Value,
        approval_key: &str,
        workspace: Option<String>,
    ) -> Self {
        Self::new_with_intent(
            id,
            tool_name,
            description,
            params,
            approval_key,
            workspace,
            None,
        )
    }

    /// Build the diff preview from disk and store it.  This performs
    /// synchronous filesystem I/O — call it from `spawn_blocking` or
    /// at construction time when not inside an async executor.
    pub fn build_and_set_diff_preview(&mut self) {
        self.diff_preview =
            build_diff_preview(&self.tool_name, &self.params, self.workspace.as_deref());
    }

    /// Extract the most important param values for the approval widget.
    #[must_use]
    pub fn prominent_detail_items(&self, locale: Locale) -> Vec<ApprovalDetail> {
        self.prominent_details
            .iter()
            .map(|detail| ApprovalDetail {
                label: localize_detail_label(&detail.label, locale).to_string(),
                value: detail.value.clone(),
                shell_lines: detail.shell_lines.clone(),
            })
            .collect()
    }

    /// Cached diff/preview snapshot, or `None` when the tool isn't a file
    /// modification. Building happens once at request construction; never
    /// re-reads the filesystem.
    #[must_use]
    pub fn diff_preview(&self) -> Option<&ApprovalDiffPreview> {
        self.diff_preview.as_ref()
    }
}

fn localize_detail_label(label: &str, locale: Locale) -> &str {
    match locale {
        Locale::ZhHans => match label {
            "Command" => "命令",
            "Dir" => "目录",
            "File" => "文件",
            "Path" => "路径",
            "Target" => "目标",
            "Input" => "输入",
            _ => label,
        },
        _ => label,
    }
}

/// Get the category for a tool by name
pub fn get_tool_category(name: &str) -> ToolCategory {
    if matches!(name, "write_file" | "edit_file" | "apply_patch") {
        ToolCategory::FileWrite
    } else if matches!(name, "web_run" | "web_search" | "fetch_url") {
        ToolCategory::Network
    } else if matches!(
        name,
        "exec_shell" | "task_shell_start" | "exec_shell_wait" | "exec_shell_interact"
    ) {
        ToolCategory::Shell
    } else if name.starts_with("list_mcp_")
        || name.starts_with("read_mcp_")
        || name.starts_with("get_mcp_")
    {
        ToolCategory::McpRead
    } else if name.starts_with("mcp_") {
        ToolCategory::McpAction
    } else if matches!(
        name,
        "read_file"
            | "list_dir"
            | "todo_write"
            | "todo_read"
            | "note"
            | "update_plan"
            | "search"
            | "file_search"
            | "project"
            | "diagnostics"
    ) || name.starts_with("read_")
        || name.starts_with("list_")
        || name.starts_with("get_")
    {
        ToolCategory::Safe
    } else {
        ToolCategory::Unknown
    }
}

/// Decide the stakes variant for an approval request.
///
/// The bias is conservative: a category we don't recognise routes to
/// `Destructive`, and any shell command that `command_safety` flags as
/// `Dangerous` is forced to `Destructive` even when the rest of the
/// request looks calm. The split lets the modal render stronger warning
/// copy on anything that can touch state outside this turn.
#[must_use]
pub fn classify_risk(tool_name: &str, category: ToolCategory, params: &Value) -> RiskLevel {
    match category {
        // Read paths and discovery.
        ToolCategory::Safe | ToolCategory::McpRead => RiskLevel::Benign,
        // Query-only network is benign; opening a URL pulls arbitrary
        // remote content, so it stays destructive.
        ToolCategory::Network => match tool_name {
            "web_search" | "web_run" => RiskLevel::Benign,
            _ => RiskLevel::Destructive,
        },
        // Shell is always destructive. We probe command_safety for
        // shape so a future routing tweak (say, pure-readonly `ls`
        // staying benign) lands here without a second pass.
        ToolCategory::Shell => {
            if let Some(cmd) = params
                .get("command")
                .or_else(|| params.get("cmd"))
                .and_then(Value::as_str)
            {
                let _ = crate::command_safety::analyze_command(cmd);
            }
            RiskLevel::Destructive
        }
        // File writes, MCP actions, unclassified surfaces — all
        // require explicit confirmation.
        ToolCategory::FileWrite | ToolCategory::McpAction | ToolCategory::Unknown => {
            RiskLevel::Destructive
        }
    }
}

/// Like [`param_preview`] but never truncates the string value. Used for
/// shell commands so the popup shows what's actually being run instead of
/// `...`-eliding the dangerous tail. The popup body uses `Paragraph::wrap`
/// so long values fold across multiple visual lines on their own.
fn param_text(params: &Value, keys: &[&str]) -> Option<String> {
    let Value::Object(map) = params else {
        return None;
    };
    for key in keys {
        if let Some(Value::String(text)) = map.get(*key) {
            return Some(text.clone());
        }
    }
    None
}

fn build_prominent_details(
    category: ToolCategory,
    params: &Value,
    workspace: Option<&str>,
) -> Vec<ApprovalDetail> {
    let mut details = Vec::new();
    match category {
        ToolCategory::Shell => {
            if let Some(cmd) = param_text(params, &["command", "cmd"]) {
                let shell_lines = format_shell_command_for_approval(&cmd);
                details.push(ApprovalDetail {
                    label: "Command".into(),
                    value: cmd,
                    shell_lines: Some(shell_lines),
                });
            }
            if let Some(raw_dir) = param_text(params, &["workdir", "cwd"])
                .or_else(|| param_preview(params, &["workdir", "cwd"], 96))
            {
                let is_current = workspace.is_some_and(|ws| paths_equivalent(&raw_dir, ws));
                let value = if is_current {
                    "(current)".to_string()
                } else {
                    truncate_string_value(&raw_dir, 96)
                };
                details.push(ApprovalDetail {
                    label: "Dir".into(),
                    value,
                    shell_lines: None,
                });
            }
        }
        ToolCategory::FileWrite => {
            if let Some(path) = param_preview(params, &["path", "target", "destination"], 200) {
                details.push(ApprovalDetail {
                    label: "File".into(),
                    value: path,
                    shell_lines: None,
                });
            }
        }
        ToolCategory::Safe => {
            if let Some(path) = param_preview(params, &["path", "ref_id", "uri"], 200) {
                details.push(ApprovalDetail {
                    label: "Path".into(),
                    value: path,
                    shell_lines: None,
                });
            }
        }
        ToolCategory::Network => {
            if let Some(target) =
                param_preview(params, &["url", "q", "query", "location", "repo"], 200)
            {
                details.push(ApprovalDetail {
                    label: "Target".into(),
                    value: target,
                    shell_lines: None,
                });
            }
        }
        _ => {
            if let Some(val) = param_preview(
                params,
                &["command", "path", "url", "q", "query", "ref_id"],
                200,
            ) {
                details.push(ApprovalDetail {
                    label: "Input".into(),
                    value: val,
                    shell_lines: None,
                });
            }
        }
    }
    details
}

fn paths_equivalent(left: &str, right: &str) -> bool {
    let left = Path::new(left);
    let right = Path::new(right);
    left.canonicalize().unwrap_or_else(|_| left.to_path_buf())
        == right.canonicalize().unwrap_or_else(|_| right.to_path_buf())
}

pub(crate) fn format_shell_command_for_approval(command: &str) -> Vec<String> {
    if let Some(write) = parse_printf_write_file_command(command) {
        return format_printf_write_file_preview(write);
    }

    let mut out = Vec::new();
    for raw in command.split('\n') {
        let mut current = String::new();
        let mut chars = raw.chars().peekable();
        let mut quote: Option<char> = None;
        while let Some(ch) = chars.next() {
            if matches!(ch, '"' | '\'') {
                if quote == Some(ch) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(ch);
                }
                current.push(ch);
                continue;
            }

            if quote.is_none() && ch == '&' && chars.peek() == Some(&'&') {
                chars.next();
                push_shell_clause(&mut out, &mut current, Some("&&"));
                continue;
            }

            if quote.is_none() && ch == '|' {
                if chars.peek() == Some(&'|') {
                    chars.next();
                    push_shell_clause(&mut out, &mut current, Some("||"));
                } else {
                    push_shell_clause(&mut out, &mut current, Some("|"));
                }
                continue;
            }

            if quote.is_none() && ch == ';' {
                current.push(';');
                push_shell_clause(&mut out, &mut current, None);
                continue;
            }

            if quote.is_none() && matches!(ch, '>' | '<') {
                current.push(ch);
                while chars
                    .peek()
                    .is_some_and(|next| matches!(next, '>' | '<' | '&'))
                {
                    current.push(chars.next().expect("peeked char exists"));
                }
                while chars.peek().is_some_and(|next| next.is_whitespace()) {
                    current.push(chars.next().expect("peeked char exists"));
                }
                while chars
                    .peek()
                    .is_some_and(|next| !next.is_whitespace() && !matches!(next, '&' | '|' | ';'))
                {
                    current.push(chars.next().expect("peeked char exists"));
                }
                continue;
            }

            current.push(ch);
        }
        push_shell_clause(&mut out, &mut current, None);
    }

    if out.is_empty() {
        vec![String::new()]
    } else {
        out
    }
}

struct PrintfWriteFilePreview {
    target: String,
    lines: Vec<String>,
}

fn parse_printf_write_file_command(command: &str) -> Option<PrintfWriteFilePreview> {
    let (before_redirect, after_redirect) = split_unquoted_redirect(command)?;
    let before_redirect = before_redirect.trim();
    if !before_redirect.starts_with("printf") {
        return None;
    }
    let tokens = shlex::split(before_redirect)?;
    if tokens.len() < 3 || tokens.first()?.as_str() != "printf" {
        return None;
    }
    let fmt = tokens.get(1)?.as_str();
    if fmt != "%s\n" && fmt != "%s\\n" {
        return None;
    }
    let target_tokens = shlex::split(after_redirect.trim_start_matches(['>', '&']).trim_start())?;
    if target_tokens.len() != 1 {
        return None;
    }
    let target = target_tokens.into_iter().next()?;
    let lines = tokens.into_iter().skip(2).collect::<Vec<_>>();
    if lines.is_empty() {
        return None;
    }
    Some(PrintfWriteFilePreview { target, lines })
}

fn format_printf_write_file_preview(write: PrintfWriteFilePreview) -> Vec<String> {
    let mut out = Vec::new();
    out.push("Write file via printf".to_string());
    out.push(format!("Target: {}", write.target));
    out.push(format!("Lines: {}", write.lines.len()));
    out.push(String::new());
    out.push("Content:".to_string());
    let total = write.lines.len();
    for (idx, line) in write.lines.into_iter().enumerate().take(12) {
        out.push(format!("{:>4}  {}", idx + 1, line));
    }
    if total > 12 {
        out.push(format!("  ... (+{} more lines)", total - 12));
    }
    out
}

fn split_unquoted_redirect(command: &str) -> Option<(&str, &str)> {
    let mut quote: Option<char> = None;
    for (idx, ch) in command.char_indices() {
        if matches!(ch, '"' | '\'') {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            }
            continue;
        }
        if quote.is_none() && ch == '>' {
            let before = &command[..idx];
            let after = &command[idx + ch.len_utf8()..];
            return Some((before, after));
        }
    }
    None
}

fn push_shell_clause(out: &mut Vec<String>, current: &mut String, connector: Option<&str>) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        let mut line = trimmed.to_string();
        if let Some(connector) = connector {
            line.push(' ');
            line.push_str(connector);
        }
        out.push(line);
    } else if let Some(connector) = connector
        && let Some(prev) = out.last_mut()
        && !prev.ends_with(connector)
    {
        prev.push(' ');
        prev.push_str(connector);
    }
    current.clear();
}

/// Resolve a tool-supplied path against the workspace when it's relative.
/// Absolute paths are returned unchanged so `write_file` to `/etc/foo` still
/// shows the right diff. The original string flows through if there's no
/// workspace context — matching the previous behavior for tests / direct
/// constructors.
fn resolve_workspace_path(raw: &str, workspace: Option<&str>) -> std::path::PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match workspace {
        Some(ws) => Path::new(ws).join(path),
        None => path.to_path_buf(),
    }
}

/// Count `+` and `-` lines in a unified diff. Delegates to the shared
/// `summarize_diff` so the popup header reads the same `+N -M` totals
/// the detail pager shows in its summary section — keeps the two views
/// agreeing on what "changed" means even for tricky inputs (no-newline
/// markers, multi-file patches, etc.).
fn count_diff_changes(diff: &str) -> (usize, usize) {
    let summaries = crate::tui::diff_render::summarize_diff(diff);
    if summaries.is_empty() {
        // summarize_diff only collects files that have a `diff --git` or
        // `+++` header. For single-file fragments produced by
        // `make_unified_diff` we fall back to a plain line scan so the
        // header still reflects the change.
        let mut added = 0usize;
        let mut deleted = 0usize;
        for line in diff.lines() {
            if line.starts_with("+++") || line.starts_with("---") {
                continue;
            }
            if line.starts_with('+') {
                added += 1;
            } else if line.starts_with('-') {
                deleted += 1;
            }
        }
        return (added, deleted);
    }
    let added = summaries.iter().map(|s| s.added).sum();
    let deleted = summaries.iter().map(|s| s.deleted).sum();
    (added, deleted)
}

enum PreviewFileRead {
    Content(String),
    Skipped(ApprovalDiffPreview),
    Unreadable,
}

fn read_file_for_diff(resolved: &Path, display_path: &str) -> PreviewFileRead {
    if let Ok(metadata) = std::fs::metadata(resolved)
        && metadata.is_file()
        && metadata.len() > DIFF_PREVIEW_MAX_BYTES
    {
        return PreviewFileRead::Skipped(ApprovalDiffPreview::SkippedLargeFile {
            path: display_path.to_string(),
            size: metadata.len(),
            limit: DIFF_PREVIEW_MAX_BYTES,
        });
    }

    match std::fs::read_to_string(resolved) {
        Ok(content) => PreviewFileRead::Content(content),
        Err(_) => PreviewFileRead::Unreadable,
    }
}

/// Build the diff snapshot for an approval request. Reads the filesystem
/// at most once per request — relative paths resolve against `workspace`
/// so previews work when the agent is rooted elsewhere from the TUI's CWD.
pub fn build_diff_preview(
    tool_name: &str,
    params: &Value,
    workspace: Option<&str>,
) -> Option<ApprovalDiffPreview> {
    match tool_name {
        "edit_file" => {
            let path = params.get("path")?.as_str()?;
            let search = params.get("search")?.as_str()?;
            let replace = params.get("replace")?.as_str()?;
            let resolved = resolve_workspace_path(path, workspace);
            match read_file_for_diff(&resolved, path) {
                PreviewFileRead::Content(file) => {
                    let count = file.matches(search).count();
                    if count == 0 {
                        // search isn't present — render the search/replace pair
                        // as a fallback diff so the user still sees the intent,
                        // and flag it so the UI can warn.
                        let text =
                            crate::tools::diff_format::make_unified_diff(path, search, replace);
                        Some(ApprovalDiffPreview::MissingMatch {
                            path: path.to_string(),
                            text,
                            match_count: 0,
                        })
                    } else {
                        // Simulate the replace and diff the full file so the
                        // user sees the actual change in context.
                        let updated = file.replacen(search, replace, 1);
                        let text =
                            crate::tools::diff_format::make_unified_diff(path, &file, &updated);
                        if text.is_empty() {
                            Some(ApprovalDiffPreview::NoChange {
                                path: path.to_string(),
                            })
                        } else {
                            let (added, deleted) = count_diff_changes(&text);
                            Some(ApprovalDiffPreview::Diff {
                                text,
                                added,
                                deleted,
                            })
                        }
                    }
                }
                PreviewFileRead::Skipped(preview) => Some(preview),
                PreviewFileRead::Unreadable => {
                    // File missing — fall back to inputs-only diff. edit_file
                    // would fail at execution anyway, so MissingMatch is the
                    // honest framing.
                    let text = crate::tools::diff_format::make_unified_diff(path, search, replace);
                    Some(ApprovalDiffPreview::MissingMatch {
                        path: path.to_string(),
                        text,
                        match_count: 0,
                    })
                }
            }
        }
        "write_file" => {
            let path = params.get("path")?.as_str()?;
            let new_content = params.get("content")?.as_str()?;
            let resolved = resolve_workspace_path(path, workspace);
            match read_file_for_diff(&resolved, path) {
                PreviewFileRead::Content(old_content) => {
                    let text = crate::tools::diff_format::make_unified_diff(
                        path,
                        &old_content,
                        new_content,
                    );
                    if text.is_empty() {
                        Some(ApprovalDiffPreview::NoChange {
                            path: path.to_string(),
                        })
                    } else {
                        let (added, deleted) = count_diff_changes(&text);
                        Some(ApprovalDiffPreview::Diff {
                            text,
                            added,
                            deleted,
                        })
                    }
                }
                PreviewFileRead::Skipped(preview) => Some(preview),
                PreviewFileRead::Unreadable => Some(ApprovalDiffPreview::NewFile {
                    path: path.to_string(),
                    content: new_content.to_string(),
                }),
            }
        }
        "apply_patch" => {
            if let Some(patch) = params.get("patch").and_then(|v| v.as_str()) {
                if patch.is_empty() {
                    None
                } else {
                    let (added, deleted) = count_diff_changes(patch);
                    Some(ApprovalDiffPreview::Diff {
                        text: patch.to_string(),
                        added,
                        deleted,
                    })
                }
            } else if let Some(changes) = params.get("changes").and_then(|v| v.as_array()) {
                // `changes` is an array of `{path, content}` full-file
                // replacements. Build a multi-file unified diff against the
                // current contents so the approval shows the same shape as
                // the `patch` path.
                let mut out = String::new();
                for change in changes {
                    let Some(path) = change.get("path").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let Some(new_content) = change.get("content").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let resolved = resolve_workspace_path(path, workspace);
                    let old_content = match read_file_for_diff(&resolved, path) {
                        PreviewFileRead::Content(content) => content,
                        PreviewFileRead::Skipped(ApprovalDiffPreview::SkippedLargeFile {
                            path,
                            size,
                            limit,
                        }) => {
                            if !out.is_empty() {
                                out.push('\n');
                            }
                            out.push_str(&format!("diff --git a/{path} b/{path}\n"));
                            out.push_str(&format!(
                                "  (diff preview skipped - file is {size} bytes, limit is {limit} bytes)\n"
                            ));
                            continue;
                        }
                        PreviewFileRead::Skipped(preview) => return Some(preview),
                        PreviewFileRead::Unreadable => String::new(),
                    };
                    let fragment = crate::tools::diff_format::make_unified_diff(
                        path,
                        &old_content,
                        new_content,
                    );
                    if !fragment.is_empty() {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        // Synthesize a `diff --git` header so the multi-file
                        // summary in `diff_render` picks the file up.
                        out.push_str(&format!("diff --git a/{path} b/{path}\n"));
                        out.push_str(&fragment);
                    }
                }
                if out.is_empty() {
                    None
                } else {
                    let (added, deleted) = count_diff_changes(&out);
                    Some(ApprovalDiffPreview::Diff {
                        text: out,
                        added,
                        deleted,
                    })
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

fn param_preview(params: &Value, keys: &[&str], max_len: usize) -> Option<String> {
    let Value::Object(map) = params else {
        return None;
    };

    for key in keys {
        let Some(value) = map.get(*key) else {
            continue;
        };
        match value {
            Value::String(text) => return Some(truncate_string_value(text, max_len)),
            Value::Number(number) => return Some(number.to_string()),
            Value::Bool(flag) => return Some(flag.to_string()),
            Value::Array(items) if !items.is_empty() => {
                let preview = items
                    .iter()
                    .take(3)
                    .map(|item| match item {
                        Value::String(text) => truncate_string_value(text, max_len / 2),
                        other => truncate_string_value(&other.to_string(), max_len / 2),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                return Some(truncate_string_value(&preview, max_len));
            }
            other => return Some(truncate_string_value(&other.to_string(), max_len)),
        }
    }

    None
}

/// Indices into the option list shared by both variants. Visible to
/// the widget module so it can render the staged-confirmation banner
/// without re-deriving the variant from the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOption {
    ApproveOnce,
    ApproveAlways,
    Deny,
    Abort,
}

impl ApprovalOption {
    const ORDER: [ApprovalOption; 4] = [
        ApprovalOption::ApproveOnce,
        ApprovalOption::ApproveAlways,
        ApprovalOption::Deny,
        ApprovalOption::Abort,
    ];

    fn from_index(idx: usize) -> ApprovalOption {
        Self::ORDER.get(idx).copied().unwrap_or(Self::Abort)
    }

    fn index(self) -> usize {
        Self::ORDER
            .iter()
            .position(|o| *o == self)
            .unwrap_or(Self::ORDER.len() - 1)
    }

    fn decision(self) -> ReviewDecision {
        match self {
            ApprovalOption::ApproveOnce => ReviewDecision::Approved,
            ApprovalOption::ApproveAlways => ReviewDecision::ApprovedForSession,
            ApprovalOption::Deny => ReviewDecision::Denied,
            ApprovalOption::Abort => ReviewDecision::Abort,
        }
    }

    fn requires_confirm(self, risk: RiskLevel) -> bool {
        matches!(risk, RiskLevel::Destructive)
            && matches!(
                self,
                ApprovalOption::ApproveOnce | ApprovalOption::ApproveAlways
            )
    }
}

/// Approval overlay state managed by the modal view stack
#[derive(Debug, Clone)]
pub struct ApprovalView {
    request: ApprovalRequest,
    selected: usize,
    locale: Locale,
    timeout: Option<Duration>,
    requested_at: Instant,
    /// Whether the approval card is collapsed to a single-line banner.
    pub(crate) collapsed: bool,
    diff_scroll: Cell<usize>,
    diff_total_lines: Cell<usize>,
    diff_visible_lines: Cell<usize>,
    diff_panel_cache: RefCell<Option<DiffPanelCache>>,
    /// Tracks the staged option for two-step destructive confirm.
    pending_confirm: Option<ApprovalOption>,
}

impl ApprovalView {
    #[cfg(test)]
    pub fn new(request: ApprovalRequest) -> Self {
        Self::new_for_locale(request, Locale::En)
    }

    pub fn new_for_locale(request: ApprovalRequest, locale: Locale) -> Self {
        Self {
            request,
            selected: 0,
            locale,
            timeout: None,
            requested_at: Instant::now(),
            collapsed: false,
            diff_scroll: Cell::new(0),
            diff_total_lines: Cell::new(0),
            diff_visible_lines: Cell::new(0),
            diff_panel_cache: RefCell::new(None),
            pending_confirm: None,
        }
    }

    fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn select_next(&mut self) {
        self.selected = (self.selected + 1).min(ApprovalOption::ORDER.len() - 1);
    }

    fn current_option(&self) -> ApprovalOption {
        ApprovalOption::from_index(self.selected)
    }

    /// Test-only accessor for the selected option's decision.
    #[cfg(test)]
    fn current_decision(&self) -> ReviewDecision {
        self.current_option().decision()
    }

    /// Selected option for the renderer (used by the widget tests too).
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Risk level for the renderer's accent picking.
    #[cfg(test)]
    pub fn risk(&self) -> RiskLevel {
        self.request.risk
    }

    pub(crate) fn locale(&self) -> Locale {
        self.locale
    }

    pub(crate) fn pending_confirm(&self) -> Option<ApprovalOption> {
        self.pending_confirm
    }

    pub(crate) fn set_diff_metrics(&self, total: usize, visible: usize) -> usize {
        self.diff_total_lines.set(total);
        self.diff_visible_lines.set(visible);
        let max_scroll = total.saturating_sub(visible);
        let scroll = self.diff_scroll.get().min(max_scroll);
        self.diff_scroll.set(scroll);
        scroll
    }

    pub(crate) fn cached_diff_panel(
        &self,
        width: u16,
        locale: Locale,
    ) -> Option<RenderedDiffPanel> {
        let cache = self.diff_panel_cache.borrow();
        let cache = cache.as_ref()?;
        if cache.width == width && cache.locale == locale {
            Some(cache.panel.clone())
        } else {
            None
        }
    }

    pub(crate) fn set_cached_diff_panel(
        &self,
        width: u16,
        locale: Locale,
        panel: RenderedDiffPanel,
    ) {
        *self.diff_panel_cache.borrow_mut() = Some(DiffPanelCache {
            width,
            locale,
            panel,
        });
    }

    fn scroll_diff_up(&mut self, amount: usize) {
        self.diff_scroll
            .set(self.diff_scroll.get().saturating_sub(amount));
    }

    fn scroll_diff_down(&mut self, amount: usize) {
        let visible = self.diff_visible_lines.get();
        let max_scroll = self.diff_total_lines.get().saturating_sub(visible);
        self.diff_scroll
            .set((self.diff_scroll.get() + amount).min(max_scroll));
    }

    fn diff_page_height(&self) -> usize {
        self.diff_visible_lines.get().max(1)
    }

    fn diff_half_page_height(&self) -> usize {
        self.diff_page_height().div_ceil(2).max(1)
    }

    /// Try to commit (or stage) the given option respecting the
    /// variant's confirmation policy. Returns the action the modal
    /// stack should apply.
    fn commit_or_stage(&mut self, option: ApprovalOption) -> ViewAction {
        if option.requires_confirm(self.request.risk) {
            // Two-step destructive flow: first press stages, second
            // press of the same option commits.
            if self.pending_confirm == Some(option) {
                self.pending_confirm = None;
                return self.emit_decision(option.decision(), false);
            }
            self.pending_confirm = Some(option);
            self.selected = option.index();
            return ViewAction::None;
        }
        // Benign variant or non-approve options commit immediately.
        self.pending_confirm = None;
        self.emit_decision(option.decision(), false)
    }

    fn emit_decision(&self, decision: ReviewDecision, timed_out: bool) -> ViewAction {
        ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
            tool_id: self.request.id.clone(),
            tool_name: self.request.tool_name.clone(),
            decision,
            timed_out,
            approval_key: self.request.approval_key.clone(),
            approval_grouping_key: self.request.approval_grouping_key.clone(),
        })
    }

    fn emit_params_pager(&self) -> ViewAction {
        if let Some(lines) = self.build_detail_lines() {
            ViewAction::Emit(ViewEvent::OpenStyledPager {
                title: format!("Details: {}", self.request.tool_name),
                lines,
            })
        } else {
            let content = serde_json::to_string_pretty(&self.request.params)
                .unwrap_or_else(|_| self.request.params.to_string());
            ViewAction::Emit(ViewEvent::OpenTextPager {
                title: format!("Tool Params: {}", self.request.tool_name),
                content,
            })
        }
    }

    /// Build a styled details view for file tools.
    fn build_detail_lines(&self) -> Option<Vec<Line<'static>>> {
        let label_style = Style::default()
            .fg(palette::DEEPSEEK_SKY)
            .add_modifier(Modifier::BOLD);
        let muted_style = Style::default().fg(palette::TEXT_MUTED);
        let mut lines: Vec<Line<'static>> = Vec::new();

        match self.request.tool_name.as_str() {
            "edit_file" => {
                let path = self.request.params.get("path")?.as_str()?;
                lines.push(Line::from(vec![
                    Span::styled("File: ", muted_style),
                    Span::raw(path.to_string()),
                ]));
                push_approval_changes(&mut lines, self.request.diff_preview(), 120);
                if let Some(search) = self.request.params.get("search").and_then(|v| v.as_str()) {
                    push_multiline_field(&mut lines, "Search", search, label_style);
                }
                if let Some(replace) = self.request.params.get("replace").and_then(|v| v.as_str()) {
                    push_multiline_field(&mut lines, "Replace", replace, label_style);
                }
                if let Some(fuzz) = self.request.params.get("fuzz") {
                    push_scalar_field(&mut lines, "Fuzz", &fuzz.to_string(), label_style);
                }
                Some(lines)
            }
            "write_file" => {
                let path = self.request.params.get("path")?.as_str()?;
                lines.push(Line::from(vec![
                    Span::styled("File: ", muted_style),
                    Span::raw(path.to_string()),
                ]));
                push_approval_changes(&mut lines, self.request.diff_preview(), 120);
                if let Some(content) = self.request.params.get("content").and_then(|v| v.as_str()) {
                    push_multiline_field(&mut lines, "Content", content, label_style);
                }
                if let Some(mode) = self.request.params.get("mode").and_then(|v| v.as_str()) {
                    push_scalar_field(&mut lines, "Mode", mode, label_style);
                }
                Some(lines)
            }
            "apply_patch" => {
                if let Some(path) = self.request.params.get("path").and_then(|v| v.as_str()) {
                    lines.push(Line::from(vec![
                        Span::styled("File: ", muted_style),
                        Span::raw(path.to_string()),
                    ]));
                }
                push_approval_changes(&mut lines, self.request.diff_preview(), 120);
                if let Some(patch) = self.request.params.get("patch").and_then(|v| v.as_str()) {
                    push_multiline_field(&mut lines, "Patch", patch, label_style);
                }
                if let Some(changes) = self
                    .request
                    .params
                    .get("changes")
                    .and_then(|v| v.as_array())
                {
                    lines.push(Line::from(Span::styled("Changes:", label_style)));
                    for (idx, change) in changes.iter().enumerate() {
                        let Some(change_path) = change.get("path").and_then(|v| v.as_str()) else {
                            continue;
                        };
                        lines.push(Line::from(vec![
                            Span::raw("  "),
                            Span::styled(
                                format!("{}.", idx + 1),
                                Style::default().fg(palette::TEXT_MUTED),
                            ),
                            Span::raw(" "),
                            Span::styled(change_path.to_string(), muted_style),
                        ]));
                        if let Some(content) = change.get("content").and_then(|v| v.as_str()) {
                            push_multiline_body(&mut lines, content, 4);
                        }
                    }
                }
                Some(lines)
            }
            _ => None,
        }
    }

    fn is_timed_out(&self) -> bool {
        match self.timeout {
            Some(timeout) => self.requested_at.elapsed() >= timeout,
            None => false,
        }
    }
}

impl ModalView for ApprovalView {
    fn kind(&self) -> ModalKind {
        ModalKind::Approval
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl {
            match key.code {
                KeyCode::Char('u') | KeyCode::Char('U') => {
                    self.scroll_diff_up(self.diff_half_page_height());
                    return ViewAction::None;
                }
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    self.scroll_diff_down(self.diff_half_page_height());
                    return ViewAction::None;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Tab => {
                self.collapsed = !self.collapsed;
                ViewAction::None
            }
            KeyCode::PageUp => {
                self.scroll_diff_up(self.diff_page_height());
                ViewAction::None
            }
            KeyCode::PageDown => {
                self.scroll_diff_down(self.diff_page_height());
                ViewAction::None
            }
            KeyCode::Up => {
                self.scroll_diff_up(1);
                ViewAction::None
            }
            KeyCode::Down => {
                self.scroll_diff_down(1);
                ViewAction::None
            }
            KeyCode::Char('k') => {
                self.select_prev();
                ViewAction::None
            }
            KeyCode::Char('j') => {
                self.select_next();
                ViewAction::None
            }
            KeyCode::Enter => self.commit_or_stage(self.current_option()),
            // Direct shortcuts; '1' / '2' map to the first two options
            // so a numeric pad still works for approve flows.
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('1') => {
                self.commit_or_stage(ApprovalOption::ApproveOnce)
            }
            KeyCode::Char('a') | KeyCode::Char('A') | KeyCode::Char('2') => {
                self.commit_or_stage(ApprovalOption::ApproveAlways)
            }
            KeyCode::Char('n')
            | KeyCode::Char('N')
            | KeyCode::Char('d')
            | KeyCode::Char('D')
            | KeyCode::Char('3') => self.commit_or_stage(ApprovalOption::Deny),
            KeyCode::Char('v') | KeyCode::Char('V') => self.emit_params_pager(),
            KeyCode::Esc => self.emit_decision(ReviewDecision::Abort, false),
            _ => ViewAction::None,
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_diff_up(3);
                ViewAction::None
            }
            MouseEventKind::ScrollDown => {
                self.scroll_diff_down(3);
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        let approval_widget = ApprovalWidget::new(&self.request, self);
        approval_widget.render(area, buf);
    }

    fn tick(&mut self) -> ViewAction {
        if self.is_timed_out() {
            return self.emit_decision(ReviewDecision::Denied, true);
        }
        ViewAction::None
    }
}

fn push_approval_changes(
    lines: &mut Vec<Line<'static>>,
    preview: Option<&ApprovalDiffPreview>,
    width: u16,
) {
    let label_style = Style::default()
        .fg(palette::DEEPSEEK_SKY)
        .add_modifier(Modifier::BOLD);
    let muted_style = Style::default().fg(palette::TEXT_MUTED);

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Changes:", label_style)));
    match preview {
        Some(ApprovalDiffPreview::Diff { text, .. })
        | Some(ApprovalDiffPreview::MissingMatch { text, .. }) => {
            if text.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  (no textual changes - content matches current file)".to_string(),
                    muted_style,
                )));
            } else {
                lines.extend(crate::tui::diff_render::render_diff(text, width));
            }
        }
        Some(ApprovalDiffPreview::NewFile { path, content }) => {
            let diff = crate::tools::diff_format::make_unified_diff(path, "", content);
            lines.extend(crate::tui::diff_render::render_diff(&diff, width));
        }
        Some(ApprovalDiffPreview::SkippedLargeFile { size, limit, .. }) => {
            lines.push(Line::from(Span::styled(
                format!("  (diff preview skipped - file is {size} bytes, limit is {limit} bytes)"),
                muted_style,
            )));
        }
        Some(ApprovalDiffPreview::NoChange { .. }) | None => {
            lines.push(Line::from(Span::styled(
                "  (no textual changes - content matches current file)".to_string(),
                muted_style,
            )));
        }
    }
    lines.push(Line::from(""));
}

fn push_scalar_field(lines: &mut Vec<Line<'static>>, label: &str, value: &str, label_style: Style) {
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{label}: "), label_style),
        Span::raw(value.to_string()),
    ]));
}

fn push_multiline_field(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    label_style: Style,
) {
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{label}:"), label_style),
    ]));
    push_multiline_body(lines, value, 4);
}

fn push_multiline_body(lines: &mut Vec<Line<'static>>, value: &str, indent: usize) {
    let indent = " ".repeat(indent);
    for raw in value.split('\n') {
        lines.push(Line::from(Span::raw(format!("{indent}{raw}"))));
    }
}

fn truncate_string_value(value: &str, max_len: usize) -> String {
    if value.chars().count() <= max_len {
        return value.to_string();
    }
    let truncated: String = value.chars().take(max_len).collect();
    format!("{truncated}...")
}

// ============================================================================
// Sandbox Elevation Flow
// ============================================================================

/// Options for elevating sandbox permissions after a denial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElevationOption {
    /// Add network access to the sandbox policy.
    WithNetwork,
    /// Add write access to specific paths.
    WithWriteAccess(Vec<PathBuf>),
    /// Remove sandbox restrictions entirely (dangerous).
    FullAccess,
    /// Abort the tool execution.
    Abort,
}

impl ElevationOption {
    /// Get the display label for this option.
    pub fn label(&self) -> &'static str {
        match self {
            ElevationOption::WithNetwork => "Allow outbound network",
            ElevationOption::WithWriteAccess(_) => "Allow extra write access",
            ElevationOption::FullAccess => "Full access (filesystem + network)",
            ElevationOption::Abort => "Abort",
        }
    }

    /// Get a short description.
    pub fn description(&self) -> &'static str {
        match self {
            ElevationOption::WithNetwork => {
                "Retry this tool call with outbound network access for downloads and HTTP requests"
            }
            ElevationOption::WithWriteAccess(_) => {
                "Retry this tool call with additional writable filesystem scope"
            }
            ElevationOption::FullAccess => {
                "Retry without sandbox limits; grants unrestricted filesystem and network access"
            }
            ElevationOption::Abort => "Cancel this tool execution",
        }
    }

    /// Convert to a sandbox policy.
    pub fn to_policy(&self, base_cwd: &Path) -> SandboxPolicy {
        match self {
            ElevationOption::WithNetwork => SandboxPolicy::workspace_with_network(),
            ElevationOption::WithWriteAccess(paths) => {
                let mut roots = paths.clone();
                roots.push(base_cwd.to_path_buf());
                SandboxPolicy::workspace_with_roots(roots, false)
            }
            ElevationOption::FullAccess => SandboxPolicy::DangerFullAccess,
            ElevationOption::Abort => SandboxPolicy::default(), // Won't be used
        }
    }
}

/// Request for user decision after a sandbox denial.
#[derive(Debug, Clone)]
pub struct ElevationRequest {
    /// The tool ID that was blocked.
    pub tool_id: String,
    /// The tool name.
    pub tool_name: String,
    /// The command that was blocked (if shell).
    pub command: Option<String>,
    /// The reason for denial (from sandbox).
    pub denial_reason: String,
    /// Available elevation options.
    pub options: Vec<ElevationOption>,
}

impl ElevationRequest {
    /// Create a new elevation request for a shell command.
    pub fn for_shell(
        tool_id: &str,
        command: &str,
        denial_reason: &str,
        blocked_network: bool,
        blocked_write: bool,
    ) -> Self {
        let mut options = Vec::new();

        if blocked_network {
            options.push(ElevationOption::WithNetwork);
        }
        if blocked_write {
            options.push(ElevationOption::WithWriteAccess(vec![]));
        }
        options.push(ElevationOption::FullAccess);
        options.push(ElevationOption::Abort);

        Self {
            tool_id: tool_id.to_string(),
            tool_name: "exec_shell".to_string(),
            command: Some(command.to_string()),
            denial_reason: denial_reason.to_string(),
            options,
        }
    }

    /// Create a generic elevation request.
    #[allow(dead_code)]
    pub fn generic(tool_id: &str, tool_name: &str, denial_reason: &str) -> Self {
        Self {
            tool_id: tool_id.to_string(),
            tool_name: tool_name.to_string(),
            command: None,
            denial_reason: denial_reason.to_string(),
            options: vec![
                ElevationOption::WithNetwork,
                ElevationOption::FullAccess,
                ElevationOption::Abort,
            ],
        }
    }
}

/// Elevation overlay state managed by the modal view stack.
#[derive(Debug, Clone)]
pub struct ElevationView {
    request: ElevationRequest,
    selected: usize,
}

impl ElevationView {
    pub fn new(request: ElevationRequest) -> Self {
        Self {
            request,
            selected: 0,
        }
    }

    fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn select_next(&mut self) {
        let max = self.request.options.len().saturating_sub(1);
        self.selected = (self.selected + 1).min(max);
    }

    fn current_option(&self) -> &ElevationOption {
        &self.request.options[self.selected]
    }

    fn emit_decision(&self, option: ElevationOption) -> ViewAction {
        ViewAction::EmitAndClose(ViewEvent::ElevationDecision {
            tool_id: self.request.tool_id.clone(),
            tool_name: self.request.tool_name.clone(),
            option,
        })
    }

    /// Get the request for rendering.
    #[allow(dead_code)]
    pub fn request(&self) -> &ElevationRequest {
        &self.request
    }

    /// Get the currently selected index.
    #[allow(dead_code)]
    pub fn selected(&self) -> usize {
        self.selected
    }
}

impl ModalView for ElevationView {
    fn kind(&self) -> ModalKind {
        ModalKind::Elevation
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.select_prev();
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.select_next();
                ViewAction::None
            }
            KeyCode::Enter => self.emit_decision(self.current_option().clone()),
            KeyCode::Char('n') => self.emit_decision(ElevationOption::WithNetwork),
            KeyCode::Char('w') => {
                // Find the write access option if available
                for opt in &self.request.options {
                    if matches!(opt, ElevationOption::WithWriteAccess(_)) {
                        return self.emit_decision(opt.clone());
                    }
                }
                ViewAction::None
            }
            KeyCode::Char('f') => self.emit_decision(ElevationOption::FullAccess),
            KeyCode::Esc | KeyCode::Char('a') => self.emit_decision(ElevationOption::Abort),
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        let elevation_widget = ElevationWidget::new(&self.request, self.selected);
        elevation_widget.render(area, buf);
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
    use serde_json::json;

    fn create_key_event(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    fn benign_request() -> ApprovalRequest {
        ApprovalRequest::new(
            "test-id",
            "read_file",
            "Read a file from disk",
            &json!({"path": "src/main.rs"}),
            "tool:read_file",
        )
    }

    fn destructive_request() -> ApprovalRequest {
        ApprovalRequest::new(
            "test-id",
            "write_file",
            "Write a file to disk",
            &json!({"path": "src/main.rs", "content": "test"}),
            "tool:write_file",
        )
    }

    // ========================================================================
    // Tool Category Tests
    // ========================================================================

    #[test]
    fn test_get_tool_category_safe_tools() {
        assert_eq!(get_tool_category("read_file"), ToolCategory::Safe);
        assert_eq!(get_tool_category("list_dir"), ToolCategory::Safe);
        assert_eq!(get_tool_category("todo_write"), ToolCategory::Safe);
        assert_eq!(get_tool_category("todo_read"), ToolCategory::Safe);
        assert_eq!(get_tool_category("note"), ToolCategory::Safe);
        assert_eq!(get_tool_category("update_plan"), ToolCategory::Safe);
    }

    #[test]
    fn test_get_tool_category_file_write_tools() {
        assert_eq!(get_tool_category("write_file"), ToolCategory::FileWrite);
        assert_eq!(get_tool_category("edit_file"), ToolCategory::FileWrite);
        assert_eq!(get_tool_category("apply_patch"), ToolCategory::FileWrite);
    }

    #[test]
    fn test_get_tool_category_shell_tools() {
        assert_eq!(get_tool_category("exec_shell"), ToolCategory::Shell);
        assert_eq!(get_tool_category("task_shell_start"), ToolCategory::Shell);
        assert_eq!(get_tool_category("exec_shell_wait"), ToolCategory::Shell);
        assert_eq!(
            get_tool_category("exec_shell_interact"),
            ToolCategory::Shell
        );
        assert_eq!(
            get_tool_category("mcp_linear_save_issue"),
            ToolCategory::McpAction
        );
        assert_eq!(get_tool_category("list_mcp_tools"), ToolCategory::McpRead);
    }

    #[test]
    fn test_get_tool_category_unknown_tools_need_review() {
        assert_eq!(get_tool_category("unknown_tool"), ToolCategory::Unknown);
    }

    // ========================================================================
    // Risk Routing Tests (#129)
    // ========================================================================

    #[test]
    fn risk_safe_categories_route_benign() {
        let cat = ToolCategory::Safe;
        assert_eq!(
            classify_risk("read_file", cat, &json!({"path": "x"})),
            RiskLevel::Benign
        );
        let cat = ToolCategory::McpRead;
        assert_eq!(
            classify_risk("list_mcp_tools", cat, &json!({})),
            RiskLevel::Benign
        );
    }

    #[test]
    fn risk_query_only_network_is_benign_but_fetch_is_destructive() {
        // web_search is read-only enough to use the benign variant.
        let cat = ToolCategory::Network;
        assert_eq!(
            classify_risk("web_search", cat, &json!({"q": "rust"})),
            RiskLevel::Benign
        );
        // fetch_url pulls arbitrary remote content, so it stays destructive.
        assert_eq!(
            classify_risk("fetch_url", cat, &json!({"url": "https://example.com"})),
            RiskLevel::Destructive
        );
    }

    #[test]
    fn risk_writes_shell_mcp_action_unknown_route_destructive() {
        for (name, cat) in [
            ("write_file", ToolCategory::FileWrite),
            ("edit_file", ToolCategory::FileWrite),
            ("apply_patch", ToolCategory::FileWrite),
            ("exec_shell", ToolCategory::Shell),
            ("mcp_linear_save_issue", ToolCategory::McpAction),
            ("totally_new_tool", ToolCategory::Unknown),
        ] {
            assert_eq!(
                classify_risk(name, cat, &json!({})),
                RiskLevel::Destructive,
                "expected {name:?} to be Destructive",
            );
        }
    }

    #[test]
    fn risk_dangerous_shell_command_stays_destructive() {
        // command_safety would flag this as Dangerous; classify_risk
        // already routes Shell to Destructive. The check exists so a
        // future attempt to relax shell to Benign cannot smuggle this
        // through unexamined.
        let cat = ToolCategory::Shell;
        assert_eq!(
            classify_risk("exec_shell", cat, &json!({"command": "rm -rf /"})),
            RiskLevel::Destructive
        );
    }

    // ========================================================================
    // ApprovalRequest Tests
    // ========================================================================

    #[test]
    fn test_approval_request_new() {
        let params = json!({"path": "src/main.rs", "content": "test"});
        let request = ApprovalRequest::new(
            "test-id",
            "write_file",
            "Write a file to disk",
            &params,
            "test_key",
        );

        assert_eq!(request.id, "test-id");
        assert_eq!(request.tool_name, "write_file");
        assert_eq!(request.category, ToolCategory::FileWrite);
        assert_eq!(request.risk, RiskLevel::Destructive);
        assert_eq!(request.params, params);
    }

    #[test]
    fn test_prominent_details_shell() {
        let params = json!({"command": "npm run build", "cwd": "/home/user"});
        let request = ApprovalRequest::new(
            "test-id",
            "exec_shell",
            "Run a shell command",
            &params,
            "test_key",
        );
        let details = request.prominent_detail_items(Locale::En);
        assert_eq!(details[0].label, "Command");
        assert_eq!(details[0].value, "npm run build");
        assert_eq!(
            details[0].shell_lines.as_deref(),
            Some(&["npm run build".to_string()][..])
        );
        assert_eq!(details[1].label, "Dir");
        assert_eq!(details[1].value, "/home/user");
    }

    #[test]
    fn test_prominent_details_shell_does_not_truncate_long_command() {
        // Regression: shell commands used to be hard-clipped at 120 chars
        // with a trailing `…`, hiding the dangerous tail of long pipelines
        // (the part where `rm -rf` or `>` redirects usually live). The
        // popup body wraps long lines via `Paragraph::wrap`, so we now
        // pass the command through verbatim.
        let cmd = format!("printf '{}\n' > /tmp/x && cat /tmp/x", "x".repeat(300));
        let params = json!({"command": cmd, "cwd": "/home/user"});
        let request =
            ApprovalRequest::new("test-id", "exec_shell", "Run shell", &params, "test_key");
        let details = request.prominent_detail_items(Locale::En);
        assert_eq!(details[0].label, "Command");
        assert_eq!(
            details[0].value, cmd,
            "shell command must be returned verbatim, no `…` truncation",
        );
        let shell_lines = details[0]
            .shell_lines
            .as_ref()
            .expect("shell command lines should be cached");
        assert!(shell_lines.iter().any(|line| line.contains("cat /tmp/x")));
    }

    #[test]
    fn test_shell_formatter_preserves_logical_or_operator() {
        let lines = format_shell_command_for_approval("cargo build || echo fallback");

        assert_eq!(lines, vec!["cargo build ||", "echo fallback"]);
    }

    #[test]
    fn test_prominent_details_shell_marks_current_dir() {
        let params = json!({"command": "ls", "cwd": "/home/user/project"});
        let request = ApprovalRequest::new_with_workspace(
            "test-id",
            "exec_shell",
            "Run a shell command",
            &params,
            "test_key",
            Some("/home/user/project".to_string()),
        );
        let details = request.prominent_detail_items(Locale::En);
        assert_eq!(details[1].label, "Dir");
        assert_eq!(details[1].value, "(current)");
    }

    #[test]
    fn test_prominent_details_file_write() {
        let params = json!({"path": "src/main.rs", "content": "fn main() {}"});
        let request =
            ApprovalRequest::new("test-id", "write_file", "Write file", &params, "test_key");
        let details = request.prominent_detail_items(Locale::En);
        assert_eq!(details[0].label, "File");
        assert_eq!(details[0].value, "src/main.rs");
    }

    #[test]
    fn test_diff_preview_edit_file() {
        let params = json!({
            "path": "src/main.rs",
            "search": "fn main() {\n    println!(\"hello\");\n}",
            "replace": "fn main() {\n    println!(\"world\");\n}"
        });
        let request =
            ApprovalRequest::new("test-id", "edit_file", "Edit file", &params, "test_key");
        let preview = request
            .diff_preview()
            .expect("edit_file produces a preview");
        // Tests don't see src/main.rs, so we land in the MissingMatch fallback
        // which still surfaces a search→replace diff for visual confirmation.
        let diff = preview.diff_text();
        assert!(diff.contains("--- a/src/main.rs"));
        assert!(diff.contains("+++ b/src/main.rs"));
        assert!(diff.contains("-    println!(\"hello\");"));
        assert!(diff.contains("+    println!(\"world\");"));
        assert!(
            matches!(preview, ApprovalDiffPreview::MissingMatch { .. }),
            "expected MissingMatch when file is absent, got {preview:?}"
        );
    }

    #[test]
    fn test_diff_preview_edit_file_existing_simulates_replace() {
        // When the file exists and search matches, we should produce a full
        // simulated diff against the real file content.
        let path = std::env::temp_dir().join("deepseek_test_edit_file_existing.txt");
        std::fs::write(&path, "alpha\nbeta\ngamma\n").unwrap();
        let params = json!({
            "path": path.display().to_string(),
            "search": "beta",
            "replace": "BETA",
        });
        let request =
            ApprovalRequest::new("test-id", "edit_file", "Edit file", &params, "test_key");
        let preview = request.diff_preview().expect("edit_file preview");
        match preview {
            ApprovalDiffPreview::Diff { text, .. } => {
                assert!(text.contains("-beta"), "got {text}");
                assert!(text.contains("+BETA"), "got {text}");
            }
            other => panic!("expected Diff for existing edit_file, got {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_build_detail_lines_edit_file_structures_multiline_params() {
        let params = json!({
            "path": "test_db.js",
            "search": "async transaction(callback) {\n    const conn = await this.pool.getConnection();\n}",
            "replace": "async transaction(callback, timeout = 30000) {\n    const conn = await this.pool.getConnection();\n}",
        });
        let request =
            ApprovalRequest::new("test-id", "edit_file", "Edit file", &params, "test_key");
        let view = ApprovalView::new(request);
        let lines = view.build_detail_lines().expect("file tools have details");
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("File: test_db.js"), "{rendered}");
        assert!(rendered.contains("Changes:"), "{rendered}");
        assert!(rendered.contains("Search:"), "{rendered}");
        assert!(rendered.contains("Replace:"), "{rendered}");
        assert!(!rendered.contains("\"search\""), "{rendered}");
        assert!(!rendered.contains("\"replace\""), "{rendered}");
    }

    #[test]
    fn test_diff_preview_write_file_existing() {
        let path = std::env::temp_dir().join("deepseek_test_diff_preview.txt");
        std::fs::write(&path, "old content\n").unwrap();
        let params = json!({"path": path.display().to_string(), "content": "new content\n"});
        let request =
            ApprovalRequest::new("test-id", "write_file", "Write file", &params, "test_key");
        let preview = request
            .diff_preview()
            .expect("write_file on existing file should produce a preview");
        let diff = preview.diff_text();
        assert!(diff.contains("-old content"), "{diff}");
        assert!(diff.contains("+new content"), "{diff}");
        assert!(
            matches!(preview, ApprovalDiffPreview::Diff { .. }),
            "expected Diff variant, got {preview:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_diff_preview_write_file_large_existing_skips_inline_diff() {
        let path = std::env::temp_dir().join("deepseek_test_diff_large_file.txt");
        std::fs::write(&path, "x".repeat((DIFF_PREVIEW_MAX_BYTES + 1) as usize)).unwrap();
        let params = json!({"path": path.display().to_string(), "content": "new content
"});
        let request =
            ApprovalRequest::new("test-id", "write_file", "Write file", &params, "test_key");
        let preview = request
            .diff_preview()
            .expect("large existing file should still produce a preview state");
        match preview {
            ApprovalDiffPreview::SkippedLargeFile { size, limit, .. } => {
                assert!(*size > *limit);
                assert_eq!(*limit, DIFF_PREVIEW_MAX_BYTES);
            }
            other => panic!("expected SkippedLargeFile, got {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_diff_preview_write_file_unchanged_shows_no_change() {
        // write_file with content identical to the file's current bytes used
        // to drop the whole preview panel. We now surface a NoChange hint so
        // the user knows the call is a no-op.
        let path = std::env::temp_dir().join("deepseek_test_diff_no_change.txt");
        std::fs::write(&path, "same\n").unwrap();
        let params = json!({"path": path.display().to_string(), "content": "same\n"});
        let request =
            ApprovalRequest::new("test-id", "write_file", "Write file", &params, "test_key");
        let preview = request.diff_preview().expect("NoChange is still a preview");
        assert!(
            matches!(preview, ApprovalDiffPreview::NoChange { .. }),
            "expected NoChange, got {preview:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_diff_preview_write_file_new() {
        let path = std::env::temp_dir().join("deepseek_test_diff_new_file.txt");
        let _ = std::fs::remove_file(&path);
        let params = json!({"path": path.display().to_string(), "content": "brand new\n"});
        let request =
            ApprovalRequest::new("test-id", "write_file", "Write file", &params, "test_key");
        let preview = request
            .diff_preview()
            .expect("write_file on new file should produce a preview");
        match preview {
            ApprovalDiffPreview::NewFile { content, .. } => {
                assert!(content.contains("brand new"), "{content}");
            }
            other => panic!("expected NewFile variant, got {other:?}"),
        }
    }

    #[test]
    fn test_diff_preview_write_file_resolves_workspace_relative_path() {
        // Regression for the bug where write_file with a workspace-relative
        // path produced an empty preview because std::fs::read_to_string was
        // called with the raw relative path.
        let workspace = std::env::temp_dir().join("deepseek_test_ws_relative");
        std::fs::create_dir_all(&workspace).unwrap();
        let file_rel = "nested/file.txt";
        let abs = workspace.join(file_rel);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, "before\n").unwrap();

        let params = json!({"path": file_rel, "content": "after\n"});
        let mut request = ApprovalRequest::new_with_workspace(
            "test-id",
            "write_file",
            "Write file",
            &params,
            "test_key",
            Some(workspace.display().to_string()),
        );
        request.build_and_set_diff_preview();
        let preview = request.diff_preview().expect("preview built");
        match preview {
            ApprovalDiffPreview::Diff { text, .. } => {
                assert!(text.contains("-before"), "{text}");
                assert!(text.contains("+after"), "{text}");
            }
            other => panic!("expected Diff for resolved path, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn test_diff_preview_apply_patch() {
        let patch = "--- a/f.rs\n+++ b/f.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let params = json!({"path": "f.rs", "patch": patch});
        let request =
            ApprovalRequest::new("test-id", "apply_patch", "Apply patch", &params, "test_key");
        let preview = request.diff_preview().expect("apply_patch preview");
        match preview {
            ApprovalDiffPreview::Diff { text, .. } => assert_eq!(text, patch),
            other => panic!("expected Diff variant for apply_patch, got {other:?}"),
        }
    }

    #[test]
    fn test_diff_preview_apply_patch_changes_array() {
        // apply_patch accepts a `changes` array as a full-file replacement
        // alternative to `patch`. The preview must surface those changes
        // instead of leaving the popup blank.
        let workspace = std::env::temp_dir().join("deepseek_test_apply_patch_changes");
        std::fs::create_dir_all(&workspace).unwrap();
        let a = workspace.join("a.txt");
        std::fs::write(&a, "old\n").unwrap();

        let params = json!({
            "changes": [
                {"path": "a.txt", "content": "new\n"},
                {"path": "b.txt", "content": "added\n"},
            ]
        });
        let mut request = ApprovalRequest::new_with_workspace(
            "test-id",
            "apply_patch",
            "Apply patch",
            &params,
            "test_key",
            Some(workspace.display().to_string()),
        );
        request.build_and_set_diff_preview();
        let preview = request
            .diff_preview()
            .expect("changes array should produce a preview");
        match preview {
            ApprovalDiffPreview::Diff { text, .. } => {
                assert!(text.contains("diff --git a/a.txt b/a.txt"), "{text}");
                assert!(text.contains("-old"), "{text}");
                assert!(text.contains("+new"), "{text}");
                assert!(text.contains("diff --git a/b.txt b/b.txt"), "{text}");
                assert!(text.contains("+added"), "{text}");
            }
            other => panic!("expected Diff for changes, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn test_diff_preview_apply_patch_changes_array_keeps_diffs_when_one_file_skips() {
        let workspace = std::env::temp_dir().join("deepseek_test_apply_patch_changes_large");
        std::fs::create_dir_all(&workspace).unwrap();
        let a = workspace.join("a.txt");
        let large = workspace.join("large.txt");
        std::fs::write(&a, "old\n").unwrap();
        std::fs::write(&large, "x".repeat((DIFF_PREVIEW_MAX_BYTES + 1) as usize)).unwrap();

        let params = json!({
            "changes": [
                {"path": "a.txt", "content": "new\n"},
                {"path": "large.txt", "content": "replacement\n"},
                {"path": "b.txt", "content": "added\n"},
            ]
        });
        let mut request = ApprovalRequest::new_with_workspace(
            "test-id",
            "apply_patch",
            "Apply patch",
            &params,
            "test_key",
            Some(workspace.display().to_string()),
        );
        request.build_and_set_diff_preview();
        let preview = request
            .diff_preview()
            .expect("changes array should keep a partial preview");
        match preview {
            ApprovalDiffPreview::Diff { text, .. } => {
                assert!(text.contains("diff --git a/a.txt b/a.txt"), "{text}");
                assert!(text.contains("-old"), "{text}");
                assert!(text.contains("+new"), "{text}");
                assert!(
                    text.contains("diff --git a/large.txt b/large.txt"),
                    "{text}"
                );
                assert!(text.contains("diff preview skipped"), "{text}");
                assert!(text.contains("diff --git a/b.txt b/b.txt"), "{text}");
                assert!(text.contains("+added"), "{text}");
            }
            other => panic!("expected Diff for partial changes preview, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn test_build_detail_lines_apply_patch_changes_without_top_level_path() {
        let params = json!({
            "changes": [
                {"path": "a.txt", "content": "new\n"},
            ]
        });
        let request =
            ApprovalRequest::new("test-id", "apply_patch", "Apply patch", &params, "test_key");
        let view = ApprovalView::new(request);
        let lines = view.build_detail_lines().expect("apply_patch has details");
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Changes:"), "{rendered}");
        assert!(rendered.contains("a.txt"), "{rendered}");
        assert!(!rendered.contains("\"changes\""), "{rendered}");
    }

    #[test]
    fn test_diff_preview_none_for_other_tools() {
        let params = json!({"command": "ls"});
        let request =
            ApprovalRequest::new("test-id", "exec_shell", "Run shell", &params, "test_key");
        assert!(request.diff_preview().is_none());
    }

    // ========================================================================
    // ApprovalView Tests — Benign Variant (single-key approve)
    // ========================================================================

    #[test]
    fn test_approval_view_initial_state() {
        let view = ApprovalView::new(benign_request());
        assert_eq!(view.selected, 0);
        assert!(view.timeout.is_none());
        assert_eq!(view.risk(), RiskLevel::Benign);
    }

    #[test]
    fn tab_toggles_collapsed_card_so_transcript_stays_visible() {
        // Regression for PR #1455 / @tiger-dog: the approval modal
        // rendered as a full-screen takeover that hid the transcript
        // behind it, so users had to dismiss the prompt to remember
        // what they were approving. Tab now flips between the full
        // takeover card and a single-line bottom banner.
        let mut view = ApprovalView::new(benign_request());
        assert!(
            !view.collapsed,
            "modal must start expanded so first-time users notice it"
        );

        let action = view.handle_key(create_key_event(KeyCode::Tab));
        assert!(matches!(action, ViewAction::None));
        assert!(view.collapsed, "first Tab collapses the card");

        let action = view.handle_key(create_key_event(KeyCode::Tab));
        assert!(matches!(action, ViewAction::None));
        assert!(!view.collapsed, "second Tab restores the takeover card");
    }

    #[test]
    fn test_approval_view_navigation() {
        let mut view = ApprovalView::new(benign_request());
        assert_eq!(view.selected, 0);

        view.select_next();
        assert_eq!(view.selected, 1);
        view.select_next();
        assert_eq!(view.selected, 2);
        view.select_next();
        assert_eq!(view.selected, 3);

        // Should clamp at 3
        view.select_next();
        assert_eq!(view.selected, 3);

        view.select_prev();
        assert_eq!(view.selected, 2);
    }

    #[test]
    fn benign_y_one_step_approves() {
        for code in [KeyCode::Char('y'), KeyCode::Char('Y')] {
            let mut view = ApprovalView::new(benign_request());
            let action = view.handle_key(create_key_event(code));
            assert!(
                matches!(
                    action,
                    ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
                        decision: ReviewDecision::Approved,
                        ..
                    })
                ),
                "expected Approved for {code:?}"
            );
        }
    }

    #[test]
    fn benign_one_key_approves_via_numeric_pad() {
        let mut view = ApprovalView::new(benign_request());
        let action = view.handle_key(create_key_event(KeyCode::Char('1')));
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
                decision: ReviewDecision::Approved,
                ..
            })
        ));
    }

    #[test]
    fn benign_enter_approves_in_one_step() {
        let mut view = ApprovalView::new(benign_request());
        let action = view.handle_key(create_key_event(KeyCode::Enter));
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
                decision: ReviewDecision::Approved,
                ..
            })
        ));
    }

    #[test]
    fn benign_a_two_approves_for_session() {
        for code in [KeyCode::Char('a'), KeyCode::Char('A'), KeyCode::Char('2')] {
            let mut view = ApprovalView::new(benign_request());
            let action = view.handle_key(create_key_event(code));
            assert!(
                matches!(
                    action,
                    ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
                        decision: ReviewDecision::ApprovedForSession,
                        ..
                    })
                ),
                "expected ApprovedForSession for {code:?}"
            );
        }
    }

    #[test]
    fn benign_n_d_three_all_deny() {
        for code in [
            KeyCode::Char('n'),
            KeyCode::Char('N'),
            KeyCode::Char('d'),
            KeyCode::Char('D'),
            KeyCode::Char('3'),
        ] {
            let mut view = ApprovalView::new(benign_request());
            let action = view.handle_key(create_key_event(code));
            assert!(
                matches!(
                    action,
                    ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
                        decision: ReviewDecision::Denied,
                        ..
                    })
                ),
                "expected Denied for {code:?}"
            );
        }
    }

    #[test]
    fn benign_esc_aborts() {
        let mut view = ApprovalView::new(benign_request());
        let action = view.handle_key(create_key_event(KeyCode::Esc));
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
                decision: ReviewDecision::Abort,
                ..
            })
        ));
    }

    #[test]
    fn test_approval_view_enter_uses_selected_option() {
        let mut view = ApprovalView::new(benign_request());

        // Navigate to index 2 (Denied)
        view.select_next();
        view.select_next();
        assert_eq!(view.selected, 2);

        let action = view.handle_key(create_key_event(KeyCode::Enter));
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
                decision: ReviewDecision::Denied,
                ..
            })
        ));
    }

    #[test]
    fn test_approval_view_navigation_keys() {
        let mut view = ApprovalView::new(benign_request());

        view.handle_key(create_key_event(KeyCode::Up));
        assert_eq!(view.selected, 0); // clamped at 0

        view.handle_key(create_key_event(KeyCode::Down));
        assert_eq!(view.selected, 0);

        view.handle_key(create_key_event(KeyCode::Char('j')));
        assert_eq!(view.selected, 1);

        view.handle_key(create_key_event(KeyCode::Char('k')));
        assert_eq!(view.selected, 0);
    }

    #[test]
    fn test_approval_view_view_params() {
        let mut view = ApprovalView::new(benign_request());
        let action = view.handle_key(create_key_event(KeyCode::Char('v')));
        assert!(matches!(
            action,
            ViewAction::Emit(ViewEvent::OpenTextPager { .. } | ViewEvent::OpenStyledPager { .. })
        ));

        let mut view = ApprovalView::new(benign_request());
        let action = view.handle_key(create_key_event(KeyCode::Char('V')));
        assert!(matches!(
            action,
            ViewAction::Emit(ViewEvent::OpenTextPager { .. } | ViewEvent::OpenStyledPager { .. })
        ));
    }

    #[test]
    fn test_approval_view_view_params_uses_styled_pager_for_files() {
        let mut view = ApprovalView::new(destructive_request());
        let action = view.handle_key(create_key_event(KeyCode::Char('v')));
        match action {
            ViewAction::Emit(ViewEvent::OpenStyledPager { title, lines }) => {
                assert!(title.contains("Details"));
                let rendered = lines
                    .iter()
                    .map(|line| {
                        line.spans
                            .iter()
                            .map(|span| span.content.as_ref())
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(rendered.contains("Changes:"));
                assert!(rendered.contains("@@"));
            }
            other => panic!("expected styled pager, got {other:?}"),
        }
    }

    #[test]
    fn test_approval_view_current_decision_mapping() {
        let mut view = ApprovalView::new(benign_request());

        view.selected = 0;
        assert_eq!(view.current_decision(), ReviewDecision::Approved);
        view.selected = 1;
        assert_eq!(view.current_decision(), ReviewDecision::ApprovedForSession);
        view.selected = 2;
        assert_eq!(view.current_decision(), ReviewDecision::Denied);
        view.selected = 3;
        assert_eq!(view.current_decision(), ReviewDecision::Abort);
    }

    // ========================================================================
    // ApprovalView Tests — Destructive Variant (one-step approve with warning)
    // ========================================================================

    #[test]
    fn destructive_request_routes_destructive() {
        let view = ApprovalView::new(destructive_request());
        assert_eq!(view.risk(), RiskLevel::Destructive);
    }

    #[test]
    fn destructive_y_first_press_approves_once() {
        for code in [KeyCode::Char('y'), KeyCode::Char('Y')] {
            let mut view = ApprovalView::new(destructive_request());

            // First press stages — no decision emitted yet.
            let action = view.handle_key(create_key_event(code));
            assert!(
                matches!(action, ViewAction::None),
                "first press should stage, got {action:?} for {code:?}"
            );
            assert_eq!(view.pending_confirm(), Some(ApprovalOption::ApproveOnce));

            // Second press commits.
            let action = view.handle_key(create_key_event(code));
            assert!(
                matches!(
                    action,
                    ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
                        decision: ReviewDecision::Approved,
                        ..
                    })
                ),
                "expected Approved for {code:?}"
            );
        }
    }

    #[test]
    fn destructive_enter_approves_selected_option() {
        let mut view = ApprovalView::new(destructive_request());

        // First Enter stages the selected option (ApproveOnce).
        let action = view.handle_key(create_key_event(KeyCode::Enter));
        assert!(matches!(action, ViewAction::None));

        // Second Enter commits.
        let action = view.handle_key(create_key_event(KeyCode::Enter));
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
                decision: ReviewDecision::Approved,
                ..
            })
        ));
    }

    #[test]
    fn destructive_navigation_then_enter_commits_highlighted_option() {
        let mut view = ApprovalView::new(destructive_request());

        view.handle_key(create_key_event(KeyCode::Char('j')));
        // First Enter stages.
        view.handle_key(create_key_event(KeyCode::Enter));
        // Second Enter commits.
        let action = view.handle_key(create_key_event(KeyCode::Enter));
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
                decision: ReviewDecision::ApprovedForSession,
                ..
            })
        ));
    }

    #[test]
    fn destructive_unrelated_key_keeps_modal_open() {
        let mut view = ApprovalView::new(destructive_request());

        let action = view.handle_key(create_key_event(KeyCode::Char('q')));
        assert!(matches!(action, ViewAction::None));
    }

    #[test]
    fn destructive_a_first_press_approves_for_session() {
        for code in [KeyCode::Char('a'), KeyCode::Char('A')] {
            let mut view = ApprovalView::new(destructive_request());

            // First press stages.
            let action = view.handle_key(create_key_event(code));
            assert!(matches!(action, ViewAction::None));
            assert_eq!(view.pending_confirm(), Some(ApprovalOption::ApproveAlways));

            // Second press commits.
            let action = view.handle_key(create_key_event(code));
            assert!(
                matches!(
                    action,
                    ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
                        decision: ReviewDecision::ApprovedForSession,
                        ..
                    })
                ),
                "expected ApprovedForSession for {code:?}"
            );
        }
    }

    #[test]
    fn destructive_deny_commits_immediately() {
        // Deny commits immediately — the user is rejecting the tool.
        for code in [
            KeyCode::Char('n'),
            KeyCode::Char('N'),
            KeyCode::Char('d'),
            KeyCode::Char('D'),
        ] {
            let mut view = ApprovalView::new(destructive_request());
            let action = view.handle_key(create_key_event(code));
            assert!(
                matches!(
                    action,
                    ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
                        decision: ReviewDecision::Denied,
                        ..
                    })
                ),
                "expected Denied for {code:?}"
            );
        }
    }

    #[test]
    fn destructive_esc_aborts_immediately() {
        let mut view = ApprovalView::new(destructive_request());
        let action = view.handle_key(create_key_event(KeyCode::Esc));
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
                decision: ReviewDecision::Abort,
                ..
            })
        ));
    }

    // ========================================================================
    // Render takeover smoke tests — keep the visual contract honest so a
    // future widget refactor cannot silently shrink back to a popup.
    // ========================================================================

    fn render_lines(view: &ApprovalView, w: u16, h: u16) -> Vec<String> {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
        ModalView::render(view, Rect::new(0, 0, w, h), &mut buf);
        (0..buf.area.height)
            .map(|row| {
                (0..buf.area.width)
                    .map(|col| buf[(col, row)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    fn compact_rendered_text(lines: &[String]) -> String {
        lines.join("\n").replace(' ', "")
    }

    #[test]
    fn render_benign_includes_review_badge_and_selection_hint() {
        let view = ApprovalView::new(benign_request());
        let lines = render_lines(&view, 100, 40);
        let joined = lines.join("\n");
        assert!(joined.contains("REVIEW"), "missing REVIEW badge:\n{joined}");
        assert!(
            joined.contains("Single key approves"),
            "benign hint missing:\n{joined}"
        );
        assert!(
            joined.contains("Enter / 1 / y"),
            "benign key hint missing:\n{joined}"
        );
        assert!(joined.contains("Safe"));
        assert!(joined.contains("Path"));
        assert!(joined.contains("src/main.rs"));
    }

    #[test]
    fn render_destructive_shows_warning_badge_and_one_step_hint() {
        let view = ApprovalView::new(destructive_request());
        let lines = render_lines(&view, 100, 40);
        let joined = lines.join("\n");
        assert!(
            joined.contains("DESTRUCTIVE"),
            "missing DESTRUCTIVE badge:\n{joined}"
        );
        assert!(
            joined.contains("Two keys to approve"),
            "destructive hint missing:\n{joined}"
        );
        assert!(joined.contains("File Write"));
        assert!(joined.contains("File"));
        assert!(joined.contains("src/main.rs"));
    }

    #[test]
    fn render_destructive_after_stage_shows_confirm_banner() {
        let mut view = ApprovalView::new(destructive_request());
        view.handle_key(create_key_event(KeyCode::Char('y')));
        let lines = render_lines(&view, 100, 40);
        let joined = lines.join("\n");
        assert!(
            joined.contains("Confirm destructive action"),
            "confirm banner missing:\n{joined}"
        );
        assert!(
            joined.contains("Confirm file:"),
            "confirm detail missing:\n{joined}"
        );
        assert!(
            joined.contains("(staged)"),
            "stage marker missing:\n{joined}"
        );
    }

    #[test]
    fn up_down_scroll_diff_without_changing_selection() {
        let mut view = ApprovalView::new(destructive_request());
        let before = view.selected();
        view.handle_key(create_key_event(KeyCode::Char('y')));
        assert_eq!(view.pending_confirm(), Some(ApprovalOption::ApproveOnce));
        assert!(matches!(
            view.handle_key(create_key_event(KeyCode::Up)),
            ViewAction::None
        ));
        assert_eq!(view.selected(), before);
        assert_eq!(view.pending_confirm(), Some(ApprovalOption::ApproveOnce));
        assert!(matches!(
            view.handle_key(create_key_event(KeyCode::Down)),
            ViewAction::None
        ));
        assert_eq!(view.selected(), before);
        assert_eq!(view.pending_confirm(), Some(ApprovalOption::ApproveOnce));
    }

    #[test]
    fn render_destructive_zh_hans_localizes_security_copy() {
        let mut view = ApprovalView::new_for_locale(destructive_request(), Locale::ZhHans);
        let lines = render_lines(&view, 100, 40);
        let joined = compact_rendered_text(&lines);
        assert!(
            joined.contains("破坏性"),
            "missing zh risk badge:\n{joined}"
        );
        assert!(
            joined.contains("两次按键确认："),
            "missing zh two-key hint:\n{joined}"
        );
        assert!(
            joined.contains("文件写入"),
            "missing zh category:\n{joined}"
        );
        assert!(joined.contains("文件"), "missing zh file label:\n{joined}");
        assert!(
            joined.contains("src/main.rs"),
            "missing file path:\n{joined}"
        );
        assert!(
            joined.contains("仅本次批准"),
            "missing zh approve option:\n{joined}"
        );
        assert!(
            joined.contains("本会话自动批准文件写入"),
            "missing zh session approve option:\n{joined}"
        );

        view.handle_key(create_key_event(KeyCode::Char('y')));
        let lines = render_lines(&view, 100, 40);
        let joined = compact_rendered_text(&lines);
        assert!(
            joined.contains("确认破坏性操作"),
            "missing zh confirm banner:\n{joined}"
        );
        assert!(
            joined.contains("确认文件："),
            "missing zh confirm detail:\n{joined}"
        );
        assert!(
            joined.contains("(待确认)"),
            "missing zh staged marker:\n{joined}"
        );
        assert!(
            joined.contains("Enter或y"),
            "missing zh confirm key:\n{joined}"
        );
    }

    #[test]
    fn render_takeover_card_fills_most_of_area() {
        // The card should be wider than the old 65-cell popup whenever
        // the terminal can hold it; this guards against a regression
        // back to the centered popup.
        let view = ApprovalView::new(benign_request());
        let lines = render_lines(&view, 120, 40);
        // Find the widest non-blank rendered row.
        let widest = lines
            .iter()
            .map(|l| l.trim_end_matches(' ').len())
            .max()
            .unwrap_or(0);
        assert!(
            widest >= 80,
            "takeover card too narrow: widest row = {widest} cells"
        );
    }

    // ========================================================================
    // ElevationView Tests
    // ========================================================================

    #[test]
    fn test_elevation_view_initial_state() {
        let request =
            ElevationRequest::for_shell("test-id", "cargo build", "network blocked", true, false);
        let view = ElevationView::new(request);
        assert_eq!(view.selected, 0);
    }

    #[test]
    fn test_elevation_view_keybindings() {
        let request =
            ElevationRequest::for_shell("test-id", "cargo test", "write blocked", false, true);
        let mut view = ElevationView::new(request);

        let action = view.handle_key(create_key_event(KeyCode::Char('n')));
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ElevationDecision {
                option: ElevationOption::WithNetwork,
                ..
            })
        ));

        let request =
            ElevationRequest::for_shell("test-id", "cargo build", "write blocked", false, true);
        let mut view = ElevationView::new(request);
        let action = view.handle_key(create_key_event(KeyCode::Char('w')));
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ElevationDecision {
                option: ElevationOption::WithWriteAccess(_),
                ..
            })
        ));

        let request =
            ElevationRequest::for_shell("test-id", "cargo build", "blocked", false, false);
        let mut view = ElevationView::new(request);
        let action = view.handle_key(create_key_event(KeyCode::Char('f')));
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ElevationDecision {
                option: ElevationOption::FullAccess,
                ..
            })
        ));

        let request =
            ElevationRequest::for_shell("test-id", "cargo build", "blocked", false, false);
        let mut view = ElevationView::new(request);
        let action = view.handle_key(create_key_event(KeyCode::Esc));
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ElevationDecision {
                option: ElevationOption::Abort,
                ..
            })
        ));

        let request =
            ElevationRequest::for_shell("test-id", "cargo build", "blocked", false, false);
        let mut view = ElevationView::new(request);
        let action = view.handle_key(create_key_event(KeyCode::Char('a')));
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ElevationDecision {
                option: ElevationOption::Abort,
                ..
            })
        ));
    }

    #[test]
    fn test_elevation_view_navigation() {
        let request = ElevationRequest::for_shell("test-id", "cargo build", "blocked", true, false);
        let mut view = ElevationView::new(request);

        assert_eq!(view.selected, 0);

        view.handle_key(create_key_event(KeyCode::Down));
        assert_eq!(view.selected, 1);

        view.handle_key(create_key_event(KeyCode::Up));
        assert_eq!(view.selected, 0);

        view.handle_key(create_key_event(KeyCode::Char('j')));
        assert_eq!(view.selected, 1);

        view.handle_key(create_key_event(KeyCode::Char('k')));
        assert_eq!(view.selected, 0);
    }

    #[test]
    fn test_elevation_view_enter_uses_selected_option() {
        let request = ElevationRequest::for_shell("test-id", "cargo build", "blocked", true, false);
        let mut view = ElevationView::new(request);

        view.handle_key(create_key_event(KeyCode::Down));
        assert_eq!(view.selected, 1);

        let action = view.handle_key(create_key_event(KeyCode::Enter));
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ElevationDecision {
                option: ElevationOption::FullAccess,
                ..
            })
        ));
    }

    // ========================================================================
    // ElevationOption Tests
    // ========================================================================

    #[test]
    fn test_elevation_option_labels() {
        assert_eq!(
            ElevationOption::WithNetwork.label(),
            "Allow outbound network"
        );
        assert_eq!(
            ElevationOption::FullAccess.label(),
            "Full access (filesystem + network)"
        );
        assert!(
            ElevationOption::WithWriteAccess(vec![])
                .label()
                .contains("write")
        );
        assert_eq!(ElevationOption::Abort.label(), "Abort");
    }

    #[test]
    fn test_elevation_option_descriptions() {
        assert!(
            ElevationOption::WithNetwork
                .description()
                .contains("network")
        );
        assert!(
            ElevationOption::FullAccess
                .description()
                .contains("filesystem and network access")
        );
        assert!(ElevationOption::Abort.description().contains("Cancel"));
    }

    #[test]
    fn test_elevation_option_to_policy() {
        let cwd = PathBuf::from("/tmp/test");

        let policy = ElevationOption::WithNetwork.to_policy(&cwd);
        assert!(matches!(
            policy,
            SandboxPolicy::WorkspaceWrite {
                network_access: true,
                ..
            }
        ));

        let policy = ElevationOption::FullAccess.to_policy(&cwd);
        assert!(matches!(policy, SandboxPolicy::DangerFullAccess));

        let paths = vec![PathBuf::from("/tmp/test/src")];
        let policy = ElevationOption::WithWriteAccess(paths).to_policy(&cwd);
        assert!(matches!(policy, SandboxPolicy::WorkspaceWrite { .. }));
    }

    // ========================================================================
    // ElevationRequest Tests
    // ========================================================================

    #[test]
    fn test_elevation_request_for_shell_with_network_block() {
        let request = ElevationRequest::for_shell(
            "test-id",
            "curl example.com",
            "network blocked",
            true,
            false,
        );

        assert_eq!(request.tool_id, "test-id");
        assert_eq!(request.tool_name, "exec_shell");
        assert!(request.command.is_some());
        assert!(request.denial_reason.contains("network"));
        assert!(
            request
                .options
                .iter()
                .any(|o| matches!(o, ElevationOption::WithNetwork))
        );
    }

    #[test]
    fn test_elevation_request_for_shell_with_write_block() {
        let request =
            ElevationRequest::for_shell("test-id", "rm -rf /tmp", "write blocked", false, true);

        assert_eq!(request.tool_id, "test-id");
        assert!(
            request
                .options
                .iter()
                .any(|o| matches!(o, ElevationOption::WithWriteAccess(_)))
        );
    }

    #[test]
    fn test_elevation_request_generic() {
        let request = ElevationRequest::generic("test-id", "some_tool", "permission denied");

        assert_eq!(request.tool_id, "test-id");
        assert_eq!(request.tool_name, "some_tool");
        assert!(request.command.is_none());
        assert!(
            request
                .options
                .iter()
                .any(|o| matches!(o, ElevationOption::WithNetwork))
        );
        assert!(
            request
                .options
                .iter()
                .any(|o| matches!(o, ElevationOption::FullAccess))
        );
        assert!(
            request
                .options
                .iter()
                .any(|o| matches!(o, ElevationOption::Abort))
        );
    }

    // ========================================================================
    // ApprovalMode Tests
    // ========================================================================

    #[test]
    fn test_approval_mode_labels() {
        assert_eq!(ApprovalMode::Auto.label(), "AUTO");
        assert_eq!(ApprovalMode::Suggest.label(), "SUGGEST");
        assert_eq!(ApprovalMode::Never.label(), "NEVER");
    }

    #[test]
    fn test_approval_mode_from_config_value_accepts_aliases() {
        assert_eq!(
            ApprovalMode::from_config_value("auto"),
            Some(ApprovalMode::Auto)
        );
        assert_eq!(
            ApprovalMode::from_config_value("on-request"),
            Some(ApprovalMode::Suggest)
        );
        assert_eq!(
            ApprovalMode::from_config_value("deny"),
            Some(ApprovalMode::Never)
        );
        assert_eq!(ApprovalMode::from_config_value("unknown"), None);
    }
}
