//! Network command.
//!
//! This module separates the command handler from the implementation.

pub mod network_command;
pub mod network_impl;
pub use network_command::Network;
pub use network_impl::network;
