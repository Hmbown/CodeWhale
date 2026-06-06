use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    BranchResult, BranchSpec, CondSpec, ControlNodeKind, ControlNodeResult, ExpandSpec, LeafResult,
    LeafSpec, LoopUntilSpec, SequenceSpec, WorkflowExecution, WorkflowExecutionError,
    WorkflowMemoUsage, WorkflowNode, WorkflowRunStatus, WorkflowSpec, WorkflowUsage,
    validate_workflow_nodes,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReplayOptions {
    #[serde(default)]
    pub allow_live_replay: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowReplayTrace {
    pub trace_id: String,
    #[serde(default)]
    pub leaf_records: Vec<ReplayLeafRecord>,
    #[serde(default)]
    pub control_records: Vec<ReplayControlRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayLeafRecord {
    pub trace_id: String,
    pub leaf_id: String,
    pub input_hash: String,
    pub result: LeafResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayControlRecord {
    pub trace_id: String,
    pub node_id: String,
    pub kind: ControlNodeKind,
    pub result: ControlNodeResult,
    #[serde(default)]
    pub generated_nodes: Vec<WorkflowNode>,
}

#[derive(Debug, Clone)]
pub struct WorkflowReplayExecutor {
    trace_id: String,
    options: ReplayOptions,
    leaf_records: BTreeMap<ReplayLeafKey, LeafResult>,
    control_records: BTreeMap<ReplayControlKey, ReplayControlRecord>,
    resolved_outputs: BTreeMap<String, Option<String>>,
}

impl WorkflowReplayExecutor {
    pub fn new(trace: WorkflowReplayTrace) -> Self {
        Self::with_options(trace, ReplayOptions::default())
    }

    pub fn with_options(trace: WorkflowReplayTrace, options: ReplayOptions) -> Self {
        let trace_id = trace.trace_id;
        let leaf_records = trace
            .leaf_records
            .into_iter()
            .map(|record| {
                (
                    ReplayLeafKey {
                        trace_id: record.trace_id,
                        leaf_id: record.leaf_id,
                        input_hash: record.input_hash,
                    },
                    record.result,
                )
            })
            .collect();
        let control_records = trace
            .control_records
            .into_iter()
            .map(|record| {
                (
                    ReplayControlKey {
                        trace_id: record.trace_id.clone(),
                        node_id: record.node_id.clone(),
                        kind: record.kind,
                    },
                    record,
                )
            })
            .collect();

        Self {
            trace_id,
            options,
            leaf_records,
            control_records,
            resolved_outputs: BTreeMap::new(),
        }
    }

    pub fn run(&mut self, spec: &WorkflowSpec) -> Result<WorkflowExecution, WorkflowReplayError> {
        validate_workflow_nodes(&spec.nodes)?;
        let mut execution = WorkflowExecution::default();
        self.execute_nodes(spec, &spec.nodes, &mut execution)?;
        Ok(execution)
    }

    fn execute_nodes(
        &mut self,
        spec: &WorkflowSpec,
        nodes: &[WorkflowNode],
        execution: &mut WorkflowExecution,
    ) -> Result<(), WorkflowReplayError> {
        for node in nodes {
            self.execute_node(spec, node, execution)?;
        }
        Ok(())
    }

    fn execute_node(
        &mut self,
        spec: &WorkflowSpec,
        node: &WorkflowNode,
        execution: &mut WorkflowExecution,
    ) -> Result<(), WorkflowReplayError> {
        match node {
            WorkflowNode::BranchSet(branch) => self.execute_branch_set(spec, branch, execution),
            WorkflowNode::Leaf(leaf) => self.execute_leaf(spec, leaf, execution),
            WorkflowNode::Sequence(sequence) => self.execute_sequence(spec, sequence, execution),
            WorkflowNode::Reduce(reduce) => self.replay_recorded_control(
                reduce.id.as_str(),
                ControlNodeKind::Reduce,
                execution,
                Some(reduce.inputs.clone()),
                Some(reduce.prompt.clone()),
            ),
            WorkflowNode::TeacherReview(review) => self.replay_recorded_control(
                review.id.as_str(),
                ControlNodeKind::TeacherReview,
                execution,
                Some(review.candidates.clone()),
                Some("teacher review replayed from recorded candidates".to_string()),
            ),
            WorkflowNode::LoopUntil(loop_until) => {
                self.execute_loop_until(spec, loop_until, execution)
            }
            WorkflowNode::Cond(cond) => self.execute_cond(spec, cond, execution),
            WorkflowNode::Expand(expand) => self.execute_expand(spec, expand, execution),
        }
    }

    fn execute_branch_set(
        &mut self,
        spec: &WorkflowSpec,
        branch: &BranchSpec,
        execution: &mut WorkflowExecution,
    ) -> Result<(), WorkflowReplayError> {
        let before = execution.leaf_results.len();
        self.execute_nodes(spec, &branch.children, execution)?;
        let status = branch_status(&execution.leaf_results[before..]);
        let mut usage = WorkflowUsage::default();
        let mut memo_usage = WorkflowMemoUsage::default();
        for result in &execution.leaf_results[before..] {
            usage.add_assign(result.usage);
            memo_usage.add_assign(result.memo_usage);
        }
        if status == WorkflowRunStatus::ReplayDiverged {
            execution.mark_replay_diverged();
        } else if status == WorkflowRunStatus::Failed {
            execution.mark_failed();
        }
        execution.branch_results.push(BranchResult {
            branch_id: branch.id.clone(),
            task_id: branch.id.clone(),
            status,
            usage,
            memo_usage,
            artifacts: Vec::new(),
            notes: Some("replay branch set evaluated from recorded leaf results".to_string()),
        });
        self.replay_recorded_control(
            branch.id.as_str(),
            ControlNodeKind::BranchSet,
            execution,
            Some(branch.children.iter().map(workflow_node_id).collect()),
            Some("branch set replayed declared children".to_string()),
        )
    }

    fn execute_leaf(
        &mut self,
        spec: &WorkflowSpec,
        leaf: &LeafSpec,
        execution: &mut WorkflowExecution,
    ) -> Result<(), WorkflowReplayError> {
        let inputs = resolved_inputs_for_leaf(leaf, &self.resolved_outputs);
        let input_hash = compute_leaf_input_hash(spec, leaf, &inputs)?;
        let key = ReplayLeafKey {
            trace_id: self.trace_id.clone(),
            leaf_id: leaf.id.clone(),
            input_hash,
        };

        let Some(result) = self.leaf_records.get(&key).cloned() else {
            if self.options.allow_live_replay {
                return Err(WorkflowReplayError::LiveReplayUnavailable {
                    leaf: leaf.id.clone(),
                });
            }
            execution.mark_replay_diverged();
            let result = LeafResult {
                leaf_id: leaf.id.clone(),
                task_id: leaf.id.clone(),
                status: WorkflowRunStatus::ReplayDiverged,
                usage: WorkflowUsage::default(),
                memo_usage: WorkflowMemoUsage::default(),
                output: None,
                artifacts: Vec::new(),
            };
            self.resolved_outputs.insert(leaf.id.clone(), None);
            execution.leaf_results.push(result);
            return Ok(());
        };

        if result.status == WorkflowRunStatus::ReplayDiverged {
            execution.mark_replay_diverged();
        } else if result.status == WorkflowRunStatus::Failed {
            execution.mark_failed();
        }
        execution.usage.add_assign(result.usage);
        execution.memo_usage.add_assign(result.memo_usage);
        self.resolved_outputs
            .insert(leaf.id.clone(), result.output.clone());
        execution.leaf_results.push(result);
        Ok(())
    }

    fn execute_sequence(
        &mut self,
        spec: &WorkflowSpec,
        sequence: &SequenceSpec,
        execution: &mut WorkflowExecution,
    ) -> Result<(), WorkflowReplayError> {
        self.execute_nodes(spec, &sequence.children, execution)?;
        self.replay_recorded_control(
            sequence.id.as_str(),
            ControlNodeKind::Sequence,
            execution,
            Some(sequence.children.iter().map(workflow_node_id).collect()),
            Some("sequence replayed in declaration order".to_string()),
        )
    }

    fn execute_loop_until(
        &mut self,
        spec: &WorkflowSpec,
        loop_until: &LoopUntilSpec,
        execution: &mut WorkflowExecution,
    ) -> Result<(), WorkflowReplayError> {
        let record = self.control_record(loop_until.id.as_str(), ControlNodeKind::LoopUntil);
        let selected = record
            .as_ref()
            .map(|record| record.result.selected_children.clone())
            .unwrap_or_else(|| loop_until.children.iter().map(workflow_node_id).collect());
        let children = select_nodes(&loop_until.children, &selected);
        self.execute_nodes(spec, &children, execution)?;
        self.push_control_or_diverge(
            loop_until.id.as_str(),
            ControlNodeKind::LoopUntil,
            execution,
            record,
            Some(selected),
            Some("loop_until replayed recorded child selection".to_string()),
        );
        Ok(())
    }

    fn execute_cond(
        &mut self,
        spec: &WorkflowSpec,
        cond: &CondSpec,
        execution: &mut WorkflowExecution,
    ) -> Result<(), WorkflowReplayError> {
        let record = self.control_record(cond.id.as_str(), ControlNodeKind::Cond);
        let selected = record
            .as_ref()
            .map(|record| record.result.selected_children.clone())
            .unwrap_or_default();
        let available = cond
            .then_nodes
            .iter()
            .chain(cond.else_nodes.iter())
            .cloned()
            .collect::<Vec<_>>();
        let nodes = select_nodes(&available, &selected);
        self.execute_nodes(spec, &nodes, execution)?;
        self.push_control_or_diverge(
            cond.id.as_str(),
            ControlNodeKind::Cond,
            execution,
            record,
            Some(selected),
            Some("cond replayed recorded branch selection".to_string()),
        );
        Ok(())
    }

    fn execute_expand(
        &mut self,
        spec: &WorkflowSpec,
        expand: &ExpandSpec,
        execution: &mut WorkflowExecution,
    ) -> Result<(), WorkflowReplayError> {
        let record = self.control_record(expand.id.as_str(), ControlNodeKind::Expand);
        let generated_nodes = record
            .as_ref()
            .map(|record| record.generated_nodes.clone())
            .unwrap_or_default();
        validate_workflow_nodes(&generated_nodes)?;
        self.execute_nodes(spec, &generated_nodes, execution)?;
        let selected = record
            .as_ref()
            .map(|record| record.result.selected_children.clone())
            .unwrap_or_else(|| generated_nodes.iter().map(workflow_node_id).collect());
        self.push_control_or_diverge(
            expand.id.as_str(),
            ControlNodeKind::Expand,
            execution,
            record,
            Some(selected),
            Some(format!(
                "expand replayed recorded nodes from {}",
                expand.source
            )),
        );
        Ok(())
    }

    fn replay_recorded_control(
        &self,
        node_id: &str,
        kind: ControlNodeKind,
        execution: &mut WorkflowExecution,
        fallback_children: Option<Vec<String>>,
        fallback_summary: Option<String>,
    ) -> Result<(), WorkflowReplayError> {
        let record = self.control_record(node_id, kind);
        self.push_control_or_diverge(
            node_id,
            kind,
            execution,
            record,
            fallback_children,
            fallback_summary,
        );
        Ok(())
    }

    fn control_record(&self, node_id: &str, kind: ControlNodeKind) -> Option<ReplayControlRecord> {
        self.control_records
            .get(&ReplayControlKey {
                trace_id: self.trace_id.clone(),
                node_id: node_id.to_string(),
                kind,
            })
            .cloned()
    }

    fn push_control_or_diverge(
        &self,
        node_id: &str,
        kind: ControlNodeKind,
        execution: &mut WorkflowExecution,
        record: Option<ReplayControlRecord>,
        fallback_children: Option<Vec<String>>,
        fallback_summary: Option<String>,
    ) {
        let Some(record) = record else {
            execution.mark_replay_diverged();
            execution.control_node_results.push(ControlNodeResult {
                node_id: node_id.to_string(),
                kind,
                status: WorkflowRunStatus::ReplayDiverged,
                selected_children: fallback_children.unwrap_or_default(),
                summary: fallback_summary
                    .or_else(|| Some("missing replay control record".to_string())),
            });
            return;
        };
        if record.result.status == WorkflowRunStatus::ReplayDiverged {
            execution.mark_replay_diverged();
        } else if record.result.status == WorkflowRunStatus::Failed {
            execution.mark_failed();
        }
        execution.control_node_results.push(record.result);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReplayLeafKey {
    trace_id: String,
    leaf_id: String,
    input_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReplayControlKey {
    trace_id: String,
    node_id: String,
    kind: ControlNodeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WorkflowReplayError {
    #[error(transparent)]
    Validation(#[from] WorkflowExecutionError),
    #[error("live replay requested for leaf `{leaf}`, but no live replay provider is configured")]
    LiveReplayUnavailable { leaf: String },
    #[error("failed to compute replay input hash: {reason}")]
    InputHash { reason: String },
}

pub fn compute_leaf_input_hash(
    spec: &WorkflowSpec,
    leaf: &LeafSpec,
    resolved_inputs: &BTreeMap<String, Option<String>>,
) -> Result<String, WorkflowReplayError> {
    let input = ReplayLeafInput {
        workflow_id: spec.id.as_deref(),
        workflow_goal: spec.goal.as_str(),
        leaf,
        resolved_inputs,
    };
    let bytes = serde_json::to_vec(&input).map_err(|error| WorkflowReplayError::InputHash {
        reason: error.to_string(),
    })?;
    let digest = Sha256::digest(bytes);
    Ok(format!("{digest:x}"))
}

#[derive(Serialize)]
struct ReplayLeafInput<'a> {
    workflow_id: Option<&'a str>,
    workflow_goal: &'a str,
    leaf: &'a LeafSpec,
    resolved_inputs: &'a BTreeMap<String, Option<String>>,
}

fn resolved_inputs_for_leaf(
    leaf: &LeafSpec,
    resolved_outputs: &BTreeMap<String, Option<String>>,
) -> BTreeMap<String, Option<String>> {
    leaf.depends_on_results
        .iter()
        .map(|dependency| {
            (
                dependency.clone(),
                resolved_outputs.get(dependency).cloned().unwrap_or(None),
            )
        })
        .collect()
}

fn branch_status(results: &[LeafResult]) -> WorkflowRunStatus {
    if results
        .iter()
        .any(|result| result.status == WorkflowRunStatus::ReplayDiverged)
    {
        WorkflowRunStatus::ReplayDiverged
    } else if results
        .iter()
        .any(|result| result.status != WorkflowRunStatus::Succeeded)
    {
        WorkflowRunStatus::Failed
    } else {
        WorkflowRunStatus::Succeeded
    }
}

fn select_nodes(nodes: &[WorkflowNode], selected: &[String]) -> Vec<WorkflowNode> {
    let by_id: BTreeMap<_, _> = nodes
        .iter()
        .map(|node| (workflow_node_id(node), node.clone()))
        .collect();
    selected
        .iter()
        .filter_map(|id| by_id.get(id).cloned())
        .collect()
}

fn workflow_node_id(node: &WorkflowNode) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentType, BudgetSpec, CondSpec, ControlNodeKind, ControlNodeResult, ExpandSpec, LeafSpec,
        ModelPolicy, PermissionSpec, TaskMode,
    };

    fn leaf(id: &str) -> LeafSpec {
        LeafSpec {
            id: id.to_string(),
            prompt: format!("run {id}"),
            agent_type: AgentType::General,
            mode: TaskMode::ReadOnly,
            isolation: crate::IsolationMode::Shared,
            file_scope: Vec::new(),
            depends_on_results: Vec::new(),
            budget: BudgetSpec::default(),
            permissions: PermissionSpec::default(),
            model_policy: ModelPolicy::default(),
        }
    }

    fn leaf_node(id: &str) -> WorkflowNode {
        WorkflowNode::Leaf(leaf(id))
    }

    fn workflow(nodes: Vec<WorkflowNode>) -> WorkflowSpec {
        WorkflowSpec {
            id: Some("wf".to_string()),
            goal: "replay safely".to_string(),
            description: None,
            budget: BudgetSpec::default(),
            permissions: PermissionSpec::default(),
            model_policy: ModelPolicy::default(),
            promotion_policy: crate::PromotionPolicy::default(),
            nodes,
        }
    }

    fn leaf_result(id: &str, output: &str) -> LeafResult {
        LeafResult {
            leaf_id: id.to_string(),
            task_id: id.to_string(),
            status: WorkflowRunStatus::Succeeded,
            usage: WorkflowUsage {
                input_tokens: 10,
                output_tokens: 5,
                cost_microusd: 2,
            },
            memo_usage: WorkflowMemoUsage::default(),
            output: Some(output.to_string()),
            artifacts: Vec::new(),
        }
    }

    fn leaf_record(spec: &WorkflowSpec, leaf: &LeafSpec, result: LeafResult) -> ReplayLeafRecord {
        ReplayLeafRecord {
            trace_id: "trace-1".to_string(),
            leaf_id: leaf.id.clone(),
            input_hash: compute_leaf_input_hash(spec, leaf, &BTreeMap::new()).unwrap(),
            result,
        }
    }

    fn control_record(
        id: &str,
        kind: ControlNodeKind,
        status: WorkflowRunStatus,
        selected_children: Vec<&str>,
    ) -> ReplayControlRecord {
        ReplayControlRecord {
            trace_id: "trace-1".to_string(),
            node_id: id.to_string(),
            kind,
            result: ControlNodeResult {
                node_id: id.to_string(),
                kind,
                status,
                selected_children: selected_children.into_iter().map(str::to_string).collect(),
                summary: Some("recorded".to_string()),
            },
            generated_nodes: Vec::new(),
        }
    }

    #[test]
    fn replay_uses_recorded_leaf_outputs_not_live_calls() {
        let scan = leaf("scan");
        let spec = workflow(vec![WorkflowNode::Leaf(scan.clone())]);
        let trace = WorkflowReplayTrace {
            trace_id: "trace-1".to_string(),
            leaf_records: vec![leaf_record(
                &spec,
                &scan,
                leaf_result("scan", "recorded output"),
            )],
            control_records: Vec::new(),
        };

        let execution = WorkflowReplayExecutor::new(trace)
            .run(&spec)
            .expect("replay should run");

        assert_eq!(execution.status, WorkflowRunStatus::Succeeded);
        assert_eq!(
            execution.leaf_results[0].output.as_deref(),
            Some("recorded output")
        );
        assert_eq!(execution.usage.cost_microusd, 2);
    }

    #[test]
    fn workflow_trace_can_replay_from_records() {
        let scan = leaf("scan");
        let summarize = leaf("summarize");
        let spec = workflow(vec![WorkflowNode::BranchSet(BranchSpec {
            id: "discover".to_string(),
            description: None,
            parallel: true,
            budget: BudgetSpec::default(),
            permissions: PermissionSpec::default(),
            model_policy: ModelPolicy::default(),
            children: vec![
                WorkflowNode::Leaf(scan.clone()),
                WorkflowNode::Leaf(summarize.clone()),
            ],
        })]);
        let trace = WorkflowReplayTrace {
            trace_id: "trace-1".to_string(),
            leaf_records: vec![
                leaf_record(&spec, &scan, leaf_result("scan", "scan ok")),
                leaf_record(&spec, &summarize, leaf_result("summarize", "summary ok")),
            ],
            control_records: vec![control_record(
                "discover",
                ControlNodeKind::BranchSet,
                WorkflowRunStatus::Succeeded,
                vec!["scan", "summarize"],
            )],
        };

        let execution = WorkflowReplayExecutor::new(trace)
            .run(&spec)
            .expect("replay should run");

        assert_eq!(execution.status, WorkflowRunStatus::Succeeded);
        assert_eq!(execution.leaf_results.len(), 2);
        assert_eq!(
            execution.branch_results[0].status,
            WorkflowRunStatus::Succeeded
        );
        assert_eq!(execution.branch_results[0].usage.cost_microusd, 4);
        assert_eq!(execution.usage.cost_microusd, 4);
    }

    #[test]
    fn workflow_replay_diverges_on_missing_leaf_record() {
        let spec = workflow(vec![leaf_node("scan")]);
        let trace = WorkflowReplayTrace {
            trace_id: "trace-1".to_string(),
            leaf_records: Vec::new(),
            control_records: Vec::new(),
        };

        let execution = WorkflowReplayExecutor::new(trace)
            .run(&spec)
            .expect("missing records should be reported as divergence");

        assert_eq!(execution.status, WorkflowRunStatus::ReplayDiverged);
        assert_eq!(
            execution.leaf_results[0].status,
            WorkflowRunStatus::ReplayDiverged
        );
        assert_eq!(execution.leaf_results[0].output, None);
    }

    #[test]
    fn live_replay_requires_explicit_opt_in() {
        let spec = workflow(vec![leaf_node("scan")]);
        let trace = WorkflowReplayTrace {
            trace_id: "trace-1".to_string(),
            leaf_records: Vec::new(),
            control_records: Vec::new(),
        };
        let err = WorkflowReplayExecutor::with_options(
            trace,
            ReplayOptions {
                allow_live_replay: true,
            },
        )
        .run(&spec)
        .expect_err("live replay cannot run without a configured provider");

        assert!(matches!(
            err,
            WorkflowReplayError::LiveReplayUnavailable { .. }
        ));
        assert!(!ReplayOptions::default().allow_live_replay);
    }

    #[test]
    fn leaf_input_hash_is_stable_across_object_key_order() {
        let mut downstream = leaf("summarize");
        downstream.depends_on_results = vec!["b".to_string(), "a".to_string()];
        let spec = workflow(vec![WorkflowNode::Leaf(downstream.clone())]);
        let mut left = BTreeMap::new();
        left.insert("a".to_string(), Some("one".to_string()));
        left.insert("b".to_string(), Some("two".to_string()));
        let mut right = BTreeMap::new();
        right.insert("b".to_string(), Some("two".to_string()));
        right.insert("a".to_string(), Some("one".to_string()));

        let left_hash = compute_leaf_input_hash(&spec, &downstream, &left).unwrap();
        let right_hash = compute_leaf_input_hash(&spec, &downstream, &right).unwrap();

        assert_eq!(left_hash, right_hash);
    }

    #[test]
    fn replay_control_records_drive_cond_expand_loop_until() {
        let patch = leaf("patch");
        let generated = leaf("generated-check");
        let spec = workflow(vec![
            WorkflowNode::Cond(CondSpec {
                id: "choose".to_string(),
                condition: "patch?".to_string(),
                then_nodes: vec![WorkflowNode::Leaf(patch.clone())],
                else_nodes: vec![leaf_node("report")],
            }),
            WorkflowNode::Expand(ExpandSpec {
                id: "split".to_string(),
                source: "choose".to_string(),
                max_children: None,
                template: None,
            }),
            WorkflowNode::LoopUntil(crate::LoopUntilSpec {
                id: "verify".to_string(),
                condition: "done".to_string(),
                max_iterations: Some(3),
                children: vec![leaf_node("unused-live-child")],
            }),
        ]);
        let mut expand_record = control_record(
            "split",
            ControlNodeKind::Expand,
            WorkflowRunStatus::Succeeded,
            vec!["generated-check"],
        );
        expand_record.generated_nodes = vec![WorkflowNode::Leaf(generated.clone())];
        let trace = WorkflowReplayTrace {
            trace_id: "trace-1".to_string(),
            leaf_records: vec![
                leaf_record(&spec, &patch, leaf_result("patch", "patched")),
                leaf_record(&spec, &generated, leaf_result("generated-check", "checked")),
            ],
            control_records: vec![
                control_record(
                    "choose",
                    ControlNodeKind::Cond,
                    WorkflowRunStatus::Succeeded,
                    vec!["patch"],
                ),
                expand_record,
                control_record(
                    "verify",
                    ControlNodeKind::LoopUntil,
                    WorkflowRunStatus::Succeeded,
                    Vec::new(),
                ),
            ],
        };

        let execution = WorkflowReplayExecutor::new(trace)
            .run(&spec)
            .expect("replay should run");

        assert_eq!(
            execution
                .leaf_results
                .iter()
                .map(|result| result.leaf_id.as_str())
                .collect::<Vec<_>>(),
            vec!["patch", "generated-check"]
        );
        assert_eq!(execution.status, WorkflowRunStatus::Succeeded);
    }
}
