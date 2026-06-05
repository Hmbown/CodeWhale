//! Model command.

use crate::tui::app::App;

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Model;
impl Command for Model {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "model",
            aliases: &["moxing"],
            usage: "/model [name]",
            description_id: MessageId::CmdModelDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::core::model(app, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use std::path::PathBuf;

    fn test_app() -> App {
        App::new(
            TuiOptions {
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
            },
            &Config::default(),
        )
    }

    #[test]
    fn info_returns_metadata() {
        let cmd = Model;
        let info = cmd.info();
        assert_eq!(info.name, "model");
        assert!(info.aliases.contains(&"moxing"));
    }

    #[test]
    fn execute_without_args_shows_current() {
        let mut app = test_app();
        let result = Model.execute(&mut app, None);
        assert!(!result.is_error, "{:?}", result.message);
    }


    #[test]
    fn execute_with_model_name_switches() {
        let mut app = test_app();
        let result = Model.execute(&mut app, Some("deepseek-v4-flash"));
        assert!(!result.is_error, "{:?}", result.message);
    }

    #[test]
    fn execute_with_full_model_spec_succeeds() {
        let mut app = test_app();
        let result = Model.execute(&mut app, Some("deepseek-v4-flash"));
        assert!(!result.is_error, "{:?}", result.message);
    }
}
