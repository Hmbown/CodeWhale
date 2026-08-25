use std::collections::VecDeque;
#[cfg(test)]
use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::StatusCode;
use reqwest::header::CONTENT_TYPE;

use super::headers::{apply_safe_custom_headers, with_default_mcp_http_headers};
use super::oauth;
use super::wire::{MAX_MCP_RESPONSE_BYTES, parse_sse_message_data};
use super::{ERROR_BODY_PREVIEW_BYTES, McpHttpAuth, bounded_body_excerpt, mask_url_secrets};

pub(super) struct StreamableHttpTransport {
    pub(super) client: reqwest::Client,
    pub(super) url: String,
    /// Request-time auth and custom header resolver for outbound POSTs.
    pub(super) auth: McpHttpAuth,
    pending_messages: VecDeque<Vec<u8>>,
    /// Per-spec MCP session identifier returned by the server in the
    /// first response (typically the `initialize` response). Attached
    /// as the `Mcp-Session-Id` header on every subsequent outbound
    /// request so the server can correlate messages within the same
    /// session.
    pub(super) session_id: Option<String>,
    /// Test double for [`oauth::McpOAuthRuntime::force_refresh`] so the
    /// 401/403 retry-once path can be exercised on loopback without a
    /// real AuthorizationManager.
    #[cfg(test)]
    test_force_refresh: Option<Arc<dyn Fn() -> Result<()> + Send + Sync>>,
}

#[derive(Debug)]
pub(super) enum StreamableSendError {
    Incompatible(String),
    StaleSession(String),
    Other(anyhow::Error),
}

impl StreamableHttpTransport {
    pub(super) fn new(client: reqwest::Client, url: String, auth: McpHttpAuth) -> Self {
        Self {
            client,
            url,
            auth,
            pending_messages: VecDeque::new(),
            session_id: None,
            #[cfg(test)]
            test_force_refresh: None,
        }
    }

    /// Install a test-only OAuth refresh hook (T4 loopback).
    #[cfg(test)]
    pub(super) fn with_test_oauth_refresh(
        mut self,
        refresh: impl Fn() -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.test_force_refresh = Some(Arc::new(refresh));
        self
    }

    fn has_refreshable_oauth(&self) -> bool {
        #[cfg(test)]
        if self.test_force_refresh.is_some() {
            return true;
        }
        self.auth.oauth.is_some()
    }

    async fn try_force_oauth_refresh(&self) -> Option<Result<()>> {
        #[cfg(test)]
        if let Some(hook) = self.test_force_refresh.as_ref() {
            return Some(hook());
        }
        match self.auth.oauth.as_ref() {
            Some(oauth) => Some(oauth.force_refresh().await),
            None => None,
        }
    }

