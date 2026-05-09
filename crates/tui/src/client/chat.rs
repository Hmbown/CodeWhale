//! Chat Completions API helpers for DeepSeek's OpenAI-compatible endpoint.
//!
//! This is the production code path. Streaming (`create_message_stream`),
//! request building (`build_chat_messages*`), and SSE parsing (`parse_sse_chunk`)
//! all live here.

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::time::timeout as tokio_timeout;

/// Default idle timeout for SSE stream reads (300 seconds = 5 minutes).
/// After this period with no data, the stream is considered stalled and
/// yields a recoverable error so the caller can retry.
const DEFAULT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Default timeout for the initial streaming response headers.
///
/// `doctor` uses a bounded non-streaming request, but normal TUI turns first
/// wait for the SSE response to open. On some Windows/proxy paths that wait can
/// hang before any stream chunk exists, leaving the UI stuck at "Working...".
const DEFAULT_STREAM_OPEN_TIMEOUT: Duration = Duration::from_secs(45);

/// Reads `DEEPSEEK_STREAM_OPEN_TIMEOUT_SECS` as a bounded override for the
/// response-header wait. This is intentionally shorter than the per-chunk idle
/// timeout because it only covers connection setup and upstream header return,
/// not model thinking time after streaming has started.
fn stream_open_timeout() -> Duration {
    stream_open_timeout_from_env(
        std::env::var("DEEPSEEK_STREAM_OPEN_TIMEOUT_SECS")
            .ok()
            .as_deref(),
    )
}

fn stream_open_timeout_from_env(value: Option<&str>) -> Duration {
    let secs = value
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_STREAM_OPEN_TIMEOUT.as_secs())
        .clamp(5, 300);
    Duration::from_secs(secs)
}

/// Reads the `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS` env var, falling back to
/// the default 300s. The parsed value is clamped to [1, 3600] seconds.
fn stream_idle_timeout() -> Duration {
    let secs = std::env::var("DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_STREAM_IDLE_TIMEOUT.as_secs())
        .clamp(1, 3600);
    Duration::from_secs(secs)
}

use crate::llm_client::StreamEventBox;
use crate::logging;
use crate::models::{
    ContentBlock, ContentBlockStart, Delta, Message, MessageDelta, MessageRequest, MessageResponse,
    StreamEvent, SystemPrompt, Tool, ToolCaller, Usage,
};

use super::{
    DeepSeekClient, ERROR_BODY_MAX_BYTES, SSE_BACKPRESSURE_HIGH_WATERMARK,
    SSE_BACKPRESSURE_SLEEP_MS, SSE_MAX_LINES_PER_CHUNK, acquire_stream_buffer, api_url,
    apply_reasoning_effort, bounded_error_text, from_api_tool_name, parse_usage,
    release_stream_buffer, system_to_instructions, to_api_tool_name,
};

impl DeepSeekClient {
    pub(super) async fn create_message_chat(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse> {
        let messages = build_chat_messages_for_request(request);
        let mut body = json!({
            "model": request.model,
            "messages": messages,
            "max_tokens": request.max_tokens,
        });

        if let Some(temperature) = request.temperature {
            body["temperature"] = json!(temperature);
        }
        if let Some(top_p) = request.top_p {
            body["top_p"] = json!(top_p);
        }
        if let Some(tools) = request.tools.as_ref() {
            body["tools"] = json!(
                tools
                    .iter()
                    .map(|tool| tool_to_chat_for_base_url(tool, &self.base_url))
                    .collect::<Vec<_>>()
            );
        }
        if let Some(choice) = request.tool_choice.as_ref()
            && let Some(mapped) = map_tool_choice_for_chat(choice)
        {
            body["tool_choice"] = mapped;
        }
        apply_reasoning_effort(
            &mut body,
            request.reasoning_effort.as_deref(),
            self.api_provider,
        );

        let url = api_url(&self.base_url, "chat/completions");
        let open_timeout = stream_open_timeout();
        let response = match tokio_timeout(
            open_timeout,
            self.send_with_retry(|| self.http_client.post(&url).json(&body)),
        )
        .await
        {
            Ok(result) => result?,
            Err(_elapsed) => {
                anyhow::bail!(
                    "SSE stream request did not receive response headers after {}s. \
                     `deepseek doctor` can still pass when non-streaming requests work; \
                     on Windows or proxy networks, try `DEEPSEEK_FORCE_HTTP1=1` and rerun `deepseek`.",
                    open_timeout.as_secs()
                );
            }
        };

        let status = response.status();
        if !status.is_success() {
            let error_text = bounded_error_text(response, ERROR_BODY_MAX_BYTES).await;
            anyhow::bail!("Failed to call DeepSeek Chat API: HTTP {status}: {error_text}");
        }

        let response_text = response.text().await.unwrap_or_default();
        let value: Value =
            serde_json::from_str(&response_text).context("Failed to parse Chat API JSON")?;
        parse_chat_message(&value)
    }
}

impl DeepSeekClient {
    pub(super) async fn handle_chat_completion_stream(
        &self,
        request: MessageRequest,
    ) -> Result<StreamEventBox> {
        // Try true SSE streaming via chat completions (widely supported)
        let messages = build_chat_messages_for_request(&request);
        let mut body = json!({
            "model": request.model,
            "messages": messages,
            "max_tokens": request.max_tokens,
            "stream": true,
            "stream_options": {
                "include_usage": true
            },
        });

        if let Some(temperature) = request.temperature {
            body["temperature"] = json!(temperature);
        }
        if let Some(top_p) = request.top_p {
            body["top_p"] = json!(top_p);
        }
        if let Some(tools) = request.tools.as_ref() {
            body["tools"] = json!(
                tools
                    .iter()
                    .map(|tool| tool_to_chat_for_base_url(tool, &self.base_url))
                    .collect::<Vec<_>>()
            );
        }
        if let Some(choice) = request.tool_choice.as_ref()
            && let Some(mapped) = map_tool_choice_for_chat(choice)
        {
            body["tool_choice"] = mapped;
        }
        apply_reasoning_effort(
            &mut body,
            request.reasoning_effort.as_deref(),
            self.api_provider,
        );

        // Bulletproof final sanitizer: walk the wire payload and force
        // `reasoning_content` onto any assistant message that has tool_calls
        // but no reasoning_content. DeepSeek's thinking-mode API rejects
        // such messages with a 400. This is the last line of defense after
        // engine-side and build-side substitution; if either upstream path
        // misses a case (e.g. a session restored from disk, a sub-agent
        // adding messages directly, or a cached prefix mismatch), this pass
        // still produces a valid request.
        let replay_input_tokens = sanitize_thinking_mode_messages(
            &mut body,
            &request.model,
            request.reasoning_effort.as_deref(),
        );

        let url = api_url(&self.base_url, "chat/completions");
        let response = self
            .send_with_retry(|| self.http_client.post(&url).json(&body))
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = bounded_error_text(response, ERROR_BODY_MAX_BYTES).await;
            // If DeepSeek rejected for missing reasoning_content despite the
            // sanitizer, dump the offending indices so we can diagnose where
            // they came from on the next failure.
            if error_text.contains("reasoning_content") {
                log_thinking_mode_violations(&body);
            }
            anyhow::bail!("SSE stream request failed: HTTP {status}: {error_text}");
        }

        let model = request.model.clone();

        // Capture transport-shape headers before we consume `response` into
        // `bytes_stream()`. They are surfaced in the decode-error log path so
        // we can tell HTTP/2 RST_STREAM from chunked-encoding corruption from
        // gzip-compressor failure when investigating #103.
        let response_headers = format_stream_headers(response.headers());
        let byte_stream = response.bytes_stream();

        let stream = async_stream::stream! {
            use futures_util::StreamExt;

            // Emit a synthetic MessageStart
            yield Ok(StreamEvent::MessageStart {
                message: MessageResponse {
                    id: String::new(),
                    r#type: "message".to_string(),
                    role: "assistant".to_string(),
                    content: Vec::new(),
                    model: model.clone(),
                    stop_reason: None,
                    stop_sequence: None,
                    container: None,
                    usage: Usage {
                        input_tokens: 0,
                        output_tokens: 0,
                        ..Usage::default()
                    },
                },
            });

            let mut line_buf = String::new();
            let mut byte_buf = acquire_stream_buffer();
            let mut content_index: u32 = 0;
            let mut text_started = false;
            let mut thinking_started = false;
            let mut tool_indices: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
            let is_reasoning_model = requires_reasoning_content(&model);

            let mut byte_stream = std::pin::pin!(byte_stream);
            let idle = stream_idle_timeout();

            // Telemetry for #103 stream-decode diagnostics: bytes received
            // since the start of this stream and last successful event time.
            // Surfaces in the error log when reqwest yields a chunk error so
            // we can tell HTTP/2 RST_STREAM from chunk-decode-failure from
            // gzip-corruption when investigating a flaky session.
            let stream_start = std::time::Instant::now();
            let mut last_event_at = std::time::Instant::now();
            let mut bytes_received: usize = 0;

            loop {
                let chunk_result = match tokio_timeout(idle, byte_stream.next()).await {
                    Ok(Some(result)) => result,
                    Ok(None) => break, // Stream ended normally
                    Err(_elapsed) => {
                        yield Err(anyhow::anyhow!(
                            "SSE stream idle timeout after {}s — no data received",
                            idle.as_secs(),
                        ));
                        break;
                    }
                };
                let chunk = match chunk_result {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        // Walk the error source chain so reqwest's underlying
                        // hyper / h2 / io error is visible — without this the
                        // outer "error decoding response body" message tells
                        // us nothing about WHY the stream died.
                        let mut error_chain = format!("{e}");
                        let mut current: Option<&(dyn std::error::Error + 'static)> =
                            std::error::Error::source(&e);
                        while let Some(source) = current {
                            error_chain.push_str(&format!(" -> {source}"));
                            current = std::error::Error::source(source);
                        }
                        crate::logging::warn(format!(
                            "Stream read error: {error_chain} \
                             (elapsed: {}ms, bytes_received: {}, ms_since_last_event: {}, headers: {})",
                            stream_start.elapsed().as_millis(),
                            bytes_received,
                            last_event_at.elapsed().as_millis(),
                            response_headers,
                        ));
                        yield Err(anyhow::anyhow!("Stream read error: {e}"));
                        break;
                    }
                };

                bytes_received = bytes_received.saturating_add(chunk.len());
                last_event_at = std::time::Instant::now();
                byte_buf.extend_from_slice(&chunk);

                // Guard against unbounded buffer growth (e.g., malformed stream without newlines)
                const MAX_SSE_BUF: usize = 10 * 1024 * 1024; // 10 MB
                if byte_buf.len() > MAX_SSE_BUF {
                    yield Err(anyhow::anyhow!("SSE buffer exceeded {MAX_SSE_BUF} bytes — aborting stream"));
                    break;
                }

                if byte_buf.len() > SSE_BACKPRESSURE_HIGH_WATERMARK {
                    tokio::time::sleep(Duration::from_millis(SSE_BACKPRESSURE_SLEEP_MS)).await;
                }

                // Process complete SSE lines from the buffer
                let mut lines_processed = 0usize;
                while let Some(newline_pos) = byte_buf.iter().position(|&b| b == b'\n') {
                    let mut end = newline_pos;
                    if end > 0 && byte_buf[end - 1] == b'\r' {
                        end -= 1;
                    }
                    let line = String::from_utf8_lossy(&byte_buf[..end]).into_owned();
                    byte_buf.drain(..newline_pos + 1);

                    if line.is_empty() {
                        // Empty line = event boundary, process accumulated data
                        if !line_buf.is_empty() {
                            let data = std::mem::take(&mut line_buf);
                            if data.trim() == "[DONE]" {
                                // Stream complete
                            } else if let Ok(chunk_json) = serde_json::from_str::<Value>(&data) {
                                // Parse the SSE chunk into stream events
                                for mut event in parse_sse_chunk(
                                    &chunk_json,
                                    &mut content_index,
                                    &mut text_started,
                                    &mut thinking_started,
                                    &mut tool_indices,
                                    is_reasoning_model,
                                ) {
                                    // Stamp the client-side replay-token estimate
                                    // onto the final usage so the UI can surface
                                    // it (#30). We compute it pre-request and
                                    // overlay it on the server-reported usage at
                                    // stream completion.
                                    if let Some(tokens) = replay_input_tokens
                                        && let StreamEvent::MessageDelta {
                                            usage: Some(usage),
                                            ..
                                        } = &mut event
                                    {
                                        usage.reasoning_replay_tokens = Some(tokens);
                                    }
                                    yield Ok(event);
                                }
                            }
                        }
                        continue;
                    }

                    if let Some(data) = line.strip_prefix("data: ") {
                        line_buf.push_str(data);
                    }
                    // Ignore other SSE fields (event:, id:, retry:)

                    lines_processed = lines_processed.saturating_add(1);
                    if lines_processed >= SSE_MAX_LINES_PER_CHUNK {
                        // Yield backpressure relief to avoid starving downstream consumers.
                        break;
                    }
                }
            }

            // Close any open blocks
            if thinking_started {
                yield Ok(StreamEvent::ContentBlockStop { index: content_index.saturating_sub(1) });
            }
            if text_started {
                yield Ok(StreamEvent::ContentBlockStop { index: content_index.saturating_sub(1) });
            }

            release_stream_buffer(byte_buf);
            yield Ok(StreamEvent::MessageStop);
        };

        Ok(Pin::from(Box::new(stream)
            as Box<
                dyn futures_util::Stream<Item = Result<StreamEvent>> + Send,
            >))
    }
}

// === Chat Completions Helpers ===

#[cfg(test)]
pub(super) fn build_chat_messages(
    system: Option<&SystemPrompt>,
    messages: &[Message],
    model: &str,
) -> Vec<Value> {
    build_chat_messages_with_reasoning(
        system,
        messages,
        model,
        should_replay_reasoning_content(model, None),
        false,
    )
}

pub(super) fn build_chat_messages_for_request(request: &MessageRequest) -> Vec<Value> {
    PromptBuilder::for_request(request).build()
}

pub(crate) fn inspect_prompt_for_request(request: &MessageRequest) -> PromptInspection {
    PromptBuilder::for_request(request).inspect()
}

pub(crate) fn build_cache_warmup_request(request: &MessageRequest) -> MessageRequest {
    PromptBuilder::for_request(request).build_cache_warmup_request()
}

struct PromptBuilder<'a> {
    system: Option<&'a SystemPrompt>,
    messages: &'a [Message],
    model: &'a str,
    reasoning_effort: Option<&'a str>,
}

