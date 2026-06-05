//! Init command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Init;
impl Command for Init {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "init",
            aliases: &[],
            usage: "/init",
            description_id: MessageId::CmdInitDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::init::init(app)
    }
}
