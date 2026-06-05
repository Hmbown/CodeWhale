//! Compact command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
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
        crate::commands::shared::session::compact(app)
    }
}
