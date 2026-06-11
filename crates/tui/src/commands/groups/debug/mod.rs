//! Debug command area: token/cost introspection, cache tooling, undo/retry,
//! and the change log.

mod balance;
mod change;
#[allow(clippy::module_inception)]
mod debug;

use crate::commands::CommandResult;
use crate::tui::app::App;

pub(in crate::commands) fn dispatch(
    app: &mut App,
    command: &str,
    arg: Option<&str>,
) -> Option<CommandResult> {
    let result = match command {
        "tokens" => debug::tokens(app),
        "cost" => debug::cost(app),
        "balance" => balance::balance(app),
        "cache" => debug::cache(app, arg),
        "change" => change::change(app, arg),
        "system" | "xitong" => debug::system_prompt(app),
        "context" | "ctx" => debug::context(app),
        "edit" => debug::edit(app),
        "diff" => debug::diff(app),
        "undo" => {
            // Try surgical patch-undo first; fall back to conversation undo
            // if no snapshots are available or if the snapshot undo couldn't
            // find anything useful.
            let result = debug::patch_undo(app);
            if result.message.as_deref().is_none_or(|m| {
                m.starts_with("No snapshots found")
                    || m.starts_with("No tool or pre-turn")
                    || m.starts_with("Snapshot repo")
            }) {
                debug::undo_conversation(app)
            } else {
                result
            }
        }
        "retry" | "chongshi" => debug::retry(app),
        _ => return None,
    };
    Some(result)
}
