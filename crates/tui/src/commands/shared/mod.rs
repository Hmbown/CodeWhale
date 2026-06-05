//! Implementation backend modules for slash commands.
//!
//! This module exists solely to keep `commands/mod.rs` clean — it contains
//! zero dispatch logic, only the module declarations for the implementation
//! files that the command groups call into. Groups access these via
//! `super::back::core::help()` etc.

pub(crate) mod config;
pub(crate) mod config_handlers;
pub(crate) mod core;
pub(crate) mod debug;
pub(crate) mod session;
pub(crate) mod skills;
