//! Core engine for `DeepSeek` CLI.
//!
//! The engine handles all AI interactions in a background task,
//! communicating with the UI via channels. This enables:
//! - Non-blocking UI during API calls
//! - Real-time streaming updates
//! - Proper cancellation support
//! - Tool execution orchestration

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use serde_json::json;
use tokio::sync::{Mutex as AsyncMutex, RwLock, mpsc};
use tokio_util::sync::CancellationToken;

use crate::client::DeepSeekClient;
use crate::compaction::{
    CompactionConfig, compact_messages_safe, merge_system_prompts, should_compact,
};
use crate::config::{ApiProvider, Config, DEFAULT_MAX_SUBAGENTS, DEFAULT_TEXT_MODEL};
use crate::cycle_manager::{
    CycleBriefing, CycleConfig, StructuredState, archive_cycle, build_seed_messages,
    estimate_briefing_tokens, produce_briefing, should_advance_cycle,
};
use crate::error_taxonomy::{ErrorCategory, ErrorEnvelope, StreamError};
use crate::features::{Feature, Features};
use crate::llm_client::LlmClient;
use crate::mcp::McpPool;
#[cfg(test)]
use crate::models::ToolCaller;
use crate::models::{
    ContentBlock, ContentBlockStart, Delta, LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS, Message,
    MessageRequest, StreamEvent, SystemPrompt, Tool, Usage,
};
use crate::prompts;
use crate::seam_manager::{SeamConfig, SeamManager};
use crate::tools::plan::{SharedPlanState, new_shared_plan_state};
use crate::tools::shell::{SharedShellManager, new_shared_shell_manager};
use crate::tools::spec::RuntimeToolServices;
use crate::tools::spec::{ApprovalRequirement, ToolError, ToolResult};
use crate::tools::subagent::{
    Mailbox, SharedSubAgentManager, SubAgentCompletion, SubAgentForkContext, SubAgentRuntime,
    SubAgentType, new_shared_subagent_manager, resolve_subagent_assignment_route,
};
use crate::tools::todo::{SharedTodoList, new_shared_todo_list};
use crate::tools::user_input::{UserInputRequest, UserInputResponse};
use crate::tools::{ToolContext, ToolRegistryBuilder};
use crate::tui::app::AppMode;
use crate::utils::spawn_supervised;

use super::capacity::{
    CapacityController, CapacityControllerConfig, CapacityDecision, CapacityObservationInput,
    CapacitySnapshot, GuardrailAction, RiskBand,
};
use super::capacity_memory::{
    CanonicalState, CapacityMemoryRecord, ReplayInfo, append_capacity_record,
    load_last_k_capacity_records, new_record_id, now_rfc3339,
};
use super::coherence::{CoherenceSignal, CoherenceState, next_coherence_state};
use super::events::{Event, TurnOutcomeStatus};
use super::ops::Op;
use super::session::Session;
use super::tool_parser;
use super::turn::{TurnContext, TurnToolCall, post_turn_snapshot, pre_turn_snapshot};

// === Types ===

/// Configuration for the engine
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Model identifier to use for responses.
    pub model: String,
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
    /// Additional instruction files concatenated into the system
    /// prompt (#454). Loaded in declared order from the user's
    /// `instructions = [...]` config (or the per-project override).
    /// Resolved via `expand_path` so `~` works.
    pub instructions: Vec<PathBuf>,
    pub project_context_pack_enabled: bool,
    /// Maximum number of assistant steps before stopping.
    pub max_steps: u32,
    /// Maximum number of concurrently active subagents.
    pub max_subagents: usize,
    /// Feature flags controlling tool availability.
    pub features: Features,
    /// Auto-compaction settings for long conversations.
    ///
    /// As of v0.6.6 the high-level summarization compaction (`compact_messages_safe`)
    /// is **disabled by default**; the checkpoint-restart cycle architecture
    /// (`cycle_manager`) replaces it. The compaction config is still wired through
    /// for the per-tool-result truncation path (`compact_tool_result_for_context`)
    /// and for users who explicitly opt back in through the `auto_compact`
    /// setting or a direct engine config.
    pub compaction: CompactionConfig,
    /// Checkpoint-restart cycle settings (issue #124).
    pub cycle: CycleConfig,
    /// Capacity-controller settings.
    pub capacity: CapacityControllerConfig,
    /// Shared Todo list state.
    pub todos: SharedTodoList,
    /// Shared Plan state.
    pub plan_state: SharedPlanState,
    /// Maximum sub-agent recursion depth (default 3). See
    /// `SubAgentRuntime::max_spawn_depth`. Override via
    /// `[runtime] max_spawn_depth = N` in `~/.deepseek/config.toml`.
    pub max_spawn_depth: u32,
    /// Per-domain network policy decider (#135). Shared across the session so
    /// session-scoped approvals (`/network allow <host>`) persist for the
    /// remainder of the run.
    pub network_policy: Option<crate::network_policy::NetworkPolicyDecider>,
    /// Whether to take side-git workspace snapshots before/after each turn.
    pub snapshots_enabled: bool,
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
    /// Path to the user memory file (#489). Always populated; only
    /// consulted when `memory_enabled` is `true`.
    pub memory_path: PathBuf,
    pub goal_objective: Option<String>,
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
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_TEXT_MODEL.to_string(),
            workspace: PathBuf::from("."),
            allow_shell: true,
            trust_mode: false,
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            skills_dir: crate::skills::default_skills_dir(),
            instructions: Vec::new(),
            project_context_pack_enabled: true,
            max_steps: 100,
            max_subagents: DEFAULT_MAX_SUBAGENTS,
            features: Features::with_defaults(),
            compaction: CompactionConfig::default(),
            cycle: CycleConfig::default(),
            capacity: CapacityControllerConfig::default(),
            todos: new_shared_todo_list(),
            plan_state: new_shared_plan_state(),
            max_spawn_depth: crate::tools::subagent::DEFAULT_MAX_SPAWN_DEPTH,
            network_policy: None,
            snapshots_enabled: true,
            lsp_config: None,
            runtime_services: RuntimeToolServices::default(),
            subagent_model_overrides: HashMap::new(),
            memory_enabled: false,
            memory_path: PathBuf::from("./memory.md"),
            strict_tool_mode: false,
            goal_objective: None,
            locale_tag: "en".to_string(),
            workshop: None,
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
    /// Send approval decisions to the engine
    tx_approval: mpsc::Sender<ApprovalDecision>,
    /// Send user input responses to the engine
    tx_user_input: mpsc::Sender<UserInputDecision>,
    /// Send steer input for an in-flight turn.
    tx_steer: mpsc::Sender<String>,
}

