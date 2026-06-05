//! Load command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Load;
impl Command for Load {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "load",
            aliases: &["jiazai"],
            usage: "/load <file>",
            description_id: MessageId::CmdLoadDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::session::load(app, args)
    }
}
