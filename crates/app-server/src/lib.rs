use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use codewhale_agent::ModelRegistry;
use codewhale_config::{CliRuntimeOverrides, ConfigStore};
use codewhale_core::Runtime;
use codewhale_hooks::{HookDispatcher, JsonlHookSink, StdoutHookSink, UnixSocketHookSink};
use codewhale_mcp::McpManager;
use codewhale_protocol::{
    AppRequest, AppResponse, PromptRequest, PromptResponse, ThreadGoalClearParams,
    ThreadGoalGetParams, ThreadGoalSetParams, ThreadRequest, ThreadResponse, UserInputAnswerEvent,
};
use codewhale_state::StateStore;
use codewhale_tools::{ToolCall, ToolRegistry};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, RwLock};
use tower_http::cors::CorsLayer;
use uuid::Uuid;

/// Answers submitted for a pending `request_user_input` clarification.
///
/// The headless runtime emits [`codewhale_protocol::EventFrame::UserInputRequest`]
/// fire-and-return (it has no resume channel, mirroring headless approval).
/// Clients POST answers back via [`AppRequest::SubmitUserInput`]; we record
/// them here keyed by `request_id` so a driver can retrieve and feed them into
/// the next turn as structured context. True in-flight resume would require an
/// awaiter in `invoke_tool` and is left as a follow-up.
type PendingUserInputAnswers = Vec<UserInputAnswerEvent>;

mod chat_completions;

/// Legacy DeepSeek-era naming kept for external compatibility.
///
/// CodeWhale began life as a DeepSeek CLI; existing health probes, SDK
/// harnesses, and on-disk layouts still key off these names. Every remaining
/// legacy reference in this crate routes through this shim so a future
/// coordinated migration touches exactly one place (repo policy: preserve
/// legacy migration care).
mod legacy_deepseek_compat {
    use std::path::PathBuf;

    /// Service name advertised by the HTTP and stdio health probes.
    pub(crate) const SERVICE_NAME: &str = "deepseek-app-server";

    /// Fallback hook-event log location used when no config path is
    /// provided (legacy `.deepseek/` dot-directory layout).
    pub(crate) fn default_events_log_path() -> PathBuf {
        PathBuf::from(".deepseek/events.jsonl")
    }
}

/// Upper bound on JSON request bodies accepted by the HTTP app-server.
const MAX_HTTP_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_SSE_FRAME_BYTES: usize = 16 * 1024 * 1024;

const DEFAULT_CORS_ORIGINS: &[&str] = &[
    "http://localhost",
    "http://localhost:1420",
    "http://localhost:3000",
    "http://localhost:5173",
    "http://127.0.0.1",
    "http://127.0.0.1:1420",
    "tauri://localhost",
];

#[derive(Clone)]
pub struct AppServerOptions {
    pub listen: SocketAddr,
    pub config_path: Option<PathBuf>,
    pub auth_token: Option<String>,
    pub insecure_no_auth: bool,
    pub cors_origins: Vec<String>,
}

impl std::fmt::Debug for AppServerOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppServerOptions")
            .field("listen", &self.listen)
            .field("config_path", &self.config_path)
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| "<redacted>"),
            )
            .field("insecure_no_auth", &self.insecure_no_auth)
            .field("cors_origins", &self.cors_origins)
            .finish()
    }
}

/// Cached stdio→runtime bridge handle.
///
/// The outer [`AppState::stdio_bridge`] mutex guards only the cache slot;
/// this inner mutex serializes traffic on one bridge (single child process
/// plus per-thread seq bookkeeping requires ordered access).
type SharedRuntimeBridge = Arc<Mutex<RuntimeBridge>>;

#[derive(Clone)]
struct AppState {
    config_path: Option<PathBuf>,
    config: Arc<RwLock<codewhale_config::ConfigToml>>,
    /// Read/write split mirrors [`Runtime`]'s own receivers: `&self`
    /// operations (tool calls, status, MCP startup) share a read guard and
    /// run concurrently; `&mut self` turns (prompt/thread) and config pushes
    /// take the write guard because the runtime genuinely requires
    /// exclusivity there.
    runtime: Arc<RwLock<Runtime>>,
    registry: ModelRegistry,
    auth_token: Option<String>,
    stdio_bridge: Arc<Mutex<Option<SharedRuntimeBridge>>>,
    stdio_thread_hints: Arc<Mutex<HashMap<String, RuntimeThreadHint>>>,
    /// Answers submitted via `AppRequest::SubmitUserInput`, keyed by
    /// `request_id`. A driver polls this to resolve clarification questions
    /// raised by the model during a headless run.
    pending_user_input: Arc<Mutex<std::collections::HashMap<String, PendingUserInputAnswers>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolCallRequest {
    call: ToolCall,
    #[serde(default)]
    cwd: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[serde(default)]
    jsonrpc: Option<String>,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug)]
struct JsonRpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

#[derive(Debug)]
struct StdioDispatchResult {
    result: Value,
    should_exit: bool,
}

#[derive(Debug)]
struct RuntimeBridge {
    base_url: String,
    client: reqwest::Client,
    auth_token: Option<String>,
    child: Option<Child>,
    thread_map: HashMap<String, String>,
    last_seq_by_thread: HashMap<String, u64>,
}

#[derive(Debug, Clone, Default)]
struct RuntimeThreadHint {
    model: Option<String>,
    workspace: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnTerminalStatus {
    Completed,
    Failed,
    Interrupted,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppTransport {
    Http,
    Stdio,
}

#[derive(Debug, Deserialize)]
struct ConfigGetParams {
    key: String,
}

#[derive(Debug, Deserialize)]
struct ConfigSetParams {
    key: String,
    value: String,
}

#[derive(Debug, Deserialize)]
struct ThreadIdParams {
    thread_id: String,
}

#[derive(Debug, Deserialize)]
struct ThreadMessageParams {
    thread_id: String,
    input: String,
}

pub async fn run(options: AppServerOptions) -> Result<()> {
    let auth_token = resolve_auth_token(&options)?;
    let state = build_state(options.config_path.clone(), auth_token)?;
    let app = app_router(state, &options.cors_origins);

    let listener = tokio::net::TcpListener::bind(options.listen).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

fn app_router(state: AppState, cors_origins: &[String]) -> Router {
    let protected_routes = Router::new()
        .route("/thread", post(thread_handler))
        .route("/app", post(app_handler))
        .route("/prompt", post(prompt_handler))
        .route("/tool", post(tool_handler))
        .route("/jobs", get(jobs_handler))
        .route("/mcp/startup", post(mcp_startup_handler))
        .route(
            "/v1/chat/completions",
            post(chat_completions::chat_completions_handler),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_app_server_token,
        ));

    Router::new()
        .route("/healthz", get(healthz))
        .merge(protected_routes)
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .layer(cors_layer(cors_origins))
        .with_state(state)
}

pub async fn run_stdio(config_path: Option<PathBuf>) -> Result<()> {
    let state = build_state(config_path, None)?;
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();
    let mut writer = tokio::io::BufWriter::new(stdout);
    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(err) => {
                let response = jsonrpc_error(
                    None,
                    JsonRpcError::parse_error(format!("invalid json: {err}")),
                );
                writer.write_all(&serde_json::to_vec(&response)?).await?;
                writer.write_all(b"\n").await?;
                writer.flush().await?;
                continue;
            }
        };

        if request
            .jsonrpc
            .as_deref()
            .is_some_and(|version| version != "2.0")
        {
            let response = jsonrpc_error(
                request.id,
                JsonRpcError::invalid_request("jsonrpc version must be 2.0"),
            );
            writer.write_all(&serde_json::to_vec(&response)?).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
            continue;
        }

        let response = match dispatch_stdio_request_with_writer(
            &state,
            &mut writer,
            &request.method,
            request.params,
        )
        .await
        {
            Ok(dispatch) => {
                let encoded = jsonrpc_result(request.id, dispatch.result);
                writer.write_all(&serde_json::to_vec(&encoded)?).await?;
                writer.write_all(b"\n").await?;
                writer.flush().await?;
                if dispatch.should_exit {
                    break;
                }
                continue;
            }
            Err(err) => jsonrpc_error(request.id, err),
        };

        writer.write_all(&serde_json::to_vec(&response)?).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    Ok(())
}

async fn healthz() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "protocol": "v2",
        "service": legacy_deepseek_compat::SERVICE_NAME
    }))
}

async fn thread_handler(
    State(state): State<AppState>,
    Json(req): Json<ThreadRequest>,
) -> (StatusCode, Json<ThreadResponse>) {
    let mut runtime = state.runtime.write().await;
    match runtime.handle_thread(req).await {
        Ok(res) => (StatusCode::OK, Json(res)),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ThreadResponse {
                thread_id: "error".to_string(),
                status: format!("error:{err}"),
                thread: None,
                threads: Vec::new(),
                goal: None,
                model: None,
                model_provider: None,
                cwd: None,
                approval_policy: None,
                sandbox: None,
                events: Vec::new(),
                data: json!({}),
            }),
        ),
    }
}

async fn prompt_handler(
    State(state): State<AppState>,
    Json(req): Json<PromptRequest>,
) -> (StatusCode, Json<PromptResponse>) {
    let mut runtime = state.runtime.write().await;
    let overrides = CliRuntimeOverrides::default();
    match runtime.handle_prompt(req, &overrides).await {
        Ok(res) => (StatusCode::OK, Json(res)),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(PromptResponse {
                output: err.to_string(),
                model: "unknown".to_string(),
                events: Vec::new(),
            }),
        ),
    }
}

