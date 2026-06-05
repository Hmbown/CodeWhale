//! Provider command.

use crate::tui::app::App;

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Provider;
impl Command for Provider {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "provider",
            aliases: &[],
            usage: "/provider [name] [model]",
            description_id: MessageId::CmdProviderDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::groups::core::provider::provider(app, args)
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
        let cmd = Provider;
        let info = cmd.info();
        assert_eq!(info.name, "provider");
        assert!(info.aliases.is_empty());
    }

    #[test]
    fn execute_without_args_shows_current() {
        let mut app = test_app();
        let result = Provider.execute(&mut app, None);
        assert!(!result.is_error, "{:?}", result.message);
    }

    #[test]
    fn execute_with_unknown_provider_returns_error() {
        let mut app = test_app();
        let result = Provider.execute(&mut app, Some("nonexistent"));
        assert!(result.is_error);
    }
}
