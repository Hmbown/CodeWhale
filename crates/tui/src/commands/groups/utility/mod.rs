//! Utility command area: attachments, background tasks, jobs, MCP, and
//! network inspection.

mod attachment;
mod jobs;
mod mcp;
mod network;
mod task;

use crate::commands::CommandResult;
use crate::tui::app::App;

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
        _ => return None,
    };
    Some(result)
}
