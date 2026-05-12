//! # Protocol — Upgrade Protocol types and validation
//!
//! Ported from triadmind-core TypeScript:
//! - `protocol.ts` — data types, validation
//! - `protocolRightBranch.ts` — schema definitions, category maps
//!
//! @LeftBranch: assert_protocol_shape, read_triad_map, parse_node_ref
//! @RightBranch: TriadNodeDefinition, UpgradeProtocol, TriadOp, TriadCategory

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

// ── Core Types ──────────────────────────────────────────────────────

/// The three operation types allowed in a TriadMind upgrade protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriadOp {
    /// Reuse an existing topology node without modification.
    Reuse,
    /// Modify an existing node's inputs/outputs/responsibility boundary.
    Modify,
    /// Create a new child node under an existing parent.
    CreateChild,
}

impl TriadOp {
    pub fn as_str(&self) -> &'static str {
        match self {
            TriadOp::Reuse => "reuse",
            TriadOp::Modify => "modify",
            TriadOp::CreateChild => "create_child",
        }
    }
}

/// Architecture category for a topology node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TriadCategory {
    Known(String),
}

impl TriadCategory {
    pub fn as_str(&self) -> &str {
        match self {
            TriadCategory::Known(s) => s.as_str(),
        }
    }
}

impl Default for TriadCategory {
    fn default() -> Self {
        TriadCategory::Known("core".to_string())
    }
}

impl From<&str> for TriadCategory {
    fn from(s: &str) -> Self {
        TriadCategory::Known(s.to_string())
    }
}

/// The fission triple — the core architecture description of a node.
///
/// Follows the TriadMind vertex method:
/// - `problem`: what responsibility this node owns
/// - `demand`: what inputs it requires (method parameters / dependencies)
/// - `answer`: what outputs it produces (return types / side effects)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriadFission {
    pub problem: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub demand: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answer: Vec<String>,
}

/// Lifecycle stage of a topology node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriadLifecycle {
    Existing,
    New,
    Modified,
    Reused,
    Proposed,
}

/// A parsed reference to a topology node.
#[derive(Debug, Clone)]
pub struct ParsedNodeRef {
    pub raw_node_id: String,
    pub normalized_node_id: String,
    pub category: TriadCategory,
    pub class_name: String,
    pub method_name: String,
}

/// A single topology node definition in the triad map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriadNodeDefinition {
    #[serde(rename = "nodeId")]
    pub node_id: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(rename = "sourcePath", default)]
    pub source_path: Option<String>,
    #[serde(default)]
    pub lifecycle: Option<TriadLifecycle>,
    #[serde(default)]
    pub fission: Option<TriadFission>,
}

/// A single action in an upgrade protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolAction {
    pub op: TriadOp,
    /// Target node id for `reuse` or `modify`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    /// Parent node id for `create_child`.
    #[serde(rename = "parentNodeId", default, skip_serializing_if = "Option::is_none")]
    pub parent_node_id: Option<String>,
    /// New node definition for `create_child`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<TriadNodeDefinition>,
    /// Fission override for `modify`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fission: Option<TriadFission>,
    /// Reused node ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reuse: Vec<String>,
    /// Rationale for this action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Confidence score (0.0–1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

/// Triadization focus reference trait implemented by macro/meso/micro splits.
pub trait TriadizationFocusReference {
    fn triadization_focus(&self) -> &str;
    fn recommended_operation(&self) -> &str;
}

/// A macro-split segment of an upgrade protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroSplit {
    /// The anchor node for this split level.
    #[serde(rename = "anchorNodeId")]
    pub anchor_node_id: String,
    /// Left branch: dynamic execution nodes.
    #[serde(rename = "leftBranch", default)]
    pub left_branch: Vec<String>,
    /// Right branch: static constraint nodes.
    #[serde(rename = "rightBranch", default)]
    pub right_branch: Vec<String>,
    /// The recommended operation for the triadization focus.
    #[serde(rename = "recommendedOperation", default, skip_serializing_if = "Option::is_none")]
    pub recommended_operation: Option<String>,
    /// The vertex goal description.
    #[serde(rename = "vertexGoal", default, skip_serializing_if = "Option::is_none")]
    pub vertex_goal: Option<String>,
}

