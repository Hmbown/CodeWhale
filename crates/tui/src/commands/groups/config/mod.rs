//! Config command area: settings, modes, themes, trust, and status surfaces.

#[allow(clippy::module_inception)]
pub mod config;
mod status;

use crate::commands::CommandResult;
use crate::tui::app::App;

pub(in crate::commands) fn dispatch(
    app: &mut App,
    command: &str,
    arg: Option<&str>,
) -> Option<CommandResult> {
    let result = match command {
        "config" => config::config_command(app, arg),
        "sidebar" => config::sidebar(app, arg),
        "settings" => config::show_settings(app),
        "status" => status::status(app),
        "statusline" => config::status_line(app),
        "mode" => config::mode(app, arg),
        "jihua" => config::mode(app, Some("plan")),
        "zidong" => config::mode(app, Some("yolo")),
        "theme" => config::theme(app, arg),
        "verbose" => config::verbose(app, arg),
        "trust" | "xinren" => config::trust(app, arg),
        "logout" => config::logout(app),
        "slop" | "canzha" => config::slop(app, arg),
        _ => return None,
    };
    Some(result)
}
