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

use crate::config::ApiProvider;
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
pub(super) const CODEX_RESPONSES_PATH: &str = "/codex/responses";

/// Build the Responses API request body from a `MessageRequest`.
#[cfg(test)]
pub(super) fn build_responses_body(request: &MessageRequest) -> Value {
    build_responses_body_for_provider(request, ApiProvider::OpenaiCodex)
}

/// Build a provider-aware Responses API request body.
///
/// DeepSeek-V4-Flash-0731 implements the Responses wire shape but is stateless
/// and exposes plain reasoning text rather than OpenAI encrypted summaries.
/// Keep those exact-route differences here instead of leaking them into the
/// provider-neutral message model.
pub(super) fn build_responses_body_for_provider(
    request: &MessageRequest,
    provider: ApiProvider,
) -> Value {
    let is_deepseek = matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN);
    let model = &request.model;
    let mut body = json!({
        "model": model,
        "stream": true,
    });
    if !is_deepseek {
        body["store"] = json!(false);
    }
    if is_deepseek {
        if request.max_tokens > 0 {
            body["max_output_tokens"] = json!(request.max_tokens);
        }
        if let Some(temperature) = request.temperature {
            body["temperature"] = json!(temperature);
        }
        if let Some(top_p) = request.top_p {
            body["top_p"] = json!(top_p);
        }
    }

    // Instructions (system prompt). The Codex Responses backend rejects
    // requests without instructions, so fall back to a minimal system
    // prompt when the caller did not supply one.
    let instructions = system_to_instructions(request.system.clone())
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| "You are a helpful assistant.".to_string());
    body["instructions"] = json!(instructions);

    // Convert messages to Responses input items.
    let input = convert_messages_to_responses_input(request, is_deepseek);
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
        && let Some(effort) = responses_reasoning_effort(raw, is_deepseek)
    {
        body["reasoning"] = if is_deepseek {
            json!({ "effort": effort })
        } else {
            json!({
                "effort": effort,
                "summary": "auto",
            })
        };
    }

    // OpenAI Codex can replay encrypted reasoning. DeepSeek exposes plain
    // `reasoning_text` and does not support `include`.
    if !is_deepseek {
        body["include"] = json!(["reasoning.encrypted_content"]);
    }

    body
}

