//! Slop command.

use super::slop_impl::slop;
use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Slop;
impl Command for Slop {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "slop",
            aliases: &["canzha"],
            usage: "/slop [query|export]",
            description_id: MessageId::CmdSlopDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        slop(app, args)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Slop.info();
        assert_eq!(info.name, "slop");
        assert!(!info.usage.is_empty());
    }
}
