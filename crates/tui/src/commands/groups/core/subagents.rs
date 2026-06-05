//! Subagents command.

use crate::tui::app::App;
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Subagents;
impl Command for Subagents {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "subagents",
            aliases: &["agents", "zhinengti"],
            usage: "/subagents",
            description_id: MessageId::CmdSubagentsDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::core::subagents(app)
    }
}
