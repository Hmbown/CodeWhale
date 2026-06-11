//! Memory command area: persistent memory and quick notes.

#[allow(clippy::module_inception)]
mod memory;
mod note;

use crate::commands::CommandResult;
use crate::tui::app::App;

pub(in crate::commands) fn dispatch(
    app: &mut App,
    command: &str,
    arg: Option<&str>,
) -> Option<CommandResult> {
    let result = match command {
        "memory" => memory::memory(app, arg),
        "note" => note::note(app, arg),
        _ => return None,
    };
    Some(result)
}
