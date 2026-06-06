//! Attach command.
//!
//! This module separates the command handler from the implementation.

pub mod attach_command;
pub mod attach_impl;
pub use attach_command::Attach;
pub use attach_impl::attach;
