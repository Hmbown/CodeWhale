//! Utility commands group — queue, stash, hooks, anchor, network, mcp, rlm,
//! task, jobs, slop

use crate::tui::app::{App, AppAction};

use super::traits::{Command, CommandGroup, CommandInfo};
use super::CommandResult;
use crate::localization::MessageId;

pub struct Queue;
impl Command for Queue {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "queue", aliases: &["queued"], usage: "/queue [list|edit <n>|drop <n>|clear]", description_id: MessageId::CmdQueueDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::back::queue::queue(app, args) }
}

pub struct Stash;
impl Command for Stash {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "stash", aliases: &["park"], usage: "/stash [list|pop|clear]", description_id: MessageId::CmdStashDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::back::stash::stash(app, args) }
}

pub struct Hooks;
impl Command for Hooks {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "hooks", aliases: &["hook", "gouzi"], usage: "/hooks [list|events]", description_id: MessageId::CmdHooksDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::back::hooks::hooks(app, args) }
}

pub struct Anchor;
impl Command for Anchor {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "anchor", aliases: &["maodian"], usage: "/anchor <text> | /anchor list | /anchor remove <n>", description_id: MessageId::CmdAnchorDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::back::anchor::anchor(app, args) }
}

pub struct Network;
impl Command for Network {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "network", aliases: &[], usage: "/network [allow|deny] <host>", description_id: MessageId::CmdNetworkDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::back::network::network(app, args) }
}

pub struct Mcp;
impl Command for Mcp {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "mcp", aliases: &[], usage: "/mcp [list|restart <name>|stop <name>|start <name>|add <name> <transport> <args>|remove <name>]", description_id: MessageId::CmdMcpDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::back::mcp::mcp(app, args) }
}

pub struct Rlm;
impl Command for Rlm {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "rlm", aliases: &["recursive", "digui"], usage: "/rlm [N] <file_or_text>", description_id: MessageId::CmdRlmDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { rlm(app, args) }
}

pub struct Task;
impl Command for Task {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "task", aliases: &["tasks"], usage: "/task [list|read <id>|revert <id>|cancel <id>]", description_id: MessageId::CmdTaskDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::back::task::task(app, args) }
}

pub struct Jobs;
impl Command for Jobs {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "jobs", aliases: &["job", "zuoye"], usage: "/jobs", description_id: MessageId::CmdJobsDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::back::jobs::jobs(app, args) }
}

pub struct Slop;
impl Command for Slop {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "slop", aliases: &["canzha"], usage: "/slop [query|export]", description_id: MessageId::CmdSlopDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::back::config::slop(app, args) }
}

pub struct UtilityCommands;
impl CommandGroup for UtilityCommands {

    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(Queue),
            Box::new(Stash),
            Box::new(Hooks),
            Box::new(Anchor),
            Box::new(Network),
            Box::new(Mcp),
            Box::new(Rlm),
            Box::new(Task),
            Box::new(Jobs),
            Box::new(Slop),
        ]
    }
}


// ── Helper functions ───────────────────────────────────────────────────────

fn parse_depth_prefixed_arg(
    arg: Option<&str>,
    default_depth: u32,
) -> Result<(u32, Option<&str>), String> {
    let Some(raw) = arg.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok((default_depth, None));
    };
    let mut parts = raw.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or_default();
    if first.chars().all(|ch| ch.is_ascii_digit()) {
        let depth: u32 = first
            .parse()
            .map_err(|_| "Depth must be an integer from 0 to 3".to_string())?;
        if depth > 3 {
            return Err("Depth must be between 0 and 3".to_string());
        }
        Ok((depth, parts.next().map(str::trim)))
    } else {
        Ok((default_depth, Some(raw)))
    }
}

fn resolves_to_existing_file(app: &App, input: &str) -> bool {
    let path = std::path::Path::new(input);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        app.workspace.join(path)
    };
    candidate.is_file()
}

pub fn rlm(app: &mut App, arg: Option<&str>) -> CommandResult {
    let (max_depth, target) = match parse_depth_prefixed_arg(arg, 1) {
        Ok(parsed) => parsed,
        Err(message) => return CommandResult::error(message),
    };
    let target = match target {
        Some(p) if !p.trim().is_empty() => p.trim().to_string(),
        _ => {
            return CommandResult::error(
                "Usage: /rlm [N] <file_or_text>\n\n\
                 Opens a persistent RLM context with sub_rlm depth N (0-3, default 1)."
                    .to_string(),
            );
        }
    };
    let source_arg = if resolves_to_existing_file(app, &target) {
        format!(r#"file_path: "{target}""#)
    } else {
        format!("content: {target:?}")
    };
    let message = format!(
        "Open and use a persistent RLM session for this request. Call `rlm_open` with name `slash_rlm` and {source_arg}. Then call `rlm_configure` with `sub_rlm_max_depth: {max_depth}`. Use `rlm_eval` to inspect the context through `peek`, `search`, and `chunk`, and call `finalize(...)` from the REPL when ready. If a `var_handle` is returned, use `handle_read` for bounded slices or projections before answering."
    );
    CommandResult::with_message_and_action(
        format!("Opening persistent RLM context at depth {max_depth}..."),
        AppAction::SendMessage(message),
    )
}
