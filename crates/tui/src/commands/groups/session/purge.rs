//! `/purge` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "purge",
    aliases: &["qingchu"],
    usage: "/purge",
    description_id: MessageId::CmdPurgeDescription,
};

pub(in crate::commands) struct PurgeCmd;

impl RegisterCommand for PurgeCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        super::session::purge(app)
    }
}
