//! Models command.

use crate::tui::app::App;
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Models;
impl Command for Models {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "models",
            aliases: &["moxingliebiao"],
            usage: "/models",
            description_id: MessageId::CmdModelsDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::core::models(app)
    }
}
