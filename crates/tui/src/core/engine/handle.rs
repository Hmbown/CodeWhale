//! Public `EngineHandle` methods.
//!
//! The struct itself lives next door in `engine.rs` because two
//! construction sites (`Engine::new` and the test-only
//! `mock_engine_handle`) need access to its private mpsc channels.
//! The method surface — `send`, `cancel*`, `is_cancelled`,
//! `approve_tool_call` / `deny_tool_call` / `retry_tool_with_policy`,
//! `submit_user_input` / `cancel_user_input`, and `steer` — moves here
//! so the agent loop's mailbox API is reviewable on its own.

use anyhow::Result;

use super::approval::{ApprovalDecision, UserInputDecision};
use super::{CancelReason, EngineHandle, Op, UserInputResponse};

impl EngineHandle {
    /// Send an operation to the engine
    pub async fn send(&self, op: Op) -> Result<()> {
        self.tx_op.send(op).await?;
        Ok(())
    }

    /// Try to send an operation without blocking.
    ///
    /// Returns `Err` if the channel is full or closed.  Use this for
    /// non-critical, refresh-type ops (e.g. `Op::ListSubAgents`) that can
    /// safely be dropped and re-requested on the next drain cycle.
    pub fn try_send(&self, op: Op) -> Result<()> {
        self.tx_op.try_send(op)?;
        Ok(())
    }

    /// Cancel the current request (user-initiated path — keeps the
    /// public `cancel()` signature stable). Equivalent to
    /// `cancel_with_reason(CancelReason::User)`.
    pub fn cancel(&self) {
        self.cancel_with_reason(CancelReason::User);
    }

    /// Cancel the current request and latch the reason so downstream
    /// "request cancelled" error messages can name a cause.
    pub fn cancel_with_reason(&self, reason: CancelReason) {
        match self.cancel_reason.lock() {
            Ok(mut slot) => *slot = Some(reason),
            Err(poisoned) => *poisoned.into_inner() = Some(reason),
        }
        match self.cancel_token.lock() {
            Ok(token) => token.cancel(),
            Err(poisoned) => poisoned.into_inner().cancel(),
        }
        crate::retry_status::clear();
    }

    /// Check if a request is currently cancelled
    #[must_use]
    #[allow(dead_code)]
    pub fn is_cancelled(&self) -> bool {
        match self.cancel_token.lock() {
            Ok(token) => token.is_cancelled(),
            Err(poisoned) => poisoned.into_inner().is_cancelled(),
        }
    }

    /// Pause or resume the current pausable command.
    pub fn set_paused(&self, paused: bool) {
        match self.shared_paused.lock() {
            Ok(mut slot) => *slot = paused,
            Err(poisoned) => *poisoned.into_inner() = paused,
        }
    }

    /// Check whether the engine pause gate is set.
    #[cfg(test)]
    #[must_use]
    pub fn is_paused(&self) -> bool {
        match self.shared_paused.lock() {
            Ok(slot) => *slot,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    /// Approve a pending tool call.
    ///
    /// Script-issued (WhaleFlow PTC) approvals resolve through the broker;
    /// ids not pending there are model-issued and flow to the engine's
    /// approval channel exactly as before. Front-ends call this one method
    /// for both, so session-cache auto-approve/deny covers PTC calls too.
    pub async fn approve_tool_call(&self, id: impl Into<String>) -> Result<()> {
        let id = id.into();
        if self
            .approval_broker
            .resolve(&id, crate::tools::approval_broker::BrokerVerdict::Approved)
        {
            return Ok(());
        }
        self.tx_approval
            .send(ApprovalDecision::Approved { id })
            .await?;
        Ok(())
    }

    /// Deny a pending tool call (broker-first; see [`Self::approve_tool_call`]).
    pub async fn deny_tool_call(&self, id: impl Into<String>) -> Result<()> {
        let id = id.into();
        if self
            .approval_broker
            .resolve(&id, crate::tools::approval_broker::BrokerVerdict::Denied)
        {
            return Ok(());
        }
        self.tx_approval
            .send(ApprovalDecision::Denied { id })
            .await?;
        Ok(())
    }

    /// Retry a tool call with an elevated sandbox policy (broker-first; see
    /// [`Self::approve_tool_call`]).
    pub async fn retry_tool_with_policy(
        &self,
        id: impl Into<String>,
        policy: crate::sandbox::SandboxPolicy,
    ) -> Result<()> {
        let id = id.into();
        if self.approval_broker.resolve(
            &id,
            crate::tools::approval_broker::BrokerVerdict::RetryWithPolicy(policy.clone()),
        ) {
            return Ok(());
        }
        self.tx_approval
            .send(ApprovalDecision::RetryWithPolicy { id, policy })
            .await?;
        Ok(())
    }

    /// Submit a response for request_user_input.
    pub async fn submit_user_input(
        &self,
        id: impl Into<String>,
        response: UserInputResponse,
    ) -> Result<()> {
        self.tx_user_input
            .send(UserInputDecision::Submitted {
                id: id.into(),
                response,
            })
            .await?;
        Ok(())
    }

    /// Cancel a request_user_input prompt.
    pub async fn cancel_user_input(&self, id: impl Into<String>) -> Result<()> {
        self.tx_user_input
            .send(UserInputDecision::Cancelled { id: id.into() })
            .await?;
        Ok(())
    }

    /// Steer an in-flight turn with additional user input.
    pub async fn steer(&self, content: impl Into<String>) -> Result<()> {
        self.tx_steer.send(content.into()).await?;
        Ok(())
    }

    /// Request a snapshot of the current session state.
    /// Returns the snapshot directly via a oneshot channel, avoiding
    /// competition with the SSE event stream on the mpsc receiver.
    pub async fn get_session_snapshot(&self) -> Result<crate::core::ops::SessionSnapshot> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
        self.send(Op::GetSessionSnapshot { tx }).await?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Engine dropped session snapshot oneshot"))
    }

    /// Request active provider request concurrency state.
    pub async fn get_provider_runtime_status(
        &self,
    ) -> Result<crate::core::ops::ProviderRuntimeStatus> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
        self.send(Op::GetProviderRuntimeStatus { tx }).await?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Engine dropped provider runtime status oneshot"))
    }
}
