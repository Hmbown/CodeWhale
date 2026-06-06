//! Review command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
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
        crate::commands::groups::skills::review::review(app, args)
    }
}

#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Review.info();
        assert_eq!(info.name, "review");
        assert!(!info.usage.is_empty());
    }
}
