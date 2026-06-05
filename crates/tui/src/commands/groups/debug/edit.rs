//! Edit command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Edit;
impl Command for Edit {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "edit",
            aliases: &[],
            usage: "/edit",
            description_id: MessageId::CmdEditDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::debug::edit(app)
    }
}
