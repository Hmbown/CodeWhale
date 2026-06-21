//! `/sessions` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "sessions",
    aliases: &["resume"],
    usage: "/sessions [show|prune <days>]",
    description_id: MessageId::CmdSessionsDescription,
};

pub(in crate::commands) struct SessionsCmd;

impl RegisterCommand for SessionsCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        super::session::sessions(app, arg)
    }
}
