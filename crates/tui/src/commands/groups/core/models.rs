//! Models command.

use crate::tui::app::App;

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Models;
impl Command for Models {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "models",
            aliases: &["moxingliebiao"],
            usage: "/models",
            description_id: MessageId::CmdModelsDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::shared::core::models(app)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, AppAction, TuiOptions};
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
        let cmd = Models;
        let info = cmd.info();
        assert_eq!(info.name, "models");
        assert!(info.aliases.contains(&"moxingliebiao"));
    }

    #[test]
    fn execute_returns_fetch_action() {
        let mut app = test_app();
        let result = Models.execute(&mut app, None);
        assert!(!result.is_error);
        assert!(matches!(result.action, Some(AppAction::FetchModels)),
            "expected FetchModels, got {:?}", result.action);
    }
}
