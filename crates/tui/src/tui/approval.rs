//! Tool approval system for `DeepSeek` CLI.
//!
//! Hosts the [`ApprovalRequest`] / [`ApprovalView`] pair the engine asks
//! the TUI to present whenever a tool needs human approval, plus the
//! sandbox elevation flow ([`ElevationRequest`] / [`ElevationView`]) that
//! follows a sandbox denial.
//!
//! ## v0.6.7: Codex-style takeover with stakes-based variants (#129)
//!
//! The modal renders as a compact bottom-anchored approval card that preserves
//! transcript context and routes each request to one of two
//! stakes-based variants:
//!
//! - **Benign** (`RiskLevel::Benign`) — read-only ops, MCP discovery,
//!   query-only network. A single `Enter` / `1` / `y` approves once;
//!   `2` / `a` approves for the session.
//! - **Destructive** (`RiskLevel::Destructive`) — file writes, shell
//!   commands that are not proven read-only, patches, MCP actions,
//!   unclassified tools, and any "fetch arbitrary content" surface.
//!   The approval card keeps the destructive badge and
//!   impact summary visible, then lets `Enter` commit the highlighted
//!   option or `y` / `a` / `d` commit directly.
//!
//! The decision events emitted upstream are unchanged
//! (`ViewEvent::ApprovalDecision`), so `ui.rs` and the engine handle
//! both variants without modification. Auto-approve / YOLO bypasses
//! happen *before* the view is constructed (see `tui/ui.rs`); this
//! module always assumes the user is being asked.

use crate::config::ApprovalDefaultSelection;
use crate::localization::{Locale, MessageId, tr};
use crate::sandbox::SandboxPolicy;
use crate::tools::apply_patch::{NormalizedApplyPatchInput, normalize_apply_patch_input};
use crate::tools::canonical_action::canonical_action_alias;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};
use crate::tui::widgets::{ApprovalWidget, ElevationWidget, Renderable};
use codewhale_config::ToolAskRule;
use codewhale_execpolicy::PermissionAction;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use serde_json::Value;
use std::borrow::Cow;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub mod policy;

pub use policy::{
    ApprovalStakes, RiskLevel, ToolCategory, classify_risk, classify_stakes,
    get_tool_category_for_call,
};

/// Determines when tool executions require user approval. Defined in
/// codewhale-execpolicy (next to `AskForApproval`); re-exported here so
/// `crate::tui::approval::ApprovalMode` keeps working.
pub use codewhale_execpolicy::ApprovalMode;

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
    /// Stakes-based routing for the compact approval card
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
    /// Exact repo-scoped allow rules available for safe approval requests.
    pub persistent_allow_rules: Vec<ToolAskRule>,
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

/// Human-readable preview of rules an approval action would append.
///
/// This is intentionally derived from the already validated persistent-rule
/// candidates; the approval UI must not re-parse tool inputs such as patches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRuleSavePreview {
    pub action: PermissionAction,
    pub rule_count: usize,
    pub entries: Vec<String>,
    pub omitted: usize,
}

impl PermissionRuleSavePreview {
    #[must_use]
    pub fn summary(&self) -> String {
        let action = match self.action {
            PermissionAction::Allow => "allow",
            PermissionAction::Ask => "ask",
            PermissionAction::Deny => "deny",
        };
        let noun = if self.rule_count == 1 {
            "rule"
        } else {
            "rules"
        };
        format!("{} {action} {noun}", self.rule_count)
    }
}

const ASK_RULE_SAVE_PREVIEW_MAX_ENTRIES: usize = 4;

