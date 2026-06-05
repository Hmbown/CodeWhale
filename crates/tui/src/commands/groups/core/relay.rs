//! Relay command.

use crate::tui::app::{App, AppAction};
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

use super::build_relay_instruction;

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
        let focus = arg.map(str::trim).filter(|value| !value.is_empty());
        let message = build_relay_instruction(app, focus);
        CommandResult::with_message_and_action(
            "Preparing session relay at .deepseek/handoff.md...",
            AppAction::SendMessage(message),
        )
    }
}
