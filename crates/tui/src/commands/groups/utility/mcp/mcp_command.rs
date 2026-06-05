//! Mcp command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Mcp;
impl Command for Mcp {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "mcp",
            aliases: &[],
            usage: "/mcp [list|restart|stop|start|add|remove]",
            description_id: MessageId::CmdMcpDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::groups::utility::mcp::mcp(app, args)
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
}
