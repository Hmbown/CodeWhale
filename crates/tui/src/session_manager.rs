//! Session management for resuming conversations.
//!
//! This module provides functionality for:
//! - Saving sessions to disk
//! - Listing previous sessions
//! - Resuming sessions by ID
//! - Managing session lifecycle

use crate::approval_log::{ApprovalReceipt, ApprovalReceiptStore, ApprovalReplay};
use crate::artifacts::ArtifactRecord;
use crate::config::ApiProvider;
use crate::model_routing::AutoRouteReceipt;
use crate::models::{ContentBlock, Message, SystemPrompt};
use crate::project_context::find_git_root;
use crate::session_tree::{SessionEntry, SessionImportContainer, SessionJournal};
use crate::tools::goal::{GoalPauseReason, GoalSnapshot};
use crate::tools::plan::PlanSnapshot;
use crate::tools::todo::TodoListSnapshot;
use crate::tui::file_mention::ContextReference;
use crate::utils::write_atomic;
use crate::work_graph::ReasoningEffortTier;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

/// Maximum number of sessions to retain
const MAX_SESSIONS: usize = 50;
/// Maximum session title length, in `char`s. Matches the bound the session
/// picker's rename prompt has always enforced.
pub const MAX_SESSION_TITLE_CHARS: usize = 100;
const WORK_GRAPH_IMPORT_ARCHIVE_DIR: &str = ".work-graph-import-archive";
const SESSION_GOALS_DIR: &str = ".goals";
const CURRENT_SESSION_GOAL_SCHEMA_VERSION: u32 = 1;
const MAX_SESSION_GOAL_OBJECTIVE_CHARS: usize = 8_192;
const MAX_SESSION_GOAL_FILE_BYTES: u64 = 64 * 1_024;
const CURRENT_SESSION_SCHEMA_VERSION: u32 = 1;
const CURRENT_QUEUE_SCHEMA_VERSION: u32 = 1;

const fn default_session_schema_version() -> u32 {
    CURRENT_SESSION_SCHEMA_VERSION
}

const fn default_queue_schema_version() -> u32 {
    CURRENT_QUEUE_SCHEMA_VERSION
}

fn normalize_managed_dir(path: PathBuf) -> std::io::Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "managed directory path cannot be empty",
        ));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    }) && path.is_relative()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "managed directory path cannot contain traversal components",
        ));
    }
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir().map(|cwd| cwd.join(path))
}

/// Persisted queued message for offline/degraded mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedSessionMessage {
    pub display: String,
    #[serde(default)]
    pub skill_instruction: Option<String>,
    #[serde(default)]
    pub skill_provenance: Option<crate::plugins::types::PluginAuthority>,
}

/// Persisted queue state for recovery after restart/crash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfflineQueueState {
    #[serde(default = "default_queue_schema_version")]
    pub schema_version: u32,
    /// Session ID this queue belongs to. Queue is only restored when
    /// resuming the same session to prevent stale messages leaking into new chats.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub messages: Vec<QueuedSessionMessage>,
    #[serde(default)]
    pub draft: Option<QueuedSessionMessage>,
}

/// Result of explicitly repairing a persisted session for process resume.
///
/// Normal snapshot reads must not infer that an unmatched tool call crashed:
/// an embedding host can persist and inspect a session while that tool is
/// still running. Hosts should use [`SessionManager::load_session_snapshot`]
/// during normal operation and reserve this recovery path for a known process
/// or engine restart.
#[derive(Debug, Clone)]
pub struct SessionRecovery {
    pub session: SavedSession,
    pub changed: bool,
    pub repaired_call_count: usize,
    pub duplicate_result_count: usize,
    pub orphan_result_count: usize,
}

impl Default for OfflineQueueState {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_QUEUE_SCHEMA_VERSION,
            session_id: None,
            messages: Vec::new(),
            draft: None,
        }
    }
}

/// Durable context-reference metadata attached to a user message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionContextReference {
    pub message_index: usize,
    pub reference: ContextReference,
}

/// Session metadata stored with each saved session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Unique session identifier
    pub id: String,
    /// Human-readable title (derived from first message)
    pub title: String,
    /// When the session was created
    pub created_at: DateTime<Utc>,
    /// When the session was last updated
    pub updated_at: DateTime<Utc>,
    /// Number of messages in the session
    pub message_count: usize,
    /// Total tokens used
    pub total_tokens: u64,
    /// Model used for the session
    pub model: String,
    /// Provider used for the session model. Defaults for legacy saved sessions.
    #[serde(default = "default_model_provider")]
    pub model_provider: String,
    /// Exact configured provider key. This is separate from `model_provider`
    /// so old consumers can keep treating that field as the built-in provider
    /// kind (`custom` for every named custom route).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider_id: Option<String>,
    /// Workspace directory
    pub workspace: PathBuf,
    /// Optional mode label (agent/plan/etc.)
    #[serde(default)]
    pub mode: Option<String>,
    /// Accumulated cost data for persisted billing and high-water mark.
    #[serde(default)]
    pub cost: SessionCostSnapshot,
    /// Source session id when this session was created with `deepseek fork`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    /// Source message count at fork time. This is intentionally coarse:
    /// current saved sessions are linear JSON files, not per-entry trees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from_message_count: Option<usize>,
    /// Cumulative turn duration in seconds (sum of completed turn elapsed
    /// times). Persisted so the footer "worked" chip survives restarts
    /// (#2038).
    #[serde(default)]
    pub cumulative_turn_secs: u64,
    /// Durable archive flag (#2934 / #4397). Archived sessions stay on disk
    /// and stay loadable; they are hidden from the default browse surfaces
    /// and are never chosen by auto-resume.
    ///
    /// This mirrors `ThreadRecord::archived` in [`crate::runtime_threads`] so
    /// the TUI session surfaces and the Runtime API/web dashboard project the
    /// same lifecycle field instead of two divergent notions of "put away".
    /// Additive and `skip_serializing_if`-guarded: sessions written before
    /// v0.9.2 load as `archived = false` and round-trip byte-identically
    /// until the flag is actually set.
    #[serde(default, skip_serializing_if = "is_not_archived")]
    pub archived: bool,
    #[serde(default)]
    pub spawn_depth: u32,
}

fn is_not_archived(archived: &bool) -> bool {
    !*archived
}

/// Sessions currently owned by an in-process interactive surface (the TUI).
///
/// A saved session is a file, and a running TUI holds the authoritative copy
/// in memory: it autosaves the whole document from `App` state. That makes an
/// out-of-band write to the *same* session unsafe — the next autosave would
/// silently revert it. Rather than let that happen quietly, the owner claims
/// the id here and any external writer is refused.
///
/// A static registry rather than a field on `RuntimeApiState` because the
/// embedded Runtime API runs inside the TUI process; a standalone
/// `codewhale web` has an empty registry and is therefore never blocked, which
/// is exactly right — there is no TUI holding anything.
static LIVE_SESSIONS: std::sync::OnceLock<std::sync::RwLock<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

fn live_sessions() -> &'static std::sync::RwLock<std::collections::HashSet<String>> {
    LIVE_SESSIONS.get_or_init(Default::default)
}

/// Who is asking to mutate a saved session.
///
/// This is an authority distinction, not a convenience one: the owner may
/// write because it will update its in-memory copy in the same step; anyone
/// else may not, because it cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMutator {
    /// The in-process surface that currently owns the session (the TUI). It
    /// is responsible for updating its cached metadata atomically with the
    /// write — see `App::apply_session_mutation`.
    Owner,
    /// Any other writer: the Runtime API, the web dashboard, a second
    /// process. Refused while the session is claimed.
    External,
}

/// Set the claimed session to exactly `session_id` (or nothing).
///
/// The TUI owns at most one session at a time, so switching sessions must
/// release the previous claim in the same step — otherwise a `/new` would
/// leave the old id permanently locked against the dashboard.
pub fn set_live_session(session_id: Option<&str>) {
    if let Ok(mut live) = live_sessions().write() {
        live.clear();
        if let Some(id) = session_id.map(str::trim).filter(|id| !id.is_empty()) {
            live.insert(id.to_string());
        }
    }
}

/// The canonical session-id shape: a hyphenated UUID, `8-4-4-4-12` hex.
///
/// Used to gate directory removal, so it is deliberately exact rather than
/// permissive — see `reclaim_orphaned_session_dirs`.
fn is_session_uuid(name: &str) -> bool {
    let groups: Vec<&str> = name.split('-').collect();
    if groups.len() != 5 {
        return false;
    }
    const WIDTHS: [usize; 5] = [8, 4, 4, 4, 12];
    groups
        .iter()
        .zip(WIDTHS)
        .all(|(group, width)| group.len() == width && group.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Is this session currently owned by **this process's** interactive surface?
///
/// The registry is process-local. Reclamation must not treat a missing entry
/// here as proof that no other Codewhale process still owns the directory.
#[must_use]
pub fn is_live_session(session_id: &str) -> bool {
    live_sessions()
        .read()
        .is_ok_and(|live| live.contains(session_id))
}

/// The error an external writer gets when the session is live.
///
/// `ResourceBusy` so callers can map it to a typed conflict rather than
/// pattern-matching on a message.
fn live_session_conflict(session_id: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::ResourceBusy,
        format!(
            "session '{session_id}' is open in an interactive Codewhale session; \
             change it there instead — an external write would be reverted by its next autosave"
        ),
    )
}

/// File-name stem of the sidecar mapping session ids to the session
/// instance (process boot) that created their persisted record. Lives in
/// the sessions directory next to the `<id>.json` records it describes.
const SESSION_BOOT_OWNERS_STEM: &str = "session_boot_owners";

static SESSION_BOOT_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Identity of this running session instance (one per process boot).
///
/// Mirrors the `SubAgentManager` boot id from #405: persisted records are
/// stamped with the instance that created them, so a later Codewhale
/// instance in the same workspace can tell restored rows from its own live
/// work (#4416).
#[must_use]
pub fn current_session_boot_id() -> &'static str {
    SESSION_BOOT_ID.get_or_init(|| format!("boot_{}", &Uuid::new_v4().to_string()[..12]))
}

/// Which archive states a session listing includes.
///
/// Deliberately the same three-way shape as
/// [`crate::runtime_threads::ThreadListFilter`] so `/v1/sessions` and
/// `/v1/threads` answer the same `include_archived` / `archived_only` query
/// pair with the same semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionListFilter {
    /// Only `archived = false` sessions. The browse default.
    #[default]
    ActiveOnly,
    /// Active and archived sessions, newest first.
    IncludeArchived,
    /// Only `archived = true` sessions.
    ArchivedOnly,
}

impl SessionListFilter {
    /// Resolve the `include_archived` / `archived_only` query pair the same
    /// way the threads routes do.
    #[must_use]
    pub fn from_query(include_archived: Option<bool>, archived_only: Option<bool>) -> Self {
        if archived_only.unwrap_or(false) {
            Self::ArchivedOnly
        } else if include_archived.unwrap_or(false) {
            Self::IncludeArchived
        } else {
            Self::ActiveOnly
        }
    }

    #[must_use]
    pub fn admits(self, archived: bool) -> bool {
        match self {
            Self::ActiveOnly => !archived,
            Self::IncludeArchived => true,
            Self::ArchivedOnly => archived,
        }
    }
}

fn default_model_provider() -> String {
    "deepseek".to_string()
}

impl SessionMetadata {
    pub(crate) fn set_model_provider_route(&mut self, kind: &str, identity: Option<&str>) {
        self.model_provider = kind.to_string();
        self.model_provider_id = identity.map(str::to_string);
    }
}

/// Cost and high-water-mark fields persisted with each session.
///
/// The coverage fields below are persisted **alongside** the money so a restored
/// session can still say what its total covers. Without them a reload produced a
/// dollar figure with no completeness information, which then rendered as "0 of 0
/// turns priced" — a fabricated claim of a complete total. Sessions written
/// before these fields existed deserialize them from `Default`, which is
/// indistinguishable from that same false reading, so the load path detects the
/// legacy shape explicitly (see [`Self::coverage_is_legacy_unknown`]) rather than
/// trusting the defaults (#4318).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionCostSnapshot {
    /// Accumulated parent-turn session cost in USD.
    #[serde(default)]
    pub session_cost_usd: f64,
    /// Accumulated parent-turn session cost in CNY.
    #[serde(default)]
    pub session_cost_cny: f64,
    /// Accumulated sub-agent/background LLM cost in USD.
    #[serde(default)]
    pub subagent_cost_usd: f64,
    /// Accumulated sub-agent/background LLM cost in CNY.
    #[serde(default)]
    pub subagent_cost_cny: f64,
    /// Max-ever displayed session+subagent cost in USD (preserves #244
    /// monotonic guarantee across session restarts).
    #[serde(default)]
    pub displayed_cost_high_water_usd: f64,
    /// Max-ever displayed session+subagent cost in CNY.
    #[serde(default)]
    pub displayed_cost_high_water_cny: f64,
    /// Turns whose route was money-metered and produced an authoritative price.
    /// These are exactly the turns the persisted totals contain.
    #[serde(default)]
    pub priced_turns: u32,
    /// Money-metered (or unknown-basis) turns that produced no authoritative
    /// price, so their spend is missing from the persisted totals.
    #[serde(default)]
    pub unpriced_turns: u32,
    /// CNY-specific coverage. USD-only routes are unpriced in CNY rather than
    /// silently contributing a fabricated zero.
    #[serde(default)]
    pub cny_priced_turns: u32,
    #[serde(default)]
    pub cny_unpriced_turns: u32,
    /// Stable reason labels for the unpriced turns.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub unpriced_reasons: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub cny_unpriced_reasons: BTreeSet<String>,
    /// Token classes used on some route that carry no published price.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub unpriced_classes: BTreeSet<String>,
    /// Provenance labels of the pricing rows the totals were built from.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub pricing_provenances: BTreeSet<String>,
    /// Live-pricing downgrade receipts recorded while building the totals.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub live_pricing_defects: BTreeSet<String>,
    /// Live rows that failed validation and had no usable bundled fallback.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub live_pricing_unusable_defects: BTreeSet<String>,
    /// Redacted per-route receipts: provider, configured identity, wire model,
    /// billing surface, endpoint fingerprint, billing mode, currency. Never a URL, a
    /// credential, or a filesystem path.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub route_receipts: BTreeSet<String>,
    /// Redacted provider-response identities already included in the live and
    /// durable sub-agent totals. Worker records persist the same fingerprints.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub usage_source_fingerprints: BTreeSet<String>,
    /// Written by builds that track coverage, so a reader can tell "this session
    /// genuinely had zero money-metered turns" apart from "this session predates
    /// coverage tracking". Absent on legacy rows.
    #[serde(default)]
    pub coverage_recorded: bool,
}

impl SessionCostSnapshot {
    /// Session + subagent spend as **one** dual-currency accumulator.
    ///
    /// The persisted USD and CNY columns are projections of per-turn
    /// [`crate::pricing::CostEstimate`]s that were accumulated jointly; every
    /// display total is derived from this single fold so the two currencies
    /// cannot be re-summed by separate code paths that then drift (#4939).
    /// CNY is *not* an FX multiple of USD: a turn carries CNY only when its
    /// route published an authoritative CNY row (provider-published
    /// dual-currency pricing, e.g. DeepSeek's CNY table), and a USD-only turn
    /// contributes exactly zero CNY while `cny_unpriced_turns` records the gap.
    #[must_use]
    pub fn total_estimate(&self) -> crate::pricing::CostEstimate {
        crate::pricing::CostEstimate {
            usd: self.session_cost_usd,
            cny: self.session_cost_cny,
        }
        .saturating_add(crate::pricing::CostEstimate {
            usd: self.subagent_cost_usd,
            cny: self.subagent_cost_cny,
        })
    }

    /// Session + subagent cost in USD.
    pub fn total_usd(&self) -> f64 {
        self.total_estimate()
            .amount(crate::pricing::CostCurrency::Usd)
    }

    /// Session + subagent cost in CNY.
    pub fn total_cny(&self) -> f64 {
        self.total_estimate()
            .amount(crate::pricing::CostCurrency::Cny)
    }

    /// Whether this snapshot's coverage state must be shown as unknown.
    ///
    /// True when the snapshot has no coverage evidence — the signature of a
    /// session written before coverage was persisted. Reporting any such
    /// session as "0 of 0 priced" would claim completeness without evidence,
    /// including when the saved amount is zero.
    #[must_use]
    pub fn coverage_is_legacy_unknown(&self) -> bool {
        !self.coverage_recorded
    }
}

impl SessionMetadata {
    /// Copy cost fields from another metadata (used when forking a session).
    #[allow(dead_code)]
    pub fn copy_cost_from(&mut self, other: &SessionMetadata) {
        self.cost = other.cost.clone();
    }

    /// Record additive lineage metadata for a forked saved session.
    pub fn mark_forked_from(&mut self, parent: &SessionMetadata) {
        self.parent_session_id = Some(parent.id.clone());
        self.forked_from_message_count = Some(parent.message_count);
    }
}

/// Durable Work-panel state. Optional on [`SavedSession`] so every session
/// written before v0.8.68 remains loadable without migration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionWorkState {
    /// Authoritative Work Graph. Optional so pre-Work-Graph sessions and old
    /// binaries continue to exchange fully populated Plan/To-do views.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<crate::work_graph::WorkGraphSnapshot>,
    #[serde(default, skip_serializing_if = "TodoListSnapshot::is_empty")]
    pub todos: TodoListSnapshot,
    #[serde(default, skip_serializing_if = "PlanSnapshot::is_empty")]
    pub plan: PlanSnapshot,
}

/// Bounded goal projection persisted beside the owning saved session.
///
/// This intentionally excludes completion prose, verifier output, transcripts,
/// and filesystem evidence. The saved session already owns conversation
/// history; restart only needs the typed control state that makes the next turn
/// continue the same objective without trusting text reconstructed from it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionGoalState {
    #[serde(default = "current_session_goal_schema_version")]
    pub schema_version: u32,
    pub objective: String,
    pub status: SessionGoalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u32>,
    #[serde(default)]
    pub tokens_used: u64,
    #[serde(default)]
    pub time_used_seconds: u64,
    #[serde(default)]
    pub continuation_count: u32,
    #[serde(default)]
    pub elapsed_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_reason: Option<GoalPauseReason>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionGoalStatus {
    Active,
    Paused,
    Complete,
    Blocked,
}

const fn current_session_goal_schema_version() -> u32 {
    CURRENT_SESSION_GOAL_SCHEMA_VERSION
}

