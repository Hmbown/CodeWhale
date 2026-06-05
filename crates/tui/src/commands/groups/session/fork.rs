//! Fork command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Fork;
impl Command for Fork {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "fork",
            aliases: &["branch"],
            usage: "/fork",
            description_id: MessageId::CmdForkDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::shared::session::fork(app)
    }
}
