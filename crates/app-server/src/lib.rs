use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, bail};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use codewhale_agent::ModelRegistry;
use codewhale_config::{CliRuntimeOverrides, ConfigStore};
use codewhale_core::Runtime;
use codewhale_hooks::{HookDispatcher, JsonlHookSink, StdoutHookSink};
use codewhale_mcp::McpManager;
use codewhale_protocol::{
    AppRequest, AppResponse, PromptRequest, PromptResponse, ThreadRequest, ThreadResponse,
};
use codewhale_state::StateStore;
use codewhale_tools::{ToolCall, ToolRegistry};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, RwLock};
use tower_http::cors::CorsLayer;
use uuid::Uuid;

const DEFAULT_CORS_ORIGINS: &[&str] = &[
    "http://localhost",
    "http://localhost:1420",
    "http://localhost:3000",
    "http://localhost:5173",
    "http://127.0.0.1",
    "http://127.0.0.1:1420",
    "tauri://localhost",
];

#[derive(Debug, Clone)]
pub struct AppServerOptions {
    pub listen: SocketAddr,
    pub config_path: Option<PathBuf>,
    pub auth_token: Option<String>,
    pub insecure_no_auth: bool,
    pub cors_origins: Vec<String>,
}

#[derive(Clone)]
struct AppState {
    config_path: Option<PathBuf>,
    config: Arc<RwLock<codewhale_config::ConfigToml>>,
    runtime: Arc<Mutex<Runtime>>,
    registry: ModelRegistry,
    auth_token: Option<String>,
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
    axum::serve(listener, app).await?;
    Ok(())
}

fn app_router(state: AppState, cors_origins: &[String]) -> Router {
    let protected_routes = Router::new()
        .route("/thread", post(thread_handler))
        .route("/app", post(app_handler))
        .route("/prompt", post(prompt_handler))
        .route("/tool", post(tool_handler))
        .route("/jobs", get(jobs_handler))
        .route("/mcp/startup", post(mcp_startup_handler))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_app_server_token,
        ));

    Router::new()
        .route("/healthz", get(healthz))
        .merge(protected_routes)
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
                writer.write_all(response.to_string().as_bytes()).await?;
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
            writer.write_all(response.to_string().as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
            continue;
        }

        let response = match dispatch_stdio_request(&state, &request.method, request.params).await {
            Ok(dispatch) => {
                let encoded = jsonrpc_result(request.id, dispatch.result);
                writer.write_all(encoded.to_string().as_bytes()).await?;
                writer.write_all(b"\n").await?;
                writer.flush().await?;
                if dispatch.should_exit {
                    break;
                }
                continue;
            }
            Err(err) => jsonrpc_error(request.id, err),
        };

        writer.write_all(response.to_string().as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    Ok(())
}

async fn healthz() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "protocol": "v2",
        "service": "deepseek-app-server"
    }))
}

async fn thread_handler(
    State(state): State<AppState>,
    Json(req): Json<ThreadRequest>,
) -> Json<ThreadResponse> {
    let mut runtime = state.runtime.lock().await;
    match runtime.handle_thread(req).await {
        Ok(res) => Json(res),
        Err(err) => Json(ThreadResponse {
            thread_id: "error".to_string(),
            status: format!("error:{err}"),
            thread: None,
            threads: Vec::new(),
            model: None,
            model_provider: None,
            cwd: None,
            approval_policy: None,
            sandbox: None,
            events: Vec::new(),
            data: json!({}),
        }),
    }
}

async fn prompt_handler(
    State(state): State<AppState>,
    Json(req): Json<PromptRequest>,
) -> Json<PromptResponse> {
    let mut runtime = state.runtime.lock().await;
    let overrides = CliRuntimeOverrides::default();
    match runtime.handle_prompt(req, &overrides).await {
        Ok(res) => Json(res),
        Err(err) => Json(PromptResponse {
            output: err.to_string(),
            model: "unknown".to_string(),
            events: Vec::new(),
        }),
    }
}

async fn tool_handler(
    State(state): State<AppState>,
    Json(req): Json<ToolCallRequest>,
) -> Json<Value> {
    let runtime = state.runtime.lock().await;
    let cwd = req
        .cwd
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    match runtime
        .invoke_tool(
            req.call,
            codewhale_execpolicy::AskForApproval::OnRequest,
            &cwd,
        )
        .await
    {
        Ok(value) => Json(value),
        Err(err) => Json(json!({ "ok": false, "error": err.to_string() })),
    }
}

