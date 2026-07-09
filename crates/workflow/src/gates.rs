//! Workflow gate nodes and role-to-role handoffs (#4179).
//!
//! Gates live in the Workflow definition; Fleet only supplies roles.
//! Handoff artifacts are **lane-scoped** (keyed by lane id), never fleet-scoped.
//!
//! Gate semantics:
//! - **block** — downstream role cannot start until the gate passes or a human
//!   override approves
//! - **approve** — promote an artifact into the next role's context substrate
//! - **escalate** — after N retries, surface to parent / lane status
//!
//! This module is pure IR + evaluation. Runtime execution and Lane status UI
//! wire in later; unit tests cover block/approve/retry/escalate paths.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// When a gate fires relative to role lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateOn {
    /// After a fleet role task completes successfully.
    RoleComplete,
    /// Before a fleet role is allowed to start.
    RoleStart,
}

/// Kind of verification / review gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateKind {
    /// Compile/test/lint suite (verifier role; #4013).
    Verify,
    /// Diff review (reviewer role).
    Review,
    /// Explicit human/operator approve.
    Approve,
}

/// Policy when a gate fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateOnFail {
    /// Re-run the upstream role (up to `max_retries`).
    Retry,
    /// Block downstream until resolved.
    Block,
    /// Surface to parent / lane status after retries exhausted.
    Escalate,
}

/// One gate node in a Workflow definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateSpec {
    pub id: String,
    /// Role whose completion (or start) triggers this gate.
    pub role: String,
    #[serde(rename = "on")]
    pub on: GateOn,
    pub gate: GateKind,
    pub on_fail: GateOnFail,
    /// Downstream role blocked until this gate passes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks_role: Option<String>,
    /// Max retries before escalate (default 1).
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Optional artifact type this gate produces/consumes (e.g. `findings`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_kind: Option<String>,
}

fn default_max_retries() -> u32 {
    1
}

/// Live state of one gate within a lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateState {
    Pending,
    Passed,
    Blocked { reason: String },
    Retrying { attempt: u32, reason: String },
    Escalated { reason: String },
}

impl GateState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Passed => "passed",
            Self::Blocked { .. } => "blocked",
            Self::Retrying { .. } => "retrying",
            Self::Escalated { .. } => "escalated",
        }
    }

    pub fn is_blocking(&self) -> bool {
        matches!(
            self,
            Self::Blocked { .. } | Self::Escalated { .. } | Self::Retrying { .. }
        )
    }

    pub fn blocked_reason(&self) -> Option<&str> {
        match self {
            Self::Blocked { reason }
            | Self::Retrying { reason, .. }
            | Self::Escalated { reason } => Some(reason.as_str()),
            _ => None,
        }
    }
}

/// Outcome reported by a verifier/reviewer/human for a gate evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateOutcome {
    Pass,
    Fail {
        reason: String,
    },
    /// Explicit human override that clears a block.
    HumanApprove {
        note: String,
    },
}

/// Lane-scoped handoff artifact produced by a role for the next role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffArtifact {
    pub id: String,
    pub lane_id: String,
    /// Producing fleet role (e.g. `scout`).
    pub from_role: String,
    /// Consuming fleet role (e.g. `implementer`).
    pub to_role: String,
    /// Artifact kind (`findings`, `diff`, `verify_report`, …).
    pub kind: String,
    /// Opaque payload (JSON text or path reference).
    pub payload: String,
    pub created_at: String,
}

/// In-memory (and serializable) gate + handoff store for one lane.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneGateBoard {
    pub lane_id: String,
    /// Gate id → current state.
    #[serde(default)]
    pub gates: BTreeMap<String, GateState>,
    /// Retry counters per gate id.
    #[serde(default)]
    pub retries: BTreeMap<String, u32>,
    /// Handoff artifacts for this lane.
    #[serde(default)]
    pub artifacts: Vec<HandoffArtifact>,
}

impl LaneGateBoard {
    pub fn new(lane_id: impl Into<String>) -> Self {
        Self {
            lane_id: lane_id.into(),
            gates: BTreeMap::new(),
            retries: BTreeMap::new(),
            artifacts: Vec::new(),
        }
    }

    /// Register gate specs in pending state.
    pub fn install_gates(&mut self, specs: &[GateSpec]) {
        for spec in specs {
            self.gates
                .entry(spec.id.clone())
                .or_insert(GateState::Pending);
        }
    }

