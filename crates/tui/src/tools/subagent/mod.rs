//! Sub-agent spawning system.
//!
//! Provides tools to spawn background sub-agents, query their status,
//! and retrieve results. Sub-agents run with a filtered toolset and
//! inherit the workspace configuration from the main session.
//!
//! v0.8.33's new model-facing surface is `agent_open` / `agent_eval` /
//! `agent_close`. Some older structs and manager helpers remain in this
//! module while the durable runtime is being reused by the new surface.
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock};

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
use crate::llm_client::LlmClient;
use crate::models::{ContentBlock, Message, MessageRequest, MessageResponse, SystemPrompt, Tool};
use crate::tools::handle::VarHandle;
use crate::tools::plan::{PlanState, SharedPlanState};
use crate::tools::registry::{ToolRegistry, ToolRegistryBuilder};
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_bool, optional_u64, required_str,
};
use crate::tools::todo::{SharedTodoList, TodoList};
use crate::utils::spawn_supervised;

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
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);
// Non-streaming sub-agents need enough response budget to carry large tool-call
// arguments, especially write_file content. The API bills generated tokens, not
// the requested ceiling.
const SUBAGENT_RESPONSE_MAX_TOKENS: u32 = 16_384;
const MAX_CONSECUTIVE_TRUNCATED_SUBAGENT_RESPONSES: u32 = 5;
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
const RESULT_POLL_INTERVAL: Duration = Duration::from_millis(250);
const DEFAULT_RESULT_TIMEOUT_MS: u64 = 30_000;
#[allow(dead_code)] // Legacy agent_wait clamp; new agent_eval uses DEFAULT/MAX.
const MIN_WAIT_TIMEOUT_MS: u64 = 10_000;
const MAX_RESULT_TIMEOUT_MS: u64 = 3_600_000;
const COMPLETED_AGENT_RETENTION: Duration = Duration::from_secs(60 * 60);
const SUBAGENT_STATE_SCHEMA_VERSION: u32 = 1;
const SUBAGENT_STATE_FILE: &str = "subagents.v1.json";
const SUBAGENT_RESTART_REASON: &str = "Interrupted by process restart";

const VALID_SUBAGENT_TYPES: &str = "general (aliases: general-purpose, general_purpose, worker, default), \
     explore (aliases: exploration, explorer), plan (aliases: planning, planner, awaiter), \
     review (aliases: code-review, code_review, reviewer), implementer (aliases: implement, implementation, builder), \
     verifier (aliases: verify, verification, validator, tester), tool_agent (aliases: tool-agent, toolagent, executor, execution, fin), custom";
/// Role aliases accepted by `normalize_role_alias`. Kept in sync with the
/// match arms below so every input that `SubAgentType::from_str` accepts also
/// resolves to a canonical role (avoids the dual-validation rejection in #2649).
const VALID_ROLE_ALIASES: &str = "default; worker (aliases: general, general-purpose, general_purpose); \
     explorer (aliases: explore, exploration); awaiter (aliases: plan, planning, planner); \
     reviewer (aliases: review, code-review, code_review); implementer (aliases: implement, implementation, builder); \
     verifier (aliases: verify, verification, validator, tester); tool_agent (aliases: tool-agent, toolagent, executor, execution, fin); custom";
const SUBAGENT_TYPE_DESCRIPTION: &str = "Sub-agent type. Accepted vocabulary: general (aliases: general-purpose, general_purpose, worker, default), \
     explore (aliases: exploration, explorer), plan (aliases: planning, planner, awaiter), \
     review (aliases: code-review, code_review, reviewer), implementer (aliases: implement, implementation, builder), \
     verifier (aliases: verify, verification, validator, tester), tool_agent (aliases: tool-agent, toolagent, executor, execution, fin), custom.";
const SUBAGENT_ROLE_DESCRIPTION: &str = "Role alias. Accepted vocabulary: default; worker (aliases: general, general-purpose, general_purpose); \
     explorer (aliases: explore, exploration); awaiter (aliases: plan, planning, planner); \
     reviewer (aliases: review, code-review, code_review); implementer (aliases: implement, implementation, builder); \
     verifier (aliases: verify, verification, validator, tester); tool_agent (aliases: tool-agent, toolagent, executor, execution, fin); custom. \
     Must match `type` if both are given.";
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

/// Removal version for deprecated tool aliases.
const DEPRECATION_REMOVAL_VERSION: &str = "0.8.0";

#[must_use]
pub fn whale_nickname_for_index(index: usize) -> String {
    let base = WHALE_NICKNAMES[index % WHALE_NICKNAMES.len()];
    if index < WHALE_NICKNAMES.len() {
        base.to_string()
    } else {
        format!("{base} {}", index / WHALE_NICKNAMES.len() + 1)
    }
}

// === Deprecation helpers ===

/// Wrap a `ToolResult` with a `_deprecation` block in its metadata.
///
/// Applied exclusively on alias paths (not on canonical tool names) so the
/// model can detect and migrate away from the old name before removal in
/// v`DEPRECATION_REMOVAL_VERSION`.
///
/// The `_deprecation` key is merged into any existing metadata so other
/// metadata (e.g. `status`, `timed_out`) is preserved unchanged.
fn wrap_with_deprecation_notice(
    mut result: ToolResult,
    this_tool: &str,
    use_instead: &str,
) -> ToolResult {
    tracing::warn!(
        "Deprecated tool '{}' invoked — use '{}' instead (removal: v{})",
        this_tool,
        use_instead,
        DEPRECATION_REMOVAL_VERSION,
    );

    let notice = json!({
        "_deprecation": {
            "this_tool": this_tool,
            "use_instead": use_instead,
            "removed_in": DEPRECATION_REMOVAL_VERSION,
            "message": format!(
                "Tool '{}' is deprecated; switch to '{}' before v{}.",
                this_tool, use_instead, DEPRECATION_REMOVAL_VERSION
            )
        }
    });

    result.metadata = Some(match result.metadata.take() {
        Some(Value::Object(mut map)) => {
            if let Value::Object(notice_map) = notice {
                map.extend(notice_map);
            }
            Value::Object(map)
        }
        Some(other) => {
            // Existing metadata was not an object — keep it as-is and add
            // the deprecation notice as a sibling under a wrapper.
            json!({ "_deprecation": notice["_deprecation"].clone(), "_original_metadata": other })
        }
        None => notice,
    });

    result
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
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
    /// Tool execution — a fast, non-thinking Flash V4 executor for simple
    /// machine-bound tasks. Intended as the experimental "Fin" lane: the
    /// parent does planning/synthesis while this child runs tools and reports
    /// compact facts.
    ToolAgent,
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
            "tool-agent" | "tool_agent" | "toolagent" | "executor" | "execution" | "fin" => {
                Some(Self::ToolAgent)
            }
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
            Self::ToolAgent => "tool_agent",
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
            Self::ToolAgent => TOOL_AGENT_INTRO,
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
            Self::ToolAgent => vec![
                "list_dir",
                "read_file",
                "grep_files",
                "file_search",
                "image_ocr",
                "fetch_url",
                "web_search",
                "web.run",
                "exec_shell",
                "exec_shell_wait",
                "exec_shell_interact",
                "exec_wait",
                "exec_interact",
                "handle_read",
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
}

/// Snapshot of sub-agent state for tool results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentResult {
    pub name: String,
    pub agent_id: String,
    pub context_mode: String,
    pub fork_context: bool,
    pub agent_type: SubAgentType,
    pub assignment: SubAgentAssignment,
    #[serde(default)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    pub status: SubAgentStatus,
    pub result: Option<String>,
    pub steps_taken: u32,
    pub duration_ms: u64,
    /// `true` when this agent was loaded from a prior-session persisted
    /// state file rather than spawned in the current session (#405).
    /// Lets `agent_list` filter out historical noise by default while
    /// keeping the records reachable via `include_archived=true`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub from_prior_session: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SubAgentSpawnOptions {
    pub name: Option<String>,
    pub model: Option<String>,
    pub nickname: Option<String>,
    pub fork_context: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitMode {
    Any,
    All,
}

impl WaitMode {
    fn from_str(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "any" | "first" => Some(Self::Any),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    #[allow(dead_code)] // Legacy wait metadata while registry moves to agent_eval.
    fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::All => "all",
        }
    }

    fn condition_met(self, snapshots: &[SubAgentResult]) -> bool {
        match self {
            Self::Any => snapshots
                .iter()
                .any(|snapshot| snapshot.status != SubAgentStatus::Running),
            Self::All => snapshots
                .iter()
                .all(|snapshot| snapshot.status != SubAgentStatus::Running),
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
    /// Optional working directory for the child. Must canonicalize to a
    /// path inside the parent's workspace. Used to dispatch parallel work
    /// into separate git worktrees: parent runs `git worktree add` first,
    /// then spawns children with the worktree path as `cwd`.
    cwd: Option<PathBuf>,
    /// Optional file path for cache-aware resident mode (#529). When set,
    /// the child's prompt is prefixed with the file contents for prefix-cache
    /// locality. A global ownership table prevents two agents from holding
    /// a resident lease on the same file simultaneously.
    resident_file: Option<String>,
    /// When true, seed the child with the parent's system prompt and message
    /// prefix before appending the child task.
    fork_context: bool,
    /// Optional recursion budget for descendants opened by this child.
    /// `0` means the child may not call `agent_open` recursively.
    max_depth: Option<u32>,
}

#[derive(Debug, Clone)]
struct AssignRequest {
    agent_id: String,
    objective: Option<String>,
    role: Option<String>,
    message: Option<String>,
    interrupt: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSubAgent {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_name: Option<String>,
    #[serde(default)]
    fork_context: bool,
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
}

impl Default for PersistedSubAgentState {
    fn default() -> Self {
        Self {
            schema_version: SUBAGENT_STATE_SCHEMA_VERSION,
            agents: Vec::new(),
        }
    }
}

/// Default cap on sub-agent recursion depth. Override via
/// `[runtime] max_spawn_depth = N` in `~/.deepseek/config.toml`.
pub const DEFAULT_MAX_SPAWN_DEPTH: u32 = 3;

/// Terminal-state notification emitted to the engine's parent turn loop
/// when one of its direct children finishes (issue #756). Carries the
/// already-rendered `<codewhale:subagent.done>` sentinel that the model
/// expects in the transcript per `prompts/base.md`.
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
/// `agent_open` sessions from the caller's turn token.
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
    pub event_tx: Option<mpsc::Sender<Event>>,
    /// Manager handle so children can recurse via `agent_spawn`. All agents
    /// at every depth share the same manager.
    pub manager: SharedSubAgentManager,
    /// Depth in the spawn tree. 0 = top-level user turn; 1 = direct child;
    /// etc. Children clone the parent runtime and increment this on spawn.
    pub spawn_depth: u32,
    /// Hard cap on recursion depth. A child whose `spawn_depth + 1` would
    /// exceed this is rejected at the spawn entry. Use `>` (strictly
    /// greater than) so equality is allowed — matches codex's pattern.
    pub max_spawn_depth: u32,
    /// Cooperative cancellation token. Direct `child_runtime()` callers derive
    /// a child token from the parent; model-visible `agent_open` uses
    /// `background_runtime()` to replace that token with a detached one.
    pub cancel_token: CancellationToken,
    /// Structured progress / lifecycle stream. Cloned across children so the
    /// whole spawn tree publishes into one ordered, fan-out-able mailbox.
    /// `None` only when no consumer is wired (legacy entry points / tests).
    pub mailbox: Option<Mailbox>,
    /// Wakeup channel for the engine's parent turn loop (issue #756). Only
    /// the engine's direct children fire on this — propagated to descendants
    /// via clone but gated to `spawn_depth == 1` at the send site so the
    /// parent isn't flooded with grandchild completions it didn't directly
    /// orchestrate. `None` when no consumer is wired (tests / legacy paths).
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
    /// Default directory for Xiaomi MiMo speech/TTS tool outputs inherited by
    /// child registries. Keeps parent and sub-agent `speech` / `tts` tools on
    /// the same `[speech].output_dir` / env override.
    pub speech_output_dir: Option<PathBuf>,
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
            event_tx,
            manager,
            spawn_depth: 0,
            max_spawn_depth: DEFAULT_MAX_SPAWN_DEPTH,
            cancel_token: CancellationToken::new(),
            mailbox: None,
            parent_completion_tx: None,
            fork_context: None,
            mcp_pool: None,
            step_api_timeout: DEFAULT_STEP_API_TIMEOUT,
            speech_output_dir: None,
        }
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
        self.speech_output_dir = output_dir;
        self
    }

    /// Attach the wakeup channel so the engine's parent turn loop can resume
    /// when this runtime's direct children finish (issue #756). The channel
    /// is propagated to descendants via clone, but only `spawn_depth == 1`
    /// agents fire on it — see `run_subagent_task`.
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
    /// Used by config wiring (`[runtime] max_spawn_depth = N`) and tests.
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

    /// Preserve the parent's thinking configuration. `reasoning_effort_auto`
    /// stays true even when the parent turn itself was sent with a concrete
    /// flash-router recommendation, so children can resolve their own tier.
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
            event_tx: self.event_tx.clone(),
            manager: self.manager.clone(),
            spawn_depth: self.spawn_depth + 1,
            max_spawn_depth: self.max_spawn_depth,
            cancel_token: self.cancel_token.child_token(),
            mailbox: self.mailbox.clone(),
            parent_completion_tx: self.parent_completion_tx.clone(),
            fork_context: self.fork_context.clone(),
            mcp_pool: self.mcp_pool.clone(),
            step_api_timeout: self.step_api_timeout,
            speech_output_dir: self.speech_output_dir.clone(),
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
            started_at,
            last_activity_at: started_at,
            allowed_tools,
            session_boot_id,
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
            agent_type: self.agent_type.clone(),
            assignment: self.assignment.clone(),
            model: self.model.clone(),
            nickname: self.nickname.clone(),
            status: self.status.clone(),
            result: self.result.clone(),
            steps_taken: self.steps_taken,
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
    #[allow(dead_code)] // Stored for future workspace-scoped operations
    workspace: PathBuf,
    state_path: Option<PathBuf>,
    max_steps: u32,
    max_agents: usize,
    running_heartbeat_timeout: Duration,
    /// Stable id assigned at manager construction (#405). Stamped on
    /// every agent the manager spawns; agents loaded from the
    /// persisted state file carry whatever id the prior session
    /// stamped (or empty for pre-#405 records). The manager classifies
    /// agents whose `session_boot_id` doesn't match this value as
    /// "from prior session" so `agent_list` can hide them by default.
    current_session_boot_id: String,
}

