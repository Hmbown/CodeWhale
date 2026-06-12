//! OpenAI Responses API bridge for the OpenAI Codex / ChatGPT provider.
//!
//! Implements a dedicated Responses API client that maps CodeWhale's internal
//! message/tool types to the Responses wire format and parses streaming SSE
//! events back into CodeWhale's `StreamEvent` / `MessageResponse` types.
//!
//! This is intentionally separate from the Chat Completions path
//! (`client/chat.rs`) to avoid protocol hacks.

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::llm_client::StreamEventBox;
use crate::logging;
use crate::models::{
    ContentBlock, ContentBlockStart, Delta, MessageDelta, MessageRequest, MessageResponse,
    StreamEvent, Tool, Usage,
};
use crate::tools::schema_sanitize;

use super::{DeepSeekClient, ERROR_BODY_MAX_BYTES, bounded_error_text, system_to_instructions};

/// Base URL path for the Codex Responses endpoint.
const CODEX_RESPONSES_PATH: &str = "/codex/responses";

impl DeepSeekClient {
    /// Build the Responses API request body from a `MessageRequest`.
    fn build_responses_body(&self, request: &MessageRequest) -> Value {
        let model = &request.model;
        let mut body = json!({
            "model": model,
            "stream": true,
            "store": false,
        });

        // Instructions (system prompt). The Codex Responses backend rejects
        // requests without instructions, so fall back to a minimal system
        // prompt when the caller did not supply one.
        let instructions = system_to_instructions(request.system.clone())
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| "You are a helpful assistant.".to_string());
        body["instructions"] = json!(instructions);

        // Convert messages to Responses input items.
        let input = convert_messages_to_responses_input(request);
        body["input"] = json!(input);

        // Convert tools to Responses function tools.
        if let Some(tools) = request.tools.as_ref() {
            let responses_tools: Vec<Value> =
                tools.iter().map(tool_to_responses_function).collect();
            if !responses_tools.is_empty() {
                body["tools"] = json!(responses_tools);
                body["tool_choice"] = json!("auto");
                body["parallel_tool_calls"] = json!(true);
            }
        }

        // Reasoning configuration. The Codex Responses backend only accepts a
        // fixed set of effort levels (none/minimal/low/medium/high/xhigh), so
        // map CodeWhale's effort string onto those and omit reasoning entirely
        // when it is disabled. CodeWhale's "auto" has no Codex equivalent and
        // falls back to "medium".
        if let Some(raw) = request.reasoning_effort.as_deref()
            && let Some(effort) = codex_responses_reasoning_effort(raw)
        {
            body["reasoning"] = json!({
                "effort": effort,
                "summary": "auto",
            });
        }

        // Include reasoning summaries in the stream.
        body["include"] = json!(["reasoning.encrypted_content"]);

