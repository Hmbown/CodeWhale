//! `/pin` command: toggle the host terminal window into the always-on-top
//! mini window (Windows). Keyboard-friendly twin of the right-click menu
//! entry, for mouse-less users.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "pin",
    aliases: &["mini", "window-pin"],
    usage: "/pin",
    description_id: MessageId::CmdPinDescription,
};

pub(in crate::commands) struct PinCmd;

impl RegisterCommand for PinCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        match arg.map(str::trim).filter(|value| !value.is_empty()) {
            Some("help" | "?" | "--help" | "-h") => {
                return CommandResult::message(format!("Usage: {}", COMMAND_INFO.usage));
            }
            Some(_) => return CommandResult::error(format!("Usage: {}", COMMAND_INFO.usage)),
            None => {}
        }
        let pinned = crate::tui::window_control::toggle_pin();
        app.needs_redraw = true;
        CommandResult::message(
            app.tr(if pinned {
                MessageId::WindowPinActive
            } else {
                MessageId::WindowPinReleased
            })
            .into_owned(),
        )
    }
}
