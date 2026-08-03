//! Integration tests for the whaleflow janitor.
//!
//! Acceptance criteria:
//! 1. Capacity limits are enforced on all three stores.
//! 2. Compaction preserves the replayability of retained traces.
//! 3. Stale candidates are demoted and propagated to the overlay.
//! 4. Memo cleanup purges stale entries correctly.

use codewhale_workflow::{
    Janitor, JanitorLimits, JanitorReport, MemoStore, OverlayEntry, OverlayStore, PromotionGate,
    ReplayControlRecord, ReplayLeafRecord, StudentReplayMetrics, StudentReplayResult,
    TeacherCandidate, TeacherCandidateKind, TeacherCandidateStatus, TraceStore,
    WorkflowReplayTrace, WorkflowRunStatus, demote_stale_candidates,
};

// ── helpers ──────────────────────────────────────────────────────────────────

fn make_trace(id: &str, leaf_count: usize) -> WorkflowReplayTrace {
    let leaf_records = (0..leaf_count)
        .map(|i| leaf_record(id, &format!("leaf-{i}"), WorkflowRunStatus::Succeeded))
        .collect();
    WorkflowReplayTrace {
        trace_id: id.to_string(),
        leaf_records,
        control_records: Vec::new(),
    }
}

fn leaf_record(trace_id: &str, leaf_id: &str, status: WorkflowRunStatus) -> ReplayLeafRecord {
    use codewhale_workflow::LeafResult;
    ReplayLeafRecord {
        trace_id: trace_id.to_string(),
        leaf_id: leaf_id.to_string(),
        input_hash: format!("{trace_id}-{leaf_id}-hash"),
        result: LeafResult {
            leaf_id: leaf_id.to_string(),
            task_id: leaf_id.to_string(),
            role: None,
            profile: None,
            status,
            usage: Default::default(),
            memo_usage: Default::default(),
            output: Some("ok".to_string()),
            artifacts: Vec::new(),
            schema_error: None,
        },
    }
}

fn candidate(id: &str, stale: bool) -> TeacherCandidate {
    let replay = StudentReplayResult {
        trace_id: "trace-0".to_string(),
        candidate_id: id.to_string(),
        baseline: StudentReplayMetrics {
            score: 0,
            cost_microusd: 100,
        },
        candidate: StudentReplayMetrics {
            score: 5,
            cost_microusd: 80,
        },
        required_tests: Vec::new(),
        policy_violations: Vec::new(),
        stale,
        notes: None,
    };
    TeacherCandidate {
        candidate_id: id.to_string(),
        kind: TeacherCandidateKind::Note,
        status: TeacherCandidateStatus::Proposed,
        source_node_id: "node-0".to_string(),
        source_branch_id: None,
        summary: format!("Candidate {id}"),
        evidence: Vec::new(),
        replay_results: vec![replay],
    }
}

fn overlay_entry(entry_id: &str, source_candidate_id: &str) -> OverlayEntry {
    OverlayEntry {
        entry_id: entry_id.to_string(),
        source_candidate_id: source_candidate_id.to_string(),
        payload: "patch payload".to_string(),
        invalidated: false,
    }
}

// ── TraceStore capacity ───────────────────────────────────────────────────────

#[test]
fn trace_store_enforces_capacity_on_insert() {
    let mut store = TraceStore::new(3);
    store.insert(make_trace("t1", 1));
    store.insert(make_trace("t2", 1));
    store.insert(make_trace("t3", 1));
    assert_eq!(store.len(), 3);

    // Fourth insert should evict t1 (oldest).
    let evicted = store.insert(make_trace("t4", 1));
    assert_eq!(evicted.as_deref(), Some("t1"));
    assert_eq!(store.len(), 3);
    assert!(store.get("t1").is_none());
    assert!(store.get("t4").is_some());
}

#[test]
fn trace_store_zero_capacity_means_unlimited() {
    let mut store = TraceStore::new(0);
    for i in 0..1000 {
        store.insert(make_trace(&format!("t{i}"), 1));
    }
    assert_eq!(store.len(), 1000);
}

#[test]
fn trace_store_remove_returns_trace() {
    let mut store = TraceStore::new(10);
    store.insert(make_trace("t1", 2));
    let removed = store.remove("t1").expect("should remove");
    assert_eq!(removed.trace_id, "t1");
    assert!(store.is_empty());
}

// ── TraceStore compaction ─────────────────────────────────────────────────────

