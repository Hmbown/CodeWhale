//! Approval + user-input handshake for the agent loop.
//!
//! Extracted from `core/engine.rs` (P1.3). The agent loop blocks on these
//! two futures whenever a tool requires explicit approval (`await_tool_approval`)
//! or whenever a tool requests live user input (`await_user_input`). Channels
//! and engine state stay private to the parent module.

use std::time::Duration;

use crate::core::events::Event;
use crate::tools::spec::ToolError;
use crate::tools::user_input::{UserInputRequest, UserInputResponse};

const USER_INPUT_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_LATE_SENSITIVE_PROJECTION_ATTEMPTS: u32 = 5;

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
    /// Once an answer is classified as private, all attached durable child
    /// sinks must converge before the exact response can resume the provider
    /// turn. Provenance is extended first, so concurrent/new writes already
    /// project through it; retrying both sink families on every pass also
    /// repairs a partial prior pass instead of treating set membership as a
    /// completed-cleanup receipt. Permanent failures stop after a bounded
    /// number of passes and cancellation interrupts backoff; either path fails
    /// closed without returning the exact response to the provider.
    async fn refresh_late_sensitive_projections(&mut self) -> Result<(), ToolError> {
        let mut attempt = 0u32;
        loop {
            attempt = attempt.saturating_add(1);
            #[cfg(test)]
            {
                self.late_sensitive_projection_attempts = attempt;
            }
            let workflow_result = crate::tools::workflow::refresh_sensitive_user_input_provenance(
                &self.session.sensitive_user_input_provenance,
            );
            let child_result = self
                .subagent_manager
                .write()
                .await
                .refresh_sensitive_user_input_provenance(
                    &self.session.sensitive_user_input_provenance,
                );
            #[cfg(test)]
            let forced_failure = self.force_late_sensitive_projection_failure;
            #[cfg(not(test))]
            let forced_failure = false;
            let workflow_clean = workflow_result.is_ok() && !forced_failure;
            let child_clean = child_result.is_ok() && !forced_failure;
            if workflow_clean && child_clean {
                return Ok(());
            }

            // Do not return the exact answer while any known public/durable
            // sink is dirty. Error details can include paths or stale prose,
            // so the operational warning intentionally reports only the pass.
            tracing::warn!(
                attempt,
                workflow_clean,
                child_clean,
                "late privacy projection incomplete; retrying before provider resume"
            );
            if attempt >= MAX_LATE_SENSITIVE_PROJECTION_ATTEMPTS {
                return Err(ToolError::execution_failed(
                    "Private user input was not forwarded because Codewhale could not complete durable privacy cleanup"
                        .to_string(),
                ));
            }
            let shift = attempt.saturating_sub(1).min(4);
            let backoff_ms = 50u64.saturating_mul(1u64 << shift);
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    let suffix = self.cancel_reason_suffix();
                    return Err(ToolError::cancelled(format!(
                        "Request cancelled during private-data cleanup{suffix}"
                    )));
                }
                () = tokio::time::sleep(Duration::from_millis(backoff_ms)) => {}
            }
        }
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
                    return Err(ToolError::cancelled(
                        format!("Request cancelled while awaiting approval{suffix}"),
                    ));
                }
                decision = self.rx_approval.recv() => {
                    let Some(decision) = decision else {
                        return Err(ToolError::execution_failed(
                            "Approval channel closed — engine is shutting down. \
                             The approval modal can no longer reach the engine; \
                             this is typically a teardown race, not a user action."
                                .to_string(),
                        ));
                    };
                    match decision {
                        ApprovalDecision::Approved { id } if id == tool_id => {
                            return Ok(ApprovalResult::Approved);
                        }
                        ApprovalDecision::Denied { id } if id == tool_id => {
                            return Ok(ApprovalResult::Denied);
                        }
                        ApprovalDecision::RetryWithPolicy { id, policy } if id == tool_id => {
                            return Ok(ApprovalResult::RetryWithPolicy(policy));
                        }
                        _ => continue,
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
        let public_request = crate::runtime_threads::redacted_user_input_request_for_public(
            &request,
            &self.session.sensitive_user_input_provenance.snapshot(),
        );
        let _ = self
            .tx_event
            .send(Event::UserInputRequired {
                id: tool_id.to_string(),
                request: public_request,
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
                                    let mut sensitive_values = std::collections::HashSet::new();
                                    crate::runtime_threads::collect_sensitive_user_input_response_values(
                                        &request,
                                        &response,
                                        &mut sensitive_values,
                                    );
                                    let has_sensitive_values = !sensitive_values.is_empty();
                                    self.session
                                        .sensitive_user_input_provenance
                                        .extend(sensitive_values);
                                    if has_sensitive_values {
                                        self.refresh_late_sensitive_projections().await?;
                                    }
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
