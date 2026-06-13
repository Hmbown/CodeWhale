//! Agent Fleet control-plane protocol types.
//!
//! These types define the durable, serializable contract between the fleet
//! manager, workers, CLI/TUI surfaces, and the Runtime API. They are
//! intentionally additive: existing runtime-event consumers ignore unknown
//! fields and are unaffected by fleet extensions.
//!
//! See:
//! - <https://github.com/Hmbown/CodeWhale/issues/3154> (Agent Fleet control plane)
//! - <https://github.com/Hmbown/CodeWhale/issues/3096> (Runtime API sub-agent direction)

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

pub const FLEET_PROTOCOL_VERSION: &str = "0.1.0";

/// Globally unique identifier for a fleet run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct FleetRunId(pub String);

impl From<String> for FleetRunId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for FleetRunId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// Top-level fleet run handle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetRun {
    pub id: FleetRunId,
    pub name: String,
    pub status: FleetRunStatus,
    #[serde(default)]
    pub task_specs: Vec<FleetTaskSpec>,
    #[serde(default)]
    pub worker_specs: Vec<FleetWorkerSpec>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

/// Lifecycle status for an entire fleet run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FleetRunStatus {
    Pending,
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

/// Specification of a single unit of work within a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetTaskSpec {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub instructions: String,
    #[serde(default)]
    pub expected_artifacts: Vec<FleetArtifactKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scorer: Option<FleetScorerSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_policy: Option<FleetRetryPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alert_policy: Option<FleetAlertPolicy>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

/// Reference to an artifact produced or consumed by a task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FleetArtifactRef {
    pub kind: FleetArtifactKind,
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

/// Kind of artifact a task may produce or consume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetArtifactKind {
    Log,
    Patch,
    TestResult,
    Report,
    Checkpoint,
    Receipt,
    Other(String),
}

impl FleetArtifactKind {
    fn as_wire_str(&self) -> &str {
        match self {
            Self::Log => "log",
            Self::Patch => "patch",
            Self::TestResult => "test_result",
            Self::Report => "report",
            Self::Checkpoint => "checkpoint",
            Self::Receipt => "receipt",
            Self::Other(kind) => kind.as_str(),
        }
    }

    fn from_wire_str(value: &str) -> Self {
        match value {
            "log" => Self::Log,
            "patch" => Self::Patch,
            "test_result" => Self::TestResult,
            "report" => Self::Report,
            "checkpoint" => Self::Checkpoint,
            "receipt" => Self::Receipt,
            other => Self::Other(other.to_string()),
        }
    }
}

impl Serialize for FleetArtifactKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_wire_str())
    }
}

impl<'de> Deserialize<'de> for FleetArtifactKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from_wire_str(&value))
    }
}

/// Scoring rule used to verify a task result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FleetScorerSpec {
    ExitCode,
    FileExists { path: PathBuf },
    RegexMatch { path: PathBuf, pattern: String },
    JsonPath { path: PathBuf, expression: String },
    Manual,
}

/// Worker specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetWorkerSpec {
    pub id: String,
    pub name: String,
    pub host: FleetHostSpec,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_tasks: Option<usize>,
}

/// Host on which a worker runs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FleetHostSpec {
    Local,
    Ssh {
        host: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        user: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        identity: Option<PathBuf>,
    },
    Docker {
        image: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

/// Runtime status of a worker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FleetWorkerStatus {
    Unknown,
    Online,
    Busy,
    Offline,
    Unhealthy,
    Draining,
    Retired,
}

/// Durable inbox entry: a task waiting to be leased to a worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetInboxEntry {
    pub run_id: FleetRunId,
    pub task_id: String,
    pub priority: i32,
    pub enqueued_at: String,
    #[serde(default)]
    pub lease_deadline: Option<String>,
    #[serde(default)]
    pub attempts: u32,
}

/// Worker event envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetWorkerEvent {
    pub seq: u64,
    pub run_id: FleetRunId,
    pub worker_id: String,
    pub task_id: String,
    pub timestamp: String,
    #[serde(flatten)]
    pub payload: FleetWorkerEventPayload,
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

