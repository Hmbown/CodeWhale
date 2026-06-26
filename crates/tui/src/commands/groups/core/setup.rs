//! `/setup` command — opens the setup summary wizard.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "setup",
    aliases: &["config-summary", "status"],
    usage: "/setup",
    description_id: MessageId::CmdSetupDescription,
};

pub(in crate::commands) struct SetupCmd;

impl RegisterCommand for SetupCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(_app: &mut App, arg: Option<&str>) -> CommandResult {
        match arg.map(str::trim).filter(|arg| !arg.is_empty()) {
            None | Some("summary" | "wizard") => {
                CommandResult::action(AppAction::OpenSetupSummary)
            }
            Some("help" | "?") => CommandResult::message(
                "Usage: /setup [summary]\n\n/setup opens the configuration summary wizard.",
            ),
            Some(other) => CommandResult::error(format!(
                "Unknown /setup target '{other}'. Use `/setup` or `/setup summary`."
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
    fn setup_command_opens_summary() {
        let mut app = test_app();
        let result = SetupCmd::execute(&mut app, None);
        assert_eq!(result.action, Some(AppAction::OpenSetupSummary));
    }

    #[test]
    fn setup_summary_alias() {
        let mut app = test_app();
        let result = SetupCmd::execute(&mut app, Some("summary"));
        assert_eq!(result.action, Some(AppAction::OpenSetupSummary));
    }

    #[test]
    fn setup_help_arg() {
        let mut app = test_app();
        let result = SetupCmd::execute(&mut app, Some("help"));
        assert!(!result.is_error);
        assert!(result.action.is_none());
    }

    #[test]
    fn setup_unknown_arg() {
        let mut app = test_app();
        let result = SetupCmd::execute(&mut app, Some("bogus"));
        assert!(result.is_error);
    }
}