async fn tool_handler(
    State(state): State<AppState>,
    Json(req): Json<ToolCallRequest>,
) -> (StatusCode, Json<Value>) {
    let cwd = req
        .cwd
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    // Resolve approval policy from config instead of hardcoding.
    let approval_mode = {
        let cfg = state.config.read().await;
        cfg.approval_policy
            .as_deref()
            .and_then(|p| match p.trim().to_ascii_lowercase().as_str() {
                "auto" | "yolo" => Some(codewhale_execpolicy::AskForApproval::UnlessTrusted),
                "never" | "deny" => Some(codewhale_execpolicy::AskForApproval::Never),
                _ => None,
            })
            .unwrap_or(codewhale_execpolicy::AskForApproval::OnRequest)
    };
    // `invoke_tool` takes `&self`, so long-running tool executions share a
    // read guard: they run concurrently with each other and with status
    // reads instead of serializing every request behind one Mutex.
    let runtime = state.runtime.read().await;
    match runtime.invoke_tool(req.call, approval_mode, &cwd).await {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        ),
    }
}

async fn jobs_handler(State(state): State<AppState>) -> Json<AppResponse> {
    let runtime = state.runtime.read().await;
    Json(runtime.app_status())
}

async fn mcp_startup_handler(State(state): State<AppState>) -> Json<Value> {
    let runtime = state.runtime.read().await;
    let summary = runtime.mcp_startup().await;
    Json(json!({
        "ok": true,
        "summary": summary
    }))
}

async fn app_handler(
    State(state): State<AppState>,
    Json(req): Json<AppRequest>,
) -> (StatusCode, Json<AppResponse>) {
    let response = process_app_request(&state, req, AppTransport::Http).await;
    (app_response_status(&response), Json(response))
}

fn app_response_status(response: &AppResponse) -> StatusCode {
    if response.ok {
        return StatusCode::OK;
    }
    if response.data.get("request_id").is_some() {
        StatusCode::CONFLICT
    } else if response
        .data
        .get("error")
        .and_then(Value::as_str)
        .is_some_and(|err| err.contains("failed to load config"))
    {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::BAD_REQUEST
    }
}

fn build_state(config_path: Option<PathBuf>, auth_token: Option<String>) -> Result<AppState> {
    let has_explicit_config_path = config_path.is_some();
    let store = ConfigStore::load(config_path)?;
    let config_path = has_explicit_config_path.then(|| store.path().to_path_buf());
    let config = store.config.clone();
    let exec_policy = store.exec_policy_engine();
    let registry = ModelRegistry::default();

    let state_db_path = config_path
        .as_ref()
        .and_then(|p| p.parent().map(|parent| parent.join("state.db")));
    let state_store = StateStore::open(state_db_path)?;

    let mut hooks = HookDispatcher::default();
    hooks.add_sink(Arc::new(StdoutHookSink));
    let hook_log_path = config_path
        .as_ref()
        .and_then(|p| p.parent().map(|parent| parent.join("events.jsonl")))
        .unwrap_or_else(legacy_deepseek_compat::default_events_log_path);
    hooks.add_sink(Arc::new(JsonlHookSink::new(hook_log_path)));

    if let Some(socket_path) = config
        .hook_sinks
        .as_ref()
        .and_then(|sinks| sinks.unix_socket_path.as_ref())
        .filter(|path| !path.as_os_str().is_empty())
    {
        hooks.add_sink(Arc::new(UnixSocketHookSink::new(socket_path.clone())));
    }

    let runtime = Runtime::new(
        config.clone(),
        registry.clone(),
        state_store,
        Arc::new(ToolRegistry::default()),
        Arc::new(McpManager::default()),
        exec_policy,
        hooks,
    );

    Ok(AppState {
        config_path,
        config: Arc::new(RwLock::new(config)),
        runtime: Arc::new(RwLock::new(runtime)),
        registry,
        auth_token,
        stdio_bridge: Arc::new(Mutex::new(None)),
        stdio_thread_hints: Arc::new(Mutex::new(HashMap::new())),
        pending_user_input: Arc::new(Mutex::new(std::collections::HashMap::new())),
    })
}

fn resolve_auth_token(options: &AppServerOptions) -> Result<Option<String>> {
    let configured = options.auth_token.as_ref().map(|token| token.trim());
    if let Some(token) = configured
        && token.is_empty()
    {
        bail!("app-server auth token cannot be empty");
    }
    let has_explicit_token = configured.is_some();

    if options.insecure_no_auth {
        if !options.listen.ip().is_loopback() {
            bail!("refusing unauthenticated app-server bind on non-loopback address");
        }
        eprintln!("warning: app-server HTTP auth disabled by --insecure-no-auth");
        return Ok(None);
    }

    if !has_explicit_token && !options.listen.ip().is_loopback() {
        bail!(
            "refusing non-loopback app-server bind without explicit auth token; pass --auth-token or set CODEWHALE_APP_SERVER_TOKEN"
        );
    }

    let token = configured
        .map(str::to_string)
        .unwrap_or_else(|| format!("cwapp_{}", Uuid::new_v4().simple()));
    for line in app_server_auth_status_lines(has_explicit_token) {
        eprintln!("{line}");
    }
    Ok(Some(token))
}

fn app_server_auth_status_lines(has_explicit_token: bool) -> Vec<&'static str> {
    if has_explicit_token {
        return vec!["app-server auth: bearer token required for HTTP routes."];
    }
    vec![
        "app-server auth: generated bearer token for this process (not printed).",
        "  Pass --auth-token or set CODEWHALE_APP_SERVER_TOKEN when another client needs to connect.",
    ]
}

fn cors_layer(extra_origins: &[String]) -> CorsLayer {
    let mut origins: Vec<HeaderValue> = DEFAULT_CORS_ORIGINS
        .iter()
        .filter_map(|origin| HeaderValue::from_str(origin).ok())
        .collect();
    for raw in extra_origins {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        match HeaderValue::from_str(trimmed) {
            Ok(value) if !origins.contains(&value) => origins.push(value),
            Ok(_) => {}
            Err(err) => {
                eprintln!("warning: ignoring invalid app-server CORS origin `{trimmed}`: {err}")
            }
        }
    }

    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}

async fn require_app_server_token(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let Some(expected) = state.auth_token.as_deref() else {
        return next.run(req).await;
    };
    let authorized = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| raw.strip_prefix("Bearer "))
        .is_some_and(|token| constant_time_eq(token.as_bytes(), expected.as_bytes()));

    if authorized {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": {
                    "message": "app-server bearer token required",
                    "status": StatusCode::UNAUTHORIZED.as_u16(),
                }
            })),
        )
            .into_response()
    }
}

/// Compares the full length of both inputs regardless of where they first
/// differ, so auth failures don't leak the matching prefix length via timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= usize::from(x ^ y);
    }
    diff == 0
}

fn params_or_object(params: Value) -> Value {
    if params.is_null() { json!({}) } else { params }
}

fn parse_params<T: DeserializeOwned>(params: Value) -> std::result::Result<T, JsonRpcError> {
    serde_json::from_value(params).map_err(|err| JsonRpcError::invalid_params(err.to_string()))
}

fn jsonrpc_result(id: Option<Value>, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "result": result
    })
}

fn jsonrpc_error(id: Option<Value>, err: JsonRpcError) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": {
            "code": err.code,
            "message": err.message,
            "data": err.data
        }
    })
}

impl JsonRpcError {
    fn parse_error(message: impl Into<String>) -> Self {
        Self {
            code: -32700,
            message: message.into(),
            data: None,
        }
    }

    fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: -32600,
            message: message.into(),
            data: None,
        }
    }

    fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("unsupported method: {method}"),
            data: None,
        }
    }

    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            data: None,
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
            data: None,
        }
    }
}

async fn handle_thread_request(
    state: &AppState,
    req: ThreadRequest,
) -> std::result::Result<ThreadResponse, JsonRpcError> {
    let mut runtime = state.runtime.write().await;
    runtime
        .handle_thread(req)
        .await
        .map_err(|err| JsonRpcError::internal(err.to_string()))
}

async fn handle_prompt_request(
    state: &AppState,
    req: PromptRequest,
) -> std::result::Result<PromptResponse, JsonRpcError> {
    let mut runtime = state.runtime.write().await;
    runtime
        .handle_prompt(req, &CliRuntimeOverrides::default())
        .await
        .map_err(|err| JsonRpcError::internal(err.to_string()))
}

async fn handle_stdio_thread_message<W: AsyncWrite + Unpin>(
    state: &AppState,
    writer: &mut W,
    parsed: ThreadMessageParams,
) -> std::result::Result<Value, JsonRpcError> {
    let hint = {
        let hints = state.stdio_thread_hints.lock().await;
        hints.get(&parsed.thread_id).cloned()
    };
    let bridge = acquire_stdio_bridge(state).await?;
    // The inner bridge lock is held for the whole turn: one child process
    // serves all threads and per-thread seq tracking requires ordered
    // access. The cache slot itself stays unlocked, so config updates and
    // bridge invalidation are never queued behind a streaming turn.
    let mut bridge = bridge.lock().await;
    let runtime_thread_id = bridge
        .ensure_runtime_thread(&parsed.thread_id, hint)
        .await
        .map_err(|err| JsonRpcError::internal(err.to_string()))?;
    let mut result = bridge
        .message_thread(&runtime_thread_id, &parsed.input, writer)
        .await
        .map_err(|err| JsonRpcError::internal(err.to_string()))?;
    if let Some(object) = result.as_object_mut() {
        object.insert("thread_id".to_string(), Value::String(parsed.thread_id));
    }
    Ok(result)
}

