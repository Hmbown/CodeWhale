//! Jobs command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Jobs;
impl Command for Jobs {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "jobs",
            aliases: &["job", "zuoye"],
            usage: "/jobs",
            description_id: MessageId::CmdJobsDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::jobs::jobs(app, args)
    }
}
