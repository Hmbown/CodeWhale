//! Relay command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Relay;
impl Command for Relay {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "relay",
            aliases: &["batonpass", "\u{63E5}\u{529B}"],
            usage: "/relay [focus]",
            description_id: MessageId::CmdRelayDescription,
        }
    }

    fn execute(&self, app: &mut App, arg: Option<&str>) -> CommandResult {
        super::relay_impl::relay(app, arg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Relay.info();
        assert_eq!(info.name, "relay");
        assert_eq!(info.usage, "/relay [focus]");
        assert!(info.aliases.contains(&"batonpass"));
    }

    #[test]
    fn execute_sends_relay_instruction() {
        let mut app = crate::commands::groups::test_support::test_app();
        let result = Relay.execute(&mut app, Some("next refactor step"));
        assert!(!result.is_error);
        assert!(result.message.unwrap().contains("handoff.md"));
        let action = result.action.expect("expected send action");
        assert!(
            matches!(action, crate::tui::app::AppAction::SendMessage(message) if message.contains("Session relay") && message.contains("next refactor step"))
        );
    }
}
