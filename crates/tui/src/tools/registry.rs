//! Tool registry for managing and executing tools.
//!
//! The registry provides:
//! - Dynamic tool registration
//! - Tool lookup by name
//! - Conversion to API Tool format
//! - Filtering by capability

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use serde_json::Value;

use crate::client::DeepSeekClient;
use crate::models::Tool;

use super::schema_sanitize;
use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

// === Types ===

/// Registry that holds all available tools.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ToolSpec>>,
    context: ToolContext,
    /// Memoised serialised tool catalog. Rebuilt lazily on first
    /// `to_api_tools` call after a mutation; pinned across reads so the
    /// description and schema bytes stay byte-stable for DeepSeek's KV
    /// prefix cache. Invalidated on `register` / `remove` / `clear`.
    api_cache: OnceLock<Vec<Tool>>,
}

impl ToolRegistry {
    /// Create a new empty registry with the given context.
    #[must_use]
    pub fn new(context: ToolContext) -> Self {
        Self {
            tools: HashMap::new(),
            context,
            api_cache: OnceLock::new(),
        }
    }

    /// Register a tool in the registry.
    pub fn register(&mut self, tool: Arc<dyn ToolSpec>) {
        let name = tool.name().to_string();
        if self.tools.insert(name.clone(), tool).is_some() {
            tracing::warn!("Overwriting existing tool: {}", name);
        }
        self.invalidate_api_cache();
    }

    /// Register multiple tools at once.
    pub fn register_all(&mut self, tools: Vec<Arc<dyn ToolSpec>>) {
        for tool in tools {
            self.register(tool);
        }
    }

