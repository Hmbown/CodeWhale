//! Project commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

pub(crate) mod change;
pub(crate) mod goal;
pub(crate) mod init;
pub(crate) mod lsp;
pub(crate) mod share;

use crate::commands::traits::{Command, CommandGroup};

use self::change::Change;
use self::goal::Goal;
use self::init::Init;
use self::lsp::Lsp;
use self::share::Share;

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
