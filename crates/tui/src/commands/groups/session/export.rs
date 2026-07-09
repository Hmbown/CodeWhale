//! `/export` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "export",
    aliases: &["daochu"],
    usage: "/export [turn|path]",
    description_id: MessageId::CmdExportDescription,
};

pub(in crate::commands) struct ExportCmd;

impl RegisterCommand for ExportCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        super::session::export(app, arg)
    }
}
