use crate::commands::CommandResult;
use crate::tui::app::App;

pub(crate) fn cost(app: &mut App) -> CommandResult {
    crate::commands::groups::debug::debug_impl::cost(app)
}
