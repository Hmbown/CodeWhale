//! Events emitted by the core engine to the UI.
//!
//! These events flow from the engine to the TUI via a channel,
//! enabling non-blocking, real-time updates.

use std::{path::PathBuf, sync::Arc};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::config::ApiProvider;
use crate::error_taxonomy::ErrorEnvelope;
use crate::models::{Message, SystemPrompt, Tool, Usage};
use crate::tools::goal::GoalSnapshot;
use crate::tools::spec::{ToolError, ToolResult};
use crate::tools::subagent::{AgentWorkerStatus, CoordinationDetailProjection, SubAgentResult};
use crate::tools::user_input::UserInputRequest;

/// Final status for a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcomeStatus {
    Completed,
    Interrupted,
    Failed,
}

/// Provider/model route resolved for a model-backed turn.
///
/// Emitted at `RouteDispatched` so hosts retain provenance until the matching
/// `TurnComplete` without relying on mutable global selection state. Non-model
/// turns such as composer `!` shell commands use no route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRoute {
    pub provider: ApiProvider,
    /// Exact non-secret configured route key. Named custom providers all map
    /// to [`ApiProvider::Custom`], so the enum alone is not provenance.
    pub provider_identity: String,
    pub model: String,
    pub auto_model: bool,
    /// Secret-free proof of the endpoint and credential generation the turn's
    /// client was *installed* on, minted from that client rather than re-read
    /// from config later.
    ///
    /// Hosts that dispatch follow-up work derived from this turn's context must
    /// authorize it against this receipt: config is mutable and web config
    /// events are drained ahead of engine events, so anything a host resolves
    /// while handling `TurnStarted` may already describe a different route.
    /// `None` when no concrete client was installed (injected-client engines,
    /// or a client that failed to construct).
    pub receipt: Option<crate::route_receipt::TurnRouteReceipt>,
    /// Billing evidence for the request that was actually put on the wire.
    ///
    /// `None` at `TurnStarted`: a lifecycle start is not a dispatch, and a
    /// route that has not been sent has no billing time, no metering surface,
    /// and no endpoint to attest. Populated exactly once, at the wire
    /// boundary, and delivered on `RouteDispatched`. Consumers that price a
    /// turn must treat `None` as *unknown*, never as a zero-cost turn.
    pub billing: Option<RouteBillingEnvelope>,
    /// Endpoint this turn's client was frozen against, verbatim.
    ///
    /// [`crate::route_receipt::TurnRouteReceipt`] deliberately keeps only a
    /// redacted endpoint identity, which billing cannot classify from, so the
    /// non-secret URL travels here. Captured from the resolved route candidate
    /// at the client-freeze boundary, before any ambient selection state can
    /// move. Empty only when no endpoint was captured, which bills Unknown
    /// rather than guessing.
    pub base_url: String,
    /// Credential/pay-mode product truth captured from the route-scoped config
    /// at the same instant.
    ///
    /// Together with `provider_identity` and `base_url` this is a complete
    /// [`crate::route_billing::DispatchedReceipt`]: every fact billing needs,
    /// frozen at the client-freeze boundary. Consumers must classify from
    /// these fields and must never re-read an ambient `Config` after the turn
    /// starts — by `TurnComplete` a provider switch, an auto-router hop, or a
    /// `/provider` change can have moved it elsewhere.
    pub billing_product: crate::route_billing::RouteProduct,
}

/// Dispatch-time billing evidence. Separate from [`TurnRoute`] so the type
/// system — not a convention — enforces that no caller can read a billing
/// surface, endpoint fingerprint, or dispatch instant off a route that was
/// only *planned*.
///
/// This is deliberately *not* the same thing as the classification receipt
/// carried by [`TurnRoute::base_url`] / [`TurnRoute::billing_product`], and
/// the two are not merged. They are captured at different instants and answer
/// different questions:
///
/// - `base_url` + `billing_product` + `provider_identity` are frozen at the
///   **client-freeze** boundary and answer *which route is this and how does
///   it bill* — a [`crate::route_billing::DispatchedReceipt`]. They must be
///   readable from `TurnStarted` onward so a child turn arriving mid-flight
///   can be billed against the parent's frozen route.
/// - This envelope is stamped at the **wire** boundary and answers *what was
///   actually put on the wire, when*. A planned-but-unsent route has no
///   metering surface and no dispatch instant, so it must be structurally
///   absent rather than defaulted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteBillingEnvelope {
    pub billing_surface: Option<String>,
    pub endpoint_fingerprint: Option<String>,
    pub billing_mode: crate::cost_status::RouteBillingMode,
    pub dispatched_at: DateTime<Utc>,
}

