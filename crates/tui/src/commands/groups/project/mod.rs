//! Project command area: workspace bootstrap, LSP wiring, sharing, and goals.

mod goal;
mod init;
pub mod share;

use crate::commands::CommandResult;
use crate::tui::app::App;

pub(in crate::commands) fn dispatch(
    app: &mut App,
    command: &str,
    arg: Option<&str>,
) -> Option<CommandResult> {
    let result = match command {
        "init" => init::init(app),
        "lsp" => super::config::config::lsp_command(app, arg),
        "share" => share::share(app, arg),
        "goal" | "hunt" | "mubiao" | "狩猎" => goal::hunt(app, arg),
        _ => return None,
    };
    Some(result)
}
