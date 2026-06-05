//! Review command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Review;
impl Command for Review {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "review",
            aliases: &["shencha"],
            usage: "/review <target>",
            description_id: MessageId::CmdReviewDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::review::review(app, args)
    }
}
