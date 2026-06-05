//! Sessions command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Sessions;
impl Command for Sessions {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "sessions",
            aliases: &["resume"],
            usage: "/sessions",
            description_id: MessageId::CmdSessionsDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::session::sessions(app, args)
    }
}
