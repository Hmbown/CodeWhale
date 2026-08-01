//! Per-turn tool registry setup.
//!
//! This keeps mode/feature-specific registry construction out of the send path.

use super::*;
use crate::core::authority::shell_policy_for_mode;
use crate::tools::AgentToolSurfaceOptions;
use crate::worker_profile::ShellPolicy;

fn should_register_remember_tool(memory_enabled: bool, moraine_fallback: bool) -> bool {
    memory_enabled && !moraine_fallback
}

impl Engine {
    pub(super) fn agent_tool_surface_options(
        &self,
        shell_policy: ShellPolicy,
    ) -> AgentToolSurfaceOptions {
        let mut options = AgentToolSurfaceOptions::new(shell_policy);
        options.apply_patch_enabled = self.config.features.enabled(Feature::ApplyPatch);
        options.web_search_enabled = self.config.features.enabled(Feature::WebSearch);
        options.memory_tool_enabled =
            should_register_remember_tool(self.config.memory_enabled, self.config.moraine_fallback);
        options.vision_config = if self.config.features.enabled(Feature::VisionModel) {
            self.config.vision_config.clone()
        } else {
            None
        };
        options.speech_output_dir = self.config.speech_output_dir.clone();
        options.goal_state = Some(self.config.goal_state.clone());
        options.verify_tool_enabled = self.config.features.enabled(Feature::Verify);
        options
    }

    #[cfg(test)]
    pub(super) fn build_turn_tool_registry_builder(
        &self,
        mode: AppMode,
        todo_list: SharedTodoList,
        plan_state: SharedPlanState,
    ) -> ToolRegistryBuilder {
        self.build_turn_tool_registry_builder_for_route(
            mode,
            self.session.allow_shell,
            self.deepseek_client.clone(),
            &self.session.model,
            todo_list,
            plan_state,
        )
    }

    /// Build the registry from the route and authority already resolved for
    /// this turn. Preview calls this before either is installed on the engine,
    /// so reading `self.session` here would describe the previous turn's shell
    /// posture, client, and model.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_turn_tool_registry_builder_for_route(
        &self,
        mode: AppMode,
        allow_shell: bool,
        client: Option<DeepSeekClient>,
        model: &str,
        todo_list: SharedTodoList,
        plan_state: SharedPlanState,
    ) -> ToolRegistryBuilder {
        let shell_policy = shell_policy_for_mode(mode, allow_shell);
        if mode != AppMode::Plan {
            let mut builder = ToolRegistryBuilder::new().with_agent_runtime_surface(
                client.clone(),
                model.to_string(),
                self.agent_tool_surface_options(shell_policy),
                todo_list,
                plan_state,
            );
            // `start_mcp_server` belongs to every executable mode. Keep its
            // handler aligned with the model catalog, which always loads the
            // tool while MCP is enabled. The former early return registered
            // it only in Plan mode, so Agent/Full Access advertised a tool
            // that could never cross the execution boundary.
            if let Some(ref pool) = self.mcp_pool {
                builder = builder.with_runtime_mcp_tool(Arc::clone(pool));
            }
            return builder;
        }

        let mut builder = {
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
        };

        builder = builder
            .with_review_tool(client, model.to_string())
            .with_user_input_tool()
            .with_parallel_tool();

        if self.config.features.enabled(Feature::WebSearch) {
            builder = builder.with_web_tools();
        }

        // Register the `remember` tool only when the user has opted in to
        // user-memory (#489). Without that opt-in the tool would always
        // fail; surfacing it would just waste catalog slots.
        // TODO(v0.8.71): remove when Moraine recall stable; see #3490, #3495
        if should_register_remember_tool(self.config.memory_enabled, self.config.moraine_fallback) {
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

        // Register the start_mcp_server tool so LLM can dynamically start
        // MCP servers from conversation context. Only when the pool has been
        // initialized (lazy via ensure_mcp_pool).
        if let Some(ref pool) = self.mcp_pool {
            builder = builder.with_runtime_mcp_tool(Arc::clone(pool));
        }

        builder
    }
}

#[cfg(test)]
mod tests {
    use super::should_register_remember_tool;

    #[test]
    fn remember_tool_registration_respects_moraine_fallback() {
        assert!(should_register_remember_tool(true, false));
        assert!(!should_register_remember_tool(false, false));
        assert!(!should_register_remember_tool(true, true));
    }
}
