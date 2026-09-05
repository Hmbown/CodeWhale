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
    "diff",
    "edit",
    "hf",
    "lsp",
    "modeldb",
    "models",
    "network",
    "plugin",
    "preview-request",
    "profile",
    "purge",
    "relay",
    "rename",
    "rlm",
    "settings",
    "share",
    "workbar",
    "status",
    "system",
    "theme",
    "tools",
    "trust",
    "verbose",
];

pub(crate) const COMPATIBILITY_DISCOVERY_COMMANDS: &[&str] = &["subagents"];

/// Small, task-oriented starting set for a bare `/` in the composer.
///
/// The full command catalog remains searchable through `/help`, the command
/// palette, and by typing any command prefix. `agents` is the preferred alias
/// for the compatibility-owned `subagents` command.
pub(crate) const BARE_SLASH_DISCOVERY_COMMANDS: &[&str] =
    &["help", "setup", "model", "settings", "resume", "rc"];

#[must_use]
pub(crate) fn bare_slash_discovery_rank(name: &str) -> Option<usize> {
    BARE_SLASH_DISCOVERY_COMMANDS
        .iter()
        .position(|entry| *entry == name)
}

/// Built-in commands that the palette pastes into the composer instead of
/// executing, even though they have no *required* argument.
///
/// Prefer keeping this empty. Every name here must be a registered canonical
/// command name — see `palette_paste_only_names_are_registered` in the
/// command palette tests.
pub(crate) const PALETTE_PASTE_ONLY: &[&str] = &[];

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

    /// Whether the slash menu / composer should leave a trailing space so the
    /// user can type arguments immediately. `/change` is bare-useful (opens
    /// the latest changelog) even though its usage documents an optional
    /// version, so it is the only historical carve-out.
    pub fn composer_wants_trailing_space(&self) -> bool {
        self.name != "change" && self.requires_argument()
    }

    /// Whether the command palette should run this command immediately when
    /// selected, instead of pasting it into the composer.
    ///
    /// Default: run anything that does not require a mandatory positional
    /// argument (including optional-arg commands that open a picker when bare).
    /// [`PALETTE_PASTE_ONLY`] is the explicit opt-out for side-effectful or
    /// multi-step no-arg commands that should still paste for confirmation.
    pub fn palette_runs_directly(&self) -> bool {
        if self.requires_required_argument() {
            return false;
        }
        !PALETTE_PASTE_ONLY.contains(&self.name)
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
        if !prefix.trim_start_matches('/').trim().is_empty() {
            return true;
        }
        BARE_SLASH_DISCOVERY_COMMANDS
            .iter()
            .any(|name| self.name == *name || self.aliases.contains(name))
    }
}

pub trait Command: Send + Sync {
    fn info(&self) -> &'static CommandInfo;
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult;

    /// FEAT-015 dual-path seam: if the entry carries a capability-scoped
    /// handler, the dispatcher builds the envelope from `app` and calls it
    /// here; otherwise the legacy `execute(app, args)` path is used. The
    /// default keeps every existing entry legacy (D2).
    fn contextual_handler(
        &self,
    ) -> Option<codewhale_command_contract::handler::CommandHandler<CommandResult>> {
        None
    }
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

/// A registry entry that carries an optional capability-scoped handler.
///
/// FEAT-015's dual-path seam (D2): migrated registrations may supply a
/// `CommandHandler<CommandResult>` (App-free; built from `CommandContexts`),
/// while unmigrated registrations keep the legacy `execute(app, args)` path.
/// This entry type is App-free — only the dispatcher in `commands/mod.rs`
/// touches `App` when it builds the envelope from the bundle.
///
/// FEAT-015 ships no production contextual registration, so in production
/// builds this type is only referenced through the trait; the test fixture
/// (D6) constructs it under `#[cfg(test)]`. The allow is removed once a
/// production group migrates (FEAT-018+).
pub(crate) struct ContextualCommand {
    info: &'static CommandInfo,
    handler: Option<codewhale_command_contract::handler::CommandHandler<CommandResult>>,
    legacy: Option<CommandHandler>,
}

impl ContextualCommand {
    pub(crate) const fn contextual(
        info: &'static CommandInfo,
        handler: codewhale_command_contract::handler::CommandHandler<CommandResult>,
    ) -> Self {
        Self {
            info,
            handler: Some(handler),
            legacy: None,
        }
    }

    /// Bridge one portable contract registration into the TUI-owned registry.
    ///
    /// The command supplies only contract metadata and an App-free handler;
    /// the TUI resolves the localization key and owns the resulting registry
    /// entry. This is the dependency inversion later command crates reuse.
    pub(crate) fn from_contract<C>() -> Result<Self, String>
    where
        C: codewhale_command_contract::metadata::RegisterCommand<CommandResult>,
    {
        let portable = C::info();
        let description_id = super::contract::key_to_message_id(portable.description_key)
            .ok_or_else(|| {
                format!(
                    "unknown command description key {:?} for /{}",
                    portable.description_key, portable.name
                )
            })?;
        let info = Box::leak(Box::new(CommandInfo {
            name: portable.name,
            aliases: portable.aliases,
            usage: portable.usage,
            description_id,
        }));
        Ok(Self::contextual(info, C::handler()))
    }
}
impl Command for ContextualCommand {
    fn info(&self) -> &'static CommandInfo {
        self.info
    }

    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        match self.legacy {
            Some(legacy) => legacy(app, args),
            None => CommandResult::error("command has no executable handler"),
        }
    }

    fn contextual_handler(
        &self,
    ) -> Option<codewhale_command_contract::handler::CommandHandler<CommandResult>> {
        self.handler.clone()
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

    /// FEAT-015: register a test-only contextual command under `#[cfg(test)]`.
    /// The production registry is untouched (D6); the fixture dispatches
    /// through the public `execute()` to prove the seam.
    #[cfg(test)]
    pub(crate) fn register_test_only(&mut self, command: &'static dyn Command) {
        self.register(command);
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

    /// FEAT-015: whether the named entry has a capability-scoped handler.
    /// Used by test assertions under `#[cfg(test)]`; production builds have
    /// no contextual entries, so the method is dead there until a group
    /// migrates (FEAT-018+).
    #[allow(dead_code)]
    pub(crate) fn has_contextual_handler(&self, name: &str) -> bool {
        self.get(name)
            .is_some_and(|command| command.contextual_handler().is_some())
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn Command> {
        self.commands.iter().copied()
    }

    pub fn infos(&self) -> Vec<&'static CommandInfo> {
        self.iter().map(Command::info).collect()
    }
}
