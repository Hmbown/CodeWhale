//! Rename command.
//!
//! This module separates the command handler from the implementation.

pub mod rename_command;
pub mod rename_impl;
pub use rename_command::Rename;
pub use rename_impl::rename;