impl EngineHandle {
    /// Send an operation to the engine
    pub async fn send(&self, op: Op) -> Result<()> {
        self.tx_op.send(op).await?;
        Ok(())
    }

    /// Cancel the current request
    pub fn cancel(&self) {
        match self.cancel_token.lock() {
            Ok(token) => token.cancel(),
            Err(poisoned) => poisoned.into_inner().cancel(),
        }
    }

    /// Check if a request is currently cancelled
    #[must_use]
    #[allow(dead_code)]
    pub fn is_cancelled(&self) -> bool {
        match self.cancel_token.lock() {
            Ok(token) => token.is_cancelled(),
            Err(poisoned) => poisoned.into_inner().is_cancelled(),
        }
    }

    /// Approve a pending tool call
    pub async fn approve_tool_call(&self, id: impl Into<String>) -> Result<()> {
        self.tx_approval
            .send(ApprovalDecision::Approved { id: id.into() })
            .await?;
        Ok(())
    }

    /// Deny a pending tool call
    pub async fn deny_tool_call(&self, id: impl Into<String>) -> Result<()> {
        self.tx_approval
            .send(ApprovalDecision::Denied { id: id.into() })
            .await?;
        Ok(())
    }

    /// Retry a tool call with an elevated sandbox policy.
    pub async fn retry_tool_with_policy(
        &self,
        id: impl Into<String>,
        policy: crate::sandbox::SandboxPolicy,
    ) -> Result<()> {
        self.tx_approval
            .send(ApprovalDecision::RetryWithPolicy {
                id: id.into(),
                policy,
            })
            .await?;
        Ok(())
    }

    /// Submit a response for request_user_input.
    pub async fn submit_user_input(
        &self,
        id: impl Into<String>,
        response: UserInputResponse,
    ) -> Result<()> {
        self.tx_user_input
            .send(UserInputDecision::Submitted {
                id: id.into(),
                response,
            })
            .await?;
        Ok(())
    }

    /// Cancel a request_user_input prompt.
    pub async fn cancel_user_input(&self, id: impl Into<String>) -> Result<()> {
        self.tx_user_input
            .send(UserInputDecision::Cancelled { id: id.into() })
            .await?;
        Ok(())
    }

    /// Steer an in-flight turn with additional user input.
    pub async fn steer(&self, content: impl Into<String>) -> Result<()> {
        self.tx_steer.send(content.into()).await?;
        Ok(())
    }
}

// === Engine ===

/// The core engine that processes operations and emits events
pub struct Engine {
    config: EngineConfig,
    deepseek_client: Option<DeepSeekClient>,
    deepseek_client_error: Option<String>,
    api_key_env_only_recovery: Option<String>,
    session: Session,
    subagent_manager: SharedSubAgentManager,
    shell_manager: SharedShellManager,
    mcp_pool: Option<Arc<AsyncMutex<McpPool>>>,
    rx_op: mpsc::Receiver<Op>,
    rx_approval: mpsc::Receiver<ApprovalDecision>,
    rx_user_input: mpsc::Receiver<UserInputDecision>,
    rx_steer: mpsc::Receiver<String>,
    tx_event: mpsc::Sender<Event>,
    /// Wakeup channel for the parent turn loop when a direct child sub-agent
    /// terminates (issue #756). Cloned into `SubAgentRuntime` so the runtime
    /// can fan completion events back into the engine.
    tx_subagent_completion: mpsc::UnboundedSender<SubAgentCompletion>,
    /// Receiver paired with `tx_subagent_completion`. Drained at the
    /// turn-loop's empty-tool_uses branch to surface `<deepseek:subagent.done>`
    /// sentinels into the parent's transcript before deciding to end the turn.
    pub(super) rx_subagent_completion: mpsc::UnboundedReceiver<SubAgentCompletion>,
    cancel_token: CancellationToken,
    shared_cancel_token: Arc<StdMutex<CancellationToken>>,
    tool_exec_lock: Arc<RwLock<()>>,
    capacity_controller: CapacityController,
    /// Append-only layered context manager (#159). Opt-in for v0.7.5 while
    /// cache-hit behavior is audited.
    seam_manager: Option<SeamManager>,
    coherence_state: CoherenceState,
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
}

