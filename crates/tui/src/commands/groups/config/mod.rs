//! Config commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

pub(crate) mod config;
pub(crate) mod settings;
pub(crate) mod status;
pub(crate) mod statusline;
pub(crate) mod mode;
pub(crate) mod theme;
pub(crate) mod verbose;
pub(crate) mod trust;
pub(crate) mod logout;

use crate::commands::traits::{Command, CommandGroup};

use self::config::Config;
use self::settings::Settings;
use self::status::Status;
use self::statusline::Statusline;
use self::mode::Mode;
use self::theme::Theme;
use self::verbose::Verbose;
use self::trust::Trust;
use self::logout::Logout;

pub struct ConfigCommands;
impl CommandGroup for ConfigCommands {
    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(Config),
            Box::new(Settings),
            Box::new(Status),
            Box::new(Statusline),
            Box::new(Mode),
            Box::new(Theme),
            Box::new(Verbose),
            Box::new(Trust),
            Box::new(Logout),
        ]
    }
}
