//! Sub-agent spawning system.
//!
//! Provides tools to spawn background sub-agents, query their status,
//! and retrieve results. Sub-agents run with a filtered toolset and
//! inherit the workspace configuration from the main session.
//!
//! The model-facing creation surface is the `agent` tool. Narrow coordination
//! tools (`agents/list`, `agents/message`, `agents/followup`,
//! `agents/interrupt`, `agents/coordinate`, `agents/wait`) are retired from
//! the model catalog (#5462) — still registered and executable by name so
//! persisted transcripts replay, never advertised — and wrap the same runtime without restoring
//! the retired lifecycle theater. Older manager helpers remain executable for
//! persisted records and internal recovery.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock, Semaphore};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::client::DeepSeekClient;
use crate::config::MAX_SUBAGENTS;
use crate::core::engine::tool_catalog::{
    TOOL_SEARCH_NAME, active_tools_for_request, apply_native_tool_deferral,
    ensure_advanced_tooling, execute_tool_search_with_cache, initial_active_tools,
    is_tool_search_tool, remove_evicted_cache_activations, tool_matches_any_rule,
    touch_cached_tool_after_execution,
};
use crate::core::events::{AgentProgressEventMeta, Event};
use crate::core::session::ToolActivationCache;
use crate::dependencies::{ExternalTool, Git};
/// Compatibility re-export: the closed role set lives in
/// [`crate::fleet::role`], the lightweight role surface every spawn path
/// consumes. Existing `tools::subagent::FleetRole` paths keep resolving.
pub use crate::fleet::role::FleetRole;
use crate::fleet::role::{
    FLEET_ROLE_SCHEMA_VALUES, NETWORK_DENIAL_SENTINEL, SHELL_AUTHORITY_SENTINEL,
    VALID_ROLE_ALIASES, is_posture_denial, migrate_legacy_role_token, public_role_label,
};
use crate::llm_client::{LlmClient, LlmError};
use crate::models::{
    ContentBlock, Message, MessageRequest, MessageResponse, SystemPrompt, Tool, Usage,
    is_incomplete_stop_reason, is_output_limit_stop_reason, stop_reason_detail,
};
use crate::request_tuning::RequestTuning;
use crate::tools::canonical_action::{
    CANONICAL_ACTION_ALIASES, canonical_action_alias, is_action_family,
};
use crate::tools::handle::VarHandle;
use crate::tools::plan::{PlanState, SharedPlanState};
use crate::tools::registry::{AgentToolSurfaceOptions, ToolRegistry, ToolRegistryBuilder};
use crate::tools::shell::SharedShellManager;
use crate::tools::spec::{
    ApprovalRequirement, RichToolResult, ToolCapability, ToolContext, ToolError, ToolResult,
    ToolSpec,
};
use crate::tools::todo::SharedTodoList;
#[cfg(test)]
use crate::tools::todo::TodoList;
use crate::tui::app::AppMode;
use crate::tui::app::ReasoningEffort;
use crate::utils::spawn_supervised;
use crate::work_graph::{
    EvidenceKind, EvidenceRef, OperationIntent, OperationOwnerSnapshot, OwnerState,
    SharedWorkRuntime,
};
use crate::worker_profile::{
    ChildLaunchManifest, ModelRoute, ShellPolicy, ToolScope, WorkerRuntimeProfile,
};
use coord::{
    CoordinationDetailMetrics, CoordinationHotPath, CoordinationLedger, DecisionRecord,
    DecisionStatus, PersistedWriteClaim, ReconciliationReceipt, WriteScopeClaim,
};

pub mod advisor;
pub mod coord;
pub mod mailbox;
mod naming;
mod worktree;

use worktree::{SubAgentWorktreeRequest, prepare_child_workspace};
#[cfg(test)]
use worktree::{create_isolated_worktree, git_repo_root};

use crate::models::Role;
#[allow(unused_imports)] // re-exported for hosts / tests; registration uses concrete types
pub use advisor::{
    AdvisorConfig, EmissionGuard, ToolCallPair, build_advisor_prompt, extract_tool_call_pairs,
    run_advisor_for_turn,
};
#[allow(unused_imports)] // re-exported for hosts / tests; registration uses concrete types
pub use coord::{
    AgentsCoordinateTool, AgentsFollowupTool, AgentsInterruptTool, AgentsListTool,
    AgentsMessageTool, AgentsWaitTool, CoordinationDetailProjection, register_coordination_tools,
};
#[allow(unused_imports)]
pub use mailbox::{Mailbox, MailboxEnvelope, MailboxMessage, MailboxReceiver};
use naming::generated_whale_name_base;
pub(crate) use naming::localized_whale_display_names;
#[allow(unused_imports)] // compatibility path; some consumers exist only in test builds today
pub use naming::{
    WHALE_NICKNAMES, assign_unique_whale_name_in_locale, whale_name_for_id_in_locale,
};
#[cfg(test)]
use naming::{
    WHALE_NICKNAMES_CA, WHALE_NICKNAMES_DE, WHALE_NICKNAMES_ES_419, WHALE_NICKNAMES_FR,
    WHALE_NICKNAMES_HI, WHALE_NICKNAMES_ID, WHALE_NICKNAMES_JA, WHALE_NICKNAMES_KO,
    WHALE_NICKNAMES_PT_BR, WHALE_NICKNAMES_RU, WHALE_NICKNAMES_UK, WHALE_NICKNAMES_VI,
    WHALE_NICKNAMES_ZH_HANT,
};

// === Constants ===

/// Global ownership table for cache-aware resident file sub-agents (#529).
/// Maps file path → agent id. Agents hold a lease on a file while running;
/// the lease is released when the agent reaches a terminal state.
static RESIDENT_LEASES: std::sync::OnceLock<
    parking_lot::Mutex<std::collections::HashMap<String, String>>,
> = std::sync::OnceLock::new();
const MAX_RESIDENT_CONTEXT_BYTES: u64 = 64 * 1024;

/// Release all resident file leases held by `agent_id`. Called when an
/// agent transitions to a terminal state (completed, failed, cancelled).
fn release_resident_leases_for(agent_id: &str) {
    if let Some(lock) = RESIDENT_LEASES.get() {
        let mut guard = lock.lock();
        guard.retain(|_, owner| owner != agent_id);
    }
}

fn reserve_resident_lease(lease_key: &str, display_path: &str) -> Result<(), ToolError> {
    let leases = RESIDENT_LEASES.get_or_init(|| parking_lot::Mutex::new(HashMap::new()));
    let mut guard = leases.lock();
    if let Some(owner) = guard.get(lease_key) {
        return Err(ToolError::invalid_input(format!(
            "resident_file '{display_path}' is already leased by agent {owner}"
        )));
    }
    guard.insert(lease_key.to_string(), "pending".to_string());
    Ok(())
}

fn rollback_pending_resident_lease(file_path: &str) {
    if let Some(leases) = RESIDENT_LEASES.get() {
        let mut guard = leases.lock();
        if guard.get(file_path).is_some_and(|owner| owner == "pending") {
            guard.remove(file_path);
        }
    }
}

fn commit_resident_lease(file_path: &str, agent_id: &str) {
    if let Some(leases) = RESIDENT_LEASES.get() {
        let mut guard = leases.lock();
        if let Some(owner) = guard.get_mut(file_path)
            && owner == "pending"
        {
            *owner = agent_id.to_string();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResidentContext {
    display_path: String,
    lease_key: String,
    contents: String,
}

fn read_bounded_resident_context(
    context: &ToolContext,
    raw_path: &str,
) -> Result<ResidentContext, ToolError> {
    let path = crate::tools::spec::resolve_strict_authority_path(context, raw_path)?;
    let metadata = std::fs::metadata(&path).map_err(|error| {
        ToolError::invalid_input(format!(
            "resident_file '{}' is not a readable workspace file: {error}",
            raw_path
        ))
    })?;
    if !metadata.is_file() {
        return Err(ToolError::invalid_input(format!(
            "resident_file '{}' must name one regular workspace file",
            raw_path
        )));
    }
    if metadata.len() > MAX_RESIDENT_CONTEXT_BYTES {
        return Err(ToolError::invalid_input(format!(
            "resident_file '{}' is {} bytes; the bounded context limit is {} bytes",
            raw_path,
            metadata.len(),
            MAX_RESIDENT_CONTEXT_BYTES
        )));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .and_then(|file| {
            file.take(MAX_RESIDENT_CONTEXT_BYTES.saturating_add(1))
                .read_to_end(&mut bytes)
        })
        .map_err(|error| {
            ToolError::invalid_input(format!(
                "resident_file '{}' could not be read: {error}",
                raw_path
            ))
        })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RESIDENT_CONTEXT_BYTES {
        return Err(ToolError::invalid_input(format!(
            "resident_file '{}' grew beyond the bounded {} byte context limit",
            raw_path, MAX_RESIDENT_CONTEXT_BYTES
        )));
    }
    let contents = String::from_utf8(bytes).map_err(|_| {
        ToolError::invalid_input(format!(
            "resident_file '{}' must contain UTF-8 text",
            raw_path
        ))
    })?;
    let workspace = context.workspace.canonicalize().map_err(|error| {
        ToolError::execution_failed(format!(
            "Failed to canonicalize resident workspace {}: {error}",
            context.workspace.display()
        ))
    })?;
    let relative = path.strip_prefix(&workspace).map_err(|_| {
        ToolError::permission_denied(format!(
            "resident_file escapes workspace: {}",
            path.display()
        ))
    })?;
    let display_path =
        normalize_claim_path(&relative.to_string_lossy()).map_err(ToolError::permission_denied)?;
    Ok(ResidentContext {
        display_path,
        // The lease table is process-wide, so a repo-relative path alone
        // would falsely collide across unrelated workspaces. The authority
        // resolver returned a canonical in-workspace file; use that exact
        // identity internally while keeping only the relative label visible.
        lease_key: path.to_string_lossy().into_owned(),
        contents,
    })
}

/// Positive child model-turn budgets are clamped to this hard ceiling. Zero is
/// the unbounded sentinel used by the default agent loop.
const MAX_SUBAGENT_STEPS: u32 = 2_000;
/// Default wall-clock budget for one child run, including model and tool work.
const DEFAULT_CHILD_WALL_TIME: Duration = Duration::from_secs(30 * 60);
const MAX_CHILD_WALL_TIME: Duration = Duration::from_secs(24 * 60 * 60);
/// Default wall-clock budget for a single sub-agent tool execution. The active
/// value travels on `SubAgentRuntime::tool_timeout` so a long-but-legitimate
/// tool (a large build, a slow shell command, a deep search) is not killed
/// mid-flight. Kept non-zero so `timeout(Duration::ZERO, ...)` can never fire
/// immediately. The per-step API timeout, streaming watchdogs, and heartbeat
/// floors remain the independent stall detectors. Derived from the shared
/// `DEFAULT_SUBAGENT_TOOL_TIMEOUT_SECS` so the heartbeat floor (which must sit
/// above this timeout, see `resolve_subagent_heartbeat_timeout_secs`) can never
/// drift from the value actually applied to a running tool.
const DEFAULT_TOOL_TIMEOUT: Duration =
    Duration::from_secs(crate::config::DEFAULT_SUBAGENT_TOOL_TIMEOUT_SECS);
const MIN_SUBAGENT_SPAWN_TOKEN_RESERVE: u64 = 1;
const MIN_EVENT_CHANNEL_HEADROOM_FOR_ROUTINE_PROGRESS: usize = 32;

/// Format a step counter for sub-agent progress messages.
///
fn format_step_counter(steps: u32, max_steps: u32) -> String {
    if max_steps == 0 {
        format!("step {steps}")
    } else {
        format!("step {steps}/{max_steps}")
    }
}

fn resolve_max_steps(role: FleetRole, explicit: Option<u32>, configured: Option<u32>) -> u32 {
    explicit
        .unwrap_or_else(|| {
            configured.unwrap_or_else(|| WorkerRuntimeProfile::default_max_steps(role))
        })
        .min(MAX_SUBAGENT_STEPS)
}

fn child_wall_time_exhausted_reason(limit: Duration) -> String {
    format!(
        "child wall-time budget exhausted (limit: {}s); raise it with wall_time_secs or split the work into smaller independent tasks",
        limit.as_secs()
    )
}
// Non-streaming sub-agents need enough response budget to carry large tool-call
// arguments, especially write_file content. The API bills generated tokens, not
// the requested ceiling.
const SUBAGENT_TRANSIENT_PROVIDER_MAX_RETRIES: u32 = 2;
const SUBAGENT_TRANSIENT_PROVIDER_INITIAL_BACKOFF: Duration = Duration::from_millis(250);
/// Per-step API-call timeout retry budget. A `create_message` call that
/// exceeds `step_api_timeout` is re-issued up to this many times (with
/// exponential backoff) before the step is interrupted with a preserved
/// checkpoint. Dogfooding showed a single 120s stall wiping out every child
/// in a fan-out one by one even though the provider call was live-but-slow,
/// so a timeout now gets the same retry dignity as a transient provider
/// error (kimi-code comparison, FINISH-0.9.4 entry #40).
const SUBAGENT_API_TIMEOUT_MAX_RETRIES: u32 = 5;
/// Initial backoff for a timed-out per-step API call; doubles per retry and
/// is capped at [`SUBAGENT_API_TIMEOUT_MAX_BACKOFF`].
const SUBAGENT_API_TIMEOUT_INITIAL_BACKOFF: Duration = Duration::from_secs(1);
/// Cap for the per-step API-call timeout backoff.
const SUBAGENT_API_TIMEOUT_MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Jitter applied to the timeout backoff (0.2 = ±20%) so a fan-out of
/// children that all time out together does not re-fire in lockstep.
const SUBAGENT_API_TIMEOUT_BACKOFF_JITTER_FACTOR: f64 = 0.2;
/// Per-step LLM API call timeout. Each `create_message` request must complete
/// within this window or the attempt is treated as timed out (and retried up
/// to [`SUBAGENT_API_TIMEOUT_MAX_RETRIES`] times before interrupting the
/// step). Prevents a single stuck API call from blocking the sub-agent
/// indefinitely.
/// Legacy fallback for the per-step DeepSeek API timeout. The active timeout
/// now travels on `SubAgentRuntime::step_api_timeout` so users can override
/// it via `[subagents] api_timeout_secs` in `~/.codewhale/config.toml`. The
/// constant only exists for tests/stub runtimes that need a hard-coded
/// default; production runtimes set the field explicitly (#1806, #1808).
const DEFAULT_STEP_API_TIMEOUT: Duration =
    Duration::from_secs(crate::config::DEFAULT_SUBAGENT_API_TIMEOUT_SECS);
const COMPLETED_AGENT_RETENTION: Duration = Duration::from_secs(60 * 60);
const MAX_AGENT_WORKER_RECORDS: usize = 256;
const MAX_AGENT_WORKER_EVENTS_PER_RECORD: usize = 128;
/// Byte budget for the message tail retained in a [`SubAgentCheckpoint`]
/// (#3882). Checkpoints fire on every step of every worker and are cloned
/// into snapshots, projections, and `subagents.v1.json`; an unbounded
/// `messages` clone turns one large tool output into many resident copies
/// under Fleet fanout. The checkpoint keeps the most recent messages within
/// this budget (always at least the last one, so continuability is
/// preserved) and records how many older messages were omitted. Full tool
/// outputs remain recoverable from the spillover files on disk.
const SUBAGENT_CHECKPOINT_MESSAGE_BUDGET_BYTES: usize = 256 * 1024;
/// Byte budget for the message tail embedded in a `subagent_full_transcript`
/// handle (#3882). One handle is retained in memory per agent; the payload
/// keeps a bounded tail plus the true `message_count` so inspection stays
/// useful without pinning a whole unbounded transcript in RAM.
const SUBAGENT_TRANSCRIPT_MESSAGE_BUDGET_BYTES: usize = 1024 * 1024;
const SUBAGENT_TRANSCRIPT_ARTIFACT_SCHEMA_VERSION: u32 = 1;
const SUBAGENT_TRANSCRIPT_ARTIFACT_DIR: &str = "subagent-transcripts";
const SUBAGENT_STATE_SCHEMA_VERSION: u32 = 1;
const SUBAGENT_STATE_FILE: &str = "subagents.v1.json";
const SUBAGENT_STATE_LOCK_FILE: &str = "subagents.v1.lock";
const SUBAGENT_RESTART_REASON: &str = "Interrupted by process restart";
const SUBAGENT_SESSION_CLOSED_REASON: &str = "Interrupted: parent session closed";
#[cfg(test)]
const SUBAGENT_MODEL_WAIT_REASON: &str = "waiting for model response";
const SUBAGENT_QUEUED_LAUNCH_REASON: &str = "queued: waiting for a sub-agent launch slot";
/// #freeze: minimum spacing between hot-path (per-step checkpoint) state
/// persists. `update_checkpoint` fires on every step of every agent; at high
/// fanout an unconditional full-fleet rewrite under the manager write lock
/// wedges the UI. Hot-path writes coalesce to at most one per this interval;
/// terminal/structural changes still persist immediately, and any terminal
/// write flushes the full in-memory fleet (including other agents' pending
/// checkpoints) to disk.
const SUBAGENT_PERSIST_DEBOUNCE: Duration = Duration::from_millis(1500);

/// #3803: minimum interval between write-locked `cleanup` runs triggered by the
/// sidebar refresh (`Op::ListSubAgents`). Cleanup auto-cancels stale agents
/// (heartbeat timeout, default 300s) and drops old finished records, so a 2s
/// floor keeps it responsive while preventing per-refresh write-lock contention
/// during a high-fanout burst.
pub const SUBAGENT_LIST_CLEANUP_MIN_INTERVAL: Duration = Duration::from_secs(2);

/// #freeze: lightweight perf counters for the sub-agent persist hot path,
/// gated behind `CODEWHALE_SUBAGENT_PERF_TRACE=1`. The atomic increments are
/// always cheap; only the structured `subagent_perf` log line is gated.
static SUBAGENT_PERSIST_WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SUBAGENT_PERSIST_SKIPPED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn subagent_perf_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("CODEWHALE_SUBAGENT_PERF_TRACE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

const VALID_SUBAGENT_TYPES: &str = "general, explore, planner, reviewer, implement, test, advisor, custom \
     (legacy aliases remain accepted: worker, scout, builder, verifier, consultant, general-purpose, general_purpose, default, exploration/explorer, plan/planning/awaiter, review/code-review/code_review, implementer/implementation, verify/verification/validator/tester, oracle)";
const SUBAGENT_TYPE_DESCRIPTION: &str = "Fleet role for this delegated worker. general: full tool access for multi-step tasks. explore: fast read-only exploration. planner: grounded strategy with read-only probes. reviewer: reads and grades code. implement: lands focused code changes. test: runs tests/validation gates and reports evidence. advisor: read-only high-reasoning counsel for judgement calls and design critique. custom: the tools listed in allowed_tools on the parent's posture. Legacy aliases remain accepted at deserialization boundaries.";

// === Types ===

/// Assignment metadata for sub-agent orchestration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubAgentAssignment {
    pub objective: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

impl SubAgentAssignment {
    fn new(objective: String, role: Option<String>) -> Self {
        Self { objective, role }
    }
}

/// Role presentation: system prompts and config key lookup. The role itself
/// ([`FleetRole`], parsing, posture) lives in [`crate::fleet::role`]; this
/// impl stays on the agent tool because it renders prompt text the tool owns.
impl FleetRole {
    /// Pre-Fleet model-override key (`explorer_model` / `ni_model` tables).
    /// Not used for receipts or UI — only config key lookup.
    #[must_use]
    fn legacy_type_name(&self) -> &'static str {
        match self {
            Self::Worker => "general",
            Self::Scout => "explore",
            Self::Planner => "plan",
            Self::Reviewer => "review",
            Self::Builder => "implementer",
            Self::Verifier => "verifier",
            // Consultant is post-Fleet; it never had a pre-Fleet override table key.
            Self::Consultant => "consultant",
            Self::Custom => "custom",
        }
    }

    /// Get the system prompt for this Fleet role.
    #[must_use]
    pub fn system_prompt(&self) -> String {
        let role_intro = match self {
            Self::Worker => GENERAL_AGENT_INTRO,
            Self::Scout => EXPLORE_AGENT_INTRO,
            Self::Planner => PLAN_AGENT_INTRO,
            Self::Reviewer => REVIEW_AGENT_INTRO,
            Self::Builder => IMPLEMENTER_AGENT_INTRO,
            Self::Verifier => VERIFIER_AGENT_INTRO,
            Self::Consultant => CONSULTANT_AGENT_INTRO,
            Self::Custom => CUSTOM_AGENT_INTRO,
        };
        match self {
            Self::Scout => format!(
                "{role_intro}{}",
                crate::prompts::text::SUBAGENT_SCOUT_OUTPUT_FORMAT
            ),
            _ => format!("{role_intro}{SUBAGENT_OUTPUT_FORMAT}"),
        }
    }
}

/// Status of a sub-agent execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SubAgentStatus {
    Running,
    Completed,
    Interrupted(String),
    Failed(String),
    Cancelled,
    /// Worker stopped because it exceeded its own per-worker token budget.
    /// Distinct from the scope-level admission gate (#3319): this caps a
    /// single runaway worker mid-run, while the scope gate bounds total
    /// fan-out across a root run and its descendants.
    BudgetExhausted,
}

/// Structured reason a non-running sub-agent needs parent action.
///
/// This is intentionally separate from `SubAgentStatus`: legacy surfaces keep
/// seeing `Interrupted`, while parent-visible projections get a concrete
/// question/action instead of a parked child task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubAgentNeedsInput {
    pub question: String,
}

/// Snapshot of sub-agent state for tool results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentResult {
    pub name: String,
    pub agent_id: String,
    pub context_mode: String,
    pub fork_context: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    pub agent_type: FleetRole,
    pub assignment: SubAgentAssignment,
    #[serde(default)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    pub status: SubAgentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_status: Option<AgentWorkerStatus>,
    /// Effective non-secret runtime posture for Fleet-backed workers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_permissions: Option<codewhale_protocol::fleet::FleetEffectivePermissions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    #[serde(default)]
    pub spawn_depth: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_route: Option<ChildRouteReceipt>,
    pub result: Option<String>,
    pub steps_taken: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<SubAgentCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_input: Option<SubAgentNeedsInput>,
    pub duration_ms: u64,
    /// Live start timestamp for elapsed derivation at render (4b). The
    /// `duration_ms` above is a frozen snapshot; this `Instant` is the
    /// source of truth for ticking rows. `None` for deserialized or
    /// non-running agents, `Some` for live running children. Skipped in
    /// serialization so persisted agents remain correct.
    #[serde(skip)]
    pub started_at: Option<std::time::Instant>,
    /// `true` when this agent was loaded from a prior-session persisted
    /// state file rather than spawned in the current session (#405).
    /// Lets listings filter out historical noise by default while
    /// keeping the records reachable via `include_archived=true`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub from_prior_session: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChildRouteReceipt {
    pub requested_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_profile: Option<String>,
    pub resolved_profile_id: Option<String>,
    pub profile_origin: Option<String>,
    pub canonical_role: String,
    pub provider_id: String,
    pub model_id: String,
    pub route_source: String,
    pub requested_reasoning: String,
    pub effective_reasoning: Option<String>,
    pub runtime_version: String,
    pub runtime_build_sha: String,
}
struct RequestedChildRoute {
    requested_type: String,
    requested_profile: Option<String>,
    requested_reasoning: String,
}
/// Headless worker lifecycle states for sub-agent execution.
///
/// This is the TUI-independent state machine that future CLI/API/workflow
/// surfaces should consume. The legacy `SubAgentStatus` remains the
/// compatibility projection returned by sub-agent runs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentWorkerStatus {
    Queued,
    Starting,
    Running,
    WaitingForUser,
    ModelWait,
    RunningTool,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl AgentWorkerStatus {
    /// Terminal worker statuses may be age-evicted from the run ledger (#4217).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

/// Tool capability profile requested for a headless worker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentWorkerToolProfile {
    /// Inherit the parent runtime registry for compatibility.
    Inherited,
    /// Use the listed tools only.
    Explicit(Vec<String>),
}

/// Declarative headless worker request derived from `agent`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentWorkerSpec {
    pub worker_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    pub objective: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub agent_type: FleetRole,
    pub model: String,
    pub workspace: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    pub context_mode: String,
    pub fork_context: bool,
    pub tool_profile: AgentWorkerToolProfile,
    #[serde(default)]
    pub runtime_profile: WorkerRuntimeProfile,
    pub max_steps: u32,
    pub spawn_depth: u32,
    pub max_spawn_depth: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_route: Option<ChildRouteReceipt>,
    /// #414 launch authority and #4647 write contract, persisted as one record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_manifest: Option<ChildLaunchManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunFollowUpDelivery {
    pub delivered: bool,
    pub timestamp_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub interrupt: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub continued_from_checkpoint: bool,
}

/// Parent → child mail queued by `agents/message` / `agents/followup`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedParentMessage {
    pub text: String,
    pub queued_at_ms: u64,
    /// When true, delivery should also attempt a live wake (`followup`).
    pub wake: bool,
}

/// Receipt returned by queue / followup coordination helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentMailReceipt {
    pub agent_id: String,
    pub status: String,
    pub queue_depth: usize,
    pub woke: bool,
    pub continued_from_checkpoint: bool,
    /// Present when the child is interrupted_continuable and still has a
    /// checkpoint handle the parent can re-dispatch with. Live in-place
    /// resume from `agents/followup` is not automated yet.
    pub continuation_handle: Option<String>,
    pub note: String,
}

/// Compact coordination projection for `agents/list`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentCoordSummary {
    pub agent_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_route: Option<ChildRouteReceipt>,
    pub status: String,
    pub steps_taken: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_spent_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_remaining_tokens: Option<u64>,
    #[serde(default)]
    pub recent_progress: Vec<String>,
    #[serde(default)]
    pub queued_mail: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    #[serde(default)]
    pub continuable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_claim: Option<PersistedWriteClaim>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_decisions: Vec<DecisionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunFollowUpTarget {
    #[serde(default = "default_agent_inspect_tool")]
    pub tool: String,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    #[serde(default)]
    pub accepted_statuses: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_delivery: Option<AgentRunFollowUpDelivery>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunTakeoverTarget {
    #[serde(default = "default_subagent_takeover_kind")]
    pub kind: String,
    #[serde(default)]
    pub supported: bool,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    pub instructions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsupported_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunArtifactRef {
    pub kind: String,
    pub name: String,
    pub target: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunUsage {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// Priced USD subtotal from the immutable per-response route audits, in
    /// microdollars. Absent means the worker has no authoritative USD receipt;
    /// it never means the worker was free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_microusd: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_spent_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_remaining_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_scope: Option<String>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunVerificationSummary {
    pub status: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunRecommendedAction {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub reason: String,
}

/// Structured headless worker event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentWorkerEvent {
    pub seq: u64,
    pub worker_id: String,
    pub status: AgentWorkerStatus,
    pub timestamp_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

/// Canonical headless worker record retained by `SubAgentManager`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentWorkerRecord {
    pub spec: AgentWorkerSpec,
    /// Root conversation that admitted this worker. Persisted independently
    /// of the optional paired `SubAgent` row so headless workers remain
    /// session-addressable without being guessed into a later conversation.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner_session_id: String,
    #[serde(default = "default_subagent_actor_kind")]
    pub actor_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    #[serde(default = "default_agent_run_follow_up")]
    pub follow_up: AgentRunFollowUpTarget,
    #[serde(default = "default_agent_run_takeover")]
    pub takeover: AgentRunTakeoverTarget,
    #[serde(default)]
    pub artifacts: Vec<AgentRunArtifactRef>,
    #[serde(default = "default_agent_run_usage")]
    pub usage: AgentRunUsage,
    /// Redacted provider-response identities already folded into `usage`.
    /// Persisting the same fingerprint as the session cost snapshot makes a
    /// retried delivery idempotent in both projections after reload.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub usage_source_fingerprints: BTreeSet<String>,
    #[serde(default = "default_agent_run_verification")]
    pub verification: AgentRunVerificationSummary,
    #[serde(default = "default_agent_run_recommended_action")]
    pub recommended_action: AgentRunRecommendedAction,
    pub status: AgentWorkerStatus,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub steps_taken: u32,
    #[serde(default)]
    pub events: VecDeque<AgentWorkerEvent>,
}

#[derive(Clone)]
pub(crate) struct CoordinationRegistrationSnapshot {
    worker_records: HashMap<String, AgentWorkerRecord>,
    coordination: CoordinationLedger,
}

impl AgentWorkerRecord {
    /// Build a record exactly as the manager does. `pub(crate)` so the agents
    /// roster projection (#5479) can be tested against real records instead of
    /// a hand-rolled struct literal that would drift from this one.
    #[cfg(test)]
    pub(crate) fn new(spec: AgentWorkerSpec, now_ms: u64) -> Self {
        Self::new_for_session(spec, now_ms, String::new())
    }

    fn new_for_session(spec: AgentWorkerSpec, now_ms: u64, owner_session_id: String) -> Self {
        let run_id = agent_worker_run_id(&spec);
        let artifacts = default_subagent_artifacts(&run_id);
        let follow_up = follow_up_target_for_spec(&spec);
        let takeover = takeover_target_for_spec(&spec);
        let recommended_action =
            recommended_action_for_worker_status(AgentWorkerStatus::Starting, &spec);
        Self {
            parent_run_id: spec.parent_run_id.clone(),
            spec,
            owner_session_id,
            actor_kind: default_subagent_actor_kind(),
            follow_up,
            takeover,
            artifacts,
            usage: default_agent_run_usage(),
            usage_source_fingerprints: BTreeSet::new(),
            verification: default_agent_run_verification(),
            recommended_action,
            status: AgentWorkerStatus::Starting,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            started_at_ms: None,
            completed_at_ms: None,
            latest_message: None,
            result_summary: None,
            error: None,
            steps_taken: 0,
            events: VecDeque::new(),
        }
    }
}

fn default_subagent_actor_kind() -> String {
    "subagent".to_string()
}

fn default_agent_inspect_tool() -> String {
    "handle_read".to_string()
}

fn default_subagent_takeover_kind() -> String {
    "local_subagent_session".to_string()
}

fn default_agent_run_follow_up() -> AgentRunFollowUpTarget {
    AgentRunFollowUpTarget {
        tool: default_agent_inspect_tool(),
        agent_id: String::new(),
        session_name: None,
        accepted_statuses: vec!["running".to_string(), "interrupted_continuable".to_string()],
        latest_delivery: None,
    }
}

fn default_agent_run_takeover() -> AgentRunTakeoverTarget {
    AgentRunTakeoverTarget {
        kind: default_subagent_takeover_kind(),
        supported: false,
        agent_id: String::new(),
        session_name: None,
        instructions: "No takeover target is available for this older record.".to_string(),
        unsupported_reason: Some("legacy_record_missing_agent_id".to_string()),
    }
}

fn default_agent_run_usage() -> AgentRunUsage {
    AgentRunUsage {
        status: "unknown".to_string(),
        input_tokens: None,
        output_tokens: None,
        total_tokens: None,
        cost_microusd: None,
        token_budget: None,
        budget_spent_tokens: None,
        budget_remaining_tokens: None,
        budget_scope: None,
        note: "Token usage is not yet reported by the sub-agent worker ledger.".to_string(),
    }
}

fn positive_token_budget(budget: Option<u64>) -> Option<u64> {
    budget.filter(|value| *value > 0)
}

fn usage_total_tokens(usage: &Usage) -> u64 {
    u64::from(usage.input_tokens).saturating_add(u64::from(usage.output_tokens))
}

/// Convert an authoritative USD audit into the workflow IR's integer
/// microdollar receipt. Route coverage stays on the cost-status path; this
/// narrow projection deliberately preserves only a priced subtotal.
fn priced_usd_microusd(audit: &crate::pricing::TurnCostAudit) -> Option<u64> {
    if !audit.usd_priced {
        return None;
    }
    let usd = audit.estimate?.usd;
    let microusd = usd * 1_000_000.0;
    if !microusd.is_finite() || !(0.0..=(u64::MAX as f64)).contains(&microusd) {
        return None;
    }
    Some(microusd.round() as u64)
}

/// Publish one child provider response into every projection that owns it.
/// The stable source id is the shared exactly-once key: runtime/session cost
/// and the durable worker record hash it with the same canonical function.
async fn record_provider_response_usage(
    runtime: &SubAgentRuntime,
    agent_id: &str,
    source_id: &str,
    route: crate::cost_status::EffectiveRouteEnvelope,
    usage: &Usage,
) {
    let priced_cost_microusd = priced_usd_microusd(&route.audit(usage));
    if let Some(lease) = runtime.runtime_usage_lease.as_ref() {
        crate::cost_status::report_effective_route_for_runtime(
            crate::cost_status::scope_token(),
            Some(lease.owner()),
            source_id,
            &route,
            usage,
        );
    }
    if let Some(mailbox) = runtime.mailbox.as_ref() {
        let _ = mailbox.send(MailboxMessage::token_usage(
            agent_id,
            source_id,
            route,
            usage.clone(),
        ));
    }
    runtime.manager.write().await.record_worker_usage(
        agent_id,
        source_id,
        usage,
        priced_cost_microusd,
    );
}

fn refresh_usage_note(usage: &mut AgentRunUsage) {
    let worker_total = usage.total_tokens.unwrap_or(0);
    if let Some(limit) = usage.token_budget {
        let spent = usage.budget_spent_tokens.unwrap_or(worker_total);
        let remaining = usage
            .budget_remaining_tokens
            .unwrap_or_else(|| limit.saturating_sub(spent));
        usage.status = if remaining == 0 {
            "budget_exhausted".to_string()
        } else if worker_total > 0 {
            "reported".to_string()
        } else {
            "tracking".to_string()
        };
        usage.note = if worker_total > 0 {
            format!(
                "Token budget: {spent}/{limit} spent, {remaining} remaining. This worker reported {worker_total} tokens."
            )
        } else {
            format!("Token budget: {spent}/{limit} spent, {remaining} remaining.")
        };
    } else if worker_total > 0 {
        usage.status = "reported".to_string();
        usage.note = format!("Provider reported {worker_total} tokens for this worker.");
    } else if usage.status.is_empty() {
        *usage = default_agent_run_usage();
    }
}

fn default_agent_run_verification() -> AgentRunVerificationSummary {
    AgentRunVerificationSummary {
        status: "self_report_only".to_string(),
        summary:
            "No verified command or test receipt is attached; treat the result summary as a child self-report."
                .to_string(),
    }
}

/// Compare a completed child's claimed changed-files against `git status`
/// in its workspace (R7, finish-operator 2026-08-02). The morning report
/// caught a child claiming edits git had never seen — by hand. Extraction
/// is deliberately conservative to keep taint high-signal: only path-like
/// tokens on a line that also carries a change verb count as claims, and a
/// claim is a mismatch only when git shows the path untouched. Returns
/// `None` when there is nothing to dispute (no git, no claims, all claims
/// visible in the status).
fn claimed_diff_taint(
    summary: &str,
    workspace: &Path,
    worker_started_at_ms: Option<u64>,
) -> Option<AgentRunVerificationSummary> {
    const CHANGE_VERBS: [&str; 14] = [
        "changed",
        "modified",
        "updated",
        "edited",
        "wrote",
        "rewrote",
        "created",
        "added",
        "deleted",
        "removed",
        "renamed",
        "fixed",
        "patched",
        "implemented",
    ];

    let status_output = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["status", "--porcelain"])
        // Read-only probe: never take the index lock in the user's repo
        // (#5617).
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    if !status_output.status.success() {
        return None;
    }
    let mut dirty: std::collections::HashSet<String> =
        String::from_utf8_lossy(&status_output.stdout)
            .lines()
            .filter_map(|line| {
                let entry = line.get(3..)?.trim();
                // Renames report "old -> new"; the new path is the claimable one.
                let path = entry.rsplit(" -> ").next().unwrap_or(entry);
                Some(path.trim_matches('"').to_string())
            })
            .collect();
    // A child that committed its work leaves git status clean; files changed
    // by commits made after the worker started are visible claims too.
    if let Some(started_ms) = worker_started_at_ms
        && let Some(since) =
            chrono::DateTime::from_timestamp_millis(i64::try_from(started_ms).unwrap_or(i64::MAX))
        && let Ok(log_output) = std::process::Command::new("git")
            .arg("-C")
            .arg(workspace)
            .args([
                "log",
                "--name-only",
                "--pretty=format:",
                &format!("--since={}", since.to_rfc3339()),
            ])
            .output()
        && log_output.status.success()
    {
        dirty.extend(
            String::from_utf8_lossy(&log_output.stdout)
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string),
        );
    }

    let mut mismatched: Vec<String> = Vec::new();
    for line in summary.lines() {
        let words: std::collections::HashSet<String> = line
            .split(|c: char| !c.is_ascii_alphanumeric())
            .map(str::to_ascii_lowercase)
            .collect();
        if !CHANGE_VERBS.iter().any(|verb| words.contains(*verb)) {
            continue;
        }
        for token in line.split_whitespace() {
            let token = token.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '/'));
            // Path-shaped: has a directory separator and an extension-ish dot.
            if !token.contains('/') || !token.contains('.') || token.len() < 4 {
                continue;
            }
            let claimed = token.trim_start_matches("./").to_string();
            if dirty.contains(&claimed) {
                continue;
            }
            // A tracked-and-clean or nonexistent path claimed as changed is
            // the mismatch; a dirty or renamed path is a visible claim.
            if !mismatched.contains(&claimed) {
                mismatched.push(claimed);
            }
        }
    }
    if mismatched.is_empty() {
        return None;
    }
    Some(AgentRunVerificationSummary {
        status: "claim_mismatch".to_string(),
        summary: format!(
            "Result claims changed file(s) that git status does not show at delivery: {}. Treat the self-report as unverified and inspect the transcript.",
            mismatched.join(", ")
        ),
    })
}

fn default_agent_run_recommended_action() -> AgentRunRecommendedAction {
    AgentRunRecommendedAction {
        action: "inspect_transcript".to_string(),
        tool: Some(default_agent_inspect_tool()),
        reason: "Inspect the returned transcript handle if the child result needs audit detail."
            .to_string(),
    }
}

fn recommended_action_for_worker_status(
    status: AgentWorkerStatus,
    spec: &AgentWorkerSpec,
) -> AgentRunRecommendedAction {
    let agent_ref = spec
        .session_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or(&spec.worker_id);
    match status {
        AgentWorkerStatus::Queued => AgentRunRecommendedAction {
            action: "continue_parent_work".to_string(),
            tool: None,
            reason: format!(
                "Worker {agent_ref} is queued in the background; continue coordinating and consume its completion event when it arrives."
            ),
        },
        AgentWorkerStatus::Starting
        | AgentWorkerStatus::Running
        | AgentWorkerStatus::ModelWait
        | AgentWorkerStatus::RunningTool => AgentRunRecommendedAction {
            action: "continue_parent_work".to_string(),
            tool: None,
            reason: format!(
                "Worker {agent_ref} is active in the background; continue parent work until its completion event arrives."
            ),
        },
        AgentWorkerStatus::WaitingForUser => AgentRunRecommendedAction {
            action: "inspect_or_replace".to_string(),
            tool: Some(default_agent_inspect_tool()),
            reason: format!(
                "Worker {agent_ref} needs parent action; inspect the transcript handle and open a replacement with agent if the task still matters."
            ),
        },
        AgentWorkerStatus::Completed => AgentRunRecommendedAction {
            action: "verify_self_report".to_string(),
            tool: Some("handle_read".to_string()),
            reason: format!(
                "Worker {agent_ref} completed; verify its self-report before treating side effects as fact."
            ),
        },
        AgentWorkerStatus::Failed => AgentRunRecommendedAction {
            action: "inspect_failure".to_string(),
            tool: Some(default_agent_inspect_tool()),
            reason: format!(
                "Worker {agent_ref} failed; inspect the transcript handle and decide whether to open a replacement."
            ),
        },
        AgentWorkerStatus::Cancelled => AgentRunRecommendedAction {
            action: "open_replacement_if_needed".to_string(),
            tool: Some("agent".to_string()),
            reason: format!(
                "Worker {agent_ref} was cancelled; open a replacement with agent only if the assignment still matters."
            ),
        },
        AgentWorkerStatus::Interrupted => AgentRunRecommendedAction {
            action: "inspect_or_replace".to_string(),
            tool: Some(default_agent_inspect_tool()),
            reason: format!(
                "Worker {agent_ref} was interrupted; inspect the transcript handle before deciding whether to re-dispatch."
            ),
        },
    }
}

fn agent_worker_run_id(spec: &AgentWorkerSpec) -> String {
    if spec.run_id.is_empty() {
        spec.worker_id.clone()
    } else {
        spec.run_id.clone()
    }
}

fn follow_up_target_for_spec(spec: &AgentWorkerSpec) -> AgentRunFollowUpTarget {
    AgentRunFollowUpTarget {
        tool: default_agent_inspect_tool(),
        agent_id: spec.worker_id.clone(),
        session_name: spec.session_name.clone(),
        accepted_statuses: vec!["running".to_string(), "interrupted_continuable".to_string()],
        latest_delivery: None,
    }
}

fn takeover_target_for_spec(spec: &AgentWorkerSpec) -> AgentRunTakeoverTarget {
    let agent_ref = spec
        .session_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or(&spec.worker_id);
    AgentRunTakeoverTarget {
        kind: default_subagent_takeover_kind(),
        supported: true,
        agent_id: spec.worker_id.clone(),
        session_name: spec.session_name.clone(),
        instructions: format!(
            "Inspect agent '{agent_ref}' through the returned transcript_handle with handle_read; open a replacement with agent if the lane no longer fits."
        ),
        unsupported_reason: None,
    }
}

fn default_subagent_artifacts(run_id: &str) -> Vec<AgentRunArtifactRef> {
    vec![
        AgentRunArtifactRef {
            kind: "worker_events".to_string(),
            name: "worker_record.events".to_string(),
            target: run_id.to_string(),
            description: "Bounded structured lifecycle events retained on the worker record."
                .to_string(),
        },
        AgentRunArtifactRef {
            kind: "transcript".to_string(),
            name: "transcript_handle".to_string(),
            target: format!("agent:{run_id}"),
            description: "Open loads the complete private chat artifact, including the child's agent-owned todo_write working notes; use the bounded transcript_handle with handle_read for slices and artifact metadata."
                .to_string(),
        },
        AgentRunArtifactRef {
            kind: "receipt".to_string(),
            name: "result_summary".to_string(),
            target: run_id.to_string(),
            description: "Child final summary when present; verify before treating as fact."
                .to_string(),
        },
    ]
}

fn normalize_worker_spec(mut spec: AgentWorkerSpec) -> AgentWorkerSpec {
    if spec.run_id.is_empty() {
        spec.run_id = spec.worker_id.clone();
    }
    canonicalize_persisted_advisory_role(&mut spec.role);
    spec
}

fn canonicalize_persisted_advisory_role(role: &mut Option<String>) {
    if role.as_deref().is_some_and(|role| {
        matches!(
            role.trim().to_ascii_lowercase().as_str(),
            "oracle" | "advisor"
        )
    }) {
        *role = Some(FleetRole::Consultant.as_str().to_string());
    }
}

fn worker_coordination_claim(
    spec: &AgentWorkerSpec,
) -> Result<Option<(WriteScopeClaim, bool)>, String> {
    if !spec.runtime_profile.permissions.write {
        return Ok(None);
    }
    let manifest = spec.launch_manifest.as_ref().ok_or_else(|| {
        format!(
            "write-capable worker '{}' requires a persisted ChildLaunchManifest",
            spec.worker_id
        )
    })?;
    if manifest.child_id != spec.worker_id {
        return Err(format!(
            "worker '{}' launch manifest belongs to '{}'",
            spec.worker_id, manifest.child_id
        ));
    }
    let normalize_paths = |values: &[String], field: &str| {
        if values.len() > 32 {
            return Err(format!(
                "worker '{}' {field} accepts at most 32 entries",
                spec.worker_id
            ));
        }
        let mut normalized = Vec::new();
        for value in values {
            let value = normalize_claim_path(value)?;
            if !normalized.contains(&value) {
                normalized.push(value);
            }
        }
        Ok(normalized)
    };
    let roots = normalize_paths(&manifest.writable_roots, "writable_roots")?;
    let exact_files = normalize_paths(&manifest.writable_files, "writable_files")?;
    if manifest.coordination_contracts.len() > 16 {
        return Err(format!(
            "worker '{}' coordination_contracts accepts at most 16 entries",
            spec.worker_id
        ));
    }
    let mut contracts = Vec::new();
    for contract in &manifest.coordination_contracts {
        let contract = contract.trim();
        if contract.is_empty() || contract.chars().count() > 128 {
            return Err(format!(
                "worker '{}' coordination contracts must be 1..=128 characters",
                spec.worker_id
            ));
        }
        if !contracts.iter().any(|existing| existing == contract) {
            contracts.push(contract.to_string());
        }
    }
    if roots.is_empty() && exact_files.is_empty() && contracts.is_empty() {
        return Err(format!(
            "write-capable worker '{}' requires a bounded root, exact file, or coordination contract",
            spec.worker_id
        ));
    }
    Ok(Some((
        WriteScopeClaim {
            owner: spec.worker_id.clone(),
            roots,
            exact_files,
            contracts,
        },
        manifest.worktree,
    )))
}

fn worker_tool_scope(tool_profile: &AgentWorkerToolProfile) -> ToolScope {
    match tool_profile {
        AgentWorkerToolProfile::Inherited => ToolScope::Inherit,
        AgentWorkerToolProfile::Explicit(tools) => ToolScope::Explicit(tools.clone()),
    }
}

fn worker_profile_from_spec(spec: &AgentWorkerSpec) -> WorkerRuntimeProfile {
    let mut profile = WorkerRuntimeProfile::for_role(spec.agent_type.clone());
    profile.tools = worker_tool_scope(&spec.tool_profile);
    profile.model = ModelRoute::Fixed(spec.model.clone());
    profile.max_spawn_depth = spec.max_spawn_depth.saturating_sub(spec.spawn_depth);
    profile.max_steps = spec.max_steps.min(MAX_SUBAGENT_STEPS);
    profile.background = true;
    profile
}

fn worker_profile_for_spawn(
    runtime: &SubAgentRuntime,
    agent_type: &FleetRole,
    tool_profile: &AgentWorkerToolProfile,
    effective_model: &str,
    model_route: Option<ModelRoute>,
    custom_write_authority: bool,
) -> WorkerRuntimeProfile {
    let mut requested = WorkerRuntimeProfile::for_role(agent_type.clone());
    // Custom inherits the parent's effective posture by default (its explicit
    // tool list is the narrowing). The bounded write authority a spawning call
    // may pass is kept as an explicit, redundant grant for older callers.
    // Parent intersection below remains the hard ceiling.
    if *agent_type == FleetRole::Custom && custom_write_authority {
        requested.permissions.write = true;
        requested.shell = ShellPolicy::Full;
    }
    requested.tools = worker_tool_scope(tool_profile);
    requested.model = model_route.unwrap_or_else(|| ModelRoute::Fixed(effective_model.to_string()));
    let provider = runtime.client.api_provider();
    requested.provider = Some(
        runtime
            .api_config
            .as_ref()
            .map(|config| config.provider_identity_for(provider))
            .unwrap_or_else(|| provider.as_str().to_string()),
    );
    requested.max_spawn_depth = runtime.max_spawn_depth.saturating_sub(runtime.spawn_depth);
    requested.background = true;
    runtime.worker_profile.derive_child(&requested)
}

fn normalize_worker_record(mut record: AgentWorkerRecord) -> AgentWorkerRecord {
    record.spec = normalize_worker_spec(record.spec);
    if record.spec.runtime_profile == WorkerRuntimeProfile::default() {
        record.spec.runtime_profile = worker_profile_from_spec(&record.spec);
    }
    let run_id = agent_worker_run_id(&record.spec);
    if record.actor_kind.is_empty() {
        record.actor_kind = default_subagent_actor_kind();
    }
    if record.parent_run_id.is_none() {
        record.parent_run_id = record.spec.parent_run_id.clone();
    }
    if record.follow_up.agent_id.is_empty() {
        record.follow_up = follow_up_target_for_spec(&record.spec);
    } else if record.follow_up.tool != default_agent_inspect_tool() {
        record.follow_up.tool = default_agent_inspect_tool();
    }
    if record.takeover.agent_id.is_empty()
        || !record
            .takeover
            .instructions
            .contains(&default_agent_inspect_tool())
    {
        record.takeover = takeover_target_for_spec(&record.spec);
    }
    record.recommended_action = recommended_action_for_worker_status(record.status, &record.spec);
    if record.artifacts.is_empty() {
        record.artifacts = default_subagent_artifacts(&run_id);
    }
    if record.usage.status.is_empty() {
        record.usage = default_agent_run_usage();
    } else {
        refresh_usage_note(&mut record.usage);
    }
    if record.verification.status.is_empty() {
        record.verification = default_agent_run_verification();
    }
    record
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn current_git_branch(workspace: &Path) -> Option<String> {
    let branch = run_git(workspace, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = branch.trim();
    if branch.is_empty() {
        return None;
    }
    if branch != "HEAD" {
        return Some(branch.to_string());
    }

    let short_hash = run_git(workspace, &["rev-parse", "--short", "HEAD"])?;
    let short_hash = short_hash.trim();
    (!short_hash.is_empty()).then(|| format!("detached:{short_hash}"))
}

fn run_git(workspace: &Path, args: &[&str]) -> Option<String> {
    let output = Git::output(args, workspace).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SubAgentSpawnOptions {
    pub name: Option<String>,
    pub model: Option<String>,
    pub model_route: Option<ModelRoute>,
    pub child_route: Option<ChildRouteReceipt>,
    pub nickname: Option<String>,
    pub fork_context: bool,
    pub token_budget: Option<u64>,
    /// Optional per-child model-turn override, clamped to the runtime ceiling.
    pub max_steps: Option<u32>,
    /// Optional per-child wall-clock override, clamped to the runtime ceiling.
    pub wall_time: Option<Duration>,
    pub write_claim: Option<WriteScopeClaim>,
    pub isolated_worktree: bool,
    pub expected_artifact: Option<String>,
    /// Source agent id this child continues, stamped into the ChildLaunchManifest
    /// for receipt traceability.
    pub resume_from_agent_id: Option<String>,
    /// Checkpoint resume: the claim comes from the coordination ledger and is
    /// already namespaced — skip re-namespacing in the spawn seam.
    pub claim_pre_namespaced: bool,
    /// Checkpoint resume: preserve the interrupted child's runtime posture
    /// instead of rebuilding it from the caller's role.
    pub preserve_runtime_profile: Option<WorkerRuntimeProfile>,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkflowTaskSpawnResult {
    pub result: SubAgentResult,
    pub metadata: WorkflowTaskSpawnMetadata,
}

/// Workflow identity stamped onto children launched via `spawn_workflow_task`
/// (#4119). Lets panel/history render without parsing the child prompt.
#[derive(Debug, Clone)]
pub(crate) struct WorkflowTaskSpawnIdentity {
    pub workflow_run_id: String,
    pub workflow_phase_id: Option<String>,
    pub workflow_task_label: Option<String>,
    pub workflow_child_index: u32,
    /// Fingerprint of the exact-Fleet permission envelope this task was routed
    /// under, taken from the durable receipt.
    ///
    /// `None` for every non-Fleet task. When it is `Some`,
    /// [`spawn_workflow_task`] recomputes the fingerprint from the spawn input
    /// it actually built and refuses the launch on any difference. That check is
    /// what makes the Fleet's clamped authority a property of the child that
    /// runs rather than a value recorded next to it.
    pub fleet_authority_fingerprint: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkflowTaskSpawnMetadata {
    pub child_route: ChildRouteReceipt,
    pub resolved_provider: String,
    pub resolved_model: String,
    pub route_source: String,
    pub requested_reasoning: Option<String>,
    pub effective_reasoning: Option<String>,
    pub resolved_role: Option<String>,
    pub resolved_profile: Option<String>,
    pub parent_task_id: Option<String>,
    pub depth: u32,
    /// Workflow run that launched this child (`None` for direct `agent` spawns).
    pub workflow_run_id: Option<String>,
    /// Active phase title/id when the child was admitted (`None` outside workflows).
    pub workflow_phase_id: Option<String>,
    /// Human label from the Workflow `task({ label })` option.
    pub workflow_task_label: Option<String>,
    /// 0-based admission order among children of this workflow run.
    pub workflow_child_index: Option<u32>,
    /// Source agent this child was continued from via `resume_from`, if any.
    pub resume_from_agent_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubAgentModelStrength {
    Same,
    Faster,
}

impl SubAgentModelStrength {
    fn parse(value: &str) -> Result<Self, ToolError> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "same" | "inherit" | "parent" | "current" => Ok(Self::Same),
            "faster" | "fast" | "smaller" | "small" | "lower" | "cheap" | "flash" => {
                Ok(Self::Faster)
            }
            _ => Err(ToolError::invalid_input(
                "model_strength must be one of: same, faster".to_string(),
            )),
        }
    }

    fn model_route(self) -> ModelRoute {
        match self {
            Self::Same => ModelRoute::Inherit,
            Self::Faster => ModelRoute::Faster,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubAgentThinking {
    Inherit,
    Auto,
    Effort(ReasoningEffort),
}

impl SubAgentThinking {
    fn parse(value: &str) -> Result<Self, ToolError> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "inherit" | "parent" | "same" | "current" => Ok(Self::Inherit),
            _ => ReasoningEffort::parse_strict(value)
                .map(|effort| match effort {
                    ReasoningEffort::Auto => Self::Auto,
                    effort => Self::Effort(effort),
                })
                .map_err(|_| {
                    ToolError::invalid_input(
                        "thinking must be one of: inherit, auto, off, low, medium, high, max"
                            .to_string(),
                    )
                }),
        }
    }
}

/// Stable, non-secret label for the reasoning a caller *requested* (#4039).
///
/// `inherit`/`auto` are requests, not efforts: they must stay distinguishable
/// from the effort the route resolved to, or a row could claim the user asked
/// for something they only got.
pub(crate) fn subagent_thinking_label(thinking: SubAgentThinking) -> &'static str {
    match thinking {
        SubAgentThinking::Inherit => "inherit",
        SubAgentThinking::Auto => "auto",
        SubAgentThinking::Effort(effort) => effort.as_setting(),
    }
}

#[derive(Debug, Clone)]
struct SubAgentInput {
    text: String,
    interrupt: bool,
    /// Live "queued for the next round" counter shared with the manager.
    /// Decremented when the child loop takes the input off its channel, so
    /// the rail's `· N queued` count is the truthful number of follow-ups the
    /// child has not yet folded into its next model round.
    pending: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

impl SubAgentInput {
    /// Mark this input as consumed by the child loop.
    fn mark_taken(&self) {
        if let Some(pending) = self.pending.as_ref() {
            let _ = pending.fetch_update(
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
                |value| Some(value.saturating_sub(1)),
            );
        }
    }
}

fn append_subagent_inputs_as_user_messages(
    messages: &mut Vec<Message>,
    pending_inputs: &mut VecDeque<SubAgentInput>,
) {
    while let Some(input) = pending_inputs.pop_front() {
        if !input.text.trim().is_empty() {
            messages.push(Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: input.text,
                    cache_control: None,
                }],
            });
        }
    }
}

#[derive(Debug, Clone)]
struct SpawnRequest {
    session_name: Option<String>,
    prompt: String,
    /// Explicit bounded prerequisites relevant to this child. Persisted in the
    /// launch prompt; never populated from a parent transcript.
    dependencies: Vec<String>,
    /// Explicit bounded acceptance checks relevant to this child.
    acceptance: Vec<String>,
    agent_type: FleetRole,
    /// True when the caller supplied `type`/`agent_type` or `role` explicitly
    /// (vs the `Worker` default). A `profile` only sets the agent type when
    /// the caller did not, and conflicts are rejected only for explicit
    /// values.
    agent_type_explicit: bool,
    /// True only when the caller wrote the `type` field itself. `role` also
    /// sets `agent_type_explicit` (a role may be a type alias), but a role is
    /// a label for role resolution while `type` is a claim about what
    /// the child can do. Only the latter can contradict `write_authority`
    /// (#5123).
    agent_type_named: bool,
    /// Optional Fleet role id (trimmed, lowercased). Resolved at spawn time
    /// against the closed role set — parsing has no runtime access.
    profile: Option<String>,
    assignment: SubAgentAssignment,
    allowed_tools: Option<Vec<String>>,
    model: Option<String>,
    model_strength: SubAgentModelStrength,
    /// True when the caller supplied `model_strength` explicitly. An explicit
    /// strength outranks a configured role default; the parse-time
    /// default does not.
    model_strength_explicit: bool,
    thinking: SubAgentThinking,
    /// Optional working directory for the child. Must canonicalize to a path
    /// inside the parent's workspace. For first-class git worktree isolation,
    /// use `worktree` instead of pre-creating a cwd by hand.
    cwd: Option<PathBuf>,
    /// Optional first-class git worktree isolation. When set, Codewhale
    /// creates a sibling worktree/branch and runs the child from that checkout.
    worktree: Option<SubAgentWorktreeRequest>,
    /// Optional file path for cache-aware resident mode (#529). When set,
    /// the child's prompt is prefixed with the file contents for prefix-cache
    /// locality. A global ownership table prevents two agents from holding
    /// a resident lease on the same file simultaneously.
    resident_file: Option<String>,
    /// `Some(true)`: seed the child with the parent's system prompt and
    /// message prefix before appending the child task. `Some(false)`: force a
    /// fresh isolated context. `None` keeps the child fresh; transcript
    /// inheritance is always an explicit caller decision.
    fork_context: Option<bool>,
    /// Legacy recursion budget for descendants. The model-facing child tool
    /// surface is leaf-only; this remains for persisted/internal records.
    max_depth: Option<u32>,
    /// Optional aggregate token budget for this child and its descendants.
    /// When unset, the child inherits the parent's budget pool or the
    /// configured root default.
    token_budget: Option<u64>,
    max_steps: Option<u32>,
    wall_time: Option<Duration>,
    /// Extra tool deny-list from the caller, unioned with the parent runtime's
    /// inherited deny-list. Deny always wins over allow (#4042).
    disallowed_tools: Option<Vec<String>>,
    /// When true (default), the child inherits the parent runtime's
    /// `disallowed_tools`. Set `false` to start the child with a clean slate
    /// (only the explicit `disallowed_tools` above, if any, then apply).
    inherit_disallowed_tools: bool,
    /// Declared child write authority. Not schema decoration: `ReadOnly`
    /// narrows the child worker profile's write permission before spawn, so a
    /// child declared read-only cannot run Suggest-level write tools
    /// (TUI-DOG-017 truthful-affordance gate).
    write_authority: Option<SpawnWriteAuthority>,
    /// Declared expected artifact. Surfaced to the child in its prompt so the
    /// contract the spawner declared is visible to the agent doing the work.
    expected_artifact: Option<String>,
    /// Expected mutation boundary. Write-capable launches without an explicit
    /// root, exact file, or named contract default to the parent workspace
    /// root (`"."`). Escalation outside the parent workspace is refused.
    write_roots: Vec<String>,
    exact_files: Vec<String>,
    coordination_contracts: Vec<String>,
    /// Optional settled child agent id or session name whose transcript this
    /// child should continue. When set, the source agent's messages are loaded
    /// and injected as the initial context (fork_context=true), preserving
    /// transcript lineage across role/profile transitions (explore → implement
    /// → verify). The source must be settled (not running), in the same
    /// workspace, and reachable by the spawning agent.
    resume_from: Option<String>,
    /// Detached children deliberately outlive the active parent turn. The
    /// default is foreground ownership: normal turn end parks and joins the
    /// child's entire non-detached subtree before the turn becomes terminal.
    detached: bool,
}

/// Declared child write authority for a (deliberate) spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpawnWriteAuthority {
    ReadOnly,
    WorkspaceWrite,
    WorktreeWrite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentUsageBudgetScope {
    scope_id: String,
    limit: u64,
    spent: u64,
    remaining: u64,
}

/// Which terminal states `resume_from_checkpoint_with_policy` may re-dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResumePolicy {
    /// The model-facing `agents/followup` contract: interrupted children only.
    InterruptedOnly,
    /// The operator-facing continue-a-fork contract: interrupted or completed
    /// children whose last checkpoint is continuable.
    InterruptedOrCompleted,
}

impl ResumePolicy {
    fn describe(self) -> &'static str {
        match self {
            Self::InterruptedOnly => "only interrupted children are resumable",
            Self::InterruptedOrCompleted => {
                "only interrupted or completed children can be continued"
            }
        }
    }
}

/// Receipt for a user-originated follow-up to a child agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserFollowUpOutcome {
    /// The child the user addressed.
    pub agent_id: String,
    /// The child that now carries the conversation: the same id when the
    /// message was delivered live, or the resumed fork's new id.
    pub target_agent_id: String,
    /// Whether the text actually reached a live loop.
    pub delivered: bool,
    /// Whether a new agent loop was re-dispatched from the checkpoint.
    pub resumed: bool,
    /// Human-readable delivery note (never a secret).
    pub note: String,
}

/// Durable recovery point for an interrupted sub-agent session.
///
/// `messages` is a byte-bounded tail (#3882), not the full history:
/// checkpoints fire per step and are cloned into snapshots/persistence, so an
/// unbounded clone multiplies large tool outputs under Fleet fanout.
/// `message_count` records the true total and `omitted_messages` how many of
/// the oldest were dropped from this snapshot; spilled tool outputs remain on
/// disk under the spillover directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubAgentCheckpoint {
    pub checkpoint_id: String,
    pub agent_id: String,
    pub continuation_handle: String,
    pub reason: String,
    pub continuable: bool,
    pub steps_taken: u32,
    pub message_count: usize,
    pub created_at_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<Message>,
    /// Oldest messages omitted from `messages` to honor the checkpoint byte
    /// budget. `0` for records written before v0.8.67 (serde default).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub omitted_messages: usize,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSubAgent {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_name: Option<String>,
    #[serde(default)]
    fork_context: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace: Option<PathBuf>,
    agent_type: FleetRole,
    prompt: String,
    assignment: SubAgentAssignment,
    #[serde(default)]
    model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nickname: Option<String>,
    status: SubAgentStatus,
    result: Option<String>,
    steps_taken: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkpoint: Option<SubAgentCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    needs_input: Option<SubAgentNeedsInput>,
    duration_ms: u64,
    allowed_tools: Vec<String>,
    updated_at_ms: u64,
    /// Stable id of the manager / process boot that spawned this agent
    /// (#405). Lets a fresh manager filter out agents that were
    /// persisted by a prior session. Optional with `#[serde(default)]`
    /// for backward compatibility — older records lack the field and
    /// load with an empty string, which the manager treats as
    /// "from_prior_session" because it can't match any current id.
    #[serde(default)]
    session_boot_id: String,
    /// Root conversation that launched this child. Records written before
    /// this field existed deserialize to empty and are never eligible for
    /// completion synthesis (fail closed).
    #[serde(default)]
    owner_session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSubAgentState {
    schema_version: u32,
    /// Monotonic in-process snapshot id. Concurrent background writers use it
    /// to prevent an older clone from publishing after a newer one.
    #[serde(default)]
    snapshot_sequence: u64,
    agents: Vec<PersistedSubAgent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    workers: Vec<AgentWorkerRecord>,
    #[serde(default)]
    coordination: CoordinationLedger,
}

impl Default for PersistedSubAgentState {
    fn default() -> Self {
        Self {
            schema_version: SUBAGENT_STATE_SCHEMA_VERSION,
            snapshot_sequence: 0,
            agents: Vec::new(),
            workers: Vec::new(),
            coordination: CoordinationLedger::default(),
        }
    }
}

/// Default cap on sub-agent recursion depth. Override via
/// `[subagents] max_depth = N` in config.
///
/// Sourced from [`codewhale_config::DEFAULT_SPAWN_DEPTH`] so standalone
/// sub-agents and fleet workers share ONE recursion axis (no "two moving
/// targets"). Configured/requested depths clamp to
/// [`codewhale_config::MAX_SPAWN_DEPTH_CEILING`].
pub const DEFAULT_MAX_SPAWN_DEPTH: u32 = codewhale_config::DEFAULT_SPAWN_DEPTH;

/// Resolve a child runtime's `max_spawn_depth` from its (already-incremented)
/// `spawn_depth` and the model-supplied per-call `max_depth`, clamped to the
/// absolute [`codewhale_config::MAX_SPAWN_DEPTH_CEILING`].
///
/// Without the absolute clamp, `max_spawn_depth = spawn_depth + max_depth`
/// makes the recursion gate (`spawn_depth + 1 > max_spawn_depth`) reduce to
/// `1 > max_depth` at every level — always false when the model re-supplies
/// `max_depth >= 1` per spawn — so ring depth would grow to the global
/// admission cap instead of the intended 8-ring ceiling.
fn clamp_child_max_spawn_depth(child_spawn_depth: u32, requested_max_depth: u32) -> u32 {
    child_spawn_depth
        .saturating_add(requested_max_depth)
        .min(codewhale_config::MAX_SPAWN_DEPTH_CEILING)
}

/// Terminal-state notification emitted to the immediate parent's completion
/// inbox when one of its children finishes (issue #756). For root-spawned
/// agents that inbox is the engine turn loop; for nested agents it is a
/// parent-local receiver inside `run_subagent`. Carries the already-rendered
/// `<codewhale:subagent.done>` sentinel that the model expects in the
/// transcript per the constitution (`prompts/text.rs`, `BASE_PROMPT`).
#[derive(Debug, Clone)]
pub struct SubAgentCompletion {
    /// Root session that owned the child when it was launched. Completion
    /// channels outlive individual conversations, so consumers must compare
    /// this immutable owner with the active session before deduplicating or
    /// injecting the payload.
    pub owner_session_id: String,
    /// The completing child's agent id. Held for routing/logging — the
    /// engine's turn loop does not currently key on it (it just injects
    /// the payload), but downstream tooling and tests need the field.
    #[allow(dead_code)]
    pub agent_id: String,
    /// Human summary on line 1, sentinel on line 2. Same payload shape as
    /// `Event::AgentComplete::result`.
    pub payload: String,
}

impl SubAgentCompletion {
    /// Terminal failures are marked inside the model-visible sentinel so the
    /// same fact survives channel fan-in, transcript persistence, and replay.
    #[must_use]
    pub fn is_high_priority_failure(&self) -> bool {
        self.payload.contains(r#""event":"subagent.failed""#)
    }
}

/// Live-only sinks needed to publish one terminal child outcome.
///
/// This deliberately lives on [`SubAgent`] rather than the persisted worker
/// record: channels are process-local capabilities and must never cross a
/// restart boundary. Keeping the immediate-parent sender here lets explicit
/// Stop and stale cleanup use the same claim -> deliver -> commit path as a
/// natural task exit instead of aborting the only future that knew how to wake
/// the parent (#4408).
#[derive(Clone)]
struct SubAgentTerminalDeliveryContext {
    spawn_depth: u32,
    parent_completion_tx: Option<mpsc::UnboundedSender<SubAgentCompletion>>,
    mailbox: Option<Mailbox>,
    event_tx: Option<mpsc::Sender<Event>>,
    /// Shared session namespace (root session id), cloned down the spawn
    /// tree; over-budget final reports are spilled under it so the truncation
    /// footer can name a retrievable artifact.
    session_id: String,
}

impl SubAgentTerminalDeliveryContext {
    fn from_runtime(runtime: &SubAgentRuntime) -> Self {
        Self {
            spawn_depth: runtime.spawn_depth,
            parent_completion_tx: runtime.parent_completion_tx.clone(),
            mailbox: runtime.mailbox.clone(),
            event_tx: runtime.event_tx.clone(),
            session_id: runtime.context.state_namespace.clone(),
        }
    }

    /// Publish to every live sink without blocking or awaiting while the
    /// manager owns the terminal claim. The public agent/worker states remain
    /// Running until all three sends have been attempted.
    fn deliver(&self, result: &SubAgentResult) {
        let report_ref = spill_subagent_final_report(&self.session_id, result);
        let completion = subagent_completion_from_result_with_ref_for_session(
            &self.session_id,
            result,
            report_ref.as_deref(),
        );

        if self.spawn_depth > 0
            && let Some(tx) = self.parent_completion_tx.as_ref()
        {
            let _ = tx.send(completion.clone());
        }

        if let Some(mailbox) = self.mailbox.as_ref() {
            let _ = mailbox.send(terminal_mailbox_message(result, report_ref.as_deref()));
        }

        if let Some(event_tx) = self.event_tx.as_ref() {
            let _ = event_tx.try_send(Event::AgentComplete {
                owner_session_id: self.session_id.clone(),
                id: result.agent_id.clone(),
                result: completion.payload,
            });
        }
    }
}

fn terminal_mailbox_message(result: &SubAgentResult, report_ref: Option<&str>) -> MailboxMessage {
    match &result.status {
        SubAgentStatus::Completed => {
            let (summary, _) =
                stamp_subagent_summary_with_ref(&summarize_subagent_result(result), report_ref);
            MailboxMessage::Completed {
                agent_id: result.agent_id.clone(),
                summary,
            }
        }
        SubAgentStatus::Interrupted(reason) => MailboxMessage::Interrupted {
            agent_id: result.agent_id.clone(),
            reason: reason.clone(),
        },
        SubAgentStatus::Failed(error) => MailboxMessage::Failed {
            agent_id: result.agent_id.clone(),
            error: error.clone(),
        },
        SubAgentStatus::Cancelled => MailboxMessage::Cancelled {
            agent_id: result.agent_id.clone(),
        },
        SubAgentStatus::BudgetExhausted => MailboxMessage::Failed {
            agent_id: result.agent_id.clone(),
            error: summarize_subagent_result(result),
        },
        SubAgentStatus::Running => MailboxMessage::Progress {
            agent_id: result.agent_id.clone(),
            status: "running".to_string(),
        },
    }
}

/// Parent transcript snapshot available to sub-agents that opt into context
/// forking. Leading messages may be inherited as context, but every child
/// keeps its own resolved system prompt so parent-specific model identity or
/// role text cannot override the worker's actual route and instructions.
#[derive(Clone, Debug)]
pub struct SubAgentForkContext {
    pub messages: Vec<Message>,
    /// Stable, To-do-free parent state captured once at turn start. History
    /// semantics stay exactly as they were: this text is not re-derived per
    /// spawn.
    pub structured_state_block: Option<String>,
    /// Where to read the spawning agent's To-do list *at spawn time*.
    ///
    /// The block above is captured before the turn's first tool call, so a
    /// `work_update` earlier in the same turn would otherwise hand the child a
    /// stale list (#3983). Only the To-do portion is refreshed.
    pub work_source: Option<crate::todo_snapshot::TodoSource>,
}

impl SubAgentForkContext {
    /// Resolve the fork-state block at the fork seam: the stable captured
    /// state, then the current To-do snapshot. Shown to the child once, in the
    /// context block stored in its own history — never re-sent.
    pub(crate) async fn with_resolved_state_block(&self) -> Self {
        let stable = self
            .structured_state_block
            .as_deref()
            .map(str::trim)
            .filter(|state| !state.is_empty())
            .map(str::to_string);
        let todo_body = match self.work_source.as_ref() {
            Some(source) => source.body().await,
            None => None,
        };
        let todo_section = todo_body
            .as_deref()
            .map(crate::todo_snapshot::fork_state_todo_section);

        let structured_state_block = match (stable, todo_section) {
            (Some(stable), Some(work)) => Some(format!("{stable}\n{work}")),
            (Some(stable), None) => Some(stable),
            (None, work) => work,
        };

        Self {
            messages: self.messages.clone(),
            structured_state_block,
            work_source: self.work_source.clone(),
        }
    }
}

/// Runtime configuration for spawning sub-agents.
///
/// Carries everything a child needs to (a) build its own tool registry —
/// including the manager so grandchildren can spawn — and (b) cooperate with
/// lifecycle cancellation and depth caps. `child_runtime()` links cancellation
/// tokens and turn ownership, while `background_runtime()` explicitly detaches
/// long-running `agent` sessions from the caller's turn token.
#[derive(Debug)]
pub(crate) struct ForegroundChildRegistry {
    state: std::sync::Mutex<ForegroundChildState>,
    settled: tokio::sync::watch::Sender<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForegroundSettlement {
    Cancel,
    Park,
}

#[derive(Debug)]
struct ForegroundChildEntry {
    agent_id: String,
    token: CancellationToken,
    parking_requested: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Debug, Default)]
struct ForegroundChildState {
    settlement: Option<ForegroundSettlement>,
    next_id: u64,
    children: HashMap<u64, ForegroundChildEntry>,
}

pub(crate) struct ForegroundChildRegistration {
    registry: std::sync::Weak<ForegroundChildRegistry>,
    id: u64,
    parking_requested: Arc<std::sync::atomic::AtomicBool>,
}

impl ForegroundChildRegistry {
    #[must_use]
    pub(crate) fn new() -> Self {
        let (settled, _) = tokio::sync::watch::channel(());
        Self {
            state: std::sync::Mutex::new(ForegroundChildState::default()),
            settled,
        }
    }

    pub(crate) fn register(
        self: &Arc<Self>,
        agent_id: impl Into<String>,
        token: CancellationToken,
    ) -> Result<ForegroundChildRegistration, ForegroundSettlement> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(settlement) = state.settlement {
            token.cancel();
            return Err(settlement);
        }
        let id = state.next_id;
        state.next_id = state.next_id.saturating_add(1);
        let parking_requested = Arc::new(std::sync::atomic::AtomicBool::new(false));
        state.children.insert(
            id,
            ForegroundChildEntry {
                agent_id: agent_id.into(),
                token,
                parking_requested: Arc::clone(&parking_requested),
            },
        );
        Ok(ForegroundChildRegistration {
            registry: Arc::downgrade(self),
            id,
            parking_requested,
        })
    }

    #[must_use]
    pub(crate) fn active_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .children
            .len()
    }

    #[must_use]
    pub(crate) fn active_agent_ids(&self) -> Vec<String> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut agent_ids = state
            .children
            .values()
            .map(|entry| entry.agent_id.clone())
            .collect::<Vec<_>>();
        agent_ids.sort();
        agent_ids
    }

    fn release(&self, id: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.children.remove(&id).is_some() {
            self.settled.send_replace(());
        }
    }

    /// Cancel every currently-owned child and wait until each task has
    /// released its registration. Multiple terminal paths share this barrier:
    /// only the first call issues cancellation, while all callers await the
    /// same settled set. A child registered after cancellation observes the
    /// latched state and is cancelled before it can reach a provider request.
    pub(crate) async fn cancel_and_wait(&self) {
        self.settle_and_wait(ForegroundSettlement::Cancel).await;
    }

    /// Park turn-owned children as resumable before cancelling their live
    /// tasks. The flag is stored before token cancellation, so every child
    /// projects the race as Interrupted with a checkpoint rather than a
    /// terminal Cancelled receipt.
    pub(crate) async fn park_and_wait(&self) {
        self.settle_and_wait(ForegroundSettlement::Park).await;
    }

    async fn settle_and_wait(&self, requested: ForegroundSettlement) {
        use std::sync::atomic::Ordering;

        let mut settled = self.settled.subscribe();
        let tokens = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.settlement.is_some() {
                Vec::new()
            } else {
                state.settlement = Some(requested);
                state
                    .children
                    .values()
                    .map(|entry| {
                        if requested == ForegroundSettlement::Park {
                            entry.parking_requested.store(true, Ordering::Release);
                        }
                        entry.token.clone()
                    })
                    .collect::<Vec<_>>()
            }
        };
        for token in tokens {
            token.cancel();
        }

        loop {
            let is_settled = {
                let state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.children.is_empty()
            };
            if is_settled {
                return;
            }
            // `watch` retains a version change that races this check, unlike a
            // plain notification created but not yet polled by this task.
            let _ = settled.changed().await;
        }
    }
}

impl ForegroundChildRegistration {
    pub(crate) fn parking_signal(&self) -> Arc<std::sync::atomic::AtomicBool> {
        Arc::clone(&self.parking_requested)
    }
}

impl Drop for ForegroundChildRegistration {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            registry.release(self.id);
        }
    }
}

#[derive(Clone)]
pub struct SubAgentRuntime {
    pub client: DeepSeekClient,
    /// Session `Config` snapshot, used for role-model defaults,
    /// provider-identity receipts, and model routing at spawn time. The engine
    /// threads it in via [`SubAgentRuntime::with_api_config`];
    /// `child_runtime`/`background_runtime` clone the `Arc` so every
    /// descendant resolves the same session route.
    ///
    /// `None` for legacy/test runtimes that never threaded a config.
    pub api_config: Option<std::sync::Arc<crate::config::Config>>,
    pub model: String,
    /// Active UI/model locale used for generated human-facing worker names.
    /// Internal ids and session handles remain language-neutral.
    pub locale_tag: String,
    pub auto_model: bool,
    pub reasoning_effort: Option<String>,
    pub reasoning_effort_auto: bool,
    pub role_models: HashMap<String, String>,
    pub context: ToolContext,
    pub allow_shell: bool,
    /// When true, Suggest-level file writes auto-accept for write-capable roles
    /// without full parent auto-approve. Shell/network/MCP still gated.
    /// Set for Workflow-spawned children and parent-approved root Operate
    /// workers.
    pub accept_edits: bool,
    /// Allow the built-in, non-custom verification tools after a root Operate
    /// worker start has crossed the parent's approval boundary. This is not a
    /// general shell grant: arbitrary commands and custom verifier programs
    /// remain blocked unless the parent session is auto-approved.
    pub accept_verification: bool,
    /// Native Agent-mode tool surface inherited from the parent turn. Carries
    /// feature/config-dependent families such as web search, patch, memory,
    /// vision, notify, and FIM so child catalogs stay in parity with the parent.
    pub agent_tool_surface_options: AgentToolSurfaceOptions,
    /// Capability contract inherited by descendants. `agent` derives a
    /// child profile from this before registering the worker record so parent,
    /// sub-agent, and fleet projections share one worker contract.
    pub worker_profile: WorkerRuntimeProfile,
    pub event_tx: Option<mpsc::Sender<Event>>,
    /// Manager handle so children can recurse via `agent`. All agents
    /// at every depth share the same manager.
    pub manager: SharedSubAgentManager,
    /// Depth in the spawn tree. 0 = top-level user turn; 1 = direct child;
    /// etc. Children clone the parent runtime and increment this on spawn.
    pub spawn_depth: u32,
    /// Agent id that should be recorded as parent for any child spawned
    /// through this runtime's model-visible `agent` tool. `None` for the
    /// root engine; set to the running sub-agent id for nested spawns so UI
    /// surfaces can render the tree.
    pub parent_agent_id: Option<String>,
    /// Hard cap on recursion depth. A child whose `spawn_depth + 1` would
    /// exceed this is rejected at the spawn entry. Use `>` (strictly
    /// greater than) so equality is allowed — matches codex's pattern.
    pub max_spawn_depth: u32,
    /// Cooperative cancellation token. Direct `child_runtime()` callers derive
    /// a child token from the parent; explicitly detached model-visible
    /// `agent` starts use `background_runtime()` to replace it.
    pub cancel_token: CancellationToken,
    /// Turn-scoped ownership barrier for the complete foreground subtree.
    /// Every non-detached descendant registers with the same root barrier;
    /// explicitly detached runtimes clear it for their whole subtree.
    foreground_children: Option<Arc<ForegroundChildRegistry>>,
    /// Structured progress / lifecycle stream. Cloned across children so the
    /// whole spawn tree publishes into one ordered, fan-out-able mailbox.
    /// `None` only when no consumer is wired (legacy entry points / tests).
    pub mailbox: Option<Mailbox>,
    /// Lease on the durable runtime-turn accounting sink. Detached child
    /// runtimes clone this guard, keeping the sink alive after the parent UI
    /// mailbox closes until the final child response has been persisted.
    pub(crate) runtime_usage_lease: Option<crate::cost_status::RuntimeUsageLease>,
    /// Wakeup channel for this runtime's immediate parent (issue #756). For
    /// the engine's direct children this points at the engine turn loop. While
    /// a sub-agent is running, its tool registry swaps this for a local inbox
    /// so nested children report to their orchestrating sub-agent instead of
    /// flooding the root parent. `None` when no consumer is wired (tests /
    /// legacy paths).
    pub parent_completion_tx: Option<mpsc::UnboundedSender<SubAgentCompletion>>,
    /// Snapshot of the request prefix visible to an opt-in forked child.
    pub fork_context: Option<SubAgentForkContext>,
    /// The parent's MCP pool if available.
    pub mcp_pool: Option<std::sync::Arc<tokio::sync::Mutex<crate::mcp::McpPool>>>,
    /// Per-step DeepSeek API timeout for the child's `create_message` call.
    /// Resolved from `[subagents] api_timeout_secs` (clamped to 1..=3600) at
    /// engine construction so a slow but legitimate model turn does not
    /// false-timeout the child mid-thinking. `child_runtime()` and
    /// `background_runtime()` preserve the parent's value (#1806, #1808).
    pub step_api_timeout: Duration,
    /// Initial backoff between timed-out `create_message` retries. Defaults
    /// to [`SUBAGENT_API_TIMEOUT_INITIAL_BACKOFF`]; tests shrink it so the
    /// timeout-retry path runs in milliseconds. `child_runtime()` preserves
    /// the parent's value.
    pub(crate) api_timeout_retry_base_backoff: Duration,
    /// Wall-clock budget for a single tool execution within a sub-agent step.
    /// Defaults to `DEFAULT_TOOL_TIMEOUT`; the engine may override it so a long
    /// but legitimate tool run is not killed mid-flight. `child_runtime()`
    /// preserves the parent's value.
    pub tool_timeout: Duration,
    /// Default directory for Xiaomi MiMo speech/TTS tool outputs inherited by
    /// child registries. Keeps parent and sub-agent `speech` / `tts` tools on
    /// the same `[speech].output_dir` / env override.
    pub speech_output_dir: Option<PathBuf>,
    /// This runtime's **own** todo list. It is never shared with a parent or a
    /// sibling: `child_runtime()` allocates a fresh `SharedTodoList` for every
    /// spawned agent (#4810). Sharing the parent's `Arc` here let a child's
    /// `work_update` / `todo_write` replace the parent's Work checklist
    /// wholesale — a data-integrity bug, because the tools *replace* the list
    /// rather than merge into it, and because `WorkRuntime::matches_todos`
    /// routes any write on the parent `Arc` straight into the parent's work
    /// graph.
    ///
    /// Parent progress reaches a child as **immutable context only**, via the
    /// `fork_context` structured-state block (see `StructuredState::capture`),
    /// never as writable state. Child progress reaches the parent through the
    /// completion payload and delegate card, not by mutating the parent list.
    pub todos: SharedTodoList,
    /// Session mode of the orchestrating parent at spawn time (Wave 7 M4/M5).
    pub parent_mode: AppMode,
    /// The session's permission posture at spawn time. Children inherit it
    /// faithfully: under Auto-Review the same deterministic floor and model
    /// guardian that gate the parent gate the child's held calls; under Ask a
    /// held call is routed to the parent's approval UI when one exists;
    /// Full Access still fails closed on the non-bypassable safety floor.
    pub approval_mode: crate::tui::approval::ApprovalMode,
    /// The session's deterministic Auto-Review policy (configured allow/block
    /// rules plus the built-in safety floor), shared with every descendant.
    pub auto_review_policy: std::sync::Arc<crate::tui::auto_review::AutoReviewPolicy>,
    /// Whether the host can answer an approval prompt for a child (an
    /// interactive TUI). Headless hosts keep the fail-closed denial.
    pub parent_can_prompt: bool,
    /// Durable approval evidence inherited from the parent session. Legacy
    /// runtimes that do not install a store cannot open child approval prompts.
    approval_receipt_store: Option<Result<crate::approval_log::ApprovalReceiptStore, String>>,
}

impl SubAgentRuntime {
    /// Create a top-level runtime configuration for sub-agent execution.
    /// Use this from the engine when constructing the runtime that the
    /// parent's tool registry passes through. Children should derive their
    /// runtime via `Self::child_runtime` instead.
    #[must_use]
    pub fn new(
        client: DeepSeekClient,
        model: String,
        context: ToolContext,
        allow_shell: bool,
        event_tx: Option<mpsc::Sender<Event>>,
        manager: SharedSubAgentManager,
    ) -> Self {
        Self {
            client,
            api_config: None,
            model,
            locale_tag: "en".to_string(),
            auto_model: false,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            role_models: HashMap::new(),
            context,
            allow_shell,
            accept_edits: false,
            accept_verification: false,
            agent_tool_surface_options: AgentToolSurfaceOptions::new(
                ShellPolicy::from_legacy_allow_shell(allow_shell),
            ),
            worker_profile: WorkerRuntimeProfile::for_role(FleetRole::Worker),
            event_tx,
            manager,
            spawn_depth: 0,
            parent_agent_id: None,
            max_spawn_depth: DEFAULT_MAX_SPAWN_DEPTH,
            cancel_token: CancellationToken::new(),
            foreground_children: None,
            mailbox: None,
            runtime_usage_lease: None,
            parent_completion_tx: None,
            fork_context: None,
            mcp_pool: None,
            step_api_timeout: DEFAULT_STEP_API_TIMEOUT,
            api_timeout_retry_base_backoff: SUBAGENT_API_TIMEOUT_INITIAL_BACKOFF,
            tool_timeout: DEFAULT_TOOL_TIMEOUT,
            speech_output_dir: None,
            todos: crate::tools::todo::new_shared_todo_list(),
            parent_mode: AppMode::Agent,
            approval_mode: crate::tui::approval::ApprovalMode::Suggest,
            auto_review_policy: std::sync::Arc::new(
                crate::tui::auto_review::AutoReviewPolicy::default(),
            ),
            parent_can_prompt: false,
            approval_receipt_store: None,
        }
    }

    /// Preserve the parent session mode for spawn-policy decisions.
    #[must_use]
    pub fn with_parent_mode(mut self, mode: AppMode) -> Self {
        self.parent_mode = mode;
        self
    }

    /// Install the parent session's append-only approval receipt store.
    #[must_use]
    pub(crate) fn with_approval_receipt_store(
        mut self,
        store: Result<crate::approval_log::ApprovalReceiptStore, String>,
    ) -> Self {
        self.approval_receipt_store = Some(store);
        self
    }

    /// Install the session's permission posture and Auto-Review policy so
    /// children are gated exactly like the parent, and say whether the host
    /// can answer a prompt raised on a child's behalf.
    #[must_use]
    pub fn with_permission_posture(
        mut self,
        approval_mode: crate::tui::approval::ApprovalMode,
        auto_review_policy: std::sync::Arc<crate::tui::auto_review::AutoReviewPolicy>,
        parent_can_prompt: bool,
    ) -> Self {
        self.approval_mode = approval_mode;
        self.auto_review_policy = auto_review_policy;
        self.parent_can_prompt = parent_can_prompt;
        self
    }

    /// Match generated worker display names to the active session language.
    #[must_use]
    pub fn with_locale_tag(mut self, locale_tag: impl Into<String>) -> Self {
        self.locale_tag = locale_tag.into();
        self
    }

    /// Bind the todo list this runtime writes to. The engine passes its own
    /// session list here so the *root* runtime and the turn tool registry
    /// agree on one list; spawned agents never inherit it, because
    /// `child_runtime()` allocates a fresh list per agent (#4810).
    #[must_use]
    pub fn with_todos(mut self, todos: SharedTodoList) -> Self {
        self.todos = todos;
        self
    }

    /// Preserve the parent Agent-mode native tool surface for child registries.
    #[must_use]
    pub fn with_agent_tool_surface_options(mut self, options: AgentToolSurfaceOptions) -> Self {
        self.speech_output_dir = options.speech_output_dir.clone();
        self.agent_tool_surface_options = options;
        self
    }

    /// Attach an MCP pool so the subagent can execute MCP tools.
    #[must_use]
    pub fn with_mcp_pool(
        mut self,
        pool: Option<std::sync::Arc<tokio::sync::Mutex<crate::mcp::McpPool>>>,
    ) -> Self {
        self.mcp_pool = pool;
        self
    }

    /// Override the per-step DeepSeek API timeout (default
    /// `DEFAULT_STEP_API_TIMEOUT`). Called by the engine after reading
    /// `[subagents] api_timeout_secs`. Tests may use this to fail fast
    /// without waiting the default 600 seconds (#1806, #1808).
    #[must_use]
    pub fn with_step_api_timeout(mut self, timeout: Duration) -> Self {
        self.step_api_timeout = timeout;
        self
    }

    /// Shrink the timeout-retry backoff so tests covering the retry path do
    /// not wait out the production 1s..=30s backoff sequence.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_api_timeout_retry_base_backoff(mut self, backoff: Duration) -> Self {
        self.api_timeout_retry_base_backoff = backoff;
        self
    }

    /// Preserve the configured speech output directory for sub-agent tools.
    #[must_use]
    pub fn with_speech_output_dir(mut self, output_dir: Option<PathBuf>) -> Self {
        self.speech_output_dir = output_dir.clone();
        self.agent_tool_surface_options.speech_output_dir = output_dir;
        self
    }

    /// Attach the wakeup channel for this runtime's immediate parent. The
    /// engine uses this for direct children; running sub-agents replace it in
    /// the runtime handed to their nested `agent` tool so child completions are
    /// routed back to the sub-agent that spawned them.
    #[must_use]
    pub fn with_parent_completion_tx(
        mut self,
        tx: mpsc::UnboundedSender<SubAgentCompletion>,
    ) -> Self {
        self.parent_completion_tx = Some(tx);
        self
    }

    /// Attach the current parent request prefix for `fork_context` spawns.
    #[must_use]
    pub fn with_fork_context(mut self, context: SubAgentForkContext) -> Self {
        self.fork_context = Some(context);
        self
    }

    /// Attach a `Mailbox` so this runtime and its derived children publish
    /// structured `MailboxMessage` envelopes alongside the legacy `Event`
    /// stream. Pair with [`Self::with_cancel_token`] when the mailbox close
    /// token should match this runtime's cancellation token.
    #[must_use]
    #[allow(dead_code)] // wired by #128 (in-transcript cards) when it lands.
    pub fn with_mailbox(mut self, mailbox: Mailbox) -> Self {
        self.mailbox = Some(mailbox);
        self
    }

    /// Bind descendants to the durable accounting owner created by a runtime
    /// host. Interactive TUI turns have no owner and continue using mailbox
    /// delivery only.
    #[must_use]
    pub(crate) fn with_runtime_cost_owner(mut self, owner: Option<&str>) -> Self {
        self.runtime_usage_lease = owner.and_then(crate::cost_status::acquire_runtime_usage_lease);
        self
    }

    /// Replace the cancellation token (e.g. when the engine constructs the
    /// runtime alongside a mailbox bound to the same token).
    #[must_use]
    #[allow(dead_code)] // wired by #128 alongside `with_mailbox`.
    pub fn with_cancel_token(mut self, token: CancellationToken) -> Self {
        self.context.cancel_token = Some(token.clone());
        self.cancel_token = token;
        self
    }

    /// Attach the turn-owned foreground-subtree registry. Engine-only wiring
    /// keeps the ownership boundary out of Fleet scheduling and persisted
    /// records.
    #[must_use]
    pub(crate) fn with_foreground_children(
        mut self,
        foreground_children: Arc<ForegroundChildRegistry>,
    ) -> Self {
        self.foreground_children = Some(foreground_children);
        self
    }

    /// Override the maximum spawn depth (default `DEFAULT_MAX_SPAWN_DEPTH`).
    /// Used by config wiring (`[subagents] max_depth = N`) and tests.
    #[must_use]
    #[allow(dead_code)]
    pub fn with_max_spawn_depth(mut self, max: u32) -> Self {
        self.max_spawn_depth = max;
        self
    }

    /// Attach raw role/type model overrides. Values are intentionally
    /// validated at spawn time so bad config fails before a partial spawn.
    #[must_use]
    pub fn with_role_models(mut self, role_models: HashMap<String, String>) -> Self {
        self.role_models = role_models;
        self
    }

    /// Attach the session `Config` for spawn-time role-model defaults, receipts,
    /// and model routing (see the [`api_config`](Self::api_config) field
    /// docs). Engine-only wiring; test and legacy runtimes may leave it unset.
    #[must_use]
    pub fn with_api_config(mut self, config: crate::config::Config) -> Self {
        self.api_config = Some(std::sync::Arc::new(config));
        self
    }

    /// Preserve whether the parent session is using per-turn model routing.
    #[must_use]
    pub fn with_auto_model(mut self, auto_model: bool) -> Self {
        self.auto_model = auto_model;
        self
    }

    /// Preserve the parent's thinking configuration. Child model strength is
    /// explicit on the `agent` call; this only controls reasoning effort.
    #[must_use]
    pub fn with_reasoning_effort(
        mut self,
        reasoning_effort: Option<String>,
        reasoning_effort_auto: bool,
    ) -> Self {
        self.reasoning_effort = reasoning_effort;
        self.reasoning_effort_auto = reasoning_effort_auto;
        self
    }

    /// Return a child runtime that is deliberately detached from the parent
    /// turn cancellation token and its foreground ownership barrier. Explicit
    /// agent cancellation still aborts its task handle through the manager.
    #[must_use]
    pub fn background_runtime(&self) -> Self {
        let mut runtime = self.child_runtime();
        let token = CancellationToken::new();
        runtime.cancel_token = token.clone();
        runtime.context.cancel_token = Some(token);
        runtime.foreground_children = None;
        runtime
    }

    fn foreground_child_registration(
        &self,
        agent_id: &str,
    ) -> Result<Option<ForegroundChildRegistration>> {
        if self.spawn_depth == 0 {
            return Ok(None);
        }
        let Some(registry) = self.foreground_children.as_ref() else {
            return Ok(None);
        };
        registry
            .register(agent_id, self.cancel_token.clone())
            .map(Some)
            .map_err(|settlement| {
                anyhow!(
                    "parent turn is already settling foreground work as {settlement:?}; refusing to launch child {agent_id} after the ownership barrier closed"
                )
            })
    }

    /// Build a child runtime cloning this one, incrementing `spawn_depth`,
    /// and deriving a child cancellation token. Used at spawn entry to
    /// construct the runtime the new sub-agent will see.
    ///
    /// Children inherit the parent's approval state. A non-auto parent can
    /// still delegate read-only investigation, but approval-gated child tools
    /// are blocked by the sub-agent registry instead of being silently run
    /// without a prompt.
    #[must_use]
    pub fn child_runtime(&self) -> Self {
        let mut child_context = self.context.clone();
        child_context.auto_approve = self.context.auto_approve;
        let cancel_token = self.cancel_token.child_token();
        child_context.cancel_token = Some(cancel_token.clone());
        Self {
            client: self.client.clone(),
            api_config: self.api_config.clone(),
            model: self.model.clone(),
            locale_tag: self.locale_tag.clone(),
            auto_model: self.auto_model,
            reasoning_effort: self.reasoning_effort.clone(),
            reasoning_effort_auto: self.reasoning_effort_auto,
            role_models: self.role_models.clone(),
            context: child_context,
            allow_shell: self.allow_shell,
            accept_edits: self.accept_edits,
            // A parent-approved Operate verification lease belongs to its
            // direct worker only; nested children must cross their own
            // approval boundary instead of silently inheriting it.
            accept_verification: self.accept_verification && self.spawn_depth == 0,
            agent_tool_surface_options: self.agent_tool_surface_options.clone(),
            worker_profile: self.worker_profile.clone(),
            event_tx: self.event_tx.clone(),
            manager: self.manager.clone(),
            spawn_depth: self.spawn_depth + 1,
            parent_agent_id: self.parent_agent_id.clone(),
            max_spawn_depth: self.max_spawn_depth,
            cancel_token,
            foreground_children: self.foreground_children.clone(),
            mailbox: self.mailbox.clone(),
            runtime_usage_lease: self.runtime_usage_lease.clone(),
            parent_completion_tx: self.parent_completion_tx.clone(),
            fork_context: self.fork_context.clone(),
            mcp_pool: self.mcp_pool.clone(),
            step_api_timeout: self.step_api_timeout,
            api_timeout_retry_base_backoff: self.api_timeout_retry_base_backoff,
            tool_timeout: self.tool_timeout,
            speech_output_dir: self.speech_output_dir.clone(),
            // #4810: every spawned agent owns its todo list. Cloning the
            // parent `Arc` here made child `work_update` / `todo_write` replace
            // the parent's Work checklist (and, through
            // `WorkRuntime::matches_todos`, the parent's work graph), so a
            // worker could silently erase the supervisor's plan and its
            // siblings' progress. Parent todo state is still visible to an
            // opt-in forked child as immutable `fork_context` text.
            todos: crate::tools::todo::new_shared_todo_list(),
            parent_mode: self.parent_mode,
            approval_mode: self.approval_mode,
            auto_review_policy: Arc::clone(&self.auto_review_policy),
            parent_can_prompt: self.parent_can_prompt,
            approval_receipt_store: self.approval_receipt_store.clone(),
        }
    }

    /// Whether the next spawn would exceed the depth cap.
    #[must_use]
    pub fn would_exceed_depth(&self) -> bool {
        self.spawn_depth + 1 > self.max_spawn_depth
    }
}

#[derive(Clone)]
struct SubAgentWorkLifecycle {
    work: SharedWorkRuntime,
    session_id: String,
    external: String,
}

impl SubAgentWorkLifecycle {
    fn register(runtime: &SubAgentRuntime, agent_id: &str, title: &str) -> Result<Option<Self>> {
        let Some(work) = runtime.context.runtime.work.clone() else {
            return Ok(None);
        };
        let session_id = runtime.context.state_namespace.clone();
        let external = format!("worker:{agent_id}");
        let lifecycle = Self {
            work,
            session_id,
            external,
        };
        lifecycle
            .work
            .register_operation(
                &lifecycle.session_id,
                OperationIntent::new(
                    lifecycle.external.clone(),
                    title,
                    true,
                    "agent",
                    format!("agent:{agent_id}:spawn"),
                ),
            )
            .map_err(|err| anyhow!("failed to register sub-agent work: {err}"))?;

        // A root Operate verification lease is provenance, not delegated
        // authority. Nested workers cannot inherit this bit.
        if runtime.accept_verification && runtime.spawn_depth == 1 {
            let reference = format!("operate-verification:{agent_id}");
            if let Err(err) = lifecycle.work.record_operation_approval(
                &lifecycle.session_id,
                &lifecycle.external,
                &reference,
                "agent",
                &format!("agent:{agent_id}:approval"),
            ) {
                let _ = lifecycle.reconcile_state(OwnerState::Failed, 1, None);
                return Err(anyhow!(
                    "failed to record sub-agent verification approval: {err}"
                ));
            }
        }
        Ok(Some(lifecycle))
    }

    fn reconcile_record(&self, record: &AgentWorkerRecord) -> Result<bool, String> {
        let Some(snapshot) = agent_worker_owner_snapshot(record) else {
            return Ok(false);
        };
        self.work.reconcile_operation(&self.session_id, snapshot)
    }

    fn reconcile_state(
        &self,
        state: OwnerState,
        seq: u64,
        output: Option<EvidenceRef>,
    ) -> Result<bool, String> {
        let observed_at = i64::try_from(epoch_millis_now()).unwrap_or(i64::MAX);
        let mut snapshot =
            OperationOwnerSnapshot::new(self.external.clone(), state, seq, observed_at);
        if let Some(output) = output {
            snapshot = snapshot.with_output(output);
        }
        self.work.reconcile_operation(&self.session_id, snapshot)
    }
}

/// Translate the persisted worker owner record after its manager has applied
/// restart recovery. A record without an event predates the lifecycle owner
/// protocol and cannot safely invent a sequence.
pub(crate) fn agent_worker_owner_snapshot(
    record: &AgentWorkerRecord,
) -> Option<OperationOwnerSnapshot> {
    let event = record.events.back()?;
    let output = record.result_summary.as_ref().and_then(|result| {
        EvidenceRef::new(
            EvidenceKind::Receipt {
                owner: "worker".to_string(),
            },
            format!("worker:{}:result", record.spec.worker_id),
            Some(u64::try_from(result.len()).unwrap_or(u64::MAX)),
            false,
        )
        .ok()
    });
    let observed_at = i64::try_from(event.timestamp_ms).unwrap_or(i64::MAX);
    let mut snapshot = OperationOwnerSnapshot::new(
        format!("worker:{}", record.spec.worker_id),
        owner_state_from_worker_status(record.status),
        event.seq,
        observed_at,
    );
    if let Some(output) = output {
        snapshot = snapshot.with_output(output);
    }
    Some(snapshot)
}

fn owner_state_from_worker_status(status: AgentWorkerStatus) -> OwnerState {
    match status {
        AgentWorkerStatus::Starting | AgentWorkerStatus::Running => OwnerState::Running,
        AgentWorkerStatus::Queued | AgentWorkerStatus::WaitingForUser => OwnerState::Waiting,
        AgentWorkerStatus::ModelWait | AgentWorkerStatus::RunningTool => OwnerState::Running,
        AgentWorkerStatus::Completed => OwnerState::Completed,
        AgentWorkerStatus::Failed | AgentWorkerStatus::Interrupted => OwnerState::Failed,
        AgentWorkerStatus::Cancelled => OwnerState::Cancelled,
    }
}

/// A running sub-agent instance.
pub struct SubAgent {
    pub id: String,
    pub session_name: String,
    pub fork_context: bool,
    pub agent_type: FleetRole,
    pub prompt: String,
    pub assignment: SubAgentAssignment,
    pub model: String,
    pub nickname: Option<String>,
    pub status: SubAgentStatus,
    pub result: Option<String>,
    pub steps_taken: u32,
    pub checkpoint: Option<SubAgentCheckpoint>,
    pub needs_input: Option<SubAgentNeedsInput>,
    pub started_at: Instant,
    pub last_activity_at: Instant,
    /// `None` = full registry inheritance, with approval-gated tools still
    /// blocked unless the parent runtime is auto-approved.
    /// `Some(list)` = explicit narrow allowlist (Custom agents, legacy).
    pub allowed_tools: Option<Vec<String>>,
    /// Stable id of the manager that spawned this agent (#405). Compared
    /// against the manager's `current_session_boot_id` to classify the
    /// agent as in-session vs prior-session at list time.
    pub session_boot_id: String,
    /// Immutable root conversation owner. Empty is a legacy/unattached value
    /// and must never match an active session.
    owner_session_id: String,
    pub workspace: PathBuf,
    /// Internal completion/cancellation arbitration bit. While set, the task
    /// has won the right to publish its terminal notifications, but the public
    /// status deliberately remains `Running` until those notifications are
    /// queued (#1961). Competing cancellation/interrupt paths must treat the
    /// claim as terminal ownership and leave the task to finalize.
    completion_claimed: bool,
    /// Process-local terminal fan-in sinks. Never serialized; restored agents
    /// have no live parent/mailbox/event consumers and are reconciled directly
    /// to interrupted state during load.
    terminal_delivery: Option<SubAgentTerminalDeliveryContext>,
    work_lifecycle: Option<SubAgentWorkLifecycle>,
    input_tx: Option<mpsc::UnboundedSender<SubAgentInput>>,
    task_handle: Option<JoinHandle<()>>,
}

impl SubAgent {
    /// Create a new sub-agent. The `id` is generated by the caller so that
    /// deterministic whale-naming can hash the ID before construction.
    #[allow(clippy::too_many_arguments)]
    fn new(
        id: String,
        agent_type: FleetRole,
        prompt: String,
        assignment: SubAgentAssignment,
        model: String,
        nickname: Option<String>,
        allowed_tools: Option<Vec<String>>,
        input_tx: mpsc::UnboundedSender<SubAgentInput>,
        workspace: PathBuf,
        session_boot_id: String,
    ) -> Self {
        let session_name = id.clone();

        let started_at = Instant::now();
        Self {
            id,
            session_name,
            fork_context: false,
            agent_type,
            prompt,
            assignment,
            model,
            nickname,
            status: SubAgentStatus::Running,
            result: None,
            steps_taken: 0,
            checkpoint: None,
            needs_input: None,
            started_at,
            last_activity_at: started_at,
            allowed_tools,
            session_boot_id,
            owner_session_id: String::new(),
            workspace,
            completion_claimed: false,
            terminal_delivery: None,
            work_lifecycle: None,
            input_tx: Some(input_tx),
            task_handle: None,
        }
    }

    /// Get a snapshot of the current state.
    #[must_use]
    pub fn snapshot(&self) -> SubAgentResult {
        SubAgentResult {
            name: self.session_name.clone(),
            agent_id: self.id.clone(),
            context_mode: if self.fork_context { "forked" } else { "fresh" }.to_string(),
            fork_context: self.fork_context,
            workspace: Some(self.workspace.clone()),
            git_branch: current_git_branch(&self.workspace),
            agent_type: self.agent_type.clone(),
            assignment: self.assignment.clone(),
            model: self.model.clone(),
            nickname: self.nickname.clone(),
            status: self.status.clone(),
            worker_status: None,
            runtime_permissions: None,
            parent_run_id: None,
            spawn_depth: 0,
            child_route: None,
            result: self.result.clone(),
            steps_taken: self.steps_taken,
            checkpoint: self.checkpoint.clone(),
            needs_input: self.needs_input.clone(),
            duration_ms: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            started_at: Some(self.started_at),
            // Snapshots from the agent itself don't know the manager's
            // current boot id, so default to false. The manager fills
            // this in when it produces a snapshot via its own
            // `snapshot_for_listing` helper (#405).
            from_prior_session: false,
        }
    }
}

/// Manager for active sub-agents.
struct CoordinationProcessLock {
    release: Option<std::sync::mpsc::SyncSender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Marker prefix for a coordination-lock failure whose holder is THIS
/// process: an engine swap (model/provider switch) constructs the new
/// engine's manager while the old engine still holds the flock on its own
/// fd, and flock treats a second descriptor in the same process as a
/// conflict. This is a transient handover that self-heals on the next
/// projection retry (#5036) — UI surfaces must not present it as "another
/// Codewhale process" (owner report, 2026-08-04).
pub const COORDINATION_SAME_PROCESS_HANDOVER: &str =
    "handing delegated coordination between engines in this process";

/// Marker inside a lock-acquisition error meaning the flock wait expired
/// rather than that another session holds it. The status strip distinguishes
/// the two, because "a second session is open here" and "the claim timed out"
/// call for different responses.
pub const COORDINATION_LOCK_TIMEOUT_MARKER: &str =
    "timed out acquiring delegated coordination lock";

impl CoordinationProcessLock {
    fn acquire(state_root: &Path) -> Result<Self> {
        let requested_root = normalize_subagent_workspace(state_root);
        let lock_dir = requested_root.join(".codewhale").join("state");
        fs::create_dir_all(&lock_dir)?;
        // Creating a missing root can change its canonical spelling on
        // Windows (for example by adding a `\\?\` prefix). Re-resolve both
        // sides before checking containment.
        let state_root = normalize_subagent_workspace(state_root);
        let lock_path = state_root
            .join(".codewhale")
            .join("state")
            .join(SUBAGENT_STATE_LOCK_FILE);
        reject_root_relative_symlinks(&state_root, &lock_path)?;
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let thread = std::thread::spawn(move || {
            let lock = fd_lock::RwLock::new(lock_file);
            // Changed from try_write (exclusive) to try_read (shared) so two
            // sessions in the same state root can coexist without a sticky
            // error — job rows still settle via atomic rename + sequence guard.
            // Matches user intent: "no reason to have a lock like that".
            match lock.try_read() {
                Ok(guard) => {
                    // Shared lock: multiple sessions coexist, no pid stamping needed.
                    // The exclusive pid dance was for telling same-process handover
                    // from foreign owner — with shared locks every holder is equal
                    // and job rows settle via atomic rename, so we just hold the
                    // shared flock for the session lifetime.
                    let _guard = guard;
                    let _ = ready_tx.send(Ok::<(), String>(()));
                    let _ = release_rx.recv();
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                }
            }
        });
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                release: Some(release_tx),
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                let holder_pid = std::fs::read_to_string(&lock_path)
                    .ok()
                    .and_then(|contents| contents.trim().parse::<u32>().ok());
                if holder_pid == Some(std::process::id()) {
                    Err(anyhow!(
                        "{COORDINATION_SAME_PROCESS_HANDOVER} for {}: {error}",
                        state_root.display()
                    ))
                } else {
                    Err(anyhow!(
                        "another Codewhale process{} owns delegated coordination for {}: {error}",
                        holder_pid
                            .map(|pid| format!(" (pid {pid})"))
                            .unwrap_or_default(),
                        state_root.display()
                    ))
                }
            }
            Err(error) => {
                drop(release_tx);
                let _ = thread.join();
                Err(anyhow!(
                    "{COORDINATION_LOCK_TIMEOUT_MARKER} for {}: {error}",
                    state_root.display()
                ))
            }
        }
    }
}

impl Drop for CoordinationProcessLock {
    fn drop(&mut self) {
        self.release.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub struct SubAgentManager {
    agents: HashMap<String, SubAgent>,
    worker_records: HashMap<String, AgentWorkerRecord>,
    worker_event_seq: u64,
    persist_sequence: std::sync::atomic::AtomicU64,
    coordination: CoordinationLedger,
    #[allow(dead_code)] // Stored for future workspace-scoped operations
    workspace: PathBuf,
    /// Root that owns the delegated-agent ledger, complete transcript
    /// artifacts and coordination lock. Defaults to `workspace`; embedders may
    /// isolate it without changing child execution cwd or file authority.
    state_root: PathBuf,
    state_path: Option<PathBuf>,
    coordination_process_lock: std::sync::Mutex<Option<CoordinationProcessLock>>,
    coordination_process_lock_required: bool,
    /// Configured default per-child model-turn budget (`[subagents]
    /// default_max_steps`, #5324). `None` keeps the unbounded Fleet default;
    /// an explicit spawn `max_steps` still wins. Zero means unbounded.
    max_steps: Option<u32>,
    /// Configured default per-child wall-clock budget (`[subagents]
    /// default_wall_time_secs`, #5324). `None` keeps
    /// `DEFAULT_CHILD_WALL_TIME`; an explicit spawn `wall_time_secs` still
    /// wins.
    wall_time: Option<Duration>,
    max_agents: usize,
    max_admitted_agents: usize,
    default_token_budget: Option<u64>,
    running_heartbeat_timeout: Duration,
    /// Stable id assigned at manager construction (#405). Stamped on
    /// every agent the manager spawns; agents loaded from the
    /// persisted state file carry whatever id the prior session
    /// stamped (or empty for pre-#405 records). The manager classifies
    /// agents whose `session_boot_id` doesn't match this value as
    /// "from prior session" so listings can hide them by default.
    current_session_boot_id: String,
    /// Launch gate for direct (depth-1) sub-agent launches (#3095). Each
    /// permit is one actively executing direct child; further direct
    /// children spawn immediately but queue for a permit before starting,
    /// publishing a visible "queued" reason instead of bursting. Deeper
    /// descendants bypass the gate so a permit-holding parent waiting on
    /// its own children cannot deadlock the tree.
    launch_gate: Arc<Semaphore>,
    /// #freeze: hot-path persist debounce bookkeeping (see
    /// `SUBAGENT_PERSIST_DEBOUNCE`). `last_persist_at` is the last time any
    /// state persist ran; `persist_pending` records that a hot-path write was
    /// coalesced away so a later flush (terminal write or shutdown) can
    /// capture the most recent checkpoint.
    last_persist_at: Option<Instant>,
    persist_pending: bool,
    /// #3803: last time `cleanup` ran. The sidebar refresh (`Op::ListSubAgents`)
    /// renders from a read-only `list()` snapshot and only runs the
    /// write-locked `cleanup` on a bounded cadence, so a UI refresh storm during
    /// a sub-agent fanout no longer contends for the write lock on every request.
    last_cleanup_at: Option<Instant>,
    /// Parent mail queued by `agents/message` without waking the child.
    /// `agents/followup` drains into `input_tx` when a live wake is possible.
    queued_mail: HashMap<String, VecDeque<QueuedParentMessage>>,
    /// Follow-ups handed to a running child's live input channel that the
    /// child has not yet taken at a round boundary. Read by the rail rows.
    pending_follow_ups: HashMap<String, Arc<std::sync::atomic::AtomicUsize>>,
    /// Test/observability: agent ids that received a live wake via followup.
    woken_agents: HashMap<String, bool>,
    /// Agent ids whose handle-store entries should be evicted on the next async
    /// drain. Populated by `cleanup()` when an agent record is retired; drained
    /// by async callers that hold the `HandleStore` lock (#3885).
    pending_handle_evictions: Vec<(String, String)>,
    /// Checkpoint-resume idempotency map: interrupted agent id -> the agent
    /// id of the session resumed from its checkpoint. A second followup on
    /// the same interrupted id returns the existing resumed target instead of
    /// spawning a duplicate agent loop (duplicate-resume guard).
    resume_targets: HashMap<String, String>,
    /// Approval prompts raised on a child's behalf under Ask: the approval id
    /// the host sees (`agent:<agent_id>:approval:<boot_id>:<n>`) → the
    /// waiting child. The engine
    /// routes the person's decision here; a decision for an id nobody is
    /// waiting on is dropped, never applied to a different call.
    child_approvals: HashMap<String, tokio::sync::oneshot::Sender<ChildApprovalOutcome>>,
    child_approval_seq: u64,
}

/// A person's answer to an approval prompt raised for a child's tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildApprovalOutcome {
    Approved,
    Denied,
}

impl SubAgentManager {
    /// Register a prompt raised for a child's held call. Returns the approval
    /// id to publish and the receiver the child awaits.
    pub fn register_child_approval(
        &mut self,
        agent_id: &str,
    ) -> (String, tokio::sync::oneshot::Receiver<ChildApprovalOutcome>) {
        self.child_approval_seq = self.child_approval_seq.wrapping_add(1);
        // Namespace with the manager's boot id (#5615): the sequence restarts
        // with every manager, and a resumed agent under a new manager would
        // otherwise reuse ids from an earlier lifecycle. Durable approval
        // receipts (#5584) make a stale id a live hazard — the old receipt
        // could auto-answer the new prompt.
        let id = format!(
            "agent:{agent_id}:approval:{}:{}",
            self.current_session_boot_id, self.child_approval_seq
        );
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.child_approvals.insert(id.clone(), tx);
        (id, rx)
    }

    /// Whether an approval id belongs to a child prompt (routing hint for the
    /// engine before it consults the map).
    #[must_use]
    pub fn is_child_approval_id(id: &str) -> bool {
        id.starts_with("agent:") && id.contains(":approval:")
    }

    /// Deliver a person's decision to the waiting child. Returns `false` when
    /// no child is waiting on that id (already answered, cancelled, or not a
    /// child prompt).
    pub fn resolve_child_approval(&mut self, id: &str, outcome: ChildApprovalOutcome) -> bool {
        match self.child_approvals.remove(id) {
            Some(tx) => tx.send(outcome).is_ok(),
            None => false,
        }
    }

    /// Forget a prompt the child stopped waiting for (cancellation).
    pub fn cancel_child_approval(&mut self, id: &str) {
        self.child_approvals.remove(id);
    }

    /// Number of child prompts currently awaiting a person.
    #[must_use]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn pending_child_approvals(&self) -> usize {
        self.child_approvals.len()
    }

    /// Create a new manager for sub-agents.
    #[must_use]
    pub fn new(workspace: PathBuf, max_agents: usize) -> Self {
        let state_root = workspace.clone();
        Self::new_with_state_root(workspace, state_root, max_agents)
    }

    /// Create a manager whose delegated control-plane state is rooted
    /// separately from its execution workspace.
    #[must_use]
    pub fn new_with_state_root(workspace: PathBuf, state_root: PathBuf, max_agents: usize) -> Self {
        Self {
            agents: HashMap::new(),
            worker_records: HashMap::new(),
            worker_event_seq: 0,
            persist_sequence: std::sync::atomic::AtomicU64::new(0),
            coordination: CoordinationLedger::default(),
            workspace,
            state_root,
            state_path: None,
            coordination_process_lock: std::sync::Mutex::new(None),
            coordination_process_lock_required: false,
            max_steps: None,
            wall_time: None,
            max_agents,
            max_admitted_agents: max_agents,
            default_token_budget: None,
            running_heartbeat_timeout: Duration::from_secs(
                crate::config::DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS,
            ),
            // Fresh boot id per manager. Used by #405 to classify
            // re-loaded persisted agents as "prior session".
            current_session_boot_id: format!("boot_{}", &Uuid::new_v4().to_string()[..12]),
            // Default launch concurrency = the full agent cap; the gate only
            // throttles when a lower `launch_concurrency` is configured.
            launch_gate: Arc::new(Semaphore::new(max_agents.max(1))),
            last_persist_at: None,
            persist_pending: false,
            last_cleanup_at: None,
            queued_mail: HashMap::new(),
            pending_follow_ups: HashMap::new(),
            woken_agents: HashMap::new(),
            pending_handle_evictions: Vec::new(),
            resume_targets: HashMap::new(),
            child_approvals: HashMap::new(),
            child_approval_seq: 0,
        }
    }

    /// Set the number of direct children that may execute concurrently
    /// before further launches queue (#3095). Clamped to `1..=max_agents`.
    #[must_use]
    pub fn with_launch_concurrency(mut self, limit: usize) -> Self {
        self.launch_gate = Arc::new(Semaphore::new(limit.clamp(1, self.max_agents)));
        self
    }

    /// Set the total queued + running admission ceiling for this manager.
    /// The value is always at least the instantaneous concurrency cap.
    #[must_use]
    pub fn with_admission_limit(mut self, max_admitted: usize) -> Self {
        self.max_admitted_agents =
            max_admitted.clamp(self.max_agents, crate::config::MAX_SUBAGENT_ADMISSION);
        self
    }

    /// Set the default aggregate token budget for root sub-agent runs.
    /// `None` and `Some(0)` both preserve unlimited legacy behavior.
    #[must_use]
    pub fn with_default_token_budget(mut self, budget: Option<u64>) -> Self {
        self.default_token_budget = positive_token_budget(budget);
        self
    }

    /// Set the configured default per-child model-turn budget applied when a
    /// spawn carries no explicit `max_steps` (#5324). `None` keeps the
    /// unbounded default; a positive explicit/configured value still caps it.
    #[must_use]
    pub fn with_default_max_steps(mut self, max_steps: Option<u32>) -> Self {
        self.max_steps = max_steps;
        self
    }

    /// Set the configured default per-child wall-clock budget applied when a
    /// spawn carries no explicit `wall_time_secs` (#5324). `None` keeps
    /// `DEFAULT_CHILD_WALL_TIME`.
    #[must_use]
    pub fn with_default_wall_time(mut self, wall_time: Option<Duration>) -> Self {
        self.wall_time = wall_time;
        self
    }

    /// Return the boot id this manager stamps on agents it spawns.
    /// Exposed for tests; internal callers use the field directly.
    #[cfg(test)]
    pub fn session_boot_id(&self) -> &str {
        &self.current_session_boot_id
    }

    pub fn record_coordination_decision(
        &mut self,
        decision: DecisionRecord,
    ) -> Result<DecisionRecord, String> {
        self.ensure_coordination_process_lock()?;
        self.coordination.record_decision(decision)
    }

    pub(crate) fn stamp_coordination_sequence_for_session(
        &mut self,
        sequence: u64,
        active_session_id: &str,
    ) -> Result<(), String> {
        if active_session_id.is_empty() {
            return Err("active session id is empty".to_string());
        }
        self.coordination
            .record_sessions
            .insert(sequence, active_session_id.to_string());
        Ok(())
    }

    fn coordination_record_is_owned_by_session(
        &self,
        active_session_id: &str,
        sequence: u64,
        owner: Option<&str>,
    ) -> bool {
        self.coordination
            .record_sessions
            .get(&sequence)
            .is_some_and(|session_id| session_id == active_session_id)
            || owner
                .is_some_and(|owner| self.agent_id_is_owned_by_session(owner, active_session_id))
    }

    pub(crate) fn coordination_decision_is_owned_by_session(
        &self,
        active_session_id: &str,
        decision_id: &str,
    ) -> bool {
        self.coordination.decisions.iter().any(|record| {
            record.decision_id == decision_id
                && self.coordination_record_is_owned_by_session(
                    active_session_id,
                    record.sequence,
                    Some(&record.owner),
                )
        })
    }

    pub fn update_coordination_decision(
        &mut self,
        decision_id: &str,
        status: DecisionStatus,
        owner: &str,
        expected_version: u32,
    ) -> Result<DecisionRecord, String> {
        self.ensure_coordination_process_lock()?;
        self.coordination
            .update_decision_status(decision_id, status, owner, expected_version)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn reconcile_coordination(
        &mut self,
        subject: String,
        owner: String,
        input_decisions: Vec<String>,
        outcome: String,
        evidence_handles: Vec<String>,
        candidate_handles: Vec<String>,
        retry_count: u32,
        retry_limit: u32,
        reviewer_evidence_handles: Vec<String>,
        verifier_evidence_handles: Vec<String>,
        verification_outcome: String,
    ) -> Result<ReconciliationReceipt, String> {
        self.ensure_coordination_process_lock()?;
        let expected_owner = self.nearest_common_fan_in_owner(&input_decisions)?;
        if owner != expected_owner {
            return Err(format!(
                "neutral fan-in must be owned by nearest common Planner/release owner '{expected_owner}', not '{owner}'"
            ));
        }
        let reviewer_ids = self.validate_reconciliation_role_evidence(
            &reviewer_evidence_handles,
            FleetRole::Reviewer,
            "Reviewer",
        )?;
        let verifier_ids = self.validate_reconciliation_role_evidence(
            &verifier_evidence_handles,
            FleetRole::Verifier,
            "Verifier",
        )?;
        if !reviewer_ids.is_disjoint(&verifier_ids) {
            return Err(
                "Reviewer and Verifier evidence must come from distinct workers".to_string(),
            );
        }
        let candidate_owners = input_decisions
            .iter()
            .filter_map(|decision_id| {
                self.coordination
                    .decisions
                    .iter()
                    .find(|decision| &decision.decision_id == decision_id)
                    .map(|decision| decision.owner.as_str())
            })
            .collect::<BTreeSet<_>>();
        if reviewer_ids
            .iter()
            .chain(verifier_ids.iter())
            .any(|worker| worker == &owner || candidate_owners.contains(worker.as_str()))
        {
            return Err(
                "Reviewer/Verifier evidence workers must be independent of the neutral owner and candidate authors"
                    .to_string(),
            );
        }
        self.coordination.reconcile(
            subject,
            owner,
            input_decisions,
            outcome,
            evidence_handles,
            candidate_handles,
            retry_count,
            retry_limit,
            reviewer_evidence_handles,
            verifier_evidence_handles,
            verification_outcome,
        )
    }

    fn validate_reconciliation_role_evidence(
        &self,
        handles: &[String],
        expected: FleetRole,
        label: &str,
    ) -> Result<BTreeSet<String>, String> {
        if handles.is_empty() {
            return Err(format!("neutral fan-in requires {label} evidence"));
        }
        let mut workers = BTreeSet::new();
        for handle in handles {
            let reference = handle
                .strip_prefix("agent:")
                .and_then(|rest| rest.split([':', '@', '#']).next())
                .unwrap_or(handle)
                .trim();
            let Some((worker_id, record)) = self.worker_record_by_ref(reference) else {
                return Err(format!(
                    "{label} evidence handle '{handle}' does not identify a persisted worker"
                ));
            };
            let role_matches = record.spec.agent_type == expected
                || record.spec.role.as_deref().is_some_and(|role| {
                    role.trim().eq_ignore_ascii_case(label)
                        || (expected == FleetRole::Reviewer
                            && role.trim().eq_ignore_ascii_case("reviewer"))
                        || (expected == FleetRole::Verifier
                            && role.trim().eq_ignore_ascii_case("verifier"))
                });
            if !role_matches || record.status != AgentWorkerStatus::Completed {
                return Err(format!(
                    "{label} evidence worker '{worker_id}' must have the {label} role and completed status"
                ));
            }
            workers.insert(worker_id);
        }
        Ok(workers)
    }

    fn nearest_common_fan_in_owner(&self, input_decisions: &[String]) -> Result<String, String> {
        let decision_owners = input_decisions
            .iter()
            .map(|decision_id| {
                self.coordination
                    .decisions
                    .iter()
                    .find(|decision| &decision.decision_id == decision_id)
                    .map(|decision| decision.owner.clone())
                    .ok_or_else(|| {
                        format!("reconciliation references unknown decision '{decision_id}'")
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if decision_owners.len() < 2 {
            return Err("neutral fan-in requires at least two input decisions".to_string());
        }

        let ancestry = decision_owners
            .iter()
            .map(|owner| self.worker_ancestry(owner))
            .collect::<Vec<_>>();
        let Some(first) = ancestry.first() else {
            return Ok("root".to_string());
        };
        for candidate in first {
            if decision_owners.contains(candidate)
                || !ancestry
                    .iter()
                    .skip(1)
                    .all(|chain| chain.contains(candidate))
            {
                continue;
            }
            if candidate == "root" || self.worker_is_fan_in_owner(candidate) {
                return Ok(candidate.clone());
            }
        }
        Ok("root".to_string())
    }

    fn worker_ancestry(&self, owner: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut cursor = Some(owner.to_string());
        while let Some(reference) = cursor.take() {
            let Some((worker_id, record)) = self.worker_record_by_ref(&reference) else {
                break;
            };
            if chain.contains(&worker_id) {
                break;
            }
            chain.push(worker_id);
            cursor = record.parent_run_id.clone();
        }
        if !chain.iter().any(|entry| entry == "root") {
            chain.push("root".to_string());
        }
        chain
    }

    fn worker_record_by_ref(&self, reference: &str) -> Option<(String, &AgentWorkerRecord)> {
        self.worker_records
            .get(reference)
            .map(|record| (reference.to_string(), record))
            .or_else(|| {
                self.worker_records.iter().find_map(|(worker_id, record)| {
                    (record.spec.run_id == reference).then(|| (worker_id.clone(), record))
                })
            })
    }

    fn worker_is_fan_in_owner(&self, reference: &str) -> bool {
        self.worker_record_by_ref(reference)
            .is_some_and(|(_, record)| {
                record.spec.agent_type == FleetRole::Planner
                    || record.spec.role.as_deref().is_some_and(|role| {
                        matches!(
                            role.trim().to_ascii_lowercase().as_str(),
                            "planner" | "manager" | "operator" | "release-owner"
                        )
                    })
            })
    }

    #[must_use]
    pub fn coordination_detail_projection(
        &self,
        subject: Option<&str>,
        limit: usize,
    ) -> CoordinationDetailProjection {
        let limit = limit.clamp(1, coord::COORDINATION_RECORD_LIMIT);
        let matches_subject = |value: &str| subject.is_none_or(|subject| value == subject);
        let decisions = self
            .coordination
            .decisions
            .iter()
            .rev()
            .filter(|decision| matches_subject(&decision.subject))
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let reconciliations = self
            .coordination
            .reconciliations
            .iter()
            .rev()
            .filter(|receipt| matches_subject(&receipt.subject))
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let claims = self
            .coordination
            .write_claims
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let projections = self
            .coordination
            .projections
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let contentions = self
            .coordination
            .contentions
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let active_owners = self.active_coordination_owners();
        let mut hot_path_counts = std::collections::BTreeMap::<String, usize>::new();
        for record in self
            .coordination
            .write_claims
            .iter()
            .filter(|record| active_owners.contains(&record.claim.owner))
        {
            for root in &record.claim.roots {
                *hot_path_counts.entry(root.clone()).or_default() += 1;
            }
            for file in &record.claim.exact_files {
                *hot_path_counts.entry(file.clone()).or_default() += 1;
            }
        }
        let mut hottest_paths = hot_path_counts.into_iter().collect::<Vec<_>>();
        hottest_paths.sort_by(|(path_a, count_a), (path_b, count_b)| {
            count_b.cmp(count_a).then_with(|| path_a.cmp(path_b))
        });
        hottest_paths.truncate(limit.min(8));
        CoordinationDetailProjection {
            schema_version: self.coordination.schema_version,
            sequence: self.coordination.sequence,
            decisions,
            write_claims: claims,
            reconciliations,
            context_projections: projections,
            contentions,
            metrics: CoordinationDetailMetrics {
                hottest_paths: hottest_paths
                    .into_iter()
                    .map(|(path, active_claims)| CoordinationHotPath {
                        path,
                        active_claims,
                    })
                    .collect(),
                package_or_module_growth: None,
                route_or_cost: None,
                note: "growth and route/cost stay null when the coordination ledger has no authoritative source".to_string(),
            },
            bounded: true,
            limit,
            process_lock_held: {
                // Retry acquire on each projection so a session that lost the
                // flock at construction recovers once the previous owner exits.
                let _ = self.coordination_process_lock_status();
                self.holds_coordination_process_lock()
            },
            process_lock_note: if self.holds_coordination_process_lock() {
                None
            } else {
                self.coordination_process_lock_status().err()
            },
        }
    }

    #[must_use]
    pub(crate) fn coordination_detail_projection_for_session(
        &self,
        active_session_id: &str,
        subject: Option<&str>,
        limit: usize,
    ) -> CoordinationDetailProjection {
        let mut projection = self.coordination_detail_projection(subject, limit);
        let record_matches = |sequence: u64, owner: Option<&str>| {
            self.coordination_record_is_owned_by_session(active_session_id, sequence, owner)
        };

        projection
            .decisions
            .retain(|record| record_matches(record.sequence, Some(&record.owner)));
        let visible_decision_ids = projection
            .decisions
            .iter()
            .map(|record| record.decision_id.clone())
            .collect::<HashSet<_>>();
        projection
            .write_claims
            .retain(|record| record_matches(record.sequence, Some(record.claim.owner.as_str())));
        projection.reconciliations.retain(|record| {
            record_matches(record.sequence, Some(&record.owner))
                && record
                    .input_decisions
                    .iter()
                    .all(|decision_id| visible_decision_ids.contains(decision_id))
        });
        projection.context_projections.retain_mut(|record| {
            if !record_matches(record.sequence, Some(&record.child_id)) {
                return false;
            }
            record
                .decision_ids
                .retain(|decision_id| visible_decision_ids.contains(decision_id));
            true
        });
        projection.contentions.retain(|record| {
            record_matches(record.sequence, Some(&record.claimant))
                && self.agent_id_is_owned_by_session(&record.conflicting_owner, active_session_id)
        });

        let mut hot_path_counts = std::collections::BTreeMap::<String, usize>::new();
        for record in &projection.write_claims {
            for root in &record.claim.roots {
                *hot_path_counts.entry(root.clone()).or_default() += 1;
            }
            for file in &record.claim.exact_files {
                *hot_path_counts.entry(file.clone()).or_default() += 1;
            }
        }
        let mut hottest_paths = hot_path_counts.into_iter().collect::<Vec<_>>();
        hottest_paths.sort_by(|(path_a, count_a), (path_b, count_b)| {
            count_b.cmp(count_a).then_with(|| path_a.cmp(path_b))
        });
        hottest_paths.truncate(projection.limit.min(8));
        projection.metrics.hottest_paths = hottest_paths
            .into_iter()
            .map(|(path, active_claims)| CoordinationHotPath {
                path,
                active_claims,
            })
            .collect();
        projection.sequence = projection
            .decisions
            .iter()
            .map(|record| record.sequence)
            .chain(projection.write_claims.iter().map(|record| record.sequence))
            .chain(
                projection
                    .reconciliations
                    .iter()
                    .map(|record| record.sequence),
            )
            .chain(
                projection
                    .context_projections
                    .iter()
                    .map(|record| record.sequence),
            )
            .chain(projection.contentions.iter().map(|record| record.sequence))
            .max()
            .unwrap_or_default();
        projection
    }

    #[must_use]
    #[cfg(test)]
    pub fn inspect_coordination(&self, subject: Option<&str>, limit: usize) -> Value {
        serde_json::to_value(self.coordination_detail_projection(subject, limit))
            .expect("typed coordination projection is serializable")
    }

    #[must_use]
    pub(crate) fn inspect_coordination_for_session(
        &self,
        active_session_id: &str,
        subject: Option<&str>,
        limit: usize,
    ) -> Value {
        serde_json::to_value(self.coordination_detail_projection_for_session(
            active_session_id,
            subject,
            limit,
        ))
        .expect("typed coordination projection is serializable")
    }

    pub fn expand_write_claim(
        &mut self,
        owner: &str,
        roots: Vec<String>,
        exact_files: Vec<String>,
        contracts: Vec<String>,
    ) -> Result<PersistedWriteClaim, String> {
        self.ensure_coordination_process_lock()?;
        let Some(existing) = self
            .coordination
            .write_claims
            .iter()
            .find(|record| record.claim.owner == owner)
            .cloned()
        else {
            return Err(format!("agent '{owner}' has no write claim to expand"));
        };
        let mut claim = existing.claim;
        let (roots, exact_files) = self.namespace_claim_paths_for_owner(
            owner,
            existing.isolated_worktree,
            roots,
            exact_files,
        )?;
        for root in roots {
            let root = normalize_claim_path(&root)?;
            if !claim.roots.contains(&root) {
                claim.roots.push(root);
            }
        }
        for file in exact_files {
            let file = normalize_claim_path(&file)?;
            if !claim.exact_files.contains(&file) {
                claim.exact_files.push(file);
            }
        }
        for contract in contracts
            .into_iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            if !claim.contracts.contains(&contract) {
                claim.contracts.push(contract);
            }
        }
        let active_owners = self.active_coordination_owners();
        self.coordination
            .register_claim(claim, existing.isolated_worktree, |candidate| {
                active_owners.contains(candidate)
            })
    }

    /// Release write claims whose owner is no longer a live claimant.
    ///
    /// The operator-of-record path for #5562: `coordinate inspect` shows the
    /// standing claims, and `coordinate release` clears the stale ones after
    /// the manager agrees no live claimant is being touched. An optional owner
    /// restricts the sweep to one agent; live claims are never removed.
    pub(crate) fn release_stale_write_claims(
        &mut self,
        owner: Option<String>,
    ) -> Result<Vec<String>, String> {
        self.ensure_coordination_process_lock()?;
        let active = self.active_coordination_owners();
        let released = self
            .coordination
            .release_stale_claims(owner.as_deref(), |candidate| active.contains(candidate))?;
        self.persist_state_synchronously()
            .map_err(|error| error.to_string())?;
        Ok(released)
    }

    fn validate_write_scope(&self, owner: &str, paths: &[String]) -> Result<(), String> {
        let Some(claim) = self
            .coordination
            .write_claims
            .iter()
            .find(|record| record.claim.owner == owner)
        else {
            return Err(format!(
                "agent '{owner}' has no registered write claim; declare scope at launch before mutation"
            ));
        };
        let (_, paths) = self.namespace_claim_paths_for_owner(
            owner,
            claim.isolated_worktree,
            Vec::new(),
            paths.to_vec(),
        )?;
        if let Some(path) = paths.iter().find(|path| !claim.claim.contains_path(path)) {
            return Err(format!(
                "write '{path}' is outside agent '{owner}' scope (roots: {:?}, files: {:?}); expand it first with agent action=claim",
                claim.claim.roots, claim.claim.exact_files
            ));
        }
        Ok(())
    }

    fn shared_write_claim(&self, owner: &str) -> Option<&PersistedWriteClaim> {
        self.coordination
            .write_claims
            .iter()
            .find(|record| record.claim.owner == owner && !record.isolated_worktree)
    }

    /// The live peers whose shared-checkout write claims gate `owner` — the
    /// names the contention refusal must surface so a refused child knows
    /// what it is waiting on and what to cancel or release.
    ///
    /// Liveness is the load-bearing part. Claims outlive the agents that
    /// registered them, so a workspace accumulates one per builder that ever
    /// ran: six completed agents left four standing claims in testing. Counting
    /// those made every later builder look contended by children that had long
    /// since finished, and the contention only ever grew. A terminal owner
    /// cannot write anything, so its claim cannot contend.
    ///
    /// The same single predicate as claim admission (#5562): an agent restored
    /// `Running` from a prior session (crash leftover) and an owner whose agent
    /// record is not loaded are both non-live — the per-workspace coordination
    /// process lock means no other process is writing concurrently, so there is
    /// no live peer to fail closed against.
    ///
    /// Worktree-isolated peers are excluded too: they write into their own
    /// checkout and can never contend for these paths.
    fn live_peer_shared_write_claim_owners(&self, owner: &str) -> Vec<String> {
        self.coordination
            .write_claims
            .iter()
            .filter(|record| record.claim.owner != owner && !record.isolated_worktree)
            .filter(|record| self.is_live_coordination_owner(&record.claim.owner))
            .map(|record| record.claim.owner.clone())
            .collect()
    }

    /// Classify an agent by its `session_boot_id`: `true` when the
    /// agent was either (a) loaded from disk with no id, or (b) carries
    /// a different id than the manager's current boot. Filters
    /// listing output by default (#405).
    fn is_from_prior_session(&self, agent: &SubAgent) -> bool {
        agent.session_boot_id.is_empty() || agent.session_boot_id != self.current_session_boot_id
    }

    #[must_use]
    fn with_state_path(mut self, path: PathBuf) -> Self {
        self.state_path = Some(path);
        self
    }

    fn require_coordination_process_lock(mut self) -> Self {
        self.coordination_process_lock_required = true;
        *self
            .coordination_process_lock
            .get_mut()
            .expect("coordination lock slot poisoned") =
            CoordinationProcessLock::acquire(&self.state_root).ok();
        self
    }

    /// A failed acquisition is never terminal: the previous owner may have
    /// exited since the last attempt, and the flock is held on an open fd —
    /// deleting the lock file does not release it, so the only correct
    /// recovery is to re-attempt acquisition on each use (#5036).
    fn ensure_coordination_process_lock(&self) -> Result<(), String> {
        if !self.coordination_process_lock_required {
            return Ok(());
        }
        let mut slot = self
            .coordination_process_lock
            .lock()
            .expect("coordination lock slot poisoned");
        if slot.is_some() {
            return Ok(());
        }
        match CoordinationProcessLock::acquire(&self.state_root) {
            Ok(lock) => {
                *slot = Some(lock);
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    /// Whether this process currently owns the workspace coordination flock.
    /// Read-only inspection of the in-process slot — does not attempt acquire.
    #[must_use]
    pub fn holds_coordination_process_lock(&self) -> bool {
        if !self.coordination_process_lock_required {
            // Tests and non-durable managers do not require the flock.
            return true;
        }
        self.coordination_process_lock
            .lock()
            .expect("coordination lock slot poisoned")
            .is_some()
    }

    /// Probe lock ownership for UI/inspect surfaces. Retries acquire when the
    /// slot is empty so a session that started without the lock can recover
    /// after the previous owner exits (#2.6 / #5036).
    pub fn coordination_process_lock_status(&self) -> Result<(), String> {
        self.ensure_coordination_process_lock()
    }

    #[must_use]
    pub fn with_running_heartbeat_timeout(mut self, timeout: Duration) -> Self {
        self.running_heartbeat_timeout = if timeout.is_zero() {
            Duration::from_secs(crate::config::DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS)
        } else {
            timeout
        };
        self
    }

    /// Apply live runtime limits. The launch semaphore is replaced only when
    /// no sub-agent is currently running, because active tasks may still hold
    /// permits from the previous semaphore.
    pub fn update_runtime_limits(
        &mut self,
        max_agents: usize,
        max_admitted_agents: usize,
        running_heartbeat_timeout: Duration,
        launch_concurrency: usize,
        default_token_budget: Option<u64>,
    ) -> bool {
        self.max_agents = max_agents.clamp(1, crate::config::MAX_SUBAGENTS);
        self.max_admitted_agents =
            max_admitted_agents.clamp(self.max_agents, crate::config::MAX_SUBAGENT_ADMISSION);
        self.default_token_budget = positive_token_budget(default_token_budget);
        self.running_heartbeat_timeout = if running_heartbeat_timeout.is_zero() {
            Duration::from_secs(crate::config::DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS)
        } else {
            running_heartbeat_timeout
        };
        if self.running_count() == 0 {
            self.launch_gate =
                Arc::new(Semaphore::new(launch_concurrency.clamp(1, self.max_agents)));
            true
        } else {
            false
        }
    }

    /// Build the [`PersistedSubAgentState`] snapshot from the current fleet.
    ///
    /// This is a cheap clone operation that runs under the caller's lock.
    /// The returned payload is fully owned and safe to move to a background
    /// thread for disk I/O.
    fn build_persist_payload(&self) -> Result<Option<(PathBuf, PersistedSubAgentState)>> {
        let Some(path) = self.state_path.as_ref() else {
            return Ok(None);
        };
        let path = checked_subagent_state_path(&self.state_root, path)?;
        let now_ms = epoch_millis_now();
        let mut agents = Vec::with_capacity(self.agents.len());
        for agent in self.agents.values() {
            agents.push(PersistedSubAgent {
                id: agent.id.clone(),
                session_name: Some(agent.session_name.clone()),
                fork_context: agent.fork_context,
                workspace: Some(agent.workspace.clone()),
                agent_type: agent.agent_type.clone(),
                prompt: agent.prompt.clone(),
                assignment: agent.assignment.clone(),
                model: agent.model.clone(),
                // Generated whale names are locale-derived presentation, not
                // durable identity. Persist only an explicit custom nickname;
                // legacy generated values are discarded again on load.
                nickname: agent
                    .nickname
                    .clone()
                    .filter(|name| generated_whale_name_base(&agent.id, name).is_none()),
                status: agent.status.clone(),
                result: agent.result.clone(),
                steps_taken: agent.steps_taken,
                checkpoint: agent.checkpoint.clone(),
                needs_input: agent.needs_input.clone(),
                duration_ms: u64::try_from(agent.started_at.elapsed().as_millis())
                    .unwrap_or(u64::MAX),
                // Backward-compat: Vec on disk. None → empty vec; Some(list) → list.
                // Reload converts empty vec back to None (full inheritance).
                allowed_tools: agent.allowed_tools.clone().unwrap_or_default(),
                updated_at_ms: now_ms,
                session_boot_id: agent.session_boot_id.clone(),
                owner_session_id: agent.owner_session_id.clone(),
            });
        }
        agents.sort_by(|a, b| a.id.cmp(&b.id));

        let payload = PersistedSubAgentState {
            schema_version: SUBAGENT_STATE_SCHEMA_VERSION,
            snapshot_sequence: self
                .persist_sequence
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .saturating_add(1),
            agents,
            workers: self.sorted_worker_records(),
            coordination: self.coordination.clone(),
        };
        Ok(Some((path, payload)))
    }

    /// Persist the current fleet state to disk.
    ///
    /// #freeze: JSON serialization runs cheaply under the caller's lock; the
    /// expensive disk I/O (`write_json_atomic`) is spawned onto a background
    /// thread so the caller's write lock is released before touching the
    /// filesystem.
    ///
    /// Returns a [`std::thread::JoinHandle`] that resolves when the disk write
    /// completes.  Callers may `.join()` for synchronous semantics or drop it
    /// for fire-and-forget.
    fn persist_state(&self) -> Result<std::thread::JoinHandle<()>> {
        self.ensure_coordination_process_lock()
            .map_err(anyhow::Error::msg)?;
        let Some((path, payload)) = self.build_persist_payload()? else {
            // Nothing to persist — return a no-op handle.
            return Ok(std::thread::spawn(|| {}));
        };
        let state_root = self.state_root.clone();
        // Spawn disk I/O off the write-lock hot path.  `payload` is fully
        // owned (cloned from `self.agents`) so it is `Send` and safe to move.
        let handle = std::thread::spawn(move || {
            if let Err(err) = write_json_atomic(&state_root, &path, &payload) {
                tracing::warn!(target: "subagent", ?err, "failed to persist sub-agent state");
            }
        });
        Ok(handle)
    }

    fn persist_state_synchronously(&self) -> Result<()> {
        self.ensure_coordination_process_lock()
            .map_err(anyhow::Error::msg)?;
        let Some((path, payload)) = self.build_persist_payload()? else {
            return Ok(());
        };
        write_json_atomic(&self.state_root, &path, &payload)
    }

    /// Fire-and-forget persist — logs errors, drops the join handle.
    fn persist_state_best_effort(&self) {
        if let Err(err) = self.persist_state() {
            // Must not be `eprintln!` — raw stderr inside the alt-screen
            // leaks into the buffer and produces the scroll-demon
            // regression (#1085). Routed through tracing so the
            // file-backed subscriber in `runtime_log` captures it.
            tracing::warn!(target: "subagent", ?err, "failed to persist sub-agent state");
        } else {
            // Join handle is dropped here — disk I/O proceeds in background.
        }
    }

    /// #freeze: persist on the hot per-step checkpoint path, coalesced to at
    /// most one disk write per `SUBAGENT_PERSIST_DEBOUNCE`. A skipped write
    /// sets `persist_pending` so the next terminal persist (which always
    /// rewrites the full fleet) or `flush_pending_persist` captures it.
    fn persist_state_debounced(&mut self) {
        let now = Instant::now();
        let due = match self.last_persist_at {
            Some(last) => now.duration_since(last) >= SUBAGENT_PERSIST_DEBOUNCE,
            None => true,
        };
        if due {
            self.last_persist_at = Some(now);
            self.persist_pending = false;
            self.persist_state_best_effort();
            let writes =
                SUBAGENT_PERSIST_WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if subagent_perf_enabled() {
                let skipped = SUBAGENT_PERSIST_SKIPPED.load(std::sync::atomic::Ordering::Relaxed);
                tracing::info!(
                    target: "subagent_perf",
                    writes,
                    skipped,
                    agents = self.agents.len(),
                    "checkpoint persist (debounced write)"
                );
            }
        } else {
            self.persist_pending = true;
            SUBAGENT_PERSIST_SKIPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// #freeze: force a persist if a hot-path write was previously coalesced
    /// away. Call on graceful shutdown / session teardown so the most recent
    /// intermediate checkpoint is not lost.
    ///
    /// Unlike `persist_state`, this performs disk I/O **synchronously** to
    /// guarantee data is flushed before the process exits.
    pub fn flush_pending_persist(&mut self) {
        if let Err(error) = self.ensure_coordination_process_lock() {
            tracing::warn!(target: "subagent", %error, "skipping persist without state-root coordination lock");
            return;
        }
        if self.persist_pending {
            self.last_persist_at = Some(Instant::now());
            self.persist_pending = false;
            // Synchronous disk I/O — safe because we are shutting down and no
            // callers depend on releasing the write lock quickly.
            if let Ok(Some((path, payload))) = self.build_persist_payload()
                && let Err(err) = write_json_atomic(&self.state_root, &path, &payload)
            {
                tracing::warn!(target: "subagent", ?err, "failed to flush pending sub-agent state");
            }
        }
    }

    fn load_state(&mut self) -> Result<()> {
        let Some(path) = self.state_path.as_ref() else {
            return Ok(());
        };
        let path = checked_subagent_state_path(&self.state_root, path)?;

        // If canonical path doesn't exist, try legacy .deepseek/ path for one-time
        // migration. The next persist will write to the canonical .codewhale/ path.
        let path = if path.exists() {
            path
        } else {
            let legacy = checked_subagent_state_path(
                &self.state_root,
                &Path::new(".deepseek")
                    .join("state")
                    .join(SUBAGENT_STATE_FILE),
            )?;
            if legacy.exists() {
                tracing::info!(
                    target: "subagent",
                    "loading sub-agent state from legacy path for migration: {}",
                    legacy.display()
                );
                legacy
            } else {
                return Ok(());
            }
        };

        let raw = read_subagent_state_file(&self.state_root, &path)?;
        let state = serde_json::from_str::<PersistedSubAgentState>(&raw)?;
        if state.schema_version != SUBAGENT_STATE_SCHEMA_VERSION {
            return Err(anyhow!(
                "Unsupported sub-agent state schema {}",
                state.schema_version
            ));
        }

        let mut coordination = state.coordination;
        coordination
            .validate_replay()
            .map_err(|error| anyhow!("Invalid coordination ledger: {error}"))?;
        self.agents.clear();
        self.worker_records.clear();
        self.persist_sequence.store(
            state.snapshot_sequence,
            std::sync::atomic::Ordering::Relaxed,
        );
        self.coordination = coordination;
        for persisted in state.agents {
            let nickname = persisted
                .nickname
                .filter(|name| generated_whale_name_base(&persisted.id, name).is_none());
            let mut status = persisted.status;
            if matches!(status, SubAgentStatus::Running) {
                status = SubAgentStatus::Interrupted(SUBAGENT_RESTART_REASON.to_string());
            }

            let started_at = instant_from_duration(Duration::from_millis(persisted.duration_ms));
            // Empty vec on disk → None (full inheritance, v0.6.6 default).
            // Non-empty vec → Some(list) (preserves narrow scope from older sessions).
            let allowed_tools = if persisted.allowed_tools.is_empty() {
                None
            } else {
                Some(persisted.allowed_tools)
            };
            let mut assignment = persisted.assignment;
            canonicalize_persisted_advisory_role(&mut assignment.role);
            let agent = SubAgent {
                id: persisted.id.clone(),
                session_name: persisted
                    .session_name
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| persisted.id.clone()),
                fork_context: persisted.fork_context,
                workspace: persisted
                    .workspace
                    .unwrap_or_else(|| self.workspace.clone()),
                agent_type: persisted.agent_type,
                prompt: persisted.prompt,
                assignment,
                model: if persisted.model.is_empty() {
                    "unknown".to_string()
                } else {
                    persisted.model
                },
                // v0.8.68 and earlier persisted generated whale text. It may
                // have been chosen under a different UI language, so never
                // replay it into a new session. Explicit custom names survive.
                nickname,
                status,
                result: persisted.result,
                steps_taken: persisted.steps_taken,
                checkpoint: persisted.checkpoint,
                needs_input: persisted.needs_input,
                started_at,
                last_activity_at: started_at,
                allowed_tools,
                // Empty string when loading pre-#405 records; the
                // manager treats that the same as a non-matching id —
                // i.e. agent classified as prior-session.
                session_boot_id: persisted.session_boot_id,
                owner_session_id: persisted.owner_session_id,
                completion_claimed: false,
                terminal_delivery: None,
                work_lifecycle: None,
                input_tx: None,
                task_handle: None,
            };
            self.agents.insert(persisted.id, agent);
        }
        for worker in state.workers {
            let worker = normalize_worker_record(worker);
            self.worker_event_seq = self.worker_event_seq.max(
                worker
                    .events
                    .iter()
                    .map(|event| event.seq)
                    .max()
                    .unwrap_or(0),
            );
            self.worker_records
                .insert(worker.spec.worker_id.clone(), worker);
        }
        self.reconcile_orphaned_workers_after_restart();
        self.refresh_all_budget_scopes();
        self.prune_worker_records();

        Ok(())
    }

    /// No in-process task survives a manager restart. Reconcile every worker
    /// status that requires a live executor to `Interrupted`, matching the
    /// existing top-level agent restoration above. Terminal receipts and
    /// waiting-for-user records remain unchanged, and the status guard makes
    /// repeated reconciliation idempotent (#4408).
    fn reconcile_orphaned_workers_after_restart(&mut self) -> usize {
        let orphaned = self
            .worker_records
            .values()
            .filter(|record| {
                matches!(
                    record.status,
                    AgentWorkerStatus::Queued
                        | AgentWorkerStatus::Starting
                        | AgentWorkerStatus::Running
                        | AgentWorkerStatus::ModelWait
                        | AgentWorkerStatus::RunningTool
                )
            })
            .map(|record| (record.spec.worker_id.clone(), record.steps_taken))
            .collect::<Vec<_>>();
        for (worker_id, steps_taken) in &orphaned {
            self.record_worker_event(
                worker_id,
                AgentWorkerStatus::Interrupted,
                Some(SUBAGENT_RESTART_REASON.to_string()),
                Some(*steps_taken),
                None,
            );
        }
        orphaned.len()
    }

    /// Finalize the live fleet when the parent session closes without a
    /// process restart (#5372). A session switch in the same process keeps
    /// the manager (and its coordination ledger) alive, so running children
    /// and their write claims would otherwise survive into the next
    /// conversation and keep blocking new writers.
    ///
    /// Loaded prior-session agents are already `Interrupted` and loaded
    /// workers are already terminal (reconciled on `load_state`), so only the
    /// currently-live fleet is finalized here. Write claims owned by absent
    /// or prior-session owners stay put — a sibling session's live claim must
    /// never be released by this process.
    #[cfg(test)]
    pub fn finalize_session_close(&mut self) -> usize {
        self.finalize_session_close_inner(None)
    }

    pub(crate) fn finalize_session_close_for_session(&mut self, active_session_id: &str) -> usize {
        self.finalize_session_close_inner(Some(active_session_id))
    }

    fn finalize_session_close_inner(&mut self, active_session_id: Option<&str>) -> usize {
        let reason = SUBAGENT_SESSION_CLOSED_REASON;
        let running_agent_ids = self
            .agents
            .values()
            .filter(|agent| agent.status == SubAgentStatus::Running)
            .filter(|agent| {
                active_session_id
                    .is_none_or(|session_id| self.agent_is_owned_by_session(agent, session_id))
            })
            .map(|agent| agent.id.clone())
            .collect::<std::collections::HashSet<_>>();
        let mut finalized = 0usize;
        for agent_id in &running_agent_ids {
            let Some(snapshot) = self.agents.get(agent_id).map(SubAgent::snapshot) else {
                continue;
            };
            let mut terminal = snapshot;
            terminal.status = SubAgentStatus::Interrupted(reason.to_string());
            terminal.result = Some(reason.to_string());
            terminal.needs_input = None;
            if self.finish_terminal_result(agent_id, terminal, true, true) {
                finalized += 1;
            }
        }
        // Headless workers keep no agent entry, and a waiting-for-user worker
        // may survive its agent's transition above, so reconcile every live
        // (non-terminal) worker record directly.
        let live_worker_ids = self
            .worker_records
            .values()
            .filter(|record| !record.status.is_terminal())
            .filter(|record| {
                active_session_id.is_none_or(|session_id| {
                    !session_id.is_empty() && record.owner_session_id == session_id
                })
            })
            .map(|record| (record.spec.worker_id.clone(), record.steps_taken))
            .collect::<Vec<_>>();
        for (worker_id, steps_taken) in &live_worker_ids {
            self.record_worker_event(
                worker_id,
                AgentWorkerStatus::Interrupted,
                Some(reason.to_string()),
                Some(*steps_taken),
                None,
            );
            finalized += 1;
        }
        // Release write claims owned by the fleet being finalized. Claims from
        // absent owners (sibling sessions) and prior-session records stay put.
        let finalized_owners = running_agent_ids
            .into_iter()
            .chain(live_worker_ids.into_iter().map(|(id, _)| id))
            .collect::<std::collections::HashSet<_>>();
        let mut released = 0usize;
        self.coordination.write_claims.retain(|record| {
            if finalized_owners.contains(&record.claim.owner) {
                released += 1;
                false
            } else {
                true
            }
        });
        // Drop finalized agents from the in-memory roster so they can never
        // be counted active again. Prior-session archive rows remain.
        self.agents.retain(|id, _| !finalized_owners.contains(id));
        let _ = self.persist_state_synchronously();
        tracing::debug!(
            target: "subagent",
            finalized,
            released,
            "finalized sub-agent fleet on session close"
        );
        finalized
    }

    fn sorted_worker_records(&self) -> Vec<AgentWorkerRecord> {
        let mut workers: Vec<_> = self.worker_records.values().cloned().collect();
        workers.sort_by(|a, b| {
            b.updated_at_ms
                .cmp(&a.updated_at_ms)
                .then_with(|| a.spec.worker_id.cmp(&b.spec.worker_id))
        });
        workers
    }

    fn prune_worker_records(&mut self) {
        while self.worker_records.len() > MAX_AGENT_WORKER_RECORDS {
            let oldest_terminal = self
                .worker_records
                .values()
                .filter(|record| record.status.is_terminal())
                .min_by(|a, b| {
                    a.updated_at_ms
                        .cmp(&b.updated_at_ms)
                        .then_with(|| a.spec.worker_id.cmp(&b.spec.worker_id))
                })
                .map(|record| record.spec.worker_id.clone());
            let Some(worker_id) = oldest_terminal else {
                // Active launch identity is never a retention casualty. The
                // live population may temporarily exceed the history cap and
                // is compacted after terminal records become reclaimable.
                break;
            };
            self.worker_records.remove(&worker_id);
        }
    }

    pub fn register_worker(&mut self, spec: AgentWorkerSpec) {
        self.register_worker_for_session(spec, "");
    }

    fn register_worker_for_session(&mut self, spec: AgentWorkerSpec, owner_session_id: &str) {
        let worker_id = spec.worker_id.clone();
        let now_ms = epoch_millis_now();
        let mut record = AgentWorkerRecord::new_for_session(
            normalize_worker_spec(spec),
            now_ms,
            owner_session_id.to_string(),
        );
        self.push_worker_event(
            &mut record,
            AgentWorkerStatus::Starting,
            Some("starting".to_string()),
            None,
            None,
            now_ms,
        );
        self.worker_records.insert(worker_id, record);
        self.prune_worker_records();
    }

    /// Validate a Fleet/headless worker's persisted launch claim before its
    /// durable task lease is committed. A contention is still receipted in the
    /// live coordination ledger, but a successful preflight does not reserve
    /// anything until `register_worker_with_coordination` runs under the same
    /// manager write lock.
    pub fn preflight_worker_coordination(&mut self, spec: &AgentWorkerSpec) -> Result<(), String> {
        let Some((claim, isolated_worktree)) = worker_coordination_claim(spec)? else {
            return Ok(());
        };
        // Isolated-worktree builders mutate their own checkout, not the
        // shared workspace, so they must not contend for the per-workspace
        // coordination process lock (#5036).
        if !isolated_worktree {
            self.ensure_coordination_process_lock()?;
        }
        let active_owners = self.active_coordination_owners();
        let mut probe = self.coordination.clone();
        match probe.register_claim(claim.clone(), isolated_worktree, |owner| {
            active_owners.contains(owner)
        }) {
            Ok(_) => Ok(()),
            Err(error) => {
                // Re-run against the authoritative ledger so a real overlap
                // remains visible after restart. Invalid/schema failures do
                // not mutate either ledger.
                let coordination_before = self.coordination.clone();
                let _ = self
                    .coordination
                    .register_claim(claim, isolated_worktree, |owner| {
                        active_owners.contains(owner)
                    });
                if let Err(persist_error) = self.persist_state_synchronously() {
                    self.coordination = coordination_before;
                    return Err(format!(
                        "{error}; additionally failed to persist contention receipt: {persist_error}"
                    ));
                }
                Err(error)
            }
        }
    }

    /// Commit a preflighted Fleet/headless worker, its bounded write claim, and
    /// its minimal accepted-decision projection as one in-memory projection.
    /// Callers hold the manager write lock from preflight through this method,
    /// so the second validation cannot race another registration.
    pub fn register_worker_with_coordination(
        &mut self,
        mut spec: AgentWorkerSpec,
    ) -> Result<(), String> {
        let claim = worker_coordination_claim(&spec)?;
        // Same isolation rule as `preflight_worker_coordination` (#5036).
        let isolated_worktree = claim
            .as_ref()
            .is_some_and(|(_, isolated_worktree)| *isolated_worktree);
        if !isolated_worktree {
            self.ensure_coordination_process_lock()?;
        }
        let previous_worker_records = self.worker_records.clone();
        let previous_coordination = self.coordination.clone();
        let persisted_claim = claim
            .map(|(claim, isolated_worktree)| {
                let active_owners = self.active_coordination_owners();
                self.coordination
                    .register_claim(claim, isolated_worktree, |owner| {
                        active_owners.contains(owner)
                    })
            })
            .transpose()?;

        let mut capabilities = match &spec.tool_profile {
            AgentWorkerToolProfile::Inherited => Vec::new(),
            AgentWorkerToolProfile::Explicit(tools) => tools.clone(),
        };
        capabilities.push(spec.agent_type.as_str().to_string());
        if let Some(role) = spec.role.as_ref()
            && !capabilities.contains(role)
        {
            capabilities.push(role.clone());
        }
        let (projection, _) = self.coordination.project_relevant_decisions(
            &spec.worker_id,
            persisted_claim.as_ref().map(|record| &record.claim),
            &capabilities,
        );
        if !projection.is_empty() {
            spec.objective.push_str("\n\n");
            spec.objective.push_str(&projection);
            if let Some(manifest) = spec.launch_manifest.as_mut() {
                manifest.prompt = spec.objective.clone();
            }
        }
        let worker_id_for_log = spec.worker_id.clone();
        self.register_worker(spec);
        if isolated_worktree && self.ensure_coordination_process_lock().is_err() {
            // This process does not own the durable coordination state and an
            // isolated-worktree builder must not be blocked on it: register
            // in memory only and leave the durable ledger to the lock owner.
            // The persist-layer lock check stays intact (#5036).
            tracing::warn!(
                target: "subagent",
                worker = %worker_id_for_log,
                "registered isolated-worktree worker in memory only; delegated coordination lock is owned elsewhere"
            );
            return Ok(());
        }
        if let Err(error) = self.persist_state_synchronously() {
            self.worker_records = previous_worker_records;
            self.coordination = previous_coordination;
            return Err(format!(
                "failed to persist Fleet coordination launch record: {error}"
            ));
        }
        Ok(())
    }

    pub(crate) fn coordination_registration_snapshot(&self) -> CoordinationRegistrationSnapshot {
        CoordinationRegistrationSnapshot {
            worker_records: self.worker_records.clone(),
            coordination: self.coordination.clone(),
        }
    }

    pub(crate) fn restore_coordination_registration_snapshot(
        &mut self,
        snapshot: CoordinationRegistrationSnapshot,
    ) -> Result<(), String> {
        self.worker_records = snapshot.worker_records;
        self.coordination = snapshot.coordination;
        self.persist_state_synchronously()
            .map_err(|error| format!("failed to persist Fleet coordination rollback: {error}"))
    }

    /// The single definition of a live coordination claimant.
    ///
    /// One predicate, evaluated per owner id by every caller: claim admission
    /// (`active_coordination_owners` delegates below), the shared-checkout peer
    /// gate (`live_peer_shared_write_claim_owners`), and stale-claim release.
    /// The admission and gate sides used to disagree, which let a claim from a
    /// crashed prior session (an agent record restored as `Running` with a
    /// different boot id, or an owner whose record was never loaded at all)
    /// keep looking like a live writer forever — every later builder/worker
    /// was then denied all command execution (issue #5562: "stale write-claims
    /// persist forever and cascade-lock other agents"). A claim cannot contend
    /// for an owner that cannot write.
    fn is_live_coordination_owner(&self, id: &str) -> bool {
        self.agents.get(id).is_some_and(|agent| {
            agent.status == SubAgentStatus::Running && !self.is_from_prior_session(agent)
        }) || self.worker_records.get(id).is_some_and(|record| {
            !record.status.is_terminal()
                // A headless worker (no paired agent entry) that reaches
                // WaitingForUser can never be answered: no user path will
                // resume or finalize it, so counting it live leaks its claim
                // as a permanent gate on every later writer. A *paired*
                // waiting child keeps its claim — the user can still answer
                // and the child will write again.
                && !(record.status == AgentWorkerStatus::WaitingForUser
                    && !self.agents.contains_key(id))
                && !self
                    .agents
                    .get(id)
                    .is_some_and(|agent| self.is_from_prior_session(agent))
        })
    }

    // Worker records carry no boot id of their own, so the per-id predicate
    // consults the paired agent when one exists; delegating keeps admission,
    // the peer gate, and stale-claim release on one liveness definition.
    fn active_coordination_owners(&self) -> std::collections::HashSet<String> {
        self.agents
            .keys()
            .chain(self.worker_records.keys())
            .filter(|id| self.is_live_coordination_owner(id))
            .cloned()
            .collect()
    }

    fn namespace_write_claim(
        &self,
        workspace: &Path,
        isolated_worktree: bool,
        mut claim: WriteScopeClaim,
    ) -> Result<WriteScopeClaim, String> {
        if isolated_worktree {
            return Ok(claim);
        }
        let prefix = coordination_workspace_prefix(&self.workspace, workspace)?;
        claim.roots = claim
            .roots
            .iter()
            .map(|path| namespace_coordination_path(&prefix, path))
            .collect::<Result<Vec<_>, _>>()?;
        claim.exact_files = claim
            .exact_files
            .iter()
            .map(|path| namespace_coordination_path(&prefix, path))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(claim)
    }

    fn namespace_claim_paths_for_owner(
        &self,
        owner: &str,
        isolated_worktree: bool,
        roots: Vec<String>,
        exact_files: Vec<String>,
    ) -> Result<(Vec<String>, Vec<String>), String> {
        if isolated_worktree {
            return Ok((roots, exact_files));
        }
        let workspace = self
            .worker_records
            .get(owner)
            .map(|record| record.spec.workspace.as_path())
            .or_else(|| {
                self.agents
                    .get(owner)
                    .map(|agent| agent.workspace.as_path())
            })
            .unwrap_or(self.workspace.as_path());
        let prefix = coordination_workspace_prefix(&self.workspace, workspace)?;
        let roots = roots
            .iter()
            .map(|path| namespace_coordination_path(&prefix, path))
            .collect::<Result<Vec<_>, _>>()?;
        let exact_files = exact_files
            .iter()
            .map(|path| namespace_coordination_path(&prefix, path))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((roots, exact_files))
    }

    pub fn list_worker_records(&self) -> Vec<AgentWorkerRecord> {
        self.sorted_worker_records()
    }

    /// Empty-owner legacy records fail closed. Headless records keep their own
    /// immutable owner so they can remain visible without requiring a paired
    /// `SubAgent` row.
    pub(crate) fn list_worker_records_for_session(
        &self,
        active_session_id: &str,
    ) -> Vec<AgentWorkerRecord> {
        self.sorted_worker_records()
            .into_iter()
            .filter(|record| {
                !active_session_id.is_empty() && record.owner_session_id == active_session_id
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn coordination_snapshot(&self) -> CoordinationLedger {
        self.coordination.clone()
    }

    pub fn get_worker_record(&self, worker_id: &str) -> Option<AgentWorkerRecord> {
        self.worker_records.get(worker_id).cloned()
    }

    pub(crate) fn get_worker_record_for_session(
        &self,
        active_session_id: &str,
        worker_id: &str,
    ) -> Option<AgentWorkerRecord> {
        self.worker_records.get(worker_id).and_then(|record| {
            (!active_session_id.is_empty() && record.owner_session_id == active_session_id)
                .then(|| record.clone())
        })
    }

    #[cfg(test)]
    pub(crate) fn replace_registered_worker_spec_for_test(
        &mut self,
        spec: AgentWorkerSpec,
    ) -> Result<(), String> {
        let worker_id = spec.worker_id.clone();
        let record = self
            .worker_records
            .get_mut(&worker_id)
            .ok_or_else(|| format!("Fleet worker {worker_id} has no registered launch spec"))?;
        record.spec = spec;
        self.persist_state_synchronously()
            .map_err(|error| format!("failed to persist test worker spec: {error}"))
    }

    /// Persist the next exact launch generation for a Fleet restart without
    /// changing the worker's authority, route, prompt, or retained lifecycle
    /// evidence. The Fleet manager validates the full spec against the task
    /// lease before calling this; this local check makes generation the only
    /// field that can change through this narrow seam.
    pub(crate) fn advance_registered_worker_generation(
        &mut self,
        spec: AgentWorkerSpec,
    ) -> Result<(), String> {
        self.ensure_coordination_process_lock()?;
        let worker_id = spec.worker_id.clone();
        let previous = self
            .worker_records
            .get(&worker_id)
            .cloned()
            .ok_or_else(|| format!("Fleet worker {worker_id} has no registered launch spec"))?;
        let old_generation = previous
            .spec
            .launch_manifest
            .as_ref()
            .map(|manifest| manifest.generation)
            .ok_or_else(|| format!("Fleet worker {worker_id} has no persisted launch manifest"))?;
        let new_generation = spec
            .launch_manifest
            .as_ref()
            .map(|manifest| manifest.generation)
            .ok_or_else(|| {
                format!("Fleet worker {worker_id} replacement has no launch manifest")
            })?;
        if new_generation != old_generation.saturating_add(1) {
            return Err(format!(
                "Fleet worker {worker_id} restart generation must advance from {old_generation} to {}",
                old_generation.saturating_add(1)
            ));
        }
        let mut expected = previous.spec.clone();
        expected
            .launch_manifest
            .as_mut()
            .expect("old launch manifest checked above")
            .generation = new_generation;
        if expected != spec {
            return Err(format!(
                "Fleet worker {worker_id} restart may change only its launch generation"
            ));
        }

        let record = self
            .worker_records
            .get_mut(&worker_id)
            .expect("worker record checked above");
        record.spec = spec;
        record.updated_at_ms = epoch_millis_now();
        if let Err(error) = self.persist_state_synchronously() {
            self.worker_records.insert(worker_id, previous);
            return Err(format!(
                "failed to persist Fleet restart launch generation: {error}"
            ));
        }
        Ok(())
    }

    /// Reconcile an externally executed Fleet worker with the shared headless
    /// lifecycle projection. This is idempotent and makes old write claims stop
    /// participating in active-overlap checks once the durable Fleet task is
    /// terminal.
    pub fn project_external_worker_status(
        &mut self,
        worker_id: &str,
        status: AgentWorkerStatus,
        message: Option<String>,
    ) -> bool {
        let Some(record) = self.worker_records.get(worker_id) else {
            return false;
        };
        if record.status == status {
            return false;
        }
        self.record_worker_event(worker_id, status, message, None, None);
        self.persist_state_best_effort();
        true
    }

    fn aggregate_budget_spent(&self, scope_id: &str) -> u64 {
        self.worker_records
            .values()
            .filter(|record| record.usage.budget_scope.as_deref() == Some(scope_id))
            .fold(0_u64, |total, record| {
                total.saturating_add(record.usage.total_tokens.unwrap_or(0))
            })
    }

    fn inherited_budget_scope(&self, parent_run_id: Option<&str>) -> Option<(String, u64)> {
        let parent = self.worker_records.get(parent_run_id?)?;
        let limit = parent.usage.token_budget?;
        let scope_id = parent
            .usage
            .budget_scope
            .clone()
            .unwrap_or_else(|| parent.spec.worker_id.clone());
        Some((scope_id, limit))
    }

    fn resolve_spawn_budget_scope(
        &self,
        worker_id: &str,
        parent_run_id: Option<&str>,
        requested_budget: Option<u64>,
    ) -> Result<Option<AgentUsageBudgetScope>> {
        let scope = if let Some(limit) = positive_token_budget(requested_budget) {
            Some((worker_id.to_string(), limit))
        } else if let Some(parent_scope) = self.inherited_budget_scope(parent_run_id) {
            Some(parent_scope)
        } else {
            self.default_token_budget
                .map(|limit| (worker_id.to_string(), limit))
        };

        let Some((scope_id, limit)) = scope else {
            return Ok(None);
        };
        let spent = self.aggregate_budget_spent(&scope_id);
        let remaining = limit.saturating_sub(spent);
        if remaining < MIN_SUBAGENT_SPAWN_TOKEN_RESERVE {
            return Err(anyhow!(
                "Sub-agent token budget exhausted for scope {scope_id}: {spent}/{limit} tokens spent, {remaining} remaining. Wait for the parent/Workflow to summarize results or start a fresh agent run."
            ));
        }
        Ok(Some(AgentUsageBudgetScope {
            scope_id,
            limit,
            spent,
            remaining,
        }))
    }

    fn attach_budget_scope(&mut self, worker_id: &str, scope: AgentUsageBudgetScope) {
        let Some(record) = self.worker_records.get_mut(worker_id) else {
            return;
        };
        record.usage.token_budget = Some(scope.limit);
        record.usage.budget_scope = Some(scope.scope_id.clone());
        record.usage.budget_spent_tokens = Some(scope.spent);
        record.usage.budget_remaining_tokens = Some(scope.remaining);
        refresh_usage_note(&mut record.usage);
        self.refresh_budget_scope(&scope.scope_id);
    }

    /// Aggregate token spend for a shared workflow budget scope.
    pub(crate) fn budget_spent_for_scope(&self, scope_id: &str) -> u64 {
        self.aggregate_budget_spent(scope_id)
    }

    /// Current `(spent, limit)` for the shared budget scope this worker is
    /// attached to, if any. `spent` is the live aggregate across every worker
    /// in the scope, so a caller checking mid-run sees sibling spend as it
    /// lands, not the snapshot frozen at attach time.
    pub(crate) fn budget_scope_state(&self, worker_id: &str) -> Option<(u64, u64)> {
        let record = self.worker_records.get(worker_id)?;
        let scope_id = record.usage.budget_scope.as_deref()?;
        let limit = record.usage.token_budget?;
        Some((self.aggregate_budget_spent(scope_id), limit))
    }

    /// Attach a workflow child to the run-level shared budget pool.
    pub(crate) fn attach_shared_budget_scope(
        &mut self,
        worker_id: &str,
        scope_id: &str,
        limit: u64,
    ) {
        let spent = self.aggregate_budget_spent(scope_id);
        self.attach_budget_scope(
            worker_id,
            AgentUsageBudgetScope {
                scope_id: scope_id.to_string(),
                limit,
                spent,
                remaining: limit.saturating_sub(spent),
            },
        );
    }

    fn refresh_budget_scope(&mut self, scope_id: &str) {
        let Some(limit) = self
            .worker_records
            .values()
            .find(|record| record.usage.budget_scope.as_deref() == Some(scope_id))
            .and_then(|record| record.usage.token_budget)
        else {
            return;
        };
        let spent = self.aggregate_budget_spent(scope_id);
        let remaining = limit.saturating_sub(spent);
        for record in self.worker_records.values_mut() {
            if record.usage.budget_scope.as_deref() == Some(scope_id) {
                record.usage.token_budget = Some(limit);
                record.usage.budget_spent_tokens = Some(spent);
                record.usage.budget_remaining_tokens = Some(remaining);
                refresh_usage_note(&mut record.usage);
            }
        }
    }

    fn refresh_all_budget_scopes(&mut self) {
        let scope_ids = self
            .worker_records
            .values()
            .filter_map(|record| record.usage.budget_scope.clone())
            .collect::<std::collections::HashSet<_>>();
        for scope_id in scope_ids {
            self.refresh_budget_scope(&scope_id);
        }
    }

    fn record_worker_usage(
        &mut self,
        worker_id: &str,
        source_id: &str,
        usage: &Usage,
        priced_cost_microusd: Option<u64>,
    ) {
        let now_ms = epoch_millis_now();
        let total_delta = usage_total_tokens(usage);
        let Some(record) = self.worker_records.get_mut(worker_id) else {
            return;
        };
        let source_fingerprint = crate::cost_status::usage_source_fingerprint(source_id);
        if !record.usage_source_fingerprints.insert(source_fingerprint) {
            return;
        }
        record.updated_at_ms = now_ms;
        record.usage.input_tokens = Some(
            record
                .usage
                .input_tokens
                .unwrap_or(0)
                .saturating_add(u64::from(usage.input_tokens)),
        );
        record.usage.output_tokens = Some(
            record
                .usage
                .output_tokens
                .unwrap_or(0)
                .saturating_add(u64::from(usage.output_tokens)),
        );
        record.usage.total_tokens = Some(
            record
                .usage
                .total_tokens
                .unwrap_or(0)
                .saturating_add(total_delta),
        );
        if let Some(cost_microusd) = priced_cost_microusd {
            record.usage.cost_microusd = Some(
                record
                    .usage
                    .cost_microusd
                    .unwrap_or(0)
                    .saturating_add(cost_microusd),
            );
        }
        let scope_id = record.usage.budget_scope.clone();
        refresh_usage_note(&mut record.usage);
        if let Some(scope_id) = scope_id {
            self.refresh_budget_scope(&scope_id);
        }
        self.persist_state_debounced();
    }

    fn push_worker_event(
        &mut self,
        record: &mut AgentWorkerRecord,
        status: AgentWorkerStatus,
        message: Option<String>,
        step: Option<u32>,
        tool_name: Option<String>,
        now_ms: u64,
    ) {
        self.worker_event_seq = self.worker_event_seq.saturating_add(1);
        record.events.push_back(AgentWorkerEvent {
            seq: self.worker_event_seq,
            worker_id: record.spec.worker_id.clone(),
            status,
            timestamp_ms: now_ms,
            message,
            step,
            tool_name,
        });
        while record.events.len() > MAX_AGENT_WORKER_EVENTS_PER_RECORD {
            record.events.pop_front();
        }
    }

    fn record_worker_event(
        &mut self,
        worker_id: &str,
        status: AgentWorkerStatus,
        message: Option<String>,
        step: Option<u32>,
        tool_name: Option<String>,
    ) {
        let now_ms = epoch_millis_now();
        let Some(mut record) = self.worker_records.remove(worker_id) else {
            return;
        };
        record.status = status;
        record.recommended_action = recommended_action_for_worker_status(status, &record.spec);
        record.updated_at_ms = now_ms;
        record.latest_message = message.clone();
        if matches!(
            status,
            AgentWorkerStatus::Starting | AgentWorkerStatus::Running
        ) && record.started_at_ms.is_none()
        {
            record.started_at_ms = Some(now_ms);
        }
        if matches!(
            status,
            AgentWorkerStatus::Completed
                | AgentWorkerStatus::Failed
                | AgentWorkerStatus::Cancelled
                | AgentWorkerStatus::Interrupted
        ) {
            record.completed_at_ms = Some(now_ms);
        }
        if let Some(step) = step {
            record.steps_taken = step;
        }
        self.push_worker_event(&mut record, status, message, step, tool_name, now_ms);
        self.worker_records.insert(worker_id.to_string(), record);
        self.reconcile_worker_lifecycle(worker_id);
    }

    fn reconcile_worker_lifecycle(&self, worker_id: &str) {
        let Some(lifecycle) = self
            .agents
            .get(worker_id)
            .and_then(|agent| agent.work_lifecycle.clone())
        else {
            return;
        };
        let Some(record) = self.worker_records.get(worker_id) else {
            return;
        };
        if let Err(err) = lifecycle.reconcile_record(record) {
            tracing::warn!(
                target: "subagent",
                worker_id,
                ?err,
                "failed to reconcile sub-agent Work lifecycle"
            );
        }
    }

    fn complete_worker_from_result(&mut self, worker_id: &str, result: &SubAgentResult) {
        let status = worker_status_from_subagent_result(result);
        let message = match &result.status {
            SubAgentStatus::Completed => Some("completed".to_string()),
            SubAgentStatus::Failed(err) => Some(err.clone()),
            SubAgentStatus::Interrupted(reason) => Some(reason.clone()),
            SubAgentStatus::Cancelled => Some("cancelled".to_string()),
            SubAgentStatus::BudgetExhausted => Some("token budget exhausted".to_string()),
            SubAgentStatus::Running => Some("running".to_string()),
        };
        if let Some(record) = self.worker_records.get_mut(worker_id) {
            record.result_summary = result.result.clone();
            record.steps_taken = result.steps_taken;
            if let SubAgentStatus::Failed(err) = &result.status {
                record.error = Some(err.clone());
            }
            // R7 (finish-operator 2026-08-02): a completed child's claimed
            // changed-files are checked against `git status` in its own
            // workspace at terminal delivery. A claim git cannot see taints
            // the verification summary the worker record already carries —
            // the parent keeps the result, but labeled, not trusted.
            if matches!(result.status, SubAgentStatus::Completed)
                && let Some(summary_text) = result.result.as_deref()
                && let Some(workspace) = result.workspace.as_deref()
                && let Some(taint) =
                    claimed_diff_taint(summary_text, workspace, Some(record.created_at_ms))
            {
                record.verification = taint;
            }
        }
        self.record_worker_event(worker_id, status, message, Some(result.steps_taken), None);
    }

    pub fn cancel_agent(&mut self, agent_ref: &str) -> Result<SubAgentResult> {
        let agent_id = self.resolve_agent_ref(agent_ref)?;
        let mut terminal = {
            let agent = self
                .agents
                .get(&agent_id)
                .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;
            if agent.status != SubAgentStatus::Running || agent.completion_claimed {
                return Ok(agent.snapshot());
            }
            agent.snapshot()
        };
        terminal.status = SubAgentStatus::Cancelled;
        terminal.result = Some("Cancelled by parent request.".to_string());
        terminal.needs_input = None;
        if !self.finish_terminal_result(&agent_id, terminal, true, true) {
            return self.get_result(&agent_id);
        }
        self.get_result(&agent_id)
    }

    pub(crate) fn cancel_agent_for_session(
        &mut self,
        active_session_id: &str,
        agent_ref: &str,
    ) -> Result<SubAgentResult> {
        // Resolution and mutation share this manager write borrow, so an alias
        // cannot be rebound between the owner check and the exact-id action.
        let agent_id = self.resolve_agent_ref_for_session(active_session_id, agent_ref)?;
        self.cancel_agent(&agent_id)
    }

    /// Queue parent mail without waking the child (`agents/message`).
    pub fn queue_parent_message(
        &mut self,
        agent_ref: &str,
        text: String,
        wake: bool,
    ) -> Result<ParentMailReceipt> {
        let agent_id = self.resolve_agent_ref(agent_ref)?;
        let status = self
            .agents
            .get(&agent_id)
            .map(|agent| subagent_status_name(&agent.status).to_string())
            .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;
        let entry = QueuedParentMessage {
            text,
            queued_at_ms: epoch_millis_now(),
            wake,
        };
        let queue = self.queued_mail.entry(agent_id.clone()).or_default();
        queue.push_back(entry);
        let queue_depth = queue.len();
        Ok(ParentMailReceipt {
            agent_id,
            status,
            queue_depth,
            woke: false,
            continued_from_checkpoint: false,
            continuation_handle: None,
            note: "queued without wake".to_string(),
        })
    }

    /// Queue-only message entrypoint for live children. Terminal records are
    /// immutable receipts, not mailboxes, so the message surface fails closed
    /// instead of acknowledging undeliverable work.
    pub fn queue_running_parent_message(
        &mut self,
        agent_ref: &str,
        text: String,
    ) -> Result<ParentMailReceipt> {
        let agent_id = self.resolve_agent_ref(agent_ref)?;
        let status = self
            .agents
            .get(&agent_id)
            .map(|agent| agent.status.clone())
            .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;
        if status != SubAgentStatus::Running {
            return Err(anyhow!(
                "Cannot queue a parent message for agent {agent_id}: status is {} (only running children accept messages)",
                subagent_status_name(&status)
            ));
        }
        self.queue_parent_message(&agent_id, text, false)
    }

    pub(crate) fn queue_running_parent_message_for_session(
        &mut self,
        active_session_id: &str,
        agent_ref: &str,
        text: String,
    ) -> Result<ParentMailReceipt> {
        let agent_id = self.resolve_agent_ref_for_session(active_session_id, agent_ref)?;
        self.queue_running_parent_message(&agent_id, text)
    }

    /// Queue mail and attempt a live wake (`agents/followup`).
    pub fn followup_child(&mut self, agent_ref: &str, text: String) -> Result<ParentMailReceipt> {
        let agent_id = self.resolve_agent_ref(agent_ref)?;
        let status = self
            .agents
            .get(&agent_id)
            .map(|agent| agent.status.clone())
            .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;
        if matches!(
            status,
            SubAgentStatus::Completed
                | SubAgentStatus::Failed(_)
                | SubAgentStatus::Cancelled
                | SubAgentStatus::BudgetExhausted
        ) {
            return Err(anyhow!(
                "Cannot follow up agent {agent_id}: status is {} and the child cannot resume",
                subagent_status_name(&status)
            ));
        }
        let mut receipt = self.queue_parent_message(&agent_id, text.clone(), true)?;
        let has_input_tx = self
            .agents
            .get(&agent_id)
            .is_some_and(|agent| agent.input_tx.is_some());
        let continuation_handle = self.agents.get(&agent_id).and_then(|agent| {
            agent.checkpoint.as_ref().and_then(|cp| {
                (cp.continuable && !cp.messages.is_empty()).then(|| cp.continuation_handle.clone())
            })
        });
        let continuable = continuation_handle.is_some();

        match status {
            SubAgentStatus::Running if has_input_tx => {
                let pending = self.queued_mail.remove(&agent_id).unwrap_or_default();
                let input_tx = self
                    .agents
                    .get(&agent_id)
                    .and_then(|agent| agent.input_tx.clone());
                let mut pending = pending.into_iter();
                let mut undelivered = VecDeque::new();
                let mut delivered = 0_usize;
                if let Some(tx) = input_tx {
                    let counter =
                        Arc::clone(self.pending_follow_ups.entry(agent_id.clone()).or_default());
                    while let Some(mail) = pending.next() {
                        counter.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                        if tx
                            .send(SubAgentInput {
                                text: mail.text.clone(),
                                interrupt: false,
                                pending: Some(Arc::clone(&counter)),
                            })
                            .is_ok()
                        {
                            delivered = delivered.saturating_add(1);
                        } else {
                            counter.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                            undelivered.push_back(mail);
                            undelivered.extend(pending);
                            break;
                        }
                    }
                }
                if !undelivered.is_empty() {
                    self.queued_mail.insert(agent_id.clone(), undelivered);
                }
                receipt.woke = delivered > 0;
                receipt.queue_depth = self
                    .queued_mail
                    .get(&agent_id)
                    .map(VecDeque::len)
                    .unwrap_or(0);
                receipt.continuation_handle = None;
                receipt.note = if receipt.woke && receipt.queue_depth == 0 {
                    self.woken_agents.insert(agent_id.clone(), true);
                    "queued and delivered to running child".to_string()
                } else if receipt.woke {
                    self.woken_agents.insert(agent_id.clone(), true);
                    format!(
                        "partially delivered to running child; {} message(s) remain queued after the live input channel closed",
                        receipt.queue_depth
                    )
                } else {
                    "queued; running child's live input channel is closed".to_string()
                };
                if receipt.woke
                    && let Some(record) = self.worker_records.get_mut(&agent_id)
                {
                    record.follow_up.latest_delivery = Some(AgentRunFollowUpDelivery {
                        delivered: true,
                        timestamp_ms: epoch_millis_now(),
                        message_preview: Some(truncate_preview(&text, 120)),
                        reason: None,
                        interrupt: false,
                        continued_from_checkpoint: false,
                    });
                }
            }
            SubAgentStatus::Running => {
                receipt.woke = false;
                receipt.note =
                    "queued; running child has no live input channel (likely stale handle)"
                        .to_string();
            }
            SubAgentStatus::Interrupted(_) => {
                // Manager-level followup is queue-only; checkpoint resume is
                // wired at the tool layer (`agents/followup`), which
                // re-dispatches a fresh agent loop seeded with the checkpoint
                // messages when a runtime is attached. This arm keeps the
                // honest queue-only receipt for callers without a runtime.
                receipt.woke = false;
                receipt.continued_from_checkpoint = false;
                receipt.continuation_handle = continuation_handle.clone();
                receipt.note = if continuable {
                    format!(
                        "queued; child is interrupted_continuable — attach a runtime to agents/followup to resume from checkpoint, or re-dispatch using continuation_handle={}",
                        continuation_handle.as_deref().unwrap_or("<missing>")
                    )
                } else {
                    "queued; child is interrupted without a continuable checkpoint".to_string()
                };
                if let Some(record) = self.worker_records.get_mut(&agent_id) {
                    record.follow_up.latest_delivery = Some(AgentRunFollowUpDelivery {
                        delivered: false,
                        timestamp_ms: epoch_millis_now(),
                        message_preview: Some(truncate_preview(&text, 120)),
                        reason: Some(receipt.note.clone()),
                        interrupt: false,
                        continued_from_checkpoint: false,
                    });
                }
            }
            other => {
                receipt.woke = false;
                receipt.note = format!(
                    "queued; child status is {} — no live wake performed",
                    subagent_status_name(&other)
                );
            }
        }
        Ok(receipt)
    }

    pub(crate) fn followup_child_for_session(
        &mut self,
        active_session_id: &str,
        agent_ref: &str,
        text: String,
    ) -> Result<ParentMailReceipt> {
        let agent_id = self.resolve_agent_ref_for_session(active_session_id, agent_ref)?;
        self.followup_child(&agent_id, text)
    }

    /// Resume an `interrupted_continuable` child by re-dispatching a fresh
    /// agent loop seeded with the checkpoint message tail and the follow-up
    /// text (checkpoint-based continuation).
    ///
    /// The interrupted terminal record stays immutable — receipts are never
    /// rewritten — so the resumed session runs under a new agent id, matching
    /// the existing "re-dispatch using checkpoint {handle}" guidance. Step and
    /// token budgets are not stored in the checkpoint and are not restored;
    /// the resumed loop starts with the runtime's defaults.
    ///
    /// Callers must hold the manager write lock (same contract as
    /// `spawn_background_with_assignment_options`); `manager_handle` is passed
    /// through to the spawn machinery, which takes no further lock.
    pub(crate) fn resume_from_checkpoint(
        &mut self,
        manager_handle: SharedSubAgentManager,
        runtime: SubAgentRuntime,
        agent_ref: &str,
        followup_text: &str,
    ) -> Result<SubAgentResult> {
        self.resume_from_checkpoint_with_policy(
            manager_handle,
            runtime,
            agent_ref,
            followup_text,
            ResumePolicy::InterruptedOnly,
        )
    }

    pub(crate) fn resume_from_checkpoint_for_session(
        &mut self,
        active_session_id: &str,
        manager_handle: SharedSubAgentManager,
        runtime: SubAgentRuntime,
        agent_ref: &str,
        followup_text: &str,
    ) -> Result<SubAgentResult> {
        let agent_id = self.resolve_agent_ref_for_session(active_session_id, agent_ref)?;
        self.resume_from_checkpoint(manager_handle, runtime, &agent_id, followup_text)
    }

    /// Continue a child on its own fork from the user's side of the shell.
    ///
    /// This is the operator-facing twin of `agents/followup`: a running child
    /// receives the text on its live input channel; an interrupted *or
    /// completed* child whose last checkpoint is continuable is re-dispatched
    /// from that checkpoint under a new agent id (the terminal record stays an
    /// immutable receipt and `resume_targets` links the fork). Failed,
    /// cancelled, and budget-exhausted children cannot be continued.
    pub(crate) fn continue_child_from_user(
        &mut self,
        manager_handle: SharedSubAgentManager,
        runtime: Option<SubAgentRuntime>,
        agent_ref: &str,
        text: &str,
    ) -> Result<UserFollowUpOutcome> {
        let agent_id = self.resolve_agent_ref(agent_ref)?;
        let status = self
            .agents
            .get(&agent_id)
            .map(|agent| agent.status.clone())
            .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;
        match status {
            SubAgentStatus::Running => {
                let receipt = self.followup_child(&agent_id, text.to_string())?;
                Ok(UserFollowUpOutcome {
                    agent_id: agent_id.clone(),
                    target_agent_id: agent_id,
                    delivered: receipt.woke,
                    resumed: false,
                    note: receipt.note,
                })
            }
            SubAgentStatus::Interrupted(_) | SubAgentStatus::Completed => {
                let Some(runtime) = runtime else {
                    return Err(anyhow!(
                        "Cannot continue agent {agent_id}: no runtime is available to resume it"
                    ));
                };
                let resumed = self.resume_from_checkpoint_with_policy(
                    manager_handle,
                    runtime,
                    &agent_id,
                    text,
                    ResumePolicy::InterruptedOrCompleted,
                )?;
                let target = resumed.agent_id.clone();
                Ok(UserFollowUpOutcome {
                    agent_id,
                    delivered: true,
                    resumed: true,
                    note: format!("continued from checkpoint as {target}"),
                    target_agent_id: target,
                })
            }
            other => Err(anyhow!(
                "Cannot continue agent {agent_id}: status is {} and the child cannot resume",
                subagent_status_name(&other)
            )),
        }
    }

    pub(crate) fn continue_child_from_user_for_session(
        &mut self,
        active_session_id: &str,
        manager_handle: SharedSubAgentManager,
        runtime: Option<SubAgentRuntime>,
        agent_ref: &str,
        text: &str,
    ) -> Result<UserFollowUpOutcome> {
        let agent_id = self.resolve_agent_ref_for_session(active_session_id, agent_ref)?;
        self.continue_child_from_user(manager_handle, runtime, &agent_id, text)
    }

    fn resume_from_checkpoint_with_policy(
        &mut self,
        manager_handle: SharedSubAgentManager,
        runtime: SubAgentRuntime,
        agent_ref: &str,
        followup_text: &str,
        policy: ResumePolicy,
    ) -> Result<SubAgentResult> {
        let agent_id = self.resolve_agent_ref(agent_ref)?;
        // Idempotency: a second resume on the same interrupted id returns the
        // already-resumed target instead of spawning a duplicate agent loop
        // that would concurrently write the same workspace.
        if let Some(existing) = self.resume_targets.get(&agent_id).cloned() {
            // Forward the follow-up to the already-resumed target (best
            // effort; a terminal target simply cannot receive it) so a
            // retried followup is never silently dropped.
            let _ = self.followup_child(&existing, followup_text.to_string());
            return self.get_result(&existing);
        }
        let (
            agent_type,
            resume_prompt,
            assignment,
            allowed_tools,
            model,
            fork_context,
            workspace,
            claim,
            preserved_profile,
            child_route,
        ) = {
            let agent = self
                .agents
                .get(&agent_id)
                .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;
            let resumable = match policy {
                ResumePolicy::InterruptedOnly => {
                    matches!(agent.status, SubAgentStatus::Interrupted(_))
                }
                ResumePolicy::InterruptedOrCompleted => matches!(
                    agent.status,
                    SubAgentStatus::Interrupted(_) | SubAgentStatus::Completed
                ),
            };
            if !resumable {
                return Err(anyhow!(
                    "Cannot resume agent {agent_id}: status is {} ({})",
                    subagent_status_name(&agent.status),
                    policy.describe()
                ));
            }
            let checkpoint = agent
                .checkpoint
                .as_ref()
                .filter(|cp| cp.continuable && !cp.messages.is_empty())
                .ok_or_else(|| {
                    let continuable = agent.checkpoint.as_ref().is_some_and(|cp| cp.continuable);
                    let messages = agent
                        .checkpoint
                        .as_ref()
                        .map(|cp| cp.messages.len())
                        .unwrap_or(0);
                    anyhow!(
                        "Agent {agent_id} has no continuable checkpoint to resume from (continuable={continuable}, messages={messages})"
                    )
                })?;
            // Restore the interrupted child's write claim so the resumed loop
            // stays inside the coordination ledger with the original bounded
            // scope instead of inheriting the caller's unchecked write surface.
            // The ledger claim is already namespaced and carries the isolation
            // flag; both are passed through to the spawn seam.
            let claim = self
                .coordination
                .write_claims
                .iter()
                .find(|record| record.claim.owner == agent_id)
                .map(|record| (record.claim.clone(), record.isolated_worktree));
            // Preserve the interrupted child's runtime posture (read_only /
            // denied tools / shell) instead of rebuilding from the caller's
            // role, which could widen the resumed child's authority.
            let preserved_profile = self
                .worker_records
                .get(&agent_id)
                .map(|record| record.spec.runtime_profile.clone());
            let child_route = self
                .worker_records
                .get(&agent_id)
                .and_then(|record| record.spec.child_route.clone());
            (
                agent.agent_type.clone(),
                build_resume_prompt(&agent.prompt, checkpoint, followup_text),
                agent.assignment.clone(),
                agent.allowed_tools.clone(),
                agent.model.clone(),
                agent.fork_context,
                agent.workspace.clone(),
                claim,
                preserved_profile,
                child_route,
            )
        };
        // Resume runs at child depth with a detached cancellation token, the
        // same seam a fresh spawn uses; fail closed on the depth ceiling.
        // Checked on the parent runtime before derivation, matching the
        // fresh-spawn order (would_exceed_depth at the spawn seam).
        if runtime.would_exceed_depth() {
            return Err(anyhow!(
                "Cannot resume agent {agent_id}: sub-agent depth limit reached (current {}, max {})",
                runtime.spawn_depth,
                runtime.max_spawn_depth
            ));
        }
        let runtime = runtime.background_runtime();
        // Resume in the interrupted child's workspace, not the caller's
        // (worktree/cwd children must not resume in the parent directory).
        let mut runtime = runtime;
        runtime.context.workspace = workspace;
        let options = SubAgentSpawnOptions {
            name: None, // the old session name stays owned by the terminal record
            model: Some(model),
            model_route: None,
            child_route,
            nickname: None,
            fork_context,
            write_claim: claim.as_ref().map(|(claim, _)| claim.clone()),
            isolated_worktree: claim
                .as_ref()
                .map(|(_, isolated)| *isolated)
                .unwrap_or(false),
            claim_pre_namespaced: claim.is_some(),
            preserve_runtime_profile: preserved_profile,
            ..Default::default()
        };
        let resumed = self.spawn_background_with_assignment_options(
            manager_handle,
            runtime,
            agent_type,
            resume_prompt,
            assignment,
            allowed_tools,
            options,
        )?;
        self.resume_targets
            .insert(agent_id, resumed.agent_id.clone());
        Ok(resumed)
    }

    /// Interrupt a child, preserve checkpoint, fail closed on root/self.
    pub fn interrupt_child(
        &mut self,
        agent_ref: &str,
        caller_agent_id: Option<&str>,
        reason: String,
    ) -> Result<(SubAgentResult, SubAgentResult)> {
        if agent_ref.trim().eq_ignore_ascii_case("root") {
            return Err(anyhow!(
                "Refusing to interrupt root. agents/interrupt fails closed on the root session."
            ));
        }
        let agent_id = self.resolve_agent_ref(agent_ref)?;
        self.ensure_caller_controls_descendant(&agent_id, caller_agent_id, "agents/interrupt")?;

        let prior = self.get_result_by_ref(&agent_id)?;
        if prior.status != SubAgentStatus::Running
            || self
                .agents
                .get(&agent_id)
                .is_some_and(|agent| agent.completion_claimed)
        {
            return Ok((prior.clone(), prior));
        }

        // Build a continuable checkpoint from the latest stored checkpoint or a
        // minimal placeholder so interrupt never drops recoverability silently.
        let checkpoint = {
            let agent = self
                .agents
                .get(&agent_id)
                .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;
            agent.checkpoint.clone().unwrap_or_else(|| {
                build_subagent_checkpoint(&agent_id, &reason, &[], agent.steps_taken, true)
            })
        };

        let mut terminal = prior.clone();
        terminal.status = SubAgentStatus::Interrupted(reason.clone());
        terminal.result = Some(reason);
        terminal.steps_taken = checkpoint.steps_taken;
        terminal.checkpoint = Some(checkpoint);
        terminal.needs_input = None;
        if !self.finish_terminal_result(&agent_id, terminal, true, true) {
            return Ok((prior, self.get_result(&agent_id)?));
        }
        let snapshot = self.get_result(&agent_id)?;
        Ok((prior, snapshot))
    }

    pub(crate) fn interrupt_child_for_session(
        &mut self,
        active_session_id: &str,
        agent_ref: &str,
        caller_agent_id: Option<&str>,
        reason: String,
    ) -> Result<(SubAgentResult, SubAgentResult)> {
        let agent_id = self.resolve_agent_ref_for_session(active_session_id, agent_ref)?;
        self.interrupt_child(&agent_id, caller_agent_id, reason)
    }

    /// Follow-ups per running child that were handed to its live input
    /// channel but not yet taken at a round boundary. Only non-zero entries.
    #[cfg(test)]
    pub fn queued_follow_up_counts(&self) -> HashMap<String, usize> {
        self.queued_follow_up_counts_inner(None)
    }

    pub(crate) fn queued_follow_up_counts_for_session(
        &self,
        active_session_id: &str,
    ) -> HashMap<String, usize> {
        self.queued_follow_up_counts_inner(Some(active_session_id))
    }

    fn queued_follow_up_counts_inner(
        &self,
        active_session_id: Option<&str>,
    ) -> HashMap<String, usize> {
        self.pending_follow_ups
            .iter()
            .filter_map(|(agent_id, counter)| {
                if active_session_id.is_some_and(|session_id| {
                    !self.agent_id_is_owned_by_session(agent_id, session_id)
                }) {
                    return None;
                }
                let count = counter.load(std::sync::atomic::Ordering::Acquire);
                (count > 0).then(|| (agent_id.clone(), count))
            })
            .collect()
    }

    pub(crate) fn list_coordination_summaries_for_session(
        &self,
        active_session_id: &str,
        include_archived: bool,
        recent_limit: usize,
    ) -> Vec<AgentCoordSummary> {
        self.list_filtered_for_session(active_session_id, include_archived)
            .into_iter()
            .filter_map(|snap| {
                self.coordination_summary_for_session(
                    active_session_id,
                    &snap.agent_id,
                    recent_limit,
                )
                .ok()
            })
            .collect()
    }

    pub fn coordination_summary_for(
        &self,
        agent_ref: &str,
        recent_limit: usize,
    ) -> Result<AgentCoordSummary> {
        let agent_id = self.resolve_agent_ref(agent_ref)?;
        let snap = self.get_result_by_ref(&agent_id)?;
        let record = self.worker_records.get(&agent_id);
        let recent_progress = record
            .map(|r| {
                r.events
                    .iter()
                    .rev()
                    .filter_map(|ev| ev.message.clone())
                    .take(recent_limit)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            })
            .unwrap_or_default();
        let queued_mail = self
            .queued_mail
            .get(&agent_id)
            .map(VecDeque::len)
            .unwrap_or(0);
        let continuable = subagent_checkpoint_is_continuable(&snap);
        let write_claim = self
            .coordination
            .write_claims
            .iter()
            .find(|claim| claim.claim.owner == agent_id)
            .cloned();
        let accepted_decisions = self
            .coordination
            .decisions
            .iter()
            .rev()
            .filter(|decision| {
                decision.owner == agent_id && decision.status == DecisionStatus::Accepted
            })
            .take(recent_limit)
            .cloned()
            .collect();
        Ok(AgentCoordSummary {
            agent_id: snap.agent_id.clone(),
            name: snap.name.clone(),
            parent_run_id: record.and_then(|r| r.parent_run_id.clone()),
            child_route: record.and_then(|r| r.spec.child_route.clone()),
            status: subagent_status_name(&snap.status).to_string(),
            steps_taken: snap.steps_taken,
            token_budget: record.and_then(|r| r.usage.token_budget),
            budget_spent_tokens: record.and_then(|r| r.usage.budget_spent_tokens),
            budget_remaining_tokens: record.and_then(|r| r.usage.budget_remaining_tokens),
            recent_progress,
            queued_mail,
            checkpoint_id: snap.checkpoint.as_ref().map(|c| c.checkpoint_id.clone()),
            continuable,
            write_claim,
            accepted_decisions,
        })
    }

    pub(crate) fn coordination_summary_for_session(
        &self,
        active_session_id: &str,
        agent_ref: &str,
        recent_limit: usize,
    ) -> Result<AgentCoordSummary> {
        let agent_id = self.resolve_agent_ref_for_session(active_session_id, agent_ref)?;
        self.coordination_summary_for(&agent_id, recent_limit)
    }

    #[allow(dead_code)] // coord list/wait surfaces; wired when agents/list hosts go live
    pub fn queued_mail_depth(&self, agent_id: &str) -> Option<usize> {
        self.queued_mail.get(agent_id).map(VecDeque::len)
    }

    #[allow(dead_code)] // followup honesty probe for coordination tools
    pub fn child_was_woken(&self, agent_id: &str) -> bool {
        self.woken_agents.get(agent_id).copied().unwrap_or(false)
    }

    /// Fingerprint of recent progress for activity waits.
    pub fn activity_fingerprint(&self, agent_id: &str) -> Option<u64> {
        let agent = self.agents.get(agent_id)?;
        let record = self.worker_records.get(agent_id);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        subagent_status_name(&agent.status).hash(&mut hasher);
        agent.steps_taken.hash(&mut hasher);
        if let Some(record) = record {
            record.events.len().hash(&mut hasher);
            if let Some(last) = record.events.back() {
                last.seq.hash(&mut hasher);
                last.message.hash(&mut hasher);
            }
        }
        let queued = self
            .queued_mail
            .get(agent_id)
            .map(VecDeque::len)
            .unwrap_or(0);
        queued.hash(&mut hasher);
        Some(hasher.finish())
    }

    /// Test helper: seed a running child with a live input channel.
    #[cfg(test)]
    pub fn insert_test_running_agent(&mut self, name: &str, workspace: &Path) -> String {
        self.insert_test_running_agent_with_input(name, workspace).0
    }

    #[cfg(test)]
    pub fn assign_test_session_owner(&mut self, agent_id: &str, owner_session_id: &str) {
        self.agents
            .get_mut(agent_id)
            .expect("test agent")
            .owner_session_id = owner_session_id.to_string();
        if let Some(record) = self.worker_records.get_mut(agent_id) {
            record.owner_session_id = owner_session_id.to_string();
        }
    }

    /// Test helper exposing the receiving side so delivery and provenance can
    /// be verified rather than inferred from a still-present sender handle.
    #[cfg(test)]
    fn insert_test_running_agent_with_input(
        &mut self,
        name: &str,
        workspace: &Path,
    ) -> (String, mpsc::UnboundedReceiver<SubAgentInput>) {
        let agent_id = format!("agent_{name}");
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let mut agent = SubAgent::new(
            agent_id.clone(),
            FleetRole::Worker,
            "test".to_string(),
            SubAgentAssignment::new("test".to_string(), None),
            "test-model".to_string(),
            None,
            None,
            input_tx,
            workspace.to_path_buf(),
            self.current_session_boot_id.clone(),
        );
        agent.session_name = name.to_string();
        agent.status = SubAgentStatus::Running;
        // `ToolContext::new` uses this deterministic namespace in unit tests.
        // Tests exercising real conversation boundaries override it through
        // `assign_test_session_owner`.
        agent.owner_session_id = "workspace".to_string();
        // Make the test agent live for 4a liveness (handle required, otherwise
        // list_filtered hides it as phantom). Use try_current + leaked runtime
        // fallback so sync tests also work.
        let handle = if let Ok(h) = tokio::runtime::Handle::try_current() {
            h.spawn(async {
                std::future::pending::<()>().await;
            })
        } else {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime");
            let h = rt.spawn(async {
                std::future::pending::<()>().await;
            });
            std::mem::forget(rt);
            h
        };
        agent.task_handle = Some(handle);
        self.agents.insert(agent_id.clone(), agent);
        let spec = AgentWorkerSpec {
            worker_id: agent_id.clone(),
            run_id: agent_id.clone(),
            parent_run_id: Some("parent_session".to_string()),
            session_name: Some(name.to_string()),
            objective: "test".to_string(),
            role: None,
            agent_type: FleetRole::Worker,
            model: "test-model".to_string(),
            workspace: workspace.to_path_buf(),
            git_branch: None,
            context_mode: "fresh".to_string(),
            fork_context: false,
            tool_profile: AgentWorkerToolProfile::Inherited,
            runtime_profile: WorkerRuntimeProfile::default(),
            max_steps: WorkerRuntimeProfile::default().max_steps,
            spawn_depth: 1,
            max_spawn_depth: 3,
            child_route: None,
            launch_manifest: None,
        };
        self.register_worker_for_session(spec, "workspace");
        (agent_id, input_rx)
    }

    /// Test helper: seed a current-session direct child whose future terminal
    /// result is eligible for automatic parent delivery.
    #[cfg(test)]
    pub fn insert_test_running_direct_child(&mut self, name: &str, workspace: &Path) -> String {
        let agent_id = self.insert_test_running_agent(name, workspace);
        if let Some(record) = self.worker_records.get_mut(&agent_id) {
            record.parent_run_id = None;
            record.spec.parent_run_id = None;
        }
        agent_id
    }

    /// Test helper: seed a settled direct child whose result has not yet been
    /// delivered to the parent.
    #[cfg(test)]
    pub fn insert_test_terminal_direct_child(&mut self, name: &str, workspace: &Path) -> String {
        let agent_id = self.insert_test_running_direct_child(name, workspace);
        if let Some(agent) = self.agents.get_mut(&agent_id) {
            agent.status = SubAgentStatus::Completed;
            agent.result = Some("test terminal result".to_string());
        }
        if let Some(record) = self.worker_records.get_mut(&agent_id) {
            record.status = AgentWorkerStatus::Completed;
        }
        agent_id
    }

    /// Test helper: seed an interrupted_continuable child with a checkpoint.
    #[cfg(test)]
    pub fn insert_test_interrupted_continuable_agent(
        &mut self,
        name: &str,
        workspace: &Path,
        messages: Vec<crate::models::Message>,
    ) -> (String, String) {
        let agent_id = self.insert_test_running_agent(name, workspace);
        let checkpoint = build_subagent_checkpoint(&agent_id, "test_interrupt", &messages, 1, true);
        let handle = checkpoint.continuation_handle.clone();
        if let Some(agent) = self.agents.get_mut(&agent_id) {
            agent.status = SubAgentStatus::Interrupted("test interrupt".to_string());
            agent.checkpoint = Some(checkpoint);
            agent.input_tx = None;
            agent.task_handle = None;
        }
        (agent_id, handle)
    }

    /// Count running agents.
    pub fn running_count(&self) -> usize {
        self.admitted_count()
    }

    pub(crate) fn running_count_for_session(&self, active_session_id: &str) -> usize {
        self.agents
            .values()
            .filter(|agent| self.agent_is_owned_by_session(agent, active_session_id))
            .filter(|agent| {
                agent.status == SubAgentStatus::Running
                    && agent.task_handle.is_some()
                    && !self.running_heartbeat_timed_out(agent)
            })
            .count()
    }

    /// Count live sub-agents that have been admitted, including queued
    /// workers waiting on the launch gate.
    pub fn admitted_count(&self) -> usize {
        self.agents
            .values()
            .filter(|agent| {
                // Exclude non-running statuses
                if agent.status != SubAgentStatus::Running {
                    return false;
                }
                // Exclude persisted agents with no task_handle (they're not actually running)
                if agent.task_handle.is_none() {
                    return false;
                }
                // Keep recently finished handles counted until the terminal
                // status update has reconciled. Otherwise a fanout burst can
                // refill the cap before the UI/state catches up (#2211).
                !self.running_heartbeat_timed_out(agent)
            })
            .count()
    }

    /// Count admitted workers that are currently waiting for the launch gate.
    pub fn queued_count(&self) -> usize {
        self.agents
            .values()
            .filter(|agent| {
                agent.status == SubAgentStatus::Running
                    && agent.task_handle.is_some()
                    && !self.running_heartbeat_timed_out(agent)
                    && self
                        .worker_records
                        .get(&agent.id)
                        .is_some_and(|record| record.status == AgentWorkerStatus::Queued)
            })
            .count()
    }

    /// Count admitted workers not currently in the queued launch state.
    pub fn active_count(&self) -> usize {
        self.admitted_count().saturating_sub(self.queued_count())
    }

    fn check_admission_capacity(&self) -> Result<()> {
        let admitted = self.admitted_count();
        if admitted >= self.max_admitted_agents {
            return Err(anyhow!(
                "Sub-agent admission limit reached (max_admitted {}, admitted {}, running {}, queued {}). Wait for queued/running agents to finish, cancel unneeded agents, or raise [subagents] max_admitted for this Workflow.",
                self.max_admitted_agents,
                admitted,
                self.active_count(),
                self.queued_count()
            ));
        }
        Ok(())
    }

    fn running_heartbeat_timed_out(&self, agent: &SubAgent) -> bool {
        agent.status == SubAgentStatus::Running
            && agent.task_handle.is_some()
            && agent.last_activity_at.elapsed() >= self.running_heartbeat_timeout
    }

    pub fn touch(&mut self, agent_id: &str) -> bool {
        let Some(agent) = self.agents.get_mut(agent_id) else {
            return false;
        };
        if agent.status != SubAgentStatus::Running {
            return false;
        }
        agent.last_activity_at = Instant::now();
        true
    }

    pub(crate) fn touch_for_session(&mut self, active_session_id: &str, agent_id: &str) -> bool {
        if !self.agent_id_is_owned_by_session(agent_id, active_session_id) {
            return false;
        }
        self.touch(agent_id)
    }

    /// Spawn a new background sub-agent with explicit assignment and display
    /// metadata.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_background_with_assignment_options(
        &mut self,
        manager_handle: SharedSubAgentManager,
        mut runtime: SubAgentRuntime,
        agent_type: FleetRole,
        mut prompt: String,
        assignment: SubAgentAssignment,
        allowed_tools: Option<Vec<String>>,
        options: SubAgentSpawnOptions,
    ) -> Result<SubAgentResult> {
        self.cleanup(COMPLETED_AGENT_RETENTION);

        self.check_admission_capacity()?;

        if let Some(model) = options.model.as_deref() {
            runtime.model = model.to_string();
        }
        let effective_model = runtime.model.clone();
        let agent_id = format!("agent_{}", &Uuid::new_v4().to_string()[..8]);
        // Admission into the turn-owned barrier happens before worker records,
        // mailbox events, or task scheduling. Once turn settlement closes the
        // registry, a racing descendant is refused rather than escaping the
        // one-shot join and publishing after TurnComplete.
        let foreground_child_registration = runtime.foreground_child_registration(&agent_id)?;
        let budget_scope = self.resolve_spawn_budget_scope(
            &agent_id,
            runtime.parent_agent_id.as_deref(),
            options.token_budget,
        )?;
        let active_names: std::collections::HashSet<String> = self
            .agents
            .values()
            .filter_map(|a| a.nickname.clone())
            .collect();
        let nickname = options.nickname.or_else(|| {
            Some(assign_unique_whale_name_in_locale(
                &agent_id,
                &active_names,
                &runtime.locale_tag,
            ))
        });
        let tools = build_allowed_tools(&agent_type, allowed_tools, runtime.allow_shell)?;
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let mut agent = SubAgent::new(
            agent_id.clone(),
            agent_type.clone(),
            prompt.clone(),
            assignment.clone(),
            effective_model,
            nickname,
            tools.clone(),
            input_tx,
            runtime.context.workspace.clone(),
            self.current_session_boot_id.clone(),
        );
        if let Some(name) = options
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            // Names are scoped to the live session: a completed worker
            // hydrated from a previous session's ledger is invisible to
            // `status`/`peek`/`followup`, so it must not reserve the name
            // either (cloud-agent e2e, 2026-08-30: a fresh `exec` in the same
            // workspace could not spawn `worker-a` again).
            if let Some(existing) = self.agents.values().find(|existing| {
                existing.session_name == name && !self.is_from_prior_session(existing)
            }) {
                // #3020: Include elapsed time so the parent can distinguish a
                // live worker from a stale/failed earlier spawn (#2656).
                let elapsed = existing.started_at.elapsed();
                let since = format!(
                    "{} ago",
                    crate::elapsed::format_elapsed_secs(elapsed.as_secs())
                );
                return Err(anyhow!(
                    "Sub-agent session name '{name}' is already in use by agent_id '{}' \
                     (status: {}, started {since}). \
                     Wait for its completion event, or open a new agent with a different name.",
                    existing.id,
                    subagent_status_name(&existing.status)
                ));
            }
            agent.session_name = name.to_string();
        }
        agent.fork_context = options.fork_context;
        let agent_id = agent.id.clone();
        let started_at = agent.started_at;
        let tool_profile = match tools.clone() {
            Some(tools) => AgentWorkerToolProfile::Explicit(tools),
            None => AgentWorkerToolProfile::Inherited,
        };
        let runtime_profile = match options.preserve_runtime_profile.clone() {
            Some(preserved) => {
                runtime.worker_profile = preserved.clone();
                preserved
            }
            None => {
                let profile = worker_profile_for_spawn(
                    &runtime,
                    &agent_type,
                    &tool_profile,
                    &agent.model,
                    options.model_route.clone(),
                    options.write_claim.is_some(),
                );
                runtime.worker_profile = profile.clone();
                profile
            }
        };
        let write_capable = runtime_profile.permissions.write;
        if write_capable {
            // Isolated-worktree children mutate their own checkout, so they
            // do not contend for the shared-workspace process lock (#5036).
            if !options.isolated_worktree {
                self.ensure_coordination_process_lock()
                    .map_err(anyhow::Error::msg)?;
            }
            if self.coordination_process_lock_required && self.state_path.is_none() {
                return Err(anyhow!(
                    "write-capable sub-agent launch requires a durable coordination state path"
                ));
            }
        }
        let durable_launch_snapshot =
            write_capable.then(|| (self.worker_records.clone(), self.coordination.clone()));
        let persisted_claim = if write_capable {
            options
                .write_claim
                .clone()
                .map(|mut claim| {
                    claim.owner = agent_id.clone();
                    let claim = if options.claim_pre_namespaced {
                        claim
                    } else {
                        self.namespace_write_claim(
                            &agent.workspace,
                            options.isolated_worktree,
                            claim,
                        )?
                    };
                    let active_owners = self.active_coordination_owners();
                    self.coordination
                        .register_claim(claim, options.isolated_worktree, |owner| {
                            active_owners.contains(owner)
                        })
                })
                .transpose()
        } else {
            Ok(None)
        };
        let persisted_claim = match persisted_claim {
            Ok(claim) => claim,
            Err(error) => {
                if let Err(persist_error) = self.persist_state_synchronously() {
                    if let Some((worker_records, coordination)) = durable_launch_snapshot.as_ref() {
                        self.worker_records = worker_records.clone();
                        self.coordination = coordination.clone();
                    }
                    return Err(anyhow!(
                        "{error}; additionally failed to persist contention receipt: {persist_error}"
                    ));
                }
                return Err(anyhow!(error));
            }
        };
        let mut projection_capabilities = tools.clone().unwrap_or_else(|| {
            ["Bash", "File", "Git", "Run", "Web"]
                .into_iter()
                .map(str::to_string)
                .collect()
        });
        projection_capabilities.push(agent_type.as_str().to_string());
        if let Some(role) = assignment.role.as_ref()
            && !projection_capabilities.contains(role)
        {
            projection_capabilities.push(role.clone());
        }
        let (decision_projection, _) = self.coordination.project_relevant_decisions(
            &agent_id,
            persisted_claim.as_ref().map(|record| &record.claim),
            &projection_capabilities,
        );
        if !decision_projection.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&decision_projection);
            agent.prompt = prompt.clone();
        }
        if let Some(claim) = persisted_claim.as_ref().map(|record| &record.claim) {
            prompt.push_str(&format!(
                "\n\nWrite scope (enforced; coordination-root-relative): roots={:?}; exact_files={:?}; contracts={:?}. Expand it with agent action=claim before mutating anything outside this scope.",
                claim.roots, claim.exact_files, claim.contracts
            ));
            agent.prompt = prompt.clone();
        }
        let max_steps = resolve_max_steps(agent_type.clone(), options.max_steps, self.max_steps);
        runtime.worker_profile.max_steps = max_steps;
        let wall_time = options
            .wall_time
            .or(self.wall_time)
            .unwrap_or(DEFAULT_CHILD_WALL_TIME)
            .min(MAX_CHILD_WALL_TIME);
        let worker_spec = AgentWorkerSpec {
            worker_id: agent_id.clone(),
            run_id: agent_id.clone(),
            parent_run_id: runtime.parent_agent_id.clone(),
            session_name: Some(agent.session_name.clone()),
            objective: assignment.objective.clone(),
            role: assignment.role.clone(),
            agent_type: agent_type.clone(),
            model: agent.model.clone(),
            workspace: agent.workspace.clone(),
            git_branch: current_git_branch(&agent.workspace),
            context_mode: if options.fork_context {
                "forked"
            } else {
                "fresh"
            }
            .to_string(),
            fork_context: options.fork_context,
            tool_profile,
            runtime_profile: runtime_profile.clone(),
            max_steps,
            spawn_depth: runtime.spawn_depth,
            max_spawn_depth: runtime.max_spawn_depth,
            child_route: options.child_route.clone(),
            launch_manifest: Some(ChildLaunchManifest {
                owner_session: runtime
                    .parent_agent_id
                    .clone()
                    .unwrap_or_else(|| "root".to_string()),
                child_id: agent_id.clone(),
                profile: runtime_profile,
                prompt: prompt.clone(),
                cwd: Some(agent.workspace.display().to_string()),
                worktree: options.isolated_worktree,
                writable_roots: persisted_claim
                    .as_ref()
                    .map(|record| record.claim.roots.clone())
                    .unwrap_or_default(),
                writable_files: persisted_claim
                    .as_ref()
                    .map(|record| record.claim.exact_files.clone())
                    .unwrap_or_default(),
                coordination_contracts: persisted_claim
                    .as_ref()
                    .map(|record| record.claim.contracts.clone())
                    .unwrap_or_default(),
                expected_artifact: options.expected_artifact.clone(),
                token_budget: options.token_budget,
                resume_identity: Some(agent.session_name.clone()),
                generation: 1,
                resume_from_agent_id: options.resume_from_agent_id.clone(),
            }),
        };
        agent.work_lifecycle =
            match SubAgentWorkLifecycle::register(&runtime, &agent_id, &assignment.objective) {
                Ok(lifecycle) => lifecycle,
                Err(error) => {
                    if let Some((worker_records, coordination)) = durable_launch_snapshot.as_ref() {
                        self.worker_records = worker_records.clone();
                        self.coordination = coordination.clone();
                    }
                    return Err(error);
                }
            };
        agent.owner_session_id = runtime.context.state_namespace.clone();
        agent.terminal_delivery = Some(SubAgentTerminalDeliveryContext::from_runtime(&runtime));
        self.register_worker_for_session(worker_spec, &runtime.context.state_namespace);
        if let Some(scope) = budget_scope {
            self.attach_budget_scope(&agent_id, scope);
        }

        // Shared-workspace writers may execute only after their exact worker
        // identity and claim are durably replayable. Persist a Starting record
        // while the manager write lock still excludes the child; then launch.
        // A crash can therefore leave an interrupted owner, never an accepted
        // edit with no durable scope/identity record.
        if write_capable {
            self.agents.insert(agent_id.clone(), agent);
            let persist_result = self.persist_state_synchronously();
            agent = self
                .agents
                .remove(&agent_id)
                .expect("pre-launch agent remains registered under manager lock");
            if let Err(error) = persist_result {
                let (worker_records, coordination) = durable_launch_snapshot
                    .expect("write-capable launch captured a registration snapshot");
                self.worker_records = worker_records;
                self.coordination = coordination;
                if let Some(lifecycle) = agent.work_lifecycle.as_ref() {
                    let _ = lifecycle.reconcile_state(OwnerState::Failed, 1, None);
                }
                return Err(anyhow!(
                    "failed to durably register write-capable sub-agent before launch: {error}"
                ));
            }
        }

        if let Some(mb) = runtime.mailbox.as_ref() {
            let _ = mb.send(MailboxMessage::started(&agent_id, agent_type.clone()));
        }

        if let Some(event_tx) = runtime.event_tx.clone() {
            let _ = event_tx.try_send(Event::AgentSpawned {
                owner_session_id: runtime.context.state_namespace.clone(),
                id: agent_id.clone(),
                prompt: prompt.clone(),
                parent_run_id: runtime.parent_agent_id.clone(),
                spawn_depth: runtime.spawn_depth,
                // The model the child was actually installed with. Read here
                // rather than from session state so a later `/model` switch
                // cannot rewrite a launched child's attribution.
                model: agent.model.clone(),
                // Route provenance is resolved on the workflow spawn seam
                // (`WorkflowTaskSpawnMetadata`), not on this path, so it is
                // honestly absent rather than guessed. The model — the half
                // that determines billing — is present either way.
                route_source: None,
            });
        }

        let launch_gate = (runtime.spawn_depth == 1).then(|| self.launch_gate.clone());
        let task = SubAgentTask {
            manager_handle,
            runtime,
            agent_id: agent_id.clone(),
            agent_type,
            prompt,
            assignment,
            allowed_tools: tools,
            fork_context: options.fork_context,
            started_at,
            max_steps,
            token_budget: options.token_budget,
            wall_time,
            input_rx,
            launch_gate,
            _foreground_child_registration: foreground_child_registration,
        };
        let handle = spawn_supervised(
            "subagent-task",
            std::panic::Location::caller(),
            run_subagent_task(task),
        );
        agent.task_handle = Some(handle);
        self.agents.insert(agent_id.clone(), agent);
        self.record_worker_event(
            &agent_id,
            AgentWorkerStatus::Running,
            Some("running".to_string()),
            None,
            None,
        );
        self.persist_state_best_effort();

        let agent = self
            .agents
            .get(&agent_id)
            .expect("agent should exist after spawn");
        Ok(self.snapshot_for_listing(agent))
    }

    /// Get the current snapshot for an agent.
    pub fn get_result(&self, agent_id: &str) -> Result<SubAgentResult> {
        let agent = self
            .agents
            .get(agent_id)
            .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;
        Ok(self.snapshot_for_listing(agent))
    }

    pub fn get_result_by_ref(&self, agent_ref: &str) -> Result<SubAgentResult> {
        let agent_id = self.resolve_agent_ref(agent_ref)?;
        self.get_result(&agent_id)
    }

    /// Get an agent snapshot only when it belongs to the active root session.
    ///
    /// The error deliberately does not distinguish a foreign agent from a
    /// missing one. User- and model-facing callers must not gain an existence
    /// oracle for another conversation by guessing a durable id or name.
    pub(crate) fn get_result_by_ref_for_session(
        &self,
        active_session_id: &str,
        agent_ref: &str,
    ) -> Result<SubAgentResult> {
        let agent_id = self.resolve_agent_ref_for_session(active_session_id, agent_ref)?;
        self.get_result(&agent_id)
    }

    #[cfg(test)]
    pub fn terminal_results_excluding(
        &self,
        delivered_ids: &std::collections::HashSet<String>,
    ) -> Vec<SubAgentResult> {
        self.terminal_results_excluding_inner(None, delivered_ids)
    }

    /// Return terminal direct-child results owned by the active root session.
    ///
    /// The manager and its persisted roster can survive `SyncSession`; exact
    /// owner matching prevents a completed child from the previous
    /// conversation being synthesized into the new turn. Empty legacy owners
    /// do not match and therefore fail closed.
    pub(crate) fn terminal_results_excluding_for_session(
        &self,
        active_session_id: &str,
        delivered_ids: &std::collections::HashSet<String>,
    ) -> Vec<SubAgentResult> {
        self.terminal_results_excluding_inner(Some(active_session_id), delivered_ids)
    }

    fn terminal_results_excluding_inner(
        &self,
        active_session_id: Option<&str>,
        delivered_ids: &std::collections::HashSet<String>,
    ) -> Vec<SubAgentResult> {
        let mut results = self
            .agents
            .values()
            .filter(|agent| agent.status != SubAgentStatus::Running)
            .filter(|agent| agent.session_boot_id == self.current_session_boot_id)
            .filter(|agent| {
                active_session_id.is_none_or(|session_id| agent.owner_session_id == session_id)
            })
            .filter(|agent| {
                self.worker_records
                    .get(&agent.id)
                    .is_none_or(|record| record.spec.parent_run_id.is_none())
            })
            .filter(|agent| !delivered_ids.contains(&agent.id))
            .map(|agent| self.snapshot_for_listing(agent))
            .collect::<Vec<_>>();
        results.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        results
    }

    /// Whether a direct child can still inject a completion into the parent
    /// before its next provider request.
    ///
    /// This is deliberately read-only and conservative: a current-session
    /// direct child is eligible while it is still running, and remains
    /// eligible after settling until its terminal result has been delivered.
    /// Preview uses this predicate instead of draining either completion
    /// channel, so inspecting the request cannot consume or claim the state it
    /// is reporting.
    pub(crate) fn may_transform_next_parent_request_for_session(
        &self,
        active_session_id: &str,
        delivered_ids: &std::collections::HashSet<String>,
    ) -> bool {
        self.may_transform_next_parent_request_inner(Some(active_session_id), delivered_ids)
    }

    fn may_transform_next_parent_request_inner(
        &self,
        active_session_id: Option<&str>,
        delivered_ids: &std::collections::HashSet<String>,
    ) -> bool {
        self.agents.values().any(|agent| {
            agent.session_boot_id == self.current_session_boot_id
                && active_session_id
                    .is_none_or(|session_id| self.agent_is_owned_by_session(agent, session_id))
                && self
                    .worker_records
                    .get(&agent.id)
                    .is_none_or(|record| record.spec.parent_run_id.is_none())
                && !delivered_ids.contains(&agent.id)
        })
    }

    /// Resolve either a durable agent id or a model-facing session name.
    fn resolve_agent_ref(&self, agent_ref: &str) -> Result<String> {
        self.resolve_agent_ref_inner(agent_ref, None)
    }

    fn resolve_agent_ref_for_session(
        &self,
        active_session_id: &str,
        agent_ref: &str,
    ) -> Result<String> {
        self.resolve_agent_ref_inner(agent_ref, Some(active_session_id))
            .map_err(|_| anyhow!("Agent not found in the active session"))
    }

    fn resolve_agent_ref_inner(
        &self,
        agent_ref: &str,
        active_session_id: Option<&str>,
    ) -> Result<String> {
        let agent_ref = agent_ref.trim();
        if let Some(agent) = self.agents.get(agent_ref)
            && active_session_id
                .is_none_or(|session_id| self.agent_is_owned_by_session(agent, session_id))
        {
            return Ok(agent.id.clone());
        }

        let matches = self
            .agents
            .values()
            .filter(|agent| agent.session_name == agent_ref)
            .filter(|agent| {
                active_session_id
                    .is_none_or(|session_id| self.agent_is_owned_by_session(agent, session_id))
            })
            .map(|agent| agent.id.clone())
            .collect::<Vec<_>>();

        match matches.as_slice() {
            [id] => Ok(id.clone()),
            [] => Err(anyhow!("Agent session {agent_ref} not found")),
            _ => Err(anyhow!(
                "Agent session name '{agent_ref}' is ambiguous; use an agent_id"
            )),
        }
    }

    fn agent_is_owned_by_session(&self, agent: &SubAgent, active_session_id: &str) -> bool {
        !active_session_id.is_empty() && agent.owner_session_id == active_session_id
    }

    fn agent_id_is_owned_by_session(&self, agent_id: &str, active_session_id: &str) -> bool {
        self.agents
            .get(agent_id)
            .is_some_and(|agent| self.agent_is_owned_by_session(agent, active_session_id))
    }

    /// Resolve a hierarchy mutation target and prove that it is a strict
    /// descendant of the calling agent. Root registries carry no caller id
    /// (or the literal `root`) and retain authority over every child. This
    /// prevents a child from messaging, waking, or interrupting a sibling or
    /// ancestor and then claiming the ownership that target released.
    pub(super) fn ensure_caller_controls_descendant(
        &self,
        agent_ref: &str,
        caller_agent_id: Option<&str>,
        action: &str,
    ) -> Result<String> {
        let agent_id = self.resolve_agent_ref(agent_ref)?;
        let Some(caller) = caller_agent_id
            .map(str::trim)
            .filter(|caller| !caller.is_empty() && *caller != "root")
        else {
            return Ok(agent_id);
        };
        if caller == agent_id {
            return Err(anyhow!(
                "Refusing {action} on self (agent_id '{agent_id}'); child coordination authority is limited to strict descendants."
            ));
        }

        let mut cursor = agent_id.clone();
        let mut visited = std::collections::HashSet::new();
        while visited.insert(cursor.clone()) {
            let Some((_, record)) = self.worker_record_by_ref(&cursor) else {
                break;
            };
            let Some(parent_ref) = record
                .parent_run_id
                .as_deref()
                .or(record.spec.parent_run_id.as_deref())
            else {
                break;
            };
            if parent_ref == caller {
                return Ok(agent_id);
            }
            if parent_ref == "root" {
                break;
            }
            let Some((parent_id, _)) = self.worker_record_by_ref(parent_ref) else {
                break;
            };
            cursor = parent_id;
        }

        Err(anyhow!(
            "Refusing {action} from agent '{caller}' to '{agent_id}'; a child may control only its own descendants."
        ))
    }

    pub(super) fn ensure_caller_controls_descendant_for_session(
        &self,
        active_session_id: &str,
        agent_ref: &str,
        caller_agent_id: Option<&str>,
        action: &str,
    ) -> Result<String> {
        let agent_id = self.resolve_agent_ref_for_session(active_session_id, agent_ref)?;
        if let Some(caller) = caller_agent_id
            .map(str::trim)
            .filter(|caller| !caller.is_empty() && *caller != "root")
        {
            self.resolve_agent_ref_for_session(active_session_id, caller)?;
        }
        self.ensure_caller_controls_descendant(&agent_id, caller_agent_id, action)
    }

    /// List all agents and their status.
    #[must_use]
    /// Snapshot a single agent and tag it with the manager's
    /// classification. The bare `SubAgent::snapshot` defaults
    /// `from_prior_session` to `false`; only the manager knows the
    /// matching boot id, so listing goes through here.
    fn snapshot_for_listing(&self, agent: &SubAgent) -> SubAgentResult {
        let mut snap = agent.snapshot();
        snap.started_at = Some(agent.started_at);
        snap.from_prior_session = self.is_from_prior_session(agent);
        if let Some(record) = self.worker_records.get(&agent.id) {
            snap.worker_status = Some(record.status);
            snap.runtime_permissions = Some(crate::fleet::role::fleet_effective_permissions(
                &record.spec.agent_type,
                &record.spec.runtime_profile,
                None,
                None,
            ));
            snap.parent_run_id = record
                .parent_run_id
                .clone()
                .or_else(|| record.spec.parent_run_id.clone());
            snap.spawn_depth = record.spec.spawn_depth;
            snap.child_route = record.spec.child_route.clone();
        }
        snap
    }

    /// List all agents currently held by the manager, regardless of
    /// session origin. Use [`Self::list_filtered`] in user-facing tool
    /// paths so prior-session agents stay hidden by default (#405).
    #[cfg(test)]
    pub fn list(&self) -> Vec<SubAgentResult> {
        self.agents
            .values()
            .map(|agent| self.snapshot_for_listing(agent))
            .collect()
    }

    pub(crate) fn list_for_session(&self, active_session_id: &str) -> Vec<SubAgentResult> {
        self.agents
            .values()
            .filter(|agent| self.agent_is_owned_by_session(agent, active_session_id))
            .map(|agent| self.snapshot_for_listing(agent))
            .collect()
    }

    /// Legacy test-global projection for boot-origin filtering (#405).
    /// Production user/model surfaces call `list_filtered_for_session`, which
    /// first requires exact non-empty root-conversation ownership. Its
    /// `include_archived` option includes only that conversation's archived
    /// rows and never makes a foreign or ownerless record visible.
    #[cfg(test)]
    pub fn list_filtered(&self, include_archived: bool) -> Vec<SubAgentResult> {
        self.list_filtered_inner(None, include_archived)
    }

    pub(crate) fn list_filtered_for_session(
        &self,
        active_session_id: &str,
        include_archived: bool,
    ) -> Vec<SubAgentResult> {
        self.list_filtered_inner(Some(active_session_id), include_archived)
    }

    fn list_filtered_inner(
        &self,
        active_session_id: Option<&str>,
        include_archived: bool,
    ) -> Vec<SubAgentResult> {
        self.agents
            .values()
            .filter(|agent| {
                active_session_id
                    .is_none_or(|session_id| self.agent_is_owned_by_session(agent, session_id))
            })
            .filter(|agent| {
                if include_archived {
                    return true;
                }
                // Live roster: only actually running children (4a). In the
                // legacy unscoped projection, prior-boot Running stays visible
                // for recovery; the scoped production projection has already
                // excluded every foreign conversation above. This
                // excludes completed/failed/cancelled and children that
                // never started (no task_handle) or timed out — same root
                // as the phantom watch entry. Prior-session Running stays
                // visible for recovery even without a handle (persisted
                // without task). Current-session terminals stay visible
                // for result fetch; prior-session terminals hide by default.
                if agent.status == SubAgentStatus::Running {
                    if self.is_from_prior_session(agent) {
                        return true;
                    }
                    return agent.task_handle.is_some() && !self.running_heartbeat_timed_out(agent);
                }
                !self.is_from_prior_session(agent)
            })
            .map(|agent| self.snapshot_for_listing(agent))
            .collect()
    }

    /// Clean up stale running agents and completed agents older than the
    /// given duration. Returns the number of running agents auto-cancelled
    /// during this pass.
    pub fn cleanup(&mut self, max_age: Duration) -> usize {
        self.cleanup_inner(None, max_age)
    }

    pub(crate) fn cleanup_for_session(
        &mut self,
        active_session_id: &str,
        max_age: Duration,
    ) -> usize {
        self.cleanup_inner(Some(active_session_id), max_age)
    }

    fn cleanup_inner(&mut self, active_session_id: Option<&str>, max_age: Duration) -> usize {
        let before = self.agents.len();
        let before_workers = self.worker_records.len();
        let scoped_agent_owners = self
            .agents
            .iter()
            .filter(|(_, agent)| {
                active_session_id
                    .is_none_or(|session_id| self.agent_is_owned_by_session(agent, session_id))
            })
            .map(|(agent_id, agent)| (agent_id.clone(), agent.owner_session_id.clone()))
            .collect::<HashMap<_, _>>();
        let scoped_agent_ids = scoped_agent_owners.keys().cloned().collect::<HashSet<_>>();
        let scoped_worker_owners = self
            .worker_records
            .iter()
            .filter(|(_, record)| {
                active_session_id.is_none_or(|session_id| {
                    !session_id.is_empty() && record.owner_session_id == session_id
                })
            })
            .map(|(worker_id, record)| (worker_id.clone(), record.owner_session_id.clone()))
            .collect::<HashMap<_, _>>();
        let scoped_worker_ids = scoped_worker_owners.keys().cloned().collect::<HashSet<_>>();
        let mut transcript_candidates = scoped_agent_ids
            .iter()
            .chain(scoped_worker_ids.iter())
            .cloned()
            .collect::<Vec<_>>();
        transcript_candidates.sort();
        transcript_candidates.dedup();
        let mut auto_cancelled = 0;
        let timeout = self.running_heartbeat_timeout;
        let stale_agent_ids = self
            .agents
            .values()
            .filter(|agent| scoped_agent_ids.contains(&agent.id))
            .filter(|agent| {
                agent.status == SubAgentStatus::Running
                    && !agent.completion_claimed
                    && agent.last_activity_at.elapsed() >= timeout
            })
            .map(|agent| agent.id.clone())
            .collect::<Vec<_>>();
        for agent_id in stale_agent_ids {
            let orphan = self
                .agents
                .get(&agent_id)
                .is_some_and(|agent| agent.task_handle.is_none());
            if let Some(agent) = self.agents.get(&agent_id) {
                tracing::warn!(
                    target: "subagent",
                    agent_id = %agent.id,
                    timeout_secs = timeout.as_secs(),
                    orphan,
                    "auto-cancelling stale sub-agent with no manager-visible progress"
                );
            }
            let Some(mut terminal) = self.agents.get(&agent_id).map(SubAgent::snapshot) else {
                continue;
            };
            // Orphans have no live executor — Interrupted is more honest than
            // Cancelled (nothing was stopped; the process was already gone).
            if orphan {
                terminal.status = SubAgentStatus::Interrupted(
                    "No live executor; marked terminal locally after heartbeat timeout".to_string(),
                );
                terminal.result = Some(format!(
                    "Marked terminal after {}s with no live task handle (local-only; durable coordination may be owned elsewhere).",
                    timeout.as_secs()
                ));
            } else {
                terminal.status = SubAgentStatus::Cancelled;
                terminal.result = Some(format!(
                    "Auto-cancelled after {}s without sub-agent progress.",
                    timeout.as_secs()
                ));
            }
            terminal.needs_input = None;
            // Cleanup batches stale transitions and persists the final fleet
            // snapshot once below. Spawning one unordered background write
            // per child could let an earlier partial snapshot rename last and
            // resurrect a cancelled worker after restart.
            // persist_after_commit=true still best-efforts; lock loss only
            // skips disk — in-memory terminal state always commits.
            if self.finish_terminal_result(&agent_id, terminal, true, true) {
                auto_cancelled += 1;
            }
        }
        // Deliberately NOT here: a pass that terminalized every Running agent
        // without a live task handle whenever this process lacked the
        // coordination flock (#2.6). Two Codewhale sessions in one workspace is
        // ordinary usage, and failure to append to a shared bookkeeping ledger
        // is not evidence that live work has stopped. Liveness is decided by
        // the heartbeat above — which is actual evidence — never by who owns
        // the flock (owner report, 2026-08-04). See `docs/architecture/
        // delegated-coordination.md` for what lock loss legitimately costs.

        self.agents.retain(|agent_id, agent| {
            if active_session_id.is_some() && !scoped_agent_ids.contains(agent_id) {
                return true;
            }
            if agent.status == SubAgentStatus::Running {
                true
            } else {
                agent.started_at.elapsed() < max_age
            }
        });
        // #4217: age-evict terminal worker ledger entries. Agents already drop
        // after `max_age`, but worker_records previously only had an LRU cap of
        // 256 — long-lived sessions rewrote multi-MB subagents.v1.json forever.
        // Running / starting / waiting records are always preserved.
        let now_ms = epoch_millis_now();
        let max_age_ms = max_age.as_millis() as u64;
        self.worker_records.retain(|worker_id, record| {
            if active_session_id.is_some() && !scoped_worker_ids.contains(worker_id) {
                return true;
            }
            if !record.status.is_terminal() {
                return true;
            }
            let anchor_ms = record.completed_at_ms.unwrap_or(record.updated_at_ms);
            now_ms.saturating_sub(anchor_ms) < max_age_ms
        });
        // The transcript artifact follows the same retention lifecycle as the
        // worker ledger. Keep it while either the agent or worker record is
        // inspectable; once both age out, remove the one deterministic file so
        // long-lived workspaces do not accumulate silent transcript copies.
        // Also queue handle-store eviction for the same fully-retired agents
        // (#3885): handles pinned in memory per-agent are freed on the next
        // async drain by callers that hold the SharedHandleStore lock.
        for agent_id in transcript_candidates {
            if self.agents.contains_key(&agent_id) || self.worker_records.contains_key(&agent_id) {
                continue;
            }
            if let Err(err) = remove_subagent_transcript_artifact(&self.state_root, &agent_id) {
                tracing::warn!(
                    target: "subagent",
                    ?err,
                    agent_id,
                    "failed to remove expired sub-agent transcript artifact"
                );
            }
            let owner_session_id = scoped_agent_owners
                .get(&agent_id)
                .cloned()
                .or_else(|| scoped_worker_owners.get(&agent_id).cloned())
                .unwrap_or_default();
            self.pending_handle_evictions
                .push((agent_id, owner_session_id));
        }
        if self.agents.len() != before
            || auto_cancelled > 0
            || self.worker_records.len() != before_workers
        {
            self.persist_state_best_effort();
        }
        self.last_cleanup_at = Some(Instant::now());
        auto_cancelled
    }

    /// #3803: whether enough time has elapsed since the last `cleanup` that the
    /// next sidebar refresh should run the write-locked cleanup again. Every
    /// other refresh renders from the read-only `list()` snapshot, so a UI
    /// refresh storm during a fanout does not take the write lock per request.
    #[must_use]
    pub fn cleanup_due(&self, min_interval: Duration) -> bool {
        self.last_cleanup_at
            .is_none_or(|last| last.elapsed() >= min_interval)
    }

    #[must_use]
    pub(crate) fn drain_pending_handle_evictions_for_session(
        &mut self,
        active_session_id: &str,
    ) -> Vec<String> {
        let pending = std::mem::take(&mut self.pending_handle_evictions);
        let (owned, foreign): (Vec<_>, Vec<_>) = pending
            .into_iter()
            .partition(|(_, owner_session_id)| owner_session_id == active_session_id);
        self.pending_handle_evictions = foreign;
        owned.into_iter().map(|(agent_id, _)| agent_id).collect()
    }

    /// Claim terminal delivery if this task is still the running owner.
    ///
    /// The claim excludes cancellation while deliberately leaving the public
    /// status `Running`. `run_subagent_task` can therefore queue completion to
    /// the parent before the running-child gate closes (#1961). The winning
    /// finisher performs only non-awaiting channel sends while it owns the
    /// manager guard, then commits the terminal projections.
    fn claim_terminal_delivery(&mut self, agent_id: &str) -> bool {
        let Some(agent) = self.agents.get_mut(agent_id) else {
            return false;
        };
        if agent.status != SubAgentStatus::Running || agent.completion_claimed {
            return false;
        }
        agent.completion_claimed = true;
        true
    }

    /// Own, publish, and commit one terminal outcome.
    ///
    /// Claiming first makes natural completion, explicit Stop, coordination
    /// interrupt, and stale cleanup race on one bit. The winning path attempts
    /// every live fan-in send while both public projections still read
    /// Running, then commits the matching agent and worker terminal states.
    /// Late task output and repeated Stop calls cannot publish a second result.
    fn finish_terminal_result(
        &mut self,
        agent_id: &str,
        mut result: SubAgentResult,
        abort_task: bool,
        persist_after_commit: bool,
    ) -> bool {
        if result.status == SubAgentStatus::Running || result.agent_id != agent_id {
            return false;
        }
        if !self.claim_terminal_delivery(agent_id) {
            return false;
        }
        result.child_route = self
            .worker_records
            .get(agent_id)
            .and_then(|record| record.spec.child_route.clone());

        if abort_task
            && let Some(handle) = self
                .agents
                .get_mut(agent_id)
                .and_then(|agent| agent.task_handle.take())
        {
            handle.abort();
        }

        let delivery = self
            .agents
            .get(agent_id)
            .and_then(|agent| agent.terminal_delivery.clone());
        if let Some(delivery) = delivery {
            delivery.deliver(&result);
        }

        self.update_from_result_with_persist(agent_id, result, persist_after_commit)
    }

    /// Commit a claimed natural task result.
    ///
    /// Returns `true` only when the prior claim still owns the terminal
    /// transition. External notification is deliberately queued between
    /// [`Self::claim_terminal_delivery`] and this commit.
    #[cfg(test)]
    fn update_from_result(&mut self, agent_id: &str, result: SubAgentResult) -> bool {
        self.update_from_result_with_persist(agent_id, result, true)
    }

    fn update_from_result_with_persist(
        &mut self,
        agent_id: &str,
        result: SubAgentResult,
        persist_after_commit: bool,
    ) -> bool {
        let Some(agent) = self.agents.get_mut(agent_id) else {
            return false;
        };
        if agent.status != SubAgentStatus::Running || !agent.completion_claimed {
            return false;
        }
        agent.status = result.status.clone();
        agent.assignment = result.assignment.clone();
        agent.result = result.result.clone();
        agent.steps_taken = result.steps_taken;
        agent.checkpoint = result.checkpoint.clone();
        agent.needs_input = result.needs_input.clone();
        if result.status != SubAgentStatus::Running {
            agent.input_tx = None;
        }
        agent.completion_claimed = false;
        agent.task_handle = None;
        agent.terminal_delivery = None;
        release_resident_leases_for(agent_id);
        self.complete_worker_from_result(agent_id, &result);
        if persist_after_commit {
            self.persist_state_best_effort();
        }
        true
    }

    fn update_checkpoint(&mut self, agent_id: &str, checkpoint: SubAgentCheckpoint) -> bool {
        let Some(agent) = self.agents.get_mut(agent_id) else {
            return false;
        };
        agent.steps_taken = checkpoint.steps_taken;
        agent.checkpoint = Some(checkpoint);
        agent.last_activity_at = Instant::now();
        // #freeze: hot per-step path — coalesce the full-fleet persist so 20
        // agents stepping concurrently do not serialize the whole fleet (with
        // full transcripts) to disk under the write lock on every step.
        self.persist_state_debounced();
        true
    }
}

/// Thread-safe wrapper for `SubAgentManager`.
pub type SharedSubAgentManager = Arc<RwLock<SubAgentManager>>;

pub fn load_persisted_agent_worker_records(workspace: &Path) -> Result<Vec<AgentWorkerRecord>> {
    load_persisted_agent_worker_records_with_state_root(workspace, workspace)
}

/// Load persisted worker records from an explicit delegated-agent state root.
pub fn load_persisted_agent_worker_records_with_state_root(
    workspace: &Path,
    state_root: &Path,
) -> Result<Vec<AgentWorkerRecord>> {
    let mut manager =
        SubAgentManager::new_with_state_root(workspace.to_path_buf(), state_root.to_path_buf(), 1)
            .with_state_path(default_state_path(state_root)?);
    manager.load_state()?;
    Ok(manager.list_worker_records())
}

/// Model-facing session projection returned by the v0.8.33 sub-agent API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentSessionProjection {
    pub name: String,
    pub agent_id: String,
    #[serde(default)]
    pub run_id: String,
    pub status: String,
    pub terminal: bool,
    pub context_mode: String,
    pub fork_context: bool,
    pub prefix_cache: SubAgentPrefixCacheProjection,
    pub transcript_handle: VarHandle,
    #[serde(default = "default_agent_run_follow_up")]
    pub follow_up: AgentRunFollowUpTarget,
    #[serde(default = "default_agent_run_takeover")]
    pub takeover: AgentRunTakeoverTarget,
    #[serde(default)]
    pub artifacts: Vec<AgentRunArtifactRef>,
    #[serde(default = "default_agent_run_usage")]
    pub usage: AgentRunUsage,
    #[serde(default = "default_agent_run_verification")]
    pub verification: AgentRunVerificationSummary,
    pub snapshot: SubAgentResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<SubAgentCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_input: Option<SubAgentNeedsInput>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub continuable: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub needs_continuation: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub timed_out: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub timed_out_with_checkpoint: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_record: Option<AgentWorkerRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fleet_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_route: Option<ChildRouteReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentPrefixCacheProjection {
    pub mode: String,
    pub parent_prefix: String,
    pub deepseek_prefix_cache_reuse: String,
}

fn subagent_prefix_cache_projection(snapshot: &SubAgentResult) -> SubAgentPrefixCacheProjection {
    if snapshot.fork_context {
        SubAgentPrefixCacheProjection {
            mode: "forked".to_string(),
            parent_prefix: "preserved_byte_identical_when_available".to_string(),
            deepseek_prefix_cache_reuse: "optimized_for_existing_parent_prefill".to_string(),
        }
    } else {
        SubAgentPrefixCacheProjection {
            mode: "fresh".to_string(),
            parent_prefix: "not_inherited".to_string(),
            deepseek_prefix_cache_reuse: "independent_child_prefill".to_string(),
        }
    }
}

fn subagent_checkpoint_is_continuable(snapshot: &SubAgentResult) -> bool {
    matches!(snapshot.status, SubAgentStatus::Interrupted(_))
        && snapshot
            .checkpoint
            .as_ref()
            .is_some_and(|checkpoint| checkpoint.continuable && !checkpoint.messages.is_empty())
}

async fn subagent_session_projection(
    snapshot: SubAgentResult,
    timed_out: bool,
    context: &ToolContext,
    worker_record: Option<AgentWorkerRecord>,
) -> SubAgentSessionProjection {
    let transcript_session_id = format!("agent:{}", snapshot.agent_id);
    let continuable = subagent_checkpoint_is_continuable(&snapshot);
    let transcript_payload = json!({
        "kind": "subagent_session_snapshot",
        "agent_id": snapshot.agent_id.clone(),
        "name": snapshot.name.clone(),
        "status": subagent_status_name(&snapshot.status),
        "context_mode": snapshot.context_mode.clone(),
        "fork_context": snapshot.fork_context,
        "result": snapshot.result.clone(),
        "steps_taken": snapshot.steps_taken,
        "duration_ms": snapshot.duration_ms,
        "assignment": snapshot.assignment.clone(),
        "checkpoint": snapshot.checkpoint.clone(),
        "needs_input": snapshot.needs_input.clone(),
        "needs_continuation": continuable,
        "timed_out_with_checkpoint": timed_out && continuable,
        "snapshot": snapshot.clone(),
    });
    let transcript_handle = {
        let mut store = context.runtime.handle_store.lock().await;
        let full_transcript_lookup = VarHandle {
            kind: "var_handle".to_string(),
            session_id: transcript_session_id.clone(),
            name: "full_transcript".to_string(),
            type_name: String::new(),
            length: 0,
            repr_preview: String::new(),
            sha256: String::new(),
        };
        if snapshot.status != SubAgentStatus::Running
            && let Some(record) = store.get(&full_transcript_lookup)
        {
            record.handle.clone()
        } else {
            store.insert_json(transcript_session_id, "transcript", transcript_payload)
        }
    };
    let run_id = worker_record
        .as_ref()
        .map(|record| agent_worker_run_id(&record.spec))
        .unwrap_or_else(|| snapshot.agent_id.clone());
    let follow_up = worker_record
        .as_ref()
        .map(|record| record.follow_up.clone())
        .unwrap_or_else(|| AgentRunFollowUpTarget {
            tool: default_agent_inspect_tool(),
            agent_id: snapshot.agent_id.clone(),
            session_name: Some(snapshot.name.clone()),
            accepted_statuses: vec!["running".to_string(), "interrupted_continuable".to_string()],
            latest_delivery: None,
        });
    let takeover = worker_record
        .as_ref()
        .map(|record| record.takeover.clone())
        .unwrap_or_else(|| AgentRunTakeoverTarget {
            kind: default_subagent_takeover_kind(),
            supported: true,
            agent_id: snapshot.agent_id.clone(),
            session_name: Some(snapshot.name.clone()),
            instructions: format!(
                "Inspect agent '{}' through the returned transcript_handle with handle_read; open a replacement with agent if the lane no longer fits.",
                snapshot.agent_id
            ),
            unsupported_reason: None,
        });
    let artifacts = worker_record
        .as_ref()
        .map(|record| record.artifacts.clone())
        .unwrap_or_else(|| default_subagent_artifacts(&run_id));
    let usage = worker_record
        .as_ref()
        .map(|record| record.usage.clone())
        .unwrap_or_else(default_agent_run_usage);
    let verification = worker_record
        .as_ref()
        .map(|record| record.verification.clone())
        .unwrap_or_else(default_agent_run_verification);
    // Status must stay coherent with the continuation flags below. An
    // Interrupted snapshot that carries a continuable checkpoint
    // (`continuable`/`needs_continuation` true, `terminal` true) means the
    // worker is parked waiting for the parent to act, so it must project as
    // `waiting_for_user` rather than a bare `interrupted`. When a worker
    // record exists its status was already derived via
    // `worker_status_from_subagent_result`; mirror that derivation when there
    // is no record so both paths agree on the "needs parent action" signal.
    let status = worker_record
        .as_ref()
        .map(|record| agent_worker_status_name(record.status))
        .unwrap_or_else(|| agent_worker_status_name(worker_status_from_subagent_result(&snapshot)))
        .to_string();

    SubAgentSessionProjection {
        name: snapshot.name.clone(),
        agent_id: snapshot.agent_id.clone(),
        run_id,
        status,
        terminal: snapshot.status != SubAgentStatus::Running,
        context_mode: snapshot.context_mode.clone(),
        fork_context: snapshot.fork_context,
        prefix_cache: subagent_prefix_cache_projection(&snapshot),
        transcript_handle,
        follow_up,
        takeover,
        artifacts,
        usage,
        verification,
        checkpoint: snapshot.checkpoint.clone(),
        needs_input: snapshot.needs_input.clone(),
        continuable: subagent_checkpoint_is_continuable(&snapshot),
        needs_continuation: continuable,
        snapshot,
        timed_out,
        timed_out_with_checkpoint: timed_out && continuable,
        fleet_profile: worker_record
            .as_ref()
            .and_then(|record| record.spec.child_route.as_ref())
            .and_then(|receipt| receipt.resolved_profile_id.clone()),
        child_route: worker_record
            .as_ref()
            .and_then(|record| record.spec.child_route.clone()),
        worker_record,
    }
}

/// Append-only, per-run backing store for the worker's complete structured
/// message stream. The in-memory `full_transcript` handle deliberately keeps a
/// bounded tail; this artifact is the durable source used by the TUI's Open
/// action when the conversation is larger than that tail.
struct SubAgentTranscriptArtifactWriter {
    state_root: PathBuf,
    path: PathBuf,
    relative_path: PathBuf,
    persisted_messages: usize,
}

impl SubAgentTranscriptArtifactWriter {
    async fn for_runtime(runtime: &SubAgentRuntime, agent_id: &str) -> Result<Self> {
        let state_root = runtime.manager.read().await.state_root.clone();
        Self::create(&state_root, agent_id)
    }

    fn create(state_root: &Path, agent_id: &str) -> Result<Self> {
        let state_root = normalize_subagent_workspace(state_root);
        let relative_path = subagent_transcript_artifact_relative_path(agent_id);
        let path = checked_subagent_transcript_artifact_path(&state_root, agent_id)?;
        let header = json!({
            "kind": "subagent_transcript_header",
            "schema_version": SUBAGENT_TRANSCRIPT_ARTIFACT_SCHEMA_VERSION,
            "agent_id": agent_id,
        });
        create_private_subagent_transcript(&state_root, &path, &json_line(&header)?)?;
        Ok(Self {
            state_root,
            path,
            relative_path,
            persisted_messages: 0,
        })
    }

    fn sync_messages(&mut self, messages: &[Message], durable: bool) -> Result<()> {
        if messages.len() < self.persisted_messages {
            return Err(anyhow!(
                "sub-agent transcript history shrank from {} to {} messages",
                self.persisted_messages,
                messages.len()
            ));
        }

        let mut encoded = Vec::new();
        for (index, message) in messages.iter().enumerate().skip(self.persisted_messages) {
            encoded.extend(json_line(&json!({
                "kind": "message",
                "index": index,
                "message": message,
            }))?);
        }

        if !encoded.is_empty() || durable {
            append_private_subagent_transcript(&self.state_root, &self.path, &encoded, durable)?;
        }
        self.persisted_messages = messages.len();
        Ok(())
    }

    fn metadata(&self, complete: bool) -> Value {
        json!({
            "kind": "subagent_transcript_jsonl",
            "schema_version": SUBAGENT_TRANSCRIPT_ARTIFACT_SCHEMA_VERSION,
            "relative_path": self.relative_path,
            "persisted_messages": self.persisted_messages,
            "complete": complete,
            "contains_session_content": true,
        })
    }
}

fn json_line(value: &Value) -> Result<Vec<u8>> {
    let mut encoded = serde_json::to_vec(value)?;
    encoded.push(b'\n');
    Ok(encoded)
}

fn subagent_transcript_artifact_relative_path(agent_id: &str) -> PathBuf {
    let digest = crate::hashing::sha256_hex(agent_id.as_bytes());
    Path::new(".codewhale")
        .join("state")
        .join(SUBAGENT_TRANSCRIPT_ARTIFACT_DIR)
        .join(format!("{digest}.jsonl"))
}

fn checked_subagent_transcript_artifact_path(state_root: &Path, agent_id: &str) -> Result<PathBuf> {
    checked_subagent_state_path(
        state_root,
        &subagent_transcript_artifact_relative_path(agent_id),
    )
}

/// Read the complete structured worker chat for the TUI Open action. The path
/// is derived from `agent_id` rather than accepted from handle JSON, so a
/// corrupted or model-supplied payload cannot redirect the reader outside the
/// manager state root.
pub fn load_subagent_transcript_artifact(
    state_root: &Path,
    agent_id: &str,
) -> Result<Vec<Message>> {
    let state_root = normalize_subagent_workspace(state_root);
    let path = checked_subagent_transcript_artifact_path(&state_root, agent_id)?;
    let raw = read_subagent_state_file(&state_root, &path)?;
    let mut lines = raw.lines();
    let header_line = lines
        .next()
        .ok_or_else(|| anyhow!("sub-agent transcript artifact is empty"))?;
    let header: Value = serde_json::from_str(header_line)?;
    if header.get("kind").and_then(Value::as_str) != Some("subagent_transcript_header")
        || header.get("schema_version").and_then(Value::as_u64)
            != Some(u64::from(SUBAGENT_TRANSCRIPT_ARTIFACT_SCHEMA_VERSION))
        || header.get("agent_id").and_then(Value::as_str) != Some(agent_id)
    {
        return Err(anyhow!(
            "sub-agent transcript artifact header does not match agent {agent_id}"
        ));
    }

    let mut messages = Vec::new();
    for line in lines.filter(|line| !line.trim().is_empty()) {
        let record: Value = serde_json::from_str(line)?;
        if record.get("kind").and_then(Value::as_str) != Some("message") {
            return Err(anyhow!("unknown sub-agent transcript artifact record"));
        }
        let index = record
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| anyhow!("sub-agent transcript message is missing its index"))?;
        if index != messages.len() {
            return Err(anyhow!(
                "sub-agent transcript message index {index} does not follow {}",
                messages.len()
            ));
        }
        let message = serde_json::from_value::<Message>(
            record
                .get("message")
                .cloned()
                .ok_or_else(|| anyhow!("sub-agent transcript record is missing its message"))?,
        )?;
        messages.push(message);
    }
    Ok(messages)
}

fn remove_subagent_transcript_artifact(state_root: &Path, agent_id: &str) -> Result<bool> {
    let state_root = normalize_subagent_workspace(state_root);
    let path = checked_subagent_transcript_artifact_path(&state_root, agent_id)?;
    reject_root_relative_symlinks(&state_root, &path)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(anyhow!(
            "sub-agent transcript artifact is not a regular file: {}",
            path.display()
        ));
    }
    fs::remove_file(path)?;
    Ok(true)
}

#[cfg(test)]
pub(crate) fn write_subagent_transcript_artifact_for_test(
    state_root: &Path,
    agent_id: &str,
    messages: &[Message],
) -> Result<PathBuf> {
    let mut writer = SubAgentTranscriptArtifactWriter::create(state_root, agent_id)?;
    writer.sync_messages(messages, true)?;
    Ok(writer.path)
}

fn default_state_path(state_root: &Path) -> Result<PathBuf> {
    let state_root = normalize_subagent_workspace(state_root);
    // Canonical post-rebrand state path. On first run the file won't exist yet;
    // write_json_atomic creates parent directories. Legacy .deepseek/state/ data
    // is migrated on load (see load_state).
    checked_subagent_state_path(
        &state_root,
        &Path::new(".codewhale")
            .join("state")
            .join(SUBAGENT_STATE_FILE),
    )
}

fn checked_subagent_state_path(state_root: &Path, path: &Path) -> Result<PathBuf> {
    let state_root = normalize_subagent_workspace(state_root);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        state_root.join(path)
    };
    let file_name = absolute
        .file_name()
        .ok_or_else(|| anyhow!("sub-agent state path must include a file name"))?;
    let parent = absolute
        .parent()
        .ok_or_else(|| anyhow!("sub-agent state path must include a parent directory"))?;
    let parent = match parent.canonicalize() {
        Ok(parent) => parent,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => normalize_path_components(parent),
        Err(err) => return Err(err.into()),
    };
    let state_path = parent.join(file_name);
    if !state_path.starts_with(&state_root) {
        return Err(anyhow!(
            "sub-agent state path must stay within state root: {}",
            state_path.display()
        ));
    }
    reject_root_relative_symlinks(&state_root, &state_path)?;
    Ok(state_path)
}

fn normalize_subagent_workspace(workspace: &Path) -> PathBuf {
    if let Ok(canonical) = workspace.canonicalize() {
        return canonical;
    }
    let absolute = if workspace.is_absolute() {
        workspace.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(workspace)
    };
    normalize_path_components(&absolute)
}

fn normalize_path_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn reject_root_relative_symlinks(root: &Path, path: &Path) -> Result<()> {
    let relative = path.strip_prefix(root).map_err(|_| {
        anyhow!(
            "sub-agent state path must stay within state root: {}",
            path.display()
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        let Ok(metadata) = fs::symlink_metadata(&current) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            return Err(anyhow!(
                "sub-agent state path must not traverse symlinks: {}",
                current.display()
            ));
        }
    }
    Ok(())
}

fn read_subagent_state_file(state_root: &Path, path: &Path) -> Result<String> {
    let state_root = normalize_subagent_workspace(state_root);
    reject_root_relative_symlinks(&state_root, path)?;
    let metadata = fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() || !file_type.is_file() {
        return Err(anyhow!(
            "sub-agent state path must be a regular file: {}",
            path.display()
        ));
    }

    let mut file = open_subagent_state_file(path)?;
    let mut raw = String::new();
    file.read_to_string(&mut raw)?;
    Ok(raw)
}

#[cfg(unix)]
fn open_subagent_state_file(path: &Path) -> Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(Into::into)
}

#[cfg(not(unix))]
fn open_subagent_state_file(path: &Path) -> Result<fs::File> {
    fs::File::open(path).map_err(Into::into)
}

fn prepare_subagent_transcript_parent(state_root: &Path, path: &Path) -> Result<()> {
    reject_root_relative_symlinks(state_root, path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("sub-agent transcript artifact must have a parent directory"))?;
    fs::create_dir_all(parent)?;
    // Re-check after creation so a pre-existing component cannot redirect the
    // private transcript outside the state root.
    reject_root_relative_symlinks(state_root, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn create_private_subagent_transcript(state_root: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    prepare_subagent_transcript_parent(state_root, path)?;
    let mut file = open_private_subagent_transcript(path, false)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn append_private_subagent_transcript(
    state_root: &Path,
    path: &Path,
    bytes: &[u8],
    durable: bool,
) -> Result<()> {
    reject_root_relative_symlinks(state_root, path)?;
    let mut file = open_private_subagent_transcript(path, true)?;
    if !bytes.is_empty() {
        file.write_all(bytes)?;
    }
    if durable {
        file.sync_all()?;
    }
    Ok(())
}

#[cfg(unix)]
fn open_private_subagent_transcript(path: &Path, append: bool) -> Result<fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut options = fs::OpenOptions::new();
    options
        .write(true)
        .append(append)
        .create(!append)
        .truncate(!append)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600);
    let file = options.open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[cfg(not(unix))]
fn open_private_subagent_transcript(path: &Path, append: bool) -> Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .append(append)
        .create(!append)
        .truncate(!append)
        .open(path)
        .map_err(Into::into)
}

pub(crate) fn epoch_millis_now() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        Err(_) => 0,
    }
}

/// Compact preview for follow-up delivery receipts (sibling coord surface).
fn truncate_preview(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn instant_from_duration(duration: Duration) -> Instant {
    Instant::now()
        .checked_sub(duration)
        .unwrap_or_else(Instant::now)
}

/// Per-write sequence so each `write_json_atomic` uses a distinct temp file.
/// `persist_state_best_effort` fires a fresh thread per call, so multiple
/// persists of the same `state.json` can be in flight at once; keying the temp
/// name only on the pid (as before) made every thread write the *same*
/// `state.<pid>.tmp` and a rename could publish a half-written file — corrupt
/// state that fails to parse on reload.
static WRITE_JSON_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static STATE_PUBLISH_SEQUENCES: std::sync::OnceLock<parking_lot::Mutex<HashMap<PathBuf, u64>>> =
    std::sync::OnceLock::new();

fn write_json_atomic(state_root: &Path, path: &Path, value: &PersistedSubAgentState) -> Result<()> {
    let state_root = normalize_subagent_workspace(state_root);
    reject_root_relative_symlinks(&state_root, path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_string_pretty(value)?;
    let seq = WRITE_JSON_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp_path = path.with_extension(format!("{}.{seq}.tmp", std::process::id()));
    reject_root_relative_symlinks(&state_root, &tmp_path)?;
    fs::write(&tmp_path, payload)?;
    let publish_sequences =
        STATE_PUBLISH_SEQUENCES.get_or_init(|| parking_lot::Mutex::new(HashMap::new()));
    let mut published = publish_sequences.lock();
    if published
        .get(path)
        .is_some_and(|sequence| *sequence > value.snapshot_sequence)
    {
        let _ = fs::remove_file(&tmp_path);
        return Ok(());
    }
    if let Err(err) = fs::rename(&tmp_path, path) {
        // Don't leave a stray temp behind if the publish failed.
        let _ = fs::remove_file(&tmp_path);
        return Err(err.into());
    }
    published.insert(path.to_path_buf(), value.snapshot_sequence);
    Ok(())
}

/// Create a shared sub-agent manager with a configurable limit.
#[cfg(test)]
#[must_use]
pub fn new_shared_subagent_manager(workspace: PathBuf, max_agents: usize) -> SharedSubAgentManager {
    new_shared_subagent_manager_with_timeout(
        workspace,
        max_agents,
        max_agents,
        Duration::from_secs(crate::config::DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS),
        max_agents,
        None,
    )
}

/// Create a shared sub-agent manager with configurable concurrency and stale
/// running-agent heartbeat timeout.
#[must_use]
pub fn new_shared_subagent_manager_with_timeout(
    workspace: PathBuf,
    max_agents: usize,
    max_admitted_agents: usize,
    running_heartbeat_timeout: Duration,
    launch_concurrency: usize,
    default_token_budget: Option<u64>,
) -> SharedSubAgentManager {
    let state_root = workspace.clone();
    new_shared_subagent_manager_with_state_root_and_timeout(
        workspace,
        state_root,
        max_agents,
        max_admitted_agents,
        running_heartbeat_timeout,
        launch_concurrency,
        default_token_budget,
        // `[subagents]` budget defaults are an interactive `agent`-tool
        // concern; this control-plane path keeps role/spec defaults.
        None,
        None,
    )
}

/// Create a shared sub-agent manager with an explicit control-plane state root.
///
/// `workspace` remains the child execution and file-authority root. The worker
/// ledger, complete transcript artifacts and coordination lock live under
/// `state_root/.codewhale/state`. Distinct state roots intentionally do not
/// share write claims; an embedding host that runs them against the same
/// workspace must provide any required cross-session write coordination.
#[allow(clippy::too_many_arguments)] // legacy open constructor; budget pair rides along
#[must_use]
pub fn new_shared_subagent_manager_with_state_root_and_timeout(
    workspace: PathBuf,
    state_root: PathBuf,
    max_agents: usize,
    max_admitted_agents: usize,
    running_heartbeat_timeout: Duration,
    launch_concurrency: usize,
    default_token_budget: Option<u64>,
    default_max_steps: Option<u32>,
    default_wall_time: Option<Duration>,
) -> SharedSubAgentManager {
    let max_agents = max_agents.clamp(1, MAX_SUBAGENTS);
    let state_path = match default_state_path(&state_root) {
        Ok(path) => Some(path),
        Err(err) => {
            tracing::warn!(target: "subagent", ?err, "failed to resolve sub-agent state path");
            None
        }
    };
    let manager = if state_root == workspace {
        SubAgentManager::new(workspace, max_agents)
    } else {
        SubAgentManager::new_with_state_root(workspace, state_root, max_agents)
    };
    let mut manager = manager
        .with_admission_limit(max_admitted_agents)
        .with_running_heartbeat_timeout(running_heartbeat_timeout)
        .with_launch_concurrency(launch_concurrency)
        .with_default_token_budget(default_token_budget)
        .with_default_max_steps(default_max_steps)
        .with_default_wall_time(default_wall_time);
    if let Some(state_path) = state_path {
        manager = manager.with_state_path(state_path);
    }
    manager = manager.require_coordination_process_lock();
    if let Err(error) = manager.ensure_coordination_process_lock() {
        tracing::warn!(target: "subagent", %error, "this session cannot append to the shared coordination ledger");
    }
    // Loading is a read. It was previously gated behind the *write* flock,
    // which left a second session in the same workspace blind to every
    // decision and write claim already recorded — and holding a default,
    // empty ledger that would be written straight over the real one the
    // moment the first session exited and the flock became acquirable
    // (owner report, 2026-08-04). Read unconditionally; the write gate on
    // `persist_state`/`persist_state_synchronously` is what keeps the file
    // single-writer.
    if let Err(err) = manager.load_state() {
        // Routed through tracing instead of stderr — see comment in
        // `persist_state_best_effort` above.
        tracing::warn!(target: "subagent", ?err, "failed to load sub-agent state");
    }
    Arc::new(RwLock::new(manager))
}

// === Tool Implementations ===

/// Start a child agent task through a single simplified model-facing surface.
pub struct AgentTool {
    manager: SharedSubAgentManager,
    runtime: SubAgentRuntime,
    /// Last projection fingerprint per agent, used to throttle repeat
    /// peek/status calls that observe no change (#4097). Std mutex: locked
    /// only for brief map reads/writes, never across an await.
    inspect_memo: Arc<std::sync::Mutex<HashMap<String, PeekMemo>>>,
}

/// Fingerprint of the last peek/status response for one agent (#4097).
#[derive(Debug, Clone, Copy)]
struct PeekMemo {
    fingerprint: u64,
    at: Instant,
}

impl AgentTool {
    #[must_use]
    pub fn new(manager: SharedSubAgentManager, runtime: SubAgentRuntime) -> Self {
        Self {
            manager,
            runtime,
            inspect_memo: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentToolAction {
    Start,
    Roster,
    Status,
    Peek,
    Message,
    Followup,
    Interrupt,
    Wait,
    Cancel,
    /// Expand this caller's write claim before mutating outside it (#5462).
    ///
    /// The one coordination action that had no equivalent on `agent`: write
    /// scope could only be widened through `agents/coordinate action=claim`,
    /// so retiring that tool from the catalog without this action would have
    /// left fail-closed write enforcement with no in-band way to satisfy it.
    Claim,
}

fn parse_agent_tool_action(input: &Value) -> Result<AgentToolAction, ToolError> {
    let Some(action) = optional_input_str(input, &["action", "op"])? else {
        return Ok(AgentToolAction::Start);
    };
    match action.trim().to_ascii_lowercase().as_str() {
        "" | "start" | "spawn" | "run" => Ok(AgentToolAction::Start),
        "roster" | "members" | "profiles" => Ok(AgentToolAction::Roster),
        "status" | "list" | "inspect" => Ok(AgentToolAction::Status),
        "peek" | "progress" => Ok(AgentToolAction::Peek),
        "message" | "queue_message" => Ok(AgentToolAction::Message),
        "followup" | "follow_up" | "steer" => Ok(AgentToolAction::Followup),
        "interrupt" | "pause" => Ok(AgentToolAction::Interrupt),
        "wait" | "join" | "await" | "block" => Ok(AgentToolAction::Wait),
        "cancel" | "stop" | "abort" => Ok(AgentToolAction::Cancel),
        "claim" => Ok(AgentToolAction::Claim),
        other => Err(ToolError::invalid_input(format!(
            "Invalid agent action '{other}'. Use start, roster, status, peek, message, followup, interrupt, wait, claim, or cancel."
        ))),
    }
}

/// Translate `agent{action:"claim", ...}` into the `agents/coordinate` wire.
///
/// Two things this function exists to get right, both of which fail *silently*
/// when they are wrong:
///
/// 1. The coordinate wire key is `roots`, not `write_roots`
///    ([`AgentsCoordinateTool::execute`] reads `roots`). A translation that
///    forwarded `write_roots` would hand `expand_write_claim` three empty
///    lists, which returns the unchanged claim with `Ok` — a successful
///    receipt for an expansion that never happened, and then a fail-closed
///    write refusal the model cannot explain.
/// 2. An all-empty claim is refused here. `expand_write_claim` treats it as a
///    no-op success for the same reason, so without this check a call that
///    forgot its scope would read as "claim granted".
///
/// The three field names are the ones `agent action=start` already uses for
/// write scope (`write_roots` advertised; `exact_files` and
/// `coordination_contracts` parse-accepted), so one vocabulary describes a
/// child's scope whether it is declared at launch or widened later.
fn agent_claim_coordinate_input(input: &Value) -> Result<Value, ToolError> {
    let roots = parse_coordination_paths(input, "write_roots")?;
    let exact_files = parse_coordination_paths(input, "exact_files")?;
    let contracts = parse_bounded_strings(input, "coordination_contracts", 16)?;
    if roots.is_empty() && exact_files.is_empty() && contracts.is_empty() {
        return Err(ToolError::invalid_input(
            "agent action=claim needs at least one scope entry: write_roots, exact_files, or coordination_contracts. An empty claim would report success while expanding nothing."
                .to_string(),
        ));
    }
    Ok(json!({
        "action": "claim",
        "roots": roots,
        "exact_files": exact_files,
        "contracts": contracts,
    }))
}

fn parse_agent_ref(input: &Value) -> Result<Option<String>, ToolError> {
    Ok(
        optional_input_str(input, &["agent_id", "id", "session_name", "name"])?
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    )
}

/// #5186: whether an `agent action=start` call asks for exactly a canonical
/// read-only Fleet role and nothing that could widen it. Those spawns run
/// without an approval modal in the default posture because the child's own
/// posture gates (`role_posture_permits`, `SubAgentToolRegistry`) enforce
/// read-only behavior from the inside.
///
/// Anything this parser cannot prove read-only stays gated: no role token
/// (defaults to `worker`), an unparseable role, a `profile`
/// reference, a conflicting type/role pair, or an explicit write authority.
fn start_requests_read_only_role(input: &Value) -> bool {
    // A parameter this function cannot even read is not proof of anything,
    // so a type error fails closed into the approval modal. `execute` then
    // refuses the call outright with the named-parameter error.
    let read = |keys: &[&str]| optional_input_str(input, keys).map(|v| v.map(str::to_string));
    let Ok(profile) = read(&["profile", "fleet_profile", "roster_profile"]) else {
        return false;
    };
    if profile.is_some() {
        return false;
    }
    let Ok(type_input) = read(&["type", "agent_type", "agent_name"]) else {
        return false;
    };
    let Ok(role_input) = read(&["role", "agent_role"]) else {
        return false;
    };
    let type_input = type_input.as_deref();
    let role_input = role_input.as_deref();
    let parsed_type = type_input.and_then(FleetRole::from_str);
    let parsed_role = role_input.and_then(FleetRole::from_str);
    let role = match (parsed_type, parsed_role) {
        (Some(from_type), Some(from_role)) if from_type == from_role => from_type,
        // A second token that does not parse as a canonical role may be a
        // role name resolved later — fail closed like `profile`.
        (Some(from_type), None) if role_input.is_none() => from_type,
        (None, Some(from_role)) if type_input.is_none() => from_role,
        _ => return false,
    };
    let Ok(write_authority) = read(&["write_authority", "writeAuthority"]) else {
        return false;
    };
    if write_authority.is_some_and(|authority| !authority.trim().eq_ignore_ascii_case("read_only"))
    {
        return false;
    }
    matches!(
        role,
        FleetRole::Scout
            | FleetRole::Planner
            | FleetRole::Reviewer
            | FleetRole::Verifier
            | FleetRole::Consultant
    )
}

#[async_trait]
impl ToolSpec for AgentTool {
    fn name(&self) -> &'static str {
        "agent"
    }

    fn description(&self) -> &'static str {
        concat!(
            "Start with action=start and prompt; returns a turn-owned agent_id immediately. Read-only roles need no extra fields. Set detached=true only for work that must remain independently observable after the turn. ",
            "Use multiple starts for independent parallel tasks. ",
            "type selects the Fleet role: worker (full tool access), scout (fast read-only exploration), planner (grounded strategy, read-only probes), reviewer (reads and grades code), builder (lands focused code changes), verifier (runs tests and reports evidence), consultant (read-only design counsel), or custom (allowed_tools on the parent's posture). ",
            "profile runs the child as a named Fleet role — pass a profile only when the task needs a different role than type selects. Without a profile the child inherits the parent's model; per-call model or thinking overrides are not part of this surface. ",
            "Use action=roster to inspect the Fleet roles and their descriptions before choosing a type or profile. ",
            "Child run budgets (model turns, wall time) come from Fleet role defaults and operator [subagents] config, not per-call fields. ",
            "worktree=true gives the child an isolated git worktree — use it whenever parallel writers must not collide with the parent checkout. ",
            "A write-capable child defaults write scope to the parent workspace; narrow it with write_roots (repo-relative directory trees) so parallel children claim disjoint scope. ",
            "Prefer type=builder for write work and type=verifier (or the Run tool with action=\"verifiers\") after writes settle — dispatch is not completion. ",
            "Coordinate through this same tool: action=message queues a note without waking the child; action=followup delivers queued notes and wakes a running child for its next user-provenance turn; action=interrupt stops the current child turn while preserving its checkpoint; action=wait blocks without changing child state, and until=\"all\" joins a whole fan-out in one call. ",
            "action=claim widens your own enforced write scope: pass write_roots (and optionally exact_files, coordination_contracts) before mutating anything a fail-closed write refusal named. It records a durable claim receipt and fails on contention with a peer claim; it never touches another agent's scope. ",
            "Action contract: start requires prompt; message/followup require a target and message; peek/interrupt/cancel require a target; claim requires at least one scope entry; roster, status, and wait are unscoped. ",
            "This is the whole model-facing sub-agent surface; there is no second transport. ",
            "In Operate, use detached=true only for independent or long work that must outlive the active turn; a write-capable root start defaults write scope to the parent workspace unless narrowed with write_roots; arbitrary shell remains gated. ",
            "Legacy action=status|peek|cancel remain for compatibility."
        )
    }

    /// Advertised `agent` schema: exactly 12 fields (#5324, #5123) —
    /// action, prompt, type, profile, name, agent_id, message, until,
    /// detached, worktree, write_roots, resume_from — plus the
    /// action-discriminated `dependentSchemas` tree. Every field removed
    /// from this schema (budgets, model/thinking overrides, worktree-path
    /// knobs, deliberate/spawn-contract knobs, wait/status extras) stays
    /// parse-accepted unchanged for saved transcripts, ACP/MCP clients and
    /// Fleet configs, exactly like `token_budget`; see docs/SUBAGENTS.md.
    fn input_schema(&self) -> Value {
        let target_required = json!([
            {
                "properties": {"agent_id": {}},
                "required": ["agent_id"]
            },
            {
                "properties": {"name": {}},
                "required": ["name"]
            }
        ]);
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["start", "roster", "status", "peek", "message", "followup", "interrupt", "wait", "claim", "cancel"],
                    "description": "start launches a turn-owned worker and returns immediately. roster lists the Fleet roles and their descriptions. status/peek inspect running or retained workers. message queues a note without waking a running child. followup delivers queued notes and wakes a running child for its next user-provenance model turn. interrupt stops the current turn while preserving the child checkpoint. wait only observes; see until. claim widens your own enforced write scope (see write_roots). cancel permanently cancels a running child."
                },
                "until": {
                    "type": "string",
                    "enum": ["completion", "all", "activity"],
                    "description": "For action=wait. completion (default) returns when any one child settles. all returns only once every child running at call time has settled, with each outcome — the fan-out join: start the batch, make one wait, then synthesize. activity also returns on progress."
                },
                "agent_id": {
                    "type": "string",
                    "description": "Agent id or session name for any action except start and unscoped status/wait."
                },
                "message": {
                    "type": "string",
                    "description": "Parent note for action=message or action=followup. message queues only; followup also wakes a running child."
                },
                "name": {
                    "type": "string",
                    "description": "For action=start, optional stable session name. For status/peek/cancel, accepted as an alias for agent_id."
                },
                "prompt": {
                    "type": "string",
                    "description": "The focused task to give the worker. A read-only role needs no write scope; a write-capable role defaults to the parent workspace unless narrowed with write_roots."
                },
                "detached": {
                    "type": "boolean",
                    "description": "False (default): the turn owns this child's non-detached subtree; the parent gets one chance to join before resumable parking. true: detached work outlives the turn; inspect or cancel it with agent(action=cancel)."
                },
                "type": {
                    "type": "string",
                    "enum": FLEET_ROLE_SCHEMA_VALUES,
                    "description": SUBAGENT_TYPE_DESCRIPTION
                },
                "profile": {
                    "type": "string",
                    "description": "Optional Fleet role selector. Use a role name (action=roster lists the roles); unknown values are refused. The resolved role supplies the child's posture. There is no per-call model override on this surface."
                },
                "worktree": {
                    "type": "boolean",
                    "description": "When true, create a fresh git worktree and branch for this child before it starts. Use for parallel edit tasks that must not collide with the parent checkout."
                },
                "write_roots": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Repo-relative directory trees a write-capable agent may mutate. On action=start: the scope this child claims, defaulting to the parent workspace ('.') when omitted. On action=claim: the trees to add to your own enforced scope, which you must do before mutating anything outside it. Paths outside the parent workspace are refused."
                },
                "resume_from": {
                    "type": "string",
                    "description": "Settled child agent_id or session name to continue. The source must not be running. Its full transcript is loaded and prepended as the new child's context (fork_context=true), continuing the transcript lineage under a new role or profile (e.g. explore → implementer → verifier). Mutually exclusive with fork_context=false. Cross-workspace or missing sources are rejected with a clear error."
                }
            },
            "dependentSchemas": {
                "action": {
                    "anyOf": [
                        {
                            "properties": {
                                "action": {"const": "start"},
                                "prompt": {}
                            },
                            "required": ["prompt"]
                        },
                        {
                            "properties": {"action": {"const": "roster"}}
                        },
                        {
                            "properties": {"action": {"const": "status"}}
                        },
                        {
                            "properties": {"action": {"const": "peek"}},
                            "anyOf": target_required.clone()
                        },
                        {
                            "properties": {
                                "action": {"const": "message"},
                                "message": {}
                            },
                            "required": ["message"],
                            "anyOf": target_required.clone()
                        },
                        {
                            "properties": {
                                "action": {"const": "followup"},
                                "message": {}
                            },
                            "required": ["message"],
                            "anyOf": target_required.clone()
                        },
                        {
                            "properties": {"action": {"const": "interrupt"}},
                            "anyOf": target_required.clone()
                        },
                        {
                            "properties": {"action": {"const": "wait"}}
                        },
                        {
                            // `claim` names no agent: it always widens the
                            // caller's own scope. Which of the three scope
                            // lists carries the entries is left to the call —
                            // `execute` refuses an empty claim rather than
                            // silently succeeding with no expansion.
                            "properties": {"action": {"const": "claim"}}
                        },
                        {
                            "properties": {"action": {"const": "cancel"}},
                            "anyOf": target_required
                        }
                    ]
                }
            },
            // Model-facing calls choose an action explicitly so the matching
            // dependent schema is always active. The parser still defaults a
            // missing action to start for stored and programmatic legacy calls.
            "required": ["action"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::ExecutesCode,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    /// #3801: status and peek are read-only queries — no approval needed.
    /// #4097: wait passively observes children — also read-only.
    /// #5186: starting an explicitly read-only role no longer modals in the
    /// default posture; the child's own posture gates keep it read-only from
    /// the inside. Write-capable spawns keep their gate.
    fn approval_requirement_for(&self, input: &Value) -> ApprovalRequirement {
        match parse_agent_tool_action(input) {
            Ok(
                AgentToolAction::Roster
                | AgentToolAction::Status
                | AgentToolAction::Peek
                | AgentToolAction::Wait,
            ) => ApprovalRequirement::Auto,
            Ok(AgentToolAction::Start) if start_requests_read_only_role(input) => {
                ApprovalRequirement::Auto
            }
            // #5462: `agents/coordinate` declared `Auto` because gating a
            // coordination record deadlocks autonomous fan-in — a child that
            // must widen its scope to proceed cannot raise a modal in a
            // parent UI nobody is watching. Inheriting the action must
            // inherit that reasoning, not quietly upgrade it to `Required`.
            // Authority is unchanged: `claim` can only widen the *caller's
            // own* scope, and contention with a peer claim still fails.
            Ok(AgentToolAction::Claim) => ApprovalRequirement::Auto,
            _ => ApprovalRequirement::Required,
        }
    }

    /// #3801: only explicit `detached=true` starts durable work that should not
    /// hold the global tool-exec write lock while the child spins up. Foreground
    /// starts still return promptly, but stay owned by their active turn.
    fn starts_detached_for(&self, input: &Value) -> bool {
        matches!(parse_agent_tool_action(input), Ok(AgentToolAction::Start))
            && input.get("detached").and_then(Value::as_bool) == Some(true)
    }

    /// #3801: Read-only `agent` actions (status, peek) can safely run in
    /// parallel batches.
    fn supports_parallel_for(&self, input: &Value) -> bool {
        matches!(
            parse_agent_tool_action(input),
            Ok(AgentToolAction::Roster) | Ok(AgentToolAction::Status) | Ok(AgentToolAction::Peek)
        )
    }

    /// #3801: status/peek actions are read-only queries of manager state.
    /// #4097: wait only observes child lifecycle — read-only as well.
    fn is_read_only_for(&self, input: &Value) -> bool {
        matches!(
            parse_agent_tool_action(input),
            Ok(AgentToolAction::Roster
                | AgentToolAction::Status
                | AgentToolAction::Peek
                | AgentToolAction::Wait)
        )
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let action = parse_agent_tool_action(&input)?;
        match action {
            AgentToolAction::Start => {}
            AgentToolAction::Roster => {
                // Role catalog, not a roster: exec spawns resolve roles only
                // (see `resolve_spawn_role`). The saved-member roster lives in
                // the durable Fleet UI (`/fleet`); the agent tool never reads it.
                let members: Vec<Value> = FleetRole::all()
                    .iter()
                    .map(|role| {
                        json!({
                            "member_id": role.as_str(),
                            "role": role.as_str(),
                            "description": role.description(),
                        })
                    })
                    .collect();
                let payload = json!({
                    "action": "roster",
                    "count": members.len(),
                    "total_count": members.len(),
                    "truncated": false,
                    "members": members,
                    "selector_help": "Use type:<role> with one of the listed roles. There are no saved members: every spawn resolves a role only.",
                });
                let mut result = ToolResult::json(&payload)
                    .map_err(|error| ToolError::execution_failed(error.to_string()))?;
                result.metadata = Some(json!({
                    "action": "roster",
                    "count": payload["count"],
                }));
                return Ok(result);
            }
            AgentToolAction::Status | AgentToolAction::Peek => {
                return inspect_agent_from_input(
                    &input,
                    self.manager.clone(),
                    context,
                    matches!(action, AgentToolAction::Peek),
                    Some(&self.inspect_memo),
                )
                .await;
            }
            AgentToolAction::Message => {
                return AgentsMessageTool::new(self.manager.clone())
                    .with_optional_caller(self.runtime.parent_agent_id.clone())
                    .execute(input, context)
                    .await;
            }
            AgentToolAction::Followup => {
                return AgentsFollowupTool::new(self.manager.clone())
                    .with_optional_caller(self.runtime.parent_agent_id.clone())
                    .with_runtime(self.runtime.clone())
                    .execute(input, context)
                    .await;
            }
            AgentToolAction::Interrupt => {
                return AgentsInterruptTool::new(self.manager.clone())
                    .with_optional_caller(self.runtime.parent_agent_id.clone())
                    .execute(input, context)
                    .await;
            }
            AgentToolAction::Wait => {
                // Shared with `agents/wait` so `until` (completion | all |
                // activity) means the same thing on both surfaces.
                return coord::dispatch_wait(&input, self.manager.clone(), context).await;
            }
            AgentToolAction::Cancel => {
                return cancel_agent_from_input(
                    &input,
                    self.manager.clone(),
                    context,
                    self.runtime.parent_agent_id.as_deref(),
                )
                .await;
            }
            AgentToolAction::Claim => {
                return AgentsCoordinateTool::new(
                    self.manager.clone(),
                    self.runtime.parent_agent_id.clone(),
                )
                .execute(agent_claim_coordinate_input(&input)?, context)
                .await;
            }
        }
        touch_running_shell_owners(
            &self.manager,
            &context.execution.shell_manager,
            &context.state_namespace,
        )
        .await;
        let verbose = input
            .get("verbose")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (snapshot, _spawn_metadata) =
            spawn_subagent_from_input(input, self.manager.clone(), self.runtime.clone()).await?;
        let worker_record = {
            let manager = self.manager.read().await;
            manager.get_worker_record_for_session(&context.state_namespace, &snapshot.agent_id)
        };
        let projection = subagent_session_projection(snapshot, false, context, worker_record).await;
        let mut value = serde_json::to_value(&projection)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        compact_spawn_receipt(&mut value, verbose);
        let mut tool_result =
            ToolResult::json(&value).map_err(|e| ToolError::execution_failed(e.to_string()))?;
        let metadata = json!({
            "action": "start",
            "agent_id": projection.agent_id,
            "status": projection.status,
            "terminal": projection.terminal,
            "context_mode": projection.context_mode,
            "prefix_cache": projection.prefix_cache,
            "child_route": projection.child_route,
        });
        tool_result.metadata = Some(metadata);
        Ok(tool_result)
    }
}

/// A spawn receipt is an acknowledgement, not an archive: the full projection
/// carried the complete child prompt (launch_manifest inside `worker_record`)
/// plus a duplicated `snapshot` — ~12KB per spawn (morning-report issue #4).
/// Same compaction contract as unscoped status (9fa5e04e6): strip the heavy
/// keys, say so, and let `verbose: true` restore the old shape.
fn compact_spawn_receipt(value: &mut Value, verbose: bool) {
    if verbose {
        return;
    }
    let Some(object) = value.as_object_mut() else {
        return;
    };
    object.remove("snapshot");
    object.remove("worker_record");
    object.remove("checkpoint");
    object.remove("artifacts");
    object.remove("takeover");
    object.remove("transcript_handle");
    object.remove("verification");
    object.insert("compact".to_string(), json!(true));
    object.insert(
        "compact_note".to_string(),
        json!("Spawn receipt compacted; inspect with agent_id or start verbose: true."),
    );
}

/// Repeat peek/status calls on an unchanged running child inside this window
/// return a compact "no change" nudge instead of a full projection (#4097).
const PEEK_UNCHANGED_THROTTLE_WINDOW: Duration = Duration::from_secs(30);

/// Stable change fingerprint for a running child's model-visible state.
/// Volatile fields (durations, timestamps) are deliberately excluded so an
/// idle child fingerprints identically across back-to-back peeks.
fn inspect_fingerprint(snapshot: &SubAgentResult) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    subagent_status_name(&snapshot.status).hash(&mut hasher);
    snapshot.steps_taken.hash(&mut hasher);
    snapshot.result.is_some().hash(&mut hasher);
    snapshot.needs_input.is_some().hash(&mut hasher);
    snapshot.checkpoint.is_some().hash(&mut hasher);
    hasher.finish()
}

async fn inspect_agent_from_input(
    input: &Value,
    manager: SharedSubAgentManager,
    context: &ToolContext,
    peek: bool,
    inspect_memo: Option<&Arc<std::sync::Mutex<HashMap<String, PeekMemo>>>>,
) -> Result<ToolResult, ToolError> {
    let include_archived =
        parse_optional_bool(input, &["include_archived", "includeArchived"])?.unwrap_or(false);

    if let Some(agent_ref) = parse_agent_ref(input)? {
        let (snapshot, worker_record, evicted_ids) = {
            touch_running_shell_owners(
                &manager,
                &context.execution.shell_manager,
                &context.state_namespace,
            )
            .await;
            let mut manager = manager.write().await;
            manager.cleanup_for_session(&context.state_namespace, COMPLETED_AGENT_RETENTION);
            let evicted_ids =
                manager.drain_pending_handle_evictions_for_session(&context.state_namespace);
            let snapshot = manager
                .get_result_by_ref_for_session(&context.state_namespace, &agent_ref)
                .map_err(|err| ToolError::invalid_input(err.to_string()))?;
            let worker_record =
                manager.get_worker_record_for_session(&context.state_namespace, &snapshot.agent_id);
            (snapshot, worker_record, evicted_ids)
        };
        // Evict retired handles outside the manager lock (#3885).
        if !evicted_ids.is_empty() {
            let mut store = context.runtime.handle_store.lock().await;
            for agent_id in &evicted_ids {
                store.evict_session(&format!("agent:{agent_id}"));
            }
        }

        // #4097: a running child whose model-visible state hasn't changed
        // since the last peek gets a compact nudge, not another full
        // projection. Terminal/parked children always return in full — the
        // model may legitimately be fetching results.
        if snapshot.status == SubAgentStatus::Running
            && let Some(memo_map) = inspect_memo
        {
            let fingerprint = inspect_fingerprint(&snapshot);
            let now = Instant::now();
            let unchanged = {
                let mut memo_map = memo_map.lock().expect("inspect memo lock");
                let unchanged = memo_map.get(&snapshot.agent_id).is_some_and(|memo| {
                    memo.fingerprint == fingerprint
                        && now.duration_since(memo.at) < PEEK_UNCHANGED_THROTTLE_WINDOW
                });
                memo_map.insert(
                    snapshot.agent_id.clone(),
                    PeekMemo {
                        fingerprint,
                        at: now,
                    },
                );
                unchanged
            };
            if unchanged {
                let child_route = worker_record
                    .as_ref()
                    .and_then(|record| record.spec.child_route.clone());
                let payload = json!({
                    "action": if peek { "peek" } else { "status" },
                    "agent_id": snapshot.agent_id,
                    "name": snapshot.name,
                    "status": "running",
                    "unchanged": true,
                    "child_route": child_route,
                    "hint": "No change since your last check. Checking again in a loop is the anti-pattern; one blocking wait is not. Make one agent(action=\"wait\") call — until=\"all\" to join every running child in a single block — or continue independent work, or end your turn. Results arrive automatically as <codewhale:subagent.done> sentinels.",
                });
                let mut tool_result = ToolResult::json(&payload)
                    .map_err(|err| ToolError::execution_failed(err.to_string()))?;
                tool_result.metadata = Some(json!({
                    "action": if peek { "peek" } else { "status" },
                    "status": "running",
                    "terminal": false,
                    "agent_id": payload["agent_id"],
                    "unchanged": true,
                    "child_route": child_route,
                }));
                return Ok(tool_result);
            }
        }

        let projection =
            subagent_session_projection(snapshot, include_archived, context, worker_record).await;
        let mut tool_result = ToolResult::json(&projection)
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
        tool_result.metadata = Some(json!({
            "action": if peek { "peek" } else { "status" },
            "status": projection.status,
            "terminal": projection.terminal,
            "agent_id": projection.agent_id,
            "child_route": projection.child_route,
        }));
        return Ok(tool_result);
    }

    let (snapshots, evicted_ids) = {
        touch_running_shell_owners(
            &manager,
            &context.execution.shell_manager,
            &context.state_namespace,
        )
        .await;
        let mut manager = manager.write().await;
        manager.cleanup_for_session(&context.state_namespace, COMPLETED_AGENT_RETENTION);
        let evicted_ids =
            manager.drain_pending_handle_evictions_for_session(&context.state_namespace);
        let snapshots = manager
            .list_filtered_for_session(&context.state_namespace, include_archived)
            .into_iter()
            .map(|snapshot| {
                let worker_record = manager
                    .get_worker_record_for_session(&context.state_namespace, &snapshot.agent_id);
                (snapshot, worker_record)
            })
            .collect::<Vec<_>>();
        (snapshots, evicted_ids)
    };
    // Evict retired handles outside the manager lock (#3885).
    if !evicted_ids.is_empty() {
        let mut store = context.runtime.handle_store.lock().await;
        for agent_id in &evicted_ids {
            store.evict_session(&format!("agent:{agent_id}"));
        }
    }

    // Unscoped status is a supervision poll, and it used to return the full
    // projection for every agent — launch manifest, event ring, and any
    // checkpointed message history included (one observed poll: 203KB).
    // Running children now compact to their top-level supervision facts
    // (status, usage, follow-up, verification stay; the heavy snapshot and
    // worker_record drop). Terminal agents keep the full projection —
    // fetching results is the point of a terminal row — and `verbose: true`
    // restores the old shape everywhere.
    let verbose = input
        .get("verbose")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut projections = Vec::with_capacity(snapshots.len());
    for (snapshot, worker_record) in snapshots {
        let running = snapshot.status == SubAgentStatus::Running;
        let projection =
            subagent_session_projection(snapshot, include_archived, context, worker_record).await;
        let mut value = serde_json::to_value(&projection)
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
        if running
            && !verbose
            && let Some(object) = value.as_object_mut()
        {
            object.remove("snapshot");
            object.remove("worker_record");
            object.remove("checkpoint");
            object.insert("compact".to_string(), json!(true));
            object.insert(
                "compact_note".to_string(),
                json!(
                    "Running child compacted; pass agent_id (or verbose: true) for the full projection."
                ),
            );
        }
        projections.push(value);
    }
    let payload = json!({
        "action": if peek { "peek" } else { "status" },
        "count": projections.len(),
        "agents": projections,
    });
    let mut tool_result =
        ToolResult::json(&payload).map_err(|err| ToolError::execution_failed(err.to_string()))?;
    tool_result.metadata = Some(json!({
        "action": if peek { "peek" } else { "status" },
        "count": payload["count"],
    }));
    Ok(tool_result)
}

/// Keep a running child alive while one of its tracked background shell jobs
/// is still active. Shell output is progress owned by the child even before
/// the next model-visible completion event is emitted.
async fn touch_running_shell_owners(
    manager: &SharedSubAgentManager,
    shell_manager: &SharedShellManager,
    active_session_id: &str,
) {
    let owner_ids = {
        let Ok(mut shell_manager) = shell_manager.lock() else {
            return;
        };
        shell_manager.running_owner_agent_ids_for_session(active_session_id)
    };
    if owner_ids.is_empty() {
        return;
    }
    let mut manager = manager.write().await;
    for owner_id in owner_ids {
        manager.touch_for_session(active_session_id, &owner_id);
    }
}

async fn cancel_agent_from_input(
    input: &Value,
    manager: SharedSubAgentManager,
    context: &ToolContext,
    caller_agent_id: Option<&str>,
) -> Result<ToolResult, ToolError> {
    let agent_ref = parse_agent_ref(input)?.ok_or_else(|| ToolError::missing_field("agent_id"))?;
    let (snapshot, worker_record) = {
        let mut manager = manager.write().await;
        manager
            .ensure_caller_controls_descendant_for_session(
                &context.state_namespace,
                &agent_ref,
                caller_agent_id,
                "agent/cancel",
            )
            .map_err(|err| ToolError::invalid_input(err.to_string()))?;
        let snapshot = manager
            .cancel_agent_for_session(&context.state_namespace, &agent_ref)
            .map_err(|err| ToolError::invalid_input(err.to_string()))?;
        let worker_record =
            manager.get_worker_record_for_session(&context.state_namespace, &snapshot.agent_id);
        (snapshot, worker_record)
    };
    let projection = subagent_session_projection(snapshot, false, context, worker_record).await;
    let mut tool_result = ToolResult::json(&projection)
        .map_err(|err| ToolError::execution_failed(err.to_string()))?;
    tool_result.metadata = Some(json!({
        "action": "cancel",
        "status": projection.status,
        "terminal": projection.terminal,
        "agent_id": projection.agent_id,
    }));
    Ok(tool_result)
}

/// Bounds for `agent(action="wait")` (#4097). The default is short so a
/// `wait` does not make the session deaf: the turn is blocked for at most
/// this long and user messages have nowhere to land. Results arrive
/// automatically as `<codewhale:subagent.done>` sentinels, so ending the
/// turn and staying reachable is the preferred default — only `wait` when
/// you must join before continuing.
const SUBAGENT_WAIT_DEFAULT_TIMEOUT_SECS: u64 = 30;
/// Runtime floor is 1s (schema advertises 5) so tests can exercise the
/// timeout path without multi-second sleeps.
const SUBAGENT_WAIT_MIN_TIMEOUT_SECS: u64 = 1;
const SUBAGENT_WAIT_MAX_TIMEOUT_SECS: u64 = 120;
/// Internal state-check cadence while blocked. Invisible to the model — the
/// #4097 anti-pattern is model-visible polling that burns turns and tokens,
/// not a cheap in-process timer.
const SUBAGENT_WAIT_CHECK_INTERVAL: Duration = Duration::from_millis(250);

/// `agent(action="wait")`: block until a running child settles (leaves
/// `Running` — completed, failed, cancelled, interrupted/needs-input, or
/// budget-exhausted), then return a compact summary. Full child results are
/// still delivered as `<codewhale:subagent.done>` sentinels by the runtime;
/// this call only provides the legitimate "join" the model previously faked
/// with peek→sleep loops (#4097).
///
/// With `agent_id`, waits for that child specifically. Without it, waits for
/// the next child to settle (returning every child that settled while
/// blocked). Returns immediately when nothing is running. Cancel-safe: the
/// engine turn's cancel token interrupts the block, and no lock is held
/// across an await.
///
/// This is the `until=completion` arm only. `until=all` — the fan-out join
/// that blocks for *every* watched child — lives in
/// [`coord::dispatch_wait`]; both arms are reachable from `agents/wait` and
/// from `agent(action="wait")`.
async fn wait_for_subagents_from_input(
    input: &Value,
    manager: SharedSubAgentManager,
    context: &ToolContext,
) -> Result<ToolResult, ToolError> {
    let timeout_secs = parse_optional_u64(input, &["timeout_secs", "timeout"])?
        .unwrap_or(SUBAGENT_WAIT_DEFAULT_TIMEOUT_SECS)
        .clamp(
            SUBAGENT_WAIT_MIN_TIMEOUT_SECS,
            SUBAGENT_WAIT_MAX_TIMEOUT_SECS,
        );
    let timeout = Duration::from_secs(timeout_secs);
    let agent_ref = parse_agent_ref(input)?;

    // Resolve the watch set up front so a bad reference fails immediately
    // instead of blocking for the full timeout.
    let watched: Vec<String> = {
        let manager = manager.read().await;
        if let Some(agent_ref) = &agent_ref {
            let snapshot = manager
                .get_result_by_ref_for_session(&context.state_namespace, agent_ref)
                .map_err(|err| ToolError::invalid_input(err.to_string()))?;
            if snapshot.status != SubAgentStatus::Running {
                let running = manager.running_count_for_session(&context.state_namespace);
                drop(manager);
                return wait_result_payload(&[snapshot], running, 0, false).await;
            }
            vec![snapshot.agent_id]
        } else {
            manager
                .list_filtered_for_session(&context.state_namespace, false)
                .into_iter()
                .filter(|snapshot| snapshot.status == SubAgentStatus::Running)
                .map(|snapshot| snapshot.agent_id)
                .collect()
        }
    };

    if watched.is_empty() {
        let payload = json!({
            "action": "wait",
            "settled": [],
            "running": 0,
            "note": "No running sub-agents; nothing to wait for.",
        });
        let mut tool_result = ToolResult::json(&payload)
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
        tool_result.metadata = Some(json!({ "action": "wait", "settled": 0, "running": 0 }));
        return Ok(tool_result);
    }

    let started = Instant::now();
    let cancelled = async {
        match &context.cancel_token {
            Some(token) => token.cancelled().await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(cancelled);

    loop {
        let (settled, running) = {
            let manager = manager.read().await;
            let mut settled = Vec::new();
            for agent_id in &watched {
                if let Ok(snapshot) =
                    manager.get_result_by_ref_for_session(&context.state_namespace, agent_id)
                    && snapshot.status != SubAgentStatus::Running
                {
                    settled.push(snapshot);
                }
            }
            (
                settled,
                manager.running_count_for_session(&context.state_namespace),
            )
        };

        if !settled.is_empty() || running == 0 {
            return wait_result_payload(&settled, running, started.elapsed().as_millis(), false)
                .await;
        }
        if started.elapsed() >= timeout {
            return wait_result_payload(&[], running, started.elapsed().as_millis(), true).await;
        }

        tokio::select! {
            () = &mut cancelled => {
                return Ok(ToolResult::success(
                    "Wait interrupted by user cancellation before any sub-agent settled.",
                ));
            }
            () = tokio::time::sleep(SUBAGENT_WAIT_CHECK_INTERVAL) => {}
        }
    }
}

/// Compact `action=wait` result. Deliberately not a full projection: the
/// runtime's completion sentinels (and a follow-up peek on a settled child)
/// carry the full payload; duplicating it here would double token cost.
async fn wait_result_payload(
    settled: &[SubAgentResult],
    running: usize,
    waited_ms: u128,
    timed_out: bool,
) -> Result<ToolResult, ToolError> {
    let settled_entries: Vec<Value> = settled
        .iter()
        .map(|snapshot| {
            json!({
                "agent_id": snapshot.agent_id,
                "name": snapshot.name,
                "status": subagent_status_name(&snapshot.status),
            })
        })
        .collect();
    let note = if timed_out {
        "Wait timed out with children still running. Do not poll — wait again (until=\"all\" blocks for the whole batch), continue independent work, or end your turn; results arrive automatically as <codewhale:subagent.done> sentinels."
    } else if settled_entries.is_empty() {
        "No sub-agents are running anymore."
    } else {
        "Full results arrive as <codewhale:subagent.done> sentinels — read those before synthesizing; do not re-peek settled children unless you need the full projection."
    };
    let payload = json!({
        "action": "wait",
        "settled": settled_entries,
        "running": running,
        "waited_ms": u64::try_from(waited_ms).unwrap_or(u64::MAX),
        "timed_out": timed_out,
        "note": note,
    });
    let mut tool_result =
        ToolResult::json(&payload).map_err(|err| ToolError::execution_failed(err.to_string()))?;
    tool_result.metadata = Some(json!({
        "action": "wait",
        "settled": settled.len(),
        "running": running,
        "timed_out": timed_out,
    }));
    Ok(tool_result)
}

async fn spawn_subagent_from_input(
    input: Value,
    manager: SharedSubAgentManager,
    mut runtime: SubAgentRuntime,
) -> Result<(SubAgentResult, WorkflowTaskSpawnMetadata), ToolError> {
    apply_session_spawn_defaults(&mut runtime);
    refresh_spawn_route_sources(&mut runtime);
    let mut spawn_request = parse_spawn_request(&input)?;
    let requested_route = RequestedChildRoute {
        requested_type: spawn_request.agent_type.as_str().to_string(),
        requested_profile: spawn_request.profile.clone(),
        requested_reasoning: subagent_thinking_label(spawn_request.thinking).to_string(),
    };
    resolve_spawn_role(&mut spawn_request)?;
    // Role resolution runs before classification so the bounded-write contract
    // sees the effective role: read-only roles stay ergonomic while a
    // manager/builder role can never acquire an implicit repository-wide
    // write claim.
    validate_spawn_write_contract(&mut spawn_request, false)?;

    if runtime.would_exceed_depth() {
        return Err(ToolError::execution_failed(format!(
            "Sub-agent depth limit reached (current depth {}, max {}). \
             Increase via [subagents] max_depth in config.toml.",
            runtime.spawn_depth, runtime.max_spawn_depth
        )));
    }

    if let Some(remaining) = crate::retry_status::rate_limit_remaining() {
        let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
        return Err(ToolError::execution_failed(format!(
            "Provider is rate-limiting; sub-agent spawning is paused for {seconds}s. \
             Wait for the current backoff window before starting new agent work."
        )));
    }

    let mut child_runtime = if spawn_request.detached {
        runtime.background_runtime()
    } else {
        runtime.child_runtime()
    };
    // Role-only dispatch inherits the session client: there are no saved
    // provider pins outside the durable Fleet runs, so every child runs on
    // the parent's provider and there is no cross-provider client to build.
    let mut model_selection = resolve_spawn_model_selection(&child_runtime, &spawn_request)?;
    resolve_fixed_spawn_model_route(&child_runtime, &mut model_selection, true)?;
    let resident_context = spawn_request
        .resident_file
        .as_deref()
        .map(|file_path| read_bounded_resident_context(&runtime.context, file_path))
        .transpose()?;
    let effective_prompt = assemble_spawn_prompt(&spawn_request, resident_context.as_ref());
    let route = resolve_subagent_assignment_route(
        &child_runtime,
        None,
        &effective_prompt,
        &spawn_request.agent_type,
        model_selection.model_route,
        spawn_request.thinking,
    )
    .await;
    let effective_model =
        ensure_subagent_model_for_provider(&child_runtime, &route.model_route, route.model)?;
    child_runtime.model = effective_model.clone();
    if let Some(rebound) = child_runtime
        .client
        .rebound_for_model_protocol(child_runtime.api_config.as_deref(), &effective_model)
        .map_err(|err| {
            ToolError::execution_failed(format!(
                "Fleet dispatch could not bind the wire protocol for model {effective_model:?}: {err:#}"
            ))
        })?
    {
        child_runtime.client = rebound;
    }
    child_runtime.reasoning_effort = route.reasoning_effort.clone();
    child_runtime.reasoning_effort_auto = false;
    let model_route = route.model_route;
    let child_route = mint_child_route_receipt(
        &requested_route,
        &spawn_request,
        &child_runtime,
        effective_model.clone(),
        model_selection.source.as_str(),
    )?;

    if spawn_request.worktree.is_some() {
        let manager_guard = manager.read().await;
        manager_guard
            .check_admission_capacity()
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
    }
    let child_workspace = prepare_child_workspace(
        &runtime.context.workspace,
        spawn_request.cwd.as_deref(),
        spawn_request.worktree.as_ref(),
        spawn_request.session_name.as_deref(),
        &spawn_request.agent_type,
    )?;

    child_runtime.max_spawn_depth = child_max_spawn_depth_for_spawn(
        child_runtime.max_spawn_depth,
        child_runtime.spawn_depth,
        spawn_request.max_depth,
    );
    if let Some(workspace) = child_workspace {
        child_runtime.context.workspace = workspace.clone();
        // A worktree child gets a distinct workspace-scoped plugin catalog.
        // Reusing the parent's registry here would leak workspace plugins (and
        // their authority receipts) across the exact isolation boundary the
        // child requested. User-global roots remain available through the
        // registry's frozen pre-dotenv discovery context.
        if let Some(parent_plugins) = child_runtime.context.plugin_registry.as_ref() {
            child_runtime.context.plugin_registry =
                Some(parent_plugins.rediscover_for_workspace(&workspace));
        }
    }
    // #4042: merge the parent runtime's inherited deny-list with the caller's
    // explicit `disallowed_tools`. `background_runtime()` already cloned the
    // parent's `worker_profile.denied_tools` (the session `--disallowed-tools`),
    // so by default the child inherits it. `inherit_disallowed_tools: false`
    // drops *only* the inherited list; an explicit caller `disallowed_tools`
    // always applies (union, deny never relaxes).
    if !spawn_request.inherit_disallowed_tools {
        // Drops the *preference* half of the inherited list only. A rule that
        // expresses an enforced ceiling survives, because a child that could
        // clear it would be widening its parent's network/write/execution
        // envelope by asking — see `crate::fleet::role::is_posture_denial`.
        child_runtime
            .worker_profile
            .denied_tools
            .retain(|rule| is_posture_denial(rule));
    }
    if let Some(ref caller_deny) = spawn_request.disallowed_tools {
        for tool in caller_deny {
            if !child_runtime
                .worker_profile
                .denied_tools
                .iter()
                .any(|existing| existing == tool)
            {
                child_runtime.worker_profile.denied_tools.push(tool.clone());
            }
        }
    }
    apply_spawn_write_authority(&mut child_runtime, &spawn_request);
    let write_capable = spawn_request_is_write_capable(&spawn_request);
    let write_claim = write_capable.then(|| WriteScopeClaim {
        owner: String::new(),
        roots: spawn_request.write_roots.clone(),
        exact_files: spawn_request.exact_files.clone(),
        contracts: spawn_request.coordination_contracts.clone(),
    });
    let mut spawn_metadata = WorkflowTaskSpawnMetadata {
        resolved_provider: child_route.provider_id.clone(),
        resolved_model: child_route.model_id.clone(),
        route_source: child_route.route_source.clone(),
        requested_reasoning: Some(child_route.requested_reasoning.clone()),
        effective_reasoning: child_route.effective_reasoning.clone(),
        resolved_role: Some(child_route.canonical_role.clone()),
        resolved_profile: child_route.resolved_profile_id.clone(),
        child_route: child_route.clone(),
        parent_task_id: child_runtime.parent_agent_id.clone(),
        depth: child_runtime.spawn_depth,
        workflow_run_id: None,
        workflow_phase_id: None,
        workflow_task_label: None,
        workflow_child_index: None,
        resume_from_agent_id: None,
    };

    // #4647: a child receives only its explicit objective/dependencies/
    // acceptance and relevant accepted-decision projection by default. Forking
    // a parent transcript is still available, but only through an explicit
    // `fork_context: true` request.
    let fork_context = spawn_request.fork_context.unwrap_or(false);

    // resolve_resume_from: look up the source agent, validate it is settled
    // and lives in the same workspace, then load its transcript so the new
    // child inherits the full lineage (issue #425).
    let (fork_context, resume_from_agent_id) =
        if let Some(ref source_ref) = spawn_request.resume_from {
            // Validate: fork_context=false is incompatible with resume_from.
            if spawn_request.fork_context == Some(false) {
                return Err(ToolError::invalid_input(
                    "resume_from requires fork_context to be true or unset; \
                 explicit fork_context=false conflicts with transcript continuation."
                        .to_string(),
                ));
            }
            let (source_agent_id, source_workspace, source_state_root, checkpoint_messages) = {
                let manager_read = manager.read().await;
                let source_id = manager_read
                    .resolve_agent_ref_for_session(&runtime.context.state_namespace, source_ref)
                    .map_err(|_| {
                        ToolError::invalid_input(format!(
                            "resume_from: agent or session '{source_ref}' not found. \
                     Use agent action=status to list available agents."
                        ))
                    })?;
                let source = manager_read.agents.get(&source_id).ok_or_else(|| {
                    ToolError::invalid_input(format!("resume_from: agent '{source_id}' not found"))
                })?;
                if source.status == SubAgentStatus::Running {
                    return Err(ToolError::invalid_input(format!(
                        "resume_from: agent '{source_id}' (session '{}') is still running. \
                     Only settled agents (completed, interrupted, failed, cancelled) \
                     may be used as a resume source. Use action=wait to block until \
                     it settles, or action=interrupt to stop it.",
                        source.session_name
                    )));
                }
                // Capture checkpoint messages now while holding the read lock; used
                // as a fallback when the transcript artifact is unavailable.
                let checkpoint_messages = source
                    .checkpoint
                    .as_ref()
                    .filter(|cp| cp.continuable && !cp.messages.is_empty())
                    .map(|cp| cp.messages.clone())
                    .unwrap_or_default();
                (
                    source_id,
                    source.workspace.clone(),
                    manager_read.state_root.clone(),
                    checkpoint_messages,
                )
            };
            // Cross-workspace resume remains unsupported because execution
            // authority and inherited context belong to the source workspace,
            // even when the transcript artifact itself has a separate state root.
            let parent_workspace = normalize_subagent_workspace(&runtime.context.workspace);
            let source_workspace_normalized = normalize_subagent_workspace(&source_workspace);
            if parent_workspace != source_workspace_normalized {
                return Err(ToolError::invalid_input(format!(
                    "resume_from: source agent '{source_agent_id}' lives in a different \
                 workspace ({}) than this agent ({}). Cross-workspace continuation \
                 is not supported.",
                    source_workspace.display(),
                    runtime.context.workspace.display()
                )));
            }
            // Load the full transcript from the on-disk artifact. Fall back to the
            // checkpoint messages for legacy records that predate transcript
            // artifacts or for agents whose artifacts were cleaned up.
            let messages = load_subagent_transcript_artifact(&source_state_root, &source_agent_id)
                .unwrap_or(checkpoint_messages);
            let resume_ctx = SubAgentForkContext {
                messages,
                structured_state_block: None,
                work_source: None,
            };
            child_runtime.fork_context = Some(resume_ctx);
            spawn_metadata.resume_from_agent_id = Some(source_agent_id.clone());
            (true, Some(source_agent_id))
        } else {
            (fork_context, None)
        };

    let resident_lease = resident_context
        .as_ref()
        .map(|resident| (resident.lease_key.clone(), resident.display_path.clone()));
    if let Some((lease_key, display_path)) = resident_lease.as_ref() {
        reserve_resident_lease(lease_key, display_path)?;
    }
    let mut manager_guard = manager.write().await;

    let result = manager_guard.spawn_background_with_assignment_options(
        Arc::clone(&manager),
        child_runtime,
        spawn_request.agent_type,
        effective_prompt,
        spawn_request.assignment,
        spawn_request.allowed_tools,
        SubAgentSpawnOptions {
            name: spawn_request.session_name.clone(),
            model: Some(effective_model),
            model_route: Some(model_route),
            child_route: Some(child_route),
            nickname: None,
            fork_context,
            token_budget: spawn_request.token_budget,
            max_steps: spawn_request.max_steps,
            wall_time: spawn_request.wall_time,
            write_claim,
            isolated_worktree: spawn_request.worktree.is_some(),
            expected_artifact: spawn_request.expected_artifact.clone(),
            resume_from_agent_id: resume_from_agent_id.clone(),
            claim_pre_namespaced: false,
            preserve_runtime_profile: None,
        },
    );
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            if let Some((lease_key, _)) = resident_lease.as_ref() {
                rollback_pending_resident_lease(lease_key);
            }
            return Err(ToolError::execution_failed(format!(
                "Failed to spawn sub-agent: {error}"
            )));
        }
    };

    if let Some((lease_key, _)) = resident_lease.as_ref() {
        commit_resident_lease(lease_key, &result.agent_id);
    }

    Ok((result, spawn_metadata))
}
const CHILD_ROUTE_RECEIPT_MAX_BYTES: usize = 448;

fn assemble_spawn_prompt(request: &SpawnRequest, resident: Option<&ResidentContext>) -> String {
    let prompt = match resident {
        Some(resident) => format!(
            "<!-- resident_file: {} -->\n```\n{}\n```\n\n{}",
            resident.display_path, resident.contents, request.prompt
        ),
        None => request.prompt.clone(),
    };
    let prompt = match request.expected_artifact.as_deref() {
        Some(artifact) => {
            format!("{prompt}\n\nExpected artifact (declared by the spawner): {artifact}")
        }
        None => prompt,
    };
    if request.dependencies.is_empty() && request.acceptance.is_empty() {
        return prompt;
    }
    let lines = |items: &[String]| {
        items
            .iter()
            .map(|item| format!("- {}", item.chars().take(256).collect::<String>()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "{prompt}\n\nDelegation contract (bounded):\nDependencies:\n{}\nAcceptance:\n{}",
        lines(&request.dependencies),
        lines(&request.acceptance),
    )
}

fn mint_child_route_receipt(
    requested_route: &RequestedChildRoute,
    request: &SpawnRequest,
    runtime: &SubAgentRuntime,
    model_id: String,
    route_source: &str,
) -> Result<ChildRouteReceipt, ToolError> {
    // Role-only dispatch: the canonical role comes from the resolved request,
    // and no saved member is ever bound, so the profile fields stay empty.
    let canonical_role = request
        .assignment
        .role
        .as_deref()
        .map(public_role_label)
        .unwrap_or_else(|| request.agent_type.as_str().to_string());
    let provider_id = runtime
        .api_config
        .as_ref()
        .map(|config| config.provider_identity_for(runtime.client.api_provider()))
        .unwrap_or_else(|| runtime.client.api_provider().as_str().to_string());
    let receipt = ChildRouteReceipt {
        requested_type: requested_route.requested_type.clone(),
        requested_profile: requested_route.requested_profile.clone(),
        resolved_profile_id: None,
        profile_origin: None,
        canonical_role,
        provider_id,
        model_id,
        route_source: route_source.to_string(),
        requested_reasoning: requested_route.requested_reasoning.clone(),
        effective_reasoning: runtime.reasoning_effort.clone(),
        runtime_version: env!("CARGO_PKG_VERSION").to_string(),
        // CI builds embed the full 40-hex GITHUB_SHA; a full-length field
        // pushed real receipts past 384 bytes (394 with the sha, 386 on some
        // legitimate routes without it), breaking admission. Twelve hex chars
        // plus the version identify the build without bloating every receipt.
        runtime_build_sha: option_env!("CODEWHALE_BUILD_COMMIT")
            .map(|sha| sha.get(..12).unwrap_or(sha).to_string())
            .unwrap_or_else(|| "unknown".to_string()),
    };
    let length = serde_json::to_vec(&receipt)
        .map_err(|error| ToolError::execution_failed(error.to_string()))?
        .len();
    if length > CHILD_ROUTE_RECEIPT_MAX_BYTES {
        return Err(ToolError::invalid_input(format!(
            "resolved child route receipt is {length} bytes; the {CHILD_ROUTE_RECEIPT_MAX_BYTES}-byte limit prevents an oversized admission record"
        )));
    }
    Ok(receipt)
}

fn apply_spawn_write_authority(runtime: &mut SubAgentRuntime, request: &SpawnRequest) {
    if request.write_authority != Some(SpawnWriteAuthority::ReadOnly) {
        return;
    }
    // `read_only` must be an executable posture, not just metadata. Normally
    // write-capable identities also inherit Full shell, which could mutate the
    // workspace without a scope-aware claim under Auto/Full Access. Clamp that
    // shell surface completely; verifier keeps its deliberate test runner.
    runtime.worker_profile.permissions.write = false;
    if matches!(
        request.agent_type,
        FleetRole::Worker | FleetRole::Builder | FleetRole::Custom
    ) {
        runtime.worker_profile.shell = ShellPolicy::None;
    }
}

fn spawn_request_is_write_capable(request: &SpawnRequest) -> bool {
    match request.agent_type {
        FleetRole::Worker | FleetRole::Builder => {
            request.write_authority != Some(SpawnWriteAuthority::ReadOnly)
        }
        FleetRole::Custom => matches!(
            request.write_authority,
            Some(SpawnWriteAuthority::WorkspaceWrite | SpawnWriteAuthority::WorktreeWrite)
        ),
        FleetRole::Scout
        | FleetRole::Planner
        | FleetRole::Reviewer
        | FleetRole::Verifier
        | FleetRole::Consultant => false,
    }
}

/// A root Operate dispatch has already crossed the approval boundary on the
/// `agent` call. Delegate Suggest-level file edits and the bounded built-in
/// verification surfaces so a normal message can produce verified work.
/// Arbitrary shell and custom verifier commands still follow the active
/// permission posture.
fn apply_session_spawn_defaults(runtime: &mut SubAgentRuntime) {
    if runtime.spawn_depth == 0 && runtime.parent_mode == AppMode::Operate {
        runtime.accept_edits = true;
        runtime.accept_verification = true;
    }
}

/// Spawn one Workflow `task(...)` through the same path as the public `agent`
/// tool. Keeping this adapter inside the sub-agent module prevents the
/// Workflow driver from copying Fleet roster/profile/depth/budget semantics.
///
/// `identity` is stamped onto the returned spawn metadata so panel/history
/// consumers can render workflow children without parsing prompt text (#4119).
pub(crate) async fn spawn_workflow_task(
    request: codewhale_workflow_js::TaskRequest,
    manager: SharedSubAgentManager,
    mut runtime: SubAgentRuntime,
    identity: WorkflowTaskSpawnIdentity,
) -> Result<WorkflowTaskSpawnResult, ToolError> {
    // Capture identity fallbacks before consuming `request` fields into the
    // agent-tool input JSON.
    let request_label = request
        .label
        .as_ref()
        .map(|label| label.trim())
        .filter(|label| !label.is_empty())
        .map(str::to_string);
    let request_phase = request
        .phase
        .as_ref()
        .map(|phase| phase.trim())
        .filter(|phase| !phase.is_empty())
        .map(str::to_string);
    let mut input = json!({
        "prompt": request.description,
        "worktree": request.worktree,
    });
    if let Some(value) = request.cwd {
        input["cwd"] = json!(value);
    }
    if let Some(value) = request.write_authority {
        input["write_authority"] = json!(value);
    }
    if !request.write_roots.is_empty() {
        input["write_roots"] = json!(request.write_roots);
    }
    if !request.exact_files.is_empty() {
        input["exact_files"] = json!(request.exact_files);
    }
    if !request.coordination_contracts.is_empty() {
        input["coordination_contracts"] = json!(request.coordination_contracts);
    }
    if !request.dependencies.is_empty() {
        input["dependencies"] = json!(request.dependencies);
    }
    if !request.acceptance.is_empty() {
        input["acceptance"] = json!(request.acceptance);
    }
    if let Some(value) = request.subagent_type {
        input["type"] = json!(value);
    }
    if let Some(value) = request.role {
        input["role"] = json!(value);
    }
    if let Some(value) = request.profile {
        input["profile"] = json!(value);
    }
    if let Some(value) = request.model {
        input["model"] = json!(value);
    }
    if let Some(value) = request.model_strength {
        input["model_strength"] = json!(value);
    }
    if let Some(value) = request.thinking {
        input["thinking"] = json!(value);
    }
    if let Some(value) = request.allowed_tools {
        input["allowed_tools"] = json!(value);
    }
    // A host-derived ceiling (e.g. an exact Fleet member's
    // `network_tool = false`) reaches the child as a deny list, which the child
    // registry applies to both the model-visible surface and the call guard.
    if !request.disallowed_tools.is_empty() {
        input["disallowed_tools"] = json!(request.disallowed_tools);
    }
    if let Some(value) = request.max_depth {
        input["max_depth"] = json!(value);
    }
    if let Some(value) = request.token_budget {
        input["token_budget"] = json!(value);
    }
    if let Some(value) = request.max_steps {
        input["max_steps"] = json!(value);
    }
    if let Some(value) = request.wall_time_secs {
        input["wall_time_secs"] = json!(value);
    }
    // An exact-Fleet task must arrive here carrying the envelope its receipt
    // names, and the input just built is the last place both are visible. A
    // missing, stale, or mismatched fingerprint fails the spawn closed rather
    // than launching a child whose surface nobody can tie back to a decision.
    if let Some(expected) = identity.fleet_authority_fingerprint.as_deref() {
        // `spawn_workflow_task` reports `ToolError`, so the verification's
        // `anyhow` failure is mapped onto the variant that matches what just
        // happened: the child's surface could not be authorized, so it is
        // denied rather than launched.
        verify_fleet_authority_input(expected, &input)
            .map_err(|err| ToolError::permission_denied(err.to_string()))?;
    }
    // Workflow children inherit the parent tool surface and auto-accept
    // Suggest-level file edits for write-capable roles. Shell / network / MCP
    // still require parent auto-approve (or fail closed).
    runtime.accept_edits = true;
    let (result, mut metadata) = spawn_subagent_from_input(input, manager, runtime).await?;
    // Prefer the identity values the driver stamped; fall back to task options.
    let workflow_task_label = identity
        .workflow_task_label
        .filter(|label| !label.trim().is_empty())
        .or(request_label);
    let workflow_phase_id = identity
        .workflow_phase_id
        .filter(|phase| !phase.trim().is_empty())
        .or(request_phase);
    metadata.workflow_run_id = Some(identity.workflow_run_id);
    metadata.workflow_phase_id = workflow_phase_id;
    metadata.workflow_task_label = workflow_task_label;
    metadata.workflow_child_index = Some(identity.workflow_child_index);
    Ok(WorkflowTaskSpawnResult { result, metadata })
}

/// Check the spawn input against the fingerprint on the Fleet receipt.
///
/// The fingerprint is produced by `ChildAuthority::fingerprint` and names every
/// field of the envelope. Only four of them survive into the spawn input as
/// distinct keys — `write_authority`, `max_depth`, `allowed_tools`,
/// `disallowed_tools` — and those are exactly the four this function can and
/// does verify. The remaining fields (`tools`, `network`, `shell`, `posture`)
/// are *derivations* the Fleet used to compute those four, so a divergence in
/// any of them shows up in one of the four; verifying the wire form is
/// therefore the stronger check, not the weaker one, because it is the value
/// the child is actually constructed from.
///
/// Fails closed in every ambiguous case: an unparseable fingerprint is a
/// refusal, not a pass.
fn verify_fleet_authority_input(expected: &str, input: &Value) -> Result<()> {
    let fields: std::collections::HashMap<&str, &str> = expected
        .split(';')
        .filter_map(|part| part.split_once('='))
        .collect();
    if !expected.starts_with("v1;") || fields.len() < 8 {
        return Err(anyhow!(
            "Fleet authority fingerprint `{expected}` is not a form this build understands; \
             refusing the spawn rather than launching an unverified child"
        ));
    }

    let listed = |key: &str| -> String {
        let mut values: Vec<String> = input
            .get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        values.sort();
        values.dedup();
        values.join(",")
    };

    let actual_allow = match input.get("allowed_tools") {
        None | Some(Value::Null) => "inherit".to_string(),
        Some(Value::Array(list)) if list.is_empty() => "none".to_string(),
        Some(_) => listed("allowed_tools"),
    };
    let actual_write = input
        .get("write_authority")
        .and_then(Value::as_str)
        .unwrap_or("read_only");
    let actual_depth = input
        .get("max_depth")
        .and_then(Value::as_u64)
        .map(|depth| depth.to_string())
        .unwrap_or_default();

    for (key, actual) in [
        ("write", actual_write.to_string()),
        ("depth", actual_depth),
        ("allow", actual_allow),
        ("deny", listed("disallowed_tools")),
    ] {
        let expected_value = fields.get(key).copied().unwrap_or_default();
        if expected_value != actual {
            return Err(anyhow!(
                "Fleet authority mismatch at the spawn boundary: the receipt names {key}=`{expected_value}` \
                 but the child would be constructed with `{actual}`. Refusing the spawn — a Fleet \
                 ceiling that does not reach the runtime is not a ceiling."
            ));
        }
    }
    Ok(())
}

// === Sub-agent Execution ===

/// Build the system prompt for a sub-agent.
///
/// Starts with the per-type prompt (`FleetRole::system_prompt`) and
/// appends a one-line role overlay when `assignment.role` is set. The
/// full role library — TOML overlays from `~/.deepseek/roles/`, the
/// `/roles` slash command, model overrides per role — lands in 0.6.7.
/// For 0.6.6 we just don't drop the role on the floor: the model sees
/// "You are operating in the role of `{name}`." as a final line so its
/// behavior reflects the user's choice.
fn build_subagent_system_prompt(agent_type: &FleetRole, assignment: &SubAgentAssignment) -> String {
    let base = agent_type.system_prompt();
    let mut prompt = match assignment.role.as_deref() {
        Some(role) if !role.trim().is_empty() => {
            format!(
                "{base}\n\nYou are operating in the role of `{}`.",
                role.trim()
            )
        }
        _ => base,
    };
    // Sub-agents are background workers: the orchestrating agent is their only
    // caller. They never talk to the end user.
    prompt.push_str(
        "\n\nYou are a background sub-agent: every instruction comes from the orchestrating agent, not a human. Never address the end user or ask them questions — do the assigned work and report results back to the orchestrator.",
    );
    // C1: write-capable children must return PASS/FAIL evidence, not only a diff.
    if write_capable_child_needs_verify_contract(agent_type) {
        prompt.push_str(WRITE_CHILD_VERIFY_CONTRACT);
    }
    prompt
}

/// True when the child role is expected to mutate the workspace and therefore
/// must end with structured verify evidence (Operate Phase 1 / C1).
///
/// Scout/planner/reviewer/verifier stay read-oriented; builder/custom/worker
/// are write-capable defaults. Explicit read-only spawn authority is enforced
/// separately in the tool registry — the prompt still asks write-role children
/// for evidence when they did edit.
fn write_capable_child_needs_verify_contract(agent_type: &FleetRole) -> bool {
    matches!(
        agent_type,
        FleetRole::Builder | FleetRole::Custom | FleetRole::Worker
    )
}

fn build_subagent_system_prompt_with_skills(
    agent_type: &FleetRole,
    assignment: &SubAgentAssignment,
    context: &ToolContext,
) -> String {
    let mut prompt = build_subagent_system_prompt(agent_type, assignment);
    let catalog = subagent_skill_catalog(context);
    if !catalog.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(&catalog);
    }
    prompt
}

/// Render the same workspace/plugin-qualified Skill authority available to
/// `load_skill`, without leaking a mutable source or staged filesystem path.
/// Every fresh and nested child derives this from its inherited ToolContext;
/// forked children receive it at system precedence as well.
fn subagent_skill_catalog(context: &ToolContext) -> String {
    let mode =
        crate::skills::SkillDiscoveryMode::from_codewhale_only(context.skills_scan_codewhale_only);
    let registry = context
        .skills_dir
        .as_deref()
        .map_or_else(
            || {
                crate::skills::discover_in_workspace_with_mode_and_plugins(
                    &context.workspace,
                    mode,
                    context.plugin_registry.as_deref(),
                )
            },
            |skills_dir| {
                crate::skills::discover_for_workspace_and_dir_with_mode_and_plugins(
                    &context.workspace,
                    skills_dir,
                    mode,
                    context.plugin_registry.as_deref(),
                )
            },
        )
        .into_enabled();
    if registry.list().is_empty() {
        return String::new();
    }
    let mut output = String::from(
        "## Skills\n\nUse `load_skill` with an exact name before applying a Skill. Catalog entries are workspace-scoped snapshots; plugin entries are revalidated at use.\n",
    );
    for skill in registry.list() {
        let source = match &skill.source {
            crate::skills::SkillSource::Native => "native workspace catalog".to_string(),
            crate::skills::SkillSource::Plugin {
                plugin_id,
                plugin_name,
                authority,
            } => format!(
                "reviewed plugin {plugin_name} id={plugin_id} generation={} content={}",
                authority.state_generation,
                &authority.content_hash[..authority.content_hash.len().min(12)]
            ),
        };
        use std::fmt::Write as _;
        let _ = writeln!(
            output,
            "- `{}`: {} ({source})",
            skill.name,
            skill.description.replace(['\n', '\r'], " ")
        );
    }
    output
}

fn subagent_request_system_prompt(subagent_system_prompt: &str) -> SystemPrompt {
    // Forking inherits conversation context, not the parent's identity. A
    // child can have a different provider/model/profile, so its own resolved
    // role prompt must stay at system precedence.
    SystemPrompt::Text(subagent_system_prompt.to_string())
}

#[cfg(test)]
fn build_initial_subagent_messages(
    prompt: &str,
    assignment: &SubAgentAssignment,
    agent_type: &FleetRole,
    fork_context: Option<&SubAgentForkContext>,
) -> Vec<Message> {
    let system_prompt = build_subagent_system_prompt(agent_type, assignment);
    build_initial_subagent_messages_with_system(
        prompt,
        assignment,
        agent_type,
        &system_prompt,
        fork_context,
    )
}

fn build_initial_subagent_messages_with_system(
    prompt: &str,
    assignment: &SubAgentAssignment,
    agent_type: &FleetRole,
    subagent_system_prompt: &str,
    fork_context: Option<&SubAgentForkContext>,
) -> Vec<Message> {
    let mut messages = fork_context
        .map(|context| context.messages.clone())
        .unwrap_or_default();

    if let Some(context) = fork_context {
        if let Some(state) = context
            .structured_state_block
            .as_deref()
            .map(str::trim)
            .filter(|state| !state.is_empty())
        {
            messages.push(system_text_message(format!(
                "<codewhale:fork_state>\n{state}\n</codewhale:fork_state>"
            )));
        }

        messages.push(system_text_message(format!(
            "<codewhale:subagent_context>\n{}\n</codewhale:subagent_context>",
            subagent_system_prompt
        )));
    }

    messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: build_assignment_prompt(prompt, assignment, agent_type),
            cache_control: None,
        }],
    });

    messages
}

/// Whether an agent's current To-do snapshot is worth publishing to the
/// mailbox (#4810).
///
/// Two silences are deliberate: an unchanged list is not news, and an agent
/// that has never stated any work says nothing at all rather than announcing
/// an empty list. A list that goes from non-empty to empty *is* published —
/// that is a real transition the card must reflect instead of showing stale
/// items.
fn work_state_worth_publishing(
    last_published: Option<&crate::tools::todo::TodoListSnapshot>,
    current: &crate::tools::todo::TodoListSnapshot,
) -> bool {
    match last_published {
        Some(last) => last != current,
        None => !current.is_empty(),
    }
}

fn system_text_message(text: String) -> Message {
    Message {
        role: Role::System,
        content: vec![ContentBlock::Text {
            text,
            cache_control: None,
        }],
    }
}

struct SubAgentTask {
    manager_handle: SharedSubAgentManager,
    runtime: SubAgentRuntime,
    agent_id: String,
    agent_type: FleetRole,
    prompt: String,
    assignment: SubAgentAssignment,
    /// `None` = full registry inheritance. `Some(list)` = explicit narrow.
    /// Approval-gated tools still require an auto-approved parent runtime.
    allowed_tools: Option<Vec<String>>,
    fork_context: bool,
    started_at: Instant,
    max_steps: u32,
    /// Per-worker token cap sourced from the spawn request's `token_budget`
    /// (the explicit `max_tokens`/`tokenBudget` override). `None` means no
    /// per-worker limit; the worker still obeys the scope admission gate.
    /// When set, the worker stops with `BudgetExhausted` once its accumulated
    /// model tokens exceed this value. Independent of the scope budget (#3319).
    token_budget: Option<u64>,
    /// Hard wall-clock deadline for the whole child run.
    wall_time: Duration,
    input_rx: mpsc::UnboundedReceiver<SubAgentInput>,
    /// Interactive launch gate (#3095). `Some` only for direct (depth-1)
    /// children: the task acquires a permit before its first model step and
    /// holds it until completion, so a fanout burst beyond the limit queues
    /// with a visible reason instead of executing all at once.
    launch_gate: Option<Arc<Semaphore>>,
    /// Releases the parent turn's settlement barrier after this turn-owned
    /// child or descendant has completed its terminal fan-in.
    _foreground_child_registration: Option<ForegroundChildRegistration>,
}

/// Spawn the child task body under a panic guard.
///
/// Children run under `spawn_supervised`, which catch_unwinds a panic so the
/// parent process survives — and that used to be where the story ended: the
/// terminal commit at the end of `run_subagent_task_inner` never ran, so the
/// panicked child stayed `Running` and its write claim kept gating live peers
/// until heartbeat auto-cancel (indefinitely, while a lingering shell kept
/// the heartbeat fresh). The guard catches the panic first, commits a crash
/// terminal result through the same `finish_terminal_result` arbitration every
/// other terminal path uses, then resumes unwinding so the supervisor still
/// logs the panic and writes its crash dump.
async fn run_subagent_task(task: SubAgentTask) {
    let agent_id = task.agent_id.clone();
    let manager_handle = task.manager_handle.clone();
    supervise_subagent_task_body(manager_handle, agent_id, run_subagent_task_inner(task)).await;
}

async fn supervise_subagent_task_body(
    manager_handle: SharedSubAgentManager,
    agent_id: String,
    body: impl std::future::Future<Output = ()> + Send,
) {
    use futures_util::FutureExt;
    let panicked = std::panic::AssertUnwindSafe(body).catch_unwind().await;
    let Err(panic) = panicked else {
        return;
    };
    let message = crate::utils::panic_message(&*panic);
    {
        let mut manager = manager_handle.write().await;
        match manager.get_result(&agent_id) {
            Ok(mut result) => {
                result.status =
                    SubAgentStatus::Failed(format!("sub-agent task panicked: {message}"));
                result.result = None;
                result.needs_input = None;
                // Arbitrated exactly like the natural terminal commit: when a
                // cancel or another terminal outcome already won, this is a
                // no-op rather than a second result.
                manager.finish_terminal_result(&agent_id, result, false, true);
            }
            Err(err) => {
                tracing::error!(
                    target: "subagent",
                    agent_id = %agent_id,
                    ?err,
                    "panicked task no longer has a manager record"
                );
            }
        }
    }
    std::panic::resume_unwind(panic);
}

#[allow(clippy::too_many_lines)]
async fn run_subagent_task_inner(task: SubAgentTask) {
    // `spawn_background_with_assignment_options` installs this before the task
    // is scheduled. Keep this fallback for internal/test task launchers so a
    // manually-created worker still owns the same terminal fan-in contract.
    {
        let delivery = SubAgentTerminalDeliveryContext::from_runtime(&task.runtime);
        let mut manager = task.manager_handle.write().await;
        if let Some(agent) = manager.agents.get_mut(&task.agent_id)
            && agent.status == SubAgentStatus::Running
            && !agent.completion_claimed
            && agent.terminal_delivery.is_none()
        {
            agent.owner_session_id = delivery.session_id.clone();
            agent.terminal_delivery = Some(delivery);
        }
    }

    let deadline = task.started_at + task.wall_time;

    // Interactive launch gate (#3095): direct children acquire a permit
    // before their first model step so a fanout burst beyond the limit
    // queues visibly instead of executing all at once. The permit is held
    // for the lifetime of the task. The permit wait shares the authored child
    // deadline with model/tool work, so saturation cannot extend the whole
    // child beyond its wall-time budget. Cancellation while queued is handled
    // by `run_subagent` before it emits Started/Starting.
    let mut _launch_permit = None;
    let mut launch_wait_timed_out = false;
    if let Some(gate) = task.launch_gate.as_ref() {
        match Arc::clone(gate).try_acquire_owned() {
            Ok(permit) => _launch_permit = Some(permit),
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                match tokio::time::timeout_at(
                    deadline.into(),
                    acquire_queued_launch_permit(&task, Arc::clone(gate)),
                )
                .await
                {
                    Ok(permit) => _launch_permit = permit,
                    Err(_) => launch_wait_timed_out = true,
                }
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                crate::logging::warn(format!(
                    "sub-agent launch gate closed for {}; proceeding without backpressure",
                    task.agent_id
                ));
            }
        }
    }

    let turn_end_parking = task
        ._foreground_child_registration
        .as_ref()
        .map(ForegroundChildRegistration::parking_signal);
    let result = if launch_wait_timed_out {
        Err(anyhow!(child_wall_time_exhausted_reason(task.wall_time)))
    } else {
        tokio::time::timeout_at(
            deadline.into(),
            run_subagent(
                &task.runtime,
                task.agent_id.clone(),
                task.agent_type,
                task.prompt,
                task.assignment,
                task.allowed_tools,
                task.fork_context,
                task.started_at,
                task.max_steps,
                task.token_budget,
                turn_end_parking,
                task.input_rx,
            ),
        )
        .await
        .unwrap_or_else(|_| Err(anyhow!(child_wall_time_exhausted_reason(task.wall_time))))
    };

    let agent_id = task.agent_id.clone();
    let failure_error = result.as_ref().err().map(|err| {
        crate::logging::warn(format!(
            "sub-agent {} model request failed: {err:#}",
            task.agent_id
        ));
        annotate_child_model_error(
            &subagent_failure_message(err),
            &task.runtime.model,
            task.runtime.client.api_provider(),
            &task.runtime.worker_profile.model,
        )
    });

    // Every terminal path — successful/fatal model exit, explicit Stop,
    // coordination interrupt, and stale cleanup — arbitrates and publishes
    // through `finish_terminal_result`. Cancellation that already won leaves
    // this late epilogue with no claim and therefore no duplicate fan-in.
    let terminal_committed = {
        let mut manager = task.manager_handle.write().await;
        let terminal = match result {
            Ok(result) => result,
            Err(_) => {
                let mut result = match manager.get_result(&agent_id) {
                    Ok(result) => result,
                    Err(err) => {
                        tracing::error!(
                            target: "subagent",
                            agent_id = %agent_id,
                            ?err,
                            "failed task no longer has a manager record"
                        );
                        return;
                    }
                };
                result.status = SubAgentStatus::Failed(
                    failure_error
                        .clone()
                        .expect("failed task should carry annotated error"),
                );
                result.result = None;
                result.needs_input = None;
                result
            }
        };
        manager.finish_terminal_result(&agent_id, terminal, false, true)
    };
    if !terminal_committed {
        tracing::debug!(
            target: "subagent",
            agent_id = %agent_id,
            "suppressing late task completion after another terminal outcome won"
        );
    }
}

async fn acquire_queued_launch_permit(
    task: &SubAgentTask,
    gate: Arc<Semaphore>,
) -> Option<tokio::sync::OwnedSemaphorePermit> {
    record_queued_launch_progress(task).await;
    tokio::select! {
        biased;
        () = task.runtime.cancel_token.cancelled() => {
            None
        }
        permit = Arc::clone(&gate).acquire_owned() => {
            permit.ok()
        }
    }
}

async fn record_queued_launch_progress(task: &SubAgentTask) {
    {
        let mut manager = task.runtime.manager.write().await;
        manager.touch(&task.agent_id);
        manager.record_worker_event(
            &task.agent_id,
            AgentWorkerStatus::Queued,
            Some(SUBAGENT_QUEUED_LAUNCH_REASON.to_string()),
            None,
            None,
        );
    }
    emit_agent_progress(
        task.runtime.event_tx.as_ref(),
        &task.runtime.context.state_namespace,
        &task.agent_id,
        SUBAGENT_QUEUED_LAUNCH_REASON.to_string(),
        AgentProgressEventMeta::new(AgentWorkerStatus::Queued),
        task.runtime.parent_agent_id.clone(),
        task.runtime.spawn_depth,
    );
    if let Some(mailbox) = task.runtime.mailbox.as_ref() {
        let _ = mailbox.send(MailboxMessage::progress(
            &task.agent_id,
            SUBAGENT_QUEUED_LAUNCH_REASON,
        ));
    }
}

/// Notify this runtime's immediate parent that the child finished (issue
/// #756). Root-spawned children send to the engine turn loop. Nested children
/// send to the parent sub-agent's local inbox, which is swapped into the
/// runtime used by that parent's `agent` tool. Returns `true` if a send was
/// attempted, `false` if this is the engine itself or no channel is wired.
/// Skips silently when the channel sender has no receiver — the receiver may
/// have ended because the parent turn/agent already completed.
#[cfg(test)]
pub(crate) fn emit_parent_completion(
    runtime: &SubAgentRuntime,
    agent_id: &str,
    payload: &str,
) -> bool {
    if runtime.spawn_depth == 0 {
        return false;
    }
    let Some(tx) = runtime.parent_completion_tx.as_ref() else {
        return false;
    };
    let _ = tx.send(SubAgentCompletion {
        owner_session_id: runtime.context.state_namespace.clone(),
        agent_id: agent_id.to_string(),
        payload: payload.to_string(),
    });
    true
}

#[cfg(test)]
pub(crate) fn subagent_completion_from_result(result: &SubAgentResult) -> SubAgentCompletion {
    subagent_completion_from_result_with_ref(result, None)
}

/// Completion builder that names the persisted full report in the truncation
/// footer when `report_ref` is available; see `spill_subagent_final_report`.
#[cfg(test)]
pub(crate) fn subagent_completion_from_result_with_ref(
    result: &SubAgentResult,
    report_ref: Option<&str>,
) -> SubAgentCompletion {
    subagent_completion_from_result_with_ref_for_session("", result, report_ref)
}

/// Session-owned completion builder used by live delivery and turn synthesis.
/// An empty owner is reserved for legacy test helpers and is rejected by the
/// session-aware engine claim path.
pub(crate) fn subagent_completion_from_result_with_ref_for_session(
    owner_session_id: &str,
    result: &SubAgentResult,
    report_ref: Option<&str>,
) -> SubAgentCompletion {
    let raw = summarize_subagent_result(result);
    let mut evidence_truncated = false;
    let evidence_block = match &result.status {
        SubAgentStatus::Failed(_)
        | SubAgentStatus::BudgetExhausted
        | SubAgentStatus::Cancelled
        | SubAgentStatus::Interrupted(_) => None,
        _ => result
            .result
            .as_deref()
            .and_then(extract_evidence_block)
            .map(|block| {
                let (clipped, ev_trunc) = clip_evidence_block(&block);
                evidence_truncated = ev_trunc;
                clipped
            })
            .filter(|evidence| !evidence.trim().is_empty()),
    };
    let summary_source = evidence_block
        .as_ref()
        .map(|_| strip_evidence_block(&raw))
        .unwrap_or(raw);
    let (summary, truncated) = stamp_subagent_summary_with_ref(&summary_source, report_ref);
    let summary_truncated = truncated || evidence_truncated;
    let sentinel = match &result.status {
        SubAgentStatus::Failed(error) => subagent_failed_sentinel(result, error),
        SubAgentStatus::BudgetExhausted => {
            subagent_failed_sentinel(result, "child token budget exhausted")
        }
        _ => subagent_done_sentinel(&result.agent_id, result, summary_truncated),
    };
    let payload = match evidence_block {
        Some(evidence) => format!("{summary}\n{evidence}\n{sentinel}"),
        None => format!("{summary}\n{sentinel}"),
    };
    SubAgentCompletion {
        owner_session_id: owner_session_id.to_string(),
        agent_id: result.agent_id.clone(),
        payload,
    }
}

const SUBAGENT_EVIDENCE_CHAR_BUDGET: usize = 4_000;

fn clip_evidence_block(block: &str) -> (String, bool) {
    let total = block.chars().count();
    if total <= SUBAGENT_EVIDENCE_CHAR_BUDGET {
        return (block.to_string(), false);
    }
    let clipped: String = block.chars().take(SUBAGENT_EVIDENCE_CHAR_BUDGET).collect();
    (format!("{clipped}…"), true)
}

fn extract_evidence_block(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let markers = ["### evidence", "## evidence", "evidence:"];
    for marker in markers {
        let Some(start) = lower.find(marker) else {
            continue;
        };
        let block = &text[start..];
        let tail = &block[marker.len()..];
        let end = tail
            .find("\n### ")
            .or_else(|| tail.find("\n## "))
            .or_else(|| tail.to_ascii_lowercase().find("\ngaps"))
            .or_else(|| tail.to_ascii_lowercase().find("\nnext"))
            .unwrap_or(tail.len());
        let extracted = format!("{}{}", &block[..marker.len()], &tail[..end])
            .trim()
            .to_string();
        if !extracted.is_empty() {
            return Some(extracted);
        }
    }
    None
}

fn strip_evidence_block(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let markers = ["### evidence", "## evidence", "evidence:"];
    for marker in markers {
        let Some(start) = lower.find(marker) else {
            continue;
        };
        let block = &text[start..];
        let tail = &block[marker.len()..];
        let end = tail
            .find("\n### ")
            .or_else(|| tail.find("\n## "))
            .or_else(|| tail.to_ascii_lowercase().find("\ngaps"))
            .or_else(|| tail.to_ascii_lowercase().find("\nnext"))
            .unwrap_or(tail.len());
        let mut without = format!("{}{}", &text[..start], &block[marker.len() + end..]);
        without = without.trim().to_string();
        return without;
    }
    text.trim().to_string()
}

/// Build a `<codewhale:subagent.done>` JSON sentinel for a successful child.
/// Intended to surface in the parent's transcript so the model recognizes
/// child completion.
///
/// Keep this payload deliberately lean. The human summary is emitted on the
/// line immediately before the sentinel; duplicating it here bloats the next
/// parent request's cache-miss tail. Wall-clock duration is useful UI
/// telemetry, but it is volatile and not useful for model coordination.
///
/// `truncated` reflects whether the previous-line summary was length-gated by
/// `stamp_subagent_summary` (issue #2652); it surfaces as `summary_kind` so
/// the parent model can tell a complete self-report from a clipped one and
/// verify material claims accordingly.
fn subagent_done_sentinel(agent_id: &str, res: &SubAgentResult, truncated: bool) -> String {
    let mut payload = json!({
        "agent_id": agent_id,
        // Whale name — a stable, human-friendly handle the orchestrator can use
        // to refer to this child in its own reasoning/output.
        "name": res.nickname,
        "agent_type": res.agent_type.as_str(),
        "status": subagent_status_name(&res.status),
        "summary_location": "previous_line",
        // issue #2652: lets the parent branch on whether the previous-line
        // summary is the full child report or a head+tail excerpt.
        "summary_kind": if truncated { "truncated" } else { "complete" },
    });
    if let Some(needs_input) = res.needs_input.clone() {
        payload["needs_input"] = json!(needs_input);
    }
    if let Some(child_route) = res.child_route.clone() {
        payload["child_route"] = json!(child_route);
    }
    format!("<codewhale:subagent.done>{payload}</codewhale:subagent.done>")
}

fn subagent_failure_class(status: &SubAgentStatus, error: &str) -> &'static str {
    if matches!(status, SubAgentStatus::BudgetExhausted) {
        return "token_budget";
    }
    let error = error.to_ascii_lowercase();
    if error.contains("no assistant text") || error.contains("without returning a final summary") {
        "empty_turn"
    } else if error.contains("step budget exhausted") {
        "step_budget"
    } else if error.contains("wall-time budget exhausted") {
        "wall_time_budget"
    } else if error.contains("authorization failed")
        || error.contains("usage limit")
        || error.contains("quota")
    {
        "auth_or_quota"
    } else if error.contains("timed out") || error.contains("timeout") {
        "timeout"
    } else {
        "runtime_error"
    }
}

/// Build the distinct high-priority failure event carried to the owning
/// parent. The human-readable, already-sanitized reason stays on the previous
/// line; this sentinel carries only bounded routing and recovery metadata.
fn subagent_failed_sentinel(res: &SubAgentResult, error: &str) -> String {
    let transcript_handle = format!("agent:{}/full_transcript", res.agent_id);
    let payload = json!({
        "event": "subagent.failed",
        "priority": "high",
        "agent_id": res.agent_id,
        "name": res.nickname.as_deref().unwrap_or(&res.name),
        "agent_type": res.agent_type.as_str(),
        "status": subagent_status_name(&res.status),
        "failure_class": subagent_failure_class(&res.status, error),
        "steps": res.steps_taken,
        "elapsed_ms": res.duration_ms,
        "transcript_handle": transcript_handle,
        "error_location": "previous_line",
        "child_route": res.child_route,
    });
    format!("<codewhale:subagent.done>{payload}</codewhale:subagent.done>")
}

fn response_was_truncated(response: &MessageResponse) -> bool {
    is_output_limit_stop_reason(response.stop_reason.as_deref())
}

fn incomplete_subagent_response_failure(response: &MessageResponse) -> String {
    let reason = stop_reason_detail(response.stop_reason.as_deref());
    if response_was_truncated(response) {
        format!(
            "Sub-agent model output was truncated (provider stop reason `{reason}`); no partial tool call was executed and any partial text was preserved for diagnostics."
        )
    } else {
        format!(
            "Sub-agent model response was incomplete (provider stop reason `{reason}`); no partial tool call or final result was accepted, and any partial text was preserved for diagnostics."
        )
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_subagent_full_transcript_handle(
    runtime: &SubAgentRuntime,
    agent_id: &str,
    agent_type: &FleetRole,
    assignment: &SubAgentAssignment,
    status: &SubAgentStatus,
    result: Option<&String>,
    checkpoint: Option<&SubAgentCheckpoint>,
    transcript_artifact: Option<&mut SubAgentTranscriptArtifactWriter>,
    messages: &[Message],
    steps_taken: u32,
    duration_ms: u64,
    fork_context: bool,
) -> VarHandle {
    // Byte-bound the retained transcript (#3882): the handle store keeps this
    // payload resident per agent, and the checkpoint already carries its own
    // bounded message tail — embedding it verbatim would duplicate that tail
    // inside one payload. Keep checkpoint metadata, drop its messages, and
    // record how much of the true history the bounded tail omits.
    let projected_messages = crate::image_attach::safe_tool_result_message_projection(messages);
    let (bounded_messages, omitted_messages) = bounded_tail_messages(
        &projected_messages,
        SUBAGENT_TRANSCRIPT_MESSAGE_BUDGET_BYTES,
    );
    let checkpoint_meta = checkpoint.map(|checkpoint| SubAgentCheckpoint {
        omitted_messages: checkpoint.message_count,
        messages: Vec::new(),
        ..checkpoint.clone()
    });
    let transcript_artifact = transcript_artifact.map(|writer| {
        let synced =
            match writer.sync_messages(&projected_messages, *status != SubAgentStatus::Running) {
                Ok(()) => true,
                Err(err) => {
                    tracing::warn!(
                        target: "subagent",
                        ?err,
                        agent_id,
                        "failed to persist complete sub-agent transcript artifact"
                    );
                    false
                }
            };
        writer.metadata(synced && writer.persisted_messages == projected_messages.len())
    });
    let payload = json!({
        "kind": "subagent_full_transcript",
        "agent_id": agent_id,
        "agent_type": agent_type.as_str(),
        "status": subagent_status_name(status),
        "context_mode": if fork_context { "forked" } else { "fresh" },
        "fork_context": fork_context,
        "result": result,
        "steps_taken": steps_taken,
        "duration_ms": duration_ms,
        "assignment": assignment,
        "checkpoint": checkpoint_meta,
        "message_count": messages.len(),
        "omitted_messages": omitted_messages,
        "messages_complete": omitted_messages == 0,
        "messages": bounded_messages,
        "complete_transcript_artifact": transcript_artifact,
    });
    let mut store = runtime.context.runtime.handle_store.lock().await;
    store.insert_json(format!("agent:{agent_id}"), "full_transcript", payload)
}

/// Publish the inspectable worker transcript while a child is still running.
///
/// The sidebar's Open action is intentionally backed by the same
/// `full_transcript` handle before and after completion. Keeping a separate
/// live-only snapshot name meant Open could only show a compact status card
/// until the worker stopped, which is exactly when observing it is least
/// useful.
#[allow(clippy::too_many_arguments)]
async fn publish_live_subagent_transcript(
    runtime: &SubAgentRuntime,
    agent_id: &str,
    agent_type: &FleetRole,
    assignment: &SubAgentAssignment,
    result: Option<&String>,
    checkpoint: Option<&SubAgentCheckpoint>,
    transcript_artifact: Option<&mut SubAgentTranscriptArtifactWriter>,
    messages: &[Message],
    steps_taken: u32,
    started_at: Instant,
    fork_context: bool,
) {
    let duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
    insert_subagent_full_transcript_handle(
        runtime,
        agent_id,
        agent_type,
        assignment,
        &SubAgentStatus::Running,
        result,
        checkpoint,
        transcript_artifact,
        messages,
        steps_taken,
        duration_ms,
        fork_context,
    )
    .await;
}

/// Bound a sub-agent tool result before it enters `messages` (#3882).
///
/// The root engine applies spillover in `turn_loop.rs`; the sub-agent loop
/// bypassed it, so one multi-MB build log became many resident copies across
/// child messages, checkpoints, transcript handles, and persistence — the
/// Fleet fanout memory blow-up. Over-threshold content (successes AND
/// errors: sub-agent error output is routinely a full build log, so the root
/// loop's pass-errors-through rationale does not hold here) is written to the
/// shared spillover directory and replaced inline by a bounded head plus a
/// footer naming the on-disk path.
///
/// Returns the (possibly bounded) content and the spillover path when one was
/// written. Spillover write failures degrade to passing the original content
/// through, mirroring `apply_spillover`.
fn bound_subagent_tool_result(
    agent_id: &str,
    tool_id: &str,
    tool_name: &str,
    session_id: &str,
    success: bool,
    content: String,
) -> (String, Option<PathBuf>) {
    let spill_id = format!("sa_{agent_id}_{tool_id}");
    let mut result = if success {
        ToolResult::success(content)
    } else {
        ToolResult::error(content)
    };
    let path = crate::tools::truncate::apply_spillover_with_artifact(
        &mut result,
        &spill_id,
        tool_name,
        session_id,
    );
    (result.content, path)
}

/// Rough serialized size of one message, used for checkpoint/transcript byte
/// budgets. Exact JSON size via serde; unserializable messages (should not
/// happen) count as 1 KiB so they still consume budget.
fn approximate_message_bytes(message: &Message) -> usize {
    serde_json::to_string(message).map_or(1024, |s| s.len())
}

/// Keep the most recent messages whose combined approximate size fits
/// `budget_bytes`. Always keeps at least the final message (even if it alone
/// exceeds the budget) so a non-empty history stays continuable. Returns the
/// retained tail and how many older messages were omitted.
fn bounded_tail_messages(messages: &[Message], budget_bytes: usize) -> (Vec<Message>, usize) {
    let mut kept_rev: Vec<Message> = Vec::new();
    let mut used = 0usize;
    for message in messages.iter().rev() {
        let size = approximate_message_bytes(message);
        if !kept_rev.is_empty() && used.saturating_add(size) > budget_bytes {
            break;
        }
        used = used.saturating_add(size);
        kept_rev.push(message.clone());
    }
    kept_rev.reverse();
    let omitted = messages.len().saturating_sub(kept_rev.len());
    (kept_rev, omitted)
}

fn build_subagent_checkpoint(
    agent_id: &str,
    reason: impl Into<String>,
    messages: &[Message],
    steps_taken: u32,
    continuable: bool,
) -> SubAgentCheckpoint {
    let created_at_ms = epoch_millis_now();
    let checkpoint_id = format!("{agent_id}:step:{steps_taken}:ts:{created_at_ms}");
    let projected_messages = crate::image_attach::safe_tool_result_message_projection(messages);
    let (bounded_messages, omitted_messages) = bounded_tail_messages(
        &projected_messages,
        SUBAGENT_CHECKPOINT_MESSAGE_BUDGET_BYTES,
    );
    SubAgentCheckpoint {
        checkpoint_id: checkpoint_id.clone(),
        agent_id: agent_id.to_string(),
        continuation_handle: format!("agent:{agent_id}:checkpoint:{checkpoint_id}"),
        reason: reason.into(),
        continuable,
        steps_taken,
        message_count: messages.len(),
        created_at_ms,
        messages: bounded_messages,
        omitted_messages,
    }
}

async fn checkpoint_subagent_progress(
    runtime: &SubAgentRuntime,
    agent_id: &str,
    reason: impl Into<String>,
    messages: &[Message],
    steps_taken: u32,
    continuable: bool,
) -> SubAgentCheckpoint {
    let checkpoint =
        build_subagent_checkpoint(agent_id, reason, messages, steps_taken, continuable);
    let mut manager = runtime.manager.write().await;
    manager.update_checkpoint(agent_id, checkpoint.clone());
    checkpoint
}

/// Render a checkpoint's message tail as readable prose for re-seeding a
/// resumed session. Text and tool blocks are shown; images and internal
/// thinking blocks are summarized/omitted (thinking was already consumed by
/// the interrupted model step).
fn checkpoint_messages_to_text(messages: &[Message]) -> String {
    let mut lines = Vec::new();
    for message in messages {
        let mut parts: Vec<String> = Vec::new();
        for block in &message.content {
            match block {
                ContentBlock::Text { text, .. } => parts.push(text.clone()),
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => parts.push(format!("[tool use {name} (id {id}): {input}]")),
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    ..
                } => {
                    let verdict = if is_error == &Some(true) {
                        "error"
                    } else {
                        "ok"
                    };
                    parts.push(format!(
                        "[tool result {tool_use_id} ({verdict}): {content}]"
                    ))
                }
                ContentBlock::ServerToolUse {
                    id, name, input, ..
                } => parts.push(format!("[server tool use {name} (id {id}): {input}]")),
                ContentBlock::ToolSearchToolResult {
                    tool_use_id,
                    content,
                    ..
                } => parts.push(format!("[tool search result {tool_use_id}: {content}]")),
                ContentBlock::CodeExecutionToolResult {
                    tool_use_id,
                    content,
                    ..
                } => parts.push(format!("[code execution result {tool_use_id}: {content}]")),
                ContentBlock::ImageUrl { .. } => parts.push("[image]".to_string()),
                ContentBlock::Thinking { .. } => {}
            }
        }
        if parts.is_empty() {
            continue;
        }
        lines.push(format!("[{}]\n{}", message.role, parts.join("\n")));
    }
    lines.join("\n\n")
}

/// Build the prompt for a resumed session: the original objective plus the
/// checkpoint context tail and the parent's follow-up as the latest
/// instruction.
fn build_resume_prompt(
    original_prompt: &str,
    checkpoint: &SubAgentCheckpoint,
    followup_text: &str,
) -> String {
    format!(
        "{original_prompt}\n\n\
         [RESUMED SESSION — checkpoint {}]\n\
         This session was interrupted after {} step(s). The prior conversation tail follows; \
         continue the work from where it left off. The parent follow-up below is the latest \
         instruction.\n\n\
         --- prior conversation (tail) ---\n{}\n--- end prior conversation ---\n\n\
         Parent follow-up: {followup_text}",
        checkpoint.checkpoint_id,
        checkpoint.steps_taken,
        checkpoint_messages_to_text(&checkpoint.messages),
    )
}

fn needs_input_for_interrupted_checkpoint(
    reason: &str,
    checkpoint: &SubAgentCheckpoint,
) -> SubAgentNeedsInput {
    SubAgentNeedsInput {
        question: format!(
            "Sub-agent interrupted before completion ({reason}). Re-dispatch this worker or provide explicit follow-up using checkpoint {}.",
            checkpoint.continuation_handle
        ),
    }
}

#[derive(Debug)]
enum SubAgentApiRequestFailure {
    Fatal(anyhow::Error),
    Interrupted {
        reason: String,
        checkpoint_reason: &'static str,
    },
}

fn subagent_transient_provider_retry_delay(retry_number: u32) -> Duration {
    let multiplier = 1u32
        .checked_shl(retry_number.saturating_sub(1))
        .unwrap_or(4);
    SUBAGENT_TRANSIENT_PROVIDER_INITIAL_BACKOFF.saturating_mul(multiplier.min(4))
}

/// Deterministic exponential backoff for a timed-out per-step API call
/// (`retry_number` is 1-based): `initial_backoff`, ×2, ×4, …, capped at
/// [`SUBAGENT_API_TIMEOUT_MAX_BACKOFF`].
fn subagent_api_timeout_retry_base_delay(retry_number: u32, initial_backoff: Duration) -> Duration {
    let multiplier = 1u32
        .checked_shl(retry_number.saturating_sub(1))
        .unwrap_or(u32::MAX);
    initial_backoff
        .saturating_mul(multiplier)
        .min(SUBAGENT_API_TIMEOUT_MAX_BACKOFF)
}

/// Timeout retry backoff with ±20% jitter (UUID v4 entropy, the same idiom
/// as `llm_client::RetryConfig::delay_for_attempt`) so a fan-out of children
/// whose calls all time out together does not re-fire in lockstep.
fn subagent_api_timeout_retry_delay(retry_number: u32, initial_backoff: Duration) -> Duration {
    let base = subagent_api_timeout_retry_base_delay(retry_number, initial_backoff);
    let bytes = *Uuid::new_v4().as_bytes();
    let sample = u16::from_le_bytes([bytes[0], bytes[1]]);
    let random_factor = f64::from(sample) / f64::from(u16::MAX); // 0.0 to 1.0
    let jitter = base.as_secs_f64()
        * SUBAGENT_API_TIMEOUT_BACKOFF_JITTER_FACTOR
        * (2.0 * random_factor - 1.0); // -20% to +20%
    Duration::from_secs_f64((base.as_secs_f64() + jitter).max(0.0))
}

#[derive(Debug, Clone, Copy)]
struct RetryableSubAgentProviderFailure {
    label: &'static str,
    checkpoint_reason: &'static str,
    delay: Duration,
}

fn retryable_subagent_provider_failure(
    error: &anyhow::Error,
    retry_number: u32,
) -> Option<RetryableSubAgentProviderFailure> {
    if let Some(LlmError::RateLimited { retry_after, .. }) = error.downcast_ref::<LlmError>() {
        return Some(RetryableSubAgentProviderFailure {
            label: "rate-limited provider response",
            checkpoint_reason: "api_rate_limited",
            delay: retry_after
                .unwrap_or_else(|| subagent_transient_provider_retry_delay(retry_number)),
        });
    }

    if is_transient_subagent_provider_error(error) {
        return Some(RetryableSubAgentProviderFailure {
            label: "transient provider failure",
            checkpoint_reason: "api_transient_provider_failure",
            delay: subagent_transient_provider_retry_delay(retry_number),
        });
    }

    None
}

fn is_transient_subagent_provider_error(error: &anyhow::Error) -> bool {
    if let Some(LlmError::RateLimited { .. }) = error.downcast_ref::<LlmError>() {
        return true;
    }

    let message = format!("{error:#}").to_ascii_lowercase();
    [
        "did not receive response headers",
        "response headers",
        "stream request",
        "request timed out",
        "operation timed out",
        "deadline has elapsed",
        "connection reset",
        "connection closed",
        "connection aborted",
        "temporarily unavailable",
        "bad gateway",
        "gateway timeout",
        "service unavailable",
        "rate limited",
        "rate_limit",
        "rate_limited",
        "too many requests",
        "429",
        "502",
        "503",
        "504",
        // Body/decode failures are transport-class: the provider accepted the
        // request and died mid-response (a DeepSeek stream decode error killed
        // a 141s scout with zero retries — morning-report issue #7). One
        // same-prompt retry is cheap next to re-planning the child.
        "error decoding response body",
        "failed to read chat api response body",
        "failed to parse chat api json",
        "unexpected end of file",
        "incomplete message",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

async fn request_subagent_model_response_with_retries(
    runtime: &SubAgentRuntime,
    agent_id: &str,
    steps: u32,
    max_steps: u32,
    request: MessageRequest,
) -> std::result::Result<
    (MessageResponse, crate::cost_status::EffectiveRouteEnvelope),
    SubAgentApiRequestFailure,
> {
    let mut transient_failures = 0u32;
    let mut timeout_failures = 0u32;

    loop {
        // Billing time is the immutable wire-dispatch boundary, not worker
        // start/checkpoint time. A retry gets its own current route envelope.
        let usage_route = runtime
            .client
            .effective_route_envelope(&runtime.model, chrono::Utc::now());
        match tokio::time::timeout(
            runtime.step_api_timeout,
            runtime.client.create_message(request.clone()),
        )
        .await
        {
            Ok(Ok(response)) => return Ok((response, usage_route)),
            Ok(Err(err)) => {
                let retry_number = transient_failures.saturating_add(1);
                let Some(retryable) = retryable_subagent_provider_failure(&err, retry_number)
                else {
                    return Err(SubAgentApiRequestFailure::Fatal(err));
                };

                if transient_failures >= SUBAGENT_TRANSIENT_PROVIDER_MAX_RETRIES {
                    let attempts = transient_failures.saturating_add(1);
                    return Err(SubAgentApiRequestFailure::Interrupted {
                        reason: format!(
                            "{} after {attempts} API attempt(s): {err}; checkpoint preserved for continuation",
                            retryable.label
                        ),
                        checkpoint_reason: retryable.checkpoint_reason,
                    });
                }

                transient_failures = transient_failures.saturating_add(1);
                let delay = retryable.delay;
                record_agent_progress(
                    runtime,
                    agent_id,
                    AgentProgressEventMeta::new(AgentWorkerStatus::ModelWait).with_step(steps),
                    format!(
                        "{}: {}; retrying API request {}/{} in {}ms ({err})",
                        format_step_counter(steps, max_steps),
                        retryable.label,
                        transient_failures,
                        SUBAGENT_TRANSIENT_PROVIDER_MAX_RETRIES,
                        delay.as_millis(),
                    ),
                );
                tokio::time::sleep(delay).await;
            }
            Err(_) => {
                // A wall-clock timeout is usually a live-but-slow provider
                // call, not a dead one: retry with backoff like the transient
                // path before giving up the step (FINISH-0.9.4 entry #40).
                if timeout_failures >= SUBAGENT_API_TIMEOUT_MAX_RETRIES {
                    let attempts = timeout_failures.saturating_add(1);
                    return Err(SubAgentApiRequestFailure::Interrupted {
                        reason: format!(
                            "API call timed out after {}ms on {attempts} API attempt(s); checkpoint preserved for continuation",
                            runtime.step_api_timeout.as_millis()
                        ),
                        checkpoint_reason: "api_timeout",
                    });
                }

                timeout_failures = timeout_failures.saturating_add(1);
                let delay = subagent_api_timeout_retry_delay(
                    timeout_failures,
                    runtime.api_timeout_retry_base_backoff,
                );
                record_agent_progress(
                    runtime,
                    agent_id,
                    AgentProgressEventMeta::new(AgentWorkerStatus::ModelWait).with_step(steps),
                    format!(
                        "{}: API call timed out after {}ms; retrying API request {}/{} in {}ms",
                        format_step_counter(steps, max_steps),
                        runtime.step_api_timeout.as_millis(),
                        timeout_failures,
                        SUBAGENT_API_TIMEOUT_MAX_RETRIES,
                        delay.as_millis(),
                    ),
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
}

fn record_agent_progress(
    runtime: &SubAgentRuntime,
    agent_id: &str,
    activity: AgentProgressEventMeta,
    message: impl Into<String>,
) {
    let message = message.into();
    if let Ok(mut manager) = runtime.manager.try_write() {
        manager.touch(agent_id);
        manager.record_worker_event(
            agent_id,
            activity.worker_status,
            Some(message.clone()),
            activity.step,
            activity.tool_name.clone(),
        );
    }
    emit_agent_progress(
        runtime.event_tx.as_ref(),
        &runtime.context.state_namespace,
        agent_id,
        message,
        activity,
        runtime.parent_agent_id.clone(),
        runtime.spawn_depth,
    );
}

fn runtime_for_nested_agent_tools(
    runtime: &SubAgentRuntime,
    parent_agent_id: &str,
    fork_context: SubAgentForkContext,
) -> (SubAgentRuntime, mpsc::UnboundedReceiver<SubAgentCompletion>) {
    let (child_completion_tx, child_completion_rx) =
        mpsc::unbounded_channel::<SubAgentCompletion>();
    let runtime_for_tools = runtime
        .clone()
        .with_parent_completion_tx(child_completion_tx)
        .with_fork_context(fork_context);
    let runtime_for_tools = SubAgentRuntime {
        parent_agent_id: Some(parent_agent_id.to_string()),
        ..runtime_for_tools
    };
    (runtime_for_tools, child_completion_rx)
}

fn drain_child_completion_events(
    child_completion_rx: &mut mpsc::UnboundedReceiver<SubAgentCompletion>,
) -> Vec<SubAgentCompletion> {
    let mut completions = Vec::new();
    while let Ok(completion) = child_completion_rx.try_recv() {
        completions.push(completion);
    }
    completions
}

fn child_completion_runtime_message(completions: &[SubAgentCompletion]) -> Message {
    let mut text = String::from(
        "<codewhale:runtime_event kind=\"child_subagent_completion\" visibility=\"internal\">\n\
This is an internal runtime event, not user input. One or more child sub-agents \
you spawned have finished. Treat each child summary as an unverified self-report: \
if you rely on it, cite the child agent_id and the EVIDENCE lines it provided, \
and distinguish that from evidence you personally verified. A sentinel marked \
event=subagent.failed is high priority: inspect its failure_class and transcript_handle, \
then re-plan dependent work before claiming completion.\n",
    );
    for completion in completions {
        text.push_str("\n--- child sub-agent completion ---\n");
        text.push_str("agent_id: ");
        text.push_str(&completion.agent_id);
        text.push('\n');
        text.push_str(&completion.payload);
        text.push('\n');
    }
    text.push_str("</codewhale:runtime_event>");

    Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text,
            cache_control: None,
        }],
    }
}

fn subagent_cancellation_projection(
    agent_id: &str,
    messages: &[Message],
    steps: u32,
    latest_checkpoint: Option<&SubAgentCheckpoint>,
    turn_end_parking: Option<&Arc<std::sync::atomic::AtomicBool>>,
) -> (
    SubAgentStatus,
    Option<String>,
    Option<SubAgentCheckpoint>,
    Option<SubAgentNeedsInput>,
    AgentWorkerStatus,
    &'static str,
) {
    let parking_requested =
        turn_end_parking.is_some_and(|signal| signal.load(std::sync::atomic::Ordering::Acquire));
    if parking_requested {
        let reason = format!(
            "Parent turn ended before this turn-owned child settled. Work was parked instead of discarded; resume with agent(action=\"start\", prompt=\"Continue the parked assignment.\", resume_from=\"{agent_id}\")."
        );
        let checkpoint = build_subagent_checkpoint(agent_id, &reason, messages, steps, true);
        let needs_input = SubAgentNeedsInput {
            question: format!(
                "Resume this parked child with agent(action=\"start\", prompt=\"Continue the parked assignment.\", resume_from=\"{agent_id}\")."
            ),
        };
        return (
            SubAgentStatus::Interrupted(reason.clone()),
            Some(reason),
            Some(checkpoint),
            Some(needs_input),
            AgentWorkerStatus::Interrupted,
            "parked at parent turn end",
        );
    }

    (
        SubAgentStatus::Cancelled,
        None,
        latest_checkpoint.cloned(),
        None,
        AgentWorkerStatus::Cancelled,
        "cancelled",
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubAgentLoopBoundary {
    Continue,
    Cancellation,
    StepLimit,
}

fn subagent_loop_boundary(
    max_steps: u32,
    steps: u32,
    cancellation_requested: bool,
) -> SubAgentLoopBoundary {
    if cancellation_requested {
        SubAgentLoopBoundary::Cancellation
    } else if max_steps > 0 && steps >= max_steps {
        SubAgentLoopBoundary::StepLimit
    } else {
        SubAgentLoopBoundary::Continue
    }
}

#[allow(clippy::too_many_arguments)]
async fn cancelled_subagent_result(
    runtime: &SubAgentRuntime,
    agent_id: &str,
    agent_type: &FleetRole,
    assignment: &SubAgentAssignment,
    messages: &[Message],
    steps: u32,
    max_steps: u32,
    latest_checkpoint: Option<&SubAgentCheckpoint>,
    turn_end_parking: Option<&Arc<std::sync::atomic::AtomicBool>>,
    transcript_artifact: &mut Option<SubAgentTranscriptArtifactWriter>,
    started_at: Instant,
    fork_context_enabled: bool,
    progress_suffix: &str,
) -> SubAgentResult {
    let (status, result, checkpoint, needs_input, worker_status, progress) =
        subagent_cancellation_projection(
            agent_id,
            messages,
            steps,
            latest_checkpoint,
            turn_end_parking,
        );
    record_agent_progress(
        runtime,
        agent_id,
        AgentProgressEventMeta::new(worker_status).with_step(steps),
        format!(
            "{}: {progress}{progress_suffix}",
            format_step_counter(steps, max_steps)
        ),
    );
    let duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
    insert_subagent_full_transcript_handle(
        runtime,
        agent_id,
        agent_type,
        assignment,
        &status,
        result.as_ref(),
        checkpoint.as_ref(),
        transcript_artifact.as_mut(),
        messages,
        steps,
        duration_ms,
        fork_context_enabled,
    )
    .await;
    SubAgentResult {
        name: agent_id.to_string(),
        agent_id: agent_id.to_string(),
        context_mode: if fork_context_enabled {
            "forked"
        } else {
            "fresh"
        }
        .to_string(),
        fork_context: fork_context_enabled,
        workspace: Some(runtime.context.workspace.clone()),
        git_branch: current_git_branch(&runtime.context.workspace),
        agent_type: agent_type.clone(),
        assignment: assignment.clone(),
        model: runtime.model.clone(),
        nickname: None,
        status,
        worker_status: None,
        runtime_permissions: None,
        parent_run_id: runtime.parent_agent_id.clone(),
        spawn_depth: runtime.spawn_depth,
        child_route: None,
        result,
        steps_taken: steps,
        checkpoint,
        needs_input,
        duration_ms,
        started_at: Some(started_at),
        from_prior_session: false,
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_subagent(
    runtime: &SubAgentRuntime,
    agent_id: String,
    agent_type: FleetRole,
    prompt: String,
    assignment: SubAgentAssignment,
    allowed_tools: Option<Vec<String>>,
    fork_context: bool,
    started_at: Instant,
    max_steps: u32,
    token_budget: Option<u64>,
    turn_end_parking: Option<Arc<std::sync::atomic::AtomicBool>>,
    mut input_rx: mpsc::UnboundedReceiver<SubAgentInput>,
) -> Result<SubAgentResult> {
    let system_prompt =
        build_subagent_system_prompt_with_skills(&agent_type, &assignment, &runtime.context);
    let fork_context_enabled = fork_context;
    let fork_context = fork_context_enabled
        .then_some(runtime.fork_context.as_ref())
        .flatten();
    let request_system = subagent_request_system_prompt(&system_prompt);
    // Refresh only the Work portion of the inherited state, now, at the fork
    // seam (#3983). The parent's captured transcript and its stable state text
    // are untouched.
    let refreshed_fork_context = match fork_context {
        Some(context) => Some(context.with_resolved_state_block().await),
        None => None,
    };
    let mut messages = build_initial_subagent_messages_with_system(
        &prompt,
        &assignment,
        &agent_type,
        &system_prompt,
        refreshed_fork_context.as_ref(),
    );
    // This agent's *own* To-do list (#4810): the runtime carries the parent's
    // work-graph handle but a private list, so this source resolves against
    // the child's store and can never read a parent's or sibling's list. Read
    // for the agent card and the fork handoff — never appended to a request.
    let todo_source = crate::todo_snapshot::TodoSource::new(
        runtime.context.runtime.work.clone(),
        runtime.todos.clone(),
    );
    // Last snapshot published to the mailbox for *this* agent, so repeated
    // tool calls that leave the list untouched do not redraw its card.
    let mut last_published_todo: Option<crate::tools::todo::TodoListSnapshot> = None;
    let mut transcript_artifact =
        match SubAgentTranscriptArtifactWriter::for_runtime(runtime, &agent_id).await {
            Ok(mut writer) => {
                if let Err(err) = writer.sync_messages(&messages, false) {
                    tracing::warn!(
                        target: "subagent",
                        ?err,
                        agent_id,
                        "failed to persist initial sub-agent transcript"
                    );
                }
                Some(writer)
            }
            Err(err) => {
                tracing::warn!(
                    target: "subagent",
                    ?err,
                    agent_id,
                    "failed to initialize complete sub-agent transcript artifact"
                );
                None
            }
        };
    let (runtime_for_tools, mut child_completion_rx) = runtime_for_nested_agent_tools(
        runtime,
        &agent_id,
        SubAgentForkContext {
            messages: messages.clone(),
            structured_state_block: None,
            // A grandchild forks *this* agent, so it inherits this agent's own
            // list, resolved when that spawn actually happens.
            work_source: Some(todo_source.clone()),
        },
    );
    let tool_registry = SubAgentToolRegistry::new_with_owner(
        runtime_for_tools,
        agent_type.clone(),
        agent_id.clone(),
        assignment
            .role
            .as_deref()
            .filter(|role| !role.trim().is_empty())
            .unwrap_or(agent_type.as_str())
            .to_string(),
        allowed_tools.clone(),
        // This agent's own list, allocated by `child_runtime()` (#4810). It is
        // writable by this agent alone: neither the parent nor a sibling holds
        // this `Arc`, so `work_update` here cannot reach the parent's Work
        // checklist or work graph.
        runtime.todos.clone(),
        Arc::new(Mutex::new(PlanState::default())),
    );
    let unavailable_tools = tool_registry.unavailable_allowed_tools();
    if !unavailable_tools.is_empty() {
        return Err(anyhow!(
            "Sub-agent requested unavailable tools: {}",
            unavailable_tools.join(", ")
        ));
    }
    let tool_catalog = tool_registry.deferred_catalog_for_model(&agent_type);
    let mut tool_surface = SubAgentToolSurface::new(tool_catalog, &[]);
    let mut steps = 0;
    let mut final_result: Option<String> = None;
    let mut pending_inputs: VecDeque<SubAgentInput> = VecDeque::new();
    let mut latest_checkpoint: Option<SubAgentCheckpoint> = None;
    let mut tokens_used: u64 = 0;
    let mut terminal_failure_reason: Option<String> = None;
    // Distinguish a real "the model chose to stop" exit from an explicitly
    // configured step-cap exit. The normal loop is unbounded (max_steps == 0).
    let mut stopped_naturally = false;

    // A queued child can be parked before it ever acquires a launch permit.
    // Project that terminal state before emitting Started/Starting so the
    // lifecycle never claims that work began after its parent already ended.
    if subagent_loop_boundary(max_steps, steps, runtime.cancel_token.is_cancelled())
        == SubAgentLoopBoundary::Cancellation
    {
        return Ok(cancelled_subagent_result(
            runtime,
            &agent_id,
            &agent_type,
            &assignment,
            &messages,
            steps,
            max_steps,
            latest_checkpoint.as_ref(),
            turn_end_parking.as_ref(),
            &mut transcript_artifact,
            started_at,
            fork_context_enabled,
            " before start",
        )
        .await);
    }

    if let Some(mb) = runtime.mailbox.as_ref() {
        let _ = mb.send(MailboxMessage::started(&agent_id, agent_type.clone()));
    }
    record_agent_progress(
        runtime,
        &agent_id,
        AgentProgressEventMeta::new(AgentWorkerStatus::Starting),
        format!("started ({})", agent_type.as_str()),
    );

    // A worker is inspectable as soon as it is launched, not only after its
    // first model round trip. This gives Open a real conversation destination
    // while the worker is waiting on the provider.
    publish_live_subagent_transcript(
        runtime,
        &agent_id,
        &agent_type,
        &assignment,
        None,
        None,
        transcript_artifact.as_mut(),
        &messages,
        steps,
        started_at,
        fork_context_enabled,
    )
    .await;

    loop {
        match subagent_loop_boundary(max_steps, steps, runtime.cancel_token.is_cancelled()) {
            // Cancellation must win even after the final allowed tool step.
            // Otherwise a turn-end park at that seam is mislabeled as step
            // exhaustion and its continuable checkpoint becomes terminal.
            SubAgentLoopBoundary::Cancellation => {
                return Ok(cancelled_subagent_result(
                    runtime,
                    &agent_id,
                    &agent_type,
                    &assignment,
                    &messages,
                    steps,
                    max_steps,
                    latest_checkpoint.as_ref(),
                    turn_end_parking.as_ref(),
                    &mut transcript_artifact,
                    started_at,
                    fork_context_enabled,
                    "",
                )
                .await);
            }
            SubAgentLoopBoundary::StepLimit => break,
            SubAgentLoopBoundary::Continue => {}
        }

        steps = steps.saturating_add(1);
        record_agent_progress(
            runtime,
            &agent_id,
            AgentProgressEventMeta::new(AgentWorkerStatus::ModelWait).with_step(steps),
            format!(
                "{}: requesting model response",
                format_step_counter(steps, max_steps)
            ),
        );

        while let Ok(input) = input_rx.try_recv() {
            input.mark_taken();
            if input.interrupt {
                pending_inputs.clear();
            }
            pending_inputs.push_back(input);
        }

        append_subagent_inputs_as_user_messages(&mut messages, &mut pending_inputs);

        let child_completions = drain_child_completion_events(&mut child_completion_rx);
        if !child_completions.is_empty() {
            let count = child_completions.len();
            record_agent_progress(
                runtime,
                &agent_id,
                AgentProgressEventMeta::new(AgentWorkerStatus::Running).with_step(steps),
                format!(
                    "{}: received {count} child sub-agent completion(s)",
                    format_step_counter(steps, max_steps)
                ),
            );
            messages.push(child_completion_runtime_message(&child_completions));
        }

        let tools = tool_surface.request_tools(
            tool_registry.deferred_catalog_for_model(&agent_type),
            runtime
                .api_config
                .as_ref()
                .and_then(|config| config.strict_tool_mode)
                .unwrap_or(false),
        );
        let request_active_tool_names = tool_surface.active_names.clone();
        let has_tools = !tools.is_empty();
        // A child sends its stored messages and nothing else. Its To-do state
        // reaches it the same way the parent's does: through the tool results
        // its own `work_update` calls returned, which are already in
        // `messages`. Nothing synthetic is appended per step.
        let mut request_messages = messages.clone();
        let request_route = runtime
            .client
            .effective_route_envelope(&runtime.model, chrono::Utc::now());
        let image_input = runtime
            .api_config
            .as_deref()
            .and_then(|config| {
                crate::route_runtime::resolve_runtime_route(
                    config,
                    request_route.provider,
                    Some(&request_route.model),
                )
                .ok()
            })
            .map_or(crate::model_profile::SupportState::Unknown, |route| {
                route.candidate.capabilities().image_input
            });
        crate::image_attach::strip_images_when_unsupported(
            &mut request_messages,
            image_input,
            &request_route.model,
        );
        let request = MessageRequest {
            model: runtime.model.clone(),
            messages: request_messages,
            max_tokens: runtime
                .client
                .effective_max_output_tokens(&request_route.model),
            system: Some(request_system.clone()),
            tools: has_tools.then(|| tools.clone()),
            tool_choice: has_tools.then(|| json!({ "type": "auto" })),
            metadata: None,
            thinking: None,
            reasoning_effort: runtime.reasoning_effort.clone(),
            stream: Some(false),
            temperature: None,
            top_p: None,
        };
        latest_checkpoint = Some(
            checkpoint_subagent_progress(
                runtime,
                &agent_id,
                "before_api_request",
                &messages,
                steps,
                true,
            )
            .await,
        );
        publish_live_subagent_transcript(
            runtime,
            &agent_id,
            &agent_type,
            &assignment,
            final_result.as_ref(),
            latest_checkpoint.as_ref(),
            transcript_artifact.as_mut(),
            &messages,
            steps,
            started_at,
            fork_context_enabled,
        )
        .await;

        // Race the API call against the cancellation token so a parent
        // cancel during a long thinking turn doesn't have to wait for the
        // step timeout.
        let (response, usage_route) = tokio::select! {
            biased;
            () = runtime.cancel_token.cancelled() => {
                return Ok(cancelled_subagent_result(
                    runtime,
                    &agent_id,
                    &agent_type,
                    &assignment,
                    &messages,
                    steps,
                    max_steps,
                    latest_checkpoint.as_ref(),
                    turn_end_parking.as_ref(),
                    &mut transcript_artifact,
                    started_at,
                    fork_context_enabled,
                    " mid-request",
                )
                .await);
            }
            api = request_subagent_model_response_with_retries(
                runtime,
                &agent_id,
                steps,
                max_steps,
                request,
            ) => {
                match api {
                    Ok(response) => response,
                    Err(failure) => {
                        // Fatal provider failures used to return bare Err —
                        // no checkpoint, no transcript handle — stranding
                        // every completed step of child work (a 141s scout
                        // died unrecoverable in dogfood). A fatal error is
                        // not retried, but the work it interrupts is still
                        // work: checkpoint and park exactly like the
                        // transient-exhaustion arm so the parent can fix the
                        // route and resume from the continuation handle.
                        // Only a child with no steps fails plainly — there
                        // is nothing to preserve yet.
                        let (reason, checkpoint_reason) = match failure {
                            SubAgentApiRequestFailure::Fatal(err) => {
                                // `steps` was already incremented for THIS
                                // attempt, so steps == 1 means the very first
                                // request died with zero completed work —
                                // fail plainly, exactly as before.
                                if steps <= 1 {
                                    return Err(err);
                                }
                                (
                                    format!(
                                        "fatal provider error: {err}; checkpoint preserved for continuation"
                                    ),
                                    "fatal_provider_error",
                                )
                            }
                            SubAgentApiRequestFailure::Interrupted {
                                reason,
                                checkpoint_reason,
                            } => (reason, checkpoint_reason),
                        };
                        let checkpoint = checkpoint_subagent_progress(
                            runtime,
                            &agent_id,
                            checkpoint_reason,
                            &messages,
                            steps,
                            true,
                        )
                        .await;
                        record_agent_progress(
                            runtime,
                            &agent_id,
                            AgentProgressEventMeta::new(AgentWorkerStatus::Interrupted)
                                .with_step(steps),
                            format!("{}: interrupted; {reason}", format_step_counter(steps, max_steps)),
                        );
                        let status = SubAgentStatus::Interrupted(reason.clone());
                        let duration_ms =
                            u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
                        insert_subagent_full_transcript_handle(
                            runtime,
                            &agent_id,
                            &agent_type,
                            &assignment,
                            &status,
                            Some(&reason),
                            Some(&checkpoint),
                            transcript_artifact.as_mut(),
                            &messages,
                            steps,
                            duration_ms,
                            fork_context_enabled,
                        )
                        .await;
                        let needs_input =
                            needs_input_for_interrupted_checkpoint(&reason, &checkpoint);
                        record_agent_progress(
                            runtime,
                            &agent_id,
                            AgentProgressEventMeta::new(AgentWorkerStatus::WaitingForUser)
                                .with_step(steps),
                            format!(
                                "{}: waiting for user; {}",
                                format_step_counter(steps, max_steps),
                                needs_input.question
                            ),
                        );
                        return Ok(SubAgentResult {
                            name: agent_id.clone(),
                            agent_id: agent_id.clone(),
                            context_mode: if fork_context_enabled {
                                "forked"
                            } else {
                                "fresh"
                            }
                            .to_string(),
                            fork_context: fork_context_enabled,
                            workspace: Some(runtime.context.workspace.clone()),
                            git_branch: current_git_branch(&runtime.context.workspace),
                            agent_type: agent_type.clone(),
                            assignment: assignment.clone(),
                            model: runtime.model.clone(),
                            nickname: None,
                            status,
                            worker_status: None,
                            runtime_permissions: None,
                            parent_run_id: runtime.parent_agent_id.clone(),
                            spawn_depth: runtime.spawn_depth,
                            child_route: None,
                            result: Some(reason),
                            steps_taken: steps,
                            checkpoint: Some(checkpoint),
                            needs_input: Some(needs_input),
                            duration_ms,
                            started_at: Some(started_at),
                            from_prior_session: false,
                        });
                    }
                }
            }
        };

        let mut tool_uses = Vec::new();

        let usage_source_id = format!("subagent:{agent_id}:step:{steps}:response:{}", response.id);
        // Runtime-owned children persist directly before best-effort UI
        // delivery. The owner lease outlives the parent mailbox when a top-
        // level child remains active after the parent turn terminates.
        // The child's own route billing travels on `usage_route`: the client
        // that actually ran froze its provider, identity, endpoint fingerprint,
        // billing surface and billing mode at construction. It is deliberately
        // not re-derived from ambient parent config at completion time.
        record_provider_response_usage(
            runtime,
            &agent_id,
            &usage_source_id,
            usage_route,
            &response.usage,
        )
        .await;

        // Per-worker token-budget enforcement (#3321): stop a single runaway
        // worker once its accumulated model tokens exceed its own cap. This
        // complements — and does not double-count — the scope-level admission
        // gate (#3319), which bounds aggregate fan-out across siblings. The
        // local accumulator mirrors the manager's `record.usage.total_tokens`
        // (both derive from `response.usage`), so the scope accounting stays
        // consistent and is never inflated by this check.
        //
        // The shared scope is also enforced HERE, mid-run: admission alone
        // only refuses *future* spawns, so a parallel fan-out whose children
        // all attached while the scope was nearly full could collectively
        // burn many times the configured budget and still report Completed.
        // Checking the live aggregate after each turn caps the overshoot at
        // the turns already in flight.
        // Compute the budget state now, after accounting, but terminalize on
        // it only after the provider stop reason has been classified below.
        tokens_used = tokens_used.saturating_add(usage_total_tokens(&response.usage));
        let scope_budget_state = {
            let manager = runtime.manager.read().await;
            manager.budget_scope_state(&agent_id)
        };
        let budget_exhausted_detail = if let Some(budget) =
            token_budget.filter(|&budget| tokens_used > budget)
        {
            Some(format!("token budget exhausted ({tokens_used}/{budget})"))
        } else {
            scope_budget_state
                .filter(|(spent, limit)| spent > limit)
                .map(|(spent, limit)| {
                    format!("shared token budget exhausted ({spent}/{limit} spent across the run)")
                })
        };

        let mut current_response_text = None;
        for block in &response.content {
            match block {
                ContentBlock::Text { text, .. } if !text.trim().is_empty() => {
                    current_response_text = Some(text.clone());
                    final_result = Some(text.clone());
                }
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    tool_uses.push((id.clone(), name.clone(), input.clone()));
                }
                _ => {}
            }
        }

        messages.push(Message {
            role: Role::Assistant,
            content: response.content.clone(),
        });
        latest_checkpoint = Some(
            checkpoint_subagent_progress(
                runtime,
                &agent_id,
                "after_model_response",
                &messages,
                steps,
                true,
            )
            .await,
        );
        publish_live_subagent_transcript(
            runtime,
            &agent_id,
            &agent_type,
            &assignment,
            final_result.as_ref(),
            latest_checkpoint.as_ref(),
            transcript_artifact.as_mut(),
            &messages,
            steps,
            started_at,
            fork_context_enabled,
        )
        .await;

        if is_incomplete_stop_reason(response.stop_reason.as_deref()) {
            final_result = current_response_text;
            let failure = incomplete_subagent_response_failure(&response);
            record_agent_progress(
                runtime,
                &agent_id,
                AgentProgressEventMeta::new(AgentWorkerStatus::Failed).with_step(steps),
                format!("{}: {failure}", format_step_counter(steps, max_steps)),
            );
            terminal_failure_reason = Some(failure);
            break;
        }

        if let Some(detail) = budget_exhausted_detail {
            record_agent_progress(
                runtime,
                &agent_id,
                AgentProgressEventMeta::new(AgentWorkerStatus::Failed).with_step(steps),
                format!("{}: {detail}", format_step_counter(steps, max_steps)),
            );
            let status = SubAgentStatus::BudgetExhausted;
            let duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
            latest_checkpoint = Some(
                checkpoint_subagent_progress(
                    runtime,
                    &agent_id,
                    "token_budget_exhausted",
                    &messages,
                    steps,
                    true,
                )
                .await,
            );
            insert_subagent_full_transcript_handle(
                runtime,
                &agent_id,
                &agent_type,
                &assignment,
                &status,
                final_result.as_ref(),
                latest_checkpoint.as_ref(),
                transcript_artifact.as_mut(),
                &messages,
                steps,
                duration_ms,
                fork_context_enabled,
            )
            .await;
            return Ok(SubAgentResult {
                name: agent_id.clone(),
                agent_id: agent_id.clone(),
                context_mode: if fork_context_enabled {
                    "forked"
                } else {
                    "fresh"
                }
                .to_string(),
                fork_context: fork_context_enabled,
                workspace: Some(runtime.context.workspace.clone()),
                git_branch: current_git_branch(&runtime.context.workspace),
                agent_type: agent_type.clone(),
                assignment: assignment.clone(),
                model: runtime.model.clone(),
                nickname: None,
                status,
                worker_status: None,
                runtime_permissions: None,
                parent_run_id: runtime.parent_agent_id.clone(),
                spawn_depth: runtime.spawn_depth,
                child_route: None,
                result: final_result.clone(),
                steps_taken: steps,
                checkpoint: latest_checkpoint.clone(),
                needs_input: None,
                duration_ms,
                started_at: Some(started_at),
                from_prior_session: false,
            });
        }
        if tool_uses.is_empty() {
            let child_completions = drain_child_completion_events(&mut child_completion_rx);
            if !child_completions.is_empty() {
                let count = child_completions.len();
                record_agent_progress(
                    runtime,
                    &agent_id,
                    AgentProgressEventMeta::new(AgentWorkerStatus::Running).with_step(steps),
                    format!(
                        "{}: resuming with {count} child sub-agent completion(s)",
                        format_step_counter(steps, max_steps)
                    ),
                );
                messages.push(child_completion_runtime_message(&child_completions));
                latest_checkpoint = Some(
                    checkpoint_subagent_progress(
                        runtime,
                        &agent_id,
                        "after_tail_child_subagent_completion",
                        &messages,
                        steps,
                        true,
                    )
                    .await,
                );
                publish_live_subagent_transcript(
                    runtime,
                    &agent_id,
                    &agent_type,
                    &assignment,
                    final_result.as_ref(),
                    latest_checkpoint.as_ref(),
                    transcript_artifact.as_mut(),
                    &messages,
                    steps,
                    started_at,
                    fork_context_enabled,
                )
                .await;
                continue;
            }
            while let Ok(input) = input_rx.try_recv() {
                input.mark_taken();
                if input.interrupt {
                    pending_inputs.clear();
                }
                pending_inputs.push_back(input);
            }
            if pending_inputs.is_empty() {
                record_agent_progress(
                    runtime,
                    &agent_id,
                    AgentProgressEventMeta::new(AgentWorkerStatus::Completed).with_step(steps),
                    format!("{}: complete", format_step_counter(steps, max_steps)),
                );
                stopped_naturally = true;
                break;
            }
            continue;
        }

        record_agent_progress(
            runtime,
            &agent_id,
            AgentProgressEventMeta::new(AgentWorkerStatus::Running).with_step(steps),
            format!(
                "{}: executing {} tool call(s)",
                format_step_counter(steps, max_steps),
                tool_uses.len()
            ),
        );
        let mut tool_results: Vec<ContentBlock> = Vec::new();
        for (tool_id, tool_name, tool_input) in tool_uses {
            let activity_tool_name = canonical_action_alias(&tool_name, &tool_input).to_string();
            let tool_display_name = subagent_progress_tool_display_name(&activity_tool_name);
            record_agent_progress(
                runtime,
                &agent_id,
                AgentProgressEventMeta::new(AgentWorkerStatus::RunningTool)
                    .with_step(steps)
                    .with_tool(activity_tool_name.clone()),
                format!(
                    "{}: running tool '{tool_display_name}'",
                    format_step_counter(steps, max_steps)
                ),
            );
            if let Some(mb) = runtime.mailbox.as_ref() {
                let _ = mb.send(MailboxMessage::ToolCallStarted {
                    agent_id: agent_id.clone(),
                    tool_name: activity_tool_name.clone(),
                    step: steps,
                });
            }
            let output = match tokio::time::timeout(runtime.tool_timeout, async {
                tool_registry
                    .execute_from_surface(
                        &agent_id,
                        &tool_id,
                        &mut tool_surface,
                        &request_active_tool_names,
                        &tool_name,
                        tool_input,
                    )
                    .await
            })
            .await
            {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => RichToolResult::plain(ToolResult::error(format!("Error: {e}"))),
                Err(_) => RichToolResult::plain(ToolResult::error(format!(
                    "Error: Tool {tool_name} timed out"
                ))),
            };
            let tool_ok = output.result.success;
            let content_blocks = output
                .content_blocks
                .iter()
                .filter_map(|block| serde_json::to_value(block).ok())
                .collect::<Vec<_>>();
            let (result, spilled_to) = bound_subagent_tool_result(
                &agent_id,
                &tool_id,
                &tool_name,
                &runtime.context.state_namespace,
                tool_ok,
                output.result.content,
            );
            if let Some(path) = spilled_to.as_ref() {
                record_agent_progress(
                    runtime,
                    &agent_id,
                    AgentProgressEventMeta::new(AgentWorkerStatus::RunningTool)
                        .with_step(steps)
                        .with_tool(activity_tool_name.clone()),
                    format!(
                        "{}: tool '{tool_display_name}' output spilled to {}",
                        format_step_counter(steps, max_steps),
                        path.display()
                    ),
                );
            }
            record_agent_progress(
                runtime,
                &agent_id,
                AgentProgressEventMeta::new(AgentWorkerStatus::Running).with_step(steps),
                format!(
                    "{}: finished tool '{tool_display_name}'",
                    format_step_counter(steps, max_steps)
                ),
            );
            if let Some(mb) = runtime.mailbox.as_ref() {
                let _ = mb.send(MailboxMessage::ToolCallCompleted {
                    agent_id: agent_id.clone(),
                    tool_name: activity_tool_name,
                    step: steps,
                    ok: tool_ok,
                });
                // This child's own list, read from its own store right after
                // the tool that may have changed it — so a `work_update` in
                // this step is visible on this child's card in this same turn,
                // not one step later (#4810). Published only on change, and
                // never before the child has stated any work, so a child that
                // never uses the To-do surface adds no rows to its card.
                let todo = todo_source.snapshot().await;
                if work_state_worth_publishing(last_published_todo.as_ref(), &todo) {
                    let _ = mb.send(MailboxMessage::work_state(agent_id.clone(), todo.clone()));
                    last_published_todo = Some(todo);
                }
            }

            tool_results.push(ContentBlock::ToolResult {
                tool_use_id: tool_id,
                content: result,
                is_error: None,
                content_blocks: (!content_blocks.is_empty()).then_some(content_blocks),
            });
        }

        if !tool_results.is_empty() {
            messages.push(Message {
                role: Role::User,
                content: tool_results,
            });
            latest_checkpoint = Some(
                checkpoint_subagent_progress(
                    runtime,
                    &agent_id,
                    "after_tool_results",
                    &messages,
                    steps,
                    true,
                )
                .await,
            );
            publish_live_subagent_transcript(
                runtime,
                &agent_id,
                &agent_type,
                &assignment,
                final_result.as_ref(),
                latest_checkpoint.as_ref(),
                transcript_artifact.as_mut(),
                &messages,
                steps,
                started_at,
                fork_context_enabled,
            )
            .await;
        }
    }

    release_resident_leases_for(&agent_id);
    let has_final_summary = final_result
        .as_deref()
        .map(|text| !text.trim().is_empty())
        .unwrap_or(false);
    // #4050: only a natural stop with a final summary is a real success.
    let status = if let Some(reason) = terminal_failure_reason {
        SubAgentStatus::Failed(reason)
    } else if stopped_naturally {
        if has_final_summary {
            SubAgentStatus::Completed
        } else {
            SubAgentStatus::Failed(
                "child stopped without returning a final summary (its last turn produced no assistant text)".to_string(),
            )
        }
    } else {
        SubAgentStatus::Failed(format!(
            "child step budget exhausted (limit: {max_steps} steps; used: {steps}); \
             raise it with max_steps or split the work into smaller independent tasks"
        ))
    };
    let duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
    latest_checkpoint = Some(build_subagent_checkpoint(
        &agent_id,
        subagent_status_name(&status),
        &messages,
        steps,
        false,
    ));
    insert_subagent_full_transcript_handle(
        runtime,
        &agent_id,
        &agent_type,
        &assignment,
        &status,
        final_result.as_ref(),
        latest_checkpoint.as_ref(),
        transcript_artifact.as_mut(),
        &messages,
        steps,
        duration_ms,
        fork_context_enabled,
    )
    .await;

    Ok(SubAgentResult {
        name: agent_id.clone(),
        agent_id,
        context_mode: if fork_context_enabled {
            "forked"
        } else {
            "fresh"
        }
        .to_string(),
        fork_context: fork_context_enabled,
        workspace: Some(runtime.context.workspace.clone()),
        git_branch: current_git_branch(&runtime.context.workspace),
        agent_type,
        assignment,
        model: runtime.model.clone(),
        nickname: None,
        status,
        worker_status: None,
        runtime_permissions: None,
        parent_run_id: runtime.parent_agent_id.clone(),
        spawn_depth: runtime.spawn_depth,
        child_route: None,
        result: final_result,
        steps_taken: steps,
        checkpoint: latest_checkpoint,
        needs_input: None,
        duration_ms,
        started_at: Some(started_at),
        from_prior_session: false,
    })
}

/// First non-empty string among `keys`, refusing a wrong type.
///
/// A key that is absent, `null`, or an empty/blank string falls through to
/// the next spelling exactly as before. Anything else present under one of
/// these names is a type error naming the parameter: silently dropping a
/// value the caller supplied is how a declared restriction evaporates.
fn optional_input_str<'a>(input: &'a Value, keys: &[&str]) -> Result<Option<&'a str>, ToolError> {
    for key in keys {
        let Some(value) = input.get(*key) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        let text = value
            .as_str()
            .ok_or_else(|| codewhale_tools::type_mismatch(key, value, "a string"))?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        return Ok(Some(trimmed));
    }
    Ok(None)
}

/// First present value among `keys`, treating `null` as absent.
fn aliased_value<'a, 'k>(input: &'a Value, keys: &'k [&'k str]) -> Option<(&'k str, &'a Value)> {
    keys.iter().find_map(|key| {
        input
            .get(*key)
            .filter(|value| !value.is_null())
            .map(|value| (*key, value))
    })
}

/// Optional aliased u64, refusing a wrong type instead of dropping it.
fn parse_optional_u64(input: &Value, keys: &[&str]) -> Result<Option<u64>, ToolError> {
    let Some((key, value)) = aliased_value(input, keys) else {
        return Ok(None);
    };
    value
        .as_u64()
        .map(Some)
        .ok_or_else(|| codewhale_tools::type_mismatch(key, value, "a non-negative integer"))
}

/// Optional array of tool names, refusing a wrong type instead of dropping it.
///
/// A deny-list handed over as a bare string used to vanish without a word,
/// which silently *widened* the child's authority. Entries are trimmed and
/// de-duplicated; blanks are dropped.
fn parse_tool_name_list(input: &Value, key: &str) -> Result<Option<Vec<String>>, ToolError> {
    let Some(value) = input.get(key).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let array = value
        .as_array()
        .ok_or_else(|| codewhale_tools::type_mismatch(key, value, "an array of tool names"))?;
    let mut tools: Vec<String> = Vec::new();
    for item in array {
        let tool = item
            .as_str()
            .ok_or_else(|| codewhale_tools::type_mismatch(&format!("{key}[]"), item, "a string"))?;
        let trimmed = tool.trim();
        if !trimmed.is_empty() && !tools.iter().any(|existing| existing == trimmed) {
            tools.push(trimmed.to_string());
        }
    }
    Ok(Some(tools))
}

fn parse_text_or_items(
    input: &Value,
    text_keys: &[&str],
    items_key: &str,
    required_field: &str,
) -> Result<String, ToolError> {
    let text = optional_input_str(input, text_keys)?.map(str::to_string);
    let items = parse_items_text(input, items_key)?;
    match (text, items) {
        (Some(_), Some(_)) => Err(ToolError::invalid_input(format!(
            "Provide either {required_field} text or {items_key}, but not both"
        ))),
        (Some(text), None) => Ok(text),
        (None, Some(items)) => Ok(items),
        (None, None) => Err(ToolError::missing_field(required_field)),
    }
}

fn parse_items_text(input: &Value, key: &str) -> Result<Option<String>, ToolError> {
    let Some(items) = input.get(key) else {
        return Ok(None);
    };
    let array = items
        .as_array()
        .ok_or_else(|| ToolError::invalid_input(format!("'{key}' must be an array")))?;
    if array.is_empty() {
        return Err(ToolError::invalid_input(format!("'{key}' cannot be empty")));
    }

    let mut lines = Vec::new();
    for item in array {
        let object = item
            .as_object()
            .ok_or_else(|| ToolError::invalid_input("each item must be an object"))?;
        let item_type = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("text")
            .trim();
        let rendered = match item_type {
            "text" => object
                .get("text")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .ok_or_else(|| ToolError::invalid_input("text item requires non-empty text"))?,
            "mention" => {
                let name = object
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::invalid_input("mention item requires name"))?;
                let path = object
                    .get("path")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::invalid_input("mention item requires path"))?;
                format!("[mention:${name}]({path})")
            }
            "skill" => {
                let name = object
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::invalid_input("skill item requires name"))?;
                let path = object
                    .get("path")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::invalid_input("skill item requires path"))?;
                format!("[skill:${name}]({path})")
            }
            "local_image" => {
                let path = object
                    .get("path")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::invalid_input("local_image item requires path"))?;
                format!("[local_image:{path}]")
            }
            "image" => {
                let url = object
                    .get("image_url")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::invalid_input("image item requires image_url"))?;
                format!("[image:{url}]")
            }
            _ => object
                .get("text")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| "[input]".to_string()),
        };
        lines.push(rendered);
    }

    Ok(Some(lines.join("\n")))
}

fn parse_spawn_request(input: &Value) -> Result<SpawnRequest, ToolError> {
    let prompt = parse_text_or_items(
        input,
        &["prompt", "message", "objective"],
        "items",
        "prompt",
    )?;
    let dependencies = parse_bounded_strings(input, "dependencies", 8)?;
    let acceptance = parse_bounded_strings(input, "acceptance", 8)?;
    let session_name = optional_input_str(input, &["name", "session_name"])?
        .map(validate_session_name)
        .transpose()?;

    let type_input = optional_input_str(input, &["type", "agent_type", "agent_name"])?;
    let role_input = optional_input_str(input, &["role", "agent_role"])?;

    let parsed_type = type_input
        .map(|kind| {
            FleetRole::from_str(kind).ok_or_else(|| {
                ToolError::invalid_input(format!(
                    "Invalid sub-agent type '{kind}'. Use: {VALID_SUBAGENT_TYPES}"
                ))
            })
        })
        .transpose()?;

    // Role may be either a FleetRole alias (reviewer → FleetRole::Reviewer)
    // or a Fleet role name resolved later (`resolve_spawn_role`). Type aliases
    // still set agent_type; non-alias roles resolve as the profile key (#4177).
    let parsed_role_type = role_input.and_then(FleetRole::from_str);
    let role_is_type_alias = parsed_role_type.is_some();

    if let (Some(type_kind), Some(role_kind)) = (&parsed_type, &parsed_role_type)
        && type_kind != role_kind
    {
        return Err(ToolError::invalid_input(
            "Fleet role conflicts with the explicit legacy agent type".to_string(),
        ));
    }

    let agent_type_explicit = parsed_type.is_some() || parsed_role_type.is_some();
    let agent_type_named = parsed_type.is_some();
    let agent_type = parsed_type
        .or(parsed_role_type)
        .unwrap_or(FleetRole::Worker);

    let role_alias = role_input
        .and_then(normalize_role_alias)
        .or_else(|| type_input.and_then(normalize_role_alias))
        .map(str::to_string);

    // Fleet role token: the raw role only when it is not a descriptive type
    // alias. Type aliases remain local FleetRole vocabulary and must not be
    // promoted into profile keys.
    let fleet_role_token = match role_input {
        Some(raw) if !role_is_type_alias => {
            let token = validate_role_name(raw)?;
            Some(token)
        }
        _ => None,
    };

    let role = role_alias.or_else(|| fleet_role_token.clone()).or_else(|| {
        type_input
            .and_then(normalize_role_alias)
            .map(str::to_string)
    });

    let mut profile = optional_input_str(input, &["profile", "fleet_profile", "roster_profile"])?
        .map(validate_profile_name)
        .transpose()?;
    // When the caller declared a non-type Fleet role, use it as the profile
    // key so `resolve_spawn_role` is the single role resolution path.
    // Descriptive FleetRole aliases (worker/review/plan/verify/...) keep
    // profile=None; promoting those aliases to profile keys made valid direct
    // agent calls fail because several are not role ids (#4177).
    if profile.is_none() {
        profile = fleet_role_token.clone();
    }

    let allowed_tools = parse_tool_name_list(input, "allowed_tools")?;

    let cwd = parse_optional_cwd(input)?;
    let worktree = parse_optional_worktree_request(input)?;
    let model = parse_optional_subagent_model(input, "model")?;
    let explicit_model_strength = optional_input_str(input, &["model_strength", "modelStrength"])?
        .map(SubAgentModelStrength::parse)
        .transpose()?;
    let model_strength_explicit = explicit_model_strength.is_some();
    // Fleet is predictable before setup: every role inherits the active model.
    // A cheaper sibling is an explicit routing choice through model_strength,
    // a saved Fleet profile, or a concrete model override.
    let model_strength = explicit_model_strength.unwrap_or(SubAgentModelStrength::Same);
    let thinking = optional_input_str(input, &["thinking", "reasoning_effort", "reasoningEffort"])?
        .map(SubAgentThinking::parse)
        .transpose()?
        .unwrap_or(SubAgentThinking::Inherit);
    let resident_file = optional_input_str(input, &["resident_file"])?.map(str::to_string);
    let detached = parse_optional_bool(input, &["detached"])?.unwrap_or(false);
    let fork_context =
        parse_optional_bool(input, &["fork_context", "forkContext", "inherit_context"])?;
    let max_depth = parse_optional_u64(input, &["max_depth", "maxDepth", "max_spawn_depth"])?
        .map(|depth| {
            let ceiling = codewhale_config::MAX_SPAWN_DEPTH_CEILING;
            u32::try_from(depth)
                .map_err(|_| {
                    ToolError::invalid_input(format!("max_depth must be between 0 and {ceiling}"))
                })
                .and_then(|depth| {
                    if depth <= ceiling {
                        Ok(depth)
                    } else {
                        Err(ToolError::invalid_input(format!(
                            "max_depth must be between 0 and {ceiling}"
                        )))
                    }
                })
        })
        .transpose()?;
    let token_budget =
        parse_optional_positive_u64(input, &["token_budget", "tokenBudget", "max_tokens"])?;
    let max_steps = parse_optional_u64(input, &["max_steps", "maxSteps"])?.map(|steps| {
        u32::try_from(steps.min(u64::from(MAX_SUBAGENT_STEPS)))
            .expect("max_steps is clamped before conversion")
    });
    let wall_time = parse_optional_u64(input, &["wall_time_secs", "wallTimeSecs"])?
        .map(|seconds| Duration::from_secs(seconds.clamp(1, MAX_CHILD_WALL_TIME.as_secs())));

    // #4042: optional caller-supplied tool deny-list (unioned with the parent's
    // inherited deny-list) and the inheritance opt-out flag (default inherits).
    let disallowed_tools = parse_disallowed_tools(input)?;
    let inherit_disallowed_tools = parse_optional_bool(
        input,
        &["inherit_disallowed_tools", "inheritDisallowedTools"],
    )?
    .unwrap_or(true);

    // Deliberate delegation contract: when `deliberate=true`, require the
    // model to declare task type (or profile), workspace policy, expected
    // artifact, and write authority. The declared values are
    // parsed and ENFORCED whenever present (deliberate or not): declaring
    // authority the runtime ignores would be a false affordance
    // (TUI-DOG-017).
    let deliberate = parse_optional_bool(input, &["deliberate"])?.unwrap_or(false);
    let workspace_policy_str = optional_input_str(input, &["workspace_policy", "workspacePolicy"])?;
    let expected_artifact = optional_input_str(input, &["expected_artifact", "expectedArtifact"])?
        .map(str::trim)
        .filter(|artifact| !artifact.is_empty())
        .map(str::to_string);
    let write_authority_str = optional_input_str(input, &["write_authority", "writeAuthority"])?;
    if deliberate {
        let has_type = agent_type_explicit || profile.is_some();
        let mut missing = Vec::new();
        if !has_type {
            missing.push("type (or profile)");
        }
        if workspace_policy_str.is_none() && worktree.is_none() {
            missing.push("workspace_policy (or worktree=true)");
        }
        if expected_artifact.is_none() {
            missing.push("expected_artifact");
        }
        if write_authority_str.is_none() {
            missing.push("write_authority");
        }
        if !missing.is_empty() {
            return Err(ToolError::invalid_input(format!(
                "deliberate spawn requires: {}. Missing: {}.",
                "type/profile, workspace_policy, expected_artifact, write_authority",
                missing.join(", ")
            )));
        }
    }
    // Enforce the declared workspace policy: `worktree` materializes a real
    // worktree request (the separate `worktree` field is the mechanism that
    // actually creates one), and `shared` must not contradict an explicit
    // worktree ask.
    let worktree = match workspace_policy_str
        .map(|policy| policy.trim().to_ascii_lowercase())
        .as_deref()
    {
        None => worktree,
        Some("worktree") => worktree.or(Some(SubAgentWorktreeRequest {
            branch: None,
            path: None,
            base_ref: None,
        })),
        Some("shared") => {
            if worktree.is_some() {
                return Err(ToolError::invalid_input(
                    "workspace_policy 'shared' conflicts with worktree isolation options; \
                     use workspace_policy 'worktree' or drop the worktree fields.",
                ));
            }
            worktree
        }
        Some(other) => {
            return Err(ToolError::invalid_input(format!(
                "Invalid workspace_policy '{other}'. Use shared or worktree."
            )));
        }
    };
    let write_authority = match write_authority_str
        .map(|auth| auth.trim().to_ascii_lowercase())
        .as_deref()
    {
        None => None,
        Some("read_only") => Some(SpawnWriteAuthority::ReadOnly),
        Some("workspace_write") => Some(SpawnWriteAuthority::WorkspaceWrite),
        Some("worktree_write") => Some(SpawnWriteAuthority::WorktreeWrite),
        Some(other) => {
            return Err(ToolError::invalid_input(format!(
                "Invalid write_authority '{other}'. Use read_only, workspace_write, or worktree_write."
            )));
        }
    };
    if write_authority == Some(SpawnWriteAuthority::WorktreeWrite) && worktree.is_none() {
        return Err(ToolError::invalid_input(
            "write_authority 'worktree_write' requires worktree isolation \
             (workspace_policy 'worktree' or worktree=true).",
        ));
    }
    let write_roots = parse_coordination_paths(input, "write_roots")?;
    let exact_files = parse_coordination_paths(input, "exact_files")?;
    let coordination_contracts = parse_bounded_strings(input, "coordination_contracts", 16)?;
    let resume_from = optional_input_str(input, &["resume_from", "resumeFrom"])?
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let prompt_only_general = agent_type == FleetRole::Worker
        && !agent_type_explicit
        && profile.is_none()
        && role_input.is_none()
        && type_input.is_none();
    let unresolved_profile = profile.is_some();
    let mut request = SpawnRequest {
        session_name,
        prompt: prompt.clone(),
        dependencies,
        acceptance,
        agent_type,
        agent_type_explicit,
        agent_type_named,
        profile,
        assignment: SubAgentAssignment::new(prompt, role),
        allowed_tools,
        model,
        model_strength,
        model_strength_explicit,
        thinking,
        cwd,
        worktree,
        resident_file,
        fork_context,
        max_depth,
        token_budget,
        max_steps,
        wall_time,
        disallowed_tools,
        inherit_disallowed_tools,
        write_authority,
        expected_artifact,
        write_roots,
        exact_files,
        coordination_contracts,
        resume_from,
        detached,
    };
    // A role profile may resolve the parse-time General placeholder to a
    // read-only scout/reviewer or to a write-capable manager/builder. Defer
    // classification until resolve_spawn_role runs; all profile-less requests
    // can be validated immediately.
    if !unresolved_profile {
        validate_spawn_write_contract(&mut request, prompt_only_general)?;
    }
    Ok(request)
}

fn validate_spawn_write_contract(
    request: &mut SpawnRequest,
    allow_prompt_only_general: bool,
) -> Result<(), ToolError> {
    if matches!(
        request.agent_type,
        FleetRole::Scout
            | FleetRole::Planner
            | FleetRole::Reviewer
            | FleetRole::Verifier
            | FleetRole::Consultant
    ) && request
        .write_authority
        .is_some_and(|authority| authority != SpawnWriteAuthority::ReadOnly)
    {
        return Err(ToolError::invalid_input(format!(
            "{} is a read-only role and cannot declare write-capable authority",
            request.agent_type.as_str()
        )));
    }
    // #5123: `type=builder` plus read_only authority used to parse, then
    // silently clamp write/shell off — the child self-BLOCKED as a "builder"
    // holding only read-only inspection tools, after burning a turn
    // discovering it. Fail closed at spawn instead.
    //
    // Two narrowings keep this to the actual lie:
    //
    // 1. Only `type` counts, not `role`. `role: "advisor"` is a role name
    //    copied into `profile` as a resolution key, and the role is not resolved
    //    until `resolve_spawn_role`, so the role says nothing here about write
    //    capability. `role: "implementer"` is a type alias but still an
    //    identity — a Fleet role and its authority posture are independent, and
    //    an acceptance workflow must be able to resolve `implementer` to its
    //    saved profile while narrowing that child to the read-only tool set.
    //
    // 2. Only `builder` counts, not `worker`. Builder is the explicitly
    //    write-capable role, and it is the one the #5123 transcript shows
    //    BLOCKED — the worker in that same transcript ran fine. Worker is the
    //    unnamed default (it renders as "general") whose capability comes from
    //    authority rather than from its name, so a read-only worker is an
    //    ordinary general-purpose child, not a contradiction. The release QA
    //    contract calls exactly that set — worker, scout, reviewer, verifier —
    //    the four canonical read-only Fleet roles.
    //
    // Widening past either narrowing rejected every read-only Workflow leaf and
    // the canonical read-only worker along with it.
    if request.agent_type_named
        && request.agent_type == FleetRole::Builder
        && request.write_authority == Some(SpawnWriteAuthority::ReadOnly)
    {
        return Err(ToolError::invalid_input(format!(
            "{} implies write capability; write_authority=read_only is a contradiction. \
            Use type=explore (or another read-only role), or set write_authority to \
             workspace_write / worktree_write.",
            request.agent_type.as_str()
        )));
    }
    let declares_scope = !request.write_roots.is_empty()
        || !request.exact_files.is_empty()
        || !request.coordination_contracts.is_empty();
    if request.write_authority == Some(SpawnWriteAuthority::ReadOnly) && declares_scope {
        return Err(ToolError::invalid_input(
            "read_only authority cannot declare write_roots, exact_files, or coordination_contracts"
                .to_string(),
        ));
    }
    if request.agent_type == FleetRole::Custom && request.write_authority.is_none() {
        if declares_scope {
            return Err(ToolError::invalid_input(
                "custom write scopes require explicit workspace_write or worktree_write authority"
                    .to_string(),
            ));
        }
        request.write_authority = Some(SpawnWriteAuthority::ReadOnly);
    }
    let write_capable = spawn_request_is_write_capable(request);
    if write_capable
        && request.write_roots.is_empty()
        && request.exact_files.is_empty()
        && request.coordination_contracts.is_empty()
    {
        if request.write_authority.is_some() || !allow_prompt_only_general {
            // Default write scope to the parent workspace root. Escalation
            // outside the workspace is still refused by path normalization
            // when an explicit scope is declared.
            request.write_roots = vec![".".to_string()];
        } else {
            // A prompt-only/general launch remains ergonomic but is not
            // silently granted the whole repository. It starts read-only
            // until the caller supplies an explicit write-capable identity
            // or mutation claim.
            request.write_authority = Some(SpawnWriteAuthority::ReadOnly);
        }
    }
    Ok(())
}

fn parse_bounded_strings(input: &Value, key: &str, limit: usize) -> Result<Vec<String>, ToolError> {
    let Some(value) = input.get(key) else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        return Err(ToolError::invalid_input(format!(
            "{key} must be an array of strings"
        )));
    };
    if items.len() > limit {
        return Err(ToolError::invalid_input(format!(
            "{key} accepts at most {limit} entries"
        )));
    }
    let mut result = Vec::new();
    for item in items {
        let Some(text) = item.as_str() else {
            return Err(ToolError::invalid_input(format!(
                "{key} must contain only strings"
            )));
        };
        let text = text.trim();
        if text.chars().count() > 512 {
            return Err(ToolError::invalid_input(format!(
                "{key} entries must be at most 512 characters"
            )));
        }
        if !text.is_empty() && !result.iter().any(|existing| existing == text) {
            result.push(text.to_string());
        }
    }
    Ok(result)
}

fn parse_coordination_paths(input: &Value, key: &str) -> Result<Vec<String>, ToolError> {
    parse_bounded_strings(input, key, 32)?
        .into_iter()
        .map(|path| normalize_claim_path(&path).map_err(ToolError::invalid_input))
        .collect()
}

fn normalize_claim_path(path: &str) -> Result<String, String> {
    let path = path.replace('\\', "/");
    let trimmed = path.trim();
    if trimmed.len() > 4096 || trimmed.chars().any(|ch| matches!(ch, '\0' | '\r' | '\n')) {
        return Err("write scope path must be one bounded repo-relative line".to_string());
    }
    if trimmed.is_empty() || trimmed == "." {
        return Ok(".".to_string());
    }
    let candidate = Path::new(trimmed);
    if candidate.is_absolute() {
        return Err(format!(
            "write scope path must be repo-relative without traversal: {path}"
        ));
    }
    let mut components = Vec::new();
    for component in candidate.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => {
                let value = value.to_string_lossy().nfc().collect::<String>();
                if !value.is_empty() {
                    components.push(value);
                }
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "write scope path must be repo-relative without traversal: {path}"
                ));
            }
        }
    }
    if components.is_empty() {
        Ok(".".to_string())
    } else {
        Ok(components.join("/"))
    }
}

fn coordination_workspace_prefix(
    manager_workspace: &Path,
    worker_workspace: &Path,
) -> Result<String, String> {
    let manager_workspace = normalize_subagent_workspace(manager_workspace);
    let worker_workspace = normalize_subagent_workspace(worker_workspace);
    let relative = worker_workspace
        .strip_prefix(&manager_workspace)
        .map_err(|_| {
            format!(
                "shared writer workspace '{}' must remain inside coordination root '{}'",
                worker_workspace.display(),
                manager_workspace.display()
            )
        })?;
    normalize_claim_path(&relative.to_string_lossy())
}

fn namespace_coordination_path(prefix: &str, path: &str) -> Result<String, String> {
    let path = normalize_claim_path(path)?;
    match (prefix, path.as_str()) {
        (".", path) | (path, ".") => Ok(path.to_string()),
        (prefix, path) => normalize_claim_path(&format!("{prefix}/{path}")),
    }
}

fn validate_session_name(name: &str) -> Result<String, ToolError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ToolError::invalid_input("name cannot be blank"));
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err(ToolError::invalid_input(
            "name must not contain whitespace; use letters, numbers, '-', '_', or '.'",
        ));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(ToolError::invalid_input(
            "name may only contain ASCII letters, numbers, '-', '_', or '.'",
        ));
    }
    Ok(trimmed.to_string())
}

/// Validate a bounded human Fleet selector. Resolution owns normalization so
/// the route receipt can preserve the safe spelling the caller actually used
/// (`DeepSeek V4 Flash`, `role:scout`, or an exact member id).
fn validate_profile_name(value: &str) -> Result<String, ToolError> {
    validate_roster_selector(value, "profile")
}

fn validate_role_name(value: &str) -> Result<String, ToolError> {
    validate_roster_selector(value, "role")
}

fn validate_roster_selector(value: &str, field: &str) -> Result<String, ToolError> {
    const MAX_SELECTOR_CHARS: usize = 128;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ToolError::invalid_input(format!("{field} cannot be blank")));
    }
    if trimmed.chars().count() > MAX_SELECTOR_CHARS {
        return Err(ToolError::invalid_input(format!(
            "{field} must be at most {MAX_SELECTOR_CHARS} characters"
        )));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(ToolError::invalid_input(format!(
            "{field} must not contain control characters or newlines"
        )));
    }
    Ok(trimmed.to_string())
}

/// Refresh the role-model defaults from the session `Config` at spawn time
/// (#5099). The runtime's `role_models` are a launch-time snapshot, so an
/// explicit `[subagents]` config change mid-session would otherwise stay
/// invisible. There is no roster to re-read: role-only dispatch resolves
/// roles, never saved members. Without the session `Config` (tests, legacy
/// runtimes) the launch-time snapshot is the only source available and is kept.
fn refresh_spawn_route_sources(runtime: &mut SubAgentRuntime) {
    let Some(config) = runtime.api_config.as_deref() else {
        return;
    };
    // No roster to re-read: keep the launch-time role defaults (which include
    // the roster's model overrides) and overlay the live `[subagents]` config
    // on top so mid-session config edits still win.
    let mut role_models = std::mem::take(&mut runtime.role_models);
    role_models.extend(config.subagent_model_overrides());
    runtime.role_models = role_models;
}

/// Resolve the `profile` spawn parameter against the closed role set and fold
/// it into the request: agent type (when not explicitly given) and assignment
/// role.
///
/// Runs at spawn time — `parse_spawn_request` has no runtime access. There is
/// no roster: `profile` must name a Fleet role (canonical or legacy alias),
/// and roles carry no provider/model pins, instruction overlays, or delegation
/// hints — the child's capability posture is governed by its [`FleetRole`]
/// via `WorkerRuntimeProfile::for_role`. Anything else fails closed with the
/// role list, the same shape the roster lookup's unknown-member error had.
fn resolve_spawn_role(request: &mut SpawnRequest) -> Result<(), ToolError> {
    let Some(profile_id) = request.profile.clone() else {
        return Ok(());
    };
    let Some(role) = FleetRole::from_str(&profile_id) else {
        return Err(ToolError::invalid_input(format!(
            "Unknown Fleet role/profile '{profile_id}'. Fleet profiles are roles: {VALID_ROLE_ALIASES}."
        )));
    };
    if request.agent_type_explicit && request.agent_type != role {
        return Err(ToolError::invalid_input(format!(
            "profile '{}' implies type {}; conflicting explicit type '{}'",
            profile_id,
            role.as_str(),
            request.agent_type.as_str()
        )));
    }
    request.agent_type = role.clone();
    // Record the canonical role id after resolution.
    request.profile = Some(role.as_str().to_string());
    // Surface the role in prompts and ledger records.
    request.assignment.role = Some(role.as_str().to_string());
    Ok(())
}

/// The active parent's posture, expressed as the upper bound for a child's
/// requested role policy.
///
/// Read off the live parent runtime rather than assumed: this is the value
/// that makes "a spawn cannot widen what the operator is currently allowed
/// to do" true at runtime instead of on paper.
#[must_use]
pub(crate) fn session_permission_ceiling(
    runtime: &SubAgentRuntime,
) -> codewhale_workflow::PermissionCeiling {
    codewhale_workflow::PermissionCeiling {
        write: runtime.worker_profile.permissions.write,
        network_tool: runtime.worker_profile.permissions.network
            && runtime.agent_tool_surface_options.web_search_enabled,
        shell: crate::fleet::role::session_shell_ceiling(
            runtime.worker_profile.shell,
            runtime.allow_shell,
        ),
        delegation_depth: runtime.worker_profile.max_spawn_depth,
        // The parent side never withholds the coarse tool bit: fine-grained
        // inherited ToolScope/deny rules are intersected again by the spawn
        // runtime. This value only captures the dimensions represented here.
        tools: true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpawnRouteSource {
    TaskModel,
    TaskModelStrength,
    RoleDefault,
    RunModel,
}

impl SpawnRouteSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::TaskModel => "task.model",
            Self::TaskModelStrength => "task.model_strength",
            Self::RoleDefault => "role.default",
            Self::RunModel => "run.model",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SpawnModelSelection {
    model_route: ModelRoute,
    source: SpawnRouteSource,
}

/// Resolve the child model once, with receipt-grade precedence provenance:
/// explicit task field > configured role/type default > operator run model.
/// Roles pin no model, so there is no profile layer: a later
/// configured-model lookup cannot silently override anything.
fn resolve_spawn_model_selection(
    runtime: &SubAgentRuntime,
    request: &SpawnRequest,
) -> Result<SpawnModelSelection, ToolError> {
    if let Some(model) = request.model.as_deref() {
        let model =
            normalize_requested_subagent_model(model, "model", runtime.client.api_provider())?;
        return Ok(SpawnModelSelection {
            model_route: ModelRoute::Fixed(model),
            source: SpawnRouteSource::TaskModel,
        });
    }
    if request.model_strength_explicit {
        return Ok(SpawnModelSelection {
            model_route: request.model_strength.model_route(),
            source: SpawnRouteSource::TaskModelStrength,
        });
    }
    if let Some(model) = configured_model_for_role_or_type(
        runtime,
        request.assignment.role.as_deref(),
        &request.agent_type,
    )? {
        return Ok(SpawnModelSelection {
            model_route: ModelRoute::Fixed(model),
            source: SpawnRouteSource::RoleDefault,
        });
    }
    if request.model_strength == SubAgentModelStrength::Faster {
        return Ok(SpawnModelSelection {
            model_route: ModelRoute::Faster,
            source: SpawnRouteSource::RoleDefault,
        });
    }
    Ok(SpawnModelSelection {
        model_route: ModelRoute::Inherit,
        source: SpawnRouteSource::RunModel,
    })
}

/// Resolve caller/config model pins to the child provider's exact wire id
/// before a child reserves worktree or concurrency resources. Provider-less
/// pins also receive the conservative known-foreign check; explicit provider
/// pairs keep their deliberate route intent. Inherited/strength routes stay
/// unchanged.
///
/// #5099: the known-foreign check distinguishes who asked for the model. An
/// explicit `task.model` is the caller's deliberate pin and still fails with
/// the pin-vs-inherit error. A provider-less DEFAULT the session never chose
/// (a role/type default) must not hard-fail the spawn —
/// the child inherits the session route instead of colliding with a foreign
/// provider's bare model id, and the downgrade is logged.
fn resolve_fixed_spawn_model_route(
    runtime: &SubAgentRuntime,
    selection: &mut SpawnModelSelection,
    providerless: bool,
) -> Result<(), ToolError> {
    if !matches!(
        selection.source,
        SpawnRouteSource::TaskModel | SpawnRouteSource::RoleDefault
    ) {
        return Ok(());
    }
    let ModelRoute::Fixed(model) = &selection.model_route else {
        return Ok(());
    };
    let provider = runtime.client.api_provider();
    if providerless
        && let Err(reason) = crate::route_runtime::validate_unpinned_model_provider(
            provider,
            model,
            runtime.client.base_url(),
        )
    {
        if matches!(selection.source, SpawnRouteSource::TaskModel) {
            return Err(ToolError::invalid_input(reason));
        }
        tracing::warn!(
            model = %model,
            source = selection.source.as_str(),
            session_provider = %provider.as_str(),
            "provider-less spawn default is foreign to the session route; \
             the child inherits the session route instead ({reason})"
        );
        selection.model_route = ModelRoute::Inherit;
        selection.source = SpawnRouteSource::RunModel;
        return Ok(());
    }
    let candidate = if providerless {
        crate::route_runtime::resolve_unpinned_model_candidate(
            provider,
            model,
            runtime.client.base_url(),
        )
    } else {
        crate::route_runtime::resolve_route_candidate(
            provider,
            Some(model),
            None,
            Some(runtime.client.base_url().to_string()),
            None,
        )
    }
    .map_err(ToolError::invalid_input)?;
    selection.model_route = ModelRoute::Fixed(candidate.wire_model_id().as_str().to_string());
    Ok(())
}

/// Effective absolute `max_spawn_depth` for a child, combining the inherited
/// runtime budget and the caller's `max_depth` request. The inherited budget
/// is an immutable absolute boundary: an explicit request may never widen a
/// child past the depth the root/session selected. A request only narrows —
/// the effective depth is the minimum of the inherited budget and the clamped
/// request (#5253).
fn child_max_spawn_depth_for_spawn(
    inherited: u32,
    child_spawn_depth: u32,
    requested: Option<u32>,
) -> u32 {
    match requested {
        Some(requested) => inherited.min(clamp_child_max_spawn_depth(child_spawn_depth, requested)),
        None => inherited,
    }
}

/// Optional aliased boolean, refusing a wrong type instead of dropping it.
///
/// `{"deliberate": "true"}` used to coerce to the default `false` and skip
/// the whole deliberate-delegation contract in silence.
fn parse_optional_bool(input: &Value, names: &[&str]) -> Result<Option<bool>, ToolError> {
    let Some((name, value)) = aliased_value(input, names) else {
        return Ok(None);
    };
    value
        .as_bool()
        .map(Some)
        .ok_or_else(|| codewhale_tools::type_mismatch(name, value, "a boolean"))
}

/// Parse an optional caller-supplied `disallowed_tools` array (#4042). Mirrors
/// the `allowed_tools` parsing: trimmed, de-duplicated, non-empty-only. Returns
/// `None` when the key is absent or yields no usable entries so the union merge
/// in `spawn_subagent_from_input` only runs when there is something to add.
fn parse_disallowed_tools(input: &Value) -> Result<Option<Vec<String>>, ToolError> {
    Ok(parse_tool_name_list(input, "disallowed_tools")?.filter(|tools| !tools.is_empty()))
}

fn parse_optional_positive_u64(input: &Value, names: &[&str]) -> Result<Option<u64>, ToolError> {
    for name in names {
        let Some(value) = input.get(*name) else {
            continue;
        };
        let Some(parsed) = value.as_u64() else {
            return Err(ToolError::invalid_input(format!(
                "{name} must be a positive integer token count"
            )));
        };
        if parsed == 0 {
            return Err(ToolError::invalid_input(format!(
                "{name} must be greater than zero; omit it to inherit or disable the budget"
            )));
        }
        return Ok(Some(parsed));
    }
    Ok(None)
}

#[cfg(test)]
fn with_default_fork_context(mut input: Value, default: bool) -> Value {
    let Some(object) = input.as_object_mut() else {
        return input;
    };
    if !object.contains_key("fork_context")
        && !object.contains_key("forkContext")
        && !object.contains_key("inherit_context")
    {
        object.insert("fork_context".to_string(), Value::Bool(default));
    }
    input
}

pub(crate) fn normalize_requested_subagent_model(
    value: &str,
    field: &str,
    provider: crate::config::ApiProvider,
) -> Result<String, ToolError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ToolError::invalid_input(format!("{field} cannot be blank")));
    }
    // #3018: Use provider-aware validation so non-DeepSeek providers can
    // accept their own model IDs instead of failing with "Expected a
    // DeepSeek model id".
    let normalized =
        crate::config::requested_model_for_provider(provider, trimmed).ok_or_else(|| {
            let valid_names = crate::provider_lake::all_catalog_models_for_provider(provider);
            let valid_hint = if valid_names.is_empty() {
                String::new()
            } else {
                format!(" (accepted: {})", valid_names.join(", "))
            };
            ToolError::invalid_input(format!(
                "Invalid {field} '{trimmed}' for provider {}{valid_hint}",
                provider_name_for_error(provider)
            ))
        })?;
    crate::config::validate_route(provider, &normalized).map_err(ToolError::invalid_input)?;
    Ok(normalized)
}

fn provider_name_for_error(provider: crate::config::ApiProvider) -> &'static str {
    // Reuse the canonical picker/status label so every provider is named
    // concretely (DeepSeek, Sakana, Zhipu, …) instead of collapsing the long
    // tail to "this provider", and so error copy stays in sync with the model
    // picker labels (#4049).
    provider.display_name()
}

pub(crate) fn configured_model_for_role_or_type(
    runtime: &SubAgentRuntime,
    role: Option<&str>,
    agent_type: &FleetRole,
) -> Result<Option<String>, ToolError> {
    let mut keys = Vec::new();
    let mut push_key = |key: String| {
        if !keys.contains(&key) {
            keys.push(key);
        }
    };
    if let Some(role) = role.map(str::trim).filter(|role| !role.is_empty()) {
        let normalized = role.to_ascii_lowercase();
        push_key(
            migrate_legacy_role_token(&normalized)
                .unwrap_or(normalized.as_str())
                .to_string(),
        );
        push_key(normalized);
    }
    push_key(agent_type.as_str().to_string());
    if agent_type.legacy_type_name() != agent_type.as_str() {
        push_key(agent_type.legacy_type_name().to_string());
    }
    // `[subagents.models].oracle` shipped before the public role was renamed.
    // Keep both historical advisory keys readable, but never expose them in
    // schemas, prompts, receipts, or newly serialized role values.
    if *agent_type == FleetRole::Consultant {
        push_key("oracle".to_string());
        push_key("advisor".to_string());
    }
    push_key("default".to_string());

    for key in keys {
        if let Some(model) = runtime.role_models.get(&key) {
            return normalize_requested_subagent_model(
                model,
                &format!("subagents.{key}.model"),
                runtime.client.api_provider(),
            )
            .map(Some);
        }
    }
    Ok(None)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubAgentResolvedRoute {
    pub(crate) model_route: ModelRoute,
    pub(crate) model: String,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) tuning: RequestTuning,
}

impl SubAgentResolvedRoute {
    fn new(
        model_route: ModelRoute,
        model: String,
        reasoning_effort: Option<String>,
    ) -> SubAgentResolvedRoute {
        let tuning = subagent_request_tuning(reasoning_effort.as_deref());
        SubAgentResolvedRoute {
            model_route,
            model,
            reasoning_effort,
            tuning,
        }
    }
}

pub(crate) async fn resolve_subagent_assignment_route(
    runtime: &SubAgentRuntime,
    configured_model: Option<String>,
    prompt: &str,
    agent_type: &FleetRole,
    requested_model_route: ModelRoute,
    requested_thinking: SubAgentThinking,
) -> SubAgentResolvedRoute {
    let model_route = assignment_model_route(configured_model.as_deref(), requested_model_route);
    worker_profile_subagent_assignment_route(
        runtime,
        &model_route,
        requested_thinking,
        prompt,
        agent_type,
    )
}

fn assignment_model_route(
    configured_model: Option<&str>,
    requested_model_route: ModelRoute,
) -> ModelRoute {
    if let Some(model) = configured_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        return ModelRoute::Fixed(model.to_string());
    }

    requested_model_route
}

fn subagent_request_tuning(reasoning_effort: Option<&str>) -> RequestTuning {
    RequestTuning {
        reasoning_effort: reasoning_effort.map(ReasoningEffort::from_setting),
        max_output_tokens: None,
    }
}

/// Candidate pair for explicit sub-agent strength routing, derived from the
/// active provider and the already provider-resolved parent model.
fn subagent_router_candidates(runtime: &SubAgentRuntime) -> crate::model_routing::RouterCandidates {
    crate::model_routing::provider_router_candidates(runtime.client.api_provider(), &runtime.model)
}

#[cfg(test)]
fn fallback_subagent_assignment_route(
    runtime: &SubAgentRuntime,
    configured_model: Option<String>,
    requested_model_route: ModelRoute,
    requested_thinking: SubAgentThinking,
    prompt: &str,
) -> SubAgentResolvedRoute {
    let model_route = assignment_model_route(configured_model.as_deref(), requested_model_route);
    worker_profile_subagent_assignment_route(
        runtime,
        &model_route,
        requested_thinking,
        prompt,
        &FleetRole::Worker,
    )
}

/// Operator-visible model for the active provider when inherit/faster routing
/// must not cross namespaces (#3227, subagent route validation 2026-07-07).
///
/// Enumerates through the catalog-backed [`crate::provider_lake`] facade rather
/// than the raw legacy `model_completion_names_for_provider` table (#4116 /
/// #4188). The facade prefers live Models.dev, then the offline bundled
/// snapshot, and only then the legacy hardcoded table for Codewhale-only /
/// unbundled providers. This consumer only reads the first entry.
fn operator_model_for_subagent(runtime: &SubAgentRuntime) -> String {
    let provider = runtime.client.api_provider();
    if crate::config::validate_route(provider, &runtime.model).is_ok() {
        return runtime.model.clone();
    }
    crate::provider_lake::all_catalog_models_for_provider(provider)
        .into_iter()
        .next()
        .unwrap_or_else(|| runtime.model.clone())
}

/// Reject or remap a resolved sub-agent model so it matches the runtime
/// provider before spawn. Explicit fixed pins fail fast; inherit/faster/auto
/// fall back to the operator route instead of cross-wiring namespaces.
pub(crate) fn ensure_subagent_model_for_provider(
    runtime: &SubAgentRuntime,
    model_route: &ModelRoute,
    model: String,
) -> Result<String, ToolError> {
    let provider = runtime.client.api_provider();
    if crate::config::validate_route(provider, &model).is_ok() {
        return Ok(model);
    }
    match model_route {
        ModelRoute::Inherit | ModelRoute::Faster | ModelRoute::Auto => {
            Ok(operator_model_for_subagent(runtime))
        }
        ModelRoute::Fixed(_) => Err(ToolError::invalid_input(
            crate::config::validate_route(provider, &model).unwrap_err(),
        )),
    }
}

fn worker_profile_subagent_assignment_route(
    runtime: &SubAgentRuntime,
    model_route: &ModelRoute,
    requested_thinking: SubAgentThinking,
    prompt: &str,
    agent_type: &FleetRole,
) -> SubAgentResolvedRoute {
    let candidates = subagent_router_candidates(runtime);
    let mut requested_fast_lane = false;
    let model = match model_route {
        ModelRoute::Fixed(model) => model.clone(),
        ModelRoute::Faster | ModelRoute::Auto => {
            requested_fast_lane = true;
            candidates
                .cheap
                .clone()
                .unwrap_or_else(|| runtime.model.clone())
        }
        ModelRoute::Inherit => runtime.model.clone(),
    };

    let role_reasoning_default =
        WorkerRuntimeProfile::for_role(agent_type.clone()).reasoning_effort;
    let reasoning_effort = subagent_reasoning_effort_for_request(
        runtime,
        &model,
        prompt,
        requested_fast_lane,
        requested_thinking,
        role_reasoning_default.as_deref(),
    );

    SubAgentResolvedRoute::new(model_route.clone(), model, reasoning_effort)
}

fn subagent_reasoning_effort_for_request(
    runtime: &SubAgentRuntime,
    model: &str,
    prompt: &str,
    requested_fast_lane: bool,
    requested_thinking: SubAgentThinking,
    role_reasoning_default: Option<&str>,
) -> Option<String> {
    let normalize = |effort: ReasoningEffort| {
        effort.normalize_for_route(
            runtime.client.api_provider(),
            runtime.client.base_url(),
            model,
        )
    };
    match requested_thinking {
        SubAgentThinking::Effort(effort) => Some(normalize(effort).as_setting().to_string()),
        SubAgentThinking::Auto => Some(
            normalize(auto_subagent_reasoning_effort(prompt))
                .as_setting()
                .to_string(),
        ),
        // A role default is child intent, not inherited parent state. It wins
        // whenever the caller left thinking at `inherit`, while explicit
        // Effort/Auto requests above still take precedence. Route
        // normalization remains the final capability ceiling.
        SubAgentThinking::Inherit if role_reasoning_default.is_some() => role_reasoning_default
            .map(ReasoningEffort::from_setting)
            .map(normalize)
            .map(|effort| effort.as_setting().to_string()),
        SubAgentThinking::Inherit if requested_fast_lane => {
            // Faster/explore lane: cheaper reasoning by default. The OpenAI Codex
            // (GPT-5.5) adapter has no true "off" on the wire (it collapses off
            // to low), so we resolve Low honestly for that provider instead of
            // emitting an off that is silently rewritten. Explicit thinking
            // passed by the caller already won via the arms above.
            let provider = runtime.client.api_provider();
            let effort = if matches!(provider, crate::config::ApiProvider::OpenaiCodex) {
                ReasoningEffort::Low
            } else {
                ReasoningEffort::Off
            };
            Some(normalize(effort).as_setting().to_string())
        }
        SubAgentThinking::Inherit => fallback_subagent_reasoning_effort(runtime, model, prompt),
    }
}

fn fallback_subagent_reasoning_effort(
    runtime: &SubAgentRuntime,
    model: &str,
    prompt: &str,
) -> Option<String> {
    let normalize = |effort: ReasoningEffort| {
        effort.normalize_for_route(
            runtime.client.api_provider(),
            runtime.client.base_url(),
            model,
        )
    };
    // `reasoning_effort_auto` is the flag; a literal `"auto"` tier is the same
    // request by another spelling. A fixed-model Fleet subprocess arrives with
    // the string and no flag, so treating only the flag as Auto left `"auto"`
    // to travel raw to a provider that has no such tier.
    let requested_auto = runtime.reasoning_effort_auto
        || runtime
            .reasoning_effort
            .as_deref()
            .is_some_and(|effort| ReasoningEffort::from_setting(effort) == ReasoningEffort::Auto);
    if requested_auto {
        Some(
            normalize(auto_subagent_reasoning_effort(prompt))
                .as_setting()
                .to_string(),
        )
    } else {
        runtime
            .reasoning_effort
            .as_deref()
            .map(ReasoningEffort::from_setting)
            .map(normalize)
            .map(|effort| effort.as_setting().to_string())
    }
}

fn auto_subagent_reasoning_effort(prompt: &str) -> ReasoningEffort {
    crate::auto_reasoning::select(false, prompt)
}

fn parse_optional_subagent_model(input: &Value, key: &str) -> Result<Option<String>, ToolError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Err(ToolError::invalid_input(format!("{key} cannot be blank")));
            }
            // #3018: Basic parsing only — provider-aware validation is deferred
            // to the spawn path where the runtime's ApiProvider is available.
            Ok(Some(trimmed.to_string()))
        }
        Some(_) => Err(ToolError::invalid_input(format!("{key} must be a string"))),
    }
}

/// Extract an optional `cwd: String` from spawn input and convert to a
/// `PathBuf`. Empty / absent → `None`. Workspace-boundary check happens
/// at spawn time (the parent's workspace is known there, not here).
fn parse_optional_cwd(input: &Value) -> Result<Option<PathBuf>, ToolError> {
    let raw = input.get("cwd").and_then(|v| v.as_str()).map(str::trim);
    match raw {
        None | Some("") => Ok(None),
        Some(s) => Ok(Some(PathBuf::from(s))),
    }
}

fn parse_optional_worktree_request(
    input: &Value,
) -> Result<Option<SubAgentWorktreeRequest>, ToolError> {
    let worktree_flag =
        parse_optional_bool(input, &["worktree", "isolate_worktree", "isolateWorktree"])?;
    let isolation = optional_input_str(input, &["isolation"])?
        .map(|value| value.trim().to_ascii_lowercase().replace(['_', '-'], ""));
    let isolation_wants_worktree = match isolation.as_deref() {
        None | Some("") | Some("none") | Some("shared") => false,
        Some("worktree") | Some("gitworktree") => true,
        Some(other) => {
            return Err(ToolError::invalid_input(format!(
                "isolation must be 'worktree' or 'none' (got '{other}')"
            )));
        }
    };

    let branch = optional_input_str(
        input,
        &[
            "worktree_branch",
            "worktreeBranch",
            "branch_name",
            "branchName",
            "branch",
        ],
    )?
    .map(str::to_string);
    let path = optional_input_str(
        input,
        &[
            "worktree_path",
            "worktreePath",
            "worktree_dir",
            "worktreeDir",
        ],
    )?
    .map(PathBuf::from);
    let base_ref = optional_input_str(
        input,
        &["worktree_base", "worktreeBase", "base_ref", "baseRef"],
    )?
    .map(str::to_string);

    let has_worktree_details = branch.is_some() || path.is_some() || base_ref.is_some();
    if worktree_flag == Some(false) && (isolation_wants_worktree || has_worktree_details) {
        return Err(ToolError::invalid_input(
            "worktree=false conflicts with worktree isolation options".to_string(),
        ));
    }
    if worktree_flag.unwrap_or(false) || isolation_wants_worktree || has_worktree_details {
        Ok(Some(SubAgentWorktreeRequest {
            branch,
            path,
            base_ref,
        }))
    } else {
        Ok(None)
    }
}

/// Resolve a user-supplied role/agent_role value to a canonical role string.
///
/// This must accept the full set that [`FleetRole::from_str`] accepts, plus
/// role-only aliases (`worker`, `default`, `awaiter`). Before #2649 it covered
/// only a subset, so `role: "reviewer"` (accepted by `from_str`) was rejected
/// here by the second validation pass with a misleading four-value hint.
fn normalize_role_alias(input: &str) -> Option<&'static str> {
    match input.to_ascii_lowercase().as_str() {
        "default" | "worker" | "general" | "general-purpose" | "general_purpose" => Some("general"),
        "scout" | "explorer" | "explore" | "exploration" => Some("explore"),
        "awaiter" | "plan" | "planner" | "planning" => Some("planner"),
        "reviewer" | "review" | "code-review" | "code_review" => Some("reviewer"),
        "implementer" | "implement" | "implementation" | "builder" => Some("implement"),
        "verifier" | "verify" | "verification" | "validator" | "tester" => Some("test"),
        "consultant" | "oracle" | "advisor" => Some("advisor"),
        "custom" => Some("custom"),
        _ => None,
    }
}

fn build_assignment_prompt(
    prompt: &str,
    assignment: &SubAgentAssignment,
    agent_type: &FleetRole,
) -> String {
    let role = assignment
        .role
        .as_deref()
        .map(|role| normalize_role_alias(role).unwrap_or(role))
        .unwrap_or("default");
    format!(
        "Assignment metadata:\n- objective: {}\n- role: {}\n- resolved_type: {}\n\nTask:\n{}",
        assignment.objective,
        role,
        agent_type.as_str(),
        prompt
    )
}

fn worker_status_from_subagent_status(status: &SubAgentStatus) -> AgentWorkerStatus {
    match status {
        SubAgentStatus::Running => AgentWorkerStatus::Running,
        SubAgentStatus::Completed => AgentWorkerStatus::Completed,
        SubAgentStatus::Failed(_) => AgentWorkerStatus::Failed,
        SubAgentStatus::Cancelled => AgentWorkerStatus::Cancelled,
        SubAgentStatus::BudgetExhausted => AgentWorkerStatus::Failed,
        SubAgentStatus::Interrupted(_) => AgentWorkerStatus::Interrupted,
    }
}

pub fn agent_worker_status_name(status: AgentWorkerStatus) -> &'static str {
    match status {
        AgentWorkerStatus::Queued => "queued",
        AgentWorkerStatus::Starting => "starting",
        AgentWorkerStatus::Running => "running",
        AgentWorkerStatus::WaitingForUser => "waiting_for_user",
        AgentWorkerStatus::ModelWait => "model_wait",
        AgentWorkerStatus::RunningTool => "running_tool",
        AgentWorkerStatus::Completed => "completed",
        AgentWorkerStatus::Failed => "failed",
        AgentWorkerStatus::Cancelled => "cancelled",
        AgentWorkerStatus::Interrupted => "interrupted",
    }
}

fn worker_status_from_subagent_result(result: &SubAgentResult) -> AgentWorkerStatus {
    if subagent_checkpoint_is_continuable(result) {
        AgentWorkerStatus::WaitingForUser
    } else {
        worker_status_from_subagent_status(&result.status)
    }
}

pub(crate) fn subagent_progress_tool_display_name(name: &str) -> &str {
    match name {
        "exec_shell"
        | "exec_shell_wait"
        | "exec_shell_interact"
        | "exec_shell_cancel"
        | "exec_wait"
        | "exec_interact"
        | "task_shell_start"
        | "task_shell_wait" => "bash",
        _ => name,
    }
}

fn emit_agent_progress(
    event_tx: Option<&mpsc::Sender<Event>>,
    owner_session_id: &str,
    agent_id: &str,
    status: String,
    activity: AgentProgressEventMeta,
    parent_run_id: Option<String>,
    spawn_depth: u32,
) {
    if let Some(event_tx) = event_tx {
        if event_tx.max_capacity() > MIN_EVENT_CHANNEL_HEADROOM_FOR_ROUTINE_PROGRESS
            && event_tx.capacity() <= MIN_EVENT_CHANNEL_HEADROOM_FOR_ROUTINE_PROGRESS
            && routine_agent_progress_can_preserve_event_headroom(activity.worker_status)
        {
            return;
        }
        let _ = event_tx.try_send(Event::AgentProgress {
            owner_session_id: owner_session_id.to_string(),
            id: agent_id.to_string(),
            status,
            activity,
            parent_run_id,
            spawn_depth,
        });
    }
}

fn routine_agent_progress_can_preserve_event_headroom(status: AgentWorkerStatus) -> bool {
    matches!(
        status,
        AgentWorkerStatus::Running | AgentWorkerStatus::ModelWait | AgentWorkerStatus::RunningTool
    )
}

// === Tool Registry Helpers ===

/// Request projection over one child's independently filtered full registry.
///
/// The forked transcript and instructions remain model context, but never grant
/// tools or seed authority. Each child starts with its own empty activation
/// cache and may discover anything that survives that child's role, scope,
/// depth, and execution-envelope filters. Search and hydration can only admit
/// names from this filtered catalog; they cannot make a denied tool executable.
/// The active-name snapshot is rebuilt once per model request so a same-batch
/// guessed call cannot use a schema that was not present in that request.
struct SubAgentToolSurface {
    catalog: Vec<Tool>,
    active_names: std::collections::HashSet<String>,
    cache: ToolActivationCache,
}

impl SubAgentToolSurface {
    fn new(catalog: Vec<Tool>, warm_names: &[String]) -> Self {
        let mut cache = ToolActivationCache::default();
        let activation = cache.activate(&catalog, warm_names);
        let mut active_names = initial_active_tools(&catalog);
        active_names.extend(activation.admitted);
        Self {
            catalog,
            active_names,
            cache,
        }
    }

    fn request_tools(&mut self, catalog: Vec<Tool>, strict: bool) -> Vec<Tool> {
        self.catalog = catalog;
        self.cache.revalidate(&self.catalog);
        let mut active_names = initial_active_tools(&self.catalog);
        active_names.extend(self.cache.names().map(str::to_string));
        self.active_names = active_names;
        active_tools_for_request(&self.catalog, &self.active_names, strict).unwrap_or_default()
    }

    fn search(&mut self, name: &str, input: &Value) -> Result<String> {
        execute_tool_search_with_cache(
            name,
            input,
            &self.catalog,
            &mut self.active_names,
            &mut self.cache,
        )
        .map(|result| result.content)
        .map_err(|error| anyhow!(error))
    }

    fn hydrate(&mut self, name: &str) -> Result<String> {
        let activation = self.cache.activate(&self.catalog, &[name.to_string()]);
        remove_evicted_cache_activations(&self.catalog, &mut self.active_names, activation.evicted);
        self.active_names
            .extend(activation.admitted.iter().cloned());
        if activation.admitted.iter().any(|admitted| admitted == name) {
            return Ok(format!(
                "Tool `{name}` was deferred and has now been loaded. Retry the call with the newly available schema."
            ));
        }
        Err(anyhow!(
            "Tool {name} could not enter the bounded child toolbox; use tool_search with a narrower query"
        ))
    }
}

/// Role-only approval posture; the registry allow/deny and runtime envelope
/// remain the authoritative per-tool checks.
///
/// `Auto` is safe for every role. `Suggest` requires a write-capable role;
/// `Required` requires the full-shell posture. A Custom worker may describe
/// either posture, but its explicit allowlist and the runtime envelope still
/// narrow what it can execute. Read-only bash operations are classified Auto,
/// so Scouts and Reviewers do not gain general shell authority through this
/// branch.
fn role_posture_permits(agent_type: &FleetRole, approval: ApprovalRequirement) -> bool {
    if matches!(agent_type, FleetRole::Custom) {
        return true;
    }
    let profile = WorkerRuntimeProfile::for_role(agent_type.clone());
    match approval {
        ApprovalRequirement::Auto => true,
        ApprovalRequirement::Suggest => profile.permissions.write,
        ApprovalRequirement::Required => {
            matches!(profile.shell, crate::worker_profile::ShellPolicy::Full)
        }
    }
}

/// Intersect an explicit parent scope with the child's requested subset.
///
/// A child can narrow an explicit parent scope, never widen it. Omitting the
/// child list inherits the parent's exact scope. A parent with the default
/// role-defined scope may accept a child list, but the role and envelope gates
/// below remain authoritative at visibility and dispatch time.
fn intersect_explicit_tool_scope(
    parent: &ToolScope,
    child: Option<Vec<String>>,
) -> Option<Vec<String>> {
    let ToolScope::Explicit(parent) = parent else {
        return child;
    };
    let Some(child) = child else {
        return Some(parent.clone());
    };
    Some(
        child
            .into_iter()
            .filter(|name| explicit_scope_permits(parent, name))
            .collect(),
    )
}

/// Family, legacy action, and lowercase primitive spellings are equivalent.
/// This is intentionally case-insensitive so a saved `File`/`Bash` scope and
/// the current `read`/`write`/`edit`/`bash` names describe the same authority.
fn explicit_scope_permits(parent: &[String], name: &str) -> bool {
    parent
        .iter()
        .any(|allowed| policy_tool_name_matches(allowed, name))
}

fn policy_tool_name_matches(rule: &str, name: &str) -> bool {
    let empty = Value::Null;
    let semantic_rule = canonical_action_alias(rule, &empty);
    let semantic_name = canonical_action_alias(name, &empty);
    rule.eq_ignore_ascii_case(name)
        || semantic_rule.eq_ignore_ascii_case(semantic_name)
        || CANONICAL_ACTION_ALIASES.iter().any(|(family, _, alias)| {
            rule.eq_ignore_ascii_case(family) && semantic_name.eq_ignore_ascii_case(alias)
                || name.eq_ignore_ascii_case(family) && semantic_rule.eq_ignore_ascii_case(alias)
        })
}

struct SubAgentToolRegistry {
    // `None` means the role-defined surface; `Some` is the already-intersected
    // parent/child scope. Deny rules always win, including canonical prefixes.
    allowed_tools: Option<Vec<String>>,
    disallowed_tools: Vec<String>,
    // Approval posture is separate from authority. Auto approval can remove a
    // prompt, but cannot restore a tool removed by role, scope, or envelope.
    auto_approve: bool,
    accept_edits: bool,
    agent_type: FleetRole,
    runtime_profile: WorkerRuntimeProfile,
    // Depth is the only special authority governing nested `agent` calls.
    can_spawn_child: bool,
    // Every mutation is attributed to the child and checked against its live
    // coordination claim before the underlying registry sees the call.
    owner_agent_id: String,
    owner_agent_name: String,
    coordination_manager: SharedSubAgentManager,
    enforce_write_claim: bool,
    registry: ToolRegistry,
    /// The session posture and reviewer wiring this child is gated by (see
    /// [`SubAgentToolRegistry::gate_held_call`]). Cloned from the spawning
    /// runtime so a child is gated exactly like the parent turn.
    gate_runtime: SubAgentRuntime,
}

/// What the child permission gate decided for one held call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ChildGateVerdict {
    /// Run the call.
    Proceed,
    /// Refuse the call with this reason (returned to the child model).
    Deny(String),
}

impl SubAgentToolRegistry {
    const ACTION_ALIASES: &'static [(&'static str, &'static str, &'static str)] =
        CANONICAL_ACTION_ALIASES;

    #[cfg(test)]
    fn new(
        runtime: SubAgentRuntime,
        agent_type: FleetRole,
        explicit_allowed_tools: Option<Vec<String>>,
        todo_list: SharedTodoList,
        plan_state: SharedPlanState,
    ) -> Self {
        let mut registry = Self::new_with_owner(
            runtime,
            agent_type,
            "agent_unknown".to_string(),
            "sub-agent".to_string(),
            explicit_allowed_tools,
            todo_list,
            plan_state,
        );
        registry.enforce_write_claim = false;
        registry
    }

    fn new_with_owner(
        runtime: SubAgentRuntime,
        agent_type: FleetRole,
        owner_agent_id: String,
        owner_agent_name: String,
        explicit_allowed_tools: Option<Vec<String>>,
        todo_list: SharedTodoList,
        plan_state: SharedPlanState,
    ) -> Self {
        let can_spawn_child = !runtime.would_exceed_depth();
        let coordination_manager = Arc::clone(&runtime.manager);
        let mut surface_options = runtime.agent_tool_surface_options.clone();
        // Shell authority is an intersection: a parent cannot delegate more
        // than it owns, and read-only inspection roles (Scout, Reviewer) are
        // narrowed to the hardened read-only classifier even when the parent
        // has a full shell.
        let parent_shell = ShellPolicy::from_legacy_allow_shell(runtime.allow_shell);
        let mut child_shell = runtime.worker_profile.shell.min_with(parent_shell);
        if crate::fleet::role::role_requires_read_only_shell(&agent_type)
            && child_shell.allows_shell()
        {
            child_shell = ShellPolicy::ReadOnly;
        }
        let mut effective_profile = runtime.worker_profile.clone();
        effective_profile.shell = child_shell;
        let allowed_tools =
            intersect_explicit_tool_scope(&effective_profile.tools, explicit_allowed_tools);
        surface_options.shell_policy = child_shell;
        let context = runtime.context.clone().with_shell_policy(child_shell);
        let mut child_runtime = runtime.clone();
        child_runtime.parent_agent_id = Some(owner_agent_id.clone());
        child_runtime.worker_profile = effective_profile.clone();
        let mut registry = ToolRegistryBuilder::new().with_full_agent_surface_options(
            Some(runtime.client.clone()),
            runtime.model.clone(),
            runtime.manager.clone(),
            child_runtime,
            surface_options,
            todo_list,
            plan_state,
        );

        if let Some(pool) = runtime.mcp_pool.as_ref() {
            registry = registry.with_mcp_tools(std::sync::Arc::clone(pool));
        }

        let mut registry = registry.build(context);
        // Goals belong to the root conversation. Registering the complete child
        // surface first keeps every other tool discoverable, then these two
        // root-only mutations are removed before catalog filtering and search.
        registry.remove_tool("create_goal");
        registry.remove_tool("update_goal");

        Self {
            allowed_tools,
            disallowed_tools: effective_profile.denied_tools.clone(),
            auto_approve: runtime.context.auto_approve,
            accept_edits: runtime.accept_edits,
            agent_type,
            runtime_profile: effective_profile,
            can_spawn_child,
            owner_agent_id,
            owner_agent_name,
            coordination_manager,
            enforce_write_claim: true,
            registry,
            gate_runtime: runtime,
        }
    }

    /// The refusal the pre-posture registry applied to an approval-gated call
    /// when the parent was not Full Access: read-only roles cannot write,
    /// arbitrary shell needs the parent auto-approved. `None` when the call
    /// clears the role-based delegation rules on its own.
    fn delegation_refusal(&self, name: &str, input: &Value) -> Option<String> {
        // #5595/#5438: session approval is transport, not a second shell
        // classifier. Scout/Reviewer/Planner calls already proven by the exact
        // predicate used at posture, envelope, and execute boundaries are
        // delegated reads even though lowercase `bash` deliberately keeps the
        // stricter parent-parallel classifier as its generic approval baseline.
        if self.bounded_readonly_bash_evidence(name, input) {
            return None;
        }
        let spec = self.registry.get(name)?;
        match spec.approval_requirement_for(input) {
            ApprovalRequirement::Auto => None,
            ApprovalRequirement::Suggest => {
                // Write/edit/patch tools land here. Explicit write-capable
                // roles (`builder`, `custom`) may run them without parent
                // auto-approve (#1828, #1833). Workflow-spawned children also
                // accept Suggest edits for any write-capable posture. #5186:
                // children inherit the session's in-workspace write carve-out
                // (#5185) too.
                let may_write = self.runtime_profile.permissions.write
                    && (self.accept_edits || Self::role_can_delegate_writes(&self.agent_type));
                (!may_write && !self.workspace_write_carve_out_permits(name, input)).then(|| {
                    format!(
                        "Tool {name} requires approval and is not delegated to {role} sub-agents; pick a write-capable role or approve it from the session",
                        role = self.agent_type.as_str()
                    )
                })
            }
            ApprovalRequirement::Required => {
                // #5186: the bounded built-in verification surface is
                // delegated to any shell-capable child; arbitrary shell and
                // every other Required tool stay gated.
                (!Self::is_delegated_builtin_verification(name, input)).then(|| {
                    format!(
                        "Tool {name} requires approval and cannot run inside this sub-agent without a session decision"
                    )
                })
            }
        }
    }

    /// Emit a transcript-visible receipt for a decision made on this child's
    /// call without a person seeing a prompt (the audit log has the record).
    #[allow(clippy::too_many_arguments)]
    async fn emit_child_gate_receipt(
        &self,
        agent_id: &str,
        tool_id: &str,
        name: &str,
        gate: crate::core::events::ToolGate,
        decision: crate::core::events::ToolGateVerdict,
        risk: Option<&str>,
        reason: &str,
    ) {
        crate::core::engine::emit_tool_audit(json!({
            "event": "tool.auto_review",
            "gate": gate.as_str(),
            "agent_id": agent_id,
            "tool_id": tool_id,
            "tool_name": name,
            "decision": decision.as_str(),
            "risk": risk,
            "reason": reason,
        }));
        if let Some(tx) = self.gate_runtime.event_tx.as_ref() {
            let _ = tx
                .send(Event::ToolGateDecision {
                    agent_id: Some(agent_id.to_string()),
                    tool_id: tool_id.to_string(),
                    tool_name: name.to_string(),
                    gate,
                    decision,
                    risk: risk.map(str::to_string),
                    reason: crate::core::events::bounded_gate_reason(reason),
                })
                .await;
        }
    }

    /// Apply the session's permission posture to one of this child's tool
    /// calls, exactly as the parent turn applies it to its own:
    ///
    /// - **Full Access**: ordinary calls run; the non-bypassable safety floor
    ///   (publish-like, destructive background work) still fails closed.
    /// - **Auto-Review**: the deterministic policy allows proven-safe calls
    ///   and blocks the floor; anything it cannot prove safe goes to the same
    ///   one-shot model guardian the parent uses. No prompt is ever opened;
    ///   an unavailable guardian denies (fail closed).
    /// - **Ask**: a call the role may delegate runs; a held call is raised as
    ///   an approval prompt in the parent's UI when the host can answer one
    ///   (the child waits, visibly, as `waiting for user`); otherwise it is
    ///   denied with the reason.
    /// - **never**: held calls are denied.
    ///
    /// Every decision a person did not make at a prompt is written to the
    /// audit log and, through [`Event::ToolGateDecision`], into this child's
    /// transcript. Role posture and the execution envelope are checked by the
    /// caller before and after; this gate never widens them.
    async fn gate_held_call(
        &self,
        agent_id: &str,
        tool_id: &str,
        name: &str,
        input: &Value,
    ) -> ChildGateVerdict {
        use crate::core::engine::{AutoReviewPlanDecision, auto_review_plan_decision_for_context};
        use crate::core::events::{ToolGate, ToolGateVerdict};
        use crate::tui::approval::ApprovalMode;
        use crate::tui::auto_review::{AutoReviewContext, RunOrigin};

        let approval_mode = if self.auto_approve {
            ApprovalMode::Bypass
        } else {
            self.gate_runtime.approval_mode
        };
        let workspace = self.gate_runtime.context.workspace.clone();
        let workspace_trusted = crate::config::is_workspace_trusted(&workspace);
        // Children are background workers: destructive detached work holds
        // in every posture, exactly as it does for a detached parent start.
        let review_context = AutoReviewContext::from_tool_call(
            name,
            input,
            RunOrigin::Background,
            approval_mode,
            workspace_trusted,
            Some(&workspace),
        );
        let (decision, _audit) = auto_review_plan_decision_for_context(
            &self.gate_runtime.auto_review_policy,
            &review_context,
        );

        match approval_mode {
            ApprovalMode::Bypass => match decision {
                AutoReviewPlanDecision::Block(reason) => {
                    self.emit_child_gate_receipt(
                        agent_id,
                        tool_id,
                        name,
                        ToolGate::AutoReviewDeterministic,
                        ToolGateVerdict::Denied,
                        None,
                        &reason,
                    )
                    .await;
                    ChildGateVerdict::Deny(reason)
                }
                // Full Access has no prompt to force; the plan decision
                // already turned every safety-floor hold into a block.
                AutoReviewPlanDecision::ForcePrompt(reason) => ChildGateVerdict::Deny(reason),
                AutoReviewPlanDecision::NoChange
                | AutoReviewPlanDecision::Allow
                | AutoReviewPlanDecision::ConsultReviewer(_) => ChildGateVerdict::Proceed,
            },
            ApprovalMode::Auto => match decision {
                AutoReviewPlanDecision::Allow | AutoReviewPlanDecision::NoChange => {
                    ChildGateVerdict::Proceed
                }
                AutoReviewPlanDecision::Block(reason)
                | AutoReviewPlanDecision::ForcePrompt(reason) => {
                    self.emit_child_gate_receipt(
                        agent_id,
                        tool_id,
                        name,
                        ToolGate::AutoReviewDeterministic,
                        ToolGateVerdict::Denied,
                        None,
                        &reason,
                    )
                    .await;
                    ChildGateVerdict::Deny(reason)
                }
                AutoReviewPlanDecision::ConsultReviewer(held_reason) => {
                    self.consult_child_guardian(
                        agent_id,
                        tool_id,
                        name,
                        input,
                        &review_context,
                        &held_reason,
                    )
                    .await
                }
            },
            ApprovalMode::Suggest | ApprovalMode::Never => {
                // The deterministic floor still hard-blocks what it blocks for
                // the parent (publish-like, destructive background work).
                if let AutoReviewPlanDecision::Block(reason) = &decision {
                    self.emit_child_gate_receipt(
                        agent_id,
                        tool_id,
                        name,
                        ToolGate::AutoReviewDeterministic,
                        ToolGateVerdict::Denied,
                        None,
                        reason,
                    )
                    .await;
                    return ChildGateVerdict::Deny(reason.clone());
                }
                let force_prompt = matches!(decision, AutoReviewPlanDecision::ForcePrompt(_));
                let Some(refusal) = self.delegation_refusal(name, input) else {
                    if !force_prompt {
                        return ChildGateVerdict::Proceed;
                    }
                    // A safety-floor hold on a call the role could otherwise
                    // delegate: a person still has to decide it.
                    let AutoReviewPlanDecision::ForcePrompt(reason) = decision else {
                        unreachable!("force_prompt implies ForcePrompt");
                    };
                    return self
                        .prompt_parent_for_child_call(agent_id, name, input, &reason, true)
                        .await;
                };
                if approval_mode == ApprovalMode::Never {
                    return ChildGateVerdict::Deny(refusal);
                }
                self.prompt_parent_for_child_call(agent_id, name, input, &refusal, force_prompt)
                    .await
            }
        }
    }

    /// Auto-Review: ask the one-shot model guardian about a held call, using
    /// the child's own session client, and turn its answer into a verdict
    /// plus a transcript receipt. Any failure denies (fail closed).
    async fn consult_child_guardian(
        &self,
        agent_id: &str,
        tool_id: &str,
        name: &str,
        input: &Value,
        review_context: &crate::tui::auto_review::AutoReviewContext<'_>,
        held_reason: &str,
    ) -> ChildGateVerdict {
        use crate::core::engine::reviewer::{ReviewerOutcome, consult_reviewer};
        use crate::core::events::{ToolGate, ToolGateVerdict};

        let context_text =
            crate::tui::auto_review::build_reviewer_context(review_context, held_reason, input);
        let review = consult_reviewer(
            &self.gate_runtime.client,
            &context_text,
            &self.gate_runtime.cancel_token,
        )
        .await;
        let risk = review.outcome.audit_risk();
        let (verdict, reason) = match &review.outcome {
            ReviewerOutcome::Allow { reason, .. } => (ToolGateVerdict::Allowed, reason.clone()),
            ReviewerOutcome::Deny { reason, .. } => (ToolGateVerdict::Denied, reason.clone()),
            ReviewerOutcome::Unavailable { reason } => {
                (ToolGateVerdict::Unavailable, reason.clone())
            }
            ReviewerOutcome::Cancelled => {
                return ChildGateVerdict::Deny(
                    "Auto-Review guardian request cancelled".to_string(),
                );
            }
        };
        self.emit_child_gate_receipt(
            agent_id,
            tool_id,
            name,
            ToolGate::AutoReviewGuardian,
            verdict,
            risk,
            &reason,
        )
        .await;
        match review.outcome.into_tool_result(name) {
            Ok(_) => ChildGateVerdict::Proceed,
            Err(error) => ChildGateVerdict::Deny(error.to_string()),
        }
    }

    async fn commit_child_approval_receipt(
        &self,
        receipt: crate::approval_log::ApprovalReceipt,
    ) -> Result<(), ToolError> {
        let store = self
            .gate_runtime
            .approval_receipt_store
            .clone()
            .ok_or_else(|| {
                tracing::warn!(
                    target: "approval",
                    "child approval receipt store was not installed"
                );
                ToolError::execution_failed(
                    "Approval evidence could not be committed; tool execution was blocked."
                        .to_string(),
                )
            })?
            .map_err(|error| {
                tracing::warn!(
                    target: "approval",
                    %error,
                    "child approval receipt store is unavailable"
                );
                ToolError::execution_failed(
                    "Approval evidence could not be committed; tool execution was blocked."
                        .to_string(),
                )
            })?;
        let session_id = self.gate_runtime.context.state_namespace.clone();
        let write = tokio::task::spawn_blocking(move || store.append(&session_id, &receipt))
            .await
            .map_err(|error| {
                tracing::warn!(
                    target: "approval",
                    %error,
                    "child approval receipt writer did not complete"
                );
                ToolError::execution_failed(
                    "Approval evidence could not be committed; tool execution was blocked."
                        .to_string(),
                )
            })?;
        write.map_err(|error| {
            tracing::warn!(
                target: "approval",
                error_kind = ?error.kind(),
                "child approval receipt write failed"
            );
            ToolError::execution_failed(
                "Approval evidence could not be committed; tool execution was blocked.".to_string(),
            )
        })
    }

    /// Ask: raise the held call as an approval prompt in the parent's UI and
    /// wait for the person, visibly (`waiting for user`). Hosts that cannot
    /// prompt keep the fail-closed denial with the reason.
    async fn prompt_parent_for_child_call(
        &self,
        agent_id: &str,
        name: &str,
        input: &Value,
        reason: &str,
        force_prompt: bool,
    ) -> ChildGateVerdict {
        let Some(event_tx) = self
            .gate_runtime
            .event_tx
            .as_ref()
            .filter(|_| self.gate_runtime.parent_can_prompt)
        else {
            return ChildGateVerdict::Deny(format!(
                "{reason} (this host cannot raise a prompt for a worker; run the call in the main conversation, or switch the session to Auto-Review or Full Access)"
            ));
        };
        let (approval_id, receiver) = self
            .gate_runtime
            .manager
            .write()
            .await
            .register_child_approval(agent_id);
        if let Err(error) = self
            .commit_child_approval_receipt(crate::approval_log::ApprovalReceipt::asked(
                approval_id.clone(),
                name,
            ))
            .await
        {
            self.gate_runtime
                .manager
                .write()
                .await
                .cancel_child_approval(&approval_id);
            return ChildGateVerdict::Deny(error.to_string());
        }
        let description = format!(
            "{} (worker {}) wants to run '{name}': {reason}",
            self.owner_agent_name,
            agent_id.chars().take(12).collect::<String>()
        );
        let approval_key = format!("{approval_id}:{name}");
        let sent = event_tx
            .send(Event::ApprovalRequired {
                id: approval_id.clone(),
                tool_name: name.to_string(),
                description,
                input: input.clone(),
                approval_key: approval_key.clone(),
                approval_grouping_key: approval_key,
                intent_summary: None,
                approval_force_prompt: force_prompt,
            })
            .await
            .is_ok();
        if !sent {
            self.gate_runtime
                .manager
                .write()
                .await
                .cancel_child_approval(&approval_id);
            if let Err(error) = self
                .commit_child_approval_receipt(crate::approval_log::ApprovalReceipt::decided(
                    approval_id,
                    crate::approval_log::ApprovalOutcome::Unavailable,
                ))
                .await
            {
                return ChildGateVerdict::Deny(error.to_string());
            }
            return ChildGateVerdict::Deny(format!(
                "{reason} (the session could not be asked; the call was denied)"
            ));
        }
        record_agent_progress(
            &self.gate_runtime,
            agent_id,
            AgentProgressEventMeta::new(AgentWorkerStatus::WaitingForUser)
                .with_tool(name.to_string()),
            format!("waiting for your decision on '{name}'"),
        );
        #[derive(Clone, Copy)]
        enum WaitOutcome {
            Answer(ChildApprovalOutcome),
            Cancelled,
            Unavailable,
        }
        let outcome = tokio::select! {
            () = self.gate_runtime.cancel_token.cancelled() => WaitOutcome::Cancelled,
            answer = receiver => match answer {
                Ok(answer) => WaitOutcome::Answer(answer),
                Err(_) => WaitOutcome::Unavailable,
            },
        };
        if !matches!(outcome, WaitOutcome::Answer(_)) {
            self.gate_runtime
                .manager
                .write()
                .await
                .cancel_child_approval(&approval_id);
        }
        let receipt_outcome = match outcome {
            WaitOutcome::Answer(ChildApprovalOutcome::Approved) => {
                crate::approval_log::ApprovalOutcome::ApprovedOnce
            }
            WaitOutcome::Answer(ChildApprovalOutcome::Denied) => {
                crate::approval_log::ApprovalOutcome::Denied
            }
            WaitOutcome::Cancelled => crate::approval_log::ApprovalOutcome::Cancelled,
            WaitOutcome::Unavailable => crate::approval_log::ApprovalOutcome::Unavailable,
        };
        if let Err(error) = self
            .commit_child_approval_receipt(crate::approval_log::ApprovalReceipt::decided(
                approval_id,
                receipt_outcome,
            ))
            .await
        {
            record_agent_progress(
                &self.gate_runtime,
                agent_id,
                AgentProgressEventMeta::new(AgentWorkerStatus::RunningTool)
                    .with_tool(name.to_string()),
                format!("blocked '{name}': approval evidence could not be committed"),
            );
            return ChildGateVerdict::Deny(error.to_string());
        }
        record_agent_progress(
            &self.gate_runtime,
            agent_id,
            AgentProgressEventMeta::new(AgentWorkerStatus::RunningTool).with_tool(name.to_string()),
            match outcome {
                WaitOutcome::Answer(ChildApprovalOutcome::Approved) => {
                    format!("approved '{name}'")
                }
                WaitOutcome::Answer(ChildApprovalOutcome::Denied) => {
                    format!("denied '{name}'")
                }
                WaitOutcome::Cancelled => format!("stopped waiting on '{name}'"),
                WaitOutcome::Unavailable => format!("lost approval channel for '{name}'"),
            },
        );
        match outcome {
            WaitOutcome::Answer(ChildApprovalOutcome::Approved) => ChildGateVerdict::Proceed,
            WaitOutcome::Answer(ChildApprovalOutcome::Denied) => {
                ChildGateVerdict::Deny(format!("Tool {name} was denied by the user"))
            }
            WaitOutcome::Cancelled => ChildGateVerdict::Deny(format!(
                "Tool {name} was cancelled while awaiting the user's decision"
            )),
            WaitOutcome::Unavailable => ChildGateVerdict::Deny(format!(
                "Tool {name} approval could no longer reach the worker; tool execution was blocked"
            )),
        }
    }

    fn role_can_delegate_writes(agent_type: &FleetRole) -> bool {
        // Builder is the named implementation role. Custom may write only when
        // its profile, explicit scope, and execution envelope all agree.
        matches!(agent_type, FleetRole::Builder | FleetRole::Custom)
    }

    fn workspace_write_carve_out_permits(&self, name: &str, input: &Value) -> bool {
        // This is a bounded convenience for write-capable children, not an
        // authority escalation: every target must resolve inside the workspace
        // and still pass sensitive-path, repository-law, and claim checks.
        if !self.runtime_profile.permissions.write {
            return false;
        }
        crate::core::authority::paths_within_workspace_write_carve_out(
            &self.registry.context().workspace,
            &raw_mutation_target_paths(name, input),
        )
    }

    fn is_delegated_builtin_verification(name: &str, input: &Value) -> bool {
        use crate::tools::execution_envelope::{VerificationBound, classify_verification};

        // Reuse the same classifier as the execution envelope. This prevents a
        // second, looser notion of "test command" from growing in this module.
        matches!(
            classify_verification(canonical_action_alias(name, input), input),
            Some(VerificationBound::Default | VerificationBound::Filter)
        )
    }

    fn posture_permits_tool(&self, name: &str, input: Option<&Value>) -> bool {
        // Delegation depth governs `agent`; write posture must not accidentally
        // suppress it or turn depth into a mutation permission.
        if name == "agent" {
            return true;
        }
        match self.registry.get(name) {
            Some(spec) => match input.map_or_else(
                || spec.approval_requirement(),
                |input| spec.approval_requirement_for(input),
            ) {
                ApprovalRequirement::Auto => true,
                ApprovalRequirement::Suggest => {
                    self.runtime_profile.permissions.write
                        && role_posture_permits(&self.agent_type, ApprovalRequirement::Suggest)
                }
                ApprovalRequirement::Required => {
                    // #5426 acceptance point 1: the bounded read-only shell.
                    // `allows_bounded_readonly_bash` admits canonical `bash`
                    // to the inspection roles through the raw-shell deny
                    // list; a call the agent read-only classifier proves
                    // mutation-free is Auto-class evidence, not a held
                    // mutation, so the gate must not demand `ShellPolicy::
                    // Full` for it. Judged by the same predicate
                    // `BashTool::execute` enforces under
                    // `ShellPolicy::ReadOnly` (shell.rs), so this admission
                    // can never widen past the execute-time refusal — the
                    // first live dogfood against #5428 was denied all three
                    // canonical inspection commands here because the gate
                    // consulted only `Required` → `Full`.
                    if self.allows_bounded_readonly_bash(name)
                        && input.is_some_and(crate::tools::shell::agent_readonly_bash_input)
                    {
                        return true;
                    }
                    matches!(self.runtime_profile.shell, ShellPolicy::Full)
                        && role_posture_permits(&self.agent_type, ApprovalRequirement::Required)
                }
            },
            None => true,
        }
    }

    fn is_tool_denied(&self, name: &str) -> bool {
        // The shared matcher canonicalizes legacy/lowercase spellings before
        // applying exact or prefix rules. For example `exec_shell*` denies
        // `bash`, and `write_file*` denies `write`, in roots and children alike.
        tool_matches_any_rule(&self.disallowed_tools, name)
    }

    /// Whether this child may surface and dispatch the canonical lowercase
    /// `bash` tool under the read-only inspection posture.
    ///
    /// Scout, Reviewer, and Planner keep exactly one shell entry point —
    /// canonical `bash` — whose concrete calls the strict read-only
    /// classifier bounds.
    /// Catalog visibility and dispatch authorization consult this same
    /// predicate, so a tool that appears on the wire can always be called and
    /// one that is denied never appears.
    ///
    /// The spawn clamp keeps the raw-shell deny list (legacy `Bash` /
    /// `exec_shell` rules and the [`RAW_SHELL_SENTINEL`]) installed, because
    /// removing it would read as "this child has raw shell". Instead the
    /// catalog and dispatch admit canonical `bash` here, behind the same
    /// input-specific read-only classifier the legacy carve-out used; every
    /// other role still loses bash to the raw-shell rules.
    ///
    /// The name match is **exact**, not case-insensitive. Legacy `Bash` is a
    /// hidden execution-compatibility alias for saved transcripts, and it is
    /// exactly what the raw-shell deny list names; admitting it here through a
    /// case-insensitive compare would hand these roles back the raw shell this
    /// carve-out exists to withhold. Only the canonical lowercase tool the
    /// first-turn catalog actually offers is bounded by the classifier.
    fn allows_bounded_readonly_bash(&self, name: &str) -> bool {
        name == "bash"
            && matches!(
                &self.agent_type,
                FleetRole::Scout | FleetRole::Reviewer | FleetRole::Planner
            )
            && self.runtime_profile.shell != crate::worker_profile::ShellPolicy::None
    }

    fn legacy_action_alias(family: &str, action: &str) -> Option<&'static str> {
        Self::ACTION_ALIASES
            .iter()
            .find_map(|(candidate_family, candidate_action, alias)| {
                (*candidate_family == family && *candidate_action == action).then_some(*alias)
            })
    }

    fn is_action_allowed(&self, family: &str, action: &str) -> bool {
        let alias = Self::legacy_action_alias(family, action);
        // Read-only inspection keeps two deliberate evidence carve-outs:
        // classifier-bounded Bash reads and Web search/fetch. They bypass only
        // the coarse family sentinel; action, posture, and envelope checks still
        // reject mutation, arbitrary shell, and non-evidence Web actions.
        let bounded_readonly_bash = self.allows_bounded_readonly_bash(family) && action == "run";
        let web_readonly_action = family.eq_ignore_ascii_case("Web")
            && matches!(action, "search" | "fetch")
            && self.network_is_denied();
        if self.is_tool_denied(family)
            || !bounded_readonly_bash
                && !web_readonly_action
                && alias.is_some_and(|name| self.is_tool_denied(name))
        {
            return false;
        }
        match &self.allowed_tools {
            None => true,
            Some(list) => {
                list.iter().any(|name| name.eq_ignore_ascii_case(family))
                    || alias.is_some_and(|alias| explicit_scope_permits(list, alias))
            }
        }
    }

    fn is_tool_allowed(&self, name: &str) -> bool {
        if name == "agent" && !self.can_spawn_child {
            return false;
        }
        if self.is_tool_denied(name) && !self.allows_bounded_readonly_bash(name) {
            return false;
        }
        match &self.allowed_tools {
            None => true,
            Some(list) => {
                explicit_scope_permits(list, name)
                    || Self::ACTION_ALIASES.iter().any(|(family, _, alias)| {
                        *family == name && list.iter().any(|allowed| allowed == alias)
                    })
            }
        }
    }

    fn role_blocks_unhardened_process_tool(&self, name: &str) -> bool {
        // Catalog filtering is defense in depth. Scout/Reviewer see only the
        // hardened evidence profile; Verifier may receive bounded Run but not
        // any raw or session-oriented shell path. Dispatch repeats this check.
        let lower = name.to_ascii_lowercase();
        let evidence_tool = crate::tools::registry::readonly_evidence_tool_name(name)
            || self
                .registry
                .get(name)
                .is_some_and(|tool| crate::tools::registry::readonly_evidence_tool(tool.as_ref()));
        let bounded_inspection = matches!(&self.agent_type, FleetRole::Scout | FleetRole::Reviewer)
            && lower != "agent"
            && !evidence_tool;
        let raw_shell = lower == "bash"
            || lower.starts_with("exec_shell")
            || matches!(
                lower.as_str(),
                "exec_wait" | "exec_interact" | "task_shell_start" | "task_shell_wait"
            )
            || lower.starts_with("terminal/");
        bounded_inspection || matches!(&self.agent_type, FleetRole::Verifier) && raw_shell
    }

    fn network_is_denied(&self) -> bool {
        // Network denial has two sources: the resolved permission profile and
        // the exact-fleet sentinel. Either one is sufficient to deny a call.
        !self.runtime_profile.permissions.network || self.is_tool_denied(NETWORK_DENIAL_SENTINEL)
    }

    fn write_is_denied(&self) -> bool {
        !self.runtime_profile.permissions.write
    }

    fn shell_is_denied(&self) -> bool {
        // Likewise, shell requires both a Full profile and no explicit shell
        // sentinel. Read-only evidence commands are Auto-classified exceptions,
        // not a Full-shell grant.
        !matches!(self.runtime_profile.shell, ShellPolicy::Full)
            || self.is_tool_denied(SHELL_AUTHORITY_SENTINEL)
    }

    fn execution_envelope(&self) -> crate::tools::execution_envelope::ExecutionEnvelope {
        // Keep the network sentinel distinct from the broader permission bit:
        // read-only Web evidence remains visible when policy allows it, while
        // arbitrary network-reaching inputs are rejected separately at dispatch.
        crate::tools::execution_envelope::ExecutionEnvelope {
            write: !self.write_is_denied(),
            network: !self.is_tool_denied(NETWORK_DENIAL_SENTINEL),
            shell: !self.shell_is_denied(),
        }
    }

    #[cfg(test)]
    pub(crate) fn envelope_refusal(&self, name: &str, input: &Value) -> Option<String> {
        let spec = self.registry.get(name)?;
        crate::tools::execution_envelope::enforce_execution_envelope(
            name,
            input,
            spec.as_ref(),
            self.execution_envelope(),
            self.bounded_readonly_bash_evidence(name, input),
        )
        .err()
    }

    fn envelope_permits(&self, name: &str, input: &Value) -> bool {
        let envelope = self.execution_envelope();
        if envelope.is_unrestricted() {
            return true;
        }
        match self.registry.get(name) {
            Some(spec) => crate::tools::execution_envelope::enforce_execution_envelope(
                name,
                input,
                spec.as_ref(),
                envelope,
                self.bounded_readonly_bash_evidence(name, input),
            )
            .is_ok(),
            None => true,
        }
    }

    /// #5426/#5438: the bounded read-only shell evidence for the exact call.
    /// True only for canonical `bash` on the inspection roles whose command
    /// the agent read-only classifier proves mutation-free — the same
    /// predicate the posture gate and `BashTool::execute` (under
    /// `ShellPolicy::ReadOnly`) enforce, so gate, envelope, and execute all
    /// answer one question with one classifier. The first live dogfood was
    /// denied at the gate; after the gate fix it would STILL have been
    /// denied here — `classify_call` consults `spec.is_read_only_for`, which
    /// for bash is the deliberately tighter parallel classifier.
    fn bounded_readonly_bash_evidence(&self, name: &str, input: &Value) -> bool {
        self.allows_bounded_readonly_bash(name)
            && crate::tools::shell::agent_readonly_bash_input(input)
    }

    /// Per-role gating for the single multi-action model-facing tool.
    ///
    /// Every other capability gate in this file keys off a *tool name*: the
    /// registry allow/deny lists, `posture_permits_tool`, the execution
    /// envelope. That worked while each coordination capability had its own
    /// name, and it is exactly what breaks when six tools collapse into one —
    /// `agent` is deliberately exempt from both name-keyed gates (
    /// `posture_permits_tool` short-circuits it so delegation depth, not write
    /// posture, governs spawning; `execution_envelope::is_delegation_tool`
    /// classifies it `Bounded` so a read-only member can still fan out
    /// read-only work). A capability folded into `agent` therefore inherits
    /// *no* gate at all unless one is written for the action.
    ///
    /// So the answer is per-action, and it reproduces the gate the retired
    /// tool actually had rather than inventing a new one:
    ///
    /// - `claim` carried `agents/coordinate`'s authority, and that tool was
    ///   kept off a read-only child's catalog by declaring
    ///   `ToolCapability::WritesFiles` against `envelope.write`. The same
    ///   question is asked here directly. A read-only role has no write scope
    ///   to widen, so the action is meaningless to it, not merely refused.
    /// - Every other action keeps exactly today's visibility. `agent` already
    ///   offered message/followup/interrupt to read-only roles even while the
    ///   narrow tools were posture-gated; narrowing that here would be an
    ///   unrelated behavior change smuggled in behind a catalog cleanup.
    ///
    /// An operator deny rule naming the retired tool still removes the
    /// action, so a ceiling written against `agents/coordinate` keeps meaning
    /// what it meant.
    fn agent_action_permitted(&self, action: &str) -> bool {
        if action != "claim" {
            return true;
        }
        !self.write_is_denied() && !self.is_tool_denied("agents/coordinate")
    }

    fn visibility_representative_input(&self, name: &str) -> Option<Value> {
        // Visibility and dispatch consult the same capability guard. These
        // representative calls let a read-only bash schema survive catalog
        // shaping without treating an empty input as arbitrary shell authority.
        if !matches!(
            &self.agent_type,
            FleetRole::Scout | FleetRole::Reviewer | FleetRole::Planner
        ) {
            return None;
        }
        match name {
            "bash" => Some(json!({"command": "pwd"})),
            "Bash" => Some(json!({"action": "run", "command": "pwd"})),
            _ => None,
        }
    }

    fn tools_for_model(&self, agent_type: &FleetRole) -> Vec<Tool> {
        // Filter the full registry in deny-first order. These catalog filters
        // reduce accidental exposure, but are never the authority boundary:
        // execute() repeats role, scope, posture, envelope, and claim checks.
        let _ = agent_type;
        let api_tools = self.registry.to_api_tools();
        let filtered = match &self.allowed_tools {
            None => api_tools,
            Some(list) => api_tools
                .into_iter()
                .filter(|tool| {
                    explicit_scope_permits(list, &tool.name)
                        || is_action_family(&tool.name)
                            && tool.input_schema["properties"]["action"]["enum"]
                                .as_array()
                                .is_some_and(|actions| {
                                    actions.iter().any(|action| {
                                        action.as_str().is_some_and(|action| {
                                            Self::legacy_action_alias(&tool.name, action)
                                                .is_some_and(|alias| {
                                                    list.iter().any(|n| n == alias)
                                                })
                                        })
                                    })
                                })
                })
                .collect::<Vec<_>>(),
        };
        let mut tools = filtered
            .into_iter()
            .filter(|tool| tool.name != "agent" || self.can_spawn_child)
            .filter(|tool| {
                !self.is_tool_denied(&tool.name) || self.allows_bounded_readonly_bash(&tool.name)
            })
            .filter(|tool| !self.role_blocks_unhardened_process_tool(&tool.name))
            .filter(|tool| {
                let representative = self.visibility_representative_input(&tool.name);
                tool.name == "File"
                    || self.posture_permits_tool(&tool.name, representative.as_ref())
            })
            .filter(|tool| {
                if is_action_family(&tool.name) {
                    return true;
                }
                let representative = self
                    .visibility_representative_input(&tool.name)
                    .unwrap_or_else(|| json!({}));
                self.envelope_permits(&tool.name, &representative)
            })
            .collect::<Vec<_>>();

        for tool in &mut tools {
            if !is_action_family(&tool.name) {
                continue;
            }
            // Indexing `["properties"]["action"]["enum"]` mutably would
            // fabricate an `"action": {"enum": null}` property on schemas
            // that have no action discriminator (the lowercase `bash`
            // command/timeout shape) — a phantom node that fails Moonshot
            // MFJS validation. Only shape enums that already exist.
            let Some(actions) = tool
                .input_schema
                .pointer_mut("/properties/action/enum")
                .and_then(serde_json::Value::as_array_mut)
            else {
                continue;
            };
            actions.retain(|action| {
                let Some(action) = action.as_str() else {
                    return false;
                };
                let posture_allows = tool.name != "File"
                    || (self.runtime_profile.permissions.write
                        && role_posture_permits(&self.agent_type, ApprovalRequirement::Suggest))
                    || matches!(action, "read" | "list" | "search_name" | "search_content");
                let evidence_action =
                    !matches!(&self.agent_type, FleetRole::Scout | FleetRole::Reviewer)
                        || tool.name != "Web"
                        || matches!(action, "search" | "fetch");
                let mut representative = self
                    .visibility_representative_input(&tool.name)
                    .unwrap_or_else(|| json!({}));
                representative["action"] = json!(action);
                posture_allows
                    && evidence_action
                    && self.is_action_allowed(&tool.name, action)
                    && self.envelope_permits(&tool.name, &representative)
            });
        }
        // `agent` is not a `CANONICAL_ACTION_ALIASES` family, so the pruner
        // above never reaches it — and it must not become one, because
        // `canonical_action_alias` feeds `execution_envelope`, where `agent`'s
        // `ExecutesCode` capability is deliberately reclassified `Bounded`.
        // Shape its enum explicitly instead.
        for tool in &mut tools {
            if tool.name != "agent" {
                continue;
            }
            let Some(actions) = tool
                .input_schema
                .pointer_mut("/properties/action/enum")
                .and_then(serde_json::Value::as_array_mut)
            else {
                continue;
            };
            actions.retain(|action| {
                action
                    .as_str()
                    .is_some_and(|action| self.agent_action_permitted(action))
            });
        }
        tools.retain(|tool| {
            tool.input_schema["properties"]["action"]["enum"]
                .as_array()
                .is_none_or(|actions| !actions.is_empty())
        });
        tools
    }

    fn deferred_catalog_for_model(&self, agent_type: &FleetRole) -> Vec<Tool> {
        // Every allowed tool remains searchable. Native deferral then leaves the
        // fixed lowercase primitives plus agent/tool_search active; Web, MCP,
        // plugins, and other tools stay discoverable rather than eager.
        let mut catalog = self.tools_for_model(agent_type);
        catalog.retain(|tool| !is_tool_search_tool(&tool.name));
        ensure_advanced_tooling(
            &mut catalog,
            AppMode::Agent,
            &std::collections::HashSet::new(),
        );
        // A tool-free child (explicit empty scope) has nothing to discover:
        // drop tool_search as well so the wire request genuinely omits tools.
        let discoverable = !self.allowed_tools.as_ref().is_some_and(Vec::is_empty);
        catalog.retain(|tool| {
            (discoverable && tool.name == TOOL_SEARCH_NAME) || self.registry.contains(&tool.name)
        });
        apply_native_tool_deferral(&mut catalog, &std::collections::HashSet::new());
        catalog
    }

    fn unavailable_allowed_tools(&self) -> Vec<String> {
        match &self.allowed_tools {
            None => Vec::new(),
            Some(list) => list
                .iter()
                .filter(|name| !is_tool_search_tool(name) && !self.registry.contains(name))
                .cloned()
                .collect(),
        }
    }

    async fn execute_full(
        &self,
        agent_id: &str,
        tool_id: &str,
        name: &str,
        input: Value,
    ) -> Result<RichToolResult> {
        if self.role_blocks_unhardened_process_tool(name) {
            return Err(anyhow!(
                "Tool {name} is not available to this read-only worker because its process path does not share the hardened evidence boundary. Use read/search, classifier-bounded bash reads, or the verifier's bounded Run tool instead."
            ));
        }
        let action = input.get("action").and_then(Value::as_str);
        if matches!(&self.agent_type, FleetRole::Scout | FleetRole::Reviewer)
            && name == "Web"
            && !matches!(action, Some("search" | "fetch"))
        {
            return Err(anyhow!(
                "Tool Web is limited to search/fetch in the read-only evidence profile"
            ));
        }
        // Catalog shaping is not authority. `agent` clears both name-keyed
        // gates below by design, so the per-action gate has to be repeated
        // here or a hand-written call would reach an action the role's own
        // catalog withheld.
        if name == "agent"
            && matches!(parse_agent_tool_action(&input), Ok(AgentToolAction::Claim))
            && !self.agent_action_permitted("claim")
        {
            return Err(anyhow!(
                "agent action=claim widens an enforced write scope, and the Fleet role `{role}` has no write authority to widen. Use an `implement` or `general` role.",
                role = self.agent_type.as_str()
            ));
        }
        let family_action_allowed = if !Self::ACTION_ALIASES
            .iter()
            .any(|(family, _, _)| *family == name)
        {
            true
        } else if let Some(action) = action {
            self.is_action_allowed(name, action)
        } else {
            self.allowed_tools
                .as_ref()
                .is_none_or(|list| list.iter().any(|allowed| allowed == name))
        };
        if !self.is_tool_allowed(name) || !family_action_allowed {
            return Err(anyhow!("Tool {name} not allowed for this sub-agent"));
        }
        // #3217: authoritative per-role posture — read-only roles cannot mutate
        // and non-`Full`-shell roles cannot run shell, regardless of whether
        // the parent session is auto-approved. This closes the auto-approve
        // bypass where a read-only child could quietly write or shell out.
        if !self.posture_permits_tool(name, Some(&input)) {
            if self.allows_bounded_readonly_bash(name) {
                return Err(anyhow!(
                    "[shell.readonly.command] Tool {name} input did not match the bounded read-only shell grammar for Fleet role `{role}`. Use read-only inspection commands, or an `implement`/`general` role for mutation or arbitrary execution.",
                    role = self.agent_type.as_str()
                ));
            }
            return Err(anyhow!(
                "[role.posture.denied] Tool {name} is not permitted for the read-only Fleet role `{role}`. Use an `implement` or `general` role (or `custom` with an explicit allowed_tools list) to mutate the workspace or run shell commands.",
                role = self.agent_type.as_str()
            ));
        }
        // The session's permission posture, applied to this child exactly as
        // it is applied to the parent turn: the deterministic Auto-Review
        // floor first, then (Auto-Review) the model guardian for holds it
        // could not prove safe, or (Ask) a prompt raised in the parent's UI.
        // Full Access still fails closed on the non-bypassable safety floor.
        // Role posture and the execution envelope below stay authoritative:
        // this gate can only decide whether a call the role permits also
        // clears the session's approval boundary.
        if let ChildGateVerdict::Deny(reason) =
            self.gate_held_call(agent_id, tool_id, name, &input).await
        {
            return Err(anyhow!(reason));
        }
        reject_subagent_terminal_takeover(name, &input)?;
        if self.network_is_denied() {
            reject_network_reaching_input(name, &input)?;
        }
        if self.write_is_denied() {
            reject_unbounded_verification(name, &input, !self.shell_is_denied())?;
        }
        // The centralized envelope check. Everything above is name- or
        // shape-specific; this one is derived from the tool's real capabilities
        // and this call's canonical action, so it also covers the tools no list
        // in this file can name — repository plugins, runtime MCP server tools,
        // and anything registered later. The bounded read-only bash carve-out
        // (#5426/#5438) carries its proven-read-only evidence so the envelope
        // classifies it Bounded instead of refusing it as Executes.
        if let Some(spec) = self.registry.get(name) {
            crate::tools::execution_envelope::enforce_execution_envelope(
                name,
                &input,
                spec.as_ref(),
                self.execution_envelope(),
                self.bounded_readonly_bash_evidence(name, &input),
            )
            .map_err(|refusal| anyhow!(refusal))?;
        }
        let scope_aware_write = matches!(
            name,
            "write" | "edit" | "write_file" | "edit_file" | "apply_patch" | "fim_edit"
        ) || (name == "File"
            && input
                .get("action")
                .and_then(Value::as_str)
                .is_some_and(|action| matches!(action, "write" | "edit" | "patch")))
            || (name == "pandoc_convert" && input.get("output_path").is_some());
        if scope_aware_write && self.enforce_write_claim {
            let paths = mutation_paths(name, &input)?;
            if paths.is_empty() {
                return Err(anyhow!(
                    "Write tool {name} did not expose a bounded repo-relative target for coordination"
                ));
            }
            let manager = self.coordination_manager.read().await;
            manager
                .validate_write_scope(&self.owner_agent_id, &paths)
                .map_err(anyhow::Error::msg)?;
        } else if self.enforce_write_claim
            && !is_internal_coordination_state_tool(name)
            // A shell run the read-only classifier proves mutation-free cannot
            // collide with the peer's writes no matter how contended the
            // checkout is, so the gate below does not apply to it.
            && !proven_readonly_shell_run(name, &input)
            && (is_unbounded_shell_run(name, &input)
                || self.registry.get(name).is_some_and(|spec| {
                    let canonical = canonical_action_alias(name, &input);
                    let is_shell_control = matches!(
                        canonical,
                        "exec_shell_wait" | "exec_shell_interact" | "exec_shell_cancel"
                    );
                    let capabilities = spec.capabilities();
                    !is_shell_control
                        && (spec.approval_requirement_for(&input) == ApprovalRequirement::Suggest
                            || (!spec.is_read_only_for(&input)
                                && capabilities.iter().any(|capability| {
                                    matches!(
                                        capability,
                                        ToolCapability::WritesFiles
                                            | ToolCapability::ExecutesCode
                                            | ToolCapability::Network
                                    )
                                })))
                }))
        {
            let manager = self.coordination_manager.read().await;
            // Only a *contended* shared checkout needs this gate. The claim
            // exists so concurrent children cannot overwrite each other; a lone
            // writer has no peer to collide with, and blocking it there bought
            // no safety while making a builder unable to run ordinary shell
            // work in the workspace the operator actually watches — worktree
            // isolation "fixes" that by writing somewhere they never see.
            if manager.shared_write_claim(&self.owner_agent_id).is_some() {
                let blocking_peers =
                    manager.live_peer_shared_write_claim_owners(&self.owner_agent_id);
                if !blocking_peers.is_empty() {
                    return Err(anyhow!(
                        "Tool {name} cannot prove a bounded file target, and another child is writing in this shared checkout (blocking peers: {}). Wait for the peer to finish, ask the parent to cancel it, run agents/coordinate action=release to clear a stale claim, or relaunch the children with worktree isolation.",
                        blocking_peers.join(", ")
                    ));
                }
            }
        }
        let context = self
            .registry
            .context()
            .clone()
            .with_owner_agent(self.owner_agent_id.clone(), self.owner_agent_name.clone());
        self.registry
            .execute_rich_full_with_context(name, input, Some(&context))
            .await
            .map_err(|e| anyhow!(e))
    }

    #[cfg(test)]
    async fn execute(&self, agent_id: &str, name: &str, input: Value) -> Result<String> {
        self.execute_full(agent_id, "", name, input)
            .await
            .map(|result| result.result.content)
    }

    async fn execute_from_surface(
        &self,
        agent_id: &str,
        tool_id: &str,
        surface: &mut SubAgentToolSurface,
        request_active_names: &std::collections::HashSet<String>,
        name: &str,
        input: Value,
    ) -> Result<RichToolResult> {
        if is_tool_search_tool(name) {
            if !request_active_names.contains(name) {
                return Err(anyhow!("Tool {name} is not in this child's catalog"));
            }
            return surface
                .search(name, &input)
                .map(|content| RichToolResult::plain(ToolResult::success(content)));
        }

        let Some(deferred) = surface
            .catalog
            .iter()
            .find(|tool| tool.name == name)
            .map(|tool| tool.defer_loading.unwrap_or(false))
        else {
            return Err(anyhow!(
                "Tool {name} is not in this child's policy-filtered catalog"
            ));
        };
        if deferred && !request_active_names.contains(name) {
            return surface
                .hydrate(name)
                .map(|content| RichToolResult::plain(ToolResult::success(content)));
        }
        if !request_active_names.contains(name) {
            return Err(anyhow!("Tool {name} is not active for this sub-agent"));
        }
        let result = self.execute_full(agent_id, tool_id, name, input).await;
        if deferred && result.is_ok() {
            touch_cached_tool_after_execution(
                &surface.catalog,
                &mut surface.active_names,
                &mut surface.cache,
                name,
            );
        }
        result
    }
}

fn is_unbounded_shell_run(name: &str, input: &Value) -> bool {
    canonical_action_alias(name, input) == "exec_shell"
}

/// Whether this exact call is a shell run the agent read-only classifier
/// proves mutation-free — the contention gate's carve-out.
///
/// This shares its classifier with `bounded_readonly_bash_evidence` but not
/// its role restriction, deliberately: the envelope helper's carve-out
/// *grants* shell to inspection roles that otherwise lack it, so it is scoped
/// to them; the contention gate grants nothing — shell authority, posture, and
/// the envelope have already settled by the time it runs — and asks only
/// whether this call can collide with a live peer's writes. A proven
/// read-only `ls` cannot, whichever role runs it.
fn proven_readonly_shell_run(name: &str, input: &Value) -> bool {
    is_unbounded_shell_run(name, input) && crate::tools::shell::agent_readonly_bash_input(input)
}

/// Parameter names whose *value* is a location to reach, rather than content.
///
/// Deliberately a name list and not a scan of every string: a network-denied
/// child may perfectly well write a file whose body mentions `https://`, or grep
/// for one. Refusing that would be a false positive with no security value. The
/// question this guard asks is "is this call *addressed* somewhere remote", and
/// only a field that names a destination can answer it.
const URL_BEARING_FIELDS: &[&str] = &[
    "url",
    "urls",
    "uri",
    "href",
    "link",
    "endpoint",
    "base_url",
    "target",
    "pr",
    "pull_request",
];

/// Whether a string addresses a remote location over a network scheme.
///
/// Scheme-anchored, so a workspace path, a bare filename, or a `git@` remote is
/// not mistaken for one. `file:` is deliberately absent: it names no host.
fn is_network_url(value: &str) -> bool {
    let value = value.trim();
    let lowered = value.to_ascii_lowercase();
    ["http://", "https://", "ws://", "wss://", "ftp://"]
        .iter()
        .any(|scheme| lowered.starts_with(scheme))
}

/// Whether any URL-bearing field in `input` addresses the network.
///
/// Scans the top level and one level of nesting (objects and arrays), which
/// covers every shape the tool schemas here actually use — `{"url": ...}`,
/// `{"urls": [...]}`, `{"source": {"url": ...}}` — without recursing into
/// arbitrary model-supplied content.
fn carries_network_url(input: &Value) -> bool {
    fn field_reaches(key: &str, value: &Value) -> bool {
        if !URL_BEARING_FIELDS.contains(&key) {
            return false;
        }
        match value {
            Value::String(text) => is_network_url(text),
            Value::Array(items) => items
                .iter()
                .any(|item| item.as_str().is_some_and(is_network_url)),
            _ => false,
        }
    }

    let Some(object) = input.as_object() else {
        return false;
    };
    object.iter().any(|(key, value)| {
        field_reaches(key, value)
            || match value {
                Value::Object(nested) => nested
                    .iter()
                    .any(|(nested_key, nested_value)| field_reaches(nested_key, nested_value)),
                Value::Array(items) => items.iter().any(|item| {
                    item.as_object().is_some_and(|nested| {
                        nested.iter().any(|(nested_key, nested_value)| {
                            field_reaches(nested_key, nested_value)
                        })
                    })
                }),
                _ => false,
            }
    })
}

/// Refuse a call that reaches the network through a tool the name deny list
/// does not consider a network tool.
///
/// The deny list is keyed on names, and that is sufficient for any tool whose
/// whole purpose is the network — those are simply absent. It is not sufficient
/// for a *mostly local* tool that reaches out only for certain inputs, because
/// such a tool calls the fetch path itself, inside the process, under its own
/// name. Two live examples, both real escapes before this guard existed:
///
/// - `rlm{action:"open", url:...}` calls `FetchUrlTool::execute` directly.
/// - `review{target:"https://github.com/o/r/pull/1"}` shells out to `gh pr
///   diff`, which is the network by way of a subprocess.
///
/// Both are now unreachable by name as well (`rlm_open` is denied outright;
/// `review`'s local forms are the ones worth keeping), so this is the layer that
/// catches the *next* one — a tool that grows a `url` field after this list was
/// written. It fails closed and names the posture, so the refusal reads as a
/// contract rather than a malfunction.
fn reject_network_reaching_input(name: &str, input: &Value) -> Result<()> {
    let github_shell_read = matches!(name, "bash" | "Bash" | "exec_shell")
        && input
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(crate::command_safety::is_github_readonly_command);
    if !github_shell_read && !carries_network_url(input) {
        return Ok(());
    }
    Err(anyhow!(
        "Tool {name} was called with a network address, but this agent runs with no network \
         capability (`network_tool = false`). Local sources are still available; a remote one \
         needs a member whose saved ceiling grants network tools."
    ))
}

/// Refuse the unbounded forms of the verification surface for a write-denied
/// child.
///
/// A read-only member keeps `Run` / `run_tests` / `run_verifiers` on purpose:
/// running the checks is what a verifier is *for*, and removing them would make
/// the role useless. But both tools accept an escape hatch that is not
/// verification at all — `run_verifiers` takes `commands`, an array of arbitrary
/// `program` + `args` pairs, and `run_tests` takes `args`, a raw cargo argv.
/// `{"program": "bash", "args": ["-lc", "rm -rf src"]}` is exactly the raw shell
/// that [`crate::fleet::role::RAW_SHELL_DENYLIST`] just removed, re-entered
/// through the one door that was left open for honest reasons.
///
/// So the tools stay and the arbitrary arguments go. The default form — the one
/// the deny list's comment actually promises is bounded — keeps working.
fn reject_unbounded_verification(name: &str, input: &Value, shell: bool) -> Result<()> {
    use crate::tools::execution_envelope::{VerificationBound, classify_verification};

    match classify_verification(canonical_action_alias(name, input), input) {
        None | Some(VerificationBound::Default) => Ok(()),
        // A pure test selection is what the shipped `verifier` role exists to
        // run. It starts a process, so it costs shell authority — and nothing
        // else, because `write` is not what a test filter needs.
        Some(VerificationBound::Filter) if shell => Ok(()),
        Some(VerificationBound::Filter) => Err(anyhow!(
            "Tool {name} was called with test-selection arguments, which start a test process, \
             and this agent has no shell authority. Drop `args` to run the default verification \
             gate."
        )),
        Some(VerificationBound::Unbounded) => Err(anyhow!(
            "Tool {name} was called with operator-supplied commands or arguments that can name a \
             program or redirect what runs, which spawns arbitrary programs and can mutate the \
             workspace. This agent runs read-only, so only the built-in verification gates and \
             test-selection arguments are available. Drop `commands`, drop the redirecting flag, \
             or use a write-capable role."
        )),
    }
}

fn is_internal_coordination_state_tool(name: &str) -> bool {
    matches!(
        name,
        "agent"
            | "agents/list"
            | "agents/message"
            | "agents/followup"
            | "agents/interrupt"
            | "agents/coordinate"
            | "agents/wait"
            | "work_update"
            | "checklist_add"
            | "checklist_update"
            | "checklist_write"
            | "todo_add"
            | "todo_update"
            | "todo_write"
    )
}

/// Raw target paths of a write-capable call, before claim normalization:
/// the patch preflight's touched files for `apply_patch`/`File.patch`, else
/// the `path`/`output_path` field. Feeds the child write carve-out (#5186);
/// claim enforcement keeps using the normalized [`mutation_paths`].
fn raw_mutation_target_paths(name: &str, input: &Value) -> Vec<String> {
    if name == "apply_patch"
        || (name == "File" && input.get("action").and_then(Value::as_str) == Some("patch"))
    {
        let mut patch_input = input.clone();
        if let Some(object) = patch_input.as_object_mut() {
            object.remove("action");
        }
        return crate::tools::apply_patch::preflight_apply_patch(&patch_input)
            .map(|preflight| preflight.touched_files)
            .unwrap_or_default();
    }
    input
        .get("path")
        .or_else(|| input.get("output_path"))
        .and_then(Value::as_str)
        .map(|path| vec![path.to_string()])
        .unwrap_or_default()
}

fn mutation_paths(name: &str, input: &Value) -> Result<Vec<String>> {
    let raw_paths = if name == "apply_patch"
        || (name == "File" && input.get("action").and_then(Value::as_str) == Some("patch"))
    {
        let mut patch_input = input.clone();
        if let Some(object) = patch_input.as_object_mut() {
            object.remove("action");
        }
        crate::tools::apply_patch::preflight_apply_patch(&patch_input)
            .map_err(|err| anyhow!(err.to_string()))?
            .touched_files
    } else if let Some(path) = input
        .get("path")
        .or_else(|| input.get("output_path"))
        .and_then(Value::as_str)
    {
        vec![path.to_string()]
    } else {
        Vec::new()
    };
    raw_paths
        .into_iter()
        .map(|path| normalize_claim_path(&path).map_err(anyhow::Error::msg))
        .collect()
}

fn reject_subagent_terminal_takeover(name: &str, input: &Value) -> Result<()> {
    let wants_interactive_shell = matches!(name, "bash" | "Bash" | "exec_shell")
        && input
            .get("action")
            .and_then(Value::as_str)
            .is_none_or(|action| action == "run")
        && input
            .get("interactive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    if wants_interactive_shell {
        return Err(anyhow!(
            "Sub-agents run in the background and cannot use Bash with interactive=true \
             because that would take over the parent TUI terminal. Use Bash without \
             interactive, or with background=true / tty=true, or task_shell_start \
             instead."
        ));
    }
    Ok(())
}

/// Resolve the effective allowed-tools list for a child.
///
/// **v0.6.6 default: full inheritance.** Returning `Ok(None)` means the
/// child sees the same tool surface as the parent's Agent mode — every
/// family including `with_subagent_tools` so it can recurse. The narrowing
/// path (`Ok(Some(list))`) is only used by:
/// - `Custom` agent types (which require an explicit list).
/// - Callers that pass `explicit_tools` (advanced / legacy use).
///
/// `allow_shell = false` no longer narrows the tool LIST — the child's
/// registry simply doesn't register shell tools, which has the same
/// effect without papering over the parent's choice with a deny-list.
fn build_allowed_tools(
    agent_type: &FleetRole,
    explicit_tools: Option<Vec<String>>,
    _allow_shell: bool,
) -> Result<Option<Vec<String>>> {
    if let Some(tools) = explicit_tools {
        let mut deduped = Vec::new();
        for tool in tools {
            let name = tool.trim();
            if !name.is_empty() && !deduped.iter().any(|existing: &String| existing == name) {
                deduped.push(name.to_string());
            }
        }
        if matches!(agent_type, FleetRole::Custom) && deduped.is_empty() {
            return Err(anyhow!(
                "Custom sub-agent requires a non-empty allowed_tools list"
            ));
        }
        return Ok(Some(deduped));
    }

    if matches!(agent_type, FleetRole::Custom) {
        return Err(anyhow!(
            "Custom sub-agent requires a non-empty allowed_tools list"
        ));
    }

    // Default: full registry inheritance from the parent. The child sees every
    // tool the parent has, including the sub-agent management family. The
    // registry execution guard still blocks approval-gated tools unless the
    // parent runtime is auto-approved.
    Ok(None)
}

/// Render a sub-agent model failure with its full error chain. `to_string()`
/// on an anyhow error prints only the outermost context (for Codex children
/// that is the bare "Responses API request failed"), discarding the HTTP
/// status, sanitized body snippet, and error class carried by the source
/// `LlmError` — the exact masking reported in #3884. The alternate format
/// walks the chain, and the downcast prefixes a stable class tag so failure
/// records distinguish auth/rate-limit/invalid-request/model/server/network
/// failures at a glance.
fn subagent_failure_message(err: &anyhow::Error) -> String {
    let class = match err.downcast_ref::<LlmError>() {
        Some(LlmError::RateLimited { .. }) => Some("rate_limited"),
        Some(LlmError::QuotaExhausted(_)) => Some("quota_exhausted"),
        Some(LlmError::ServerError { .. }) => Some("server"),
        Some(LlmError::NetworkError(_)) | Some(LlmError::Timeout(_)) => Some("network"),
        Some(LlmError::AuthenticationError(_)) | Some(LlmError::AuthorizationError(_)) => {
            Some("auth")
        }
        Some(LlmError::InvalidRequest { .. }) => Some("invalid_request"),
        Some(LlmError::ModelError(_)) => Some("model"),
        Some(LlmError::ContentPolicyError(_)) => Some("content_policy"),
        Some(LlmError::ContextLengthError(_)) => Some("context_length"),
        Some(LlmError::ParseError(_)) | Some(LlmError::Other(_)) | None => None,
    };
    match class {
        Some(class) => format!("[{class}] {err:#}"),
        None => format!("{err:#}"),
    }
}

/// Human label for how a child's model was selected, so a launch failure can
/// name the route that produced the failing model — inherited from the parent,
/// a faster same-family sibling, or an explicit id (#4049).
fn route_source_label(route: &ModelRoute) -> String {
    match route {
        ModelRoute::Inherit => "inherited from the parent/session model".to_string(),
        ModelRoute::Faster => "faster same-family sibling of the parent model".to_string(),
        ModelRoute::Auto => "auto (legacy route, treated as a faster sibling)".to_string(),
        ModelRoute::Fixed(id) => format!("explicit model id `{id}`"),
    }
}

/// When a child agent fails because its model is unavailable under the current
/// access profile, a bare provider 403/404 (classified `Authorization` or
/// `State`) is unactionable. Annotate it so the parent knows which provider and
/// route produced the failing model and how to recover (#2653, #4049) without
/// re-classifying the underlying error. Errors unrelated to model availability
/// pass through unchanged.
fn annotate_child_model_error(
    err: &str,
    model: &str,
    provider: crate::config::ApiProvider,
    route: &ModelRoute,
) -> String {
    let hint = || {
        format!(
            "{err}\n(provider `{}` · requested model `{model}` · route: {} — \
             the model may be unavailable under the current access profile; remove the explicit \
             child model override or adjust child-agent model config before retrying)",
            provider_name_for_error(provider),
            route_source_label(route),
        )
    };
    match crate::error_taxonomy::classify_error_message(err) {
        crate::error_taxonomy::ErrorCategory::Authorization
        | crate::error_taxonomy::ErrorCategory::State => hint(),
        _ => {
            // #3020 (#2653): Provider rejections like "Model Not Exist" or
            // "does not exist or you do not have access" often classify as
            // `Internal` rather than `Authorization`/`State`.  Catch these
            // patterns in the raw error text and annotate anyway.
            let lower = err.to_ascii_lowercase();
            if lower.contains("model not exist")
                || lower.contains("model_not_found")
                || lower.contains("does not exist")
                || lower.contains("no such model")
                || lower.contains("invalid model")
            {
                hint()
            } else {
                err.to_string()
            }
        }
    }
}

/// Char budget above which a sub-agent summary is treated as a large dump and
/// head+tail truncated. Mirrors `TOOL_RESULT_SENT_CHAR_BUDGET` in
/// `crates/tui/src/client/chat.rs:1377` so sub-agent summaries use the same
/// threshold as regular tool outputs. Duplicated locally to avoid coupling the
/// sub-agent module to the wire-compaction internals.
const SUBAGENT_SUMMARY_CHAR_BUDGET: usize = 12_000;
/// Head/tail slice sizes when truncating; mirror the wire constants
/// (`TOOL_RESULT_HEAD_CHARS`/`TOOL_RESULT_TAIL_CHARS`, chat.rs:1378-1379).
const SUBAGENT_SUMMARY_HEAD_CHARS: usize = 4_000;
const SUBAGENT_SUMMARY_TAIL_CHARS: usize = 4_000;

/// One-line provenance suffix reinforcing that a sub-agent summary is a
/// self-report (issue #2652). Appended only when the summary was NOT
/// length-truncated, so every summary carries exactly one boundary marker.
const SUBAGENT_SELF_REPORT_NOTE: &str = "\n[Sub-agent self-report — re-verify material claims (read changed files, \
run the relevant tests) before relying on it.]";

/// Stamp a sub-agent summary with a provenance/clip marker (issue #2652).
///
/// Returns `(stamped_summary, truncated)`:
/// - When the raw summary is within the budget, append the soft self-report
///   note and report `truncated: false`.
/// - When it exceeds the budget, keep a head+tail slice and stamp it with the
///   existing `[Output truncated ...]` vocabulary (reused from tool-output
///   truncation). When `report_ref` names the persisted full report (see
///   `spill_subagent_final_report`), the footer points at it so the elided
///   middle stays retrievable via `retrieve_tool_result`; with no ref the
///   footer stays honest that the middle cannot be retrieved. Report
///   `truncated: true` either way.
///
/// Every summary therefore gets exactly one boundary marker, never both.
#[cfg(test)]
fn stamp_subagent_summary(raw: &str) -> (String, bool) {
    stamp_subagent_summary_with_ref(raw, None)
}

/// The ref-aware stamper; see `stamp_subagent_summary`.
fn stamp_subagent_summary_with_ref(raw: &str, report_ref: Option<&str>) -> (String, bool) {
    let total = raw.chars().count();
    if total <= SUBAGENT_SUMMARY_CHAR_BUDGET {
        return (format!("{raw}{SUBAGENT_SELF_REPORT_NOTE}"), false);
    }
    let chars: Vec<char> = raw.chars().collect();
    let head: String = chars.iter().take(SUBAGENT_SUMMARY_HEAD_CHARS).collect();
    let tail: String = chars
        .iter()
        .skip(total.saturating_sub(SUBAGENT_SUMMARY_TAIL_CHARS))
        .collect();
    let omitted = total
        .saturating_sub(SUBAGENT_SUMMARY_HEAD_CHARS)
        .saturating_sub(SUBAGENT_SUMMARY_TAIL_CHARS);
    let retrieval = match report_ref {
        Some(reference) => format!(
            "the full report is retained as artifact {reference} — read the elided middle ({omitted} \
chars) with retrieve_tool_result using that ref (mode=lines/query/bytes). Re-verify material claims \
before relying on them."
        ),
        None => format!(
            "the elided middle ({omitted} chars) is not in the spillover store and cannot be \
retrieved via retrieve_tool_result. Re-open the child or read changed files directly to verify \
material claims."
        ),
    };
    let stamped = format!(
        "{head}\n\n[Sub-agent summary truncated: {SUBAGENT_SUMMARY_HEAD_CHARS} + {SUBAGENT_SUMMARY_TAIL_CHARS} of {total} \
chars shown. This is the child's self-report; {retrieval}]\n\n{tail}",
    );
    (stamped, true)
}

/// Persist a final report that exceeds the summary budget so the truncated
/// summary can name a retrievable artifact instead of dropping the elided
/// middle. Returns the `retrieve_tool_result` ref on success; write failures
/// degrade to no ref, mirroring `apply_spillover`'s passthrough posture. The
/// artifact lands under the shared session root (`state_namespace`), which
/// every agent in the spawn tree clones, so the parent and any sibling can
/// resolve it. Same-id writes carry identical bytes, so a synthesized
/// re-delivery of the same terminal result is idempotent.
pub(crate) fn spill_subagent_final_report(
    session_id: &str,
    result: &SubAgentResult,
) -> Option<String> {
    let raw = summarize_subagent_result(result);
    if raw.chars().count() <= SUBAGENT_SUMMARY_CHAR_BUDGET {
        return None;
    }
    let artifact_id = format!("art_sa_{}_report", result.agent_id);
    crate::artifacts::write_session_artifact(session_id, &artifact_id, &raw)
        .ok()
        .map(|_| artifact_id)
}

fn summarize_subagent_result(result: &SubAgentResult) -> String {
    if let Some(needs_input) = result.needs_input.as_ref() {
        return format!("Needs input: {}", needs_input.question);
    }
    match (&result.status, result.result.as_ref()) {
        (SubAgentStatus::Completed, Some(text)) => text.clone(),
        (SubAgentStatus::Completed, None) => "Completed (no final summary returned)".to_string(),
        (SubAgentStatus::Interrupted(error), _) => format!("Interrupted: {error}"),
        (SubAgentStatus::Cancelled, _) => "Cancelled".to_string(),
        (SubAgentStatus::BudgetExhausted, Some(text)) => format!(
            "Child token budget exhausted before finishing; partial output preserved below.\n{text}"
        ),
        (SubAgentStatus::BudgetExhausted, None) => {
            "Child token budget exhausted before returning a final summary; retry with a smaller scoped task or split the work.".to_string()
        }
        (SubAgentStatus::Failed(error), _) => format!("Failed: {error}"),
        (SubAgentStatus::Running, _) => "Running".to_string(),
    }
}

fn subagent_status_name(status: &SubAgentStatus) -> &'static str {
    match status {
        SubAgentStatus::Running => "running",
        SubAgentStatus::Completed => "completed",
        SubAgentStatus::Interrupted(_) => "interrupted",
        SubAgentStatus::Failed(_) => "failed",
        SubAgentStatus::Cancelled => "cancelled",
        SubAgentStatus::BudgetExhausted => "budget_exhausted",
    }
}

use crate::prompts::text::SUBAGENT_OUTPUT_FORMAT;

const GENERAL_AGENT_INTRO: &str = concat!(
    "You are a trusted Fleet worker. Your job is to complete the one task you were given, end-to-end, and report back concisely.\n",
    "Stay inside the assigned scope; put adjacent work under RISKS/BLOCKERS.\n",
    "For genuinely multi-step work, track progress with `todo_write`; skip it for short, focused tasks.\n",
    "**Stop quickly on failure**: if the same tool call fails 2 times in a row, stop retrying and return what you have so far with a one-line note explaining what's missing. Do not loop on impossible queries (e.g. external API unreachable, rate-limited, or returning empty).\n",
    "For builder or repair-style work, keep going within the assigned scope; checkpoint before broadening the task or after repeated failures instead of forcing a tiny tool-call cap.\n\n"
);

const EXPLORE_AGENT_INTRO: &str = concat!(
    "You are a trusted Fleet scout (role: `scout`). Your job is to map the relevant code quickly and stay strictly read-only.\n",
    "Default to `EFFORT: quick`: aim for about 3-5 tool calls unless the brief explicitly asks for more.\n",
    "Orient first: confirm the workspace/project root, read relevant AGENTS.md/README guidance when the tree is unfamiliar, then search only the likely scope.\n",
    "Use `read` for bounded file reads and `bash` only for the allowed read-only inspection subset: navigation/rg, safe Git reads (for example `git log -n 5`), and read-only GitHub views such as `gh issue view`. Builds, tests, writes, and shell control actions are unavailable.\n",
    "Use your private `todo_write` list as editable working notes when useful; it is agent-owned state, not permission to write project files. Those tool calls remain in the complete transcript artifact returned to the parent.\n",
    "Honor QUESTION, SCOPE, ALREADY_KNOWN, and STOP_CONDITION. Do not repeat ALREADY_KNOWN work unless evidence contradicts it; do not broaden once QUESTION is answered.\n",
    "Your value is compressed evidence: cite `path:line-range` for each finding and stop once evidence is sufficient. Return partial findings if the next step would be speculative or duplicative.\n",
    "CHANGES will almost always be \"None.\" for a scout.\n\n"
);

const PLAN_AGENT_INTRO: &str = concat!(
    "You are a trusted Fleet planner (role: `planner`). Your job is to produce a grounded, prioritized plan, not patches.\n",
    "Read enough code to avoid guessing; each step names its artifact and verification.\n",
    "Use `read` for bounded file reads and `bash` only for the allowed read-only inspection subset: navigation/rg, safe Git reads (for example `git log -n 5`), and read-only GitHub views such as `gh issue view`. Builds, tests, writes, and shell control actions are unavailable.\n",
    "Use todo_write for concrete To-do progress; explain key trade-offs in the plan you return.\n",
    "CHANGES should list plan artifacts only, not future speculative edits.\n\n"
);

const REVIEW_AGENT_INTRO: &str = concat!(
    "You are an adversarial Fleet reviewer (role: `reviewer`). Assume the change is broken until the evidence proves otherwise: actively try to refute the claims made about it, and stay strictly read-only.\n",
    "Read the diff/files, grep sibling patterns/tests, hunt regressions, missing tests, unhandled edge cases, and quiet behavior changes, then order EVIDENCE by severity.\n",
    "Use `read` for bounded file reads and `bash` only for the allowed read-only navigation/rg, safe Git, and read-only GitHub evidence subset; builds, tests, writes, and shell control actions are unavailable.\n",
    "Use your private `todo_write` list as editable working notes when useful; it is agent-owned state, not permission to write project files. Those tool calls remain in the complete transcript artifact returned to the parent.\n",
    "Use BLOCKER/MAJOR/MINOR/NIT and include path:line-range plus suggested fix.\n",
    "You may use more tool calls than quick exploration, but stop after decisive evidence instead of widening the review forever.\n",
    "If nothing survives your attack, say plainly in SUMMARY that no MAJOR+ issues exist — a clean verdict earned adversarially is a real result, not a failure.\n",
    "CHANGES will almost always be \"None.\" for a reviewer.\n\n"
);

const CUSTOM_AGENT_INTRO: &str = concat!(
    "You are a trusted custom Fleet worker (role: `custom`) with a narrowed tool registry. Your job is to stay tightly scoped to the assigned objective.\n",
    "Use only tools available at runtime; put missing capabilities under BLOCKERS and stop.\n\n"
);

const IMPLEMENTER_AGENT_INTRO: &str = concat!(
    "You are a trusted Fleet builder (role: `builder`). Your job is to land the assigned change with minimal surrounding edits.\n",
    "Use `edit` for precise unique replacements, `write` for whole-file changes, and discover `apply_patch` for unified multi-file patches when needed.\n",
    "Run relevant verification after edit batches; write needed tests with the implementation.\n",
    "You are not limited to a scout-style 3-5 tool-call cap. Checkpoint before expanding scope or after repeated failures, then continue only inside the assigned brief.\n",
    "CHANGES is load-bearing: list every modified file with a one-line why.\n",
    "Before finishing, end with a VERDICT block: PASS or FAIL, the exact commands you ran (or why verification was impossible), and brief evidence. A diff alone is not completion.\n\n"
);

/// Spawn-time contract for write-capable children: they must return PASS/FAIL
/// evidence, not just a patch. Injected into the child system prompt for
/// implementer/builder and other write-authority roles (Operate C1).
const WRITE_CHILD_VERIFY_CONTRACT: &str = concat!(
    "\n\n## Verify-before-return (write child)\n",
    "You are a write-capable worker. Completing file edits is not enough.\n",
    "1. Run the repository's relevant checks for the change you made (tests, lint, typecheck, or the acceptance criteria in your brief).\n",
    "2. End your final message with a structured evidence block:\n",
    "   VERDICT: PASS | FAIL\n",
    "   COMMANDS: <exact commands run, one per line, or NONE with reason>\n",
    "   EVIDENCE: <exit codes, failing assertion, or concise proof of PASS>\n",
    "3. Do not claim PASS without command or inspection evidence. If you cannot run checks, report FAIL or BLOCKED with the blocker — never invent success.\n",
);

const CONSULTANT_AGENT_INTRO: &str = concat!(
    "You are a trusted Fleet consultant (role: `consultant`). You are asked for judgement, not for labour.\n",
    "You are read-only and have no shell. Read the workspace and the public web to ground your advice, then give counsel.\n",
    "Lead with your actual recommendation, not a survey of options. If you would do something different from what was proposed, say so first and say why.\n",
    "Name what the asker appears not to have considered: the failure mode, the constraint, the cheaper alternative, the reason this is harder than it looks.\n",
    "Distinguish what you verified by reading from what you are inferring. An unverified hunch is still useful — labelled as one.\n",
    "If the question is underspecified in a way that changes the answer, say which detail decides it rather than answering both ways at length.\n",
    "CHANGES will always be \"None.\" for a consultant.\n\n"
);

const VERIFIER_AGENT_INTRO: &str = concat!(
    "You are a trusted Fleet verifier (role: `verifier`). Your job is to run the requested gates with your bounded validation tools — the allowed test/check selections — and report results. You never write: patching the workspace is denied. Unbounded shell forms are refused; use the verification surface.\n",
    "Report PASS/FAIL/FLAKY at the top of SUMMARY with exact command evidence.\n",
    "Capture failing assertion and file:line; put obvious fixes under RISKS.\n",
    "You may use more tool calls than quick exploration, but stop after decisive pass/fail evidence.\n",
    "CHANGES will almost always be \"None.\" for a verifier.\n\n"
);

// === Tests ===

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use tests::kimi_general_child_request_tools_fixture;