    /// Evaluate a gate against an outcome; updates board state.
    pub fn evaluate(
        &mut self,
        spec: &GateSpec,
        outcome: GateOutcome,
    ) -> Result<GateState, GateError> {
        if spec.id.trim().is_empty() {
            return Err(GateError::EmptyGateId);
        }
        match outcome {
            GateOutcome::Pass | GateOutcome::HumanApprove { .. } => {
                let state = GateState::Passed;
                self.gates.insert(spec.id.clone(), state.clone());
                self.retries.remove(&spec.id);
                Ok(state)
            }
            GateOutcome::Fail { reason } => {
                let attempt = self.retries.entry(spec.id.clone()).or_insert(0);
                *attempt = attempt.saturating_add(1);
                let attempt = *attempt;
                let state = match spec.on_fail {
                    GateOnFail::Block => GateState::Blocked { reason },
                    GateOnFail::Retry if attempt <= spec.max_retries => {
                        GateState::Retrying { attempt, reason }
                    }
                    GateOnFail::Retry | GateOnFail::Escalate => GateState::Escalated { reason },
                };
                self.gates.insert(spec.id.clone(), state.clone());
                Ok(state)
            }
        }
    }

    /// Whether `role` is currently blocked by any gate that targets it.
    pub fn role_is_blocked(&self, specs: &[GateSpec], role: &str) -> Option<&GateState> {
        for spec in specs {
            let blocks = spec
                .blocks_role
                .as_deref()
                .unwrap_or("")
                .eq_ignore_ascii_case(role);
            if !blocks {
                continue;
            }
            if let Some(state) = self.gates.get(&spec.id)
                && state.is_blocking()
            {
                return Some(state);
            }
        }
        None
    }

    /// Record a scout→implementer (etc.) handoff artifact.
    pub fn record_handoff(&mut self, artifact: HandoffArtifact) -> Result<(), GateError> {
        if artifact.lane_id != self.lane_id {
            return Err(GateError::LaneMismatch {
                expected: self.lane_id.clone(),
                got: artifact.lane_id,
            });
        }
        self.artifacts.push(artifact);
        Ok(())
    }

    /// Latest handoff of `kind` from `from_role` to `to_role`, if any.
    pub fn latest_handoff(
        &self,
        from_role: &str,
        to_role: &str,
        kind: &str,
    ) -> Option<&HandoffArtifact> {
        self.artifacts.iter().rev().find(|a| {
            a.from_role.eq_ignore_ascii_case(from_role)
                && a.to_role.eq_ignore_ascii_case(to_role)
                && a.kind.eq_ignore_ascii_case(kind)
        })
    }

    /// Persist board under a lane directory (JSON).
    pub fn save_to_dir(&self, dir: &Path) -> Result<PathBuf, GateError> {
        std::fs::create_dir_all(dir).map_err(|e| GateError::Io(e.to_string()))?;
        let path = dir.join("gates.json");
        let json =
            serde_json::to_string_pretty(self).map_err(|e| GateError::Serde(e.to_string()))?;
        std::fs::write(&path, json).map_err(|e| GateError::Io(e.to_string()))?;
        Ok(path)
    }

    pub fn load_from_dir(dir: &Path) -> Result<Self, GateError> {
        let path = dir.join("gates.json");
        let text = std::fs::read_to_string(&path).map_err(|e| GateError::Io(e.to_string()))?;
        serde_json::from_str(&text).map_err(|e| GateError::Serde(e.to_string()))
    }

