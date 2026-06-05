//! Network command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
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
        crate::commands::back::network::network(app, args)
    }
}
