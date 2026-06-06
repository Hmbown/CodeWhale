use crate::commands::CommandResult;
use crate::tui::app::App;

pub(crate) fn tokens(app: &mut App) -> CommandResult {
    crate::commands::groups::debug::debug_impl::tokens(app)
}
