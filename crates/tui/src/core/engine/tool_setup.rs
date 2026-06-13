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
/// - **ProPlan**: `ReadOnly` as a defense-in-depth fallback. Normal ProPlan
///   turns are resolved to `Plan` or `Agent` before reaching the engine; if a
///   future path passes raw `ProPlan`, fail closed.
/// - **YOLO**: `DangerFullAccess` — explicit no-guardrails contract.
pub(crate) fn sandbox_policy_for_mode(mode: AppMode, workspace: &Path) -> SandboxPolicy {
    match mode {
        AppMode::Plan | AppMode::ProPlan => SandboxPolicy::ReadOnly,
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
        let read_only_mode = matches!(mode, AppMode::Plan | AppMode::ProPlan);
        let mut builder = if read_only_mode {
            ToolRegistryBuilder::new()
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
                .with_goal_tools(self.config.goal_state.clone())
        } else {
            ToolRegistryBuilder::new()
                .with_agent_tools(self.session.allow_shell)
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
        builder = if read_only_mode {
            builder.with_slop_ledger_read_only_tools()
        } else {
            builder.with_slop_ledger_tools()
        };

        if !read_only_mode {
            builder = builder
                .with_rlm_tool(self.deepseek_client.clone(), self.session.model.clone())
                .with_fim_tool(self.deepseek_client.clone(), self.session.model.clone())
                .with_speech_tools(
                    self.deepseek_client.clone(),
                    self.config.speech_output_dir.clone(),
                );
        }

        if self.config.features.enabled(Feature::ApplyPatch) && !read_only_mode {
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
        if self.config.memory_enabled {
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