    /// Get a tool by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn ToolSpec>> {
        self.tools.get(name).cloned()
    }

    /// Check if a tool exists.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Get all registered tool names.
    #[must_use]
    #[allow(dead_code)]
    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(std::string::String::as_str).collect()
    }

    /// Get the number of registered tools.
    #[must_use]
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Check if the registry is empty.
    #[must_use]
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Get all registered tools.
    #[must_use]
    pub fn all(&self) -> Vec<Arc<dyn ToolSpec>> {
        self.tools.values().cloned().collect()
    }

    /// Execute a tool by name with the given input.
    pub async fn execute(&self, name: &str, input: Value) -> Result<String, ToolError> {
        let tool = self
            .get(name)
            .ok_or_else(|| ToolError::not_available(format!("tool '{name}' is not registered")))?;

        let result = tool.execute(input, &self.context).await?;
        Ok(result.content)
    }

    /// Execute a tool by name, returning the full `ToolResult`.
    pub async fn execute_full(&self, name: &str, input: Value) -> Result<ToolResult, ToolError> {
        let tool = self
            .get(name)
            .ok_or_else(|| ToolError::not_available(format!("tool '{name}' is not registered")))?;

        tool.execute(input, &self.context).await
    }

    /// Execute a tool with an optional context override.
    ///
    /// This is used for retrying tools with elevated sandbox policies.
    /// After execution, large results are routed through the workshop (#548).
    pub async fn execute_full_with_context(
        &self,
        name: &str,
        input: Value,
        context_override: Option<&ToolContext>,
    ) -> Result<ToolResult, ToolError> {
        let tool = self
            .get(name)
            .ok_or_else(|| ToolError::not_available(format!("tool '{name}' is not registered")))?;

        let ctx = context_override.unwrap_or(&self.context);
        let result = tool.execute(input.clone(), ctx).await?;

        // Large-output routing (#548): if the result exceeds the threshold and
        // the caller did not request `raw=true`, synthesise via the workshop.
        let raw_bypass = input.get("raw").and_then(|v| v.as_bool()).unwrap_or(false);

        if let Some(router) = ctx.large_output_router.as_ref() {
            use crate::tools::large_output_router::{LargeOutputRouter, RouteDecision};
            match router.route(name, &result, raw_bypass) {
                RouteDecision::PassThrough => {}
                RouteDecision::Synthesise {
                    estimated_tokens,
                    threshold,
                } => {
                    // Store the raw output in the workshop variable store.
                    if let Some(vars_arc) = ctx.workshop_vars.as_ref() {
                        let mut vars = vars_arc.lock().await;
                        vars.store_raw(name, &result.content);
                    }

                    // Build a terse synthesis using the same model the registry
                    // was constructed for (workshop Flash model). For now we
                    // produce a structured header + truncated preview without
                    // a live API call so the engine stays dependency-free at
                    // the registry layer. A follow-up can wire in the Flash
                    // client when the async LLM call is safe here.
                    let preview_chars = 1_200usize;
                    let preview: String = result.content.chars().take(preview_chars).collect();
                    let ellipsis = if result.content.chars().count() > preview_chars {
                        "\n… [output truncated — full text in workshop variable `last_tool_result`]"
                    } else {
                        ""
                    };
                    let synthesis = format!("{preview}{ellipsis}");
                    let wrapped = LargeOutputRouter::wrap_synthesis(
                        name,
                        &synthesis,
                        estimated_tokens,
                        threshold,
                    );
                    tracing::debug!(
                        tool = name,
                        estimated_tokens,
                        threshold,
                        "large-output routed through workshop"
                    );
                    return Ok(ToolResult::success(wrapped));
                }
            }
        }

        Ok(result)
    }

    /// Get the current tool context.
    #[must_use]
    pub fn context(&self) -> &ToolContext {
        &self.context
    }

    /// Convert all tools to API Tool format for sending to the model.
    ///
    /// Output is sorted by tool name for **prefix-cache stability** (#263).
    /// Rust's `HashMap` uses a randomly-seeded hasher per process, so a raw
    /// `self.tools.values()` iteration emits tools in a different order on
    /// every `deepseek` launch, invalidating DeepSeek's KV prefix cache for
    /// every cross-session resume. Sorting here matches the way Claude Code
    /// stabilises its tool array (`assembleToolPool` in their reference).
    ///
    /// The serialised catalog is memoised on first call and pinned across
    /// reads so each tool's `description()` and `input_schema()` are sampled
    /// exactly once per registration. MCP adapters whose upstream description
    /// drifts on reconnect would otherwise rewrite the catalog mid-session
    /// and bust the prefix cache. The cache is invalidated on `register`,
    /// `remove`, and `clear`.
    #[must_use]
    pub fn to_api_tools(&self) -> Vec<Tool> {
        self.api_cache
            .get_or_init(|| self.build_api_tools())
            .clone()
    }

    fn build_api_tools(&self) -> Vec<Tool> {
        let mut tools: Vec<&Arc<dyn ToolSpec>> = self.tools.values().collect();
        tools.sort_by(|a, b| a.name().cmp(b.name()));
        tools
            .into_iter()
            .map(|tool| {
                let mut schema = tool.input_schema();
                schema_sanitize::sanitize(&mut schema);
                Tool {
                    tool_type: None,
                    name: tool.name().to_string(),
                    description: tool.description().to_string(),
                    input_schema: schema,
                    allowed_callers: Some(vec!["direct".to_string()]),
                    defer_loading: Some(tool.defer_loading()),
                    input_examples: None,
                    strict: None,
                    cache_control: None,
                }
            })
            .collect()
    }

    fn invalidate_api_cache(&mut self) {
        self.api_cache = OnceLock::new();
    }

    /// Convert tools to API Tool format with optional cache control on the last tool.
    #[must_use]
    pub fn to_api_tools_with_cache(&self, enable_cache: bool) -> Vec<Tool> {
        let mut tools = self.to_api_tools();
        if enable_cache && let Some(last) = tools.last_mut() {
            last.cache_control = Some(crate::models::CacheControl {
                cache_type: "ephemeral".to_string(),
            });
        }
        tools
    }

    /// Filter tools by capability.
    #[must_use]
    #[allow(dead_code)]
    pub fn filter_by_capability(&self, capability: ToolCapability) -> Vec<Arc<dyn ToolSpec>> {
        self.tools
            .values()
            .filter(|t| t.capabilities().contains(&capability))
            .cloned()
            .collect()
    }

    /// Get read-only tools.
    #[must_use]
    #[allow(dead_code)]
    pub fn read_only_tools(&self) -> Vec<Arc<dyn ToolSpec>> {
        self.tools
            .values()
            .filter(|t| t.is_read_only())
            .cloned()
            .collect()
    }

    /// Get tools that require approval.
    #[must_use]
    #[allow(dead_code)]
    pub fn approval_required_tools(&self) -> Vec<Arc<dyn ToolSpec>> {
        self.tools
            .values()
            .filter(|t| t.approval_requirement() == ApprovalRequirement::Required)
            .cloned()
            .collect()
    }

    /// Get tools that suggest approval.
    #[must_use]
    #[allow(dead_code)]
    pub fn approval_suggested_tools(&self) -> Vec<Arc<dyn ToolSpec>> {
        self.tools
            .values()
            .filter(|t| {
                matches!(
                    t.approval_requirement(),
                    ApprovalRequirement::Suggest | ApprovalRequirement::Required
                )
            })
            .cloned()
            .collect()
    }

    /// Update the context (e.g., when workspace changes).
    #[allow(dead_code)]
    pub fn set_context(&mut self, context: ToolContext) {
        self.context = context;
    }

    /// Get a mutable reference to the current context.
    #[must_use]
    #[allow(dead_code)]
    pub fn context_mut(&mut self) -> &mut ToolContext {
        &mut self.context
    }

    /// Remove a tool by name.
    #[must_use]
    #[allow(dead_code)]
    pub fn remove(&mut self, name: &str) -> Option<Arc<dyn ToolSpec>> {
        let removed = self.tools.remove(name);
        if removed.is_some() {
            self.invalidate_api_cache();
        }
        removed
    }

    /// Resolve a non-canonical tool name to a registered canonical name.
    ///
    /// Runs a deterministic ladder against the registered tool names:
    /// 1. Lowercase exact match.
    /// 2. Hyphens/spaces → underscores (read-file → read_file).
    /// 3. CamelCase → snake_case (ReadFile → read_file).
    /// 4. Strip trailing `_tool` / `-tool` suffix (twice).
    /// 5. Fuzzy match via simple prefix/suffix similarity.
    ///
    /// Returns `None` when no resolution is found (let the caller surface
    /// "Unknown tool").
    #[must_use]
    pub fn resolve(&self, requested: &str) -> Option<&str> {
        let names: Vec<&str> = self.tools.keys().map(String::as_str).collect();
        let lower = requested.to_lowercase();

        // 1. lowercase exact
        if let Some(n) = names.iter().find(|n| n.to_lowercase() == lower) {
            return Some(n);
        }
        // 2. hyphen/space → underscore
        let snaked = lower.replace(['-', ' '], "_");
        if let Some(n) = names.iter().find(|n| **n == snaked) {
            return Some(n);
        }
        // 3. CamelCase → snake_case
        let cc = to_snake_case(requested);
        if let Some(n) = names.iter().find(|n| **n == cc) {
            return Some(n);
        }
        // 4. strip _tool/-tool/tool suffix, twice
        let mut stripped = cc.clone();
        for _ in 0..2 {
            for suf in ["_tool", "-tool", "tool"] {
                if let Some(s) = stripped.strip_suffix(suf) {
                    stripped = s.to_string();
                    break;
                }
            }
        }
        if !stripped.is_empty()
            && let Some(n) = names.iter().find(|n| **n == stripped)
        {
            return Some(n);
        }
        // 5. fuzzy: simple prefix match (at least 3 chars)
        if lower.len() >= 3 {
            for n in &names {
                if n.len() >= 3 && (n.starts_with(&lower) || lower.starts_with(n)) {
                    return Some(n);
                }
            }
        }
        None
    }

    /// Clear all tools from the registry.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.tools.clear();
        self.invalidate_api_cache();
    }
}