impl TurnRoute {
    /// Priceable envelope for this route, or `None` when the route was never
    /// dispatched. Deliberately not a `Default`-filled envelope: an undispatched
    /// route has no cost, and "no cost" is not "zero cost".
    #[must_use]
    pub fn cost_envelope(&self) -> Option<crate::cost_status::EffectiveRouteEnvelope> {
        let billing = self.billing.as_ref()?;
        Some(crate::cost_status::EffectiveRouteEnvelope {
            provider: self.provider,
            provider_identity: self.provider_identity.clone(),
            model: self.model.clone(),
            billing_surface: billing.billing_surface.clone(),
            endpoint_fingerprint: billing.endpoint_fingerprint.clone(),
            billing_mode: billing.billing_mode,
            dispatched_at: billing.dispatched_at,
        })
    }
}

/// Structured lifecycle metadata paired with a human-readable
/// [`Event::AgentProgress`] message.
///
/// Producers own this classification. UI consumers may bound the display
/// message, but must never recover lifecycle state by parsing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProgressEventMeta {
    pub worker_status: AgentWorkerStatus,
    pub step: Option<u32>,
    /// Canonical action/tool name. Presentation aliases are applied by the UI
    /// when it creates the bounded current-activity projection.
    pub tool_name: Option<String>,
}

impl AgentProgressEventMeta {
    #[must_use]
    pub const fn new(worker_status: AgentWorkerStatus) -> Self {
        Self {
            worker_status,
            step: None,
            tool_name: None,
        }
    }

    #[must_use]
    pub const fn with_step(mut self, step: u32) -> Self {
        self.step = Some(step);
        self
    }

    #[must_use]
    pub fn with_tool(mut self, tool_name: impl Into<String>) -> Self {
        self.tool_name = Some(tool_name.into());
        self
    }
}

/// Events emitted by the engine to update the UI.
#[derive(Debug, Clone)]
pub enum Event {
    /// A route compatibility check omitted tools from the provider request.
    /// Names are wire-safe and bounded by the request catalog; schemas and
    /// validator details never cross this user-visible boundary.
    ToolProjectionWarning {
        provider: String,
        omitted_tool_names: Vec<String>,
        omitted_tool_count: usize,
    },
    // === Streaming Events ===
    /// A new message block has started
    MessageStarted { index: usize },

    /// Incremental text content delta
    MessageDelta { index: usize, content: String },

    /// Message block completed
    MessageComplete { index: usize },

    /// Thinking block started
    ThinkingStarted { index: usize },

    /// Incremental thinking content delta
    ThinkingDelta { index: usize, content: String },

    /// Thinking block completed
    ThinkingComplete { index: usize },

    // === Tool Events ===
    /// Tool call initiated
    ToolCallStarted {
        id: String,
        name: String,
        input: Value,
    },

    /// Best-effort liveness pulse while a tool future remains pending.
    ///
    /// This carries no output and must not change user-visible status or the
    /// transcript. It only prevents the TUI from declaring a healthy,
    /// deliberately long-running tool turn stale.
    ToolCallHeartbeat,

    /// Tool call completed
    ToolCallComplete {
        id: String,
        name: String,
        result: Result<ToolResult, ToolError>,
    },

    // === Turn Lifecycle ===
    /// A new turn has started (user sent a message)
    TurnStarted {
        turn_id: String,
        created_at: DateTime<Utc>,
        /// Legacy/non-model hosts may still attach a route at start. Model
        /// turns emit it separately at the real provider dispatch boundary.
        route: Option<TurnRoute>,
    },

    /// Bounded tool-field projection from a prepared model-client request.
    /// Delivery remains unknown; this event is emitted before connection setup.
    ToolRequestSnapshot {
        snapshot: crate::tool_inspection::ToolInspectionSnapshot,
    },

