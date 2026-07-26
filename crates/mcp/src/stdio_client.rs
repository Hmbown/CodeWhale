//! A real MCP client that speaks JSON-RPC to a spawned child process.
//!
//! `codewhale mcp-server` used to wire every configured server to an
//! in-memory stub, so a user's `command`/`args`/`env` were never executed and
//! every health probe answered `{"status": "ok"}` from a hardcoded literal
//! (#4727). A fabricated success is the worst possible answer here: it is
//! indistinguishable from a working integration. This module replaces it with
//! an actual subprocess connection, and every failure path below returns an
//! error naming the server rather than a plausible-looking value.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::{McpManagedClient, McpResourceDescriptor, McpServerConfig, McpToolDescriptor};

/// Protocol revision advertised during the handshake. Matches the revision the
/// TUI's MCP pool negotiates (`crates/tui/src/mcp.rs`), so a server that works
/// in the TUI works here.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Budget for spawn + `initialize` + `notifications/initialized`. Generous
/// because a first `npx`/`uvx` launch may download the server package.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Budget for a single request once the server is up.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// How long a dropped client waits for a graceful exit after closing stdin
/// before it kills the child.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(500);

/// How long a failure path waits for a dying child's exit status before giving
/// up and reporting the failure without one.
const EXIT_STATUS_GRACE: Duration = Duration::from_millis(200);

/// Upper bound on `*/list` pagination follow-ups, so a server that keeps
/// echoing the same cursor cannot pin us in a loop.
const MAX_LIST_PAGES: usize = 100;

/// What the server said it supports in its `initialize` response.
///
/// `None` means the server sent no `capabilities` object at all. Those are
/// treated as legacy servers and probed optimistically; an explicit
/// capabilities object is honoured, because a tools-only server answers
/// `resources/list` with a "method not found" error that would otherwise fail
/// the whole aggregated listing.
#[derive(Debug, Clone, Copy)]
struct ServerCapabilities {
    tools: bool,
    resources: bool,
}

/// A live connection to one MCP server subprocess.
pub struct ChildProcessMcpClient {
    server_name: String,
    capabilities: Option<ServerCapabilities>,
    connection: Mutex<Connection>,
    request_timeout: Duration,
}

impl std::fmt::Debug for ChildProcessMcpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChildProcessMcpClient")
            .field("server_name", &self.server_name)
            .finish_non_exhaustive()
    }
}

impl ChildProcessMcpClient {
    /// Spawn `config.command` with `config.args`/`config.env` and complete the
    /// MCP handshake.
    ///
    /// Returns `Err` — never a degraded-but-usable client — when the command
    /// cannot be executed, exits immediately, or does not answer `initialize`
    /// within [`HANDSHAKE_TIMEOUT`].
    pub fn spawn(config: &McpServerConfig) -> Result<Self> {
        Self::spawn_with_timeouts(config, HANDSHAKE_TIMEOUT, REQUEST_TIMEOUT)
    }