impl SubAgentManager {
    /// Create a new manager for sub-agents.
    #[must_use]
    pub fn new(workspace: PathBuf, max_agents: usize) -> Self {
        Self {
            agents: HashMap::new(),
            workspace,
            state_path: None,
            max_steps: DEFAULT_MAX_STEPS,
            max_agents,
            running_heartbeat_timeout: Duration::from_secs(
                crate::config::DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS,
            ),
            // Fresh boot id per manager. Used by #405 to classify
            // re-loaded persisted agents as "prior session".
            current_session_boot_id: format!("boot_{}", &Uuid::new_v4().to_string()[..12]),
        }
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
    /// `agent_list` output by default (#405).
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

    fn persist_state(&self) -> Result<()> {
        let Some(path) = self.state_path.as_ref() else {
            return Ok(());
        };
        let now_ms = epoch_millis_now();
        let mut agents = Vec::with_capacity(self.agents.len());
        for agent in self.agents.values() {
            agents.push(PersistedSubAgent {
                id: agent.id.clone(),
                session_name: Some(agent.session_name.clone()),
                fork_context: agent.fork_context,
                agent_type: agent.agent_type.clone(),
                prompt: agent.prompt.clone(),
                assignment: agent.assignment.clone(),
                model: agent.model.clone(),
                nickname: agent.nickname.clone(),
                status: agent.status.clone(),
                result: agent.result.clone(),
                steps_taken: agent.steps_taken,
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
        };
        write_json_atomic(path, &payload)
    }

    fn persist_state_best_effort(&self) {
        if let Err(err) = self.persist_state() {
            // Must not be `eprintln!` — raw stderr inside the alt-screen
            // leaks into the buffer and produces the scroll-demon
            // regression (#1085). Routed through tracing so the
            // file-backed subscriber in `runtime_log` captures it.
            tracing::warn!(target: "subagent", ?err, "failed to persist sub-agent state");
        }
    }

    fn load_state(&mut self) -> Result<()> {
        let Some(path) = self.state_path.as_ref() else {
            return Ok(());
        };
        if !path.exists() {
            return Ok(());
        }

        let raw = fs::read_to_string(path)?;
        let state = serde_json::from_str::<PersistedSubAgentState>(&raw)?;
        if state.schema_version != SUBAGENT_STATE_SCHEMA_VERSION {
            return Err(anyhow!(
                "Unsupported sub-agent state schema {}",
                state.schema_version
            ));
        }

        self.agents.clear();
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

        Ok(())
    }

    /// Count running agents.
    pub fn running_count(&self) -> usize {
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

        if self.running_count() >= self.max_agents {
            return Err(anyhow!(
                "Sub-agent limit reached (max {}, running {}). Cancel, close, or wait for an existing agent to finish. Consider issuing multiple tool calls in one turn (the dispatcher runs them in parallel) for parallel one-shot work.",
                self.max_agents,
                self.running_count()
            ));
        }

        if let Some(model) = options.model.as_deref() {
            runtime.model = model.to_string();
        }
        let effective_model = runtime.model.clone();
        let agent_id = format!("agent_{}", &Uuid::new_v4().to_string()[..8]);
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
                return Err(anyhow!(
                    "Sub-agent session name '{name}' is already in use by agent_id '{}' (status: {}). \
                     Reuse that agent_id with agent_eval/agent_close, or open with a different name.",
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

        if let Some(event_tx) = runtime.event_tx.clone() {
            let _ = event_tx.try_send(Event::AgentSpawned {
                id: agent_id.clone(),
                prompt: prompt.clone(),
            });
        }

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
            input_rx,
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

    /// Cancel a running sub-agent.
    pub fn cancel(&mut self, agent_id: &str) -> Result<SubAgentResult> {
        let (snapshot, changed) = {
            let agent = self
                .agents
                .get_mut(agent_id)
                .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;

            let mut changed = false;
            if agent.status == SubAgentStatus::Running {
                agent.status = SubAgentStatus::Cancelled;
                release_resident_leases_for(&agent.id);
                if let Some(handle) = agent.task_handle.take() {
                    handle.abort();
                }
                changed = true;
            }
            (agent.snapshot(), changed)
        };

        if changed {
            self.persist_state_best_effort();
        }
        Ok(snapshot)
    }

    /// Resume a non-running sub-agent by restarting it with the original assignment.
    #[allow(dead_code)] // Legacy agent_resume path; retained until registry migration.
    pub fn resume(
        &mut self,
        manager_handle: SharedSubAgentManager,
        runtime: SubAgentRuntime,
        agent_id: &str,
    ) -> Result<SubAgentResult> {
        let status = self
            .agents
            .get(agent_id)
            .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?
            .status
            .clone();

        if status == SubAgentStatus::Running {
            let agent = self
                .agents
                .get(agent_id)
                .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;
            return Ok(agent.snapshot());
        }

        if self.running_count() >= self.max_agents {
            return Err(anyhow!(
                "Sub-agent limit reached (max {}, running {}). Close or wait for an existing agent before resuming. Consider issuing multiple tool calls in one turn (the dispatcher runs them in parallel) for parallel one-shot work.",
                self.max_agents,
                self.running_count()
            ));
        }

        let snapshot = {
            let agent = self
                .agents
                .get_mut(agent_id)
                .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;

            let (input_tx, input_rx) = mpsc::unbounded_channel();
            let restarted_at = Instant::now();
            let mut restart_runtime = runtime.clone();
            if !agent.model.trim().is_empty() && agent.model != "unknown" {
                restart_runtime.model.clone_from(&agent.model);
            }
            let task = SubAgentTask {
                manager_handle,
                runtime: restart_runtime,
                agent_id: agent.id.clone(),
                agent_type: agent.agent_type.clone(),
                prompt: agent.prompt.clone(),
                assignment: agent.assignment.clone(),
                allowed_tools: agent.allowed_tools.clone(),
                fork_context: false,
                started_at: restarted_at,
                max_steps: self.max_steps,
                input_rx,
            };
            let handle = spawn_supervised(
                "subagent-task-resume",
                std::panic::Location::caller(),
                run_subagent_task(task),
            );

            agent.status = SubAgentStatus::Running;
            agent.result = None;
            agent.steps_taken = 0;
            agent.started_at = restarted_at;
            agent.last_activity_at = restarted_at;
            agent.input_tx = Some(input_tx);
            agent.task_handle = Some(handle);

            if let Some(event_tx) = runtime.event_tx {
                let _ = event_tx.try_send(Event::AgentSpawned {
                    id: agent.id.clone(),
                    prompt: format!("(resumed) {}", agent.prompt),
                });
            }

            agent.snapshot()
        };
        self.persist_state_best_effort();

        Ok(snapshot)
    }

    /// Send input to a running sub-agent.
    pub fn send_input(&mut self, agent_id: &str, text: String, interrupt: bool) -> Result<()> {
        let agent = self
            .agents
            .get_mut(agent_id)
            .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;

        if agent.status != SubAgentStatus::Running {
            return Err(anyhow!("Agent {agent_id} is not running"));
        }

        let tx = agent
            .input_tx
            .as_ref()
            .ok_or_else(|| anyhow!("Agent {agent_id} cannot accept input"))?;

        tx.send(SubAgentInput { text, interrupt })
            .map_err(|_| anyhow!("Failed to send input to agent {agent_id}"))?;

        Ok(())
    }

    /// Update assignment metadata and optionally send immediate guidance.
    pub fn assign(
        &mut self,
        agent_id: &str,
        objective: Option<String>,
        role: Option<String>,
        message: Option<String>,
        interrupt: bool,
    ) -> Result<SubAgentResult> {
        if objective.is_none() && role.is_none() && message.is_none() {
            return Err(anyhow!(
                "Provide at least one of objective, role, or message"
            ));
        }

        if message.is_some() {
            let status = self
                .agents
                .get(agent_id)
                .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?
                .status
                .clone();
            if status != SubAgentStatus::Running {
                return Err(anyhow!(
                    "Agent {agent_id} is not running; cannot deliver assignment message"
                ));
            }
        }

        let mut changed = false;
        let (input_tx, payload) = {
            let agent = self
                .agents
                .get_mut(agent_id)
                .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;

            let mut assignment_lines = Vec::new();
            if let Some(objective) = objective {
                let objective = objective.trim();
                if objective.is_empty() {
                    return Err(anyhow!("objective cannot be empty"));
                }
                if agent.assignment.objective != objective {
                    agent.assignment.objective = objective.to_string();
                    changed = true;
                }
                assignment_lines.push(format!("- objective: {}", agent.assignment.objective));
            }

            if let Some(role) = role {
                let normalized = normalize_role_alias(&role)
                    .ok_or_else(|| {
                        anyhow!("Invalid role alias '{role}'. Use: {VALID_ROLE_ALIASES}")
                    })?
                    .to_string();
                if agent.assignment.role.as_deref() != Some(normalized.as_str()) {
                    agent.assignment.role = Some(normalized.clone());
                    changed = true;
                }
                assignment_lines.push(format!("- role: {normalized}"));
            }

            let mut payload_parts = Vec::new();
            if !assignment_lines.is_empty() && agent.status == SubAgentStatus::Running {
                payload_parts.push(format!(
                    "Assignment updated:\n{}",
                    assignment_lines.join("\n")
                ));
            }
            if let Some(message) = message {
                let message = message.trim();
                if message.is_empty() {
                    return Err(anyhow!("message cannot be empty"));
                }
                payload_parts.push(format!("Coordinator note:\n{message}"));
            }

            let payload = if payload_parts.is_empty() {
                None
            } else {
                Some(payload_parts.join("\n\n"))
            };

            (agent.input_tx.clone(), payload)
        };

        if let Some(payload) = payload {
            let tx = input_tx
                .ok_or_else(|| anyhow!("Agent {agent_id} cannot accept assignment input"))?;
            tx.send(SubAgentInput {
                text: payload,
                interrupt,
            })
            .map_err(|_| anyhow!("Failed to send assignment to agent {agent_id}"))?;
        }

        if changed {
            self.persist_state_best_effort();
        }

        self.get_result(agent_id)
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
    /// `include_archived = false` (the default for `agent_list`) drops
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
                auto_cancelled += 1;
            }
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
        auto_cancelled
    }

    fn update_from_result(&mut self, agent_id: &str, result: SubAgentResult) {
        let mut changed = false;
        if let Some(agent) = self.agents.get_mut(agent_id) {
            agent.status = result.status;
            agent.assignment = result.assignment;
            agent.result = result.result;
            agent.steps_taken = result.steps_taken;
            agent.task_handle = None;
            changed = true;
        }
        if changed {
            self.persist_state_best_effort();
        }
    }

    fn update_failed(&mut self, agent_id: &str, error: String) {
        let mut changed = false;
        if let Some(agent) = self.agents.get_mut(agent_id) {
            agent.status = SubAgentStatus::Failed(error);
            release_resident_leases_for(agent_id);
            agent.task_handle = None;
            changed = true;
        }
        if changed {
            self.persist_state_best_effort();
        }
    }
}

/// Thread-safe wrapper for `SubAgentManager`.
pub type SharedSubAgentManager = Arc<RwLock<SubAgentManager>>;

/// Model-facing session projection returned by the v0.8.33 sub-agent API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentSessionProjection {
    pub name: String,
    pub agent_id: String,
    pub status: String,
    pub terminal: bool,
    pub context_mode: String,
    pub fork_context: bool,
    pub prefix_cache: SubAgentPrefixCacheProjection,
    pub transcript_handle: VarHandle,
    pub snapshot: SubAgentResult,
    #[serde(default, skip_serializing_if = "is_false")]
    pub timed_out: bool,
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

async fn subagent_session_projection(
    snapshot: SubAgentResult,
    timed_out: bool,
    context: &ToolContext,
) -> SubAgentSessionProjection {
    let transcript_session_id = format!("agent:{}", snapshot.agent_id);
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

    SubAgentSessionProjection {
        name: snapshot.name.clone(),
        agent_id: snapshot.agent_id.clone(),
        status: subagent_status_name(&snapshot.status).to_string(),
        terminal: snapshot.status != SubAgentStatus::Running,
        context_mode: snapshot.context_mode.clone(),
        fork_context: snapshot.fork_context,
        prefix_cache: subagent_prefix_cache_projection(&snapshot),
        transcript_handle,
        snapshot,
        timed_out,
    }
}

fn default_state_path(workspace: &Path) -> PathBuf {
    // Prefer .codewhale, fall back to .deepseek for project-local state
    let primary = workspace.join(".codewhale").join("state");
    if primary.exists() {
        return primary.join(SUBAGENT_STATE_FILE);
    }
    workspace
        .join(".deepseek")
        .join("state")
        .join(SUBAGENT_STATE_FILE)
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

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_string_pretty(value)?;
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, payload)?;
    fs::rename(tmp_path, path)?;
    Ok(())
}

/// Create a shared sub-agent manager with a configurable limit.
#[must_use]
pub fn new_shared_subagent_manager(workspace: PathBuf, max_agents: usize) -> SharedSubAgentManager {
    new_shared_subagent_manager_with_timeout(
        workspace,
        max_agents,
        Duration::from_secs(crate::config::DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS),
    )
}

/// Create a shared sub-agent manager with configurable concurrency and stale
/// running-agent heartbeat timeout.
#[must_use]
pub fn new_shared_subagent_manager_with_timeout(
    workspace: PathBuf,
    max_agents: usize,
    running_heartbeat_timeout: Duration,
) -> SharedSubAgentManager {
    let max_agents = max_agents.clamp(1, MAX_SUBAGENTS);
    let state_path = default_state_path(&workspace);
    let mut manager = SubAgentManager::new(workspace, max_agents)
        .with_running_heartbeat_timeout(running_heartbeat_timeout)
        .with_state_path(state_path);
    if let Err(err) = manager.load_state() {
        // Routed through tracing instead of stderr — see comment in
        // `persist_state_best_effort` above.
        tracing::warn!(target: "subagent", ?err, "failed to load sub-agent state");
    }
    Arc::new(RwLock::new(manager))
}

// === Tool Implementations ===

/// Open a named background sub-agent session.
#[allow(dead_code)] // Registered by the adjacent v0.8.33 registry surface update.
pub struct AgentOpenTool {
    manager: SharedSubAgentManager,
    runtime: SubAgentRuntime,
}

impl AgentOpenTool {
    #[allow(dead_code)] // Registered by the adjacent v0.8.33 registry surface update.
    #[must_use]
    pub fn new(manager: SharedSubAgentManager, runtime: SubAgentRuntime) -> Self {
        Self { manager, runtime }
    }
}

#[async_trait]
impl ToolSpec for AgentOpenTool {
    fn name(&self) -> &'static str {
        "agent_open"
    }

    fn description(&self) -> &'static str {
        concat!(
            "Open a named child sub-agent session for focused background work. Returns the session name, status, agent_id, context_mode, prefix_cache metadata, and a handle_read-compatible transcript_handle. ",
            "Use agent_eval to fetch or wait on the session, and agent_close to cancel/close it.\n\n",
            "Context control is explicit: omit fork_context or set it false for a fresh child with an independent prefill; set fork_context=true for perspective fanout over the current parent context. ",
            "Forked children preserve the parent system prompt and leading message prefix byte-identically where the runtime has that prefix, so DeepSeek can reuse its prefix cache before the child-specific task is appended.\n\n",
            "Sub-agent results are self-reports. Re-verify claimed side effects such as file edits, commands, network writes, tests, or git operations before reporting them as facts."
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Stable model-facing session name. Defaults to the generated agent_id when omitted."
                },
                "session_name": {
                    "type": "string",
                    "description": "Alias for name"
                },
                "prompt": {
                    "type": "string",
                    "description": "Initial task description for the child session"
                },
                "message": {
                    "type": "string",
                    "description": "Alias for prompt"
                },
                "objective": {
                    "type": "string",
                    "description": "Alias for prompt"
                },
                "items": {
                    "type": "array",
                    "description": "Structured input items (text, mention, skill, local_image, image)",
                    "items": { "type": "object" }
                },
                "type": {
                    "type": "string",
                    "description": SUBAGENT_TYPE_DESCRIPTION
                },
                "agent_type": {
                    "type": "string",
                    "description": "Alias for type"
                },
                "role": {
                    "type": "string",
                    "description": SUBAGENT_ROLE_DESCRIPTION
                },
                "agent_role": {
                    "type": "string",
                    "description": "Alias for role"
                },
                "allowed_tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Explicit tool allowlist (required for custom type)"
                },
                "model": {
                    "type": "string",
                    "description": "Optional DeepSeek model id for this child"
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional working directory for the child; must be inside the parent workspace"
                },
                "resident_file": {
                    "type": "string",
                    "description": "Optional file path for cache-aware resident mode"
                },
                "fork_context": {
                    "type": "boolean",
                    "description": "false (default): fresh child with independent context/prefill. true: forked child that preserves the parent's byte-identical system/message prefix where available, then appends this task for DeepSeek prefix-cache reuse."
                },
                "max_depth": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 3,
                    "description": "Recursive child-agent budget for this session. 0 blocks agent_open from the child; 1-3 allow that many descendant levels."
                }
            }
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

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let spawn_tool = AgentSpawnTool::new(self.manager.clone(), self.runtime.clone());
        let result = spawn_tool.execute(input, context).await?;
        let snapshot: SubAgentResult = serde_json::from_str(&result.content).map_err(|e| {
            ToolError::execution_failed(format!("agent_open projection failed: {e}"))
        })?;
        let projection = subagent_session_projection(snapshot, false, context).await;
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

/// Open a fast, non-thinking Flash V4 execution agent.
///
/// This is deliberately a thin wrapper over the durable `agent_open` runtime:
/// cost accounting, mailbox updates, transcript handles, cancellation, and
/// `agent_eval`/`agent_close` all stay on the same path.
pub struct ToolAgentTool {
    manager: SharedSubAgentManager,
    runtime: SubAgentRuntime,
}

impl ToolAgentTool {
    #[must_use]
    pub fn new(manager: SharedSubAgentManager, runtime: SubAgentRuntime) -> Self {
        Self { manager, runtime }
    }
}

#[async_trait]
impl ToolSpec for ToolAgentTool {
    fn name(&self) -> &'static str {
        "tool_agent"
    }

