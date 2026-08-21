//! Session-owned outbox events at terminating-signal time.
//!
//! The outbox's turn boundaries are normally emitted engine-side; a session
//! killed mid-turn by a catchable signal (SIGTERM from a pane teardown,
//! SIGHUP from a closed pane, SIGINT) never reaches the `turn_end` emit, so
//! its `turn_start` would stay unpaired in the shared outbox. This module
//! registers the session's outbox handle and thread identity in a
//! process-global so the terminating-signal cleanup task in
//! [`crate::spawn_signal_cleanup_task`] can append the missing `turn_end`
//! before the process exits.
//!
//! The flush derives the open turn from the *file* via
//! [`codewhale_hooks::LifecycleOutbox::reconcile_interrupted_turns`], never
//! from in-memory turn state: the scan + append run under the outbox's
//! cross-process lock, so the flush can never fabricate a duplicate
//! `turn_end` (G1) even if it races the session's own writer task. SIGKILL
//! runs no code at all; the same reconciliation function covers it at the
//! next boot, which is why both paths share one mechanism.

use std::sync::{Mutex, OnceLock};

use codewhale_hooks::LifecycleOutbox;

/// The session's outbox handle plus the thread id its events carry. Set once
/// per session, at startup, by the surface that owns the outbox emits (the
/// interactive TUI). Surfaces that never register (exec, CLI subcommands)
/// get a no-op flush.
static OUTBOX_SIGNAL_CONTEXT: OnceLock<Mutex<Option<(LifecycleOutbox, String)>>> = OnceLock::new();

fn context() -> &'static Mutex<Option<(LifecycleOutbox, String)>> {
    OUTBOX_SIGNAL_CONTEXT.get_or_init(|| Mutex::new(None))
}

/// Register this session's outbox identity for the signal path. Called once
/// at startup, after the outbox handle is constructed and before any turn
/// can start. A disabled outbox still registers (the flush no-ops through
/// the handle), keeping the shape uniform.
pub(crate) fn register(outbox: LifecycleOutbox, thread_id: String) {
    if let Ok(mut guard) = context().lock() {
        *guard = Some((outbox, thread_id));
    }
}

/// Best-effort graceful-shutdown turn closure, called from the
/// terminating-signal cleanup task before the process exits.
///
/// `signal` is the human-readable signal name for the outbox payload
/// (`SIGTERM`, `SIGHUP`, `SIGINT`). Blocking: appends run synchronously
/// under the outbox lock, so the exit waits at most for the bounded queue
/// drain plus the append itself. Nothing here can fail the exit — the flush
/// logs and the signal path proceeds either way.
pub(crate) fn flush_open_turn_on_signal(signal: &str) {
    let Some((outbox, thread_id)) = context().lock().ok().and_then(|guard| guard.clone()) else {
        return;
    };
    if thread_id.is_empty() {
        return;
    }
    let reason = format!("signal:{signal}");
    match outbox.reconcile_interrupted_turns(&thread_id, &reason) {
        Ok(0) => {}
        Ok(reconciled) => {
            tracing::info!(
                target: "lifecycle_outbox",
                %thread_id,
                reconciled,
                %signal,
                "terminating-signal flush reconciled open turn(s)"
            );
        }
        Err(error) => {
            tracing::warn!(
                target: "lifecycle_outbox",
                %thread_id,
                %signal,
                %error,
                "terminating-signal flush could not reconcile open turn(s)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codewhale_hooks::{LifecycleEvent, LifecycleOutbox};
    use serde_json::json;

    /// The graceful-shutdown path: a registered session with an open turn
    /// (turn_start written, turn_end never) gets its synthetic end from the
    /// signal flush, with `status: interrupted`, `reconciled: true` and the
    /// signal named in `reason`. The flush is idempotent by file truth — a
    /// second flush must not duplicate the end.
    #[test]
    fn signal_flush_pairs_an_open_turn_and_does_not_duplicate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("signal-flush.jsonl");
        let outbox = LifecycleOutbox::new(Some(path.clone()), None, None);
        let thread_id = "sess_signal_test";
        let turn_id = "turn-open";
        outbox
            .emit_blocking(LifecycleEvent {
                event: "turn_start".to_string(),
                kind: "turn.started".to_string(),
                thread_id: thread_id.to_string(),
                turn_id: Some(turn_id.to_string()),
                item_id: None,
                payload: json!({ "workspace": "/tmp/signal-test" }),
            })
            .expect("turn_start");

        register(outbox, thread_id.to_string());
        flush_open_turn_on_signal("SIGTERM");

        let text = std::fs::read_to_string(&path).expect("read outbox");
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("json line"))
            .collect();
        assert_eq!(lines.len(), 2);
        let synthetic = &lines[1];
        assert_eq!(synthetic["event"], "turn_end");
        assert_eq!(synthetic["kind"], "turn.interrupted");
        assert_eq!(synthetic["thread_id"], thread_id);
        assert_eq!(synthetic["turn_id"], turn_id);
        assert_eq!(synthetic["payload"]["status"], "interrupted");
        assert_eq!(synthetic["payload"]["reconciled"], true);
        assert_eq!(synthetic["payload"]["reason"], "signal:SIGTERM");

        // A second flush must not duplicate the end.
        flush_open_turn_on_signal("SIGTERM");
        let text = std::fs::read_to_string(&path).expect("read outbox");
        assert_eq!(text.lines().count(), 2);
    }

    /// With no session registered, the flush is a silent no-op.
    #[test]
    fn signal_flush_is_a_noop_without_a_registered_session() {
        // The registry is process-global and shared with the test above;
        // clear it so this test observes the unregistered state.
        if let Ok(mut guard) = context().lock() {
            *guard = None;
        }
        flush_open_turn_on_signal("SIGTERM");
    }
}
