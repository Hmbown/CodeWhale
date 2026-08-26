//! Independent, object-safe capability shapes for staged command migration.
//!
//! FEAT-014 publishes these interfaces without implementing them for the TUI
//! or changing an existing command. Later work adopts them inside
//! `codewhale-tui` one command group at a time. Only after every group uses
//! these shapes will groups move physically into a commands crate.

use std::path::{Path, PathBuf};

use codewhale_core::request::{Message, SystemPrompt};

use crate::types::{
    CommandApprovalMode, CommandCurrency, CommandMode, CommandProviderId, CommandReasoningEffort,
};

/// Session identity, messages, queue operations, and token totals.
pub trait CommandSessionContext {
    fn session_id(&self) -> Option<String>;
    fn api_messages(&self) -> Vec<Message>;
    fn add_message(&mut self, message: Message);
    fn queued_message_count(&self) -> usize;
    fn remove_queued_message(&mut self, index: usize) -> Result<(), String>;
    fn total_tokens(&self) -> u64;
}

/// Model selection, provider identity, effort, and fallback chain.
pub trait CommandModelContext {
    fn current_model(&self) -> String;
    fn auto_model(&self) -> bool;
    fn set_model_selection(&mut self, model: String, provider: Option<CommandProviderId>);
    fn reasoning_effort(&self) -> CommandReasoningEffort;
    fn provider_identity(&self) -> Option<CommandProviderId>;
    fn fallback_chain(&self) -> Vec<CommandProviderId>;
}

/// Cost display and accounting operations.
pub trait CommandCostContext {
    fn display_currency(&self) -> CommandCurrency;
    fn session_cost_for_currency(&self, currency: CommandCurrency) -> f64;
    fn subagent_cost_for_currency(&self, currency: CommandCurrency) -> f64;
    fn accrue_cost_estimate(&mut self, amount: f64, currency: CommandCurrency);
    fn record_turn_cost(
        &mut self,
        amount: f64,
        currency: CommandCurrency,
        route_receipt: Option<String>,
    );
}

/// Operating mode, approval posture, shell access, and policy lock.
pub trait CommandModePolicyContext {
    fn mode(&self) -> CommandMode;
    fn set_mode(&mut self, mode: CommandMode);
    fn approval_mode(&self) -> CommandApprovalMode;
    fn allow_shell(&self) -> bool;
    fn set_shell_access(&mut self, allow: bool);
    fn policy_locked(&self) -> bool;
}

/// Read access to the effective system prompt.
pub trait CommandSystemPromptContext {
    fn system_prompt(&self) -> Option<SystemPrompt>;
}

/// Active skill identity and skill-cache refresh.
pub trait CommandSkillsContext {
    fn active_skill(&self) -> Option<String>;
    fn active_skill_provenance(&self) -> Option<String>;
    fn refresh_skill_cache(&mut self);
}

/// Workspace path and a bounded serialized work-state snapshot.
pub trait CommandWorkspaceContext {
    fn workspace(&self) -> PathBuf;
    fn work_state_snapshot(&self) -> Result<Option<String>, String>;
    /// Session-aware canonical operation digest. Returns the final user-facing
    /// digest text or a safe explicit error; never a serialized snapshot.
    /// No-active-work and temporary-unavailability semantics are preserved by
    /// the host implementation (FEAT-018 D5).
    fn operation_digest(&mut self) -> Result<String, String>;
}

/// Stable-key translation with named replacements (FEAT-018 D3).
///
/// Message identity uses stable snake_case keys plus named replacements. The
/// TUI host maps those keys to the current catalog and preserves the existing
/// English fallback for intentionally incomplete locale packs. Unknown keys or
/// invalid replacement contracts fail safely and produce a command error; they
/// never panic and never display a raw lookup key.
pub trait CommandPresentationContext {
    /// Resolve a stable message key with its named replacements.
    fn translate(&self, key: &str, replacements: &[(&str, &str)]) -> Result<String, String>;
}