    fn description(&self) -> &'static str {
        concat!(
            "Open an experimental fast-lane execution agent (Fin): DeepSeek V4 Flash with thinking forced off. ",
            "Use it for simple tool-bound work such as OCR, file/search lookups, fetches, or command probes where the parent model should keep planning and synthesis context clean. ",
            "Returns the same session projection as agent_open; use agent_eval to fetch/wait and agent_close to close it. ",
            "Do not use this for nuanced implementation, architecture, release decisions, or tasks that need careful reasoning."
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Stable model-facing session name. Defaults to the generated agent_id when omitted."
                },
                "session_name": {
                    "type": "string",
                    "description": "Alias for name"
                },
                "prompt": {
                    "type": "string",
                    "description": "Initial tool-bound task for the fast execution agent"
                },
                "message": {
                    "type": "string",
                    "description": "Alias for prompt"
                },
                "objective": {
                    "type": "string",
                    "description": "Alias for prompt"
                },
                "items": {
                    "type": "array",
                    "description": "Structured input items (text, mention, skill, local_image, image)",
                    "items": { "type": "object" }
                },
                "allowed_tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional explicit tool allowlist for this executor"
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional working directory for the child; must be inside the parent workspace"
                },
                "fork_context": {
                    "type": "boolean",
                    "description": "Defaults to false. Set true only when the executor needs the parent prefix."
                },
                "max_depth": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 3,
                    "description": "Recursive child-agent budget. Defaults to 0 for tool_agent."
                }
            },
            "required": ["prompt"]
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

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let mut forwarded = input;
        let object = forwarded.as_object_mut().ok_or_else(|| {
            ToolError::invalid_input("tool_agent input must be an object".to_string())
        })?;
        object.insert("type".to_string(), Value::String("tool-agent".to_string()));
        object.remove("model");
        object.remove("agent_type");
        object.remove("agent_name");
        object.remove("role");
        object.remove("agent_role");
        object
            .entry("fork_context".to_string())
            .or_insert(Value::Bool(false));
        object.entry("max_depth".to_string()).or_insert(json!(0));

        AgentOpenTool::new(self.manager.clone(), self.runtime.clone())
            .execute(forwarded, context)
            .await
    }
}

/// Tool to spawn a background sub-agent.
pub struct AgentSpawnTool {
    manager: SharedSubAgentManager,
    runtime: SubAgentRuntime,
    name: &'static str,
}

impl AgentSpawnTool {
    /// Create a new spawn tool.
    #[must_use]
    pub fn new(manager: SharedSubAgentManager, runtime: SubAgentRuntime) -> Self {
        Self::with_name(manager, runtime, "agent_spawn")
    }

    /// Create a new spawn tool with a custom tool name alias.
    #[must_use]
    pub fn with_name(
        manager: SharedSubAgentManager,
        runtime: SubAgentRuntime,
        name: &'static str,
    ) -> Self {
        Self {
            manager,
            runtime,
            name,
        }
    }
}

