//! Approval + user-input handshake for the agent loop.
//!
//! Extracted from `core/engine.rs` (P1.3). The agent loop blocks on these
//! two futures whenever a tool requires explicit approval (`await_tool_approval`)
//! or whenever a tool requests live user input (`await_user_input`). Channels
//! and engine state stay private to the parent module.

use std::time::Duration;

use crate::approval_log::{ApprovalOutcome, ApprovalReceipt};
use crate::core::events::Event;
use crate::tools::spec::ToolError;
use crate::tools::user_input::{UserInputRequest, UserInputResponse};

const USER_INPUT_TIMEOUT: Duration = Duration::from_secs(300);

use super::Engine;

#[derive(Debug, Clone)]
pub(super) enum ApprovalDecision {
    Approved {
        id: String,
    },
    Denied {
        id: String,
    },
    /// Retry a tool with an elevated sandbox policy.
    RetryWithPolicy {
        id: String,
        policy: crate::sandbox::SandboxPolicy,
    },
}

#[derive(Debug, Clone)]
pub(super) enum UserInputDecision {
    Submitted {
        id: String,
        response: UserInputResponse,
    },
    Cancelled {
        id: String,
    },
}

/// Result of awaiting tool approval from the user.
#[derive(Debug)]
pub(super) enum ApprovalResult {
    /// User approved the tool execution.
    Approved,
    /// User denied the tool execution.
    Denied,
    /// User requested retry with an elevated sandbox policy.
    RetryWithPolicy(crate::sandbox::SandboxPolicy),
}

impl Engine {
    async fn commit_approval_receipt(&self, receipt: ApprovalReceipt) -> Result<(), ToolError> {
        let store = self.approval_receipt_store.clone().map_err(|error| {
            tracing::warn!(
                target: "approval",
                %error,
                "approval receipt store is unavailable"
            );
            ToolError::execution_failed(
                "Approval evidence could not be committed; tool execution was blocked.".to_string(),
            )
        })?;
        let session_id = self.session.id.clone();
        let write = tokio::task::spawn_blocking(move || store.append(&session_id, &receipt))
            .await
            .map_err(|error| {
                tracing::warn!(
                    target: "approval",
                    %error,
                    "approval receipt writer did not complete"
                );
                ToolError::execution_failed(
                    "Approval evidence could not be committed; tool execution was blocked."
                        .to_string(),
                )
            })?;
        write.map_err(|error| {
            tracing::warn!(
                target: "approval",
                error_kind = ?error.kind(),
                "approval receipt write failed"
            );
            ToolError::execution_failed(
                "Approval evidence could not be committed; tool execution was blocked.".to_string(),
            )
        })
    }

    async fn commit_approval_outcome(
        &self,
        tool_id: &str,
        outcome: ApprovalOutcome,
    ) -> Result<(), ToolError> {
        self.commit_approval_receipt(ApprovalReceipt::decided(tool_id, outcome))
            .await
    }

    pub(super) async fn request_tool_approval(
        &mut self,
        tool_id: &str,
        tool_name: &str,
        event: Event,
    ) -> Result<ApprovalResult, ToolError> {
        self.commit_approval_receipt(ApprovalReceipt::asked(tool_id, tool_name))
            .await?;
        if self.tx_event.send(event).await.is_err() {
            self.commit_approval_outcome(tool_id, ApprovalOutcome::Unavailable)
                .await?;
            return Err(ToolError::execution_failed(
                "Approval request could not reach its decision host; tool execution was blocked."
                    .to_string(),
            ));
        }
        // R1: the per-turn wall-clock budget bounds what the agent spends on
        // its own, not how long a person takes to answer. Pause it across the
        // human decision — otherwise an approval prompt left open would fail
        // the turn (and discard the work just approved) the moment the user
        // came back. Every non-unwinding exit of `await_tool_approval` runs
        // through the resume below; a panic unwinds out of `run_turn`, which
        // restarts the clock on its next turn anyway.
        self.turn_wall_clock.begin_human_wait();
        let decision = self.await_tool_approval(tool_id).await;
        self.turn_wall_clock.end_human_wait();
        decision
    }

    /// Format a cancellation suffix when the engine knows the cause.
    /// Some internal cancellation paths still use the raw token while
    /// #1541 is open; those keep the legacy message without a guessed
    /// reason.
    fn cancel_reason_suffix(&self) -> String {
        let reason = match self.cancel_reason.lock() {
            Ok(slot) => *slot,
            Err(poisoned) => *poisoned.into_inner(),
        };
        match reason {
            Some(reason) => format!(" (reason: {})", reason.describe()),
            None => String::new(),
        }
    }

