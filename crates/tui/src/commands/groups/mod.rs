//! Command group modules.
//!
//! Each group module registers its commands into the registry via the
//! `CommandGroup` trait. `commands/mod.rs` only knows about this barrel
//! module — individual groups are never referenced there.

pub(crate) mod core_group;
pub(crate) mod session_group;
pub(crate) mod config_group;
pub(crate) mod debug_group;
pub(crate) mod project_group;
pub(crate) mod skills_group;
pub(crate) mod memory_group;
pub(crate) mod utility_group;
