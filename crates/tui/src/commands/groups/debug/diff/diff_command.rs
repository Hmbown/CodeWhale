//! Diff command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Diff;
impl Command for Diff {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "diff",
            aliases: &[],
            usage: "/diff",
            description_id: MessageId::CmdDiffDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        super::diff_impl::diff(app)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Diff.info();
        assert_eq!(info.name, "diff");
        assert!(!info.usage.is_empty());
    }
}
