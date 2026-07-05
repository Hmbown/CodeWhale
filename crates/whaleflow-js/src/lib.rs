//! Sandboxed rquickjs runtime for dynamic WhaleFlow scripts.
//!
//! This crate owns the JavaScript half of the WhaleFlow dynamic-workflow
//! bridge (design doc `WHALEFLOW_DYNAMIC_RQUICKJS_DESIGN_2026-07-04.md`):
//!
//! - a QuickJS VM pinned to one dedicated OS thread (`Ctx` is `!Send`; the
//!   `parallel` feature is deliberately NOT enabled — design §2.2),
//! - `Send`-only bridge request types ([`SpawnRequest`], [`ToolCallRequest`],
//!   [`BudgetQuery`]) with `oneshot` replies that cross to a driver pump
//!   running on the host's multi-thread runtime (design §3.2),
//! - the injected JS stdlib prelude: `task()`, `tools.*`, `budget`,
//!   `parallel()`, `pipeline()`, `log()` (design §7).
//!
//! The driver that services these requests lives in `codewhale-tui`
//! (`crates/tui/src/tools/subagent/whaleflow_bridge.rs`) so the tui side
//! keeps sole ownership of `SubAgentManager` without a dependency cycle.

mod bridge;
mod vm;

pub use bridge::{
    BudgetQuery, BudgetSnapshot, HostChannels, SpawnRequest, TaskCompletion, ToolCallOutcome,
    ToolCallRequest,
};
pub use codewhale_tools::ToolCallSource;
pub use vm::{DEFAULT_MAX_STACK_BYTES, DEFAULT_MEMORY_LIMIT_BYTES, VmOptions, run_script};
