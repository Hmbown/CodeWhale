//! Approval risk and stakes policy.
//!
//! This module is intentionally UI-free: it classifies tool calls so the
//! approval and elevation views can render the decision without owning the
//! policy itself.

use crate::command_safety::is_parallel_readonly_command;
use serde_json::Value;

/// Categorizes tools by cost/risk level.
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
    /// own tool gates govern what it may actually do.
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
/// Routing rules live in [`classify_risk`] - when in doubt, route to
/// `Destructive`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Benign,
    Destructive,
}

/// Presentation-level stakes for the approval prompt (#3883 follow-up).
///
/// `RiskLevel` drives keymaps and stays conservative ("not provably
/// read-only" is `Destructive`), but rendering everything in that bucket
/// as a red DESTRUCTIVE takeover made routine file edits and build
/// commands read like emergencies. Stakes split presentation three ways:
///
/// - `Routine` - provably read-only; minimal chrome.
/// - `Elevated` - ordinary state-touching work (edits, builds, MCP
///   actions); a calm approval, not a warning.
/// - `Critical` - genuinely destructive, publish-like, or
///   secret-touching per `ToolActionKind`; keeps the strong styling and
///   the policy semantics lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalStakes {
    Routine,
    Elevated,
    Critical,
}

/// Get the category for a tool by name.
pub fn get_tool_category(name: &str) -> ToolCategory {
    if name == "agent" || name == "workflow" {
        // Workflow is multi-agent orchestration; reuse Agent stakes/routing
        // and specialize the impact card via build_impact_summary (#4126).
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
            | "work_update"
            | "todo_write"
            | "todo_read"
            | "checklist_write"
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
    } else if name == "start_mcp_server" {
        // Starting an MCP server spawns child processes or opens network
        // connections — classify as McpAction to trigger appropriate
        // approval prompts.
        ToolCategory::McpAction
    } else {
        ToolCategory::Unknown
    }
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
            // actions fetch model-supplied URLs (arbitrary remote content) -
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
            if let Some(cmd) = params.get("command").and_then(Value::as_str)
                && is_parallel_readonly_command(cmd)
            {
                return RiskLevel::Benign;
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
        // File writes, MCP actions, unclassified surfaces - all require
        // explicit confirmation.
        ToolCategory::FileWrite | ToolCategory::McpAction | ToolCategory::Unknown => {
            RiskLevel::Destructive
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_read_only_surfaces_as_benign() {
        for name in ["read_file", "list_dir", "list_mcp_tools", "web_search"] {
            let category = get_tool_category(name);
            assert_eq!(
                classify_risk(name, category, &json!({})),
                RiskLevel::Benign,
                "{name}"
            );
        }
    }

    #[test]
    fn classifies_stateful_or_unknown_surfaces_as_destructive() {
        for name in [
            "write_file",
            "edit_file",
            "apply_patch",
            "mcp_linear_save_issue",
            "fetch_url",
            "unknown_tool",
        ] {
            let category = get_tool_category(name);
            assert_eq!(
                classify_risk(name, category, &json!({})),
                RiskLevel::Destructive,
                "{name}"
            );
        }
    }

    #[test]
    fn shell_risk_uses_command_safety_analysis() {
        let category = get_tool_category("exec_shell");
        assert_eq!(
            classify_risk(
                "exec_shell",
                category,
                &json!({"command": "git status --short"})
            ),
            RiskLevel::Benign
        );
        assert_eq!(
            classify_risk(
                "exec_shell",
                category,
                &json!({"command": "rm -rf /tmp/example"})
            ),
            RiskLevel::Destructive
        );
    }

    #[test]
    fn web_run_open_and_click_fetch_remote_content() {
        let category = get_tool_category("web_run");
        assert_eq!(
            classify_risk(
                "web_run",
                category,
                &json!({"search_query": [{"q": "rust"}]})
            ),
            RiskLevel::Benign
        );
        assert_eq!(
            classify_risk("web_run", category, &json!({"open": [{"ref_id": "x"}]})),
            RiskLevel::Destructive
        );
        assert_eq!(
            classify_risk(
                "web_run",
                category,
                &json!({"click": [{"ref_id": "x", "id": 1}]})
            ),
            RiskLevel::Destructive
        );
    }
}