/// Portable receipt for a successful atomic media attachment (FEAT-018 D4).
/// Carries only the information needed for the existing confirmation text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaAttachmentReceipt {
    pub kind: String,
    pub path: std::path::PathBuf,
}

/// Atomic composer/media capability (FEAT-018 D4).
///
/// The host performs media validation and composer insertion as one atomic
/// operation. Rejected, missing, unsupported, corrupt, or oversized media
/// leaves composer state unchanged and returns a safe error. Only portable
/// success information crosses the boundary; composer markup, mutable input
/// text, decoder internals, and TUI types never do.
pub trait CommandMediaContext {
    /// Validate and insert a resolved media path atomically.
    fn attach_media(&mut self, resolved_path: &Path) -> Result<MediaAttachmentReceipt, String>;
}

// ---------------------------------------------------------------------------
// Memory (FEAT-019 D1/D2/D8/D9)
// ---------------------------------------------------------------------------

/// Portable semantic hit for a native-memory search or get result.
///
/// Carries only the typed location and text the handler consumes for
/// formatting; the TUI-owned `NativeMemoryHit` never crosses the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryHit {
    pub source: PathBuf,
    pub line_start: usize,
    pub line_end: usize,
    pub text: String,
}

/// Portable native-memory location summary (status operation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryStatus {
    pub root: PathBuf,
    pub source: PathBuf,
    pub index: PathBuf,
}

/// Portable result of a successful remember operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRemembered {
    pub source: PathBuf,
    pub line_start: usize,
}

/// Portable import outcome: imported (with destination) or skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryImportOutcome {
    Imported { destination: PathBuf },
    Skipped,
}

/// Portable get outcome: found hit or explicit not-found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryGetOutcome {
    Found(MemoryHit),
    NotFound,
}

/// Portable export payload — the exported memory document itself, never a
/// preformatted command response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryExport {
    pub content: String,
}

/// Portable reindex entry count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryReindex {
    pub entry_count: usize,
}

/// Zero-field success value for delete operations (D2): the handler already
/// owns the selected scope and needs no additional success data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MemoryDelete;

/// Typed remember target (D9): the handler resolves workspace identity through
/// the workspace facet and passes the resulting typed ID here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryRememberTarget {
    Global,
    Workspace { workspace_id: String },
}

/// Typed delete scope for the non-workspace delete method (D8/D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryDeleteScope {
    /// Delete every memory entry (global and all workspace scopes).
    All,
    /// Delete only the global scope entries.
    Global,
}

/// Host memory data for the memory command group (FEAT-019 D1).
///
/// Exposes the resolved user-memory file path, the enablement flag, and one
/// typed method per exposed native-memory operation. All results are
/// contract-owned portable values; implementation errors cross as safe text.
/// Workspace-scoped operations take the borrowed workspace path as their first
/// argument (D8); non-workspace operations never receive workspace authority
/// and the facet never captures or retains workspace state internally.
pub trait CommandMemoryContext {
    /// The resolved user-memory file path.
    fn memory_path(&self) -> PathBuf;
    /// Whether the `[memory] enabled` / `DEEPSEEK_MEMORY=on` flag is set.
    fn memory_enabled(&self) -> bool;
    /// Native-memory root, global source, and index paths.
    fn status(&self) -> Result<MemoryStatus, String>;
    /// The native-memory root path.
    fn path(&self) -> Result<PathBuf, String>;
    /// Workspace identity for the given workspace path.
    fn workspace_id(&self, workspace: &Path) -> Result<String, String>;
    /// Workspace-scoped search over the native-memory store.
    fn search(&self, workspace: &Path, query: &str, limit: usize)
    -> Result<Vec<MemoryHit>, String>;
    /// Append a reviewed note to the typed global or workspace target.
    fn remember(
        &self,
        target: MemoryRememberTarget,
        note: &str,
    ) -> Result<MemoryRemembered, String>;
    /// Import legacy memory; distinguishes imported from skipped.
    fn import(&self) -> Result<MemoryImportOutcome, String>;
    /// Workspace-scoped get by entry id; not-found is a typed outcome.
    fn get(&self, workspace: &Path, id: i64) -> Result<MemoryGetOutcome, String>;
    /// Export the native-memory document content.
    fn export(&self) -> Result<MemoryExport, String>;
    /// Reindex the native-memory store; returns the indexed entry count.
    fn reindex(&self) -> Result<MemoryReindex, String>;
    /// Delete all or global scope; never receives workspace authority.
    fn delete(&self, scope: MemoryDeleteScope) -> Result<MemoryDelete, String>;
    /// Delete the given workspace scope; workspace path is the first argument.
    fn delete_workspace(&self, workspace: &Path) -> Result<MemoryDelete, String>;
}

