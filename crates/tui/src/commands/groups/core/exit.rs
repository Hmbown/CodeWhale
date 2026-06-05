//! Exit command.


use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::tui::app::App;
use crate::localization::MessageId;

pub struct Exit;
impl Command for Exit {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "exit",
            aliases: &["quit", "q", "tuichu"],
            usage: "/exit",
            description_id: MessageId::CmdExitDescription,
        }
    }
    fn execute(&self, _app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::shared::core::exit()
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
        let cmd = Exit;
        let info = cmd.info();
        assert_eq!(info.name, "exit");
        assert!(info.aliases.contains(&"quit"));
        assert!(info.aliases.contains(&"q"));
        assert!(info.aliases.contains(&"tuichu"));
    }

    #[test]
    fn execute_returns_quit_action() {
        let mut app = test_app();
        let result = Exit.execute(&mut app, None);
        assert!(!result.is_error);
        assert!(matches!(result.action, Some(crate::tui::app::AppAction::Quit)),
            "expected Quit, got {:?}", result.action);
    }
}
