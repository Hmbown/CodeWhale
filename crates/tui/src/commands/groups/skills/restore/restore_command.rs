//! Restore command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Restore;
impl Command for Restore {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "restore",
            aliases: &[],
            usage: "/restore [N]",
            description_id: MessageId::CmdRestoreDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::groups::skills::restore::restore(app, args)
    }
}

#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Restore.info();
        assert_eq!(info.name, "restore");
        assert!(!info.usage.is_empty());
    }
}