/// Builder for constructing a `ToolRegistry` with common tools.
pub struct ToolRegistryBuilder {
    tools: Vec<Arc<dyn ToolSpec>>,
}

impl ToolRegistryBuilder {
    /// Create a new builder.
    #[must_use]
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Add a custom tool.
    #[must_use]
    pub fn with_tool(mut self, tool: Arc<dyn ToolSpec>) -> Self {
        self.tools.push(tool);
        self
    }

    /// Include file tools (read, write, edit, list).
    #[must_use]
    pub fn with_file_tools(self) -> Self {
        use super::file::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
        self.with_tool(Arc::new(ReadFileTool))
            .with_tool(Arc::new(WriteFileTool))
            .with_tool(Arc::new(EditFileTool))
            .with_tool(Arc::new(ListDirTool))
    }

    /// Include only read-only file tools (read, list).
    #[must_use]
    pub fn with_read_only_file_tools(self) -> Self {
        use super::file::{ListDirTool, ReadFileTool};
        self.with_tool(Arc::new(ReadFileTool))
            .with_tool(Arc::new(ListDirTool))
            .with_tool(Arc::new(
                super::tool_result_retrieval::RetrieveToolResultTool,
            ))
    }

    /// Include shell execution tool.
    #[must_use]
    pub fn with_shell_tools(self) -> Self {
        use super::shell::{ExecShellTool, ShellCancelTool, ShellInteractTool, ShellWaitTool};
        self.with_tool(Arc::new(ExecShellTool))
            .with_tool(Arc::new(ShellWaitTool::new("exec_shell_wait")))
            .with_tool(Arc::new(ShellInteractTool::new("exec_shell_interact")))
            .with_tool(Arc::new(ShellCancelTool))
            .with_tool(Arc::new(ShellWaitTool::new("exec_wait")))
            .with_tool(Arc::new(ShellInteractTool::new("exec_interact")))
    }

    /// Include search tools (`grep_files`).
    #[must_use]
    pub fn with_search_tools(self) -> Self {
        use super::file_search::FileSearchTool;
        use super::search::GrepFilesTool;
        self.with_tool(Arc::new(GrepFilesTool))
            .with_tool(Arc::new(FileSearchTool))
    }

    /// Include git inspection tools (`git_status`, `git_diff`).
    #[must_use]
    pub fn with_git_tools(self) -> Self {
        use super::git::{GitDiffTool, GitStatusTool};
        self.with_tool(Arc::new(GitStatusTool))
            .with_tool(Arc::new(GitDiffTool))
    }

    /// Include git history tools (`git_log`, `git_show`, `git_blame`).
    #[must_use]
    pub fn with_git_history_tools(self) -> Self {
        use super::git_history::{GitBlameTool, GitLogTool, GitShowTool};
        self.with_tool(Arc::new(GitLogTool))
            .with_tool(Arc::new(GitShowTool))
            .with_tool(Arc::new(GitBlameTool))
    }

    /// Include workspace diagnostics tool.
    #[must_use]
    pub fn with_diagnostics_tool(self) -> Self {
        use super::diagnostics::DiagnosticsTool;
        self.with_tool(Arc::new(DiagnosticsTool))
    }

    /// Include the `load_skill` tool (#434) so the model can pull a
    /// SKILL.md body + companion file list into context with one
    /// call instead of `read_file` + `list_dir` against the path
    /// shown in the system prompt's `## Skills` section.
    #[must_use]
    pub fn with_skill_tools(self) -> Self {
        use super::skill::LoadSkillTool;
        self.with_tool(Arc::new(LoadSkillTool))
    }

