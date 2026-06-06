use crate::commands::CommandResult;
use crate::tui::app::App;

pub(crate) fn cache(app: &mut App, args: Option<&str>) -> CommandResult {
    crate::commands::groups::debug::debug_impl::cache(app, args)
}
