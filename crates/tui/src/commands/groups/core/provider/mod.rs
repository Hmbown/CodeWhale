//! Provider command.
//!
//! This module separates the command handler from the implementation.

pub mod provider_command;
pub mod provider_impl;
pub use provider_command::Provider;
pub use provider_impl::provider;
