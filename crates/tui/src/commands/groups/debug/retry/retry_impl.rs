use crate::commands::CommandResult;
use crate::tui::app::App;

pub(crate) fn retry(app: &mut App) -> CommandResult {
    crate::commands::groups::debug::debug_impl::retry(app)
}