impl TriadizationFocusReference for MacroSplit {
    fn triadization_focus(&self) -> &str {
        &self.anchor_node_id
    }
    fn recommended_operation(&self) -> &str {
        self.recommended_operation.as_deref().unwrap_or("split")
    }
}

/// A meso-split segment — class-level decomposition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MesoSplit {
    #[serde(rename = "anchorNodeId")]
    pub anchor_node_id: String,
    #[serde(rename = "recommendedOperation", default)]
    pub recommended_operation: String,
    #[serde(default)]
    pub modules: Vec<MesoModule>,
}

impl TriadizationFocusReference for MesoSplit {
    fn triadization_focus(&self) -> &str {
        &self.anchor_node_id
    }
    fn recommended_operation(&self) -> &str {
        &self.recommended_operation
    }
}

/// A module in the meso-split decomposition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MesoModule {
    #[serde(rename = "className")]
    pub class_name: String,
    pub responsibility: String,
    #[serde(rename = "staticRightBranch", default)]
    pub static_right_branch: Vec<MesoBranchItem>,
    #[serde(rename = "dynamicLeftBranch", default)]
    pub dynamic_left_branch: Vec<MesoBranchItem>,
}

/// A branch item (attribute or method) in meso-split.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MesoBranchItem {
    pub name: String,
    pub r#type: Option<String>,
    pub role: Option<String>,
    #[serde(default)]
    pub demand: Vec<String>,
    #[serde(default)]
    pub answer: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responsibility: Option<String>,
}

/// A micro-split segment — method/attribute-level decomposition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MicroSplit {
    #[serde(rename = "anchorNodeId")]
    pub anchor_node_id: String,
    #[serde(rename = "recommendedOperation", default)]
    pub recommended_operation: String,
    #[serde(default)]
    pub modules: Vec<MicroModule>,
}

impl TriadizationFocusReference for MicroSplit {
    fn triadization_focus(&self) -> &str {
        &self.anchor_node_id
    }
    fn recommended_operation(&self) -> &str {
        &self.recommended_operation
    }
}

/// A module in the micro-split decomposition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MicroModule {
    #[serde(rename = "className")]
    pub class_name: String,
    #[serde(rename = "staticRightBranch", default)]
    pub static_right_branch: Vec<MicroBranchItem>,
    #[serde(rename = "dynamicLeftBranch", default)]
    pub dynamic_left_branch: Vec<MicroBranchItem>,
}

/// A branch item in micro-split.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MicroBranchItem {
    pub name: String,
    #[serde(default)]
    pub demand: Vec<String>,
    #[serde(default)]
    pub answer: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responsibility: Option<String>,
}

/// Upgrade policy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradePolicy {
    #[serde(rename = "allowedOps")]
    pub allowed_ops: Vec<String>,
    pub principle: String,
}

/// The complete upgrade protocol — the central artifact of TriadMind.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradeProtocol {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub project: String,
    #[serde(rename = "mapSource")]
    pub map_source: String,
    #[serde(rename = "userDemand")]
    pub user_demand: String,
    #[serde(rename = "upgradePolicy")]
    pub upgrade_policy: UpgradePolicy,
    #[serde(rename = "macroSplit", default)]
    pub macro_split: Option<MacroSplit>,
    #[serde(rename = "mesoSplit", default)]
    pub meso_split: Option<MesoSplit>,
    #[serde(rename = "microSplit", default)]
    pub micro_split: Option<MicroSplit>,
    pub actions: Vec<ProtocolAction>,
}

/// Context for protocol validation.
#[derive(Debug, Clone, Default)]
pub struct ProtocolValidationContext {
    pub min_confidence: Option<f64>,
    pub require_confidence: bool,
    pub existing_nodes: Vec<TriadNodeDefinition>,
    pub expected_triadization_focus: Option<String>,
    pub expected_recommended_operation: Option<String>,
}

// ── Validation Logic ────────────────────────────────────────────────
// Ported from triadmind-core/protocol.ts

