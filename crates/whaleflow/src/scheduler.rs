//! Phase scheduler with topological ordering, concurrency control,
//! result plumbing, and failure handling.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use crate::config::{FailurePolicy, Task, WorkflowConfig};
use crate::result::{
    PhaseResult, TaskCounts, TaskResult, TaskStatus, WorkflowResult, WorkflowStatus,
};
use crate::spawner::{AgentResult, AgentSpawner};

/// Executes a workflow config against the provided agent spawner.
pub struct Scheduler {
    config: WorkflowConfig,
    spawner: Arc<dyn AgentSpawner>,
    /// Shared concurrency semaphore across all phases.
    concurrency: Arc<Semaphore>,
    /// Accumulated results keyed by task id.
    results: HashMap<String, AgentResult>,
}

impl Scheduler {
    pub fn new(config: WorkflowConfig, spawner: Arc<dyn AgentSpawner>) -> Self {
        let max = config.max_concurrent.max(1);
        Self {
            config,
            spawner,
            concurrency: Arc::new(Semaphore::new(max)),
            results: HashMap::new(),
        }
    }

    /// Run the full workflow. Returns a structured result.
    pub async fn run(&mut self) -> WorkflowResult {
        info!(
            goal = %self.config.goal,
            phases = self.config.phases.len(),
            max_concurrent = self.config.max_concurrent,
            "starting workflow"
        );

        // Validate config before execution.
        if let Err(errors) = self.config.validate() {
            return self.fail_fast("config validation failed", &errors);
        }

        // Topological sort phases.
        let ordered = match self.topological_sort() {
            Ok(phases) => phases,
            Err(cycle) => {
                return self.fail_fast(
                    "cycle detected in phase dependencies",
                    &[format!("cycle: {}", cycle.join(" -> "))],
                );
            }
        };

        let mut phase_results: Vec<PhaseResult> = Vec::new();
        let mut workflow_status = WorkflowStatus::Completed;

        // Pre-extract per-phase failure policy to avoid borrow conflicts.
        let abort_policies: HashMap<String, bool> = self
            .config
            .phases
            .iter()
            .map(|p| (p.name.clone(), p.on_failure == FailurePolicy::Abort))
            .collect();

        for phase_name in &ordered {
            debug!(phase = %phase_name, "executing phase");

            let result = self.run_phase(phase_name).await;
            let phase_status = phase_status(&result.0.tasks);

            if phase_status == TaskStatus::Failed && abort_policies.get(phase_name).copied().unwrap_or(false) {
                workflow_status = WorkflowStatus::Aborted;
                phase_results.push(result.0);
                break;
            }

            phase_results.push(result.0);
        }

        let counts = compute_counts(&phase_results);
        let mut result = WorkflowResult {
            goal: self.config.goal.clone(),
            status: if counts.failed > 0 && workflow_status != WorkflowStatus::Aborted {
                WorkflowStatus::Partial
            } else {
                workflow_status
            },
            phases: phase_results,
            counts,
            summary: String::new(),
        };
        result.build_summary();
        result
    }

