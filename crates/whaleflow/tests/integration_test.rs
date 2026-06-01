//! Integration tests for WhaleFlow — full pipeline from config to result.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use codewhale_whaleflow::{
    AgentResult, AgentSpawner, FailurePolicy, IsolationMode, Phase, Scheduler, SpawnError, Task,
    TaskMode, TaskStatus, WorkflowConfig, WorkflowStatus,
};

/// A controllable mock spawner for integration testing.
struct MockSpawner {
    responses: HashMap<String, Result<AgentResult, SpawnError>>,
}

impl MockSpawner {
    fn new() -> Self {
        Self {
            responses: HashMap::new(),
        }
    }

    fn set(&mut self, id: &str, result: Result<AgentResult, SpawnError>) {
        self.responses.insert(id.to_string(), result);
    }
}

#[async_trait]
impl AgentSpawner for MockSpawner {
    async fn spawn(
        &self,
        task_id: String,
        _prompt: String,
        _agent_type: Option<String>,
        _cwd: Option<std::path::PathBuf>,
        _timeout_secs: Option<u64>,
        _max_steps: Option<u32>,
    ) -> Result<AgentResult, SpawnError> {
        match self.responses.get(&task_id) {
            Some(Ok(r)) => Ok(AgentResult {
                task_id: r.task_id.clone(),
                success: r.success,
                summary: r.summary.clone(),
                files_touched: r.files_touched.clone(),
                raw_output: r.raw_output.clone(),
                tokens_used: r.tokens_used,
                cost_usd: r.cost_usd,
                elapsed_ms: r.elapsed_ms,
                last_checkpoint: r.last_checkpoint.clone(),
            }),
            Some(Err(_)) => Err(SpawnError::SpawnFailed("mock error".into())),
            None => Ok(AgentResult {
                task_id: task_id.clone(),
                success: true,
                summary: format!("default result for {}", task_id),
                files_touched: vec![],
                raw_output: None,
                tokens_used: Some(500),
                cost_usd: Some(0.005),
                elapsed_ms: Some(250),
                last_checkpoint: Some("completed".into()),
            }),
        }
    }
}

fn make_result(task_id: &str, summary: &str, files: &[&str]) -> AgentResult {
    AgentResult {
        task_id: task_id.to_string(),
        success: true,
        summary: summary.to_string(),
        files_touched: files.iter().map(|s| s.to_string()).collect(),
        raw_output: None,
        tokens_used: Some(1000),
        cost_usd: Some(0.01),
        elapsed_ms: Some(500),
        last_checkpoint: Some("done".into()),
    }
}

fn make_task(id: &str, prompt: &str) -> Task {
    Task {
        id: id.into(),
        prompt: prompt.into(),
        agent_type: None,
        depends_on_results: vec![],
        max_steps: None,
        timeout_secs: None,
        mode: TaskMode::ReadOnly,
        file_scope: vec![],
        isolation: IsolationMode::Shared,
    }
}

