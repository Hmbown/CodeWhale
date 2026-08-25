//! MCP server implementation for exposing Codewhale tools over stdio.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::runtime::Runtime;
use uuid::Uuid;

use crate::client::DeepSeekClient;
use crate::config::Config;
use crate::llm_client::LlmClient;
use crate::models::Role;
use crate::models::{ContentBlock, Message, MessageRequest};
use crate::session_manager::SessionManager;
use crate::tools::spec::{ToolError, ToolResult};
use crate::tools::{ToolContext, ToolRegistryBuilder};

#[derive(Debug, Default, Deserialize)]
struct McpServerConfigFile {
    #[serde(default)]
    server: McpServerSection,
}

#[derive(Debug, Default, Deserialize)]
struct McpServerSection {
    expose_tools: Option<Vec<String>>,
    require_approval: Option<bool>,
}

#[derive(Debug, Clone)]
struct McpServerSettings {
    expose_tools: Vec<String>,
    require_approval: bool,
}

impl McpServerSettings {
    fn load() -> Result<Self> {
        let path = default_config_path();
        if let Some(path) = path.filter(|p| p.exists()) {
            let contents = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read MCP server config: {}", path.display()))?;
            let config: McpServerConfigFile = toml::from_str(&contents).with_context(|| {
                format!("Failed to parse MCP server config: {}", path.display())
            })?;
            let expose_tools = config
                .server
                .expose_tools
                .unwrap_or_else(default_expose_tools);
            let require_approval = config.server.require_approval.unwrap_or(false);
            Ok(Self {
                expose_tools,
                require_approval,
            })
        } else {
            Ok(Self {
                expose_tools: default_expose_tools(),
                require_approval: false,
            })
        }
    }
}

#[derive(Debug, Clone)]
struct ExposedTool {
    public: String,
    internal: String,
}

fn mcp_request_model(arguments: &Value, default_model: &str) -> String {
    arguments
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(default_model)
        .to_string()
}

pub fn run_mcp_server(workspace: PathBuf) -> Result<()> {
    let settings = McpServerSettings::load()?;
    let mut server = McpServer::new(workspace, settings)?;
    server.run()
}

struct McpServer {
    workspace: PathBuf,
    registry: crate::tools::ToolRegistry,
    exposed_tools: Vec<ExposedTool>,
    require_approval: bool,
    /// Thread-based conversation state for deepseek/deepseek-reply tools.
    /// Maps thread_id -> ordered list of messages in the conversation.
    threads: Arc<Mutex<HashMap<String, Vec<Message>>>>,
    /// Monotonic request counter for notification correlation.
    next_notification_id: u64,
}

impl McpServer {
    fn new(workspace: PathBuf, settings: McpServerSettings) -> Result<Self> {
        let exposed_tools = build_exposed_tools(&settings.expose_tools);
        let mut internal_names: HashSet<String> = HashSet::new();
        for tool in &exposed_tools {
            internal_names.insert(tool.internal.clone());
        }

        let mut builder = ToolRegistryBuilder::new()
            .with_file_tools()
            .with_search_tools();

        if internal_names.contains("apply_patch") {
            builder = builder.with_patch_tools();
        }
        if internal_names.contains("exec_shell") {
            builder = builder.with_shell_tools();
        }

        let context = ToolContext::new(workspace.clone());
        let registry = builder.build(context);

        Ok(Self {
            workspace,
            registry,
            exposed_tools,
            require_approval: settings.require_approval,
            threads: Arc::new(Mutex::new(HashMap::new())),
            next_notification_id: 0,
        })
    }

    fn run(&mut self) -> Result<()> {
        let runtime = Runtime::new().context("Failed to start MCP runtime")?;
        let stdin = io::stdin();
        let mut stdout = io::stdout();

        for line in stdin.lock().lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(message) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };

            if let Some(response) = self.handle_message(&runtime, message) {
                let payload = serde_json::to_string(&response)?;
                writeln!(stdout, "{payload}")?;
                stdout.flush()?;
            }
        }

        Ok(())
    }

    fn handle_message(&mut self, runtime: &Runtime, message: Value) -> Option<Value> {
        let method = message.get("method").and_then(Value::as_str)?;
        let id = message.get("id").cloned();

        match method {
            "initialize" => respond(id.as_ref(), initialize_response()),
            "tools/list" => respond(id.as_ref(), self.list_tools_response()),
            "tools/call" => {
                let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
                match self.call_tool(runtime, params, id.clone()) {
                    Ok(result) => respond(id.as_ref(), result),
                    Err(err) => respond_error(id.as_ref(), err.code, err.message),
                }
            }
            "resources/list" => respond(id.as_ref(), self.list_resources_response()),
            "ping" => respond(id.as_ref(), json!({})),
            "notifications/initialized" => None,
            _ => respond_error(id.as_ref(), -32601, format!("Method not found: {method}")),
        }
    }

    fn list_tools_response(&self) -> Value {
        let mut tools = Vec::new();
        let mut seen = HashSet::new();
        for entry in &self.exposed_tools {
            if !seen.insert(entry.public.clone()) {
                continue;
            }
            match entry.internal.as_str() {
                "deepseek" => {
                    tools.push(json!({
                        "name": "deepseek",
                        "description": "Send a prompt to Codewhale and get a response. Creates a new conversation thread.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "prompt": {
                                    "type": "string",
                                    "description": "The user prompt to send to Codewhale"
                                },
                                "model": {
                                    "type": "string",
                                    "description": "Optional model identifier. When omitted, uses the active provider's default model."
                                },
                                "cwd": {
                                    "type": "string",
                                    "description": "Optional working directory context"
                                }
                            },
                            "required": ["prompt"]
                        }
                    }));
                }
                "deepseek-reply" => {
                    tools.push(json!({
                        "name": "deepseek-reply",
                        "description": "Continue an existing conversation thread with Codewhale. Requires a thread_id from a previous deepseek call.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "thread_id": {
                                    "type": "string",
                                    "description": "Thread ID from a previous deepseek call"
                                },
                                "prompt": {
                                    "type": "string",
                                    "description": "The follow-up prompt"
                                },
                                "model": {
                                    "type": "string",
                                    "description": "Optional model override"
                                }
                            },
                            "required": ["thread_id", "prompt"]
                        }
                    }));
                }
                _ => {
                    if let Some(tool) = self.registry.get(&entry.internal) {
                        tools.push(json!({
                            "name": entry.public,
                            "description": tool.description(),
                            "inputSchema": tool.input_schema(),
                        }));
                    }
                }
            }
        }
        // MCP spec: `nextCursor` must be omitted (or be a string) when there
        // are no more results. Emitting `null` violates the spec and breaks
        // strict clients (e.g. Claude Code) that validate the response shape.
        json!({ "tools": tools })
    }

    fn list_resources_response(&self) -> Value {
        let mut resources = Vec::new();
        resources.push(json!({
            "uri": format!("file://{}", self.workspace.display()),
            "name": "workspace",
            "description": "Workspace root",
            "mimeType": "inode/directory",
        }));

        if let Ok(manager) = SessionManager::default_location()
            && let Ok(sessions) = manager.list_sessions()
        {
            for session in sessions {
                resources.push(json!({
                    "uri": format!("deepseek://session/{}", session.id),
                    "name": session.title,
                    "description": format!("{} messages", session.message_count),
                    "mimeType": "application/json",
                }));
            }
        }

        // Same spec point as `list_tools_response`: omit `nextCursor` when
        // there are no further pages rather than emitting `null`.
        json!({ "resources": resources })
    }

    fn call_tool(
        &mut self,
        runtime: &Runtime,
        params: Value,
        request_id: Option<Value>,
    ) -> Result<Value, RpcError> {
        let params = params.as_object().ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params for tools/call".to_string(),
        })?;
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Missing tool name".to_string(),
            })?;

        if self.require_approval
            && !params
                .get("approved")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            return Err(RpcError {
                code: -32001,
                message: "Approval required. Resend with approved=true.".to_string(),
            });
        }

        let internal = self
            .exposed_tools
            .iter()
            .find(|tool| tool.public == name)
            .map(|tool| tool.internal.clone())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: format!("Tool not exposed: {name}"),
            })?;

        // Handle deepseek and deepseek-reply natively
        if internal == "deepseek" || internal == "deepseek-reply" {
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            return self.handle_deepseek_call(runtime, &internal, &arguments, request_id);
        }

        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let result = runtime.block_on(self.registry.execute_full(&internal, arguments));
        Ok(tool_result_to_mcp(result))
    }

    /// Handle a `deepseek` or `deepseek-reply` tool call.
    ///
    /// Uses `DeepSeekClient` directly (not the full engine) to send a prompt
    /// and return the response. For `deepseek` a new thread is created; for
    /// `deepseek-reply` the caller supplies a `thread_id` to continue an
    /// existing conversation.
    fn handle_deepseek_call(
        &mut self,
        runtime: &Runtime,
        internal_name: &str,
        arguments: &Value,
        request_id: Option<Value>,
    ) -> Result<Value, RpcError> {
        let prompt = arguments
            .get("prompt")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Missing required argument: prompt".to_string(),
            })?;

        // Load config first so an omitted model follows the active provider
        // default instead of a hardcoded DeepSeek id.
        let config = Config::load(None, None).map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to load config: {e}"),
        })?;
        let default_model = config.default_model();
        let model = mcp_request_model(arguments, &default_model);

        // Resolve thread_id
        let thread_id = if internal_name == "deepseek" {
            // New thread
            Uuid::new_v4().to_string()
        } else {
            arguments
                .get("thread_id")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError {
                    code: -32602,
                    message: "Missing required argument: thread_id for deepseek-reply".to_string(),
                })?
                .to_string()
        };

        let client = DeepSeekClient::new(&config).map_err(model_client_init_error)?;

        // Build message list
        let user_message = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: prompt.to_string(),
                cache_control: None,
            }],
        };

        let messages = if internal_name == "deepseek" {
            vec![user_message]
        } else {
            let thread = self.threads.lock().unwrap_or_else(|e| e.into_inner());
            let mut existing = thread.get(&thread_id).cloned().ok_or_else(|| RpcError {
                code: -32602,
                message: format!("Thread not found: {thread_id}"),
            })?;
            existing.push(user_message);
            existing
        };

        // Send the API request (non-streaming for the basic version). Internal
        // chat uses the same resolved output policy as an ordinary turn.
        let request_route = client.effective_route_envelope(&model, chrono::Utc::now());
        let request = MessageRequest {
            model: model.clone(),
            messages: messages.clone(),
            max_tokens: client.effective_max_output_tokens(&request_route.model),
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: None,
            temperature: None,
            top_p: None,
        };

        let response = runtime
            .block_on(client.create_message(request))
            .map_err(model_provider_call_error)?;

        // A provider-declared incomplete reply must not enter the stored
        // thread or be returned as a successful answer. The billed usage is
        // still reported in the error payload.
        if crate::models::is_incomplete_stop_reason(response.stop_reason.as_deref()) {
            let error = format!(
                "Model response incomplete: provider stop reason `{}`; the partial reply was not accepted.",
                crate::models::stop_reason_detail(response.stop_reason.as_deref())
            );
            return Ok(json!({
                "content": [{ "type": "text", "text": &error }],
                "isError": true,
                "structuredContent": {
                    "threadId": thread_id,
                    "error": error,
                    "usage": {
                        "inputTokens": response.usage.input_tokens,
                        "outputTokens": response.usage.output_tokens,
                    }
                }
            }));
        }

        // Extract response text from content blocks
        let response_text = response
            .content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::Text { text, .. } = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");

        let usage = &response.usage;

        // Store the assistant response in the thread
        {
            let mut thread = self.threads.lock().unwrap_or_else(|e| e.into_inner());
            let convo = thread.entry(thread_id.clone()).or_default();
            // If deepseek, we already have just the user message; if deepseek-reply,
            // the user message was appended to the cloned messages above but we need
            // to also append it to the stored thread and then the assistant response.
            if internal_name == "deepseek" {
                convo.push(Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: prompt.to_string(),
                        cache_control: None,
                    }],
                });
            }
            convo.push(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: response_text.clone(),
                    cache_control: None,
                }],
            });
        }

        // Emit a notification/message so the client can correlate the response
        let notification_id = {
            let nid = self.next_notification_id;
            self.next_notification_id += 1;
            nid
        };

        // Write notification to stdout
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/message",
            "params": {
                "notificationId": notification_id,
                "requestId": request_id,
                "threadId": thread_id,
                "content": response_text,
                "usage": {
                    "inputTokens": usage.input_tokens,
                    "outputTokens": usage.output_tokens,
                }
            }
        });
        if let Ok(payload) = serde_json::to_string(&notification) {
            let mut stdout = io::stdout();
            let _ = writeln!(stdout, "{payload}");
            let _ = stdout.flush();
        }

        Ok(json!({
            "content": [{ "type": "text", "text": &response_text }],
            "isError": false,
            "structuredContent": {
                "threadId": thread_id,
                "content": response_text,
                "usage": {
                    "inputTokens": usage.input_tokens,
                    "outputTokens": usage.output_tokens,
                }
            }
        }))
    }
}

