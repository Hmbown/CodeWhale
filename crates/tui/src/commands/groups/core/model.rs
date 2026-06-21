//! `/model` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "model",
    aliases: &["moxing"],
    usage: "/model [name]",
    description_id: MessageId::CmdModelDescription,
};

pub(in crate::commands) struct ModelCmd;

impl RegisterCommand for ModelCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        super::core::model(app, arg)
    }
}
