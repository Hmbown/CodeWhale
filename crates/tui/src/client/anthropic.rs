//! Native Anthropic Messages API adapter (#3014).
//!
//! CodeWhale's internal wire types are already Anthropic-shaped (the harness
//! speaks Messages internally and translates *out* to OpenAI dialects), so
//! this adapter is mostly native serialization plus an SSE pass-through:
//! `StreamEvent` deserializes Anthropic's `message_start` /
//! `content_block_*` / `message_delta` / `message_stop` / `ping` events
//! directly. What the adapter adds on top:
//!
//! - request shaping: adaptive thinking + `output_config.effort` from
//!   CodeWhale's `reasoning_effort` tiers, sampling-parameter rules for
//!   models that reject them, and `cache_control` breakpoint placement
//!   aligned with the prefix-zone model in `prefix_cache.rs`;
//! - usage normalization (#2961): `prompt_cache_hit_tokens` comes from
//!   `cache_read_input_tokens`, `prompt_cache_miss_tokens` is `input_tokens`
//!   plus `cache_creation_input_tokens`, and the normalized `input_tokens`
//!   is the sum of all three (total prompt, the DeepSeek convention);
//! - signed-thinking handling: `signature_delta` is captured into
//!   [`crate::models::Delta::SignatureDelta`] and assistant thinking blocks
//!   replay verbatim (signature included); unsigned thinking blocks are
//!   dropped from replay because the API rejects them.
//!
//! Modeled on `client/responses.rs` (separate file per dialect, no protocol
//! hacks in the shared paths).

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::llm_client::StreamEventBox;
use crate::logging;
use crate::models::{ContentBlock, MessageRequest, MessageResponse, StreamEvent, Usage};

use super::{DeepSeekClient, ERROR_BODY_MAX_BYTES, bounded_error_text};

/// Maximum `cache_control` breakpoints Anthropic accepts per request.
const MAX_CACHE_BREAKPOINTS: usize = 4;