// ---------------------------------------------------------------------------
// Plugin (FEAT-020 D1/D2/D10/D11)
// ---------------------------------------------------------------------------

/// Portable plugin diagnostic level (FEAT-020 D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginDiagnosticLevel {
    Warning,
    Error,
}

/// Portable plugin diagnostic entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDiagnostic {
    pub level: PluginDiagnosticLevel,
    pub code: String,
    pub message: String,
    pub path: Option<PathBuf>,
}

/// Portable MCP transport classification for the capability review body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginMcpTransport {
    Stdio,
    Http,
    Invalid,
}

/// Portable MCP server detail for the capability review body (FEAT-020 D2).
///
/// Carries only the semantic fields `render_mcp_inventory` consumes:
/// transport, command/url, argv, cwd, env provenance, timeouts, required,
/// enabled/disabled tool lists, and the enabled flag. Host `McpServerConfig`
/// never crosses the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMcpServerDetail {
    pub name: String,
    pub transport: PluginMcpTransport,
    pub command: Option<String>,
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub url: Option<String>,
    pub env_headers: Vec<(String, String)>,
    pub bearer_token_env_var: Option<String>,
    pub connect_timeout_secs: Option<u64>,
    pub execute_timeout_secs: Option<u64>,
    pub read_timeout_secs: Option<u64>,
    pub required: bool,
    pub enabled_tools: Vec<String>,
    pub disabled_tools: Vec<String>,
    pub enabled: bool,
}

/// Portable summary of one loaded plugin bundle (list output, FEAT-020 D2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSummary {
    pub name: String,
    pub id: String,
    pub state_label: String,
    pub scope: String,
    pub trust_status: String,
    pub compatibility: String,
    pub inventory: String,
    pub active: bool,
    pub trusted: bool,
    pub enabled: bool,
}

/// Portable full bundle detail for show/review/validate rendering (FEAT-020 D2).
///
/// Carries every semantic value the render helpers consume. The complete
/// `LoadedPlugin` never crosses the boundary; only branch-consumed fields are
/// projected here (D10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDetail {
    pub name: String,
    pub id: String,
    pub version: String,
    pub origin: String,
    pub scope: String,
    pub state_label: String,
    pub trust_status: String,
    pub compatibility: String,
    pub content_hash: String,
    pub capability_hash: String,
    pub canonical_root: PathBuf,
    pub active: bool,
    pub trusted: bool,
    pub enabled: bool,
    pub unsupported_labels: Vec<String>,
    pub supported_labels: Vec<String>,
    pub skills: Vec<String>,
    pub filesystem_roots: Vec<String>,
    pub network_hosts: Vec<String>,
    pub stdio_mcp_servers: usize,
    pub lifecycle_mutation: bool,
    pub mcp_servers: Vec<PluginMcpServerDetail>,
    pub diagnostics: Vec<PluginDiagnostic>,
}

