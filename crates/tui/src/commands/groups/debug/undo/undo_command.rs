//! Undo command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

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
        super::undo_impl::undo(app)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Undo.info();
        assert_eq!(info.name, "undo");
        assert_eq!(info.usage, "/undo");
    }

    #[test]
    fn execute_without_history_returns_message() {
        let mut app = crate::commands::groups::test_support::test_app();
        let result = Undo.execute(&mut app, None);
        assert!(!result.is_error);
        assert!(result.message.is_some());
    }
}
