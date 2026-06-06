//! Utility commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

pub(crate) mod anchor;
pub(crate) mod hooks;
pub(crate) mod jobs;
pub(crate) mod mcp;
pub(crate) mod network;
pub(crate) mod queue;
pub(crate) mod rlm;
pub(crate) mod slop;
pub(crate) mod stash;
pub(crate) mod task;

use crate::commands::traits::{Command, CommandGroup};

use self::anchor::Anchor;
use self::hooks::Hooks;
use self::jobs::Jobs;
use self::mcp::Mcp;
use self::network::Network;
use self::queue::Queue;
use self::rlm::Rlm;
use self::slop::Slop;
use self::stash::Stash;
use self::task::Task;

pub struct UtilityCommands;
impl CommandGroup for UtilityCommands {
    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(Queue),
            Box::new(Stash),
            Box::new(Hooks),
            Box::new(Anchor),
            Box::new(Network),
            Box::new(Mcp),
            Box::new(Rlm),
            Box::new(Task),
            Box::new(Jobs),
            Box::new(Slop),
        ]
    }
}
