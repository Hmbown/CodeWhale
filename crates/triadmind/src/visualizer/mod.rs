//! # Visualizer — Interactive HTML topology knowledge graph
//!
//! Ported from triadmind-core/visualizer.ts
//!
//! Generates a self-contained HTML file that renders the triad topology
//! as an interactive graph using vis-network (CDN). Shows:
//! - Capability nodes with fission triples
//! - Edges representing dependencies
//! - Color-coded lifecycle states
//! - Filter/search controls
//!
//! @LeftBranch: generate_triad_visualizer, render_html
//! @RightBranch: VisualizerOptions, KnowledgeNode, KnowledgeEdge

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::protocol::TriadNodeDefinition;

// ── Visualizer Options ──────────────────────────────────────────────

/// Options for visualizer generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualizerOptions {
    /// Default view mode.
    #[serde(default = "default_view")]
    pub default_view: String,
    /// Whether to show isolated (unconnected) nodes.
    #[serde(default)]
    pub show_isolated_capabilities: bool,
    /// Maximum number of nodes to render.
    #[serde(default = "default_max_nodes")]
    pub max_render_nodes: usize,
    /// Maximum number of edges to render.
    #[serde(default = "default_max_edges")]
    pub max_render_edges: usize,
}

fn default_view() -> String {
    "architecture".into()
}
fn default_max_nodes() -> usize {
    500
}
fn default_max_edges() -> usize {
    1500
}

impl Default for VisualizerOptions {
    fn default() -> Self {
        Self {
            default_view: default_view(),
            show_isolated_capabilities: false,
            max_render_nodes: default_max_nodes(),
            max_render_edges: default_max_edges(),
        }
    }
}

// ── Knowledge Graph Types ───────────────────────────────────────────

/// A node in the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeNode {
    pub id: String,
    pub label: String,
    pub group: String,
    /// Node lifecycle state.
    pub lifecycle: String,
    /// Fission problem statement.
    pub problem: String,
    /// Fission demand (dependencies).
    pub demand: Vec<String>,
    /// Fission answer (outputs).
    pub answer: Vec<String>,
    /// Source file path.
    pub source_path: String,
}

/// An edge in the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEdge {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub label: String,
}

/// Complete graph data for rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualizerGraph {
    pub nodes: Vec<KnowledgeNode>,
    pub edges: Vec<KnowledgeEdge>,
    pub stats: GraphStats,
}

/// Graph statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphStats {
    pub nodes: usize,
    pub edges: usize,
    pub vertices: usize,
}

// ── Core Entry Point ────────────────────────────────────────────────

/// Generate an interactive HTML triad topology visualizer.
///
/// Reads triad nodes, builds a knowledge graph, and embeds it into a
/// self-contained HTML page.
pub fn generate_triad_visualizer(
    map_file: &Path,
    output_path: &Path,
    options: &VisualizerOptions,
) -> Result<(), anyhow::Error> {
    // ── Load nodes ────────────────────────────────────────────────
    let nodes: Vec<TriadNodeDefinition> = if map_file.exists() {
        let content = std::fs::read_to_string(map_file)?;
        let trimmed = content.trim().trim_start_matches('\u{FEFF}');
        serde_json::from_str(trimmed).unwrap_or_default()
    } else {
        Vec::new()
    };

    // ── Build graph ───────────────────────────────────────────────
    let graph = build_knowledge_graph(&nodes, options);

    // ── Render HTML ───────────────────────────────────────────────
    let html = render_html(&graph, options);

    // ── Write output ──────────────────────────────────────────────
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, html)?;

    Ok(())
}

// ── Graph Building ──────────────────────────────────────────────────

