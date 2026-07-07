//! Core engine for `DeepSeek` CLI.
//!
//! The engine handles all AI interactions in a background task,
//! communicating with the UI via channels. This enables:
//! - Non-blocking UI during API calls
//! - Real-time streaming updates
//! - Proper cancellation support
//! - Tool execution orchestration

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use codewhale_execpolicy::{AskForApproval, ExecPolicyContext};
use codewhale_protocol::runtime::DynamicToolSpec;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use serde_json::{Value, json};
use tokio::sync::{Mutex as AsyncMutex, RwLock, mpsc};
use tokio_util::sync::CancellationToken;

use crate::client::DeepSeekClient;
use crate::compaction::{
    CompactionConfig, compact_messages_safe, merge_system_prompts, should_compact,
};
use crate::config::{ApiProvider, Config, DEFAULT_MAX_SUBAGENTS, DEFAULT_TEXT_MODEL};
use crate::error_taxonomy::{ErrorCategory, ErrorEnvelope, StreamError};
use crate::features::{Feature, Features};
use crate::llm_client::LlmClient;
use crate::mcp::McpPool;
#[cfg(test)]
use crate::models::ToolCaller;
use crate::models::{
    ContentBlock, ContentBlockStart, Delta, Message, MessageRequest, StreamEvent, SystemPrompt,
    Tool, Usage,
};
use crate::prompts;
use crate::purge::{emit_purge_completed, emit_purge_failed, emit_purge_started, run_purge};
use codewhale_extension::ExtensionManager;
use crate::resource_telemetry::ResourceTelemetry;
use crate::route_runtime::resolve_runtime_route;
use crate::seam_manager::{SeamConfig, SeamManager};
use crate::tools::goal::{GoalSnapshot, GoalStatus, SharedGoalState, new_shared_goal_state};
use crate::tools::plan::{PlanSnapshot, SharedPlanState, new_shared_plan_state};
use crate::tools::shell::{SharedShellManager, new_shared_shell_manager};
use crate::tools::spec::RuntimeToolServices;
use crate::tools::spec::{ApprovalRequirement, ToolError, ToolResult};
use crate::tools::subagent::{
    Mailbox, MailboxMessage, SharedSubAgentManager, SubAgentCompletion, SubAgentForkContext,
    SubAgentResult, SubAgentRuntime, SubAgentStatus, SubAgentThinking, SubAgentType,
    new_shared_subagent_manager_with_timeout, resolve_subagent_assignment_route,
};
use crate::tools::todo::{SharedTodoList, TodoListSnapshot, new_shared_todo_list};
use crate::tools::user_input::{UserInputRequest, UserInputResponse};
use crate::tools::{ToolContext, ToolRegistryBuilder};
use crate::tui::app::AppMode;
use crate::utils::spawn_supervised;
use crate::worker_profile::{ModelRoute, WorkerRuntimeProfile};
use crate::working_set::WorkingSet;

use super::events::{Event, TurnOutcomeStatus};
use super::ops::{
    Op, ProviderRuntimeStatus, SessionSnapshot, USER_SHELL_TOOL_ID_PREFIX, UserInputProvenance,
};
use super::session::Session;
use super::tool_parser;
use super::turn::{TurnContext, post_turn_snapshot, pre_turn_snapshot};

/// Snapshot of parent state that can be passed to forked sub-agents without
/// rewriting the parent transcript.
#[derive(Debug, Clone, Default)]
struct StructuredState {
    mode_label: String,
    workspace: PathBuf,
    cwd: Option<PathBuf>,
    working_set_summary: Option<String>,
    todo_snapshot: Option<TodoListSnapshot>,
    plan_snapshot: Option<PlanSnapshot>,
    subagent_snapshots: Vec<SubAgentResult>,
}

impl StructuredState {
    async fn capture(
        mode_label: impl Into<String>,
        workspace: PathBuf,
        cwd: Option<PathBuf>,
        working_set: &WorkingSet,
        todos: &SharedTodoList,
        plan_state: &SharedPlanState,
        subagents: Option<&SharedSubAgentManager>,
    ) -> Self {
        let working_set_summary = working_set.summary_block(&workspace);

        let todo_snapshot = {
            let guard = todos.lock().await;
            let snap = guard.snapshot();
            if snap.items.is_empty() {
                None
            } else {
                Some(snap)
            }
        };

        let plan_snapshot = {
            let guard = plan_state.lock().await;
            if guard.is_empty() {
                None
            } else {
                Some(guard.snapshot())
            }
        };

        let subagent_snapshots = if let Some(handle) = subagents {
            let mut guard = handle.write().await;
            guard.cleanup(Duration::from_secs(60 * 60));
            guard
                .list()
                .into_iter()
                .filter(|s| matches!(s.status, SubAgentStatus::Running))
                .collect()
        } else {
            Vec::new()
        };

        Self {
            mode_label: mode_label.into(),
            workspace,
            cwd,
            working_set_summary,
            todo_snapshot,
            plan_snapshot,
            subagent_snapshots,
        }
    }

    #[must_use]
    fn to_system_block(&self) -> Option<String> {
        let mut out = String::new();
        out.push_str("## Fork State\n\n");
        out.push_str(&format!("- Mode: `{}`\n", self.mode_label));
        out.push_str(&format!("- Workspace: `{}`\n", self.workspace.display()));
        if let Some(cwd) = self.cwd.as_ref() {
            out.push_str(&format!("- Cwd: `{}`\n", cwd.display()));
        }

        if self.todo_snapshot.is_some() || self.plan_snapshot.is_some() {
            out.push_str("\n### Work\n");
        }

        if let Some(todos) = self.todo_snapshot.as_ref() {
            out.push_str(&format!(
                "\nChecklist ({}% complete)\n",
                todos.completion_pct
            ));
            for item in &todos.items {
                let marker = match item.status {
                    crate::tools::todo::TodoStatus::Pending => "[ ]",
                    crate::tools::todo::TodoStatus::InProgress => "[~]",
                    crate::tools::todo::TodoStatus::Completed => "[x]",
                };
                out.push_str(&format!("- {marker} {}\n", item.content));
            }
        }

        if let Some(plan) = self.plan_snapshot.as_ref() {
            out.push_str("\nStrategy metadata\n");
            append_plan_field(&mut out, "Title", plan.title.as_deref());
            append_plan_field(&mut out, "Objective", plan.objective.as_deref());
            append_plan_field(&mut out, "Context", plan.context_summary.as_deref());
            append_plan_field(&mut out, "Explanation", plan.explanation.as_deref());
            append_plan_list(&mut out, "Source", &plan.sources_used);
            append_plan_list(&mut out, "Critical file", &plan.critical_files);
            append_plan_list(&mut out, "Constraint", &plan.constraints);
            append_plan_field(
                &mut out,
                "Recommended approach",
                plan.recommended_approach.as_deref(),
            );
            append_plan_field(
                &mut out,
                "Verification plan",
                plan.verification_plan.as_deref(),
            );
            append_plan_field(
                &mut out,
                "Risks and unknowns",
                plan.risks_and_unknowns.as_deref(),
            );
            append_plan_field(&mut out, "Handoff packet", plan.handoff_packet.as_deref());
            for item in &plan.items {
                let marker = match item.status {
                    crate::tools::plan::StepStatus::Pending => "[ ]",
                    crate::tools::plan::StepStatus::InProgress => "[~]",
                    crate::tools::plan::StepStatus::Completed => "[x]",
                };
                out.push_str(&format!("- {marker} {}\n", item.step));
            }
        }

        if !self.subagent_snapshots.is_empty() {
            out.push_str("\n### Open Sub-Agents\n");
            for s in &self.subagent_snapshots {
                let role = s.assignment.role.as_deref().unwrap_or("-");
                let goal = if s.assignment.objective.is_empty() {
                    "(no objective set)"
                } else {
                    s.assignment.objective.as_str()
                };
                out.push_str(&format!("- `{}` (role: {}) - {}\n", s.agent_id, role, goal));
            }
        }

        if let Some(working_set) = self.working_set_summary.as_deref() {
            out.push('\n');
            out.push_str(working_set);
            out.push('\n');
        }

        Some(out)
    }
}

fn append_plan_field(out: &mut String, label: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        out.push_str(&format!("- {label}: {value}\n"));
    }
}

fn append_plan_list(out: &mut String, label: &str, values: &[String]) {
    for value in values {
        let value = value.trim();
        if !value.is_empty() {
            out.push_str(&format!("- {label}: {value}\n"));
        }
    }
}

// === Types ===

/// Configuration for the engine
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Model identifier to use for responses.
    pub model: String,
    /// Route/offering limits for the active provider+model, when the runtime
    /// route resolver had concrete catalog facts.
    pub active_route_limits: Option<codewhale_config::route::RouteLimits>,
    /// Workspace root for tool execution and file operations.
    pub workspace: PathBuf,
    /// Allow shell tool execution when true.
    pub allow_shell: bool,
    /// Enable trust mode (skip approvals) when true.
    pub trust_mode: bool,
    /// Path to the notes file used by the notes tool.
    pub notes_path: PathBuf,
    /// Path to the MCP configuration file.
    pub mcp_config_path: PathBuf,
    /// Directory containing discoverable skills.
    pub skills_dir: PathBuf,
    /// Restrict skill discovery to CodeWhale-owned roots plus explicit
    /// `skills_dir` configuration.
    pub skills_scan_codewhale_only: bool,
    /// Sources injected as `<instructions source="…">` blocks in the system
    /// prompt (#454). Each entry is either a disk path (read at render time)
    /// or an inline string. Loaded in declared order from the user's
    /// `instructions = [...]` config or constructed by embedders.
    ///
    /// Generalized from `Vec<PathBuf>` so embedders can inject inline content
    /// without staging a disk file. `From<PathBuf>` impl keeps existing callers
    /// working with `.into()` at the call site.
    pub instructions: Vec<crate::prompts::InstructionSource>,
    pub project_context_pack_enabled: bool,
    /// When true, the model is instructed to respond in the current locale
    /// and a post-hoc translation layer replaces remaining English output.
    pub translation_enabled: bool,
    /// Whether user-visible transcript rendering shows thinking blocks.
    /// Prompt assembly uses this to avoid localizing hidden reasoning.
    pub show_thinking: bool,
    pub verbosity: Option<String>,
    /// Maximum number of assistant steps before stopping.
    pub max_steps: u32,
    /// Maximum number of concurrently active subagents.
    pub max_subagents: usize,
    /// Maximum queued + running sub-agents admitted for this engine session.
    pub max_admitted_subagents: usize,
    /// Number of direct (depth-1) sub-agents that may execute concurrently
    /// before further launches queue for a launch slot (#3095).
    /// Resolved from `[subagents] launch_concurrency`.
    pub launch_concurrency: usize,
    /// Whether the model-facing `agent` tool is available after applying
    /// feature flags and `[subagents]` opt-out controls.
    pub subagents_enabled: bool,
    /// Feature flags controlling tool availability.
    pub features: Features,
    /// Deterministic auto-review policy for tool calls.
    pub auto_review_policy: crate::tui::auto_review::AutoReviewPolicy,
    /// Auto-compaction settings for long conversations.
    pub compaction: CompactionConfig,
    /// Shared Todo list state.
    pub todos: SharedTodoList,
    /// Shared Plan state.
    pub plan_state: SharedPlanState,
    /// Shared runtime goal state for model-visible goal tools.
    pub goal_state: SharedGoalState,
    /// Maximum sub-agent recursion depth (default 3). See
    /// `SubAgentRuntime::max_spawn_depth`. Override via
    /// `[subagents] max_depth = N` in `~/.codewhale/config.toml`.
    pub max_spawn_depth: u32,
    /// Optional aggregate token budget for each root sub-agent run.
    /// Descendant agents inherit the root pool unless a child starts a new
    /// budget scope with an explicit per-call override.
    pub subagent_token_budget: Option<u64>,
    /// Per-domain network policy decider (#135). Shared across the session so
    /// session-scoped approvals (`/network allow <host>`) persist for the
    /// remainder of the run.
    pub network_policy: Option<crate::network_policy::NetworkPolicyDecider>,
    /// Whether to take side-git workspace snapshots before/after each turn.
    pub snapshots_enabled: bool,
    /// Maximum workspace size (in bytes) before snapshots self-disable on
    /// first init. `0` disables the cap. Resolved from
    /// `[snapshots] max_workspace_gb` × 1 GB at engine construction.
    pub snapshots_max_workspace_bytes: u64,
    /// Post-edit LSP diagnostics injection (#136). When `None`, the engine
    /// constructs a disabled manager so the field is always present.
    pub lsp_config: Option<crate::lsp::LspConfig>,
    /// Durable runtime services exposed to model-visible tools.
    pub runtime_services: RuntimeToolServices,
    /// Per-role/type sub-agent model overrides already resolved from config.
    pub subagent_model_overrides: HashMap<String, String>,
    /// Whether the user-memory feature is enabled (#489). When `true` the
    /// engine reads `memory_path` on each prompt assembly and prepends a
    /// `<user_memory>` block to the system prompt.
    pub memory_enabled: bool,
    /// When `true`, the legacy `memory.rs` push/inject path is deprecated
    /// in favour of Moraine MCP recall. `compose_block` returns `None`
    /// regardless of `memory_enabled`, the `remember` tool is not
    /// registered, and `# foo` quick-add falls through.
    pub moraine_fallback: bool,
    /// Path to the user memory file (#489). Always populated; only
    /// consulted when `memory_enabled` is `true`.
    pub memory_path: PathBuf,
    /// Default directory for Xiaomi MiMo speech/TTS tool outputs.
    pub speech_output_dir: Option<PathBuf>,
    pub vision_config: Option<crate::config::VisionModelConfig>,
    pub goal_objective: Option<String>,
    pub goal_token_budget: Option<u32>,
    pub goal_status: GoalStatus,
    /// Tool restriction from custom slash command frontmatter.
    /// `None` means the current turn may use the normal tool set.
    pub allowed_tools: Option<Vec<String>>,
    /// Tool deny-list.  Deny always wins over allow (#3027).
    /// `None` means no tools are explicitly denied.
    pub disallowed_tools: Option<Vec<String>>,
    /// Hook executor for control-plane hooks.
    /// `ToolCallBefore` hooks may deny a tool call with exit code 2.
    pub hook_executor: Option<std::sync::Arc<crate::hooks::HookExecutor>>,
    /// Resolved BCP-47 locale tag (e.g. `"en"`, `"zh-Hans"`, `"ja"`)
    /// for the `## Environment` block in the system prompt. The
    /// caller resolves this from `Settings` once at engine
    /// construction; the engine never touches disk for it.
    pub locale_tag: String,
    /// When true, force `tool_choice: "required"` and opt compatible function
    /// schemas into DeepSeek beta strict mode.
    pub strict_tool_mode: bool,
    /// Workshop / large-tool-output routing (#548). `None` disables routing.
    pub workshop: Option<crate::tools::large_output_router::WorkshopConfig>,
    /// Which search backend `web_search` should use. Default: DuckDuckGo.
    pub search_provider: crate::config::SearchProvider,
    /// API key for Tavily, Bocha, Metaso, or Baidu. `None` for Bing or DuckDuckGo.
    /// Metaso also falls back to `METASO_API_KEY` env var, then a built-in key.
    /// Baidu also falls back to `BAIDU_SEARCH_API_KEY`.
    pub search_api_key: Option<String>,
    /// Optional DuckDuckGo-compatible HTML endpoint override.
    pub search_base_url: Option<String>,
    /// Per-step DeepSeek API timeout for sub-agent `create_message` requests.
    /// Resolved from `[subagents] api_timeout_secs` (clamped to 1..=1800)
    /// once at engine construction, then threaded onto every
    /// `SubAgentRuntime` the engine builds (#1806, #1808).
    pub subagent_api_timeout: Duration,
    /// Per-SSE-chunk idle timeout for streamed model responses.
    /// Resolved from `[tui].stream_chunk_timeout_secs` (or the legacy
    /// `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS`) and updated live by `/config`.
    pub stream_chunk_timeout: Duration,
    /// No-progress heartbeat timeout for live sub-agents. Used by the manager
    /// and parent wait loop to auto-cancel stuck children before they exhaust
    /// the sub-agent slot pool indefinitely (#2614).
    pub subagent_heartbeat_timeout: Duration,
    /// Native tools that should stay in the model-visible catalog even when
    /// they are outside the small default core surface (#2076).
    pub tools_always_load: HashSet<String>,
    /// When true and `/usr/bin/bwrap` is present on Linux, route exec_shell
    /// through bubblewrap instead of relying solely on Landlock (#2184).
    #[allow(dead_code)] // Wired through ShellManager in follow-up PR
    pub prefer_bwrap: bool,
    /// Tool override and plugin configuration (`[tools]` table in config.toml).
    /// Applied to the per-turn tool registry after built-in tools are registered.
    /// When `None`, no overrides or plugin loading occurs.
    pub tools: Option<crate::config::ToolsConfig>,
    /// Whether tools should follow symbolic links. When `true`, symlinked
    /// directories are traversed by walk-based tools and symlinked paths
    /// that resolve outside the workspace are still allowed (the symlink
    /// itself must be inside the workspace). Mirrors the
    /// `workspace_follow_symlinks` setting.
    pub workspace_follow_symlinks: bool,
    /// Ask-only permission rules loaded from sibling `permissions.toml`.
    pub exec_policy_engine: codewhale_execpolicy::ExecPolicyEngine,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_TEXT_MODEL.to_string(),
            active_route_limits: None,
            workspace: PathBuf::from("."),
            allow_shell: true,
            trust_mode: false,
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            skills_dir: crate::skills::default_skills_dir(),
            skills_scan_codewhale_only: false,
            instructions: Vec::new(),
            project_context_pack_enabled: true,
            translation_enabled: false,
            show_thinking: true,
            // High backstop rather than a working ceiling: the in-turn
            // loop_guard that used to brake repetition is gone, so this only
            // exists to terminate a pathological runaway turn via
            // `at_max_steps()`. 1000 stays high enough to never gate real work
            // while still guaranteeing the turn ends.
            max_steps: 1000,
            max_subagents: DEFAULT_MAX_SUBAGENTS,
            max_admitted_subagents: DEFAULT_MAX_SUBAGENTS,
            launch_concurrency: DEFAULT_MAX_SUBAGENTS,
            subagents_enabled: true,
            features: Features::with_defaults(),
            auto_review_policy: crate::tui::auto_review::AutoReviewPolicy::default(),
            compaction: CompactionConfig::default(),
            todos: new_shared_todo_list(),
            plan_state: new_shared_plan_state(),
            goal_state: new_shared_goal_state(),
            max_spawn_depth: crate::tools::subagent::DEFAULT_MAX_SPAWN_DEPTH,
            subagent_token_budget: None,
            network_policy: None,
            snapshots_enabled: true,
            snapshots_max_workspace_bytes:
                crate::snapshot::DEFAULT_MAX_WORKSPACE_BYTES_FOR_SNAPSHOT,
            lsp_config: None,
            runtime_services: RuntimeToolServices::default(),
            subagent_model_overrides: HashMap::new(),
            memory_enabled: false,
            moraine_fallback: false,
            memory_path: PathBuf::from("./memory.md"),
            speech_output_dir: None,
            vision_config: None,
            strict_tool_mode: false,
            goal_objective: None,
            goal_token_budget: None,
            goal_status: GoalStatus::Active,
            allowed_tools: None,
            disallowed_tools: None,
            hook_executor: None,
            locale_tag: "en".to_string(),
            workshop: None,
            search_provider: crate::config::SearchProvider::default(),
            search_api_key: None,
            search_base_url: None,
            subagent_api_timeout: Duration::from_secs(
                crate::config::DEFAULT_SUBAGENT_API_TIMEOUT_SECS,
            ),
            stream_chunk_timeout: Duration::from_secs(
                crate::config::DEFAULT_STREAM_CHUNK_TIMEOUT_SECS,
            ),
            subagent_heartbeat_timeout: Duration::from_secs(
                crate::config::DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS,
            ),
            tools_always_load: HashSet::new(),
            prefer_bwrap: false,
            verbosity: None,
            tools: None,
            workspace_follow_symlinks: false,
            exec_policy_engine: codewhale_execpolicy::ExecPolicyEngine::new(Vec::new(), Vec::new()),
        }
    }
}

