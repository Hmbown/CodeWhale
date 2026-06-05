//! Queue command.
//!
//! This module separates the command handler from the implementation.

pub mod queue_command;
pub mod queue_impl;
pub use queue_command::Queue;
pub use queue_impl::queue;
