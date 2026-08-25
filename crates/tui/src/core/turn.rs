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

use crate::core::events::TurnRoute;
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

    /// Number of tool calls made in this turn.

    /// Whether the turn has been cancelled
    #[allow(dead_code)]
    pub cancelled: bool,

    /// Usage for this turn
    pub usage: Usage,

    /// Input tokens reported for the most recent parent-route model request.
    /// This is deliberately separate from `usage`, which accumulates every
    /// parent step and programmatic child call for billing.
    pub(crate) latest_parent_input_tokens: Option<u32>,

    /// One-shot latch: an automatic-compaction refusal has already been
    /// surfaced this turn. Pressure is re-checked every step, and repeating
    /// the same refusal on each of a long turn's steps would be noise.
    pub(crate) compaction_refusal_notified: bool,

    /// Route facts resolved for this turn but not timestamped until the first
    /// provider request is actually dispatched.
    pub(crate) pending_route: Option<TurnRoute>,
}

impl TurnContext {
    /// Create a new turn context
    pub fn new(max_steps: u32) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            started_at: Instant::now(),
            step: 0,
            max_steps,
            cancelled: false,
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                ..Usage::default()
            },
            latest_parent_input_tokens: None,
            compaction_refusal_notified: false,
            pending_route: None,
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

    /// Model steps consumed so far (for soft-landing and reporting).
    #[must_use]
    pub fn steps_used(&self) -> u32 {
        self.step
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
        self.usage.input_tokens = self.usage.input_tokens.saturating_add(usage.input_tokens);
        self.usage.output_tokens = self.usage.output_tokens.saturating_add(usage.output_tokens);
        self.usage.prompt_cache_hit_tokens = add_optional_usage(
            self.usage.prompt_cache_hit_tokens,
            usage.prompt_cache_hit_tokens,
        );
        self.usage.prompt_cache_miss_tokens = add_optional_usage(
            self.usage.prompt_cache_miss_tokens,
            usage.prompt_cache_miss_tokens,
        );
        self.usage.prompt_cache_write_tokens = add_optional_usage(
            self.usage.prompt_cache_write_tokens,
            usage.prompt_cache_write_tokens,
        );
        self.usage.reasoning_tokens =
            add_optional_usage(self.usage.reasoning_tokens, usage.reasoning_tokens);
        self.usage.reasoning_replay_tokens = add_optional_usage(
            self.usage.reasoning_replay_tokens,
            usage.reasoning_replay_tokens,
        );
        if let Some(delta) = usage.server_tool_use.as_ref() {
            let total = self.usage.server_tool_use.get_or_insert_default();
            total.code_execution_requests =
                add_optional_usage(total.code_execution_requests, delta.code_execution_requests);
            total.tool_search_requests =
                add_optional_usage(total.tool_search_requests, delta.tool_search_requests);
        }
    }

    /// Record one parent-route response for both billing and live-context
    /// pressure. Child-model usage must call [`Self::add_usage`] directly so
    /// it cannot masquerade as the parent request's context size.
    pub fn add_parent_usage(&mut self, usage: &Usage) {
        self.latest_parent_input_tokens = (usage.input_tokens > 0).then_some(usage.input_tokens);
        self.add_usage(usage);
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

#[cfg(test)]
mod usage_tests {
    use super::*;
    use crate::models::ServerToolUsage;

    #[test]
    fn add_usage_preserves_replay_and_saturates_server_tool_counters() {
        let mut turn = TurnContext::new(2);
        turn.add_usage(&Usage {
            reasoning_replay_tokens: Some(u32::MAX - 1),
            server_tool_use: Some(ServerToolUsage {
                code_execution_requests: Some(u32::MAX),
                tool_search_requests: Some(2),
            }),
            ..Usage::default()
        });
        turn.add_usage(&Usage {
            reasoning_replay_tokens: Some(9),
            server_tool_use: Some(ServerToolUsage {
                code_execution_requests: Some(1),
                tool_search_requests: Some(3),
            }),
            ..Usage::default()
        });

        assert_eq!(turn.usage.reasoning_replay_tokens, Some(u32::MAX));
        let server = turn.usage.server_tool_use.expect("server tool usage");
        assert_eq!(server.code_execution_requests, Some(u32::MAX));
        assert_eq!(server.tool_search_requests, Some(5));
    }

    fn below_threshold(messages: &[crate::models::Message], turn: &TurnContext) -> bool {
        let config = crate::compaction::CompactionConfig {
            enabled: true,
            token_threshold: 100_000,
            ..Default::default()
        };
        !crate::compaction::compaction_pressure_reached_with_billed(
            messages,
            None,
            &config,
            turn.latest_parent_input_tokens.map(u64::from),
        )
    }

    #[test]
    fn cumulative_low_context_parent_steps_cannot_trigger_compaction() {
        let mut turn = TurnContext::new(4);
        turn.add_parent_usage(&Usage {
            input_tokens: 60_000,
            ..Usage::default()
        });
        turn.add_parent_usage(&Usage {
            input_tokens: 70_000,
            ..Usage::default()
        });

        assert_eq!(turn.usage.input_tokens, 130_000);
        assert_eq!(turn.latest_parent_input_tokens, Some(70_000));
        assert!(below_threshold(&[], &turn));
    }

    #[test]
    fn child_usage_cannot_replace_parent_context_pressure() {
        let mut turn = TurnContext::new(4);
        turn.add_parent_usage(&Usage {
            input_tokens: 70_000,
            ..Usage::default()
        });
        turn.add_usage(&Usage {
            input_tokens: 250_000,
            ..Usage::default()
        });

        assert_eq!(turn.usage.input_tokens, 320_000);
        assert_eq!(turn.latest_parent_input_tokens, Some(70_000));
        assert!(below_threshold(&[], &turn));
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
pub(crate) fn format_snapshot_label(
    prefix: &str,
    turn_seq: u64,
    user_prompt: Option<&str>,
) -> String {
    let base = format!("{prefix}:{turn_seq}");
    match user_prompt {
        None | Some("") => base,
        Some(prompt) => match snapshot_label_prompt_snippet(prompt) {
            None => base,
            Some(snippet) => format!("{base}: {snippet}"),
        },
    }
}

/// The exact prompt snippet [`format_snapshot_label`] embeds after `type:seq`.
///
/// Read surfaces that want to correlate a recorded prompt back to a restore
/// point must go through this function rather than re-deriving the truncation,
/// so the reader and the writer can never disagree about what a label means.
/// Returns `None` when the prompt contributes no snippet at all.
pub(crate) fn snapshot_label_prompt_snippet(prompt: &str) -> Option<String> {
    if prompt.is_empty() {
        return None;
    }
    let first_line = prompt.lines().next().unwrap_or("");
    let truncated: String = first_line.chars().take(USER_PROMPT_LABEL_MAX).collect();
    if truncated.chars().count() < first_line.chars().count() {
        Some(format!("{truncated}…"))
    } else {
        Some(truncated)
    }
}

/// A snapshot label parsed back into its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedSnapshotLabel {
    /// `pre-turn`, `post-turn`, `tool`, or whatever prefix produced it.
    pub kind: String,
    /// The turn sequence for `pre-turn`/`post-turn` labels. `tool` labels
    /// carry a call id rather than a sequence, so this stays `None` for them.
    pub seq: Option<u64>,
    /// The embedded prompt snippet, exactly as
    /// [`snapshot_label_prompt_snippet`] produced it.
    pub prompt_snippet: Option<String>,
}

/// Parse a label produced by [`format_snapshot_label`].
///
/// This is deliberately total: an unrecognized label still yields a record with
/// the raw text as `kind`, because a read surface must describe what is really
/// stored rather than silently dropping rows it does not recognize.
pub(crate) fn parse_snapshot_label(label: &str) -> ParsedSnapshotLabel {
    let (head, snippet) = match label.split_once(": ") {
        Some((head, rest)) => (head, Some(rest.to_string())),
        None => (label, None),
    };
    match head.split_once(':') {
        Some((kind, seq)) => ParsedSnapshotLabel {
            kind: kind.to_string(),
            seq: seq.parse::<u64>().ok(),
            prompt_snippet: snippet,
        },
        None => ParsedSnapshotLabel {
            kind: head.to_string(),
            seq: None,
            prompt_snippet: snippet,
        },
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
    session_id: Option<&str>,
) -> Option<String> {
    snapshot_with_label(
        workspace,
        &format_snapshot_label("pre-turn", turn_seq, user_prompt),
        cap_bytes,
        session_id,
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
pub fn pre_tool_snapshot(
    workspace: &Path,
    call_id: &str,
    cap_bytes: u64,
    session_id: Option<&str>,
) -> Option<String> {
    snapshot_with_label(workspace, &format!("tool:{call_id}"), cap_bytes, session_id)
}

/// Take a `post-turn:<seq>` workspace snapshot. Same failure model as
/// [`pre_turn_snapshot`].
pub fn post_turn_snapshot(
    workspace: &Path,
    turn_seq: u64,
    cap_bytes: u64,
    user_prompt: Option<&str>,
    session_id: Option<&str>,
) -> Option<String> {
    snapshot_with_label(
        workspace,
        &format_snapshot_label("post-turn", turn_seq, user_prompt),
        cap_bytes,
        session_id,
    )
}

fn snapshot_with_label(
    workspace: &Path,
    label: &str,
    cap_bytes: u64,
    session_id: Option<&str>,
) -> Option<String> {
    match SnapshotRepo::open_or_init_with_cap(workspace, cap_bytes) {
        Ok(repo) => {
            let id = match repo.snapshot_with_session(label, session_id) {
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
            maybe_notify_snapshots_disabled_once(workspace, &e);
            None
        }
    }
}

// The stderr print is deliberate: headless/CLI stderr is the user surface for
// this once-per-workspace warning, matching the pre-TUI notices in
// runtime_log.rs.
#[allow(clippy::print_stderr)]
fn maybe_notify_snapshots_disabled_once(workspace: &Path, error: &std::io::Error) {
    let message = error.to_string();
    if !(message.contains("workspace too large for snapshots")
        || message.contains("workspace snapshots are disabled"))
    {
        return;
    }
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static NOTIFIED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let key = workspace.to_string_lossy().into_owned();
    let set = NOTIFIED.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut guard) = set.lock() else {
        return;
    };
    if !guard.insert(key) {
        return;
    }
    // One prominent notice per workspace process lifetime — silent disable is
    // the §2.7 failure mode. Opt-in remains `[snapshots] max_workspace_gb`
    // (raise the cap or set 0 to disable the size gate).
    eprintln!(
        "warning: workspace snapshots/undo are OFF for {}
  {message}
  raise `[snapshots] max_workspace_gb` in config.toml (or set it to 0 to disable the cap) to opt in.",
        workspace.display()
    );
}

#[cfg(test)]
mod snapshot_label_tests {
    use super::*;

    #[test]
    fn label_writer_and_parser_agree_on_prompt_snippet() {
        let prompt = "rename the widget\nsecond line is dropped";
        let label = format_snapshot_label("pre-turn", 7, Some(prompt));
        assert_eq!(label, "pre-turn:7: rename the widget");

        let parsed = parse_snapshot_label(&label);
        assert_eq!(parsed.kind, "pre-turn");
        assert_eq!(parsed.seq, Some(7));
        assert_eq!(
            parsed.prompt_snippet.as_deref(),
            snapshot_label_prompt_snippet(prompt).as_deref(),
            "a reader must recover exactly the snippet the writer embedded"
        );
    }

    #[test]
    fn truncated_prompt_round_trips_with_its_ellipsis() {
        let prompt = "x".repeat(USER_PROMPT_LABEL_MAX + 25);
        let label = format_snapshot_label("post-turn", 2, Some(&prompt));
        let parsed = parse_snapshot_label(&label);
        let snippet = parsed.prompt_snippet.expect("snippet");
        assert!(snippet.ends_with('…'));
        assert_eq!(snippet.chars().count(), USER_PROMPT_LABEL_MAX + 1);
        assert_eq!(
            Some(snippet),
            snapshot_label_prompt_snippet(&prompt),
            "truncated snippets must also round-trip"
        );
    }

    #[test]
    fn labels_without_a_prompt_parse_without_inventing_one() {
        let label = format_snapshot_label("pre-turn", 3, None);
        assert_eq!(label, "pre-turn:3");
        let parsed = parse_snapshot_label(&label);
        assert_eq!(parsed.kind, "pre-turn");
        assert_eq!(parsed.seq, Some(3));
        assert_eq!(parsed.prompt_snippet, None);
    }

    #[test]
    fn tool_labels_carry_a_call_id_not_a_sequence() {
        let label = format!("tool:{}", "call_abc123");
        let parsed = parse_snapshot_label(&label);
        assert_eq!(parsed.kind, "tool");
        assert_eq!(parsed.seq, None, "a call id is not a turn sequence");
        assert_eq!(parsed.prompt_snippet, None);
    }

    #[test]
    fn unrecognized_labels_are_reported_rather_than_dropped() {
        let parsed = parse_snapshot_label("manual checkpoint");
        assert_eq!(parsed.kind, "manual checkpoint");
        assert_eq!(parsed.seq, None);
        assert_eq!(parsed.prompt_snippet, None);
    }

    #[test]
    fn empty_prompt_contributes_no_snippet() {
        assert_eq!(snapshot_label_prompt_snippet(""), None);
        assert_eq!(format_snapshot_label("pre-turn", 1, Some("")), "pre-turn:1");
    }
}
