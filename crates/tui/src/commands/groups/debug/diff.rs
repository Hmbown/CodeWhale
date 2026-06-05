//! Diff command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
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
        crate::commands::shared::debug::diff(app)
    }
}
