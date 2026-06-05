//! RLM command.

use crate::tui::app::{App, AppAction};
use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;

use super::{parse_depth_prefixed_arg, resolves_to_existing_file};

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
        let (max_depth, target) = match parse_depth_prefixed_arg(arg, 1) {
            Ok(parsed) => parsed,
            Err(message) => return CommandResult::error(message),
        };
        let target = match target {
            Some(p) if !p.trim().is_empty() => p.trim().to_string(),
            _ => {
                return CommandResult::error(
                    "Usage: /rlm [N] <file_or_text>\n\n                     Opens a persistent RLM context with sub_rlm depth N (0-3, default 1).".to_string(),
                );
            }
        };
        let source_arg = if resolves_to_existing_file(app, &target) {
            format!("file_path: \"{target}\"")
        } else {
            format!("content: {target:?}")
        };
        let message = format!(
            "Open and use a persistent RLM session. Call `rlm_open` with name `slash_rlm` and {source_arg}. Call `rlm_configure` with `sub_rlm_max_depth: {max_depth}`."
        );
        CommandResult::with_message_and_action(
            format!("Opening persistent RLM context at depth {max_depth}..."),
            AppAction::SendMessage(message),
        )
    }
}
