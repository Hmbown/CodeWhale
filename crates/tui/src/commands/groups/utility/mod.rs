//! Utility command area: attachments, background tasks, jobs, MCP, and
//! network inspection.

mod attachment;
mod jobs;
mod mcp;
mod network;
mod task;

use crate::commands::CommandResult;use crate::commands::groups::plugins::plugin_command as plugin_cmd;
use crate::commands::traits::{Command, CommandGroup, CommandInfo, FunctionCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct UtilityCommands;

impl CommandGroup for UtilityCommands {
    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(FunctionCommand::new(&ATTACH_INFO, run_attach)),
            Box::new(FunctionCommand::new(&TASK_INFO, run_task)),
            Box::new(FunctionCommand::new(&JOBS_INFO, run_jobs)),
            Box::new(FunctionCommand::new(&MCP_INFO, run_mcp)),
            Box::new(FunctionCommand::new(&NETWORK_INFO, run_network)),
            Box::new(FunctionCommand::new(&PLUGINS_INFO, run_plugins)),            Box::new(FunctionCommand::new(&PLUGIN_INFO, run_plugin)),
        ]
    }
}

static ATTACH_INFO: CommandInfo = CommandInfo {
    name: "attach",
    aliases: &["image", "media", "fujian"],
    usage: "/attach <path>",
    description_id: MessageId::CmdAttachDescription,
};
static TASK_INFO: CommandInfo = CommandInfo {
    name: "task",
    aliases: &["tasks"],
    usage: "/task [add <prompt>|list|show <id>|cancel <id>]",
    description_id: MessageId::CmdTaskDescription,
};
static JOBS_INFO: CommandInfo = CommandInfo {
    name: "jobs",
    aliases: &["job", "zuoye"],
    usage: "/jobs [list|show <id>|poll <id>|wait <id>|stdin <id> <input>|cancel <id>]",
    description_id: MessageId::CmdJobsDescription,
};
static MCP_INFO: CommandInfo = CommandInfo {
    name: "mcp",
    aliases: &[],
    usage: "/mcp [init|add stdio <name> <command> [args...]|add http <name> <url>|enable <name>|disable <name>|remove <name>|validate|reload]",
    description_id: MessageId::CmdMcpDescription,
};
static NETWORK_INFO: CommandInfo = CommandInfo {
    name: "network",
    aliases: &[],
    usage: "/network [list|allow <host>|deny <host>|remove <host>|default <allow|deny|prompt>]",
    description_id: MessageId::CmdNetworkDescription,
};
static PLUGIN_INFO: CommandInfo = CommandInfo {
    name: "plugin",
    aliases: &[],
    usage: "/plugin list|enable <name>|disable <name>|info <name>",
    description_id: MessageId::CmdPluginDescription,
};
static PLUGINS_INFO: CommandInfo = CommandInfo {
    name: "plugins",
    aliases: &[],  // "plugin" is now a separate command
    usage: "/plugins [name]",
    description_id: MessageId::CmdPluginDescription,
};

fn run_registered(app: &mut App, name: &str, arg: Option<&str>) -> CommandResult {
    dispatch(app, name, arg).expect("registered utility command should dispatch")
}

fn run_attach(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "attach", arg)
}
fn run_task(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "task", arg)
}
fn run_jobs(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "jobs", arg)
}
fn run_mcp(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "mcp", arg)
}
fn run_network(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "network", arg)
}
fn run_plugin(app: &mut App, arg: Option<&str>) -> CommandResult {
    plugin_cmd(app, arg)
}
fn run_plugins(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "plugins", arg)
}

pub(in crate::commands) fn dispatch(
    app: &mut App,
    command: &str,
    arg: Option<&str>,
) -> Option<CommandResult> {
    let result = match command {
        "attach" | "image" | "media" | "fujian" => attachment::attach(app, arg),
        "task" | "tasks" => task::task(app, arg),
        "jobs" | "job" | "zuoye" => jobs::jobs(app, arg),
        "mcp" => mcp::mcp(app, arg),
        "network" => network::network(app, arg),
                "plugins" => crate::commands::plugins::plugins(app, arg),
        "plugin" => plugin_cmd(app, arg),
        _ => return None,
    };
    Some(result)
}

