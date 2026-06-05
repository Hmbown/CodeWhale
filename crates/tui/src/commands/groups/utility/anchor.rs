//! Anchor command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
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
        crate::commands::back::anchor::anchor(app, args)
    }
}