#[async_trait]
impl ToolSpec for AgentSpawnTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        concat!(
            "Spawn a background sub-agent for a focused task. Returns an agent_id immediately; follow with agent_result to retrieve the final result. Default cap of 10 concurrent sub-agents (configurable via `[subagents].max_concurrent`); each is a full sub-agent loop, so cancel or wait if you hit the cap. For parallel one-shot LLM queries, just emit multiple tool calls in one turn — the dispatcher runs them in parallel.\n\n",
            "## Trust model: subagent results are self-reports, not verified facts\n\n",
            "`agent_result` returns the child's narrative summary of what happened. For operations with external side effects, the child's summary may be wrong. Re-verify before reporting success to the user:\n\n",
            "| Side effect | Re-verify with |\n|---|---|\n| URL claimed posted/written | `fetch_url` and check the response |\n| File claimed created | `read_file` or `list_dir` |\n| File claimed edited | `read_file` and check the change is present |\n| HTTP POST/PUT response | inspect status code and body |\n| Git operation | `git_status` / `git_diff` |\n| Test claimed passing | `run_tests` |\n| Process claimed started | `exec_shell` (e.g. `pgrep`, `lsof -i`) |\n\n",
            "If the child returns a verifiable handle (URL, file path, exit code, commit SHA), check it. If it doesn't, ask the child to return one or verify yourself before proceeding."
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Task description for the sub-agent"
                },
                "message": {
                    "type": "string",
                    "description": "Alias for prompt"
                },
                "objective": {
                    "type": "string",
                    "description": "Alias for prompt"
                },
                "items": {
                    "type": "array",
                    "description": "Structured input items (text, mention, skill, local_image, image)",
                    "items": {
                        "type": "object"
                    }
                },
                "type": {
                    "type": "string",
                    "description": SUBAGENT_TYPE_DESCRIPTION
                },
                "agent_type": {
                    "type": "string",
                    "description": "Alias for type"
                },
                "agent_name": {
                    "type": "string",
                    "description": "Alias for type"
                },
                "role": {
                    "type": "string",
                    "description": SUBAGENT_ROLE_DESCRIPTION
                },
                "agent_role": {
                    "type": "string",
                    "description": "Alias for role"
                },
                "allowed_tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Explicit tool allowlist (required for custom type). Default behavior is full registry inheritance from the parent; approval-gated tools still require an auto-approved parent."
                },
                "model": {
                    "type": "string",
                    "description": "Optional DeepSeek model id for this child. Explicit model wins over role/type defaults; omit to inherit."
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional working directory for the child. Must be inside the parent's workspace (use a relative path or an absolute path under the workspace root). Used for the parallel-worktree pattern: parent runs `git worktree add .worktrees/feature-x ...` then spawns the child with `cwd: \".worktrees/feature-x\"`."
                },
                "resident_file": {
                    "type": "string",
                    "description": "Optional file path for cache-aware resident mode. When set, the child's system prefix is augmented with the full contents of this file so DeepSeek's prefix cache stays warm across follow-up send_input calls. Only one agent may hold a resident lease on a given file at a time — a second spawn with the same path receives a conflict warning in the result."
                },
                "fork_context": {
                    "type": "boolean",
                    "description": "When true, inherit the parent's system prompt and conversation prefix before appending this task. This preserves DeepSeek prefix-cache reuse and gives the child full parent context. Defaults to false for independent exploration."
                }
            }
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

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let spawn_request = parse_spawn_request(&input)?;

        // Depth cap: reject before locking the manager so we don't introduce
        // unnecessary contention. Mirrors codex's pattern (allow-equal at the
        // boundary; reject when `next > max`).
        if self.runtime.would_exceed_depth() {
            return Err(ToolError::execution_failed(format!(
                "Sub-agent depth limit reached (current depth {}, max {}). \
                 Increase via [runtime] max_spawn_depth in config.toml.",
                self.runtime.spawn_depth, self.runtime.max_spawn_depth
            )));
        }

        // Validate cwd if supplied: must canonicalize inside the parent
        // workspace. Catches accidents like `cwd: "/etc"`.
        let validated_cwd = if let Some(requested_cwd) = spawn_request.cwd.as_ref() {
            let parent_workspace = &self.runtime.context.workspace;
            let resolved = if requested_cwd.is_absolute() {
                requested_cwd.clone()
            } else {
                parent_workspace.join(requested_cwd)
            };
            let canonical = resolved.canonicalize().map_err(|e| {
                ToolError::invalid_input(format!(
                    "Invalid cwd '{}': {e} (path may not exist yet — create the worktree first)",
                    requested_cwd.display()
                ))
            })?;
            let workspace_canonical = parent_workspace
                .canonicalize()
                .unwrap_or_else(|_| parent_workspace.clone());
            if !canonical.starts_with(&workspace_canonical) {
                return Err(ToolError::invalid_input(format!(
                    "cwd must be inside the parent workspace: {} is not under {}",
                    canonical.display(),
                    workspace_canonical.display()
                )));
            }
            Some(canonical)
        } else {
            None
        };

        // Derive the child's runtime as a durable background job: it keeps
        // its own cancellation token, inherits the parent approval state, and
        // optionally overrides cwd if the caller passed one (used for the
        // parallel-worktree pattern).
        let mut child_runtime = self.runtime.background_runtime();
        if let Some(max_depth) = spawn_request.max_depth {
            child_runtime.max_spawn_depth = child_runtime.spawn_depth.saturating_add(max_depth);
        }
        if let Some(cwd) = validated_cwd {
            child_runtime.context.workspace = cwd;
        }
        let configured_model = match spawn_request.model.clone() {
            Some(model) => Some(model),
            None => configured_model_for_role_or_type(
                &self.runtime,
                spawn_request.assignment.role.as_deref(),
                &spawn_request.agent_type,
            )?,
        };

        // Cache-aware resident mode (#529): prepend file contents to the prompt
        // so the child's prefix is byte-stable for DeepSeek prefix caching.
        let (effective_prompt, resident_conflict) =
            if let Some(ref file_path) = spawn_request.resident_file {
                let abs_path = if std::path::Path::new(file_path).is_absolute() {
                    std::path::PathBuf::from(file_path)
                } else {
                    self.runtime.context.workspace.join(file_path)
                };
                let file_contents = std::fs::read_to_string(&abs_path)
                    .unwrap_or_else(|e| format!("<!-- resident_file read error: {e} -->"));
                let prefixed = format!(
                    "<!-- resident_file: {file_path} -->\n```\n{file_contents}\n```\n\n{}",
                    spawn_request.prompt
                );
                // Check ownership (best-effort, non-blocking).
                let conflict = {
                    let leases = RESIDENT_LEASES
                        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
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

        let route = resolve_subagent_assignment_route(
            &self.runtime,
            configured_model,
            &effective_prompt,
            &spawn_request.agent_type,
        )
        .await;
        child_runtime.model = route.model.clone();
        child_runtime.reasoning_effort = route.reasoning_effort.clone();
        child_runtime.reasoning_effort_auto = false;
        let effective_model = route.model;

        let mut manager = self.manager.write().await;

        let result = manager
            .spawn_background_with_assignment_options(
                Arc::clone(&self.manager),
                child_runtime,
                spawn_request.agent_type,
                effective_prompt,
                spawn_request.assignment,
                spawn_request.allowed_tools,
                SubAgentSpawnOptions {
                    name: spawn_request.session_name.clone(),
                    model: Some(effective_model),
                    nickname: None,
                    fork_context: spawn_request.fork_context,
                },
            )
            .map_err(|e| ToolError::execution_failed(format!("Failed to spawn sub-agent: {e}")))?;

        // Replace the "pending" lease placeholder with the real agent id now that
        // the manager has assigned one. Without this, `release_resident_leases_for`
        // (which matches by agent id at terminal-state transitions) can never find
        // the entry — leases would stay stamped as "pending" forever, defeating the
        // release machinery added in #660.
        if let Some(ref file_path) = spawn_request.resident_file
            && let Some(lock) = RESIDENT_LEASES.get()
            && let Ok(mut guard) = lock.lock()
            && let Some(owner) = guard.get_mut(file_path)
            && owner == "pending"
        {
            *owner = result.agent_id.clone();
        }

        let mut tool_result = if self.name == "spawn_agent" {
            let mut payload = json!({
                "agent_id": result.agent_id.clone(),
                "nickname": result.nickname.clone(),
                "model": result.model.clone()
            });
            if let Some(ref warning) = resident_conflict {
                payload["resident_conflict"] = json!(warning);
            }
            ToolResult::json(&payload).map_err(|e| ToolError::execution_failed(e.to_string()))?
        } else {
            ToolResult::json(&result).map_err(|e| ToolError::execution_failed(e.to_string()))?
        };
        if result.status == SubAgentStatus::Running {
            if self.name == "spawn_agent" {
                tool_result.metadata = Some(json!({
                    "status": "Running",
                    "snapshot": result
                }));
            } else {
                tool_result.metadata = Some(json!({ "status": "Running" }));
            }
        }
        // Annotate alias invocations with a deprecation notice so the model
        // can migrate to the canonical name before removal in v0.8.0.
        if self.name == "spawn_agent" {
            tool_result = wrap_with_deprecation_notice(tool_result, "spawn_agent", "agent_spawn");
        }
        Ok(tool_result)
    }
}

/// Evaluate/fetch a child session boundary for the v0.8.33 sub-agent API.
#[allow(dead_code)] // Registered by the adjacent v0.8.33 registry surface update.
pub struct AgentEvalTool {
    manager: SharedSubAgentManager,
}

impl AgentEvalTool {
    #[allow(dead_code)] // Registered by the adjacent v0.8.33 registry surface update.
    #[must_use]
    pub fn new(manager: SharedSubAgentManager) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ToolSpec for AgentEvalTool {
    fn name(&self) -> &'static str {
        "agent_eval"
    }

    fn description(&self) -> &'static str {
        "Fetch or wait on a child sub-agent session. Optionally deliver a message/items to a running session, then return the latest session projection. With block=true (default), waits for the session to reach a terminal boundary; block=false is a non-blocking status fetch. Terminal projections expose a handle_read-compatible transcript_handle for the full child transcript."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Session name returned by agent_open"
                },
                "agent_name": {
                    "type": "string",
                    "description": "Alias for name (session name returned by agent_open)"
                },
                "agent_id": {
                    "type": "string",
                    "description": "Generated agent id returned by agent_open"
                },
                "id": {
                    "type": "string",
                    "description": "Alias for agent_id"
                },
                "message": {
                    "type": "string",
                    "description": "Optional message to deliver before evaluating the session"
                },
                "input": {
                    "type": "string",
                    "description": "Alias for message"
                },
                "items": {
                    "type": "array",
                    "description": "Structured input items (text, mention, skill, local_image, image)",
                    "items": { "type": "object" }
                },
                "interrupt": {
                    "type": "boolean",
                    "description": "When sending input, prioritize it over pending inputs"
                },
                "block": {
                    "type": "boolean",
                    "description": "Wait for a terminal boundary before returning (default true)"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Max wait time in milliseconds (default: 30000, clamped to 1000-3600000)"
                }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let agent_ref = input
            .get("name")
            .or_else(|| input.get("agent_name"))
            .or_else(|| input.get("agent_id"))
            .or_else(|| input.get("id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ToolError::missing_field("name"))?;
        let message = parse_optional_text_or_items(&input, &["message", "input"], "items")?;
        let interrupt = optional_bool(&input, "interrupt", false);
        let block = optional_bool(&input, "block", true);
        let timeout_ms = optional_u64(&input, "timeout_ms", DEFAULT_RESULT_TIMEOUT_MS)
            .clamp(1000, MAX_RESULT_TIMEOUT_MS);

        let agent_id = {
            let manager = self.manager.read().await;
            manager
                .resolve_agent_ref(agent_ref)
                .map_err(|e| ToolError::execution_failed(e.to_string()))?
        };

        // Track whether a supplied follow-up message actually reached the
        // child. A completed/failed/cancelled session cannot accept input, but
        // that must NOT abort the whole call: the parent still needs the
        // session projection (and its `transcript_handle`) to retrieve the
        // child's full output. Hard-failing here was #1738 — "agent_eval on a
        // completed session returns 'not running', no way to recover the full
        // child output".
        let mut message_delivery: Option<Value> = None;
        if let Some(message) = message {
            let terminal = {
                let manager = self.manager.read().await;
                manager
                    .get_result(&agent_id)
                    .map(|snap| snap.status != SubAgentStatus::Running)
                    .unwrap_or(false)
            };
            if terminal {
                message_delivery = Some(json!({
                    "delivered": false,
                    "reason": "session already terminated; follow-up not delivered",
                    "recover_full_output": "read the returned transcript_handle with handle_read"
                }));
            } else {
                let mut manager = self.manager.write().await;
                manager
                    .send_input(&agent_id, message, interrupt)
                    .map_err(|e| ToolError::execution_failed(e.to_string()))?;
                message_delivery = Some(json!({ "delivered": true }));
            }
        }

        let (snapshot, timed_out) = if block {
            wait_for_result(&self.manager, &agent_id, Duration::from_millis(timeout_ms)).await?
        } else {
            let manager = self.manager.read().await;
            (
                manager
                    .get_result(&agent_id)
                    .map_err(|e| ToolError::execution_failed(e.to_string()))?,
                false,
            )
        };

        let projection = subagent_session_projection(snapshot, timed_out, context).await;
        let mut result = ToolResult::json(&projection)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        result.metadata = Some(json!({
            "status": if timed_out { "TimedOut".to_string() } else { projection.status.clone() },
            "timed_out": timed_out,
            "terminal": projection.terminal,
            "context_mode": projection.context_mode,
            "timeout_ms": timeout_ms,
            "message_delivery": message_delivery
        }));
        Ok(result)
    }
}

