//! `/hotbar` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "hotbar",
    aliases: &["hotkeys"],
    usage: "/hotbar",
    description_id: MessageId::CmdHotbarDescription,
};

pub(in crate::commands) struct HotbarCmd;

impl RegisterCommand for HotbarCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(_app: &mut App, arg: Option<&str>) -> CommandResult {
        match arg.map(str::trim).filter(|arg| !arg.is_empty()) {
            None | Some("setup" | "edit" | "configure" | "config") => {
                CommandResult::action(AppAction::OpenHotbarSetup)
            }
            Some("help" | "?") => CommandResult::message(
                "Usage: /hotbar [setup]\n\n/hotbar opens the Hotbar setup wizard. Press Alt+1 through Alt+8 to run configured slots; use hotbar = [] to hide the default bar.",
            ),
            Some(other) => CommandResult::error(format!(
                "Unknown /hotbar target '{other}'. Use `/hotbar` or `/hotbar setup`."
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::TuiOptions;
    use std::path::PathBuf;

    fn test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
    }

    #[test]
    fn hotbar_command_opens_setup_view() {
        let mut app = test_app();

        let result = HotbarCmd::execute(&mut app, None);

        assert_eq!(result.action, Some(AppAction::OpenHotbarSetup));
        assert!(result.message.is_none());
    }

    #[test]
    fn hotbar_setup_alias_opens_setup_view() {
        let mut app = test_app();

        let result = HotbarCmd::execute(&mut app, Some("setup"));

        assert_eq!(result.action, Some(AppAction::OpenHotbarSetup));
        assert!(result.message.is_none());
    }

    #[test]
    fn hotbar_help_arg_returns_usage() {
        let mut app = test_app();

        let result = HotbarCmd::execute(&mut app, Some("help"));

        assert!(!result.is_error);
        assert!(result.action.is_none());
        assert!(result.message.as_deref().is_some_and(|message| {
            message.contains("/hotbar opens")
                && message.contains("Alt+1 through Alt+8")
                && message.contains("hotbar = []")
        }));
    }

    #[test]
    fn hotbar_unknown_arg_reports_error() {
        let mut app = test_app();

        let result = HotbarCmd::execute(&mut app, Some("bogus"));

        assert!(result.is_error);
        assert!(result.action.is_none());
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("Unknown /hotbar target 'bogus'"))
        );
    }
}