    /// Execute all tasks in a single phase, respecting the concurrency limit.
    async fn run_phase(&mut self, phase_name: &str) -> (PhaseResult, TaskStatus) {
        let phase = self
            .config
            .phases
            .iter()
            .find(|p| &p.name == phase_name)
            .unwrap()
            .clone();
        let mut task_results: Vec<TaskResult> = Vec::new();

        if phase.parallel && phase.tasks.len() > 1 {
            // Fan-out: spawn all tasks, limited by semaphore.
            let mut handles = Vec::new();
            for task in &phase.tasks {
                let task_id = task.id.clone();
                let task = task.clone();
                let spawner = Arc::clone(&self.spawner);
                let sem = Arc::clone(&self.concurrency);
                let prompt = self.build_prompt(&task);

                let task_id_for_closure = task_id.clone();
                let cwd = task.isolation.cwd_path(&task_id);
                let timeout_secs = task.timeout_secs;
                let max_steps = task.max_steps;
                let handle = tokio::spawn(async move {
                    let _permit = sem.acquire().await;
                    spawner
                        .spawn(task_id_for_closure, prompt, task.agent_type.clone(), cwd, timeout_secs, max_steps)
                        .await
                });
                handles.push((task_id, handle));
            }

            for (task_id, handle) in handles {
                match handle.await {
                    Ok(Ok(agent_result)) => {
                        self.results.insert(task_id.clone(), agent_result.clone());
                        task_results.push(TaskResult {
                            id: task_id,
                            status: TaskStatus::Completed,
                            summary: Some(truncate(&agent_result.summary, 500)),
                            files_touched: agent_result.files_touched,
                            error: None,
                        });
                    }
                    Ok(Err(spawn_err)) => {
                        warn!(task = %task_id, error = %spawn_err, "task failed");
                        task_results.push(TaskResult {
                            id: task_id.clone(),
                            status: TaskStatus::Failed,
                            summary: None,
                            files_touched: vec![],
                            error: Some(spawn_err.to_string()),
                        });
                        if phase.on_failure == FailurePolicy::Abort {
                            break;
                        }
                    }
                    Err(join_err) => {
                        warn!(task = %task_id, error = %join_err, "task panicked");
                        task_results.push(TaskResult {
                            id: task_id.clone(),
                            status: TaskStatus::Failed,
                            summary: None,
                            files_touched: vec![],
                            error: Some(format!("join error: {}", join_err)),
                        });
                        if phase.on_failure == FailurePolicy::Abort {
                            break;
                        }
                    }
                }
            }
        } else {
            // Sequential execution.
            for task in &phase.tasks {
                let prompt = self.build_prompt(task);
                let _permit = self.concurrency.acquire().await;
                let cwd = task.isolation.cwd_path(&task.id);

                match self
                    .spawner
                    .spawn(task.id.clone(), prompt, task.agent_type.clone(), cwd, task.timeout_secs, task.max_steps)
                    .await
                {
                    Ok(agent_result) => {
                        self.results.insert(task.id.clone(), agent_result.clone());
                        task_results.push(TaskResult {
                            id: task.id.clone(),
                            status: TaskStatus::Completed,
                            summary: Some(truncate(&agent_result.summary, 500)),
                            files_touched: agent_result.files_touched,
                            error: None,
                        });
                    }
                    Err(spawn_err) => {
                        warn!(task = %task.id, error = %spawn_err, "task failed");
                        task_results.push(TaskResult {
                            id: task.id.clone(),
                            status: TaskStatus::Failed,
                            summary: None,
                            files_touched: vec![],
                            error: Some(spawn_err.to_string()),
                        });

                        if phase.on_failure == FailurePolicy::Abort {
                            // Mark remaining tasks as skipped.
                            // (We already collected results for completed tasks.)
                            break;
                        }
                    }
                }
            }
        }

        // Mark unexecuted tasks as skipped.
        let executed: HashSet<&str> = task_results.iter().map(|t| t.id.as_str()).collect();
        let mut skipped: Vec<TaskResult> = Vec::new();
        for task in &phase.tasks {
            if !executed.contains(task.id.as_str()) {
                skipped.push(TaskResult {
                    id: task.id.clone(),
                    status: TaskStatus::Skipped,
                    summary: None,
                    files_touched: vec![],
                    error: None,
                });
            }
        }
        drop(executed);
        task_results.extend(skipped);

        let pstatus = phase_status(&task_results);
        (
            PhaseResult {
                name: phase.name.clone(),
                status: pstatus,
                tasks: task_results,
            },
            pstatus,
        )
    }

    /// Build the agent prompt, injecting results from upstream dependencies.
    fn build_prompt(&self, task: &Task) -> String {
        if task.depends_on_results.is_empty() {
            return task.prompt.clone();
        }

        let mut ctx = String::from("## Context from upstream tasks\n\n");
        for dep_id in &task.depends_on_results {
            if let Some(result) = self.results.get(dep_id) {
                ctx.push_str(&format!(
                    "### {} ({})\n{}\n\n",
                    dep_id,
                    if result.success { "success" } else { "failed" },
                    truncate(&result.summary, 1000),
                ));
            } else {
                ctx.push_str(&format!("### {} (not available)\n\n", dep_id));
            }
        }
        ctx.push_str("---\n\n");
        ctx.push_str(&task.prompt);
        ctx
    }

