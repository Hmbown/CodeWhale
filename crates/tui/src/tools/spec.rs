//! Tool specification traits for the DeepSeek TUI agent system.
//!
//! This module defines the core abstractions for tools:
//! - `ToolSpec`: The main trait that all tools must implement
//! - `ToolContext`: Execution context passed to tools
//! - `ToolResult`: Unified result type for tool execution
//! - `ToolCapability`: Capabilities and requirements of tools

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::features::Features;
use crate::lsp::LspManager;
use crate::network_policy::NetworkPolicyDecider;
use crate::sandbox::backend::SandboxBackend;
use crate::tools::shell::{SharedShellManager, new_shared_shell_manager};
#[allow(unused_imports)]
pub use deepseek_tools::{
    ApprovalRequirement, ToolCapability, ToolError, ToolResult, optional_bool, optional_str,
    optional_u64, required_str, required_u64,
};

/// Optional durable runtime services made available to model-visible tools.
///
/// These are intentionally optional so existing unit tests and one-off tool
/// contexts keep working. Tools that need durable task/automation state fail
/// closed with a clear "not available" error when the relevant service is not
/// attached.
#[derive(Clone, Default)]
pub struct RuntimeToolServices {
    pub shell_manager: Option<SharedShellManager>,
    pub task_manager: Option<crate::task_manager::SharedTaskManager>,
    pub automations: Option<crate::automation_manager::SharedAutomationManager>,
    pub task_data_dir: Option<PathBuf>,
    pub active_task_id: Option<String>,
    pub active_thread_id: Option<String>,
    /// Hook executor for `shell_env` injection (#456) and any future
    /// tool-side hook events. `None` outside the live engine — test
    /// contexts that don't care about hooks get a no-op.
    pub hook_executor: Option<std::sync::Arc<crate::hooks::HookExecutor>>,
}

impl std::fmt::Debug for RuntimeToolServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeToolServices")
            .field("shell_manager", &self.shell_manager.is_some())
            .field("task_manager", &self.task_manager.is_some())
            .field("automations", &self.automations.is_some())
            .field("task_data_dir", &self.task_data_dir)
            .field("active_task_id", &self.active_task_id)
            .field("active_thread_id", &self.active_thread_id)
            .field("hook_executor", &self.hook_executor.is_some())
            .finish()
    }
}

/// Sandbox policy for command execution.
#[derive(Debug, Clone, Default)]
pub enum SandboxPolicy {
    /// No sandboxing (dangerous but sometimes needed)
    #[default]
    None,
}

/// Context passed to tools during execution.
#[derive(Clone)]
pub struct ToolContext {
    /// The workspace root directory
    pub workspace: PathBuf,
    /// Shared shell manager for background tasks and streaming IO.
    pub shell_manager: SharedShellManager,
    /// Whether to allow paths outside workspace
    pub trust_mode: bool,
    /// Current sandbox policy
    #[allow(dead_code)]
    pub sandbox_policy: SandboxPolicy,
    /// Path for notes file
    pub notes_path: PathBuf,
    /// MCP configuration path
    #[allow(dead_code)]
    pub mcp_config_path: PathBuf,
    /// Elevated sandbox policy override (used when retrying after sandbox denial).
    /// This overrides the default sandbox behavior for shell commands.
    pub elevated_sandbox_policy: Option<crate::sandbox::SandboxPolicy>,
    /// Optional user-facing hint for shell commands that fail because the
    /// active sandbox policy intentionally denies outbound network access.
    pub shell_network_denied_hint: Option<String>,
    /// Whether tools should auto-approve without safety checks (YOLO mode).
    /// When true, command safety analysis is skipped for shell execution.
    pub auto_approve: bool,
    /// Effective feature flag set for the running session.
    pub features: Features,
    /// Namespace for tool state that should be scoped to the current session/thread.
    pub state_namespace: String,
    /// User-trusted external paths the agent may read/write even when they
    /// fall outside `workspace`. Loaded from `~/.deepseek/workspace-trust.json`
    /// and refreshed when the user runs `/trust add <path>`. Distinct from
    /// `trust_mode`, which is the all-or-nothing legacy switch (#29).
    pub trusted_external_paths: Vec<PathBuf>,
    /// Per-domain network policy (#135). When `None`, network tools fall back
    /// to a permissive default that mirrors pre-v0.7.0 behavior so tests and
    /// other contexts that don't construct a real policy keep working.
    pub network_policy: Option<NetworkPolicyDecider>,
    /// Durable runtime services for task, gate, PR-attempt, GitHub evidence,
    /// and automation tools.
    pub runtime: RuntimeToolServices,
    /// Cancellation token for the active engine turn. Tools that may wait on
    /// external work should observe this so UI cancel can interrupt them.
    pub cancel_token: Option<CancellationToken>,
    /// Optional external sandbox backend for shell execution.
    /// When set, exec_shell routes commands through this instead of spawning
    /// a local process.
    pub sandbox_backend: Option<std::sync::Arc<dyn SandboxBackend>>,
    /// Path to the user memory file. `None` when the user-memory feature
    /// (#489) is disabled — tools that read or write the file should
    /// short-circuit on `None` rather than fall back to a workspace-local
    /// default.
    pub memory_path: Option<PathBuf>,
    /// LSP manager for post-edit diagnostics injection (#428). `None` when
    /// LSP is disabled or the context is constructed in a test that does not
    /// need diagnostics. Edit tools append a `<diagnostics>` block to their
    /// result when this is present and the manager is enabled.
    pub lsp_manager: Option<Arc<LspManager>>,