/// Tool to fetch a sub-agent's result.
#[allow(dead_code)] // Legacy surface superseded by agent_eval.
pub struct AgentResultTool {
    manager: SharedSubAgentManager,
}

impl AgentResultTool {
    /// Create a new result tool.
    #[allow(dead_code)] // Legacy surface superseded by agent_eval.
    #[must_use]
    pub fn new(manager: SharedSubAgentManager) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ToolSpec for AgentResultTool {
    fn name(&self) -> &'static str {
        "agent_result"
    }

    fn description(&self) -> &'static str {
        "Get the latest status or final result for a sub-agent. Set `block: true` to wait until the \
         agent reaches a terminal state (respects `timeout_ms`)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "ID returned by agent_spawn"
                },
                "id": {
                    "type": "string",
                    "description": "Alias for agent_id"
                },
                "block": {
                    "type": "boolean",
                    "description": "Wait for completion (default: false)"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Max wait time in milliseconds (default: 30000, clamped to 1000-3600000)"
                }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let agent_id = input
            .get("agent_id")
            .or_else(|| input.get("id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::missing_field("agent_id"))?;
        let block = optional_bool(&input, "block", false);
        let timeout_ms = optional_u64(&input, "timeout_ms", DEFAULT_RESULT_TIMEOUT_MS)
            .clamp(1000, MAX_RESULT_TIMEOUT_MS);

        let (result, timed_out) = if block {
            wait_for_result(&self.manager, agent_id, Duration::from_millis(timeout_ms)).await?
        } else {
            let manager = self.manager.read().await;
            (
                manager
                    .get_result(agent_id)
                    .map_err(|e| ToolError::execution_failed(e.to_string()))?,
                false,
            )
        };

        let mut tool_result =
            ToolResult::json(&result).map_err(|e| ToolError::execution_failed(e.to_string()))?;
        if timed_out {
            tool_result.metadata = Some(json!({
                "status": "TimedOut",
                "timed_out": true,
                "timeout_ms": timeout_ms
            }));
        } else if result.status == SubAgentStatus::Running {
            tool_result.metadata = Some(json!({ "status": "Running" }));
        }
        Ok(tool_result)
    }
}

/// Tool to cancel a sub-agent.
#[allow(dead_code)] // Legacy surface superseded by agent_close.
pub struct AgentCancelTool {
    manager: SharedSubAgentManager,
}

impl AgentCancelTool {
    /// Create a new cancel tool.
    #[allow(dead_code)] // Legacy surface superseded by agent_close.
    #[must_use]
    pub fn new(manager: SharedSubAgentManager) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ToolSpec for AgentCancelTool {
    fn name(&self) -> &'static str {
        "agent_cancel"
    }

    fn description(&self) -> &'static str {
        "Cancel a running sub-agent. Returns the final snapshot with the cancelled status."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "ID returned by agent_spawn"
                }
            },
            "required": ["agent_id"]
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

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let agent_id = required_str(&input, "agent_id")?;
        let mut manager = self.manager.write().await;
        let result = manager
            .cancel(agent_id)
            .map_err(|e| ToolError::execution_failed(format!("Failed to cancel sub-agent: {e}")))?;

        ToolResult::json(&result).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

/// Tool to list all sub-agents.
#[allow(dead_code)] // Legacy surface superseded by named agent_open/eval/close sessions.
pub struct AgentListTool {
    manager: SharedSubAgentManager,
}

/// Tool to close a running sub-agent (alias for cancel).
pub struct AgentCloseTool {
    manager: SharedSubAgentManager,
}

impl AgentCloseTool {
    /// Create a new close tool.
    #[must_use]
    pub fn new(manager: SharedSubAgentManager) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ToolSpec for AgentCloseTool {
    fn name(&self) -> &'static str {
        "agent_close"
    }

    fn description(&self) -> &'static str {
        "Close a child sub-agent session by cancelling it if still running. Returns the final session projection with transcript_handle metadata."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Agent id returned by agent_open"
                },
                "name": {
                    "type": "string",
                    "description": "Session name returned by agent_open"
                },
                "agent_name": {
                    "type": "string",
                    "description": "Alias for name (session name returned by agent_open)"
                },
                "agent_id": {
                    "type": "string",
                    "description": "Alias for id"
                }
            }
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

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let agent_id = input
            .get("name")
            .or_else(|| input.get("agent_name"))
            .or_else(|| input.get("id"))
            .or_else(|| input.get("agent_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::missing_field("id"))?;
        let agent_id = {
            let manager = self.manager.read().await;
            manager
                .resolve_agent_ref(agent_id)
                .map_err(|e| ToolError::execution_failed(e.to_string()))?
        };
        let mut manager = self.manager.write().await;
        let result = manager
            .cancel(&agent_id)
            .map_err(|e| ToolError::execution_failed(format!("Failed to close sub-agent: {e}")))?;
        let projection = subagent_session_projection(result, false, context).await;
        ToolResult::json(&projection).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

/// Tool to resume an existing sub-agent.
#[allow(dead_code)] // Legacy surface superseded by agent_open/eval.
pub struct AgentResumeTool {
    manager: SharedSubAgentManager,
    runtime: SubAgentRuntime,
}

impl AgentResumeTool {
    /// Create a new resume tool.
    #[allow(dead_code)] // Legacy surface superseded by agent_open/eval.
    #[must_use]
    pub fn new(manager: SharedSubAgentManager, runtime: SubAgentRuntime) -> Self {
        Self { manager, runtime }
    }
}

#[async_trait]
impl ToolSpec for AgentResumeTool {
    fn name(&self) -> &'static str {
        "resume_agent"
    }

    fn description(&self) -> &'static str {
        "Resume a previously closed or completed sub-agent by restarting its assignment."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Agent id to resume"
                },
                "agent_id": {
                    "type": "string",
                    "description": "Alias for id"
                }
            }
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

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let agent_id = input
            .get("id")
            .or_else(|| input.get("agent_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::missing_field("id"))?;
        let mut manager = self.manager.write().await;
        let result = manager
            .resume(Arc::clone(&self.manager), self.runtime.clone(), agent_id)
            .map_err(|e| ToolError::execution_failed(format!("Failed to resume sub-agent: {e}")))?;
        ToolResult::json(&result).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

impl AgentListTool {
    /// Create a new list tool.
    #[allow(dead_code)] // Legacy surface superseded by named agent_open/eval/close sessions.
    #[must_use]
    pub fn new(manager: SharedSubAgentManager) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ToolSpec for AgentListTool {
    fn name(&self) -> &'static str {
        "agent_list"
    }

    fn description(&self) -> &'static str {
        "List sub-agents from the current session with their status, type, assignment, steps, \
         and duration. Pass `include_archived=true` to also see agents that were spawned in a \
         prior session (e.g. before the TUI restarted) and persisted on disk; those carry \
         `from_prior_session: true` in the result. Default is the current-session view because \
         prior-session agents almost never matter for the live turn."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "include_archived": {
                    "type": "boolean",
                    "description": "When true, include agents from prior sessions in the listing. Default false."
                }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let include_archived = input
            .get("include_archived")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut manager = self.manager.write().await;
        manager.cleanup(COMPLETED_AGENT_RETENTION);
        let results = manager.list_filtered(include_archived);
        ToolResult::json(&results).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

/// Tool to send input to a running sub-agent.
#[allow(dead_code)] // Legacy surface superseded by agent_eval.
pub struct AgentSendInputTool {
    manager: SharedSubAgentManager,
    name: &'static str,
}

impl AgentSendInputTool {
    /// Create a new send-input tool.
    #[allow(dead_code)] // Legacy surface superseded by agent_eval.
    #[must_use]
    pub fn new(manager: SharedSubAgentManager, name: &'static str) -> Self {
        Self { manager, name }
    }
}

#[async_trait]
impl ToolSpec for AgentSendInputTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        "Send input to a running sub-agent. Returns the agent's current snapshot after delivery."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "ID returned by agent_spawn"
                },
                "id": {
                    "type": "string",
                    "description": "Alias for agent_id"
                },
                "message": {
                    "type": "string",
                    "description": "Message to deliver to the agent"
                },
                "input": {
                    "type": "string",
                    "description": "Alias for message"
                },
                "items": {
                    "type": "array",
                    "description": "Structured input items (text, mention, skill, local_image, image)",
                    "items": {
                        "type": "object"
                    }
                },
                "interrupt": {
                    "type": "boolean",
                    "description": "Prioritize this message over pending inputs"
                }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![]
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let agent_id = input
            .get("agent_id")
            .or_else(|| input.get("id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::missing_field("agent_id"))?;
        let message = parse_text_or_items(&input, &["message", "input"], "items", "message")?;
        let interrupt = optional_bool(&input, "interrupt", false);

        let mut manager = self.manager.write().await;
        manager
            .send_input(agent_id, message, interrupt)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        let snapshot = manager
            .get_result(agent_id)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;

        let tool_result =
            ToolResult::json(&snapshot).map_err(|e| ToolError::execution_failed(e.to_string()))?;
        // Annotate the alias name "send_input" with a deprecation notice;
        // the canonical name "agent_send_input" passes through unchanged.
        if self.name == "send_input" {
            Ok(wrap_with_deprecation_notice(
                tool_result,
                "send_input",
                "agent_send_input",
            ))
        } else {
            Ok(tool_result)
        }
    }
}

/// Tool to update assignment metadata for a sub-agent.
#[allow(dead_code)] // Legacy surface superseded by agent_eval/open metadata.
pub struct AgentAssignTool {
    manager: SharedSubAgentManager,
    name: &'static str,
}

impl AgentAssignTool {
    /// Create a new assignment tool.
    #[allow(dead_code)] // Legacy surface superseded by agent_eval/open metadata.
    #[must_use]
    pub fn new(manager: SharedSubAgentManager, name: &'static str) -> Self {
        Self { manager, name }
    }
}

#[async_trait]
impl ToolSpec for AgentAssignTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        "Update a sub-agent's assignment (objective, role) and optionally deliver an immediate \
         coordinator note. The update is delivered as a high-priority message when `interrupt` is \
         true (the default). Returns the agent's current snapshot."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "Agent id returned by agent_spawn"
                },
                "id": {
                    "type": "string",
                    "description": "Alias for agent_id"
                },
                "objective": {
                    "type": "string",
                    "description": "Updated assignment objective"
                },
                "role": {
                    "type": "string",
                    "description": SUBAGENT_ROLE_DESCRIPTION
                },
                "agent_role": {
                    "type": "string",
                    "description": "Alias for role"
                },
                "message": {
                    "type": "string",
                    "description": "Optional coordinator note to send to the agent"
                },
                "input": {
                    "type": "string",
                    "description": "Alias for message"
                },
                "items": {
                    "type": "array",
                    "description": "Structured input items (text, mention, skill, local_image, image)",
                    "items": {
                        "type": "object"
                    }
                },
                "interrupt": {
                    "type": "boolean",
                    "description": "Prioritize this assignment update in the agent inbox (default: true)"
                }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![]
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let request = parse_assign_request(&input)?;
        let mut manager = self.manager.write().await;
        let result = manager
            .assign(
                &request.agent_id,
                request.objective,
                request.role,
                request.message,
                request.interrupt,
            )
            .map_err(|e| ToolError::execution_failed(format!("Failed to assign sub-agent: {e}")))?;

        ToolResult::json(&result).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

/// Tool to wait for sub-agents to complete.
#[allow(dead_code)] // Legacy surface superseded by agent_eval.
pub struct AgentWaitTool {
    manager: SharedSubAgentManager,
    name: &'static str,
}

impl AgentWaitTool {
    /// Create a new wait tool.
    #[allow(dead_code)] // Legacy surface superseded by agent_eval.
    #[must_use]
    pub fn new(manager: SharedSubAgentManager, name: &'static str) -> Self {
        Self { manager, name }
    }
}

#[async_trait]
impl ToolSpec for AgentWaitTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        "Wait for one or more sub-agents to reach a terminal status. Use `wait_mode: \"all\"` to block \
         until every listed agent finishes, or `wait_mode: \"any\"` (default) to return as soon as \
         one finishes. When no ids are given, waits on all currently running sub-agents."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Agent IDs to wait on. When omitted, waits on all currently running sub-agents."
                },
                "agent_ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Alias for ids"
                },
                "agent_id": {
                    "type": "string",
                    "description": "Single agent ID"
                },
                "id": {
                    "type": "string",
                    "description": "Alias for agent_id"
                },
                "wait_mode": {
                    "type": "string",
                    "description": "Wait behavior: any (default) or all"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Max wait time in milliseconds (default: 30000, clamped to 10000-3600000)"
                }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let timeout_ms = optional_u64(&input, "timeout_ms", DEFAULT_RESULT_TIMEOUT_MS)
            .clamp(MIN_WAIT_TIMEOUT_MS, MAX_RESULT_TIMEOUT_MS);
        let mut ids = parse_wait_ids(&input);
        if ids.is_empty() {
            let manager = self.manager.read().await;
            ids = manager
                .list()
                .into_iter()
                .filter(|snapshot| snapshot.status == SubAgentStatus::Running)
                .map(|snapshot| snapshot.agent_id)
                .collect();
        }
        let wait_mode = parse_wait_mode(&input)?;

        if ids.is_empty() {
            let empty: Vec<SubAgentResult> = Vec::new();
            let mut result =
                ToolResult::json(&empty).map_err(|e| ToolError::execution_failed(e.to_string()))?;
            result.metadata = Some(json!({
                "wait_mode": wait_mode.as_str(),
                "timed_out": false,
                "status": "Completed",
                "timeout_ms": timeout_ms,
                "waited_ids": [],
                "completed_ids": [],
                "running_ids": [],
                "status_by_id": {}
            }));
            return Ok(result);
        }

        let waited_ids = ids.clone();

        let (snapshots, timed_out) = wait_for_agents(
            &self.manager,
            &ids,
            wait_mode,
            Duration::from_millis(timeout_ms),
        )
        .await?;

        let all_done = snapshots
            .iter()
            .all(|snapshot| snapshot.status != SubAgentStatus::Running);
        let completed_ids = snapshots
            .iter()
            .filter(|snapshot| snapshot.status != SubAgentStatus::Running)
            .map(|snapshot| snapshot.agent_id.clone())
            .collect::<Vec<_>>();
        let running_ids = snapshots
            .iter()
            .filter(|snapshot| snapshot.status == SubAgentStatus::Running)
            .map(|snapshot| snapshot.agent_id.clone())
            .collect::<Vec<_>>();
        let status_by_id = snapshots
            .iter()
            .map(|snapshot| {
                (
                    snapshot.agent_id.clone(),
                    subagent_status_name(&snapshot.status).to_string(),
                )
            })
            .collect::<HashMap<_, _>>();

        let mut result =
            ToolResult::json(&snapshots).map_err(|e| ToolError::execution_failed(e.to_string()))?;
        result.metadata = Some(json!({
            "wait_mode": wait_mode.as_str(),
            "timed_out": timed_out,
            "status": if timed_out { "TimedOut" } else if all_done { "Completed" } else { "Partial" },
            "timeout_ms": timeout_ms,
            "waited_ids": waited_ids,
            "completed_ids": completed_ids,
            "running_ids": running_ids,
            "status_by_id": status_by_id
        }));
        Ok(result)
    }
}

