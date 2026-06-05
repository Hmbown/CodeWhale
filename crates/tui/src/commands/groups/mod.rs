//! Command group modules.
//!
//! Each group module registers its commands into the registry via the
//! `CommandGroup` trait. `commands/mod.rs` only calls `all_command_groups()`
//! — it never names individual groups.
//!
//! Adding a new group:
//!   1. Create `groups/my_group.rs` with a struct implementing `CommandGroup`
//!   2. Add `pub(crate) mod my_group;` below
//!   3. Add `&my_group::MyGroupCommands` to the `all_command_groups()` vec

pub(crate) mod core_group;
pub(crate) mod session_group;
pub(crate) mod config_group;
pub(crate) mod debug_group;
pub(crate) mod project_group;
pub(crate) mod skills_group;
pub(crate) mod memory_group;
pub(crate) mod utility_group;

use crate::commands::traits::CommandGroup;

/// Returns all registered command groups.
///
/// This is the single source of truth for which groups exist. Callers
/// iterate this list without knowing which groups are present.
pub fn all_command_groups() -> Vec<&'static dyn CommandGroup> {
    vec![
        &core_group::CoreCommands,
        &session_group::SessionCommands,
        &config_group::ConfigCommands,
        &debug_group::DebugCommands,
        &project_group::ProjectCommands,
        &skills_group::SkillsCommands,
        &memory_group::MemoryCommands,
        &utility_group::UtilityCommands,
    ]
}
