//! A real MCP client that speaks JSON-RPC to a spawned child process.
//!
//! `codewhale mcp-server` used to wire every configured server to an
//! in-memory stub, so a user's `command`/`args`/`env` were never executed and
//! every health probe answered `{"status": "ok"}` from a hardcoded literal
//! (#4727). A fabricated success is the worst possible answer here: it is
//! indistinguishable from a working integration. This module replaces it with
//! an actual subprocess connection, and every failure path below returns an
//! error naming the server rather than a plausible-looking value.

use std::collections::HashSet;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::sync::{Arc, Mutex};
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

/// Hard bounds for paginated catalog discovery. These match the TUI's MCP
/// catalog guardrails so the proxy cannot turn one malformed catalog into an
/// unbounded allocation or wait.
const MAX_LIST_PAGES: usize = 64;
const MAX_LIST_ITEMS: usize = 4_096;
const MAX_LIST_BYTES: usize = 32 * 1024 * 1024;
/// A single JSON-RPC message cannot be larger than the entire catalog budget.
/// Enforce this while consuming the child's buffered stdout, before a newline
/// (or EOF) can make an unbounded `read_line` allocation visible to the proxy.
pub(crate) const MAX_JSONRPC_LINE_BYTES: usize = MAX_LIST_BYTES;
/// Bound queued child output as well as individual lines. When the consumer is
/// slower than the child, the reader blocks and the OS pipe supplies the rest
/// of the backpressure instead of allowing an unbounded heap queue. A
/// zero-capacity channel is deliberate: at most the line currently being read
/// and the line currently being handled can be resident in userspace.
const MAX_PENDING_CHILD_MESSAGES: usize = 0;

fn valid_tool_input_schema(input_schema: &Value) -> bool {
    let Some(schema) = input_schema.as_object() else {
        return false;
    };
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return false;
    }
    if let Some(properties) = schema.get("properties") {
        let Some(properties) = properties.as_object() else {
            return false;
        };
        if properties.values().any(|property| !property.is_object()) {
            return false;
        }
    }
    if let Some(required) = schema.get("required") {
        let Some(required) = required.as_array() else {
            return false;
        };
        if required.iter().any(|name| !name.is_string()) {
            return false;
        }
    }
    true
}

fn valid_annotations(annotations: &Value) -> bool {
    let Some(annotations) = annotations.as_object() else {
        return false;
    };
    if let Some(audience) = annotations.get("audience") {
        let Some(audience) = audience.as_array() else {
            return false;
        };
        if audience
            .iter()
            .any(|role| !matches!(role.as_str(), Some("user") | Some("assistant")))
        {
            return false;
        }
    }
    if let Some(priority) = annotations.get("priority") {
        let Some(priority) = priority.as_f64() else {
            return false;
        };
        if !priority.is_finite() || !(0.0..=1.0).contains(&priority) {
            return false;
        }
    }
    true
}

fn optional_string_field(
    fields: &serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<Option<String>> {
    match fields.get(field) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => bail!("{context}.{field} must be a string"),
    }
}

fn parse_tool_entry(
    server_name: &str,
    tool: &Value,
    index: usize,
    allow_legacy_schema_omission: bool,
) -> Result<(McpToolDescriptor, Value)> {
    let context = format!("MCP server '{server_name}': tools/list tools[{index}]");
    let fields = tool
        .as_object()
        .with_context(|| format!("{context} must be an object"))?;
    let tool_name = fields
        .get("name")
        .and_then(Value::as_str)
        .with_context(|| format!("{context}.name must be a string"))?
        .to_string();
    let description = optional_string_field(fields, "description", &context)?;
    let input_schema = match fields.get("inputSchema") {
        Some(schema) if valid_tool_input_schema(schema) => schema.clone(),
        Some(_) => bail!("{context}.inputSchema must be a valid object-shaped MCP input schema"),
        None if allow_legacy_schema_omission => {
            // Servers that omit the entire initialize capabilities object are
            // already identified as legacy by `validate_initialize_result`.
            // Preserve omission compatibility only for that explicit mode;
            // a standard advertised tools capability must send inputSchema.
            json!({"type": "object", "properties": {}})
        }
        None => bail!("{context}.inputSchema is required for an advertised MCP tool"),
    };
    Ok((
        McpToolDescriptor {
            server_name: server_name.to_string(),
            // The manager owns qualification; report the raw name and let it
            // build `mcp__server__tool`.
            qualified_name: tool_name.clone(),
            tool_name,
            description,
        },
        input_schema,
    ))
}

