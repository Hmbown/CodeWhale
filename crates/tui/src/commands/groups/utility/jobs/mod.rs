//! Jobs command.
//!
//! This module separates the command handler from the implementation.

pub mod jobs_command;
pub mod jobs_impl;
pub use jobs_command::Jobs;
pub use jobs_impl::jobs;