async fn record_stdio_thread_hint(state: &AppState, response: &ThreadResponse) {
    let mut hints = state.stdio_thread_hints.lock().await;
    hints.insert(
        response.thread_id.clone(),
        RuntimeThreadHint {
            model: response.model.clone(),
            workspace: response.cwd.clone(),
        },
    );
}

/// Fetch the cached stdio→runtime bridge, spawning one on first use.
///
/// The cache-slot lock is held only for the lookup/insert — never across
/// the child spawn or any request traffic — so [`invalidate_stdio_bridge`]
/// and other slot users are never blocked behind a slow bridge operation.
async fn acquire_stdio_bridge(
    state: &AppState,
) -> std::result::Result<SharedRuntimeBridge, JsonRpcError> {
    if let Some(bridge) = state.stdio_bridge.lock().await.as_ref() {
        return Ok(bridge.clone());
    }
    let bridge = Arc::new(Mutex::new(
        RuntimeBridge::start(state.config_path.as_deref())
            .await
            .map_err(|err| JsonRpcError::internal(err.to_string()))?,
    ));
    let mut slot = state.stdio_bridge.lock().await;
    // Prefer a bridge cached by a concurrent caller while we were spawning;
    // dropping our unused one kills the extra child via `Drop`.
    Ok(slot.get_or_insert_with(|| bridge.clone()).clone())
}

/// Drop the cached runtime bridge so the next stdio thread message spawns a
/// fresh child that re-reads the persisted config. An in-flight message
/// keeps its own [`SharedRuntimeBridge`] clone and finishes against the old
/// child, which is killed when the last clone drops.
async fn invalidate_stdio_bridge(state: &AppState) {
    let mut bridge = state.stdio_bridge.lock().await;
    *bridge = None;
}

impl RuntimeBridge {
    async fn start(config_path: Option<&Path>) -> Result<Self> {
        install_rustls_crypto_provider();
        let port = reserve_runtime_port()?;
        let auth_token = format!("cwrt_{}", Uuid::new_v4().simple());
        let child = Self::runtime_command(config_path, port, &auth_token)?
            .spawn()
            .context("failed to start runtime API bridge")?;
        let mut bridge = Self {
            base_url: format!("http://127.0.0.1:{port}"),
            client: reqwest::Client::builder()
                .build()
                .context("failed to build runtime API client")?,
            auth_token: Some(auth_token),
            child: Some(child),
            thread_map: HashMap::new(),
            last_seq_by_thread: HashMap::new(),
        };
        bridge.wait_until_ready().await?;
        Ok(bridge)
    }

    fn runtime_command(config_path: Option<&Path>, port: u16, auth_token: &str) -> Result<Command> {
        let current_exe = std::env::current_exe().ok();
        let mut command = if let Some(path) = current_exe {
            Command::new(path)
        } else {
            Command::new("codewhale")
        };
        command
            .arg("app-server")
            .arg("--http")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("--auth-token")
            .arg(auth_token)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(config_path) = config_path {
            command.arg("--config").arg(config_path);
        }
        Ok(command)
    }

    async fn wait_until_ready(&mut self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(child) = self.child.as_mut()
                && let Some(status) = child.try_wait()?
            {
                return Err(anyhow!(
                    "runtime API bridge exited before becoming ready (status {status})"
                ));
            }

            match self
                .client
                .get(format!("{}/health", self.base_url))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => return Ok(()),
                _ if Instant::now() >= deadline => {
                    bail!(
                        "timed out waiting for runtime API bridge at {}/health",
                        self.base_url
                    )
                }
                _ => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    }

    fn authed(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.auth_token.as_deref() {
            Some(token) => builder.bearer_auth(token),
            None => builder,
        }
    }

    async fn request_json(&self, builder: reqwest::RequestBuilder) -> Result<Value> {
        let response = builder.send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            let detail = body.trim();
            if detail.is_empty() {
                bail!("runtime API returned {status}");
            }
            bail!("runtime API returned {status}: {detail}");
        }
        serde_json::from_str(&body).with_context(|| format!("invalid runtime API json: {body}"))
    }

    async fn ensure_runtime_thread(
        &mut self,
        stdio_thread_id: &str,
        hint: Option<RuntimeThreadHint>,
    ) -> Result<String> {
        if let Some(runtime_thread_id) = self.thread_map.get(stdio_thread_id) {
            return Ok(runtime_thread_id.clone());
        }
        let hint = hint.unwrap_or_default();
        let runtime_thread_id = self
            .create_runtime_thread(hint.model, hint.workspace)
            .await?;
        self.thread_map
            .insert(stdio_thread_id.to_string(), runtime_thread_id.clone());
        Ok(runtime_thread_id)
    }

    async fn create_runtime_thread(
        &mut self,
        model: Option<String>,
        workspace: Option<PathBuf>,
    ) -> Result<String> {
        let record = self
            .request_json(
                self.authed(self.client.post(format!("{}/v1/threads", self.base_url)))
                    .json(&json!({
                        "model": model,
                        "workspace": workspace,
                        "mode": "agent",
                        "archived": false,
                    })),
            )
            .await?;
        let thread_id = extract_runtime_thread_id(&record)?.to_string();
        self.last_seq_by_thread
            .entry(thread_id.clone())
            .or_insert(0);
        Ok(thread_id)
    }

    async fn message_thread<W: AsyncWrite + Unpin>(
        &mut self,
        thread_id: &str,
        input: &str,
        writer: &mut W,
    ) -> Result<Value> {
        let turn = self
            .request_json(
                self.authed(
                    self.client
                        .post(format!("{}/v1/threads/{thread_id}/turns", self.base_url)),
                )
                .json(&json!({ "prompt": input })),
            )
            .await?;
        let turn_id = turn
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("runtime API turn response missing turn.id"))?
            .to_string();
        let response_id = format!("{thread_id}:{turn_id}");

        emit_stdio_event(
            writer,
            json!({
                "type": "response_start",
                "response_id": response_id,
            }),
        )
        .await?;

        let since_seq = self.last_seq_by_thread.get(thread_id).copied().unwrap_or(0);
        let stream_result = self
            .stream_turn_events(thread_id, &turn_id, &response_id, writer, since_seq)
            .await;

        let _ = emit_stdio_event(
            writer,
            json!({
                "type": "response_end",
                "response_id": response_id,
            }),
        )
        .await;

        let (last_seq, status, error) = stream_result?;
        self.last_seq_by_thread
            .insert(thread_id.to_string(), last_seq);

        match status {
            TurnTerminalStatus::Completed => Ok(json!({
                "thread_id": thread_id,
                "status": "accepted",
                "thread": Value::Null,
                "threads": [],
                "model": Value::Null,
                "model_provider": Value::Null,
                "cwd": Value::Null,
                "approval_policy": Value::Null,
                "sandbox": Value::Null,
                "events": [],
                "data": { "turn_id": turn_id },
            })),
            TurnTerminalStatus::Failed => Err(anyhow!(
                "{}",
                error.unwrap_or_else(|| "turn failed".to_string())
            )),
            TurnTerminalStatus::Interrupted => Err(anyhow!(
                "{}",
                error.unwrap_or_else(|| "turn interrupted".to_string())
            )),
            TurnTerminalStatus::Canceled => Err(anyhow!(
                "{}",
                error.unwrap_or_else(|| "turn canceled".to_string())
            )),
        }
    }

    async fn stream_turn_events<W: AsyncWrite + Unpin>(
        &self,
        thread_id: &str,
        turn_id: &str,
        response_id: &str,
        writer: &mut W,
        since_seq: u64,
    ) -> Result<(u64, TurnTerminalStatus, Option<String>)> {
        let mut response = self
            .authed(self.client.get(format!(
                "{}/v1/threads/{thread_id}/events?since_seq={since_seq}",
                self.base_url
            )))
            .send()
            .await?
            .error_for_status()?;

        let mut buffer = Vec::new();
        let mut last_seq = since_seq;

        while let Some(chunk) = response.chunk().await? {
            buffer.extend_from_slice(&chunk);
            if buffer.len() > MAX_SSE_FRAME_BYTES {
                bail!(
                    "runtime SSE frame exceeded {MAX_SSE_FRAME_BYTES} bytes without a frame delimiter"
                );
            }
            while let Some(frame_bytes) = take_sse_frame(&mut buffer) {
                let Some((event_name, frame_data)) = parse_sse_frame(&frame_bytes) else {
                    continue;
                };
                let envelope: Value = serde_json::from_str(&frame_data)
                    .with_context(|| format!("invalid SSE json for {event_name}: {frame_data}"))?;
                if let Some(seq) = envelope.get("seq").and_then(Value::as_u64) {
                    last_seq = last_seq.max(seq);
                }
                if envelope.get("turn_id").and_then(Value::as_str) != Some(turn_id) {
                    continue;
                }
                let payload = envelope.get("payload").cloned().unwrap_or(Value::Null);
                match event_name.as_str() {
                    "item.delta" => {
                        let kind = payload
                            .get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if kind == "agent_message"
                            && let Some(delta) = payload.get("delta").and_then(Value::as_str)
                            && !delta.is_empty()
                        {
                            emit_stdio_event(
                                writer,
                                json!({
                                    "type": "response_delta",
                                    "response_id": response_id,
                                    "delta": delta,
                                }),
                            )
                            .await?;
                        }
                    }
                    "turn.completed" => {
                        let status = turn_terminal_status(&payload);
                        let error = payload
                            .pointer("/turn/error")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        return Ok((last_seq, status, error));
                    }
                    _ => {}
                }
            }
        }

        bail!("runtime event stream ended before turn.completed")
    }

    #[cfg(test)]
    fn from_base_url_for_test(base_url: String) -> Self {
        install_rustls_crypto_provider();
        Self {
            base_url,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("build reqwest test client"),
            auth_token: None,
            child: None,
            thread_map: HashMap::new(),
            last_seq_by_thread: HashMap::new(),
        }
    }
}

