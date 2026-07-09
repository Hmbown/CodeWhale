//! Minimal Agent Client Protocol stdio adapter.
//!
//! This intentionally starts with the ACP baseline: initialize, new session,
//! prompt, and cancel. It keeps stdout protocol-clean for editor clients and
//! routes prompts through the same configured DeepSeek client as one-shot CLI
//! mode.
//!
//! `session/prompt` streams the provider response: each text delta is emitted
//! as a `session/update` agent_message_chunk as it arrives, instead of buffering
//! the whole turn and sending one chunk at the end. The stream is consumed
//! concurrently with the input reader so that a `session/cancel` for the same
//! session can interrupt the turn mid-stream (returning `stopReason: "cancelled"`)
//! instead of being queued behind it. A single writer task is preserved so
//! stdout stays protocol-clean.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, Lines};

use crate::client::DeepSeekClient;
use crate::config::{ApiProvider, Config};
use crate::llm_client::{LlmClient, StreamEventBox};
use crate::models::{
    ContentBlock, ContentBlockStart, Delta, Message, MessageRequest, StreamEvent, SystemPrompt,
};

const ACP_PROTOCOL_VERSION: u64 = 1;

pub async fn run_acp_server(config: Config, model: String, default_cwd: PathBuf) -> Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();
    let mut writer = tokio::io::BufWriter::new(stdout);
    let mut server = AcpServer::new(config, model, default_cwd);

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let message: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(err) => {
                write_jsonrpc_error(&mut writer, None, -32700, format!("invalid json: {err}"))
                    .await?;
                continue;
            }
        };

        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            write_jsonrpc_error(
                &mut writer,
                message.get("id").cloned(),
                -32600,
                "jsonrpc version must be 2.0",
            )
            .await?;
            continue;
        }

        let id = message.get("id").cloned();
        let method = match message.get("method").and_then(Value::as_str) {
            Some(method) => method,
            None => {
                write_jsonrpc_error(&mut writer, id, -32600, "missing method").await?;
                continue;
            }
        };
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

        // `session/prompt` is driven concurrently with the reader so a
        // `session/cancel` can interrupt the in-flight provider call. Every
        // other method is request/response and handled synchronously below.
        if method == "session/prompt" {
            match server.begin_prompt(params) {
                Ok(prepared) => {
                    let PreparedPrompt {
                        session_id,
                        messages,
                        cwd,
                    } = prepared;
                    // Opening the stream borrows `&server` only briefly; the
                    // returned `StreamEventBox` is `'static`, so it can be raced
                    // against the reader without holding a borrow on the server,
                    // and the main task keeps exclusive ownership of stdout.
                    match server.open_prompt_stream(&messages, &cwd).await {
                        Ok(stream) => {
                            let outcome =
                                drive_prompt_stream(stream, &session_id, &mut reader, &mut writer)
                                    .await;
                            match outcome {
                                Ok(PromptOutcome::Completed(output)) => {
                                    // Chunks were already streamed; record the full
                                    // assistant turn in history for the next prompt.
                                    server.finish_prompt(&session_id, &output);
                                    if let Some(id) = id {
                                        write_jsonrpc_result(
                                            &mut writer,
                                            id,
                                            json!({ "stopReason": "end_turn" }),
                                        )
                                        .await?;
                                    }
                                }
                                Ok(PromptOutcome::Cancelled) => {
                                    if let Some(id) = id {
                                        write_jsonrpc_result(
                                            &mut writer,
                                            id,
                                            json!({ "stopReason": "cancelled" }),
                                        )
                                        .await?;
                                    }
                                }
                                Err(err) => {
                                    write_jsonrpc_error(&mut writer, id, -32603, err.to_string())
                                        .await?;
                                }
                            }
                        }
                        Err(err) => {
                            write_jsonrpc_error(&mut writer, id, -32603, err.to_string()).await?;
                        }
                    }
                }
                Err(err) => {
                    write_jsonrpc_error(&mut writer, id, err.code, err.message).await?;
                }
            }
            continue;
        }

        match server.handle_request(method, params).await {
            Ok(AcpDispatch::Response(result)) => {
                if let Some(id) = id {
                    write_jsonrpc_result(&mut writer, id, result).await?;
                }
            }
            Ok(AcpDispatch::Shutdown) => {
                if let Some(id) = id {
                    write_jsonrpc_result(&mut writer, id, json!(null)).await?;
                }
                break;
            }
            Err(err) => {
                write_jsonrpc_error(&mut writer, id, err.code, err.message).await?;
            }
        }
    }

    Ok(())
}

