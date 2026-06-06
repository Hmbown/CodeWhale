//! Mcp command.
//!
//! This module separates the command handler from the implementation.

pub mod mcp_command;
pub mod mcp_impl;
pub use mcp_command::Mcp;
pub use mcp_impl::mcp;
