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
