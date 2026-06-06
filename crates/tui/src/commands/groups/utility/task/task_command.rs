//! Task command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Task;
impl Command for Task {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "task",
            aliases: &["tasks"],
            usage: "/task [list|read|revert|cancel]",
            description_id: MessageId::CmdTaskDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::groups::utility::task::task(app, args)
    }
}

#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Task.info();
        assert_eq!(info.name, "task");
        assert!(!info.usage.is_empty());
    }
}