/// Validate an upgrade protocol against topology rules.
///
/// Checks:
/// 1. Confidence thresholds
/// 2. Triadization focus stability across splits
/// 3. Topology consistency (existing nodes, no duplicates, no C2C violations)
pub fn assert_protocol_shape(
    protocol: &UpgradeProtocol,
    context: &ProtocolValidationContext,
) -> Result<(), ProtocolValidationError> {
    validate_confidence_rules(protocol, context)?;
    validate_triadization_focus_rules(protocol, context)?;
    validate_topology_rules(protocol, context)?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolValidationError {
    #[error("action[{index}] missing confidence; confidence is required")]
    MissingConfidence { index: usize },
    #[error("action[{index}] confidence={confidence} below minimum {minimum}")]
    LowConfidence {
        index: usize,
        confidence: f64,
        minimum: f64,
    },
    #[error("protocol must contain all three splits (macro, meso, micro) to validate focus stability")]
    IncompleteSplits,
    #[error("{stage} triadization focus drifted: expected '{expected}' -> '{expected_op}', got '{actual}' -> '{actual_op}'")]
    FocusDrift {
        stage: String,
        expected: String,
        expected_op: String,
        actual: String,
        actual_op: String,
    },
    #[error("action[{index}] references non-existent node: {node_id}")]
    MissingNode { index: usize, node_id: String },
    #[error("action[{index}] duplicate operation on node: {node_id}")]
    DuplicateNode { index: usize, node_id: String },
    #[error("action[{index}] modify must not change problem: '{original}' vs '{changed}'")]
    ProblemChanged {
        index: usize,
        original: String,
        changed: String,
    },
    #[error("action[{index}] create_child cannot reuse existing nodeId: {node_id}")]
    ReuseExistingChild { index: usize, node_id: String },
    #[error("parse error: {0}")]
    ParseError(String),
}

fn validate_confidence_rules(
    protocol: &UpgradeProtocol,
    context: &ProtocolValidationContext,
) -> Result<(), ProtocolValidationError> {
    let min_confidence = context.min_confidence.unwrap_or(0.0);
    let require_confidence = context.require_confidence;

    for (index, action) in protocol.actions.iter().enumerate() {
        if require_confidence && action.confidence.is_none() {
            return Err(ProtocolValidationError::MissingConfidence { index });
        }
        if let Some(confidence) = action.confidence {
            if confidence < min_confidence {
                return Err(ProtocolValidationError::LowConfidence {
                    index,
                    confidence,
                    minimum: min_confidence,
                });
            }
        }
    }
    Ok(())
}

fn validate_triadization_focus_rules(
    protocol: &UpgradeProtocol,
    _context: &ProtocolValidationContext,
) -> Result<(), ProtocolValidationError> {
    let splits: Vec<(&str, Option<&dyn TriadizationFocusReference>)> = vec![
        ("macroSplit", protocol.macro_split.as_ref().map(|s| s as &dyn TriadizationFocusReference)),
        ("mesoSplit", protocol.meso_split.as_ref().map(|s| s as &dyn TriadizationFocusReference)),
        ("microSplit", protocol.micro_split.as_ref().map(|s| s as &dyn TriadizationFocusReference)),
    ];

    let present: Vec<_> = splits.iter().filter_map(|(name, split)| split.map(|s| (*name, s))).collect();

    if present.is_empty() {
        return Ok(());
    }
    if present.len() != 3 {
        return Err(ProtocolValidationError::IncompleteSplits);
    }

    let canonical = present[0].1;
    let canonical_focus = canonical.triadization_focus();
    let canonical_op = canonical.recommended_operation();

    for (stage_name, reference) in &present[1..] {
        let ref_focus = reference.triadization_focus();
        let ref_op = reference.recommended_operation();
        if ref_focus != canonical_focus || ref_op != canonical_op {
            return Err(ProtocolValidationError::FocusDrift {
                stage: stage_name.to_string(),
                expected: canonical_focus.to_string(),
                expected_op: canonical_op.to_string(),
                actual: ref_focus.to_string(),
                actual_op: ref_op.to_string(),
            });
        }
    }

    Ok(())
}

fn validate_topology_rules(
    protocol: &UpgradeProtocol,
    context: &ProtocolValidationContext,
) -> Result<(), ProtocolValidationError> {
    let existing_node_map: HashMap<&str, &TriadNodeDefinition> = context
        .existing_nodes
        .iter()
        .map(|n| (n.node_id.as_str(), n))
        .collect();

    let mut action_target_ids = HashSet::new();

    for (index, action) in protocol.actions.iter().enumerate() {
        match action.op {
            TriadOp::Reuse => {
                let node_id = action.node_id.as_deref().ok_or_else(|| {
                    ProtocolValidationError::ParseError(format!("action[{index}] reuse missing node_id"))
                })?;
                ensure_existing_node(&existing_node_map, node_id, index)?;
                ensure_unique_target(&mut action_target_ids, node_id, index)?;
            }
            TriadOp::Modify => {
                let node_id = action.node_id.as_deref().ok_or_else(|| {
                    ProtocolValidationError::ParseError(format!("action[{index}] modify missing node_id"))
                })?;
                let existing = ensure_existing_node(&existing_node_map, node_id, index)?;
                ensure_unique_target(&mut action_target_ids, node_id, index)?;

                // Modify must not change the core problem (C2C violation)
                if let Some(new_fission) = &action.fission {
                    if let Some(ref existing_fission) = existing.fission {
                        let orig = normalize_text(&existing_fission.problem);
                        let changed = normalize_text(&new_fission.problem);
                        if orig != changed {
                            return Err(ProtocolValidationError::ProblemChanged {
                                index,
                                original: existing_fission.problem.clone(),
                                changed: new_fission.problem.clone(),
                            });
                        }
                    }
                }
            }
            TriadOp::CreateChild => {
                let parent_node_id = action.parent_node_id.as_deref().ok_or_else(|| {
                    ProtocolValidationError::ParseError(format!(
                        "action[{index}] create_child missing parent_node_id"
                    ))
                })?;
                ensure_existing_node(&existing_node_map, parent_node_id, index)?;

                let new_node = action.node.as_ref().ok_or_else(|| {
                    ProtocolValidationError::ParseError(format!("action[{index}] create_child missing node"))
                })?;

                if existing_node_map.contains_key(new_node.node_id.as_str()) {
                    return Err(ProtocolValidationError::ReuseExistingChild {
                        index,
                        node_id: new_node.node_id.clone(),
                    });
                }
                ensure_unique_target(&mut action_target_ids, &new_node.node_id, index)?;
            }
        }
    }

    Ok(())
}

fn ensure_existing_node<'a>(
    existing_node_map: &HashMap<&str, &'a TriadNodeDefinition>,
    node_id: &str,
    index: usize,
) -> Result<&'a TriadNodeDefinition, ProtocolValidationError> {
    existing_node_map
        .get(node_id)
        .copied()
        .ok_or_else(|| ProtocolValidationError::MissingNode {
            index,
            node_id: node_id.to_string(),
        })
}

