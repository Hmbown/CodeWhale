//! `/workspace` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "workspace",
    aliases: &["cwd"],
    usage: "/workspace [path]",
    description_id: MessageId::CmdWorkspaceDescription,
};

pub(in crate::commands) struct WorkspaceCmd;

impl RegisterCommand for WorkspaceCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        super::core::workspace_switch(app, arg)
    }
}
