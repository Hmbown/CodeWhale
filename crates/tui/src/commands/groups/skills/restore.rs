//! Restore command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Restore;
impl Command for Restore {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "restore",
            aliases: &[],
            usage: "/restore [N]",
            description_id: MessageId::CmdRestoreDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::restore::restore(app, args)
    }
}
