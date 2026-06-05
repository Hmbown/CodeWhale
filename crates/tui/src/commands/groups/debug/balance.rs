//! Balance command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Balance;
impl Command for Balance {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "balance",
            aliases: &[],
            usage: "/balance",
            description_id: MessageId::CmdBalanceDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::balance::balance(app)
    }
}
