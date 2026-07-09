//! Memory command area: persistent memory and quick notes.

// This group dir intentionally has a `memory.rs` child module with the same
// name. The module_inception allow is a permanent structure rationale, not
// migration scaffolding; see docs/architecture/command-dispatch.md.
#[allow(clippy::module_inception)]
mod memory;
mod note;

use crate::commands::traits::{Command, CommandGroup, FunctionCommand, RegisterCommand};

pub struct MemoryCommands;

impl CommandGroup for MemoryCommands {
    fn commands(&self) -> &'static [Box<dyn Command>] {
        cached_command_list!(vec![
            Box::new(FunctionCommand::new(
                note::NoteCmd::info(),
                note::NoteCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                memory::MemoryCmd::info(),
                memory::MemoryCmd::execute,
            )),
        ])
    }
}
