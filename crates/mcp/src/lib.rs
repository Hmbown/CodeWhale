use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod stdio_client;
// Unix-gated as well as test-gated: every helper in here builds and spawns a
// POSIX-sh script, so the tests that use it are `#[cfg(unix)]` and on Windows
// the whole module compiles to dead code, which `-D warnings` rejects.
#[cfg(all(test, unix))]
mod test_support;

pub use stdio_client::ChildProcessMcpClient;

/// Configuration for a single MCP server process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Unique server identifier used for tool name qualification.
    pub name: String,
    /// Path or name of the server executable.
    pub command: String,
    /// Command-line arguments passed to the server process.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables set for the server process.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Whether this server should be started. Disabled servers are skipped.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Filter controlling which tools from an MCP server are exposed.
///
/// When `allow` is empty, all tools are permitted (unless denied).
/// `deny` takes precedence over `allow`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolFilter {
    /// Tool names to expose. Empty means expose all.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Tool names to exclude. Takes precedence over `allow`.
    #[serde(default)]
    pub deny: Vec<String>,
}

/// A complete MCP server definition including config and tool filter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerDefinition {
    /// Server process configuration.
    pub config: McpServerConfig,
    /// Tool filter controlling which tools are exposed.
    #[serde(default)]
    pub filter: ToolFilter,
}

/// Status of an individual MCP server during startup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpStartupStatus {
    /// Server process is starting.
    Starting,
    /// Server is ready to accept tool calls.
    Ready,
    /// Server failed to start.
    Failed { error: String },
    /// Server startup was cancelled (e.g., disabled in config).
    Cancelled,
}

/// Status update for a single MCP server during startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStartupUpdateEvent {
    /// Name of the server this update pertains to.
    pub server_name: String,
    /// Current startup status.
    pub status: McpStartupStatus,
}

/// Record of an MCP server that failed to start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStartupFailure {
    /// Name of the server that failed.
    pub server_name: String,
    /// Error message describing the failure.
    pub error: String,
}

/// Summary emitted after all MCP servers have completed startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStartupCompleteEvent {
    /// Names of servers that started successfully.
    pub ready: Vec<String>,
    /// Servers that failed with error details.
    pub failed: Vec<McpStartupFailure>,
    /// Names of servers that were skipped (disabled).
    pub cancelled: Vec<String>,
}

/// Describes a single tool provided by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDescriptor {
    /// Name of the server providing this tool.
    pub server_name: String,
    /// Original tool name as reported by the server.
    pub tool_name: String,
    /// Fully qualified name (e.g., `mcp__server__tool`).
    pub qualified_name: String,
    /// Human-readable description of what the tool does.
    pub description: Option<String>,
}

/// Describes a resource provided by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceDescriptor {
    /// Name of the server providing this resource.
    pub server_name: String,
    /// URI identifying the resource.
    pub uri: String,
    /// Human-readable description.
    pub description: Option<String>,
}

/// Trait abstracting an MCP client connection.
///
/// Implementations handle communication with a single MCP server process.
pub trait McpManagedClient: Send + Sync {
    /// List all tools provided by this server.
    fn list_tools(&self) -> Result<Vec<McpToolDescriptor>>;
    /// List tools together with their MCP argument schemas.
    ///
    /// The default keeps existing external client implementations source
    /// compatible: clients that predate schema forwarding still advertise an
    /// empty object schema, while transports that receive `inputSchema` can
    /// override this method and preserve it.
    fn list_tools_with_input_schemas(&self) -> Result<Vec<(McpToolDescriptor, Value)>> {
        Ok(self
            .list_tools()?
            .into_iter()
            .map(|tool| (tool, default_tool_input_schema()))
            .collect())
    }
    /// Invoke a tool by name with the given arguments.
    fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<Value>;
    /// List all resources provided by this server.
    fn list_resources(&self) -> Result<Vec<McpResourceDescriptor>>;
    /// List resources together with their standard MCP metadata.
    ///
    /// `McpResourceDescriptor` predates the required MCP `name` field and the
    /// optional `mimeType` field. Keeping those fields in additive metadata
    /// preserves source compatibility for external trait implementations,
    /// while transports that receive the standard fields can override this
    /// method and forward them without loss.
    fn list_resources_with_metadata(&self) -> Result<Vec<(McpResourceDescriptor, Value)>> {
        Ok(self
            .list_resources()?
            .into_iter()
            .map(|resource| {
                let metadata = default_resource_metadata(&resource);
                (resource, metadata)
            })
            .collect())
    }
    /// Read a resource by URI.
    fn read_resource(&self, uri: &str) -> Result<Value>;
}

/// A simple in-memory MCP client for tests and embedding callers.
///
/// This is **not** wired into `codewhale mcp-server`: that path spawns
/// [`ChildProcessMcpClient`] and reports a typed error when the configured
/// command cannot be run. Serving canned values there made a broken
/// integration look identical to a working one (#4727).
#[derive(Debug, Default)]
pub struct InMemoryMcpClient {
    tools: HashMap<String, Value>,
    resources: HashMap<String, Value>,
}

impl InMemoryMcpClient {
    /// Register a tool with a fixed response value.
    pub fn with_tool(mut self, name: &str, sample_result: Value) -> Self {
        self.tools.insert(name.to_string(), sample_result);
        self
    }

    /// Register a resource with a fixed data value.
    pub fn with_resource(mut self, uri: &str, data: Value) -> Self {
        self.resources.insert(uri.to_string(), data);
        self
    }
}

impl McpManagedClient for InMemoryMcpClient {
    fn list_tools(&self) -> Result<Vec<McpToolDescriptor>> {
        Ok(self
            .tools
            .keys()
            .map(|name| McpToolDescriptor {
                server_name: "in-memory".to_string(),
                tool_name: name.clone(),
                qualified_name: name.clone(),
                description: None,
            })
            .collect())
    }

    fn call_tool(&self, tool_name: &str, _arguments: Value) -> Result<Value> {
        self.tools
            .get(tool_name)
            .cloned()
            .with_context(|| format!("tool '{tool_name}' not found"))
    }

    fn list_resources(&self) -> Result<Vec<McpResourceDescriptor>> {
        Ok(self
            .resources
            .keys()
            .map(|uri| McpResourceDescriptor {
                server_name: "in-memory".to_string(),
                uri: uri.clone(),
                description: None,
            })
            .collect())
    }

    fn read_resource(&self, uri: &str) -> Result<Value> {
        self.resources
            .get(uri)
            .cloned()
            .with_context(|| format!("resource '{uri}' not found"))
    }
}

/// Manages multiple MCP server connections and their tool/resource registrations.
#[derive(Default)]
pub struct McpManager {
    configs: HashMap<String, (McpServerConfig, ToolFilter)>,
    clients: HashMap<String, Box<dyn McpManagedClient>>,
}

impl McpManager {
    /// Register an MCP server with its config, tool filter, and client implementation.
    ///
    /// Fails when the server's name collides with an already-registered server
    /// after `sanitize_component` folding. Qualified tool names are built
    /// from the sanitized name, so `my-server`, `my_server`, and `My.Server`
    /// all produce `mcp__my_server__*`: registering two of them would let
    /// either server answer a qualified name meant for the other. Re-registering
    /// the same name replaces it, which is how restart works.
    pub fn register_server(
        &mut self,
        config: McpServerConfig,
        filter: ToolFilter,
        client: Box<dyn McpManagedClient>,
    ) -> Result<()> {
        if let Some(existing) = self.colliding_server_name(&config.name) {
            bail!(
                "MCP server '{}' collides with already-registered server '{existing}': \
                 both qualify tools as 'mcp__{}__*'",
                config.name,
                sanitize_component(&config.name)
            );
        }
        self.clients.insert(config.name.clone(), client);
        self.configs.insert(config.name.clone(), (config, filter));
        Ok(())
    }

    /// Returns a registered server whose sanitized name matches `name`'s but
    /// which is not `name` itself.
    fn colliding_server_name(&self, name: &str) -> Option<&str> {
        let sanitized = sanitize_component(name);
        self.configs
            .keys()
            .find(|existing| existing.as_str() != name && sanitize_component(existing) == sanitized)
            .map(String::as_str)
    }

    /// Resolve a sanitized tool segment from a qualified name back to the
    /// server's original tool name.
    ///
    /// `qualify_tool_name` folds `-`, `.`, and case into `_`, so the segment
    /// carried by `mcp__server__segment` is not necessarily the name the
    /// server expects. Exactly one advertised, allowed match resolves to its
    /// original name. Multiple matches fail closed; no match preserves the
    /// direct-call behavior for clients whose catalog is not exhaustive.
    fn resolve_original_tool_name(
        &self,
        server_name: &str,
        tool_segment: &str,
        qualified_tool_name: &str,
    ) -> Result<String> {
        let Some(client) = self.clients.get(server_name) else {
            return Ok(tool_segment.to_string());
        };
        let Ok(tools) = client.list_tools() else {
            return Ok(tool_segment.to_string());
        };
        let filter = self.configs.get(server_name).map(|(_, filter)| filter);
        let mut matches = tools.iter().filter(|tool| {
            filter.is_none_or(|filter| allowed_by_filter(&tool.tool_name, filter))
                && qualify_tool_name(server_name, &tool.tool_name) == qualified_tool_name
        });
        match (matches.next(), matches.next()) {
            (Some(tool), None) => Ok(tool.tool_name.clone()),
            (None, _) => Ok(tool_segment.to_string()),
            (Some(_), Some(_)) => bail!(
                "qualified MCP tool name '{qualified_tool_name}' is ambiguous within server '{server_name}'"
            ),
        }
    }

