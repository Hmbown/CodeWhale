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
//! - usage normalization (#2961 / #4318): `prompt_cache_hit_tokens` comes from
//!   `cache_read_input_tokens`, `prompt_cache_write_tokens` from
//!   `cache_creation_input_tokens`, `prompt_cache_miss_tokens` is the raw
//!   non-cached `input_tokens`, and the normalized `input_tokens` is the sum
//!   of all three (total prompt, the DeepSeek convention);
//! - signed-thinking handling: `signature_delta` is captured into
//!   [`crate::models::Delta::SignatureDelta`] and assistant thinking blocks
//!   replay verbatim (signature included); unsigned thinking blocks are
//!   dropped from replay because the API rejects them.
//!
//! Modeled on `client/responses.rs` (separate file per dialect, no protocol
//! hacks in the shared paths).

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::config::{ApiProvider, wire_model_for_provider_route};
use crate::llm_client::StreamEventBox;
use crate::logging;
use crate::models::{ContentBlock, MessageRequest, MessageResponse, StreamEvent, Usage};
use crate::tools::schema_sanitize;

use super::prepared::WireDialect;
use super::role_placement::{RolePlacement, role_placement};
use super::{DeepSeekClient, ERROR_BODY_MAX_BYTES, bounded_error_text};

/// Maximum `cache_control` breakpoints Anthropic accepts per request.
const MAX_CACHE_BREAKPOINTS: usize = 4;

