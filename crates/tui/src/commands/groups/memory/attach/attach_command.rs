//! Attach command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
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
        crate::commands::groups::memory::attach::attach(app, args)
    }
}

#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Attach.info();
        assert_eq!(info.name, "attach");
        assert!(!info.usage.is_empty());
    }
}
