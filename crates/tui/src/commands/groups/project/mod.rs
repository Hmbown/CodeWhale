//! Project command area: workspace bootstrap, LSP wiring, sharing, and goals.
//!
//! FEAT-021 Phase 6: every command registers through the portable contract
//! bridge (`ContextualCommand::from_contract`) with its exact capability set.
//! The transitional App shells are gone.

mod goal;
mod init;
mod lsp;
pub mod share;

use crate::commands::traits::{Command, CommandGroup, ContextualCommand};

pub struct ProjectCommands;

impl CommandGroup for ProjectCommands {
    fn commands(&self) -> &'static [Box<dyn Command>] {
        cached_command_list!(vec![
            Box::new(
                ContextualCommand::from_contract::<init::InitCmd>().expect("init registration"),
            ),
            Box::new(ContextualCommand::from_contract::<lsp::LspCmd>().expect("lsp registration"),),
            Box::new(
                ContextualCommand::from_contract::<share::ShareCmd>().expect("share registration"),
            ),
            Box::new(
                ContextualCommand::from_contract::<goal::GoalCmd>().expect("goal registration"),
            ),
        ])
    }
}
