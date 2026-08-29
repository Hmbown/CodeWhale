//! Dedicated persistence actor for session save / checkpoint I/O.
//!
//! ## Motivation
//!
//! Before this module, `persist_checkpoint` and `persist_session_snapshot` ran
//! synchronously on the tokio worker thread that drives the TUI event loop.
//! Each call serialised all API messages to JSON, wrote a temp file, and
//! renamed it atomically — blocking keyboard input for the duration.
//! `save_session` additionally called `cleanup_old_sessions`, which listed all
//! session files, parsed metadata from every one, sorted, and deleted the
//! oldest — scaling O(session-bytes + file-count) with every turn.
//!
//! ## Design
//!
//! - **One dedicated tokio task** owns disk I/O. The UI only sends requests;
//!   keystrokes never wait for writes.
//! - **Latest-wins coalescing per session**: when multiple `SaveCheckpoint`,
//!   `SessionSnapshot`, or offline-queue requests pile up before the actor's
//!   next write cycle, only the most recent one per session is written.
//!   Checkpoints and clears are keyed by session id, so concurrent sessions
//!   never coalesce into (or clear) each other's slot.
//! - **Durability reporting**: `FlushAndReport` returns accumulated results;
//!   cycles without a listener log failures instead of discarding them.
//! - **Unbounded channel** for `try_send` to always succeed. Queued snapshots
//!   retain only the canonical journal; legacy `messages` are derived at the
//!   disk boundary instead of doubling every paused request.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use tokio::sync::{mpsc, oneshot};

use crate::session_manager::{OfflineQueueState, SavedSession, SessionManager};
use crate::utils::spawn_supervised;

// ---------------------------------------------------------------------------
// Request type
// ---------------------------------------------------------------------------

/// Persistence work item sent to the actor.
#[derive(Debug)]
pub enum PersistRequest {
    /// Write a crash-recovery checkpoint (in-flight turn state) to the
    /// session's own file (`checkpoints/<session_id>.json`).
    SaveCheckpoint { session: SavedSession },
    /// Write a full session snapshot (completed turn, durable save).
    SessionSnapshot(SavedSession),
    /// Compound completion commit: write the completed session snapshot,
    /// and only if that write succeeds, clear that same session's
    /// crash-recovery checkpoint. A failed snapshot write RETAINS the
    /// checkpoint as the only surviving recovery record, and the clear is
    /// scoped to the committed session's id — it can never remove another
    /// session's checkpoint. Turn completion must send this instead of a
    /// `SessionSnapshot` + `ClearCheckpoint` pair, which the actor could
    /// otherwise apply with the clear first, erasing the recovery record
    /// before the snapshot safely landed.
    CompletedCommit { session: SavedSession },
    /// Write queued/draft offline input for crash recovery.
    OfflineQueue {
        state: OfflineQueueState,
        session_id: Option<String>,
    },
    /// Remove the queued/draft offline input file.
    ClearOfflineQueue,
    /// Remove one session's crash-recovery checkpoint file. Scoped: cannot
    /// remove another session's checkpoint.
    ClearCheckpoint { session_id: String },
    /// Flush all pending work now and report durability results through
    /// `reply`. The report aggregates every write/removal result since the
    /// previous report (including background write cycles) — errors are
    /// collected and surfaced, never discarded.
    FlushAndReport { reply: oneshot::Sender<FlushReport> },
    /// Graceful shutdown — flush pending writes, then exit the actor loop.
    Shutdown,
}

/// Aggregated durability results: how many writes/removals completed and
/// which failed (labelled by what was being persisted, with the I/O error
/// kind).
#[derive(Debug, Default)]
pub struct FlushReport {
    pub completed: usize,
    pub failures: Vec<(String, std::io::ErrorKind)>,
}

