use crate::commands::CommandResult;
use crate::tui::app::App;
use crate::tui::app::AppAction;

pub fn status_line(_app: &mut App) -> CommandResult {
    CommandResult::action(AppAction::OpenStatusPicker)
}
