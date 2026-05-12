//! # Navigate — Pre-implementation architecture impact mapping
//!
//! Ported from triadmind-core/navigator.ts
//!
//! Before writing code, generates an "impact map" showing how a proposed
//! feature intersects with the existing topology. Produces:
//! - An impact protocol (UpgradeProtocol draft)
//! - A navigator prompt for LLM-guided protocol generation
//! - Impact visualizer data
//!
//! @LeftBranch: run_navigator, build_navigator_prompt
//! @RightBranch: NavigatorRunOptions, NavigatorRunResult, ImpactMapArtifact

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::protocol::{TriadNodeDefinition, UpgradeProtocol};

// ── Run Options ─────────────────────────────────────────────────────

/// Options for running the navigator.
#[derive(Debug, Clone)]
pub struct NavigatorRunOptions {
    /// Optional path to an existing upgrade protocol.
    pub protocol_path: Option<PathBuf>,
    /// LLM provider name (for prompt-based protocol generation).
    pub llm: Option<String>,
}

impl Default for NavigatorRunOptions {
    fn default() -> Self {
        Self {
            protocol_path: None,
            llm: None,
        }
    }
}

// ── Run Result ──────────────────────────────────────────────────────

/// Result of a navigator run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigatorRunResult {
    /// Status: "pending_protocol" or "ready".
    pub status: String,
    /// The feature demand that was analyzed.
    pub demand: String,
    /// Path to the impact map artifact.
    #[serde(rename = "impactMapFile")]
    pub impact_map_file: String,
    /// Path to the impact protocol.
    #[serde(rename = "impactProtocolFile")]
    pub impact_protocol_file: String,
    /// Path to the navigator prompt.
    #[serde(rename = "impactPromptFile")]
    pub impact_prompt_file: String,
    /// Path to the visualizer output.
    #[serde(rename = "impactVisualizerFile")]
    pub impact_visualizer_file: String,
    /// Summary lines for display.
    pub summary: Vec<String>,
}

// ── Impact Map Artifact ─────────────────────────────────────────────

/// The impact map artifact written to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactMapArtifact {
    #[serde(rename = "schemaVersion")]
    pub schema_version: String,
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    /// Project name.
    pub project: String,
    /// Feature description.
    pub feature: String,
    /// Path to the impact protocol.
    #[serde(rename = "protocolFile")]
    pub protocol_file: String,
    /// Path to the visualizer output.
    #[serde(rename = "visualizerFile")]
    pub visualizer_file: String,
    /// The generated upgrade protocol.
    pub protocol: UpgradeProtocol,
    /// Preview topology data.
    #[serde(rename = "previewTopology")]
    pub preview_topology: serde_json::Value,
    /// Impact graph data.
    pub graph: serde_json::Value,
    /// Summary statistics.
    pub summary: ImpactGraphSummary,
}

/// Summary of the impact graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactGraphSummary {
    #[serde(rename = "proposedNodeCount")]
    pub proposed_node_count: usize,
    #[serde(rename = "proposedEdgeCount")]
    pub proposed_edge_count: usize,
    #[serde(rename = "totalVisibleNodes")]
    pub total_visible_nodes: usize,
    #[serde(rename = "totalVisibleEdges")]
    pub total_visible_edges: usize,
}

// ── Core Entry Point ────────────────────────────────────────────────