    /// Immutable billing route captured immediately before the first provider
    /// request, after snapshots and other potentially slow pre-dispatch work.
    RouteDispatched { turn_id: String, route: TurnRoute },

    /// The turn is complete (no more tool calls)
    TurnComplete {
        usage: Usage,
        status: TurnOutcomeStatus,
        error: Option<String>,
        /// Tool catalog sent with this turn's model request.
        tool_catalog: Option<Vec<Tool>>,
        /// API base URL used by this turn's client.
        base_url: Option<String>,
    },

    /// A single model call (turn-step) within the turn completed and the
    /// provider reported usage for it. Unlike `TurnComplete`, which fires
    /// once per turn with the cumulative usage, this fires once per model
    /// request so consumers can attribute tokens (including reasoning and
    /// cache behavior) to individual steps. It is not emitted when the
    /// provider never reported usage for the call — absence is honest, and
    /// fields inside `usage` stay `None` when the provider omits them.
    TurnUsage {
        usage: Usage,
        /// Wall-clock duration of this model call's stream.
        duration_ms: u64,
        /// Wall-clock time from the moment the request was dispatched to the
        /// provider until the first content-bearing stream event arrived
        /// (time to first token). `None` when the call produced no content
        /// or the emitting path does not measure dispatch (reviewer / REPL
        /// consults), so the session metrics never invent a latency.
        first_token_ms: Option<u64>,
        /// Wall-clock time from request dispatch to the usage receipt for
        /// this model call — the whole call including connection setup, not
        /// only the stream. `None` where dispatch is not measured.
        request_ms: Option<u64>,
    },

    /// Runtime goal state changed inside the engine, usually from model-visible
    /// `create_goal` or `update_goal` tool calls.
    GoalUpdated { snapshot: GoalSnapshot },

    /// The interactive engine is in the configured quiet period before one
    /// already-authorized goal continuation. This is lifecycle state, not a
    /// status string: Esc/Ctrl+C can cancel it without pretending a provider
    /// turn is still in flight.
    GoalContinuationWaiting { delay_seconds: u64 },