impl SessionGoalState {
    /// Convert a runtime update into the durable, bounded session contract.
    /// The canonical empty runtime snapshot removes the sidecar.
    pub fn from_runtime(snapshot: &GoalSnapshot) -> io::Result<Option<Self>> {
        if snapshot.objective.is_none() && snapshot.status.trim() == "none" {
            return Ok(None);
        }
        let objective = snapshot
            .objective
            .as_deref()
            .map(str::trim)
            .filter(|objective| !objective.is_empty())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "goal snapshot has no objective")
            })?;
        let status = match snapshot.status.trim() {
            "active" => SessionGoalStatus::Active,
            "paused" => SessionGoalStatus::Paused,
            "complete" => SessionGoalStatus::Complete,
            "blocked" => SessionGoalStatus::Blocked,
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("goal snapshot has unsupported status '{other}'"),
                ));
            }
        };
        let state = Self {
            schema_version: CURRENT_SESSION_GOAL_SCHEMA_VERSION,
            objective: objective.to_string(),
            status,
            token_budget: snapshot.token_budget,
            tokens_used: snapshot.tokens_used,
            time_used_seconds: snapshot.time_used_seconds,
            continuation_count: snapshot.continuation_count,
            elapsed_seconds: snapshot.elapsed_seconds.unwrap_or_default(),
            pause_reason: snapshot.pause_reason,
        };
        state.validate()?;
        Ok(Some(state))
    }

    pub fn validate(&self) -> io::Result<()> {
        if self.schema_version > CURRENT_SESSION_GOAL_SCHEMA_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Session goal schema v{} is newer than supported v{}",
                    self.schema_version, CURRENT_SESSION_GOAL_SCHEMA_VERSION
                ),
            ));
        }
        let objective = self.objective.trim();
        if objective.is_empty() || objective.chars().count() > MAX_SESSION_GOAL_OBJECTIVE_CHARS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Session goal objective must contain 1..={MAX_SESSION_GOAL_OBJECTIVE_CHARS} characters"
                ),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn to_runtime_snapshot(&self) -> GoalSnapshot {
        GoalSnapshot {
            objective: Some(self.objective.clone()),
            status: match self.status {
                SessionGoalStatus::Active => "active",
                SessionGoalStatus::Paused => "paused",
                SessionGoalStatus::Complete => "complete",
                SessionGoalStatus::Blocked => "blocked",
            }
            .to_string(),
            token_budget: self.token_budget,
            tokens_used: self.tokens_used,
            time_used_seconds: self.time_used_seconds,
            continuation_count: self.continuation_count,
            elapsed_seconds: Some(self.elapsed_seconds),
            evidence: None,
            blocker: None,
            pause_reason: self.pause_reason,
            completion_verification: None,
            advisories: Vec::new(),
            last_gap_fingerprint: None,
            repeated_gap_count: 0,
        }
    }
}

impl SessionWorkState {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.graph
            .as_ref()
            .is_none_or(crate::work_graph::WorkGraphSnapshot::is_empty)
            && self.todos.is_empty()
            && self.plan.is_empty()
    }
}

/// Latest concrete Auto route and the decision receipt that produced it.
///
/// This is additive, optional session metadata: sessions written before
/// v0.9.1 deserialize with no receipt and keep their legacy restore behavior.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SavedAutoRouteReceipt {
    pub(crate) provider: ApiProvider,
    pub(crate) provider_identity: String,
    pub(crate) model: String,
    pub(crate) receipt: AutoRouteReceipt,
    /// Canonical effective reasoning receipt for the selected route, including
    /// routes where a concrete tier cannot be proven. Optional so older
    /// sessions remain loadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) effective_reasoning_effort: Option<ReasoningEffortTier>,
}

/// A saved session containing full conversation history
/// Starting with v0.9.5 (#5262) the canonical history is the append-only entry journal (`journal` / `leaf_id`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSession {
    /// Schema version for migration compatibility
    #[serde(default = "default_session_schema_version")]
    pub schema_version: u32,
    /// Session metadata
    pub metadata: SessionMetadata,
    /// Conversation messages — derived from the journal's active branch (kept for compat).
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal: Option<SessionJournal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaf_id: Option<String>,
    /// System prompt if any
    pub system_prompt: Option<String>,
    /// Compact linked context references for user-visible `@path` and
    /// `/attach` mentions. Optional for backward-compatible session loads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_references: Vec<SessionContextReference>,
    /// Metadata registry of large outputs produced during this session.
    /// Artifact contents are stored in the session-owned artifact directory.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactRecord>,
    /// Session-owned approval evidence. The append-only sidecar is canonical
    /// during a live turn; this projection makes saved snapshots self-
    /// describing without putting receipts in the model transcript.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) approval_receipts: Vec<ApprovalReceipt>,
    /// To-do and plan state shown in the Work sidebar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_state: Option<SessionWorkState>,
    /// User-configured tab/window title for this session (`/title`), shown as
    /// `[title] …` in front of the terminal window title. Optional for
    /// backward-compatible session loads; absent sessions use the `title`
    /// config default instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    /// Most recent accepted/completed Auto decision, when the saved model mode
    /// is `auto`. Optional for backward-compatible session loads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_auto_route: Option<SavedAutoRouteReceipt>,
}
impl SavedSession {
    /// Drop the journal-derived compatibility projection before an async
    /// persistence request takes ownership. Disk serialization restores it.
    pub(crate) fn compact_for_persistence_queue(&mut self) {
        if self.journal.is_some() {
            self.messages = Vec::new();
        }
    }

    fn storage_compatible_copy(&self) -> Option<Self> {
        let journal = self.journal.as_ref()?;
        let active_messages = journal.to_messages();
        if !self.messages.is_empty() && self.messages == active_messages {
            return None;
        }
        let mut copy = self.clone();
        if copy.messages.is_empty() {
            copy.messages = active_messages;
        } else if let Some(journal) = copy.journal.as_mut() {
            journal.rebranch_active_messages(&copy.messages);
            copy.leaf_id = journal.leaf_id.clone();
        }
        copy.metadata.message_count = copy.messages.len();
        Some(copy)
    }

    pub fn ensure_journal(&mut self) {
        if self.journal.is_some() {
            if self.leaf_id.is_none() {
                self.leaf_id = self.journal.as_ref().and_then(|j| j.leaf_id.clone());
            }
            let active = self
                .journal
                .as_ref()
                .map(|j| j.to_messages())
                .unwrap_or_default();
            if !active.is_empty() {
                self.messages = active;
                self.metadata.message_count = self.messages.len();
            }
            return;
        }
        let journal =
            SessionJournal::from_messages(self.messages.clone(), self.metadata.spawn_depth);
        self.leaf_id = journal.leaf_id.clone();
        self.journal = Some(journal);
    }
    pub fn journal_append_message(&mut self, message: Message) -> String {
        self.ensure_journal();
        let journal = self.journal.as_mut().expect("journal ensured");
        let id = journal.append_message(message.clone());
        self.leaf_id = journal.leaf_id.clone();
        self.messages = journal.to_messages();
        self.metadata.message_count = self.messages.len();
        self.metadata.updated_at = Utc::now();
        id
    }
    pub fn journal_branch_to(&mut self, entry_id: &str) -> Result<(), String> {
        self.ensure_journal();
        let journal = self.journal.as_mut().expect("journal ensured");
        journal.branch_to(entry_id)?;
        self.leaf_id = journal.leaf_id.clone();
        self.messages = journal.to_messages();
        self.metadata.updated_at = Utc::now();
        Ok(())
    }
    pub fn active_entries(&self) -> Vec<SessionEntry> {
        self.journal
            .as_ref()
            .map(|j| j.root_to_leaf().into_iter().cloned().collect())
            .unwrap_or_default()
    }
    pub fn export_container(&self, source: &str) -> SessionImportContainer {
        let journal = self.journal.clone().unwrap_or_else(|| {
            SessionJournal::from_messages(self.messages.clone(), self.metadata.spawn_depth)
        });
        SessionImportContainer::new(
            source.to_string(),
            &journal,
            serde_json::to_value(&self.metadata).ok(),
        )
    }
    pub fn import_foreign(
        container: SessionImportContainer,
        workspace: PathBuf,
        model: String,
    ) -> Result<Self, String> {
        let journal = container.into_journal()?;
        let leaf_id = journal.leaf_id.clone();
        let messages = journal.to_messages();
        let now = Utc::now();
        let spawn_depth = journal.spawn_depth.saturating_add(1);
        let title = messages
            .iter()
            .find(|m| m.role == "user")
            .and_then(|m| {
                m.content.iter().find_map(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
            })
            .map(|s| crate::session_manager::truncate_title(s, 50))
            .unwrap_or_else(|| crate::session_manager::DEFAULT_SESSION_TITLE.to_string());
        let metadata = SessionMetadata {
            id: Uuid::new_v4().to_string(),
            title,
            created_at: now,
            updated_at: now,
            message_count: messages.len(),
            total_tokens: 0,
            model,
            model_provider: default_model_provider(),
            model_provider_id: None,
            workspace,
            mode: None,
            cost: SessionCostSnapshot::default(),
            parent_session_id: None,
            forked_from_message_count: None,
            cumulative_turn_secs: 0,
            archived: false,
            spawn_depth,
        };
        let mut journal = journal;
        journal.spawn_depth = spawn_depth;
        Ok(Self {
            schema_version: CURRENT_SESSION_SCHEMA_VERSION,
            metadata,
            messages,
            journal: Some(journal),
            leaf_id,
            system_prompt: None,
            context_references: Vec::new(),
            artifacts: Vec::new(),
            approval_receipts: Vec::new(),
            work_state: None,
            window_title: None,
            last_auto_route: None,
        })
    }
}

fn serialize_saved_session(session: &SavedSession) -> io::Result<String> {
    let compatible = session.storage_compatible_copy();
    serde_json::to_string_pretty(compatible.as_ref().unwrap_or(session))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Manager for session persistence operations
#[derive(Debug)]
pub struct SessionManager {
    /// Directory where sessions are stored
    sessions_dir: PathBuf,
}

/// Origin of a crash-recovery checkpoint file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointSource {
    /// Per-session checkpoint file `checkpoints/<session_id>.json`.
    Session(String),
    /// Legacy single-slot checkpoint file `checkpoints/latest.json`.
    Legacy,
}

/// A crash-recovery checkpoint file discovered on disk (metadata only —
/// callers load the session content separately).
#[derive(Debug, Clone)]
pub struct CheckpointRef {
    pub source: CheckpointSource,
    pub path: PathBuf,
    pub modified: std::time::SystemTime,
}

/// File names in `checkpoints/` that are never per-session checkpoints.
const LEGACY_CHECKPOINT_FILE: &str = "latest.json";
const OFFLINE_QUEUE_FILE: &str = "offline_queue.json";

impl SessionManager {
    fn approval_receipt_store(&self) -> ApprovalReceiptStore {
        ApprovalReceiptStore::new(self.sessions_dir.clone())
    }

    fn hydrate_approval_receipts(&self, session: &mut SavedSession) -> io::Result<()> {
        let durable = self.approval_receipt_store().load(&session.metadata.id)?;
        if !durable.is_empty() {
            session.approval_receipts = durable;
        }
        ApprovalReplay::from_receipts(&session.approval_receipts)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        Ok(())
    }

    /// Reconstruct completed approvals and interrupted unmatched asks for one
    /// session without consulting the model transcript.
    pub(crate) fn replay_approvals(&self, session_id: &str) -> io::Result<ApprovalReplay> {
        self.approval_receipt_store().replay(session_id)
    }

