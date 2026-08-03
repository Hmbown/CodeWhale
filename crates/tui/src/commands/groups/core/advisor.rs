//! `/advisor` command — toggle the background advisor watcher.
//!
//! Usage:
//!   `/advisor on`     — enable the advisor watcher for this session
//!   `/advisor off`    — disable it
//!   `/advisor status` — show whether it is currently enabled
//!   `/advisor`        — same as `status`
//!
//! The advisor runs as a fire-and-forget background task after each turn that
//! contains tool calls. It reads a bounded slice of recent tool calls, makes a
//! concise LLM advisory call, and emits a note into the status area.
//! Failures do not affect the parent turn. Off by default.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "advisor",
    aliases: &["watchers", "watch"],
    usage: "/advisor [on|off|status]",
    description_id: MessageId::CmdAdvisorDescription,
};

pub(in crate::commands) struct AdvisorCmd;

impl RegisterCommand for AdvisorCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(_app: &mut App, arg: Option<&str>) -> CommandResult {
        match arg.map(str::trim).filter(|s| !s.is_empty()) {
            Some("on") | Some("enable") | Some("yes") | Some("1") | Some("true") => {
                CommandResult::action(AppAction::SetAdvisorEnabled { enabled: true })
            }
            Some("off") | Some("disable") | Some("no") | Some("0") | Some("false") => {
                CommandResult::action(AppAction::SetAdvisorEnabled { enabled: false })
            }
            Some("status") | None => {
                // The engine owns the authoritative state; report from config.
                CommandResult::message(
                    "Advisor status: use `/advisor on` or `/advisor off` to toggle. \
                     Check `[advisor] enabled` in config.toml for the session default.",
                )
            }
            Some(unknown) => CommandResult::error(format!(
                "Unknown advisor argument: {unknown:?}. Use `on`, `off`, or `status`."
            )),
        }
    }
}
