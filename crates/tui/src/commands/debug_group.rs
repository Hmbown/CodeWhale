//! Debug commands group — translate, tokens, cost, balance, cache, system,
//! context, edit, diff, undo, retry

use crate::tui::app::App;

use super::traits::{Command, CommandGroup, CommandInfo};
use super::CommandResult;
use crate::localization::MessageId;

pub struct Translate;
impl Command for Translate {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "translate", aliases: &["translation", "transale"], usage: "/translate", description_id: MessageId::CmdTranslateDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { super::core::translate(app) }
}

pub struct Tokens;
impl Command for Tokens {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "tokens", aliases: &[], usage: "/tokens", description_id: MessageId::CmdTokensDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { super::debug::tokens(app) }
}

pub struct Cost;
impl Command for Cost {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "cost", aliases: &[], usage: "/cost", description_id: MessageId::CmdCostDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { super::debug::cost(app) }
}

pub struct Balance;
impl Command for Balance {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "balance", aliases: &[], usage: "/balance", description_id: MessageId::CmdBalanceDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { super::balance::balance(app) }
}

pub struct Cache;
impl Command for Cache {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "cache", aliases: &[], usage: "/cache [count|inspect|stats|zones|warmup]", description_id: MessageId::CmdCacheDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::debug::cache(app, args) }
}

pub struct System;
impl Command for System {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "system", aliases: &["xitong"], usage: "/system", description_id: MessageId::CmdSystemDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { super::debug::system_prompt(app) }
}

pub struct Context;
impl Command for Context {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "context", aliases: &["ctx"], usage: "/context", description_id: MessageId::CmdContextDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { super::debug::context(app) }
}

pub struct Edit;
impl Command for Edit {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "edit", aliases: &[], usage: "/edit", description_id: MessageId::CmdEditDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { super::debug::edit(app) }
}

pub struct Diff;
impl Command for Diff {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "diff", aliases: &[], usage: "/diff", description_id: MessageId::CmdDiffDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { super::debug::diff(app) }
}

pub struct Undo;
impl Command for Undo {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "undo", aliases: &[], usage: "/undo", description_id: MessageId::CmdUndoDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        // Try surgical patch-undo first; fall back to conversation undo
        let result = super::debug::patch_undo(app);
        if result.message.as_deref().is_none_or(|m| {
            m.starts_with("No snapshots found")
                || m.starts_with("No tool or pre-turn")
                || m.starts_with("Snapshot repo")
        }) {
            super::debug::undo_conversation(app)
        } else {
            result
        }
    }
}

pub struct Retry;
impl Command for Retry {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "retry", aliases: &["chongshi"], usage: "/retry", description_id: MessageId::CmdRetryDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { super::debug::retry(app) }
}

pub struct DebugCommands;
impl CommandGroup for DebugCommands {
    fn group_name(&self) -> &'static str { "Debug" }
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
