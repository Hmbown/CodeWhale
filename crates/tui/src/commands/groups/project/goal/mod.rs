//! Goal command.
//!
//! This module separates the command handler from the implementation.

pub mod goal_command;
pub mod goal_impl;
pub use goal_command::Goal;
pub use goal_impl::hunt;
