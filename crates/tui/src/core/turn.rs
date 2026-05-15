//! Turn context and tracking.
//!
//! A "turn" is one user message and the resulting AI response,
//! including any tool calls that occur.
//!
//! ## Snapshot lifecycle hooks
//!
//! [`pre_turn_snapshot`] and [`post_turn_snapshot`] book-end a turn by
//! taking a workspace-level snapshot into a side git repo (see
//! `crate::snapshot`). They are intentionally non-blocking and
//! non-fatal: any IO error is logged at WARN and swallowed so a busted
//! filesystem or missing `git` binary never derails the agent loop.
//! `/restore N` and the `revert_turn` tool both consume these
//! snapshots.

use crate::models::Usage;
use crate::snapshot::SnapshotRepo;
use std::path::Path;
use std::time::{Duration, Instant};

/// Context for a single turn (user message + AI response).
#[derive(Debug)]
pub struct TurnContext {
    /// Turn ID
    pub id: String,

    /// When the turn started
    #[allow(dead_code)]
    pub started_at: Instant,

    /// Current step in the turn (tool call iteration)
    pub step: u32,

    /// Maximum steps allowed
    pub max_steps: u32,

    /// Tool calls made in this turn
    pub tool_calls: Vec<TurnToolCall>,

    /// Whether the turn has been cancelled
    #[allow(dead_code)]
    pub cancelled: bool,

    /// Usage for this turn
    pub usage: Usage,
}

/// Record of a tool call within a turn.
#[derive(Debug, Clone)]
pub struct TurnToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub result: Option<String>,
    pub error: Option<String>,
    pub duration: Option<Duration>,
}

impl TurnContext {
    /// Create a new turn context
    pub fn new(max_steps: u32) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            started_at: Instant::now(),
            step: 0,
            max_steps,
            tool_calls: Vec::new(),
            cancelled: false,
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                ..Usage::default()
            },
        }
    }

    /// Increment the step counter
    pub fn next_step(&mut self) -> bool {
        self.step += 1;
        self.step <= self.max_steps
    }

    /// Check if the turn has reached max steps
    pub fn at_max_steps(&self) -> bool {
        self.step >= self.max_steps
    }

    /// Record a tool call
    pub fn record_tool_call(&mut self, call: TurnToolCall) {
        self.tool_calls.push(call);
    }

    /// Cancel the turn
    #[allow(dead_code)]
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    /// Get the elapsed time
    #[allow(dead_code)]
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Add usage from an API response
    pub fn add_usage(&mut self, usage: &Usage) {
        self.usage.input_tokens += usage.input_tokens;
        self.usage.output_tokens += usage.output_tokens;
        self.usage.prompt_cache_hit_tokens = add_optional_usage(
            self.usage.prompt_cache_hit_tokens,
            usage.prompt_cache_hit_tokens,
        );
        self.usage.prompt_cache_miss_tokens = add_optional_usage(
            self.usage.prompt_cache_miss_tokens,
            usage.prompt_cache_miss_tokens,
        );
        self.usage.reasoning_tokens =
            add_optional_usage(self.usage.reasoning_tokens, usage.reasoning_tokens);
    }
}

fn add_optional_usage(total: Option<u32>, delta: Option<u32>) -> Option<u32> {
    match (total, delta) {
        (Some(total), Some(delta)) => Some(total.saturating_add(delta)),
        (None, Some(delta)) => Some(delta),
        (Some(total), None) => Some(total),
        (None, None) => None,
    }
}

/// Take a `pre-turn:<seq>` workspace snapshot.
///
/// `cap_bytes` is the workspace-size ceiling that gates first-init
/// (passed through to [`SnapshotRepo::open_or_init_with_cap`]); pass
/// `0` to disable the cap.
///
/// Returns the snapshot SHA on success, `None` on any error. Errors are
/// logged at WARN; the turn loop must not block on this.
pub fn pre_turn_snapshot(workspace: &Path, turn_seq: u64, cap_bytes: u64) -> Option<String> {
    snapshot_with_label(workspace, &format!("pre-turn:{turn_seq}"), cap_bytes)
}

/// Take a `tool:<call_id>` workspace snapshot, taken before executing a
/// file-modifying tool call (write_file, edit_file, apply_patch).
///
/// This enables surgical undo: `/undo` can restore to the most recent
/// `tool:<call_id>` snapshot to revert just the last file write.
///
/// Returns the snapshot SHA on success, `None` on any error. Errors are
/// logged at WARN and are non-fatal.
pub fn pre_tool_snapshot(workspace: &Path, call_id: &str, cap_bytes: u64) -> Option<String> {
    snapshot_with_label(workspace, &format!("tool:{call_id}"), cap_bytes)
}

/// Take a `post-turn:<seq>` workspace snapshot. Same failure model as
/// [`pre_turn_snapshot`].
pub fn post_turn_snapshot(workspace: &Path, turn_seq: u64, cap_bytes: u64) -> Option<String> {
    snapshot_with_label(workspace, &format!("post-turn:{turn_seq}"), cap_bytes)
}

fn snapshot_with_label(workspace: &Path, label: &str, cap_bytes: u64) -> Option<String> {
    match SnapshotRepo::open_or_init_with_cap(workspace, cap_bytes) {
        Ok(repo) => {
            let id = match repo.snapshot(label) {
                Ok(id) => Some(id.0),
                Err(e) => {
                    tracing::warn!(target: "snapshot", "snapshot '{label}' failed: {e}");
                    return None;
                }
            };
            // Prune oldest snapshots to cap disk usage (#1112).
            if let Err(e) = repo.prune_keep_last_n(crate::snapshot::DEFAULT_MAX_SNAPSHOTS) {
                tracing::warn!(target: "snapshot", "snapshot prune failed: {e}");
            }
            id
        }
        Err(e) => {
            tracing::warn!(target: "snapshot", "snapshot repo init failed: {e}");
            None
        }
    }
}

impl TurnToolCall {
    /// Create a new tool call record
    pub fn new(id: String, name: String, input: serde_json::Value) -> Self {
        Self {
            id,
            name,
            input,
            result: None,
            error: None,
            duration: None,
        }
    }

    /// Set the result
    pub fn set_result(&mut self, result: String, duration: Duration) {
        self.result = Some(result);
        self.duration = Some(duration);
    }

    /// Set an error
    pub fn set_error(&mut self, error: String, duration: Duration) {
        self.error = Some(error);
        self.duration = Some(duration);
    }
}
