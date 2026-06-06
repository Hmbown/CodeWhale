//! Memory command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Memory;
impl Command for Memory {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "memory",
            aliases: &[],
            usage: "/memory [show|path|clear|edit|help]",
            description_id: MessageId::CmdMemoryDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::groups::memory::memory::memory(app, args)
    }
}

#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Memory.info();
        assert_eq!(info.name, "memory");
        assert!(!info.usage.is_empty());
    }
}