impl<'a> PromptBuilder<'a> {
    fn for_request(request: &'a MessageRequest) -> Self {
        Self {
            system: request.system.as_ref(),
            messages: &request.messages,
            model: &request.model,
            reasoning_effort: request.reasoning_effort.as_deref(),
        }
    }

    fn build(self) -> Vec<Value> {
        build_chat_messages_with_reasoning(
            self.system,
            self.messages,
            self.model,
            should_replay_reasoning_content(self.model, self.reasoning_effort),
            false,
        )
    }

    fn inspect(self) -> PromptInspection {
        let messages = build_chat_messages_with_reasoning(
            self.system,
            self.messages,
            self.model,
            should_replay_reasoning_content(self.model, self.reasoning_effort),
            true,
        );
        inspect_wire_messages(&messages)
    }

    fn build_cache_warmup_request(self) -> MessageRequest {
        let system = stable_system_prompt(self.system);
        let mut messages = stable_history_messages(self.messages);
        messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: CACHE_WARMUP_USER_TAIL.to_string(),
                cache_control: None,
            }],
        });

        MessageRequest {
            model: self.model.to_string(),
            messages,
            max_tokens: 8,
            system,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: self.reasoning_effort.map(str::to_string),
            stream: None,
            temperature: Some(0.0),
            top_p: None,
        }
    }
}

