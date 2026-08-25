//! Minimal stdio MCP server (line-delimited JSON-RPC 2.0).
//!
//! Codewhale's MCP client pins protocol `2024-11-05`
//! (`crates/tui/src/mcp.rs`); newer clients get their own version echoed
//! back when it is one we understand. Only `initialize`, `ping`,
//! `tools/list`, and `tools/call` are meaningful; notifications are ignored.

use std::io::{BufRead, Write};

use base64::Engine;
use serde_json::{Value, json};

use crate::session::{Session, tool_catalog};

pub const SERVER_NAME: &str = "codewhale-computer-use";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_PROTOCOL: &str = "2024-11-05";
const KNOWN_PROTOCOLS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

pub const INSTRUCTIONS: &str = "Computer-use tools. Start with computer_screenshot; give every x/y in pixels of the most recent screenshot image (top-left origin). Action tools return a fresh screenshot. Use computer_zoom for small targets, computer_ui_tree on phones, and stop to ask the user before credentials, payments, or destructive confirmations.";

pub struct McpServer {
    session: Session,
}

impl McpServer {
    pub fn new(session: Session) -> Self {
        Self { session }
    }

    /// Serve until stdin closes. Returns the process exit code.
    pub fn serve<R: BufRead, W: Write>(&mut self, reader: R, mut writer: W) -> i32 {
        for line in reader.lines() {
            let line = match line {
                Ok(line) => line,
                Err(_) => break,
            };
            if line.trim().is_empty() {
                continue;
            }
            let responses = self.handle_line(&line);
            for response in responses {
                if serde_json::to_writer(&mut writer, &response).is_err()
                    || writer.write_all(b"\n").is_err()
                {
                    return 1;
                }
                if writer.flush().is_err() {
                    return 1;
                }
            }
        }
        0
    }

    /// Handle one line; batches yield one response per request.
    pub fn handle_line(&mut self, line: &str) -> Vec<Value> {
        let parsed: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return vec![error_response(
                    Value::Null,
                    -32700,
                    &format!("parse error: {e}"),
                )];
            }
        };
        match parsed {
            Value::Array(items) => items
                .into_iter()
                .filter_map(|m| self.handle_message(m))
                .collect(),
            other => self.handle_message(other).into_iter().collect(),
        }
    }

    fn handle_message(&mut self, message: Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let Some(method) = method else {
            // A response or malformed message; nothing to answer.
            return id
                .filter(|id| !id.is_null())
                .map(|id| error_response(id, -32600, "invalid request: missing method"));
        };
        let Some(id) = id.filter(|id| !id.is_null()) else {
            // Notification.
            return None;
        };
        let result = match method.as_str() {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_catalog() })),
            "tools/call" => self.tools_call(&params),
            "resources/list" => Ok(json!({ "resources": [] })),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            other => Err((-32601, format!("method not found: {other}"))),
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => error_response(id, code, &message),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_PROTOCOL);
        let version = if KNOWN_PROTOCOLS.contains(&requested) {
            requested
        } else {
            DEFAULT_PROTOCOL
        };
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
            "instructions": INSTRUCTIONS,
        })
    }

    fn tools_call(&mut self, params: &Value) -> Result<Value, (i32, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| (-32602, "tools/call requires a string `name`".to_string()))?;
        let args = params.get("arguments").cloned().unwrap_or(Value::Null);
        if !args.is_null() && !args.is_object() {
            return Err((
                -32602,
                "tools/call `arguments` must be an object".to_string(),
            ));
        }
        let outcome = self.session.call(name, &args);
        let mut content = vec![json!({ "type": "text", "text": outcome.text })];
        if let Some(png) = outcome.image_png {
            content.push(json!({
                "type": "image",
                "data": base64::engine::general_purpose::STANDARD.encode(png),
                "mimeType": "image/png",
            }));
        }
        Ok(json!({ "content": content, "isError": outcome.is_error }))
    }
}

fn error_response(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::drivers::mock::MockDriver;

    fn server() -> McpServer {
        let (driver, _) = MockDriver::new(1600, 1000);
        let cfg = Config {
            settle_ms_desktop: 0,
            ..Config::default()
        };
        McpServer::new(Session::new(Box::new(driver), cfg))
    }

    #[test]
    fn handshake_echoes_known_version_and_defaults_otherwise() {
        let mut s = server();
        let out = s.handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#);
        assert_eq!(out[0]["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(out[0]["result"]["serverInfo"]["name"], SERVER_NAME);
        let out = s.handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#);
        assert_eq!(out[0]["result"]["protocolVersion"], DEFAULT_PROTOCOL);
        assert!(
            s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .is_empty()
        );
        let out = s.handle_line(r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#);
        assert_eq!(out[0]["result"], json!({}));
    }

    #[test]
    fn tools_list_and_call_return_image_blocks() {
        let mut s = server();
        let out = s.handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        let tools = out[0]["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), crate::session::TOOL_NAMES.len());
        let out = s.handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"computer_screenshot","arguments":{}}}"#);
        let result = &out[0]["result"];
        assert_eq!(result["isError"], false);
        let content = result["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert!(
            content[0]["text"]
                .as_str()
                .unwrap()
                .starts_with("frame: 1024x640")
        );
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["mimeType"], "image/png");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(content[1]["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn errors_are_reported_as_tool_errors_or_jsonrpc_errors() {
        let mut s = server();
        let out = s.handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"computer_click","arguments":{"x":1,"y":1}}}"#);
        assert_eq!(out[0]["result"]["isError"], true);
        let out = s.handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"nope"}"#);
        assert_eq!(out[0]["error"]["code"], -32601);
        let out = s.handle_line("{not json");
        assert_eq!(out[0]["error"]["code"], -32700);
        let out = s.handle_line(r#"[{"jsonrpc":"2.0","id":3,"method":"ping"},{"jsonrpc":"2.0","id":4,"method":"ping"}]"#);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn serve_reads_until_eof() {
        let mut s = server();
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n";
        let mut out = Vec::new();
        let code = s.serve(&input[..], &mut out);
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 2);
    }
}