    /// The between-turn quiet period ended. `interrupted` distinguishes a
    /// user/external cancel from normal expiry or a goal status control.
    GoalContinuationWaitEnded { interrupted: bool },

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
        messages_before: Option<usize>,
        /// Number of messages after compaction.
        messages_after: Option<usize>,
        /// Rendered text of the accumulated compaction summary prompt, if any.
        /// Host layers (e.g. the /v1 runtime) persist this into the thread
        /// record so the summary survives engine reloads — without it the
        /// summary lives only in engine memory and is lost on LRU eviction
        /// or restart (SyncSession re-extracts it from the record prompt).
        summary_prompt: Option<String>,
        /// Conservative input-token estimate for the complete post-compaction
        /// request, including its system prompt. Hosts can use this until the
        /// next provider-reported usage arrives.
        post_input_tokens: Option<u64>,
    },

    /// Context compaction was canceled before it could commit a checkpoint.
    ///
    /// The stable id makes cancellation idempotent and lets host layers settle
    /// the exact durable item without inferring lifecycle from status prose.
    CompactionCancelled {
        id: String,
        auto: bool,
        message: String,
    },

    /// Context purge started.
    PurgeStarted {
        /// Status message for display.
        message: String,
    },

    /// Context purge completed.
    PurgeCompleted {
        /// Number of messages before purge.
        messages_before: usize,
        /// Number of messages after purge.
        messages_after: usize,
        /// How many messages were removed.
        removed_count: usize,
        /// How many replace operations were applied.
        replaced_count: usize,
        /// Summary message for display.
        message: String,
    },

    /// Context purge failed.
    PurgeFailed { message: String },

    /// Context compaction failed.
    CompactionFailed {
        id: String,
        auto: bool,
        message: String,
    },

    // === Sub-Agent Events ===
    /// A sub-agent has been spawned
    AgentSpawned {
        owner_session_id: String,
        id: String,
        prompt: String,
        parent_run_id: Option<String>,
        spawn_depth: u32,
        /// Model the child runtime was actually installed with, after route
        /// resolution. Structured-output hosts surface this so child billing
        /// attribution never depends on reading source or an invoice.
        model: String,
        /// Why the child got that route (`task.model`, `agent_profile.loadout`,
        /// `run.model`, …). `None` for spawn paths that bypass route
        /// resolution (checkpoint resume, engine-internal spawns).
        route_source: Option<String>,
    },

    /// Sub-agent progress update
    AgentProgress {
        owner_session_id: String,
        id: String,
        status: String,
        activity: AgentProgressEventMeta,
        parent_run_id: Option<String>,
        spawn_depth: u32,
    },

    /// Sub-agent completed
    AgentComplete {
        owner_session_id: String,
        id: String,
        result: String,
    },

    /// Receipt for an operator follow-up sent to a child (`Op::FollowUpSubAgent`).
    /// `Ok` carries the delivery outcome (the target id may differ from the
    /// addressed id when a fork was continued from a checkpoint); `Err` is the
    /// exact reason nothing was delivered.
    SubAgentFollowUp {
        owner_session_id: String,
        agent_id: String,
        outcome: Result<crate::tools::subagent::UserFollowUpOutcome, String>,
    },

    /// Sub-agent listing plus the same bounded typed coordination projection
    /// used by machine-readable `agents/coordinate inspect`.
    AgentList {
        owner_session_id: String,
        agents: Vec<SubAgentResult>,
        coordination: CoordinationDetailProjection,
        /// Follow-ups handed to a running child that it has not yet taken at
        /// its next round boundary (`agent_id` → count). Only non-zero entries.
        queued_follow_ups: std::collections::HashMap<String, usize>,
        /// Receipts-only roster of every agent that ran this session (#5479):
        /// status, current step, elapsed and token usage per row, built from
        /// the retained worker records rather than from live agent state, so a
        /// finished agent keeps the numbers it finished with.
        roster: Vec<crate::tui::agent_roster::AgentRosterRow>,
    },

    /// Structured sub-agent mailbox envelope (issue #128). Carries the
    /// monotonic seq + the typed `MailboxMessage` so the UI can route each
    /// envelope to the correct in-transcript card.
    SubAgentMailbox {
        owner_session_id: String,
        /// Engine turn identity. Sequence numbers restart for every mailbox,
        /// so consumers must deduplicate on `(turn_id, seq)`, never `seq`
        /// alone.
        turn_id: String,
        seq: u64,
        message: crate::tools::subagent::MailboxMessage,
    },

    /// Live workflow UI event (#4122). Mirrors a typed `WorkflowUiEvent` JSON
    /// object so the TUI can advance the WorkflowPanel and the compact history
    /// card while a run is still in flight (not only on tool complete).
    WorkflowUi {
        /// Immutable conversation owner. Consumers must compare this before
        /// revealing or applying any workflow state.
        owner_session_id: String,
        run_id: String,
        /// Flattened event JSON: `{"type":"task_started", "at_ms":…, …}`.
        /// Callers inject `run_id` on the object when available.
        event: Value,
    },

    // === System Events ===
    /// An error occurred
    Error {
        envelope: ErrorEnvelope,
        recoverable: bool,
    },

    /// Status message for UI display
    Status { message: String },

    /// Session-owned MCP + plugin boot progress.
    ///
    /// Failures stay on this event (and therefore on the session page) until
    /// retry succeeds. They are not `Status` toasts. `connecting` names the
    /// enabled servers that have not settled yet; `finished` is the terminal
    /// receipt for this boot pass.
    McpSessionBoot {
        /// Monotonic engine-owned event generation. A direct `/mcp` snapshot
        /// may supersede one generation without suppressing later passes.
        generation: u64,
        snapshot: crate::mcp::McpManagerSnapshot,
        connecting: Vec<String>,
        finished: bool,
    },

    /// Rendered `/preview-request` manifest (#1004).
    ///
    /// The engine is the only authority that can rebuild the exact next-turn
    /// request, so the manifest is rendered there and delivered as text. The
    /// payload is normally a redacted, typed manifest — never a request body.
    /// The explicit `base-prompt` mode may instead carry only the exact base
    /// prompt; it never carries runtime/system additions. There is no error
    /// variant: a manifest that cannot describe something says so in a typed
    /// unavailable section instead.
    RequestManifestReady { rendered: String },

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
        /// Tool parameters for approval display. Carried on the event so the
        /// TUI does not need to reconstruct them from `pending_tool_uses`.
        input: Value,
        /// Exact-argument fingerprint, used to scope *denials* (#1617).
        approval_key: String,
        /// Lossy / arity-aware fingerprint, used to scope *approvals* so an
        /// "approve for session" covers later flag variants (v0.8.37).
        approval_grouping_key: String,
        /// The model's explanation of intent before invoking write tools (#2381).
        /// Displayed in the approval view so users understand *why* the change
        /// is being made before reviewing *what* will change.
        intent_summary: Option<String>,
        /// When true, the UI must show the prompt instead of consuming
        /// session/auto approval shortcuts.
        approval_force_prompt: bool,
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
    ElevationRequired {
        tool_id: String,
        tool_name: String,
        command: Option<String>,
        denial_reason: String,
        blocked_network: bool,
        blocked_write: bool,
    },

    /// Observable LSP repair-loop update for the Turn Inspector (#4107).
    /// Carries only summary counts/state — never raw prompt internals.
    LspRepairUpdate {
        diagnostics_found: usize,
        files: usize,
        injected: bool,
    },

    /// Advisory note emitted by the background advisor watcher (#3982).
    ///
    /// A permission decision the runtime made for one proposed tool call
    /// without a user prompt, so the transcript can carry a visible receipt
    /// of who decided and why (the audit log keeps the full record).
    ///
    /// Only decisions a person would otherwise never see are emitted:
    /// Auto-Review guardian verdicts, guardian failures (which deny, fail
    /// closed), and deterministic Auto-Review blocks. Proven-safe
    /// deterministic allows stay silent, like rule-based auto-approvals in
    /// other harnesses, so a routine read does not spam the transcript.
    ToolGateDecision {
        /// The child (sub-agent / Fleet worker) whose call was gated, or
        /// `None` for the parent turn. Hosts route a child's receipt into that
        /// child's transcript.
        agent_id: Option<String>,
        /// Tool-call id the decision applies to.
        tool_id: String,
        /// Tool name as the model called it.
        tool_name: String,
        /// Which gate decided.
        gate: ToolGate,
        /// What it decided.
        decision: ToolGateVerdict,
        /// Reviewer risk tier when a guardian answered (`low`, `medium`,
        /// `high`, `critical`); `None` for deterministic gates and failures.
        risk: Option<String>,
        /// Bounded, control-stripped rationale safe to render as one line.
        reason: String,
    },

    /// Fired fire-and-forget after `TurnComplete` when the advisor is enabled
    /// and the completed turn contained at least one tool call. The note is
    /// a concise LLM-generated summary of concerns observed in the bounded
    /// tool-call slice; it never blocks or fails the parent turn.
    AdvisoryNote {
        /// The turn whose tool calls were reviewed.
        turn_id: String,
        /// Concise advisory text (one to three sentences). May be suppressed
        /// by the emission guard's rate-limit or dedup window.
        note: String,
        /// Number of tool-call pairs that were included in the review slice.
        tool_call_count: u32,
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
        /// Current pinned prefix combined hash (SHA-256, 64 hex chars).
        /// Carried so `/cache stats` can surface it without reaching
        /// into the engine's PrefixStabilityManager.
        pinned_combined_hash: String,
        /// Why the current pin exists: `initial`, `resume`, or
        /// `change:<what>`. Empty when unknown.
        pin_reason: String,
        /// Explanation of the most recent expected miss (declared header
        /// change, history reset, or undeclared drift). Empty when none.
        last_miss_reason: String,
        /// `<context_update>` snapshots appended this session.
        context_updates: u64,
    },
}

