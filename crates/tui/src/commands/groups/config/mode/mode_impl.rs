use crate::commands::CommandResult;
use crate::tui::app::{App, AppAction, AppMode};

pub fn mode(app: &mut App, arg: Option<&str>) -> CommandResult {
    let Some(arg) = arg.filter(|value| !value.trim().is_empty()) else {
        return CommandResult::action(AppAction::OpenModePicker);
    };
    match parse_mode_arg(arg) {
        Some(mode) => {
            let (message, changed) = switch_mode_with_status(app, mode);
            if changed {
                CommandResult::with_message_and_action(message, AppAction::ModeChanged(mode))
            } else {
                CommandResult::message(message)
            }
        }
        None => CommandResult::error("Usage: /mode [agent|plan|yolo|1|2|3]"),
    }
}

pub(crate) fn switch_mode(app: &mut App, mode: AppMode) -> String {
    switch_mode_with_status(app, mode).0
}

fn switch_mode_with_status(app: &mut App, mode: AppMode) -> (String, bool) {
    if app.set_mode(mode) {
        (
            format!("Switched to {} mode.", mode_display_name(mode)),
            true,
        )
    } else {
        (
            format!("Already in {} mode.", mode_display_name(mode)),
            false,
        )
    }
}

fn parse_mode_arg(arg: &str) -> Option<AppMode> {
    match arg.trim().to_ascii_lowercase().as_str() {
        "agent" | "1" => Some(AppMode::Agent),
        "plan" | "2" => Some(AppMode::Plan),
        "yolo" | "3" => Some(AppMode::Yolo),
        _ => None,
    }
}

fn mode_display_name(mode: AppMode) -> &'static str {
    match mode {
        AppMode::Agent => "Agent",
        AppMode::Plan => "Plan",
        AppMode::Yolo => "YOLO",
    }
}
