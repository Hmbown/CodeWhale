//! Retry command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
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
        super::retry_impl::retry(app)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Retry.info();
        assert_eq!(info.name, "retry");
        assert!(!info.usage.is_empty());
    }
}
