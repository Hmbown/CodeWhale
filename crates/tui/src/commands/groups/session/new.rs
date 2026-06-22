//! `/new` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "new",
    aliases: &[],
    usage: "/new [--force]",
    description_id: MessageId::CmdNewDescription,
};

pub(in crate::commands) struct NewCmd;

impl RegisterCommand for NewCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        super::session::new_session(app, arg)
    }
}
