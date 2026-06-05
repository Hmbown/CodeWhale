//! Help command.

use crate::tui::app::App;
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Help;
impl Command for Help {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "help",
            aliases: &["?", "bangzhu", "帮助"],
            usage: "/help [command]",
            description_id: MessageId::CmdHelpDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::core::help(app, args)
    }
}
