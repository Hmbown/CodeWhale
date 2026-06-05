//! Note command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
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
        crate::commands::back::note::note(app, args)
    }
}