    fn validated_session_id<'a>(&self, id: &'a str) -> std::io::Result<&'a str> {
        let trimmed = id.trim();
        if trimmed.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Session id cannot be empty",
            ));
        }
        if !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Invalid session id '{id}'"),
            ));
        }
        if trimmed == SESSION_BOOT_OWNERS_STEM {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Session id '{trimmed}' collides with a reserved sessions file"),
            ));
        }
        Ok(trimmed)
    }

    fn validated_session_path(&self, id: &str) -> std::io::Result<PathBuf> {
        let trimmed = self.validated_session_id(id)?;
        Ok(self.sessions_dir.join(format!("{trimmed}.json")))
    }

    fn checkpoints_dir(&self) -> PathBuf {
        self.sessions_dir.join("checkpoints")
    }

    fn session_goals_dir(&self) -> PathBuf {
        self.sessions_dir.join(SESSION_GOALS_DIR)
    }

    fn checked_existing_session_goals_dir(&self) -> std::io::Result<Option<PathBuf>> {
        let dir = self.session_goals_dir();
        let metadata = match fs::symlink_metadata(&dir) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Session goal store {} must be a real directory",
                    dir.display()
                ),
            ));
        }
        Ok(Some(dir))
    }

    fn ensure_session_goals_dir(&self) -> std::io::Result<PathBuf> {
        if let Some(dir) = self.checked_existing_session_goals_dir()? {
            return Ok(dir);
        }
        let dir = self.session_goals_dir();
        match fs::create_dir(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        self.checked_existing_session_goals_dir()?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("Session goal store {} was not created", dir.display()),
            )
        })
    }

    fn validated_session_goal_path(&self, session_id: &str) -> std::io::Result<PathBuf> {
        let id = self.validated_session_id(session_id)?;
        Ok(self.session_goals_dir().join(format!("{id}.json")))
    }

    fn checked_existing_session_goal_file(path: &Path) -> std::io::Result<bool> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Session goal {} must be a regular file", path.display()),
            ));
        }
        Ok(true)
    }

    fn validated_checkpoint_path(&self, session_id: &str) -> std::io::Result<PathBuf> {
        let trimmed = self.validated_session_id(session_id)?;
        // Reserved file names inside `checkpoints/` must never collide with a
        // per-session checkpoint file.
        if format!("{trimmed}.json") == LEGACY_CHECKPOINT_FILE
            || format!("{trimmed}.json") == OFFLINE_QUEUE_FILE
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Session id '{trimmed}' collides with a reserved checkpoint file"),
            ));
        }
        Ok(self.checkpoints_dir().join(format!("{trimmed}.json")))
    }

    /// Create a new `SessionManager` with the specified sessions directory
    pub fn new(sessions_dir: PathBuf) -> std::io::Result<Self> {
        let sessions_dir = normalize_managed_dir(sessions_dir)?;
        // Ensure the sessions directory exists
        fs::create_dir_all(&sessions_dir)?;
        Ok(Self { sessions_dir })
    }

    /// Create a `SessionManager` using the default location.
    pub fn default_location() -> std::io::Result<Self> {
        Self::new(default_sessions_dir()?)
    }

    /// Return the resolved sessions directory path.
    pub fn sessions_dir(&self) -> &Path {
        &self.sessions_dir
    }

    /// Persist the bounded goal control state for one saved session.
    /// `None` is the canonical clear operation and is idempotent.
    pub fn save_session_goal(
        &self,
        session_id: &str,
        goal: Option<&SessionGoalState>,
    ) -> std::io::Result<()> {
        let path = self.validated_session_goal_path(session_id)?;
        let Some(goal) = goal else {
            if self.checked_existing_session_goals_dir()?.is_some() && path.exists() {
                fs::remove_file(path)?;
            }
            return Ok(());
        };
        goal.validate()?;
        self.ensure_session_goals_dir()?;
        Self::checked_existing_session_goal_file(&path)?;
        let content = serde_json::to_string_pretty(goal)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        write_atomic(&path, content.as_bytes())
    }

    /// Load a saved session's durable goal, rejecting malformed or future
    /// records instead of silently starting a different objective.
    pub fn load_session_goal(&self, session_id: &str) -> std::io::Result<Option<SessionGoalState>> {
        let path = self.validated_session_goal_path(session_id)?;
        if self.checked_existing_session_goals_dir()?.is_none()
            || !Self::checked_existing_session_goal_file(&path)?
        {
            return Ok(None);
        }
        let file_len = fs::metadata(&path)?.len();
        if file_len > MAX_SESSION_GOAL_FILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Session goal {} is {file_len} bytes; maximum is {MAX_SESSION_GOAL_FILE_BYTES}",
                    path.display()
                ),
            ));
        }
        let raw = fs::read_to_string(path)?;
        let goal: SessionGoalState = serde_json::from_str(&raw)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        goal.validate()?;
        Ok(Some(goal))
    }

    /// Save a session to disk using atomic write (temp file + fsync + rename).
    pub fn save_session(&self, session: &SavedSession) -> std::io::Result<PathBuf> {
        let path = self.validated_session_path(&session.metadata.id)?;
        let already_persisted = path.exists()
            || self
                .validated_checkpoint_path(&session.metadata.id)
                .is_ok_and(|checkpoint| checkpoint.exists());

        self.archive_before_first_graph_write(session, &path)?;

        let mut durable_session = session.clone();
        self.hydrate_approval_receipts(&mut durable_session)?;
        let content = serialize_saved_session(&durable_session)?;

        // Atomic write via write_atomic (NamedTempFile + fsync + persist)
        write_atomic(&path, content.as_bytes())?;
        self.stamp_session_boot_owner_for_new_record(&session.metadata.id, already_persisted);

        // Clean up old sessions if we have too many
        self.cleanup_old_sessions()?;

        Ok(path)
    }

    /// Save a crash-recovery checkpoint for in-flight turns.
    ///
    /// Checkpoints are keyed per session (`checkpoints/<session_id>.json`) so
    /// concurrent sessions never overwrite each other's crash-recovery state.
    pub fn save_checkpoint(&self, session: &SavedSession) -> std::io::Result<PathBuf> {
        let path = self.validated_checkpoint_path(&session.metadata.id)?;
        let session_path = self.validated_session_path(&session.metadata.id)?;
        self.archive_before_first_graph_write(session, &session_path)?;
        fs::create_dir_all(self.checkpoints_dir())?;
        let already_persisted = path.exists() || session_path.exists();
        let mut durable_session = session.clone();
        self.hydrate_approval_receipts(&mut durable_session)?;
        let content = serialize_saved_session(&durable_session)?;
        write_atomic(&path, content.as_bytes())?;
        self.stamp_session_boot_owner_for_new_record(&session.metadata.id, already_persisted);
        Ok(path)
    }

    fn session_boot_owners_path(&self) -> PathBuf {
        self.sessions_dir
            .join(format!("{SESSION_BOOT_OWNERS_STEM}.json"))
    }

    fn load_session_boot_owners(&self) -> BTreeMap<String, String> {
        fs::read_to_string(self.session_boot_owners_path())
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default()
    }

    /// Does any durable record (session file or crash checkpoint) exist for
    /// this session id?
    fn session_record_exists(&self, session_id: &str) -> bool {
        self.validated_session_path(session_id)
            .is_ok_and(|path| path.exists())
            || self
                .validated_checkpoint_path(session_id)
                .is_ok_and(|path| path.exists())
    }

    /// Record which session instance owns `session_id`'s persisted record.
    ///
    /// Entries whose durable record no longer exists are pruned on the same
    /// write, so the sidecar cannot grow without bound.
    pub(crate) fn record_session_boot_owner(
        &self,
        session_id: &str,
        boot_id: &str,
    ) -> std::io::Result<()> {
        let id = self.validated_session_id(session_id)?.to_string();
        let mut owners = self.load_session_boot_owners();
        owners.retain(|owned, _| owned == &id || self.session_record_exists(owned));
        owners.insert(id, boot_id.to_string());
        let content = serde_json::to_string_pretty(&owners)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        write_atomic(&self.session_boot_owners_path(), content.as_bytes())
    }

    /// The session-instance boot id stamped on this session's persisted
    /// record, when one was recorded.
    #[must_use]
    pub fn session_boot_owner(&self, session_id: &str) -> Option<String> {
        let id = self.validated_session_id(session_id).ok()?;
        self.load_session_boot_owners().get(id).cloned()
    }

    /// Was this session's persisted record created by a different session
    /// instance (an earlier or sibling Codewhale process)?
    ///
    /// Mirrors `SubAgentManager::is_from_prior_session` (#405): a durable
    /// record with no stamped owner predates the marker and is classified as
    /// prior-instance work, while an id with no durable record at all is
    /// this instance's own not-yet-persisted session.
    #[must_use]
    pub fn session_from_prior_instance(&self, session_id: &str) -> bool {
        match self.session_boot_owner(session_id) {
            Some(owner) => owner != current_session_boot_id(),
            None => self.session_record_exists(session_id),
        }
    }

    /// Stamp this instance as creator when a save writes the first durable
    /// record for `session_id`. A record that already existed keeps its
    /// original owner: re-serializing another instance's work (crash
    /// recovery, external mutation) must not re-badge it as ours.
    fn stamp_session_boot_owner_for_new_record(&self, session_id: &str, already_persisted: bool) {
        if already_persisted || self.session_boot_owner(session_id).is_some() {
            return;
        }
        if let Err(error) = self.record_session_boot_owner(session_id, current_session_boot_id()) {
            tracing::warn!(session_id, %error, "could not stamp session boot owner");
        }
    }

    fn clear_session_boot_owner(&self, session_id: &str) {
        let Ok(id) = self.validated_session_id(session_id) else {
            return;
        };
        let mut owners = self.load_session_boot_owners();
        if owners.remove(id).is_none() {
            return;
        }
        if let Ok(content) = serde_json::to_string_pretty(&owners) {
            let _ = write_atomic(&self.session_boot_owners_path(), content.as_bytes());
        }
    }

    /// Preserve the exact pre-import session once, before the first graph-
    /// bearing session or checkpoint write can replace it.
    fn archive_before_first_graph_write(
        &self,
        session: &SavedSession,
        source: &Path,
    ) -> std::io::Result<()> {
        let writes_graph = session
            .work_state
            .as_ref()
            .and_then(|state| state.graph.as_ref())
            .is_some_and(|graph| !graph.is_empty());
        if !writes_graph || !source.exists() {
            return Ok(());
        }
        let bytes = fs::read(source)?;
        let already_graph_backed = serde_json::from_slice::<SavedSession>(&bytes)
            .ok()
            .and_then(|saved| saved.work_state)
            .and_then(|state| state.graph)
            .is_some_and(|graph| !graph.is_empty());
        if already_graph_backed {
            return Ok(());
        }
        let archive_dir = self.sessions_dir.join(WORK_GRAPH_IMPORT_ARCHIVE_DIR);
        fs::create_dir_all(&archive_dir)?;
        let archive =
            archive_dir.join(source.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid session path")
            })?);
        if !archive.exists() {
            write_atomic(&archive, &bytes)?;
        }
        Ok(())
    }

    fn read_checkpoint_file(&self, path: &Path) -> std::io::Result<Option<SavedSession>> {
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(path)?;
        let mut session: SavedSession = serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if session.schema_version > CURRENT_SESSION_SCHEMA_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Checkpoint schema v{} is newer than supported v{}",
                    session.schema_version, CURRENT_SESSION_SCHEMA_VERSION
                ),
            ));
        }
        session.system_prompt = strip_legacy_truncation_note(session.system_prompt);
        self.hydrate_approval_receipts(&mut session)?;
        Ok(Some(session))
    }

    /// Load a specific session's crash-recovery checkpoint if present.
    pub fn load_session_checkpoint(
        &self,
        session_id: &str,
    ) -> std::io::Result<Option<SavedSession>> {
        let path = self.validated_checkpoint_path(session_id)?;
        self.read_checkpoint_file(&path)
    }

    /// Load the legacy single-slot checkpoint (`checkpoints/latest.json`) if
    /// present. Compatibility read only — this release no longer writes it.
    pub fn load_legacy_checkpoint(&self) -> std::io::Result<Option<SavedSession>> {
        let path = self.checkpoints_dir().join(LEGACY_CHECKPOINT_FILE);
        self.read_checkpoint_file(&path)
    }

    /// Clear one session's crash-recovery checkpoint. Scoped: this can never
    /// remove another session's checkpoint file or the legacy slot.
    pub fn clear_session_checkpoint(&self, session_id: &str) -> std::io::Result<()> {
        let path = self.validated_checkpoint_path(session_id)?;
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Remove the legacy single-slot checkpoint file.
    pub fn clear_legacy_checkpoint(&self) -> std::io::Result<()> {
        let path = self.checkpoints_dir().join(LEGACY_CHECKPOINT_FILE);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Enumerate all crash-recovery checkpoint files (per-session files plus
    /// the legacy single slot), sorted most recently modified first. Only
    /// file metadata is read here; callers load content per candidate.
    pub fn list_checkpoints(&self) -> std::io::Result<Vec<CheckpointRef>> {
        let dir = self.checkpoints_dir();
        let mut refs = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(refs),
            Err(err) => return Err(err),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() || path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let source = if name == LEGACY_CHECKPOINT_FILE {
                CheckpointSource::Legacy
            } else if name == OFFLINE_QUEUE_FILE {
                continue;
            } else {
                let session_id = name.trim_end_matches(".json").to_string();
                if self.validated_checkpoint_path(&session_id).is_err() {
                    continue;
                }
                CheckpointSource::Session(session_id)
            };
            let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            refs.push(CheckpointRef {
                source,
                path,
                modified,
            });
        }
        refs.sort_by_key(|r| std::cmp::Reverse(r.modified));
        Ok(refs)
    }

    /// Migrate a session recovered from the legacy single-slot checkpoint to
    /// a per-session checkpoint file. Never overwrites an existing
    /// per-session file and leaves the legacy file in place (older binaries
    /// still read it; the legacy writer is already gone). Returns whether a
    /// file was written.
    pub fn write_session_checkpoint_if_absent(
        &self,
        session: &SavedSession,
    ) -> std::io::Result<bool> {
        let path = self.validated_checkpoint_path(&session.metadata.id)?;
        if path.exists() {
            return Ok(false);
        }
        self.save_checkpoint(session)?;
        Ok(true)
    }

    /// Save offline queue state (queued + draft messages).
    pub fn save_offline_queue_state(
        &self,
        state: &OfflineQueueState,
        session_id: Option<&str>,
    ) -> std::io::Result<PathBuf> {
        let checkpoints = self.sessions_dir.join("checkpoints");
        fs::create_dir_all(&checkpoints)?;
        let path = checkpoints.join("offline_queue.json");
        let mut state_with_id = state.clone();
        state_with_id.session_id = session_id.map(|s| s.to_string());
        let content = serde_json::to_string_pretty(&state_with_id)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        write_atomic(&path, content.as_bytes())?;
        Ok(path)
    }

    /// Load offline queue state if present.
    pub fn load_offline_queue_state(&self) -> std::io::Result<Option<OfflineQueueState>> {
        let path = self
            .sessions_dir
            .join("checkpoints")
            .join("offline_queue.json");
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path)?;
        let state: OfflineQueueState = serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if state.schema_version > CURRENT_QUEUE_SCHEMA_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Offline queue schema v{} is newer than supported v{}",
                    state.schema_version, CURRENT_QUEUE_SCHEMA_VERSION
                ),
            ));
        }
        Ok(Some(state))
    }

    /// Remove persisted offline queue state.
    pub fn clear_offline_queue_state(&self) -> std::io::Result<()> {
        let path = self
            .sessions_dir
            .join("checkpoints")
            .join("offline_queue.json");
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Read a session snapshot without repairing tool call/result pairs.
    ///
    /// This is the correct API for embedding hosts that inspect or update a
    /// durable session while an engine may still be executing a tool call.
    /// A dangling `tool_use` is not proof of a crashed process in that state.
    pub fn load_session_snapshot(&self, id: &str) -> std::io::Result<SavedSession> {
        let path = self.validated_session_path(id)?;

        let content = fs::read_to_string(&path)?;
        let mut session: SavedSession = serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if session.schema_version > CURRENT_SESSION_SCHEMA_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Session schema v{} is newer than supported v{}",
                    session.schema_version, CURRENT_SESSION_SCHEMA_VERSION
                ),
            ));
        }

        session.system_prompt = strip_legacy_truncation_note(session.system_prompt);
        session.ensure_journal();
        self.hydrate_approval_receipts(&mut session)?;

        Ok(session)
    }

    /// Load and repair a session after a known process or engine restart.
    ///
    /// The returned repair remains in memory until the caller persists
    /// `recovery.session`. Keeping persistence explicit lets embedding hosts
    /// serialize recovery with their own transcript mutation lock.
    pub fn recover_session_for_resume(&self, id: &str) -> std::io::Result<SessionRecovery> {
        let mut session = self.load_session_snapshot(id)?;

        let repair = crate::tool_history_repair::repair_tool_call_pairs(&mut session.messages);
        let changed = !repair.is_empty();
        if changed {
            if let Some(journal) = session.journal.as_mut() {
                journal.rebranch_active_messages(&session.messages);
                session.leaf_id = journal.leaf_id.clone();
            }
            session.metadata.message_count = session.messages.len();
            tracing::warn!(
                session_id = %session.metadata.id,
                repaired_call_ids = ?repair.repaired_call_ids,
                duplicate_result_ids = ?repair.duplicate_result_ids,
                orphan_result_ids = ?repair.orphan_result_ids,
                "repaired persisted tool call/result history"
            );
        }

        Ok(SessionRecovery {
            session,
            changed,
            repaired_call_count: repair.repaired_call_ids.len(),
            duplicate_result_count: repair.duplicate_result_ids.len(),
            orphan_result_count: repair.orphan_result_ids.len(),
        })
    }

    /// Load a session by ID for the standalone CodeWhale resume flow.
    ///
    /// This preserves the historical recovery behavior for existing callers.
    /// Embedding hosts performing ordinary runtime reads should use
    /// [`Self::load_session_snapshot`] instead.
    pub fn load_session(&self, id: &str) -> std::io::Result<SavedSession> {
        self.recover_session_for_resume(id)
            .map(|recovery| recovery.session)
    }

    /// Load a session by partial ID prefix
    pub fn load_session_by_prefix(&self, prefix: &str) -> std::io::Result<SavedSession> {
        let sessions = self.list_sessions()?;

        let matches: Vec<_> = sessions
            .into_iter()
            .filter(|s| s.id.starts_with(prefix))
            .collect();

        match matches.len() {
            0 => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("No session found with prefix: {prefix}"),
            )),
            1 => self.load_session(&matches[0].id),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "Ambiguous prefix '{}' matches {} sessions",
                    prefix,
                    matches.len()
                ),
            )),
        }
    }

    /// List all saved sessions, sorted by most recently updated
    pub fn list_sessions(&self) -> std::io::Result<Vec<SessionMetadata>> {
        let mut sessions = Vec::new();

        for entry in fs::read_dir(&self.sessions_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "json")
                && let Ok(session) = Self::load_session_metadata(&path)
            {
                sessions.push(session);
            }
        }

        // Sort by updated_at descending (most recent first)
        sessions.sort_by_key(|s| std::cmp::Reverse(s.updated_at));

        Ok(sessions)
    }

    /// Set the durable archive flag on a saved session and return the
    /// resulting metadata.
    ///
    /// This is the single writer for the flag: the picker, the `/sessions`
    /// command, and `PATCH /v1/sessions/{id}` all route through it so the TUI
    /// and the web dashboard cannot drift into two archive notions. A no-op
    /// call (already in the requested state) still returns the metadata and
    /// does not rewrite the file.
    pub fn set_session_archived(
        &self,
        id: &str,
        archived: bool,
        mutator: SessionMutator,
    ) -> std::io::Result<SessionMetadata> {
        if mutator == SessionMutator::External && is_live_session(id) {
            return Err(live_session_conflict(id));
        }
        let mut session = self.load_session(id)?;
        if session.metadata.archived == archived {
            return Ok(session.metadata);
        }
        session.metadata.archived = archived;
        self.save_session(&session)?;
        Ok(session.metadata)
    }

    /// Re-read the durable lifecycle fields for `metadata` from disk.
    ///
    /// This is the autosave-survival guard. A TUI autosave rebuilds the whole
    /// session document from in-memory `App` state; any lifecycle field it
    /// carries from a stale cache would silently revert a rename or archive
    /// that landed in between — including one applied by the picker earlier in
    /// the same event loop, or by `/rename` while a snapshot was already
    /// queued.
    ///
    /// So rather than trusting any cache, the writer re-reads the persisted
    /// values immediately before writing. `title`, `archived`, `created_at`,
    /// and fork lineage are *lifecycle* state owned by the file, not
    /// conversation state owned by the running turn. Reading them back costs
    /// one bounded metadata-prefix read.
    ///
    /// Returns `true` when an existing record was found and merged. A missing
    /// record is not an error: the first save of a new session has nothing to
    /// merge from.
    pub fn merge_persisted_lifecycle(&self, metadata: &mut SessionMetadata) -> bool {
        let Ok(path) = self.validated_session_path(&metadata.id) else {
            return false;
        };
        let Ok(persisted) = Self::load_session_metadata(&path) else {
            return false;
        };
        metadata.title = persisted.title;
        metadata.archived = persisted.archived;
        metadata.created_at = persisted.created_at;
        metadata.parent_session_id = persisted.parent_session_id;
        metadata.forked_from_message_count = persisted.forked_from_message_count;
        true
    }

    /// Rename a saved session and return the resulting metadata.
    ///
    /// Titles are trimmed and bounded to [`MAX_SESSION_TITLE_CHARS`]
    /// characters (counted in `char`s, not bytes, so a CJK or emoji title is
    /// not truncated mid-scalar). Created-at and fork lineage are untouched.
    pub fn rename_session(
        &self,
        id: &str,
        title: &str,
        mutator: SessionMutator,
    ) -> std::io::Result<SessionMetadata> {
        let title = normalize_session_title(title)?;
        if mutator == SessionMutator::External && is_live_session(id) {
            return Err(live_session_conflict(id));
        }
        let mut session = self.load_session(id)?;
        if session.metadata.title == title {
            return Ok(session.metadata);
        }
        session.metadata.title = title;
        self.save_session(&session)?;
        Ok(session.metadata)
    }

    /// Load only the metadata from a session file.
    ///
    /// Optimization for #337: previously this called
    /// `serde_json::from_reader` which forces serde to scan every token in
    /// the file just to validate JSON structure — including the
    /// (potentially many MB of) `messages` and `tool_log` arrays we're
    /// going to discard. For a user with hundreds of long sessions, a
    /// single `list_sessions()` call could chew through tens of MB of
    /// JSON per startup.
    ///
    /// We now read at most 64 KB up front and string-extract the
    /// top-level `metadata` object, which is invariably tiny (~500 B)
    /// and appears before any large `messages`/`tool_log` payload. We
    /// fall back to a full-file read only if the prefix doesn't yield a
    /// parseable metadata block (e.g. an oddly-formatted legacy file).
    fn load_session_metadata(path: &Path) -> std::io::Result<SessionMetadata> {
        use std::io::Read;

        const PREFIX_BYTES: usize = 64 * 1024;
        let mut file = fs::File::open(path)?;
        let mut buf = Vec::with_capacity(PREFIX_BYTES);
        file.by_ref()
            .take(PREFIX_BYTES as u64)
            .read_to_end(&mut buf)?;

        if let Some(metadata) = extract_top_level_metadata(&buf) {
            return Ok(metadata);
        }

        // Metadata wasn't extractable from the prefix (truncated mid-block,
        // unusual key ordering, etc.). Read the rest and try again with the
        // full buffer before giving up.
        let mut rest = Vec::new();
        file.read_to_end(&mut rest)?;
        buf.extend_from_slice(&rest);
        extract_top_level_metadata(&buf).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "session file missing parseable `metadata` block",
            )
        })
    }

    /// Delete a session by ID
    pub fn delete_session(&self, id: &str) -> std::io::Result<()> {
        let path = self.validated_session_path(id)?;
        self.save_session_goal(id, None)?;
        fs::remove_file(path)?;
        self.clear_session_boot_owner(id);
        let session_dir = self.sessions_dir.join(id.trim());
        if session_dir.exists() {
            fs::remove_dir_all(session_dir)?;
        }
        Ok(())
    }

    /// Ceiling on orphan directories reclaimed per `cleanup` call.
    ///
    /// Reconciliation runs on the save path, so it must never turn one save
    /// into a long stall. A real machine accumulated 780 orphans; at this rate
    /// it converges over a couple of dozen saves instead of blocking one.
    const MAX_ORPHAN_DIRS_PER_SWEEP: usize = 32;

    /// Remove per-session artifact directories whose session no longer exists.
    ///
    /// `delete_session` removes `sessions/<id>/` along with `<id>.json`, so
    /// nothing written by the current code leaks. What was missing is
    /// *reconciliation*: directories stranded by earlier versions — or by a
    /// `remove_dir_all` that failed while the `remove_file` before it
    /// succeeded, an error `cleanup_old_sessions_keeping` deliberately
    /// swallows — were never collected by anything. A real `~/.codewhale`
    /// held **780** such directories, each holding shell-completion evidence
    /// artifacts, which is also why traversing that tree had become slow.
    ///
    /// Deliberately conservative, because this removes directories under
    /// `$HOME`. A directory is reclaimed only when **all** of these hold:
    ///
    /// - its name is a valid session id by `validated_session_id` (so
    ///   `checkpoints/` and any other bookkeeping directory is excluded);
    /// - `sessions/<id>.json` does not exist;
    /// - `checkpoints/<id>.json` does not exist — a crashed session's evidence
    ///   must outlive its missing document, since that is exactly what
    ///   recovery reads;
    /// - the session is not live in *this* process (`is_live_session`).
    ///   That check is process-local; a second Codewhale sharing `$HOME`
    ///   is not visible here, so reclaim also requires the session
    ///   document and checkpoint to be gone.
    ///
    /// Best effort throughout: a failure to read the directory or remove an
    /// entry is ignored rather than failing the save that triggered it.
    fn reclaim_orphaned_session_dirs(&self) {
        let Ok(entries) = fs::read_dir(&self.sessions_dir) else {
            return;
        };
        let mut reclaimed = 0usize;
        for entry in entries.flatten() {
            if reclaimed >= Self::MAX_ORPHAN_DIRS_PER_SWEEP {
                return;
            }
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let name = entry.file_name();
            let Some(id) = name.to_str() else {
                continue;
            };
            // `validated_session_id` is far too permissive to gate a
            // `remove_dir_all` — it accepts any `[A-Za-z0-9_-]+`, which
            // includes `checkpoints` itself. Reclaiming that directory would
            // delete every crash-recovery checkpoint, and then, on the same
            // pass, every session directory those checkpoints were protecting.
            // Require the exact shape the runtime actually mints instead. An
            // id that is not a UUID simply keeps its directory: leaving a
            // stranger alone is the safe direction to be wrong in.
            if !is_session_uuid(id) {
                continue;
            }
            let Ok(session_path) = self.validated_session_path(id) else {
                continue;
            };
            if session_path.exists() || is_live_session(id) {
                continue;
            }
            if self
                .validated_checkpoint_path(id)
                .is_ok_and(|checkpoint| checkpoint.exists())
            {
                continue;
            }
            if fs::remove_dir_all(entry.path()).is_ok() {
                reclaimed += 1;
            }
        }
    }

    /// Clean up old sessions to stay within `MAX_SESSIONS` limit.
    pub fn cleanup_old_sessions(&self) -> std::io::Result<()> {
        self.cleanup_old_sessions_keeping(None)
    }

    /// As [`Self::cleanup_old_sessions`], but never deletes `keep` — the
    /// session being resumed at boot. Without this, a background cleanup that
    /// races session restore can prune the just-resumed session when 50+
    /// newer records exist (its `updated_at` is not bumped until first save).
    pub fn cleanup_old_sessions_keeping(&self, keep: Option<&str>) -> std::io::Result<()> {
        let sessions = self.list_sessions()?;

        if sessions.len() > MAX_SESSIONS {
            for session in sessions.iter().skip(MAX_SESSIONS) {
                if keep.is_some_and(|id| id == session.id) {
                    continue;
                }
                let _ = self.delete_session(&session.id);
            }
        }
        self.reclaim_orphaned_session_dirs();

        Ok(())
    }

    /// Remove session files whose `updated_at` is older than `max_age`
    /// from the persisted-sessions directory. Returns the number of
    /// records pruned. Building block for #406's phase-2 auto-archive
    /// on boot; today the user-facing entry point is the
    /// `/sessions prune <days>` slash command.
    ///
    /// Crash-recovery safety: skips the per-session checkpoint files
    /// (`checkpoints/<session_id>.json`), the legacy single-slot
    /// checkpoint (`checkpoints/latest.json`), and any file under `checkpoints/`
    /// — those are owned by the checkpoint subsystem and live with
    /// stricter durability rules. Only top-level `<session_id>.json`
    /// files are candidates.
    ///
    /// `max_age` is checked against the metadata's `updated_at`
    /// timestamp embedded in the JSON, not the filesystem mtime — the
    /// user may have rsynced their `~/.deepseek` between machines and
    /// fs mtimes can lie.
    pub fn prune_sessions_older_than(
        &self,
        max_age: std::time::Duration,
    ) -> std::io::Result<usize> {
        self.prune_sessions_older_than_keeping(max_age, None)
    }

    /// As [`Self::prune_sessions_older_than`], but never deletes `keep` — the
    /// active session. A just-resumed session's `updated_at` is stale until
    /// its first post-resume save, so an age prune could otherwise delete the
    /// live session out from under the TUI.
    pub fn prune_sessions_older_than_keeping(
        &self,
        max_age: std::time::Duration,
        keep: Option<&str>,
    ) -> std::io::Result<usize> {
        let cutoff = Utc::now()
            - chrono::Duration::from_std(max_age).unwrap_or(chrono::Duration::days(365 * 10));
        let sessions = self.list_sessions()?;
        let mut pruned = 0usize;
        for session in sessions {
            if keep.is_some_and(|id| id == session.id) {
                continue;
            }
            if session.updated_at < cutoff {
                if let Err(err) = self.delete_session(&session.id) {
                    tracing::warn!(
                        target: "session",
                        session = session.id,
                        ?err,
                        "session prune skipped a record",
                    );
                    continue;
                }
                pruned += 1;
            }
        }
        Ok(pruned)
    }

    /// Get the most recent session scoped to the current workspace.
    ///
    /// Archived sessions are skipped: archiving is the user saying "not this
    /// one", and `--continue` / auto-resume must honour that rather than
    /// dragging a put-away session back.
    pub fn get_latest_session_for_workspace(
        &self,
        workspace: &Path,
    ) -> std::io::Result<Option<SessionMetadata>> {
        let sessions = self.list_sessions()?;
        Ok(sessions.into_iter().find(|session| {
            !session.archived
                && workspace_scope_matches(&session.workspace, workspace)
                && !is_empty_auto_created_session(session)
        }))
    }

    /// Search sessions by title
    pub fn search_sessions(&self, query: &str) -> std::io::Result<Vec<SessionMetadata>> {
        let query_lower = query.to_lowercase();
        let sessions = self.list_sessions()?;

        Ok(sessions
            .into_iter()
            .filter(|s| s.title.to_lowercase().contains(&query_lower))
            .collect())
    }
}

