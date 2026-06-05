//! Clear command.

use crate::tui::app::App;
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Clear;
impl Command for Clear {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "clear",
            aliases: &["qingping"],
            usage: "/clear",
            description_id: MessageId::CmdClearDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::core::clear(app)
    }
}
