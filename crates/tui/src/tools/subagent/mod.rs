//! Sub-agent spawning system.
//!
//! Provides tools to spawn background sub-agents, query their status,
//! and retrieve results. Sub-agents run with a filtered toolset and
//! inherit the workspace configuration from the main session.
//!
//! The model-facing creation surface is the `agent` tool. Narrow coordination
//! tools (`agents/list`, `agents/message`, `agents/followup`,
//! `agents/interrupt`, `agents/coordinate`, `agents/wait`) wrap the same runtime without restoring
//! the retired lifecycle theater. Older manager helpers remain executable for
//! persisted records and internal recovery.

use std::collections::{BTreeSet, HashMap, VecDeque};
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
use crate::core::events::{AgentProgressEventMeta, Event};
use crate::dependencies::{ExternalTool, Git};
use crate::llm_client::{LlmClient, LlmError};
use crate::models::{
    ContentBlock, Message, MessageRequest, MessageResponse, SystemPrompt, Tool, Usage,
};
use crate::request_tuning::RequestTuning;
use crate::tools::canonical_action::{CANONICAL_ACTION_ALIASES, canonical_action_alias};
use crate::tools::handle::VarHandle;
use crate::tools::plan::{PlanState, SharedPlanState};
use crate::tools::registry::{AgentToolSurfaceOptions, ToolRegistry, ToolRegistryBuilder};
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
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

pub mod coord;
pub mod mailbox;

#[allow(unused_imports)] // re-exported for hosts / tests; registration uses concrete types
pub use coord::{
    AgentsCoordinateTool, AgentsFollowupTool, AgentsInterruptTool, AgentsListTool,
    AgentsMessageTool, AgentsWaitTool, CoordinationDetailProjection, register_coordination_tools,
};
#[allow(unused_imports)]
pub use mailbox::{Mailbox, MailboxEnvelope, MailboxMessage, MailboxReceiver};

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

/// Child model-turn budgets are finite by role; explicit spawn values are
/// clamped to the hard ceiling below.
const MAX_SUBAGENT_STEPS: u32 = 2_000;
/// Default wall-clock budget for one child run, including model and tool work.
const DEFAULT_CHILD_WALL_TIME: Duration = Duration::from_secs(30 * 60);
const MAX_CHILD_WALL_TIME: Duration = Duration::from_secs(24 * 60 * 60);
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
fn format_step_counter(steps: u32, max_steps: u32) -> String {
    format!("step {steps}/{max_steps}")
}

fn resolve_max_steps(role: SubAgentType, explicit: Option<u32>, configured: Option<u32>) -> u32 {
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
const SUBAGENT_TRANSCRIPT_ARTIFACT_SCHEMA_VERSION: u32 = 1;
const SUBAGENT_TRANSCRIPT_ARTIFACT_DIR: &str = "subagent-transcripts";
const SUBAGENT_STATE_SCHEMA_VERSION: u32 = 1;
const SUBAGENT_STATE_FILE: &str = "subagents.v1.json";
const SUBAGENT_STATE_LOCK_FILE: &str = "subagents.v1.lock";
const SUBAGENT_WORKTREE_ROOT_DIR: &str = ".codewhale-worktrees";
const SUBAGENT_RESTART_REASON: &str = "Interrupted by process restart";
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
/// English and Simplified-Chinese names are stored as adjacent pairs. Name
/// selection follows the active session locale; it never mixes languages in
/// one session. Smaller curated pools below cover every other shipped locale.
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

const WHALE_NICKNAMES_JA: &[&str] = &[
    "シロナガスクジラ",
    "ザトウクジラ",
    "マッコウクジラ",
    "ナガスクジラ",
    "イワシクジラ",
    "ミンククジラ",
    "コククジラ",
    "ホッキョククジラ",
    "シロイルカ",
    "イッカク",
    "シャチ",
    "ゴンドウクジラ",
];

const WHALE_NICKNAMES_ZH_HANT: &[&str] = &[
    "藍鯨",
    "座頭鯨",
    "抹香鯨",
    "長鬚鯨",
    "塞鯨",
    "布氏鯨",
    "小鬚鯨",
    "灰鯨",
    "弓頭鯨",
    "白鯨",
    "獨角鯨",
    "虎鯨",
];

const WHALE_NICKNAMES_PT_BR: &[&str] = &[
    "Azul",
    "Jubarte",
    "Cachalote",
    "Baleia-fin",
    "Baleia-sei",
    "Baleia-de-bryde",
    "Baleia-minke",
    "Cinzenta",
    "Baleia-franca",
    "Beluga",
    "Narval",
    "Orca",
];

const WHALE_NICKNAMES_ES_419: &[&str] = &[
    "Azul",
    "Jorobada",
    "Cachalote",
    "Rorcual común",
    "Rorcual sei",
    "Rorcual de Bryde",
    "Rorcual aliblanco",
    "Gris",
    "Ballena franca",
    "Beluga",
    "Narval",
    "Orca",
];

const WHALE_NICKNAMES_VI: &[&str] = &[
    "Cá voi xanh",
    "Cá voi lưng gù",
    "Cá nhà táng",
    "Cá voi vây",
    "Cá voi Sei",
    "Cá voi Bryde",
    "Cá voi Minke",
    "Cá voi xám",
    "Cá voi đầu cong",
    "Cá voi trắng",
    "Kỳ lân biển",
    "Cá voi sát thủ",
];

const WHALE_NICKNAMES_KO: &[&str] = &[
    "대왕고래",
    "혹등고래",
    "향유고래",
    "참고래",
    "보리고래",
    "브라이드고래",
    "밍크고래",
    "귀신고래",
    "북극고래",
    "흰고래",
    "외뿔고래",
    "범고래",
];

/// Return a deterministic whale name in the active UI locale.
#[must_use]
pub fn whale_name_for_id_in_locale(id: &str, locale_tag: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    let hash = hasher.finish() as usize;
    let normalized = locale_tag.trim().to_ascii_lowercase();

    let localized_pool = match normalized.as_str() {
        "ja" => Some(WHALE_NICKNAMES_JA),
        "zh-hant" => Some(WHALE_NICKNAMES_ZH_HANT),
        "pt-br" => Some(WHALE_NICKNAMES_PT_BR),
        "es-419" => Some(WHALE_NICKNAMES_ES_419),
        "vi" => Some(WHALE_NICKNAMES_VI),
        "ko" => Some(WHALE_NICKNAMES_KO),
        _ => None,
    };
    if let Some(pool) = localized_pool {
        return pool[hash % pool.len()].to_string();
    }

    debug_assert_eq!(WHALE_NICKNAMES.len() % 2, 0);
    let pair_count = WHALE_NICKNAMES.len() / 2;
    let pair = hash % pair_count;
    let language_offset = usize::from(normalized == "zh-hans");
    let idx = pair * 2 + language_offset;
    WHALE_NICKNAMES[idx].to_string()
}

/// Assign a unique locale-matched whale name for an agent ID.
/// If the deterministic name is taken, appends a numeric suffix (for example,
/// `Orca (2)`).
#[must_use]
pub fn assign_unique_whale_name_in_locale(
    id: &str,
    active_names: &std::collections::HashSet<String>,
    locale_tag: &str,
) -> String {
    let base = whale_name_for_id_in_locale(id, locale_tag);
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

/// Return the unsuffixed whale label when `name` could have been generated for
/// this exact agent id in a shipped locale. Numeric collision suffixes are
/// presentation-only and do not make the label user-authored.
fn generated_whale_name_base<'a>(agent_id: &str, name: &'a str) -> Option<&'a str> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let base = name
        .rsplit_once(" (")
        .and_then(|(base, suffix)| {
            suffix
                .strip_suffix(')')
                .filter(|number| !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit()))
                .map(|_| base)
        })
        .unwrap_or(name);

    // With no persisted provenance bit, the narrowest truthful test is whether
    // this exact agent id could have generated the label in a shipped locale.
    // A user-authored label that happens to be a whale word for some other id
    // remains explicit. An exact deterministic match is inherently ambiguous
    // and stays classified as generated for backward compatibility.
    crate::localization::Locale::shipped()
        .iter()
        .any(|locale| whale_name_for_id_in_locale(agent_id, locale.tag()) == base)
        .then_some(base)
}

