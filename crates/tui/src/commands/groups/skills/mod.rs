//! Skills commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

mod skills;
mod skill;
mod review;
mod restore;

use crate::commands::traits::{Command, CommandGroup};

use self::skills::Skills;
use self::skill::Skill;
use self::review::Review;
use self::restore::Restore;

pub struct SkillsCommands;
impl CommandGroup for SkillsCommands {
    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(Skills),
            Box::new(Skill),
            Box::new(Review),
            Box::new(Restore),
        ]
    }
}
