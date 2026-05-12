//! # Workflow — Prompt pipeline orchestration
//!
//! Ported from triadmind-core/workflow.ts
//!
//! Manages the Macro→Meso→Micro prompt pipeline:
//! - `write_prompt_packet`: generates all prompt files for a feature demand
//! - `build_macro_prompt`: Macro-level architecture split prompt
//! - `build_meso_prompt`: Meso-level module split prompt
//! - `build_micro_prompt`: Micro-level implementation prompt
//!
//! @LeftBranch: write_prompt_packet, build_macro_prompt, build_implementation_prompt
//! @RightBranch: WorkflowPaths, PromptPacket

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::protocol::{TriadNodeDefinition, UpgradeProtocol};

// ── Workflow Paths ──────────────────────────────────────────────────

/// Resolved paths for the TriadMind workflow pipeline.
#[derive(Debug, Clone)]
pub struct WorkflowPaths {
    pub project_root: PathBuf,
    pub triad_dir: PathBuf,
    /// Main prompt file.
    pub prompt_file: PathBuf,
    /// Macro split prompt.
    pub macro_prompt_file: PathBuf,
    /// Meso split prompt.
    pub meso_prompt_file: PathBuf,
    /// Micro split prompt.
    pub micro_prompt_file: PathBuf,
    /// Pipeline orchestration prompt.
    pub pipeline_prompt_file: PathBuf,
    /// Protocol task prompt.
    pub protocol_task_file: PathBuf,
    /// Implementation prompt.
    pub implementation_prompt_file: PathBuf,
    /// Master orchestration prompt.
    pub master_prompt_file: PathBuf,
    /// Feature demand file.
    pub demand_file: PathBuf,
    /// Triad spec file.
    pub triad_spec_file: PathBuf,
    /// Draft protocol file.
    pub draft_file: PathBuf,
}

impl WorkflowPaths {
    /// Build workflow paths from a project root.
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        let root = project_root.into();
        let triad_dir = root.join(".triadmind");
        Self {
            prompt_file: triad_dir.join("prompt.txt"),
            macro_prompt_file: triad_dir.join("macro-prompt.txt"),
            meso_prompt_file: triad_dir.join("meso-prompt.txt"),
            micro_prompt_file: triad_dir.join("micro-prompt.txt"),
            pipeline_prompt_file: triad_dir.join("pipeline-prompt.txt"),
            protocol_task_file: triad_dir.join("protocol-task.txt"),
            implementation_prompt_file: triad_dir.join("implementation-prompt.txt"),
            master_prompt_file: triad_dir.join("master-prompt.txt"),
            demand_file: triad_dir.join("demand.txt"),
            triad_spec_file: triad_dir.join("triad.md"),
            draft_file: triad_dir.join("draft-protocol.json"),
            project_root: root,
            triad_dir,
        }
    }
}

// ── Prompt Packet ───────────────────────────────────────────────────

/// A collection of generated prompt files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptPacket {
    /// The normalized feature demand.
    pub demand: String,
    /// Path to the protocol task prompt.
    #[serde(rename = "promptFile")]
    pub prompt_file: String,
    /// Path to the pipeline orchestration prompt.
    #[serde(rename = "pipelinePromptFile")]
    pub pipeline_prompt_file: String,
    /// Path to the implementation prompt.
    #[serde(rename = "implementationPromptFile")]
    pub implementation_prompt_file: String,
    /// Number of existing topology nodes loaded.
    #[serde(rename = "topologyNodeCount")]
    pub topology_node_count: usize,
}

// ── Core Entry Point ────────────────────────────────────────────────

