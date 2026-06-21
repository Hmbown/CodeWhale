//! `/fork` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "fork",
    aliases: &["branch"],
    usage: "/fork",
    description_id: MessageId::CmdForkDescription,
};

pub(in crate::commands) struct ForkCmd;

impl RegisterCommand for ForkCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        super::session::fork(app)
    }
}
