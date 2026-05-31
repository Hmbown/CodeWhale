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
//! - **One dedicated tokio task** spawned at TUI startup. All disk I/O moves
//!   to this task. The UI merely `try_send`s a request (non-blocking,
//!   bounded-channel drop) and returns immediately — keystrokes are never
//!   gated on write completion.
//! - **Latest-wins coalescing**: when multiple `Checkpoint`,
//!   `SessionSnapshot`, or offline-queue requests pile up before the actor's
//!   next write cycle, only the most recent one is written. `ClearCheckpoint`
//!   requests accumulate normally (they're cheap and commutative).
//! - **Unbounded channel** for `try_send` to always succeed; the actor
//!   naturally backpressures via the spawn pool. A few outstanding
//!   `SavedSession` values in the channel (< 1 MB) is negligible pressure.

use std::sync::OnceLock;

use tokio::sync::mpsc;

use crate::session_manager::{OfflineQueueState, SavedSession, SessionManager};
use crate::utils::spawn_supervised;

// ---------------------------------------------------------------------------
// Request type
// ---------------------------------------------------------------------------

/// Persistence work item sent to the actor.
#[derive(Debug)]
pub enum PersistRequest {
    /// Write a crash-recovery checkpoint (in-flight turn state).
    Checkpoint(SavedSession),
    /// Write a full session snapshot (completed turn, durable save).
    SessionSnapshot(SavedSession),
    /// Write queued/draft offline input for crash recovery.
    OfflineQueue {
        state: OfflineQueueState,
        session_id: Option<String>,
    },
    /// Remove the queued/draft offline input file.
    ClearOfflineQueue,
    /// Remove the crash-recovery checkpoint file.
    ClearCheckpoint,
    /// Graceful shutdown — flush pending writes, then exit the actor loop.
    Shutdown,
}

#[derive(Debug)]
enum PendingOfflineQueue {
    Save {
        state: OfflineQueueState,
        session_id: Option<String>,
    },
    Clear,
}

// ---------------------------------------------------------------------------
// Handle (held by the TUI)
// ---------------------------------------------------------------------------

/// Lightweight handle that the UI holds to queue persistence work.
#[derive(Debug, Clone)]
pub struct PersistActorHandle {
    tx: mpsc::UnboundedSender<PersistRequest>,
}

