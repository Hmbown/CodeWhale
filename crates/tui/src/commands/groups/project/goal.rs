//! Goal command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Goal;
impl Command for Goal {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "goal",
            aliases: &["hunt", "mubiao", "\u{72e9}\u{730e}"],
            usage: "/goal [start|show|close <reason>]",
            description_id: MessageId::CmdGoalDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::goal::hunt(app, args)
    }
}
