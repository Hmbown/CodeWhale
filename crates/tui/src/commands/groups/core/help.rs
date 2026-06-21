//! `/help` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "help",
    aliases: &["?", "bangzhu", "帮助"],
    usage: "/help [command]",
    description_id: MessageId::CmdHelpDescription,
};

pub(in crate::commands) struct HelpCmd;

impl RegisterCommand for HelpCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        super::core::help(app, arg)
    }
}
