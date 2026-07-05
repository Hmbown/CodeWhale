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
//! - **Destructive** (`RiskLevel::Destructive`) — file writes, shell
//!   commands that are not proven read-only, patches, MCP actions,
//!   unclassified tools, and any "fetch arbitrary content" surface.
//!   The takeover keeps the destructive badge and
//!   impact summary visible, then lets `Enter` commit the highlighted
//!   option or `y` / `a` / `d` commit directly.
//!
//! The decision events emitted upstream are unchanged
//! (`ViewEvent::ApprovalDecision`), so `ui.rs` and the engine handle
//! both variants without modification. Auto-approve / YOLO bypasses
//! happen *before* the view is constructed (see `tui/ui.rs`); this
//! module always assumes the user is being asked.

use crate::command_safety::is_parallel_readonly_command;
use crate::localization::Locale;
use crate::sandbox::SandboxPolicy;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};
use crate::tui::widgets::{ApprovalWidget, ElevationWidget, Renderable};
use codewhale_config::ToolAskRule;
use crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Determines when tool executions require user approval
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApprovalMode {
    /// Automatically review risky tool calls before deciding whether to ask.
    Auto,
    /// Bypass approvals entirely (YOLO mode / --yolo flag).
    Bypass,
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
            ApprovalMode::Bypass => "BYPASS",
            ApprovalMode::Suggest => "SUGGEST",
            ApprovalMode::Never => "NEVER",
        }
    }

    pub fn from_config_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(ApprovalMode::Auto),
            "bypass" | "yolo" | "dontask" | "dont_ask" | "bypass-permissions"
            | "bypasspermissions" => Some(ApprovalMode::Bypass),
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
    /// Sub-agent lifecycle (`agent` start/status/peek/cancel); the child's
    /// own tool gates govern what it may actually do
    Agent,
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
    /// The model's explanation of intent before invoking write tools (#2381).
    /// Displayed in the approval view so users understand *why* the change
    /// is being made before reviewing *what* will change.
    pub intent_summary: Option<String>,
    /// Ask-only persistent rules that can be saved with the approval.
    pub persistent_ask_rules: Vec<ToolAskRule>,
}

/// Key approval details rendered prominently in the approval card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalDetail {
    pub label: String,
    pub value: String,
    /// Preformatted shell lines for commands that benefit from safe wrapping
    /// or a compact write-file preview. `value` remains the original command.
    pub shell_lines: Option<Vec<String>>,
}

/// Human-readable preview of ask-only rules the `S` approval shortcut would
/// append. This is intentionally derived from `persistent_ask_rules` only; the
/// approval UI must not re-parse tool inputs such as patches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskRuleSavePreview {
    pub rule_count: usize,
    pub entries: Vec<String>,
    pub omitted: usize,
}

impl AskRuleSavePreview {
    #[must_use]
    pub fn summary(&self) -> String {
        let noun = if self.rule_count == 1 {
            "rule"
        } else {
            "rules"
        };
        format!("{} ask {noun}", self.rule_count)
    }
}

const ASK_RULE_SAVE_PREVIEW_MAX_ENTRIES: usize = 4;

impl ApprovalRequest {
    /// Presentation stakes for this request (see [`ApprovalStakes`]).
    #[must_use]
    pub fn stakes(&self) -> ApprovalStakes {
        classify_stakes(&self.tool_name, self.category, self.risk, &self.params)
    }

    #[cfg(test)]
    pub fn new(
        id: &str,
        tool_name: &str,
        description: &str,
        params: &Value,
        approval_key: &str,
    ) -> Self {
        Self::new_with_intent(
            id,
            tool_name,
            description,
            params,
            approval_key,
            None,
            Path::new("/workspace"),
        )
    }

    pub fn new_with_intent(
        id: &str,
        tool_name: &str,
        description: &str,
        params: &Value,
        approval_key: &str,
        intent_summary: Option<&str>,
        workspace: &Path,
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
            intent_summary: intent_summary.and_then(|summary| {
                let summary = summary.trim();
                if summary.is_empty() {
                    None
                } else {
                    Some(summary.to_string())
                }
            }),
            persistent_ask_rules: build_persistent_ask_rules(tool_name, params, workspace),
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
            _ if self.category == ToolCategory::Shell => {
                "Review the Bash command before it runs.".to_string()
            }
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

    #[must_use]
    pub fn can_save_ask_rule(&self) -> bool {
        !self.persistent_ask_rules.is_empty()
    }

    #[must_use]
    pub fn ask_rule_save_preview(&self) -> Option<AskRuleSavePreview> {
        build_ask_rule_save_preview(
            &self.persistent_ask_rules,
            ASK_RULE_SAVE_PREVIEW_MAX_ENTRIES,
        )
    }

    #[must_use]
    #[cfg(test)]
    pub fn ask_rule_preview(&self) -> Option<String> {
        if self.persistent_ask_rules.is_empty() {
            return None;
        }
        let permissions = codewhale_config::PermissionsToml {
            rules: self.persistent_ask_rules.clone(),
        };
        toml::to_string_pretty(&permissions).ok()
    }

    /// Extract the most important params for the approval card.
    #[must_use]
    pub fn prominent_detail_items(&self, locale: Locale) -> Vec<ApprovalDetail> {
        build_prominent_details(&self.tool_name, self.category, &self.params)
            .into_iter()
            .map(|mut detail| {
                let is_preview = detail.label == "Preview";
                detail.label = localize_detail_label(&detail.label, locale).to_string();
                if is_preview {
                    if let Some(lines) = detail.shell_lines.as_mut() {
                        for line in lines.iter_mut() {
                            *line = localize_preview_shell_line(&self.tool_name, line, locale)
                                .to_string();
                        }
                        detail.value = lines.join("\n");
                    }
                }
                detail
            })
            .collect()
    }
}

#[must_use]
fn build_ask_rule_save_preview(
    rules: &[ToolAskRule],
    max_entries: usize,
) -> Option<AskRuleSavePreview> {
    if rules.is_empty() {
        return None;
    }

    let entries = rules
        .iter()
        .take(max_entries)
        .map(format_ask_rule_save_entry)
        .collect();
    Some(AskRuleSavePreview {
        rule_count: rules.len(),
        entries,
        omitted: rules.len().saturating_sub(max_entries),
    })
}

#[must_use]
fn format_ask_rule_save_entry(rule: &ToolAskRule) -> String {
    let mut parts = vec![format!(
        "tool={}",
        sanitize_ask_rule_preview_value(&rule.tool)
    )];
    if let Some(command) = &rule.command {
        parts.push(format!(
            "command={}",
            sanitize_ask_rule_preview_value(command)
        ));
    }
    if let Some(path) = &rule.path {
        parts.push(format!("path={}", sanitize_ask_rule_preview_value(path)));
    }
    parts.join(" ")
}

#[must_use]
fn sanitize_ask_rule_preview_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

#[must_use]
fn build_persistent_ask_rules(
    tool_name: &str,
    params: &Value,
    workspace: &Path,
) -> Vec<ToolAskRule> {
    match tool_name {
        "exec_shell" => build_exec_shell_ask_rules(params),
        // File writes save an exact, workspace-relative path so a later
        // edit/write of the same file is matched. read_file stays out: this
        // boundary is about persisting *write* approvals only.
        "write_file" | "edit_file" => build_file_write_ask_rules(tool_name, params, workspace),
        "apply_patch" => build_apply_patch_ask_rules(params, workspace),
        _ => Vec::new(),
    }
}

#[must_use]
fn build_exec_shell_ask_rules(params: &Value) -> Vec<ToolAskRule> {
    let Some(command) = params
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|command| !command.is_empty())
    else {
        return Vec::new();
    };
    vec![ToolAskRule::exec_shell(command)]
}

#[must_use]
fn build_file_write_ask_rules(
    tool_name: &str,
    params: &Value,
    workspace: &Path,
) -> Vec<ToolAskRule> {
    let Some(path) = params
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
    else {
        return Vec::new();
    };
    // Reuse the canonical matcher normalization so the saved rule equals what
    // runtime matching compares against. `None` (and the degenerate
    // workspace-root case) means the path is empty, traversing, drive-relative,
    // or outside the workspace, so we save nothing and the `S` shortcut and
    // preview stay disabled.
    let workspace = workspace.to_string_lossy();
    let Some(relative) =
        codewhale_execpolicy::normalize_workspace_relative_path(path, workspace.as_ref())
            .filter(|relative| !relative.is_empty())
    else {
        return Vec::new();
    };
    vec![ToolAskRule::file_path(tool_name, relative)]
}

#[must_use]
fn build_apply_patch_ask_rules(params: &Value, workspace: &Path) -> Vec<ToolAskRule> {
    let Ok(preflight) = crate::tools::apply_patch::preflight_apply_patch(params) else {
        return Vec::new();
    };
    let workspace = workspace.to_string_lossy();
    let mut rules = Vec::new();

    for path in preflight.touched_files {
        let Some(relative) =
            codewhale_execpolicy::normalize_workspace_relative_path(&path, workspace.as_ref())
                .filter(|relative| !relative.is_empty())
        else {
            return Vec::new();
        };
        let rule = ToolAskRule::file_path("apply_patch", relative);
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    }

    rules
}