/// Outcome of a `session/prompt` turn driven against the input stream.
#[derive(Debug, PartialEq, Eq)]
enum PromptOutcome {
    /// The provider call finished first; carries the assistant text.
    Completed(String),
    /// A matching `session/cancel` arrived before the call finished.
    Cancelled,
}

/// The text payload an ACP client should see for a given stream event, if any.
/// ACP baseline is text-only, so thinking/tool/control events carry no chunk.
fn stream_text_chunk(event: &StreamEvent) -> Option<&str> {
    match event {
        StreamEvent::ContentBlockDelta {
            delta: Delta::TextDelta { text },
            ..
        } => Some(text),
        StreamEvent::ContentBlockStart {
            content_block: ContentBlockStart::Text { text },
            ..
        } => Some(text),
        _ => None,
    }
}

/// Consume a provider response `stream`, emitting each text delta as a
/// `session/update` chunk, while concurrently watching `reader` for a
/// `session/cancel` targeting `session_id`.
///
/// This is the streaming + cancellation control point. It is generic over the
/// reader/writer and takes the boxed stream, so it is unit-tested with canned
/// in-memory streams and readers — no real provider call required. The caller
/// keeps the only writer, so streamed chunks and acknowledgements all stay on
/// the single protocol-clean stdout stream.
///
/// Returns [`PromptOutcome::Completed`] with the full accumulated text once the
/// stream ends (or emits `message_stop`), so the caller can record the turn in
/// history. A matching `session/cancel` (request or notification form) ends it
/// early with [`PromptOutcome::Cancelled`] — dropping the stream aborts the
/// underlying provider connection. The turn is single-flight: a cancel for a
/// different session is acknowledged and ignored; any other concurrent *request*
/// is rejected with a clear error so the client is not left waiting;
/// notifications without an id are ignored.
async fn drive_prompt_stream<R, W>(
    mut stream: StreamEventBox,
    session_id: &str,
    reader: &mut Lines<R>,
    writer: &mut W,
) -> Result<PromptOutcome>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut accumulated = String::new();
    // Once input closes mid-turn we stop selecting on the reader and just drain
    // the stream to completion, rather than spinning on repeated EOFs.
    let mut reader_open = true;
    loop {
        tokio::select! {
            event = stream.next() => {
                match event {
                    // Stream exhausted without an explicit stop: turn is done.
                    None => return Ok(PromptOutcome::Completed(accumulated)),
                    Some(Ok(event)) => {
                        if let Some(text) = stream_text_chunk(&event)
                            && !text.is_empty() {
                                accumulated.push_str(text);
                                write_session_update(writer, session_id, text.to_string()).await?;
                            }
                        match event {
                            StreamEvent::MessageStop => {
                                return Ok(PromptOutcome::Completed(accumulated));
                            }
                            StreamEvent::Error { error } => {
                                return Err(anyhow!("provider stream error: {error}"));
                            }
                            _ => {}
                        }
                    }
                    Some(Err(err)) => return Err(err),
                }
            }
            line = reader.next_line(), if reader_open => {
                let line = match line? {
                    Some(line) => line,
                    // Input closed mid-turn: stop watching it, keep draining.
                    None => {
                        reader_open = false;
                        continue;
                    }
                };
                if line.trim().is_empty() {
                    continue;
                }
                let message: Value = match serde_json::from_str(&line) {
                    Ok(value) => value,
                    Err(err) => {
                        write_jsonrpc_error(writer, None, -32700, format!("invalid json: {err}"))
                            .await?;
                        continue;
                    }
                };
                let id = message.get("id").cloned();
                match message.get("method").and_then(Value::as_str) {
                    Some("session/cancel") => {
                        let target = message.pointer("/params/sessionId").and_then(Value::as_str);
                        // A cancel with no sessionId is treated as targeting the
                        // single in-flight turn.
                        if target.is_none() || target == Some(session_id) {
                            if let Some(id) = id {
                                write_jsonrpc_result(writer, id, json!(null)).await?;
                            }
                            // Dropping `stream` on return aborts the provider call.
                            return Ok(PromptOutcome::Cancelled);
                        }
                        // Cancel for some other session: acknowledge, keep going.
                        if let Some(id) = id {
                            write_jsonrpc_result(writer, id, json!(null)).await?;
                        }
                    }
                    _ => {
                        // The turn is single-flight; do not silently drop a
                        // request the client expects a response to.
                        if let Some(id) = id {
                            write_jsonrpc_error(
                                writer,
                                Some(id),
                                -32603,
                                "a session/prompt turn is already in progress",
                            )
                            .await?;
                        }
                    }
                }
            }
        }
    }
}