/// Build a knowledge graph from triad node definitions.
fn build_knowledge_graph(
    nodes: &[TriadNodeDefinition],
    options: &VisualizerOptions,
) -> VisualizerGraph {
    let mut graph_nodes = Vec::new();
    let mut graph_edges = Vec::new();
    let mut node_ids = std::collections::HashSet::new();

    let limit = nodes.len().min(options.max_render_nodes);

    // ── First pass: build all nodes and collect ids ─────────────
    for node in nodes.iter().take(limit) {
        let fission = node.fission.as_ref();
        let problem = fission.map(|f| f.problem.clone()).unwrap_or_default();
        let demand = fission.map(|f| f.demand.clone()).unwrap_or_default();
        let answer = fission.map(|f| f.answer.clone()).unwrap_or_default();

        // Determine group/category for color coding
        let group = categorize_node(&node.node_id, &problem);

        graph_nodes.push(KnowledgeNode {
            id: node.node_id.clone(),
            label: format_label(&node.node_id, &problem),
            group,
            lifecycle: "existing".into(),
            problem,
            demand: demand.clone(),
            answer: answer.clone(),
            source_path: node.source_path.clone().unwrap_or_default(),
        });

        node_ids.insert(node.node_id.clone());
    }

    // ── Second pass: build edges (now all node_ids are known) ──
    let empty_demand = Vec::new();
    for node in nodes.iter().take(limit) {
        let demand = node
            .fission
            .as_ref()
            .map(|f| &f.demand)
            .unwrap_or(&empty_demand);

        for dep in demand {
            // Skip generic/ghost dependencies
            if dep.starts_with("[Ghost:") || is_generic_type(dep) {
                continue;
            }
            // Only create edge if the target exists in our node set
            if node_ids.contains(dep.as_str()) {
                graph_edges.push(KnowledgeEdge {
                    from: node.node_id.clone(),
                    to: dep.clone(),
                    label: String::new(),
                });
            }
        }
    }

    // Apply edge limit
    if graph_edges.len() > options.max_render_edges {
        graph_edges.truncate(options.max_render_edges);
    }

    // Filter isolated nodes if configured
    if !options.show_isolated_capabilities {
        let connected_nodes: std::collections::HashSet<&str> = graph_edges
            .iter()
            .flat_map(|e| [e.from.as_str(), e.to.as_str()])
            .collect();

        graph_nodes.retain(|n| connected_nodes.contains(n.id.as_str()));
    }

    let stats = GraphStats {
        nodes: graph_nodes.len(),
        edges: graph_edges.len(),
        vertices: graph_nodes.len(),
    };

    VisualizerGraph {
        nodes: graph_nodes,
        edges: graph_edges,
        stats,
    }
}

// ── HTML Rendering ──────────────────────────────────────────────────