pub(crate) const CACHE_WARMUP_USER_TAIL: &str = "请只回复 OK";
const TOOL_RESULT_SENT_CHAR_BUDGET: usize = 12_000;
const TOOL_RESULT_HEAD_CHARS: usize = 4_000;
const TOOL_RESULT_TAIL_CHARS: usize = 4_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptInspection {
    pub base_static_prefix_hash: String,
    pub full_request_prefix_hash: String,
    pub layers: Vec<PromptLayerInspection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptLayerInspection {
    pub name: String,
    pub stability: PromptLayerStability,
    pub char_len: usize,
    pub sha256: String,
    pub tool_result: Option<ToolResultInspection>,
    pub turn_meta: Option<TurnMetaInspection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolResultInspection {
    pub original_chars: usize,
    pub sent_chars: usize,
    pub truncated: bool,
    pub deduplicated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnMetaInspection {
    pub original_chars: usize,
    pub sent_chars: usize,
    pub deduplicated: bool,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptLayerStability {
    Static,
    History,
    Dynamic,
}

impl PromptLayerStability {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::History => "history",
            Self::Dynamic => "dynamic",
        }
    }
}

fn inspect_wire_messages(messages: &[Value]) -> PromptInspection {
    let mut layers = Vec::new();
    let mut base_static_prefix_parts = Vec::new();
    let mut full_request_prefix_parts = Vec::new();

    for (index, message) in messages.iter().enumerate() {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let content = message_content_for_inspect(message);
        let is_last = index + 1 == messages.len();

        if index == 0 && role == "system" {
            for (name, stability, body) in split_system_layers(&content) {
                if stability == PromptLayerStability::Static {
                    base_static_prefix_parts.push(body.to_string());
                }
                if stability != PromptLayerStability::Dynamic {
                    full_request_prefix_parts.push(body.to_string());
                }
                layers.push(prompt_layer(name, stability, body));
            }
        } else {
            let stability = if (is_last && role == "user") || role == "tool" {
                PromptLayerStability::Dynamic
            } else {
                PromptLayerStability::History
            };
            let name = if is_last && role == "user" {
                "User task".to_string()
            } else {
                format!("Message #{index} {role}")
            };
            if stability != PromptLayerStability::Dynamic {
                full_request_prefix_parts.push(content.clone());
            }
            let mut layer = prompt_layer(name, stability, &content);
            layer.tool_result = tool_result_inspection_for_message(message);
            layer.turn_meta = turn_meta_inspection_for_message(message);
            layers.push(layer);
        }
    }

    let base_static_prefix = base_static_prefix_parts.join("\n");
    let full_request_prefix = full_request_prefix_parts.join("\n");

    PromptInspection {
        base_static_prefix_hash: sha256_hex(base_static_prefix.as_bytes()),
        full_request_prefix_hash: sha256_hex(full_request_prefix.as_bytes()),
        layers,
    }
}

fn message_content_for_inspect(message: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(content) = message.get("content").and_then(Value::as_str)
        && !content.is_empty()
    {
        parts.push(content.to_string());
    }
    if let Some(reasoning) = message.get("reasoning_content").and_then(Value::as_str)
        && !reasoning.is_empty()
    {
        parts.push(reasoning.to_string());
    }
    if let Some(tool_calls) = message.get("tool_calls") {
        parts.push(tool_calls.to_string());
    }
    parts.join("\n")
}

fn tool_result_inspection_for_message(message: &Value) -> Option<ToolResultInspection> {
    if message.get("role").and_then(Value::as_str) != Some("tool") {
        return None;
    }
    let budget = message.get("_tool_result_budget")?;
    Some(ToolResultInspection {
        original_chars: budget
            .get("original_chars")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())?,
        sent_chars: budget
            .get("sent_chars")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())?,
        truncated: budget
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        deduplicated: budget
            .get("deduplicated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn turn_meta_inspection_for_message(message: &Value) -> Option<TurnMetaInspection> {
    let budget = message.get("_turn_meta_budget")?;
    Some(TurnMetaInspection {
        original_chars: budget
            .get("original_chars")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())?,
        sent_chars: budget
            .get("sent_chars")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())?,
        deduplicated: budget
            .get("deduplicated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        sha256: budget
            .get("sha256")
            .and_then(Value::as_str)
            .map(str::to_string)?,
    })
}

fn split_system_layers(content: &str) -> Vec<(String, PromptLayerStability, &str)> {
    let markers = [
        ("Project context", "<project_instructions"),
        ("Project context pack", "## Project Context Pack"),
        ("Environment", "## Environment"),
        ("Configured instructions", "<instructions "),
        ("User memory", "## User Memory"),
        ("Current session goal", "## Current Session Goal"),
        ("Skills", "## Skills"),
        ("Context management", "## Context Management"),
        ("Compact template", "## Compact"),
        ("Previous session handoff", "## Previous Session Handoff"),
    ];

    let mut starts: Vec<(usize, &str)> = markers
        .iter()
        .filter_map(|(name, marker)| content.find(marker).map(|idx| (idx, *name)))
        .collect();
    starts.sort_by_key(|(idx, _)| *idx);

    let mut layers = Vec::new();
    let first_marker = starts.first().map_or(content.len(), |(idx, _)| *idx);
    if first_marker > 0 {
        layers.push((
            "Global system prefix".to_string(),
            PromptLayerStability::Static,
            content[..first_marker].trim(),
        ));
    }

    for (i, (start, name)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map_or(content.len(), |(idx, _)| *idx);
        let stability = if *name == "Previous session handoff" {
            PromptLayerStability::Dynamic
        } else if is_static_base_layer(name) {
            PromptLayerStability::Static
        } else {
            PromptLayerStability::History
        };
        layers.push(((*name).to_string(), stability, content[*start..end].trim()));
    }

    if layers.is_empty() {
        layers.push((
            "Global system prefix".to_string(),
            PromptLayerStability::Static,
            content.trim(),
        ));
    }
    layers
}

fn is_static_base_layer(name: &str) -> bool {
    matches!(
        name,
        "Global system prefix"
            | "Environment"
            | "Skills"
            | "Project context"
            | "Project context pack"
            | "Context management"
            | "Compact template"
    )
}

fn stable_system_prompt(system: Option<&SystemPrompt>) -> Option<SystemPrompt> {
    let instructions = system_to_instructions(system.cloned())?;
    let stable = split_system_layers(&instructions)
        .into_iter()
        .filter_map(|(_, stability, body)| {
            (stability == PromptLayerStability::Static).then_some(body)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if stable.trim().is_empty() {
        None
    } else {
        Some(SystemPrompt::Text(stable))
    }
}

fn stable_history_messages(messages: &[Message]) -> Vec<Message> {
    let mut end = messages.len();
    if messages
        .last()
        .is_some_and(|message| message.role.as_str() == "user")
    {
        end = end.saturating_sub(1);
    }
    messages[..end].to_vec()
}

fn prompt_layer(
    name: String,
    stability: PromptLayerStability,
    content: &str,
) -> PromptLayerInspection {
    PromptLayerInspection {
        name,
        stability,
        char_len: content.chars().count(),
        sha256: sha256_hex(content.as_bytes()),
        tool_result: None,
        turn_meta: None,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[derive(Clone)]
struct PendingToolCallInfo {
    tool_name: String,
    input: Value,
}

struct SeenToolResult {
    message_label: String,
    original_chars: usize,
}

struct WireToolResult {
    content: String,
    original_chars: usize,
    sent_chars: usize,
    truncated: bool,
    deduplicated: bool,
}

#[derive(Clone)]
struct TurnMetaBudget {
    original_chars: usize,
    sent_chars: usize,
    deduplicated: bool,
    sha256: String,
}

struct LastFullTurnMeta {
    sha256: String,
}

fn render_turn_meta_for_wire(
    text: &str,
    last_full_turn_meta: &mut Option<LastFullTurnMeta>,
) -> (String, TurnMetaBudget) {
    let original_chars = text.chars().count();
    let sha = sha256_hex(text.as_bytes());

    if last_full_turn_meta
        .as_ref()
        .is_some_and(|previous| previous.sha256 == sha)
    {
        let rendered =
            format!("<TURN_META_REF sha=\"{sha}\" original_chars=\"{original_chars}\" />");
        let budget = TurnMetaBudget {
            original_chars,
            sent_chars: rendered.chars().count(),
            deduplicated: true,
            sha256: sha,
        };
        return (rendered, budget);
    }

    *last_full_turn_meta = Some(LastFullTurnMeta {
        sha256: sha.clone(),
    });
    (
        text.to_string(),
        TurnMetaBudget {
            original_chars,
            sent_chars: original_chars,
            deduplicated: false,
            sha256: sha,
        },
    )
}

fn is_turn_meta_text(text: &str) -> bool {
    text.trim_start().starts_with("<turn_meta>")
}

fn turn_meta_budget_json(turn_meta: &TurnMetaBudget) -> Value {
    json!({
        "original_chars": turn_meta.original_chars,
        "sent_chars": turn_meta.sent_chars,
        "deduplicated": turn_meta.deduplicated,
        "sha256": turn_meta.sha256,
    })
}

fn compact_tool_result_for_wire(
    tool_name: &str,
    input: &Value,
    content: &str,
    message_label: &str,
    seen_tool_results: &mut HashMap<String, SeenToolResult>,
) -> WireToolResult {
    let original_chars = content.chars().count();
    let sha = sha256_hex(content.as_bytes());

    if let Some(previous) = seen_tool_results.get(&sha) {
        let content = format!(
            "<TOOL_RESULT_REF sha=\"{}\" original_message=\"{}\" chars=\"{}\" />",
            sha, previous.message_label, previous.original_chars
        );
        return WireToolResult {
            sent_chars: content.chars().count(),
            content,
            original_chars,
            truncated: false,
            deduplicated: true,
        };
    }

    seen_tool_results.insert(
        sha.clone(),
        SeenToolResult {
            message_label: message_label.to_string(),
            original_chars,
        },
    );

    if original_chars <= TOOL_RESULT_SENT_CHAR_BUDGET {
        return WireToolResult {
            content: content.to_string(),
            original_chars,
            sent_chars: original_chars,
            truncated: false,
            deduplicated: false,
        };
    }

    let head = first_chars(content, TOOL_RESULT_HEAD_CHARS);
    let tail = last_chars(content, TOOL_RESULT_TAIL_CHARS);
    let kept = head.chars().count() + tail.chars().count();
    let omitted = original_chars.saturating_sub(kept);
    let compacted = format!(
        "[TOOL_RESULT_TRUNCATED]\n\
         tool_name: {tool_name}\n\
         command_or_query: {}\n\
         exit_status: {}\n\
         original_chars: {original_chars}\n\
         sha256: {sha}\n\
         first_chars:\n\
         {head}\n\n\
         [... truncated {omitted} chars from middle ...]\n\n\
         last_chars:\n\
         {tail}",
        tool_command_or_query(input),
        tool_exit_status(content)
    );

    WireToolResult {
        sent_chars: compacted.chars().count(),
        content: compacted,
        original_chars,
        truncated: true,
        deduplicated: false,
    }
}

fn tool_command_or_query(input: &Value) -> String {
    for key in ["command", "cmd", "query", "q", "pattern", "path", "url"] {
        if let Some(value) = input.get(key) {
            return summarize_for_metadata(value, 500);
        }
    }
    summarize_for_metadata(input, 500)
}

fn tool_exit_status(content: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(content) {
        for key in ["exit_code", "exit_status", "status", "code"] {
            if let Some(value) = value.get(key) {
                return summarize_for_metadata(value, 120);
            }
        }
    }

    for line in content.lines().take(20) {
        let trimmed = line.trim();
        for prefix in ["Exit code:", "exit code:", "Exit status:", "exit status:"] {
            if let Some(value) = trimmed.strip_prefix(prefix) {
                return value.trim().to_string();
            }
        }
    }
    "unknown".to_string()
}

fn summarize_for_metadata(value: &Value, max_chars: usize) -> String {
    let raw = value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string());
    let mut summarized = first_chars(&raw.replace('\n', "\\n"), max_chars);
    if raw.chars().count() > max_chars {
        summarized.push_str("...");
    }
    summarized
}

fn first_chars(value: &str, count: usize) -> String {
    value.chars().take(count).collect()
}

fn last_chars(value: &str, count: usize) -> String {
    let mut chars: Vec<char> = value.chars().rev().take(count).collect();
    chars.reverse();
    chars.into_iter().collect()
}

fn build_chat_messages_with_reasoning(
    system: Option<&SystemPrompt>,
    messages: &[Message],
    _model: &str,
    include_reasoning: bool,
    include_tool_budget_metadata: bool,
) -> Vec<Value> {
    let mut out = Vec::new();
    let mut pending_tool_calls: HashMap<String, PendingToolCallInfo> = HashMap::new();
    let mut seen_tool_results: HashMap<String, SeenToolResult> = HashMap::new();
    let mut last_full_turn_meta: Option<LastFullTurnMeta> = None;

    if let Some(instructions) = system_to_instructions(system.cloned())
        && !instructions.trim().is_empty()
    {
        out.push(json!({
            "role": "system",
            "content": instructions,
        }));
    }

    for (message_index, message) in messages.iter().enumerate() {
        let role = message.role.as_str();
        let mut text_parts = Vec::new();
        let mut thinking_parts = Vec::new();
        let mut tool_calls = Vec::new();
        let mut tool_call_infos = Vec::new();
        let mut tool_results: Vec<(String, String, String)> = Vec::new();
        let mut turn_meta_budget: Option<TurnMetaBudget> = None;
        let later_user_turn = messages[message_index + 1..]
            .iter()
            .any(message_starts_user_turn);

        for block in &message.content {
            match block {
                ContentBlock::Text { text, .. } => {
                    if is_turn_meta_text(text) {
                        let (rendered, budget) =
                            render_turn_meta_for_wire(text, &mut last_full_turn_meta);
                        text_parts.push(rendered);
                        turn_meta_budget = Some(budget);
                    } else {
                        text_parts.push(text.clone());
                    }
                }
                ContentBlock::Thinking { thinking } => thinking_parts.push(thinking.clone()),
                ContentBlock::ToolUse {
                    id,
                    name,
                    input,
                    caller,
                    ..
                } => {
                    let args = serde_json::to_string(input).unwrap_or_else(|_| input.to_string());
                    let mut call = json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": to_api_tool_name(name),
                            "arguments": args,
                        }
                    });
                    if let Some(caller) = caller {
                        call["caller"] = json!({
                            "type": caller.caller_type,
                            "tool_id": caller.tool_id,
                        });
                    }
                    tool_calls.push(call);
                    tool_call_infos.push((
                        id.clone(),
                        PendingToolCallInfo {
                            tool_name: name.clone(),
                            input: input.clone(),
                        },
                    ));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    let message_label = format!("Message #{message_index}");
                    tool_results.push((tool_use_id.clone(), content.clone(), message_label));
                }
                ContentBlock::ServerToolUse { .. }
                | ContentBlock::ToolSearchToolResult { .. }
                | ContentBlock::CodeExecutionToolResult { .. } => {}
            }
        }

        if role == "assistant" {
            let content = text_parts.join("\n");
            let mut reasoning_content = thinking_parts.join("\n");
            let has_text = !content.trim().is_empty();
            let has_tool_calls = !tool_calls.is_empty();
            // DeepSeek thinking-mode tool calls must replay `reasoning_content`
            // on subsequent requests. Non-tool assistant reasoning can be
            // omitted once a later real user text message starts a new turn.
            let include_reasoning_for_turn =
                include_reasoning && (has_tool_calls || !later_user_turn);
            let mut has_reasoning =
                include_reasoning_for_turn && !reasoning_content.trim().is_empty();
            if include_reasoning_for_turn && has_tool_calls && !has_reasoning {
                logging::warn(
                    "Substituting placeholder reasoning_content for DeepSeek tool-call assistant message",
                );
                reasoning_content = String::from("(reasoning omitted)");
                has_reasoning = true;
            }

            // DeepSeek rejects assistant messages where both `content` and
            // `tool_calls` are missing/null. Skip such entries even if they
            // carry reasoning-only metadata unless we can send a non-null
            // placeholder content field.
            if !has_text && !has_tool_calls && !has_reasoning {
                pending_tool_calls.clear();
                continue;
            }

            let mut msg = json!({
                "role": "assistant",
                "content": if has_text {
                    json!(content)
                } else if has_reasoning {
                    json!("")
                } else {
                    Value::Null
                },
            });
            if has_reasoning {
                msg["reasoning_content"] = json!(reasoning_content);
            }
            if has_tool_calls {
                msg["tool_calls"] = json!(tool_calls);
                pending_tool_calls = tool_call_infos.into_iter().collect();
            } else {
                pending_tool_calls.clear();
            }
            out.push(msg);
        } else if role == "system" {
            let content = text_parts.join("\n");
            if !content.trim().is_empty() {
                let mut msg = json!({
                    "role": "system",
                    "content": content,
                });
                if include_tool_budget_metadata && let Some(turn_meta) = &turn_meta_budget {
                    msg["_turn_meta_budget"] = turn_meta_budget_json(turn_meta);
                }
                out.push(msg);
            }
        } else if role == "user" {
            let content = text_parts.join("\n");
            if !content.trim().is_empty() {
                let mut msg = json!({
                    "role": "user",
                    "content": content,
                });
                if include_tool_budget_metadata && let Some(turn_meta) = &turn_meta_budget {
                    msg["_turn_meta_budget"] = turn_meta_budget_json(turn_meta);
                }
                out.push(msg);
            }
        }

        if !tool_results.is_empty() {
            if pending_tool_calls.is_empty() {
                logging::warn("Dropping tool results without matching tool_calls");
            } else {
                for (tool_id, content, message_label) in tool_results {
                    if let Some(tool_info) = pending_tool_calls.remove(&tool_id) {
                        let wire_result = compact_tool_result_for_wire(
                            &tool_info.tool_name,
                            &tool_info.input,
                            &content,
                            &message_label,
                            &mut seen_tool_results,
                        );
                        let mut tool_msg = json!({
                            "role": "tool",
                            "tool_call_id": tool_id,
                            "content": wire_result.content,
                        });
                        if include_tool_budget_metadata {
                            tool_msg["_tool_result_budget"] = json!({
                                "original_chars": wire_result.original_chars,
                                "sent_chars": wire_result.sent_chars,
                                "truncated": wire_result.truncated,
                                "deduplicated": wire_result.deduplicated,
                            });
                        }
                        out.push(tool_msg);
                    } else {
                        logging::warn(format!(
                            "Dropping tool result for unknown tool_call_id: {tool_id}"
                        ));
                    }
                }
            }
        } else if role != "assistant" {
            pending_tool_calls.clear();
        }
    }

    // Safety net: after compaction, an assistant message may have tool_calls
    // whose results were summarized away. The API rejects these, so strip
    // the tool_calls (downgrading to a plain assistant message) and remove
    // the now-orphaned tool result messages.
    let mut i = 0;
    while i < out.len() {
        let is_assistant_with_tools = out[i].get("role").and_then(Value::as_str)
            == Some("assistant")
            && out[i].get("tool_calls").is_some();

        if is_assistant_with_tools {
            let expected_ids: HashSet<String> = out[i]
                .get("tool_calls")
                .and_then(Value::as_array)
                .map(|calls| {
                    calls
                        .iter()
                        .filter_map(|c| c.get("id").and_then(Value::as_str).map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            // Collect tool result IDs immediately following this assistant message.
            let mut found_ids: HashSet<String> = HashSet::new();
            let mut tool_result_end = i + 1;
            while tool_result_end < out.len() {
                if out[tool_result_end].get("role").and_then(Value::as_str) == Some("tool") {
                    if let Some(id) = out[tool_result_end]
                        .get("tool_call_id")
                        .and_then(Value::as_str)
                    {
                        found_ids.insert(id.to_string());
                    }
                    tool_result_end += 1;
                } else {
                    break;
                }
            }

            // Also scan non-contiguous tool results up to the next assistant message
            // in case compaction left gaps.
            let mut scan = tool_result_end;
            while scan < out.len() {
                if out[scan].get("role").and_then(Value::as_str) == Some("assistant") {
                    break;
                }
                if out[scan].get("role").and_then(Value::as_str) == Some("tool")
                    && let Some(id) = out[scan].get("tool_call_id").and_then(Value::as_str)
                {
                    found_ids.insert(id.to_string());
                }
                scan += 1;
            }

            if !expected_ids.is_subset(&found_ids) {
                let missing: Vec<_> = expected_ids.difference(&found_ids).collect();
                logging::warn(format!(
                    "Stripping orphaned tool_calls from assistant message \
                     (expected {} tool results, found {}, missing: {:?})",
                    expected_ids.len(),
                    found_ids.len(),
                    missing
                ));
                if let Some(obj) = out[i].as_object_mut() {
                    obj.remove("tool_calls");
                }
                // If tool_calls were the only assistant content, remove the now-invalid
                // assistant message entirely (DeepSeek requires content or tool_calls).
                let assistant_content_empty = out[i]
                    .get("content")
                    .is_none_or(|v| v.is_null() || v.as_str().is_some_and(str::is_empty));
                if assistant_content_empty {
                    // Remove orphaned tool results tied to this stripped assistant call set.
                    let mut j = out.len();
                    while j > i + 1 {
                        j -= 1;
                        if out[j].get("role").and_then(Value::as_str) == Some("tool")
                            && let Some(id) = out[j].get("tool_call_id").and_then(Value::as_str)
                            && expected_ids.contains(id)
                        {
                            out.remove(j);
                        }
                    }
                    out.remove(i);
                    i = i.saturating_sub(1);
                    continue;
                }
                // Remove contiguous tool results first
                if tool_result_end > i + 1 {
                    out.drain((i + 1)..tool_result_end);
                }
                // Remove any remaining non-contiguous tool results referencing expected_ids
                // (scan backward to avoid index shifting issues)
                let mut j = out.len();
                while j > i + 1 {
                    j -= 1;
                    if out[j].get("role").and_then(Value::as_str) == Some("tool")
                        && let Some(id) = out[j].get("tool_call_id").and_then(Value::as_str)
                        && expected_ids.contains(id)
                    {
                        out.remove(j);
                    }
                }
            }
        }
        i += 1;
    }

    out
}

fn message_starts_user_turn(message: &Message) -> bool {
    message.role == "user"
        && message.content.iter().any(|block| match block {
            ContentBlock::Text { text, .. } => !text.trim().is_empty(),
            _ => false,
        })
}

pub(super) fn tool_to_chat(tool: &Tool) -> Value {
    let mut value = json!({
        "type": "function",
        "function": {
            "name": to_api_tool_name(&tool.name),
            "description": tool.description,
            "parameters": tool.input_schema,
        }
    });
    if let Some(allowed_callers) = &tool.allowed_callers {
        value["allowed_callers"] = json!(allowed_callers);
    }
    if let Some(defer_loading) = tool.defer_loading {
        value["defer_loading"] = json!(defer_loading);
    }
    if let Some(input_examples) = &tool.input_examples {
        value["input_examples"] = json!(input_examples);
    }
    if let Some(strict) = tool.strict
        && let Some(function) = value.get_mut("function")
    {
        function["strict"] = json!(strict);
    }
    value
}

pub(super) fn tool_to_chat_for_base_url(tool: &Tool, base_url: &str) -> Value {
    let mut value = tool_to_chat(tool);
    if !deepseek_base_url_supports_strict_tools(base_url)
        && let Some(function) = value.get_mut("function")
        && let Some(obj) = function.as_object_mut()
    {
        obj.remove("strict");
    }
    value
}

fn deepseek_base_url_supports_strict_tools(base_url: &str) -> bool {
    let trimmed = base_url.trim_end_matches('/').to_ascii_lowercase();
    let is_deepseek = trimmed == "https://api.deepseek.com"
        || trimmed == "https://api.deepseek.com/v1"
        || trimmed == "https://api.deepseek.com/beta"
        || trimmed == "https://api.deepseeki.com"
        || trimmed == "https://api.deepseeki.com/v1"
        || trimmed == "https://api.deepseeki.com/beta";
    !is_deepseek || trimmed.ends_with("/beta")
}

fn map_tool_choice_for_chat(choice: &Value) -> Option<Value> {
    if let Some(choice_str) = choice.as_str() {
        return Some(json!(choice_str));
    }
    let Some(choice_type) = choice.get("type").and_then(Value::as_str) else {
        return Some(choice.clone());
    };

    match choice_type {
        "auto" | "none" => Some(json!(choice_type)),
        "any" => Some(json!("auto")),
        "tool" => choice.get("name").and_then(Value::as_str).map(|name| {
            json!({
                "type": "function",
                "function": { "name": to_api_tool_name(name) }
            })
        }),
        _ => Some(choice.clone()),
    }
}

/// Final-pass sanitizer over the outgoing chat-completions JSON payload.
/// Forces a non-empty `reasoning_content` onto assistant messages that carry
/// `tool_calls`, when the model + effort combination requires it. DeepSeek's
/// thinking-mode API rejects such messages with a 400 error; substituting a
/// placeholder keeps the conversation chain intact. Non-tool assistant
/// reasoning can stay omitted once a later user text turn begins.
///
/// Also tallies the size of all replayed `reasoning_content` and logs it, so
/// users on `RUST_LOG=deepseek_tui=debug` can see how much of their input
/// budget is being spent re-sending prior thinking traces.
pub(super) fn sanitize_thinking_mode_messages(
    body: &mut Value,
    model: &str,
    effort: Option<&str>,
) -> Option<u32> {
    if !should_replay_reasoning_content(model, effort) {
        return None;
    }
    let messages = body.get_mut("messages").and_then(Value::as_array_mut)?;
    let mut substitutions: u32 = 0;
    let mut replay_chars: u64 = 0;
    let mut replay_messages: u32 = 0;
    for (idx, msg) in messages.iter_mut().enumerate() {
        if msg.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let has_tool_calls = msg.get("tool_calls").is_some();
        let needs_placeholder = msg
            .get("reasoning_content")
            .and_then(Value::as_str)
            .is_none_or(|s| s.trim().is_empty());
        if has_tool_calls && needs_placeholder {
            msg["reasoning_content"] = json!("(reasoning omitted)");
            substitutions = substitutions.saturating_add(1);
            logging::warn(format!(
                "Final sanitizer: forced reasoning_content placeholder on assistant[{idx}]",
            ));
        }
        if let Some(reasoning) = msg.get("reasoning_content").and_then(Value::as_str) {
            let len = reasoning.len() as u64;
            if len > 0 {
                replay_chars = replay_chars.saturating_add(len);
                replay_messages = replay_messages.saturating_add(1);
            }
        }
    }
    if substitutions > 0 {
        logging::warn(format!(
            "Final sanitizer: {substitutions} assistant message(s) needed reasoning_content placeholder",
        ));
    }
    if replay_messages == 0 {
        return None;
    }
    // ~4 chars/token is the standard rough estimate; DeepSeek tokens skew
    // a touch shorter on Chinese/code but this is order-of-magnitude info.
    let approx_tokens = (replay_chars / 4).min(u64::from(u32::MAX)) as u32;
    logging::info(format!(
        "Reasoning-content replay: {replay_messages} assistant message(s), ~{approx_tokens} input tokens ({replay_chars} chars) being re-sent in this request",
    ));
    Some(approx_tokens)
}

/// Sums the byte length of `reasoning_content` across all assistant messages in
/// an outgoing chat-completions body. Used by tests; the production sanitizer
/// computes the same number inline and logs it.
#[cfg(test)]
pub(super) fn count_reasoning_replay_chars(body: &Value) -> u64 {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return 0;
    };
    messages
        .iter()
        .filter(|m| m.get("role").and_then(Value::as_str) == Some("assistant"))
        .filter_map(|m| m.get("reasoning_content").and_then(Value::as_str))
        .map(|s| s.len() as u64)
        .sum()
}

/// Render the transport-shape headers we care about for #103 diagnostics.
/// Always returns SOMETHING printable so the decode-error log line is parseable
/// even when the server stripped a header we expected.
fn format_stream_headers(headers: &reqwest::header::HeaderMap) -> String {
    const FIELDS: &[&str] = &[
        "content-encoding",
        "transfer-encoding",
        "connection",
        "server",
    ];
    let mut parts: Vec<String> = Vec::with_capacity(FIELDS.len());
    for field in FIELDS {
        let rendered = headers
            .get(*field)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("(absent)");
        parts.push(format!("{field}={rendered}"));
    }
    parts.join(", ")
}

/// Diagnostic logger fired when DeepSeek rejects the request despite the
/// sanitizer. Walks the body and logs which assistant messages have tool_calls
/// but no `reasoning_content` — useful to track down a code path that bypasses
/// the sanitizer entirely.
fn log_thinking_mode_violations(body: &Value) {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        logging::warn("400-after-sanitizer: body has no `messages` array");
        return;
    };
    let mut violations: Vec<String> = Vec::new();
    for (idx, msg) in messages.iter().enumerate() {
        if msg.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let reasoning = msg
            .get("reasoning_content")
            .and_then(Value::as_str)
            .unwrap_or("");
        let has_tc = msg.get("tool_calls").is_some();
        if reasoning.trim().is_empty() {
            violations.push(format!(
                "assistant[{idx}] (reasoning_content missing, tool_calls={})",
                has_tc
            ));
        }
    }
    if violations.is_empty() {
        logging::warn(
            "400-after-sanitizer: all assistant messages have reasoning_content — DeepSeek rejected for a different reason",
        );
    } else {
        logging::warn(format!(
            "400-after-sanitizer: {} assistant message(s) lack reasoning_content despite sanitizer: {}",
            violations.len(),
            violations.join(", ")
        ));
    }
}

fn requires_reasoning_content(model: &str) -> bool {
    let lower = model.to_lowercase();
    lower.contains("deepseek-v4")
        || lower.contains("reasoner")
        || lower.contains("-reasoning")
        || lower.contains("-thinking")
        || has_deepseek_r_series_marker(&lower)
}

fn should_replay_reasoning_content(model: &str, effort: Option<&str>) -> bool {
    if effort
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "off" | "disabled" | "none" | "false"
            )
        })
        .unwrap_or(false)
    {
        return false;
    }

    requires_reasoning_content(model)
}