struct AcpServer {
    config: Config,
    model: String,
    default_cwd: PathBuf,
    sessions: HashMap<String, AcpSession>,
}

struct AcpSession {
    cwd: PathBuf,
    messages: Vec<Message>,
}

/// The `&mut self` result of validating a `session/prompt`: the user turn is
/// already recorded, and the cloned conversation + cwd are ready for the
/// borrow-free provider call that the prompt driver races against cancellation.
struct PreparedPrompt {
    session_id: String,
    messages: Vec<Message>,
    cwd: PathBuf,
}

enum AcpDispatch {
    Response(Value),
    Shutdown,
}

#[derive(Debug)]
struct AcpError {
    code: i32,
    message: String,
}

impl AcpServer {
    fn new(config: Config, model: String, default_cwd: PathBuf) -> Self {
        Self {
            config,
            model,
            default_cwd,
            sessions: HashMap::new(),
        }
    }

    // `session/prompt` is handled in the main loop (it needs to run concurrently
    // with the reader for cancellation); every other method is request/response.
    async fn handle_request(
        &mut self,
        method: &str,
        params: Value,
    ) -> std::result::Result<AcpDispatch, AcpError> {
        match method {
            "initialize" => Ok(AcpDispatch::Response(initialize_result(
                params.get("protocolVersion").and_then(Value::as_u64),
                &self.config,
            ))),
            "session/new" => Ok(AcpDispatch::Response(self.new_session(params)?)),
            "session/listProviders" => Ok(AcpDispatch::Response(self.list_providers())),
            "session/currentModel" => Ok(AcpDispatch::Response(self.current_model())),
            "session/selectModel" => Ok(AcpDispatch::Response(self.select_model(params)?)),
            // A cancel that arrives with no prompt in flight is an idempotent
            // no-op (the in-flight case is handled by the prompt driver).
            "session/cancel" => Ok(AcpDispatch::Response(json!(null))),
            "shutdown" => Ok(AcpDispatch::Shutdown),
            _ => Err(AcpError::method_not_found(method)),
        }
    }

