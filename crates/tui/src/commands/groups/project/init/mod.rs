//! Init command.
//!
//! This module separates the command handler from the implementation.

pub mod init_command;
pub mod init_impl;
pub use init_command::Init;
pub use init_impl::init;
