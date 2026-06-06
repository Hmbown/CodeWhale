//! New command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct New;
impl Command for New {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "new",
            aliases: &[],
            usage: "/new",
            description_id: MessageId::CmdNewDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        super::new_impl::new_session(app, args)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = New.info();
        assert_eq!(info.name, "new");
        assert!(!info.usage.is_empty());
    }
}
