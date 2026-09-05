//! Runtime HTTP/SSE API for local Codewhale automation.

use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_stream::stream;
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::header;
use axum::http::{HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware;
use axum::response::Html;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::Utc;
use codewhale_protocol::agent_mail::{
    AgentMailDeliveryMode, AgentMailEnvelope, AgentMailMessageId, AgentMailSendRequest,
    AgentMailSendResponse,
};
use codewhale_protocol::runtime::{
    DynamicToolCallResult, RUNTIME_API_VERSION, RUNTIME_EVENT_ENVELOPE_SCHEMA_VERSION,
    RuntimeCapabilities, RuntimeEventEnvelope, RuntimeExperimentalCapabilities,
};
use codewhale_secrets::account::{
    ACCOUNT_API_BASE_ENV, DEFAULT_ACCOUNT_API_BASE, RuntimeAccountInfo,
};
#[cfg(not(test))]
use codewhale_secrets::account::{AccountSessionStore, secure_account_session_secrets};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;

#[cfg(test)]
use crate::dependencies::ExternalTool;

use crate::automation_manager::{
    AutomationManager, AutomationRecord, AutomationRunRecord, AutomationSchedulerConfig,
    CreateAutomationRequest, SharedAutomationManager, UpdateAutomationRequest, spawn_scheduler,
};
use crate::config::{
    ApiProvider, Config, DEFAULT_TEXT_MODEL, normalize_model_name_for_provider, validate_route,
};
use crate::fleet::executor::{FleetExecutor, configured_codewhale_binary};
use crate::fleet::ledger::{FleetEventReplayError, FleetLedgerState, FleetTaskLedgerStatus};
use crate::fleet::manager::{
    FleetManager, FleetStatusSnapshot, FleetWorkerInspection, FleetWorkerRuntimeProjection,
    ManagedFleetRunDescriptor,
};
use crate::fleet::profile::canonical_public_role_name;
use crate::fleet::task_spec::FleetTaskSpecDocument;
use crate::fleet::worker_runtime::fleet_write_roots;
use crate::mcp::McpPool;
#[cfg(test)]
pub(super) use crate::models::{ContentBlock, Message};
use crate::runtime_threads::{
    CompactThreadRequest, CreateThreadRequest, ExternalApprovalDecision,
    MAX_RUNTIME_EVENT_REPLAY_TAIL, RuntimeThreadManager, RuntimeThreadManagerConfig,
    SharedRuntimeThreadManager, StartTurnRequest, SteerTurnRequest, ThreadDetail, ThreadListFilter,
    ThreadRecord, TurnItemKind, TurnRecord, UpdateThreadRequest, UsageGroupBy, UsageTotals,
};
#[cfg(test)]
pub(super) use crate::runtime_threads::{RuntimeTurnStatus, TurnItemLifecycleStatus};
use crate::session_manager::default_sessions_dir;
#[cfg(test)]
pub(super) use crate::session_manager::{SavedSession, SessionMetadata};
use crate::skill_state::SkillStateStore;
use crate::task_manager::{
    NewTaskRequest, SharedTaskManager, TaskManager, TaskManagerConfig, TaskRecord, TaskSummary,
};
use crate::tools::subagent::{
    AgentWorkerRecord, SharedSubAgentManager, load_persisted_agent_worker_records,
    new_shared_subagent_manager_with_timeout,
};
use codewhale_protocol::fleet::{
    FleetArtifactKind, FleetEventReplay, FleetRun, FleetRunId, FleetRuntimeEvent,
    FleetRuntimeTarget, FleetSecurityPolicy, FleetTaskSpec, FleetWorkerEventPayload,
    FleetWorkerSpec, FleetWorkerStatus, FleetWorkflowDescriptor, FleetWorkflowKind,
};

mod auth;
mod mobile;
mod plugins;
mod sessions;
mod web;
mod workspace;
#[cfg(test)]
use self::auth::ResolvedRuntimeAuth;
use self::auth::{
    require_runtime_token, resolve_runtime_auth, runtime_auth_status_lines,
    runtime_request_is_authorized,
};
use self::sessions::{
    create_session_from_thread, delete_session, get_session, list_sessions, list_sessions_summary,
    patch_session, resume_session_thread, save_current_session,
};
#[cfg(test)]
use self::sessions::{messages_from_thread_detail, session_to_detail};
#[cfg(test)]
use self::workspace::collect_workspace_status;
use self::workspace::{collect_workspace_git_metadata, workspace_status};

const RUNTIME_TOKEN_ENV: &str = "CODEWHALE_RUNTIME_TOKEN";
const LEGACY_RUNTIME_TOKEN_ENV: &str = "DEEPSEEK_RUNTIME_TOKEN";
const LEGACY_RUNTIME_TOKEN_WARNING: &str = "Warning: DEEPSEEK_RUNTIME_TOKEN is deprecated; use \
CODEWHALE_RUNTIME_TOKEN (the legacy alias is removed in 0.10.0).";

struct RuntimeTokenEnvironment {
    token: Option<String>,
    legacy_alias_used: bool,
}

fn runtime_token_environment(lookup: &dyn Fn(&str) -> Option<String>) -> RuntimeTokenEnvironment {
    let nonblank = |name| {
        lookup(name)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };

    if let Some(token) = nonblank(RUNTIME_TOKEN_ENV) {
        return RuntimeTokenEnvironment {
            token: Some(token),
            legacy_alias_used: false,
        };
    }

    let token = nonblank(LEGACY_RUNTIME_TOKEN_ENV);
    RuntimeTokenEnvironment {
        legacy_alias_used: token.is_some(),
        token,
    }
}

fn runtime_token_alias_warning(
    cli_token: Option<&str>,
    environment: &RuntimeTokenEnvironment,
) -> Option<&'static str> {
    let cli_token_is_used = cli_token.is_some_and(|token| !token.trim().is_empty());
    (!cli_token_is_used && environment.legacy_alias_used).then_some(LEGACY_RUNTIME_TOKEN_WARNING)
}

#[derive(Clone)]
pub struct RuntimeApiState {
    config: Arc<parking_lot::RwLock<Config>>,
    workspace: PathBuf,
    plugin_discovery: Arc<crate::plugins::PluginDiscoveryContext>,
    task_manager: SharedTaskManager,
    runtime_threads: SharedRuntimeThreadManager,
    cors_origins: Vec<String>,
    sessions_dir: PathBuf,
    /// Original `--config` path (if any) used to load the initial config.
    /// Passed to `Config::load` on reload and to persistence helpers so
    /// GUI-driven config changes target the same file the server was
    /// started with, instead of falling back to the default discovery.
    config_path: Option<PathBuf>,
    /// Effective initial profile (`--profile` or `DEEPSEEK_PROFILE`).
    /// Reload must retain this overlay so profile-scoped routes do not vanish.
    config_profile: Option<String>,
    automations: SharedAutomationManager,
    sub_agent_manager: SharedSubAgentManager,
    runtime_token: Option<String>,
    skill_state: Arc<Mutex<SkillStateStore>>,
    auth_required: bool,
    bind_host: String,
    bind_port: u16,
    mobile_enabled: bool,
    mobile: Option<mobile::RuntimeMobileState>,
    web: Option<web::RuntimeWebState>,
    /// Executable used by Runtime API-owned Fleet manager loops. Stored on
    /// state so tests and embedded callers can provide a hermetic worker.
    fleet_codewhale_binary: String,
    /// Shared McpPool reused for explicit live MCP discovery. Passive API
    /// calls do not initialize this pool so dashboards cannot accidentally
    /// become a second stdio-process owner. The outer mutex guards only the
    /// lazily-initialized slot; slow per-pool work (connect_all) runs under
    /// the inner handle so it cannot block slot reads.
    mcp_pool: Arc<Mutex<Option<Arc<Mutex<McpPool>>>>>,
    #[cfg(test)]
    compat_stream_test_hook: Option<tokio::sync::mpsc::UnboundedSender<CompatStreamTestPoint>>,
}

#[cfg(test)]
enum CompatStreamTestPoint {
    ThreadCreated {
        thread_id: String,
        resume: tokio::sync::oneshot::Sender<()>,
    },
    SubscribedBeforeReplay {
        thread_id: String,
        turn_id: String,
        resume: tokio::sync::oneshot::Sender<()>,
    },
    ReplayLoaded {
        thread_id: String,
        turn_id: String,
        resume: tokio::sync::oneshot::Sender<()>,
    },
}

#[derive(Debug, Clone)]
pub struct RuntimeApiOptions {
    pub host: String,
    pub port: u16,
    pub workers: usize,
    /// Additional CORS origins to allow on top of the built-in defaults
    /// (`http://localhost:{3000,1420}`, `http://127.0.0.1:{3000,1420}`,
    /// `tauri://localhost`). Populated by `--cors-origin` (repeatable),
    /// `CODEWHALE_CORS_ORIGINS` (comma-separated, `DEEPSEEK_CORS_ORIGINS`
    /// as alias), and `[runtime_api] cors_origins` in `config.toml`.
    /// Whalescale#255 / #561.
    pub cors_origins: Vec<String>,
    /// Optional bearer token required for `/v1/*` routes. If omitted here,
    /// `run_http_server` checks `CODEWHALE_RUNTIME_TOKEN`, then
    /// `DEEPSEEK_RUNTIME_TOKEN` as an alias.
    pub auth_token: Option<String>,
    /// Allow `/v1/*` routes without auth when no token is configured.
    pub insecure_no_auth: bool,
    /// Enables the built-in mobile control page at `/mobile`.
    pub mobile: bool,
    /// Enables the embedded local browser client and opens it after binding.
    /// Web mode is always loopback-only and uses a one-time bootstrap cookie
    /// exchange rather than exposing the Runtime token to the browser URL.
    pub web: bool,
    /// Show a QR code for the mobile URL in the terminal.
    pub show_qr: bool,
    /// Original `--config` path used to load the initial config. When
    /// `Some`, GUI-driven config reloads and persistence target this file
    /// instead of the default discovery path.
    pub config_path: Option<PathBuf>,
    /// Effective profile used to load the server's initial Config.
    pub config_profile: Option<String>,
}