/// Union of all worker event payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FleetWorkerEventPayload {
    Queued,
    Leased {
        #[serde(skip_serializing_if = "Option::is_none")]
        lease_expires_at: Option<String>,
    },
    Starting,
    Running,
    ModelWait {
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    RunningTool {
        tool: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
    },
    Heartbeat {
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        cpu_percent: Option<f32>,
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        memory_mb: Option<u64>,
    },
    Artifact(FleetArtifactRef),
    Completed {
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    Failed {
        reason: String,
        #[serde(default)]
        recoverable: bool,
    },
    Cancelled {
        #[serde(skip_serializing_if = "Option::is_none")]
        cancelled_by: Option<String>,
    },
    Interrupted {
        #[serde(skip_serializing_if = "Option::is_none")]
        signal: Option<String>,
    },
    Stale {
        #[serde(skip_serializing_if = "Option::is_none")]
        last_heartbeat_at: Option<String>,
    },
    Restarted {
        #[serde(default)]
        restart_count: u32,
    },
    Escalated {
        channel: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        alert_id: Option<String>,
    },
}

/// Retry policy for a task or worker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FleetRetryPolicy {
    #[serde(default = "default_retry_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_retry_initial_backoff_seconds")]
    pub initial_backoff_seconds: u64,
    #[serde(default = "default_retry_max_backoff_seconds")]
    pub max_backoff_seconds: u64,
    #[serde(default = "default_retry_backoff_multiplier")]
    pub backoff_multiplier: u32,
}

impl Default for FleetRetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff_seconds: 5,
            max_backoff_seconds: 300,
            backoff_multiplier: 2,
        }
    }
}

fn default_retry_max_attempts() -> u32 {
    FleetRetryPolicy::default().max_attempts
}

fn default_retry_initial_backoff_seconds() -> u64 {
    FleetRetryPolicy::default().initial_backoff_seconds
}

fn default_retry_max_backoff_seconds() -> u64 {
    FleetRetryPolicy::default().max_backoff_seconds
}

fn default_retry_backoff_multiplier() -> u32 {
    FleetRetryPolicy::default().backoff_multiplier
}

/// Alert/escalation policy attached to a task or run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FleetAlertPolicy {
    #[serde(default)]
    pub channels: Vec<FleetAlertChannel>,
    #[serde(default)]
    pub after_attempts: Option<u32>,
    #[serde(default)]
    pub after_minutes_stale: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FleetAlertChannel {
    Slack {
        webhook_url: String,
    },
    Webhook {
        url: String,
        secret: Option<String>,
    },
    PagerDuty {
        routing_key: String,
        severity: String,
    },
}