/// Portable outcome of a plugin mutation (FEAT-020 D2/D11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginMutationOutcome {
    Installed,
    Updated,
    NoChange,
    Uninstalled,
    NeedsApproval(String),
    NetworkDenied(String),
}

/// Portable mutation receipt returned synchronously by the facet (FEAT-020 D11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMutationReceipt {
    pub name: String,
    pub path: Option<PathBuf>,
    pub content_hash: Option<String>,
    pub installed_content_hash: Option<String>,
    pub outcome: PluginMutationOutcome,
}

/// Portable bundle export receipt (FEAT-020 D2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginExportReceipt {
    pub exported_name: String,
    pub target: PathBuf,
    pub display_name: Option<String>,
    pub wrote_mcp_json: bool,
    pub files_copied: u64,
    pub skills_normalized: bool,
}

/// Portable legacy executable-tool detail (FEAT-020 D2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginLegacyTool {
    pub name: String,
    pub description: String,
    pub approval: String,
    pub input_schema: Option<String>,
    pub path: PathBuf,
}

/// Portable legacy-tool scan result: directory and discovered tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginLegacyScan {
    pub dir: PathBuf,
    pub tools: Vec<PluginLegacyTool>,
}

/// Portable Kimi managed-plugin candidate (FEAT-020 D2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManagedCandidate {
    pub name: String,
    pub version: String,
    pub license: Option<String>,
    pub canonical_path: PathBuf,
    pub content_hash: String,
    pub capability_hash: String,
    pub inventory: String,
    pub applicable: bool,
}

/// Portable Kimi managed-scan result (FEAT-020 D2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManagedScan {
    pub root: PathBuf,
    pub candidates: Vec<PluginManagedCandidate>,
    pub rejected: Vec<String>,
}

/// Portable marketplace candidate install plan (FEAT-020 D2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginMarketplaceInstallPlan {
    Supported { spec: String, source_kind: String },
    Unsupported { reason: String },
}

/// Portable marketplace candidate (FEAT-020 D2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMarketplaceCandidate {
    pub name: String,
    pub display_name: Option<String>,
    pub version: Option<String>,
    pub tier: String,
    pub compatibility: Option<String>,
    pub install_plan: PluginMarketplaceInstallPlan,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub author: Option<String>,
    pub license: Option<String>,
    pub keywords: Vec<String>,
    pub when: Option<String>,
    pub diagnostics: Vec<PluginDiagnostic>,
    pub has_errors: bool,
}

/// Portable marketplace catalog (FEAT-020 D2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMarketplaceCatalog {
    pub id: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub format: String,
    pub tier: String,
    pub publisher: Option<String>,
    pub total_candidates: usize,
    pub warning_count: usize,
    pub candidates: Vec<PluginMarketplaceCandidate>,
    pub diagnostics: Vec<PluginDiagnostic>,
}

/// Portable marketplace add receipt (FEAT-020 D2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMarketplaceAddReceipt {
    pub name: String,
    pub candidate_count: usize,
    pub warning_count: usize,
    pub catalog: PluginMarketplaceCatalog,
}

/// Portable marketplace state: stored catalogs plus the builtin `official` one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMarketplaceState {
    pub official: PluginMarketplaceCatalog,
    pub stored: Vec<PluginMarketplaceCatalog>,
}

/// Portable suggestion for the `/plugin suggest` recommendation output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSuggestion {
    pub name: String,
    pub description: String,
    pub why: Vec<String>,
    pub next_step: String,
}

