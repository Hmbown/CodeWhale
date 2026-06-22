//! `/subagents` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "subagents",
    aliases: &["agents", "zhinengti"],
    usage: "/subagents",
    description_id: MessageId::CmdSubagentsDescription,
};

pub(in crate::commands) struct SubagentsCmd;

impl RegisterCommand for SubagentsCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        super::core::subagents(app)
    }
}
