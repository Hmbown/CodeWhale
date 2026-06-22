//! Minimal Agent Client Protocol stdio adapter.
//!
//! This intentionally starts with the ACP baseline: initialize, new session,
//! prompt, and cancel. It keeps stdout protocol-clean for editor clients and
//! routes prompts through the same configured DeepSeek client as one-shot CLI
//! mode.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::client::DeepSeekClient;
use crate::config::Config;
use crate::llm_client::LlmClient;
use crate::models::{ContentBlock, Message, MessageRequest, SystemPrompt};

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

        match server.handle_request(method, params, &mut writer).await {
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

    async fn handle_request<W>(
        &mut self,
        method: &str,
        params: Value,
        writer: &mut W,
    ) -> std::result::Result<AcpDispatch, AcpError>
    where
        W: AsyncWrite + Unpin,
    {
        match method {
            "initialize" => Ok(AcpDispatch::Response(initialize_result(
                params.get("protocolVersion").and_then(Value::as_u64),
                &self.config,
            ))),
            "session/new" => Ok(AcpDispatch::Response(self.new_session(params)?)),
            "session/prompt" => {
                self.prompt(params, writer).await?;
                Ok(AcpDispatch::Response(json!({ "stopReason": "end_turn" })))
            }
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

    async fn prompt<W>(
        &mut self,
        params: Value,
        writer: &mut W,
    ) -> std::result::Result<(), AcpError>
    where
        W: AsyncWrite + Unpin,
    {
        let session_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| AcpError::invalid_params("sessionId is required"))?
            .to_string();
        let prompt = extract_prompt_text(params.get("prompt"))
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| AcpError::invalid_params("prompt must include text content"))?;

        // Append user message to session history
        {
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
        }

        // Clone messages for the LLM call (avoids borrowing self across await)
        let (messages, cwd) = {
            let session = self
                .sessions
                .get(&session_id)
                .ok_or_else(|| AcpError::invalid_params("unknown sessionId"))?;
            (session.messages.clone(), session.cwd.clone())
        };

        let output = self
            .run_prompt(&messages, &cwd)
            .await
            .map_err(|err| AcpError::internal(err.to_string()))?;

        // Append assistant response to session history
        if !output.is_empty() {
            {
                let session = self
                    .sessions
                    .get_mut(&session_id)
                    .ok_or_else(|| AcpError::invalid_params("unknown sessionId"))?;
                session.messages.push(Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::Text {
                        text: output.clone(),
                        cache_control: None,
                    }],
                });
            }

            write_session_update(writer, &session_id, output)
                .await
                .map_err(|err| AcpError::internal(err.to_string()))?;
        }

        Ok(())
    }

    async fn run_prompt(&self, messages: &[Message], cwd: &PathBuf) -> Result<String> {
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
            stream: Some(false),
            temperature: Some(0.2),
            top_p: Some(0.9),
        };

        let response = client.create_message(request).await?;
        let mut output = String::new();
        for block in response.content {
            if let ContentBlock::Text { text, .. } = block {
                output.push_str(&text);
            }
        }
        Ok(output)
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

    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
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
