//! Janitor: stale invalidation, memo cleanup, candidate demotion,
//! trace compaction, and capacity enforcement over TraceStore/memo/overlay
//! state.
//!
//! The janitor operates entirely on owned in-memory state — there is no I/O.
//! The caller decides when to run passes and how to persist results.
//!
//! # Acceptance gate
//! - Capacity limits must be enforced (proven by the `janitor` integration test).
//! - Compaction must preserve the replayability of retained traces.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{
    PromotionGate, ReplayLeafRecord, TeacherCandidate, TeacherCandidateStatus, WorkflowReplayTrace,
    WorkflowRunStatus,
};

// --- Capacity limits ---------------------------------------------------------

/// Capacity limits used by the [`Janitor`].  All sizes are *entry counts*.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JanitorLimits {
    /// Maximum traces retained in the [`TraceStore`].  `0` means unlimited.
    #[serde(default = "default_trace_capacity")]
    pub trace_capacity: usize,
    /// Maximum entries retained in the [`MemoStore`].  `0` means unlimited.
    #[serde(default = "default_memo_capacity")]
    pub memo_capacity: usize,
    /// Maximum entries retained in the [`OverlayStore`].  `0` means unlimited.
    #[serde(default = "default_overlay_capacity")]
    pub overlay_capacity: usize,
    /// Maximum leaf records kept per trace during compaction.
    #[serde(default = "default_compacted_leaf_limit")]
    pub compacted_leaf_limit: usize,
}

fn default_trace_capacity() -> usize {
    256
}
fn default_memo_capacity() -> usize {
    512
}
fn default_overlay_capacity() -> usize {
    128
}
fn default_compacted_leaf_limit() -> usize {
    64
}

impl Default for JanitorLimits {
    fn default() -> Self {
        Self {
            trace_capacity: default_trace_capacity(),
            memo_capacity: default_memo_capacity(),
            overlay_capacity: default_overlay_capacity(),
            compacted_leaf_limit: default_compacted_leaf_limit(),
        }
    }
}

// --- TraceStore --------------------------------------------------------------

/// In-memory store for [`WorkflowReplayTrace`] records.
///
/// Traces are indexed by `trace_id`.  When `insert` would exceed `capacity`
/// the oldest-inserted trace is evicted first (FIFO, tracked by a monotone
/// generation counter).  A capacity of `0` means unlimited.
#[derive(Debug, Clone, Default)]
pub struct TraceStore {
    capacity: usize,
    // (insertion_generation, trace)
    inner: Vec<(u64, WorkflowReplayTrace)>,
    generation: u64,
}

impl TraceStore {
    /// Create a new store limited to `capacity` traces (`0` = unlimited).
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            inner: Vec::new(),
            generation: 0,
        }
    }

    /// Configured capacity (`0` means unlimited).
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of traces currently held.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// True when no traces are held.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Insert a trace, evicting the oldest entry when at capacity.
    ///
    /// Returns the `trace_id` of the evicted trace, if any.
    pub fn insert(&mut self, trace: WorkflowReplayTrace) -> Option<String> {
        let evicted = if self.capacity > 0 && self.inner.len() >= self.capacity {
            self.evict_oldest()
        } else {
            None
        };
        let ins_gen = self.generation;
        self.generation = self.generation.saturating_add(1);
        self.inner.push((ins_gen, trace));
        evicted
    }

    /// Retrieve an immutable reference to a trace by id.
    pub fn get(&self, trace_id: &str) -> Option<&WorkflowReplayTrace> {
        self.inner
            .iter()
            .find(|(_, t)| t.trace_id == trace_id)
            .map(|(_, t)| t)
    }

    /// Remove a trace by id.  Returns the removed trace, or `None`.
    pub fn remove(&mut self, trace_id: &str) -> Option<WorkflowReplayTrace> {
        if let Some(pos) = self.inner.iter().position(|(_, t)| t.trace_id == trace_id) {
            Some(self.inner.remove(pos).1)
        } else {
            None
        }
    }

    /// Iterate over all held trace ids.
    pub fn trace_ids(&self) -> impl Iterator<Item = &str> {
        self.inner.iter().map(|(_, t)| t.trace_id.as_str())
    }

    /// Compact a single trace by retaining at most `leaf_limit` leaf records,
    /// preferring succeeded ones.  Control records are always kept (they are
    /// cheap and required for correct replay).
    ///
    /// Returns `true` when the trace was found.
    pub fn compact_trace(&mut self, trace_id: &str, leaf_limit: usize) -> bool {
        if let Some((_, trace)) = self.inner.iter_mut().find(|(_, t)| t.trace_id == trace_id) {
            compact_trace_leaves(trace, leaf_limit);
            true
        } else {
            false
        }
    }

    /// Compact every trace, keeping at most `leaf_limit` leaf records each.
    pub fn compact_all(&mut self, leaf_limit: usize) {
        for (_, trace) in &mut self.inner {
            compact_trace_leaves(trace, leaf_limit);
        }
    }

    /// Remove all traces that have neither leaf records nor control records
    /// (i.e. traces that can no longer support replay).
    ///
    /// Returns the ids of removed traces.
    pub fn remove_unreplayable(&mut self) -> Vec<String> {
        let stale: Vec<String> = self
            .inner
            .iter()
            .filter(|(_, t)| !is_replayable(t))
            .map(|(_, t)| t.trace_id.clone())
            .collect();
        self.inner.retain(|(_, t)| !stale.contains(&t.trace_id));
        stale
    }

    /// Evict the oldest trace and return its id.
    fn evict_oldest(&mut self) -> Option<String> {
        if self.inner.is_empty() {
            return None;
        }
        let pos = self
            .inner
            .iter()
            .enumerate()
            .min_by_key(|(_, (ins_gen, _))| *ins_gen)
            .map(|(i, _)| i)
            .unwrap();
        let id = self.inner[pos].1.trace_id.clone();
        self.inner.remove(pos);
        Some(id)
    }
}

