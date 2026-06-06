//! Rename command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Rename;
impl Command for Rename {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "rename",
            aliases: &["gaiming", "chongmingming"],
            usage: "/rename <title>",
            description_id: MessageId::CmdRenameDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::groups::session::rename::rename(app, args)
    }
}

#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Rename.info();
        assert_eq!(info.name, "rename");
        assert!(!info.usage.is_empty());
    }
}
