//! Help command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Help;
impl Command for Help {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "help",
            aliases: &["?", "bangzhu", "\u{5e2e}\u{52a9}"],
            usage: "/help [command]",
            description_id: MessageId::CmdHelpDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::shared::core::help(app, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use std::path::PathBuf;

    fn test_app() -> App {
        App::new(TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None, config_profile: None,
            allow_shell: false, use_alt_screen: true,
            use_mouse_capture: false, use_bracketed_paste: true,
            max_subagents: 1, skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false, start_in_agent_mode: false,
            skip_onboarding: true, yolo: false,
            resume_session_id: None, initial_input: None,
        }, &Config::default())
    }

    #[test]
    fn info_returns_metadata() {
        let info = Help.info();
        assert_eq!(info.name, "help");
        assert!(info.aliases.contains(&"?"));
        assert!(info.aliases.contains(&"bangzhu"));
        assert_eq!(info.usage, "/help [command]");
    }

    #[test]
    fn execute_topic_help_succeeds() {
        let mut app = test_app();
        let result = Help.execute(&mut app, Some("model"));
        assert!(!result.is_error, "{:?}", result.message);
    }

    #[test]
    fn execute_nonexistent_topic_returns_error() {
        let mut app = test_app();
        let result = Help.execute(&mut app, Some("nonexistent"));
        assert!(result.is_error);
    }
}
