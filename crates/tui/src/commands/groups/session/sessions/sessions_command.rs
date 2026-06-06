//! Sessions command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Sessions;
impl Command for Sessions {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "sessions",
            aliases: &["resume"],
            usage: "/sessions",
            description_id: MessageId::CmdSessionsDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        super::sessions_impl::sessions(app, args)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Sessions.info();
        assert_eq!(info.name, "sessions");
        assert!(!info.usage.is_empty());
    }
}
