//! Memory commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

mod note;
mod memory;
mod attach;

use crate::commands::traits::{Command, CommandGroup};

use self::note::Note;
use self::memory::Memory;
use self::attach::Attach;

pub struct MemoryCommands;
impl CommandGroup for MemoryCommands {
    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(Note),
            Box::new(Memory),
            Box::new(Attach),
        ]
    }
}