fn parse_resource_entry(
    server_name: &str,
    resource: &Value,
    index: usize,
) -> Result<(McpResourceDescriptor, Value)> {
    let context = format!("MCP server '{server_name}': resources/list resources[{index}]");
    let fields = resource
        .as_object()
        .with_context(|| format!("{context} must be an object"))?;
    let uri = fields
        .get("uri")
        .and_then(Value::as_str)
        .with_context(|| format!("{context}.uri must be a string"))?
        .to_string();
    let name = fields
        .get("name")
        .and_then(Value::as_str)
        .with_context(|| format!("{context}.name must be a string"))?
        .to_string();
    let description = optional_string_field(fields, "description", &context)?;
    let mime_type = optional_string_field(fields, "mimeType", &context)?;

    let mut metadata = json!({"name": name});
    if let Some(mime_type) = mime_type {
        metadata["mimeType"] = Value::String(mime_type);
    }
    if let Some(size) = fields.get("size") {
        if size.as_i64().is_none() && size.as_u64().is_none() {
            bail!("{context}.size must be an integer");
        }
        metadata["size"] = size.clone();
    }
    if let Some(annotations) = fields.get("annotations") {
        if !valid_annotations(annotations) {
            bail!("{context}.annotations is not a valid MCP annotations object");
        }
        metadata["annotations"] = annotations.clone();
    }

    Ok((
        McpResourceDescriptor {
            server_name: server_name.to_string(),
            uri,
            description,
        },
        metadata,
    ))
}

/// Read one newline-delimited child message without ever retaining more than
/// `max_bytes`. On an oversized line the reader is intentionally abandoned;
/// continuing after losing JSON-RPC framing would be unsafe.
pub(crate) fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            break;
        }

        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            if line.len().saturating_add(newline) > max_bytes {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("JSON-RPC line exceeded the {max_bytes}-byte limit"),
                ));
            }
            line.extend_from_slice(&available[..newline]);
            reader.consume(newline + 1);
            break;
        }

        if line.len().saturating_add(available.len()) > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("JSON-RPC line exceeded the {max_bytes}-byte limit"),
            ));
        }
        line.extend_from_slice(available);
        let consumed = available.len();
        reader.consume(consumed);
    }

    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line).map(Some).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("child stdout was not valid UTF-8: {err}"),
        )
    })
}

enum ChildStdoutMessage {
    Line(String),
    Invalid(String),
}

fn response_to_server_request(message: &Value) -> Option<Value> {
    let method = message.get("method").and_then(Value::as_str)?;
    let id = message.get("id")?;
    Some(match method {
        "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("Method not found: {method}")
            }
        }),
    })
}

fn write_jsonrpc_line<W: Write>(writer: &mut W, message: &Value) -> Result<()> {
    let mut line = serde_json::to_string(message)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()?;
    Ok(())
}

struct ListBudget {
    method: String,
    pages: usize,
    items: usize,
    bytes: usize,
    max_pages: usize,
    max_items: usize,
    max_bytes: usize,
    seen_cursors: HashSet<String>,
    deadline: Instant,
    overall_timeout: Duration,
}

impl ListBudget {
    fn new(method: &str, timeout: Duration) -> Self {
        Self {
            method: method.to_string(),
            pages: 0,
            items: 0,
            bytes: 0,
            max_pages: MAX_LIST_PAGES,
            max_items: MAX_LIST_ITEMS,
            max_bytes: MAX_LIST_BYTES,
            seen_cursors: HashSet::new(),
            deadline: Instant::now() + timeout,
            overall_timeout: timeout,
        }
    }

