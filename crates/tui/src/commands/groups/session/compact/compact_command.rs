//! Compact command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Compact;
impl Command for Compact {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "compact",
            aliases: &["yasuo"],
            usage: "/compact",
            description_id: MessageId::CmdCompactDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        super::compact_impl::compact(app)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Compact.info();
        assert_eq!(info.name, "compact");
        assert!(!info.usage.is_empty());
    }
}
