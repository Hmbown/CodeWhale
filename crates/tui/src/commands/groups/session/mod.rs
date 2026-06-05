//! Session commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

pub(crate) mod rename;
pub(crate) mod save;
pub(crate) mod fork;
pub(crate) mod new;
pub(crate) mod sessions;
pub(crate) mod load;
pub(crate) mod compact;
pub(crate) mod purge;
pub(crate) mod export;

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