    /// Include project mapping tools.
    #[must_use]
    pub fn with_project_tools(self) -> Self {
        use super::project::ProjectMapTool;
        self.with_tool(Arc::new(ProjectMapTool))
    }

    /// Include cargo test runner tool.
    #[must_use]
    pub fn with_test_runner_tool(self) -> Self {
        use super::test_runner::RunTestsTool;
        self.with_tool(Arc::new(RunTestsTool))
    }

    /// Include structured data validation tool (`validate_data`).
    #[must_use]
    pub fn with_validation_tools(self) -> Self {
        use super::validate_data::ValidateDataTool;
        self.with_tool(Arc::new(ValidateDataTool))
    }

    /// Include retrieval for spilled historical tool results.
    #[must_use]
    pub fn with_tool_result_retrieval_tool(self) -> Self {
        use super::tool_result_retrieval::RetrieveToolResultTool;
        self.with_tool(Arc::new(RetrieveToolResultTool))
    }

    /// Include durable task, gate, PR-attempt, GitHub, and automation tools.
    #[must_use]
    pub fn with_runtime_task_tools(self) -> Self {
        use super::automation::{
            AutomationCreateTool, AutomationDeleteTool, AutomationListTool, AutomationPauseTool,
            AutomationReadTool, AutomationResumeTool, AutomationRunTool, AutomationUpdateTool,
        };
        use super::github::{
            GithubCloseIssueTool, GithubCommentTool, GithubIssueContextTool, GithubPrContextTool,
        };
        use super::tasks::{
            PrAttemptListTool, PrAttemptPreflightTool, PrAttemptReadTool, PrAttemptRecordTool,
            TaskCancelTool, TaskCreateTool, TaskGateRunTool, TaskListTool, TaskReadTool,
            TaskShellStartTool, TaskShellWaitTool,
        };

        self.with_tool(Arc::new(TaskCreateTool))
            .with_tool(Arc::new(TaskListTool))
            .with_tool(Arc::new(TaskReadTool))
            .with_tool(Arc::new(TaskCancelTool))
            .with_tool(Arc::new(TaskGateRunTool))
            .with_tool(Arc::new(TaskShellStartTool))
            .with_tool(Arc::new(TaskShellWaitTool))
            .with_tool(Arc::new(GithubIssueContextTool))
            .with_tool(Arc::new(GithubPrContextTool))
            .with_tool(Arc::new(PrAttemptRecordTool))
            .with_tool(Arc::new(PrAttemptListTool))
            .with_tool(Arc::new(PrAttemptReadTool))
            .with_tool(Arc::new(PrAttemptPreflightTool))
            .with_tool(Arc::new(AutomationCreateTool))
            .with_tool(Arc::new(AutomationListTool))
            .with_tool(Arc::new(AutomationReadTool))
            .with_tool(Arc::new(AutomationUpdateTool))
            .with_tool(Arc::new(AutomationPauseTool))
            .with_tool(Arc::new(AutomationResumeTool))
            .with_tool(Arc::new(AutomationDeleteTool))
            .with_tool(Arc::new(AutomationRunTool))
            .with_tool(Arc::new(GithubCommentTool))
            .with_tool(Arc::new(GithubCloseIssueTool))
    }

    /// Include only read-only durable task, PR-attempt, GitHub, and automation
    /// inspection tools. Plan mode uses this surface so it can observe state
    /// without starting work, changing remotes, or mutating automation config.
    #[must_use]
    pub fn with_runtime_read_only_task_tools(self) -> Self {
        use super::automation::{AutomationListTool, AutomationReadTool};
        use super::github::{GithubIssueContextTool, GithubPrContextTool};
        use super::tasks::{PrAttemptListTool, PrAttemptReadTool, TaskListTool, TaskReadTool};

        self.with_tool(Arc::new(TaskListTool))
            .with_tool(Arc::new(TaskReadTool))
            .with_tool(Arc::new(GithubIssueContextTool))
            .with_tool(Arc::new(GithubPrContextTool))
            .with_tool(Arc::new(PrAttemptListTool))
            .with_tool(Arc::new(PrAttemptReadTool))
            .with_tool(Arc::new(AutomationListTool))
            .with_tool(Arc::new(AutomationReadTool))
    }

    /// Include web search tools.
    #[must_use]
    pub fn with_web_tools(self) -> Self {
        use super::fetch_url::FetchUrlTool;
        use super::finance::FinanceTool;
        use super::web_run::WebRunTool;
        use super::web_search::WebSearchTool;
        self.with_tool(Arc::new(WebSearchTool))
            .with_tool(Arc::new(FetchUrlTool))
            .with_tool(Arc::new(FinanceTool::new()))
            .with_tool(Arc::new(WebRunTool))
    }

    /// Previously registered the OpenAI-style `multi_tool_use.parallel`
    /// meta-tool. DeepSeek-V4 has native parallel tool calls (multiple
    /// `tool_calls` entries in one assistant turn) and the meta-tool name
    /// triggered the model to hallucinate OpenAI-internal XML wrappers
    /// (`<multi_tool_use.parallel><tool_name>…</tool_name>…`) instead of
    /// emitting native calls. Kept as a no-op so existing callers compile;
    /// the engine's compatibility dispatcher still handles legacy emissions.
    #[must_use]
    pub fn with_parallel_tool(self) -> Self {
        self
    }