impl PersistActorHandle {
    /// Queue a persistence request without blocking. If the actor's channel is
    /// closed (shutdown has already happened) the request is silently dropped.
    pub fn try_send(&self, request: PersistRequest) {
        let _ = self.tx.send(request);
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

/// Queue a persistence request through the global handle. No-op (silently
/// ignored) when the actor hasn't been initialised yet — this can happen in
/// tests or early startup before the actor is ready.
pub fn persist(request: PersistRequest) {
    if let Some(handle) = ACTOR_TX.get() {
        handle.try_send(request);
    }
}

// ---------------------------------------------------------------------------
// Actor spawn
// ---------------------------------------------------------------------------

/// Spawn the persistence actor task and return a handle for the caller to
/// store and initialise.
///
/// The returned handle should be passed to [`init_actor`] so that the
/// `persist()` free function can reach it from anywhere in the TUI.
pub fn spawn_persistence_actor(manager: SessionManager) -> PersistActorHandle {
    let (tx, mut rx) = mpsc::unbounded_channel::<PersistRequest>();
    let handle = PersistActorHandle { tx };

    spawn_supervised(
        "persistence-actor",
        std::panic::Location::caller(),
        async move {
            let mut latest_checkpoint: Option<SavedSession> = None;
            let mut latest_session: Option<SavedSession> = None;
            let mut latest_offline_queue: Option<PendingOfflineQueue> = None;
            let mut should_clear: bool = false;

            loop {
                // Drain everything waiting, keeping only the latest of each kind.
                while let Ok(req) = rx.try_recv() {
                    match req {
                        PersistRequest::Checkpoint(session) => {
                            latest_checkpoint = Some(session);
                        }
                        PersistRequest::SessionSnapshot(session) => {
                            latest_session = Some(session);
                        }
                        PersistRequest::OfflineQueue { state, session_id } => {
                            latest_offline_queue =
                                Some(PendingOfflineQueue::Save { state, session_id });
                        }
                        PersistRequest::ClearOfflineQueue => {
                            latest_offline_queue = Some(PendingOfflineQueue::Clear);
                        }
                        PersistRequest::ClearCheckpoint => {
                            should_clear = true;
                        }
                        PersistRequest::Shutdown => {
                            flush_inner(
                                &manager,
                                latest_checkpoint.as_ref(),
                                latest_session.as_ref(),
                                latest_offline_queue.as_ref(),
                                should_clear,
                            );
                            return;
                        }
                    }
                }

                // Write coalesced work.
                if should_clear {
                    let _ = manager.clear_checkpoint();
                    should_clear = false;
                }
                if let Some(ref session) = latest_checkpoint.take() {
                    let _ = manager.save_checkpoint(session);
                }
                if let Some(ref session) = latest_session.take() {
                    let _ = manager.save_session(session);
                }
                if let Some(ref request) = latest_offline_queue.take() {
                    apply_offline_queue_request(&manager, request);
                }

                // Block until the next request arrives.
                match rx.recv().await {
                    Some(PersistRequest::Checkpoint(session)) => {
                        latest_checkpoint = Some(session);
                    }
                    Some(PersistRequest::SessionSnapshot(session)) => {
                        latest_session = Some(session);
                    }
                    Some(PersistRequest::OfflineQueue { state, session_id }) => {
                        latest_offline_queue =
                            Some(PendingOfflineQueue::Save { state, session_id });
                    }
                    Some(PersistRequest::ClearOfflineQueue) => {
                        latest_offline_queue = Some(PendingOfflineQueue::Clear);
                    }
                    Some(PersistRequest::ClearCheckpoint) => {
                        should_clear = true;
                    }
                    Some(PersistRequest::Shutdown) => {
                        flush_inner(
                            &manager,
                            latest_checkpoint.as_ref(),
                            latest_session.as_ref(),
                            latest_offline_queue.as_ref(),
                            should_clear,
                        );
                        return;
                    }
                    None => {
                        // Channel closed — final flush and exit.
                        flush_inner(
                            &manager,
                            latest_checkpoint.as_ref(),
                            latest_session.as_ref(),
                            latest_offline_queue.as_ref(),
                            should_clear,
                        );
                        return;
                    }
                }
            }
        },
    );

    handle
}

/// Write any pending work to disk (used on shutdown).
fn flush_inner(
    manager: &SessionManager,
    checkpoint: Option<&SavedSession>,
    session: Option<&SavedSession>,
    offline_queue: Option<&PendingOfflineQueue>,
    should_clear: bool,
) {
    if should_clear {
        let _ = manager.clear_checkpoint();
    }
    if let Some(s) = checkpoint {
        let _ = manager.save_checkpoint(s);
    }
    if let Some(s) = session {
        let _ = manager.save_session(s);
    }
    if let Some(request) = offline_queue {
        apply_offline_queue_request(manager, request);
    }
}

fn apply_offline_queue_request(manager: &SessionManager, request: &PendingOfflineQueue) {
    match request {
        PendingOfflineQueue::Save { state, session_id } => {
            let _ = manager.save_offline_queue_state(state, session_id.as_deref());
        }
        PendingOfflineQueue::Clear => {
            let _ = manager.clear_offline_queue_state();
        }
    }
}

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
        let handle = spawn_persistence_actor(manager);

        let state = OfflineQueueState {
            messages: vec![QueuedSessionMessage {
                display: "queued from enter".to_string(),
                skill_instruction: None,
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
    }
}
