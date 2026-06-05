//! Core commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

pub(crate) mod help;
pub(crate) mod clear;
pub(crate) mod exit;
pub(crate) mod model;
pub(crate) mod models;
pub(crate) mod provider;
pub(crate) mod links;
pub(crate) mod feedback;
pub(crate) mod home;
pub(crate) mod workspace;
pub(crate) mod subagents;
pub(crate) mod agent;
pub(crate) mod profile;
pub(crate) mod relay;

use crate::commands::traits::{Command, CommandGroup};

pub struct CoreCommands;
impl CommandGroup for CoreCommands {
    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(help::Help),
            Box::new(clear::Clear),
            Box::new(exit::Exit),
            Box::new(model::Model),
            Box::new(models::Models),
            Box::new(provider::Provider),
            Box::new(links::Links),
            Box::new(feedback::Feedback),
            Box::new(home::Home),
            Box::new(workspace::Workspace),
            Box::new(subagents::Subagents),
            Box::new(agent::Agent),
            Box::new(profile::Profile),
            Box::new(relay::Relay),
        ]
    }
}
