//! `/translate` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "translate",
    aliases: &["translation", "transale"],
    usage: "/translate",
    description_id: MessageId::CmdTranslateDescription,
};

pub(in crate::commands) struct TranslateCmd;

impl RegisterCommand for TranslateCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        super::core::translate(app)
    }
}
