//! Verbose command.

use super::verbose_impl::verbose;
use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
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
        verbose(app, args)
    }
}