impl Default for RuntimeApiOptions {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 7878,
            workers: 2,
            cors_origins: Vec::new(),
            auth_token: None,
            insecure_no_auth: false,
            mobile: false,
            web: false,
            show_qr: false,
            config_path: None,
            config_profile: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct StreamTurnRequest {
    prompt: String,
    model: Option<String>,
    mode: Option<String>,
    permission_posture: Option<String>,
    workspace: Option<PathBuf>,
    allow_shell: Option<bool>,
    trust_mode: Option<bool>,
    auto_approve: Option<bool>,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
    mode: &'static str,
}

#[derive(Debug, Serialize)]
struct TasksResponse {
    tasks: Vec<TaskSummary>,
    counts: crate::task_manager::TaskCounts,
}

#[derive(Debug, Deserialize)]
struct TasksQuery {
    limit: Option<usize>,
    workspace: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct ThreadsQuery {
    limit: Option<usize>,
    include_archived: Option<bool>,
    /// When `true`, returns archived threads only (overrides `include_archived`).
    /// Whalescale#260 / #563.
    archived_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ThreadSummaryQuery {
    limit: Option<usize>,
    search: Option<String>,
    include_archived: Option<bool>,
    /// When `true`, returns archived threads only (overrides `include_archived`).
    /// Whalescale#260 / #563.
    archived_only: Option<bool>,
}

fn resolve_thread_filter(
    include_archived: Option<bool>,
    archived_only: Option<bool>,
) -> ThreadListFilter {
    if archived_only.unwrap_or(false) {
        ThreadListFilter::ArchivedOnly
    } else if include_archived.unwrap_or(false) {
        ThreadListFilter::IncludeArchived
    } else {
        ThreadListFilter::ActiveOnly
    }
}

#[derive(Debug, Serialize)]
struct ThreadSummary {
    id: String,
    title: String,
    preview: String,
    model: String,
    mode: String,
    workspace: PathBuf,
    branch: Option<String>,
    head: Option<String>,
    dirty: bool,
    archived: bool,
    updated_at: chrono::DateTime<Utc>,
    latest_turn_id: Option<String>,
    latest_turn_status: Option<String>,
    /// Pending approvals plus pending user-input requests in the canonical
    /// thread snapshot. Clients use this typed fact for attention grouping;
    /// lifecycle prose and turn-status strings are not an authority signal.
    pending_attention_count: usize,
}

#[derive(Debug, Serialize)]
struct SkillEntry {
    name: String,
    description: String,
    /// Native Skill locator. Reviewed plugin paths are deliberately omitted;
    /// their bodies are available only through the authority-bound snapshot.
    path: Option<PathBuf>,
    source: String,
    plugin_id: Option<String>,
    plugin_generation: Option<u64>,
    plugin_content_hash: Option<String>,
    enabled: bool,
    is_bundled: bool,
}

#[derive(Debug, Serialize)]
struct SkillsResponse {
    directory: PathBuf,
    directories: Vec<PathBuf>,
    warnings: Vec<String>,
    skills: Vec<SkillEntry>,
}

#[derive(Debug, Serialize)]
struct AgentRunsResponse {
    runs: Vec<AgentWorkerRecord>,
}

#[derive(Debug, Deserialize)]
struct SetSkillEnabledRequest {
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct SetSkillEnabledResponse {
    name: String,
    enabled: bool,
}

// ─── Skill lifecycle request/response types ────────────────────────────────

#[derive(Debug, Deserialize)]
struct InstallSkillRequest {
    /// Remote source spec: `github:owner/repo`, `https://…`, or a registry name.
    source: String,
    /// `"project"` or `"global"` (default: `"global"`).
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateSkillRequest {
    /// `"project"`, `"global"`, or `null` (auto-detect).
    #[serde(default)]
    scope: Option<String>,
    /// Digest the caller observed before requesting the update. The mutation
    /// will fail if the on-disk digest has changed since.
    #[serde(default)]
    expected_digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UninstallSkillQuery {
    /// `"project"`, `"global"`, or `null` (auto-detect).
    #[serde(default)]
    scope: Option<String>,
    /// Digest the caller observed. The mutation will fail if it has drifted.
    #[serde(default)]
    expected_digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TrustSkillRequest {
    /// `"project"`, `"global"`, or `null` (auto-detect).
    #[serde(default)]
    scope: Option<String>,
    /// Digest the caller reviewed. The mutation will fail if it has drifted.
    #[serde(default)]
    expected_digest: Option<String>,
}

/// Scope query parameter used by the audit endpoint.
#[derive(Debug, Deserialize, Default)]
struct SkillScopeQuery {
    /// `"project"` or `"global"` to restrict to one root.
    scope: Option<String>,
}

#[derive(Debug, Serialize)]
struct SkillMutationReceiptResponse {
    /// Skill name as recorded by the mutation.
    name: String,
    /// Human-readable action performed: `"installed"`, `"updated"`, `"removed"`,
    /// `"trusted"`, `"no_change"`, etc.
    outcome: &'static str,
    /// Resolved install scope: `"project"` or `"global"`.
    scope: String,
    /// Display path of the skill package (may be redacted for plugin snapshots).
    safe_target_path: String,
    /// Trust advisory note, present only for `"trusted"` outcomes.
    #[serde(skip_serializing_if = "Option::is_none")]
    trust_note: Option<&'static str>,
}

/// Read-only audit receipt for a single installed skill.
#[derive(Debug, Serialize)]
struct SkillAuditEntry {
    name: String,
    safe_display_path: String,
    source_kind: String,
    scope: String,
    digest: SkillAuditDigest,
    trust: String,
    integrity: String,
    available_actions: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SkillAuditDigest {
    state: String,
    /// Hex digest value; absent when the digest is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
}

#[derive(Debug, Serialize)]
struct SkillAuditResponse {
    /// `true` when multiple owned copies with the same name exist. The
    /// caller should re-request with an explicit `scope` parameter.
    ambiguous: bool,
    skills: Vec<SkillAuditEntry>,
}

#[derive(Debug, Deserialize)]
struct DecideApprovalBody {
    decision: String,
    #[serde(default)]
    remember: bool,
}

#[derive(Debug, Serialize)]
struct DecideApprovalResponse {
    ok: bool,
    approval_id: String,
    decision: String,
    delivered: bool,
}

#[derive(Debug, Deserialize)]
struct SubmitUserInputBody {
    answers: Vec<UserInputAnswerBody>,
}

#[derive(Debug, Deserialize)]
struct UserInputAnswerBody {
    id: String,
    label: String,
    value: String,
}

#[derive(Debug, Serialize)]
struct SubmitUserInputResponse {
    ok: bool,
    input_id: String,
    delivered: bool,
}

#[derive(Debug, Serialize)]
struct RuntimeInfoResponse {
    service: &'static str,
    runtime_api_version: &'static str,
    codewhale_version: &'static str,
    /// Full 40-character source commit embedded by the shared build script.
    /// Desktop compatibility intentionally rejects `unknown` and abbreviated
    /// values, so source archives without build provenance fail closed.
    codewhale_commit: &'static str,
    bind_host: String,
    port: u16,
    auth_required: bool,
    transports: Vec<&'static str>,
    capabilities: RuntimeCapabilities,
    account: RuntimeAccountInfo,
    experimental: RuntimeExperimentalCapabilities,
    // Backward-compatible alias kept for existing clients.
    version: &'static str,
}

fn default_runtime_capabilities() -> RuntimeCapabilities {
    RuntimeCapabilities {
        account_session: true,
        threads: true,
        turns: true,
        turn_operation_idempotency: true,
        turn_steer: true,
        turn_interrupt: true,
        event_replay: true,
        external_tools: true,
        environments: false,
        worker_runtime: true,
        fleet_run_create: true,
        fleet_run_start: true,
        fleet_event_replay: true,
        fleet_event_stream: true,
        fleet_local_target: true,
        thread_goals: true,
        memory: true,
        mcp_server_management: true,
        skill_lifecycle: true,
        plugin_management: true,
        agent_mail: true,
    }
}

fn runtime_api_sub_agent_manager(workspace: &FsPath, workers: usize) -> SharedSubAgentManager {
    let max_agents = workers.max(1);
    new_shared_subagent_manager_with_timeout(
        workspace.to_path_buf(),
        max_agents,
        max_agents,
        Duration::from_secs(crate::config::DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS),
        max_agents,
        None,
    )
}

#[derive(Debug, Serialize)]
struct McpServerEntry {
    name: String,
    enabled: bool,
    required: bool,
    command: Option<String>,
    url: Option<String>,
    connected: bool,
    enabled_tools: Vec<String>,
    disabled_tools: Vec<String>,
}

#[derive(Debug, Serialize)]
struct McpServersResponse {
    servers: Vec<McpServerEntry>,
}

#[derive(Debug, Deserialize)]
struct McpToolsQuery {
    server: Option<String>,
    #[serde(default)]
    connect: bool,
}

#[derive(Debug, Serialize)]
struct McpToolEntry {
    server: String,
    name: String,
    prefixed_name: String,
    description: Option<String>,
    input_schema: Value,
}

#[derive(Debug, Serialize)]
struct McpToolsResponse {
    tools: Vec<McpToolEntry>,
}

/// Request body for `POST /v1/apps/mcp/servers` (create) and
/// `PATCH /v1/apps/mcp/servers/{name}` (update).
///
/// Either `command` **or** `url` must be set on create. On update, only
/// supplied fields are applied; absent fields leave the existing value in
/// place.
#[derive(Debug, Deserialize)]
struct McpServerWriteRequest {
    /// stdio command binary (e.g. `"npx"`).
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    command: Option<Option<String>>,
    /// Arguments for the stdio command.
    args: Option<Vec<String>>,
    /// Environment variables injected into the stdio child process.
    /// Values are stored as-is; use `${VAR}` syntax to reference environment
    /// variables at runtime instead of embedding secrets here.
    env: Option<std::collections::HashMap<String, String>>,
    /// HTTP(S) endpoint for streamable-HTTP or SSE MCP servers.
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    url: Option<Option<String>>,
    /// Explicit transport override (`"sse"` or `"streamable_http"`).
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    transport: Option<Option<String>>,
    /// Override the server-level connect timeout in seconds.
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    connect_timeout: Option<Option<u64>>,
    /// Override the server-level execute timeout in seconds.
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    execute_timeout: Option<Option<u64>>,
    /// Override the server-level read timeout in seconds.
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    read_timeout: Option<Option<u64>>,
    /// Whether the server is enabled. Defaults to `true` on create.
    enabled: Option<bool>,
    /// Whether a connection failure for this server is fatal.
    required: Option<bool>,
    /// Allowlist of tool names to expose (empty = expose all).
    enabled_tools: Option<Vec<String>>,
    /// Denylist of tool names to hide.
    disabled_tools: Option<Vec<String>>,
    /// Variable names whose runtime values are injected as HTTP headers.
    /// The key in this map is the HTTP header name; the value is the
    /// environment variable whose value supplies the header value at
    /// request time. Credentials remain in the environment, not on disk.
    env_headers: Option<std::collections::HashMap<String, String>>,
    /// Environment variable that contains a bearer token for URL-based servers.
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    bearer_token_env_var: Option<Option<String>>,
    /// OAuth scopes requested during `codewhale mcp login`.
    scopes: Option<Vec<String>>,
    /// RFC 8707 resource parameter for the OAuth authorization URL.
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    oauth_resource: Option<Option<String>>,
}

/// Preserve the difference between an omitted PATCH field and an explicit
/// `null`: serde only calls this decoder when the field is present.
fn deserialize_present_nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// Response returned by MCP server management endpoints.
///
/// Sensitive fields (`headers`, `env_headers`, `bearer_token_env_var`,
/// `env`, OAuth client secrets) are intentionally omitted or redacted so
/// the API never echoes credentials back to callers.
#[derive(Debug, Serialize)]
struct McpServerDetail {
    name: String,
    enabled: bool,
    required: bool,
    command: Option<String>,
    args: Vec<String>,
    /// Environment variable names injected into the process.
    /// Values are **not** returned — callers see only the keys.
    env_keys: Vec<String>,
    url: Option<String>,
    transport: Option<String>,
    connect_timeout: Option<u64>,
    execute_timeout: Option<u64>,
    read_timeout: Option<u64>,
    enabled_tools: Vec<String>,
    disabled_tools: Vec<String>,
    /// HTTP header names that are read from environment variables.
    /// The corresponding environment variable values are **not** returned.
    env_header_keys: Vec<String>,
    /// Whether a `bearer_token_env_var` is configured (value not returned).
    has_bearer_token_env_var: bool,
    scopes: Vec<String>,
    oauth_resource: Option<String>,
    /// Live connection state from the in-memory pool (if the pool is active).
    connected: bool,
}

impl McpServerDetail {
    fn from_config(name: &str, cfg: &crate::mcp::McpServerConfig, connected: bool) -> Self {
        let mut env_keys: Vec<String> = cfg.env.keys().cloned().collect();
        env_keys.sort();
        let mut env_header_keys: Vec<String> = cfg.env_headers.keys().cloned().collect();
        env_header_keys.sort();
        Self {
            name: name.to_string(),
            enabled: cfg.is_enabled(),
            required: cfg.required,
            command: cfg.command.clone(),
            args: cfg.args.clone(),
            env_keys,
            url: cfg.url.clone(),
            transport: cfg.transport.clone(),
            connect_timeout: cfg.connect_timeout,
            execute_timeout: cfg.execute_timeout,
            read_timeout: cfg.read_timeout,
            enabled_tools: cfg.enabled_tools.clone(),
            disabled_tools: cfg.disabled_tools.clone(),
            env_header_keys,
            has_bearer_token_env_var: cfg.bearer_token_env_var.is_some(),
            scopes: cfg.scopes.clone(),
            oauth_resource: cfg.oauth_resource.clone(),
            connected,
        }
    }
}

#[derive(Debug, Serialize)]
struct McpServerActionReceipt {
    name: String,
    action: &'static str,
    ok: bool,
}

#[derive(Debug, Deserialize)]
struct AutomationRunsQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ThreadEventsQuery {
    since_seq: Option<u64>,
    replay_limit: Option<usize>,
}

const DEFAULT_FLEET_EVENT_REPLAY_LIMIT: usize = 250;
const MAX_FLEET_EVENT_REPLAY_LIMIT: usize = 1_000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateFleetRunRequest {
    #[serde(default)]
    name: Option<String>,
    target: FleetRuntimeTarget,
    roles: Vec<ManagedFleetRoleRequest>,
    workflow: ManagedFleetWorkflowRequest,
    #[serde(default, alias = "workers")]
    worker_specs: Vec<FleetWorkerSpec>,
    #[serde(default)]
    labels: BTreeMap<String, String>,
    #[serde(default)]
    security_policy: Option<FleetSecurityPolicy>,
    #[serde(default)]
    max_workers: Option<usize>,
    /// Optional run-wide usage ceiling (R6, #5567).
    #[serde(default)]
    usage_ceiling: Option<codewhale_protocol::fleet::FleetUsageCeiling>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedFleetRoleRequest {
    name: String,
    #[serde(default)]
    agent_profile: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedFleetWorkflowRequest {
    id: String,
    kind: FleetWorkflowKind,
    #[serde(alias = "task_specs")]
    tasks: Vec<FleetTaskSpec>,
}

#[derive(Debug, Deserialize)]
struct FleetEventsQuery {
    after: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct StartTurnResponse {
    thread: ThreadRecord,
    turn: TurnRecord,
}

fn install_runtime_server_workshop_budgets(
    config: &Config,
) -> crate::tools::large_output_router::WorkshopConfig {
    crate::tools::large_output_router::WorkshopConfig::install_active(config.workshop.as_ref())
}

fn open_runtime_threads_for_server(
    config: &Config,
    workspace: PathBuf,
    manager_config: RuntimeThreadManagerConfig,
    plugin_registry: Arc<crate::plugins::PluginRegistry>,
) -> Result<(
    SharedRuntimeThreadManager,
    crate::tools::large_output_router::WorkshopConfig,
)> {
    // The Runtime API lazily creates engines after the HTTP/Web server starts.
    // Install the resolved process-wide read/tool byte limits before the
    // thread manager can spawn any of those engines, matching interactive and
    // headless exec startup.
    let workshop_activation = install_runtime_server_workshop_budgets(config);
    let manager = Arc::new(RuntimeThreadManager::open_with_plugin_registry(
        config.clone(),
        workspace,
        manager_config,
        plugin_registry,
    )?);
    Ok((manager, workshop_activation))
}

/// Start the runtime API server.
pub async fn run_http_server(
    config: Config,
    workspace: PathBuf,
    plugin_discovery: Arc<crate::plugins::PluginDiscoveryContext>,
    options: RuntimeApiOptions,
) -> Result<()> {
    validate_runtime_listener_security(&options)?;

    let task_default_model = config.default_model();
    let task_cfg = TaskManagerConfig::from_runtime(
        &config,
        workspace.clone(),
        Some(task_default_model),
        Some(options.workers),
    );
    let (runtime_threads, _workshop_activation) = open_runtime_threads_for_server(
        &config,
        workspace.clone(),
        RuntimeThreadManagerConfig::from_task_data_dir(task_cfg.data_dir.clone()),
        plugin_discovery.registry_for_workspace(&workspace),
    )?;
    let task_manager =
        TaskManager::start_with_runtime_manager(task_cfg, config.clone(), runtime_threads.clone())
            .await?;
    let automations = Arc::new(Mutex::new(AutomationManager::default_location()?));
    runtime_threads.attach_automation_manager(automations.clone());
    let scheduler_cancel = CancellationToken::new();
    let scheduler_handle = spawn_scheduler(
        automations.clone(),
        task_manager.clone(),
        scheduler_cancel.clone(),
        AutomationSchedulerConfig::default(),
    );

    let sessions_dir = default_sessions_dir().unwrap_or_else(|_| fallback_sessions_dir());
    let runtime_token_env = runtime_token_environment(&|name| std::env::var(name).ok());
    let runtime_token_alias_warning =
        runtime_token_alias_warning(options.auth_token.as_deref(), &runtime_token_env);
    let resolved_auth = resolve_runtime_auth(
        options.auth_token.clone(),
        runtime_token_env.token,
        options.insecure_no_auth,
    );
    let runtime_token = resolved_auth.token.clone();
    let auth_enabled = runtime_token.is_some();
    let (web, web_bootstrap) = if options.web {
        runtime_token
            .as_ref()
            .context("Codewhale web requires a Runtime authentication token")?;
        let (web, bootstrap) = web::RuntimeWebState::new();
        (Some(web), Some(bootstrap))
    } else {
        (None, None)
    };
    let (mobile, mobile_bootstrap) = if options.mobile && auth_enabled {
        let (mobile, bootstrap) = mobile::RuntimeMobileState::new();
        (Some(mobile), Some(bootstrap))
    } else {
        (None, None)
    };
    let skill_state = SkillStateStore::load_default()
        .context("load persistent Skill activation state for Runtime API")?;
    let sub_agent_manager = runtime_api_sub_agent_manager(&workspace, options.workers);
    let state = RuntimeApiState {
        config: Arc::new(parking_lot::RwLock::new(config.clone())),
        workspace,
        plugin_discovery,
        task_manager,
        runtime_threads,
        cors_origins: options.cors_origins.clone(),
        sessions_dir,
        config_path: options.config_path.clone(),
        config_profile: options.config_profile.clone(),
        automations,
        sub_agent_manager,
        runtime_token: runtime_token.clone(),
        skill_state: Arc::new(Mutex::new(skill_state)),
        auth_required: auth_enabled,
        bind_host: options.host.clone(),
        bind_port: options.port,
        mobile_enabled: options.mobile,
        mobile,
        web,
        fleet_codewhale_binary: configured_codewhale_binary(),
        mcp_pool: Arc::new(Mutex::new(None)),
        #[cfg(test)]
        compat_stream_test_hook: None,
    };
    let app = build_router(state);

    let addr = runtime_bind_address(&options.host, options.port)?;
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("Failed to bind {addr}"))?;

    let bound_addr = listener
        .local_addr()
        .context("Failed to read Runtime API listener address")?;
    println!("Runtime API listening on http://{bound_addr}");
    for line in runtime_auth_status_lines(&resolved_auth) {
        println!("{line}");
    }
    if let Some(warning) = runtime_token_alias_warning {
        println!("{warning}");
    }
    if options.mobile {
        print_mobile_urls(
            bound_addr,
            auth_enabled,
            resolved_auth.generated,
            options.show_qr,
            mobile_bootstrap.as_deref(),
        );
    }
    if let Some(bootstrap) = web_bootstrap {
        println!("Codewhale web enabled at http://{bound_addr}/");
        let bootstrap_url = web::bootstrap_url(bound_addr, &bootstrap);
        println!(
            "Codewhale web bootstrap (single-use, expires in {} min): {bootstrap_url}",
            web::BOOTSTRAP_TTL.as_secs() / 60
        );
        if let Some(warning) = web_launcher_warning(crate::utils::open_url(&bootstrap_url)) {
            println!("{warning}");
        }
    }
    let is_loopback = is_loopback_bind_host(&options.host);
    if is_loopback {
        println!("Security: this server is local-first. Do not expose it to untrusted networks.");
    } else {
        println!(
            "Security: bound to {host}; reachable from any peer that can route to this address.",
            host = options.host
        );
        if !auth_enabled {
            println!(
                "  WARNING: auth is disabled. Anyone on the network can call /v1/* without authentication."
            );
        }
        println!(
            "  /v1/runtime/info reports bind_host={host:?}, port={port}, auth_required={auth}.",
            host = options.host,
            port = options.port,
            auth = auth_enabled,
        );
    }
    let serve_result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .map_err(|e| anyhow!("Runtime API server error: {e}"));
    scheduler_cancel.cancel();
    scheduler_handle.abort();
    serve_result
}

/// Mobile control uses plain HTTP only on loopback. It has no TLS or verified
/// overlay transport, so a non-loopback listener would expose the Runtime API
/// to peers that can observe or replay browser traffic.
fn validate_runtime_listener_security(options: &RuntimeApiOptions) -> Result<()> {
    if options.port == 0 {
        bail!("Port must be > 0");
    }
    if options.web && options.host != "127.0.0.1" {
        bail!("Codewhale web is loopback-only and must bind to 127.0.0.1");
    }
    if options.web && options.insecure_no_auth {
        bail!("Codewhale web requires Runtime authentication; remove --insecure");
    }
    if options.mobile && !is_loopback_bind_host(&options.host) {
        bail!(
            "Codewhale mobile is loopback-only without TLS or a verified overlay; bind to 127.0.0.1 or ::1"
        );
    }
    Ok(())
}

fn is_loopback_bind_host(host: &str) -> bool {
    host.parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

fn runtime_bind_address(host: &str, port: u16) -> Result<SocketAddr> {
    let address = match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(_)) => format!("[{host}]:{port}"),
        _ => format!("{host}:{port}"),
    };
    address
        .parse()
        .with_context(|| format!("Invalid bind address '{host}:{port}'"))
}

fn web_launcher_warning(result: Result<()>) -> Option<String> {
    result.err().map(|error| {
        format!(
            "warning: could not open the default browser ({error}); open the bootstrap URL above manually"
        )
    })
}

fn fallback_sessions_dir() -> PathBuf {
    if let Some(home) = codewhale_paths::codewhale_home_override().ok().flatten() {
        return home.join("sessions");
    }
    codewhale_paths::legacy_deepseek_home()
        .unwrap_or_else(|| PathBuf::from(codewhale_paths::LEGACY_APP_DIR))
        .join("sessions")
}

pub fn build_router(state: RuntimeApiState) -> Router {
    let api_routes = Router::new()
        .route(
            "/v1/sessions",
            get(list_sessions)
                .post(create_session_from_thread)
                .put(save_current_session),
        )
        .route("/v1/sessions/summary", get(list_sessions_summary))
        .route(
            "/v1/sessions/{id}",
            get(get_session).patch(patch_session).delete(delete_session),
        )
        .route(
            "/v1/sessions/{id}/resume-thread",
            post(resume_session_thread),
        )
        .route("/v1/workspace/status", get(workspace_status))
        .route("/v1/agent-runs", get(list_agent_runs))
        .route("/v1/agent-runs/{run_id}", get(get_agent_run))
        .route("/v1/fleet/profiles", get(list_fleet_profiles))
        .route(
            "/v1/fleet/runs",
            get(list_fleet_runs).post(create_fleet_run),
        )
        .route("/v1/fleet/runs/{run_id}", get(get_fleet_run))
        .route(
            "/v1/fleet/runs/{run_id}/workers",
            get(list_fleet_run_workers),
        )
        .route("/v1/fleet/runs/{run_id}/start", post(start_fleet_run))
        .route("/v1/fleet/runs/{run_id}/events", get(stream_fleet_events))
        .route(
            "/v1/fleet/runs/{run_id}/events/replay",
            get(replay_fleet_events),
        )
        .route("/v1/fleet/runs/{run_id}/stop", post(stop_fleet_run))
        .route(
            "/v1/fleet/runs/{run_id}/receipts",
            get(list_fleet_run_receipts),
        )
        .route(
            "/v1/fleet/runs/{run_id}/receipts/{task_id}",
            get(get_fleet_run_receipt),
        )
        .route(
            "/v1/fleet/runs/{run_id}/receipts/{task_id}/evidence",
            get(inspect_fleet_run_receipt_evidence),
        )
        .route("/v1/fleet/workers/{worker_id}", get(get_fleet_worker))
        .route(
            "/v1/fleet/workers/{worker_id}/interrupt",
            post(interrupt_fleet_worker),
        )
        .route(
            "/v1/fleet/workers/{worker_id}/stop",
            post(stop_fleet_worker),
        )
        .route(
            "/v1/fleet/workers/{worker_id}/restart",
            post(restart_fleet_worker),
        )
        .route("/v1/stream", post(stream_turn))
        .route("/v1/threads", get(list_threads).post(create_thread))
        .route("/v1/threads/summary", get(list_threads_summary))
        .route("/v1/threads/{id}", get(get_thread).patch(update_thread))
        .route("/v1/threads/{id}/resume", post(resume_thread))
        .route("/v1/threads/{id}/fork", post(fork_thread))
        .route("/v1/threads/{id}/undo", post(undo_thread_turn))
        .route("/v1/threads/{id}/patch-undo", post(patch_undo_thread_turn))
        .route("/v1/threads/{id}/retry", post(retry_thread_turn))
        .route("/v1/threads/{id}/turns", post(start_thread_turn))
        .route(
            "/v1/threads/{id}/turns/{turn_id}/steer",
            post(steer_thread_turn),
        )
        .route(
            "/v1/threads/{id}/turns/{turn_id}/interrupt",
            post(interrupt_thread_turn),
        )
        .route(
            "/v1/threads/{id}/turns/{turn_id}/tool-calls/{call_id}/result",
            post(deliver_dynamic_tool_result),
        )
        .route("/v1/threads/{id}/compact", post(compact_thread))
        .route("/v1/threads/{id}/usage", get(get_thread_usage))
        .route("/v1/threads/{id}/events", get(stream_thread_events))
        .route("/v1/agent-mail", post(send_agent_mail))
        .route("/v1/threads/{id}/agent-mail", get(list_agent_mail))
        .route(
            "/v1/threads/{id}/agent-mail/{message_id}/deliver",
            post(deliver_agent_mail),
        )
        .route(
            "/v1/threads/{id}/agent-mail/{message_id}/read",
            post(mark_agent_mail_read),
        )
        .route(
            "/v1/threads/{id}/goal",
            get(get_thread_goal)
                .put(upsert_thread_goal)
                .delete(delete_thread_goal),
        )
        .route("/v1/threads/{id}/goal/complete", post(complete_thread_goal))
        .route("/v1/threads/{id}/goal/block", post(block_thread_goal))
        .route("/v1/approvals/{approval_id}", post(decide_approval))
        .route(
            "/v1/user-input/{thread_id}/{input_id}",
            post(submit_user_input),
        )
        .route("/v1/tasks", get(list_tasks).post(create_task))
        .route("/v1/tasks/{id}", get(get_task))
        .route("/v1/tasks/{id}/cancel", post(cancel_task))
        .route("/v1/skills", get(list_skills))
        .route(
            "/v1/skills/{name}",
            post(set_skill_enabled).delete(uninstall_skill_api),
        )
        .route(
            "/v1/apps/mcp/servers",
            get(list_mcp_servers).post(create_mcp_server),
        )
        .route(
            "/v1/apps/mcp/servers/{name}",
            get(get_mcp_server)
                .patch(update_mcp_server)
                .delete(delete_mcp_server),
        )
        .route(
            "/v1/apps/mcp/servers/{name}/enable",
            post(enable_mcp_server),
        )
        .route(
            "/v1/apps/mcp/servers/{name}/disable",
            post(disable_mcp_server),
        )
        .route(
            "/v1/apps/mcp/servers/{name}/reconnect",
            post(reconnect_mcp_server),
        )
        .route("/v1/skills/install", post(install_skill_api))
        .route("/v1/skills/{name}/update", post(update_skill_api))
        .route("/v1/skills/{name}/trust", post(trust_skill_api))
        .route("/v1/skills/{name}/audit", get(audit_skill_api))
        .route("/v1/apps/mcp/tools", get(list_mcp_tools))
        .route("/v1/apps/plugins", get(plugins::list_plugins))
        .route(
            "/v1/apps/plugins/install",
            post(plugins::install_plugin_api),
        )
        .route(
            "/v1/apps/plugins/{selector}",
            get(plugins::get_plugin).delete(plugins::uninstall_plugin_api),
        )
        .route(
            "/v1/apps/plugins/{selector}/update",
            post(plugins::update_plugin_api),
        )
        .route(
            "/v1/apps/plugins/{selector}/trust",
            post(plugins::trust_plugin_api),
        )
        .route(
            "/v1/apps/plugins/{selector}/enable",
            post(plugins::enable_plugin_api),
        )
        .route(
            "/v1/apps/plugins/{selector}/disable",
            post(plugins::disable_plugin_api),
        )
        .route(
            "/v1/apps/plugins/{selector}/revoke",
            post(plugins::revoke_plugin_api),
        )
        .route(
            "/v1/apps/marketplaces",
            get(plugins::list_marketplaces).post(plugins::add_marketplace),
        )
        .route(
            "/v1/apps/marketplaces/{name}",
            get(plugins::get_marketplace).delete(plugins::remove_marketplace),
        )
        .route(
            "/v1/apps/marketplaces/{name}/install",
            post(plugins::install_marketplace_candidate_api),
        )
        .route(
            "/v1/automations",
            get(list_automations).post(create_automation),
        )
        .route(
            "/v1/automations/{id}",
            get(get_automation)
                .patch(update_automation)
                .delete(delete_automation),
        )
        .route("/v1/automations/{id}/run", post(run_automation))
        .route("/v1/automations/{id}/pause", post(pause_automation))
        .route("/v1/automations/{id}/resume", post(resume_automation))
        .route("/v1/automations/{id}/runs", get(list_automation_runs))
        .route(
            "/v1/operate",
            get(get_operate).post(start_operate).patch(patch_operate),
        )
        .route("/v1/operate/keepalive", post(keepalive_operate))
        .route("/v1/operate/plan", put(put_operate_plan))
        .route("/v1/operate/cancel", post(cancel_operate))
        .route("/v1/operate/stop", post(cancel_operate))
        .route(
            "/v1/operate/auto-merge/check",
            post(check_operate_auto_merge),
        )
        .route("/v1/usage", get(get_usage))
        .route("/v1/snapshots", get(list_snapshots))
        .route("/v1/snapshots/{id}/restore", post(restore_snapshot))
        .route("/v1/providers", get(list_providers))
        .route("/v1/providers/{id}/models", get(list_provider_models))
        .route("/v1/providers/{id}/switch", post(switch_provider))
        .route("/v1/config", get(get_config).post(set_config))
        .route("/v1/config/reload", post(reload_config))
        .route(
            "/v1/memory",
            get(list_memory)
                .post(create_memory_entry)
                .delete(clear_memory),
        )
        .route("/v1/memory/{id}", get(get_memory_entry))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_runtime_token,
        ));

    Router::new()
        .route("/", get(web::web_page))
        .route("/assets/codewhale-web.css", get(web::web_styles))
        .route("/assets/codewhale-web.js", get(web::web_script))
        .route("/assets/codewhale-192.png", get(web::web_icon))
        .route(
            "/__codewhale/bootstrap/{nonce}",
            get(web::exchange_bootstrap),
        )
        .route(
            "/__codewhale/mobile/bootstrap/{nonce}",
            get(exchange_mobile_bootstrap),
        )
        .route("/__codewhale/mobile/session", post(exchange_mobile_session))
        .route(
            "/__codewhale/mobile/stream-ticket",
            post(refresh_mobile_stream_ticket),
        )
        .route("/health", get(health))
        .route("/mobile", get(mobile_page))
        .route("/mobile/", get(mobile_page))
        .route("/v1/runtime/info", get(runtime_info))
        .merge(api_routes)
        .layer(cors_layer(&state.cors_origins))
        .with_state(state)
}

async fn mobile_page(State(state): State<RuntimeApiState>, req: Request) -> Response {
    if !state.mobile_enabled {
        return (
            StatusCode::NOT_FOUND,
            "mobile control is disabled; start with `codewhale serve --mobile`",
        )
            .into_response();
    }
    let _ = req;
    let mut response = Html(MOBILE_HTML).into_response();
    secure_mobile_response(&mut response);
    response
}

#[derive(Serialize)]
struct MobileSessionResponse {
    request_proof: String,
    stream_ticket: String,
    session_expires_in_seconds: u64,
    stream_ticket_expires_in_seconds: u64,
}

async fn exchange_mobile_bootstrap(
    State(state): State<RuntimeApiState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(nonce): Path<String>,
) -> Response {
    let Some(mobile_state) = state.mobile.as_ref() else {
        return mobile_not_found();
    };
    let session = match mobile_state.consume_bootstrap(&nonce, peer.ip()) {
        Ok(session) => session,
        Err(mobile::BootstrapError::NonLoopback) => {
            return secured_mobile_text(StatusCode::FORBIDDEN, "bootstrap unavailable");
        }
        Err(mobile::BootstrapError::Invalid | mobile::BootstrapError::Expired) => {
            return secured_mobile_text(StatusCode::UNAUTHORIZED, "bootstrap unavailable");
        }
    };

    let location = format!(
        "/mobile#request_proof={}&stream_ticket={}",
        session.request_proof, session.stream_ticket
    );
    let cookie = mobile::mobile_session_cookie(&session.session_cookie);
    let mut response = (StatusCode::SEE_OTHER, "").into_response();
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&location).expect("generated mobile fragment is a valid header"),
    );
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("generated mobile cookie is a valid header"),
    );
    secure_mobile_response(&mut response);
    response
}

async fn exchange_mobile_session(State(state): State<RuntimeApiState>, req: Request) -> Response {
    let Some(mobile_state) = state.mobile.as_ref() else {
        return mobile_not_found();
    };
    let Some(expected) = state.runtime_token.as_deref() else {
        return mobile_not_found();
    };
    if !auth::request_has_header_runtime_token(&req, expected) {
        return mobile_unauthorized();
    }
    mobile_session_response(mobile_state.issue_session())
}

async fn refresh_mobile_stream_ticket(
    State(state): State<RuntimeApiState>,
    req: Request,
) -> Response {
    let Some(mobile_state) = state.mobile.as_ref() else {
        return mobile_not_found();
    };
    if !auth::mobile_session_request_is_authorized(&req, &state, mobile_state) {
        return mobile_unauthorized();
    }
    let ticket = mobile_state.refresh_stream_ticket(
        req.headers()
            .get(header::COOKIE)
            .and_then(|value| value.to_str().ok()),
        req.headers()
            .get(mobile::MOBILE_REQUEST_HEADER)
            .and_then(|value| value.to_str().ok()),
    );
    let Some(ticket) = ticket else {
        return mobile_unauthorized();
    };
    let mut response = Json(json!({
        "stream_ticket": ticket.ticket,
        "expires_in_seconds": ticket.expires_in_seconds,
    }))
    .into_response();
    secure_mobile_response(&mut response);
    response
}

fn mobile_session_response(session: mobile::MobileSessionBootstrap) -> Response {
    let cookie = mobile::mobile_session_cookie(&session.session_cookie);
    let mut response = Json(MobileSessionResponse {
        request_proof: session.request_proof,
        stream_ticket: session.stream_ticket,
        session_expires_in_seconds: session.session_ttl_seconds,
        stream_ticket_expires_in_seconds: session.stream_ticket_ttl_seconds,
    })
    .into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("generated mobile cookie is a valid header"),
    );
    secure_mobile_response(&mut response);
    response
}

fn mobile_not_found() -> Response {
    secured_mobile_text(StatusCode::NOT_FOUND, "not found")
}

fn mobile_unauthorized() -> Response {
    let mut response = auth::runtime_token_required_response();
    secure_mobile_response(&mut response);
    response
}

fn secured_mobile_text(status: StatusCode, body: &'static str) -> Response {
    let mut response = (status, body).into_response();
    secure_mobile_response(&mut response);
    response
}

fn secure_mobile_response(response: &mut Response) {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; object-src 'none'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
}

fn print_mobile_urls(
    addr: SocketAddr,
    auth_enabled: bool,
    generated_auth: bool,
    show_qr: bool,
    mobile_bootstrap: Option<&str>,
) {
    println!("Mobile control page enabled.");

    let url = format!("http://{addr}/mobile");
    println!("  URL:   {url}");
    if auth_enabled {
        if let Some(bootstrap) = mobile_bootstrap {
            let bootstrap_url = mobile::bootstrap_url(addr, bootstrap);
            println!(
                "  Bootstrap (single-use, expires in {} min): {bootstrap_url}",
                mobile::BOOTSTRAP_TTL.as_secs() / 60
            );
        } else if generated_auth {
            println!(
                "  Auth uses an unprinted generated token; open the bootstrap URL printed above."
            );
        } else {
            println!(
                "  Use the bootstrap URL; the page also supports one-time bearer entry without storing it."
            );
        }
    }
    println!(
        "Mobile security: loopback-only; no LAN/VPN device access without a verified transport boundary."
    );

    if show_qr {
        println!("  QR is loopback-only and cannot pair another device.");
        match qrcode::QrCode::new(url.as_bytes()) {
            Ok(qr) => {
                let qr_str = qr.render::<qrcode::render::unicode::Dense1x2>().build();
                println!("\n{qr_str}");
            }
            Err(e) => {
                eprintln!("Warning: could not generate QR code: {e}");
            }
        }
    }
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "codewhale-runtime-api",
        mode: "local",
    })
}

fn runtime_request_model(config: &Config, requested: Option<&str>) -> String {
    requested.map_or_else(|| config.default_model(), str::to_string)
}

async fn create_task(
    State(state): State<RuntimeApiState>,
    Json(mut req): Json<NewTaskRequest>,
) -> Result<(StatusCode, Json<TaskRecord>), ApiError> {
    if req.prompt.trim().is_empty() {
        return Err(ApiError::bad_request("prompt is required"));
    }
    if req.workspace.is_none() {
        req.workspace = Some(state.workspace.clone());
    }
    if req.model.is_none() {
        req.model = Some(runtime_request_model(&state.config.read(), None));
    }
    let task = state
        .task_manager
        .add_task(req)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok((StatusCode::CREATED, Json(task)))
}

async fn create_thread(
    State(state): State<RuntimeApiState>,
    Json(mut req): Json<CreateThreadRequest>,
) -> Result<(StatusCode, Json<ThreadRecord>), ApiError> {
    if req.workspace.is_none() {
        req.workspace = Some(state.workspace.clone());
    }
    if req.mode.as_ref().is_none_or(|m| m.trim().is_empty()) {
        req.mode = Some("agent".to_string());
    }

    let thread = state
        .runtime_threads
        .create_thread(req)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok((StatusCode::CREATED, Json(thread)))
}

async fn list_threads(
    State(state): State<RuntimeApiState>,
    Query(query): Query<ThreadsQuery>,
) -> Result<Json<Vec<ThreadRecord>>, ApiError> {
    let filter = resolve_thread_filter(query.include_archived, query.archived_only);
    let threads = state
        .runtime_threads
        .list_threads(filter, query.limit)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(threads))
}

async fn list_threads_summary(
    State(state): State<RuntimeApiState>,
    Query(query): Query<ThreadSummaryQuery>,
) -> Result<Json<Vec<ThreadSummary>>, ApiError> {
    let limit = query.limit.unwrap_or(50).clamp(1, 500);
    let search = query.search.as_deref().map(str::to_ascii_lowercase);
    let filter = resolve_thread_filter(query.include_archived, query.archived_only);
    // `limit` bounds the rows this route returns, not how far a search looks.
    // Passing it to the store read as well matched only inside the newest
    // `limit` threads, so any older match — the row the caller typed the query
    // to find — was invisible. Unsearched listings keep the cheap bounded read;
    // a search scans in newest-first order and stops at `limit` matches.
    //
    // Match on the thread record *before* `get_thread_detail`. Detail is a
    // whole-store turns+items walk, so loading it for every thread made a
    // non-matching dashboard keystroke O(threads × (all_turns + all_items))
    // JSON reads. Preview is filled only for matches; it is not a search key.
    let scan_limit = if search.is_some() { None } else { Some(limit) };
    let threads = state
        .runtime_threads
        .list_threads(filter, scan_limit)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let mut summaries = Vec::new();
    for thread in threads {
        if summaries.len() >= limit {
            break;
        }
        if let Some(search) = &search
            && !state
                .runtime_threads
                .thread_matches_summary_search(&thread, search)
        {
            continue;
        }
        let detail = state
            .runtime_threads
            .get_thread_detail(&thread.id)
            .await
            .map_err(map_thread_err)?;
        let latest_turn = detail.turns.last();
        let latest_status =
            latest_turn.map(|turn| format!("{:?}", turn.status).to_ascii_lowercase());
        let pending_attention_count = detail
            .pending_approvals
            .len()
            .saturating_add(detail.pending_user_inputs.len());

        let title = thread
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| truncate_text(t, 72))
            .unwrap_or_else(|| {
                latest_turn
                    .map(|turn| {
                        if turn.input_summary.trim().is_empty() {
                            "New Thread".to_string()
                        } else {
                            truncate_text(&turn.input_summary, 72)
                        }
                    })
                    .unwrap_or_else(|| "New Thread".to_string())
            });

        let preview = detail
            .items
            .iter()
            .rev()
            .find_map(|item| match item.kind {
                TurnItemKind::AgentMessage | TurnItemKind::UserMessage => {
                    let text = item.detail.clone().unwrap_or_else(|| item.summary.clone());
                    if text.trim().is_empty() {
                        None
                    } else {
                        Some(truncate_text(&text, 140))
                    }
                }
                _ => None,
            })
            .unwrap_or_else(|| title.clone());

        let workspace_git = collect_workspace_git_metadata(&thread.workspace);
        summaries.push(ThreadSummary {
            id: thread.id,
            title,
            preview,
            model: thread.model,
            mode: thread.mode,
            branch: workspace_git.branch,
            head: workspace_git.head,
            dirty: workspace_git.dirty,
            workspace: thread.workspace,
            archived: thread.archived,
            updated_at: thread.updated_at,
            latest_turn_id: thread.latest_turn_id,
            latest_turn_status: latest_status,
            pending_attention_count,
        });
    }

    Ok(Json(summaries))
}

async fn list_agent_runs(
    State(state): State<RuntimeApiState>,
) -> Result<Json<AgentRunsResponse>, ApiError> {
    let runs = load_persisted_agent_worker_records(&state.workspace).map_err(|err| {
        ApiError::internal(format!("Failed to load persisted agent run records: {err}"))
    })?;
    Ok(Json(AgentRunsResponse { runs }))
}

async fn get_agent_run(
    State(state): State<RuntimeApiState>,
    Path(run_id): Path<String>,
) -> Result<Json<AgentWorkerRecord>, ApiError> {
    let runs = load_persisted_agent_worker_records(&state.workspace).map_err(|err| {
        ApiError::internal(format!("Failed to load persisted agent run records: {err}"))
    })?;
    let run = runs
        .into_iter()
        .find(|record| {
            let effective_run_id = if record.spec.run_id.is_empty() {
                record.spec.worker_id.as_str()
            } else {
                record.spec.run_id.as_str()
            };
            effective_run_id == run_id || record.spec.worker_id == run_id
        })
        .ok_or_else(|| ApiError::not_found(format!("agent run '{run_id}' not found")))?;
    Ok(Json(run))
}

async fn list_fleet_profiles(
    State(state): State<RuntimeApiState>,
) -> Result<Json<Value>, ApiError> {
    let manager = open_fleet_manager(&state)?;
    // Same roster path the manager uses to validate `agent_profile` ids on
    // run creation, so GUI pickers can never offer a profile the runtime
    // would reject.
    let roster = manager.agent_roster();
    let profiles = roster
        .members()
        .iter()
        .map(|member| {
            json!({
                "id": member.id.clone(),
                "display_name": member.display_name.clone(),
                "description": member.description.clone(),
                "origin": member.origin.to_string(),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "profiles": profiles,
        "load_error": roster.load_error().map(str::to_string),
    })))
}

async fn create_fleet_run(
    State(state): State<RuntimeApiState>,
    Json(request): Json<CreateFleetRunRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if request.target != FleetRuntimeTarget::ThisComputer {
        return Err(ApiError::not_implemented(format!(
            "Fleet target {:?} is not available in this local Runtime; choose this_computer",
            request.target
        )));
    }
    let (document, descriptor, max_workers) = prepare_managed_fleet_run(request)?;
    let manager = open_fleet_manager(&state)?;
    let report = manager
        .create_queued_run_with_descriptor(document, max_workers, descriptor)
        .map_err(|error| ApiError::bad_request(format!("Failed to create Fleet run: {error}")))?;
    let ledger_state = manager
        .rebuild_state()
        .map_err(|error| ApiError::internal(format!("Failed to rebuild Fleet state: {error}")))?;
    let run = ledger_state
        .runs
        .get(&report.run_id.0)
        .ok_or_else(|| ApiError::internal("Created Fleet run was missing from its ledger"))?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "execution": "awaiting_start",
            "run": fleet_run_detail_json(&manager, run, &ledger_state)?,
            "warnings": report.warnings,
        })),
    ))
}

