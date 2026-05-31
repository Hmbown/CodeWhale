# RFC: MCP Modularization

**Issue:** #2190
**Status:** Draft
**Date:** 2026-05-26

## 1. Current state

### 1.1 `codewhale-mcp` crate (`crates/mcp/`)

The current MCP implementation lives in a single crate with two responsibilities:

- **MCP client** — connects to MCP servers over stdio, manages protocol handshake,
  tool discovery, and tool invocation. Used by the TUI to surface MCP tools as
  `mcp_<server>_<tool>` entries in the tool registry.
- **MCP stdio server** — a minimal MCP server that exposes CodeWhale's own tools
  over stdio for external MCP clients. Used by the `codewhale mcp` CLI subcommand.

Both the client and server share protocol types (JSON-RPC messages, tool schemas)
but have different lifecycle concerns and different callers.

### 1.2 Integration points

- `crates/tui/src/mcp.rs` — MCP client integration: server lifecycle, tool
  discovery, tool execution forwarding
- `crates/tui/src/mcp_server.rs` — MCP stdio server: exposes TUI tools via
  stdio MCP protocol
- `docs/MCP.md` — user-facing documentation

## 2. Motivation

### 2.1 Separation of concerns

The client and server share a crate but have no shared code paths at runtime.
They import the same protocol types but serve different roles:
- The client is **outbound** — it connects to external servers
- The server is **inbound** — it accepts connections from external clients

Mixing them in one crate creates unnecessary coupling: changes to the server
API recompile the client, and vice versa.

### 2.2 OAuth support

The current MCP client has no OAuth support. MCP servers that require OAuth
(e.g., GitHub, Google) cannot be used. Adding OAuth to the client requires:
- Token storage (keychain, env-based, or config-based)
- OAuth flow (device code, PKCE, or client credentials)
- Token refresh and expiry handling

These concerns are client-side only and should not affect the server crate.

### 2.3 Reuse outside the TUI

The MCP client is currently embedded in the TUI binary. If we want to use
MCP tools from:
- The `app-server` (HTTP/SSE runtime API)
- The `codewhale` CLI (non-interactive mode)
- External consumers (library use)

...the client needs to be a standalone crate with a clean public API.

## 3. Proposed crate split

```
crates/mcp/           →  crates/mcp-protocol/   (shared types, no I/O)
                          crates/mcp-client/     (client implementation)
                          crates/mcp-server/     (server implementation)
```

### 3.1 `codewhale-mcp-protocol`

**Contents:** JSON-RPC message types, tool schema types, protocol constants,
handshake types, error types. No I/O, no async runtime dependency.

**Dependencies:** `serde`, `serde_json`, `codewhale-protocol` (for tool schema)

**Public API:**
```rust
pub mod messages;     // JSON-RPC request/response/notification types
pub mod tools;        // MCP tool schema types
pub mod errors;       // MCP error codes
pub mod version;      // Protocol version constants
```

### 3.2 `codewhale-mcp-client`

**Contents:** MCP client: stdio transport, process management, handshake,
tool discovery, tool invocation, OAuth support.

**Dependencies:** `codewhale-mcp-protocol`, `tokio`, `serde_json`, `tracing`,
`oauth2` (new, for OAuth), `keyring` (optional, for token storage)

**Public API:**
```rust
pub struct McpClient {
    // Configuration
}

impl McpClient {
    pub async fn connect(config: McpClientConfig) -> Result<Self>;
    pub async fn list_tools(&self) -> Result<Vec<ToolSchema>>;
    pub async fn call_tool(&self, name: &str, args: Value) -> Result<Value>;
    pub async fn disconnect(self);
}

pub struct McpClientConfig {
    pub command: String,           // e.g., "npx", "python"
    pub args: Vec<String>,         // e.g., ["-y", "@modelcontextprotocol/server-github"]
    pub env: HashMap<String, String>,
    pub oauth: Option<OAuthConfig>,
    pub timeout: Duration,
}

pub struct OAuthConfig {
    pub provider: OAuthProvider,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub token_storage: TokenStorage,
}

pub enum OAuthProvider {
    Github,
    Google,
    Custom { auth_url: String, token_url: String },
}
```

### 3.3 `codewhale-mcp-server`

**Contents:** MCP stdio server: accepts connections, exposes tool list,
handles tool calls, manages stdio transport.

**Dependencies:** `codewhale-mcp-protocol`, `codewhale-tools`, `tokio`,
`serde_json`, `tracing`

**Public API:**
```rust
pub struct McpServer {
    // Tool registry
}

impl McpServer {
    pub fn new(tools: Vec<Arc<dyn ToolSpec>>) -> Self;
    pub async fn serve_stdio(self) -> Result<()>;
    pub async fn serve_sse(self, addr: SocketAddr) -> Result<()>;
}
```

## 4. Migration path

### Phase 1: Extract protocol crate (non-breaking)

1. Move shared types from `crates/mcp/src/` to `crates/mcp-protocol/src/`
2. Re-export from `codewhale-mcp` for backward compatibility
3. Update `Cargo.toml` in `codewhale-mcp` to depend on `codewhale-mcp-protocol`

### Phase 2: Split client and server (breaking for direct imports)

1. Create `crates/mcp-client/` with client code
2. Create `crates/mcp-server/` with server code
3. Update `codewhale-tui` to depend on `codewhale-mcp-client`
4. Update `codewhale-cli` to depend on `codewhale-mcp-server`
5. Deprecate `codewhale-mcp` crate (re-exports from new crates)

### Phase 3: Remove legacy crate

1. Remove `crates/mcp/` after a deprecation cycle (one release)

## 5. OAuth integration

### 5.1 Token storage

Tokens should be stored securely. Options (in priority order):
1. OS keychain via `keyring` crate (macOS Keychain, Windows Credential Manager,
   Linux Secret Service)
2. Encrypted file in `~/.codewhale/mcp-credentials/` (fallback)
3. Environment variable `MCP_OAUTH_TOKEN_<PROVIDER>`

### 5.2 OAuth flows

Initial implementation supports:
- **Device Code Flow** (GitHub) — user opens a URL, enters a code
- **Client Credentials** — for service-to-service MCP servers

Future (deferred):
- **PKCE** — for user-facing OAuth with redirect
- **Token refresh** — automatic refresh with refresh_token

### 5.3 Configuration

```toml
# ~/.codewhale/config.toml
[mcp.servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]

[mcp.servers.github.oauth]
provider = "github"
client_id = "your-client-id"
scopes = ["repo", "read:org"]
```

## 6. Risks and unknowns

| Risk | Mitigation |
|---|---|
| Crate proliferation | 3 small crates vs 1 medium crate; each has a clear purpose |
| Breaking internal imports | Phase 2 carries `codewhale-mcp` deprecation shim for one release |
| OAuth token security | OS keychain preferred; encrypted fallback with file permissions |
| Testing complexity | Each crate has its own test suite; integration tests remain in `crates/tui/tests/` |
| Dependency bloat | `oauth2` and `keyring` are optional features; consumers opt in |

## 7. Out of scope (future RFCs)

- MCP over HTTP/SSE transport (currently stdio only)
- MCP server discovery (currently explicit config)
- MCP tool result streaming (currently request-response)
- MCP server-side tool approval flows

## Related

- `crates/mcp/src/` — current implementation
- `crates/tui/src/mcp.rs` — TUI MCP integration
- `crates/tui/src/mcp_server.rs` — MCP stdio server
- `docs/MCP.md` — user-facing documentation
- Issue #2190 — this RFC