    /// Start all registered servers, emitting status updates via the callback.
    ///
    /// Returns a summary of which servers are ready, failed, or cancelled.
    pub fn start_all<F>(&self, mut emit: F) -> McpStartupCompleteEvent
    where
        F: FnMut(McpStartupUpdateEvent),
    {
        let mut ready = Vec::new();
        let mut failed = Vec::new();
        let mut cancelled = Vec::new();
        for (server_name, (cfg, _)) in &self.configs {
            if !cfg.enabled {
                emit(McpStartupUpdateEvent {
                    server_name: server_name.clone(),
                    status: McpStartupStatus::Cancelled,
                });
                cancelled.push(server_name.clone());
                continue;
            }
            emit(McpStartupUpdateEvent {
                server_name: server_name.clone(),
                status: McpStartupStatus::Starting,
            });
            if self.clients.contains_key(server_name) {
                emit(McpStartupUpdateEvent {
                    server_name: server_name.clone(),
                    status: McpStartupStatus::Ready,
                });
                ready.push(server_name.clone());
            } else {
                let error = "client not registered".to_string();
                emit(McpStartupUpdateEvent {
                    server_name: server_name.clone(),
                    status: McpStartupStatus::Failed {
                        error: error.clone(),
                    },
                });
                failed.push(McpStartupFailure {
                    server_name: server_name.clone(),
                    error,
                });
            }
        }
        McpStartupCompleteEvent {
            ready,
            failed,
            cancelled,
        }
    }

    /// Stop a running server by removing its client.
    pub fn stop_server(&mut self, server_name: &str) -> Result<()> {
        self.clients
            .remove(server_name)
            .with_context(|| format!("server '{server_name}' is not running"))?;
        Ok(())
    }

    /// Remove a server entirely (config and client).
    pub fn unregister_server(&mut self, server_name: &str) -> Result<()> {
        let had_config = self.configs.remove(server_name).is_some();
        self.clients.remove(server_name);
        if !had_config {
            bail!("server '{server_name}' is not registered");
        }
        Ok(())
    }

    /// List all tools from all running servers, applying tool filters.
    pub fn list_tools(&self) -> Result<Vec<McpToolDescriptor>> {
        Ok(self
            .list_tools_with_input_schemas()?
            .into_iter()
            .map(|(tool, _)| tool)
            .collect())
    }

    fn list_tools_with_input_schemas(&self) -> Result<Vec<(McpToolDescriptor, Value)>> {
        let mut out = Vec::new();
        let mut qualified_origins: HashMap<String, (String, String)> = HashMap::new();
        for (server_name, (_, filter)) in &self.configs {
            let Some(client) = self.clients.get(server_name) else {
                continue;
            };
            let tools = client.list_tools_with_input_schemas()?;
            for (tool, input_schema) in tools {
                if !allowed_by_filter(&tool.tool_name, filter) {
                    continue;
                }
                let qualified_name = qualify_tool_name(server_name, &tool.tool_name);
                if let Some((prior_server, prior_tool)) = qualified_origins.get(&qualified_name)
                    && (prior_server != server_name || prior_tool != &tool.tool_name)
                {
                    let mut origins = [
                        format!("{prior_server}:{prior_tool}"),
                        format!("{server_name}:{}", tool.tool_name),
                    ];
                    origins.sort();
                    bail!(
                        "qualified MCP tool name '{qualified_name}' is ambiguous between {}",
                        origins.join(" and ")
                    );
                }
                qualified_origins.insert(
                    qualified_name.clone(),
                    (server_name.clone(), tool.tool_name.clone()),
                );
                out.push((
                    McpToolDescriptor {
                        server_name: server_name.clone(),
                        tool_name: tool.tool_name,
                        qualified_name,
                        description: tool.description,
                    },
                    input_schema,
                ));
            }
        }
        Ok(out)
    }

    /// Call a tool on a specific server by name.
    ///
    /// The server's [`ToolFilter`] is enforced on invocation, not just at
    /// listing time: a denied (or not-allowed) tool cannot be executed by
    /// addressing the server directly, whether by bare or qualified name.
    pub fn call_tool(&self, server_name: &str, tool_name: &str, arguments: Value) -> Result<Value> {
        let client = self
            .clients
            .get(server_name)
            .with_context(|| format!("MCP server '{server_name}' not available"))?;
        if let Some((_, filter)) = self.configs.get(server_name)
            && !allowed_by_filter(tool_name, filter)
        {
            bail!("tool '{tool_name}' on MCP server '{server_name}' is blocked by the tool filter");
        }
        client.call_tool(tool_name, arguments)
    }

    /// Call a tool using its fully qualified name (e.g., `mcp__server__tool`).
    pub fn call_qualified_tool(
        &self,
        qualified_tool_name: &str,
        arguments: Value,
    ) -> Result<Value> {
        let parsed = parse_qualified_tool_name(qualified_tool_name)
            .with_context(|| format!("invalid qualified MCP tool name: {qualified_tool_name}"));

        // An exact registration is the answer. Whatever the tool returns —
        // including an error — is returned as-is: falling through to the scan
        // below on a *call* failure would re-execute the same tool, and for a
        // file write, a commit, or a paid API call that second invocation is a
        // second real side effect. Only a failed *lookup* falls through.
        //
        // The parsed tool segment is the *sanitized* name (qualify_tool_name
        // folds `-`, `.`, and case into `_`), so resolve it back to the
        // server's original tool name before dispatching — otherwise tools
        // like `my-tool` are un-callable through their advertised qualified
        // name `mcp__server__my_tool`.
        if let Ok((server_name, tool_name)) = &parsed
            && self.clients.contains_key(server_name)
        {
            let resolved =
                self.resolve_original_tool_name(server_name, tool_name, qualified_tool_name)?;
            return self.call_tool(server_name, &resolved, arguments);
        }

        // No exact registration: resolve by scanning qualified names. Collect
        // every match rather than returning the first, because `configs` is a
        // HashMap — returning early would make the choice depend on iteration
        // order when two servers' names collide after sanitizing.
        let mut matches: Vec<(&String, String)> = Vec::new();
        for (server_name, (_, filter)) in &self.configs {
            let Some(client) = self.clients.get(server_name) else {
                continue;
            };
            for tool in client.list_tools()? {
                if !allowed_by_filter(&tool.tool_name, filter) {
                    continue;
                }
                if qualify_tool_name(server_name, &tool.tool_name) == qualified_tool_name {
                    matches.push((server_name, tool.tool_name));
                }
            }
        }
        match matches.len() {
            0 => {}
            1 => {
                let (server_name, tool_name) = &matches[0];
                let client = self
                    .clients
                    .get(*server_name)
                    .with_context(|| format!("MCP server '{server_name}' not available"))?;
                return client.call_tool(tool_name, arguments);
            }
            _ => {
                matches.sort();
                let servers: Vec<&str> = matches
                    .iter()
                    .map(|(server_name, _)| server_name.as_str())
                    .collect();
                bail!(
                    "qualified MCP tool name '{qualified_tool_name}' is ambiguous across servers: \
                     {}",
                    servers.join(", ")
                );
            }
        }

        let (server_name, tool_name) = parsed?;
        self.call_tool(&server_name, &tool_name, arguments)
    }

    /// List all resources from all running servers.
    pub fn list_resources(&self) -> Result<Vec<McpResourceDescriptor>> {
        Ok(self
            .list_resources_with_metadata()?
            .into_iter()
            .map(|(resource, _)| resource)
            .collect())
    }

    fn list_resources_with_metadata(&self) -> Result<Vec<(McpResourceDescriptor, Value)>> {
        let mut out = Vec::new();
        for server_name in self.configs.keys() {
            let Some(client) = self.clients.get(server_name) else {
                continue;
            };
            for (mut resource, metadata) in client.list_resources_with_metadata()? {
                resource.server_name = server_name.clone();
                out.push((resource, metadata));
            }
        }
        Ok(out)
    }

    /// Read a resource from a specific server.
    pub fn read_resource(&self, server_name: &str, uri: &str) -> Result<Value> {
        let client = self
            .clients
            .get(server_name)
            .with_context(|| format!("MCP server '{server_name}' not available"))?;
        client.read_resource(uri)
    }

    /// Resolve a standard URI-only resource read to exactly one child server.
    ///
    /// Older Codewhale clients supplied a non-standard `server` parameter (or
    /// encoded it as the authority in an `mcp://server/...` URI). Standard MCP
    /// clients send only the URI, so discover its owner from resources/list.
    /// Never pick the first HashMap entry when more than one server advertises
    /// the same URI.
    fn read_resource_by_uri(&self, uri: &str) -> Result<Value> {
        let mut matches = Vec::new();
        for server_name in self.configs.keys() {
            let Some(client) = self.clients.get(server_name) else {
                continue;
            };
            if client
                .list_resources()?
                .iter()
                .any(|resource| resource.uri == uri)
            {
                matches.push(server_name.clone());
            }
        }

        match matches.len() {
            1 => self.read_resource(&matches[0], uri),
            0 => {
                // Preserve the pre-standard URI convention for clients whose
                // server does not implement resources/list.
                if let Some(server_name) = parse_server_from_uri(uri)
                    && self.clients.contains_key(&server_name)
                {
                    return self.read_resource(&server_name, uri);
                }
                bail!("resource URI '{uri}' was not advertised by any running MCP server")
            }
            _ => {
                matches.sort();
                bail!(
                    "resource URI '{uri}' is ambiguous across MCP servers: {}; pass the legacy \
                     server parameter to disambiguate",
                    matches.join(", ")
                )
            }
        }
    }

    /// Generate sandbox state update notices for all registered servers.
    pub fn update_sandbox_state(&self, sandbox_mode: &str, cwd: &str) -> Result<Vec<Value>> {
        let mut notices = Vec::new();
        for server_name in self.configs.keys() {
            notices.push(json!({
                "server_name": server_name,
                "method": "codex/sandbox-state/update",
                "params": {
                    "sandbox_mode": sandbox_mode,
                    "cwd": cwd
                }
            }));
        }
        Ok(notices)
    }
}

fn default_true() -> bool {
    true
}

fn default_tool_input_schema() -> Value {
    json!({"type": "object", "properties": {}})
}

fn default_resource_metadata(resource: &McpResourceDescriptor) -> Value {
    // The URI is a stable, non-empty fallback name for clients implementing
    // the older descriptor-only trait surface.
    json!({"name": resource.uri})
}

fn allowed_by_filter(name: &str, filter: &ToolFilter) -> bool {
    if filter.deny.iter().any(|pattern| pattern == name) {
        return false;
    }
    if filter.allow.is_empty() {
        return true;
    }
    filter.allow.iter().any(|pattern| pattern == name)
}

fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn qualify_tool_name(server: &str, tool: &str) -> String {
    let server = sanitize_component(server);
    let tool = sanitize_component(tool);
    let mut name = format!("mcp__{server}__{tool}");
    if name.len() > 64 {
        let mut hasher = DefaultHasher::new();
        name.hash(&mut hasher);
        let hash = format!("{:x}", hasher.finish());
        let suffix = format!("_{}", &hash[..12]);
        let component_budget = 64 - "mcp__".len() - "__".len() - suffix.len();
        let mut server_len = server.len().min(component_budget / 2);
        let mut tool_len = tool.len().min(component_budget - server_len);
        let remaining = component_budget - server_len - tool_len;
        if remaining > 0 {
            let server_extra = (server.len() - server_len).min(remaining);
            server_len += server_extra;
            tool_len += (tool.len() - tool_len).min(remaining - server_extra);
        }
        name = format!(
            "mcp__{}__{}{}",
            &server[..server_len],
            &tool[..tool_len],
            suffix
        );
    }
    name
}

fn parse_qualified_tool_name(value: &str) -> Result<(String, String)> {
    let Some(stripped) = value.strip_prefix("mcp__") else {
        bail!("missing mcp__ prefix");
    };
    let mut split = stripped.splitn(2, "__");
    let server = split
        .next()
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .context("missing server segment")?;
    let tool = split
        .next()
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .context("missing tool segment")?;
    Ok((server, tool))
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    #[serde(default)]
    id: JsonRpcRequestId,
    method: String,
    #[serde(default)]
    params: Value,
}

/// JSON-RPC defines a notification by an *absent* id. An explicit `null` id is
/// discouraged but still present and must receive a response carrying null;
/// `Option<Value>` cannot preserve that distinction during deserialization.
#[derive(Debug, Clone, Default)]
enum JsonRpcRequestId {
    #[default]
    Missing,
    Present(Value),
}

impl<'de> Deserialize<'de> for JsonRpcRequestId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        if !(value.is_null() || value.is_string() || value.is_number()) {
            return Err(serde::de::Error::custom(
                "JSON-RPC id must be a string, number, or null",
            ));
        }
        Ok(Self::Present(value))
    }
}

impl JsonRpcRequestId {
    fn should_respond(&self) -> bool {
        matches!(self, Self::Present(_))
    }

    fn response_id(&self) -> Option<Value> {
        match self {
            Self::Missing => None,
            Self::Present(id) => Some(id.clone()),
        }
    }
}

#[derive(Debug)]
struct JsonRpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ToolsListParams {
    #[serde(default)]
    server: Option<String>,
}

#[derive(Debug, Deserialize)]
struct McpImplementationInfo {
    name: String,
    version: String,
}

#[derive(Debug, Deserialize)]
struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    protocol_version: String,
    #[serde(rename = "clientInfo")]
    client_info: McpImplementationInfo,
    capabilities: serde_json::Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct ToolsCallParams {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    server: Option<String>,
    #[serde(default = "default_tool_arguments")]
    arguments: Value,
}

fn default_tool_arguments() -> Value {
    json!({})
}

#[derive(Debug, Deserialize)]
struct ResourcesListParams {
    #[serde(default)]
    server: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResourcesReadParams {
    #[serde(default)]
    server: Option<String>,
    uri: String,
}

#[derive(Debug, Deserialize)]
struct ServerRegisterParams {
    server: McpServerConfig,
    #[serde(default)]
    filter: ToolFilter,
    #[serde(default = "default_true")]
    start: bool,
}

#[derive(Debug, Deserialize)]
struct ServerNameParams {
    name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum McpSessionPhase {
    Uninitialized,
    InitializeResponded,
    Ready,
}

struct StdioMcpState {
    manager: McpManager,
    definitions: HashMap<String, McpServerDefinition>,
    running: HashMap<String, bool>,
    /// Why a defined server is not running, surfaced in every lifecycle
    /// snapshot so a failed spawn cannot be mistaken for a healthy server.
    errors: HashMap<String, String>,
    lifecycle_state: String,
    session_phase: McpSessionPhase,
}

impl StdioMcpState {
    /// Spawn `definition`'s configured command and register the resulting
    /// connection, recording the failure reason when the server cannot be
    /// brought up.
    ///
    /// This is the only way a server enters `manager`, and it has no stub
    /// branch: there is no configuration under which a registered server
    /// answers from anything but its own process.
    fn start_definition(&mut self, definition: &McpServerDefinition) -> Result<()> {
        let name = definition.config.name.clone();
        let outcome = ChildProcessMcpClient::spawn(&definition.config).and_then(|client| {
            self.manager.register_server(
                definition.config.clone(),
                definition.filter.clone(),
                Box::new(client),
            )
        });
        match outcome {
            Ok(()) => {
                self.errors.remove(&name);
                self.running.insert(name, true);
                Ok(())
            }
            Err(err) => {
                let message = format!("{err:#}");
                self.errors.insert(name.clone(), message.clone());
                self.running.insert(name, false);
                Err(err)
            }
        }
    }
}

/// Run an MCP stdio server that reads JSON-RPC requests from stdin and writes responses to stdout.
///
/// Returns the final server definitions after the session ends (useful for persisting
/// runtime changes like server registrations).
pub fn run_stdio_server(
    initial_definitions: Vec<McpServerDefinition>,
) -> Result<Vec<McpServerDefinition>> {
    use std::io::{self, Write};

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();
    let mut state = build_stdio_state(initial_definitions);
    let mut input = stdin.lock();

    loop {
        let line =
            match stdio_client::read_bounded_line(&mut input, stdio_client::MAX_JSONRPC_LINE_BYTES)
            {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(err) => {
                    let response = jsonrpc_error(
                        None,
                        JsonRpcError::parse_error(format!("invalid JSON-RPC frame: {err}")),
                    );
                    writeln!(stdout, "{response}")?;
                    stdout.flush()?;
                    bail!("failed to read bounded stdio JSON-RPC frame: {err}");
                }
            };
        if line.trim().is_empty() {
            continue;
        }

        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(err) => {
                let msg = jsonrpc_error(
                    None,
                    JsonRpcError::parse_error(format!("invalid json: {err}")),
                );
                writeln!(stdout, "{msg}")?;
                stdout.flush()?;
                continue;
            }
        };
        let request: JsonRpcRequest = match serde_json::from_value(value) {
            Ok(request) => request,
            Err(err) => {
                let response = jsonrpc_error(
                    None,
                    JsonRpcError::invalid_request(format!("invalid JSON-RPC request: {err}")),
                );
                writeln!(stdout, "{response}")?;
                stdout.flush()?;
                continue;
            }
        };
        let should_respond = request.id.should_respond();
        let response_id = request.id.response_id();

        if request.jsonrpc != "2.0" {
            let response = jsonrpc_error(
                response_id,
                JsonRpcError::invalid_request("jsonrpc version must be exactly 2.0"),
            );
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
            continue;
        }

        if !should_respond {
            // `initialize` requires a response carrying the negotiated
            // protocol version. Treating an initialize notification as a real
            // handshake would advance state even though the client could not
            // observe that negotiation.
            if request.method == "initialize" {
                continue;
            }
            match dispatch_stdio_request(&mut state, &request.method, request.params) {
                Ok((_, should_exit)) if should_exit => break,
                Ok(_) | Err(_) => {}
            }
            continue;
        }

        let response = match dispatch_stdio_request(&mut state, &request.method, request.params) {
            Ok((result, should_exit)) => {
                let payload = jsonrpc_result(response_id, result);
                writeln!(stdout, "{payload}")?;
                stdout.flush()?;
                if should_exit {
                    break;
                }
                continue;
            }
            Err(err) => jsonrpc_error(response_id, err),
        };

        writeln!(stdout, "{response}")?;
        stdout.flush()?;
    }

    state.lifecycle_state = "stopped".to_string();
    let _ = writeln!(stderr, "codewhale mcp-server: stdio server exited");
    let mut definitions: Vec<McpServerDefinition> = state.definitions.into_values().collect();
    definitions.sort_by(|a, b| a.config.name.cmp(&b.config.name));
    Ok(definitions)
}

fn build_stdio_state(initial_definitions: Vec<McpServerDefinition>) -> StdioMcpState {
    let mut state = StdioMcpState {
        manager: McpManager::default(),
        definitions: HashMap::new(),
        running: HashMap::new(),
        errors: HashMap::new(),
        lifecycle_state: "running".to_string(),
        session_phase: McpSessionPhase::Uninitialized,
    };

    for definition in initial_definitions {
        let name = definition.config.name.clone();
        state.definitions.insert(name.clone(), definition.clone());
        if !definition.config.enabled {
            state.running.insert(name, false);
            continue;
        }
        // A server that cannot be spawned stays stopped and says so on stderr.
        // stdout is the JSON-RPC channel, so the warning goes to stderr where
        // it will not corrupt the protocol stream but is still visible to the
        // operator; `lifecycle` carries the same text for programmatic clients.
        if let Err(err) = state.start_definition(&definition) {
            tracing::warn!("MCP server '{name}' is not available: {err:#}");
            eprintln!("codewhale mcp-server: server '{name}' is not available: {err:#}");
        }
    }

    state
}

fn default_rpc_methods() -> Vec<&'static str> {
    vec![
        "initialize",
        "notifications/initialized",
        "ping",
        "healthz",
        "capabilities",
        "tools/list",
        "tools/call",
        "resources/list",
        "resources/read",
        "server/list",
        "server/register",
        "server/start",
        "server/stop",
        "server/unregister",
        "shutdown",
    ]
}

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const MCP_SERVER_NAME: &str = "codewhale-mcp-server";

fn initialize_response(state: &StdioMcpState) -> Value {
    json!({
        // Standard MCP initialize result. Keep the management metadata below
        // as additive compatibility fields for existing Codewhale clients.
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {
            "tools": {},
            "resources": {}
        },
        "serverInfo": {
            "name": MCP_SERVER_NAME,
            "version": env!("CARGO_PKG_VERSION")
        },
        "server": MCP_SERVER_NAME,
        "transport": "stdio",
        "methods": default_rpc_methods(),
        "lifecycle": lifecycle_snapshot(state)
    })
}