    /// Topological sort of phases by `depends_on`.
    fn topological_sort(&self) -> Result<Vec<String>, Vec<String>> {
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();

        for phase in &self.config.phases {
            in_degree.entry(&phase.name).or_insert(0);
            adjacency.entry(&phase.name).or_default();
            for dep in &phase.depends_on {
                adjacency.entry(dep.as_str()).or_default().push(&phase.name);
                *in_degree.entry(&phase.name).or_insert(0) += 1;
            }
        }

        let mut queue: Vec<&str> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(&name, _)| name)
            .collect();

        let mut sorted: Vec<String> = Vec::new();
        while let Some(node) = queue.pop() {
            sorted.push(node.to_string());
            if let Some(neighbors) = adjacency.get(node) {
                for &neighbor in neighbors {
                    let deg = in_degree.get_mut(neighbor).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(neighbor);
                    }
                }
            }
        }

        if sorted.len() != self.config.phases.len() {
            // Cycle: find it for the error message.
            Err(self.find_cycle())
        } else {
            Ok(sorted)
        }
    }

    fn find_cycle(&self) -> Vec<String> {
        // Reuse config's cycle detection.
        // (Simplified: just return phase names for now; the config validator
        //  already catches cycles before we get here.)
        self.config.phases.iter().map(|p| p.name.clone()).collect()
    }

    fn fail_fast(&self, reason: &str, details: &[String]) -> WorkflowResult {
        let mut result = WorkflowResult {
            goal: self.config.goal.clone(),
            status: WorkflowStatus::Aborted,
            phases: vec![],
            counts: TaskCounts::default(),
            summary: String::new(),
        };
        result.summary = format!(
            "Workflow aborted: {}\n{}",
            reason,
            details
                .iter()
                .map(|d| format!("  - {}", d))
                .collect::<Vec<_>>()
                .join("\n")
        );
        result
    }
}

/// Determine the aggregate status of a phase from its task results.
fn phase_status(tasks: &[TaskResult]) -> TaskStatus {
    if tasks.iter().all(|t| t.status == TaskStatus::Completed) {
        TaskStatus::Completed
    } else if tasks.iter().any(|t| t.status == TaskStatus::Failed) {
        TaskStatus::Failed
    } else if tasks.iter().all(|t| t.status == TaskStatus::Skipped) {
        TaskStatus::Skipped
    } else {
        TaskStatus::Pending
    }
}

/// Compute aggregate task counts across all phases.
fn compute_counts(phases: &[PhaseResult]) -> TaskCounts {
    let mut counts = TaskCounts::default();
    for phase in phases {
        for task in &phase.tasks {
            counts.total += 1;
            match task.status {
                TaskStatus::Completed => counts.completed += 1,
                TaskStatus::Failed => counts.failed += 1,
                TaskStatus::Skipped => counts.skipped += 1,
                _ => counts.pending += 1,
            }
        }
    }
    counts
}

