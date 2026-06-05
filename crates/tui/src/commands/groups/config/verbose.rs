//! Verbose command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Verbose;
impl Command for Verbose {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "verbose",
            aliases: &[],
            usage: "/verbose [on|off]",
            description_id: MessageId::CmdVerboseDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::config::verbose(app, args)
    }
}
