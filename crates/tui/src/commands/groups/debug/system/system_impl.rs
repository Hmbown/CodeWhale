use crate::commands::CommandResult;
use crate::tui::app::App;

pub(crate) fn system_prompt(app: &mut App) -> CommandResult {
    crate::commands::groups::debug::debug_impl::system_prompt(app)
}