/// Write the full prompt packet for a feature demand.
///
/// Generates prompt files for protocol drafting, implementation, and
/// pipeline orchestration based on the existing triad topology.
pub fn write_prompt_packet(
    paths: &WorkflowPaths,
    demand: &str,
    existing_nodes: &[TriadNodeDefinition],
) -> Result<PromptPacket, anyhow::Error> {
    let normalized_demand = demand.trim();
    if normalized_demand.is_empty() {
        return Err(anyhow::anyhow!("Workflow demand cannot be empty."));
    }

    std::fs::create_dir_all(&paths.triad_dir)?;

    // Write triad spec
    write_triad_spec(&paths.triad_spec_file)?;

    // Write draft protocol template
    let draft = build_draft_protocol(normalized_demand);
    std::fs::write(
        &paths.draft_file,
        serde_json::to_string_pretty(&draft)?,
    )?;

    // Build prompts
    let protocol_prompt = build_protocol_prompt(paths, normalized_demand, existing_nodes);
    let implementation_prompt = build_implementation_prompt(paths, normalized_demand, existing_nodes);
    let pipeline_prompt = build_pipeline_prompt(paths, normalized_demand);
    let macro_prompt = build_macro_prompt(paths, normalized_demand, existing_nodes);
    let meso_prompt = build_meso_prompt(paths, normalized_demand, existing_nodes);
    let micro_prompt = build_micro_prompt(paths, normalized_demand, existing_nodes);
    let master_prompt = build_master_prompt(paths);

    // Write all prompt files
    std::fs::write(&paths.prompt_file, &protocol_prompt)?;
    std::fs::write(&paths.protocol_task_file, &protocol_prompt)?;
    std::fs::write(&paths.pipeline_prompt_file, &pipeline_prompt)?;
    std::fs::write(&paths.implementation_prompt_file, &implementation_prompt)?;
    std::fs::write(&paths.macro_prompt_file, &macro_prompt)?;
    std::fs::write(&paths.meso_prompt_file, &meso_prompt)?;
    std::fs::write(&paths.micro_prompt_file, &micro_prompt)?;
    std::fs::write(&paths.master_prompt_file, &master_prompt)?;
    std::fs::write(&paths.demand_file, normalized_demand)?;

    Ok(PromptPacket {
        demand: normalized_demand.into(),
        prompt_file: paths.prompt_file.to_string_lossy().to_string(),
        pipeline_prompt_file: paths.pipeline_prompt_file.to_string_lossy().to_string(),
        implementation_prompt_file: paths.implementation_prompt_file.to_string_lossy().to_string(),
        topology_node_count: existing_nodes.len(),
    })
}

// ── Prompt Builders ─────────────────────────────────────────────────

/// Build the protocol drafting prompt.
pub fn build_protocol_prompt(
    paths: &WorkflowPaths,
    demand: &str,
    nodes: &[TriadNodeDefinition],
) -> String {
    let topology = format_topology_summary(nodes);

    format!(
        "[System]\n\
         You are TriadMind Protocol Drafter.\n\
         Your task is to produce a strict UpgradeProtocol JSON for the following feature demand.\n\
         Analyze the existing topology and determine reuse/modify/create_child actions.\n\
         Output only valid JSON.\n\n\
         [Project Root]\n{}\n\n\
         [Draft Protocol Path]\n{}\n\n\
         [Existing Topology ({} nodes)]\n{}\n\n\
         [Feature Demand]\n{}\n\n\
         [Rules]\n\
         - Reuse existing nodes when possible (reuse_first principle).\n\
         - Modify nodes when their contract needs to change.\n\
         - Create child nodes only for new capabilities.\n\
         - Include fission (problem/demand/answer) for all new/modified nodes.\n\
         - Return strict UpgradeProtocol JSON only.",
        paths.project_root.display(),
        paths.draft_file.display(),
        nodes.len(),
        topology,
        demand,
    )
}

/// Build the implementation prompt.
pub fn build_implementation_prompt(
    paths: &WorkflowPaths,
    demand: &str,
    nodes: &[TriadNodeDefinition],
) -> String {
    let topology = format_topology_summary(nodes);

    format!(
        "[System]\n\
         You are TriadMind Implementation Agent.\n\
         Given an UpgradeProtocol and existing topology, implement the required code changes.\n\n\
         [Project Root]\n{}\n\n\
         [Draft Protocol Path]\n{}\n\n\
         [Existing Topology ({} nodes)]\n{}\n\n\
         [User Demand]\n{}\n\n\
         [Rules]\n\
         - Read the draft protocol from the path above.\n\
         - Implement each action in order.\n\
         - For reuse: verify the existing node meets the contract.\n\
         - For modify: update the existing implementation.\n\
         - For create_child: create new files with skeleton implementation.\n\
         - After implementation, run triadmind verify and fix any issues.",
        paths.project_root.display(),
        paths.draft_file.display(),
        nodes.len(),
        topology,
        demand,
    )
}

/// Build the pipeline orchestration prompt.
pub fn build_pipeline_prompt(paths: &WorkflowPaths, demand: &str) -> String {
    format!(
        "[System]\n\
         You are TriadMind Pipeline Orchestrator.\n\
         Your role is to coordinate the Macro→Meso→Micro workflow.\n\n\
         [Pipeline Stages]\n\
         1. Macro Split: Define high-level architecture components.\n\
         2. Meso Split: Decompose each macro component into modules.\n\
         3. Micro Split: Detail implementation for each module.\n\
         4. Protocol Draft: Generate the UpgradeProtocol.\n\
         5. Implementation: Execute the protocol.\n\n\
         [Project Root]\n{}\n\n\
         [User Demand]\n{}\n\n\
         [Artifact Files]\n\
         - Macro: {}\n\
         - Meso: {}\n\
         - Micro: {}\n\
         - Protocol: {}\n\n\
         Execute each stage sequentially, validating outputs at each step.",
        paths.project_root.display(),
        demand,
        paths.macro_prompt_file.display(),
        paths.meso_prompt_file.display(),
        paths.micro_prompt_file.display(),
        paths.prompt_file.display(),
    )
}