impl RuntimeBridge {
    /// Kills the managed runtime child and reaps it on a detached thread so
    /// neither an explicit shutdown nor Drop blocks a Tokio runtime thread.
    fn shutdown_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

impl Drop for RuntimeBridge {
    fn drop(&mut self) {
        self.shutdown_child();
    }
}

fn reserve_runtime_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

fn install_rustls_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn extract_runtime_thread_id(record: &Value) -> Result<&str> {
    record
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("runtime API thread response missing id"))
}

fn turn_terminal_status(payload: &Value) -> TurnTerminalStatus {
    match payload
        .pointer("/turn/status")
        .and_then(Value::as_str)
        .unwrap_or("completed")
        .to_ascii_lowercase()
        .as_str()
    {
        "failed" => TurnTerminalStatus::Failed,
        "interrupted" => TurnTerminalStatus::Interrupted,
        "canceled" | "cancelled" => TurnTerminalStatus::Canceled,
        _ => TurnTerminalStatus::Completed,
    }
}

async fn emit_stdio_event<W: AsyncWrite + Unpin>(writer: &mut W, event: Value) -> Result<()> {
    writer.write_all(&serde_json::to_vec(&event)?).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

fn take_sse_frame(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    if let Some(pos) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
        return Some(buffer.drain(..pos + 4).collect());
    }
    buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|pos| buffer.drain(..pos + 2).collect())
}

fn parse_sse_frame(frame_bytes: &[u8]) -> Option<(String, String)> {
    let text = String::from_utf8(frame_bytes.to_vec()).ok()?;
    let mut event_name = None;
    let mut data_lines = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if let Some(value) = line.strip_prefix("event:") {
            event_name = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data_lines.push(value.trim_start().to_string());
        }
    }
    match (event_name, data_lines.is_empty()) {
        (Some(event), false) => Some((event, data_lines.join("\n"))),
        _ => None,
    }
}

#[cfg(test)]
async fn dispatch_stdio_request(
    state: &AppState,
    method: &str,
    params: Value,
) -> std::result::Result<StdioDispatchResult, JsonRpcError> {
    let mut sink = tokio::io::sink();
    dispatch_stdio_request_with_writer(state, &mut sink, method, params).await
}

async fn dispatch_stdio_app_request(
    state: &AppState,
    request: AppRequest,
) -> std::result::Result<StdioDispatchResult, JsonRpcError> {
    let response = Box::pin(process_app_request(state, request, AppTransport::Stdio)).await;
    Ok(StdioDispatchResult {
        result: serde_json::to_value(response)
            .map_err(|err| JsonRpcError::internal(err.to_string()))?,
        should_exit: false,
    })
}