#[tokio::test]
async fn full_workflow_three_phases() {
    // A realistic 3-phase workflow: discovery → triage → fix
    let config = WorkflowConfig {
        goal: "Security audit".into(),
        max_concurrent: 4,
        phases: vec![
            Phase {
                name: "discovery".into(),
                depends_on: vec![],
                parallel: true,
                on_failure: FailurePolicy::SkipContinue,
                tasks: vec![
                    make_task("scan-auth", "Audit auth module"),
                    make_task("scan-api", "Audit API endpoints"),
                ],
            },
            Phase {
                name: "triage".into(),
                depends_on: vec!["discovery".into()],
                parallel: false,
                on_failure: FailurePolicy::SkipContinue,
                tasks: vec![Task {
                    id: "rank-findings".into(),
                    prompt: "Rank findings".into(),
                    depends_on_results: vec!["scan-auth".into(), "scan-api".into()],
                    ..make_task("rank-findings", "Rank findings")
                }],
            },
            Phase {
                name: "fix".into(),
                depends_on: vec!["triage".into()],
                parallel: true,
                on_failure: FailurePolicy::SkipContinue,
                tasks: vec![
                    Task {
                        id: "fix-1".into(),
                        prompt: "Fix #1".into(),
                        mode: TaskMode::ReadWrite,
                        file_scope: vec!["src/auth/**".into()],
                        ..make_task("fix-1", "Fix #1")
                    },
                    Task {
                        id: "fix-2".into(),
                        prompt: "Fix #2".into(),
                        mode: TaskMode::ReadWrite,
                        file_scope: vec!["src/api/**".into()],
                        ..make_task("fix-2", "Fix #2")
                    },
                ],
            },
        ],
    };

    let mut mock = MockSpawner::new();
    mock.set("scan-auth", Ok(make_result("scan-auth", "Auth looks clean", &[])));
    mock.set(
        "scan-api",
        Ok(make_result(
            "scan-api",
            "Found SQL injection risk",
            &["src/api/handler.rs"],
        )),
    );
    mock.set(
        "rank-findings",
        Ok(make_result(
            "rank-findings",
            "API injection is critical, auth is clean",
            &[],
        )),
    );
    mock.set(
        "fix-1",
        Ok(make_result(
            "fix-1",
            "Removed injection point",
            &["src/auth/login.rs"],
        )),
    );
    mock.set(
        "fix-2",
        Ok(make_result(
            "fix-2",
            "Added input validation",
            &["src/api/handler.rs"],
        )),
    );

    let spawner = Arc::new(mock);
    let mut scheduler = Scheduler::new(config.clone(), spawner);
    let result = scheduler.run().await;

    // Verify overall status.
    assert_eq!(
        result.status,
        WorkflowStatus::Completed
    );
    assert_eq!(result.counts.total, 5);
    assert_eq!(result.counts.completed, 5);
    assert_eq!(result.counts.failed, 0);

    // Verify phase ordering.
    assert_eq!(result.phases.len(), 3);
    assert_eq!(result.phases[0].name, "discovery");
    assert_eq!(result.phases[1].name, "triage");
    assert_eq!(result.phases[2].name, "fix");

    // Verify the triage task received upstream context.
    assert!(result.summary.contains("scan-auth"));
    assert!(result.summary.contains("scan-api"));
}

#[tokio::test]
async fn workflow_with_failure_skip_continue() {
    let config = WorkflowConfig {
        goal: "Partial failure test".into(),
        max_concurrent: 4,
        phases: vec![Phase {
            name: "tasks".into(),
            depends_on: vec![],
            parallel: true,
            on_failure: FailurePolicy::SkipContinue,
            tasks: vec![
                make_task("ok", "Do something"),
                Task {
                    id: "fail".into(),
                    prompt: "Will fail".into(),
                    ..make_task("fail", "Will fail")
                },
            ],
        }],
    };

    let mut mock = MockSpawner::new();
    mock.set("ok", Ok(make_result("ok", "Done", &[])));
    mock.set(
        "fail",
        Err(SpawnError::SpawnFailed("test failure".into())),
    );

    let mut scheduler = Scheduler::new(config, Arc::new(mock));
    let result = scheduler.run().await;

    assert_eq!(result.counts.total, 2);
    assert_eq!(result.counts.completed, 1);
    assert_eq!(result.counts.failed, 1);
    // Skip-continue means the workflow status is Partial, not Aborted.
    assert_eq!(
        result.status,
        WorkflowStatus::Partial
    );
}

#[tokio::test]
async fn workflow_json_roundtrip() {
    // Test that we can deserialize a realistic config and serialize the result.
    let json = r#"{
        "goal": "Quick audit",
        "max_concurrent": 2,
        "phases": [
            {
                "name": "scan",
                "parallel": true,
                "tasks": [
                    {"id": "s1", "prompt": "Scan module A"},
                    {"id": "s2", "prompt": "Scan module B"}
                ]
            }
        ]
    }"#;

    let config: WorkflowConfig =
        serde_json::from_str(json).expect("Failed to parse workflow config");
    assert_eq!(config.goal, "Quick audit");
    assert_eq!(config.phases.len(), 1);
    assert_eq!(config.phases[0].tasks.len(), 2);

    let mock = MockSpawner::new();
    let mut scheduler = Scheduler::new(config, Arc::new(mock));
    let result = scheduler.run().await;

    // Round-trip: serialize and deserialize the result.
    let result_json =
        serde_json::to_string_pretty(&result).expect("Failed to serialize result");
    let _parsed: serde_json::Value =
        serde_json::from_str(&result_json).expect("Failed to parse result JSON");

    assert!(result_json.contains("Quick audit"));
    assert!(result_json.contains("completed"));
}

