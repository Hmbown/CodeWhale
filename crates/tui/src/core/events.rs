//! Events emitted by the core engine to the UI.
//!
//! These events flow from the engine to the TUI via a channel,
//! enabling non-blocking, real-time updates.

use std::{path::PathBuf, sync::Arc};

use serde_json::Value;

use crate::core::coherence::CoherenceState;
use crate::error_taxonomy::ErrorEnvelope;
use crate::models::{Message, SystemPrompt, Usage};
use crate::tools::spec::{ToolError, ToolResult};
use crate::tools::subagent::SubAgentResult;
use crate::tools::user_input::UserInputRequest;

/// Final status for a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcomeStatus {
    Completed,
    Interrupted,
    Failed,
}

/// Events emitted by the engine to update the UI.
#[derive(Debug, Clone)]
pub enum Event {
    // === Streaming Events ===
    /// A new message block has started
    MessageStarted {
        #[allow(dead_code)]
        index: usize,
    },

    /// Incremental text content delta
    MessageDelta {
        #[allow(dead_code)]
        index: usize,
        content: String,
    },

    /// Message block completed
    MessageComplete {
        #[allow(dead_code)]
        index: usize,
    },

    /// Thinking block started
    ThinkingStarted {
        #[allow(dead_code)]
        index: usize,
    },

    /// Incremental thinking content delta
    ThinkingDelta {
        #[allow(dead_code)]
        index: usize,
        content: String,
    },

    /// Thinking block completed
    ThinkingComplete {
        #[allow(dead_code)]
        index: usize,
    },

    // === Tool Events ===
    /// Tool call initiated
    ToolCallStarted {
        id: String,
        name: String,
        input: Value,
    },

    /// Tool execution progress (for long-running tools)
    #[allow(dead_code)]
    ToolCallProgress { id: String, output: String },

    /// Tool call completed
    ToolCallComplete {
        id: String,
        name: String,
        result: Result<ToolResult, ToolError>,
    },

    // === Turn Lifecycle ===
    /// A new turn has started (user sent a message)
    TurnStarted { turn_id: String },

    /// The turn is complete (no more tool calls)
    TurnComplete {
        usage: Usage,
        status: TurnOutcomeStatus,
        error: Option<String>,
    },

    /// Context compaction started.
    CompactionStarted {
        id: String,
        auto: bool,
        message: String,
    },

    /// Context compaction completed.
    CompactionCompleted {
        id: String,
        auto: bool,
        message: String,
        /// Number of messages before compaction.
        #[allow(dead_code)]
        messages_before: Option<usize>,
        /// Number of messages after compaction.
        #[allow(dead_code)]
        messages_after: Option<usize>,
    },

    /// Context compaction failed.
    CompactionFailed {
        id: String,
        auto: bool,
        message: String,
    },

    /// Checkpoint-restart cycle boundary advanced (issue #124). The previous
    /// cycle has already been archived to disk; the engine has swapped its
    /// in-memory message buffer for the seed messages of cycle `to`.
    /// Carries the full briefing record so the UI can populate
    /// `app.cycle_briefings` for `/cycle <n>`.
    CycleAdvanced {
        from: u32,
        to: u32,
        briefing: crate::cycle_manager::CycleBriefing,
    },

    /// Capacity decision telemetry.
    #[allow(dead_code)]
    CapacityDecision {
        session_id: String,
        turn_id: String,
        h_hat: f64,
        c_hat: f64,
        slack: f64,
        min_slack: f64,
        violation_ratio: f64,
        p_fail: f64,
        risk_band: String,
        action: String,
        cooldown_blocked: bool,
        reason: String,
    },

    /// Capacity intervention telemetry.
    #[allow(dead_code)]
    CapacityIntervention {
        session_id: String,
        turn_id: String,
        action: String,
        before_prompt_tokens: usize,
        after_prompt_tokens: usize,
        compaction_size_reduction: usize,
        replay_outcome: Option<String>,
        replan_performed: bool,
    },

