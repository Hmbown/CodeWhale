//! Async MCP (Model Context Protocol) Implementation
//!
//! This module provides full async support for MCP servers with:
//! - Connection pooling for server reuse
//! - Automatic tool discovery via `tools/list`
//! - Configurable timeouts per-server and globally

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::StatusCode;
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex as TokioMutex;

mod headers;
pub mod oauth;

use self::headers::{apply_safe_custom_headers, with_default_mcp_http_headers};
use crate::child_env;
use crate::network_policy::{Decision, NetworkPolicyDecider, host_from_url};
use crate::plugins;
use crate::utils::write_atomic;

// === Error diagnostics helpers (#71) ===

/// Bytes of a non-2xx response body to surface in connection errors.
const ERROR_BODY_PREVIEW_BYTES: usize = 200;

fn validate_mcp_config_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        anyhow::bail!("MCP config path cannot be empty");
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        anyhow::bail!("MCP config path cannot contain '..' components");
    }
    Ok(())
}

/// Mask a URL so any embedded credentials in the userinfo portion (e.g.
/// `https://user:secret@host`) are replaced with `***`. Failures fall back to
/// the original string so we don't lose context — we never want masking to
/// produce an empty error.
fn mask_url_secrets(url: &str) -> String {
    if let Ok(parsed) = reqwest::Url::parse(url) {
        let mut clone = parsed.clone();
        if !parsed.username().is_empty() || parsed.password().is_some() {
            let _ = clone.set_username("***");
            let _ = clone.set_password(Some("***"));
        }
        return clone.to_string();
    }
    url.to_string()
}

/// Redact the userinfo segment (`username[:password]@…` portion) from
/// a proxy URL so it can be safely included in `tracing::warn!` output
/// without leaking the
/// password into the on-disk log. URLs without userinfo are returned
/// unchanged. Garbage input (no `://` scheme separator) is also returned
/// unchanged — the malformed-URL warning path is the only caller, so an
/// unparseable input is already the failure case.
fn redact_proxy_userinfo(proxy_url: &str) -> String {
    let Some(scheme_end) = proxy_url.find("://") else {
        return proxy_url.to_string();
    };
    let after_scheme = scheme_end + 3;
    // The userinfo segment ends at the next `@`, but only if that `@`
    // comes before the next `/`, `?`, or `#` (otherwise the `@` is in a
    // path / query and the URL has no userinfo at all).
    let rest = &proxy_url[after_scheme..];
    let at_idx = rest.find('@');
    let path_idx = rest.find(['/', '?', '#']);
    let userinfo_end = match (at_idx, path_idx) {
        (Some(a), Some(p)) if a < p => Some(a),
        (Some(a), None) => Some(a),
        _ => None,
    };
    if let Some(end) = userinfo_end {
        let mut out = String::with_capacity(proxy_url.len());
        out.push_str(&proxy_url[..after_scheme]);
        out.push_str("***@");
        out.push_str(&rest[end + 1..]);
        out
    } else {
        proxy_url.to_string()
    }
}

/// Mask any obvious token-like substrings in a body excerpt before surfacing
/// it. Conservative: replaces `Bearer <token>` and `api_key=...` shapes.
fn redact_body_preview(body: &str) -> String {
    let mut out = body.to_string();
    if let Some(idx) = out.to_lowercase().find("bearer ") {
        let tail_start = idx + "bearer ".len();
        if tail_start < out.len() {
            let end = out[tail_start..]
                .find(|c: char| c.is_whitespace() || c == '"' || c == ',')
                .map_or(out.len(), |off| tail_start + off);
            out.replace_range(tail_start..end, "***");
        }
    }
    for needle in ["api_key=", "apikey=", "api-key=", "token="] {
        if let Some(idx) = out.to_lowercase().find(needle) {
            let tail_start = idx + needle.len();
            let end = out[tail_start..]
                .find(|c: char| c.is_whitespace() || c == '&' || c == '"' || c == ',')
                .map_or(out.len(), |off| tail_start + off);
            out.replace_range(tail_start..end, "***");
        }
    }
    out
}

/// Read up to `max_bytes` of a reqwest Response body and produce a single-line
/// excerpt suitable for an error message. Best-effort — if the body can't be
/// read, returns the literal string `<no body>`.
async fn bounded_body_excerpt(response: reqwest::Response, max_bytes: usize) -> String {
    let body_text = response.text().await.unwrap_or_default();
    if body_text.is_empty() {
        return "<no body>".to_string();
    }
    let trimmed: String = body_text.chars().take(max_bytes).collect();
    let suffix = if body_text.len() > trimmed.len() {
        "…"
    } else {
        ""
    };
    let one_line = trimmed.replace(['\n', '\r'], " ");
    format!("{}{}", redact_body_preview(&one_line), suffix)
}

fn invalid_json_preview(bytes: &[u8]) -> String {
    let body_text = String::from_utf8_lossy(bytes);
    if body_text.is_empty() {
        return "<empty>".to_string();
    }

    let trimmed: String = body_text.chars().take(ERROR_BODY_PREVIEW_BYTES).collect();
    let suffix = if body_text.chars().count() > ERROR_BODY_PREVIEW_BYTES {
        "…"
    } else {
        ""
    };
    let one_line = trimmed.replace(['\n', '\r'], " ");
    format!("{}{}", redact_body_preview(&one_line), suffix)
}

// === Configuration Types ===

/// Full MCP configuration from mcp.json
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct McpConfig {
    #[serde(default)]
    pub timeouts: McpTimeouts,
    #[serde(default, alias = "mcpServers")]
    pub servers: HashMap<String, McpServerConfig>,
}

/// Global timeout configuration
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[allow(clippy::struct_field_names)]
pub struct McpTimeouts {
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout: u64,
    #[serde(default = "default_execute_timeout")]
    pub execute_timeout: u64,
    #[serde(default = "default_read_timeout")]
    pub read_timeout: u64,
}

fn default_connect_timeout() -> u64 {
    10
}
fn default_execute_timeout() -> u64 {
    60
}
fn default_read_timeout() -> u64 {
    120
}

impl Default for McpTimeouts {
    fn default() -> Self {
        Self {
            connect_timeout: default_connect_timeout(),
            execute_timeout: default_execute_timeout(),
            read_timeout: default_read_timeout(),
        }
    }
}

/// Configuration for a single MCP server
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerConfig {
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    pub url: Option<String>,
    /// Optional explicit HTTP transport override.
    ///
    /// By default URL-based MCP servers use Streamable HTTP first and fall
    /// back to legacy SSE only when the server rejects Streamable HTTP with
    /// a known incompatible status. Set this to `"sse"` for legacy SSE
    /// endpoints that must start with a long-lived GET endpoint discovery
    /// stream and cannot accept an initial POST to the configured URL.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(default)]
    pub connect_timeout: Option<u64>,
    #[serde(default)]
    pub execute_timeout: Option<u64>,
    #[serde(default)]
    pub read_timeout: Option<u64>,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub enabled_tools: Vec<String>,
    #[serde(default)]
    pub disabled_tools: Vec<String>,
    /// Extra HTTP headers sent with every request to this MCP server.
    /// Only the HTTP transports (streamable HTTP today; SSE in a
    /// follow-up) honor this — `command`-based stdio servers ignore it.
    ///
    /// Mirrors the `headers` field that Claude Code, Codex, and
    /// OpenCode already accept in their MCP config formats. Use it to
    /// authenticate against gateways that require a Bearer token or
    /// API key, e.g.:
    ///
    /// ```jsonc
    /// "huggingface": {
    ///     "url": "https://huggingface.co/api/mcp",
    ///     "headers": { "Authorization": "Bearer ${HF_TOKEN}" }
    /// }
    /// ```
    ///
    /// Header keys and values are passed through as-is — we do not
    /// substitute environment variables in v0.8.31. If you store a
    /// real token here, the value lives in plain text in
    /// `~/.deepseek/mcp.json`; treat that file with the same care
    /// as any other secret-bearing config.
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
    /// HTTP headers whose values are read from environment variables at request
    /// time. This keeps common bearer/API-token integrations out of mcp.json.
    #[serde(default, alias = "env_http_headers")]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub env_headers: HashMap<String, String>,
    /// Environment variable containing a bearer token. When present and set,
    /// CodeWhale sends `Authorization: Bearer <value>` for URL-based servers.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearer_token_env_var: Option<String>,
    /// OAuth scopes requested during `codewhale mcp login`.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    /// OAuth client override for MCP servers that require a pre-registered
    /// public client instead of dynamic registration.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<McpServerOAuthConfig>,
    /// Optional RFC 8707 resource parameter appended to the authorization URL.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_resource: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct McpServerOAuthConfig {
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

fn default_enabled() -> bool {
    true
}

impl McpServerConfig {
    pub fn effective_connect_timeout(&self, global: &McpTimeouts) -> u64 {
        self.connect_timeout.unwrap_or(global.connect_timeout)
    }

    pub fn effective_execute_timeout(&self, global: &McpTimeouts) -> u64 {
        self.execute_timeout.unwrap_or(global.execute_timeout)
    }

    pub fn effective_read_timeout(&self, global: &McpTimeouts) -> u64 {
        self.read_timeout.unwrap_or(global.read_timeout)
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled && !self.disabled
    }

    pub fn is_tool_enabled(&self, tool_name: &str) -> bool {
        let allowed = if self.enabled_tools.is_empty() {
            true
        } else {
            self.enabled_tools.iter().any(|t| t == tool_name)
        };
        if !allowed {
            return false;
        }
        !self.disabled_tools.iter().any(|t| t == tool_name)
    }
}

// === MCP Tool Definition ===

/// Tool discovered from an MCP server
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: serde_json::Value,
}

/// Resource discovered from an MCP server
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "mimeType", default)]
    pub mime_type: Option<String>,
}

/// Resource template discovered from an MCP server
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpResourceTemplate {
    #[serde(rename = "uriTemplate")]
    pub uri_template: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "mimeType", default)]
    pub mime_type: Option<String>,
}

/// Prompt discovered from an MCP server
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpPrompt {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub arguments: Vec<McpPromptArgument>,
}

/// Argument for an MCP prompt
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpPromptArgument {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

// === Connection State ===

/// State of an MCP connection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Ready,
    Disconnected,
}

// === McpConnection - Async Connection Management ===

// === Transport Trait ===

#[async_trait::async_trait]
pub trait McpTransport: Send + Sync {
    async fn send(&mut self, msg: Vec<u8>) -> Result<()>;
    async fn recv(&mut self) -> Result<Vec<u8>>;

    /// Graceful shutdown — stdio transports send SIGTERM to the child and
    /// give it a brief window to exit before tokio's `kill_on_drop` fires
    /// SIGKILL as the backstop. Default is a no-op for non-stdio transports
    /// that have no child process. Whalescale#420.
    async fn shutdown(&mut self) {}
}

pub struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    reader: tokio::io::BufReader<ChildStdout>,
    /// Tail of stderr lines from the spawned MCP server. A background task
    /// drains the child's stderr into this buffer so a mid-run crash leaves
    /// some context behind instead of `Stdio::null` swallowing it.
    stderr_tail: Arc<StderrTail>,
}

/// How long `StdioTransport::shutdown` waits for the child to exit on SIGTERM
/// before `kill_on_drop` fires SIGKILL. Tuned short so a hung MCP server
/// can't stall TUI exit; well-behaved servers almost always exit within
/// a few hundred ms.
const STDIO_SHUTDOWN_GRACE: Duration = Duration::from_millis(2_000);

