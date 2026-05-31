use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod runtime {
    use super::*;

    pub const RUNTIME_EVENT_ENVELOPE_SCHEMA_VERSION: u32 = 1;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct RuntimeEventEnvelope {
        #[serde(default = "default_runtime_event_envelope_schema_version")]
        pub schema_version: u32,
        pub seq: u64,
        pub event: String,
        pub kind: String,
        pub thread_id: String,
        pub turn_id: Option<String>,
        pub item_id: Option<String>,
        pub timestamp: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub created_at: Option<String>,
        pub payload: Value,
        #[serde(default)]
        #[serde(flatten)]
        pub extra: BTreeMap<String, Value>,
    }

    fn default_runtime_event_envelope_schema_version() -> u32 {
        RUNTIME_EVENT_ENVELOPE_SCHEMA_VERSION
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub body: T,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Running,
    Idle,
    Completed,
    Failed,
    Paused,
    Archived,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionSource {
    Interactive,
    Resume,
    Fork,
    Api,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub preview: String,
    pub ephemeral: bool,
    pub model_provider: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub status: ThreadStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub cwd: PathBuf,
    pub cli_version: String,
    pub source: SessionSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadStartParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub persist_extended_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadResumeParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality: Option<String>,
    #[serde(default)]
    pub persist_extended_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadForkParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_instructions: Option<String>,
    #[serde(default)]
    pub persist_extended_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadListParams {
    #[serde(default)]
    pub include_archived: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadReadParams {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadSetNameParams {
    pub thread_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ThreadRequest {
    Create {
        #[serde(default)]
        metadata: Value,
    },
    Start(ThreadStartParams),
    Resume(ThreadResumeParams),
    Fork(ThreadForkParams),
    List(ThreadListParams),
    Read(ThreadReadParams),
    SetName(ThreadSetNameParams),
    Archive {
        thread_id: String,
    },
    Unarchive {
        thread_id: String,
    },
    Message {
        thread_id: String,
        input: String,
    },
}

/// Response to a [`ThreadRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadResponse {
    /// The thread this response pertains to.
    pub thread_id: String,
    /// Human-readable status string (e.g. `"ok"`, `"error"`).
    pub status: String,
    /// The thread details, when a single thread is returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<Thread>,
    /// List of threads, populated by `List` requests.
    #[serde(default)]
    pub threads: Vec<Thread>,
    /// The model used for the thread, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The model provider used for the thread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    /// The working directory of the thread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// The active approval policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    /// The active sandbox configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    /// Streaming events associated with this response.
    #[serde(default)]
    pub events: Vec<EventFrame>,
    /// Arbitrary additional response data.
    #[serde(default)]
    pub data: Value,
}

/// Application-level requests that are not tied to a specific thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppRequest {
    /// Query the server's capabilities.
    Capabilities,
    /// Read a configuration value by key.
    ConfigGet { key: String },
    /// Set a configuration key to a value.
    ConfigSet { key: String, value: String },
    /// Remove a configuration key.
    ConfigUnset { key: String },
    /// List all configuration entries.
    ConfigList,
    /// List available models.
    Models,
    /// List threads that are currently loaded in memory.
    ThreadLoadedList,
}

/// Response to an [`AppRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppResponse {
    /// Whether the request succeeded.
    pub ok: bool,
    /// The response payload.
    pub data: Value,
    /// Streaming events associated with this response.
    #[serde(default)]
    pub events: Vec<EventFrame>,
}

/// A simple prompt request that sends text to the model and returns output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptRequest {
    /// Optional thread context for the prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// The prompt text.
    pub prompt: String,
    /// Model override, or the default if omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Response to a [`PromptRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptResponse {
    /// The model's output text.
    pub output: String,
    /// The model that produced the output.
    pub model: String,
    /// Streaming events associated with this response.
    #[serde(default)]
    pub events: Vec<EventFrame>,
}

/// Policy controlling when the agent must ask the user for approval before acting.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AskForApproval {
    /// Ask for approval unless the action is on a trusted path/resource.
    UnlessTrusted,
    /// Only ask after a tool call fails.
    OnFailure,
    /// Ask every time a tool call is requested.
    OnRequest,
    /// Reject the action without asking, with details on which categories are blocked.
    Reject {
        sandbox_approval: bool,
        rules: bool,
        mcp_elicitations: bool,
    },
    /// Never ask; auto-approve all actions.
    Never,
}

/// Classification of tool invocation origin.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// A built-in function tool.
    Function,
    /// An MCP (Model Context Protocol) tool.
    Mcp,
}

