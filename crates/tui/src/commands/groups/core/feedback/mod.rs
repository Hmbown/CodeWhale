//! Feedback command.
//!
//! This module separates the command handler from the implementation.

pub mod feedback_command;
pub mod feedback_impl;
pub use feedback_command::Feedback;
pub use feedback_impl::feedback;