fn is_replayable(trace: &WorkflowReplayTrace) -> bool {
    !trace.leaf_records.is_empty() || !trace.control_records.is_empty()
}

/// Keep at most `limit` leaf records, preferring succeeded results.
///
/// Within each tier the original insertion order is preserved so that replay
/// sees the same record sequence it recorded.
fn compact_trace_leaves(trace: &mut WorkflowReplayTrace, limit: usize) {
    if limit == 0 || trace.leaf_records.len() <= limit {
        return;
    }
    let (mut succeeded, mut other): (Vec<ReplayLeafRecord>, Vec<ReplayLeafRecord>) = trace
        .leaf_records
        .drain(..)
        .partition(|r| r.result.status == WorkflowRunStatus::Succeeded);

    succeeded.truncate(limit);
    let remaining = limit.saturating_sub(succeeded.len());
    other.truncate(remaining);
    succeeded.append(&mut other);
    trace.leaf_records = succeeded;
}

// --- MemoStore ---------------------------------------------------------------

/// A single memoised result entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoEntry {
    /// Stable lookup key (e.g. an input hash).
    pub key: String,
    /// Opaque serialised payload.
    pub value: String,
    /// Hit count since last insert / refresh.
    pub hits: u64,
    /// True when this entry has been explicitly marked stale.
    pub stale: bool,
}

/// In-memory store for [`MemoEntry`] records.
///
/// Capacity is enforced on `insert`: stale entries are evicted first, then
/// the least-hit entry.  A capacity of `0` means unlimited.
#[derive(Debug, Clone, Default)]
pub struct MemoStore {
    capacity: usize,
    entries: HashMap<String, MemoEntry>,
}

impl MemoStore {
    /// Create a new store limited to `capacity` entries (`0` = unlimited).
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
        }
    }

    /// Configured capacity (`0` means unlimited).
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of entries currently held.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when no entries are held.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Insert or replace an entry.
    ///
    /// Returns the previous entry for the same key if one existed.
    pub fn insert(
        &mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Option<MemoEntry> {
        let key = key.into();
        if self.capacity > 0
            && !self.entries.contains_key(&key)
            && self.entries.len() >= self.capacity
        {
            self.evict_one();
        }
        self.entries.insert(
            key.clone(),
            MemoEntry {
                key,
                value: value.into(),
                hits: 0,
                stale: false,
            },
        )
    }

    /// Look up an entry and increment its hit counter.
    pub fn get_hit(&mut self, key: &str) -> Option<&MemoEntry> {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.hits = entry.hits.saturating_add(1);
            Some(entry)
        } else {
            None
        }
    }

    /// Look up an entry without mutating the hit counter.
    pub fn peek(&self, key: &str) -> Option<&MemoEntry> {
        self.entries.get(key)
    }

    /// Mark an entry stale.  Returns `true` when the entry was found.
    pub fn mark_stale(&mut self, key: &str) -> bool {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.stale = true;
            true
        } else {
            false
        }
    }

    /// Remove all stale entries.  Returns the count removed.
    pub fn purge_stale(&mut self) -> usize {
        let before = self.entries.len();
        self.entries.retain(|_, e| !e.stale);
        before - self.entries.len()
    }

    /// Remove every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    fn evict_one(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        // Prefer stale entries; otherwise evict the least-hit.
        let key = self
            .entries
            .iter()
            .filter(|(_, e)| e.stale)
            .next()
            .map(|(k, _)| k.clone())
            .or_else(|| {
                self.entries
                    .iter()
                    .min_by_key(|(_, e)| e.hits)
                    .map(|(k, _)| k.clone())
            });
        if let Some(k) = key {
            self.entries.remove(&k);
        }
    }
}