/// Parameters for executing a local shell command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalShellParams {
    /// The shell command to execute.
    pub command: String,
    /// Working directory for the command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Timeout in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// The payload of a tool call, discriminated by tool type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolPayload {
    /// A built-in function call with JSON-encoded arguments.
    Function {
        arguments: String,
    },
    /// A custom tool invocation with a free-form input string.
    Custom {
        input: String,
    },
    /// A local shell command execution.
    LocalShell {
        params: LocalShellParams,
    },
    /// An MCP tool invocation targeting a specific server and tool.
    Mcp {
        server: String,
        tool: String,
        raw_arguments: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw_tool_call_id: Option<String>,
    },
}

/// The result of a tool call, discriminated by tool type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOutput {
    /// Result of a built-in function call.
    Function {
        /// The output body, if any.
        #[serde(skip_serializing_if = "Option::is_none")]
        body: Option<Value>,
        /// Whether the call succeeded.
        success: bool,
    },
    /// Result of an MCP tool call.
    Mcp {
        /// The result value returned by the MCP server.
        result: Value,
    },
}

/// Action to take for a network policy rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicyRuleAction {
    /// Allow network access to the host.
    Allow,
    /// Deny network access to the host.
    Deny,
}

/// A proposed amendment to the network access policy for a specific host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkPolicyAmendment {
    /// The host to amend the policy for.
    pub host: String,
    /// The action to apply.
    pub action: NetworkPolicyRuleAction,
}

/// A user's decision on an approval request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReviewDecision {
    /// Approve the action.
    Approved,
    /// Approve and also amend the execution policy.
    ApprovedExecpolicyAmendment,
    /// Approve for the remainder of this session only.
    ApprovedForSession,
    /// Approve with a network policy amendment.
    NetworkPolicyAmendment {
        host: String,
        action: NetworkPolicyRuleAction,
    },
    /// Deny the action.
    Denied,
    /// Abort the entire turn.
    Abort,
}

/// Status of an MCP server during startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpStartupStatus {
    /// The server is in the process of starting.
    Starting,
    /// The server is ready to accept requests.
    Ready,
    /// The server failed to start.
    Failed { error: String },
    /// Startup was cancelled.
    Cancelled,
}

/// A progress update for a single MCP server's startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStartupUpdateEvent {
    /// Name of the MCP server.
    pub server_name: String,
    /// Current startup status.
    pub status: McpStartupStatus,
}

/// Details of an MCP server that failed to start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStartupFailure {
    /// Name of the MCP server that failed.
    pub server_name: String,
    /// Error description.
    pub error: String,
}

/// Summary event emitted once all MCP servers have finished starting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStartupCompleteEvent {
    /// Servers that started successfully.
    pub ready: Vec<String>,
    /// Servers that failed to start.
    pub failed: Vec<McpStartupFailure>,
    /// Servers whose startup was cancelled.
    pub cancelled: Vec<String>,
}

/// Context about a network access request that requires approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkApprovalContext {
    /// The host being accessed.
    pub host: String,
    /// The network protocol (e.g. `"https"`, `"tcp"`).
    pub protocol: String,
}