fn has_deepseek_r_series_marker(model_lower: &str) -> bool {
    const PREFIX: &str = "deepseek-r";
    model_lower.match_indices(PREFIX).any(|(idx, _)| {
        model_lower[idx + PREFIX.len()..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_digit())
    })
}

fn reasoning_field(value: &Value) -> Option<&str> {
    value
        .get("reasoning_content")
        .or_else(|| value.get("reasoning"))
        .and_then(Value::as_str)
}

pub(super) fn parse_chat_message(payload: &Value) -> Result<MessageResponse> {
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("chatcmpl")
        .to_string();
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let choices = payload
        .get("choices")
        .and_then(Value::as_array)
        .context("Chat API response missing choices")?;
    let choice = choices
        .first()
        .context("Chat API response missing first choice")?;
    let message = choice
        .get("message")
        .context("Chat API response missing message")?;

    let mut content_blocks = Vec::new();
    if let Some(reasoning) =
        reasoning_field(message).filter(|reasoning| !reasoning.trim().is_empty())
    {
        content_blocks.push(ContentBlock::Thinking {
            thinking: reasoning.to_string(),
        });
    }
    if let Some(text) = message.get("content").and_then(Value::as_str)
        && !text.trim().is_empty()
    {
        content_blocks.push(ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        });
    }

    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in tool_calls {
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("tool_call")
                .to_string();
            let function = call.get("function");
            let name = tool_name_or_fallback(
                function.and_then(|f| f.get("name")).and_then(Value::as_str),
                &id,
                "Non-streaming response",
            );
            let arguments = function
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
                .map(|raw| serde_json::from_str(raw).unwrap_or(Value::String(raw.to_string())))
                .unwrap_or(Value::Null);
            let caller = call.get("caller").and_then(|v| {
                v.get("type")
                    .and_then(Value::as_str)
                    .map(|caller_type| ToolCaller {
                        caller_type: caller_type.to_string(),
                        tool_id: v
                            .get("tool_id")
                            .and_then(Value::as_str)
                            .map(std::string::ToString::to_string),
                    })
            });

            content_blocks.push(ContentBlock::ToolUse {
                id,
                name: from_api_tool_name(&name),
                input: arguments,
                caller,
            });
        }
    }

    let usage = parse_usage(payload.get("usage"));

    Ok(MessageResponse {
        id,
        r#type: "message".to_string(),
        role: "assistant".to_string(),
        content: content_blocks,
        model,
        stop_reason: choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .map(str::to_string),
        stop_sequence: None,
        container: None,
        usage,
    })
}

