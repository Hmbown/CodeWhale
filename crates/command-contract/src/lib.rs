//! Prototype command boundary for the staged TUI command extraction.
//!
//! FEAT-014 defines shapes only. It does not implement them for `App`, change
//! production dispatch, move localization or shared TUI types, or move command
//! files. Later FEATs first adopt these shapes inside `codewhale-tui` one group
//! per PR; only after all groups are decoupled will they move to a commands
//! crate, again one group per PR.

pub mod facets;
pub mod handler;
pub mod metadata;
pub mod types;

pub use facets::*;
pub use handler::{CommandCapabilities, CommandContexts, CommandHandler, ContextParts};
pub use metadata::{CommandDiscovery, CommandInfo, RegisterCommand};
pub use types::*;

#[cfg(test)]
mod tests;
