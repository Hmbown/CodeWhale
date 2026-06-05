//! Home command.

use crate::tui::app::App;
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Home;
impl Command for Home {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "home",
            aliases: &["stats", "overview", "zhuye", "shouye"],
            usage: "/home",
            description_id: MessageId::CmdHomeDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::core::home_dashboard(app)
    }
}
