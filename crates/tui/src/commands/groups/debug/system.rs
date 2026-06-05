//! System command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct System;
impl Command for System {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "system",
            aliases: &["xitong"],
            usage: "/system",
            description_id: MessageId::CmdSystemDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::shared::debug::system_prompt(app)
    }
}