    /// Large-output router (#548). When `Some`, tool results that exceed the
    /// configured token threshold are routed through a V4-Flash synthesis
    /// sub-agent before being returned to the parent context. `None` disables
    /// routing (e.g. in sub-agents and test contexts to avoid recursion).
    pub large_output_router: Option<crate::tools::large_output_router::LargeOutputRouter>,

    /// Which search backend `web_search` should use. Default: DuckDuckGo with
    /// Bing fallback. Set via `[search] provider` in config.toml.
    pub search_provider: crate::config::SearchProvider,
    /// API key for Tavily or Bocha. `None` for DuckDuckGo.
    pub search_api_key: Option<String>,

    /// Per-session workshop variable store (#548). Holds the raw content of
    /// the most recent large-tool routing event so the parent can call
    /// `promote_to_context` later. `None` when the router is disabled.
    pub workshop_vars: Option<
        std::sync::Arc<tokio::sync::Mutex<crate::tools::large_output_router::WorkshopVariables>>,
    >,
}

impl ToolContext {
    /// Create a new `ToolContext` with default settings.
    #[must_use]
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let workspace = workspace.into();
        let shell_manager = new_shared_shell_manager(workspace.clone());
        let notes_path = workspace.join(".deepseek").join("notes.md");
        let mcp_config_path = workspace.join(".deepseek").join("mcp.json");
        Self {
            workspace,
            shell_manager,
            trust_mode: false,
            sandbox_policy: SandboxPolicy::None,
            notes_path,
            mcp_config_path,
            elevated_sandbox_policy: None,
            shell_network_denied_hint: None,
            auto_approve: false,
            features: Features::with_defaults(),
            state_namespace: "workspace".to_string(),
            trusted_external_paths: Vec::new(),
            network_policy: None,
            runtime: RuntimeToolServices::default(),
            cancel_token: None,
            sandbox_backend: None,
            memory_path: None,
            lsp_manager: None,
            large_output_router: None,
            search_provider: crate::config::SearchProvider::default(),
            search_api_key: None,
            workshop_vars: None,
        }
    }

    /// Create a `ToolContext` with all settings specified.
    #[allow(dead_code)]
    pub fn with_options(
        workspace: impl Into<PathBuf>,
        trust_mode: bool,
        notes_path: impl Into<PathBuf>,
        mcp_config_path: impl Into<PathBuf>,
    ) -> Self {
        let workspace = workspace.into();
        let shell_manager = new_shared_shell_manager(workspace.clone());
        Self {
            workspace,
            shell_manager,
            trust_mode,
            sandbox_policy: SandboxPolicy::None,
            notes_path: notes_path.into(),
            mcp_config_path: mcp_config_path.into(),
            elevated_sandbox_policy: None,
            shell_network_denied_hint: None,
            auto_approve: false,
            features: Features::with_defaults(),
            state_namespace: "workspace".to_string(),
            trusted_external_paths: Vec::new(),
            network_policy: None,
            runtime: RuntimeToolServices::default(),
            cancel_token: None,
            sandbox_backend: None,
            memory_path: None,
            lsp_manager: None,
            large_output_router: None,
            search_provider: crate::config::SearchProvider::default(),
            search_api_key: None,
            workshop_vars: None,
        }
    }

    /// Create a `ToolContext` with auto-approve mode (YOLO).
    pub fn with_auto_approve(
        workspace: impl Into<PathBuf>,
        trust_mode: bool,
        notes_path: impl Into<PathBuf>,
        mcp_config_path: impl Into<PathBuf>,
        auto_approve: bool,
    ) -> Self {
        let workspace = workspace.into();
        let shell_manager = new_shared_shell_manager(workspace.clone());
        Self {
            workspace,
            shell_manager,
            trust_mode,
            sandbox_policy: SandboxPolicy::None,
            notes_path: notes_path.into(),
            mcp_config_path: mcp_config_path.into(),
            elevated_sandbox_policy: None,
            shell_network_denied_hint: None,
            auto_approve,
            features: Features::with_defaults(),
            state_namespace: "workspace".to_string(),
            trusted_external_paths: Vec::new(),
            network_policy: None,
            runtime: RuntimeToolServices::default(),
            cancel_token: None,
            sandbox_backend: None,
            memory_path: None,
            lsp_manager: None,
            large_output_router: None,
            search_provider: crate::config::SearchProvider::default(),
            search_api_key: None,
            workshop_vars: None,
        }
    }

    /// Attach a per-domain network policy to this context (#135).
    #[must_use]
    pub fn with_network_policy(mut self, policy: NetworkPolicyDecider) -> Self {
        self.network_policy = Some(policy);
        self
    }

    /// Attach durable runtime services to tools.
    #[must_use]
    pub fn with_runtime_services(mut self, runtime: RuntimeToolServices) -> Self {
        self.runtime = runtime;
        self
    }

    /// Attach the active engine cancellation token.
    #[must_use]
    pub fn with_cancel_token(mut self, cancel_token: CancellationToken) -> Self {
        self.cancel_token = Some(cancel_token);
        self
    }

    /// Attach an external sandbox backend for remote shell execution.
    #[must_use]
    #[allow(dead_code)]
    pub fn with_sandbox_backend(mut self, backend: std::sync::Arc<dyn SandboxBackend>) -> Self {
        self.sandbox_backend = Some(backend);
        self
    }

    /// Set the user's trusted external paths (loaded from
    /// `~/.deepseek/workspace-trust.json`). See [`Self::resolve_path`] for
    /// how the list is consulted.
    #[must_use]
    pub fn with_trusted_external_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.trusted_external_paths = paths;
        self
    }

    /// Attach an LSP manager so that edit tools can auto-inject diagnostics
    /// into their results after a successful file modification (#428).
    #[must_use]
    #[allow(dead_code)]
    pub fn with_lsp_manager(mut self, manager: Arc<LspManager>) -> Self {
        self.lsp_manager = Some(manager);
        self
    }

    /// Resolve a path relative to workspace, validating it doesn't escape.
    ///
    /// This handles both existing files (using canonicalize) and non-existent files
    /// (for write operations) by canonicalizing the parent directory and appending
    /// the filename.
    /// Resolve a path relative to workspace, validating it doesn't escape.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// # use crate::tools::spec::ToolContext;
    /// let ctx = ToolContext::new(".");
    /// let path = ctx.resolve_path("README.md")?;
    /// # Ok::<(), crate::tools::spec::ToolError>(())
    /// ```
    pub fn resolve_path(&self, raw: &str) -> Result<PathBuf, ToolError> {
        let candidate = if std::path::Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            self.workspace.join(raw)
        };

        // In trust mode, allow any path without validation
        if self.trust_mode {
            // Still try to canonicalize for consistency, but don't require it
            return Ok(candidate.canonicalize().unwrap_or(candidate));
        }

        // Try to canonicalize the workspace
        let workspace_canonical = self
            .workspace
            .canonicalize()
            .unwrap_or_else(|_| self.workspace.clone());

        // For the initial check, also try to canonicalize the candidate if possible
        // This handles symlinks like /var -> /private/var on macOS
        let candidate_canonical = candidate
            .canonicalize()
            .unwrap_or_else(|_| normalize_path(&candidate));
        let workspace_normalized = normalize_path(&workspace_canonical);

        // Check if the candidate is under the workspace (comparing canonical paths)
        if !candidate_canonical.starts_with(&workspace_normalized) {
            // Also try with non-canonical workspace for cases where workspace itself
            // hasn't been canonicalized yet
            let workspace_plain = normalize_path(&self.workspace);
            let candidate_normalized = normalize_path(&candidate);
            if !candidate_normalized.starts_with(&workspace_plain)
                && !self.is_trusted_external_path(&candidate_canonical)
                && !self.is_trusted_external_path(&candidate_normalized)
            {
                return Err(ToolError::PathEscape {
                    path: candidate_canonical,
                });
            }
        }

        // For existing paths, use canonicalize directly
        if candidate.exists() {
            let canonical = candidate.canonicalize().map_err(|e| {
                ToolError::execution_failed(format!(
                    "Failed to canonicalize {}: {}",
                    candidate.display(),
                    e
                ))
            })?;

            if !canonical.starts_with(&workspace_canonical)
                && !self.is_trusted_external_path(&canonical)
            {
                return Err(ToolError::PathEscape { path: canonical });
            }

            return Ok(canonical);
        }

        // For non-existent paths (e.g., files to be created), validate via parent
        // Find the deepest existing ancestor and canonicalize it
        let mut existing_ancestor = candidate.clone();
        let mut suffix_parts: Vec<std::ffi::OsString> = Vec::new();

        while !existing_ancestor.exists() {
            if let Some(file_name) = existing_ancestor.file_name() {
                suffix_parts.push(file_name.to_owned());
            }
            match existing_ancestor.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => {
                    existing_ancestor = parent.to_path_buf();
                }
                _ => {
                    // No existing parent found; fall back to simple check
                    break;
                }
            }
        }

        let canonical_ancestor = if existing_ancestor.exists() {
            existing_ancestor
                .canonicalize()
                .unwrap_or(existing_ancestor)
        } else {
            existing_ancestor
        };

        // Rebuild the full path from canonicalized ancestor
        let mut canonical = canonical_ancestor;
        for part in suffix_parts.into_iter().rev() {
            canonical.push(part);
        }
        let canonical = normalize_path(&canonical);

        // Validate it's under workspace, OR is under a user-trusted external
        // path (`/trust add <path>` from the slash command, persisted in
        // `~/.deepseek/workspace-trust.json`).
        if !canonical.starts_with(&workspace_canonical)
            && !canonical.starts_with(&workspace_normalized)
            && !self.is_trusted_external_path(&canonical)
        {
            return Err(ToolError::PathEscape { path: canonical });
        }

        Ok(canonical)
    }

    /// Whether `path` is under any of the user-trusted external roots. The
    /// caller should pass an already-canonicalized (or normalized) path.
    fn is_trusted_external_path(&self, path: &Path) -> bool {
        self.trusted_external_paths
            .iter()
            .any(|trusted| path.starts_with(trusted))
    }

    /// Set the trust mode.
    #[allow(dead_code)]
    pub fn with_trust_mode(mut self, trust: bool) -> Self {
        self.trust_mode = trust;
        self
    }

    /// Set the sandbox policy.
    #[allow(dead_code)]
    pub fn with_sandbox_policy(mut self, policy: SandboxPolicy) -> Self {
        self.sandbox_policy = policy;
        self
    }

    /// Set feature flags for tool execution.
    pub fn with_features(mut self, features: Features) -> Self {
        self.features = features;
        self
    }

    /// Override the shared shell manager.
    pub fn with_shell_manager(mut self, shell_manager: SharedShellManager) -> Self {
        self.shell_manager = shell_manager;
        self
    }

    /// Set the elevated sandbox policy override.
    ///
    /// This is used when retrying a tool after a sandbox denial, to run
    /// with elevated permissions.
    pub fn with_elevated_sandbox_policy(mut self, policy: crate::sandbox::SandboxPolicy) -> Self {
        self.elevated_sandbox_policy = Some(policy);
        self
    }

    /// Set the shell network-denial hint used by network-restricted modes.
    pub fn with_shell_network_denied_hint(mut self, hint: impl Into<String>) -> Self {
        self.shell_network_denied_hint = Some(hint.into());
        self
    }

    /// Set the namespace used for session-scoped tool state.
    pub fn with_state_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.state_namespace = namespace.into();
        self
    }

    /// Attach the large-output router (#548). When set, tool results that
    /// exceed the configured token threshold are synthesised by a V4-Flash
    /// sub-agent before being returned to the parent context.
    #[must_use]
    pub fn with_large_output_router(
        mut self,
        router: crate::tools::large_output_router::LargeOutputRouter,
        vars: std::sync::Arc<
            tokio::sync::Mutex<crate::tools::large_output_router::WorkshopVariables>,
        >,
    ) -> Self {
        self.large_output_router = Some(router);
        self.workshop_vars = Some(vars);
        self
    }
}

