//! RLM command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Rlm;
impl Command for Rlm {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "rlm",
            aliases: &["recursive", "digui"],
            usage: "/rlm [N] <file_or_text>",
            description_id: MessageId::CmdRlmDescription,
        }
    }

    fn execute(&self, app: &mut App, arg: Option<&str>) -> CommandResult {
        super::rlm_impl::rlm(app, arg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Rlm.info();
        assert_eq!(info.name, "rlm");
        assert_eq!(info.usage, "/rlm [N] <file_or_text>");
        assert!(info.aliases.contains(&"recursive"));
    }

    #[test]
    fn execute_requires_target() {
        let mut app = crate::commands::groups::test_support::test_app();
        let result = Rlm.execute(&mut app, None);
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("Usage: /rlm"));
    }

    #[test]
    fn execute_sends_rlm_open_instruction() {
        let mut app = crate::commands::groups::test_support::test_app();
        let result = Rlm.execute(&mut app, Some("2 inspect this text"));
        assert!(!result.is_error);
        assert!(result.message.unwrap().contains("depth 2"));
        let action = result.action.expect("expected send action");
        assert!(
            matches!(action, crate::tui::app::AppAction::SendMessage(message) if message.contains("rlm_open") && message.contains("inspect this text"))
        );
    }
}