/// Get the category for a tool by name
pub fn get_tool_category(name: &str) -> ToolCategory {
    if name == "agent" || name == "whaleflow" {
        ToolCategory::Agent
    } else if matches!(name, "write_file" | "edit_file" | "apply_patch") {
        ToolCategory::FileWrite
    } else if matches!(
        name,
        "web_run" | "web_search" | "fetch_url" | "wait_for_dev_server"
    ) {
        ToolCategory::Network
    } else if matches!(
        name,
        "exec_shell"
            | "task_shell_start"
            | "task_shell_wait"
            | "exec_shell_wait"
            | "exec_shell_interact"
            | "exec_wait"
            | "exec_interact"
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

/// Presentation-level stakes for the approval prompt (#3883 follow-up).
///
/// `RiskLevel` drives keymaps and stays conservative ("not provably
/// read-only" is `Destructive`), but rendering everything in that bucket
/// as a red DESTRUCTIVE takeover made routine file edits and build
/// commands read like emergencies. Stakes split presentation three ways:
///
/// - `Routine` — provably read-only; minimal chrome.
/// - `Elevated` — ordinary state-touching work (edits, builds, MCP
///   actions); a calm approval, not a warning.
/// - `Critical` — genuinely destructive, publish-like, or
///   secret-touching per `ToolActionKind`; keeps the strong styling and
///   the policy semantics lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalStakes {
    Routine,
    Elevated,
    Critical,
}

#[must_use]
pub fn classify_stakes(
    tool_name: &str,
    category: ToolCategory,
    risk: RiskLevel,
    params: &Value,
) -> ApprovalStakes {
    if matches!(risk, RiskLevel::Benign) {
        return ApprovalStakes::Routine;
    }
    match crate::tui::auto_review::ToolActionKind::from_tool_call(tool_name, params, category) {
        crate::tui::auto_review::ToolActionKind::Publish
        | crate::tui::auto_review::ToolActionKind::Destructive
        | crate::tui::auto_review::ToolActionKind::Secret => ApprovalStakes::Critical,
        _ => ApprovalStakes::Elevated,
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
            "web_search" | "wait_for_dev_server" => RiskLevel::Benign,
            // web_run is benign for search/query, but its `open`/`click`
            // actions fetch model-supplied URLs (arbitrary remote content) —
            // destructive, consistent with fetch_url.
            "web_run" => {
                let fetches_url = params
                    .get("open")
                    .and_then(Value::as_array)
                    .is_some_and(|a| !a.is_empty())
                    || params
                        .get("click")
                        .and_then(Value::as_array)
                        .is_some_and(|a| !a.is_empty());
                if fetches_url {
                    RiskLevel::Destructive
                } else {
                    RiskLevel::Benign
                }
            }
            _ => RiskLevel::Destructive,
        },
        // Shell stays destructive unless the existing command-safety analyzer
        // can prove the concrete command is read-only.
        ToolCategory::Shell => {
            if let Some(cmd) = params.get("command").and_then(Value::as_str) {
                if is_parallel_readonly_command(cmd) {
                    return RiskLevel::Benign;
                }
            }
            RiskLevel::Destructive
        }
        // Sub-agent lifecycle: status/peek are inspection-only. Starts and
        // other actions keep the explicit-options keymap (the child's own
        // gates govern what it may do once running).
        ToolCategory::Agent => match params.get("action").and_then(Value::as_str) {
            Some("status" | "peek" | "list") => RiskLevel::Benign,
            _ => RiskLevel::Destructive,
        },
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

fn mcp_target_hint(tool_name: &str) -> Option<String> {
    let remainder = tool_name.strip_prefix("mcp_")?;
    if remainder.is_empty() {
        None
    } else {
        Some(remainder.to_string())
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
            vec!["Executes a Bash command in your workspace.".to_string()]
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
            if let Some(target) = mcp_target_hint(tool_name) {
                impacts.push(format!("MCP target: {target}"));
            }
            impacts
        }
        ToolCategory::McpAction => {
            let mut impacts =
                vec!["Calls an MCP server action that may have side effects.".to_string()];
            if let Some(target) = mcp_target_hint(tool_name) {
                impacts.push(format!("MCP target: {target}"));
            }
            impacts
        }
        ToolCategory::Agent => {
            if tool_name == "whaleflow" {
                return vec![
                    "Runs a sandboxed orchestration script.".to_string(),
                    "May spawn many sub-agents and consume token budget.".to_string(),
                ];
            }
            let mut impacts = vec![
                "Starts or inspects a child agent task; the child's own tool gates still apply."
                    .to_string(),
            ];
            if let Some(kind) = param_preview(params, &["type"], 40) {
                impacts.push(format!("Child type: {kind}"));
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
        ToolCategory::Agent => {
            "请求启动或查看子代理任务；子代理仍受其自身工具门控约束。".to_string()
        }
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
            vec!["在工作区执行 Bash 命令。".to_string()]
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
            if let Some(target) = mcp_target_hint(tool_name) {
                impacts.push(format!("MCP 目标：{target}"));
            }
            impacts
        }
        ToolCategory::McpAction => {
            let mut impacts = vec!["调用可能产生副作用的 MCP 服务器操作。".to_string()];
            if let Some(target) = mcp_target_hint(tool_name) {
                impacts.push(format!("MCP 目标：{target}"));
            }
            impacts
        }
        ToolCategory::Agent => {
            if tool_name == "whaleflow" {
                return vec![
                    "运行沙盒化的编排脚本。".to_string(),
                    "可能生成大量子代理并消耗令牌预算。".to_string(),
                ];
            }
            let mut impacts =
                vec!["启动或查看子代理任务；子代理仍受其自身工具门控约束。".to_string()];
            if let Some(kind) = param_preview(params, &["type"], 40) {
                impacts.push(format!("子代理类型：{kind}"));
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

fn build_prominent_details(
    tool_name: &str,
    category: ToolCategory,
    params: &Value,
) -> Vec<ApprovalDetail> {
    let mut details = Vec::new();
    // WhaleFlow dynamic-workflow prompt: label, static plan summary
    // (explicitly a heuristic preview — aliased/computed calls evade it),
    // bounded script preview (the full script stays one `v` away in the
    // params pager), and an ALWAYS-rendered capability warning that does not
    // depend on the scan results. Exactly the widget's 4-detail cap.
    if tool_name == "whaleflow" {
        let label = params
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or("(unlabeled script)");
        details.push(ApprovalDetail {
            label: "Workflow".to_string(),
            value: label.to_string(),
            shell_lines: None,
        });
        let script = params.get("script").and_then(Value::as_str).unwrap_or("");
        let plan_lines = crate::tools::subagent::whaleflow_bridge::whaleflow_plan_summary(script);
        details.push(ApprovalDetail {
            label: "Plan".to_string(),
            value: plan_lines.join("\n"),
            shell_lines: Some(plan_lines),
        });
        let preview_lines = prefixed_preview_lines("script", "  ", script, 5);
        details.push(ApprovalDetail {
            label: "Preview".to_string(),
            value: preview_lines.join("\n"),
            shell_lines: Some(preview_lines),
        });
        details.push(ApprovalDetail {
            label: "Note".to_string(),
            value: "May spawn sub-agents and call any tool available to this context; \
                    normal approval rules apply to each call."
                .to_string(),
            shell_lines: None,
        });
        return details;
    }
    match category {
        ToolCategory::Shell => {
            if let Some(command) = param_text(params, &["command", "cmd"]) {
                details.push(ApprovalDetail {
                    label: "Command".to_string(),
                    shell_lines: Some(format_shell_command_for_approval(&command)),
                    value: command,
                });
            }
            if let Some(workdir) = param_preview(params, &["workdir", "cwd"], 96) {
                details.push(ApprovalDetail {
                    label: "Dir".to_string(),
                    value: workdir,
                    shell_lines: None,
                });
            }
        }
        ToolCategory::FileWrite => {
            if let Some(path) = param_preview(params, &["path", "target", "destination"], 200) {
                details.push(ApprovalDetail {
                    label: "File".to_string(),
                    value: path,
                    shell_lines: None,
                });
            }
            if let Some(preview_lines) = file_write_preview_lines(tool_name, params) {
                details.push(ApprovalDetail {
                    label: "Preview".to_string(),
                    value: preview_lines.join("\n"),
                    shell_lines: Some(preview_lines),
                });
            }
        }
        ToolCategory::Safe => {
            if let Some(path) = param_preview(params, &["path", "ref_id", "uri"], 200) {
                details.push(ApprovalDetail {
                    label: "Path".to_string(),
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
                    label: "Target".to_string(),
                    value: target,
                    shell_lines: None,
                });
            }
        }
        ToolCategory::Agent => {
            if let Some(action) = param_preview(params, &["action"], 40) {
                details.push(ApprovalDetail {
                    label: "Action".to_string(),
                    value: action,
                    shell_lines: None,
                });
            }
            if let Some(kind) = param_preview(params, &["type"], 40) {
                details.push(ApprovalDetail {
                    label: "Type".to_string(),
                    value: kind,
                    shell_lines: None,
                });
            }
            if let Some(prompt) = param_preview(params, &["prompt", "task", "message"], 200) {
                details.push(ApprovalDetail {
                    label: "Prompt".to_string(),
                    value: prompt,
                    shell_lines: None,
                });
            }
        }
        ToolCategory::McpRead | ToolCategory::McpAction | ToolCategory::Unknown => {
            if let Some(input) = param_preview(
                params,
                &["command", "cmd", "path", "url", "q", "query", "ref_id"],
                200,
            ) {
                details.push(ApprovalDetail {
                    label: "Input".to_string(),
                    value: input,
                    shell_lines: None,
                });
            }
        }
    }
    details
}

fn file_write_preview_lines(tool_name: &str, params: &Value) -> Option<Vec<String>> {
    match tool_name {
        "write_file" => {
            let content = param_text(params, &["content"])?;
            Some(prefixed_preview_lines(
                "proposed content",
                "+ ",
                &content,
                5,
            ))
        }
        "edit_file" => {
            let search = param_text(params, &["search"])?;
            let replace = param_text(params, &["replace"])?;
            let mut lines = Vec::new();
            lines.extend(prefixed_preview_lines("replace this", "- ", &search, 3));
            lines.extend(prefixed_preview_lines("with this", "+ ", &replace, 3));
            Some(lines)
        }
        "apply_patch" => params
            .get("patch")
            .and_then(Value::as_str)
            .and_then(apply_patch_preview_lines)
            .or_else(|| {
                params
                    .get("changes")
                    .and_then(Value::as_array)
                    .and_then(|changes| changes_preview_lines(changes))
            }),
        _ => None,
    }
    .filter(|lines| !lines.is_empty())
}

fn prefixed_preview_lines(
    header: &str,
    prefix: &str,
    content: &str,
    max_lines: usize,
) -> Vec<String> {
    let mut lines = vec![header.to_string()];
    if content.is_empty() {
        lines.push(format!("{prefix}<empty>"));
        return lines;
    }

    let total = content.lines().count();
    for line in content.lines().take(max_lines) {
        lines.push(format!("{prefix}{line}"));
    }
    if total > max_lines {
        lines.push(format!("... (+{} more lines)", total - max_lines));
    }
    lines
}

fn push_preview_line(lines: &mut Vec<String>, line: impl Into<String>, limit: usize) -> bool {
    if lines.len() >= limit {
        return false;
    }
    lines.push(line.into());
    true
}

fn append_preview_truncation(lines: &mut Vec<String>, line: String, limit: usize) {
    if push_preview_line(lines, line.clone(), limit) {
        return;
    }
    if let Some(last) = lines.last_mut() {
        *last = line;
    }
}

fn apply_patch_preview_lines(patch: &str) -> Option<Vec<String>> {
    const PREVIEW_LIMIT: usize = 7;

    let mut lines = Vec::new();
    let mut omitted = 0usize;
    for line in patch.lines().filter(|line| !line.trim().is_empty()) {
        let is_diff_header = line.starts_with("diff --git ")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("@@");
        let is_change_line = (line.starts_with('+') && !line.starts_with("+++"))
            || (line.starts_with('-') && !line.starts_with("---"));
        if is_diff_header || is_change_line {
            if !push_preview_line(&mut lines, line, PREVIEW_LIMIT) {
                omitted += 1;
            }
        } else {
            omitted += 1;
        }
    }

    if lines.is_empty() {
        omitted = 0;
        for line in patch.lines().filter(|line| !line.trim().is_empty()) {
            if !push_preview_line(&mut lines, line, PREVIEW_LIMIT) {
                omitted += 1;
            }
        }
    }

    if omitted > 0 {
        if lines.len() >= PREVIEW_LIMIT {
            omitted += 1;
        }
        append_preview_truncation(
            &mut lines,
            format!("... (+{omitted} more patch lines)"),
            PREVIEW_LIMIT,
        );
    }
    if lines.is_empty() { None } else { Some(lines) }
}

fn changes_preview_lines(changes: &[Value]) -> Option<Vec<String>> {
    const PREVIEW_LIMIT: usize = 7;

    let mut lines = Vec::new();
    let mut rendered_changes = 0usize;
    for (idx, change) in changes.iter().enumerate() {
        let path = change
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("<file>");
        let content = change.get("content").and_then(Value::as_str).unwrap_or("");
        if idx > 0 {
            if !push_preview_line(&mut lines, String::new(), PREVIEW_LIMIT) {
                break;
            }
        }
        if !push_preview_line(&mut lines, format!("file: {path}"), PREVIEW_LIMIT) {
            break;
        }
        rendered_changes += 1;
        for line in prefixed_preview_lines("replacement content", "+ ", content, PREVIEW_LIMIT)
            .into_iter()
            .skip(1)
        {
            if !push_preview_line(&mut lines, line, PREVIEW_LIMIT) {
                break;
            }
        }
        if lines.len() >= PREVIEW_LIMIT {
            break;
        }
    }
    let skipped_changes = changes.len().saturating_sub(rendered_changes);
    if skipped_changes > 0 {
        append_preview_truncation(
            &mut lines,
            format!("... (+{skipped_changes} more files)"),
            PREVIEW_LIMIT,
        );
    }
    if lines.is_empty() { None } else { Some(lines) }
}

fn param_text(params: &Value, keys: &[&str]) -> Option<String> {
    let Value::Object(map) = params else {
        return None;
    };

    for key in keys {
        let Some(value) = map.get(*key) else {
            continue;
        };
        match value {
            Value::String(text) => return Some(text.clone()),
            Value::Number(number) => return Some(number.to_string()),
            Value::Bool(flag) => return Some(flag.to_string()),
            other => return Some(other.to_string()),
        }
    }

    None
}

fn localize_detail_label(label: &str, locale: Locale) -> &str {
    match locale {
        Locale::ZhHans => match label {
            "Command" => "命令",
            "Dir" => "目录",
            "File" => "文件",
            "Preview" => "预览",
            "proposed content" => "拟写入内容",
            "replace this" => "替换此内容",
            "with this" => "替换为",
            "replacement content" => "替换内容",
            "Path" => "路径",
            "Target" => "目标",
            "Input" => "输入",
            "Action" => "操作",
            "Type" => "类型",
            "Prompt" => "提示",
            "Workflow" => "工作流",
            "Plan" => "计划",
            "Note" => "注意",
            _ => label,
        },
        _ => label,
    }
}

fn localize_preview_shell_line<'a>(tool_name: &str, line: &'a str, locale: Locale) -> &'a str {
    match tool_name {
        "write_file" if line == "proposed content" => localize_detail_label(line, locale),
        "edit_file" if matches!(line, "replace this" | "with this") => {
            localize_detail_label(line, locale)
        }
        _ => line,
    }
}

pub(crate) fn format_shell_command_for_approval(command: &str) -> Vec<String> {
    if let Some(preview) = parse_printf_write_file_command(command) {
        return format_printf_write_file_preview(preview);
    }

    let mut out = Vec::new();
    for raw_line in command.lines() {
        split_shell_display_line(raw_line, &mut out);
    }
    if out.is_empty() && !command.trim().is_empty() {
        out.push(command.trim().to_string());
    }
    out
}

fn split_shell_display_line(line: &str, out: &mut Vec<String>) {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut current = String::new();
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        if ch == '\\' {
            current.push(ch);
            escaped = true;
            continue;
        }

        if matches!(ch, '"' | '\'') {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            }
            current.push(ch);
            continue;
        }

        if quote.is_none() {
            match ch {
                '&' if chars.peek() == Some(&'&') => {
                    chars.next();
                    push_shell_clause(out, &mut current, Some("&&"));
                    continue;
                }
                '|' if chars.peek() == Some(&'|') => {
                    chars.next();
                    push_shell_clause(out, &mut current, Some("||"));
                    continue;
                }
                '|' => {
                    push_shell_clause(out, &mut current, Some("|"));
                    continue;
                }
                ';' => {
                    push_shell_clause(out, &mut current, Some(";"));
                    continue;
                }
                _ => {}
            }
        }

        current.push(ch);
    }

    push_shell_clause(out, &mut current, None);
}

fn push_shell_clause(out: &mut Vec<String>, current: &mut String, operator: Option<&str>) {
    let trimmed = current.trim();
    if trimmed.is_empty() {
        if let Some(operator) = operator {
            out.push(operator.to_string());
        }
    } else if let Some(operator) = operator {
        out.push(format!("{trimmed} {operator}"));
    } else {
        out.push(trimmed.to_string());
    }
    current.clear();
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    if tokens.first()?.as_str() != "printf" {
        return None;
    }
    let target_parts = shlex::split(after_redirect.trim())?;
    if target_parts.len() != 1 {
        return None;
    }
    let target = target_parts
        .into_iter()
        .next()?
        .trim_matches(|ch| ch == '"' || ch == '\'')
        .to_string();
    if target.is_empty() {
        return None;
    }

    let args = &tokens[1..];
    if args.is_empty() {
        return None;
    }
    let values = if args.len() >= 2 && args[0].contains('%') {
        &args[1..]
    } else {
        args
    };
    let mut lines = Vec::new();
    for value in values {
        let normalized = value.replace("\\n", "\n");
        for line in normalized.lines() {
            lines.push(line.to_string());
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }

    Some(PrintfWriteFilePreview { target, lines })
}

fn format_printf_write_file_preview(preview: PrintfWriteFilePreview) -> Vec<String> {
    const MAX_PREVIEW_LINES: usize = 12;
    let mut out = vec![format!("printf > {}", preview.target)];
    let total = preview.lines.len();
    for line in preview.lines.into_iter().take(MAX_PREVIEW_LINES) {
        out.push(format!("  {line}"));
    }
    if total > MAX_PREVIEW_LINES {
        out.push(format!("  ... (+{} more lines)", total - MAX_PREVIEW_LINES));
    }
    out
}

fn split_unquoted_redirect(command: &str) -> Option<(&str, &str)> {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in command.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if matches!(ch, '"' | '\'') {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            }
            continue;
        }
        if quote.is_none() && ch == '>' {
            return Some((&command[..idx], &command[idx + ch.len_utf8()..]));
        }
    }
    None
}

/// Indices into the option list shared by both variants.
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

    /// Commit the given option and close the approval modal.
    fn commit_option(&mut self, option: ApprovalOption) -> ViewAction {
        self.selected = option.index();
        self.emit_decision(option.decision(), false)
    }

    fn emit_decision(&self, decision: ReviewDecision, timed_out: bool) -> ViewAction {
        self.emit_decision_with_rules(decision, timed_out, Vec::new())
    }

    fn emit_decision_with_rules(
        &self,
        decision: ReviewDecision,
        timed_out: bool,
        persistent_ask_rules: Vec<ToolAskRule>,
    ) -> ViewAction {
        ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
            tool_id: self.request.id.clone(),
            tool_name: self.request.tool_name.clone(),
            decision,
            timed_out,
            approval_key: self.request.approval_key.clone(),
            approval_grouping_key: self.request.approval_grouping_key.clone(),
            persistent_ask_rules,
        })
    }

    fn emit_params_pager(&self) -> ViewAction {
        // The compact prompt keeps the about/impact dossier out of the
        // default band; the pager is where that context now lives.
        let locale = self.locale();
        let (about_label, impact_label) = match locale {
            Locale::ZhHans => ("说明：", "影响："),
            _ => ("About: ", "Impact: "),
        };
        let mut content = String::new();
        content.push_str(about_label);
        content.push_str(&self.request.description_for_locale(locale));
        content.push('\n');
        for impact in self.request.impacts_for_locale(locale) {
            content.push_str(impact_label);
            content.push_str(&impact);
            content.push('\n');
        }
        content.push('\n');
        content.push_str(
            &serde_json::to_string_pretty(&self.request.params)
                .unwrap_or_else(|_| self.request.params.to_string()),
        );
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
            KeyCode::Enter => self.commit_option(self.current_option()),
            // Direct shortcuts; '1' / '2' map to the first two options
            // so a numeric pad still works for approve flows.
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('1') => {
                self.commit_option(ApprovalOption::ApproveOnce)
            }
            KeyCode::Char('a') | KeyCode::Char('A') | KeyCode::Char('2') => {
                self.commit_option(ApprovalOption::ApproveAlways)
            }
            KeyCode::Char('s') | KeyCode::Char('S') if self.request.can_save_ask_rule() => self
                .emit_decision_with_rules(
                    ReviewDecision::Approved,
                    false,
                    self.request.persistent_ask_rules.clone(),
                ),
            KeyCode::Char('n')
            | KeyCode::Char('N')
            | KeyCode::Char('d')
            | KeyCode::Char('D')
            | KeyCode::Char('3') => self.commit_option(ApprovalOption::Deny),
            KeyCode::Char('v') | KeyCode::Char('V') => self.emit_params_pager(),
            KeyCode::Esc => self.emit_decision(ReviewDecision::Abort, false),
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        let approval_widget = ApprovalWidget::new(&self.request, self);
        approval_widget.render(area, buf);
    }

    fn occupied_region(&self, area: ratatui::layout::Rect) -> ratatui::layout::Rect {
        // The approval is an inline, bottom-anchored prompt: it only occupies
        // a band at the bottom of the frame so the backdrop dims that band and
        // the transcript above stays visible. Must match what `render` paints.
        ApprovalWidget::new(&self.request, self).inline_region(area)
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
    #[cfg(test)]
    pub fn label(&self) -> &'static str {
        match self {
            ElevationOption::WithNetwork => "Allow outbound network",
            ElevationOption::WithWriteAccess(_) => "Allow extra write access",
            ElevationOption::FullAccess => "Full access (filesystem + network)",
            ElevationOption::Abort => "Abort",
        }
    }

    /// Get a short description.
    #[cfg(test)]
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
    locale: Locale,
}

