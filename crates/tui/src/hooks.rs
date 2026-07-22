//! Hooks system for `DeepSeek` CLI
//!
//! Provides lifecycle hooks that execute user-defined shell commands at:
//! - Session start/end
//! - Tool call before/after

//! - Mode changes
//! - Message submission
//! - Error events
//! - Turn completion
//!
//! Configuration is done via `[[hooks.hooks]]` in config.toml.

mod config;
mod executor;

pub use config::{Hook, HookCondition, HookEvent, HooksConfig};
pub(crate) use executor::parse_tool_call_before_stdout;
#[allow(unused_imports)]
pub use executor::{
    HookContext, HookExecutor, HookResult, MessageSubmitOutcome, ToolCallBeforeStdout,
    ToolCallDecision, TurnEndPayloadInput, TurnEndTotals, turn_end_payload,
};