/// Gather LSP diagnostics for `paths` using the manager stored in `context`,
/// and return the rendered `<diagnostics …>` blocks joined by newlines.
///
/// Returns an empty string when:
/// - `context.lsp_manager` is `None`
/// - the manager's `enabled` flag is `false`
/// - none of the files produce diagnostics (e.g. all clean, or language unknown)
///
/// This function is non-blocking by design: every failure mode (missing LSP
/// binary, timeout, unknown language) degrades to an empty string rather than
/// propagating an error to the caller.
pub async fn lsp_diagnostics_for_paths(context: &ToolContext, paths: &[PathBuf]) -> String {
    use crate::lsp::render_blocks;

    let manager = match context.lsp_manager.as_ref() {
        Some(m) if m.config().enabled => m,
        _ => return String::new(),
    };

    let mut blocks = Vec::new();
    for (idx, path) in paths.iter().enumerate() {
        if let Some(block) = manager.diagnostics_for(path, idx as u64).await {
            blocks.push(block);
        }
    }

    render_blocks(&blocks)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut prefix: Option<std::ffi::OsString> = None;
    let mut is_root = false;
    let mut stack: Vec<std::ffi::OsString> = Vec::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix_component) => {
                prefix = Some(prefix_component.as_os_str().to_owned());
            }
            Component::RootDir => {
                is_root = true;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                let parent = Component::ParentDir.as_os_str();
                if let Some(last) = stack.pop() {
                    if last == parent {
                        stack.push(last);
                        stack.push(parent.to_owned());
                    }
                } else if !is_root {
                    stack.push(parent.to_owned());
                }
            }
            Component::Normal(part) => {
                stack.push(part.to_owned());
            }
        }
    }

    let mut normalized = PathBuf::new();
    if let Some(prefix) = prefix {
        normalized.push(prefix);
    }
    if is_root {
        normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR));
    }
    for part in stack {
        normalized.push(part);
    }
    normalized
}

