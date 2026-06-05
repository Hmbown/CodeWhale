//! Profile command.

use crate::tui::app::App;
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Profile;
impl Command for Profile {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "profile",
            aliases: &["dangan"],
            usage: "/profile <name>",
            description_id: MessageId::CmdHelpDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::core::profile_switch(app, args)
    }
}