/// How many lines of MCP-server stderr to keep around for crash diagnostics.
/// Bounded so a chatty server can't grow this without limit; large enough to
/// catch typical Node/Python startup or panic output.
const STDERR_TAIL_CAPACITY: usize = 64;

/// Bounded ring buffer for the most recent stderr lines from a spawned MCP
/// server. Used by `StdioTransport` to surface server-side context when the
/// transport read side fails (server crashed, exited early, etc).
#[derive(Default)]
pub struct StderrTail {
    lines: TokioMutex<VecDeque<String>>,
}

impl StderrTail {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            lines: TokioMutex::new(VecDeque::with_capacity(STDERR_TAIL_CAPACITY)),
        })
    }

    async fn push(&self, line: String) {
        let mut buf = self.lines.lock().await;
        if buf.len() >= STDERR_TAIL_CAPACITY {
            buf.pop_front();
        }
        buf.push_back(line);
    }

    async fn snapshot(&self) -> Vec<String> {
        self.lines.lock().await.iter().cloned().collect()
    }
}

/// Format the captured stderr tail for inclusion in an error message. Empty
/// tails return `None` so the caller can fall back to its original message.
async fn format_stderr_context(tail: &StderrTail) -> Option<String> {
    let lines = tail.snapshot().await;
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "MCP server stderr (last {} line{}):\n{}",
        lines.len(),
        if lines.len() == 1 { "" } else { "s" },
        lines.join("\n"),
    ))
}

/// Best-effort SIGTERM. On Unix uses `libc::kill`; on Windows there's no
/// equivalent so we let `kill_on_drop` (TerminateProcess) handle it via the
/// subsequent Drop. Returns whether a signal was actually sent.
fn send_sigterm(child: &Child) -> bool {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // SAFETY: pid was just obtained from `child.id()`. `libc::kill`
            // with `SIGTERM` is async-signal-safe and never observes invalid
            // memory. Worst case (pid wrap / process already gone) returns
            // ESRCH, which we deliberately ignore.
            unsafe {
                let _ = libc::kill(pid as i32, libc::SIGTERM);
            }
            return true;
        }
        false
    }
    #[cfg(not(unix))]
    {
        let _ = child;
        false
    }
}

#[async_trait::async_trait]
impl McpTransport for StdioTransport {
    async fn send(&mut self, mut msg: Vec<u8>) -> Result<()> {
        msg.push(b'\n');
        self.stdin.write_all(&msg).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn recv(&mut self) -> Result<Vec<u8>> {
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = match self.reader.read_line(&mut line).await {
                Ok(b) => b,
                Err(err) => {
                    if let Some(stderr) = format_stderr_context(&self.stderr_tail).await {
                        anyhow::bail!("Stdio transport read error: {err}\n{stderr}");
                    }
                    return Err(err.into());
                }
            };
            if bytes == 0 {
                if let Some(stderr) = format_stderr_context(&self.stderr_tail).await {
                    anyhow::bail!("Stdio transport closed\n{stderr}");
                }
                anyhow::bail!("Stdio transport closed");
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            return Ok(trimmed.as_bytes().to_vec());
        }
    }

    /// Send SIGTERM and wait up to `STDIO_SHUTDOWN_GRACE` for graceful exit
    /// before letting Drop / `kill_on_drop` fire SIGKILL as the backstop.
    async fn shutdown(&mut self) {
        send_sigterm(&self.child);
        // Give the child a window to exit cleanly. Discard the result —
        // either it exits (success) or the timeout fires (Drop will SIGKILL).
        let _ = tokio::time::timeout(STDIO_SHUTDOWN_GRACE, self.child.wait()).await;
    }
}

/// Drop fallback (#420): if `shutdown` was never called explicitly, still
/// fire SIGTERM before tokio's `kill_on_drop` sends SIGKILL. The two
/// signals arrive back-to-back so well-behaved servers at least see the
/// SIGTERM first; misbehaving ones get SIGKILL'd anyway.
impl Drop for StdioTransport {
    fn drop(&mut self) {
        send_sigterm(&self.child);
    }
}

pub struct SseTransport {
    client: reqwest::Client,
    base_url: String,
    auth: McpHttpAuth,
    endpoint_url: Option<String>,
    receiver: tokio::sync::mpsc::UnboundedReceiver<SseInbound>,
    pending_messages: VecDeque<Vec<u8>>,
    #[allow(dead_code)]
    sse_task: tokio::task::JoinHandle<()>,
}

enum SseInbound {
    Endpoint(String),
    Message(Vec<u8>),
}

struct HttpTransport {
    mode: HttpTransportMode,
    client: reqwest::Client,
    base_url: String,
    auth: McpHttpAuth,
    cancel_token: tokio_util::sync::CancellationToken,
    endpoint_timeout: Duration,
}

enum HttpTransportMode {
    Streamable(StreamableHttpTransport),
    Sse(SseTransport),
}

struct StreamableHttpTransport {
    client: reqwest::Client,
    url: String,
    /// Request-time auth and custom header resolver for outbound POSTs.
    auth: McpHttpAuth,
    pending_messages: VecDeque<Vec<u8>>,
    /// Per-spec MCP session identifier returned by the server in the
    /// first response (typically the `initialize` response). Attached
    /// as the `Mcp-Session-Id` header on every subsequent outbound
    /// request so the server can correlate messages within the same
    /// session.
    session_id: Option<String>,
}

#[derive(Clone, Default)]
struct McpHttpAuth {
    headers: HashMap<String, String>,
    env_headers: HashMap<String, String>,
    bearer_token_env_var: Option<String>,
    oauth: Option<oauth::McpOAuthRuntime>,
}

impl McpHttpAuth {
    fn from_config(config: &McpServerConfig, oauth: Option<oauth::McpOAuthRuntime>) -> Self {
        Self {
            headers: config.headers.clone(),
            env_headers: config.env_headers.clone(),
            bearer_token_env_var: config.bearer_token_env_var.clone(),
            oauth,
        }
    }

    async fn resolved_headers(&self) -> Result<HashMap<String, String>> {
        let mut headers = self.headers.clone();
        for (name, env_var) in &self.env_headers {
            if let Ok(value) = std::env::var(env_var)
                && !value.trim().is_empty()
            {
                headers.insert(name.clone(), value);
            }
        }
        if !mcp_headers_have_authorization(&headers)
            && let Some(env_var) = self.bearer_token_env_var.as_deref()
            && let Ok(token) = std::env::var(env_var)
        {
            let token = token.trim();
            if !token.is_empty() {
                headers.insert("Authorization".to_string(), format!("Bearer {token}"));
            }
        }
        if !mcp_headers_have_authorization(&headers)
            && let Some(oauth) = &self.oauth
            && let Some(value) = oauth.authorization_header().await?
        {
            headers.insert("Authorization".to_string(), value);
        }
        Ok(headers)
    }
}

fn mcp_headers_have_authorization(headers: &HashMap<String, String>) -> bool {
    headers
        .keys()
        .any(|key| key.trim().eq_ignore_ascii_case("authorization"))
}

#[derive(Debug)]
enum StreamableSendError {
    Incompatible(String),
    StaleSession(String),
    Other(anyhow::Error),
}

impl SseTransport {
    async fn connect(
        client: reqwest::Client,
        url: String,
        auth: McpHttpAuth,
        cancel_token: tokio_util::sync::CancellationToken,
        endpoint_timeout: Duration,
    ) -> Result<Self> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let client_clone = client.clone();
        let url_clone = url.clone();
        let auth_clone = auth.clone();
        let wait_cancel_token = cancel_token.clone();

        let sse_task = tokio::spawn(async move {
            if cancel_token.is_cancelled() {
                return;
            }
            use futures_util::FutureExt;
            let result = std::panic::AssertUnwindSafe(Self::run_sse_loop(
                client_clone,
                url_clone,
                auth_clone,
                tx,
                cancel_token,
            ))
            .catch_unwind()
            .await;
            match result {
                Ok(res) => {
                    if let Err(e) = res {
                        tracing::error!("SSE loop error: {}", e);
                    }
                }
                Err(panic_err) => {
                    if let Some(msg) = panic_err.downcast_ref::<&str>() {
                        tracing::error!("SSE loop panicked: {}", msg);
                    } else if let Some(msg) = panic_err.downcast_ref::<String>() {
                        tracing::error!("SSE loop panicked: {}", msg);
                    } else {
                        tracing::error!("SSE loop panicked with unknown error");
                    }
                }
            }
        });

        let mut transport = Self {
            client,
            base_url: url,
            auth,
            endpoint_url: None,
            receiver: rx,
            pending_messages: VecDeque::new(),
            sse_task,
        };
        transport
            .wait_for_endpoint(&wait_cancel_token, endpoint_timeout)
            .await?;
        Ok(transport)
    }

    async fn run_sse_loop(
        client: reqwest::Client,
        url: String,
        auth: McpHttpAuth,
        tx: tokio::sync::mpsc::UnboundedSender<SseInbound>,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        let headers = auth.resolved_headers().await?;
        let response = apply_safe_custom_headers(
            with_default_mcp_http_headers(client.get(&url), false),
            &headers,
        )
        .send()
        .await
        .with_context(|| {
            format!(
                "MCP SSE connect failed (transport=http url={})",
                mask_url_secrets(&url),
            )
        })?;
        let status = response.status();
        if !status.is_success() {
            let body_excerpt = bounded_body_excerpt(response, ERROR_BODY_PREVIEW_BYTES).await;
            anyhow::bail!(
                "MCP SSE rejected (transport=http url={} status={}): {}",
                mask_url_secrets(&url),
                status,
                body_excerpt,
            );
        }

        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;
        let mut buffer = String::new();

        loop {
            if cancel_token.is_cancelled() {
                tracing::debug!("SSE loop cancelled");
                break;
            }
            let item = tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::debug!("SSE loop shutting down");
                    break;
                }
                item = stream.next() => {
                    match item {
                        Some(i) => i,
                        None => break,
                    }
                }
            };
            let chunk = item?;
            let s = String::from_utf8_lossy(&chunk);
            buffer.push_str(&s);