impl ApprovalRequest {
    /// Mechanical repo-law asks are a distinct authority boundary, not an
    /// ordinary risk prompt. The engine stamps this stable prefix when a
    /// `.codewhale/constitution.json` ask rule forces review.
    #[must_use]
    pub fn is_repo_law_prompt(&self) -> bool {
        description_is_repo_law_prompt(&self.description)
    }

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
        let semantic_tool_name = canonical_action_alias(tool_name, params);
        let category = get_tool_category_for_call(tool_name, params);
        let risk = classify_risk(tool_name, category, params);
        let approval_grouping_key =
            crate::tools::approval_cache::build_approval_grouping_key(tool_name, params).0;
        let persistent_ask_rules =
            build_persistent_ask_rules(semantic_tool_name, params, workspace);
        let persistent_allow_rules = if classify_stakes(tool_name, category, risk, params)
            == ApprovalStakes::Critical
            || description_is_repo_law_prompt(description)
        {
            Vec::new()
        } else {
            build_persistent_allow_rules(
                semantic_tool_name,
                params,
                workspace,
                &persistent_ask_rules,
            )
        };

        Self {
            id: id.to_string(),
            tool_name: tool_name.to_string(),
            description: description.to_string(),
            category,
            risk,
            impacts: build_impact_summary(semantic_tool_name, category, params),
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
            persistent_ask_rules,
            persistent_allow_rules,
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
        let semantic_tool_name = canonical_action_alias(&self.tool_name, &self.params);
        match locale {
            Locale::ZhHans => {
                build_impact_summary_zh_hans(semantic_tool_name, self.category, &self.params)
            }
            _ => self.impacts.clone(),
        }
    }

    #[must_use]
    pub fn can_save_ask_rule(&self) -> bool {
        !self.persistent_ask_rules.is_empty()
    }

    #[must_use]
    pub fn can_save_allow_rule(&self) -> bool {
        !self.persistent_allow_rules.is_empty()
            && self.stakes() != ApprovalStakes::Critical
            && !self.is_repo_law_prompt()
    }

    #[must_use]
    pub fn ask_rule_save_preview(&self) -> Option<PermissionRuleSavePreview> {
        build_permission_rule_save_preview(
            &self.persistent_ask_rules,
            ASK_RULE_SAVE_PREVIEW_MAX_ENTRIES,
        )
    }

    #[must_use]
    pub fn allow_rule_save_preview(&self) -> Option<PermissionRuleSavePreview> {
        self.can_save_allow_rule().then(|| {
            build_permission_rule_save_preview(
                &self.persistent_allow_rules,
                ASK_RULE_SAVE_PREVIEW_MAX_ENTRIES,
            )
            .expect("eligible allow rules are non-empty")
        })
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
        let semantic_tool_name = canonical_action_alias(&self.tool_name, &self.params);
        build_prominent_details(semantic_tool_name, self.category, &self.params)
            .into_iter()
            .map(|mut detail| {
                let is_preview = detail.label == "Preview";
                detail.label = localize_detail_label(&detail.label, locale).to_string();
                if is_preview && let Some(lines) = detail.shell_lines.as_mut() {
                    for line in lines.iter_mut() {
                        *line = localize_preview_shell_line(semantic_tool_name, line, locale)
                            .to_string();
                    }
                    detail.value = lines.join("\n");
                }
                detail
            })
            .collect()
    }
}

fn description_is_repo_law_prompt(description: &str) -> bool {
    description.starts_with("Repo law holds this write:")
        && description.contains(".codewhale/constitution.json")
}

#[must_use]
fn build_permission_rule_save_preview(
    rules: &[ToolAskRule],
    max_entries: usize,
) -> Option<PermissionRuleSavePreview> {
    if rules.is_empty() {
        return None;
    }

    let entries = rules
        .iter()
        .take(max_entries)
        .map(format_permission_rule_save_entry)
        .collect();
    Some(PermissionRuleSavePreview {
        action: rules[0].action,
        rule_count: rules.len(),
        entries,
        omitted: rules.len().saturating_sub(max_entries),
    })
}