    fn spawn_with_timeouts(
        config: &McpServerConfig,
        handshake_timeout: Duration,
        request_timeout: Duration,
    ) -> Result<Self> {
        let server_name = config.name.clone();
        if config.command.trim().is_empty() {
            bail!("MCP server '{server_name}' has no command configured");
        }

        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // The child's diagnostics belong on our stderr: stdout is the
            // JSON-RPC channel and must not be polluted, and swallowing the
            // child's stderr is how a misconfigured server becomes a silent
            // one.
            .stderr(Stdio::inherit());

        let mut child = command.spawn().with_context(|| {
            format!(
                "MCP server '{server_name}': failed to spawn command '{}'",
                config.command
            )
        })?;

        let stdin = child
            .stdin
            .take()
            .with_context(|| format!("MCP server '{server_name}': child stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .with_context(|| format!("MCP server '{server_name}': child stdout unavailable"))?;

        // A dedicated reader thread keeps `recv_timeout` able to bound a wait
        // that a blocking `read_line` on the child would not.
        let (sender, responses) = channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let mut connection = Connection {
            child,
            stdin: Some(stdin),
            responses,
            next_id: 1,
        };

        let initialize = connection.request(
            &server_name,
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "clientInfo": {
                    "name": "codewhale-mcp-server",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "tools": {},
                    "resources": {}
                }
            }),
            handshake_timeout,
        )?;

        connection
            .send(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }))
            .with_context(|| {
                format!("MCP server '{server_name}': failed to confirm initialization")
            })?;

        let capabilities = initialize
            .get("capabilities")
            .and_then(Value::as_object)
            .map(|caps| ServerCapabilities {
                tools: caps.contains_key("tools"),
                resources: caps.contains_key("resources"),
            });

        Ok(Self {
            server_name,
            capabilities,
            connection: Mutex::new(connection),
            request_timeout,
        })
    }

    fn supports_tools(&self) -> bool {
        self.capabilities.is_none_or(|caps| caps.tools)
    }

    fn supports_resources(&self) -> bool {
        self.capabilities.is_none_or(|caps| caps.resources)
    }

    fn request(&self, method: &str, params: Value) -> Result<Value> {
        let mut connection = self.connection.lock().map_err(|_| {
            anyhow!(
                "MCP server '{}': connection poisoned by an earlier panic",
                self.server_name
            )
        })?;
        connection.request(&self.server_name, method, params, self.request_timeout)
    }

    /// Drive a paginated `*/list` method to exhaustion, collecting `field`.
    fn list_paginated(&self, method: &str, field: &str) -> Result<Vec<Value>> {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_LIST_PAGES {
            let params = match &cursor {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };
            let page = self.request(method, params)?;
            if let Some(values) = page.get(field).and_then(Value::as_array) {
                items.extend(values.iter().cloned());
            }
            let next = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            match next {
                // A repeated cursor is a server bug; stop rather than spin.
                Some(next) if Some(&next) != cursor.as_ref() => cursor = Some(next),
                _ => break,
            }
        }
        Ok(items)
    }
}

impl McpManagedClient for ChildProcessMcpClient {
    fn list_tools(&self) -> Result<Vec<McpToolDescriptor>> {
        if !self.supports_tools() {
            return Ok(Vec::new());
        }
        let tools = self.list_paginated("tools/list", "tools")?;
        Ok(tools
            .iter()
            .filter_map(|tool| {
                let tool_name = tool.get("name")?.as_str()?.to_string();
                Some(McpToolDescriptor {
                    server_name: self.server_name.clone(),
                    // The manager owns qualification; report the raw name and
                    // let it build `mcp__server__tool`.
                    qualified_name: tool_name.clone(),
                    tool_name,
                    description: tool
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
            })
            .collect())
    }

    fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<Value> {
        // The server's result is returned verbatim, including an `isError`
        // content payload: reinterpreting it here would replace what the
        // server actually said with our guess about it.
        self.request(
            "tools/call",
            json!({
                "name": tool_name,
                "arguments": arguments
            }),
        )
    }

    fn list_resources(&self) -> Result<Vec<McpResourceDescriptor>> {
        if !self.supports_resources() {
            return Ok(Vec::new());
        }
        let resources = self.list_paginated("resources/list", "resources")?;
        Ok(resources
            .iter()
            .filter_map(|resource| {
                Some(McpResourceDescriptor {
                    server_name: self.server_name.clone(),
                    uri: resource.get("uri")?.as_str()?.to_string(),
                    description: resource
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
            })
            .collect())
    }

    fn read_resource(&self, uri: &str) -> Result<Value> {
        self.request("resources/read", json!({ "uri": uri }))
    }
}

struct Connection {
    child: Child,
    stdin: Option<ChildStdin>,
    responses: Receiver<String>,
    next_id: u64,
}

impl Connection {
    fn send(&mut self, message: &Value) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .context("connection stdin already closed")?;
        let mut line = serde_json::to_string(message)?;
        line.push('\n');
        stdin.write_all(line.as_bytes())?;
        stdin.flush()?;
        Ok(())
    }

    fn request(
        &mut self,
        server: &str,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        if let Err(err) = self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        })) {
            // A child that has already exited leaves us racing two symptoms of
            // the same fact: either the reader thread sees EOF first, or our
            // write loses the race and returns EPIPE. Which one wins is
            // platform- and timing-dependent (macOS reliably reports the write
            // error where Linux reports the EOF), so both report the death the
            // same way rather than leaking a bare "Broken pipe".
            if is_broken_pipe(&err) {
                bail!(
                    "MCP server '{server}': process closed stdin before answering {method}{}",
                    self.exit_note()
                );
            }
            return Err(err)
                .with_context(|| format!("MCP server '{server}': failed to send {method}"));
        }

        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("MCP server '{server}': {method} timed out after {timeout:?}");
            }
            let line = match self.responses.recv_timeout(remaining) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => {
                    bail!("MCP server '{server}': {method} timed out after {timeout:?}");
                }
                Err(RecvTimeoutError::Disconnected) => {
                    bail!(
                        "MCP server '{server}': process closed stdout before answering {method}{}",
                        self.exit_note()
                    );
                }
            };

            // Servers occasionally emit banners or log lines on stdout, and
            // notifications carry no id. Both are skipped; only the matching
            // response ends the wait.
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                bail!("MCP server '{server}': {method} failed: {error}");
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// The child's exit status, when it has one, for appending to a failure
    /// message. Polls briefly because both callers run at the moment the child
    /// is dying: the write can return EPIPE, or stdout can hit EOF, before the
    /// kernel has finished reaping the process. Bounded and error-path-only,
    /// so the cost buys a real diagnostic ("exited with status 127" is the
    /// difference between a crashed server and a missing one).
    fn exit_note(&mut self) -> String {
        let deadline = Instant::now() + EXIT_STATUS_GRACE;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return format!(" (process exited with {status})"),
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                _ => return String::new(),
            }
        }
    }
}