async fn jobs_handler(State(state): State<AppState>) -> Json<AppResponse> {
    let runtime = state.runtime.lock().await;
    Json(runtime.app_status())
}

async fn mcp_startup_handler(State(state): State<AppState>) -> Json<Value> {
    let runtime = state.runtime.lock().await;
    let summary = runtime.mcp_startup().await;
    Json(json!({
        "ok": true,
        "summary": summary
    }))
}

async fn app_handler(
    State(state): State<AppState>,
    Json(req): Json<AppRequest>,
) -> Json<AppResponse> {
    Json(process_app_request(&state, req, AppTransport::Http).await)
}

fn build_state(config_path: Option<PathBuf>, auth_token: Option<String>) -> Result<AppState> {
    let store = ConfigStore::load(config_path.clone())?;
    let config = store.config.clone();
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
        .unwrap_or_else(|| PathBuf::from(".deepseek/events.jsonl"));
    hooks.add_sink(Arc::new(JsonlHookSink::new(hook_log_path)));

    let runtime = Runtime::new(
        config.clone(),
        registry.clone(),
        state_store,
        Arc::new(ToolRegistry::default()),
        Arc::new(McpManager::default()),
        config.exec_policy_engine(),
        hooks,
    );

    Ok(AppState {
        config_path,
        config: Arc::new(RwLock::new(config)),
        runtime: Arc::new(Mutex::new(runtime)),
        registry,
        auth_token,
    })
}

fn resolve_auth_token(options: &AppServerOptions) -> Result<Option<String>> {
    let configured = options.auth_token.as_ref().map(|token| token.trim());
    if let Some(token) = configured
        && token.is_empty()
    {
        bail!("app-server auth token cannot be empty");
    }

    if options.insecure_no_auth {
        if !options.listen.ip().is_loopback() {
            bail!("refusing unauthenticated app-server bind on non-loopback address");
        }
        eprintln!("warning: app-server HTTP auth disabled by --insecure-no-auth");
        return Ok(None);
    }

    let token = configured
        .map(str::to_string)
        .unwrap_or_else(|| format!("cwapp_{}", Uuid::new_v4().simple()));
    if options.auth_token.is_some() {
        eprintln!("app-server auth: bearer token required for HTTP routes.");
    } else {
        eprintln!("app-server auth: generated bearer token for this process.");
        eprintln!("  Authorization: Bearer {token}");
        eprintln!("  Pass --auth-token or set CODEWHALE_APP_SERVER_TOKEN for a stable token.");
    }
    Ok(Some(token))
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
        .is_some_and(|token| token == expected);

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
    let mut runtime = state.runtime.lock().await;
    runtime
        .handle_thread(req)
        .await
        .map_err(|err| JsonRpcError::internal(err.to_string()))
}

async fn handle_prompt_request(
    state: &AppState,
    req: PromptRequest,
) -> std::result::Result<PromptResponse, JsonRpcError> {
    let mut runtime = state.runtime.lock().await;
    runtime
        .handle_prompt(req, &CliRuntimeOverrides::default())
        .await
        .map_err(|err| JsonRpcError::internal(err.to_string()))
}