// === Internal tool helpers ===

impl Engine {
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
    }

    fn env_only_api_key_recovery_hint(api_config: &Config) -> Option<String> {
        if !crate::config::active_provider_uses_env_only_api_key(api_config) {
            return None;
        }

        let provider = api_config.api_provider();
        let env_var = match provider {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => "DEEPSEEK_API_KEY",
            ApiProvider::NvidiaNim => "NVIDIA_API_KEY/NVIDIA_NIM_API_KEY",
            ApiProvider::Openai => "OPENAI_API_KEY",
            ApiProvider::Openrouter => "OPENROUTER_API_KEY",
            ApiProvider::Novita => "NOVITA_API_KEY",
            ApiProvider::Fireworks => "FIREWORKS_API_KEY",
            ApiProvider::Sglang => "SGLANG_API_KEY",
            ApiProvider::Vllm => "VLLM_API_KEY",
            ApiProvider::Ollama => "OLLAMA_API_KEY",
        };

        Some(format!(
            "The rejected key came from {env_var}; no saved config key is present.\n\
             Run `deepseek auth status` to inspect credential sources, then \
             `deepseek auth set --provider {provider}` to save a valid key in ~/.deepseek/config.toml, \
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

    /// Create a new engine with the given configuration
    pub fn new(config: EngineConfig, api_config: &Config) -> (Self, EngineHandle) {
        let (tx_op, rx_op) = mpsc::channel(32);
        let (tx_event, rx_event) = mpsc::channel(256);
        let (tx_approval, rx_approval) = mpsc::channel(64);
        let (tx_user_input, rx_user_input) = mpsc::channel(32);
        let (tx_steer, rx_steer) = mpsc::channel(64);
        let (tx_subagent_completion, rx_subagent_completion) = mpsc::unbounded_channel();
        let cancel_token = CancellationToken::new();
        let shared_cancel_token = Arc::new(StdMutex::new(cancel_token.clone()));
        let tool_exec_lock = Arc::new(RwLock::new(()));

        // Create clients for both providers
        let (deepseek_client, deepseek_client_error) = match DeepSeekClient::new(api_config) {
            Ok(client) => (Some(client), None),
            Err(err) => (None, Some(err.to_string())),
        };
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
        let user_memory_block =
            crate::memory::compose_block(config.memory_enabled, &config.memory_path);
        let system_prompt =
            prompts::system_prompt_for_mode_with_context_skills_session_and_approval(
                AppMode::Agent,
                &config.workspace,
                None,
                Some(&config.skills_dir),
                Some(&config.instructions),
                prompts::PromptSessionContext {
                    user_memory_block: user_memory_block.as_deref(),
                    goal_objective: config.goal_objective.as_deref(),
                    project_context_pack_enabled: config.project_context_pack_enabled,
                    locale_tag: &config.locale_tag,
                },
                session.approval_mode,
            );
        let stable_prompt = Some(system_prompt);
        session.last_system_prompt_hash = Some(system_prompt_hash(stable_prompt.as_ref()));
        session.system_prompt = stable_prompt;

        let subagent_manager =
            new_shared_subagent_manager(config.workspace.clone(), config.max_subagents);
        let shell_manager = config
            .runtime_services
            .shell_manager
            .clone()
            .unwrap_or_else(|| new_shared_shell_manager(config.workspace.clone()));
        let capacity_controller = CapacityController::new(config.capacity.clone());

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
                cycle_threshold: api_config
                    .context
                    .cycle_threshold
                    .unwrap_or(crate::seam_manager::DEFAULT_CYCLE_THRESHOLD),
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

        let mut engine = Engine {
            config,
            deepseek_client,
            deepseek_client_error,
            api_key_env_only_recovery,
            session,
            subagent_manager,
            shell_manager,
            mcp_pool: None,
            rx_op,
            rx_approval,
            rx_user_input,
            rx_steer,
            tx_event,
            tx_subagent_completion,
            rx_subagent_completion,
            cancel_token: cancel_token.clone(),
            shared_cancel_token: shared_cancel_token.clone(),
            tool_exec_lock,
            capacity_controller,
            seam_manager,
            coherence_state: CoherenceState::default(),
            turn_counter: 0,
            lsp_manager,
            pending_lsp_blocks: Vec::new(),
            workshop_vars,
            sandbox_backend,
        };
        engine.rehydrate_latest_canonical_state();

        let handle = EngineHandle {
            tx_op,
            rx_event: Arc::new(RwLock::new(rx_event)),
            cancel_token: shared_cancel_token,
            tx_approval,
            tx_user_input,
            tx_steer,
        };

        (engine, handle)
    }

    /// Run the engine event loop
    #[allow(clippy::too_many_lines)]
    pub async fn run(mut self) {
        while let Some(op) = self.rx_op.recv().await {
            match op {
                Op::SendMessage {
                    content,
                    mode,
                    model,
                    goal_objective,
                    reasoning_effort,
                    reasoning_effort_auto,
                    auto_model,
                    allow_shell,
                    trust_mode,
                    auto_approve,
                    approval_mode,
                } => {
                    self.handle_send_message(
                        content,
                        mode,
                        model,
                        goal_objective,
                        reasoning_effort,
                        reasoning_effort_auto,
                        auto_model,
                        allow_shell,
                        trust_mode,
                        auto_approve,
                        approval_mode,
                    )
                    .await;
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
                                "Failed to spawn sub-agent: API client not configured".to_string()
                            });
                        let _ = self
                            .tx_event
                            .send(Event::error(ErrorEnvelope::fatal(message)))
                            .await;
                        continue;
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
                    .with_max_spawn_depth(self.config.max_spawn_depth)
                    .background_runtime();
                    let route = resolve_subagent_assignment_route(&runtime, None, &prompt).await;
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
                    let agents = {
                        let mut manager = self.subagent_manager.write().await;
                        manager.cleanup(Duration::from_secs(60 * 60));
                        manager.list()
                    };
                    let _ = self.tx_event.send(Event::AgentList { agents }).await;
                }
                Op::ChangeMode { mode } => {
                    let _ = self
                        .tx_event
                        .send(Event::status(format!("Mode changed to: {mode:?}")))
                        .await;
                }
                Op::SetModel { model } => {
                    self.session.auto_model = model.trim().eq_ignore_ascii_case("auto");
                    self.session.model = model;
                    self.config.model.clone_from(&self.session.model);
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
                Op::SyncSession {
                    session_id,
                    messages,
                    system_prompt,
                    model,
                    workspace,
                } => {
                    if let Some(session_id) = session_id {
                        self.session.id = session_id;
                    } else if messages.is_empty() && system_prompt.is_none() {
                        self.session.id = uuid::Uuid::new_v4().to_string();
                    }
                    self.session.messages = messages;
                    self.session.compaction_summary_prompt =
                        extract_compaction_summary_prompt(system_prompt.clone());
                    self.session.system_prompt = system_prompt;
                    self.session.auto_model = model.trim().eq_ignore_ascii_case("auto");
                    self.session.model = model;
                    self.session.workspace = workspace.clone();
                    self.config.model.clone_from(&self.session.model);
                    self.config.workspace = workspace.clone();
                    let ctx = crate::project_context::load_project_context_with_parents(&workspace);
                    self.session.project_context = if ctx.has_instructions() {
                        Some(ctx)
                    } else {
                        None
                    };
                    self.session.rebuild_working_set();
                    self.rehydrate_latest_canonical_state();
                    self.emit_session_updated().await;
                    let _ = self
                        .tx_event
                        .send(Event::status("Session context synced".to_string()))
                        .await;
                }
                Op::CompactContext => {
                    self.handle_manual_compaction().await;
                }
                Op::Rlm {
                    content,
                    model,
                    child_model,
                    max_depth,
                } => {
                    self.handle_rlm(content, model, child_model, max_depth)
                        .await;
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
                        self.session.messages.truncate(idx);
                    }
                    // Now dispatch the new message as a normal send,
                    // reusing the engine's stored mode/model config.
                    let mode = AppMode::Agent; // default fallback
                    self.handle_send_message(
                        new_message,
                        mode,
                        self.session.model.clone(),
                        self.config.goal_objective.clone(),
                        self.session.reasoning_effort.clone(),
                        self.session.reasoning_effort_auto,
                        self.session.auto_model,
                        self.session.allow_shell,
                        self.session.trust_mode,
                        self.session.auto_approve,
                        self.session.approval_mode,
                    )
                    .await;
                }
                Op::Shutdown => {
                    break;
                }
            }
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
                messages: self.session.messages.clone(),
                system_prompt: self.session.system_prompt.clone(),
                model: self.session.model.clone(),
                workspace: self.session.workspace.clone(),
            })
            .await;
    }

    async fn add_session_message(&mut self, message: Message) {
        self.session.add_message(message);
        self.emit_session_updated().await;
    }

    fn turn_metadata_block(&self) -> ContentBlock {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let working_set_summary = self
            .session
            .working_set
            .summary_block(&self.config.workspace)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let summary = if let Some(working_set_summary) = working_set_summary {
            format!("Current local date: {today}\n{working_set_summary}")
        } else {
            format!("Current local date: {today}")
        };

        ContentBlock::Text {
            text: format!("<turn_meta>\n{summary}\n</turn_meta>"),
            cache_control: None,
        }
    }

    fn user_text_message_with_turn_metadata(&self, text: String) -> Message {
        Message {
            role: "user".to_string(),
            content: vec![
                self.turn_metadata_block(),
                ContentBlock::Text {
                    text,
                    cache_control: None,
                },
            ],
        }
    }

    /// Handle a send message operation
    #[allow(clippy::too_many_arguments)]
    async fn handle_send_message(
        &mut self,
        content: String,
        mode: AppMode,
        model: String,
        goal_objective: Option<String>,
        reasoning_effort: Option<String>,
        reasoning_effort_auto: bool,
        auto_model: bool,
        allow_shell: bool,
        trust_mode: bool,
        auto_approve: bool,
        approval_mode: crate::tui::approval::ApprovalMode,
    ) {
        // Reset cancel token for fresh turn (in case previous was cancelled)
        self.reset_cancel_token();

        // Drain stale steer messages from previous turns.
        while self.rx_steer.try_recv().is_ok() {}

        // Create turn context first so start event includes a stable turn id.
        let mut turn = TurnContext::new(self.config.max_steps);
        self.turn_counter = self.turn_counter.saturating_add(1);
        self.capacity_controller.mark_turn_start(self.turn_counter);

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
            let pre_workspace = self.session.workspace.clone();
            let pre_seq = self.turn_counter;
            let _ = tokio::task::spawn_blocking(move || pre_turn_snapshot(&pre_workspace, pre_seq))
                .await;
        }

        // A new turn means any leftover retry banner (success cleared
        // it, failure pinned it) is no longer relevant — reset to idle
        // so the footer doesn't display a stale failure row across
        // turns (#499).
        crate::retry_status::clear();

        // Check if we have the appropriate client
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
                })
                .await;
            return;
        }

        self.session
            .working_set
            .observe_user_message(&content, &self.session.workspace);
        let force_update_plan_first = should_force_update_plan_first(mode, &content);

        // Add user message to session
        let user_msg = self.user_text_message_with_turn_metadata(content);
        self.session.add_message(user_msg);

        self.session.model = model;
        self.config.model.clone_from(&self.session.model);
        self.config.goal_objective = goal_objective;
        self.session.reasoning_effort = reasoning_effort;
        self.session.reasoning_effort_auto = reasoning_effort_auto;
        self.session.auto_model = auto_model;
        self.session.allow_shell = allow_shell;
        self.config.allow_shell = allow_shell;
        self.session.trust_mode = trust_mode;
        self.config.trust_mode = trust_mode;
        self.session.auto_approve = auto_approve;
        self.session.approval_mode = if auto_approve {
            crate::tui::approval::ApprovalMode::Auto
        } else {
            approval_mode
        };

        // Update system prompt to match current mode and include persisted compaction context.
        self.refresh_system_prompt(mode);
        self.emit_session_updated().await;

        // Build tool registry and tool list for the current mode
        let todo_list = self.config.todos.clone();
        let plan_state = self.config.plan_state.clone();

        let tool_context = self.build_tool_context(mode, auto_approve);
        let builder = self.build_turn_tool_registry_builder(mode, todo_list, plan_state);

        let fork_context_for_runtime = if self.config.features.enabled(Feature::Subagents) {
            let state = StructuredState::capture(
                mode.label(),
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
        let mailbox_for_runtime = if self.config.features.enabled(Feature::Subagents) {
            let cancel_token = self.cancel_token.child_token();
            let (mailbox, mut receiver) = Mailbox::new(cancel_token.clone());
            let tx_event_clone = self.tx_event.clone();
            spawn_supervised(
                "subagent-mailbox-drainer",
                std::panic::Location::caller(),
                async move {
                    while let Some(envelope) = receiver.recv().await {
                        if tx_event_clone
                            .send(Event::SubAgentMailbox {
                                seq: envelope.seq,
                                message: envelope.message,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                },
            );
            Some((mailbox, cancel_token))
        } else {
            None
        };

        let tool_registry = match mode {
            AppMode::Agent | AppMode::Yolo => {
                if self.config.features.enabled(Feature::Subagents) {
                    let runtime = if let Some(client) = self.deepseek_client.clone() {
                        let mut rt = SubAgentRuntime::new(
                            client,
                            self.session.model.clone(),
                            tool_context.clone(),
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
                        .with_max_spawn_depth(self.config.max_spawn_depth)
                        .with_parent_completion_tx(self.tx_subagent_completion.clone());
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
                    Some(
                        builder
                            .with_subagent_tools(
                                self.subagent_manager.clone(),
                                runtime.expect("sub-agent runtime should exist with active client"),
                            )
                            .build(tool_context),
                    )
                } else {
                    Some(builder.build(tool_context))
                }
            }
            _ => Some(builder.build(tool_context)),
        };

        let mcp_tools = if self.config.features.enabled(Feature::Mcp) {
            self.mcp_tools().await
        } else {
            Vec::new()
        };
        let tools = tool_registry.as_ref().map(|registry| {
            build_model_tool_catalog(registry.to_api_tools_with_cache(true), mcp_tools, mode)
        });

        // Main turn loop
        let (status, error) = self
            .handle_deepseek_turn(
                &mut turn,
                tool_registry.as_ref(),
                tools,
                mode,
                force_update_plan_first,
            )
            .await;

        // Checkpoint-restart cycle boundary (issue #124). Run BEFORE
        // TurnComplete so the engine loop doesn't block the terminal after
        // the turn signal (#234). The status chip ("↻ context refreshing...")
        // is visible during the wait, and once TurnComplete fires the
        // terminal is immediately responsive. No-op unless the estimated
        // input tokens have crossed the per-cycle threshold.
        if matches!(status, TurnOutcomeStatus::Completed) {
            self.maybe_advance_cycle(mode).await;
        }

        // Update session usage
        self.session.total_usage.add(&turn.usage);

        // Emit turn complete event — after all post-turn bookkeeping so
        // the terminal is immediately responsive when the UI receives it.
        let _ = self
            .tx_event
            .send(Event::TurnComplete {
                usage: turn.usage,
                status,
                error,
            })
            .await;

        // Post-turn snapshot. Fire-and-forget: TurnComplete is already
        // emitted, so the UI is unblocked and the user can type / select /
        // paste immediately (#234). The git work proceeds on the blocking
        // pool without forcing the engine loop to await it.
        if self.config.snapshots_enabled {
            let post_workspace = self.session.workspace.clone();
            let post_seq = self.turn_counter;
            crate::utils::spawn_blocking_supervised("post-turn-snapshot", move || {
                post_turn_snapshot(&post_workspace, post_seq);
            });
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
                    self.session.messages = result.messages;
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
            })
            .await;
    }

    /// Handle a Recursive Language Model (RLM) query — Algorithm 1 from
    /// Zhang et al. (arXiv:2512.24601).
    ///
    /// The prompt is stored as PROMPT in a REPL variable. The root LLM
    /// only sees metadata about the REPL state, never the prompt text
    /// directly. The model generates Python code, which is executed by
    /// the REPL. When FINAL() is called, the loop ends.
    async fn handle_rlm(
        &mut self,
        content: String,
        model: String,
        child_model: String,
        max_depth: u32,
    ) {
        use crate::rlm::turn::run_rlm_turn;

        let Some(ref client) = self.deepseek_client else {
            let err = self
                .deepseek_client_error
                .as_deref()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "API client not configured".to_string());
            let _ = self
                .tx_event
                .send(Event::error(ErrorEnvelope::fatal_auth(format!(
                    "RLM error: {err}"
                ))))
                .await;
            return;
        };

        let _ = self
            .tx_event
            .send(Event::status("RLM turn started".to_string()))
            .await;

        let result = run_rlm_turn(
            client,
            model,
            content,
            child_model,
            self.tx_event.clone(),
            max_depth,
        )
        .await;

        let has_error = result.error.is_some();
        if let Some(ref err) = result.error {
            let _ = self
                .tx_event
                .send(Event::error(ErrorEnvelope::tool(format!(
                    "RLM error: {err}"
                ))))
                .await;
        }

        if !result.answer.is_empty() {
            // Add the final answer as an assistant message in the session.
            self.add_session_message(crate::models::Message {
                role: "assistant".to_string(),
                content: vec![crate::models::ContentBlock::Text {
                    text: result.answer.clone(),
                    cache_control: None,
                }],
            })
            .await;

            let _ = self
                .tx_event
                .send(Event::MessageDelta {
                    index: 0,
                    content: result.answer.clone(),
                })
                .await;
            let _ = self
                .tx_event
                .send(Event::MessageComplete { index: 0 })
                .await;
        }

        let _ = self
            .tx_event
            .send(Event::TurnComplete {
                usage: result.usage,
                status: if has_error {
                    crate::core::events::TurnOutcomeStatus::Failed
                } else {
                    crate::core::events::TurnOutcomeStatus::Completed
                },
                error: result.error,
            })
            .await;
    }

    fn estimated_input_tokens(&self) -> usize {
        estimate_input_tokens_conservative(
            &self.session.messages,
            self.session.system_prompt.as_ref(),
        )
    }

    fn trim_oldest_messages_to_budget(&mut self, target_input_budget: usize) -> usize {
        let mut removed = 0usize;
        while self.session.messages.len() > MIN_RECENT_MESSAGES_TO_KEEP
            && self.estimated_input_tokens() > target_input_budget
        {
            self.session.messages.remove(0);
            removed = removed.saturating_add(1);
        }
        removed
    }

    async fn recover_context_overflow(
        &mut self,
        client: &DeepSeekClient,
        reason: &str,
        requested_output_tokens: u32,
    ) -> bool {
        let Some(target_budget) =
            context_input_budget(&self.session.model, requested_output_tokens)
        else {
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
        let mut compacted_messages = self.session.messages.clone();

        let mut forced_config = self.config.compaction.clone();
        forced_config.enabled = true;
        forced_config.token_threshold = forced_config
            .token_threshold
            .min(target_budget.saturating_sub(1))
            .max(1);
        // v0.8.11: forced compaction (capacity guardrail) bypasses the floor
        // because we're at a hard ceiling and have to free budget regardless
        // of cache cost.
        forced_config.auto_floor_tokens = 0;

        match compact_messages_safe(
            client,
            &self.session.messages,
            &forced_config,
            Some(&self.session.workspace),
            None,
            None,
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
            self.session.messages = compacted_messages;
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
                details.push_str(&format!(" ({} retries)", retries_used));
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
             (estimate ~{} tokens, budget ~{}).",
            after_tokens, target_budget
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
        .with_cancel_token(self.cancel_token.clone())
        .with_trusted_external_paths(trusted.paths().to_vec());

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
        let mut pool = McpPool::from_config_path(&self.session.mcp_config_path)
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
                let _ = self.tx_event.send(Event::status(err.to_string())).await;
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
        let Some(ref seam_mgr) = self.seam_manager else {
            return;
        };
        if !seam_mgr.config().enabled {
            return;
        }

        let highest = seam_mgr.highest_level().await;
        let Some(level) = seam_mgr.seam_level_for(self.estimated_input_tokens(), highest) else {
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
    /// its token threshold (issue #124). No-op in the common case.
    ///
    /// Caller must invoke this only at a clean turn boundary (no in-flight
    /// tool, no open stream, no pending approval modal). The phase guard
    /// inside `should_advance_cycle` is a defence-in-depth check; the
    /// engine's wider state machine is the primary enforcement layer.
    ///
    /// Sub-agents are intentionally NOT awaited: each sub-agent has its own
    /// context, the parent's reset doesn't invalidate them. Their handles
    /// are captured in the structured-state block so the next cycle can see
    /// they're still running.
    async fn maybe_advance_cycle(&mut self, mode: AppMode) {
        if !should_advance_cycle(
            self.estimated_input_tokens() as u64,
            turn_response_headroom_tokens(),
            &self.session.model,
            &self.config.cycle,
            false,
        ) {
            return;
        }

        let Some(client) = self.deepseek_client.clone() else {
            crate::logging::warn(
                "Cycle boundary skipped: API client not configured for briefing turn",
            );
            return;
        };

        let from = self.session.cycle_count;
        let to = from.saturating_add(1);
        let archive_started = self.session.current_cycle_started;
        let max_briefing_tokens = self.config.cycle.briefing_max_for(&self.session.model);

        let _ = self
            .tx_event
            .send(Event::status(format!(
                "↻ context refreshing (cycle {from} → {to}, generating briefing…)"
            )))
            .await;

        // 1. Generate the model-curated briefing. Prefer the Flash seam
        //    manager (#159) for cost and speed; fall back to the main model
        //    (legacy produce_briefing) when the seam manager isn't available.
        let briefing_text = if let Some(ref seam_mgr) = self.seam_manager {
            let seams = seam_mgr.collect_seam_texts(&self.session.messages).await;
            let state_text = {
                let s = StructuredState::capture(
                    mode.label(),
                    self.config.workspace.clone(),
                    std::env::current_dir().ok(),
                    &self.session.working_set,
                    &self.config.todos,
                    &self.config.plan_state,
                    Some(&self.subagent_manager),
                )
                .await;
                s.to_system_block()
            };
            match seam_mgr
                .produce_flash_briefing(&seams, state_text.as_deref())
                .await
            {
                Ok(text) => text,
                Err(err) => {
                    crate::logging::warn(format!(
                        "Flash briefing failed, falling back to main model: {err}"
                    ));
                    match produce_briefing(
                        &client,
                        &self.session.model,
                        &self.session.messages,
                        max_briefing_tokens,
                    )
                    .await
                    {
                        Ok(text) => text,
                        Err(err2) => {
                            crate::logging::warn(format!(
                                "Cycle briefing turn failed; skipping cycle advance: {err2}"
                            ));
                            let _ = self
                                .tx_event
                                .send(Event::status(format!(
                                    "↻ cycle handoff failed (continuing in cycle {from}): {err2}"
                                )))
                                .await;
                            return;
                        }
                    }
                }
            }
        } else {
            match produce_briefing(
                &client,
                &self.session.model,
                &self.session.messages,
                max_briefing_tokens,
            )
            .await
            {
                Ok(text) => text,
                Err(err) => {
                    crate::logging::warn(format!(
                        "Cycle briefing turn failed; skipping cycle advance: {err}"
                    ));
                    let _ = self
                        .tx_event
                        .send(Event::status(format!(
                            "↻ cycle handoff failed (continuing in cycle {from}): {err}"
                        )))
                        .await;
                    return;
                }
            }
        };

        let briefing_tokens = estimate_briefing_tokens(&briefing_text);
        let now = chrono::Utc::now();
        let briefing = CycleBriefing {
            cycle: to,
            timestamp: now,
            briefing_text: briefing_text.clone(),
            token_estimate: briefing_tokens,
        };

        // 2. Archive the cycle to disk. If the archive write fails we still
        //    proceed with the swap — the briefing alone preserves enough
        //    state to continue, and the user can recover the lost archive
        //    from their session log if needed.
        match archive_cycle(
            &self.session.id,
            to,
            &self.session.messages,
            &self.session.model,
            archive_started,
        ) {
            Ok(path) => {
                crate::logging::info(format!("Cycle {to} archived to {}", path.display()));
            }
            Err(err) => {
                crate::logging::warn(format!(
                    "Failed to archive cycle {to}; continuing with swap: {err}"
                ));
            }
        }

        // 3. Capture structured state. Locks are held only for the snapshot.
        let state = StructuredState::capture(
            mode.label(),
            self.config.workspace.clone(),
            std::env::current_dir().ok(),
            &self.session.working_set,
            &self.config.todos,
            &self.config.plan_state,
            Some(&self.subagent_manager),
        )
        .await;
        let state_block = state.to_system_block();

        // 4. Build the seed messages. The next cycle starts with the
        //    base system prompt (refreshed below) and these seeds.
        let seed_messages = build_seed_messages(
            state_block.as_deref(),
            Some(&briefing),
            None, // pending_user_message — pulled from steer/queue elsewhere
        );

        // 5. Atomic swap.
        self.session.messages = seed_messages;
        self.session.cycle_count = to;
        self.session.current_cycle_started = now;
        self.session.cycle_briefings.push(briefing.clone());
        // Reset seam tracking for the new cycle.
        if let Some(ref seam_mgr) = self.seam_manager {
            seam_mgr.reset().await;
        }
        // Drop any compaction summary — that path is incompatible with the
        // fresh-context model and would Frankenstein-merge with the briefing.
        self.session.compaction_summary_prompt = None;
        self.refresh_system_prompt(mode);
        self.emit_session_updated().await;

        let _ = self
            .tx_event
            .send(Event::CycleAdvanced {
                from,
                to,
                briefing: briefing.clone(),
            })
            .await;
        let _ = self
            .tx_event
            .send(Event::status(format!(
                "↻ context refreshed (cycle {from} → {to}, briefing: {briefing_tokens} tokens carried)"
            )))
            .await;
    }

    /// Refresh the system prompt based on current mode and context.
    fn refresh_system_prompt(&mut self, mode: AppMode) {
        let user_memory_block =
            crate::memory::compose_block(self.config.memory_enabled, &self.config.memory_path);
        let base = prompts::system_prompt_for_mode_with_context_skills_session_and_approval(
            mode,
            &self.config.workspace,
            None,
            Some(&self.config.skills_dir),
            Some(&self.config.instructions),
            prompts::PromptSessionContext {
                user_memory_block: user_memory_block.as_deref(),
                goal_objective: self.config.goal_objective.as_deref(),
                project_context_pack_enabled: self.config.project_context_pack_enabled,
                locale_tag: &self.config.locale_tag,
            },
            self.session.approval_mode,
        );
        let stable_prompt =
            merge_system_prompts(Some(&base), self.session.compaction_summary_prompt.clone());
        let stable_hash = system_prompt_hash(stable_prompt.as_ref());
        if self.session.last_system_prompt_hash != Some(stable_hash) {
            self.session.system_prompt = stable_prompt;
            self.session.last_system_prompt_hash = Some(stable_hash);
        }
    }

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
    let handle = EngineHandle {
        tx_op,
        rx_event: Arc::new(RwLock::new(rx_event)),
        cancel_token: shared_cancel_token,
        tx_approval,
        tx_user_input,
        tx_steer,
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
mod capacity_flow;
mod context;
pub(crate) use context::compact_tool_result_for_context;
use context::{
    COMPACTION_SUMMARY_MARKER, MAX_CONTEXT_RECOVERY_ATTEMPTS, MIN_RECENT_MESSAGES_TO_KEEP,
    TURN_MAX_OUTPUT_TOKENS, context_input_budget, effective_max_output_tokens,
    estimate_input_tokens_conservative, extract_compaction_summary_prompt,
    is_context_length_error_message, summarize_text, turn_response_headroom_tokens,
};
mod dispatch;
mod loop_guard;
mod lsp_hooks;
mod streaming;
mod tool_catalog;
mod tool_execution;
mod tool_setup;
mod turn_loop;

use self::approval::{ApprovalDecision, ApprovalResult, UserInputDecision};
use self::dispatch::{
    ParallelToolResult, ParallelToolResultEntry, ToolExecGuard, ToolExecOutcome, ToolExecutionPlan,
    caller_allowed_for_tool, caller_type_for_tool_use, final_tool_input, format_tool_error,
    mcp_tool_approval_description, mcp_tool_is_parallel_safe, mcp_tool_is_read_only,
    parse_parallel_tool_calls, parse_tool_input, should_force_update_plan_first,
    should_parallelize_tool_batch, should_stop_after_plan_tool,
};
use self::loop_guard::{AttemptDecision, LoopGuard, OutcomeDecision};
#[cfg(test)]
use self::lsp_hooks::{edited_paths_for_tool, parse_patch_paths};
#[cfg(test)]
use self::streaming::TOOL_CALL_START_MARKERS;
use self::streaming::{
    ContentBlockKind, FAKE_WRAPPER_NOTICE, MAX_STREAM_ERRORS_BEFORE_FAIL,
    MAX_TRANSPARENT_STREAM_RETRIES, STREAM_MAX_CONTENT_BYTES, STREAM_MAX_DURATION_SECS,
    ToolUseState, contains_fake_tool_wrapper, filter_tool_call_delta,
    should_transparently_retry_stream, stream_chunk_timeout_secs,
};
use self::tool_catalog::{
    CODE_EXECUTION_TOOL_NAME, MULTI_TOOL_PARALLEL_NAME, REQUEST_USER_INPUT_NAME,
    active_tools_for_step, build_model_tool_catalog, ensure_advanced_tooling,
    execute_code_execution_tool, execute_tool_search, initial_active_tools, is_tool_search_tool,
    maybe_hydrate_requested_deferred_tool, missing_tool_error_message,
};
#[cfg(test)]
use self::tool_catalog::{
    TOOL_SEARCH_BM25_NAME, maybe_activate_requested_deferred_tool, should_default_defer_tool,
};
use self::tool_execution::emit_tool_audit;
use self::tool_setup::sandbox_policy_for_mode;

#[cfg(test)]
mod tests;
