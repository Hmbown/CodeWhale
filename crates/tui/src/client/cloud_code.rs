//! Google Antigravity / `agy` cloud-code wire (`/v1internal`).
//!
//! This is not OpenAI-compat. The official `agy` CLI speaks
//! `POST {base}:streamGenerateContent?alt=sse` with a GenerateContent JSON
//! body. Anything we have not seen on the wire fails closed.

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::llm_client::StreamEventBox;
use crate::models::{
    ContentBlock, ContentBlockStart, Delta, MessageRequest, MessageResponse, StreamEvent,
    SystemPrompt,
};

use super::PreparedOutboundRequest;
use super::prepared::WireDialect;
use super::role_placement::{RolePlacement, role_placement};
use super::stream_entry;

/// Model id advertised only after a live cloud-code turn succeeds.
#[cfg(test)]
pub const GEMINI_37_FLASH: &str = "gemini-3.7-flash";

/// Semantic request-shape failures that must remain typed until the host
/// chooses localized user-facing prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CloudCodeRequestError {
    #[error("cloud-code request would omit non-empty system instructions")]
    SystemPromptUnsupported,
}

/// Build the cloud-code streaming URL from the configured `/v1internal` base.
#[must_use]
pub fn stream_generate_content_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    format!("{base}:streamGenerateContent?alt=sse")
}

/// Minimum GenerateContent JSON body. Tools, images, and unknown roles fail
/// closed — those shapes are unproven on this wire.
pub fn build_generate_content_body(request: &MessageRequest) -> Result<Value> {
    let has_system_text = match request.system.as_ref() {
        Some(SystemPrompt::Text(text)) => !text.trim().is_empty(),
        Some(SystemPrompt::Blocks(blocks)) => {
            blocks.iter().any(|block| !block.text.trim().is_empty())
        }
        None => false,
    };
    if has_system_text {
        return Err(CloudCodeRequestError::SystemPromptUnsupported.into());
    }
    if request
        .tools
        .as_ref()
        .is_some_and(|tools| !tools.is_empty())
    {
        bail!(
            "Antigravity cloud-code tools are not implemented yet; send a text-only turn or use the google provider"
        );
    }
    let mut contents = Vec::new();
    for message in &request.messages {
        // Fail closed, as this wire always has: the shared placement table
        // says only user and assistant output are representable here, and
        // anything else is refused rather than guessed at or dropped.
        let role = match role_placement(&message.role, WireDialect::GoogleCloudCode) {
            RolePlacement::User => "user",
            RolePlacement::Assistant => "model",
            // Listed rather than caught by `_` so a new placement forces a
            // decision here instead of silently becoming a hard error.
            RolePlacement::InterruptedAssistant
            | RolePlacement::System
            | RolePlacement::Developer
            | RolePlacement::Omitted
            | RolePlacement::Rejected => bail!(
                "Antigravity cloud-code does not accept role {:?}",
                message.role.as_str()
            ),
        };
        let mut parts = Vec::new();
        for block in &message.content {
            match block {
                ContentBlock::Text { text, .. } if !text.trim().is_empty() => {
                    parts.push(json!({ "text": text }));
                }
                ContentBlock::Text { .. } => {}
                _ => bail!(
                    "Antigravity cloud-code accepts text parts only; non-text content fails closed"
                ),
            }
        }
        if !parts.is_empty() {
            contents.push(json!({ "role": role, "parts": parts }));
        }
    }
    if contents.is_empty() {
        bail!("Antigravity cloud-code request has no text contents");
    }
    let model = request.model.trim();
    if model.is_empty() {
        bail!("Antigravity cloud-code request is missing a model id");
    }
    Ok(json!({
        "model": model,
        "userAgent": "codewhale",
        "request": {
            "contents": contents,
        }
    }))
}

/// Pull visible text out of a cloud-code SSE JSON object. Unknown shapes
/// return `None` so the caller can fail closed instead of guessing.
pub fn extract_cloud_code_text(value: &Value) -> Option<String> {
    if let Some(text) = value.pointer("/response/candidates/0/content/parts/0/text") {
        return text.as_str().filter(|s| !s.is_empty()).map(str::to_string);
    }
    if let Some(text) = value.pointer("/candidates/0/content/parts/0/text") {
        return text.as_str().filter(|s| !s.is_empty()).map(str::to_string);
    }
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return (!text.is_empty()).then(|| text.to_string());
    }
    None
}