// --- OverlayStore ------------------------------------------------------------

/// A single entry in the cached-main overlay.
///
/// The overlay holds promoted lessons after review and replay.  It is
/// inspectable and reversible and must never mutate Git `main`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayEntry {
    /// Stable entry identifier.
    pub entry_id: String,
    /// The candidate that sourced this overlay entry.
    pub source_candidate_id: String,
    /// Opaque lesson payload (prompt patch, note, etc.).
    pub payload: String,
    /// True when the originating candidate has been demoted or superseded.
    pub invalidated: bool,
}

/// In-memory store for [`OverlayEntry`] records.
///
/// Capacity is enforced on `insert`: invalidated entries are evicted first
/// (oldest first), then valid ones.  A capacity of `0` means unlimited.
#[derive(Debug, Clone, Default)]
pub struct OverlayStore {
    capacity: usize,
    // (insertion_generation, entry)
    inner: Vec<(u64, OverlayEntry)>,
    generation: u64,
}

impl OverlayStore {
    /// Create a new store limited to `capacity` entries (`0` = unlimited).
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            inner: Vec::new(),
            generation: 0,
        }
    }

    /// Configured capacity (`0` means unlimited).
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of entries currently held.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// True when no entries are held.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Insert an entry, evicting as needed to stay within capacity.
    ///
    /// Returns the `entry_id` of the evicted entry, if any.
    pub fn insert(&mut self, entry: OverlayEntry) -> Option<String> {
        let evicted = if self.capacity > 0 && self.inner.len() >= self.capacity {
            self.evict_one()
        } else {
            None
        };
        let ins_gen = self.generation;
        self.generation = self.generation.saturating_add(1);
        self.inner.push((ins_gen, entry));
        evicted
    }

    /// Retrieve an immutable reference to an entry by id.
    pub fn get(&self, entry_id: &str) -> Option<&OverlayEntry> {
        self.inner
            .iter()
            .find(|(_, e)| e.entry_id == entry_id)
            .map(|(_, e)| e)
    }

    /// Remove an entry by id.
    pub fn remove(&mut self, entry_id: &str) -> Option<OverlayEntry> {
        if let Some(pos) = self.inner.iter().position(|(_, e)| e.entry_id == entry_id) {
            Some(self.inner.remove(pos).1)
        } else {
            None
        }
    }

    /// Mark as invalidated every overlay entry whose `source_candidate_id`
    /// matches one of the given demoted candidate ids.
    ///
    /// Returns the count of entries newly marked (already-invalidated entries
    /// are not counted again).
    pub fn invalidate_from_candidates(&mut self, demoted_ids: &[&str]) -> usize {
        let mut count = 0;
        for (_, entry) in &mut self.inner {
            if !entry.invalidated && demoted_ids.contains(&entry.source_candidate_id.as_str()) {
                entry.invalidated = true;
                count += 1;
            }
        }
        count
    }

    /// Remove all invalidated entries.  Returns the count removed.
    pub fn purge_invalidated(&mut self) -> usize {
        let before = self.inner.len();
        self.inner.retain(|(_, e)| !e.invalidated);
        before - self.inner.len()
    }

    fn evict_one(&mut self) -> Option<String> {
        if self.inner.is_empty() {
            return None;
        }
        // Prefer the oldest invalidated entry; fall back to the oldest valid.
        let pos = self
            .inner
            .iter()
            .enumerate()
            .filter(|(_, (_, e))| e.invalidated)
            .min_by_key(|(_, (ins_gen, _))| *ins_gen)
            .map(|(i, _)| i)
            .or_else(|| {
                self.inner
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, (ins_gen, _))| *ins_gen)
                    .map(|(i, _)| i)
            });
        pos.map(|i| {
            let id = self.inner[i].1.entry_id.clone();
            self.inner.remove(i);
            id
        })
    }
}

// --- Candidate demotion ------------------------------------------------------

