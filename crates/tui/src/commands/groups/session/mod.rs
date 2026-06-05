//! Session commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

mod rename;
mod save;
mod fork;
mod new;
mod sessions;
mod load;
mod compact;
mod purge;
mod export;

use crate::commands::traits::{Command, CommandGroup};

use self::rename::Rename;
use self::save::Save;
use self::fork::Fork;
use self::new::New;
use self::sessions::Sessions;
use self::load::Load;
use self::compact::Compact;
use self::purge::Purge;
use self::export::Export;

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
