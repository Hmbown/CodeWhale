//! Tokens command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Tokens;
impl Command for Tokens {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "tokens",
            aliases: &[],
            usage: "/tokens",
            description_id: MessageId::CmdTokensDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::shared::debug::tokens(app)
    }
}
