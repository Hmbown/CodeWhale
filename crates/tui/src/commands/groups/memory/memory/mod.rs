//! Memory command.
//!
//! This module separates the command handler from the implementation.

pub mod memory_command;
pub mod memory_impl;
pub use memory_command::Memory;
pub use memory_impl::memory;
