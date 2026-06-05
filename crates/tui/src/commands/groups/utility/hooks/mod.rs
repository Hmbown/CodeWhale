//! Hooks command.
//!
//! This module separates the command handler from the implementation.

pub mod hooks_command;
pub mod hooks_impl;
pub use hooks_command::Hooks;
pub use hooks_impl::hooks;
