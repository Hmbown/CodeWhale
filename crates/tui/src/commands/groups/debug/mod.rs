//! Debug commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

mod translate;
mod tokens;
mod cost;
mod balance;
mod cache;
mod system;
mod context;
mod edit;
mod diff;
mod undo;
mod retry;

use crate::commands::traits::{Command, CommandGroup};

use self::translate::Translate;
use self::tokens::Tokens;
use self::cost::Cost;
use self::balance::Balance;
use self::cache::Cache;
use self::system::System;
use self::context::Context;
use self::edit::Edit;
use self::diff::Diff;
use self::undo::Undo;
use self::retry::Retry;

pub struct DebugCommands;
impl CommandGroup for DebugCommands {
    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(Translate),
            Box::new(Tokens),
            Box::new(Cost),
            Box::new(Balance),
            Box::new(Cache),
            Box::new(System),
            Box::new(Context),
            Box::new(Edit),
            Box::new(Diff),
            Box::new(Undo),
            Box::new(Retry),
        ]
    }
}
