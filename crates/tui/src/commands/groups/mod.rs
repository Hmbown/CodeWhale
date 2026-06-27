//! Group-owned built-in command areas.
//!
//! Each group module registers command objects into the central command
//! registry. Command implementation functions still live with their owning
//! groups, while dispatch, palette metadata, and help lookup all read from the
//! same registry surface.

pub mod config;
pub mod core;
pub mod debug;
pub mod memory;pub mod plugins;
pub mod project;
pub mod session;
pub mod skills;
pub mod utility;

use crate::commands::traits::CommandGroup;

pub fn all_command_groups() -> Vec<&'static dyn CommandGroup> {
    vec![
        &core::CoreCommands,
        &session::SessionCommands,
        &config::ConfigCommands,
        &debug::DebugCommands,
        &project::ProjectCommands,
        &skills::SkillsCommands,
        &memory::MemoryCommands,
        &utility::UtilityCommands,
    ]
}

