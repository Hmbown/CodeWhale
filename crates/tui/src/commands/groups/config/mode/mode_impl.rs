use crate::commands::CommandResult;
use crate::tui::app::{App, AppAction};

pub fn mode(app: &mut App, arg: Option<&str>) -> CommandResult {
    let Some(arg) = arg.filter(|value| !value.trim().is_empty()) else {
        return CommandResult::action(AppAction::OpenModePicker);
    };
    match crate::commands::back::config::parse_mode_arg(arg) {
        Some(mode) => {
            let (message, changed) = crate::commands::back::config::switch_mode_with_status(app, mode);
            if changed {
                CommandResult::with_message_and_action(message, AppAction::ModeChanged(mode))
            } else {
                CommandResult::message(message)
            }
        }
        None => CommandResult::error("Usage: /mode [agent|plan|yolo|1|2|3]"),
    }
}