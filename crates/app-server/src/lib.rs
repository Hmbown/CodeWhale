use std::collections::{HashMap, VecDeque};
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
use codewhale_config::ConfigStore;
use codewhale_core::Runtime;
use codewhale_hooks::{HookDispatcher, JsonlHookSink, StdoutHookSink, UnixSocketHookSink};
use codewhale_mcp::McpManager;
use codewhale_protocol::{
    AppRequest, AppResponse, EventFrame, PromptRequest, PromptResponse, ResponseChannel,
    ThreadGoalClearParams, ThreadGoalGetParams, ThreadGoalSetParams, ThreadRequest, ThreadResponse,
};
use codewhale_state::StateStore;
use codewhale_tools::{ToolCall, ToolRegistry};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, RwLock};
use tower_http::cors::CorsLayer;
use uuid::Uuid;

mod chat_completions;
pub mod daemon_socket;

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

/// Cached app-server→runtime bridge handle.
///
/// The outer [`AppState::runtime_bridge`] mutex guards only the cache slot;
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
    /// Cached bridge to the real runtime API. Shared by every surface that
    /// executes a turn — stdio `thread/message`, HTTP `/thread` messages, and
    /// both `/prompt` transports — because there is exactly one turn engine.
    runtime_bridge: Arc<Mutex<Option<SharedRuntimeBridge>>>,
    stdio_thread_hints: Arc<Mutex<HashMap<String, RuntimeThreadHint>>>,
    /// Turns currently streaming over stdio, keyed by stdio thread id.
    ///
    /// Deliberately kept *outside* the bridge mutex: a streaming turn holds
    /// that mutex for its entire duration, so anything reachable only through
    /// it cannot be used to stop the turn. This holds its own copy of what an
    /// interrupt needs, so a cancel never waits on the turn it is cancelling.
    in_flight_turns: Arc<Mutex<HashMap<String, InFlightTurn>>>,
}

/// Everything needed to interrupt a running turn without the bridge lock.
#[derive(Debug, Clone)]
struct InFlightTurn {
    base_url: String,
    auth_token: Option<String>,
    /// Thread id as the *runtime* knows it, not the stdio-facing id.
    runtime_thread_id: String,
    turn_id: String,
}

type TurnRegistry = Arc<Mutex<HashMap<String, InFlightTurn>>>;

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

/// Server error: the app-server could not reach the runtime that executes
/// turns. Kept in the JSON-RPC implementation-defined server range
/// (-32000..-32099) alongside `thread_not_found` (-32004).
const RUNTIME_UNAVAILABLE_CODE: i64 = -32005;
/// Server error: the named thread does not exist.
const THREAD_NOT_FOUND_CODE: i64 = -32004;
/// Server error: a daemon-socket client tried to act before `daemon/attach`.
/// Only the unix listener raises it; gated so the Windows build (where the
/// listener is a typed-unsupported stub) does not fail `warnings = "deny"`
/// on dead code.
#[cfg(unix)]
const ATTACH_REQUIRED_CODE: i64 = -32010;
/// Server error: a `daemon/attach` claim lost to a live owner.
#[cfg(unix)]
const DAEMON_ALREADY_CLAIMED_CODE: i64 = -32011;
/// Server error: only the owning client may `shutdown` the daemon.
const NOT_DAEMON_OWNER_CODE: i64 = -32012;
/// Server error: the client refused the daemon's version at attach time.
#[cfg(unix)]
const DAEMON_VERSION_SKEW_CODE: i64 = -32013;
/// Server error: `daemon/attach` sent twice on one connection.
const ALREADY_ATTACHED_CODE: i64 = -32014;

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

/// Structured capture of one bridged turn, for callers that must *return*
/// the turn instead of streaming it (HTTP `/prompt`, HTTP `/thread` messages).
///
/// The stdio path streams the same events to its writer and needs none of
/// this, so it passes `None` and pays nothing.
#[derive(Debug, Default)]
struct TurnTranscript {
    /// Concatenated `agent_message` deltas — the model's actual output.
    text: String,
    /// The model the runtime reports for the thread that ran the turn.
    model: Option<String>,
    /// The same frames the stdio path writes, in order.
    events: Vec<EventFrame>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppTransport {
    Http,
    Stdio,
    /// Unix-domain-socket daemon transport (`daemon_socket`). Speaks the
    /// stdio JSON-RPC protocol verbatim after a `daemon/attach` handshake.
    Socket,
}

impl AppTransport {
    /// Wire label reported by `healthz` / `capabilities`.
    fn label(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Stdio => "stdio",
            Self::Socket => "unix-socket",
        }
    }
}

/// Whether the peer driving a JSON-RPC loop may stop the whole server.
///
/// The process-owned stdio loop always may (its peer *is* the supervisor).
/// On the daemon socket only the client that claimed the daemon may; every
/// other attached client is refused with `not_daemon_owner` — the brief's
/// "never terminate a daemon the app did not spawn", enforced server-side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownAuthority {
    Granted,
    Denied,
}

/// Per-connection policy for [`run_stdio_loop`].
#[derive(Debug, Clone, Copy)]
struct StdioLoopPolicy {
    transport: AppTransport,
    shutdown: ShutdownAuthority,
}

impl StdioLoopPolicy {
    /// The loop owned by the process's own stdin/stdout.
    const fn process_stdio() -> Self {
        Self {
            transport: AppTransport::Stdio,
            shutdown: ShutdownAuthority::Granted,
        }
    }
}

/// Why [`run_stdio_loop`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StdioLoopExit {
    /// The peer closed its write side; nothing asked the server to stop.
    InputClosed,
    /// The peer sent an honoured `shutdown`.
    Shutdown,
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

#[derive(Debug, Deserialize)]
struct ThreadInterruptParams {
    thread_id: String,
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
    let state = build_state_with_transport(config_path, None, AppTransport::Stdio)?;
    let reader = BufReader::new(tokio::io::stdin()).lines();
    let writer = tokio::io::BufWriter::new(tokio::io::stdout());
    run_stdio_loop(
        &state,
        reader,
        writer,
        StdioLoopPolicy::process_stdio(),
        None::<()>,
    )
    .await
    .map(|_exit| ())
}