/// Reason the active turn was cancelled. The token from `tokio_util`
/// does not carry a cause, so the engine keeps a sibling latch for
/// approval and user-input waits that need to explain cancellation.
///
/// `External`, `Preempted`, and `Internal` are reserved for the
/// remaining direct cancellation paths tracked in #1541.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum CancelReason {
    /// User-initiated cancel (Esc, `/cancel`, click cancel on modal).
    User,
    /// External / runtime-API cancel (HTTP `DELETE /v1/threads/...`,
    /// task manager stop, parent agent cancel).
    External,
    /// Cancel triggered when a new turn starts before the previous one
    /// finished — e.g. plain Enter while busy after the queueing path
    /// pre-empts the running turn.
    Preempted,
    /// Engine internals tore down the turn (drop, channel close,
    /// shutdown). Rare — surfaced as an internal error.
    Internal,
}

impl CancelReason {
    fn describe(self) -> &'static str {
        match self {
            Self::User => "user cancelled the request",
            Self::External => "request cancelled by external caller",
            Self::Preempted => "request was preempted by a new turn",
            Self::Internal => "engine torn down before approval resolved",
        }
    }
}

/// Handle to communicate with the engine
#[derive(Clone)]
pub struct EngineHandle {
    /// Send operations to the engine
    pub tx_op: mpsc::Sender<Op>,
    /// Receive events from the engine
    pub rx_event: Arc<RwLock<mpsc::Receiver<Event>>>,
    /// Shared pointer to the cancellation token for the current request.
    cancel_token: Arc<StdMutex<CancellationToken>>,
    /// Latched reason for the most recent cancellation. Read by the
    /// approval / user-input handlers to enrich their error strings.
    /// Cleared by the engine when a fresh turn starts.
    cancel_reason: Arc<StdMutex<Option<CancelReason>>>,
    /// Send approval decisions to the engine
    tx_approval: mpsc::Sender<ApprovalDecision>,
    /// Send user input responses to the engine
    tx_user_input: mpsc::Sender<UserInputDecision>,
    /// Send steer input for an in-flight turn.
    tx_steer: mpsc::Sender<String>,
    /// Shared pause flag set by the TUI and read by the turn loop.
    shared_paused: Arc<StdMutex<bool>>,
}

// `impl EngineHandle { ... }` moved to `engine/handle.rs` so the
// mailbox API can be reviewed independently of the engine internals.

// === Engine ===

/// The core engine that processes operations and emits events
pub struct Engine {
    config: EngineConfig,
    api_config: Config,
    deepseek_client: Option<DeepSeekClient>,
    deepseek_client_error: Option<String>,
    api_key_env_only_recovery: Option<String>,
    session: Session,
    subagent_manager: SharedSubAgentManager,
    shell_manager: SharedShellManager,
    mcp_pool: Option<Arc<AsyncMutex<McpPool>>>,
    api_provider: ApiProvider,
    active_route_limits: Option<codewhale_config::route::RouteLimits>,
    rx_op: mpsc::Receiver<Op>,
    /// Clone of the op-channel sender, so the engine can self-dispatch ops
    /// (e.g. a goal-continuation `SendMessage` after a turn completes).
    tx_op: mpsc::Sender<Op>,
    rx_approval: mpsc::Receiver<ApprovalDecision>,
    rx_user_input: mpsc::Receiver<UserInputDecision>,
    rx_steer: mpsc::Receiver<String>,
    tx_event: mpsc::Sender<Event>,
    /// Wakeup channel for the parent turn loop when a direct child sub-agent
    /// terminates (issue #756). Cloned into `SubAgentRuntime` so the runtime
    /// can fan completion events back into the engine.
    tx_subagent_completion: mpsc::UnboundedSender<SubAgentCompletion>,
    /// Receiver paired with `tx_subagent_completion`. Drained at the
    /// turn-loop's empty-tool_uses branch to surface `<codewhale:subagent.done>`
    /// sentinels into the parent's transcript before deciding to end the turn.
    pub(super) rx_subagent_completion: mpsc::UnboundedReceiver<SubAgentCompletion>,
    /// Sub-agent completions already injected into the parent transcript.
    /// Channel delivery and watchdog reconciliation both mark this set so a
    /// dropped event can be synthesized once without duplicating a later
    /// delivery.
    delivered_subagent_completion_ids: HashSet<String>,
    cancel_token: CancellationToken,
    shared_cancel_token: Arc<StdMutex<CancellationToken>>,
    /// Latched reason for the current cancellation, mirrored to
    /// `EngineHandle::cancel_reason`. Read by `approval.rs` when
    /// surfacing the "Request cancelled while awaiting …" error so the
    /// user-facing message names a cause.
    pub(super) cancel_reason: Arc<StdMutex<Option<CancelReason>>>,
    tool_exec_lock: Arc<RwLock<()>>,
    /// Append-only layered context manager (#159). Opt-in for v0.7.5 while
    /// cache-hit behavior is audited.
    seam_manager: Option<SeamManager>,
    turn_counter: u64,
    /// Post-edit LSP diagnostics injection (#136). Populated unconditionally
    /// — when LSP is disabled in config, this is an inert manager that
    /// always returns `None` from `diagnostics_for`.
    lsp_manager: Arc<crate::lsp::LspManager>,
    /// Session-scoped workshop variable store (#548). Shared across all tool
    /// calls so `last_tool_result` persists within the session and can be
    /// promoted to the parent context via `promote_to_context`.
    workshop_vars: Option<
        std::sync::Arc<tokio::sync::Mutex<crate::tools::large_output_router::WorkshopVariables>>,
    >,
    /// External sandbox backend (#516). When `Some`, exec_shell routes commands
    /// through this instead of spawning a local process.
    sandbox_backend: Option<std::sync::Arc<dyn crate::sandbox::backend::SandboxBackend>>,
    /// Diagnostics collected during the current step's tool calls. Drained
    /// and forwarded as a synthetic user message before the next API call.
    pending_lsp_blocks: Vec<crate::lsp::DiagnosticBlock>,
    /// Cached SlopLedger gate block keyed by the ledger file's modified time.
    /// This keeps prompt refreshes cheap while still noticing append/update
    /// writes from slop ledger tools during the same session.
    slop_ledger_gate_cache: Option<(Option<SystemTime>, Option<String>)>,
    /// Current operating mode. Updated on `ChangeMode` and `SendMessage`.
    current_mode: AppMode,
    /// Process-local cache for `estimated_input_tokens`. Memoizes the most
    /// recent token estimate keyed on `(session.messages_revision,
    /// system_prompt_fingerprint)`. Five call sites per turn consult this
    /// (engine capacity checkpoints, seam manager, trim budget, etc.) plus
    /// four TUI / command consumers; the cache turns N×O(messages) walks
    /// into a single recompute on a content change.
    token_estimate_cache: TokenEstimateCache,
    /// Shared pause flag set by the TUI and read before tool execution.
    shared_paused: Arc<StdMutex<bool>>,
    extension_manager: std::sync::Arc<codewhale_extension::ExtensionManager>,
}

// === Internal tool helpers ===

fn subagent_mailbox_message_is_best_effort(message: &MailboxMessage) -> bool {
    matches!(
        message,
        MailboxMessage::Progress { .. }
            | MailboxMessage::ToolCallStarted { .. }
            | MailboxMessage::ToolCallCompleted { .. }
    )
}

const SUBAGENT_MAILBOX_BEST_EFFORT_MIN_INTERVAL: Duration = Duration::from_millis(100);

fn subagent_mailbox_best_effort_send_permitted(
    last_sent_at: &mut HashMap<String, Instant>,
    message: &MailboxMessage,
    now: Instant,
) -> bool {
    if !subagent_mailbox_message_is_best_effort(message) {
        return true;
    }

    let agent_id = message.agent_id().to_string();
    if last_sent_at
        .get(&agent_id)
        .is_some_and(|last| now.duration_since(*last) < SUBAGENT_MAILBOX_BEST_EFFORT_MIN_INTERVAL)
    {
        return false;
    }

    last_sent_at.insert(agent_id, now);
    true
}

