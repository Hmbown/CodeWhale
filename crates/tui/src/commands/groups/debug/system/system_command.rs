//! System command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct System;
impl Command for System {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "system",
            aliases: &["xitong"],
            usage: "/system",
            description_id: MessageId::CmdSystemDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        super::system_impl::system_prompt(app)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = System.info();
        assert_eq!(info.name, "system");
        assert!(!info.usage.is_empty());
    }
}
