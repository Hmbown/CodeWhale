//! Out-of-band approval resolution for script-issued (PTC) tool calls.
//!
//! While a tool such as `whaleflow` executes, the engine task is parked
//! inside `execute_tool_with_lock(...).await` and is **not** draining its
//! private `rx_approval` channel. A WhaleFlow script's `tools.<name>()` call
//! that needs user approval therefore cannot ride the engine's approval
//! channel — decisions sent while the engine is busy would sit buffered and
//! later be discarded as non-matching ids.
//!
//! The [`ToolApprovalBroker`] closes that gap: the WhaleFlow driver registers
//! a pending call id here and emits the same `Event::ApprovalRequired` the
//! model path emits; every front-end resolves decisions through
//! `EngineHandle::approve_tool_call` / `deny_tool_call` /
//! `retry_tool_with_policy`, which consult the broker **first** and only fall
//! through to the engine's channel for model-issued ids. Front-ends (TUI,
//! headless runtime API) need zero changes — including their session-cache
//! auto-approve/auto-deny logic, which fires before any modal.
//!
//! This module also owns the shared approval-requirement predicate so the
//! model path (`turn_loop.rs`) and the script path (`whaleflow_bridge.rs`)
//! decide "does this call need a prompt?" through the *same function*.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::tools::spec::ApprovalRequirement;

/// The user's verdict for one brokered (script-issued) tool call.
#[derive(Debug, Clone)]
pub enum BrokerVerdict {
    Approved,
    Denied,
    /// Retry the tool with an elevated sandbox policy.
    RetryWithPolicy(crate::sandbox::SandboxPolicy),
}

/// Registry of pending script-issued approval requests, keyed by the id
/// carried on the emitted `Event::ApprovalRequired`.
#[derive(Default)]
pub struct ToolApprovalBroker {
    pending: Mutex<HashMap<String, tokio::sync::oneshot::Sender<BrokerVerdict>>>,
}

impl ToolApprovalBroker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a pending approval and return the receiver the requester
    /// awaits. A second registration under the same id replaces (and thereby
    /// closes) the first — ids are uuid-fresh per call, so this never fires
    /// in practice.
    pub fn register(&self, id: &str) -> tokio::sync::oneshot::Receiver<BrokerVerdict> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.lock_pending().insert(id.to_string(), tx);
        rx
    }

    /// Drop a pending registration (cancellation / teardown). A later
    /// `resolve` for this id returns `false` and the decision falls through
    /// to the engine's normal approval channel, where it is discarded.
    pub fn unregister(&self, id: &str) {
        self.lock_pending().remove(id);
    }

    /// Deliver a verdict. Returns `true` iff `id` was pending here and the
    /// verdict was delivered (i.e. the caller must NOT also forward the
    /// decision to the engine's approval channel).
    pub fn resolve(&self, id: &str, verdict: BrokerVerdict) -> bool {
        let Some(tx) = self.lock_pending().remove(id) else {
            return false;
        };
        // A dropped receiver means the waiter already gave up (cancelled
        // between unregister and this resolve). The entry was pending here
        // either way, so the decision is consumed, not forwarded.
        let _ = tx.send(verdict);
        true
    }

    fn lock_pending(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, tokio::sync::oneshot::Sender<BrokerVerdict>>>
    {
        match self.pending.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Whether an approval prompt is required for a registered tool call.
///
/// Shared by the engine turn loop (model-issued calls) and the WhaleFlow
/// driver (script-issued calls) so both paths apply identical semantics:
/// `Auto` never prompts; a non-bypassable tool always prompts; everything
/// else prompts unless the session is auto-approved.
pub(crate) fn registered_tool_approval_required(
    tool_name: &str,
    requirement: ApprovalRequirement,
    auto_approve: bool,
) -> bool {
    if requirement == ApprovalRequirement::Auto {
        return false;
    }
    if registered_tool_requires_non_bypassable_approval(tool_name) {
        return true;
    }
    !auto_approve
}

/// Tools that must prompt even under auto-approve (YOLO).
pub(crate) fn registered_tool_requires_non_bypassable_approval(tool_name: &str) -> bool {
    matches!(tool_name, "rlm_eval")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_delivers_verdict_to_registered_waiter() {
        let broker = ToolApprovalBroker::new();
        let mut rx = broker.register("wfcall_1");
        assert!(broker.resolve("wfcall_1", BrokerVerdict::Approved));
        match rx.try_recv() {
            Ok(BrokerVerdict::Approved) => {}
            other => panic!("expected Approved, got {other:?}"),
        }
    }

    #[test]
    fn resolve_unknown_id_returns_false() {
        let broker = ToolApprovalBroker::new();
        assert!(!broker.resolve("nope", BrokerVerdict::Denied));
    }

    #[test]
    fn unregister_makes_later_resolve_fall_through() {
        let broker = ToolApprovalBroker::new();
        let _rx = broker.register("wfcall_2");
        broker.unregister("wfcall_2");
        assert!(
            !broker.resolve("wfcall_2", BrokerVerdict::Approved),
            "an unregistered id must fall through to the engine channel"
        );
    }

    #[test]
    fn dropped_receiver_still_consumes_the_decision() {
        let broker = ToolApprovalBroker::new();
        drop(broker.register("wfcall_3"));
        assert!(
            broker.resolve("wfcall_3", BrokerVerdict::Denied),
            "a still-registered entry consumes the decision even if the waiter is gone"
        );
    }
}
