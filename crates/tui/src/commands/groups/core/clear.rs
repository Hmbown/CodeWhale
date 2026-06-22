//! `/clear` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "clear",
    aliases: &["qingping"],
    usage: "/clear",
    description_id: MessageId::CmdClearDescription,
};

pub(in crate::commands) struct ClearCmd;

impl RegisterCommand for ClearCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        super::core::clear(app)
    }
}
