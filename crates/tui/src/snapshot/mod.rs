//! Workspace snapshots — pre/post-turn safety net.
//!
//! Each turn the engine takes a `pre-turn:<seq>` snapshot of the user's
//! workspace into a side git repo at
//! `~/.deepseek/snapshots/<project_hash>/<worktree_hash>/.git`, then a
//! matching `post-turn:<seq>` snapshot when the turn finishes. Users
//! can roll back via `/restore N` (slash command) or, when the model
//! recognises an "undo my last edit" intent, the `revert_turn` tool.
//!
//! ## Why a side repo?
//!
//! - The user's own `.git` is never touched. `--git-dir` and
//!   `--work-tree` are *always* set together when we shell out to git;
//!   that single invariant is what keeps snapshots and the user's repo
//!   completely independent.
//! - Workspaces without git still get snapshots.
//! - `git`'s own deduplication (object packfiles) keeps the disk
//!   footprint tractable — typical 100 MB workspace × 12 turns ≈ 1.2 GB
//!   uncompressed but git's content-addressed storage usually brings
//!   that down 10-30×. We mitigate further with:
//!     - 7-day default retention (`session_manager` prunes at session
//!       start via [`prune::prune_older_than`]).
//!     - `gc.auto = 0` on the side repo (we don't want background gcs
//!       firing mid-turn) plus an explicit `git gc --prune=now` after
//!       prune.
//!     - Startup cleanup for stale `tmp_pack_*` files left by interrupted
//!       git pack operations.
//!
//! ## Failure model
//!
//! Pre/post-turn snapshot calls are **non-fatal**. If `git` is missing,
//! the disk is full, or the workspace is on a read-only filesystem, the
//! turn proceeds and the engine logs a warning. The snapshot is a
//! safety net, not a correctness gate.

pub mod paths;
pub mod prune;
pub mod repo;

#[allow(unused_imports)]
pub use paths::{snapshot_dir_for, snapshot_git_dir};
pub use prune::{DEFAULT_MAX_AGE, prune_older_than};

/// Maximum snapshots kept per workspace side-repo. Oldest are pruned
/// after each new snapshot to cap disk usage (#1112).
pub const DEFAULT_MAX_SNAPSHOTS: usize = 50;
#[allow(unused_imports)]
pub use repo::{
    DEFAULT_MAX_WORKSPACE_BYTES_FOR_SNAPSHOT, Snapshot, SnapshotId, SnapshotRepo,
    estimate_workspace_size_bounded,
};
