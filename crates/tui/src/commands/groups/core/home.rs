//! Home command.

use crate::tui::app::App;

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Home;
impl Command for Home {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "home",
            aliases: &["stats", "overview", "zhuye", "shouye"],
            usage: "/home",
            description_id: MessageId::CmdHomeDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::core::home_dashboard(app)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use std::path::PathBuf;

    fn test_app() -> App {
        App::new(TuiOptions { model: "deepseek-v4-pro".to_string(), workspace: PathBuf::from("."), config_path: None, config_profile: None, allow_shell: false, use_alt_screen: true, use_mouse_capture: false, use_bracketed_paste: true, max_subagents: 1, skills_dir: PathBuf::from("."), memory_path: PathBuf::from("memory.md"), notes_path: PathBuf::from("notes.txt"), mcp_config_path: PathBuf::from("mcp.json"), use_memory: false, start_in_agent_mode: false, skip_onboarding: true, yolo: false, resume_session_id: None, initial_input: None, }, &Config::default())
    }

    #[test]
    fn info_returns_metadata() {
        let cmd = Home;
        let info = cmd.info();
        assert_eq!(info.name, "home");
        assert!(info.aliases.contains(&"stats"));
        assert!(info.aliases.contains(&"overview"));
    }

    #[test]
    fn execute_returns_dashboard_message() {
        let mut app = test_app();
        let result = Home.execute(&mut app, None);
        assert!(!result.is_error, "{:?}", result.message);
        let msg = result.message.as_deref().unwrap_or("");
        assert!(!msg.is_empty(), "home should have a message");
    }
}