    #[cfg(test)]
    fn with_limits(
        method: &str,
        timeout: Duration,
        max_pages: usize,
        max_items: usize,
        max_bytes: usize,
    ) -> Self {
        Self {
            max_pages,
            max_items,
            max_bytes,
            ..Self::new(method, timeout)
        }
    }

    fn remaining_timeout(&self) -> Result<Duration> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!(
                "MCP {} exceeded its overall {:?} deadline",
                self.method,
                self.overall_timeout
            );
        }
        Ok(remaining)
    }

    fn observe_page(&mut self, page: &Value, field: &str) -> Result<Option<String>> {
        let values = page.get(field).and_then(Value::as_array).with_context(|| {
            format!(
                "MCP {} response did not contain a '{field}' array",
                self.method
            )
        })?;

        self.pages = self.pages.saturating_add(1);
        self.items = self.items.saturating_add(values.len());
        self.bytes = self.bytes.saturating_add(serde_json::to_vec(page)?.len());
        if self.pages > self.max_pages {
            bail!(
                "MCP {} exceeded the {}-page catalog limit",
                self.method,
                self.max_pages
            );
        }
        if self.items > self.max_items {
            bail!(
                "MCP {} exceeded the {}-item catalog limit",
                self.method,
                self.max_items
            );
        }
        if self.bytes > self.max_bytes {
            bail!(
                "MCP {} exceeded the {}-byte aggregate catalog limit",
                self.method,
                self.max_bytes
            );
        }

        let next_cursor = match page.get("nextCursor") {
            None => None,
            Some(Value::String(cursor)) => Some(cursor.clone()),
            Some(_) => bail!("MCP {} returned a non-string nextCursor", self.method),
        };
        if let Some(cursor) = next_cursor.as_ref()
            && !self.seen_cursors.insert(cursor.clone())
        {
            bail!("MCP {} repeated a pagination cursor", self.method);
        }
        if next_cursor.is_some() && self.pages >= self.max_pages {
            bail!(
                "MCP {} exceeded the {}-page catalog limit",
                self.method,
                self.max_pages
            );
        }
        Ok(next_cursor)
    }
}

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