    /// Include request_user_input tool.
    #[must_use]
    pub fn with_user_input_tool(self) -> Self {
        use super::user_input::RequestUserInputTool;
        self.with_tool(Arc::new(RequestUserInputTool))
    }

    /// Include patch tools (`apply_patch`).
    #[must_use]
    pub fn with_patch_tools(self) -> Self {
        use super::apply_patch::ApplyPatchTool;
        self.with_tool(Arc::new(ApplyPatchTool))
    }

    /// Include the `revert_turn` tool. Approval-gated since it mutates
    /// the workspace; the model uses it when the user asks to "undo my
    /// last edit". Backed by the per-workspace snapshot side-repo
    /// (`crate::snapshot`).
    #[must_use]
    pub fn with_revert_turn_tool(self) -> Self {
        use super::revert_turn::RevertTurnTool;
        self.with_tool(Arc::new(RevertTurnTool))
    }

    /// Include the RLM tool (`rlm`). Runs the full recursive language-model
    /// loop on a long input (file or inline content); the long input never
    /// enters the calling model's context window. The Python REPL exposes
    /// `llm_query` / `llm_query_batched` / `rlm_query` / `rlm_query_batched`
    /// helpers for sub-LLM work — that's where parallel fan-out belongs.
    #[must_use]
    pub fn with_rlm_tool(self, client: Option<DeepSeekClient>, root_model: String) -> Self {
        use super::rlm::RlmTool;
        self.with_tool(Arc::new(RlmTool::new(client, root_model)))
    }

    /// Include the review tool.
    #[must_use]
    pub fn with_review_tool(self, client: Option<DeepSeekClient>, model: String) -> Self {
        use super::review::ReviewTool;
        self.with_tool(Arc::new(ReviewTool::new(client, model)))
    }

    /// Include the `recall_archive` tool — searches prior cycle archives
    /// produced by the checkpoint-restart system (issue #127).
    #[must_use]
    pub fn with_recall_archive_tool(self) -> Self {
        use super::recall_archive::RecallArchiveTool;
        self.with_tool(Arc::new(RecallArchiveTool))
    }

    /// Include note tool.
    #[must_use]
    pub fn with_note_tool(self) -> Self {
        use super::shell::NoteTool;
        self.with_tool(Arc::new(NoteTool))
    }

    /// Include the FIM (Fill-in-the-Middle) edit tool.
    #[must_use]
    pub fn with_fim_tool(self, client: Option<DeepSeekClient>, model: String) -> Self {
        use super::fim::FimEditTool;
        self.with_tool(Arc::new(FimEditTool::new(client, model)))
    }

    /// Include the `remember` tool — model-callable bullet-add into the
    /// user memory file (#489). Only register when the user has opted
    /// in to the memory feature; without that, the tool would surface
    /// in the model's catalog but always fail with "memory disabled".
    #[must_use]
    pub fn with_remember_tool(self) -> Self {
        use super::remember::RememberTool;
        self.with_tool(Arc::new(RememberTool))
    }

    /// Include the `notify` tool — model-callable desktop notification
    /// (#1322). Routes through the existing `tui::notifications` OSC 9 /
    /// BEL pipeline so the user's `[notifications].method` config is
    /// honoured automatically (including `off`). Always safe to register
    /// because the tool has no side effects beyond a single terminal
    /// escape write.
    #[must_use]
    pub fn with_notify_tool(self) -> Self {
        use super::notify::NotifyTool;
        self.with_tool(Arc::new(NotifyTool))
    }

    /// Include MCP tools from a connected pool as first-class registry
    /// citizens. Each MCP tool is wrapped in a lightweight adapter that
    /// implements `ToolSpec`, so the unified `ToolRegistryBuilder` flow
    /// handles them alongside native tools.
    ///
    /// MCP tools are marked `defer_loading` by default (except discovery
    /// helpers) to keep the model-visible catalog compact.
    #[must_use]
    #[allow(dead_code)]
    pub fn with_mcp_tools(
        mut self,
        mcp_pool: std::sync::Arc<tokio::sync::Mutex<crate::mcp::McpPool>>,
    ) -> Self {
        // Snapshot the current tool list from the pool (non-blocking).
        // The adapter lazily resolves at execution time via the pool.
        if let Ok(pool) = mcp_pool.try_lock() {
            for (name, tool) in pool.all_tools() {
                let adapter = Arc::new(McpToolAdapter {
                    name: name.clone(),
                    tool: tool.clone(),
                    pool: mcp_pool.clone(),
                });
                self.tools.push(adapter);
            }
        }
        self
    }

    /// Include all agent tools (file tools + shell + note + search + patch).
    #[must_use]
    pub fn with_agent_tools(self, allow_shell: bool) -> Self {
        let builder = self
            .with_file_tools()
            .with_note_tool()
            .with_search_tools()
            .with_web_tools()
            .with_user_input_tool()
            .with_parallel_tool()
            .with_patch_tools()
            .with_git_tools()
            .with_git_history_tools()
            .with_diagnostics_tool()
            .with_project_tools()
            .with_skill_tools()
            .with_test_runner_tool()
            .with_validation_tools()
            .with_tool_result_retrieval_tool()
            .with_runtime_task_tools()
            .with_revert_turn_tool();

        if allow_shell {
            builder.with_shell_tools()
        } else {
            builder
        }
    }