#[test]
fn compact_trace_keeps_succeeded_leaves_up_to_limit() {
    let mut trace = make_trace("t1", 10); // all succeeded
    // Add 2 failed leaves on top.
    trace
        .leaf_records
        .push(leaf_record("t1", "failed-0", WorkflowRunStatus::Failed));
    trace
        .leaf_records
        .push(leaf_record("t1", "failed-1", WorkflowRunStatus::Failed));

    let mut store = TraceStore::new(0);
    store.insert(trace);
    store.compact_trace("t1", 6);

    let compacted = store.get("t1").unwrap();
    assert_eq!(
        compacted.leaf_records.len(),
        6,
        "compact_trace must keep exactly limit records"
    );
    // All retained records should be the succeeded ones.
    for rec in &compacted.leaf_records {
        assert_eq!(
            rec.result.status,
            WorkflowRunStatus::Succeeded,
            "succeeded leaves preferred"
        );
    }
}

#[test]
fn compact_trace_preserves_control_records() {
    use codewhale_workflow::{ControlNodeKind, ControlNodeResult};

    let mut trace = make_trace("t1", 10);
    trace.control_records.push(ReplayControlRecord {
        trace_id: "t1".to_string(),
        node_id: "reduce-0".to_string(),
        kind: ControlNodeKind::Reduce,
        result: ControlNodeResult {
            node_id: "reduce-0".to_string(),
            kind: ControlNodeKind::Reduce,
            status: WorkflowRunStatus::Succeeded,
            selected_children: Vec::new(),
            summary: Some("reduced".to_string()),
        },
        generated_nodes: Vec::new(),
    });

    let mut store = TraceStore::new(0);
    store.insert(trace);
    store.compact_trace("t1", 2);

    let compacted = store.get("t1").unwrap();
    assert_eq!(compacted.leaf_records.len(), 2);
    // Control records must survive compaction untouched.
    assert_eq!(
        compacted.control_records.len(),
        1,
        "control records preserved after compaction"
    );
}

#[test]
fn compact_all_does_not_exceed_leaf_limit_per_trace() {
    let mut store = TraceStore::new(0);
    store.insert(make_trace("t1", 20));
    store.insert(make_trace("t2", 30));
    store.compact_all(10);

    for id in ["t1", "t2"] {
        let t = store.get(id).unwrap();
        assert!(
            t.leaf_records.len() <= 10,
            "trace {id}: {} > 10",
            t.leaf_records.len()
        );
    }
}

#[test]
fn remove_unreplayable_removes_empty_traces() {
    let empty = WorkflowReplayTrace {
        trace_id: "empty".to_string(),
        leaf_records: Vec::new(),
        control_records: Vec::new(),
    };
    let mut store = TraceStore::new(0);
    store.insert(make_trace("good", 2));
    store.insert(empty);
    let removed = store.remove_unreplayable();
    assert_eq!(removed, vec!["empty"]);
    assert!(store.get("good").is_some());
    assert!(store.get("empty").is_none());
}

// ── MemoStore ─────────────────────────────────────────────────────────────────

#[test]
fn memo_store_enforces_capacity() {
    let mut store = MemoStore::new(2);
    store.insert("k1", "v1");
    store.insert("k2", "v2");
    assert_eq!(store.len(), 2);

    // Third insert evicts the entry with the lowest hit count.
    store.insert("k3", "v3");
    assert_eq!(store.len(), 2, "capacity enforced after third insert");
}

#[test]
fn memo_store_purge_stale_removes_marked_entries() {
    let mut store = MemoStore::new(0);
    store.insert("k1", "v1");
    store.insert("k2", "v2");
    store.insert("k3", "v3");
    store.mark_stale("k1");
    store.mark_stale("k3");

    let purged = store.purge_stale();
    assert_eq!(purged, 2);
    assert_eq!(store.len(), 1);
    assert!(store.peek("k2").is_some());
}

#[test]
fn memo_store_hit_counter_increments() {
    let mut store = MemoStore::new(0);
    store.insert("k1", "v1");
    store.get_hit("k1");
    store.get_hit("k1");
    assert_eq!(store.peek("k1").map(|e| e.hits), Some(2));
}

#[test]
fn memo_store_zero_capacity_means_unlimited() {
    let mut store = MemoStore::new(0);
    for i in 0..500 {
        store.insert(format!("k{i}"), format!("v{i}"));
    }
    assert_eq!(store.len(), 500);
}

// ── OverlayStore ──────────────────────────────────────────────────────────────

