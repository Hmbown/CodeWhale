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
// Project (FEAT-021 D1/D2/D3/D4)
// ---------------------------------------------------------------------------

/// Portable goal status for the project facet (FEAT-021 D1).
///
/// Mirrors the four TUI-owned `tools::goal::GoalStatus` variants without
/// naming the TUI type. The adapter maps host state onto this enum; handlers
/// compare and render it directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectGoalStatus {
    #[default]
    Active,
    Paused,
    Complete,
    Blocked,
}

/// Portable session-share projection (FEAT-021 D1).
///
/// Carries only the emptiness/length and the model/mode labels the live
/// `/share` handler consumes. The session history itself, exporter I/O, and
/// all `App` state stay host-side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectShareProjection {
    /// Whether the session history is empty (drives the empty-share error).
    pub history_is_empty: bool,
    /// Session history length used in the export message and action.
    pub history_len: usize,
    /// Current model label.
    pub model: String,
    /// Current operating-mode label.
    pub mode_label: String,
}

/// Portable goal projection (FEAT-021 D1).
///
/// Carries the visible goal state, the effective pending-control view, and the
/// session-derived token fallback the live `/goal` handler consumes. Concrete
/// goal-service, session-manager, and `App` types never cross the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectGoalState {
    /// Visible goal objective.
    pub objective: Option<String>,
    /// Visible goal status.
    pub status: ProjectGoalStatus,
    /// Pause reason label when the goal is paused (already rendered).
    pub pause_reason: Option<String>,
    /// Elapsed seconds from `started_at` when present (host-computed).
    pub started_at_elapsed_seconds: Option<u64>,
    /// Seconds of goal time used (stable budget/elapsed source).
    pub time_used_seconds: u64,
    /// Optional token budget.
    pub token_budget: Option<u32>,
    /// Tokens used by the goal engine.
    pub tokens_used: u64,
    /// Session conversation-token total (fallback when tokens_used == 0).
    pub session_total_tokens: u32,
    /// Goal continuation count.
    pub continuation_count: u32,
    /// Whether pending goal controls are queued (effective-state gate).
    pub pending_controls: bool,
    /// Last-known durable objective (session-derived effective source).
    pub last_known_objective: Option<String>,
    /// Last-known durable status (session-derived effective source).
    pub last_known_status: Option<ProjectGoalStatus>,
    /// Whether the conversation has API messages (bare `/goal` context gate).
    pub conversation_present: bool,
    /// Whether the host is currently loading (idle-hint gate).
    pub is_loading: bool,
    /// Whether the goal continuation loop is waiting (idle-hint gate).
    pub goal_continuation_waiting: bool,
}

/// Host project data for the project command group (FEAT-021 D1).
///
/// Exposes the typed, exact-minimum operations the live project handlers
/// consume: `/lsp` status/set state, `/share` session payload data, and
/// `/goal` goal state including the session-derived effective values.
/// `/init` host data flows through the existing `WORKSPACE` facet (D2), so
/// `/init` destructures exactly `WORKSPACE` (D4) and consumes no
/// project-facet method. All results are contract-owned portable values; implementation
/// errors cross as safe text. The TUI adapter is the only place that touches
/// `App`, `config::config`, the goal service, or the session manager.
pub trait CommandProjectContext {
    /// `/lsp` status: whether LSP diagnostics are enabled.
    fn lsp_enabled(&self) -> bool;
    /// `/lsp` set: enable or disable LSP diagnostics.
    fn lsp_set(&mut self, enabled: bool) -> Result<(), String>;
    /// `/share` projection: session emptiness, length, model, and mode label.
    fn share_projection(&self) -> ProjectShareProjection;
    /// `/goal` projection: visible and effective goal state.
    fn goal_state(&self) -> ProjectGoalState;
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
// Skill group (FEAT-022 D1)
// ---------------------------------------------------------------------------

/// Source provenance of a discovered skill (native file vs reviewed plugin snapshot).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSourceKind {
    Native,
    Plugin {
        plugin_name: String,
        plugin_id: String,
    },
}

