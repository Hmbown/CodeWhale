//! Model command.

use crate::tui::app::App;
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Model;
impl Command for Model {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "model",
            aliases: &["moxing"],
            usage: "/model [name]",
            description_id: MessageId::CmdModelDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::core::model(app, args)
    }
}
