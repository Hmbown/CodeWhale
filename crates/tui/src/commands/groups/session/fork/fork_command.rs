//! Fork command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Fork;
impl Command for Fork {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "fork",
            aliases: &["branch"],
            usage: "/fork",
            description_id: MessageId::CmdForkDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        super::fork_impl::fork(app)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Fork.info();
        assert_eq!(info.name, "fork");
        assert!(!info.usage.is_empty());
    }
}
