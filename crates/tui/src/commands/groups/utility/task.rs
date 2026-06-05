//! Task command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
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
        crate::commands::back::task::task(app, args)
    }
}
