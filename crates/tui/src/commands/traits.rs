//! Command trait, CommandGroup trait, and CommandRegistry.

//!
//! Individual commands implement [`Command`], groups of commands implement
//! [`CommandGroup`], and the [`CommandRegistry`] collects all groups and
//! provides lookup + dispatch.
//!

use std::collections::HashMap;

use crate::localization::{Locale, MessageId};
use crate::tui::app::App;

use super::CommandResult;

// ---------------------------------------------------------------------------
// CommandInfo — metadata carried by every command
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct CommandInfo {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub description_id: MessageId,
}

impl CommandInfo {
    pub fn requires_argument(&self) -> bool {
        self.usage.contains('<') || self.usage.contains('[')
    }

    pub fn palette_command(&self) -> String {
        if self.requires_argument() {
            format!("/{} ", self.name)
        } else {
            format!("/{}", self.name)
        }
    }

    pub fn description_for(&self, locale: Locale) -> &'static str {
        crate::localization::tr(locale, self.description_id)
    }

    pub fn palette_description_for(&self, locale: Locale) -> String {
        let desc = self.description_for(locale);
        if self.aliases.is_empty() {
            desc.to_string()
        } else {
            format!("{}  aliases: {}", desc, self.aliases.join(", "))
        }
    }
}

// ---------------------------------------------------------------------------
// Command trait
// ---------------------------------------------------------------------------

pub trait Command: Send + Sync {
    fn info(&self) -> &'static CommandInfo;
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult;
}

// ---------------------------------------------------------------------------
// CommandGroup trait
// ---------------------------------------------------------------------------

pub trait CommandGroup: Send + Sync {
    fn commands(&self) -> Vec<Box<dyn Command>>;
}

// ---------------------------------------------------------------------------
// CommandRegistry
// ---------------------------------------------------------------------------

pub struct CommandRegistry {
    commands: Vec<Box<dyn Command>>,
    name_to_index: HashMap<&'static str, usize>,
}

impl CommandRegistry {
    pub fn empty() -> Self {
        Self {
            commands: Vec::new(),
            name_to_index: HashMap::new(),
        }
    }

    pub fn register(&mut self, cmd: Box<dyn Command>) {
        let idx = self.commands.len();
        let info = cmd.info();
        self.name_to_index.insert(info.name, idx);
        for alias in info.aliases {
            self.name_to_index.insert(alias, idx);
        }
        self.commands.push(cmd);
    }

    pub fn register_group(&mut self, group: &dyn CommandGroup) {
        for cmd in group.commands() {
            self.register(cmd);
        }
    }

    pub fn get(&self, name: &str) -> Option<&dyn Command> {
        let name = name.strip_prefix('/').unwrap_or(name);
        self.name_to_index
            .get(name)
            .and_then(|&idx| self.commands.get(idx))
            .map(Box::as_ref)
    }

    pub fn get_info(&self, name: &str) -> Option<&'static CommandInfo> {
        self.get(name).map(|cmd| cmd.info())
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn Command> {
        self.commands.iter().map(Box::as_ref)
    }

    pub fn infos(&self) -> Vec<&'static CommandInfo> {
        self.iter().map(|cmd| cmd.info()).collect()
    }
}