#[tokio::test]
async fn abort_policy_in_parallel_phase_stops_remaining_tasks() {
    // Phase with two parallel tasks and Abort policy. One task fails —
    // the other should be marked skipped, and the workflow status should
    // be Aborted.
    let config = WorkflowConfig {
        goal: "abort test".into(),
        max_concurrent: 4,
        phases: vec![Phase {
            name: "dangerous".into(),
            depends_on: vec![],
            parallel: true,
            on_failure: FailurePolicy::Abort,
            tasks: vec![
                make_task("ok", "good task"),
                make_task("fail", "bad task"),
                make_task("never-runs", "should be skipped"),
            ],
        }],
    };

    let mut mock = MockSpawner::new();
    mock.set("ok", Ok(make_result("ok", "all good", &[])));
    mock.set(
        "fail",
        Err(SpawnError::SpawnFailed("boom".into())),
    );

    let spawner = Arc::new(mock);
    let mut scheduler = Scheduler::new(config, spawner);
    let result = scheduler.run().await;

    // Workflow should be aborted (phase had Abort policy + failure).
    assert_eq!(result.status, WorkflowStatus::Aborted);
    assert_eq!(result.counts.total, 3);
    assert!(result.counts.completed >= 1); // "ok" completed
    assert_eq!(result.counts.failed, 1); // "fail" failed
    assert!(result.counts.skipped >= 1); // "never-runs" skipped
    // Phase 1 should be the only phase.
    assert_eq!(result.phases.len(), 1);
    let tasks = &result.phases[0].tasks;
    let never_runs = tasks.iter().find(|t| t.id == "never-runs").unwrap();
    assert_eq!(never_runs.status, TaskStatus::Skipped);
}

#[tokio::test]
async fn abort_policy_stops_subsequent_phases() {
    // Two phases: phase 1 with Abort + a failing task → phase 2 must not run.
    let config = WorkflowConfig {
        goal: "phase abort test".into(),
        max_concurrent: 4,
        phases: vec![
            Phase {
                name: "first".into(),
                depends_on: vec![],
                parallel: false,
                on_failure: FailurePolicy::Abort,
                tasks: vec![make_task("f1", "failing task")],
            },
            Phase {
                name: "second".into(),
                depends_on: vec!["first".into()],
                parallel: false,
                on_failure: FailurePolicy::SkipContinue,
                tasks: vec![make_task("s1", "should not run")],
            },
        ],
    };

    let mut mock = MockSpawner::new();
    mock.set(
        "f1",
        Err(SpawnError::SpawnFailed("fail".into())),
    );

    let spawner = Arc::new(mock);
    let mut scheduler = Scheduler::new(config, spawner);
    let result = scheduler.run().await;

    assert_eq!(result.status, WorkflowStatus::Aborted);
    // Phase 2 should not appear in results.
    assert_eq!(result.phases.len(), 1);
    assert_eq!(result.phases[0].name, "first");
}

#[test]
fn timeout_secs_field_is_deserialized_correctly() {
    let json = r#"{
        "goal": "timeout test",
        "phases": [
            {
                "name": "slow",
                "tasks": [
                    {
                        "id": "t1",
                        "prompt": "do work",
                        "timeout_secs": 30,
                        "max_steps": 10
                    }
                ]
            }
        ]
    }"#;

    let config: WorkflowConfig = serde_json::from_str(json).unwrap();
    let task = &config.phases[0].tasks[0];
    assert_eq!(task.timeout_secs, Some(30));
    assert_eq!(task.max_steps, Some(10));
}
