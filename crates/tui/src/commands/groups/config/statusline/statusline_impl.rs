use crate::commands::CommandResult;
use crate::tui::app::AppAction;
use crate::tui::app::App;

pub fn status_line(_app: &mut App) -> CommandResult {
    CommandResult::action(AppAction::OpenStatusPicker)
}

