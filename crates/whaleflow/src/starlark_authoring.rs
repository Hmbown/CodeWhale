use std::cell::{RefCell, RefMut};

use starlark::any::ProvidesStaticType;
use starlark::environment::{GlobalsBuilder, Module};
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::Value;
use starlark::values::list::UnpackList;
use starlark::values::none::NoneType;
use thiserror::Error;

use crate::{
    AgentType, BranchSpec, BudgetSpec, CondSpec, ExpandSpec, IsolationMode, LeafSpec, ModelPolicy,
    PermissionSpec, PromotionPolicy, ReduceSpec, SequenceSpec, TaskMode, TeacherReviewSpec,
    WorkflowNode, WorkflowSpec,
};

pub type StarlarkWorkflowResult<T> = std::result::Result<T, StarlarkWorkflowError>;

#[derive(Debug, Error)]
pub enum StarlarkWorkflowError {
    #[error("workflow source contains unsupported construct `{construct}`")]
    UnsupportedConstruct { construct: &'static str },
    #[error("workflow did not call workflow(...)")]
    MissingWorkflow,
    #[error("invalid workflow node: {0}")]
    InvalidNode(String),
    #[error("invalid {field} value `{value}`")]
    InvalidEnum { field: &'static str, value: String },
    #[error("starlark error: {0}")]
    Starlark(starlark::Error),
}

pub fn compile_starlark_workflow(
    identifier: &str,
    source: &str,
) -> StarlarkWorkflowResult<WorkflowSpec> {
    reject_unsupported_constructs(source)?;
    let mut dialect = Dialect::Extended.clone();
    dialect.enable_f_strings = true;
    let ast = AstModule::parse(identifier, source.to_string(), &dialect)
        .map_err(StarlarkWorkflowError::Starlark)?;
    let builder = RefCell::new(WorkflowBuilder::default());
    let globals = GlobalsBuilder::standard().with(workflow_builtins).build();
    let module = Module::new();
    {
        let mut eval = Evaluator::new(&module);
        eval.extra = Some(&builder);
        eval.eval_module(ast, &globals)
            .map_err(StarlarkWorkflowError::Starlark)?;
    }
    builder
        .into_inner()
        .workflow
        .ok_or(StarlarkWorkflowError::MissingWorkflow)
}

pub fn compile_starlark_workflow_with_repair(
    identifier: &str,
    source: &str,
) -> StarlarkWorkflowResult<WorkflowSpec> {
    match compile_starlark_workflow(identifier, source) {
        Ok(workflow) => Ok(workflow),
        Err(first_err) => {
            let repaired = repair_starlark_workflow_once(source);
            if repaired == source {
                Err(first_err)
            } else {
                compile_starlark_workflow(identifier, &repaired)
            }
        }
    }
}

pub fn repair_starlark_workflow_once(source: &str) -> String {
    source
        .replace("ctx.parallel(", "branch(")
        .replace("ctx.sequence(", "sequence(")
        .replace("ctx.loop_until(", "loop_until(")
        .replace("ctx.when(", "when(")
        .replace("ctx.expand(", "expand(")
        .replace("ctx.tournament(", "tournament(")
        .replace("ctx.teacher.review(", "teacher_review(")
}

fn reject_unsupported_constructs(source: &str) -> StarlarkWorkflowResult<()> {
    for (needle, construct) in [
        ("load(", "load"),
        ("import ", "import"),
        ("class ", "class"),
        ("while ", "while"),
        ("async ", "async"),
        ("await ", "await"),
        ("open(", "open"),
    ] {
        if source.contains(needle) {
            return Err(StarlarkWorkflowError::UnsupportedConstruct { construct });
        }
    }
    Ok(())
}

#[derive(Debug, Default, ProvidesStaticType)]
struct WorkflowBuilder {
    workflow: Option<WorkflowSpec>,
}

fn workflow_builder<'v, 'a>(eval: &Evaluator<'v, 'a, '_>) -> RefMut<'a, WorkflowBuilder> {
    #[expect(clippy::expect_used)]
    eval.extra
        .as_ref()
        .expect("workflow_builder requires Evaluator.extra to be populated")
        .downcast_ref::<RefCell<WorkflowBuilder>>()
        .expect("Evaluator.extra must contain a WorkflowBuilder")
        .borrow_mut()
}

fn encode_node(node: WorkflowNode) -> anyhow::Result<String> {
    serde_json::to_string(&node).map_err(Into::into)
}

fn decode_node(value: Value<'_>) -> anyhow::Result<WorkflowNode> {
    let raw = value.unpack_str().ok_or_else(|| {
        StarlarkWorkflowError::InvalidNode(format!(
            "expected node token string, got {}",
            value.get_type()
        ))
    })?;
    serde_json::from_str(raw).map_err(|err: serde_json::Error| {
        StarlarkWorkflowError::InvalidNode(err.to_string()).into()
    })
}

fn decode_nodes(values: UnpackList<Value<'_>>) -> anyhow::Result<Vec<WorkflowNode>> {
    values.items.into_iter().map(decode_node).collect()
}

fn decode_strings(values: Option<UnpackList<Value<'_>>>) -> anyhow::Result<Vec<String>> {
    let Some(values) = values else {
        return Ok(Vec::new());
    };
    values
        .items
        .into_iter()
        .map(|value| {
            value
                .unpack_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("expected string, got {}", value.get_type()))
        })
        .collect()
}

fn agent_type(raw: Option<&str>) -> anyhow::Result<AgentType> {
    match raw.unwrap_or("general") {
        "general" => Ok(AgentType::General),
        "explore" | "explorer" => Ok(AgentType::Explore),
        "plan" => Ok(AgentType::Plan),
        "review" => Ok(AgentType::Review),
        "implementer" | "implement" => Ok(AgentType::Implementer),
        "verifier" | "verify" => Ok(AgentType::Verifier),
        "tool_agent" | "tool" => Ok(AgentType::ToolAgent),
        value => Err(StarlarkWorkflowError::InvalidEnum {
            field: "agent_type",
            value: value.to_string(),
        }
        .into()),
    }
}

fn task_mode(raw: Option<&str>) -> anyhow::Result<TaskMode> {
    match raw.unwrap_or("read_only") {
        "read_only" => Ok(TaskMode::ReadOnly),
        "read_write" => Ok(TaskMode::ReadWrite),
        value => Err(StarlarkWorkflowError::InvalidEnum {
            field: "mode",
            value: value.to_string(),
        }
        .into()),
    }
}

fn isolation_mode(raw: Option<&str>) -> anyhow::Result<IsolationMode> {
    match raw.unwrap_or("shared") {
        "shared" => Ok(IsolationMode::Shared),
        "worktree" => Ok(IsolationMode::Worktree),
        value => Err(StarlarkWorkflowError::InvalidEnum {
            field: "isolation",
            value: value.to_string(),
        }
        .into()),
    }
}

fn leaf_spec(
    id: &str,
    prompt: &str,
    agent_type: Option<&str>,
    mode: Option<&str>,
    isolation: Option<&str>,
    file_scope: Option<UnpackList<Value<'_>>>,
    depends_on_results: Option<UnpackList<Value<'_>>>,
) -> anyhow::Result<LeafSpec> {
    Ok(LeafSpec {
        id: id.to_string(),
        prompt: prompt.to_string(),
        agent_type: self::agent_type(agent_type)?,
        mode: task_mode(mode)?,
        isolation: isolation_mode(isolation)?,
        file_scope: decode_strings(file_scope)?,
        depends_on_results: decode_strings(depends_on_results)?,
        budget: BudgetSpec::default(),
        permissions: PermissionSpec::default(),
        model_policy: ModelPolicy::default(),
    })
}

#[starlark_module]
fn workflow_builtins(builder: &mut GlobalsBuilder) {
    fn workflow<'v>(
        goal: &'v str,
        nodes: UnpackList<Value<'v>>,
        id: Option<&'v str>,
        description: Option<&'v str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<NoneType> {
        let spec = WorkflowSpec {
            id: id.map(str::to_string),
            goal: goal.to_string(),
            description: description.map(str::to_string),
            budget: BudgetSpec::default(),
            permissions: PermissionSpec::default(),
            model_policy: ModelPolicy::default(),
            promotion_policy: PromotionPolicy::default(),
            nodes: decode_nodes(nodes)?,
        };
        workflow_builder(eval).workflow = Some(spec);
        Ok(NoneType)
    }

    fn agent<'v>(
        id: &'v str,
        prompt: &'v str,
        agent_type: Option<&'v str>,
        mode: Option<&'v str>,
        isolation: Option<&'v str>,
        file_scope: Option<UnpackList<Value<'v>>>,
        depends_on_results: Option<UnpackList<Value<'v>>>,
    ) -> anyhow::Result<String> {
        encode_node(WorkflowNode::Leaf(leaf_spec(
            id,
            prompt,
            agent_type,
            mode,
            isolation,
            file_scope,
            depends_on_results,
        )?))
    }

