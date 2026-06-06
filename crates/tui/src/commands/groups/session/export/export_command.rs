//! Export command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Export;
impl Command for Export {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "export",
            aliases: &["daochu"],
            usage: "/export [path]",
            description_id: MessageId::CmdExportDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        super::export_impl::export(app, args)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Export.info();
        assert_eq!(info.name, "export");
        assert!(!info.usage.is_empty());
    }
}
