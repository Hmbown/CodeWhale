//! Memory commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

pub(crate) mod attach;
pub(crate) mod memory;
pub(crate) mod note;

use crate::commands::traits::{Command, CommandGroup};

use self::attach::Attach;
use self::memory::Memory;
use self::note::Note;

pub struct MemoryCommands;
impl CommandGroup for MemoryCommands {
    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![Box::new(Note), Box::new(Memory), Box::new(Attach)]
    }
}
