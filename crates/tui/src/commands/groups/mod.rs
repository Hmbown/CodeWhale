//! Group-owned built-in command areas.
//!
//! Each group module registers command objects into the central command
//! registry. Command implementation functions still live with their owning
//! groups, while dispatch, palette metadata, and help lookup all read from the
//! same registry surface.

macro_rules! cached_command_list {
    ($commands:expr) => {{
        static COMMANDS: std::sync::OnceLock<Vec<Box<dyn crate::commands::traits::Command>>> =
            std::sync::OnceLock::new();
        COMMANDS.get_or_init(|| $commands).as_slice()
    }};
}

pub mod config;
pub mod core;
pub mod debug;
pub mod memory;
pub mod plugins;
pub mod project;
pub mod session;
pub mod skills;
pub mod utility;

use std::sync::OnceLock;

use crate::commands::traits::CommandGroup;

pub fn all_command_groups() -> &'static [&'static dyn CommandGroup] {
    static GROUPS: OnceLock<Vec<&'static dyn CommandGroup>> = OnceLock::new();
    GROUPS
        .get_or_init(|| {
            vec![
                &core::CoreCommands,
                &session::SessionCommands,
                &config::ConfigCommands,
                &debug::DebugCommands,
                &project::ProjectCommands,
                &skills::SkillsCommands,
                &memory::MemoryCommands,
                &plugins::PluginsCommands,
                &utility::UtilityCommands,
            ]
        })
        .as_slice()
}