    pub(super) async fn await_tool_approval(
        &mut self,
        tool_id: &str,
    ) -> Result<ApprovalResult, ToolError> {
        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    let suffix = self.cancel_reason_suffix();
                    self.commit_approval_outcome(tool_id, ApprovalOutcome::Cancelled).await?;
                    return Err(ToolError::cancelled(
                        format!("Request cancelled while awaiting approval{suffix}"),
                    ));
                }
                decision = self.rx_approval.recv() => {
                    let Some(decision) = decision else {
                        self.commit_approval_outcome(tool_id, ApprovalOutcome::Unavailable).await?;
                        return Err(ToolError::execution_failed(
                            "Approval channel closed — engine is shutting down. \
                             The approval modal can no longer reach the engine; \
                             this is typically a teardown race, not a user action."
                                .to_string(),
                        ));
                    };
                    match decision {
                        ApprovalDecision::Approved { id } if id == tool_id => {
                            self.commit_approval_outcome(tool_id, ApprovalOutcome::ApprovedOnce).await?;
                            return Ok(ApprovalResult::Approved);
                        }
                        ApprovalDecision::Denied { id } if id == tool_id => {
                            self.commit_approval_outcome(tool_id, ApprovalOutcome::Denied).await?;
                            return Ok(ApprovalResult::Denied);
                        }
                        ApprovalDecision::RetryWithPolicy { id, policy } if id == tool_id => {
                            self.commit_approval_outcome(
                                tool_id,
                                ApprovalOutcome::RetryWithPolicy { policy: policy.clone() },
                            ).await?;
                            return Ok(ApprovalResult::RetryWithPolicy(policy));
                        }
                        // A child prompt answered while the parent itself is
                        // waiting: hand it to the child instead of dropping it.
                        other => {
                            self.route_child_approval_decision(other).await;
                            continue;
                        }
                    }
                }
            }
        }
    }

    pub(super) async fn await_user_input(
        &mut self,
        tool_id: &str,
        request: UserInputRequest,
    ) -> Result<UserInputResponse, ToolError> {
        let _ = self
            .tx_event
            .send(Event::UserInputRequired {
                id: tool_id.to_string(),
                request,
            })
            .await;

        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    let suffix = self.cancel_reason_suffix();
                    return Err(ToolError::cancelled(
                        format!("Request cancelled while awaiting user input{suffix}"),
                    ));
                }
                result = tokio::time::timeout(USER_INPUT_TIMEOUT, self.rx_user_input.recv()) => {
                    match result {
                        Ok(Some(decision)) => {
                            match decision {
                                UserInputDecision::Submitted { id, response } if id == tool_id => {
                                    return Ok(response);
                                }
                                UserInputDecision::Cancelled { id } if id == tool_id => {
                                    return Err(ToolError::cancelled(
                                        "User input cancelled".to_string(),
                                    ));
                                }
                                _ => continue,
                            }
                        }
                        Ok(None) => {
                            return Err(ToolError::execution_failed(
                                "User input channel closed".to_string(),
                            ));
                        }
                        Err(_) => {
                            let _ = self
                                .tx_event
                                .send(Event::Status {
                                    message: format!(
                                        "User input timed out after {}s",
                                        USER_INPUT_TIMEOUT.as_secs()
                                    ),
                                })
                                .await;
                            return Err(ToolError::Timeout {
                                seconds: USER_INPUT_TIMEOUT.as_secs(),
                            });
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::core::engine::EngineConfig;
    use crate::sandbox::SandboxPolicy;

    fn approval_event(tool_id: &str) -> Event {
        Event::ApprovalRequired {
            id: tool_id.to_string(),
            tool_name: "exec_shell".to_string(),
            description: "run a keyless approval test".to_string(),
            input: serde_json::json!({"command": "true"}),
            approval_key: format!("key-{tool_id}"),
            approval_grouping_key: "exec_shell:true".to_string(),
            intent_summary: None,
            approval_force_prompt: false,
        }
    }

    #[tokio::test]
    async fn keyless_engine_persists_every_closed_approval_outcome() {
        enum Decision {
            Approve,
            Deny,
            Cancel,
            Retry,
        }
        let cases = [
            (Decision::Approve, ApprovalOutcome::ApprovedOnce),
            (Decision::Deny, ApprovalOutcome::Denied),
            (Decision::Cancel, ApprovalOutcome::Cancelled),
            (
                Decision::Retry,
                ApprovalOutcome::RetryWithPolicy {
                    policy: SandboxPolicy::DangerFullAccess,
                },
            ),
        ];

        for (index, (decision, expected)) in cases.into_iter().enumerate() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let (mut engine, handle) = Engine::new(EngineConfig::default(), &Config::default());
            let store = crate::approval_log::ApprovalReceiptStore::new(tmp.path().join("sessions"));
            engine.approval_receipt_store = Ok(store.clone());
            let session_id = engine.session.id.clone();
            let tool_id = format!("tool-{index}");
            let event = approval_event(&tool_id);
            let pending_tool_id = tool_id.clone();
            let task = tokio::spawn(async move {
                engine
                    .request_tool_approval(&pending_tool_id, "exec_shell", event)
                    .await
            });

            let emitted = handle
                .rx_event
                .write()
                .await
                .recv()
                .await
                .expect("approval event");
            assert!(matches!(emitted, Event::ApprovalRequired { .. }));
            match decision {
                Decision::Approve => handle.approve_tool_call(&tool_id).await.expect("approve"),
                Decision::Deny => handle.deny_tool_call(&tool_id).await.expect("deny"),
                Decision::Cancel => handle.cancel(),
                Decision::Retry => handle
                    .retry_tool_with_policy(&tool_id, SandboxPolicy::DangerFullAccess)
                    .await
                    .expect("retry"),
            }

            let result = task.await.expect("approval task");
            match expected {
                ApprovalOutcome::ApprovedOnce => {
                    assert!(matches!(result, Ok(ApprovalResult::Approved)));
                }
                ApprovalOutcome::Denied => {
                    assert!(matches!(result, Ok(ApprovalResult::Denied)));
                }
                ApprovalOutcome::Cancelled => assert!(result.is_err()),
                ApprovalOutcome::RetryWithPolicy { .. } => {
                    assert!(matches!(result, Ok(ApprovalResult::RetryWithPolicy(_))));
                }
                ApprovalOutcome::Unavailable => unreachable!(),
            }
            let replay = store.replay(&session_id).expect("replay approvals");
            assert_eq!(replay.completed.len(), 1);
            assert_eq!(replay.completed[0].outcome, expected);
            assert!(replay.unmatched_asks.is_empty());
        }
    }

    #[tokio::test]
    async fn closed_approval_channel_is_persisted_as_unavailable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (mut engine, handle) = Engine::new(EngineConfig::default(), &Config::default());
        let store = crate::approval_log::ApprovalReceiptStore::new(tmp.path().join("sessions"));
        engine.approval_receipt_store = Ok(store.clone());
        let session_id = engine.session.id.clone();
        let events = handle.rx_event.clone();
        drop(handle);

        let task = tokio::spawn(async move {
            engine
                .request_tool_approval(
                    "tool-unavailable",
                    "exec_shell",
                    approval_event("tool-unavailable"),
                )
                .await
        });
        let emitted = events
            .write()
            .await
            .recv()
            .await
            .expect("approval event before channel closure is observed");
        assert!(matches!(emitted, Event::ApprovalRequired { .. }));
        assert!(task.await.expect("approval task").is_err());

        let replay = store.replay(&session_id).expect("replay approvals");
        assert_eq!(replay.completed.len(), 1);
        assert_eq!(replay.completed[0].outcome, ApprovalOutcome::Unavailable);
    }

    #[tokio::test]
    async fn stale_approval_decision_cannot_grant_current_request() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (mut engine, handle) = Engine::new(EngineConfig::default(), &Config::default());
        let store = crate::approval_log::ApprovalReceiptStore::new(tmp.path().join("sessions"));
        engine.approval_receipt_store = Ok(store.clone());
        let session_id = engine.session.id.clone();
        let mut task = tokio::spawn(async move {
            engine
                .request_tool_approval("tool-current", "exec_shell", approval_event("tool-current"))
                .await
        });

        let emitted = handle
            .rx_event
            .write()
            .await
            .recv()
            .await
            .expect("approval event");
        assert!(matches!(emitted, Event::ApprovalRequired { .. }));
        handle
            .approve_tool_call("tool-stale")
            .await
            .expect("deliver stale decision");
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut task)
                .await
                .is_err(),
            "a stale decision must not grant or close the current request"
        );
        handle
            .deny_tool_call("tool-current")
            .await
            .expect("deny current request");
        assert!(matches!(
            task.await.expect("approval task"),
            Ok(ApprovalResult::Denied)
        ));

        let replay = store.replay(&session_id).expect("replay approvals");
        assert_eq!(replay.completed.len(), 1);
        assert_eq!(replay.completed[0].outcome, ApprovalOutcome::Denied);
        assert!(replay.unmatched_asks.is_empty());
    }

    #[tokio::test]
    async fn terminal_receipt_failure_never_returns_a_grant() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (mut engine, handle) = Engine::new(EngineConfig::default(), &Config::default());
        let store = crate::approval_log::ApprovalReceiptStore::new(tmp.path().join("sessions"));
        engine.approval_receipt_store = Ok(store.clone());
        let session_id = engine.session.id.clone();
        let task = tokio::spawn(async move {
            engine
                .request_tool_approval(
                    "tool-write-fails",
                    "exec_shell",
                    approval_event("tool-write-fails"),
                )
                .await
        });

        let emitted = handle
            .rx_event
            .write()
            .await
            .recv()
            .await
            .expect("approval event");
        assert!(matches!(emitted, Event::ApprovalRequired { .. }));
        let log_path = store
            .sessions_dir()
            .join(session_id)
            .join("approval_receipts.jsonl");
        std::fs::remove_file(&log_path).expect("remove log after durable ask");
        std::fs::create_dir(&log_path).expect("replace log with unwritable directory");
        handle
            .approve_tool_call("tool-write-fails")
            .await
            .expect("deliver approval decision");

        assert!(
            task.await.expect("approval task").is_err(),
            "an approval decision without a committed terminal receipt must not grant execution"
        );
    }
}