/// Run the navigator to generate an architecture impact map.
///
/// Analyzes how a feature demand intersects with the existing triad topology.
/// When a protocol already exists, generates the impact map directly.
/// When no protocol exists, writes a prompt for LLM-guided protocol generation.
pub fn run_navigator(
    project_root: &Path,
    demand: &str,
    options: &NavigatorRunOptions,
) -> Result<NavigatorRunResult, anyhow::Error> {
    let normalized_demand = demand.trim();
    if normalized_demand.is_empty() {
        return Err(anyhow::anyhow!("Navigator demand cannot be empty."));
    }

    let triad_dir = project_root.join(".triadmind");
    std::fs::create_dir_all(&triad_dir)?;

    let now = crate::sync::chrono_now();

    // Paths
    let impact_map_file = triad_dir.join("impact-map.json");
    let impact_protocol_file = triad_dir.join("impact-protocol.json");
    let impact_prompt_file = triad_dir.join("impact-prompt.txt");
    let impact_visualizer_file = triad_dir.join("impact-visualizer.html");

    // ── Load existing topology ────────────────────────────────────
    let map_file = triad_dir.join("triad-map.json");
    let existing_nodes: Vec<TriadNodeDefinition> = if map_file.exists() {
        let content = std::fs::read_to_string(&map_file)?;
        let trimmed = content.trim().trim_start_matches('\u{FEFF}');
        serde_json::from_str(trimmed).unwrap_or_default()
    } else {
        Vec::new()
    };

    // ── Try to load existing protocol ─────────────────────────────
    let protocol_path = options
        .protocol_path
        .clone()
        .unwrap_or_else(|| impact_protocol_file.clone());

    let protocol: Option<UpgradeProtocol> = if protocol_path.exists() {
        let content = std::fs::read_to_string(&protocol_path)?;
        let trimmed = content.trim().trim_start_matches('\u{FEFF}');
        serde_json::from_str(trimmed).ok()
    } else {
        None
    };

    // ── Build navigator prompt ────────────────────────────────────
    let prompt = build_navigator_prompt(
        project_root,
        normalized_demand,
        &existing_nodes,
        protocol.as_ref(),
    );
    std::fs::write(&impact_prompt_file, &prompt)?;

    // Write demand file
    std::fs::write(triad_dir.join("demand.txt"), normalized_demand)?;

    if protocol.is_none() {
        // No protocol yet — write a template and return pending status
        let template = build_protocol_template(normalized_demand, &existing_nodes);
        std::fs::write(
            &impact_protocol_file,
            serde_json::to_string_pretty(&template)?,
        )?;

        return Ok(NavigatorRunResult {
            status: "pending_protocol".into(),
            demand: normalized_demand.into(),
            impact_map_file: impact_map_file.to_string_lossy().to_string(),
            impact_protocol_file: impact_protocol_file.to_string_lossy().to_string(),
            impact_prompt_file: impact_prompt_file.to_string_lossy().to_string(),
            impact_visualizer_file: impact_visualizer_file.to_string_lossy().to_string(),
            summary: vec![
                format!("Impact prompt written: {}", impact_prompt_file.display()),
                format!(
                    "Impact protocol template ready: {}",
                    impact_protocol_file.display()
                ),
                "Populate impact-protocol.json with a valid UpgradeProtocol, then rerun."
                    .into(),
            ],
        });
    }

    // ── Protocol exists — build impact map ────────────────────────
    let p = protocol.unwrap();
    let proposed_nodes = p
        .actions
        .iter()
        .filter(|a| {
            matches!(
                a.op,
                crate::protocol::TriadOp::CreateChild | crate::protocol::TriadOp::Modify
            )
        })
        .count();

    let artifact = ImpactMapArtifact {
        schema_version: "1.0".into(),
        generated_at: now,
        project: project_root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        feature: normalized_demand.into(),
        protocol_file: protocol_path.to_string_lossy().to_string(),
        visualizer_file: impact_visualizer_file.to_string_lossy().to_string(),
        protocol: p.clone(),
        preview_topology: serde_json::json!({
            "nodes": existing_nodes.len(),
            "proposedNodes": proposed_nodes,
        }),
        graph: serde_json::json!({}),
        summary: ImpactGraphSummary {
            proposed_node_count: proposed_nodes,
            proposed_edge_count: 0,
            total_visible_nodes: existing_nodes.len() + proposed_nodes,
            total_visible_edges: 0,
        },
    };

    std::fs::write(
        &impact_map_file,
        serde_json::to_string_pretty(&artifact)?,
    )?;

    Ok(NavigatorRunResult {
        status: "ready".into(),
        demand: normalized_demand.into(),
        impact_map_file: impact_map_file.to_string_lossy().to_string(),
        impact_protocol_file: impact_protocol_file.to_string_lossy().to_string(),
        impact_prompt_file: impact_prompt_file.to_string_lossy().to_string(),
        impact_visualizer_file: impact_visualizer_file.to_string_lossy().to_string(),
        summary: vec![
            format!(
                "Impact map generated: {} existing + {} proposed nodes",
                existing_nodes.len(),
                proposed_nodes
            ),
            format!("Impact map artifact: {}", impact_map_file.display()),
        ],
    })
}

// ── Prompt Building ─────────────────────────────────────────────────

