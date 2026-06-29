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
            // Hide the Hotbar: persist `hotbar = []` and clear the live slots.
            Some("off" | "disable" | "hide") => CommandResult::action(AppAction::DisableHotbar),
            // Restore the default recommended slots (explicit reset).
            Some("on" | "reset" | "defaults" | "default") => {
                CommandResult::action(AppAction::RestoreHotbarDefaults)
            }
            Some("help" | "?") => CommandResult::message(
                "Hotbar gives you Alt+1 through Alt+8 shortcuts (Option key on macOS, Alt \
                 elsewhere). Use `/hotbar` to customize, `/hotbar off` to hide it \
                 (`hotbar = []`), and `/hotbar on` to restore the default slots.",
            ),
            Some(other) => CommandResult::error(format!(
                "Unknown /hotbar target '{other}'. Try `/hotbar`, `/hotbar off`, \
                 `/hotbar on`, or `/hotbar help`."
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
    fn hotbar_help_arg_explains_customize_and_disable() {
        let mut app = test_app();

        let result = HotbarCmd::execute(&mut app, Some("help"));

        assert!(!result.is_error);
        assert!(result.action.is_none());
        let message = result
            .message
            .as_deref()
            .expect("help should return a message");
        assert!(
            message.contains("/hotbar")
                && message.contains("customize")
                && message.contains("Alt+1 through Alt+8"),
            "help should point at /hotbar to customize: {message:?}"
        );
        assert!(
            message.contains("hotbar = []"),
            "help should mention the explicit disabled config: {message:?}"
        );
        assert!(
            message.contains("/hotbar off") && message.contains("/hotbar on"),
            "help should mention both disable and restore paths: {message:?}"
        );
    }

    #[test]
    fn hotbar_off_and_disable_aliases_return_disable_action() {
        for arg in ["off", "disable", "hide"] {
            let mut app = test_app();
            let result = HotbarCmd::execute(&mut app, Some(arg));
            assert_eq!(
                result.action,
                Some(AppAction::DisableHotbar),
                "`/hotbar {arg}` should disable the hotbar"
            );
            assert!(
                result.message.is_none(),
                "`/hotbar {arg}` should not also emit a message"
            );
        }
    }

    #[test]
    fn hotbar_on_and_reset_aliases_return_restore_action() {
        for arg in ["on", "reset", "defaults", "default"] {
            let mut app = test_app();
            let result = HotbarCmd::execute(&mut app, Some(arg));
            assert_eq!(
                result.action,
                Some(AppAction::RestoreHotbarDefaults),
                "`/hotbar {arg}` should restore default hotbar slots"
            );
            assert!(result.message.is_none());
        }
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
