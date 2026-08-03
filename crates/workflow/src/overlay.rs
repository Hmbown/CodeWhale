//! Cached-main overlay: promoted lessons that warm future runs without
//! mutating Git main.
//!
//! ## What lives here
//!
//! Pure IR — serialisable entry types and the telemetry diff that records
//! whether a run consumed overlay entries.  File-system persistence
//! (`OverlayStore`) lives in `codewhale-lane` where IO dependencies are
//! already present.
//!
//! ## Overlay semantics (RFC `WORKFLOW_EXTERNAL_MEMORY.md`)
//!
//! | Property | Value |
//! |---|---|
//! | Scope | `$CODEWHALE_HOME/overlay/` — per-user, not per-repo |
//! | Mutates Git main? | No |
//! | Revertable? | Yes — `OverlayStore::remove` |
//! | Attributable? | Yes — every entry carries the promoting lane/run id |
//! | Telemetry diffable? | Yes — `OverlayRunDiff` records active-vs-baseline |

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::WorkflowUsage;

/// What kind of lesson/patch an overlay entry carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayEntryKind {
    /// A free-form promoted note — lessons learned, review observations.
    Note,
    /// A refined workflow definition extracted from a successful run.
    Workflow,
    /// A test snippet or test-fixture patch validated by the promoting run.
    Test,
    /// A branch-selection heuristic (e.g. "prefer fast-path for X").
    BranchHeuristic,
    /// A model or provider cache policy override.
    ModelCachePolicy,
    /// A prompt patch (prefix/suffix/injection) for a specific agent role.
    PromptPatch,
}

impl OverlayEntryKind {
    /// Stable string tag used in file names and telemetry labels.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Workflow => "workflow",
            Self::Test => "test",
            Self::BranchHeuristic => "branch_heuristic",
            Self::ModelCachePolicy => "model_cache_policy",
            Self::PromptPatch => "prompt_patch",
        }
    }
}

/// One entry in the cached-main overlay.
///
/// Each entry is attributable to the lane run that promoted it, immutable
/// after promotion (replace by remove + re-add), and independently removable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayEntry {
    /// Stable unique id for this entry (opaque string, typically a UUID).
    pub id: String,
    /// Content category.
    pub kind: OverlayEntryKind,
    /// The promoted content — plain text, TOML snippet, JSON object, etc.
    pub content: String,
    /// Lane / run id that promoted this entry.  Used for attribution and
    /// batch-remove by run.
    pub promoting_run_id: String,
    /// ISO 8601 wall-clock timestamp when the entry was promoted.
    pub promoted_at: String,
    /// Optional human-readable description of what this entry does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Arbitrary tags for filtering (e.g. `["v0.9", "perf"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl OverlayEntry {
    /// Construct a minimal entry with required fields.
    pub fn new(
        id: impl Into<String>,
        kind: OverlayEntryKind,
        content: impl Into<String>,
        promoting_run_id: impl Into<String>,
        promoted_at: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind,
            content: content.into(),
            promoting_run_id: promoting_run_id.into(),
            promoted_at: promoted_at.into(),
            description: None,
            tags: Vec::new(),
        }
    }

    /// Attach a description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Attach tags.
    #[must_use]
    pub fn with_tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }
}

/// Telemetry signal indicating whether the cached-main overlay was active
/// during a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "overlay_active")]
pub enum OverlaySignal {
    /// No overlay entries were loaded; this run is a clean baseline.
    #[serde(rename = "false")]
    Off,
    /// Overlay entries were active; `entry_count` is the number applied.
    #[serde(rename = "true")]
    On {
        /// Number of overlay entries that were in scope for this run.
        entry_count: usize,
    },
}

impl OverlaySignal {
    /// Whether any overlay entries were active.
    pub fn is_active(self) -> bool {
        matches!(self, Self::On { .. })
    }
}

/// Telemetry diff between a run that consumed overlay entries and an optional
/// baseline run that did not.
///
/// This type is the primary artefact for answering "did the overlay help?" —
/// callers compare `with_overlay` usage against `baseline_usage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayRunDiff {
    /// Identifier of the run for which this diff was produced.
    pub run_id: String,
    /// Overlay state during this run.
    pub signal: OverlaySignal,
    /// Ids of the overlay entries that were active during this run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_entry_ids: Vec<String>,
    /// Token / cost usage observed in the run (may have benefited from overlay).
    pub run_usage: WorkflowUsage,
    /// Usage from an earlier baseline run without the overlay, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_usage: Option<WorkflowUsage>,
}

impl OverlayRunDiff {
    /// Construct a diff for a run that executed **without** any overlay.
    pub fn baseline(run_id: impl Into<String>, usage: WorkflowUsage) -> Self {
        Self {
            run_id: run_id.into(),
            signal: OverlaySignal::Off,
            active_entry_ids: Vec::new(),
            run_usage: usage,
            baseline_usage: None,
        }
    }

    /// Construct a diff for a run that executed **with** overlay entries.
    pub fn with_overlay(
        run_id: impl Into<String>,
        entry_ids: Vec<String>,
        usage: WorkflowUsage,
        baseline: Option<WorkflowUsage>,
    ) -> Self {
        let entry_count = entry_ids.len();
        Self {
            run_id: run_id.into(),
            signal: OverlaySignal::On { entry_count },
            active_entry_ids: entry_ids,
            run_usage: usage,
            baseline_usage: baseline,
        }
    }

