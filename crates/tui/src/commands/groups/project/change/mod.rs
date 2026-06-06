//! Change command.
//!
//! This module separates the command handler from the implementation.

pub mod change_command;
pub mod change_impl;
pub use change_command::Change;
pub use change_impl::change;