impl DeepSeekClient {
    /// Build the native Messages API request body from a [`MessageRequest`].
    pub(super) fn build_anthropic_body(&self, request: &MessageRequest, stream: bool) -> Value {
        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "stream": stream,
        });

        if let Some(system) = request.system.as_ref() {
            body["system"] = match system {
                crate::models::SystemPrompt::Text(text) => json!(text),
                crate::models::SystemPrompt::Blocks(blocks) => json!(
                    blocks
                        .iter()
                        .map(|block| {
                            let mut value = json!({
                                "type": "text",
                                "text": block.text,
                            });
                            if let Some(cache) = block.cache_control.as_ref() {
                                value["cache_control"] = json!({ "type": cache.cache_type });
                            }
                            value
                        })
                        .collect::<Vec<_>>()
                ),
            };
        }

        body["messages"] = json!(
            request
                .messages
                .iter()
                .filter_map(message_to_anthropic)
                .collect::<Vec<_>>()
        );

        if let Some(tools) = request.tools.as_ref()
            && !tools.is_empty()
        {
            body["tools"] = json!(
                tools
                    .iter()
                    .map(|tool| {
                        let mut value = json!({
                            "name": tool.name,
                            "description": tool.description,
                            "input_schema": tool.input_schema,
                        });
                        if let Some(strict) = tool.strict {
                            value["strict"] = json!(strict);
                        }
                        if let Some(cache) = tool.cache_control.as_ref() {
                            value["cache_control"] = json!({ "type": cache.cache_type });
                        }
                        value
                    })
                    .collect::<Vec<_>>()
            );
        }

        if let Some(tool_choice) = request.tool_choice.as_ref() {
            body["tool_choice"] = anthropic_tool_choice(tool_choice);
        }

        // Thinking + effort shaping. "off" omits thinking entirely; any other
        // tier enables adaptive thinking, with `output_config.effort` only on
        // models the capability matrix marks as thinking-capable.
        let thinking_capable = crate::models::model_supports_reasoning(&request.model);
        let effort = request
            .reasoning_effort
            .as_deref()
            .map(|raw| raw.trim().to_ascii_lowercase());
        match effort.as_deref() {
            Some("off" | "disabled" | "none" | "false") => {}
            Some(level) if thinking_capable => {
                body["thinking"] = json!({ "type": "adaptive" });
                let mapped = match level {
                    "low" | "minimal" => "low",
                    "medium" | "mid" => "medium",
                    "max" | "xhigh" | "highest" => "max",
                    _ => "high",
                };
                body["output_config"] = json!({ "effort": mapped });
            }
            None if thinking_capable => {
                body["thinking"] = json!({ "type": "adaptive" });
            }
            _ => {}
        }

        // Sampling parameters: Claude 4.7+ rejects temperature/top_p
        // entirely; earlier models reject the two together. Send at most one
        // (temperature wins), or neither for models that forbid them.
        if !anthropic_model_rejects_sampling(&request.model) {
            if let Some(temperature) = request.temperature {
                body["temperature"] = json!(temperature);
            } else if let Some(top_p) = request.top_p {
                body["top_p"] = json!(top_p);
            }
        }

        apply_anthropic_cache_breakpoints(&mut body);
        body
    }

    async fn send_anthropic_request(&self, body: &Value) -> Result<reqwest::Response> {
        let url = anthropic_messages_url(&self.base_url);
        self.wait_for_rate_limit().await;
        let response = self
            .http_client
            .post(&url)
            .header("Accept", "text/event-stream")
            .json(body)
            .send()
            .await
            .context("Anthropic Messages API request failed")?;

        let status = response.status();
        if !status.is_success() {
            let raw = bounded_error_text(response, ERROR_BODY_MAX_BYTES).await;
            let (error_type, message) = parse_anthropic_error_envelope(&raw);
            self.mark_request_failure(&format!("anthropic status={status}"))
                .await;
            anyhow::bail!("Anthropic API error (HTTP {status} {error_type}): {message}");
        }
        self.mark_request_success().await;
        Ok(response)
    }

    /// Handle a streaming Messages API request.
    pub(super) async fn handle_anthropic_stream(
        &self,
        request: MessageRequest,
    ) -> Result<StreamEventBox> {
        let body = self.build_anthropic_body(&request, true);
        let response = self.send_anthropic_request(&body).await?;

        let stream_idle_timeout = self.stream_idle_timeout;
        let byte_stream = response.bytes_stream();

        let stream = async_stream::stream! {
            use futures_util::StreamExt;

            // Raw byte buffer: decode only COMPLETE lines so a multi-byte
            // UTF-8 char (CJK/emoji) split across two network reads is never
            // corrupted to U+FFFD. Line boundaries ('\n') are ASCII and can
            // never fall inside a multi-byte sequence. (Mirrors chat.rs.)
            let mut buffer: Vec<u8> = Vec::new();
            tokio::pin!(byte_stream);

            loop {
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

                while let Some(line) = super::take_sse_line(&mut buffer) {

                    // `event:` lines are redundant (the data payload carries
                    // `type`) and comment/heartbeat lines are ignorable.
                    let Some(data) = super::extract_sse_data_value(&line) else {
                        continue;
                    };

                    match convert_anthropic_sse_data(data) {
                        Some(Ok(StreamEvent::Error { error })) => {
                            let (error_type, message) = anthropic_error_fields(&error);
                            yield Err(anyhow::anyhow!(
                                "Anthropic stream error ({error_type}): {message}"
                            ));
                            return;
                        }
                        Some(Ok(event)) => {
                            let is_stop = matches!(event, StreamEvent::MessageStop);
                            yield Ok(event);
                            if is_stop {
                                return;
                            }
                        }
                        Some(Err(e)) => {
                            logging::warn(format!("Failed to parse Anthropic SSE event: {e}"));
                        }
                        None => {}
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    /// Handle a non-streaming Messages API request.
    pub(super) async fn handle_anthropic_message(
        &self,
        request: MessageRequest,
    ) -> Result<MessageResponse> {
        let body = self.build_anthropic_body(&request, false);
        let response = self.send_anthropic_request(&body).await?;
        let mut value: Value = response
            .json()
            .await
            .context("Failed to parse Anthropic Messages response")?;
        if let Some(usage) = value.get_mut("usage") {
            *usage = json!(parse_anthropic_usage(usage));
        }
        serde_json::from_value(value).context("Failed to decode Anthropic Messages response")
    }
}

/// Build the `/v1/messages` endpoint URL, tolerating base URLs that already
/// carry a `/v1` suffix.
fn anthropic_messages_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        format!("{trimmed}/messages")
    } else {
        format!("{trimmed}/v1/messages")
    }
}

/// Models that reject `temperature` / `top_p` outright (Claude 4.7+).
fn anthropic_model_rejects_sampling(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    lower.contains("opus-4-7")
        || lower.contains("opus-4-8")
        || lower.contains("fable")
        || lower.contains("mythos")
}

/// Convert the engine's `tool_choice` value (OpenAI-style string or object)
/// to the Anthropic object form.
fn anthropic_tool_choice(tool_choice: &Value) -> Value {
    match tool_choice.as_str() {
        Some("auto") => json!({ "type": "auto" }),
        Some("none") => json!({ "type": "none" }),
        Some("any" | "required") => json!({ "type": "any" }),
        Some(name) => json!({ "type": "tool", "name": name }),
        None => tool_choice.clone(),
    }
}

/// Convert one internal message to the Anthropic wire shape. Returns `None`
/// when no blocks survive conversion (Anthropic rejects empty content).
fn message_to_anthropic(message: &crate::models::Message) -> Option<Value> {
    let blocks: Vec<Value> = message
        .content
        .iter()
        .filter_map(content_block_to_anthropic)
        .collect();
    if blocks.is_empty() {
        return None;
    }
    Some(json!({ "role": message.role, "content": blocks }))
}

fn content_block_to_anthropic(block: &ContentBlock) -> Option<Value> {
    match block {
        ContentBlock::Text {
            text,
            cache_control,
        } => {
            let mut value = json!({ "type": "text", "text": text });
            if let Some(cache) = cache_control {
                value["cache_control"] = json!({ "type": cache.cache_type });
            }
            Some(value)
        }
        ContentBlock::Thinking {
            thinking,
            signature,
        } => {
            // Anthropic rejects unsigned thinking blocks on replay (and the
            // DeepSeek-era "(reasoning omitted)" placeholders mean nothing to
            // it), so only signed blocks are replayed — verbatim, signature
            // included.
            signature.as_ref().map(|signature| {
                json!({
                    "type": "thinking",
                    "thinking": thinking,
                    "signature": signature,
                })
            })
        }
        ContentBlock::ToolUse {
            id, name, input, ..
        } => Some(json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input,
        })),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            ..
        } => {
            let mut value = json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content,
            });
            if let Some(is_error) = is_error {
                value["is_error"] = json!(is_error);
            }
            Some(value)
        }
        ContentBlock::ImageUrl { image_url } => Some(json!({
            "type": "image",
            "source": { "type": "url", "url": image_url.url },
        })),
        // Server-tool block types are DeepSeek/internal concepts with no
        // Anthropic client-side wire equivalent.
        ContentBlock::ServerToolUse { .. }
        | ContentBlock::ToolSearchToolResult { .. }
        | ContentBlock::CodeExecutionToolResult { .. } => None,
    }
}