/// Curated product tier for bundled (shipped) skills.
///
/// The canonical name→tier classification stays in the TUI host
/// (`crate::skills::system::bundled_skill_tier`); the portable projection
/// carries the resolved tier so the handler can render the curated listing
/// without duplicating the canonical bundle list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillBundledTier {
    CoreAgentic,
    FormatTooling,
}

impl SkillBundledTier {
    /// Product-facing tier heading used by the `/skills` listing.
    #[must_use]
    pub fn heading(self) -> &'static str {
        match self {
            Self::CoreAgentic => "Core agentic",
            Self::FormatTooling => "Format & tooling",
        }
    }
}

/// One discovered skill entry (portable).
///
/// The body is intentionally excluded: activation and review receive body
/// text through their own delegates (`SkillActivationOutcome`/`ReviewOutcome`);
/// listing and inspect render name, description, source, and path only (D1
/// exact-minimum).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    pub source: SkillSourceKind,
    /// Native skills carry their on-disk path (inspect output).
    pub path: Option<String>,
    /// Bundled catalog tier; `None` for user/compatible skills.
    pub bundled_tier: Option<SkillBundledTier>,
}

/// Portable projection of the host skill registry (discovery, D1).
///
/// Carries every value the `/skills` and `/skill` handlers render: workspace
/// and configured skills dir displays, discovery mode label, searched
/// directories, entries, warnings, and the enabled-skill total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRegistryProjection {
    pub workspace: String,
    pub skills_dir: String,
    pub mode_label: String,
    pub dirs: Vec<String>,
    pub entries: Vec<SkillEntry>,
    pub warnings: Vec<String>,
    pub total: usize,
}

/// Target scope for skill mutations (`/skill install|update|uninstall|trust`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillTargetScope {
    Project,
    Global,
}

/// Portable mutation outcome mirroring the host receipt variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillMutationOutcome {
    Installed,
    Updated,
    NoChange,
    Removed,
    Trusted,
    Imported,
    AlreadyPresent,
    NeedsApproval(String),
    NetworkDenied(String),
}

/// Synchronous portable receipt for a skill mutation (FEAT-020 D11 mirror):
/// the host owns the async network bridge; the handler renders the receipt
/// byte-identically from these values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillMutationReceipt {
    pub name: String,
    pub safe_target_path: String,
    pub outcome: SkillMutationOutcome,
}

/// One curated remote registry entry (`/skills --remote`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSkillEntry {
    pub name: String,
    pub description: Option<String>,
    pub source: String,
}

/// Remote registry fetch outcome (`/skills --remote`, suggest source).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteRegistryOutcome {
    Loaded { entries: Vec<RemoteSkillEntry> },
    NeedsApproval(String),
    Denied(String),
}

/// Remote recommendation for `/skills suggest <task>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRecommendation {
    pub name: String,
    pub description: Option<String>,
    pub matched_terms: Vec<String>,
}

/// Per-skill outcome of `/skills sync`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSyncEntry {
    Downloaded { name: String, path: String },
    Fresh { name: String },
    Failed { name: String, reason: String },
    Denied { name: String, host: String },
    NeedsApproval { name: String, host: String },
}

/// Aggregate `/skills sync` outcome.
///
/// Registry-level network-policy outcomes are carried as variants so the
/// portable handler composes the exact `needs_approval` / `denied` messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSyncOutcome {
    Done {
        total: usize,
        downloaded: usize,
        fresh: usize,
        failed: usize,
        entries: Vec<SkillSyncEntry>,
    },
    RegistryNeedsApproval(String),
    RegistryDenied(String),
}

/// Successful skill activation data (host performs the side effects).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillActivationOutcome {
    pub name: String,
    pub description: String,
}

/// Activation failures with the exact data the handler renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillActivationError {
    NotFound {
        requested: String,
        available: Vec<String>,
        warnings: Vec<String>,
    },
    PluginRejected {
        name: String,
        reason: String,
    },
}

