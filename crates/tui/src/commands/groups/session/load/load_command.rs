//! Load command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Load;
impl Command for Load {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "load",
            aliases: &["jiazai"],
            usage: "/load <file>",
            description_id: MessageId::CmdLoadDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        super::load_impl::load(app, args)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Load.info();
        assert_eq!(info.name, "load");
        assert!(!info.usage.is_empty());
    }
}