/// Demote candidates using the given `PromotionGate`.
///
/// A candidate is moved to [`TeacherCandidateStatus::Rejected`] when:
/// - its last replay result is marked stale, **or**
/// - the gate evaluates it as not promoted.
///
/// Candidates already in `Promoted` status are never touched.
///
/// Returns the `candidate_id`s of newly-demoted candidates.
pub fn demote_stale_candidates(
    candidates: &mut [TeacherCandidate],
    gate: &PromotionGate,
) -> Vec<String> {
    let mut demoted = Vec::new();
    for candidate in candidates.iter_mut() {
        if candidate.status == TeacherCandidateStatus::Promoted {
            continue;
        }
        let last_stale = candidate.replay_results.last().is_some_and(|r| r.stale);
        if last_stale {
            if candidate.status != TeacherCandidateStatus::Rejected {
                candidate.status = TeacherCandidateStatus::Rejected;
                demoted.push(candidate.candidate_id.clone());
            }
            continue;
        }
        let decision = gate.evaluate_candidate(candidate);
        if !decision.promoted() && candidate.status != TeacherCandidateStatus::Rejected {
            candidate.status = TeacherCandidateStatus::Rejected;
            demoted.push(candidate.candidate_id.clone());
        }
    }
    demoted
}

// --- Janitor -----------------------------------------------------------------

/// Top-level coordinator that runs all cleanup passes.
///
/// The janitor is stateless — it receives mutable references to the stores it
/// manages and returns a [`JanitorReport`] describing the work done.
#[derive(Debug, Clone, Default)]
pub struct Janitor {
    /// Capacity and compaction limits.
    pub limits: JanitorLimits,
}

impl Janitor {
    /// Create a janitor with explicit limits.
    pub fn new(limits: JanitorLimits) -> Self {
        Self { limits }
    }

    /// Run a full janitor pass in the following order:
    ///
    /// 1. Demote stale/rejected candidates via the `gate`.
    /// 2. Propagate demotion into `overlay` and purge invalidated entries.
    /// 3. Compact all traces in `traces` to `limits.compacted_leaf_limit`.
    /// 4. Remove unreplayable traces.
    /// 5. Purge stale entries from `memos`.
    /// 6. Re-enforce `limits.trace_capacity` (handles post-construction limit
    ///    changes by evicting oldest entries until the store fits).
    pub fn run(
        &self,
        candidates: &mut Vec<TeacherCandidate>,
        gate: &PromotionGate,
        traces: &mut TraceStore,
        memos: &mut MemoStore,
        overlay: &mut OverlayStore,
    ) -> JanitorReport {
        // 1. Demote stale candidates.
        let candidates_demoted = demote_stale_candidates(candidates, gate);

        // 2. Propagate demotion into overlay.
        let demoted_refs: Vec<&str> = candidates_demoted.iter().map(String::as_str).collect();
        let overlay_entries_invalidated = overlay.invalidate_from_candidates(&demoted_refs);
        let overlay_entries_purged = overlay.purge_invalidated();

        // 3. Compact traces.
        traces.compact_all(self.limits.compacted_leaf_limit);

        // 4. Remove unreplayable traces.
        let unreplayable_traces_removed = traces.remove_unreplayable();

        // 5. Purge stale memo entries.
        let memo_entries_purged = memos.purge_stale();

        // 6. Re-enforce trace capacity.
        let mut extra_trace_evictions = Vec::new();
        while self.limits.trace_capacity > 0 && traces.len() > self.limits.trace_capacity {
            if let Some(id) = traces.evict_oldest() {
                extra_trace_evictions.push(id);
            } else {
                break;
            }
        }

        JanitorReport {
            candidates_demoted,
            overlay_entries_invalidated,
            overlay_entries_purged,
            traces_compacted: traces.len(),
            unreplayable_traces_removed,
            memo_entries_purged,
            extra_trace_evictions,
        }
    }
}

/// Summary of what a single janitor pass performed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JanitorReport {
    /// Ids of candidates demoted to `Rejected`.
    pub candidates_demoted: Vec<String>,
    /// Count of overlay entries newly marked invalidated.
    pub overlay_entries_invalidated: usize,
    /// Count of overlay entries purged.
    pub overlay_entries_purged: usize,
    /// Count of traces remaining after compaction.
    pub traces_compacted: usize,
    /// Ids of traces removed because they became unreplayable.
    pub unreplayable_traces_removed: Vec<String>,
    /// Count of memo entries purged.
    pub memo_entries_purged: usize,
    /// Ids of traces evicted during the capacity re-enforcement step.
    pub extra_trace_evictions: Vec<String>,
}
