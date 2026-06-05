//! Utility commands group — queue, stash, hooks, anchor, network, mcp, rlm,
//! task, jobs, slop

use crate::tui::app::App;

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
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::rlm(app, args) }
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
