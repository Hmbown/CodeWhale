//! Config command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Config;
impl Command for Config {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "config",
            aliases: &[],
            usage: "/config [key] [value]",
            description_id: MessageId::CmdConfigDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::config::config_command(app, args)
    }
}
