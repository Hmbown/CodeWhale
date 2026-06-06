//! Typed WhaleFlow workflow configuration and validation.
//!
//! This crate deliberately stops at the Rust-owned IR boundary. Runtime tool
//! exposure, worktree application, replay, and model execution are layered on
//! top only after their cancellation and evidence semantics are proven.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowConfig {
    pub goal: String,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: u8,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub phases: Vec<Phase>,
}

impl WorkflowConfig {
    pub fn validate(&self) -> Result<(), WorkflowValidationError> {
        WorkflowPlan::from_config(self).map(|_| ())
    }

    pub fn compile(&self) -> Result<WorkflowPlan, WorkflowValidationError> {
        WorkflowPlan::from_config(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowPlan {
    goal: String,
    max_concurrent: u8,
    phases: Vec<PhasePlan>,
}

impl WorkflowPlan {
    pub fn from_config(config: &WorkflowConfig) -> Result<Self, WorkflowValidationError> {
        validate_non_empty("workflow goal", &config.goal)?;
        if !(1..=20).contains(&config.max_concurrent) {
            return Err(WorkflowValidationError::InvalidMaxConcurrent {
                value: config.max_concurrent,
            });
        }
        if config.phases.is_empty() {
            return Err(WorkflowValidationError::EmptyWorkflow);
        }

        let mut phase_indices = BTreeMap::new();
        let mut all_tasks = BTreeMap::new();
        let mut task_phase = BTreeMap::new();

        for (phase_index, phase) in config.phases.iter().enumerate() {
            validate_non_empty("phase name", &phase.name)?;
            if phase.tasks.is_empty() {
                return Err(WorkflowValidationError::EmptyPhase {
                    phase: phase.name.clone(),
                });
            }
            if phase_indices
                .insert(phase.name.clone(), phase_index)
                .is_some()
            {
                return Err(WorkflowValidationError::DuplicatePhase {
                    phase: phase.name.clone(),
                });
            }

            for task in &phase.tasks {
                validate_non_empty("task id", &task.id)?;
                validate_non_empty("task prompt", &task.prompt)?;
                if all_tasks.insert(task.id.clone(), task).is_some() {
                    return Err(WorkflowValidationError::DuplicateTask {
                        task: task.id.clone(),
                    });
                }
                task_phase.insert(task.id.clone(), phase.name.clone());
            }
        }

        for phase in &config.phases {
            for dependency in &phase.depends_on {
                if dependency == &phase.name || !phase_indices.contains_key(dependency) {
                    return Err(WorkflowValidationError::InvalidPhaseDependency {
                        phase: phase.name.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
            validate_parallel_write_scope(phase)?;
        }

        let ordered_phase_names = ordered_phases(config, &phase_indices)?;
        let phase_order: BTreeMap<_, _> = ordered_phase_names
            .iter()
            .enumerate()
            .map(|(index, phase)| (phase.clone(), index))
            .collect();

        for phase in &config.phases {
            for task in &phase.tasks {
                for dependency in &task.depends_on_results {
                    let Some(dependency_phase) = task_phase.get(dependency) else {
                        return Err(WorkflowValidationError::InvalidTaskResultDependency {
                            task: task.id.clone(),
                            dependency: dependency.clone(),
                        });
                    };
                    if phase_order[dependency_phase] >= phase_order[&phase.name] {
                        return Err(WorkflowValidationError::UnavailableTaskResultDependency {
                            task: task.id.clone(),
                            dependency: dependency.clone(),
                            dependency_phase: dependency_phase.clone(),
                            task_phase: phase.name.clone(),
                        });
                    }
                }
            }
        }

        let phases = ordered_phase_names
            .iter()
            .map(|phase_name| {
                let phase = &config.phases[phase_indices[phase_name]];
                PhasePlan {
                    name: phase.name.clone(),
                    parallel: phase.parallel,
                    on_failure: phase.on_failure,
                    tasks: phase.tasks.clone(),
                }
            })
            .collect();

        Ok(Self {
            goal: config.goal.clone(),
            max_concurrent: config.max_concurrent,
            phases,
        })
    }

    pub fn goal(&self) -> &str {
        &self.goal
    }

    pub fn max_concurrent(&self) -> u8 {
        self.max_concurrent
    }

    pub fn phases(&self) -> &[PhasePlan] {
        &self.phases
    }

    pub fn phase_names(&self) -> impl Iterator<Item = &str> {
        self.phases.iter().map(|phase| phase.name.as_str())
    }
}

pub type WorkflowIr = WorkflowPlan;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhasePlan {
    pub name: String,
    pub parallel: bool,
    pub on_failure: FailurePolicy,
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phase {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub parallel: bool,
    #[serde(default)]
    pub on_failure: FailurePolicy,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

pub type WorkflowPhase = Phase;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    #[default]
    SkipContinue,
    Abort,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub prompt: String,
    #[serde(default)]
    pub agent_type: AgentType,
    #[serde(default)]
    pub mode: TaskMode,
    #[serde(default)]
    pub isolation: IsolationMode,
    #[serde(default)]
    pub file_scope: Vec<String>,
    #[serde(default)]
    pub depends_on_results: Vec<String>,
    #[serde(default)]
    pub max_steps: Option<u32>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

pub type WorkflowTask = Task;
pub type WorkflowRole = AgentType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    #[default]
    General,
    Explore,
    Plan,
    Review,
    Implementer,
    Verifier,
    ToolAgent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskMode {
    #[default]
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IsolationMode {
    #[default]
    Shared,
    Worktree,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchResult {
    pub branch_id: String,
    pub task_id: String,
    pub status: WorkflowRunStatus,
    #[serde(default)]
    pub artifacts: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeafResult {
    pub leaf_id: String,
    pub task_id: String,
    pub status: WorkflowRunStatus,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlNodeResult {
    pub node_id: String,
    pub kind: ControlNodeKind,
    pub status: WorkflowRunStatus,
    #[serde(default)]
    pub selected_children: Vec<String>,
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunStatus {
    #[default]
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlNodeKind {
    BranchSet,
    Leaf,
    Sequence,
    Reduce,
    TeacherReview,
    LoopUntil,
    Cond,
    Expand,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WorkflowValidationError {
    #[error("{field} must not be empty")]
    EmptyField { field: &'static str },
    #[error("workflow must contain at least one phase")]
    EmptyWorkflow,
    #[error("phase `{phase}` must contain at least one task")]
    EmptyPhase { phase: String },
    #[error("max_concurrent must be between 1 and 20, got {value}")]
    InvalidMaxConcurrent { value: u8 },
    #[error("duplicate workflow phase `{phase}`")]
    DuplicatePhase { phase: String },
    #[error("duplicate workflow task `{task}`")]
    DuplicateTask { task: String },
    #[error("phase `{phase}` has invalid dependency `{dependency}`")]
    InvalidPhaseDependency { phase: String, dependency: String },
    #[error("phase dependency cycle includes `{phase}`")]
    PhaseDependencyCycle { phase: String },
    #[error("task `{task}` has invalid result dependency `{dependency}`")]
    InvalidTaskResultDependency { task: String, dependency: String },
    #[error(
        "task `{task}` depends on result `{dependency}` from unavailable phase `{dependency_phase}` while running in `{task_phase}`"
    )]
    UnavailableTaskResultDependency {
        task: String,
        dependency: String,
        dependency_phase: String,
        task_phase: String,
    },
    #[error("parallel read-write task `{task}` must declare a file_scope")]
    MissingParallelWriteScope { task: String },
    #[error("parallel read-write tasks `{left}` and `{right}` have overlapping file scopes")]
    OverlappingParallelWriteScope { left: String, right: String },
}

fn default_max_concurrent() -> u8 {
    4
}

fn validate_non_empty(field: &'static str, value: &str) -> Result<(), WorkflowValidationError> {
    if value.trim().is_empty() {
        return Err(WorkflowValidationError::EmptyField { field });
    }
    Ok(())
}

fn ordered_phases(
    config: &WorkflowConfig,
    phase_indices: &BTreeMap<String, usize>,
) -> Result<Vec<String>, WorkflowValidationError> {
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut ordered = Vec::with_capacity(config.phases.len());

    for phase in &config.phases {
        visit_phase(
            &phase.name,
            config,
            phase_indices,
            &mut visiting,
            &mut visited,
            &mut ordered,
        )?;
    }

    Ok(ordered)
}

fn visit_phase(
    phase_name: &str,
    config: &WorkflowConfig,
    phase_indices: &BTreeMap<String, usize>,
    visiting: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
    ordered: &mut Vec<String>,
) -> Result<(), WorkflowValidationError> {
    if visited.contains(phase_name) {
        return Ok(());
    }
    if !visiting.insert(phase_name.to_string()) {
        return Err(WorkflowValidationError::PhaseDependencyCycle {
            phase: phase_name.to_string(),
        });
    }

    let phase = &config.phases[phase_indices[phase_name]];
    for dependency in &phase.depends_on {
        visit_phase(
            dependency,
            config,
            phase_indices,
            visiting,
            visited,
            ordered,
        )?;
    }

    visiting.remove(phase_name);
    visited.insert(phase_name.to_string());
    ordered.push(phase_name.to_string());
    Ok(())
}

fn validate_parallel_write_scope(phase: &Phase) -> Result<(), WorkflowValidationError> {
    if !phase.parallel {
        return Ok(());
    }

    let write_tasks: Vec<_> = phase
        .tasks
        .iter()
        .filter(|task| task.mode == TaskMode::ReadWrite)
        .collect();

    for task in &write_tasks {
        if task.file_scope.is_empty() {
            return Err(WorkflowValidationError::MissingParallelWriteScope {
                task: task.id.clone(),
            });
        }
    }

    for (left_index, left) in write_tasks.iter().enumerate() {
        for right in write_tasks.iter().skip(left_index + 1) {
            if scopes_overlap(&left.file_scope, &right.file_scope) {
                return Err(WorkflowValidationError::OverlappingParallelWriteScope {
                    left: left.id.clone(),
                    right: right.id.clone(),
                });
            }
        }
    }

    Ok(())
}

pub fn scopes_overlap(left: &[String], right: &[String]) -> bool {
    left.iter().any(|left_scope| {
        right
            .iter()
            .any(|right_scope| scope_overlaps(left_scope, right_scope))
    })
}

fn scope_overlaps(left: &str, right: &str) -> bool {
    let left = normalize_scope(left);
    let right = normalize_scope(right);

    if left == right || left == "." || right == "." {
        return true;
    }

    if left.contains('*') || right.contains('*') {
        return glob_prefix(&left) == glob_prefix(&right);
    }

    let left_path = Path::new(&left);
    let right_path = Path::new(&right);
    left_path.starts_with(right_path) || right_path.starts_with(left_path)
}

fn normalize_scope(scope: &str) -> String {
    let trimmed = scope.trim().trim_start_matches("./").trim_end_matches('/');
    trimmed
        .strip_suffix("/**")
        .or_else(|| trimmed.strip_suffix("/*"))
        .unwrap_or(trimmed)
        .to_string()
}

fn glob_prefix(scope: &str) -> String {
    scope
        .split('*')
        .next()
        .unwrap_or(scope)
        .trim_end_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str) -> Task {
        Task {
            id: id.to_string(),
            prompt: format!("run {id}"),
            agent_type: AgentType::General,
            mode: TaskMode::ReadOnly,
            isolation: IsolationMode::Shared,
            file_scope: Vec::new(),
            depends_on_results: Vec::new(),
            max_steps: None,
            timeout_secs: None,
        }
    }

    fn config(phases: Vec<Phase>) -> WorkflowConfig {
        WorkflowConfig {
            goal: "cache-change".to_string(),
            max_concurrent: 4,
            description: None,
            phases,
        }
    }

    fn phase(name: &str, depends_on: &[&str], tasks: Vec<Task>) -> Phase {
        Phase {
            name: name.to_string(),
            description: None,
            depends_on: depends_on.iter().map(|value| value.to_string()).collect(),
            parallel: false,
            on_failure: FailurePolicy::SkipContinue,
            tasks,
        }
    }

    #[test]
    fn independent_phases_preserve_declaration_order() {
        let workflow = config(vec![
            phase("discover", &[], vec![task("scan")]),
            phase("report", &[], vec![task("summarize")]),
        ]);

        let plan = workflow.compile().expect("workflow should compile");

        assert_eq!(
            plan.phase_names().collect::<Vec<_>>(),
            vec!["discover", "report"]
        );
    }

    #[test]
    fn dependencies_override_declaration_order_deterministically() {
        let workflow = config(vec![
            phase("review", &["implement"], vec![task("review-results")]),
            phase("discover", &[], vec![task("scan")]),
            phase("implement", &["discover"], vec![task("patch")]),
            phase("report", &["review"], vec![task("summarize")]),
        ]);

        let plan = workflow.compile().expect("workflow should compile");

        assert_eq!(
            plan.phase_names().collect::<Vec<_>>(),
            vec!["discover", "implement", "review", "report"]
        );
    }

    #[test]
    fn rejects_empty_workflow() {
        let err = config(Vec::new())
            .validate()
            .expect_err("empty workflow should fail");

        assert_eq!(err, WorkflowValidationError::EmptyWorkflow);
    }

    #[test]
    fn rejects_empty_phase() {
        let err = config(vec![phase("empty", &[], Vec::new())])
            .validate()
            .expect_err("empty phase should fail");

        assert_eq!(
            err,
            WorkflowValidationError::EmptyPhase {
                phase: "empty".to_string()
            }
        );
    }

    #[test]
    fn rejects_invalid_max_concurrent() {
        let mut workflow = config(vec![phase("discover", &[], vec![task("scan")])]);
        workflow.max_concurrent = 0;

        let err = workflow
            .validate()
            .expect_err("zero concurrency should fail");

        assert_eq!(
            err,
            WorkflowValidationError::InvalidMaxConcurrent { value: 0 }
        );
    }

    #[test]
    fn rejects_duplicate_phase_names() {
        let err = config(vec![
            phase("discover", &[], vec![task("scan")]),
            phase("discover", &[], vec![task("scan-again")]),
        ])
        .validate()
        .expect_err("duplicate phase should fail");

        assert!(matches!(
            err,
            WorkflowValidationError::DuplicatePhase { .. }
        ));
    }

    #[test]
    fn rejects_duplicate_task_ids() {
        let err = config(vec![
            phase("discover", &[], vec![task("scan")]),
            phase("report", &[], vec![task("scan")]),
        ])
        .validate()
        .expect_err("duplicate task should fail");

        assert!(matches!(err, WorkflowValidationError::DuplicateTask { .. }));
    }

    #[test]
    fn rejects_unknown_phase_dependency() {
        let err = config(vec![phase("report", &["missing"], vec![task("summarize")])])
            .validate()
            .expect_err("unknown dependency should fail");

        assert!(matches!(
            err,
            WorkflowValidationError::InvalidPhaseDependency { .. }
        ));
    }

    #[test]
    fn rejects_phase_dependency_cycles() {
        let workflow = config(vec![
            phase("a", &["b"], vec![task("a-task")]),
            phase("b", &["a"], vec![task("b-task")]),
        ]);

        let err = workflow.validate().expect_err("cycle should fail");

        assert!(matches!(
            err,
            WorkflowValidationError::PhaseDependencyCycle { .. }
        ));
    }

    #[test]
    fn rejects_task_result_dependency_from_same_parallel_phase() {
        let mut first = task("first");
        first.depends_on_results.push("second".to_string());
        let mut parallel = phase("parallel", &[], vec![first, task("second")]);
        parallel.parallel = true;

        let err = config(vec![parallel])
            .validate()
            .expect_err("same-phase result dependency should fail");

        assert!(matches!(
            err,
            WorkflowValidationError::UnavailableTaskResultDependency { .. }
        ));
    }

    #[test]
    fn rejects_task_result_dependency_from_later_phase() {
        let mut summarize = task("summarize");
        summarize.depends_on_results.push("scan".to_string());
        let workflow = config(vec![
            phase("report", &[], vec![summarize]),
            phase("discover", &[], vec![task("scan")]),
        ]);

        let err = workflow
            .validate()
            .expect_err("later-phase result dependency should fail");

        assert!(matches!(
            err,
            WorkflowValidationError::UnavailableTaskResultDependency { .. }
        ));
    }

    #[test]
    fn allows_task_result_dependency_from_earlier_phase() {
        let upstream = phase("discover", &[], vec![task("scan")]);
        let mut summarize = task("summarize");
        summarize.depends_on_results.push("scan".to_string());
        let downstream = phase("report", &["discover"], vec![summarize]);

        config(vec![upstream, downstream])
            .validate()
            .expect("earlier-phase result should be available");
    }

    #[test]
    fn rejects_parallel_read_write_without_file_scope() {
        let mut write = task("write");
        write.mode = TaskMode::ReadWrite;
        let mut parallel = phase("parallel", &[], vec![write]);
        parallel.parallel = true;

        let err = config(vec![parallel])
            .validate()
            .expect_err("write task needs a scope");

        assert!(matches!(
            err,
            WorkflowValidationError::MissingParallelWriteScope { .. }
        ));
    }

    #[test]
    fn detects_overlapping_parallel_write_scopes_with_path_boundaries() {
        let mut left = task("auth");
        left.mode = TaskMode::ReadWrite;
        left.file_scope = vec!["src/auth/**".to_string()];
        let mut right = task("auth-login");
        right.mode = TaskMode::ReadWrite;
        right.file_scope = vec!["src/auth/login.rs".to_string()];
        let mut parallel = phase("parallel", &[], vec![left, right]);
        parallel.parallel = true;

        let err = config(vec![parallel])
            .validate()
            .expect_err("nested scopes should overlap");

        assert!(matches!(
            err,
            WorkflowValidationError::OverlappingParallelWriteScope { .. }
        ));
    }

    #[test]
    fn does_not_confuse_path_prefixes_for_overlapping_scopes() {
        let mut left = task("auth");
        left.mode = TaskMode::ReadWrite;
        left.file_scope = vec!["src/auth/**".to_string()];
        let mut right = task("auth-admin");
        right.mode = TaskMode::ReadWrite;
        right.file_scope = vec!["src/auth_admin/**".to_string()];
        let mut parallel = phase("parallel", &[], vec![left, right]);
        parallel.parallel = true;

        config(vec![parallel])
            .validate()
            .expect("component boundary scopes should not overlap");
    }

    #[test]
    fn json_roundtrip_keeps_snake_case_enum_names() {
        let mut task = task("patch");
        task.agent_type = AgentType::Implementer;
        task.mode = TaskMode::ReadWrite;
        task.isolation = IsolationMode::Worktree;
        task.file_scope = vec!["src/auth/**".to_string()];
        let mut parallel = phase("implement", &[], vec![task]);
        parallel.parallel = true;
        parallel.on_failure = FailurePolicy::Abort;
        let workflow = config(vec![parallel]);

        let json = serde_json::to_string(&workflow).expect("serialize workflow");

        assert!(json.contains("\"agent_type\":\"implementer\""));
        assert!(json.contains("\"mode\":\"read_write\""));
        assert!(json.contains("\"isolation\":\"worktree\""));
        assert!(json.contains("\"on_failure\":\"abort\""));

        let parsed: WorkflowConfig = serde_json::from_str(&json).expect("parse workflow");
        assert_eq!(parsed, workflow);
    }

    #[test]
    fn branch_result_serialization() {
        let result = BranchResult {
            branch_id: "discover".to_string(),
            task_id: "scan".to_string(),
            status: WorkflowRunStatus::Succeeded,
            artifacts: vec!["trace://branches/discover".to_string()],
            notes: Some("validated prompt surfaces".to_string()),
        };

        let json = serde_json::to_string(&result).expect("serialize branch result");

        assert!(json.contains("\"status\":\"succeeded\""));
        let parsed: BranchResult = serde_json::from_str(&json).expect("parse branch result");
        assert_eq!(parsed, result);

        let minimal: BranchResult =
            serde_json::from_str(r#"{"branch_id":"discover","task_id":"scan","status":"pending"}"#)
                .expect("parse minimal branch result");
        assert!(minimal.artifacts.is_empty());
        assert_eq!(minimal.notes, None);
    }

    #[test]
    fn leaf_result_serialization() {
        let result = LeafResult {
            leaf_id: "scan-readme".to_string(),
            task_id: "scan".to_string(),
            status: WorkflowRunStatus::Failed,
            output: Some("README needs clearer setup steps".to_string()),
            artifacts: vec!["trace://leaves/scan-readme".to_string()],
        };

        let json = serde_json::to_string(&result).expect("serialize leaf result");

        assert!(json.contains("\"status\":\"failed\""));
        let parsed: LeafResult = serde_json::from_str(&json).expect("parse leaf result");
        assert_eq!(parsed, result);

        let minimal: LeafResult = serde_json::from_str(
            r#"{"leaf_id":"scan-readme","task_id":"scan","status":"pending"}"#,
        )
        .expect("parse minimal leaf result");
        assert_eq!(minimal.output, None);
        assert!(minimal.artifacts.is_empty());
    }

    #[test]
    fn control_node_result_serialization() {
        let result = ControlNodeResult {
            node_id: "select-fix".to_string(),
            kind: ControlNodeKind::TeacherReview,
            status: WorkflowRunStatus::Running,
            selected_children: vec!["branch-a".to_string(), "branch-c".to_string()],
            summary: Some("teacher review is waiting on verifier evidence".to_string()),
        };

        let json = serde_json::to_string(&result).expect("serialize control node result");

        assert!(json.contains("\"kind\":\"teacher_review\""));
        assert!(json.contains("\"status\":\"running\""));
        let parsed: ControlNodeResult =
            serde_json::from_str(&json).expect("parse control node result");
        assert_eq!(parsed, result);

        let minimal: ControlNodeResult = serde_json::from_str(
            r#"{"node_id":"select-fix","kind":"branch_set","status":"pending"}"#,
        )
        .expect("parse minimal control node result");
        assert!(minimal.selected_children.is_empty());
        assert_eq!(minimal.summary, None);
    }
}