    /// Capacity memory persistence failure telemetry.
    #[allow(dead_code)]
    CapacityMemoryPersistFailed {
        session_id: String,
        turn_id: String,
        action: String,
        error: String,
    },

    /// Plain-language session coherence state.
    CoherenceState {
        state: CoherenceState,
        label: String,
        description: String,
        reason: String,
    },

    // === Sub-Agent Events ===
    /// A sub-agent has been spawned
    AgentSpawned { id: String, prompt: String },

    /// Sub-agent progress update
    AgentProgress { id: String, status: String },

    /// Sub-agent completed
    AgentComplete { id: String, result: String },

    /// Sub-agent listing
    AgentList { agents: Vec<SubAgentResult> },

    /// Structured sub-agent mailbox envelope (issue #128). Carries the
    /// monotonic seq + the typed `MailboxMessage` so the UI can route each
    /// envelope to the correct in-transcript card.
    SubAgentMailbox {
        seq: u64,
        message: crate::tools::subagent::MailboxMessage,
    },

    // === System Events ===
    /// An error occurred
    Error {
        envelope: ErrorEnvelope,
        #[allow(dead_code)]
        recoverable: bool,
    },

    /// Status message for UI display
    Status { message: String },

    /// Pause terminal input events (for interactive subprocesses).
    PauseEvents {
        /// Optional one-shot notification fired after the UI has actually
        /// released the terminal to the child process.
        ack: Option<Arc<tokio::sync::Notify>>,
    },

    /// Resume terminal input events after subprocess completion
    ResumeEvents,

    /// Request user approval for a tool call
    ApprovalRequired {
        id: String,
        tool_name: String,
        description: String,
        /// Fingerprint key for per‑call approval caching (§5.A).
        approval_key: String,
    },

    /// Request user input for a tool call
    UserInputRequired {
        id: String,
        request: UserInputRequest,
    },

    /// Authoritative API conversation state from the engine session.
    ///
    /// The UI receives granular display events, but those are not always a
    /// lossless representation of the API transcript. DeepSeek can emit
    /// reasoning directly followed by tool calls without a visible assistant
    /// text block, and that assistant message still has to be persisted for
    /// later `reasoning_content` replay.
    SessionUpdated {
        session_id: String,
        messages: Vec<Message>,
        system_prompt: Option<SystemPrompt>,
        model: String,
        workspace: PathBuf,
    },

    /// Request user decision after sandbox denial
    #[allow(dead_code)]
    ElevationRequired {
        tool_id: String,
        tool_name: String,
        command: Option<String>,
        denial_reason: String,
        blocked_network: bool,
        blocked_write: bool,
    },

    // === Prefix-Cache Stability Events ===
    /// The prefix (system prompt + tool specs) changed between turns,
    /// which invalidates DeepSeek's KV prefix cache. Carries diagnostics
    /// for the TUI to surface.
    PrefixCacheChange {
        /// Human-readable description of what changed.
        description: String,
        /// Whether the system prompt component changed.
        system_prompt_changed: bool,
        /// Whether the tool set component changed.
        tools_changed: bool,
        /// Overall prefix stability percentage (100 = fully stable).
        stability_pct: u32,
        /// True when the prefix actually changed (cache invalidated).
        /// False for routine stable-check heartbeats.
        changed: bool,
    },
}

impl Event {
    /// Create an error event from a categorized envelope. The envelope's own
    /// `recoverable` flag controls whether the UI flips into offline mode.
    pub fn error(envelope: ErrorEnvelope) -> Self {
        let recoverable = envelope.recoverable;
        Event::Error {
            envelope,
            recoverable,
        }
    }

    /// Create a new status event
    pub fn status(message: impl Into<String>) -> Self {
        Event::Status {
            message: message.into(),
        }
    }
}