impl DeepSeekClient {
    /// Build the native Messages API request body from a [`MessageRequest`].
    pub(super) fn build_anthropic_body(&self, request: &MessageRequest, stream: bool) -> Value {
        let model =
            wire_model_for_provider_route(self.api_provider, &self.base_url, &request.model);
        let mut body = json!({
            "model": model,
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

        let mut messages: Vec<Value> = request
            .messages
            .iter()
            .filter_map(message_to_anthropic)
            .collect();
        repair_dangling_tool_uses(&mut messages);
        body["messages"] = Value::Array(messages);

        if let Some(tools) = request.tools.as_ref()
            && !tools.is_empty()
        {
            body["tools"] = json!(
                tools
                    .iter()
                    .map(|tool| {
                        // Sanitize the tool's input_schema the same way the
                        // OpenAI Responses adapter does: strip top-level
                        // oneOf/anyOf/allOf (which Anthropic rejects), merge
                        // alternative properties into the root, and surface
                        // the dropped constraint as a description note so the
                        // model still knows which parameters are expected.
                        let mut schema = tool.input_schema.clone();
                        let constraint_note = schema_sanitize::sanitize_for_responses(&mut schema);
                        let description = match constraint_note {
                            Some(note) if tool.description.trim().is_empty() => note,
                            Some(note) => format!("{}\n\n{}", tool.description.trim(), note),
                            None => tool.description.clone(),
                        };
                        let mut value = json!({
                            "name": tool.name,
                            "description": description,
                            "input_schema": schema,
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

        // Thinking + effort shaping. MiniMax supports adaptive/disabled but
        // not Anthropic's output_config effort field; native Anthropic routes
        // keep the existing effort mapping. Other Messages-compatible
        // gateways (#4978, e.g. Sensenova) only accept the documented
        // enabled/disabled/auto thinking types, so non-native routes get the
        // portable `{"type":"enabled","budget_tokens":N}` shape instead.
        let thinking_capable = crate::models::model_supports_reasoning(&model);
        let is_minimax_provider = self.api_provider == ApiProvider::MinimaxAnthropic;
        let is_minimax = crate::config::is_exact_minimax_anthropic_m3_route(
            self.api_provider,
            &self.base_url,
            &model,
        );
        let is_deepseek = self.api_provider == ApiProvider::DeepseekAnthropic;
        // Model Studio's Anthropic-compatible endpoint documents the portable
        // `{"type":"enabled","budget_tokens":N}` shape AND `{"type":"disabled"}`
        // (alibabacloud.com/help/en/model-studio/anthropic-api-messages), so
        // an explicit "off" can be honored on the wire instead of silently
        // falling through to the server default (which is thinking-ON for the
        // qwen3.x families).
        let is_modelstudio = matches!(
            self.api_provider,
            ApiProvider::ModelstudioTokenPlan
                | ApiProvider::ModelstudioTokenPlanAnthropic
                | ApiProvider::ModelstudioCodingPlan
                | ApiProvider::ModelstudioCodingPlanAnthropic
        );
        // MiniMax's exact M3 route and DeepSeek's Messages dialect both
        // document adaptive support; everything else needs the native host.
        let supports_adaptive =
            is_native_anthropic_base_url(&self.base_url) || is_minimax || is_deepseek;
        let effort = request
            .reasoning_effort
            .as_deref()
            .map(|raw| raw.trim().to_ascii_lowercase());
        match effort.as_deref() {
            _ if is_minimax_provider && !is_minimax => {}
            Some("off" | "disabled" | "none" | "false")
                if (is_minimax || is_deepseek || is_modelstudio) && thinking_capable =>
            {
                // Deliberately includes thinking-only Model Studio models
                // (qwen3.8-max family): unlike the chat dialect's
                // enable_thinking switch, the Messages endpoint documents the
                // portable {"type":"disabled"} shape for them
                // (alibabacloud.com/help/en/model-studio/anthropic-api-messages)
                // — pinned by modelstudio_messages_body_requests_thinking_
                // with_budget. Re-checked 2026-08-04.
                body["thinking"] = json!({ "type": "disabled" });
            }
            Some("off" | "disabled" | "none" | "false") => {}
            Some(level) if thinking_capable && supports_adaptive => {
                body["thinking"] = json!({ "type": "adaptive" });
                if !is_minimax {
                    let mapped = match level {
                        "low" | "minimal" => "low",
                        "medium" | "mid" => "medium",
                        "max" | "xhigh" | "highest" => "max",
                        _ => "high",
                    };
                    body["output_config"] = json!({ "effort": mapped });
                }
            }
            None if thinking_capable && supports_adaptive => {
                body["thinking"] = json!({ "type": "adaptive" });
            }
            _ if thinking_capable => {
                if let Some(budget) = compat_thinking_budget(effort.as_deref(), request.max_tokens)
                {
                    body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
                }
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

    async fn send_anthropic_request(&self, url: &str, body: &Value) -> Result<reqwest::Response> {
        let url = self.messages_transport_url(url);
        self.wait_for_rate_limit().await;
        let response = self
            .http_client
            .post(&url)
            .header("Accept", "text/event-stream")
            .json(body)
            .send()
            .await
            .context("Anthropic Messages API request failed")?;
        self.check_anthropic_response(response).await
    }

    /// Shared status/error-envelope handling for streaming and
    /// non-streaming Messages responses.
    async fn check_anthropic_response(
        &self,
        response: reqwest::Response,
    ) -> Result<reqwest::Response> {
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

    /// Open the streaming Messages request through the shared stream-entry
    /// transport policy: bounded header wait, dual-client selection, and at
    /// most one HTTP/1.1 fallback retry on a classified H2 header stall.
    /// Wire-specific request construction (headers, endpoint, body) stays
    /// here at the adapter edge.
    async fn open_anthropic_stream_response(
        &self,
        url: &str,
        body: &Value,
    ) -> Result<reqwest::Response> {
        let url = self.messages_transport_url(url);
        let open_req = super::stream_entry::StreamOpenRequest::new(
            super::stream_entry::stream_open_timeout(),
            self.stream_idle_timeout,
        );
        let opened = super::stream_entry::open_sse_response(&open_req, |policy| {
            let url = url.clone();
            async move {
                self.wait_for_rate_limit().await;
                let client = super::stream_entry::client_for_policy(
                    &self.http_client,
                    self.http1_fallback_client(),
                    policy,
                );
                client
                    .post(&url)
                    .header("Accept", "text/event-stream")
                    .json(body)
                    .send()
                    .await
                    .context("Anthropic Messages API request failed")
            }
        })
        .await;
        let response = match opened {
            Ok(response) => response,
            Err(err) => {
                self.mark_request_failure(&format!("anthropic stream open: {err}"))
                    .await;
                return Err(err);
            }
        };
        self.check_anthropic_response(response).await
    }

    /// Handle a streaming Messages API request.
    pub(super) async fn handle_anthropic_stream(
        &self,
        prepared: &super::PreparedOutboundRequest,
    ) -> Result<StreamEventBox> {
        // Body and endpoint come from the shared prepared-request seam
        // (`prepare_outbound_request`), never from a second builder.
        let body = &prepared.body;
        let response = self
            .open_anthropic_stream_response(&prepared.endpoint.url, body)
            .await?;

        let stream_idle_timeout = self.stream_idle_timeout;
        let byte_stream = response.bytes_stream();

        let stream = async_stream::stream! {
            use futures_util::StreamExt;

            // Raw byte buffer: decode only COMPLETE lines (or the stream-end
            // tail) via the shared take_sse_line / flush_sse_line helpers so a
            // multi-byte UTF-8 char (CJK/emoji) split across HTTP/2 DATA is
            // never corrupted to U+FFFD. Genuine invalid bytes fail closed.
            let mut buffer: Vec<u8> = Vec::new();
            let stream_start = std::time::Instant::now();
            let mut last_chunk_at = std::time::Instant::now();
            let mut bytes_received: usize = 0;
            let mut ended = false;
            tokio::pin!(byte_stream);

            loop {
                if !ended {
                    match tokio::time::timeout(stream_idle_timeout, byte_stream.next()).await {
                        Ok(Some(Ok(chunk))) => {
                            bytes_received += chunk.len();
                            last_chunk_at = std::time::Instant::now();
                            buffer.extend_from_slice(&chunk);
                        }
                        Ok(Some(Err(e))) => {
                            yield Err(anyhow::anyhow!("Stream read error: {e}"));
                            return;
                        }
                        Ok(None) => ended = true,
                        Err(_) => {
                            yield Err(anyhow::anyhow!(super::stream_entry::idle_timeout_message(
                                stream_idle_timeout,
                                bytes_received,
                                stream_start.elapsed(),
                                last_chunk_at.elapsed(),
                            )));
                            return;
                        }
                    }
                }

                loop {
                    let line = match super::next_sse_line(&mut buffer, ended) {
                        Ok(Some(line)) => line,
                        Ok(None) => break,
                        Err(err) => {
                            yield Err(anyhow::anyhow!("{err}"));
                            return;
                        }
                    };

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

                if ended {
                    break;
                }
            }
        };

        Ok(Box::pin(stream))
    }

    /// Handle a non-streaming Messages API request.
    pub(super) async fn handle_anthropic_message(
        &self,
        prepared: &super::PreparedOutboundRequest,
    ) -> Result<MessageResponse> {
        let response = self
            .send_anthropic_request(&prepared.endpoint.url, &prepared.body)
            .await?;
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
pub(super) fn anthropic_messages_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        format!("{trimmed}/messages")
    } else {
        format!("{trimmed}/v1/messages")
    }
}

/// Whether the route targets first-party Anthropic (`api.anthropic.com`),
/// where the `{"type":"adaptive"}` thinking control is valid. Strict
/// Anthropic-compatible gateways reject it (#4978).
fn is_native_anthropic_base_url(base_url: &str) -> bool {
    let rest = base_url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host = rest
        .split(['/', ':', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    host == "api.anthropic.com" || host.ends_with(".anthropic.com")
}

/// Minimum `budget_tokens` the Messages API accepts for extended thinking.
const MIN_THINKING_BUDGET_TOKENS: u32 = 1024;

/// Effort-tier `budget_tokens` for gateways that only accept the documented
/// `{"type":"enabled","budget_tokens":N}` thinking shape (#4978). The wire
/// contract requires `budget_tokens >= 1024` and `< max_tokens`, so requests
/// too small to fit the minimum budget send no thinking block at all.
fn compat_thinking_budget(effort: Option<&str>, max_tokens: u32) -> Option<u32> {
    let tier: u32 = match effort {
        Some("low" | "minimal") => 4_096,
        Some("medium" | "mid") => 8_192,
        Some("max" | "xhigh" | "highest") => 32_768,
        // "high" and unspecified effort share the adaptive default tier.
        _ => 16_384,
    };
    let budget = tier.min(max_tokens.checked_sub(1)?);
    (budget >= MIN_THINKING_BUDGET_TOKENS).then_some(budget)
}

/// Placeholder body for a `tool_use` that never produced a `tool_result`.
const UNEXECUTED_TOOL_RESULT: &str = "tool call was not executed";

/// Defensive wire repair (#5002): every assistant `tool_use` must be answered
/// by a `tool_result` in the immediately following user message, or the API
/// rejects the whole conversation with a 400 on every retry. Pre-dispatch
/// failure paths (e.g. the model calling an unavailable tool) can strand an
/// orphaned `tool_use` in history, so missing results get an explicit
/// error placeholder instead of poisoning the session.
fn repair_dangling_tool_uses(messages: &mut Vec<Value>) {
    let mut index = 0;
    while index < messages.len() {
        let ids = assistant_tool_use_ids(&messages[index]);
        if ids.is_empty() {
            index += 1;
            continue;
        }
        let next_is_user = messages
            .get(index + 1)
            .and_then(|message| message.get("role"))
            .and_then(Value::as_str)
            == Some("user");
        if !next_is_user {
            messages.insert(index + 1, json!({ "role": "user", "content": [] }));
        }
        if let Some(blocks) = messages[index + 1]
            .get_mut("content")
            .and_then(Value::as_array_mut)
        {
            let answered: std::collections::HashSet<String> = blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
                .filter_map(|block| block.get("tool_use_id").and_then(Value::as_str))
                .map(str::to_string)
                .collect();
            // tool_result blocks must lead the user turn, so placeholders are
            // prepended in tool_use order.
            for (offset, id) in ids
                .iter()
                .filter(|id| !answered.contains(id.as_str()))
                .enumerate()
            {
                blocks.insert(
                    offset,
                    json!({
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": UNEXECUTED_TOOL_RESULT,
                        "is_error": true,
                    }),
                );
            }
        }
        index += 1;
    }
}

fn assistant_tool_use_ids(message: &Value) -> Vec<String> {
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return Vec::new();
    }
    message
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
                .filter_map(|block| block.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
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
/// when no blocks survive conversion (Anthropic rejects empty content) or
/// when the role has no Anthropic channel.
///
/// The wire role used to be `message.role` forwarded verbatim, which is how a
/// `system` message ended up on the wire for the provider to 400 on. It now
/// comes from the shared placement table, and pairs the table rejects are
/// refused at the outbound seam before this function ever runs.
pub(super) fn message_to_anthropic(message: &crate::models::Message) -> Option<Value> {
    let placement = role_placement(&message.role, WireDialect::AnthropicMessages);
    let wire_role = match placement {
        RolePlacement::User | RolePlacement::Developer => "user",
        RolePlacement::Assistant | RolePlacement::InterruptedAssistant => "assistant",
        // Unreachable in production: `reject_unsupported_roles` refuses these
        // pairs at the seam. Failing closed here keeps a future caller that
        // skips the seam from putting an unrepresentable role on the wire.
        RolePlacement::System | RolePlacement::Omitted | RolePlacement::Rejected => return None,
    };
    let mut blocks: Vec<Value> = message
        .content
        .iter()
        .filter_map(content_block_to_anthropic)
        .collect();
    if blocks.is_empty() {
        return None;
    }
    if placement == RolePlacement::InterruptedAssistant
        && let Some(text) = blocks
            .iter_mut()
            .find(|block| block.get("type").and_then(Value::as_str) == Some("text"))
    {
        let existing = text
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        text["text"] = json!(format!(
            "{}{}",
            crate::models::INTERRUPTED_ASSISTANT_CONTEXT_PREFIX,
            existing
        ));
    }
    Some(json!({ "role": wire_role, "content": blocks }))
}

/// Project the shared `ImageUrl` block onto Anthropic's tagged image source.
///
/// The OpenAI dialects carry an image as a single URL string, so that is what
/// [`ContentBlock::ImageUrl`] stores. Anthropic instead models the source as a
/// tagged union, and — this is the part that used to be wrong here — it does
/// **not** accept a `data:` URL under `{"type":"url"}`. Sending a local
/// screenshot that way earns an opaque provider-side 400, which is exactly the
/// confusing failure this whole path exists to avoid, so the data URL is taken
/// back apart into `{"type":"base64", media_type, data}`.
fn anthropic_image_block(url: &str) -> Value {
    if let Some((media_type, data)) = crate::image_attach::parse_data_url(url) {
        return json!({
            "type": "image",
            "source": { "type": "base64", "media_type": media_type, "data": data },
        });
    }
    if crate::image_attach::is_remote_image_url(url) {
        return json!({
            "type": "image",
            "source": { "type": "url", "url": url },
        });
    }
    // Anything else (a bare path, a `file://`, a truncated data URL) has no
    // Anthropic representation. Degrade to visible text rather than emitting a
    // source the API will reject: the turn survives and the model can see that
    // something was meant to be here.
    json!({
        "type": "text",
        "text": format!("[unsupported image reference: {url}]"),
    })
}

pub(super) fn anthropic_tool_result_content(
    content: &str,
    content_blocks: Option<&[Value]>,
) -> Value {
    let (image, omitted) = crate::image_attach::provider_tool_result_image_refs(content_blocks);
    let content = crate::image_attach::tool_result_text_with_omission(content, omitted);
    let Some((mime_type, data)) = image else {
        return json!(content);
    };
    let mut blocks = Vec::with_capacity(2);
    if !content.is_empty() {
        blocks.push(json!({ "type": "text", "text": content }));
    }
    blocks.push(json!({
        "type": "image",
        "source": { "type": "base64", "media_type": mime_type, "data": data },
    }));
    json!(blocks)
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
            ..
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
            content_blocks,
        } => {
            let mut value = json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": anthropic_tool_result_content(content, content_blocks.as_deref()),
            });
            if let Some(is_error) = is_error {
                value["is_error"] = json!(is_error);
            }
            Some(value)
        }
        ContentBlock::ImageUrl { image_url } => Some(anthropic_image_block(&image_url.url)),
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
/// (#2961 / #4318): hit = cache reads, write = cache creation, miss = raw
/// uncached input, `input_tokens` = the total prompt across all three.
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
        prompt_cache_miss_tokens: Some(input_raw),
        prompt_cache_write_tokens: Some(cache_creation),
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
    use crate::models::Role;
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
                role: Role::User,
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
        anthropic_test_client(None)
    }

    fn anthropic_test_client(base_url: Option<&str>) -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = crate::config::Config {
            provider: Some("anthropic".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                anthropic: crate::config::ProviderConfig {
                    api_key: Some("test-key".to_string()),
                    base_url: base_url.map(str::to_string),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        DeepSeekClient::new(&config).expect("anthropic client constructs")
    }

    fn minimax_test_client() -> DeepSeekClient {
        minimax_test_client_for(crate::config::DEFAULT_MINIMAX_ANTHROPIC_BASE_URL)
    }

    fn minimax_test_client_for(base_url: &str) -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = crate::config::Config {
            provider: Some("minimax-anthropic".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                minimax_anthropic: crate::config::ProviderConfig {
                    api_key: Some("test-key".to_string()),
                    base_url: Some(base_url.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        DeepSeekClient::new(&config).expect("MiniMax Messages client constructs")
    }

    fn deepseek_test_client(base_url: &str) -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = crate::config::Config {
            provider: Some("deepseek-anthropic".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                deepseek_anthropic: crate::config::ProviderConfig {
                    api_key: Some("test-key".to_string()),
                    base_url: Some(base_url.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        DeepSeekClient::new(&config).expect("DeepSeek Messages client constructs")
    }

    fn modelstudio_test_client(base_url: &str) -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = crate::config::Config {
            provider: Some("modelstudio-token-plan-anthropic".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                // All four plan/dialect variants share one key slot
                // (modelstudio-token-plan); only the base URL is read from the
                // anthropic entry.
                modelstudio_token_plan: crate::config::ProviderConfig {
                    api_key: Some("test-key".to_string()),
                    ..Default::default()
                },
                modelstudio_token_plan_anthropic: crate::config::ProviderConfig {
                    base_url: Some(base_url.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        DeepSeekClient::new(&config).expect("Model Studio Messages client constructs")
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
    fn compat_gateway_sends_enabled_budget_thinking_instead_of_adaptive() {
        // #4978: strict Anthropic-compatible gateways (e.g. Sensenova) reject
        // {"type":"adaptive"} with a 400; non-native routes must send the
        // documented enabled+budget shape and no output_config.
        let client = anthropic_test_client(Some("https://api.sensenova.example/v1"));

        let mut request = request_with("claude-sonnet-4-6", Some("high"), None, None);
        request.max_tokens = 64_000;
        let body = client.build_anthropic_body(&request, true);
        assert_eq!(
            body.pointer("/thinking/type").and_then(Value::as_str),
            Some("enabled"),
            "{body}"
        );
        assert_eq!(
            body.pointer("/thinking/budget_tokens")
                .and_then(Value::as_u64),
            Some(16_384)
        );
        assert!(body.get("output_config").is_none(), "{body}");

        // Effort tiers map onto budgets, capped below max_tokens.
        let mut request = request_with("claude-sonnet-4-6", Some("max"), None, None);
        request.max_tokens = 64_000;
        let body = client.build_anthropic_body(&request, true);
        assert_eq!(
            body.pointer("/thinking/budget_tokens")
                .and_then(Value::as_u64),
            Some(32_768)
        );
        let mut request = request_with("claude-sonnet-4-6", Some("max"), None, None);
        request.max_tokens = 8_000;
        let body = client.build_anthropic_body(&request, true);
        assert_eq!(
            body.pointer("/thinking/budget_tokens")
                .and_then(Value::as_u64),
            Some(7_999),
            "budget stays below max_tokens: {body}"
        );

        // Unspecified effort defaults to the "high" tier.
        let mut request = request_with("claude-sonnet-4-6", None, None, None);
        request.max_tokens = 64_000;
        let body = client.build_anthropic_body(&request, true);
        assert_eq!(
            body.pointer("/thinking/type").and_then(Value::as_str),
            Some("enabled")
        );
        assert_eq!(
            body.pointer("/thinking/budget_tokens")
                .and_then(Value::as_u64),
            Some(16_384)
        );

        // "off" and requests too small for the 1024-token minimum budget
        // omit thinking entirely.
        let mut request = request_with("claude-sonnet-4-6", Some("off"), None, None);
        request.max_tokens = 64_000;
        let body = client.build_anthropic_body(&request, true);
        assert!(body.get("thinking").is_none(), "{body}");
        let body = client.build_anthropic_body(
            &request_with("claude-sonnet-4-6", Some("high"), None, None),
            true,
        );
        assert!(
            body.get("thinking").is_none(),
            "max_tokens=1024 cannot fit the minimum budget: {body}"
        );

        // The native route keeps adaptive; the compat shape is only for
        // non-anthropic.com hosts.
        let native = test_client().build_anthropic_body(
            &request_with("claude-sonnet-4-6", Some("high"), None, None),
            true,
        );
        assert_eq!(
            native.pointer("/thinking/type").and_then(Value::as_str),
            Some("adaptive")
        );
    }

    #[test]
    fn dangling_tool_use_gets_placeholder_tool_result() {
        // #5002: an orphaned tool_use with no matching tool_result poisons
        // the conversation with repeated 400s; request preparation must
        // repair it with an explicit placeholder result.
        let client = test_client();
        let mut request = request_with("claude-sonnet-4-6", None, None, None);
        request.messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "run both tools".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: "toolu_ok".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "a.txt"}),
                        caller: None,
                        thought_signature: None,
                    },
                    ContentBlock::ToolUse {
                        id: "toolu_orphan".to_string(),
                        name: "task".to_string(),
                        input: json!({}),
                        caller: None,
                        thought_signature: None,
                    },
                ],
            },
            // Pre-dispatch failure left only one tool_result behind.
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "toolu_ok".to_string(),
                    content: "contents".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            // Trailing assistant tool_use with no user turn at all.
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "toolu_tail".to_string(),
                    name: "task".to_string(),
                    input: json!({}),
                    caller: None,
                    thought_signature: None,
                }],
            },
        ];

        let body = client.build_anthropic_body(&request, true);
        let messages = body["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 5, "a repair turn is appended: {body}");

        // The orphaned id gets a leading placeholder; the answered one is
        // untouched (no duplicate result).
        let repaired = messages[2]["content"].as_array().expect("user content");
        assert_eq!(repaired.len(), 2, "{body}");
        assert_eq!(repaired[0]["type"].as_str(), Some("tool_result"));
        assert_eq!(repaired[0]["tool_use_id"].as_str(), Some("toolu_orphan"));
        assert_eq!(
            repaired[0]["content"].as_str(),
            Some(UNEXECUTED_TOOL_RESULT)
        );
        assert_eq!(repaired[0]["is_error"].as_bool(), Some(true));
        assert_eq!(repaired[1]["tool_use_id"].as_str(), Some("toolu_ok"));
        assert_eq!(repaired[1]["content"].as_str(), Some("contents"));

        // The trailing tool_use gains a synthesized user turn.
        assert_eq!(messages[4]["role"].as_str(), Some("user"));
        let tail = messages[4]["content"].as_array().expect("tail content");
        assert_eq!(tail.len(), 1, "{body}");
        assert_eq!(tail[0]["type"].as_str(), Some("tool_result"));
        assert_eq!(tail[0]["tool_use_id"].as_str(), Some("toolu_tail"));
        assert_eq!(tail[0]["content"].as_str(), Some(UNEXECUTED_TOOL_RESULT));

        // A fully answered history is left alone.
        request.messages.truncate(3);
        request.messages[1].content.retain(
            |block| !matches!(block, ContentBlock::ToolUse { id, ..} if id == "toolu_orphan"),
        );
        let body = client.build_anthropic_body(&request, true);
        let messages = body["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 3, "no repair turn appended: {body}");
        let untouched = messages[2]["content"].as_array().expect("user content");
        assert_eq!(untouched.len(), 1, "{body}");
        assert_eq!(untouched[0]["tool_use_id"].as_str(), Some("toolu_ok"));
    }

    #[test]
    fn modelstudio_messages_body_requests_thinking_with_budget() {
        // Model Studio's Anthropic-compatible endpoint documents the portable
        // {"type":"enabled","budget_tokens":N} shape plus {"type":"disabled"}
        // (alibabacloud.com/help/en/model-studio/anthropic-api-messages).
        let client = modelstudio_test_client(
            "https://token-plan.ap-southeast-1.maas.aliyuncs.com/apps/anthropic",
        );

        let mut request = request_with("qwen3.8-max", Some("high"), None, None);
        request.max_tokens = 64_000;
        let body = client.build_anthropic_body(&request, true);
        assert_eq!(
            body.pointer("/thinking/type").and_then(Value::as_str),
            Some("enabled"),
            "{body}"
        );
        assert!(
            body.pointer("/thinking/budget_tokens")
                .and_then(Value::as_u64)
                .is_some(),
            "{body}"
        );
        assert!(body.get("output_config").is_none(), "{body}");
        assert_eq!(
            body.get("model").and_then(Value::as_str),
            Some("qwen3.8-max"),
            "{body}"
        );

        // An explicit "off" is honored on the wire instead of silently
        // falling through to the server default (thinking-ON for qwen3.x).
        let mut request = request_with("qwen3.8-max", Some("off"), None, None);
        request.max_tokens = 64_000;
        let body = client.build_anthropic_body(&request, true);
        assert_eq!(
            body.pointer("/thinking/type").and_then(Value::as_str),
            Some("disabled"),
            "{body}"
        );
    }

    #[test]
    fn deepseek_messages_body_retires_aliases_and_keeps_thinking_control() {
        let client = deepseek_test_client(crate::config::DEFAULT_DEEPSEEK_ANTHROPIC_BASE_URL);

        let chat = client.build_anthropic_body(
            &request_with("deepseek-chat", Some("off"), None, None),
            true,
        );
        assert_eq!(
            chat.get("model").and_then(Value::as_str),
            Some(crate::config::DEEPSEEK_ALIAS_REPLACEMENT)
        );
        assert_eq!(
            chat.pointer("/thinking/type").and_then(Value::as_str),
            Some("disabled")
        );

        let reasoner = client.build_anthropic_body(
            &request_with("deepseek-reasoner", Some("high"), None, None),
            true,
        );
        assert_eq!(
            reasoner.get("model").and_then(Value::as_str),
            Some(crate::config::DEEPSEEK_ALIAS_REPLACEMENT)
        );
        assert_eq!(
            reasoner.pointer("/thinking/type").and_then(Value::as_str),
            Some("adaptive")
        );
        assert_eq!(
            reasoner
                .pointer("/output_config/effort")
                .and_then(Value::as_str),
            Some("high")
        );

        let custom = deepseek_test_client("https://messages.example/v1");
        let custom_body = custom.build_anthropic_body(
            &request_with("deepseek-reasoner", Some("high"), None, None),
            true,
        );
        assert_eq!(
            custom_body.get("model").and_then(Value::as_str),
            Some("deepseek-reasoner")
        );
    }

    #[test]
    fn omitted_alias_effort_is_migrated_into_deepseek_messages_body() {
        for (alias, expected_effort, expected_thinking) in [
            ("deepseek-chat", "off", "disabled"),
            ("deepseek-reasoner", "high", "adaptive"),
        ] {
            let mut config = crate::config::Config {
                provider: Some("deepseek-anthropic".to_string()),
                providers: Some(crate::config::ProvidersConfig {
                    deepseek_anthropic: crate::config::ProviderConfig {
                        api_key: Some("test-key".to_string()),
                        model: Some(alias.to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                ..Default::default()
            };
            assert!(
                config.reasoning_effort().is_none(),
                "fixture must omit effort"
            );

            crate::config::normalize_model_config_for_test(&mut config);
            let client = DeepSeekClient::new(&config).expect("DeepSeek Messages client");
            let model = config.default_model();
            let body = client.build_anthropic_body(
                &request_with(&model, config.reasoning_effort(), None, None),
                true,
            );

            assert_eq!(
                body.get("model").and_then(Value::as_str),
                Some(crate::config::DEEPSEEK_ALIAS_REPLACEMENT),
                "{alias}: {body}"
            );
            assert_eq!(config.reasoning_effort(), Some(expected_effort));
            assert_eq!(
                body.pointer("/thinking/type").and_then(Value::as_str),
                Some(expected_thinking),
                "{alias}: {body}"
            );
            if alias == "deepseek-reasoner" {
                assert_eq!(
                    body.pointer("/output_config/effort")
                        .and_then(Value::as_str),
                    Some("high"),
                    "{body}"
                );
            } else {
                assert!(body.get("output_config").is_none(), "{body}");
            }
        }
    }

    #[test]
    fn minimax_body_uses_supported_thinking_controls() {
        let client = minimax_test_client();
        let body =
            client.build_anthropic_body(&request_with("MiniMax-M3", Some("off"), None, None), true);
        assert_eq!(
            body.pointer("/thinking/type").and_then(Value::as_str),
            Some("disabled")
        );
        assert!(body.get("output_config").is_none(), "{body}");

        let mut enabled_bodies = Vec::new();
        for effort in ["high", "max"] {
            let body = client
                .build_anthropic_body(&request_with("MiniMax-M3", Some(effort), None, None), true);
            assert_eq!(
                body.pointer("/thinking/type").and_then(Value::as_str),
                Some("adaptive"),
                "{effort}: {body}"
            );
            assert!(body.get("output_config").is_none(), "{effort}: {body}");
            enabled_bodies.push(body);
        }
        assert_eq!(
            enabled_bodies[0].get("thinking"),
            enabled_bodies[1].get("thinking"),
            "MiniMax high/max select the same untiered adaptive wire control"
        );
    }

    #[test]
    fn minimax_messages_reasoning_controls_require_exact_first_party_m3_route() {
        for (base_url, model) in [
            (
                "https://gateway.example/anthropic",
                crate::config::DEFAULT_MINIMAX_MODEL,
            ),
            (
                crate::config::DEFAULT_MINIMAX_ANTHROPIC_BASE_URL,
                "MiniMax-M2",
            ),
        ] {
            let client = minimax_test_client_for(base_url);
            for effort in ["off", "high", "max"] {
                let body = client
                    .build_anthropic_body(&request_with(model, Some(effort), None, None), true);
                assert!(
                    body.get("thinking").is_none(),
                    "{base_url} {model} {effort}: {body}"
                );
                assert!(
                    body.get("output_config").is_none(),
                    "{base_url} {model} {effort}: {body}"
                );
            }
        }
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
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "do the thing".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "signed reasoning".to_string(),
                        signature: Some("sig-abc".to_string()),
                        state: None,
                    },
                    ContentBlock::Thinking {
                        thinking: "(reasoning omitted)".to_string(),
                        signature: None,
                        state: None,
                    },
                    ContentBlock::ToolUse {
                        id: "toolu_1".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "a.txt"}),
                        caller: None,
                        thought_signature: None,
                    },
                ],
            },
            Message {
                role: Role::User,
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
                role: Role::User,
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
        assert_eq!(message.usage.prompt_cache_miss_tokens, Some(3));
        assert_eq!(message.usage.prompt_cache_write_tokens, Some(2045));

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
        assert_eq!(usage.prompt_cache_write_tokens, Some(0));
    }

    #[test]
    fn usage_mapping_keeps_cache_write_separate_from_miss() {
        let usage = parse_anthropic_usage(&json!({
            "input_tokens": 3,
            "cache_creation_input_tokens": 2045,
            "cache_read_input_tokens": 18000,
            "output_tokens": 1,
        }));
        assert_eq!(usage.input_tokens, 3 + 2045 + 18000);
        assert_eq!(usage.prompt_cache_hit_tokens, Some(18000));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(3));
        assert_eq!(usage.prompt_cache_write_tokens, Some(2045));
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
    fn data_url_image_becomes_a_base64_source_not_a_url_source() {
        // Anthropic rejects a `data:` URL under `{"type":"url"}`. This is the
        // whole reason the projection exists; if it regresses, every locally
        // attached screenshot 400s on the native route.
        let block = content_block_to_anthropic(&ContentBlock::ImageUrl {
            image_url: crate::models::ImageUrlContent {
                url: "data:image/png;base64,QUJD".to_string(),
            },
        })
        .expect("image block");

        assert_eq!(block["type"], "image");
        assert_eq!(block["source"]["type"], "base64");
        assert_eq!(block["source"]["media_type"], "image/png");
        assert_eq!(block["source"]["data"], "QUJD");
        assert!(
            block["source"].get("url").is_none(),
            "base64 sources must not carry a url field: {block}"
        );
    }

    #[test]
    fn tool_result_image_stays_inside_the_native_tool_result_block() {
        let content = anthropic_tool_result_content(
            "screenshot captured",
            Some(&[json!({
                "type": "image",
                "mime_type": "image/png",
                "data": "QUJD",
            })]),
        );
        let blocks = content.as_array().expect("rich tool_result content");

        assert_eq!(
            blocks[0],
            json!({"type": "text", "text": "screenshot captured"})
        );
        assert_eq!(blocks[1]["type"], "image");
        assert_eq!(blocks[1]["source"]["type"], "base64");
        assert_eq!(blocks[1]["source"]["media_type"], "image/png");
        assert_eq!(blocks[1]["source"]["data"], "QUJD");
    }

    #[test]
    fn remote_image_url_stays_a_url_source() {
        let block = content_block_to_anthropic(&ContentBlock::ImageUrl {
            image_url: crate::models::ImageUrlContent {
                url: "https://example.com/shot.png".to_string(),
            },
        })
        .expect("image block");

        assert_eq!(block["type"], "image");
        assert_eq!(block["source"]["type"], "url");
        assert_eq!(block["source"]["url"], "https://example.com/shot.png");
    }

    #[test]
    fn unrepresentable_image_reference_degrades_to_visible_text() {
        for url in [
            "file:///tmp/shot.png",
            "/tmp/shot.png",
            "data:image/png,QUJD",
        ] {
            let block = content_block_to_anthropic(&ContentBlock::ImageUrl {
                image_url: crate::models::ImageUrlContent {
                    url: url.to_string(),
                },
            })
            .expect("block");

            assert_eq!(block["type"], "text", "{url} should degrade: {block}");
            assert!(
                block["text"].as_str().expect("text").contains(url),
                "the degraded text should name the reference: {block}"
            );
        }
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
        assert_eq!(
            anthropic_messages_url("https://api.minimax.io/anthropic"),
            "https://api.minimax.io/anthropic/v1/messages"
        );
        assert_eq!(
            anthropic_messages_url("https://api.minimaxi.com/anthropic"),
            "https://api.minimaxi.com/anthropic/v1/messages"
        );
    }

    #[test]
    fn anthropic_body_serializes_the_child_catalog_without_duplication() {
        // The real child catalog fixture (not a hand-built tool list) must
        // survive Messages serialization with exactly one canonical `read`
        // entry — no dedup, filter, or sanitizer may drop or duplicate it.
        // Skills are discoverable through tool_search, so the child wire
        // catalog carries no load_skill at all.
        let tools = crate::tools::subagent::kimi_general_child_request_tools_fixture();
        assert_eq!(
            tools.iter().filter(|tool| tool.name == "read").count(),
            1,
            "catalog fixture carries one canonical read"
        );
        assert_eq!(
            tools
                .iter()
                .filter(|tool| tool.name == "load_skill")
                .count(),
            0,
            "load_skill is not part of the child wire catalog"
        );
        let client = test_client();
        let mut request = request_with("claude-sonnet-4-6", None, None, None);
        request.tools = Some(tools);
        let body = client.build_anthropic_body(&request, true);
        let serialized = body["tools"]
            .as_array()
            .expect("tools serialize as an array");
        let reads: Vec<_> = serialized
            .iter()
            .filter(|tool| tool["name"] == "read")
            .collect();
        assert_eq!(
            reads.len(),
            1,
            "exactly one canonical read definition reaches the Messages wire"
        );
        assert!(
            reads[0]["input_schema"]["properties"].is_object(),
            "read keeps a valid object schema: {}",
            reads[0]
        );
        assert!(
            serialized.iter().all(|tool| tool["name"] != "load_skill"),
            "load_skill must not appear on the child Messages wire"
        );
    }

    #[tokio::test]
    async fn anthropic_stream_opens_through_shared_seam_preserving_headers() {
        use futures_util::StreamExt;
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // The wire-specific Accept header must survive the shared stream-entry
        // open path; the mock only answers when it is present.
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("Accept", "text/event-stream"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "text/event-stream")
                    .set_body_string("data: {\"type\":\"message_stop\"}\n\n"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = deepseek_test_client(&server.uri());
        let mut stream = client
            .handle_anthropic_stream(
                &client
                    .prepare_outbound_request(request_with("deepseek-v4", None, None, None), true)
                    .expect("anthropic request prepares"),
            )
            .await
            .expect("stream opens through the shared seam");

        let mut saw_stop = false;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(event) = stream.next().await {
                if matches!(event.expect("stream event"), StreamEvent::MessageStop) {
                    saw_stop = true;
                }
            }
        })
        .await
        .expect("stream finishes after message_stop");
        assert!(saw_stop, "message_stop should arrive through the seam");
    }

    #[tokio::test]
    async fn anthropic_stream_open_error_is_not_retried() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // A definitive provider error before any stream body must fail fast:
        // exactly one request, no H1 fallback, envelope preserved.
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(401).set_body_string(
                "{\"error\":{\"type\":\"authentication_error\",\"message\":\"bad key\"}}",
            ))
            .expect(1)
            .mount(&server)
            .await;

        let client = deepseek_test_client(&server.uri());
        let err = match client
            .handle_anthropic_stream(
                &client
                    .prepare_outbound_request(request_with("deepseek-v4", None, None, None), true)
                    .expect("anthropic request prepares"),
            )
            .await
        {
            Ok(_) => panic!("auth errors must fail fast"),
            Err(err) => err,
        };
        let text = err.to_string();
        assert!(
            text.contains("HTTP 401") && text.contains("authentication_error"),
            "error envelope should be preserved: {text}"
        );
    }

    /// A `system`-role history message — what a compaction summary, a branch
    /// summary, or an imported journal `system` entry becomes once it reaches
    /// `MessageRequest::messages` — must not be emitted verbatim: the Messages
    /// API accepts only `user` and `assistant` in `messages[].role` and 400s
    /// the whole conversation otherwise, on every retry.
    #[test]
    fn system_role_history_message_is_not_emitted_verbatim_on_the_messages_wire() {
        let mut request = request_with("claude-opus-4-6", None, None, None);
        request.messages.insert(
            0,
            Message {
                role: Role::System,
                content: vec![ContentBlock::Text {
                    text: "[compaction summary] the user is porting the parser".to_string(),
                    cache_control: None,
                }],
            },
        );

        let body = test_client().build_anthropic_body(&request, false);
        let messages = body["messages"].as_array().expect("messages array");

        for message in messages {
            let role = message["role"].as_str().expect("role is a string");
            assert!(
                role == "user" || role == "assistant",
                "Messages API rejects role {role:?}"
            );
        }
        let carried = messages.iter().any(|message| {
            message["content"].as_array().is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block["text"].as_str()
                        == Some("[compaction summary] the user is porting the parser")
                })
            })
        });
        assert!(carried, "the summary text must survive: {messages:?}");
    }
}
