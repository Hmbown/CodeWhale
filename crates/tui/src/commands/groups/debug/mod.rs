//! Debug command area: token/cost introspection, cache tooling, undo/retry,
//! and the change log.

mod balance;
mod cache;
mod change;
mod tokens;
mod undo;

#[cfg(test)]
mod tests;

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandGroup, CommandInfo, FunctionCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct DebugCommands;

impl CommandGroup for DebugCommands {
    fn commands(&self) -> &'static [Box<dyn Command>] {
        cached_command_list!(vec![
            Box::new(FunctionCommand::new(&TOKENS_INFO, run_tokens)),
            Box::new(FunctionCommand::new(&COST_INFO, run_cost)),
            Box::new(FunctionCommand::new(&BALANCE_INFO, run_balance)),
            Box::new(FunctionCommand::new(&CACHE_INFO, run_cache)),
            Box::new(FunctionCommand::new(&CHANGE_INFO, run_change)),
            Box::new(FunctionCommand::new(&SYSTEM_INFO, run_system)),
            Box::new(FunctionCommand::new(&CONTEXT_INFO, run_context)),
            Box::new(FunctionCommand::new(&EDIT_INFO, run_edit)),
            Box::new(FunctionCommand::new(&DIFF_INFO, run_diff)),
            Box::new(FunctionCommand::new(&UNDO_INFO, run_undo)),
            Box::new(FunctionCommand::new(&RETRY_INFO, run_retry)),
        ])
    }
}

static TOKENS_INFO: CommandInfo = CommandInfo {
    name: "tokens",
    aliases: &[],
    usage: "/tokens",
    description_id: MessageId::CmdTokensDescription,
};
static COST_INFO: CommandInfo = CommandInfo {
    name: "cost",
    aliases: &[],
    usage: "/cost",
    description_id: MessageId::CmdCostDescription,
};
static BALANCE_INFO: CommandInfo = CommandInfo {
    name: "balance",
    aliases: &[],
    usage: "/balance",
    description_id: MessageId::CmdBalanceDescription,
};
static CACHE_INFO: CommandInfo = CommandInfo {
    name: "cache",
    aliases: &[],
    usage: "/cache [count|inspect|stats|zones|warmup]",
    description_id: MessageId::CmdCacheDescription,
};
static CHANGE_INFO: CommandInfo = CommandInfo {
    name: "change",
    aliases: &[],
    usage: "/change [version]",
    description_id: MessageId::CmdChangeDescription,
};
static SYSTEM_INFO: CommandInfo = CommandInfo {
    name: "system",
    aliases: &["xitong"],
    usage: "/system",
    description_id: MessageId::CmdSystemDescription,
};
static CONTEXT_INFO: CommandInfo = CommandInfo {
    name: "context",
    aliases: &["ctx"],
    usage: "/context [report|json|summary]",
    description_id: MessageId::CmdContextDescription,
};
static EDIT_INFO: CommandInfo = CommandInfo {
    name: "edit",
    aliases: &[],
    usage: "/edit",
    description_id: MessageId::CmdEditDescription,
};
static DIFF_INFO: CommandInfo = CommandInfo {
    name: "diff",
    aliases: &[],
    usage: "/diff",
    description_id: MessageId::CmdDiffDescription,
};
static UNDO_INFO: CommandInfo = CommandInfo {
    name: "undo",
    aliases: &[],
    usage: "/undo",
    description_id: MessageId::CmdUndoDescription,
};
static RETRY_INFO: CommandInfo = CommandInfo {
    name: "retry",
    aliases: &["chongshi"],
    usage: "/retry",
    description_id: MessageId::CmdRetryDescription,
};

fn run_registered(app: &mut App, name: &str, arg: Option<&str>) -> CommandResult {
    dispatch(app, name, arg).expect("registered debug command should dispatch")
}

fn run_tokens(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "tokens", arg)
}
fn run_cost(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "cost", arg)
}
fn run_balance(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "balance", arg)
}
fn run_cache(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "cache", arg)
}
fn run_change(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "change", arg)
}
fn run_system(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "system", arg)
}
fn run_context(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "context", arg)
}
fn run_edit(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "edit", arg)
}
fn run_diff(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "diff", arg)
}
fn run_undo(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "undo", arg)
}
fn run_retry(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "retry", arg)
}

pub(in crate::commands) fn dispatch(
    app: &mut App,
    command: &str,
    arg: Option<&str>,
) -> Option<CommandResult> {
    let result = match command {
        "tokens" => tokens::tokens(app),
        "cost" => tokens::cost(app),
        "balance" => balance::balance(app),
        "cache" => cache::cache(app, arg),
        "change" => change::change(app, arg),
        "system" | "xitong" => tokens::system_prompt(app),
        "context" | "ctx" => tokens::context(app, arg),
        "edit" => undo::edit(app),
        "diff" => undo::diff(app),
        "undo" => {
            // Try surgical patch-undo first; fall back to conversation undo
            // if no snapshots are available or if the snapshot undo couldn't
            // find anything useful.
            let result = undo::patch_undo(app);
            if result.message.as_deref().is_none_or(|m| {
                m.starts_with("No snapshots found")
                    || m.starts_with("No older tool or pre-turn")
                    || m.starts_with("Snapshot repo")
            }) {
                undo::undo_conversation(app)
            } else {
                result
            }
        }
        "retry" | "chongshi" => undo::retry(app),
        _ => return None,
    };
    Some(result)
}