/// Whether an error chain bottoms out in a broken-pipe I/O error, i.e. we wrote
/// to a child that had already closed its end.
fn is_broken_pipe(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
    })
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Closing stdin is the protocol-level shutdown signal for a stdio MCP
        // server; kill only the ones that ignore it, so servers get a chance
        // to flush state.
        self.stdin.take();
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                _ => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn config(command: &str, args: &[&str]) -> McpServerConfig {
        McpServerConfig {
            name: "probe".to_string(),
            command: command.to_string(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            env: HashMap::new(),
            enabled: true,
        }
    }

    #[test]
    fn spawn_fails_loudly_when_the_command_does_not_exist() {
        let err = ChildProcessMcpClient::spawn(&config(
            "codewhale-nonexistent-mcp-server-binary",
            &["--stdio"],
        ))
        .unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("failed to spawn command"),
            "spawn failure must name the command, got: {message}"
        );
        assert!(
            message.contains("probe"),
            "spawn failure must name the server, got: {message}"
        );
    }

    #[test]
    fn spawn_rejects_an_empty_command() {
        let err = ChildProcessMcpClient::spawn(&config("   ", &[])).unwrap_err();
        assert!(
            format!("{err:#}").contains("no command configured"),
            "unexpected error: {err:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn spawn_fails_when_the_child_exits_without_answering_initialize() {
        let err = ChildProcessMcpClient::spawn(&config("/bin/sh", &["-c", "exit 0"])).unwrap_err();
        let message = format!("{err:#}");
        // Which end reports the death first is a race the OS arbitrates —
        // stdout EOF on Linux, an EPIPE write on macOS — so assert on what is
        // actually contractual: the server is named, the failure is attributed
        // to the child dying before it answered, and no raw io::Error leaks.
        assert!(
            message.contains("probe") && message.contains("before answering initialize"),
            "unexpected error: {message}"
        );
        assert!(
            !message.contains("Broken pipe"),
            "a dead child must not surface as a raw pipe error: {message}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_child_that_dies_mid_handshake_reports_its_exit_status() {
        // The child closes both pipes and exits nonzero: the diagnostic has to
        // carry the status, because "exited with 127" is what distinguishes a
        // crashed server from a missing one.
        let err =
            ChildProcessMcpClient::spawn(&config("/bin/sh", &["-c", "exit 127"])).unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("before answering initialize") && message.contains("127"),
            "unexpected error: {message}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn handshake_times_out_when_the_child_never_answers() {
        let err = ChildProcessMcpClient::spawn_with_timeouts(
            &config("/bin/sh", &["-c", "sleep 30"]),
            Duration::from_millis(250),
            Duration::from_millis(250),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("initialize timed out"),
            "unexpected error: {err:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_real_child_answers_tools_list_and_tools_call() {
        let script = crate::test_support::write_fake_mcp_server("stdio_client_roundtrip");
        let client = ChildProcessMcpClient::spawn(&config(
            "/bin/sh",
            &[script.path().to_str().expect("utf-8 script path")],
        ))
        .expect("fake MCP server should complete the handshake");

        let tools = client.list_tools().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool_name, "add");

        let result = client.call_tool("add", json!({"a": 2, "b": 3})).unwrap();
        assert_eq!(
            result["content"][0]["text"], "5",
            "the answer must come from the child process: {result}"
        );

        // The stub's fabricated tools must be gone.
        assert!(client.call_tool("health", json!({})).is_err());
    }
}