impl DeepSeekClient {
    /// Handle a streaming Responses API request for the OpenAI Codex provider.
    pub(super) async fn handle_responses_stream(
        &self,
        prepared: &super::PreparedOutboundRequest,
    ) -> Result<StreamEventBox> {
        // Body, endpoint, and route shape all come from the shared
        // prepared-request seam (`prepare_outbound_request`).
        let body = &prepared.body;
        let is_codex = prepared.endpoint.shape == super::RouteShape::CodexResponses;
        let url = prepared.endpoint.url.clone();
        // The synthetic MessageStart below is emitted from inside the stream
        // closure, which outlives `prepared`. Clone the wire model — the id
        // actually placed on the body by the shared seam, after route
        // remapping — rather than borrowing the request that no longer exists
        // at this layer.
        let wire_model = prepared.wire_model.clone();

        // The bearer Authorization header is already installed as a default
        // header on both the dual and the HTTP/1.1 twin client (resolved from
        // the Codex OAuth access token), so it must not be set again here or
        // it would be duplicated. The ChatGPT backend additionally requires
        // the account id and the experimental Responses beta opt-in.
        //
        // The open itself goes through the shared stream-entry transport
        // policy: bounded header wait, policy-selected client, and at most
        // one HTTP/1.1 fallback retry on a classified H2 header stall. The
        // pre-existing provider retry loop (rate limit / transient upstream)
        // stays inside each open attempt, before any stream body exists.
        let account_id = self.codex_account_id.clone();
        let request_body =
            serde_json::to_vec(&body).context("Failed to serialize Responses API request body")?;
        let open_req = super::stream_entry::StreamOpenRequest::new(
            super::stream_entry::stream_open_timeout(),
            self.stream_idle_timeout,
        );
        let response = super::stream_entry::open_sse_response(&open_req, |policy| {
            let url = url.clone();
            let account_id = account_id.clone();
            let request_body = request_body.clone();
            async move {
                let client = super::stream_entry::client_for_policy(
                    &self.http_client,
                    self.http1_fallback_client(),
                    policy,
                );
                self.send_with_retry(|| {
                    let mut builder = client
                        .post(&url)
                        .header("Content-Type", "application/json")
                        .header("Accept", "text/event-stream");
                    if is_codex {
                        builder = builder
                            .header("OpenAI-Beta", "responses=experimental")
                            .header("originator", "codex_cli_rs");
                        if let Some(account_id) = &account_id {
                            builder = builder.header("chatgpt-account-id", account_id);
                        }
                    }
                    builder.body(request_body.clone())
                })
                .await
                .context("Responses API request failed")
            }
        })
        .await?;

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
                    model: wire_model.clone(),
                    stop_reason: None,
                    stop_sequence: None,
                    container: None,
                    usage: Usage::default(),
                },
            });

            let mut current_block_index: Option<u32> = None;
            // Whether reasoning text has already been emitted for the current
            // reasoning block. Used to insert a paragraph break between
            // consecutive summary parts, which the wire protocol delivers
            // back-to-back with no separator.
            let mut reasoning_text_emitted = false;
            let mut saw_tool_call = false;
            let mut usage_data: Option<Usage> = None;
            // Raw byte buffer: decode only COMPLETE lines so a multi-byte
            // UTF-8 char split across two network reads is never corrupted
            // to U+FFFD (line boundaries are ASCII). Mirrors chat.rs.
            let mut buffer: Vec<u8> = Vec::new();
            let mut done = false;
            let mut content_block_counter: u32 = 0;
            let stream_start = std::time::Instant::now();
            let mut last_chunk_at = std::time::Instant::now();
            let mut bytes_received: usize = 0;

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
                        yield Err(anyhow::anyhow!(super::stream_entry::idle_timeout_message(
                            stream_idle_timeout,
                            bytes_received,
                            stream_start.elapsed(),
                            last_chunk_at.elapsed(),
                        )));
                        return;
                    }
                };

                bytes_received += chunk.len();
                last_chunk_at = std::time::Instant::now();
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
                                            reasoning_text_emitted = false;
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
                                    if !delta_text.is_empty() {
                                        reasoning_text_emitted = true;
                                    }
                                    yield Ok(StreamEvent::ContentBlockDelta {
                                        index: idx,
                                        delta: Delta::ThinkingDelta {
                                            thinking: delta_text.to_string(),
                                        },
                                    });
                                }
                            }
                            "response.reasoning_summary_part.added" => {
                                // Consecutive summary parts arrive with no
                                // separator in the text deltas, so without a
                                // boundary they concatenate as
                                // "…done.**Next Phase**…". Insert a paragraph
                                // break before every part after the first.
                                if reasoning_text_emitted
                                    && let Some(idx) = current_block_index
                                {
                                    yield Ok(StreamEvent::ContentBlockDelta {
                                        index: idx,
                                        delta: Delta::ThinkingDelta {
                                            thinking: "\n\n".to_string(),
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
                            "response.completed" | "response.incomplete" => {
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
                                // DeepSeek terminates semantic Responses
                                // streams with this event and deliberately does
                                // not send `data: [DONE]`.
                                done = true;
                            }
                            "error" | "response.failed" => {
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
        prepared: &super::PreparedOutboundRequest,
    ) -> Result<MessageResponse> {
        use futures_util::StreamExt;

        let model = prepared.wire_model.clone();
        let mut stream = self.handle_responses_stream(prepared).await?;

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
fn convert_messages_to_responses_input(request: &MessageRequest, is_deepseek: bool) -> Vec<Value> {
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
                            items.push(if is_deepseek {
                                json!({
                                    "type": "reasoning",
                                    "content": [{
                                        "type": "reasoning_text",
                                        "text": thinking,
                                    }],
                                })
                            } else {
                                json!({
                                    "type": "reasoning",
                                    "summary": [{
                                        "type": "summary_text",
                                        "text": thinking,
                                    }],
                                })
                            });
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

fn responses_reasoning_effort(raw: &str, is_deepseek: bool) -> Option<&'static str> {
    if !is_deepseek {
        return codex_responses_reasoning_effort(raw);
    }
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" | "disabled" | "none" | "false" | "minimal" | "low" => Some("low"),
        "xhigh" | "max" | "maximum" | "ultracode" => Some("max"),
        // DeepSeek maps both low and medium to its normal high tier in
        // thinking mode. Preserve explicit low for Codex compatibility, and
        // normalize every remaining/automatic tier to a documented value.
        _ => Some("high"),
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
        .map_or(0, super::saturating_u32);
    let output = val
        .get("output_tokens")
        .and_then(|v| v.as_u64())
        .map_or(0, super::saturating_u32);
    // Cache telemetry arrives in two dialects. DeepSeek's Responses payload
    // uses the same top-level `prompt_cache_hit_tokens` /
    // `prompt_cache_miss_tokens` fields as its Chat-Completions endpoint,
    // while OpenAI-style payloads nest `cached_tokens` under
    // `input_tokens_details`. Prefer the top-level hit, falling back to the
    // nested form when the payload only reports that.
    let nested_cached_tokens = val
        .get("input_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_u64());
    let prompt_cache_hit_tokens = val
        .get("prompt_cache_hit_tokens")
        .and_then(|v| v.as_u64())
        .or(nested_cached_tokens)
        .map(super::saturating_u32);
    // DeepSeek reports the miss explicitly; otherwise mirror the
    // Chat-Completions parser: derive the miss as input minus the cached hit
    // when the payload reported cached input tokens. Responses nests
    // reasoning under `output_tokens_details` (not `completion_tokens_details`).
    let prompt_cache_miss_tokens = val
        .get("prompt_cache_miss_tokens")
        .and_then(|v| v.as_u64())
        .map(super::saturating_u32)
        .or_else(|| prompt_cache_hit_tokens.map(|hit| input.saturating_sub(hit)));
    // Cache-creation tokens, kept as their own class so pricing can apply the
    // write rate where the provider publishes one. DeepSeek-style payloads
    // nest these under `input_tokens_details`; accept a top-level spelling
    // too for providers that flatten the object.
    let prompt_cache_write_tokens = val
        .get("prompt_cache_write_tokens")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            val.get("input_tokens_details")
                .and_then(|d| d.get("cache_write_tokens"))
                .and_then(|v| v.as_u64())
        })
        .map(super::saturating_u32);
    // `output_tokens` is already the total billable completion count, with
    // reasoning a subset of it. A payload reporting more reasoning than output
    // violates that, so the value is rejected as invalid telemetry rather than
    // being trusted or turned into extra billable output (#4318).
    let reasoning_tokens = val
        .get("output_tokens_details")
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(|v| v.as_u64())
        .map(super::saturating_u32)
        .filter(|reasoning| *reasoning <= output);
    // `input_tokens` stays the provider-reported *total* (cache-hit + miss +
    // write + uncategorized): `token_usage_for_pricing` partitions it into
    // billable classes and the context budget measures the window with it.
    // Reducing it here to miss-only would double-subtract at those surfaces.
    Usage {
        input_tokens: input,
        output_tokens: output,
        prompt_cache_hit_tokens,
        prompt_cache_miss_tokens,
        prompt_cache_write_tokens,
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
            .handle_responses_stream(
                &client
                    .prepare_outbound_request(minimal_responses_request(), true)
                    .expect("responses request prepares"),
            )
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
            .handle_responses_stream(
                &client
                    .prepare_outbound_request(minimal_responses_request(), true)
                    .expect("responses request prepares"),
            )
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
    async fn responses_stream_retries_upstream_499_before_streaming() {
        let server = MockServer::start().await;
        let attempts = Arc::new(AtomicUsize::new(0));
        Mock::given(method("POST"))
            .and(path(CODEX_RESPONSES_PATH))
            .respond_with(RetryThenSuccess {
                attempts: Arc::clone(&attempts),
                retry_status: 499,
                retry_body: "upstream request cancelled",
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
            .handle_responses_stream(
                &client
                    .prepare_outbound_request(minimal_responses_request(), true)
                    .expect("responses request prepares"),
            )
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
    async fn responses_stream_finishes_on_semantic_terminal_event_without_done_marker() {
        let server = MockServer::start().await;
        let sse_body = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"status\":\"in_progress\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path(CODEX_RESPONSES_PATH))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "text/event-stream")
                    .set_body_string(sse_body),
            )
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
            .handle_responses_stream(
                &client
                    .prepare_outbound_request(minimal_responses_request(), true)
                    .expect("responses request prepares"),
            )
            .await
            .expect("semantic Responses stream opens");

        let mut saw_stop = false;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(event) = stream.next().await {
                if matches!(event.unwrap(), StreamEvent::MessageStop) {
                    saw_stop = true;
                }
            }
        })
        .await
        .expect("terminal event ends the stream without [DONE]");
        assert!(saw_stop);
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
            .handle_responses_stream(
                &client
                    .prepare_outbound_request(minimal_responses_request(), true)
                    .expect("responses request prepares"),
            )
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
    fn responses_body_serializes_exactly_one_load_skill_definition() {
        // Mirror of the Anthropic contract: the real child catalog fixture
        // maps 1:1 into Responses function tools with one load_skill entry.
        let tools = crate::tools::subagent::kimi_general_child_request_tools_fixture();
        let mut request = minimal_responses_request();
        request.tools = Some(tools);
        let body = build_responses_body(&request);
        let serialized = body["tools"]
            .as_array()
            .expect("tools serialize as an array");
        let load_skills: Vec<_> = serialized
            .iter()
            .filter(|tool| tool["name"] == "load_skill")
            .collect();
        assert_eq!(
            load_skills.len(),
            1,
            "exactly one load_skill definition reaches the Responses wire"
        );
        assert!(
            load_skills[0]["parameters"]["properties"].is_object(),
            "load_skill keeps a valid parameters schema: {}",
            load_skills[0]
        );
    }

    #[tokio::test]
    async fn responses_stream_open_preserves_wire_headers_through_shared_seam() {
        use wiremock::matchers::header;

        let server = MockServer::start().await;
        // Every wire-specific header (SSE accept, Responses beta opt-in,
        // originator, bearer auth from the default headers) must survive the
        // shared stream-entry open path; the mock only answers when all are
        // present.
        Mock::given(method("POST"))
            .and(path(CODEX_RESPONSES_PATH))
            .and(header("Accept", "text/event-stream"))
            .and(header("OpenAI-Beta", "responses=experimental"))
            .and(header("originator", "codex_cli_rs"))
            .and(header("Authorization", "Bearer test-token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "text/event-stream")
                    .set_body_string("data: [DONE]\n\n"),
            )
            .expect(1)
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
            .handle_responses_stream(
                &client
                    .prepare_outbound_request(minimal_responses_request(), true)
                    .expect("responses request prepares"),
            )
            .await
            .expect("stream opens with preserved headers");

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(event) = stream.next().await {
                event.unwrap();
            }
        })
        .await
        .expect("stream should finish after [DONE]");
    }

    #[tokio::test]
    async fn responses_stream_inserts_boundary_between_reasoning_summary_parts() {
        let server = MockServer::start().await;
        let sse_body = concat!(
            "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"reasoning\",\"id\":\"rs_1\"}}\n\n",
            "data: {\"type\":\"response.reasoning_summary_part.added\",\"item_id\":\"rs_1\",\"summary_index\":0,\"part\":{\"type\":\"summary_text\",\"text\":\"\"}}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"partA\"}\n\n",
            "data: {\"type\":\"response.reasoning_summary_part.added\",\"item_id\":\"rs_1\",\"summary_index\":1,\"part\":{\"type\":\"summary_text\",\"text\":\"\"}}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"partB\"}\n\n",
            "data: {\"type\":\"response.output_item.done\"}\n\n",
            "data: [DONE]\n\n",
        );
        Mock::given(method("POST"))
            .and(path(CODEX_RESPONSES_PATH))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "text/event-stream")
                    .set_body_string(sse_body),
            )
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
            .handle_responses_stream(
                &client
                    .prepare_outbound_request(minimal_responses_request(), true)
                    .expect("responses request prepares"),
            )
            .await
            .unwrap();

        let mut thinking = String::new();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(event) = stream.next().await {
                if let StreamEvent::ContentBlockDelta {
                    delta: Delta::ThinkingDelta { thinking: chunk },
                    ..
                } = event.unwrap()
                {
                    thinking.push_str(&chunk);
                }
            }
        })
        .await
        .expect("Responses reasoning stream should finish after [DONE]");

        // The second summary part must be separated from the first by a
        // paragraph break, and no separator may precede the first part.
        assert_eq!(thinking, "partA\n\npartB");
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
    fn deepseek_flash_responses_body_uses_stateless_0731_contract() {
        let mut request = minimal_responses_request();
        request.model = "deepseek-v4-flash".to_string();
        request.reasoning_effort = Some("xhigh".to_string());
        request.temperature = Some(1.0);
        request.top_p = Some(0.95);
        request.messages.insert(
            0,
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Thinking {
                    thinking: "preserve this tool-loop reasoning".to_string(),
                    signature: None,
                }],
            },
        );

        let body = build_responses_body_for_provider(&request, ApiProvider::Deepseek);

        assert_eq!(body["model"], "deepseek-v4-flash");
        assert_eq!(body["max_output_tokens"], 128);
        assert_eq!(body["temperature"], 1.0);
        assert!(
            (body["top_p"].as_f64().expect("top_p number") - 0.95).abs() < 1e-6,
            "{}",
            body["top_p"]
        );
        assert_eq!(body.pointer("/reasoning/effort"), Some(&json!("max")));
        assert!(body.pointer("/reasoning/summary").is_none());
        assert!(body.get("include").is_none());
        assert!(body.get("store").is_none());
        assert_eq!(
            body.pointer("/input/0/content/0/type"),
            Some(&json!("reasoning_text"))
        );
        assert_eq!(
            body.pointer("/input/0/content/0/text"),
            Some(&json!("preserve this tool-loop reasoning"))
        );
    }

    #[test]
    fn deepseek_responses_reasoning_effort_uses_documented_labels() {
        assert_eq!(responses_reasoning_effort("low", true), Some("low"));
        assert_eq!(responses_reasoning_effort("medium", true), Some("high"));
        assert_eq!(responses_reasoning_effort("high", true), Some("high"));
        assert_eq!(responses_reasoning_effort("xhigh", true), Some("max"));
        assert_eq!(responses_reasoning_effort("max", true), Some("max"));
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
    fn parse_responses_usage_saturates_u64_fields() {
        let parsed = parse_responses_usage(&json!({
            "input_tokens": u64::MAX,
            "output_tokens": u64::MAX,
            "input_tokens_details": { "cached_tokens": u64::MAX },
            "output_tokens_details": { "reasoning_tokens": u64::MAX }
        }));
        assert_eq!(parsed.input_tokens, u32::MAX);
        assert_eq!(parsed.output_tokens, u32::MAX);
        assert_eq!(parsed.prompt_cache_hit_tokens, Some(u32::MAX));
        assert_eq!(parsed.prompt_cache_miss_tokens, Some(0));
        assert_eq!(parsed.reasoning_tokens, Some(u32::MAX));
    }

    #[test]
    fn parse_responses_usage_reads_deepseek_top_level_cache_fields() {
        // DeepSeek's Responses dialect reports cache telemetry as top-level
        // `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` with
        // `cache_write_tokens` nested under `input_tokens_details` -- none of
        // which the old parser read (it only looked at
        // `input_tokens_details.cached_tokens`, which DeepSeek leaves unset,
        // so every V4 Flash turn recorded cache_hit = None).
        let usage = json!({
            "input_tokens": 1_000,
            "output_tokens": 200,
            "prompt_cache_hit_tokens": 600,
            "prompt_cache_miss_tokens": 200,
            "input_tokens_details": { "cached_tokens": 999, "cache_write_tokens": 100 },
            "output_tokens_details": { "reasoning_tokens": 120 }
        });

        let parsed = parse_responses_usage(&usage);

        // Top-level DeepSeek fields win over the nested OpenAI-style shape,
        // and the explicit miss is trusted over the derived fallback.
        assert_eq!(parsed.prompt_cache_hit_tokens, Some(600));
        assert_eq!(parsed.prompt_cache_miss_tokens, Some(200));
        assert_eq!(parsed.prompt_cache_write_tokens, Some(100));
        // `input_tokens` remains the provider-reported total; the pricing
        // layer partitions it into hit / miss / write classes.
        assert_eq!(parsed.input_tokens, 1_000);
        assert_eq!(parsed.output_tokens, 200);
        assert_eq!(parsed.reasoning_tokens, Some(120));

        // The parsed fields must reach the pricing classes unchanged: 600 hit
        // at the cache-read rate, 100 write at the creation rate, and the
        // remaining 300 (200 reported miss + 100 uncategorized) at the miss
        // rate -- instead of the pre-fix all-raw-input miss billing.
        let classes = crate::pricing::token_usage_for_pricing(&parsed);
        assert_eq!(classes.input, 300);
        assert_eq!(classes.cache_read, 600);
        assert_eq!(classes.cache_write, 100);
    }

    #[test]
    fn parse_responses_usage_keeps_old_shape_with_cache_write_fallback() {
        // OpenAI-style payloads still parse from `input_tokens_details` alone:
        // hit from `cached_tokens` (fallback), miss derived as input minus
        // hit, and the write class from `cache_write_tokens` when present.
        let usage = json!({
            "input_tokens": 1_000,
            "output_tokens": 200,
            "input_tokens_details": { "cached_tokens": 600, "cache_write_tokens": 100 }
        });

        let parsed = parse_responses_usage(&usage);

        assert_eq!(parsed.input_tokens, 1_000);
        assert_eq!(parsed.prompt_cache_hit_tokens, Some(600));
        assert_eq!(parsed.prompt_cache_miss_tokens, Some(400));
        assert_eq!(parsed.prompt_cache_write_tokens, Some(100));
        assert_eq!(parsed.reasoning_tokens, None);
    }

    /// Regression fixture for the reasoning double-billing bug: a real
    /// Responses usage payload has to survive the whole way into the pricing
    /// conversion without reasoning tokens being charged twice. OpenAI's
    /// `output_tokens` is already the *total* billable completion count, with
    /// `output_tokens_details.reasoning_tokens` a subset of it.
    #[test]
    fn responses_usage_reaches_pricing_conversion_without_double_billing_reasoning() {
        use crate::config::ApiProvider;
        use crate::pricing::{calculate_turn_cost_estimate_for_provider, token_usage_for_pricing};

        let usage = parse_responses_usage(&json!({
            "input_tokens": 10_000,
            "output_tokens": 4_000,
            "total_tokens": 14_000,
            "input_tokens_details": { "cached_tokens": 6_000 },
            "output_tokens_details": { "reasoning_tokens": 3_500 }
        }));

        let classes = token_usage_for_pricing(&usage);
        assert_eq!(classes.output, 4_000, "reasoning must not inflate output");
        assert_eq!(classes.input, 4_000);
        assert_eq!(classes.cache_read, 6_000);
        assert_eq!(classes.cache_write, 0);

        // gpt-5.5: 0.50 cache-read / 5.00 input / 30.00 output per million.
        let cost =
            calculate_turn_cost_estimate_for_provider(ApiProvider::Openai, "gpt-5.5", &usage)
                .expect("direct OpenAI route is priced");
        let expected = 0.006 * 0.50 + 0.004 * 5.00 + 0.004 * 30.00;
        assert!(
            (cost.usd - expected).abs() < 1e-12,
            "expected {expected}, got {}",
            cost.usd
        );

        // The bug charged the 3_500 reasoning tokens a second time at the
        // output rate; assert the difference explicitly so a reintroduction is
        // unambiguous rather than a silent number change.
        let double_billed = expected + 0.0035 * 30.00;
        assert!((cost.usd - double_billed).abs() > 1e-6);
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

        let input = convert_messages_to_responses_input(&request, false);

        assert_eq!(input[0]["type"], "function_call");
        assert_eq!(input[0]["call_id"], "call_abc");
        assert_eq!(input[0]["name"], "checklist_write");
        assert_eq!(input[1]["type"], "function_call_output");
        assert_eq!(input[1]["call_id"], "call_abc");
        assert_eq!(input[1]["output"], "<6 items>");
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

        let input = convert_messages_to_responses_input(&request, false);

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
                    "replace": {"type": "array"},
                    "changes": {"type": "array"}
                },
                "oneOf": [
                    {"required": ["patch"]},
                    {"required": ["replace"]},
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
        assert!(parameters["properties"].get("replace").is_some());
        assert!(parameters["properties"].get("changes").is_some());
        assert_eq!(
            payload["description"],
            "Apply patch\n\nExactly one of these parameter groups must be provided: `changes` | `patch` | `replace`."
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
                    "replace": {"type": "array"},
                    "changes": {"type": "array"}
                },
                "oneOf": [
                    {"required": ["patch"]},
                    {"required": ["replace"]},
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
            "Apply patch\n\nExactly one of these parameter groups must be provided: `changes` | `patch` | `replace`."
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
