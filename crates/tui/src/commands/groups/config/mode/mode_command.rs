//! Mode command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Mode;
impl Command for Mode {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "mode",
            aliases: &[],
            usage: "/mode [plan|yolo|agent]",
            description_id: MessageId::CmdModeDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::groups::config::mode::mode_impl::mode(app, args)
    }
}
