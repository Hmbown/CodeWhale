//! Group-owned built-in command areas.
//!
//! Each group module owns the handler files for its command area and
//! exposes a `dispatch` slice that claims the command names it owns and
//! returns `None` for everything else. `commands::execute` chains the
//! group dispatchers in order, so a command name must be claimed by
//! exactly one group.

pub mod config;
pub mod core;
pub mod debug;
pub mod memory;
pub mod project;
pub mod session;
pub mod skills;
pub mod utility;
