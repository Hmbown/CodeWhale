//! Purge command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
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
        super::purge_impl::purge(app)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Purge.info();
        assert_eq!(info.name, "purge");
        assert!(!info.usage.is_empty());
    }
}
