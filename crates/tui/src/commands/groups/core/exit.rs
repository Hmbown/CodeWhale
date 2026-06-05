//! Exit command.

use crate::tui::app::App;
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Exit;
impl Command for Exit {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "exit",
            aliases: &["quit", "q", "tuichu"],
            usage: "/exit",
            description_id: MessageId::CmdExitDescription,
        }
    }
    fn execute(&self, _app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::core::exit()
    }
}
