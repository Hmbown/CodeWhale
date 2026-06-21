//! `/exit` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "exit",
    aliases: &["quit", "q", "tuichu"],
    usage: "/exit",
    description_id: MessageId::CmdExitDescription,
};

pub(in crate::commands) struct ExitCmd;

impl RegisterCommand for ExitCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(_app: &mut App, _arg: Option<&str>) -> CommandResult {
        super::core::exit()
    }
}