/// An event requesting user approval for a command execution or patch application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecApprovalRequestEvent {
    /// Identifier of the tool call requesting approval.
    pub call_id: String,
    /// Unique identifier for this approval request.
    pub approval_id: String,
    /// The turn during which the request was made.
    pub turn_id: String,
    /// The command that would be executed.
    pub command: String,
    /// The working directory for the command.
    pub cwd: String,
    /// Human-readable reason why approval is needed.
    pub reason: String,
    /// Network context if the approval involves network access.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_approval_context: Option<NetworkApprovalContext>,
    /// Proposed execution policy rule amendments.
    #[serde(default)]
    pub proposed_execpolicy_amendment: Vec<String>,
    /// Proposed network policy amendments.
    #[serde(default)]
    pub proposed_network_policy_amendments: Vec<NetworkPolicyAmendment>,
    /// Additional permissions being requested.
    #[serde(default)]
    pub additional_permissions: Vec<String>,
    /// The set of decisions the user can choose from.
    #[serde(default)]
    pub available_decisions: Vec<ReviewDecision>,
}

/// The channel a response delta is being written to.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseChannel {
    /// The main visible text output.
    #[default]
    Text,
    /// Internal reasoning / chain-of-thought output.
    Reasoning,
}

impl ResponseChannel {
    /// Returns `true` if this is the `Text` channel.
    pub const fn is_text(&self) -> bool {
        matches!(self, ResponseChannel::Text)
    }
}

/// A user's approval decision sent in response to an approval request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalDecisionRequest {
    /// The decision identifier (e.g. `"approved"`, `"denied"`).
    pub decision: String,
    /// Whether to remember this decision for future similar requests.
    #[serde(default)]
    pub remember: bool,
}

/// A single streaming event frame emitted during agent execution.
///
/// Events are tagged by the `event` field and cover the full lifecycle of a
/// turn: response streaming, tool calls, MCP lifecycle, command execution,
/// patch application, approvals, and errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EventFrame {
    /// A new model response has started.
    ResponseStart {
        response_id: String,
    },
    /// A incremental text delta for an in-progress response.
    ResponseDelta {
        response_id: String,
        delta: String,
        #[serde(default, skip_serializing_if = "ResponseChannel::is_text")]
        channel: ResponseChannel,
    },
    /// The model response has finished.
    ResponseEnd {
        response_id: String,
    },
    /// A tool call has begun.
    ToolCallStart {
        response_id: String,
        tool_name: String,
        arguments: Value,
    },
    /// A tool call has completed and produced a result.
    ToolCallResult {
        response_id: String,
        tool_name: String,
        output: Value,
    },
    /// Progress update for an MCP server starting up.
    McpStartupUpdate {
        update: McpStartupUpdateEvent,
    },
    /// All MCP servers have finished starting.
    McpStartupComplete {
        summary: McpStartupCompleteEvent,
    },
    /// An MCP tool call has begun.
    McpToolCallBegin {
        server_name: String,
        tool_name: String,
    },
    /// An MCP tool call has finished.
    McpToolCallEnd {
        server_name: String,
        tool_name: String,
        ok: bool,
    },
    /// User approval is needed for a command execution.
    ExecApprovalRequest {
        request: ExecApprovalRequestEvent,
    },
    /// User approval is needed for applying a patch.
    ApplyPatchApprovalRequest {
        request: ExecApprovalRequestEvent,
    },
    /// An MCP server is requesting user input (elicitation).
    ElicitationRequest {
        server_name: String,
        request_id: String,
        prompt: String,
    },
    /// A command has started executing.
    ExecCommandBegin {
        command: String,
        cwd: String,
    },
    /// Incremental output from a running command.
    ExecCommandOutputDelta {
        command: String,
        delta: String,
    },
    /// A command has finished executing.
    ExecCommandEnd {
        command: String,
        exit_code: i32,
    },
    /// A patch has started being applied to a file.
    PatchApplyBegin {
        path: String,
    },
    /// A patch has finished being applied.
    PatchApplyEnd {
        path: String,
        ok: bool,
    },
    /// A new turn has started within a thread.
    TurnStarted {
        turn_id: String,
    },
    /// A turn has completed successfully.
    TurnComplete {
        turn_id: String,
    },
    /// A turn was aborted before completion.
    TurnAborted {
        turn_id: String,
        reason: String,
    },
    /// An error occurred during processing.
    Error {
        response_id: String,
        message: String,
    },
}