/// Build the macro split prompt.
pub fn build_macro_prompt(
    paths: &WorkflowPaths,
    demand: &str,
    nodes: &[TriadNodeDefinition],
) -> String {
    let topology = format_topology_summary(nodes);

    format!(
        "[System]\n\
         You are TriadMind Macro Split Architect.\n\
         Decompose the feature into high-level capability domains.\n\
         Each domain should represent a distinct architectural concern.\n\n\
         [Project Root]\n{}\n\n\
         [Existing Topology ({} nodes)]\n{}\n\n\
         [Feature Demand]\n{}\n\n\
         [Output Format]\n\
         {{\n\
           \"anchorNodeId\": \"ExistingOrNewNode\",\n\
           \"recommendedOperation\": \"split\",\n\
           \"vertexGoal\": \"description of the macro split goal\"\n\
         }}",
        paths.project_root.display(),
        nodes.len(),
        topology,
        demand,
    )
}

/// Build the meso split prompt.
pub fn build_meso_prompt(
    paths: &WorkflowPaths,
    demand: &str,
    nodes: &[TriadNodeDefinition],
) -> String {
    let topology = format_topology_summary(nodes);

    format!(
        "[System]\n\
         You are TriadMind Meso Split Designer.\n\
         Given a macro split, decompose each domain into modules.\n\n\
         [Project Root]\n{}\n\n\
         [Existing Topology ({} nodes)]\n{}\n\n\
         [Feature Demand]\n{}\n\n\
         [Output Format]\n\
         {{\n\
           \"anchorNodeId\": \"MacroNode\",\n\
           \"recommendedOperation\": \"split\",\n\
           \"modules\": [\n\
             {{\"name\": \"ModuleName\", \"responsibility\": \"...\"}}\n\
           ]\n\
         }}",
        paths.project_root.display(),
        nodes.len(),
        topology,
        demand,
    )
}

/// Build the micro split prompt.
pub fn build_micro_prompt(
    paths: &WorkflowPaths,
    demand: &str,
    nodes: &[TriadNodeDefinition],
) -> String {
    let topology = format_topology_summary(nodes);

    format!(
        "[System]\n\
         You are TriadMind Micro Split Implementer.\n\
         Detail the implementation plan for each module.\n\n\
         [Project Root]\n{}\n\n\
         [Existing Topology ({} nodes)]\n{}\n\n\
         [Feature Demand]\n{}\n\n\
         [Output Format]\n\
         {{\n\
           \"anchorNodeId\": \"MesoModule\",\n\
           \"recommendedOperation\": \"split\",\n\
           \"modules\": [\n\
             {{\"name\": \"ImplementationUnit\", \"operations\": [\"op1\", \"op2\"]}}\n\
           ]\n\
         }}",
        paths.project_root.display(),
        nodes.len(),
        topology,
        demand,
    )
}

