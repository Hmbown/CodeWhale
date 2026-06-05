//! Logout command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Logout;
impl Command for Logout {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "logout",
            aliases: &[],
            usage: "/logout",
            description_id: MessageId::CmdLogoutDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::config::logout(app)
    }
}
