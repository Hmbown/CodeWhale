//! Project commands group — change, init, lsp, share, goal/hunt

use crate::tui::app::App;

use super::traits::{Command, CommandGroup, CommandInfo};
use super::CommandResult;
use crate::localization::MessageId;

pub struct Change;
impl Command for Change {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "change", aliases: &[], usage: "/change <description>", description_id: MessageId::CmdChangeDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::back::change::change(app, args) }
}

pub struct Init;
impl Command for Init {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "init", aliases: &[], usage: "/init", description_id: MessageId::CmdInitDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { super::back::init::init(app) }
}

pub struct Lsp;
impl Command for Lsp {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "lsp", aliases: &[], usage: "/lsp <command>", description_id: MessageId::CmdLspDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::back::config::lsp_command(app, args) }
}

pub struct Share;
impl Command for Share {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "share", aliases: &[], usage: "/share [path]", description_id: MessageId::CmdShareDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::share::share(app, args) }
}

pub struct Goal;
impl Command for Goal {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "goal", aliases: &["hunt", "mubiao", "狩猎"], usage: "/goal [start|show|close <reason>]", description_id: MessageId::CmdGoalDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::back::goal::hunt(app, args) }
}

pub struct ProjectCommands;
impl CommandGroup for ProjectCommands {

    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(Change),
            Box::new(Init),
            Box::new(Lsp),
            Box::new(Share),
            Box::new(Goal),
        ]
    }
}
