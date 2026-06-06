//! Status command.
//!
//! This module separates the command handler from the implementation.

pub mod status_command;
pub mod status_impl;
pub use status_command::Status;
pub use status_impl::status;