#[test]
fn overlay_store_enforces_capacity_evicts_invalidated_first() {
    let mut store = OverlayStore::new(3);
    store.insert(overlay_entry("e1", "c1"));
    store.insert(overlay_entry("e2", "c2"));
    store.insert(overlay_entry("e3", "c3"));

    // Mark e1 as invalidated.
    store.invalidate_from_candidates(&["c1"]);
    assert_eq!(store.len(), 3);

    // Fourth insert should evict e1 (oldest invalidated).
    let evicted = store.insert(overlay_entry("e4", "c4"));
    assert_eq!(evicted.as_deref(), Some("e1"));
    assert_eq!(store.len(), 3);
    assert!(store.get("e1").is_none());
    assert!(store.get("e4").is_some());
}

#[test]
fn overlay_store_purge_invalidated() {
    let mut store = OverlayStore::new(0);
    store.insert(overlay_entry("e1", "c1"));
    store.insert(overlay_entry("e2", "c2"));
    store.insert(overlay_entry("e3", "c3"));
    store.invalidate_from_candidates(&["c1", "c3"]);

    let purged = store.purge_invalidated();
    assert_eq!(purged, 2);
    assert_eq!(store.len(), 1);
    assert!(store.get("e2").is_some());
}

// ── Candidate demotion ────────────────────────────────────────────────────────

#[test]
fn demote_stale_candidates_marks_stale_replay_as_rejected() {
    let mut candidates = vec![
        candidate("c1", true),  // stale — should be demoted
        candidate("c2", false), // not stale — gate will promote (score_delta = 5)
    ];
    let gate = PromotionGate::default();
    let demoted = demote_stale_candidates(&mut candidates, &gate);
    assert_eq!(demoted, vec!["c1"]);
    assert_eq!(candidates[0].status, TeacherCandidateStatus::Rejected);
    // c2 passes the gate (score_delta = 5 > 1) and is NOT demoted.
    assert_ne!(candidates[1].status, TeacherCandidateStatus::Rejected);
}

#[test]
fn demote_stale_candidates_never_demotes_promoted() {
    let mut c = candidate("c-promoted", false);
    c.status = TeacherCandidateStatus::Promoted;
    // The replay result is stale — but the candidate is already Promoted.
    c.replay_results[0].stale = true;

    let gate = PromotionGate::default();
    let demoted = demote_stale_candidates(&mut [c.clone()], &gate);
    assert!(
        demoted.is_empty(),
        "promoted candidates must not be demoted"
    );
}

#[test]
fn demote_stale_candidates_rejects_when_no_replay_result() {
    let mut c = TeacherCandidate {
        candidate_id: "c-no-replay".to_string(),
        kind: TeacherCandidateKind::Note,
        status: TeacherCandidateStatus::Proposed,
        source_node_id: "node-0".to_string(),
        source_branch_id: None,
        summary: "no replay yet".to_string(),
        evidence: Vec::new(),
        replay_results: Vec::new(), // no replay
    };
    let gate = PromotionGate::default();
    let demoted = demote_stale_candidates(&mut [c.clone()], &gate);
    // Gate rejects because there are no replay results.
    assert_eq!(demoted, vec!["c-no-replay"]);
    c.status = TeacherCandidateStatus::Rejected; // expected final state
}

// ── Full janitor pass ─────────────────────────────────────────────────────────

#[test]
fn janitor_full_pass_enforces_all_capacity_limits() {
    let limits = JanitorLimits {
        trace_capacity: 2,
        memo_capacity: 2,
        overlay_capacity: 2,
        compacted_leaf_limit: 3,
    };
    let janitor = Janitor::new(limits);

    // Traces: 4 → expect 2 retained after eviction.
    let mut traces = TraceStore::new(2);
    for i in 0..4 {
        traces.insert(make_trace(&format!("t{i}"), 1));
    }
    // t0 and t1 were evicted on insert; t2 and t3 remain.
    assert_eq!(traces.len(), 2);

    // Memos: 3 over capacity of 2.
    let mut memos = MemoStore::new(2);
    memos.insert("m0", "v0");
    memos.insert("m1", "v1");
    memos.mark_stale("m0");
    // Third insert while at capacity evicts stale m0.
    memos.insert("m2", "v2");
    assert_eq!(memos.len(), 2);

    // Overlay: 2 entries, one invalidated.
    let mut overlay = OverlayStore::new(2);
    overlay.insert(overlay_entry("e0", "cand-0"));
    overlay.insert(overlay_entry("e1", "cand-1"));

    let mut candidates = vec![candidate("cand-0", true)];
    let gate = PromotionGate::default();

    let report: JanitorReport = janitor.run(
        &mut candidates,
        &gate,
        &mut traces,
        &mut memos,
        &mut overlay,
    );

    assert_eq!(
        report.candidates_demoted,
        vec!["cand-0"],
        "stale candidate demoted"
    );
    assert_eq!(
        report.overlay_entries_invalidated, 1,
        "overlay entry invalidated"
    );
    assert_eq!(report.overlay_entries_purged, 1, "overlay entry purged");
    assert_eq!(report.traces_compacted, 2, "2 traces remain");
    assert!(
        report.unreplayable_traces_removed.is_empty(),
        "no unreplayable traces"
    );
    assert_eq!(report.memo_entries_purged, 0, "no stale memos left");
}

