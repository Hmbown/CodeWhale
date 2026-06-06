//! Workspace command.

use crate::tui::app::App;

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;

pub struct Workspace;
impl Command for Workspace {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "workspace",
            aliases: &["cwd"],
            usage: "/workspace [path]",
            description_id: MessageId::CmdWorkspaceDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        super::workspace_impl::workspace_switch(app, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use std::path::PathBuf;
    use tempfile::tempdir;

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
        let cmd = Workspace;
        let info = cmd.info();
        assert_eq!(info.name, "workspace");
        assert!(info.aliases.contains(&"cwd"));
    }

    #[test]
    fn execute_without_args_shows_current() {
        let mut app = test_app();
        let result = Workspace.execute(&mut app, None);
        assert!(!result.is_error, "{:?}", result.message);
        let msg = result.message.as_deref().unwrap_or("");
        assert!(msg.contains("workspace"), "workspace msg: {msg}");
    }

    #[test]
    fn execute_with_valid_path_switches() {
        let dir = tempdir().expect("temp dir");
        let mut app = test_app();
        let ws_arg = dir.path().to_str().expect("utf8");
        let result = Workspace.execute(&mut app, Some(ws_arg));
        assert!(
            !result.is_error,
            "workspace switch failed: {:?}",
            result.message
        );
        let Some(crate::tui::app::AppAction::SwitchWorkspace { workspace: new_ws }) =
            &result.action
        else {
            panic!("expected SwitchWorkspace, got {:?}", result.action);
        };
        assert!(new_ws.exists(), "workspace path should exist: {new_ws:?}");
    }

    #[test]
    fn execute_with_nonexistent_path_returns_error() {
        let mut app = test_app();
        let result = Workspace.execute(&mut app, Some("/nonexistent/path/that/does/not/exist"));
        assert!(result.is_error, "expected error for nonexistent path");
    }
}
