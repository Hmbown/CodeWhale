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

use super::{
    DeepSeekClient, ERROR_BODY_MAX_BYTES, bounded_error_text, from_api_tool_name,
    system_to_instructions, to_api_tool_name,
};

/// Base URL path for the Codex Responses endpoint.
const CODEX_RESPONSES_PATH: &str = "/codex/responses";

/// Build the Responses API request body from a `MessageRequest`.
fn build_responses_body(request: &MessageRequest) -> Value {
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
        let responses_tools: Vec<Value> = tools.iter().map(tool_to_responses_function).collect();
        if !responses_tools.is_empty() {
            body["tools"] = json!(responses_tools);
            body["tool_choice"] = json!("auto");
            body["parallel_tool_calls"] = json!(true);
        }
    }

    // Reasoning configuration. The Codex Responses backend accepts
    // low/medium/high/xhigh, so provider-aware callers normalize inherited
    // DeepSeek-only values before request construction: "off" becomes
    // "low", and CodeWhale's "auto" falls back to "medium".
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

impl DeepSeekClient {
    /// Handle a streaming Responses API request for the OpenAI Codex provider.
    pub(super) async fn handle_responses_stream(
        &self,
        request: MessageRequest,
    ) -> Result<StreamEventBox> {
        let body = build_responses_body(&request);
        let url = format!("{}{}", self.base_url, CODEX_RESPONSES_PATH);

        // The bearer Authorization header is already installed as a default
        // header on `http_client` (resolved from the Codex OAuth access token),
        // so it must not be set again here or it would be duplicated. The
        // ChatGPT backend additionally requires the account id and the
        // experimental Responses beta opt-in.
        let account_id = crate::oauth::codex_account_id();
        let response = self
            .send_with_retry(|| {
                let mut builder = self
                    .http_client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .header("Accept", "text/event-stream")
                    .header("OpenAI-Beta", "responses=experimental")
                    .header("originator", "codex_cli_rs");
                if let Some(account_id) = &account_id {
                    builder = builder.header("chatgpt-account-id", account_id);
                }
                builder.json(&body)
            })
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
                    stop_sequence: None,
                    container: None,
                    usage: Usage::default(),
                },
            });

            let mut current_block_index: Option<u32> = None;
            let mut saw_tool_call = false;
            let mut usage_data: Option<Usage> = None;
            // Raw byte buffer: decode only COMPLETE lines so a multi-byte
            // UTF-8 char split across two network reads is never corrupted
            // to U+FFFD (line boundaries are ASCII). Mirrors chat.rs.
            let mut buffer: Vec<u8> = Vec::new();
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

                buffer.extend_from_slice(&chunk);

                // Process complete SSE lines.
                while let Some(line) = super::take_sse_line(&mut buffer) {

                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }

                    if let Some(data) = super::extract_sse_data_value(&line) {
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
                                                        name: from_api_tool_name(&name),
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
                                            stop_sequence: None,
                                        },
                                        usage: usage_data.take(),
                                    });
                                }
                            }
                            "error" | "response.failed" | "response.incomplete" => {
                                let (code, msg) = responses_event_error_details(&event);
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
            stop_sequence: None,
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
                                "name": to_api_tool_name(name),
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
            "system" => {
                // Mid-conversation system messages (sub-agent fork-state and
                // `<codewhale:subagent_context>` blocks) were silently dropped
                // here, so forked codex children lost their fork context while
                // Chat-Completions children kept it. The Responses API accepts
                // system-role input messages, so pass them through.
                let content_items: Vec<Value> = msg
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text, .. } => Some(json!({
                            "type": "input_text",
                            "text": text,
                        })),
                        _ => None,
                    })
                    .collect();
                if !content_items.is_empty() {
                    items.push(json!({
                        "type": "message",
                        "role": "system",
                        "content": content_items,
                    }));
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
    let constraint_note = schema_sanitize::sanitize_for_responses(&mut parameters);
    let description = match constraint_note {
        Some(note) if tool.description.trim().is_empty() => note,
        Some(note) => format!("{}\n\n{}", tool.description.trim(), note),
        None => tool.description.clone(),
    };
    json!({
        "type": "function",
        "name": to_api_tool_name(&tool.name),
        "description": description,
        "parameters": parameters,
        "strict": false,
    })
}

