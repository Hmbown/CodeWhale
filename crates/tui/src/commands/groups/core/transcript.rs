//! `/transcript` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "transcript",
    aliases: &[],
    usage: "/transcript",
    // Reuse the keybinding description so the command palette and shortcut
    // catalog describe the same live-transcript surface in every locale.
    description_id: MessageId::KbLiveTranscript,
};

pub(in crate::commands) struct TranscriptCmd;

impl RegisterCommand for TranscriptCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(_app: &mut App, _arg: Option<&str>) -> CommandResult {
        CommandResult::action(AppAction::OpenLiveTranscript)
    }
}
