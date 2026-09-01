//! Core engine module for `DeepSeek` CLI.
//!
//! This module provides the event-driven architecture that separates
//! the UI from the AI interaction logic:
//!
//! - `engine`: The main engine that processes operations
//! - `events`: Events emitted by the engine to the UI
//! - `ops`: Operations submitted by the UI to the engine
//! - `session`: Session state management
//! - `turn`: Turn context and tracking

// Engine code runs inside the TUI alt-screen — see `runtime_log` for why
// raw stdio prints must not appear here. Use `tracing::*` instead.
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

pub mod authority;
// `crates/core` now owns the engine (issue #5261). The TUI's `engine`
// consumes the core boundary directly so headless and TUI share one
// `Op`-in / `EventMsg`-out API, one `ThreadId`/`SessionId` type, and one
// `Journal` shape. New code should import from `codewhale_core`.
pub mod engine;
pub(crate) use engine::tool_catalog::allowlist_is_native_file_and_shell_only;
pub mod events;
// The first production consumer of the staged runtime contract is the
// provider-neutral model boundary. Keep the remaining contract files staged
// until their own consumers land instead of compiling dead scaffolding.
#[path = "runtime_contract/model.rs"]
pub mod model_client;
pub mod ops;
// Phase A1 of the core/protocol extraction: exhaustive projections of the
// engine's `Op` / `Event` onto `codewhale_protocol::{op::Op, EventMsg}`.
// A new engine variant without a protocol twin fails to compile there.
pub mod protocol_parity;
pub mod session;
#[path = "runtime_contract/termination.rs"]
pub mod termination;
// Moved to `codewhale_core::tool_parser` (zero crate-internal dependencies);
// re-exported so `crate::core::tool_parser` keeps working.
pub use codewhale_core::tool_parser;
pub mod turn;

// Re-exports
