//! # CodeWhale Runtime Extension API
//!
//! A generic runtime extension system for CodeWhale that mirrors the
//! Pi Coding Agent's `ExtensionAPI` pattern. Extensions can register
//! lifecycle hooks (session start, tool call, turn end, etc.), define
//! MCP servers, slash commands, keyboard shortcuts, and custom tools.
//!
//! ## Architecture
//!
//! - [`Extension`] trait — implement this to create an extension
//! - [`ExtensionManager`] — registers extensions and dispatches events
//! - [`HookEvent`] — all lifecycle events extensions can hook into
//! - [`HookResult`] — result from a hook that may modify the runtime
//!
//! ## Usage
//!
//! ```ignore
//! use codewhale_extension::prelude::*;
//!
//! struct MyExtension;
//! #[async_trait]
//! impl Extension for MyExtension {
//!     fn name(&self) -> &str { "my-ext" }
//!
//!     async fn on_event(&self, event: &HookEvent) -> HookResult {
//!         match event {
//!             HookEvent::SessionStart { session_id, .. } => {
//!                 println!("Session started: {session_id}");
//!             }
//!             _ => {}
//!         }
//!         HookResult::default()
//!     }
//!
//!     fn mcp_servers(&self) -> Vec<(String, McpServerDef)> {
//!         vec![("my-server".into(), McpServerDef {
//!             command: "my-mcp-server".into(),
//!             args: vec![],
//!             env: vec![],
//!         })]
//!     }
//! }
//! ```

pub mod extension;
pub mod prelude;

// Re-export key types at crate root
pub use extension::{
    Extension, ExtensionManager, HookEvent, HookResult,
    McpServerDef, ShortcutDef, SlashCommandDef, ToolDef,
};
