//! Context command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Context;
impl Command for Context {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "context",
            aliases: &["ctx"],
            usage: "/context",
            description_id: MessageId::CmdContextDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::shared::debug::context(app)
    }
}