    fn new_session(&mut self, params: Value) -> std::result::Result<Value, AcpError> {
        let cwd = params
            .get("cwd")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_cwd.clone());
        let session_id = format!("codewhale-{}", uuid::Uuid::new_v4());
        self.sessions.insert(
            session_id.clone(),
            AcpSession {
                cwd,
                messages: Vec::new(),
            },
        );
        Ok(json!({ "sessionId": session_id }))
    }

    fn list_providers(&self) -> Value {
        let mut providers = ApiProvider::sorted_for_display()
            .into_iter()
            .map(|provider| {
                json!({
                    "id": provider.as_str(),
                    "displayName": provider.display_name(),
                    "defaultModel": provider.metadata().map(|metadata| metadata.default_model())
                })
            })
            .collect::<Vec<_>>();

        // Include user-defined `[providers.<name>]` custom entries so ACP
        // clients can discover and round-trip the provider names that
        // `session/selectModel` now accepts (#1519).
        if let Some(custom) = self.config.providers.as_ref().map(|p| &p.custom) {
            let mut names = custom.keys().collect::<Vec<_>>();
            names.sort();
            for name in names {
                providers.push(json!({
                    "id": name,
                    "displayName": name,
                    "defaultModel": custom.get(name).and_then(|cfg| cfg.model.clone())
                }));
            }
        }

        json!({ "providers": providers })
    }

    fn current_model(&self) -> Value {
        // Prefer the raw configured provider key so a custom `[providers.<name>]`
        // entry round-trips through ACP instead of canonicalizing to "custom".
        let provider = match self.config.provider.as_deref() {
            Some(name) if !name.trim().is_empty() => name.to_string(),
            _ => self.config.api_provider().as_str().to_string(),
        };
        json!({
            "provider": provider,
            "model": self.model.as_str()
        })
    }

    fn select_model(&mut self, params: Value) -> std::result::Result<Value, AcpError> {
        let model = params
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| AcpError::invalid_params("model is required"))?
            .to_string();

        if let Some(provider_value) = params.get("provider") {
            let provider_name = provider_value
                .as_str()
                .ok_or_else(|| AcpError::invalid_params("provider must be a string"))?;
            // Accept either a built-in provider id/alias or a user-defined
            // custom provider name that has a `[providers.<name>]` table. For
            // custom providers, preserve the raw key so routing can still find
            // the configured base URL / auth / model (#1519); canonicalizing to
            // "custom" would lose that table key.
            let is_custom = self
                .config
                .providers
                .as_ref()
                .and_then(|providers| providers.custom_provider_config(provider_name))
                .is_some();
            if !is_custom && ApiProvider::parse(provider_name).is_none() {
                return Err(AcpError::invalid_params(format!(
                    "unknown provider: {provider_name}"
                )));
            }
            self.config.provider = Some(provider_name.to_string());
        }

        self.model = model;
        Ok(self.current_model())
    }

    /// Validate a `session/prompt` request and append the user turn to history,
    /// returning the cloned conversation for the (borrow-free) provider call.
    ///
    /// This is the `&mut self` half of a prompt turn; the streaming provider
    /// call lives in [`AcpServer::open_prompt_stream`] (which borrows `&self`
    /// only and returns a `'static` stream) so it can be raced against the
    /// reader for cancellation.
    fn begin_prompt(&mut self, params: Value) -> std::result::Result<PreparedPrompt, AcpError> {
        let session_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| AcpError::invalid_params("sessionId is required"))?
            .to_string();
        let prompt = extract_prompt_text(params.get("prompt"))
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| AcpError::invalid_params("prompt must include text content"))?;

        let (messages, cwd) = {
            let session = self
                .sessions
                .get_mut(&session_id)
                .ok_or_else(|| AcpError::invalid_params("unknown sessionId"))?;
            session.messages.push(Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: prompt,
                    cache_control: None,
                }],
            });
            (session.messages.clone(), session.cwd.clone())
        };

        Ok(PreparedPrompt {
            session_id,
            messages,
            cwd,
        })
    }

    /// Append a completed assistant turn to session history. A cancelled turn
    /// never calls this, so cancelled output does not pollute the transcript.
    fn finish_prompt(&mut self, session_id: &str, output: &str) {
        if output.is_empty() {
            return;
        }
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.messages.push(Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: output.to_string(),
                    cache_control: None,
                }],
            });
        }
    }

    /// Resolve the route, build the streaming request, and open the provider
    /// response stream. Borrows `&self` only to read config/model; the returned
    /// [`StreamEventBox`] is `'static`, so the caller can race it against the
    /// reader without holding any borrow on the server. The cwd guard only needs
    /// to cover route resolution and client construction, not stream
    /// consumption (ACP exposes no file/shell tools), so it is dropped here.
    async fn open_prompt_stream(
        &self,
        messages: &[Message],
        cwd: &PathBuf,
    ) -> Result<StreamEventBox> {
        let _cwd_guard = ScopedCurrentDir::new(cwd)?;
        let last_user_text = messages
            .iter()
            .rev()
            .find_map(|m| {
                if m.role == "user" {
                    m.content.iter().find_map(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                } else {
                    None
                }
            })
            .unwrap_or("");
        let route =
            crate::resolve_cli_auto_route(&self.config, &self.model, last_user_text).await?;
        let execution_config = crate::config_for_cli_route(&self.config, &route);
        let client = DeepSeekClient::new(&execution_config)?;
        let reasoning_effort = route
            .reasoning_effort
            .and_then(|effort| effort.api_value_for_provider(execution_config.api_provider()))
            .map(str::to_string);

        let request = MessageRequest {
            model: route.model,
            messages: messages.to_vec(),
            max_tokens: 4096,
            system: Some(SystemPrompt::Text(
                "You are a coding assistant inside an ACP-compatible editor. Give concise, actionable responses.".to_string(),
            )),
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort,
            stream: Some(true),
            temperature: Some(0.2),
            top_p: Some(0.9),
        };

        client.create_message_stream(request).await
    }
}

