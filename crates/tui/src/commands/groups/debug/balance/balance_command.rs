//! Balance command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
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
        crate::commands::groups::debug::balance::balance(app)
    }
}

#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Balance.info();
        assert_eq!(info.name, "balance");
        assert!(!info.usage.is_empty());
    }
}