/// Build the master orchestration prompt.
pub fn build_master_prompt(paths: &WorkflowPaths) -> String {
    let now = crate::sync::chrono_now();

    format!(
        "[System]\n\
         You are TriadMind Master Orchestrator.\n\
         Generated at: {}\n\n\
         [Pipeline]\n\
         The TriadMind workflow consists of:\n\
         1. Protocol Draft — analyze demand and produce UpgradeProtocol.\n\
         2. Macro Split — define high-level architecture domains.\n\
         3. Meso Split — decompose domains into modules.\n\
         4. Micro Split — detail implementation per module.\n\
         5. Implementation — execute the protocol.\n\
         6. Verify — validate topology quality.\n\n\
         [Artifact Paths]\n\
         - Protocol: {}\n\
         - Macro: {}\n\
         - Meso: {}\n\
         - Micro: {}\n\
         - Implementation: {}\n\n\
         Run each stage in order. Validate outputs before proceeding.",
        now,
        paths.prompt_file.display(),
        paths.macro_prompt_file.display(),
        paths.meso_prompt_file.display(),
        paths.micro_prompt_file.display(),
        paths.implementation_prompt_file.display(),
    )
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Format existing topology nodes as a readable summary.
fn format_topology_summary(nodes: &[TriadNodeDefinition]) -> String {
    nodes
        .iter()
        .take(100)
        .map(|n| {
            format!(
                "  {} [demand: {}] [answer: {}] @ {}",
                n.node_id,
                n.fission
                    .as_ref()
                    .map(|f| f.demand.join(", "))
                    .unwrap_or_default(),
                n.fission
                    .as_ref()
                    .map(|f| f.answer.join(", "))
                    .unwrap_or_default(),
                n.source_path.as_deref().unwrap_or("unknown"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Write the triad spec file.
fn write_triad_spec(path: &Path) -> Result<(), std::io::Error> {
    std::fs::write(
        path,
        "# TriadMind Architecture Governance\n\n\
         ## Vertex Method\n\n\
         Every architecture decision follows the Problem → Demand → Answer triple:\n\
         - **Problem**: what responsibility does this node own?\n\
         - **Demand**: what inputs does it require?\n\
         - **Answer**: what outputs does it produce?\n\n\
         ## Operations\n\n\
         - **Reuse**: use an existing node without modification.\n\
         - **Modify**: change an existing node's contract.\n\
         - **CreateChild**: create a new node under an existing parent.\n\n\
         ## Pipeline\n\n\
         Macro → Meso → Micro → Protocol → Implementation → Verify\n",
    )?;
    Ok(())
}

/// Build a minimal draft protocol template.
fn build_draft_protocol(demand: &str) -> UpgradeProtocol {
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

    fn make_node(id: &str, path: &str) -> TriadNodeDefinition {
        TriadNodeDefinition {
            node_id: id.into(),
            category: Some("core".into()),
            source_path: Some(path.into()),
            lifecycle: None,
            fission: Some(crate::protocol::TriadFission {
                problem: "test".into(),
                demand: vec!["Input".into()],
                answer: vec!["Output".into()],
            }),
        }
    }

    #[test]
    fn test_workflow_paths_construction() {
        let paths = WorkflowPaths::new("/test/project");
        assert_eq!(
            paths.triad_dir,
            PathBuf::from("/test/project/.triadmind")
        );
        assert_eq!(
            paths.prompt_file,
            PathBuf::from("/test/project/.triadmind/prompt.txt")
        );
    }

    #[test]
    fn test_write_prompt_packet() {
        let tmp = std::env::temp_dir().join("triadmind_wf_packet");
        let _ = std::fs::create_dir_all(&tmp);
        let paths = WorkflowPaths::new(&tmp);
        let nodes: Vec<TriadNodeDefinition> = vec![make_node("Svc.run", "src/svc.rs")];
        let result = write_prompt_packet(&paths, "add login", &nodes);
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(result.is_ok());
        let packet = result.unwrap();
        assert_eq!(packet.demand, "add login");
        assert_eq!(packet.topology_node_count, 1);
    }

    #[test]
    fn test_build_protocol_prompt_contains_demand() {
        let paths = WorkflowPaths::new("/test");
        let nodes: Vec<TriadNodeDefinition> = vec![];
        let prompt = build_protocol_prompt(&paths, "add feature X", &nodes);
        assert!(prompt.contains("add feature X"));
        assert!(prompt.contains("UpgradeProtocol"));
        assert!(prompt.contains("reuse_first"));
    }

    #[test]
    fn test_build_macro_prompt_format() {
        let paths = WorkflowPaths::new("/test");
        let nodes: Vec<TriadNodeDefinition> = vec![make_node("A.run", "src/a.rs")];
        let prompt = build_macro_prompt(&paths, "add login", &nodes);
        assert!(prompt.contains("Macro Split"));
        assert!(prompt.contains("anchorNodeId"));
        assert!(prompt.contains("A.run"));
    }

    #[test]
    fn test_build_pipeline_prompt() {
        let paths = WorkflowPaths::new("/test");
        let prompt = build_pipeline_prompt(&paths, "add pipeline");
        assert!(prompt.contains("Pipeline Orchestrator"));
        assert!(prompt.contains("Macro"));
        assert!(prompt.contains("Meso"));
        assert!(prompt.contains("Micro"));
    }

    #[test]
    fn test_write_prompt_packet_rejects_empty_demand() {
        let tmp = std::env::temp_dir().join("triadmind_wf_empty");
        let _ = std::fs::create_dir_all(&tmp);
        let paths = WorkflowPaths::new(&tmp);
        let nodes: Vec<TriadNodeDefinition> = vec![];
        let result = write_prompt_packet(&paths, "", &nodes);
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(result.is_err());
    }
}
