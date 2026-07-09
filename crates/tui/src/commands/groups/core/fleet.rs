//! `/fleet` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "fleet",
    aliases: &["loadout", "party"],
    usage: "/fleet [roster|setup|status]",
    description_id: MessageId::CmdFleetDescription,
};

pub(in crate::commands) struct FleetCmd;

impl RegisterCommand for FleetCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        match arg.map(str::trim).filter(|arg| !arg.is_empty()) {
            None
            | Some("roster" | "party" | "loadout" | "roles" | "role" | "profiles" | "profile") => {
                CommandResult::action(AppAction::OpenFleetRoster)
            }
            Some("setup" | "edit" | "new") => CommandResult::action(AppAction::OpenFleetSetup),
            Some("status" | "workers" | "worker" | "agents" | "subagents" | "list") => {
                super::core::subagents(app)
            }
            Some("help" | "?") => CommandResult::message(
                "Usage: /fleet [roster|setup|status]\n\n/fleet (or /fleet roster) opens Fleet workers and orchestration state — each member's posture, routing, and origin. Use Operate mode when managing delegated work. /fleet setup opens the authoring wizard for a new or overriding profile. /fleet status shows live Fleet worker status; /subagents is a compatibility shortcut for the same status view.",
            ),
            Some(other) => CommandResult::error(format!(
                "Unknown /fleet target '{other}'. Use `/fleet roster`, `/fleet setup`, or `/fleet status`."
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
    fn fleet_command_opens_roster_view() {
        let mut app = test_app();

        let result = FleetCmd::execute(&mut app, None);

        assert_eq!(result.action, Some(AppAction::OpenFleetRoster));
        assert!(result.message.is_none());
    }

    #[test]
    fn fleet_roster_aliases_open_roster_view() {
        for arg in [
            "roster", "party", "loadout", "roles", "role", "profiles", "profile",
        ] {
            let mut app = test_app();

            let result = FleetCmd::execute(&mut app, Some(arg));

            assert_eq!(result.action, Some(AppAction::OpenFleetRoster), "{arg}");
            assert!(result.message.is_none(), "{arg}");
        }
    }

    #[test]
    fn fleet_setup_args_open_setup_wizard() {
        for arg in ["setup", "edit", "new"] {
            let mut app = test_app();

            let result = FleetCmd::execute(&mut app, Some(arg));

            assert_eq!(result.action, Some(AppAction::OpenFleetSetup), "{arg}");
            assert!(result.message.is_none(), "{arg}");
        }
    }

    #[test]
    fn fleet_status_arg_opens_worker_status_view() {
        for arg in ["status", "workers", "worker", "agents", "subagents", "list"] {
            let mut app = test_app();

            let result = FleetCmd::execute(&mut app, Some(arg));

            assert_eq!(result.action, Some(AppAction::ListSubAgents), "{arg}");
            assert!(result.message.is_none(), "{arg}");
        }
    }

    #[test]
    fn fleet_help_arg_returns_usage() {
        let mut app = test_app();

        let result = FleetCmd::execute(&mut app, Some("help"));

        assert!(!result.is_error);
        assert!(result.action.is_none());
        let message = result.message.as_deref().unwrap_or_default();
        for surface in ["/fleet roster", "/fleet setup", "/fleet status"] {
            assert!(message.contains(surface), "help must describe {surface}");
        }
    }

    #[test]
    fn fleet_unknown_arg_reports_error() {
        let mut app = test_app();

        let result = FleetCmd::execute(&mut app, Some("bogus"));

        assert!(result.is_error);
        assert!(result.action.is_none());
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("Unknown /fleet target 'bogus'"))
        );
    }

    #[test]
    fn fleet_aliases_are_registered_on_command_info() {
        assert!(FleetCmd::info().aliases.contains(&"loadout"));
    }
}
