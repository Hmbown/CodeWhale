//! `/save` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "save",
    aliases: &[],
    usage: "/save [path]",
    description_id: MessageId::CmdSaveDescription,
};

pub(in crate::commands) struct SaveCmd;

impl RegisterCommand for SaveCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        super::session::save(app, arg)
    }
}