async fn dispatch_stdio_request(
    state: &AppState,
    method: &str,
    params: Value,
) -> std::result::Result<StdioDispatchResult, JsonRpcError> {
    let outcome = match method {
        "healthz" | "app/healthz" => StdioDispatchResult {
            result: json!({
                "status": "ok",
                "service": "deepseek-app-server",
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
                    "thread/archive",
                    "thread/unarchive",
                    "thread/message",
                    "app/capabilities",
                    "app/request",
                    "app/config/get",
                    "app/config/set",
                    "app/config/unset",
                    "app/config/list",
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
                    "thread/archive",
                    "thread/unarchive",
                    "thread/message"
                ]
            }),
            should_exit: false,
        },
        "thread/request" => {
            let request: ThreadRequest = parse_params(params)?;
            let response = handle_thread_request(state, request).await?;
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
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/start" => {
            let request = ThreadRequest::Start(parse_params(params_or_object(params))?);
            let response = handle_thread_request(state, request).await?;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/resume" => {
            let request = ThreadRequest::Resume(parse_params(params_or_object(params))?);
            let response = handle_thread_request(state, request).await?;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "thread/fork" => {
            let request = ThreadRequest::Fork(parse_params(params_or_object(params))?);
            let response = handle_thread_request(state, request).await?;
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
            let response = handle_thread_request(
                state,
                ThreadRequest::Message {
                    thread_id: parsed.thread_id,
                    input: parsed.input,
                },
            )
            .await?;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "app/capabilities" => {
            let response =
                process_app_request(state, AppRequest::Capabilities, AppTransport::Stdio).await;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "app/request" => {
            let request: AppRequest = parse_params(params)?;
            let response = process_app_request(state, request, AppTransport::Stdio).await;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "app/config/get" => {
            let parsed: ConfigGetParams = parse_params(params_or_object(params))?;
            let response = process_app_request(
                state,
                AppRequest::ConfigGet { key: parsed.key },
                AppTransport::Stdio,
            )
            .await;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "app/config/set" => {
            let parsed: ConfigSetParams = parse_params(params_or_object(params))?;
            let response = process_app_request(
                state,
                AppRequest::ConfigSet {
                    key: parsed.key,
                    value: parsed.value,
                },
                AppTransport::Stdio,
            )
            .await;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "app/config/unset" => {
            let parsed: ConfigGetParams = parse_params(params_or_object(params))?;
            let response = process_app_request(
                state,
                AppRequest::ConfigUnset { key: parsed.key },
                AppTransport::Stdio,
            )
            .await;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "app/config/list" => {
            let response =
                process_app_request(state, AppRequest::ConfigList, AppTransport::Stdio).await;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "app/models" => {
            let response =
                process_app_request(state, AppRequest::Models, AppTransport::Stdio).await;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
        }
        "app/thread_loaded_list" | "app/thread-loaded-list" => {
            let response =
                process_app_request(state, AppRequest::ThreadLoadedList, AppTransport::Stdio).await;
            StdioDispatchResult {
                result: serde_json::to_value(response)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?,
                should_exit: false,
            }
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
        "shutdown" => StdioDispatchResult {
            result: json!({"ok": true, "status": "stopped"}),
            should_exit: true,
        },
        _ => return Err(JsonRpcError::method_not_found(method)),
    };
    Ok(outcome)
}

async fn process_app_request(
    state: &AppState,
    req: AppRequest,
    transport: AppTransport,
) -> AppResponse {
    match req {
        AppRequest::Capabilities => AppResponse {
            ok: true,
            data: json!({
                "routes": ["/thread", "/app", "/prompt", "/tool", "/jobs", "/mcp/startup"],
                "config": ["get", "set", "unset", "list"],
                "events": ["response_start", "response_delta", "response_end", "tool_call_start", "tool_call_result", "mcp_startup_update", "mcp_startup_complete"],
                "transport": "stdio+http",
                "config_path": state.config_path.as_ref().map(|p| p.display().to_string()),
            }),
            events: Vec::new(),
        },
        AppRequest::ConfigGet { key } => {
            let cfg = state.config.read().await;
            let value = match transport {
                AppTransport::Http => cfg.get_display_value(&key),
                AppTransport::Stdio => cfg.get_value(&key),
            };
            AppResponse {
                ok: true,
                data: json!({ "key": key, "value": value }),
                events: Vec::new(),
            }
        }
        AppRequest::ConfigSet { key, value } => {
            let mut cfg = state.config.write().await;
            let result = cfg.set_value(&key, &value);
            let ok = result.is_ok();
            let message = result.err().map(|e| e.to_string());
            let snapshot = cfg.clone();
            drop(cfg);
            let _ = persist_config(state, snapshot).await;
            AppResponse {
                ok,
                data: json!({ "key": key, "value": value, "error": message }),
                events: Vec::new(),
            }
        }
        AppRequest::ConfigUnset { key } => {
            let mut cfg = state.config.write().await;
            let result = cfg.unset_value(&key);
            let ok = result.is_ok();
            let message = result.err().map(|e| e.to_string());
            let snapshot = cfg.clone();
            drop(cfg);
            let _ = persist_config(state, snapshot).await;
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
        AppRequest::Models => AppResponse {
            ok: true,
            data: json!({ "models": state.registry.list() }),
            events: Vec::new(),
        },
        AppRequest::ThreadLoadedList => {
            let mut runtime = state.runtime.lock().await;
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
    }
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
    use codewhale_protocol::AppRequest;
    use std::fs;
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

    #[test]
    fn non_loopback_bind_without_auth_fails_fast() {
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

    #[tokio::test]
    async fn stdio_transport_keeps_raw_config_get_for_legacy_clients() {
        let state = build_state(None, None).expect("state");
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

        assert_eq!(response.data["value"], "sk-deepseek-secret");
    }
}