            while let Some((pos, separator_len)) = find_sse_event_separator(&buffer) {
                let event_block = buffer[..pos].to_string();
                buffer = buffer[pos + separator_len..].to_string();

                let mut event_type = "message";
                let mut data = String::new();

                for line in event_block.lines() {
                    if let Some(value) = sse_field_value(line, "event:") {
                        event_type = value;
                    } else if let Some(value) = sse_field_value(line, "data:") {
                        if !data.is_empty() {
                            data.push('\n');
                        }
                        data.push_str(value);
                    }
                }

                match event_type {
                    "endpoint" => {
                        let _ = tx.send(SseInbound::Endpoint(data));
                    }
                    "message" if !data.trim().is_empty() => {
                        let _ = tx.send(SseInbound::Message(data.into_bytes()));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    async fn wait_for_endpoint(
        &mut self,
        cancel_token: &tokio_util::sync::CancellationToken,
        endpoint_timeout: Duration,
    ) -> Result<()> {
        let timeout = tokio::time::sleep(endpoint_timeout);
        tokio::pin!(timeout);

        loop {
            let msg = tokio::select! {
                _ = cancel_token.cancelled() => {
                    anyhow::bail!("SSE transport cancelled before endpoint was discovered");
                }
                _ = &mut timeout => {
                    anyhow::bail!(
                        "SSE endpoint not received within {}ms",
                        endpoint_timeout.as_millis()
                    );
                }
                msg = self.receiver.recv() => {
                    msg.context("SSE transport closed before endpoint was discovered")?
                }
            };

            match msg {
                SseInbound::Endpoint(endpoint) => {
                    self.store_endpoint(&endpoint)?;
                    return Ok(());
                }
                SseInbound::Message(msg) => self.pending_messages.push_back(msg),
            }
        }
    }

    fn store_endpoint(&mut self, endpoint: &str) -> Result<()> {
        self.endpoint_url = Some(Self::resolve_endpoint_url(&self.base_url, endpoint)?);
        Ok(())
    }

    fn resolve_endpoint_url(base_url: &str, endpoint_url: &str) -> Result<String> {
        if endpoint_url.starts_with("http://") || endpoint_url.starts_with("https://") {
            return Ok(endpoint_url.to_string());
        }
        let base = reqwest::Url::parse(base_url)?;
        let joined = base.join(endpoint_url)?;
        Ok(joined.to_string())
    }
}

impl HttpTransport {
    fn new(
        client: reqwest::Client,
        url: String,
        auth: McpHttpAuth,
        cancel_token: tokio_util::sync::CancellationToken,
        endpoint_timeout: Duration,
    ) -> Self {
        Self {
            mode: HttpTransportMode::Streamable(StreamableHttpTransport::new(
                client.clone(),
                url.clone(),
                auth.clone(),
            )),
            client,
            base_url: url,
            auth,
            cancel_token,
            endpoint_timeout,
        }
    }

    async fn switch_to_sse_and_send(&mut self, msg: Vec<u8>) -> Result<()> {
        let mut sse = SseTransport::connect(
            self.client.clone(),
            self.base_url.clone(),
            self.auth.clone(),
            self.cancel_token.clone(),
            self.endpoint_timeout,
        )
        .await?;
        sse.send(msg).await?;
        self.mode = HttpTransportMode::Sse(sse);
        Ok(())
    }

    /// Best-effort session-establishment GET preflight.
    ///
    /// Per the Streamable HTTP spec, the server may return an
    /// `Mcp-Session-Id` header on the `initialize` response (the normal
    /// path handled inside [`StreamableHttpTransport::send`] above).
    /// However some servers (e.g. Hindsight, #1629) **require** a session
    /// ID on every POST including `initialize`, creating a chicken-and-egg
    /// problem. For those servers we send a short-lived GET before the
    /// first POST: if the server returns a session ID in the GET response
    /// it will be captured by the header-reading code in
    /// [`StreamableHttpTransport::send`] just as if it came from a POST
    /// response.
    ///
    /// This is intentionally best-effort:
    /// * The GET uses a tight per-request inner timeout so it never
    ///   blocks connection startup for long.
    /// * If the server doesn't support GET (405, 404, …) we log a debug
    ///   line and move on — the `initialize` POST will proceed without a
    ///   session ID.
    /// * If the server opens an SSE stream in response (the GET from old
    ///   SSE transport), we read only the headers, then discard the body
    ///   so the SSE stream is torn down. The actual SSE path uses a
    ///   dedicated `SseTransport` and is triggered by the incompatible-
    ///   status fallback in [`HttpTransport::send`].
    async fn try_establish_session(&mut self) -> Result<()> {
        let transport = match &mut self.mode {
            HttpTransportMode::Streamable(t) => t,
            // Already on SSE — session is implicit via the long-lived GET.
            HttpTransportMode::Sse(_) => return Ok(()),
        };

        let headers = transport.auth.resolved_headers().await?;
        let request = apply_safe_custom_headers(
            with_default_mcp_http_headers(transport.client.get(&transport.url), false),
            &headers,
        );
        let response = tokio::time::timeout(Duration::from_secs(5), request.send())
            .await
            .map_err(|_| anyhow::anyhow!("GET timeout"))?
            .map_err(|e| anyhow::anyhow!("GET error: {e}"))?;

        // Capture session ID from the GET response so subsequent POSTs
        // (including `initialize`) can include it. This is the same
        // header-reading logic that would be hit inside
        // `StreamableHttpTransport::send` for POST responses, but since
        // the GET is sent before any POST we do it here directly.
        if let Some(sid) = response
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|v| v.to_str().ok())
            && transport.session_id.as_deref() != Some(sid)
        {
            let session_ref = crate::utils::redacted_identifier_for_log(sid);
            tracing::debug!(target: "mcp", session = %session_ref, "captured MCP session ID via GET preflight");
            transport.session_id = Some(sid.to_string());
        }

        // We only care about the response headers — discard the body.
        // If the server opened an SSE stream in response (some servers
        // do this on GET), it will be torn down when response is dropped.
        drop(response);

        Ok(())
    }
}

#[async_trait::async_trait]
impl McpTransport for HttpTransport {
    async fn send(&mut self, msg: Vec<u8>) -> Result<()> {
        match &mut self.mode {
            HttpTransportMode::Streamable(transport) => match transport.send(msg.clone()).await {
                Ok(()) => Ok(()),
                Err(StreamableSendError::Incompatible(detail)) => {
                    tracing::debug!(
                        "MCP Streamable HTTP unavailable; falling back to SSE endpoint discovery: {}",
                        detail
                    );
                    self.switch_to_sse_and_send(msg).await
                }
                Err(StreamableSendError::StaleSession(detail)) => {
                    if let HttpTransportMode::Streamable(transport) = &mut self.mode {
                        tracing::debug!(
                            target: "mcp",
                            error = %detail,
                            "MCP Streamable HTTP session expired; clearing cached session ID"
                        );
                        transport.session_id = None;
                    }
                    Err(anyhow::anyhow!(
                        "MCP Streamable HTTP session expired; retry with a new session required ({detail})"
                    ))
                }
                Err(StreamableSendError::Other(err)) => Err(err),
            },
            HttpTransportMode::Sse(transport) => transport.send(msg).await,
        }
    }

    async fn recv(&mut self) -> Result<Vec<u8>> {
        match &mut self.mode {
            HttpTransportMode::Streamable(transport) => transport.recv().await,
            HttpTransportMode::Sse(transport) => transport.recv().await,
        }
    }

    async fn shutdown(&mut self) {
        if let HttpTransportMode::Sse(transport) = &mut self.mode {
            transport.shutdown().await;
        }
    }
}

impl StreamableHttpTransport {
    fn new(client: reqwest::Client, url: String, auth: McpHttpAuth) -> Self {
        Self {
            client,
            url,
            auth,
            pending_messages: VecDeque::new(),
            session_id: None,
        }
    }

    async fn send(&mut self, msg: Vec<u8>) -> std::result::Result<(), StreamableSendError> {
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
            .body(msg)
            .send()
            .await
            .map_err(|err| StreamableSendError::Other(err.into()))?;

        let status = response.status();

        // Capture session ID from any response (2xx, 202, 4xx, …). The
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

        if !status.is_success() {
            let body_excerpt = bounded_body_excerpt(response, ERROR_BODY_PREVIEW_BYTES).await;
            if self.session_id.is_some()
                && is_streamable_http_stale_session_status(status, &body_excerpt)
            {
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
        let body = response
            .text()
            .await
            .map_err(|err| StreamableSendError::Other(err.into()))?;
        self.store_response_body(content_type.as_deref(), &body)
            .map_err(StreamableSendError::Other)
    }

    async fn recv(&mut self) -> Result<Vec<u8>> {
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

fn is_mcp_stale_session_body(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    body.contains("session") && (body.contains("expired") || body.contains("invalid"))
}

fn is_mcp_stale_session_error(err: &anyhow::Error) -> bool {
    let err = format!("{err:#}");
    let lower_err = err.to_ascii_lowercase();
    err.contains("MCP Streamable HTTP session expired")
        || err.contains("MCP session expired")
        || err.contains("SSE transport closed")
        || (err.contains("MCP SSE POST send failed") && is_connection_closed_error_text(&lower_err))
        || is_mcp_stale_session_body(&err)
}

fn is_connection_closed_error_text(err: &str) -> bool {
    err.contains("connection closed")
        || err.contains("connection reset")
        || err.contains("broken pipe")
        || err.contains("unexpected eof")
        || err.contains("forcibly closed")
}

fn parse_sse_message_data(body: &str) -> Vec<Vec<u8>> {
    let normalized = body.replace("\r\n", "\n");
    let mut messages = Vec::new();

    for block in normalized.split("\n\n") {
        let mut event_type = "message";
        let mut data = String::new();

        for line in block.lines() {
            if let Some(value) = sse_field_value(line, "event:") {
                event_type = value;
            } else if let Some(value) = sse_field_value(line, "data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value);
            }
        }

        if event_type != "message" || data.trim().is_empty() {
            continue;
        }

        messages.push(data.trim().as_bytes().to_vec());
    }

    messages
}

fn find_sse_event_separator(buffer: &str) -> Option<(usize, usize)> {
    match (buffer.find("\n\n"), buffer.find("\r\n\r\n")) {
        (Some(lf), Some(crlf)) if crlf < lf => Some((crlf, 4)),
        (Some(lf), _) => Some((lf, 2)),
        (_, Some(crlf)) => Some((crlf, 4)),
        _ => None,
    }
}

fn sse_field_value<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    let value = line.strip_prefix(field)?;
    Some(value.strip_prefix(' ').unwrap_or(value))
}

fn is_legacy_sse_transport(config: &McpServerConfig) -> bool {
    config
        .transport
        .as_deref()
        .map(|transport| transport.trim().eq_ignore_ascii_case("sse"))
        .unwrap_or(false)
}

fn validate_mcp_transport(transport: Option<&str>) -> Result<()> {
    let Some(transport) = transport else {
        return Ok(());
    };
    if transport.trim().eq_ignore_ascii_case("sse") {
        return Ok(());
    }
    anyhow::bail!("Unsupported MCP transport '{transport}'. Supported values: sse");
}

fn response_id_matches(id: Option<&serde_json::Value>, expected_id: &str) -> bool {
    let Some(id) = id else {
        return false;
    };
    if id.as_str() == Some(expected_id) {
        return true;
    }
    id.as_u64()
        .map(|id| id.to_string() == expected_id)
        .unwrap_or(false)
}

#[async_trait::async_trait]
impl McpTransport for SseTransport {
    async fn send(&mut self, msg: Vec<u8>) -> Result<()> {
        let endpoint = self
            .endpoint_url
            .as_ref()
            .context("SSE endpoint not yet discovered")?
            .clone();
        let headers = self.auth.resolved_headers().await?;
        let response = apply_safe_custom_headers(
            with_default_mcp_http_headers(self.client.post(&endpoint), true),
            &headers,
        )
        .body(msg)
        .send()
        .await
        .with_context(|| {
            format!(
                "MCP SSE POST send failed (transport=sse endpoint={})",
                mask_url_secrets(&endpoint)
            )
        })?;
        let status = response.status();
        if !status.is_success() {
            let body_excerpt = bounded_body_excerpt(response, ERROR_BODY_PREVIEW_BYTES).await;
            if is_mcp_stale_session_body(&body_excerpt) {
                anyhow::bail!(
                    "MCP session expired (transport=sse endpoint={} status={}): {}",
                    mask_url_secrets(&endpoint),
                    status,
                    body_excerpt
                );
            }
            anyhow::bail!(
                "MCP SSE POST rejected (transport=sse endpoint={} status={}): {}",
                mask_url_secrets(&endpoint),
                status,
                body_excerpt
            );
        }
        Ok(())
    }

    async fn recv(&mut self) -> Result<Vec<u8>> {
        loop {
            if let Some(msg) = self.pending_messages.pop_front() {
                return Ok(msg);
            }

            match self.receiver.recv().await.context("SSE transport closed")? {
                SseInbound::Endpoint(endpoint) => {
                    self.store_endpoint(&endpoint)?;
                }
                SseInbound::Message(msg) => return Ok(msg),
            }
        }
    }
}

// === McpConnection - Async Connection Management ===

/// Manages a single async connection to an MCP server
pub struct McpConnection {
    name: String,
    transport: Box<dyn McpTransport>,
    tools: Vec<McpTool>,
    resources: Vec<McpResource>,
    resource_templates: Vec<McpResourceTemplate>,
    prompts: Vec<McpPrompt>,
    request_id: AtomicU64,
    state: ConnectionState,
    config: McpServerConfig,
    read_timeout_secs: u64,
    cancel_token: tokio_util::sync::CancellationToken,
}

impl McpConnection {
    /// Connect to an MCP server and initialize it.
    ///
    /// `network_policy` (added in v0.7.0 for #135) is consulted for HTTP/SSE
    /// transports only — STDIO transports are unaffected. Pass `None` to
    /// match pre-v0.7.0 permissive behavior.
    pub async fn connect_with_policy(
        name: String,
        config: McpServerConfig,
        global_timeouts: &McpTimeouts,
        network_policy: Option<&NetworkPolicyDecider>,
    ) -> Result<Self> {
        let connect_timeout_secs = config.effective_connect_timeout(global_timeouts);
        let read_timeout_secs = config.effective_read_timeout(global_timeouts);
        let cancel_token = tokio_util::sync::CancellationToken::new();

        let transport: Box<dyn McpTransport> = if let Some(url) = &config.url {
            // Per-domain network policy gate (#135). Only the HTTP/SSE transport
            // is gated; STDIO MCP servers run as local subprocesses and never
            // touch the network from this code path.
            if let Some(decider) = network_policy
                && let Some(host) = host_from_url(url)
            {
                match decider.evaluate(&host, "mcp") {
                    Decision::Allow => {}
                    Decision::Deny => {
                        anyhow::bail!(
                            "MCP server '{name}' connection to '{host}' blocked by network policy"
                        );
                    }
                    Decision::Prompt => {
                        anyhow::bail!(
                            "MCP server '{name}' connection to '{host}' requires approval; \
                             re-run after `/network allow {host}` or set network.default = \"allow\" in config"
                        );
                    }
                }
            }
            // Honor the standard `HTTP_PROXY` / `HTTPS_PROXY` (and their
            // lowercase equivalents) plus `NO_PROXY` env vars when
            // reaching MCP HTTP servers (#1408). Reqwest 0.13 does not
            // auto-detect these by default, so users behind corporate
            // proxies, on China-mainland connections routing through a
            // local Clash / Shadowsocks tunnel, etc. previously had MCP
            // HTTP traffic bypass the proxy entirely while every other
            // tool on the box (curl, npm, …) used it.
            let mut client_builder = crate::tls::reqwest_client_builder()
                .timeout(Duration::from_secs(connect_timeout_secs));
            let env_proxy_url = std::env::var("HTTPS_PROXY")
                .or_else(|_| std::env::var("https_proxy"))
                .or_else(|_| std::env::var("HTTP_PROXY"))
                .or_else(|_| std::env::var("http_proxy"))
                .ok()
                .filter(|s| !s.trim().is_empty());
            if let Some(proxy_url) = env_proxy_url {
                match reqwest::Proxy::all(&proxy_url) {
                    Ok(proxy) => {
                        let proxy = proxy.no_proxy(reqwest::NoProxy::from_env());
                        client_builder = client_builder.proxy(proxy);
                    }
                    Err(err) => {
                        // Redact userinfo (the `username[:password]@…`
                        // portion of the URL) before logging so an
                        // HTTPS_PROXY that embeds credentials
                        // (common in corporate setups) doesn't leak the
                        // password to the on-disk `~/.deepseek/logs/`.
                        let proxy_redacted = redact_proxy_userinfo(&proxy_url);
                        tracing::warn!(
                            target: "mcp",
                            ?err,
                            proxy = %proxy_redacted,
                            "ignoring malformed HTTP(S)_PROXY env var; MCP connection will bypass proxy"
                        );
                    }
                }
            }
            let client = client_builder.build()?;
            let oauth_runtime = match oauth::build_default_headers(
                &config.headers,
                &config.env_headers,
            ) {
                Ok(default_headers) => match oauth::McpOAuthRuntime::from_server_config(
                    &name,
                    &config,
                    default_headers,
                )
                .await
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        tracing::warn!(
                            target: "mcp",
                            server = %name,
                            error = %err,
                            "failed to prepare MCP OAuth runtime; continuing without stored OAuth token"
                        );
                        None
                    }
                },
                Err(err) => {
                    tracing::warn!(
                        target: "mcp",
                        server = %name,
                        error = %err,
                        "failed to prepare MCP OAuth default headers; continuing without stored OAuth token"
                    );
                    None
                }
            };
            let http_auth = McpHttpAuth::from_config(&config, oauth_runtime);
            if is_legacy_sse_transport(&config) {
                Box::new(
                    SseTransport::connect(
                        client,
                        url.clone(),
                        http_auth,
                        cancel_token.clone(),
                        Duration::from_secs(connect_timeout_secs),
                    )
                    .await?,
                )
            } else {
                let mut http = HttpTransport::new(
                    client,
                    url.clone(),
                    http_auth,
                    cancel_token.clone(),
                    Duration::from_secs(connect_timeout_secs),
                );
                // Best-effort session preflight for servers that require
                // a session ID on every POST including `initialize`
                // (e.g. Hindsight, #1629). Failures are non-fatal — the
                // `initialize` POST will proceed and may capture a session
                // ID from the response instead.
                if let Err(e) = http.try_establish_session().await {
                    tracing::debug!(
                        target: "mcp",
                        server = %name,
                        error = %e,
                        "session-establishment GET skipped; proceeding with POST initialize"
                    );
                }
                Box::new(http)
            }
        } else if let Some(command) = &config.command {
            let mut cmd = tokio::process::Command::new(command);
            cmd.args(&config.args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            if let Some(cwd) = &config.cwd {
                cmd.current_dir(cwd);
            }

            // MCP stdio servers are user-configured integrations. Use the
            // wider MCP allowlist so common Node/Python/proxy/CA-bundle
            // bootstrap variables (NVM_DIR, NODE_OPTIONS, NPM_CONFIG_*,
            // HTTP(S)_PROXY, …) reach the child. See `sanitized_mcp_env`
            // and #1244 for context.
            child_env::apply_to_tokio_command_mcp(&mut cmd, child_env::string_map_env(&config.env));

            let mut child = cmd.spawn().with_context(|| {
                let env_keys: Vec<&str> = config.env.keys().map(String::as_str).collect();
                format!(
                    "MCP stdio spawn failed (transport=stdio server={name} cmd={command:?} args={:?} env_keys={env_keys:?})",
                    config.args,
                )
            })?;

            let stdin = child.stdin.take().context("Failed to get MCP stdin")?;
            let stdout = child.stdout.take().context("Failed to get MCP stdout")?;
            let stderr = child.stderr.take().context("Failed to get MCP stderr")?;

            // Drain stderr into a bounded ring buffer so a crash mid-run
            // leaves diagnostic breadcrumbs instead of disappearing into
            // `Stdio::null`. The task exits naturally when the child closes
            // its stderr (kill_on_drop / exit / explicit shutdown).
            let stderr_tail = StderrTail::new();
            {
                let tail = Arc::clone(&stderr_tail);
                tokio::spawn(async move {
                    let mut lines = tokio::io::BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tail.push(line).await;
                    }
                });
            }

            Box::new(StdioTransport {
                child,
                stdin,
                reader: tokio::io::BufReader::new(stdout),
                stderr_tail,
            })
        } else {
            anyhow::bail!("MCP server '{name}' config must have either 'command' or 'url'");
        };

        let mut conn = Self {
            name: name.clone(),
            transport,
            tools: Vec::new(),
            resources: Vec::new(),
            resource_templates: Vec::new(),
            prompts: Vec::new(),
            request_id: AtomicU64::new(1),
            state: ConnectionState::Connecting,
            config,
            read_timeout_secs,
            cancel_token,
        };

        // Initialize with timeout
        tokio::time::timeout(Duration::from_secs(connect_timeout_secs), conn.initialize())
            .await
            .with_context(|| format!("MCP server '{name}' initialization timed out"))??;

        // Discover tools, resources, and prompts with timeout
        tokio::time::timeout(
            Duration::from_secs(connect_timeout_secs),
            conn.discover_all(),
        )
        .await
        .with_context(|| format!("MCP server '{name}' discovery timed out"))??;

        conn.state = ConnectionState::Ready;
        Ok(conn)
    }

    /// Send initialize request and wait for response
    async fn initialize(&mut self) -> Result<()> {
        let init_id = self.next_id();
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": &init_id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "clientInfo": {
                    "name": "codewhale-tui",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "tools": {},
                    "resources": {},
                    "prompts": {}
                }
            }
        }))
        .await?;

        self.recv(init_id).await?;

        // Send initialized notification (no id, no response expected)
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .await?;

        Ok(())
    }

    /// Discover tools, resources, and prompts
    async fn discover_all(&mut self) -> Result<()> {
        // We use join! to discover everything concurrently if possible,
        // but for now let's keep it sequential for simplicity in error handling
        self.discover_tools().await?;
        self.discover_resources().await?;
        self.discover_resource_templates().await?;
        self.discover_prompts().await?;
        Ok(())
    }

    /// Discover available tools from the MCP server
    async fn discover_tools(&mut self) -> Result<()> {
        let mut cursor: Option<String> = None;
        loop {
            let list_id = self.next_id();
            let params = match &cursor {
                Some(c) => serde_json::json!({ "cursor": c }),
                None => serde_json::json!({}),
            };
            self.send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": &list_id,
                "method": "tools/list",
                "params": params
            }))
            .await?;

            let response = self.recv(list_id).await?;
            let Some(result) = response.get("result") else {
                break;
            };

            if let Some(arr) = result.get("tools").and_then(|t| t.as_array()) {
                for item in arr {
                    match serde_json::from_value::<McpTool>(item.clone()) {
                        Ok(tool) => self.tools.push(tool),
                        Err(err) => {
                            // Skip individual malformed entries instead of
                            // dropping the whole page (#1410). The old
                            // `unwrap_or_default()` would silently throw
                            // away every tool when one was misshapen.
                            tracing::debug!(target: "mcp", ?err, "skipping malformed tool item");
                        }
                    }
                }
            }

            cursor = result
                .get("nextCursor")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        // Sort by tool name so the order the model sees doesn't depend on
        // server-side pagination ordering — keeps the prompt prefix stable
        // for cache-hit purposes (#1319).
        self.tools.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(())
    }

    /// Discover available resources from the MCP server
    async fn discover_resources(&mut self) -> Result<()> {
        let mut cursor: Option<String> = None;
        loop {
            let list_id = self.next_id();
            let params = match &cursor {
                Some(c) => serde_json::json!({ "cursor": c }),
                None => serde_json::json!({}),
            };
            self.send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": &list_id,
                "method": "resources/list",
                "params": params
            }))
            .await?;

            let response = self.recv(list_id).await?;
            let Some(result) = response.get("result") else {
                break;
            };

            if let Some(arr) = result.get("resources").and_then(|r| r.as_array()) {
                for item in arr {
                    match serde_json::from_value::<McpResource>(item.clone()) {
                        Ok(resource) => self.resources.push(resource),
                        Err(err) => {
                            tracing::debug!(target: "mcp", ?err, "skipping malformed resource item");
                        }
                    }
                }
            }

            cursor = result
                .get("nextCursor")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(())
    }

    /// Discover available resource templates from the MCP server
    async fn discover_resource_templates(&mut self) -> Result<()> {
        let mut cursor: Option<String> = None;
        loop {
            let list_id = self.next_id();
            let params = match &cursor {
                Some(c) => serde_json::json!({ "cursor": c }),
                None => serde_json::json!({}),
            };
            self.send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": &list_id,
                "method": "resources/templates/list",
                "params": params
            }))
            .await?;

            let response = self.recv(list_id).await?;
            let Some(result) = response.get("result") else {
                break;
            };

            let templates = result
                .get("resourceTemplates")
                .or_else(|| result.get("templates"))
                .or_else(|| result.get("resource_templates"));
            if let Some(arr) = templates.and_then(|t| t.as_array()) {
                for item in arr {
                    match serde_json::from_value::<McpResourceTemplate>(item.clone()) {
                        Ok(tmpl) => self.resource_templates.push(tmpl),
                        Err(err) => {
                            tracing::debug!(target: "mcp", ?err, "skipping malformed resource_template item");
                        }
                    }
                }
            }

            cursor = result
                .get("nextCursor")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(())
    }

    /// Discover available prompts from the MCP server
    async fn discover_prompts(&mut self) -> Result<()> {
        let mut cursor: Option<String> = None;
        loop {
            let list_id = self.next_id();
            let params = match &cursor {
                Some(c) => serde_json::json!({ "cursor": c }),
                None => serde_json::json!({}),
            };
            self.send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": &list_id,
                "method": "prompts/list",
                "params": params
            }))
            .await?;

            let response = self.recv(list_id).await?;
            let Some(result) = response.get("result") else {
                break;
            };

            if let Some(arr) = result.get("prompts").and_then(|p| p.as_array()) {
                for item in arr {
                    match serde_json::from_value::<McpPrompt>(item.clone()) {
                        Ok(prompt) => self.prompts.push(prompt),
                        Err(err) => {
                            tracing::debug!(target: "mcp", ?err, "skipping malformed prompt item");
                        }
                    }
                }
            }

            cursor = result
                .get("nextCursor")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(())
    }

    /// Call a tool on this MCP server
    pub async fn call_tool(
        &mut self,
        tool_name: &str,
        arguments: serde_json::Value,
        timeout_secs: u64,
    ) -> Result<serde_json::Value> {
        self.call_method(
            "tools/call",
            serde_json::json!({
                "name": tool_name,
                "arguments": arguments
            }),
            timeout_secs,
        )
        .await
    }

    /// Read a resource from this MCP server
    pub async fn read_resource(
        &mut self,
        uri: &str,
        timeout_secs: u64,
    ) -> Result<serde_json::Value> {
        self.call_method(
            "resources/read",
            serde_json::json!({
                "uri": uri
            }),
            timeout_secs,
        )
        .await
    }

    /// Get a prompt from this MCP server
    pub async fn get_prompt(
        &mut self,
        prompt_name: &str,
        arguments: serde_json::Value,
        timeout_secs: u64,
    ) -> Result<serde_json::Value> {
        self.call_method(
            "prompts/get",
            serde_json::json!({
                "name": prompt_name,
                "arguments": arguments
            }),
            timeout_secs,
        )
        .await
    }

    /// Generic method to call an MCP method
    async fn call_method(
        &mut self,
        method: &str,
        params: serde_json::Value,
        timeout_secs: u64,
    ) -> Result<serde_json::Value> {
        if self.state != ConnectionState::Ready {
            anyhow::bail!(
                "Failed to call MCP method '{}': connection '{}' is not ready",
                method,
                self.name
            );
        }

        let call_id = self.next_id();
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": &call_id,
            "method": method,
            "params": params
        }))
        .await?;

        let response = tokio::time::timeout(Duration::from_secs(timeout_secs), self.recv(call_id))
            .await
            .with_context(|| {
                format!(
                    "MCP method '{}' on server '{}' timed out after {}s",
                    method, self.name, timeout_secs
                )
            })??;

        if let Some(error) = response.get("error") {
            return Err(anyhow::anyhow!(
                "MCP error in '{}': {}",
                method,
                serde_json::to_string_pretty(error)?
            ));
        }

        Ok(response
            .get("result")
            .cloned()
            .unwrap_or(serde_json::json!(null)))
    }

    /// Get discovered tools
    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    /// Get discovered resources
    pub fn resources(&self) -> &[McpResource] {
        &self.resources
    }

    /// Get discovered resource templates
    pub fn resource_templates(&self) -> &[McpResourceTemplate] {
        &self.resource_templates
    }

    /// Get discovered prompts
    pub fn prompts(&self) -> &[McpPrompt] {
        &self.prompts
    }

    /// Get server name
    #[allow(dead_code)] // Public API for MCP consumers
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Check if connection is ready
    pub fn is_ready(&self) -> bool {
        self.state == ConnectionState::Ready
    }

    /// Get server config
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }

    /// Get connection state
    #[allow(dead_code)] // Public API for MCP consumers
    pub fn state(&self) -> ConnectionState {
        self.state
    }

    fn next_id(&self) -> String {
        self.request_id.fetch_add(1, Ordering::SeqCst).to_string()
    }

    async fn send(&mut self, msg: serde_json::Value) -> Result<()> {
        let bytes = serde_json::to_vec(&msg).context("Failed to serialize MCP JSON-RPC message")?;
        self.transport.send(bytes).await
    }

    async fn recv(&mut self, expected_id: String) -> Result<serde_json::Value> {
        loop {
            let bytes = match tokio::time::timeout(
                Duration::from_secs(self.read_timeout_secs),
                self.transport.recv(),
            )
            .await
            {
                Ok(result) => result.inspect_err(|_e| {
                    self.state = ConnectionState::Disconnected;
                })?,
                Err(_) => {
                    self.state = ConnectionState::Disconnected;
                    anyhow::bail!(
                        "Timed out waiting for MCP JSON-RPC response from server '{}' after {}s",
                        self.name,
                        self.read_timeout_secs
                    );
                }
            };
            let value: serde_json::Value = match serde_json::from_slice(&bytes) {
                Ok(value) => value,
                Err(err) => {
                    self.state = ConnectionState::Disconnected;
                    return Err(err).with_context(|| {
                        format!(
                            "Invalid MCP JSON-RPC message from server '{}': {}",
                            self.name,
                            invalid_json_preview(&bytes)
                        )
                    });
                }
            };

            // Check if this is a response with the expected id. We emit
            // string IDs because some MCP gateways reject numeric JSON-RPC
            // IDs, but accept numeric echoes for compatibility with older
            // servers and tests.
            if response_id_matches(value.get("id"), &expected_id) {
                if let Some(error) = value.get("error")
                    && is_mcp_stale_session_body(&error.to_string())
                {
                    anyhow::bail!("MCP session expired: {error}");
                }
                return Ok(value);
            }
            // Skip notifications (no id) and responses with different ids
        }
    }

    /// Gracefully close the connection
    #[allow(dead_code)] // Public API for MCP consumers
    pub fn close(&mut self) {
        self.cancel_token.cancel();
        self.state = ConnectionState::Disconnected;
    }
}