/// The core trait that all tools must implement.
#[async_trait]
pub trait ToolSpec: Send + Sync {
    /// Returns the unique name of this tool (used in API calls).
    fn name(&self) -> &str;

    /// Returns a human-readable description of what this tool does.
    fn description(&self) -> &str;

    /// Returns the JSON Schema for the tool's input parameters.
    fn input_schema(&self) -> Value;

    /// Returns the capabilities this tool has.
    fn capabilities(&self) -> Vec<ToolCapability>;

    /// Returns the approval requirement for this tool.
    fn approval_requirement(&self) -> ApprovalRequirement {
        let caps = self.capabilities();
        if caps.contains(&ToolCapability::ExecutesCode) {
            ApprovalRequirement::Required
        } else if caps.contains(&ToolCapability::WritesFiles) {
            ApprovalRequirement::Suggest
        } else {
            ApprovalRequirement::Auto
        }
    }

    /// Returns whether this tool is sandboxable.
    #[allow(dead_code)]
    fn is_sandboxable(&self) -> bool {
        self.capabilities().contains(&ToolCapability::Sandboxable)
    }

    /// Returns whether this tool is read-only.
    fn is_read_only(&self) -> bool {
        let caps = self.capabilities();
        caps.contains(&ToolCapability::ReadOnly)
            && !caps.contains(&ToolCapability::WritesFiles)
            && !caps.contains(&ToolCapability::ExecutesCode)
    }