/// Compatibility delegate tool. It routes through `agent_spawn`, but defaults
/// to `fork_context=true` because delegation is usually continuation work.
#[allow(dead_code)] // Legacy alias superseded by agent_open(fork_context=true).
pub struct DelegateToAgentTool {
    manager: SharedSubAgentManager,
    runtime: SubAgentRuntime,
}

impl DelegateToAgentTool {
    /// Create a new delegation tool.
    #[allow(dead_code)] // Legacy alias superseded by agent_open(fork_context=true).
    #[must_use]
    pub fn new(manager: SharedSubAgentManager, runtime: SubAgentRuntime) -> Self {
        Self { manager, runtime }
    }
}

#[async_trait]
impl ToolSpec for DelegateToAgentTool {
    fn name(&self) -> &'static str {
        "delegate_to_agent"
    }

    fn description(&self) -> &'static str {
        "Delegate a task to a specialized sub-agent. Compatibility wrapper around agent_spawn; \
         defaults fork_context=true so the child inherits the parent transcript. Use `type` \
         (or `agent_name`, `agent_type`) to pick the agent flavor."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": SUBAGENT_TYPE_DESCRIPTION
                },
                "type": {
                    "type": "string",
                    "description": "Alias for agent_name"
                },
                "agent_type": {
                    "type": "string",
                    "description": "Alias for agent_name"
                },
                "role": {
                    "type": "string",
                    "description": SUBAGENT_ROLE_DESCRIPTION
                },
                "agent_role": {
                    "type": "string",
                    "description": "Alias for role"
                },
                "objective": {
                    "type": "string",
                    "description": "The goal or task description for the agent"
                },
                "prompt": {
                    "type": "string",
                    "description": "Alias for objective"
                },
                "message": {
                    "type": "string",
                    "description": "Alias for objective"
                },
                "items": {
                    "type": "array",
                    "description": "Structured input items (text, mention, skill, local_image, image)",
                    "items": {
                        "type": "object"
                    }
                },
                "allowed_tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Explicit tool allowlist (required for custom type)"
                },
                "fork_context": {
                    "type": "boolean",
                    "description": "When true, inherit the parent's system prompt and conversation prefix before appending this task. delegate_to_agent defaults this to true."
                }
            }
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

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let spawn_tool = AgentSpawnTool::new(self.manager.clone(), self.runtime.clone());
        let input = with_default_fork_context(input, true);
        let result = spawn_tool.execute(input, context).await?;
        Ok(wrap_with_deprecation_notice(
            result,
            "delegate_to_agent",
            "agent_spawn",
        ))
    }
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
    match assignment.role.as_deref() {
        Some(role) if !role.trim().is_empty() => {
            format!(
                "{base}\n\nYou are operating in the role of `{}`.",
                role.trim()
            )
        }
        _ => base,
    }
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
    input_rx: mpsc::UnboundedReceiver<SubAgentInput>,
}

