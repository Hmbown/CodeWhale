//! Edit command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Edit;
impl Command for Edit {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "edit",
            aliases: &[],
            usage: "/edit",
            description_id: MessageId::CmdEditDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        super::edit_impl::edit(app)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Edit.info();
        assert_eq!(info.name, "edit");
        assert!(!info.usage.is_empty());
    }
}
