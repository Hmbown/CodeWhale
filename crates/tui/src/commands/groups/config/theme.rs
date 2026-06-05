//! Theme command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Theme;
impl Command for Theme {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "theme",
            aliases: &[],
            usage: "/theme [name]",
            description_id: MessageId::CmdThemeDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::config::theme(app, args)
    }
}
