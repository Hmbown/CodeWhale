use serde::Deserialize;
use thiserror::Error;

use crate::{
    BranchSpec, BudgetSpec, CondSpec, ExpandSpec, LeafSpec, LoopUntilSpec, ModelPolicy,
    PermissionSpec, PromotionPolicy, ReduceSpec, SequenceSpec, TeacherReviewSpec, WorkflowNode,
    WorkflowSpec, validate_workflow_nodes,
};

pub type JavascriptWorkflowResult<T> = std::result::Result<T, JavascriptWorkflowError>;

#[derive(Debug, Error)]
pub enum JavascriptWorkflowError {
    #[error("workflow source contains unsupported construct `{construct}`")]
    UnsupportedConstruct { construct: &'static str },
    #[error("workflow source did not call workflow({{...}})")]
    MissingWorkflowCall,
    #[error("workflow({{...}}) object could not be extracted: {0}")]
    InvalidWorkflowObject(String),
    #[error("invalid workflow JSON object: {0}")]
    InvalidJson(serde_json::Error),
    #[error("invalid workflow node: {0}")]
    InvalidNode(String),
}

pub fn compile_javascript_workflow(
    identifier: &str,
    source: &str,
) -> JavascriptWorkflowResult<WorkflowSpec> {
    compile_js_like_workflow(identifier, source)
}

pub fn compile_typescript_workflow(
    identifier: &str,
    source: &str,
) -> JavascriptWorkflowResult<WorkflowSpec> {
    compile_js_like_workflow(identifier, source)
}

fn compile_js_like_workflow(
    _identifier: &str,
    source: &str,
) -> JavascriptWorkflowResult<WorkflowSpec> {
    reject_unsupported_constructs(source)?;
    let object = extract_workflow_object(source)?;
    let authored = serde_json::from_str::<JsWorkflowSpec>(object)
        .map_err(JavascriptWorkflowError::InvalidJson)?;
    let mut workflow = authored.into_workflow();
    normalize_leaf_profiles(&mut workflow.nodes);
    if workflow.goal.trim().is_empty() {
        return Err(JavascriptWorkflowError::InvalidNode(
            "workflow goal cannot be empty".to_string(),
        ));
    }
    validate_workflow_nodes(&workflow.nodes)
        .map_err(|error| JavascriptWorkflowError::InvalidNode(error.to_string()))?;
    Ok(workflow)
}

// Role/profile names are case-insensitive roster keys; the IR stores the
// canonical lowercase form. Invalid tokens are left as-is so validation
// reports them.
fn normalize_leaf_profiles(nodes: &mut [WorkflowNode]) {
    for node in nodes {
        match node {
            WorkflowNode::Leaf(spec) => {
                if let Some(role) = spec.role.as_mut() {
                    *role = role.trim().to_lowercase();
                }
                if let Some(profile) = spec.profile.as_mut() {
                    *profile = profile.trim().to_lowercase();
                }
            }
            WorkflowNode::BranchSet(spec) => normalize_leaf_profiles(&mut spec.children),
            WorkflowNode::Sequence(spec) => normalize_leaf_profiles(&mut spec.children),
            WorkflowNode::LoopUntil(spec) => normalize_leaf_profiles(&mut spec.children),
            WorkflowNode::Cond(spec) => {
                normalize_leaf_profiles(&mut spec.then_nodes);
                normalize_leaf_profiles(&mut spec.else_nodes);
            }
            WorkflowNode::Expand(spec) => {
                if let Some(template) = spec.template.as_deref_mut() {
                    normalize_leaf_profiles(std::slice::from_mut(template));
                }
            }
            WorkflowNode::Reduce(_) | WorkflowNode::TeacherReview(_) => {}
        }
    }
}

fn reject_unsupported_constructs(source: &str) -> JavascriptWorkflowResult<()> {
    for (needle, construct) in [
        ("import ", "import"),
        ("import(", "dynamic import"),
        ("require(", "require"),
        ("fetch(", "fetch"),
        ("XMLHttpRequest", "XMLHttpRequest"),
        ("WebSocket", "WebSocket"),
        ("process.", "process"),
        ("Deno.", "Deno"),
        ("Bun.", "Bun"),
        ("child_process", "child_process"),
        ("exec(", "exec"),
        ("spawn(", "spawn"),
        ("open(", "open"),
        ("readFile", "readFile"),
        ("writeFile", "writeFile"),
        ("async ", "async"),
        ("await ", "await"),
        ("eval(", "eval"),
        ("new Function", "Function"),
    ] {
        if source.contains(needle) {
            return Err(JavascriptWorkflowError::UnsupportedConstruct { construct });
        }
    }
    Ok(())
}

fn extract_workflow_object(source: &str) -> JavascriptWorkflowResult<&str> {
    let workflow_pos = source
        .find("workflow")
        .ok_or(JavascriptWorkflowError::MissingWorkflowCall)?;
    let open_paren_rel = source[workflow_pos..]
        .find('(')
        .ok_or(JavascriptWorkflowError::MissingWorkflowCall)?;
    let open_paren = workflow_pos + open_paren_rel;
    let object_start = source[open_paren + 1..]
        .char_indices()
        .find_map(|(idx, ch)| {
            if ch.is_whitespace() {
                None
            } else {
                Some((open_paren + 1 + idx, ch))
            }
        })
        .ok_or(JavascriptWorkflowError::MissingWorkflowCall)?;
    if object_start.1 != '{' {
        return Err(JavascriptWorkflowError::InvalidWorkflowObject(
            "workflow(...) must receive a JSON-compatible object literal".to_string(),
        ));
    }

    let mut depth = 0usize;
    let mut in_string: Option<char> = None;
    let mut escape = false;
    for (idx, ch) in source[object_start.0..].char_indices() {
        let absolute = object_start.0 + idx;
        if let Some(quote) = in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == quote {
                in_string = None;
            }
            continue;
        }

        match ch {
            '"' | '\'' | '`' => in_string = Some(ch),
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    JavascriptWorkflowError::InvalidWorkflowObject(
                        "unbalanced closing brace".to_string(),
                    )
                })?;
                if depth == 0 {
                    return Ok(&source[object_start.0..=absolute]);
                }
            }
            _ => {}
        }
    }

    Err(JavascriptWorkflowError::InvalidWorkflowObject(
        "missing closing brace for workflow object".to_string(),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsWorkflowSpec {
    #[serde(default)]
    id: Option<String>,
    goal: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    budget: BudgetSpec,
    #[serde(default)]
    permissions: PermissionSpec,
    #[serde(default)]
    model_policy: ModelPolicy,
    #[serde(default)]
    promotion_policy: PromotionPolicy,
    #[serde(default)]
    nodes: Vec<JsWorkflowNode>,
}

impl JsWorkflowSpec {
    fn into_workflow(self) -> WorkflowSpec {
        WorkflowSpec {
            id: self.id,
            goal: self.goal,
            description: self.description,
            budget: self.budget,
            permissions: self.permissions,
            model_policy: self.model_policy,
            promotion_policy: self.promotion_policy,
            nodes: self
                .nodes
                .into_iter()
                .map(JsWorkflowNode::into_node)
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum JsWorkflowNode {
    Raw(WorkflowNode),
    Agent(JsAgentNode),
    Branch(JsBranchNode),
    Sequence(JsSequenceNode),
    Reduce(JsReduceNode),
    TeacherReview(JsTeacherReviewNode),
    LoopUntil(JsLoopUntilNode),
    Cond(JsCondNode),
    Expand(JsExpandNode),
}

impl JsWorkflowNode {
    fn into_node(self) -> WorkflowNode {
        match self {
            Self::Raw(node) => node,
            Self::Agent(node) => WorkflowNode::Leaf(node.agent),
            Self::Branch(node) => WorkflowNode::BranchSet(node.branch.into_branch()),
            Self::Sequence(node) => WorkflowNode::Sequence(node.sequence.into_sequence()),
            Self::Reduce(node) => WorkflowNode::Reduce(node.reduce),
            Self::TeacherReview(node) => WorkflowNode::TeacherReview(node.teacher_review),
            Self::LoopUntil(node) => WorkflowNode::LoopUntil(node.loop_until.into_loop_until()),
            Self::Cond(node) => WorkflowNode::Cond(node.cond.into_cond()),
            Self::Expand(node) => WorkflowNode::Expand(node.expand.into_expand()),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsAgentNode {
    agent: LeafSpec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsBranchNode {
    branch: JsBranchSpec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsBranchSpec {
    id: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "default_true")]
    parallel: bool,
    #[serde(default)]
    budget: BudgetSpec,
    #[serde(default)]
    permissions: PermissionSpec,
    #[serde(default)]
    model_policy: ModelPolicy,
    #[serde(default)]
    children: Vec<JsWorkflowNode>,
}

impl JsBranchSpec {
    fn into_branch(self) -> BranchSpec {
        BranchSpec {
            id: self.id,
            description: self.description,
            parallel: self.parallel,
            budget: self.budget,
            permissions: self.permissions,
            model_policy: self.model_policy,
            children: self
                .children
                .into_iter()
                .map(JsWorkflowNode::into_node)
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsSequenceNode {
    sequence: JsSequenceSpec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsSequenceSpec {
    id: String,
    #[serde(default)]
    children: Vec<JsWorkflowNode>,
}

impl JsSequenceSpec {
    fn into_sequence(self) -> SequenceSpec {
        SequenceSpec {
            id: self.id,
            children: self
                .children
                .into_iter()
                .map(JsWorkflowNode::into_node)
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsReduceNode {
    reduce: ReduceSpec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsTeacherReviewNode {
    teacher_review: TeacherReviewSpec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsLoopUntilNode {
    loop_until: JsLoopUntilSpec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsLoopUntilSpec {
    id: String,
    condition: String,
    #[serde(default)]
    max_iterations: Option<u32>,
    #[serde(default)]
    children: Vec<JsWorkflowNode>,
}

impl JsLoopUntilSpec {
    fn into_loop_until(self) -> LoopUntilSpec {
        LoopUntilSpec {
            id: self.id,
            condition: self.condition,
            max_iterations: self.max_iterations,
            children: self
                .children
                .into_iter()
                .map(JsWorkflowNode::into_node)
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsCondNode {
    cond: JsCondSpec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsCondSpec {
    id: String,
    condition: String,
    #[serde(default)]
    then_nodes: Vec<JsWorkflowNode>,
    #[serde(default)]
    else_nodes: Vec<JsWorkflowNode>,
}

impl JsCondSpec {
    fn into_cond(self) -> CondSpec {
        CondSpec {
            id: self.id,
            condition: self.condition,
            then_nodes: self
                .then_nodes
                .into_iter()
                .map(JsWorkflowNode::into_node)
                .collect(),
            else_nodes: self
                .else_nodes
                .into_iter()
                .map(JsWorkflowNode::into_node)
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsExpandNode {
    expand: JsExpandSpec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsExpandSpec {
    id: String,
    source: String,
    #[serde(default)]
    max_children: Option<usize>,
    #[serde(default)]
    template: Option<Box<JsWorkflowNode>>,
}

impl JsExpandSpec {
    fn into_expand(self) -> ExpandSpec {
        ExpandSpec {
            id: self.id,
            source: self.source,
            max_children: self.max_children,
            template: self.template.map(|node| Box::new(node.into_node())),
        }
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentType, WorkflowReplayExecutor};

    #[test]
    fn javascript_workflow_compiles_branch_reduce_to_ir() {
        let source = r#"
export default workflow({
  "id": "js-audit",
  "goal": "Audit a change with parallel agents",
  "nodes": [
    {
      "branch": {
        "id": "parallel-audit",
        "children": [
          {
            "agent": {
              "id": "docs-audit",
              "prompt": "Inspect docs for missing updates",
              "agent_type": "review",
              "file_scope": ["docs"]
            }
          },
          {
            "agent": {
              "id": "tests-audit",
              "prompt": "Inspect targeted tests",
              "agent_type": "verifier",
              "budget": { "max_steps": 4 }
            }
          }
        ]
      }
    },
    {
      "reduce": {
        "id": "synthesize",
        "inputs": ["docs-audit", "tests-audit"],
        "prompt": "Merge the branch findings"
      }
    }
  ]
});
"#;

        let workflow =
            compile_javascript_workflow("audit.workflow.js", source).expect("compile JS workflow");

        assert_eq!(workflow.id.as_deref(), Some("js-audit"));
        assert_eq!(workflow.nodes.len(), 2);
        let WorkflowNode::BranchSet(branch) = &workflow.nodes[0] else {
            panic!("first node should be a branch");
        };
        assert!(branch.parallel);
        assert_eq!(branch.children.len(), 2);
        let WorkflowNode::Leaf(leaf) = &branch.children[1] else {
            panic!("second branch child should be a leaf");
        };
        assert_eq!(leaf.agent_type, AgentType::Verifier);
        assert_eq!(leaf.budget.max_steps, Some(4));
        assert!(matches!(workflow.nodes[1], WorkflowNode::Reduce(_)));
    }

    #[test]
    fn typescript_workflow_allows_satisfies_suffix_without_executing_js() {
        let source = r#"
export default workflow({
  "goal": "TS authored workflow",
  "nodes": [
    { "agent": { "id": "scan", "prompt": "scan safely" } }
  ]
} satisfies WorkflowSpec);
"#;

        let workflow =
            compile_typescript_workflow("scan.workflow.ts", source).expect("compile TS workflow");

        assert_eq!(workflow.goal, "TS authored workflow");
        assert_eq!(workflow.nodes.len(), 1);
    }

    #[test]
    fn javascript_workflow_accepts_and_normalizes_agent_profile() {
        let source = r#"
workflow({
  "goal": "profile routing",
  "nodes": [
    { "agent": { "id": "review", "prompt": "review the diff", "profile": " Reviewer " } },
    { "agent": { "id": "scan", "prompt": "scan safely" } }
  ]
});
"#;

        let workflow = compile_javascript_workflow("profile.workflow.js", source)
            .expect("profile-carrying workflow should compile");

        let WorkflowNode::Leaf(review) = &workflow.nodes[0] else {
            panic!("first node should be a leaf");
        };
        assert_eq!(review.profile.as_deref(), Some("reviewer"));
        let WorkflowNode::Leaf(scan) = &workflow.nodes[1] else {
            panic!("second node should be a leaf");
        };
        assert_eq!(scan.profile, None);
    }

    #[test]
    fn javascript_workflow_accepts_and_normalizes_agent_role() {
        let source = r#"
workflow({
  "goal": "role routing",
  "nodes": [
    { "agent": { "id": "scout-issue", "prompt": "Investigate #4090. Read-only.", "role": " Scout " } },
    { "agent": { "id": "fix-it", "prompt": "Apply minimal fix.", "role": "implementer" } }
  ]
});
"#;

        let workflow = compile_javascript_workflow("role.workflow.js", source)
            .expect("role-carrying workflow should compile");

        let WorkflowNode::Leaf(scout) = &workflow.nodes[0] else {
            panic!("first node should be a leaf");
        };
        assert_eq!(scout.role.as_deref(), Some("scout"));
        assert_eq!(scout.profile, None);
        // Provider/model are not required identity fields on role steps.
        assert_eq!(scout.model_policy.provider, None);
        assert_eq!(scout.model_policy.model, None);

        let WorkflowNode::Leaf(fix) = &workflow.nodes[1] else {
            panic!("second node should be a leaf");
        };
        assert_eq!(fix.role.as_deref(), Some("implementer"));
    }

    #[test]
    fn javascript_workflow_rejects_invalid_agent_profiles() {
        for bad in [r#""""#, r#""has space""#, r#""quote\"y""#, r#""a=b""#] {
            let source = format!(
                r#"
workflow({{
  "goal": "bad profile",
  "nodes": [
    {{ "agent": {{ "id": "scan", "prompt": "scan safely", "profile": {bad} }} }}
  ]
}});
"#
            );

            let err = compile_javascript_workflow("bad-profile.workflow.js", &source)
                .expect_err("invalid profile should be rejected");

            assert!(
                matches!(err, JavascriptWorkflowError::InvalidNode(_)),
                "profile {bad} should fail as an invalid node, got {err:?}"
            );
            assert!(err.to_string().contains("profile"));
        }
    }

    #[test]
    fn javascript_workflow_rejects_runtime_effects() {
        let source = r#"
import fs from "fs";
workflow({ "goal": "bad", "nodes": [] });
"#;

        let err = compile_javascript_workflow("bad.workflow.js", source)
            .expect_err("imports must be rejected");

        assert!(matches!(
            err,
            JavascriptWorkflowError::UnsupportedConstruct {
                construct: "import"
            }
        ));
    }

    #[test]
    fn javascript_workflow_rejects_unknown_result_reference() {
        let source = r#"
workflow({
  "goal": "bad dependency",
  "nodes": [
    {
      "agent": {
        "id": "scan",
        "prompt": "scan safely",
        "depends_on_results": ["missing"]
      }
    }
  ]
});
"#;

        let err = compile_javascript_workflow("bad-reference.workflow.js", source)
            .expect_err("validation must reject unknown result references");

        assert!(matches!(err, JavascriptWorkflowError::InvalidNode(_)));
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn javascript_example_compiles_and_replays_with_mock_trace() {
        let source = include_str!("../../../workflows/issue_audit.workflow.js");
        let workflow =
            compile_javascript_workflow("issue_audit.workflow.js", source).expect("compile");
        let trace = crate::WorkflowReplayTrace {
            trace_id: "empty".to_string(),
            leaf_records: Vec::new(),
            control_records: Vec::new(),
        };

        let replayed = WorkflowReplayExecutor::new(trace)
            .run(&workflow)
            .expect("replay executor should accept validated JS IR");

        assert_eq!(replayed.status, crate::WorkflowRunStatus::ReplayDiverged);
    }
}