impl ElevationView {
    pub fn new(request: ElevationRequest, locale: Locale) -> Self {
        Self {
            request,
            selected: 0,
            locale,
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
        let elevation_widget = ElevationWidget::new(&self.request, self.selected, self.locale);
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

    fn critical_request() -> ApprovalRequest {
        ApprovalRequest::new(
            "test-id",
            "exec_shell",
            "Run a shell command",
            &json!({"command": "rm -rf ~/"}),
            "tool:exec_shell",
        )
    }

    fn shell_request() -> ApprovalRequest {
        ApprovalRequest::new(
            "test-id",
            "exec_shell",
            "Run a shell command",
            &json!({"command": "cargo test --workspace"}),
            "tool:exec_shell",
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
        assert_eq!(get_tool_category("task_shell_wait"), ToolCategory::Shell);
        assert_eq!(get_tool_category("exec_shell_wait"), ToolCategory::Shell);
        assert_eq!(
            get_tool_category("exec_shell_interact"),
            ToolCategory::Shell
        );
        assert_eq!(get_tool_category("exec_wait"), ToolCategory::Shell);
        assert_eq!(get_tool_category("exec_interact"), ToolCategory::Shell);
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
        // wait_for_dev_server only permits loopback targets.
        assert_eq!(
            classify_risk("wait_for_dev_server", cat, &json!({"port": 5173})),
            RiskLevel::Benign
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
    fn risk_read_only_shell_commands_route_benign() {
        let cat = ToolCategory::Shell;
        for command in [
            "codewhale --version",
            "codewhale --help",
            "git status --porcelain",
        ] {
            assert_eq!(
                classify_risk("exec_shell", cat, &json!({ "command": command })),
                RiskLevel::Benign,
                "expected read-only shell command {command:?} to be Benign",
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
                .any(|line| line.contains("Executes a Bash command"))
        );
        assert!(
            request
                .impacts
                .iter()
                .all(|line| !line.contains("cargo test")),
            "command detail should not be duplicated in the impact summary"
        );
        let details = request.prominent_detail_items(Locale::En);
        assert!(
            details
                .iter()
                .any(|detail| detail.label == "Command" && detail.value.contains("cargo test"))
        );
    }

    #[test]
    fn mcp_impact_summary_preserves_full_target_for_underscored_names() {
        let request = ApprovalRequest::new(
            "test-id",
            "mcp_my_db_execute_sql",
            "Call an MCP tool",
            &json!({}),
            "tool:mcp_my_db_execute_sql",
        );

        assert!(
            request
                .impacts
                .iter()
                .any(|line| line == "MCP target: my_db_execute_sql")
        );
        assert!(!request.impacts.iter().any(|line| line == "Server: my"));

        let zh_impacts = request.impacts_for_locale(Locale::ZhHans);
        assert!(
            zh_impacts
                .iter()
                .any(|line| line == "MCP 目标：my_db_execute_sql")
        );
        assert!(!zh_impacts.iter().any(|line| line == "服务器：my"));
    }

    #[test]
    fn test_prominent_details_shell_does_not_truncate_long_command() {
        let command = format!("printf '{}\\n' > /tmp/x && cat /tmp/x", "x".repeat(300));
        let request = ApprovalRequest::new(
            "test-id",
            "exec_shell",
            "Run a shell command",
            &json!({"command": command, "cwd": "/tmp/project"}),
            "test_key",
        );

        let details = request.prominent_detail_items(Locale::En);

        assert_eq!(details[0].label, "Command");
        assert_eq!(details[0].value, command);
        assert!(
            details[0]
                .shell_lines
                .as_ref()
                .is_some_and(|lines| lines.iter().any(|line| line.contains("cat /tmp/x"))),
            "shell preview should preserve the dangerous tail of long commands"
        );
        assert_eq!(details[1].label, "Dir");
        assert_eq!(details[1].value, "/tmp/project");
    }

    #[test]
    fn test_prominent_details_file_write() {
        let request = ApprovalRequest::new(
            "test-id",
            "write_file",
            "Write a file to disk",
            &json!({"path": "src/main.rs", "content": "fn main() {}"}),
            "test_key",
        );

        let details = request.prominent_detail_items(Locale::En);

        assert_eq!(details[0].label, "File");
        assert_eq!(details[0].value, "src/main.rs");
        assert!(details[0].shell_lines.is_none());
        assert_eq!(details[1].label, "Preview");
        let preview = details[1].shell_lines.as_ref().expect("preview lines");
        assert!(preview.iter().any(|line| line == "+ fn main() {}"));
    }

    // ── whaleflow: rich dynamic-workflow approval prompt ───────────────

    fn whaleflow_request(script: &str, label: Option<&str>) -> ApprovalRequest {
        let mut params = json!({ "script": script });
        if let Some(label) = label {
            params["label"] = json!(label);
        }
        ApprovalRequest::new(
            "wfcall_test",
            "whaleflow",
            "Run a dynamic workflow?",
            &params,
            "tool:whaleflow:key",
        )
    }

    #[test]
    fn whaleflow_categorizes_as_agent_destructive_elevated() {
        assert_eq!(get_tool_category("whaleflow"), ToolCategory::Agent);
        let request = whaleflow_request("await task({ description: \"x\" });", Some("fanout"));
        assert_eq!(request.category, ToolCategory::Agent);
        assert_eq!(
            request.risk,
            RiskLevel::Destructive,
            "no `action` param → explicit-options keymap"
        );
        assert_eq!(
            request.stakes(),
            ApprovalStakes::Elevated,
            "a calm approval card, not a red emergency"
        );
        assert_eq!(
            request.impacts,
            vec![
                "Runs a sandboxed orchestration script.".to_string(),
                "May spawn many sub-agents and consume token budget.".to_string(),
            ]
        );
    }

    #[test]
    fn whaleflow_prominent_details_show_workflow_plan_preview_and_note() {
        let script = "// staged review\n\
                      const a = await task({ description: \"a\", profile: \"reviewer\" });\n\
                      await parallel([() => task({ description: \"b\" })]);\n\
                      const c = await tools.read_file({ path: \"x\" });\n\
                      await tools.exec_shell({ command: \"cargo test\" });\n\
                      const d = 1;\n\
                      const e = 2;\n\
                      return c;";
        let request = whaleflow_request(script, Some("review train"));
        let details = request.prominent_detail_items(Locale::En);

        let labels: Vec<&str> = details.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(labels, vec!["Workflow", "Plan", "Preview", "Note"]);

        assert_eq!(details[0].value, "review train");

        let plan = details[1].shell_lines.as_ref().expect("plan lines");
        assert!(
            plan.iter()
                .any(|line| line.contains("task() sites") && line.contains("parallel()")),
            "plan must count spawn call sites: {plan:?}"
        );
        assert!(
            plan.iter().any(|line| line.contains("profiles: reviewer")),
            "plan must list referenced profiles: {plan:?}"
        );
        assert!(
            plan.iter()
                .any(|line| line.contains("read_file") && line.contains("exec_shell")),
            "plan must list referenced tools: {plan:?}"
        );
        // The static scan is a heuristic (aliased/computed calls evade it):
        // the plan must say so, always.
        assert!(
            plan.iter()
                .any(|line| line
                    .contains("Static preview — aliased or computed calls may not appear")),
            "plan must be labeled a heuristic preview: {plan:?}"
        );
        // The script's own comment appears only as a clearly-quoted
        // untrusted line, never in the system voice.
        assert!(
            plan.iter()
                .any(|line| line == "Script's own summary (untrusted): \"staged review\""),
            "leading comment must be quoted and attributed: {plan:?}"
        );

        let preview = details[2].shell_lines.as_ref().expect("preview lines");
        assert_eq!(preview.first().map(String::as_str), Some("script"));
        assert!(
            preview
                .last()
                .is_some_and(|line| line.starts_with("... (+") && line.ends_with("more lines)")),
            "long scripts must render a bounded preview with a truncation tail: {preview:?}"
        );

        // The capability warning renders regardless of what the scan found.
        assert_eq!(
            details[3].value,
            "May spawn sub-agents and call any tool available to this context; \
             normal approval rules apply to each call."
        );

        // Evasion case: aliasing hides the shell call from the scan, but the
        // capability warning and the heuristic label still render.
        let evasive = whaleflow_request(
            "const t = tools; await t.exec_shell({ command: \"ls\" });",
            None,
        );
        let evasive_details = evasive.prominent_detail_items(Locale::En);
        let evasive_plan = evasive_details[1].shell_lines.as_ref().expect("plan lines");
        assert!(
            evasive_plan
                .iter()
                .any(|line| line.contains("no direct task()/tools.* references found")),
            "an evaded scan claims only DIRECT references: {evasive_plan:?}"
        );
        assert!(
            evasive_plan
                .iter()
                .any(|line| line
                    .contains("Static preview — aliased or computed calls may not appear")),
            "the heuristic label survives an evaded scan: {evasive_plan:?}"
        );
        assert!(
            evasive_details[3]
                .value
                .contains("call any tool available to this context"),
            "the capability warning is unconditional: {:?}",
            evasive_details[3].value
        );
    }

    #[test]
    fn whaleflow_unlabeled_script_and_pager_carries_full_script() {
        let script = "const marker = \"WHALEFLOW_PAGER_MARKER_LINE\";\nreturn marker;";
        let request = whaleflow_request(script, None);
        let details = request.prominent_detail_items(Locale::En);
        assert_eq!(details[0].label, "Workflow");
        assert_eq!(details[0].value, "(unlabeled script)");

        // The full script stays one `v` away in the params pager.
        let view = ApprovalView::new(request);
        match view.emit_params_pager() {
            ViewAction::Emit(ViewEvent::OpenTextPager { content, .. }) => {
                assert!(
                    content.contains("WHALEFLOW_PAGER_MARKER_LINE"),
                    "pager must contain the entire script: {content}"
                );
            }
            other => panic!("expected pager emit, got {other:?}"),
        }
    }

    #[test]
    fn prominent_details_edit_file_includes_search_replace_preview() {
        let request = ApprovalRequest::new(
            "test-id",
            "edit_file",
            "Edit a file on disk",
            &json!({
                "path": "src/lib.rs",
                "search": "old_call();",
                "replace": "new_call();"
            }),
            "tool:edit_file",
        );

        let details = request.prominent_detail_items(Locale::En);
        let preview = details
            .iter()
            .find(|detail| detail.label == "Preview")
            .and_then(|detail| detail.shell_lines.as_ref())
            .expect("edit preview");

        assert!(preview.iter().any(|line| line == "- old_call();"));
        assert!(preview.iter().any(|line| line == "+ new_call();"));
    }

    #[test]
    fn prominent_details_apply_patch_includes_diff_preview() {
        let patch = r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,2 @@
-old
+new
"#;
        let request = ApprovalRequest::new(
            "test-id",
            "apply_patch",
            "Apply a patch",
            &json!({"patch": patch}),
            "tool:apply_patch",
        );

        let details = request.prominent_detail_items(Locale::En);
        let preview = details
            .iter()
            .find(|detail| detail.label == "Preview")
            .and_then(|detail| detail.shell_lines.as_ref())
            .expect("patch preview");

        assert!(preview.iter().any(|line| line.starts_with("@@")));
        assert!(preview.iter().any(|line| line == "-old"));
        assert!(preview.iter().any(|line| line == "+new"));
    }

    #[test]
    fn prominent_details_apply_patch_changes_array_preview_stays_bounded() {
        let request = ApprovalRequest::new(
            "test-id",
            "apply_patch",
            "Apply a patch",
            &json!({
                "changes": [
                    {
                        "path": "src/lib.rs",
                        "content": "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight"
                    },
                    {
                        "path": "src/main.rs",
                        "content": "main"
                    },
                    {
                        "path": "src/extra.rs",
                        "content": "extra"
                    }
                ]
            }),
            "tool:apply_patch",
        );

        let details = request.prominent_detail_items(Locale::En);
        let preview = details
            .iter()
            .find(|detail| detail.label == "Preview")
            .and_then(|detail| detail.shell_lines.as_ref())
            .expect("changes preview");

        assert!(
            preview.len() <= 7,
            "preview should stay bounded: {preview:?}"
        );
        assert!(preview.iter().any(|line| line == "file: src/lib.rs"));
        assert_eq!(
            preview.last().map(String::as_str),
            Some("... (+2 more files)")
        );
    }

    #[test]
    fn apply_patch_changes_array_preview_reports_second_file_when_first_fills_buffer() {
        let request = ApprovalRequest::new(
            "test-id",
            "apply_patch",
            "Apply a patch",
            &json!({
                "changes": [
                    {
                        "path": "src/lib.rs",
                        "content": "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight"
                    },
                    {
                        "path": "src/main.rs",
                        "content": "main"
                    }
                ]
            }),
            "tool:apply_patch",
        );

        let details = request.prominent_detail_items(Locale::En);
        let preview = details
            .iter()
            .find(|detail| detail.label == "Preview")
            .and_then(|detail| detail.shell_lines.as_ref())
            .expect("changes preview");

        assert!(
            preview.len() <= 7,
            "preview should stay bounded: {preview:?}"
        );
        assert!(preview.iter().any(|line| line == "file: src/lib.rs"));
        assert_eq!(
            preview.last().map(String::as_str),
            Some("... (+1 more files)")
        );
    }

    #[test]
    fn apply_patch_preview_counts_omitted_context_lines() {
        let patch = r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,8 +1,8 @@
 context one
 context two
-old
+new
 context three
 context four
 context five
"#;

        let preview = apply_patch_preview_lines(patch).expect("patch preview");

        assert!(
            preview.len() <= 7,
            "preview should stay bounded: {preview:?}"
        );
        assert_eq!(
            preview.last().map(String::as_str),
            Some("... (+5 more patch lines)")
        );
    }

    #[test]
    fn apply_patch_preview_counts_replaced_visible_line_as_omitted() {
        let patch = r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,4 @@
-old1
+new1
-old2
+new2
 context one
 context two
"#;

        let preview = apply_patch_preview_lines(patch).expect("patch preview");

        assert_eq!(preview.len(), 7);
        assert_eq!(
            preview.last().map(String::as_str),
            Some("... (+4 more patch lines)")
        );
    }

    #[test]
    fn preview_sublabels_are_localized_for_zh_hans() {
        let write = ApprovalRequest::new(
            "test-id",
            "write_file",
            "Write a file",
            &json!({"path": "src/lib.rs", "content": "proposed content\nreplacement content"}),
            "tool:write_file",
        );
        let write_preview = write
            .prominent_detail_items(Locale::ZhHans)
            .into_iter()
            .find(|detail| detail.label == "预览")
            .and_then(|detail| detail.shell_lines)
            .expect("localized write preview");
        assert!(write_preview.iter().any(|line| line == "拟写入内容"));
        assert!(
            write_preview
                .iter()
                .any(|line| line == "+ proposed content")
        );
        assert!(
            write_preview
                .iter()
                .any(|line| line == "+ replacement content")
        );

        let edit = ApprovalRequest::new(
            "test-id",
            "edit_file",
            "Edit a file",
            &json!({
                "path": "src/lib.rs",
                "search": "with this",
                "replace": "replace this"
            }),
            "tool:edit_file",
        );
        let edit_preview = edit
            .prominent_detail_items(Locale::ZhHans)
            .into_iter()
            .find(|detail| detail.label == "预览")
            .and_then(|detail| detail.shell_lines)
            .expect("localized edit preview");
        assert!(edit_preview.iter().any(|line| line == "替换此内容"));
        assert!(edit_preview.iter().any(|line| line == "替换为"));
        assert!(edit_preview.iter().any(|line| line == "- with this"));
        assert!(edit_preview.iter().any(|line| line == "+ replace this"));
    }

    #[test]
    fn test_shell_formatter_preserves_logical_or_operator() {
        let lines = format_shell_command_for_approval("cargo build || echo fallback");

        assert_eq!(lines, vec!["cargo build ||", "echo fallback"]);
    }

    #[test]
    fn test_shell_formatter_detects_printf_write_file_preview() {
        let lines =
            format_shell_command_for_approval("printf '%s\\n' 'hello' 'world' > src/main.rs");

        assert_eq!(lines[0], "printf > src/main.rs");
        assert!(lines.iter().any(|line| line.contains("hello")));
        assert!(lines.iter().any(|line| line.contains("world")));
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
    fn exec_shell_request_builds_ask_rule_preview() {
        let request = shell_request();

        assert_eq!(
            request.persistent_ask_rules,
            vec![ToolAskRule::exec_shell("cargo test --workspace")]
        );
        let preview = request.ask_rule_preview().expect("preview");
        assert!(preview.contains("[[rules]]"));
        assert!(preview.contains("tool = \"exec_shell\""));
        assert!(preview.contains("command = \"cargo test --workspace\""));
    }

    #[test]
    fn ask_rule_save_preview_formats_shell_rule() {
        let request = shell_request();

        let preview = request.ask_rule_save_preview().expect("save preview");
        assert_eq!(preview.rule_count, 1);
        assert_eq!(preview.summary(), "1 ask rule");
        assert_eq!(
            preview.entries,
            vec!["tool=exec_shell command=cargo test --workspace"]
        );
        assert_eq!(preview.omitted, 0);
    }

    #[test]
    fn file_ask_rule_saved_for_write_file_approval() {
        // A write_file approval offers an exact, workspace-relative file rule
        // plus a preview so `S` can persist it.
        let request = destructive_request();

        assert_eq!(
            request.persistent_ask_rules,
            vec![ToolAskRule::file_path("write_file", "src/main.rs")]
        );
        assert!(request.can_save_ask_rule());
        let preview = request.ask_rule_preview().expect("preview");
        assert!(preview.contains("[[rules]]"));
        assert!(preview.contains("tool = \"write_file\""));
        assert!(preview.contains("path = \"src/main.rs\""));
    }

    #[test]
    fn ask_rule_save_preview_formats_write_and_edit_file_paths() {
        let write = destructive_request();
        let edit = ApprovalRequest::new(
            "test-id",
            "edit_file",
            "Edit a file on disk",
            &json!({"path": "/workspace/src/lib.rs"}),
            "tool:edit_file",
        );

        assert_eq!(
            write
                .ask_rule_save_preview()
                .expect("write save preview")
                .entries,
            vec!["tool=write_file path=src/main.rs"]
        );
        assert_eq!(
            edit.ask_rule_save_preview()
                .expect("edit save preview")
                .entries,
            vec!["tool=edit_file path=src/lib.rs"]
        );
    }

    #[test]
    fn file_ask_rule_normalizes_absolute_edit_file_path_to_workspace_relative() {
        // An absolute in-workspace path is stored in the workspace-relative
        // form, matching how runtime ask-rule matching normalizes paths.
        let request = ApprovalRequest::new(
            "test-id",
            "edit_file",
            "Edit a file on disk",
            &json!({"path": "/workspace/src/lib.rs"}),
            "tool:edit_file",
        );

        assert_eq!(
            request.persistent_ask_rules,
            vec![ToolAskRule::file_path("edit_file", "src/lib.rs")]
        );
    }

    #[test]
    fn read_file_request_has_no_file_ask_rule() {
        // The save boundary is write approvals only; read_file never offers a
        // persistent rule.
        let request = benign_request();

        assert!(request.persistent_ask_rules.is_empty());
        assert!(!request.can_save_ask_rule());
        assert_eq!(request.ask_rule_preview(), None);
        assert_eq!(request.ask_rule_save_preview(), None);
    }

    #[test]
    fn file_ask_rule_skipped_for_unsafe_empty_or_external_paths() {
        // Traversal, empty, and outside-workspace paths must not become rules,
        // so the preview and `S` shortcut stay disabled.
        for path in ["../escape.rs", "/etc/passwd", "   ", ""] {
            let request = ApprovalRequest::new(
                "test-id",
                "write_file",
                "Write a file to disk",
                &json!({"path": path}),
                "tool:write_file",
            );
            assert!(
                request.persistent_ask_rules.is_empty(),
                "path {path:?} must not produce a rule"
            );
            assert!(!request.can_save_ask_rule());
            assert_eq!(request.ask_rule_preview(), None);
            assert_eq!(request.ask_rule_save_preview(), None);
        }
    }

    #[test]
    fn apply_patch_ask_rules_saved_for_multi_file_patch() {
        let patch = r"diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,1 +1,1 @@
-old
+new
diff --git a/src/b.rs b/src/b.rs
--- a/src/b.rs
+++ b/src/b.rs
@@ -1,1 +1,1 @@
-old
+new
";

        let request = ApprovalRequest::new(
            "test-id",
            "apply_patch",
            "Apply a patch",
            &json!({"patch": patch}),
            "tool:apply_patch",
        );

        assert_eq!(
            request.persistent_ask_rules,
            vec![
                ToolAskRule::file_path("apply_patch", "src/a.rs"),
                ToolAskRule::file_path("apply_patch", "src/b.rs"),
            ]
        );
        assert!(request.can_save_ask_rule());
        let preview = request.ask_rule_save_preview().expect("save preview");
        assert_eq!(preview.summary(), "2 ask rules");
        assert_eq!(
            preview.entries,
            vec![
                "tool=apply_patch path=src/a.rs",
                "tool=apply_patch path=src/b.rs"
            ]
        );
    }

    #[test]
    fn apply_patch_ask_rules_dedupe_targets_after_normalization() {
        let request = ApprovalRequest::new(
            "test-id",
            "apply_patch",
            "Apply a patch",
            &json!({
                "changes": [
                    { "path": "src/a.rs", "content": "one" },
                    { "path": "/workspace/src/a.rs", "content": "two" }
                ]
            }),
            "tool:apply_patch",
        );

        assert_eq!(
            request.persistent_ask_rules,
            vec![ToolAskRule::file_path("apply_patch", "src/a.rs")]
        );
    }

    #[test]
    fn apply_patch_ask_rule_handles_timestamp_headers() {
        let patch = "diff --git a/src/lib.rs b/src/lib.rs\n\
--- a/src/lib.rs\t2026-06-26 10:00:00 +0000\n\
+++ b/src/lib.rs\t2026-06-26 10:01:00 +0000\n\
@@ -1,1 +1,1 @@\n\
-old\n\
+new\n";

        let request = ApprovalRequest::new(
            "test-id",
            "apply_patch",
            "Apply a patch",
            &json!({"patch": patch}),
            "tool:apply_patch",
        );

        assert_eq!(
            request.persistent_ask_rules,
            vec![ToolAskRule::file_path("apply_patch", "src/lib.rs")]
        );
    }

    #[test]
    fn apply_patch_ask_rule_ignores_forged_headers_inside_hunk() {
        let patch = r"--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,3 @@
 line1
--- a/forged.rs
+++ b/forged.rs
 line3
";

        let request = ApprovalRequest::new(
            "test-id",
            "apply_patch",
            "Apply a patch",
            &json!({"path": "src/lib.rs", "patch": patch}),
            "tool:apply_patch",
        );

        assert_eq!(
            request.persistent_ask_rules,
            vec![ToolAskRule::file_path("apply_patch", "src/lib.rs")]
        );
    }

    #[test]
    fn apply_patch_ask_rule_skipped_when_any_target_traverses_workspace() {
        let request = ApprovalRequest::new(
            "test-id",
            "apply_patch",
            "Apply a patch",
            &json!({
                "changes": [
                    { "path": "src/a.rs", "content": "safe" },
                    { "path": "../escape.rs", "content": "unsafe" }
                ]
            }),
            "tool:apply_patch",
        );

        assert!(request.persistent_ask_rules.is_empty());
        assert!(!request.can_save_ask_rule());
        assert_eq!(request.ask_rule_save_preview(), None);
    }

    #[test]
    fn apply_patch_ask_rule_skipped_on_preflight_failure() {
        let request = ApprovalRequest::new(
            "test-id",
            "apply_patch",
            "Apply a patch",
            &json!({"patch": "@@ -1 +1 @@\n-old\n+new\n"}),
            "tool:apply_patch",
        );

        assert!(request.persistent_ask_rules.is_empty());
        assert_eq!(request.ask_rule_preview(), None);
        assert_eq!(request.ask_rule_save_preview(), None);
    }

    #[test]
    fn ask_rule_save_preview_truncates_rule_list() {
        let rules = vec![
            ToolAskRule::file_path("apply_patch", "src/a.rs"),
            ToolAskRule::file_path("apply_patch", "src/b.rs"),
            ToolAskRule::file_path("apply_patch", "src/c.rs"),
            ToolAskRule::file_path("apply_patch", "src/d.rs"),
        ];

        let preview = build_ask_rule_save_preview(&rules, 2).expect("save preview");
        assert_eq!(preview.rule_count, 4);
        assert_eq!(preview.summary(), "4 ask rules");
        assert_eq!(
            preview.entries,
            vec![
                "tool=apply_patch path=src/a.rs",
                "tool=apply_patch path=src/b.rs"
            ]
        );
        assert_eq!(preview.omitted, 2);
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
    fn save_ask_rule_shortcut_approves_once_with_rule() {
        let mut view = ApprovalView::new(shell_request());

        let action = view.handle_key(create_key_event(KeyCode::Char('s')));
        let ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
            decision,
            persistent_ask_rules,
            ..
        }) = action
        else {
            panic!("expected approval decision");
        };

        assert_eq!(decision, ReviewDecision::Approved);
        assert_eq!(
            persistent_ask_rules,
            vec![ToolAskRule::exec_shell("cargo test --workspace")]
        );
    }

    #[test]
    fn save_file_ask_rule_shortcut_emits_file_rule() {
        // `S` on a write_file approval approves once and carries the exact
        // workspace-relative file rule for persistence.
        let mut view = ApprovalView::new(destructive_request());

        let action = view.handle_key(create_key_event(KeyCode::Char('S')));
        let ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
            decision,
            persistent_ask_rules,
            ..
        }) = action
        else {
            panic!("expected approval decision");
        };

        assert_eq!(decision, ReviewDecision::Approved);
        assert_eq!(
            persistent_ask_rules,
            vec![ToolAskRule::file_path("write_file", "src/main.rs")]
        );
    }

    #[test]
    fn save_ask_rule_shortcut_is_ignored_without_rule() {
        let mut view = ApprovalView::new(benign_request());

        let action = view.handle_key(create_key_event(KeyCode::Char('s')));

        assert!(matches!(action, ViewAction::None));
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

        // Selection starts at ApproveOnce — Enter commits the selected option.
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

        view.handle_key(create_key_event(KeyCode::Down));
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

    fn assert_approval_key_badges_visible(joined: &str) {
        for badge in ["[1 / y]", "[2 / a]", "[3 / d / n]", "[Esc]"] {
            assert!(
                joined.contains(badge),
                "missing key badge {badge}:\n{joined}"
            );
        }
    }

    #[test]
    fn web_run_risk_is_param_aware() {
        // search/query is benign; open/click fetch arbitrary URLs -> destructive.
        assert_eq!(
            classify_risk("web_run", ToolCategory::Network, &json!({"search": "rust"})),
            RiskLevel::Benign
        );
        assert_eq!(
            classify_risk(
                "web_run",
                ToolCategory::Network,
                &json!({"open": [{"ref": "https://evil.example"}]})
            ),
            RiskLevel::Destructive
        );
        assert_eq!(
            classify_risk(
                "web_run",
                ToolCategory::Network,
                &json!({"click": [{"ref": "1"}]})
            ),
            RiskLevel::Destructive
        );
    }

    #[test]
    fn stakes_split_routine_elevated_critical() {
        assert_eq!(benign_request().stakes(), ApprovalStakes::Routine);
        assert_eq!(destructive_request().stakes(), ApprovalStakes::Elevated);
        assert_eq!(shell_request().stakes(), ApprovalStakes::Elevated);
        assert_eq!(critical_request().stakes(), ApprovalStakes::Critical);
        // Publish-like shell is critical in every origin.
        let publish = ApprovalRequest::new(
            "test-id",
            "exec_shell",
            "Run a shell command",
            &json!({"command": "git push origin main"}),
            "tool:exec_shell",
        );
        assert_eq!(publish.stakes(), ApprovalStakes::Critical);
    }

    #[test]
    fn agent_tool_is_classified_and_renders_calm() {
        assert_eq!(get_tool_category("agent"), ToolCategory::Agent);

        let request = ApprovalRequest::new(
            "test-id",
            "agent",
            "Start a sub-agent",
            &json!({"action": "start", "type": "explore", "prompt": "map the workspace"}),
            "tool:agent",
        );
        assert_eq!(request.category, ToolCategory::Agent);
        assert_eq!(request.stakes(), ApprovalStakes::Elevated);

        let view = ApprovalView::new(request);
        let lines = render_lines(&view, 100, 40);
        let joined = lines.join("\n");
        assert!(joined.contains("APPROVAL"), "{joined}");
        assert!(!joined.contains("DESTRUCTIVE"), "{joined}");
        assert!(
            !joined.contains("not classified"),
            "agent must not render the unknown-tool warning:\n{joined}"
        );
        assert!(joined.contains("Action"), "{joined}");
        assert!(joined.contains("start"), "{joined}");
        assert!(joined.contains("explore"), "{joined}");
        assert!(joined.contains("map the workspace"), "{joined}");
    }

    #[test]
    fn agent_status_and_peek_are_benign() {
        for action in ["status", "peek", "list"] {
            let request = ApprovalRequest::new(
                "test-id",
                "agent",
                "Inspect a sub-agent",
                &json!({"action": action, "agent_id": "agent_1"}),
                "tool:agent",
            );
            assert_eq!(request.risk, RiskLevel::Benign, "{action}");
            assert_eq!(request.stakes(), ApprovalStakes::Routine, "{action}");
        }
    }

    #[test]
    fn render_benign_includes_review_badge_and_selection_hint() {
        let view = ApprovalView::new(benign_request());
        let lines = render_lines(&view, 100, 40);
        let joined = lines.join("\n");
        assert!(joined.contains("REVIEW"), "missing REVIEW badge:\n{joined}");
        assert_approval_key_badges_visible(&joined);
        assert!(joined.contains("Choose"), "benign hint missing:\n{joined}");
        assert!(
            joined.contains("Enter selected option"),
            "benign selection hint missing:\n{joined}"
        );
        assert!(joined.contains("read_file"));
    }

    #[test]
    fn render_elevated_write_is_calm_and_compact() {
        // Ordinary state-touching work (a file write) renders as a calm
        // APPROVAL ask: no DESTRUCTIVE badge, no policy dossier, no
        // impact/category taxonomy — that detail stays one `v` away.
        let view = ApprovalView::new(destructive_request());
        let lines = render_lines(&view, 100, 40);
        let joined = lines.join("\n");
        assert!(joined.contains("APPROVAL"), "missing calm badge:\n{joined}");
        assert!(
            !joined.contains("DESTRUCTIVE"),
            "routine write must not scream DESTRUCTIVE:\n{joined}"
        );
        assert_approval_key_badges_visible(&joined);
        assert!(
            joined.contains("Enter selected option"),
            "selection hint missing:\n{joined}"
        );
        assert!(
            !joined.contains("active approval policy"),
            "policy prose is critical-only:\n{joined}"
        );
        assert!(
            !joined.contains("Impact:"),
            "impact dossier is critical-only:\n{joined}"
        );
        assert!(
            !joined.contains("Type:"),
            "category taxonomy is critical-only:\n{joined}"
        );
        assert!(joined.contains("write_file"));
    }

    #[test]
    fn render_critical_shows_warning_badge_and_policy_semantics() {
        // Genuinely destructive work keeps the strong styling and the
        // policy/cancel semantics.
        let view = ApprovalView::new(critical_request());
        let lines = render_lines(&view, 100, 40);
        let joined = lines.join("\n");
        assert!(
            joined.contains("DESTRUCTIVE"),
            "missing DESTRUCTIVE badge:\n{joined}"
        );
        assert_approval_key_badges_visible(&joined);
        assert!(
            joined.contains("active approval policy"),
            "missing policy/review-rule semantics:\n{joined}"
        );
        assert!(
            joined.contains("Deny rejects only this tool call"),
            "missing deny-vs-abort semantics:\n{joined}"
        );
        assert!(joined.contains("rm -rf"));
    }

    #[test]
    fn render_elevated_zh_hans_is_calm_and_localized() {
        let view = ApprovalView::new_for_locale(destructive_request(), Locale::ZhHans);
        let lines = render_lines(&view, 100, 40);
        let joined = compact_rendered_text(&lines);
        assert!(
            joined.contains("需要批准"),
            "missing zh calm badge:\n{joined}"
        );
        assert!(
            !joined.contains("破坏性"),
            "routine write must not use the destructive zh badge:\n{joined}"
        );
        assert!(
            joined.contains("选择："),
            "missing zh selection prefix:\n{joined}"
        );
        assert!(
            joined.contains("Enter执行选中项，或直接按y/a/d"),
            "missing zh one-step hint:\n{joined}"
        );
        assert!(
            !joined.contains("影响："),
            "impact dossier is critical-only:\n{joined}"
        );
        assert!(
            joined.contains("仅本次批准"),
            "missing zh approve option:\n{joined}"
        );
    }

    #[test]
    fn render_critical_zh_hans_localizes_security_copy() {
        let view = ApprovalView::new_for_locale(critical_request(), Locale::ZhHans);
        let lines = render_lines(&view, 100, 40);
        let joined = compact_rendered_text(&lines);
        assert!(
            joined.contains("破坏性"),
            "missing zh risk badge:\n{joined}"
        );
        assert!(
            joined.contains("影响："),
            "missing zh impact label:\n{joined}"
        );
        assert!(
            joined.contains("规则:"),
            "missing zh policy semantics:\n{joined}"
        );
        assert!(
            joined.contains("仅本次批准"),
            "missing zh approve option:\n{joined}"
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
        let view = ElevationView::new(request, Locale::En);
        assert_eq!(view.selected, 0);
    }

    #[test]
    fn test_elevation_view_keybindings() {
        let request =
            ElevationRequest::for_shell("test-id", "cargo test", "write blocked", false, true);
        let mut view = ElevationView::new(request, Locale::En);

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
        let mut view = ElevationView::new(request, Locale::En);
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
        let mut view = ElevationView::new(request, Locale::En);
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
        let mut view = ElevationView::new(request, Locale::En);
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
        let mut view = ElevationView::new(request, Locale::En);
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
        let mut view = ElevationView::new(request, Locale::En);

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
        let mut view = ElevationView::new(request, Locale::En);

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

    fn render_elevation_lines(view: &ElevationView, w: u16, h: u16) -> Vec<String> {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
        view.render(Rect::new(0, 0, w, h), &mut buf);
        (0..h)
            .map(|row| {
                (0..w)
                    .map(|col| buf[(col, row)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    fn compact_elevation_text(lines: &[String]) -> String {
        lines.join("\n").replace(' ', "")
    }

    fn elevation_shell_request() -> ElevationRequest {
        ElevationRequest::for_shell("test-id", "cargo build", "network blocked", true, false)
    }

    #[test]
    fn test_elevation_render_en_has_expected_strings() {
        let view = ElevationView::new(elevation_shell_request(), Locale::En);
        let lines = render_elevation_lines(&view, 70, 22);
        let joined = compact_elevation_text(&lines);
        assert!(
            joined.contains("SandboxDenied"),
            "missing en title:\n{joined}"
        );
        assert!(joined.contains("Tool:"), "missing en tool label:\n{joined}");
        assert!(joined.contains("Cmd:"), "missing en cmd label:\n{joined}");
        assert!(
            joined.contains("Reason:"),
            "missing en reason label:\n{joined}"
        );
    }

    #[test]
    fn test_elevation_render_zh_hans_localizes_copy() {
        let view = ElevationView::new(elevation_shell_request(), Locale::ZhHans);
        let lines = render_elevation_lines(&view, 70, 22);
        let joined = compact_elevation_text(&lines);
        assert!(joined.contains("沙箱拒绝"), "missing zh title:\n{joined}");
        assert!(
            joined.contains("工具："),
            "missing zh tool label:\n{joined}"
        );
        assert!(joined.contains("命令："), "missing zh cmd label:\n{joined}");
        assert!(
            joined.contains("原因："),
            "missing zh reason label:\n{joined}"
        );
        assert!(
            joined.contains("批准后的影响"),
            "missing zh impact header:\n{joined}"
        );
        let en_artifacts = [
            "SandboxDenied",
            "Tool:",
            "Cmd:",
            "Reason:",
            "Impactifapproved",
            "Choosehowtoproceed",
            "Allowoutboundnetwork",
            "Allowextrawriteaccess",
            "Fullaccess",
            "Abort",
        ];
        for artifact in &en_artifacts {
            assert!(
                !joined.contains(artifact),
                "English leak '{artifact}' in zh rendering:\n{joined}"
            );
        }
    }

    #[test]
    fn test_elevation_render_ja_has_translated_copy() {
        let view = ElevationView::new(elevation_shell_request(), Locale::Ja);
        let lines = render_elevation_lines(&view, 70, 22);
        let joined = compact_elevation_text(&lines);
        assert!(
            joined.contains("サンドボックス拒否"),
            "missing ja title:\n{joined}"
        );
        assert!(
            joined.contains("ツール："),
            "missing ja tool label:\n{joined}"
        );
        assert!(
            joined.contains("コマンド："),
            "missing ja cmd label:\n{joined}"
        );
        assert!(
            joined.contains("理由："),
            "missing ja reason label:\n{joined}"
        );
        for eng in &["SandboxDenied", "Tool:", "Cmd:", "Reason:"] as &[&str] {
            assert!(
                !joined.contains(eng),
                "English leak '{eng}' in ja:\n{joined}"
            );
        }
    }

    #[test]
    fn test_elevation_render_zh_hant_has_translated_copy() {
        let view = ElevationView::new(elevation_shell_request(), Locale::ZhHant);
        let lines = render_elevation_lines(&view, 70, 22);
        let joined = compact_elevation_text(&lines);
        assert!(
            joined.contains("沙箱拒絕"),
            "missing zh-Hant title:\n{joined}"
        );
        assert!(
            joined.contains("工具："),
            "missing zh-Hant tool label:\n{joined}"
        );
        assert!(
            joined.contains("命令："),
            "missing zh-Hant cmd label:\n{joined}"
        );
        assert!(
            joined.contains("原因："),
            "missing zh-Hant reason label:\n{joined}"
        );
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
