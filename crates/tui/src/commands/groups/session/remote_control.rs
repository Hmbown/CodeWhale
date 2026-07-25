//! `/rc` account-owned web remote control.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::remote_control::RemoteControlAction;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "rc",
    aliases: &["remote-control"],
    usage: "/rc [status|stop]",
    description_id: MessageId::CmdRemoteControlDescription,
};

pub(in crate::commands) struct RemoteControlCmd;

impl RegisterCommand for RemoteControlCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        match arg.map(str::trim).filter(|value| !value.is_empty()) {
            None | Some("start") => {
                if app.is_loading {
                    return CommandResult::error(
                        "Finish or interrupt the current turn before handing this session to the web.",
                    );
                }
                CommandResult::with_message_and_action(
                    "Starting account-owned web remote control…",
                    AppAction::RemoteControl(RemoteControlAction::Start),
                )
            }
            Some("status") => CommandResult::message(app.remote_control.status_line()),
            Some("stop") => CommandResult::with_message_and_action(
                "Stopping web remote control…",
                AppAction::RemoteControl(RemoteControlAction::Stop),
            ),
            Some(_) => CommandResult::error("Usage: /rc [status|stop]"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::TuiOptions;
    use std::path::PathBuf;

    #[test]
    fn start_is_blocked_during_an_active_turn() {
        let options = TuiOptions {
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        };
        let mut app = crate::test_support::test_app_with_options(options);
        app.is_loading = true;
        let result = RemoteControlCmd::execute(&mut app, None);
        assert!(result.is_error);
        assert!(result.action.is_none());
    }
}