    /// Include the full agent tool surface: every tool family the parent gets
    /// in Agent mode, including review, RLM, and the sub-agent management
    /// family (so children can recurse). Used by both the parent's Agent-mode
    /// registry build (`core/engine.rs`) and by every sub-agent
    /// (`subagent::SubAgentToolRegistry`) — keeping them in lockstep.
    ///
    /// `allow_shell` mirrors the session's shell permission. `manager` and
    /// `runtime` are the sub-agent runtime — children pass through their own
    /// runtime so grandchildren can spawn within the same depth/cancellation
    /// envelope.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn with_full_agent_surface(
        self,
        client: Option<DeepSeekClient>,
        model: String,
        manager: super::subagent::SharedSubAgentManager,
        runtime: super::subagent::SubAgentRuntime,
        allow_shell: bool,
        todo_list: super::todo::SharedTodoList,
        plan_state: super::plan::SharedPlanState,
    ) -> Self {
        self.with_agent_tools(allow_shell)
            .with_todo_tool(todo_list)
            .with_plan_tool(plan_state)
            .with_review_tool(client.clone(), model.clone())
            .with_rlm_tool(client, model)
            .with_recall_archive_tool()
            .with_subagent_tools(manager, runtime)
    }

    /// Include the todo tool with a shared `TodoList`.
    #[must_use]
    pub fn with_todo_tool(self, todo_list: super::todo::SharedTodoList) -> Self {
        use super::todo::{TodoAddTool, TodoListTool, TodoUpdateTool, TodoWriteTool};
        self.with_tool(Arc::new(TodoWriteTool::checklist(todo_list.clone())))
            .with_tool(Arc::new(TodoAddTool::checklist(todo_list.clone())))
            .with_tool(Arc::new(TodoUpdateTool::checklist(todo_list.clone())))
            .with_tool(Arc::new(TodoListTool::checklist(todo_list.clone())))
            .with_tool(Arc::new(TodoWriteTool::new(todo_list.clone())))
            .with_tool(Arc::new(TodoAddTool::new(todo_list.clone())))
            .with_tool(Arc::new(TodoUpdateTool::new(todo_list.clone())))
            .with_tool(Arc::new(TodoListTool::new(todo_list)))
    }

    /// Include the plan tool with a shared `PlanState`.
    #[must_use]
    pub fn with_plan_tool(self, plan_state: super::plan::SharedPlanState) -> Self {
        use super::plan::UpdatePlanTool;
        self.with_tool(Arc::new(UpdatePlanTool::new(plan_state)))
    }

    /// Include sub-agent management tools.
    #[must_use]
    pub fn with_subagent_tools(
        self,
        manager: super::subagent::SharedSubAgentManager,
        runtime: super::subagent::SubAgentRuntime,
    ) -> Self {
        use super::subagent::{
            AgentAssignTool, AgentCancelTool, AgentCloseTool, AgentListTool, AgentResultTool,
            AgentResumeTool, AgentSendInputTool, AgentSpawnTool, AgentWaitTool,
            DelegateToAgentTool,
        };

        self.with_tool(Arc::new(AgentSpawnTool::new(
            manager.clone(),
            runtime.clone(),
        )))
        .with_tool(Arc::new(AgentSpawnTool::with_name(
            manager.clone(),
            runtime.clone(),
            "spawn_agent",
        )))
        .with_tool(Arc::new(DelegateToAgentTool::new(
            manager.clone(),
            runtime.clone(),
        )))
        .with_tool(Arc::new(AgentResultTool::new(manager.clone())))
        .with_tool(Arc::new(AgentSendInputTool::new(
            manager.clone(),
            "send_input",
        )))
        .with_tool(Arc::new(AgentAssignTool::new(
            manager.clone(),
            "agent_assign",
        )))
        .with_tool(Arc::new(AgentAssignTool::new(
            manager.clone(),
            "assign_agent",
        )))
        .with_tool(Arc::new(AgentWaitTool::new(manager.clone(), "wait")))
        .with_tool(Arc::new(AgentSendInputTool::new(
            manager.clone(),
            "agent_send_input",
        )))
        .with_tool(Arc::new(AgentWaitTool::new(manager.clone(), "agent_wait")))
        .with_tool(Arc::new(AgentResumeTool::new(
            manager.clone(),
            runtime.clone(),
        )))
        .with_tool(Arc::new(AgentCloseTool::new(manager.clone())))
        .with_tool(Arc::new(AgentCancelTool::new(manager.clone())))
        .with_tool(Arc::new(AgentListTool::new(manager)))
    }

    /// Build the registry with the given context.
    #[must_use]
    pub fn build(self, context: ToolContext) -> ToolRegistry {
        let mut registry = ToolRegistry::new(context);
        registry.register_all(self.tools);
        registry
    }
}