/// `/review` outcome data (host performs the side effects).
///
/// On success the baseline `/review` renders no message — it only emits the
/// `SendMessage` action — so `Ready` carries no payload (D1 exact-minimum).
/// Warnings are only rendered on the not-found path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewOutcome {
    Ready,
    NotFound {
        skills_dir: String,
        global_dir: String,
        warnings: Vec<String>,
    },
}

/// One snapshot entry for `/restore` listings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotEntry {
    pub id: String,
    pub label: String,
    pub timestamp: i64,
}

/// Host approval posture for the `/restore` trust gate (D4: no MODE_POLICY).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandApprovalState {
    pub yolo: bool,
    pub trust_mode: bool,
}

/// Host skill data for the skills command group (FEAT-022 D1).
///
/// Exposes the typed, exact-minimum operations the live skills handlers
/// consume: discovery (`/skills`), activation (`/skill`), synchronous
/// mutation receipts (`/skill install|update|uninstall|trust`), remote
/// registry + sync (`/skills --remote|sync|suggest`), review (`/review`),
/// and snapshot list/restore plus approval state (`/restore`). The host
/// adapter is the only place that touches `App`, `crate::plugins`,
/// `SnapshotRepo`, `crate::skills` services, config/network policy, and the
/// async runtime bridge. The shared FEAT-015 `CommandSkillsContext` is never
/// widened; active-skill reads use that facet, mutations flow through the
/// delegates here (D2). All results are contract-owned portable values;
/// implementation errors cross as safe text. `/skill` declares this facet
/// plus `CommandSkillsContext` for the baseline cache-refresh policy;
/// `/skills`, `/review`, and `/restore` declare exactly this facet.
pub trait CommandSkillGroupContext {
    /// `/skills` discovery projection (workspace, skills dir, scan mode,
    /// searched directories, plugin-provided skills, warnings).
    fn skill_registry_projection(&self) -> SkillRegistryProjection;
    /// `/skill` activation: host lookup, plugin-authority verification, and
    /// active-skill/history side effects. `SendMessage` task composition is
    /// handler-side.
    fn activate_skill(
        &mut self,
        name: &str,
    ) -> Result<SkillActivationOutcome, SkillActivationError>;
    /// `/skill install` — synchronous portable receipt; host owns network/async.
    fn install_skill(
        &mut self,
        scope: Option<SkillTargetScope>,
        spec: &str,
    ) -> Result<SkillMutationReceipt, String>;
    /// `/skill update` — synchronous portable receipt; host owns network/async.
    fn update_skill(
        &mut self,
        scope: Option<SkillTargetScope>,
        name: &str,
    ) -> Result<SkillMutationReceipt, String>;
    /// `/skill uninstall` — synchronous portable receipt.
    fn uninstall_skill(
        &mut self,
        scope: Option<SkillTargetScope>,
        name: &str,
    ) -> Result<SkillMutationReceipt, String>;
    /// `/skill trust` — synchronous portable receipt.
    fn trust_skill(
        &mut self,
        scope: Option<SkillTargetScope>,
        name: &str,
    ) -> Result<SkillMutationReceipt, String>;
    /// `/skills --remote` registry fetch (network policy host-side).
    fn fetch_remote_registry(&mut self) -> Result<RemoteRegistryOutcome, String>;
    /// `/skills suggest <task>` — host fetch + recommendation computation.
    fn recommend_skills(&mut self, task: &str) -> Result<Vec<SkillRecommendation>, String>;
    /// `/skills sync` — host registry sync (async bridge host-side).
    fn sync_registry(&mut self) -> Result<SkillSyncOutcome, String>;
    /// `/review` activation: host discovery + side effects (empty-target
    /// validation and `SendMessage` composition are handler-side).
    fn run_review(&mut self) -> Result<ReviewOutcome, String>;
    /// `/restore` snapshot listing.
    fn snapshot_list(&mut self, limit: usize) -> Result<Vec<SnapshotEntry>, String>;
    /// `/restore <N>`: host restores by snapshot id; handler composes the
    /// exact success message from its list entry.
    fn restore_snapshot(&mut self, id: &str) -> Result<(), String>;
    /// `/restore` trust gate posture (yolo / trust_mode).
    fn approval_state(&self) -> CommandApprovalState;
}
