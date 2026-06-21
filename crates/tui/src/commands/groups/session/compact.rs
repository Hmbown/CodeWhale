//! `/compact` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "compact",
    aliases: &["yasuo"],
    usage: "/compact",
    description_id: MessageId::CmdCompactDescription,
};

pub(in crate::commands) struct CompactCmd;

impl RegisterCommand for CompactCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        super::session::compact(app)
    }
}
