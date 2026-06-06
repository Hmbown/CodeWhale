//! Debug commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

pub(crate) mod balance;
pub(crate) mod cache;
pub(crate) mod context;
pub(crate) mod cost;
pub(crate) mod debug_impl;
pub(crate) mod diff;
pub(crate) mod edit;
pub(crate) mod retry;
pub(crate) mod system;
pub(crate) mod tokens;
pub(crate) mod translate;
pub(crate) mod undo;

use crate::commands::traits::{Command, CommandGroup};

use self::balance::Balance;
use self::cache::Cache;
use self::context::Context;
use self::cost::Cost;
use self::diff::Diff;
use self::edit::Edit;
use self::retry::Retry;
use self::system::System;
use self::tokens::Tokens;
use self::translate::Translate;
use self::undo::Undo;

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