fn ensure_unique_target(
    target_ids: &mut HashSet<String>,
    node_id: &str,
    index: usize,
) -> Result<(), ProtocolValidationError> {
    if !target_ids.insert(node_id.to_string()) {
        return Err(ProtocolValidationError::DuplicateNode {
            index,
            node_id: node_id.to_string(),
        });
    }
    Ok(())
}

fn normalize_text(value: &str) -> String {
    value.trim().split_whitespace().collect::<Vec<_>>().join(" ")
}

// ── Node Reference Parsing ──────────────────────────────────────────
// Ported from triadmind-core/protocol.ts

const PREFIX_CATEGORY_MAP: &[(&str, &str)] = &[
    ("frontend", "frontend"),
    ("backend", "backend"),
    ("infra", "infra"),
    ("core", "core"),
    ("api", "api"),
    ("service", "service"),
    ("adapter", "adapter"),
    ("workflow", "workflow"),
];

/// Parse a nodeId string (e.g. "Workflow.buildMasterPrompt") into structured parts.
pub fn parse_node_ref(node_id: &str, category: Option<&str>) -> Result<ParsedNodeRef, ProtocolValidationError> {
    let trimmed = node_id.trim();
    if trimmed.is_empty() {
        return Err(ProtocolValidationError::ParseError("node_id cannot be empty".into()));
    }

    let raw_parts: Vec<&str> = trimmed.split('.').filter(|s| !s.is_empty()).collect();
    if raw_parts.is_empty() {
        return Err(ProtocolValidationError::ParseError(format!("invalid node_id: {node_id}")));
    }

    let mut resolved_category = category
        .map(|c| c.trim().to_lowercase())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "core".into());

    let mut parts = raw_parts.clone();

    let first_part = parts[0].to_lowercase();
    // Only strip the prefix category if we have enough remaining parts
    // to still form a class.method pair (i.e. at least 2 parts after stripping).
    if parts.len() > 2 {
        if let Some(&(_, cat)) = PREFIX_CATEGORY_MAP.iter().find(|(prefix, _)| *prefix == first_part) {
            resolved_category = cat.to_string();
            parts = parts[1..].to_vec();
        }
    }

    if parts.is_empty() {
        return Err(ProtocolValidationError::ParseError(format!(
            "node_id '{node_id}' missing class name"
        )));
    }

    let method_name = if parts.len() >= 2 {
        parts.last().unwrap().to_string()
    } else {
        "execute".to_string()
    };
    let class_name = if parts.len() >= 2 {
        parts[parts.len() - 2].to_string()
    } else {
        parts[0].to_string()
    };

    Ok(ParsedNodeRef {
        raw_node_id: trimmed.to_string(),
        normalized_node_id: format!("{}.{}", class_name, method_name),
        category: TriadCategory::Known(resolved_category),
        class_name,
        method_name,
    })
}

