//! Clear command.

use crate::tui::app::App;

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;

pub struct Clear;
impl Command for Clear {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "clear",
            aliases: &["qingping"],
            usage: "/clear",
            description_id: MessageId::CmdClearDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        super::clear_impl::clear(app)
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
        let cmd = Clear;
        let info = cmd.info();
        assert_eq!(info.name, "clear");
        assert!(info.aliases.contains(&"qingping"));
        assert_eq!(info.usage, "/clear");
    }

    #[test]
    fn execute_succeeds() {
        let mut app = test_app();
        let result = Clear.execute(&mut app, None);
        assert!(!result.is_error, "{:?}", result.message);
    }
}
