//! Save command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Save;
impl Command for Save {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "save",
            aliases: &[],
            usage: "/save [path]",
            description_id: MessageId::CmdSaveDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::shared::session::save(app, args)
    }
}
