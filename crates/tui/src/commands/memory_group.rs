//! Memory / Notes commands group — note, memory, attach

use crate::tui::app::App;

use super::traits::{Command, CommandGroup, CommandInfo};
use super::CommandResult;
use crate::localization::MessageId;

pub struct Note;
impl Command for Note {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "note", aliases: &[], usage: "/note <text> | /note add <text> | /note list | /note show <n> | /note edit <n> <text> | /note remove <n> | /note clear | /note path", description_id: MessageId::CmdNoteDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::note::note(app, args) }
}

pub struct Memory;
impl Command for Memory {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "memory", aliases: &[], usage: "/memory [show|path|clear|edit|help]", description_id: MessageId::CmdMemoryDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::memory::memory(app, args) }
}

pub struct Attach;
impl Command for Attach {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "attach", aliases: &["image", "media", "fujian"], usage: "/attach <path|url> [description]", description_id: MessageId::CmdAttachDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::attachment::attach(app, args) }
}

pub struct MemoryCommands;
impl CommandGroup for MemoryCommands {
    fn group_name(&self) -> &'static str { "Memory" }
    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(Note),
            Box::new(Memory),
            Box::new(Attach),
        ]
    }
}
