//! `/swarm` command - gated until durable Fleet-backed workers are available.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "swarm",
    aliases: &["fanout", "qun"],
    usage: "/swarm [N] <task>",
    description_id: MessageId::CmdSwarmDescription,
};

pub(in crate::commands) struct SwarmCmd;

impl RegisterCommand for SwarmCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        swarm(app, arg)
    }
}

/// Gate the old prompt-only swarm fanout until it can route through durable
/// WhaleFlow/Fleet workers (#3218).
pub fn swarm(_app: &mut App, arg: Option<&str>) -> CommandResult {
    let (_max_depth, task) = match super::util::parse_depth_prefixed_arg(arg, 1) {
        Ok(parsed) => parsed,
        Err(message) => return CommandResult::error(message),
    };
    if !matches!(task.map(str::trim), Some(task) if !task.is_empty()) {
        return CommandResult::error(
            "Usage: /swarm [N] <task>\n\n\
             /swarm is currently gated. Use /goal for a persistent objective \
             or /agent for a single sub-agent while durable Fleet-backed \
             swarm workers are still landing.",
        );
    }
    CommandResult::error(
        "/swarm is gated in v0.8.61: prompt-only agent fanout is disabled until the durable Train-3 worker/goal re-dispatch substrate lands. Use /goal for the persistent objective or /agent [N] <task> for one bounded sub-agent.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_app() -> App {
        let options = crate::tui::app::TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: std::path::PathBuf::from("/tmp/test-workspace"),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: std::path::PathBuf::from("/tmp/test-skills"),
            memory_path: std::path::PathBuf::from("memory.md"),
            notes_path: std::path::PathBuf::from("notes.txt"),
            mcp_config_path: std::path::PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            initial_input: None,
            resume_session_id: None,
            yolo: false,
        };
        App::new(options, &crate::config::Config::default())
    }

    #[test]
    fn swarm_is_gated_until_durable_worker_substrate_lands() {
        let mut app = create_test_app();
        let result = swarm(&mut app, Some("inspect five files"));

        assert!(result.is_error);
        assert!(result.action.is_none());
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("gated")
        );
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("Train-3")
        );
    }
}