#[allow(clippy::too_many_lines)]
async fn run_subagent_task(task: SubAgentTask) {
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
        Ok(res) => (
            summarize_subagent_result(res),
            subagent_done_sentinel(&task.agent_id, res),
        ),
        Err(err) => {
            let annotated = annotate_child_model_error(&err.to_string(), &model_id);
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
                error: annotate_child_model_error(&err.to_string(), &model_id),
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
                annotate_child_model_error(&err.to_string(), &model_id),
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

/// Notify the engine's parent turn loop that a direct child finished
/// (issue #756). Returns `true` if a send was attempted, `false` if the
/// notification was skipped because this isn't a direct child or no channel
/// is wired. Skips silently when the channel sender has no receiver — the
/// engine outlives the runtime, so a dropped receiver means we're shutting
/// down anyway.
pub(crate) fn emit_parent_completion(
    runtime: &SubAgentRuntime,
    agent_id: &str,
    payload: &str,
) -> bool {
    if runtime.spawn_depth != 1 {
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

/// Build a `<codewhale:subagent.done>` JSON sentinel for a successful child.
/// Intended to surface in the parent's transcript so the model recognizes
/// child completion and can decide whether to read the full result via
/// `agent_eval`.
///
/// Keep this payload deliberately lean. The human summary is emitted on the
/// line immediately before the sentinel; duplicating it here bloats the next
/// parent request's cache-miss tail. Wall-clock duration is useful UI
/// telemetry, but it is volatile and not useful for model coordination.
fn subagent_done_sentinel(agent_id: &str, res: &SubAgentResult) -> String {
    let clipped = subagent_result_clipped(res);
    let payload = json!({
        "agent_id": agent_id,
        "agent_type": res.agent_type.as_str(),
        "status": subagent_status_name(&res.status),
        "summary_location": "previous_line",
        "result_clipped": clipped,
        "summary_complete": !clipped,
        // What the parent should do next: trust the previous-line summary, or
        // fetch the full transcript with agent_eval when it was clipped.
        "next_action": if clipped { "call_agent_eval" } else { "use_summary" },
        "details": "agent_eval",
    });
    format!("<codewhale:subagent.done>{payload}</codewhale:subagent.done>")
}

/// Build a `<codewhale:subagent.done>` sentinel for a failed child.
///
/// Kept lean: the (annotated) error is on the previous line (`error_location`)
/// and the full projection is available via `agent_eval`, so the sentinel only
/// signals the recovery path rather than re-embedding the error text.
fn subagent_failed_sentinel(agent_id: &str, _err: &str) -> String {
    let payload = json!({
        "agent_id": agent_id,
        "status": "failed",
        "error_location": "previous_line",
        "next_action": "call_agent_eval",
        "details": "agent_eval",
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
    messages: &[Message],
    steps_taken: u32,
    duration_ms: u64,
    fork_context: bool,
) -> VarHandle {
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
        "messages": messages,
    });
    let mut store = runtime.context.runtime.handle_store.lock().await;
    store.insert_json(format!("agent:{agent_id}"), "full_transcript", payload)
}

fn record_agent_progress(runtime: &SubAgentRuntime, agent_id: &str, message: impl Into<String>) {
    if let Ok(mut manager) = runtime.manager.try_write() {
        manager.touch(agent_id);
    }
    emit_agent_progress(
        runtime.event_tx.as_ref(),
        runtime.mailbox.as_ref(),
        agent_id,
        message.into(),
    );
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
    let runtime_for_tools = runtime.clone().with_fork_context(SubAgentForkContext {
        system: Some(request_system.clone()),
        messages: messages.clone(),
        structured_state_block: None,
    });
    let tool_registry = SubAgentToolRegistry::new(
        runtime_for_tools,
        agent_type.clone(),
        allowed_tools.clone(),
        Arc::new(Mutex::new(TodoList::new())),
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

    for _step in 0..max_steps {
        // Cooperative cancellation: bail if this session's token was cancelled
        // while we were between steps. Top-level model-visible sub-agents use
        // a detached token so parent turn cancellation does not stop them.
        if runtime.cancel_token.is_cancelled() {
            record_agent_progress(
                runtime,
                &agent_id,
                format!("step {steps}/{max_steps}: cancelled"),
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
                agent_type: agent_type.clone(),
                assignment: assignment.clone(),
                model: runtime.model.clone(),
                nickname: None,
                status,
                result: None,
                steps_taken: steps,
                duration_ms,
                from_prior_session: false,
            });
        }

        steps += 1;
        record_agent_progress(
            runtime,
            &agent_id,
            format!("step {steps}/{max_steps}: requesting model response"),
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

        // Race the API call against the cancellation token so a parent
        // cancel during a long thinking turn doesn't have to wait for the
        // step timeout.
        let response = tokio::select! {
            biased;
            () = runtime.cancel_token.cancelled() => {
                record_agent_progress(
                    runtime,
                    &agent_id,
                    format!("step {steps}/{max_steps}: cancelled mid-request"),
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
                    agent_type: agent_type.clone(),
                    assignment: assignment.clone(),
                    model: runtime.model.clone(),
                    nickname: None,
                    status,
                    result: None,
                    steps_taken: steps,
                    duration_ms,
                    from_prior_session: false,
                });
            }
            api = tokio::time::timeout(runtime.step_api_timeout, runtime.client.create_message(request)) => {
                api.map_err(|_| anyhow!("API call timed out after {}s", runtime.step_api_timeout.as_secs()))??
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
                format!("step {steps}/{max_steps}: {progress}"),
            );
            messages.push(Message {
                role: "user".to_string(),
                content: if tool_uses.is_empty() {
                    truncated_response_text_retry_message()
                } else {
                    truncated_response_tool_results(&tool_uses)
                },
            });
            continue;
        }
        reset_truncated_subagent_responses(&mut consecutive_truncated_responses);

        if tool_uses.is_empty() {
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
                    format!("step {steps}/{max_steps}: complete"),
                );
                break;
            }
            continue;
        }

        record_agent_progress(
            runtime,
            &agent_id,
            format!(
                "step {steps}/{max_steps}: executing {} tool call(s)",
                tool_uses.len()
            ),
        );
        let mut tool_results: Vec<ContentBlock> = Vec::new();
        for (tool_id, tool_name, tool_input) in tool_uses {
            record_agent_progress(
                runtime,
                &agent_id,
                format!("step {steps}/{max_steps}: running tool '{tool_name}'"),
            );
            if let Some(mb) = runtime.mailbox.as_ref() {
                let _ = mb.send(MailboxMessage::ToolCallStarted {
                    agent_id: agent_id.clone(),
                    tool_name: tool_name.clone(),
                    step: steps,
                });
            }
            let result = match tokio::time::timeout(TOOL_TIMEOUT, async {
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
            record_agent_progress(
                runtime,
                &agent_id,
                format!("step {steps}/{max_steps}: finished tool '{tool_name}'"),
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
        }
    }

    release_resident_leases_for(&agent_id);
    let status = SubAgentStatus::Completed;
    let duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
    insert_subagent_full_transcript_handle(
        runtime,
        &agent_id,
        &agent_type,
        &assignment,
        &status,
        final_result.as_ref(),
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
        agent_type,
        assignment,
        model: runtime.model.clone(),
        nickname: None,
        status,
        result: final_result,
        steps_taken: steps,
        duration_ms,
        from_prior_session: false,
    })
}

async fn wait_for_result(
    manager: &SharedSubAgentManager,
    agent_id: &str,
    timeout: Duration,
) -> Result<(SubAgentResult, bool), ToolError> {
    let deadline = Instant::now() + timeout;

    loop {
        let snapshot = {
            let manager = manager.read().await;
            manager
                .get_result(agent_id)
                .map_err(|e| ToolError::execution_failed(e.to_string()))?
        };

        if snapshot.status != SubAgentStatus::Running {
            return Ok((snapshot, false));
        }
        if Instant::now() >= deadline {
            return Ok((snapshot, true));
        }

        tokio::time::sleep(RESULT_POLL_INTERVAL).await;
    }
}

#[allow(dead_code)] // Legacy agent_wait helper; agent_eval uses wait_for_result.
async fn wait_for_agents(
    manager: &SharedSubAgentManager,
    ids: &[String],
    wait_mode: WaitMode,
    timeout: Duration,
) -> Result<(Vec<SubAgentResult>, bool), ToolError> {
    let deadline = Instant::now() + timeout;

    loop {
        let snapshots = {
            let manager = manager.read().await;
            ids.iter()
                .map(|id| {
                    manager
                        .get_result(id)
                        .map_err(|e| ToolError::execution_failed(e.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?
        };

        if wait_mode.condition_met(&snapshots) {
            return Ok((snapshots, false));
        }
        if Instant::now() >= deadline {
            return Ok((snapshots, true));
        }

        tokio::time::sleep(RESULT_POLL_INTERVAL).await;
    }
}

fn parse_wait_mode(input: &Value) -> Result<WaitMode, ToolError> {
    let raw_mode = input
        .get("wait_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("any");
    WaitMode::from_str(raw_mode).ok_or_else(|| {
        ToolError::invalid_input(format!("Invalid wait_mode '{raw_mode}'. Use: any or all"))
    })
}

fn parse_wait_ids(input: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    for key in ["ids", "agent_ids"] {
        if let Some(list) = input.get(key).and_then(|v| v.as_array()) {
            for value in list {
                if let Some(id) = value.as_str() {
                    let id = id.trim();
                    if !id.is_empty() && !ids.iter().any(|existing| existing == id) {
                        ids.push(id.to_string());
                    }
                }
            }
        }
    }

    for key in ["agent_id", "id"] {
        if let Some(id) = input.get(key).and_then(|v| v.as_str()) {
            let id = id.trim();
            if !id.is_empty() && !ids.iter().any(|existing| existing == id) {
                ids.push(id.to_string());
            }
        }
    }

    ids
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

fn parse_optional_text_or_items(
    input: &Value,
    text_keys: &[&str],
    items_key: &str,
) -> Result<Option<String>, ToolError> {
    let text = optional_input_str(input, text_keys).map(str::to_string);
    let items = parse_items_text(input, items_key)?;
    match (text, items) {
        (Some(_), Some(_)) => Err(ToolError::invalid_input(format!(
            "Provide either {} text or {}, but not both",
            text_keys[0], items_key
        ))),
        (Some(text), None) => Ok(Some(text)),
        (None, Some(items)) => Ok(Some(items)),
        (None, None) => Ok(None),
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
    let model = parse_optional_subagent_model(input, "model")?;
    let resident_file = input
        .get("resident_file")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty());
    let fork_context =
        parse_optional_bool(input, &["fork_context", "forkContext", "inherit_context"])
            .unwrap_or(false);
    let max_depth = input
        .get("max_depth")
        .or_else(|| input.get("maxDepth"))
        .or_else(|| input.get("max_spawn_depth"))
        .and_then(Value::as_u64)
        .map(|depth| {
            u32::try_from(depth)
                .map_err(|_| ToolError::invalid_input("max_depth must be between 0 and 3"))
                .and_then(|depth| {
                    if depth <= 3 {
                        Ok(depth)
                    } else {
                        Err(ToolError::invalid_input(
                            "max_depth must be between 0 and 3",
                        ))
                    }
                })
        })
        .transpose()?;

    Ok(SpawnRequest {
        session_name,
        prompt: prompt.clone(),
        agent_type,
        assignment: SubAgentAssignment::new(prompt, role),
        allowed_tools,
        model,
        cwd,
        resident_file,
        fork_context,
        max_depth,
    })
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
) -> Result<String, ToolError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ToolError::invalid_input(format!("{field} cannot be blank")));
    }
    crate::config::normalize_model_name(trimmed).ok_or_else(|| {
        ToolError::invalid_input(format!(
            "Invalid {field} '{trimmed}'. Expected a DeepSeek model id such as deepseek-v4-pro or deepseek-v4-flash"
        ))
    })
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
            return normalize_requested_subagent_model(model, &format!("subagents.{key}.model"))
                .map(Some);
        }
    }
    Ok(None)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubAgentResolvedRoute {
    pub(crate) model: String,
    pub(crate) reasoning_effort: Option<String>,
}

pub(crate) async fn resolve_subagent_assignment_route(
    runtime: &SubAgentRuntime,
    configured_model: Option<String>,
    prompt: &str,
    agent_type: &SubAgentType,
) -> SubAgentResolvedRoute {
    if matches!(agent_type, SubAgentType::ToolAgent) {
        return tool_agent_route(runtime);
    }

    let explicit_model = configured_model.is_some();
    let mut route = fallback_subagent_assignment_route(runtime, configured_model, prompt);

    if should_use_subagent_flash_router(runtime)
        && let Ok(Some(recommendation)) = subagent_flash_router(runtime, prompt).await
    {
        if runtime.auto_model && !explicit_model {
            route.model = recommendation.model;
        }
        if runtime.reasoning_effort_auto {
            route.reasoning_effort = recommendation
                .reasoning_effort
                .map(|effort| effort.as_setting().to_string())
                .or(route.reasoning_effort);
        }
    }

    route
}

fn tool_agent_route(runtime: &SubAgentRuntime) -> SubAgentResolvedRoute {
    SubAgentResolvedRoute {
        // Inherit the parent session's model instead of hardcoding a DeepSeek
        // model id: the tool-agent "fast lane" semantics come from
        // `reasoning_effort = off`, not from a specific model. Hardcoding
        // `deepseek-v4-flash` makes tool-agent routing fail with a 404 on any
        // backend or provider that doesn't serve that exact model id.
        model: runtime.model.clone(),
        reasoning_effort: Some("off".to_string()),
    }
}

fn should_use_subagent_flash_router(runtime: &SubAgentRuntime) -> bool {
    runtime.auto_model
}

fn fallback_subagent_assignment_route(
    runtime: &SubAgentRuntime,
    configured_model: Option<String>,
    prompt: &str,
) -> SubAgentResolvedRoute {
    let model = if let Some(model) = configured_model {
        model
    } else if runtime.auto_model {
        crate::commands::auto_model_heuristic(prompt, &runtime.model)
    } else {
        runtime.model.clone()
    };

    let reasoning_effort = if runtime.reasoning_effort_auto {
        let effort = match crate::auto_reasoning::select(false, prompt) {
            crate::tui::app::ReasoningEffort::Low | crate::tui::app::ReasoningEffort::Medium => {
                crate::tui::app::ReasoningEffort::High
            }
            other => other,
        };
        Some(effort.as_setting().to_string())
    } else {
        runtime.reasoning_effort.clone()
    };

    SubAgentResolvedRoute {
        model,
        reasoning_effort,
    }
}

async fn subagent_flash_router(
    runtime: &SubAgentRuntime,
    prompt: &str,
) -> Result<Option<crate::commands::AutoRouteRecommendation>> {
    if cfg!(test) {
        return Ok(None);
    }

    let request = MessageRequest {
        model: "deepseek-v4-flash".to_string(),
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: subagent_router_prompt(runtime, prompt),
                cache_control: None,
            }],
        }],
        max_tokens: 96,
        system: Some(SystemPrompt::Text(
            SUBAGENT_ROUTER_SYSTEM_PROMPT.to_string(),
        )),
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort: Some("off".to_string()),
        stream: Some(false),
        temperature: Some(0.0),
        top_p: None,
    };

    let response = tokio::time::timeout(
        Duration::from_secs(4),
        runtime.client.create_message(request),
    )
    .await??;
    Ok(crate::commands::parse_auto_route_recommendation(
        &message_response_text(&response.content),
    ))
}

const SUBAGENT_ROUTER_SYSTEM_PROMPT: &str = "\
You are the codewhale sub-agent routing manager. Return only compact JSON: \
{\"model\":\"deepseek-v4-flash|deepseek-v4-pro\",\"thinking\":\"off|high|max\"}. \
Treat each child assignment like a customer request entering a team queue: decide the least \
sufficient worker and thinking budget for that assignment. Do not treat being a sub-agent as \
important by itself. Use Flash for trivial, read-only, status, lookup, or single-step work. \
Use Pro for coding, debugging, release work, multi-file changes, security, architecture, \
high-risk decisions, ambiguous requests, or work likely to need tool-call judgment. Use thinking \
off for trivial no-tool work, high for ordinary reasoning, and max only for hard, risky, \
multi-step, uncertain, or tool-heavy work.";

fn subagent_router_prompt(runtime: &SubAgentRuntime, prompt: &str) -> String {
    format!(
        "Parent selected model mode: {}\nParent selected thinking mode: {}\n\nSub-agent assignment:\n{}\n\nReturn JSON only.",
        if runtime.auto_model { "auto" } else { "fixed" },
        if runtime.reasoning_effort_auto {
            "auto"
        } else {
            runtime
                .reasoning_effort
                .as_deref()
                .unwrap_or("provider-default")
        },
        truncate_subagent_router_prompt(prompt, 4_000)
    )
}

fn truncate_subagent_router_prompt(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = text.chars().take(max_chars).collect::<String>();
    out.push_str("\n[truncated]");
    out
}

fn message_response_text(blocks: &[ContentBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text, .. } => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
            ContentBlock::Thinking { thinking } => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(thinking);
            }
            _ => {}
        }
    }
    out
}