fn validate_initialize_result(
    server_name: &str,
    result: &Value,
) -> Result<Option<ServerCapabilities>> {
    let response = result.as_object().with_context(|| {
        format!("MCP server '{server_name}': initialize result must be an object")
    })?;
    let protocol_version = response
        .get("protocolVersion")
        .and_then(Value::as_str)
        .with_context(|| {
            format!("MCP server '{server_name}': initialize result omitted protocolVersion")
        })?;
    if protocol_version != PROTOCOL_VERSION {
        bail!(
            "MCP server '{server_name}': unsupported protocol version '{protocol_version}' (expected {PROTOCOL_VERSION})"
        );
    }

    let server_info = response
        .get("serverInfo")
        .and_then(Value::as_object)
        .with_context(|| {
            format!("MCP server '{server_name}': initialize result omitted serverInfo")
        })?;
    for field in ["name", "version"] {
        if !server_info.get(field).is_some_and(Value::is_string) {
            bail!("MCP server '{server_name}': initialize serverInfo.{field} must be a string");
        }
    }

    match response.get("capabilities") {
        None => Ok(None),
        Some(Value::Object(capabilities)) => Ok(Some(ServerCapabilities {
            tools: capabilities.contains_key("tools"),
            resources: capabilities.contains_key("resources"),
        })),
        Some(_) => bail!("MCP server '{server_name}': initialize capabilities must be an object"),
    }
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
    /// within `HANDSHAKE_TIMEOUT`.
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
        // that a blocking read on the child would not. The custom line reader
        // also caps memory before a hostile child can complete an oversized
        // stdout line.
        let stdin = Arc::new(Mutex::new(stdin));
        let response_stdin = Arc::downgrade(&stdin);
        let response_server_name = server_name.clone();
        let (sender, responses) = sync_channel(MAX_PENDING_CHILD_MESSAGES);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_bounded_line(&mut reader, MAX_JSONRPC_LINE_BYTES) {
                    Ok(Some(line)) => {
                        // Server requests must be answered even while no
                        // client request is in flight. Route them before the
                        // zero-capacity response channel: blocking there until
                        // the next client call would violate MCP ping's prompt
                        // response requirement. Valid notifications need no
                        // response and likewise must not occupy the rendezvous.
                        if let Ok(message) = serde_json::from_str::<Value>(&line)
                            && message.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
                            && message.get("method").and_then(Value::as_str).is_some()
                        {
                            let Some(response) = response_to_server_request(&message) else {
                                continue;
                            };
                            let Some(stdin) = response_stdin.upgrade() else {
                                break;
                            };
                            let result = stdin
                                .lock()
                                .map_err(|_| {
                                    anyhow!("connection stdin poisoned by an earlier panic")
                                })
                                .and_then(|mut stdin| write_jsonrpc_line(&mut *stdin, &response));
                            // Never retain the temporary strong handle while a
                            // failure waits on the rendezvous channel. Drop
                            // must remain able to close child stdin promptly.
                            drop(stdin);
                            if let Err(error) = result {
                                let _ = sender.send(ChildStdoutMessage::Invalid(format!(
                                    "MCP server '{response_server_name}': failed to answer idle child request: {error:#}"
                                )));
                                break;
                            }
                            continue;
                        }
                        if sender.send(ChildStdoutMessage::Line(line)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(err) => {
                        let _ = sender.send(ChildStdoutMessage::Invalid(err.to_string()));
                        break;
                    }
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
        let capabilities = validate_initialize_result(&server_name, &initialize)?;

        connection
            .send(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }))
            .with_context(|| {
                format!("MCP server '{server_name}': failed to confirm initialization")
            })?;

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
        self.request_with_timeout(method, params, self.request_timeout)
    }

    fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let mut connection = self.connection.lock().map_err(|_| {
            anyhow!(
                "MCP server '{}': connection poisoned by an earlier panic",
                self.server_name
            )
        })?;
        connection.request(&self.server_name, method, params, timeout)
    }

    /// Drive a paginated `*/list` method to exhaustion, validating every item
    /// before requesting the next page. A malformed page therefore fails at
    /// its source rather than being silently thinned or discovered only after
    /// the rest of the catalog has been fetched.
    fn list_paginated<T, F>(&self, method: &str, field: &str, mut parse: F) -> Result<Vec<T>>
    where
        F: FnMut(&Value, usize) -> Result<T>,
    {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        let mut budget = ListBudget::new(method, self.request_timeout);
        loop {
            let params = match &cursor {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };
            let page = self.request_with_timeout(method, params, budget.remaining_timeout()?)?;
            let next_cursor = budget.observe_page(&page, field)?;
            let values = page
                .get(field)
                .and_then(Value::as_array)
                .expect("ListBudget validated the catalog field");
            for value in values {
                let index = items.len();
                items.push(parse(value, index)?);
            }
            match next_cursor {
                Some(next_cursor) => cursor = Some(next_cursor),
                None => break,
            }
        }
        Ok(items)
    }
}

impl McpManagedClient for ChildProcessMcpClient {
    fn list_tools(&self) -> Result<Vec<McpToolDescriptor>> {
        Ok(self
            .list_tools_with_input_schemas()?
            .into_iter()
            .map(|(tool, _)| tool)
            .collect())
    }

    fn list_tools_with_input_schemas(&self) -> Result<Vec<(McpToolDescriptor, Value)>> {
        if !self.supports_tools() {
            return Ok(Vec::new());
        }
        let allow_legacy_schema_omission = self.capabilities.is_none();
        self.list_paginated("tools/list", "tools", |tool, index| {
            parse_tool_entry(&self.server_name, tool, index, allow_legacy_schema_omission)
        })
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
        Ok(self
            .list_resources_with_metadata()?
            .into_iter()
            .map(|(resource, _)| resource)
            .collect())
    }