fn valid_mcp_annotations(value: &Value) -> bool {
    let Some(annotations) = value.as_object() else {
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

fn valid_optional_annotations(fields: &serde_json::Map<String, Value>) -> bool {
    fields.get("annotations").is_none_or(valid_mcp_annotations)
}

fn valid_resource_content(value: &Value) -> bool {
    let Some(content) = value.as_object() else {
        return false;
    };
    let has_uri = content.get("uri").and_then(Value::as_str).is_some();
    let has_payload = content.get("text").and_then(Value::as_str).is_some()
        || content.get("blob").and_then(Value::as_str).is_some();
    let valid_mime_type = content.get("mimeType").is_none_or(Value::is_string);
    has_uri && has_payload && valid_mime_type
}

fn valid_tool_content(value: &Value) -> bool {
    let Some(content) = value.as_object() else {
        return false;
    };
    if !valid_optional_annotations(content) {
        return false;
    }
    match content.get("type").and_then(Value::as_str) {
        Some("text") => content.get("text").is_some_and(Value::is_string),
        Some("image") => {
            content.get("data").is_some_and(Value::is_string)
                && content.get("mimeType").is_some_and(Value::is_string)
        }
        Some("resource") => content.get("resource").is_some_and(valid_resource_content),
        _ => false,
    }
}

fn valid_call_tool_result(fields: &serde_json::Map<String, Value>) -> bool {
    fields
        .get("content")
        .and_then(Value::as_array)
        .is_some_and(|content| content.iter().all(valid_tool_content))
        && fields.get("isError").is_none_or(Value::is_boolean)
        && fields.get("_meta").is_none_or(Value::is_object)
}

fn looks_like_call_tool_result(fields: &serde_json::Map<String, Value>) -> bool {
    fields.contains_key("content") || fields.contains_key("isError") || fields.contains_key("_meta")
}

fn legacy_value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        value => value.to_string(),
    }
}

fn stdio_tool_descriptor((tool, input_schema): (McpToolDescriptor, Value)) -> Value {
    let McpToolDescriptor {
        server_name,
        tool_name,
        qualified_name,
        description,
    } = tool;
    let mut value = json!({
        // Standard MCP fields. The qualified name is the only collision-safe
        // public name once several child servers are aggregated.
        "name": qualified_name.clone(),
        "inputSchema": input_schema,
        // Retain the pre-0.9.11 management fields for compatibility.
        "server_name": server_name,
        "tool_name": tool_name,
        "qualified_name": qualified_name,
    });
    if let Some(description) = description {
        value["description"] = Value::String(description);
    }
    value
}

fn stdio_tool_call_result(result: Value) -> Result<Value> {
    let legacy_result = result.clone();
    match result {
        Value::Object(mut fields) if looks_like_call_tool_result(&fields) => {
            if !valid_call_tool_result(&fields) {
                bail!("child returned a malformed MCP CallToolResult");
            }
            // The child already returned a standard MCP CallToolResult. Expose
            // it directly, while retaining the old nested result for clients
            // that used the proxy before its MCP envelope was corrected.
            fields.insert("result".to_string(), legacy_result);
            Ok(Value::Object(fields))
        }
        value => Ok(json!({
            "content": [{"type": "text", "text": legacy_value_text(&value)}],
            "result": legacy_result
        })),
    }
}

fn stdio_resource_descriptor((resource, metadata): (McpResourceDescriptor, Value)) -> Value {
    let McpResourceDescriptor {
        server_name,
        uri,
        description,
    } = resource;
    let name = metadata
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&uri)
        .to_string();
    let mut value = json!({
        // Standard MCP Resource fields.
        "uri": uri,
        "name": name,
        // Retain the pre-0.9.11 server selector as additive metadata.
        "server_name": server_name,
    });
    if let Some(description) = description {
        value["description"] = Value::String(description);
    }
    if let Some(mime_type) = metadata.get("mimeType").and_then(Value::as_str) {
        value["mimeType"] = Value::String(mime_type.to_string());
    }
    if let Some(size) = metadata
        .get("size")
        .filter(|size| size.as_i64().is_some() || size.as_u64().is_some())
    {
        value["size"] = size.clone();
    }
    if let Some(annotations) = metadata
        .get("annotations")
        .filter(|annotations| valid_mcp_annotations(annotations))
    {
        value["annotations"] = annotations.clone();
    }
    value
}

fn valid_resource_contents(contents: &Value) -> bool {
    contents
        .as_array()
        .is_some_and(|contents| contents.iter().all(valid_resource_content))
}

fn stdio_resource_read_result(uri: &str, result: Value) -> Value {
    let legacy_resource = result.clone();
    match result {
        Value::Object(mut fields)
            if fields.get("contents").is_some_and(valid_resource_contents)
                && fields.get("_meta").is_none_or(Value::is_object) =>
        {
            // Pass through a valid standard ReadResourceResult and retain the
            // old nested value for existing Codewhale management clients.
            fields.insert("resource".to_string(), legacy_resource);
            Value::Object(fields)
        }
        Value::String(text) => json!({
            "contents": [{"uri": uri, "text": text}],
            "resource": legacy_resource
        }),
        value => json!({
            "contents": [{"uri": uri, "text": legacy_value_text(&value)}],
            "resource": legacy_resource
        }),
    }
}

fn lifecycle_snapshot(state: &StdioMcpState) -> Value {
    let mut servers: Vec<Value> = state
        .definitions
        .iter()
        .map(|(name, definition)| {
            let is_running = state.running.get(name).copied().unwrap_or(false);
            json!({
                "name": name,
                "enabled": definition.config.enabled,
                "running": is_running,
                "command": definition.config.command.clone(),
                "args": definition.config.args.clone(),
                // Null when the server is healthy. A client polling
                // `server/list` must be able to tell "up" from "never
                // started" without guessing.
                "error": state.errors.get(name).cloned(),
            })
        })
        .collect();
    servers.sort_by(|a, b| {
        let a_name = a.get("name").and_then(Value::as_str).unwrap_or_default();
        let b_name = b.get("name").and_then(Value::as_str).unwrap_or_default();
        a_name.cmp(b_name)
    });

    let running_count = state.running.values().filter(|running| **running).count();
    json!({
        "status": state.lifecycle_state,
        "servers": servers,
        "counts": {
            "defined": state.definitions.len(),
            "running": running_count
        }
    })
}

fn params_or_object(params: Value) -> Value {
    if params.is_null() { json!({}) } else { params }
}

fn parse_params<T: DeserializeOwned>(params: Value) -> std::result::Result<T, JsonRpcError> {
    serde_json::from_value(params).map_err(|err| JsonRpcError::invalid_params(err.to_string()))
}

fn parse_server_from_uri(uri: &str) -> Option<String> {
    let stripped = uri.strip_prefix("mcp://")?;
    let server = stripped.split('/').next()?;
    if server.is_empty() {
        None
    } else {
        Some(server.to_string())
    }
}

fn require_ready_session(
    state: &StdioMcpState,
    method: &str,
) -> std::result::Result<(), JsonRpcError> {
    if state.session_phase != McpSessionPhase::Ready {
        return Err(JsonRpcError::invalid_request(format!(
            "{method} requires a completed initialize / notifications/initialized handshake"
        )));
    }
    Ok(())
}

