use crate::commands::CommandResult;
use crate::tui::app::App;

pub(crate) fn undo(app: &mut App) -> CommandResult {
    let result = crate::commands::groups::debug::debug_impl::patch_undo(app);
    if result.message.as_deref().is_none_or(|message| {
        message.starts_with("No snapshots found")
            || message.starts_with("No tool or pre-turn")
            || message.starts_with("Snapshot repo")
    }) {
        crate::commands::groups::debug::debug_impl::undo_conversation(app)
    } else {
        result
    }
}