        body
    }

    /// Handle a streaming Responses API request for the OpenAI Codex provider.
    pub(super) async fn handle_responses_stream(
        &self,
        request: MessageRequest,
    ) -> Result<StreamEventBox> {
        let body = self.build_responses_body(&request);
        let url = format!("{}{}", self.base_url, CODEX_RESPONSES_PATH);

        // The bearer Authorization header is already installed as a default
        // header on `http_client` (resolved from the Codex OAuth access token),
        // so it must not be set again here or it would be duplicated. The
        // ChatGPT backend additionally requires the account id and the
        // experimental Responses beta opt-in.
        let mut builder = self
            .http_client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .header("OpenAI-Beta", "responses=experimental")
            .header("originator", "codex_cli_rs");
        if let Some(account_id) = crate::oauth::codex_account_id() {
            builder = builder.header("chatgpt-account-id", account_id);
        }

        let response = builder
            .json(&body)
            .send()
            .await
            .context("Responses API request failed")?;

        let status = response.status();
        if !status.is_success() {
            let raw = bounded_error_text(response, ERROR_BODY_MAX_BYTES).await;
            anyhow::bail!("Responses API error (HTTP {status}): {raw}");
        }

        let stream_idle_timeout = self.stream_idle_timeout;
        let byte_stream = response.bytes_stream();

        let stream = async_stream::stream! {
            use futures_util::StreamExt;

            // Emit synthetic MessageStart.
            yield Ok(StreamEvent::MessageStart {
                message: MessageResponse {
                    id: String::new(),
                    r#type: "message".to_string(),
                    role: "assistant".to_string(),
                    content: vec![],
                    model: request.model.clone(),
                    stop_reason: None,

                    container: None,
                    usage: Usage::default(),
                },
            });

            let mut current_block_index: Option<u32> = None;
            let mut saw_tool_call = false;
            let mut usage_data: Option<Usage> = None;
            let mut buffer = String::new();
            let mut done = false;
            let mut content_block_counter: u32 = 0;

            tokio::pin!(byte_stream);

            while !done {
                let chunk = match tokio::time::timeout(stream_idle_timeout, byte_stream.next()).await {
                    Ok(Some(Ok(chunk))) => chunk,
                    Ok(Some(Err(e))) => {
                        yield Err(anyhow::anyhow!("Stream read error: {e}"));
                        return;
                    }
                    Ok(None) => break,
                    Err(_) => {
                        yield Err(anyhow::anyhow!("Stream idle timeout"));
                        return;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Process complete SSE lines.
                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }

                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            done = true;
                            break;
                        }

                        let event: Value = match serde_json::from_str(data) {
                            Ok(v) => v,
                            Err(e) => {
                                logging::warn(format!(
                                    "Failed to parse Responses SSE event: {e}"
                                ));
                                continue;
                            }
                        };

                        let event_type =
                            event.get("type").and_then(|t| t.as_str()).unwrap_or("");

                        match event_type {
                            "response.output_item.added" => {
                                if let Some(item) = event.get("item") {
                                    let item_type = item
                                        .get("type")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");

                                    match item_type {
                                        "message" => {
                                            content_block_counter += 1;
                                            yield Ok(StreamEvent::ContentBlockStart {
                                                index: content_block_counter - 1,
                                                content_block: ContentBlockStart::Text {
                                                    text: String::new(),
                                                },
                                            });
                                            current_block_index =
                                                Some(content_block_counter - 1);
                                        }
                                        "function_call" => {
                                            let call_id = item
                                                .get("call_id")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let item_id = item
                                                .get("id")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let name = item
                                                .get("name")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            saw_tool_call = true;
                                            // call_id and item_id are folded
                                            // into a composite tool-use id so
                                            // the function_call_output can be
                                            // routed back to the right call.
                                            let composite_id =
                                                format!("{call_id}|{item_id}");
                                            content_block_counter += 1;
                                            yield Ok(StreamEvent::ContentBlockStart {
                                                index: content_block_counter - 1,
                                                content_block:
                                                    ContentBlockStart::ToolUse {
                                                        id: composite_id,
                                                        name,
                                                        input: json!({}),
                                                        caller: None,
                                                    },
                                            });
                                            current_block_index =
                                                Some(content_block_counter - 1);
                                        }
                                        "reasoning" => {
                                            content_block_counter += 1;
                                            yield Ok(StreamEvent::ContentBlockStart {
                                                index: content_block_counter - 1,
                                                content_block:
                                                    ContentBlockStart::Thinking {
                                                        thinking: String::new(),
                                                    },
                                            });
                                            current_block_index =
                                                Some(content_block_counter - 1);
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            "response.output_text.delta" => {
                                if let Some(delta_text) =
                                    event.get("delta").and_then(|d| d.as_str())
                                    && let Some(idx) = current_block_index
                                {
                                    yield Ok(StreamEvent::ContentBlockDelta {
                                        index: idx,
                                        delta: Delta::TextDelta {
                                            text: delta_text.to_string(),
                                        },
                                    });
                                }
                            }
                            "response.function_call_arguments.delta" => {
                                if let Some(delta_text) =
                                    event.get("delta").and_then(|d| d.as_str())
                                    && let Some(idx) = current_block_index
                                {
                                    yield Ok(StreamEvent::ContentBlockDelta {
                                        index: idx,
                                        delta: Delta::InputJsonDelta {
                                            partial_json: delta_text.to_string(),
                                        },
                                    });
                                }
                            }
                            "response.reasoning_summary_text.delta"
                            | "response.reasoning_text.delta" => {
                                if let Some(delta_text) =
                                    event.get("delta").and_then(|d| d.as_str())
                                    && let Some(idx) = current_block_index
                                {
                                    yield Ok(StreamEvent::ContentBlockDelta {
                                        index: idx,
                                        delta: Delta::ThinkingDelta {
                                            thinking: delta_text.to_string(),
                                        },
                                    });
                                }
                            }
                            "response.output_item.done" => {
                                if let Some(idx) = current_block_index {
                                    yield Ok(StreamEvent::ContentBlockStop { index: idx });
                                    current_block_index = None;
                                }
                            }
                            "response.completed" => {
                                if let Some(resp) = event.get("response") {
                                    if let Some(usage_val) = resp.get("usage") {
                                        usage_data =
                                            Some(parse_responses_usage(usage_val));
                                    }
                                    let status = resp
                                        .get("status")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("completed");
                                    let stop_reason = match status {
                                        "completed" if saw_tool_call => "tool_use",
                                        "completed" => "end_turn",
                                        "incomplete" => "max_tokens",
                                        _ => "end_turn",
                                    };
                                    yield Ok(StreamEvent::MessageDelta {
                                        delta: MessageDelta {
                                            stop_reason: Some(stop_reason.to_string()),

                                        },
                                        usage: usage_data.take(),
                                    });
                                }
                            }
                            "error" => {
                                let msg = event
                                    .get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("Unknown error");
                                let code = event
                                    .get("code")
                                    .and_then(|c| c.as_str())
                                    .unwrap_or("unknown");
                                yield Err(anyhow::anyhow!(
                                    "Responses API error [{code}]: {msg}"
                                ));
                                return;
                            }
                            _ => {
                                // Ignore unknown event types.
                            }
                        }
                    }
                }
            }

            // Emit MessageStop.
            yield Ok(StreamEvent::MessageStop);
        };

        Ok(Box::pin(stream))
    }

    /// Non-streaming Responses request: drive the streaming handler and fold
    /// its events into a single `MessageResponse`.
    ///
    /// The ChatGPT Codex backend only serves streaming responses, so the
    /// non-streaming entry point (`create_message`, used by `exec`) reuses the
    /// same wire path as the interactive stream rather than a second request
    /// shape.
    pub(super) async fn handle_responses_message(
        &self,
        request: MessageRequest,
    ) -> Result<MessageResponse> {
        use futures_util::StreamExt;

        let model = request.model.clone();
        let mut stream = self.handle_responses_stream(request).await?;

        let mut response = MessageResponse {
            id: String::new(),
            r#type: "message".to_string(),
            role: "assistant".to_string(),
            content: Vec::new(),
            model,
            stop_reason: None,

            container: None,
            usage: Usage::default(),
        };
        // Accumulated tool-call argument JSON, parallel to `response.content`.
        let mut tool_args: Vec<String> = Vec::new();

        while let Some(event) = stream.next().await {
            match event? {
                StreamEvent::MessageStart { message } => {
                    response.id = message.id;
                    response.usage = message.usage;
                }
                StreamEvent::ContentBlockStart { content_block, .. } => {
                    let block = match content_block {
                        ContentBlockStart::Text { text } => ContentBlock::Text {
                            text,
                            cache_control: None,
                        },
                        ContentBlockStart::Thinking { thinking } => ContentBlock::Thinking {
                            thinking,
                            signature: None,
                        },
                        ContentBlockStart::ToolUse {
                            id,
                            name,
                            input,
                            caller,
                        } => ContentBlock::ToolUse {
                            id,
                            name,
                            input,
                            caller,
                        },
                        ContentBlockStart::ServerToolUse { id, name, input } => {
                            ContentBlock::ServerToolUse { id, name, input }
                        }
                    };
                    response.content.push(block);
                    tool_args.push(String::new());
                }
                StreamEvent::ContentBlockDelta { index, delta } => {
                    let i = index as usize;
                    match delta {
                        Delta::TextDelta { text } => {
                            if let Some(ContentBlock::Text { text: existing, .. }) =
                                response.content.get_mut(i)
                            {
                                existing.push_str(&text);
                            }
                        }
                        Delta::ThinkingDelta { thinking } => {
                            if let Some(ContentBlock::Thinking {
                                thinking: existing, ..
                            }) = response.content.get_mut(i)
                            {
                                existing.push_str(&thinking);
                            }
                        }
                        Delta::InputJsonDelta { partial_json } => {
                            if let Some(buf) = tool_args.get_mut(i) {
                                buf.push_str(&partial_json);
                            }
                        }
                        Delta::SignatureDelta { .. } => {
                            // Anthropic-native signature deltas never occur on
                            // the Responses bridge (#3014).
                        }
                    }
                }
                StreamEvent::ContentBlockStop { index } => {
                    let i = index as usize;
                    if let Some(buf) = tool_args.get(i)
                        && !buf.trim().is_empty()
                        && let Ok(parsed) = serde_json::from_str::<Value>(buf)
                        && let Some(ContentBlock::ToolUse { input, .. }) =
                            response.content.get_mut(i)
                    {
                        *input = parsed;
                    }
                }
                StreamEvent::MessageDelta { delta, usage } => {
                    if let Some(stop_reason) = delta.stop_reason {
                        response.stop_reason = Some(stop_reason);
                    }
                    if let Some(usage) = usage {
                        response.usage = usage;
                    }
                }
                StreamEvent::MessageStop => break,
                _ => {}
            }
        }

        Ok(response)
    }
}

/// Convert CodeWhale messages to Responses API input items.
fn convert_messages_to_responses_input(request: &MessageRequest) -> Vec<Value> {
    let mut items = Vec::new();

    for msg in &request.messages {
        match msg.role.as_str() {
            "user" => {
                let mut content_items = Vec::new();
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            content_items.push(json!({
                                "type": "input_text",
                                "text": text,
                            }));
                        }
                        ContentBlock::ImageUrl { image_url } => {
                            content_items.push(json!({
                                "type": "input_image",
                                "image_url": image_url.url,
                            }));
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            if !content_items.is_empty() {
                                items.push(json!({
                                    "type": "message",
                                    "role": "user",
                                    "content": content_items,
                                }));
                                content_items = Vec::new();
                            }
                            let (call_id, _item_id) = parse_tool_use_id(tool_use_id);
                            items.push(json!({
                                "type": "function_call_output",
                                "call_id": call_id,
                                "output": content,
                            }));
                        }
                        _ => {}
                    }
                }
                if !content_items.is_empty() {
                    items.push(json!({
                        "type": "message",
                        "role": "user",
                        "content": content_items,
                    }));
                }
            }
            "assistant" => {
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            items.push(json!({
                                "type": "message",
                                "role": "assistant",
                                "content": [{
                                    "type": "output_text",
                                    "text": text,
                                }],
                            }));
                        }
                        ContentBlock::ToolUse {
                            id, name, input, ..
                        } => {
                            let (call_id, _item_id) = parse_tool_use_id(id);
                            items.push(json!({
                                "type": "function_call",
                                "call_id": call_id,
                                "name": name,
                                "arguments": serde_json::to_string(input).unwrap_or_default(),
                            }));
                        }
                        ContentBlock::Thinking { thinking, .. } => {
                            items.push(json!({
                                "type": "reasoning",
                                "summary": [{
                                    "type": "summary_text",
                                    "text": thinking,
                                }],
                            }));
                        }
                        _ => {}
                    }
                }
            }
            "tool" => {
                for block in &msg.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } = block
                    {
                        let (call_id, _item_id) = parse_tool_use_id(tool_use_id);
                        items.push(json!({
                            "type": "function_call_output",
                            "call_id": call_id,
                            "output": content,
                        }));
                    }
                }
            }
            _ => {}
        }
    }

    items
}