// ── Map Loading ─────────────────────────────────────────────────────

/// Read the triad-map.json file and parse it into a vector of node definitions.
pub fn read_triad_map<P: AsRef<Path>>(map_path: P) -> Result<Vec<TriadNodeDefinition>, ProtocolValidationError> {
    let content = std::fs::read_to_string(map_path.as_ref()).map_err(|e| {
        ProtocolValidationError::ParseError(format!("failed to read triad-map.json: {e}"))
    })?;

    serde_json::from_str(&content).map_err(|e| {
        ProtocolValidationError::ParseError(format!("failed to parse triad-map.json: {e}"))
    })
}

/// Check if a triad-map.json file exists at the given path.
pub fn triad_map_exists<P: AsRef<Path>>(map_path: P) -> bool {
    map_path.as_ref().exists()
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id: &str, problem: &str) -> TriadNodeDefinition {
        TriadNodeDefinition {
            node_id: id.to_string(),
            category: Some("core".into()),
            source_path: Some("src/lib.rs".into()),
            lifecycle: Some(TriadLifecycle::Existing),
            fission: Some(TriadFission {
                problem: problem.to_string(),
                demand: vec!["input".into()],
                answer: vec!["output".into()],
            }),
        }
    }

    #[test]
    fn test_parse_node_ref_simple() {
        let parsed = parse_node_ref("Workflow.execute", None).unwrap();
        assert_eq!(parsed.normalized_node_id, "Workflow.execute");
        assert_eq!(parsed.class_name, "Workflow");
        assert_eq!(parsed.method_name, "execute");
    }

    #[test]
    fn test_parse_node_ref_with_prefix_category() {
        let parsed = parse_node_ref("api.Router.handle", None).unwrap();
        assert_eq!(parsed.category.as_str(), "api");
        assert_eq!(parsed.class_name, "Router");
        assert_eq!(parsed.method_name, "handle");
    }

    #[test]
    fn test_parse_node_ref_no_method_uses_execute() {
        let parsed = parse_node_ref("Parser", None).unwrap();
        assert_eq!(parsed.method_name, "execute");
        assert_eq!(parsed.class_name, "Parser");
    }

    #[test]
    fn test_validate_reuse_existing_node() {
        let nodes = vec![make_node("Workflow.run", "run workflow")];
        let protocol = UpgradeProtocol {
            protocol_version: "1.0".into(),
            project: "test".into(),
            map_source: "triad-map.json".into(),
            user_demand: "test".into(),
            upgrade_policy: UpgradePolicy {
                allowed_ops: vec!["reuse".into()],
                principle: "reuse_first".into(),
            },
            macro_split: None,
            meso_split: None,
            micro_split: None,
            actions: vec![ProtocolAction {
                op: TriadOp::Reuse,
                node_id: Some("Workflow.run".into()),
                parent_node_id: None,
                node: None,
                fission: None,
                reuse: vec![],
                reason: Some("reuse workflow".into()),
                confidence: Some(0.95),
            }],
        };

        let context = ProtocolValidationContext {
            existing_nodes: nodes,
            ..Default::default()
        };

        assert!(assert_protocol_shape(&protocol, &context).is_ok());
    }

    #[test]
    fn test_validate_reuse_missing_node_fails() {
        let nodes = vec![];
        let protocol = UpgradeProtocol {
            protocol_version: "1.0".into(),
            project: "test".into(),
            map_source: "triad-map.json".into(),
            user_demand: "test".into(),
            upgrade_policy: UpgradePolicy {
                allowed_ops: vec!["reuse".into()],
                principle: "reuse_first".into(),
            },
            macro_split: None,
            meso_split: None,
            micro_split: None,
            actions: vec![ProtocolAction {
                op: TriadOp::Reuse,
                node_id: Some("Missing.run".into()),
                parent_node_id: None,
                node: None,
                fission: None,
                reuse: vec![],
                reason: None,
                confidence: Some(0.95),
            }],
        };

        let context = ProtocolValidationContext {
            existing_nodes: nodes,
            ..Default::default()
        };

        let err = assert_protocol_shape(&protocol, &context).unwrap_err();
        assert!(err.to_string().contains("Missing.run"));
    }

    #[test]
    fn test_validate_modify_changing_problem_fails() {
        let nodes = vec![make_node("Service.handle", "process orders")];
        let protocol = UpgradeProtocol {
            protocol_version: "1.0".into(),
            project: "test".into(),
            map_source: "triad-map.json".into(),
            user_demand: "test".into(),
            upgrade_policy: UpgradePolicy {
                allowed_ops: vec!["modify".into()],
                principle: "reuse_first".into(),
            },
            macro_split: None,
            meso_split: None,
            micro_split: None,
            actions: vec![ProtocolAction {
                op: TriadOp::Modify,
                node_id: Some("Service.handle".into()),
                parent_node_id: None,
                node: None,
                fission: Some(TriadFission {
                    problem: "send emails".into(), // changed!
                    demand: vec![],
                    answer: vec![],
                }),
                reuse: vec![],
                reason: None,
                confidence: Some(0.95),
            }],
        };

        let context = ProtocolValidationContext {
            existing_nodes: nodes,
            ..Default::default()
        };

        let err = assert_protocol_shape(&protocol, &context).unwrap_err();
        assert!(err.to_string().contains("problem"));
    }

    #[test]
    fn test_validate_create_child_existing_node_fails() {
        let nodes = vec![make_node("Parent.run", "parent")];
        let protocol = UpgradeProtocol {
            protocol_version: "1.0".into(),
            project: "test".into(),
            map_source: "triad-map.json".into(),
            user_demand: "test".into(),
            upgrade_policy: UpgradePolicy {
                allowed_ops: vec!["create_child".into()],
                principle: "reuse_first".into(),
            },
            macro_split: None,
            meso_split: None,
            micro_split: None,
            actions: vec![ProtocolAction {
                op: TriadOp::CreateChild,
                node_id: None,
                parent_node_id: Some("Parent.run".into()),
                node: Some(make_node("Parent.run", "child")), // same id as existing!
                fission: None,
                reuse: vec![],
                reason: None,
                confidence: Some(0.95),
            }],
        };

        let context = ProtocolValidationContext {
            existing_nodes: nodes,
            ..Default::default()
        };

        let err = assert_protocol_shape(&protocol, &context).unwrap_err();
        assert!(err.to_string().contains("cannot reuse existing nodeId"));
    }
}
