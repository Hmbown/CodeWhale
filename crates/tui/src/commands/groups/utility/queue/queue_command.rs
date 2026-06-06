//! Queue command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
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
        crate::commands::groups::utility::queue::queue(app, args)
    }
}

#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Queue.info();
        assert_eq!(info.name, "queue");
        assert!(!info.usage.is_empty());
    }
}