impl Drop for McpConnection {
    fn drop(&mut self) {
        self.cancel_token.cancel();
    }
}

// === McpPool - Connection Pool Management ===

/// Pool of MCP connections for reuse
pub struct McpPool {
    connections: HashMap<String, McpConnection>,
    config: McpConfig,
    network_policy: Option<NetworkPolicyDecider>,
    /// Source paths the config was loaded from. Empty for pools constructed
    /// directly via `new` (tests, ad-hoc snapshots). Workspace-aware pools
    /// track both global and project-level MCP config paths so lazy reload sees
    /// either file appear or change.
    config_sources: Vec<PathBuf>,
    workspace: Option<PathBuf>,
    /// 64-bit content hash of the active config (`hash_mcp_config`). Compared
    /// against the freshly-loaded config after an mtime change to skip
    /// reloading when the file was merely touched.
    config_hash: u64,
    /// Most recently observed mtime for `config_sources`.
    last_mtimes: Vec<Option<std::time::SystemTime>>,
}

/// Merge plugin-provided MCP servers into the pool config.
/// Plugin servers are added only when no user-configured server with the
/// same name exists, giving user config (global + workspace) priority.
fn merge_plugin_mcp_servers(config: &mut McpConfig) {
    if let Some(servers) = plugins::try_with_registry(|r| r.enabled_mcp_servers()) {
        for (name, server_config) in servers {
            config.servers.entry(name).or_insert(server_config);
        }
    }
}

