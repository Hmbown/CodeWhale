//! Skills commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

pub(crate) mod restore;
pub(crate) mod review;
pub(crate) mod skill;
pub(crate) mod skills;
pub(crate) mod support;
#[cfg(test)]
pub(crate) mod test_support;

use crate::commands::traits::{Command, CommandGroup};

use self::restore::Restore;
use self::review::Review;
use self::skill::Skill;
use self::skills::Skills;

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
