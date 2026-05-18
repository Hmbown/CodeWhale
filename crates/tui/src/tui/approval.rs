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
//!   content" surface. The first approve press *stages* a decision and
//!   the second matching press commits — muscle-memory `Enter` cannot
//!   accidentally land on an approval. Any non-approve key clears the
//!   staging and keeps the user in selection mode.
//!
//! The decision events emitted upstream are unchanged
//! (`ViewEvent::ApprovalDecision`), so `ui.rs` and the engine handle
//! both variants without modification. Auto-approve / YOLO bypasses
//! happen *before* the view is constructed (see `tui/ui.rs`); this
//! module always assumes the user is being asked.

use crate::localization::Locale;
use crate::sandbox::SandboxPolicy;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};
use crate::tui::widgets::{ApprovalWidget, ElevationWidget, Renderable};
use crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
/// `RiskLevel::Destructive` requires an explicit second confirmation
/// keypress so muscle-memory `Enter` never lands on an irreversible op.
///
/// Routing rules live in [`classify_risk`] — when in doubt, route to
/// `Destructive`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Benign,
    Destructive,
}

/// Request for user approval of a tool execution
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// Unique ID for this tool use
    pub id: String,
    /// Tool being executed
    pub tool_name: String,
    /// Human-readable tool description from the engine
    pub description: String,
    /// Tool category
    pub category: ToolCategory,
    /// Stakes-based routing for the takeover modal
    pub risk: RiskLevel,
    /// Derived impact summary for the approval prompt
    pub impacts: Vec<String>,
    /// Tool parameters (for display)
    pub params: Value,
    /// Exact-argument fingerprint, used to scope *denials* (#1617).
    pub approval_key: String,
    /// Lossy / arity-aware fingerprint, used to scope *approvals* so an
    /// "approve for session" covers later flag variants (v0.8.37).
    pub approval_grouping_key: String,
}

impl ApprovalRequest {
    pub fn new(
        id: &str,
        tool_name: &str,
        description: &str,
        params: &Value,
        approval_key: &str,
    ) -> Self {
        let category = get_tool_category(tool_name);
        let risk = classify_risk(tool_name, category, params);
        let approval_grouping_key =
            crate::tools::approval_cache::build_approval_grouping_key(tool_name, params).0;

        Self {
            id: id.to_string(),
            tool_name: tool_name.to_string(),
            description: description.to_string(),
            category,
            risk,
            impacts: build_impact_summary(tool_name, category, params),
            params: params.clone(),
            approval_key: approval_key.to_string(),
            approval_grouping_key,
        }
    }

    /// Format parameters for display (truncated)
    pub fn params_display(&self) -> String {
        let truncated = truncate_params_value(&self.params, 200);
        serde_json::to_string(&truncated).unwrap_or_else(|_| truncated.to_string())
    }

    pub fn description_for_locale(&self, locale: Locale) -> String {
        match locale {
            Locale::ZhHans => localized_description_zh_hans(self.category),
            _ => self.description.clone(),
        }
    }

    pub fn impacts_for_locale(&self, locale: Locale) -> Vec<String> {
        match locale {
            Locale::ZhHans => {
                build_impact_summary_zh_hans(&self.tool_name, self.category, &self.params)
            }
            _ => self.impacts.clone(),
        }
    }
}

