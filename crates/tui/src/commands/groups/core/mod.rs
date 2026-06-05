//! Core commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

mod help;
mod clear;
mod exit;
mod model;
mod models;
mod provider;
mod links;
mod feedback;
mod home;
mod workspace;
mod subagents;
mod agent;
mod profile;
mod relay;

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