const TOOL_PROJECTION_WARNING_MAX_NAMES: usize = 8;
const TOOL_PROJECTION_WARNING_MAX_NAME_CHARS: usize = 64;

/// Return a privacy-safe, display-bounded sample of omitted wire tool names.
///
/// MCP catalogs may contain thousands of tools and their names are supplied by
/// external servers. Keep the exact count separately, but never let one route
/// diagnostic flood a transcript, toast, or runtime record.
#[must_use]
pub fn bounded_tool_projection_warning_names(omitted_tool_names: &[String]) -> Vec<String> {
    omitted_tool_names
        .iter()
        .take(TOOL_PROJECTION_WARNING_MAX_NAMES)
        .map(|name| {
            let cleaned: String = name
                .chars()
                .filter(|c| !c.is_control() && !is_bidi_format_control(*c))
                .collect();
            let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
            let collapsed = if collapsed.is_empty() {
                "<unnamed>".to_string()
            } else {
                collapsed
            };
            if collapsed.chars().count() <= TOOL_PROJECTION_WARNING_MAX_NAME_CHARS {
                return collapsed;
            }
            let mut out: String = collapsed
                .chars()
                .take(TOOL_PROJECTION_WARNING_MAX_NAME_CHARS - 1)
                .collect();
            out.push('…');
            out
        })
        .collect()
}

