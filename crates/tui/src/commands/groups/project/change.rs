//! Change command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Change;
impl Command for Change {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "change",
            aliases: &[],
            usage: "/change <description>",
            description_id: MessageId::CmdChangeDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::change::change(app, args)
    }
}