impl McpPool {
    /// Create a new pool with the given configuration
    pub fn new(config: McpConfig) -> Self {
        let config_hash = hash_mcp_config(&config);
        Self {
            connections: HashMap::new(),
            config,
            network_policy: None,
            config_sources: Vec::new(),
            workspace: None,
            config_hash,
            last_mtimes: Vec::new(),
        }
    }

    /// Create a pool from a configuration file path.
    #[cfg(test)]
    pub fn from_config_path(path: &std::path::Path) -> Result<Self> {
        let config = load_config(path)?;
        let mut pool = Self::new(config);
        pool.config_sources = vec![path.to_path_buf()];
        pool.last_mtimes = vec![mcp_config_mtime(path)];
        Ok(pool)
    }

    /// Create a pool from global MCP config plus workspace-local
    /// `.codewhale/mcp.json`. Project servers override same-name global
    /// servers and default stdio `cwd` to the workspace root.
    pub fn from_config_path_with_workspace(
        path: &std::path::Path,
        workspace: &Path,
    ) -> Result<Self> {
        let config = load_config_with_workspace(path, workspace)?;
        let mut pool = Self::new(config);
        merge_plugin_mcp_servers(&mut pool.config);
        let workspace = checked_workspace_path(workspace)?;
        pool.config_sources = vec![
            path.to_path_buf(),
            checked_workspace_mcp_config_path(&workspace)?,
        ];
        pool.config_sources
            .extend(crate::config::workspace_trust_config_candidate_paths());
        pool.last_mtimes = pool
            .config_sources
            .iter()
            .map(|source| mcp_config_mtime(source))
            .collect();
        pool.workspace = Some(workspace);
        Ok(pool)
    }

    /// Attach a per-domain network policy (#135). When set, HTTP/SSE
    /// transports are gated through it; STDIO transports are unaffected.
    pub fn with_network_policy(mut self, policy: NetworkPolicyDecider) -> Self {
        self.network_policy = Some(policy);
        self
    }

    fn drop_connection(&mut self, server_name: &str, reason: &str) {
        if self.connections.remove(server_name).is_some() {
            tracing::debug!(
                target: "mcp",
                server = %server_name,
                reason = %reason,
                "dropped MCP connection"
            );
        }
    }

    fn drop_all_connections(&mut self, reason: &str) {
        if self.connections.is_empty() {
            return;
        }
        let count = self.connections.len();
        tracing::debug!(
            target: "mcp",
            count,
            reason = %reason,
            "dropping MCP connections"
        );
        self.connections.clear();
    }

