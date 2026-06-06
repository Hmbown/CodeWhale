//! Agent command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Agent;
impl Command for Agent {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "agent",
            aliases: &["daili"],
            usage: "/agent [N] <task>",
            description_id: MessageId::CmdAgentDescription,
        }
    }

    fn execute(&self, app: &mut App, arg: Option<&str>) -> CommandResult {
        super::agent_impl::agent(app, arg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Agent.info();
        assert_eq!(info.name, "agent");
        assert_eq!(info.usage, "/agent [N] <task>");
        assert!(info.aliases.contains(&"daili"));
    }

    #[test]
    fn execute_requires_task() {
        let mut app = crate::commands::groups::test_support::test_app();
        let result = Agent.execute(&mut app, None);
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("Usage: /agent"));
    }

    #[test]
    fn execute_sends_agent_open_instruction() {
        let mut app = crate::commands::groups::test_support::test_app();
        let result = Agent.execute(&mut app, Some("2 inspect the build"));
        assert!(!result.is_error);
        assert!(result.message.unwrap().contains("depth 2"));
        let action = result.action.expect("expected send action");
        assert!(
            matches!(action, crate::tui::app::AppAction::SendMessage(message) if message.contains("agent_open") && message.contains("inspect the build"))
        );
    }
}