/// Render the complete HTML page with embedded graph data.
fn render_html(graph: &VisualizerGraph, _options: &VisualizerOptions) -> String {
    let graph_json = serde_json::to_string(graph).unwrap_or_default();

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>TriadMind — Architecture Topology</title>
<script src="https://unpkg.com/vis-network@9.1.6/standalone/umd/vis-network.min.js"></script>
<style>
  body {{ margin: 0; padding: 0; font-family: system-ui, sans-serif; background: #0d1117; color: #c9d1d9; }}
  #header {{ padding: 12px 16px; background: #161b22; border-bottom: 1px solid #30363d; display: flex; align-items: center; gap: 16px; }}
  #header h1 {{ margin: 0; font-size: 18px; font-weight: 600; }}
  #stats {{ font-size: 12px; color: #8b949e; }}
  #stats span {{ margin-right: 12px; }}
  #container {{ width: 100%; height: calc(100vh - 50px); }}
  .legend {{ display: flex; gap: 12px; font-size: 11px; }}
  .legend-item {{ display: flex; align-items: center; gap: 4px; }}
  .legend-dot {{ width: 10px; height: 10px; border-radius: 50%; }}
</style>
</head>
<body>
<div id="header">
  <h1>🧠 TriadMind Topology</h1>
  <div id="stats">
    <span>Nodes: {nodes}</span>
    <span>Edges: {edges}</span>
    <span>Vertices: {vertices}</span>
  </div>
  <div class="legend">
    <div class="legend-item"><div class="legend-dot" style="background:#58a6ff"></div> core</div>
    <div class="legend-item"><div class="legend-dot" style="background:#3fb950"></div> service</div>
    <div class="legend-item"><div class="legend-dot" style="background:#d29922"></div> adapter</div>
    <div class="legend-item"><div class="legend-dot" style="background:#f78166"></div> handler</div>
    <div class="legend-item"><div class="legend-dot" style="background:#8b949e"></div> other</div>
  </div>
</div>
<div id="container"></div>
<script>
const graphData = {graph_json};

const nodes = new vis.DataSet(graphData.nodes.map(n => ({{
  id: n.id,
  label: n.label,
  group: n.group,
  title: `<b>${{n.id}}</b><br/>${{n.problem}}<br/><br/>Demand: ${{n.demand.join(', ') || 'none'}}<br/>Answer: ${{n.answer.join(', ') || 'none'}}<br/>Source: ${{n.source_path}}`,
  color: getGroupColor(n.group),
}})));

const edges = new vis.DataSet(graphData.edges.map(e => ({{
  from: e.from,
  to: e.to,
  arrows: 'to',
  color: {{ color: '#30363d', highlight: '#58a6ff' }},
}})));

const container = document.getElementById('container');
const network = new vis.Network(container, {{ nodes, edges }}, {{
  physics: {{ solver: 'forceAtlas2Based', forceAtlas2Based: {{ gravitationalConstant: -50, centralGravity: 0.01 }} }},
  groups: {{
    core: {{ color: {{ background: '#58a6ff', border: '#1f6feb' }} }},
    service: {{ color: {{ background: '#3fb950', border: '#238636' }} }},
    adapter: {{ color: {{ background: '#d29922', border: '#9e6a03' }} }},
    handler: {{ color: {{ background: '#f78166', border: '#c93c37' }} }},
    other: {{ color: {{ background: '#8b949e', border: '#6e7681' }} }},
  }},
  edges: {{ smooth: {{ type: 'continuous' }} }},
  interaction: {{ hover: true, tooltipDelay: 200 }},
}});

function getGroupColor(group) {{
  const colors = {{
    core: {{ background: '#58a6ff', border: '#1f6feb' }},
    service: {{ background: '#3fb950', border: '#238636' }},
    adapter: {{ background: '#d29922', border: '#9e6a03' }},
    handler: {{ background: '#f78166', border: '#c93c37' }},
  }};
  return colors[group] || {{ background: '#8b949e', border: '#6e7681' }};
}}
</script>
</body>
</html>"#,
        nodes = graph.stats.nodes,
        edges = graph.stats.edges,
        vertices = graph.stats.vertices,
        graph_json = graph_json,
    )
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Categorize a node into a group for color coding.
fn categorize_node(node_id: &str, problem: &str) -> String {
    let combined = format!("{} {}", node_id.to_lowercase(), problem.to_lowercase());

    if combined.contains("handle")
        || combined.contains("handler")
        || combined.contains("controller")
        || combined.contains("route")
    {
        "handler".into()
    } else if combined.contains("service") || combined.contains("usecase") || combined.contains("use_case") {
        "service".into()
    } else if combined.contains("adapter")
        || combined.contains("db")
        || combined.contains("repo")
        || combined.contains("storage")
        || combined.contains("gateway")
    {
        "adapter".into()
    } else if combined.contains("core") || combined.contains("domain") || combined.contains("engine") {
        "core".into()
    } else {
        "other".into()
    }
}

/// Format a human-readable label for a node.
fn format_label(node_id: &str, problem: &str) -> String {
    let parts: Vec<&str> = node_id.split('.').collect();
    let short_name = parts.last().copied().unwrap_or(node_id);
    if problem.len() > 40 {
        format!("{}\n{}…", short_name, &problem[..40])
    } else if problem.is_empty() {
        short_name.to_string()
    } else {
        format!("{}\n{}", short_name, problem)
    }
}

/// Check if a type name is a generic/low-value contract.
fn is_generic_type(type_name: &str) -> bool {
    let generics = [
        "str", "string", "int", "i32", "i64", "u32", "u64", "f32", "f64", "usize", "isize",
        "number", "bool", "boolean", "float",
        "void", "()", "any", "unknown", "object", "array", "list",
        "option", "result", "vec", "hashmap", "json", "request", "response",
        "promise", "future",
    ];
    let lower = type_name.to_lowercase();
    generics.contains(&lower.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id: &str, problem: &str, demand: &[&str], answer: &[&str]) -> TriadNodeDefinition {
        TriadNodeDefinition {
            node_id: id.into(),
            category: Some("core".into()),
            source_path: Some("src/main.rs".into()),
            lifecycle: None,
            fission: Some(crate::protocol::TriadFission {
                problem: problem.into(),
                demand: demand.iter().map(|s| s.to_string()).collect(),
                answer: answer.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }

    #[test]
    fn test_build_knowledge_graph() {
        let nodes = vec![
            make_node("Handler.run", "handle request", &["Service.process"], &["Response"]),
            make_node("Service.process", "process data", &["String"], &["Result"]),
        ];
        let opts = VisualizerOptions::default();
        let graph = build_knowledge_graph(&nodes, &opts);
        assert_eq!(graph.stats.nodes, 2);
        assert!(graph.edges.len() >= 1);
    }

    #[test]
    fn test_categorize_node() {
        assert_eq!(categorize_node("HttpHandler.run", "handle"), "handler");
        assert_eq!(categorize_node("UserService.create", "service"), "service");
        assert_eq!(categorize_node("DatabaseAdapter.connect", "adapter"), "adapter");
        assert_eq!(categorize_node("Core.engine", "core engine"), "core");
        assert_eq!(categorize_node("Utils.helper", "helper"), "other");
    }

    #[test]
    fn test_is_generic_type() {
        assert!(is_generic_type("String"));
        assert!(is_generic_type("i32"));
        assert!(is_generic_type("bool"));
        assert!(!is_generic_type("UserService"));
        assert!(!is_generic_type("MyCustomType"));
    }

    #[test]
    fn test_render_html_contains_graph_data() {
        let nodes = vec![make_node("Test.run", "test", &[], &[])];
        let mut opts = VisualizerOptions::default();
        opts.show_isolated_capabilities = true;
        let graph = build_knowledge_graph(&nodes, &opts);
        let html = render_html(&graph, &opts);
        assert!(html.contains("TriadMind"));
        assert!(html.contains("Test.run"));
        assert!(html.contains("vis-network"));
    }
}