    /// Returns whether this tool can be executed in parallel with others.
    fn supports_parallel(&self) -> bool {
        false
    }

    /// Returns whether this tool should be excluded from the model-visible
    /// tool catalog (deferred loading). Tools marked `true` are registered
    /// but not sent to the model until explicitly activated via tool search.
    fn defer_loading(&self) -> bool {
        false
    }

    /// Execute the tool with the given input and context.
    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError>;
}

// === Unit Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn test_tool_result_success() {
        let result = ToolResult::success("hello");
        assert!(result.success);
        assert_eq!(result.content, "hello");
        assert!(result.metadata.is_none());
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult::error("something failed");
        assert!(!result.success);
        assert_eq!(result.content, "something failed");
    }

    #[test]
    fn test_tool_result_json() {
        let data = json!({"key": "value"});
        let result = ToolResult::json(&data).unwrap();
        assert!(result.success);
        assert!(result.content.contains("key"));
    }

    #[test]
    fn test_tool_result_with_metadata() {
        let result = ToolResult::success("content").with_metadata(json!({"extra": true}));
        assert!(result.metadata.is_some());
    }

    #[test]
    fn test_tool_context_resolve_path_relative() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // Create a test file
        let test_file = tmp.path().join("test.txt");
        std::fs::write(&test_file, "test").expect("write");

        let resolved = ctx.resolve_path("test.txt").expect("resolve");
        assert!(resolved.ends_with("test.txt"));
    }

    #[test]
    fn test_tool_context_resolve_path_escape() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // Try to escape workspace
        let result = ctx.resolve_path("/etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_tool_context_resolve_path_parent_traversal() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let result = ctx.resolve_path("../escape.txt");
        assert!(result.is_err());
    }

    #[test]
    fn test_tool_context_resolve_path_normalizes_parent() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let result = ctx.resolve_path("new/../safe.txt");
        assert!(result.is_ok());
    }

    #[test]
    fn test_tool_context_trust_mode() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf()).with_trust_mode(true);

        // In trust mode, absolute paths should work
        let result = ctx.resolve_path("/tmp");
        assert!(result.is_ok());
    }

    /// Issue #29: paths under a user-trusted external directory resolve
    /// successfully even though they fall outside the workspace, while
    /// untrusted external paths still error with `PathEscape`.
    #[test]
    fn test_tool_context_trusted_external_path_allows_escape() {
        let workspace = tempdir().expect("workspace tempdir");
        let trusted_root = tempdir().expect("trusted tempdir");
        let trusted_file = trusted_root.path().join("notes.md");
        std::fs::write(&trusted_file, "shared notes").unwrap();

        let ctx =
            ToolContext::new(workspace.path().to_path_buf()).with_trusted_external_paths(vec![
                trusted_root
                    .path()
                    .canonicalize()
                    .unwrap_or_else(|_| trusted_root.path().to_path_buf()),
            ]);

        let resolved = ctx
            .resolve_path(trusted_file.to_str().unwrap())
            .expect("trusted path should resolve");
        assert!(resolved.ends_with("notes.md"));

        // Path outside workspace AND outside the trust list should still fail.
        let other = tempdir().expect("untrusted tempdir");
        let other_file = other.path().join("secret.md");
        std::fs::write(&other_file, "x").unwrap();
        let err = ctx
            .resolve_path(other_file.to_str().unwrap())
            .expect_err("untrusted path must error");
        assert!(matches!(err, ToolError::PathEscape { .. }));
    }

    #[test]
    fn test_required_str() {
        let input = json!({"name": "test", "count": 42});
        assert_eq!(required_str(&input, "name").unwrap(), "test");
        assert!(required_str(&input, "missing").is_err());
        assert!(required_str(&input, "count").is_err()); // not a string
    }

    #[test]
    fn test_optional_str() {
        let input = json!({"name": "test"});
        assert_eq!(optional_str(&input, "name"), Some("test"));
        assert_eq!(optional_str(&input, "missing"), None);
    }

    #[test]
    fn test_required_u64() {
        let input = json!({"count": 42});
        assert_eq!(required_u64(&input, "count").unwrap(), 42);
        assert!(required_u64(&input, "missing").is_err());
    }

    #[test]
    fn test_optional_u64() {
        let input = json!({"count": 42});
        assert_eq!(optional_u64(&input, "count", 0), 42);
        assert_eq!(optional_u64(&input, "missing", 100), 100);
    }

    #[test]
    fn test_optional_bool() {
        let input = json!({"flag": true});
        assert!(optional_bool(&input, "flag", false));
        assert!(!optional_bool(&input, "missing", false));
    }

    #[test]
    fn test_tool_error_display() {
        let err = ToolError::missing_field("path");
        assert_eq!(
            format!("{err}"),
            "Failed to validate input: missing required field 'path'"
        );

        let err = ToolError::execution_failed("boom");
        assert_eq!(format!("{err}"), "Failed to execute tool: boom");
    }

    #[test]
    fn test_approval_requirement_default() {
        let level = ApprovalRequirement::default();
        assert_eq!(level, ApprovalRequirement::Auto);
    }
}