// === Streaming Helpers ===

/// Build synthetic stream events from a non-streaming response (used as fallback).
#[allow(dead_code)]
fn build_stream_events(response: &MessageResponse) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    let mut index = 0u32;

    events.push(StreamEvent::MessageStart {
        message: response.clone(),
    });

    for block in &response.content {
        match block {
            ContentBlock::Text { text, .. } => {
                events.push(StreamEvent::ContentBlockStart {
                    index,
                    content_block: ContentBlockStart::Text {
                        text: String::new(),
                    },
                });
                if !text.is_empty() {
                    events.push(StreamEvent::ContentBlockDelta {
                        index,
                        delta: Delta::TextDelta { text: text.clone() },
                    });
                }
                events.push(StreamEvent::ContentBlockStop { index });
            }
            ContentBlock::Thinking { thinking } => {
                events.push(StreamEvent::ContentBlockStart {
                    index,
                    content_block: ContentBlockStart::Thinking {
                        thinking: String::new(),
                    },
                });
                if !thinking.is_empty() {
                    events.push(StreamEvent::ContentBlockDelta {
                        index,
                        delta: Delta::ThinkingDelta {
                            thinking: thinking.clone(),
                        },
                    });
                }
                events.push(StreamEvent::ContentBlockStop { index });
            }
            ContentBlock::ToolUse {
                id, name, input, ..
            } => {
                events.push(StreamEvent::ContentBlockStart {
                    index,
                    content_block: ContentBlockStart::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                        caller: None,
                    },
                });
                events.push(StreamEvent::ContentBlockStop { index });
            }
            ContentBlock::ToolResult { .. } => {}
            ContentBlock::ServerToolUse { id, name, input } => {
                events.push(StreamEvent::ContentBlockStart {
                    index,
                    content_block: ContentBlockStart::ServerToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    },
                });
                events.push(StreamEvent::ContentBlockStop { index });
            }
            ContentBlock::ToolSearchToolResult { .. }
            | ContentBlock::CodeExecutionToolResult { .. } => {}
        }
        index = index.saturating_add(1);
    }

    events.push(StreamEvent::MessageDelta {
        delta: MessageDelta {
            stop_reason: response.stop_reason.clone(),
            stop_sequence: response.stop_sequence.clone(),
        },
        usage: Some(response.usage.clone()),
    });
    events.push(StreamEvent::MessageStop);

    events
}