    /// If the source config file's mtime has changed since the last check,
    /// re-read it and (only when the content hash also changed) drop all
    /// existing connections so the next `get_or_connect` reattaches under
    /// the new config. No-op when the pool was constructed via [`McpPool::new`]
    /// (no source path), when stat fails, or when the file content is
    /// byte-identical to what we last loaded. Returns `Ok(true)` if any
    /// connections were dropped, `Ok(false)` otherwise.
    ///
    /// This is the lazy half of the auto-reload story for #1267: instead of a
    /// long-lived file watcher, the next tool invocation pays a single `stat`
    /// call (and only re-reads the file when the mtime moved). On networked
    /// or remote filesystems where mtime granularity is poor, the hash
    /// compare keeps us from churning connections on every check.
    pub async fn reload_if_config_changed(&mut self) -> Result<bool> {
        if self.config_sources.is_empty() {
            return Ok(false);
        }
        let current_mtimes: Vec<_> = self
            .config_sources
            .iter()
            .map(|path| mcp_config_mtime(path))
            .collect();
        if current_mtimes == self.last_mtimes {
            return Ok(false);
        }
        // mtime moved — we owe a re-read.
        let primary = self
            .config_sources
            .first()
            .context("MCP config source list unexpectedly empty")?;
        let new_config = if let Some(workspace) = self.workspace.as_deref() {
            load_config_with_workspace(primary, workspace)?
        } else {
            load_config(primary)?
        };
        let new_hash = hash_mcp_config(&new_config);
        // Always advance mtimes so a touched-but-unchanged file doesn't
        // make us re-read on every subsequent call.
        self.last_mtimes = current_mtimes;
        if new_hash == self.config_hash {
            return Ok(false);
        }
        // Real content change — drop all live connections so the next
        // get_or_connect picks up the new config (sandbox flags, env, args).
        self.drop_all_connections("config reload");
        self.config = new_config;
        merge_plugin_mcp_servers(&mut self.config);
        self.config_hash = new_hash;
        Ok(true)
    }

    /// Get or create a connection to a server
    pub async fn get_or_connect(&mut self, server_name: &str) -> Result<&mut McpConnection> {
        // Lazy auto-reload (#1267 part 2): cheap mtime-then-hash check before
        // each connection lookup. Transient FS errors are logged but not
        // propagated so a brief hiccup can't take down the whole tool dispatch.
        if let Err(e) = self.reload_if_config_changed().await {
            tracing::warn!("MCP config reload check failed: {e:#}");
        }

        let is_ready = self
            .connections
            .get(server_name)
            .map(|conn| conn.is_ready())
            .unwrap_or(false);
        if is_ready {
            return self
                .connections
                .get_mut(server_name)
                .ok_or_else(|| anyhow::anyhow!("MCP connection disappeared for {server_name}"));
        }

        self.drop_connection(server_name, "reconnect");

        let server_config = self
            .config
            .servers
            .get(server_name)
            .ok_or_else(|| anyhow::anyhow!("Failed to find MCP server: {server_name}"))?
            .clone();

        if !server_config.is_enabled() {
            anyhow::bail!("Failed to connect MCP server '{server_name}': server is disabled");
        }

        let connection = McpConnection::connect_with_policy(
            server_name.to_string(),
            server_config,
            &self.config.timeouts,
            self.network_policy.as_ref(),
        )
        .await?;

        self.connections.insert(server_name.to_string(), connection);
        self.connections
            .get_mut(server_name)
            .ok_or_else(|| anyhow::anyhow!("Failed to store MCP connection for {server_name}"))
    }

    /// Connect to all enabled servers, returning errors for failed connections
    pub async fn connect_all(&mut self) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        let names: Vec<String> = self
            .config
            .servers
            .keys()
            .filter(|n| self.config.servers[*n].is_enabled())
            .cloned()
            .collect();

        for name in names {
            if let Err(e) = self.get_or_connect(&name).await {
                errors.push((name, e));
            }
        }

        for (name, server_cfg) in &self.config.servers {
            if server_cfg.required
                && server_cfg.is_enabled()
                && !self
                    .connections
                    .get(name)
                    .is_some_and(McpConnection::is_ready)
            {
                errors.push((
                    name.clone(),
                    anyhow::anyhow!("required MCP server failed to initialize"),
                ));
            }
        }