struct ScopedCurrentDir {
    prior: PathBuf,
}

impl ScopedCurrentDir {
    fn new(cwd: &PathBuf) -> Result<Self> {
        let prior = std::env::current_dir()?;
        if cwd.as_os_str().is_empty() {
            return Ok(Self { prior });
        }
        std::env::set_current_dir(cwd)
            .map_err(|err| anyhow!("failed to enter ACP session cwd {}: {err}", cwd.display()))?;
        Ok(Self { prior })
    }
}

impl Drop for ScopedCurrentDir {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.prior);
    }
}

impl AcpError {
    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }

    fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {method}"),
        }
    }
}

fn initialize_result(client_protocol_version: Option<u64>, config: &Config) -> Value {
    json!({
        "protocolVersion": client_protocol_version
            .map(|version| version.min(ACP_PROTOCOL_VERSION))
            .unwrap_or(ACP_PROTOCOL_VERSION),
        "agentCapabilities": {
            "loadSession": false,
            "modelSelection": true,
            "promptCapabilities": {
                "image": false,
                "audio": false,
                "embeddedContext": true
            },
            "mcpCapabilities": {
                "http": false,
                "sse": false
            },
            "sessionCapabilities": {}
        },
        "agentInfo": {
            "name": "codewhale",
            "title": "codewhale",
            "version": env!("CARGO_PKG_VERSION")
        },
        "authMethods": acp_auth_methods(config)
    })
}

fn acp_auth_methods(config: &Config) -> Value {
    let provider = config.api_provider().as_str();
    json!([
        {
            "id": "codewhale-terminal-auth",
            "name": "Set CodeWhale API key",
            "description": format!("Run CodeWhale's terminal credential setup for the {provider} provider."),
            "type": "terminal",
            "args": ["auth", "set", "--provider", provider],
            "env": {}
        }
    ])
}

fn extract_prompt_text(prompt: Option<&Value>) -> Option<String> {
    match prompt? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let parts = blocks
                .iter()
                .filter_map(content_block_text)
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join("\n\n"))
        }
        _ => None,
    }
}

fn content_block_text(block: &Value) -> Option<String> {
    match block.get("type").and_then(Value::as_str)? {
        "text" => block
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string),
        "resource" => resource_text(block),
        "resource_link" | "resourceLink" => resource_link_text(block),
        _ => None,
    }
}

fn resource_text(block: &Value) -> Option<String> {
    let resource = block.get("resource").unwrap_or(block);
    if let Some(text) = resource.get("text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    resource_link_text(resource)
}

fn resource_link_text(block: &Value) -> Option<String> {
    let uri = block
        .get("uri")
        .or_else(|| block.pointer("/resource/uri"))
        .and_then(Value::as_str)?;
    Some(format!("@{uri}"))
}

async fn write_session_update<W>(writer: &mut W, session_id: &str, text: String) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let notification = json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {
                    "type": "text",
                    "text": text
                }
            }
        }
    });
    write_json_line(writer, notification).await
}

async fn write_jsonrpc_result<W>(writer: &mut W, id: Value, result: Value) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let id = jsonrpc_response_id(id);
    write_json_line(
        writer,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        }),
    )
    .await
}

async fn write_jsonrpc_error<W>(
    writer: &mut W,
    id: Option<Value>,
    code: i32,
    message: impl Into<String>,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let id = id.map(jsonrpc_response_id);
    write_json_line(
        writer,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": code,
                "message": message.into()
            }
        }),
    )
    .await
}

