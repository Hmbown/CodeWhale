//! Export command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
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
        crate::commands::shared::session::export(app, args)
    }
}
