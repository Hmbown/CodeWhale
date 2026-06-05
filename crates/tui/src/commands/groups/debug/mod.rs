//! Debug commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

pub(crate) mod translate;
pub(crate) mod tokens;
pub(crate) mod cost;
pub(crate) mod balance;
pub(crate) mod cache;
pub(crate) mod system;
pub(crate) mod context;
pub(crate) mod edit;
pub(crate) mod diff;
pub(crate) mod undo;
pub(crate) mod retry;

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