/// Derive the generated whale labels shown for a set of workers from their
/// locale-neutral ids and the active UI language.
///
/// Persisted `nickname` values predate locale-scoped naming and may contain a
/// whale label chosen under another language. Those generated values are
/// deliberately ignored here. A nickname that this agent id could not have
/// generated is an explicit custom label and remains intact, even when it is a
/// whale word from a built-in pool.
#[must_use]
pub(crate) fn localized_whale_display_names<'a>(
    agents: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    locale_tag: &str,
) -> std::collections::HashMap<String, String> {
    let mut by_id = std::collections::BTreeMap::<String, Option<String>>::new();
    for (agent_id, nickname) in agents {
        if agent_id.trim().is_empty() {
            continue;
        }
        let nickname = nickname
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        by_id
            .entry(agent_id.to_string())
            .and_modify(|existing| {
                if existing.is_none() && nickname.is_some() {
                    *existing = nickname.clone();
                }
            })
            .or_insert(nickname);
    }

    let mut names = std::collections::HashMap::with_capacity(by_id.len());
    let mut active_names = std::collections::HashSet::new();

    // Reserve explicit labels first so generated names never shadow them.
    for (agent_id, nickname) in &by_id {
        let Some(nickname) = nickname
            .as_deref()
            .filter(|name| generated_whale_name_base(agent_id, name).is_none())
        else {
            continue;
        };
        active_names.insert(nickname.to_string());
        names.insert(agent_id.clone(), nickname.to_string());
    }

    // BTreeMap iteration makes collision suffix ownership stable even when
    // manager/progress event order changes between frames or session loads.
    for agent_id in by_id.keys() {
        if names.contains_key(agent_id) {
            continue;
        }
        let name = assign_unique_whale_name_in_locale(agent_id, &active_names, locale_tag);
        active_names.insert(name.clone());
        names.insert(agent_id.clone(), name);
    }

    names
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

#[derive(Clone)]
pub(crate) struct CoordinationRegistrationSnapshot {
    worker_records: HashMap<String, AgentWorkerRecord>,
    coordination: CoordinationLedger,
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
            description: "Open loads the complete private chat artifact; use the bounded transcript_handle with handle_read for slices and artifact metadata."
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
    agent_type: &SubAgentType,
    tool_profile: &AgentWorkerToolProfile,
    effective_model: &str,
    model_route: Option<ModelRoute>,
) -> WorkerRuntimeProfile {
    let mut requested = WorkerRuntimeProfile::for_role(agent_type.clone());
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
}

#[derive(Debug, Clone)]
pub(crate) struct WorkflowTaskSpawnMetadata {
    pub resolved_provider: String,
    pub resolved_model: String,
    pub route_source: String,
    /// Fleet role resolved for this spawn, if any (#4177).
    pub resolved_role: Option<String>,
    /// AgentProfile id resolved for this spawn, if any (#4177).
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

#[derive(Debug, Clone)]
struct SubAgentInput {
    text: String,
    interrupt: bool,
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
    agent_type: SubAgentType,
    /// True when the caller supplied `type`/`agent_type` or `role` explicitly
    /// (vs the `General` default). A fleet `profile` only sets the agent type
    /// when the caller did not, and conflicts are rejected only for explicit
    /// values.
    agent_type_explicit: bool,
    /// Optional Fleet roster member id (trimmed, lowercased). Resolved at
    /// spawn time against the runtime roster — parsing has no runtime access.
    profile: Option<String>,
    assignment: SubAgentAssignment,
    allowed_tools: Option<Vec<String>>,
    model: Option<String>,
    model_strength: SubAgentModelStrength,
    /// True when the caller supplied `model_strength` explicitly. An explicit
    /// strength outranks a fleet profile's model pin/loadout; the parse-time
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
    /// Expected mutation boundary. Write-capable launches must declare at
    /// least one bounded root, exact file, or named contract.
    write_roots: Vec<String>,
    exact_files: Vec<String>,
    coordination_contracts: Vec<String>,
}

/// Declared child write authority for a (deliberate) spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpawnWriteAuthority {
    ReadOnly,
    WorkspaceWrite,
    WorktreeWrite,
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
    /// The completing child's agent id. Held for routing/logging — the
    /// engine's turn loop does not currently key on it (it just injects
    /// the payload), but downstream tooling and tests need the field.
    #[allow(dead_code)]
    pub agent_id: String,
    /// Human summary on line 1, sentinel on line 2. Same payload shape as
    /// `Event::AgentComplete::result`.
    pub payload: String,
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
}

impl SubAgentTerminalDeliveryContext {
    fn from_runtime(runtime: &SubAgentRuntime) -> Self {
        Self {
            spawn_depth: runtime.spawn_depth,
            parent_completion_tx: runtime.parent_completion_tx.clone(),
            mailbox: runtime.mailbox.clone(),
            event_tx: runtime.event_tx.clone(),
        }
    }

    /// Publish to every live sink without blocking or awaiting while the
    /// manager owns the terminal claim. The public agent/worker states remain
    /// Running until all three sends have been attempted.
    fn deliver(&self, result: &SubAgentResult) {
        let completion = subagent_completion_from_result(result);

        if self.spawn_depth > 0
            && let Some(tx) = self.parent_completion_tx.as_ref()
        {
            let _ = tx.send(completion.clone());
        }

        if let Some(mailbox) = self.mailbox.as_ref() {
            let _ = mailbox.send(terminal_mailbox_message(result));
        }

        if let Some(event_tx) = self.event_tx.as_ref() {
            let _ = event_tx.try_send(Event::AgentComplete {
                id: result.agent_id.clone(),
                result: completion.payload,
            });
        }
    }
}

fn terminal_mailbox_message(result: &SubAgentResult) -> MailboxMessage {
    match &result.status {
        SubAgentStatus::Completed => {
            let (summary, _) = stamp_subagent_summary(&summarize_subagent_result(result));
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
    /// Session `Config` snapshot, used to build a *fresh* LLM client bound to a
    /// different provider when a fleet roster member's profile pins one (#4193,
    /// the interactive-TUI twin of the headless `codewhale exec --provider`
    /// route from #4181). The engine threads it in via
    /// [`SubAgentRuntime::with_api_config`]; `child_runtime`/`background_runtime`
    /// clone the `Arc` so every descendant can re-derive a provider-B client.
    ///
    /// `None` for legacy/test runtimes that never threaded a config. When a
    /// profile pins a provider different from the session's and this is `None`
    /// (or the pinned provider's credentials cannot be resolved), the spawn
    /// FAILS rather than silently reusing the session client — a silent reuse
    /// would send model B's id to provider A's endpoint, the exact #4093 defect.
    pub api_config: Option<std::sync::Arc<crate::config::Config>>,
    pub model: String,
    /// Active UI/model locale used for generated human-facing worker names.
    /// Internal ids and session handles remain language-neutral.
    pub locale_tag: String,
    pub auto_model: bool,
    pub reasoning_effort: Option<String>,
    pub reasoning_effort_auto: bool,
    pub role_models: HashMap<String, String>,
    /// Shared fleet roster of named agent roles (#fleet-roster cutover
    /// (v0.8.67)). Built-ins only by default; the engine installs the merged
    /// built-in/config/workspace roster so model-spawned sub-agents and fleet
    /// dispatch resolve the same party. Cloned into child runtimes.
    pub fleet_roster: std::sync::Arc<crate::fleet::roster::FleetRoster>,
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
    /// Session mode of the orchestrating parent at spawn time (Wave 7 M4/M5).
    pub parent_mode: AppMode,
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
            fleet_roster: std::sync::Arc::new(crate::fleet::roster::FleetRoster::built_ins_only()),
            context,
            allow_shell,
            accept_edits: false,
            accept_verification: false,
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
            parent_mode: AppMode::Agent,
        }
    }

    /// Preserve the parent session mode for spawn-policy decisions.
    #[must_use]
    pub fn with_parent_mode(mut self, mode: AppMode) -> Self {
        self.parent_mode = mode;
        self
    }

    /// Match generated worker display names to the active session language.
    #[must_use]
    pub fn with_locale_tag(mut self, locale_tag: impl Into<String>) -> Self {
        self.locale_tag = locale_tag.into();
        self
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

    /// Attach the session `Config` so a spawn can build a fresh LLM client for a
    /// fleet profile's pinned provider (#4193). Without it, cross-provider
    /// in-process spawns fail closed rather than misrouting (see the
    /// [`api_config`](Self::api_config) field docs). Engine-only wiring; test
    /// and legacy runtimes may leave it unset.
    #[must_use]
    pub fn with_api_config(mut self, config: crate::config::Config) -> Self {
        self.api_config = Some(std::sync::Arc::new(config));
        self
    }

    /// Build an LLM client bound to `provider_id` from the threaded session
    /// `Config` (#4193). Mirrors the proven per-provider client factory used by
    /// per-turn auto-routing (`model_routing`) and the engine's provider switch:
    /// clone the session config, override only its `provider`, and let
    /// [`DeepSeekClient::new`] re-resolve that provider's base URL + credentials
    /// from config/env. `provider_id` may be a built-in provider id or a
    /// user-named `[providers.<id>] kind="openai-compatible"` custom provider
    /// such as `lm-studio` (#3965).
    ///
    /// Returns `Err` when no config was threaded in, or when the provider's
    /// credentials/base URL cannot be resolved. Callers MUST surface that error
    /// rather than fall back to the session client: a silent fallback would send
    /// the pinned model id to the session provider's endpoint (#4093).
    fn scoped_config_for_provider_id(
        &self,
        provider_id: &str,
    ) -> Result<(crate::config::Config, crate::config::ProviderIdentity), String> {
        let Some(api_config) = self.api_config.as_ref() else {
            return Err(
                "session Config was not threaded into this runtime; cannot build a \
                 provider-pinned client"
                    .to_string(),
            );
        };
        let provider_id = provider_id.trim();
        if provider_id.is_empty() {
            return Err("provider pin was blank".to_string());
        }
        let identity = api_config.resolve_provider_identity(provider_id)?;
        let mut provider_config = (**api_config).clone();
        // EPIC #2608: the provider is taken verbatim from the profile pin
        // (built-in id or configured custom id), never inferred from the model
        // id. Overriding only `provider` makes `Config::api_provider`,
        // `deepseek_base_url`, and `deepseek_api_key` all re-resolve for the
        // pinned provider.
        provider_config.provider = Some(identity.key.clone());
        Ok((provider_config, identity))
    }

    /// Install the merged fleet roster (#fleet-roster cutover (v0.8.67)).
    /// The engine builds it once per session config; children inherit it.
    #[must_use]
    pub fn with_fleet_roster(
        mut self,
        roster: std::sync::Arc<crate::fleet::roster::FleetRoster>,
    ) -> Self {
        self.fleet_roster = roster;
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
            api_config: self.api_config.clone(),
            model: self.model.clone(),
            locale_tag: self.locale_tag.clone(),
            auto_model: self.auto_model,
            reasoning_effort: self.reasoning_effort.clone(),
            reasoning_effort_auto: self.reasoning_effort_auto,
            role_models: self.role_models.clone(),
            fleet_roster: self.fleet_roster.clone(),
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
            cancel_token: self.cancel_token.child_token(),
            mailbox: self.mailbox.clone(),
            parent_completion_tx: self.parent_completion_tx.clone(),
            fork_context: self.fork_context.clone(),
            mcp_pool: self.mcp_pool.clone(),
            step_api_timeout: self.step_api_timeout,
            tool_timeout: self.tool_timeout,
            speech_output_dir: self.speech_output_dir.clone(),
            todos: self.todos.clone(),
            parent_mode: self.parent_mode,
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
struct CoordinationProcessLock {
    release: Option<std::sync::mpsc::SyncSender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CoordinationProcessLock {
    fn acquire(workspace: &Path) -> Result<Self> {
        let lock_path = normalize_subagent_workspace(workspace)
            .join(".codewhale")
            .join("state")
            .join(SUBAGENT_STATE_LOCK_FILE);
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent)?;
        }
        reject_workspace_relative_symlinks(&normalize_subagent_workspace(workspace), &lock_path)?;
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let thread = std::thread::spawn(move || {
            let mut lock = fd_lock::RwLock::new(lock_file);
            match lock.try_write() {
                Ok(_guard) => {
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
                Err(anyhow!(
                    "another Codewhale process owns delegated coordination for {}: {error}",
                    workspace.display()
                ))
            }
            Err(error) => {
                drop(release_tx);
                let _ = thread.join();
                Err(anyhow!(
                    "timed out acquiring delegated coordination lock for {}: {error}",
                    workspace.display()
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
    state_path: Option<PathBuf>,
    coordination_process_lock: Option<CoordinationProcessLock>,
    coordination_process_lock_error: Option<String>,
    coordination_process_lock_required: bool,
    max_steps: Option<u32>,
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
    /// Test/observability: agent ids that received a live wake via followup.
    woken_agents: HashMap<String, bool>,
}

impl SubAgentManager {
    /// Create a new manager for sub-agents.
    #[must_use]
    pub fn new(workspace: PathBuf, max_agents: usize) -> Self {
        Self {
            agents: HashMap::new(),
            worker_records: HashMap::new(),
            worker_event_seq: 0,
            persist_sequence: std::sync::atomic::AtomicU64::new(0),
            coordination: CoordinationLedger::default(),
            workspace,
            state_path: None,
            coordination_process_lock: None,
            coordination_process_lock_error: None,
            coordination_process_lock_required: false,
            max_steps: None,
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
            woken_agents: HashMap::new(),
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

    pub fn record_coordination_decision(
        &mut self,
        decision: DecisionRecord,
    ) -> Result<DecisionRecord, String> {
        self.ensure_coordination_process_lock()?;
        self.coordination.record_decision(decision)
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
            SubAgentType::Review,
            "Reviewer",
        )?;
        let verifier_ids = self.validate_reconciliation_role_evidence(
            &verifier_evidence_handles,
            SubAgentType::Verifier,
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
        expected: SubAgentType,
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
                        || (expected == SubAgentType::Review
                            && role.trim().eq_ignore_ascii_case("reviewer"))
                        || (expected == SubAgentType::Verifier
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
                record.spec.agent_type == SubAgentType::Plan
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
        }
    }

    #[must_use]
    pub fn inspect_coordination(&self, subject: Option<&str>, limit: usize) -> Value {
        serde_json::to_value(self.coordination_detail_projection(subject, limit))
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
                "write '{path}' is outside agent '{owner}' scope (roots: {:?}, files: {:?}); expand it first with agents/coordinate action=claim",
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
        match CoordinationProcessLock::acquire(&self.workspace) {
            Ok(lock) => {
                self.coordination_process_lock = Some(lock);
                self.coordination_process_lock_error = None;
            }
            Err(error) => {
                self.coordination_process_lock = None;
                self.coordination_process_lock_error = Some(error.to_string());
            }
        }
        self
    }

    fn ensure_coordination_process_lock(&self) -> Result<(), String> {
        if !self.coordination_process_lock_required || self.coordination_process_lock.is_some() {
            return Ok(());
        }
        Err(self
            .coordination_process_lock_error
            .clone()
            .unwrap_or_else(|| {
                "delegated coordination requires one manager process per workspace".to_string()
            }))
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

    fn persist_state_synchronously(&self) -> Result<()> {
        self.ensure_coordination_process_lock()
            .map_err(anyhow::Error::msg)?;
        let Some((path, payload)) = self.build_persist_payload()? else {
            return Ok(());
        };
        write_json_atomic(&self.workspace, &path, &payload)
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
            tracing::warn!(target: "subagent", %error, "skipping persist without workspace coordination lock");
            return;
        }
        if self.persist_pending {
            self.last_persist_at = Some(Instant::now());
            self.persist_pending = false;
            // Synchronous disk I/O — safe because we are shutting down and no
            // callers depend on releasing the write lock quickly.
            if let Ok(Some((path, payload))) = self.build_persist_payload()
                && let Err(err) = write_json_atomic(&self.workspace, &path, &payload)
            {
                tracing::warn!(target: "subagent", ?err, "failed to flush pending sub-agent state");
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

    /// Validate a Fleet/headless worker's persisted launch claim before its
    /// durable task lease is committed. A contention is still receipted in the
    /// live coordination ledger, but a successful preflight does not reserve
    /// anything until `register_worker_with_coordination` runs under the same
    /// manager write lock.
    pub fn preflight_worker_coordination(&mut self, spec: &AgentWorkerSpec) -> Result<(), String> {
        self.ensure_coordination_process_lock()?;
        let Some((claim, isolated_worktree)) = worker_coordination_claim(spec)? else {
            return Ok(());
        };
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
        self.ensure_coordination_process_lock()?;
        let previous_worker_records = self.worker_records.clone();
        let previous_coordination = self.coordination.clone();
        let claim = worker_coordination_claim(&spec)?;
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
        self.register_worker(spec);
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

    fn active_coordination_owners(&self) -> std::collections::HashSet<String> {
        self.agents
            .iter()
            .filter(|(_, agent)| agent.status == SubAgentStatus::Running)
            .map(|(id, _)| id.clone())
            .chain(
                self.worker_records
                    .iter()
                    .filter(|(_, record)| !record.status.is_terminal())
                    .map(|(id, _)| id.clone()),
            )
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

    #[cfg(test)]
    pub(crate) fn coordination_snapshot(&self) -> CoordinationLedger {
        self.coordination.clone()
    }

    pub fn get_worker_record(&self, worker_id: &str) -> Option<AgentWorkerRecord> {
        self.worker_records.get(worker_id).cloned()
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

    /// Queue mail and attempt a live wake (`agents/followup`).
    pub fn followup_child(&mut self, agent_ref: &str, text: String) -> Result<ParentMailReceipt> {
        let mut receipt = self.queue_parent_message(agent_ref, text.clone(), true)?;
        let agent_id = receipt.agent_id.clone();
        let status = self
            .agents
            .get(&agent_id)
            .map(|agent| agent.status.clone())
            .ok_or_else(|| anyhow!("Agent {agent_id} not found"))?;
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
                if let Some(agent) = self.agents.get_mut(&agent_id)
                    && let Some(tx) = agent.input_tx.as_ref()
                {
                    for mail in pending {
                        let _ = tx.send(SubAgentInput {
                            text: mail.text,
                            interrupt: false,
                        });
                    }
                }
                self.woken_agents.insert(agent_id.clone(), true);
                receipt.woke = true;
                receipt.queue_depth = 0;
                receipt.continuation_handle = None;
                receipt.note = "queued and delivered to running child".to_string();
                if let Some(record) = self.worker_records.get_mut(&agent_id) {
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
                // Honest gap: checkpoints are preserved and the continuation
                // handle is returned, but there is no in-process
                // `run_subagent_from_checkpoint` substrate yet. Auto-resume
                // would require re-spawning with seeded checkpoint messages
                // (new agent loop + runtime client), not just waking input_tx.
                receipt.woke = false;
                receipt.continued_from_checkpoint = false;
                receipt.continuation_handle = continuation_handle.clone();
                receipt.note = if continuable {
                    format!(
                        "queued; child is interrupted_continuable — live checkpoint resume is not automated (no run_subagent_from_checkpoint substrate). Re-dispatch via agent using continuation_handle={}",
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

    /// Bounded coordination summaries for `agents/list`.
    pub fn list_coordination_summaries(
        &self,
        include_archived: bool,
        recent_limit: usize,
    ) -> Vec<AgentCoordSummary> {
        self.list_filtered(include_archived)
            .into_iter()
            .filter_map(|snap| {
                self.coordination_summary_for(&snap.agent_id, recent_limit)
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
        let agent_id = format!("agent_{name}");
        let (input_tx, _input_rx) = mpsc::unbounded_channel();
        let mut agent = SubAgent::new(
            agent_id.clone(),
            SubAgentType::General,
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
        self.agents.insert(agent_id.clone(), agent);
        let spec = AgentWorkerSpec {
            worker_id: agent_id.clone(),
            run_id: agent_id.clone(),
            parent_run_id: Some("parent_session".to_string()),
            session_name: Some(name.to_string()),
            objective: "test".to_string(),
            role: None,
            agent_type: SubAgentType::General,
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
            launch_manifest: None,
        };
        self.register_worker(spec);
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
        let write_capable = runtime_profile.permissions.write;
        if write_capable {
            self.ensure_coordination_process_lock()
                .map_err(anyhow::Error::msg)?;
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
                    let claim = self.namespace_write_claim(
                        &agent.workspace,
                        options.isolated_worktree,
                        claim,
                    )?;
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
                "\n\nWrite scope (enforced; coordination-root-relative): roots={:?}; exact_files={:?}; contracts={:?}. Expand it with agents/coordinate action=claim before mutating anything outside this scope.",
                claim.roots, claim.exact_files, claim.contracts
            ));
            agent.prompt = prompt.clone();
        }
        let max_steps = resolve_max_steps(agent_type.clone(), options.max_steps, self.max_steps);
        runtime.worker_profile.max_steps = max_steps;
        let wall_time = options
            .wall_time
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
        agent.terminal_delivery = Some(SubAgentTerminalDeliveryContext::from_runtime(&runtime));
        self.register_worker(worker_spec);
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
            wall_time,
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
        self.record_worker_event(
            &agent_id,
            AgentWorkerStatus::Running,
            Some("running".to_string()),
            None,
            None,
        );
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
        let before_workers = self.worker_records.len();
        let mut transcript_candidates: Vec<String> = self
            .agents
            .keys()
            .chain(self.worker_records.keys())
            .cloned()
            .collect();
        transcript_candidates.sort();
        transcript_candidates.dedup();
        let mut auto_cancelled = 0;
        let timeout = self.running_heartbeat_timeout;
        let stale_agent_ids = self
            .agents
            .values()
            .filter(|agent| {
                agent.status == SubAgentStatus::Running
                    && !agent.completion_claimed
                    && agent.task_handle.is_some()
                    && agent.last_activity_at.elapsed() >= timeout
            })
            .map(|agent| agent.id.clone())
            .collect::<Vec<_>>();
        for agent_id in stale_agent_ids {
            if let Some(agent) = self.agents.get(&agent_id) {
                tracing::warn!(
                    target: "subagent",
                    agent_id = %agent.id,
                    timeout_secs = timeout.as_secs(),
                    "auto-cancelling stale sub-agent with no manager-visible progress"
                );
            }
            let Some(mut terminal) = self.agents.get(&agent_id).map(SubAgent::snapshot) else {
                continue;
            };
            terminal.status = SubAgentStatus::Cancelled;
            terminal.result = Some(format!(
                "Auto-cancelled after {}s without sub-agent progress.",
                timeout.as_secs()
            ));
            terminal.needs_input = None;
            // Cleanup batches stale transitions and persists the final fleet
            // snapshot once below. Spawning one unordered background write
            // per child could let an earlier partial snapshot rename last and
            // resurrect a cancelled worker after restart.
            if self.finish_terminal_result(&agent_id, terminal, true, false) {
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
        // #4217: age-evict terminal worker ledger entries. Agents already drop
        // after `max_age`, but worker_records previously only had an LRU cap of
        // 256 — long-lived sessions rewrote multi-MB subagents.v1.json forever.
        // Running / starting / waiting records are always preserved.
        let now_ms = epoch_millis_now();
        let max_age_ms = max_age.as_millis() as u64;
        self.worker_records.retain(|_, record| {
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
        for agent_id in transcript_candidates {
            if self.agents.contains_key(&agent_id) || self.worker_records.contains_key(&agent_id) {
                continue;
            }
            if let Err(err) = remove_subagent_transcript_artifact(&self.workspace, &agent_id) {
                tracing::warn!(
                    target: "subagent",
                    ?err,
                    agent_id,
                    "failed to remove expired sub-agent transcript artifact"
                );
            }
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
        result: SubAgentResult,
        abort_task: bool,
        persist_after_commit: bool,
    ) -> bool {
        if result.status == SubAgentStatus::Running || result.agent_id != agent_id {
            return false;
        }
        if !self.claim_terminal_delivery(agent_id) {
            return false;
        }

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

/// Append-only, per-run backing store for the worker's complete structured
/// message stream. The in-memory `full_transcript` handle deliberately keeps a
/// bounded tail; this artifact is the durable source used by the TUI's Open
/// action when the conversation is larger than that tail.
struct SubAgentTranscriptArtifactWriter {
    workspace: PathBuf,
    path: PathBuf,
    relative_path: PathBuf,
    persisted_messages: usize,
}

impl SubAgentTranscriptArtifactWriter {
    async fn for_runtime(runtime: &SubAgentRuntime, agent_id: &str) -> Result<Self> {
        let workspace = runtime.manager.read().await.workspace.clone();
        Self::create(&workspace, agent_id)
    }

    fn create(workspace: &Path, agent_id: &str) -> Result<Self> {
        let workspace = normalize_subagent_workspace(workspace);
        let relative_path = subagent_transcript_artifact_relative_path(agent_id);
        let path = checked_subagent_transcript_artifact_path(&workspace, agent_id)?;
        let header = json!({
            "kind": "subagent_transcript_header",
            "schema_version": SUBAGENT_TRANSCRIPT_ARTIFACT_SCHEMA_VERSION,
            "agent_id": agent_id,
        });
        create_private_subagent_transcript(&workspace, &path, &json_line(&header)?)?;
        Ok(Self {
            workspace,
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
            append_private_subagent_transcript(&self.workspace, &self.path, &encoded, durable)?;
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

fn checked_subagent_transcript_artifact_path(workspace: &Path, agent_id: &str) -> Result<PathBuf> {
    checked_subagent_state_path(
        workspace,
        &subagent_transcript_artifact_relative_path(agent_id),
    )
}

/// Read the complete structured worker chat for the TUI Open action. The path
/// is derived from `agent_id` rather than accepted from handle JSON, so a
/// corrupted or model-supplied payload cannot redirect the reader outside the
/// manager workspace.
pub(crate) fn load_subagent_transcript_artifact(
    workspace: &Path,
    agent_id: &str,
) -> Result<Vec<Message>> {
    let workspace = normalize_subagent_workspace(workspace);
    let path = checked_subagent_transcript_artifact_path(&workspace, agent_id)?;
    let raw = read_subagent_state_file(&workspace, &path)?;
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

fn remove_subagent_transcript_artifact(workspace: &Path, agent_id: &str) -> Result<bool> {
    let workspace = normalize_subagent_workspace(workspace);
    let path = checked_subagent_transcript_artifact_path(&workspace, agent_id)?;
    reject_workspace_relative_symlinks(&workspace, &path)?;
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
    workspace: &Path,
    agent_id: &str,
    messages: &[Message],
) -> Result<PathBuf> {
    let mut writer = SubAgentTranscriptArtifactWriter::create(workspace, agent_id)?;
    writer.sync_messages(messages, true)?;
    Ok(writer.path)
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

fn prepare_subagent_transcript_parent(workspace: &Path, path: &Path) -> Result<()> {
    reject_workspace_relative_symlinks(workspace, path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("sub-agent transcript artifact must have a parent directory"))?;
    fs::create_dir_all(parent)?;
    // Re-check after creation so a pre-existing component cannot redirect the
    // private transcript outside the workspace.
    reject_workspace_relative_symlinks(workspace, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn create_private_subagent_transcript(workspace: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    prepare_subagent_transcript_parent(workspace, path)?;
    let mut file = open_private_subagent_transcript(path, false)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn append_private_subagent_transcript(
    workspace: &Path,
    path: &Path,
    bytes: &[u8],
    durable: bool,
) -> Result<()> {
    reject_workspace_relative_symlinks(workspace, path)?;
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

fn epoch_millis_now() -> u64 {
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

fn write_json_atomic(workspace: &Path, path: &Path, value: &PersistedSubAgentState) -> Result<()> {
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
    manager = manager.require_coordination_process_lock();
    if let Err(error) = manager.ensure_coordination_process_lock() {
        tracing::warn!(target: "subagent", %error, "delegated coordination unavailable in this process");
    } else if let Err(err) = manager.load_state() {
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
    Status,
    Peek,
    Wait,
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
        "wait" | "join" | "await" | "block" => Ok(AgentToolAction::Wait),
        "cancel" | "stop" | "abort" => Ok(AgentToolAction::Cancel),
        other => Err(ToolError::invalid_input(format!(
            "Invalid agent action '{other}'. Use start, status, peek, wait, or cancel."
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
            "Start one focused background worker and return immediately with its agent_id; a prompt is enough for a read-only role. ",
            "Use multiple starts for independent parallel tasks. Add a Fleet profile, role, worktree, or explicit limits only when they improve the task. ",
            "Coordinate later with agents/list, agents/message, agents/followup, agents/interrupt, or agents/wait instead of polling. ",
            "In Operate, a write-capable root start also declares bounded write_roots, exact_files, or coordination_contracts; arbitrary shell remains gated. ",
            "Legacy action=status|peek|wait|cancel remain for compatibility."
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["start", "status", "peek", "wait", "cancel"],
                    "description": "start (default) launches a background worker and returns immediately. status lists current children or inspects agent_id. peek is status for one child. wait blocks until a running child settles (agent_id for one specific child, otherwise the next completion). cancel stops a running child by agent_id."
                },
                "agent_id": {
                    "type": "string",
                    "description": "Agent id or session name for action=status, action=peek, action=wait, or action=cancel."
                },
                "timeout_secs": {
                    "type": "integer",
                    "minimum": 5,
                    "maximum": 1800,
                    "description": "For action=wait: maximum seconds to block before returning a still-running snapshot. Default 300."
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
                    "description": "The focused task to give the background worker. A read-only role needs no write scope; a write-capable role must also declare a bounded write scope."
                },
                "dependencies": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Bounded prerequisite facts or task ids relevant to this child. Raw parent reasoning and transcript text do not belong here."
                },
                "acceptance": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Bounded observable checks this child must satisfy before completion."
                },
                "type": {
                    "type": "string",
                    "description": SUBAGENT_TYPE_DESCRIPTION
                },
                "profile": {
                    "type": "string",
                    "description": "Optional Fleet roster member to run this child as (e.g. reviewer, scout, builder, verifier, synthesizer, manager, or a custom member from project .codewhale/agents/, personal $CODEWHALE_HOME/agents/, or [fleet.profiles] config). The member supplies role posture, model routing, instruction overlay, and delegation bounds; explicit type/model/model_strength/max_depth here override the member's defaults. See /fleet."
                },
                "model_strength": {
                    "type": "string",
                    "enum": ["same", "faster"],
                    "description": "Optional child model strength. Children inherit the active model by default. Choose faster explicitly for read-only lookup/search, status, or other low-risk tasks that can use the configured fast sibling. The run receipt is authoritative for the resolved route; no hidden auto-downgrade happens."
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
                    "description": "Unset (default): auto — a read-only child (explore/plan/review/verifier or read_only write authority) running the parent's exact model route in the parent workspace forks the parent's cached context prefix; anything else starts fresh. true: force the parent prefix (cheap only on the parent's model route). false: force a fresh isolated context."
                },
                "max_depth": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 3,
                    "description": "Optional remaining nested-agent depth budget for this child. Defaults to the configured runtime budget."
                },
                "max_steps": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 2000,
                    "description": "Optional child model-turn budget. Defaults by role (60 for explore/review/plan/verifier, 120 for implementer/general/custom) and is clamped to 2000."
                },
                "wall_time_secs": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 86400,
                    "description": "Optional child wall-clock budget in seconds. Default 1800; clamped to 86400."
                },
                "workspace_policy": {
                    "type": "string",
                    "enum": ["shared", "worktree"],
                    "description": "Workspace isolation policy — enforced. worktree creates a fresh git worktree for the child; shared runs in the parent checkout and conflicts with worktree options."
                },
                "expected_artifact": {
                    "type": "string",
                    "description": "What the child should return (summary, patch path, test report, review findings, …). Appended to the child's prompt so the contract is visible to it."
                },
                "write_authority": {
                    "type": "string",
                    "enum": ["read_only", "workspace_write", "worktree_write"],
                    "description": "Write authority for the child — enforced. read_only removes write permission from the child's runtime profile (and its descendants); worktree_write requires worktree isolation."
                },
                "write_roots": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Expected repo-relative directory trees this child may mutate. Shared write-capable children claim these before launch; scope expansion must use agents/coordinate before mutation."
                },
                "exact_files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Expected repo-relative individual files this child may mutate."
                },
                "coordination_contracts": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Named shared contracts or schemas this child owns while active; equal names contend even when file paths differ."
                },
                "deliberate": {
                    "type": "boolean",
                    "description": "When true, require type (or profile), workspace_policy, expected_artifact, and write_authority."
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
    /// #4097: wait passively observes children — also read-only.
    fn approval_requirement_for(&self, input: &Value) -> ApprovalRequirement {
        match parse_agent_tool_action(input) {
            Ok(AgentToolAction::Status | AgentToolAction::Peek | AgentToolAction::Wait) => {
                ApprovalRequirement::Auto
            }
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

    /// #3801: status/peek actions are read-only queries of manager state.
    /// #4097: wait only observes child lifecycle — read-only as well.
    fn is_read_only_for(&self, input: &Value) -> bool {
        matches!(
            parse_agent_tool_action(input),
            Ok(AgentToolAction::Status | AgentToolAction::Peek | AgentToolAction::Wait)
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
                    Some(&self.inspect_memo),
                )
                .await;
            }
            AgentToolAction::Wait => {
                return wait_for_subagents_from_input(&input, self.manager.clone(), context).await;
            }
            AgentToolAction::Cancel => {
                return cancel_agent_from_input(&input, self.manager.clone(), context).await;
            }
        }
        let (snapshot, _) =
            spawn_subagent_from_input(input, self.manager.clone(), self.runtime.clone()).await?;
        let worker_record = {
            let manager = self.manager.read().await;
            manager.get_worker_record(&snapshot.agent_id)
        };
        let projection = subagent_session_projection(snapshot, false, context, worker_record).await;
        let mut tool_result = ToolResult::json(&projection)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        let metadata = json!({
            "action": "start",
            "agent_id": projection.agent_id,
            "status": projection.status,
            "terminal": projection.terminal,
            "context_mode": projection.context_mode,
            "prefix_cache": projection.prefix_cache,
        });
        tool_result.metadata = Some(metadata);
        Ok(tool_result)
    }
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
                let payload = json!({
                    "action": if peek { "peek" } else { "status" },
                    "agent_id": snapshot.agent_id,
                    "name": snapshot.name,
                    "status": "running",
                    "unchanged": true,
                    "hint": "No change since your last check. Do not poll: results arrive automatically as <codewhale:subagent.done> sentinels. Either continue independent work, end your turn, or make one agent(action=\"wait\") call to block until this child settles.",
                });
                let mut tool_result = ToolResult::json(&payload)
                    .map_err(|err| ToolError::execution_failed(err.to_string()))?;
                tool_result.metadata = Some(json!({
                    "action": if peek { "peek" } else { "status" },
                    "status": "running",
                    "terminal": false,
                    "agent_id": payload["agent_id"],
                    "unchanged": true,
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

/// Bounds for `agent(action="wait")` (#4097). The default keeps one wait call
/// well under provider/tool timeouts while covering typical child runtimes;
/// on expiry the model gets a still-running snapshot and can wait again.
const SUBAGENT_WAIT_DEFAULT_TIMEOUT_SECS: u64 = 300;
/// Runtime floor is 1s (schema advertises 5) so tests can exercise the
/// timeout path without multi-second sleeps.
const SUBAGENT_WAIT_MIN_TIMEOUT_SECS: u64 = 1;
const SUBAGENT_WAIT_MAX_TIMEOUT_SECS: u64 = 1800;
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
async fn wait_for_subagents_from_input(
    input: &Value,
    manager: SharedSubAgentManager,
    context: &ToolContext,
) -> Result<ToolResult, ToolError> {
    let timeout_secs = input
        .get("timeout_secs")
        .or_else(|| input.get("timeout"))
        .and_then(Value::as_u64)
        .unwrap_or(SUBAGENT_WAIT_DEFAULT_TIMEOUT_SECS)
        .clamp(
            SUBAGENT_WAIT_MIN_TIMEOUT_SECS,
            SUBAGENT_WAIT_MAX_TIMEOUT_SECS,
        );
    let timeout = Duration::from_secs(timeout_secs);
    let agent_ref = parse_agent_ref(input);

    // Resolve the watch set up front so a bad reference fails immediately
    // instead of blocking for the full timeout.
    let watched: Vec<String> = {
        let manager = manager.read().await;
        if let Some(agent_ref) = &agent_ref {
            let snapshot = manager
                .get_result_by_ref(agent_ref)
                .map_err(|err| ToolError::invalid_input(err.to_string()))?;
            if snapshot.status != SubAgentStatus::Running {
                let running = manager.running_count();
                drop(manager);
                return wait_result_payload(&[snapshot], running, 0, false).await;
            }
            vec![snapshot.agent_id]
        } else {
            manager
                .list_filtered(false)
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
                if let Ok(snapshot) = manager.get_result_by_ref(agent_id)
                    && snapshot.status != SubAgentStatus::Running
                {
                    settled.push(snapshot);
                }
            }
            (settled, manager.running_count())
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
        "Wait timed out with children still running. Do not poll — either wait again, continue independent work, or end your turn; results arrive automatically as <codewhale:subagent.done> sentinels."
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

fn provider_pin_matches_session(runtime: &SubAgentRuntime, provider_id: &str) -> bool {
    let provider_id = provider_id.trim();
    let session_provider = runtime.client.api_provider();
    if let Some(config) = runtime.api_config.as_ref() {
        let Ok(pinned) = config.resolve_provider_identity(provider_id) else {
            return false;
        };
        let active_identity = config.provider_identity_for(session_provider);
        if pinned.provider == crate::config::ApiProvider::Custom
            || session_provider == crate::config::ApiProvider::Custom
        {
            return pinned.provider == session_provider && pinned.key == active_identity;
        }
        return pinned.provider == session_provider;
    }
    if let Some(provider) = crate::config::ApiProvider::parse(provider_id) {
        return provider == session_provider;
    }
    session_provider == crate::config::ApiProvider::Custom
        && runtime
            .api_config
            .as_ref()
            .and_then(|config| config.provider.as_deref())
            .map(str::trim)
            .is_some_and(|active| active == provider_id)
}

struct ChildProviderBinding {
    client: DeepSeekClient,
    api_config: Option<std::sync::Arc<crate::config::Config>>,
}

fn child_provider_binding(
    runtime: &SubAgentRuntime,
    member: Option<&crate::fleet::profile::AgentProfile>,
) -> Result<ChildProviderBinding, ToolError> {
    let session_provider = runtime.client.api_provider();
    match crate::fleet::worker_runtime::explicit_fleet_provider_id(member) {
        Some(pinned_id) if !provider_pin_matches_session(runtime, &pinned_id) => {
            let (scoped_config, _) =
                runtime
                    .scoped_config_for_provider_id(&pinned_id)
                    .map_err(|err| {
                        ToolError::execution_failed(format!(
                            "fleet profile pins provider '{}' but its client could not be built \
                         ({err}). Configure that provider's credentials/base URL, or drop the \
                         provider pin to inherit the session provider '{}'.",
                            pinned_id,
                            session_provider.as_str()
                        ))
                    })?;
            let client = DeepSeekClient::new(&scoped_config).map_err(|err| {
                ToolError::execution_failed(format!(
                    "fleet profile pins provider '{}' but its client could not be built \
                     ({err}). Configure that provider's credentials/base URL, or drop the \
                     provider pin to inherit the session provider '{}'.",
                    pinned_id,
                    session_provider.as_str()
                ))
            })?;
            Ok(ChildProviderBinding {
                client,
                api_config: Some(std::sync::Arc::new(scoped_config)),
            })
        }
        _ => Ok(ChildProviderBinding {
            client: runtime.client.clone(),
            api_config: runtime.api_config.clone(),
        }),
    }
}

/// Resolve the LLM client a freshly spawned in-process child should run on,
/// honoring a fleet roster member's explicit provider pin (#4193).
///
/// - No member, a member pinning no provider (profile-less / `inherit`), or a
///   member pinning the session's own provider: reuse the parent/session client
///   unchanged. Preserves pre-#4193 behavior — no regression.
/// - A member pinning a provider DIFFERENT from the session: build a fresh
///   client for that provider (its base URL + credentials). This is the
///   substantive fix; the `provider` metadata tag alone is inert while the
///   client is shared, so without this the request still hits the session
///   provider's endpoint with model B's id (#4093).
///
/// A pinned-but-unbuildable provider is a hard error — never a silent fallback
/// to the session client (that silent fallback IS the #4093 misroute). The
/// provider comes only from the explicit pin ([`explicit_fleet_provider`]),
/// never inferred from the model id (EPIC #2608).
#[cfg(test)]
fn child_client_for_member(
    runtime: &SubAgentRuntime,
    member: Option<&crate::fleet::profile::AgentProfile>,
) -> Result<DeepSeekClient, ToolError> {
    child_provider_binding(runtime, member).map(|binding| binding.client)
}

async fn spawn_subagent_from_input(
    input: Value,
    manager: SharedSubAgentManager,
    mut runtime: SubAgentRuntime,
) -> Result<(SubAgentResult, WorkflowTaskSpawnMetadata), ToolError> {
    apply_session_spawn_defaults(&mut runtime);
    let mut spawn_request = parse_spawn_request(&input)?;
    let profile_member = apply_spawn_profile(&mut spawn_request, &runtime.fleet_roster)?;

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
    // #4193 seam 3 (the substantive fix): if the resolved roster member's
    // profile pins a provider different from the session's, rebind the child to
    // a fresh client for that provider BEFORE any model normalization/routing.
    // Every downstream model decision below derives its provider from
    // `child_runtime.client.api_provider()`, so swapping the client here is what
    // actually routes the request to provider B's endpoint with B's creds —
    // rather than tagging `provider = B` on a client still pointed at A (#4093).
    let provider_binding = child_provider_binding(&runtime, profile_member.as_ref())?;
    child_runtime.client = provider_binding.client;
    child_runtime.api_config = provider_binding.api_config;
    child_runtime.max_spawn_depth = child_max_spawn_depth_for_spawn(
        child_runtime.max_spawn_depth,
        child_runtime.spawn_depth,
        spawn_request.max_depth,
        profile_member
            .as_ref()
            .and_then(|member| member.profile.delegation.max_spawn_depth),
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
        child_runtime.worker_profile.denied_tools.clear();
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
    // Enforce declared write authority (TUI-DOG-017): `read_only` narrows the
    // child's runtime profile so Suggest-level write tools are actually gated,
    // not just described. `derive_child` intersects permissions, so the
    // narrowing also binds every grandchild.
    if spawn_request.write_authority == Some(SpawnWriteAuthority::ReadOnly) {
        child_runtime.worker_profile.permissions.write = false;
    }
    // Resolve the model once against the CHILD's (possibly profile-pinned)
    // provider. The typed selection carries both precedence and provenance so
    // a role default cannot override a saved AgentProfile model (#4177).
    let model_selection =
        resolve_spawn_model_selection(&child_runtime, &spawn_request, profile_member.as_ref())?;
    let resident_context = spawn_request
        .resident_file
        .as_deref()
        .map(|file_path| read_bounded_resident_context(&runtime.context, file_path))
        .transpose()?;
    let effective_prompt = if let Some(resident) = resident_context.as_ref() {
        let prefixed = format!(
            "<!-- resident_file: {} -->\n```\n{}\n```\n\n{}",
            resident.display_path, resident.contents, spawn_request.prompt
        );
        prefixed
    } else {
        spawn_request.prompt
    };
    // Surface the declared expected artifact to the child so the deliberate
    // contract is visible to the agent doing the work, not just validated at
    // the parse boundary (TUI-DOG-017).
    let effective_prompt = match spawn_request.expected_artifact.as_deref() {
        Some(artifact) => {
            format!("{effective_prompt}\n\nExpected artifact (declared by the spawner): {artifact}")
        }
        None => effective_prompt,
    };
    let effective_prompt = if spawn_request.dependencies.is_empty()
        && spawn_request.acceptance.is_empty()
    {
        effective_prompt
    } else {
        let dependencies = spawn_request
            .dependencies
            .iter()
            .map(|item| format!("- {}", item.chars().take(256).collect::<String>()))
            .collect::<Vec<_>>()
            .join("\n");
        let acceptance = spawn_request
            .acceptance
            .iter()
            .map(|item| format!("- {}", item.chars().take(256).collect::<String>()))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "{effective_prompt}\n\nDelegation contract (bounded):\nDependencies:\n{dependencies}\nAcceptance:\n{acceptance}"
        )
    };
    let write_capable = spawn_request.write_authority != Some(SpawnWriteAuthority::ReadOnly)
        && matches!(
            spawn_request.agent_type,
            SubAgentType::General | SubAgentType::Implementer | SubAgentType::Custom
        );
    let write_claim = write_capable.then(|| WriteScopeClaim {
        owner: String::new(),
        roots: spawn_request.write_roots.clone(),
        exact_files: spawn_request.exact_files.clone(),
        contracts: spawn_request.coordination_contracts.clone(),
    });
    // #4193 seam 2 (cont.): strength/inherit/faster routing and the final
    // provider-namespace guard both read the provider from the runtime's client,
    // so route them through `child_runtime` (pinned provider) instead of the
    // session `runtime`. Router candidates, reasoning-effort defaults, and the
    // fixed-model validation then all resolve against provider B.
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
    child_runtime.reasoning_effort = route.reasoning_effort.clone();
    child_runtime.reasoning_effort_auto = false;
    let model_route = route.model_route;
    let resolved_role = profile_member
        .as_ref()
        .map(|member| member.profile.role.name.clone())
        .filter(|name| !name.trim().is_empty())
        .or_else(|| spawn_request.assignment.role.clone());
    let resolved_profile = profile_member
        .as_ref()
        .map(|member| member.id.clone())
        .or_else(|| spawn_request.profile.clone());
    let spawn_metadata = WorkflowTaskSpawnMetadata {
        resolved_provider: child_runtime
            .api_config
            .as_ref()
            .map(|config| config.provider_identity_for(child_runtime.client.api_provider()))
            .unwrap_or_else(|| child_runtime.client.api_provider().as_str().to_string()),
        resolved_model: effective_model.clone(),
        route_source: model_selection.source.as_str().to_string(),
        resolved_role,
        resolved_profile,
        parent_task_id: child_runtime.parent_agent_id.clone(),
        depth: child_runtime.spawn_depth,
        workflow_run_id: None,
        workflow_phase_id: None,
        workflow_task_label: None,
        workflow_child_index: None,
    };

    // #4647: a child receives only its explicit objective/dependencies/
    // acceptance and relevant accepted-decision projection by default. Forking
    // a parent transcript is still available, but only through an explicit
    // `fork_context: true` request.
    let fork_context = spawn_request.fork_context.unwrap_or(false);
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
            nickname: None,
            fork_context,
            token_budget: spawn_request.token_budget,
            max_steps: spawn_request.max_steps,
            wall_time: spawn_request.wall_time,
            write_claim,
            isolated_worktree: spawn_request.worktree.is_some(),
            expected_artifact: spawn_request.expected_artifact.clone(),
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

fn build_subagent_system_prompt_with_skills(
    agent_type: &SubAgentType,
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
    agent_type: &SubAgentType,
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
    agent_type: &SubAgentType,
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
}

#[allow(clippy::too_many_lines)]
async fn run_subagent_task(task: SubAgentTask) {
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
    // by `run_subagent`'s own first-step cancel check.
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
            record_agent_progress(
                &task.runtime,
                &task.agent_id,
                AgentProgressEventMeta::new(AgentWorkerStatus::Cancelled),
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
        agent_id: agent_id.to_string(),
        payload: payload.to_string(),
    });
    true
}

pub(crate) fn subagent_completion_from_result(result: &SubAgentResult) -> SubAgentCompletion {
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
    let (summary, truncated) = stamp_subagent_summary(&summary_source);
    let summary_truncated = truncated || evidence_truncated;
    let sentinel = match &result.status {
        SubAgentStatus::Failed(error) => subagent_failed_sentinel(&result.agent_id, error),
        _ => subagent_done_sentinel(&result.agent_id, result, summary_truncated),
    };
    let payload = match evidence_block {
        Some(evidence) => format!("{summary}\n{evidence}\n{sentinel}"),
        None => format!("{summary}\n{sentinel}"),
    };
    SubAgentCompletion {
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
    let (bounded_messages, omitted_messages) =
        bounded_tail_messages(messages, SUBAGENT_TRANSCRIPT_MESSAGE_BUDGET_BYTES);
    let checkpoint_meta = checkpoint.map(|checkpoint| SubAgentCheckpoint {
        omitted_messages: checkpoint.message_count,
        messages: Vec::new(),
        ..checkpoint.clone()
    });
    let transcript_artifact = transcript_artifact.map(|writer| {
        let synced = match writer.sync_messages(messages, *status != SubAgentStatus::Running) {
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
        writer.metadata(synced && writer.persisted_messages == messages.len())
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
    agent_type: &SubAgentType,
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
    let system_prompt =
        build_subagent_system_prompt_with_skills(&agent_type, &assignment, &runtime.context);
    let fork_context_enabled = fork_context;
    let fork_context = fork_context_enabled
        .then_some(runtime.fork_context.as_ref())
        .flatten();
    let request_system = subagent_request_system_prompt(&system_prompt);
    let mut messages = build_initial_subagent_messages_with_system(
        &prompt,
        &assignment,
        &agent_type,
        &system_prompt,
        fork_context,
    );
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
        AgentProgressEventMeta::new(AgentWorkerStatus::Starting),
        format!("started ({})", agent_type.as_str()),
    );

    let mut steps = 0;
    let mut final_result: Option<String> = None;
    let mut pending_inputs: VecDeque<SubAgentInput> = VecDeque::new();
    let mut consecutive_truncated_responses = 0;
    let mut latest_checkpoint: Option<SubAgentCheckpoint> = None;
    let mut tokens_used: u64 = 0;
    // #4050: distinguish a real "the model chose to stop" exit (the `break`
    // below) from loop exhaustion (running out of `max_steps` while still
    // tool-calling). Only the former, with a non-empty final summary, is a
    // genuine success; everything else must surface its stop reason instead of
    // reporting a completed child with no payload.
    let mut stopped_naturally = false;
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

    for _step in 0..max_steps {
        // Cooperative cancellation: bail if this session's token was cancelled
        // while we were between steps. Top-level model-visible sub-agents use
        // a detached token so parent turn cancellation does not stop them.
        if runtime.cancel_token.is_cancelled() {
            record_agent_progress(
                runtime,
                &agent_id,
                AgentProgressEventMeta::new(AgentWorkerStatus::Cancelled).with_step(steps),
                format!("{}: cancelled", format_step_counter(steps, max_steps)),
            );
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
            AgentProgressEventMeta::new(AgentWorkerStatus::ModelWait).with_step(steps),
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
                AgentProgressEventMeta::new(AgentWorkerStatus::Running).with_step(steps),
                format!(
                    "{}: received {count} child sub-agent completion(s)",
                    format_step_counter(steps, max_steps)
                ),
            );
            messages.push(child_completion_runtime_message(&child_completions));
        }

        let has_tools = !tools.is_empty();
        let request = MessageRequest {
            model: runtime.model.clone(),
            messages: messages.clone(),
            max_tokens: SUBAGENT_RESPONSE_MAX_TOKENS,
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
        let response = tokio::select! {
            biased;
            () = runtime.cancel_token.cancelled() => {
                record_agent_progress(
                    runtime,
                    &agent_id,
                    AgentProgressEventMeta::new(AgentWorkerStatus::Cancelled).with_step(steps),
                    format!("{}: cancelled mid-request", format_step_counter(steps, max_steps)),
                );
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
                            parent_run_id: runtime.parent_agent_id.clone(),
                            spawn_depth: runtime.spawn_depth,
                            result: Some(reason),
                            steps_taken: steps,
                            checkpoint: Some(checkpoint),
                            needs_input: Some(needs_input),
                            duration_ms,
                            from_prior_session: false,
                        });
                    }
                }
            }
        };

        let mut tool_uses = Vec::new();

        // Report token usage so the parent's cost counter updates live.
        if let Some(mb) = runtime.mailbox.as_ref() {
            let _ = mb.send(MailboxMessage::token_usage(
                &agent_id,
                runtime.client.api_provider(),
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
        if let Some(budget) = token_budget
            && tokens_used > budget
        {
            record_agent_progress(
                runtime,
                &agent_id,
                AgentProgressEventMeta::new(AgentWorkerStatus::Failed).with_step(steps),
                format!(
                    "{}: token budget exhausted ({tokens_used}/{budget})",
                    format_step_counter(steps, max_steps)
                ),
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
                AgentProgressEventMeta::new(AgentWorkerStatus::Running).with_step(steps),
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
        reset_truncated_subagent_responses(&mut consecutive_truncated_responses);

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
            let (result, spilled_to) = bound_subagent_tool_result(
                &agent_id,
                &tool_id,
                &tool_name,
                &runtime.context.state_namespace,
                tool_ok,
                result,
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
    let status = if stopped_naturally {
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
    let dependencies = parse_bounded_strings(input, "dependencies", 8)?;
    let acceptance = parse_bounded_strings(input, "acceptance", 8)?;
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

    // Role may be either a SubAgentType alias (reviewer → Review) or a fleet
    // roster role / member id (scout, release_lead). Type aliases still set
    // agent_type; non-alias roles defer to fleet profile resolution (#4177).
    let parsed_role_type = role_input.and_then(SubAgentType::from_str);
    let role_is_type_alias = parsed_role_type.is_some();

    if let (Some(type_kind), Some(role_kind)) = (&parsed_type, &parsed_role_type)
        && type_kind != role_kind
    {
        return Err(ToolError::invalid_input(
            "Conflicting type/agent_type and role/agent_role values".to_string(),
        ));
    }

    let agent_type_explicit = parsed_type.is_some() || parsed_role_type.is_some();
    let agent_type = parsed_type
        .or(parsed_role_type)
        .unwrap_or(SubAgentType::General);

    let role_alias = role_input
        .and_then(normalize_role_alias)
        .or_else(|| type_input.and_then(normalize_role_alias))
        .map(str::to_string);

    // Fleet role token: the raw role only when it is not a descriptive type
    // alias. Type aliases remain local SubAgentType vocabulary and must not be
    // promoted into roster lookup keys.
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

    let mut profile = optional_input_str(input, &["profile", "fleet_profile", "roster_profile"])
        .map(validate_profile_name)
        .transpose()?;
    // When the caller declared a non-type Fleet role, use it as the profile
    // key so `apply_spawn_profile` is the single roster resolution path.
    // Descriptive SubAgentType aliases (worker/review/plan/verify/...) keep
    // profile=None; promoting those aliases to roster ids made valid direct
    // agent calls fail because several are not member ids (#4177).
    if profile.is_none() {
        profile = fleet_role_token.clone();
    }

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
    let model = parse_optional_subagent_model(input, "model")?;
    let explicit_model_strength = optional_input_str(input, &["model_strength", "modelStrength"])
        .map(SubAgentModelStrength::parse)
        .transpose()?;
    let model_strength_explicit = explicit_model_strength.is_some();
    // Fleet is predictable before setup: every role inherits the active model.
    // A cheaper sibling is an explicit routing choice through model_strength,
    // a saved Fleet profile, or a concrete model override.
    let model_strength = explicit_model_strength.unwrap_or(SubAgentModelStrength::Same);
    let thinking = optional_input_str(input, &["thinking", "reasoning_effort", "reasoningEffort"])
        .map(SubAgentThinking::parse)
        .transpose()?
        .unwrap_or(SubAgentThinking::Inherit);
    let resident_file = input
        .get("resident_file")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty());
    let fork_context =
        parse_optional_bool(input, &["fork_context", "forkContext", "inherit_context"]);
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
    let max_steps = input
        .get("max_steps")
        .or_else(|| input.get("maxSteps"))
        .and_then(Value::as_u64)
        .map(|steps| {
            u32::try_from(steps.min(u64::from(MAX_SUBAGENT_STEPS)))
                .expect("max_steps is clamped before conversion")
        });
    let wall_time = input
        .get("wall_time_secs")
        .or_else(|| input.get("wallTimeSecs"))
        .and_then(Value::as_u64)
        .map(|seconds| Duration::from_secs(seconds.clamp(1, MAX_CHILD_WALL_TIME.as_secs())));

    // #4042: optional caller-supplied tool deny-list (unioned with the parent's
    // inherited deny-list) and the inheritance opt-out flag (default inherits).
    let disallowed_tools = parse_disallowed_tools(input)?;
    let inherit_disallowed_tools = parse_optional_bool(
        input,
        &["inherit_disallowed_tools", "inheritDisallowedTools"],
    )
    .unwrap_or(true);

    // Deliberate delegation contract: when `deliberate=true`, require the
    // model to declare task type (or profile), workspace policy, expected
    // artifact, and write authority. The declared values are
    // parsed and ENFORCED whenever present (deliberate or not): declaring
    // authority the runtime ignores would be a false affordance
    // (TUI-DOG-017).
    let deliberate = parse_optional_bool(input, &["deliberate"]).unwrap_or(false);
    let workspace_policy_str = optional_input_str(input, &["workspace_policy", "workspacePolicy"]);
    let expected_artifact = optional_input_str(input, &["expected_artifact", "expectedArtifact"])
        .map(str::trim)
        .filter(|artifact| !artifact.is_empty())
        .map(str::to_string);
    let write_authority_str = optional_input_str(input, &["write_authority", "writeAuthority"]);
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
    let mut write_authority = match write_authority_str
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
    if matches!(
        agent_type,
        SubAgentType::Explore | SubAgentType::Plan | SubAgentType::Review | SubAgentType::Verifier
    ) && write_authority.is_some_and(|authority| authority != SpawnWriteAuthority::ReadOnly)
    {
        return Err(ToolError::invalid_input(format!(
            "{} is a read-only role and cannot declare write-capable authority",
            agent_type.as_str()
        )));
    }
    if agent_type == SubAgentType::Implementer
        && write_authority == Some(SpawnWriteAuthority::ReadOnly)
    {
        return Err(ToolError::invalid_input(
            "implementer is a write-capable role and cannot declare read_only authority; use review, verifier, plan, explore, or general for a read-only child",
        ));
    }
    let write_roots = parse_coordination_paths(input, "write_roots")?;
    let exact_files = parse_coordination_paths(input, "exact_files")?;
    let coordination_contracts = parse_bounded_strings(input, "coordination_contracts", 16)?;
    let write_capable = write_authority != Some(SpawnWriteAuthority::ReadOnly)
        && matches!(
            agent_type,
            SubAgentType::General | SubAgentType::Implementer | SubAgentType::Custom
        );
    if write_capable
        && write_roots.is_empty()
        && exact_files.is_empty()
        && coordination_contracts.is_empty()
    {
        let prompt_only_general = agent_type == SubAgentType::General
            && !agent_type_explicit
            && profile.is_none()
            && role_input.is_none()
            && type_input.is_none();
        if write_authority.is_some() || !prompt_only_general {
            return Err(ToolError::invalid_input(
                "explicit write-capable agent starts must declare write_roots, exact_files, or coordination_contracts; choose a read-only role for non-mutating work"
                    .to_string(),
            ));
        }
        // A prompt-only/general launch remains ergonomic but is not silently
        // granted the whole repository. It starts read-only until the caller
        // supplies an explicit bounded mutation claim.
        write_authority = Some(SpawnWriteAuthority::ReadOnly);
    }

    Ok(SpawnRequest {
        session_name,
        prompt: prompt.clone(),
        dependencies,
        acceptance,
        agent_type,
        agent_type_explicit,
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
    })
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

/// Validate and normalize the `profile` spawn parameter: a bare roster member
/// id token (same rule as fleet model/profile tokens — visible, no
/// whitespace, quotes, backticks, or '='), lowercased for the roster's
/// case-insensitive lookup.
fn validate_profile_name(value: &str) -> Result<String, ToolError> {
    validate_roster_token(value, "profile")
}

fn validate_role_name(value: &str) -> Result<String, ToolError> {
    validate_roster_token(value, "role")
}

fn validate_roster_token(value: &str, field: &str) -> Result<String, ToolError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ToolError::invalid_input(format!("{field} cannot be blank")));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_graphic() && !matches!(ch, '"' | '\'' | '`' | '='))
    {
        return Err(ToolError::invalid_input(format!(
            "{field} must be a bare roster member id without whitespace, quotes, backticks, or '='"
        )));
    }
    Ok(trimmed.to_ascii_lowercase())
}

/// Resolve the `profile` spawn parameter against the fleet roster and fold
/// the member into the request: agent type (when not explicitly given),
/// assignment role, and the profile instruction overlay on the child prompt.
///
/// Runs at spawn time — `parse_spawn_request` has no runtime access. Returns
/// the resolved member so the spawn path can apply its model routing and
/// delegation bounds. The member's `permissions` block is intentionally NOT
/// consumed here: it defaults to the floor (no shell, no trust, approvals on)
/// and the child's capability posture is governed by the member's
/// `SubAgentType` via `WorkerRuntimeProfile::for_role` — applying the block
/// here could only widen that posture.
fn apply_spawn_profile(
    request: &mut SpawnRequest,
    roster: &crate::fleet::roster::FleetRoster,
) -> Result<Option<crate::fleet::profile::AgentProfile>, ToolError> {
    let Some(profile_id) = request.profile.as_deref() else {
        return Ok(None);
    };
    let Some(member) = resolve_roster_member(roster, profile_id) else {
        let available = roster
            .members()
            .iter()
            .map(|member| member.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(ToolError::invalid_input(format!(
            "Unknown fleet role/profile '{profile_id}'. Available fleet roster members: {available}. \
             Type aliases: {VALID_ROLE_ALIASES}. See /fleet."
        )));
    };

    let member_type = crate::fleet::worker_runtime::roster_member_agent_type(member);
    if request.agent_type_explicit && request.agent_type != member_type {
        return Err(ToolError::invalid_input(format!(
            "profile '{}' implies type {}; conflicting explicit type '{}'",
            member.id,
            member_type.as_str(),
            request.agent_type.as_str()
        )));
    }
    request.agent_type = member_type;
    // Record the canonical profile id after role→profile resolution.
    request.profile = Some(member.id.clone());

    // Surface the member's role in prompts and ledger records.
    let role_name = member.profile.role.name.trim();
    request.assignment.role = Some(if role_name.is_empty() {
        member.id.clone()
    } else {
        role_name.to_string()
    });

    if let Some(overlay) = spawn_profile_prompt_overlay(member) {
        request.prompt.push_str(&overlay);
    }

    Ok(Some(member.clone()))
}

/// Resolve a fleet role or profile token against the roster (#4177).
///
/// Lookup order:
/// 1. Member id (case-insensitive)
/// 2. Member role name
/// 3. Common stopship aliases (`implementer` → `builder`, `release_lead` → `manager`)
fn resolve_roster_member<'a>(
    roster: &'a crate::fleet::roster::FleetRoster,
    id_or_role: &str,
) -> Option<&'a crate::fleet::profile::AgentProfile> {
    let key = id_or_role.trim();
    if key.is_empty() {
        return None;
    }
    if let Some(member) = roster.get(key) {
        return Some(member);
    }
    if let Some(member) = roster
        .members()
        .iter()
        .find(|member| member.profile.role.name.trim().eq_ignore_ascii_case(key))
    {
        return Some(member);
    }
    let alias = match key.to_ascii_lowercase().as_str() {
        "implementer" | "implement" | "implementation" => Some("builder"),
        "release_lead" | "release-lead" | "releaselead" => Some("manager"),
        "scout" | "explore" | "explorer" | "exploration" => Some("scout"),
        _ => None,
    };
    alias.and_then(|id| roster.get(id))
}

/// Compact profile block appended to the child prompt, mirroring the fleet
/// dispatcher's `fleet_task_prompt_with_profile` overlay. `None` when the
/// member carries no description or instructions (built-ins: posture alone
/// speaks through the type system prompt).
fn spawn_profile_prompt_overlay(member: &crate::fleet::profile::AgentProfile) -> Option<String> {
    let description = member.description.as_deref().map(str::trim);
    let instructions = member.profile.role.instructions.as_deref().map(str::trim);
    if description.is_none_or(str::is_empty) && instructions.is_none_or(str::is_empty) {
        return None;
    }
    let mut overlay = String::new();
    overlay.push_str("\n\nFleet profile: ");
    overlay.push_str(&member.id);
    if let Some(display_name) = member.display_name.as_deref() {
        overlay.push_str(" (");
        overlay.push_str(display_name);
        overlay.push(')');
    }
    if let Some(description) = description.filter(|text| !text.is_empty()) {
        overlay.push_str("\nProfile description:\n");
        overlay.push_str(description);
    }
    if let Some(instructions) = instructions.filter(|text| !text.is_empty()) {
        overlay.push_str("\nProfile instructions:\n");
        overlay.push_str(instructions);
    }
    Some(overlay)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpawnRouteSource {
    TaskModel,
    TaskModelStrength,
    AgentProfileModel,
    AgentProfileLoadout,
    RoleDefault,
    RunModel,
}

impl SpawnRouteSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::TaskModel => "task.model",
            Self::TaskModelStrength => "task.model_strength",
            Self::AgentProfileModel => "agent_profile.model",
            Self::AgentProfileLoadout => "agent_profile.loadout",
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
/// explicit task field > saved AgentProfile > configured role/type default >
/// operator run model. Keeping the route and its source together prevents a
/// later configured-model lookup from silently overriding a profile pin.
fn resolve_spawn_model_selection(
    runtime: &SubAgentRuntime,
    request: &SpawnRequest,
    member: Option<&crate::fleet::profile::AgentProfile>,
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
    if let Some(member) = member {
        if let Some(model) = member
            .profile
            .model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty() && !model.eq_ignore_ascii_case("auto"))
        {
            let model = normalize_requested_subagent_model(
                model,
                &format!("fleet.profiles.{}.model", member.id),
                runtime.client.api_provider(),
            )?;
            return Ok(SpawnModelSelection {
                model_route: ModelRoute::Fixed(model),
                source: SpawnRouteSource::AgentProfileModel,
            });
        }
        if member.profile.loadout == codewhale_config::FleetLoadout::Fast {
            return Ok(SpawnModelSelection {
                model_route: ModelRoute::Faster,
                source: SpawnRouteSource::AgentProfileLoadout,
            });
        }
        // Richer custom loadouts (strong/balanced/...) have no exact
        // ModelRoute equivalent here. Auto means "cheap sibling" in the
        // sub-agent router, so those and explicit Inherit both preserve the
        // operator run model and report that model's actual source.
        return Ok(SpawnModelSelection {
            model_route: ModelRoute::Inherit,
            source: SpawnRouteSource::RunModel,
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

/// Effective absolute `max_spawn_depth` for a child, combining the inherited
/// runtime budget, the caller's `max_depth` request, and a fleet profile's
/// `delegation.max_spawn_depth` hint. An explicit request keeps its existing
/// semantics (may widen up to the ceiling); a profile hint only narrows —
/// either the request (min) or the inherited budget.
fn child_max_spawn_depth_for_spawn(
    inherited: u32,
    child_spawn_depth: u32,
    requested: Option<u32>,
    profile_hint: Option<u32>,
) -> u32 {
    match (requested, profile_hint) {
        (Some(requested), hint) => {
            let depth = hint.map_or(requested, |hint| requested.min(hint));
            clamp_child_max_spawn_depth(child_spawn_depth, depth)
        }
        (None, Some(hint)) => inherited.min(clamp_child_max_spawn_depth(child_spawn_depth, hint)),
        (None, None) => inherited,
    }
}

fn parse_optional_bool(input: &Value, names: &[&str]) -> Option<bool> {
    names
        .iter()
        .find_map(|name| input.get(*name))
        .and_then(Value::as_bool)
}

/// Parse an optional caller-supplied `disallowed_tools` array (#4042). Mirrors
/// the `allowed_tools` parsing: trimmed, de-duplicated, non-empty-only. Returns
/// `None` when the key is absent or yields no usable entries so the union merge
/// in `spawn_subagent_from_input` only runs when there is something to add.
fn parse_disallowed_tools(input: &Value) -> Result<Option<Vec<String>>, ToolError> {
    let Some(array) = input.get("disallowed_tools").and_then(Value::as_array) else {
        return Ok(None);
    };
    let mut tools = Vec::new();
    for item in array {
        let Some(tool) = item.as_str() else {
            continue;
        };
        let trimmed = tool.trim();
        if !trimmed.is_empty() && !tools.iter().any(|existing: &String| existing == trimmed) {
            tools.push(trimmed.to_string());
        }
    }
    if tools.is_empty() {
        Ok(None)
    } else {
        Ok(Some(tools))
    }
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
        &model,
        prompt,
        requested_fast_lane,
        requested_thinking,
    );

    SubAgentResolvedRoute::new(model_route.clone(), model, reasoning_effort)
}

fn subagent_reasoning_effort_for_request(
    runtime: &SubAgentRuntime,
    model: &str,
    prompt: &str,
    requested_fast_lane: bool,
    requested_thinking: SubAgentThinking,
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
    if runtime.reasoning_effort_auto {
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
    let discovery_anchor = if let Some(requested_cwd) = request.cwd.as_ref() {
        validate_existing_child_cwd(parent_workspace, requested_cwd)?
    } else {
        parent_workspace
            .canonicalize()
            .unwrap_or_else(|_| parent_workspace.to_path_buf())
    };

    if let Some(worktree) = request.worktree.as_ref() {
        return create_isolated_worktree(
            &discovery_anchor,
            worktree,
            request.session_name.as_deref(),
            &request.agent_type,
        )
        .map(Some);
    }

    if request.cwd.is_some() {
        return Ok(Some(discovery_anchor));
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
            "Invalid cwd '{}': {e} (path may not exist yet — use worktree=true to let Codewhale create an isolated checkout)",
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
    const MAX_PARENT_LEVELS: usize = 4;
    let start = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let mut paths_tried = Vec::new();
    let mut current = Some(start.as_path());
    let mut levels = 0usize;

    while let Some(dir) = current {
        paths_tried.push(dir.display().to_string());

        if let Some(root) = try_git_toplevel(dir) {
            return Ok(root);
        }

        if let Ok(entries) = fs::read_dir(dir) {
            let mut nested_roots = Vec::new();
            for entry in entries.flatten() {
                let child = entry.path();
                if !child.is_dir() || !path_looks_like_git_checkout(&child) {
                    continue;
                }
                if child
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with('.'))
                {
                    continue;
                }
                if let Some(root) = try_git_toplevel(&child) {
                    nested_roots.push(root);
                }
            }
            match nested_roots.len() {
                0 => {}
                1 => return Ok(nested_roots.into_iter().next().expect("single nested root")),
                _ => {
                    let repos = nested_roots
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(ToolError::invalid_input(format!(
                        "Multiple git repositories found under {}. Specify cwd to disambiguate: {repos}",
                        dir.display()
                    )));
                }
            }
        }

        levels += 1;
        if levels > MAX_PARENT_LEVELS {
            break;
        }
        current = dir.parent();
    }

    Err(ToolError::invalid_input(format!(
        "worktree=true requires a git repository. Tried: {}",
        paths_tried.join(", ")
    )))
}

fn path_looks_like_git_checkout(path: &Path) -> bool {
    let git_path = path.join(".git");
    git_path.is_dir() || git_path.is_file()
}

fn try_git_toplevel(path: &Path) -> Option<PathBuf> {
    let output = Git::output(&["rev-parse", "--show-toplevel"], path).ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        None
    } else {
        Some(PathBuf::from(root))
    }
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

pub(crate) fn subagent_progress_tool_display_name(name: &str) -> &str {
    match name {
        "exec_shell"
        | "exec_shell_wait"
        | "exec_shell_interact"
        | "exec_shell_cancel"
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
    /// Tool deny-list inherited from the parent runtime's `worker_profile`
    /// (#4042). Deny always wins over allow, even when a tool is in both the
    /// allowlist and this list. Wildcard matching mirrors the session-side
    /// `command_denies_tool` (exact + `prefix*`, case-insensitive).
    disallowed_tools: Vec<String>,
    auto_approve: bool,
    /// Workflow-spawned children auto-accept Suggest-level file edits.
    accept_edits: bool,
    /// Root Operate workers may run only the built-in verifier surfaces after
    /// their parent-approved `agent` start. This never delegates raw shell or
    /// user-supplied verifier commands.
    accept_verification: bool,
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
    coordination_manager: SharedSubAgentManager,
    /// Disabled only by the legacy unit-test constructor, which has no
    /// admitted worker identity. Production registries always enforce claims.
    enforce_write_claim: bool,
    registry: ToolRegistry,
}

impl SubAgentToolRegistry {
    const ACTION_ALIASES: &'static [(&'static str, &'static str, &'static str)] =
        CANONICAL_ACTION_ALIASES;

    #[cfg(test)]
    fn new(
        runtime: SubAgentRuntime,
        agent_type: SubAgentType,
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
        agent_type: SubAgentType,
        owner_agent_id: String,
        owner_agent_name: String,
        explicit_allowed_tools: Option<Vec<String>>,
        todo_list: SharedTodoList,
        plan_state: SharedPlanState,
    ) -> Self {
        // Build the full agent surface — same as the parent's Agent mode except
        // for root-owned goal lifecycle mutation.
        // Children inherit shell, file, patch, search, web, git, diagnostics,
        // review, and RLM, plus per-child fresh todo/plan state. `agent` is
        // retained only when depth budget remains.
        let can_spawn_child = !runtime.would_exceed_depth();
        let context = runtime.context.clone();
        let coordination_manager = Arc::clone(&runtime.manager);
        let mut surface_options = runtime.agent_tool_surface_options.clone();
        surface_options.shell_policy = ShellPolicy::from_legacy_allow_shell(runtime.allow_shell);
        // This registry executes as the admitted child named by
        // `owner_agent_id`. Coordination tools derive their hierarchy caller
        // from `parent_agent_id`, so stamp the same identity here even for
        // test/restore constructors whose runtime snapshot predates admission.
        let mut child_runtime = runtime.clone();
        child_runtime.parent_agent_id = Some(owner_agent_id.clone());
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
        // Children may inspect the parent goal for context, but only the root
        // agent may create or transition that shared lifecycle. Removing the
        // mutators after all built-in and MCP registrations also prevents an
        // accidental name collision from restoring child authority.
        registry.remove_tool("create_goal");
        registry.remove_tool("update_goal");

        Self {
            allowed_tools: explicit_allowed_tools,
            disallowed_tools: runtime.worker_profile.denied_tools.clone(),
            auto_approve: runtime.context.auto_approve,
            accept_edits: runtime.accept_edits,
            accept_verification: runtime.accept_verification,
            agent_type,
            runtime_profile: runtime.worker_profile,
            can_spawn_child,
            owner_agent_id,
            owner_agent_name,
            coordination_manager,
            enforce_write_claim: true,
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

    fn is_delegated_builtin_verification(name: &str, input: &Value) -> bool {
        match name {
            // `run_tests.args` is raw Cargo argv and can redirect manifests or
            // inject toolchain config. Only the fixed workspace-root command
            // (optionally with the structured all_features flag) is delegated.
            "run_tests" => match input.get("args") {
                None => true,
                Some(Value::String(args)) => args.trim().is_empty(),
                Some(_) => false,
            },
            "run_verifiers" => input
                .get("commands")
                .map(|commands| commands.as_array().is_some_and(Vec::is_empty))
                .unwrap_or(true),
            "Run" => match input.get("action").and_then(Value::as_str) {
                Some("tests") => input
                    .get("args")
                    .is_none_or(|args| args.as_str().is_some_and(|args| args.trim().is_empty())),
                Some("verifiers") => input
                    .get("commands")
                    .map(|commands| commands.as_array().is_some_and(Vec::is_empty))
                    .unwrap_or(true),
                _ => false,
            },
            _ => false,
        }
    }

    /// Whether the role posture permits a given registered tool, independent of
    /// parent auto-approval. Delegates to the pure `role_posture_permits`.
    /// Unregistered names pass through (the allowlist / availability checks
    /// handle those separately).
    fn posture_permits_tool(&self, name: &str, input: Option<&Value>) -> bool {
        // Delegation (`agent`) is governed by the depth budget and the
        // allowlist (`can_spawn_child` / `is_tool_allowed`), not the write/shell
        // posture — a read-only role may still fan out child work.
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
                    matches!(self.runtime_profile.shell, ShellPolicy::Full)
                        && role_posture_permits(&self.agent_type, ApprovalRequirement::Required)
                }
            },
            None => true,
        }
    }

    /// Check whether a tool name is denied by the `disallowed_tools` list, using
    /// the same matching logic as the session-side `command_denies_tool`: exact
    /// match + `prefix*` wildcard, case-insensitive (#4042, #3027).
    fn is_tool_denied(&self, name: &str) -> bool {
        if self.disallowed_tools.is_empty() {
            return false;
        }
        let tool_name = name.to_ascii_lowercase();
        self.disallowed_tools.iter().any(|rule| {
            let rule = rule.to_ascii_lowercase();
            if let Some(prefix) = rule.strip_suffix('*') {
                tool_name.starts_with(prefix)
            } else {
                tool_name == rule
            }
        })
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
        if self.is_tool_denied(family) || alias.is_some_and(|name| self.is_tool_denied(name)) {
            return false;
        }
        match &self.allowed_tools {
            None => true,
            Some(list) => {
                list.iter().any(|name| name == family)
                    || alias.is_some_and(|alias| list.iter().any(|name| name == alias))
            }
        }
    }

    /// Whether a given tool name is permitted under this child's filter.
    /// `None` filter = everything permitted.
    fn is_tool_allowed(&self, name: &str) -> bool {
        if name == "agent" && !self.can_spawn_child {
            return false;
        }
        // Deny always wins over allow — check the deny-list first so a tool in
        // both the allowlist and the deny-list is still blocked (#4042).
        if self.is_tool_denied(name) {
            return false;
        }
        match &self.allowed_tools {
            None => true,
            Some(list) => {
                list.iter().any(|tool| tool == name)
                    || Self::ACTION_ALIASES.iter().any(|(family, _, alias)| {
                        *family == name && list.iter().any(|allowed| allowed == alias)
                    })
            }
        }
    }

    fn tools_for_model(&self, agent_type: &SubAgentType) -> Vec<Tool> {
        let _ = agent_type;
        let api_tools = self.registry.to_api_tools();
        let filtered = match &self.allowed_tools {
            None => api_tools,
            Some(list) => api_tools
                .into_iter()
                .filter(|tool| {
                    list.contains(&tool.name)
                        || ["Bash", "File", "Git", "Run", "Web"].contains(&tool.name.as_str())
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
            // #4042: hide explicitly disallowed tools so the model never sees
            // them in the function-calling schema (defense-in-depth with the
            // `is_tool_allowed` / `execute` guards).
            .filter(|tool| !self.is_tool_denied(&tool.name))
            // #3217: hide tools the role posture forbids so the model never
            // even sees write/edit/patch (read-only roles) or shell (no-shell
            // roles). Defense-in-depth with the `execute` guard below.
            .filter(|tool| tool.name == "File" || self.posture_permits_tool(&tool.name, None))
            .collect::<Vec<_>>();

        for tool in &mut tools {
            if !["Bash", "File", "Git", "Run", "Web"].contains(&tool.name.as_str()) {
                continue;
            }
            if let Some(actions) = tool.input_schema["properties"]["action"]["enum"].as_array_mut()
            {
                actions.retain(|action| {
                    let Some(action) = action.as_str() else {
                        return false;
                    };
                    let posture_allows = tool.name != "File"
                        || (self.runtime_profile.permissions.write
                            && role_posture_permits(
                                &self.agent_type,
                                ApprovalRequirement::Suggest,
                            ))
                        || matches!(action, "read" | "list" | "search_name" | "search_content");
                    posture_allows && self.is_action_allowed(&tool.name, action)
                });
            }
        }
        tools.retain(|tool| {
            tool.input_schema["properties"]["action"]["enum"]
                .as_array()
                .is_none_or(|actions| !actions.is_empty())
        });
        tools
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
        let action = input.get("action").and_then(Value::as_str);
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
            return Err(anyhow!(
                "Tool {name} is not permitted for the read-only `{role}` sub-agent role. Use an `implementer` or `general` role (or a `custom` role with an explicit allowed_tools list) to mutate the workspace or run shell commands.",
                role = self.agent_type.as_str()
            ));
        }
        if !self.auto_approve {
            let Some(spec) = self.registry.get(name) else {
                return Err(anyhow!("Tool {name} is not registered"));
            };
            match spec.approval_requirement_for(&input) {
                ApprovalRequirement::Auto => {}
                ApprovalRequirement::Suggest => {
                    // Write/edit/patch tools land here. Explicit
                    // write-capable roles (`implementer`, `custom`) may run them
                    // without parent auto-approve (#1828, #1833). Workflow-spawned
                    // children also accept Suggest edits for any write-capable
                    // posture (including general). Read-only roles still bounce.
                    let may_write = self.runtime_profile.permissions.write
                        && (self.accept_edits || Self::role_can_delegate_writes(&self.agent_type));
                    if !may_write {
                        return Err(anyhow!(
                            "Tool {name} requires approval and is not delegated to {role} sub-agents; rerun the parent with auto approval or pick a write-capable role",
                            role = self.agent_type.as_str()
                        ));
                    }
                }
                ApprovalRequirement::Required => {
                    if !(self.accept_verification
                        && Self::is_delegated_builtin_verification(name, &input))
                    {
                        return Err(anyhow!(
                            "Tool {name} requires approval and cannot run inside this sub-agent unless the parent session is auto-approved"
                        ));
                    }
                }
            }
        }
        reject_subagent_terminal_takeover(name, &input)?;
        let scope_aware_write = matches!(
            name,
            "write_file" | "edit_file" | "apply_patch" | "fim_edit"
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
            if manager.shared_write_claim(&self.owner_agent_id).is_some() {
                return Err(anyhow!(
                    "Tool {name} cannot prove a bounded file target for this shared-workspace write claim. Use scope-aware file tools, or launch the child with worktree isolation."
                ));
            }
        }
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

fn is_unbounded_shell_run(name: &str, input: &Value) -> bool {
    canonical_action_alias(name, input) == "exec_shell"
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
            | "update_plan"
            | "checklist_add"
            | "checklist_update"
            | "checklist_write"
            | "todo_add"
            | "todo_update"
            | "todo_write"
    )
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
    let wants_interactive_shell = matches!(name, "exec_shell" | "Bash")
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
    "You are a trusted general-purpose sub-agent. Your job is to complete the one task you were given, end-to-end, and report back concisely.\n",
    "Stay inside the assigned scope; put adjacent work under RISKS/BLOCKERS.\n",
    "For genuinely multi-step work, track progress with `work_update` (and `update_plan` for Strategy metadata); skip it for short, focused tasks.\n",
    "**Stop quickly on failure**: if the same tool call fails 2 times in a row, stop retrying and return what you have so far with a one-line note explaining what's missing. Do not loop on impossible queries (e.g. external API unreachable, rate-limited, or returning empty).\n",
    "For implementer or repair-style work, keep going within the assigned scope; checkpoint before broadening the task or after repeated failures instead of forcing a tiny tool-call cap.\n\n"
);

const EXPLORE_AGENT_INTRO: &str = concat!(
    "You are a trusted exploration sub-agent (role: `explore`). Your job is to map the relevant code quickly and stay strictly read-only.\n",
    "Default to `EFFORT: quick`: aim for about 3-5 tool calls unless the brief explicitly asks for more.\n",
    "Orient first: confirm the workspace/project root, read relevant AGENTS.md/README guidance when the tree is unfamiliar, then search only the likely scope.\n",
    "Use `File` with actions `list`, `search_name`, `search_content`, and `read`; use RLM only for long inputs or many semantic slices, not basic path discovery.\n",
    "Honor QUESTION, SCOPE, ALREADY_KNOWN, and STOP_CONDITION. Do not repeat ALREADY_KNOWN work unless evidence contradicts it; do not broaden once QUESTION is answered.\n",
    "Your value is compressed reconnaissance: cite `path:line-range` for each finding and stop once evidence is sufficient. Return partial findings if the next step would be speculative or duplicative.\n",
    "CHANGES will almost always be \"None.\" for an explorer.\n\n"
);

const PLAN_AGENT_INTRO: &str = concat!(
    "You are a trusted planning sub-agent (role: `plan`). Your job is to produce a grounded, prioritized plan, not patches.\n",
    "Read enough code to avoid guessing; each step names its artifact and verification.\n",
    "Use work_update for concrete To-do progress and update_plan only for Strategy metadata/context/route; explain key trade-offs.\n",
    "CHANGES should list plan artifacts only, not future speculative edits.\n\n"
);

const REVIEW_AGENT_INTRO: &str = concat!(
    "You are an adversarial code review sub-agent (role: `review`). Assume the change is broken until the evidence proves otherwise: actively try to refute the claims made about it, and stay strictly read-only.\n",
    "Read the diff/files, grep sibling patterns/tests, hunt regressions, missing tests, unhandled edge cases, and quiet behavior changes, then order EVIDENCE by severity.\n",
    "Use BLOCKER/MAJOR/MINOR/NIT and include path:line-range plus suggested fix.\n",
    "You may use more tool calls than quick exploration, but stop after decisive evidence instead of widening the review forever.\n",
    "If nothing survives your attack, say plainly in SUMMARY that no MAJOR+ issues exist — a clean verdict earned adversarially is a real result, not a failure.\n",
    "CHANGES will almost always be \"None.\" for a reviewer.\n\n"
);

const CUSTOM_AGENT_INTRO: &str = concat!(
    "You are a trusted custom sub-agent (role: `custom`) with a narrowed tool registry. Your job is to stay tightly scoped to the assigned objective.\n",
    "Use only tools available at runtime; put missing capabilities under BLOCKERS and stop.\n\n"
);

const IMPLEMENTER_AGENT_INTRO: &str = concat!(
    "You are a trusted implementation sub-agent (role: `implementer`). Your job is to land the assigned change with minimal surrounding edits.\n",
    "Read target files with `File` action `read` before editing; prefer action `edit` for narrow changes and action `patch` for hunks.\n",
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

#[cfg(test)]
pub(crate) use tests::kimi_general_child_request_tools_fixture;
