//! Command traits and registry support.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::localization::{Locale, MessageId, tr};
use crate::tui::app::App;

use super::CommandResult;

#[derive(Debug, Clone, Copy)]
pub struct CommandInfo {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub description_id: MessageId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandDiscovery {
    Primary,
    Advanced,
    Compatibility,
}

pub(crate) const ADVANCED_DISCOVERY_COMMANDS: &[&str] = &[
    "anchor",
    "balance",
    "cache",
    "change",
    "context",
    "debt",
    "diff",
    "edit",
    "goal",
    "hf",
    "hooks",
    "lsp",
    "modeldb",
    "models",
    "network",
    "plugin",
    "profile",
    "purge",
    "relay",
    "rename",
    "rlm",
    "settings",
    "share",
    "sidebar",
    "status",
    "system",
    "theme",
    "tokens",
    "translate",
    "trust",
    "verbose",
    "workspace",
];

pub(crate) const COMPATIBILITY_DISCOVERY_COMMANDS: &[&str] = &["subagents"];

impl CommandDiscovery {
    pub fn show_at_root(self) -> bool {
        matches!(self, CommandDiscovery::Primary)
    }
}

impl CommandInfo {
    pub fn requires_argument(&self) -> bool {
        self.usage.contains('<') || self.usage.contains('[')
    }

    pub fn requires_required_argument(&self) -> bool {
        let mut optional_depth = 0usize;
        for ch in self.usage.chars() {
            match ch {
                '[' => optional_depth += 1,
                ']' => optional_depth = optional_depth.saturating_sub(1),
                '<' if optional_depth == 0 => return true,
                _ => {}
            }
        }
        false
    }

    pub fn palette_command(&self) -> String {
        if self.requires_argument() {
            format!("/{} ", self.name)
        } else {
            format!("/{}", self.name)
        }
    }

    pub fn description_for(&self, locale: Locale) -> Cow<'static, str> {
        tr(locale, self.description_id)
    }

    pub fn palette_description_for(&self, locale: Locale) -> String {
        let desc = self.description_for(locale);
        if self.aliases.is_empty() {
            desc.to_string()
        } else {
            format!("{}  aliases: {}", desc, self.aliases.join(", "))
        }
    }

    pub fn discovery(&self) -> CommandDiscovery {
        if COMPATIBILITY_DISCOVERY_COMMANDS.contains(&self.name) {
            CommandDiscovery::Compatibility
        } else if ADVANCED_DISCOVERY_COMMANDS.contains(&self.name) {
            CommandDiscovery::Advanced
        } else {
            CommandDiscovery::Primary
        }
    }

    pub fn show_in_empty_discovery(&self) -> bool {
        self.discovery().show_at_root()
    }

    pub fn show_in_slash_completion(&self, prefix: &str) -> bool {
        !prefix.trim_start_matches('/').trim().is_empty() || self.show_in_empty_discovery()
    }
}

pub trait Command: Send + Sync {
    fn info(&self) -> &'static CommandInfo;
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult;
}

pub trait CommandGroup: Send + Sync {
    fn commands(&self) -> &'static [Box<dyn Command>];
}

pub(crate) type CommandHandler = fn(&mut App, Option<&str>) -> CommandResult;

/// Trait implemented by focused built-in command modules.
///
/// A command module owns its metadata and exposes a static execution function
/// that the group registry can wire into [`FunctionCommand`].
pub trait RegisterCommand {
    fn info() -> &'static CommandInfo;
    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult;
}

pub(crate) struct FunctionCommand {
    info: &'static CommandInfo,
    handler: CommandHandler,
}

impl FunctionCommand {
    pub(crate) const fn new(info: &'static CommandInfo, handler: CommandHandler) -> Self {
        Self { info, handler }
    }
}

impl Command for FunctionCommand {
    fn info(&self) -> &'static CommandInfo {
        self.info
    }

    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        (self.handler)(app, args)
    }
}

pub struct CommandRegistry {
    commands: Vec<&'static dyn Command>,
    name_to_index: HashMap<&'static str, usize>,
}

impl CommandRegistry {
    pub fn empty() -> Self {
        Self {
            commands: Vec::new(),
            name_to_index: HashMap::new(),
        }
    }

    pub fn register(&mut self, command: &'static dyn Command) {
        let index = self.commands.len();
        let info = command.info();
        self.name_to_index.insert(info.name, index);
        for alias in info.aliases {
            self.name_to_index.insert(alias, index);
        }
        self.commands.push(command);
    }

    pub fn register_group(&mut self, group: &dyn CommandGroup) {
        for command in group.commands() {
            self.register(command.as_ref());
        }
    }

    pub fn get(&self, name: &str) -> Option<&dyn Command> {
        let name = name.strip_prefix('/').unwrap_or(name);
        self.name_to_index
            .get(name)
            .and_then(|index| self.commands.get(*index))
            .copied()
    }

    pub fn get_info(&self, name: &str) -> Option<&'static CommandInfo> {
        self.get(name).map(Command::info)
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn Command> {
        self.commands.iter().copied()
    }

    pub fn infos(&self) -> Vec<&'static CommandInfo> {
        self.iter().map(Command::info).collect()
    }
}