/// Enforce the prefix-zone breakpoint policy (#3014):
/// 1. the last tool in the catalog (or, with no tools, the last system
///    block) — caches the immutable prefix;
/// 2. the last content block of the most recent user turn — caches the
///    append-only history.
///
/// Caller-provided breakpoints are preserved, but the total is capped at
/// [`MAX_CACHE_BREAKPOINTS`] by dropping the earliest markers first (the
/// latest markers cover the longest prefixes).
fn apply_anthropic_cache_breakpoints(body: &mut Value) {
    // Place breakpoint 1: prefer the last tool; otherwise last system block.
    let mut placed_prefix = false;
    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut)
        && let Some(last) = tools.last_mut()
    {
        last["cache_control"] = json!({ "type": "ephemeral" });
        placed_prefix = true;
    }
    if !placed_prefix
        && let Some(system) = body.get_mut("system").and_then(Value::as_array_mut)
        && let Some(last) = system.last_mut()
    {
        last["cache_control"] = json!({ "type": "ephemeral" });
    }

    // Place breakpoint 2: last content block of the latest user message.
    if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut)
        && let Some(last_user) = messages
            .iter_mut()
            .rev()
            .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        && let Some(last_block) = last_user
            .get_mut("content")
            .and_then(Value::as_array_mut)
            .and_then(|blocks| blocks.last_mut())
    {
        last_block["cache_control"] = json!({ "type": "ephemeral" });
    }

    // Cap at MAX_CACHE_BREAKPOINTS in render order (tools → system →
    // messages), dropping the earliest extras.
    let mut marked: Vec<*mut Value> = Vec::new();
    let collect = |value: Option<&mut Value>| {
        let Some(array) = value.and_then(Value::as_array_mut) else {
            return Vec::new();
        };
        array
            .iter_mut()
            .filter(|item| item.get("cache_control").is_some())
            .map(|item| item as *mut Value)
            .collect::<Vec<_>>()
    };
    marked.extend(collect(body.get_mut("tools")));
    marked.extend(collect(body.get_mut("system")));
    if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages.iter_mut() {
            if let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) {
                marked.extend(
                    blocks
                        .iter_mut()
                        .filter(|block| block.get("cache_control").is_some())
                        .map(|block| block as *mut Value),
                );
            }
        }
    }
    if marked.len() > MAX_CACHE_BREAKPOINTS {
        let excess = marked.len() - MAX_CACHE_BREAKPOINTS;
        for pointer in marked.into_iter().take(excess) {
            // SAFETY: the pointers were collected from `body`, which is
            // exclusively borrowed for the duration of this function, and
            // each pointer targets a distinct JSON node.
            unsafe {
                if let Some(map) = (*pointer).as_object_mut() {
                    map.remove("cache_control");
                }
            }
        }
    }
}