/// Receipt produced when a task completes verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetReceipt {
    pub run_id: FleetRunId,
    pub task_id: String,
    pub worker_id: String,
    pub completed_at: String,
    pub result: FleetTaskResult,
    #[serde(default)]
    pub artifacts: Vec<FleetArtifactRef>,
    #[serde(default)]
    pub score: Option<FleetScore>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FleetTaskResult {
    Pass,
    Fail,
    Skip,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FleetScore {
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fleet_run_round_trip() {
        let run = FleetRun {
            id: FleetRunId::from("run-001"),
            name: "dogfood smoke".to_string(),
            status: FleetRunStatus::Running,
            task_specs: vec![FleetTaskSpec {
                id: "task-1".to_string(),
                name: "lint".to_string(),
                description: None,
                instructions: "run cargo clippy".to_string(),
                expected_artifacts: vec![FleetArtifactKind::Log],
                scorer: Some(FleetScorerSpec::ExitCode),
                retry_policy: Some(FleetRetryPolicy::default()),
                alert_policy: None,
                timeout_seconds: Some(300),
                metadata: BTreeMap::new(),
            }],
            worker_specs: vec![],
            labels: BTreeMap::new(),
            created_at: "2026-06-12T17:00:00Z".to_string(),
            updated_at: None,
            completed_at: None,
        };
        let json = serde_json::to_string(&run).unwrap();
        let back: FleetRun = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, run.id);
        assert_eq!(back.status, FleetRunStatus::Running);
        assert_eq!(back.task_specs.len(), 1);
    }

    #[test]
    fn worker_event_lifecycle_round_trip() {
        let events = vec![
            FleetWorkerEvent {
                seq: 1,
                run_id: FleetRunId::from("run-002"),
                worker_id: "worker-a".to_string(),
                task_id: "task-1".to_string(),
                timestamp: "2026-06-12T17:01:00Z".to_string(),
                payload: FleetWorkerEventPayload::Queued,
                extra: BTreeMap::new(),
            },
            FleetWorkerEvent {
                seq: 2,
                run_id: FleetRunId::from("run-002"),
                worker_id: "worker-a".to_string(),
                task_id: "task-1".to_string(),
                timestamp: "2026-06-12T17:01:05Z".to_string(),
                payload: FleetWorkerEventPayload::RunningTool {
                    tool: "bash".to_string(),
                    call_id: Some("call-1".to_string()),
                },
                extra: BTreeMap::new(),
            },
            FleetWorkerEvent {
                seq: 3,
                run_id: FleetRunId::from("run-002"),
                worker_id: "worker-a".to_string(),
                task_id: "task-1".to_string(),
                timestamp: "2026-06-12T17:02:00Z".to_string(),
                payload: FleetWorkerEventPayload::Completed {
                    exit_code: Some(0),
                    summary: Some("ok".to_string()),
                },
                extra: BTreeMap::new(),
            },
        ];
        let json = serde_json::to_string(&events).unwrap();
        let back: Vec<FleetWorkerEvent> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 3);
        assert!(matches!(back[0].payload, FleetWorkerEventPayload::Queued));
        assert!(matches!(
            back[2].payload,
            FleetWorkerEventPayload::Completed { .. }
        ));
    }

    #[test]
    fn alert_policy_round_trip() {
        let policy = FleetAlertPolicy {
            channels: vec![FleetAlertChannel::Slack {
                webhook_url: "https://hooks.slack.com/test".to_string(),
            }],
            after_attempts: Some(2),
            after_minutes_stale: Some(10),
        };
        let json = serde_json::to_string(&policy).unwrap();
        assert!(json.contains("\"kind\":\"slack\""));
        let back: FleetAlertPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back.after_attempts, Some(2));
    }

    #[test]
    fn artifact_other_kind_round_trip() {
        let artifact = FleetArtifactRef {
            kind: FleetArtifactKind::Other("coverage.xml".to_string()),
            path: PathBuf::from("/tmp/coverage.xml"),
            checksum: Some("sha256:abc".to_string()),
            mime_type: Some("application/xml".to_string()),
            size_bytes: Some(1024),
        };
        let json = serde_json::to_string(&artifact).unwrap();
        let back: FleetArtifactRef = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, artifact.kind);
        assert_eq!(back.size_bytes, Some(1024));
    }

    #[test]
    fn artifact_kind_uses_flat_string_json() {
        let known = serde_json::to_string(&FleetArtifactKind::TestResult).unwrap();
        assert_eq!(known, "\"test_result\"");

        let custom =
            serde_json::to_string(&FleetArtifactKind::Other("coverage.xml".to_string())).unwrap();
        assert_eq!(custom, "\"coverage.xml\"");

        let parsed: FleetArtifactKind = serde_json::from_str("\"coverage.xml\"").unwrap();
        assert_eq!(parsed, FleetArtifactKind::Other("coverage.xml".to_string()));
    }

    #[test]
    fn retry_policy_missing_fields_use_nonzero_defaults() {
        let policy: FleetRetryPolicy = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(policy, FleetRetryPolicy::default());

        let policy: FleetRetryPolicy =
            serde_json::from_value(serde_json::json!({"max_attempts": 5})).unwrap();
        assert_eq!(policy.max_attempts, 5);
        assert_eq!(
            policy.initial_backoff_seconds,
            FleetRetryPolicy::default().initial_backoff_seconds
        );
        assert_eq!(
            policy.max_backoff_seconds,
            FleetRetryPolicy::default().max_backoff_seconds
        );
        assert_eq!(
            policy.backoff_multiplier,
            FleetRetryPolicy::default().backoff_multiplier
        );
    }

    #[test]
    fn sparse_worker_events_omit_absent_optional_fields() {
        let heartbeat = FleetWorkerEventPayload::Heartbeat {
            cpu_percent: None,
            memory_mb: None,
        };
        let heartbeat_json = serde_json::to_value(&heartbeat).unwrap();
        assert_eq!(heartbeat_json, serde_json::json!({"state": "heartbeat"}));

        let completed = FleetWorkerEventPayload::Completed {
            exit_code: None,
            summary: None,
        };
        let completed_json = serde_json::to_value(&completed).unwrap();
        assert_eq!(completed_json, serde_json::json!({"state": "completed"}));
    }

    #[test]
    fn receipt_round_trip() {
        let receipt = FleetReceipt {
            run_id: FleetRunId::from("run-003"),
            task_id: "task-1".to_string(),
            worker_id: "worker-b".to_string(),
            completed_at: "2026-06-12T17:03:00Z".to_string(),
            result: FleetTaskResult::Pass,
            artifacts: vec![],
            score: Some(FleetScore {
                value: 0.95,
                max: Some(1.0),
                notes: None,
            }),
        };
        let json = serde_json::to_string(&receipt).unwrap();
        let back: FleetReceipt = serde_json::from_str(&json).unwrap();
        assert_eq!(back.result, FleetTaskResult::Pass);
        assert_eq!(back.score.as_ref().unwrap().value, 0.95);
    }
}