/// Unicode format characters that never belong in a session title: bidi
/// embeddings/overrides/isolates and marks, zero-width joiners/spaces, the
/// soft hyphen, BOM, and line/paragraph separators. Together with
/// `char::is_control` (C0, DEL, C1 — so ESC, BEL, ST, and OSC introducers)
/// this is the one character policy for the persisted title, the terminal
/// tab title, and every plain-text listing that echoes a title.
pub(crate) fn is_title_format_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{00ad}'
            | '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
    )
}

/// Drop control and bidi/zero-width format characters from a title.
///
/// A session title is user- or content-derived text that later reaches an
/// OSC 0 terminal title, `codewhale sessions` stdout, and the picker, so the
/// persisted value must not be able to carry a raw escape sequence. Ordinary
/// text, punctuation, CJK, and emoji pass through untouched.
pub fn sanitize_session_title(raw: &str) -> String {
    raw.chars()
        .filter(|ch| !ch.is_control() && !is_title_format_char(*ch))
        .collect()
}

/// Sanitize, trim, and bound a user-supplied session title.
///
/// Returns `InvalidInput` for an empty title or one longer than
/// [`MAX_SESSION_TITLE_CHARS`] so every rename surface (picker, `/rename`,
/// `PATCH /v1/sessions/{id}`) rejects the same inputs with the same reason.
pub fn normalize_session_title(title: &str) -> std::io::Result<String> {
    let sanitized = sanitize_session_title(title);
    let trimmed = sanitized.trim();
    if trimmed.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Session title cannot be empty",
        ));
    }
    if trimmed.chars().count() > MAX_SESSION_TITLE_CHARS {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Session title cannot exceed {MAX_SESSION_TITLE_CHARS} characters"),
        ));
    }
    Ok(trimmed.to_string())
}

pub(crate) fn workspace_scope_matches(saved_workspace: &Path, current_workspace: &Path) -> bool {
    if paths_equivalent(saved_workspace, current_workspace) {
        return true;
    }

    // Repository identity comes from the containing checkout itself (Git
    // dir/worktree traversal shared with project-context scope resolution),
    // never from branch names or paths mentioned in conversation.
    let canonical = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match (
        find_git_root(&canonical(saved_workspace)),
        find_git_root(&canonical(current_workspace)),
    ) {
        (Some(saved_root), Some(current_root)) => paths_equivalent(&saved_root, &current_root),
        _ => false,
    }
}

pub(crate) fn is_empty_auto_created_session(session: &SessionMetadata) -> bool {
    session.message_count == 0
        && session
            .title
            .trim()
            .eq_ignore_ascii_case(DEFAULT_SESSION_TITLE)
}

fn paths_equivalent(lhs: &Path, rhs: &Path) -> bool {
    let lhs_canonical = fs::canonicalize(lhs).ok();
    let rhs_canonical = fs::canonicalize(rhs).ok();
    match (lhs_canonical, rhs_canonical) {
        (Some(lhs), Some(rhs)) => lhs == rhs,
        _ => lhs == rhs,
    }
}

/// Resolve the default session directory path.
///
/// v0.8.44: prefers `~/.codewhale/sessions`, falls back to
/// `~/.deepseek/sessions` for existing installs. Uses the write-path resolver
/// so the first access relocates any legacy `~/.deepseek/sessions` into
/// `~/.codewhale/sessions` when the primary directory is missing (#3240).
/// If an older build already created an empty primary sessions directory, copy
/// missing legacy entries into it without overwriting newer CodeWhale data.
pub fn default_sessions_dir() -> std::io::Result<PathBuf> {
    let dir = codewhale_config::ensure_state_dir("sessions")
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()))?;
    match merge_missing_legacy_session_entries(&dir) {
        Ok(0) => {}
        Ok(count) => {
            tracing::info!(
                target: "session::migration",
                "Copied {count} missing legacy session entries into {}",
                dir.display()
            );
        }
        Err(err) => {
            tracing::warn!(
                target: "session::migration",
                "Could not copy legacy sessions into {}: {err}",
                dir.display()
            );
        }
    }
    Ok(dir)
}

fn merge_missing_legacy_session_entries(primary: &Path) -> io::Result<usize> {
    if codewhale_paths::codewhale_home_is_explicit() {
        return Ok(0);
    }

    let legacy = codewhale_config::legacy_deepseek_home()
        .map_err(|e| io::Error::new(io::ErrorKind::NotFound, e.to_string()))?
        .join("sessions");
    if !legacy.is_dir() || paths_equivalent(primary, &legacy) {
        return Ok(0);
    }

    copy_missing_dir_entries(&legacy, primary)
}

fn copy_missing_dir_entries(src: &Path, dst: &Path) -> io::Result<usize> {
    fs::create_dir_all(dst)?;
    let mut copied = 0;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let source = entry.path();
        let target = dst.join(entry.file_name());

        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if entry.file_name() == std::ffi::OsStr::new("checkpoints") || target.exists() {
                continue;
            }
            copied += copy_missing_dir_entries(&source, &target)?;
        } else if file_type.is_file() {
            copied += usize::from(copy_file_create_new(&source, &target)?);
        }
    }
    Ok(copied)
}

fn copy_file_create_new(src: &Path, dst: &Path) -> io::Result<bool> {
    let mut source = fs::File::open(src)?;
    let mut target = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)
    {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(err) => return Err(err),
    };
    if let Err(err) = io::copy(&mut source, &mut target) {
        let _ = fs::remove_file(dst);
        return Err(err);
    }
    Ok(true)
}

/// Prune snapshots older than `max_age` for `workspace`.
///
/// Always non-fatal. Returns silently — callers don't need the count
/// (the underlying repo logs at WARN if anything blew up).
pub fn prune_workspace_snapshots(workspace: &Path, max_age: std::time::Duration) {
    match crate::snapshot::prune_older_than(workspace, max_age) {
        Ok(0) => {}
        Ok(n) => {
            tracing::debug!(target: "snapshot", "boot prune removed {n} snapshot(s)");
        }
        Err(e) => {
            tracing::warn!(target: "snapshot", "boot prune failed: {e}");
        }
    }
}

/// Create a new `SavedSession` from conversation state
pub fn create_saved_session(
    messages: &[Message],
    model: &str,
    workspace: &Path,
    total_tokens: u64,
    system_prompt: Option<&SystemPrompt>,
) -> SavedSession {
    create_saved_session_with_mode(
        messages,
        model,
        workspace,
        total_tokens,
        system_prompt,
        None,
    )
}

/// Placeholder title used for a session that has no first user message yet.
/// `build_session_snapshot` (tui/ui/frame.rs) treats a title equal to this
/// constant as an auto-generated placeholder and lets the conversation-derived
/// title win once a user message exists. Keep this string stable on purpose.
pub(crate) const DEFAULT_SESSION_TITLE: &str = "New Session";

/// Create a new `SavedSession` from conversation state with optional mode label
pub fn create_saved_session_with_mode(
    messages: &[Message],
    model: &str,
    workspace: &Path,
    total_tokens: u64,
    system_prompt: Option<&SystemPrompt>,
    mode: Option<&str>,
) -> SavedSession {
    create_saved_session_with_id_and_mode(
        Uuid::new_v4().to_string(),
        messages,
        model,
        workspace,
        total_tokens,
        system_prompt,
        mode,
    )
}

/// Create a new `SavedSession` using a caller-owned session id.
pub fn create_saved_session_with_id_and_mode(
    id: String,
    messages: &[Message],
    model: &str,
    workspace: &Path,
    total_tokens: u64,
    system_prompt: Option<&SystemPrompt>,
    mode: Option<&str>,
) -> SavedSession {
    let now = Utc::now();

    // Generate title from first user message
    let title = messages
        .iter()
        .find(|m| m.role == "user")
        .and_then(|m| {
            m.content.iter().find_map(|block| match block {
                ContentBlock::Text { text, .. } => {
                    let prompt = extract_user_prompt(text);
                    if prompt.is_empty() {
                        None
                    } else {
                        Some(truncate_title(prompt, 50))
                    }
                }
                _ => None,
            })
        })
        .unwrap_or_else(|| DEFAULT_SESSION_TITLE.to_string());

    let journal = SessionJournal::from_messages(messages.to_vec(), 0);
    let leaf_id = journal.leaf_id.clone();
    SavedSession {
        schema_version: CURRENT_SESSION_SCHEMA_VERSION,
        metadata: SessionMetadata {
            id,
            title,
            created_at: now,
            updated_at: now,
            message_count: messages.len(),
            total_tokens,
            model: model.to_string(),
            model_provider: default_model_provider(),
            model_provider_id: None,
            workspace: workspace.to_path_buf(),
            mode: mode.map(str::to_string),
            cost: SessionCostSnapshot::default(),
            parent_session_id: None,
            forked_from_message_count: None,
            cumulative_turn_secs: 0,
            archived: false,
            spawn_depth: 0,
        },
        messages: messages.to_vec(),
        journal: Some(journal),
        leaf_id,
        system_prompt: system_prompt_to_string(system_prompt),
        context_references: Vec::new(),
        artifacts: Vec::new(),
        approval_receipts: Vec::new(),
        work_state: None,
        window_title: None,
        last_auto_route: None,
    }
}

/// Update an existing session with new messages
pub fn update_session(
    mut session: SavedSession,
    messages: &[Message],
    total_tokens: u64,
    system_prompt: Option<&SystemPrompt>,
) -> SavedSession {
    session.schema_version = CURRENT_SESSION_SCHEMA_VERSION;
    session.ensure_journal();
    let old_len = session.messages.len();
    let new_len = messages.len();
    if new_len >= old_len && messages[..old_len] == session.messages[..] {
        if let Some(journal) = session.journal.as_mut() {
            for msg in &messages[old_len..] {
                journal.append_message(msg.clone());
            }
            session.leaf_id = journal.leaf_id.clone();
        }
    } else if (new_len != old_len || messages != session.messages.as_slice())
        && let Some(journal) = session.journal.as_mut()
    {
        let common = messages
            .iter()
            .zip(session.messages.iter())
            .take_while(|(a, b)| a == b)
            .count();
        if common > 0 && common <= journal.entries.len() {
            let target_id = journal
                .root_to_leaf()
                .get(common - 1)
                .map(|entry| entry.id.clone());
            if let Some(target_id) = target_id {
                let _ = journal.branch_to(&target_id);
            } else {
                journal.leaf_id = None;
            }
        } else if common == 0 {
            journal.leaf_id = journal.entries.first().and_then(|e| e.parent_id.clone());
            if journal.leaf_id.is_none() && !journal.entries.is_empty() {
                journal.leaf_id = None;
            }
        }
        for msg in messages.iter().skip(common) {
            journal.append_message(msg.clone());
        }
        session.leaf_id = journal.leaf_id.clone();
    }
    session.messages.clear();
    session.messages.extend_from_slice(messages);
    session.metadata.updated_at = Utc::now();
    session.metadata.message_count = messages.len();
    session.metadata.total_tokens = total_tokens;
    session.system_prompt = system_prompt_to_string(system_prompt);
    session
}

/// Strip a stale `[Session note]` block that was written by the old
/// 500-message cap. Only removes notes that contain the specific
/// "older messages were dropped" phrase — ordinary user-added
/// `[Session note]` prompts are left untouched.
fn strip_legacy_truncation_note(system_prompt: Option<String>) -> Option<String> {
    let sp = system_prompt?;
    let Some(trimmed) = sp.strip_prefix("[Session note]\n") else {
        return Some(sp);
    };
    // Only strip if this is the known cap_messages note.
    if !trimmed.contains("older messages were dropped") {
        return Some(sp);
    }
    // The note block ends with "\n\n---\n\n" (7 chars) followed by the real prompt.
    trimmed
        .find("\n\n---\n\n")
        .map(|pos| trimmed[pos + 7..].to_string())
}