impl super::DeepSeekClient {
    pub(super) async fn handle_cloud_code_stream(
        &self,
        prepared: &PreparedOutboundRequest,
    ) -> Result<StreamEventBox> {
        let url = prepared.endpoint.url.clone();
        let body = prepared.body.clone();
        let open_req = stream_entry::StreamOpenRequest::new(
            stream_entry::stream_open_timeout(),
            self.stream_idle_timeout,
        );
        let opened = stream_entry::open_sse_response(&open_req, |policy| {
            let url = url.clone();
            let body = body.clone();
            async move {
                self.wait_for_rate_limit().await;
                let client = stream_entry::client_for_policy(
                    &self.http_client,
                    self.http1_fallback_client(),
                    policy,
                );
                client
                    .post(&url)
                    .header("Accept", "text/event-stream")
                    .json(&body)
                    .send()
                    .await
                    .context("Antigravity cloud-code request failed")
            }
        })
        .await;
        let response = match opened {
            Ok(response) => response,
            Err(err) => {
                self.mark_request_failure(&format!("cloud-code stream open: {err}"))
                    .await;
                return Err(err);
            }
        };
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let redacted = crate::llm_client::sanitize_http_error_body(
                Some("antigravity"),
                status.as_u16(),
                &body,
            );
            bail!("Antigravity cloud-code HTTP {status}: {redacted}");
        }

