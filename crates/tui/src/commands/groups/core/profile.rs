//! `/profile` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "profile",
    aliases: &["dangan"],
    usage: "/profile <name>",
    description_id: MessageId::CmdProfileDescription,
};

pub(in crate::commands) struct ProfileCmd;

impl RegisterCommand for ProfileCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        super::core::profile_switch(app, arg)
    }
}
