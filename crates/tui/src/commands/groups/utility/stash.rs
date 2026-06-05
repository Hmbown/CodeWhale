//! Stash command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Stash;
impl Command for Stash {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "stash",
            aliases: &["park"],
            usage: "/stash [list|pop|clear]",
            description_id: MessageId::CmdStashDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::stash::stash(app, args)
    }
}
