//! Project command area: workspace bootstrap, LSP wiring, sharing, and goals.

mod goal;
mod init;
mod lsp;
pub mod share;

use crate::commands::traits::{Command, CommandGroup, FunctionCommand, RegisterCommand};

pub struct ProjectCommands;

impl CommandGroup for ProjectCommands {
    fn commands(&self) -> &'static [Box<dyn Command>] {
        cached_command_list!(vec![
            Box::new(FunctionCommand::new(
                init::InitCmd::info(),
                init::InitCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                lsp::LspCmd::info(),
                lsp::LspCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                share::ShareCmd::info(),
                share::ShareCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                goal::GoalCmd::info(),
                goal::GoalCmd::execute,
            )),
        ])
    }
}