    fn test<'v>(
        id: &'v str,
        command: &'v str,
        file_scope: Option<UnpackList<Value<'v>>>,
    ) -> anyhow::Result<String> {
        encode_node(WorkflowNode::Leaf(leaf_spec(
            id,
            &format!("Run test command: {command}"),
            Some("verifier"),
            Some("read_only"),
            Some("shared"),
            file_scope,
            None,
        )?))
    }

    fn search<'v>(
        id: &'v str,
        query: &'v str,
        file_scope: Option<UnpackList<Value<'v>>>,
    ) -> anyhow::Result<String> {
        encode_node(WorkflowNode::Leaf(leaf_spec(
            id,
            &format!("Search codebase: {query}"),
            Some("tool"),
            Some("read_only"),
            Some("shared"),
            file_scope,
            None,
        )?))
    }

    fn shell<'v>(
        id: &'v str,
        command: &'v str,
        file_scope: Option<UnpackList<Value<'v>>>,
    ) -> anyhow::Result<String> {
        encode_node(WorkflowNode::Leaf(leaf_spec(
            id,
            &format!("Run shell command: {command}"),
            Some("tool"),
            Some("read_only"),
            Some("shared"),
            file_scope,
            None,
        )?))
    }

    fn branch<'v>(
        id: &'v str,
        children: UnpackList<Value<'v>>,
        parallel: Option<bool>,
    ) -> anyhow::Result<String> {
        encode_node(WorkflowNode::BranchSet(BranchSpec {
            id: id.to_string(),
            description: None,
            parallel: parallel.unwrap_or(true),
            budget: BudgetSpec::default(),
            permissions: PermissionSpec::default(),
            model_policy: ModelPolicy::default(),
            children: decode_nodes(children)?,
        }))
    }

    fn sequence<'v>(id: &'v str, children: UnpackList<Value<'v>>) -> anyhow::Result<String> {
        encode_node(WorkflowNode::Sequence(SequenceSpec {
            id: id.to_string(),
            children: decode_nodes(children)?,
        }))
    }

    fn reduce<'v>(
        id: &'v str,
        prompt: &'v str,
        inputs: Option<UnpackList<Value<'v>>>,
    ) -> anyhow::Result<String> {
        encode_node(WorkflowNode::Reduce(ReduceSpec {
            id: id.to_string(),
            inputs: decode_strings(inputs)?,
            prompt: prompt.to_string(),
            model_policy: ModelPolicy::default(),
        }))
    }

    fn teacher_review<'v>(
        id: &'v str,
        candidates: Option<UnpackList<Value<'v>>>,
    ) -> anyhow::Result<String> {
        encode_node(WorkflowNode::TeacherReview(TeacherReviewSpec {
            id: id.to_string(),
            candidates: decode_strings(candidates)?,
            promotion_policy: PromotionPolicy::default(),
        }))
    }

    fn tournament<'v>(
        id: &'v str,
        candidates: Option<UnpackList<Value<'v>>>,
    ) -> anyhow::Result<String> {
        encode_node(WorkflowNode::TeacherReview(TeacherReviewSpec {
            id: id.to_string(),
            candidates: decode_strings(candidates)?,
            promotion_policy: PromotionPolicy::default(),
        }))
    }

    fn loop_until<'v>(
        id: &'v str,
        condition: &'v str,
        children: UnpackList<Value<'v>>,
        max_iterations: Option<u32>,
    ) -> anyhow::Result<String> {
        encode_node(WorkflowNode::LoopUntil(crate::LoopUntilSpec {
            id: id.to_string(),
            condition: condition.to_string(),
            max_iterations,
            children: decode_nodes(children)?,
        }))
    }

    fn r#when<'v>(
        id: &'v str,
        condition: &'v str,
        then_nodes: UnpackList<Value<'v>>,
        else_nodes: Option<UnpackList<Value<'v>>>,
    ) -> anyhow::Result<String> {
        encode_node(WorkflowNode::Cond(CondSpec {
            id: id.to_string(),
            condition: condition.to_string(),
            then_nodes: decode_nodes(then_nodes)?,
            else_nodes: else_nodes
                .map(decode_nodes)
                .transpose()?
                .unwrap_or_default(),
        }))
    }

    fn expand<'v>(
        id: &'v str,
        source: &'v str,
        max_children: Option<usize>,
    ) -> anyhow::Result<String> {
        encode_node(WorkflowNode::Expand(ExpandSpec {
            id: id.to_string(),
            source: source.to_string(),
            max_children,
            template: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentType, ControlNodeKind, MockWorkflowExecutor, WorkflowRunStatus};

    #[test]
    fn starlark_compiles_to_ir() {
        let source = include_str!("../../../workflows/rlm_cache_change.star");
        let workflow = compile_starlark_workflow("rlm_cache_change.star", source)
            .expect("example should compile");

        assert_eq!(workflow.id.as_deref(), Some("rlm-cache-change"));
        assert_eq!(workflow.nodes.len(), 2);
        let WorkflowNode::BranchSet(branch) = &workflow.nodes[0] else {
            panic!("first node should be a branch set");
        };
        assert_eq!(branch.id, "candidate-branches");
        assert!(branch.parallel);
        let WorkflowNode::Leaf(leaf) = &branch.children[0] else {
            panic!("first branch child should be a leaf");
        };
        assert_eq!(leaf.agent_type, AgentType::ToolAgent);
    }

    #[test]
    fn rlm_cache_change_workflow_runs_with_mock_provider() {
        let source = include_str!("../../../workflows/rlm_cache_change.star");
        let workflow = compile_starlark_workflow("rlm_cache_change.star", source)
            .expect("example should compile");
        let mut executor = MockWorkflowExecutor::new()
            .with_predicate_results("implement-until-tests-pass", vec![true]);

        let execution = executor
            .run(&workflow)
            .expect("dogfood workflow should run with mock leaves");

        assert_eq!(execution.status, WorkflowRunStatus::Succeeded);
        assert!(
            execution
                .leaf_results
                .iter()
                .any(|result| result.leaf_id == "regression-tests")
        );
        assert!(
            execution
                .control_node_results
                .iter()
                .any(|result| result.node_id == "teacher-review")
        );
    }

    #[test]
    fn starlark_repair_loop() {
        let source = r#"
workflow(
    id = "repair-demo",
    goal = "repair ctx aliases",
    nodes = [
        ctx.parallel(id = "discover", children = [
            agent(id = "scan", prompt = "scan repo"),
        ]),
    ],
)
"#;

        let workflow = compile_starlark_workflow_with_repair("repair.star", source)
            .expect("repair should rewrite ctx.parallel");

        assert_eq!(workflow.id.as_deref(), Some("repair-demo"));
        assert!(matches!(workflow.nodes[0], WorkflowNode::BranchSet(_)));
    }

    #[test]
    fn starlark_generated_workflow_repairs_then_runs() {
        let source = r#"
workflow(
    id = "generated-repair-run",
    goal = "repair generated workflow aliases",
    nodes = [
        ctx.parallel(id = "discover", children = [
            agent(id = "scan", prompt = "scan repo"),
        ]),
        ctx.loop_until(
            id = "verify",
            condition = "checks pass",
            max_iterations = 1,
            children = [
                test(id = "run-tests", command = "cargo test -p codewhale-whaleflow --locked"),
            ],
        ),
    ],
)
"#;
        let workflow = compile_starlark_workflow_with_repair("generated.star", source)
            .expect("repair should produce runnable IR");
        let mut executor = MockWorkflowExecutor::new().with_predicate_results("verify", vec![true]);

        let execution = executor
            .run(&workflow)
            .expect("repaired workflow should run with mock leaves");

        assert_eq!(execution.status, WorkflowRunStatus::Succeeded);
        assert_eq!(execution.leaf_results.len(), 2);
    }

    #[test]
    fn invalid_workflow_rejected() {
        let source = r#"
load("@stdlib//fs.star", "open")
workflow(goal = "bad", nodes = [])
"#;

        let err = compile_starlark_workflow("invalid.star", source)
            .expect_err("imports should be rejected before eval");

        assert!(matches!(
            err,
            StarlarkWorkflowError::UnsupportedConstruct { construct: "load" }
        ));
    }

    #[test]
    fn issue_fix_tournament_example_compiles() {
        let source = include_str!("../../../workflows/issue_fix_tournament.star");
        let workflow = compile_starlark_workflow("issue_fix_tournament.star", source)
            .expect("example should compile");

        let WorkflowNode::Sequence(sequence) = &workflow.nodes[1] else {
            panic!("second node should be a sequence");
        };
        assert!(sequence.children.iter().any(|node| matches!(
            node,
            WorkflowNode::TeacherReview(review)
                if review.id == "select-fix"
        )));
        assert_eq!(
            ControlNodeKind::TeacherReview,
            ControlNodeKind::TeacherReview
        );
    }
}