impl FlushReport {
    /// Upper bound on retained failure entries when accumulating across
    /// write cycles. Every failure is logged at the cycle it happened, so
    /// dropping older-than-bound entries from the reply loses no evidence.
    const MAX_ACCUMULATED_FAILURES: usize = 256;

    fn merge(&mut self, other: FlushReport) {
        self.completed += other.completed;
        self.failures.extend(other.failures);
        if self.failures.len() > Self::MAX_ACCUMULATED_FAILURES {
            let excess = self.failures.len() - Self::MAX_ACCUMULATED_FAILURES;
            self.failures.drain(..excess);
        }
    }
}

#[derive(Debug)]
enum PendingOfflineQueue {
    Save {
        state: Box<OfflineQueueState>,
        session_id: Option<String>,
    },
    Clear,
}

// ---------------------------------------------------------------------------
// Handle (held by the TUI)
// ---------------------------------------------------------------------------

type PersistRequestSender = mpsc::UnboundedSender<PersistRequest>;
type PersistRequestReceiver = mpsc::UnboundedReceiver<PersistRequest>;

/// Single construction seam for the production persistence request channel.
///
/// The ignored backlog measurement uses this same factory with the receiver
/// deliberately paused, so a later bounded-channel change cannot leave the
/// baseline measuring an obsolete representation.
fn persistence_request_channel() -> (PersistRequestSender, PersistRequestReceiver) {
    mpsc::unbounded_channel()
}

/// Lightweight handle that the UI holds to queue persistence work.
#[derive(Debug, Clone)]
pub struct PersistActorHandle {
    tx: PersistRequestSender,
}