/// Convert one SSE `data:` payload into a [`StreamEvent`], normalizing usage
/// objects to the #2961 convention. Returns `None` for ignorable payloads.
fn convert_anthropic_sse_data(data: &str) -> Option<Result<StreamEvent>> {
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut value: Value = match serde_json::from_str(trimmed) {
        Ok(value) => value,
        Err(e) => return Some(Err(anyhow::anyhow!("invalid SSE JSON: {e}"))),
    };

    match value.get("type").and_then(Value::as_str) {
        Some("message_start") => {
            if let Some(usage) = value
                .get_mut("message")
                .and_then(|message| message.get_mut("usage"))
            {
                *usage = json!(parse_anthropic_usage(usage));
            }
        }
        Some("message_delta") => {
            if let Some(usage) = value.get_mut("usage") {
                *usage = json!(parse_anthropic_usage(usage));
            }
        }
        // Tolerate unknown event types (e.g. future additions) silently.
        Some(known)
            if !matches!(
                known,
                "message_start"
                    | "content_block_start"
                    | "content_block_delta"
                    | "content_block_stop"
                    | "message_delta"
                    | "message_stop"
                    | "ping"
                    | "error"
            ) =>
        {
            return None;
        }
        _ => {}
    }

    Some(serde_json::from_value(value).map_err(|e| anyhow::anyhow!("unrecognized SSE event: {e}")))
}

/// Map Anthropic's usage payload onto the normalized [`Usage`] convention
/// (#2961): hit = cache reads, miss = uncached input + cache writes,
/// `input_tokens` = the total prompt across all three.
fn parse_anthropic_usage(usage: &Value) -> Usage {
    let field = |name: &str| {
        usage
            .get(name)
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0)
    };
    let input_raw = field("input_tokens");
    let cache_creation = field("cache_creation_input_tokens");
    let cache_read = field("cache_read_input_tokens");
    let output = field("output_tokens");

    Usage {
        input_tokens: input_raw
            .saturating_add(cache_creation)
            .saturating_add(cache_read),
        output_tokens: output,
        prompt_cache_hit_tokens: Some(cache_read),
        prompt_cache_miss_tokens: Some(input_raw.saturating_add(cache_creation)),
        reasoning_tokens: None,
        reasoning_replay_tokens: None,
        server_tool_use: None,
    }
}