/// Build a navigator prompt for LLM-guided protocol generation.
pub fn build_navigator_prompt(
    project_root: &Path,
    demand: &str,
    existing_nodes: &[TriadNodeDefinition],
    _existing_protocol: Option<&UpgradeProtocol>,
) -> String {
    let mut sections = Vec::new();

    sections.push(format!(
        "[System]\n\
         You are TriadMind Navigator, a pre-implementation architecture copilot.\n\
         Your task is to infer the architecture impact of a requested feature before any code is written.\n\
         Return only strict JSON compatible with UpgradeProtocol. Do not include prose outside the JSON payload."
    ));

    sections.push(format!(
        "[Project Root]\n{}",
        project_root.display()
    ));

    sections.push(format!(
        "[Triad Map Path]\n{}",
        project_root
            .join(".triadmind")
            .join("triad-map.json")
            .display()
    ));

    // Existing topology summary
    let topology_summary: Vec<String> = existing_nodes
        .iter()
        .take(50)
        .map(|n| {
            format!(
                "  {} (demand: [{}], answer: [{}])",
                n.node_id,
                n.fission
                    .as_ref()
                    .map(|f| f.demand.join(", "))
                    .unwrap_or_default(),
                n.fission
                    .as_ref()
                    .map(|f| f.answer.join(", "))
                    .unwrap_or_default(),
            )
        })
        .collect();

    sections.push(format!(
        "[Existing Topology ({} nodes)]\n{}",
        existing_nodes.len(),
        topology_summary.join("\n")
    ));

    sections.push(format!("[User Demand]\n{}", demand));

    sections.push(
        "[Navigator Rules]\n\
         - This is a dry-run architecture preview, not an apply step.\n\
         - Favor reuse of mature existing nodes before inventing new capability hubs.\n\
         - Use only reuse / modify / create_child actions.\n\
         - Keep changes minimal and topology-aware.\n\
         - Return strict UpgradeProtocol JSON only."
            .to_string(),
    );

    sections.join("\n\n")
}

// ── Protocol Template ───────────────────────────────────────────────

/// Build a minimal protocol template for the user to fill in.
fn build_protocol_template(
    demand: &str,
    _existing_nodes: &[TriadNodeDefinition],
) -> UpgradeProtocol {
    UpgradeProtocol {
        protocol_version: "1.0".into(),
        project: String::new(),
        map_source: "triad-map.json".into(),
        user_demand: demand.into(),
        upgrade_policy: crate::protocol::UpgradePolicy {
            allowed_ops: vec!["reuse".into(), "modify".into(), "create_child".into()],
            principle: "reuse_first".into(),
        },
        macro_split: None,
        meso_split: None,
        micro_split: None,
        actions: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_navigator_rejects_empty_demand() {
        let tmp = std::env::temp_dir().join("triadmind_nav_test");
        let _ = std::fs::create_dir_all(&tmp);
        let opts = NavigatorRunOptions::default();
        let result = run_navigator(&tmp, "", &opts);
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(result.is_err());
    }

    #[test]
    fn test_navigator_pending_without_protocol() {
        let tmp = std::env::temp_dir().join("triadmind_nav_pending");
        let _ = std::fs::create_dir_all(tmp.join(".triadmind"));
        let opts = NavigatorRunOptions::default();
        let result = run_navigator(&tmp, "add login feature", &opts);
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.status, "pending_protocol");
        assert!(r.summary.iter().any(|s| s.contains("template")));
    }

    #[test]
    fn test_build_navigator_prompt() {
        let nodes: Vec<TriadNodeDefinition> = vec![];
        let prompt = build_navigator_prompt(
            Path::new("/test"),
            "add feature X",
            &nodes,
            None,
        );
        assert!(prompt.contains("add feature X"));
        assert!(prompt.contains("Navigator"));
        assert!(prompt.contains("UpgradeProtocol"));
    }

    #[test]
    fn test_navigator_with_existing_protocol() {
        let tmp = std::env::temp_dir().join("triadmind_nav_ready");
        let _ = std::fs::create_dir_all(tmp.join(".triadmind"));

        // Write a simple protocol
        let protocol = UpgradeProtocol {
            protocol_version: "1.0".into(),
            project: "test".into(),
            map_source: "triad-map.json".into(),
            user_demand: "test feature".into(),
            upgrade_policy: crate::protocol::UpgradePolicy {
                allowed_ops: vec!["reuse".into()],
                principle: "reuse_first".into(),
            },
            macro_split: None,
            meso_split: None,
            micro_split: None,
            actions: vec![],
        };
        let protocol_path = tmp.join(".triadmind").join("impact-protocol.json");
        std::fs::write(
            &protocol_path,
            serde_json::to_string_pretty(&protocol).unwrap(),
        )
        .unwrap();

        let opts = NavigatorRunOptions {
            protocol_path: Some(protocol_path),
            llm: None,
        };
        let result = run_navigator(&tmp, "test feature", &opts);
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.status, "ready");
        assert!(r.summary.iter().any(|s| s.contains("Impact map generated")));
    }
}
