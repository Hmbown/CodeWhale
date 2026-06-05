//! Retry command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Retry;
impl Command for Retry {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "retry",
            aliases: &["chongshi"],
            usage: "/retry",
            description_id: MessageId::CmdRetryDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::shared::debug::retry(app)
    }
}
