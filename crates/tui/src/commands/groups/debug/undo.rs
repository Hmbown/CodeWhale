//! Undo command.

use crate::tui::app::App;
use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Undo;
impl Command for Undo {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "undo",
            aliases: &[],
            usage: "/undo",
            description_id: MessageId::CmdUndoDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        let result = crate::commands::shared::debug::patch_undo(app);
        if result.message.as_deref().is_none_or(|m| {
            m.starts_with("No snapshots found")
                || m.starts_with("No tool or pre-turn")
                || m.starts_with("Snapshot repo")
        }) {
            crate::commands::shared::debug::undo_conversation(app)
        } else {
            result
        }
    }
}
