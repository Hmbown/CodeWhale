//! Dynamic Workflow runtime for CodeWhale.
//!
//! This crate is the imperative half of Workflow: a sandboxed QuickJS
//! (rquickjs) runtime that executes a model-authored JS program which
//! dispatches fleet-routed subagents via `task()`, fans out with
//! `parallel()`/`pipeline()`, reports progress with `log()`/`phase()`, and
//! scales itself to a token pool via the `budget` global. The static,
//! declarative IR (record/replay, model policy) stays in `codewhale-workflow`;
//! this crate only speaks to the outside world through the
//! [`WorkflowDriver`] seam, so it is fully testable without spawning a real
//! subagent (see [`testing::FakeDriver`]).
//!
//! # Script surface
//!
//! Every script runs inside an async function with these globals:
//!
//! * `args` — the invocation input, verbatim.
//! * `await task(opts)` — dispatch one subagent; resolves to the full result
//!   text, or to a parsed + schema-validated object when `opts.responseSchema`
//!   is set. Throws on rejection, failure, cancellation, budget exhaustion,
//!   or once [`WORKFLOW_LIFETIME_CAP`] spawn attempts have been made.
//! * `parallel(thunks)` — all-settled fan-out; a failed slot becomes `null`;
//!   at most [`PARALLEL_MAX_ITEMS`] items.
//! * `pipeline(items, ...stages)` — per-item stage chains with no barrier
//!   between stages; a stage error drops that item to `null`; same item cap.
//! * `log(msg)` / `phase(title)` — progress events forwarded to the driver.
//! * `budget.total` / `budget.spent()` / `budget.remaining()` — live driver
//!   snapshots (`total` is `null` and `remaining()` is `Infinity` when no
//!   ceiling is configured).
//!
//! `Date.now()`, `new Date()`, `Date.parse/UTC`, and `Math.random()` throw:
//! runs must be deterministic so recorded traces can be replayed.
//!
//! # Ownership boundaries
//!
//! Token accounting and reservation (design §5.3) belong to the driver; the
//! VM only reads snapshots and fast-fails a spawn when the pool is already
//! exhausted. Fleet roster resolution for `profile` also happens driver-side;
//! this crate normalizes and token-validates the profile string, nothing
//! more.

mod driver;
mod error;
mod schema;
pub mod testing;
mod vm;

pub use driver::{
    BudgetSnapshot, ProgressEvent, SpawnedTask, TaskCompletion, TaskRequest, WorkflowDriver,
    normalize_profile,
};
pub use error::{DriverError, WorkflowJsError};
pub use vm::{VmLimits, WorkflowRunCancel, WorkflowVm};

/// Maximum `task()` spawn attempts per run (design §4.3). Counted in the VM
/// before the driver is consulted, so a runaway `loop-until-dry` terminates
/// even if the driver would keep admitting work.
///
/// Product scale: up to 1_000 agents per Workflow run.
pub const WORKFLOW_LIFETIME_CAP: u64 = 1000;

/// Maximum concurrently executing agents within one Workflow run.
///
/// Fan-out may *declare* more work via `parallel()` / `pipeline()`, but the
/// host admits at most this many live `task()` children at once; additional
/// spawns wait for a slot.
pub const WORKFLOW_MAX_CONCURRENT: usize = 16;

/// Maximum items per `parallel()` or `pipeline()` call (design §4.2).
/// Kept at the per-run agent ceiling so a single fan-out cannot declare more
/// work than the lifetime cap can ever complete.
pub const PARALLEL_MAX_ITEMS: usize = 1000;