impl Default for ToolRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert CamelCase to snake_case.
fn to_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Adapter that wraps an MCP tool definition so it can live in the
/// unified `ToolRegistry` alongside native tools (§5.B).
#[allow(dead_code)]
struct McpToolAdapter {
    name: String,
    tool: crate::mcp::McpTool,
    pool: std::sync::Arc<tokio::sync::Mutex<crate::mcp::McpPool>>,
}

#[async_trait::async_trait]
impl ToolSpec for McpToolAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        // McpTool.description is Option<String>; fall back to the
        // prefixed name when absent.
        self.tool.description.as_deref().unwrap_or(&self.name)
    }

    fn input_schema(&self) -> Value {
        self.tool.input_schema.clone()
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        // Conservatively treat MCP tools as requiring approval and
        // network access unless they're known discovery helpers.
        let name_lower = self.name.to_lowercase();
        if name_lower.contains("list_mcp")
            || name_lower.contains("read_mcp")
            || name_lower.contains("mcp_read")
            || name_lower.contains("mcp_get_prompt")
        {
            vec![ToolCapability::ReadOnly]
        } else {
            vec![ToolCapability::Network, ToolCapability::RequiresApproval]
        }
    }

    fn defer_loading(&self) -> bool {
        // Discovery helpers stay loaded; everything else is deferred.
        let keep_loaded = matches!(
            self.name.as_str(),
            "list_mcp_resources"
                | "list_mcp_resource_templates"
                | "mcp_read_resource"
                | "read_mcp_resource"
                | "mcp_get_prompt"
        );
        !keep_loaded
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let mut pool = self.pool.lock().await;
        let result = pool
            .call_tool(&self.name, input)
            .await
            .map_err(|e| ToolError::execution_failed(format!("MCP tool failed: {e}")))?;
        let content = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
        Ok(ToolResult::success(content))
    }
}

