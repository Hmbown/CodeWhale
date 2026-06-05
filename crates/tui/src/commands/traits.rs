//! Command trait, CommandGroup trait, and CommandRegistry.
//!
//! This is the core of the strategy-pattern refactoring. Individual commands
//! implement [`Command`], groups of commands implement [`CommandGroup`], and
//! the [`CommandRegistry`] collects all groups and provides lookup + dispatch.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::localization::{Locale, MessageId};
use crate::tui::app::App;

use super::CommandResult;

// ---------------------------------------------------------------------------
// CommandInfo — metadata carried by every command
// ---------------------------------------------------------------------------

/// Static metadata about a slash command.
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
// Command trait — one struct per command
// ---------------------------------------------------------------------------

/// A single slash command.
///
/// Every concrete command is a unit struct that implements this trait.
/// The `info()` method returns static metadata; `execute()` performs the
/// actual work (usually delegating to the existing backend in `core.rs`,
/// `session.rs`, etc.).
pub trait Command: Send + Sync {
    fn info(&self) -> &'static CommandInfo;
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult;
}

// ---------------------------------------------------------------------------
// CommandGroup trait — one struct per logical group
// ---------------------------------------------------------------------------

/// A group of related commands (e.g. Core, Session, Config, Debug).
///
/// Each group returns a list of boxed commands that it owns. The registry
/// collects commands from all registered groups.
pub trait CommandGroup: Send + Sync {
    fn commands(&self) -> Vec<Box<dyn Command>>;
}

// ---------------------------------------------------------------------------
// CommandRegistry — central dispatch
// ---------------------------------------------------------------------------

/// Central registry that holds all registered commands and provides O(1)
/// lookup by name or alias.
pub struct CommandRegistry {
    commands: Vec<Box<dyn Command>>,
    name_to_index: HashMap<&'static str, usize>,
}

impl CommandRegistry {
    /// Create an empty registry.
    pub fn empty() -> Self {
        Self {
            commands: Vec::new(),
            name_to_index: HashMap::new(),
        }
    }

    /// Register a single command.
    pub fn register(&mut self, cmd: Box<dyn Command>) {
        let idx = self.commands.len();
        let info = cmd.info();
        self.name_to_index.insert(info.name, idx);
        for alias in info.aliases {
            self.name_to_index.insert(alias, idx);
        }
        self.commands.push(cmd);
    }

    /// Register all commands from a group.
    pub fn register_group(&mut self, group: &dyn CommandGroup) {
        for cmd in group.commands() {
            self.register(cmd);
        }
    }

    /// Look up a command by name or alias (with or without leading `/`).
    pub fn get(&self, name: &str) -> Option<&dyn Command> {
        let name = name.strip_prefix('/').unwrap_or(name);
        self.name_to_index
            .get(name)
            .and_then(|&idx| self.commands.get(idx))
            .map(Box::as_ref)
    }

    /// Look up command metadata by name or alias.
    pub fn get_info(&self, name: &str) -> Option<&'static CommandInfo> {
        self.get(name).map(|cmd| cmd.info())
    }

    /// Iterate over all registered commands.
    pub fn iter(&self) -> impl Iterator<Item = &dyn Command> {
        self.commands.iter().map(Box::as_ref)
    }

    /// All registered command infos.
    pub fn infos(&self) -> Vec<&'static CommandInfo> {
        self.iter().map(|cmd| cmd.info()).collect()
    }
}

// ---------------------------------------------------------------------------
// Global lazy registry
// ---------------------------------------------------------------------------

static REGISTRY: OnceLock<CommandRegistry> = OnceLock::new();

/// Build and initialize the global command registry.
///
/// Called once on first access. All command groups are registered here.
fn build_registry() -> CommandRegistry {
    let mut reg = CommandRegistry::empty();

    // Register groups in order of logical grouping.
    reg.register_group(&super::core_group::CoreCommands);
    reg.register_group(&super::session_group::SessionCommands);
    reg.register_group(&super::config_group::ConfigCommands);
    reg.register_group(&super::debug_group::DebugCommands);
    reg.register_group(&super::project_group::ProjectCommands);
    reg.register_group(&super::skills_group::SkillsCommands);
    reg.register_group(&super::memory_group::MemoryCommands);
    reg.register_group(&super::utility_group::UtilityCommands);

    reg
}

/// Access the global registry (lazily initialised on first call).
pub fn registry() -> &'static CommandRegistry {
    REGISTRY.get_or_init(build_registry)
}
