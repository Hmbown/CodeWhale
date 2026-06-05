//! Queue command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Queue;
impl Command for Queue {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "queue",
            aliases: &["queued"],
            usage: "/queue [list|edit <n>|drop <n>|clear]",
            description_id: MessageId::CmdQueueDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::queue::queue(app, args)
    }
}
