//! Per-turn tool registry setup.
//!
//! This keeps mode/feature-specific registry construction out of the send path.

use std::path::Path;

use super::*;
use crate::sandbox::SandboxPolicy;

/// Pick the sandbox policy that gates shell commands for a given UI mode.
///
/// - **Plan** (#1077): `ReadOnly` — no writes, no network. The previous
///   `WorkspaceWrite` policy let `python -c "open('f','w').write('x')"` mutate
///   files inside the workspace because it whitelisted the workspace as
///   writable. Plan mode is investigation only; if the user wants to change
///   files they should switch to Agent.
/// - **Agent**: `WorkspaceWrite` with workspace as writable root and network
///   on. Approval flow gates risky individual commands; the sandbox handles
///   the rest. Network is allowed because cargo / npm / curl-style commands
///   are normal during agent work and DNS-deny breaks them silently.
/// - **YOLO**: `DangerFullAccess` — explicit no-guardrails contract.
pub(crate) fn sandbox_policy_for_mode(mode: AppMode, workspace: &Path) -> SandboxPolicy {
    match mode {
        AppMode::Plan => SandboxPolicy::ReadOnly,
        AppMode::Agent => SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![workspace.to_path_buf()],
            network_access: true,
            exclude_tmpdir: false,
            exclude_slash_tmp: false,
        },
        AppMode::Yolo => SandboxPolicy::DangerFullAccess,
    }
}

impl Engine {
    pub(super) fn build_turn_tool_registry_builder(
        &self,
        mode: AppMode,
        todo_list: SharedTodoList,
        plan_state: SharedPlanState,
    ) -> ToolRegistryBuilder {
        let mut builder = if mode == AppMode::Plan {
            ToolRegistryBuilder::new()
                .with_read_only_file_tools()
                .with_search_tools()
                .with_git_tools()
                .with_git_history_tools()
                .with_diagnostics_tool()
                .with_skill_tools()
                .with_validation_tools()
                .with_runtime_read_only_task_tools()
                .with_todo_tool(todo_list)
                .with_plan_tool(plan_state)
        } else {
            ToolRegistryBuilder::new()
                .with_agent_tools(self.session.allow_shell)
                .with_todo_tool(todo_list)
                .with_plan_tool(plan_state)
        };

        builder = builder
            .with_review_tool(self.deepseek_client.clone(), self.session.model.clone())
            .with_user_input_tool()
            .with_parallel_tool()
            .with_recall_archive_tool();

        if mode != AppMode::Plan {
            builder = builder
                .with_rlm_tool(self.deepseek_client.clone(), self.session.model.clone())
                .with_fim_tool(self.deepseek_client.clone(), self.session.model.clone());
        }

        if self.config.features.enabled(Feature::ApplyPatch) && mode != AppMode::Plan {
            builder = builder.with_patch_tools();
        }
        if self.config.features.enabled(Feature::WebSearch) {
            builder = builder.with_web_tools();
        }
        // Plan mode is strictly read-only: do not expose shell execution at
        // all, even if the session would otherwise allow it.
        if mode != AppMode::Plan
            && self.config.features.enabled(Feature::ShellTool)
            && self.session.allow_shell
        {
            builder = builder.with_shell_tools();
        }

        // Register the `remember` tool only when the user has opted in to
        // user-memory (#489). Without that opt-in the tool would always
        // fail; surfacing it would just waste catalog slots.
        if self.config.memory_enabled {
            builder = builder.with_remember_tool();
        }

        builder
    }
}