impl PersistActorHandle {
    /// Queue a persistence request without blocking. If the actor's channel is
    /// closed (shutdown has already happened), return `false`.
    pub fn try_send(&self, mut request: PersistRequest) -> bool {
        match &mut request {
            PersistRequest::SaveCheckpoint { session }
            | PersistRequest::SessionSnapshot(session)
            | PersistRequest::CompletedCommit { session } => {
                session.compact_for_persistence_queue();
            }
            _ => {}
        }
        self.tx.send(request).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Global singleton (avoid threading through App)
// ---------------------------------------------------------------------------

static ACTOR_TX: OnceLock<PersistActorHandle> = OnceLock::new();

/// Initialise the global persistence actor handle. Must be called once at
/// startup, before the event loop starts.
pub fn init_actor(handle: PersistActorHandle) {
    let _ = ACTOR_TX.set(handle);
}

/// Queue a persistence request through the global handle. When the request
/// cannot be queued — actor not initialised yet (tests, early startup) or
/// already shut down — the drop is logged instead of discarded silently, so
/// lost session/work-graph state is diagnosable after the fact.
pub fn persist(request: PersistRequest) {
    let label = request_label(&request);
    if try_persist(request) {
        return;
    }
    if ACTOR_TX.get().is_some() {
        tracing::warn!(
            request = label,
            "persistence request dropped: actor channel is closed (shutdown already happened)"
        );
    } else {
        tracing::debug!(
            request = label,
            "persistence request dropped: actor not initialised yet"
        );
    }
}

fn request_label(request: &PersistRequest) -> &'static str {
    match request {
        PersistRequest::SaveCheckpoint { .. } => "SaveCheckpoint",
        PersistRequest::SessionSnapshot(_) => "SessionSnapshot",
        PersistRequest::CompletedCommit { .. } => "CompletedCommit",
        PersistRequest::OfflineQueue { .. } => "OfflineQueue",
        PersistRequest::ClearOfflineQueue => "ClearOfflineQueue",
        PersistRequest::ClearCheckpoint { .. } => "ClearCheckpoint",
        PersistRequest::FlushAndReport { .. } => "FlushAndReport",
        PersistRequest::Shutdown => "Shutdown",
    }
}

/// Queue persistence and report whether the actor accepted ownership. Work
/// Graph projections use this acknowledgement as their publish boundary.
pub fn try_persist(request: PersistRequest) -> bool {
    ACTOR_TX
        .get()
        .is_some_and(|handle| handle.try_send(request))
}

// ---------------------------------------------------------------------------
// Actor spawn
// ---------------------------------------------------------------------------

/// Spawn the persistence actor task and return a handle for the caller to
/// store and initialise.
///
/// The returned handle should be passed to [`init_actor`] so that the
/// `persist()` free function can reach it from anywhere in the TUI.
pub fn spawn_persistence_actor(
    manager: SessionManager,
) -> (PersistActorHandle, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = persistence_request_channel();
    let handle = PersistActorHandle { tx };

    let task = spawn_supervised(
        "persistence-actor",
        std::panic::Location::caller(),
        async move {
            let mut pending = PendingState::default();
            // Durability results from write cycles that no caller has asked
            // about yet; drained into the next `FlushAndReport` reply.
            let mut unreported = FlushReport::default();

            // Flush pending work, log new failures, and fold the cycle's
            // results into the unreported accumulator.
            fn flush_cycle(
                manager: &SessionManager,
                pending: &mut PendingState,
                unreported: &mut FlushReport,
            ) {
                let cycle = flush_inner(manager, pending);
                log_flush_failures(&cycle);
                unreported.merge(cycle);
            }

            loop {
                // Drain everything waiting, keeping only the latest of each kind.
                while let Ok(req) = rx.try_recv() {
                    match pending.absorb(req) {
                        Control::Continue => {}
                        Control::Flush(reply) => {
                            flush_cycle(&manager, &mut pending, &mut unreported);
                            let _ = reply.send(std::mem::take(&mut unreported));
                        }
                        Control::Shutdown => {
                            flush_cycle(&manager, &mut pending, &mut unreported);
                            return;
                        }
                    }
                }

                // Write coalesced work.
                flush_cycle(&manager, &mut pending, &mut unreported);

                // Block until the next request arrives.
                match rx.recv().await {
                    Some(req) => match pending.absorb(req) {
                        Control::Continue => {}
                        Control::Flush(reply) => {
                            flush_cycle(&manager, &mut pending, &mut unreported);
                            let _ = reply.send(std::mem::take(&mut unreported));
                        }
                        Control::Shutdown => {
                            flush_cycle(&manager, &mut pending, &mut unreported);
                            return;
                        }
                    },
                    None => {
                        // Channel closed — final flush and exit.
                        flush_cycle(&manager, &mut pending, &mut unreported);
                        return;
                    }
                }
            }
        },
    );

    (handle, task)
}

/// Coalesced work waiting for the next write cycle.
#[derive(Debug, Default)]
struct PendingState {
    /// Latest-wins per session id. Crash checkpoints are keyed per session
    /// (mirroring `sessions` below) so concurrent sessions can interleave
    /// saves and clears without clobbering each other.
    checkpoints: BTreeMap<String, SavedSession>,
    /// Session ids whose checkpoint file should be removed.
    checkpoint_clears: BTreeSet<String>,
    /// Latest-wins per session id. Coalescing into one global slot can
    /// drop session A when an immediate `/new` queues session B before
    /// the actor drains.
    sessions: BTreeMap<String, SavedSession>,
    /// Compound completion commits, latest-wins per session id: the
    /// completed snapshot body to write, followed by that session's own
    /// checkpoint clear — the clear only if the write succeeded. Kept
    /// separate from `sessions` so a plain snapshot can never be drained
    /// as a completion (or vice versa) and the clear intent stays bound to
    /// exactly the session that completed.
    completed_commits: BTreeMap<String, SavedSession>,
    offline_queue: Option<PendingOfflineQueue>,
}

/// What the actor loop should do after absorbing a request.
enum Control {
    Continue,
    Flush(oneshot::Sender<FlushReport>),
    Shutdown,
}

impl PendingState {
    fn absorb(&mut self, req: PersistRequest) -> Control {
        match req {
            PersistRequest::SaveCheckpoint { session } => {
                // Last-writer-wins per session: a fresh checkpoint supersedes
                // a pending clear for the same session so the two never both
                // apply in one drain (which previously cleared then re-wrote
                // the stale checkpoint, undoing the clear).
                let id = session.metadata.id.clone();
                self.checkpoint_clears.remove(&id);
                // A new in-flight checkpoint means newer turn work started
                // after the completion it would have cleared: the compound's
                // clear-on-success intent is stale now (it would erase the
                // fresher recovery record), so drop the pending compound and
                // keep only the newer checkpoint body.
                if self.completed_commits.remove(&id).is_some() {
                    tracing::debug!(
                        session_id = %id,
                        "pending completed commit superseded by a newer in-flight checkpoint"
                    );
                }
                self.checkpoints.insert(id, session);
            }
            PersistRequest::SessionSnapshot(session) => {
                // A newer full snapshot of a session with a pending
                // completed commit refreshes the commit's body (and keeps
                // its clear-on-success intent): the in-flight checkpoint it
                // guards is already captured by the newer completed state.
                let id = session.metadata.id.clone();
                if let Some(pending) = self.completed_commits.get_mut(&id) {
                    *pending = session;
                } else {
                    self.sessions.insert(id, session);
                }
            }
            PersistRequest::CompletedCommit { session } => {
                let id = session.metadata.id.clone();
                // The compound owns this session's snapshot and clear: a
                // pending plain snapshot is superseded, a pending standalone
                // clear would erase the recovery record even when the save
                // fails, and a pending checkpoint write would re-create the
                // record after the compound cleared it.
                self.sessions.remove(&id);
                self.checkpoint_clears.remove(&id);
                self.checkpoints.remove(&id);
                self.completed_commits.insert(id, session);
            }
            PersistRequest::OfflineQueue { state, session_id } => {
                self.offline_queue = Some(PendingOfflineQueue::Save {
                    state: Box::new(state),
                    session_id,
                });
            }
            PersistRequest::ClearOfflineQueue => {
                self.offline_queue = Some(PendingOfflineQueue::Clear);
            }
            PersistRequest::ClearCheckpoint { session_id } => {
                // A clear supersedes a pending checkpoint write for the same
                // session only — other sessions' pending work is untouched.
                // An explicit clear is also a user-owned boundary (e.g.
                // `/new`): it supersedes a pending compound for the same
                // session so the discarded session is not re-written.
                self.checkpoints.remove(&session_id);
                self.completed_commits.remove(&session_id);
                self.checkpoint_clears.insert(session_id);
            }
            PersistRequest::FlushAndReport { reply } => return Control::Flush(reply),
            PersistRequest::Shutdown => return Control::Shutdown,
        }
        Control::Continue
    }
}

/// Write all pending work to disk, draining `pending`. Every write and
/// removal result is collected into the returned [`FlushReport`] — failures
/// are reported, never silently discarded.
///
/// Ordering is durability-critical: every session snapshot write (plain or
/// completion commit) happens BEFORE any checkpoint clear. A completion
/// commit clears its session's checkpoint only after that session's own
/// write succeeded, so a failed save always leaves the crash-recovery
/// checkpoint in place.
fn flush_inner(manager: &SessionManager, pending: &mut PendingState) -> FlushReport {
    let mut report = FlushReport::default();
    let mut record = |what: String, result: std::io::Result<()>| match result {
        Ok(()) => report.completed += 1,
        Err(err) => report.failures.push((what, err.kind())),
    };

    for (session_id, session) in std::mem::take(&mut pending.sessions) {
        record(
            format!("session:{session_id}"),
            manager.save_session(&session).map(|_| ()),
        );
    }
    for (session_id, session) in std::mem::take(&mut pending.completed_commits) {
        let commit_result = manager.save_session(&session);
        let save_succeeded = commit_result.is_ok();
        record(
            format!("completed-commit:{session_id}"),
            commit_result.map(|_| ()),
        );
        if save_succeeded {
            // Only the committed session's own checkpoint is cleared, and
            // only because its snapshot safely landed. A failure above
            // retains the checkpoint as the sole recovery record.
            record(
                format!("clear-checkpoint:{session_id}"),
                manager.clear_session_checkpoint(&session_id),
            );
        }
    }
    for session_id in std::mem::take(&mut pending.checkpoint_clears) {
        record(
            format!("clear-checkpoint:{session_id}"),
            manager.clear_session_checkpoint(&session_id),
        );
    }
    for (session_id, session) in std::mem::take(&mut pending.checkpoints) {
        record(
            format!("checkpoint:{session_id}"),
            manager.save_checkpoint(&session).map(|_| ()),
        );
    }
    if let Some(request) = pending.offline_queue.take() {
        match request {
            PendingOfflineQueue::Save { state, session_id } => record(
                "offline-queue".to_string(),
                manager
                    .save_offline_queue_state(&state, session_id.as_deref())
                    .map(|_| ()),
            ),
            PendingOfflineQueue::Clear => record(
                "clear-offline-queue".to_string(),
                manager.clear_offline_queue_state(),
            ),
        }
    }
    report
}

/// Surface flush failures in the log for write cycles that have no caller
/// waiting on a [`FlushReport`].
fn log_flush_failures(report: &FlushReport) {
    for (what, kind) in &report.failures {
        tracing::warn!(
            target: "persistence",
            what = %what,
            error_kind = ?kind,
            "persistence write failed",
        );
    }
}

#[cfg(test)]
#[path = "persistence_actor/tests.rs"]
mod backlog_measurement_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::session_manager::{OfflineQueueState, QueuedSessionMessage};

