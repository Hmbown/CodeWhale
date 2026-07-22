//! Tool specification traits for the CodeWhale agent system.
//!
//! This module defines the core abstractions for tools:
//! - `ToolSpec`: The main trait that all tools must implement
//! - `ToolContext`: Execution context passed to tools
//! - `ToolResult`: Unified result type for tool execution
//! - `ToolCapability`: Capabilities and requirements of tools

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use unicode_normalization::UnicodeNormalization;

use crate::features::Features;
use crate::lsp::LspManager;
use crate::network_policy::NetworkPolicyDecider;
use crate::rlm::session::SessionObjectSnapshot;
use crate::rlm::session::{SharedRlmSessionStore, new_shared_rlm_session_store};
use crate::sandbox::backend::SandboxBackend;
use crate::tools::handle::{SharedHandleStore, new_shared_handle_store};
use crate::tools::shell::{SharedShellManager, new_shared_shell_manager};
use crate::worker_profile::ShellPolicy;
#[allow(unused_imports)]
pub use codewhale_tools::{
    ApprovalRequirement, PreparedToolCall, ResourceClaim, ToolCapability, ToolError,
    ToolExecutionOutcome, ToolResult, ToolTerminalStatus, optional_bool, optional_str,
    optional_u64, required_str, required_u64, schedule_non_conflicting,
};

#[async_trait]
pub trait DynamicToolExecutor: Send + Sync {
    async fn execute_dynamic_tool(
        &self,
        thread_id: Option<String>,
        namespace: Option<String>,
        name: String,
        input: Value,
    ) -> Result<ToolResult, ToolError>;
}

/// Optional durable runtime services made available to model-visible tools.
///
/// These are intentionally optional so existing unit tests and one-off tool
/// contexts keep working. Tools that need durable task/automation state fail
/// closed with a clear "not available" error when the relevant service is not
/// attached.
#[derive(Clone)]
pub struct RuntimeToolServices {
    pub shell_manager: Option<SharedShellManager>,
    pub task_manager: Option<crate::task_manager::SharedTaskManager>,
    pub automations: Option<crate::automation_manager::SharedAutomationManager>,
    pub task_data_dir: Option<PathBuf>,
    pub active_task_id: Option<String>,
    pub active_thread_id: Option<String>,
    pub dynamic_tool_executor: Option<Arc<dyn DynamicToolExecutor>>,
    /// Active-session Work Graph authority plus its legacy Plan/To-do views.
    pub work: Option<crate::work_graph::SharedWorkRuntime>,
    /// Hook executor for `shell_env` injection (#456) and any future
    /// tool-side hook events. `None` outside the live engine — test
    /// contexts that don't care about hooks get a no-op.
    pub hook_executor: Option<std::sync::Arc<crate::hooks::HookExecutor>>,
    /// Per-session backing store for `var_handle` payloads. Cloned tool
    /// contexts share this Arc so handles survive across turns.
    pub handle_store: SharedHandleStore,
    /// Per-session persistent RLM kernels, keyed by caller-chosen context name.
    pub rlm_sessions: SharedRlmSessionStore,
}

