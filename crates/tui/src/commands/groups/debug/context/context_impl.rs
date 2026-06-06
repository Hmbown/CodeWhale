use crate::commands::CommandResult;
use crate::tui::app::App;

pub(crate) fn context(app: &mut App) -> CommandResult {
    crate::commands::groups::debug::debug_impl::context(app)
}
