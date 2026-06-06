use crate::commands::CommandResult;
use crate::tui::app::{App, AppAction};

/// Share the current session as a web URL.
pub(crate) fn share(app: &mut App, arg: Option<&str>) -> CommandResult {
    let raw = arg.map(str::trim).unwrap_or("");

    match raw {
        "" => do_share(app),
        "help" | "--help" | "-h" => CommandResult::message(
            "/share - Export the current session as a shareable web URL.\n\
             \n\
             Usage:\n\
             /share         Export and upload the current session\n\
             \n\
             The session transcript is rendered as static HTML and uploaded\n\
             to a GitHub Gist using the `gh` CLI. The Gist URL is displayed\n\
             so you can paste it into Slack, GitHub, Twitter, etc."
                .to_string(),
        ),
        _ => CommandResult::error(format!(
            "Unknown /share argument `{raw}`. Use `/share` with no arguments or `/share help`."
        )),
    }
}

fn do_share(app: &mut App) -> CommandResult {
    if app.history.is_empty() {
        return CommandResult::error("Nothing to share. The current session is empty.");
    }

    let history_len = app.history.len();
    let model = &app.model;
    let mode = app.mode.label();

    CommandResult::with_message_and_action(
        format!(
            "Exporting {history_len} cell(s) from {model} ({mode}) session...\n\n\
             The session will be rendered as static HTML and uploaded to a GitHub Gist.\n\
             This requires the `gh` CLI to be installed and authenticated."
        ),
        AppAction::ShareSession {
            history_len,
            model: model.clone(),
            mode: mode.to_string(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::groups::test_support::test_app;
    use crate::tui::history::HistoryCell;

    #[test]
    fn share_empty_session_returns_error() {
        let mut app = test_app();

        let result = share(&mut app, None);

        assert!(result.is_error);
        assert!(result.message.unwrap().contains("Nothing to share"));
        assert!(result.action.is_none());
    }

    #[test]
    fn share_help_returns_usage() {
        let mut app = test_app();

        let result = share(&mut app, Some("help"));

        let msg = result.message.expect("usage message");
        assert!(msg.contains("Usage:"));
        assert!(msg.contains("/share"));
        assert!(result.action.is_none());
    }

    #[test]
    fn share_with_history_returns_share_action() {
        let mut app = test_app();
        app.history.push(HistoryCell::User {
            content: "hello".to_string(),
        });

        let result = share(&mut app, None);

        assert!(result.message.is_some());
        assert!(matches!(
            result.action,
            Some(AppAction::ShareSession { history_len: 1, .. })
        ));
    }

    #[test]
    fn share_unknown_argument_returns_error() {
        let mut app = test_app();

        let result = share(&mut app, Some("bogus"));

        assert!(result.is_error);
        assert!(result.message.unwrap().contains("Unknown /share argument"));
    }
}