fn codex_responses_reasoning_effort(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" | "disabled" | "none" | "false" => Some("low"),
        "minimal" => Some("low"),
        "low" => Some("low"),
        "high" => Some("high"),
        "xhigh" | "max" | "maximum" | "ultracode" => Some("xhigh"),
        _ => Some("medium"),
    }
}

fn responses_event_error_details(event: &Value) -> (String, String) {
    let event_type = string_at(event, "/type").unwrap_or("error");
    let code = first_string_at(
        event,
        &[
            "/code",
            "/error/code",
            "/response/error/code",
            "/response/incomplete_details/reason",
            "/response/status",
        ],
    )
    .unwrap_or("unknown");
    let message = first_string_at(
        event,
        &[
            "/message",
            "/error/message",
            "/response/error/message",
            "/response/incomplete_details/reason",
        ],
    )
    .map_or_else(
        || format!("{event_type} event received"),
        |message| {
            if message == code && event_type == "response.incomplete" {
                format!("response incomplete: {message}")
            } else {
                message.to_string()
            }
        },
    );
    (code.to_string(), message)
}

fn first_string_at<'a>(value: &'a Value, paths: &[&str]) -> Option<&'a str> {
    paths.iter().find_map(|path| string_at(value, path))
}

fn string_at<'a>(value: &'a Value, path: &str) -> Option<&'a str> {
    value.pointer(path).and_then(Value::as_str).and_then(|s| {
        let trimmed = s.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    })
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
    // Mirror the Chat-Completions parser: derive cache-miss as input minus the
    // cached hit when the payload reports cached input tokens. Responses nests
    // reasoning under `output_tokens_details` (not `completion_tokens_details`).
    let prompt_cache_hit_tokens = if cached > 0 { Some(cached) } else { None };
    let prompt_cache_miss_tokens = prompt_cache_hit_tokens.map(|hit| input.saturating_sub(hit));
    let reasoning_tokens = val
        .get("output_tokens_details")
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    Usage {
        input_tokens: input,
        output_tokens: output,
        prompt_cache_hit_tokens,
        prompt_cache_miss_tokens,
        reasoning_tokens,
        reasoning_replay_tokens: None,
        server_tool_use: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures_util::StreamExt;

    use crate::config::{Config, ProviderConfig, ProvidersConfig, RetryConfig};
    use crate::models::Message;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    #[derive(Clone)]
    struct RetryThenSuccess {
        attempts: Arc<AtomicUsize>,
        retry_status: u16,
        retry_body: &'static str,
    }

    impl Respond for RetryThenSuccess {
        fn respond(&self, _request: &Request) -> ResponseTemplate {
            if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                let mut response =
                    ResponseTemplate::new(self.retry_status).set_body_string(self.retry_body);
                if self.retry_status == 429 {
                    response = response.insert_header("Retry-After", "0");
                }
                return response;
            }

            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string("data: [DONE]\n\n")
        }
    }

    #[derive(Clone)]
    struct AlwaysError {
        attempts: Arc<AtomicUsize>,
        status: u16,
        body: &'static str,
    }

    impl Respond for AlwaysError {
        fn respond(&self, _request: &Request) -> ResponseTemplate {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(self.status).set_body_string(self.body)
        }
    }

    fn minimal_responses_request() -> MessageRequest {
        MessageRequest {
            model: "gpt-5.5".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                    cache_control: None,
                }],
            }],
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
        }
    }

    fn test_codex_config(server: &MockServer) -> Config {
        Config {
            provider: Some("openai-codex".to_string()),
            retry: Some(RetryConfig {
                enabled: Some(true),
                max_retries: Some(1),
                initial_delay: Some(0.0),
                max_delay: Some(0.0),
                exponential_base: Some(1.0),
            }),
            providers: Some(ProvidersConfig {
                openai_codex: ProviderConfig {
                    base_url: Some(server.uri()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        }
    }

    #[tokio::test]
    async fn responses_stream_retries_rate_limited_request() {
        let server = MockServer::start().await;
        let attempts = Arc::new(AtomicUsize::new(0));
        Mock::given(method("POST"))
            .and(path(CODEX_RESPONSES_PATH))
            .respond_with(RetryThenSuccess {
                attempts: Arc::clone(&attempts),
                retry_status: 429,
                retry_body: "rate limited",
            })
            .mount(&server)
            .await;

        let client = {
            let _env_lock = crate::test_support::lock_test_env();
            let _codex_token =
                crate::test_support::EnvVarGuard::set("OPENAI_CODEX_ACCESS_TOKEN", "test-token");
            let _legacy_codex_token =
                crate::test_support::EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
            DeepSeekClient::new(&test_codex_config(&server)).unwrap()
        };
        let mut stream = client
            .handle_responses_stream(minimal_responses_request())
            .await
            .unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(event) = stream.next().await {
                event.unwrap();
            }
        })
        .await
        .expect("Responses retry stream should finish after [DONE]");

        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn responses_stream_retries_transient_server_error() {
        let server = MockServer::start().await;
        let attempts = Arc::new(AtomicUsize::new(0));
        Mock::given(method("POST"))
            .and(path(CODEX_RESPONSES_PATH))
            .respond_with(RetryThenSuccess {
                attempts: Arc::clone(&attempts),
                retry_status: 503,
                retry_body: "temporarily unavailable",
            })
            .mount(&server)
            .await;

        let client = {
            let _env_lock = crate::test_support::lock_test_env();
            let _codex_token =
                crate::test_support::EnvVarGuard::set("OPENAI_CODEX_ACCESS_TOKEN", "test-token");
            let _legacy_codex_token =
                crate::test_support::EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
            DeepSeekClient::new(&test_codex_config(&server)).unwrap()
        };
        let mut stream = client
            .handle_responses_stream(minimal_responses_request())
            .await
            .unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(event) = stream.next().await {
                event.unwrap();
            }
        })
        .await
        .expect("Responses retry stream should finish after [DONE]");

        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    /// Fake ChatGPT Codex backend: enforces the subscription backend's model
    /// policy the way the real endpoint does — any model outside the Codex
    /// family is refused with the exact observed 400 shape:
    /// `The '<model>' model is not supported when using Codex with a ChatGPT
    /// account.` Codex-family models get a minimal successful SSE stream.
    struct StrictChatGptBackend;

    impl Respond for StrictChatGptBackend {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&request.body).expect("JSON request body");
            let model = body["model"].as_str().unwrap_or_default().to_string();
            if !crate::config::openai_codex_route_can_serve_model(&model) {
                return ResponseTemplate::new(400).set_body_string(format!(
                    "{{\"detail\":\"The '{model}' model is not supported when using Codex with a ChatGPT account.\"}}"
                ));
            }
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string("data: [DONE]\n\n")
        }
    }

    #[tokio::test]
    async fn strict_chatgpt_backend_rejects_foreign_child_models_with_observed_400() {
        // Reproduces the live sub-agent failure: a fleet profile pinned
        // `deepseek-v4-pro` while the active provider was ChatGPT Codex
        // OAuth, and the backend answered with the 400 below. The guard
        // fixes keep such ids from ever reaching the wire; this test pins
        // the wire-level failure shape so the reproduction stays honest.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(CODEX_RESPONSES_PATH))
            .respond_with(StrictChatGptBackend)
            .mount(&server)
            .await;

        let client = {
            let _env_lock = crate::test_support::lock_test_env();
            let _codex_token =
                crate::test_support::EnvVarGuard::set("OPENAI_CODEX_ACCESS_TOKEN", "test-token");
            let _legacy_codex_token =
                crate::test_support::EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
            DeepSeekClient::new(&test_codex_config(&server)).unwrap()
        };

        // Foreign model → the exact observed 400.
        let mut request = minimal_responses_request();
        request.model = "deepseek-v4-pro".to_string();
        let err = match client.handle_responses_stream(request).await {
            Ok(_) => panic!("foreign models must be refused by the ChatGPT backend"),
            Err(err) => err,
        };
        let message = format!("{err:#}");
        assert!(
            message.contains(
                "The 'deepseek-v4-pro' model is not supported when using Codex with a ChatGPT account."
            ),
            "{message}"
        );

        // The parent's own model family passes the same strict backend.
        for model in ["gpt-5.5", "gpt-5.5-codex"] {
            let mut request = minimal_responses_request();
            request.model = model.to_string();
            let mut stream = client
                .handle_responses_stream(request)
                .await
                .unwrap_or_else(|err| panic!("{model} must be accepted: {err:#}"));
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while let Some(event) = stream.next().await {
                    event.unwrap();
                }
            })
            .await
            .expect("codex-family stream should finish after [DONE]");
        }

        // And the guard the fix installs agrees with the backend: every id a
        // child could now resolve to on the codex route is one the backend
        // serves; the foreign ids are known-foreign before the wire.
        assert!(crate::config::model_is_foreign_to_provider(
            crate::config::ApiProvider::OpenaiCodex,
            "deepseek-v4-pro"
        ));
        assert!(crate::config::model_is_foreign_to_provider(
            crate::config::ApiProvider::OpenaiCodex,
            "glm-5.2"
        ));
        assert!(!crate::config::model_is_foreign_to_provider(
            crate::config::ApiProvider::OpenaiCodex,
            "gpt-5.5"
        ));
    }

    #[tokio::test]
    async fn responses_stream_fails_fast_on_non_retryable_provider_error() {
        let server = MockServer::start().await;
        let attempts = Arc::new(AtomicUsize::new(0));
        Mock::given(method("POST"))
            .and(path(CODEX_RESPONSES_PATH))
            .respond_with(AlwaysError {
                attempts: Arc::clone(&attempts),
                status: 403,
                body: "<html><title>Access Denied</title><body>Security alert. Contact support. Ray ID 1234abcd.</body></html>",
            })
            .mount(&server)
            .await;

        let client = {
            let _env_lock = crate::test_support::lock_test_env();
            let _codex_token =
                crate::test_support::EnvVarGuard::set("OPENAI_CODEX_ACCESS_TOKEN", "test-token");
            let _legacy_codex_token =
                crate::test_support::EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
            DeepSeekClient::new(&test_codex_config(&server)).unwrap()
        };

        let err = match client
            .handle_responses_stream(minimal_responses_request())
            .await
        {
            Ok(_) => panic!("non-retryable Responses errors should fail fast"),
            Err(err) => err,
        };

        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        let message = format!("{err:#}");
        assert!(
            message.contains("Responses API request failed"),
            "{message}"
        );
        assert!(message.contains("OpenAI Codex"), "{message}");
        assert!(message.contains("Access Denied"), "{message}");
        assert!(
            message.contains("blocked before it reached the model"),
            "{message}"
        );
        // #3884: the structured LlmError must stay downcastable through the
        // context layers so sub-agent failure records can classify it.
        assert!(
            err.downcast_ref::<crate::llm_client::LlmError>().is_some(),
            "LlmError should survive the anyhow chain"
        );
    }

    #[test]
    fn codex_reasoning_effort_uses_responses_labels() {
        assert_eq!(codex_responses_reasoning_effort("max"), Some("xhigh"));
        assert_eq!(codex_responses_reasoning_effort("maximum"), Some("xhigh"));
        assert_eq!(codex_responses_reasoning_effort("xhigh"), Some("xhigh"));
        assert_eq!(codex_responses_reasoning_effort("ultracode"), Some("xhigh"));
        assert_eq!(codex_responses_reasoning_effort("high"), Some("high"));
        assert_eq!(codex_responses_reasoning_effort("medium"), Some("medium"));
        assert_eq!(codex_responses_reasoning_effort("minimal"), Some("low"));
        assert_eq!(codex_responses_reasoning_effort("auto"), Some("medium"));
        assert_eq!(codex_responses_reasoning_effort("off"), Some("low"));
    }

    #[test]
    fn codex_responses_body_uses_responses_reasoning_not_deepseek_thinking() {
        let request = MessageRequest {
            model: "gpt-5.5".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                    cache_control: None,
                }],
            }],
            max_tokens: 128,
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: Some("max".to_string()),
            stream: None,
            temperature: None,
            top_p: None,
        };

        let body = build_responses_body(&request);

        assert_eq!(
            body.pointer("/reasoning/effort").and_then(Value::as_str),
            Some("xhigh")
        );
        assert_eq!(
            body.pointer("/reasoning/summary").and_then(Value::as_str),
            Some("auto")
        );
        assert!(body.get("thinking").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn responses_failed_event_reports_nested_error() {
        let event = json!({
            "type": "response.failed",
            "response": {
                "id": "resp_123",
                "error": {
                    "code": "rate_limit_exceeded",
                    "message": "Please retry later"
                }
            }
        });

        let (code, message) = responses_event_error_details(&event);

        assert_eq!(code, "rate_limit_exceeded");
        assert_eq!(message, "Please retry later");
    }

    #[test]
    fn responses_incomplete_event_reports_reason() {
        let event = json!({
            "type": "response.incomplete",
            "response": {
                "id": "resp_123",
                "status": "incomplete",
                "error": null,
                "incomplete_details": {
                    "reason": "content_filter"
                }
            }
        });

        let (code, message) = responses_event_error_details(&event);

        assert_eq!(code, "content_filter");
        assert_eq!(message, "response incomplete: content_filter");
    }

    #[test]
    fn parse_responses_usage_derives_cache_miss_and_reasoning() {
        let usage = json!({
            "input_tokens": 1000,
            "output_tokens": 200,
            "input_tokens_details": { "cached_tokens": 600 },
            "output_tokens_details": { "reasoning_tokens": 120 }
        });

        let parsed = parse_responses_usage(&usage);

        assert_eq!(parsed.input_tokens, 1000);
        assert_eq!(parsed.output_tokens, 200);
        assert_eq!(parsed.prompt_cache_hit_tokens, Some(600));
        // Cache-miss is derived as input minus the cached hit when cached > 0.
        assert_eq!(parsed.prompt_cache_miss_tokens, Some(400));
        // Reasoning surfaces from output_tokens_details (Responses dialect).
        assert_eq!(parsed.reasoning_tokens, Some(120));

        // Without cached/reasoning details, the derived fields stay None.
        let bare = json!({ "input_tokens": 1000, "output_tokens": 200 });
        let parsed_bare = parse_responses_usage(&bare);
        assert_eq!(parsed_bare.prompt_cache_hit_tokens, None);
        assert_eq!(parsed_bare.prompt_cache_miss_tokens, None);
        assert_eq!(parsed_bare.reasoning_tokens, None);
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
        assert_eq!(input[0]["name"], "checklist_write");
        assert_eq!(input[1]["type"], "function_call_output");
        assert_eq!(input[1]["call_id"], "call_abc");
        assert_eq!(input[1]["output"], "<6 items>");
    }

    #[test]
    fn responses_input_keeps_system_role_messages() {
        // Sub-agent fork context injects mid-conversation `role:"system"`
        // messages (fork state + subagent context). The Responses converter
        // used to drop them silently, so forked codex children lost context
        // that Chat-Completions children kept.
        let request = MessageRequest {
            model: "gpt-5.5".to_string(),
            messages: vec![
                Message {
                    role: "system".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "<codewhale:fork_state>state</codewhale:fork_state>".to_string(),
                        cache_control: None,
                    }],
                },
                Message {
                    role: "user".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "do the task".to_string(),
                        cache_control: None,
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

        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "system");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(
            input[0]["content"][0]["text"],
            "<codewhale:fork_state>state</codewhale:fork_state>"
        );
        assert_eq!(input[1]["role"], "user");
    }

    #[test]
    fn responses_input_encodes_tool_call_names() {
        let request = MessageRequest {
            model: "gpt-5.5".to_string(),
            messages: vec![Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "call_abc|fc_123".to_string(),
                    name: "web.run".to_string(),
                    input: json!({}),
                    caller: None,
                }],
            }],
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
        assert_eq!(input[0]["name"], to_api_tool_name("web.run"));
    }

    #[test]
    fn responses_function_tool_sanitizes_root_composition_schema() {
        let tool = Tool {
            tool_type: None,
            name: "web.run".to_string(),
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

        assert_eq!(payload["name"], to_api_tool_name("web.run"));
        assert_eq!(parameters["type"], "object");
        assert!(parameters.get("oneOf").is_none());
        assert!(parameters.get("anyOf").is_none());
        assert!(parameters.get("allOf").is_none());
        assert!(parameters.get("enum").is_none());
        assert!(parameters.get("not").is_none());
        assert!(parameters["properties"].get("patch").is_some());
        assert!(parameters["properties"].get("changes").is_some());
        assert_eq!(
            payload["description"],
            "Apply patch\n\nExactly one of these parameter groups must be provided: `changes` | `patch`."
        );
        assert!(tool.input_schema.get("oneOf").is_some());
    }

    #[test]
    fn responses_function_tool_trims_description_before_constraint_note() {
        let tool = Tool {
            tool_type: None,
            name: "apply_patch".to_string(),
            description: "Apply patch\n".to_string(),
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

        assert_eq!(
            payload["description"],
            "Apply patch\n\nExactly one of these parameter groups must be provided: `changes` | `patch`."
        );
    }

    #[test]
    fn responses_function_tool_leaves_description_unchanged_without_constraint_note() {
        let tool = Tool {
            tool_type: None,
            name: "lookup".to_string(),
            description: "Lookup".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                }
            }),
            allowed_callers: None,
            defer_loading: None,
            input_examples: None,
            strict: None,
            cache_control: None,
        };

        let payload = tool_to_responses_function(&tool);

        assert_eq!(payload["description"], "Lookup");
    }
}