/// Host plugin data for the plugin command group (FEAT-020 D1).
///
/// One object-safe, synchronous facet exposing the exact-minimum typed
/// operations the live `/plugin` branch closure consumes. Registry reads and
/// mutations, async-bridged install/update/uninstall (returning synchronous
/// portable receipts), export, legacy executable-tool scan, Kimi managed
/// import, and marketplace operations are all represented. The handler never
/// names `crate::plugins`, `PluginRegistry`, `LoadedPlugin`, `Config`, or
/// another concrete host service; implementation errors cross as safe text.
///
/// Post-mutation side effects (rediscovery, skill-cache refresh, active-skill
/// reset) happen host-side inside the facet implementation; the handler only
/// renders the returned receipt (D11).
pub trait CommandPluginContext {
    /// Read-only: registry summaries for list output.
    fn summaries(&self) -> Result<Vec<PluginSummary>, String>;
    /// Read-only: full portable detail for show/review/validate.
    fn detail(&self, selector: &str) -> Result<PluginDetail, String>;
    /// Read-only: registry-level diagnostics.
    fn registry_diagnostics(&self) -> Vec<PluginDiagnostic>;
    /// Read-only: whether validation reports no errors.
    fn validation_is_clean(&self) -> bool;
    /// Read-only: registry length (used by list/reload empty branches).
    fn len(&self) -> usize;
    /// Read-only: whether the registry is empty.
    fn is_empty(&self) -> bool;
    /// Read-only: persistence store path for marketplace state.
    fn state_path(&self) -> Option<PathBuf>;
    /// Read-only: recommend installed bundles for a task without side effects.
    fn suggest(&self, task: &str) -> Result<Vec<PluginSuggestion>, String>;
    /// Mutation: trust a bundle by exact review token. Success means the
    /// mutation was applied; the handler renders the action word from its own
    /// dispatch arm and may re-read `detail` for post-mutation state.
    fn trust(&mut self, selector: &str, token: &str) -> Result<(), String>;
    /// Mutation: enable a bundle. Success means enabled; re-read `detail` for
    /// the post-mutation compatibility note.
    fn enable(&mut self, selector: &str) -> Result<(), String>;
    /// Mutation: disable a bundle.
    fn disable(&mut self, selector: &str) -> Result<(), String>;
    /// Mutation: revoke trust.
    fn revoke_trust(&mut self, selector: &str) -> Result<(), String>;
    /// Async-bridged install; returns a synchronous portable receipt (D11).
    fn install(
        &mut self,
        source: &str,
        expected_content_hash: Option<&str>,
    ) -> Result<PluginMutationReceipt, String>;
    /// Async-bridged update; returns a synchronous portable receipt (D11).
    fn update(&mut self, selector: &str) -> Result<PluginMutationReceipt, String>;
    /// Async-bridged uninstall; returns a synchronous portable receipt (D11).
    fn uninstall(&mut self, selector: &str) -> Result<PluginMutationReceipt, String>;
    /// Read-only: export a loaded bundle to a target directory.
    fn export(&self, selector: &str, target: &Path) -> Result<PluginExportReceipt, String>;
    /// Read-only: scan legacy executable plugin tools.
    fn legacy_scan(&self) -> Result<Option<PluginLegacyScan>, String>;
    /// Read-only: Kimi managed-plugin directory scan.
    fn managed_scan(&self, home_override: Option<&Path>) -> Result<PluginManagedScan, String>;
    /// Mutation: install a Kimi managed candidate by exact content hash.
    fn managed_install(
        &mut self,
        canonical_path: &Path,
        expected_content_hash: &str,
    ) -> Result<PluginMutationReceipt, String>;
    /// Read-only: marketplace state (builtin official + stored catalogs).
    fn marketplace_state(&self) -> Result<PluginMarketplaceState, String>;
    /// Mutation: add a local catalog document to the marketplace store.
    fn marketplace_add(
        &mut self,
        name: &str,
        path: &Path,
    ) -> Result<PluginMarketplaceAddReceipt, String>;
    /// Mutation: remove a stored marketplace catalog.
    fn marketplace_remove(&mut self, name: &str) -> Result<bool, String>;
    /// Mutation: install a marketplace candidate through the reviewed installer.
    fn marketplace_install(
        &mut self,
        catalog: &str,
        candidate: &str,
    ) -> Result<PluginMutationReceipt, String>;
}