    fn list_resources_with_metadata(&self) -> Result<Vec<(McpResourceDescriptor, Value)>> {
        if !self.supports_resources() {
            return Ok(Vec::new());
        }
        self.list_paginated("resources/list", "resources", |resource, index| {
            parse_resource_entry(&self.server_name, resource, index)
        })
    }

    fn read_resource(&self, uri: &str) -> Result<Value> {
        self.request("resources/read", json!({ "uri": uri }))
    }
}

struct Connection {
    child: Child,
    stdin: Option<Arc<Mutex<ChildStdin>>>,
    responses: Receiver<ChildStdoutMessage>,
    next_id: u64,
}

impl Connection {
    fn send(&mut self, message: &Value) -> Result<()> {
        let stdin = self
            .stdin
            .as_ref()
            .context("connection stdin already closed")?;
        let mut stdin = stdin
            .lock()
            .map_err(|_| anyhow!("connection stdin poisoned by an earlier panic"))?;
        write_jsonrpc_line(&mut *stdin, message)
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
                Ok(ChildStdoutMessage::Line(line)) => line,
                Ok(ChildStdoutMessage::Invalid(error)) => {
                    bail!(
                        "MCP server '{server}': invalid child stdout while awaiting {method}: {error}"
                    );
                }
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

            // Servers occasionally emit banners or log lines on stdout. They
            // are skipped; only the matching response ends the wait. Valid
            // requests and notifications are handled by the reader before
            // they reach this rendezvous.
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                bail!(
                    "MCP server '{server}': {method} received a child message without jsonrpc \"2.0\""
                );
            }
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                bail!("MCP server '{server}': {method} failed: {error}");
            }
            return message.get("result").cloned().with_context(|| {
                format!(
                    "MCP server '{server}': {method} response contained neither result nor error"
                )
            });
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
    use std::io::Cursor;

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

    #[test]
    fn standard_tool_input_schemas_are_required_and_fail_closed_when_malformed() {
        let fallback = json!({"type": "object", "properties": {}});
        let (_, legacy_schema) =
            parse_tool_entry("legacy", &json!({"name": "legacy-tool"}), 0, true).unwrap();
        assert_eq!(legacy_schema, fallback);

        for malformed_tool in [
            json!({"name": "missing"}),
            json!({"name": "null", "inputSchema": null}),
            json!({"name": "empty", "inputSchema": {}}),
            json!({"name": "wrong-type", "inputSchema": {"type": "string"}}),
            json!({
                "name": "bad-properties",
                "inputSchema": {"type": "object", "properties": "not-an-object"}
            }),
            json!({
                "name": "bad-property",
                "inputSchema": {"type": "object", "properties": {"path": true}}
            }),
            json!({
                "name": "bad-required-shape",
                "inputSchema": {"type": "object", "required": "path"}
            }),
            json!({
                "name": "bad-required-entry",
                "inputSchema": {"type": "object", "required": [1]}
            }),
        ] {
            let error = parse_tool_entry("standard", &malformed_tool, 0, false).unwrap_err();
            assert!(
                error.to_string().contains("inputSchema"),
                "malformed schema produced an unrelated error: {error:#}"
            );
        }

        // Legacy mode tolerates only omission. Once a field is present, it is
        // standard data and must pass the same validation.
        let legacy_error = parse_tool_entry(
            "legacy",
            &json!({"name": "malformed", "inputSchema": null}),
            0,
            true,
        )
        .unwrap_err();
        assert!(legacy_error.to_string().contains("inputSchema"));
    }

    #[test]
    fn catalog_entry_parsers_preserve_valid_fields_and_reject_malformed_entries() {
        let (tool, schema) = parse_tool_entry(
            "fixture",
            &json!({
                "name": "read-file",
                "description": "Read one file",
                "inputSchema": {
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]
                }
            }),
            0,
            false,
        )
        .unwrap();
        assert_eq!(tool.tool_name, "read-file");
        assert_eq!(tool.description.as_deref(), Some("Read one file"));
        assert_eq!(schema["required"], json!(["path"]));

        let (resource, metadata) = parse_resource_entry(
            "fixture",
            &json!({
                "uri": "file:///guide.md",
                "name": "Guide",
                "description": "User guide",
                "mimeType": "text/markdown",
                "size": 42,
                "annotations": {"audience": ["assistant"], "priority": 0.8}
            }),
            0,
        )
        .unwrap();
        assert_eq!(resource.uri, "file:///guide.md");
        assert_eq!(resource.description.as_deref(), Some("User guide"));
        assert_eq!(
            metadata,
            json!({
                "name": "Guide",
                "mimeType": "text/markdown",
                "size": 42,
                "annotations": {"audience": ["assistant"], "priority": 0.8}
            })
        );

        for malformed in [
            Value::Null,
            json!({}),
            json!({"name": 7}),
            json!({"name": "tool", "description": []}),
        ] {
            assert!(
                parse_tool_entry("fixture", &malformed, 3, false).is_err(),
                "malformed tool entry passed: {malformed}"
            );
        }
        for malformed in [
            Value::Null,
            json!({}),
            json!({"uri": "file:///x"}),
            json!({"uri": 7, "name": "x"}),
            json!({"uri": "file:///x", "name": 7}),
            json!({"uri": "file:///x", "name": "x", "description": []}),
            json!({"uri": "file:///x", "name": "x", "mimeType": []}),
            json!({"uri": "file:///x", "name": "x", "size": 1.5}),
            json!({"uri": "file:///x", "name": "x", "annotations": []}),
        ] {
            assert!(
                parse_resource_entry("fixture", &malformed, 4).is_err(),
                "malformed resource entry passed: {malformed}"
            );
        }
    }

    #[test]
    fn bounded_line_reader_rejects_an_oversized_line_before_completion() {
        let mut exact = BufReader::with_capacity(3, Cursor::new(b"12345678\nnext\n"));
        assert_eq!(
            read_bounded_line(&mut exact, 8).unwrap().as_deref(),
            Some("12345678")
        );
        assert_eq!(
            read_bounded_line(&mut exact, 8).unwrap().as_deref(),
            Some("next")
        );

        // The three-byte BufReader forces the bound check to span chunks. It
        // rejects when the ninth payload byte is buffered, before consuming
        // the newline or growing the retained line beyond eight bytes.
        let mut oversized = BufReader::with_capacity(3, Cursor::new(b"123456789\n"));
        let error = read_bounded_line(&mut oversized, 8).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("8-byte limit"));
    }

    #[test]
    fn child_stdout_queue_is_rendezvous_backpressured() {
        let (sender, receiver) = sync_channel(MAX_PENDING_CHILD_MESSAGES);
        match sender.try_send(ChildStdoutMessage::Line("one".to_string())) {
            Err(std::sync::mpsc::TrySendError::Full(_)) => {}
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                panic!("child stdout receiver disconnected")
            }
            Ok(()) => panic!("child stdout queue accepted a message without a waiting consumer"),
        }
        drop(receiver);
    }

    #[test]
    fn initialize_result_requires_supported_protocol_and_server_identity() {
        let valid = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "serverInfo": {"name": "fixture", "version": "1"},
            "capabilities": {"tools": {}, "resources": {}}
        });
        let capabilities = validate_initialize_result("fixture", &valid)
            .unwrap()
            .expect("capabilities");
        assert!(capabilities.tools);
        assert!(capabilities.resources);

        for invalid in [
            json!({}),
            json!({
                "protocolVersion": "2099-01-01",
                "serverInfo": {"name": "fixture", "version": "1"}
            }),
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "serverInfo": {"name": "fixture"}
            }),
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "serverInfo": {"name": "fixture", "version": "1"},
                "capabilities": []
            }),
        ] {
            assert!(
                validate_initialize_result("fixture", &invalid).is_err(),
                "invalid initialize result passed: {invalid}"
            );
        }
    }

    #[test]
    fn catalog_budget_fails_instead_of_returning_partial_results() {
        let mut pages =
            ListBudget::with_limits("resources/list", Duration::from_secs(1), 2, 2, 256);
        assert_eq!(
            pages
                .observe_page(
                    &json!({"resources": [{"uri": "file:///a", "name": "a"}], "nextCursor": "a"}),
                    "resources"
                )
                .unwrap(),
            Some("a".to_string())
        );
        let page_error = pages
            .observe_page(
                &json!({"resources": [{"uri": "file:///b", "name": "b"}], "nextCursor": "b"}),
                "resources",
            )
            .unwrap_err();
        assert!(page_error.to_string().contains("page catalog limit"));

        let mut items = ListBudget::with_limits("tools/list", Duration::from_secs(1), 4, 1, 256);
        let item_error = items
            .observe_page(&json!({"tools": [{}, {}]}), "tools")
            .unwrap_err();
        assert!(item_error.to_string().contains("item catalog limit"));

        let mut bytes = ListBudget::with_limits("tools/list", Duration::from_secs(1), 4, 4, 8);
        let byte_error = bytes
            .observe_page(&json!({"tools": [{"name": "large"}]}), "tools")
            .unwrap_err();
        assert!(
            byte_error
                .to_string()
                .contains("byte aggregate catalog limit")
        );

        let mut malformed =
            ListBudget::with_limits("resources/list", Duration::from_secs(1), 4, 4, 256);
        assert!(
            malformed
                .observe_page(&json!({"resources": null}), "resources")
                .unwrap_err()
                .to_string()
                .contains("did not contain a 'resources' array")
        );

        let expired = ListBudget::new("resources/list", Duration::ZERO);
        assert!(
            expired
                .remaining_timeout()
                .unwrap_err()
                .to_string()
                .contains("overall")
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
    fn handshake_answers_a_same_id_child_ping_before_accepting_the_response() {
        let script = r#"
IFS= read -r line
id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
printf '{"jsonrpc":"2.0","id":%s,"method":"ping"}\n' "$id"
IFS= read -r pong
case "$pong" in
  *'"result":{}'*) ;;
  *) exit 9 ;;
esac
printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"pinging-child","version":"1"}}}\n' "$id"
while IFS= read -r _line; do :; done
"#;
        let client = ChildProcessMcpClient::spawn_with_timeouts(
            &config("/bin/sh", &["-c", script]),
            Duration::from_secs(2),
            Duration::from_secs(2),
        )
        .expect("the child ping must be answered during initialize");
        assert!(client.supports_tools());
    }

    #[cfg(unix)]
    #[test]
    fn idle_child_requests_are_answered_before_the_next_client_request() {
        let script = r#"
ready=0
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
  case "$method" in
    initialize)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"idle-request-child","version":"1"}}}\n' "$id"
      ;;
    notifications/initialized)
      printf '{"jsonrpc":"2.0","id":"idle-ping","method":"ping"}\n'
      (sleep 1; kill -KILL "$$") &
      watchdog=$!
      IFS= read -r pong || exit 9
      kill "$watchdog" 2>/dev/null || :
      wait "$watchdog" 2>/dev/null || :
      case "$pong" in *'"id":"idle-ping"'*) ;; *) exit 10 ;; esac
      case "$pong" in *'"result":{}'*) ;; *) exit 11 ;; esac

      printf '{"jsonrpc":"2.0","id":"idle-unsupported","method":"client/unsupported"}\n'
      (sleep 1; kill -KILL "$$") &
      watchdog=$!
      IFS= read -r unsupported || exit 12
      kill "$watchdog" 2>/dev/null || :
      wait "$watchdog" 2>/dev/null || :
      case "$unsupported" in *'"id":"idle-unsupported"'*) ;; *) exit 13 ;; esac
      case "$unsupported" in *'"code":-32601'*) ;; *) exit 14 ;; esac
      ready=1
      ;;
    tools/list)
      [ "$ready" = 1 ] || exit 15
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[]}}\n' "$id"
      ;;
  esac
