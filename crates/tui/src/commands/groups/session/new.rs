//! New command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct New;
impl Command for New {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "new",
            aliases: &[],
            usage: "/new",
            description_id: MessageId::CmdNewDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::session::new_session(app, args)
    }
}
