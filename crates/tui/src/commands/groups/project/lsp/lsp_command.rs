//! Lsp command.

use super::lsp_impl::lsp_command;
use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Lsp;
impl Command for Lsp {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "lsp",
            aliases: &[],
            usage: "/lsp <command>",
            description_id: MessageId::CmdLspDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        lsp_command(app, args)
    }
}
