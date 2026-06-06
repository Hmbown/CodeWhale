//! Typed WhaleFlow workflow configuration and validation.
//!
//! This crate deliberately stops at the Rust-owned IR boundary. Runtime tool
//! exposure, worktree application, replay, and model execution are layered on
//! top only after their cancellation and evidence semantics are proven.

#[cfg(not(target_env = "ohos"))]
mod starlark_authoring;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(not(target_env = "ohos"))]
pub use starlark_authoring::{
    compile_starlark_workflow, compile_starlark_workflow_with_repair, repair_starlark_workflow_once,
};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowSpec {
    #[serde(default)]
    pub id: Option<String>,
    pub goal: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub budget: BudgetSpec,
    #[serde(default)]
    pub permissions: PermissionSpec,
    #[serde(default)]
    pub model_policy: ModelPolicy,
    #[serde(default)]
    pub promotion_policy: PromotionPolicy,
    #[serde(default)]
    pub nodes: Vec<WorkflowNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "spec", rename_all = "snake_case")]
pub enum WorkflowNode {
    BranchSet(BranchSpec),
    Leaf(LeafSpec),
    Sequence(SequenceSpec),
    Reduce(ReduceSpec),
    TeacherReview(TeacherReviewSpec),
    LoopUntil(LoopUntilSpec),
    Cond(CondSpec),
    Expand(ExpandSpec),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchSpec {
    pub id: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub parallel: bool,
    #[serde(default)]
    pub budget: BudgetSpec,
    #[serde(default)]
    pub permissions: PermissionSpec,
    #[serde(default)]
    pub model_policy: ModelPolicy,
    #[serde(default)]
    pub children: Vec<WorkflowNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeafSpec {
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
    pub budget: BudgetSpec,
    #[serde(default)]
    pub permissions: PermissionSpec,
    #[serde(default)]
    pub model_policy: ModelPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceSpec {
    pub id: String,
    #[serde(default)]
    pub children: Vec<WorkflowNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReduceSpec {
    pub id: String,
    #[serde(default)]
    pub inputs: Vec<String>,
    pub prompt: String,
    #[serde(default)]
    pub model_policy: ModelPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeacherReviewSpec {
    pub id: String,
    #[serde(default)]
    pub candidates: Vec<String>,
    #[serde(default)]
    pub promotion_policy: PromotionPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopUntilSpec {
    pub id: String,
    pub condition: String,
    #[serde(default)]
    pub max_iterations: Option<u32>,
    #[serde(default)]
    pub children: Vec<WorkflowNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CondSpec {
    pub id: String,
    pub condition: String,
    #[serde(default)]
    pub then_nodes: Vec<WorkflowNode>,
    #[serde(default)]
    pub else_nodes: Vec<WorkflowNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpandSpec {
    pub id: String,
    pub source: String,
    #[serde(default)]
    pub max_children: Option<usize>,
    #[serde(default)]
    pub template: Option<Box<WorkflowNode>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BudgetSpec {
    #[serde(default)]
    pub max_steps: Option<u32>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub max_parallel: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PermissionSpec {
    #[serde(default)]
    pub allow_write: bool,
    #[serde(default)]
    pub allow_network: bool,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub file_scope: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModelPolicy {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub fallback_models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PromotionPolicy {
    #[serde(default)]
    pub strategy: PromotionStrategy,
    #[serde(default)]
    pub require_teacher_review: bool,
    #[serde(default)]
    pub min_successful_branches: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromotionStrategy {
    #[default]
    All,
    FirstSuccess,
    BestScore,
    TeacherSelected,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowExecution {
    pub status: WorkflowRunStatus,
    #[serde(default)]
    pub leaf_results: Vec<LeafResult>,
    #[serde(default)]
    pub branch_results: Vec<BranchResult>,
    #[serde(default)]
    pub control_node_results: Vec<ControlNodeResult>,
}

impl Default for WorkflowExecution {
    fn default() -> Self {
        Self {
            status: WorkflowRunStatus::Succeeded,
            leaf_results: Vec::new(),
            branch_results: Vec::new(),
            control_node_results: Vec::new(),
        }
    }
}

impl WorkflowExecution {
    pub fn mark_failed(&mut self) {
        self.status = WorkflowRunStatus::Failed;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockLeafOutcome {
    pub status: WorkflowRunStatus,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<String>,
}

impl MockLeafOutcome {
    pub fn succeeded(output: impl Into<String>) -> Self {
        Self {
            status: WorkflowRunStatus::Succeeded,
            output: Some(output.into()),
            artifacts: Vec::new(),
        }
    }

    pub fn failed(output: impl Into<String>) -> Self {
        Self {
            status: WorkflowRunStatus::Failed,
            output: Some(output.into()),
            artifacts: Vec::new(),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct MockWorkflowExecutor {
    leaf_outcomes: BTreeMap<String, MockLeafOutcome>,
    predicate_results: BTreeMap<String, Vec<bool>>,
    generated_nodes: BTreeMap<String, Vec<WorkflowNode>>,
}

impl MockWorkflowExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_leaf_outcome(
        mut self,
        leaf_id: impl Into<String>,
        outcome: MockLeafOutcome,
    ) -> Self {
        self.leaf_outcomes.insert(leaf_id.into(), outcome);
        self
    }

    pub fn with_predicate_results(
        mut self,
        node_id: impl Into<String>,
        results: Vec<bool>,
    ) -> Self {
        self.predicate_results.insert(node_id.into(), results);
        self
    }

    pub fn with_generated_nodes(
        mut self,
        node_id: impl Into<String>,
        nodes: Vec<WorkflowNode>,
    ) -> Self {
        self.generated_nodes.insert(node_id.into(), nodes);
        self
    }

    pub fn run(
        &mut self,
        spec: &WorkflowSpec,
    ) -> Result<WorkflowExecution, WorkflowExecutionError> {
        validate_workflow_nodes(&spec.nodes)?;
        let mut execution = WorkflowExecution::default();
        self.execute_nodes(&spec.nodes, &mut execution)?;
        Ok(execution)
    }

    fn execute_nodes(
        &mut self,
        nodes: &[WorkflowNode],
        execution: &mut WorkflowExecution,
    ) -> Result<(), WorkflowExecutionError> {
        for node in nodes {
            self.execute_node(node, execution)?;
        }
        Ok(())
    }

    fn execute_node(
        &mut self,
        node: &WorkflowNode,
        execution: &mut WorkflowExecution,
    ) -> Result<(), WorkflowExecutionError> {
        match node {
            WorkflowNode::BranchSet(spec) => self.execute_branch_set(spec, execution),
            WorkflowNode::Leaf(spec) => {
                self.execute_leaf(spec, execution);
                Ok(())
            }
            WorkflowNode::Sequence(spec) => {
                self.execute_nodes(&spec.children, execution)?;
                execution.control_node_results.push(ControlNodeResult {
                    node_id: spec.id.clone(),
                    kind: ControlNodeKind::Sequence,
                    status: execution.status,
                    selected_children: spec.children.iter().map(node_id).collect(),
                    summary: Some("sequence executed in declaration order".to_string()),
                });
                Ok(())
            }
            WorkflowNode::Reduce(spec) => {
                execution.control_node_results.push(ControlNodeResult {
                    node_id: spec.id.clone(),
                    kind: ControlNodeKind::Reduce,
                    status: WorkflowRunStatus::Succeeded,
                    selected_children: spec.inputs.clone(),
                    summary: Some(spec.prompt.clone()),
                });
                Ok(())
            }
            WorkflowNode::TeacherReview(spec) => {
                execution.control_node_results.push(ControlNodeResult {
                    node_id: spec.id.clone(),
                    kind: ControlNodeKind::TeacherReview,
                    status: WorkflowRunStatus::Succeeded,
                    selected_children: spec.candidates.clone(),
                    summary: Some(
                        "teacher review scaffold selected declared candidates".to_string(),
                    ),
                });
                Ok(())
            }
            WorkflowNode::LoopUntil(spec) => self.execute_loop_until(spec, execution),
            WorkflowNode::Cond(spec) => self.execute_cond(spec, execution),
            WorkflowNode::Expand(spec) => self.execute_expand(spec, execution),
        }
    }

    fn execute_branch_set(
        &mut self,
        spec: &BranchSpec,
        execution: &mut WorkflowExecution,
    ) -> Result<(), WorkflowExecutionError> {
        let before = execution.leaf_results.len();
        self.execute_nodes(&spec.children, execution)?;
        let branch_failed = execution.leaf_results[before..]
            .iter()
            .any(|result| result.status != WorkflowRunStatus::Succeeded);
        let status = if branch_failed {
            WorkflowRunStatus::Failed
        } else {
            WorkflowRunStatus::Succeeded
        };
        if status == WorkflowRunStatus::Failed {
            execution.mark_failed();
        }
        execution.branch_results.push(BranchResult {
            branch_id: spec.id.clone(),
            task_id: spec.id.clone(),
            status,
            artifacts: Vec::new(),
            notes: Some("mock branch set executed without runtime fanout".to_string()),
        });
        execution.control_node_results.push(ControlNodeResult {
            node_id: spec.id.clone(),
            kind: ControlNodeKind::BranchSet,
            status,
            selected_children: spec.children.iter().map(node_id).collect(),
            summary: Some("branch set scaffold executed children deterministically".to_string()),
        });
        Ok(())
    }

    fn execute_leaf(&mut self, spec: &LeafSpec, execution: &mut WorkflowExecution) {
        let outcome = self
            .leaf_outcomes
            .remove(&spec.id)
            .unwrap_or_else(|| MockLeafOutcome::succeeded(format!("mock leaf {}", spec.id)));
        if outcome.status != WorkflowRunStatus::Succeeded {
            execution.mark_failed();
        }
        execution.leaf_results.push(LeafResult {
            leaf_id: spec.id.clone(),
            task_id: spec.id.clone(),
            status: outcome.status,
            output: outcome.output,
            artifacts: outcome.artifacts,
        });
    }

    fn execute_loop_until(
        &mut self,
        spec: &LoopUntilSpec,
        execution: &mut WorkflowExecution,
    ) -> Result<(), WorkflowExecutionError> {
        let max_iterations = spec.max_iterations.unwrap_or(1).max(1);
        let mut iterations = 0;
        let mut passed = false;
        while iterations < max_iterations {
            iterations += 1;
            self.execute_nodes(&spec.children, execution)?;
            if self.next_predicate_result(&spec.id) {
                passed = true;
                break;
            }
        }
        let status = if passed {
            WorkflowRunStatus::Succeeded
        } else {
            WorkflowRunStatus::Failed
        };
        if status == WorkflowRunStatus::Failed {
            execution.mark_failed();
        }
        execution.control_node_results.push(ControlNodeResult {
            node_id: spec.id.clone(),
            kind: ControlNodeKind::LoopUntil,
            status,
            selected_children: spec.children.iter().map(node_id).collect(),
            summary: Some(format!("loop_until iterations={iterations}")),
        });
        Ok(())
    }

    fn execute_cond(
        &mut self,
        spec: &CondSpec,
        execution: &mut WorkflowExecution,
    ) -> Result<(), WorkflowExecutionError> {
        let passed = self.next_predicate_result(&spec.id);
        let selected_nodes = if passed {
            &spec.then_nodes
        } else {
            &spec.else_nodes
        };
        self.execute_nodes(selected_nodes, execution)?;
        execution.control_node_results.push(ControlNodeResult {
            node_id: spec.id.clone(),
            kind: ControlNodeKind::Cond,
            status: WorkflowRunStatus::Succeeded,
            selected_children: selected_nodes.iter().map(node_id).collect(),
            summary: Some(format!("predicate_result={passed}")),
        });
        Ok(())
    }

    fn execute_expand(
        &mut self,
        spec: &ExpandSpec,
        execution: &mut WorkflowExecution,
    ) -> Result<(), WorkflowExecutionError> {
        let mut nodes = self.generated_nodes.remove(&spec.id).unwrap_or_default();
        if let Some(max_children) = spec.max_children {
            nodes.truncate(max_children);
        }
        validate_workflow_nodes(&nodes)?;
        self.execute_nodes(&nodes, execution)?;
        execution.control_node_results.push(ControlNodeResult {
            node_id: spec.id.clone(),
            kind: ControlNodeKind::Expand,
            status: WorkflowRunStatus::Succeeded,
            selected_children: nodes.iter().map(node_id).collect(),
            summary: Some(format!("expanded_from={}", spec.source)),
        });
        Ok(())
    }

    fn next_predicate_result(&mut self, node_id: &str) -> bool {
        let Some(results) = self.predicate_results.get_mut(node_id) else {
            return false;
        };
        if results.is_empty() {
            return false;
        }
        results.remove(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchCandidate {
    pub branch_id: String,
    pub status: WorkflowRunStatus,
    pub score: u32,
    pub cost: u64,
    #[serde(default)]
    pub diversity_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchTournament {
    #[serde(default)]
    pub min_score: u32,
}

impl BranchTournament {
    pub fn select(&self, candidates: &[BranchCandidate]) -> Option<BranchCandidate> {
        candidates
            .iter()
            .filter(|candidate| {
                candidate.status == WorkflowRunStatus::Succeeded
                    && candidate.score >= self.min_score
            })
            .min_by_key(|candidate| (candidate.cost, std::cmp::Reverse(candidate.score)))
            .cloned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParetoFrontier {
    #[serde(default = "default_frontier_limit")]
    pub max_items: usize,
}

impl Default for ParetoFrontier {
    fn default() -> Self {
        Self {
            max_items: default_frontier_limit(),
        }
    }
}

impl ParetoFrontier {
    pub fn select(&self, candidates: &[BranchCandidate]) -> Vec<BranchCandidate> {
        let mut frontier: Vec<_> = candidates
            .iter()
            .filter(|candidate| candidate.status == WorkflowRunStatus::Succeeded)
            .filter(|candidate| {
                !candidates.iter().any(|other| {
                    other.status == WorkflowRunStatus::Succeeded
                        && other.score >= candidate.score
                        && other.cost <= candidate.cost
                        && (other.score > candidate.score || other.cost < candidate.cost)
                })
            })
            .cloned()
            .collect();
        frontier.sort_by_key(|candidate| (std::cmp::Reverse(candidate.score), candidate.cost));
        frontier.truncate(self.max_items.max(1));
        frontier
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WorkflowExecutionError {
    #[error("{kind} node id must not be empty")]
    EmptyNodeId { kind: &'static str },
    #[error("leaf `{leaf}` prompt must not be empty")]
    EmptyLeafPrompt { leaf: String },
    #[error("duplicate workflow node `{node}`")]
    DuplicateNodeId { node: String },
}

fn default_frontier_limit() -> usize {
    8
}

fn node_id(node: &WorkflowNode) -> String {
    match node {
        WorkflowNode::BranchSet(spec) => spec.id.clone(),
        WorkflowNode::Leaf(spec) => spec.id.clone(),
        WorkflowNode::Sequence(spec) => spec.id.clone(),
        WorkflowNode::Reduce(spec) => spec.id.clone(),
        WorkflowNode::TeacherReview(spec) => spec.id.clone(),
        WorkflowNode::LoopUntil(spec) => spec.id.clone(),
        WorkflowNode::Cond(spec) => spec.id.clone(),
        WorkflowNode::Expand(spec) => spec.id.clone(),
    }
}

fn validate_workflow_nodes(nodes: &[WorkflowNode]) -> Result<(), WorkflowExecutionError> {
    let mut seen = BTreeSet::new();
    validate_workflow_nodes_inner(nodes, &mut seen)
}

fn validate_workflow_nodes_inner(
    nodes: &[WorkflowNode],
    seen: &mut BTreeSet<String>,
) -> Result<(), WorkflowExecutionError> {
    for node in nodes {
        let id = node_id(node);
        let kind = control_kind_name(node);
        if id.trim().is_empty() {
            return Err(WorkflowExecutionError::EmptyNodeId { kind });
        }
        if !seen.insert(id.clone()) {
            return Err(WorkflowExecutionError::DuplicateNodeId { node: id });
        }
        match node {
            WorkflowNode::BranchSet(spec) => validate_workflow_nodes_inner(&spec.children, seen)?,
            WorkflowNode::Leaf(spec) => {
                if spec.prompt.trim().is_empty() {
                    return Err(WorkflowExecutionError::EmptyLeafPrompt {
                        leaf: spec.id.clone(),
                    });
                }
            }
            WorkflowNode::Sequence(spec) => validate_workflow_nodes_inner(&spec.children, seen)?,
            WorkflowNode::LoopUntil(spec) => validate_workflow_nodes_inner(&spec.children, seen)?,
            WorkflowNode::Cond(spec) => {
                validate_workflow_nodes_inner(&spec.then_nodes, seen)?;
                validate_workflow_nodes_inner(&spec.else_nodes, seen)?;
            }
            WorkflowNode::Reduce(_) | WorkflowNode::TeacherReview(_) | WorkflowNode::Expand(_) => {}
        }
    }
    Ok(())
}

fn control_kind_name(node: &WorkflowNode) -> &'static str {
    match node {
        WorkflowNode::BranchSet(_) => "branch_set",
        WorkflowNode::Leaf(_) => "leaf",
        WorkflowNode::Sequence(_) => "sequence",
        WorkflowNode::Reduce(_) => "reduce",
        WorkflowNode::TeacherReview(_) => "teacher_review",
        WorkflowNode::LoopUntil(_) => "loop_until",
        WorkflowNode::Cond(_) => "cond",
        WorkflowNode::Expand(_) => "expand",
    }
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

    fn leaf_node(id: &str) -> WorkflowNode {
        WorkflowNode::Leaf(LeafSpec {
            id: id.to_string(),
            prompt: format!("run {id}"),
            agent_type: AgentType::General,
            mode: TaskMode::ReadOnly,
            isolation: IsolationMode::Shared,
            file_scope: Vec::new(),
            depends_on_results: Vec::new(),
            budget: BudgetSpec::default(),
            permissions: PermissionSpec::default(),
            model_policy: ModelPolicy::default(),
        })
    }

    fn invalid_leaf_node(id: &str) -> WorkflowNode {
        WorkflowNode::Leaf(LeafSpec {
            id: id.to_string(),
            prompt: " ".to_string(),
            agent_type: AgentType::General,
            mode: TaskMode::ReadOnly,
            isolation: IsolationMode::Shared,
            file_scope: Vec::new(),
            depends_on_results: Vec::new(),
            budget: BudgetSpec::default(),
            permissions: PermissionSpec::default(),
            model_policy: ModelPolicy::default(),
        })
    }

    fn workflow_spec(nodes: Vec<WorkflowNode>) -> WorkflowSpec {
        WorkflowSpec {
            id: Some("mock-workflow".to_string()),
            goal: "prove mock executor control flow".to_string(),
            description: None,
            budget: BudgetSpec::default(),
            permissions: PermissionSpec::default(),
            model_policy: ModelPolicy::default(),
            promotion_policy: PromotionPolicy::default(),
            nodes,
        }
    }

    fn control_result<'a>(
        execution: &'a WorkflowExecution,
        node_id: &str,
    ) -> &'a ControlNodeResult {
        execution
            .control_node_results
            .iter()
            .find(|result| result.node_id == node_id)
            .expect("control node result should exist")
    }

    fn candidate(
        branch_id: &str,
        status: WorkflowRunStatus,
        score: u32,
        cost: u64,
        diversity_key: &str,
    ) -> BranchCandidate {
        BranchCandidate {
            branch_id: branch_id.to_string(),
            status,
            score,
            cost,
            diversity_key: Some(diversity_key.to_string()),
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
    fn workflow_ir_roundtrip() {
        let discover_leaf = LeafSpec {
            id: "scan-readme".to_string(),
            prompt: "Inspect README setup gaps".to_string(),
            agent_type: AgentType::Explore,
            mode: TaskMode::ReadOnly,
            isolation: IsolationMode::Shared,
            file_scope: vec!["README.md".to_string()],
            depends_on_results: Vec::new(),
            budget: BudgetSpec {
                max_steps: Some(8),
                timeout_secs: Some(300),
                max_parallel: None,
            },
            permissions: PermissionSpec::default(),
            model_policy: ModelPolicy {
                provider: Some("openai".to_string()),
                model: Some("gpt-5.4".to_string()),
                fallback_models: Vec::new(),
            },
        };
        let workflow = WorkflowSpec {
            id: Some("v090-readme-check".to_string()),
            goal: "tighten setup docs".to_string(),
            description: Some("metadata-only typed WhaleFlow IR".to_string()),
            budget: BudgetSpec {
                max_steps: Some(30),
                timeout_secs: Some(1_800),
                max_parallel: Some(2),
            },
            permissions: PermissionSpec {
                allow_write: false,
                allow_network: false,
                allowed_tools: vec!["rg".to_string()],
                file_scope: vec!["README.md".to_string()],
            },
            model_policy: ModelPolicy {
                provider: Some("openai".to_string()),
                model: Some("gpt-5.4".to_string()),
                fallback_models: vec!["gpt-5.4-mini".to_string()],
            },
            promotion_policy: PromotionPolicy {
                strategy: PromotionStrategy::TeacherSelected,
                require_teacher_review: true,
                min_successful_branches: Some(1),
            },
            nodes: vec![
                WorkflowNode::BranchSet(BranchSpec {
                    id: "discover".to_string(),
                    description: Some("parallel doc inspection".to_string()),
                    parallel: true,
                    budget: BudgetSpec {
                        max_steps: Some(12),
                        timeout_secs: Some(600),
                        max_parallel: Some(2),
                    },
                    permissions: PermissionSpec::default(),
                    model_policy: ModelPolicy::default(),
                    children: vec![WorkflowNode::Leaf(discover_leaf)],
                }),
                WorkflowNode::Sequence(SequenceSpec {
                    id: "review-and-reduce".to_string(),
                    children: vec![
                        WorkflowNode::TeacherReview(TeacherReviewSpec {
                            id: "select-best".to_string(),
                            candidates: vec!["scan-readme".to_string()],
                            promotion_policy: PromotionPolicy {
                                strategy: PromotionStrategy::BestScore,
                                require_teacher_review: true,
                                min_successful_branches: Some(1),
                            },
                        }),
                        WorkflowNode::Reduce(ReduceSpec {
                            id: "summarize".to_string(),
                            inputs: vec!["scan-readme".to_string()],
                            prompt: "Summarize the smallest safe patch".to_string(),
                            model_policy: ModelPolicy::default(),
                        }),
                    ],
                }),
                WorkflowNode::Cond(CondSpec {
                    id: "maybe-expand".to_string(),
                    condition: "summary identifies multiple independent gaps".to_string(),
                    then_nodes: vec![WorkflowNode::Expand(ExpandSpec {
                        id: "split-followups".to_string(),
                        source: "summarize".to_string(),
                        max_children: None,
                        template: Some(Box::new(WorkflowNode::Leaf(LeafSpec {
                            id: "followup-template".to_string(),
                            prompt: "Patch one independent gap".to_string(),
                            agent_type: AgentType::Implementer,
                            mode: TaskMode::ReadWrite,
                            isolation: IsolationMode::Worktree,
                            file_scope: vec!["README.md".to_string()],
                            depends_on_results: Vec::new(),
                            budget: BudgetSpec::default(),
                            permissions: PermissionSpec {
                                allow_write: true,
                                allow_network: false,
                                allowed_tools: Vec::new(),
                                file_scope: vec!["README.md".to_string()],
                            },
                            model_policy: ModelPolicy::default(),
                        }))),
                    })],
                    else_nodes: vec![WorkflowNode::LoopUntil(LoopUntilSpec {
                        id: "verify-once".to_string(),
                        condition: "local verification passes".to_string(),
                        max_iterations: Some(1),
                        children: Vec::new(),
                    })],
                }),
            ],
        };

        let json = serde_json::to_string_pretty(&workflow).expect("serialize workflow ir");

        assert!(json.contains("\"kind\": \"branch_set\""));
        assert!(json.contains("\"strategy\": \"teacher_selected\""));
        let parsed: WorkflowSpec = serde_json::from_str(&json).expect("parse workflow ir");
        assert_eq!(parsed, workflow);

        let minimal: WorkflowSpec = serde_json::from_str(r#"{"goal":"ship v0.9","nodes":[]}"#)
            .expect("parse minimal workflow ir");
        assert_eq!(minimal.budget, BudgetSpec::default());
        assert_eq!(minimal.permissions, PermissionSpec::default());
        assert_eq!(minimal.model_policy, ModelPolicy::default());
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

    #[test]
    fn run_mock_three_branch_workflow() {
        let workflow = workflow_spec(vec![WorkflowNode::BranchSet(BranchSpec {
            id: "discover".to_string(),
            description: None,
            parallel: true,
            budget: BudgetSpec::default(),
            permissions: PermissionSpec::default(),
            model_policy: ModelPolicy::default(),
            children: vec![
                leaf_node("scan-readme"),
                leaf_node("scan-config"),
                leaf_node("scan-tests"),
            ],
        })]);

        let mut executor = MockWorkflowExecutor::new();
        let execution = executor.run(&workflow).expect("mock workflow should run");

        assert_eq!(execution.status, WorkflowRunStatus::Succeeded);
        assert_eq!(
            execution
                .leaf_results
                .iter()
                .map(|result| result.leaf_id.as_str())
                .collect::<Vec<_>>(),
            vec!["scan-readme", "scan-config", "scan-tests"]
        );
        assert_eq!(execution.branch_results.len(), 1);
        assert_eq!(execution.branch_results[0].branch_id, "discover");
        assert_eq!(
            control_result(&execution, "discover").selected_children,
            vec!["scan-readme", "scan-config", "scan-tests"]
        );
    }

    #[test]
    fn loop_until_stops_on_pass() {
        let workflow = workflow_spec(vec![WorkflowNode::LoopUntil(LoopUntilSpec {
            id: "verify".to_string(),
            condition: "verification passed".to_string(),
            max_iterations: Some(5),
            children: vec![leaf_node("run-check")],
        })]);

        let mut executor =
            MockWorkflowExecutor::new().with_predicate_results("verify", vec![false, false, true]);
        let execution = executor.run(&workflow).expect("loop should run");

        assert_eq!(execution.status, WorkflowRunStatus::Succeeded);
        assert_eq!(execution.leaf_results.len(), 3);
        assert_eq!(
            control_result(&execution, "verify").summary.as_deref(),
            Some("loop_until iterations=3")
        );
    }

    #[test]
    fn loop_until_honors_max_iters() {
        let workflow = workflow_spec(vec![WorkflowNode::LoopUntil(LoopUntilSpec {
            id: "verify".to_string(),
            condition: "verification passed".to_string(),
            max_iterations: Some(2),
            children: vec![leaf_node("run-check")],
        })]);

        let mut executor =
            MockWorkflowExecutor::new().with_predicate_results("verify", vec![false, false, true]);
        let execution = executor.run(&workflow).expect("loop should run");

        assert_eq!(execution.status, WorkflowRunStatus::Failed);
        assert_eq!(execution.leaf_results.len(), 2);
        assert_eq!(
            control_result(&execution, "verify").summary.as_deref(),
            Some("loop_until iterations=2")
        );
    }

    #[test]
    fn cond_uses_logged_predicate_result() {
        let workflow = workflow_spec(vec![WorkflowNode::Cond(CondSpec {
            id: "should-fix".to_string(),
            condition: "finding requires a patch".to_string(),
            then_nodes: vec![leaf_node("patch")],
            else_nodes: vec![leaf_node("report-only")],
        })]);

        let mut executor =
            MockWorkflowExecutor::new().with_predicate_results("should-fix", vec![true]);
        let execution = executor.run(&workflow).expect("cond should run");

        assert_eq!(
            execution
                .leaf_results
                .iter()
                .map(|result| result.leaf_id.as_str())
                .collect::<Vec<_>>(),
            vec!["patch"]
        );
        assert_eq!(
            control_result(&execution, "should-fix").summary.as_deref(),
            Some("predicate_result=true")
        );
    }

    #[test]
    fn expand_respects_max_children() {
        let workflow = workflow_spec(vec![WorkflowNode::Expand(ExpandSpec {
            id: "split".to_string(),
            source: "plan".to_string(),
            max_children: Some(2),
            template: None,
        })]);

        let generated = vec![leaf_node("first"), leaf_node("second"), leaf_node("third")];
        let mut executor = MockWorkflowExecutor::new().with_generated_nodes("split", generated);
        let execution = executor.run(&workflow).expect("expand should run");

        assert_eq!(
            execution
                .leaf_results
                .iter()
                .map(|result| result.leaf_id.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert_eq!(
            control_result(&execution, "split").selected_children,
            vec!["first", "second"]
        );
    }

    #[test]
    fn expand_generated_nodes_validate_before_run() {
        let workflow = workflow_spec(vec![WorkflowNode::Expand(ExpandSpec {
            id: "split".to_string(),
            source: "plan".to_string(),
            max_children: None,
            template: None,
        })]);

        let mut executor = MockWorkflowExecutor::new()
            .with_generated_nodes("split", vec![invalid_leaf_node("bad")]);
        let err = executor
            .run(&workflow)
            .expect_err("invalid generated leaf should fail before execution");

        assert_eq!(
            err,
            WorkflowExecutionError::EmptyLeafPrompt {
                leaf: "bad".to_string()
            }
        );
    }

    #[test]
    fn tournament_selects_passing_minimal_branch() {
        let tournament = BranchTournament { min_score: 60 };
        let candidates = vec![
            candidate(
                "expensive-pass",
                WorkflowRunStatus::Succeeded,
                90,
                90,
                "quality",
            ),
            candidate("failed-cheap", WorkflowRunStatus::Failed, 100, 1, "broken"),
            candidate(
                "cheap-pass",
                WorkflowRunStatus::Succeeded,
                70,
                10,
                "minimal",
            ),
            candidate("too-low", WorkflowRunStatus::Succeeded, 40, 2, "weak"),
        ];

        let selected = tournament
            .select(&candidates)
            .expect("one passing branch should be selected");

        assert_eq!(selected.branch_id, "cheap-pass");
    }

    #[test]
    fn pareto_frontier_keeps_diverse_candidates() {
        let frontier = ParetoFrontier { max_items: 4 };
        let candidates = vec![
            candidate("quality", WorkflowRunStatus::Succeeded, 95, 100, "quality"),
            candidate("minimal", WorkflowRunStatus::Succeeded, 70, 10, "small"),
            candidate("dominated", WorkflowRunStatus::Succeeded, 60, 40, "middle"),
            candidate("failed", WorkflowRunStatus::Failed, 100, 1, "broken"),
        ];

        let selected = frontier.select(&candidates);

        assert_eq!(
            selected
                .iter()
                .map(|candidate| candidate.branch_id.as_str())
                .collect::<Vec<_>>(),
            vec!["quality", "minimal"]
        );
        assert_eq!(
            selected
                .iter()
                .filter_map(|candidate| candidate.diversity_key.as_deref())
                .collect::<Vec<_>>(),
            vec!["quality", "small"]
        );
    }
}
