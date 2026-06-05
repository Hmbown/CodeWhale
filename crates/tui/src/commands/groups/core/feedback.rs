//! Feedback command.

use crate::tui::app::App;
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Feedback;
impl Command for Feedback {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "feedback",
            aliases: &[],
            usage: "/feedback [bug|feature|security]",
            description_id: MessageId::CmdFeedbackDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::feedback::feedback(app, args)
    }
}