fn prepare_managed_fleet_run(
    request: CreateFleetRunRequest,
) -> Result<(FleetTaskSpecDocument, ManagedFleetRunDescriptor, usize), ApiError> {
    if request.security_policy.is_some() {
        return Err(ApiError::not_implemented(
            "Managed Fleet security_policy overrides are not executable yet; use named roles and bounded task workspace/tool scopes",
        ));
    }
    if !request.worker_specs.is_empty() {
        return Err(ApiError::not_implemented(
            "Managed Fleet custom worker_specs are not available yet; local Runtime worker IDs are generated per run so worker controls cannot collide across Fleets",
        ));
    }
    if request.roles.is_empty() {
        return Err(ApiError::bad_request(
            "roles must declare at least one named Fleet role",
        ));
    }
    if request.roles.len() > 128 {
        return Err(ApiError::bad_request(
            "roles cannot contain more than 128 entries",
        ));
    }
    let workflow_id = managed_fleet_token("workflow.id", &request.workflow.id)?;
    let workflow_kind = request.workflow.kind;
    let name = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(workflow_id.as_str())
        .to_string();
    if name.len() > 256 || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            "name must be one printable line no longer than 256 bytes",
        ));
    }

    let mut roles = BTreeMap::new();
    for role in request.roles {
        let normalized = canonical_public_role_name(&managed_fleet_token("role.name", &role.name)?);
        let agent_profile = role
            .agent_profile
            .as_deref()
            .map(|profile| managed_fleet_token("role.agent_profile", profile))
            .transpose()?;
        if roles.insert(normalized.clone(), agent_profile).is_some() {
            return Err(ApiError::bad_request(format!(
                "duplicate Fleet role '{normalized}'"
            )));
        }
    }

    let mut tasks = request.workflow.tasks;
    let mut used_roles = BTreeSet::new();
    for task in &mut tasks {
        let worker = task.worker.as_mut().ok_or_else(|| {
            ApiError::bad_request(format!(
                "Fleet task '{}' must select one named role through worker.role",
                task.id
            ))
        })?;
        let role = worker.role.as_deref().ok_or_else(|| {
            ApiError::bad_request(format!(
                "Fleet task '{}' must select one named role through worker.role",
                task.id
            ))
        })?;
        let role = canonical_public_role_name(&managed_fleet_token("task.worker.role", role)?);
        let declared_profile = roles.get(&role).ok_or_else(|| {
            ApiError::bad_request(format!(
                "Fleet task '{}' references undeclared role '{role}'",
                task.id
            ))
        })?;
        if let Some(profile) = declared_profile {
            match worker.agent_profile.as_deref() {
                Some(task_profile) if task_profile != profile => {
                    return Err(ApiError::bad_request(format!(
                        "Fleet task '{}' overrides role '{role}' agent_profile '{profile}' with '{task_profile}'",
                        task.id
                    )));
                }
                None => worker.agent_profile = Some(profile.clone()),
                Some(_) => {}
            }
        }
        worker.role = Some(role.clone());
        used_roles.insert(role);
    }
    let unused_roles = roles
        .keys()
        .filter(|role| !used_roles.contains(*role))
        .cloned()
        .collect::<Vec<_>>();
    if !unused_roles.is_empty() {
        return Err(ApiError::bad_request(format!(
            "Every declared Fleet role must own a Workflow task; unused roles: {}",
            unused_roles.join(", ")
        )));
    }
    reject_parallel_write_collisions(&tasks)?;

    let default_workers = roles.len().min(tasks.len()).max(1);
    let max_workers = request.max_workers.unwrap_or(default_workers);
    if !(1..=128).contains(&max_workers) {
        return Err(ApiError::bad_request(
            "max_workers must be between 1 and 128",
        ));
    }
    let role_names = roles.into_keys().collect::<Vec<_>>();
    Ok((
        FleetTaskSpecDocument {
            name: Some(name),
            labels: request.labels,
            security_policy: None,
            workers: Vec::new(),
            tasks,
            usage_ceiling: request.usage_ceiling,
        },
        ManagedFleetRunDescriptor {
            target: Some(request.target),
            workflow: Some(FleetWorkflowDescriptor {
                id: workflow_id,
                kind: workflow_kind,
            }),
            roles: role_names,
        },
        max_workers,
    ))
}

fn managed_fleet_token(field: &str, value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 128
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(ApiError::bad_request(format!(
            "{field} must be a simple ASCII token no longer than 128 bytes"
        )));
    }
    Ok(value.to_string())
}

fn reject_parallel_write_collisions(tasks: &[FleetTaskSpec]) -> Result<(), ApiError> {
    let mut claims: Vec<(String, String)> = Vec::new();
    for task in tasks {
        let write_roots = fleet_write_roots(task).map_err(|error| {
            ApiError::bad_request(format!(
                "Fleet task '{}' has an invalid write scope: {error}",
                task.id
            ))
        })?;
        for normalized in write_roots {
            for (owner, existing) in &claims {
                if owner != &task.id && managed_paths_overlap(existing.as_str(), &normalized) {
                    return Err(ApiError::bad_request(format!(
                        "Parallel Workflow write scope collision: tasks '{owner}' and '{}' both claim overlapping paths",
                        task.id
                    )));
                }
            }
            claims.push((task.id.clone(), normalized));
        }
    }
    Ok(())
}

fn managed_paths_overlap(left: &str, right: &str) -> bool {
    // `normalize_fleet_relative_path` collapses the workspace root to ".", so
    // a task claiming the whole tree presents as "." rather than as a textual
    // prefix of its siblings. String containment alone never matched it, and
    // two workers could be admitted to write the same tree in parallel.
    if left == "." || right == "." {
        return true;
    }
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

async fn start_fleet_run(
    State(state): State<RuntimeApiState>,
    Path(run_id): Path<String>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let manager = open_fleet_manager(&state)?;
    let durable = manager
        .rebuild_state()
        .map_err(|error| ApiError::internal(format!("Failed to rebuild Fleet state: {error}")))?;
    let run = durable
        .runs
        .get(&run_id)
        .ok_or_else(|| ApiError::not_found(format!("Fleet run '{run_id}' not found")))?;
    match run.target {
        Some(FleetRuntimeTarget::ThisComputer) => {}
        Some(target) => {
            return Err(ApiError::not_implemented(format!(
                "Fleet target {target:?} is not available in this local Runtime"
            )));
        }
        None => {
            return Err(ApiError::bad_request(
                "Fleet run has no explicit Runtime target and cannot be started through the managed API",
            ));
        }
    }
    if run.workflow.is_none() || run.roles.is_empty() {
        return Err(ApiError::bad_request(
            "Fleet run has no managed Workflow/role descriptor and cannot be started through the managed API",
        ));
    }
    let run_id = FleetRunId::from(run_id);
    let report = manager.activate_run(&run_id).map_err(|error| {
        let message = format!("Failed to start Fleet run '{}': {error}", run_id.0);
        if message.contains("already terminal") {
            ApiError::conflict(message)
        } else {
            ApiError::bad_request(message)
        }
    })?;
    let max_workers = durable
        .runs
        .get(&run_id.0)
        .and_then(|run| run.max_workers)
        .unwrap_or_else(|| report.worker_ids.len().max(1));
    let workspace = state.workspace.clone();
    let codewhale_binary = state.fleet_codewhale_binary.clone();
    let execution_run_id = run_id.clone();
    tokio::spawn(async move {
        let mut executor = FleetExecutor::new(&workspace);
        if let Err(error) = manager
            .run_to_completion(
                &execution_run_id,
                max_workers,
                &mut executor,
                &codewhale_binary,
                None,
                Duration::from_millis(250),
            )
            .await
        {
            tracing::error!(
                run_id = %execution_run_id.0,
                error = %error,
                "Runtime API Fleet manager exited with an error"
            );
        }
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "action": "start",
            "execution": "scheduled",
            "run_id": run_id.0,
            "target": "this_computer",
            "leased": report.leased,
            "queued": report.queued,
            "worker_ids": report.worker_ids,
        })),
    ))
}

async fn replay_fleet_events(
    State(state): State<RuntimeApiState>,
    Path(run_id): Path<String>,
    Query(query): Query<FleetEventsQuery>,
) -> Result<Json<FleetEventReplay>, ApiError> {
    let (after, limit) = validate_fleet_events_query(query)?;
    let replay = load_fleet_event_replay(state, FleetRunId::from(run_id), after, limit)
        .await
        .map_err(map_fleet_replay_error)?;
    Ok(Json(replay))
}

