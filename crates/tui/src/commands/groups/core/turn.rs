//! `/turn` command — inspect the current or latest completed turn.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "turn",
    aliases: &["turns"],
    usage: "/turn inspect",
    description_id: MessageId::CmdTurnInspectDescription,
};

pub(in crate::commands) struct TurnCmd;

impl RegisterCommand for TurnCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(_app: &mut App, arg: Option<&str>) -> CommandResult {
        let verb = arg.map(str::trim).unwrap_or("");
        if matches!(verb, "inspect" | "i" | "") {
            return CommandResult::action(AppAction::OpenTurnInspector);
        }
        CommandResult::error(format!(
            "Unknown /turn verb '{verb}'. Use /turn inspect to open the whole-turn inspector."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use std::path::PathBuf;

    fn test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-flash".to_string(),
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        };
        App::new(options, &Config::default())
    }

    #[test]
    fn bare_turn_inspect_opens_turn_inspector() {
        let mut app = test_app();
        let result = TurnCmd::execute(&mut app, None);
        assert!(!result.is_error);
        assert_eq!(result.action, Some(AppAction::OpenTurnInspector));
        assert!(result.message.is_none());
    }

    #[test]
    fn turn_inspect_verb_opens_turn_inspector() {
        let mut app = test_app();
        let result = TurnCmd::execute(&mut app, Some("inspect"));
        assert!(!result.is_error);
        assert_eq!(result.action, Some(AppAction::OpenTurnInspector));
    }

    #[test]
    fn unknown_turn_verb_returns_error() {
        let mut app = test_app();
        let result = TurnCmd::execute(&mut app, Some("bad"));
        assert!(result.is_error);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("/turn inspect"))
        );
        assert!(result.action.is_none());
    }
}
