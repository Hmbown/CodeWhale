//! Command group modules.
//!
//! Each group module registers its commands into the registry via the
//! `CommandGroup` trait. `commands/mod.rs` only calls `all_command_groups()`
//! — it never names individual groups.
//!
//! Adding a new group:
//!   1. Create `groups/my_group/` directory with `mod.rs` barrel + command files
//!   2. Add `mod my_group;` below
//!   3. Add `&my_group::MyGroupCommands` to the `all_command_groups()` vec

pub(crate) mod config;
pub(crate) mod core;
pub(crate) mod debug;
pub(crate) mod memory;
pub(crate) mod project;
pub(crate) mod session;
pub(crate) mod skills;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod utility;

use crate::commands::traits::CommandGroup;

/// Returns all registered command groups.
///
/// This is the single source of truth for which groups exist. Callers
/// iterate this list without knowing which groups are present.
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
