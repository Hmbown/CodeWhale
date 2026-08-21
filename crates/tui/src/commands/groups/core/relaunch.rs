//! `/relaunch` command — save the session exactly like `/exit`, then restart
//! the application resuming it.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "relaunch",
    aliases: &["restart"],
    usage: "/relaunch",
    description_id: MessageId::CmdRelaunchDescription,
};

pub(in crate::commands) struct RelaunchCmd;

impl RegisterCommand for RelaunchCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        relaunch(app)
    }
}

/// Decide whether `/relaunch` can hand the current session over. Pure so the
/// session-id contract is testable without a full `App`.
fn relaunch_decision(current_session_id: Option<&str>) -> Result<String, &'static str> {
    match current_session_id {
        Some(id) if !id.trim().is_empty() => Ok(id.to_string()),
        _ => Err(
            "There is no saved session to relaunch yet — complete a turn first, then run /relaunch.",
        ),
    }
}

fn relaunch(app: &mut App) -> CommandResult {
    if app.session_transition_blocked() {
        return CommandResult::error(
            "Cannot relaunch while runtime work is active. Wait for the turn to finish, or cancel it first.",
        );
    }
    match relaunch_decision(app.current_session_id.as_deref()) {
        Ok(session_id) => {
            // Reuse the `/exit` teardown path: quitting flushes and saves the
            // session exactly like /exit, and the relaunch handoff at the end
            // of `run_tui` replaces the process image afterwards.
            app.pending_relaunch = Some(session_id);
            CommandResult::action(AppAction::Quit)
        }
        Err(message) => CommandResult::error(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::TuiOptions;
    use std::path::PathBuf;

    fn app() -> App {
        App::new(
            TuiOptions {
                use_alt_screen: false,
                max_subagents: 2,
                ..crate::test_support::test_tui_options(PathBuf::from("."))
            },
            &Config::default(),
        )
    }

    #[test]
    fn relaunch_decision_accepts_the_current_session_id() {
        assert_eq!(
            relaunch_decision(Some("session-123")),
            Ok("session-123".to_string())
        );
    }

    #[test]
    fn relaunch_decision_rejects_a_missing_or_blank_session_id() {
        for missing in [None, Some(""), Some("   ")] {
            let message = relaunch_decision(missing).unwrap_err();
            assert!(message.contains("no saved session"), "{message}");
        }
    }

    #[test]
    fn relaunch_without_a_saved_session_reports_an_error_instead_of_quitting() {
        let mut app = app();
        assert!(app.current_session_id.is_none());
        let result = relaunch(&mut app);
        assert!(result.is_error);
        assert!(result.action.is_none());
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("no saved session")),
            "{:?}",
            result.message
        );
        assert!(app.pending_relaunch.is_none());
    }

    #[test]
    fn relaunch_with_a_saved_session_quits_and_hands_the_id_over() {
        let mut app = app();
        app.current_session_id = Some("session-456".to_string());
        let result = relaunch(&mut app);
        assert!(!result.is_error);
        assert!(matches!(result.action, Some(AppAction::Quit)));
        assert_eq!(app.pending_relaunch.as_deref(), Some("session-456"));
    }
}
