//! Status command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Status;
impl Command for Status {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "status",
            aliases: &[],
            usage: "/status",
            description_id: MessageId::CmdStatusDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::groups::config::status::status(app)
    }
}
