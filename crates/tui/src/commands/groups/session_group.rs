//! Session commands group — rename, save, fork, new, sessions/resume, load,
//! compact, purge, export

use crate::tui::app::App;

use crate::commands::traits::{Command, CommandGroup, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Rename;
impl Command for Rename {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "rename", aliases: &["gaiming", "chongmingming"], usage: "/rename <title>", description_id: MessageId::CmdRenameDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { crate::commands::back::rename::rename(app, args) }
}

pub struct Save;
impl Command for Save {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "save", aliases: &[], usage: "/save [path]", description_id: MessageId::CmdSaveDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { crate::commands::back::session::save(app, args) }
}

pub struct Fork;
impl Command for Fork {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "fork", aliases: &["branch"], usage: "/fork", description_id: MessageId::CmdForkDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { crate::commands::back::session::fork(app) }
}

pub struct New;
impl Command for New {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "new", aliases: &[], usage: "/new", description_id: MessageId::CmdNewDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { crate::commands::back::session::new_session(app, args) }
}

pub struct Sessions;
impl Command for Sessions {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "sessions", aliases: &["resume"], usage: "/sessions", description_id: MessageId::CmdSessionsDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { crate::commands::back::session::sessions(app, _args) }
}

pub struct Load;
impl Command for Load {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "load", aliases: &["jiazai"], usage: "/load <file>", description_id: MessageId::CmdLoadDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { crate::commands::back::session::load(app, args) }
}

pub struct Compact;
impl Command for Compact {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "compact", aliases: &["yasuo"], usage: "/compact", description_id: MessageId::CmdCompactDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { crate::commands::back::session::compact(app) }
}

pub struct Purge;
impl Command for Purge {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "purge", aliases: &["qingchu"], usage: "/purge", description_id: MessageId::CmdPurgeDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { crate::commands::back::session::purge(app) }
}

pub struct Export;
impl Command for Export {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "export", aliases: &["daochu"], usage: "/export [path]", description_id: MessageId::CmdExportDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { crate::commands::back::session::export(app, args) }
}

pub struct SessionCommands;
impl CommandGroup for SessionCommands {

    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(Rename),
            Box::new(Save),
            Box::new(Fork),
            Box::new(New),
            Box::new(Sessions),
            Box::new(Load),
            Box::new(Compact),
            Box::new(Purge),
            Box::new(Export),
        ]
    }
}
