//! Per-turn tool registry setup.
//!
//! This keeps mode/feature-specific registry construction out of the send path.

use std::path::Path;

use super::*;
use crate::sandbox::SandboxPolicy;
use crate::worker_profile::ShellPolicy;

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

/// Resolve the effective shell policy for a turn from the legacy shell opt-in
/// plus the active mode. This is the typed bridge away from passing a bare
/// `allow_shell` boolean through the runtime.
pub(crate) fn shell_policy_for_mode(mode: AppMode, allow_shell: bool) -> ShellPolicy {
    if !allow_shell {
        return ShellPolicy::None;
    }
    match mode {
        // Plan is read-only planning with no shell execution. The runtime
        // prompt already reports `shell_access="none"` for Plan, so mapping it
        // to `ReadOnly` here created a prompt/registry inconsistency (the
        // registry would expose `exec_shell` while the prompt said there was
        // no shell). Keep Plan shell-free; switch to Agent to run commands.
        AppMode::Plan => ShellPolicy::None,
        AppMode::Agent | AppMode::Yolo => ShellPolicy::Full,
    }
}

impl Engine {
    pub(super) fn build_turn_tool_registry_builder(
        &self,
        mode: AppMode,
        todo_list: SharedTodoList,
        plan_state: SharedPlanState,
    ) -> ToolRegistryBuilder {
        let shell_policy = shell_policy_for_mode(mode, self.session.allow_shell);
        let mut builder = if mode == AppMode::Plan {
            let builder = ToolRegistryBuilder::new()
                .with_read_only_file_tools()
                .with_search_tools()
                .with_git_tools()
                .with_git_history_tools()
                .with_diagnostics_tool()
                .with_skill_tools()
                .with_validation_tools()
                .with_handle_tools()
                .with_runtime_read_only_task_tools()
                .with_todo_tool(todo_list)
                .with_plan_tool(plan_state)
                .with_goal_tools(self.config.goal_state.clone());
            if shell_policy.allows_shell() {
                builder.with_shell_tools().with_runtime_task_shell_tools()
            } else {
                builder
            }
        } else {
            ToolRegistryBuilder::new()
                .with_agent_tools_policy(shell_policy)
                .with_todo_tool(todo_list)
                .with_plan_tool(plan_state)
                .with_goal_tools(self.config.goal_state.clone())
        };

        builder = builder
            .with_review_tool(self.deepseek_client.clone(), self.session.model.clone())
            .with_user_input_tool()
            .with_parallel_tool();

        // SlopLedger: plan mode only gets read-only query + export,
        // agent/yolo get the full set including append + update.
        builder = if mode == AppMode::Plan {
            builder.with_slop_ledger_read_only_tools()
        } else {
            builder.with_slop_ledger_tools()
        };

        if mode != AppMode::Plan {
            builder = builder
                .with_rlm_tool(self.deepseek_client.clone(), self.session.model.clone())
                .with_fim_tool(self.deepseek_client.clone(), self.session.model.clone())
                .with_speech_tools(
                    self.deepseek_client.clone(),
                    self.config.speech_output_dir.clone(),
                );
        }

        if self.config.features.enabled(Feature::ApplyPatch) && mode != AppMode::Plan {
            builder = builder.with_patch_tools();
        }
        if self.config.features.enabled(Feature::WebSearch) {
            builder = builder.with_web_tools();
        }
        // Shell tools (exec_shell, task_shell_start, etc.) are already gated
        // behind `allow_shell` inside `with_agent_tools`. No separate
        // feature-flag gate here to avoid double-registration.

        // Register the `remember` tool only when the user has opted in to
        // user-memory (#489). Without that opt-in the tool would always
        // fail; surfacing it would just waste catalog slots.
        // TODO(v0.8.71): remove when Moraine recall stable; see #3490, #3495
        if self.config.memory_enabled && !self.config.moraine_fallback {
            builder = builder.with_remember_tool();
        }

        // Register image_analyze tool when vision_model is configured and feature enabled.
        if self.config.features.enabled(Feature::VisionModel)
            && let Some(ref vision_config) = self.config.vision_config
        {
            builder = builder.with_vision_tools(vision_config.clone());
        }

        // Register the `notify` tool unconditionally (#1322). It has no
        // side effects beyond a single terminal escape write and respects
        // the user's `[notifications].method` config (including `off`),
        // so there's no failure mode worth gating on.
        builder = builder.with_notify_tool();

        builder
    }
}
