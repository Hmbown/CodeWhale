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
use tokio::sync::mpsc;

use super::approval::{ApprovalDecision, UserInputDecision};
use super::{
    CancelReason, EngineHandle, LiveRuntimeAuthority, Op, RuntimePermissionAuthority,
    UserInputResponse,
};

impl EngineHandle {
    /// True when the caller must preflight a concrete provider client before
    /// committing UI/runtime turn state. Test and embedding handles with an
    /// injected model client return false because that client owns model I/O.
    #[must_use]
    pub(crate) fn client_preflight_required(&self) -> bool {
        self.client_preflight_required
    }

    /// Send an operation to the engine
    pub async fn send(&self, op: Op) -> Result<()> {
        let authority = Self::change_mode_authority(&op);
        let permit = self.tx_op.reserve().await?;
        if let Some(authority) = authority {
            self.publish_runtime_authority(authority);
        }
        permit.send(op);
        Ok(())
    }

    /// Try to send an operation without blocking.
    ///
    /// Returns `Err` if the channel is full or closed.  Use this for
    /// non-critical, refresh-type ops (e.g. `Op::ListSubAgents`) that can
    /// safely be dropped and re-requested on the next drain cycle.
    pub fn try_send(&self, op: Op) -> Result<()> {
        let authority = Self::change_mode_authority(&op);
        let result = self.tx_op.try_send(op);
        // A full channel already guarantees that the engine will wake and
        // drain an operation. Publish the typed authority anyway: the drain
        // applies pending authority before handling that queued operation, so
        // a posture edit never blocks behind refresh traffic. A closed
        // channel has no engine left to observe the update.
        if !matches!(&result, Err(mpsc::error::TrySendError::Closed(_)))
            && let Some(authority) = authority
        {
            self.publish_runtime_authority(authority);
        }
        result?;
        Ok(())
    }

    fn change_mode_authority(op: &Op) -> Option<LiveRuntimeAuthority> {
        let Op::ChangeMode {
            mode,
            allow_shell,
            trust_mode,
            auto_approve,
            approval_mode,
            configured_sandbox_mode,
        } = op
        else {
            return None;
        };
        Some(LiveRuntimeAuthority::from_fields(
            *mode,
            *allow_shell,
            *trust_mode,
            *auto_approve,
            *approval_mode,
            configured_sandbox_mode.clone(),
        ))
    }

    fn publish_runtime_authority(&self, authority: LiveRuntimeAuthority) {
        let mut state = self
            .live_runtime_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.revision = state.revision.wrapping_add(1).max(1);
        state.authority = authority;
    }

    pub(crate) fn publish_turn_authority(
        &self,
        mode: crate::tui::app::AppMode,
        allow_shell: bool,
        trust_mode: bool,
        auto_approve: bool,
        approval_mode: crate::tui::approval::ApprovalMode,
        configured_sandbox_mode: Option<String>,
    ) {
        self.publish_runtime_authority(LiveRuntimeAuthority::from_fields(
            mode,
            allow_shell,
            trust_mode,
            auto_approve,
            approval_mode,
            configured_sandbox_mode,
        ));
    }

    /// Exact live permission authority for runtime approval and elevation
    /// gates. This is the same typed state the active engine turn drains.
    #[must_use]
    pub(crate) fn runtime_permission_authority(&self) -> RuntimePermissionAuthority {
        self.live_runtime_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .authority
            .permission_snapshot()
    }

    /// Reserve capacity for a runtime steer before it mutates durable state.
    /// The owned permit lets the caller persist and dispatch synchronously,
    /// without a cancellation point between those two operations.
    pub(crate) async fn reserve_steer(&self) -> Result<mpsc::OwnedPermit<String>> {
        Ok(self.tx_steer.clone().reserve_owned().await?)
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

    /// Approve a pending tool call
    pub async fn approve_tool_call(&self, id: impl Into<String>) -> Result<()> {
        self.tx_approval
            .send(ApprovalDecision::Approved { id: id.into() })
            .await?;
        Ok(())
    }

    /// Deny a pending tool call
    pub async fn deny_tool_call(&self, id: impl Into<String>) -> Result<()> {
        self.tx_approval
            .send(ApprovalDecision::Denied { id: id.into() })
            .await?;
        Ok(())
    }

    /// Retry a tool call with an elevated sandbox policy.
    pub async fn retry_tool_with_policy(
        &self,
        id: impl Into<String>,
        policy: crate::sandbox::SandboxPolicy,
    ) -> Result<()> {
        self.tx_approval
            .send(ApprovalDecision::RetryWithPolicy {
                id: id.into(),
                policy,
            })
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

    /// Force the engine-owned MCP pool to reload and reconnect, returning a
    /// snapshot from the exact live pool that supplies the next model turn.
    pub async fn reload_mcp(
        &self,
        config_path: std::path::PathBuf,
    ) -> Result<crate::mcp::McpManagerSnapshot> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
        self.send(Op::ReloadMcp { config_path, tx }).await?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Engine dropped MCP reload oneshot"))?
            .map_err(anyhow::Error::msg)
    }
}
