//! Statusline command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;
use super::statusline_impl::status_line;

pub struct Statusline;
impl Command for Statusline {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "statusline",
            aliases: &[],
            usage: "/statusline",
            description_id: MessageId::CmdStatuslineDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        status_line(app)
    }
}