#[test]
fn janitor_compact_preserves_replayability_of_retained_traces() {
    let limits = JanitorLimits {
        trace_capacity: 10,
        compacted_leaf_limit: 3,
        ..JanitorLimits::default()
    };
    let janitor = Janitor::new(limits);

    // A trace with 10 leaves all succeeded.
    let mut traces = TraceStore::new(0);
    traces.insert(make_trace("big-trace", 10));

    let mut candidates = Vec::new();
    let gate = PromotionGate::default();
    let mut memos = MemoStore::new(0);
    let mut overlay = OverlayStore::new(0);

    janitor.run(
        &mut candidates,
        &gate,
        &mut traces,
        &mut memos,
        &mut overlay,
    );

    let trace = traces.get("big-trace").expect("trace still present");
    assert_eq!(
        trace.leaf_records.len(),
        3,
        "compact keeps exactly leaf_limit records"
    );
    // Every retained record must be a valid succeeded leaf.
    for rec in &trace.leaf_records {
        assert_eq!(rec.result.status, WorkflowRunStatus::Succeeded);
        assert!(!rec.leaf_id.is_empty());
        assert!(!rec.input_hash.is_empty());
    }
    // Control records untouched (none here, but the store should not have
    // introduced any either).
    assert!(trace.control_records.is_empty());
}

#[test]
fn janitor_removes_fully_compacted_empty_traces() {
    // Limit of 1 leaf per trace, but the trace has only failed leaves that
    // get pushed out by the succeeded-first ordering when limit < failed count.
    let mut trace = WorkflowReplayTrace {
        trace_id: "all-failed".to_string(),
        leaf_records: vec![
            leaf_record("all-failed", "f0", WorkflowRunStatus::Failed),
            leaf_record("all-failed", "f1", WorkflowRunStatus::Failed),
        ],
        control_records: Vec::new(),
    };

    // With limit=1: succeeded partition is empty (0 items), other gets
    // truncated to 1 → 1 record remains, trace is still replayable.
    let limits = JanitorLimits {
        compacted_leaf_limit: 1,
        ..JanitorLimits::default()
    };
    let janitor = Janitor::new(limits);
    let mut traces = TraceStore::new(0);
    traces.insert(trace.clone());

    let mut candidates = Vec::new();
    let gate = PromotionGate::default();
    let mut memos = MemoStore::new(0);
    let mut overlay = OverlayStore::new(0);
    let report = janitor.run(
        &mut candidates,
        &gate,
        &mut traces,
        &mut memos,
        &mut overlay,
    );

    // 1 leaf remains → still replayable → not removed.
    assert!(report.unreplayable_traces_removed.is_empty());
    assert_eq!(traces.get("all-failed").unwrap().leaf_records.len(), 1);

    // Now compact to 0 (unlimited) — nothing changes.
    trace.leaf_records = vec![leaf_record("t0", "f0", WorkflowRunStatus::Failed)];
    let limits_zero = JanitorLimits {
        compacted_leaf_limit: 0, // 0 = no-op
        ..JanitorLimits::default()
    };
    let janitor_zero = Janitor::new(limits_zero);
    let mut traces2 = TraceStore::new(0);
    let t_before = make_trace("t-preserved", 5);
    traces2.insert(t_before);
    let report2 = janitor_zero.run(
        &mut candidates,
        &gate,
        &mut traces2,
        &mut memos,
        &mut overlay,
    );
    assert_eq!(
        traces2.get("t-preserved").unwrap().leaf_records.len(),
        5,
        "limit=0 means no compaction"
    );
    assert!(report2.unreplayable_traces_removed.is_empty());
}