    async fn wait_until(mut predicate: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if predicate() {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for persistence actor"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn actor_persists_and_clears_offline_queue_requests() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("manager");
        let queue_path = sessions_dir.join("checkpoints").join("offline_queue.json");
        let (handle, task) = spawn_persistence_actor(manager);

        let state = OfflineQueueState {
            messages: vec![QueuedSessionMessage {
                display: "queued from enter".to_string(),
                skill_instruction: None,
                skill_provenance: None,
            }],
            ..OfflineQueueState::default()
        };

        handle.try_send(PersistRequest::OfflineQueue {
            state,
            session_id: Some("session-A".to_string()),
        });
        wait_until(|| {
            std::fs::read_to_string(&queue_path)
                .is_ok_and(|body| body.contains("queued from enter"))
        })
        .await;

        handle.try_send(PersistRequest::ClearOfflineQueue);
        wait_until(|| !queue_path.exists()).await;
        handle.try_send(PersistRequest::Shutdown);
        task.await.expect("persistence actor join");
    }

    #[tokio::test]
    async fn shutdown_wait_flushes_queued_session_before_returning() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("manager");
        let verification_manager = SessionManager::new(sessions_dir).expect("verification manager");
        let session = crate::session_manager::create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
            Some("agent"),
        );
        let session_id = session.metadata.id.clone();
        let (handle, task) = spawn_persistence_actor(manager);