fn dispatch_stdio_request(
    state: &mut StdioMcpState,
    method: &str,
    params: Value,
) -> std::result::Result<(Value, bool), JsonRpcError> {
    match method {
        "initialize" => {
            if state.session_phase != McpSessionPhase::Uninitialized {
                return Err(JsonRpcError::invalid_request(
                    "initialize may only be sent once per stdio session",
                ));
            }
            let parsed: InitializeParams = parse_params(params_or_object(params))?;
            if parsed.protocol_version.trim().is_empty() {
                return Err(JsonRpcError::invalid_params(
                    "protocolVersion must not be empty",
                ));
            }
            if parsed.client_info.name.trim().is_empty()
                || parsed.client_info.version.trim().is_empty()
            {
                return Err(JsonRpcError::invalid_params(
                    "clientInfo.name and clientInfo.version must not be empty",
                ));
            }
            // Deserializing into a Map above is the object-shape check. The
            // proxy does not currently consume any client capability.
            let _client_capabilities = parsed.capabilities;
            state.session_phase = McpSessionPhase::InitializeResponded;
            Ok((initialize_response(state), false))
        }
        // Pre-standard Codewhale management alias; it intentionally requires
        // no MCP initialize envelope.
        "capabilities" => Ok((initialize_response(state), false)),
        "notifications/initialized" => {
            if state.session_phase != McpSessionPhase::InitializeResponded {
                return Err(JsonRpcError::invalid_request(
                    "notifications/initialized requires a successful initialize request",
                ));
            }
            state.session_phase = McpSessionPhase::Ready;
            Ok((json!({}), false))
        }
        "ping" => Ok((json!({}), false)),
        "healthz" => Ok((
            json!({
                "status": "ok",
                "service": MCP_SERVER_NAME,
                "transport": "stdio",
                "lifecycle": lifecycle_snapshot(state)
            }),
            false,
        )),
        "tools/list" => {
            require_ready_session(state, method)?;
            let parsed: ToolsListParams = parse_params(params_or_object(params))?;
            let mut tools = state
                .manager
                .list_tools_with_input_schemas()
                .map_err(|err| JsonRpcError::internal(err.to_string()))?;
            if let Some(server) = parsed.server {
                tools.retain(|(tool, _)| tool.server_name == server);
            }
            let tools = tools
                .into_iter()
                .map(stdio_tool_descriptor)
                .collect::<Vec<_>>();
            Ok((json!({ "tools": tools }), false))
        }
        "tools/call" => {
            require_ready_session(state, method)?;
            let parsed: ToolsCallParams = parse_params(params_or_object(params))?;
            let ToolsCallParams {
                name,
                tool,
                server,
                arguments,
            } = parsed;
            let tool_name = name
                .or(tool)
                .context("missing tool name")
                .map_err(|err| JsonRpcError::invalid_params(err.to_string()))?;
            if !arguments.is_object() {
                return Err(JsonRpcError::invalid_params(
                    "tools/call arguments must be an object",
                ));
            }
            let result = if tool_name.starts_with("mcp__") {
                state
                    .manager
                    .call_qualified_tool(&tool_name, arguments)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?
            } else {
                let server = server
                    .context("missing server for unqualified tool")
                    .map_err(|err| JsonRpcError::invalid_params(err.to_string()))?;
                state
                    .manager
                    .call_tool(&server, &tool_name, arguments)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?
            };
            let result = stdio_tool_call_result(result)
                .map_err(|err| JsonRpcError::internal(err.to_string()))?;
            Ok((result, false))
        }
        "resources/list" => {
            require_ready_session(state, method)?;
            let parsed: ResourcesListParams = parse_params(params_or_object(params))?;
            let mut resources = state
                .manager
                .list_resources_with_metadata()
                .map_err(|err| JsonRpcError::internal(err.to_string()))?;
            if let Some(server) = parsed.server {
                resources.retain(|(resource, _)| resource.server_name == server);
            }
            let resources = resources
                .into_iter()
                .map(stdio_resource_descriptor)
                .collect::<Vec<_>>();
            Ok((json!({ "resources": resources }), false))
        }
        "resources/read" => {
            require_ready_session(state, method)?;
            let parsed: ResourcesReadParams = parse_params(params_or_object(params))?;
            let ResourcesReadParams { server, uri } = parsed;
            let value = match server {
                Some(server_name) => state.manager.read_resource(&server_name, &uri),
                None => state.manager.read_resource_by_uri(&uri),
            }
            .map_err(|err| JsonRpcError::internal(err.to_string()))?;
            Ok((stdio_resource_read_result(&uri, value), false))
        }
        "server/list" | "servers/list" => {
            Ok((json!({ "lifecycle": lifecycle_snapshot(state) }), false))
        }
        "server/register" | "servers/register" => {
            let parsed: ServerRegisterParams = parse_params(params_or_object(params))?;
            let name = parsed.server.name.clone();
            if name.trim().is_empty() {
                return Err(JsonRpcError::invalid_params(
                    "server.name must not be empty",
                ));
            }

            if state.definitions.contains_key(&name) {
                let _ = state.manager.unregister_server(&name);
            }
            let definition = McpServerDefinition {
                config: parsed.server.clone(),
                filter: parsed.filter.clone(),
            };
            state.definitions.insert(name.clone(), definition.clone());
            state.errors.remove(&name);
            if parsed.start && parsed.server.enabled {
                // Registration is only "ok" if the configured command actually
                // came up. Reporting success here and answering later tool
                // calls from a stub is what #4727 was.
                state
                    .start_definition(&definition)
                    .map_err(|err| JsonRpcError::internal(format!("{err:#}")))?;
            } else {
                state.running.insert(name, false);
            }
            Ok((json!({ "lifecycle": lifecycle_snapshot(state) }), false))
        }
        "server/start" | "servers/start" => {
            let parsed: ServerNameParams = parse_params(params_or_object(params))?;
            let definition = state
                .definitions
                .get(&parsed.name)
                .cloned()
                .with_context(|| format!("server '{}' is not defined", parsed.name))
                .map_err(|err| JsonRpcError::invalid_params(err.to_string()))?;
            if !definition.config.enabled {
                return Err(JsonRpcError::invalid_params(format!(
                    "server '{}' is disabled",
                    parsed.name
                )));
            }
            if !state.running.get(&parsed.name).copied().unwrap_or(false) {
                state
                    .start_definition(&definition)
                    .map_err(|err| JsonRpcError::internal(format!("{err:#}")))?;
            }
            Ok((json!({ "lifecycle": lifecycle_snapshot(state) }), false))
        }
        "server/stop" | "servers/stop" => {
            let parsed: ServerNameParams = parse_params(params_or_object(params))?;
            if state.running.get(&parsed.name).copied().unwrap_or(false) {
                state
                    .manager
                    .stop_server(&parsed.name)
                    .map_err(|err| JsonRpcError::internal(err.to_string()))?;
            }
            // A deliberate stop is not a failure, so it clears any recorded
            // startup error rather than leaving a stale one on display.
            state.errors.remove(&parsed.name);
            state.running.insert(parsed.name, false);
            Ok((json!({ "lifecycle": lifecycle_snapshot(state) }), false))
        }
        "server/unregister" | "servers/unregister" => {
            let parsed: ServerNameParams = parse_params(params_or_object(params))?;
            if state.definitions.remove(&parsed.name).is_none() {
                return Err(JsonRpcError::invalid_params(format!(
                    "server '{}' is not defined",
                    parsed.name
                )));
            }
            let _ = state.manager.unregister_server(&parsed.name);
            state.running.remove(&parsed.name);
            state.errors.remove(&parsed.name);
            Ok((json!({ "lifecycle": lifecycle_snapshot(state) }), false))
        }
        "shutdown" => {
            state.lifecycle_state = "shutting_down".to_string();
            Ok((
                json!({
                    "ok": true,
                    "lifecycle": lifecycle_snapshot(state)
                }),
                true,
            ))
        }
        _ => Err(JsonRpcError::method_not_found(method)),
    }
}

fn jsonrpc_result(id: Option<Value>, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "result": result
    })
}

fn jsonrpc_error(id: Option<Value>, err: JsonRpcError) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": {
            "code": err.code,
            "message": err.message,
            "data": err.data
        }
    })
}

impl JsonRpcError {
    fn parse_error(message: impl Into<String>) -> Self {
        Self {
            code: -32700,
            message: message.into(),
            data: None,
        }
    }

    fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: -32600,
            message: message.into(),
            data: None,
        }
    }

    fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("unsupported method: {method}"),
            data: None,
        }
    }

    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            data: None,
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
            data: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn complete_stdio_handshake(state: &mut StdioMcpState) {
        dispatch_stdio_request(
            state,
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "clientInfo": {"name": "test-client", "version": "1"},
                "capabilities": {}
            }),
        )
        .expect("initialize");
        dispatch_stdio_request(state, "notifications/initialized", Value::Null)
            .expect("initialized notification");
    }

    struct EchoMcpClient;

    impl McpManagedClient for EchoMcpClient {
        fn list_tools(&self) -> Result<Vec<McpToolDescriptor>> {
            Ok(vec![])
        }

        fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<Value> {
            if tool_name == "error" {
                bail!("intentional error for testing");
            }
            Ok(arguments)
        }

        fn list_resources(&self) -> Result<Vec<McpResourceDescriptor>> {
            Ok(vec![])
        }

        fn read_resource(&self, _uri: &str) -> Result<Value> {
            bail!("not supported")
        }
    }

    // ── InMemoryMcpClient ──────────────────────────────────────────────

    #[test]
    fn in_memory_client_list_tools_returns_registered() {
        let client = InMemoryMcpClient::default()
            .with_tool("echo", json!({"output": "hi"}))
            .with_tool("greet", json!({"msg": "hello"}));
        let tools = client.list_tools().unwrap();
        assert_eq!(tools.len(), 2);
        let names: Vec<&str> = tools.iter().map(|t| t.tool_name.as_str()).collect();
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"greet"));
    }

    #[test]
    fn in_memory_client_call_tool_returns_value() {
        let client = InMemoryMcpClient::default().with_tool("echo", json!({"output": "hi"}));
        let result = client.call_tool("echo", json!({})).unwrap();
        assert_eq!(result["output"], "hi");
    }

    #[test]
    fn in_memory_client_call_tool_errors_on_missing() {
        let client = InMemoryMcpClient::default();
        let err = client.call_tool("nope", json!({})).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn in_memory_client_list_resources_returns_registered() {
        let client = InMemoryMcpClient::default()
            .with_resource("mcp://s/health", json!({"ok": true}))
            .with_resource("mcp://s/caps", json!({"tools": []}));
        let resources = client.list_resources().unwrap();
        assert_eq!(resources.len(), 2);
    }

    #[test]
    fn in_memory_client_read_resource_returns_value() {
        let client =
            InMemoryMcpClient::default().with_resource("mcp://s/health", json!({"ok": true}));
        let result = client.read_resource("mcp://s/health").unwrap();
        assert_eq!(result["ok"], true);
    }

    #[test]
    fn in_memory_client_read_resource_errors_on_missing() {
        let client = InMemoryMcpClient::default();
        let err = client.read_resource("mcp://s/nope").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // ── McpManager ─────────────────────────────────────────────────────

    fn make_server_config(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            command: "test".to_string(),
            args: vec![],
            env: HashMap::new(),
            enabled: true,
        }
    }

    /// Client that counts `call_tool` invocations and always fails, so a
    /// retry shows up as a count rather than as a swallowed error.
    #[derive(Default)]
    struct CountingFailingClient {
        calls: Arc<AtomicUsize>,
    }

    impl McpManagedClient for CountingFailingClient {
        fn list_tools(&self) -> Result<Vec<McpToolDescriptor>> {
            Ok(vec![McpToolDescriptor {
                server_name: "counting".to_string(),
                tool_name: "write".to_string(),
                qualified_name: "write".to_string(),
                description: None,
            }])
        }

        fn call_tool(&self, _tool_name: &str, _arguments: Value) -> Result<Value> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            bail!("transient upstream failure")
        }

        fn list_resources(&self) -> Result<Vec<McpResourceDescriptor>> {
            Ok(Vec::new())
        }

        fn read_resource(&self, _uri: &str) -> Result<Value> {
            bail!("no resources")
        }
    }

    #[test]
    fn failed_qualified_tool_call_is_not_retried() {
        // #4728: the fast path used to fall through to a re-resolution loop
        // whenever the *call* errored, re-invoking the same tool. For a file
        // write, a commit, or a paid API call, that second invocation is a
        // second real side effect.
        let calls = Arc::new(AtomicUsize::new(0));
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("writer"),
                ToolFilter::default(),
                Box::new(CountingFailingClient {
                    calls: Arc::clone(&calls),
                }),
            )
            .unwrap();

        let err = manager
            .call_qualified_tool("mcp__writer__write", json!({}))
            .unwrap_err();

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "tool must be invoked exactly once, got {} invocations",
            calls.load(Ordering::SeqCst)
        );
        // The original error is propagated, not discarded in favour of a
        // later attempt's.
        assert!(
            err.to_string().contains("transient upstream failure"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn register_server_rejects_a_name_that_collides_after_sanitizing() {
        // #4729: `my-server` and `my_server` both qualify as `mcp__my_server__*`,
        // so registering both would let either answer a call meant for the
        // other, decided by HashMap iteration order.
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("my_server"),
                ToolFilter::default(),
                Box::new(InMemoryMcpClient::default().with_tool("t", json!("trusted"))),
            )
            .unwrap();

        for colliding in ["my-server", "My.Server"] {
            let err = manager
                .register_server(
                    make_server_config(colliding),
                    ToolFilter::default(),
                    Box::new(InMemoryMcpClient::default().with_tool("t", json!("hostile"))),
                )
                .unwrap_err();
            assert!(
                err.to_string().contains("collides"),
                "expected collision error for {colliding}, got: {err}"
            );
        }

        // The trusted server keeps answering its own qualified name.
        assert_eq!(
            manager
                .call_qualified_tool("mcp__my_server__t", json!({}))
                .unwrap(),
            json!("trusted")
        );
    }

    #[test]
    fn same_server_tool_name_collisions_fail_closed_in_list_and_call() {
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("s1"),
                ToolFilter::default(),
                Box::new(
                    InMemoryMcpClient::default()
                        .with_tool("foo-bar", json!("hyphen"))
                        .with_tool("foo_bar", json!("underscore")),
                ),
            )
            .unwrap();

        let list_error = manager.list_tools().unwrap_err();
        assert!(
            list_error.to_string().contains("ambiguous"),
            "unexpected list error: {list_error}"
        );

        let call_error = manager
            .call_qualified_tool("mcp__s1__foo_bar", json!({}))
            .unwrap_err();
        assert!(
            call_error
                .to_string()
                .contains("ambiguous within server 's1'"),
            "unexpected call error: {call_error}"
        );
    }

    #[test]
    fn re_registering_the_same_server_name_replaces_it() {
        // Collision rejection must not break restart, which re-registers the
        // same name with a fresh client.
        let mut manager = McpManager::default();
        for value in ["first", "second"] {
            manager
                .register_server(
                    make_server_config("s1"),
                    ToolFilter::default(),
                    Box::new(InMemoryMcpClient::default().with_tool("t", json!(value))),
                )
                .unwrap();
        }
        assert_eq!(
            manager
                .call_qualified_tool("mcp__s1__t", json!({}))
                .unwrap(),
            json!("second")
        );
    }

    #[test]
    fn manager_start_all_marks_ready_for_registered_clients() {
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("s1"),
                ToolFilter::default(),
                Box::new(InMemoryMcpClient::default().with_tool("t", json!(null))),
            )
            .unwrap();
        let mut events = Vec::new();
        let summary = manager.start_all(|e| events.push(e));
        assert_eq!(summary.ready, vec!["s1"]);
        assert!(summary.failed.is_empty());
        assert!(events.iter().any(|event| {
            event.server_name == "s1" && event.status == McpStartupStatus::Starting
        }));
        assert!(
            events.iter().any(|event| {
                event.server_name == "s1" && event.status == McpStartupStatus::Ready
            })
        );
    }

    #[test]
    fn manager_start_all_marks_failed_when_client_missing() {
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("s1"),
                ToolFilter::default(),
                Box::new(InMemoryMcpClient::default()),
            )
            .unwrap();
        manager.stop_server("s1").unwrap();
        let summary = manager.start_all(|_| {});
        assert!(summary.ready.is_empty());
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].server_name, "s1");
    }

    #[test]
    fn manager_start_all_cancels_disabled_servers() {
        let mut manager = McpManager::default();
        let mut cfg = make_server_config("s1");
        cfg.enabled = false;
        manager
            .register_server(
                cfg,
                ToolFilter::default(),
                Box::new(InMemoryMcpClient::default()),
            )
            .unwrap();
        let summary = manager.start_all(|_| {});
        assert!(summary.ready.is_empty());
        assert_eq!(summary.cancelled, vec!["s1"]);
    }

    #[test]
    fn manager_list_tools_applies_filter() {
        let mut manager = McpManager::default();
        let client = InMemoryMcpClient::default()
            .with_tool("allowed", json!(null))
            .with_tool("denied", json!(null));
        manager
            .register_server(
                make_server_config("s1"),
                ToolFilter {
                    allow: vec!["allowed".to_string()],
                    deny: vec![],
                },
                Box::new(client),
            )
            .unwrap();
        let tools = manager.list_tools().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool_name, "allowed");
    }

    #[test]
    fn manager_list_tools_deny_overrides_allow() {
        let mut manager = McpManager::default();
        let client = InMemoryMcpClient::default()
            .with_tool("a", json!(null))
            .with_tool("b", json!(null));
        manager
            .register_server(
                make_server_config("s1"),
                ToolFilter {
                    allow: vec!["a".to_string(), "b".to_string()],
                    deny: vec!["b".to_string()],
                },
                Box::new(client),
            )
            .unwrap();
        let tools = manager.list_tools().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool_name, "a");
    }

    #[test]
    fn manager_call_tool_delegates_to_client() {
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("s1"),
                ToolFilter::default(),
                Box::new(InMemoryMcpClient::default().with_tool("t", json!({"v": 42}))),
            )
            .unwrap();
        let result = manager.call_tool("s1", "t", json!({})).unwrap();
        assert_eq!(result["v"], 42);
    }

    #[test]
    fn manager_call_tool_passes_arguments_to_client() {
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("s1"),
                ToolFilter::default(),
                Box::new(EchoMcpClient),
            )
            .unwrap();
        let args = json!({"hello": "world", "num": 100});
        let result = manager.call_tool("s1", "echo", args.clone()).unwrap();
        assert_eq!(result, args);
    }

    #[test]
    fn manager_call_tool_propagates_client_error() {
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("s1"),
                ToolFilter::default(),
                Box::new(EchoMcpClient),
            )
            .unwrap();
        let err = manager.call_tool("s1", "error", json!({})).unwrap_err();
        assert!(err.to_string().contains("intentional error for testing"));
    }

    #[test]
    fn manager_call_tool_errors_on_missing_server() {
        let manager = McpManager::default();
        let err = manager.call_tool("nope", "t", json!({})).unwrap_err();
        assert!(err.to_string().contains("not available"));
    }

    #[test]
    fn manager_call_tool_enforces_deny_filter() {
        // The filter used to be consulted only when listing tools; a denied
        // tool stayed callable by addressing the server directly.
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("s1"),
                ToolFilter {
                    allow: vec![],
                    deny: vec!["secret".to_string()],
                },
                Box::new(InMemoryMcpClient::default().with_tool("secret", json!({"ok": true}))),
            )
            .unwrap();
        let err = manager.call_tool("s1", "secret", json!({})).unwrap_err();
        assert!(
            err.to_string().contains("blocked by the tool filter"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn manager_call_tool_enforces_allow_filter() {
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("s1"),
                ToolFilter {
                    allow: vec!["allowed".to_string()],
                    deny: vec![],
                },
                Box::new(
                    InMemoryMcpClient::default()
                        .with_tool("allowed", json!({"ok": true}))
                        .with_tool("other", json!({"ok": false})),
                ),
            )
            .unwrap();
        let err = manager.call_tool("s1", "other", json!({})).unwrap_err();
        assert!(
            err.to_string().contains("blocked by the tool filter"),
            "unexpected error: {err}"
        );
        // The allowed tool still runs.
        assert_eq!(
            manager.call_tool("s1", "allowed", json!({})).unwrap(),
            json!({"ok": true})
        );
    }

    #[test]
    fn denied_tool_cannot_be_called_by_qualified_name() {
        // Security: `mcp__s1__secret` must be as unreachable as `secret`.
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("s1"),
                ToolFilter {
                    allow: vec![],
                    deny: vec!["secret".to_string()],
                },
                Box::new(InMemoryMcpClient::default().with_tool("secret", json!({"ok": true}))),
            )
            .unwrap();
        let err = manager
            .call_qualified_tool("mcp__s1__secret", json!({}))
            .unwrap_err();
        assert!(
            err.to_string().contains("blocked by the tool filter"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn manager_call_qualified_tool_parses_name() {
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("my_server"),
                ToolFilter::default(),
                Box::new(InMemoryMcpClient::default().with_tool("my_tool", json!({"ok": true}))),
            )
            .unwrap();
        let result = manager
            .call_qualified_tool("mcp__my_server__my_tool", json!({}))
            .unwrap();
        assert_eq!(result["ok"], true);
    }

    #[test]
    fn manager_call_qualified_tool_resolves_sanitized_segment_to_original_name() {
        // `qualify_tool_name` folds `-`/`.`/case into `_`, so the qualified
        // name advertised for `my-tool` is `mcp__s1__my_tool`. The exact-match
        // fast path used to dispatch that sanitized segment verbatim, and the
        // server (which only knows `my-tool`) rejected the call.
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("s1"),
                ToolFilter::default(),
                Box::new(
                    InMemoryMcpClient::default()
                        .with_tool("my-tool", json!({"via": "hyphen"}))
                        .with_tool("other.thing", json!({"via": "dot"})),
                ),
            )
            .unwrap();

        let hyphen = manager
            .call_qualified_tool("mcp__s1__my_tool", json!({}))
            .unwrap();
        assert_eq!(hyphen, json!({"via": "hyphen"}));

        let dot = manager
            .call_qualified_tool("mcp__s1__other_thing", json!({}))
            .unwrap();
        assert_eq!(dot, json!({"via": "dot"}));
    }

    #[test]
    fn manager_call_qualified_tool_handles_truncated_names() {
        let long_server = "server".repeat(20);
        let long_tool = "tool".repeat(20);
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config(&long_server),
                ToolFilter::default(),
                Box::new(InMemoryMcpClient::default().with_tool(&long_tool, json!({"ok": true}))),
            )
            .unwrap();
        let tools = manager.list_tools().unwrap();
        let qualified = &tools[0].qualified_name;
        assert!(qualified.len() <= 64);
        assert!(parse_qualified_tool_name(qualified).is_ok());

        let result = manager.call_qualified_tool(qualified, json!({})).unwrap();
        assert_eq!(result["ok"], true);
    }

    #[test]
    fn manager_unregister_removes_server() {
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("s1"),
                ToolFilter::default(),
                Box::new(InMemoryMcpClient::default()),
            )
            .unwrap();
        manager.unregister_server("s1").unwrap();
        assert!(manager.configs.is_empty());
    }

    #[test]
    fn manager_unregister_errors_on_unknown() {
        let mut manager = McpManager::default();
        let err = manager.unregister_server("nope").unwrap_err();
        assert!(err.to_string().contains("not registered"));
    }

    #[test]
    fn manager_stop_server_errors_on_unknown() {
        let mut manager = McpManager::default();
        let err = manager.stop_server("nope").unwrap_err();
        assert!(err.to_string().contains("not running"));
    }

    #[test]
    fn manager_list_resources_returns_from_clients() {
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("s1"),
                ToolFilter::default(),
                Box::new(
                    InMemoryMcpClient::default()
                        .with_resource("mcp://s1/health", json!({"ok": true})),
                ),
            )
            .unwrap();
        let resources = manager.list_resources().unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].server_name, "s1");
    }

    #[test]
    fn manager_read_resource_delegates() {
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("s1"),
                ToolFilter::default(),
                Box::new(
                    InMemoryMcpClient::default()
                        .with_resource("mcp://s1/health", json!({"ok": true})),
                ),
            )
            .unwrap();
        let result = manager.read_resource("s1", "mcp://s1/health").unwrap();
        assert_eq!(result["ok"], true);
    }

    #[test]
    fn manager_resolves_a_unique_standard_resource_uri() {
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("docs"),
                ToolFilter::default(),
                Box::new(
                    InMemoryMcpClient::default()
                        .with_resource("file:///guide.md", json!({"text": "guide"})),
                ),
            )
            .unwrap();

        let result = manager.read_resource_by_uri("file:///guide.md").unwrap();
        assert_eq!(result["text"], "guide");
    }

    #[test]
    fn manager_rejects_an_ambiguous_standard_resource_uri() {
        let mut manager = McpManager::default();
        for server in ["alpha", "beta"] {
            manager
                .register_server(
                    make_server_config(server),
                    ToolFilter::default(),
                    Box::new(
                        InMemoryMcpClient::default()
                            .with_resource("file:///shared.txt", json!({"server": server})),
                    ),
                )
                .unwrap();
        }

        let err = manager
            .read_resource_by_uri("file:///shared.txt")
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("ambiguous"), "unexpected error: {message}");
        assert!(
            message.contains("alpha, beta"),
            "server names must be deterministic: {message}"
        );
    }

    #[test]
    fn manager_update_sandbox_state_returns_notices() {
        let mut manager = McpManager::default();
        manager
            .register_server(
                make_server_config("s1"),
                ToolFilter::default(),
                Box::new(InMemoryMcpClient::default()),
            )
            .unwrap();
        let notices = manager.update_sandbox_state("strict", "/tmp").unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0]["server_name"], "s1");
    }

    // ── Tool filter ────────────────────────────────────────────────────

    #[test]
    fn allowed_by_filter_empty_allow_permits_all() {
        let filter = ToolFilter {
            allow: vec![],
            deny: vec![],
        };
        assert!(allowed_by_filter("anything", &filter));
    }

    #[test]
    fn allowed_by_filter_deny_blocks() {
        let filter = ToolFilter {
            allow: vec![],
            deny: vec!["danger".to_string()],
        };
        assert!(!allowed_by_filter("danger", &filter));
        assert!(allowed_by_filter("safe", &filter));
    }

    #[test]
    fn allowed_by_filter_allow_only_permits_listed() {
        let filter = ToolFilter {
            allow: vec!["a".to_string()],
            deny: vec![],
        };
        assert!(allowed_by_filter("a", &filter));
        assert!(!allowed_by_filter("b", &filter));
    }

    // ── Helper functions ───────────────────────────────────────────────

    #[test]
    fn sanitize_component_lowercases_and_replaces_specials() {
        assert_eq!(sanitize_component("My-Server.Name"), "my_server_name");
        assert_eq!(sanitize_component("ABC123"), "abc123");
    }

    #[test]
    fn qualify_tool_name_produces_mcp_prefix() {
        let name = qualify_tool_name("server", "tool");
        assert!(name.starts_with("mcp__server__tool"));
    }

    #[test]
    fn qualify_tool_name_truncates_long_names() {
        let long_server = "a".repeat(100);
        let name = qualify_tool_name(&long_server, "tool");
        assert!(name.len() <= 64);
        assert!(parse_qualified_tool_name(&name).is_ok());
    }

    #[test]
    fn parse_qualified_tool_name_round_trip() {
        let qualified = qualify_tool_name("my_server", "my_tool");
        let (server, tool) = parse_qualified_tool_name(&qualified).unwrap();
        assert_eq!(server, "my_server");
        assert_eq!(tool, "my_tool");
    }

    #[test]
    fn parse_qualified_tool_name_rejects_missing_prefix() {
        let err = parse_qualified_tool_name("not_mcp__server__tool").unwrap_err();
        assert!(err.to_string().contains("missing mcp__ prefix"));
    }

    #[test]
    fn parse_qualified_tool_name_rejects_empty_segments() {
        let err = parse_qualified_tool_name("mcp____tool").unwrap_err();
        assert!(err.to_string().contains("missing server segment"));
    }

    #[test]
    fn parse_server_from_uri_extracts_server() {
        assert_eq!(
            parse_server_from_uri("mcp://my-server/capabilities"),
            Some("my-server".to_string())
        );
    }

    #[test]
    fn parse_server_from_uri_returns_none_for_invalid() {
        assert!(parse_server_from_uri("http://not-mcp").is_none());
        assert!(parse_server_from_uri("mcp:///path").is_none());
    }

    // ── JsonRpcError ───────────────────────────────────────────────────

    #[test]
    fn jsonrpc_error_codes_are_correct() {
        assert_eq!(JsonRpcError::parse_error("").code, -32700);
        assert_eq!(JsonRpcError::invalid_request("").code, -32600);
        assert_eq!(JsonRpcError::method_not_found("x").code, -32601);
        assert_eq!(JsonRpcError::invalid_params("").code, -32602);
        assert_eq!(JsonRpcError::internal("").code, -32603);
    }

    #[test]
    fn jsonrpc_result_produces_valid_envelope() {
        let result = jsonrpc_result(Some(json!(1)), json!({"ok": true}));
        assert_eq!(result["jsonrpc"], "2.0");
        assert_eq!(result["id"], 1);
        assert_eq!(result["result"]["ok"], true);
    }

    #[test]
    fn jsonrpc_error_produces_valid_envelope() {
        let err = jsonrpc_error(Some(json!(2)), JsonRpcError::invalid_params("bad"));
        assert_eq!(err["jsonrpc"], "2.0");
        assert_eq!(err["id"], 2);
        assert_eq!(err["error"]["code"], -32602);
    }

    #[test]
    fn jsonrpc_missing_and_explicit_null_ids_remain_distinct() {
        let notification: JsonRpcRequest = serde_json::from_value(json!({
            "jsonrpc": "2.0",
            "method": "ping"
        }))
        .unwrap();
        assert!(!notification.id.should_respond());
        assert_eq!(notification.id.response_id(), None);

        let null_id: JsonRpcRequest = serde_json::from_value(json!({
            "jsonrpc": "2.0",
            "id": null,
            "method": "ping"
        }))
        .unwrap();
        assert!(null_id.id.should_respond());
        assert_eq!(null_id.id.response_id(), Some(Value::Null));

        for invalid in [
            json!({"method": "ping"}),
            json!({"jsonrpc": null, "method": "ping"}),
            json!({"jsonrpc": "2.0", "id": {}, "method": "ping"}),
        ] {
            assert!(
                serde_json::from_value::<JsonRpcRequest>(invalid).is_err(),
                "invalid envelope was accepted"
            );
        }
    }

    #[test]
    fn stdio_initialize_uses_standard_mcp_shape_and_codewhale_identity() {
        let state = build_stdio_state(Vec::new());
        let response = initialize_response(&state);
        assert_eq!(response["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(response["serverInfo"]["name"], MCP_SERVER_NAME);
        assert_eq!(response["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        assert!(response["capabilities"]["tools"].is_object());
        assert!(response["capabilities"]["resources"].is_object());
        assert_eq!(response["server"], MCP_SERVER_NAME);
    }

    #[test]
    fn stdio_initialize_validates_required_client_fields() {
        let mut state = build_stdio_state(Vec::new());
        let valid = dispatch_stdio_request(
            &mut state,
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "clientInfo": {"name": "test-client", "version": "1"},
                "capabilities": {}
            }),
        )
        .expect("valid initialize");
        assert_eq!(valid.0["protocolVersion"], MCP_PROTOCOL_VERSION);

        for invalid in [
            json!({}),
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "clientInfo": {"name": "test-client", "version": "1"}
            }),
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "clientInfo": {"name": "test-client", "version": "1"},
                "capabilities": []
            }),
            json!({
                "protocolVersion": "",
                "clientInfo": {"name": "test-client", "version": "1"},
                "capabilities": {}
            }),
        ] {
            let mut state = build_stdio_state(Vec::new());
            let error = dispatch_stdio_request(&mut state, "initialize", invalid)
                .expect_err("malformed initialize must fail");
            assert_eq!(error.code, -32602);
        }
    }

    #[test]
    fn standard_catalog_methods_require_the_complete_mcp_handshake() {
        let mut state = build_stdio_state(Vec::new());
        let early_initialized =
            dispatch_stdio_request(&mut state, "notifications/initialized", Value::Null)
                .expect_err("initialized cannot precede initialize");
        assert_eq!(early_initialized.code, -32600);

        // Explicit Codewhale management compatibility remains available before
        // MCP initialization.
        assert!(dispatch_stdio_request(&mut state, "capabilities", Value::Null).is_ok());
        assert!(dispatch_stdio_request(&mut state, "server/list", Value::Null).is_ok());

        for method in [
            "tools/list",
            "tools/call",
            "resources/list",
            "resources/read",
        ] {
            let error = dispatch_stdio_request(&mut state, method, json!({}))
                .expect_err("standard MCP data methods must fail before initialize");
            assert_eq!(error.code, -32600);
            assert!(error.message.contains("completed initialize"));
        }

        dispatch_stdio_request(
            &mut state,
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "clientInfo": {"name": "test-client", "version": "1"},
                "capabilities": {}
            }),
        )
        .unwrap();
        let too_early = dispatch_stdio_request(&mut state, "tools/list", json!({}))
            .expect_err("initialized notification is required");
        assert_eq!(too_early.code, -32600);

        dispatch_stdio_request(&mut state, "notifications/initialized", Value::Null).unwrap();
        let tools = dispatch_stdio_request(&mut state, "tools/list", json!({})).unwrap();
        assert_eq!(tools.0["tools"], json!([]));

        let duplicate = dispatch_stdio_request(
            &mut state,
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "clientInfo": {"name": "test-client", "version": "1"},
                "capabilities": {}
            }),
        )
        .expect_err("duplicate initialize must fail");
        assert_eq!(duplicate.code, -32600);
    }

    #[test]
    fn stdio_tools_call_rejects_non_object_arguments() {
        let mut state = build_stdio_state(Vec::new());
        complete_stdio_handshake(&mut state);
        for arguments in [json!(null), json!("bad"), json!([]), json!(1)] {
            let error = dispatch_stdio_request(
                &mut state,
                "tools/call",
                json!({"name": "mcp__missing__tool", "arguments": arguments}),
            )
            .expect_err("non-object arguments must fail before dispatch");
            assert_eq!(error.code, -32602);
            assert!(error.message.contains("arguments must be an object"));
        }
    }

    #[test]
    fn stdio_tool_results_only_pass_through_valid_mcp_content_arrays() {
        let standard = json!({
            "content": [{"type": "text", "text": "ok"}],
            "isError": true
        });
        let standard_response = stdio_tool_call_result(standard.clone()).unwrap();
        assert_eq!(standard_response["content"][0]["text"], "ok");
        assert_eq!(standard_response["isError"], true);
        assert_eq!(standard_response["result"], standard);

        let legacy = json!({"answer": 42});
        let legacy_response = stdio_tool_call_result(legacy.clone()).unwrap();
        assert_eq!(legacy_response["content"][0]["type"], "text");
        assert_eq!(legacy_response["result"], legacy);

        for malformed in [
            json!({"content": {"type": "text", "text": "not-an-array"}}),
            json!({"content": [null]}),
            json!({"content": [{"type": "text"}]}),
            json!({"content": [{"type": "image", "data": "abc"}]}),
            json!({"content": [{"type": "resource", "resource": {"uri": "file:///x"}}]}),
            json!({"content": [], "isError": "false"}),
            json!({"content": [], "_meta": []}),
            json!({"isError": true}),
            json!({"_meta": {"trace": "x"}}),
        ] {
            let error = stdio_tool_call_result(malformed.clone())
                .expect_err("CallToolResult-shaped malformed output must fail closed");
            assert!(error.to_string().contains("malformed MCP CallToolResult"));
        }

        let legacy_string = stdio_tool_call_result(json!("plain text")).unwrap();
        assert_eq!(legacy_string["content"][0]["text"], "plain text");
    }

    #[test]
    fn malformed_call_tool_result_shape_becomes_a_protocol_error() {
        let mut state = build_stdio_state(Vec::new());
        state
            .manager
            .register_server(
                make_server_config("malformed"),
                ToolFilter::default(),
                Box::new(
                    InMemoryMcpClient::default().with_tool("broken", json!({"isError": true})),
                ),
            )
            .unwrap();
        complete_stdio_handshake(&mut state);

        let error = dispatch_stdio_request(
            &mut state,
            "tools/call",
            json!({"name": "mcp__malformed__broken", "arguments": {}}),
        )
        .expect_err("malformed CallToolResult-shaped output must not become success text");
        assert_eq!(error.code, -32603);
        assert!(error.message.contains("malformed MCP CallToolResult"));
    }

    #[test]
    fn stdio_resources_use_standard_shapes_with_legacy_metadata() {
        let descriptor = McpResourceDescriptor {
            server_name: "docs".to_string(),
            uri: "file:///guide.md".to_string(),
            description: Some("User guide".to_string()),
        };
        let listed = stdio_resource_descriptor((
            descriptor,
            json!({
                "name": "Guide",
                "mimeType": "text/markdown",
                "size": 42,
                "annotations": {"audience": ["assistant"], "priority": 0.75}
            }),
        ));
        assert_eq!(listed["uri"], "file:///guide.md");
        assert_eq!(listed["name"], "Guide");
        assert_eq!(listed["mimeType"], "text/markdown");
        assert_eq!(listed["size"], 42);
        assert_eq!(listed["annotations"]["audience"], json!(["assistant"]));
        assert_eq!(listed["annotations"]["priority"], 0.75);
        assert_eq!(listed["server_name"], "docs");

        let standard = json!({
            "contents": [{
                "uri": "file:///guide.md",
                "mimeType": "text/markdown",
                "text": "hello"
            }]
        });
        let standard_response = stdio_resource_read_result("file:///guide.md", standard.clone());
        assert_eq!(standard_response["contents"][0]["text"], "hello");
        assert_eq!(standard_response["resource"], standard);

        for legacy in [
            json!({"body": "legacy"}),
            json!({"contents": [{"text": "missing URI"}]}),
            json!({"contents": [], "_meta": []}),
        ] {
            let response = stdio_resource_read_result("file:///guide.md", legacy.clone());
            assert_eq!(response["contents"][0]["uri"], "file:///guide.md");
            assert!(response["contents"][0]["text"].is_string());
            assert_eq!(response["resource"], legacy);
        }
    }

    // ── stdio dispatch: no stub may answer for a configured server ─────

    fn definition(name: &str, command: &str, args: &[&str]) -> McpServerDefinition {
        McpServerDefinition {
            config: McpServerConfig {
                name: name.to_string(),
                command: command.to_string(),
                args: args.iter().map(|arg| (*arg).to_string()).collect(),
                env: HashMap::new(),
                enabled: true,
            },
            filter: ToolFilter::default(),
        }
    }

    fn call(state: &mut StdioMcpState, method: &str, params: Value) -> Value {
        dispatch_stdio_request(state, method, params)
            .unwrap_or_else(|err| panic!("{method} failed: {}", err.message))
            .0
    }

    #[test]
    fn a_server_that_cannot_be_spawned_is_reported_not_running() {
        // #4727: this used to register a stub and report `running: true`, so a
        // typo in `command` was indistinguishable from a working server.
        let mut state = build_stdio_state(vec![definition(
            "broken",
            "codewhale-nonexistent-mcp-server-binary",
            &[],
        )]);

        let lifecycle = call(&mut state, "server/list", json!({}))["lifecycle"].clone();
        assert_eq!(lifecycle["servers"][0]["running"], json!(false));
        let error = lifecycle["servers"][0]["error"]
            .as_str()
            .expect("a stopped server must carry its failure reason");
        assert!(
            error.contains("failed to spawn command"),
            "unexpected error: {error}"
        );

        // And nothing answers on its behalf.
        complete_stdio_handshake(&mut state);
        let err = dispatch_stdio_request(
            &mut state,
            "tools/call",
            json!({"name": "mcp__broken__health", "arguments": {}}),
        )
        .expect_err("a server that never started must not answer tool calls");
        assert_eq!(err.code, -32603);
    }

    #[cfg(unix)]
    #[test]
    fn tools_come_from_the_spawned_process_not_a_stub() {
        let script = crate::test_support::write_fake_mcp_server("dispatch_tools");
        let mut state = build_stdio_state(vec![definition(
            "fake",
            "/bin/sh",
            &[script.path().to_str().expect("utf-8 script path")],
        )]);
        complete_stdio_handshake(&mut state);

        let tools = call(&mut state, "tools/list", json!({}))["tools"].clone();
        let names: Vec<&str> = tools
            .as_array()
            .expect("tools array")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert_eq!(
            names,
            vec!["mcp__fake__add"],
            "only the child's real tools may be listed, got {names:?}"
        );
        assert_eq!(tools[0]["tool_name"], "add");
        assert_eq!(tools[0]["inputSchema"]["required"], json!(["a", "b"]));

        let result = call(
            &mut state,
            "tools/call",
            json!({"name": "mcp__fake__add", "arguments": {"a": 2, "b": 3}}),
        );
        assert_eq!(result["content"][0]["text"], "5");
        assert_eq!(result["result"]["content"][0]["text"], "5");

        let resources = call(&mut state, "resources/list", json!({}))["resources"].clone();
        assert_eq!(resources[0]["uri"], "file:///fake/readme.txt");
        assert_eq!(resources[0]["name"], "Fake readme");
        assert_eq!(resources[0]["mimeType"], "text/plain");
        assert_eq!(resources[0]["size"], 16);
        assert_eq!(
            resources[0]["annotations"]["audience"],
            json!(["assistant"])
        );
        assert_eq!(resources[0]["server_name"], "fake");

        let read = call(
            &mut state,
            "resources/read",
            json!({"uri": "file:///fake/readme.txt"}),
        );
        assert_eq!(read["contents"][0]["text"], "spawned-resource");
        assert_eq!(read["resource"]["contents"][0]["text"], "spawned-resource");
    }

    #[cfg(unix)]
    #[test]
    fn server_register_fails_when_the_command_cannot_be_started() {
        let mut state = build_stdio_state(Vec::new());
        let err = dispatch_stdio_request(
            &mut state,
            "server/register",
            json!({"server": {"name": "late", "command": "/bin/sh", "args": ["-c", "exit 1"]}}),
        )
        .expect_err("registering an unstartable server must not report success");
        assert_eq!(err.code, -32603);
        assert!(
            err.message.contains("initialize"),
            "unexpected error: {}",
            err.message
        );
        assert_eq!(state.running.get("late"), Some(&false));
    }

    // ── McpServerConfig serialization ──────────────────────────────────

    #[test]
    fn mcp_server_config_defaults_enabled_to_true() {
        let json = json!({"name": "s", "command": "cmd"});
        let config: McpServerConfig = serde_json::from_value(json).unwrap();
        assert!(config.enabled);
        assert!(config.args.is_empty());
        assert!(config.env.is_empty());
    }

    #[test]
    fn mcp_startup_status_serializes_with_snake_case() {
        let status = McpStartupStatus::Failed {
            error: "oops".to_string(),
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["failed"]["error"], "oops");
    }
}