        errors
    }

    /// Get all discovered tools with server-prefixed names
    pub fn all_tools(&self) -> Vec<(String, &McpTool)> {
        let mut tools = Vec::new();
        for (server, conn) in &self.connections {
            for tool in conn.tools() {
                if !conn.config().is_tool_enabled(&tool.name) {
                    continue;
                }
                // Format: mcp_{server}_{tool}
                tools.push((format!("mcp_{}_{}", server, tool.name), tool));
            }
        }
        // Sort by prefixed name so iteration order across servers is
        // deterministic for prefix-cache stability (#1319).
        tools.sort_by(|a, b| a.0.cmp(&b.0));
        tools
    }

    /// Get all discovered resources with server-prefixed names
    pub fn all_resources(&self) -> Vec<(String, &McpResource)> {
        let mut resources = Vec::new();
        for (server, conn) in &self.connections {
            for resource in conn.resources() {
                // Format: mcp_{server}_{resource_name}
                // Note: resource names might contain spaces, we should probably slugify them
                let safe_name = resource.name.replace(' ', "_").to_lowercase();
                resources.push((format!("mcp_{server}_{safe_name}"), resource));
            }
        }
        resources
    }

    /// Get all discovered resource templates with server-prefixed names
    #[allow(dead_code)] // Public API for MCP resource discovery
    pub fn all_resource_templates(&self) -> Vec<(String, &McpResourceTemplate)> {
        let mut templates = Vec::new();
        for (server, conn) in &self.connections {
            for template in conn.resource_templates() {
                let safe_name = template.name.replace(' ', "_").to_lowercase();
                templates.push((format!("mcp_{server}_{safe_name}"), template));
            }
        }
        templates
    }

    async fn list_resources(&mut self, server: Option<String>) -> Result<Vec<serde_json::Value>> {
        if let Some(server_name) = server {
            let conn = self.get_or_connect(&server_name).await?;
            let resources = conn
                .resources()
                .iter()
                .map(|resource| {
                    serde_json::json!({
                        "server": server_name.clone(),
                        "uri": resource.uri,
                        "name": resource.name,
                        "description": resource.description,
                        "mime_type": resource.mime_type,
                    })
                })
                .collect();
            return Ok(resources);
        }

        let errors = self.connect_all().await;
        for (server, err) in errors {
            tracing::warn!("Failed to connect MCP server '{server}' for resources: {err:#}");
        }
        let mut items = Vec::new();
        for (server, conn) in &self.connections {
            for resource in conn.resources() {
                items.push(serde_json::json!({
                    "server": server,
                    "uri": resource.uri,
                    "name": resource.name,
                    "description": resource.description,
                    "mime_type": resource.mime_type,
                }));
            }
        }
        Ok(items)
    }

    async fn list_resource_templates(
        &mut self,
        server: Option<String>,
    ) -> Result<Vec<serde_json::Value>> {
        if let Some(server_name) = server {
            let conn = self.get_or_connect(&server_name).await?;
            let templates = conn
                .resource_templates()
                .iter()
                .map(|template| {
                    serde_json::json!({
                        "server": server_name.clone(),
                        "uri_template": template.uri_template,
                        "name": template.name,
                        "description": template.description,
                        "mime_type": template.mime_type,
                    })
                })
                .collect();
            return Ok(templates);
        }

        let errors = self.connect_all().await;
        for (server, err) in errors {
            tracing::warn!(
                "Failed to connect MCP server '{server}' for resource templates: {err:#}"
            );
        }
        let mut items = Vec::new();
        for (server, conn) in &self.connections {
            for template in conn.resource_templates() {
                items.push(serde_json::json!({
                    "server": server,
                    "uri_template": template.uri_template,
                    "name": template.name,
                    "description": template.description,
                    "mime_type": template.mime_type,
                }));
            }
        }
        Ok(items)
    }

    /// Get all discovered prompts with server-prefixed names
    pub fn all_prompts(&self) -> Vec<(String, &McpPrompt)> {
        let mut prompts = Vec::new();
        for (server, conn) in &self.connections {
            for prompt in conn.prompts() {
                // Format: mcp_{server}_{prompt}
                prompts.push((format!("mcp_{}_{}", server, prompt.name), prompt));
            }
        }
        prompts
    }

    /// Read a resource from a specific server
    pub async fn read_resource(
        &mut self,
        server_name: &str,
        uri: &str,
    ) -> Result<serde_json::Value> {
        let global_timeouts = self.config.timeouts;
        let conn = self.get_or_connect(server_name).await?;
        let timeout = conn.config().effective_read_timeout(&global_timeouts);
        conn.read_resource(uri, timeout).await
    }

    /// Get a prompt from a specific server
    pub async fn get_prompt(
        &mut self,
        server_name: &str,
        prompt_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let global_timeouts = self.config.timeouts;
        let conn = self.get_or_connect(server_name).await?;
        let timeout = conn.config().effective_execute_timeout(&global_timeouts);
        conn.get_prompt(prompt_name, arguments, timeout).await
    }

    /// Parse a prefixed name into (server_name, tool_name)
    pub(crate) fn parse_prefixed_name<'a>(
        &self,
        prefixed_name: &'a str,
    ) -> Result<(&'a str, &'a str)> {
        let Some(rest) = prefixed_name.strip_prefix("mcp_") else {
            anyhow::bail!("Invalid MCP tool name: {prefixed_name}");
        };

        let mut best_match: Option<(&str, &str)> = None;
        for server in self.connections.keys().chain(self.config.servers.keys()) {
            let Some(tool) = rest
                .strip_prefix(server)
                .and_then(|tail| tail.strip_prefix('_'))
            else {
                continue;
            };
            if tool.is_empty() {
                continue;
            }
            if best_match.is_none_or(|(matched, _)| server.len() > matched.len()) {
                best_match = Some((&rest[..server.len()], tool));
            }
        }

        if let Some((server, tool)) = best_match {
            return Ok((server, tool));
        }

        let Some((server, tool)) = rest.split_once('_') else {
            anyhow::bail!("Invalid MCP tool name format: {prefixed_name}");
        };
        Ok((server, tool))
    }

    /// Convert discovered tools to API Tool format
    pub fn to_api_tools(&self) -> Vec<crate::models::Tool> {
        let mut api_tools = Vec::new();

        // Add regular tools
        for (name, tool) in self.all_tools() {
            api_tools.push(crate::models::Tool {
                tool_type: None,
                name,
                description: tool.description.clone().unwrap_or_default(),
                input_schema: tool.input_schema.clone(),
                allowed_callers: Some(vec!["direct".to_string()]),
                defer_loading: Some(false),
                input_examples: None,
                strict: None,
                cache_control: None,
            });
        }

        if !self.config.servers.is_empty() {
            api_tools.push(crate::models::Tool {
                tool_type: None,
                name: "list_mcp_resources".to_string(),
                description: "List available MCP resources across servers (optionally filtered by server).".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "server": { "type": "string", "description": "Optional MCP server name to filter by" }
                    }
                }),
                allowed_callers: Some(vec!["direct".to_string()]),
                defer_loading: Some(false),
                input_examples: None,
                strict: None,
                cache_control: None,
            });
            api_tools.push(crate::models::Tool {
                tool_type: None,
                name: "list_mcp_resource_templates".to_string(),
                description: "List available MCP resource templates across servers (optionally filtered by server).".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "server": { "type": "string", "description": "Optional MCP server name to filter by" }
                    }
                }),
                allowed_callers: Some(vec!["direct".to_string()]),
                defer_loading: Some(false),
                input_examples: None,
                strict: None,
                cache_control: None,
            });
        }

        // Add resource reading tools if resources exist
        let resources = self.all_resources();
        if !resources.is_empty() {
            api_tools.push(crate::models::Tool {
                tool_type: None,
                name: "mcp_read_resource".to_string(),
                description: "Read a resource from an MCP server using its URI".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "server": { "type": "string", "description": "The name of the MCP server" },
                        "uri": { "type": "string", "description": "The URI of the resource to read" }
                    },
                    "required": ["server", "uri"]
                }),
                allowed_callers: Some(vec!["direct".to_string()]),
                defer_loading: Some(false),
                input_examples: None,
                strict: None,
                cache_control: None,
            });
            api_tools.push(crate::models::Tool {
                tool_type: None,
                name: "read_mcp_resource".to_string(),
                description: "Alias for mcp_read_resource.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "server": { "type": "string", "description": "The name of the MCP server" },
                        "uri": { "type": "string", "description": "The URI of the resource to read" }
                    },
                    "required": ["server", "uri"]
                }),
                allowed_callers: Some(vec!["direct".to_string()]),
                defer_loading: Some(false),
                input_examples: None,
                strict: None,
                cache_control: None,
            });
        }

        // Add prompt getting tools if prompts exist
        let prompts = self.all_prompts();
        if !prompts.is_empty() {
            api_tools.push(crate::models::Tool {
                tool_type: None,
                name: "mcp_get_prompt".to_string(),
                description: "Get a prompt from an MCP server".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "server": { "type": "string", "description": "The name of the MCP server" },
                        "name": { "type": "string", "description": "The name of the prompt" },
                        "arguments": {
                            "type": "object",
                            "description": "Optional arguments for the prompt",
                            "additionalProperties": { "type": "string" }
                        }
                    },
                    "required": ["server", "name"]
                }),
                allowed_callers: Some(vec!["direct".to_string()]),
                defer_loading: Some(false),
                input_examples: None,
                strict: None,
                cache_control: None,
            });
        }

        // Sort by name for prefix-cache stability — the tool block sent to
        // the model needs to be deterministic across runs (#1319).
        api_tools.sort_by(|a, b| a.name.cmp(&b.name));
        api_tools
    }

    /// Call a tool by its prefixed name (mcp_{server}_{tool})
    pub async fn call_tool(
        &mut self,
        prefixed_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value> {
        if prefixed_name == "list_mcp_resources" {
            let server = arguments
                .get("server")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let resources = self.list_resources(server).await?;
            return Ok(serde_json::json!({ "resources": resources }));
        }

        if prefixed_name == "list_mcp_resource_templates" {
            let server = arguments
                .get("server")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let templates = self.list_resource_templates(server).await?;
            return Ok(serde_json::json!({ "templates": templates }));
        }

        if prefixed_name == "mcp_read_resource" {
            let server_name = arguments
                .get("server")
                .and_then(|v| v.as_str())
                .context("Missing 'server' argument")?;
            let uri = arguments
                .get("uri")
                .and_then(|v| v.as_str())
                .context("Missing 'uri' argument")?;
            return self.read_resource(server_name, uri).await;
        }

        if prefixed_name == "read_mcp_resource" {
            let server_name = arguments
                .get("server")
                .and_then(|v| v.as_str())
                .context("Missing 'server' argument")?;
            let uri = arguments
                .get("uri")
                .and_then(|v| v.as_str())
                .context("Missing 'uri' argument")?;
            return self.read_resource(server_name, uri).await;
        }

        if prefixed_name == "mcp_get_prompt" {
            let server_name = arguments
                .get("server")
                .and_then(|v| v.as_str())
                .context("Missing 'server' argument")?;
            let name = arguments
                .get("name")
                .and_then(|v| v.as_str())
                .context("Missing 'name' argument")?;
            let args = arguments
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            return self.get_prompt(server_name, name, args).await;
        }

        let (server_name, tool_name) = self.parse_prefixed_name(prefixed_name)?;
        // Copy the global timeouts to avoid borrow conflict
        let global_timeouts = self.config.timeouts;
        let conn = self.get_or_connect(server_name).await?;
        if !conn.config().is_tool_enabled(tool_name) {
            anyhow::bail!("MCP tool '{tool_name}' is disabled for server '{server_name}'");
        }
        let timeout = conn.config().effective_execute_timeout(&global_timeouts);
        match conn.call_tool(tool_name, arguments.clone(), timeout).await {
            Ok(result) => Ok(result),
            Err(err) if is_mcp_stale_session_error(&err) => {
                tracing::debug!(
                    target: "mcp",
                    server = server_name,
                    tool = tool_name,
                    error = %err,
                    "retrying MCP tool call after stale session"
                );
                self.drop_connection(server_name, "stale session retry");
                let conn = self.get_or_connect(server_name).await?;
                if !conn.config().is_tool_enabled(tool_name) {
                    anyhow::bail!("MCP tool '{tool_name}' is disabled for server '{server_name}'");
                }
                let timeout = conn.config().effective_execute_timeout(&global_timeouts);
                conn.call_tool(tool_name, arguments, timeout).await
            }
            Err(err) => Err(err),
        }
    }

    /// Get list of configured server names
    #[allow(dead_code)] // Public API for MCP consumers
    pub fn server_names(&self) -> Vec<&str> {
        self.config
            .servers
            .keys()
            .map(std::string::String::as_str)
            .collect()
    }

    /// Get list of connected server names
    #[allow(dead_code)] // Public API; the HTTP list endpoint no longer spawns a pool to call it (#3532)
    pub fn connected_servers(&self) -> Vec<&str> {
        self.connections
            .iter()
            .filter(|(_, c)| c.is_ready())
            .map(|(n, _)| n.as_str())
            .collect()
    }

    /// Disconnect all connections
    #[allow(dead_code)] // Public API for MCP lifecycle management
    pub fn disconnect_all(&mut self) {
        self.drop_all_connections("disconnect all");
    }

    /// Graceful shutdown of every connection in the pool: send SIGTERM to
    /// each stdio child and give them a short grace period before drop
    /// fires SIGKILL. Whalescale#420.
    ///
    /// Call from the TUI exit path *before* dropping the pool to give
    /// MCP servers a chance to flush state. The fallback Drop on
    /// `StdioTransport` still sends SIGTERM if this never runs, so even
    /// abnormal exits avoid leaking PIDs without a signal.
    #[allow(dead_code)] // Wired in by callers that want graceful shutdown
    pub async fn shutdown_all(&mut self) {
        let names: Vec<String> = self.connections.keys().cloned().collect();
        for name in names {
            if let Some(conn) = self.connections.get_mut(&name) {
                conn.transport.shutdown().await;
            }
        }
        self.connections.clear();
    }

    /// Get the underlying configuration
    #[allow(dead_code)] // Public API for MCP consumers
    pub fn config(&self) -> &McpConfig {
        &self.config
    }

    /// Check if a tool name is an MCP tool
    pub fn is_mcp_tool(name: &str) -> bool {
        name.starts_with("mcp_")
            || matches!(
                name,
                "list_mcp_resources" | "list_mcp_resource_templates" | "read_mcp_resource"
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpWriteStatus {
    Created,
    Overwritten,
    SkippedExists,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpDiscoveredItem {
    pub name: String,
    pub model_name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSnapshot {
    pub name: String,
    pub enabled: bool,
    pub required: bool,
    pub transport: String,
    pub command_or_url: String,
    pub connect_timeout: u64,
    pub execute_timeout: u64,
    pub read_timeout: u64,
    pub connected: bool,
    pub error: Option<String>,
    pub tools: Vec<McpDiscoveredItem>,
    pub resources: Vec<McpDiscoveredItem>,
    pub prompts: Vec<McpDiscoveredItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpManagerSnapshot {
    pub config_path: std::path::PathBuf,
    pub config_exists: bool,
    pub restart_required: bool,
    pub servers: Vec<McpServerSnapshot>,
}

pub fn load_config(path: &Path) -> Result<McpConfig> {
    validate_mcp_config_path(path)?;
    let Some(contents) = read_mcp_config_file(path)? else {
        return Ok(McpConfig::default());
    };
    serde_json::from_str(&contents)
        .with_context(|| format!("Failed to parse MCP config {}", path.display()))
}

fn read_mcp_config_file(path: &Path) -> Result<Option<String>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("Failed to inspect MCP config {}", path.display()));
        }
    };
    let file_type = metadata.file_type();
    if file_type.is_symlink() || !file_type.is_file() {
        anyhow::bail!("MCP config path must be a regular file: {}", path.display());
    }

    let mut file = open_mcp_config_file(path)
        .with_context(|| format!("Failed to read MCP config {}", path.display()))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .with_context(|| format!("Failed to read MCP config {}", path.display()))?;
    Ok(Some(contents))
}

#[cfg(unix)]
fn open_mcp_config_file(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_mcp_config_file(path: &Path) -> std::io::Result<fs::File> {
    fs::File::open(path)
}

pub fn workspace_mcp_config_path(workspace: &Path) -> PathBuf {
    normalize_workspace_path(workspace)
        .join(".codewhale")
        .join("mcp.json")
}

pub fn load_config_with_workspace(global_path: &Path, workspace: &Path) -> Result<McpConfig> {
    let mut merged = load_config(global_path)?;
    let workspace = checked_workspace_path(workspace)?;
    let project_path = checked_workspace_mcp_config_path(&workspace)?;
    if !project_path.exists() || paths_refer_to_same_config(global_path, &project_path) {
        return Ok(merged);
    }
    // Workspace-local MCP can spawn stdio servers, so it is only honored after
    // the user has trusted this workspace in user-owned config. Do not accept
    // project-local legacy trust markers here: a repository could carry those
    // files itself and silently reintroduce the project-scope `mcp_config_path`
    // risk denied in #417.
    if !workspace_allows_project_mcp_config(&workspace) {
        return Ok(merged);
    }

    let mut project = load_config(&project_path)?;
    for server in project.servers.values_mut() {
        if server.command.is_some() && server.url.is_none() {
            server.cwd = Some(resolve_project_mcp_cwd(&workspace, server.cwd.as_deref())?);
        }
    }
    merged.servers.extend(project.servers);
    Ok(merged)
}

fn workspace_allows_project_mcp_config(workspace: &Path) -> bool {
    crate::config::is_workspace_trusted(workspace)
}

fn checked_workspace_mcp_config_path(workspace: &Path) -> Result<PathBuf> {
    Ok(checked_workspace_path(workspace)?
        .join(".codewhale")
        .join("mcp.json"))
}

fn checked_workspace_path(workspace: &Path) -> Result<PathBuf> {
    if workspace.as_os_str().is_empty() {
        anyhow::bail!("workspace path cannot be empty");
    }
    if workspace
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        anyhow::bail!("workspace path cannot contain '..' components");
    }
    let absolute = if workspace.is_absolute() {
        workspace.to_path_buf()
    } else {
        std::env::current_dir()
            .context("failed to resolve current directory for workspace")?
            .join(workspace)
    };
    match absolute.canonicalize() {
        Ok(path) => Ok(path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(normalize_path_components(&absolute))
        }
        Err(err) => {
            Err(err).with_context(|| format!("failed to resolve workspace {}", workspace.display()))
        }
    }
}

fn normalize_workspace_path(workspace: &Path) -> PathBuf {
    if let Ok(canonical) = workspace.canonicalize() {
        return canonical;
    }
    let absolute = if workspace.is_absolute() {
        workspace.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(workspace)
    };
    normalize_path_components(&absolute)
}

fn resolve_project_mcp_cwd(workspace: &Path, cwd: Option<&Path>) -> Result<PathBuf> {
    let cwd = match cwd {
        Some(cwd) if cwd.is_relative() => normalize_path_components(&workspace.join(cwd)),
        Some(cwd) => normalize_path_components(cwd),
        None => workspace.to_path_buf(),
    };
    let resolved = cwd
        .canonicalize()
        .unwrap_or_else(|_| normalize_path_components(&cwd));
    if !resolved.starts_with(workspace) {
        anyhow::bail!(
            "Project MCP server cwd must stay within workspace: {}",
            resolved.display()
        );
    }
    Ok(resolved)
}

fn normalize_path_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn paths_refer_to_same_config(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => normalize_workspace_path(left) == normalize_workspace_path(right),
    }
}

/// 64-bit content hash of an [`McpConfig`]. Used by [`McpPool`] to decide
/// whether a freshly-read config differs from the one currently driving the
/// live connections. Hashing the JSON serialization avoids forcing every
/// nested config type to derive `Hash` (the timeouts struct, network policy
/// stubs, etc.). The hash is stable across runs of the same Rust toolchain
/// for byte-identical input.
fn hash_mcp_config(config: &McpConfig) -> u64 {
    use std::hash::{Hash, Hasher};
    let bytes = serde_json::to_vec(config).unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Best-effort fetch of the MCP config file's last-modified time. Returns
/// `None` when the file is missing, when stat fails, when the platform
/// doesn't expose mtime, or when the path fails the same allow-list check
/// that `load_config` / `save_config` apply. The lazy-reload check in
/// `McpPool::get_or_connect` treats `None` as "skip the check this turn",
/// so a rejected path simply degrades to "no auto-reload" rather than an
/// error path. Callers already validate via `validate_mcp_config_path` at
/// construction time; the redundant validation here keeps this helper
/// safe-by-construction for any future caller and ties the validation to
/// the call site rather than relying on cross-function reasoning.
fn mcp_config_mtime(path: &Path) -> Option<std::time::SystemTime> {
    validate_mcp_config_path(path).ok()?;
    fs::metadata(path).ok()?.modified().ok()
}

pub fn save_config(path: &Path, cfg: &McpConfig) -> Result<()> {
    validate_mcp_config_path(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create MCP config directory {}", parent.display())
        })?;
    }
    let rendered = serde_json::to_string_pretty(cfg).context("Failed to serialize MCP config")?;
    write_atomic(path, rendered.as_bytes())
        .with_context(|| format!("Failed to write MCP config {}", path.display()))?;
    Ok(())
}

