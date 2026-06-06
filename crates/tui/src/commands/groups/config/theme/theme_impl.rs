use crate::commands::CommandResult;
use crate::config_actions::set_config_value;
use crate::tui::app::App;
use crate::tui::app::AppAction;

pub fn theme(app: &mut App, arg: Option<&str>) -> CommandResult {
    match arg.map(str::trim).filter(|s| !s.is_empty()) {
        None => CommandResult::action(AppAction::OpenThemePicker),
        Some(name) => set_config_value(app, "theme", name, true),
    }
}