async fn dispatch_stdio_request_with_writer<W: AsyncWrite + Unpin>(
    state: &AppState,
    writer: &mut W,
    method: &str,
    params: Value,
) -> std::result::Result<StdioDispatchResult, JsonRpcError> {
    let outcome = match method {
        "healthz" | "app/healthz" => StdioDispatchResult {
            result: json!({
                "status": "ok",
                "service": legacy_deepseek_compat::SERVICE_NAME,
                "transport": "stdio"
            }),
            should_exit: false,
        },
        "capabilities" => StdioDispatchResult {
            result: json!({
                "transport": "stdio",
                "families": ["thread/*", "app/*", "prompt/*"],
                "methods": [
                    "healthz",
                    "thread/capabilities",
                    "thread/request",
                    "thread/create",
                    "thread/start",
                    "thread/resume",
                    "thread/fork",
                    "thread/list",
                    "thread/read",
                    "thread/set_name",
                    "thread/goal/set",
                    "thread/goal/get",
                    "thread/goal/clear",
                    "thread/archive",
                    "thread/unarchive",
                    "thread/message",
                    "app/capabilities",
                    "app/request",
                    "app/config/get",
                    "app/config/set",
                    "app/config/unset",
                    "app/config/list",
                    "app/config/reload",
                    "app/models",
                    "app/thread_loaded_list",
                    "prompt/capabilities",
                    "prompt/request",
                    "prompt/run",
                    "shutdown"
                ]
            }),
            should_exit: false,
        },
        "thread/capabilities" => StdioDispatchResult {
            result: json!({
                "methods": [
                    "thread/request",
                    "thread/create",
                    "thread/start",
                    "thread/resume",
                    "thread/fork",
                    "thread/list",
                    "thread/read",
                    "thread/set_name",
                    "thread/goal/set",
                    "thread/goal/get",
                    "thread/goal/clear",
                    "thread/archive",
                    "thread/unarchive",
                    "thread/message"
                ]
            }),
            should_exit: false,
        },
        "thread/request" => {
            let request: ThreadRequest = parse_params(params)?;
            if let ThreadRequest::Message { thread_id, input } = request {
                let response = handle_stdio_thread_message(
                    state,
                    writer,
                    ThreadMessageParams { thread_id, input },
                )
                .await?;
                return Ok(StdioDispatchResult {
                    result: response,
                    should_exit: false,
                });
            }
            let should_record_hint = matches!(
                &request,
                ThreadRequest::Create { .. }
                    | ThreadRequest::Start(_)
                    | ThreadRequest::Resume(_)
                    | ThreadRequest::Fork(_)
            );
            let response = handle_thread_request(state, request).await?;
            if should_record_hint {
                record_stdio_thread_hint(state, &response).await;
            }
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/create" => {
            #[derive(Debug, Deserialize)]
            struct CreateParams {
                #[serde(default)]
                metadata: Value,
            }
            let parsed: CreateParams = parse_params(params_or_object(params))?;
            let response = handle_thread_request(
                state,
                ThreadRequest::Create {
                    metadata: parsed.metadata,
                },
            )
            .await?;
            record_stdio_thread_hint(state, &response).await;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/start" => {
            let request = ThreadRequest::Start(parse_params(params_or_object(params))?);
            let response = handle_thread_request(state, request).await?;
            record_stdio_thread_hint(state, &response).await;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/resume" => {
            let request = ThreadRequest::Resume(parse_params(params_or_object(params))?);
            let response = handle_thread_request(state, request).await?;
            record_stdio_thread_hint(state, &response).await;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/fork" => {
            let request = ThreadRequest::Fork(parse_params(params_or_object(params))?);
            let response = handle_thread_request(state, request).await?;
            record_stdio_thread_hint(state, &response).await;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/list" => {
            let request = ThreadRequest::List(parse_params(params_or_object(params))?);
            let response = handle_thread_request(state, request).await?;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/read" => {
            let request = ThreadRequest::Read(parse_params(params_or_object(params))?);
            let response = handle_thread_request(state, request).await?;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/set_name" | "thread/set-name" => {
            let request = ThreadRequest::SetName(parse_params(params_or_object(params))?);
            let response = handle_thread_request(state, request).await?;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/goal/set" | "thread/goal_set" | "thread/goal-set" => {
            let request = ThreadRequest::GoalSet(parse_params::<ThreadGoalSetParams>(
                params_or_object(params),
            )?);
            let response = handle_thread_request(state, request).await?;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/goal/get" | "thread/goal_get" | "thread/goal-get" => {
            let request = ThreadRequest::GoalGet(parse_params::<ThreadGoalGetParams>(
                params_or_object(params),
            )?);
            let response = handle_thread_request(state, request).await?;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/goal/clear" | "thread/goal_clear" | "thread/goal-clear" => {
            let request = ThreadRequest::GoalClear(parse_params::<ThreadGoalClearParams>(
                params_or_object(params),
            )?);
            let response = handle_thread_request(state, request).await?;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/archive" => {
            let parsed: ThreadIdParams = parse_params(params_or_object(params))?;
            let response = handle_thread_request(
                state,
                ThreadRequest::Archive {
                    thread_id: parsed.thread_id,
                },
            )
            .await?;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/unarchive" => {
            let parsed: ThreadIdParams = parse_params(params_or_object(params))?;
            let response = handle_thread_request(
                state,
                ThreadRequest::Unarchive {
                    thread_id: parsed.thread_id,
                },
            )
            .await?;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/message" => {
            let parsed: ThreadMessageParams = parse_params(params_or_object(params))?;
            let response = handle_stdio_thread_message(state, writer, parsed).await?;
            StdioDispatchResult {
                result: response,
                should_exit: false,
            }
        }
        "app/capabilities" => dispatch_stdio_app_request(state, AppRequest::Capabilities).await?,
        "app/request" => {
            let request: AppRequest = parse_params(params)?;
            dispatch_stdio_app_request(state, request).await?
        }
        "app/config/get" => {
            let parsed: ConfigGetParams = parse_params(params_or_object(params))?;
            dispatch_stdio_app_request(state, AppRequest::ConfigGet { key: parsed.key }).await?
        }
        "app/config/set" => {
            let parsed: ConfigSetParams = parse_params(params_or_object(params))?;
            dispatch_stdio_app_request(
                state,
                AppRequest::ConfigSet {
                    key: parsed.key,
                    value: parsed.value,
                },
            )
            .await?
        }
        "app/config/unset" => {
            let parsed: ConfigGetParams = parse_params(params_or_object(params))?;
            dispatch_stdio_app_request(state, AppRequest::ConfigUnset { key: parsed.key }).await?
        }
        "app/config/list" => dispatch_stdio_app_request(state, AppRequest::ConfigList).await?,
        "app/config/reload" => dispatch_stdio_app_request(state, AppRequest::ConfigReload).await?,
        "app/models" => dispatch_stdio_app_request(state, AppRequest::Models).await?,
        "app/thread_loaded_list" | "app/thread-loaded-list" => {
            dispatch_stdio_app_request(state, AppRequest::ThreadLoadedList).await?
        }
        "prompt/capabilities" => StdioDispatchResult {
            result: json!({
                "methods": ["prompt/request", "prompt/run"]
            }),
            should_exit: false,
        },
        "prompt/request" | "prompt/run" => {
            let request: PromptRequest = parse_params(params)?;
            let response = handle_prompt_request(state, request).await?;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "shutdown" => {
            if let Some(bridge) = state.stdio_bridge.lock().await.take() {
                bridge.lock().await.shutdown_child();
            }
            StdioDispatchResult {
                result: json!({"ok": true, "status": "stopped"}),
                should_exit: true,
            }
        }
        _ => return Err(JsonRpcError::method_not_found(method)),
    };
    Ok(outcome)
}

async fn process_app_request(
    state: &AppState,
    req: AppRequest,
    _transport: AppTransport,
) -> AppResponse {
    match req {
        AppRequest::Capabilities => AppResponse {
            ok: true,
            data: json!({
                "routes": ["/thread", "/app", "/prompt", "/tool", "/jobs", "/mcp/startup"],
                "config": ["get", "set", "unset", "list", "reload"],
                "events": ["response_start", "response_delta", "response_end", "tool_call_start", "tool_call_result", "mcp_startup_update", "mcp_startup_complete"],
                "transport": "stdio+http",
                "config_path": state.config_path.as_ref().map(|p| p.display().to_string()),
            }),
            events: Vec::new(),
        },
        AppRequest::ConfigGet { key } => {
            let cfg = state.config.read().await;
            let value = cfg.get_display_value(&key);
            AppResponse {
                ok: true,
                data: json!({ "key": key, "value": value }),
                events: Vec::new(),
            }
        }
        AppRequest::ConfigSet { key, value } => {
            let (result, snapshot) = {
                let mut cfg = state.config.write().await;
                let result = cfg.set_value(&key, &value);
                (result, cfg.clone())
            };
            let ok = result.is_ok();
            let message = result.err().map(|e| e.to_string());
            apply_config_update(state, snapshot, None, true).await;
            AppResponse {
                ok,
                data: json!({ "key": key, "value": value, "error": message }),
                events: Vec::new(),
            }
        }
        AppRequest::ConfigUnset { key } => {
            let (result, snapshot) = {
                let mut cfg = state.config.write().await;
                let result = cfg.unset_value(&key);
                (result, cfg.clone())
            };
            let ok = result.is_ok();
            let message = result.err().map(|e| e.to_string());
            apply_config_update(state, snapshot, None, true).await;
            AppResponse {
                ok,
                data: json!({ "key": key, "error": message }),
                events: Vec::new(),
            }
        }
        AppRequest::ConfigList => {
            let cfg = state.config.read().await;
            AppResponse {
                ok: true,
                data: json!({ "values": cfg.list_values() }),
                events: Vec::new(),
            }
        }
        AppRequest::ConfigReload => {
            // Re-read both `config.toml` and the sibling `permissions.toml`
            // from disk (the headless equivalent of the TUI
            // `reload_runtime_config` codepath) and push the fresh
            // snapshots into `state.config` and the live `Runtime`.
            //
            // `ConfigStore::load` resolves the same default config path
            // that `build_state` used at startup when `config_path` is
            // `None`, so a `None` here reloads from the same on-disk file
            // the server booted from.
            let store = match ConfigStore::load(state.config_path.clone()) {
                Ok(store) => store,
                Err(e) => {
                    return AppResponse {
                        ok: false,
                        data: json!({ "error": format!("failed to load config: {e}") }),
                        events: Vec::new(),
                    };
                }
            };
            let new_config = store.config.clone();
            let new_exec_policy = store.exec_policy_engine();

            // Disk is already the source of truth here, so nothing to
            // persist; the exec policy rides along so the runtime picks up
            // external `permissions.toml` edits too.
            apply_config_update(state, new_config, Some(new_exec_policy), false).await;

            AppResponse {
                ok: true,
                data: json!({ "reloaded": true }),
                events: Vec::new(),
            }
        }
        AppRequest::Models => AppResponse {
            ok: true,
            data: json!({ "models": state.registry.list() }),
            events: Vec::new(),
        },
        AppRequest::ThreadLoadedList => {
            let mut runtime = state.runtime.write().await;
            let response = runtime
                .handle_thread(codewhale_protocol::ThreadRequest::List(
                    codewhale_protocol::ThreadListParams {
                        include_archived: false,
                        limit: Some(50),
                    },
                ))
                .await;
            match response {
                Ok(thread_resp) => AppResponse {
                    ok: true,
                    data: json!({ "threads": thread_resp.threads }),
                    events: thread_resp.events,
                },
                Err(err) => AppResponse {
                    ok: false,
                    data: json!({ "error": err.to_string() }),
                    events: Vec::new(),
                },
            }
        }
        AppRequest::SubmitUserInput {
            request_id,
            answers,
        } => {
            // Record the user's answers against the pending clarification
            // request so a driver can retrieve them. The headless runtime does
            // not block on `request_user_input` (fire-and-return, like
            // approval), so there is no in-flight turn to resume here — the
            // caller is expected to feed these answers into the next turn.
            let mut pending = state.pending_user_input.lock().await;
            if pending.contains_key(&request_id) {
                return AppResponse {
                    ok: false,
                    data: json!({
                        "error": "request_id already resolved",
                        "request_id": request_id,
                    }),
                    events: Vec::new(),
                };
            }
            pending.insert(request_id.clone(), answers);
            AppResponse {
                ok: true,
                data: json!({ "request_id": request_id, "resolved": true }),
                events: Vec::new(),
            }
        }
    }
}

/// Propagate a new config snapshot to every place that must observe it:
/// optionally persist it to disk, install it in the shared `state.config`,
/// push it into the live [`Runtime`], and invalidate the cached stdio
/// bridge so the next stdio request spawns a fresh child that reads the
/// new on-disk config. Shared by `ConfigSet` / `ConfigUnset` / `ConfigReload`.
///
/// `exec_policy` is `Some` only on the reload path, which re-reads
/// `permissions.toml` from disk; set/unset intentionally leave the live
/// exec policy alone (use `ConfigReload` to pick up external permission
/// edits). `persist` is false on the reload path because disk is already
/// the source of truth there.
async fn apply_config_update(
    state: &AppState,
    snapshot: codewhale_config::ConfigToml,
    exec_policy: Option<codewhale_execpolicy::ExecPolicyEngine>,
    persist: bool,
) {
    if persist && let Err(e) = persist_config(state, snapshot.clone()).await {
        tracing::error!("Failed to persist config update: {e}");
    }
    {
        let mut cfg = state.config.write().await;
        *cfg = snapshot.clone();
    }
    // Sync into the live Runtime so the next turn picks up the change
    // without a restart. MCP server connections are NOT refreshed here —
    // see `Runtime::reload_config_and_policy` for the rationale and the
    // matching TUI `mcp_restart_required` note.
    {
        let mut runtime = state.runtime.write().await;
        match exec_policy {
            Some(policy) => runtime.reload_config_and_policy(snapshot, policy),
            None => runtime.update_config(snapshot),
        }
    }
    invalidate_stdio_bridge(state).await;
}

async fn persist_config(state: &AppState, config: codewhale_config::ConfigToml) -> Result<()> {
    if state.config_path.is_none() {
        return Ok(());
    }
    let mut store = ConfigStore::load(state.config_path.clone())?;
    store.config = config;
    store.save()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::extract::{Path as AxumPath, Query};
    use axum::http::header;
    use codewhale_protocol::AppRequest;
    use std::collections::HashMap;
    use std::fs;
    use tokio::io::AsyncReadExt;
    use tower::ServiceExt;

    fn app_with_config(auth_token: Option<&str>) -> (Router, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        fs::write(&config_path, "api_key = \"sk-deepseek-secret\"\n").expect("write config");
        let state = build_state(
            Some(config_path),
            auth_token.map(std::string::ToString::to_string),
        )
        .expect("state");
        (app_router(state, &[]), tmp)
    }

    #[test]
    fn build_state_keeps_resolved_explicit_config_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_dir = tmp.path().join("config-dir");
        fs::create_dir_all(&config_dir).expect("config dir");
        let config_path = config_dir.join("config.toml");
        fs::write(&config_path, "api_key = \"sk-deepseek-secret\"\n").expect("write config");

        let state = build_state(Some(config_path.clone()), None).expect("state");

        assert_eq!(
            state.config_path.as_deref(),
            Some(
                config_path
                    .canonicalize()
                    .expect("canonical config")
                    .as_path()
            )
        );
    }

    async fn response_body_json(response: Response) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        serde_json::from_slice(&bytes).expect("json response")
    }

    #[tokio::test]
    async fn http_app_routes_require_bearer_token_when_auth_enabled() {
        let (app, _tmp) = app_with_config(Some("test-token"));
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/app")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&AppRequest::ConfigGet {
                            key: "api_key".to_string(),
                        })
                        .expect("request json"),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn http_config_get_redacts_sensitive_values_after_auth() {
        let (app, _tmp) = app_with_config(Some("test-token"));
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/app")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&AppRequest::ConfigGet {
                            key: "api_key".to_string(),
                        })
                        .expect("request json"),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_body_json(response).await;
        assert_eq!(body["data"]["value"], "sk-d***cret");
    }

    #[tokio::test]
    async fn cors_does_not_allow_arbitrary_origins() {
        let (app, _tmp) = app_with_config(Some("test-token"));
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/healthz")
                    .header(header::ORIGIN, "https://attacker.example")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }

    #[tokio::test]
    async fn build_state_loads_permissions_into_runtime_policy() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        fs::write(&config_path, "api_key = \"sk-deepseek-secret\"\n").expect("write config");
        fs::write(
            tmp.path().join("permissions.toml"),
            r#"
            [[rules]]
            tool = "exec_shell"
            command = "cargo test"
            "#,
        )
        .expect("write permissions");

        let state = build_state(Some(config_path), None).expect("state");
        let runtime = state.runtime.read().await;
        let decision = runtime
            .exec_policy
            .check(codewhale_execpolicy::ExecPolicyContext {
                command: "cargo test --workspace",
                cwd: "/workspace",
                tool: Some("exec_shell"),
                path: None,
                ask_for_approval: codewhale_execpolicy::AskForApproval::UnlessTrusted,
                sandbox_mode: Some("workspace-write"),
            })
            .expect("policy check");

        assert!(decision.allow);
        assert!(decision.requires_approval);
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("tool=exec_shell command=cargo test")
        );
    }

    #[tokio::test]
    async fn config_reload_refreshes_runtime_config_and_exec_policy_from_disk() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        fs::write(
            &config_path,
            "api_key = \"sk-deepseek-secret\"\nmodel = \"deepseek-chat\"\n",
        )
        .expect("write config");
        // No permissions.toml at startup → exec_policy starts empty.
        let state = build_state(Some(config_path.clone()), None).expect("state");

        // Sanity: initial runtime sees the on-disk model and has no rule.
        {
            let runtime = state.runtime.read().await;
            assert_eq!(runtime.config.model.as_deref(), Some("deepseek-chat"));
            let decision = runtime
                .exec_policy
                .check(codewhale_execpolicy::ExecPolicyContext {
                    command: "cargo test",
                    cwd: "/workspace",
                    tool: Some("exec_shell"),
                    path: None,
                    ask_for_approval: codewhale_execpolicy::AskForApproval::UnlessTrusted,
                    sandbox_mode: Some("workspace-write"),
                })
                .expect("policy check");
            assert!(decision.matched_rule.is_none());
        }

        // Edit both files on disk: new model + a permission rule.
        fs::write(
            &config_path,
            "api_key = \"sk-deepseek-secret\"\nmodel = \"deepseek-reasoner\"\n",
        )
        .expect("rewrite config");
        fs::write(
            tmp.path().join("permissions.toml"),
            r#"
            [[rules]]
            tool = "exec_shell"
            command = "cargo test"
            "#,
        )
        .expect("write permissions");

        // ConfigReload must re-read both files and push them into the
        // live Runtime without a restart.
        let response =
            process_app_request(&state, AppRequest::ConfigReload, AppTransport::Stdio).await;
        assert!(response.ok, "reload should succeed");
        assert_eq!(response.data["reloaded"], true);

        // The shared config lock reflects the new model.
        {
            let cfg = state.config.read().await;
            assert_eq!(cfg.model.as_deref(), Some("deepseek-reasoner"));
        }
        // The live Runtime reflects both the new model and the new rule.
        {
            let runtime = state.runtime.read().await;
            assert_eq!(runtime.config.model.as_deref(), Some("deepseek-reasoner"));
            let decision = runtime
                .exec_policy
                .check(codewhale_execpolicy::ExecPolicyContext {
                    command: "cargo test --workspace",
                    cwd: "/workspace",
                    tool: Some("exec_shell"),
                    path: None,
                    ask_for_approval: codewhale_execpolicy::AskForApproval::UnlessTrusted,
                    sandbox_mode: Some("workspace-write"),
                })
                .expect("policy check");
            assert!(decision.allow);
            assert!(decision.requires_approval);
            assert_eq!(
                decision.matched_rule.as_deref(),
                Some("tool=exec_shell command=cargo test")
            );
        }
    }

    #[tokio::test]
    async fn config_set_propagates_to_runtime_config_without_touching_exec_policy() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        fs::write(
            &config_path,
            "api_key = \"sk-deepseek-secret\"\nmodel = \"deepseek-chat\"\n",
        )
        .expect("write config");
        let state = build_state(Some(config_path.clone()), None).expect("state");

        // Set a new model via the API. Only config.toml is touched; no
        // permissions.toml exists, so exec_policy must stay empty.
        let response = process_app_request(
            &state,
            AppRequest::ConfigSet {
                key: "model".to_string(),
                value: "deepseek-reasoner".to_string(),
            },
            AppTransport::Stdio,
        )
        .await;
        assert!(response.ok, "set should succeed");

        // Live runtime sees the new model.
        {
            let runtime = state.runtime.read().await;
            assert_eq!(runtime.config.model.as_deref(), Some("deepseek-reasoner"));
            // exec_policy was empty at startup and must remain empty.
            let decision = runtime
                .exec_policy
                .check(codewhale_execpolicy::ExecPolicyContext {
                    command: "cargo test",
                    cwd: "/workspace",
                    tool: Some("exec_shell"),
                    path: None,
                    ask_for_approval: codewhale_execpolicy::AskForApproval::UnlessTrusted,
                    sandbox_mode: Some("workspace-write"),
                })
                .expect("policy check");
            assert!(decision.matched_rule.is_none());
        }
        // The on-disk file was persisted.
        let persisted = fs::read_to_string(&config_path).expect("read config");
        assert!(persisted.contains("deepseek-reasoner"));
    }

    #[tokio::test]
    async fn config_unset_propagates_to_runtime_config() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        fs::write(
            &config_path,
            "api_key = \"sk-deepseek-secret\"\nmodel = \"deepseek-chat\"\n",
        )
        .expect("write config");
        let state = build_state(Some(config_path.clone()), None).expect("state");

        // Sanity: runtime starts with the on-disk model.
        {
            let runtime = state.runtime.read().await;
            assert_eq!(runtime.config.model.as_deref(), Some("deepseek-chat"));
        }

        // Unset the model via the API. This walks a separate code path
        // from ConfigSet (unset_value + update_config), so it needs its
        // own regression coverage.
        let response = process_app_request(
            &state,
            AppRequest::ConfigUnset {
                key: "model".to_string(),
            },
            AppTransport::Stdio,
        )
        .await;
        assert!(response.ok, "unset should succeed");

        // Live runtime sees the cleared model.
        {
            let runtime = state.runtime.read().await;
            assert!(runtime.config.model.is_none());
        }
        // Shared config lock agrees.
        {
            let cfg = state.config.read().await;
            assert!(cfg.model.is_none());
        }
        // The on-disk file no longer carries the model value.
        let persisted = fs::read_to_string(&config_path).expect("read config");
        assert!(!persisted.contains("deepseek-chat"));
    }

    #[tokio::test]
    async fn config_reload_returns_error_when_disk_config_is_invalid() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        fs::write(
            &config_path,
            "api_key = \"sk-deepseek-secret\"\nmodel = \"deepseek-chat\"\n",
        )
        .expect("write config");
        let state = build_state(Some(config_path.clone()), None).expect("state");

        // Corrupt the on-disk config so ConfigStore::load fails to parse.
        fs::write(&config_path, "api_key = \"unterminated\n").expect("corrupt config");

        let response =
            process_app_request(&state, AppRequest::ConfigReload, AppTransport::Stdio).await;
        assert!(!response.ok, "reload of corrupt config must fail");
        let err = response.data["error"]
            .as_str()
            .expect("error message present")
            .to_string();
        assert!(
            err.contains("failed to load config"),
            "error should mention load failure, got: {err}"
        );

        // Live state is untouched: the early-return on load error must
        // not have clobbered runtime.config or state.config.
        {
            let runtime = state.runtime.read().await;
            assert_eq!(runtime.config.model.as_deref(), Some("deepseek-chat"));
        }
        {
            let cfg = state.config.read().await;
            assert_eq!(cfg.model.as_deref(), Some("deepseek-chat"));
        }
    }

    async fn seed_test_bridge(state: &AppState) -> SharedRuntimeBridge {
        let bridge = Arc::new(Mutex::new(RuntimeBridge::from_base_url_for_test(
            "http://127.0.0.1:9".to_string(),
        )));
        *state.stdio_bridge.lock().await = Some(bridge.clone());
        bridge
    }

    #[tokio::test]
    async fn config_set_invalidates_cached_stdio_bridge() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        fs::write(&config_path, "model = \"deepseek-chat\"\n").expect("write config");
        let state = build_state(Some(config_path), None).expect("state");
        seed_test_bridge(&state).await;

        let response = process_app_request(
            &state,
            AppRequest::ConfigSet {
                key: "model".to_string(),
                value: "deepseek-reasoner".to_string(),
            },
            AppTransport::Stdio,
        )
        .await;
        assert!(response.ok, "set should succeed");

        // The cached bridge child must be dropped so the next stdio request
        // spawns a fresh runtime that reads the persisted config.
        assert!(state.stdio_bridge.lock().await.is_none());
    }

    #[tokio::test]
    async fn config_reload_invalidates_cached_stdio_bridge() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        fs::write(&config_path, "model = \"deepseek-chat\"\n").expect("write config");
        let state = build_state(Some(config_path), None).expect("state");
        seed_test_bridge(&state).await;

        let response =
            process_app_request(&state, AppRequest::ConfigReload, AppTransport::Stdio).await;
        assert!(response.ok, "reload should succeed");

        assert!(state.stdio_bridge.lock().await.is_none());
    }

    #[tokio::test]
    async fn stdio_bridge_invalidation_not_blocked_by_in_flight_turn() {
        let (state, _tmp) = capability_test_state();
        let bridge = seed_test_bridge(&state).await;

        // Simulate a long streaming turn holding the inner bridge lock.
        let _in_flight = bridge.lock().await;

        // Invalidation only touches the cache slot, so it must complete
        // without waiting for the in-flight turn to release the bridge.
        tokio::time::timeout(Duration::from_secs(1), invalidate_stdio_bridge(&state))
            .await
            .expect("invalidation must not wait on bridge traffic");
        assert!(state.stdio_bridge.lock().await.is_none());
    }

    #[tokio::test]
    async fn runtime_read_paths_run_concurrently() {
        // Tool/status/mcp handlers take read guards; two must coexist so a
        // long-running tool call cannot serialize unrelated requests. With
        // the old `Mutex<Runtime>` this pattern would deadlock.
        let (state, _tmp) = capability_test_state();
        let first = state.runtime.read().await;
        let second = state.runtime.read().await;
        assert!(first.app_status().ok);
        assert!(second.app_status().ok);
    }

    #[tokio::test]
    async fn health_probes_advertise_legacy_deepseek_service_name() {
        // External probes still key off the DeepSeek-era service name; both
        // transports must serve it from the single compat shim.
        let (app, _tmp) = app_with_config(None);
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/healthz")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let body = response_body_json(response).await;
        assert_eq!(body["service"], legacy_deepseek_compat::SERVICE_NAME);
        assert_eq!(body["service"], "deepseek-app-server");

        let (state, _tmp) = capability_test_state();
        let stdio = dispatch_stdio_request(&state, "healthz", json!({}))
            .await
            .expect("stdio healthz");
        assert_eq!(
            stdio.result["service"],
            legacy_deepseek_compat::SERVICE_NAME
        );
    }

    #[test]
    fn non_loopback_bind_without_auth_fails_fast() {
        let options = AppServerOptions {
            listen: "0.0.0.0:8787".parse().expect("socket addr"),
            config_path: None,
            auth_token: None,
            insecure_no_auth: false,
            cors_origins: Vec::new(),
        };

        let err =
            resolve_auth_token(&options).expect_err("non-loopback generated auth should fail");
        assert!(err.to_string().contains("without explicit auth token"));
    }

    #[tokio::test]
    async fn stdio_transport_redacts_config_get_secrets() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        fs::write(&config_path, "").expect("write config");
        let state = build_state(Some(config_path), None).expect("state");
        {
            let mut cfg = state.config.write().await;
            cfg.api_key = Some("sk-deepseek-secret".to_string());
        }

        let response = process_app_request(
            &state,
            AppRequest::ConfigGet {
                key: "api_key".to_string(),
            },
            AppTransport::Stdio,
        )
        .await;

        assert_eq!(response.data["value"], "sk-d***cret");
    }

    #[tokio::test]
    async fn stdio_thread_goal_methods_round_trip_persisted_goal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        fs::write(&config_path, "").expect("write config");
        let state = build_state(Some(config_path), None).expect("state");

        let capabilities = dispatch_stdio_request(&state, "thread/capabilities", json!({}))
            .await
            .expect("thread capabilities");
        assert!(
            capabilities.result["methods"]
                .as_array()
                .expect("methods")
                .iter()
                .any(|method| method == "thread/goal/set")
        );

        let started = dispatch_stdio_request(&state, "thread/start", json!({}))
            .await
            .expect("start thread");
        let thread_id = started.result["thread_id"]
            .as_str()
            .expect("thread id")
            .to_string();

        let set = dispatch_stdio_request(
            &state,
            "thread/goal/set",
            json!({
                "thread_id": thread_id,
                "objective": "Release 0.8.59",
                "token_budget": 59000
            }),
        )
        .await
        .expect("set goal");
        assert_eq!(set.result["status"], "ok");
        assert_eq!(set.result["goal"]["objective"], "Release 0.8.59");
        assert_eq!(set.result["goal"]["status"], "active");

        let got = dispatch_stdio_request(
            &state,
            "thread/goal/get",
            json!({
                "thread_id": thread_id
            }),
        )
        .await
        .expect("get goal");
        assert_eq!(got.result["goal"]["token_budget"], 59000);

        let cleared = dispatch_stdio_request(
            &state,
            "thread/goal/clear",
            json!({
                "thread_id": thread_id
            }),
        )
        .await
        .expect("clear goal");
        assert_eq!(cleared.result["status"], "cleared");
        assert_eq!(cleared.result["data"]["cleared"], true);
    }

    fn sse_frame(event: &str, payload: Value) -> String {
        format!("event: {event}\ndata: {payload}\n\n")
    }

    #[tokio::test]
    async fn stdio_runtime_bridge_streams_response_delta_events() {
        async fn create_turn(AxumPath(thread_id): AxumPath<String>) -> Json<Value> {
            Json(json!({
                "thread": { "id": thread_id },
                "turn": { "id": "turn_test" },
            }))
        }

        async fn thread_events(
            AxumPath(thread_id): AxumPath<String>,
            Query(query): Query<HashMap<String, String>>,
        ) -> ([(header::HeaderName, &'static str); 1], String) {
            assert_eq!(thread_id, "thr_test");
            assert_eq!(query.get("since_seq").map(String::as_str), Some("0"));

            let body = [
                sse_frame(
                    "item.delta",
                    json!({
                        "seq": 1,
                        "turn_id": "turn_test",
                        "payload": {
                            "kind": "agent_message",
                            "delta": "hello"
                        }
                    }),
                ),
                sse_frame(
                    "turn.completed",
                    json!({
                        "seq": 2,
                        "turn_id": "turn_test",
                        "payload": {
                            "turn": {
                                "status": "completed"
                            }
                        }
                    }),
                ),
            ]
            .concat();

            ([(header::CONTENT_TYPE, "text/event-stream")], body)
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("listener addr");
        let app = Router::new()
            .route("/v1/threads/{thread_id}/turns", post(create_turn))
            .route("/v1/threads/{thread_id}/events", get(thread_events));

        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve test runtime");
        });

        let mut bridge = RuntimeBridge::from_base_url_for_test(format!("http://{addr}"));
        let (mut reader, mut writer) = tokio::io::duplex(4096);

        let result = bridge
            .message_thread("thr_test", "hello", &mut writer)
            .await
            .expect("message_thread should succeed");
        drop(writer);

        let mut stdout = Vec::new();
        reader
            .read_to_end(&mut stdout)
            .await
            .expect("read stdio output");
        server.abort();
        let _ = server.await;

        let lines: Vec<Value> = String::from_utf8(stdout)
            .expect("utf8 output")
            .lines()
            .map(|line| serde_json::from_str(line).expect("json line"))
            .collect();

        assert_eq!(
            result.get("status").and_then(Value::as_str),
            Some("accepted")
        );
        assert_eq!(
            result.pointer("/data/turn_id").and_then(Value::as_str),
            Some("turn_test")
        );
        assert_eq!(bridge.last_seq_by_thread.get("thr_test"), Some(&2));

        let event_types: Vec<&str> = lines
            .iter()
            .map(|line| {
                line.get("type")
                    .and_then(Value::as_str)
                    .expect("event type")
            })
            .collect();
        assert_eq!(
            event_types,
            vec!["response_start", "response_delta", "response_end"]
        );
        assert_eq!(lines[1]["delta"], "hello");
    }

    #[tokio::test]
    async fn stdio_runtime_bridge_applies_thread_start_hints() {
        async fn create_thread(Json(body): Json<Value>) -> Json<Value> {
            assert_eq!(body["model"], "deepseek-v4");
            assert_eq!(body["workspace"], "/tmp/codewhale-stdio");
            Json(json!({
                "id": "thr_runtime",
                "model": body["model"].clone(),
                "workspace": body["workspace"].clone(),
            }))
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("listener addr");
        let app = Router::new().route("/v1/threads", post(create_thread));

        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve test runtime");
        });

        let mut bridge = RuntimeBridge::from_base_url_for_test(format!("http://{addr}"));
        let runtime_id = bridge
            .ensure_runtime_thread(
                "legacy_thread",
                Some(RuntimeThreadHint {
                    model: Some("deepseek-v4".to_string()),
                    workspace: Some(PathBuf::from("/tmp/codewhale-stdio")),
                }),
            )
            .await
            .expect("runtime thread");
        server.abort();
        let _ = server.await;

        assert_eq!(runtime_id, "thr_runtime");
        assert_eq!(
            bridge.thread_map.get("legacy_thread").map(String::as_str),
            Some("thr_runtime")
        );
    }

    // ── capability drift guard ─────────────────────────────────────────
    //
    // The stdio `capabilities` method is the benchmark/SDK contract: external
    // harnesses probe it (without spending model tokens) to learn what the
    // app-server can do. Pin the advertised method set so any change forces a
    // deliberate update here, in the dispatcher, and in docs/RUNTIME_API.md.

    /// Methods advertised by the top-level `capabilities` probe, in order.
    const EXPECTED_CAPABILITY_METHODS: &[&str] = &[
        "healthz",
        "thread/capabilities",
        "thread/request",
        "thread/create",
        "thread/start",
        "thread/resume",
        "thread/fork",
        "thread/list",
        "thread/read",
        "thread/set_name",
        "thread/goal/set",
        "thread/goal/get",
        "thread/goal/clear",
        "thread/archive",
        "thread/unarchive",
        "thread/message",
        "app/capabilities",
        "app/request",
        "app/config/get",
        "app/config/set",
        "app/config/unset",
        "app/config/list",
        "app/config/reload",
        "app/models",
        "app/thread_loaded_list",
        "prompt/capabilities",
        "prompt/request",
        "prompt/run",
        "shutdown",
    ];

    fn capability_test_state() -> (AppState, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        fs::write(&config_path, "").expect("write config");
        let state = build_state(Some(config_path), None).expect("state");
        (state, tmp)
    }

    #[tokio::test]
    async fn capabilities_method_set_is_stable() {
        let (state, _tmp) = capability_test_state();
        let caps = dispatch_stdio_request(&state, "capabilities", json!({}))
            .await
            .expect("capabilities dispatch");
        let methods: Vec<String> = caps.result["methods"]
            .as_array()
            .expect("methods array")
            .iter()
            .map(|m| m.as_str().expect("method string").to_string())
            .collect();
        assert_eq!(
            methods, EXPECTED_CAPABILITY_METHODS,
            "app-server stdio capability set drifted; update the dispatcher, this \
             snapshot, and docs/RUNTIME_API.md together"
        );
    }

    #[tokio::test]
    async fn every_advertised_capability_is_dispatchable() {
        let (state, _tmp) = capability_test_state();
        // Empty params: methods may fail validation (-32602), but none may report
        // method-not-found (-32601). Required fields (e.g. PromptRequest.prompt)
        // make the prompt routes fail at parse time, so no model tokens are spent.
        for method in EXPECTED_CAPABILITY_METHODS {
            if let Err(err) = dispatch_stdio_request(&state, method, json!({})).await {
                assert_ne!(
                    err.code,
                    JsonRpcError::method_not_found(method).code,
                    "advertised capability `{method}` is not dispatchable"
                );
            }
        }
    }

    // ── resolve_auth_token ─────────────────────────────────────────────

    #[test]
    fn auth_token_empty_string_fails() {
        let options = AppServerOptions {
            listen: "127.0.0.1:0".parse().expect("addr"),
            config_path: None,
            auth_token: Some("  ".to_string()),
            insecure_no_auth: false,
            cors_origins: Vec::new(),
        };
        let err = resolve_auth_token(&options).expect_err("empty token should fail");
        assert!(err.to_string().contains("cannot be empty"));
    }

    #[test]
    fn auth_token_generated_when_none_provided() {
        let options = AppServerOptions {
            listen: "127.0.0.1:0".parse().expect("addr"),
            config_path: None,
            auth_token: None,
            insecure_no_auth: false,
            cors_origins: Vec::new(),
        };
        let token = resolve_auth_token(&options).unwrap();
        assert!(token.is_some());
        assert!(token.unwrap().starts_with("cwapp_"));
    }

    #[test]
    fn generated_auth_status_does_not_render_token() {
        let rendered = app_server_auth_status_lines(false).join("\n");

        assert!(!rendered.contains("Authorization: Bearer"));
        assert!(rendered.contains("not printed"));
        assert!(rendered.contains("CODEWHALE_APP_SERVER_TOKEN"));
    }

    #[test]
    fn auth_token_explicit_is_preserved() {
        let options = AppServerOptions {
            listen: "127.0.0.1:0".parse().expect("addr"),
            config_path: None,
            auth_token: Some("my-secret".to_string()),
            insecure_no_auth: false,
            cors_origins: Vec::new(),
        };
        let token = resolve_auth_token(&options).unwrap();
        assert_eq!(token.as_deref(), Some("my-secret"));
    }

    #[test]
    fn auth_token_explicit_allows_non_loopback_bind() {
        let options = AppServerOptions {
            listen: "0.0.0.0:8787".parse().expect("socket addr"),
            config_path: None,
            auth_token: Some("my-secret".to_string()),
            insecure_no_auth: false,
            cors_origins: Vec::new(),
        };
        let token = resolve_auth_token(&options).unwrap();
        assert_eq!(token.as_deref(), Some("my-secret"));
    }

    #[test]
    fn insecure_no_auth_on_loopback_returns_none() {
        let options = AppServerOptions {
            listen: "127.0.0.1:0".parse().expect("addr"),
            config_path: None,
            auth_token: None,
            insecure_no_auth: true,
            cors_origins: Vec::new(),
        };
        let token = resolve_auth_token(&options).unwrap();
        assert!(token.is_none());
    }

    #[test]
    fn insecure_no_auth_on_non_loopback_fails_fast() {
        let options = AppServerOptions {
            listen: "0.0.0.0:8787".parse().expect("socket addr"),
            config_path: None,
            auth_token: None,
            insecure_no_auth: true,
            cors_origins: Vec::new(),
        };

        let err = resolve_auth_token(&options).expect_err("non-loopback unauth should fail");
        assert!(
            err.to_string()
                .contains("refusing unauthenticated app-server bind")
        );
    }

    // ── cors_layer ─────────────────────────────────────────────────────

    #[test]
    fn cors_layer_includes_default_origins() {
        let layer = cors_layer(&[]);
        // Just verify it doesn't panic and creates successfully
        let _ = layer;
    }

    #[test]
    fn cors_layer_adds_extra_origins() {
        let extras = vec!["https://example.com".to_string()];
        let layer = cors_layer(&extras);
        let _ = layer;
    }

    #[test]
    fn cors_layer_skips_empty_origins() {
        let extras = vec!["".to_string(), "  ".to_string()];
        let layer = cors_layer(&extras);
        let _ = layer;
    }

    // ── JsonRpc helpers ────────────────────────────────────────────────

    #[test]
    fn params_or_object_returns_object_for_null() {
        let result = params_or_object(Value::Null);
        assert_eq!(result, json!({}));
    }

    #[test]
    fn params_or_object_passthrough_for_non_null() {
        let input = json!({"key": "value"});
        let result = params_or_object(input.clone());
        assert_eq!(result, input);
    }

    #[test]
    fn jsonrpc_result_format() {
        let result = jsonrpc_result(Some(json!(1)), json!({"ok": true}));
        assert_eq!(result["jsonrpc"], "2.0");
        assert_eq!(result["id"], 1);
        assert_eq!(result["result"]["ok"], true);
    }

    #[test]
    fn jsonrpc_result_null_id() {
        let result = jsonrpc_result(None, json!(null));
        assert_eq!(result["id"], Value::Null);
    }

    #[test]
    fn jsonrpc_error_format() {
        let err = jsonrpc_error(Some(json!(2)), JsonRpcError::internal("oops"));
        assert_eq!(err["jsonrpc"], "2.0");
        assert_eq!(err["id"], 2);
        assert_eq!(err["error"]["code"], -32603);
        assert_eq!(err["error"]["message"], "oops");
    }

    #[test]
    fn jsonrpc_error_codes() {
        assert_eq!(JsonRpcError::parse_error("").code, -32700);
        assert_eq!(JsonRpcError::invalid_request("").code, -32600);
        assert_eq!(JsonRpcError::method_not_found("x").code, -32601);
        assert_eq!(JsonRpcError::invalid_params("").code, -32602);
        assert_eq!(JsonRpcError::internal("").code, -32603);
    }

    // ── AppServerOptions ───────────────────────────────────────────────

    #[test]
    fn app_server_options_debug_does_not_leak_token() {
        let options = AppServerOptions {
            listen: "127.0.0.1:8080".parse().expect("addr"),
            config_path: None,
            auth_token: Some("secret-token".to_string()),
            insecure_no_auth: false,
            cors_origins: vec!["https://example.com".to_string()],
        };
        let debug = format!("{options:?}");
        assert!(!debug.contains("secret-token"));
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("8080"));
    }

    // ── Default CORS origins ──────────────────────────────────────────

    #[test]
    fn default_cors_origins_include_common_dev_ports() {
        assert!(DEFAULT_CORS_ORIGINS.contains(&"http://localhost:3000"));
        assert!(DEFAULT_CORS_ORIGINS.contains(&"http://localhost:5173"));
        assert!(DEFAULT_CORS_ORIGINS.contains(&"tauri://localhost"));
    }
}
