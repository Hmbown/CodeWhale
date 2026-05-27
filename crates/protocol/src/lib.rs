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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadResponse {
    pub thread_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<Thread>,
    #[serde(default)]
    pub threads: Vec<Thread>,
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
    #[serde(default)]
    pub events: Vec<EventFrame>,
    #[serde(default)]
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppRequest {
    Capabilities,
    ConfigGet { key: String },
    ConfigSet { key: String, value: String },
    ConfigUnset { key: String },
    ConfigList,
    Models,
    ThreadLoadedList,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppResponse {
    pub ok: bool,
    pub data: Value,
    #[serde(default)]
    pub events: Vec<EventFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptResponse {
    pub output: String,
    pub model: String,
    #[serde(default)]
    pub events: Vec<EventFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AskForApproval {
    UnlessTrusted,
    OnFailure,
    OnRequest,
    Reject {
        sandbox_approval: bool,
        rules: bool,
        mcp_elicitations: bool,
    },
    Never,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Function,
    Mcp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalShellParams {
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolPayload {
    Function {
        arguments: String,
    },
    Custom {
        input: String,
    },
    LocalShell {
        params: LocalShellParams,
    },
    Mcp {
        server: String,
        tool: String,
        raw_arguments: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw_tool_call_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOutput {
    Function {
        #[serde(skip_serializing_if = "Option::is_none")]
        body: Option<Value>,
        success: bool,
    },
    Mcp {
        result: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicyRuleAction {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkPolicyAmendment {
    pub host: String,
    pub action: NetworkPolicyRuleAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReviewDecision {
    Approved,
    ApprovedExecpolicyAmendment,
    ApprovedForSession,
    NetworkPolicyAmendment {
        host: String,
        action: NetworkPolicyRuleAction,
    },
    Denied,
    Abort,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpStartupStatus {
    Starting,
    Ready,
    Failed { error: String },
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStartupUpdateEvent {
    pub server_name: String,
    pub status: McpStartupStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStartupFailure {
    pub server_name: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStartupCompleteEvent {
    pub ready: Vec<String>,
    pub failed: Vec<McpStartupFailure>,
    pub cancelled: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkApprovalContext {
    pub host: String,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecApprovalRequestEvent {
    pub call_id: String,
    pub approval_id: String,
    pub turn_id: String,
    pub command: String,
    pub cwd: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_approval_context: Option<NetworkApprovalContext>,
    #[serde(default)]
    pub proposed_execpolicy_amendment: Vec<String>,
    #[serde(default)]
    pub proposed_network_policy_amendments: Vec<NetworkPolicyAmendment>,
    #[serde(default)]
    pub additional_permissions: Vec<String>,
    #[serde(default)]
    pub available_decisions: Vec<ReviewDecision>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseChannel {
    #[default]
    Text,
    Reasoning,
}

impl ResponseChannel {
    pub const fn is_text(&self) -> bool {
        matches!(self, ResponseChannel::Text)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalDecisionRequest {
    pub decision: String,
    #[serde(default)]
    pub remember: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EventFrame {
    ResponseStart {
        response_id: String,
    },
    ResponseDelta {
        response_id: String,
        delta: String,
        #[serde(default, skip_serializing_if = "ResponseChannel::is_text")]
        channel: ResponseChannel,
    },
    ResponseEnd {
        response_id: String,
    },
    ToolCallStart {
        response_id: String,
        tool_name: String,
        arguments: Value,
    },
    ToolCallResult {
        response_id: String,
        tool_name: String,
        output: Value,
    },
    McpStartupUpdate {
        update: McpStartupUpdateEvent,
    },
    McpStartupComplete {
        summary: McpStartupCompleteEvent,
    },
    McpToolCallBegin {
        server_name: String,
        tool_name: String,
    },
    McpToolCallEnd {
        server_name: String,
        tool_name: String,
        ok: bool,
    },
    ExecApprovalRequest {
        request: ExecApprovalRequestEvent,
    },
    ApplyPatchApprovalRequest {
        request: ExecApprovalRequestEvent,
    },
    ElicitationRequest {
        server_name: String,
        request_id: String,
        prompt: String,
    },
    ExecCommandBegin {
        command: String,
        cwd: String,
    },
    ExecCommandOutputDelta {
        command: String,
        delta: String,
    },
    ExecCommandEnd {
        command: String,
        exit_code: i32,
    },
    PatchApplyBegin {
        path: String,
    },
    PatchApplyEnd {
        path: String,
        ok: bool,
    },
    TurnStarted {
        turn_id: String,
    },
    TurnComplete {
        turn_id: String,
    },
    TurnAborted {
        turn_id: String,
        reason: String,
    },
    Error {
        response_id: String,
        message: String,
    },
}