/// The stdio JSON-RPC loop, generic over its transport so it can be driven by
/// a duplex pipe in tests rather than the process's real stdin/stdout.
async fn run_stdio_loop<R, W, C>(
    state: &AppState,
    mut reader: tokio::io::Lines<R>,
    mut writer: W,
    policy: StdioLoopPolicy,
    // Dropped the moment input closes, not when the in-flight turn ends. The
    // socket transport passes its owner claim here: a `thread/message` can run
    // for minutes, and an owner who disconnects mid-turn must not keep the
    // daemon claimed for the rest of it, or a relaunched client is locked out
    // with `daemon_already_claimed` and cannot even shut the daemon down.
    // Process stdio has no claim and passes `None`.
    mut input_claim: Option<C>,
) -> Result<StdioLoopExit>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
    C: Send,
{
    // Work that arrived while a turn was streaming. The turn owns the writer
    // for its whole duration, so these wait for it rather than interleaving
    // into the middle of a response.
    let mut pending: VecDeque<PendingStdioWork> = VecDeque::new();
    let mut stdin_open = true;

    loop {
        let request = match pending.pop_front() {
            Some(PendingStdioWork::Response(response)) => {
                write_stdio_line(&mut writer, &response).await?;
                continue;
            }
            Some(PendingStdioWork::Request(request)) => request,
            None => {
                if !stdin_open {
                    return Ok(StdioLoopExit::InputClosed);
                }
                let Some(line) = reader.next_line().await? else {
                    return Ok(StdioLoopExit::InputClosed);
                };
                match parse_stdio_line(&line) {
                    ParsedStdioLine::Blank => continue,
                    ParsedStdioLine::Rejected(response) => {
                        write_stdio_line(&mut writer, &response).await?;
                        continue;
                    }
                    ParsedStdioLine::Request(request) => request,
                }
            }
        };

        let id = request.id.clone();
        if request.method == "shutdown" && policy.shutdown == ShutdownAuthority::Denied {
            write_stdio_line(
                &mut writer,
                &jsonrpc_error(id, JsonRpcError::not_daemon_owner()),
            )
            .await?;
            continue;
        }
        let dispatched = if request.method == "thread/message" {
            // A turn can run for minutes. Keep reading stdin while it streams
            // so an interrupt (or a shutdown) can actually reach it — with a
            // plain `await` here, nothing could be read until it finished.
            let dispatch = dispatch_stdio_request_with_writer(
                state,
                &mut writer,
                &request.method,
                request.params,
                policy.transport,
            );
            tokio::pin!(dispatch);
            loop {
                tokio::select! {
                    outcome = &mut dispatch => break outcome,
                    line = reader.next_line(), if stdin_open => {
                        match line? {
                            None => {
                                stdin_open = false;
                                // Release the claim here, not after `dispatch`
                                // resolves.
                                drop(input_claim.take());
                            }
                            Some(line) => {
                                handle_line_during_turn(state, &line, &mut pending, policy).await;
                            }
                        }
                    }
                }
            }
        } else {
            dispatch_stdio_request_with_writer(
                state,
                &mut writer,
                &request.method,
                request.params,
                policy.transport,
            )
            .await
        };

        match dispatched {
            Ok(dispatch) => {
                write_stdio_line(&mut writer, &jsonrpc_result(id, dispatch.result)).await?;
                if dispatch.should_exit {
                    return Ok(StdioLoopExit::Shutdown);
                }
            }
            Err(err) => {
                write_stdio_line(&mut writer, &jsonrpc_error(id, err)).await?;
            }
        }
    }
}

/// Work deferred until a streaming turn releases the writer.
enum PendingStdioWork {
    /// Already answered (an interrupt acted immediately); just needs writing.
    Response(Value),
    /// Not started yet; runs normally once the turn is done.
    Request(JsonRpcRequest),
}

enum ParsedStdioLine {
    Blank,
    Request(JsonRpcRequest),
    Rejected(Value),
}

fn parse_stdio_line(line: &str) -> ParsedStdioLine {
    if line.trim().is_empty() {
        return ParsedStdioLine::Blank;
    }
    let request: JsonRpcRequest = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(err) => {
            return ParsedStdioLine::Rejected(jsonrpc_error(
                None,
                JsonRpcError::parse_error(format!("invalid json: {err}")),
            ));
        }
    };
    if request
        .jsonrpc
        .as_deref()
        .is_some_and(|version| version != "2.0")
    {
        return ParsedStdioLine::Rejected(jsonrpc_error(
            request.id,
            JsonRpcError::invalid_request("jsonrpc version must be 2.0"),
        ));
    }
    ParsedStdioLine::Request(request)
}

/// Triage a request that arrived mid-turn.
///
/// Cancellation is the whole point of reading here, so `thread/interrupt`
/// runs immediately and only its reply waits for the writer. `shutdown` also
/// interrupts immediately — otherwise it would block on the bridge mutex the
/// turn is holding — and then queues so the turn can unwind first. Everything
/// else simply queues: it was never urgent, and running it now would race the
/// turn for the writer.
async fn handle_line_during_turn(
    state: &AppState,
    line: &str,
    pending: &mut VecDeque<PendingStdioWork>,
    policy: StdioLoopPolicy,
) {
    let request = match parse_stdio_line(line) {
        ParsedStdioLine::Blank => return,
        ParsedStdioLine::Rejected(response) => {
            pending.push_back(PendingStdioWork::Response(response));
            return;
        }
        ParsedStdioLine::Request(request) => request,
    };

    match request.method.as_str() {
        "thread/interrupt" => {
            let id = request.id.clone();
            let response = match parse_params::<ThreadInterruptParams>(params_or_object(
                request.params.clone(),
            )) {
                Ok(parsed) => match interrupt_stdio_turn(state, &parsed.thread_id).await {
                    Ok(interrupted) => jsonrpc_result(
                        id,
                        json!({ "thread_id": parsed.thread_id, "interrupted": interrupted }),
                    ),
                    Err(err) => jsonrpc_error(id, err),
                },
                Err(err) => jsonrpc_error(id, err),
            };
            pending.push_back(PendingStdioWork::Response(response));
        }
        "shutdown" if policy.shutdown == ShutdownAuthority::Denied => {
            // A non-owner may not even interrupt the live turns: that is the
            // first half of what shutdown does.
            pending.push_back(PendingStdioWork::Response(jsonrpc_error(
                request.id,
                JsonRpcError::not_daemon_owner(),
            )));
        }
        "shutdown" => {
            let live: Vec<String> = state.in_flight_turns.lock().await.keys().cloned().collect();
            for thread_id in live {
                let _ = interrupt_stdio_turn(state, &thread_id).await;
            }
            pending.push_back(PendingStdioWork::Request(request));
        }
        _ => pending.push_back(PendingStdioWork::Request(request)),
    }
}

async fn write_stdio_line<W: AsyncWrite + Unpin>(writer: &mut W, response: &Value) -> Result<()> {
    writer.write_all(&serde_json::to_vec(response)?).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

async fn healthz() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "protocol": "v2",
        "service": legacy_deepseek_compat::SERVICE_NAME
    }))
}

/// Render a routing failure as a typed HTTP error body.
///
/// Deliberately *not* a success-shaped payload with the error stuffed into a
/// content field: a client must be able to tell "the model said this" from
/// "nothing ran".
fn http_error_from_jsonrpc(err: JsonRpcError) -> (StatusCode, Json<Value>) {
    let (status, code) = match err.code {
        -32600 | -32602 => (StatusCode::BAD_REQUEST, "invalid_request"),
        THREAD_NOT_FOUND_CODE => (StatusCode::NOT_FOUND, "thread_not_found"),
        RUNTIME_UNAVAILABLE_CODE => (StatusCode::SERVICE_UNAVAILABLE, "runtime_unavailable"),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    };
    (
        status,
        Json(json!({
            "error": {
                "code": code,
                "jsonrpc_code": err.code,
                "message": err.message,
            }
        })),
    )
}

async fn thread_handler(State(state): State<AppState>, Json(req): Json<ThreadRequest>) -> Response {
    // A message is a turn, and turns belong to the runtime — not to the
    // bookkeeping `Runtime` behind the other thread operations. This mirrors
    // the interception stdio `thread/message` has always done.
    if let ThreadRequest::Message { thread_id, input } = req {
        return match run_http_thread_message(&state, thread_id, input).await {
            Ok(res) => (StatusCode::OK, Json(res)).into_response(),
            Err(err) => http_error_from_jsonrpc(err).into_response(),
        };
    }
    let mut runtime = state.runtime.write().await;
    match runtime.handle_thread(req).await {
        Ok(res) => (StatusCode::OK, Json(res)).into_response(),
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
        )
            .into_response(),
    }
}

