//! `/rc` account-owned web remote control.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::remote_control::RemoteControlAction;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "rc",
    aliases: &["remote-control"],
    usage: "/rc [status|link|open|stop]",
    description_id: MessageId::CmdRemoteControlDescription,
};

/// Shown by `/rc link` and `/rc open` before the control plane has advertised
/// a session link (not connected yet, or an older control plane).
const NO_LINK_MESSAGE: &str =
    "Remote control has no live session link yet; run /rc to hand this session to the web first.";

pub(in crate::commands) struct RemoteControlCmd;

impl RegisterCommand for RemoteControlCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        match arg.map(str::trim).filter(|value| !value.is_empty()) {
            None | Some("start") => CommandResult::with_message_and_action(
                if app.is_loading || app.dispatch_in_flight {
                    "Connecting web remote control to the active turn…"
                } else {
                    "Starting account-owned web remote control…"
                },
                AppAction::RemoteControl(RemoteControlAction::Start),
            ),
            Some("status") => CommandResult::message(app.remote_control.status_line()),
            Some("link") => match app.remote_control.run_url() {
                Some(url) => {
                    let mut message = format!("Remote control session: {url}");
                    if let Some(computer_url) = app.remote_control.computer_url() {
                        message.push_str(&format!("\nManage this computer: {computer_url}"));
                    }
                    CommandResult::message(message)
                }
                None => CommandResult::error(NO_LINK_MESSAGE),
            },
            Some("open") => match app.remote_control.run_url() {
                Some(url) => {
                    let url = url.to_string();
                    match crate::utils::open_url(&url) {
                        Ok(()) => CommandResult::message(format!("Opening {url} in your browser…")),
                        Err(_) => CommandResult::error(format!(
                            "Could not launch a browser; open {url} manually."
                        )),
                    }
                }
                None => CommandResult::error(NO_LINK_MESSAGE),
            },
            Some("stop") => {
                // Stop is refused while a remote turn is active or while any
                // terminal/approval/integrity envelope is still awaiting the
                // server-confirmed cursor; releasing the session earlier could
                // strand account-side truth or create a second owner.
                if let Some(reason) = app.remote_control.stop_refusal() {
                    return CommandResult::error(reason);
                }
                CommandResult::with_message_and_action(
                    "Stopping web remote control…",
                    AppAction::RemoteControl(RemoteControlAction::Stop),
                )
            }
            Some(_) => CommandResult::error("Usage: /rc [status|link|open|stop]"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::TuiOptions;
    use std::path::PathBuf;

    #[test]
    fn start_during_an_active_turn_dispatches_the_typed_handoff() {
        let options = TuiOptions {
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        };
        let mut app = crate::test_support::test_app_with_options(options);
        app.is_loading = true;
        let result = RemoteControlCmd::execute(&mut app, None);
        assert!(!result.is_error);
        assert!(matches!(
            result.action,
            Some(AppAction::RemoteControl(RemoteControlAction::Start))
        ));
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("active turn"))
        );
    }

    #[test]
    fn link_and_open_report_no_link_before_the_session_is_live() {
        let options = TuiOptions {
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        };
        let mut app = crate::test_support::test_app_with_options(options);
        for arg in ["link", "open"] {
            let result = RemoteControlCmd::execute(&mut app, Some(arg));
            assert!(result.is_error, "{arg} must not pretend a link exists");
            assert!(result.action.is_none());
            assert!(
                result
                    .message
                    .as_deref()
                    .is_some_and(|message| message.contains("no live session link"))
            );
        }
    }

    #[test]
    fn stop_is_blocked_while_a_remote_turn_is_active() {
        let options = TuiOptions {
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        };
        let mut app = crate::test_support::test_app_with_options(options);
        app.remote_control
            .activate_prompt("run-1", "turn-1")
            .unwrap();

        let result = RemoteControlCmd::execute(&mut app, Some("stop"));

        assert!(result.is_error);
        assert!(result.action.is_none());
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("active remote turn"))
        );
    }
}
