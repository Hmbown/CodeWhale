//! Statusline command.

use super::statusline_impl::status_line;
use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

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
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        status_line(app)
    }
}