// === SSE Chunk Parser ===

/// Parse a single SSE chunk from the Chat Completions streaming API into
/// our internal `StreamEvent` representation.
pub(super) fn parse_sse_chunk(
    chunk: &Value,
    content_index: &mut u32,
    text_started: &mut bool,
    thinking_started: &mut bool,
    tool_indices: &mut std::collections::HashMap<u32, u32>,
    is_reasoning_model: bool,
) -> Vec<StreamEvent> {
    let mut events = Vec::new();

    let Some(choices) = chunk.get("choices").and_then(Value::as_array) else {
        // Usage-only chunk (sent at end with stream_options)
        if let Some(usage_val) = chunk.get("usage") {
            let usage = parse_usage(Some(usage_val));
            events.push(StreamEvent::MessageDelta {
                delta: MessageDelta {
                    stop_reason: None,
                    stop_sequence: None,
                },
                usage: Some(usage),
            });
        }
        return events;
    };

    if choices.is_empty() {
        if let Some(usage_val) = chunk.get("usage") {
            let usage = parse_usage(Some(usage_val));
            events.push(StreamEvent::MessageDelta {
                delta: MessageDelta {
                    stop_reason: None,
                    stop_sequence: None,
                },
                usage: Some(usage),
            });
        }
        return events;
    }

    for choice in choices {
        let delta = choice.get("delta");
        let finish_reason = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .map(str::to_string);

        if let Some(delta) = delta {
            // Handle reasoning_content / reasoning thinking deltas.
            if is_reasoning_model
                && let Some(reasoning) = reasoning_field(delta)
                && !reasoning.is_empty()
            {
                if !*thinking_started {
                    events.push(StreamEvent::ContentBlockStart {
                        index: *content_index,
                        content_block: ContentBlockStart::Thinking {
                            thinking: String::new(),
                        },
                    });
                    *thinking_started = true;
                }
                events.push(StreamEvent::ContentBlockDelta {
                    index: *content_index,
                    delta: Delta::ThinkingDelta {
                        thinking: reasoning.to_string(),
                    },
                });
            }

            // Handle regular content
            if let Some(content) = delta.get("content").and_then(Value::as_str)
                && !content.is_empty()
            {
                // Close thinking block if transitioning to text
                if *thinking_started {
                    events.push(StreamEvent::ContentBlockStop {
                        index: *content_index,
                    });
                    *content_index += 1;
                    *thinking_started = false;
                }
                if !*text_started {
                    events.push(StreamEvent::ContentBlockStart {
                        index: *content_index,
                        content_block: ContentBlockStart::Text {
                            text: String::new(),
                        },
                    });
                    *text_started = true;
                }
                events.push(StreamEvent::ContentBlockDelta {
                    index: *content_index,
                    delta: Delta::TextDelta {
                        text: content.to_string(),
                    },
                });
            }

            // Handle tool calls
            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for tc in tool_calls {
                    let tc_index = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
                    let tool_block_index = match tool_indices.entry(tc_index) {
                        std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            // Close text block if transitioning to tool use
                            if *text_started {
                                events.push(StreamEvent::ContentBlockStop {
                                    index: *content_index,
                                });
                                *content_index += 1;
                                *text_started = false;
                            }
                            if *thinking_started {
                                events.push(StreamEvent::ContentBlockStop {
                                    index: *content_index,
                                });
                                *content_index += 1;
                                *thinking_started = false;
                            }

                            let block_index = *content_index;
                            let id = tc
                                .get("id")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                // Some upstream gateways (and the responses-API
                                // bridge) elide the `id` on the first chunk of a
                                // tool call. Falling back to a constant string
                                // collides when the model emits parallel tool
                                // calls in the same delta — every call ended up
                                // with the same id and downstream tool-result
                                // routing matched the first one twice. Index by
                                // the content-block position to keep the
                                // fallback unique within the response.
                                .unwrap_or_else(|| format!("call_{block_index}"));
                            let name = tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(Value::as_str);
                            let name = tool_name_or_fallback(name, &id, "Streaming response chunk");
                            let caller = tc.get("caller").and_then(|v| {
                                v.get("type").and_then(Value::as_str).map(|caller_type| {
                                    ToolCaller {
                                        caller_type: caller_type.to_string(),
                                        tool_id: v
                                            .get("tool_id")
                                            .and_then(Value::as_str)
                                            .map(std::string::ToString::to_string),
                                    }
                                })
                            });

                            events.push(StreamEvent::ContentBlockStart {
                                index: block_index,
                                content_block: ContentBlockStart::ToolUse {
                                    id,
                                    name: from_api_tool_name(&name),
                                    input: json!({}),
                                    caller,
                                },
                            });
                            *content_index = (*content_index).saturating_add(1);
                            entry.insert(block_index);
                            block_index
                        }
                    };

                    // Stream tool call arguments
                    if let Some(args) = tc
                        .get("function")
                        .and_then(|f| f.get("arguments"))
                        .and_then(Value::as_str)
                        && !args.is_empty()
                    {
                        events.push(StreamEvent::ContentBlockDelta {
                            index: tool_block_index,
                            delta: Delta::InputJsonDelta {
                                partial_json: args.to_string(),
                            },
                        });
                    }
                }
            }
        }

        // Handle finish reason
        if let Some(reason) = finish_reason {
            // Close any open blocks
            if *text_started {
                events.push(StreamEvent::ContentBlockStop {
                    index: *content_index,
                });
                *text_started = false;
            }
            if *thinking_started {
                events.push(StreamEvent::ContentBlockStop {
                    index: *content_index,
                });
                *thinking_started = false;
            }
            // Close tool blocks
            let mut open_tool_indices: Vec<u32> =
                tool_indices.drain().map(|(_, idx)| idx).collect();
            open_tool_indices.sort_unstable();
            for tool_block_index in open_tool_indices {
                events.push(StreamEvent::ContentBlockStop {
                    index: tool_block_index,
                });
            }

            // Emit usage from the chunk if available
            let chunk_usage = chunk.get("usage").map(|u| parse_usage(Some(u)));
            events.push(StreamEvent::MessageDelta {
                delta: MessageDelta {
                    stop_reason: Some(reason),
                    stop_sequence: None,
                },
                usage: chunk_usage,
            });
        }
    }

    events
}

fn tool_name_or_fallback(name: Option<&str>, id: &str, source: &str) -> String {
    let trimmed = name.unwrap_or("").trim();
    if trimmed.is_empty() {
        logging::warn(format!(
            "{source} returned an empty tool name for call {id}; using unknown_tool"
        ));
        "unknown_tool".to_string()
    } else {
        trimmed.to_string()
    }
}

// === #103 Phase 1: stream-decode diagnostics ===================================