/// Convert a CodeWhale tool definition to a Responses API function tool.
fn tool_to_responses_function(tool: &Tool) -> Value {
    let mut parameters = tool.input_schema.clone();
    schema_sanitize::sanitize_for_responses(&mut parameters);
    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": parameters,
        "strict": false,
    })
}

fn codex_responses_reasoning_effort(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" | "disabled" | "none" | "false" => None,
        "minimal" => Some("minimal"),
        "low" => Some("low"),
        "high" => Some("high"),
        "xhigh" | "max" | "maximum" => Some("xhigh"),
        _ => Some("medium"),
    }
}

/// Parse a composite tool_use_id back to (call_id, item_id).
/// Composite format: "call_id|item_id"
fn parse_tool_use_id(id: &str) -> (String, String) {
    if let Some(pipe_pos) = id.find('|') {
        (id[..pipe_pos].to_string(), id[pipe_pos + 1..].to_string())
    } else {
        (id.to_string(), String::new())
    }
}

/// Parse usage from a Responses API usage object.
fn parse_responses_usage(val: &Value) -> Usage {
    let input = val
        .get("input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let output = val
        .get("output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let cached = val
        .get("input_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    Usage {
        input_tokens: input,
        output_tokens: output,
        prompt_cache_hit_tokens: if cached > 0 { Some(cached) } else { None },
        prompt_cache_miss_tokens: None,
        reasoning_tokens: None,
        reasoning_replay_tokens: None,
        server_tool_use: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Message;

    #[test]
    fn codex_reasoning_effort_uses_responses_labels() {
        assert_eq!(codex_responses_reasoning_effort("max"), Some("xhigh"));
        assert_eq!(codex_responses_reasoning_effort("maximum"), Some("xhigh"));
        assert_eq!(codex_responses_reasoning_effort("xhigh"), Some("xhigh"));
        assert_eq!(codex_responses_reasoning_effort("high"), Some("high"));
        assert_eq!(codex_responses_reasoning_effort("medium"), Some("medium"));
        assert_eq!(codex_responses_reasoning_effort("auto"), Some("medium"));
        assert_eq!(codex_responses_reasoning_effort("off"), None);
    }

    #[test]
    fn responses_input_includes_user_role_tool_results() {
        let request = MessageRequest {
            model: "gpt-5.5".to_string(),
            messages: vec![
                Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::ToolUse {
                        id: "call_abc|fc_123".to_string(),
                        name: "checklist_write".to_string(),
                        input: json!({"items": []}),
                        caller: None,
                    }],
                },
                Message {
                    role: "user".to_string(),
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: "call_abc|fc_123".to_string(),
                        content: "<6 items>".to_string(),
                        is_error: None,
                        content_blocks: None,
                    }],
                },
            ],
            max_tokens: 128,
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

        let input = convert_messages_to_responses_input(&request);

        assert_eq!(input[0]["type"], "function_call");
        assert_eq!(input[0]["call_id"], "call_abc");
        assert_eq!(input[1]["type"], "function_call_output");
        assert_eq!(input[1]["call_id"], "call_abc");
        assert_eq!(input[1]["output"], "<6 items>");
    }

    #[test]
    fn responses_function_tool_sanitizes_root_composition_schema() {
        let tool = Tool {
            tool_type: None,
            name: "apply_patch".to_string(),
            description: "Apply patch".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "patch": {"type": "string"},
                    "changes": {"type": "array"}
                },
                "oneOf": [
                    {"required": ["patch"]},
                    {"required": ["changes"]}
                ]
            }),
            allowed_callers: None,
            defer_loading: None,
            input_examples: None,
            strict: None,
            cache_control: None,
        };

        let payload = tool_to_responses_function(&tool);
        let parameters = &payload["parameters"];

        assert_eq!(parameters["type"], "object");
        assert!(parameters.get("oneOf").is_none());
        assert!(parameters.get("anyOf").is_none());
        assert!(parameters.get("allOf").is_none());
        assert!(parameters.get("enum").is_none());
        assert!(parameters.get("not").is_none());
        assert!(parameters["properties"].get("patch").is_some());
        assert!(parameters["properties"].get("changes").is_some());
        assert!(tool.input_schema.get("oneOf").is_some());
    }
}