    /// Estimated token savings vs baseline (`None` if baseline is absent or
    /// either total is unknown).
    pub fn estimated_token_savings(&self) -> Option<i64> {
        let baseline = self.baseline_usage.as_ref()?;
        let with_tokens = self.run_usage.total_tokens()? as i64;
        let baseline_tokens = baseline.total_tokens()? as i64;
        Some(baseline_tokens - with_tokens)
    }
}

/// Parsing errors for overlay entry content.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OverlayError {
    #[error("overlay entry id must not be empty")]
    EmptyId,
    #[error("promoting_run_id must not be empty")]
    EmptyRunId,
    #[error("overlay entry content must not be empty")]
    EmptyContent,
    #[error("overlay entry id `{id}` contains path-traversal characters")]
    UnsafeId { id: String },
}

/// Validate an overlay entry for basic well-formedness.
///
/// This is a pure function — no IO — so it belongs in the workflow IR crate.
pub fn validate_overlay_entry(entry: &OverlayEntry) -> Result<(), OverlayError> {
    if entry.id.is_empty() {
        return Err(OverlayError::EmptyId);
    }
    if entry.promoting_run_id.is_empty() {
        return Err(OverlayError::EmptyRunId);
    }
    if entry.content.is_empty() {
        return Err(OverlayError::EmptyContent);
    }
    // Reject path-traversal characters in the id so the store can use it
    // safely as a file-name component.
    if entry
        .id
        .contains(|c: char| c == '/' || c == '\\' || c == '\0')
    {
        return Err(OverlayError::UnsafeId {
            id: entry.id.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkflowUsage;

    fn make_entry(id: &str) -> OverlayEntry {
        OverlayEntry::new(
            id,
            OverlayEntryKind::Note,
            "content",
            "run-123",
            "2026-08-03T00:00:00Z",
        )
    }

    #[test]
    fn entry_builder_roundtrips() {
        let entry = make_entry("abc")
            .with_description("a promoted note")
            .with_tags(["v0.9", "perf"]);

        assert_eq!(entry.id, "abc");
        assert_eq!(entry.description.as_deref(), Some("a promoted note"));
        assert_eq!(entry.tags, ["v0.9", "perf"]);
    }

    #[test]
    fn entry_serialises_and_deserialises() {
        let entry = make_entry("round-trip");
        let json = serde_json::to_string(&entry).expect("serialise");
        let back: OverlayEntry = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(entry, back);
    }

    #[test]
    fn validate_accepts_valid_entry() {
        assert!(validate_overlay_entry(&make_entry("ok-id")).is_ok());
    }

    #[test]
    fn validate_rejects_empty_id() {
        assert!(matches!(
            validate_overlay_entry(&make_entry("")),
            Err(OverlayError::EmptyId)
        ));
    }

    #[test]
    fn validate_rejects_path_traversal() {
        assert!(matches!(
            validate_overlay_entry(&make_entry("../escape")),
            Err(OverlayError::UnsafeId { .. })
        ));
    }

    #[test]
    fn validate_rejects_empty_run_id() {
        let mut entry = make_entry("x");
        entry.promoting_run_id = String::new();
        assert!(matches!(
            validate_overlay_entry(&entry),
            Err(OverlayError::EmptyRunId)
        ));
    }

    #[test]
    fn validate_rejects_empty_content() {
        let mut entry = make_entry("x");
        entry.content = String::new();
        assert!(matches!(
            validate_overlay_entry(&entry),
            Err(OverlayError::EmptyContent)
        ));
    }

    #[test]
    fn overlay_signal_is_active() {
        assert!(!OverlaySignal::Off.is_active());
        assert!(OverlaySignal::On { entry_count: 2 }.is_active());
    }

    #[test]
    fn overlay_run_diff_baseline() {
        let usage = WorkflowUsage {
            input_tokens: Some(100),
            output_tokens: Some(50),
            cost_microusd: Some(10),
        };
        let diff = OverlayRunDiff::baseline("run-1", usage.clone());
        assert!(!diff.signal.is_active());
        assert!(diff.active_entry_ids.is_empty());
        assert_eq!(diff.run_usage, usage);
        assert!(diff.baseline_usage.is_none());
        assert!(diff.estimated_token_savings().is_none());
    }

    #[test]
    fn overlay_run_diff_with_overlay_savings() {
        let baseline = WorkflowUsage {
            input_tokens: Some(200),
            output_tokens: Some(100),
            cost_microusd: Some(30),
        };
        let with_overlay = WorkflowUsage {
            input_tokens: Some(80),
            output_tokens: Some(40),
            cost_microusd: Some(12),
        };
        let diff = OverlayRunDiff::with_overlay(
            "run-2",
            vec!["e1".to_string(), "e2".to_string()],
            with_overlay,
            Some(baseline),
        );
        assert!(diff.signal.is_active());
        assert_eq!(diff.active_entry_ids.len(), 2);
        // savings = 300 - 120 = 180
        assert_eq!(diff.estimated_token_savings(), Some(180));
    }

    #[test]
    fn overlay_entry_kind_as_str() {
        assert_eq!(OverlayEntryKind::Note.as_str(), "note");
        assert_eq!(OverlayEntryKind::Workflow.as_str(), "workflow");
        assert_eq!(OverlayEntryKind::Test.as_str(), "test");
        assert_eq!(
            OverlayEntryKind::BranchHeuristic.as_str(),
            "branch_heuristic"
        );
        assert_eq!(
            OverlayEntryKind::ModelCachePolicy.as_str(),
            "model_cache_policy"
        );
        assert_eq!(OverlayEntryKind::PromptPatch.as_str(), "prompt_patch");
    }
}
