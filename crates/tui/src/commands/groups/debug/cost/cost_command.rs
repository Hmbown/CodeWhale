//! Cost command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
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
        super::cost_impl::cost(app)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Cost.info();
        assert_eq!(info.name, "cost");
        assert!(!info.usage.is_empty());
    }
}