        handle.try_send(PersistRequest::SessionSnapshot(session));
        handle.try_send(PersistRequest::Shutdown);
        task.await.expect("persistence actor join");

        let loaded = verification_manager
            .load_session(&session_id)
            .expect("shutdown must flush queued session");
        assert_eq!(loaded.metadata.id, session_id);
    }

    #[tokio::test]
    async fn shutdown_flushes_latest_snapshot_for_each_session_id() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("manager");
        let verification_manager = SessionManager::new(sessions_dir).expect("verification manager");
        let mut first = crate::session_manager::create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
            Some("agent"),
        );
        first.metadata.title = "Session A".to_string();
        let mut second = crate::session_manager::create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
            Some("agent"),
        );
        second.metadata.title = "Session B".to_string();
        let first_id = first.metadata.id.clone();
        let second_id = second.metadata.id.clone();
        let (handle, task) = spawn_persistence_actor(manager);

        handle.try_send(PersistRequest::SessionSnapshot(first));
        handle.try_send(PersistRequest::SessionSnapshot(second));
        handle.try_send(PersistRequest::Shutdown);
        task.await.expect("persistence actor join");

        assert_eq!(
            verification_manager
                .load_session(&first_id)
                .expect("session A flushed")
                .metadata
                .title,
            "Session A"
        );
        assert_eq!(
            verification_manager
                .load_session(&second_id)
                .expect("session B flushed")
                .metadata
                .title,
            "Session B"
        );
    }

    #[tokio::test]
    async fn interleaved_checkpoint_saves_and_clears_stay_per_session() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("manager");
        let verification_manager = SessionManager::new(sessions_dir).expect("verification manager");
        let first = crate::session_manager::create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
            Some("agent"),
        );
        let second = crate::session_manager::create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
            Some("agent"),
        );
        let first_id = first.metadata.id.clone();
        let second_id = second.metadata.id.clone();
        let (handle, task) = spawn_persistence_actor(manager);

        // Interleave: save A, save B, clear A — all coalesced into one drain.
        handle.try_send(PersistRequest::SaveCheckpoint { session: first });
        handle.try_send(PersistRequest::SaveCheckpoint { session: second });
        handle.try_send(PersistRequest::ClearCheckpoint {
            session_id: first_id.clone(),
        });
        handle.try_send(PersistRequest::Shutdown);
        task.await.expect("persistence actor join");

        assert!(
            verification_manager
                .load_session_checkpoint(&first_id)
                .expect("load first checkpoint")
                .is_none(),
            "cleared session must have no checkpoint file"
        );
        let survivor = verification_manager
            .load_session_checkpoint(&second_id)
            .expect("load second checkpoint")
            .expect("second session's checkpoint must survive an unrelated clear");
        assert_eq!(survivor.metadata.id, second_id);
    }

    #[tokio::test]
    async fn flush_and_report_returns_completed_counts() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir).expect("manager");
        let session = crate::session_manager::create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
            Some("agent"),
        );
        let (handle, task) = spawn_persistence_actor(manager);

        handle.try_send(PersistRequest::SaveCheckpoint { session });
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        handle.try_send(PersistRequest::FlushAndReport { reply: reply_tx });
        let report = reply_rx.await.expect("flush report reply");
        // Whether the checkpoint was written by an earlier background cycle
        // or by this flush, the accumulated report must count it and show no
        // failures — and the actor keeps running afterwards.
        assert!(report.completed >= 1, "checkpoint write must be counted");
        assert!(report.failures.is_empty(), "no failures expected");
        handle.try_send(PersistRequest::Shutdown);
        task.await.expect("persistence actor join");
    }

    #[tokio::test]
    async fn flush_and_report_propagates_write_failures() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("manager");
        // Occupy the checkpoints directory path with a regular file so every
        // checkpoint write deterministically fails on all platforms.
        std::fs::write(sessions_dir.join("checkpoints"), b"not a directory")
            .expect("block checkpoints dir");
        let session = crate::session_manager::create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
            Some("agent"),
        );
        let session_id = session.metadata.id.clone();
        let (handle, task) = spawn_persistence_actor(manager);

        handle.try_send(PersistRequest::SaveCheckpoint { session });
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        handle.try_send(PersistRequest::FlushAndReport { reply: reply_tx });
        let report = reply_rx.await.expect("flush report reply");

        assert!(
            report
                .failures
                .iter()
                .any(|(what, _)| what == &format!("checkpoint:{session_id}")),
            "failed checkpoint write must be reported, got: {:?}",
            report.failures
        );
        handle.try_send(PersistRequest::Shutdown);
        task.await.expect("persistence actor join");
    }

    /// Pre-write a crash-recovery checkpoint file for `session_id` directly,
    /// so completion-commit tests can assert on its survival without needing
    /// a prior in-flight turn.
    fn seed_checkpoint_file(
        sessions_dir: &std::path::Path,
        session_id: &str,
    ) -> std::path::PathBuf {
        let path = sessions_dir
            .join("checkpoints")
            .join(format!("{session_id}.json"));
        std::fs::create_dir_all(path.parent().expect("checkpoints parent")).expect("mkdir");
        std::fs::write(&path, "{}").expect("seed checkpoint file");
        path
    }

    /// Deterministically fail session-file saves only: a directory at the
    /// session's own `<id>.json` path makes `save_session` fail while the
    /// checkpoints directory stays fully usable.
    fn block_session_file(sessions_dir: &std::path::Path, session_id: &str) {
        std::fs::create_dir_all(sessions_dir.join(format!("{session_id}.json")))
            .expect("block session file path");
    }

    #[tokio::test]
    async fn completed_commit_preserves_checkpoint_when_session_save_fails() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("manager");
        let session = crate::session_manager::create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
            Some("agent"),
        );
        let session_id = session.metadata.id.clone();
        let checkpoint_path = seed_checkpoint_file(&sessions_dir, &session_id);
        block_session_file(&sessions_dir, &session_id);
        let (handle, task) = spawn_persistence_actor(manager);

        handle.try_send(PersistRequest::CompletedCommit { session });
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        handle.try_send(PersistRequest::FlushAndReport { reply: reply_tx });
        let report = reply_rx.await.expect("flush report reply");

        assert!(
            report
                .failures
                .iter()
                .any(|(what, _)| what == &format!("completed-commit:{session_id}")),
            "failed session save must be reported, got: {:?}",
            report.failures
        );
        assert!(
            !report
                .failures
                .iter()
                .any(|(what, _)| what == &format!("clear-checkpoint:{session_id}")),
            "the checkpoint clear must not be attempted after a failed save"
        );
        assert!(
            checkpoint_path.exists(),
            "a failed session save must retain the crash-recovery checkpoint"
        );
        handle.try_send(PersistRequest::Shutdown);
        task.await.expect("persistence actor join");
    }

    #[tokio::test]
    async fn completed_commit_clears_only_after_session_save() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("manager");
        let verification_manager = SessionManager::new(sessions_dir.clone()).expect("verify");
        let session = crate::session_manager::create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
            Some("agent"),
        );
        let session_id = session.metadata.id.clone();
        let checkpoint_path = seed_checkpoint_file(&sessions_dir, &session_id);
        let (handle, task) = spawn_persistence_actor(manager);

        handle.try_send(PersistRequest::CompletedCommit { session });
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        handle.try_send(PersistRequest::FlushAndReport { reply: reply_tx });
        let report = reply_rx.await.expect("flush report reply");

        assert!(
            report.failures.is_empty(),
            "commit save and its clear must both succeed, got: {:?}",
            report.failures
        );
        let saved = verification_manager
            .load_session(&session_id)
            .expect("completed session must be saved before its checkpoint is cleared");
        assert_eq!(saved.metadata.id, session_id);
        assert!(
            !checkpoint_path.exists(),
            "the checkpoint is cleared only after the session save succeeded"
        );
        handle.try_send(PersistRequest::Shutdown);
        task.await.expect("persistence actor join");
    }

    #[tokio::test]
    async fn completed_commit_never_clears_another_sessions_checkpoint() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("manager");
        let verification_manager = SessionManager::new(sessions_dir.clone()).expect("verify");
        let committed = crate::session_manager::create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
            Some("agent"),
        );
        let inflight = crate::session_manager::create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
            Some("agent"),
        );
        let committed_id = committed.metadata.id.clone();
        let inflight_id = inflight.metadata.id.clone();
        let committed_checkpoint = seed_checkpoint_file(&sessions_dir, &committed_id);
        // The concurrent session's checkpoint must carry a loadable body, so
        // seed it through the real manager instead of a placeholder file.
        let inflight_checkpoint = verification_manager
            .save_checkpoint(&inflight)
            .expect("seed the concurrent session's checkpoint");
        let (handle, task) = spawn_persistence_actor(manager);

        handle.try_send(PersistRequest::CompletedCommit { session: committed });
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        handle.try_send(PersistRequest::FlushAndReport { reply: reply_tx });
        let report = reply_rx.await.expect("flush report reply");
        assert!(report.failures.is_empty(), "{:?}", report.failures);

        assert!(
            !committed_checkpoint.exists(),
            "the committed session's own checkpoint must be cleared"
        );
        let survivor = verification_manager
            .load_session_checkpoint(&inflight_id)
            .expect("load the concurrent session's checkpoint")
            .expect("another session's checkpoint must never be cleared");
        assert_eq!(survivor.metadata.id, inflight_id);
        assert!(inflight_checkpoint.exists());
        handle.try_send(PersistRequest::Shutdown);
        task.await.expect("persistence actor join");
    }

    #[tokio::test]
    async fn shutdown_preserves_inflight_checkpoint() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("manager");
        let verification_manager = SessionManager::new(sessions_dir.clone()).expect("verify");
        let session = crate::session_manager::create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
            Some("agent"),
        );
        let session_id = session.metadata.id.clone();
        let checkpoint_path = seed_checkpoint_file(&sessions_dir, &session_id);
        let (handle, task) = spawn_persistence_actor(manager);

        handle.try_send(PersistRequest::SaveCheckpoint { session });
        handle.try_send(PersistRequest::Shutdown);
        task.await.expect("persistence actor join");

        assert!(
            checkpoint_path.exists(),
            "shutdown must never unconditionally clear an in-flight checkpoint"
        );
        let recovered = verification_manager
            .load_session_checkpoint(&session_id)
            .expect("load checkpoint after shutdown")
            .expect("in-flight work must survive shutdown for recovery review");
        assert_eq!(recovered.metadata.id, session_id);
    }

    #[tokio::test]
    async fn newer_inflight_checkpoint_supersedes_pending_completed_commit() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("manager");
        let completed = crate::session_manager::create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
            Some("agent"),
        );
        let mut inflight = crate::session_manager::create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
            Some("agent"),
        );
        // The new turn reuses the same session id: a fresh in-flight
        // checkpoint arrives while the previous completion is still queued.
        inflight.metadata.id = completed.metadata.id.clone();
        let session_id = completed.metadata.id.clone();
        let (handle, task) = spawn_persistence_actor(manager);

        handle.try_send(PersistRequest::CompletedCommit { session: completed });
        handle.try_send(PersistRequest::SaveCheckpoint { session: inflight });
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        handle.try_send(PersistRequest::FlushAndReport { reply: reply_tx });
        let report = reply_rx.await.expect("flush report reply");

        assert!(
            !report
                .failures
                .iter()
                .any(|(what, _)| what.starts_with("clear-checkpoint:")),
            "the newer in-flight checkpoint must not be cleared, got: {:?}",
            report.failures
        );
        let checkpoint = std::fs::read_to_string(
            sessions_dir
                .join("checkpoints")
                .join(format!("{session_id}.json")),
        )
        .expect("the newer checkpoint must survive the drain");
        assert!(
            !checkpoint.is_empty(),
            "the newer checkpoint body must be on disk"
        );
        assert!(
            report.completed >= 1,
            "the newer checkpoint write must be counted, got: {report:?}"
        );
        handle.try_send(PersistRequest::Shutdown);
        task.await.expect("persistence actor join");
    }
}