async fn stream_fleet_events(
    State(state): State<RuntimeApiState>,
    Path(run_id): Path<String>,
    Query(query): Query<FleetEventsQuery>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<SseEvent, Infallible>>>, ApiError> {
    let (after, limit) = validate_fleet_events_query(query)?;
    let run_id = FleetRunId::from(run_id);
    let initial = load_fleet_event_replay(state.clone(), run_id.clone(), after.clone(), limit)
        .await
        .map_err(map_fleet_replay_error)?;
    let event_stream = replay_live_fleet_events(state, run_id, after, limit, initial);
    Ok(Sse::new(event_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

fn replay_live_fleet_events(
    state: RuntimeApiState,
    run_id: FleetRunId,
    mut after: Option<String>,
    limit: usize,
    initial: FleetEventReplay,
) -> impl futures_util::Stream<Item = Result<SseEvent, Infallible>> {
    stream! {
        let mut page = initial;
        loop {
            if page.history_truncated {
                yield Ok(sse_json(
                    "fleet.replay.truncated",
                    json!({
                        "run_id": run_id.0.clone(),
                        "reload_projection": true,
                    }),
                ));
            }
            for event in page.events {
                after = Some(event.cursor.clone());
                yield Ok(fleet_sse_event(&event));
            }
            if !page.has_more {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            match load_fleet_event_replay(
                state.clone(),
                run_id.clone(),
                after.clone(),
                limit,
            )
            .await
            {
                Ok(next) => page = next,
                Err(FleetEventReplayError::CursorUnavailable { .. }) => {
                    yield Ok(sse_json(
                        "fleet.replay.cursor_unavailable",
                        json!({
                            "run_id": run_id.0.clone(),
                            "reload_projection": true,
                        }),
                    ));
                    return;
                }
                Err(error) => {
                    tracing::warn!(
                        run_id = %run_id.0,
                        error = %error,
                        "Fleet event stream stopped while reading durable history"
                    );
                    yield Ok(sse_json(
                        "fleet.stream.error",
                        json!({ "retryable": true }),
                    ));
                    return;
                }
            }
        }
    }
}

async fn load_fleet_event_replay(
    state: RuntimeApiState,
    run_id: FleetRunId,
    after: Option<String>,
    limit: usize,
) -> std::result::Result<FleetEventReplay, FleetEventReplayError> {
    tokio::task::spawn_blocking(move || {
        let manager =
            open_fleet_manager(&state).map_err(|error| FleetEventReplayError::Storage {
                message: error.message,
            })?;
        manager.replay_events(&run_id, after.as_deref(), limit)
    })
    .await
    .map_err(|error| FleetEventReplayError::Storage {
        message: format!("Fleet replay worker failed: {error}"),
    })?
}

fn validate_fleet_events_query(
    query: FleetEventsQuery,
) -> Result<(Option<String>, usize), ApiError> {
    let after = query
        .after
        .map(|cursor| cursor.trim().to_string())
        .filter(|cursor| !cursor.is_empty());
    if after.as_deref().is_some_and(|cursor| {
        cursor.len() > 96
            || !cursor.starts_with("fev1_")
            || !cursor
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    }) {
        return Err(ApiError::bad_request(
            "after is not a valid Fleet event cursor",
        ));
    }
    let limit = query.limit.unwrap_or(DEFAULT_FLEET_EVENT_REPLAY_LIMIT);
    if !(1..=MAX_FLEET_EVENT_REPLAY_LIMIT).contains(&limit) {
        return Err(ApiError::bad_request(format!(
            "limit must be between 1 and {MAX_FLEET_EVENT_REPLAY_LIMIT}"
        )));
    }
    Ok((after, limit))
}

fn map_fleet_replay_error(error: FleetEventReplayError) -> ApiError {
    let message = error.to_string();
    match error {
        FleetEventReplayError::UnknownRun { .. } => ApiError::not_found(message),
        FleetEventReplayError::CursorUnavailable { .. } => ApiError::conflict(message),
        FleetEventReplayError::Storage { .. } => ApiError::internal(message),
    }
}

fn fleet_sse_event(event: &FleetRuntimeEvent) -> SseEvent {
    let data = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string());
    SseEvent::default()
        .id(event.cursor.clone())
        .event(event.event.clone())
        .data(data)
}

async fn list_fleet_runs(State(state): State<RuntimeApiState>) -> Result<Json<Value>, ApiError> {
    let manager = open_fleet_manager(&state)?;
    let ledger_state = manager
        .rebuild_state()
        .map_err(|err| ApiError::internal(format!("Failed to rebuild Fleet state: {err}")))?;
    let runs: Vec<_> = ledger_state
        .runs
        .values()
        .map(|run| fleet_run_summary_json(&manager, run, &ledger_state))
        .collect::<Result<Vec<_>, _>>()?;
    let status = manager
        .status()
        .map_err(|err| ApiError::internal(format!("Failed to read Fleet status: {err}")))?;
    Ok(Json(json!({
        "status": fleet_status_json(&status),
        "runs": runs,
    })))
}

async fn get_fleet_run(
    State(state): State<RuntimeApiState>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let manager = open_fleet_manager(&state)?;
    let ledger_state = manager
        .rebuild_state()
        .map_err(|err| ApiError::internal(format!("Failed to rebuild Fleet state: {err}")))?;
    let run = ledger_state
        .runs
        .get(&run_id)
        .ok_or_else(|| ApiError::not_found(format!("Fleet run '{run_id}' not found")))?;
    Ok(Json(fleet_run_detail_json(&manager, run, &ledger_state)?))
}

async fn list_fleet_run_workers(
    State(state): State<RuntimeApiState>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let manager = open_fleet_manager(&state)?;
    let ledger_state = manager
        .rebuild_state()
        .map_err(|err| ApiError::internal(format!("Failed to rebuild Fleet state: {err}")))?;
    let run = ledger_state
        .runs
        .get(&run_id)
        .ok_or_else(|| ApiError::not_found(format!("Fleet run '{run_id}' not found")))?;
    let workers = run
        .worker_specs
        .iter()
        .map(|worker| {
            manager
                .inspect_worker(&worker.id)
                .map(|inspection| fleet_worker_json(&inspection))
                .map_err(|err| {
                    ApiError::internal(format!(
                        "Failed to inspect Fleet worker {}: {err}",
                        worker.id
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(json!({
        "run_id": run_id,
        "workers": workers,
    })))
}

async fn get_fleet_worker(
    State(state): State<RuntimeApiState>,
    Path(worker_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let manager = open_fleet_manager(&state)?;
    let inspection = manager.inspect_worker(&worker_id).map_err(|err| {
        ApiError::not_found(format!("Fleet worker '{worker_id}' not found: {err}"))
    })?;
    Ok(Json(fleet_worker_json(&inspection)))
}

async fn interrupt_fleet_worker(
    State(state): State<RuntimeApiState>,
    Path(worker_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let manager = open_fleet_manager(&state)?;
    let inspection = manager.interrupt_worker(&worker_id).map_err(|err| {
        ApiError::bad_request(format!(
            "Failed to interrupt Fleet worker '{worker_id}': {err}"
        ))
    })?;
    Ok(Json(json!({
        "action": "interrupt",
        "worker": fleet_worker_json(&inspection),
    })))
}

async fn stop_fleet_worker(
    State(state): State<RuntimeApiState>,
    Path(worker_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let manager = open_fleet_manager(&state)?;
    let inspection = manager.interrupt_worker(&worker_id).map_err(|err| {
        ApiError::bad_request(format!("Failed to stop Fleet worker '{worker_id}': {err}"))
    })?;
    Ok(Json(json!({
        "action": "stop",
        "worker": fleet_worker_json(&inspection),
    })))
}

async fn restart_fleet_worker(
    State(state): State<RuntimeApiState>,
    Path(worker_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let manager = open_fleet_manager(&state)?;
    let report = manager.restart_worker(&worker_id).map_err(|err| {
        ApiError::bad_request(format!(
            "Failed to restart Fleet worker '{worker_id}': {err}"
        ))
    })?;
    let worker = fleet_worker_json(&report.inspection);
    let run_id = report.run_id.clone();
    let max_workers = report.max_workers;
    let workspace = state.workspace.clone();
    let codewhale_binary = state.fleet_codewhale_binary.clone();
    tokio::spawn(async move {
        let mut executor = FleetExecutor::new(&workspace);
        if let Err(err) = manager
            .run_to_completion(
                &run_id,
                max_workers,
                &mut executor,
                &codewhale_binary,
                None,
                Duration::from_millis(250),
            )
            .await
        {
            tracing::error!(
                run_id = %run_id.0,
                error = %err,
                "Runtime API Fleet restart manager exited with an error"
            );
        }
    });
    Ok(Json(json!({
        "action": "restart",
        "execution": "scheduled",
        "run_id": report.run_id.0,
        "worker": worker,
    })))
}

async fn stop_fleet_run(
    State(state): State<RuntimeApiState>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let manager = open_fleet_manager(&state)?;
    let run_id = FleetRunId::from(run_id);
    let stopped = manager.stop_run(&run_id).map_err(|err| {
        ApiError::bad_request(format!("Failed to stop Fleet run '{}': {err}", run_id.0))
    })?;
    let status = manager
        .run_status(&run_id)
        .map_err(|err| ApiError::internal(format!("Failed to read Fleet run status: {err}")))?;
    Ok(Json(json!({
        "action": "stop",
        "run_id": run_id.0,
        "stopped": stopped,
        "status": fleet_status_json(&status),
    })))
}

/// Maximum bytes read from a receipt evidence file for the inspection endpoint.
const MAX_RECEIPT_EVIDENCE_READ_BYTES: u64 = 65_536;

async fn list_fleet_run_receipts(
    State(state): State<RuntimeApiState>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let manager = open_fleet_manager(&state)?;
    let ledger_state = manager
        .rebuild_state()
        .map_err(|err| ApiError::internal(format!("Failed to rebuild Fleet state: {err}")))?;
    if !ledger_state.runs.contains_key(&run_id) {
        return Err(ApiError::not_found(format!(
            "Fleet run '{run_id}' not found"
        )));
    }
    let run_id_parsed = FleetRunId::from(run_id.clone());
    let receipts: Vec<Value> = ledger_state
        .receipts
        .values()
        .filter(|r| r.run_id == run_id_parsed)
        .map(fleet_receipt_json)
        .collect();
    Ok(Json(json!({
        "run_id": run_id,
        "receipts": receipts,
    })))
}

async fn get_fleet_run_receipt(
    State(state): State<RuntimeApiState>,
    Path((run_id, task_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let manager = open_fleet_manager(&state)?;
    let ledger_state = manager
        .rebuild_state()
        .map_err(|err| ApiError::internal(format!("Failed to rebuild Fleet state: {err}")))?;
    let key = format!("{run_id}:{task_id}");
    let receipt = ledger_state.receipts.get(&key).ok_or_else(|| {
        ApiError::not_found(format!(
            "no receipt found for run '{run_id}' task '{task_id}'"
        ))
    })?;
    Ok(Json(fleet_receipt_json(receipt)))
}

/// Receipt artifact paths are recorded relative to the workspace; an absolute
/// path (including Windows drive prefixes and root-relative paths) or any `..`
/// component would escape it.
fn receipt_evidence_path_is_confined(path: &std::path::Path) -> bool {
    use std::path::Component;
    !path.is_absolute()
        && !path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

async fn inspect_fleet_run_receipt_evidence(
    State(state): State<RuntimeApiState>,
    Path((run_id, task_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let manager = open_fleet_manager(&state)?;
    let ledger_state = manager
        .rebuild_state()
        .map_err(|err| ApiError::internal(format!("Failed to rebuild Fleet state: {err}")))?;
    let key = format!("{run_id}:{task_id}");
    let receipt = ledger_state.receipts.get(&key).ok_or_else(|| {
        ApiError::not_found(format!(
            "no receipt found for run '{run_id}' task '{task_id}'"
        ))
    })?;
    // Locate the most recent Receipt-kind artifact.
    let receipt_artifact = receipt
        .artifacts
        .iter()
        .rfind(|a| a.kind == FleetArtifactKind::Receipt)
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "no verifier evidence file for run '{run_id}' task '{task_id}'"
            ))
        })?;
    // Receipt artifacts are workspace-relative paths recorded by the verifier;
    // reject absolute paths and `..` escapes before joining onto the workspace.
    // An EMPTY recorded path is not a path at all: joining it would resolve
    // to the workspace directory itself and read it as a file.
    if receipt_artifact.path.as_os_str().is_empty() {
        return Err(ApiError::not_found(format!(
            "no verifier evidence file recorded for run '{run_id}' task '{task_id}'"
        )));
    }
    if !receipt_evidence_path_is_confined(&receipt_artifact.path) {
        return Err(ApiError::bad_request(format!(
            "evidence path for run '{run_id}' task '{task_id}' escapes the workspace"
        )));
    }
    let abs_path = state.workspace.join(&receipt_artifact.path);
    let metadata = std::fs::metadata(&abs_path).map_err(|err| {
        ApiError::not_found(format!(
            "evidence file not readable for run '{run_id}' task '{task_id}': {err}"
        ))
    })?;
    let size_bytes = metadata.len();
    let truncated = size_bytes > MAX_RECEIPT_EVIDENCE_READ_BYTES;
    let raw = {
        use std::io::Read;
        let file = std::fs::File::open(&abs_path)
            .map_err(|err| ApiError::internal(format!("Failed to open evidence file: {err}")))?;
        let mut buf = Vec::new();
        file.take(MAX_RECEIPT_EVIDENCE_READ_BYTES)
            .read_to_end(&mut buf)
            .map_err(|err| ApiError::internal(format!("Failed to read evidence file: {err}")))?;
        buf
    };
    // Parse as JSON if possible; fall back to a raw string representation.
    let content: Value = serde_json::from_slice(&raw)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).into_owned()));
    Ok(Json(json!({
        "run_id": run_id,
        "task_id": task_id,
        "path": receipt_artifact.path,
        "checksum": receipt_artifact.checksum,
        "size_bytes": size_bytes,
        "truncated": truncated,
        "content": content,
    })))
}

fn open_fleet_manager(state: &RuntimeApiState) -> Result<FleetManager, ApiError> {
    let (exec_config, fleet_config, session_model, route_config) = {
        let config = state.config.read();
        let exec_config = config
            .fleet
            .as_ref()
            .map(|fleet| fleet.exec.clone())
            .unwrap_or_default();
        // The active session route is the operator: workers without a
        // task/profile model pin inherit the model the user picked in /model.
        (
            exec_config,
            config.fleet_config(),
            config.default_model(),
            config.clone(),
        )
    };
    FleetManager::open(&state.workspace)
        .map(|manager| {
            manager
                .with_exec_config(exec_config)
                .with_fleet_config(fleet_config)
                .with_sub_agent_manager(state.sub_agent_manager.clone())
                .with_session_model(session_model)
                .with_route_config(route_config)
        })
        .map_err(|err| ApiError::internal(format!("Failed to open Fleet manager: {err}")))
}

fn fleet_run_summary_json(
    manager: &FleetManager,
    run: &FleetRun,
    ledger_state: &FleetLedgerState,
) -> Result<Value, ApiError> {
    let status = manager
        .run_status(&run.id)
        .map_err(|err| ApiError::internal(format!("Failed to read Fleet run status: {err}")))?;
    let task_statuses = ledger_state
        .tasks
        .values()
        .filter(|task| task.entry.run_id == run.id)
        .map(|task| {
            json!({
                "task_id": task.entry.task_id.clone(),
                "status": fleet_task_status_label(task.status),
                "leased_to": task.leased_to.clone(),
                "attempts": task.entry.attempts,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "id": run.id.0.clone(),
        "name": run.name.clone(),
        "lifecycle_status": ledger_state
            .run_status_overrides
            .get(&run.id.0)
            .unwrap_or(&run.status),
        "status": fleet_status_json(&status),
        "target": run.target,
        "workflow": run.workflow.clone(),
        "roles": run.roles.clone(),
        "task_count": run.task_specs.len(),
        "worker_count": run.worker_specs.len(),
        "tasks": task_statuses,
        "labels": run.labels.clone(),
        "created_at": run.created_at.clone(),
        "updated_at": run.updated_at.clone(),
        "completed_at": run.completed_at.clone(),
    }))
}

fn fleet_run_detail_json(
    manager: &FleetManager,
    run: &FleetRun,
    ledger_state: &FleetLedgerState,
) -> Result<Value, ApiError> {
    let mut value = fleet_run_summary_json(manager, run, ledger_state)?;
    if let Some(map) = value.as_object_mut() {
        map.insert("task_specs".to_string(), json!(run.task_specs.clone()));
        map.insert("worker_specs".to_string(), json!(run.worker_specs.clone()));
    }
    Ok(value)
}

fn fleet_status_json(status: &FleetStatusSnapshot) -> Value {
    json!({
        "runs": status.runs,
        "queued": status.queued,
        "running": status.running,
        "completed": status.completed,
        "partial": status.partial,
        "failed": status.failed,
        "restarted": status.restarted,
        "escalated": status.escalated,
        "transport_failed": status.transport_failed,
        "task_failed": status.task_failed,
        "verifier_failed": status.verifier_failed,
        "cancelled": status.cancelled,
        "stale": status.stale,
        "workers": status
            .workers
            .iter()
            .map(|(worker_id, status)| {
                (
                    worker_id.clone(),
                    Value::String(worker_status_label(status).to_string()),
                )
            })
            .collect::<serde_json::Map<String, Value>>(),
    })
}

fn fleet_worker_json(inspection: &FleetWorkerInspection) -> Value {
    json!({
        "worker_id": inspection.worker_id.clone(),
        "status": worker_status_label(&inspection.status),
        "run_id": inspection.current_run_id.as_ref().map(|run_id| run_id.0.clone()),
        "task_id": inspection.current_task_id.clone(),
        "objective": inspection.objective.clone(),
        "role": inspection.role.clone(),
        "host": inspection.host.clone(),
        "latest_heartbeat_at": inspection.latest_heartbeat_at.clone(),
        "latest_event": inspection.latest_event.as_ref().map(fleet_event_json),
        "artifacts": inspection.artifacts.iter().map(fleet_artifact_json).collect::<Vec<_>>(),
        "last_error": inspection.last_error.clone(),
        "alert_state": inspection.alert_state.clone(),
        "runtime_state": inspection.runtime_state.as_ref().map(fleet_worker_runtime_json),
    })
}

fn fleet_worker_runtime_json(runtime: &FleetWorkerRuntimeProjection) -> Value {
    json!({
        "agent_status": runtime.agent_status.clone(),
        "steps_taken": runtime.steps_taken,
        "latest_message": runtime.latest_message.clone(),
        "error": runtime.error.clone(),
        "result_summary": runtime.result_summary.clone(),
        "has_session": runtime.has_session,
    })
}

fn fleet_artifact_json(artifact: &codewhale_protocol::fleet::FleetArtifactRef) -> Value {
    json!({
        "kind": artifact_kind_label(&artifact.kind),
        "path": artifact.path.clone(),
        "checksum": artifact.checksum.clone(),
        "mime_type": artifact.mime_type.clone(),
        "size_bytes": artifact.size_bytes,
    })
}

fn fleet_receipt_json(receipt: &codewhale_protocol::fleet::FleetReceipt) -> Value {
    use codewhale_protocol::fleet::{FleetTaskFailureKind, FleetTaskResult};

    let result_label = match receipt.result {
        FleetTaskResult::Pass => "pass",
        FleetTaskResult::Partial => "partial",
        FleetTaskResult::Fail => "fail",
        FleetTaskResult::Skip => "skip",
        FleetTaskResult::Timeout => "timeout",
    };
    let (failure_kind_label, failure_class, retry_eligible) = match receipt.failure_kind.as_ref() {
        Some(FleetTaskFailureKind::Transport) => (
            Some("transport"),
            Some("Infrastructure or network failure during task transport"),
            true,
        ),
        Some(FleetTaskFailureKind::Task) => (
            Some("task"),
            Some("Task logic exited unsuccessfully"),
            false,
        ),
        Some(FleetTaskFailureKind::Verifier) => (
            Some("verifier"),
            Some("Verifier rejected the task output; manual review or code change required"),
            false,
        ),
        None => (None, None, false),
    };
    let evidence_available = receipt
        .artifacts
        .iter()
        .any(|a| a.kind == FleetArtifactKind::Receipt);
    let score_json = receipt.score.as_ref().map(|s| {
        json!({
            "value": s.value,
            "max": s.max,
            "notes": s.notes,
        })
    });
    json!({
        "run_id": receipt.run_id.0.clone(),
        "task_id": receipt.task_id.clone(),
        "worker_id": receipt.worker_id.clone(),
        "attempt": receipt.attempt,
        "terminal_seq": receipt.terminal_seq,
        "completed_at": receipt.completed_at.clone(),
        "result": result_label,
        "failure_kind": failure_kind_label,
        "failure_class": failure_class,
        "retry_eligible": retry_eligible,
        "score": score_json,
        "artifacts": receipt.artifacts.iter().map(fleet_artifact_json).collect::<Vec<_>>(),
        "evidence_available": evidence_available,
    })
}

fn fleet_event_json(event: &codewhale_protocol::fleet::FleetWorkerEvent) -> Value {
    json!({
        "seq": event.seq,
        "run_id": event.run_id.0.clone(),
        "worker_id": event.worker_id.clone(),
        "task_id": event.task_id.clone(),
        "timestamp": event.timestamp.clone(),
        "label": fleet_event_label(&event.payload),
        "payload": event.payload.clone(),
    })
}

fn worker_status_label(status: &FleetWorkerStatus) -> &'static str {
    match status {
        FleetWorkerStatus::Unknown => "unknown",
        FleetWorkerStatus::Online => "online",
        FleetWorkerStatus::Busy => "busy",
        FleetWorkerStatus::Offline => "offline",
        FleetWorkerStatus::Unhealthy => "unhealthy",
        FleetWorkerStatus::Draining => "draining",
        FleetWorkerStatus::Retired => "retired",
    }
}

fn fleet_task_status_label(status: FleetTaskLedgerStatus) -> &'static str {
    match status {
        FleetTaskLedgerStatus::Enqueued => "enqueued",
        FleetTaskLedgerStatus::Leased => "leased",
        FleetTaskLedgerStatus::Completed => "completed",
        FleetTaskLedgerStatus::Failed => "failed",
        FleetTaskLedgerStatus::Cancelled => "cancelled",
    }
}

fn artifact_kind_label(kind: &FleetArtifactKind) -> String {
    match kind {
        FleetArtifactKind::Log => "log".to_string(),
        FleetArtifactKind::Patch => "patch".to_string(),
        FleetArtifactKind::TestResult => "test_result".to_string(),
        FleetArtifactKind::Report => "report".to_string(),
        FleetArtifactKind::Checkpoint => "checkpoint".to_string(),
        FleetArtifactKind::Receipt => "receipt".to_string(),
        FleetArtifactKind::Other(value) => value.clone(),
    }
}

fn fleet_event_label(payload: &FleetWorkerEventPayload) -> String {
    match payload {
        FleetWorkerEventPayload::Queued => "queued".to_string(),
        FleetWorkerEventPayload::Leased { .. } => "leased".to_string(),
        FleetWorkerEventPayload::Starting => "starting".to_string(),
        FleetWorkerEventPayload::Running => "running".to_string(),
        FleetWorkerEventPayload::ModelWait { model } => model
            .as_ref()
            .map(|model| format!("model_wait model={model}"))
            .unwrap_or_else(|| "model_wait".to_string()),
        FleetWorkerEventPayload::RunningTool { tool, call_id } => call_id
            .as_ref()
            .map(|call_id| format!("running_tool tool={tool} call_id={call_id}"))
            .unwrap_or_else(|| format!("running_tool tool={tool}")),
        FleetWorkerEventPayload::WorkflowEvent {
            workflow_run_id,
            event,
        } => event
            .get("type")
            .and_then(serde_json::Value::as_str)
            .map(|kind| format!("workflow_event run_id={workflow_run_id} type={kind}"))
            .unwrap_or_else(|| format!("workflow_event run_id={workflow_run_id}")),
        FleetWorkerEventPayload::Heartbeat { .. } => "heartbeat".to_string(),
        FleetWorkerEventPayload::UsageReport {
            input_tokens,
            output_tokens,
        } => format!("usage_report input={input_tokens} output={output_tokens}"),
        FleetWorkerEventPayload::Artifact(artifact) => {
            format!("artifact kind={}", artifact_kind_label(&artifact.kind))
        }
        FleetWorkerEventPayload::Completed { exit_code, summary } => match (exit_code, summary) {
            (Some(code), Some(summary)) => format!("completed exit_code={code} {summary}"),
            (Some(code), None) => format!("completed exit_code={code}"),
            (None, Some(summary)) => format!("completed {summary}"),
            (None, None) => "completed".to_string(),
        },
        FleetWorkerEventPayload::Failed {
            reason,
            recoverable,
        } => {
            format!("failed recoverable={recoverable} reason={reason}")
        }
        FleetWorkerEventPayload::Cancelled { cancelled_by } => cancelled_by
            .as_ref()
            .map(|by| format!("cancelled by={by}"))
            .unwrap_or_else(|| "cancelled".to_string()),
        FleetWorkerEventPayload::Interrupted { signal } => signal
            .as_ref()
            .map(|signal| format!("interrupted signal={signal}"))
            .unwrap_or_else(|| "interrupted".to_string()),
        FleetWorkerEventPayload::Stale { last_heartbeat_at } => last_heartbeat_at
            .as_ref()
            .map(|ts| format!("stale last_heartbeat_at={ts}"))
            .unwrap_or_else(|| "stale".to_string()),
        FleetWorkerEventPayload::Restarted { restart_count } => {
            format!("restarted count={restart_count}")
        }
        FleetWorkerEventPayload::Escalated { channel, alert_id } => alert_id
            .as_ref()
            .map(|alert_id| format!("escalated channel={channel} alert_id={alert_id}"))
            .unwrap_or_else(|| format!("escalated channel={channel}")),
    }
}

async fn list_skills(
    State(state): State<RuntimeApiState>,
) -> Result<Json<SkillsResponse>, ApiError> {
    let (skills_dir, mode) = {
        let config = state.config.read();
        let skills_dir = resolve_skills_dir(&config, &state.workspace);
        let mode = crate::skills::SkillDiscoveryMode::from_codewhale_only(
            config.skills_config().scan_codewhale_only(),
        );
        (skills_dir, mode)
    };
    let plugin_registry = state
        .plugin_discovery
        .registry_for_workspace(&state.workspace);
    let (registry, directories) = discover_skills_for_runtime_api(
        &state.workspace,
        &skills_dir,
        mode,
        Some(plugin_registry.as_ref()),
    );
    let mut skill_state = state.skill_state.lock().await;
    skill_state
        .refresh()
        .map_err(|error| ApiError::internal(format!("refresh skill state: {error}")))?;
    let skills = registry
        .list()
        .iter()
        .map(|skill| {
            let (path, source, plugin_id, plugin_generation, plugin_content_hash) =
                match &skill.source {
                    crate::skills::SkillSource::Native => (
                        Some(skill.path.clone()),
                        "native".to_string(),
                        None,
                        None,
                        None,
                    ),
                    crate::skills::SkillSource::Plugin {
                        plugin_id,
                        plugin_name,
                        authority,
                    } => (
                        None,
                        format!("reviewed-plugin-snapshot:{plugin_name}"),
                        Some(plugin_id.clone()),
                        Some(authority.state_generation),
                        Some(authority.content_hash.clone()),
                    ),
                };
            SkillEntry {
                name: skill.name.clone(),
                description: skill.description.clone(),
                path,
                source,
                plugin_id,
                plugin_generation,
                plugin_content_hash,
                enabled: skill_state.is_enabled(&skill.name),
                is_bundled: skill_entry_is_bundled(skill, &skills_dir),
            }
        })
        .collect();
    Ok(Json(SkillsResponse {
        directory: skills_dir,
        directories,
        warnings: registry.warnings().to_vec(),
        skills,
    }))
}

async fn set_skill_enabled(
    State(state): State<RuntimeApiState>,
    Path(name): Path<String>,
    Json(req): Json<SetSkillEnabledRequest>,
) -> Result<Json<SetSkillEnabledResponse>, ApiError> {
    let (skills_dir, mode) = {
        let config = state.config.read();
        let skills_dir = resolve_skills_dir(&config, &state.workspace);
        let mode = crate::skills::SkillDiscoveryMode::from_codewhale_only(
            config.skills_config().scan_codewhale_only(),
        );
        (skills_dir, mode)
    };
    let plugin_registry = state
        .plugin_discovery
        .registry_for_workspace(&state.workspace);
    let (registry, directories) = discover_skills_for_runtime_api(
        &state.workspace,
        &skills_dir,
        mode,
        Some(plugin_registry.as_ref()),
    );
    let exists = registry.list().iter().any(|skill| skill.name == name);
    if !exists {
        return Err(ApiError::not_found(format!(
            "skill '{name}' not found in searched directories: {}",
            format_skill_search_paths(&directories)
        )));
    }

    let mut store = state.skill_state.lock().await;
    store
        .set_enabled(&name, req.enabled)
        .map_err(|err| ApiError::internal(format!("persist skill state: {err}")))?;
    Ok(Json(SetSkillEnabledResponse {
        name,
        enabled: req.enabled,
    }))
}

// ─── Skill lifecycle helpers ────────────────────────────────────────────────

/// Build a [`crate::skills::mutation::MutationContext`] from the current
/// server state. Reads the network policy and installer settings directly
/// from the config already held in `state`.
fn mutation_context_settings(
    state: &RuntimeApiState,
) -> (
    crate::network_policy::NetworkPolicy,
    u64,
    String,
    Option<PathBuf>,
) {
    use crate::skills::install::{DEFAULT_MAX_SIZE_BYTES, DEFAULT_REGISTRY_URL};
    let config = state.config.read();
    let network = config
        .network
        .clone()
        .map(|p| p.into_runtime())
        .unwrap_or_default();
    let skills_cfg = config.skills.as_ref();
    let max_size = skills_cfg
        .and_then(|s| s.max_install_size_bytes)
        .unwrap_or(DEFAULT_MAX_SIZE_BYTES);
    let registry_url = skills_cfg
        .and_then(|s| s.registry_url.clone())
        .unwrap_or_else(|| DEFAULT_REGISTRY_URL.to_string());
    let configured_skills_dir = config.skills_dir.as_ref().map(PathBuf::from);
    (network, max_size, registry_url, configured_skills_dir)
}

fn parse_api_scope(
    scope: Option<&str>,
) -> Result<Option<crate::skills::mutation::SkillTargetScope>, ApiError> {
    match scope {
        None => Ok(None),
        Some("project") => Ok(Some(crate::skills::mutation::SkillTargetScope::Project)),
        Some("global") => Ok(Some(crate::skills::mutation::SkillTargetScope::Global)),
        Some(other) => Err(ApiError::bad_request(format!(
            "invalid scope '{other}'; expected \"project\" or \"global\""
        ))),
    }
}

fn receipt_to_response(
    receipt: &crate::skills::mutation::SkillMutationReceipt,
) -> SkillMutationReceiptResponse {
    use crate::skills::mutation::SkillMutationOutcome;
    use crate::skills::roots::SkillScope;

    const TRUST_NOTE: &str = "The .trusted marker is advisory and digest-bound; \
         it records your review intent but does not sandbox or auto-authorize scripts.";

    let outcome: &'static str = match &receipt.outcome {
        SkillMutationOutcome::Installed => "installed",
        SkillMutationOutcome::Updated => "updated",
        SkillMutationOutcome::NoChange => "no_change",
        SkillMutationOutcome::Removed => "removed",
        SkillMutationOutcome::Trusted => "trusted",
        SkillMutationOutcome::Imported => "imported",
        SkillMutationOutcome::AlreadyPresent => "already_present",
        // NeedsApproval / NetworkDenied are returned as ApiError::forbidden
        // before reaching this conversion; they should not appear here.
        SkillMutationOutcome::NeedsApproval(_) => "needs_approval",
        SkillMutationOutcome::NetworkDenied(_) => "network_denied",
    };
    let scope = match receipt.scope {
        SkillScope::Project => "project".to_string(),
        SkillScope::Global => "global".to_string(),
        SkillScope::Logical => "logical".to_string(),
    };
    let trust_note = if receipt.outcome == SkillMutationOutcome::Trusted {
        Some(TRUST_NOTE)
    } else {
        None
    };
    SkillMutationReceiptResponse {
        name: receipt.name.clone(),
        outcome,
        scope,
        safe_target_path: receipt.safe_target_path.clone(),
        trust_note,
    }
}

fn outcome_is_policy_error(outcome: &crate::skills::mutation::SkillMutationOutcome) -> bool {
    matches!(
        outcome,
        crate::skills::mutation::SkillMutationOutcome::NeedsApproval(_)
            | crate::skills::mutation::SkillMutationOutcome::NetworkDenied(_)
    )
}

fn policy_error_message(outcome: &crate::skills::mutation::SkillMutationOutcome) -> String {
    match outcome {
        crate::skills::mutation::SkillMutationOutcome::NeedsApproval(host) => format!(
            "network access to '{host}' requires explicit approval; \
             approve the host in your network policy before installing this skill"
        ),
        crate::skills::mutation::SkillMutationOutcome::NetworkDenied(host) => {
            format!("network access to '{host}' was denied by the active network policy")
        }
        _ => "operation denied by policy".to_string(),
    }
}

// ─── POST /v1/skills/install ────────────────────────────────────────────────

async fn install_skill_api(
    State(state): State<RuntimeApiState>,
    Json(req): Json<InstallSkillRequest>,
) -> Result<(StatusCode, Json<SkillMutationReceiptResponse>), ApiError> {
    use crate::skills::install::InstallSource;
    use crate::skills::mutation::{MutationContext, SkillMutationRequest, SkillTargetScope};

    let source = InstallSource::parse(&req.source)
        .map_err(|err| ApiError::bad_request(format!("invalid install source: {err}")))?;
    let target = parse_api_scope(req.scope.as_deref())?.unwrap_or(SkillTargetScope::Global);

    let (network, max_size, registry_url, configured_skills_dir) =
        mutation_context_settings(&state);
    let home = crate::config::effective_home_dir();
    let workspace = state.workspace.clone();

    let receipt = crate::skills::mutation::execute(
        SkillMutationRequest::InstallRemote { source, target },
        &MutationContext {
            workspace: &workspace,
            home: home.as_deref(),
            configured_skills_dir: configured_skills_dir.as_deref(),
            network: &network,
            max_size,
            registry_url: &registry_url,
        },
    )
    .await
    .map_err(|err| ApiError::bad_request(format!("install failed: {err:#}")))?;

    if outcome_is_policy_error(&receipt.outcome) {
        return Err(ApiError::forbidden(policy_error_message(&receipt.outcome)));
    }

    let status = if receipt.outcome == crate::skills::mutation::SkillMutationOutcome::Installed {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(receipt_to_response(&receipt))))
}

// ─── POST /v1/skills/{name}/update ─────────────────────────────────────────

async fn update_skill_api(
    State(state): State<RuntimeApiState>,
    Path(name): Path<String>,
    Json(req): Json<UpdateSkillRequest>,
) -> Result<Json<SkillMutationReceiptResponse>, ApiError> {
    use crate::skills::mutation::{MutationContext, SkillMutationRequest};

    let scope = parse_api_scope(req.scope.as_deref())?;
    let (network, max_size, registry_url, configured_skills_dir) =
        mutation_context_settings(&state);
    let home = crate::config::effective_home_dir();
    let workspace = state.workspace.clone();

    let receipt = crate::skills::mutation::execute(
        SkillMutationRequest::UpdateByName {
            name: name.clone(),
            scope,
            expected_digest: req.expected_digest,
        },
        &MutationContext {
            workspace: &workspace,
            home: home.as_deref(),
            configured_skills_dir: configured_skills_dir.as_deref(),
            network: &network,
            max_size,
            registry_url: &registry_url,
        },
    )
    .await
    .map_err(|err| {
        let msg = err.to_string();
        if msg.contains("not found") {
            ApiError::not_found(format!("update failed: {err:#}"))
        } else {
            ApiError::bad_request(format!("update failed: {err:#}"))
        }
    })?;

    if outcome_is_policy_error(&receipt.outcome) {
        return Err(ApiError::forbidden(policy_error_message(&receipt.outcome)));
    }

    Ok(Json(receipt_to_response(&receipt)))
}

// ─── DELETE /v1/skills/{name} (uninstall) ──────────────────────────────────

async fn uninstall_skill_api(
    State(state): State<RuntimeApiState>,
    Path(name): Path<String>,
    Query(query): Query<UninstallSkillQuery>,
) -> Result<Json<SkillMutationReceiptResponse>, ApiError> {
    use crate::skills::mutation::{MutationContext, SkillMutationRequest};

    let scope = parse_api_scope(query.scope.as_deref())?;
    let (network, max_size, registry_url, configured_skills_dir) =
        mutation_context_settings(&state);
    let home = crate::config::effective_home_dir();

    let receipt = crate::skills::mutation::execute_sync(
        SkillMutationRequest::RemoveByName {
            name: name.clone(),
            scope,
            expected_digest: query.expected_digest,
        },
        &MutationContext {
            workspace: &state.workspace,
            home: home.as_deref(),
            configured_skills_dir: configured_skills_dir.as_deref(),
            network: &network,
            max_size,
            registry_url: &registry_url,
        },
    )
    .map_err(|err| {
        let msg = err.to_string();
        if msg.contains("not found") {
            ApiError::not_found(format!("uninstall failed: {err:#}"))
        } else {
            ApiError::bad_request(format!("uninstall failed: {err:#}"))
        }
    })?;

    Ok(Json(receipt_to_response(&receipt)))
}

// ─── POST /v1/skills/{name}/trust ──────────────────────────────────────────

async fn trust_skill_api(
    State(state): State<RuntimeApiState>,
    Path(name): Path<String>,
    Json(req): Json<TrustSkillRequest>,
) -> Result<Json<SkillMutationReceiptResponse>, ApiError> {
    use crate::skills::mutation::{MutationContext, SkillMutationRequest};

    let scope = parse_api_scope(req.scope.as_deref())?;
    let (network, max_size, registry_url, configured_skills_dir) =
        mutation_context_settings(&state);
    let home = crate::config::effective_home_dir();

    let receipt = crate::skills::mutation::execute_sync(
        SkillMutationRequest::TrustByName {
            name: name.clone(),
            scope,
            expected_digest: req.expected_digest,
        },
        &MutationContext {
            workspace: &state.workspace,
            home: home.as_deref(),
            configured_skills_dir: configured_skills_dir.as_deref(),
            network: &network,
            max_size,
            registry_url: &registry_url,
        },
    )
    .map_err(|err| {
        let msg = err.to_string();
        if msg.contains("not found") {
            ApiError::not_found(format!("trust failed: {err:#}"))
        } else {
            ApiError::bad_request(format!("trust failed: {err:#}"))
        }
    })?;

    Ok(Json(receipt_to_response(&receipt)))
}

// ─── GET /v1/skills/{name}/audit ───────────────────────────────────────────

async fn audit_skill_api(
    State(state): State<RuntimeApiState>,
    Path(name): Path<String>,
    Query(query): Query<SkillScopeQuery>,
) -> Result<Json<SkillAuditResponse>, ApiError> {
    use crate::skills::audit::{
        AuditedSkill, DigestState, IntegrityState, SkillActionKind, SkillAuditMode,
        SkillAuditWarning, SkillSourceKind, TrustState, scan_with_configured,
    };
    use crate::skills::roots::SkillRootKind;

    let scope_filter = parse_api_scope(query.scope.as_deref())?;
    let home = crate::config::effective_home_dir();
    let configured_skills_dir = {
        let config = state.config.read();
        config.skills_dir.as_ref().map(PathBuf::from)
    };
    let canonical = crate::skills::normalize_skill_name_for_lookup(&name);

    let snap = scan_with_configured(
        &state.workspace,
        home.as_deref(),
        configured_skills_dir.as_deref(),
        SkillAuditMode::Compatible,
        None,
    );

    let mut matches: Vec<&AuditedSkill> = snap
        .skills
        .iter()
        .filter(|s| s.id.canonical_name == canonical)
        .collect();

    if let Some(scope) = scope_filter {
        let want = match scope {
            crate::skills::mutation::SkillTargetScope::Project => SkillRootKind::CodeWhaleProject,
            crate::skills::mutation::SkillTargetScope::Global => SkillRootKind::CodeWhaleGlobal,
        };
        matches.retain(|s| s.root.kind == want);
    }

    if matches.is_empty() {
        return Err(ApiError::not_found(format!(
            "skill '{name}' not found in any audited root"
        )));
    }

    let ambiguous = matches.len() > 1;
    let entries = matches
        .into_iter()
        .map(|skill| {
            let source_kind = match skill.source_kind {
                SkillSourceKind::CodeWhaleManaged => "codewhale_managed",
                SkillSourceKind::CodeWhaleManual => "codewhale_manual",
                SkillSourceKind::CompatibleExternal => "compatible_external",
                SkillSourceKind::BuiltIn => "built_in",
                SkillSourceKind::ReviewedPluginSnapshot => "reviewed_plugin_snapshot",
                SkillSourceKind::RegistryCache => "registry_cache",
            };
            let scope_str = match skill.root.kind {
                SkillRootKind::CodeWhaleProject => "project",
                SkillRootKind::CodeWhaleGlobal => "global",
                _ => "other",
            };
            let digest = match &skill.digest {
                DigestState::Known(v) => SkillAuditDigest {
                    state: "known".to_string(),
                    value: Some(v.clone()),
                },
                DigestState::Unknown(reason) => SkillAuditDigest {
                    state: format!("unknown:{reason:?}").to_ascii_lowercase(),
                    value: None,
                },
            };
            let trust = match &skill.trust {
                TrustState::TrustedForDigest(_) => "trusted_for_digest",
                TrustState::TrustStale => "trust_stale",
                TrustState::LegacyAdvisory => "legacy_advisory",
                TrustState::Untrusted => "untrusted",
                TrustState::NotApplicable => "not_applicable",
                TrustState::Unknown => "unknown",
            };
            let integrity = match &skill.integrity {
                IntegrityState::Healthy => "healthy",
                IntegrityState::LocalContentDrift => "local_content_drift",
                IntegrityState::BrokenManagedInstall => "broken_managed_install",
                IntegrityState::LegacyMetadataUnknown => "legacy_metadata_unknown",
                IntegrityState::Unknown => "unknown",
            };
            let available_actions = skill
                .available_actions
                .iter()
                .map(|a| match a {
                    SkillActionKind::Install => "install",
                    SkillActionKind::Import => "import",
                    SkillActionKind::Update => "update",
                    SkillActionKind::Remove => "remove",
                    SkillActionKind::Trust => "trust",
                })
                .map(str::to_string)
                .collect();
            let warnings = skill
                .warnings
                .iter()
                .map(|w| match w {
                    SkillAuditWarning::Message(m) => m.clone(),
                })
                .collect();
            SkillAuditEntry {
                name: skill.name.clone(),
                safe_display_path: skill.safe_display_path.clone(),
                source_kind: source_kind.to_string(),
                scope: scope_str.to_string(),
                digest,
                trust: trust.to_string(),
                integrity: integrity.to_string(),
                available_actions,
                warnings,
            }
        })
        .collect();

    Ok(Json(SkillAuditResponse {
        ambiguous,
        skills: entries,
    }))
}

async fn decide_approval(
    State(state): State<RuntimeApiState>,
    Path(approval_id): Path<String>,
    Json(req): Json<DecideApprovalBody>,
) -> Result<Json<DecideApprovalResponse>, ApiError> {
    let decision = match req.decision.as_str() {
        "allow" => ExternalApprovalDecision::Allow {
            remember: req.remember,
        },
        "deny" => ExternalApprovalDecision::Deny {
            remember: req.remember,
        },
        other => {
            return Err(ApiError::bad_request(format!(
                "invalid decision '{other}'; expected \"allow\" or \"deny\""
            )));
        }
    };
    let delivered = state
        .runtime_threads
        .deliver_external_approval(&approval_id, decision);
    if !delivered {
        return Err(ApiError::not_found(format!(
            "no pending approval with id '{approval_id}'"
        )));
    }
    Ok(Json(DecideApprovalResponse {
        ok: true,
        approval_id,
        decision: req.decision,
        delivered,
    }))
}

async fn submit_user_input(
    State(state): State<RuntimeApiState>,
    Path((thread_id, input_id)): Path<(String, String)>,
    Json(req): Json<SubmitUserInputBody>,
) -> Result<Json<SubmitUserInputResponse>, ApiError> {
    use crate::tools::user_input::{UserInputAnswer, UserInputResponse};
    let answers: Vec<UserInputAnswer> = req
        .answers
        .into_iter()
        .map(|a| UserInputAnswer {
            id: a.id,
            label: a.label,
            value: a.value,
        })
        .collect();
    let response = UserInputResponse { answers };
    let delivered = state
        .runtime_threads
        .submit_user_input(&thread_id, &input_id, response)
        .await
        .map_err(map_thread_err)?;
    if !delivered {
        return Err(ApiError::not_found(format!(
            "no pending user-input request with id '{input_id}'"
        )));
    }
    Ok(Json(SubmitUserInputResponse {
        ok: true,
        input_id,
        delivered,
    }))
}

async fn runtime_info(
    State(state): State<RuntimeApiState>,
    request: Request,
) -> Json<RuntimeInfoResponse> {
    let version = env!("CARGO_PKG_VERSION");
    let commit = option_env!("CODEWHALE_BUILD_COMMIT").unwrap_or("unknown");
    let api_base = runtime_account_api_base();
    let account = runtime_account_info_for_request(
        runtime_request_is_authorized(&request, &state),
        &api_base,
        || runtime_account_info(state.config_profile.as_deref(), &api_base),
    );
    Json(RuntimeInfoResponse {
        service: "codewhale-runtime-api",
        runtime_api_version: RUNTIME_API_VERSION,
        codewhale_version: version,
        codewhale_commit: commit,
        bind_host: state.bind_host.clone(),
        port: state.bind_port,
        auth_required: state.auth_required,
        transports: vec!["http", "sse"],
        capabilities: default_runtime_capabilities(),
        account,
        experimental: RuntimeExperimentalCapabilities::default(),
        version,
    })
}

fn runtime_account_info(profile: Option<&str>, api_base: &str) -> RuntimeAccountInfo {
    #[cfg(test)]
    {
        let _ = profile;
        RuntimeAccountInfo::signed_out(api_base.to_string())
    }

    #[cfg(not(test))]
    {
        secure_account_session_secrets()
            .and_then(|secrets| {
                AccountSessionStore::new(secrets, profile, api_base).runtime_info_at(Utc::now())
            })
            .unwrap_or_else(|_| RuntimeAccountInfo::signed_out(api_base.to_string()))
    }
}

fn runtime_account_info_for_request(
    authorized: bool,
    api_base: &str,
    load: impl FnOnce() -> RuntimeAccountInfo,
) -> RuntimeAccountInfo {
    if authorized {
        load()
    } else {
        RuntimeAccountInfo::signed_out(api_base.to_string())
    }
}

fn runtime_account_api_base() -> String {
    std::env::var(ACCOUNT_API_BASE_ENV)
        .ok()
        .and_then(|value| normalize_runtime_account_api_base(&value))
        .unwrap_or_else(|| DEFAULT_ACCOUNT_API_BASE.to_string())
}

fn normalize_runtime_account_api_base(value: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(value.trim()).ok()?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return None;
    }
    let host = url.host_str()?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return None;
    }
    url.set_path("/");
    Some(url.as_str().trim_end_matches('/').to_string())
}

async fn list_mcp_servers(
    State(state): State<RuntimeApiState>,
) -> Result<Json<McpServersResponse>, ApiError> {
    let mcp_config_path = state.config.read().mcp_config_path();
    let plugin_registry = state
        .plugin_discovery
        .registry_for_workspace(&state.workspace);
    let config = crate::mcp::load_config_with_workspace_and_plugins(
        &mcp_config_path,
        &state.workspace,
        plugin_registry.as_ref(),
    )
    .map_err(|e| ApiError::internal(format!("Failed to load MCP config: {e}")))?;

    let mut servers = Vec::new();
    for (name, server_cfg) in config.servers {
        servers.push(McpServerEntry {
            name: name.clone(),
            enabled: server_cfg.is_enabled(),
            required: server_cfg.required,
            command: server_cfg.command.clone(),
            url: server_cfg.url.clone(),
            connected: false,
            enabled_tools: server_cfg.enabled_tools.clone(),
            disabled_tools: server_cfg.disabled_tools.clone(),
        });
    }
    servers.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Json(McpServersResponse { servers }))
}

async fn list_mcp_tools(
    State(state): State<RuntimeApiState>,
    Query(query): Query<McpToolsQuery>,
) -> Result<Json<McpToolsResponse>, ApiError> {
    // Double-checked init: hold the state-level slot mutex only long enough
    // to grab (or lazily create) the pool handle. connect_all can stall on a
    // slow MCP server and must not run under the slot lock.
    let pool_handle = {
        let mut pool_slot = state.mcp_pool.lock().await;
        match pool_slot.as_ref() {
            Some(pool) => Some(Arc::clone(pool)),
            None if query.connect => {
                let mcp_config_path = state.config.read().mcp_config_path();
                let plugin_registry = state
                    .plugin_discovery
                    .registry_for_workspace(&state.workspace);
                let new_pool = McpPool::from_config_path_with_workspace_and_plugins(
                    &mcp_config_path,
                    &state.workspace,
                    plugin_registry,
                )
                .map_err(|e| ApiError::internal(format!("Failed to load MCP config: {e}")))?;
                let handle = Arc::new(Mutex::new(new_pool));
                pool_slot.replace(Arc::clone(&handle));
                Some(handle)
            }
            None => None,
        }
    };

    let Some(pool_handle) = pool_handle else {
        return Ok(Json(McpToolsResponse { tools: Vec::new() }));
    };

    let mut pool = pool_handle.lock().await;
    if query.connect {
        let _errors = pool.connect_all().await;
    }

    let mut tools = Vec::new();
    for (prefixed_name, tool) in pool.all_tools() {
        let Ok((server, name)) = pool.parse_prefixed_name(&prefixed_name) else {
            continue;
        };

        if let Some(filter) = query.server.as_deref()
            && server != filter
        {
            continue;
        }

        tools.push(McpToolEntry {
            server: server.to_string(),
            name: name.to_string(),
            prefixed_name,
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
        });
    }

    tools.sort_by(|a, b| a.server.cmp(&b.server).then_with(|| a.name.cmp(&b.name)));

    Ok(Json(McpToolsResponse { tools }))
}

/// `GET /v1/apps/mcp/servers/{name}` — fetch a single server's redacted config.
async fn get_mcp_server(
    State(state): State<RuntimeApiState>,
    Path(name): Path<String>,
) -> Result<Json<McpServerDetail>, ApiError> {
    let mcp_config_path = state.config.read().mcp_config_path();
    let plugin_registry = state
        .plugin_discovery
        .registry_for_workspace(&state.workspace);
    let config = crate::mcp::load_config_with_workspace_and_plugins(
        &mcp_config_path,
        &state.workspace,
        plugin_registry.as_ref(),
    )
    .map_err(|e| ApiError::internal(format!("Failed to load MCP config: {e}")))?;

    let server_cfg = config
        .servers
        .get(&name)
        .ok_or_else(|| ApiError::not_found(format!("MCP server '{name}' not found")))?;

    let connected = {
        let pool_slot = state.mcp_pool.lock().await;
        pool_slot.as_ref().is_some_and(|pool_handle| {
            pool_handle
                .try_lock()
                .is_ok_and(|p| p.connected_servers().contains(&name.as_str()))
        })
    };

    Ok(Json(McpServerDetail::from_config(
        &name, server_cfg, connected,
    )))
}

/// `POST /v1/apps/mcp/servers` — add a new server to the persistent config.
///
/// Body: JSON object with all `McpServerWriteRequest` fields **plus** a
/// required top-level `"name"` string that will be the server key.
async fn create_mcp_server(
    State(state): State<RuntimeApiState>,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<McpServerDetail>), ApiError> {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_request("'name' is required"))?
        .to_string();

    if name.trim().is_empty() {
        return Err(ApiError::bad_request("'name' must not be empty"));
    }

    let req: McpServerWriteRequest = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("Invalid request body: {e}")))?;

    if req.command.as_ref().and_then(Option::as_ref).is_none()
        && req.url.as_ref().and_then(Option::as_ref).is_none()
    {
        return Err(ApiError::bad_request(
            "Either 'command' or 'url' is required to create an MCP server",
        ));
    }

    if let Some(Some(transport)) = &req.transport {
        crate::mcp::validate_mcp_transport(Some(transport.as_str()))
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
    }

    let mcp_config_path = state.config.read().mcp_config_path();

    // Build the config entry from the request.
    let new_cfg = mcp_server_config_from_write_request(req, None);

    // Persist to the global MCP config.
    {
        let mut cfg = crate::mcp::load_config(&mcp_config_path)
            .map_err(|e| ApiError::internal(format!("Failed to load MCP config: {e}")))?;
        if cfg.servers.contains_key(&name) {
            return Err(ApiError {
                status: StatusCode::CONFLICT,
                message: format!("MCP server '{name}' already exists"),
            });
        }
        cfg.servers.insert(name.clone(), new_cfg.clone());
        crate::mcp::save_config(&mcp_config_path, &cfg)
            .map_err(|e| ApiError::internal(format!("Failed to save MCP config: {e}")))?;
    }

    // Invalidate the in-memory pool so the next tool call reloads from disk.
    {
        let mut pool_slot = state.mcp_pool.lock().await;
        *pool_slot = None;
    }

    Ok((
        StatusCode::CREATED,
        Json(McpServerDetail::from_config(&name, &new_cfg, false)),
    ))
}

/// `PATCH /v1/apps/mcp/servers/{name}` — update an existing server's config.
async fn update_mcp_server(
    State(state): State<RuntimeApiState>,
    Path(name): Path<String>,
    Json(req): Json<McpServerWriteRequest>,
) -> Result<Json<McpServerDetail>, ApiError> {
    if let Some(Some(transport)) = &req.transport {
        crate::mcp::validate_mcp_transport(Some(transport.as_str()))
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
    }

    let mcp_config_path = state.config.read().mcp_config_path();

    let updated_cfg = {
        let mut cfg = crate::mcp::load_config(&mcp_config_path)
            .map_err(|e| ApiError::internal(format!("Failed to load MCP config: {e}")))?;
        let existing = cfg
            .servers
            .get_mut(&name)
            .ok_or_else(|| ApiError::not_found(format!("MCP server '{name}' not found")))?;
        apply_write_request_to_config(req, existing);
        if existing.command.is_none() && existing.url.is_none() {
            return Err(ApiError::bad_request(
                "Either 'command' or 'url' must remain configured for an MCP server",
            ));
        }
        let updated = existing.clone();
        crate::mcp::save_config(&mcp_config_path, &cfg)
            .map_err(|e| ApiError::internal(format!("Failed to save MCP config: {e}")))?;
        updated
    };

    // Invalidate the in-memory pool.
    {
        let mut pool_slot = state.mcp_pool.lock().await;
        *pool_slot = None;
    }

    Ok(Json(McpServerDetail::from_config(
        &name,
        &updated_cfg,
        false,
    )))
}

/// `DELETE /v1/apps/mcp/servers/{name}` — remove a server from the persistent config.
async fn delete_mcp_server(
    State(state): State<RuntimeApiState>,
    Path(name): Path<String>,
) -> Result<Json<McpServerActionReceipt>, ApiError> {
    let mcp_config_path = state.config.read().mcp_config_path();

    crate::mcp::remove_server_config(&mcp_config_path, &name).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("not found") {
            ApiError::not_found(msg)
        } else {
            ApiError::internal(msg)
        }
    })?;

    // Invalidate the in-memory pool.
    {
        let mut pool_slot = state.mcp_pool.lock().await;
        *pool_slot = None;
    }

    Ok(Json(McpServerActionReceipt {
        name,
        action: "deleted",
        ok: true,
    }))
}

