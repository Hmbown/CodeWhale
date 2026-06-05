//! Agent command.

use crate::tui::app::{App, AppAction};
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

use super::parse_depth_prefixed_arg;

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
    fn execute(&self, _app: &mut App, arg: Option<&str>) -> CommandResult {
        let (max_depth, task) = match parse_depth_prefixed_arg(arg, 1) {
            Ok(parsed) => parsed,
            Err(message) => return CommandResult::error(message),
        };
        let task = match task {
            Some(task) if !task.trim().is_empty() => task.trim().to_string(),
            _ => {
                return CommandResult::error(
                    "Usage: /agent [N] <task>\n\n                     Opens a persistent sub-agent session with recursive agent depth N (0-3, default 1).",
                );
            }
        };
        let message = format!(
            "Open a persistent sub-agent session for this task. Call `agent_open` with name `slash_agent`, `prompt: {task:?}`, and `max_depth: {max_depth}`. Use `agent_eval` to wait for the next terminal/current projection and `handle_read` on the returned transcript_handle if you need more detail. Verify any claimed side effects before reporting success."
        );
        CommandResult::with_message_and_action(
            format!("Opening persistent sub-agent at depth {max_depth}..."),
            AppAction::SendMessage(message),
        )
    }
}