impl Engine {
    fn mode_runtime_instructions(mode: AppMode) -> &'static str {
        match mode {
            AppMode::Agent | AppMode::Auto => prompts::AGENT_MODE,
            AppMode::Plan => prompts::PLAN_MODE,
            AppMode::Yolo => prompts::YOLO_MODE,
        }
        .trim()
    }

    pub(super) async fn emit_compaction_started(
        &mut self,
        id: String,
        auto: bool,
        message: String,
    ) {
        let _ = self
            .tx_event
            .send(Event::CompactionStarted { id, auto, message })
            .await;
    }

    pub(super) async fn emit_compaction_completed(
        &mut self,
        id: String,
        auto: bool,
        message: String,
        messages_before: Option<usize>,
        messages_after: Option<usize>,
    ) {
        let _ = self
            .tx_event
            .send(Event::CompactionCompleted {
                id,
                auto,
                message,
                messages_before,
                messages_after,
            })
            .await;
    }

    pub(super) async fn emit_compaction_failed(&mut self, id: String, auto: bool, message: String) {
        let _ = self
            .tx_event
            .send(Event::CompactionFailed { id, auto, message })
            .await;
    }

    fn reset_cancel_token(&mut self) {
        let token = CancellationToken::new();
        self.cancel_token = token.clone();
        match self.shared_cancel_token.lock() {
            Ok(mut shared) => {
                *shared = token;
            }
            Err(poisoned) => {
                *poisoned.into_inner() = token;
            }
        }
        // Fresh turn → clear any latched cancellation reason from the
        // previous turn so a downstream "request cancelled" message
        // doesn't inherit a stale cause.
        match self.cancel_reason.lock() {
            Ok(mut slot) => *slot = None,
            Err(poisoned) => *poisoned.into_inner() = None,
        }
        match self.shared_paused.lock() {
            Ok(mut paused) => *paused = false,
            Err(poisoned) => *poisoned.into_inner() = false,
        }
    }

    fn env_only_api_key_recovery_hint(api_config: &Config) -> Option<String> {
        if !crate::config::active_provider_uses_env_only_api_key(api_config) {
            return None;
        }

        let provider = api_config.api_provider();
        let env_var = provider.env_vars_label();

        Some(format!(
            "The rejected key came from {env_var}; no saved config key is present.\n\
             Run `codewhale auth status` to inspect credential sources, then \
             `codewhale auth set --provider {provider}` to save a valid key in ~/.codewhale/config.toml, \
             or remove the stale export and open a fresh shell.",
            provider = provider.as_str()
        ))
    }

    pub(super) fn decorate_auth_error_message(&self, message: String) -> String {
        let Some(hint) = self.api_key_env_only_recovery.as_ref() else {
            return message;
        };
        if crate::error_taxonomy::classify_error_message(&message) != ErrorCategory::Authentication
            || message.contains("no saved config key is present")
        {
            return message;
        }
        format!("{message}\n\n{hint}")
    }

    fn activate_runtime_route(&mut self, provider: ApiProvider, model: &str) -> Result<(), String> {
        if self.api_provider == provider
            && self
                .deepseek_client
                .as_ref()
                .is_some_and(|client| client.api_provider() == provider)
        {
            return Ok(());
        }

        let route =
            resolve_runtime_route(&self.api_config, provider, Some(model)).map_err(|reason| {
                format!(
                    "Failed to resolve provider route {} / {}: {reason}",
                    provider.as_str(),
                    model
                )
            })?;
        let route_config = route.config;
        match DeepSeekClient::from_candidate(&route_config, &route.candidate) {
            Ok(client) => {
                self.api_provider = provider;
                self.api_config = route_config;
                self.active_route_limits =
                    crate::route_budget::known_route_limits(route.candidate.limits);
                self.api_key_env_only_recovery =
                    Self::env_only_api_key_recovery_hint(&self.api_config);
                self.deepseek_client = Some(client.clone());
                self.deepseek_client_error = None;
                self.seam_manager = self
                    .seam_manager
                    .as_ref()
                    .filter(|manager| manager.config().enabled)
                    .map(|manager| SeamManager::new(client, manager.config().clone()));
                Ok(())
            }
            Err(err) => Err(format!(
                "Failed to configure provider route {} / {}: {err}",
                provider.as_str(),
                model
            )),
        }
    }

    /// Create a new engine with the given configuration
    pub fn new(config: EngineConfig, api_config: &Config) -> (Self, EngineHandle) {
        crate::tls::ensure_rustls_crypto_provider();

        if let Some(objective) = normalized_goal_objective(config.goal_objective.as_deref()) {
            sync_goal_state_from_host(
                &config.goal_state,
                Some(&objective),
                config.goal_token_budget,
                config.goal_status,
            );
        }

        let (tx_op, rx_op) = mpsc::channel(32);
        let (tx_event, rx_event) = mpsc::channel(256);
        let (tx_approval, rx_approval) = mpsc::channel(64);
        let (tx_user_input, rx_user_input) = mpsc::channel(32);
        let (tx_steer, rx_steer) = mpsc::channel(64);
        let (tx_subagent_completion, rx_subagent_completion) = mpsc::unbounded_channel();
        let cancel_token = CancellationToken::new();
        let shared_cancel_token = Arc::new(StdMutex::new(cancel_token.clone()));
        let cancel_reason: Arc<StdMutex<Option<CancelReason>>> = Arc::new(StdMutex::new(None));
        let shared_paused = Arc::new(StdMutex::new(false));
        let tool_exec_lock = Arc::new(RwLock::new(()));

        // Create clients for both providers
        let (deepseek_client, deepseek_client_error) = match DeepSeekClient::new(api_config) {
            Ok(client) => (Some(client), None),
            Err(err) => (None, Some(err.to_string())),
        };
        let api_provider = api_config.api_provider();
        let api_key_env_only_recovery = Self::env_only_api_key_recovery_hint(api_config);

        let mut session = Session::new(
            config.model.clone(),
            config.workspace.clone(),
            config.allow_shell,
            config.trust_mode,
            config.notes_path.clone(),
            config.mcp_config_path.clone(),
        );
        // Set up stable system prompt with project context (default to agent mode).
        // Per-turn working-set metadata is injected into the latest user
        // message at request time so file churn does not rewrite this prefix.
        let user_memory_block = crate::memory::compose_block(
            config.memory_enabled && !config.moraine_fallback, // TODO(v0.8.71): remove when Moraine recall stable; see #3490, #3495
            &config.memory_path,
        );
        let prompt_goal_objective =
            goal_objective_for_prompt(config.goal_objective.as_deref(), &config.goal_state);
        let system_prompt =
            prompts::system_prompt_for_mode_with_context_skills_session_and_approval(
                &config.workspace,
                None,
                Some(&config.skills_dir),
                Some(&config.instructions),
                prompts::PromptSessionContext {
                    user_memory_block: user_memory_block.as_deref(),
                    goal_objective: prompt_goal_objective.as_deref(),
                    project_context_pack_enabled: config.project_context_pack_enabled,
                    locale_tag: &config.locale_tag,
                    translation_enabled: config.translation_enabled,
                    model_id: &config.model,
                    context_window_override: Some(
                        crate::route_budget::route_context_window_tokens(
                            api_provider,
                            &config.model,
                            config.active_route_limits,
                        ),
                    ),
                    show_thinking: config.show_thinking,
                    verbosity: config.verbosity.as_deref(),
                    skills_scan_codewhale_only: config.skills_scan_codewhale_only,
                },
            );
        let stable_prompt = Some(system_prompt);
        session.last_system_prompt_hash = Some(system_prompt_hash(stable_prompt.as_ref()));
        session.system_prompt = stable_prompt;

        // Initialize prefix-cache stability monitor (lazy-pin).
        // The system prompt is available now but the tool catalog isn't
        // fully built until the first turn, so we start unpinned. The
        // first `check_and_update` call in the turn loop will pin the
        // fingerprint automatically.
        let _ = session.prefix_stability.get_or_insert_with(|| {
            // Use the tool registry's spec names for fingerprinting.
            // At this point tool spec builders may not be registered yet,
            // so we start with None — fingerprint will pin on first request.
            crate::prefix_cache::PrefixStabilityManager::new_unpinned()
        });

        let subagent_manager = new_shared_subagent_manager_with_timeout(
            config.workspace.clone(),
            config.max_subagents,
            config.max_admitted_subagents,
            config.subagent_heartbeat_timeout,
            config.launch_concurrency,
            config.subagent_token_budget,
        );
        let shell_manager = config
            .runtime_services
            .shell_manager
            .clone()
            .unwrap_or_else(|| new_shared_shell_manager(config.workspace.clone()));
        // Create Flash seam manager for layered context (#159). v0.7.5 keeps
        // this opt-in until the prefix-cache audit proves when seam production
        // is worth the extra request and transcript mutation.
        let seam_manager = deepseek_client.as_ref().map(|main_client| {
            let seam_config = SeamConfig {
                enabled: api_config.context.enabled.unwrap_or(false),
                verbatim_window_turns: api_config
                    .context
                    .verbatim_window_turns
                    .unwrap_or(crate::seam_manager::VERBATIM_WINDOW_TURNS),
                l1_threshold: api_config
                    .context
                    .l1_threshold
                    .unwrap_or(crate::seam_manager::DEFAULT_L1_THRESHOLD),
                l2_threshold: api_config
                    .context
                    .l2_threshold
                    .unwrap_or(crate::seam_manager::DEFAULT_L2_THRESHOLD),
                l3_threshold: api_config
                    .context
                    .l3_threshold
                    .unwrap_or(crate::seam_manager::DEFAULT_L3_THRESHOLD),
                seam_model: api_config
                    .context
                    .seam_model
                    .clone()
                    .unwrap_or_else(|| crate::seam_manager::DEFAULT_SEAM_MODEL.to_string()),
            };
            SeamManager::new(main_client.clone(), seam_config)
        });

        let lsp_manager = Arc::new(match config.lsp_config.clone() {
            Some(cfg) => crate::lsp::LspManager::new(cfg, config.workspace.clone()),
            None => crate::lsp::LspManager::disabled(),
        });

        // Workshop variable store (#548). Created unconditionally so the Arc
        // can be handed to every ToolContext; routing is gated on the router
        // field being Some rather than on the vars Arc being present.
        let workshop_vars: Option<
            std::sync::Arc<
                tokio::sync::Mutex<crate::tools::large_output_router::WorkshopVariables>,
            >,
        > = if config.workshop.is_some() {
            Some(std::sync::Arc::new(tokio::sync::Mutex::new(
                crate::tools::large_output_router::WorkshopVariables::default(),
            )))
        } else {
            None
        };

        // External sandbox backend (#516). Logged but non-fatal: if the
        // backend fails to construct, the engine continues with local
        // execution as the fallback.
        let sandbox_backend = crate::sandbox::backend::create_backend(api_config)
            .unwrap_or_else(|e| {
                tracing::warn!("Failed to create sandbox backend: {e}");
                None
            })
            .map(std::sync::Arc::from);

        let active_route_limits = config.active_route_limits;
        let engine = Engine {
            config,
            api_config: api_config.clone(),
            deepseek_client,
            deepseek_client_error,
            api_key_env_only_recovery,
            session,
            subagent_manager,
            shell_manager,
            mcp_pool: None,
            api_provider,
            active_route_limits,
            rx_op,
            tx_op: tx_op.clone(),
            rx_approval,
            rx_user_input,
            rx_steer,
            tx_event,
            tx_subagent_completion,
            rx_subagent_completion,
            delivered_subagent_completion_ids: HashSet::new(),
            cancel_token: cancel_token.clone(),
            shared_cancel_token: shared_cancel_token.clone(),
            cancel_reason: cancel_reason.clone(),
            tool_exec_lock,
            seam_manager,
            turn_counter: 0,
            lsp_manager,
            pending_lsp_blocks: Vec::new(),
            slop_ledger_gate_cache: None,
            workshop_vars,
            sandbox_backend,
            current_mode: AppMode::Agent,
            token_estimate_cache: TokenEstimateCache::new(),
            shared_paused: shared_paused.clone(),
            extension_manager: {
                let mut mgr = codewhale_extension::ExtensionManager::new();
                codewhale_tn_extension::TormentNexusExtension::register(&mut mgr);
                std::sync::Arc::new(mgr)
            },
        };
        let handle = EngineHandle {
            tx_op,
            rx_event: Arc::new(RwLock::new(rx_event)),
            cancel_token: shared_cancel_token,
            cancel_reason,
            tx_approval,
            tx_user_input,
            tx_steer,
            shared_paused,
        };

        (engine, handle)
    }

    async fn handle_run_shell_command(
        &mut self,
        command: String,
        mode: AppMode,
        allow_shell: bool,
        trust_mode: bool,
        auto_approve: bool,
        approval_mode: crate::tui::approval::ApprovalMode,
    ) {
        self.reset_cancel_token();
        self.turn_counter = self.turn_counter.saturating_add(1);

        let turn_id = format!(
            "{}{seq}",
            USER_SHELL_TOOL_ID_PREFIX,
            seq = self.turn_counter
        );
        let tool_id = turn_id.clone();
        let tool_name = "exec_shell".to_string();
        let tool_input = json!({ "command": command, "source": "user" });
        let snapshot_prompt = tool_input["command"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        self.apply_runtime_mode_policy(mode, allow_shell, trust_mode, auto_approve, approval_mode);

        let _ = self
            .tx_event
            .send(Event::TurnStarted {
                turn_id: turn_id.clone(),
            })
            .await;

        if self.config.snapshots_enabled {
            let pre_workspace = self.session.workspace.clone();
            let pre_seq = self.turn_counter;
            let pre_cap = self.config.snapshots_max_workspace_bytes;
            let pre_prompt = snapshot_prompt.clone();
            let _ = tokio::task::spawn_blocking(move || {
                pre_turn_snapshot(&pre_workspace, pre_seq, pre_cap, Some(&pre_prompt))
            })
            .await;
        }

        let _ = self
            .tx_event
            .send(Event::ToolCallStarted {
                id: tool_id.clone(),
                name: tool_name.clone(),
                input: tool_input.clone(),
            })
            .await;

        let tool_context = self.build_tool_context(mode, auto_approve);
        let registry = ToolRegistryBuilder::new()
            .with_shell_tools()
            .build(tool_context);

        let result = if mode == AppMode::Plan {
            Err(ToolError::permission_denied(
                "Tool 'exec_shell' is unavailable in Plan mode".to_string(),
            ))
        } else if !self.config.features.enabled(Feature::ShellTool) {
            Err(ToolError::not_available(
                "Tool 'exec_shell' is disabled by feature flag".to_string(),
            ))
        } else if let Some(spec) = registry.get(&tool_name) {
            let mut approval_required = spec.approval_requirement_for(&tool_input)
                != ApprovalRequirement::Auto
                && !registry.context().auto_approve;
            let mut approval_description = spec.description().to_string();
            let mut approval_force_prompt = false;
            let ask_rule_decision = exec_shell_ask_rule_decision(
                &self.config,
                &tool_name,
                &tool_input,
                &self.session.workspace,
                self.session.approval_mode,
            );
            if let Some(ToolAskRuleDecision::Prompt(reason)) = ask_rule_decision.as_ref() {
                // YOLO mode (auto_approve) is the explicit "no approvals"
                // contract: a typed ask-rule must not pop a modal in YOLO.
                // A typed deny rule still blocks hard below.
                if !self.session.auto_approve {
                    approval_required = true;
                    approval_description = reason.clone();
                    approval_force_prompt = true;
                }
            }
            if let Some(ToolAskRuleDecision::Block(reason)) = ask_rule_decision {
                Err(ToolError::permission_denied(reason))
            } else if approval_required {
                emit_tool_audit(json!({
                    "event": "tool.approval_required",
                    "tool_id": tool_id.clone(),
                    "tool_name": tool_name.clone(),
                    "source": "composer_bang",
                }));
                let approval_key =
                    crate::tools::approval_cache::build_approval_key(&tool_name, &tool_input).0;
                let approval_grouping_key =
                    crate::tools::approval_cache::build_approval_grouping_key(
                        &tool_name,
                        &tool_input,
                    )
                    .0;
                let _ = self
                    .tx_event
                    .send(Event::ApprovalRequired {
                        id: tool_id.clone(),
                        tool_name: tool_name.clone(),
                        input: tool_input.clone(),
                        description: approval_description,
                        approval_key,
                        approval_grouping_key,
                        intent_summary: None,
                        approval_force_prompt,
                    })
                    .await;

                match self.await_tool_approval(&tool_id).await {
                    Ok(ApprovalResult::Approved) => {
                        emit_tool_audit(json!({
                            "event": "tool.approval_decision",
                            "tool_id": tool_id.clone(),
                            "tool_name": tool_name.clone(),
                            "decision": "approved",
                            "source": "composer_bang",
                        }));
                        let mut result = Self::execute_tool_with_lock(
                            self.tool_exec_lock.clone(),
                            spec.supports_parallel(),
                            false,
                            self.tx_event.clone(),
                            tool_name.clone(),
                            tool_input.clone(),
                            self.session.workspace.clone(),
                            Some(&registry),
                            None,
                            None,
                        )
                        .await;
                        if let Ok(tool_result) = result.as_mut() {
                            stamp_tool_result_approval(
                                tool_result,
                                ToolApprovalStamp::ApprovedByUser,
                            );
                        }
                        result
                    }
                    Ok(ApprovalResult::Denied) => {
                        emit_tool_audit(json!({
                            "event": "tool.approval_decision",
                            "tool_id": tool_id.clone(),
                            "tool_name": tool_name.clone(),
                            "decision": "denied",
                            "source": "composer_bang",
                        }));
                        Err(ToolError::permission_denied(format!(
                            "Tool '{tool_name}' denied by user"
                        )))
                    }
                    Ok(ApprovalResult::RetryWithPolicy(policy)) => {
                        emit_tool_audit(json!({
                            "event": "tool.approval_decision",
                            "tool_id": tool_id.clone(),
                            "tool_name": tool_name.clone(),
                            "decision": "retry_with_policy",
                            "policy": format!("{policy:?}"),
                            "source": "composer_bang",
                        }));
                        let elevated_context = registry
                            .context()
                            .clone()
                            .with_elevated_sandbox_policy(policy);
                        let mut result = Self::execute_tool_with_lock(
                            self.tool_exec_lock.clone(),
                            spec.supports_parallel(),
                            false,
                            self.tx_event.clone(),
                            tool_name.clone(),
                            tool_input.clone(),
                            self.session.workspace.clone(),
                            Some(&registry),
                            None,
                            Some(elevated_context),
                        )
                        .await;
                        if let Ok(tool_result) = result.as_mut() {
                            stamp_tool_result_approval(
                                tool_result,
                                ToolApprovalStamp::ApprovedWithPolicy,
                            );
                        }
                        result
                    }
                    Err(err) => Err(err),
                }
            } else {
                Self::execute_tool_with_lock(
                    self.tool_exec_lock.clone(),
                    spec.supports_parallel(),
                    false,
                    self.tx_event.clone(),
                    tool_name.clone(),
                    tool_input.clone(),
                    self.session.workspace.clone(),
                    Some(&registry),
                    None,
                    None,
                )
                .await
            }
        } else {
            Err(ToolError::not_available(
                "tool 'exec_shell' is not registered".to_string(),
            ))
        };

        let mut result = result;
        if let Ok(tool_result) = result.as_mut()
            && let Some(path) = crate::tools::truncate::apply_spillover_with_artifact(
                tool_result,
                &tool_id,
                &tool_name,
                &self.session.id,
            )
        {
            emit_tool_audit(json!({
                "event": "tool.spillover",
                "tool_id": tool_id.clone(),
                "tool_name": tool_name.clone(),
                "path": path.display().to_string(),
                "source": "composer_bang",
            }));
        }

        let status = if result.is_err() {
            TurnOutcomeStatus::Failed
        } else {
            TurnOutcomeStatus::Completed
        };
        let error = result.as_ref().err().map(ToString::to_string);

        let _ = self
            .tx_event
            .send(Event::ToolCallComplete {
                id: tool_id,
                name: tool_name,
                result,
            })
            .await;

        let _ = self
            .tx_event
            .send(Event::TurnComplete {
                usage: Usage::default(),
                status,
                error,
                tool_catalog: None,
                base_url: None,
            })
            .await;

        if self.config.snapshots_enabled {
            let post_workspace = self.session.workspace.clone();
            let post_seq = self.turn_counter;
            let post_cap = self.config.snapshots_max_workspace_bytes;
            crate::utils::spawn_blocking_supervised("post-shell-turn-snapshot", move || {
                post_turn_snapshot(&post_workspace, post_seq, post_cap, Some(&snapshot_prompt));
            });
        }
    }

    fn apply_runtime_mode_policy(
        &mut self,
        mode: AppMode,
        allow_shell: bool,
        trust_mode: bool,
        auto_approve: bool,
        approval_mode: crate::tui::approval::ApprovalMode,
    ) {
        self.current_mode = mode;
        self.session.allow_shell = allow_shell;
        self.config.allow_shell = allow_shell;
        self.session.trust_mode = trust_mode;
        self.config.trust_mode = trust_mode;
        self.session.auto_approve = auto_approve;
        self.session.approval_mode = agent_approval_mode_for_turn(auto_approve, approval_mode);
    }

    /// Run the engine event loop
    #[allow(clippy::too_many_lines)]
    pub async fn run(mut self) {
        enum EngineRunInput {
            Operation(Op),
            SubAgentCompletion(SubAgentCompletion),
        }

        loop {
            let input = tokio::select! {
                op = self.rx_op.recv() => op.map(EngineRunInput::Operation),
                completion = self.rx_subagent_completion.recv() => {
                    completion.map(EngineRunInput::SubAgentCompletion)
                }
            };
            let Some(input) = input else {
                break;
            };

            match input {
                EngineRunInput::SubAgentCompletion(completion) => {
                    self.handle_idle_subagent_completion(completion).await;
                }
                EngineRunInput::Operation(op) => match op {
                    Op::SendMessage {
                        content,
                        mode,
                        provider,
                        model,
                        goal_objective,
                        goal_token_budget,
                        goal_status,
                        reasoning_effort,
                        reasoning_effort_auto,
                        auto_model,
                        allow_shell,
                        trust_mode,
                        auto_approve,
                        approval_mode,
                        translation_enabled,
                        show_thinking,
                        allowed_tools,
                        dynamic_tools,
                        hook_executor,
                        verbosity,
                        provenance,
                    } => {
                        self.handle_send_message(
                            content,
                            mode,
                            provider,
                            model,
                            goal_objective,
                            goal_token_budget,
                            goal_status,
                            reasoning_effort,
                            reasoning_effort_auto,
                            auto_model,
                            allow_shell,
                            trust_mode,
                            auto_approve,
                            approval_mode,
                            translation_enabled,
                            show_thinking,
                            allowed_tools,
                            dynamic_tools,
                            hook_executor,
                            verbosity,
                            provenance,
                        )
                        .await;
                    }
                    Op::RunShellCommand {
                        command,
                        mode,
                        allow_shell,
                        trust_mode,
                        auto_approve,
                        approval_mode,
                    } => {
                        self.handle_run_shell_command(
                            command,
                            mode,
                            allow_shell,
                            trust_mode,
                            auto_approve,
                            approval_mode,
                        )
                        .await;
                    }
                    Op::SetGoalStatus { status, clear } => {
                        self.handle_set_goal_status(status, clear).await;
                    }
                    Op::CancelRequest => {
                        self.cancel_token.cancel();
                        self.reset_cancel_token();
                    }
                    Op::ApproveToolCall { id } => {
                        // Tool approval handling will be implemented in tools module
                        let _ = self
                            .tx_event
                            .send(Event::status(format!("Approved tool call: {id}")))
                            .await;
                    }
                    Op::DenyToolCall { id } => {
                        let _ = self
                            .tx_event
                            .send(Event::status(format!("Denied tool call: {id}")))
                            .await;
                    }
                    Op::SpawnSubAgent { prompt } => {
                        let Some(client) = self.deepseek_client.clone() else {
                            let message = self
                                .deepseek_client_error
                                .as_deref()
                                .map(|err| format!("Failed to spawn sub-agent: {err}"))
                                .unwrap_or_else(|| {
                                    "Failed to spawn sub-agent: API client not configured"
                                        .to_string()
                                });
                            let _ = self
                                .tx_event
                                .send(Event::error(ErrorEnvelope::fatal(message)))
                                .await;
                            continue;
                        };

                        let mcp_pool = if self.config.features.enabled(Feature::Mcp) {
                            self.ensure_mcp_pool().await.ok()
                        } else {
                            None
                        };

                        let mut runtime = SubAgentRuntime::new(
                            client,
                            self.session.model.clone(),
                            // Sub-agents don't inherit YOLO mode - use Agent mode defaults
                            self.build_tool_context(AppMode::Agent, self.session.auto_approve),
                            self.session.allow_shell,
                            Some(self.tx_event.clone()),
                            Arc::clone(&self.subagent_manager),
                        )
                        .with_role_models(self.config.subagent_model_overrides.clone())
                        .with_auto_model(self.session.auto_model)
                        .with_reasoning_effort(
                            self.session.reasoning_effort.clone(),
                            self.session.reasoning_effort_auto,
                        )
                        .with_agent_tool_surface_options(self.agent_tool_surface_options(
                            shell_policy_for_mode(AppMode::Agent, self.session.allow_shell),
                        ))
                        .with_max_spawn_depth(self.config.max_spawn_depth)
                        .with_step_api_timeout(self.config.subagent_api_timeout)
                        .with_speech_output_dir(self.config.speech_output_dir.clone())
                        .with_mcp_pool(mcp_pool)
                        .with_todos(self.config.todos.clone())
                        .background_runtime();
                        let route = resolve_subagent_assignment_route(
                            &runtime,
                            None,
                            &prompt,
                            &SubAgentType::General,
                            ModelRoute::Inherit,
                            SubAgentThinking::Inherit,
                        )
                        .await;
                        runtime.model = route.model;
                        runtime.reasoning_effort = route.reasoning_effort;
                        runtime.reasoning_effort_auto = false;

                        let result = {
                            let mut manager = self.subagent_manager.write().await;
                            manager.spawn_background(
                                Arc::clone(&self.subagent_manager),
                                runtime,
                                SubAgentType::General,
                                prompt.clone(),
                                None,
                            )
                        };

                        match result {
                            Ok(snapshot) => {
                                let _ = self
                                    .tx_event
                                    .send(Event::status(format!(
                                        "Spawned sub-agent {}",
                                        snapshot.agent_id
                                    )))
                                    .await;
                            }
                            Err(err) => {
                                let _ = self
                                    .tx_event
                                    .send(Event::error(ErrorEnvelope::fatal(format!(
                                        "Failed to spawn sub-agent: {err}"
                                    ))))
                                    .await;
                            }
                        }
                    }
                    Op::ListSubAgents => {
                        // #3803: the sidebar refresh is a read-only snapshot.
                        // Render from a read lock; only take the write lock to
                        // run cleanup on a bounded cadence, so a UI refresh storm
                        // during a sub-agent fanout no longer contends for the
                        // write lock (against completions/persistence) on every
                        // request. Cleanup still auto-cancels stale agents.
                        let due = {
                            let manager = self.subagent_manager.read().await;
                            manager.cleanup_due(
                                crate::tools::subagent::SUBAGENT_LIST_CLEANUP_MIN_INTERVAL,
                            )
                        };
                        let agents = if due {
                            let mut manager = self.subagent_manager.write().await;
                            manager.cleanup(Duration::from_secs(60 * 60));
                            manager.list()
                        } else {
                            self.subagent_manager.read().await.list()
                        };
                        // #3802: use non-blocking send — this is a refresh event
                        // that can safely be dropped when the channel is full.
                        // The next drain cycle will re-request the list.
                        if let Err(_e) = self.tx_event.try_send(Event::AgentList { agents }) {
                            tracing::debug!(
                                "Event channel full; dropping ListSubAgents refresh (will retry next drain)"
                            );
                        }
                    }
                    Op::CancelSubAgent { agent_id } => {
                        let result = {
                            let mut manager = self.subagent_manager.write().await;
                            match manager.cancel_agent(&agent_id) {
                                Ok(_) => Ok(manager.list()),
                                Err(err) => Err(err),
                            }
                        };
                        match result {
                            Ok(agents) => {
                                if let Err(_e) = self.tx_event.try_send(Event::AgentList { agents })
                                {
                                    tracing::debug!(
                                        "Event channel full; dropping CancelSubAgent refresh"
                                    );
                                }
                            }
                            Err(err) => {
                                let _ =
                                    self.tx_event
                                        .try_send(Event::error(ErrorEnvelope::transient(format!(
                                            "Failed to cancel sub-agent {agent_id}: {err}"
                                        ))));
                            }
                        }
                    }
                    Op::ChangeMode {
                        mode,
                        allow_shell,
                        trust_mode,
                        auto_approve,
                        approval_mode,
                    } => {
                        self.apply_runtime_mode_policy(
                            mode,
                            allow_shell,
                            trust_mode,
                            auto_approve,
                            approval_mode,
                        );
                        self.emit_session_updated().await;
                        let _ = self
                            .tx_event
                            .send(Event::status(format!(
                                "Mode changed to: {}",
                                mode.description()
                            )))
                            .await;
                    }
                    Op::SetModel {
                        model,
                        mode: _,
                        route_limits,
                    } => {
                        self.session.auto_model = model.trim().eq_ignore_ascii_case("auto");
                        self.session.model = model;
                        self.config.model.clone_from(&self.session.model);
                        self.active_route_limits = route_limits;
                        self.refresh_system_prompt();
                        self.emit_session_updated().await;
                        let _ = self
                            .tx_event
                            .send(Event::status(format!(
                                "Model set to: {}",
                                self.session.model
                            )))
                            .await;
                    }
                    Op::SetCompaction { config } => {
                        let enabled = config.enabled;
                        self.config.compaction = config;
                        let _ = self
                            .tx_event
                            .send(Event::status(format!(
                                "Auto-compaction {}",
                                if enabled { "enabled" } else { "disabled" }
                            )))
                            .await;
                    }
                    Op::SetStreamChunkTimeout { timeout_secs } => {
                        self.config.stream_chunk_timeout = Duration::from_secs(timeout_secs);
                        let _ = self
                            .tx_event
                            .send(Event::status(format!(
                                "Stream chunk timeout set to {timeout_secs}s"
                            )))
                            .await;
                    }
                    Op::SetSubagentRuntimeConfig {
                        enabled,
                        max_subagents,
                        launch_concurrency,
                        max_spawn_depth,
                        api_timeout_secs,
                        heartbeat_timeout_secs,
                    } => {
                        self.config.subagents_enabled = enabled;
                        self.config.max_subagents =
                            max_subagents.clamp(1, crate::config::MAX_SUBAGENTS);
                        self.config.launch_concurrency =
                            launch_concurrency.clamp(1, self.config.max_subagents);
                        self.config.max_spawn_depth =
                            max_spawn_depth.min(codewhale_config::MAX_SPAWN_DEPTH_CEILING);
                        self.config.subagent_api_timeout = Duration::from_secs(api_timeout_secs);
                        self.config.subagent_heartbeat_timeout =
                            Duration::from_secs(heartbeat_timeout_secs);
                        let launch_gate_applied = {
                            let mut manager = self.subagent_manager.write().await;
                            manager.update_runtime_limits(
                                self.config.max_subagents,
                                self.config.max_admitted_subagents,
                                self.config.subagent_heartbeat_timeout,
                                self.config.launch_concurrency,
                                self.config.subagent_token_budget,
                            )
                        };
                        let launch_note = if launch_gate_applied {
                            ""
                        } else {
                            "; launch_concurrency takes full effect after active sub-agents finish or the session restarts"
                        };
                        let _ = self
                            .tx_event
                            .send(Event::status(format!(
                                "Sub-agent runtime updated: enabled={enabled}, max_subagents={}, launch_concurrency={}, max_depth={}{}",
                                self.config.max_subagents,
                                self.config.launch_concurrency,
                                self.config.max_spawn_depth,
                                launch_note
                            )))
                            .await;
                    }
                    Op::SyncSession {
                        session_id,
                        messages,
                        system_prompt,
                        system_prompt_override,
                        model,
                        workspace,
                        mode,
                    } => {
                        if let Some(session_id) = session_id {
                            self.session.id = session_id;
                        } else if messages.is_empty() && system_prompt.is_none() {
                            self.session.id = uuid::Uuid::new_v4().to_string();
                        }
                        self.session.messages = messages.into();
                        self.session.compaction_summary_prompt =
                            extract_compaction_summary_prompt(system_prompt.clone());
                        self.session.system_prompt = system_prompt;
                        self.session.last_system_prompt_hash =
                            Some(system_prompt_hash(self.session.system_prompt.as_ref()));
                        // Host-supplied prompts are persisted prefixes. Keep them
                        // byte-stable; mode/runtime state is projected per request.
                        self.session.system_prompt_override =
                            system_prompt_override && self.session.system_prompt.is_some();
                        self.session.auto_model = model.trim().eq_ignore_ascii_case("auto");
                        self.session.model = model;
                        self.session.workspace = workspace.clone();
                        self.current_mode = mode;
                        self.config.model.clone_from(&self.session.model);
                        self.config.workspace = workspace.clone();
                        let ctx =
                            crate::project_context::load_project_context_with_parents(&workspace);
                        self.session.project_context = if ctx.has_instructions() {
                            Some(ctx)
                        } else {
                            None
                        };
                        self.session.rebuild_working_set();
                        self.emit_session_updated().await;
                        let _ = self
                            .tx_event
                            .send(Event::status("Session context synced".to_string()))
                            .await;
                    }
                    Op::CompactContext => {
                        self.handle_manual_compaction().await;
                    }
                    Op::GetSessionSnapshot { tx } => {
                        let total_tokens = self.session.total_usage.input_tokens
                            + self.session.total_usage.output_tokens;
                        let snapshot = SessionSnapshot {
                            messages: self.session.messages.to_vec(),
                            total_tokens,
                            model: self.session.model.clone(),
                            workspace: self.session.workspace.clone(),
                            system_prompt: self.session.system_prompt.clone(),
                            mode: self.current_mode.as_setting().to_string(),
                        };
                        if let Some(tx) = tx.lock().ok().and_then(|mut g| g.take()) {
                            let _ = tx.send(snapshot);
                        }
                    }
                    Op::GetProviderRuntimeStatus { tx } => {
                        let status = if let Some(client) = self.deepseek_client.as_ref() {
                            ProviderRuntimeStatus {
                                provider: client.api_provider(),
                                request_concurrency_limit: client
                                    .provider_request_concurrency_limit(),
                                active_provider_requests: client.active_provider_requests(),
                            }
                        } else {
                            let provider = self.api_config.api_provider();
                            ProviderRuntimeStatus {
                                provider,
                                request_concurrency_limit: self
                                    .api_config
                                    .provider_max_concurrency(provider),
                                active_provider_requests: 0,
                            }
                        };
                        if let Some(tx) = tx.lock().ok().and_then(|mut g| g.take()) {
                            let _ = tx.send(status);
                        }
                    }
                    Op::PurgeContext => {
                        self.handle_purge().await;
                    }
                    Op::EditLastTurn { new_message } => {
                        // #383: /edit — remove the last user+assistant exchange
                        // from the session, then re-send with the new content.
                        // Pop messages from the tail until we've removed the
                        // most recent user message and everything after it.
                        // First, find the last user message index.
                        let mut cut = None;
                        for (idx, msg) in self.session.messages.iter().enumerate().rev() {
                            if msg.role == "user" {
                                cut = Some(idx);
                                break;
                            }
                        }
                        if let Some(idx) = cut {
                            self.session.messages.truncate_to(idx);
                            self.session.bump_messages_revision();
                        }
                        // Now dispatch the new message as a normal send,
                        // reusing the engine's stored mode/model config.
                        let mode = self.current_mode;
                        self.handle_send_message(
                            new_message,
                            mode,
                            Some(self.api_provider),
                            self.session.model.clone(),
                            self.config.goal_objective.clone(),
                            self.config.goal_token_budget,
                            self.config.goal_status,
                            self.session.reasoning_effort.clone(),
                            self.session.reasoning_effort_auto,
                            self.session.auto_model,
                            self.session.allow_shell,
                            self.session.trust_mode,
                            self.session.auto_approve,
                            self.session.approval_mode,
                            self.config.translation_enabled,
                            self.config.show_thinking,
                            self.config.allowed_tools.clone(),
                            Vec::new(),
                            self.config.hook_executor.clone(),
                            self.config.verbosity.clone(),
                            UserInputProvenance::ExternalUser,
                        )
                        .await;
                    }
                    Op::Shutdown => {
                        break;
                    }
                },
            }
        }

        // #freeze: flush any sub-agent checkpoint that the hot-path debounce
        // coalesced away, so a graceful shutdown keeps the latest progress.
        {
            let mut manager = self.subagent_manager.write().await;
            manager.flush_pending_persist();
        }

        // #420: graceful MCP shutdown — send SIGTERM and give stdio servers
        // a brief window to exit before drop fires SIGKILL via kill_on_drop.
        // Best-effort: pool may not exist (no MCP configured) and the lock
        // can fail under contention; either way the kill_on_drop fallback
        // still reaps the children.
        if let Some(pool) = self.mcp_pool.as_ref() {
            let mut guard = pool.lock().await;
            guard.shutdown_all().await;
        }
    }

    async fn emit_session_updated(&self) {
        let _ = self
            .tx_event
            .send(Event::SessionUpdated {
                session_id: self.session.id.clone(),
                messages: self.session.messages.clone().into(),
                system_prompt: self.session.system_prompt.clone(),
                model: self.session.model.clone(),
                workspace: self.session.workspace.clone(),
            })
            .await;
    }

    fn goal_snapshot_for_event(&self) -> Option<GoalSnapshot> {
        match self.config.goal_state.lock() {
            Ok(state) => {
                let snapshot = state.snapshot();
                snapshot.objective.is_some().then_some(snapshot)
            }
            Err(err) => {
                tracing::warn!("goal state lock poisoned while emitting goal update: {err}");
                None
            }
        }
    }

    async fn emit_goal_updated(&self) {
        if let Some(snapshot) = self.goal_snapshot_for_event() {
            let _ = self.tx_event.send(Event::GoalUpdated { snapshot }).await;
        }
    }

    fn record_goal_usage_for_turn(&self, usage: &Usage, elapsed: std::time::Duration) {
        let token_delta =
            u64::from(usage.input_tokens).saturating_add(u64::from(usage.output_tokens));
        let time_delta_seconds = elapsed.as_secs();
        if token_delta == 0 && time_delta_seconds == 0 {
            return;
        }
        match self.config.goal_state.lock() {
            Ok(mut state) => state.record_usage(token_delta, time_delta_seconds),
            Err(err) => tracing::warn!("goal state lock poisoned while recording usage: {err}"),
        }
    }

    fn active_input_tokens_with_current_text(&self, current_text: &str) -> usize {
        let mut messages: Vec<Message> = self.session.messages.clone().into();
        if !current_text.trim().is_empty() {
            messages.push(Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: current_text.to_string(),
                    cache_control: None,
                }],
            });
        }
        estimate_input_tokens_conservative(&messages, self.session.system_prompt.as_ref())
    }

    fn append_resource_metadata_lines(
        &self,
        lines: &mut Vec<String>,
        routed_model: &str,
        current_text: &str,
    ) {
        let input_tokens = self.active_input_tokens_with_current_text(current_text);
        if let Some(budget) = route_context_budget_for_route(
            self.api_provider,
            routed_model,
            self.active_route_limits,
            input_tokens,
        ) {
            lines.push(format!(
                "Context pressure: {} ({:.1}% used, {} / {} tokens; {} input tokens available)",
                budget.pressure.label(),
                budget.usage_percent(),
                budget.input_tokens,
                budget.window_tokens,
                budget.available_input_tokens,
            ));
        }

        if let Some(line) = self.session_token_usage_line() {
            lines.push(line);
        }
        if let Some(line) = self.active_goal_resource_line() {
            lines.push(line);
        }
    }

    fn session_token_usage_line(&self) -> Option<String> {
        let usage = &self.session.total_usage;
        let total = usage.input_tokens.saturating_add(usage.output_tokens);
        if total == 0 {
            return None;
        }

        let mut line = format!(
            "Session token usage: {total} total ({} input, {} output)",
            usage.input_tokens, usage.output_tokens,
        );
        if let Some(hit_tokens) = usage.cache_read_input_tokens {
            line.push_str(&format!(", cache hits {hit_tokens}"));
        }
        if let Some(miss_tokens) = usage.cache_creation_input_tokens {
            line.push_str(&format!(", cache misses {miss_tokens}"));
        }
        Some(line)
    }

    fn active_goal_resource_line(&self) -> Option<String> {
        let snapshot = self.config.goal_state.lock().ok()?.snapshot();
        if !snapshot.is_active() {
            return None;
        }

        let mut telemetry =
            ResourceTelemetry::new(snapshot.tokens_used, snapshot.time_used_seconds);
        if let Some(token_budget) = snapshot.token_budget {
            telemetry = telemetry.with_token_budget(u64::from(token_budget));
        }

        let mut line = format!("Active goal resource usage: {}", telemetry.human_summary());
        if snapshot.tokens_used > 0 && snapshot.time_used_seconds > 0 {
            let rate = snapshot.tokens_used as f64 / snapshot.time_used_seconds as f64;
            line.push_str(&format!("; {rate:.1} tok/s"));
        }
        line.push_str(&format!("; {} continuations", snapshot.continuation_count));
        Some(line)
    }

    async fn add_session_message(&mut self, message: Message) {
        self.session.add_message(message);
        self.emit_session_updated().await;
    }

    fn turn_metadata_block(
        &self,
        routed_model: &str,
        auto_model: bool,
        reasoning_effort: Option<&str>,
        reasoning_effort_auto: bool,
        provenance: UserInputProvenance,
        current_text: &str,
    ) -> ContentBlock {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let working_set_summary = self
            .session
            .working_set
            .summary_block(&self.config.workspace)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let mut lines = vec![
            format!("Current local date: {today}"),
            // Workspace path moved here from the static `## Environment` block so
            // the static system prefix stays byte-stable across sessions (see
            // `render_environment_block` for the prefix-cache rationale).
            format!("Current workspace: {}", self.config.workspace.display()),
            format!("Current model: {routed_model}"),
            format!("Current mode: {}", self.current_mode.as_setting()),
            "Current mode policy source: runtime".to_string(),
            format!(
                "Current mode policy:\n{}",
                Self::mode_runtime_instructions(self.current_mode)
            ),
            format!("Input provenance: {}", provenance.as_str()),
            format!(
                "Input authority: {}",
                if provenance.can_authorize_work() {
                    "external_current_turn"
                } else {
                    "non_authoritative"
                }
            ),
        ];
        if auto_model {
            lines.push(format!("Auto model route: {routed_model}"));
        }
        if reasoning_effort_auto && let Some(reasoning_effort) = reasoning_effort {
            lines.push(format!("Auto reasoning effort: {reasoning_effort}"));
        }
        self.append_resource_metadata_lines(&mut lines, routed_model, current_text);
        if let Some(working_set_summary) = working_set_summary {
            lines.push(working_set_summary);
        }
        let summary = lines.join("\n");

        ContentBlock::Text {
            text: format!("<turn_meta>\n{summary}\n</turn_meta>"),
            cache_control: None,
        }
    }

    fn user_text_message_with_turn_metadata(&self, text: String) -> Message {
        self.user_text_message_with_turn_metadata_for_route(
            text,
            &self.session.model,
            self.session.auto_model,
            self.session.reasoning_effort.as_deref(),
            self.session.reasoning_effort_auto,
        )
    }

    fn user_text_message_with_turn_metadata_for_route(
        &self,
        text: String,
        routed_model: &str,
        auto_model: bool,
        reasoning_effort: Option<&str>,
        reasoning_effort_auto: bool,
    ) -> Message {
        self.user_text_message_with_turn_metadata_for_route_and_provenance(
            text,
            routed_model,
            auto_model,
            reasoning_effort,
            reasoning_effort_auto,
            UserInputProvenance::ExternalUser,
        )
    }

    fn runtime_text_message_with_turn_metadata(
        &self,
        text: String,
        provenance: UserInputProvenance,
    ) -> Message {
        self.user_text_message_with_turn_metadata_for_route_and_provenance(
            text,
            &self.session.model,
            self.session.auto_model,
            self.session.reasoning_effort.as_deref(),
            self.session.reasoning_effort_auto,
            provenance,
        )
    }

    fn user_text_message_with_turn_metadata_for_route_and_provenance(
        &self,
        text: String,
        routed_model: &str,
        auto_model: bool,
        reasoning_effort: Option<&str>,
        reasoning_effort_auto: bool,
        provenance: UserInputProvenance,
    ) -> Message {
        // Place the user text first and turn_meta last so that the leading
        // bytes of each user message stay stable across date / model-route /
        // working-set changes. DeepSeek's KV prefix cache matches byte
        // sequences from the start of each message; when turn_meta (which
        // contains the current date) sits at position 0 the entire user
        // message prefix is invalidated at every date boundary. Moving it
        // to the tail preserves the user-input prefix and limits cache
        // invalidation to the trailing metadata block.
        let turn_metadata = self.turn_metadata_block(
            routed_model,
            auto_model,
            reasoning_effort,
            reasoning_effort_auto,
            provenance,
            &text,
        );
        Message {
            role: "user".to_string(),
            content: vec![
                ContentBlock::Text {
                    text,
                    cache_control: None,
                },
                turn_metadata,
            ],
        }
    }

    async fn handle_idle_subagent_completion(&mut self, first: SubAgentCompletion) {
        let mut completions = vec![first];
        while let Ok(completion) = self.rx_subagent_completion.try_recv() {
            completions.push(completion);
        }

        let count = completions.len();
        let content = completions
            .iter()
            .map(|completion| turn_loop::subagent_completion_runtime_text(&completion.payload))
            .collect::<Vec<_>>()
            .join("\n\n");

        let _ = self
            .tx_event
            .send(Event::status(format!(
                "Resuming turn with {count} idle sub-agent completion(s)"
            )))
            .await;

        self.handle_send_message(
            content,
            self.current_mode,
            Some(self.api_provider),
            self.session.model.clone(),
            self.config.goal_objective.clone(),
            self.config.goal_token_budget,
            self.config.goal_status,
            self.session.reasoning_effort.clone(),
            self.session.reasoning_effort_auto,
            self.session.auto_model,
            self.session.allow_shell,
            self.session.trust_mode,
            self.session.auto_approve,
            self.session.approval_mode,
            self.config.translation_enabled,
            self.config.show_thinking,
            self.config.allowed_tools.clone(),
            Vec::new(),
            self.config.hook_executor.clone(),
            self.config.verbosity.clone(),
            UserInputProvenance::SubAgentHandoff,
        )
        .await;
    }

    /// Handle a send message operation
    #[allow(clippy::too_many_arguments)]
    /// After a turn completes, check whether an active goal should keep going.
    /// Returns a continuation message to re-dispatch as a new turn, or `None`
    /// if the goal is complete, blocked, paused, or over an optional budget.
    ///
    /// There is no continuation cap — a goal runs until the model self-reports
    /// done/blocked, the user pauses or clears, or an optional token/time
    /// budget is exhausted. The loop is "until done," not "until N turns."
    fn goal_continuation_if_active(&self) -> Option<String> {
        let snapshot = self.config.goal_state.lock().ok()?.snapshot();
        if !snapshot.is_active() {
            return None;
        }

        // The snapshot status is a string ("active", "paused", "complete",
        // "blocked"). Map it to the goal-loop decision core's status enum.
        let status = match snapshot.status.as_str() {
            "active" => crate::goal_loop::GoalRunStatus::Active,
            "complete" => crate::goal_loop::GoalRunStatus::Completed,
            // Paused / Blocked / unknown → no continuation.
            _ => return None,
        };

        let decision = crate::goal_loop::decide_continuation(
            status,
            crate::goal_loop::GoalProgress {
                tokens_used: snapshot.tokens_used,
                time_used_seconds: snapshot.time_used_seconds,
                continuations: snapshot.continuation_count,
            },
            crate::goal_loop::GoalBudget {
                token_budget: snapshot.token_budget.map(u64::from),
                time_budget_seconds: None,
            },
        );

        match decision {
            crate::goal_loop::ContinuationDecision::Continue => {
                Some(crate::tools::goal::render_continuation_prompt(
                    &snapshot,
                    snapshot.continuation_count,
                ))
            }
            // All stop reasons → no continuation. The caller (the async turn
            // completion path) emits a status message for budget-exhaustion.
            crate::goal_loop::ContinuationDecision::Stop(reason) => {
                tracing::info!(?reason, "goal continuation stopped");
                None
            }
        }
    }

    /// Handle `/goal pause|resume|clear|complete|blocked` by writing the new
    /// status to `SharedGoalState` so the cross-turn continuation loop respects
    /// it. This does NOT dispatch a model turn — it's a control-plane update.
    async fn handle_set_goal_status(&mut self, status: GoalStatus, clear: bool) {
        match self.config.goal_state.lock() {
            Ok(mut state) => {
                if clear {
                    // `/goal clear` — wipe the objective entirely.
                    state.sync_from_host_status(None, None, GoalStatus::Active);
                } else {
                    // Update only the status; keep the objective and budget.
                    // `sync_from_host_status` resets usage when the objective
                    // changes, but here we pass the existing objective so usage
                    // is preserved (pause/resume shouldn't reset the counter).
                    let objective = state.objective().map(str::to_string);
                    let budget = state.token_budget();
                    state.sync_from_host_status(objective.as_deref(), budget, status);
                }
            }
            Err(err) => {
                tracing::warn!("goal state lock poisoned during SetGoalStatus: {err}");
            }
        }
        let label = if clear {
            "cleared"
        } else {
            match status {
                GoalStatus::Active => "resumed",
                GoalStatus::Paused => "paused",
                GoalStatus::Complete => "complete",
                GoalStatus::Blocked => "blocked",
            }
        };
        let _ = self
            .tx_event
            .send(Event::status(format!("Goal {label}.")))
            .await;
        self.emit_goal_updated().await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_send_message(
        &mut self,
        content: String,
        mode: AppMode,
        provider: Option<ApiProvider>,
        model: String,
        goal_objective: Option<String>,
        goal_token_budget: Option<u32>,
        goal_status: GoalStatus,
        reasoning_effort: Option<String>,
        reasoning_effort_auto: bool,
        auto_model: bool,
        allow_shell: bool,
        trust_mode: bool,
        auto_approve: bool,
        approval_mode: crate::tui::approval::ApprovalMode,
        translation_enabled: bool,
        show_thinking: bool,
        allowed_tools: Option<Vec<String>>,
        dynamic_tools: Vec<DynamicToolSpec>,
        hook_executor: Option<std::sync::Arc<crate::hooks::HookExecutor>>,
        verbosity: Option<String>,
        provenance: UserInputProvenance,
    ) {
        let input_policy = effective_input_policy(
            provenance,
            mode,
            &content,
            allow_shell,
            trust_mode,
            mode == AppMode::Yolo || auto_approve,
            approval_mode,
        );
        if let Some(status) = input_policy.status.clone() {
            let _ = self.tx_event.send(Event::status(status)).await;
        }
        // Reset cancel token for fresh turn (in case previous was cancelled)
        self.reset_cancel_token();

        // Track the complete effective mode policy so mid-turn metadata, `/edit`,
        // idle worker resumptions, and approval gates cannot read a stale policy
        // after the UI changed modes (#3568).
        self.apply_runtime_mode_policy(
            input_policy.mode,
            input_policy.allow_shell,
            input_policy.trust_mode,
            input_policy.auto_approve,
            input_policy.approval_mode,
        );

        // Drain stale steer messages from previous turns.
        while self.rx_steer.try_recv().is_ok() {}

        // Create turn context first so start event includes a stable turn id.
        let mut turn = TurnContext::new(self.config.max_steps);
        self.turn_counter = self.turn_counter.saturating_add(1);

        // Emit turn started event IMMEDIATELY so the UI knows the turn is
        // active. The snapshot below can take 30+ seconds on slow filesystems
        // (e.g. WSL2 /mnt/c) and must not delay the TurnStarted event.
        let _ = self
            .tx_event
            .send(Event::TurnStarted {
                turn_id: turn.id.clone(),
            })
            .await;

        // Snapshot the workspace BEFORE we touch a single tool. Run the git
        // work on the blocking pool so the async runtime stays responsive;
        // failure is non-fatal (the helper logs at WARN).
        if self.config.snapshots_enabled {
            // Clone the user prompt now — `content` is moved into
            // `user_text_message_with_turn_metadata_for_route` below, so we need
            // a copy for both pre- and post-turn snapshot labels. The
            // label carries a truncated first line so `/restore`
            // listings are human-readable.
            let snapshot_prompt = content.clone();
            let pre_workspace = self.session.workspace.clone();
            let pre_seq = self.turn_counter;
            let pre_cap = self.config.snapshots_max_workspace_bytes;
            let _ = tokio::task::spawn_blocking(move || {
                pre_turn_snapshot(&pre_workspace, pre_seq, pre_cap, Some(&snapshot_prompt))
            })
            .await;
        }

        // A new turn means any leftover retry banner (success cleared
        // it, failure pinned it) is no longer relevant — reset to idle
        // so the footer doesn't display a stale failure row across
        // turns (#499).
        crate::retry_status::clear();

        // Clone user prompt for post-turn snapshot label before `content`
        // is moved into `user_text_message_with_turn_metadata_for_route` below.
        let snapshot_prompt_post = content.clone();

        // Check if we have the appropriate client
        if let Some(provider) = provider
            && let Err(message) = self.activate_runtime_route(provider, &model)
        {
            self.deepseek_client_error = Some(message.clone());
            let _ = self
                .tx_event
                .send(Event::error(ErrorEnvelope::fatal_auth(message.clone())))
                .await;
            let _ = self
                .tx_event
                .send(Event::TurnComplete {
                    usage: turn.usage.clone(),
                    status: TurnOutcomeStatus::Failed,
                    error: Some(message),
                    tool_catalog: None,
                    base_url: None,
                })
                .await;
            return;
        }

        if self.deepseek_client.is_none() {
            let message = self
                .deepseek_client_error
                .as_deref()
                .map(|err| format!("Failed to send message: {err}"))
                .unwrap_or_else(|| "Failed to send message: API client not configured".to_string());
            let _ = self
                .tx_event
                .send(Event::error(ErrorEnvelope::fatal_auth(message.clone())))
                .await;
            let _ = self
                .tx_event
                .send(Event::TurnComplete {
                    usage: turn.usage.clone(),
                    status: TurnOutcomeStatus::Failed,
                    error: Some(message),
                    tool_catalog: None,
                    base_url: None,
                })
                .await;
            return;
        }

        self.session
            .working_set
            .observe_user_message(&content, &self.session.workspace);

        let _results = self.extension_manager
            .dispatch(&codewhale_extension::HookEvent::BeforeAgentStart {
                system_prompt: String::new(),
                prompt: content.clone(),
                is_first_turn: self.turn_counter <= 1,
            })
            .await;
        let force_update_plan_first = should_force_update_plan_first(input_policy.mode, &content);

        // Add user message to session
        let user_msg = self.user_text_message_with_turn_metadata_for_route_and_provenance(
            content,
            &model,
            auto_model,
            reasoning_effort.as_deref(),
            reasoning_effort_auto,
            provenance,
        );
        self.session.add_message(user_msg);

        let previous_goal_objective = self.config.goal_objective.clone();
        let previous_goal_token_budget = self.config.goal_token_budget;
        let previous_goal_status = self.config.goal_status;

        self.session.model = model;
        self.config.model.clone_from(&self.session.model);
        self.config.goal_objective = goal_objective.clone();
        self.config.goal_token_budget = goal_token_budget;
        self.config.goal_status = goal_status;
        if normalized_goal_objective(previous_goal_objective.as_deref())
            != normalized_goal_objective(goal_objective.as_deref())
            || previous_goal_token_budget != goal_token_budget
            || previous_goal_status != goal_status
        {
            sync_goal_state_from_host(
                &self.config.goal_state,
                normalized_goal_objective(goal_objective.as_deref()).as_deref(),
                goal_token_budget,
                goal_status,
            );
        }
        self.config.allowed_tools = allowed_tools;
        self.config.hook_executor = hook_executor;
        self.session.reasoning_effort = reasoning_effort;
        self.session.reasoning_effort_auto = reasoning_effort_auto;
        self.session.auto_model = auto_model;
        self.config.translation_enabled = translation_enabled;
        self.config.show_thinking = show_thinking;
        self.config.verbosity = verbosity;

        // Refresh stable prompt context.
        self.refresh_system_prompt();
        self.emit_session_updated().await;

        // Build tool registry and tool list for the current mode
        let todo_list = self.config.todos.clone();
        let plan_state = self.config.plan_state.clone();

        let tool_context = self.build_tool_context(input_policy.mode, input_policy.auto_approve);
        let builder = self
            .build_turn_tool_registry_builder(input_policy.mode, todo_list, plan_state)
            .with_dynamic_tools(&dynamic_tools);

        let subagents_available =
            self.config.subagents_enabled && self.config.features.enabled(Feature::Subagents);

        let fork_context_for_runtime = if subagents_available {
            let state = StructuredState::capture(
                input_policy.mode.label(),
                self.config.workspace.clone(),
                std::env::current_dir().ok(),
                &self.session.working_set,
                &self.config.todos,
                &self.config.plan_state,
                Some(&self.subagent_manager),
            )
            .await;
            Some(SubAgentForkContext {
                system: self.session.system_prompt.clone(),
                messages: self.messages_with_turn_metadata(),
                structured_state_block: state.to_system_block(),
            })
        } else {
            None
        };

        // Mailbox for structured sub-agent envelopes (#128/#130). One per
        // turn: the receiver is drained by a short-lived task that converts
        // envelopes into `Event::SubAgentMailbox` so the UI can route them
        // to the matching in-transcript card. The drainer exits naturally
        // when every cloned sender is dropped at turn-end.
        let mailbox_for_runtime = if subagents_available {
            let cancel_token = self.cancel_token.child_token();
            let (mailbox, mut receiver) = Mailbox::new(cancel_token.clone());
            let tx_event_clone = self.tx_event.clone();
            spawn_supervised(
                "subagent-mailbox-drainer",
                std::panic::Location::caller(),
                async move {
                    let mut best_effort_sent_at: HashMap<String, Instant> = HashMap::new();
                    while let Some(envelope) = receiver.recv().await {
                        let event = Event::SubAgentMailbox {
                            seq: envelope.seq,
                            message: envelope.message,
                        };
                        if let Event::SubAgentMailbox { message, .. } = &event
                            && subagent_mailbox_message_is_best_effort(message)
                        {
                            if !subagent_mailbox_best_effort_send_permitted(
                                &mut best_effort_sent_at,
                                message,
                                Instant::now(),
                            ) {
                                continue;
                            }
                            match tx_event_clone.try_send(event) {
                                Ok(()) => continue,
                                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => continue,
                                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                            }
                        }
                        if tx_event_clone.send(event).await.is_err() {
                            break;
                        }
                    }
                },
            );
            Some((mailbox, cancel_token))
        } else {
            None
        };

        let mcp_pool = if self.config.features.enabled(Feature::Mcp) {
            self.ensure_mcp_pool().await.ok()
        } else {
            None
        };

        let mut tool_registry = if subagents_available {
            let runtime = if let Some(client) = self.deepseek_client.clone() {
                let runtime_allow_shell =
                    self.session.allow_shell && !matches!(input_policy.mode, AppMode::Plan);
                let runtime_shell_policy =
                    shell_policy_for_mode(input_policy.mode, runtime_allow_shell);
                let mut rt = SubAgentRuntime::new(
                    client,
                    self.session.model.clone(),
                    tool_context.clone(),
                    runtime_allow_shell,
                    Some(self.tx_event.clone()),
                    Arc::clone(&self.subagent_manager),
                )
                .with_role_models(self.config.subagent_model_overrides.clone())
                .with_auto_model(self.session.auto_model)
                .with_reasoning_effort(
                    self.session.reasoning_effort.clone(),
                    self.session.reasoning_effort_auto,
                )
                .with_agent_tool_surface_options(
                    self.agent_tool_surface_options(runtime_shell_policy),
                )
                .with_max_spawn_depth(self.config.max_spawn_depth)
                .with_step_api_timeout(self.config.subagent_api_timeout)
                .with_speech_output_dir(self.config.speech_output_dir.clone())
                .with_mcp_pool(mcp_pool.clone())
                .with_todos(self.config.todos.clone())
                .with_parent_completion_tx(self.tx_subagent_completion.clone());
                if matches!(input_policy.mode, AppMode::Plan) {
                    rt.worker_profile = WorkerRuntimeProfile::for_role(SubAgentType::Plan);
                }
                if let Some(context) = fork_context_for_runtime.clone() {
                    rt = rt.with_fork_context(context);
                }
                if let Some((mailbox, cancel_token)) = mailbox_for_runtime.as_ref() {
                    rt = rt
                        .with_mailbox(mailbox.clone())
                        .with_cancel_token(cancel_token.clone());
                }
                Some(rt)
            } else {
                None
            };
            if let Some(subagent_runtime) = runtime {
                Some(
                    builder
                        .with_subagent_tools(self.subagent_manager.clone(), subagent_runtime)
                        .build(tool_context),
                )
            } else {
                tracing::warn!(
                    "Sub-agents enabled but no API client available, falling back to basic tool set"
                );
                Some(builder.build(tool_context))
            }
        } else {
            Some(builder.build(tool_context))
        };

        // Load plugin tools from the user's tools directory and apply any
        // config.toml overrides. Explicit overrides win over auto-discovered
        // scripts with the same tool name.
        let mut plugin_tool_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        if let Some(ref mut tool_registry) = tool_registry {
            plugin_tool_names = configure_plugin_tools(tool_registry, self.config.tools.as_ref());
        }

        let mcp_tools = if self.config.features.enabled(Feature::Mcp) {
            self.mcp_tools().await
        } else {
            Vec::new()
        };
        let tools = tool_registry.as_ref().map(|registry| {
            let capability = crate::model_profile::resolved_capability_profile(
                self.api_config.api_provider(),
                &self.config.model,
            );
            let mut catalog = build_model_tool_catalog_with_surface(
                registry.to_api_tools_with_cache(true),
                mcp_tools,
                input_policy.mode,
                &self.config.tools_always_load,
                capability.tool_surface_budget,
            );
            for tool in &mut catalog {
                if plugin_tool_names.contains(&tool.name) {
                    tool.defer_loading = Some(false);
                }
            }
            filter_tool_catalog_for_gates(
                &mut catalog,
                self.config.allowed_tools.as_deref(),
                self.config.disallowed_tools.as_deref(),
            );
            catalog
        });
        let tool_catalog_for_event = tools.clone();
        let base_url_for_event = self
            .deepseek_client
            .as_ref()
            .map(|client| client.base_url().to_string());

        // Main turn loop. Catch panics here so an internal error surfaces as a
        // failed TurnComplete instead of unwinding through `engine.run()` and
        // killing the whole engine-event-loop task — which left the UI stuck
        // on "working" forever with the engine silently dead (#2583, #1269).
        use futures_util::FutureExt as _;
        let turn_result = std::panic::AssertUnwindSafe(self.handle_deepseek_turn(
            &mut turn,
            tool_registry.as_ref(),
            tools,
            input_policy.mode,
            force_update_plan_first,
            input_policy.dynamic_active_tools,
        ))
        .catch_unwind()
        .await;
        let (status, error) = match turn_result {
            Ok(outcome) => outcome,
            Err(panic) => {
                let detail = crate::utils::panic_message(&*panic);
                crate::utils::record_caught_panic("engine-event-loop", &detail);
                (
                    TurnOutcomeStatus::Failed,
                    Some(format!(
                        "The engine hit an internal error and stopped this turn: {detail}. \
                         Your session is intact — send your message again to retry. \
                         A crash report was saved to ~/.codewhale/crashes/."
                    )),
                )
            }
        };

        // Update session usage
        self.session.total_usage.add(&turn.usage);
        self.record_goal_usage_for_turn(&turn.usage, turn.elapsed());

        // Emit turn complete event — after all post-turn bookkeeping so
        // the terminal is immediately responsive when the UI receives it.
        self.emit_goal_updated().await;
        let _ = self
            .tx_event
            .send(Event::TurnComplete {
                usage: turn.usage,
                status,
                error,
                tool_catalog: tool_catalog_for_event,
                base_url: base_url_for_event,
            })
            .await;

        // Post-turn snapshot. Fire-and-forget: TurnComplete is already
        // emitted, so the UI is unblocked and the user can type / select /
        // paste immediately (#234). The git work proceeds on the blocking
        // pool without forcing the engine loop to await it.
        if self.config.snapshots_enabled {
            // `snapshot_prompt_post` was cloned from `content` above,
            // before `content` was moved into the session messages.
            let post_workspace = self.session.workspace.clone();
            let post_seq = self.turn_counter;
            let post_cap = self.config.snapshots_max_workspace_bytes;
            crate::utils::spawn_blocking_supervised("post-turn-snapshot", move || {
                post_turn_snapshot(
                    &post_workspace,
                    post_seq,
                    post_cap,
                    Some(&snapshot_prompt_post),
                );
            });
        }

        // ── Cross-turn goal continuation ───────────────────────────────────
        // If the turn completed successfully and a goal is still Active (and
        // under any optional budget), re-dispatch a synthetic continuation
        // message back into the engine's own op channel. This makes `/goal` a
        // persistent loop that runs until the model self-reports complete or
        // blocked, the user pauses/clears, or an optional budget is exhausted.
        // There is no continuation cap. A Failed or Interrupted turn does NOT
        // continue — Esc cancels the loop by interrupting the turn.
        if status == TurnOutcomeStatus::Completed {
            if let Some(continuation) = self.goal_continuation_if_active() {
                // Re-dispatch with the same route/mode/approval settings as
                // the prior turn. The non-Copy values were moved into
                // `self.config` / `self.session` earlier in this function, so
                // we clone them back out here.
                let _ = self
                    .tx_op
                    .send(Op::SendMessage {
                        content: continuation,
                        mode,
                        provider,
                        model: self.session.model.clone(),
                        goal_objective: None,
                        goal_token_budget: None,
                        goal_status: GoalStatus::Active,
                        reasoning_effort: self.session.reasoning_effort.clone(),
                        reasoning_effort_auto,
                        auto_model,
                        allow_shell,
                        trust_mode,
                        auto_approve,
                        approval_mode,
                        translation_enabled,
                        show_thinking,
                        allowed_tools: self.config.allowed_tools.clone(),
                        dynamic_tools: dynamic_tools.clone(),
                        hook_executor: self.config.hook_executor.clone(),
                        verbosity: self.config.verbosity.clone(),
                        provenance: UserInputProvenance::Runtime,
                    })
                    .await;
            }
        }
    }

    async fn handle_manual_compaction(&mut self) {
        let id = format!("compact_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let zero_usage = Usage {
            input_tokens: 0,
            output_tokens: 0,
            ..Usage::default()
        };
        let Some(client) = self.deepseek_client.clone() else {
            let message = "Manual compaction unavailable: API client not configured".to_string();
            self.emit_compaction_failed(id, false, message.clone())
                .await;
            let _ = self
                .tx_event
                .send(Event::error(ErrorEnvelope::fatal_auth(message.clone())))
                .await;
            let _ = self
                .tx_event
                .send(Event::TurnComplete {
                    usage: zero_usage,
                    status: TurnOutcomeStatus::Failed,
                    error: Some(message),
                    tool_catalog: None,
                    base_url: None,
                })
                .await;
            return;
        };

        let start_message = "Manual context compaction started".to_string();
        self.emit_compaction_started(id.clone(), false, start_message)
            .await;

        let compaction_pins = self
            .session
            .working_set
            .pinned_message_indices(&self.session.messages, &self.session.workspace);
        let compaction_paths = self.session.working_set.top_paths(24);
        let messages_before = self.session.messages.len();
        let mut turn_status = TurnOutcomeStatus::Completed;
        let mut turn_error = None;

        match compact_messages_safe(
            &client,
            &self.session.messages,
            &self.config.compaction,
            Some(&self.session.workspace),
            Some(&compaction_pins),
            Some(&compaction_paths),
        )
        .await
        {
            Ok(result) => {
                if !result.messages.is_empty() || self.session.messages.is_empty() {
                    let messages_after = result.messages.len();
                    self.session.messages = result.messages.into();
                    self.merge_compaction_summary(result.summary_prompt);
                    self.emit_session_updated().await;
                    let removed = messages_before.saturating_sub(messages_after);
                    let message = if result.retries_used > 0 {
                        format!(
                            "Compaction complete: {messages_before} → {messages_after} messages ({removed} removed, {} retries)",
                            result.retries_used
                        )
                    } else {
                        format!(
                            "Compaction complete: {messages_before} → {messages_after} messages ({removed} removed)"
                        )
                    };
                    self.emit_compaction_completed(
                        id,
                        false,
                        message,
                        Some(messages_before),
                        Some(messages_after),
                    )
                    .await;
                } else {
                    let message = "Compaction skipped: produced empty result".to_string();
                    self.emit_compaction_failed(id, false, message.clone())
                        .await;
                    turn_status = TurnOutcomeStatus::Failed;
                    turn_error = Some(message);
                }
            }
            Err(err) => {
                let message = format!("Manual context compaction failed: {err}");
                self.emit_compaction_failed(id, false, message.clone())
                    .await;
                let _ = self.tx_event.send(Event::status(message.clone())).await;
                turn_status = TurnOutcomeStatus::Failed;
                turn_error = Some(message);
            }
        }

        let _ = self
            .tx_event
            .send(Event::TurnComplete {
                usage: zero_usage,
                status: turn_status,
                error: turn_error,
                tool_catalog: None,
                base_url: None,
            })
            .await;
    }

    async fn handle_purge(&mut self) {
        let zero_usage = Usage {
            input_tokens: 0,
            output_tokens: 0,
            ..Usage::default()
        };
        let Some(client) = self.deepseek_client.clone() else {
            let message = "Purge unavailable: API client not configured".to_string();
            emit_purge_failed(&self.tx_event, message.clone()).await;
            let _ = self
                .tx_event
                .send(Event::error(ErrorEnvelope::fatal_auth(message.clone())))
                .await;
            let _ = self
                .tx_event
                .send(Event::TurnComplete {
                    usage: zero_usage,
                    status: TurnOutcomeStatus::Failed,
                    error: Some(message),
                    tool_catalog: None,
                    base_url: None,
                })
                .await;
            return;
        };

        emit_purge_started(
            &self.tx_event,
            "Agent context purge in progress\u{2026}".to_string(),
        )
        .await;
        let messages_before = self.session.messages.len();

        let (status, error) = match run_purge(
            &client,
            &self.session.messages,
            &self.session.model,
            self.session.reasoning_effort.clone(),
            effective_max_output_tokens_for_route(&self.session.model, self.active_route_limits),
        )
        .await
        {
            Ok(result) => {
                let messages_after = result.messages.len();
                self.session.messages = result.messages.into();
                self.emit_session_updated().await;

                let summary = format!(
                    "Purge complete: {messages_before} → {messages_after} messages \
                         ({} removed, {} condensed)",
                    result.removed_count, result.replaced_count,
                );
                emit_purge_completed(
                    &self.tx_event,
                    messages_before,
                    messages_after,
                    result.removed_count,
                    result.replaced_count,
                    summary,
                )
                .await;
                (TurnOutcomeStatus::Completed, None)
            }
            Err(e) => {
                emit_purge_failed(&self.tx_event, e.clone()).await;
                (TurnOutcomeStatus::Failed, Some(e))
            }
        };

        let _ = self
            .tx_event
            .send(Event::TurnComplete {
                usage: zero_usage,
                status,
                error,
                tool_catalog: None,
                base_url: None,
            })
            .await;
    }

    fn estimated_input_tokens(&mut self) -> usize {
        // Memoized on (session.messages_revision, system-prompt fingerprint).
        // The cache invalidates as soon as either input changes; until then
        // repeated calls (capacity checkpoints, /status, context inspector,
        // TUI footer) all hit the cached value.
        self.token_estimate_cache.lookup_or_compute(
            self.session.messages_revision,
            self.session.system_prompt.as_ref(),
            &self.session.messages,
        )
    }

    fn trim_oldest_messages_to_budget(&mut self, target_input_budget: usize) -> usize {
        let mut removed = 0usize;
        while self.session.messages.len() > MIN_RECENT_MESSAGES_TO_KEEP
            && self.estimated_input_tokens() > target_input_budget
        {
            self.session.messages.trim_front(1);
            self.session.bump_messages_revision();
            removed = removed.saturating_add(1);
        }
        removed
    }

    async fn recover_context_overflow(&mut self, client: &DeepSeekClient, reason: &str) -> bool {
        let Some(target_budget) = context_input_budget_for_route(
            self.api_provider,
            &self.session.model,
            self.active_route_limits,
            0,
        ) else {
            return false;
        };

        let id = format!("compact_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let start_message = format!("Emergency context compaction started ({reason})");
        self.emit_compaction_started(id.clone(), true, start_message)
            .await;

        let before_tokens = self.estimated_input_tokens();
        let before_count = self.session.messages.len();

        let mut retries_used = 0u32;
        let mut summary_prompt = None;
        let mut compacted_messages: Vec<Message> = self.session.messages.clone().into();

        let mut forced_config = self.config.compaction.clone();
        forced_config.enabled = true;
        forced_config.token_threshold = forced_config
            .token_threshold
            .min(target_budget.saturating_sub(1))
            .max(1);

        // Preserve the working-set pins on the emergency/preflight path too.
        // Previously this passed None/None, so a compaction routed here (which,
        // on large windows, is the path that actually fires) could summarize
        // away pinned errors, patches, and the files the user is editing.
        let compaction_pins = self
            .session
            .working_set
            .pinned_message_indices(&self.session.messages, &self.session.workspace);
        let compaction_paths = self.session.working_set.top_paths(24);

        match compact_messages_safe(
            client,
            &self.session.messages,
            &forced_config,
            Some(&self.session.workspace),
            Some(&compaction_pins),
            Some(&compaction_paths),
        )
        .await
        {
            Ok(result) => {
                retries_used = result.retries_used;
                compacted_messages = result.messages;
                summary_prompt = result.summary_prompt;
            }
            Err(err) => {
                let _ = self
                    .tx_event
                    .send(Event::status(format!(
                        "Emergency compaction API pass failed: {err}. Falling back to local trim."
                    )))
                    .await;
            }
        }

        if !compacted_messages.is_empty() || self.session.messages.is_empty() {
            self.session.messages = compacted_messages.into();
        }
        self.merge_compaction_summary(summary_prompt);

        let trimmed = self.trim_oldest_messages_to_budget(target_budget);
        self.emit_session_updated().await;
        let after_tokens = self.estimated_input_tokens();
        let after_count = self.session.messages.len();
        let recovered = after_tokens <= target_budget
            && (after_tokens < before_tokens || after_count < before_count || trimmed > 0);

        if recovered {
            let removed = before_count.saturating_sub(after_count);
            let mut details = format!(
                "Emergency compaction complete: {before_count} → {after_count} messages ({removed} removed), ~{before_tokens} → ~{after_tokens} tokens"
            );
            if retries_used > 0 {
                details.push_str(&format!(" ({retries_used} retries)"));
            }
            if trimmed > 0 {
                details.push_str(&format!(", trimmed {trimmed} oldest"));
            }
            self.emit_compaction_completed(
                id,
                true,
                details.clone(),
                Some(before_count),
                Some(after_count),
            )
            .await;
            let _ = self.tx_event.send(Event::status(details)).await;
            return true;
        }

        let message = format!(
            "Emergency context compaction failed to reduce request below model limit \
             (estimate ~{after_tokens} tokens, budget ~{target_budget})."
        );
        self.emit_compaction_failed(id, true, message.clone()).await;
        let _ = self.tx_event.send(Event::status(message)).await;
        false
    }

    fn build_tool_context(&self, mode: AppMode, auto_approve: bool) -> ToolContext {
        // Load the per-workspace trusted-paths list (#29) on every tool-context
        // build. Cheap (a small JSON file) and always reflects the latest
        // `/trust add` / `/trust remove` mutations without an explicit cache
        // refresh hook.
        let trusted = crate::workspace_trust::WorkspaceTrust::load_for(&self.session.workspace);
        let mut trusted_external_paths = trusted.paths().to_vec();
        let clipboard_images_dir =
            crate::tui::clipboard::clipboard_images_dir(&self.session.workspace);
        if !trusted_external_paths
            .iter()
            .any(|path| path == &clipboard_images_dir)
        {
            trusted_external_paths.push(clipboard_images_dir);
        }
        let mut ctx = ToolContext::with_auto_approve(
            self.session.workspace.clone(),
            self.session.trust_mode,
            self.session.notes_path.clone(),
            self.session.mcp_config_path.clone(),
            mode == AppMode::Yolo || auto_approve,
        )
        .with_state_namespace(self.session.id.clone())
        .with_features(self.config.features.clone())
        .with_shell_manager(self.shell_manager.clone())
        .with_runtime_services(self.config.runtime_services.clone())
        .with_skills_config(
            self.config.skills_dir.clone(),
            self.config.skills_scan_codewhale_only,
        )
        .with_session_objects(crate::rlm::session::SessionObjectSnapshot::new(
            self.session.id.clone(),
            self.session.model.clone(),
            self.session.workspace.clone(),
            self.session.system_prompt.clone(),
            self.session.messages.clone().into(),
        ))
        .with_cancel_token(self.cancel_token.clone())
        .with_shell_policy(shell_policy_for_mode(mode, self.session.allow_shell))
        .with_trusted_external_paths(trusted_external_paths)
        .with_follow_symlinks(self.config.workspace_follow_symlinks);

        // Hand the user-memory path to tools so the model-callable
        // `remember` tool can append entries (#489). `None` when the
        // feature is disabled — tools short-circuit on that.
        if self.config.memory_enabled {
            ctx.memory_path = Some(self.config.memory_path.clone());
        }

        if let Some(decider) = self.config.network_policy.as_ref() {
            ctx = ctx.with_network_policy(decider.clone());
        }

        // Wire the large-output router (#548). Only attaches when the
        // [workshop] config table is present; sub-agents don't inherit the
        // router (their ToolContext is built separately) to prevent recursive
        // routing of the synthesis call itself.
        if let Some(workshop_cfg) = self.config.workshop.as_ref()
            && let Some(vars_arc) = self.workshop_vars.as_ref()
        {
            let router =
                crate::tools::large_output_router::LargeOutputRouter::new(workshop_cfg.clone());
            ctx = ctx.with_large_output_router(router, vars_arc.clone());
        }

        // Wire the external sandbox backend (#516). exec_shell checks this
        // field and routes commands through the backend instead of spawning
        // a local process when it's set.
        if let Some(backend) = self.sandbox_backend.as_ref() {
            ctx = ctx.with_sandbox_backend(std::sync::Arc::clone(backend));
        }

        // Wire search provider config.
        ctx.search_provider = self.config.search_provider;
        ctx.search_api_key = self.config.search_api_key.clone();
        ctx.search_base_url = self.config.search_base_url.clone();

        let policy = sandbox_policy_for_mode(mode, &self.session.workspace);
        let mut ctx = ctx.with_elevated_sandbox_policy(policy);
        if matches!(mode, AppMode::Plan) {
            ctx = ctx.with_shell_network_denied_hint(
                "Shell command blocked: Plan mode runs shell commands in a read-only sandbox — no writes, no network. Use Agent mode (`/mode agent`) for any command that creates or modifies files, or that needs network access.",
            );
        }
        ctx
    }

    async fn ensure_mcp_pool(&mut self) -> Result<Arc<AsyncMutex<McpPool>>, ToolError> {
        if let Some(pool) = self.mcp_pool.as_ref() {
            return Ok(Arc::clone(pool));
        }
        let mut pool = McpPool::from_config_path_with_workspace(
            &self.session.mcp_config_path,
            &self.session.workspace,
        )
        .map_err(|e| ToolError::execution_failed(format!("Failed to load MCP config: {e}")))?;
        if let Some(decider) = self.config.network_policy.as_ref() {
            pool = pool.with_network_policy(decider.clone());
        }
        let pool = Arc::new(AsyncMutex::new(pool));
        self.mcp_pool = Some(Arc::clone(&pool));
        Ok(pool)
    }

    async fn mcp_tools(&mut self) -> Vec<Tool> {
        let pool = match self.ensure_mcp_pool().await {
            Ok(pool) => pool,
            Err(err) => {
                let _ = self.tx_event.send(Event::status(format!("{err:#}"))).await;
                return Vec::new();
            }
        };

        let mut pool = pool.lock().await;
        let errors = pool.connect_all().await;
        for (server, err) in errors {
            let _ = self
                .tx_event
                .send(Event::status(format!(
                    "Failed to connect MCP server '{server}': {err:#}"
                )))
                .await;
        }

        pool.to_api_tools()
    }

    /// Handle a turn using the DeepSeek API.
    #[allow(clippy::too_many_lines)]
    /// Run the pre-request layered-context checkpoint (#159). Checks whether
    /// the active input estimate has crossed a soft-seam threshold and, if so,
    /// produces an `<archived_context>` block via Flash and appends it as an
    /// assistant message. Called from `handle_deepseek_turn` before each API
    /// request so the model always has the latest navigation aids.
    async fn layered_context_checkpoint(&mut self) {
        if self.seam_manager.is_none() {
            return;
        }
        if !self.seam_manager.as_ref().unwrap().config().enabled {
            return;
        }

        // Compute the estimated token count *before* taking a long-lived
        // `&SeamManager` borrow — `estimated_input_tokens` mutates the
        // engine's token-estimate cache, which would conflict.
        let estimated_tokens = self.estimated_input_tokens();
        let seam_mgr = self.seam_manager.as_ref().unwrap();
        let highest = seam_mgr.highest_level().await;
        let Some(level) = seam_mgr.seam_level_for(estimated_tokens, highest) else {
            return;
        };

        // Determine the message range to summarize: everything before the
        // verbatim window. The verbatim window (last ~16 turns) stays
        // untouched so the model always has ground-truth recent context.
        let msg_count = self.session.messages.len();
        let verbatim_start = seam_mgr.verbatim_window_start(msg_count);
        if verbatim_start == 0 {
            return; // Not enough messages to summarize.
        }

        let msg_range_end = verbatim_start;
        let pinned = self
            .session
            .working_set
            .pinned_message_indices(&self.session.messages, &self.session.workspace);

        let _ = self
            .tx_event
            .send(Event::status(format!(
                "⏻ producing L{level} context seam ({msg_range_end} messages)…"
            )))
            .await;

        // If we have existing seams, recompact; otherwise produce fresh.
        let existing_seams = seam_mgr.collect_seam_texts(&self.session.messages).await;
        let seam_text = if existing_seams.is_empty() {
            match seam_mgr
                .produce_soft_seam(
                    &self.session.messages,
                    level,
                    0,
                    msg_range_end,
                    Some(&self.session.workspace),
                    &pinned,
                )
                .await
            {
                Ok(text) => text,
                Err(err) => {
                    crate::logging::warn(format!("L{level} soft seam failed: {err}"));
                    return;
                }
            }
        } else {
            let recent: Vec<&Message> = (0..msg_range_end)
                .filter_map(|i| self.session.messages.get(i))
                .collect();
            match seam_mgr
                .recompact(&existing_seams, &recent, level, 0, msg_range_end)
                .await
            {
                Ok(text) => text,
                Err(err) => {
                    crate::logging::warn(format!("L{level} recompact failed: {err}"));
                    return;
                }
            }
        };

        if seam_text.is_empty() {
            return;
        }

        // Capture seam count before the mutable borrow below.
        let seam_count = seam_mgr.seam_count().await;

        // Append the seam as an assistant message. This is an append-only
        // operation — no messages are deleted. The prefix cache stays hot.
        self.add_session_message(Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: seam_text,
                cache_control: None,
            }],
        })
        .await;

        let _ = self
            .tx_event
            .send(Event::status(format!(
                "⏻ L{level} seam complete ({seam_count} total, {msg_range_end} messages covered)"
            )))
            .await;
    }
    /// Refresh the stable system prompt based on current non-mode context.
    fn refresh_system_prompt(&mut self) {
        let user_memory_block = crate::memory::compose_block(
            self.config.memory_enabled && !self.config.moraine_fallback, // TODO(v0.8.71): remove when Moraine recall stable; see #3490, #3495
            &self.config.memory_path,
        );
        let prompt_goal_objective = goal_objective_for_prompt(
            self.config.goal_objective.as_deref(),
            &self.config.goal_state,
        );
        let base = prompts::system_prompt_for_mode_with_context_skills_session_and_approval(
            &self.config.workspace,
            None,
            Some(&self.config.skills_dir),
            Some(&self.config.instructions),
            prompts::PromptSessionContext {
                user_memory_block: user_memory_block.as_deref(),
                goal_objective: prompt_goal_objective.as_deref(),
                project_context_pack_enabled: self.config.project_context_pack_enabled,
                locale_tag: &self.config.locale_tag,
                translation_enabled: self.config.translation_enabled,
                model_id: &self.config.model,
                context_window_override: Some(crate::route_budget::route_context_window_tokens(
                    self.api_provider,
                    &self.config.model,
                    self.active_route_limits,
                )),
                show_thinking: self.config.show_thinking,
                verbosity: self.config.verbosity.as_deref(),
                skills_scan_codewhale_only: self.config.skills_scan_codewhale_only,
            },
        );
        let mut stable_prompt =
            merge_system_prompts(Some(&base), self.session.compaction_summary_prompt.clone());

        // SlopLedger completion-gate: inject unresolved slop entries into the
        // system prompt so the agent can autonomously review them before
        // claiming the task is done (#2127).
        let gate_block = self.slop_ledger_gate_block();
        if let Some(ref block) = gate_block
            && let Some(SystemPrompt::Text(prompt_text)) = &mut stable_prompt
        {
            prompt_text.push_str("\n\n");
            prompt_text.push_str(block);
        }

        let stable_hash = system_prompt_hash(stable_prompt.as_ref());
        if self.session.system_prompt_override {
            return;
        }
        if self.session.last_system_prompt_hash != Some(stable_hash) {
            self.session.system_prompt = stable_prompt;
            self.session.last_system_prompt_hash = Some(stable_hash);
        }
    }

    fn slop_ledger_gate_block(&mut self) -> Option<String> {
        let modified = crate::slop_ledger::SlopLedger::default_path()
            .ok()
            .and_then(|path| std::fs::metadata(path).ok())
            .and_then(|metadata| metadata.modified().ok());

        if let Some((cached_modified, cached_block)) = &self.slop_ledger_gate_cache
            && *cached_modified == modified
        {
            return cached_block.clone();
        }

        let loaded = crate::slop_ledger::SlopLedger::load()
            .ok()
            .and_then(|ledger| {
                if ledger.has_open_entries() {
                    ledger.completion_gate_summary()
                } else {
                    None
                }
            });
        self.slop_ledger_gate_cache = Some((modified, loaded.clone()));
        loaded
    }

    /// Merge a compaction summary into the system prompt.
    ///
    /// **Zone affiliation (#2264)**: this mutates the system prompt, which is
    /// part of the `PinnedPrefix` zone in the three-zone contract. Compaction
    /// is the one intentional mid-session prefix mutation — the engine
    /// intentionally accepts the cache-invalidation cost because the
    /// context-reduction benefit outweighs it.
    fn merge_compaction_summary(&mut self, summary_prompt: Option<SystemPrompt>) {
        if summary_prompt.is_none() {
            return;
        }
        self.session.compaction_summary_prompt = merge_system_prompts(
            self.session.compaction_summary_prompt.as_ref(),
            summary_prompt.clone(),
        );
        let merged = merge_system_prompts(self.session.system_prompt.as_ref(), summary_prompt);
        self.session.last_system_prompt_hash = Some(system_prompt_hash(merged.as_ref()));
        self.session.system_prompt = merged;
    }
}