    pub(super) async fn send(
        &mut self,
        msg: Vec<u8>,
    ) -> std::result::Result<(), StreamableSendError> {
        // Reactive OAuth recovery (T4): a 401/403 may mean the server no
        // longer accepts a token that the local expiry clock still trusts.
        // Retry once after a forced refresh; then surface a login hint instead
        // of a raw rejection that reads like a broken server.
        let mut retried = false;
        loop {
            // Apply user-configured custom headers after protocol framing so
            // reserved Accept / Content-Type overrides can be filtered out.
            let headers = self
                .auth
                .resolved_headers()
                .await
                .map_err(StreamableSendError::Other)?;
            let mut request = apply_safe_custom_headers(
                with_default_mcp_http_headers(self.client.post(&self.url), true),
                &headers,
            );
            // Attach any previously captured session ID per the Streamable
            // HTTP spec so the server can correlate this request to the
            // existing session.
            if let Some(ref sid) = self.session_id {
                request = request.header("Mcp-Session-Id", sid.as_str());
            }
            let response = request
                .body(msg.clone())
                .send()
                .await
                .map_err(|err| StreamableSendError::Other(err.into()))?;

            let status = response.status();

            // Capture session ID from any response (2xx, 202, 4xx, ...). The
            // server may return it on the `initialize` response or on a
            // best-effort GET preflight below.
            if let Some(sid) = response
                .headers()
                .get("Mcp-Session-Id")
                .and_then(|v| v.to_str().ok())
                && self.session_id.as_deref() != Some(sid)
            {
                let session_ref = crate::utils::redacted_identifier_for_log(sid);
                tracing::debug!(target: "mcp", session = %session_ref, "captured MCP session ID");
                self.session_id = Some(sid.to_string());
            }
            if status == StatusCode::ACCEPTED || status == StatusCode::NO_CONTENT {
                return Ok(());
            }

            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                if !retried {
                    match self.try_force_oauth_refresh().await {
                        Some(Ok(())) => {
                            retried = true;
                            continue;
                        }
                        Some(Err(_)) => {
                            tracing::warn!(
                                target: "mcp",
                                server = %self.auth.server_name,
                                status = %status,
                                "MCP OAuth force-refresh failed; surfacing login hint"
                            );
                            return Err(StreamableSendError::Other(auth_challenge_error(
                                &self.auth.server_name,
                                &self.url,
                                status,
                                true,
                                true,
                            )));
                        }
                        None => {}
                    }
                }
                return Err(StreamableSendError::Other(auth_challenge_error(
                    &self.auth.server_name,
                    &self.url,
                    status,
                    self.has_refreshable_oauth(),
                    false,
                )));
            }

            if !status.is_success() {
                let body_excerpt = bounded_body_excerpt(response, ERROR_BODY_PREVIEW_BYTES).await;
                let stale_session = self.session_id.is_some()
                    && is_streamable_http_stale_session_status(status, &body_excerpt);
                let body_excerpt = self.auth.server_error_preview(&body_excerpt);
                if stale_session {
                    return Err(StreamableSendError::StaleSession(format!(
                        "status={status} body={body_excerpt}"
                    )));
                }
                if is_streamable_http_incompatible_status(status) {
                    return Err(StreamableSendError::Incompatible(format!(
                        "status={status} body={body_excerpt}"
                    )));
                }
                return Err(StreamableSendError::Other(anyhow::anyhow!(
                    "MCP Streamable HTTP rejected (transport=http url={} status={}): {}",
                    mask_url_secrets(&self.url),
                    status,
                    body_excerpt,
                )));
            }

            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            // Reject an over-large declared body before reading anything (fast
            // path), then bound the read itself so chunked / length-less
            // responses cannot OOM us either — Content-Length alone does not
            // protect against a server that streams without declaring a length.
            if let Some(len) = response.content_length()
                && len > MAX_MCP_RESPONSE_BYTES as u64
            {
                return Err(StreamableSendError::Other(anyhow::anyhow!(
                    "MCP response Content-Length {len} exceeds {} bytes — aborting",
                    MAX_MCP_RESPONSE_BYTES
                )));
            }
            let body = read_body_capped(response, MAX_MCP_RESPONSE_BYTES)
                .await
                .map_err(StreamableSendError::Other)?;
            return self
                .store_response_body(content_type.as_deref(), &body)
                .map_err(StreamableSendError::Other);
        }
    }

    pub(super) async fn recv(&mut self) -> Result<Vec<u8>> {
        self.pending_messages
            .pop_front()
            .context("MCP Streamable HTTP response queue is empty")
    }

    fn store_response_body(&mut self, content_type: Option<&str>, body: &str) -> Result<()> {
        if body.trim().is_empty() {
            return Ok(());
        }

        let is_event_stream = content_type
            .map(|value| value.to_ascii_lowercase().contains("text/event-stream"))
            .unwrap_or(false)
            || body.trim_start().starts_with("event:")
            || body.trim_start().starts_with("data:");

        if is_event_stream {
            for msg in parse_sse_message_data(body) {
                self.pending_messages.push_back(msg);
            }
            return Ok(());
        }

        self.pending_messages.push_back(body.as_bytes().to_vec());
        Ok(())
    }
}

/// Fail-closed 401/403 message (T4). Names the recovery command; never
/// interpolates the refresh error or URL userinfo, so a leaked token cannot
/// persist in the log.
pub(super) fn auth_challenge_error(
    server_name: &str,
    url: &str,
    status: StatusCode,
    has_oauth: bool,
    refresh_failed: bool,
) -> anyhow::Error {
    let display_url = mask_url_secrets(url);
    let name = {
        let trimmed = server_name.trim();
        if trimmed.is_empty() {
            "this MCP server"
        } else {
            trimmed
        }
    };
    if has_oauth {
        let hint = oauth::auth_required_login_hint(name);
        if refresh_failed {
            anyhow::anyhow!(
                "MCP server '{name}' ({display_url}) rejected the request with {status} and refreshing the OAuth session failed. {hint}"
            )
        } else {
            anyhow::anyhow!(
                "MCP server '{name}' ({display_url}) rejected the request with {status}; the session is no longer accepted. {hint}"
            )
        }
    } else {
        anyhow::anyhow!(
            "MCP server '{name}' ({display_url}) rejected the request with {status}; the session is no longer accepted. Check the configured bearer token (or its environment variable)."
        )
    }
}

/// Read a response body through the byte stream, failing as soon as it
/// exceeds `max_bytes`. This bounds chunked and missing-Content-Length
/// responses exactly like declared ones (the declared-length fast path in
/// `send` only covers servers honest enough to announce their size).
/// MCP bodies are JSON or SSE, so lossy UTF-8 matches `.text()` behavior.
pub(super) async fn read_body_capped(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<String> {
    use futures_util::StreamExt;

    let mut stream = response.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("failed to read MCP response body")?;
        if buf.len().saturating_add(chunk.len()) > max_bytes {
            anyhow::bail!("MCP response body exceeds {max_bytes} bytes — aborting");
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn is_streamable_http_incompatible_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::NOT_FOUND
            | StatusCode::METHOD_NOT_ALLOWED
            | StatusCode::NOT_ACCEPTABLE
            | StatusCode::UNSUPPORTED_MEDIA_TYPE
            | StatusCode::NOT_IMPLEMENTED
    )
}

fn is_streamable_http_stale_session_status(status: StatusCode, body_excerpt: &str) -> bool {
    if status == StatusCode::NOT_FOUND {
        return true;
    }
    if status != StatusCode::BAD_REQUEST && status != StatusCode::UNAUTHORIZED {
        return false;
    }
    let body = body_excerpt.to_ascii_lowercase();
    body.contains("session") && (body.contains("expired") || body.contains("invalid"))
}