fn default_config_path() -> Option<PathBuf> {
    crate::config::effective_home_dir().map(|home| home.join(".deepseek").join("mcp_server.toml"))
}

fn default_expose_tools() -> Vec<String> {
    vec![
        "file_read".to_string(),
        "file_write".to_string(),
        "search".to_string(),
        "apply_patch".to_string(),
        "shell".to_string(),
        "deepseek".to_string(),
        "deepseek-reply".to_string(),
    ]
}

fn build_exposed_tools(names: &[String]) -> Vec<ExposedTool> {
    let mut tools = Vec::new();
    for name in names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }
        let public = trimmed.to_string();
        let internal = match trimmed {
            "file_read" => "read_file",
            "file_write" => "write_file",
            "file_edit" => "edit_file",
            "shell" => "exec_shell",
            "search" => "grep_files",
            "file_search" => "file_search",
            // deepseek and deepseek-reply are handled natively in call_tool
            "deepseek" | "deepseek-reply" => trimmed,
            other => other,
        }
        .to_string();
        tools.push(ExposedTool { public, internal });
    }
    tools
}

fn tool_result_to_mcp(result: Result<ToolResult, ToolError>) -> Value {
    match result {
        Ok(tool_result) => {
            let mut response = json!({
                "content": [{ "type": "text", "text": tool_result.content }],
                "isError": !tool_result.success,
            });
            if let Some(metadata) = tool_result.metadata {
                response["structuredContent"] = metadata;
            }
            response
        }
        Err(err) => json!({
            "content": [{ "type": "text", "text": err.to_string() }],
            "isError": true,
        }),
    }
}

