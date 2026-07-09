//! Lane registry + Runtime backends (#4176).
//!
//! A **Lane** is a running workflow instance (one issue/goal). **Runtime** owns
//! where/how it executes (tmux, inline, vm, ci) — never Fleet.
//!
//! Persistence: `$CODEWHALE_HOME/lanes/<lane-id>.json` plus stream-json logs
//! under `$CODEWHALE_HOME/lanes/logs/<lane-id>.ndjson`.

mod registry;
mod runtime;
mod worktree;

pub use registry::{LaneRecord, LaneRegistry, LaneStatus, lanes_dir};
pub use runtime::{
    InlineRuntime, LaneStartSpec, RuntimeBackend, RuntimeBackendKind, TmuxRuntime, backend_for,
    resolve_backend,
};
pub use worktree::{WorktreeProvision, provision_worktree, remove_worktree_if_expired};