fn default_plugin_tools_dir() -> PathBuf {
    codewhale_config::codewhale_home()
        .unwrap_or_else(|_| {
            dirs::home_dir().map_or_else(|| PathBuf::from(".codewhale"), |h| h.join(".codewhale"))
        })
        .join("tools")
}

fn plugin_tools_dir(tools_config: Option<&crate::config::ToolsConfig>) -> PathBuf {
    if let Some(tools_config) = tools_config
        && let Some(custom_dir) = tools_config.plugin_dir.as_deref()
    {
        return PathBuf::from(shellexpand::tilde(custom_dir).as_ref());
    }
    default_plugin_tools_dir()
}

fn configure_plugin_tools(
    tool_registry: &mut crate::tools::ToolRegistry,
    tools_config: Option<&crate::config::ToolsConfig>,
) -> std::collections::HashSet<String> {
    let names_before: std::collections::HashSet<String> = tool_registry
        .names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();

    let plugin_dir = plugin_tools_dir(tools_config);
    tool_registry.load_plugins(&plugin_dir);

    if let Some(tools_config) = tools_config
        && let Some(ref overrides) = tools_config.overrides
    {
        tool_registry.apply_overrides(overrides, &plugin_dir);
    }

    let names_after: std::collections::HashSet<String> = tool_registry
        .names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    &names_after - &names_before
}

