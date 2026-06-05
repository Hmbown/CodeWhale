//! Hooks command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Hooks;
impl Command for Hooks {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "hooks",
            aliases: &["hook", "gouzi"],
            usage: "/hooks [list|events]",
            description_id: MessageId::CmdHooksDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::hooks::hooks(app, args)
    }
}
