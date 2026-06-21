//! `/load` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "load",
    aliases: &["jiazai"],
    usage: "/load [path]",
    description_id: MessageId::CmdLoadDescription,
};

pub(in crate::commands) struct LoadCmd;

impl RegisterCommand for LoadCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        super::session::load(app, arg)
    }
}
