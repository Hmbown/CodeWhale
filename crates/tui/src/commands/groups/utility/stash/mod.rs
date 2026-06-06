//! Stash command.
//!
//! This module separates the command handler from the implementation.

pub mod stash_command;
pub mod stash_impl;
pub use stash_command::Stash;
pub use stash_impl::stash;
