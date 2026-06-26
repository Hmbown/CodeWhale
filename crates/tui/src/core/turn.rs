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
    #[allow(dead_code)] // TurnMetadata started_at (see #3490)
    pub started_at: Instant,

    /// Current step in the turn (tool call iteration)
    pub step: u32,

    /// Maximum steps allowed
    pub max_steps: u32,

    /// Number of tool calls made in this turn.
    tool_call_count: usize,

    /// Whether the turn has been cancelled
    #[allow(dead_code)] // TurnMetadata cancelled flag (see #3490)
    pub cancelled: bool,

    /// Usage for this turn
    pub usage: Usage,
}

impl TurnContext {
    /// Create a new turn context
    pub fn new(max_steps: u32) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            started_at: Instant::now(),
            step: 0,
            max_steps,
            tool_call_count: 0,
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

    /// Record that a tool call occurred.
    pub fn record_tool_call(&mut self) {
        self.tool_call_count += 1;
    }

    /// Whether this turn has executed at least one tool call.
    pub fn has_tool_calls(&self) -> bool {
        self.tool_call_count > 0
    }

    /// Cancel the turn
    #[allow(dead_code)] // TurnContext cancel; part of public API (see #3490)
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    /// Get the elapsed time
    #[allow(dead_code)] // TurnContext elapsed; part of public API (see #3490)
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

/// Maximum characters of the user prompt snippet to embed in a snapshot
/// label. Longer prompts are truncated with an ellipsis.
const USER_PROMPT_LABEL_MAX: usize = 100;

/// Format a snapshot label that includes the user prompt for readability
/// in `/restore` listings.
///
/// Takes the first line of the prompt (up to `USER_PROMPT_LABEL_MAX`
/// characters) and appends it to the traditional `type:seq` label so
/// users can identify which turn each snapshot belongs to.
fn format_snapshot_label(prefix: &str, turn_seq: u64, user_prompt: Option<&str>) -> String {
    let base = format!("{prefix}:{turn_seq}");
    match user_prompt {
        None | Some("") => base,
        Some(prompt) => {
            let first_line = prompt.lines().next().unwrap_or("");
            let truncated: String = first_line.chars().take(USER_PROMPT_LABEL_MAX).collect();
            if truncated.chars().count() < first_line.chars().count() {
                format!("{base}: {truncated}…")
            } else {
                format!("{base}: {truncated}")
            }
        }
    }
}

/// Take a `pre-turn:<seq>` workspace snapshot.
///
/// `cap_bytes` is the workspace-size ceiling that gates first-init
/// (passed through to [`SnapshotRepo::open_or_init_with_cap`]); pass
/// `0` to disable the cap.
/// `user_prompt` is an optional snippet of the user's message for this
/// turn, embedded in the snapshot label so `/restore` listings are
/// human-readable.
///
/// Returns the snapshot SHA on success, `None` on any error. Errors are
/// logged at WARN; the turn loop must not block on this.
pub fn pre_turn_snapshot(
    workspace: &Path,
    turn_seq: u64,
    cap_bytes: u64,
    user_prompt: Option<&str>,
) -> Option<String> {
    snapshot_with_label(
        workspace,
        &format_snapshot_label("pre-turn", turn_seq, user_prompt),
        cap_bytes,
    )
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
pub fn post_turn_snapshot(
    workspace: &Path,
    turn_seq: u64,
    cap_bytes: u64,
    user_prompt: Option<&str>,
) -> Option<String> {
    snapshot_with_label(
        workspace,
        &format_snapshot_label("post-turn", turn_seq, user_prompt),
        cap_bytes,
    )
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