/// `POST /prompt` — runs a genuine model turn through the runtime bridge.
///
/// Note what this handler does *not* do: it never takes the `Runtime` write
/// lock. The old implementation held it across the whole request while doing
/// no model work at all.
async fn prompt_handler(State(state): State<AppState>, Json(req): Json<PromptRequest>) -> Response {
    let mut sink = tokio::io::sink();
    match run_prompt_turn(&state, &mut sink, req).await {
        Ok(res) => (StatusCode::OK, Json(res)).into_response(),
        Err(err) => http_error_from_jsonrpc(err).into_response(),
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
    build_state_with_transport(config_path, auth_token, AppTransport::Http)
}

fn build_state_with_transport(
    config_path: Option<PathBuf>,
    auth_token: Option<String>,
    transport: AppTransport,
) -> Result<AppState> {
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
    // Stdio carries JSON-RPC on stdout: printing raw hook events there
    // corrupts the protocol stream (#5165). HTTP mode keeps the stdout
    // sink for local development visibility.
    if transport == AppTransport::Http {
        hooks.add_sink(Arc::new(StdoutHookSink));
    }
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
        runtime_bridge: Arc::new(Mutex::new(None)),
        stdio_thread_hints: Arc::new(Mutex::new(HashMap::new())),
        in_flight_turns: Arc::new(Mutex::new(HashMap::new())),
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

    /// Server error (-32000..-32099): the turn engine could not be reached,
    /// or refused to start the turn — either way nothing ran. Distinct from
    /// `internal` because the caller can retry this one once a runtime is up.
    fn runtime_unavailable(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            code: RUNTIME_UNAVAILABLE_CODE,
            message: message.clone(),
            data: Some(json!({
                "error": "runtime_unavailable",
                "detail": message,
            })),
        }
    }

    /// Server error (-32000..-32099): the named thread does not exist.
    fn thread_not_found(thread_id: &str) -> Self {
        Self {
            code: THREAD_NOT_FOUND_CODE,
            message: format!("thread not found: {thread_id}"),
            data: Some(json!({
                "error": "thread_not_found",
                "thread_id": thread_id,
            })),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
            data: None,
        }
    }

    /// Server error (-32000..-32099): the daemon-socket connection has not
    /// completed `daemon/attach`, so nothing but `healthz` is allowed yet.
    #[cfg(unix)]
    fn attach_required(method: &str) -> Self {
        Self {
            code: ATTACH_REQUIRED_CODE,
            message: format!("send daemon/attach before `{method}`"),
            data: Some(json!({
                "error": "attach_required",
                "method": method,
                "attach_method": daemon_socket::ATTACH_METHOD,
            })),
        }
    }

    /// Server error (-32000..-32099): a `claim` attach lost to a live owner.
    #[cfg(unix)]
    fn daemon_already_claimed(owner: &Value) -> Self {
        Self {
            code: DAEMON_ALREADY_CLAIMED_CODE,
            message: "daemon already claimed by another client; attach with mode=attach"
                .to_string(),
            data: Some(json!({
                "error": "daemon_already_claimed",
                "owner": owner,
            })),
        }
    }

    /// Server error (-32000..-32099): only the owner may stop the daemon.
    fn not_daemon_owner() -> Self {
        Self {
            code: NOT_DAEMON_OWNER_CODE,
            message: "only the client that claimed this daemon may shut it down".to_string(),
            data: Some(json!({ "error": "not_daemon_owner" })),
        }
    }

    /// Server error (-32000..-32099): the client's expected daemon version
    /// does not match the running binary (bundle skew).
    #[cfg(unix)]
    fn daemon_version_skew(expected: &str, actual: &str) -> Self {
        Self {
            code: DAEMON_VERSION_SKEW_CODE,
            message: format!(
                "daemon version {actual} does not match the client's expected {expected}"
            ),
            data: Some(json!({
                "error": "daemon_version_skew",
                "expected": expected,
                "actual": actual,
            })),
        }
    }

    /// Server error (-32000..-32099): `daemon/attach` after attaching.
    fn already_attached() -> Self {
        Self {
            code: ALREADY_ATTACHED_CODE,
            message: "this connection is already attached".to_string(),
            data: Some(json!({ "error": "already_attached" })),
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

/// One turn's worth of routing decisions, shared by every surface that runs
/// a turn through the bridge.
struct BridgedTurn<'a> {
    /// Client-facing thread id; the bridge maps it to a runtime thread.
    thread_key: &'a str,
    input: &'a str,
    /// Model for the runtime thread when this call is the one that creates
    /// it. An existing thread keeps the model it was created with.
    model_override: Option<String>,
    /// Publish the live turn so a concurrent `thread/interrupt` can cancel
    /// it. Only stdio has a mid-turn channel, so only stdio sets this.
    interruptible: bool,
    /// Forget the thread mapping once the turn ends. Set for one-shot
    /// prompts, whose synthetic thread key no client can name again.
    ephemeral: bool,
}

/// Execute exactly one turn on the real runtime.
///
/// This is the only way any app-server surface runs a model: `/prompt`,
/// `prompt/request`, `prompt/run`, stdio `thread/message`, and HTTP `/thread`
/// messages all land here. There is no local fallback that fabricates a
/// response — if the runtime cannot be reached the caller gets
/// [`JsonRpcError::runtime_unavailable`] and nothing is written to history.
async fn run_bridged_turn<W: AsyncWrite + Unpin>(
    state: &AppState,
    writer: &mut W,
    turn: BridgedTurn<'_>,
    transcript: Option<&mut TurnTranscript>,
) -> std::result::Result<Value, JsonRpcError> {
    let mut hint = {
        let hints = state.stdio_thread_hints.lock().await;
        hints.get(turn.thread_key).cloned()
    };
    if let Some(model) = turn.model_override {
        hint.get_or_insert_with(RuntimeThreadHint::default).model = Some(model);
    }
    let bridge = acquire_runtime_bridge(state).await?;
    // The inner bridge lock is held for the whole turn: one child process
    // serves all threads and per-thread seq tracking requires ordered
    // access. The cache slot itself stays unlocked, so config updates and
    // bridge invalidation are never queued behind a streaming turn.
    let mut bridge = bridge.lock().await;
    let runtime_thread_id = bridge
        .ensure_runtime_thread(turn.thread_key, hint)
        .await
        .map_err(|err| JsonRpcError::runtime_unavailable(err.to_string()))?;
    let registration = turn
        .interruptible
        .then(|| (state.in_flight_turns.clone(), turn.thread_key.to_string()));
    let result = bridge
        .message_thread(
            &runtime_thread_id,
            turn.input,
            writer,
            registration,
            transcript,
        )
        .await;
    if turn.ephemeral {
        // Drop the mapping while we still hold the lock, so a long-lived
        // app-server does not accumulate one entry per one-shot prompt.
        bridge.forget_thread(turn.thread_key);
    }
    result.map_err(|err| JsonRpcError::internal(err.to_string()))
}

/// Run a prompt as a genuine model turn and return what the model actually
/// said.
///
/// `writer` receives the same streaming frames stdio `thread/message` emits;
/// HTTP callers pass a sink and read the frames back out of
/// [`PromptResponse::events`].
async fn run_prompt_turn<W: AsyncWrite + Unpin>(
    state: &AppState,
    writer: &mut W,
    req: PromptRequest,
) -> std::result::Result<PromptResponse, JsonRpcError> {
    if req.prompt.trim().is_empty() {
        return Err(JsonRpcError::invalid_params("prompt must not be empty"));
    }
    // The turn engine has no threadless mode, so a prompt without a thread
    // gets a fresh one. Keying it on a uuid keeps a one-shot prompt out of
    // any caller's history and out of the way of concurrent prompts.
    let ephemeral = req.thread_id.is_none();
    let thread_key = req
        .thread_id
        .clone()
        .unwrap_or_else(|| format!("prompt-{}", Uuid::new_v4()));

    let mut transcript = TurnTranscript::default();
    run_bridged_turn(
        state,
        writer,
        BridgedTurn {
            thread_key: &thread_key,
            input: &req.prompt,
            model_override: req.model.clone(),
            // `thread/interrupt` addresses client-facing thread ids. A
            // one-shot prompt has none to hand back, and a caller-supplied
            // thread id is already interruptible through `thread/message`.
            interruptible: false,
            ephemeral,
        },
        Some(&mut transcript),
    )
    .await?;

    // Report the model the runtime actually ran, never a locally resolved
    // guess. The fallbacks only matter for a runtime that omits the field.
    let model = match transcript.model {
        Some(model) => model,
        None => match req.model {
            Some(model) => model,
            None => state
                .config
                .read()
                .await
                .model
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
        },
    };

    Ok(PromptResponse {
        output: transcript.text,
        model,
        events: transcript.events,
    })
}

async fn handle_prompt_request<W: AsyncWrite + Unpin>(
    state: &AppState,
    writer: &mut W,
    req: PromptRequest,
) -> std::result::Result<PromptResponse, JsonRpcError> {
    run_prompt_turn(state, writer, req).await
}

/// HTTP `/thread` with a `Message` body: same engine as stdio
/// `thread/message`, but the turn is collected rather than streamed because
/// this transport is request/response.
async fn run_http_thread_message(
    state: &AppState,
    thread_id: String,
    input: String,
) -> std::result::Result<ThreadResponse, JsonRpcError> {
    let mut transcript = TurnTranscript::default();
    let mut sink = tokio::io::sink();
    let result = run_bridged_turn(
        state,
        &mut sink,
        BridgedTurn {
            thread_key: &thread_id,
            input: &input,
            model_override: None,
            interruptible: false,
            ephemeral: false,
        },
        Some(&mut transcript),
    )
    .await?;

    Ok(ThreadResponse {
        thread_id,
        // The turn ran to a terminal state before this response was built,
        // which is exactly what the old `accepted` did not mean.
        status: "completed".to_string(),
        thread: None,
        threads: Vec::new(),
        goal: None,
        model: transcript.model,
        model_provider: None,
        cwd: None,
        approval_policy: None,
        sandbox: None,
        events: transcript.events,
        data: result.get("data").cloned().unwrap_or_else(|| json!({})),
    })
}

async fn handle_stdio_thread_message<W: AsyncWrite + Unpin>(
    state: &AppState,
    writer: &mut W,
    parsed: ThreadMessageParams,
) -> std::result::Result<Value, JsonRpcError> {
    let mut result = run_bridged_turn(
        state,
        writer,
        BridgedTurn {
            thread_key: &parsed.thread_id,
            input: &parsed.input,
            model_override: None,
            interruptible: true,
            ephemeral: false,
        },
        None,
    )
    .await?;
    if let Some(object) = result.as_object_mut() {
        object.insert("thread_id".to_string(), Value::String(parsed.thread_id));
    }
    Ok(result)
}

/// Resuming or forking a thread the runtime reports as `missing` must fail
/// with a named not-found error. Recording the null model/workspace of that
/// response as a stdio hint would clobber any previously cached hint for
/// the same thread id (#5171).
fn ensure_thread_found(response: &ThreadResponse) -> std::result::Result<(), JsonRpcError> {
    if response.status == "missing" {
        return Err(JsonRpcError::thread_not_found(&response.thread_id));
    }
    Ok(())
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
/// the child spawn or any request traffic — so [`invalidate_runtime_bridge`]
/// and other slot users are never blocked behind a slow bridge operation.
async fn acquire_runtime_bridge(
    state: &AppState,
) -> std::result::Result<SharedRuntimeBridge, JsonRpcError> {
    if let Some(bridge) = state.runtime_bridge.lock().await.as_ref() {
        return Ok(bridge.clone());
    }
    let bridge = Arc::new(Mutex::new(
        RuntimeBridge::start(state.config_path.as_deref())
            .await
            .map_err(|err| JsonRpcError::runtime_unavailable(err.to_string()))?,
    ));
    let mut slot = state.runtime_bridge.lock().await;
    // Prefer a bridge cached by a concurrent caller while we were spawning;
    // dropping our unused one kills the extra child via `Drop`.
    Ok(slot.get_or_insert_with(|| bridge.clone()).clone())
}

/// Ask the runtime to interrupt a turn that is streaming right now.
///
/// Everything this needs was copied out of the bridge when the turn started,
/// so it never touches the bridge mutex the turn is holding. Returns whether
/// a live turn was found for `thread_id`.
async fn interrupt_stdio_turn(
    state: &AppState,
    thread_id: &str,
) -> std::result::Result<bool, JsonRpcError> {
    let Some(turn) = state.in_flight_turns.lock().await.get(thread_id).cloned() else {
        return Ok(false);
    };
    let mut request = codewhale_release::platform_http_client_builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|err| JsonRpcError::internal(err.to_string()))?
        .post(format!(
            "{}/v1/threads/{}/turns/{}/interrupt",
            turn.base_url, turn.runtime_thread_id, turn.turn_id
        ));
    if let Some(token) = turn.auth_token.as_deref() {
        request = request.bearer_auth(token);
    }
    request
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| JsonRpcError::internal(format!("interrupt failed: {err}")))?;
    Ok(true)
}

/// Drop the cached runtime bridge so the next stdio thread message spawns a
/// fresh child that re-reads the persisted config. An in-flight message
/// keeps its own [`SharedRuntimeBridge`] clone and finishes against the old
/// child, which is killed when the last clone drops.
async fn invalidate_runtime_bridge(state: &AppState) {
    let mut bridge = state.runtime_bridge.lock().await;
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
            client: codewhale_release::platform_http_client_builder()
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
        // Pass the runtime auth token out-of-band via env (not argv) so local
        // `ps` cannot read credential material from the child command line.
        // The TUI/runtime server already accepts CODEWHALE_RUNTIME_TOKEN /
        // DEEPSEEK_RUNTIME_TOKEN when --auth-token is absent.
        command
            .arg("app-server")
            .arg("--http")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .env("CODEWHALE_RUNTIME_TOKEN", auth_token)
            .env("DEEPSEEK_RUNTIME_TOKEN", auth_token)
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

    /// Drop a thread mapping (and its seq cursor) once no caller can name
    /// the client-facing key again.
    fn forget_thread(&mut self, stdio_thread_id: &str) {
        if let Some(runtime_thread_id) = self.thread_map.remove(stdio_thread_id) {
            self.last_seq_by_thread.remove(&runtime_thread_id);
        }
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

    /// Run one turn to completion, streaming its events to `writer`.
    ///
    /// `registration` is `Some` on the stdio path: it publishes the live turn
    /// so an `thread/interrupt` arriving mid-stream can reach the runtime
    /// without waiting on the bridge mutex this call holds.
    async fn message_thread<W: AsyncWrite + Unpin>(
        &mut self,
        thread_id: &str,
        input: &str,
        writer: &mut W,
        registration: Option<(TurnRegistry, String)>,
        mut transcript: Option<&mut TurnTranscript>,
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

        if let Some(transcript) = transcript.as_deref_mut() {
            transcript.model = turn
                .pointer("/thread/model")
                .and_then(Value::as_str)
                .map(str::to_string);
            transcript.events.push(EventFrame::ResponseStart {
                response_id: response_id.clone(),
            });
        }

        emit_stdio_event(
            writer,
            json!({
                "type": "response_start",
                "response_id": response_id,
            }),
        )
        .await?;

        // Publish the turn only for the streaming window, and take it back
        // before any `?` below: a turn that has already finished must never
        // look cancellable.
        if let Some((registry, key)) = registration.as_ref() {
            registry.lock().await.insert(
                key.clone(),
                InFlightTurn {
                    base_url: self.base_url.clone(),
                    auth_token: self.auth_token.clone(),
                    runtime_thread_id: thread_id.to_string(),
                    turn_id: turn_id.clone(),
                },
            );
        }

        let since_seq = self.last_seq_by_thread.get(thread_id).copied().unwrap_or(0);
        let stream_result = self
            .stream_turn_events(
                thread_id,
                &turn_id,
                &response_id,
                writer,
                since_seq,
                transcript.as_deref_mut(),
            )
            .await;

        if let Some((registry, key)) = registration.as_ref() {
            registry.lock().await.remove(key);
        }

        let _ = emit_stdio_event(
            writer,
            json!({
                "type": "response_end",
                "response_id": response_id,
            }),
        )
        .await;
        if let Some(transcript) = transcript {
            transcript.events.push(EventFrame::ResponseEnd {
                response_id: response_id.clone(),
            });
        }

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
        mut transcript: Option<&mut TurnTranscript>,
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
                            if let Some(transcript) = transcript.as_deref_mut() {
                                transcript.text.push_str(delta);
                                transcript.events.push(EventFrame::ResponseDelta {
                                    response_id: response_id.to_string(),
                                    delta: delta.to_string(),
                                    channel: ResponseChannel::Text,
                                });
                            }
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
            client: codewhale_release::platform_http_client_builder()
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
    dispatch_stdio_request_with_writer(state, &mut sink, method, params, AppTransport::Stdio).await
}

async fn dispatch_stdio_app_request(
    state: &AppState,
    request: AppRequest,
    transport: AppTransport,
) -> std::result::Result<StdioDispatchResult, JsonRpcError> {
    let response = Box::pin(process_app_request(state, request, transport)).await;
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
    transport: AppTransport,
) -> std::result::Result<StdioDispatchResult, JsonRpcError> {
    let outcome = match method {
        "healthz" | "app/healthz" => StdioDispatchResult {
            result: json!({
                "status": "ok",
                "service": legacy_deepseek_compat::SERVICE_NAME,
                "transport": transport.label()
            }),
            should_exit: false,
        },
        "capabilities" => {
            let mut methods = vec![
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
                "thread/interrupt",
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
            if transport == AppTransport::Socket {
                // The daemon handshake exists only on the socket transport;
                // stdio/HTTP clients never see it, so the stdio pin is unchanged.
                methods.insert(1, daemon_socket::ATTACH_METHOD);
            }
            StdioDispatchResult {
                result: json!({
                    "transport": transport.label(),
                    "families": ["thread/*", "app/*", "prompt/*"],
                    "methods": methods,
                }),
                should_exit: false,
            }
        }
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
                    "thread/message",
                    "thread/interrupt"
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
            ensure_thread_found(&response)?;
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
            ensure_thread_found(&response)?;
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
        "app/capabilities" => {
            dispatch_stdio_app_request(state, AppRequest::Capabilities, transport).await?
        }
        "app/request" => {
            let request: AppRequest = parse_params(params)?;
            dispatch_stdio_app_request(state, request, transport).await?
        }
        "app/config/get" => {
            let parsed: ConfigGetParams = parse_params(params_or_object(params))?;
            dispatch_stdio_app_request(state, AppRequest::ConfigGet { key: parsed.key }, transport)
                .await?
        }
        "app/config/set" => {
            let parsed: ConfigSetParams = parse_params(params_or_object(params))?;
            dispatch_stdio_app_request(
                state,
                AppRequest::ConfigSet {
                    key: parsed.key,
                    value: parsed.value,
                },
                transport,
            )
            .await?
        }
        "app/config/unset" => {
            let parsed: ConfigGetParams = parse_params(params_or_object(params))?;
            dispatch_stdio_app_request(
                state,
                AppRequest::ConfigUnset { key: parsed.key },
                transport,
            )
            .await?
        }
        "app/config/list" => {
            dispatch_stdio_app_request(state, AppRequest::ConfigList, transport).await?
        }
        "app/config/reload" => {
            dispatch_stdio_app_request(state, AppRequest::ConfigReload, transport).await?
        }
        "app/models" => dispatch_stdio_app_request(state, AppRequest::Models, transport).await?,
        "app/thread_loaded_list" | "app/thread-loaded-list" => {
            dispatch_stdio_app_request(state, AppRequest::ThreadLoadedList, transport).await?
        }
        "prompt/capabilities" => StdioDispatchResult {
            result: json!({
                "methods": ["prompt/request", "prompt/run"]
            }),
            should_exit: false,
        },
        "prompt/request" | "prompt/run" => {
            let request: PromptRequest = parse_params(params)?;
            let response = handle_prompt_request(state, writer, request).await?;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/interrupt" => {
            let parsed: ThreadInterruptParams = parse_params(params_or_object(params))?;
            let interrupted = interrupt_stdio_turn(state, &parsed.thread_id).await?;
            StdioDispatchResult {
                result: json!({
                    "thread_id": parsed.thread_id,
                    "interrupted": interrupted,
                }),
                should_exit: false,
            }
        }
        "shutdown" => {
            // A turn streaming right now holds the bridge mutex, so taking it
            // to kill the child would block until that turn ends — the exact
            // deadlock that made shutdown useless against a runaway turn.
            // Interrupt live turns first; they release the mutex promptly.
            let live: Vec<String> = state.in_flight_turns.lock().await.keys().cloned().collect();
            for thread_id in live {
                let _ = interrupt_stdio_turn(state, &thread_id).await;
            }
            if let Some(bridge) = state.runtime_bridge.lock().await.take() {
                bridge.lock().await.shutdown_child();
            }
            StdioDispatchResult {
                result: json!({"ok": true, "status": "stopped"}),
                should_exit: true,
            }
        }
        daemon_socket::ATTACH_METHOD if transport == AppTransport::Socket => {
            return Err(JsonRpcError::already_attached());
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
            // Only propagate a mutation that actually happened. `set_value`
            // leaves the config untouched on an unknown key or invalid value,
            // so this is a no-op from the caller's point of view — but
            // `apply_config_update` invalidates the cached stdio bridge
            // regardless, and dropping the last reference kills the running
            // child runtime along with its thread map. A single typo'd key
            // would orphan every in-flight thread on that bridge.
            if ok {
                apply_config_update(state, snapshot, None, true).await;
            }
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
            // See ConfigSet: a failed unset changed nothing and must not tear
            // down the runtime bridge.
            if ok {
                apply_config_update(state, snapshot, None, true).await;
            }
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
        AppRequest::SubmitUserInput { request_id, .. } => {
            // This transport cannot deliver a clarification answer, and
            // saying otherwise was the bug: the previous implementation
            // reported `resolved: true` and filed the answers in a map with
            // no reader anywhere in this crate.
            //
            // It cannot be made to work here. `handle_line_during_turn`
            // executes exactly one method while a turn is streaming —
            // `thread/interrupt`. Everything else, `app/request` included,
            // queues until the turn ends, so an answer sent over this
            // transport would wait on the very turn that is waiting for it.
            // The runtime API owns the pending request and can resume the
            // turn, so that is where the reply belongs.
            AppResponse {
                ok: false,
                data: json!({
                    "error": "user_input_reply_unsupported",
                    "request_id": request_id,
                    "message": concat!(
                        "the app-server control transport cannot deliver clarification answers: ",
                        "only `thread/interrupt` runs while a turn is streaming, so an answer sent ",
                        "here would queue behind the turn waiting for it. Reply on the runtime API ",
                        "instead: POST /v1/user-input/{thread_id}/{request_id}."
                    ),
                }),
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
    // see `Runtime::reload_config_and_policy` for the headless boundary;
    // the TUI's explicit `/mcp reload` operation is a separate path.
    {
        let mut runtime = state.runtime.write().await;
        match exec_policy {
            Some(policy) => runtime.reload_config_and_policy(snapshot, policy),
            None => runtime.update_config(snapshot),
        }
    }
    invalidate_runtime_bridge(state).await;
}

async fn persist_config(state: &AppState, config: codewhale_config::ConfigToml) -> Result<()> {
    if state.config_path.is_none() {
        return Ok(());
    }
    let mut store = ConfigStore::load(state.config_path.clone())?;
    store.config = config;
    store.save()
}

/// Install the process-wide rustls crypto provider once for tests that build
/// an HTTP client. Production installs it at startup; each test must do the
/// same instead of relying on another test in the process having run first
/// (nextest runs every test in its own process).
#[cfg(test)]
pub(crate) fn install_test_crypto_provider() {
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INIT.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
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

    #[tokio::test]
    async fn stdio_transport_never_registers_the_stdout_hook_sink() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        fs::write(&config_path, "api_key = \"sk-deepseek-secret\"\n").expect("write config");

        let http_state =
            build_state_with_transport(Some(config_path.clone()), None, AppTransport::Http)
                .expect("http state");
        let stdio_state = build_state_with_transport(Some(config_path), None, AppTransport::Stdio)
            .expect("stdio state");

        let http_sinks = http_state.runtime.read().await.hooks.sink_count();
        let stdio_sinks = stdio_state.runtime.read().await.hooks.sink_count();
        assert_eq!(
            http_sinks,
            stdio_sinks + 1,
            "HTTP mode keeps StdoutHookSink + JsonlHookSink; stdio must drop the stdout sink (#5165)"
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

    /// A bridge stand-in with no child process: this test only cares about
    /// whether the cache slot survives, not about talking to a runtime.
    fn sentinel_bridge() -> SharedRuntimeBridge {
        Arc::new(Mutex::new(RuntimeBridge {
            base_url: "http://127.0.0.1:0".to_string(),
            client: reqwest::Client::new(),
            auth_token: None,
            child: None,
            thread_map: HashMap::from([("stdio-1".to_string(), "runtime-1".to_string())]),
            last_seq_by_thread: HashMap::new(),
        }))
    }

    #[tokio::test]
    async fn failed_config_set_keeps_the_stdio_bridge() {
        crate::install_test_crypto_provider();
        // #4737: `set_value` rejects an invalid value before assigning, so the
        // request is a no-op — but `apply_config_update` ran anyway and
        // invalidated the cached bridge, dropping the child runtime along with
        // its thread map. A single bad value orphaned every in-flight stdio
        // thread, behind a response that correctly reported `ok: false`.
        //
        // Only `set_value` is exercised: an unknown key lands in `extras` and
        // succeeds, and `unset_value` has no failing input today, so its
        // identical guard has nothing to assert against.
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        fs::write(&config_path, "model = \"deepseek-chat\"\n").expect("write config");
        let state = build_state(Some(config_path.clone()), None).expect("state");
        *state.runtime_bridge.lock().await = Some(sentinel_bridge());

        let response = process_app_request(
            &state,
            AppRequest::ConfigSet {
                key: "telemetry".to_string(),
                value: "not-a-bool".to_string(),
            },
            AppTransport::Stdio,
        )
        .await;
        assert!(!response.ok, "invalid value must fail: {response:?}");

        let slot = state.runtime_bridge.lock().await;
        let kept = slot
            .as_ref()
            .expect("bridge must survive a failed config/set");
        assert_eq!(
            kept.lock()
                .await
                .thread_map
                .get("stdio-1")
                .map(String::as_str),
            Some("runtime-1"),
            "the live thread map must be intact",
        );
    }

    #[tokio::test]
    async fn successful_config_set_still_invalidates_the_stdio_bridge() {
        crate::install_test_crypto_provider();
        // The other half of #4737: a mutation that *did* happen must still
        // rebuild the bridge, or the runtime keeps serving the old config.
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        fs::write(&config_path, "model = \"deepseek-chat\"\n").expect("write config");
        let state = build_state(Some(config_path.clone()), None).expect("state");
        *state.runtime_bridge.lock().await = Some(sentinel_bridge());

        let response = process_app_request(
            &state,
            AppRequest::ConfigSet {
                key: "model".to_string(),
                value: "deepseek-reasoner".to_string(),
            },
            AppTransport::Stdio,
        )
        .await;
        assert!(response.ok, "valid set should succeed: {response:?}");
        assert!(
            state.runtime_bridge.lock().await.is_none(),
            "a successful config change must invalidate the cached bridge",
        );
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
        *state.runtime_bridge.lock().await = Some(bridge.clone());
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
        assert!(state.runtime_bridge.lock().await.is_none());
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

        assert!(state.runtime_bridge.lock().await.is_none());
    }

    #[tokio::test]
    async fn stdio_bridge_invalidation_not_blocked_by_in_flight_turn() {
        let (state, _tmp) = capability_test_state();
        let bridge = seed_test_bridge(&state).await;

        // Simulate a long streaming turn holding the inner bridge lock.
        let _in_flight = bridge.lock().await;

        // Invalidation only touches the cache slot, so it must complete
        // without waiting for the in-flight turn to release the bridge.
        tokio::time::timeout(Duration::from_secs(1), invalidate_runtime_bridge(&state))
            .await
            .expect("invalidation must not wait on bridge traffic");
        assert!(state.runtime_bridge.lock().await.is_none());
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

    #[tokio::test]
    async fn stdio_resume_of_missing_thread_fails_without_clobbering_the_hint() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        fs::write(&config_path, "").expect("write config");
        let state = build_state(Some(config_path), None).expect("state");

        // A cached hint for a thread the runtime no longer knows: the exact
        // clobber scenario from #5171.
        let workspace = tmp.path().join("ws");
        {
            let mut hints = state.stdio_thread_hints.lock().await;
            hints.insert(
                "ghost-thread".to_string(),
                RuntimeThreadHint {
                    model: Some("deepseek-v4-pro".to_string()),
                    workspace: Some(workspace.clone()),
                },
            );
        }

        let err = dispatch_stdio_request(
            &state,
            "thread/resume",
            json!({ "thread_id": "ghost-thread" }),
        )
        .await
        .expect_err("resuming a missing thread must fail with a named not-found error");
        assert_eq!(err.code, -32004);
        assert!(err.message.contains("ghost-thread"), "{}", err.message);

        let fork_err = dispatch_stdio_request(
            &state,
            "thread/fork",
            json!({ "thread_id": "ghost-thread" }),
        )
        .await
        .expect_err("forking a missing thread must fail with a named not-found error");
        assert_eq!(fork_err.code, -32004);

        let hints = state.stdio_thread_hints.lock().await;
        let hint = hints.get("ghost-thread").expect("cached hint survives");
        assert_eq!(hint.model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(hint.workspace.as_deref(), Some(workspace.as_path()));
    }

    fn sse_frame(event: &str, payload: Value) -> String {
        format!("event: {event}\ndata: {payload}\n\n")
    }
    /// A runtime whose turn never ends on its own — only an interrupt stops
    /// it. That is the shape of the runaway turn this protects against.
    async fn spawn_uninterruptible_until_asked_runtime() -> (
        String,
        Arc<tokio::sync::Notify>,
        tokio::task::JoinHandle<()>,
    ) {
        use axum::body::Body;
        use axum::extract::Path as AxumPath;

        let interrupted = Arc::new(tokio::sync::Notify::new());

        async fn create_turn(AxumPath(_thread_id): AxumPath<String>) -> Json<Value> {
            Json(json!({ "turn": { "id": "turn_runaway" } }))
        }
        async fn create_thread() -> Json<Value> {
            Json(json!({ "id": "thr_runaway" }))
        }
        async fn interrupt(
            State(notify): State<Arc<tokio::sync::Notify>>,
            AxumPath((_thread_id, _turn_id)): AxumPath<(String, String)>,
        ) -> Json<Value> {
            notify.notify_waiters();
            Json(json!({ "ok": true }))
        }
        async fn thread_events(
            State(notify): State<Arc<tokio::sync::Notify>>,
            AxumPath(_thread_id): AxumPath<String>,
        ) -> ([(header::HeaderName, &'static str); 1], Body) {
            // Hold the event response open until something interrupts the
            // turn. Nothing else can end it, which is the point.
            notify.notified().await;
            let body = [
                sse_frame(
                    "item.delta",
                    json!({
                        "seq": 1,
                        "turn_id": "turn_runaway",
                        "payload": { "kind": "agent_message", "delta": "thinking" }
                    }),
                ),
                sse_frame(
                    "turn.completed",
                    json!({
                        "seq": 2,
                        "turn_id": "turn_runaway",
                        "payload": { "turn": { "status": "interrupted" } }
                    }),
                ),
            ]
            .concat();
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                Body::from(body),
            )
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("listener addr");
        let app = Router::new()
            .route("/v1/threads", post(create_thread))
            .route("/v1/threads/{thread_id}/turns", post(create_turn))
            .route(
                "/v1/threads/{thread_id}/turns/{turn_id}/interrupt",
                post(interrupt),
            )
            .route("/v1/threads/{thread_id}/events", get(thread_events))
            .with_state(interrupted.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve test runtime");
        });
        (format!("http://{addr}"), interrupted, server)
    }

    #[tokio::test]
    async fn interrupt_stops_a_turn_that_would_otherwise_stream_forever() {
        let (base_url, _notify, server) = spawn_uninterruptible_until_asked_runtime().await;
        let (state, _tmp) = capability_test_state();
        *state.runtime_bridge.lock().await = Some(Arc::new(Mutex::new(
            RuntimeBridge::from_base_url_for_test(base_url),
        )));

        let (client, server_side) = tokio::io::duplex(16 * 1024);
        let (client_reader, mut client_writer) = tokio::io::split(client);

        let loop_state = state.clone();
        let loop_handle = tokio::spawn(async move {
            let (rx, tx) = tokio::io::split(server_side);
            run_stdio_loop(
                &loop_state,
                BufReader::new(rx).lines(),
                tx,
                StdioLoopPolicy::process_stdio(),
                None::<()>,
            )
            .await
        });

        // Start the runaway turn.
        client_writer
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"thread/message\",\
                  \"params\":{\"thread_id\":\"thr_a\",\"input\":\"go\"}}\n",
            )
            .await
            .expect("send thread/message");

        // Wait until the turn is genuinely in flight before cancelling, so the
        // test exercises mid-stream cancellation rather than a race.
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if state.in_flight_turns.lock().await.contains_key("thr_a") {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("turn should register itself as in flight");

        // The read loop must accept this while the turn holds the bridge.
        client_writer
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"thread/interrupt\",\
                  \"params\":{\"thread_id\":\"thr_a\"}}\n",
            )
            .await
            .expect("send thread/interrupt");
        client_writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"shutdown\"}\n")
            .await
            .expect("send shutdown");

        let finished = tokio::time::timeout(Duration::from_secs(20), loop_handle)
            .await
            .expect("the loop must exit rather than hang on the runaway turn");
        finished.expect("join loop").expect("loop result");

        let mut output = String::new();
        let mut lines = BufReader::new(client_reader);
        lines
            .read_to_string(&mut output)
            .await
            .expect("read stdio output");

        let responses: Vec<Value> = output
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .collect();
        let by_id = |id: u64| {
            responses
                .iter()
                .find(|value| value["id"] == json!(id))
                .unwrap_or_else(|| panic!("no response for id {id} in {output}"))
                .clone()
        };

        // The turn ended as interrupted rather than running to completion.
        assert!(
            by_id(1)["error"].is_object(),
            "the interrupted turn should report an error, got: {}",
            by_id(1)
        );
        assert_eq!(by_id(2)["result"]["interrupted"], json!(true));
        assert_eq!(by_id(3)["result"]["status"], json!("stopped"));

        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn interrupting_an_idle_thread_is_not_an_error() {
        let (state, _tmp) = capability_test_state();
        let response = dispatch_stdio_request(
            &state,
            "thread/interrupt",
            json!({ "thread_id": "thr_nothing_running" }),
        )
        .await
        .expect("interrupt dispatch");
        assert_eq!(response.result["interrupted"], json!(false));
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
            .message_thread("thr_test", "hello", &mut writer, None, None)
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

    // ── prompt routing runs a real turn ────────────────────────────────
    //
    // `/prompt`, `prompt/request` and `prompt/run` used to return HTTP 200
    // with a stringified echo of the caller's own routing metadata, having
    // called no model at all. These stand up the in-crate stub runtime and
    // assert the response is what the model streamed — not an echo — and
    // that an unreachable runtime is an explicit typed failure.

    /// Prompts the stub runtime was actually asked to run.
    type StubPrompts = Arc<Mutex<Vec<String>>>;

    /// A minimal but honest runtime: it creates threads, starts turns, and
    /// streams `agent_message` deltas followed by `turn.completed`.
    async fn spawn_stub_runtime() -> (String, StubPrompts, tokio::task::JoinHandle<()>) {
        async fn create_thread(Json(body): Json<Value>) -> Json<Value> {
            Json(json!({
                "id": "thr_stub",
                "model": body["model"].as_str().unwrap_or("stub-model-v1"),
            }))
        }

        async fn create_turn(
            State(prompts): State<StubPrompts>,
            AxumPath(thread_id): AxumPath<String>,
            Json(body): Json<Value>,
        ) -> Json<Value> {
            prompts
                .lock()
                .await
                .push(body["prompt"].as_str().unwrap_or_default().to_string());
            Json(json!({
                "thread": { "id": thread_id, "model": "stub-model-v1" },
                "turn": { "id": "turn_stub" },
            }))
        }

        async fn thread_events(
            AxumPath(_thread_id): AxumPath<String>,
        ) -> ([(header::HeaderName, &'static str); 1], String) {
            let body = [
                sse_frame(
                    "item.delta",
                    json!({
                        "seq": 1,
                        "turn_id": "turn_stub",
                        "payload": { "kind": "agent_message", "delta": "the answer" }
                    }),
                ),
                sse_frame(
                    "item.delta",
                    json!({
                        "seq": 2,
                        "turn_id": "turn_stub",
                        "payload": { "kind": "agent_message", "delta": " is 4" }
                    }),
                ),
                sse_frame(
                    "turn.completed",
                    json!({
                        "seq": 3,
                        "turn_id": "turn_stub",
                        "payload": { "turn": { "status": "completed" } }
                    }),
                ),
            ]
            .concat();
            ([(header::CONTENT_TYPE, "text/event-stream")], body)
        }

        let prompts: StubPrompts = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stub runtime");
        let addr = listener.local_addr().expect("listener addr");
        let app = Router::new()
            .route("/v1/threads", post(create_thread))
            .route("/v1/threads/{thread_id}/turns", post(create_turn))
            .route("/v1/threads/{thread_id}/events", get(thread_events))
            .with_state(prompts.clone());
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), prompts, server)
    }

    async fn seed_bridge_at(state: &AppState, base_url: String) -> SharedRuntimeBridge {
        let bridge = Arc::new(Mutex::new(RuntimeBridge::from_base_url_for_test(base_url)));
        *state.runtime_bridge.lock().await = Some(bridge.clone());
        bridge
    }

    #[tokio::test]
    async fn prompt_request_executes_a_genuine_model_turn() {
        let (state, _tmp) = capability_test_state();
        let (base_url, prompts, server) = spawn_stub_runtime().await;
        let bridge = seed_bridge_at(&state, base_url).await;

        let (mut reader, mut writer) = tokio::io::duplex(4096);
        let dispatched = dispatch_stdio_request_with_writer(
            &state,
            &mut writer,
            "prompt/request",
            json!({ "prompt": "what is 2+2" }),
            AppTransport::Stdio,
        )
        .await
        .expect("prompt/request dispatch");
        drop(writer);

        let response: PromptResponse =
            serde_json::from_value(dispatched.result).expect("prompt response");

        // The model's words, not a restatement of the request.
        assert_eq!(response.output, "the answer is 4");
        assert!(
            !response.output.contains("what is 2+2"),
            "prompt echo leaked into the output: {}",
            response.output
        );
        assert_eq!(response.model, "stub-model-v1");
        assert_eq!(
            prompts.lock().await.as_slice(),
            ["what is 2+2".to_string()],
            "the prompt must reach the runtime's turn endpoint"
        );

        // Real streaming frames, not three canned ones.
        let deltas: Vec<String> = response
            .events
            .iter()
            .filter_map(|event| match event {
                EventFrame::ResponseDelta { delta, .. } => Some(delta.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, vec!["the answer".to_string(), " is 4".to_string()]);
        assert!(matches!(
            response.events.first(),
            Some(EventFrame::ResponseStart { .. })
        ));
        assert!(matches!(
            response.events.last(),
            Some(EventFrame::ResponseEnd { .. })
        ));

        // The stdio transport sees the same turn stream `thread/message` emits.
        let mut stdout = Vec::new();
        reader.read_to_end(&mut stdout).await.expect("read stdout");
        let stdout = String::from_utf8(stdout).expect("utf8 stdout");
        assert!(
            stdout.contains("\"type\":\"response_delta\"") && stdout.contains("the answer"),
            "stdio prompt turn must stream its deltas, got: {stdout}"
        );

        // A prompt without a thread_id must not leave a mapping behind.
        assert!(
            bridge.lock().await.thread_map.is_empty(),
            "one-shot prompt threads must not accumulate in the bridge"
        );

        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn prompt_without_a_reachable_runtime_fails_explicitly() {
        let (state, _tmp) = capability_test_state();
        // Port 9 (discard) refuses immediately: no runtime is listening.
        seed_bridge_at(&state, "http://127.0.0.1:9".to_string()).await;

        let err = dispatch_stdio_request(&state, "prompt/run", json!({ "prompt": "hello" }))
            .await
            .expect_err("a prompt with no reachable runtime must fail, not echo");
        assert_eq!(err.code, RUNTIME_UNAVAILABLE_CODE);

        let (status, Json(body)) = http_error_from_jsonrpc(err);
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["code"], "runtime_unavailable");
        assert!(
            body.get("output").is_none(),
            "a failure must not be shaped like a PromptResponse: {body}"
        );
    }

    #[tokio::test]
    async fn empty_prompt_is_rejected_before_any_runtime_work() {
        let (state, _tmp) = capability_test_state();
        let err = dispatch_stdio_request(&state, "prompt/request", json!({ "prompt": "   " }))
            .await
            .expect_err("an empty prompt must be rejected");
        assert_eq!(err.code, -32602);
        assert!(
            state.runtime_bridge.lock().await.is_none(),
            "a rejected prompt must not start a runtime"
        );
    }

    #[tokio::test]
    async fn http_thread_message_runs_the_turn_instead_of_queueing_it() {
        let (state, _tmp) = capability_test_state();
        let (base_url, prompts, server) = spawn_stub_runtime().await;
        seed_bridge_at(&state, base_url).await;

        let response = run_http_thread_message(&state, "thr_http".to_string(), "go".to_string())
            .await
            .expect("http thread message");

        assert_eq!(response.status, "completed");
        assert_eq!(response.thread_id, "thr_http");
        assert_eq!(response.data["turn_id"], "turn_stub");
        assert_eq!(prompts.lock().await.as_slice(), ["go".to_string()]);
        assert!(
            response
                .events
                .iter()
                .any(|event| matches!(event, EventFrame::ResponseDelta { .. })),
            "a completed turn must carry the deltas it streamed"
        );

        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn http_thread_message_without_a_runtime_is_a_typed_error() {
        let (state, _tmp) = capability_test_state();
        seed_bridge_at(&state, "http://127.0.0.1:9".to_string()).await;

        let err = run_http_thread_message(&state, "thr_http".to_string(), "go".to_string())
            .await
            .expect_err("no runtime means no turn");
        assert_eq!(err.code, RUNTIME_UNAVAILABLE_CODE);
    }

    #[tokio::test]
    async fn submit_user_input_refuses_instead_of_claiming_resolution() {
        let (state, _tmp) = capability_test_state();
        let response = process_app_request(
            &state,
            AppRequest::SubmitUserInput {
                request_id: "user-input-1".to_string(),
                answers: Vec::new(),
            },
            AppTransport::Stdio,
        )
        .await;

        assert!(!response.ok, "this transport cannot deliver the answer");
        assert_eq!(response.data["error"], "user_input_reply_unsupported");
        assert!(
            response.data.get("resolved").is_none(),
            "nothing was resolved: {}",
            response.data
        );
        assert!(
            response.data["message"]
                .as_str()
                .expect("message")
                .contains("/v1/user-input/"),
            "the refusal must name the transport that can accept the answer"
        );
        assert!(
            !response.data["message"]
                .as_str()
                .expect("message")
                .contains("  "),
            "the refusal must not expose source-formatting whitespace"
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
        "thread/interrupt",
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

    /// The socket transport advertises the `daemon/attach` handshake right
    /// after `healthz`; the stdio pin above must stay untouched by it.
    #[tokio::test]
    async fn socket_transport_advertises_daemon_attach() {
        let (state, _tmp) = capability_test_state();
        let mut sink = tokio::io::sink();
        let caps = dispatch_stdio_request_with_writer(
            &state,
            &mut sink,
            "capabilities",
            json!({}),
            AppTransport::Socket,
        )
        .await
        .expect("capabilities dispatch");
        assert_eq!(caps.result["transport"], json!("unix-socket"));
        let methods: Vec<String> = caps.result["methods"]
            .as_array()
            .expect("methods array")
            .iter()
            .map(|m| m.as_str().expect("method string").to_string())
            .collect();
        let mut expected: Vec<String> = EXPECTED_CAPABILITY_METHODS
            .iter()
            .map(|m| m.to_string())
            .collect();
        expected.insert(1, daemon_socket::ATTACH_METHOD.to_string());
        assert_eq!(methods, expected);
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
    fn runtime_bridge_command_keeps_auth_token_out_of_argv() {
        // FR001-C001: runtime auth token must not appear on the child argv
        // (visible via local `ps`); pass it via env instead.
        let token = "cwrt_unit_test_secret_token_not_for_argv";
        let cmd = RuntimeBridge::runtime_command(None, 18787, token).expect("command");
        let argv: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            !argv
                .iter()
                .any(|a| a.contains(token) || a == "--auth-token"),
            "auth token must not be present in child argv: {argv:?}"
        );
        let envs: Vec<(String, String)> = cmd
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect();
        assert!(
            envs.iter()
                .any(|(k, v)| k == "CODEWHALE_RUNTIME_TOKEN" && v == token),
            "token must be carried via CODEWHALE_RUNTIME_TOKEN: {envs:?}"
        );
        assert!(
            envs.iter()
                .any(|(k, v)| k == "DEEPSEEK_RUNTIME_TOKEN" && v == token),
            "legacy alias DEEPSEEK_RUNTIME_TOKEN must also carry the token: {envs:?}"
        );
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