fn parse_optional_subagent_model(input: &Value, key: &str) -> Result<Option<String>, ToolError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => normalize_requested_subagent_model(value, key).map(Some),
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

fn parse_assign_request(input: &Value) -> Result<AssignRequest, ToolError> {
    let agent_id = input
        .get("agent_id")
        .or_else(|| input.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ToolError::missing_field("agent_id"))?
        .to_string();
    let objective = optional_input_str(input, &["objective"]).map(str::to_string);
    let role = optional_input_str(input, &["role", "agent_role"])
        .map(|role| {
            normalize_role_alias(role).ok_or_else(|| {
                ToolError::invalid_input(format!(
                    "Invalid role alias '{role}'. Use: {VALID_ROLE_ALIASES}"
                ))
            })
        })
        .transpose()?
        .map(str::to_string);
    let message = parse_optional_text_or_items(input, &["message", "input"], "items")?;
    let interrupt = optional_bool(input, "interrupt", true);

    if objective.is_none() && role.is_none() && message.is_none() {
        return Err(ToolError::invalid_input(
            "Provide at least one of objective, role/agent_role, message/input, or items"
                .to_string(),
        ));
    }

    Ok(AssignRequest {
        agent_id,
        objective,
        role,
        message,
        interrupt,
    })
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
        "tool-agent" | "tool_agent" | "toolagent" | "executor" | "execution" | "fin" => {
            Some("tool_agent")
        }
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

fn emit_agent_progress(
    event_tx: Option<&mpsc::Sender<Event>>,
    mailbox: Option<&Mailbox>,
    agent_id: &str,
    status: String,
) {
    if let Some(mb) = mailbox {
        let _ = mb.send(MailboxMessage::progress(agent_id, status.clone()));
    }
    if let Some(event_tx) = event_tx {
        let _ = event_tx.try_send(Event::AgentProgress {
            id: agent_id.to_string(),
            status,
        });
    }
}

// === Tool Registry Helpers ===

/// Per-sub-agent tool registry.
///
/// Two modes:
/// - **Full inheritance** (`allowed_tools = None`): the child sees the same
///   tool surface as the parent's Agent mode — every tool family including
///   `with_subagent_tools` (so it can recurse). Approval-gated tools are
///   callable only when the parent runtime is auto-approved or, for explicit
///   write-capable roles (`implementer`, `custom`), when the tool's approval
///   requirement is `Suggest`.
/// - **Explicit narrow** (`allowed_tools = Some(list)`): legacy / Custom
///   path. The registry still builds the full surface, but only the listed
///   tool names are visible to the model and callable.
struct SubAgentToolRegistry {
    /// `None` → full inheritance (no allowlist filter applied). `Some(list)` →
    /// only the listed tools are visible to the model and callable.
    allowed_tools: Option<Vec<String>>,
    auto_approve: bool,
    /// The role/type of the sub-agent that this registry belongs to. Used to
    /// decide whether `Suggest`-level tools (write/edit/patch) may run inside
    /// the child without the parent runtime being auto-approved (#1828, #1833).
    agent_type: SubAgentType,
    registry: ToolRegistry,
}

impl SubAgentToolRegistry {
    fn new(
        runtime: SubAgentRuntime,
        agent_type: SubAgentType,
        explicit_allowed_tools: Option<Vec<String>>,
        todo_list: SharedTodoList,
        plan_state: SharedPlanState,
    ) -> Self {
        // Build the full agent surface — same as the parent's Agent mode.
        // Children inherit shell, file, patch, search, web, git, diagnostics,
        // review, RLM, sub-agent management (so grandchildren can spawn),
        // plus per-child fresh todo/plan state.
        let context = runtime.context.clone();
        let mut registry = ToolRegistryBuilder::new().with_full_agent_surface(
            Some(runtime.client.clone()),
            runtime.model.clone(),
            runtime.manager.clone(),
            runtime.clone(),
            runtime.allow_shell,
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

    /// Whether a given tool name is permitted under this child's filter.
    /// `None` filter = everything permitted.
    fn is_tool_allowed(&self, name: &str) -> bool {
        match &self.allowed_tools {
            None => true,
            Some(list) => list.iter().any(|t| t == name),
        }
    }

    fn tools_for_model(&self, agent_type: &SubAgentType) -> Vec<Tool> {
        let disallowed = match agent_type {
            // Review and tool-executor agents should not spawn or manage
            // sub-agents recursively (#1489, fast-lane executor).
            SubAgentType::Review => &["agent_spawn", "agent_open", "agent_eval", "agent_close"][..],
            SubAgentType::ToolAgent => &[
                "agent_spawn",
                "agent_open",
                "agent_eval",
                "agent_close",
                "tool_agent",
                "rlm_open",
                "rlm_eval",
                "rlm_configure",
                "rlm_close",
            ][..],
            _ => &[][..],
        };
        let api_tools = self.registry.to_api_tools();
        let filtered = match &self.allowed_tools {
            None => api_tools,
            Some(list) => api_tools
                .into_iter()
                .filter(|tool| list.contains(&tool.name))
                .collect::<Vec<_>>(),
        };
        if disallowed.is_empty() {
            filtered
        } else {
            filtered
                .into_iter()
                .filter(|tool| !disallowed.contains(&tool.name.as_str()))
                .collect()
        }
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
                    if !Self::role_can_delegate_writes(&self.agent_type) {
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
        self.registry
            .execute(name, input)
            .await
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

/// When a child agent fails because its model is unavailable under the current
/// access profile, a bare provider 403/404 (classified `Authorization` or
/// `State`) is unactionable. Annotate it so the parent knows the likely cause
/// and how to recover (#2653) without re-classifying the underlying error.
fn annotate_child_model_error(err: &str, model: &str) -> String {
    match crate::error_taxonomy::classify_error_message(err) {
        crate::error_taxonomy::ErrorCategory::Authorization
        | crate::error_taxonomy::ErrorCategory::State => format!(
            "{err}\n(child model `{model}` may be unavailable under the current access profile — \
             retry agent_open with a different `model`, or remove `model` to inherit the parent's)"
        ),
        _ => err.to_string(),
    }
}

fn summarize_subagent_result(result: &SubAgentResult) -> String {
    match (&result.status, result.result.as_ref()) {
        (SubAgentStatus::Completed, Some(text)) => truncate_preview(text),
        (SubAgentStatus::Completed, None) => "Completed (no output)".to_string(),
        (SubAgentStatus::Interrupted(error), _) => format!("Interrupted: {error}"),
        (SubAgentStatus::Cancelled, _) => "Cancelled".to_string(),
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
    }
}

/// Max length of the human-friendly one-line summary emitted alongside the
/// completion sentinel. Longer results are clipped and the full transcript is
/// available via `agent_eval` (see `result_clipped` in the sentinel payload).
const SUBAGENT_SUMMARY_PREVIEW_MAX: usize = 240;

fn truncate_preview(text: &str) -> String {
    if text.len() <= SUBAGENT_SUMMARY_PREVIEW_MAX {
        text.to_string()
    } else {
        format!(
            "{}...",
            text.chars()
                .take(SUBAGENT_SUMMARY_PREVIEW_MAX)
                .collect::<String>()
        )
    }
}

/// Whether `summarize_subagent_result` clipped the preview, i.e. the parent's
/// previous-line summary is incomplete and the model should call `agent_eval`
/// for the full result rather than relying on the summary.
fn subagent_result_clipped(res: &SubAgentResult) -> bool {
    matches!(res.status, SubAgentStatus::Completed)
        && res
            .result
            .as_deref()
            .is_some_and(|text| text.len() > SUBAGENT_SUMMARY_PREVIEW_MAX)
}

const SUBAGENT_OUTPUT_FORMAT: &str = include_str!("../../prompts/subagent_output_format.md");

const GENERAL_AGENT_INTRO: &str = concat!(
    "You are a general-purpose sub-agent spawned to handle a specific task autonomously.\n",
    "Stay inside the assigned scope; put adjacent work under RISKS/BLOCKERS.\n",
    "Plan multi-step work with `checklist_write`; add `update_plan` for complex strategy.\n",
    "**Stop quickly on failure**: if the same tool call fails 2 times in a row, stop retrying and return what you have so far with a one-line note explaining what's missing. Do not loop on impossible queries (e.g. external API unreachable, rate-limited, or returning empty).\n",
    "**Bounded effort**: prefer one focused attempt over many speculative retries. If you cannot complete the task with available data within 3-5 tool calls, return your current partial findings — the parent agent can compensate with its own knowledge.\n\n"
);

const EXPLORE_AGENT_INTRO: &str = concat!(
    "You are an exploration sub-agent (role: `explore`). Map the relevant code quickly and stay read-only.\n",
    "Orient first: confirm the workspace/project root, read relevant AGENTS.md/README guidance when the tree is unfamiliar, then search only the likely scope.\n",
    "Use list_dir/file_search, grep_files, and read_file; use RLM only for long inputs or many semantic slices, not basic path discovery.\n",
    "DeepSeek V4 can hold broad evidence, but your value is compressed reconnaissance: cite `path:line-range` for each finding and stop once evidence is sufficient.\n",
    "CHANGES will almost always be \"None.\" for an explorer.\n\n"
);

const PLAN_AGENT_INTRO: &str = concat!(
    "You are a planning sub-agent. Produce a grounded, prioritized plan, not patches.\n",
    "Read enough code to avoid guessing; each step names its artifact and verification.\n",
    "Use update_plan/checklist_write for plan artifacts and explain key trade-offs.\n",
    "CHANGES should list plan artifacts only, not future speculative edits.\n\n"
);

const REVIEW_AGENT_INTRO: &str = concat!(
    "You are a code review sub-agent. Stay read-only and report severity-scored findings.\n",
    "Read the diff/files, grep sibling patterns/tests, then order EVIDENCE by severity.\n",
    "Use BLOCKER/MAJOR/MINOR/NIT and include path:line-range plus suggested fix.\n",
    "If no MAJOR+ issues exist, say so plainly in SUMMARY.\n",
    "CHANGES will almost always be \"None.\" for a reviewer.\n\n"
);

const CUSTOM_AGENT_INTRO: &str = concat!(
    "You are a custom sub-agent with a narrowed tool registry.\n",
    "Use only tools available at runtime; put missing capabilities under BLOCKERS and stop.\n",
    "Stay tightly scoped to the assigned objective.\n\n"
);

const IMPLEMENTER_AGENT_INTRO: &str = concat!(
    "You are an implementation sub-agent. Land the assigned change with minimal surrounding edits.\n",
    "Read target files before editing; prefer edit_file for narrow changes and apply_patch for hunks.\n",
    "Run relevant verification after edit batches; write needed tests with the implementation.\n",
    "CHANGES is load-bearing: list every modified file with a one-line why.\n\n"
);

const VERIFIER_AGENT_INTRO: &str = concat!(
    "You are a verification sub-agent. Run requested gates and stay read-only.\n",
    "Report PASS/FAIL/FLAKY at the top of SUMMARY with exact command evidence.\n",
    "Capture failing assertion and file:line; put obvious fixes under RISKS.\n",
    "CHANGES will almost always be \"None.\" for a verifier.\n\n"
);

const TOOL_AGENT_INTRO: &str = concat!(
    "You are a tool execution sub-agent (experimental Fin fast lane). You run simple tools quickly and report compact facts.\n",
    "The parent model owns planning, trade-offs, and synthesis; do not expand the task or narrate strategy.\n",
    "Prefer direct tool calls, concise evidence, and one-pass results. Stop after the requested machine-bound action is done.\n",
    "CHANGES should be \"None.\" unless an explicitly allowed tool made a real edit.\n\n"
);

// === Tests ===

#[cfg(test)]
mod tests;
