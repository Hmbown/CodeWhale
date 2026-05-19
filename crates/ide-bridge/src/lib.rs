//! MCP-over-WebSocket IDE bridge consumer for DeepSeek TUI.
//!
//! The wire contract is the one Claude Code's IDE extensions publish to
//! `~/.claude/ide/<port>.lock`: a loopback WebSocket carrying JSON-RPC 2.0
//! framed MCP. The behavioural shape mirrors `opencode`'s editor-context
//! consumer (`packages/opencode/src/cli/cmd/tui/context/editor.ts`) so the
//! two TUIs negotiate identically with any third-party IDE that already
//! speaks the Claude Code lockfile protocol.

#![forbid(unsafe_code)]

mod client;
pub mod discovery;
mod protocol;

pub use client::{ConnectOptions, IdeBridgeClient};
pub use discovery::{BridgeAuth, BridgeTarget, DiscoverySource, LockfileEntry, discover};
pub use protocol::{
    Diagnostic, Position, Selection, SelectionChange, SelectionRange, WorkspaceFolder,
    WorkspaceFolders,
};

/// Primary port env var, set by the IDE bridge host. This is the
/// cross-product wire name shared with Claude Code.
pub const IDE_BRIDGE_PORT_ENV: &str = "CLAUDE_CODE_SSE_PORT";

/// DeepSeek-specific port override, used when callers prefer not to set
/// the Claude-namespaced env. Mirrors opencode's `OPENCODE_EDITOR_SSE_PORT`.
pub const IDE_BRIDGE_DEEPSEEK_PORT_ENV: &str = "DEEPSEEK_EDITOR_SSE_PORT";

/// Lockfile directory under `$HOME`.
pub const IDE_BRIDGE_LOCKFILE_DIR: &str = ".claude/ide";

/// Header used when an IDE bridge lockfile publishes an auth token.
pub const IDE_BRIDGE_AUTHORIZATION_HEADER: &str = "x-claude-code-ide-authorization";

/// Loopback host the IDE bridge always publishes on.
pub const DEFAULT_BRIDGE_HOST: &str = "127.0.0.1";

/// Latest MCP protocol version supported by the IDE bridge client.
///
/// Per the MCP spec, `initialize.params.protocolVersion` is the latest
/// version the client supports. The server can respond with the version
/// it wants to use for the session.
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("discovery: {0}")]
    Discovery(String),
    #[error("connect: {0}")]
    Connect(String),
    #[error("handshake: {0}")]
    Handshake(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("call: {0}")]
    Call(String),
    #[error("tool error: {0}")]
    ToolError(String),
    #[error("decode: {0}")]
    Decode(String),
    #[error("timeout after {0:?}")]
    Timeout(std::time::Duration),
    #[error("connection closed")]
    Closed,
}

pub type Result<T> = std::result::Result<T, Error>;