/// `POST /v1/apps/mcp/servers/{name}/enable` — enable a configured server.
async fn enable_mcp_server(
    State(state): State<RuntimeApiState>,
    Path(name): Path<String>,
) -> Result<Json<McpServerActionReceipt>, ApiError> {
    let mcp_config_path = state.config.read().mcp_config_path();

    crate::mcp::set_server_enabled(&mcp_config_path, &name, true).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("not found") {
            ApiError::not_found(msg)
        } else {
            ApiError::internal(msg)
        }
    })?;

    // Invalidate the in-memory pool so the enabled server participates next time.
    {
        let mut pool_slot = state.mcp_pool.lock().await;
        *pool_slot = None;
    }

    Ok(Json(McpServerActionReceipt {
        name,
        action: "enabled",
        ok: true,
    }))
}

/// `POST /v1/apps/mcp/servers/{name}/disable` — disable a configured server.
async fn disable_mcp_server(
    State(state): State<RuntimeApiState>,
    Path(name): Path<String>,
) -> Result<Json<McpServerActionReceipt>, ApiError> {
    let mcp_config_path = state.config.read().mcp_config_path();

    crate::mcp::set_server_enabled(&mcp_config_path, &name, false).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("not found") {
            ApiError::not_found(msg)
        } else {
            ApiError::internal(msg)
        }
    })?;

    // Invalidate the in-memory pool so the disabled server is excluded next time.
    {
        let mut pool_slot = state.mcp_pool.lock().await;
        *pool_slot = None;
    }

    Ok(Json(McpServerActionReceipt {
        name,
        action: "disabled",
        ok: true,
    }))
}

/// `POST /v1/apps/mcp/servers/{name}/reconnect` — drop the cached pool entry
/// for this server so it re-initializes on the next call that needs tools.
async fn reconnect_mcp_server(
    State(state): State<RuntimeApiState>,
    Path(name): Path<String>,
) -> Result<Json<McpServerActionReceipt>, ApiError> {
    // Verify the server exists in the config.
    let mcp_config_path = state.config.read().mcp_config_path();
    let plugin_registry = state
        .plugin_discovery
        .registry_for_workspace(&state.workspace);
    let config = crate::mcp::load_config_with_workspace_and_plugins(
        &mcp_config_path,
        &state.workspace,
        plugin_registry.as_ref(),
    )
    .map_err(|e| ApiError::internal(format!("Failed to load MCP config: {e}")))?;

    if !config.servers.contains_key(&name) {
        return Err(ApiError::not_found(format!(
            "MCP server '{name}' not found"
        )));
    }

    // Drop the whole pool so the next connect_all call recreates all
    // connections from the current on-disk config.
    {
        let mut pool_slot = state.mcp_pool.lock().await;
        *pool_slot = None;
    }

    Ok(Json(McpServerActionReceipt {
        name,
        action: "reconnect_scheduled",
        ok: true,
    }))
}

/// Build a fresh [`McpServerConfig`] from a create request.
fn mcp_server_config_from_write_request(
    req: McpServerWriteRequest,
    _existing: Option<&crate::mcp::McpServerConfig>,
) -> crate::mcp::McpServerConfig {
    let enabled = req.enabled.unwrap_or(true);
    crate::mcp::McpServerConfig {
        command: req.command.flatten(),
        args: req.args.unwrap_or_default(),
        env: req.env.unwrap_or_default(),
        cwd: None,
        url: req.url.flatten(),
        transport: req.transport.flatten(),
        connect_timeout: req.connect_timeout.flatten(),
        execute_timeout: req.execute_timeout.flatten(),
        read_timeout: req.read_timeout.flatten(),
        disabled: !enabled,
        enabled,
        required: req.required.unwrap_or(false),
        enabled_tools: req.enabled_tools.unwrap_or_default(),
        disabled_tools: req.disabled_tools.unwrap_or_default(),
        headers: std::collections::HashMap::new(),
        env_headers: req.env_headers.unwrap_or_default(),
        bearer_token_env_var: req.bearer_token_env_var.flatten(),
        scopes: req.scopes.unwrap_or_default(),
        oauth: None,
        oauth_resource: req.oauth_resource.flatten(),
        reviewed_plugin: None,
    }
}

/// Apply a partial update from a PATCH request onto an existing config entry.
fn apply_write_request_to_config(
    req: McpServerWriteRequest,
    cfg: &mut crate::mcp::McpServerConfig,
) {
    if let Some(v) = req.command {
        cfg.command = v;
    }
    if let Some(v) = req.args {
        cfg.args = v;
    }
    if let Some(v) = req.env {
        cfg.env = v;
    }
    if let Some(v) = req.url {
        cfg.url = v;
    }
    if let Some(v) = req.transport {
        cfg.transport = v;
    }
    if let Some(v) = req.connect_timeout {
        cfg.connect_timeout = v;
    }
    if let Some(v) = req.execute_timeout {
        cfg.execute_timeout = v;
    }
    if let Some(v) = req.read_timeout {
        cfg.read_timeout = v;
    }
    if let Some(v) = req.enabled {
        cfg.enabled = v;
        cfg.disabled = !v;
    }
    if let Some(v) = req.required {
        cfg.required = v;
    }
    if let Some(v) = req.enabled_tools {
        cfg.enabled_tools = v;
    }
    if let Some(v) = req.disabled_tools {
        cfg.disabled_tools = v;
    }
    if let Some(v) = req.env_headers {
        cfg.env_headers = v;
    }
    if let Some(v) = req.bearer_token_env_var {
        cfg.bearer_token_env_var = v;
    }
    if let Some(v) = req.scopes {
        cfg.scopes = v;
    }
    if let Some(v) = req.oauth_resource {
        cfg.oauth_resource = v;
    }
}

async fn list_automations(
    State(state): State<RuntimeApiState>,
) -> Result<Json<Vec<AutomationRecord>>, ApiError> {
    let manager = state.automations.lock().await;
    let automations = manager
        .list_automations()
        .map_err(|e| ApiError::internal(format!("Failed to list automations: {e}")))?;
    Ok(Json(automations))
}

async fn create_automation(
    State(state): State<RuntimeApiState>,
    Json(req): Json<CreateAutomationRequest>,
) -> Result<(StatusCode, Json<AutomationRecord>), ApiError> {
    let manager = state.automations.lock().await;
    let automation = manager
        .create_automation(req)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok((StatusCode::CREATED, Json(automation)))
}

