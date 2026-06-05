//! Cost command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Cost;
impl Command for Cost {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "cost",
            aliases: &[],
            usage: "/cost",
            description_id: MessageId::CmdCostDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::debug::cost(app)
    }
}