/// Get the category for a tool by name
pub fn get_tool_category(name: &str) -> ToolCategory {
    if matches!(name, "write_file" | "edit_file" | "apply_patch") {
        ToolCategory::FileWrite
    } else if matches!(name, "web_run" | "web_search" | "fetch_url") {
        ToolCategory::Network
    } else if name == "exec_shell" {
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
/// request looks calm. The split lets the modal swap muscle-memory
/// approval for an explicit two-key confirmation on anything that can
/// touch state outside this turn.
#[must_use]
pub fn classify_risk(tool_name: &str, category: ToolCategory, params: &Value) -> RiskLevel {
    match category {
        // Read paths and discovery — never staged.
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
            if let Some(cmd) = params.get("command").and_then(Value::as_str) {
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

fn mcp_server_hint(tool_name: &str) -> Option<String> {
    let remainder = tool_name.strip_prefix("mcp_")?;
    let (server, _) = remainder.split_once('_')?;
    if server.is_empty() {
        None
    } else {
        Some(server.to_string())
    }
}

fn build_impact_summary(tool_name: &str, category: ToolCategory, params: &Value) -> Vec<String> {
    match category {
        ToolCategory::Safe => {
            let mut impacts = vec!["Read-only operation.".to_string()];
            if let Some(path) = param_preview(params, &["path", "ref_id", "uri"], 72) {
                impacts.push(format!("Reads: {path}"));
            }
            impacts
        }
        ToolCategory::FileWrite => {
            let mut impacts =
                vec!["Writes files in the workspace or an approved write scope.".to_string()];
            if let Some(path) = param_preview(params, &["path", "target", "destination"], 72) {
                impacts.push(format!("Writes: {path}"));
            }
            impacts
        }
        ToolCategory::Shell => {
            let mut impacts = vec!["Executes a shell command.".to_string()];
            if let Some(command) = param_preview(params, &["cmd", "command"], 96) {
                impacts.push(format!("Command: {command}"));
            }
            if let Some(workdir) = param_preview(params, &["workdir", "cwd"], 72) {
                impacts.push(format!("Working dir: {workdir}"));
            }
            impacts
        }
        ToolCategory::Network => {
            let mut impacts = vec!["May reach network services or remote content.".to_string()];
            if let Some(target) =
                param_preview(params, &["url", "q", "query", "location", "repo"], 96)
            {
                impacts.push(format!("Target: {target}"));
            }
            impacts
        }
        ToolCategory::McpRead => {
            let mut impacts =
                vec!["Reads from an MCP server without an obvious local write.".to_string()];
            if let Some(server) = mcp_server_hint(tool_name) {
                impacts.push(format!("Server: {server}"));
            }
            impacts
        }
        ToolCategory::McpAction => {
            let mut impacts =
                vec!["Calls an MCP server action that may have side effects.".to_string()];
            if let Some(server) = mcp_server_hint(tool_name) {
                impacts.push(format!("Server: {server}"));
            }
            impacts
        }
        ToolCategory::Unknown => {
            let mut impacts = vec![
                "Tool is not classified. Review params carefully before approving.".to_string(),
            ];
            if let Some(target) = param_preview(
                params,
                &["path", "cmd", "command", "url", "q", "query", "ref_id"],
                96,
            ) {
                impacts.push(format!("Primary input: {target}"));
            }
            impacts
        }
    }
}

fn localized_description_zh_hans(category: ToolCategory) -> String {
    match category {
        ToolCategory::Safe => "请求执行只读操作。".to_string(),
        ToolCategory::FileWrite => "请求修改文件。请确认路径和内容符合预期。".to_string(),
        ToolCategory::Shell => "请求执行 shell 命令。请先检查命令和工作目录。".to_string(),
        ToolCategory::Network => "请求访问网络或远程内容。请确认目标可信。".to_string(),
        ToolCategory::McpRead => "请求从 MCP 服务器读取信息。".to_string(),
        ToolCategory::McpAction => "请求调用 MCP 服务器操作，可能产生副作用。".to_string(),
        ToolCategory::Unknown => "请求运行未分类工具。批准前请仔细检查参数。".to_string(),
    }
}

fn build_impact_summary_zh_hans(
    tool_name: &str,
    category: ToolCategory,
    params: &Value,
) -> Vec<String> {
    match category {
        ToolCategory::Safe => {
            let mut impacts = vec!["只读操作。".to_string()];
            if let Some(path) = param_preview(params, &["path", "ref_id", "uri"], 72) {
                impacts.push(format!("读取：{path}"));
            }
            impacts
        }
        ToolCategory::FileWrite => {
            let mut impacts = vec!["会写入工作区或已批准写入范围内的文件。".to_string()];
            if let Some(path) = param_preview(params, &["path", "target", "destination"], 72) {
                impacts.push(format!("写入：{path}"));
            }
            impacts
        }
        ToolCategory::Shell => {
            let mut impacts = vec!["执行 shell 命令。".to_string()];
            if let Some(command) = param_preview(params, &["cmd", "command"], 96) {
                impacts.push(format!("命令：{command}"));
            }
            if let Some(workdir) = param_preview(params, &["workdir", "cwd"], 72) {
                impacts.push(format!("工作目录：{workdir}"));
            }
            impacts
        }
        ToolCategory::Network => {
            let mut impacts = vec!["可能访问网络服务或远程内容。".to_string()];
            if let Some(target) =
                param_preview(params, &["url", "q", "query", "location", "repo"], 96)
            {
                impacts.push(format!("目标：{target}"));
            }
            impacts
        }
        ToolCategory::McpRead => {
            let mut impacts = vec!["从 MCP 服务器读取信息，不应产生本地写入。".to_string()];
            if let Some(server) = mcp_server_hint(tool_name) {
                impacts.push(format!("服务器：{server}"));
            }
            impacts
        }
        ToolCategory::McpAction => {
            let mut impacts = vec!["调用可能产生副作用的 MCP 服务器操作。".to_string()];
            if let Some(server) = mcp_server_hint(tool_name) {
                impacts.push(format!("服务器：{server}"));
            }
            impacts
        }
        ToolCategory::Unknown => {
            let mut impacts = vec!["工具未分类。批准前请仔细检查参数。".to_string()];
            if let Some(target) = param_preview(
                params,
                &["path", "cmd", "command", "url", "q", "query", "ref_id"],
                96,
            ) {
                impacts.push(format!("主要输入：{target}"));
            }
            impacts
        }
    }
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

    /// Whether this option needs an explicit second-key confirmation in
    /// the destructive variant. Deny/Abort are never staged.
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
    /// When `Some`, the destructive variant has staged this approval and
    /// is waiting for the user to press the same key (or `Enter`) again.
    /// Any other key clears the staging.
    pending_confirm: Option<ApprovalOption>,
    timeout: Option<Duration>,
    requested_at: Instant,
    /// Whether the approval card is collapsed to a single-line banner.
    pub(crate) collapsed: bool,
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
            pending_confirm: None,
            timeout: None,
            requested_at: Instant::now(),
            collapsed: false,
        }
    }

    fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        // Moving the selection abandons any staged confirmation; the
        // user is reconsidering.
        self.pending_confirm = None;
    }

    fn select_next(&mut self) {
        self.selected = (self.selected + 1).min(ApprovalOption::ORDER.len() - 1);
        self.pending_confirm = None;
    }

    fn current_option(&self) -> ApprovalOption {
        ApprovalOption::from_index(self.selected)
    }

    /// Test-only accessor — the widget reads decisions through
    /// `commit_or_stage` instead of polling.
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

    /// The staged option, if any. `None` in the benign variant or when
    /// no approve key has been pressed yet.
    pub(crate) fn pending_confirm(&self) -> Option<ApprovalOption> {
        self.pending_confirm
    }

    pub(crate) fn locale(&self) -> Locale {
        self.locale
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
        let content = serde_json::to_string_pretty(&self.request.params)
            .unwrap_or_else(|_| self.request.params.to_string());
        ViewAction::Emit(ViewEvent::OpenTextPager {
            title: format!("Tool Params: {}", self.request.tool_name),
            content,
        })
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
        match key.code {
            KeyCode::Tab => {
                self.collapsed = !self.collapsed;
                ViewAction::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.select_prev();
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.select_next();
                ViewAction::None
            }
            KeyCode::Enter => self.commit_or_stage(self.current_option()),
            // Direct shortcuts; '1' / '2' map to the first two options
            // so a numeric pad still works for benign approve flows.
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
            KeyCode::Char('v') | KeyCode::Char('V') => {
                self.pending_confirm = None;
                self.emit_params_pager()
            }
            KeyCode::Esc => self.emit_decision(ReviewDecision::Abort, false),
            _ => {
                // Any unrecognised key cancels a staged confirmation —
                // the user is no longer aiming at "approve".
                self.pending_confirm = None;
                ViewAction::None
            }
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

fn truncate_params_value(value: &Value, max_len: usize) -> Value {
    match value {
        Value::Object(map) => {
            let truncated = map
                .iter()
                .map(|(key, val)| (key.clone(), truncate_params_value(val, max_len)))
                .collect();
            Value::Object(truncated)
        }
        Value::Array(items) => {
            let truncated_items = items
                .iter()
                .map(|val| truncate_params_value(val, max_len))
                .collect();
            Value::Array(truncated_items)
        }
        Value::String(text) => Value::String(truncate_string_value(text, max_len)),
        other => {
            let rendered = other.to_string();
            if rendered.chars().count() > max_len {
                Value::String(truncate_string_value(&rendered, max_len))
            } else {
                other.clone()
            }
        }
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
        // web_search is read-only enough to skip the two-key dance.
        let cat = ToolCategory::Network;
        assert_eq!(
            classify_risk("web_search", cat, &json!({"q": "rust"})),
            RiskLevel::Benign
        );
        // fetch_url pulls arbitrary remote content; never staged.
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
    fn test_approval_request_params_display_truncates() {
        let long_content = "x".repeat(300);
        let params = json!({"path": "src/main.rs", "content": long_content});
        let request = ApprovalRequest::new(
            "test-id",
            "write_file",
            "Write a file to disk",
            &params,
            "test_key",
        );

        let display = request.params_display();
        assert!(display.len() < 250);
        assert!(display.contains("src/main.rs"));
    }

    #[test]
    fn test_approval_request_params_display_short() {
        let params = json!({"path": "src/main.rs"});
        let request = ApprovalRequest::new(
            "test-id",
            "read_file",
            "Read a file from disk",
            &params,
            "test_key",
        );

        let display = request.params_display();
        assert!(display.contains("src/main.rs"));
    }

    #[test]
    fn test_approval_request_derives_impact_summary() {
        let params = json!({"cmd": "cargo test", "workdir": "/tmp/project"});
        let request = ApprovalRequest::new(
            "test-id",
            "exec_shell",
            "Run a shell command",
            &params,
            "test_key",
        );

        assert_eq!(request.category, ToolCategory::Shell);
        assert!(
            request
                .impacts
                .iter()
                .any(|line| line.contains("Executes a shell command"))
        );
        assert!(
            request
                .impacts
                .iter()
                .any(|line| line.contains("cargo test"))
        );
    }

    // ========================================================================
    // ApprovalView Tests — Benign Variant (single-key approve)
    // ========================================================================

    #[test]
    fn test_approval_view_initial_state() {
        let view = ApprovalView::new(benign_request());
        assert_eq!(view.selected, 0);
        assert!(view.timeout.is_none());
        assert_eq!(view.pending_confirm(), None);
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
        assert_eq!(view.selected, 1);

        view.handle_key(create_key_event(KeyCode::Char('j')));
        assert_eq!(view.selected, 2);

        view.handle_key(create_key_event(KeyCode::Char('k')));
        assert_eq!(view.selected, 1);
    }

    #[test]
    fn test_approval_view_view_params() {
        let mut view = ApprovalView::new(benign_request());
        let action = view.handle_key(create_key_event(KeyCode::Char('v')));
        assert!(matches!(
            action,
            ViewAction::Emit(ViewEvent::OpenTextPager { .. })
        ));

        let mut view = ApprovalView::new(benign_request());
        let action = view.handle_key(create_key_event(KeyCode::Char('V')));
        assert!(matches!(
            action,
            ViewAction::Emit(ViewEvent::OpenTextPager { .. })
        ));
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
    // ApprovalView Tests — Destructive Variant (two-key confirm)
    // ========================================================================

    #[test]
    fn destructive_request_routes_destructive() {
        let view = ApprovalView::new(destructive_request());
        assert_eq!(view.risk(), RiskLevel::Destructive);
    }

    #[test]
    fn destructive_y_first_press_stages_then_second_commits() {
        for code in [KeyCode::Char('y'), KeyCode::Char('Y')] {
            let mut view = ApprovalView::new(destructive_request());

            // First press stages — no decision emitted yet.
            let action = view.handle_key(create_key_event(code));
            assert!(matches!(action, ViewAction::None));
            assert_eq!(view.pending_confirm(), Some(ApprovalOption::ApproveOnce));

            // Second press of the same key commits.
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
    fn destructive_enter_first_press_stages_then_second_commits() {
        let mut view = ApprovalView::new(destructive_request());

        // Selection starts at ApproveOnce — Enter stages.
        let action = view.handle_key(create_key_event(KeyCode::Enter));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.pending_confirm(), Some(ApprovalOption::ApproveOnce));

        // Second Enter on the same selection commits.
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
    fn destructive_navigation_clears_staged_confirmation() {
        let mut view = ApprovalView::new(destructive_request());

        view.handle_key(create_key_event(KeyCode::Char('y')));
        assert_eq!(view.pending_confirm(), Some(ApprovalOption::ApproveOnce));

        // Moving the selection abandons the staging.
        view.handle_key(create_key_event(KeyCode::Down));
        assert_eq!(view.pending_confirm(), None);
    }

    #[test]
    fn destructive_unrelated_key_clears_staged_confirmation() {
        let mut view = ApprovalView::new(destructive_request());

        view.handle_key(create_key_event(KeyCode::Char('y')));
        assert_eq!(view.pending_confirm(), Some(ApprovalOption::ApproveOnce));

        // A key with no mapped action clears the staging.
        let action = view.handle_key(create_key_event(KeyCode::Char('q')));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.pending_confirm(), None);
    }

    #[test]
    fn destructive_a_first_press_stages_then_second_commits_session() {
        for code in [KeyCode::Char('a'), KeyCode::Char('A')] {
            let mut view = ApprovalView::new(destructive_request());

            let action = view.handle_key(create_key_event(code));
            assert!(matches!(action, ViewAction::None));
            assert_eq!(view.pending_confirm(), Some(ApprovalOption::ApproveAlways));

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
    fn destructive_y_then_a_does_not_commit_either() {
        // Pressing 'y' then 'a' must NOT commit ApproveAlways — the
        // second key is a different option, so it re-stages instead.
        let mut view = ApprovalView::new(destructive_request());

        let action = view.handle_key(create_key_event(KeyCode::Char('y')));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.pending_confirm(), Some(ApprovalOption::ApproveOnce));

        let action = view.handle_key(create_key_event(KeyCode::Char('a')));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.pending_confirm(), Some(ApprovalOption::ApproveAlways));
    }

    #[test]
    fn destructive_deny_does_not_require_confirmation() {
        // Deny / Abort skip the two-key dance — the user is bailing.
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
        // Stage something first.
        view.handle_key(create_key_event(KeyCode::Char('y')));
        // Esc still aborts in one press.
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
    fn render_benign_includes_review_badge_and_one_step_hint() {
        let view = ApprovalView::new(benign_request());
        let lines = render_lines(&view, 100, 40);
        let joined = lines.join("\n");
        assert!(joined.contains("REVIEW"), "missing REVIEW badge:\n{joined}");
        assert!(
            joined.contains("Single key approves"),
            "benign hint missing:\n{joined}"
        );
        assert!(joined.contains("read_file"));
    }

    #[test]
    fn render_destructive_shows_warning_badge_and_two_step_hint() {
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
        assert!(joined.contains("write_file"));
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
            joined.contains("(staged)"),
            "stage marker missing:\n{joined}"
        );
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
            joined.contains("两次按键确认"),
            "missing zh two-step hint:\n{joined}"
        );
        assert!(
            joined.contains("文件写入"),
            "missing zh category:\n{joined}"
        );
        assert!(
            joined.contains("影响："),
            "missing zh impact label:\n{joined}"
        );
        assert!(
            joined.contains("写入：src/main.rs"),
            "missing zh impact path:\n{joined}"
        );
        assert!(
            joined.contains("仅本次批准"),
            "missing zh approve option:\n{joined}"
        );

        view.handle_key(create_key_event(KeyCode::Char('y')));
        let lines = render_lines(&view, 100, 40);
        let joined = compact_rendered_text(&lines);
        assert!(
            joined.contains("确认破坏性操作"),
            "missing zh confirm banner:\n{joined}"
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
