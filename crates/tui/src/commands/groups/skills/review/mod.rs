//! Review command.
//!
//! This module separates the command handler from the implementation.

pub mod review_command;
pub mod review_impl;
pub use review_command::Review;
pub use review_impl::review;