#[cfg(test)]
mod stream_diagnostics_tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

    #[test]
    fn stream_open_timeout_defaults_and_clamps_env_values() {
        assert_eq!(stream_open_timeout_from_env(None), Duration::from_secs(45));
        assert_eq!(
            stream_open_timeout_from_env(Some("not-a-number")),
            Duration::from_secs(45)
        );
        assert_eq!(
            stream_open_timeout_from_env(Some("1")),
            Duration::from_secs(5)
        );
        assert_eq!(
            stream_open_timeout_from_env(Some("120")),
            Duration::from_secs(120)
        );
        assert_eq!(
            stream_open_timeout_from_env(Some("999")),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn format_stream_headers_renders_all_fields_when_present() {
        let mut headers = HeaderMap::new();
        headers.insert("content-encoding", HeaderValue::from_static("gzip"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.insert("connection", HeaderValue::from_static("keep-alive"));
        headers.insert("server", HeaderValue::from_static("openresty/1.25.3.1"));

        let rendered = format_stream_headers(&headers);
        // Order is fixed by FIELDS in the helper; assert each field appears.
        assert!(
            rendered.contains("content-encoding=gzip"),
            "got: {rendered}"
        );
        assert!(
            rendered.contains("transfer-encoding=chunked"),
            "got: {rendered}"
        );
        assert!(
            rendered.contains("connection=keep-alive"),
            "got: {rendered}"
        );
        assert!(
            rendered.contains("server=openresty/1.25.3.1"),
            "got: {rendered}"
        );
    }

    #[test]
    fn format_stream_headers_marks_missing_fields_as_absent() {
        // DeepSeek frequently omits content-encoding when not compressing.
        // The diagnostic must still produce a parseable line so log scrapers
        // don't lose the slot.
        let headers = HeaderMap::new();
        let rendered = format_stream_headers(&headers);
        assert!(
            rendered.contains("content-encoding=(absent)"),
            "missing field must be explicitly marked; got: {rendered}"
        );
        assert!(
            rendered.contains("transfer-encoding=(absent)"),
            "missing field must be explicitly marked; got: {rendered}"
        );
    }

    #[test]
    fn format_stream_headers_handles_non_ascii_value_gracefully() {
        // If a header value isn't UTF-8, `.to_str()` fails — we must not panic
        // and should still produce a parseable line.
        let mut headers = HeaderMap::new();
        // 0xFF is a valid byte but invalid UTF-8 start byte.
        headers.insert(
            "server",
            HeaderValue::from_bytes(b"\xff\xfemystery").expect("header value"),
        );
        let rendered = format_stream_headers(&headers);
        assert!(
            rendered.contains("server=(absent)"),
            "non-UTF8 header values fall back to (absent); got: {rendered}"
        );
    }
}

// === #103 Phase 4: SSE decoder behavior on canned chunk sequences ============

#[cfg(test)]
mod stream_decoder_tests {
    //! Drive `parse_sse_chunk` (the in-place SSE event extractor) over canned
    //! chunk sequences. The full `handle_chat_completion_stream` path needs a
    //! live `reqwest::Response` so it isn't unit-testable without a mock HTTP
    //! harness (issue #69 tracks that). For #103 we exercise the chunk decoder
    //! directly to verify each "class of stream failure" the engine relies on.
    use super::*;
    use crate::models::{ContentBlockStart, Delta, StreamEvent};

    /// Decode a raw SSE-data JSON chunk into our internal events, mirroring
    /// the per-event call shape used by `handle_chat_completion_stream`.
    fn decode_chunk(json_text: &str) -> Vec<StreamEvent> {
        let chunk: Value = serde_json::from_str(json_text).expect("valid SSE JSON");
        let mut content_index = 0u32;
        let mut text_started = false;
        let mut thinking_started = false;
        let mut tool_indices = std::collections::HashMap::new();
        parse_sse_chunk(
            &chunk,
            &mut content_index,
            &mut text_started,
            &mut thinking_started,
            &mut tool_indices,
            true,
        )
    }

    #[test]
    fn decoder_emits_text_delta_for_content_chunk() {
        // The "happy" first chunk: a normal content delta. The engine treats
        // this as `any_content_received = true` and would NOT transparently
        // retry on a subsequent error.
        let events = decode_chunk(r#"{"choices":[{"delta":{"content":"hello"}}]}"#);
        assert!(
            matches!(
                events.first(),
                Some(StreamEvent::ContentBlockStart {
                    content_block: ContentBlockStart::Text { .. },
                    ..
                })
            ),
            "first event should open a text block; got {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::ContentBlockDelta {
                    delta: Delta::TextDelta { text },
                    ..
                } if text == "hello")),
            "should yield a TextDelta carrying 'hello'; got {events:?}"
        );
    }

    #[test]
    fn decoder_emits_thinking_delta_for_reasoning_chunk() {
        // V4 thinking models surface reasoning_content first — the engine
        // also counts these as content received (so a subsequent stream error
        // surfaces rather than retrying transparently).
        let events = decode_chunk(r#"{"choices":[{"delta":{"reasoning_content":"plan..."}}]}"#);
        assert!(
            matches!(
                events.first(),
                Some(StreamEvent::ContentBlockStart {
                    content_block: ContentBlockStart::Thinking { .. },
                    ..
                })
            ),
            "first event should open a thinking block; got {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::ContentBlockDelta {
                    delta: Delta::ThinkingDelta { thinking },
                    ..
                } if thinking == "plan...")),
            "should yield a ThinkingDelta carrying 'plan...'; got {events:?}"
        );
    }

    #[test]
    fn decoder_yields_no_events_for_keepalive_chunk() {
        // DeepSeek often sends `{"choices":[]}` keepalive chunks before
        // emitting real content. The engine MUST treat a stream error after
        // these as "no content received" and be eligible for transparent
        // retry — assert here that the decoder yields no payload events.
        let events = decode_chunk(r#"{"choices":[]}"#);
        assert!(
            events.is_empty(),
            "empty-choices chunk must produce no events; got {events:?}"
        );
    }

    #[test]
    fn decoder_emits_tool_use_block_for_tool_call_delta() {
        // Tool-call deltas are content too — once one arrives, transparent
        // retry must be off (the model has committed to a tool invocation
        // path that DeepSeek has billed for).
        let events = decode_chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"grep_files","arguments":"{\"pattern\":\"foo\"}"}}]}}]}"#,
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                StreamEvent::ContentBlockStart {
                    content_block: ContentBlockStart::ToolUse { name, .. },
                    ..
                } if name == "grep_files"
            )),
            "should open a ToolUse block for grep_files; got {events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                StreamEvent::ContentBlockDelta {
                    delta: Delta::InputJsonDelta { partial_json },
                    ..
                } if partial_json.contains("\"pattern\"")
            )),
            "should yield InputJsonDelta carrying the tool args; got {events:?}"
        );
    }

    #[test]
    fn decoder_uses_fallback_name_for_empty_streaming_tool_name() {
        let events = decode_chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_empty","function":{"name":"","arguments":"{}"}}]}}]}"#,
        );

        assert!(
            events.iter().any(|event| matches!(
                event,
                StreamEvent::ContentBlockStart {
                    content_block: ContentBlockStart::ToolUse { name, .. },
                    ..
                } if name == "unknown_tool"
            )),
            "empty upstream tool names should render as unknown_tool; got {events:?}"
        );
    }

    #[test]
    fn non_streaming_response_uses_fallback_name_for_missing_tool_name() {
        let payload: Value = serde_json::from_str(
            r#"{
                "id": "chatcmpl_1",
                "model": "deepseek-v4-pro",
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "tool_calls": [{
                            "id": "call_missing",
                            "function": { "arguments": "{}" }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }"#,
        )
        .expect("valid response");

        let parsed = parse_chat_message(&payload).expect("message parses");
        let tool_name = parsed.content.iter().find_map(|block| match block {
            ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
            _ => None,
        });

        assert_eq!(tool_name, Some("unknown_tool"));
    }

    /// Regression for the parallel-tool-calls-without-id collision (audit
    /// Finding 8): when the upstream chunk omits the `id` field, the
    /// fallback used to be the literal string `"tool_call"` for every
    /// parallel call, so two tool calls in one delta ended up sharing an
    /// id. Downstream routing then matched the first call's tool_result
    /// twice and the second call hung. The fallback is now indexed by the
    /// content-block position, keeping each call unique within the
    /// response.
    #[test]
    fn decoder_assigns_unique_fallback_ids_to_parallel_tool_calls_missing_id() {
        let events = decode_chunk(
            r#"{"choices":[{"delta":{"tool_calls":[
                {"index":0,"function":{"name":"grep_files","arguments":"{\"pattern\":\"a\"}"}},
                {"index":1,"function":{"name":"read_file","arguments":"{\"path\":\"x\"}"}}
            ]}}]}"#,
        );

        let ids: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ContentBlockStart {
                    content_block: ContentBlockStart::ToolUse { id, .. },
                    ..
                } => Some(id.as_str()),
                _ => None,
            })
            .collect();

        assert_eq!(
            ids.len(),
            2,
            "expected two tool-use blocks for parallel tool calls; got {events:?}"
        );
        assert_ne!(
            ids[0], ids[1],
            "parallel tool calls without upstream `id` must get distinct fallback ids; got {ids:?}"
        );
    }

    #[test]
    fn decoder_preserves_upstream_tool_call_id_when_present() {
        // Counter-test to the fallback regression: when the upstream chunk
        // does include `id`, we forward it verbatim — we shouldn't quietly
        // rewrite ids the API gave us just because we have a fallback path.
        let events = decode_chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_xyz","function":{"name":"grep_files","arguments":"{}"}}]}}]}"#,
        );
        let id = events
            .iter()
            .find_map(|e| match e {
                StreamEvent::ContentBlockStart {
                    content_block: ContentBlockStart::ToolUse { id, .. },
                    ..
                } => Some(id.as_str()),
                _ => None,
            })
            .expect("tool-use block present");
        assert_eq!(id, "call_xyz");
    }

    #[test]
    fn request_builder_preserves_internal_system_messages() {
        let messages = vec![Message {
            role: "system".to_string(),
            content: vec![ContentBlock::Text {
                text: "internal runtime event".to_string(),
                cache_control: None,
            }],
        }];

        let built = build_chat_messages(None, &messages, "deepseek-v4-flash");

        assert_eq!(built.len(), 1);
        assert_eq!(built[0]["role"], "system");
        assert_eq!(built[0]["content"], "internal runtime event");
    }

    fn tool_use_message(id: &str, name: &str, input: Value) -> Message {
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input,
                caller: None,
            }],
        }
    }

    fn tool_result_message(id: &str, content: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: content.to_string(),
                is_error: None,
                content_blocks: None,
            }],
        }
    }

    fn user_message_with_turn_meta(turn_meta: &str, task: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: turn_meta.to_string(),
                    cache_control: None,
                },
                ContentBlock::Text {
                    text: task.to_string(),
                    cache_control: None,
                },
            ],
        }
    }

    fn tool_message_content(messages: &[Value], index: usize) -> &str {
        messages
            .iter()
            .filter(|message| message.get("role").and_then(Value::as_str) == Some("tool"))
            .nth(index)
            .and_then(|message| message.get("content").and_then(Value::as_str))
            .expect("tool message content")
    }

    fn user_message_content(messages: &[Value], index: usize) -> &str {
        messages
            .iter()
            .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
            .nth(index)
            .and_then(|message| message.get("content").and_then(Value::as_str))
            .expect("user message content")
    }

    #[test]
    fn request_builder_deduplicates_consecutive_identical_turn_meta_for_wire() {
        let turn_meta = "<turn_meta>\nCurrent local date: 2026-05-09\n</turn_meta>";
        let messages = vec![
            user_message_with_turn_meta(turn_meta, "first task"),
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "first answer".to_string(),
                    cache_control: None,
                }],
            },
            user_message_with_turn_meta(turn_meta, "second task"),
        ];

        let built = build_chat_messages(None, &messages, "deepseek-v4-flash");
        let first = user_message_content(&built, 0);
        let second = user_message_content(&built, 1);
        let expected_sha = sha256_hex(turn_meta.as_bytes());
        let expected_ref = format!(
            "<TURN_META_REF sha=\"{expected_sha}\" original_chars=\"{}\" />",
            turn_meta.chars().count()
        );

        assert!(first.starts_with(turn_meta), "got: {first}");
        assert!(second.starts_with(&expected_ref), "got: {second}");
        assert!(second.ends_with("second task"), "got: {second}");
        assert_eq!(
            second,
            format!("{expected_ref}\nsecond task"),
            "ref text must stay stable"
        );
    }

    #[test]
    fn request_builder_keeps_changed_turn_meta_full_and_updates_recent_hash() {
        let first_meta = "<turn_meta>\nCurrent local date: 2026-05-09\n</turn_meta>";
        let second_meta =
            "<turn_meta>\nCurrent local date: 2026-05-09\nWorking set: src/lib.rs\n</turn_meta>";
        let messages = vec![
            user_message_with_turn_meta(first_meta, "first task"),
            user_message_with_turn_meta(second_meta, "second task"),
        ];

        let built = build_chat_messages(None, &messages, "deepseek-v4-flash");
        let first = user_message_content(&built, 0);
        let second = user_message_content(&built, 1);

        assert!(first.starts_with(first_meta), "got: {first}");
        assert!(second.starts_with(second_meta), "got: {second}");
        assert!(!second.contains("<TURN_META_REF"), "got: {second}");
    }

    #[test]
    fn turn_meta_dedup_is_wire_only_and_does_not_mutate_session_message() {
        let turn_meta = "<turn_meta>\nCurrent local date: 2026-05-09\n</turn_meta>";
        let messages = vec![
            user_message_with_turn_meta(turn_meta, "first task"),
            user_message_with_turn_meta(turn_meta, "second task"),
        ];

        let built = build_chat_messages(None, &messages, "deepseek-v4-flash");
        assert!(
            user_message_content(&built, 1).starts_with("<TURN_META_REF"),
            "got: {}",
            user_message_content(&built, 1)
        );

        match &messages[1].content[0] {
            ContentBlock::Text { text, .. } => assert_eq!(text, turn_meta),
            other => panic!("expected text block, got {other:?}"),
        }
    }

    #[test]
    fn cache_inspect_reports_turn_meta_dedup_metadata() {
        let turn_meta = format!(
            "<turn_meta>\nCurrent local date: 2026-05-09\n{}\n</turn_meta>",
            "Working set: src/lib.rs\n".repeat(20)
        );
        let request = MessageRequest {
            model: "deepseek-v4-flash".to_string(),
            messages: vec![
                user_message_with_turn_meta(&turn_meta, "first task"),
                user_message_with_turn_meta(&turn_meta, "second task"),
            ],
            max_tokens: 0,
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

        let inspection = inspect_prompt_for_request(&request);
        let turn_meta_layers: Vec<_> = inspection
            .layers
            .iter()
            .filter_map(|layer| layer.turn_meta.as_ref())
            .collect();

        assert_eq!(turn_meta_layers.len(), 2);
        assert_eq!(
            turn_meta_layers[0].original_chars,
            turn_meta.chars().count()
        );
        assert_eq!(turn_meta_layers[0].sent_chars, turn_meta.chars().count());
        assert!(!turn_meta_layers[0].deduplicated);
        assert_eq!(turn_meta_layers[0].sha256, sha256_hex(turn_meta.as_bytes()));
        assert_eq!(
            turn_meta_layers[1].original_chars,
            turn_meta.chars().count()
        );
        assert!(turn_meta_layers[1].sent_chars < turn_meta_layers[1].original_chars);
        assert!(turn_meta_layers[1].deduplicated);
        assert_eq!(turn_meta_layers[1].sha256, turn_meta_layers[0].sha256);
    }

    #[test]
    fn request_builder_truncates_large_tool_result_for_wire() {
        let long_output = format!("{}{}", "A".repeat(7_000), "Z".repeat(7_000));
        let messages = vec![
            tool_use_message(
                "tool-long",
                "shell_command",
                json!({"command": "cargo test"}),
            ),
            tool_result_message("tool-long", &long_output),
        ];

        let built = build_chat_messages(None, &messages, "deepseek-v4-flash");
        let sent = tool_message_content(&built, 0);

        assert!(sent.contains("[TOOL_RESULT_TRUNCATED]"), "got: {sent}");
        assert!(sent.contains("tool_name: shell_command"), "got: {sent}");
        assert!(sent.contains("command_or_query: cargo test"), "got: {sent}");
        assert!(sent.contains("original_chars: 14000"), "got: {sent}");
        assert!(sent.contains("sha256:"), "got: {sent}");
        assert!(sent.contains(&"A".repeat(4_000)), "got: {sent}");
        assert!(sent.contains(&"Z".repeat(4_000)), "got: {sent}");
        assert!(
            sent.contains("truncated 6000 chars from middle"),
            "got: {sent}"
        );
        assert_ne!(sent, long_output);
    }

    #[test]
    fn request_builder_deduplicates_identical_tool_results_for_wire() {
        let output = "same tool output";
        let messages = vec![
            tool_use_message("tool-1", "read_file", json!({"path": "README.md"})),
            tool_result_message("tool-1", output),
            tool_use_message("tool-2", "read_file", json!({"path": "README.md"})),
            tool_result_message("tool-2", output),
        ];

        let built = build_chat_messages(None, &messages, "deepseek-v4-flash");
        let first = tool_message_content(&built, 0);
        let second = tool_message_content(&built, 1);

        assert_eq!(first, output);
        assert!(
            second.starts_with("<TOOL_RESULT_REF sha=\""),
            "got: {second}"
        );
        assert!(
            second.contains("original_message=\"Message #1\""),
            "got: {second}"
        );
        assert!(second.contains("chars=\"16\""), "got: {second}");
    }

    #[test]
    fn tool_result_budget_is_wire_only_and_does_not_mutate_session_message() {
        let long_output = format!("{}{}", "A".repeat(7_000), "Z".repeat(7_000));
        let messages = vec![
            tool_use_message(
                "tool-long",
                "shell_command",
                json!({"command": "cargo test"}),
            ),
            tool_result_message("tool-long", &long_output),
        ];

        let built = build_chat_messages(None, &messages, "deepseek-v4-flash");
        let sent = tool_message_content(&built, 0);
        assert_ne!(sent, long_output);

        match &messages[1].content[0] {
            ContentBlock::ToolResult { content, .. } => assert_eq!(content, &long_output),
            other => panic!("expected tool result, got {other:?}"),
        }
    }

    #[test]
    fn cache_inspect_reports_tool_result_budget_metadata() {
        let long_output = format!("{}{}", "A".repeat(7_000), "Z".repeat(7_000));
        let request = MessageRequest {
            model: "deepseek-v4-flash".to_string(),
            messages: vec![
                tool_use_message("tool-1", "shell_command", json!({"command": "cargo test"})),
                tool_result_message("tool-1", &long_output),
                tool_use_message("tool-2", "shell_command", json!({"command": "cargo test"})),
                tool_result_message("tool-2", &long_output),
            ],
            max_tokens: 0,
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

        let inspection = inspect_prompt_for_request(&request);
        let tool_layers: Vec<_> = inspection
            .layers
            .iter()
            .filter_map(|layer| layer.tool_result.as_ref())
            .collect();

        assert_eq!(tool_layers.len(), 2);
        assert_eq!(tool_layers[0].original_chars, 14_000);
        assert!(tool_layers[0].sent_chars < tool_layers[0].original_chars);
        assert!(tool_layers[0].truncated);
        assert!(!tool_layers[0].deduplicated);
        assert_eq!(tool_layers[1].original_chars, 14_000);
        assert!(tool_layers[1].sent_chars < 200);
        assert!(!tool_layers[1].truncated);
        assert!(tool_layers[1].deduplicated);
    }
}
