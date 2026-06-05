//! Rename command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Rename;
impl Command for Rename {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "rename",
            aliases: &["gaiming", "chongmingming"],
            usage: "/rename <title>",
            description_id: MessageId::CmdRenameDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::rename::rename(app, args)
    }
}
