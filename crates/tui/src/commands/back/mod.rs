//! Implementation backend modules for slash commands.
//!
//! This module exists solely to keep `commands/mod.rs` clean — it contains
//! zero dispatch logic, only the module declarations for the implementation
//! files that the command groups call into. Groups access these via
//! `super::back::core::help()` etc.

pub(crate) mod anchor;
pub(crate) mod attachment;
pub(crate) mod balance;
pub(crate) mod change;
pub(crate) mod config;
pub(crate) mod core;
pub(crate) mod debug;
pub(crate) mod feedback;
pub(crate) mod goal;
pub(crate) mod hooks;
pub(crate) mod init;
pub(crate) mod jobs;
pub(crate) mod mcp;
pub(crate) mod memory;
pub(crate) mod network;
pub(crate) mod note;
pub(crate) mod provider;
pub(crate) mod queue;
pub(crate) mod rename;
pub(crate) mod restore;
pub(crate) mod review;
pub(crate) mod session;
pub(crate) mod skills;
pub(crate) mod stash;
pub(crate) mod status;
pub(crate) mod task;
