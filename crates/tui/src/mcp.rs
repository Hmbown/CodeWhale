//! Async MCP (Model Context Protocol) Implementation
//!
//! This module provides full async support for MCP servers with:
//! - Connection pooling for server reuse
//! - Automatic tool discovery via `tools/list`
//! - Configurable timeouts per-server and globally

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::child_env;
use crate::network_policy::{Decision, NetworkPolicyDecider, host_from_url};
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
    pub url: Option<String>,
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
    async fn send(&mut self, msg: serde_json::Value) -> Result<()>;
    async fn recv(&mut self) -> Result<serde_json::Value>;

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
}

/// How long `StdioTransport::shutdown` waits for the child to exit on SIGTERM
/// before `kill_on_drop` fires SIGKILL. Tuned short so a hung MCP server
/// can't stall TUI exit; well-behaved servers almost always exit within
/// a few hundred ms.
const STDIO_SHUTDOWN_GRACE: Duration = Duration::from_millis(2_000);

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
    async fn send(&mut self, msg: serde_json::Value) -> Result<()> {
        let line = serde_json::to_string(&msg)? + "\n";
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn recv(&mut self) -> Result<serde_json::Value> {
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = self.reader.read_line(&mut line).await?;
            if bytes == 0 {
                anyhow::bail!("Stdio transport closed");
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
                return Ok(value);
            }
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
    endpoint_url: Option<String>,
    receiver: tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>,
    pending_messages: VecDeque<serde_json::Value>,
}

impl SseTransport {
    pub async fn connect(
        client: reqwest::Client,
        url: String,
        cancel_token: tokio_util::sync::CancellationToken,
        endpoint_timeout: Duration,
    ) -> Result<Self> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let client_clone = client.clone();
        let url_clone = url.clone();
        let wait_cancel_token = cancel_token.clone();

        tokio::spawn(async move {
            if cancel_token.is_cancelled() {
                return;
            }
            use futures_util::FutureExt;
            let result = std::panic::AssertUnwindSafe(Self::run_sse_loop(
                client_clone,
                url_clone,
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
            endpoint_url: None,
            receiver: rx,
            pending_messages: VecDeque::new(),
        };
        transport
            .wait_for_endpoint(&wait_cancel_token, endpoint_timeout)
            .await?;
        Ok(transport)
    }

    async fn run_sse_loop(
        client: reqwest::Client,
        url: String,
        tx: tokio::sync::mpsc::UnboundedSender<serde_json::Value>,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        let response = client.get(&url).send().await.with_context(|| {
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

            while let Some(pos) = buffer.find("\n\n") {
                let event_block = buffer[..pos].to_string();
                buffer = buffer[pos + 2..].to_string();

                let mut event_type = "message";
                let mut data = String::new();

                for line in event_block.lines() {
                    if let Some(stripped) = line.strip_prefix("event: ") {
                        event_type = stripped;
                    } else if let Some(stripped) = line.strip_prefix("data: ") {
                        data.push_str(stripped);
                    }
                }

                match event_type {
                    "endpoint" => {
                        // Special internal message to set endpoint
                        let _ = tx.send(serde_json::json!({
                            "__internal_sse_endpoint__": data
                        }));
                    }
                    "message" => {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&data) {
                            let _ = tx.send(val);
                        }
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

            if self.store_endpoint_from_internal_message(&msg)? {
                return Ok(());
            }

            self.pending_messages.push_back(msg);
        }
    }

    fn store_endpoint_from_internal_message(&mut self, msg: &serde_json::Value) -> Result<bool> {
        let Some(endpoint) = msg.get("__internal_sse_endpoint__") else {
            return Ok(false);
        };
        let url_str = endpoint.as_str().context("Invalid endpoint format")?;
        self.endpoint_url = Some(Self::resolve_endpoint_url(&self.base_url, url_str)?);
        Ok(true)
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

#[async_trait::async_trait]
impl McpTransport for SseTransport {
    async fn send(&mut self, msg: serde_json::Value) -> Result<()> {
        let endpoint = self
            .endpoint_url
            .as_ref()
            .context("SSE endpoint not yet discovered")?;
        let response = self.client.post(endpoint).json(&msg).send().await?;
        if !response.status().is_success() {
            anyhow::bail!("Failed to send message via SSE POST: {}", response.status());
        }
        Ok(())
    }

    async fn recv(&mut self) -> Result<serde_json::Value> {
        loop {
            let msg = if let Some(msg) = self.pending_messages.pop_front() {
                msg
            } else {
                self.receiver.recv().await.context("SSE transport closed")?
            };
            if self.store_endpoint_from_internal_message(&msg)? {
                continue;
            }
            return Ok(msg);
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
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(connect_timeout_secs))
                .build()?;
            Box::new(
                SseTransport::connect(
                    client,
                    url.clone(),
                    cancel_token.clone(),
                    Duration::from_secs(connect_timeout_secs),
                )
                .await?,
            )
        } else if let Some(command) = &config.command {
            let mut cmd = tokio::process::Command::new(command);
            cmd.args(&config.args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);

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

            Box::new(StdioTransport {
                child,
                stdin,
                reader: tokio::io::BufReader::new(stdout),
            })
        } else {
            anyhow::bail!(
                "MCP server '{}' config must have either 'command' or 'url'",
                name
            );
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
            "id": init_id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "clientInfo": {
                    "name": "deepseek-tui",
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
        let list_id = self.next_id();
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": list_id,
            "method": "tools/list",
            "params": {}
        }))
        .await?;

        let response = self.recv(list_id).await?;

        if let Some(result) = response.get("result")
            && let Some(tools) = result.get("tools")
        {
            self.tools = serde_json::from_value(tools.clone()).unwrap_or_default();
        }

        Ok(())
    }

    /// Discover available resources from the MCP server
    async fn discover_resources(&mut self) -> Result<()> {
        let list_id = self.next_id();
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": list_id,
            "method": "resources/list",
            "params": {}
        }))
        .await?;

        let response = self.recv(list_id).await?;

        if let Some(result) = response.get("result")
            && let Some(resources) = result.get("resources")
        {
            self.resources = serde_json::from_value(resources.clone()).unwrap_or_default();
        }

        Ok(())
    }

    /// Discover available resource templates from the MCP server
    async fn discover_resource_templates(&mut self) -> Result<()> {
        let list_id = self.next_id();
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": list_id,
            "method": "resources/templates/list",
            "params": {}
        }))
        .await?;

        let response = self.recv(list_id).await?;

        if let Some(result) = response.get("result") {
            let templates = result
                .get("resourceTemplates")
                .or_else(|| result.get("templates"))
                .or_else(|| result.get("resource_templates"));
            if let Some(templates) = templates {
                self.resource_templates =
                    serde_json::from_value(templates.clone()).unwrap_or_default();
            }
        }

        Ok(())
    }

    /// Discover available prompts from the MCP server
    async fn discover_prompts(&mut self) -> Result<()> {
        let list_id = self.next_id();
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": list_id,
            "method": "prompts/list",
            "params": {}
        }))
        .await?;

        let response = self.recv(list_id).await?;

        if let Some(result) = response.get("result")
            && let Some(prompts) = result.get("prompts")
        {
            self.prompts = serde_json::from_value(prompts.clone()).unwrap_or_default();
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
            "id": call_id,
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

    fn next_id(&self) -> u64 {
        self.request_id.fetch_add(1, Ordering::SeqCst)
    }

    async fn send(&mut self, msg: serde_json::Value) -> Result<()> {
        self.transport.send(msg).await
    }

    async fn recv(&mut self, expected_id: u64) -> Result<serde_json::Value> {
        loop {
            let value = self.transport.recv().await.inspect_err(|_e| {
                self.state = ConnectionState::Disconnected;
            })?;

            // Check if this is a response with the expected id
            if value.get("id").and_then(serde_json::Value::as_u64) == Some(expected_id) {
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
}

impl McpPool {
    /// Create a new pool with the given configuration
    pub fn new(config: McpConfig) -> Self {
        Self {
            connections: HashMap::new(),
            config,
            network_policy: None,
        }
    }

    /// Create a pool from a configuration file path
    pub fn from_config_path(path: &std::path::Path) -> Result<Self> {
        validate_mcp_config_path(path)?;
        let config = if path.exists() {
            let contents = fs::read_to_string(path)
                .with_context(|| format!("Failed to read MCP config: {}", path.display()))?;
            serde_json::from_str(&contents)
                .with_context(|| format!("Failed to parse MCP config: {}", path.display()))?
        } else {
            McpConfig::default()
        };
        Ok(Self::new(config))
    }

    /// Attach a per-domain network policy (#135). When set, HTTP/SSE
    /// transports are gated through it; STDIO transports are unaffected.
    pub fn with_network_policy(mut self, policy: NetworkPolicyDecider) -> Self {
        self.network_policy = Some(policy);
        self
    }

    /// Get or create a connection to a server
    pub async fn get_or_connect(&mut self, server_name: &str) -> Result<&mut McpConnection> {
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

        self.connections.remove(server_name);

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
                resources.push((format!("mcp_{}_{}", server, safe_name), resource));
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
                templates.push((format!("mcp_{}_{}", server, safe_name), template));
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

        let _ = self.connect_all().await;
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

        let _ = self.connect_all().await;
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
    fn parse_prefixed_name<'a>(&self, prefixed_name: &'a str) -> Result<(&'a str, &'a str)> {
        if !prefixed_name.starts_with("mcp_") {
            anyhow::bail!("Invalid MCP tool name: {}", prefixed_name);
        }
        let rest = &prefixed_name[4..];
        let Some((server, tool)) = rest.split_once('_') else {
            anyhow::bail!("Invalid MCP tool name format: {}", prefixed_name);
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
        conn.call_tool(tool_name, arguments, timeout).await
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
        self.connections.clear();
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
    if !path.exists() {
        return Ok(McpConfig::default());
    }
    let contents = fs::read_to_string(path)
        .with_context(|| format!("Failed to read MCP config {}", path.display()))?;
    serde_json::from_str(&contents)
        .with_context(|| format!("Failed to parse MCP config {}", path.display()))
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
            url: None,
            connect_timeout: None,
            execute_timeout: None,
            read_timeout: None,
            disabled: true,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
        },
    );
    serde_json::to_string_pretty(&cfg).context("Failed to render MCP template JSON")
}

pub fn init_config(path: &Path, force: bool) -> Result<McpWriteStatus> {
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
) -> Result<()> {
    if command.is_none() && url.is_none() {
        anyhow::bail!("Provide either a command or URL for MCP server '{name}'.");
    }
    let mut cfg = load_config(path)?;
    cfg.servers.insert(
        name,
        McpServerConfig {
            command,
            args,
            env: HashMap::new(),
            url,
            connect_timeout: None,
            execute_timeout: None,
            read_timeout: None,
            disabled: false,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
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
                "http/sse"
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
mod tests {
    use super::*;

    #[test]
    fn test_mcp_config_defaults() {
        let config = McpConfig::default();
        assert_eq!(config.timeouts.connect_timeout, 10);
        assert_eq!(config.timeouts.execute_timeout, 60);
        assert_eq!(config.timeouts.read_timeout, 120);
        assert!(config.servers.is_empty());
    }

    #[test]
    fn test_mcp_config_parse() {
        let json = r#"{
            "timeouts": {
                "connect_timeout": 15,
                "execute_timeout": 90
            },
            "servers": {
                "test": {
                    "command": "node",
                    "args": ["server.js"],
                    "env": {"FOO": "bar"}
                }
            }
        }"#;

        let config: McpConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.timeouts.connect_timeout, 15);
        assert_eq!(config.timeouts.execute_timeout, 90);
        assert_eq!(config.timeouts.read_timeout, 120); // default
        assert!(config.servers.contains_key("test"));

        let server = config.servers.get("test").unwrap();
        assert_eq!(server.command, Some("node".to_string()));
        assert_eq!(server.args, vec!["server.js"]);
        assert_eq!(server.env.get("FOO"), Some(&"bar".to_string()));
    }

    #[test]
    fn test_mcp_config_parse_mcp_servers_alias_and_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        fs::write(
            &path,
            r#"{
              "mcpServers": {
                "disabled": {
                  "command": "node",
                  "args": ["server.js"],
                  "disabled": true
                }
              }
            }"#,
        )
        .unwrap();

        let cfg = load_config(&path).unwrap();
        assert!(cfg.servers.contains_key("disabled"));
        let snapshot = manager_snapshot_from_config(&path, true).unwrap();
        assert!(snapshot.restart_required);
        assert_eq!(snapshot.servers[0].name, "disabled");
        assert!(!snapshot.servers[0].enabled);
        assert_eq!(snapshot.servers[0].error.as_deref(), Some("disabled"));
    }

    #[test]
    fn test_mcp_config_rejects_traversal_path() {
        let err = load_config(Path::new("../mcp.json")).expect_err("traversal path should fail");
        assert!(
            format!("{err:#}").contains("cannot contain '..'"),
            "got: {err:#}"
        );
    }

    #[test]
    fn test_mcp_config_manager_actions_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");

        assert_eq!(init_config(&path, false).unwrap(), McpWriteStatus::Created);
        assert_eq!(
            init_config(&path, false).unwrap(),
            McpWriteStatus::SkippedExists
        );

        add_server_config(
            &path,
            "local".to_string(),
            Some("node".to_string()),
            None,
            vec!["server.js".to_string()],
        )
        .unwrap();
        set_server_enabled(&path, "local", false).unwrap();
        let disabled = manager_snapshot_from_config(&path, true).unwrap();
        let local = disabled
            .servers
            .iter()
            .find(|server| server.name == "local")
            .unwrap();
        assert!(!local.enabled);
        assert_eq!(local.transport, "stdio");

        remove_server_config(&path, "local").unwrap();
        let removed = manager_snapshot_from_config(&path, true).unwrap();
        assert!(removed.servers.iter().all(|server| server.name != "local"));
    }

    #[test]
    fn test_server_effective_timeouts() {
        let global = McpTimeouts::default();

        let server_with_override = McpServerConfig {
            command: Some("test".to_string()),
            args: vec![],
            env: HashMap::new(),
            url: None,
            connect_timeout: Some(20),
            execute_timeout: None,
            read_timeout: Some(180),
            disabled: false,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
        };

        assert_eq!(server_with_override.effective_connect_timeout(&global), 20);
        assert_eq!(server_with_override.effective_execute_timeout(&global), 60); // global default
        assert_eq!(server_with_override.effective_read_timeout(&global), 180);
    }

    #[test]
    fn test_mcp_pool_is_mcp_tool() {
        assert!(McpPool::is_mcp_tool("mcp_filesystem_read"));
        assert!(McpPool::is_mcp_tool("mcp_git_status"));
        assert!(McpPool::is_mcp_tool("list_mcp_resources"));
        assert!(McpPool::is_mcp_tool("list_mcp_resource_templates"));
        assert!(McpPool::is_mcp_tool("read_mcp_resource"));
        assert!(!McpPool::is_mcp_tool("read_file"));
        assert!(!McpPool::is_mcp_tool("exec_shell"));
    }

    #[test]
    fn test_format_tool_result_text() {
        let result = serde_json::json!({
            "content": [
                {"type": "text", "text": "Hello, world!"}
            ]
        });
        assert_eq!(format_tool_result(&result), "Hello, world!");
    }

    #[test]
    fn test_format_tool_result_error() {
        let result = serde_json::json!({
            "isError": true,
            "content": [
                {"type": "text", "text": "Something went wrong"}
            ]
        });
        assert_eq!(format_tool_result(&result), "Error: Something went wrong");
    }

    #[test]
    fn test_format_tool_result_multiple_content() {
        let result = serde_json::json!({
            "content": [
                {"type": "text", "text": "Line 1"},
                {"type": "text", "text": "Line 2"},
                {"type": "image", "data": "base64..."}
            ]
        });
        let formatted = format_tool_result(&result);
        assert!(formatted.contains("Line 1"));
        assert!(formatted.contains("Line 2"));
        assert!(formatted.contains("[image content]"));
    }

    #[tokio::test]
    async fn test_mcp_pool_empty_config() {
        let pool = McpPool::new(McpConfig::default());
        assert!(pool.server_names().is_empty());
        assert!(pool.all_tools().is_empty());
    }

    /// #1244: when an MCP stdio server fails to spawn, the underlying OS
    /// error (e.g. ENOENT for a missing binary) must reach the user via the
    /// snapshot.error string. Regression test for `err.to_string()` dropping
    /// the anyhow chain — without `{err:#}` the user sees only the opaque
    /// wrapper "MCP stdio spawn failed (...)" and has nothing to act on.
    #[tokio::test]
    async fn discover_snapshot_includes_underlying_spawn_error_in_chain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "broken": {
                        "command": "deepseek-tui-test-this-binary-does-not-exist-9f8e7d6c5b4a",
                        "args": []
                    }
                }
            }"#,
        )
        .unwrap();

        let snapshot = discover_manager_snapshot(&path, None, false).await.unwrap();
        let server = snapshot
            .servers
            .iter()
            .find(|s| s.name == "broken")
            .expect("broken server should appear in snapshot");
        let err = server
            .error
            .as_deref()
            .expect("broken server should have an error");
        let lowered = err.to_lowercase();
        assert!(
            lowered.contains("os error")
                || lowered.contains("not found")
                || lowered.contains("no such"),
            "expected underlying spawn error in chain, got: {err}"
        );
    }

    #[test]
    fn mask_url_secrets_strips_userinfo() {
        let masked = mask_url_secrets("https://user:s3cret@host.example/api?foo=bar");
        assert!(masked.contains("***"), "expected masked userinfo: {masked}");
        assert!(!masked.contains("s3cret"), "secret leaked: {masked}");
        assert!(masked.contains("host.example"), "host preserved: {masked}");
    }

    #[test]
    fn mask_url_secrets_passes_through_clean_url() {
        assert_eq!(
            mask_url_secrets("https://api.example.com/mcp"),
            "https://api.example.com/mcp"
        );
    }

    #[test]
    fn redact_body_preview_masks_bearer_token() {
        let redacted = redact_body_preview("Authorization: Bearer abc.def.ghi end");
        assert!(redacted.contains("Bearer ***"), "redacted: {redacted}");
        assert!(!redacted.contains("abc.def.ghi"), "leaked: {redacted}");
    }

    #[test]
    fn redact_body_preview_masks_api_key_param() {
        let redacted = redact_body_preview("error message api_key=sk-12345&other=val");
        assert!(redacted.contains("api_key=***"), "redacted: {redacted}");
        assert!(!redacted.contains("sk-12345"), "leaked: {redacted}");
        assert!(
            redacted.contains("other=val"),
            "non-secret preserved: {redacted}"
        );
    }

    /// #420: `StdioTransport::shutdown` reaps the child process by sending
    /// SIGTERM and giving it a brief grace period before drop fires SIGKILL.
    /// The test spawns `cat` (which exits immediately on stdin EOF / SIGTERM)
    /// and verifies the transport tears down cleanly. Unix-only because
    /// SIGTERM doesn't exist on Windows; on Windows the test would just
    /// duplicate the kill_on_drop path.
    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_transport_shutdown_terminates_child() {
        use tokio::process::Command as TokioCommand;
        let mut cmd = TokioCommand::new("cat");
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd.spawn().expect("spawn cat");
        let pid = child.id().expect("child pid");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let mut transport = StdioTransport {
            child,
            stdin,
            reader: tokio::io::BufReader::new(stdout),
        };

        // shutdown() should send SIGTERM and complete within the grace window.
        let start = std::time::Instant::now();
        transport.shutdown().await;
        let elapsed = start.elapsed();
        assert!(
            elapsed < STDIO_SHUTDOWN_GRACE + Duration::from_millis(500),
            "shutdown blocked beyond grace window: {elapsed:?}"
        );

        // The child should be reaped — kill(pid, 0) returning ESRCH means
        // the pid is gone. If it's still alive, kill(0) returns 0, which
        // means our shutdown didn't terminate it.
        // SAFETY: pid was just collected from a tokio Child we spawned.
        // libc::kill with signal 0 only checks pid existence and is
        // async-signal-safe.
        let still_alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        assert!(
            !still_alive,
            "child {pid} survived StdioTransport::shutdown — SIGTERM not delivered"
        );
    }

    #[tokio::test]
    async fn sse_connect_waits_for_endpoint_before_first_send() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering as AtomicOrdering},
        };
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let post_seen = Arc::new(AtomicBool::new(false));
        let server_post_seen = Arc::clone(&post_seen);
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let server_cancel = cancel_token.clone();

        let server = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let post_seen = Arc::clone(&server_post_seen);
                let server_cancel = server_cancel.clone();
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buf = [0; 1024];
                    loop {
                        let n = socket.read(&mut buf).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        request.extend_from_slice(&buf[..n]);
                        if request.windows(4).any(|window| window == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let request = String::from_utf8_lossy(&request);
                    if request.starts_with("GET /sse ") {
                        socket
                            .write_all(
                                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n",
                            )
                            .await
                            .unwrap();
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        socket
                            .write_all(b"event: endpoint\ndata: /messages\n\n")
                            .await
                            .unwrap();
                        server_cancel.cancelled().await;
                    } else if request.starts_with("POST /messages ") {
                        post_seen.store(true, AtomicOrdering::SeqCst);
                        socket
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                            .await
                            .unwrap();
                    }
                });
            }
        });

        let client = reqwest::Client::new();
        let url = format!("http://{addr}/sse");
        let mut transport =
            SseTransport::connect(client, url, cancel_token.clone(), Duration::from_secs(2))
                .await
                .unwrap();

        transport
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize"
            }))
            .await
            .unwrap();

        assert!(
            post_seen.load(AtomicOrdering::SeqCst),
            "first SSE send should POST to the discovered endpoint"
        );

        cancel_token.cancel();
        server.abort();
    }
}
