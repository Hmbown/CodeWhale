//! Attach command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Attach;
impl Command for Attach {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "attach",
            aliases: &["image", "media", "fujian"],
            usage: "/attach <path|url> [description]",
            description_id: MessageId::CmdAttachDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::attachment::attach(app, args)
    }
}
