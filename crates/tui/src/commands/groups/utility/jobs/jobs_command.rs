//! Jobs command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
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
        crate::commands::groups::utility::jobs::jobs(app, args)
    }
}

#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Jobs.info();
        assert_eq!(info.name, "jobs");
        assert!(!info.usage.is_empty());
    }
}