async fn get_automation(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<Json<AutomationRecord>, ApiError> {
    let manager = state.automations.lock().await;
    let automation = manager.get_automation(&id).map_err(map_automation_err)?;
    Ok(Json(automation))
}

async fn update_automation(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateAutomationRequest>,
) -> Result<Json<AutomationRecord>, ApiError> {
    let manager = state.automations.lock().await;
    let automation = manager
        .update_automation(&id, req)
        .map_err(map_automation_err)?;
    Ok(Json(automation))
}

async fn delete_automation(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<Json<AutomationRecord>, ApiError> {
    let manager = state.automations.lock().await;
    let automation = manager.delete_automation(&id).map_err(map_automation_err)?;
    Ok(Json(automation))
}

async fn run_automation(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<Json<AutomationRunRecord>, ApiError> {
    // run_now_shared drops the manager mutex across the task-manager await so
    // other automation endpoints stay responsive behind a slow enqueue.
    let run =
        crate::automation_manager::run_now_shared(&state.automations, &id, &state.task_manager)
            .await
            .map_err(map_automation_err)?;
    Ok(Json(run))
}

async fn pause_automation(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<Json<AutomationRecord>, ApiError> {
    let manager = state.automations.lock().await;
    let automation = manager.pause_automation(&id).map_err(map_automation_err)?;
    Ok(Json(automation))
}

async fn resume_automation(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<Json<AutomationRecord>, ApiError> {
    let manager = state.automations.lock().await;
    let automation = manager.resume_automation(&id).map_err(map_automation_err)?;
    Ok(Json(automation))
}

async fn list_automation_runs(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
    Query(query): Query<AutomationRunsQuery>,
) -> Result<Json<Vec<AutomationRunRecord>>, ApiError> {
    let manager = state.automations.lock().await;
    let runs = manager
        .list_runs(&id, query.limit)
        .map_err(map_automation_err)?;
    Ok(Json(runs))
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct StartOperateRequest {
    #[serde(default)]
    direction: Option<String>,
    /// CWC `OperateBurnRate` object, positive $/hr number, or null (unbounded).
    #[serde(default)]
    burn_rate: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct KeepAliveOperateRequest {
    #[serde(default)]
    spent_usd: Option<f64>,
    #[serde(default)]
    observed_burn_usd_per_hour: Option<f64>,
    #[serde(default)]
    credentials_present: Option<bool>,
    #[serde(default)]
    human_gated: Option<bool>,
}

#[derive(Debug, Serialize)]
struct OperateView {
    /// `None` until an operation is actually started — a GET before that
    /// must not fabricate an identity the client can never mutate.
    operation: Option<crate::operate::Operation>,
    board: String,
}

fn operate_store() -> Result<crate::operate::OperationStore, ApiError> {
    crate::operate::OperationStore::open(crate::operate::default_operate_dir())
        .map_err(|e| ApiError::internal(format!("Failed to open operate store: {e}")))
}

fn operate_credentials_present(config: &Config) -> bool {
    crate::operate::operate_credentials_present(config)
}

fn operate_view(operation: crate::operate::Operation) -> Json<OperateView> {
    Json(OperateView {
        board: crate::operate::render_plan_board(&operation),
        operation: Some(operation),
    })
}

fn load_operate(
    store: &crate::operate::OperationStore,
) -> Result<Option<crate::operate::Operation>, ApiError> {
    store
        .load()
        .map_err(|e| ApiError::internal(format!("Failed to load operate: {e}")))
}

fn parse_request_burn_rate(value: Option<&serde_json::Value>) -> Result<Option<f64>, ApiError> {
    Ok(crate::operate::parse_burn_rate(value)
        .map_err(|e| ApiError::bad_request(e.to_string()))?
        .map(|rate| rate.amount_usd_per_hour))
}

async fn get_operate(State(_state): State<RuntimeApiState>) -> Result<Json<OperateView>, ApiError> {
    let store = operate_store()?;
    match load_operate(&store)? {
        Some(operation) => Ok(operate_view(operation)),
        // No operation has been started: a fabricated `Operation::new` would
        // mint a fresh id and timestamps on every poll — phantom records the
        // client can neither patch nor cancel. `operation: null` is the
        // stable no-operation answer.
        None => Ok(Json(OperateView {
            operation: None,
            board: String::new(),
        })),
    }
}

async fn start_operate(
    State(state): State<RuntimeApiState>,
    Json(req): Json<StartOperateRequest>,
) -> Result<Json<OperateView>, ApiError> {
    let store = operate_store()?;
    let burn = parse_request_burn_rate(req.burn_rate.as_ref())?;
    // Keepalive first: a persisted operation without its keepalive is not
    // always-on, and a fresh operation has no lead plan yet — kick the first
    // lead run to the next scheduler tick instead of waiting out the hourly
    // recurrence.
    {
        let manager = state.automations.lock().await;
        crate::operate::upsert_keepalive(&manager, &state.workspace, true)
            .map_err(|e| ApiError::internal(format!("Failed to keep operate alive: {e}")))?;
    }
    let config = state.config.read();
    let operation = crate::operate::start_operation(
        &store,
        &state.workspace,
        req.direction,
        burn,
        operate_credentials_present(&config),
    )
    .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(operate_view(operation))
}

async fn patch_operate(
    State(state): State<RuntimeApiState>,
    Json(patch): Json<serde_json::Value>,
) -> Result<Json<OperateView>, ApiError> {
    let store = operate_store()?;
    let credentials = {
        let config = state.config.read();
        operate_credentials_present(&config)
    };
    // Read-merge-write under the operate store lock: a concurrent keepalive
    // or plan save can no longer be lost by a stale read.
    let direction_changed = std::cell::Cell::new(false);
    let operation = store
        .mutate(|op| {
            let before = op.direction.clone();
            crate::operate::apply_operate_patch(op, &patch)?;
            direction_changed.set(op.direction != before);
            op.credentials_present = credentials;
            op.project();
            Ok(())
        })
        .map_err(|e| {
            if e.to_string().contains("cancelled") {
                ApiError::conflict(e.to_string())
            } else {
                ApiError::bad_request(e.to_string())
            }
        })?
        .ok_or_else(|| ApiError::not_found("Unknown Operation."))?;
    // A changed direction invalidated the lead plan; pull the keepalive lead
    // run forward so the operation does not idle until the next recurrence.
    if direction_changed.get() {
        let manager = state.automations.lock().await;
        crate::operate::kick_keepalive(&manager)
            .map_err(|e| ApiError::internal(format!("Failed to reschedule operate: {e}")))?;
    }
    Ok(operate_view(operation))
}

async fn keepalive_operate(
    State(state): State<RuntimeApiState>,
    Json(req): Json<KeepAliveOperateRequest>,
) -> Result<Json<OperateView>, ApiError> {
    let store = operate_store()?;
    let config = state.config.read();
    let credentials = req
        .credentials_present
        .unwrap_or_else(|| operate_credentials_present(&config));
    drop(config);
    let operation = store
        .mutate(|op| {
            crate::operate::keep_alive_observation(
                op,
                req.observed_burn_usd_per_hour,
                req.spent_usd,
                Some(credentials),
                req.human_gated,
            );
            Ok(())
        })
        .map_err(|e| ApiError::internal(format!("Failed to keep operate alive: {e}")))?
        .ok_or_else(|| ApiError::not_found("Unknown Operation."))?;
    Ok(operate_view(operation))
}

async fn put_operate_plan(
    Json(plan): Json<serde_json::Value>,
) -> Result<Json<OperateView>, ApiError> {
    let store = operate_store()?;
    let patch = serde_json::json!({ "leadPlan": plan });
    let operation = store
        .mutate(|op| crate::operate::apply_operate_patch(op, &patch))
        .map_err(|e| {
            if e.to_string().contains("cancelled") {
                ApiError::conflict(e.to_string())
            } else if e.to_string().contains("leadPlan") {
                ApiError::bad_request(e.to_string())
            } else {
                ApiError::internal(format!("Failed to save operate plan: {e}"))
            }
        })?
        .ok_or_else(|| ApiError::not_found("Unknown Operation."))?;
    Ok(operate_view(operation))
}

async fn cancel_operate(
    State(state): State<RuntimeApiState>,
) -> Result<Json<OperateView>, ApiError> {
    let store = operate_store()?;
    let operation = crate::operate::cancel_operation(&store)
        .map_err(|e| ApiError::internal(format!("Failed to cancel operate: {e}")))?
        .ok_or_else(|| ApiError::not_found("Unknown Operation."))?;
    // Cancel tears down the keepalive too: an unattended hourly lead run
    // after cancel is pure cost.
    {
        let manager = state.automations.lock().await;
        crate::operate::pause_keepalive(&manager)
            .map_err(|e| ApiError::internal(format!("Failed to pause operate keepalive: {e}")))?;
    }
    Ok(operate_view(operation))
}

#[derive(Debug, Deserialize)]
struct OperateAutoMergeCheckRequest {
    repo: String,
    pr: String,
    agent: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OperateAutoMergeCheckView {
    allow: bool,
    reason: Option<String>,
    checker: Option<String>,
    check_args: Vec<String>,
    merge_args: Vec<String>,
}

async fn check_operate_auto_merge(
    State(state): State<RuntimeApiState>,
    Json(req): Json<OperateAutoMergeCheckRequest>,
) -> Result<Json<OperateAutoMergeCheckView>, ApiError> {
    let checker = crate::operate::discover_auto_merge_checker(&state.workspace);
    let repo = req.repo.clone();
    let pr = req.pr.clone();
    let agent = req.agent.clone();
    let checker_for_task = checker.clone();
    // The checker shells out synchronously (`python3 …; .status()`); run it on
    // the blocking pool so a slow `gh`/network wait cannot pin a Tokio worker.
    let decision = tokio::task::spawn_blocking(move || {
        crate::operate::evaluate_auto_merge(
            crate::operate::AutoMergeRequest {
                repo: &repo,
                pr: &pr,
                role: &agent,
            },
            checker_for_task.as_deref(),
        )
    })
    .await
    .map_err(|e| ApiError::internal(format!("auto-merge check join failed: {e}")))?;
    let (allow, reason) = match decision {
        crate::operate::AutoMergeDecision::Allow => (true, None),
        crate::operate::AutoMergeDecision::Deny { reason } => (false, Some(reason)),
    };
    Ok(Json(OperateAutoMergeCheckView {
        allow,
        reason,
        checker: checker.as_ref().map(|path| path.display().to_string()),
        check_args: crate::operate::check_auto_merge_args(&req.repo, &req.pr, &req.agent),
        merge_args: crate::operate::auto_merge_pr_args(&req.repo, &req.pr, &req.agent),
    }))
}

async fn get_thread(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<Json<ThreadDetail>, ApiError> {
    let detail = state
        .runtime_threads
        .get_thread_detail(&id)
        .await
        .map_err(map_thread_err)?;
    Ok(Json(detail))
}

/// Response for `GET /v1/threads/{id}/usage`.
///
/// Thin adapter over `RuntimeThreadManager::aggregate_usage_for_thread`: the
/// GUI's session-cost surface reads provider-aware, recorded-time pricing in
/// both published currencies from the same accumulation that powers
/// `/v1/usage`, instead of reimplementing rate tables client-side.
#[derive(Debug, Serialize)]
struct ThreadUsageResponse {
    thread_id: String,
    totals: UsageTotals,
}

async fn get_thread_usage(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<Json<ThreadUsageResponse>, ApiError> {
    let totals = state
        .runtime_threads
        .aggregate_usage_for_thread(&id)
        .await
        .map_err(map_thread_err)?
        .combined();
    Ok(Json(ThreadUsageResponse {
        thread_id: id,
        totals,
    }))
}

async fn update_thread(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateThreadRequest>,
) -> Result<Json<ThreadRecord>, ApiError> {
    let thread = state
        .runtime_threads
        .update_thread(&id, req)
        .await
        .map_err(map_thread_err)?;
    Ok(Json(thread))
}

async fn resume_thread(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<Json<ThreadRecord>, ApiError> {
    let thread = state
        .runtime_threads
        .resume_thread(&id)
        .await
        .map_err(map_thread_err)?;
    Ok(Json(thread))
}

async fn fork_thread(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<ThreadRecord>), ApiError> {
    let thread = state
        .runtime_threads
        .fork_thread(&id)
        .await
        .map_err(map_thread_err)?;
    Ok((StatusCode::CREATED, Json(thread)))
}

#[derive(Debug, Deserialize)]
struct UndoTurnRequest {
    /// How many turns back to undo (default 0 = last turn only).
    #[serde(default)]
    depth: Option<usize>,
}

#[derive(Debug, Serialize)]
struct UndoTurnResponse {
    /// The new forked thread (with the last N turns removed).
    thread: ThreadRecord,
    /// The original user message text from the first dropped turn,
    /// so the GUI can pre-populate the input box.
    original_user_text: Option<String>,
}

async fn undo_thread_turn(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
    Json(req): Json<UndoTurnRequest>,
) -> Result<(StatusCode, Json<UndoTurnResponse>), ApiError> {
    let depth = req.depth.unwrap_or(0);
    let (forked_thread, original_user_text) = state
        .runtime_threads
        .fork_at_user_message(&id, depth)
        .await
        .map_err(map_thread_err)?;
    Ok((
        StatusCode::CREATED,
        Json(UndoTurnResponse {
            thread: forked_thread,
            original_user_text,
        }),
    ))
}

/// Result of the snapshot-based file rollback step of patch-undo, reported
/// alongside the new forked thread.
#[derive(Debug, Serialize)]
struct PatchUndoResult {
    /// Whether files were restored from a snapshot.
    files_restored: bool,
    /// Human-readable summary of what was restored (diff stat).
    summary: Option<String>,
    /// The label of the restored snapshot (e.g. "tool:apply_patch" or "pre-turn:3").
    snapshot_label: Option<String>,
}

#[derive(Debug, Serialize)]
struct PatchUndoResponse {
    /// Result of the snapshot-based file rollback step.
    patch_result: PatchUndoResult,
    /// The new forked thread (with the last turn removed).
    thread: ThreadRecord,
    /// The original user text from the removed turn (for re-editing).
    original_user_text: Option<String>,
}

async fn patch_undo_thread_turn(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
    Json(req): Json<UndoTurnRequest>,
) -> Result<(StatusCode, Json<PatchUndoResponse>), ApiError> {
    let depth = req.depth.unwrap_or(0);

    // Step 1: Try snapshot-based file rollback (patch_undo).
    let thread = state
        .runtime_threads
        .get_thread(&id)
        .await
        .map_err(map_thread_err)?;
    let patch_result = patch_undo_workspace_files(&thread.workspace, thread.session_id.as_deref());

    // Step 2: Remove the last conversation turn (undo_conversation).
    let (forked_thread, original_user_text) = state
        .runtime_threads
        .fork_at_user_message(&id, depth)
        .await
        .map_err(map_thread_err)?;

    Ok((
        StatusCode::CREATED,
        Json(PatchUndoResponse {
            patch_result,
            thread: forked_thread,
            original_user_text,
        }),
    ))
}

/// Restore the newest `tool:` or `pre-turn:` snapshot that differs from the
/// current workspace — same target selection as the TUI's `patch_undo`.
fn patch_undo_workspace_files(
    workspace: &FsPath,
    current_session_id: Option<&str>,
) -> PatchUndoResult {
    let repo = match crate::snapshot::SnapshotRepo::open_or_init(workspace) {
        Ok(repo) => repo,
        Err(e) => {
            return PatchUndoResult {
                files_restored: false,
                summary: Some(format!("Snapshot repo unavailable: {e}")),
                snapshot_label: None,
            };
        }
    };
    let Some(current_session_id) = current_session_id else {
        return PatchUndoResult {
            files_restored: false,
            summary: Some(
                "No current session is bound to this thread; workspace files were not changed."
                    .to_string(),
            ),
            snapshot_label: None,
        };
    };
    let snapshots = match repo.list(100) {
        Ok(snapshots) => snapshots,
        Err(e) => {
            return PatchUndoResult {
                files_restored: false,
                summary: Some(format!("Failed to list snapshots: {e}")),
                snapshot_label: None,
            };
        }
    };
    let target = snapshots
        .iter()
        .filter(|s| s.label.starts_with("tool:") || s.label.starts_with("pre-turn:"))
        .filter(|s| s.session_id.as_deref() == Some(current_session_id))
        .find(|s| matches!(repo.work_tree_matches_snapshot(&s.id), Ok(false)));
    let Some(target) = target else {
        return PatchUndoResult {
            files_restored: false,
            summary: Some(
                "No current-session tool or pre-turn snapshots differ from the current workspace."
                    .to_string(),
            ),
            snapshot_label: None,
        };
    };
    if let Err(e) = repo.restore(&target.id) {
        return PatchUndoResult {
            files_restored: false,
            summary: Some(format!("Restore failed: {e}")),
            snapshot_label: None,
        };
    }

    // Compute a diff stat for the summary.
    use crate::dependencies::{ExternalTool as _, Git};
    let diff_stat = Git::command().and_then(|mut git| {
        git.args(["diff", "--stat"])
            .current_dir(workspace)
            .output()
            .ok()
            .and_then(|o| {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if s.is_empty() { None } else { Some(s) }
            })
    });

    let short = &target.id.as_str()[..target.id.as_str().len().min(8)];
    let summary = match diff_stat {
        Some(ref stat) => format!(
            "Restored snapshot '{}' ({}). Files affected:\n{stat}",
            target.label, short
        ),
        None => format!(
            "Restored snapshot '{}' ({}). No diff changes detected.",
            target.label, short
        ),
    };
    PatchUndoResult {
        files_restored: true,
        summary: Some(summary),
        snapshot_label: Some(target.label.clone()),
    }
}

#[derive(Debug, Deserialize)]
struct RetryTurnRequest {
    /// How many turns back to retry (default 0 = last turn only).
    #[serde(default)]
    depth: Option<usize>,
    /// Override the user message text. If omitted, the original text
    /// from the dropped turn is re-used.
    #[serde(default)]
    prompt: Option<String>,
}

#[derive(Debug, Serialize)]
struct RetryTurnResponse {
    /// The new forked thread (with the last N turns removed).
    thread: ThreadRecord,
    /// The turn created by the retry.
    turn: TurnRecord,
}

async fn retry_thread_turn(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
    Json(req): Json<RetryTurnRequest>,
) -> Result<(StatusCode, Json<RetryTurnResponse>), ApiError> {
    let depth = req.depth.unwrap_or(0);
    let (forked_thread, original_user_text) = state
        .runtime_threads
        .fork_at_user_message(&id, depth)
        .await
        .map_err(map_thread_err)?;

    let retry_prompt = req.prompt.or(original_user_text).unwrap_or_default();
    if retry_prompt.trim().is_empty() {
        return Err(ApiError::bad_request(
            "No user message to retry — the dropped turn had no user text",
        ));
    }

    let turn = state
        .runtime_threads
        .start_turn(
            &forked_thread.id,
            StartTurnRequest {
                prompt: retry_prompt,
                operation_key: None,
                input_summary: None,
                model: None,
                reasoning_effort: None,
                allowed_tools: None,
                mode: None,
                permission_posture: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                dynamic_tools: Vec::new(),
                environment_id: None,
            },
        )
        .await
        .map_err(map_thread_err)?;

    Ok((
        StatusCode::CREATED,
        Json(RetryTurnResponse {
            thread: forked_thread,
            turn,
        }),
    ))
}

async fn start_thread_turn(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
    Json(req): Json<StartTurnRequest>,
) -> Result<(StatusCode, Json<StartTurnResponse>), ApiError> {
    let turn = state
        .runtime_threads
        .start_turn(&id, req)
        .await
        .map_err(map_thread_err)?;
    let thread = state
        .runtime_threads
        .get_thread(&id)
        .await
        .map_err(map_thread_err)?;
    Ok((
        StatusCode::CREATED,
        Json(StartTurnResponse { thread, turn }),
    ))
}

#[derive(Debug, Serialize)]
struct AgentMailDeliveryResponse {
    envelope: AgentMailEnvelope,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn: Option<TurnRecord>,
}

async fn send_agent_mail(
    State(state): State<RuntimeApiState>,
    Json(request): Json<AgentMailSendRequest>,
) -> Result<(StatusCode, Json<AgentMailSendResponse>), ApiError> {
    let mut response = state
        .runtime_threads
        .queue_agent_mail(request)
        .await
        .map_err(map_agent_mail_err)?;
    if response.envelope.delivery_mode == AgentMailDeliveryMode::WakeAtSafeBoundary
        && response.envelope.trigger_turn
    {
        let (envelope, _) = state
            .runtime_threads
            .deliver_agent_mail(
                &response.envelope.destination.thread_id,
                &response.envelope.message_id,
            )
            .await
            .map_err(map_agent_mail_err)?;
        response.envelope = envelope;
    }
    let status = if response.idempotent_replay {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(response)))
}

async fn list_agent_mail(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<AgentMailEnvelope>>, ApiError> {
    let inbox = state
        .runtime_threads
        .list_agent_mail_for_thread(&id)
        .await
        .map_err(map_agent_mail_err)?;
    Ok(Json(inbox))
}

async fn deliver_agent_mail(
    State(state): State<RuntimeApiState>,
    Path((id, message_id)): Path<(String, String)>,
) -> Result<Json<AgentMailDeliveryResponse>, ApiError> {
    let message_id = AgentMailMessageId::parse(message_id)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let (envelope, turn) = state
        .runtime_threads
        .deliver_agent_mail(&id, &message_id)
        .await
        .map_err(map_agent_mail_err)?;
    Ok(Json(AgentMailDeliveryResponse { envelope, turn }))
}

async fn mark_agent_mail_read(
    State(state): State<RuntimeApiState>,
    Path((id, message_id)): Path<(String, String)>,
) -> Result<Json<AgentMailEnvelope>, ApiError> {
    let message_id = AgentMailMessageId::parse(message_id)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let envelope = state
        .runtime_threads
        .mark_agent_mail_read(&id, &message_id)
        .await
        .map_err(map_agent_mail_err)?;
    Ok(Json(envelope))
}

async fn steer_thread_turn(
    State(state): State<RuntimeApiState>,
    Path((id, turn_id)): Path<(String, String)>,
    Json(req): Json<SteerTurnRequest>,
) -> Result<Json<TurnRecord>, ApiError> {
    let turn = state
        .runtime_threads
        .steer_turn(&id, &turn_id, req)
        .await
        .map_err(map_thread_err)?;
    Ok(Json(turn))
}

async fn interrupt_thread_turn(
    State(state): State<RuntimeApiState>,
    Path((id, turn_id)): Path<(String, String)>,
) -> Result<Json<TurnRecord>, ApiError> {
    let turn = state
        .runtime_threads
        .interrupt_turn(&id, &turn_id)
        .await
        .map_err(map_thread_err)?;
    Ok(Json(turn))
}

async fn deliver_dynamic_tool_result(
    State(state): State<RuntimeApiState>,
    Path((id, turn_id, call_id)): Path<(String, String, String)>,
    Json(result): Json<DynamicToolCallResult>,
) -> Result<StatusCode, ApiError> {
    state
        .runtime_threads
        .get_thread(&id)
        .await
        .map_err(map_thread_err)?;
    if state
        .runtime_threads
        .deliver_dynamic_tool_result(&id, &turn_id, &call_id, result)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?
    {
        Ok(StatusCode::ACCEPTED)
    } else {
        Err(ApiError::not_found(format!(
            "No pending dynamic tool call '{call_id}'"
        )))
    }
}

async fn compact_thread(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
    Json(req): Json<CompactThreadRequest>,
) -> Result<(StatusCode, Json<StartTurnResponse>), ApiError> {
    let turn = state
        .runtime_threads
        .compact_thread(&id, req)
        .await
        .map_err(map_thread_err)?;
    let thread = state
        .runtime_threads
        .get_thread(&id)
        .await
        .map_err(map_thread_err)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(StartTurnResponse { thread, turn }),
    ))
}

// ---------------------------------------------------------------------------
// Thread goal endpoints
// ---------------------------------------------------------------------------

/// `GET /v1/threads/{id}/goal` — return the persistent goal for a thread, or
/// 404 if the thread has no goal.
async fn get_thread_goal(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<Json<codewhale_protocol::ThreadGoal>, ApiError> {
    // Verify the thread exists so we can return a clean 404 for unknown threads.
    state
        .runtime_threads
        .get_thread(&id)
        .await
        .map_err(map_thread_err)?;
    let goal = state
        .runtime_threads
        .get_goal(&id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("thread '{id}' has no goal")))?;
    Ok(Json(goal))
}

#[derive(Debug, Deserialize)]
struct UpsertThreadGoalRequest {
    objective: String,
    #[serde(default)]
    token_budget: Option<i64>,
}

/// `PUT /v1/threads/{id}/goal` — create or replace the persistent goal for a
/// thread. Only `Active` goals may be created through this route; lifecycle
/// transitions (`complete`, `block`) have dedicated action endpoints.
async fn upsert_thread_goal(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
    Json(req): Json<UpsertThreadGoalRequest>,
) -> Result<(StatusCode, Json<codewhale_protocol::ThreadGoal>), ApiError> {
    if req.objective.trim().is_empty() {
        return Err(ApiError::bad_request("objective must not be blank"));
    }
    // Verify the thread exists.
    state
        .runtime_threads
        .get_thread(&id)
        .await
        .map_err(map_thread_err)?;
    let now = chrono::Utc::now().timestamp();
    let existing = state
        .runtime_threads
        .get_goal(&id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let is_new = existing.is_none();
    let goal = codewhale_protocol::ThreadGoal {
        thread_id: id.clone(),
        goal_id: format!("goal-{}", uuid::Uuid::new_v4()),
        objective: req.objective.clone(),
        status: codewhale_protocol::ThreadGoalStatus::Active,
        token_budget: req.token_budget,
        tokens_used: 0,
        time_used_seconds: 0,
        continuation_count: 0,
        created_at: now,
        updated_at: now,
    };
    state
        .runtime_threads
        .save_goal(goal.clone())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let status_code = if is_new {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    // Emit a replayable goal-updated event so SSE subscribers can react.
    let _ = state
        .runtime_threads
        .emit_goal_updated_event(&id, goal.clone())
        .await;
    // Inject the goal into a cached engine (if any) and dispatch the kickoff
    // turn while the thread is idle. Errors are advisory: the goal record is
    // already durable and a subsequent turn still carries it.
    if let Err(err) = state.runtime_threads.activate_thread_goal(&id).await {
        tracing::warn!("failed to activate goal for thread '{id}': {err}");
    }
    Ok((status_code, Json(goal)))
}

/// `DELETE /v1/threads/{id}/goal` — remove the persistent goal from a thread.
/// Returns 204 No Content on success, 404 if there was no goal.
async fn delete_thread_goal(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .runtime_threads
        .get_thread(&id)
        .await
        .map_err(map_thread_err)?;
    let deleted = state
        .runtime_threads
        .remove_goal(&id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if !deleted {
        return Err(ApiError::not_found(format!("thread '{id}' has no goal")));
    }
    let _ = state.runtime_threads.emit_goal_cleared_event(&id).await;
    state
        .runtime_threads
        .sync_engine_goal_status(&id, crate::tools::goal::GoalStatus::Active, true)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/threads/{id}/goal/complete` — transition the goal to `Complete`.
/// Only valid from a non-terminal status; returns 409 Conflict if the goal is
/// already in a terminal state, and 404 if the thread has no goal.
async fn complete_thread_goal(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<Json<codewhale_protocol::ThreadGoal>, ApiError> {
    state
        .runtime_threads
        .get_thread(&id)
        .await
        .map_err(map_thread_err)?;
    let goal = state
        .runtime_threads
        .get_goal(&id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("thread '{id}' has no goal")))?;
    if matches!(goal.status, codewhale_protocol::ThreadGoalStatus::Complete) {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            message: format!("goal for thread '{id}' is already complete"),
        });
    }
    let now = chrono::Utc::now().timestamp();
    let updated = codewhale_protocol::ThreadGoal {
        status: codewhale_protocol::ThreadGoalStatus::Complete,
        updated_at: now,
        ..goal
    };
    state
        .runtime_threads
        .save_goal(updated.clone())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let _ = state
        .runtime_threads
        .emit_goal_updated_event(&id, updated.clone())
        .await;
    state
        .runtime_threads
        .sync_engine_goal_status(&id, crate::tools::goal::GoalStatus::Complete, false)
        .await;
    Ok(Json(updated))
}

/// `POST /v1/threads/{id}/goal/block` — transition the goal to `Blocked`.
/// Rejects transitions from terminal states (returns 409).
async fn block_thread_goal(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<Json<codewhale_protocol::ThreadGoal>, ApiError> {
    state
        .runtime_threads
        .get_thread(&id)
        .await
        .map_err(map_thread_err)?;
    let goal = state
        .runtime_threads
        .get_goal(&id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("thread '{id}' has no goal")))?;
    if matches!(goal.status, codewhale_protocol::ThreadGoalStatus::Complete) {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            message: format!(
                "goal for thread '{id}' is already complete; cannot transition to blocked"
            ),
        });
    }
    let now = chrono::Utc::now().timestamp();
    let updated = codewhale_protocol::ThreadGoal {
        status: codewhale_protocol::ThreadGoalStatus::Blocked,
        updated_at: now,
        ..goal
    };
    state
        .runtime_threads
        .save_goal(updated.clone())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let _ = state
        .runtime_threads
        .emit_goal_updated_event(&id, updated.clone())
        .await;
    state
        .runtime_threads
        .sync_engine_goal_status(&id, crate::tools::goal::GoalStatus::Blocked, false)
        .await;
    Ok(Json(updated))
}

/// Runtime-authenticated administrative task inventory.
///
/// Unlike in-session TUI/model controls, the Runtime API token authorizes the
/// caller for the whole host runtime, so these endpoints intentionally span
/// sessions. Running with `--insecure` explicitly opts out of that host boundary.
async fn list_tasks(
    State(state): State<RuntimeApiState>,
    Query(query): Query<TasksQuery>,
) -> Result<Json<TasksResponse>, ApiError> {
    let tasks = match query.workspace.as_deref() {
        Some(workspace) => {
            state
                .task_manager
                .list_tasks_scoped(query.limit, Some(workspace))
                .await
        }
        None => state.task_manager.list_tasks(query.limit).await,
    };
    let counts = state.task_manager.counts().await;
    Ok(Json(TasksResponse { tasks, counts }))
}

/// Runtime-authenticated administrative task lookup across host sessions.
async fn get_task(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<Json<TaskRecord>, ApiError> {
    let task = state
        .task_manager
        .get_task(&id)
        .await
        .map_err(map_task_err)?;
    Ok(Json(task))
}

/// Runtime-authenticated administrative task cancellation across host sessions.
async fn cancel_task(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<Json<TaskRecord>, ApiError> {
    let cancellation = state
        .task_manager
        .cancel_task(&id)
        .await
        .map_err(map_task_err)?;
    Ok(Json(cancellation.task))
}

async fn stream_thread_events(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
    Query(query): Query<ThreadEventsQuery>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<SseEvent, Infallible>>>, ApiError> {
    let _ = state
        .runtime_threads
        .get_thread(&id)
        .await
        .map_err(map_thread_err)?;

    // Subscribe before reading durable history. An event emitted while replay
    // is loaded is then present in both places (and deduped below) or queued
    // live, never in an uncovered handoff window.
    let live = state.runtime_threads.subscribe_events();
    if query
        .replay_limit
        .is_some_and(|limit| limit > MAX_RUNTIME_EVENT_REPLAY_TAIL)
    {
        return Err(ApiError::bad_request(format!(
            "replay_limit cannot exceed {MAX_RUNTIME_EVENT_REPLAY_TAIL}"
        )));
    }
    let replay = state
        .runtime_threads
        .replay_events(&id, query.since_seq, query.replay_limit)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let stream = replay_live_thread_events(
        state.runtime_threads.clone(),
        id,
        replay.base_seq,
        replay.batches,
        live,
    );

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

fn replay_live_thread_events(
    runtime_threads: SharedRuntimeThreadManager,
    thread_id: String,
    mut last_seq: u64,
    mut backlog: tokio::sync::mpsc::Receiver<
        std::result::Result<Vec<crate::runtime_threads::RuntimeEventRecord>, String>,
    >,
    mut live: tokio::sync::broadcast::Receiver<crate::runtime_threads::RuntimeEventRecord>,
) -> impl futures_util::Stream<Item = Result<SseEvent, Infallible>> {
    stream! {
        while let Some(batch) = backlog.recv().await {
            let events = match batch {
                Ok(events) => events,
                Err(error) => {
                    tracing::warn!(
                        thread_id = %thread_id,
                        last_seq,
                        %error,
                        "Failed to replay Runtime web event stream from durable history"
                    );
                    return;
                }
            };
            for event in events {
                if event.thread_id != thread_id || event.seq <= last_seq {
                    continue;
                }
                let previous_seq = last_seq;
                last_seq = event.seq;
                let event_name = event.event.clone();
                yield Ok(sse_json(
                    &event_name,
                    runtime_event_payload_with_previous(event, previous_seq),
                ));
            }
        }

        'live: loop {
            match live.recv().await {
                Ok(event) => {
                    if event.thread_id != thread_id || event.seq <= last_seq {
                        continue;
                    }
                    let previous_seq = last_seq;
                    last_seq = event.seq;
                    let event_name = event.event.clone();
                    yield Ok(sse_json(
                        &event_name,
                        runtime_event_payload_with_previous(event, previous_seq),
                    ));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    // Broadcast is only a wake-up path; durable history remains
                    // authoritative. Catch up from the last delivered cursor so
                    // receiver pressure cannot turn into a silent prompt loss.
                    let mut recovered = match runtime_threads
                        .replay_events(&thread_id, Some(last_seq), None)
                        .await
                    {
                        Ok(replay) => replay.batches,
                        Err(error) => {
                            tracing::warn!(
                                thread_id = %thread_id,
                                last_seq,
                                skipped,
                                %error,
                                "Failed to recover lagged Runtime web event stream from durable history"
                            );
                            break 'live;
                        }
                    };
                    while let Some(batch) = recovered.recv().await {
                        let events = match batch {
                            Ok(events) => events,
                            Err(error) => {
                                tracing::warn!(
                                    thread_id = %thread_id,
                                    last_seq,
                                    skipped,
                                    %error,
                                    "Failed to recover lagged Runtime web event stream from durable history"
                                );
                                break 'live;
                            }
                        };
                        for event in events {
                            if event.thread_id != thread_id || event.seq <= last_seq {
                                continue;
                            }
                            let previous_seq = last_seq;
                            last_seq = event.seq;
                            let event_name = event.event.clone();
                            yield Ok(sse_json(
                                &event_name,
                                runtime_event_payload_with_previous(event, previous_seq),
                            ));
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}

async fn stream_turn(
    State(state): State<RuntimeApiState>,
    Json(req): Json<StreamTurnRequest>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<SseEvent, Infallible>>>, ApiError> {
    if req.prompt.trim().is_empty() {
        return Err(ApiError::bad_request("prompt is required"));
    }

    let model = runtime_request_model(&state.config.read(), req.model.as_deref());
    let workspace = req
        .workspace
        .clone()
        .unwrap_or_else(|| state.workspace.clone());
    let mode = req.mode.clone().unwrap_or_else(|| "agent".to_string());
    let permission_posture = req.permission_posture.clone();
    let allow_shell = req.allow_shell.unwrap_or(state.config.read().allow_shell());
    let trust_mode = req.trust_mode.unwrap_or(false);
    let auto_approve = req.auto_approve.unwrap_or(false);
    let prompt = req.prompt;

    let thread = state
        .runtime_threads
        .create_thread(CreateThreadRequest {
            model: Some(model.clone()),
            workspace: Some(workspace.clone()),
            mode: Some(mode.clone()),
            permission_posture: permission_posture.clone(),
            allow_shell: Some(allow_shell),
            trust_mode: Some(trust_mode),
            auto_approve: Some(auto_approve),
            archived: true,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await
        .map_err(|e| ApiError::internal(format!("Failed to create stream thread: {e}")))?;

    #[cfg(test)]
    if let Some(hook) = &state.compat_stream_test_hook {
        let (resume, wait_for_resume) = tokio::sync::oneshot::channel();
        hook.send(CompatStreamTestPoint::ThreadCreated {
            thread_id: thread.id.clone(),
            resume,
        })
        .map_err(|_| ApiError::internal("Compatibility stream test hook closed"))?;
        wait_for_resume
            .await
            .map_err(|_| ApiError::internal("Compatibility stream test hook dropped resume"))?;
    }

    let turn = state
        .runtime_threads
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt,
                input_summary: None,
                model: Some(model.clone()),
                mode: Some(mode.clone()),
                permission_posture,
                allow_shell: Some(allow_shell),
                trust_mode: Some(trust_mode),
                auto_approve: Some(auto_approve),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| ApiError::internal(format!("Failed to start stream turn: {e}")))?;

    // Subscribe before reading the durable replay. Events produced while the
    // replay is loaded then exist in at least one source, and the sequence
    // cursor below removes overlap without dropping the handoff edge.
    let mut live = state.runtime_threads.subscribe_events();
    let thread_id = thread.id.clone();
    let turn_id = turn.id.clone();

    #[cfg(test)]
    if let Some(hook) = &state.compat_stream_test_hook {
        let (resume, wait_for_resume) = tokio::sync::oneshot::channel();
        hook.send(CompatStreamTestPoint::SubscribedBeforeReplay {
            thread_id: thread_id.clone(),
            turn_id: turn_id.clone(),
            resume,
        })
        .map_err(|_| ApiError::internal("Compatibility stream test hook closed"))?;
        wait_for_resume
            .await
            .map_err(|_| ApiError::internal("Compatibility stream test hook dropped resume"))?;
    }

    let mut backlog = state
        .runtime_threads
        .replay_events(&thread.id, None, None)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to load stream backlog: {e}")))?;

    #[cfg(test)]
    if let Some(hook) = &state.compat_stream_test_hook {
        let (resume, wait_for_resume) = tokio::sync::oneshot::channel();
        hook.send(CompatStreamTestPoint::ReplayLoaded {
            thread_id: thread_id.clone(),
            turn_id: turn_id.clone(),
            resume,
        })
        .map_err(|_| ApiError::internal("Compatibility stream test hook closed"))?;
        wait_for_resume
            .await
            .map_err(|_| ApiError::internal("Compatibility stream test hook dropped resume"))?;
    }

    let stream = stream! {
        let mut last_seq = 0;
        yield Ok(sse_json("turn.started", json!({
            "thread_id": thread.id,
            "turn_id": turn.id,
            "model": model,
            "mode": mode,
            "workspace": workspace,
        })));

        while let Some(batch) = backlog.batches.recv().await {
            let events = match batch {
                Ok(events) => events,
                Err(error) => {
                    tracing::warn!(
                        thread_id = %thread_id,
                        turn_id = %turn_id,
                        %error,
                        "Failed to replay compatibility stream from durable history"
                    );
                    yield Ok(sse_json("error", json!({
                        "message": "failed to replay durable event stream",
                    })));
                    return;
                }
            };
            for event in events {
                let Some((mapped, terminal)) = take_compat_turn_event(
                    &event,
                    &thread_id,
                    &turn_id,
                    &mut last_seq,
                ) else {
                    continue;
                };
                if let Some(mapped) = mapped {
                    yield Ok(mapped);
                }
                if terminal {
                    yield Ok(sse_json("done", json!({})));
                    return;
                }
            }
        }

        loop {
            match live.recv().await {
                Ok(event) => {
                    let Some((mapped, terminal)) = take_compat_turn_event(
                        &event,
                        &thread_id,
                        &turn_id,
                        &mut last_seq,
                    ) else {
                        continue;
                    };
                    if let Some(mapped) = mapped {
                        yield Ok(mapped);
                    }
                    if terminal {
                        yield Ok(sse_json("done", json!({})));
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    let mut recovered = match state.runtime_threads
                        .replay_events(&thread_id, Some(last_seq), None)
                        .await
                    {
                        Ok(replay) => replay.batches,
                        Err(error) => {
                            tracing::warn!(
                                thread_id = %thread_id,
                                turn_id = %turn_id,
                                last_seq,
                                skipped,
                                %error,
                                "Failed to recover lagged compatibility stream from durable history"
                            );
                            yield Ok(sse_json("error", json!({
                                "message": "failed to recover lagged event stream",
                            })));
                            return;
                        }
                    };
                    while let Some(batch) = recovered.recv().await {
                        let events = match batch {
                            Ok(events) => events,
                            Err(error) => {
                                tracing::warn!(
                                    thread_id = %thread_id,
                                    turn_id = %turn_id,
                                    last_seq,
                                    skipped,
                                    %error,
                                    "Failed to recover lagged compatibility stream from durable history"
                                );
                                yield Ok(sse_json("error", json!({
                                    "message": "failed to recover lagged event stream",
                                })));
                                return;
                            }
                        };
                        for event in events {
                            let Some((mapped, terminal)) = take_compat_turn_event(
                                &event,
                                &thread_id,
                                &turn_id,
                                &mut last_seq,
                            ) else {
                                continue;
                            };
                            if let Some(mapped) = mapped {
                                yield Ok(mapped);
                            }
                            if terminal {
                                yield Ok(sse_json("done", json!({})));
                                return;
                            }
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    yield Ok(sse_json("error", json!({ "message": "event channel closed" })));
                    return;
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

fn take_compat_turn_event(
    event: &crate::runtime_threads::RuntimeEventRecord,
    thread_id: &str,
    turn_id: &str,
    last_seq: &mut u64,
) -> Option<(Option<SseEvent>, bool)> {
    if event.thread_id != thread_id
        || event.turn_id.as_deref() != Some(turn_id)
        || event.seq <= *last_seq
    {
        return None;
    }
    *last_seq = event.seq;
    Some((
        map_compat_stream_event(event),
        event.event == "turn.completed",
    ))
}

fn runtime_event_payload(event: crate::runtime_threads::RuntimeEventRecord) -> serde_json::Value {
    let event_name = event.event.clone();
    let timestamp = event.timestamp.to_rfc3339();
    let schema_version = RUNTIME_EVENT_ENVELOPE_SCHEMA_VERSION;
    let envelope = RuntimeEventEnvelope {
        schema_version,
        seq: event.seq,
        event: event_name.clone(),
        kind: event_name,
        thread_id: event.thread_id,
        turn_id: event.turn_id,
        item_id: event.item_id,
        timestamp: timestamp.clone(),
        created_at: Some(timestamp),
        payload: event.payload,
        extra: Default::default(),
    };
    serde_json::to_value(envelope).expect("serialize runtime event envelope")
}

fn runtime_event_payload_with_previous(
    event: crate::runtime_threads::RuntimeEventRecord,
    previous_seq: u64,
) -> serde_json::Value {
    let mut payload = runtime_event_payload(event);
    if let Some(object) = payload.as_object_mut() {
        object.insert("previous_seq".to_string(), json!(previous_seq));
    }
    payload
}

fn map_compat_stream_event(event: &crate::runtime_threads::RuntimeEventRecord) -> Option<SseEvent> {
    let payload = &event.payload;
    match event.event.as_str() {
        "item.delta" => {
            let kind = payload
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if kind == "agent_message" {
                let content = payload
                    .get("delta")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                Some(sse_json("message.delta", json!({ "content": content })))
            } else if kind == "tool_call" {
                let output = payload
                    .get("delta")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                Some(sse_json("tool.progress", json!({ "output": output })))
            } else {
                None
            }
        }
        "item.started" => {
            let tool = payload.get("tool")?;
            let id = tool.get("id").cloned().unwrap_or(Value::Null);
            let name = tool.get("name").cloned().unwrap_or(Value::Null);
            let input = tool.get("input").cloned().unwrap_or(Value::Null);
            Some(sse_json(
                "tool.started",
                json!({
                    "id": id,
                    "name": name,
                    "input": input,
                }),
            ))
        }
        "item.completed" | "item.failed" => {
            let item = payload.get("item")?;
            let kind = item
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if kind == "tool_call" || kind == "file_change" || kind == "command_execution" {
                let id = item.get("id").cloned().unwrap_or(Value::Null);
                let success = event.event == "item.completed";
                let output = item.get("detail").cloned().unwrap_or_else(|| {
                    Value::String(
                        item.get("summary")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    )
                });
                Some(sse_json(
                    "tool.completed",
                    json!({
                        "id": id,
                        "success": success,
                        "output": output,
                    }),
                ))
            } else if kind == "status" {
                let message = item
                    .get("detail")
                    .and_then(|v| v.as_str())
                    .or_else(|| item.get("summary").and_then(|v| v.as_str()))
                    .unwrap_or_default();
                Some(sse_json("status", json!({ "message": message })))
            } else if kind == "error" {
                let message = item
                    .get("detail")
                    .and_then(|v| v.as_str())
                    .or_else(|| item.get("summary").and_then(|v| v.as_str()))
                    .unwrap_or_default();
                Some(sse_json("error", json!({ "message": message })))
            } else {
                None
            }
        }
        "approval.required" => {
            let approval_id = payload
                .get("approval_id")
                .or_else(|| payload.get("id"))?
                .clone();
            Some(sse_json(
                "approval.required",
                json!({
                    "id": approval_id,
                    "approval_id": approval_id,
                    "thread_id": event.thread_id,
                    "turn_id": event.turn_id,
                    "tool_name": payload.get("tool_name"),
                    "description": payload.get("description"),
                    "intent_summary": payload.get("intent_summary"),
                }),
            ))
        }
        "approval.decided" => {
            let approval_id = payload
                .get("approval_id")
                .or_else(|| payload.get("id"))?
                .clone();
            Some(sse_json(
                "approval.decided",
                json!({
                    "id": approval_id,
                    "approval_id": approval_id,
                    "thread_id": event.thread_id,
                    "turn_id": event.turn_id,
                    "decision": payload.get("decision"),
                    "remember": payload.get("remember"),
                    "auto": payload.get("auto"),
                    "timeout": payload.get("timeout"),
                }),
            ))
        }
        "approval.timeout" => {
            let approval_id = payload
                .get("approval_id")
                .or_else(|| payload.get("id"))?
                .clone();
            Some(sse_json(
                "approval.timeout",
                json!({
                    "id": approval_id,
                    "approval_id": approval_id,
                    "thread_id": event.thread_id,
                    "turn_id": event.turn_id,
                    "timeout_secs": payload.get("timeout_secs"),
                }),
            ))
        }
        "user_input.required" => {
            let input_id = payload
                .get("input_id")
                .or_else(|| payload.get("id"))?
                .clone();
            let request = payload.get("request")?.clone();
            Some(sse_json(
                "user_input.required",
                json!({
                    "id": input_id,
                    "input_id": input_id,
                    "thread_id": event.thread_id,
                    "turn_id": event.turn_id,
                    "status": "required",
                    "request": request,
                }),
            ))
        }
        "user_input.answered" | "user_input.canceled" => {
            let input_id = payload
                .get("input_id")
                .or_else(|| payload.get("id"))?
                .clone();
            let status = if event.event == "user_input.answered" {
                "submitted"
            } else {
                "canceled"
            };
            Some(sse_json(
                &event.event,
                json!({
                    "id": input_id,
                    "input_id": input_id,
                    "thread_id": event.thread_id,
                    "turn_id": event.turn_id,
                    "status": status,
                    "terminal": payload.get("terminal").and_then(Value::as_bool).unwrap_or(false),
                }),
            ))
        }
        "sandbox.denied" => Some(sse_json("sandbox.denied", payload.clone())),
        "turn.completed" => {
            let usage = payload
                .get("turn")
                .and_then(|turn| turn.get("usage"))
                .cloned()
                .unwrap_or(json!(null));
            Some(sse_json("turn.completed", json!({ "usage": usage })))
        }
        _ => None,
    }
}

fn sse_json(event: &str, payload: serde_json::Value) -> SseEvent {
    let data = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
    SseEvent::default().event(event).data(data)
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars.saturating_sub(3)).collect();
    format!("{truncated}...")
}

fn resolve_skills_dir(config: &Config, workspace: &std::path::Path) -> PathBuf {
    if config.skills_config().scan_codewhale_only() {
        if config.skills_dir.is_some() {
            return config.skills_dir();
        }
        if let Some(codewhale_skills_dir) = crate::skills::codewhale_workspace_skills_dir(workspace)
            && let Ok(canonical_skills) = fs::canonicalize(&codewhale_skills_dir)
        {
            return canonical_skills;
        }
        return config.skills_dir();
    }

    // Canonicalize the workspace once so the symlink-containment check below
    // compares like-for-like. If the workspace can't be canonicalized at all
    // (e.g. it doesn't exist on disk yet) fall back to the configured global
    // skills dir rather than risk constructing paths from a non-existent root.
    let canonical_workspace = match fs::canonicalize(workspace) {
        Ok(path) => path,
        Err(_) => return config.skills_dir(),
    };
    for candidate in [
        canonical_workspace.join(".agents").join("skills"),
        canonical_workspace.join("skills"),
    ] {
        // Re-canonicalize the candidate so a `.agents/skills` symlink to e.g.
        // `/etc` cannot promote arbitrary filesystem locations into the
        // skills directory. The candidate must still resolve under the
        // canonicalized workspace root after symlink expansion.
        if let Ok(canon) = fs::canonicalize(&candidate)
            && canon.starts_with(&canonical_workspace)
            && canon.is_dir()
        {
            return canon;
        }
    }
    config.skills_dir()
}

fn skills_search_directories(
    workspace: &FsPath,
    skills_dir: &FsPath,
    mode: crate::skills::SkillDiscoveryMode,
) -> Vec<PathBuf> {
    crate::skills::skill_directories_for_workspace_and_dir(workspace, skills_dir, mode)
}

fn discover_skills_for_runtime_api(
    workspace: &FsPath,
    skills_dir: &FsPath,
    mode: crate::skills::SkillDiscoveryMode,
    plugins: Option<&crate::plugins::PluginRegistry>,
) -> (crate::skills::SkillRegistry, Vec<PathBuf>) {
    let directories = skills_search_directories(workspace, skills_dir, mode);
    let registry =
        crate::skills::discover_from_directories_with_plugins(directories.clone(), plugins);
    (registry, directories)
}

fn skill_entry_is_bundled(skill: &crate::skills::Skill, skills_dir: &FsPath) -> bool {
    if !crate::skills::is_bundled_skill_name(&skill.name) {
        return false;
    }

    let expected_path = skills_dir.join(&skill.name).join("SKILL.md");
    paths_refer_to_same_file(&skill.path, &expected_path)
}

fn paths_refer_to_same_file(left: &FsPath, right: &FsPath) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn format_skill_search_paths(directories: &[PathBuf]) -> String {
    if directories.is_empty() {
        return "<none>".to_string();
    }
    directories
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Deserialize)]
struct UsageQuery {
    /// ISO-8601 lower bound (inclusive). When omitted, no lower bound.
    since: Option<String>,
    /// ISO-8601 upper bound (inclusive). When omitted, no upper bound.
    until: Option<String>,
    /// Bucket key. One of `day` (default), `model`, `provider`, `thread`.
    group_by: Option<String>,
}

fn parse_iso8601(raw: &str, field: &str) -> Result<chrono::DateTime<Utc>, ApiError> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| ApiError::bad_request(format!("Invalid {field} (expected RFC 3339): {e}")))
}

async fn get_usage(
    State(state): State<RuntimeApiState>,
    Query(query): Query<UsageQuery>,
) -> Result<Json<Value>, ApiError> {
    let since = match query.since.as_deref() {
        Some(raw) => Some(parse_iso8601(raw, "since")?),
        None => None,
    };
    let until = match query.until.as_deref() {
        Some(raw) => Some(parse_iso8601(raw, "until")?),
        None => None,
    };
    if let (Some(s), Some(u)) = (since, until)
        && s > u
    {
        return Err(ApiError::bad_request("since must be <= until".to_string()));
    }
    let group_by = match query.group_by.as_deref().unwrap_or("day") {
        "day" => UsageGroupBy::Day,
        "model" => UsageGroupBy::Model,
        "provider" => UsageGroupBy::Provider,
        "thread" => UsageGroupBy::Thread,
        other => {
            return Err(ApiError::bad_request(format!(
                "Unsupported group_by '{other}': expected one of day, model, provider, thread"
            )));
        }
    };

    let aggregation = state
        .runtime_threads
        .aggregate_usage(since, until, group_by)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!(aggregation)))
}

#[derive(Debug, Deserialize)]
struct SnapshotsQuery {
    /// Maximum number of snapshots to return. Mirrors `/restore list [N]`.
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct SnapshotEntry {
    id: String,
    label: String,
    timestamp: i64,
}

async fn list_snapshots(
    State(state): State<RuntimeApiState>,
    Query(query): Query<SnapshotsQuery>,
) -> Result<Json<Vec<SnapshotEntry>>, ApiError> {
    Ok(Json(snapshot_entries_for_workspace(
        &state.workspace,
        query,
    )?))
}

async fn restore_snapshot(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    restore_snapshot_for_workspace(&state.workspace, &id)?;
    Ok(Json(json!({
        "restored": id,
    })))
}

fn restore_snapshot_for_workspace(workspace: &FsPath, id: &str) -> Result<(), ApiError> {
    let repo = crate::snapshot::SnapshotRepo::open_or_init(workspace)
        .map_err(|e| ApiError::internal(format!("Snapshot repo init failed: {e}")))?;
    let snapshot_id = crate::snapshot::SnapshotId(id.to_string());
    repo.restore(&snapshot_id)
        .map_err(|e| ApiError::internal(format!("Snapshot restore failed: {e}")))
}

fn snapshot_entries_for_workspace(
    workspace: &FsPath,
    query: SnapshotsQuery,
) -> Result<Vec<SnapshotEntry>, ApiError> {
    const DEFAULT_LIMIT: usize = 20;
    const MAX_LIMIT: usize = 100;

    let limit = match query.limit.unwrap_or(DEFAULT_LIMIT) {
        1..=MAX_LIMIT => query.limit.unwrap_or(DEFAULT_LIMIT),
        other => {
            return Err(ApiError::bad_request(format!(
                "limit must be between 1 and {MAX_LIMIT}; got {other}",
            )));
        }
    };
    let repo = crate::snapshot::SnapshotRepo::open_or_init(workspace)
        .map_err(|e| ApiError::internal(format!("Snapshot repo unavailable: {e}")))?;
    let snapshots = repo
        .list(limit)
        .map_err(|e| ApiError::internal(format!("Failed to list snapshots: {e}")))?;
    Ok(snapshots
        .into_iter()
        .map(|snapshot| SnapshotEntry {
            id: snapshot.id.as_str().to_string(),
            label: snapshot.label,
            timestamp: snapshot.timestamp,
        })
        .collect())
}

// ── Provider / Model catalog endpoints ──

/// Entry in `GET /v1/providers`.
///
/// Exposes the static provider registry so the GUI can render a dynamic
/// provider picker instead of hard-coding `deepseek` only. The `id` matches
/// `ApiProvider::as_str()`; callers must also preserve `model_provider_id`
/// when present. Both can be pinned to one new thread via `POST /v1/threads`
/// without mutating the runtime's global provider configuration.
#[derive(Debug, Clone, Serialize)]
struct ProviderEntry {
    /// Stable generic provider kind — matches `ApiProvider::as_str()` and is
    /// suitable for `CreateThreadRequest.model_provider`. This is not always
    /// the exact configured route id: named custom routes also require
    /// `model_provider_id` below.
    id: String,
    /// Exact configured provider key for the active route, when one exists.
    /// A named custom route such as `lm-studio` is represented as generic
    /// `id = "custom"` plus `model_provider_id = "lm-studio"` so a new
    /// thread never collapses back to the legacy root custom route.
    model_provider_id: Option<String>,
    /// Human-friendly name for picker UIs (e.g. "DeepSeek", "OpenAI").
    display_name: String,
    /// Default model id for this provider, if any. Empty for pass-through
    /// providers (Ollama / Custom) that expose no built-in catalog.
    default_model: String,
    /// Whether this provider exposes a built-in model list. When false, the
    /// GUI should render a free-text input instead of calling
    /// `/v1/providers/{id}/models`.
    has_model_catalog: bool,
    /// Sanitized structural credential classification for the exact route.
    /// This deliberately contains no credential, endpoint, path, environment
    /// variable, consent-source, or token metadata.
    #[serde(rename = "credentialState")]
    credential_state: ProviderCredentialState,
}

/// Stable, non-secret wire projection of provider readiness.
///
/// The richer internal classification remains private to the Runtime. In
/// particular, saved API keys and imported tokens collapse to `configured`,
/// while login and external-consent states collapse to `login_required`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProviderCredentialState {
    Configured,
    LoginRequired,
    Missing,
    NoAuth,
    Local,
    Legacy,
}

impl From<crate::provider_readiness::CredentialState> for ProviderCredentialState {
    fn from(value: crate::provider_readiness::CredentialState) -> Self {
        use crate::provider_readiness::CredentialState;

        match value {
            CredentialState::Saved | CredentialState::ImportedToken => Self::Configured,
            CredentialState::MissingLogin | CredentialState::ExternalConsent => Self::LoginRequired,
            CredentialState::MissingKey => Self::Missing,
            CredentialState::NoAuth => Self::NoAuth,
            CredentialState::Local => Self::Local,
            CredentialState::Legacy => Self::Legacy,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ProvidersResponse {
    /// Currently active provider id (matches `GET /v1/config`'s `provider`).
    current: String,
    providers: Vec<ProviderEntry>,
}

/// Entry in `GET /v1/providers/{id}/models`.
#[derive(Debug, Clone, Serialize)]
struct ProviderModelEntry {
    /// Canonical model id suitable for `POST /v1/threads`'s `model` field.
    id: String,
    /// Image-input support reported by the exact resolved provider/model
    /// offering. Unknown stays unknown: the API never guesses from a model
    /// name or transport protocol.
    image_input: codewhale_config::route::CapabilityState,
}

#[derive(Debug, Clone, Serialize)]
struct ProviderModelsResponse {
    provider: String,
    models: Vec<ProviderModelEntry>,
}

fn push_unique_model(models: &mut Vec<String>, model: &str) {
    let model = model.trim();
    if !model.is_empty()
        && !models
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(model))
    {
        models.push(model.to_string());
    }
}

fn provider_uses_custom_route_for_api(config: &Config, provider: ApiProvider) -> bool {
    config.provider_uses_custom_endpoint(provider)
}

fn provider_models_for_api(
    config: &Config,
    active_provider: ApiProvider,
    provider: ApiProvider,
) -> Vec<String> {
    let mut models = Vec::new();
    if let Some(model) = config
        .provider_config_for(provider)
        .and_then(|entry| entry.model.as_deref())
    {
        push_unique_model(&mut models, model);
    }
    if provider == active_provider {
        let active_model = config.default_model();
        if !active_model.trim().eq_ignore_ascii_case("auto") {
            push_unique_model(&mut models, &active_model);
        }
        if config.model_ids_pass_through() {
            return models;
        }
    }
    if provider_uses_custom_route_for_api(config, provider) {
        return models;
    }
    for model in crate::provider_lake::models_for_provider(config, active_provider, provider) {
        push_unique_model(&mut models, &model);
    }
    models
}

fn provider_model_image_input_for_api(
    config: &Config,
    provider: ApiProvider,
    model: &str,
) -> codewhale_config::route::CapabilityState {
    crate::route_runtime::resolve_runtime_route(config, provider, Some(model))
        .map(|route| route.candidate.capabilities().image_input)
        .unwrap_or_default()
}

fn provider_default_model_for_api(
    config: &Config,
    active_provider: ApiProvider,
    provider: ApiProvider,
) -> String {
    if provider == active_provider {
        return config.default_model();
    }
    provider_models_for_api(config, active_provider, provider)
        .into_iter()
        .next()
        .unwrap_or_default()
}

pub(crate) fn runtime_chat_model_id_is_safe(value: &str) -> bool {
    let sanitized = crate::cost_status::sanitize_persisted_route_label(value);
    value == value.trim()
        && !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && !value.contains("..")
        && !value.contains("://")
        // Runtime Chat publishes a non-secret selector, never an endpoint or
        // userinfo-bearing authority. Model families that need revisions can
        // use their ordinary slash/dash ids; `@` is intentionally excluded at
        // this trust boundary because `user:password@host:port/path` otherwise
        // passes the generic route-label sanitizer.
        && !value.contains('@')
        && !runtime_chat_model_id_looks_like_host_port(value)
        && !value.starts_with("redacted-")
        && sanitized == value
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'.' | b'_' | b':' | b'/' | b'@' | b'+' | b'-')
        })
}

fn runtime_chat_model_id_looks_like_host_port(value: &str) -> bool {
    let authority = value.split('/').next().unwrap_or(value);
    let Some((host, port)) = authority.rsplit_once(':') else {
        return false;
    };
    !host.is_empty() && !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn runtime_chat_route_id_is_safe(value: &str) -> bool {
    let sanitized = crate::cost_status::sanitize_persisted_route_label(value);
    value == value.trim()
        && !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && !value.contains("..")
        && !value.starts_with("redacted-")
        && sanitized == value
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Build the deliberately narrow provider projection used by the account-owned
/// Runtime Chat relay. This is the same active-route truth exposed by the
/// authenticated native `/v1/runtime/info`, `/v1/providers`, and
/// `/v1/providers/{id}/models` endpoints, collapsed to the one exact route the
/// current Runtime can use without moving credentials across the relay.
pub(crate) fn runtime_chat_relay_catalog(
    config: &Config,
    challenge: &str,
) -> Result<Value, String> {
    use crate::provider_readiness::CredentialState;

    const PROTOCOL: &str = "codewhale.runtime-chat-relay.v1";
    if !(32..=128).contains(&challenge.len())
        || !challenge
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("Codewhale returned an invalid Runtime Chat relay challenge.".to_string());
    }

    let provider = config.api_provider();
    let identity = config
        .active_provider_identity(provider)
        .map_err(|_| "The active Runtime provider identity is invalid.".to_string())?;
    let credential_state =
        match crate::provider_readiness::credential_state_for_provider(config, provider) {
            CredentialState::Saved | CredentialState::ImportedToken => "configured",
            CredentialState::Local => "local",
            CredentialState::NoAuth => "no_auth",
            CredentialState::MissingKey
            | CredentialState::MissingLogin
            | CredentialState::ExternalConsent
            | CredentialState::Legacy => {
                return Err("The active Runtime provider is not ready for Chat.".to_string());
            }
        };

    let mut models = provider_models_for_api(config, provider, provider);
    models.retain(|model| runtime_chat_model_id_is_safe(model));
    models.sort();
    models.dedup();
    models.truncate(256);
    if models.is_empty() {
        return Err("The active Runtime provider has no safe model catalog.".to_string());
    }
    let requested_default = provider_default_model_for_api(config, provider, provider);
    let default_model = models
        .iter()
        .find(|model| model.as_str() == requested_default)
        .cloned()
        .unwrap_or_else(|| models[0].clone());
    let model_provider_id = identity
        .persisted_id()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| provider.as_str())
        .to_string();
    if !runtime_chat_route_id_is_safe(&model_provider_id) {
        return Err("The active Runtime model-provider identity is invalid.".to_string());
    }

    Ok(json!({
        "protocol": PROTOCOL,
        "challenge": challenge,
        "runtime": {
            "service": "codewhale-runtime-api",
            "apiVersion": RUNTIME_API_VERSION,
            "codewhaleVersion": env!("CARGO_PKG_VERSION"),
            "authRequired": true,
            "capabilities": {
                "relay_chat_v1": true,
                "isolated_chat_threads": true,
                "turn_operation_idempotency": true,
                "tool_execution": false,
                "stable_event_ids": true,
            },
        },
        "providers": [{
            "id": provider.as_str(),
            "modelProviderId": model_provider_id,
            "displayName": provider.display_name(),
            "defaultModel": default_model,
            "credentialState": credential_state,
            "models": models.into_iter().map(|model| json!({
                // Runtime Chat commands currently carry text only. Provider
                // vision support is not relay capability truth until bounded
                // image bytes are part of this protocol.
                "imageInput": "unsupported",
                "id": model,
            })).collect::<Vec<_>>(),
        }],
    }))
}

async fn list_providers(
    State(state): State<RuntimeApiState>,
) -> Result<Json<ProvidersResponse>, ApiError> {
    let config = state.config.read().clone();
    let active_provider = config.api_provider();
    let active_identity = config
        .active_provider_identity(active_provider)
        .map_err(ApiError::bad_request)?;
    let current = active_provider.as_str().to_string();
    let mut providers = Vec::new();
    for api_provider in ApiProvider::sorted_for_display() {
        let default_model = provider_default_model_for_api(&config, active_provider, api_provider);
        let has_model_catalog =
            !crate::provider_lake::all_catalog_models_for_provider(api_provider).is_empty();
        providers.push(ProviderEntry {
            id: api_provider.as_str().to_string(),
            model_provider_id: (api_provider == active_provider)
                .then(|| active_identity.persisted_id().map(str::to_string))
                .flatten(),
            display_name: api_provider.display_name().to_string(),
            default_model,
            has_model_catalog,
            credential_state: crate::provider_readiness::credential_state_for_provider(
                &config,
                api_provider,
            )
            .into(),
        });
    }
    Ok(Json(ProvidersResponse { current, providers }))
}

#[derive(Debug, Deserialize)]
struct ListProviderModelsParams {
    /// Optional filter: when provided, models whose id contains this
    /// substring (case-insensitive) are returned. Currently informational —
    /// the catalog is small enough to filter client-side.
    #[serde(default)]
    #[allow(dead_code)]
    filter: Option<String>,
}

async fn list_provider_models(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
    _params: Query<ListProviderModelsParams>,
) -> Result<Json<ProviderModelsResponse>, ApiError> {
    let config = state.config.read().clone();
    let active_provider = config.api_provider();
    let api_provider = ApiProvider::parse(&id)
        .ok_or_else(|| ApiError::bad_request(format!("Unknown provider id '{id}'")))?;
    // Reject requests for the legacy deepseek-cn alias that has no
    // ProviderKind metadata — the GUI should use `deepseek` instead.
    if api_provider == ApiProvider::DeepseekCN {
        return Err(ApiError::bad_request(
            "provider 'deepseek-cn' is a legacy alias; use 'deepseek' instead",
        ));
    }
    let models = provider_models_for_api(&config, active_provider, api_provider)
        .into_iter()
        .map(|id| ProviderModelEntry {
            image_input: provider_model_image_input_for_api(&config, api_provider, &id),
            id,
        })
        .collect();
    Ok(Json(ProviderModelsResponse {
        provider: api_provider.as_str().to_string(),
        models,
    }))
}

/// Request body for `POST /v1/providers/{id}/switch`.
///
/// Mirrors the TUI's `AppAction::SwitchProvider { provider, model }` payload
/// (see `tui/ui.rs::switch_provider`). `model` is optional: when omitted,
/// the runtime resolves the active model from `[providers.<id>].model` (or
/// the provider's built-in default) and **does not** persist a `model` key,
/// so the user's per-provider config is preserved. When provided, the model
/// is normalized, persisted for the target provider, and (for DeepSeek
/// providers) also pinned as `default_text_model`.
#[derive(Debug, Deserialize, Default)]
struct SwitchProviderRequest {
    #[serde(default)]
    model: Option<String>,
}

/// Response for `POST /v1/providers/{id}/switch`.
#[derive(Debug, Serialize)]
struct SwitchProviderResponse {
    /// The provider id that was switched to (echoes the path).
    provider: String,
    /// The resolved active model after the switch. This is the model the
    /// runtime will use for new turns — either the user-supplied override
    /// or the value resolved from `[providers.<id>].model` / the
    /// provider's built-in default. The GUI should display *this* value,
    /// not `ProviderEntry.default_model`, to avoid showing the catalog
    /// default when the user has configured a different model.
    model: String,
    /// Human-readable status message for logging/toasts.
    message: String,
    /// Whether the new provider + model were persisted to config.toml.
    persisted: bool,
}

/// `POST /v1/providers/{id}/switch` — switch the active provider, optionally
/// overriding the model.
///
/// This is the GUI-facing counterpart of the TUI's `/provider` slash command
/// (`commands/groups/core/provider.rs`) and `AppAction::SwitchProvider`
/// (`tui/ui.rs::switch_provider`). It exists so the GUI does not have to
/// simulate the switch with multiple `POST /v1/config` calls + a reload,
/// which historically led to two bugs:
///
/// 1. The GUI persisted `model = <catalog default>` even when the user
///    clicked the picker without choosing a model, clobbering a user-set
///    `[providers.<id>].model` (e.g. `glm-2` overwritten with
///    `deepseek-v4-pro`).
/// 2. The GUI then displayed the catalog default instead of the actually
///    resolved model, because it never asked the backend what model was
///    selected.
///
/// Persistence mirrors `switch_provider` (ui.rs:9390-9410):
/// - `provider` is always persisted (root `provider` key).
/// - `model` is persisted **only** when `model_override.is_some()`, via
///   `persist_provider_model_key` (writes `[providers.<id>].model`, or the
///   root `default_text_model` for DeepSeek). The `Settings` provider-model
///   map is updated the same way, including the DeepSeek-specific
///   `default_model` pin.
/// - Config is reloaded from disk and synced to active engines via
///   `runtime_threads.reload_config`, exactly like `POST /v1/config/reload`.
async fn switch_provider(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
    Json(req): Json<SwitchProviderRequest>,
) -> Result<Json<SwitchProviderResponse>, ApiError> {
    use crate::config_persistence;

    let target = ApiProvider::parse(&id)
        .ok_or_else(|| ApiError::bad_request(format!("Unknown provider id '{id}'")))?;
    // Reject the legacy deepseek-cn alias — same guard as list_provider_models.
    if target == ApiProvider::DeepseekCN {
        return Err(ApiError::bad_request(
            "provider 'deepseek-cn' is a legacy alias; use 'deepseek' instead",
        ));
    }

    // Normalize the optional model override against the *target* provider.
    // Mirrors `set_config`'s `model` branch, which validates against the
    // active route — except here we validate against the target provider,
    // because the active route is about to change.
    let model_override: Option<String> = match req.model.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => Some(normalize_runtime_config_model(target, raw)?),
    };

    // Resolve the target provider identity *before* mutating config, so
    // persistence uses the same key the TUI's switch_provider would.
    let (provider_identity, _active_provider) = {
        let config = state.config.read();
        (config.provider_identity_for(target), config.api_provider())
    };

    // Persist `provider` (always) + `model` (only when explicitly given).
    // This is the critical TUI-parity rule: a bare `/provider <id>` (no
    // model arg) MUST NOT write a `model` key, otherwise the user's
    // per-provider `[providers.<id>].model` config gets overwritten with
    // whatever the runtime resolves as the default.
    config_persistence::persist_root_string_key(
        state.config_path.as_deref(),
        "provider",
        &provider_identity,
    )
    .map_err(|e| ApiError::internal(format!("Failed to persist provider: {e}")))?;

    if let Some(ref model) = model_override {
        config_persistence::persist_provider_model_key(
            state.config_path.as_deref(),
            target,
            &provider_identity,
            model,
        )
        .map_err(|e| ApiError::internal(format!("Failed to persist model: {e}")))?;

        // Mirror the TUI's Settings update (ui.rs:9398-9406): record the
        // provider→model mapping, and for DeepSeek also pin the global
        // `default_model`. Failures here are non-fatal — the config.toml
        // write above is the source of truth.
        let _ = crate::settings::Settings::transact(|settings| {
            settings.set_model_for_provider(target.as_str(), model);
            if matches!(target, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
                let _ = settings.set("default_model", model);
            }
            Ok(())
        });
    }

    // Reload config from disk and sync to active engines. This matches
    // `POST /v1/config/reload` exactly: load → validate thread routes →
    // swap in the new config. A failure here means an active thread's
    // route is invalid under the new provider — surface it so the GUI can
    // tell the user to fix their config.
    let reloaded = Config::load(state.config_path.clone(), state.config_profile.as_deref())
        .map_err(|e| ApiError::internal(format!("Failed to reload config: {e}")))?;
    state
        .runtime_threads
        .reload_config(reloaded.clone())
        .await
        .map_err(|err| ApiError::bad_request(format!("Config reload rejected: {err}")))?;
    {
        let mut config = state.config.write();
        *config = reloaded;
    }

    // Read the resolved active model + provider from the freshly reloaded
    // config. This is the value the GUI must display — NOT the catalog
    // default and NOT the previously-active model.
    let (active_provider, active_model) = {
        let config = state.config.read();
        (config.api_provider(), config.default_model())
    };

    let message = if model_override.is_some() {
        format!(
            "Provider switched to {} (model: {}).",
            active_provider.as_str(),
            active_model
        )
    } else {
        format!(
            "Provider switched to {} (model: {}, resolved from config).",
            active_provider.as_str(),
            active_model
        )
    };

    Ok(Json(SwitchProviderResponse {
        provider: active_provider.as_str().to_string(),
        model: active_model,
        message,
        persisted: true,
    }))
}

// ── Config endpoints ──

/// GUI-relevant config snapshot returned by `GET /v1/config`.
#[derive(Debug, Clone, Serialize)]
struct GuiConfigResponse {
    model: String,
    provider: String,
    approval_mode: String,
    reasoning_effort: String,
    auto_compact: bool,
    cost_currency: String,
    default_mode: String,
    default_model: String,
    base_url: String,
    allow_shell: bool,
    mcp_config_path: String,
    subagents_enabled: bool,
    subagents_max_depth: u32,
    show_thinking: bool,
    thinking_default_expanded: bool,
    thinking_highlight: bool,
    show_tool_details: bool,
    inline_diffs: String,
    locale: String,
    max_history: usize,
    workspace_follow_symlinks: bool,
    calm_mode: bool,
    sandbox_mode: String,
    strict_tool_mode: bool,
    memory_enabled: bool,
    search_provider: String,
    prompt_suggestion: bool,
}

/// Request body for `POST /v1/config` (set a single config key).
#[derive(Debug, Deserialize)]
struct SetConfigRequest {
    key: String,
    value: String,
    #[serde(default)]
    persist: bool,
}

/// Response for `POST /v1/config` (set a single config key).
#[derive(Debug, Serialize)]
struct SetConfigResponse {
    key: String,
    value: String,
    message: String,
    persisted: bool,
    requires_reload: bool,
}

fn persist_runtime_tui_setting(key: &str, value: &str) -> Result<(), ApiError> {
    // Validate against a throwaway copy first, so an invalid value is still a
    // 400 rather than an internal error raised from inside the transaction.
    let mut probe = crate::settings::Settings::load_persisted()
        .map_err(|e| ApiError::internal(format!("Failed to load settings: {e}")))?;
    probe
        .set(key, value)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    // The write itself re-applies the key inside `Settings::transact`, so it
    // cannot save the stale snapshot above over a concurrent writer's field.
    crate::settings::Settings::transact(|settings| settings.set(key, value))
        .map_err(|e| ApiError::internal(format!("Failed to save settings: {e}")))
}

/// Response for `POST /v1/config/reload`.
#[derive(Debug, Serialize)]
struct ReloadConfigResponse {
    message: String,
}

async fn get_config(
    State(state): State<RuntimeApiState>,
) -> Result<Json<GuiConfigResponse>, ApiError> {
    let config = state.config.read();
    let settings = crate::settings::Settings::load_persisted().unwrap_or_default();
    let mcp_config_path = config.mcp_config_path().display().to_string();

    let model = config.default_model();

    let provider = config.provider_identity_for(config.api_provider());

    let approval_mode = config
        .approval_policy
        .as_deref()
        .unwrap_or("suggest")
        .to_string();
    let reasoning_effort = config.reasoning_effort().unwrap_or("auto").to_string();
    let cost_currency = settings.cost_currency.clone();
    let default_mode = settings.default_mode.as_str().to_string();
    // This field is the legacy root DeepSeek fallback, not the active
    // provider model above. Keeping the two explicit prevents a Z.ai model
    // update from silently rewriting a future DeepSeek route.
    let default_model = config
        .default_text_model
        .clone()
        .unwrap_or_else(|| DEFAULT_TEXT_MODEL.to_string());
    let base_url = config.deepseek_base_url().to_string();

    Ok(Json(GuiConfigResponse {
        model,
        provider,
        approval_mode,
        reasoning_effort,
        auto_compact: settings.auto_compact,
        cost_currency,
        default_mode,
        default_model,
        base_url,
        allow_shell: config.allow_shell(),
        mcp_config_path,
        subagents_enabled: config.subagents_enabled(),
        subagents_max_depth: config.subagent_max_spawn_depth(),
        show_thinking: settings.show_thinking,
        thinking_default_expanded: settings.thinking_default_expanded,
        thinking_highlight: settings.thinking_highlight,
        show_tool_details: settings.show_tool_details,
        inline_diffs: settings.inline_diffs.clone(),
        locale: settings.locale.clone(),
        max_history: settings.max_input_history,
        workspace_follow_symlinks: settings.workspace_follow_symlinks,
        calm_mode: settings.calm_mode,
        sandbox_mode: config
            .sandbox_mode
            .clone()
            .unwrap_or_else(|| "workspace-write".to_string()),
        strict_tool_mode: config.strict_tool_mode.unwrap_or(false),
        memory_enabled: config.memory_enabled(),
        search_provider: config.search_provider().as_str().to_string(),
        prompt_suggestion: config.prompt_suggestion_enabled(),
    }))
}

async fn set_config(
    State(state): State<RuntimeApiState>,
    Json(req): Json<SetConfigRequest>,
) -> Result<Json<SetConfigResponse>, ApiError> {
    use crate::config_persistence;

    let key = req.key.to_lowercase();
    let mut value = req.value;
    let persist = req.persist;

    // Validate model keys even for dry-run requests. Model ids are provider
    // owned; accepting a DeepSeek id while Z.ai is active creates a saved
    // route that cannot execute after reload.
    let active_route = {
        let config = state.config.read();
        let provider = config.api_provider();
        (provider, config.provider_identity_for(provider))
    };
    match key.as_str() {
        "model" => {
            value = normalize_runtime_config_model(active_route.0, &value)?;
        }
        "default_model" => {
            value = normalize_runtime_config_model(ApiProvider::Deepseek, &value)?;
        }
        _ => {}
    }

    // All persisted config keys require a reload to take effect in the
    // runtime (including syncing to active engines). The caller should
    // POST /v1/config/reload after persisting.
    let requires_reload = persist;

    // Handle persistence directly via config_persistence.
    // The runtime's in-memory state is NOT mutated here; the caller
    // should POST /v1/config/reload after persisting to apply changes.
    if persist {
        let config_path = state.config_path.as_deref();
        let result: anyhow::Result<PathBuf> = match key.as_str() {
            "model" => config_persistence::persist_provider_model_key(
                config_path,
                active_route.0,
                &active_route.1,
                &value,
            ),
            "default_model" => config_persistence::persist_root_string_key(
                config_path,
                "default_text_model",
                &value,
            ),
            "reasoning_effort" => {
                config_persistence::persist_root_string_key(config_path, "reasoning_effort", &value)
            }
            "approval_mode" | "approval_policy" => {
                config_persistence::persist_root_string_key(config_path, "approval_policy", &value)
            }
            "base_url" => config_persistence::persist_root_string_key(
                config_path,
                "deepseek_base_url",
                &value,
            ),
            "provider" => {
                // Validate the provider id against the static registry so the
                // GUI gets a clear error instead of silently persisting an
                // unknown value that `Config::api_provider()` would later
                // ignore (falling back to DeepSeek).
                let parsed = ApiProvider::parse(&value).ok_or_else(|| {
                    ApiError::bad_request(format!(
                        "Unknown provider '{value}'. Call GET /v1/providers for the list of supported ids."
                    ))
                })?;
                let result =
                    config_persistence::persist_root_string_key(config_path, "provider", &value);
                if result.is_ok() {
                    // Keep the in-memory provider in step with the persisted
                    // value so a following set_config(model) resolves the new
                    // provider's table instead of clobbering the previous
                    // provider's root default_text_model (#4658 follow-up).
                    state.config.write().provider = Some(parsed.as_str().to_string());
                }
                result
            }
            "provider_url" | "provider_base_url" => {
                let provider = state.config.read().api_provider();
                config_persistence::persist_provider_base_url_key(config_path, provider, &value)
            }
            "cost_currency"
            | "default_mode"
            | "auto_compact"
            | "show_thinking"
            | "thinking_default_expanded"
            | "thinking_highlight"
            | "show_tool_details"
            | "inline_diffs"
            | "calm_mode"
            | "workspace_follow_symlinks"
            | "locale"
            | "max_history" => {
                persist_runtime_tui_setting(&key, &value)?;
                return Ok(Json(SetConfigResponse {
                    key,
                    value,
                    message: "Config persisted. Call /v1/config/reload to apply.".to_string(),
                    persisted: true,
                    requires_reload,
                }));
            }
            "allow_shell" => {
                let enabled = value.parse::<bool>().map_err(|_| {
                    ApiError::bad_request(format!(
                        "Invalid value '{value}' for allow_shell: expected 'true' or 'false'"
                    ))
                })?;
                config_persistence::persist_root_bool_key(config_path, "allow_shell", enabled)
            }
            "mcp_config_path" => {
                config_persistence::persist_root_string_key(config_path, "mcp_config_path", &value)
            }
            "subagents_enabled" => {
                let enabled = value.parse::<bool>().map_err(|_| {
                    ApiError::bad_request(format!(
                        "Invalid value '{value}' for subagents_enabled: expected 'true' or 'false'"
                    ))
                })?;
                config_persistence::persist_subagents_bool_key(config_path, "enabled", enabled)
            }
            "subagents_max_depth" => {
                let raw = value.parse::<u64>().map_err(|_| {
                    ApiError::bad_request(format!(
                        "Invalid value '{value}' for subagents_max_depth: expected a non-negative integer"
                    ))
                })?;
                let clamped = raw.min(u64::from(codewhale_config::MAX_SPAWN_DEPTH_CEILING));
                config_persistence::persist_subagents_integer_key(config_path, "max_depth", clamped)
            }
            "sandbox_mode" => {
                let normalized = match value.to_lowercase().as_str() {
                    "none" | "off" | "disabled" => "none".to_string(),
                    "opensandbox" | "external-sandbox" | "external" => "opensandbox".to_string(),
                    "workspace-write" | "workspace_write" => "workspace-write".to_string(),
                    "read-only" | "read_only" => "read-only".to_string(),
                    "danger-full-access" | "danger_full_access" | "full" => {
                        "danger-full-access".to_string()
                    }
                    "workspace" | "workspace-read-write" | "workspace_read_write" => {
                        "workspace-write".to_string()
                    }
                    _ => {
                        return Err(ApiError::bad_request(format!(
                            "Invalid sandbox_mode '{value}'. Supported: none, read-only, workspace-write, danger-full-access, opensandbox"
                        )));
                    }
                };
                config_persistence::persist_root_string_key(
                    config_path,
                    "sandbox_mode",
                    &normalized,
                )
            }
            "strict_tool_mode" => {
                let enabled = value.parse::<bool>().map_err(|_| {
                    ApiError::bad_request(format!(
                        "Invalid value '{value}' for strict_tool_mode: expected 'true' or 'false'"
                    ))
                })?;
                config_persistence::persist_root_bool_key(config_path, "strict_tool_mode", enabled)
            }
            "memory_enabled" => {
                let enabled = value.parse::<bool>().map_err(|_| {
                    ApiError::bad_request(format!(
                        "Invalid value '{value}' for memory_enabled: expected 'true' or 'false'"
                    ))
                })?;
                config_persistence::persist_table_bool_key(
                    config_path,
                    "memory",
                    "enabled",
                    enabled,
                )
            }
            "search_provider" => {
                let normalized = value.to_lowercase();
                config_persistence::persist_table_string_key(
                    config_path,
                    "search",
                    "provider",
                    &normalized,
                )
            }
            "prompt_suggestion" => {
                let enabled = value.parse::<bool>().map_err(|_| {
                    ApiError::bad_request(format!(
                        "Invalid value '{value}' for prompt_suggestion: expected 'true' or 'false'"
                    ))
                })?;
                config_persistence::persist_root_bool_key(config_path, "prompt_suggestion", enabled)
            }
            _ => {
                return Err(ApiError::bad_request(format!(
                    "Unknown config key '{key}'. Supported keys: model, default_model, reasoning_effort, approval_mode, base_url, provider, provider_url, cost_currency, default_mode, auto_compact, allow_shell, mcp_config_path, show_thinking, thinking_default_expanded, thinking_highlight, show_tool_details, inline_diffs, locale, max_history, calm_mode, workspace_follow_symlinks, subagents_enabled, subagents_max_depth, sandbox_mode, strict_tool_mode, memory_enabled, search_provider, prompt_suggestion"
                )));
            }
        };

        if let Err(e) = result {
            return Err(ApiError::internal(format!(
                "Failed to persist config key '{key}': {e}"
            )));
        }
    }

    Ok(Json(SetConfigResponse {
        key,
        value,
        message: if persist {
            "Config persisted. Call /v1/config/reload to apply.".to_string()
        } else {
            "Config not persisted (add persist: true to save)".to_string()
        },
        persisted: persist,
        requires_reload,
    }))
}

fn normalize_runtime_config_model(provider: ApiProvider, value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    validate_route(provider, value).map_err(ApiError::bad_request)?;
    if value.eq_ignore_ascii_case("auto") {
        return Ok("auto".to_string());
    }
    normalize_model_name_for_provider(provider, value).ok_or_else(|| {
        ApiError::bad_request(format!(
            "Invalid model '{value}' for provider '{}'.",
            provider.as_str()
        ))
    })
}

async fn reload_config(
    State(state): State<RuntimeApiState>,
) -> Result<Json<ReloadConfigResponse>, ApiError> {
    let reloaded = Config::load(state.config_path.clone(), state.config_profile.as_deref())
        .map_err(|e| ApiError::internal(format!("Failed to reload config: {e}")))?;
    state
        .runtime_threads
        .reload_config(reloaded.clone())
        .await
        .map_err(|err| ApiError::bad_request(format!("Config reload rejected: {err}")))?;
    {
        let mut config = state.config.write();
        *config = reloaded;
    }
    Ok(Json(ReloadConfigResponse {
        message: "Config reloaded from disk; new turns will resolve the updated provider routes"
            .to_string(),
    }))
}

// ── Memory inspection and lifecycle endpoints ──

/// Maximum summary length returned per entry. Bounds the API surface so raw
/// private text cannot exfiltrate through JSON responses.
const MEMORY_SUMMARY_MAX_CHARS: usize = 300;
/// Default result cap for `GET /v1/memory`.
const MEMORY_LIST_DEFAULT_LIMIT: usize = 50;
/// Hard ceiling — protects against oversized responses.
const MEMORY_LIST_MAX_LIMIT: usize = 200;

/// Typed, redacted projection of a single native memory entry.
///
/// Raw file-system paths are never exposed; `scope` and `workspace_id` (a
/// SHA-256 digest of the repository origin URL, not a local path) give
/// managed clients enough provenance to reason about each entry.
#[derive(Debug, Serialize)]
struct MemoryEntryRecord {
    /// SQLite row id. Stable across reindexes unless the source Markdown
    /// file is cleared and rewritten.
    id: i64,
    /// `"global"` or `"workspace"`.
    scope: &'static str,
    /// SHA-256 digest of the repository origin URL for workspace-scoped
    /// entries; `null` for global entries.
    workspace_id: Option<String>,
    /// Bounded plain-text summary (max `MEMORY_SUMMARY_MAX_CHARS` chars).
    /// Truncated with `…` when the source text is longer. Never contains
    /// raw prompt or turn content.
    summary: String,
    /// `true` when the source Markdown file has been modified since the
    /// entry was last indexed.
    stale: bool,
    /// 1-based start line in the source Markdown file.
    line_start: usize,
    /// 1-based end line in the source Markdown file.
    line_end: usize,
    /// `"active"` or `"stale"` (human-readable alias for `stale`).
    status: &'static str,
}

#[derive(Debug, Deserialize)]
struct ListMemoryQuery {
    /// Filter by scope: `"global"`, `"workspace"`, or `"all"` (default).
    scope: Option<String>,
    /// FTS search query (max 256 chars). When absent all entries for the
    /// requested scope are returned in insertion order.
    q: Option<String>,
    /// Maximum entries to return (default 50, max 200).
    limit: Option<usize>,
}

/// Request body for `POST /v1/memory`.
#[derive(Debug, Deserialize)]
struct CreateMemoryRequest {
    /// The memory note text (max 64 KiB after normalisation).
    text: String,
    /// `"global"` (default) or `"workspace"`.
    #[serde(default)]
    scope: String,
}

/// Query params for `DELETE /v1/memory`.
#[derive(Debug, Deserialize)]
struct ClearMemoryQuery {
    /// One of `"global"`, `"workspace"`, or `"all"`. Required.
    scope: String,
}

/// Build a `NativeMemoryStore` rooted at the same location the TUI uses.
/// Mirrors `native_store()` in `commands/groups/memory/memory.rs`.
fn native_store_for_state(state: &RuntimeApiState) -> crate::native_memory::NativeMemoryStore {
    let memory_path = state.config.read().memory_path();
    if let Some(store) = crate::native_memory::NativeMemoryStore::from_global_path(&memory_path) {
        return store;
    }
    let root = memory_path
        .parent()
        .unwrap_or_else(|| FsPath::new("."))
        .join("memory");
    crate::native_memory::NativeMemoryStore::new(root)
}

/// Derive a scope label from a source path relative to the store root.
/// Returns `"global"`, `"workspace"`, or `"unknown"`.
fn scope_label_for_source(source: &FsPath, store_root: &FsPath) -> &'static str {
    let Ok(rel) = source.strip_prefix(store_root) else {
        return "unknown";
    };
    match rel.components().next().and_then(|c| c.as_os_str().to_str()) {
        Some("global") => "global",
        Some("workspace") => "workspace",
        _ => "unknown",
    }
}

/// Extract the workspace_id component from a workspace-scoped source path.
fn workspace_id_for_source(source: &FsPath, store_root: &FsPath) -> Option<String> {
    let rel = source.strip_prefix(store_root).ok()?;
    let mut comps = rel.components();
    if comps.next()?.as_os_str().to_str()? != "workspace" {
        return None;
    }
    Some(comps.next()?.as_os_str().to_str()?.to_string())
}

/// Convert a `MemoryHit` into a redacted, bounded `MemoryEntryRecord`.
fn memory_hit_to_record(
    hit: crate::native_memory::MemoryHit,
    store_root: &FsPath,
) -> MemoryEntryRecord {
    let scope = scope_label_for_source(&hit.source, store_root);
    let workspace_id = workspace_id_for_source(&hit.source, store_root);
    let summary = truncate_text(&hit.text, MEMORY_SUMMARY_MAX_CHARS);
    let status = if hit.stale { "stale" } else { "active" };
    MemoryEntryRecord {
        id: hit.id,
        scope,
        workspace_id,
        summary,
        stale: hit.stale,
        line_start: hit.line_start,
        line_end: hit.line_end,
        status,
    }
}

/// Resolve a scope query parameter into a `MemoryScope` filter and an
/// optional workspace_id.  `"all"` / absent → `(None, None)`.
fn resolve_memory_scope(
    scope_param: &Option<String>,
    workspace: &FsPath,
) -> Result<(Option<crate::native_memory::MemoryScope>, Option<String>), ApiError> {
    match scope_param.as_deref().unwrap_or("all").trim() {
        "all" | "" => Ok((None, None)),
        "global" => Ok((Some(crate::native_memory::MemoryScope::Global), None)),
        "workspace" => {
            let workspace_id = crate::native_memory::NativeMemoryStore::workspace_id(workspace)
                .map_err(|e| ApiError::internal(format!("resolve workspace id: {e}")))?;
            Ok((
                Some(crate::native_memory::MemoryScope::Workspace),
                workspace_id,
            ))
        }
        other => Err(ApiError::bad_request(format!(
            "Invalid scope '{other}': expected one of all, global, workspace"
        ))),
    }
}

/// `GET /v1/memory` — list memory entries with optional scope and FTS
/// filtering.
///
/// Query params:
/// - `scope` — `"global"`, `"workspace"`, or `"all"` (default)
/// - `q` — FTS search query (max 256 chars; omit to list all)
/// - `limit` — max results (default 50, max 200)
async fn list_memory(
    State(state): State<RuntimeApiState>,
    Query(query): Query<ListMemoryQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = match query.limit.unwrap_or(MEMORY_LIST_DEFAULT_LIMIT) {
        0 => {
            return Err(ApiError::bad_request("limit must be at least 1"));
        }
        n if n > MEMORY_LIST_MAX_LIMIT => {
            return Err(ApiError::bad_request(format!(
                "limit must be at most {MEMORY_LIST_MAX_LIMIT}; got {n}"
            )));
        }
        n => n,
    };

    let store = native_store_for_state(&state);
    let root = store.root().to_path_buf();
    let (scope_filter, workspace_id) = resolve_memory_scope(&query.scope, &state.workspace)?;

    let hits = if let Some(ref q) = query.q {
        let q = q.trim();
        if q.is_empty() || q.chars().count() > 256 {
            return Err(ApiError::bad_request("q must be 1–256 characters"));
        }
        match scope_filter {
            None => store.search(q, limit),
            Some(crate::native_memory::MemoryScope::Global) => store.search(q, limit).map(|h| {
                h.into_iter()
                    .filter(|h| scope_label_for_source(&h.source, &root) == "global")
                    .collect()
            }),
            Some(crate::native_memory::MemoryScope::Workspace) => store
                .search_for_workspace(&state.workspace, q, limit)
                .map(|h| {
                    h.into_iter()
                        .filter(|h| scope_label_for_source(&h.source, &root) == "workspace")
                        .collect()
                }),
        }
    } else {
        store.list_all(scope_filter, workspace_id.as_deref(), limit)
    }
    .map_err(|e| ApiError::internal(format!("memory list error: {e}")))?;

    let entries: Vec<MemoryEntryRecord> = hits
        .into_iter()
        .map(|h| memory_hit_to_record(h, &root))
        .collect();
    let total = entries.len();
    Ok(Json(json!({ "entries": entries, "total": total })))
}

/// `GET /v1/memory/{id}` — inspect a single memory entry.
///
/// The lookup is scoped to global memory plus the current repository's
/// workspace memory; numeric IDs from a different machine or repository
/// will not resolve.
async fn get_memory_entry(
    State(state): State<RuntimeApiState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let store = native_store_for_state(&state);
    let root = store.root().to_path_buf();
    let hit = store
        .get_for_workspace(&state.workspace, id)
        .map_err(|e| ApiError::internal(format!("memory lookup error: {e}")))?
        .ok_or_else(|| ApiError::not_found(format!("memory entry '{id}' not found")))?;
    let entry = memory_hit_to_record(hit, &root);
    Ok(Json(json!({ "entry": entry })))
}

/// `POST /v1/memory` — append a new memory entry.
///
/// The note is treated as user data (lower authority than instructions).
/// Requires the standard Runtime auth token when auth is configured.
async fn create_memory_entry(
    State(state): State<RuntimeApiState>,
    Json(req): Json<CreateMemoryRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let scope_str = if req.scope.is_empty() {
        "global"
    } else {
        req.scope.as_str()
    };
    let scope = match scope_str.trim() {
        "global" => crate::native_memory::MemoryScope::Global,
        "workspace" => crate::native_memory::MemoryScope::Workspace,
        other => {
            return Err(ApiError::bad_request(format!(
                "Invalid scope '{other}': expected 'global' or 'workspace'"
            )));
        }
    };
    let workspace_id = if scope == crate::native_memory::MemoryScope::Workspace {
        let id = crate::native_memory::NativeMemoryStore::workspace_id(&state.workspace)
            .map_err(|e| ApiError::internal(format!("resolve workspace id: {e}")))?
            .ok_or_else(|| {
                ApiError::bad_request(
                    "workspace scope requires a git repository with a remote origin",
                )
            })?;
        Some(id)
    } else {
        None
    };
    let store = native_store_for_state(&state);
    let root = store.root().to_path_buf();
    let hit = store
        .remember(scope, workspace_id.as_deref(), &req.text)
        .map_err(|e| ApiError::bad_request(format!("memory create error: {e}")))?;
    let entry = memory_hit_to_record(hit, &root);
    Ok((StatusCode::CREATED, Json(json!({ "entry": entry }))))
}

/// `DELETE /v1/memory` — clear all memory entries for the given scope.
///
/// The `scope` query parameter is required: `"global"`, `"workspace"`, or
/// `"all"`.  This is a destructive, non-reversible operation.
async fn clear_memory(
    State(state): State<RuntimeApiState>,
    Query(query): Query<ClearMemoryQuery>,
) -> Result<Json<Value>, ApiError> {
    let (scope_filter, workspace_id) = resolve_memory_scope(&Some(query.scope), &state.workspace)?;
    let store = native_store_for_state(&state);
    store
        .delete_all(scope_filter, workspace_id.as_deref())
        .map_err(|e| ApiError::internal(format!("memory clear error: {e}")))?;
    Ok(Json(json!({ "cleared": true })))
}

const MOBILE_HTML: &str = include_str!("runtime_mobile.html");

/// Built-in dev origins always allowed by the runtime API (whalescale#255).
const DEFAULT_CORS_ORIGINS: &[&str] = &[
    "http://localhost:3000",
    "http://127.0.0.1:3000",
    "http://localhost:1420",
    "http://127.0.0.1:1420",
    "tauri://localhost",
];

fn cors_layer(extra_origins: &[String]) -> CorsLayer {
    let mut origins: Vec<HeaderValue> = DEFAULT_CORS_ORIGINS
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();
    for raw in extra_origins {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        match HeaderValue::from_str(trimmed) {
            Ok(value) if !origins.contains(&value) => origins.push(value),
            Ok(_) => {}
            Err(err) => tracing::warn!(
                "Ignoring invalid CORS origin '{trimmed}': {err}; expected scheme://host[:port]"
            ),
        }
    }
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
            HeaderName::from_static("x-codewhale-runtime-token"),
            HeaderName::from_static("x-deepseek-runtime-token"),
        ])
}

fn map_task_err(err: anyhow::Error) -> ApiError {
    let message = err.to_string();
    if message.contains("not found") {
        ApiError::not_found(message)
    } else {
        ApiError::bad_request(message)
    }
}

fn map_automation_err(err: anyhow::Error) -> ApiError {
    let message = err.to_string();
    if message.contains("Failed to read automation")
        || message.contains("No such file or directory")
    {
        ApiError::not_found(message)
    } else {
        ApiError::bad_request(message)
    }
}

fn map_thread_err(err: anyhow::Error) -> ApiError {
    let message = err.to_string();
    let lower = message.to_ascii_lowercase();
    if (lower.starts_with("thread '") && lower.ends_with("' not found"))
        || lower.starts_with("thread not found:")
    {
        ApiError::not_found(message)
    } else if message.contains("already has an active turn")
        || message.contains("No active turn")
        || message.contains("is not active")
        || lower.contains("operation_key is already bound")
        || lower.contains("operation_key binding is incomplete")
        || lower.contains("operation_key binding does not match")
    {
        ApiError::conflict(message)
    } else {
        ApiError::bad_request(message)
    }
}

fn map_agent_mail_err(err: anyhow::Error) -> ApiError {
    let message = err.to_string();
    let lower = message.to_ascii_lowercase();
    if lower.contains("ownership denied") {
        ApiError::forbidden(message)
    } else if lower.contains("already exists with different delivery intent") {
        ApiError::conflict(message)
    } else if (lower.contains("failed to read agent mail envelope")
        && lower.contains("no such file"))
        || (lower.starts_with("thread '") && lower.ends_with("' not found"))
    {
        ApiError::not_found(message)
    } else {
        ApiError::bad_request(message)
    }
}

#[derive(Debug, Clone)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    fn not_implemented(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "error": {
                    "message": self.message,
                    "status": self.status.as_u16(),
                }
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests;
