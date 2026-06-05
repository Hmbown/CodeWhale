//! Purge command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Purge;
impl Command for Purge {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "purge",
            aliases: &["qingchu"],
            usage: "/purge",
            description_id: MessageId::CmdPurgeDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::shared::session::purge(app)
    }
}
