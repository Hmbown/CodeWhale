//! Discoverable terminal-tab name command backed by the existing session name.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "title",
    aliases: &["tabtitle", "window-title"],
    usage: "/title <name>",
    description_id: MessageId::CmdTitleDescription,
};

pub(in crate::commands) struct TitleCmd;

impl RegisterCommand for TitleCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        let Some(title) = arg.map(str::trim).filter(|title| !title.is_empty()) else {
            return CommandResult::error("Usage: /title <name>");
        };
        super::rename::rename(app, Some(title))
    }
}