async fn write_json_line<W>(writer: &mut W, value: Value) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer.write_all(value.to_string().as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

fn jsonrpc_response_id(id: Value) -> Value {
    match id {
        Value::Null => Value::Null,
        Value::String(_) => id,
        Value::Number(number) => Value::String(number.to_string()),
        other => Value::String(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_advertises_baseline_acp_agent() {
        let result = initialize_result(Some(1), &Config::default());

        assert_eq!(result["protocolVersion"], 1);
        assert_eq!(result["agentInfo"]["name"], "codewhale");
        assert_eq!(result["agentCapabilities"]["loadSession"], false);
        assert_eq!(
            result["agentCapabilities"]["promptCapabilities"]["embeddedContext"],
            true
        );
        assert_eq!(result["authMethods"][0]["type"], "terminal");
        assert_eq!(
            result["authMethods"][0]["args"],
            json!(["auth", "set", "--provider", "deepseek"])
        );
    }

    #[test]
    fn initialize_advertises_model_selection_capability() {
        let result = initialize_result(Some(1), &Config::default());

        assert_eq!(result["agentCapabilities"]["modelSelection"], true);
    }

    #[test]
    fn list_providers_returns_provider_set() {
        let server = AcpServer::new(
            Config::default(),
            "deepseek-chat".into(),
            PathBuf::from("/tmp"),
        );
        let result = server.list_providers();
        let providers = result["providers"].as_array().expect("providers array");

        assert!(!providers.is_empty());
        assert!(
            providers
                .iter()
                .any(|provider| provider["id"] == "deepseek")
        );
    }

    #[test]
    fn current_model_reflects_constructor_default() {
        let config = Config::default();
        let expected_provider = config.api_provider().as_str();
        let server = AcpServer::new(config, "deepseek-reasoner".into(), PathBuf::from("/tmp"));
        let result = server.current_model();

        assert_eq!(result["provider"], expected_provider);
        assert_eq!(result["model"], "deepseek-reasoner");
    }

    #[test]
    fn select_model_updates_active_selection() {
        let mut server = AcpServer::new(
            Config::default(),
            "deepseek-chat".into(),
            PathBuf::from("/tmp"),
        );

        let result = server
            .select_model(json!({ "provider": "openai", "model": "gpt-4o" }))
            .expect("select model");

        assert_eq!(result["provider"], "openai");
        assert_eq!(result["model"], "gpt-4o");
        assert_eq!(server.current_model()["provider"], "openai");
        assert_eq!(server.current_model()["model"], "gpt-4o");
    }

    #[test]
    fn select_model_rejects_unknown_provider() {
        let mut server = AcpServer::new(
            Config::default(),
            "deepseek-chat".into(),
            PathBuf::from("/tmp"),
        );
        let before = server.current_model();

        let err = server
            .select_model(json!({ "provider": "unknown-provider", "model": "gpt-4o" }))
            .expect_err("unknown provider rejected");

        assert_eq!(err.code, -32602);
        assert_eq!(server.current_model(), before);
    }

    #[test]
    fn select_model_rejects_missing_model() {
        let mut server = AcpServer::new(
            Config::default(),
            "deepseek-chat".into(),
            PathBuf::from("/tmp"),
        );

        let err = server
            .select_model(json!({ "provider": "openai" }))
            .expect_err("missing model rejected");

        assert_eq!(err.code, -32602);
    }

    #[test]
    fn extract_prompt_text_accepts_text_and_resource_blocks() {
        let prompt = json!([
            { "type": "text", "text": "Review this file" },
            {
                "type": "resource",
                "resource": {
                    "uri": "file:///tmp/app.rs",
                    "mimeType": "text/rust",
                    "text": "fn main() {}"
                }
            },
            { "type": "resource_link", "uri": "file:///tmp/lib.rs" }
        ]);

        let text = extract_prompt_text(Some(&prompt)).expect("prompt text");

        assert!(text.contains("Review this file"));
        assert!(text.contains("fn main() {}"));
        assert!(text.contains("@file:///tmp/lib.rs"));
    }

    #[tokio::test]
    async fn session_update_is_protocol_clean_single_line_json() {
        let mut out = Vec::new();

        write_session_update(&mut out, "sess_1", "hello\nworld".to_string())
            .await
            .expect("write update");

        let line = String::from_utf8(out).expect("utf8");
        assert_eq!(line.lines().count(), 1);
        let value: Value = serde_json::from_str(line.trim()).expect("json");
        assert_eq!(value["method"], "session/update");
        assert_eq!(value["params"]["sessionId"], "sess_1");
        assert_eq!(value["params"]["update"]["content"]["text"], "hello\nworld");
    }

    #[tokio::test]
    async fn jsonrpc_result_stringifies_numeric_ids_for_zed_acp() {
        let mut out = Vec::new();

        write_jsonrpc_result(&mut out, json!(1), json!({"ok": true}))
            .await
            .expect("write result");

        let line = String::from_utf8(out).expect("utf8");
        let value: Value = serde_json::from_str(line.trim()).expect("json");
        assert_eq!(value["id"], "1");
        assert_eq!(value["result"], json!({"ok": true}));
    }

    #[tokio::test]
    async fn jsonrpc_error_keeps_absent_id_null() {
        let mut out = Vec::new();

        write_jsonrpc_error(&mut out, None, -32700, "invalid json")
            .await
            .expect("write error");

        let line = String::from_utf8(out).expect("utf8");
        let value: Value = serde_json::from_str(line.trim()).expect("json");
        assert_eq!(value["id"], Value::Null);
        assert_eq!(value["error"]["code"], -32700);
    }

    #[test]
    fn new_session_starts_with_empty_messages() {
        let mut server = AcpServer::new(
            Config::default(),
            "test-model".to_string(),
            PathBuf::from("/tmp"),
        );
        let result = server
            .new_session(json!({ "cwd": "/tmp" }))
            .expect("new session");
        let session_id = result["sessionId"].as_str().expect("session id");
        let session = server.sessions.get(session_id).expect("session exists");
        assert!(session.messages.is_empty());
    }

    #[test]
    fn prompt_appends_user_and_assistant_messages_to_history() {
        let mut server = AcpServer::new(
            Config::default(),
            "test-model".to_string(),
            PathBuf::from("/tmp"),
        );
        let result = server
            .new_session(json!({ "cwd": "/tmp" }))
            .expect("new session");
        let session_id = result["sessionId"].as_str().unwrap().to_string();

        // Simulate adding a user message (same logic as prompt() but without LLM call)
        {
            let session = server.sessions.get_mut(&session_id).unwrap();
            session.messages.push(Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "1+1".to_string(),
                    cache_control: None,
                }],
            });
        }

        // Simulate assistant response
        {
            let session = server.sessions.get_mut(&session_id).unwrap();
            session.messages.push(Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "2".to_string(),
                    cache_control: None,
                }],
            });
        }

        // Second user message
        {
            let session = server.sessions.get_mut(&session_id).unwrap();
            session.messages.push(Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "add one more".to_string(),
                    cache_control: None,
                }],
            });
        }

        // Verify full conversation history
        let session = server.sessions.get(&session_id).unwrap();
        assert_eq!(session.messages.len(), 3);
        assert_eq!(session.messages[0].role, "user");
        assert_eq!(session.messages[1].role, "assistant");
        assert_eq!(session.messages[2].role, "user");

        // Verify text content
        assert_eq!(
            match &session.messages[0].content[0] {
                ContentBlock::Text { text, .. } => text.clone(),
                _ => String::new(),
            },
            "1+1"
        );
        assert_eq!(
            match &session.messages[1].content[0] {
                ContentBlock::Text { text, .. } => text.clone(),
                _ => String::new(),
            },
            "2"
        );
        assert_eq!(
            match &session.messages[2].content[0] {
                ContentBlock::Text { text, .. } => text.clone(),
                _ => String::new(),
            },
            "add one more"
        );
    }

    fn lines_from(input: &'static str) -> Lines<BufReader<&'static [u8]>> {
        BufReader::new(input.as_bytes()).lines()
    }

    fn text_delta(text: &str) -> StreamEvent {
        StreamEvent::ContentBlockDelta {
            index: 0,
            delta: Delta::TextDelta {
                text: text.to_string(),
            },
        }
    }

    /// A stream that yields the given events immediately, then ends.
    fn ready_stream(events: Vec<StreamEvent>) -> StreamEventBox {
        Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok::<_, anyhow::Error>),
        ))
    }

    /// A stream that never yields, so a concurrent cancel always wins.
    fn pending_stream() -> StreamEventBox {
        Box::pin(futures_util::stream::pending::<Result<StreamEvent>>())
    }

    /// A stream that yields `events` immediately, then emits `message_stop`
    /// after a short delay — long enough that an already-buffered reader line is
    /// processed first, making the ordering deterministic in tests.
    fn events_then_delayed_stop(events: Vec<StreamEvent>) -> StreamEventBox {
        let head = futures_util::stream::iter(events.into_iter().map(Ok::<_, anyhow::Error>));
        let tail = futures_util::stream::once(async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            Ok(StreamEvent::MessageStop)
        });
        Box::pin(head.chain(tail))
    }

    fn parse_lines(out: Vec<u8>) -> Vec<Value> {
        String::from_utf8(out)
            .expect("utf8")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("json"))
            .collect()
    }

    #[tokio::test]
    async fn drive_prompt_streams_each_delta_as_a_chunk_then_completes() {
        let stream = ready_stream(vec![
            text_delta("hello"),
            text_delta(" world"),
            StreamEvent::MessageStop,
        ]);
        let mut reader = lines_from("");
        let mut out = Vec::new();

        let outcome = drive_prompt_stream(stream, "sess_1", &mut reader, &mut out)
            .await
            .expect("driver ok");

        // Full text is accumulated for history...
        assert_eq!(outcome, PromptOutcome::Completed("hello world".to_string()));
        // ...and each delta was emitted as its own session/update chunk.
        let updates = parse_lines(out);
        assert_eq!(updates.len(), 2);
        assert!(updates.iter().all(|u| u["method"] == "session/update"));
        assert_eq!(updates[0]["params"]["update"]["content"]["text"], "hello");
        assert_eq!(updates[1]["params"]["update"]["content"]["text"], " world");
    }

    #[tokio::test]
    async fn drive_prompt_cancels_when_matching_cancel_arrives() {
        // A provider stream that never finishes within the test.
        let stream = pending_stream();
        let mut reader = lines_from(
            r#"{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"sess_1"}}"#,
        );
        let mut out = Vec::new();

        let outcome = drive_prompt_stream(stream, "sess_1", &mut reader, &mut out)
            .await
            .expect("driver ok");

        assert_eq!(outcome, PromptOutcome::Cancelled);
        // Notification-form cancel (no id) is acknowledged by acting, not writing.
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn drive_prompt_ignores_cancel_for_a_different_session() {
        // The unrelated cancel line is buffered and ready; the delayed stop makes
        // it process first, proving it does not abort the turn.
        let stream = events_then_delayed_stop(vec![text_delta("kept")]);
        let mut reader = lines_from(
            r#"{"jsonrpc":"2.0","id":7,"method":"session/cancel","params":{"sessionId":"other"}}"#,
        );
        let mut out = Vec::new();

        let outcome = drive_prompt_stream(stream, "sess_1", &mut reader, &mut out)
            .await
            .expect("driver ok");

        assert_eq!(outcome, PromptOutcome::Completed("kept".to_string()));
        // The other-session cancel carried an id, so it was acknowledged with null.
        let lines = parse_lines(out);
        assert!(
            lines
                .iter()
                .any(|v| v["id"] == "7" && v["result"] == Value::Null),
            "expected a null ack for the other-session cancel, got {lines:?}"
        );
    }

    #[tokio::test]
    async fn drive_prompt_rejects_a_concurrent_request_but_keeps_running() {
        let stream = events_then_delayed_stop(vec![text_delta("done")]);
        // A non-cancel request arrives mid-turn.
        let mut reader =
            lines_from(r#"{"jsonrpc":"2.0","id":9,"method":"session/new","params":{}}"#);
        let mut out = Vec::new();

        let outcome = drive_prompt_stream(stream, "sess_1", &mut reader, &mut out)
            .await
            .expect("driver ok");

        assert_eq!(outcome, PromptOutcome::Completed("done".to_string()));
        let lines = parse_lines(out);
        assert!(
            lines
                .iter()
                .any(|v| v["id"] == "9" && v["error"]["code"] == -32603),
            "expected a prompt-in-progress error for the concurrent request, got {lines:?}"
        );
    }

    #[test]
    fn different_sessions_have_independent_history() {
        let mut server = AcpServer::new(
            Config::default(),
            "test-model".to_string(),
            PathBuf::from("/tmp"),
        );
        let result1 = server
            .new_session(json!({ "cwd": "/tmp" }))
            .expect("session 1");
        let result2 = server
            .new_session(json!({ "cwd": "/tmp" }))
            .expect("session 2");
        let sid1 = result1["sessionId"].as_str().unwrap().to_string();
        let sid2 = result2["sessionId"].as_str().unwrap().to_string();

        // Add messages to session 1
        {
            let session = server.sessions.get_mut(&sid1).unwrap();
            session.messages.push(Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                    cache_control: None,
                }],
            });
        }

        // Session 2 should remain empty
        let session2 = server.sessions.get(&sid2).unwrap();
        assert!(session2.messages.is_empty());

        // Session 1 should have the message
        let session1 = server.sessions.get(&sid1).unwrap();
        assert_eq!(session1.messages.len(), 1);
    }
}