/// Truncate a string to `max_len` characters, adding "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut truncated: String = s.chars().take(max_len).collect();
        truncated.push_str("...");
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Phase;
    use crate::spawner::{AgentResult, SpawnError};
    use crate::{IsolationMode, TaskMode};

    struct MockSpawner {
        responses: HashMap<String, Result<AgentResult, SpawnError>>,
    }

    #[async_trait::async_trait]
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
                Some(result) => match result {
                    Ok(r) => Ok(AgentResult {
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
                    Err(_) => Err(SpawnError::SpawnFailed("mock error".into())),
                },
                None => Ok(AgentResult {
                    task_id: task_id.clone(),
                    success: true,
                    summary: "mock result".into(),
                    files_touched: vec![],
                    raw_output: None,
                    tokens_used: None,
                    cost_usd: None,
                    elapsed_ms: None,
                    last_checkpoint: None,
                }),
            }
        }
    }

    fn mock_result(task_id: &str) -> AgentResult {
        AgentResult {
            task_id: task_id.into(),
            success: true,
            summary: format!("result from {}", task_id),
            files_touched: vec!["src/lib.rs".into()],
            raw_output: None,
            tokens_used: Some(1000),
            cost_usd: Some(0.01),
            elapsed_ms: Some(500),
            last_checkpoint: Some("mock checkpoint".into()),
        }
    }

    fn test_task(id: &str, prompt: &str) -> Task {
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
    async fn single_phase_parallel() {
        let config = WorkflowConfig {
            goal: "test".into(),
            max_concurrent: 4,
            phases: vec![Phase {
                name: "discovery".into(),
                depends_on: vec![],
                parallel: true,
                on_failure: FailurePolicy::SkipContinue,
                tasks: vec![test_task("a", "scan a"), test_task("b", "scan b")],
            }],
        };

        let spawner = Arc::new(MockSpawner {
            responses: HashMap::new(),
        });
        let mut scheduler = Scheduler::new(config, spawner);
        let result = scheduler.run().await;

        assert_eq!(result.status, WorkflowStatus::Completed);
        assert_eq!(result.counts.total, 2);
        assert_eq!(result.counts.completed, 2);
    }

    #[tokio::test]
    async fn phase_dependency_ordering() {
        let config = WorkflowConfig {
            goal: "test".into(),
            max_concurrent: 4,
            phases: vec![
                Phase {
                    name: "second".into(),
                    depends_on: vec!["first".into()],
                    parallel: false,
                    on_failure: FailurePolicy::SkipContinue,
                    tasks: vec![Task {
                        id: "b".into(),
                        prompt: "second task".into(),
                        agent_type: None,
                        depends_on_results: vec!["a".into()],
                        max_steps: None,
                        timeout_secs: None,
                        mode: TaskMode::ReadOnly,
                        file_scope: vec![],
                        isolation: IsolationMode::Shared,
                    }],
                },
                Phase {
                    name: "first".into(),
                    depends_on: vec![],
                    parallel: false,
                    on_failure: FailurePolicy::SkipContinue,
                    tasks: vec![Task {
                        id: "a".into(),
                        prompt: "first task".into(),
                        agent_type: None,
                        depends_on_results: vec![],
                        max_steps: None,
                        timeout_secs: None,
                        mode: TaskMode::ReadOnly,
                        file_scope: vec![],
                        isolation: IsolationMode::Shared,
                    }],
                },
            ],
        };

        let spawner = Arc::new(MockSpawner {
            responses: HashMap::from([
                ("a".into(), Ok(mock_result("a"))),
                ("b".into(), Ok(mock_result("b"))),
            ]),
        });
        let mut scheduler = Scheduler::new(config, spawner);
        let result = scheduler.run().await;

        assert_eq!(result.status, WorkflowStatus::Completed);
        assert_eq!(result.counts.completed, 2);
        // Phase "first" should appear before "second" in results.
        assert_eq!(result.phases[0].name, "first");
        assert_eq!(result.phases[1].name, "second");
    }

    #[tokio::test]
    async fn skip_continue_on_failure() {
        let config = WorkflowConfig {
            goal: "test".into(),
            max_concurrent: 4,
            phases: vec![Phase {
                name: "tasks".into(),
                depends_on: vec![],
                parallel: true,
                on_failure: FailurePolicy::SkipContinue,
                tasks: vec![
                    test_task("ok", "ok"),
                    test_task("fail", "fail"),
                ],
            }],
        };

        let spawner = Arc::new(MockSpawner {
            responses: HashMap::from([
                ("ok".into(), Ok(mock_result("ok"))),
                (
                    "fail".into(),
                    Err(SpawnError::SpawnFailed("boom".into())),
                ),
            ]),
        });
        let mut scheduler = Scheduler::new(config, spawner);
        let result = scheduler.run().await;

        assert_eq!(result.status, WorkflowStatus::Partial);
        assert_eq!(result.counts.completed, 1);
        assert_eq!(result.counts.failed, 1);
    }
}