impl Default for RuntimeToolServices {
    fn default() -> Self {
        Self {
            shell_manager: None,
            task_manager: None,
            automations: None,
            task_data_dir: None,
            active_task_id: None,
            active_thread_id: None,
            dynamic_tool_executor: None,
            work: None,
            hook_executor: None,
            handle_store: new_shared_handle_store(),
            rlm_sessions: new_shared_rlm_session_store(),
        }
    }
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
            .field(
                "dynamic_tool_executor",
                &self.dynamic_tool_executor.is_some(),
            )
            .field("work", &self.work.is_some())
            .field("hook_executor", &self.hook_executor.is_some())
            .field("handle_store", &true)
            .field("rlm_sessions", &true)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileReadSnapshot {
    len: u64,
    modified: Option<SystemTime>,
}

#[derive(Debug, Default)]
pub struct FileReadTracker {
    reads: HashMap<PathBuf, FileReadSnapshot>,
}

pub type SharedFileReadTracker = Arc<Mutex<FileReadTracker>>;

pub(crate) fn new_shared_file_read_tracker() -> SharedFileReadTracker {
    Arc::new(Mutex::new(FileReadTracker::default()))
}

fn file_read_snapshot(path: &Path) -> Result<FileReadSnapshot, ToolError> {
    let metadata = fs::metadata(path).map_err(|e| {
        ToolError::execution_failed(format!("Failed to inspect {}: {e}", path.display()))
    })?;
    Ok(FileReadSnapshot {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

/// Sandbox policy for command execution.
#[derive(Debug, Clone, Default)]
pub enum SandboxPolicy {
    /// No sandboxing (dangerous but sometimes needed)
    #[default]
    None,
}

/// Machine-readable mutation boundary for a headless worker process.
///
/// Fleet serializes this envelope onto the exact `codewhale exec` argv. The
/// child installs it before constructing its engine, and every ToolContext in
/// that process inherits the same outer cap. Nested agents may narrow this
/// boundary, but cannot remove or expand it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolAuthorityEnvelope {
    pub schema_version: u32,
    pub owner: String,
    pub authority: ToolMutationAuthority,
    #[serde(default)]
    pub writable_roots: Vec<String>,
    #[serde(default)]
    pub writable_files: Vec<String>,
    #[serde(default)]
    pub coordination_contracts: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolMutationAuthority {
    ReadOnly,
    ScopedWrite,
}

static PROCESS_TOOL_AUTHORITY: OnceLock<Arc<ToolAuthorityEnvelope>> = OnceLock::new();

impl ToolAuthorityEnvelope {
    pub fn normalized(mut self) -> Result<Self, String> {
        if self.schema_version != 1 {
            return Err(format!(
                "unsupported tool authority schema version {}",
                self.schema_version
            ));
        }
        self.owner = bounded_authority_value("owner", &self.owner, 128)?;
        self.writable_roots = normalize_authority_paths(&self.writable_roots, "writable_roots")?;
        self.writable_files = normalize_authority_paths(&self.writable_files, "writable_files")?;
        self.coordination_contracts = normalize_authority_values(
            &self.coordination_contracts,
            "coordination_contracts",
            16,
            128,
        )?;
        if self.authority == ToolMutationAuthority::ScopedWrite
            && self.writable_roots.is_empty()
            && self.writable_files.is_empty()
            && self.coordination_contracts.is_empty()
        {
            return Err(
                "scoped_write authority requires a writable root, exact file, or coordination contract"
                    .to_string(),
            );
        }
        if self.authority == ToolMutationAuthority::ReadOnly
            && (!self.writable_roots.is_empty()
                || !self.writable_files.is_empty()
                || !self.coordination_contracts.is_empty())
        {
            return Err("read_only authority cannot carry mutation scope".to_string());
        }
        Ok(self)
    }

    pub fn from_json(raw: &str) -> Result<Self, String> {
        serde_json::from_str::<Self>(raw)
            .map_err(|error| format!("invalid tool authority envelope: {error}"))?
            .normalized()
    }

    #[cfg(test)]
    fn is_within(&self, outer: &Self) -> bool {
        if self.authority == ToolMutationAuthority::ReadOnly {
            return true;
        }
        if outer.authority != ToolMutationAuthority::ScopedWrite {
            return false;
        }
        self.writable_roots.iter().all(|path| {
            outer
                .writable_roots
                .iter()
                .any(|root| authority_path_is_within_root(path, root))
        }) && self.writable_files.iter().all(|path| {
            outer.writable_files.contains(path)
                || outer
                    .writable_roots
                    .iter()
                    .any(|root| authority_path_is_within_root(path, root))
        }) && self
            .coordination_contracts
            .iter()
            .all(|contract| outer.coordination_contracts.contains(contract))
    }

    pub fn permits_mutation_path(
        &self,
        context: &ToolContext,
        raw_path: &str,
    ) -> Result<bool, ToolError> {
        if self.authority == ToolMutationAuthority::ReadOnly {
            return Ok(false);
        }
        let target = resolve_strict_authority_path(context, raw_path)?;
        for file in &self.writable_files {
            if resolve_strict_authority_path(context, file)? == target {
                return Ok(true);
            }
        }
        for root in &self.writable_roots {
            if target.starts_with(resolve_strict_authority_path(context, root)?) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
fn authority_path_is_within_root(path: &str, root: &str) -> bool {
    root == "."
        || path == root
        || path
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

pub fn install_process_tool_authority(envelope: ToolAuthorityEnvelope) -> Result<(), String> {
    let envelope = Arc::new(envelope.normalized()?);
    if let Some(existing) = PROCESS_TOOL_AUTHORITY.get() {
        return if existing.as_ref() == envelope.as_ref() {
            Ok(())
        } else {
            Err("tool authority envelope was already installed for this process".to_string())
        };
    }
    PROCESS_TOOL_AUTHORITY
        .set(envelope)
        .map_err(|_| "tool authority envelope was already installed for this process".to_string())
}

fn process_tool_authority() -> Option<Arc<ToolAuthorityEnvelope>> {
    PROCESS_TOOL_AUTHORITY.get().cloned()
}

fn bounded_authority_value(field: &str, value: &str, max_chars: usize) -> Result<String, String> {
    let value = value.trim().nfc().collect::<String>();
    if value.is_empty()
        || value.chars().count() > max_chars
        || value.chars().any(|ch| matches!(ch, '\0' | '\r' | '\n'))
    {
        return Err(format!(
            "tool authority {field} must be one non-empty line of at most {max_chars} characters"
        ));
    }
    Ok(value)
}

fn normalize_authority_paths(values: &[String], field: &str) -> Result<Vec<String>, String> {
    if values.len() > 32 {
        return Err(format!("tool authority {field} accepts at most 32 entries"));
    }
    let mut normalized = Vec::new();
    for raw in values {
        let raw = bounded_authority_value(field, raw, 512)?.replace('\\', "/");
        let windows_drive = raw.as_bytes().get(1) == Some(&b':')
            && raw.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
        if raw.starts_with('/') || raw.starts_with("//") || windows_drive {
            return Err(format!(
                "tool authority {field} entries must be repo-relative"
            ));
        }
        let mut segments = Vec::new();
        for segment in raw.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    return Err(format!(
                        "tool authority {field} cannot contain parent traversal"
                    ));
                }
                value => segments.push(value),
            }
        }
        let path = if segments.is_empty() {
            ".".to_string()
        } else {
            segments.join("/")
        };
        if !normalized.contains(&path) {
            normalized.push(path);
        }
    }
    Ok(normalized)
}

fn normalize_authority_values(
    values: &[String],
    field: &str,
    max_entries: usize,
    max_chars: usize,
) -> Result<Vec<String>, String> {
    if values.len() > max_entries {
        return Err(format!(
            "tool authority {field} accepts at most {max_entries} entries"
        ));
    }
    let mut normalized = Vec::new();
    for value in values {
        let value = bounded_authority_value(field, value, max_chars)?;
        if !normalized.contains(&value) {
            normalized.push(value);
        }
    }
    Ok(normalized)
}

pub(crate) fn resolve_strict_authority_path(
    context: &ToolContext,
    raw_path: &str,
) -> Result<PathBuf, ToolError> {
    let normalized = normalize_authority_paths(&[raw_path.to_string()], "mutation_path")
        .map_err(ToolError::permission_denied)?
        .into_iter()
        .next()
        .ok_or_else(|| ToolError::permission_denied("mutation path cannot be empty"))?;
    let workspace = context.workspace.canonicalize().map_err(|error| {
        ToolError::execution_failed(format!(
            "Failed to canonicalize authority workspace {}: {error}",
            context.workspace.display()
        ))
    })?;
    let mut current = workspace.clone();
    if normalized != "." {
        for segment in normalized.split('/') {
            current.push(segment);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(ToolError::permission_denied(format!(
                        "machine-readable authority paths must not traverse symlinks: {}",
                        current.display()
                    )));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(ToolError::execution_failed(format!(
                        "Failed to inspect authority path {}: {error}",
                        current.display()
                    )));
                }
            }
        }
    }
    if !current.starts_with(&workspace) {
        return Err(ToolError::permission_denied(format!(
            "machine-readable authority path escapes workspace: {}",
            current.display()
        )));
    }
    Ok(current)
}

/// Context passed to tools during execution.
#[derive(Clone)]
pub struct ToolContext {
    /// The workspace root directory
    pub workspace: PathBuf,
    /// Per-turn policy and attached services. Kept behind one owned group so
    /// cloning a context preserves the historical value semantics while the
    /// top-level context remains small and stable as services evolve.
    pub execution: Box<ToolExecutionState>,
}

/// Policy and service state attached to one tool-execution context.
///
/// `ToolContext` dereferences to this group for source compatibility with
/// existing tools. New code can use `context.execution` when the grouping is
/// useful, without growing the top-level context by another field per feature.
#[derive(Clone)]
pub struct ToolExecutionState {
    /// Shared shell manager for background tasks and streaming IO.
    pub shell_manager: SharedShellManager,
    /// Per-session snapshots for files successfully observed by `read_file`.
    /// Mutation tools use this to reject narrow edits against unread or stale
    /// content.
    pub file_read_tracker: SharedFileReadTracker,
    /// Sub-agent that owns tool work started through this context. Root user
    /// turns leave this unset; child contexts stamp it so long-running shell
    /// jobs can be attributed in UI surfaces.
    pub owner_agent_id: Option<String>,
    pub owner_agent_name: Option<String>,
    /// Outer process authority cap installed by Fleet/headless dispatch.
    /// `None` for ordinary interactive/root sessions.
    pub(crate) tool_authority: Option<Arc<ToolAuthorityEnvelope>>,
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
    /// Explicit skills directory used for model-visible skill discovery.
    pub skills_dir: Option<PathBuf>,
    /// Restrict skill discovery to CodeWhale-owned roots plus `skills_dir`.
    pub skills_scan_codewhale_only: bool,
    /// Immutable registry snapshot for this workspace/engine context.
    pub plugin_registry: Option<Arc<crate::plugins::PluginRegistry>>,
    /// Elevated sandbox policy override (used when retrying after sandbox denial).
    /// This overrides the default sandbox behavior for shell commands.
    pub elevated_sandbox_policy: Option<crate::sandbox::SandboxPolicy>,
    /// Optional user-facing hint for shell commands that fail because the
    /// active sandbox policy intentionally denies outbound network access.
    pub shell_network_denied_hint: Option<String>,
    /// Whether tools should auto-approve without safety checks (YOLO mode).
    /// When true, command safety analysis is skipped for shell execution.
    pub auto_approve: bool,
    /// Plan-mode `update_plan` calls must create a validated, reviewable graph
    /// proposal. Other modes may update ordinary progress directly.
    pub review_plan_changes: bool,
    /// Effective shell policy for this execution context.
    pub shell_policy: ShellPolicy,
    /// Effective feature flag set for the running session.
    pub features: Features,
    /// Namespace for tool state that should be scoped to the current session/thread.
    pub state_namespace: String,
    /// Effective context window for the active provider/model route. Web tools
    /// use this to keep inline page content below three percent of the route.
    pub route_context_window: Option<u32>,
    /// User-trusted external paths the agent may read/write even when they
    /// fall outside `workspace`. Loaded from `~/.deepseek/workspace-trust.json`
    /// and refreshed when the user runs `/trust add <path>`. Distinct from
    /// `trust_mode`, which is the all-or-nothing legacy switch (#29).
    pub trusted_external_paths: Vec<PathBuf>,
    /// Whether to follow symbolic links during file discovery and tool
    /// operations. When `true`, symlinked directories are traversed and
    /// symlinked paths that resolve outside the workspace are still allowed
    /// (the symlink itself must be inside the workspace). Mirrors the
    /// `workspace_follow_symlinks` setting.
    pub follow_symlinks: bool,
    /// Per-domain network policy (#135). When `None`, network tools fall back
    /// to a permissive default that mirrors pre-v0.7.0 behavior so tests and
    /// other contexts that don't construct a real policy keep working.
    pub network_policy: Option<NetworkPolicyDecider>,
    /// Durable runtime services for task, gate, PR-attempt, GitHub evidence,
    /// and automation tools.
    pub runtime: RuntimeToolServices,
    /// Snapshot of the active prompt/session/history exposed as symbolic RLM
    /// objects. Tools only receive compact cards unless explicitly opening a
    /// bounded object through `rlm_open`.
    pub session_objects: Option<SessionObjectSnapshot>,
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

    /// Which search backend `web_search` should use. Default: DuckDuckGo. Set via
    /// `[search] provider` in config.toml.
    pub search_provider: crate::config::SearchProvider,
    /// API key for Tavily, Bocha, Metaso, Baidu, Volcengine, or Sofya.
    /// `None` for Bing, DuckDuckGo, or SearXNG.
    /// Metaso also falls back to the `METASO_API_KEY` env var.
    /// Baidu also falls back to `BAIDU_SEARCH_API_KEY`.
    pub search_api_key: Option<String>,
    /// Optional DuckDuckGo-compatible HTML endpoint override for `web_search`.
    pub search_base_url: Option<String>,
    /// Opaque client for the active route's documented first-party search
    /// tool. It owns provider authentication internally and is attached only
    /// when the exact route capability says server-side search is supported.
    pub(crate) provider_native_search: Option<crate::client::ProviderNativeSearchClient>,
    /// Exact active route capability facts. Unknown stays fail-closed.
    pub(crate) route_capabilities: codewhale_config::route::RouteCapabilities,

    /// Per-session workshop variable store (#548). Holds the raw content of
    /// the most recent large-tool routing event so the parent can call
    /// `promote_to_context` later. `None` when the router is disabled.
    pub workshop_vars: Option<
        std::sync::Arc<tokio::sync::Mutex<crate::tools::large_output_router::WorkshopVariables>>,
    >,
}

impl std::ops::Deref for ToolContext {
    type Target = ToolExecutionState;

    fn deref(&self) -> &Self::Target {
        &self.execution
    }
}

impl std::ops::DerefMut for ToolContext {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.execution
    }
}

impl ToolContext {
    /// Create a new `ToolContext` with default settings.
    #[must_use]
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let workspace = workspace.into();
        // Prefer .codewhale, fall back to .deepseek for project-local state
        let notes_path = codewhale_config::resolve_project_state_dir(&workspace, "notes.md")
            .expect("hardcoded project notes state path is valid")
            .1;
        let mcp_config_path = codewhale_config::resolve_project_state_dir(&workspace, "mcp.json")
            .expect("hardcoded project MCP state path is valid")
            .1;
        Self::with_options(workspace, false, notes_path, mcp_config_path)
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
            execution: Box::new(ToolExecutionState {
                shell_manager,
                file_read_tracker: new_shared_file_read_tracker(),
                owner_agent_id: None,
                owner_agent_name: None,
                tool_authority: process_tool_authority(),
                trust_mode,
                sandbox_policy: SandboxPolicy::None,
                notes_path: notes_path.into(),
                mcp_config_path: mcp_config_path.into(),
                skills_dir: None,
                skills_scan_codewhale_only: false,
                plugin_registry: None,
                elevated_sandbox_policy: None,
                shell_network_denied_hint: None,
                auto_approve: false,
                review_plan_changes: false,
                shell_policy: ShellPolicy::Full,
                features: Features::with_defaults(),
                state_namespace: "workspace".to_string(),
                route_context_window: None,
                trusted_external_paths: Vec::new(),
                follow_symlinks: false,
                network_policy: None,
                runtime: RuntimeToolServices::default(),
                session_objects: None,
                cancel_token: None,
                sandbox_backend: None,
                memory_path: None,
                lsp_manager: None,
                large_output_router: None,
                search_provider: crate::config::SearchProvider::default(),
                search_api_key: None,
                search_base_url: None,
                provider_native_search: None,
                route_capabilities: codewhale_config::route::RouteCapabilities::default(),
                workshop_vars: None,
            }),
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
        let mut context = Self::with_options(workspace, trust_mode, notes_path, mcp_config_path);
        context.auto_approve = auto_approve;
        context
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

    /// Require plan updates in this turn to remain pending until the user
    /// accepts their graph diff from the Plan review surface.
    #[must_use]
    pub fn with_review_plan_changes(mut self, review: bool) -> Self {
        self.review_plan_changes = review;
        self
    }

    /// Stamp tool work with the sub-agent that owns it.
    #[must_use]
    pub fn with_owner_agent(
        mut self,
        agent_id: impl Into<String>,
        agent_name: impl Into<String>,
    ) -> Self {
        let agent_id = agent_id.into();
        let agent_name = agent_name.into();
        self.owner_agent_id = (!agent_id.trim().is_empty()).then_some(agent_id);
        self.owner_agent_name = (!agent_name.trim().is_empty()).then_some(agent_name);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_tool_authority(
        mut self,
        envelope: ToolAuthorityEnvelope,
    ) -> Result<Self, String> {
        let envelope = envelope.normalized()?;
        if let Some(outer) = self.tool_authority.as_ref()
            && !envelope.is_within(outer)
        {
            return Err(
                "nested tool authority cannot expand its process authority cap".to_string(),
            );
        }
        self.tool_authority = Some(Arc::new(envelope));
        Ok(self)
    }

    /// Attach skill discovery settings for tools that need to resolve
    /// model-visible skills by name.
    #[must_use]
    pub fn with_skills_config(
        mut self,
        skills_dir: impl Into<PathBuf>,
        scan_codewhale_only: bool,
    ) -> Self {
        self.skills_dir = Some(skills_dir.into());
        self.skills_scan_codewhale_only = scan_codewhale_only;
        self
    }

    #[must_use]
    pub fn with_plugin_registry(mut self, registry: Arc<crate::plugins::PluginRegistry>) -> Self {
        self.plugin_registry = Some(registry);
        self
    }

    /// Attach active prompt/history/session symbolic objects for RLM tools.
    #[must_use]
    pub fn with_session_objects(mut self, snapshot: SessionObjectSnapshot) -> Self {
        self.session_objects = Some(snapshot);
        self
    }

    /// Attach the active engine cancellation token.
    #[must_use]
    pub fn with_cancel_token(mut self, cancel_token: CancellationToken) -> Self {
        self.cancel_token = Some(cancel_token);
        self
    }

    /// Attach the effective shell policy for this turn.
    #[must_use]
    pub fn with_shell_policy(mut self, policy: ShellPolicy) -> Self {
        self.shell_policy = policy;
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

    /// Set whether tools should follow symbolic links. When `true`,
    /// `resolve_path` allows symlinked paths that resolve outside the
    /// workspace, and walk-based tools traverse symlinked directories.
    /// Mirrors the `workspace_follow_symlinks` setting.
    #[must_use]
    pub fn with_follow_symlinks(mut self, follow: bool) -> Self {
        self.follow_symlinks = follow;
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

    /// Remember that the caller has observed the current on-disk state of a
    /// file. This is intentionally best-effort so successful reads/writes do
    /// not fail after completing only because a post-operation metadata lookup
    /// raced with filesystem changes.
    pub fn note_file_read(&self, path: &Path) {
        let Ok(snapshot) = file_read_snapshot(path) else {
            return;
        };
        let Ok(mut tracker) = self.file_read_tracker.lock() else {
            return;
        };
        tracker.reads.insert(path.to_path_buf(), snapshot);
    }

    /// Require a successful, still-fresh `read_file` snapshot before a narrow
    /// in-place edit. This catches model edits made against guessed or stale
    /// content while leaving transactional patch preflight separate.
    pub fn require_fresh_file_read(
        &self,
        path: &Path,
        requested_path: &str,
    ) -> Result<(), ToolError> {
        let prior = {
            let tracker = self.file_read_tracker.lock().map_err(|_| {
                ToolError::execution_failed(
                    "Failed to check read-before-edit state: tracker lock poisoned".to_string(),
                )
            })?;
            tracker.reads.get(path).cloned()
        };

        let Some(prior) = prior else {
            return Err(ToolError::execution_failed(format!(
                "Refusing edit_file for {} because it has not been read in this session. \
                 Recovery: call read_file with path=\"{requested_path}\" to inspect the current contents, \
                 then retry edit_file with a unique search string.",
                path.display()
            )));
        };

        let current = file_read_snapshot(path).map_err(|e| {
            ToolError::execution_failed(format!(
                "Refusing edit_file for {} because the file could not be checked for staleness ({e}). \
                 Recovery: call read_file with path=\"{requested_path}\" again, then retry edit_file.",
                path.display()
            ))
        })?;

        if current != prior {
            return Err(ToolError::execution_failed(format!(
                "Refusing edit_file for {} because it changed since the last read_file call. \
                 Recovery: call read_file with path=\"{requested_path}\" again and retry with the current contents.",
                path.display()
            )));
        }

        Ok(())
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

        // When follow_symlinks is enabled, check the non-canonical (symlink)
        // path against the workspace first. A symlink inside the workspace
        // that resolves outside is allowed — the symlink itself is the gate.
        if self.follow_symlinks {
            let candidate_normalized = normalize_path(&candidate);
            let workspace_normalized = normalize_path(&self.workspace);
            let workspace_canonical_normalized = normalize_path(&workspace_canonical);

            if candidate_normalized.starts_with(&workspace_normalized)
                || candidate_normalized.starts_with(&workspace_canonical_normalized)
            {
                // The symlink (or plain path) is inside the workspace.
                // Return the canonicalized target so file I/O works correctly.
                if candidate.exists() {
                    return Ok(candidate.canonicalize().unwrap_or(candidate));
                }
                // Non-existent path: canonicalize the deepest existing ancestor
                return self.resolve_nonexistent_path(candidate, &workspace_canonical);
            }

            // Path is outside workspace even before resolving symlinks.
            // Fall through to the standard escape check.
        }

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

        self.resolve_nonexistent_path(candidate, &workspace_canonical)
    }

    /// Resolve a non-existent path by canonicalizing its deepest existing
    /// ancestor and validating the result is under the workspace or a
    /// trusted external path.
    fn resolve_nonexistent_path(
        &self,
        candidate: PathBuf,
        workspace_canonical: &Path,
    ) -> Result<PathBuf, ToolError> {
        let workspace_normalized = normalize_path(workspace_canonical);
        let workspace_plain = normalize_path(&self.workspace);
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
        let ancestor_normalized = normalize_path(&existing_ancestor);

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

        if self.follow_symlinks
            && (ancestor_normalized.starts_with(&workspace_plain)
                || ancestor_normalized.starts_with(&workspace_normalized))
        {
            return Ok(canonical);
        }

        // Validate it's under workspace, OR is under a user-trusted external
        // path (`/trust add <path>` from the slash command, persisted in
        // `~/.deepseek/workspace-trust.json`).
        if !canonical.starts_with(workspace_canonical)
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

    /// Reuse the engine's session-scoped read snapshots across tool-context
    /// rebuilds. A fresh context is assembled for each turn, but successful
    /// reads must remain authoritative until the observed file changes.
    pub fn with_file_read_tracker(mut self, tracker: SharedFileReadTracker) -> Self {
        self.file_read_tracker = tracker;
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

    /// Attach the active route's effective context window.
    #[must_use]
    pub fn with_route_context_window(mut self, context_window: u32) -> Self {
        self.route_context_window = (context_window > 0).then_some(context_window);
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

    /// Returns the approval requirement for this concrete tool input.
    fn approval_requirement_for(&self, _input: &Value) -> ApprovalRequirement {
        self.approval_requirement()
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

    /// Returns whether this concrete tool input is read-only.
    fn is_read_only_for(&self, _input: &Value) -> bool {
        self.is_read_only()
    }

    /// Returns whether this tool can be executed in parallel with others.
    fn supports_parallel(&self) -> bool {
        false
    }

    /// Returns whether this concrete tool input can run in parallel.
    fn supports_parallel_for(&self, _input: &Value) -> bool {
        self.supports_parallel()
    }

    /// Returns whether this input starts durable/detached work and returns
    /// immediately. Detached starts are not read-only, but in auto-approved
    /// turns they do not need to block neighboring read-only inspections.
    fn starts_detached_for(&self, _input: &Value) -> bool {
        false
    }

    /// Resolve input-specific policy without performing external side effects.
    ///
    /// Resource claims deliberately default to global exclusivity until a
    /// first-party tool opts into narrower, canonicalized claims. The initial
    /// seam records this decision but leaves the existing scheduler unchanged.
    fn prepare(&self, input: Value, _context: &ToolContext) -> Result<PreparedToolCall, ToolError> {
        Ok(PreparedToolCall {
            name: self.name().to_string(),
            description: self.description().to_string(),
            read_only: self.is_read_only_for(&input),
            supports_parallel: self.supports_parallel_for(&input),
            starts_detached: self.starts_detached_for(&input),
            approval: self.approval_requirement_for(&input),
            resources: vec![ResourceClaim::GlobalExclusive],
            input,
        })
    }

    /// Returns whether this tool should be excluded from the model-visible
    /// tool catalog (deferred loading). Tools marked `true` are registered
    /// but not sent to the model until explicitly activated via tool search.
    fn defer_loading(&self) -> bool {
        false
    }

    /// Returns whether this tool should be advertised in the model-facing
    /// catalog. Hidden compatibility tools remain registered and executable
    /// by name so saved transcripts can replay without teaching new sessions
    /// the deprecated spelling.
    fn model_visible(&self) -> bool {
        true
    }

    /// Execute the tool with the given input and context.
    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError>;
}

// === Unit Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
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

    #[test]
    fn tool_context_keeps_execution_state_grouped_and_value_cloned() {
        let mut context = ToolContext::new(".");
        context.auto_approve = true;
        context.state_namespace = "session-a".to_string();

        assert!(context.execution.auto_approve);
        assert_eq!(context.execution.state_namespace, "session-a");

        let mut cloned = context.clone();
        cloned.state_namespace = "session-b".to_string();
        assert_eq!(context.state_namespace, "session-a");
        assert_eq!(cloned.execution.state_namespace, "session-b");
    }

    #[test]
    fn tool_context_top_level_stays_slim_as_services_grow() {
        assert!(
            std::mem::size_of::<ToolContext>()
                <= std::mem::size_of::<PathBuf>() + 2 * std::mem::size_of::<usize>(),
            "ToolContext should contain only the workspace and boxed execution group"
        );
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
    #[cfg(unix)]
    fn test_tool_context_follow_symlinks_allows_nonexistent_path_under_workspace_symlink() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&workspace).expect("mkdir workspace");
        std::fs::create_dir_all(outside.join("target")).expect("mkdir outside target");
        symlink(outside.join("target"), workspace.join("linked")).expect("symlink");

        let ctx = ToolContext::new(workspace).with_follow_symlinks(true);
        let resolved = ctx
            .resolve_path("linked/new.txt")
            .expect("path under workspace symlink should resolve");

        let expected = outside
            .join("target")
            .canonicalize()
            .expect("canonical target")
            .join("new.txt");
        assert_eq!(resolved, normalize_path(&expected));
    }

    #[test]
    #[cfg(unix)]
    fn test_tool_context_default_mode_rejects_nonexistent_path_under_workspace_symlink() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&workspace).expect("mkdir workspace");
        std::fs::create_dir_all(outside.join("target")).expect("mkdir outside target");
        symlink(outside.join("target"), workspace.join("linked")).expect("symlink");

        let ctx = ToolContext::new(workspace);
        let err = ctx
            .resolve_path("linked/new.txt")
            .expect_err("default mode should still reject workspace symlink escapes");

        assert!(matches!(err, ToolError::PathEscape { .. }));
    }

    fn scoped_authority(roots: &[&str], files: &[&str]) -> ToolAuthorityEnvelope {
        ToolAuthorityEnvelope {
            schema_version: 1,
            owner: "fleet-worker-1".to_string(),
            authority: ToolMutationAuthority::ScopedWrite,
            writable_roots: roots.iter().map(|value| (*value).to_string()).collect(),
            writable_files: files.iter().map(|value| (*value).to_string()).collect(),
            coordination_contracts: Vec::new(),
        }
        .normalized()
        .expect("valid test authority")
    }

    #[test]
    fn tool_authority_allows_normal_nonexistent_children_only_inside_scope() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("src")).expect("src");
        let context = ToolContext::new(tmp.path().to_path_buf());
        let authority = scoped_authority(&["src"], &[]);

        assert!(
            authority
                .permits_mutation_path(&context, "src/new/nested.rs")
                .expect("normal nonexistent child")
        );
        assert!(
            !authority
                .permits_mutation_path(&context, "docs/outside.md")
                .expect("ordinary out-of-scope path")
        );
    }

    #[cfg(unix)]
    #[test]
    fn tool_authority_rejects_exact_file_symlink_aliases() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("src")).expect("src");
        std::fs::create_dir(tmp.path().join("other")).expect("other");
        std::fs::write(tmp.path().join("other/target.rs"), "outside scope\n").expect("target");
        symlink("../other/target.rs", tmp.path().join("src/alias.rs")).expect("alias");
        let context = ToolContext::new(tmp.path().to_path_buf());
        let authority = scoped_authority(&[], &["src/alias.rs"]);

        let error = authority
            .permits_mutation_path(&context, "src/alias.rs")
            .expect_err("an exact-file claim must not authorize a symlink target")
            .to_string();
        assert!(error.contains("must not traverse symlinks"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn tool_authority_rejects_claimed_root_and_child_symlink_aliases() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("real")).expect("real");
        symlink("real", tmp.path().join("linked")).expect("linked root");
        let context = ToolContext::new(tmp.path().to_path_buf());
        let claimed_alias = scoped_authority(&["linked"], &[]);
        let claimed_real = scoped_authority(&["real"], &[]);

        for (authority, path) in [
            (&claimed_alias, "linked/new.rs"),
            (&claimed_real, "linked/new.rs"),
        ] {
            let error = authority
                .permits_mutation_path(&context, path)
                .expect_err("symlinked roots and mutation paths must fail closed")
                .to_string();
            assert!(error.contains("must not traverse symlinks"), "{error}");
        }
    }

    #[test]
    fn nested_tool_authority_may_only_narrow_the_outer_cap() {
        let tmp = tempdir().expect("tempdir");
        let outer = scoped_authority(&["src"], &["Cargo.toml"]);
        let narrower = scoped_authority(&["src/parser"], &[]);
        let expansion = scoped_authority(&["docs"], &[]);
        ToolContext::new(tmp.path().to_path_buf())
            .with_tool_authority(outer.clone())
            .unwrap()
            .with_tool_authority(narrower)
            .expect("nested scope may narrow");
        let error = ToolContext::new(tmp.path().to_path_buf())
            .with_tool_authority(outer.clone())
            .unwrap()
            .with_tool_authority(expansion)
            .err()
            .expect("nested scope expansion must fail closed");
        assert!(error.contains("cannot expand"), "{error}");

        let read_only = ToolAuthorityEnvelope {
            schema_version: 1,
            owner: "read-only-child".to_string(),
            authority: ToolMutationAuthority::ReadOnly,
            writable_roots: Vec::new(),
            writable_files: Vec::new(),
            coordination_contracts: Vec::new(),
        };
        ToolContext::new(tmp.path().to_path_buf())
            .with_tool_authority(outer)
            .unwrap()
            .with_tool_authority(read_only)
            .expect("read-only always narrows a write cap");
    }

    #[test]
    fn process_tool_authority_inherits_into_all_context_constructors() {
        const CHILD_ENV: &str = "CODEWHALE_TEST_PROCESS_TOOL_AUTHORITY_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            let tmp = tempdir().expect("tempdir");
            install_process_tool_authority(ToolAuthorityEnvelope {
                schema_version: 1,
                owner: "fleet-worker-child-process".to_string(),
                authority: ToolMutationAuthority::ReadOnly,
                writable_roots: Vec::new(),
                writable_files: Vec::new(),
                coordination_contracts: Vec::new(),
            })
            .expect("install process authority once in isolated child");
            let notes = tmp.path().join("notes.md");
            let mcp = tmp.path().join("mcp.json");
            let contexts = [
                ToolContext::new(tmp.path().to_path_buf()),
                ToolContext::with_options(
                    tmp.path().to_path_buf(),
                    false,
                    notes.clone(),
                    mcp.clone(),
                ),
                ToolContext::with_auto_approve(tmp.path().to_path_buf(), false, notes, mcp, true),
            ];
            for context in contexts {
                let authority = context
                    .tool_authority
                    .as_ref()
                    .expect("every constructor inherits process authority");
                assert_eq!(authority.owner, "fleet-worker-child-process");
                assert_eq!(authority.authority, ToolMutationAuthority::ReadOnly);
            }
            return;
        }

        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .arg("--exact")
            .arg(
                "tools::spec::tests::process_tool_authority_inherits_into_all_context_constructors",
            )
            .arg("--nocapture")
            .env(CHILD_ENV, "1")
            .output()
            .expect("spawn isolated authority test child");
        assert!(
            output.status.success(),
            "child failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
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