fn system_prompt_hash(prompt: Option<&SystemPrompt>) -> u64 {
    let mut hasher = DefaultHasher::new();
    match prompt {
        Some(SystemPrompt::Text(text)) => {
            0u8.hash(&mut hasher);
            text.hash(&mut hasher);
        }
        Some(SystemPrompt::Blocks(blocks)) => {
            1u8.hash(&mut hasher);
            for block in blocks {
                block.block_type.hash(&mut hasher);
                block.text.hash(&mut hasher);
                if let Some(cache_control) = &block.cache_control {
                    cache_control.cache_type.hash(&mut hasher);
                }
            }
        }
        None => {
            2u8.hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn normalized_goal_objective(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn sync_goal_state_from_host(
    goal_state: &SharedGoalState,
    objective: Option<&str>,
    token_budget: Option<u32>,
    status: GoalStatus,
) {
    match goal_state.lock() {
        Ok(mut state) => state.sync_from_host_status(objective, token_budget, status),
        Err(err) => tracing::warn!("goal state lock poisoned while syncing host goal: {err}"),
    }
}

fn goal_objective_for_prompt(
    configured_goal: Option<&str>,
    goal_state: &SharedGoalState,
) -> Option<String> {
    match goal_state.lock() {
        Ok(state) => {
            if let Some(objective) = state.objective() {
                // Preserve original behavior: return None (not fallback) when
                // objective exists but goal is inactive.
                return state.is_active().then(|| objective.to_string());
            }
        }
        Err(err) => tracing::warn!("goal state lock poisoned while building prompt: {err}"),
    }
    normalized_goal_objective(configured_goal)
}

// ── Mode & approval prompts as request-time runtime metadata ─────────
//
// Mode contracts and approval policies are not persisted in the session
// history and are not sent as extra system messages. Instead, each API
// request projects a transient user-role runtime metadata message at the
// tail. The stable system prompt remains byte-stable, stored history remains
// byte-stable, and strict chat-template providers never see a system message
// outside messages[0].

#[derive(Debug, Clone)]
struct EffectiveInputPolicy {
    mode: AppMode,
    allow_shell: bool,
    trust_mode: bool,
    auto_approve: bool,
    approval_mode: crate::tui::approval::ApprovalMode,
    dynamic_active_tools: Vec<&'static str>,
    status: Option<String>,
}

fn effective_input_policy(
    provenance: UserInputProvenance,
    requested_mode: AppMode,
    _content: &str,
    allow_shell: bool,
    trust_mode: bool,
    auto_approve: bool,
    approval_mode: crate::tui::approval::ApprovalMode,
) -> EffectiveInputPolicy {
    let mut mode = requested_mode;
    let mut trust_mode = trust_mode;
    let mut auto_approve = auto_approve;
    let mut approval_mode = approval_mode;
    let dynamic_active_tools = Vec::new();
    let mut status = None;

    if !provenance_can_inherit_standing_auto_authority(provenance) {
        let had_auto_authority = matches!(mode, AppMode::Yolo)
            || trust_mode
            || auto_approve
            || matches!(approval_mode, crate::tui::approval::ApprovalMode::Bypass);
        if matches!(mode, AppMode::Yolo) {
            mode = AppMode::Agent;
        }
        trust_mode = false;
        auto_approve = false;
        if matches!(
            approval_mode,
            crate::tui::approval::ApprovalMode::Auto | crate::tui::approval::ApprovalMode::Bypass
        ) {
            approval_mode = crate::tui::approval::ApprovalMode::Suggest;
        }
        if had_auto_authority {
            status = Some(format!(
                "Input provenance '{}' cannot inherit standing auto-approval authority; continuing with approvals required.",
                provenance.as_str()
            ));
        }
    }

    EffectiveInputPolicy {
        mode,
        allow_shell,
        trust_mode,
        auto_approve,
        approval_mode,
        dynamic_active_tools,
        status,
    }
}

fn provenance_can_inherit_standing_auto_authority(provenance: UserInputProvenance) -> bool {
    matches!(
        provenance,
        UserInputProvenance::ExternalUser
            | UserInputProvenance::Runtime
            | UserInputProvenance::SubAgentHandoff
    )
}

fn agent_approval_mode_for_turn(
    auto_approve: bool,
    approval_mode: crate::tui::approval::ApprovalMode,
) -> crate::tui::approval::ApprovalMode {
    if auto_approve {
        crate::tui::approval::ApprovalMode::Bypass
    } else {
        approval_mode
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ToolAskRuleDecision {
    Prompt(String),
    Block(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AutoReviewPlanDecision {
    NoChange,
    ForcePrompt(String),
    Block(String),
}

pub(super) fn auto_review_run_origin_for_plan(
    detached_start: bool,
) -> crate::tui::auto_review::RunOrigin {
    if detached_start {
        crate::tui::auto_review::RunOrigin::Background
    } else {
        crate::tui::auto_review::RunOrigin::Interactive
    }
}

// The parameter list intentionally mirrors `AutoReviewContext::from_tool_call`,
// which this thin wrapper builds; the 8 call sites (1 prod + tests) read clearer
// passing the fields than constructing a context first.
#[allow(clippy::too_many_arguments)]
pub(super) fn auto_review_plan_decision(
    policy: &crate::tui::auto_review::AutoReviewPolicy,
    tool_name: &str,
    tool_input: &Value,
    run_origin: crate::tui::auto_review::RunOrigin,
    approval_mode: crate::tui::approval::ApprovalMode,
    user_intent: Option<&str>,
    workspace_trusted: bool,
    dirty_worktree: bool,
) -> (AutoReviewPlanDecision, Value) {
    let context = crate::tui::auto_review::AutoReviewContext::from_tool_call(
        tool_name,
        tool_input,
        run_origin,
        approval_mode,
        user_intent,
        workspace_trusted,
        dirty_worktree,
    );
    let decision = policy.evaluate(&context);
    let audit_event = policy.audit_event(&context, &decision);
    let plan_decision = match decision.action {
        crate::tui::auto_review::AutoReviewAction::Allow
        | crate::tui::auto_review::AutoReviewAction::AskUser => AutoReviewPlanDecision::NoChange,
        crate::tui::auto_review::AutoReviewAction::HoldForReview => {
            // HoldForReview only originates from the built-in safety floor
            // (configured rules produce Allow/Block), so name the gate
            // honestly instead of blaming an "auto-review policy" the user
            // may never have configured (#3883).
            let reason = format!(
                "Built-in safety gate requires approval: {}",
                decision.reason
            );
            if matches!(approval_mode, crate::tui::approval::ApprovalMode::Never) {
                AutoReviewPlanDecision::Block(reason)
            } else {
                AutoReviewPlanDecision::ForcePrompt(reason)
            }
        }
        crate::tui::auto_review::AutoReviewAction::Block => AutoReviewPlanDecision::Block(format!(
            "Auto-review policy blocked tool '{tool_name}': {}",
            decision.reason
        )),
    };
    (plan_decision, audit_event)
}

pub(super) fn exec_shell_ask_rule_decision(
    config: &EngineConfig,
    tool_name: &str,
    tool_input: &Value,
    workspace: &Path,
    approval_mode: crate::tui::approval::ApprovalMode,
) -> Option<ToolAskRuleDecision> {
    if tool_name != "exec_shell" {
        return None;
    }
    let command = tool_input.get("command").and_then(Value::as_str)?;
    tool_ask_rule_decision_for_context(config, tool_name, command, None, workspace, approval_mode)
}

pub(super) fn file_tool_ask_rule_decision(
    config: &EngineConfig,
    tool_name: &str,
    tool_input: &Value,
    workspace: &Path,
    approval_mode: crate::tui::approval::ApprovalMode,
) -> Option<ToolAskRuleDecision> {
    let paths = file_tool_permission_paths(tool_name, tool_input)?;
    if paths.is_empty() {
        return tool_ask_rule_decision_for_context(
            config,
            tool_name,
            "",
            None,
            workspace,
            approval_mode,
        );
    }

    let mut prompt: Option<String> = None;
    for path in paths {
        match tool_ask_rule_decision_for_context(
            config,
            tool_name,
            "",
            Some(&path),
            workspace,
            approval_mode,
        ) {
            Some(ToolAskRuleDecision::Block(reason)) => {
                return Some(ToolAskRuleDecision::Block(reason));
            }
            Some(ToolAskRuleDecision::Prompt(reason)) => {
                prompt.get_or_insert(reason);
            }
            None => {}
        }
    }
    prompt.map(ToolAskRuleDecision::Prompt)
}

fn tool_ask_rule_decision_for_context(
    config: &EngineConfig,
    tool_name: &str,
    command: &str,
    path: Option<&str>,
    workspace: &Path,
    approval_mode: crate::tui::approval::ApprovalMode,
) -> Option<ToolAskRuleDecision> {
    let cwd = workspace.to_string_lossy();
    let ask_for_approval = match approval_mode {
        crate::tui::approval::ApprovalMode::Never => AskForApproval::Never,
        crate::tui::approval::ApprovalMode::Auto
        | crate::tui::approval::ApprovalMode::Bypass
        | crate::tui::approval::ApprovalMode::Suggest => AskForApproval::OnFailure,
    };
    let decision = config
        .exec_policy_engine
        .check(ExecPolicyContext {
            command,
            cwd: cwd.as_ref(),
            tool: Some(tool_name),
            path,
            ask_for_approval,
            sandbox_mode: None,
        })
        .ok()?;
    if !decision.allow {
        Some(ToolAskRuleDecision::Block(decision.reason().to_string()))
    } else if decision.requires_approval {
        Some(ToolAskRuleDecision::Prompt(decision.reason().to_string()))
    } else {
        None
    }
}

fn file_tool_permission_paths(tool_name: &str, input: &Value) -> Option<Vec<String>> {
    match tool_name {
        "read_file" | "write_file" | "edit_file" | "file_search" | "grep_files" => {
            Some(string_field(input, "path").into_iter().collect())
        }
        "list_dir" => Some(vec![
            string_field(input, "path").unwrap_or_else(|| ".".to_string()),
        ]),
        "apply_patch" => Some(apply_patch_permission_paths(input)),
        _ => None,
    }
}

fn string_field(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn apply_patch_permission_paths(input: &Value) -> Vec<String> {
    crate::tools::apply_patch::preflight_apply_patch(input)
        .map(|preflight| preflight.touched_files)
        .unwrap_or_default()
}

/// Spawn the engine in a background task
pub fn spawn_engine(config: EngineConfig, api_config: &Config) -> EngineHandle {
    let (engine, handle) = Engine::new(config, api_config);

    spawn_supervised(
        "engine-event-loop",
        std::panic::Location::caller(),
        async move {
            engine.run().await;
        },
    );

    handle
}

#[cfg(test)]
pub(crate) struct MockEngineHandle {
    pub handle: EngineHandle,
    pub rx_op: mpsc::Receiver<Op>,
    rx_approval: mpsc::Receiver<ApprovalDecision>,
    pub rx_steer: mpsc::Receiver<String>,
    pub tx_event: mpsc::Sender<Event>,
    pub cancel_token: CancellationToken,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MockApprovalEvent {
    Approved {
        id: String,
    },
    Denied {
        id: String,
    },
    RetryWithPolicy {
        id: String,
        policy: crate::sandbox::SandboxPolicy,
    },
}

#[cfg(test)]
impl MockEngineHandle {
    pub(crate) async fn recv_approval_event(&mut self) -> Option<MockApprovalEvent> {
        match self.rx_approval.recv().await? {
            ApprovalDecision::Approved { id } => Some(MockApprovalEvent::Approved { id }),
            ApprovalDecision::Denied { id } => Some(MockApprovalEvent::Denied { id }),
            ApprovalDecision::RetryWithPolicy { id, policy } => {
                Some(MockApprovalEvent::RetryWithPolicy { id, policy })
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn mock_engine_handle() -> MockEngineHandle {
    let (tx_op, rx_op) = mpsc::channel(32);
    let (tx_event, rx_event) = mpsc::channel(256);
    let (tx_approval, rx_approval) = mpsc::channel(64);
    let (tx_user_input, _rx_user_input) = mpsc::channel(32);
    let (tx_steer, rx_steer) = mpsc::channel(64);
    let cancel_token = CancellationToken::new();
    let shared_cancel_token = Arc::new(StdMutex::new(cancel_token.clone()));
    let cancel_reason: Arc<StdMutex<Option<CancelReason>>> = Arc::new(StdMutex::new(None));
    let shared_paused = Arc::new(StdMutex::new(false));
    let handle = EngineHandle {
        tx_op,
        rx_event: Arc::new(RwLock::new(rx_event)),
        cancel_token: shared_cancel_token,
        cancel_reason,
        tx_approval,
        tx_user_input,
        tx_steer,
        shared_paused,
    };

    MockEngineHandle {
        handle,
        rx_op,
        rx_approval,
        rx_steer,
        tx_event,
        cancel_token,
    }
}

mod approval;
mod context;
mod handle;
pub(crate) use context::compact_tool_result_for_context;
/// Public so external hosts/wrappers can reuse the engine's input-budget math
/// (see `context_input_budget_for_route`'s doc) instead of re-deriving it.
pub use context::context_input_budget_for_route;
#[cfg(test)]
use context::route_context_budget_for_provider;
use context::{
    MAX_CONTEXT_RECOVERY_ATTEMPTS, MIN_RECENT_MESSAGES_TO_KEEP,
    effective_max_output_tokens_for_route, estimate_input_tokens_conservative,
    extract_compaction_summary_prompt, is_context_length_error_message,
    route_context_budget_for_route, summarize_text,
};
#[cfg(test)]
use context::{context_input_budget_for_provider, effective_max_output_tokens};
mod dispatch;
mod lsp_hooks;
mod streaming;
mod token_estimate_cache;
mod tool_catalog;
mod tool_execution;
mod tool_setup;
mod turn_loop;
pub(crate) use token_estimate_cache::TokenEstimateCache;

pub(super) const MAX_PARALLEL_SHELL_EXEC: usize = 4;

pub(crate) fn default_active_native_tool_names() -> &'static [&'static str] {
    tool_catalog::DEFAULT_ACTIVE_NATIVE_TOOLS
}

/// Drop catalog entries the execution gates would reject (#3027): the model
/// should never be advertised a tool it cannot call. Deny wins over allow.
fn filter_tool_catalog_for_gates(
    catalog: &mut Vec<Tool>,
    allowed_tools: Option<&[String]>,
    disallowed_tools: Option<&[String]>,
) {
    catalog.retain(|tool| {
        !turn_loop::command_denies_tool(disallowed_tools, &tool.name)
            && turn_loop::command_allows_tool(allowed_tools, &tool.name)
    });
}

use self::approval::{ApprovalDecision, ApprovalResult, UserInputDecision};
#[cfg(test)]
use self::dispatch::should_parallelize_tool_batch;
use self::dispatch::{
    ParallelToolResult, ParallelToolResultEntry, ToolApprovalStamp, ToolExecGuard, ToolExecOutcome,
    ToolExecutionBatch, ToolExecutionPlan, caller_allowed_for_tool, caller_type_for_tool_use,
    final_tool_input, format_tool_error, malformed_tool_arguments_error,
    malformed_tool_arguments_input, mcp_tool_approval_description, mcp_tool_is_parallel_safe,
    mcp_tool_is_read_only, parse_parallel_tool_calls, parse_tool_input,
    plan_tool_execution_batches, should_force_update_plan_first, should_stop_after_plan_tool,
    stamp_tool_result_approval,
};
#[cfg(test)]
use self::lsp_hooks::edited_paths_for_tool;
#[cfg(test)]
use self::streaming::TOOL_CALL_START_MARKERS;
use self::streaming::{
    ContentBlockKind, FAKE_WRAPPER_NOTICE, MAX_STREAM_ERRORS_BEFORE_FAIL, MAX_STREAM_RETRIES,
    MAX_TRANSPARENT_STREAM_RETRIES, STREAM_MAX_CONTENT_BYTES, STREAM_MAX_DURATION_SECS,
    ToolUseState, contains_fake_tool_wrapper, filter_tool_call_delta, should_resume_after_sleep,
    should_transparently_retry_stream, sleep_gap_detected, stream_read_error_user_message,
};
use self::tool_catalog::{
    CODE_EXECUTION_TOOL_NAME, JS_EXECUTION_TOOL_NAME, MULTI_TOOL_PARALLEL_NAME,
    REQUEST_USER_INPUT_NAME, active_tools_for_step, build_model_tool_catalog_with_surface,
    ensure_advanced_tooling, execute_code_execution_tool, execute_tool_search,
    initial_active_tools, is_tool_search_tool, maybe_hydrate_requested_deferred_tool,
    missing_tool_error_message, tool_catalog_consistency_issues,
};
#[cfg(test)]
use self::tool_catalog::{
    TOOL_SEARCH_NAME, build_model_tool_catalog, maybe_activate_requested_deferred_tool,
    preflight_requested_deferred_tool, should_default_defer_tool,
};
use self::tool_execution::emit_tool_audit;
use self::tool_setup::{sandbox_policy_for_mode, shell_policy_for_mode};
use crate::tools::js_execution::execute_js_execution_tool;

#[cfg(test)]
mod tests;


