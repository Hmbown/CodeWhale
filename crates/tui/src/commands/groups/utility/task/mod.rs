//! Task command.
//!
//! This module separates the command handler from the implementation.

pub mod task_command;
pub mod task_impl;
pub use task_command::Task;
pub use task_impl::task;