// === Unit Tests ===

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{Value, json};
    use tempfile::tempdir;

    use crate::tools::ToolRegistryBuilder;
    use crate::tools::spec::{
        ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, required_str,
    };

    use super::ToolRegistry;

    /// A simple test tool for unit testing
    struct TestTool {
        name: String,
        description: String,
    }

    #[async_trait::async_trait]
    impl ToolSpec for TestTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            &self.description
        }

        fn input_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            })
        }

        fn capabilities(&self) -> Vec<ToolCapability> {
            vec![ToolCapability::ReadOnly]
        }

        async fn execute(
            &self,
            input: Value,
            _context: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            let message = required_str(&input, "message")?;
            Ok(ToolResult::success(format!("Echo: {message}")))
        }
    }

    fn make_test_tool(name: &str) -> Arc<TestTool> {
        Arc::new(TestTool {
            name: name.to_string(),
            description: "A test tool".to_string(),
        })
    }

    #[test]
    fn test_registry_register_and_get() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        let tool = make_test_tool("test_tool");
        registry.register(tool);

        assert!(registry.contains("test_tool"));
        assert!(!registry.contains("nonexistent"));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_registry_names() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("tool_a"));
        registry.register(make_test_tool("tool_b"));

        let names = registry.names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"tool_a"));
        assert!(names.contains(&"tool_b"));
    }

    #[test]
    fn test_registry_to_api_tools() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("my_tool"));

        let api_tools = registry.to_api_tools();
        assert_eq!(api_tools.len(), 1);
        assert_eq!(api_tools[0].name, "my_tool");
        assert_eq!(api_tools[0].description, "A test tool");
    }

    #[test]
    fn api_tools_with_cache_marks_last_tool_ephemeral() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("tool_a"));
        registry.register(make_test_tool("tool_b"));

        let api_tools = registry.to_api_tools_with_cache(true);
        assert_eq!(api_tools.len(), 2);
        assert!(api_tools[0].cache_control.is_none());
        assert_eq!(
            api_tools[1]
                .cache_control
                .as_ref()
                .map(|c| c.cache_type.as_str()),
            Some("ephemeral")
        );
    }

    /// Tool whose `description()` advances through a script of pre-built
    /// strings, one per call. Used to demonstrate that the api-tools cache
    /// pins the description bytes on first read instead of re-sampling them
    /// each turn (#263 follow-up; mirrors reference-cc's `getToolSchemaCache`).
    struct VaryingDescriptionTool {
        name: String,
        descriptions: Vec<String>,
        next: std::sync::atomic::AtomicUsize,
    }

    impl VaryingDescriptionTool {
        fn new(name: &str, descriptions: &[&str]) -> Self {
            Self {
                name: name.to_string(),
                descriptions: descriptions.iter().map(|s| (*s).to_string()).collect(),
                next: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl ToolSpec for VaryingDescriptionTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            let idx = self
                .next
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                .min(self.descriptions.len() - 1);
            &self.descriptions[idx]
        }

        fn input_schema(&self) -> Value {
            json!({"type": "object", "properties": {}, "required": []})
        }

        fn capabilities(&self) -> Vec<ToolCapability> {
            vec![ToolCapability::ReadOnly]
        }

        async fn execute(
            &self,
            _input: Value,
            _context: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::success("ok".to_string()))
        }
    }

    #[test]
    fn to_api_tools_pins_description_bytes_across_calls() {
        // Regression for the cache-stability follow-up: an MCP adapter that
        // returns a different `description()` on reconnect (or any other
        // tool whose description isn't a `&'static str`) would otherwise
        // rewrite the catalog bytes mid-session and miss the prefix cache.
        // The registry pins the first call's value until it's mutated.
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);
        registry.register(Arc::new(VaryingDescriptionTool::new(
            "varying",
            &["first description", "second description"],
        )));

        let first = registry.to_api_tools();
        let second = registry.to_api_tools();

        assert_eq!(first.len(), 1);
        assert_eq!(first[0].description, "first description");
        assert_eq!(
            first, second,
            "api-tools catalog must be byte-identical across reads with no mutation in between"
        );
    }

    #[test]
    fn register_invalidates_api_tools_cache() {
        // Counter-test: when a real change happens (a new tool registers,
        // an existing one is removed, or `clear` is called), the cache must
        // be discarded so the next read reflects the live registry.
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);
        registry.register(Arc::new(VaryingDescriptionTool::new(
            "varying",
            &["first description", "second description"],
        )));

        let before = registry.to_api_tools();
        assert_eq!(before.len(), 1);

        registry.register(make_test_tool("late_arrival"));

        let after = registry.to_api_tools();
        assert_eq!(after.len(), 2, "cache must rebuild after register");
        assert!(after.iter().any(|t| t.name == "varying"));
        assert!(after.iter().any(|t| t.name == "late_arrival"));
        // The varying tool's description advances on cache rebuild — the
        // first read above sampled `first description`; this rebuild samples
        // `second description`. The point is just that the bytes *can*
        // change after a real mutation, not that they always do.
        let varying_after = after
            .iter()
            .find(|t| t.name == "varying")
            .expect("varying tool present");
        assert_eq!(varying_after.description, "second description");
    }

    #[test]
    fn remove_and_clear_invalidate_api_tools_cache() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);
        registry.register(make_test_tool("alpha"));
        registry.register(make_test_tool("beta"));

        let before = registry.to_api_tools();
        assert_eq!(before.len(), 2);

        let _ = registry.remove("alpha");
        let after_remove = registry.to_api_tools();
        assert_eq!(after_remove.len(), 1);
        assert_eq!(after_remove[0].name, "beta");

        registry.clear();
        let after_clear = registry.to_api_tools();
        assert!(after_clear.is_empty(), "cache must clear with the registry");
    }

    #[test]
    fn to_api_tools_emits_alphabetical_order_regardless_of_registration_order() {
        // Regression for #263: HashMap iteration is non-deterministic across
        // process launches, which busts DeepSeek's KV prefix cache for every
        // cross-session resume. `to_api_tools` must emit by name regardless
        // of registration order so two consecutive calls (and two distinct
        // launches) produce byte-identical output.
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let order_a = {
            let mut registry = ToolRegistry::new(ctx.clone());
            registry.register(make_test_tool("zebra"));
            registry.register(make_test_tool("alpha"));
            registry.register(make_test_tool("mango"));
            registry
                .to_api_tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>()
        };

        let order_b = {
            let mut registry = ToolRegistry::new(ctx.clone());
            registry.register(make_test_tool("alpha"));
            registry.register(make_test_tool("mango"));
            registry.register(make_test_tool("zebra"));
            registry
                .to_api_tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>()
        };

        assert_eq!(order_a, vec!["alpha", "mango", "zebra"]);
        assert_eq!(order_a, order_b);
    }

    #[test]
    fn test_registry_remove() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("removable"));
        assert!(registry.contains("removable"));

        let _ = registry.remove("removable");
        assert!(!registry.contains("removable"));
    }

    #[test]
    fn test_registry_clear() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("tool1"));
        registry.register(make_test_tool("tool2"));
        assert_eq!(registry.len(), 2);

        registry.clear();
        assert!(registry.is_empty());
    }

    #[tokio::test]
    async fn test_registry_execute() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("echo"));

        let result = registry
            .execute("echo", json!({"message": "hello"}))
            .await
            .expect("execute");

        assert_eq!(result, "Echo: hello");
    }

    #[tokio::test]
    async fn test_registry_execute_unknown_tool() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let registry = ToolRegistry::new(ctx);

        let result = registry.execute("nonexistent", json!({})).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_builder_basic() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new()
            .with_tool(make_test_tool("custom"))
            .build(ctx);

        assert!(registry.contains("custom"));
    }

    #[test]
    fn test_filter_by_capability() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("readonly_tool"));

        let readonly = registry.filter_by_capability(ToolCapability::ReadOnly);
        assert_eq!(readonly.len(), 1);

        let writes = registry.filter_by_capability(ToolCapability::WritesFiles);
        assert_eq!(writes.len(), 0);
    }

    #[test]
    fn test_read_only_tools() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("reader"));

        let readonly = registry.read_only_tools();
        assert_eq!(readonly.len(), 1);
        assert_eq!(readonly[0].name(), "reader");
    }

    #[test]
    fn test_builder_with_web_tools_includes_finance() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new().with_web_tools().build(ctx);

        assert!(registry.contains("finance"));
    }

    #[test]
    fn test_builder_with_agent_tools_includes_finance() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new()
            .with_agent_tools(false)
            .build(ctx);

        assert!(registry.contains("finance"));
    }
}
