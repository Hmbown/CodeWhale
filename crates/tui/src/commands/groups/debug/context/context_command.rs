//! Context command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Context;
impl Command for Context {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "context",
            aliases: &["ctx"],
            usage: "/context",
            description_id: MessageId::CmdContextDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        super::context_impl::context(app)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Context.info();
        assert_eq!(info.name, "context");
        assert!(!info.usage.is_empty());
    }
}
