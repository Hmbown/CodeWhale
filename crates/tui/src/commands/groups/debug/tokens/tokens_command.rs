//! Tokens command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
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
        super::tokens_impl::tokens(app)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Tokens.info();
        assert_eq!(info.name, "tokens");
        assert!(!info.usage.is_empty());
    }
}