/// Extract `error.type` / `error.message` from an Anthropic error envelope
/// (`{"type":"error","error":{"type":...,"message":...}}`), falling back to
/// the raw body so nothing is swallowed.
fn parse_anthropic_error_envelope(raw: &str) -> (String, String) {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return ("unknown".to_string(), raw.to_string());
    };
    let error = value.get("error").unwrap_or(&value);
    anthropic_error_fields(error)
}

fn anthropic_error_fields(error: &Value) -> (String, String) {
    let error_type = error
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| error.to_string());
    (error_type, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CacheControl, Message, SystemBlock, SystemPrompt, Tool};

    fn request_with(
        model: &str,
        reasoning_effort: Option<&str>,
        temperature: Option<f32>,
        top_p: Option<f32>,
    ) -> MessageRequest {
        MessageRequest {
            model: model.to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                    cache_control: None,
                }],
            }],
            max_tokens: 1024,
            system: Some(SystemPrompt::Blocks(vec![SystemBlock {
                block_type: "text".to_string(),
                text: "be helpful".to_string(),
                cache_control: Some(CacheControl {
                    cache_type: "ephemeral".to_string(),
                }),
            }])),
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: reasoning_effort.map(str::to_string),
            stream: Some(true),
            temperature,
            top_p,
        }
    }

    fn test_client() -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = crate::config::Config {
            provider: Some("anthropic".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                anthropic: crate::config::ProviderConfig {
                    api_key: Some("test-key".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        DeepSeekClient::new(&config).expect("anthropic client constructs")
    }

    #[test]
    fn body_keeps_native_cache_control_on_system_and_tools() {
        let client = test_client();
        let mut request = request_with("claude-sonnet-4-6", Some("high"), None, None);
        request.tools = Some(vec![Tool {
            tool_type: None,
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            input_schema: json!({"type": "object", "additionalProperties": false}),
            allowed_callers: None,
            defer_loading: None,
            input_examples: None,
            strict: Some(true),
            cache_control: None,
        }]);

        let body = client.build_anthropic_body(&request, true);

        assert_eq!(
            body.pointer("/system/0/cache_control/type")
                .and_then(Value::as_str),
            Some("ephemeral"),
            "system cache_control must survive natively: {body}"
        );
        assert_eq!(
            body.pointer("/tools/0/strict").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            body.pointer("/tools/0/cache_control/type")
                .and_then(Value::as_str),
            Some("ephemeral"),
            "breakpoint 1 lands on the last tool: {body}"
        );
        // Breakpoint 2 lands on the latest user turn's last block.
        assert_eq!(
            body.pointer("/messages/0/content/0/cache_control/type")
                .and_then(Value::as_str),
            Some("ephemeral")
        );
    }

    #[test]
    fn body_maps_reasoning_effort_to_adaptive_thinking_and_effort() {
        let client = test_client();

        let body = client.build_anthropic_body(
            &request_with("claude-sonnet-4-6", Some("high"), None, None),
            true,
        );
        assert_eq!(
            body.pointer("/thinking/type").and_then(Value::as_str),
            Some("adaptive")
        );
        assert_eq!(
            body.pointer("/output_config/effort")
                .and_then(Value::as_str),
            Some("high")
        );

        let body = client.build_anthropic_body(
            &request_with("claude-opus-4-8", Some("xhigh"), None, None),
            true,
        );
        assert_eq!(
            body.pointer("/output_config/effort")
                .and_then(Value::as_str),
            Some("max")
        );

        let body = client.build_anthropic_body(
            &request_with("claude-sonnet-4-6", Some("off"), None, None),
            true,
        );
        assert!(body.get("thinking").is_none(), "off omits thinking: {body}");
        assert!(body.get("output_config").is_none());

        // Haiku is not thinking-capable: no thinking, no effort.
        let body = client.build_anthropic_body(
            &request_with("claude-haiku-4-5", Some("high"), None, None),
            true,
        );
        assert!(body.get("thinking").is_none(), "{body}");
        assert!(body.get("output_config").is_none(), "{body}");
    }

    #[test]
    fn body_drops_sampling_params_for_models_that_reject_them() {
        let client = test_client();

        let body = client.build_anthropic_body(
            &request_with("claude-opus-4-8", None, Some(0.7), Some(0.9)),
            true,
        );
        assert!(body.get("temperature").is_none(), "{body}");
        assert!(body.get("top_p").is_none(), "{body}");

        // Older models accept ONE of temperature / top_p (temperature wins).
        let body = client.build_anthropic_body(
            &request_with("claude-sonnet-4-6", None, Some(0.7), Some(0.9)),
            true,
        );
        assert_eq!(
            body.get("temperature").and_then(Value::as_f64),
            Some(f64::from(0.7f32))
        );
        assert!(body.get("top_p").is_none(), "never send both: {body}");
    }

    #[test]
    fn body_replays_signed_thinking_and_drops_unsigned_placeholders() {
        let client = test_client();
        let mut request = request_with("claude-sonnet-4-6", None, None, None);
        request.messages = vec![
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "do the thing".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "signed reasoning".to_string(),
                        signature: Some("sig-abc".to_string()),
                    },
                    ContentBlock::Thinking {
                        thinking: "(reasoning omitted)".to_string(),
                        signature: None,
                    },
                    ContentBlock::ToolUse {
                        id: "toolu_1".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "a.txt"}),
                        caller: None,
                    },
                ],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "toolu_1".to_string(),
                    content: "contents".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];

        let body = client.build_anthropic_body(&request, true);
        let assistant = &body["messages"][1]["content"];
        assert_eq!(assistant.as_array().map(Vec::len), Some(2));
        assert_eq!(
            assistant[0]["signature"].as_str(),
            Some("sig-abc"),
            "signed thinking replays verbatim: {assistant}"
        );
        assert_eq!(assistant[1]["type"].as_str(), Some("tool_use"));
        assert!(
            assistant[1].get("caller").is_none(),
            "internal caller metadata must not reach the wire"
        );
        assert_eq!(
            body["messages"][2]["content"][0]["type"].as_str(),
            Some("tool_result")
        );
    }

    #[test]
    fn breakpoints_are_capped_at_four_dropping_earliest() {
        let client = test_client();
        let mut request = request_with("claude-sonnet-4-6", None, None, None);
        // Five caller-marked user turns + the two placed breakpoints.
        request.messages = (0..5)
            .map(|i| Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: format!("turn {i}"),
                    cache_control: Some(CacheControl {
                        cache_type: "ephemeral".to_string(),
                    }),
                }],
            })
            .collect();

        let body = client.build_anthropic_body(&request, true);
        let mut count = 0;
        if body.pointer("/system/0/cache_control").is_some() {
            count += 1;
        }
        for message in body["messages"].as_array().unwrap() {
            for block in message["content"].as_array().unwrap() {
                if block.get("cache_control").is_some() {
                    count += 1;
                }
            }
        }
        assert!(
            count <= MAX_CACHE_BREAKPOINTS,
            "breakpoints must be capped at {MAX_CACHE_BREAKPOINTS}, got {count}: {body}"
        );
        // The latest user turn keeps its marker (longest prefix coverage).
        assert!(
            body.pointer("/messages/4/content/0/cache_control")
                .is_some(),
            "{body}"
        );
    }

    #[test]
    fn sse_fixture_decodes_text_thinking_signature_and_tool_use() {
        use crate::models::{ContentBlockStart, Delta};

        let events = [
            r#"{"type":"message_start","message":{"id":"msg_01","type":"message","role":"assistant","content":[],"model":"claude-sonnet-4-6","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":3,"cache_creation_input_tokens":2045,"cache_read_input_tokens":18000,"output_tokens":1}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me check"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-xyz"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Reading the file."}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_9","name":"read_file","input":{}}}"#,
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"a.txt\"}"}}"#,
            r#"{"type":"content_block_stop","index":2}"#,
            r#"{"type":"ping"}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":42}}"#,
            r#"{"type":"message_stop"}"#,
        ];

        let decoded: Vec<StreamEvent> = events
            .iter()
            .map(|data| {
                convert_anthropic_sse_data(data)
                    .expect("known event")
                    .expect("decodes")
            })
            .collect();

        // message_start usage normalized to the #2961 convention.
        let StreamEvent::MessageStart { message } = &decoded[0] else {
            panic!("expected MessageStart, got {:?}", decoded[0]);
        };
        assert_eq!(message.usage.input_tokens, 3 + 2045 + 18000);
        assert_eq!(message.usage.prompt_cache_hit_tokens, Some(18000));
        assert_eq!(message.usage.prompt_cache_miss_tokens, Some(3 + 2045));

        assert!(matches!(
            &decoded[1],
            StreamEvent::ContentBlockStart {
                content_block: ContentBlockStart::Thinking { .. },
                ..
            }
        ));
        assert!(matches!(
            &decoded[3],
            StreamEvent::ContentBlockDelta {
                delta: Delta::SignatureDelta { signature },
                ..
            } if signature == "sig-xyz"
        ));
        assert!(matches!(
            &decoded[6],
            StreamEvent::ContentBlockDelta {
                delta: Delta::TextDelta { text },
                ..
            } if text == "Reading the file."
        ));
        let mut tool_json = String::new();
        for event in &decoded {
            if let StreamEvent::ContentBlockDelta {
                delta: Delta::InputJsonDelta { partial_json },
                ..
            } = event
            {
                tool_json.push_str(partial_json);
            }
        }
        assert_eq!(
            serde_json::from_str::<Value>(&tool_json).expect("accumulated tool args parse"),
            json!({"path": "a.txt"})
        );
        assert!(matches!(&decoded[12], StreamEvent::Ping));
        let StreamEvent::MessageDelta { delta, usage } = &decoded[13] else {
            panic!("expected MessageDelta");
        };
        assert_eq!(delta.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(usage.as_ref().map(|u| u.output_tokens), Some(42));
        assert!(matches!(&decoded[14], StreamEvent::MessageStop));
    }

    #[test]
    fn sse_error_event_and_unknown_events_are_handled() {
        let error = convert_anthropic_sse_data(
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        )
        .expect("error event decodes")
        .expect("error event is a StreamEvent");
        let StreamEvent::Error { error } = error else {
            panic!("expected StreamEvent::Error");
        };
        let (error_type, message) = anthropic_error_fields(&error);
        assert_eq!(error_type, "overloaded_error");
        assert_eq!(message, "Overloaded");

        assert!(
            convert_anthropic_sse_data(r#"{"type":"content_block_started_v2","index":0}"#)
                .is_none(),
            "unknown event types are tolerated"
        );
        assert!(convert_anthropic_sse_data("   ").is_none());
    }

    #[test]
    fn usage_mapping_handles_missing_cache_fields() {
        let usage = parse_anthropic_usage(&json!({"input_tokens": 10, "output_tokens": 5}));
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.prompt_cache_hit_tokens, Some(0));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(10));
    }

    #[test]
    fn error_envelope_parses_type_and_message() {
        let (error_type, message) = parse_anthropic_error_envelope(
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"Too many requests"},"request_id":"req_1"}"#,
        );
        assert_eq!(error_type, "rate_limit_error");
        assert_eq!(message, "Too many requests");

        let (error_type, message) = parse_anthropic_error_envelope("upstream blew up");
        assert_eq!(error_type, "unknown");
        assert_eq!(message, "upstream blew up");
    }

    #[test]
    fn messages_url_tolerates_v1_suffix() {
        assert_eq!(
            anthropic_messages_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            anthropic_messages_url("https://api.anthropic.com/"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            anthropic_messages_url("https://gateway.example/v1"),
            "https://gateway.example/v1/messages"
        );
        assert_eq!(
            anthropic_messages_url("https://api.deepseek.com/anthropic"),
            "https://api.deepseek.com/anthropic/v1/messages"
        );
    }
}
