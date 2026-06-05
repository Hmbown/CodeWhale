//! Memory command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Memory;
impl Command for Memory {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "memory",
            aliases: &[],
            usage: "/memory [show|path|clear|edit|help]",
            description_id: MessageId::CmdMemoryDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::memory::memory(app, args)
    }
}