#[must_use]
fn format_permission_rule_save_entry(rule: &ToolAskRule) -> String {
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
    if rule.command_exact {
        parts.push("command_exact=true".to_string());
    }
    if let Some(workspace) = &rule.workspace {
        parts.push(format!(
            "workspace={}",
            sanitize_ask_rule_preview_value(workspace)
        ));
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
    let semantic = canonical_action_alias(tool_name, params);
    match semantic {
        "exec_shell" => build_exec_shell_ask_rules(params),
        // File writes save an exact, workspace-relative path so a later
        // edit/write of the same file is matched. read_file stays out: this
        // boundary is about persisting *write* approvals only.
        "write_file" | "edit_file" => build_file_write_ask_rules(semantic, params, workspace),
        "apply_patch" => build_apply_patch_ask_rules(params, workspace),
        _ => Vec::new(),
    }
}

#[must_use]
fn build_persistent_allow_rules(
    tool_name: &str,
    params: &Value,
    workspace: &Path,
    exact_rules: &[ToolAskRule],
) -> Vec<ToolAskRule> {
    if exact_rules.is_empty() {
        return Vec::new();
    }

    if tool_name == "exec_shell" {
        let Some(command) = params.get("command").and_then(Value::as_str) else {
            return Vec::new();
        };
        if !matches!(
            crate::command_safety::analyze_command(command).level,
            crate::command_safety::SafetyLevel::Safe
                | crate::command_safety::SafetyLevel::WorkspaceSafe
        ) {
            return Vec::new();
        }
    }

    let workspace = workspace.to_string_lossy();
    let Some(workspace) = codewhale_execpolicy::normalize_workspace_scope(workspace.as_ref())
    else {
        return Vec::new();
    };

    exact_rules
        .iter()
        .cloned()
        .map(|rule| rule.into_exact_workspace_allow(workspace.clone()))
        .collect()
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
        ToolCategory::Agent if tool_name == "workflow" => {
            // #4126: elevated Workflow plan card — goal, children, capability flags, budget.
            crate::tools::workflow_plan_approval::analyze_workflow_plan_approval(params)
                .approval_impacts()
        }
        ToolCategory::Agent => {
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
    let locale = Locale::ZhHans;
    match category {
        ToolCategory::Safe => tr(locale, MessageId::ApprovalDescSafe).to_string(),
        ToolCategory::FileWrite => tr(locale, MessageId::ApprovalDescFileWrite).to_string(),
        ToolCategory::Shell => tr(locale, MessageId::ApprovalDescShell).to_string(),
        ToolCategory::Network => tr(locale, MessageId::ApprovalDescNetwork).to_string(),
        ToolCategory::McpRead => tr(locale, MessageId::ApprovalDescMcpRead).to_string(),
        ToolCategory::McpAction => tr(locale, MessageId::ApprovalDescMcpAction).to_string(),
        ToolCategory::Agent => tr(locale, MessageId::ApprovalDescAgent).to_string(),
        ToolCategory::Unknown => tr(locale, MessageId::ApprovalDescUnknown).to_string(),
    }
}

fn build_impact_summary_zh_hans(
    tool_name: &str,
    category: ToolCategory,
    params: &Value,
) -> Vec<String> {
    let locale = Locale::ZhHans;
    match category {
        ToolCategory::Safe => {
            let mut impacts = vec![tr(locale, MessageId::ApprovalImpactSafe).to_string()];
            if let Some(path) = param_preview(params, &["path", "ref_id", "uri"], 72) {
                impacts.push(format!("读取：{path}"));
            }
            impacts
        }
        ToolCategory::FileWrite => {
            let mut impacts = vec![tr(locale, MessageId::ApprovalImpactFileWrite).to_string()];
            if let Some(path) = param_preview(params, &["path", "target", "destination"], 72) {
                impacts.push(format!("写入：{path}"));
            }
            impacts
        }
        ToolCategory::Shell => {
            vec![tr(locale, MessageId::ApprovalImpactShell).to_string()]
        }
        ToolCategory::Network => {
            let mut impacts = vec![tr(locale, MessageId::ApprovalImpactNetwork).to_string()];
            if let Some(target) =
                param_preview(params, &["url", "q", "query", "location", "repo"], 96)
            {
                impacts.push(format!("目标：{target}"));
            }
            impacts
        }
        ToolCategory::McpRead => {
            let mut impacts = vec![tr(locale, MessageId::ApprovalImpactMcpRead).to_string()];
            if let Some(target) = mcp_target_hint(tool_name) {
                impacts.push(format!("MCP 目标：{target}"));
            }
            impacts
        }
        ToolCategory::McpAction => {
            let mut impacts = vec![tr(locale, MessageId::ApprovalImpactMcpAction).to_string()];
            if let Some(target) = mcp_target_hint(tool_name) {
                impacts.push(format!("MCP 目标：{target}"));
            }
            impacts
        }
        ToolCategory::Agent => {
            let mut impacts = vec![tr(locale, MessageId::ApprovalImpactAgent).to_string()];
            if let Some(kind) = param_preview(params, &["type"], 40) {
                impacts.push(format!("子代理类型：{kind}"));
            }
            impacts
        }
        ToolCategory::Unknown => {
            let mut impacts = vec![tr(locale, MessageId::ApprovalImpactUnknown).to_string()];
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
        ToolCategory::Agent if tool_name == "workflow" => {
            // #4126: elevated Workflow plan card fields.
            let summary =
                crate::tools::workflow_plan_approval::analyze_workflow_plan_approval(params);
            for (label, value) in summary.card_fields() {
                details.push(ApprovalDetail {
                    label: label.to_string(),
                    value,
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
    match canonical_action_alias(tool_name, params) {
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
            // Keep the per-frame card preview bounded. The details pager builds the
            // complete version lazily when the reviewer asks for it.
            edit_file_preview_lines(params, 3)
        }
        "apply_patch" => match normalize_apply_patch_input(params) {
            Ok(NormalizedApplyPatchInput::Patch(patch)) => apply_patch_preview_lines(patch),
            Ok(NormalizedApplyPatchInput::Replacement { entries, .. }) => {
                changes_preview_lines(entries)
            }
            Err(_) => None,
        },
        _ => None,
    }
    .filter(|lines| !lines.is_empty())
}

fn edit_file_preview_lines(params: &Value, max_lines: usize) -> Option<Vec<String>> {
    if let Some(edits) = params.get("edits").and_then(Value::as_array) {
        let mut lines = Vec::new();
        for (index, edit) in edits.iter().take(max_lines).enumerate() {
            let old = param_text(edit, &["oldText"])?;
            let new = param_text(edit, &["newText"])?;
            lines.push(format!("edit {}", index + 1));
            lines.extend(prefixed_preview_lines("replace this", "- ", &old, 1));
            lines.extend(prefixed_preview_lines("with this", "+ ", &new, 1));
        }
        if edits.len() > max_lines {
            lines.push(format!("... (+{} more edits)", edits.len() - max_lines));
        }
        return (!lines.is_empty()).then_some(lines);
    }
    let search = param_text(params, &["search"])?;
    let replace = param_text(params, &["replace"])?;
    let mut lines = Vec::new();
    lines.extend(prefixed_preview_lines(
        "replace this",
        "- ",
        &search,
        max_lines,
    ));
    lines.extend(prefixed_preview_lines(
        "with this",
        "+ ",
        &replace,
        max_lines,
    ));
    Some(lines)
}

fn exact_edit_file_preview_lines(params: &Value, locale: Locale) -> Option<Vec<String>> {
    if let Some(edits) = params.get("edits").and_then(Value::as_array) {
        let mut lines = Vec::new();
        for (index, edit) in edits.iter().enumerate() {
            let old = param_text(edit, &["oldText"])?;
            let new = param_text(edit, &["newText"])?;
            lines.push(format!("edit {}", index + 1));
            lines.push(tr(locale, MessageId::ApprovalLabelReplaceThis).into_owned());
            lines.extend(exact_preview_body_lines("- ", &old));
            lines.push(tr(locale, MessageId::ApprovalLabelWithThis).into_owned());
            lines.extend(exact_preview_body_lines("+ ", &new));
        }
        return (!lines.is_empty()).then_some(lines);
    }
    let search = param_text(params, &["search"])?;
    let replace = param_text(params, &["replace"])?;
    let mut lines = vec![tr(locale, MessageId::ApprovalLabelReplaceThis).into_owned()];
    lines.extend(exact_preview_body_lines("- ", &search));
    lines.push(tr(locale, MessageId::ApprovalLabelWithThis).into_owned());
    lines.extend(exact_preview_body_lines("+ ", &replace));
    Some(lines)
}

fn exact_preview_body_lines(prefix: &str, content: &str) -> Vec<String> {
    if content.is_empty() {
        return vec![format!("{prefix}\"\"")];
    }

    content
        .split_inclusive('\n')
        .map(|chunk| {
            let (body, ending) = if let Some(body) = chunk.strip_suffix("\r\n") {
                (body, "\\r\\n")
            } else if let Some(body) = chunk.strip_suffix('\n') {
                (body, "\\n")
            } else {
                (chunk, "")
            };
            exact_preview_body_line(prefix, body, ending)
        })
        .collect()
}

fn exact_preview_body_line(prefix: &str, body: &str, ending: &str) -> String {
    let mut line = String::with_capacity(prefix.len() + body.len() + ending.len() + 2);
    line.push_str(prefix);
    line.push('"');
    for ch in body.chars() {
        match ch {
            '\\' => line.push_str("\\\\"),
            '"' => line.push_str("\\\""),
            ' ' => line.push_str("\\x20"),
            '\t' => line.push_str("\\t"),
            '\r' => line.push_str("\\r"),
            ch if ch.is_whitespace() || ch.is_control() => line.extend(ch.escape_unicode()),
            ch => line.push(ch),
        }
    }
    line.push_str(ending);
    line.push('"');
    line
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
        if idx > 0 && !push_preview_line(&mut lines, String::new(), PREVIEW_LIMIT) {
            break;
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

fn localize_detail_label(label: &str, locale: Locale) -> Cow<'static, str> {
    match locale {
        Locale::ZhHans => match label {
            "Command" => tr(locale, MessageId::ApprovalLabelCommand),
            "Dir" => tr(locale, MessageId::ApprovalLabelDir),
            "File" => tr(locale, MessageId::ApprovalLabelFile),
            "Preview" => tr(locale, MessageId::ApprovalLabelPreview),
            "proposed content" => tr(locale, MessageId::ApprovalLabelProposedContent),
            "replace this" => tr(locale, MessageId::ApprovalLabelReplaceThis),
            "with this" => tr(locale, MessageId::ApprovalLabelWithThis),
            "replacement content" => tr(locale, MessageId::ApprovalLabelReplacementContent),
            "Path" => tr(locale, MessageId::ApprovalLabelPath),
            "Target" => tr(locale, MessageId::ApprovalLabelTarget),
            "Input" => tr(locale, MessageId::ApprovalLabelInput),
            "Action" => tr(locale, MessageId::ApprovalLabelAction),
            "Type" => tr(locale, MessageId::ApprovalLabelType),
            "Prompt" => tr(locale, MessageId::ApprovalLabelPrompt),
            "Goal" => "目标".into(),
            "Children" => "子任务".into(),
            "Writes" => "写入".into(),
            "Shell" => "Shell".into(),
            "Network" => "网络".into(),
            "Budget" => "预算".into(),
            _ => label.to_string().into(),
        },
        _ => label.to_string().into(),
    }
}

fn localize_preview_shell_line(tool_name: &str, line: &str, locale: Locale) -> Cow<'static, str> {
    match tool_name {
        "write_file" if line == "proposed content" => localize_detail_label(line, locale),
        "edit_file" if matches!(line, "replace this" | "with this") => {
            localize_detail_label(line, locale)
        }
        _ => line.to_string().into(),
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
    AllowExactRepo,
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
    const ORDER_WITH_PERSISTENT_ALLOW: [ApprovalOption; 5] = [
        ApprovalOption::ApproveOnce,
        ApprovalOption::ApproveAlways,
        ApprovalOption::AllowExactRepo,
        ApprovalOption::Deny,
        ApprovalOption::Abort,
    ];

    /// Workflow elevated-plan card (#4126): Approve / Edit plan / Cancel.
    const WORKFLOW_ORDER: [ApprovalOption; 3] = [
        ApprovalOption::ApproveOnce,
        ApprovalOption::Deny,
        ApprovalOption::Abort,
    ];

    fn order_for(request: &ApprovalRequest) -> &'static [ApprovalOption] {
        if request.tool_name == "workflow" {
            &Self::WORKFLOW_ORDER
        } else if request.can_save_allow_rule() {
            &Self::ORDER_WITH_PERSISTENT_ALLOW
        } else {
            &Self::ORDER
        }
    }

    fn from_index_for(request: &ApprovalRequest, idx: usize) -> ApprovalOption {
        Self::order_for(request)
            .get(idx)
            .copied()
            .unwrap_or(Self::Abort)
    }

    fn index_for(self, request: &ApprovalRequest) -> usize {
        Self::order_for(request)
            .iter()
            .position(|o| *o == self)
            .unwrap_or(Self::order_for(request).len().saturating_sub(1))
    }

    fn decision(self) -> ReviewDecision {
        match self {
            ApprovalOption::ApproveOnce => ReviewDecision::Approved,
            ApprovalOption::ApproveAlways => ReviewDecision::ApprovedForSession,
            ApprovalOption::AllowExactRepo => ReviewDecision::Approved,
            // Workflow maps Deny → "Edit plan" (model revises plan).
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
    row_hitboxes: RefCell<Vec<Rect>>,
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

    #[cfg(test)]
    pub fn new_for_locale(request: ApprovalRequest, locale: Locale) -> Self {
        Self::new_with_default_selection(request, locale, ApprovalDefaultSelection::default())
    }

    /// `default_selection` is `[approval] default_selection` (#5293). Deny
    /// stays the default so a fresh card never turns a reflexive Enter into
    /// authorization; `allow_once` is a user opting out of that guard.
    pub fn new_with_default_selection(
        request: ApprovalRequest,
        locale: Locale,
        default_selection: ApprovalDefaultSelection,
    ) -> Self {
        // Resolve the semantic option because its numeric index differs for
        // persistent-allow and workflow approval cards.
        let selected = match default_selection {
            ApprovalDefaultSelection::Deny => ApprovalOption::Deny,
            ApprovalDefaultSelection::AllowOnce => ApprovalOption::ApproveOnce,
        }
        .index_for(&request);
        Self {
            request,
            selected,
            row_hitboxes: RefCell::new(Vec::new()),
            locale,
            timeout: None,
            requested_at: Instant::now(),
            collapsed: false,
        }
    }

    fn select_prev(&mut self) {
        let len = ApprovalOption::order_for(&self.request).len();
        self.selected = crate::tui::list_nav::wrap_index(self.selected, len, -1);
    }

    fn select_next(&mut self) {
        let len = ApprovalOption::order_for(&self.request).len();
        self.selected = crate::tui::list_nav::wrap_index(self.selected, len, 1);
    }

    fn current_option(&self) -> ApprovalOption {
        ApprovalOption::from_index_for(&self.request, self.selected)
    }

    /// Whether this approval is the elevated Workflow plan card (#4126).
    #[must_use]
    pub fn is_workflow_plan_approval(&self) -> bool {
        self.request.tool_name == "workflow"
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

    pub(crate) fn set_mouse_hitboxes(&self, hitboxes: Vec<Rect>) {
        *self.row_hitboxes.borrow_mut() = hitboxes;
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
        self.selected = option.index_for(&self.request);
        if option == ApprovalOption::AllowExactRepo && self.request.can_save_allow_rule() {
            self.emit_decision_with_rules(
                option.decision(),
                false,
                self.request.persistent_allow_rules.clone(),
            )
        } else {
            self.emit_decision(option.decision(), false)
        }
    }

    fn emit_decision(&self, decision: ReviewDecision, timed_out: bool) -> ViewAction {
        self.emit_decision_with_rules(decision, timed_out, Vec::new())
    }

    fn emit_decision_with_rules(
        &self,
        decision: ReviewDecision,
        timed_out: bool,
        persistent_rules: Vec<ToolAskRule>,
    ) -> ViewAction {
        ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
            tool_id: self.request.id.clone(),
            tool_name: self.request.tool_name.clone(),
            decision,
            timed_out,
            approval_key: self.request.approval_key.clone(),
            approval_grouping_key: self.request.approval_grouping_key.clone(),
            persistent_rules,
        })
    }

    fn emit_params_pager(&self) -> ViewAction {
        // The compact prompt keeps the about/impact dossier out of the
        // default band; the pager is where that context now lives.
        let locale = self.locale();
        let about_label = tr(locale, MessageId::ApprovalLabelAbout);
        let impact_label = tr(locale, MessageId::ApprovalLabelImpact);
        let mut content = String::new();
        content.push_str(&about_label);
        content.push_str(&self.request.description_for_locale(locale));
        content.push('\n');
        for impact in self.request.impacts_for_locale(locale) {
            content.push_str(&impact_label);
            content.push_str(&impact);
            content.push('\n');
        }
        content.push('\n');
        if canonical_action_alias(&self.request.tool_name, &self.request.params) == "edit_file"
            && let Some(preview_lines) = exact_edit_file_preview_lines(&self.request.params, locale)
        {
            content.push_str(&tr(locale, MessageId::ApprovalLabelPreview));
            content.push_str(":\n");
            for line in preview_lines {
                content.push_str(&line);
                content.push('\n');
            }
            content.push('\n');
        }
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
            KeyCode::Char('a') | KeyCode::Char('A') | KeyCode::Char('2')
                if !self.is_workflow_plan_approval() =>
            {
                self.commit_option(ApprovalOption::ApproveAlways)
            }
            KeyCode::Char('p') | KeyCode::Char('P') if self.request.can_save_allow_rule() => {
                self.commit_option(ApprovalOption::AllowExactRepo)
            }
            // Workflow plan card (#4126): [2/e] Edit plan, [3/n/d] Cancel.
            KeyCode::Char('e') | KeyCode::Char('E') | KeyCode::Char('2')
                if self.is_workflow_plan_approval() =>
            {
                self.commit_option(ApprovalOption::Deny)
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
            | KeyCode::Char('3') => {
                if self.is_workflow_plan_approval() {
                    // Cancel (abort turn) rather than session-deny.
                    self.commit_option(ApprovalOption::Abort)
                } else {
                    self.commit_option(ApprovalOption::Deny)
                }
            }
            // Details is Alt+V / Option+V only; bare `v` is never a shortcut.
            _ if crate::tui::shell_key_routing::is_tool_details_shortcut(&key) => {
                self.emit_params_pager()
            }
            KeyCode::Esc => self.emit_decision(ReviewDecision::Abort, false),
            _ => ViewAction::None,
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.select_prev();
                ViewAction::None
            }
            MouseEventKind::ScrollDown => {
                self.select_next();
                ViewAction::None
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let clicked = self.row_hitboxes.borrow().iter().position(|rect| {
                    rect.contains(ratatui::layout::Position::new(mouse.column, mouse.row))
                });
                if let Some(index) = clicked {
                    return self
                        .commit_option(ApprovalOption::from_index_for(&self.request, index));
                }
                ViewAction::None
            }
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
    row_hitboxes: RefCell<Vec<Rect>>,
}

impl ElevationView {
    pub fn new(request: ElevationRequest, locale: Locale) -> Self {
        Self {
            request,
            selected: 0,
            locale,
            row_hitboxes: RefCell::new(Vec::new()),
        }
    }

    fn select_prev(&mut self) {
        self.selected =
            crate::tui::list_nav::wrap_index(self.selected, self.request.options.len(), -1);
    }

    fn select_next(&mut self) {
        self.selected =
            crate::tui::list_nav::wrap_index(self.selected, self.request.options.len(), 1);
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

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.select_prev();
                ViewAction::None
            }
            MouseEventKind::ScrollDown => {
                self.select_next();
                ViewAction::None
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let clicked = self.row_hitboxes.borrow().iter().position(|rect| {
                    rect.contains(ratatui::layout::Position::new(mouse.column, mouse.row))
                });
                if let Some(index) = clicked {
                    return self.emit_decision(self.request.options[index].clone());
                }
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        let elevation_widget = ElevationWidget::new_with_hitboxes(
            &self.request,
            self.selected,
            self.locale,
            &self.row_hitboxes,
        );
        elevation_widget.render(area, buf);
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests;
