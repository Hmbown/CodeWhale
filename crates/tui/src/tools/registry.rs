//! Tool registry for managing and executing tools.
//!
//! The registry provides:
//! - Dynamic tool registration
//! - Tool lookup by name
//! - Conversion to API Tool format
//! - Filtering by capability

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use std::path::{Path, PathBuf};

use codewhale_protocol::runtime::DynamicToolSpec;
use serde_json::Value;

use crate::client::DeepSeekClient;
use crate::models::Tool;
use crate::tools::goal::SharedGoalState;

use super::schema_canonicalize;
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
    /// prefix cache. Invalidated on `register` / `remove_tool`.
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
    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(std::string::String::as_str).collect()
    }

    /// Get all registered tools.
    #[must_use]
    pub fn all(&self) -> Vec<Arc<dyn ToolSpec>> {
        self.tools.values().cloned().collect()
    }

    /// Execute a tool by name, returning the full `ToolResult`.
    pub async fn execute_full(&self, name: &str, input: Value) -> Result<ToolResult, ToolError> {
        let tool = self
            .get(name)
            .ok_or_else(|| ToolError::not_available(format!("tool '{name}' is not registered")))?;

        enforce_tool_authority(name, &input, tool.as_ref(), &self.context)?;
        tool.execute(input, &self.context).await
    }

    /// Execute a tool with an optional context override.
    ///
    /// This is used for retrying tools with elevated sandbox policies.
    /// After execution, results are stamped with adaptive evidence routing.
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
        enforce_tool_authority(name, &input, tool.as_ref(), ctx)?;
        let mut result = tool.execute(input.clone(), ctx).await?;

        // Adaptive evidence routing (#4619) is storage-free here because this
        // layer does not own a call id. The engine/subagent completion boundary
        // publishes the exact artifact. Classic workshop previews remain an
        // explicit local rollback path.
        let raw_bypass = input.get("raw").and_then(|v| v.as_bool()).unwrap_or(false);

        if let Some(router) = ctx.large_output_router.as_ref() {
            use crate::tools::large_output_router::{
                LargeOutputRouter, RouteDecision, classic_output_routing_enabled,
            };
            if !classic_output_routing_enabled() {
                let (routing, estimated_tokens, threshold) =
                    router.evidence_routing(name, &result, raw_bypass);
                let metadata = result.metadata.get_or_insert_with(|| serde_json::json!({}));
                if let Some(object) = metadata.as_object_mut() {
                    object.insert(
                        "evidence_routing".to_string(),
                        serde_json::to_value(routing)
                            .unwrap_or_else(|_| serde_json::json!("inline")),
                    );
                    object.insert(
                        "evidence_estimated_tokens".to_string(),
                        estimated_tokens.into(),
                    );
                    object.insert("evidence_threshold_tokens".to_string(), threshold.into());
                }
                return Ok(result);
            }
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
            .filter(|tool| tool.model_visible())
            .map(|tool| {
                let mut schema = tool.input_schema();
                schema_sanitize::sanitize(&mut schema);
                schema_canonicalize::canonicalize_schema(&mut schema);
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

    /// Flatten every registered tool into the exact facts the read-only
    /// request projection is allowed to report: name, description, model
    /// visibility, declared capabilities, declared approval requirement, and
    /// whether the tool came from the plugin surface.
    ///
    /// This hands out *data*, never tool objects, so the projection layer
    /// cannot execute anything. Output is sorted by name and does not touch the
    /// registry's own ordering or the memoised API catalog.
    #[must_use]
    pub fn registry_facts(
        &self,
        plugin_names: &std::collections::HashSet<String>,
    ) -> Vec<crate::tool_inspection::RegistryFacts> {
        let mut facts: Vec<crate::tool_inspection::RegistryFacts> = self
            .tools
            .values()
            .map(|tool| crate::tool_inspection::RegistryFacts {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                model_visible: tool.model_visible(),
                capabilities: tool
                    .capabilities()
                    .iter()
                    .map(|capability| format!("{capability:?}"))
                    .collect(),
                approval: format!("{:?}", tool.approval_requirement()),
                plugin: plugin_names.contains(tool.name()),
            })
            .collect();
        facts.sort_by(|a, b| a.name.cmp(&b.name));
        facts
    }

    /// Resolve a non-canonical tool name to a registered canonical name.
    ///
    /// Runs a deterministic ladder against the registered tool names:
    /// 1. Lowercase exact match.
    /// 2. Hyphens/spaces → underscores (read-file → read_file).
    /// 3. CamelCase → snake_case (ReadFile → read_file).
    /// 4. Strip trailing `_tool` / `-tool` suffix (twice).
    ///
    /// Returns `None` when no normalization matches (the caller surfaces
    /// "Unknown tool … did you mean: …"). There is deliberately **no fuzzy
    /// step**: a prefix guess over the registry would execute an arbitrary
    /// sibling tool the model never asked for (#5123-class) — a hallucinated
    /// name must fail, never dispatch.
    #[must_use]
    pub fn resolve(&self, requested: &str) -> Option<&str> {
        let names: Vec<&str> = self.tools.keys().map(String::as_str).collect();
        let lower = requested.to_lowercase();

        // 1. ASCII case-insensitive exact
        if let Some(n) = names.iter().find(|n| n.eq_ignore_ascii_case(requested)) {
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
        None
    }

    /// Remove a tool from the registry by name. Returns `true` if the tool
    /// was present and removed, `false` if no tool with that name existed.
    pub fn remove_tool(&mut self, name: &str) -> bool {
        let existed = self.tools.remove(name).is_some();
        if existed {
            self.invalidate_api_cache();
        }
        existed
    }

    /// Apply config.toml tool overrides to this registry.
    ///
    /// For each entry in `overrides`:
    /// - `Disabled` removes the tool.
    /// - `Script` / `Command` replaces the tool with the user's implementation.
    ///
    /// `plugin_dir` is used as the base for relative script paths.
    pub fn apply_overrides(
        &mut self,
        overrides: &std::collections::HashMap<String, crate::config::ToolOverride>,
        plugin_dir: &Path,
    ) {
        for (tool_name, override_cfg) in overrides {
            match override_cfg {
                crate::config::ToolOverride::Disabled => {
                    if self.remove_tool(tool_name) {
                        tracing::info!("Tool '{}' disabled via config override", tool_name);
                    } else {
                        tracing::warn!("Cannot disable tool '{}': not registered", tool_name);
                    }
                }
                _ => {
                    // Script and Command overrides create replacement tools.
                    use crate::tools::plugin::tool_from_override;
                    match tool_from_override(tool_name, override_cfg, plugin_dir) {
                        Some(replacement) => {
                            self.register(replacement);
                            tracing::info!("Tool '{}' replaced via config override", tool_name);
                        }
                        None => {
                            if self.remove_tool(tool_name) {
                                tracing::warn!(
                                    "Tool '{}' override did not create a replacement; removed the original tool to avoid override fallthrough",
                                    tool_name
                                );
                            } else {
                                tracing::warn!(
                                    "Tool '{}' override did not create a replacement and no registered tool existed",
                                    tool_name
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// Load and register plugin tools from a directory.
    ///
    /// Each script with valid frontmatter (`# name:`, `# description:`, etc.)
    /// becomes a registered `ScriptPluginTool`. Tools whose name matches an
    /// already-registered tool will overwrite it.
    pub fn load_plugins(&mut self, plugin_dir: &Path) {
        if !plugin_dir.exists() {
            tracing::debug!(
                "Plugin directory {} does not exist, skipping",
                plugin_dir.display()
            );
            return;
        }
        let plugins = crate::tools::plugin::load_plugin_tools(plugin_dir);
        let count = plugins.len();
        for tool in plugins {
            self.register(tool);
        }
        if count > 0 {
            tracing::info!(
                "Loaded {count} plugin tool(s) from {}",
                plugin_dir.display()
            );
        }
    }
}

fn enforce_tool_authority(
    name: &str,
    input: &Value,
    tool: &dyn ToolSpec,
    context: &ToolContext,
) -> Result<(), ToolError> {
    let Some(authority) = context.tool_authority.as_ref() else {
        return Ok(());
    };
    let capabilities = tool.capabilities();
    if matches!(name, "Bash" | "exec_shell" | "Run") {
        return Err(ToolError::permission_denied(format!(
            "worker '{}' cannot run {name}: arbitrary command execution is outside its machine-readable authority envelope",
            authority.owner
        )));
    }
    if name == "Git" || name.starts_with("git_") || name == "review" {
        return Err(ToolError::permission_denied(format!(
            "worker '{}' cannot run {name}: repository-configured Git helpers cannot prove read-only execution under its machine-readable authority envelope",
            authority.owner
        )));
    }
    if tool.is_read_only_for(input) {
        return Ok(());
    }
    if capabilities.contains(&ToolCapability::ExecutesCode) {
        return Err(ToolError::permission_denied(format!(
            "worker '{}' cannot run {name}: code or child execution is outside its machine-readable authority envelope",
            authority.owner
        )));
    }
    if let Some(paths) = authority_mutation_paths(name, input)? {
        if paths.is_empty() {
            return Err(ToolError::permission_denied(format!(
                "worker '{}' mutation through {name} did not expose a bounded file target",
                authority.owner
            )));
        }
        for path in paths {
            if !authority.permits_mutation_path(context, &path)? {
                return Err(ToolError::permission_denied(format!(
                    "worker '{}' cannot mutate '{path}' outside its machine-readable authority envelope",
                    authority.owner
                )));
            }
        }
        return Ok(());
    }
    Err(ToolError::permission_denied(format!(
        "worker '{}' cannot run mutating tool {name}: the call has no authorized file target",
        authority.owner
    )))
}

fn authority_mutation_paths(name: &str, input: &Value) -> Result<Option<Vec<String>>, ToolError> {
    let is_patch = name == "apply_patch"
        || (name == "File" && input.get("action").and_then(Value::as_str) == Some("patch"));
    if is_patch {
        let mut patch_input = input.clone();
        if let Some(object) = patch_input.as_object_mut() {
            object.remove("action");
        }
        let paths = crate::tools::apply_patch::preflight_apply_patch(&patch_input)
            .map_err(|error| ToolError::invalid_input(error.to_string()))?
            .touched_files;
        return Ok(Some(paths));
    }
    let path_bound = matches!(name, "write_file" | "edit_file" | "fim_edit")
        || (name == "File"
            && input
                .get("action")
                .and_then(Value::as_str)
                .is_some_and(|action| matches!(action, "write" | "edit")))
        || (name == "pandoc_convert" && input.get("output_path").is_some());
    if !path_bound {
        return Ok(None);
    }
    Ok(Some(
        input
            .get("path")
            .or_else(|| input.get("output_path"))
            .and_then(Value::as_str)
            .map(|path| vec![path.to_string()])
            .unwrap_or_default(),
    ))
}

/// Builder for constructing a `ToolRegistry` with common tools.
pub struct ToolRegistryBuilder {
    tools: Vec<Arc<dyn ToolSpec>>,
}

/// Feature/config-dependent native Agent-mode tool surface.
///
/// Parent Agent/Yolo turns and default child sub-agents both build through this
/// options object so the catalog does not drift as new first-party tools are
/// gated behind feature flags or config state.
#[derive(Clone)]
pub struct AgentToolSurfaceOptions {
    pub shell_policy: crate::worker_profile::ShellPolicy,
    pub apply_patch_enabled: bool,
    pub web_search_enabled: bool,
    pub memory_tool_enabled: bool,
    pub vision_config: Option<crate::config::VisionModelConfig>,
    pub speech_output_dir: Option<PathBuf>,
    pub goal_state: Option<SharedGoalState>,
    /// Register the agent-callable `verify` self-critique tool (#4196).
    /// Gated by `Feature::Verify` (`[features] verify_tool`), default on.
    pub verify_tool_enabled: bool,
}

impl AgentToolSurfaceOptions {
    #[must_use]
    pub fn new(shell_policy: crate::worker_profile::ShellPolicy) -> Self {
        Self {
            shell_policy,
            apply_patch_enabled: false,
            web_search_enabled: false,
            memory_tool_enabled: false,
            vision_config: None,
            speech_output_dir: None,
            goal_state: None,
            verify_tool_enabled: true,
        }
    }
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

    #[must_use]
    pub fn with_dynamic_tools(mut self, dynamic_tools: &[DynamicToolSpec]) -> Self {
        for tool in dynamic_tools {
            self = self.with_tool(Arc::new(super::dynamic::RuntimeDynamicTool::new(
                tool.clone(),
            )));
        }
        self
    }

    /// Include file tools (read, write, edit, list).
    #[must_use]
    pub fn with_file_tools(self) -> Self {
        use super::file_tool::FileTool;
        self.with_tool(Arc::new(FileTool::new("File")))
    }

    /// Include only read-only file tools (read, list).
    #[must_use]
    pub fn with_read_only_file_tools(self) -> Self {
        use super::file_tool::FileTool;
        self.with_tool(Arc::new(FileTool::read_only("File")))
            .with_tool(Arc::new(
                super::tool_result_retrieval::RetrieveToolResultTool,
            ))
    }

    /// Include shell execution tools.
    ///
    /// Model and execution surfaces expose only `Bash` (#4625). Per-action
    /// `exec_shell*` spellings were removed in v0.9.3.
    #[must_use]
    pub fn with_shell_tools(self) -> Self {
        use super::shell::BashTool;
        self.with_tool(Arc::new(BashTool::new("Bash")))
            .with_terminal_tools()
    }

    /// Include the stateful PTY terminal tools. Like `exec_shell`, these are
    /// only exposed when the active shell policy allows shell access.
    #[cfg(not(target_env = "ohos"))]
    #[must_use]
    pub fn with_terminal_tools(self) -> Self {
        use super::terminal_session::{
            TerminalCancelTool, TerminalResetTool, TerminalRunTool, TerminalSendTool,
            TerminalWaitTool,
        };
        self.with_tool(Arc::new(TerminalRunTool))
            .with_tool(Arc::new(TerminalSendTool))
            .with_tool(Arc::new(TerminalWaitTool))
            .with_tool(Arc::new(TerminalCancelTool))
            .with_tool(Arc::new(TerminalResetTool))
    }

    /// OpenHarmony does not include the `portable-pty` dependency, so keep the
    /// ordinary shell tools without advertising unavailable persistent PTYs.
    #[cfg(target_env = "ohos")]
    #[must_use]
    pub fn with_terminal_tools(self) -> Self {
        self
    }

    /// Search is part of the canonical `File` action surface.
    #[must_use]
    pub fn with_search_tools(self) -> Self {
        self
    }

    /// Include the canonical `Git` inspection/history surface.
    #[must_use]
    pub fn with_git_tools(self) -> Self {
        use super::git_tool::GitTool;
        self.with_tool(Arc::new(GitTool::new("Git")))
    }

    /// Git history is part of the canonical `Git` action surface.
    #[must_use]
    pub fn with_git_history_tools(self) -> Self {
        self
    }

    /// Include workspace diagnostics tool.
    #[must_use]
    pub fn with_diagnostics_tool(self) -> Self {
        use super::diagnostics::DiagnosticsTool;
        self.with_tool(Arc::new(DiagnosticsTool))
    }

    /// Include the `pandoc_convert` tool only when the `pandoc`
    /// binary is present on this host. Same probe-then-decide
    /// pattern v0.8.31 introduced for Python — when pandoc is
    /// missing the tool is not registered, so the model never
    /// sees a binary it can't actually use.
    #[must_use]
    pub fn with_pandoc_tools(self) -> Self {
        if crate::dependencies::resolve_pandoc().is_some() {
            use super::pandoc::PandocConvertTool;
            self.with_tool(Arc::new(PandocConvertTool))
        } else {
            self
        }
    }

    /// Include the `image_ocr` tool only when a local OCR backend is present.
    /// macOS uses the built-in Vision framework, while other platforms use
    /// Tesseract when installed.
    #[must_use]
    pub fn with_image_ocr_tools(self) -> Self {
        if super::image_ocr::ocr_available() {
            use super::image_ocr::ImageOcrTool;
            self.with_tool(Arc::new(ImageOcrTool))
        } else {
            self
        }
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
        use super::run_tool::RunTool;
        self.with_tool(Arc::new(RunTool::new("Run")))
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
    ///
    /// Each family is one tool with an `action` parameter (`tasks`, `github`,
    /// `automation`). Per-action execution aliases were removed in v0.9.3.
    ///
    /// Shell-related task tools (`task_shell_start`, `task_shell_wait`) are
    /// *not* included here — use `with_runtime_task_shell_tools` to register
    /// them when `allow_shell` is true.
    #[must_use]
    pub fn with_runtime_task_tools(self) -> Self {
        use super::automation::AutomationTool;
        use super::github::GithubTool;
        use super::tasks::TasksTool;

        self.with_tool(Arc::new(TasksTool::new("tasks")))
            .with_tool(Arc::new(GithubTool::new("github")))
            .with_tool(Arc::new(AutomationTool::new("automation")))
    }

    /// Include shell-related task tools (`task_shell_start`, `task_shell_wait`).
    ///
    /// These are gated behind `allow_shell` because `task_shell_start`
    /// delegates directly to `BashTool`, providing the same shell
    /// execution capability as `Bash`.
    #[must_use]
    pub fn with_runtime_task_shell_tools(self) -> Self {
        use super::tasks::{TaskShellStartTool, TaskShellWaitTool};
        self.with_tool(Arc::new(TaskShellStartTool))
            .with_tool(Arc::new(TaskShellWaitTool))
    }

    /// Include only read-only durable task, PR-attempt, GitHub, and automation
    /// inspection tools. Plan mode uses this surface so it can observe state
    /// without starting work, changing remotes, or mutating automation config.
    ///
    /// The model sees the same canonical `tasks` / `github` / `automation`
    /// tools as the full surface, restricted to their read-only actions.
    #[must_use]
    pub fn with_runtime_read_only_task_tools(self) -> Self {
        use super::automation::AutomationTool;
        use super::github::GithubTool;
        use super::tasks::TasksTool;

        self.with_tool(Arc::new(TasksTool::read_only("tasks")))
            .with_tool(Arc::new(GithubTool::read_only("github")))
            .with_tool(Arc::new(AutomationTool::read_only("automation")))
    }

    /// Include web search and fetch tools.
    ///
    /// These are feature-gated behind `Feature::WebSearch` in `tool_setup.rs`.
    /// `finance` is registered separately via `with_finance_tool()` and is
    /// NOT gated behind the web-search feature.
    #[must_use]
    pub fn with_web_tools(self) -> Self {
        use super::web_run::WebRunTool;
        use super::web_tool::WebTool;
        self.with_tool(Arc::new(WebTool::new("Web")))
            .with_tool(Arc::new(WebRunTool))
    }

    /// Include the `finance` market-data tool.
    ///
    /// This tool is registered unconditionally for agent modes and is NOT
    /// gated behind `Feature::WebSearch` (it fetches financial data, not
    /// web search results).
    #[must_use]
    pub fn with_finance_tool(self) -> Self {
        use super::finance::FinanceTool;
        self.with_tool(Arc::new(FinanceTool::new()))
    }

    /// Register the `image_analyze` vision tool.
    /// Only registered when `[vision_model]` is configured in config.toml.
    #[must_use]
    pub fn with_vision_tools(self, config: crate::config::VisionModelConfig) -> Self {
        use crate::vision::tools::ImageAnalyzeTool;
        self.with_tool(Arc::new(ImageAnalyzeTool::new(config)))
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
        use super::file_tool::FileTool;
        self.with_tool(Arc::new(FileTool::with_patch("File")))
            .with_tool(Arc::new(FileTool::alias("apply_patch", "patch")))
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

    /// Include Xiaomi MiMo speech/TTS tools (`speech`, `tts`).
    #[must_use]
    pub fn with_speech_tools(
        self,
        client: Option<DeepSeekClient>,
        output_dir: Option<PathBuf>,
    ) -> Self {
        use super::speech::SpeechTool;
        self.with_tool(Arc::new(SpeechTool::new(
            "speech",
            client.clone(),
            output_dir.clone(),
        )))
        .with_tool(Arc::new(SpeechTool::new("tts", client, output_dir)))
    }

    /// Include the canonical persistent RLM session tool.
    #[must_use]
    pub fn with_rlm_tool(self, client: Option<DeepSeekClient>, _root_model: String) -> Self {
        use super::rlm::RlmTool;
        self.with_tool(Arc::new(RlmTool::new("rlm", client)))
    }

    /// Include `handle_read`, the bounded projection reader for symbolic
    /// `var_handle` payloads.
    #[must_use]
    pub fn with_handle_tools(self) -> Self {
        use super::handle::HandleReadTool;
        self.with_tool(Arc::new(HandleReadTool))
    }

    /// Include the review tool.
    #[must_use]
    pub fn with_review_tool(self, client: Option<DeepSeekClient>, model: String) -> Self {
        use super::review::ReviewTool;
        self.with_tool(Arc::new(ReviewTool::new(client, model)))
    }

    /// Include the agent-callable `verify` self-critique tool (#4196). The
    /// critic runs at elevated reasoning (default `Max`) independent of the
    /// session tier and is given no tools, so it cannot recurse into `verify`.
    #[must_use]
    pub fn with_verify_tool(self, client: Option<DeepSeekClient>, model: String) -> Self {
        use super::verify::VerifyTool;
        self.with_tool(Arc::new(VerifyTool::new(client, model)))
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

    /// Include the native-memory retrieval tools alongside reviewed capture.
    #[must_use]
    pub fn with_native_memory_tools(self) -> Self {
        use super::native_memory::{MemoryGetTool, MemorySearchTool};
        self.with_tool(Arc::new(MemorySearchTool))
            .with_tool(Arc::new(MemoryGetTool))
    }

    /// Include the model-facing `lsp` intelligence tool. Reuses the session
    /// [`crate::lsp::LspManager`] attached to `ToolContext` — never spawns a
    /// second server lifecycle.
    #[must_use]
    pub fn with_lsp_tool(self) -> Self {
        use super::lsp::LspTool;
        self.with_tool(Arc::new(LspTool))
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

    /// Register the `start_mcp_server` tool for dynamically adding MCP servers
    /// from conversation context. Does not register MCP tool adapters — those
    /// are returned by `pool.to_api_tools()` in `engine.mcp_tools()`.
    #[must_use]
    pub fn with_runtime_mcp_tool(
        mut self,
        mcp_pool: std::sync::Arc<tokio::sync::Mutex<crate::mcp::McpPool>>,
    ) -> Self {
        self.tools
            .push(Arc::new(super::runtime_mcp::StartRuntimeMcpServer::new(
                mcp_pool,
            )));
        self
    }

    /// Include all agent tools under a typed shell policy.
    #[must_use]
    pub fn with_agent_tools_policy(self, shell_policy: crate::worker_profile::ShellPolicy) -> Self {
        let builder = self
            .with_file_tools()
            .with_note_tool()
            .with_search_tools()
            .with_user_input_tool()
            .with_parallel_tool()
            .with_git_tools()
            .with_git_history_tools()
            .with_diagnostics_tool()
            .with_lsp_tool()
            .with_project_tools()
            .with_skill_tools()
            .with_test_runner_tool()
            .with_validation_tools()
            .with_tool_result_retrieval_tool()
            .with_handle_tools()
            .with_runtime_task_tools()
            .with_revert_turn_tool()
            .with_pandoc_tools()
            .with_image_ocr_tools()
            .with_finance_tool();

        if shell_policy.allows_shell() {
            builder.with_shell_tools().with_runtime_task_shell_tools()
        } else {
            builder
        }
    }

    /// Include the native Agent-mode surface shared by the parent runtime and
    /// default child sub-agents, excluding the `agent` launcher itself.
    #[must_use]
    pub fn with_agent_runtime_surface(
        self,
        client: Option<DeepSeekClient>,
        model: String,
        options: AgentToolSurfaceOptions,
        todo_list: super::todo::SharedTodoList,
        plan_state: super::plan::SharedPlanState,
    ) -> Self {
        let speech_client = client.clone();
        let verify_client = client.clone();
        let verify_model = model.clone();
        let mut builder = self
            .with_agent_tools_policy(options.shell_policy)
            .with_todo_tool(todo_list)
            .with_plan_tool(plan_state)
            .with_review_tool(client.clone(), model.clone())
            .with_rlm_tool(client.clone(), model.clone())
            .with_fim_tool(client, model)
            .with_speech_tools(speech_client, options.speech_output_dir.clone());

        if options.verify_tool_enabled {
            builder = builder.with_verify_tool(verify_client, verify_model);
        }
        if let Some(goal_state) = options.goal_state {
            builder = builder.with_goal_tools(goal_state);
        }
        if options.apply_patch_enabled {
            builder = builder.with_patch_tools();
        }
        if options.web_search_enabled {
            builder = builder.with_web_tools();
        }
        if options.memory_tool_enabled {
            builder = builder.with_remember_tool().with_native_memory_tools();
        }
        if let Some(vision_config) = options.vision_config {
            builder = builder.with_vision_tools(vision_config);
        }

        builder.with_notify_tool()
    }

    /// Include the full child-inherited Agent surface under resolved
    /// feature/config options.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn with_full_agent_surface_options(
        self,
        client: Option<DeepSeekClient>,
        model: String,
        manager: super::subagent::SharedSubAgentManager,
        runtime: super::subagent::SubAgentRuntime,
        options: AgentToolSurfaceOptions,
        todo_list: super::todo::SharedTodoList,
        plan_state: super::plan::SharedPlanState,
    ) -> Self {
        self.with_agent_runtime_surface(client, model, options, todo_list, plan_state)
            .with_subagent_tools(manager, runtime)
    }

    /// Include the canonical work-progress tool with a shared `TodoList`.
    #[must_use]
    pub fn with_todo_tool(self, todo_list: super::todo::SharedTodoList) -> Self {
        use super::todo::TodoWriteTool;
        self.with_tool(Arc::new(TodoWriteTool::work_update(todo_list)))
    }

    /// Include the plan tool with a shared `PlanState`.
    #[must_use]
    pub fn with_plan_tool(self, plan_state: super::plan::SharedPlanState) -> Self {
        use super::plan::UpdatePlanTool;
        self.with_tool(Arc::new(UpdatePlanTool::new(plan_state)))
    }

    /// Include runtime goal tools (`create_goal`, `get_goal`, `update_goal`).
    #[must_use]
    pub fn with_goal_tools(self, goal_state: super::goal::SharedGoalState) -> Self {
        use super::goal::{CreateGoalTool, GetGoalTool, UpdateGoalTool};
        self.with_tool(Arc::new(CreateGoalTool::new(goal_state.clone())))
            .with_tool(Arc::new(GetGoalTool::new(goal_state.clone())))
            .with_tool(Arc::new(UpdateGoalTool::new(goal_state)))
    }

    /// Include sub-agent management tools.
    #[must_use]
    pub fn with_subagent_tools(
        self,
        manager: super::subagent::SharedSubAgentManager,
        runtime: super::subagent::SubAgentRuntime,
    ) -> Self {
        use super::subagent::AgentTool;
        use super::subagent::register_coordination_tools;
        use super::workflow::WorkflowTool;
        use super::workflow_trigger::soft_auto_policy_is_linked;

        // Keep soft-auto trigger policy linked in release builds (#4127).
        debug_assert!(
            soft_auto_policy_is_linked(),
            "workflow soft-auto policy must stay linked"
        );

        let builder = self
            .with_tool(Arc::new(WorkflowTool::new(
                Arc::clone(&manager),
                runtime.clone(),
            )))
            .with_tool(Arc::new(AgentTool::new(
                Arc::clone(&manager),
                runtime.clone(),
            )));
        register_coordination_tools(builder, manager, runtime)
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
struct McpToolAdapter {
    name: String,
    tool: crate::mcp::McpTool,
    pool: std::sync::Arc<tokio::sync::Mutex<crate::mcp::McpPool>>,
}

fn is_mcp_read_helper(name: &str) -> bool {
    matches!(
        name,
        "list_mcp_resources"
            | "list_mcp_resource_templates"
            | "mcp_read_resource"
            | "read_mcp_resource"
            | "mcp_get_prompt"
    )
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
        if is_mcp_read_helper(&self.name) {
            vec![ToolCapability::ReadOnly]
        } else {
            vec![ToolCapability::Network, ToolCapability::RequiresApproval]
        }
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        if is_mcp_read_helper(&self.name) {
            ApprovalRequirement::Auto
        } else {
            ApprovalRequirement::Required
        }
    }

    fn defer_loading(&self) -> bool {
        // Discovery helpers stay loaded; everything else is deferred.
        !is_mcp_read_helper(&self.name)
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let mut pool = self.pool.lock().await;
        let result = pool
            .call_tool(&self.name, input)
            .await
            .map_err(|e| ToolError::execution_failed(format!("MCP tool failed: {e}")))?;
        Ok(mcp_result_to_tool_result(&result))
    }
}

/// Map an MCP `tools/call` result to a `ToolResult`. MCP servers signal tool
/// failure with `isError: true` on an otherwise successful JSON-RPC response;
/// wrapping that in `ToolResult::success` tells the model a rejected call
/// worked (#5123-class). Error results keep their text payload verbatim so
/// the model still sees the server's message.
fn mcp_result_to_tool_result(result: &Value) -> ToolResult {
    let content = serde_json::to_string(result).unwrap_or_else(|_| result.to_string());
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !is_error {
        return ToolResult::success(content);
    }
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|text| !text.is_empty())
        .unwrap_or(content);
    ToolResult::error(text)
}

#[cfg(test)]
pub(super) fn mcp_tool_adapter_for_test(name: &str) -> Arc<dyn ToolSpec> {
    Arc::new(McpToolAdapter {
        name: name.to_string(),
        tool: crate::mcp::McpTool {
            name: name.to_string(),
            description: None,
            input_schema: serde_json::json!({"type": "object"}),
        },
        pool: Arc::new(tokio::sync::Mutex::new(crate::mcp::McpPool::new(
            crate::mcp::McpConfig::default(),
        ))),
    })
}

// === Unit Tests ===

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use serde_json::{Value, json};
    use tempfile::tempdir;

    use crate::config::ToolOverride;
    use crate::tools::ToolRegistryBuilder;
    use crate::tools::spec::{
        ApprovalRequirement, ToolAuthorityEnvelope, ToolCapability, ToolContext, ToolError,
        ToolMutationAuthority, ToolResult, ToolSpec, required_str,
    };

    use super::{ToolRegistry, mcp_result_to_tool_result, mcp_tool_adapter_for_test};

    #[test]
    fn mcp_iserror_result_maps_to_tool_error_preserving_text() {
        // #5123-class: MCP servers report tool failure via isError on an
        // otherwise successful response; the model must see a failure, not a
        // success carrying an error message body.
        let error_payload = json!({
            "content": [
                {"type": "text", "text": "delete failed: permission denied"}
            ],
            "isError": true
        });
        let result = mcp_result_to_tool_result(&error_payload);
        assert!(!result.success, "isError must not be reported as success");
        assert_eq!(result.content, "delete failed: permission denied");

        let ok_payload = json!({
            "content": [{"type": "text", "text": "wrote 3 rows"}]
        });
        let result = mcp_result_to_tool_result(&ok_payload);
        assert!(result.success);
        assert!(result.content.contains("wrote 3 rows"));

        // isError without text content falls back to the serialized payload.
        let bare_error = json!({"isError": true, "content": []});
        let result = mcp_result_to_tool_result(&bare_error);
        assert!(!result.success);
        assert!(result.content.contains("isError"));
    }

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
    fn mcp_read_helpers_remain_auto_and_eagerly_loaded() {
        for name in [
            "list_mcp_resources",
            "list_mcp_resource_templates",
            "mcp_read_resource",
            "read_mcp_resource",
            "mcp_get_prompt",
        ] {
            let adapter = mcp_tool_adapter_for_test(name);
            assert_eq!(
                adapter.approval_requirement(),
                ApprovalRequirement::Auto,
                "{name} should remain an automatic read helper"
            );
            assert!(adapter.is_read_only(), "{name} should remain read-only");
            assert!(!adapter.defer_loading(), "{name} should remain loaded");
        }
    }

    #[test]
    fn mcp_actions_require_approval_with_exact_helper_matching() {
        for name in [
            "mcp_github_create_pull_request",
            "mcp_github_list_mcp_resources_export",
            "read_mcp_resource_and_delete",
        ] {
            let adapter = mcp_tool_adapter_for_test(name);
            assert_eq!(
                adapter.approval_requirement(),
                ApprovalRequirement::Required,
                "{name} must not inherit read-helper approval"
            );
            assert!(
                adapter
                    .capabilities()
                    .contains(&ToolCapability::RequiresApproval),
                "{name} should advertise approval gating"
            );
            assert!(adapter.defer_loading(), "{name} should remain deferred");
        }
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
        assert_eq!(registry.all().len(), 1);
    }

    #[test]
    fn resolve_exact_match_is_ascii_case_insensitive() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("read_file"));

        assert_eq!(registry.resolve("READ_FILE"), Some("read_file"));
    }

    #[test]
    fn resolve_never_executes_a_fuzzy_prefix_guess() {
        // #5123-class: a hallucinated name that merely shares a prefix with a
        // real tool must NOT resolve — executing a prefix guess dispatched an
        // arbitrary sibling tool ("agents" -> "agents/interrupt"). Exact and
        // lossless normalizations still resolve; guesses return None so the
        // caller can surface "unknown tool, did you mean: …".
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("agents/interrupt"));
        registry.register(make_test_tool("read_file"));

        // Prefix guesses in both directions are rejected.
        assert_eq!(registry.resolve("agents"), None);
        assert_eq!(registry.resolve("agents/int"), None);
        assert_eq!(registry.resolve("read"), None);
        assert_eq!(registry.resolve("read_file_extra"), None);

        // Lossless normalizations still resolve.
        let mut hyphen_registry = ToolRegistry::new(ToolContext::new(tmp.path().to_path_buf()));
        hyphen_registry.register(make_test_tool("read_file"));
        assert_eq!(hyphen_registry.resolve("read-file"), Some("read_file"));
        assert_eq!(hyphen_registry.resolve("ReadFile"), Some("read_file"));
        assert_eq!(hyphen_registry.resolve("read_file_tool"), Some("read_file"));
    }

    #[test]
    fn work_update_is_the_only_registered_progress_surface() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let registry = ToolRegistryBuilder::new()
            .with_todo_tool(crate::tools::todo::new_shared_todo_list())
            .build(ctx);

        assert!(registry.contains("work_update"));
        for retired in [
            "checklist_write",
            "checklist_add",
            "checklist_update",
            "checklist_list",
            "todo_write",
            "todo_add",
            "todo_update",
            "todo_list",
        ] {
            assert!(
                !registry.contains(retired),
                "{retired} must no longer be callable"
            );
        }

        let api_names = registry
            .to_api_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();

        assert!(
            api_names.iter().any(|name| name == "work_update"),
            "work_update should be the sole model-visible progress surface"
        );
        for retired in [
            "checklist_write",
            "checklist_add",
            "checklist_update",
            "checklist_list",
            "todo_write",
            "todo_add",
            "todo_update",
            "todo_list",
        ] {
            assert!(
                api_names.iter().all(|name| name != retired),
                "{retired} must not appear in the model catalog"
            );
        }
    }

    #[test]
    fn rlm_is_the_only_registered_session_surface() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let registry = ToolRegistryBuilder::new()
            .with_rlm_tool(None, "test-model".to_string())
            .build(ctx);

        assert!(registry.contains("rlm"));
        for retired in [
            "rlm_session_objects",
            "rlm_open",
            "rlm_eval",
            "rlm_configure",
            "rlm_close",
        ] {
            assert!(
                !registry.contains(retired),
                "{retired} must no longer be callable"
            );
        }
    }

    #[test]
    fn apply_overrides_removes_original_when_replacement_is_missing() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistryBuilder::new().with_file_tools().build(ctx);

        assert!(registry.contains("File"));

        let mut overrides = HashMap::new();
        overrides.insert(
            "File".to_string(),
            ToolOverride::Script {
                path: "missing-wrapper.sh".to_string(),
                args: None,
            },
        );

        registry.apply_overrides(&overrides, tmp.path());

        assert!(!registry.contains("File"));
    }

    #[test]
    fn builder_registers_speech_alias_tools() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let registry = ToolRegistryBuilder::new()
            .with_speech_tools(None, None)
            .build(ctx);

        assert!(registry.contains("speech"));
        assert!(registry.contains("tts"));
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
    fn remove_tool_invalidates_api_tools_cache() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);
        registry.register(make_test_tool("alpha"));
        registry.register(make_test_tool("beta"));

        let before = registry.to_api_tools();
        assert_eq!(before.len(), 2);

        assert!(registry.remove_tool("alpha"));
        let after_remove = registry.to_api_tools();
        assert_eq!(after_remove.len(), 1);
        assert_eq!(after_remove[0].name, "beta");
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

    fn scoped_context(workspace: &std::path::Path) -> ToolContext {
        ToolContext::new(workspace.to_path_buf())
            .with_tool_authority(
                ToolAuthorityEnvelope {
                    schema_version: 1,
                    owner: "fleet-worker-1".to_string(),
                    authority: ToolMutationAuthority::ScopedWrite,
                    network_access: None,
                    writable_roots: vec!["src".to_string()],
                    writable_files: Vec::new(),
                    coordination_contracts: Vec::new(),
                }
                .normalized()
                .expect("test authority"),
            )
            .expect("test context authority")
    }

    #[tokio::test]
    async fn fleet_authority_allows_scoped_file_writes_and_rejects_outside_paths() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("src")).expect("src");
        std::fs::create_dir(tmp.path().join("docs")).expect("docs");
        let registry = ToolRegistryBuilder::new()
            .with_file_tools()
            .with_patch_tools()
            .build(scoped_context(tmp.path()));

        registry
            .execute_full(
                "File",
                json!({"action": "write", "path": "src/ok.txt", "content": "ok\n"}),
            )
            .await
            .expect("scoped File write");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("src/ok.txt")).expect("written file"),
            "ok\n"
        );

        let error = registry
            .execute_full(
                "File",
                json!({"action": "write", "path": "docs/no.txt", "content": "no\n"}),
            )
            .await
            .expect_err("out-of-scope File write")
            .to_string();
        assert!(error.contains("outside its machine-readable"), "{error}");
        assert!(!tmp.path().join("docs/no.txt").exists());
    }

    #[tokio::test]
    async fn fleet_authority_denies_bash_even_when_command_classifier_calls_it_read_only() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("src")).expect("src");
        let registry = ToolRegistryBuilder::new()
            .with_shell_tools()
            .build(scoped_context(tmp.path()));

        let error = registry
            .execute_full("Bash", json!({"action": "run", "command": "git status"}))
            .await
            .expect_err("Bash remains unprovable under a file scope")
            .to_string();
        assert!(error.contains("arbitrary command execution"), "{error}");
    }

    #[tokio::test]
    async fn fleet_authority_denies_git_even_when_the_action_is_nominally_read_only() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("src")).expect("src");
        let registry = ToolRegistryBuilder::new()
            .with_git_tools()
            .with_git_history_tools()
            .with_review_tool(None, "fixture-model".to_string())
            .build(scoped_context(tmp.path()));

        for (name, input) in [
            ("Git", json!({"action": "status"})),
            ("Git", json!({"action": "diff"})),
            ("Git", json!({"action": "show", "revision": "HEAD"})),
            ("Git", json!({"action": "blame", "path": "src/lib.rs"})),
            ("review", json!({"target": "diff"})),
        ] {
            let error = registry
                .execute_full(name, input)
                .await
                .expect_err("Git subprocesses remain unprovable under Fleet authority")
                .to_string();
            assert!(error.contains("Git helpers"), "{name}: {error}");
        }
    }

    #[tokio::test]
    async fn fleet_authority_rejects_fim_edit_outside_its_write_scope() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("src")).expect("src");
        std::fs::create_dir(tmp.path().join("docs")).expect("docs");
        std::fs::write(tmp.path().join("docs/outside.txt"), "before\nafter\n").expect("fixture");
        let registry = ToolRegistryBuilder::new()
            .with_fim_tool(None, "fixture-model".to_string())
            .build(scoped_context(tmp.path()));

        let error = registry
            .execute_full(
                "fim_edit",
                json!({
                    "path": "docs/outside.txt",
                    "prefix_anchor": "before\n",
                    "suffix_anchor": "after\n"
                }),
            )
            .await
            .expect_err("FIM mutation must be checked before model execution")
            .to_string();
        assert!(error.contains("outside its machine-readable"), "{error}");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("docs/outside.txt")).unwrap(),
            "before\nafter\n"
        );
    }

    struct MixedExecutionTool;

    #[async_trait::async_trait]
    impl ToolSpec for MixedExecutionTool {
        fn name(&self) -> &str {
            "mixed_execution"
        }

        fn description(&self) -> &str {
            "inspect or start a child"
        }

        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }

        fn capabilities(&self) -> Vec<ToolCapability> {
            vec![ToolCapability::ExecutesCode]
        }

        fn is_read_only_for(&self, input: &Value) -> bool {
            input.get("action").and_then(Value::as_str) == Some("inspect")
        }

        async fn execute(
            &self,
            _input: Value,
            _context: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::success("observed"))
        }
    }

    #[tokio::test]
    async fn fleet_authority_allows_read_only_actions_but_denies_mixed_family_starts() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("src")).expect("src");
        let registry = ToolRegistryBuilder::new()
            .with_tool(Arc::new(MixedExecutionTool))
            .build(scoped_context(tmp.path()));

        registry
            .execute_full("mixed_execution", json!({"action": "inspect"}))
            .await
            .expect("read-only status/inspect actions remain usable");
        let error = registry
            .execute_full("mixed_execution", json!({"action": "start"}))
            .await
            .expect_err("child/code starts remain denied")
            .to_string();
        assert!(error.contains("child execution"), "{error}");
    }

    struct UnscopedMutator;

    #[async_trait::async_trait]
    impl ToolSpec for UnscopedMutator {
        fn name(&self) -> &str {
            "unscoped_mutator"
        }

        fn description(&self) -> &str {
            "mutates state without a file target"
        }

        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }

        fn capabilities(&self) -> Vec<ToolCapability> {
            Vec::new()
        }

        fn is_read_only_for(&self, _input: &Value) -> bool {
            false
        }

        async fn execute(
            &self,
            _input: Value,
            _context: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::success("mutated"))
        }
    }

    #[tokio::test]
    async fn fleet_authority_denies_every_unscoped_mutator_not_only_file_capabilities() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("src")).expect("src");
        let registry = ToolRegistryBuilder::new()
            .with_tool(Arc::new(UnscopedMutator))
            .build(scoped_context(tmp.path()));

        let error = registry
            .execute_full("unscoped_mutator", json!({}))
            .await
            .expect_err("unscoped mutation must fail closed")
            .to_string();
        assert!(error.contains("mutating tool"), "{error}");
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
    fn test_builder_with_web_tools_no_longer_includes_finance() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new().with_web_tools().build(ctx);

        // The model-facing web surface is the canonical action-dispatched tool.
        assert!(registry.contains("Web"));
        assert!(registry.contains("web.run"));
        for retired in ["web_search", "fetch_url", "wait_for_dev_server"] {
            assert!(!registry.contains(retired), "{retired} must stay removed");
        }
        assert!(!registry.contains("finance"));
    }

    #[test]
    fn canonical_runtime_tools_remove_legacy_aliases() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let registry = ToolRegistryBuilder::new()
            .with_file_tools()
            .with_search_tools()
            .with_git_tools()
            .with_git_history_tools()
            .with_test_runner_tool()
            .with_web_tools()
            .with_patch_tools()
            .build(ctx);

        let api_names = registry
            .to_api_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        for canonical in ["File", "Git", "Run", "Web"] {
            assert!(api_names.iter().any(|name| name == canonical));
        }
        for retired in [
            "read_file",
            "write_file",
            "edit_file",
            "list_dir",
            "file_search",
            "grep_files",
            "git_status",
            "git_diff",
            "git_log",
            "git_show",
            "git_blame",
            "run_tests",
            "run_verifiers",
            "web_search",
            "fetch_url",
            "wait_for_dev_server",
        ] {
            assert!(!registry.contains(retired), "{retired} must stay removed");
            assert!(
                api_names.iter().all(|name| name != retired),
                "{retired} must not be advertised"
            );
        }
        // DeepSeek Responses exposes apply_patch as its one custom tool, so it
        // remains callable but is not duplicated in the ordinary API catalog.
        assert!(registry.contains("apply_patch"));
        assert!(api_names.iter().all(|name| name != "apply_patch"));
    }

    #[tokio::test]
    async fn canonical_file_actions_share_read_before_edit_state() {
        let tmp = tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("sample.txt"), "before\n").expect("fixture");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let registry = ToolRegistryBuilder::new().with_file_tools().build(ctx);

        registry
            .execute_full("File", json!({"action": "read", "path": "sample.txt"}))
            .await
            .expect("canonical read should execute");
        registry
            .execute_full(
                "File",
                json!({
                    "action": "edit",
                    "path": "sample.txt",
                    "search": "before",
                    "replace": "after"
                }),
            )
            .await
            .expect("canonical edit should execute after the read");

        assert_eq!(
            std::fs::read_to_string(tmp.path().join("sample.txt")).expect("edited file"),
            "after\n"
        );
    }

    #[test]
    fn read_only_file_surface_does_not_advertise_write_actions() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let registry = ToolRegistryBuilder::new()
            .with_read_only_file_tools()
            .with_search_tools()
            .build(ctx);
        let file = registry
            .to_api_tools()
            .into_iter()
            .find(|tool| tool.name == "File")
            .expect("canonical File tool");
        let actions = file.input_schema["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum");

        for blocked in ["write", "edit", "patch"] {
            assert!(actions.iter().all(|action| action != blocked));
        }
    }

    #[test]
    fn test_builder_with_finance_tool() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new().with_finance_tool().build(ctx);

        assert!(registry.contains("finance"));
    }

    #[test]
    fn with_verify_tool_registers_and_exposes_verify() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new()
            .with_verify_tool(None, "test-model".to_string())
            .build(ctx);

        assert!(
            registry.contains("verify"),
            "verify tool should be registered"
        );
        let api_names = registry
            .to_api_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert!(
            api_names.iter().any(|name| name == "verify"),
            "verify tool should be model-visible"
        );
    }

    #[test]
    fn agent_runtime_surface_gates_verify_on_option() {
        use super::AgentToolSurfaceOptions;
        use crate::worker_profile::ShellPolicy;

        let build_surface = |verify_enabled: bool| {
            let tmp = tempdir().expect("tempdir");
            let ctx = ToolContext::new(tmp.path().to_path_buf());
            let mut options = AgentToolSurfaceOptions::new(ShellPolicy::Full);
            options.verify_tool_enabled = verify_enabled;
            ToolRegistryBuilder::new()
                .with_agent_runtime_surface(
                    None,
                    "test-model".to_string(),
                    options,
                    crate::tools::todo::new_shared_todo_list(),
                    crate::tools::plan::new_shared_plan_state(),
                )
                .build(ctx)
        };

        assert!(
            build_surface(true).contains("verify"),
            "verify should register when enabled"
        );
        assert!(
            !build_surface(false).contains("verify"),
            "verify should be absent when the opt-out disables it"
        );
    }

    #[test]
    fn test_builder_with_agent_tools_policy_includes_finance() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new()
            .with_agent_tools_policy(crate::worker_profile::ShellPolicy::None)
            .build(ctx);

        assert!(registry.contains("finance"));
    }

    #[test]
    fn agent_tools_with_shell_policy_none_excludes_shell_tools() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new()
            .with_agent_tools_policy(crate::worker_profile::ShellPolicy::None)
            .build(ctx);

        assert!(
            !registry.contains("Bash"),
            "Bash should be excluded when the shell policy is None"
        );
        assert!(
            !registry.contains("exec_shell"),
            "retired exec_shell must remain absent"
        );
        assert!(
            !registry.contains("task_shell_start"),
            "task_shell_start should be excluded when the shell policy is None"
        );
        assert!(
            !registry.contains("task_shell_wait"),
            "task_shell_wait should be excluded when the shell policy is None"
        );
    }

    #[test]
    fn agent_tools_with_shell_policy_readonly_includes_shell_tools() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new()
            .with_agent_tools_policy(crate::worker_profile::ShellPolicy::ReadOnly)
            .build(ctx);

        assert!(registry.contains("Bash"));
        assert!(!registry.contains("exec_shell"));
        assert!(registry.contains("task_shell_start"));
        assert!(registry.contains("task_shell_wait"));
    }

    #[test]
    fn agent_tools_with_shell_policy_full_includes_shell_tools() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new()
            .with_agent_tools_policy(crate::worker_profile::ShellPolicy::Full)
            .build(ctx);

        assert!(registry.contains("Bash"));
        assert!(!registry.contains("exec_shell"));
        assert!(
            registry.contains("task_shell_start"),
            "task_shell_start should be included when the shell policy is Full"
        );
        assert!(
            registry.contains("task_shell_wait"),
            "task_shell_wait should be included when the shell policy is Full"
        );
    }

    /// v0.9.3 removes the per-action shell aliases entirely.
    #[test]
    fn shell_surface_contains_only_the_canonical_bash_tool() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let registry = ToolRegistryBuilder::new().with_shell_tools().build(ctx);

        for alias in [
            "exec_shell",
            "exec_wait",
            "exec_interact",
            "exec_shell_wait",
            "exec_shell_interact",
            "exec_shell_cancel",
        ] {
            assert!(!registry.contains(alias), "{alias} must be removed");
        }

        let api_names: Vec<String> = registry
            .to_api_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect();

        // Only Bash is model-visible.
        assert!(
            api_names.iter().any(|n| n == "Bash"),
            "Bash should be model-visible"
        );

        // Removed names also cannot leak back into the model catalog.
        for alias in [
            "exec_shell",
            "exec_wait",
            "exec_interact",
            "exec_shell_wait",
            "exec_shell_interact",
            "exec_shell_cancel",
        ] {
            assert!(
                api_names.iter().all(|n| n != alias),
                "{alias} should be hidden from the model catalog"
            );
        }
    }

    /// Each durable-work family exposes one canonical action tool; v0.9.3
    /// removes the per-action execution aliases.
    #[test]
    fn runtime_task_families_expose_only_canonical_tools() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let registry = ToolRegistryBuilder::new()
            .with_runtime_task_tools()
            .build(ctx);

        let legacy_aliases = [
            "task_create",
            "task_list",
            "task_read",
            "task_cancel",
            "task_gate_run",
            "pr_attempt_record",
            "pr_attempt_list",
            "pr_attempt_read",
            "pr_attempt_preflight",
            "github_issue_context",
            "github_pr_context",
            "github_comment",
            "github_close_issue",
            "github_close_pr",
            "automation_create",
            "automation_list",
            "automation_read",
            "automation_update",
            "automation_pause",
            "automation_resume",
            "automation_delete",
            "automation_run",
        ];
        for alias in legacy_aliases {
            assert!(!registry.contains(alias), "{alias} must be removed");
        }

        let api_names: Vec<String> = registry
            .to_api_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect();

        // Only the canonical tools are model-visible.
        for canonical in ["tasks", "github", "automation"] {
            assert!(
                api_names.iter().any(|n| n == canonical),
                "{canonical} should be model-visible"
            );
        }
        // Removed aliases also cannot leak back into the model catalog.
        for alias in legacy_aliases {
            assert!(
                api_names.iter().all(|n| n != alias),
                "{alias} should be hidden from the model catalog"
            );
        }
    }

    /// The Plan-mode read-only surface registers only the canonical families,
    /// restricted to their read actions.
    #[test]
    fn read_only_task_surface_contains_no_per_action_aliases() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let registry = ToolRegistryBuilder::new()
            .with_runtime_read_only_task_tools()
            .build(ctx);

        for name in [
            "task_list",
            "task_read",
            "pr_attempt_list",
            "pr_attempt_read",
            "github_issue_context",
            "github_pr_context",
            "automation_list",
            "automation_read",
            "task_create",
            "task_cancel",
            "task_gate_run",
            "pr_attempt_record",
            "pr_attempt_preflight",
            "github_comment",
            "github_close_issue",
            "github_close_pr",
            "automation_create",
            "automation_update",
            "automation_pause",
            "automation_resume",
            "automation_delete",
            "automation_run",
        ] {
            assert!(!registry.contains(name), "{name} must be removed");
        }

        let api_names: Vec<String> = registry
            .to_api_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        assert_eq!(api_names.len(), 3);
        for canonical in ["tasks", "github", "automation"] {
            assert!(
                api_names.iter().any(|n| n == canonical),
                "{canonical} should be model-visible on the read-only surface"
            );
        }
        // Every registered tool stays read-only (Plan-mode invariant).
        for tool in registry.all() {
            let caps = tool.capabilities();
            assert!(
                !caps.contains(&ToolCapability::WritesFiles)
                    && !caps.contains(&ToolCapability::ExecutesCode),
                "read-only surface must not register write/exec tools: {}",
                tool.name()
            );
        }
    }

    /// The unified `rlm` tool is the only registered RLM surface.
    #[test]
    fn rlm_family_removes_legacy_aliases() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let registry = ToolRegistryBuilder::new()
            .with_rlm_tool(None, "deepseek-v4-pro".to_string())
            .build(ctx);

        for alias in [
            "rlm_session_objects",
            "rlm_open",
            "rlm_eval",
            "rlm_configure",
            "rlm_close",
        ] {
            assert!(!registry.contains(alias), "{alias} must stay removed");
        }

        let api_names: Vec<String> = registry
            .to_api_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        assert!(
            api_names.iter().any(|n| n == "rlm"),
            "rlm should be model-visible"
        );
        for retired in [
            "rlm_session_objects",
            "rlm_open",
            "rlm_eval",
            "rlm_configure",
            "rlm_close",
        ] {
            assert!(
                api_names.iter().all(|n| n != retired),
                "{retired} must not be advertised"
            );
        }
    }
}
