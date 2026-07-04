//! Sub-agent spawning system.
//!
//! Provides tools to spawn background sub-agents, query their status,
//! and retrieve results. Sub-agents run with a filtered toolset and
//! inherit the workspace configuration from the main session.
//!
//! The model-facing surface is the single `agent` tool. Older lifecycle
//! structs and manager helpers remain executable for persisted records and
//! internal recovery while the durable runtime is reused by the new surface.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Read;
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
use uuid::Uuid;

use crate::client::DeepSeekClient;
use crate::config::MAX_SUBAGENTS;
use crate::core::events::Event;
use crate::dependencies::{ExternalTool, Git};
use crate::llm_client::{LlmClient, LlmError};
use crate::models::{
    ContentBlock, Message, MessageRequest, MessageResponse, SystemPrompt, Tool, Usage,
};
use crate::request_tuning::RequestTuning;
use crate::tools::handle::VarHandle;
use crate::tools::plan::{PlanState, SharedPlanState};
use crate::tools::registry::{AgentToolSurfaceOptions, ToolRegistry, ToolRegistryBuilder};
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};
use crate::tools::todo::SharedTodoList;
#[cfg(test)]
use crate::tools::todo::TodoList;
use crate::tools::truncate::{SPILLOVER_HEAD_BYTES, SPILLOVER_THRESHOLD_BYTES, maybe_spillover};
use crate::tui::app::ReasoningEffort;
use crate::utils::spawn_supervised;
use crate::worker_profile::{ModelRoute, ShellPolicy, ToolScope, WorkerRuntimeProfile};

pub mod mailbox;
#[allow(unused_imports)]
pub use mailbox::{Mailbox, MailboxEnvelope, MailboxMessage, MailboxReceiver};

// === Constants ===

/// Global ownership table for cache-aware resident file sub-agents (#529).
/// Maps file path → agent id. Agents hold a lease on a file while running;
/// the lease is released when the agent reaches a terminal state.
static RESIDENT_LEASES: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, String>>,
> = std::sync::OnceLock::new();

/// Release all resident file leases held by `agent_id`. Called when an
/// agent transitions to a terminal state (completed, failed, cancelled).
fn release_resident_leases_for(agent_id: &str) {
    if let Some(lock) = RESIDENT_LEASES.get()
        && let Ok(mut guard) = lock.lock()
    {
        guard.retain(|_, owner| owner != agent_id);
    }
}

/// Default maximum steps for sub-agent loops. Set to `u32::MAX` to remove the
/// arbitrary fixed cap (#2034). Sub-agents run until they produce a final text
/// response (no tool calls), are cancelled by the parent, or hit a configured
/// explicit budget. Callers that want a hard bound can override `max_steps` on
/// the `SubAgentManager`.
const DEFAULT_MAX_STEPS: u32 = u32::MAX;
/// Default wall-clock budget for a single sub-agent tool execution. The active
/// value travels on `SubAgentRuntime::tool_timeout` so a long-but-legitimate
/// tool (a large build, a slow shell command, a deep search) is not killed
/// mid-flight. Kept non-zero so `timeout(Duration::ZERO, ...)` can never fire
/// immediately. The per-step API timeout, streaming watchdogs, and heartbeat
/// floors remain the independent stall detectors.
const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(300);
const MIN_SUBAGENT_SPAWN_TOKEN_RESERVE: u64 = 1;
const MIN_EVENT_CHANNEL_HEADROOM_FOR_ROUTINE_PROGRESS: usize = 32;

/// Format a step counter for sub-agent progress messages.
///
/// When `max_steps == u32::MAX` (the default), the denominator is a sentinel
/// meaning "unbounded" — render just `step N` instead of `step N/4294967295`.
fn format_step_counter(steps: u32, max_steps: u32) -> String {
    if max_steps == u32::MAX {
        format!("step {steps}")
    } else {
        format!("step {steps}/{max_steps}")
    }
}
// Non-streaming sub-agents need enough response budget to carry large tool-call
// arguments, especially write_file content. The API bills generated tokens, not
// the requested ceiling.
const SUBAGENT_RESPONSE_MAX_TOKENS: u32 = 16_384;
const MAX_CONSECUTIVE_TRUNCATED_SUBAGENT_RESPONSES: u32 = 5;
const SUBAGENT_TRANSIENT_PROVIDER_MAX_RETRIES: u32 = 2;
const SUBAGENT_TRANSIENT_PROVIDER_INITIAL_BACKOFF: Duration = Duration::from_millis(250);
/// Per-step LLM API call timeout. Each `create_message` request must complete
/// within this window or the step is treated as timed out. Prevents a single
/// stuck API call from blocking the sub-agent indefinitely.
/// Legacy fallback for the per-step DeepSeek API timeout. The active timeout
/// now travels on `SubAgentRuntime::step_api_timeout` so users can override
/// it via `[subagents] api_timeout_secs` in `~/.deepseek/config.toml`. The
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
const SUBAGENT_STATE_SCHEMA_VERSION: u32 = 1;
const SUBAGENT_STATE_FILE: &str = "subagents.v1.json";
const SUBAGENT_WORKTREE_ROOT_DIR: &str = ".codewhale-worktrees";
const SUBAGENT_RESTART_REASON: &str = "Interrupted by process restart";
const SUBAGENT_QUEUED_LAUNCH_REASON: &str = "queued: waiting for a sub-agent launch slot";
const SUBAGENT_MODEL_WAIT_REASON: &str = "waiting for model response";
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

const VALID_SUBAGENT_TYPES: &str = "general (aliases: general-purpose, general_purpose, worker, default), \
     explore (aliases: exploration, explorer), plan (aliases: planning, planner, awaiter), \
     review (aliases: code-review, code_review, reviewer), implementer (aliases: implement, implementation, builder), \
     verifier (aliases: verify, verification, validator, tester), custom";
/// Role aliases accepted by `normalize_role_alias`. Kept in sync with the
/// match arms below so every input that `SubAgentType::from_str` accepts also
/// resolves to a canonical role (avoids the dual-validation rejection in #2649).
const VALID_ROLE_ALIASES: &str = "default; worker (aliases: general, general-purpose, general_purpose); \
     explorer (aliases: explore, exploration); awaiter (aliases: plan, planning, planner); \
     reviewer (aliases: review, code-review, code_review); implementer (aliases: implement, implementation, builder); \
     verifier (aliases: verify, verification, validator, tester); custom";
const SUBAGENT_TYPE_DESCRIPTION: &str = "Sub-agent type. Accepted vocabulary: general (aliases: general-purpose, general_purpose, worker, default), \
     explore (aliases: exploration, explorer), plan (aliases: planning, planner, awaiter), \
     review (aliases: code-review, code_review, reviewer), implementer (aliases: implement, implementation, builder), \
     verifier (aliases: verify, verification, validator, tester), custom.";
/// Whale species used as friendly names for sub-agents in the UI. The full
/// Cetacea infraorder — baleen whales (Mysticeti), toothed whales
/// (Odontoceti), plus select dolphin species (family Delphinidae) that
/// don't conflate with existing agent type labels. Porpoises (Phocoenidae)
/// are excluded because their name doesn't carry well as a friendly label.
///
/// English and Simplified-Chinese names are interleaved so any newly spawned
/// agent has a roughly even chance of either — the goal is friendly variety,
/// not a strict locale match.
///
/// Taxonomy source: Society for Marine Mammalogy (2025).
pub const WHALE_NICKNAMES: &[&str] = &[
    "Blue",
    "蓝鲸",
    "Humpback",
    "座头鲸",
    "Sperm",
    "抹香鲸",
    "Fin",
    "长须鲸",
    "Sei",
    "塞鲸",
    "Bryde's",
    "布氏鲸",
    "Minke",
    "小须鲸",
    "Antarctic Minke",
    "南极小须鲸",
    "Pygmy Right",
    "小露脊鲸",
    "Omura's",
    "大村鲸",
    "Eden's",
    "艾氏鲸",
    "Rice's",
    "赖斯鲸",
    "Gray",
    "灰鲸",
    "Bowhead",
    "弓头鲸",
    "North Atlantic Right",
    "北大西洋露脊鲸",
    "North Pacific Right",
    "北太平洋露脊鲸",
    "Southern Right",
    "南露脊鲸",
    "Beluga",
    "白鲸",
    "Narwhal",
    "独角鲸",
    "Orca",
    "虎鲸",
    "Pilot",
    "领航鲸",
    "False Killer",
    "伪虎鲸",
    "Pygmy Killer",
    "小虎鲸",
    "Melon-headed",
    "瓜头鲸",
    "Beaked",
    "喙鲸",
    "Cuvier's Beaked",
    "柯氏喙鲸",
    "Baird's Beaked",
    "贝氏喙鲸",
    "Blainville's Beaked",
    "柏氏喙鲸",
    "Ginkgo-toothed Beaked",
    "银杏齿喙鲸",
    "Strap-toothed",
    "带齿喙鲸",
    "Stejneger's Beaked",
    "斯氏喙鲸",
    "Dwarf Sperm",
    "小抹香鲸",
    "Pygmy Sperm",
    "侏儒抹香鲸",
    "Rough-toothed",
    "糙齿海豚",
    "Atlantic Spotted",
    "大西洋斑海豚",
    "Pantropical Spotted",
    "热带斑海豚",
    "Spinner",
    "长吻飞旋海豚",
    "Clymene",
    "短吻飞旋海豚",
    "Striped",
    "条纹海豚",
    "Common Bottlenose",
    "宽吻海豚",
    "Indo-Pacific Bottlenose",
    "印太瓶鼻海豚",
    "Risso's",
    "灰海豚",
    "Commerson's",
    "花斑海豚",
    "Chilean",
    "智利海豚",
    "Heaviside's",
    "海氏矮海豚",
    "Hector's",
    "赫氏矮海豚",
    "Amazon River",
    "亚马逊河豚",
    "Ganges River",
    "恒河豚",
    "Indus River",
    "印度河豚",
    "La Plata",
    "拉普拉塔河豚",
    "Franciscana",
    "拉河豚",
];

/// Return a deterministic whale name for a given agent ID using a hash of
/// the ID string. The same ID always gets the same name — stable across
/// session restarts for persisted agents.
#[must_use]
pub fn whale_name_for_id(id: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    let idx = (hasher.finish() as usize) % WHALE_NICKNAMES.len();
    WHALE_NICKNAMES[idx].to_string()
}

/// Assign a unique whale name for an agent ID, avoiding collisions with
/// names already in `active_names`. If the deterministic name is taken,
/// appends a numeric suffix (e.g. "Orca (2)").
#[must_use]
pub fn assign_unique_whale_name(
    id: &str,
    active_names: &std::collections::HashSet<String>,
) -> String {
    let base = whale_name_for_id(id);
    if !active_names.contains(&base) {
        return base;
    }
    // Deterministic suffix from the same hash to keep it stable
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    let suffix_seed = hasher.finish();
    for i in 2.. {
        let candidate = format!("{base} ({i})");
        if !active_names.contains(&candidate) {
            return candidate;
        }
        // Vary the probe using the seed
        let probe = (suffix_seed.wrapping_add(i as u64)) % 100;
        let candidate2 = format!("{base} ({probe})");
        if !active_names.contains(&candidate2) {
            return candidate2;
        }
    }
    // Fallback (should never reach here)
    format!("{base} ({})", id.get(..4).unwrap_or("?"))
}

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

/// Sub-agent execution types with specialized behavior and tool access.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentType {
    /// General purpose - full tool access for multi-step tasks.
    #[default]
    General,
    /// Fast exploration - read-only tools for codebase search.
    Explore,
    /// Planning - analysis tools only for architectural planning.
    Plan,
    /// Code review - read + analysis tools.
    Review,
    /// Implementation — focused on writing / patching code to satisfy
    /// a specific change. Distinct from `General` in that the prompt
    /// posture pushes hard on landing the change cleanly with the
    /// minimum surrounding edit (#404).
    Implementer,
    /// Verification — focused on running the test suite or other
    /// validation gates and reporting pass/fail with evidence.
    /// Distinct from `Review` in that Review reads code and grades it;
    /// Verifier *runs* tests and reports the outcome (#404).
    Verifier,
    /// Custom tool access defined at spawn time.
    Custom,
}

