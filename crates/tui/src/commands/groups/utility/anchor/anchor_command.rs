//! Anchor command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Anchor;
impl Command for Anchor {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "anchor",
            aliases: &["maodian"],
            usage: "/anchor <text>",
            description_id: MessageId::CmdAnchorDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::groups::utility::anchor::anchor(app, args)
    }
}

#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Anchor.info();
        assert_eq!(info.name, "anchor");
        assert!(!info.usage.is_empty());
    }
}