fn initialize_response() -> Value {
    json!({
        "protocolVersion": "2024-11-05",
        "serverInfo": {
            "name": "codewhale-mcp-server",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "capabilities": {
            "tools": {},
            "resources": {},
        }
    })
}

fn respond(id: Option<&Value>, result: Value) -> Option<Value> {
    id.map(|id| json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn respond_error(id: Option<&Value>, code: i64, message: String) -> Option<Value> {
    id.map(|id| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        })
    })
}

#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
}

fn model_client_init_error(error: impl std::fmt::Display) -> RpcError {
    RpcError {
        code: -32000,
        message: format!("Failed to create Codewhale model client: {error}"),
    }
}

fn model_provider_call_error(error: impl std::fmt::Display) -> RpcError {
    RpcError {
        code: -32000,
        message: format!("Model provider call failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn exposed_tools_map_aliases() {
        let names = vec![
            "file_read".to_string(),
            "file_write".to_string(),
            "search".to_string(),
            "apply_patch".to_string(),
            "shell".to_string(),
        ];
        let tools = build_exposed_tools(&names);
        let mut map = HashMap::new();
        for tool in tools {
            map.insert(tool.public, tool.internal);
        }
        assert_eq!(map.get("file_read").map(String::as_str), Some("read_file"));
        assert_eq!(
            map.get("file_write").map(String::as_str),
            Some("write_file")
        );
        assert_eq!(map.get("search").map(String::as_str), Some("grep_files"));
        assert_eq!(
            map.get("apply_patch").map(String::as_str),
            Some("apply_patch")
        );
        assert_eq!(map.get("shell").map(String::as_str), Some("exec_shell"));
    }

    #[test]
    fn list_responses_omit_null_next_cursor() {
        // MCP spec: `nextCursor` must be omitted (or be a string) when there
        // are no further pages. Emitting `null` breaks strict clients such as
        // Claude Code, which validate the response shape.
        let settings = McpServerSettings {
            expose_tools: vec!["deepseek".to_string(), "apply_patch".to_string()],
            require_approval: false,
        };
        let server = McpServer::new(PathBuf::from("."), settings).expect("build server");

        let tools_value = server.list_tools_response();
        let tools = tools_value
            .as_object()
            .expect("tools/list response is an object");
        assert!(tools.contains_key("tools"));
        assert!(
            tools.get("nextCursor").is_none(),
            "tools/list must omit nextCursor when there are no more pages"
        );

        let resources_value = server.list_resources_response();
        let resources = resources_value
            .as_object()
            .expect("resources/list response is an object");
        assert!(resources.contains_key("resources"));
        assert!(
            resources.get("nextCursor").is_none(),
            "resources/list must omit nextCursor when there are no more pages"
        );
    }

    #[test]
    fn initialize_uses_standard_mcp_shape_and_codewhale_identity() {
        let response = initialize_response();
        assert_eq!(response["protocolVersion"], "2024-11-05");
        assert_eq!(response["serverInfo"]["name"], "codewhale-mcp-server");
        assert_eq!(response["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        assert!(response["capabilities"]["tools"].is_object());
    }

    #[test]
    fn model_failures_use_codewhale_provider_neutral_language() {
        let init = model_client_init_error("missing credential");
        assert_eq!(init.code, -32000);
        assert_eq!(
            init.message,
            "Failed to create Codewhale model client: missing credential"
        );

        let request = model_provider_call_error("route unavailable");
        assert_eq!(request.code, -32000);
        assert_eq!(
            request.message,
            "Model provider call failed: route unavailable"
        );

        assert!(!init.message.contains("DeepSeek"));
        assert!(!request.message.contains("DeepSeek"));
    }

    #[test]
    fn omitted_mcp_model_follows_the_active_provider_default() {
        assert_eq!(mcp_request_model(&json!({}), "gpt-5.6"), "gpt-5.6");
        assert_eq!(
            mcp_request_model(&json!({ "model": "   " }), "GLM-5.3"),
            "GLM-5.3"
        );
        assert_eq!(
            mcp_request_model(&json!({ "model": "kimi-k2.5" }), "gpt-5.6"),
            "kimi-k2.5"
        );
    }

    #[test]
    fn list_tools_does_not_hardcode_a_deepseek_default_model() {
        let settings = McpServerSettings {
            expose_tools: vec!["deepseek".to_string()],
            require_approval: false,
        };
        let server = McpServer::new(PathBuf::from("."), settings).expect("build server");
        let tools = server.list_tools_response();
        let description = tools["tools"][0]["inputSchema"]["properties"]["model"]["description"]
            .as_str()
            .expect("model description");
        assert!(
            !description.to_ascii_lowercase().contains("deepseek"),
            "{description}"
        );
        assert!(description.contains("active provider"), "{description}");
    }
}