/// String-scan a JSON byte buffer for the top-level `"metadata":{...}`
/// block and return it parsed. Returns `None` if no balanced metadata
/// object is present in the buffer.
///
/// Supports the optimisation in `SessionManager::load_session_metadata`
/// (#337). The scanner is brace-balanced and string-aware so a `{` or
/// `}` appearing inside a string literal doesn't perturb the depth
/// count.
fn extract_top_level_metadata(buf: &[u8]) -> Option<SessionMetadata> {
    let s = std::str::from_utf8(buf).ok()?;
    let bytes = s.as_bytes();

    // Find the FIRST `"metadata"` key that appears outside of any string
    // literal. Walking with brace/string awareness costs almost nothing
    // and avoids matching `metadata` inside an earlier message body.
    let key_pat = b"\"metadata\"";
    let mut idx = 0usize;
    let mut in_string = false;
    let mut escape = false;
    let key_offset = loop {
        if idx >= bytes.len() {
            return None;
        }
        let c = bytes[idx];
        if escape {
            escape = false;
            idx += 1;
            continue;
        }
        if c == b'\\' {
            escape = true;
            idx += 1;
            continue;
        }
        if c == b'"' {
            // If we're already in a string, this closes it; otherwise it
            // opens one. But before flipping we check for the key match
            // when we're entering a string at exactly this position.
            if !in_string && bytes[idx..].starts_with(key_pat) {
                break idx;
            }
            in_string = !in_string;
            idx += 1;
            continue;
        }
        idx += 1;
    };

    // Position past the key.
    let after_key = key_offset + key_pat.len();
    // Find the colon that separates key from value (skip whitespace).
    let mut after_colon = after_key;
    while after_colon < bytes.len() && (bytes[after_colon] as char).is_whitespace() {
        after_colon += 1;
    }
    if after_colon >= bytes.len() || bytes[after_colon] != b':' {
        return None;
    }
    after_colon += 1;
    while after_colon < bytes.len() && (bytes[after_colon] as char).is_whitespace() {
        after_colon += 1;
    }
    if after_colon >= bytes.len() || bytes[after_colon] != b'{' {
        return None;
    }

    // Walk the object, balancing braces.
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    let mut end = None;
    for (i, &c) in bytes[after_colon..].iter().enumerate() {
        let abs = after_colon + i;
        if escape {
            escape = false;
            continue;
        }
        if c == b'\\' {
            escape = true;
            continue;
        }
        if c == b'"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(abs + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    serde_json::from_str::<SessionMetadata>(&s[after_colon..end]).ok()
}

fn system_prompt_to_string(system_prompt: Option<&SystemPrompt>) -> Option<String> {
    match system_prompt {
        Some(SystemPrompt::Text(text)) => Some(text.clone()),
        Some(SystemPrompt::Blocks(blocks)) => Some(
            blocks
                .iter()
                .map(|b| b.text.clone())
                .collect::<Vec<_>>()
                .join("\n\n---\n\n"),
        ),
        None => None,
    }
}

/// Truncate a session ID to 8 characters for compact display.
/// Returns a `&str` borrowing from the input — no allocation.
pub fn truncate_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// Strip a leading `<turn_meta>...</turn_meta>` block from saved user text.
///
/// Older sessions can have turn metadata prefixed to the first user message.
/// The session picker and generated session titles should show the user's
/// prompt, not the cache/debug envelope.
pub(crate) fn extract_user_prompt(raw: &str) -> &str {
    let trimmed = raw.trim_start();
    let Some(after_open) = trimmed.strip_prefix("<turn_meta>") else {
        return trimmed;
    };
    if let Some(close_pos) = after_open.find("</turn_meta>") {
        return after_open[close_pos + "</turn_meta>".len()..].trim_start();
    }
    after_open.trim_start()
}

/// Clean a stored title for display, falling back to a neutral label.
pub(crate) fn extract_title(raw: &str) -> &str {
    let title = extract_user_prompt(raw);
    if title.is_empty() { "Session" } else { title }
}

/// Strip common inline thinking/reasoning XML sections from saved assistant
/// text before it is shown in session previews.
pub(crate) fn strip_thinking_tags(text: &str) -> String {
    if !text.contains("<think") && !text.contains("<thinking") && !text.contains("<reasoning") {
        return text.to_string();
    }

    let tags = ["think", "thinking", "reasoning"];
    let mut result = text.to_string();
    for tag in tags {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        while let Some(start) = result.find(&open) {
            let Some(end) = result[start..].find(&close) else {
                break;
            };
            let end_abs = start + end + close.len();
            result.replace_range(start..end_abs, "");
        }
    }
    result
}

/// Truncate a string to create a title (character-safe for UTF-8)
fn truncate_title(s: &str, max_len: usize) -> String {
    let s = s.trim();
    // Older sessions may carry a title saved before sanitization existed;
    // never echo raw controls into stdout or the picker. Take the first
    // line before sanitizing so a legacy multi-line title still shows only
    // its first line.
    let first_line = sanitize_session_title(s.lines().next().unwrap_or(s));
    let first_line = first_line.trim();

    let char_count = first_line.chars().count();
    if char_count <= max_len {
        first_line.to_string()
    } else {
        let truncated: String = first_line.chars().take(max_len - 3).collect();
        format!("{truncated}...")
    }
}

/// Format a session for display in a picker
pub fn format_session_line(meta: &SessionMetadata) -> String {
    let age = format_age(&meta.updated_at);
    let updated = format_session_updated_at(&meta.updated_at, &age);
    let truncated_title = truncate_title(extract_title(&meta.title), 40);
    let fork_label = if meta.parent_session_id.is_some() {
        " | fork"
    } else {
        ""
    };

    format!(
        "{} | {} | {} msgs{} | {}",
        truncate_id(&meta.id),
        truncated_title,
        meta.message_count,
        fork_label,
        updated
    )
}

pub(crate) fn format_session_updated_at(dt: &DateTime<Utc>, age: &str) -> String {
    format!("{} ({age})", dt.format("%Y-%m-%d %H:%M UTC"))
}

/// Format a datetime as relative age
fn format_age(dt: &DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(*dt);

    if duration.num_minutes() < 1 {
        "just now".to_string()
    } else if duration.num_hours() < 1 {
        format!("{}m ago", duration.num_minutes())
    } else if duration.num_days() < 1 {
        format!("{}h ago", duration.num_hours())
    } else if duration.num_weeks() < 1 {
        format!("{}d ago", duration.num_days())
    } else {
        format!("{}w ago", duration.num_weeks())
    }
}

// === Unit Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval_log::ApprovalOutcome;
    use crate::models::ContentBlock;
    use crate::models::Role;
    use crate::tools::plan::StepStatus;
    use crate::tui::history::{HistoryCell, ToolCell, history_cells_from_message};
    use std::fs;
    use tempfile::tempdir;

    fn make_test_message(role: &str, text: &str) -> Message {
        Message {
            role: Role::from(role),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    #[test]
    fn session_goal_sidecar_round_trips_control_state_without_model_output() {
        let tmp = tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("manager");
        let session_id = "11111111-2222-4333-8444-555555555555";
        let runtime = GoalSnapshot {
            objective: Some("finish the provider migration".to_string()),
            status: "paused".to_string(),
            token_budget: Some(50_000),
            tokens_used: 12_345,
            time_used_seconds: 67,
            continuation_count: 4,
            elapsed_seconds: Some(91),
            evidence: Some("Bearer credential-shaped-model-output".to_string()),
            blocker: Some("/arbitrary/private/path".to_string()),
            pause_reason: Some(GoalPauseReason::User),
            completion_verification: None,
            advisories: Vec::new(),
            last_gap_fingerprint: None,
            repeated_gap_count: 0,
        };
        let durable = SessionGoalState::from_runtime(&runtime)
            .expect("valid runtime goal")
            .expect("non-empty durable goal");

        manager
            .save_session_goal(session_id, Some(&durable))
            .expect("save goal");
        let raw = fs::read_to_string(
            sessions_dir
                .join(SESSION_GOALS_DIR)
                .join(format!("{session_id}.json")),
        )
        .expect("read goal sidecar");
        assert!(!raw.contains("credential-shaped-model-output"));
        assert!(!raw.contains("/arbitrary/private/path"));

        let reopened = SessionManager::new(sessions_dir).expect("reopen manager");
        let restored = reopened
            .load_session_goal(session_id)
            .expect("load goal")
            .expect("persisted goal");
        assert_eq!(restored, durable);
        assert_eq!(restored.to_runtime_snapshot().objective, runtime.objective);
        assert_eq!(restored.to_runtime_snapshot().status, "paused");

        reopened
            .save_session_goal(session_id, None)
            .expect("clear goal");
        assert_eq!(
            reopened.load_session_goal(session_id).expect("load clear"),
            None
        );
    }

    /// Coverage state round-trips with the money it qualifies, and a session
    /// written before coverage existed is detected as *unknown* rather than being
    /// read as a complete total covering zero turns (#4318).
    #[test]
    fn cost_snapshot_round_trips_coverage_and_detects_legacy_unknown() {
        // A pre-coverage row: real money, no coverage fields at all.
        let legacy: SessionCostSnapshot = serde_json::from_value(serde_json::json!({
            "session_cost_usd": 1.25,
            "session_cost_cny": 0.0,
            "subagent_cost_usd": 0.0,
            "subagent_cost_cny": 0.0,
            "displayed_cost_high_water_usd": 1.25,
            "displayed_cost_high_water_cny": 0.0
        }))
        .expect("legacy cost snapshot stays readable");
        assert_eq!(legacy.priced_turns, 0);
        assert_eq!(legacy.unpriced_turns, 0);
        assert!(!legacy.coverage_recorded);
        assert!(
            legacy.coverage_is_legacy_unknown(),
            "a non-zero total with no coverage evidence must not read as complete"
        );

        // An all-zero pre-coverage session is still unknown: zero may mean no
        // turns, all unpriced turns, or exact zero usage. Absence of evidence is
        // never rewritten into a complete 0/0 claim.
        let empty = SessionCostSnapshot::default();
        assert!(empty.coverage_is_legacy_unknown());

        // A coverage-aware writer that recorded zero money-metered turns is also
        // not unknown — it positively knows the answer is zero.
        let recorded_zero = SessionCostSnapshot {
            session_cost_usd: 1.25,
            coverage_recorded: true,
            ..SessionCostSnapshot::default()
        };
        assert!(!recorded_zero.coverage_is_legacy_unknown());

        // Full round-trip of every coverage field.
        let full = SessionCostSnapshot {
            session_cost_usd: 2.5,
            session_cost_cny: 3.0,
            subagent_cost_usd: 0.5,
            subagent_cost_cny: 0.25,
            displayed_cost_high_water_usd: 3.0,
            displayed_cost_high_water_cny: 3.25,
            priced_turns: 7,
            unpriced_turns: 2,
            cny_priced_turns: 1,
            cny_unpriced_turns: 8,
            unpriced_reasons: ["missing_class_price".to_string()].into(),
            cny_unpriced_reasons: ["currency_not_published".to_string()].into(),
            unpriced_classes: ["cache_write".to_string()].into(),
            pricing_provenances: ["models_dev_bundled".to_string()].into(),
            live_pricing_defects: ["live_pricing_stale".to_string()].into(),
            live_pricing_unusable_defects: ["live_pricing_scope_mismatch".to_string()].into(),
            route_receipts: ["provider=anthropic identity=- model=claude-haiku-4-5 \
                 surface=first-party-payg endpoint_fp=abc123 currency=usd"
                .to_string()]
            .into(),
            usage_source_fingerprints: ["response-fingerprint".to_string()].into(),
            coverage_recorded: true,
        };
        let json = serde_json::to_string(&full).expect("serialize");
        let back: SessionCostSnapshot = serde_json::from_str(&json).expect("round-trip");
        assert_eq!(back.priced_turns, 7);
        assert_eq!(back.unpriced_turns, 2);
        assert_eq!(back.cny_priced_turns, 1);
        assert_eq!(back.cny_unpriced_turns, 8);
        assert_eq!(back.unpriced_reasons, full.unpriced_reasons);
        assert_eq!(back.cny_unpriced_reasons, full.cny_unpriced_reasons);
        assert_eq!(back.unpriced_classes, full.unpriced_classes);
        assert_eq!(back.pricing_provenances, full.pricing_provenances);
        assert_eq!(back.live_pricing_defects, full.live_pricing_defects);
        assert_eq!(
            back.usage_source_fingerprints,
            full.usage_source_fingerprints
        );
        assert_eq!(
            back.live_pricing_unusable_defects,
            full.live_pricing_unusable_defects
        );
        assert_eq!(back.route_receipts, full.route_receipts);
        assert!(back.coverage_recorded);
        assert!(!back.coverage_is_legacy_unknown());

        // The persisted receipts carry no endpoint URL or credential.
        let lower = json.to_lowercase();
        for needle in ["http", "api_key", "authorization", "bearer", "sk-"] {
            assert!(!lower.contains(needle), "{needle} leaked into {json}");
        }
    }

    /// The USD and CNY totals a snapshot reports are projections of one
    /// dual-currency accumulation, never two independent sums that could
    /// disagree (#4939).
    ///
    /// For any turn sequence — dual-priced, USD-only, CNY-only, or garbage
    /// estimates — folding the turns jointly and projecting each currency must
    /// equal accumulating that currency on its own. This is the invariant that
    /// makes the persisted per-currency columns safe: they are written from the
    /// same joint fold, so a code path can no longer update one and forget the
    /// other. CNY is derived from provider-published CNY rows, not from an FX
    /// multiple of USD, so a USD-only turn must contribute exactly zero CNY.
    #[test]
    fn cost_snapshot_currency_totals_are_projections_of_one_accumulator() {
        use crate::pricing::CostEstimate;

        let turn_sequences: &[&[CostEstimate]] = &[
            // Dual-priced turns (DeepSeek-style routes with a published CNY row).
            &[
                CostEstimate {
                    usd: 0.01,
                    cny: 0.07,
                },
                CostEstimate {
                    usd: 0.02,
                    cny: 0.14,
                },
            ],
            // USD-only turns: CNY unpublished, so the CNY projection stays zero.
            &[
                CostEstimate {
                    usd: 0.25,
                    cny: 0.0,
                },
                CostEstimate { usd: 1.5, cny: 0.0 },
            ],
            // Mixed: one currency priced per turn, alternating.
            &[
                CostEstimate { usd: 0.5, cny: 0.0 },
                CostEstimate { usd: 0.0, cny: 3.5 },
                CostEstimate {
                    usd: 0.125,
                    cny: 0.875,
                },
            ],
            // Hostile values: sanitization must apply identically per currency.
            &[
                CostEstimate {
                    usd: f64::NAN,
                    cny: 0.25,
                },
                CostEstimate {
                    usd: 0.75,
                    cny: -1.0,
                },
                CostEstimate {
                    usd: f64::INFINITY,
                    cny: 0.25,
                },
            ],
        ];

        for turns in turn_sequences {
            // Joint fold: how the app accumulates (one accumulator, both
            // currencies advance together through the same saturating_add).
            let joint = turns.iter().fold(CostEstimate::default(), |acc, turn| {
                acc.saturating_add(*turn)
            });

            // Independent per-currency folds: what a drifted parallel
            // accumulator would compute if it only saw one currency.
            let usd_alone = turns.iter().fold(CostEstimate::default(), |acc, turn| {
                acc.saturating_add(CostEstimate {
                    usd: turn.usd,
                    cny: 0.0,
                })
            });
            let cny_alone = turns.iter().fold(CostEstimate::default(), |acc, turn| {
                acc.saturating_add(CostEstimate {
                    usd: 0.0,
                    cny: turn.cny,
                })
            });

            let snapshot = SessionCostSnapshot {
                session_cost_usd: joint.usd,
                session_cost_cny: joint.cny,
                ..SessionCostSnapshot::default()
            };
            assert_eq!(
                snapshot.total_usd(),
                usd_alone.usd,
                "USD projection drifted from independent accumulation for {turns:?}"
            );
            assert_eq!(
                snapshot.total_cny(),
                cny_alone.cny,
                "CNY projection drifted from independent accumulation for {turns:?}"
            );
            assert_eq!(snapshot.total_estimate().usd, snapshot.total_usd());
            assert_eq!(snapshot.total_estimate().cny, snapshot.total_cny());
        }

        // A USD-only session projects zero CNY — no fabricated FX conversion —
        // and the subagent column joins the same fold.
        let usd_only = SessionCostSnapshot {
            session_cost_usd: 2.5,
            subagent_cost_usd: 0.5,
            ..SessionCostSnapshot::default()
        };
        assert_eq!(usd_only.total_usd(), 3.0);
        assert_eq!(usd_only.total_cny(), 0.0);
    }

    fn write_session_record(
        manager: &SessionManager,
        id: &str,
        workspace: &Path,
        updated_at: DateTime<Utc>,
    ) {
        let session = SavedSession {
            schema_version: CURRENT_SESSION_SCHEMA_VERSION,
            messages: vec![make_test_message("user", "hi")],
            metadata: SessionMetadata {
                id: id.to_string(),
                title: format!("session-{id}"),
                created_at: updated_at,
                updated_at,
                message_count: 1,
                total_tokens: 0,
                model: "deepseek-v4-flash".to_string(),
                model_provider: "deepseek".to_string(),
                model_provider_id: None,
                workspace: workspace.to_path_buf(),
                mode: None,
                cost: SessionCostSnapshot::default(),
                parent_session_id: None,
                forked_from_message_count: None,
                cumulative_turn_secs: 0,
                archived: false,
                spawn_depth: 0,
            },
            journal: None,
            leaf_id: None,
            system_prompt: None,
            context_references: Vec::new(),
            artifacts: Vec::new(),
            approval_receipts: Vec::new(),
            work_state: None,
            window_title: None,
            last_auto_route: None,
        };
        manager.save_session(&session).expect("save");
    }

    fn write_empty_session_record(
        manager: &SessionManager,
        id: &str,
        workspace: &Path,
        updated_at: DateTime<Utc>,
    ) {
        let session = SavedSession {
            schema_version: CURRENT_SESSION_SCHEMA_VERSION,
            messages: Vec::new(),
            metadata: SessionMetadata {
                id: id.to_string(),
                title: DEFAULT_SESSION_TITLE.to_string(),
                created_at: updated_at,
                updated_at,
                message_count: 0,
                total_tokens: 0,
                model: "deepseek-v4-pro".to_string(),
                model_provider: "deepseek".to_string(),
                model_provider_id: None,
                workspace: workspace.to_path_buf(),
                mode: Some("yolo".to_string()),
                cost: SessionCostSnapshot::default(),
                parent_session_id: None,
                forked_from_message_count: None,
                cumulative_turn_secs: 0,
                archived: false,
                spawn_depth: 0,
            },
            journal: None,
            leaf_id: None,
            system_prompt: None,
            context_references: Vec::new(),
            artifacts: Vec::new(),
            approval_receipts: Vec::new(),
            work_state: None,
            window_title: None,
            last_auto_route: None,
        };
        manager.save_session(&session).expect("save empty");
    }

    // === orphaned per-session artifact directories ===

    /// A real `~/.codewhale/sessions` held 780 directories whose session had
    /// long been pruned, each still holding shell-completion evidence.
    #[test]
    fn cleanup_reclaims_session_dirs_whose_session_is_gone() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().to_path_buf()).expect("manager");
        let workspace = tmp.path().join("ws");

        let orphan = "11111111-1111-4111-8111-111111111111";
        let live = "22222222-2222-4222-8222-222222222222";
        for id in [orphan, live] {
            let artifacts = tmp.path().join(id).join("artifacts");
            fs::create_dir_all(&artifacts).expect("artifact dir");
            fs::write(artifacts.join("art_evidence.txt"), b"stdout").expect("artifact");
        }
        // Only `live` still has a session document.
        write_session_record(&manager, live, &workspace, Utc::now());

        manager.cleanup_old_sessions().expect("cleanup");

        assert!(
            !tmp.path().join(orphan).exists(),
            "a directory whose session is gone is reclaimed"
        );
        assert!(
            tmp.path().join(live).join("artifacts").exists(),
            "a directory whose session still exists must be left alone"
        );
    }

    #[test]
    fn a_crashed_sessions_evidence_survives_even_without_its_document() {
        // Recovery reads exactly this: a checkpoint with no session document.
        // Reclaiming its evidence would delete what recovery needs.
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().to_path_buf()).expect("manager");
        let crashed = "33333333-3333-4333-8333-333333333333";

        fs::create_dir_all(tmp.path().join(crashed).join("artifacts")).expect("artifacts");
        let checkpoints = tmp.path().join("checkpoints");
        fs::create_dir_all(&checkpoints).expect("checkpoints dir");
        fs::write(checkpoints.join(format!("{crashed}.json")), b"{}").expect("checkpoint");

        manager.cleanup_old_sessions().expect("cleanup");

        assert!(
            tmp.path().join(crashed).exists(),
            "a crashed session's evidence must outlive its missing document"
        );
    }

    #[test]
    fn reclamation_never_touches_bookkeeping_directories() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().to_path_buf()).expect("manager");
        // `checkpoints` is not a session id and must survive being empty.
        let checkpoints = tmp.path().join("checkpoints");
        fs::create_dir_all(&checkpoints).expect("checkpoints dir");
        let not_a_session = tmp.path().join("some-user-folder");
        fs::create_dir_all(&not_a_session).expect("user dir");

        manager.cleanup_old_sessions().expect("cleanup");

        assert!(checkpoints.exists(), "checkpoints/ is not a session dir");
        assert!(
            not_a_session.exists(),
            "a name that is not a valid session id is not ours to remove"
        );
    }

    #[test]
    fn only_a_real_uuid_can_gate_a_directory_removal() {
        // Real ids from a live ~/.codewhale.
        for id in [
            "db609d23-e25f-48b0-918e-6d1e390a7cb7",
            "5bd5095c-2a10-46bb-9979-ed967d892d45",
            "11111111-1111-4111-8111-111111111111",
        ] {
            assert!(super::is_session_uuid(id), "{id} is a session id");
        }
        // Everything `validated_session_id` would have waved through.
        for name in [
            "checkpoints",
            "some-user-folder",
            "mine",
            "artifacts",
            "db609d23-e25f-48b0-918e-6d1e390a7cb", // short final group
            "db609d23-e25f-48b0-918e-6d1e390a7cb77", // long final group
            "db609d23-e25f-48b0-918e",             // four groups
            "zz609d23-e25f-48b0-918e-6d1e390a7cb7", // non-hex
            "",
        ] {
            assert!(
                !super::is_session_uuid(name),
                "{name:?} must never gate a remove_dir_all"
            );
        }
    }

    #[test]
    fn save_and_resume_reconstructs_closed_and_interrupted_approvals() {
        let tmp = tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("manager");
        let session = create_saved_session(
            &[make_test_message("user", "approval recovery")],
            "test-model",
            tmp.path(),
            0,
            None,
        );
        let session_id = session.metadata.id.clone();
        let store = ApprovalReceiptStore::new(sessions_dir);
        store
            .append(
                &session_id,
                &ApprovalReceipt::asked("tool-complete", "exec_shell"),
            )
            .expect("persist completed ask");
        store
            .append(
                &session_id,
                &ApprovalReceipt::decided("tool-complete", ApprovalOutcome::Denied),
            )
            .expect("persist completed decision");
        store
            .append(
                &session_id,
                &ApprovalReceipt::asked("tool-interrupted", "write_file"),
            )
            .expect("persist interrupted ask");

        manager.save_session(&session).expect("save session");
        let resumed = manager
            .load_session_snapshot(&session_id)
            .expect("resume session");
        let replay = ApprovalReplay::from_receipts(&resumed.approval_receipts)
            .expect("replay resumed approval evidence");

        assert_eq!(resumed.messages, session.messages);
        assert_eq!(replay.completed.len(), 1);
        assert_eq!(replay.completed[0].outcome, ApprovalOutcome::Denied);
        assert_eq!(replay.unmatched_asks.len(), 1);
        assert!(matches!(
            &replay.unmatched_asks[0],
            ApprovalReceipt::Asked { tool_call_id, .. } if tool_call_id == "tool-interrupted"
        ));
        assert_eq!(
            manager
                .replay_approvals(&session_id)
                .expect("replay canonical sidecar"),
            replay
        );
    }

    #[test]
    fn session_boot_owner_stamps_only_the_creating_instance() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().to_path_buf()).expect("manager");
        let workspace = tmp.path().join("ws");

        // A record this instance creates is stamped with this boot id and is
        // therefore not prior-instance work.
        write_session_record(&manager, "mine", &workspace, Utc::now());
        assert_eq!(
            manager.session_boot_owner("mine").as_deref(),
            Some(current_session_boot_id())
        );
        assert!(!manager.session_from_prior_instance("mine"));

        // An id with no durable record at all is this instance's own
        // not-yet-persisted session.
        assert!(!manager.session_from_prior_instance("unsaved"));

        // A record stamped by another boot id stays owned by that instance,
        // even after this instance re-serializes it (crash recovery must not
        // re-badge restored work as ours).
        manager
            .record_session_boot_owner("theirs", "boot_other_instance")
            .expect("stamp");
        write_session_record(&manager, "theirs", &workspace, Utc::now());
        assert_eq!(
            manager.session_boot_owner("theirs").as_deref(),
            Some("boot_other_instance")
        );
        assert!(manager.session_from_prior_instance("theirs"));

        // A legacy record with no marker is classified as prior-instance
        // work, and a later re-save keeps it unclaimed.
        write_session_record(&manager, "legacy", &workspace, Utc::now());
        manager.clear_session_boot_owner("legacy");
        assert!(manager.session_from_prior_instance("legacy"));
        write_session_record(&manager, "legacy", &workspace, Utc::now());
        assert!(manager.session_from_prior_instance("legacy"));

        // Deleting the record drops its marker.
        manager.delete_session("theirs").expect("delete");
        assert_eq!(manager.session_boot_owner("theirs"), None);
    }

    #[test]
    fn session_boot_owner_sidecar_never_lists_as_a_session() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().to_path_buf()).expect("manager");
        write_session_record(&manager, "real", &tmp.path().join("ws"), Utc::now());
        assert!(manager.session_boot_owners_path().exists());
        let listed = manager.list_sessions().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "real");
        // The reserved stem cannot be claimed as a session id either.
        assert!(manager.load_session("session_boot_owners").is_err());
    }

    #[test]
    fn test_session_manager_new() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        assert!(tmp.path().join("sessions").exists());
        let _ = manager;
    }

    #[test]
    fn test_save_and_load_session() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");

        let messages = vec![
            make_test_message("user", "Hello!"),
            make_test_message("assistant", "Hi there!"),
        ];

        let session = create_saved_session(&messages, "test-model", tmp.path(), 100, None);
        let session_id = session.metadata.id.clone();

        manager.save_session(&session).expect("save");

        let loaded = manager.load_session(&session_id).expect("load");
        assert_eq!(loaded.metadata.id, session_id);
        assert_eq!(loaded.messages.len(), 2);
    }

    /// #4681: reopening a session must not surface `<turn_meta>` machine
    /// blocks in the transcript. Covers the current trailing shape and the
    /// legacy leading shape (sessions saved before the turn-meta tail move),
    /// while the loaded API history keeps both envelopes intact for replay.
    #[test]
    fn rehydrated_turn_meta_blocks_never_render_in_history_cells() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");

        let turn_meta = "<turn_meta>\nCurrent local date: 2026-08-01\n</turn_meta>";
        let trailing_shape = Message {
            role: Role::User,
            content: vec![
                ContentBlock::Text {
                    text: "Fix the flaky test".to_string(),
                    cache_control: None,
                },
                ContentBlock::Text {
                    text: turn_meta.to_string(),
                    cache_control: None,
                },
            ],
        };
        let legacy_leading_shape = Message {
            role: Role::User,
            content: vec![
                ContentBlock::Text {
                    text: turn_meta.to_string(),
                    cache_control: None,
                },
                ContentBlock::Text {
                    text: "Now add the docs".to_string(),
                    cache_control: None,
                },
            ],
        };
        let messages = vec![
            trailing_shape,
            make_test_message("assistant", "Done."),
            legacy_leading_shape,
        ];
        let session = create_saved_session(&messages, "test-model", tmp.path(), 100, None);
        let session_id = session.metadata.id.clone();
        manager.save_session(&session).expect("save");

        let loaded = manager.load_session(&session_id).expect("load");

        // Display path: no rendered cell may carry turn_meta markup.
        let rendered: Vec<HistoryCell> = loaded
            .messages
            .iter()
            .flat_map(history_cells_from_message)
            .collect();
        let user_texts: Vec<&str> = rendered
            .iter()
            .filter_map(|cell| match cell {
                HistoryCell::User { content } => Some(content.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(user_texts, vec!["Fix the flaky test", "Now add the docs"]);
        assert!(
            !user_texts.iter().any(|text| text.contains("<turn_meta")),
            "rendered cells must not contain turn_meta markup: {user_texts:?}"
        );

        // Model-facing replay: the persisted envelopes survive the round trip.
        let replayed_envelopes = loaded
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter(|block| {
                matches!(block, ContentBlock::Text { text, .. } if text.contains("<turn_meta>"))
            })
            .count();
        assert_eq!(replayed_envelopes, 2);
    }

    #[test]
    fn runtime_snapshot_load_preserves_in_flight_tool_call() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let messages = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call-in-flight".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "README.md"}),
                caller: None,
                thought_signature: None,
            }],
        }];
        let session = create_saved_session(&messages, "test-model", tmp.path(), 0, None);
        let session_id = session.metadata.id.clone();
        manager.save_session(&session).expect("save");

        let loaded = manager
            .load_session_snapshot(&session_id)
            .expect("snapshot load");

        assert_eq!(loaded.messages, messages);
        assert_eq!(loaded.metadata.message_count, 1);
        assert!(!loaded.messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::ToolResult { content, .. }
                        if content.contains("crashed_and_repaired")
                )
            })
        }));
    }

    #[test]
    fn explicit_session_recovery_is_reported_and_idempotent_after_save() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let messages = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call-crashed".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "README.md"}),
                caller: None,
                thought_signature: None,
            }],
        }];
        let session = create_saved_session(&messages, "test-model", tmp.path(), 0, None);
        let session_id = session.metadata.id.clone();
        manager.save_session(&session).expect("save");

        let recovered = manager
            .recover_session_for_resume(&session_id)
            .expect("recover");
        assert!(recovered.changed);
        assert_eq!(recovered.repaired_call_count, 1);
        assert_eq!(recovered.duplicate_result_count, 0);
        assert_eq!(recovered.orphan_result_count, 0);
        manager
            .save_session(&recovered.session)
            .expect("persist recovery");

        let second = manager
            .recover_session_for_resume(&session_id)
            .expect("recover twice");
        assert!(!second.changed);
        assert_eq!(second.repaired_call_count, 0);
        assert_eq!(second.session.messages, recovered.session.messages);
    }

    #[test]
    fn load_session_repairs_dangling_tool_call_with_visible_receipt() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let messages = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call-crashed".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "README.md"}),
                caller: None,
                thought_signature: None,
            }],
        }];
        let session = create_saved_session(&messages, "test-model", tmp.path(), 0, None);
        let session_id = session.metadata.id.clone();
        manager.save_session(&session).expect("save");

        let loaded = manager.load_session(&session_id).expect("load");

        assert_eq!(loaded.metadata.message_count, loaded.messages.len());
        assert!(loaded.messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error: Some(true),
                        ..
                    } if tool_use_id == "call-crashed" && content.contains("crashed_and_repaired")
                )
            })
        }));
        assert_eq!(
            loaded.journal.as_ref().map(SessionJournal::to_messages),
            Some(loaded.messages.clone()),
            "the append-only journal must follow the repaired active branch"
        );
        assert!(loaded.messages.iter().any(|message| {
            (message.role == "assistant"
                || message.role == crate::models::INTERRUPTED_ASSISTANT_ROLE)
                && message.content.iter().any(|block| {
                    matches!(
                        block,
                        ContentBlock::Text { text, .. }
                            if text.contains("[tool_history_repair]")
                    )
                })
        }));
    }

    #[test]
    fn save_and_load_session_preserves_rich_update_plan_tool_payload() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let messages = vec![
            make_test_message("user", "plan this carefully"),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "plan-1".to_string(),
                    name: "update_plan".to_string(),
                    input: serde_json::json!({
                        "objective": "Make Plan mode reviewable",
                        "sources_used": ["gh issue view 2691"],
                        "critical_files": ["crates/tui/src/tools/plan.rs"],
                        "constraints": ["Preserve legacy update_plan payloads"],
                        "verification_plan": "Run focused plan tests",
                        "handoff_packet": "Next agent should inspect replay",
                        "plan": [
                            { "step": "render replay card", "status": "completed" }
                        ]
                    }),
                    caller: None,
                    thought_signature: None,
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "plan-1".to_string(),
                    content: "Plan updated".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];
        let session = create_saved_session(&messages, "deepseek-v4-flash", tmp.path(), 42, None);
        let session_id = session.metadata.id.clone();

        manager.save_session(&session).expect("save");
        let loaded = manager.load_session(&session_id).expect("load");

        assert_eq!(loaded.messages.len(), 3);
        let cells = history_cells_from_message(&loaded.messages[1]);
        let Some(HistoryCell::Tool(ToolCell::PlanUpdate(cell))) = cells.first() else {
            panic!("expected loaded update_plan to replay as a PlanUpdate cell");
        };
        assert_eq!(
            cell.snapshot.objective.as_deref(),
            Some("Make Plan mode reviewable")
        );
        assert_eq!(
            cell.snapshot.critical_files,
            vec!["crates/tui/src/tools/plan.rs"]
        );
        assert_eq!(cell.snapshot.items[0].status, StepStatus::Completed);
    }

    #[test]
    fn save_session_preserves_large_tool_outputs_for_cache_fidelity() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let raw = "RAW_SESSION_SENTINEL\n".repeat(2_000);
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call-big".to_string(),
                    name: "exec_shell".to_string(),
                    input: serde_json::json!({"command": "cargo test -p codewhale-tui"}),
                    caller: None,
                    thought_signature: None,
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call-big".to_string(),
                    content: raw.clone(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];
        let mut session = create_saved_session(&messages, "test-model", tmp.path(), 100, None);
        session.artifacts.push(crate::artifacts::ArtifactRecord {
            id: "art_call-big".to_string(),
            kind: crate::artifacts::ArtifactKind::ToolOutput,
            session_id: session.metadata.id.clone(),
            tool_call_id: "call-big".to_string(),
            tool_name: "exec_shell".to_string(),
            created_at: Utc::now(),
            byte_size: raw.len() as u64,
            preview: "checking crate ... error[E0425]".to_string(),
            storage_path: PathBuf::from("artifacts/art_call-big.txt"),
        });

        let path = manager.save_session(&session).expect("save");
        let persisted_json = fs::read_to_string(path).expect("read persisted session");
        // Raw output is preserved in-session so resume can hit the LLM cache.
        assert!(persisted_json.contains("RAW_SESSION_SENTINEL"));

        let loaded = manager.load_session(&session.metadata.id).expect("load");
        let ContentBlock::ToolResult { content, .. } = &loaded.messages[1].content[0] else {
            panic!("expected loaded tool result");
        };
        // Loaded session retains the original output for cache fidelity.
        assert!(content.contains("RAW_SESSION_SENTINEL"));
        assert!(!content.contains("[TOOL_OUTPUT_RECEIPT]"));
    }

    #[test]
    fn load_session_preserves_legacy_large_tool_outputs_for_cache_fidelity() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let raw = "RAW_LEGACY_RESUME_SENTINEL\n".repeat(2_000);
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call-legacy".to_string(),
                    name: "exec_shell".to_string(),
                    input: serde_json::json!({"command": "cargo check"}),
                    caller: None,
                    thought_signature: None,
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call-legacy".to_string(),
                    content: raw.clone(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];
        let mut session = create_saved_session(&messages, "test-model", tmp.path(), 100, None);
        session.artifacts.push(crate::artifacts::ArtifactRecord {
            id: "art_call-legacy".to_string(),
            kind: crate::artifacts::ArtifactKind::ToolOutput,
            session_id: session.metadata.id.clone(),
            tool_call_id: "call-legacy".to_string(),
            tool_name: "exec_shell".to_string(),
            created_at: Utc::now(),
            byte_size: raw.len() as u64,
            preview: "cargo check output".to_string(),
            storage_path: PathBuf::from("artifacts/art_call-legacy.txt"),
        });
        let path = manager
            .validated_session_path(&session.metadata.id)
            .expect("path");
        fs::write(
            &path,
            serde_json::to_string_pretty(&session).expect("serialize legacy session"),
        )
        .expect("write legacy raw session");
        assert!(
            fs::read_to_string(&path)
                .expect("read legacy raw")
                .contains("RAW_LEGACY_RESUME_SENTINEL")
        );

        let loaded = manager.load_session(&session.metadata.id).expect("load");
        let ContentBlock::ToolResult { content, .. } = &loaded.messages[1].content[0] else {
            panic!("expected loaded tool result");
        };
        // Loaded session preserves original output so resume can hit the LLM cache.
        assert!(content.contains("RAW_LEGACY_RESUME_SENTINEL"));
        assert!(!content.contains("[TOOL_OUTPUT_RECEIPT]"));
    }

    #[test]
    fn test_list_sessions() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");

        // Create a few sessions
        for i in 0..3 {
            let messages = vec![make_test_message("user", &format!("Session {i}"))];
            let session = create_saved_session(&messages, "test-model", tmp.path(), 100, None);
            manager.save_session(&session).expect("save");
        }

        let sessions = manager.list_sessions().expect("list");
        assert_eq!(sessions.len(), 3);
    }

    #[test]
    fn default_manager_copies_legacy_sessions_when_primary_already_exists() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        let _home = crate::test_support::EnvVarGuard::set("HOME", &home);
        let _codewhale_home = crate::test_support::EnvVarGuard::remove("CODEWHALE_HOME");

        let primary_sessions = home.join(".codewhale").join("sessions");
        let legacy_sessions = home.join(".deepseek").join("sessions");
        fs::create_dir_all(&primary_sessions).expect("primary sessions");
        fs::create_dir_all(&legacy_sessions).expect("legacy sessions");
        fs::create_dir_all(legacy_sessions.join("checkpoints")).expect("legacy checkpoints");
        fs::write(
            legacy_sessions.join("checkpoints").join("latest.json"),
            "{}",
        )
        .expect("legacy checkpoint");

        let mut legacy_session = create_saved_session(
            &[make_test_message("user", "find my old session")],
            "test-model",
            tmp.path(),
            100,
            None,
        );
        legacy_session.metadata.id = "legacy-visible".to_string();
        legacy_session.metadata.title = "session from legacy home".to_string();
        fs::write(
            legacy_sessions.join("legacy-visible.json"),
            serde_json::to_string_pretty(&legacy_session).expect("serialize legacy session"),
        )
        .expect("write legacy session");

        let manager = SessionManager::default_location().expect("default manager");
        assert_eq!(manager.sessions_dir(), primary_sessions.as_path());
        assert!(primary_sessions.join("legacy-visible.json").exists());
        assert!(!primary_sessions.join("checkpoints").exists());
        assert!(legacy_sessions.join("legacy-visible.json").exists());

        let sessions = manager.list_sessions().expect("list");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "legacy-visible");
    }

    #[test]
    fn legacy_session_copy_never_overwrites_primary_session() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        let _home = crate::test_support::EnvVarGuard::set("HOME", &home);
        let _codewhale_home = crate::test_support::EnvVarGuard::remove("CODEWHALE_HOME");

        let primary_sessions = home.join(".codewhale").join("sessions");
        let legacy_sessions = home.join(".deepseek").join("sessions");
        fs::create_dir_all(&primary_sessions).expect("primary sessions");
        fs::create_dir_all(&legacy_sessions).expect("legacy sessions");

        let primary_path = primary_sessions.join("same-id.json");
        fs::write(&primary_path, "primary data wins").expect("write primary session");
        fs::write(
            legacy_sessions.join("same-id.json"),
            "legacy data must not overwrite",
        )
        .expect("write legacy session");

        let dir = default_sessions_dir().expect("default session dir");
        assert_eq!(dir, primary_sessions);
        assert_eq!(
            fs::read_to_string(primary_path).expect("read primary session"),
            "primary data wins"
        );
    }

    #[test]
    fn explicit_codewhale_home_disables_legacy_session_copy() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        let explicit_home = tmp.path().join("explicit-codewhale");
        let _home = crate::test_support::EnvVarGuard::set("HOME", &home);
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &explicit_home);

        let legacy_sessions = home.join(".deepseek").join("sessions");
        fs::create_dir_all(&legacy_sessions).expect("legacy sessions");
        fs::write(legacy_sessions.join("legacy-visible.json"), "{}").expect("write legacy session");

        let dir = default_sessions_dir().expect("default session dir");
        assert_eq!(dir, explicit_home.join("sessions"));
        assert!(!dir.join("legacy-visible.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_codewhale_home_is_still_an_explicit_session_boundary() {
        use std::os::unix::ffi::OsStringExt;

        let _lock = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        let explicit_home = tmp.path().join(std::ffi::OsString::from_vec(
            b"codewhale-\xff-home".to_vec(),
        ));
        let _home = crate::test_support::EnvVarGuard::set("HOME", &home);
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &explicit_home);

        let legacy_sessions = home.join(".deepseek").join("sessions");
        fs::create_dir_all(&legacy_sessions).expect("legacy sessions");
        fs::write(legacy_sessions.join("ambient.json"), "ambient").expect("ambient legacy session");
        let safe_primary = tmp.path().join("safe-primary");
        fs::create_dir_all(&safe_primary).expect("safe primary");

        assert_eq!(
            merge_missing_legacy_session_entries(&safe_primary).expect("merge decision"),
            0
        );
        assert!(!safe_primary.join("ambient.json").exists());
    }

    #[test]
    fn latest_session_for_workspace_ignores_newer_other_directory() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let workspace_a = tmp.path().join("aa").join("aaa");
        let workspace_b = tmp.path().join("bb").join("bbb");
        fs::create_dir_all(&workspace_a).expect("mkdir workspace a");
        fs::create_dir_all(&workspace_b).expect("mkdir workspace b");
        fs::create_dir_all(tmp.path().join(".git")).expect("mkdir invalid git boundary");

        write_session_record(
            &manager,
            "current-workspace",
            &workspace_a,
            Utc::now() - chrono::Duration::minutes(10),
        );
        write_session_record(&manager, "other-workspace", &workspace_b, Utc::now());

        let global = manager
            .list_sessions()
            .expect("list")
            .into_iter()
            .next()
            .expect("global latest");
        assert_eq!(global.id, "other-workspace");

        let scoped = manager
            .get_latest_session_for_workspace(&workspace_a)
            .expect("latest for workspace")
            .expect("scoped latest");
        assert_eq!(scoped.id, "current-workspace");
    }

    #[test]
    fn latest_session_for_workspace_ignores_invalid_parent_git_marker() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let workspace_a = tmp.path().join("aa").join("aaa");
        let workspace_b = tmp.path().join("bb").join("bbb");
        fs::create_dir_all(&workspace_a).expect("mkdir workspace a");
        fs::create_dir_all(&workspace_b).expect("mkdir workspace b");
        fs::create_dir_all(tmp.path().join(".git")).expect("mkdir invalid git marker");

        write_session_record(
            &manager,
            "current-workspace",
            &workspace_a,
            Utc::now() - chrono::Duration::minutes(10),
        );
        write_session_record(&manager, "other-workspace", &workspace_b, Utc::now());

        let scoped = manager
            .get_latest_session_for_workspace(&workspace_a)
            .expect("latest for workspace")
            .expect("scoped latest");
        assert_eq!(scoped.id, "current-workspace");
    }

    #[test]
    fn latest_session_for_workspace_matches_same_git_repository() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let repo = tmp.path().join("repo");
        let repo_app = repo.join("apps").join("client");
        let repo_crate = repo.join("crates").join("server");
        let other_repo = tmp.path().join("other").join("project");
        fs::create_dir_all(repo.join(".git")).expect("mkdir .git");
        fs::write(repo.join(".git").join("HEAD"), "ref: refs/heads/main\n").expect("write HEAD");
        fs::create_dir_all(&repo_app).expect("mkdir repo app");
        fs::create_dir_all(&repo_crate).expect("mkdir repo crate");
        fs::create_dir_all(&other_repo).expect("mkdir other repo");

        write_session_record(
            &manager,
            "same-repo",
            &repo_app,
            Utc::now() - chrono::Duration::minutes(5),
        );
        write_session_record(&manager, "other-repo", &other_repo, Utc::now());

        let scoped = manager
            .get_latest_session_for_workspace(&repo_crate)
            .expect("latest for workspace")
            .expect("same repo latest");
        assert_eq!(scoped.id, "same-repo");
    }

    #[test]
    fn latest_session_for_workspace_skips_empty_auto_created_session() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let workspace = tmp.path().join("repo");
        fs::create_dir_all(&workspace).expect("mkdir workspace");

        write_session_record(
            &manager,
            "interrupted-user-turn",
            &workspace,
            Utc::now() - chrono::Duration::minutes(5),
        );
        write_empty_session_record(&manager, "empty-auto-shell", &workspace, Utc::now());

        let global = manager
            .list_sessions()
            .expect("list")
            .into_iter()
            .next()
            .expect("global latest");
        assert_eq!(global.id, "empty-auto-shell");

        let scoped = manager
            .get_latest_session_for_workspace(&workspace)
            .expect("latest for workspace")
            .expect("scoped latest");
        assert_eq!(scoped.id, "interrupted-user-turn");
    }

    #[test]
    fn test_load_by_prefix() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");

        let messages = vec![make_test_message("user", "Test session")];
        let session = create_saved_session(&messages, "test-model", tmp.path(), 100, None);
        let prefix = truncate_id(&session.metadata.id).to_string();
        manager.save_session(&session).expect("save");

        let loaded = manager.load_session_by_prefix(&prefix).expect("load");
        assert_eq!(loaded.messages.len(), 1);
    }

    #[test]
    fn test_delete_session() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");

        let messages = vec![make_test_message("user", "To be deleted")];
        let session = create_saved_session(&messages, "test-model", tmp.path(), 100, None);
        let session_id = session.metadata.id.clone();

        manager.save_session(&session).expect("save");
        assert!(manager.load_session(&session_id).is_ok());

        manager.delete_session(&session_id).expect("delete");
        assert!(manager.load_session(&session_id).is_err());
    }

    #[test]
    fn delete_session_removes_artifact_directory() {
        let tmp = tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("new");

        let session = create_saved_session(
            &[make_test_message("user", "artifact session")],
            "test-model",
            tmp.path(),
            100,
            None,
        );
        let session_id = session.metadata.id.clone();
        let artifact_dir = sessions_dir.join(&session_id).join("artifacts");
        fs::create_dir_all(&artifact_dir).expect("artifact dir");
        fs::write(artifact_dir.join("art_call.txt"), "raw output").expect("artifact file");

        manager.save_session(&session).expect("save");
        manager.delete_session(&session_id).expect("delete");

        assert!(!sessions_dir.join(format!("{session_id}.json")).exists());
        assert!(!sessions_dir.join(&session_id).exists());
    }

    #[test]
    fn test_session_id_rejects_invalid_characters() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");

        let err = manager
            .load_session("../outside")
            .expect_err("invalid id should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);

        let err = manager
            .delete_session("sess bad")
            .expect_err("invalid id should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_session_manager_rejects_relative_traversal_dir() {
        let err = SessionManager::new(PathBuf::from("../sessions"))
            .expect_err("relative traversal directory should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_truncate_title() {
        assert_eq!(truncate_title("Short", 50), "Short");
        assert_eq!(
            truncate_title("This is a very long title that should be truncated", 20),
            "This is a very lo..."
        );
        assert_eq!(truncate_title("Line 1\nLine 2", 50), "Line 1");
    }

    #[test]
    fn extract_user_prompt_strips_turn_meta_prefix() {
        assert_eq!(
            extract_user_prompt("<turn_meta>{\"cache\":\"x\"}</turn_meta>\nReal prompt"),
            "Real prompt"
        );
        assert_eq!(extract_user_prompt("  Real prompt"), "Real prompt");
        assert_eq!(
            extract_user_prompt("<turn_meta>{\"unterminated\":true}\nReal prompt"),
            "{\"unterminated\":true}\nReal prompt"
        );
    }

    #[test]
    fn create_saved_session_uses_prompt_after_turn_meta_for_title() {
        let tmp = tempdir().expect("tempdir");
        let messages = vec![make_test_message(
            "user",
            "<turn_meta>{\"cache\":\"x\"}</turn_meta>\nFix the session picker history pane",
        )];
        let session = create_saved_session(&messages, "test-model", tmp.path(), 100, None);
        assert_eq!(
            session.metadata.title,
            "Fix the session picker history pane"
        );
    }

    #[test]
    fn strip_thinking_tags_removes_common_inline_blocks() {
        let text = "Before <think>private</think> middle <reasoning>hidden</reasoning> after";
        let cleaned = strip_thinking_tags(text);
        assert_eq!(cleaned, "Before  middle  after");
        assert_eq!(strip_thinking_tags("plain answer"), "plain answer");
    }

    #[test]
    fn test_format_age() {
        let now = Utc::now();
        assert_eq!(format_age(&now), "just now");

        let hour_ago = now - chrono::Duration::hours(2);
        assert_eq!(format_age(&hour_ago), "2h ago");

        let day_ago = now - chrono::Duration::days(3);
        assert_eq!(format_age(&day_ago), "3d ago");
    }

    #[test]
    fn session_titles_never_keep_terminal_controls_or_bidi_format_chars() {
        let raw = "Ev\u{1b}]0;PWNED\u{7}il\u{202e}R\u{200b}Z\u{9d}0;X\u{9c}After\u{2066}B\u{2069} 会議 🐳";
        assert_eq!(
            sanitize_session_title(raw),
            "Ev]0;PWNEDilRZ0;XAfterB 会議 🐳"
        );
        // Every rename surface goes through normalize_session_title.
        assert_eq!(
            normalize_session_title(raw).unwrap(),
            "Ev]0;PWNEDilRZ0;XAfterB 会議 🐳"
        );
        // A title that is nothing but controls is an empty title.
        assert!(normalize_session_title("\u{1b}\u{7}\u{200b}").is_err());
        // The listing line re-sanitizes titles saved before this policy.
        assert_eq!(truncate_title(raw, 40), "Ev]0;PWNEDilRZ0;XAfterB 会議 🐳");
    }

    #[test]
    fn format_session_line_includes_absolute_updated_timestamp() {
        let mut session = create_saved_session(
            &[make_test_message("user", "Find Friday work")],
            "test-model",
            Path::new("/tmp/project"),
            100,
            None,
        );
        session.metadata.updated_at = DateTime::parse_from_rfc3339("2026-06-01T12:34:00Z")
            .expect("timestamp")
            .with_timezone(&Utc);

        let line = format_session_line(&session.metadata);

        assert!(
            line.contains("2026-06-01 12:34 UTC"),
            "session list should include an absolute timestamp, got {line:?}"
        );
    }

    #[test]
    fn test_update_session() {
        let tmp = tempdir().expect("tempdir");

        let messages = vec![make_test_message("user", "Hello")];
        let session = create_saved_session(&messages, "test-model", tmp.path(), 50, None);

        let new_messages = vec![
            make_test_message("user", "Hello"),
            make_test_message("assistant", "Hi!"),
        ];

        let updated = update_session(session, &new_messages, 100, None);
        assert_eq!(updated.messages.len(), 2);
        assert_eq!(updated.metadata.total_tokens, 100);
    }

    #[test]
    fn save_load_round_trip_preserves_all_messages_for_cache_fidelity() {
        #[derive(serde::Deserialize)]
        struct LegacySession {
            messages: Vec<Message>,
        }

        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        // Covers the old 500-message cap boundary and well beyond.
        for count in [0, 1, 500, 501, 600, 1000] {
            let original: Vec<_> = (0..count)
                .map(|i| {
                    make_test_message(
                        if i % 2 == 0 { "user" } else { "assistant" },
                        &format!("round-trip message {i}"),
                    )
                })
                .collect();

            let mut session = create_saved_session(&original, "test-model", tmp.path(), 0, None);
            let expected_journal = session.journal.clone();
            session.compact_for_persistence_queue();
            let path = manager.save_session(&session).expect("save");
            let legacy: LegacySession =
                serde_json::from_slice(&fs::read(path).expect("read")).expect("legacy reader");
            let loaded = manager.load_session(&session.metadata.id).expect("load");

            assert_eq!(
                legacy.messages, original,
                "legacy messages for count={count}"
            );
            assert_eq!(
                loaded.journal, expected_journal,
                "journal for count={count}"
            );
            assert_eq!(
                loaded.messages.len(),
                count,
                "count preserved for count={count}"
            );
            assert_eq!(
                loaded.messages, original,
                "every message byte-identical after round-trip for count={count}"
            );
        }
    }

    #[test]
    fn test_checkpoint_round_trip_and_clear() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let messages = vec![make_test_message("user", "checkpoint me")];
        let mut session = create_saved_session(&messages, "test-model", tmp.path(), 12, None);
        session.work_state = Some(SessionWorkState {
            todos: crate::tools::todo::TodoListSnapshot {
                items: vec![crate::tools::todo::TodoItem {
                    id: 1,
                    content: "verify checkpoint durability".to_string(),
                    status: crate::tools::todo::TodoStatus::InProgress,
                }],
                completion_pct: 0,
                in_progress_id: Some(1),
            },
            ..SessionWorkState::default()
        });
        let expected_messages = session.messages.clone();
        let expected_journal = session.journal.clone();
        session.compact_for_persistence_queue();

        let path = manager.save_checkpoint(&session).expect("save checkpoint");
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some(format!("{}.json", session.metadata.id).as_str()),
            "checkpoint file must be keyed by session id"
        );
        let loaded = manager
            .load_session_checkpoint(&session.metadata.id)
            .expect("load checkpoint")
            .expect("checkpoint exists");
        assert_eq!(loaded.metadata.id, session.metadata.id);
        assert_eq!(loaded.messages, expected_messages);
        assert_eq!(loaded.journal, expected_journal);
        assert_eq!(
            loaded.work_state, session.work_state,
            "work state must survive the checkpoint round trip"
        );

        manager
            .clear_session_checkpoint(&session.metadata.id)
            .expect("clear checkpoint");
        assert!(
            manager
                .load_session_checkpoint(&session.metadata.id)
                .expect("load checkpoint")
                .is_none()
        );
    }

    #[test]
    fn graph_backed_work_state_remains_readable_by_legacy_shape() {
        #[derive(serde::Deserialize)]
        struct LegacyWorkState {
            #[serde(default)]
            todos: crate::tools::todo::TodoListSnapshot,
            #[serde(default)]
            plan: crate::tools::plan::PlanSnapshot,
        }

        let fixture = include_bytes!("../tests/fixtures/work_graph_session_v1_reader.json");
        let current: SavedSession = serde_json::from_slice(fixture).expect("current reader");
        let state = current.work_state.expect("fixture Work state");
        let legacy: LegacyWorkState = serde_json::from_value(
            serde_json::from_slice::<serde_json::Value>(fixture)
                .expect("fixture JSON")["work_state"]
                .clone(),
        )
        .expect("v1 reader ignores graph");
        assert_eq!(legacy.todos, state.todos);
        assert_eq!(legacy.plan, state.plan);
        let graph = state.graph.expect("fixture graph");
        crate::work_graph::validate(&graph).expect("valid fixture graph");
        assert_eq!(crate::work_graph::project_todos(&graph), state.todos);
        assert_eq!(crate::work_graph::project_plan(&graph), state.plan);
    }

    #[test]
    fn first_graph_write_archives_exact_legacy_session_once() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let mut session = create_saved_session(
            &[make_test_message("user", "archive before import")],
            "test-model",
            tmp.path(),
            0,
            None,
        );
        let plan = crate::tools::plan::PlanSnapshot {
            items: vec![crate::tools::plan::PlanItemArg {
                step: "Import".to_string(),
                status: crate::tools::plan::StepStatus::Pending,
            }],
            ..crate::tools::plan::PlanSnapshot::default()
        };
        let todos = crate::tools::todo::TodoListSnapshot::default();
        session.work_state = Some(SessionWorkState {
            graph: None,
            todos: todos.clone(),
            plan: plan.clone(),
        });
        let path = manager.save_session(&session).expect("save legacy session");
        let legacy_bytes = fs::read(&path).expect("read legacy bytes");

        let graph = crate::work_graph::import_legacy(&session.metadata.id, &plan, &todos)
            .expect("import graph");
        session.work_state = Some(SessionWorkState {
            graph: Some(graph),
            todos,
            plan,
        });
        manager.save_session(&session).expect("first graph write");
        let archive = manager
            .sessions_dir
            .join(WORK_GRAPH_IMPORT_ARCHIVE_DIR)
            .join(path.file_name().expect("session filename"));
        assert_eq!(fs::read(&archive).expect("archive exists"), legacy_bytes);

        session.metadata.title = "later graph write".to_string();
        manager.save_session(&session).expect("second graph write");
        assert_eq!(
            fs::read(&archive).expect("archive still exists"),
            legacy_bytes,
            "later graph writes must not replace the pre-import receipt"
        );
    }

    #[test]
    fn checkpoints_are_independent_per_session() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let first = create_saved_session(
            &[make_test_message("user", "session one")],
            "test-model",
            tmp.path(),
            0,
            None,
        );
        let second = create_saved_session(
            &[make_test_message("user", "session two")],
            "test-model",
            tmp.path(),
            0,
            None,
        );

        manager.save_checkpoint(&first).expect("save first");
        manager.save_checkpoint(&second).expect("save second");
        manager
            .clear_session_checkpoint(&first.metadata.id)
            .expect("clear first");

        assert!(
            manager
                .load_session_checkpoint(&first.metadata.id)
                .expect("load first")
                .is_none(),
            "clearing one session must remove only that session's file"
        );
        let survivor = manager
            .load_session_checkpoint(&second.metadata.id)
            .expect("load second")
            .expect("second checkpoint survives");
        assert_eq!(survivor.metadata.id, second.metadata.id);
    }

    #[test]
    fn list_checkpoints_includes_legacy_slot_and_skips_offline_queue() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let session = create_saved_session(
            &[make_test_message("user", "list me")],
            "test-model",
            tmp.path(),
            0,
            None,
        );
        manager.save_checkpoint(&session).expect("save checkpoint");
        let checkpoints = tmp.path().join("sessions").join("checkpoints");
        fs::write(checkpoints.join("latest.json"), "{}").expect("write legacy slot");
        fs::write(checkpoints.join("offline_queue.json"), "{}").expect("write offline queue");

        let refs = manager.list_checkpoints().expect("list checkpoints");
        assert_eq!(refs.len(), 2, "offline queue must not be a candidate");
        assert!(
            refs.iter()
                .any(|r| r.source == CheckpointSource::Session(session.metadata.id.clone()))
        );
        assert!(refs.iter().any(|r| r.source == CheckpointSource::Legacy));
    }

    #[test]
    fn legacy_migration_never_overwrites_existing_per_session_checkpoint() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let mut session = create_saved_session(
            &[make_test_message("user", "original")],
            "test-model",
            tmp.path(),
            0,
            None,
        );
        manager.save_checkpoint(&session).expect("save checkpoint");

        session.messages = vec![make_test_message("user", "stale legacy copy")];
        let written = manager
            .write_session_checkpoint_if_absent(&session)
            .expect("migration attempt");
        assert!(!written, "migration must not overwrite an existing file");
        let loaded = manager
            .load_session_checkpoint(&session.metadata.id)
            .expect("load")
            .expect("checkpoint exists");
        assert_eq!(
            loaded.messages,
            vec![make_test_message("user", "original")],
            "existing per-session checkpoint content must be preserved"
        );
    }

    #[test]
    fn workspace_scope_matches_subdirectories_in_same_git_checkout() {
        let tmp = tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        let nested = repo.join("crates").join("tui");
        fs::create_dir_all(&nested).expect("mkdir nested");
        fs::write(repo.join(".git"), "gitdir: .git/worktrees/repo").expect("write git marker");

        assert!(workspace_scope_matches(&repo, &nested));
    }

    #[test]
    fn workspace_scope_rejects_sibling_git_checkouts() {
        let tmp = tempdir().expect("tempdir");
        let first = tmp.path().join("repo-a");
        let second = tmp.path().join("repo-b");
        fs::create_dir_all(&first).expect("mkdir first");
        fs::create_dir_all(&second).expect("mkdir second");
        fs::write(first.join(".git"), "gitdir: .git/worktrees/a").expect("write first marker");
        fs::write(second.join(".git"), "gitdir: .git/worktrees/b").expect("write second marker");

        assert!(!workspace_scope_matches(&first, &second));
    }

    #[test]
    fn test_offline_queue_round_trip_and_clear() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");

        let state = OfflineQueueState {
            messages: vec![QueuedSessionMessage {
                display: "queued message".to_string(),
                skill_instruction: Some("Use skill".to_string()),
                skill_provenance: None,
            }],
            draft: Some(QueuedSessionMessage {
                display: "draft message".to_string(),
                skill_instruction: None,
                skill_provenance: None,
            }),
            ..OfflineQueueState::default()
        };

        manager
            .save_offline_queue_state(&state, Some("test-session"))
            .expect("save queue state");
        let loaded = manager
            .load_offline_queue_state()
            .expect("load queue state")
            .expect("queue state exists");
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0].display, "queued message");
        assert!(loaded.draft.is_some());

        manager
            .clear_offline_queue_state()
            .expect("clear queue state");
        assert!(
            manager
                .load_offline_queue_state()
                .expect("load queue state")
                .is_none()
        );
    }

    #[test]
    fn test_offline_queue_stamps_session_id_on_save() {
        // #487: save_offline_queue_state must stamp the supplied
        // session id so the load path's mismatch check has something
        // to compare against. A queue persisted without a session id
        // is the legacy unscoped form which the load path treats as
        // stale-risky and refuses to restore.
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");

        let state = OfflineQueueState {
            messages: vec![QueuedSessionMessage {
                display: "first parked".to_string(),
                skill_instruction: None,
                skill_provenance: None,
            }],
            ..OfflineQueueState::default()
        };

        manager
            .save_offline_queue_state(&state, Some("session-A"))
            .expect("save with session id");
        let loaded = manager
            .load_offline_queue_state()
            .expect("ok")
            .expect("present");
        assert_eq!(loaded.session_id.as_deref(), Some("session-A"));

        // Re-saving with a different session id replaces the stamp.
        manager
            .save_offline_queue_state(&state, Some("session-B"))
            .expect("re-save");
        let reloaded = manager
            .load_offline_queue_state()
            .expect("ok")
            .expect("present");
        assert_eq!(reloaded.session_id.as_deref(), Some("session-B"));

        // Saving without a session id explicitly (None) clears the
        // stamp — UI's load path treats that as legacy-unscoped and
        // fails closed.
        manager
            .save_offline_queue_state(&state, None)
            .expect("save without session id");
        let unscoped = manager
            .load_offline_queue_state()
            .expect("ok")
            .expect("present");
        assert!(
            unscoped.session_id.is_none(),
            "save with None must persist a missing session_id"
        );
    }

    #[test]
    fn test_session_context_references_round_trip() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let mut session = create_saved_session(
            &[make_test_message("user", "read @src/main.rs")],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
        );
        session.context_references.push(SessionContextReference {
            message_index: 0,
            reference: ContextReference {
                kind: crate::tui::file_mention::ContextReferenceKind::File,
                source: crate::tui::file_mention::ContextReferenceSource::AtMention,
                badge: "file".to_string(),
                label: "src/main.rs".to_string(),
                target: tmp.path().join("src/main.rs").display().to_string(),
                included: true,
                expanded: true,
                detail: Some("included".to_string()),
            },
        });

        let path = manager.save_session(&session).expect("save session");
        let loaded = manager
            .load_session(&session.metadata.id)
            .expect("load session");
        assert!(path.exists());
        assert_eq!(loaded.context_references, session.context_references);
    }

    #[test]
    fn test_checkpoint_rejects_newer_schema() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let checkpoints = tmp.path().join("sessions").join("checkpoints");
        fs::create_dir_all(&checkpoints).expect("create checkpoints dir");
        let path = checkpoints.join("latest.json");
        fs::write(
            &path,
            r#"{
                "schema_version": 999,
                "metadata": {
                    "id": "sid",
                    "title": "bad",
                    "created_at": "2026-01-01T00:00:00Z",
                    "updated_at": "2026-01-01T00:00:00Z",
                    "message_count": 0,
                    "total_tokens": 0,
                    "model": "m",
                    "workspace": "/tmp",
                    "mode": null
                },
                "messages": [],
                "system_prompt": null
            }"#,
        )
        .expect("write checkpoint");

        let err = manager
            .load_legacy_checkpoint()
            .expect_err("should reject schema");
        assert!(err.to_string().contains("newer than supported"));

        // The same guard applies to per-session checkpoint files.
        fs::rename(&path, checkpoints.join("sid.json")).expect("rename to per-session file");
        let err = manager
            .load_session_checkpoint("sid")
            .expect_err("should reject schema");
        assert!(err.to_string().contains("newer than supported"));
    }

    #[test]
    fn test_load_session_rejects_newer_schema() {
        let tmp = tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("new");

        let id = "future-session";
        let path = sessions_dir.join(format!("{id}.json"));
        fs::write(
            &path,
            r#"{
                "schema_version": 999,
                "metadata": {
                    "id": "future-session",
                    "title": "future",
                    "created_at": "2026-01-01T00:00:00Z",
                    "updated_at": "2026-01-01T00:00:00Z",
                    "message_count": 0,
                    "total_tokens": 0,
                    "model": "m",
                    "workspace": "/tmp",
                    "mode": null
                },
                "messages": [],
                "system_prompt": null
            }"#,
        )
        .expect("write session");

        let err = manager.load_session(id).expect_err("should reject schema");
        assert!(
            err.to_string().contains("newer than supported"),
            "unexpected error: {err}"
        );
    }

    /// Regression for #337: metadata extraction skips the (potentially
    /// huge) `messages` array — it must succeed even when the messages
    /// array is megabytes long, and it must NOT confuse a `"metadata"`
    /// substring inside a message body for the real top-level key.
    #[test]
    fn extract_top_level_metadata_skips_huge_messages_array() {
        // Build a session JSON with a large `messages` payload that
        // contains the literal string `"metadata"` in a user message —
        // a naive `find("\"metadata\"")` would mis-target this.
        let big_text = format!(
            r#"this message references "metadata" inside it, repeated:{}"#,
            "x".repeat(20_000)
        );
        let json = format!(
            r#"{{
                "schema_version": 1,
                "metadata": {{
                    "id": "abc-123",
                    "title": "Real Session",
                    "created_at": "2026-01-01T00:00:00Z",
                    "updated_at": "2026-01-02T00:00:00Z",
                    "message_count": 12,
                    "total_tokens": 4096,
                    "model": "deepseek-v4-flash",
                    "workspace": "/tmp"
                }},
                "messages": [
                    {{ "role": "user", "content": [ {{ "Text": {{ "text": {big_text:?} }} }} ] }}
                ]
            }}"#
        );

        let extracted =
            extract_top_level_metadata(json.as_bytes()).expect("metadata extractable from prefix");
        assert_eq!(extracted.id, "abc-123");
        assert_eq!(extracted.title, "Real Session");
        assert_eq!(extracted.message_count, 12);
        assert_eq!(extracted.total_tokens, 4096);
    }

    #[test]
    fn extract_top_level_metadata_handles_braces_inside_strings() {
        // A title containing `{` and `}` inside the metadata block must
        // not throw off the brace counter.
        let json = r#"{
            "metadata": {
                "id": "x",
                "title": "weird { title } with braces",
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "message_count": 0,
                "total_tokens": 0,
                "model": "m",
                "workspace": "/tmp"
            },
            "messages": []
        }"#;
        let extracted = extract_top_level_metadata(json.as_bytes())
            .expect("brace-in-string survives the scanner");
        assert_eq!(extracted.title, "weird { title } with braces");
    }

    #[test]
    fn saved_session_deserializes_without_artifacts_as_empty_registry() {
        let json = r#"{
            "schema_version": 1,
            "metadata": {
                "id": "legacy-session",
                "title": "legacy",
                "created_at": "2026-05-08T00:00:00Z",
                "updated_at": "2026-05-08T00:00:00Z",
                "message_count": 0,
                "total_tokens": 0,
                "model": "deepseek-v4-pro",
                "workspace": "/tmp"
            },
            "messages": [],
            "system_prompt": null
        }"#;

        let session: SavedSession = serde_json::from_str(json).expect("legacy session loads");
        assert!(session.artifacts.is_empty());
        assert!(session.last_auto_route.is_none());
        assert!(session.metadata.parent_session_id.is_none());
        assert!(session.metadata.forked_from_message_count.is_none());
    }

    #[test]
    fn fork_lineage_metadata_round_trips_and_formats() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let parent = create_saved_session(
            &[
                make_test_message("user", "try approach A"),
                make_test_message("assistant", "A looks viable"),
            ],
            "deepseek-v4-pro",
            Path::new("/tmp"),
            42,
            None,
        );
        let mut forked = create_saved_session(
            &parent.messages,
            &parent.metadata.model,
            &parent.metadata.workspace,
            parent.metadata.total_tokens,
            None,
        );
        forked.metadata.mark_forked_from(&parent.metadata);

        manager.save_session(&forked).expect("save fork");
        let loaded = manager
            .load_session(&forked.metadata.id)
            .expect("load fork");

        assert_eq!(
            loaded.metadata.parent_session_id.as_deref(),
            Some(parent.metadata.id.as_str())
        );
        assert_eq!(loaded.metadata.forked_from_message_count, Some(2));
        let line = format_session_line(&loaded.metadata);
        assert!(line.contains("fork"));
        assert!(!line.contains(parent.metadata.id.as_str()));
    }

    #[test]
    fn save_and_load_session_preserves_artifact_metadata() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let mut session = create_saved_session(
            &[make_test_message("user", "run tests")],
            "deepseek-v4-pro",
            Path::new("/tmp"),
            0,
            None,
        );
        session.artifacts.push(crate::artifacts::ArtifactRecord {
            id: "art_call_big".to_string(),
            kind: crate::artifacts::ArtifactKind::ToolOutput,
            session_id: session.metadata.id.clone(),
            tool_call_id: "call-big".to_string(),
            tool_name: "exec_shell".to_string(),
            created_at: Utc::now(),
            byte_size: 512_000,
            preview: "cargo test output".to_string(),
            storage_path: PathBuf::from("/tmp/tool_outputs/call-big.txt"),
        });

        manager.save_session(&session).expect("save");
        let loaded = manager.load_session(&session.metadata.id).expect("load");

        assert_eq!(loaded.artifacts, session.artifacts);
    }

    // ---- #406 prune_sessions_older_than ----
    //
    // The helper is a building block for the auto-archive design: it
    // removes session files older than a threshold while leaving fresh
    // ones (and the checkpoint directory) alone. Tests cover the empty
    // case, the all-fresh case, the all-stale case, and the mixed case.

    fn write_session_with_updated_at(
        manager: &SessionManager,
        id: &str,
        updated_at: DateTime<Utc>,
    ) {
        // Build a minimal SavedSession by hand so the test isn't tied
        // to whatever the helper functions emit; we just need a
        // metadata block whose `updated_at` matches the requested
        // value.
        write_session_record(manager, id, Path::new("/tmp"), updated_at);
    }

    #[test]
    fn prune_sessions_older_than_returns_zero_for_empty_dir() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let pruned = manager
            .prune_sessions_older_than(std::time::Duration::from_secs(3600))
            .expect("prune");
        assert_eq!(pruned, 0);
    }

    #[test]
    fn prune_sessions_older_than_keeps_fresh_records() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        // All updated within the last hour.
        write_session_with_updated_at(
            &manager,
            "fresh-1",
            Utc::now() - chrono::Duration::minutes(30),
        );
        write_session_with_updated_at(
            &manager,
            "fresh-2",
            Utc::now() - chrono::Duration::minutes(5),
        );
        let pruned = manager
            .prune_sessions_older_than(std::time::Duration::from_secs(3600))
            .expect("prune");
        assert_eq!(pruned, 0);
        // Both files still on disk.
        assert_eq!(manager.list_sessions().expect("list").len(), 2);
    }

    #[test]
    fn prune_sessions_older_than_removes_stale_records() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        // Two stale records ≥7 days old.
        write_session_with_updated_at(&manager, "stale-1", Utc::now() - chrono::Duration::days(8));
        write_session_with_updated_at(&manager, "stale-2", Utc::now() - chrono::Duration::days(30));
        let pruned = manager
            .prune_sessions_older_than(std::time::Duration::from_secs(7 * 24 * 3600))
            .expect("prune");
        assert_eq!(pruned, 2);
        assert_eq!(manager.list_sessions().expect("list").len(), 0);
    }

    #[test]
    fn prune_sessions_older_than_only_removes_stale_records_in_mixed_dir() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        write_session_with_updated_at(&manager, "fresh", Utc::now() - chrono::Duration::hours(1));
        write_session_with_updated_at(&manager, "stale", Utc::now() - chrono::Duration::days(60));
        let pruned = manager
            .prune_sessions_older_than(std::time::Duration::from_secs(7 * 24 * 3600))
            .expect("prune");
        assert_eq!(pruned, 1);
        let remaining = manager.list_sessions().expect("list");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "fresh");
    }

    #[test]
    fn prune_sessions_older_than_skips_checkpoint_directory() {
        // The checkpoint subsystem owns `<sessions>/checkpoints/` —
        // prune must not walk into it. The list_sessions iterator
        // already filters to top-level `*.json` files (skipping
        // sub-directories), so this test pins that behaviour.
        let tmp = tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("new");
        let checkpoint_dir = sessions_dir.join("checkpoints");
        fs::create_dir_all(&checkpoint_dir).expect("mkdir checkpoints");
        // Drop a stale-looking JSON inside the checkpoint dir; prune
        // should leave it alone.
        let checkpoint_file = checkpoint_dir.join("latest.json");
        fs::write(&checkpoint_file, "{}").expect("write checkpoint");

        write_session_with_updated_at(&manager, "stale", Utc::now() - chrono::Duration::days(60));
        let pruned = manager
            .prune_sessions_older_than(std::time::Duration::from_secs(7 * 24 * 3600))
            .expect("prune");
        assert_eq!(pruned, 1, "the top-level stale session should be removed");
        assert!(
            checkpoint_file.exists(),
            "checkpoint file should be untouched"
        );
    }

    #[test]
    fn test_load_offline_queue_rejects_newer_schema() {
        let tmp = tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("new");
        let checkpoints = sessions_dir.join("checkpoints");
        fs::create_dir_all(&checkpoints).expect("create checkpoints dir");
        let path = checkpoints.join("offline_queue.json");
        fs::write(
            &path,
            r#"{
                "schema_version": 999,
                "messages": [],
                "draft": null
            }"#,
        )
        .expect("write queue");

        let err = manager
            .load_offline_queue_state()
            .expect_err("should reject schema");
        assert!(
            err.to_string().contains("newer than supported"),
            "unexpected error: {err}"
        );
    }
}
