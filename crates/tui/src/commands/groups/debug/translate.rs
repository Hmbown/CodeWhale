//! Translate command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Translate;
impl Command for Translate {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "translate",
            aliases: &["translation", "transale"],
            usage: "/translate",
            description_id: MessageId::CmdTranslateDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::shared::core::translate(app)
    }
}