done
printf 'stdin closed\n' > "$CODEWHALE_MCP_TEST_MARKER"
"#;
        let marker = std::env::temp_dir().join(format!(
            "codewhale-mcp-idle-drop-{}-{}.marker",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after epoch")
                .as_nanos()
        ));
        let mut child_config = config("/bin/sh", &["-c", script]);
        child_config.env.insert(
            "CODEWHALE_MCP_TEST_MARKER".to_string(),
            marker.to_string_lossy().into_owned(),
        );
        let client = ChildProcessMcpClient::spawn_with_timeouts(
            &child_config,
            Duration::from_secs(2),
            Duration::from_secs(2),
        )
        .expect("handshake");

        // The child terminates if either server-initiated request remains
        // unanswered for one second. Keep the client genuinely idle beyond
        // that deadline before making the next ordinary client request.
        thread::sleep(Duration::from_millis(1_500));
        assert!(client.list_tools().unwrap().is_empty());

        // The reader owns only a weak stdin handle. Dropping the client must
        // therefore still deliver EOF to the child and let it exit cleanly;
        // a strong reader-thread handle would force Drop's kill fallback.
        drop(client);
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "stdin closed\n");
        std::fs::remove_file(marker).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn handshake_rejects_a_child_response_without_jsonrpc_2_0() {
        let script = r#"
IFS= read -r line
id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
printf '{"id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"invalid-child","version":"1"}}}\n' "$id"
while IFS= read -r _line; do :; done
"#;
        let error = ChildProcessMcpClient::spawn_with_timeouts(
            &config("/bin/sh", &["-c", script]),
            Duration::from_secs(2),
            Duration::from_secs(2),
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("without jsonrpc \"2.0\""),
            "unexpected error: {error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn malformed_child_catalog_entries_fail_their_page_immediately() {
        let script = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
  [ -n "$id" ] || continue
  case "$method" in
    initialize)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{},"resources":{}},"serverInfo":{"name":"malformed-catalog","version":"1"}}}\n' "$id"
      ;;
    tools/list)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"bad-schema","inputSchema":null}],"nextCursor":"more"}}\n' "$id"
      ;;
    resources/list)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"resources":[{"uri":"file:///missing-name"}],"nextCursor":"more"}}\n' "$id"
      ;;
  esac