impl SubAgentType {
    /// Parse a sub-agent type from user input.
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "general" | "general-purpose" | "general_purpose" | "worker" | "default" => {
                Some(Self::General)
            }
            "explore" | "exploration" | "explorer" => Some(Self::Explore),
            "plan" | "planning" | "planner" | "awaiter" => Some(Self::Plan),
            "review" | "code-review" | "code_review" | "reviewer" => Some(Self::Review),
            "implementer" | "implement" | "implementation" | "builder" => Some(Self::Implementer),
            "verifier" | "verify" | "verification" | "validator" | "tester" => Some(Self::Verifier),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Explore => "explore",
            Self::Plan => "plan",
            Self::Review => "review",
            Self::Implementer => "implementer",
            Self::Verifier => "verifier",
            Self::Custom => "custom",
        }
    }

    /// Get the system prompt for this agent type.
    #[must_use]
    pub fn system_prompt(&self) -> String {
        let role_intro = match self {
            Self::General => GENERAL_AGENT_INTRO,
            Self::Explore => EXPLORE_AGENT_INTRO,
            Self::Plan => PLAN_AGENT_INTRO,
            Self::Review => REVIEW_AGENT_INTRO,
            Self::Implementer => IMPLEMENTER_AGENT_INTRO,
            Self::Verifier => VERIFIER_AGENT_INTRO,
            Self::Custom => CUSTOM_AGENT_INTRO,
        };
        format!("{role_intro}{SUBAGENT_OUTPUT_FORMAT}")
    }

    /// Get the default allowed tools for this agent type.
    ///
    /// **Deprecated since v0.6.6.** Default sub-agents now inherit the full
    /// parent registry; the per-type allowlist is advisory only. Pass an explicit
    /// `allowed_tools` array for narrow Custom roles instead.
    #[must_use]
    #[deprecated(
        since = "0.6.6",
        note = "Default sub-agents inherit the full parent registry; pass an explicit allowed_tools list only for narrow Custom roles."
    )]
    pub fn allowed_tools(&self) -> Vec<&'static str> {
        match self {
            Self::General => vec![
                "list_dir",
                "read_file",
                "write_file",
                "edit_file",
                "apply_patch",
                "grep_files",
                "file_search",
                "web.run",
                "web_search",
                "exec_shell",
                "exec_shell_wait",
                "exec_shell_interact",
                "exec_wait",
                "exec_interact",
                "note",
                "checklist_write",
                "checklist_add",
                "checklist_update",
                "checklist_list",
                "todo_write",
                "todo_add",
                "todo_update",
                "todo_list",
                "update_plan",
            ],
            Self::Explore => vec![
                "list_dir",
                "read_file",
                "grep_files",
                "file_search",
                "web.run",
                "web_search",
                "exec_shell",
                "exec_shell_wait",
                "exec_shell_interact",
                "exec_wait",
                "exec_interact",
            ],
            Self::Plan => vec![
                "list_dir",
                "read_file",
                "grep_files",
                "file_search",
                "web.run",
                "note",
                "update_plan",
                "checklist_write",
                "checklist_add",
                "checklist_update",
                "checklist_list",
                "todo_write",
                "todo_add",
                "todo_update",
                "todo_list",
            ],
            Self::Review => vec!["list_dir", "read_file", "grep_files", "file_search", "note"],
            Self::Implementer => vec![
                "list_dir",
                "read_file",
                "write_file",
                "edit_file",
                "apply_patch",
                "grep_files",
                "file_search",
                "exec_shell",
                "exec_shell_wait",
                "exec_shell_interact",
                "exec_wait",
                "exec_interact",
                "note",
                "checklist_write",
                "checklist_add",
                "checklist_update",
                "checklist_list",
                "todo_write",
                "todo_add",
                "todo_update",
                "todo_list",
                "update_plan",
            ],
            Self::Verifier => vec![
                "list_dir",
                "read_file",
                "grep_files",
                "file_search",
                "exec_shell",
                "exec_shell_wait",
                "exec_shell_interact",
                "exec_wait",
                "exec_interact",
                "run_tests",
                "run_verifiers",
                "diagnostics",
                "note",
            ],
            Self::Custom => vec![], // Must be provided by caller.
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
    pub agent_type: SubAgentType,
    pub assignment: SubAgentAssignment,
    #[serde(default)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    pub status: SubAgentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_status: Option<AgentWorkerStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    #[serde(default)]
    pub spawn_depth: u32,
    pub result: Option<String>,
    pub steps_taken: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<SubAgentCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_input: Option<SubAgentNeedsInput>,
    pub duration_ms: u64,
    /// `true` when this agent was loaded from a prior-session persisted
    /// state file rather than spawned in the current session (#405).
    /// Lets listings filter out historical noise by default while
    /// keeping the records reachable via `include_archived=true`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub from_prior_session: bool,
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
    pub agent_type: SubAgentType,
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

impl AgentWorkerRecord {
    fn new(spec: AgentWorkerSpec, now_ms: u64) -> Self {
        let run_id = agent_worker_run_id(&spec);
        let artifacts = default_subagent_artifacts(&run_id);
        let follow_up = follow_up_target_for_spec(&spec);
        let takeover = takeover_target_for_spec(&spec);
        let recommended_action =
            recommended_action_for_worker_status(AgentWorkerStatus::Starting, &spec);
        Self {
            parent_run_id: spec.parent_run_id.clone(),
            spec,
            actor_kind: default_subagent_actor_kind(),
            follow_up,
            takeover,
            artifacts,
            usage: default_agent_run_usage(),
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
            description:
                "Use the projection transcript_handle with handle_read for the child transcript."
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
    spec
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
    profile.background = true;
    profile
}

fn worker_profile_for_spawn(
    runtime: &SubAgentRuntime,
    agent_type: &SubAgentType,
    tool_profile: &AgentWorkerToolProfile,
    effective_model: &str,
    model_route: Option<ModelRoute>,
) -> WorkerRuntimeProfile {
    let mut requested = WorkerRuntimeProfile::for_role(agent_type.clone());
    requested.tools = worker_tool_scope(tool_profile);
    requested.model = model_route.unwrap_or_else(|| ModelRoute::Fixed(effective_model.to_string()));
    requested.provider = Some(runtime.client.api_provider().as_str().to_string());
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
    pub nickname: Option<String>,
    pub fork_context: bool,
    pub token_budget: Option<u64>,
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
            "auto" | "automatic" => Ok(Self::Auto),
            "off" | "disabled" | "none" | "false" => Ok(Self::Effort(ReasoningEffort::Off)),
            "low" | "minimal" => Ok(Self::Effort(ReasoningEffort::Low)),
            "medium" | "mid" => Ok(Self::Effort(ReasoningEffort::Medium)),
            "high" => Ok(Self::Effort(ReasoningEffort::High)),
            "max" | "maximum" | "xhigh" | "ultracode" => Ok(Self::Effort(ReasoningEffort::Max)),
            _ => Err(ToolError::invalid_input(
                "thinking must be one of: inherit, auto, off, low, medium, high, max".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone)]
struct SubAgentInput {
    text: String,
    interrupt: bool,
}

#[derive(Debug, Clone)]
struct SpawnRequest {
    session_name: Option<String>,
    prompt: String,
    agent_type: SubAgentType,
    assignment: SubAgentAssignment,
    allowed_tools: Option<Vec<String>>,
    model: Option<String>,
    model_strength: SubAgentModelStrength,
    thinking: SubAgentThinking,
    /// Optional working directory for the child. Must canonicalize to a path
    /// inside the parent's workspace. For first-class git worktree isolation,
    /// use `worktree` instead of pre-creating a cwd by hand.
    cwd: Option<PathBuf>,
    /// Optional first-class git worktree isolation. When set, CodeWhale
    /// creates a sibling worktree/branch and runs the child from that checkout.
    worktree: Option<SubAgentWorktreeRequest>,
    /// Optional file path for cache-aware resident mode (#529). When set,
    /// the child's prompt is prefixed with the file contents for prefix-cache
    /// locality. A global ownership table prevents two agents from holding
    /// a resident lease on the same file simultaneously.
    resident_file: Option<String>,
    /// Optional continuable checkpoint from a prior interrupted child. The
    /// status/peek projection exposes this object; passing it back into start
    /// seeds the replacement child with the bounded checkpoint tail.
    resume_checkpoint: Option<SubAgentCheckpoint>,
    /// When true, seed the child with the parent's system prompt and message
    /// prefix before appending the child task.
    fork_context: bool,
    /// Legacy recursion budget for descendants. The model-facing child tool
    /// surface is leaf-only; this remains for persisted/internal records.
    max_depth: Option<u32>,
    /// Optional aggregate token budget for this child and its descendants.
    /// When unset, the child inherits the parent's budget pool or the
    /// configured root default.
    token_budget: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SubAgentWorktreeRequest {
    branch: Option<String>,
    path: Option<PathBuf>,
    base_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentUsageBudgetScope {
    scope_id: String,
    limit: u64,
    spent: u64,
    remaining: u64,
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
    agent_type: SubAgentType,
    prompt: String,
    assignment: SubAgentAssignment,
    #[serde(default)]
    model: String,
    #[serde(default)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSubAgentState {
    schema_version: u32,
    agents: Vec<PersistedSubAgent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    workers: Vec<AgentWorkerRecord>,
}

impl Default for PersistedSubAgentState {
    fn default() -> Self {
        Self {
            schema_version: SUBAGENT_STATE_SCHEMA_VERSION,
            agents: Vec::new(),
            workers: Vec::new(),
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
/// transcript per `prompts/constitution.md`.
#[derive(Debug, Clone)]
pub struct SubAgentCompletion {
    /// The completing child's agent id. Held for routing/logging — the
    /// engine's turn loop does not currently key on it (it just injects
    /// the payload), but downstream tooling and tests need the field.
    #[allow(dead_code)]
    pub agent_id: String,
    /// Human summary on line 1, sentinel on line 2. Same payload shape as
    /// `Event::AgentComplete::result`.
    pub payload: String,
}

/// Parent transcript snapshot available to sub-agents that opt into context
/// forking. The system prompt and leading messages are kept byte-identical to
/// the parent request so DeepSeek's prefix cache can reuse the warmed prefix.
#[derive(Clone, Debug)]
pub struct SubAgentForkContext {
    pub system: Option<SystemPrompt>,
    pub messages: Vec<Message>,
    pub structured_state_block: Option<String>,
}

/// Runtime configuration for spawning sub-agents.
///
/// Carries everything a child needs to (a) build its own tool registry —
/// including the manager so grandchildren can spawn — and (b) cooperate with
/// lifecycle cancellation and depth caps. `child_runtime()` links cancellation
/// tokens, while `background_runtime()` deliberately detaches long-running
/// `agent` sessions from the caller's turn token.
#[derive(Clone)]
pub struct SubAgentRuntime {
    pub client: DeepSeekClient,
    pub model: String,
    pub auto_model: bool,
    pub reasoning_effort: Option<String>,
    pub reasoning_effort_auto: bool,
    pub role_models: HashMap<String, String>,
    pub context: ToolContext,
    pub allow_shell: bool,
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
    /// a child token from the parent; model-visible `agent` uses
    /// `background_runtime()` to replace that token with a detached one.
    pub cancel_token: CancellationToken,
    /// Structured progress / lifecycle stream. Cloned across children so the
    /// whole spawn tree publishes into one ordered, fan-out-able mailbox.
    /// `None` only when no consumer is wired (legacy entry points / tests).
    pub mailbox: Option<Mailbox>,
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
    /// Resolved from `[subagents] api_timeout_secs` (clamped to 1..=1800) at
    /// engine construction so a slow but legitimate model turn does not
    /// false-timeout the child mid-thinking. `child_runtime()` and
    /// `background_runtime()` preserve the parent's value (#1806, #1808).
    pub step_api_timeout: Duration,
    /// Wall-clock budget for a single tool execution within a sub-agent step.
    /// Defaults to `DEFAULT_TOOL_TIMEOUT`; the engine may override it so a long
    /// but legitimate tool run is not killed mid-flight. `child_runtime()`
    /// preserves the parent's value.
    pub tool_timeout: Duration,
    /// Default directory for Xiaomi MiMo speech/TTS tool outputs inherited by
    /// child registries. Keeps parent and sub-agent `speech` / `tts` tools on
    /// the same `[speech].output_dir` / env override.
    pub speech_output_dir: Option<PathBuf>,
    /// Shared todo list — the parent's `SharedTodoList`, cloned into each
    /// child so sub-agent `checklist_update` calls are visible in the
    /// Work sidebar live. Without this, each child gets a fresh isolated
    /// list and the parent never sees child progress until completion.
    pub todos: SharedTodoList,
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
            model,
            auto_model: false,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            role_models: HashMap::new(),
            context,
            allow_shell,
            agent_tool_surface_options: AgentToolSurfaceOptions::new(
                ShellPolicy::from_legacy_allow_shell(allow_shell),
            ),
            worker_profile: WorkerRuntimeProfile::for_role(SubAgentType::General),
            event_tx,
            manager,
            spawn_depth: 0,
            parent_agent_id: None,
            max_spawn_depth: DEFAULT_MAX_SPAWN_DEPTH,
            cancel_token: CancellationToken::new(),
            mailbox: None,
            parent_completion_tx: None,
            fork_context: None,
            mcp_pool: None,
            step_api_timeout: DEFAULT_STEP_API_TIMEOUT,
            tool_timeout: DEFAULT_TOOL_TIMEOUT,
            speech_output_dir: None,
            todos: crate::tools::todo::new_shared_todo_list(),
        }
    }

    /// Attach the parent's shared todo list so sub-agent `checklist_update`
    /// calls are visible in the Work sidebar live. Without this, children
    /// get a fresh isolated list.
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
    /// without waiting the legacy 120 seconds (#1806, #1808).
    #[must_use]
    pub fn with_step_api_timeout(mut self, timeout: Duration) -> Self {
        self.step_api_timeout = timeout;
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

    /// Replace the cancellation token (e.g. when the engine constructs the
    /// runtime alongside a mailbox bound to the same token).
    #[must_use]
    #[allow(dead_code)] // wired by #128 alongside `with_mailbox`.
    pub fn with_cancel_token(mut self, token: CancellationToken) -> Self {
        self.cancel_token = token;
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
    /// turn cancellation token. Background sub-agents should keep running when
    /// the parent turn is cancelled; explicit agent cancellation still
    /// aborts their task handles through the manager.
    #[must_use]
    pub fn background_runtime(&self) -> Self {
        let mut runtime = self.child_runtime();
        let token = CancellationToken::new();
        runtime.cancel_token = token.clone();
        runtime.context.cancel_token = Some(token);
        runtime
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
        Self {
            client: self.client.clone(),
            model: self.model.clone(),
            auto_model: self.auto_model,
            reasoning_effort: self.reasoning_effort.clone(),
            reasoning_effort_auto: self.reasoning_effort_auto,
            role_models: self.role_models.clone(),
            context: child_context,
            allow_shell: self.allow_shell,
            agent_tool_surface_options: self.agent_tool_surface_options.clone(),
            worker_profile: self.worker_profile.clone(),
            event_tx: self.event_tx.clone(),
            manager: self.manager.clone(),
            spawn_depth: self.spawn_depth + 1,
            parent_agent_id: self.parent_agent_id.clone(),
            max_spawn_depth: self.max_spawn_depth,
            cancel_token: self.cancel_token.child_token(),
            mailbox: self.mailbox.clone(),
            parent_completion_tx: self.parent_completion_tx.clone(),
            fork_context: self.fork_context.clone(),
            mcp_pool: self.mcp_pool.clone(),
            step_api_timeout: self.step_api_timeout,
            tool_timeout: self.tool_timeout,
            speech_output_dir: self.speech_output_dir.clone(),
            todos: self.todos.clone(),
        }
    }

    /// Whether the next spawn would exceed the depth cap.
    #[must_use]
    pub fn would_exceed_depth(&self) -> bool {
        self.spawn_depth + 1 > self.max_spawn_depth
    }
}

/// A running sub-agent instance.
pub struct SubAgent {
    pub id: String,
    pub session_name: String,
    pub fork_context: bool,
    pub agent_type: SubAgentType,
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
    pub workspace: PathBuf,
    input_tx: Option<mpsc::UnboundedSender<SubAgentInput>>,
    task_handle: Option<JoinHandle<()>>,
}

impl SubAgent {
    /// Create a new sub-agent. The `id` is generated by the caller so that
    /// deterministic whale-naming can hash the ID before construction.
    #[allow(clippy::too_many_arguments)]
    fn new(
        id: String,
        agent_type: SubAgentType,
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
            workspace,
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
            parent_run_id: None,
            spawn_depth: 0,
            result: self.result.clone(),
            steps_taken: self.steps_taken,
            checkpoint: self.checkpoint.clone(),
            needs_input: self.needs_input.clone(),
            duration_ms: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            // Snapshots from the agent itself don't know the manager's
            // current boot id, so default to false. The manager fills
            // this in when it produces a snapshot via its own
            // `snapshot_for_listing` helper (#405).
            from_prior_session: false,
        }
    }
}

/// Manager for active sub-agents.
pub struct SubAgentManager {
    agents: HashMap<String, SubAgent>,
    worker_records: HashMap<String, AgentWorkerRecord>,
    worker_event_seq: u64,
    #[allow(dead_code)] // Stored for future workspace-scoped operations
    workspace: PathBuf,
    state_path: Option<PathBuf>,
    max_steps: u32,
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
}

impl SubAgentManager {
    /// Create a new manager for sub-agents.
    #[must_use]
    pub fn new(workspace: PathBuf, max_agents: usize) -> Self {
        Self {
            agents: HashMap::new(),
            worker_records: HashMap::new(),
            worker_event_seq: 0,
            workspace,
            state_path: None,
            max_steps: DEFAULT_MAX_STEPS,
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

    /// Return the boot id this manager stamps on agents it spawns.
    /// Exposed for tests; internal callers use the field directly.
    #[cfg(test)]
    pub fn session_boot_id(&self) -> &str {
        &self.current_session_boot_id
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
        let path = checked_subagent_state_path(&self.workspace, path)?;
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
                nickname: agent.nickname.clone(),
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
            });
        }
        agents.sort_by(|a, b| a.id.cmp(&b.id));

        let payload = PersistedSubAgentState {
            schema_version: SUBAGENT_STATE_SCHEMA_VERSION,
            agents,
            workers: self.sorted_worker_records(),
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
        let Some((path, payload)) = self.build_persist_payload()? else {
            // Nothing to persist — return a no-op handle.
            return Ok(std::thread::spawn(|| {}));
        };
        let workspace = self.workspace.clone();
        // Spawn disk I/O off the write-lock hot path.  `payload` is fully
        // owned (cloned from `self.agents`) so it is `Send` and safe to move.
        let handle = std::thread::spawn(move || {
            if let Err(err) = write_json_atomic(&workspace, &path, &payload) {
                tracing::warn!(target: "subagent", ?err, "failed to persist sub-agent state");
            }
        });
        Ok(handle)
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
    /// Unlike [`persist_state`], this performs disk I/O **synchronously** to
    /// guarantee data is flushed before the process exits.
    pub fn flush_pending_persist(&mut self) {
        if self.persist_pending {
            self.last_persist_at = Some(Instant::now());
            self.persist_pending = false;
            // Synchronous disk I/O — safe because we are shutting down and no
            // callers depend on releasing the write lock quickly.
            if let Ok(Some((path, payload))) = self.build_persist_payload() {
                if let Err(err) = write_json_atomic(&self.workspace, &path, &payload) {
                    tracing::warn!(target: "subagent", ?err, "failed to flush pending sub-agent state");
                }
            }
        }
    }

    fn load_state(&mut self) -> Result<()> {
        let Some(path) = self.state_path.as_ref() else {
            return Ok(());
        };
        let path = checked_subagent_state_path(&self.workspace, path)?;

        // If canonical path doesn't exist, try legacy .deepseek/ path for one-time
        // migration. The next persist will write to the canonical .codewhale/ path.
        let path = if path.exists() {
            path
        } else {
            let legacy = checked_subagent_state_path(
                &self.workspace,
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

        let raw = read_subagent_state_file(&self.workspace, &path)?;
        let state = serde_json::from_str::<PersistedSubAgentState>(&raw)?;
        if state.schema_version != SUBAGENT_STATE_SCHEMA_VERSION {
            return Err(anyhow!(
                "Unsupported sub-agent state schema {}",
                state.schema_version
            ));
        }

        self.agents.clear();
        self.worker_records.clear();
        for persisted in state.agents {
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
                assignment: persisted.assignment,
                model: if persisted.model.is_empty() {
                    "unknown".to_string()
                } else {
                    persisted.model
                },
                nickname: persisted.nickname,
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
        self.refresh_all_budget_scopes();
        self.prune_worker_records();

        Ok(())
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
        if self.worker_records.len() <= MAX_AGENT_WORKER_RECORDS {
            return;
        }
        let keep_ids: std::collections::HashSet<String> = self
            .sorted_worker_records()
            .into_iter()
            .take(MAX_AGENT_WORKER_RECORDS)
            .map(|record| record.spec.worker_id)
            .collect();
        self.worker_records
            .retain(|worker_id, _| keep_ids.contains(worker_id));
    }

    pub fn register_worker(&mut self, spec: AgentWorkerSpec) {
        let worker_id = spec.worker_id.clone();
        let now_ms = epoch_millis_now();
        let mut record = AgentWorkerRecord::new(normalize_worker_spec(spec), now_ms);
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

    pub fn list_worker_records(&self) -> Vec<AgentWorkerRecord> {
        self.sorted_worker_records()
    }

    pub fn get_worker_record(&self, worker_id: &str) -> Option<AgentWorkerRecord> {
        self.worker_records.get(worker_id).cloned()
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
                "Sub-agent token budget exhausted for scope {scope_id}: {spent}/{limit} tokens spent, {remaining} remaining. Wait for the parent/Workflow to summarize results or start a new agent run with an explicit token_budget override."
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

    fn record_worker_usage(&mut self, worker_id: &str, usage: &Usage) {
        let now_ms = epoch_millis_now();
        let total_delta = usage_total_tokens(usage);
        let Some(record) = self.worker_records.get_mut(worker_id) else {
            return;
        };
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
    }

    fn record_worker_progress(&mut self, worker_id: &str, message: String) {
        let (status, step, tool_name) = worker_progress_event_parts(&message);
        self.record_worker_event(worker_id, status, Some(message), step, tool_name);
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
        self.record_worker_event(worker_id, status, message, Some(result.steps_taken), None);
        if let Some(record) = self.worker_records.get_mut(worker_id) {
            record.result_summary = result.result.clone();
            record.steps_taken = result.steps_taken;
            if let SubAgentStatus::Failed(err) = &result.status {
                record.error = Some(err.clone());
            }
        }
    }

    fn fail_worker(&mut self, worker_id: &str, error: String) {
        self.record_worker_event(
            worker_id,
            AgentWorkerStatus::Failed,
            Some(error.clone()),
            None,
            None,
        );
        if let Some(record) = self.worker_records.get_mut(worker_id) {
            record.error = Some(error);
        }
    }

    pub fn cancel_agent(&mut self, agent_ref: &str) -> Result<SubAgentResult> {
        let agent_id = self.resolve_agent_ref(agent_ref)?;
        let snapshot = {
            let agent = self
                .agents
                .get_mut(&agent_id)
                .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;
            if agent.status != SubAgentStatus::Running {
                return Ok(agent.snapshot());
            }
            agent.status = SubAgentStatus::Cancelled;
            agent.result = Some("Cancelled by parent request.".to_string());
            release_resident_leases_for(&agent.id);
            if let Some(handle) = agent.task_handle.take() {
                handle.abort();
            }
            agent.input_tx = None;
            agent.snapshot()
        };
        self.record_worker_event(
            &agent_id,
            AgentWorkerStatus::Cancelled,
            snapshot.result.clone(),
            Some(snapshot.steps_taken),
            None,
        );
        self.persist_state_best_effort();
        Ok(snapshot)
    }

    /// Count running agents.
    pub fn running_count(&self) -> usize {
        self.admitted_count()
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

    /// Spawn a new background sub-agent.
    pub fn spawn_background(
        &mut self,
        manager_handle: SharedSubAgentManager,
        runtime: SubAgentRuntime,
        agent_type: SubAgentType,
        prompt: String,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<SubAgentResult> {
        self.spawn_background_with_assignment(
            manager_handle,
            runtime,
            agent_type,
            prompt.clone(),
            SubAgentAssignment::new(prompt, None),
            allowed_tools,
        )
    }

    /// Spawn a new background sub-agent with explicit assignment metadata.
    pub fn spawn_background_with_assignment(
        &mut self,
        manager_handle: SharedSubAgentManager,
        runtime: SubAgentRuntime,
        agent_type: SubAgentType,
        prompt: String,
        assignment: SubAgentAssignment,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<SubAgentResult> {
        self.spawn_background_with_assignment_options(
            manager_handle,
            runtime,
            agent_type,
            prompt,
            assignment,
            allowed_tools,
            SubAgentSpawnOptions::default(),
        )
    }

    /// Spawn a new background sub-agent with explicit assignment and display
    /// metadata.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_background_with_assignment_options(
        &mut self,
        manager_handle: SharedSubAgentManager,
        mut runtime: SubAgentRuntime,
        agent_type: SubAgentType,
        prompt: String,
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
        let nickname = options
            .nickname
            .or_else(|| Some(assign_unique_whale_name(&agent_id, &active_names)));
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
            if let Some(existing) = self
                .agents
                .values()
                .find(|existing| existing.session_name == name)
            {
                // #3020: Include elapsed time so the parent can distinguish a
                // live worker from a stale/failed earlier spawn (#2656).
                let elapsed = existing.started_at.elapsed();
                let since = if elapsed.as_secs() < 120 {
                    format!("{}s ago", elapsed.as_secs())
                } else {
                    let mins = elapsed.as_secs() / 60;
                    let secs = elapsed.as_secs() % 60;
                    format!("{mins}m{secs}s ago")
                };
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
        let max_steps = self.max_steps;
        let tool_profile = match tools.clone() {
            Some(tools) => AgentWorkerToolProfile::Explicit(tools),
            None => AgentWorkerToolProfile::Inherited,
        };
        let runtime_profile = worker_profile_for_spawn(
            &runtime,
            &agent_type,
            &tool_profile,
            &agent.model,
            options.model_route.clone(),
        );
        runtime.worker_profile = runtime_profile.clone();
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
            runtime_profile,
            max_steps,
            spawn_depth: runtime.spawn_depth,
            max_spawn_depth: runtime.max_spawn_depth,
        };
        self.register_worker(worker_spec);
        if let Some(scope) = budget_scope {
            self.attach_budget_scope(&agent_id, scope);
        }

        if let Some(event_tx) = runtime.event_tx.clone() {
            let _ = event_tx.try_send(Event::AgentSpawned {
                id: agent_id.clone(),
                prompt: prompt.clone(),
                parent_run_id: runtime.parent_agent_id.clone(),
                spawn_depth: runtime.spawn_depth,
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
            input_rx,
            launch_gate,
        };
        let handle = spawn_supervised(
            "subagent-task",
            std::panic::Location::caller(),
            run_subagent_task(task),
        );
        agent.task_handle = Some(handle);
        self.agents.insert(agent_id.clone(), agent);
        self.persist_state_best_effort();

        Ok(self
            .agents
            .get(&agent_id)
            .expect("agent should exist after spawn")
            .snapshot())
    }

    /// Get the current snapshot for an agent.
    pub fn get_result(&self, agent_id: &str) -> Result<SubAgentResult> {
        let agent = self
            .agents
            .get(agent_id)
            .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;
        Ok(agent.snapshot())
    }

    pub fn get_result_by_ref(&self, agent_ref: &str) -> Result<SubAgentResult> {
        let agent_id = self.resolve_agent_ref(agent_ref)?;
        self.get_result(&agent_id)
    }

    pub fn terminal_results_excluding(
        &self,
        delivered_ids: &std::collections::HashSet<String>,
    ) -> Vec<SubAgentResult> {
        let mut results = self
            .agents
            .values()
            .filter(|agent| agent.status != SubAgentStatus::Running)
            .filter(|agent| agent.session_boot_id == self.current_session_boot_id)
            .filter(|agent| {
                self.worker_records
                    .get(&agent.id)
                    .is_none_or(|record| record.spec.parent_run_id.is_none())
            })
            .filter(|agent| !delivered_ids.contains(&agent.id))
            .map(SubAgent::snapshot)
            .collect::<Vec<_>>();
        results.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        results
    }

    /// Resolve either a durable agent id or a model-facing session name.
    fn resolve_agent_ref(&self, agent_ref: &str) -> Result<String> {
        let agent_ref = agent_ref.trim();
        if self.agents.contains_key(agent_ref) {
            return Ok(agent_ref.to_string());
        }

        let matches = self
            .agents
            .values()
            .filter(|agent| agent.session_name == agent_ref)
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

    /// List all agents and their status.
    #[must_use]
    /// Snapshot a single agent and tag it with the manager's
    /// classification. The bare `SubAgent::snapshot` defaults
    /// `from_prior_session` to `false`; only the manager knows the
    /// matching boot id, so listing goes through here.
    fn snapshot_for_listing(&self, agent: &SubAgent) -> SubAgentResult {
        let mut snap = agent.snapshot();
        snap.from_prior_session = self.is_from_prior_session(agent);
        if let Some(record) = self.worker_records.get(&agent.id) {
            snap.worker_status = Some(record.status);
            snap.parent_run_id = record
                .parent_run_id
                .clone()
                .or_else(|| record.spec.parent_run_id.clone());
            snap.spawn_depth = record.spec.spawn_depth;
        }
        snap
    }

    /// List all agents currently held by the manager, regardless of
    /// session origin. Use [`Self::list_filtered`] in user-facing tool
    /// paths so prior-session agents stay hidden by default (#405).
    pub fn list(&self) -> Vec<SubAgentResult> {
        self.agents
            .values()
            .map(|agent| self.snapshot_for_listing(agent))
            .collect()
    }

    /// List agents respecting the session-boundary filter (#405).
    ///
    /// `include_archived = false` drops
    /// any prior-session agent that is no longer running. Prior-session
    /// agents that are still `Running` (e.g. interrupted by a process
    /// restart) stay visible — they may matter for ongoing recovery.
    ///
    /// `include_archived = true` returns everything, with the
    /// `from_prior_session` flag on each `SubAgentResult` so the model
    /// can tell active and archived apart at a glance.
    pub fn list_filtered(&self, include_archived: bool) -> Vec<SubAgentResult> {
        self.agents
            .values()
            .filter(|agent| {
                if include_archived {
                    return true;
                }
                if agent.status == SubAgentStatus::Running {
                    return true;
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
        let before = self.agents.len();
        let mut auto_cancelled = 0;
        let timeout = self.running_heartbeat_timeout;
        let mut worker_cancellations = Vec::new();
        for agent in self.agents.values_mut() {
            if agent.status == SubAgentStatus::Running
                && agent.task_handle.is_some()
                && agent.last_activity_at.elapsed() >= timeout
            {
                tracing::warn!(
                    target: "subagent",
                    agent_id = %agent.id,
                    timeout_secs = timeout.as_secs(),
                    "auto-cancelling stale sub-agent with no manager-visible progress"
                );
                agent.status = SubAgentStatus::Cancelled;
                agent.result = Some(format!(
                    "Auto-cancelled after {}s without sub-agent progress.",
                    timeout.as_secs()
                ));
                release_resident_leases_for(&agent.id);
                if let Some(handle) = agent.task_handle.take() {
                    handle.abort();
                }
                agent.input_tx = None;
                worker_cancellations.push((
                    agent.id.clone(),
                    agent.result.clone(),
                    agent.steps_taken,
                ));
                auto_cancelled += 1;
            }
        }
        for (agent_id, message, steps_taken) in worker_cancellations {
            self.record_worker_event(
                &agent_id,
                AgentWorkerStatus::Cancelled,
                message,
                Some(steps_taken),
                None,
            );
        }
        self.agents.retain(|_, agent| {
            if agent.status == SubAgentStatus::Running {
                true
            } else {
                agent.started_at.elapsed() < max_age
            }
        });
        if self.agents.len() != before || auto_cancelled > 0 {
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
            .map_or(true, |last| last.elapsed() >= min_interval)
    }

    fn update_from_result(&mut self, agent_id: &str, result: SubAgentResult) {
        let mut changed = false;
        if let Some(agent) = self.agents.get_mut(agent_id) {
            agent.status = result.status.clone();
            agent.assignment = result.assignment.clone();
            agent.result = result.result.clone();
            agent.steps_taken = result.steps_taken;
            agent.checkpoint = result.checkpoint.clone();
            agent.needs_input = result.needs_input.clone();
            if result.status != SubAgentStatus::Running {
                agent.input_tx = None;
            }
            agent.task_handle = None;
            changed = true;
        }
        self.complete_worker_from_result(agent_id, &result);
        if changed {
            self.persist_state_best_effort();
        }
    }

    fn update_failed(&mut self, agent_id: &str, error: String) {
        let mut changed = false;
        if let Some(agent) = self.agents.get_mut(agent_id) {
            agent.status = SubAgentStatus::Failed(error.clone());
            release_resident_leases_for(agent_id);
            agent.input_tx = None;
            agent.task_handle = None;
            changed = true;
        }
        self.fail_worker(agent_id, error);
        if changed {
            self.persist_state_best_effort();
        }
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

    fn interrupt_with_checkpoint(
        &mut self,
        agent_id: &str,
        reason: String,
        checkpoint: SubAgentCheckpoint,
        needs_input: Option<SubAgentNeedsInput>,
    ) -> Result<SubAgentResult> {
        let snapshot = {
            let agent = self
                .agents
                .get_mut(agent_id)
                .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;
            agent.status = SubAgentStatus::Interrupted(reason.clone());
            agent.result = Some(reason);
            agent.steps_taken = checkpoint.steps_taken;
            agent.checkpoint = Some(checkpoint);
            agent.needs_input = needs_input;
            agent.last_activity_at = Instant::now();
            release_resident_leases_for(agent_id);
            agent.snapshot()
        };
        self.record_worker_event(
            agent_id,
            AgentWorkerStatus::Interrupted,
            snapshot.result.clone(),
            Some(snapshot.steps_taken),
            None,
        );
        self.persist_state_best_effort();
        Ok(snapshot)
    }
}

/// Thread-safe wrapper for `SubAgentManager`.
pub type SharedSubAgentManager = Arc<RwLock<SubAgentManager>>;

pub fn load_persisted_agent_worker_records(workspace: &Path) -> Result<Vec<AgentWorkerRecord>> {
    let mut manager = SubAgentManager::new(workspace.to_path_buf(), 1)
        .with_state_path(default_state_path(workspace)?);
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
        worker_record,
    }
}

fn default_state_path(workspace: &Path) -> Result<PathBuf> {
    let workspace = normalize_subagent_workspace(workspace);
    // Canonical post-rebrand state path. On first run the file won't exist yet;
    // write_json_atomic creates parent directories. Legacy .deepseek/state/ data
    // is migrated on load (see load_state).
    checked_subagent_state_path(
        &workspace,
        &Path::new(".codewhale")
            .join("state")
            .join(SUBAGENT_STATE_FILE),
    )
}

fn checked_subagent_state_path(workspace: &Path, path: &Path) -> Result<PathBuf> {
    let workspace = normalize_subagent_workspace(workspace);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
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
    if !state_path.starts_with(&workspace) {
        return Err(anyhow!(
            "sub-agent state path must stay within workspace: {}",
            state_path.display()
        ));
    }
    reject_workspace_relative_symlinks(&workspace, &state_path)?;
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

fn reject_workspace_relative_symlinks(workspace: &Path, path: &Path) -> Result<()> {
    let relative = path.strip_prefix(workspace).map_err(|_| {
        anyhow!(
            "sub-agent state path must stay within workspace: {}",
            path.display()
        )
    })?;
    let mut current = workspace.to_path_buf();
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

fn read_subagent_state_file(workspace: &Path, path: &Path) -> Result<String> {
    let workspace = normalize_subagent_workspace(workspace);
    reject_workspace_relative_symlinks(&workspace, path)?;
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

fn epoch_millis_now() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        Err(_) => 0,
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

fn write_json_atomic<T: Serialize>(workspace: &Path, path: &Path, value: &T) -> Result<()> {
    let workspace = normalize_subagent_workspace(workspace);
    reject_workspace_relative_symlinks(&workspace, path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_string_pretty(value)?;
    let seq = WRITE_JSON_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp_path = path.with_extension(format!("{}.{seq}.tmp", std::process::id()));
    reject_workspace_relative_symlinks(&workspace, &tmp_path)?;
    fs::write(&tmp_path, payload)?;
    if let Err(err) = fs::rename(&tmp_path, path) {
        // Don't leave a stray temp behind if the publish failed.
        let _ = fs::remove_file(&tmp_path);
        return Err(err.into());
    }
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
    let max_agents = max_agents.clamp(1, MAX_SUBAGENTS);
    let state_path = match default_state_path(&workspace) {
        Ok(path) => Some(path),
        Err(err) => {
            tracing::warn!(target: "subagent", ?err, "failed to resolve sub-agent state path");
            None
        }
    };
    let mut manager = SubAgentManager::new(workspace, max_agents)
        .with_admission_limit(max_admitted_agents)
        .with_running_heartbeat_timeout(running_heartbeat_timeout)
        .with_launch_concurrency(launch_concurrency)
        .with_default_token_budget(default_token_budget);
    if let Some(state_path) = state_path {
        manager = manager.with_state_path(state_path);
    }
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
}

impl AgentTool {
    #[must_use]
    pub fn new(manager: SharedSubAgentManager, runtime: SubAgentRuntime) -> Self {
        Self { manager, runtime }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentToolAction {
    Start,
    Status,
    Peek,
    Cancel,
}

fn parse_agent_tool_action(input: &Value) -> Result<AgentToolAction, ToolError> {
    let Some(action) = optional_input_str(input, &["action", "op"]) else {
        return Ok(AgentToolAction::Start);
    };
    match action.trim().to_ascii_lowercase().as_str() {
        "" | "start" | "spawn" | "run" => Ok(AgentToolAction::Start),
        "status" | "list" | "inspect" => Ok(AgentToolAction::Status),
        "peek" | "progress" => Ok(AgentToolAction::Peek),
        "cancel" | "stop" | "abort" => Ok(AgentToolAction::Cancel),
        other => Err(ToolError::invalid_input(format!(
            "Invalid agent action '{other}'. Use start, status, peek, or cancel."
        ))),
    }
}

fn parse_agent_ref(input: &Value) -> Option<String> {
    optional_input_str(input, &["agent_id", "id", "session_name", "name"])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[async_trait]
impl ToolSpec for AgentTool {
    fn name(&self) -> &'static str {
        "agent"
    }

    fn description(&self) -> &'static str {
        concat!(
            "Start, inspect, peek at, or cancel focused child agent tasks through one surface. Use start only for independent work that benefits from a clean context. ",
            "For several independent targets, call agent separately for each target; CodeWhale runs or queues them under runtime capacity and provider rate-limit backpressure. ",
            "The child runs in the background and reports back automatically when finished; keep tiny reads/searches local. ",
            "Use action=status or action=peek with agent_id to inspect progress, and action=cancel with agent_id to stop a running child. Returns session projections with transcript_handle for UI/debug inspection."
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["start", "status", "peek", "cancel"],
                    "description": "start (default) launches a child. status lists current children or inspects agent_id. peek is status for one child. cancel stops a running child by agent_id."
                },
                "agent_id": {
                    "type": "string",
                    "description": "Agent id or session name for action=status, action=peek, or action=cancel."
                },
                "include_archived": {
                    "type": "boolean",
                    "description": "For action=status without agent_id, include prior-session completed agents."
                },
                "name": {
                    "type": "string",
                    "description": "For action=start, optional stable session name. For status/peek/cancel, accepted as an alias for agent_id."
                },
                "prompt": {
                    "type": "string",
                    "description": "Focused task for the child agent. Prefer a compact Subagent Brief with QUESTION, SCOPE, ALREADY_KNOWN, EFFORT, STOP_CONDITION, and OUTPUT."
                },
                "type": {
                    "type": "string",
                    "description": SUBAGENT_TYPE_DESCRIPTION
                },
                "model_strength": {
                    "type": "string",
                    "enum": ["same", "faster"],
                    "description": "Optional child model strength. Use same when the child should be as capable as the current model. Use faster for type=explore, read-only lookup/search, status, or other low-risk tasks that can run on a smaller/faster same-family sibling; CodeWhale maps known families such as DeepSeek V4 Pro to Flash and GLM-5.2 to GLM-5-Turbo. type=explore defaults to faster unless you pass model_strength or model explicitly. No hidden auto-downgrade happens."
                },
                "model": {
                    "type": "string",
                    "description": "Optional exact provider model id for the child. Overrides model_strength. Prefer model_strength unless you know the provider-specific id."
                },
                "thinking": {
                    "type": "string",
                    "enum": ["inherit", "auto", "off", "low", "medium", "high", "max"],
                    "description": "Optional child thinking budget. inherit (default) follows the parent thinking mode. auto chooses from the child prompt. off is best for faster explore/lookups. high is for normal reasoning. max is for hard design/debug/release/security work. Explicit thinking overrides the default off used by model_strength=faster."
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional pre-existing working directory for the child; must be inside the parent workspace. Prefer worktree=true for isolated parallel edit tasks."
                },
                "worktree": {
                    "type": "boolean",
                    "description": "When true, create a fresh git worktree and branch for this child before it starts. Use for parallel edit tasks that must not collide with the parent checkout."
                },
                "worktree_branch": {
                    "type": "string",
                    "description": "Optional branch name for worktree=true. Defaults to codex/agent-<name>-<id>."
                },
                "worktree_base": {
                    "type": "string",
                    "description": "Optional git ref to branch the worktree from. Defaults to HEAD in the parent checkout."
                },
                "worktree_path": {
                    "type": "string",
                    "description": "Optional worktree checkout path. Relative paths are created under the default sibling .codewhale-worktrees directory, not inside the parent checkout."
                },
                "fork_context": {
                    "type": "boolean",
                    "description": "false (default): fresh child context. true: include the current parent context prefix when the child needs it."
                },
                "resume_from_checkpoint": {
                    "type": "object",
                    "description": "For action=start, optional checkpoint object from a prior interrupted child status/peek response. Seeds the new child with checkpoint metadata and the bounded message tail before the continuation prompt."
                },
                "max_depth": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 3,
                    "description": "Optional remaining nested-agent depth budget for this child. Defaults to the configured runtime budget."
                },
                "token_budget": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional aggregate token budget for this child and descendants. When unset, the child inherits the parent budget pool or the configured root default."
                }
            },
            "required": []
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
    fn approval_requirement_for(&self, input: &Value) -> ApprovalRequirement {
        match parse_agent_tool_action(input) {
            Ok(AgentToolAction::Status) | Ok(AgentToolAction::Peek) => ApprovalRequirement::Auto,
            _ => ApprovalRequirement::Required,
        }
    }

    /// #3801: `action=start` launches a background agent and returns immediately —
    /// it is a detached start that should not hold the global tool-exec write
    /// lock while the child spins up.  In auto-approved modes (YOLO) this lets
    /// multiple independent `agent start` calls join a single parallel batch
    /// instead of being serialized N ways.
    fn starts_detached_for(&self, input: &Value) -> bool {
        matches!(parse_agent_tool_action(input), Ok(AgentToolAction::Start))
    }

    /// #3801: Read-only `agent` actions (status, peek) can safely run in
    /// parallel batches.
    fn supports_parallel_for(&self, input: &Value) -> bool {
        matches!(
            parse_agent_tool_action(input),
            Ok(AgentToolAction::Status) | Ok(AgentToolAction::Peek)
        )
    }

    /// #3801: status/peek/cancel actions are read-only queries of manager state.
    fn is_read_only_for(&self, input: &Value) -> bool {
        matches!(
            parse_agent_tool_action(input),
            Ok(AgentToolAction::Status) | Ok(AgentToolAction::Peek)
        )
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let action = parse_agent_tool_action(&input)?;
        match action {
            AgentToolAction::Start => {}
            AgentToolAction::Status | AgentToolAction::Peek => {
                return inspect_agent_from_input(
                    &input,
                    self.manager.clone(),
                    context,
                    matches!(action, AgentToolAction::Peek),
                )
                .await;
            }
            AgentToolAction::Cancel => {
                return cancel_agent_from_input(&input, self.manager.clone(), context).await;
            }
        }
        let snapshot =
            spawn_subagent_from_input(input, self.manager.clone(), self.runtime.clone()).await?;
        let worker_record = {
            let manager = self.manager.read().await;
            manager.get_worker_record(&snapshot.agent_id)
        };
        let projection = subagent_session_projection(snapshot, false, context, worker_record).await;
        let mut tool_result = ToolResult::json(&projection)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        tool_result.metadata = Some(json!({
            "status": projection.status,
            "terminal": projection.terminal,
            "context_mode": projection.context_mode,
            "prefix_cache": projection.prefix_cache,
        }));
        Ok(tool_result)
    }
}

async fn inspect_agent_from_input(
    input: &Value,
    manager: SharedSubAgentManager,
    context: &ToolContext,
    peek: bool,
) -> Result<ToolResult, ToolError> {
    let include_archived =
        parse_optional_bool(input, &["include_archived", "includeArchived"]).unwrap_or(false);

    if let Some(agent_ref) = parse_agent_ref(input) {
        let (snapshot, worker_record) = {
            let mut manager = manager.write().await;
            manager.cleanup(COMPLETED_AGENT_RETENTION);
            let snapshot = manager
                .get_result_by_ref(&agent_ref)
                .map_err(|err| ToolError::invalid_input(err.to_string()))?;
            let worker_record = manager.get_worker_record(&snapshot.agent_id);
            (snapshot, worker_record)
        };
        let projection =
            subagent_session_projection(snapshot, include_archived, context, worker_record).await;
        let mut tool_result = ToolResult::json(&projection)
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
        tool_result.metadata = Some(json!({
            "action": if peek { "peek" } else { "status" },
            "status": projection.status,
            "terminal": projection.terminal,
            "agent_id": projection.agent_id,
        }));
        return Ok(tool_result);
    }

    let snapshots = {
        let mut manager = manager.write().await;
        manager.cleanup(COMPLETED_AGENT_RETENTION);
        manager
            .list_filtered(include_archived)
            .into_iter()
            .map(|snapshot| {
                let worker_record = manager.get_worker_record(&snapshot.agent_id);
                (snapshot, worker_record)
            })
            .collect::<Vec<_>>()
    };

    let mut projections = Vec::with_capacity(snapshots.len());
    for (snapshot, worker_record) in snapshots {
        projections.push(
            subagent_session_projection(snapshot, include_archived, context, worker_record).await,
        );
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

async fn cancel_agent_from_input(
    input: &Value,
    manager: SharedSubAgentManager,
    context: &ToolContext,
) -> Result<ToolResult, ToolError> {
    let agent_ref = parse_agent_ref(input).ok_or_else(|| ToolError::missing_field("agent_id"))?;
    let (snapshot, worker_record) = {
        let mut manager = manager.write().await;
        let snapshot = manager
            .cancel_agent(&agent_ref)
            .map_err(|err| ToolError::invalid_input(err.to_string()))?;
        let worker_record = manager.get_worker_record(&snapshot.agent_id);
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

async fn spawn_subagent_from_input(
    input: Value,
    manager: SharedSubAgentManager,
    runtime: SubAgentRuntime,
) -> Result<SubAgentResult, ToolError> {
    let spawn_request = parse_spawn_request(&input)?;

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

    if spawn_request.worktree.is_some() {
        let manager_guard = manager.read().await;
        manager_guard
            .check_admission_capacity()
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
    }
    let child_workspace = prepare_child_workspace(&runtime.context.workspace, &spawn_request)?;

    let mut child_runtime = runtime.background_runtime();
    if let Some(max_depth) = spawn_request.max_depth {
        child_runtime.max_spawn_depth =
            clamp_child_max_spawn_depth(child_runtime.spawn_depth, max_depth);
    }
    if let Some(workspace) = child_workspace {
        child_runtime.context.workspace = workspace;
    }
    let configured_model = match spawn_request.model.clone() {
        Some(model) => Some(normalize_requested_subagent_model(
            &model,
            "model",
            runtime.client.api_provider(),
        )?),
        None => configured_model_for_role_or_type(
            &runtime,
            spawn_request.assignment.role.as_deref(),
            &spawn_request.agent_type,
        )?,
    };
    let (base_prompt, _resident_conflict) = if let Some(ref file_path) = spawn_request.resident_file
    {
        let abs_path = if std::path::Path::new(file_path).is_absolute() {
            std::path::PathBuf::from(file_path)
        } else {
            runtime.context.workspace.join(file_path)
        };
        let file_contents = std::fs::read_to_string(&abs_path)
            .unwrap_or_else(|e| format!("<!-- resident_file read error: {e} -->"));
        let prefixed = format!(
            "<!-- resident_file: {file_path} -->\n```\n{file_contents}\n```\n\n{}",
            spawn_request.prompt
        );
        let conflict = {
            let leases = RESIDENT_LEASES.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
            let mut guard = leases.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(owner) = guard.get(file_path) {
                Some(format!(
                    "Warning: agent {owner} already holds a resident lease on {file_path}"
                ))
            } else {
                guard.insert(file_path.clone(), "pending".to_string());
                None
            }
        };
        (prefixed, conflict)
    } else {
        (spawn_request.prompt, None)
    };
    let effective_prompt = match spawn_request.resume_checkpoint.as_ref() {
        Some(checkpoint) => prompt_with_resume_checkpoint(&base_prompt, checkpoint),
        None => base_prompt,
    };

    let route = resolve_subagent_assignment_route(
        &runtime,
        configured_model,
        &effective_prompt,
        &spawn_request.agent_type,
        spawn_request.model_strength.model_route(),
        spawn_request.thinking,
    )
    .await;
    child_runtime.model = route.model.clone();
    child_runtime.reasoning_effort = route.reasoning_effort.clone();
    child_runtime.reasoning_effort_auto = false;
    let effective_model = route.model;
    let model_route = route.model_route;

    let mut manager_guard = manager.write().await;

    let result = manager_guard
        .spawn_background_with_assignment_options(
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
                nickname: None,
                fork_context: spawn_request.fork_context,
                token_budget: spawn_request.token_budget,
            },
        )
        .map_err(|e| ToolError::execution_failed(format!("Failed to spawn sub-agent: {e}")))?;

    if let Some(ref file_path) = spawn_request.resident_file
        && let Some(lock) = RESIDENT_LEASES.get()
        && let Ok(mut guard) = lock.lock()
        && let Some(owner) = guard.get_mut(file_path)
        && owner == "pending"
    {
        *owner = result.agent_id.clone();
    }

    Ok(result)
}

// === Sub-agent Execution ===

/// Build the system prompt for a sub-agent.
///
/// Starts with the per-type prompt (`SubAgentType::system_prompt`) and
/// appends a one-line role overlay when `assignment.role` is set. The
/// full role library — TOML overlays from `~/.deepseek/roles/`, the
/// `/roles` slash command, model overrides per role — lands in 0.6.7.
/// For 0.6.6 we just don't drop the role on the floor: the model sees
/// "You are operating in the role of `{name}`." as a final line so its
/// behavior reflects the user's choice.
fn build_subagent_system_prompt(
    agent_type: &SubAgentType,
    assignment: &SubAgentAssignment,
) -> String {
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
    prompt
}

fn subagent_request_system_prompt(
    subagent_system_prompt: &str,
    fork_context: Option<&SubAgentForkContext>,
) -> SystemPrompt {
    fork_context
        .and_then(|context| context.system.clone())
        .unwrap_or_else(|| SystemPrompt::Text(subagent_system_prompt.to_string()))
}

fn build_initial_subagent_messages(
    prompt: &str,
    assignment: &SubAgentAssignment,
    agent_type: &SubAgentType,
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
            build_subagent_system_prompt(agent_type, assignment)
        )));
    }

    messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: build_assignment_prompt(prompt, assignment, agent_type),
            cache_control: None,
        }],
    });

    messages
}

fn system_text_message(text: String) -> Message {
    Message {
        role: "system".to_string(),
        content: vec![ContentBlock::Text {
            text,
            cache_control: None,
        }],
    }
}

fn prompt_with_resume_checkpoint(prompt: &str, checkpoint: &SubAgentCheckpoint) -> String {
    let mut text = format!(
        concat!(
            "Sub-agent checkpoint resume context.\n",
            "Treat the checkpoint message tail below as your own prior state, ",
            "then continue from the latest safe point. Verify untrusted claims ",
            "before relying on them.\n\n",
            "Checkpoint metadata:\n",
            "- checkpoint_id: {}\n",
            "- continuation_handle: {}\n",
            "- original_agent_id: {}\n",
            "- reason: {}\n",
            "- steps_taken: {}\n",
            "- message_count: {}\n",
            "- omitted_messages: {}\n\n",
            "--- checkpoint message tail ---\n"
        ),
        checkpoint.checkpoint_id,
        checkpoint.continuation_handle,
        checkpoint.agent_id,
        checkpoint.reason,
        checkpoint.steps_taken,
        checkpoint.message_count,
        checkpoint.omitted_messages
    );

    for (index, message) in checkpoint.messages.iter().enumerate() {
        text.push_str(&format!(
            "\n[message {} | role={}]\n{}",
            index + 1,
            message.role,
            render_checkpoint_message(message)
        ));
    }

    text.push_str("\n\n--- continue task ---\n");
    text.push_str(prompt);
    text
}

fn render_checkpoint_message(message: &Message) -> String {
    let mut rendered = Vec::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text, .. } => rendered.push(text.clone()),
            ContentBlock::Thinking { thinking, .. } => {
                rendered.push(format!("[thinking]\n{thinking}"));
            }
            ContentBlock::ToolUse { name, input, .. }
            | ContentBlock::ServerToolUse { name, input, .. } => {
                rendered.push(format!("[tool_use:{name}]\n{input}"));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => {
                let error_marker = if is_error.unwrap_or(false) {
                    " error=true"
                } else {
                    ""
                };
                rendered.push(format!(
                    "[tool_result:{tool_use_id}{error_marker}]\n{content}"
                ));
            }
            ContentBlock::ToolSearchToolResult {
                tool_use_id,
                content,
            } => rendered.push(format!("[tool_search_result:{tool_use_id}]\n{content}")),
            ContentBlock::CodeExecutionToolResult {
                tool_use_id,
                content,
            } => rendered.push(format!("[code_execution_result:{tool_use_id}]\n{content}")),
            ContentBlock::ImageUrl { .. } => rendered.push("[image_url omitted]".to_string()),
        }
    }
    rendered.join("\n")
}

struct SubAgentTask {
    manager_handle: SharedSubAgentManager,
    runtime: SubAgentRuntime,
    agent_id: String,
    agent_type: SubAgentType,
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
    input_rx: mpsc::UnboundedReceiver<SubAgentInput>,
    /// Interactive launch gate (#3095). `Some` only for direct (depth-1)
    /// children: the task acquires a permit before its first model step and
    /// holds it until completion, so a fanout burst beyond the limit queues
    /// with a visible reason instead of executing all at once.
    launch_gate: Option<Arc<Semaphore>>,
}

#[allow(clippy::too_many_lines)]
async fn run_subagent_task(task: SubAgentTask) {
    // Interactive launch gate (#3095): direct children acquire a permit
    // before their first model step so a fanout burst beyond the limit
    // queues visibly instead of executing all at once. The permit is held
    // for the lifetime of the task. Cancellation while queued is handled by
    // `run_subagent`'s own first-step cancel check.
    let mut _launch_permit = None;
    if let Some(gate) = task.launch_gate.as_ref() {
        match Arc::clone(gate).try_acquire_owned() {
            Ok(permit) => _launch_permit = Some(permit),
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                _launch_permit = acquire_queued_launch_permit(&task, Arc::clone(gate)).await;
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                crate::logging::warn(format!(
                    "sub-agent launch gate closed for {}; proceeding without backpressure",
                    task.agent_id
                ));
            }
        }
    }

    let result = run_subagent(
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
        task.input_rx,
    )
    .await;

    // Emit BOTH a human-friendly summary (rendered in the parent's
    // sidebar / cell) AND a structured sentinel the model can recognize
    // on its next turn. Format: human summary on the first line,
    // sentinel on the second. The sentinel uses an opaque tag
    // (`codewhale:subagent.done`) to avoid collision with normal user
    // text.
    let model_id = task.runtime.model.clone();
    let (summary, sentinel) = match &result {
        Ok(res) => {
            // Issue #2652: the child's free-text result is its self-report, not
            // verified evidence. Stamp it with a provenance marker: a soft
            // "re-verify" note when short, or a head+tail truncation (reusing
            // the tool-output vocabulary) when it exceeds the wire budget. The
            // resulting `truncated` flag is carried in the sentinel so the
            // parent model can branch on `summary_kind`.
            let raw = summarize_subagent_result(res);
            let (summary, truncated) = stamp_subagent_summary(&raw);
            let sentinel = subagent_done_sentinel(&task.agent_id, res, truncated);
            (summary, sentinel)
        }
        Err(err) => {
            crate::logging::warn(format!(
                "sub-agent {} model request failed: {err:#}",
                task.agent_id
            ));
            let annotated = annotate_child_model_error(&subagent_failure_message(err), &model_id);
            (
                format!("Failed: {annotated}"),
                subagent_failed_sentinel(&task.agent_id, &annotated),
            )
        }
    };

    if let Some(mb) = task.runtime.mailbox.as_ref() {
        let envelope = match &result {
            Ok(_) => MailboxMessage::Completed {
                agent_id: task.agent_id.clone(),
                summary: summary.clone(),
            },
            Err(err) => MailboxMessage::Failed {
                agent_id: task.agent_id.clone(),
                error: annotate_child_model_error(&subagent_failure_message(err), &model_id),
            },
        };
        let _ = mb.send(envelope);
    }

    let payload = format!("{summary}\n{sentinel}");
    let agent_id = task.agent_id.clone();

    // Wake the engine's parent turn loop if this is one of its direct
    // children (issue #756). Issue #1961 also requires emit to happen
    // before marking the manager terminal state so the parent can observe the
    // completion while its "running children" gate is still open. If we
    // update first, the parent can finalize before the completion arrives.
    emit_parent_completion(&task.runtime, &agent_id, &payload);

    let mut manager = task.manager_handle.write().await;
    match &result {
        Ok(res) => manager.update_from_result(&agent_id, res.clone()),
        Err(err) => {
            manager.update_failed(
                &agent_id,
                annotate_child_model_error(&subagent_failure_message(err), &model_id),
            );
        }
    }

    if let Some(event_tx) = task.runtime.event_tx {
        let _ = event_tx.try_send(Event::AgentComplete {
            id: agent_id.clone(),
            result: payload,
        });
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
            record_agent_progress(
                &task.runtime,
                &task.agent_id,
                "cancelled while queued for a sub-agent launch slot".to_string(),
            );
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
        &task.agent_id,
        SUBAGENT_QUEUED_LAUNCH_REASON.to_string(),
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
        agent_id: agent_id.to_string(),
        payload: payload.to_string(),
    });
    true
}

pub(crate) fn subagent_completion_from_result(result: &SubAgentResult) -> SubAgentCompletion {
    let raw = summarize_subagent_result(result);
    let (summary, truncated) = stamp_subagent_summary(&raw);
    let sentinel = match &result.status {
        SubAgentStatus::Failed(error) => subagent_failed_sentinel(&result.agent_id, error),
        _ => subagent_done_sentinel(&result.agent_id, result, truncated),
    };
    SubAgentCompletion {
        agent_id: result.agent_id.clone(),
        payload: format!("{summary}\n{sentinel}"),
    }
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
/// [`stamp_subagent_summary`] (issue #2652); it surfaces as `summary_kind` so
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
    format!("<codewhale:subagent.done>{payload}</codewhale:subagent.done>")
}

/// Build a `<codewhale:subagent.done>` sentinel for a failed child.
///
/// Kept lean: the (annotated) error is on the previous line (`error_location`)
/// so the sentinel only signals completion state rather than re-embedding the
/// error text.
fn subagent_failed_sentinel(agent_id: &str, _err: &str) -> String {
    let payload = json!({
        "agent_id": agent_id,
        "status": "failed",
        "error_location": "previous_line",
    });
    format!("<codewhale:subagent.done>{payload}</codewhale:subagent.done>")
}

fn response_was_truncated(response: &MessageResponse) -> bool {
    response.stop_reason.as_deref() == Some("length")
}

fn truncated_response_tool_results(tool_uses: &[(String, String, Value)]) -> Vec<ContentBlock> {
    tool_uses
        .iter()
        .map(|(tool_id, tool_name, _)| ContentBlock::ToolResult {
            tool_use_id: tool_id.clone(),
            content: format!(
                "Error: the model response was truncated by max_tokens before the tool call arguments for '{tool_name}' could be fully generated. Split large content into smaller writes and retry."
            ),
            is_error: Some(true),
            content_blocks: None,
        })
        .collect()
}

fn truncated_response_text_retry_message() -> Vec<ContentBlock> {
    vec![ContentBlock::Text {
        text: "Error: the model response was truncated by max_tokens. No complete tool call was available, so the partial response was not accepted as the sub-agent result. Retry with a shorter response or split the work into smaller steps.".to_string(),
        cache_control: None,
    }]
}

fn record_truncated_subagent_response(consecutive: &mut u32) -> Result<()> {
    *consecutive = consecutive.saturating_add(1);
    if *consecutive > MAX_CONSECUTIVE_TRUNCATED_SUBAGENT_RESPONSES {
        return Err(anyhow!(
            "Sub-agent response was truncated by max_tokens {count} consecutive times; stopping to avoid an unbounded retry loop.",
            count = *consecutive
        ));
    }
    Ok(())
}

fn reset_truncated_subagent_responses(consecutive: &mut u32) {
    *consecutive = 0;
}

#[allow(clippy::too_many_arguments)]
async fn insert_subagent_full_transcript_handle(
    runtime: &SubAgentRuntime,
    agent_id: &str,
    agent_type: &SubAgentType,
    assignment: &SubAgentAssignment,
    status: &SubAgentStatus,
    result: Option<&String>,
    checkpoint: Option<&SubAgentCheckpoint>,
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
    let (bounded_messages, omitted_messages) =
        bounded_tail_messages(messages, SUBAGENT_TRANSCRIPT_MESSAGE_BUDGET_BYTES);
    let checkpoint_meta = checkpoint.map(|checkpoint| SubAgentCheckpoint {
        omitted_messages: checkpoint.message_count,
        messages: Vec::new(),
        ..checkpoint.clone()
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
        "messages": bounded_messages,
    });
    let mut store = runtime.context.runtime.handle_store.lock().await;
    store.insert_json(format!("agent:{agent_id}"), "full_transcript", payload)
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
    content: String,
) -> (String, Option<PathBuf>) {
    if content.len() <= SPILLOVER_THRESHOLD_BYTES {
        return (content, None);
    }
    let spill_id = format!("sa_{agent_id}_{tool_id}");
    match maybe_spillover(
        &spill_id,
        &content,
        SPILLOVER_THRESHOLD_BYTES,
        SPILLOVER_HEAD_BYTES,
    ) {
        Ok(Some((head, path))) => {
            let footer = format!(
                "\n\n[Sub-agent tool output truncated: {head_kib} KiB of {total_kib} KiB shown. \
                 Full output saved to {path}. Use `read_file` on that path if you need the \
                 elided output.]",
                head_kib = head.len() / 1024,
                total_kib = content.len() / 1024,
                path = path.display(),
            );
            (format!("{head}{footer}"), Some(path))
        }
        Ok(None) => (content, None),
        Err(err) => {
            tracing::warn!(
                target: "subagent",
                ?err,
                agent_id,
                tool_id,
                "sub-agent spillover write failed; passing original content through"
            );
            (content, None)
        }
    }
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
    let (bounded_messages, omitted_messages) =
        bounded_tail_messages(messages, SUBAGENT_CHECKPOINT_MESSAGE_BUDGET_BYTES);
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
) -> std::result::Result<MessageResponse, SubAgentApiRequestFailure> {
    let mut transient_failures = 0u32;

    loop {
        match tokio::time::timeout(
            runtime.step_api_timeout,
            runtime.client.create_message(request.clone()),
        )
        .await
        {
            Ok(Ok(response)) => return Ok(response),
            Ok(Err(err)) if is_transient_subagent_provider_error(&err) => {
                if transient_failures >= SUBAGENT_TRANSIENT_PROVIDER_MAX_RETRIES {
                    let attempts = transient_failures.saturating_add(1);
                    return Err(SubAgentApiRequestFailure::Interrupted {
                        reason: format!(
                            "Transient provider failure after {attempts} API attempt(s): {err}; checkpoint preserved for continuation"
                        ),
                        checkpoint_reason: "api_transient_provider_failure",
                    });
                }

                transient_failures = transient_failures.saturating_add(1);
                let delay = subagent_transient_provider_retry_delay(transient_failures);
                record_agent_progress(
                    runtime,
                    agent_id,
                    format!(
                        "{}: transient provider failure; retrying API request {}/{} in {}ms ({err})",
                        format_step_counter(steps, max_steps),
                        transient_failures,
                        SUBAGENT_TRANSIENT_PROVIDER_MAX_RETRIES,
                        delay.as_millis(),
                    ),
                );
                tokio::time::sleep(delay).await;
            }
            Ok(Err(err)) => return Err(SubAgentApiRequestFailure::Fatal(err)),
            Err(_) => {
                return Err(SubAgentApiRequestFailure::Interrupted {
                    reason: format!(
                        "API call timed out after {}ms; checkpoint preserved for continuation",
                        runtime.step_api_timeout.as_millis()
                    ),
                    checkpoint_reason: "api_timeout",
                });
            }
        }
    }
}

fn record_agent_progress(runtime: &SubAgentRuntime, agent_id: &str, message: impl Into<String>) {
    let message = message.into();
    if let Ok(mut manager) = runtime.manager.try_write() {
        manager.touch(agent_id);
        manager.record_worker_progress(agent_id, message.clone());
    }
    emit_agent_progress(
        runtime.event_tx.as_ref(),
        agent_id,
        message,
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
and distinguish that from evidence you personally verified.\n",
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
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text,
            cache_control: None,
        }],
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_subagent(
    runtime: &SubAgentRuntime,
    agent_id: String,
    agent_type: SubAgentType,
    prompt: String,
    assignment: SubAgentAssignment,
    allowed_tools: Option<Vec<String>>,
    fork_context: bool,
    started_at: Instant,
    max_steps: u32,
    token_budget: Option<u64>,
    mut input_rx: mpsc::UnboundedReceiver<SubAgentInput>,
) -> Result<SubAgentResult> {
    let system_prompt = build_subagent_system_prompt(&agent_type, &assignment);
    let fork_context_enabled = fork_context;
    let fork_context = fork_context_enabled
        .then_some(runtime.fork_context.as_ref())
        .flatten();
    let request_system = subagent_request_system_prompt(&system_prompt, fork_context);
    let mut messages =
        build_initial_subagent_messages(&prompt, &assignment, &agent_type, fork_context);
    let (runtime_for_tools, mut child_completion_rx) = runtime_for_nested_agent_tools(
        runtime,
        &agent_id,
        SubAgentForkContext {
            system: Some(request_system.clone()),
            messages: messages.clone(),
            structured_state_block: None,
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
        // Share the parent's todo list so child checklist updates are visible
        // in the Work sidebar live. Previously each child got a fresh isolated
        // TodoList — parent never saw child progress until completion.
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
    let tools = tool_registry.tools_for_model(&agent_type);
    if let Some(mb) = runtime.mailbox.as_ref() {
        let _ = mb.send(MailboxMessage::started(&agent_id, agent_type.clone()));
    }
    record_agent_progress(
        runtime,
        &agent_id,
        format!("started ({})", agent_type.as_str()),
    );

    let mut steps = 0;
    let mut final_result: Option<String> = None;
    let mut pending_inputs: VecDeque<SubAgentInput> = VecDeque::new();
    let mut consecutive_truncated_responses = 0;
    let mut latest_checkpoint: Option<SubAgentCheckpoint> = None;
    let mut tokens_used: u64 = 0;

    for _step in 0..max_steps {
        // Cooperative cancellation: bail if this session's token was cancelled
        // while we were between steps. Top-level model-visible sub-agents use
        // a detached token so parent turn cancellation does not stop them.
        if runtime.cancel_token.is_cancelled() {
            record_agent_progress(
                runtime,
                &agent_id,
                format!("{}: cancelled", format_step_counter(steps, max_steps)),
            );
            if let Some(mb) = runtime.mailbox.as_ref() {
                let _ = mb.send(MailboxMessage::Cancelled {
                    agent_id: agent_id.clone(),
                });
            }
            let status = SubAgentStatus::Cancelled;
            let duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
            insert_subagent_full_transcript_handle(
                runtime,
                &agent_id,
                &agent_type,
                &assignment,
                &status,
                None,
                latest_checkpoint.as_ref(),
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
                parent_run_id: runtime.parent_agent_id.clone(),
                spawn_depth: runtime.spawn_depth,
                result: None,
                steps_taken: steps,
                checkpoint: latest_checkpoint.clone(),
                needs_input: None,
                duration_ms,
                from_prior_session: false,
            });
        }

        steps += 1;
        record_agent_progress(
            runtime,
            &agent_id,
            format!(
                "{}: requesting model response",
                format_step_counter(steps, max_steps)
            ),
        );

        while let Ok(input) = input_rx.try_recv() {
            if input.interrupt {
                pending_inputs.clear();
            }
            pending_inputs.push_back(input);
        }

        while let Some(input) = pending_inputs.pop_front() {
            if !input.text.trim().is_empty() {
                messages.push(Message {
                    role: "user".to_string(),
                    content: vec![ContentBlock::Text {
                        text: input.text,
                        cache_control: None,
                    }],
                });
            }
        }

        let child_completions = drain_child_completion_events(&mut child_completion_rx);
        if !child_completions.is_empty() {
            let count = child_completions.len();
            record_agent_progress(
                runtime,
                &agent_id,
                format!(
                    "{}: received {count} child sub-agent completion(s)",
                    format_step_counter(steps, max_steps)
                ),
            );
            messages.push(child_completion_runtime_message(&child_completions));
        }

        let request = MessageRequest {
            model: runtime.model.clone(),
            messages: messages.clone(),
            max_tokens: SUBAGENT_RESPONSE_MAX_TOKENS,
            system: Some(request_system.clone()),
            tools: Some(tools.clone()),
            tool_choice: Some(json!({ "type": "auto" })),
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

        // Race the API call against the cancellation token so a parent
        // cancel during a long thinking turn doesn't have to wait for the
        // step timeout.
        let response = tokio::select! {
            biased;
            () = runtime.cancel_token.cancelled() => {
                record_agent_progress(
                    runtime,
                    &agent_id,
                    format!("{}: cancelled mid-request", format_step_counter(steps, max_steps)),
                );
                if let Some(mb) = runtime.mailbox.as_ref() {
                    let _ = mb.send(MailboxMessage::Cancelled {
                        agent_id: agent_id.clone(),
                    });
                }
                let status = SubAgentStatus::Cancelled;
                let duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
                insert_subagent_full_transcript_handle(
                    runtime,
                    &agent_id,
                    &agent_type,
                    &assignment,
                    &status,
                    None,
                    latest_checkpoint.as_ref(),
                    &messages,
                    steps,
                    duration_ms,
                    fork_context_enabled,
                )
                .await;
                return Ok(SubAgentResult {
                    name: agent_id.clone(),
                    agent_id: agent_id.clone(),
                    context_mode: if fork_context_enabled { "forked" } else { "fresh" }.to_string(),
                    fork_context: fork_context_enabled,
                    workspace: Some(runtime.context.workspace.clone()),
                    git_branch: current_git_branch(&runtime.context.workspace),
                    agent_type: agent_type.clone(),
                    assignment: assignment.clone(),
                    model: runtime.model.clone(),
                    nickname: None,
                    status,
                    worker_status: None,
                    parent_run_id: runtime.parent_agent_id.clone(),
                    spawn_depth: runtime.spawn_depth,
                    result: None,
                    steps_taken: steps,
                    checkpoint: latest_checkpoint.clone(),
                    needs_input: None,
                    duration_ms,
                    from_prior_session: false,
                });
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
                    Err(SubAgentApiRequestFailure::Fatal(err)) => return Err(err),
                    Err(SubAgentApiRequestFailure::Interrupted { reason, checkpoint_reason }) => {
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
                            &messages,
                            steps,
                            duration_ms,
                            fork_context_enabled,
                        )
                        .await;
                        let needs_input =
                            needs_input_for_interrupted_checkpoint(&reason, &checkpoint);
                        let interrupted_snapshot = {
                            let mut manager = runtime.manager.write().await;
                            manager.interrupt_with_checkpoint(
                                &agent_id,
                                reason.clone(),
                                checkpoint.clone(),
                                Some(needs_input.clone()),
                            )?
                        };
                        record_agent_progress(
                            runtime,
                            &agent_id,
                            format!(
                                "{}: waiting for user; {}",
                                format_step_counter(steps, max_steps),
                                needs_input.question
                            ),
                        );
                        if let Some(mb) = runtime.mailbox.as_ref() {
                            let _ = mb.send(MailboxMessage::Interrupted {
                                agent_id: agent_id.clone(),
                                reason: reason.clone(),
                            });
                        }
                        return Ok(interrupted_snapshot);
                    }
                }
            }
        };

        let mut tool_uses = Vec::new();

        // Report token usage so the parent's cost counter updates live.
        if let Some(mb) = runtime.mailbox.as_ref() {
            let _ = mb.send(MailboxMessage::token_usage(
                &agent_id,
                response.model.clone(),
                response.usage.clone(),
            ));
        }
        {
            let mut manager = runtime.manager.write().await;
            manager.record_worker_usage(&agent_id, &response.usage);
        }

        // Per-worker token-budget enforcement (#3321): stop a single runaway
        // worker once its accumulated model tokens exceed its own cap. This
        // complements — and does not double-count — the scope-level admission
        // gate (#3319), which bounds aggregate fan-out across siblings. The
        // local accumulator mirrors the manager's `record.usage.total_tokens`
        // (both derive from `response.usage`), so the scope accounting stays
        // consistent and is never inflated by this check.
        tokens_used = tokens_used.saturating_add(usage_total_tokens(&response.usage));
        if let Some(budget) = token_budget {
            if tokens_used > budget {
                record_agent_progress(
                    runtime,
                    &agent_id,
                    format!(
                        "{}: token budget exhausted ({tokens_used}/{budget})",
                        format_step_counter(steps, max_steps)
                    ),
                );
                if let Some(mb) = runtime.mailbox.as_ref() {
                    let _ = mb.send(MailboxMessage::Cancelled {
                        agent_id: agent_id.clone(),
                    });
                }
                let status = SubAgentStatus::BudgetExhausted;
                let duration_ms =
                    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
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
                    parent_run_id: runtime.parent_agent_id.clone(),
                    spawn_depth: runtime.spawn_depth,
                    result: final_result.clone(),
                    steps_taken: steps,
                    checkpoint: latest_checkpoint.clone(),
                    needs_input: None,
                    duration_ms,
                    from_prior_session: false,
                });
            }
        }

        for block in &response.content {
            match block {
                ContentBlock::Text { text, .. } if !text.trim().is_empty() => {
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
            role: "assistant".to_string(),
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

        if response_was_truncated(&response) {
            final_result = None;
            record_truncated_subagent_response(&mut consecutive_truncated_responses)?;
            let progress = if tool_uses.is_empty() {
                "response truncated, returning retry instruction".to_string()
            } else {
                format!(
                    "response truncated, returning {} tool error(s)",
                    tool_uses.len()
                )
            };
            record_agent_progress(
                runtime,
                &agent_id,
                format!("{}: {progress}", format_step_counter(steps, max_steps)),
            );
            messages.push(Message {
                role: "user".to_string(),
                content: if tool_uses.is_empty() {
                    truncated_response_text_retry_message()
                } else {
                    truncated_response_tool_results(&tool_uses)
                },
            });
            latest_checkpoint = Some(
                checkpoint_subagent_progress(
                    runtime,
                    &agent_id,
                    "after_truncated_response_retry_message",
                    &messages,
                    steps,
                    true,
                )
                .await,
            );
            continue;
        }
        reset_truncated_subagent_responses(&mut consecutive_truncated_responses);

        if tool_uses.is_empty() {
            let child_completions = drain_child_completion_events(&mut child_completion_rx);
            if !child_completions.is_empty() {
                let count = child_completions.len();
                record_agent_progress(
                    runtime,
                    &agent_id,
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
                continue;
            }
            while let Ok(input) = input_rx.try_recv() {
                if input.interrupt {
                    pending_inputs.clear();
                }
                pending_inputs.push_back(input);
            }
            if pending_inputs.is_empty() {
                record_agent_progress(
                    runtime,
                    &agent_id,
                    format!("{}: complete", format_step_counter(steps, max_steps)),
                );
                break;
            }
            continue;
        }

        record_agent_progress(
            runtime,
            &agent_id,
            format!(
                "{}: executing {} tool call(s)",
                format_step_counter(steps, max_steps),
                tool_uses.len()
            ),
        );
        let mut tool_results: Vec<ContentBlock> = Vec::new();
        for (tool_id, tool_name, tool_input) in tool_uses {
            let tool_display_name = subagent_progress_tool_display_name(&tool_name);
            record_agent_progress(
                runtime,
                &agent_id,
                format!(
                    "{}: running tool '{tool_display_name}'",
                    format_step_counter(steps, max_steps)
                ),
            );
            if let Some(mb) = runtime.mailbox.as_ref() {
                let _ = mb.send(MailboxMessage::ToolCallStarted {
                    agent_id: agent_id.clone(),
                    tool_name: tool_name.clone(),
                    step: steps,
                });
            }
            let result = match tokio::time::timeout(runtime.tool_timeout, async {
                tool_registry
                    .execute(&agent_id, &tool_name, tool_input)
                    .await
            })
            .await
            {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => format!("Error: {e}"),
                Err(_) => format!("Error: Tool {tool_name} timed out"),
            };
            let tool_ok = !result.starts_with("Error:");
            let (result, spilled_to) = bound_subagent_tool_result(&agent_id, &tool_id, result);
            if let Some(path) = spilled_to.as_ref() {
                record_agent_progress(
                    runtime,
                    &agent_id,
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
                format!(
                    "{}: finished tool '{tool_display_name}'",
                    format_step_counter(steps, max_steps)
                ),
            );
            if let Some(mb) = runtime.mailbox.as_ref() {
                let _ = mb.send(MailboxMessage::ToolCallCompleted {
                    agent_id: agent_id.clone(),
                    tool_name: tool_name.clone(),
                    step: steps,
                    ok: tool_ok,
                });
            }

            tool_results.push(ContentBlock::ToolResult {
                tool_use_id: tool_id,
                content: result,
                is_error: None,
                content_blocks: None,
            });
        }

        if !tool_results.is_empty() {
            messages.push(Message {
                role: "user".to_string(),
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
        }
    }

    release_resident_leases_for(&agent_id);
    let status = SubAgentStatus::Completed;
    let duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
    latest_checkpoint = Some(build_subagent_checkpoint(
        &agent_id,
        "completed",
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
        parent_run_id: runtime.parent_agent_id.clone(),
        spawn_depth: runtime.spawn_depth,
        result: final_result,
        steps_taken: steps,
        checkpoint: latest_checkpoint,
        needs_input: None,
        duration_ms,
        from_prior_session: false,
    })
}

fn optional_input_str<'a>(input: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .filter_map(|key| input.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
}

fn parse_text_or_items(
    input: &Value,
    text_keys: &[&str],
    items_key: &str,
    required_field: &str,
) -> Result<String, ToolError> {
    let text = optional_input_str(input, text_keys).map(str::to_string);
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
    let session_name = optional_input_str(input, &["name", "session_name"])
        .map(validate_session_name)
        .transpose()?;

    let type_input = optional_input_str(input, &["type", "agent_type", "agent_name"]);
    let role_input = optional_input_str(input, &["role", "agent_role"]);

    let parsed_type = type_input
        .map(|kind| {
            SubAgentType::from_str(kind).ok_or_else(|| {
                ToolError::invalid_input(format!(
                    "Invalid sub-agent type '{kind}'. Use: {VALID_SUBAGENT_TYPES}"
                ))
            })
        })
        .transpose()?;

    let parsed_role_type = role_input
        .map(|role| {
            SubAgentType::from_str(role).ok_or_else(|| {
                ToolError::invalid_input(format!(
                    "Invalid role alias '{role}'. Use: {VALID_ROLE_ALIASES}"
                ))
            })
        })
        .transpose()?;

    if let (Some(type_kind), Some(role_kind)) = (&parsed_type, &parsed_role_type)
        && type_kind != role_kind
    {
        return Err(ToolError::invalid_input(
            "Conflicting type/agent_type and role/agent_role values".to_string(),
        ));
    }

    let agent_type = parsed_type
        .or(parsed_role_type)
        .unwrap_or(SubAgentType::General);

    if let Some(role) = role_input
        && normalize_role_alias(role).is_none()
    {
        return Err(ToolError::invalid_input(format!(
            "Invalid role alias '{role}'. Use: {VALID_ROLE_ALIASES}"
        )));
    }

    let role = role_input
        .and_then(normalize_role_alias)
        .or_else(|| type_input.and_then(normalize_role_alias))
        .map(str::to_string);

    let allowed_tools = input
        .get("allowed_tools")
        .and_then(|v| v.as_array())
        .map(|items| {
            let mut tools = Vec::new();
            for item in items {
                if let Some(tool) = item.as_str() {
                    let trimmed = tool.trim();
                    if !trimmed.is_empty() && !tools.iter().any(|existing| existing == trimmed) {
                        tools.push(trimmed.to_string());
                    }
                }
            }
            tools
        });

    let cwd = parse_optional_cwd(input)?;
    let worktree = parse_optional_worktree_request(input)?;
    if cwd.is_some() && worktree.is_some() {
        return Err(ToolError::invalid_input(
            "Use either cwd or worktree isolation, not both".to_string(),
        ));
    }
    let model = parse_optional_subagent_model(input, "model")?;
    let model_strength = optional_input_str(input, &["model_strength", "modelStrength"])
        .map(SubAgentModelStrength::parse)
        .transpose()?
        .unwrap_or_else(|| {
            // Default model strength. `type: "explore"` defaults to Faster for
            // bounded read-only lookup/search/status work — the cheap, fast
            // same-family sibling is exactly the lossy-breadth job a child
            // should run. Every other role (and any call that supplies an
            // explicit `model`) stays conservative at Same. Explicit
            // model_strength above already wins via .parse(); explicit `model`
            // wins downstream in assignment_model_route regardless of strength.
            if agent_type == SubAgentType::Explore && model.is_none() {
                SubAgentModelStrength::Faster
            } else {
                SubAgentModelStrength::Same
            }
        });
    let thinking = optional_input_str(input, &["thinking", "reasoning_effort", "reasoningEffort"])
        .map(SubAgentThinking::parse)
        .transpose()?
        .unwrap_or(SubAgentThinking::Inherit);
    let resident_file = input
        .get("resident_file")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty());
    let resume_checkpoint = parse_resume_checkpoint(input)?;
    let fork_context =
        parse_optional_bool(input, &["fork_context", "forkContext", "inherit_context"])
            .unwrap_or(false);
    let max_depth = input
        .get("max_depth")
        .or_else(|| input.get("maxDepth"))
        .or_else(|| input.get("max_spawn_depth"))
        .and_then(Value::as_u64)
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

    Ok(SpawnRequest {
        session_name,
        prompt: prompt.clone(),
        agent_type,
        assignment: SubAgentAssignment::new(prompt, role),
        allowed_tools,
        model,
        model_strength,
        thinking,
        cwd,
        worktree,
        resident_file,
        resume_checkpoint,
        fork_context,
        max_depth,
        token_budget,
    })
}

fn parse_resume_checkpoint(input: &Value) -> Result<Option<SubAgentCheckpoint>, ToolError> {
    let Some(value) = input
        .get("resume_from_checkpoint")
        .or_else(|| input.get("resumeFromCheckpoint"))
        .or_else(|| input.get("checkpoint"))
    else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    if value.is_string() {
        return Err(ToolError::invalid_input(
            "resume_from_checkpoint must be a checkpoint object from agent status/peek, not a continuation handle string".to_string(),
        ));
    }
    let checkpoint: SubAgentCheckpoint = serde_json::from_value(value.clone()).map_err(|err| {
        ToolError::invalid_input(format!(
            "resume_from_checkpoint must be a valid checkpoint object: {err}"
        ))
    })?;
    if !checkpoint.continuable {
        return Err(ToolError::invalid_input(
            "resume_from_checkpoint must be continuable".to_string(),
        ));
    }
    if checkpoint.messages.is_empty() {
        return Err(ToolError::invalid_input(
            "resume_from_checkpoint must include a non-empty checkpoint message tail".to_string(),
        ));
    }
    Ok(Some(checkpoint))
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

fn parse_optional_bool(input: &Value, names: &[&str]) -> Option<bool> {
    names
        .iter()
        .find_map(|name| input.get(*name))
        .and_then(Value::as_bool)
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
    crate::config::requested_model_for_provider(provider, trimmed).ok_or_else(|| {
        let valid_names = crate::config::model_completion_names_for_provider(provider);
        let valid_hint = if valid_names.is_empty() {
            String::new()
        } else {
            format!(" (accepted: {})", valid_names.join(", "))
        };
        ToolError::invalid_input(format!(
            "Invalid {field} '{trimmed}' for provider {}{valid_hint}",
            provider_name_for_error(provider)
        ))
    })
}

fn provider_name_for_error(provider: crate::config::ApiProvider) -> &'static str {
    match provider {
        crate::config::ApiProvider::Deepseek | crate::config::ApiProvider::DeepseekCN => "DeepSeek",
        crate::config::ApiProvider::Openai | crate::config::ApiProvider::OpenaiCodex => "OpenAI",
        crate::config::ApiProvider::Moonshot => "Moonshot",
        crate::config::ApiProvider::Ollama => "Ollama",
        _ => "this provider",
    }
}

pub(crate) fn configured_model_for_role_or_type(
    runtime: &SubAgentRuntime,
    role: Option<&str>,
    agent_type: &SubAgentType,
) -> Result<Option<String>, ToolError> {
    let mut keys = Vec::new();
    if let Some(role) = role.map(str::trim).filter(|role| !role.is_empty()) {
        keys.push(role.to_ascii_lowercase());
    }
    keys.push(agent_type.as_str().to_string());
    keys.push("default".to_string());

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
    agent_type: &SubAgentType,
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
        max_output_tokens: Some(SUBAGENT_RESPONSE_MAX_TOKENS),
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
        &SubAgentType::General,
    )
}

fn worker_profile_subagent_assignment_route(
    runtime: &SubAgentRuntime,
    model_route: &ModelRoute,
    requested_thinking: SubAgentThinking,
    prompt: &str,
    _agent_type: &SubAgentType,
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

    let reasoning_effort = subagent_reasoning_effort_for_request(
        runtime,
        prompt,
        requested_fast_lane,
        requested_thinking,
    );

    SubAgentResolvedRoute::new(model_route.clone(), model, reasoning_effort)
}

fn subagent_reasoning_effort_for_request(
    runtime: &SubAgentRuntime,
    prompt: &str,
    requested_fast_lane: bool,
    requested_thinking: SubAgentThinking,
) -> Option<String> {
    match requested_thinking {
        SubAgentThinking::Effort(effort) => Some(effort.as_setting().to_string()),
        SubAgentThinking::Auto => Some(
            auto_subagent_reasoning_effort(prompt)
                .as_setting()
                .to_string(),
        ),
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
            Some(effort.as_setting().to_string())
        }
        SubAgentThinking::Inherit => fallback_subagent_reasoning_effort(runtime, prompt),
    }
}

fn fallback_subagent_reasoning_effort(runtime: &SubAgentRuntime, prompt: &str) -> Option<String> {
    if runtime.reasoning_effort_auto {
        Some(
            auto_subagent_reasoning_effort(prompt)
                .as_setting()
                .to_string(),
        )
    } else {
        runtime.reasoning_effort.clone()
    }
}

fn auto_subagent_reasoning_effort(prompt: &str) -> ReasoningEffort {
    match crate::auto_reasoning::select(false, prompt) {
        ReasoningEffort::Low | ReasoningEffort::Medium => ReasoningEffort::High,
        other => other,
    }
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
        parse_optional_bool_strict(input, &["worktree", "isolate_worktree", "isolateWorktree"])?;
    let isolation = optional_input_str(input, &["isolation"])
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
    )
    .map(str::to_string);
    let path = optional_input_str(
        input,
        &[
            "worktree_path",
            "worktreePath",
            "worktree_dir",
            "worktreeDir",
        ],
    )
    .map(PathBuf::from);
    let base_ref = optional_input_str(
        input,
        &["worktree_base", "worktreeBase", "base_ref", "baseRef"],
    )
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

fn parse_optional_bool_strict(input: &Value, names: &[&str]) -> Result<Option<bool>, ToolError> {
    for name in names {
        let Some(value) = input.get(*name) else {
            continue;
        };
        return value.as_bool().map(Some).ok_or_else(|| {
            ToolError::invalid_input(format!("{name} must be a boolean when provided"))
        });
    }
    Ok(None)
}

fn prepare_child_workspace(
    parent_workspace: &Path,
    request: &SpawnRequest,
) -> Result<Option<PathBuf>, ToolError> {
    if let Some(requested_cwd) = request.cwd.as_ref() {
        return validate_existing_child_cwd(parent_workspace, requested_cwd).map(Some);
    }
    if let Some(worktree) = request.worktree.as_ref() {
        return create_isolated_worktree(
            parent_workspace,
            worktree,
            request.session_name.as_deref(),
            &request.agent_type,
        )
        .map(Some);
    }
    Ok(None)
}

fn validate_existing_child_cwd(
    parent_workspace: &Path,
    requested_cwd: &Path,
) -> Result<PathBuf, ToolError> {
    let resolved = if requested_cwd.is_absolute() {
        requested_cwd.to_path_buf()
    } else {
        parent_workspace.join(requested_cwd)
    };
    let canonical = resolved.canonicalize().map_err(|e| {
        ToolError::invalid_input(format!(
            "Invalid cwd '{}': {e} (path may not exist yet — use worktree=true to let CodeWhale create an isolated checkout)",
            requested_cwd.display()
        ))
    })?;
    let workspace_canonical = parent_workspace
        .canonicalize()
        .unwrap_or_else(|_| parent_workspace.to_path_buf());
    if !canonical.starts_with(&workspace_canonical) {
        return Err(ToolError::invalid_input(format!(
            "cwd must be inside the parent workspace: {} is not under {}",
            canonical.display(),
            workspace_canonical.display()
        )));
    }
    Ok(canonical)
}

fn create_isolated_worktree(
    parent_workspace: &Path,
    request: &SubAgentWorktreeRequest,
    session_name: Option<&str>,
    agent_type: &SubAgentType,
) -> Result<PathBuf, ToolError> {
    let repo_root = git_repo_root(parent_workspace)?;
    let branch = request
        .branch
        .clone()
        .unwrap_or_else(|| default_worktree_branch(session_name, agent_type));
    validate_git_branch_name(&repo_root, &branch)?;

    let base_ref = request
        .base_ref
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("HEAD")
        .to_string();
    let worktree_path = resolve_worktree_path(&repo_root, &branch, request.path.as_ref())?;
    if let Some(parent) = worktree_path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            ToolError::execution_failed(format!(
                "Failed to create worktree parent '{}': {err}",
                parent.display()
            ))
        })?;
    }

    let path_arg = worktree_path.to_string_lossy().to_string();
    let args = vec![
        "worktree".to_string(),
        "add".to_string(),
        "-b".to_string(),
        branch,
        path_arg,
        base_ref,
    ];
    run_git_checked(&repo_root, &args, "create sub-agent worktree")?;
    worktree_path.canonicalize().map_err(|err| {
        ToolError::execution_failed(format!(
            "Created worktree path '{}' could not be resolved: {err}",
            worktree_path.display()
        ))
    })
}

fn git_repo_root(workspace: &Path) -> Result<PathBuf, ToolError> {
    let output = run_git_checked(
        workspace,
        &["rev-parse".to_string(), "--show-toplevel".to_string()],
        "resolve git repository root",
    )?;
    let root = output.trim();
    if root.is_empty() {
        return Err(ToolError::invalid_input(
            "worktree=true requires a git repository workspace".to_string(),
        ));
    }
    Ok(PathBuf::from(root))
}

fn validate_git_branch_name(repo_root: &Path, branch: &str) -> Result<(), ToolError> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err(ToolError::invalid_input(
            "worktree_branch cannot be blank".to_string(),
        ));
    }
    run_git_checked(
        repo_root,
        &[
            "check-ref-format".to_string(),
            "--branch".to_string(),
            branch.to_string(),
        ],
        "validate sub-agent worktree branch",
    )
    .map(|_| ())
    .map_err(|err| ToolError::invalid_input(format!("Invalid worktree_branch '{branch}': {err}")))
}

fn default_worktree_branch(session_name: Option<&str>, agent_type: &SubAgentType) -> String {
    let seed = session_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| agent_type.as_str());
    format!(
        "codex/agent-{}-{}",
        sanitize_worktree_slug(seed),
        &Uuid::new_v4().to_string()[..8]
    )
}

fn resolve_worktree_path(
    repo_root: &Path,
    branch: &str,
    requested_path: Option<&PathBuf>,
) -> Result<PathBuf, ToolError> {
    let default_root = default_worktree_root(repo_root);
    let path = match requested_path {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => {
            let resolved = normalize_path_lexically(&default_root.join(path));
            if !resolved.starts_with(&default_root) {
                return Err(ToolError::invalid_input(format!(
                    "relative worktree_path '{}' must stay under {}",
                    path.display(),
                    default_root.display()
                )));
            }
            resolved
        }
        None => default_root.join(sanitize_worktree_slug(branch)),
    };
    let normalized = normalize_path_lexically(&path);
    let repo_canonical = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    if normalized.starts_with(&repo_canonical) {
        return Err(ToolError::invalid_input(format!(
            "worktree_path must not be inside the parent checkout: {} is under {}",
            normalized.display(),
            repo_canonical.display()
        )));
    }
    Ok(normalized)
}

fn default_worktree_root(repo_root: &Path) -> PathBuf {
    let repo_name = repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(sanitize_worktree_slug)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "repo".to_string());
    let parent = repo_root.parent().unwrap_or(repo_root);
    normalize_path_lexically(&parent.join(SUBAGENT_WORKTREE_ROOT_DIR).join(repo_name))
}

fn sanitize_worktree_slug(input: &str) -> String {
    let mut slug = String::new();
    for ch in input.chars() {
        let normalized = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else if matches!(ch, '-' | '_' | '.') {
            ch
        } else {
            '-'
        };
        if normalized == '-' && slug.ends_with('-') {
            continue;
        }
        slug.push(normalized);
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug.trim_matches(['-', '.', '_']).to_string();
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    }
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn run_git_checked(workspace: &Path, args: &[String], action: &str) -> Result<String, ToolError> {
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = Git::output(&arg_refs, workspace).map_err(|err| {
        ToolError::execution_failed(format!("Failed to {action}: could not run git: {err}"))
    })?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("git exited with status {}", output.status)
    };
    Err(ToolError::execution_failed(format!(
        "Failed to {action}: {detail}"
    )))
}

/// Resolve a user-supplied role/agent_role value to a canonical role string.
///
/// This must accept the full set that [`SubAgentType::from_str`] accepts, plus
/// role-only aliases (`worker`, `default`, `awaiter`). Before #2649 it covered
/// only a subset, so `role: "reviewer"` (accepted by `from_str`) was rejected
/// here by the second validation pass with a misleading four-value hint.
fn normalize_role_alias(input: &str) -> Option<&'static str> {
    match input.to_ascii_lowercase().as_str() {
        "default" => Some("default"),
        "worker" | "general" | "general-purpose" | "general_purpose" => Some("worker"),
        "explorer" | "explore" | "exploration" => Some("explorer"),
        "awaiter" | "plan" | "planner" | "planning" => Some("awaiter"),
        "reviewer" | "review" | "code-review" | "code_review" => Some("reviewer"),
        "implementer" | "implement" | "implementation" | "builder" => Some("implementer"),
        "verifier" | "verify" | "verification" | "validator" | "tester" => Some("verifier"),
        "custom" => Some("custom"),
        _ => None,
    }
}

fn build_assignment_prompt(
    prompt: &str,
    assignment: &SubAgentAssignment,
    agent_type: &SubAgentType,
) -> String {
    let role = assignment.role.as_deref().unwrap_or("default");
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

fn worker_progress_event_parts(message: &str) -> (AgentWorkerStatus, Option<u32>, Option<String>) {
    let step = parse_progress_step(message);
    let lower = message.to_ascii_lowercase();
    let status = if lower.contains("queued") {
        AgentWorkerStatus::Queued
    } else if lower.contains("waiting for user") || lower.contains("waiting for follow-up") {
        AgentWorkerStatus::WaitingForUser
    } else if lower.contains("requesting model response")
        || lower.contains(SUBAGENT_MODEL_WAIT_REASON)
    {
        AgentWorkerStatus::ModelWait
    } else if lower.contains("running tool") || lower.contains("executing") {
        AgentWorkerStatus::RunningTool
    } else if lower.contains("cancelled") {
        AgentWorkerStatus::Cancelled
    } else if lower.contains("interrupted") || lower.contains("timed out") {
        AgentWorkerStatus::Interrupted
    } else if lower.contains("complete") {
        AgentWorkerStatus::Completed
    } else if lower.contains("started") {
        AgentWorkerStatus::Starting
    } else {
        AgentWorkerStatus::Running
    };
    (status, step, parse_progress_tool_name(message))
}

fn parse_progress_step(message: &str) -> Option<u32> {
    let rest = message.strip_prefix("step ")?;
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    (!digits.is_empty())
        .then(|| digits.parse::<u32>().ok())
        .flatten()
}

fn parse_progress_tool_name(message: &str) -> Option<String> {
    let marker = "tool '";
    let start = message.find(marker)? + marker.len();
    let rest = &message[start..];
    let end = rest.find('\'')?;
    let tool = rest[..end].trim();
    (!tool.is_empty()).then(|| tool.to_string())
}

fn subagent_progress_tool_display_name(name: &str) -> &str {
    match name {
        "exec_shell"
        | "exec_shell_wait"
        | "exec_shell_interact"
        | "exec_wait"
        | "exec_interact"
        | "task_shell_start"
        | "task_shell_wait" => "Bash",
        _ => name,
    }
}

fn emit_agent_progress(
    event_tx: Option<&mpsc::Sender<Event>>,
    agent_id: &str,
    status: String,
    parent_run_id: Option<String>,
    spawn_depth: u32,
) {
    if let Some(event_tx) = event_tx {
        if event_tx.max_capacity() > MIN_EVENT_CHANNEL_HEADROOM_FOR_ROUTINE_PROGRESS
            && event_tx.capacity() <= MIN_EVENT_CHANNEL_HEADROOM_FOR_ROUTINE_PROGRESS
            && routine_agent_progress_can_preserve_event_headroom(&status)
        {
            return;
        }
        let _ = event_tx.try_send(Event::AgentProgress {
            id: agent_id.to_string(),
            status,
            parent_run_id,
            spawn_depth,
        });
    }
}

fn routine_agent_progress_can_preserve_event_headroom(status: &str) -> bool {
    matches!(
        worker_progress_event_parts(status).0,
        AgentWorkerStatus::Running | AgentWorkerStatus::ModelWait | AgentWorkerStatus::RunningTool
    )
}

// === Tool Registry Helpers ===

/// Per-sub-agent tool registry.
///
/// Two modes:
/// - **Full inheritance** (`allowed_tools = None`): the child sees the same
///   tool surface as the parent's Agent mode, except legacy sub-agent lifecycle
///   tools are removed. The single `agent` launcher remains visible only while
///   the configured depth budget allows another child. Approval-gated tools are
///   callable only when the parent runtime is auto-approved or, for explicit
///   write-capable roles (`implementer`, `custom`), when the tool's approval
///   requirement is `Suggest`.
/// - **Explicit narrow** (`allowed_tools = Some(list)`): legacy / Custom
///   path. The registry still builds the full surface, but only the listed
///   tool names are visible to the model and callable.
///
/// Pure per-role posture check (#3217), independent of any runtime: whether a
/// role may invoke a tool of the given approval level.
///
/// - Read (`Auto`) tools are always allowed.
/// - Write/edit/patch (`Suggest`) tools require a write-capable posture, so the
///   read-only roles (`explore`/`review`/`plan`/`verifier`) are denied.
/// - Shell (`Required`) tools require a `Full` shell posture, so only
///   `verifier`/`implementer`/`general` may shell out; `explore`/`review`
///   (read-only shell) and `plan` (no shell) are denied because read-only-shell
///   enforcement is not yet wired at the exec layer.
///
/// `custom` is governed by its explicit `allowed_tools` list, so the posture
/// check permits it here (the allowlist is the authority for that role).
fn role_posture_permits(agent_type: &SubAgentType, approval: ApprovalRequirement) -> bool {
    if matches!(agent_type, SubAgentType::Custom) {
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

struct SubAgentToolRegistry {
    /// `None` → full inheritance (no allowlist filter applied). `Some(list)` →
    /// only the listed tools are visible to the model and callable.
    allowed_tools: Option<Vec<String>>,
    auto_approve: bool,
    /// The role/type of the sub-agent that this registry belongs to. Used to
    /// decide whether `Suggest`-level tools (write/edit/patch) may run inside
    /// the child without the parent runtime being auto-approved (#1828, #1833).
    agent_type: SubAgentType,
    /// Already-derived capability envelope for this child. This captures the
    /// parent posture intersection, so a Plan parent can expose delegation
    /// without accidentally granting write or shell tools to the child.
    runtime_profile: WorkerRuntimeProfile,
    can_spawn_child: bool,
    owner_agent_id: String,
    owner_agent_name: String,
    registry: ToolRegistry,
}

impl SubAgentToolRegistry {
    #[cfg(test)]
    fn new(
        runtime: SubAgentRuntime,
        agent_type: SubAgentType,
        explicit_allowed_tools: Option<Vec<String>>,
        todo_list: SharedTodoList,
        plan_state: SharedPlanState,
    ) -> Self {
        Self::new_with_owner(
            runtime,
            agent_type,
            "agent_unknown".to_string(),
            "sub-agent".to_string(),
            explicit_allowed_tools,
            todo_list,
            plan_state,
        )
    }

    fn new_with_owner(
        runtime: SubAgentRuntime,
        agent_type: SubAgentType,
        owner_agent_id: String,
        owner_agent_name: String,
        explicit_allowed_tools: Option<Vec<String>>,
        todo_list: SharedTodoList,
        plan_state: SharedPlanState,
    ) -> Self {
        // Build the full agent surface — same as the parent's Agent mode.
        // Children inherit shell, file, patch, search, web, git, diagnostics,
        // review, and RLM, plus per-child fresh todo/plan state. `agent` is
        // retained only when depth budget remains.
        let can_spawn_child = !runtime.would_exceed_depth();
        let context = runtime.context.clone();
        let mut surface_options = runtime.agent_tool_surface_options.clone();
        surface_options.shell_policy = ShellPolicy::from_legacy_allow_shell(runtime.allow_shell);
        let mut registry = ToolRegistryBuilder::new().with_full_agent_surface_options(
            Some(runtime.client.clone()),
            runtime.model.clone(),
            runtime.manager.clone(),
            runtime.clone(),
            surface_options,
            todo_list,
            plan_state,
        );

        if let Some(pool) = runtime.mcp_pool.as_ref() {
            registry = registry.with_mcp_tools(std::sync::Arc::clone(pool));
        }

        let registry = registry.build(context);

        Self {
            allowed_tools: explicit_allowed_tools,
            auto_approve: runtime.context.auto_approve,
            agent_type,
            runtime_profile: runtime.worker_profile,
            can_spawn_child,
            owner_agent_id,
            owner_agent_name,
            registry,
        }
    }

    /// Whether this role is allowed to use `Suggest`-level tools (write_file,
    /// edit_file, apply_patch, ...) without the parent runtime being
    /// auto-approved. Read-only stances (`explore`, `plan`, `review`,
    /// `verifier`) stay blocked so they can't quietly mutate the workspace
    /// while a non-auto parent is delegating bounded investigation.
    /// `Required`-level tools (shell, etc.) still need parent auto-approve
    /// regardless of role (#1828, #1833).
    fn role_can_delegate_writes(agent_type: &SubAgentType) -> bool {
        matches!(agent_type, SubAgentType::Implementer | SubAgentType::Custom)
    }

    /// Whether the role posture permits a given registered tool, independent of
    /// parent auto-approval. Delegates to the pure `role_posture_permits`.
    /// Unregistered names pass through (the allowlist / availability checks
    /// handle those separately).
    fn posture_permits_tool(&self, name: &str) -> bool {
        // Delegation (`agent`) is governed by the depth budget and the
        // allowlist (`can_spawn_child` / `is_tool_allowed`), not the write/shell
        // posture — a read-only role may still fan out child work.
        if name == "agent" {
            return true;
        }
        match self.registry.get(name) {
            Some(spec) => match spec.approval_requirement() {
                ApprovalRequirement::Auto => true,
                ApprovalRequirement::Suggest => {
                    self.runtime_profile.permissions.write
                        && role_posture_permits(&self.agent_type, ApprovalRequirement::Suggest)
                }
                ApprovalRequirement::Required => {
                    matches!(self.runtime_profile.shell, ShellPolicy::Full)
                        && role_posture_permits(&self.agent_type, ApprovalRequirement::Required)
                }
            },
            None => true,
        }
    }

    /// Whether a given tool name is permitted under this child's filter.
    /// `None` filter = everything permitted.
    fn is_tool_allowed(&self, name: &str) -> bool {
        if name == "agent" && !self.can_spawn_child {
            return false;
        }
        match &self.allowed_tools {
            None => true,
            Some(list) => list.iter().any(|t| t == name),
        }
    }

    fn tools_for_model(&self, agent_type: &SubAgentType) -> Vec<Tool> {
        let _ = agent_type;
        let api_tools = self.registry.to_api_tools();
        let filtered = match &self.allowed_tools {
            None => api_tools,
            Some(list) => api_tools
                .into_iter()
                .filter(|tool| list.contains(&tool.name))
                .collect::<Vec<_>>(),
        };
        filtered
            .into_iter()
            .filter(|tool| tool.name != "agent" || self.can_spawn_child)
            // #3217: hide tools the role posture forbids so the model never
            // even sees write/edit/patch (read-only roles) or shell (no-shell
            // roles). Defense-in-depth with the `execute` guard below.
            .filter(|tool| self.posture_permits_tool(&tool.name))
            .collect()
    }

    fn unavailable_allowed_tools(&self) -> Vec<String> {
        match &self.allowed_tools {
            None => Vec::new(),
            Some(list) => list
                .iter()
                .filter(|name| !self.registry.contains(name))
                .cloned()
                .collect(),
        }
    }

    async fn execute(&self, _agent_id: &str, name: &str, input: Value) -> Result<String> {
        if !self.is_tool_allowed(name) {
            return Err(anyhow!("Tool {name} not allowed for this sub-agent"));
        }
        // #3217: authoritative per-role posture — read-only roles cannot mutate
        // and non-`Full`-shell roles cannot run shell, regardless of whether
        // the parent session is auto-approved. This closes the auto-approve
        // bypass where a read-only child could quietly write or shell out.
        if !self.posture_permits_tool(name) {
            return Err(anyhow!(
                "Tool {name} is not permitted for the read-only `{role}` sub-agent role. Use an `implementer` or `general` role (or a `custom` role with an explicit allowed_tools list) to mutate the workspace or run shell commands.",
                role = self.agent_type.as_str()
            ));
        }
        if !self.auto_approve {
            let Some(spec) = self.registry.get(name) else {
                return Err(anyhow!("Tool {name} is not registered"));
            };
            match spec.approval_requirement() {
                ApprovalRequirement::Auto => {}
                ApprovalRequirement::Suggest => {
                    // Write/edit/patch tools land here. Explicit
                    // write-capable roles (`implementer`, `custom`) may run them
                    // without parent auto-approve so that delegated work
                    // can actually land file changes; the previous
                    // behavior blocked every write under `suggest` mode
                    // even for the role explicitly chartered to write
                    // (#1828, #1833). Read-only roles still bounce so
                    // exploration/review/planning/verifier children
                    // can't mutate the workspace behind the parent's back.
                    if !self.runtime_profile.permissions.write
                        || !Self::role_can_delegate_writes(&self.agent_type)
                    {
                        return Err(anyhow!(
                            "Tool {name} requires approval and is not delegated to {role} sub-agents; rerun the parent with auto approval or pick a write-capable role",
                            role = self.agent_type.as_str()
                        ));
                    }
                }
                ApprovalRequirement::Required => {
                    return Err(anyhow!(
                        "Tool {name} requires approval and cannot run inside this sub-agent unless the parent session is auto-approved"
                    ));
                }
            }
        }
        reject_subagent_terminal_takeover(name, &input)?;
        let context = self
            .registry
            .context()
            .clone()
            .with_owner_agent(self.owner_agent_id.clone(), self.owner_agent_name.clone());
        self.registry
            .execute_full_with_context(name, input, Some(&context))
            .await
            .map(|result| result.content)
            .map_err(|e| anyhow!(e))
    }
}

fn reject_subagent_terminal_takeover(name: &str, input: &Value) -> Result<()> {
    let wants_interactive_shell = name == "exec_shell"
        && input
            .get("interactive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    if wants_interactive_shell {
        return Err(anyhow!(
            "Sub-agents run in the background and cannot use exec_shell with interactive=true \
             because that would take over the parent TUI terminal. Use non-interactive \
             exec_shell, background=true, tty=true, or task_shell_start instead."
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
    agent_type: &SubAgentType,
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
        if matches!(agent_type, SubAgentType::Custom) && deduped.is_empty() {
            return Err(anyhow!(
                "Custom sub-agent requires a non-empty allowed_tools list"
            ));
        }
        return Ok(Some(deduped));
    }

    if matches!(agent_type, SubAgentType::Custom) {
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

/// When a child agent fails because its model is unavailable under the current
/// access profile, a bare provider 403/404 (classified `Authorization` or
/// `State`) is unactionable. Annotate it so the parent knows the likely cause
/// and how to recover (#2653) without re-classifying the underlying error.
fn annotate_child_model_error(err: &str, model: &str) -> String {
    match crate::error_taxonomy::classify_error_message(err) {
        crate::error_taxonomy::ErrorCategory::Authorization
        | crate::error_taxonomy::ErrorCategory::State => format!(
            "{err}\n(child model `{model}` may be unavailable under the current access profile — \
             remove the explicit child model override or adjust child-agent model config before retrying)"
        ),
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
                format!(
                    "{err}\n(child model `{model}` may be unavailable under the current access profile — \
                     remove the explicit child model override or adjust child-agent model config before retrying)"
                )
            } else {
                err.to_string()
            }
        }
    }
}

/// Char budget above which a sub-agent summary is treated as a large dump and
/// head+tail truncated. Mirrors `TOOL_RESULT_SENT_CHAR_BUDGET` in
/// `crates/tui/src/client/chat.rs:702` so sub-agent summaries use the same
/// threshold as regular tool outputs. Duplicated locally to avoid coupling the
/// sub-agent module to the wire-compaction internals.
const SUBAGENT_SUMMARY_CHAR_BUDGET: usize = 12_000;
/// Head/tail slice sizes when truncating; mirror the wire constants
/// (`TOOL_RESULT_HEAD_CHARS`/`TOOL_RESULT_TAIL_CHARS`, chat.rs:703-704).
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
///   truncation), adapted to be honest that the elided middle is NOT in the
///   spillover store — there is no `retrieve_tool_result` handle for
///   sub-agent summaries. Report `truncated: true`.
///
/// Every summary therefore gets exactly one boundary marker, never both.
fn stamp_subagent_summary(raw: &str) -> (String, bool) {
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
    let stamped = format!(
        "{head}\n\n[Sub-agent summary truncated: {SUBAGENT_SUMMARY_HEAD_CHARS} + {SUBAGENT_SUMMARY_TAIL_CHARS} of {total} \
chars shown. This is the child's self-report; the elided middle ({omitted} chars) is not in \
the spillover store and cannot be retrieved via retrieve_tool_result. Re-open the child or \
read changed files directly to verify material claims.]\n\n{tail}",
    );
    (stamped, true)
}

fn summarize_subagent_result(result: &SubAgentResult) -> String {
    if let Some(needs_input) = result.needs_input.as_ref() {
        return format!("Needs input: {}", needs_input.question);
    }
    match (&result.status, result.result.as_ref()) {
        (SubAgentStatus::Completed, Some(text)) => text.clone(),
        (SubAgentStatus::Completed, None) => "Completed (no output)".to_string(),
        (SubAgentStatus::Interrupted(error), _) => format!("Interrupted: {error}"),
        (SubAgentStatus::Cancelled, _) => "Cancelled".to_string(),
        (SubAgentStatus::BudgetExhausted, _) => "Token budget exhausted".to_string(),
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

const SUBAGENT_OUTPUT_FORMAT: &str = include_str!("../../prompts/subagent_output_format.md");

const GENERAL_AGENT_INTRO: &str = concat!(
    "You are a trusted general-purpose sub-agent. Your job is to complete the one task you were given, end-to-end, and report back concisely.\n",
    "Stay inside the assigned scope; put adjacent work under RISKS/BLOCKERS.\n",
    "For genuinely multi-step work, track progress with `checklist_write` (and `update_plan` for complex strategy); skip it for short, focused tasks.\n",
    "**Stop quickly on failure**: if the same tool call fails 2 times in a row, stop retrying and return what you have so far with a one-line note explaining what's missing. Do not loop on impossible queries (e.g. external API unreachable, rate-limited, or returning empty).\n",
    "For implementer or repair-style work, keep going within the assigned scope; checkpoint before broadening the task or after repeated failures instead of forcing a tiny tool-call cap.\n\n"
);

const EXPLORE_AGENT_INTRO: &str = concat!(
    "You are a trusted exploration sub-agent (role: `explore`). Your job is to map the relevant code quickly and stay strictly read-only.\n",
    "Default to `EFFORT: quick`: aim for about 3-5 tool calls unless the brief explicitly asks for more.\n",
    "Orient first: confirm the workspace/project root, read relevant AGENTS.md/README guidance when the tree is unfamiliar, then search only the likely scope.\n",
    "Use list_dir/file_search, grep_files, and read_file; use RLM only for long inputs or many semantic slices, not basic path discovery.\n",
    "Honor QUESTION, SCOPE, ALREADY_KNOWN, and STOP_CONDITION. Do not repeat ALREADY_KNOWN work unless evidence contradicts it; do not broaden once QUESTION is answered.\n",
    "DeepSeek V4 can hold broad evidence, but your value is compressed reconnaissance: cite `path:line-range` for each finding and stop once evidence is sufficient. Return partial findings if the next step would be speculative or duplicative.\n",
    "CHANGES will almost always be \"None.\" for an explorer.\n\n"
);

const PLAN_AGENT_INTRO: &str = concat!(
    "You are a trusted planning sub-agent (role: `plan`). Your job is to produce a grounded, prioritized plan, not patches.\n",
    "Read enough code to avoid guessing; each step names its artifact and verification.\n",
    "Use update_plan/checklist_write for plan artifacts and explain key trade-offs.\n",
    "CHANGES should list plan artifacts only, not future speculative edits.\n\n"
);

const REVIEW_AGENT_INTRO: &str = concat!(
    "You are a trusted code review sub-agent (role: `review`). Your job is to find and report severity-scored issues, and stay strictly read-only.\n",
    "Read the diff/files, grep sibling patterns/tests, then order EVIDENCE by severity.\n",
    "Use BLOCKER/MAJOR/MINOR/NIT and include path:line-range plus suggested fix.\n",
    "You may use more tool calls than quick exploration, but stop after decisive evidence instead of widening the review forever.\n",
    "If no MAJOR+ issues exist, say so plainly in SUMMARY.\n",
    "CHANGES will almost always be \"None.\" for a reviewer.\n\n"
);

const CUSTOM_AGENT_INTRO: &str = concat!(
    "You are a trusted custom sub-agent (role: `custom`) with a narrowed tool registry. Your job is to stay tightly scoped to the assigned objective.\n",
    "Use only tools available at runtime; put missing capabilities under BLOCKERS and stop.\n\n"
);

const IMPLEMENTER_AGENT_INTRO: &str = concat!(
    "You are a trusted implementation sub-agent (role: `implementer`). Your job is to land the assigned change with minimal surrounding edits.\n",
    "Read target files before editing; prefer edit_file for narrow changes and apply_patch for hunks.\n",
    "Run relevant verification after edit batches; write needed tests with the implementation.\n",
    "You are not limited to an explorer-style 3-5 tool-call cap. Checkpoint before expanding scope or after repeated failures, then continue only inside the assigned brief.\n",
    "CHANGES is load-bearing: list every modified file with a one-line why.\n\n"
);

const VERIFIER_AGENT_INTRO: &str = concat!(
    "You are a trusted verification sub-agent (role: `verifier`). Your job is to run the requested gates and report results, and stay read-only.\n",
    "Report PASS/FAIL/FLAKY at the top of SUMMARY with exact command evidence.\n",
    "Capture failing assertion and file:line; put obvious fixes under RISKS.\n",
    "You may use more tool calls than quick exploration, but stop after decisive pass/fail evidence.\n",
    "CHANGES will almost always be \"None.\" for a verifier.\n\n"
);

// === Tests ===

#[cfg(test)]
mod tests;
