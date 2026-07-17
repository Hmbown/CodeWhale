//! Runtime HTTP/SSE API for local Codewhale automation.

use std::convert::Infallible;
use std::fs;
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_stream::stream;
use axum::extract::{Path, Query, Request, State};
#[cfg(test)]
use axum::http::header;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware;
use axum::response::Html;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use codewhale_protocol::runtime::{
    DynamicToolCallResult, RUNTIME_API_VERSION, RUNTIME_EVENT_ENVELOPE_SCHEMA_VERSION,
    RuntimeCapabilities, RuntimeEventEnvelope, RuntimeExperimentalCapabilities,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tower_http::cors::{Any, CorsLayer};

#[cfg(test)]
use crate::dependencies::ExternalTool;

use crate::automation_manager::{
    AutomationManager, AutomationRecord, AutomationRunRecord, AutomationSchedulerConfig,
    CreateAutomationRequest, SharedAutomationManager, UpdateAutomationRequest, spawn_scheduler,
};
use crate::config::{Config, DEFAULT_TEXT_MODEL};
use crate::fleet::executor::{FleetExecutor, configured_codewhale_binary};
use crate::fleet::ledger::{FleetLedgerState, FleetTaskLedgerStatus};
use crate::fleet::manager::{
    FleetManager, FleetStatusSnapshot, FleetWorkerInspection, FleetWorkerRuntimeProjection,
};
use crate::mcp::McpPool;
#[cfg(test)]
pub(super) use crate::models::{ContentBlock, Message};
use crate::runtime_threads::{
    CompactThreadRequest, CreateThreadRequest, ExternalApprovalDecision, RuntimeThreadManager,
    RuntimeThreadManagerConfig, SharedRuntimeThreadManager, StartTurnRequest, SteerTurnRequest,
    ThreadDetail, ThreadListFilter, ThreadRecord, TurnItemKind, TurnRecord, UpdateThreadRequest,
    UsageGroupBy,
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
    FleetArtifactKind, FleetRun, FleetRunId, FleetWorkerEventPayload, FleetWorkerStatus,
};

mod auth;
mod sessions;
mod workspace;
#[cfg(test)]
use self::auth::{ResolvedRuntimeAuth, token_from_cookie_header};
use self::auth::{require_runtime_token, resolve_runtime_auth, runtime_auth_status_lines};
use self::sessions::{
    create_session_from_thread, delete_session, get_session, list_sessions, resume_session_thread,
    save_current_session,
};
#[cfg(test)]
use self::sessions::{messages_from_thread_detail, session_to_detail};
#[cfg(test)]
use self::workspace::collect_workspace_status;
use self::workspace::{collect_workspace_git_metadata, workspace_status};

#[derive(Clone)]
pub struct RuntimeApiState {
    config: Arc<parking_lot::RwLock<Config>>,
    workspace: PathBuf,
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
    /// Executable used by Runtime API-owned Fleet manager loops. Stored on
    /// state so tests and embedded callers can provide a hermetic worker.
    fleet_codewhale_binary: String,
    /// Shared McpPool reused for explicit live MCP discovery. Passive API
    /// calls do not initialize this pool so dashboards cannot accidentally
    /// become a second stdio-process owner. The outer mutex guards only the
    /// lazily-initialized slot; slow per-pool work (connect_all) runs under
    /// the inner handle so it cannot block slot reads.
    mcp_pool: Arc<Mutex<Option<Arc<Mutex<McpPool>>>>>,
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
}

#[derive(Debug, Serialize)]
struct SkillEntry {
    name: String,
    description: String,
    path: PathBuf,
    enabled: bool,
    is_bundled: bool,
    readiness: crate::skills::SkillReadiness,
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
    bind_host: String,
    port: u16,
    auth_required: bool,
    transports: Vec<&'static str>,
    capabilities: RuntimeCapabilities,
    experimental: RuntimeExperimentalCapabilities,
    // Backward-compatible alias kept for existing clients.
    version: &'static str,
}

fn default_runtime_capabilities() -> RuntimeCapabilities {
    RuntimeCapabilities {
        threads: true,
        turns: true,
        turn_steer: true,
        turn_interrupt: true,
        event_replay: true,
        external_tools: true,
        environments: false,
        worker_runtime: true,
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

#[derive(Debug, Deserialize)]
struct AutomationRunsQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ThreadEventsQuery {
    since_seq: Option<u64>,
    replay_limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct StartTurnResponse {
    thread: ThreadRecord,
    turn: TurnRecord,
}

/// Start the runtime API server.
pub async fn run_http_server(
    config: Config,
    workspace: PathBuf,
    options: RuntimeApiOptions,
) -> Result<()> {
    if options.port == 0 {
        bail!("Port must be > 0");
    }

    let task_cfg = TaskManagerConfig::from_runtime(
        &config,
        workspace.clone(),
        config.default_text_model.clone(),
        Some(options.workers),
    );
    let runtime_threads = Arc::new(RuntimeThreadManager::open(
        config.clone(),
        workspace.clone(),
        RuntimeThreadManagerConfig::from_task_data_dir(task_cfg.data_dir.clone()),
    )?);
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

    let sessions_dir = default_sessions_dir().unwrap_or_else(|_| {
        dirs::home_dir()
            .map(|h| h.join(".deepseek").join("sessions"))
            .unwrap_or_else(|| PathBuf::from(".deepseek").join("sessions"))
    });
    let runtime_token_env = std::env::var("CODEWHALE_RUNTIME_TOKEN")
        .ok()
        .or_else(|| std::env::var("DEEPSEEK_RUNTIME_TOKEN").ok());
    let resolved_auth = resolve_runtime_auth(
        options.auth_token.clone(),
        runtime_token_env,
        options.insecure_no_auth,
    );
    let runtime_token = resolved_auth.token.clone();
    let auth_enabled = runtime_token.is_some();
    let skill_state = SkillStateStore::load_default().unwrap_or_else(|err| {
        tracing::warn!(
            "Failed to load skills_state.toml ({}); treating all skills as enabled",
            err
        );
        SkillStateStore::default()
    });
    let sub_agent_manager = runtime_api_sub_agent_manager(&workspace, options.workers);
    let state = RuntimeApiState {
        config: Arc::new(parking_lot::RwLock::new(config.clone())),
        workspace,
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
        fleet_codewhale_binary: configured_codewhale_binary(),
        mcp_pool: Arc::new(Mutex::new(None)),
    };
    let app = build_router(state);

    let addr: SocketAddr = format!("{}:{}", options.host, options.port)
        .parse()
        .with_context(|| format!("Invalid bind address '{}:{}'", options.host, options.port))?;
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("Failed to bind {addr}"))?;

    println!("Runtime API listening on http://{addr}");
    for line in runtime_auth_status_lines(&resolved_auth) {
        println!("{line}");
    }
    if options.mobile {
        print_mobile_urls(addr, auth_enabled, resolved_auth.generated, options.show_qr);
    }
    let is_loopback = options.host == "127.0.0.1" || options.host == "::1";
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
    let serve_result = axum::serve(listener, app)
        .await
        .map_err(|e| anyhow!("Runtime API server error: {e}"));
    scheduler_cancel.cancel();
    scheduler_handle.abort();
    serve_result
}

pub fn build_router(state: RuntimeApiState) -> Router {
    let api_routes = Router::new()
        .route(
            "/v1/sessions",
            get(list_sessions)
                .post(create_session_from_thread)
                .put(save_current_session),
        )
        .route("/v1/sessions/{id}", get(get_session).delete(delete_session))
        .route(
            "/v1/sessions/{id}/resume-thread",
            post(resume_session_thread),
        )
        .route("/v1/workspace/status", get(workspace_status))
        .route("/v1/agent-runs", get(list_agent_runs))
        .route("/v1/agent-runs/{run_id}", get(get_agent_run))
        .route("/v1/fleet/runs", get(list_fleet_runs))
        .route("/v1/fleet/runs/{run_id}", get(get_fleet_run))
        .route(
            "/v1/fleet/runs/{run_id}/workers",
            get(list_fleet_run_workers),
        )
        .route("/v1/fleet/runs/{run_id}/stop", post(stop_fleet_run))
        .route("/v1/fleet/workers/{worker_id}", get(get_fleet_worker))
        .route(
            "/v1/fleet/workers/{worker_id}/interrupt",
            post(interrupt_fleet_worker),
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
        .route("/v1/threads/{id}/events", get(stream_thread_events))
        .route("/v1/approvals/{approval_id}", post(decide_approval))
        .route(
            "/v1/user-input/{thread_id}/{input_id}",
            post(submit_user_input),
        )
        .route("/v1/tasks", get(list_tasks).post(create_task))
        .route("/v1/tasks/{id}", get(get_task))
        .route("/v1/tasks/{id}/cancel", post(cancel_task))
        .route("/v1/skills", get(list_skills))
        .route("/v1/skills/{name}", post(set_skill_enabled))
        .route("/v1/apps/mcp/servers", get(list_mcp_servers))
        .route("/v1/apps/mcp/tools", get(list_mcp_tools))
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
        .route("/v1/usage", get(get_usage))
        .route("/v1/snapshots", get(list_snapshots))
        .route("/v1/snapshots/{id}/restore", post(restore_snapshot))
        .route("/v1/config", get(get_config).post(set_config))
        .route("/v1/config/reload", post(reload_config))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_runtime_token,
        ));

    Router::new()
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
    Html(MOBILE_HTML).into_response()
}

fn print_mobile_urls(addr: SocketAddr, auth_enabled: bool, generated_auth: bool, show_qr: bool) {
    println!("Mobile control page enabled.");

    let port = addr.port();
    let qr_url = if addr.ip().is_unspecified() {
        println!("  Local: http://127.0.0.1:{port}/mobile");
        if let Some(ip) = detect_lan_ip() {
            let lan_url = format!("http://{ip}:{port}/mobile");
            println!("  LAN:   {lan_url}");
            lan_url
        } else {
            println!("  LAN:   bind is 0.0.0.0; open http://<this-machine-ip>:{port}/mobile");
            format!("http://127.0.0.1:{port}/mobile")
        }
    } else {
        let url = format!("http://{addr}/mobile");
        println!("  URL:   {url}");
        url
    };
    if auth_enabled {
        if generated_auth {
            println!(
                "  Auth uses an unprinted generated token; restart with CODEWHALE_RUNTIME_TOKEN or --auth-token to sign in from another client."
            );
        } else {
            println!("  Enter the configured runtime token in the page connection field.");
        }
    }
    println!("Mobile security: use only on a trusted LAN/VPN; this server does not provide TLS.");

    if show_qr {
        match qrcode::QrCode::new(qr_url.as_bytes()) {
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

#[cfg(test)]
fn url_query_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                use std::fmt::Write as _;
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
    }
    encoded
}

fn detect_lan_ip() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    // UDP connect only selects the outbound interface locally; no packet is sent.
    socket.connect("10.255.255.255:1").ok()?;
    let addr = socket.local_addr().ok()?;
    Some(addr.ip().to_string())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "codewhale-runtime-api",
        mode: "local",
    })
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
        req.model = Some(
            state
                .config
                .read()
                .default_text_model
                .clone()
                .unwrap_or_else(|| DEFAULT_TEXT_MODEL.to_string()),
        );
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
    let threads = state
        .runtime_threads
        .list_threads(filter, Some(limit))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let mut summaries = Vec::new();
    for thread in threads {
        let detail = state
            .runtime_threads
            .get_thread_detail(&thread.id)
            .await
            .map_err(map_thread_err)?;
        let latest_turn = detail.turns.last();
        let latest_status =
            latest_turn.map(|turn| format!("{:?}", turn.status).to_ascii_lowercase());

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

        if let Some(search) = &search {
            let haystack = format!(
                "{} {} {} {}",
                thread.id.to_ascii_lowercase(),
                title.to_ascii_lowercase(),
                preview.to_ascii_lowercase(),
                thread.model.to_ascii_lowercase()
            );
            if !haystack.contains(search) {
                continue;
            }
        }

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
        });
    }

    if summaries.len() > limit {
        summaries.truncate(limit);
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

async fn list_fleet_runs(State(state): State<RuntimeApiState>) -> Result<Json<Value>, ApiError> {
    let manager = open_fleet_manager(&state)?;
    let ledger_state = manager
        .rebuild_state()
        .map_err(|err| ApiError::internal(format!("Failed to rebuild fleet state: {err}")))?;
    let runs: Vec<_> = ledger_state
        .runs
        .values()
        .map(|run| fleet_run_summary_json(&manager, run, &ledger_state))
        .collect::<Result<Vec<_>, _>>()?;
    let status = manager
        .status()
        .map_err(|err| ApiError::internal(format!("Failed to read fleet status: {err}")))?;
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
        .map_err(|err| ApiError::internal(format!("Failed to rebuild fleet state: {err}")))?;
    let run = ledger_state
        .runs
        .get(&run_id)
        .ok_or_else(|| ApiError::not_found(format!("fleet run '{run_id}' not found")))?;
    Ok(Json(fleet_run_detail_json(&manager, run, &ledger_state)?))
}

async fn list_fleet_run_workers(
    State(state): State<RuntimeApiState>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let manager = open_fleet_manager(&state)?;
    let ledger_state = manager
        .rebuild_state()
        .map_err(|err| ApiError::internal(format!("Failed to rebuild fleet state: {err}")))?;
    let run = ledger_state
        .runs
        .get(&run_id)
        .ok_or_else(|| ApiError::not_found(format!("fleet run '{run_id}' not found")))?;
    let workers = run
        .worker_specs
        .iter()
        .map(|worker| {
            manager
                .inspect_worker(&worker.id)
                .map(|inspection| fleet_worker_json(&inspection))
                .map_err(|err| {
                    ApiError::internal(format!(
                        "Failed to inspect fleet worker {}: {err}",
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
        ApiError::not_found(format!("fleet worker '{worker_id}' not found: {err}"))
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
            "Failed to interrupt fleet worker '{worker_id}': {err}"
        ))
    })?;
    Ok(Json(json!({
        "action": "interrupt",
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
            "Failed to restart fleet worker '{worker_id}': {err}"
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
        ApiError::bad_request(format!("Failed to stop fleet run '{}': {err}", run_id.0))
    })?;
    let status = manager
        .run_status(&run_id)
        .map_err(|err| ApiError::internal(format!("Failed to read fleet run status: {err}")))?;
    Ok(Json(json!({
        "action": "stop",
        "run_id": run_id.0,
        "stopped": stopped,
        "status": fleet_status_json(&status),
    })))
}

fn open_fleet_manager(state: &RuntimeApiState) -> Result<FleetManager, ApiError> {
    let (exec_config, session_model, route_config) = {
        let config = state.config.read();
        let exec_config = config
            .fleet
            .as_ref()
            .map(|fleet| fleet.exec.clone())
            .unwrap_or_default();
        // The active session route is the operator: workers without a
        // task/profile model pin inherit the model the user picked in /model.
        (exec_config, config.default_model(), config.clone())
    };
    FleetManager::open(&state.workspace)
        .map(|manager| {
            manager
                .with_exec_config(exec_config)
                .with_sub_agent_manager(state.sub_agent_manager.clone())
                .with_session_model(session_model)
                .with_route_config(route_config)
        })
        .map_err(|err| ApiError::internal(format!("Failed to open fleet manager: {err}")))
}

fn fleet_run_summary_json(
    manager: &FleetManager,
    run: &FleetRun,
    ledger_state: &FleetLedgerState,
) -> Result<Value, ApiError> {
    let status = manager
        .run_status(&run.id)
        .map_err(|err| ApiError::internal(format!("Failed to read fleet run status: {err}")))?;
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
        "status": fleet_status_json(&status),
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
    let (registry, directories) =
        discover_skills_for_runtime_api(&state.workspace, &skills_dir, mode);
    let skill_state = state.skill_state.lock().await;
    let skills = registry
        .list()
        .iter()
        .map(|skill| SkillEntry {
            name: skill.name.clone(),
            description: skill.description.clone(),
            path: skill.path.clone(),
            enabled: skill_state.is_enabled(&skill.name),
            is_bundled: skill_entry_is_bundled(skill, &skills_dir),
            readiness: crate::skills::probe::probe(&skill.name)
                .unwrap_or(crate::skills::SkillReadiness::Unknown),
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
    let (registry, directories) =
        discover_skills_for_runtime_api(&state.workspace, &skills_dir, mode);
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
    Ok(Json(SubmitUserInputResponse {
        ok: true,
        input_id,
        delivered,
    }))
}

async fn runtime_info(State(state): State<RuntimeApiState>) -> Json<RuntimeInfoResponse> {
    let version = env!("CARGO_PKG_VERSION");
    Json(RuntimeInfoResponse {
        service: "codewhale-runtime-api",
        runtime_api_version: RUNTIME_API_VERSION,
        codewhale_version: version,
        bind_host: state.bind_host.clone(),
        port: state.bind_port,
        auth_required: state.auth_required,
        transports: vec!["http", "sse"],
        capabilities: default_runtime_capabilities(),
        experimental: RuntimeExperimentalCapabilities::default(),
        version,
    })
}

async fn list_mcp_servers(
    State(state): State<RuntimeApiState>,
) -> Result<Json<McpServersResponse>, ApiError> {
    let mcp_config_path = state.config.read().mcp_config_path();
    let config = crate::mcp::load_config_with_workspace(&mcp_config_path, &state.workspace)
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
                let new_pool =
                    McpPool::from_config_path_with_workspace(&mcp_config_path, &state.workspace)
                        .map_err(|e| {
                            ApiError::internal(format!("Failed to load MCP config: {e}"))
                        })?;
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
    let patch_result = patch_undo_workspace_files(&thread.workspace);

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
fn patch_undo_workspace_files(workspace: &FsPath) -> PatchUndoResult {
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
    let snapshots = match repo.list(20) {
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
        .find(|s| matches!(repo.work_tree_matches_snapshot(&s.id), Ok(false) | Err(_)));
    let Some(target) = target else {
        return PatchUndoResult {
            files_restored: false,
            summary: Some(
                "No older tool or pre-turn snapshots differ from the current workspace."
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
                input_summary: None,
                model: None,
                mode: None,
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
    Path((id, _turn_id, call_id)): Path<(String, String, String)>,
    Json(result): Json<DynamicToolCallResult>,
) -> Result<StatusCode, ApiError> {
    state
        .runtime_threads
        .get_thread(&id)
        .await
        .map_err(map_thread_err)?;
    if state
        .runtime_threads
        .deliver_dynamic_tool_result(&call_id, result)
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

async fn list_tasks(
    State(state): State<RuntimeApiState>,
    Query(query): Query<TasksQuery>,
) -> Result<Json<TasksResponse>, ApiError> {
    let tasks = state.task_manager.list_tasks(query.limit).await;
    let counts = state.task_manager.counts().await;
    Ok(Json(TasksResponse { tasks, counts }))
}

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

async fn cancel_task(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<Json<TaskRecord>, ApiError> {
    let task = state
        .task_manager
        .cancel_task(&id)
        .await
        .map_err(map_task_err)?;
    Ok(Json(task))
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

    let mut backlog = state
        .runtime_threads
        .events_since(&id, query.since_seq)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if let Some(limit) = query.replay_limit
        && backlog.len() > limit
    {
        backlog = backlog.split_off(backlog.len() - limit);
    }
    let mut last_seq = query.since_seq.unwrap_or(0);
    if let Some(last) = backlog.last() {
        last_seq = last.seq;
    }

    let mut live = state.runtime_threads.subscribe_events();
    let thread_id = id.clone();
    let stream = stream! {
        for event in backlog {
            let event_name = event.event.clone();
            yield Ok(sse_json(&event_name, runtime_event_payload(event)));
        }
        loop {
            let incoming = live.recv().await;
            let Ok(event) = incoming else {
                break;
            };
            if event.thread_id != thread_id {
                continue;
            }
            if event.seq <= last_seq {
                continue;
            }
            last_seq = event.seq;
            let event_name = event.event.clone();
            yield Ok(sse_json(&event_name, runtime_event_payload(event)));
        }
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

async fn stream_turn(
    State(state): State<RuntimeApiState>,
    Json(req): Json<StreamTurnRequest>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<SseEvent, Infallible>>>, ApiError> {
    if req.prompt.trim().is_empty() {
        return Err(ApiError::bad_request("prompt is required"));
    }

    let model = req.model.clone().unwrap_or_else(|| {
        state
            .config
            .read()
            .default_text_model
            .clone()
            .unwrap_or_else(|| DEFAULT_TEXT_MODEL.to_string())
    });
    let workspace = req
        .workspace
        .clone()
        .unwrap_or_else(|| state.workspace.clone());
    let mode = req.mode.clone().unwrap_or_else(|| "agent".to_string());
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

    let turn = state
        .runtime_threads
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt,
                input_summary: None,
                model: Some(model.clone()),
                mode: Some(mode.clone()),
                allow_shell: Some(allow_shell),
                trust_mode: Some(trust_mode),
                auto_approve: Some(auto_approve),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| ApiError::internal(format!("Failed to start stream turn: {e}")))?;

    let backlog = state
        .runtime_threads
        .events_since(&thread.id, None)
        .map_err(|e| ApiError::internal(format!("Failed to load stream backlog: {e}")))?;
    let mut live = state.runtime_threads.subscribe_events();
    let thread_id = thread.id.clone();
    let turn_id = turn.id.clone();

    let stream = stream! {
        yield Ok(sse_json("turn.started", json!({
            "thread_id": thread.id,
            "turn_id": turn.id,
            "model": model,
            "mode": mode,
            "workspace": workspace,
        })));

        for event in backlog {
            if event.thread_id != thread_id || event.turn_id.as_deref() != Some(&turn_id) {
                continue;
            }
            if let Some(mapped) = map_compat_stream_event(&event) {
                yield Ok(mapped);
            }
            if event.event == "turn.completed" {
                yield Ok(sse_json("done", json!({})));
                return;
            }
        }

        loop {
            let incoming = live.recv().await;
            let Ok(event) = incoming else {
                yield Ok(sse_json("error", json!({ "message": "event channel closed" })));
                break;
            };
            if event.thread_id != thread_id || event.turn_id.as_deref() != Some(&turn_id) {
                continue;
            }
            if let Some(mapped) = map_compat_stream_event(&event) {
                yield Ok(mapped);
            }
            if event.event == "turn.completed" {
                break;
            }
        }

        yield Ok(sse_json("done", json!({})));
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
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
        "approval.required" => Some(sse_json("approval.required", payload.clone())),
        "approval.decided" => Some(sse_json("approval.decided", payload.clone())),
        "approval.timeout" => Some(sse_json("approval.timeout", payload.clone())),
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
) -> (crate::skills::SkillRegistry, Vec<PathBuf>) {
    let directories = skills_search_directories(workspace, skills_dir, mode);
    let registry = crate::skills::discover_from_directories(directories.clone());
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
    show_tool_details: bool,
    locale: String,
    max_history: usize,
    prefer_external_pdftotext: bool,
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
    let mut settings = crate::settings::Settings::load_persisted()
        .map_err(|e| ApiError::internal(format!("Failed to load settings: {e}")))?;
    settings
        .set(key, value)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    settings
        .save()
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
    let default_model = settings
        .default_model
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
        show_tool_details: settings.show_tool_details,
        locale: settings.locale.clone(),
        max_history: settings.max_input_history,
        prefer_external_pdftotext: settings.prefer_external_pdftotext,
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
    let value = req.value;
    let persist = req.persist;

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
            "model" | "default_model" => config_persistence::persist_root_string_key(
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
            "provider_url" | "provider_base_url" => {
                let provider = state.config.read().api_provider();
                config_persistence::persist_provider_base_url_key(config_path, provider, &value)
            }
            "cost_currency"
            | "default_mode"
            | "auto_compact"
            | "show_thinking"
            | "show_tool_details"
            | "calm_mode"
            | "prefer_external_pdftotext"
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
                    "Unknown config key '{key}'. Supported keys: model, default_model, reasoning_effort, approval_mode, base_url, provider_url, cost_currency, default_mode, auto_compact, allow_shell, mcp_config_path, show_thinking, show_tool_details, locale, max_history, calm_mode, prefer_external_pdftotext, workspace_follow_symlinks, subagents_enabled, subagents_max_depth, sandbox_mode, strict_tool_mode, memory_enabled, search_provider, prompt_suggestion"
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
        .allow_headers(Any)
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
    {
        ApiError {
            status: StatusCode::CONFLICT,
            message,
        }
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

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
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