done
"#;
        let client = ChildProcessMcpClient::spawn_with_timeouts(
            &config("/bin/sh", &["-c", script]),
            Duration::from_secs(2),
            Duration::from_millis(250),
        )
        .expect("handshake");

        let tools_error = client.list_tools().unwrap_err();
        assert!(
            tools_error.to_string().contains("tools[0].inputSchema"),
            "the malformed first page must fail before following its cursor: {tools_error:#}"
        );
        let resources_error = client.list_resources().unwrap_err();
        assert!(
            resources_error.to_string().contains("resources[0].name"),
            "the malformed first page must fail before following its cursor: {resources_error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_real_child_answers_tools_and_resources() {
        let script = crate::test_support::write_fake_mcp_server("stdio_client_roundtrip");
        let client = ChildProcessMcpClient::spawn(&config(
            "/bin/sh",
            &[script.path().to_str().expect("utf-8 script path")],
        ))
        .expect("fake MCP server should complete the handshake");

        let tools = client.list_tools_with_input_schemas().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].0.tool_name, "add");
        assert_eq!(tools[0].1["required"], json!(["a", "b"]));

        let result = client.call_tool("add", json!({"a": 2, "b": 3})).unwrap();
        assert_eq!(
            result["content"][0]["text"], "5",
            "the answer must come from the child process: {result}"
        );

        let resources = client.list_resources_with_metadata().unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].0.uri, "file:///fake/readme.txt");
        assert_eq!(resources[0].1["name"], "Fake readme");
        assert_eq!(resources[0].1["mimeType"], "text/plain");
        assert_eq!(resources[0].1["size"], 16);
        assert_eq!(
            resources[0].1["annotations"]["audience"],
            json!(["assistant"])
        );

        let resource = client.read_resource("file:///fake/readme.txt").unwrap();
        assert_eq!(resource["contents"][0]["text"], "spawned-resource");

        // The stub's fabricated tools must be gone.
        assert!(client.call_tool("health", json!({})).is_err());
    }
}