/// Render the already-bounded name sample, marking an incomplete sample with
/// a language-neutral ellipsis. The exact total remains available in typed
/// runtime metadata.
#[must_use]
pub fn tool_projection_warning_tool_list(
    omitted_tool_names: &[String],
    omitted_tool_count: usize,
) -> String {
    let mut rendered = omitted_tool_names.join(", ");
    if omitted_tool_count > omitted_tool_names.len() {
        if !rendered.is_empty() {
            rendered.push_str(", ");
        }
        rendered.push('…');
    }
    if rendered.is_empty() {
        rendered.push_str("<unnamed>");
    }
    rendered
}

#[must_use]
pub fn tool_projection_warning_message(
    provider: &str,
    omitted_tool_names: &[String],
    omitted_tool_count: usize,
) -> String {
    format!(
        "Warning: {provider} omitted incompatible tools for this request: {}",
        tool_projection_warning_tool_list(omitted_tool_names, omitted_tool_count)
    )
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

/// Which permission gate produced a [`Event::ToolGateDecision`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolGate {
    /// The deterministic Auto-Review policy engine (configured rules plus
    /// the built-in safety floor); never model-reviewed.
    AutoReviewDeterministic,
    /// The one-shot Auto-Review model guardian consulted for a fallback hold.
    AutoReviewGuardian,
}

impl ToolGate {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AutoReviewDeterministic => "auto_review_deterministic",
            Self::AutoReviewGuardian => "auto_review_guardian",
        }
    }
}

/// What a permission gate decided for one proposed tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolGateVerdict {
    /// The call may run without a user prompt.
    Allowed,
    /// The call was refused with a stated rationale.
    Denied,
    /// The gate could not produce a verdict (timeout, transport error,
    /// unparseable answer) and the call was denied, fail closed.
    Unavailable,
}

impl ToolGateVerdict {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Denied => "denied",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Bound a gate rationale to one safe transcript line: control and bidi
/// format characters are dropped, whitespace is collapsed, and the text is
/// capped so a verbose reviewer cannot flood the transcript.
#[must_use]
pub fn bounded_gate_reason(reason: &str) -> String {
    const MAX_CHARS: usize = 220;
    let cleaned: String = reason
        .chars()
        .filter(|c| !c.is_control() && !is_bidi_format_control(*c))
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_CHARS {
        return collapsed;
    }
    let mut out: String = collapsed.chars().take(MAX_CHARS - 1).collect();
    out.push('…');
    out
}

fn is_bidi_format_control(c: char) -> bool {
    matches!(
        c,
        '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tool_projection_warning_tests {
    use super::*;

    #[test]
    fn projection_warning_names_are_count_and_length_bounded() {
        let names: Vec<String> = (0..12)
            .map(|index| format!("tool-{index}-{}\nspoof", "x".repeat(100)))
            .collect();

        let bounded = bounded_tool_projection_warning_names(&names);

        assert_eq!(bounded.len(), TOOL_PROJECTION_WARNING_MAX_NAMES);
        assert!(
            bounded
                .iter()
                .all(|name| name.chars().count() <= TOOL_PROJECTION_WARNING_MAX_NAME_CHARS)
        );
        assert!(bounded.iter().all(|name| !name.contains('\n')));
        assert!(tool_projection_warning_tool_list(&bounded, names.len()).ends_with(", …"));
    }
}
