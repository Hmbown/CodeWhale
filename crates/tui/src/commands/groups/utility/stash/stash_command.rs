//! Stash command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Stash;
impl Command for Stash {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "stash",
            aliases: &["park"],
            usage: "/stash [list|pop|clear]",
            description_id: MessageId::CmdStashDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::groups::utility::stash::stash(app, args)
    }
}

#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Stash.info();
        assert_eq!(info.name, "stash");
        assert!(!info.usage.is_empty());
    }
}