        let stream_idle_timeout = self.stream_idle_timeout;
        let byte_stream = response.bytes_stream();
        let stream = async_stream::stream! {
            let mut buffer: Vec<u8> = Vec::new();
            let stream_start = std::time::Instant::now();
            let mut last_chunk_at = std::time::Instant::now();
            let mut bytes_received: usize = 0;
            let mut started = false;
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
                        yield Err(anyhow::anyhow!(stream_entry::idle_timeout_message(
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

                loop {
                    let line = match super::take_sse_line(&mut buffer) {
                        Ok(Some(line)) => line,
                        Ok(None) => break,
                        Err(err) => {
                            yield Err(anyhow::anyhow!("{err}"));
                            return;
                        }
                    };
                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }
                    let Some(data) = super::extract_sse_data_value(&line) else {
                        continue;
                    };
                    if data == "[DONE]" {
                        break;
                    }
                    let value: Value = match serde_json::from_str(data) {
                        Ok(value) => value,
                        Err(err) => {
                            yield Err(anyhow::anyhow!(
                                "Antigravity cloud-code SSE is not JSON: {err}"
                            ));
                            return;
                        }
                    };
                    if let Some(error) = value.get("error") {
                        yield Ok(StreamEvent::Error {
                            error: error.clone(),
                        });
                        return;
                    }
                    let Some(text) = extract_cloud_code_text(&value) else {
                        if value.get("response").is_some() || value.get("candidates").is_some() {
                            continue;
                        }
                        yield Err(anyhow::anyhow!(
                            "Antigravity cloud-code SSE shape is unproven; failing closed"
                        ));
                        return;
                    };
                    if !started {
                        started = true;
                        yield Ok(StreamEvent::MessageStart {
                            message: MessageResponse {
                                id: "agy".to_string(),
                                r#type: "message".to_string(),
                                role: "assistant".to_string(),
                                content: Vec::new(),
                                model: String::new(),
                                stop_reason: None,
                                stop_sequence: None,
                                container: None,
                                usage: crate::models::Usage::default(),
                            },
                        });
                        yield Ok(StreamEvent::ContentBlockStart {
                            index: 0,
                            content_block: ContentBlockStart::Text {
                                text: String::new(),
                            },
                        });
                    }
                    yield Ok(StreamEvent::ContentBlockDelta {
                        index: 0,
                        delta: Delta::TextDelta { text },
                    });
                }
            }
            if started {
                yield Ok(StreamEvent::ContentBlockStop { index: 0 });
                yield Ok(StreamEvent::MessageStop);
            } else {
                yield Err(anyhow::anyhow!(
                    "Antigravity cloud-code stream ended without a text part"
                ));
            }
        };
        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Role;
    use crate::models::{Message, MessageRequest, SystemBlock, SystemPrompt};

    fn text_request(model: &str, prompt: &str) -> MessageRequest {
        MessageRequest {
            model: model.to_string(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: prompt.to_string(),
                    cache_control: None,
                }],
            }],
            max_tokens: 32,
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: Some(true),
            temperature: None,
            top_p: None,
        }
    }

    #[test]
    fn stream_url_uses_v1internal_colon_rpc() {
        assert_eq!(
            stream_generate_content_url("https://cloudcode-pa.googleapis.com/v1internal"),
            "https://cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn generate_content_body_is_text_only() {
        let body = build_generate_content_body(&text_request(GEMINI_37_FLASH, "ping")).unwrap();
        assert_eq!(body["model"], GEMINI_37_FLASH);
        assert_eq!(body["request"]["contents"][0]["parts"][0]["text"], "ping");
    }

    #[tokio::test]
    #[ignore = "live Antigravity cloud-code; run with --ignored"]
    async fn live_gemini_37_flash_one_turn() {
        let mut config = crate::config::Config::load(None, None).expect("load config");
        config.provider = Some("antigravity".to_string());
        config.default_text_model = Some(GEMINI_37_FLASH.to_string());
        eprintln!(
            "agy live creds: ANTIGRAVITY_API_KEY={} AGY_ADC_AUTH={}",
            if std::env::var("ANTIGRAVITY_API_KEY").is_ok_and(|v| !v.trim().is_empty()) {
                "set"
            } else {
                "unset"
            },
            if std::env::var("AGY_ADC_AUTH").is_ok_and(|v| !v.trim().is_empty()) {
                "set"
            } else {
                "unset"
            }
        );
        let client = match crate::client::DeepSeekClient::new(&config) {
            Ok(client) => client,
            Err(err) => {
                panic!("antigravity client did not resolve a sendable credential: {err}");
            }
        };
        let request = text_request(GEMINI_37_FLASH, "Reply with the single word pong.");
        let mut stream = crate::llm_client::LlmClient::create_message_stream(&client, request)
            .await
            .expect("cloud-code stream opened");
        let mut text = String::new();
        while let Some(event) = futures_util::StreamExt::next(&mut stream).await {
            match event.expect("stream event") {
                StreamEvent::ContentBlockDelta {
                    delta: Delta::TextDelta { text: chunk },
                    ..
                } => text.push_str(&chunk),
                StreamEvent::Error { error } => {
                    panic!("cloud-code error object (redacted shape): {error}");
                }
                StreamEvent::MessageStop => break,
                _ => {}
            }
        }
        assert!(
            !text.trim().is_empty(),
            "live Gemini 3.7 Flash turn returned no text"
        );
        eprintln!(
            "agy live turn ok: {} chars, first word {:?}",
            text.chars().count(),
            text.split_whitespace().next()
        );
    }

    #[test]
    fn generate_content_body_rejects_tools() {
        let mut request = text_request(GEMINI_37_FLASH, "ping");
        request.tools = Some(vec![crate::models::Tool {
            tool_type: None,
            name: "read".to_string(),
            description: "read".to_string(),
            input_schema: json!({"type": "object"}),
            allowed_callers: None,
            defer_loading: None,
            input_examples: None,
            strict: None,
            cache_control: None,
        }]);
        assert!(build_generate_content_body(&request).is_err());
    }

    #[test]
    fn generate_content_body_rejects_text_system_prompt_instead_of_dropping_it() {
        let mut request = text_request(GEMINI_37_FLASH, "ping");
        request.system = Some(SystemPrompt::Text("Keep this instruction".to_string()));

        let error = build_generate_content_body(&request).unwrap_err();

        assert!(matches!(
            error.downcast_ref::<CloudCodeRequestError>(),
            Some(CloudCodeRequestError::SystemPromptUnsupported)
        ));
    }

    #[test]
    fn generate_content_body_rejects_block_system_prompt_instead_of_dropping_it() {
        let mut request = text_request(GEMINI_37_FLASH, "ping");
        request.system = Some(SystemPrompt::Blocks(vec![SystemBlock {
            block_type: "text".to_string(),
            text: "Keep this structured instruction".to_string(),
            cache_control: None,
        }]));

        let error = build_generate_content_body(&request).unwrap_err();

        assert!(matches!(
            error.downcast_ref::<CloudCodeRequestError>(),
            Some(CloudCodeRequestError::SystemPromptUnsupported)
        ));
    }

    #[test]
    fn generate_content_body_accepts_semantically_empty_system_prompt() {
        let mut request = text_request(GEMINI_37_FLASH, "ping");
        request.system = Some(SystemPrompt::Blocks(vec![SystemBlock {
            block_type: "text".to_string(),
            text: " \n\t".to_string(),
            cache_control: None,
        }]));

        assert!(build_generate_content_body(&request).is_ok());
    }
}
