//! `/home` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "home",
    aliases: &["stats", "overview", "zhuye", "shouye"],
    usage: "/home",
    description_id: MessageId::CmdHomeDescription,
};

pub(in crate::commands) struct HomeCmd;

impl RegisterCommand for HomeCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        super::core::home_dashboard(app)
    }
}