    /// Compact status for `lane status` / panel surfaces.
    pub fn status_summary(&self) -> Vec<GateStatusLine> {
        self.gates
            .iter()
            .map(|(id, state)| GateStatusLine {
                gate_id: id.clone(),
                state: state.as_str().to_string(),
                blocked_reason: state.blocked_reason().map(str::to_string),
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateStatusLine {
    pub gate_id: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GateError {
    #[error("gate id must not be empty")]
    EmptyGateId,
    #[error("handoff lane id `{got}` does not match board lane `{expected}`")]
    LaneMismatch { expected: String, got: String },
    #[error("io error: {0}")]
    Io(String),
    #[error("serde error: {0}")]
    Serde(String),
}

/// Canonical stopship-style gate pipeline (scout → implementer → reviewer → verifier → release_lead).
pub fn stopship_gate_pipeline() -> Vec<GateSpec> {
    vec![
        GateSpec {
            id: "scout-findings".into(),
            role: "scout".into(),
            on: GateOn::RoleComplete,
            gate: GateKind::Approve,
            on_fail: GateOnFail::Block,
            blocks_role: Some("implementer".into()),
            max_retries: 0,
            artifact_kind: Some("findings".into()),
        },
        GateSpec {
            id: "reviewer-diff".into(),
            role: "reviewer".into(),
            on: GateOn::RoleComplete,
            gate: GateKind::Review,
            on_fail: GateOnFail::Block,
            blocks_role: Some("verifier".into()),
            max_retries: 1,
            artifact_kind: Some("diff_review".into()),
        },
        GateSpec {
            id: "verifier-suite".into(),
            role: "verifier".into(),
            on: GateOn::RoleComplete,
            gate: GateKind::Verify,
            on_fail: GateOnFail::Retry,
            blocks_role: Some("release_lead".into()),
            max_retries: 2,
            artifact_kind: Some("verify_report".into()),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn scout_handoff_passes_findings_to_implementer() {
        let mut board = LaneGateBoard::new("lane-test");
        let gates = stopship_gate_pipeline();
        board.install_gates(&gates);

        board
            .record_handoff(HandoffArtifact {
                id: "art-1".into(),
                lane_id: "lane-test".into(),
                from_role: "scout".into(),
                to_role: "implementer".into(),
                kind: "findings".into(),
                payload: r#"{"issue":4090,"files":["app.rs"]}"#.into(),
                created_at: "2026-07-09T00:00:00Z".into(),
            })
            .unwrap();

        let art = board
            .latest_handoff("scout", "implementer", "findings")
            .expect("findings artifact");
        assert_eq!(art.id, "art-1");
        assert!(art.payload.contains("4090"));

        // Approve scout gate so implementer unblocks.
        let state = board.evaluate(&gates[0], GateOutcome::Pass).unwrap();
        assert_eq!(state, GateState::Passed);
        assert!(board.role_is_blocked(&gates, "implementer").is_none());
    }

    #[test]
    fn reviewer_block_prevents_verifier() {
        let mut board = LaneGateBoard::new("lane-rev");
        let gates = stopship_gate_pipeline();
        board.install_gates(&gates);

        let state = board
            .evaluate(
                &gates[1],
                GateOutcome::Fail {
                    reason: "regression in Ctrl+C path".into(),
                },
            )
            .unwrap();
        assert!(matches!(state, GateState::Blocked { .. }));
        let blocked = board
            .role_is_blocked(&gates, "verifier")
            .expect("verifier blocked");
        assert_eq!(blocked.blocked_reason(), Some("regression in Ctrl+C path"));
    }

    #[test]
    fn verifier_retry_then_escalate() {
        let mut board = LaneGateBoard::new("lane-ver");
        let gates = stopship_gate_pipeline();
        board.install_gates(&gates);
        let verify = &gates[2];
        assert_eq!(verify.max_retries, 2);

        let s1 = board
            .evaluate(
                verify,
                GateOutcome::Fail {
                    reason: "cargo test failed".into(),
                },
            )
            .unwrap();
        assert!(matches!(s1, GateState::Retrying { attempt: 1, .. }));

        let s2 = board
            .evaluate(
                verify,
                GateOutcome::Fail {
                    reason: "cargo test failed again".into(),
                },
            )
            .unwrap();
        assert!(matches!(s2, GateState::Retrying { attempt: 2, .. }));

        let s3 = board
            .evaluate(
                verify,
                GateOutcome::Fail {
                    reason: "still red".into(),
                },
            )
            .unwrap();
        assert!(matches!(s3, GateState::Escalated { .. }));
        assert!(board.role_is_blocked(&gates, "release_lead").is_some());
    }

    #[test]
    fn human_approve_clears_block() {
        let mut board = LaneGateBoard::new("lane-hum");
        let gates = stopship_gate_pipeline();
        board.install_gates(&gates);
        board
            .evaluate(
                &gates[1],
                GateOutcome::Fail {
                    reason: "needs human".into(),
                },
            )
            .unwrap();
        assert!(board.role_is_blocked(&gates, "verifier").is_some());

        let state = board
            .evaluate(
                &gates[1],
                GateOutcome::HumanApprove {
                    note: "override: known flaky".into(),
                },
            )
            .unwrap();
        assert_eq!(state, GateState::Passed);
        assert!(board.role_is_blocked(&gates, "verifier").is_none());
    }

    #[test]
    fn board_persists_for_lane_status() {
        let dir = tempdir().unwrap();
        let mut board = LaneGateBoard::new("lane-persist");
        board.install_gates(&stopship_gate_pipeline());
        board
            .evaluate(
                &stopship_gate_pipeline()[0],
                GateOutcome::Fail {
                    reason: "scout incomplete".into(),
                },
            )
            .unwrap();
        board.save_to_dir(dir.path()).unwrap();
        let loaded = LaneGateBoard::load_from_dir(dir.path()).unwrap();
        let summary = loaded.status_summary();
        assert!(
            summary
                .iter()
                .any(|l| l.gate_id == "scout-findings" && l.state == "blocked")
        );
    }
}
