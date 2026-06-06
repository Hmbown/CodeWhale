//! Network command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Network;
impl Command for Network {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "network",
            aliases: &[],
            usage: "/network [allow|deny] <host>",
            description_id: MessageId::CmdNetworkDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::groups::utility::network::network(app, args)
    }
}

#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Network.info();
        assert_eq!(info.name, "network");
        assert!(!info.usage.is_empty());
    }
}
