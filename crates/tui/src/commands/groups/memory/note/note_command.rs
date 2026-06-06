//! Note command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Note;
impl Command for Note {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "note",
            aliases: &[],
            usage: "/note <text>",
            description_id: MessageId::CmdNoteDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::groups::memory::note::note(app, args)
    }
}

#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Note.info();
        assert_eq!(info.name, "note");
        assert!(!info.usage.is_empty());
    }
}