fn mcp_template_json() -> Result<String> {
    let mut cfg = McpConfig::default();
    cfg.servers.insert(
        "example".to_string(),
        McpServerConfig {
            command: Some("node".to_string()),
            args: vec!["./path/to/your-mcp-server.js".to_string()],
            env: HashMap::new(),
            cwd: None,
            url: None,
            transport: None,
            connect_timeout: None,
            execute_timeout: None,
            read_timeout: None,
            disabled: true,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            headers: HashMap::new(),
            env_headers: HashMap::new(),
            bearer_token_env_var: None,
            scopes: Vec::new(),
            oauth: None,
            oauth_resource: None,
        },
    );
    serde_json::to_string_pretty(&cfg).context("Failed to render MCP template JSON")
}

pub fn init_config(path: &Path, force: bool) -> Result<McpWriteStatus> {
    validate_mcp_config_path(path)?;
    if path.exists() && !force {
        return Ok(McpWriteStatus::SkippedExists);
    }
    let status = if path.exists() {
        McpWriteStatus::Overwritten
    } else {
        McpWriteStatus::Created
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create MCP config directory {}", parent.display())
        })?;
    }
    let template = mcp_template_json()?;
    write_atomic(path, template.as_bytes())
        .with_context(|| format!("Failed to write MCP config {}", path.display()))?;
    Ok(status)
}

pub fn add_server_config(
    path: &Path,
    name: String,
    command: Option<String>,
    url: Option<String>,
    args: Vec<String>,
    transport: Option<String>,
) -> Result<()> {
    if command.is_none() && url.is_none() {
        anyhow::bail!("Provide either a command or URL for MCP server '{name}'.");
    }
    validate_mcp_transport(transport.as_deref())?;
    let mut cfg = load_config(path)?;
    cfg.servers.insert(
        name,
        McpServerConfig {
            command,
            args,
            env: HashMap::new(),
            cwd: None,
            url,
            transport,
            connect_timeout: None,
            execute_timeout: None,
            read_timeout: None,
            disabled: false,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            headers: HashMap::new(),
            env_headers: HashMap::new(),
            bearer_token_env_var: None,
            scopes: Vec::new(),
            oauth: None,
            oauth_resource: None,
        },
    );
    save_config(path, &cfg)
}

pub fn remove_server_config(path: &Path, name: &str) -> Result<()> {
    let mut cfg = load_config(path)?;
    if cfg.servers.remove(name).is_none() {
        anyhow::bail!("MCP server '{name}' not found");
    }
    save_config(path, &cfg)
}

pub fn set_server_enabled(path: &Path, name: &str, enabled: bool) -> Result<()> {
    let mut cfg = load_config(path)?;
    let server = cfg
        .servers
        .get_mut(name)
        .ok_or_else(|| anyhow::anyhow!("MCP server '{name}' not found"))?;
    server.enabled = enabled;
    server.disabled = !enabled;
    save_config(path, &cfg)
}

#[cfg(test)]
pub fn manager_snapshot_from_config(
    path: &Path,
    restart_required: bool,
) -> Result<McpManagerSnapshot> {
    let cfg = load_config(path)?;
    Ok(snapshot_from_config(
        path,
        path.exists(),
        restart_required,
        &cfg,
        None,
    ))
}

pub fn manager_snapshot_from_config_with_workspace(
    path: &Path,
    workspace: &Path,
    restart_required: bool,
) -> Result<McpManagerSnapshot> {
    let cfg = load_config_with_workspace(path, workspace)?;
    Ok(snapshot_from_config(
        path,
        path.exists(),
        restart_required,
        &cfg,
        None,
    ))
}

#[cfg(test)]
pub async fn discover_manager_snapshot(
    path: &Path,
    network_policy: Option<NetworkPolicyDecider>,
    restart_required: bool,
) -> Result<McpManagerSnapshot> {
    let cfg = load_config(path)?;
    let mut pool = McpPool::new(cfg.clone());
    if let Some(policy) = network_policy {
        pool = pool.with_network_policy(policy);
    }
    let errors = pool
        .connect_all()
        .await
        .into_iter()
        .map(|(name, err)| (name, format!("{err:#}")))
        .collect::<HashMap<_, _>>();
    Ok(snapshot_from_config(
        path,
        path.exists(),
        restart_required,
        &cfg,
        Some((&pool, &errors)),
    ))
}

pub async fn discover_manager_snapshot_with_workspace(
    path: &Path,
    workspace: &Path,
    network_policy: Option<NetworkPolicyDecider>,
    restart_required: bool,
) -> Result<McpManagerSnapshot> {
    let cfg = load_config_with_workspace(path, workspace)?;
    let mut pool = McpPool::new(cfg.clone());
    if let Some(policy) = network_policy {
        pool = pool.with_network_policy(policy);
    }
    let errors = pool
        .connect_all()
        .await
        .into_iter()
        .map(|(name, err)| (name, format!("{err:#}")))
        .collect::<HashMap<_, _>>();
    Ok(snapshot_from_config(
        path,
        path.exists(),
        restart_required,
        &cfg,
        Some((&pool, &errors)),
    ))
}

fn snapshot_from_config(
    path: &Path,
    config_exists: bool,
    restart_required: bool,
    cfg: &McpConfig,
    discovery: Option<(&McpPool, &HashMap<String, String>)>,
) -> McpManagerSnapshot {
    let mut servers = cfg
        .servers
        .iter()
        .map(|(name, server)| {
            let transport = if server.url.is_some() {
                if is_legacy_sse_transport(server) {
                    "sse"
                } else {
                    "http/sse"
                }
            } else {
                "stdio"
            };
            let command_or_url = server.url.clone().unwrap_or_else(|| {
                let mut command = server
                    .command
                    .clone()
                    .unwrap_or_else(|| "(missing)".to_string());
                if !server.args.is_empty() {
                    command.push(' ');
                    command.push_str(&server.args.join(" "));
                }
                command
            });
            let mut snapshot = McpServerSnapshot {
                name: name.clone(),
                enabled: server.is_enabled(),
                required: server.required,
                transport: transport.to_string(),
                command_or_url,
                connect_timeout: server.effective_connect_timeout(&cfg.timeouts),
                execute_timeout: server.effective_execute_timeout(&cfg.timeouts),
                read_timeout: server.effective_read_timeout(&cfg.timeouts),
                connected: false,
                error: if server.is_enabled() {
                    None
                } else {
                    Some("disabled".to_string())
                },
                tools: Vec::new(),
                resources: Vec::new(),
                prompts: Vec::new(),
            };

            if let Some((pool, errors)) = discovery {
                if let Some(error) = errors.get(name) {
                    snapshot.error = Some(error.clone());
                }
                if let Some(conn) = pool.connections.get(name) {
                    snapshot.connected = conn.is_ready();
                    snapshot.tools = conn
                        .tools()
                        .iter()
                        .filter(|tool| conn.config().is_tool_enabled(&tool.name))
                        .map(|tool| McpDiscoveredItem {
                            name: tool.name.clone(),
                            model_name: format!("mcp_{}_{}", name, tool.name),
                            description: tool.description.clone(),
                        })
                        .collect();
                    snapshot.resources =
                        conn.resources()
                            .iter()
                            .map(|resource| McpDiscoveredItem {
                                name: resource.name.clone(),
                                model_name: format!(
                                    "mcp_{}_{}",
                                    name,
                                    resource.name.replace(' ', "_").to_lowercase()
                                ),
                                description: resource.description.clone(),
                            })
                            .chain(conn.resource_templates().iter().map(|template| {
                                McpDiscoveredItem {
                                    name: template.name.clone(),
                                    model_name: format!(
                                        "mcp_{}_{}",
                                        name,
                                        template.name.replace(' ', "_").to_lowercase()
                                    ),
                                    description: template.description.clone(),
                                }
                            }))
                            .collect();
                    snapshot.prompts = conn
                        .prompts()
                        .iter()
                        .map(|prompt| McpDiscoveredItem {
                            name: prompt.name.clone(),
                            model_name: format!("mcp_{}_{}", name, prompt.name),
                            description: prompt.description.clone(),
                        })
                        .collect();
                }
            }

            snapshot
        })
        .collect::<Vec<_>>();
    servers.sort_by(|a, b| a.name.cmp(&b.name));
    McpManagerSnapshot {
        config_path: path.to_path_buf(),
        config_exists,
        restart_required,
        servers,
    }
}

// === Helper Functions ===

/// Format MCP tool result for display
#[allow(dead_code)] // Will be used when MCP tool results are displayed in TUI
pub fn format_tool_result(result: &serde_json::Value) -> String {
    let is_error = result
        .get("isError")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let content = result
        .get("content")
        .and_then(|v| v.as_array())
        .map_or_else(
            || serde_json::to_string_pretty(result).unwrap_or_default(),
            |arr| {
                arr.iter()
                    .filter_map(|item| match item.get("type")?.as_str()? {
                        "text" => item.get("text")?.as_str().map(String::from),
                        other => Some(format!("[{other} content]")),
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        );

    if is_error {
        format!("Error: {content}")
    } else {
        content
    }
}

// === Unit Tests ===

#[cfg(test)]
mod tests;
