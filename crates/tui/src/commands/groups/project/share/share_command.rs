//! Share command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Share;
impl Command for Share {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "share",
            aliases: &[],
            usage: "/share [path]",
            description_id: MessageId::CmdShareDescription,
        }
    }

    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        super::share_impl::share(app, args)
    }
}

#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Share.info();
        assert_eq!(info.name, "share");
        assert!(!info.usage.is_empty());
    }
}
