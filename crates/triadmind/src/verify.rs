//! # Verify — Topology quality metrics and threshold checking
//!
//! Ported from triadmind-core/verify.ts
//!
//! Computes quality metrics from a parsed triad-map.json and checks them
//! against configurable thresholds. Produces a structured verify report.
//!
//! @LeftBranch: run_topology_verify, compute_verify_metrics
//! @RightBranch: VerifyMetrics, VerifyThresholds, VerifyReport

use std::collections::HashMap;
// PathBuf unused, removed

use serde::{Deserialize, Serialize};

use crate::protocol::TriadNodeDefinition;

// ── Metrics ─────────────────────────────────────────────────────────

/// Computed topology quality metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyMetrics {
    /// Total number of triad nodes.
    pub triad_nodes: usize,
    /// Distinct vertex (nodeId) count.
    pub triad_vertices: usize,
    /// Count of nodes whose nodeId matches /execute/i.
    pub execute_like_count: usize,
    /// execute_like_count / triad_nodes.
    pub execute_like_ratio: f64,
    /// Number of nodes with ghost demand ([Ghost:...] patterns).
    pub ghost_nodes: usize,
    /// ghost_nodes / triad_nodes.
    pub ghost_ratio: f64,
    /// Ghost nodes by language.
    pub ghost_ratio_by_language: HashMap<String, f64>,
    /// Ghost-in-demand count by language.
    pub ghost_in_demand_count_by_language: HashMap<String, usize>,
    /// Nodes with empty fission (no problem defined).
    pub empty_vertices: usize,
    /// Nodes with left-branch only (actions/outputs but no demands).
    pub left_only_vertices: usize,
    /// Nodes with right-branch only (demands but no actions/outputs).
    pub right_only_vertices: usize,
}

impl Default for VerifyMetrics {
    fn default() -> Self {
        Self {
            triad_nodes: 0,
            triad_vertices: 0,
            execute_like_count: 0,
            execute_like_ratio: 0.0,
            ghost_nodes: 0,
            ghost_ratio: 0.0,
            ghost_ratio_by_language: HashMap::new(),
            ghost_in_demand_count_by_language: HashMap::new(),
            empty_vertices: 0,
            left_only_vertices: 0,
            right_only_vertices: 0,
        }
    }
}

// ── Thresholds ──────────────────────────────────────────────────────

/// Configurable verification thresholds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyThresholds {
    /// Maximum allowed execute_like_ratio.
    pub execute_like_ratio: f64,
    /// Maximum allowed ghost_ratio.
    pub ghost_ratio: f64,
    /// Whether ghost policy must be compliant.
    pub ghost_policy_compliance: bool,
    /// Maximum empty vertices.
    pub empty_vertices: usize,
    /// Maximum left-only vertices.
    pub left_only_vertices: usize,
    /// Maximum right-only vertices.
    pub right_only_vertices: usize,
}

impl Default for VerifyThresholds {
    fn default() -> Self {
        Self {
            execute_like_ratio: 0.1,
            ghost_ratio: 0.4,
            ghost_policy_compliance: true,
            empty_vertices: 0,
            left_only_vertices: 10,
            right_only_vertices: 10,
        }
    }
}

// ── Check Result ────────────────────────────────────────────────────

/// Result of a single threshold check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyCheckResult {
    /// The threshold key being checked.
    pub key: String,
    /// Pass / fail / skip.
    pub status: String,
    /// Expected threshold value (as string for display).
    pub expected: String,
    /// Actual computed value (as string for display).
    pub actual: String,
    /// Human-readable detail.
    pub detail: String,
}

// ── Report ──────────────────────────────────────────────────────────

/// Complete verification report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReport {
    /// ISO 8601 generation timestamp.
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    /// Project root directory.
    #[serde(rename = "projectRoot")]
    pub project_root: String,
    /// Whether strict mode was used.
    pub strict: bool,
    /// Thresholds used for this run.
    pub thresholds: VerifyThresholds,
    /// Computed metrics.
    pub metrics: VerifyMetrics,
    /// Individual check results.
    pub checks: Vec<VerifyCheckResult>,
    /// Overall pass/fail.
    pub passed: bool,
    /// Path to the triad-map.json used.
    #[serde(rename = "mapFile")]
    pub map_file: String,
}

// ── Options ─────────────────────────────────────────────────────────

/// Options for running topology verification.
#[derive(Debug, Clone)]
pub struct VerifyOptions {
    /// Strict mode: fail on any threshold breach.
    pub strict: bool,
    /// Custom max execute-like ratio (overrides threshold).
    pub max_execute_like_ratio: Option<f64>,
    /// Custom max ghost ratio (overrides threshold).
    pub max_ghost_ratio: Option<f64>,
}

impl Default for VerifyOptions {
    fn default() -> Self {
        Self {
            strict: false,
            max_execute_like_ratio: None,
            max_ghost_ratio: None,
        }
    }
}

// ── Patterns ────────────────────────────────────────────────────────

/// Pattern for detecting execute-like method names.
const EXECUTE_LIKE_PATTERN: &str = "execute";
/// Pattern for ghost demand entries.
const GHOST_DEMAND_PREFIX: &str = "[Ghost:";

// ── Core Computation ────────────────────────────────────────────────

/// Compute all verify metrics from a set of triad nodes.
pub fn compute_verify_metrics(nodes: &[TriadNodeDefinition]) -> VerifyMetrics {
    let triad_nodes = nodes.len();

    // Distinct vertex count by nodeId
    let mut seen_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for node in nodes {
        seen_ids.insert(&node.node_id);
    }
    let triad_vertices = seen_ids.len();

    // Execute-like detection
    let execute_like_count = nodes
        .iter()
        .filter(|n| {
            n.node_id
                .to_lowercase()
                .contains(&EXECUTE_LIKE_PATTERN.to_lowercase())
        })
        .count();
    let execute_like_ratio = if triad_nodes > 0 {
        execute_like_count as f64 / triad_nodes as f64
    } else {
        0.0
    };

    // Ghost node detection
    let ghost_nodes = count_ghost_nodes(nodes);
    let ghost_ratio = if triad_nodes > 0 {
        ghost_nodes as f64 / triad_nodes as f64
    } else {
        0.0
    };

    // Ghost ratio by language
    let ghost_ratio_by_language = compute_ghost_by_language(nodes);

    // Ghost-in-demand by language
    let ghost_in_demand_count_by_language = compute_ghost_in_demand_by_language(nodes);

    // Empty / left-only / right-only vertices
    let mut empty_vertices = 0usize;
    let mut left_only_vertices = 0usize;
    let mut right_only_vertices = 0usize;

    for node in nodes {
        match &node.fission {
            None => {
                empty_vertices += 1;
            }
            Some(f) => {
                let has_problem = !f.problem.trim().is_empty();
                let has_demand = !f.demand.is_empty();
                let has_answer = !f.answer.is_empty();

                if !has_problem && !has_demand && !has_answer {
                    empty_vertices += 1;
                } else if has_answer && !has_demand {
                    // Left branch: produces output without needing external input
                    left_only_vertices += 1;
                } else if has_demand && !has_answer {
                    // Right branch: needs input but produces no output
                    right_only_vertices += 1;
                }
            }
        }
    }

    VerifyMetrics {
        triad_nodes,
        triad_vertices,
        execute_like_count,
        execute_like_ratio,
        ghost_nodes,
        ghost_ratio,
        ghost_ratio_by_language,
        ghost_in_demand_count_by_language,
        empty_vertices,
        left_only_vertices,
        right_only_vertices,
    }
}

/// Count nodes that have ghost demand entries.
fn count_ghost_nodes(nodes: &[TriadNodeDefinition]) -> usize {
    nodes
        .iter()
        .filter(|n| has_ghost_demand(n))
        .count()
}

/// Check if a node has ghost demand (any demand entry starting with [Ghost:).
fn has_ghost_demand(node: &TriadNodeDefinition) -> bool {
    node.fission
        .as_ref()
        .map(|f| {
            f.demand
                .iter()
                .any(|d| d.trim().starts_with(GHOST_DEMAND_PREFIX))
        })
        .unwrap_or(false)
}

/// Compute ghost ratio broken down by inferred language from source path.
fn compute_ghost_by_language(nodes: &[TriadNodeDefinition]) -> HashMap<String, f64> {
    let mut total_by_lang: HashMap<String, usize> = HashMap::new();
    let mut ghost_by_lang: HashMap<String, usize> = HashMap::new();

    for node in nodes {
        let lang = infer_language_from_path(node.source_path.as_deref().unwrap_or(""));
        *total_by_lang.entry(lang.clone()).or_insert(0) += 1;
        if has_ghost_demand(node) {
            *ghost_by_lang.entry(lang.clone()).or_insert(0) += 1;
        }
    }

    let mut result = HashMap::new();
    for (lang, total) in &total_by_lang {
        let ghost = ghost_by_lang.get(lang).copied().unwrap_or(0);
        let ratio = if *total > 0 {
            ghost as f64 / *total as f64
        } else {
            0.0
        };
        result.insert(lang.clone(), ratio);
    }
    result
}

/// Count ghost-in-demand nodes by language.
fn compute_ghost_in_demand_by_language(nodes: &[TriadNodeDefinition]) -> HashMap<String, usize> {
    let mut result = HashMap::new();
    for node in nodes {
        if has_ghost_demand(node) {
            let lang = infer_language_from_path(node.source_path.as_deref().unwrap_or(""));
            *result.entry(lang).or_insert(0) += 1;
        }
    }
    result
}

/// Infer language from a source file path extension.
fn infer_language_from_path(source_path: &str) -> String {
    let lower = source_path.to_lowercase();
    if lower.ends_with(".ts") || lower.ends_with(".tsx") || lower.ends_with(".mts") || lower.ends_with(".cts") {
        "typescript".into()
    } else if lower.ends_with(".js") || lower.ends_with(".jsx") || lower.ends_with(".mjs") || lower.ends_with(".cjs") {
        "javascript".into()
    } else if lower.ends_with(".py") {
        "python".into()
    } else if lower.ends_with(".go") {
        "go".into()
    } else if lower.ends_with(".rs") {
        "rust".into()
    } else if lower.ends_with(".cpp") || lower.ends_with(".cc") || lower.ends_with(".cxx") || lower.ends_with(".hpp") || lower.ends_with(".h") {
        "cpp".into()
    } else if lower.ends_with(".java") {
        "java".into()
    } else {
        "unknown".into()
    }
}

// ── Threshold Checking ──────────────────────────────────────────────

/// Run all threshold checks against computed metrics.
pub fn run_checks(metrics: &VerifyMetrics, thresholds: &VerifyThresholds) -> Vec<VerifyCheckResult> {
    vec![
        check_numeric_le(
            "execute_like_ratio",
            metrics.execute_like_ratio,
            thresholds.execute_like_ratio,
            &format!(
                "execute_like_count={}, triad_nodes={}, ratio={:.4}",
                metrics.execute_like_count, metrics.triad_nodes, metrics.execute_like_ratio
            ),
        ),
        check_numeric_le(
            "ghost_ratio",
            metrics.ghost_ratio,
            thresholds.ghost_ratio,
            &format!(
                "ghost_nodes={}, triad_nodes={}, ratio={:.4}",
                metrics.ghost_nodes, metrics.triad_nodes, metrics.ghost_ratio
            ),
        ),
        check_numeric_le(
            "empty_vertices",
            metrics.empty_vertices as f64,
            thresholds.empty_vertices as f64,
            &format!("empty_vertices={}", metrics.empty_vertices),
        ),
        check_numeric_le(
            "left_only_vertices",
            metrics.left_only_vertices as f64,
            thresholds.left_only_vertices as f64,
            &format!("left_only_vertices={}", metrics.left_only_vertices),
        ),
        check_numeric_le(
            "right_only_vertices",
            metrics.right_only_vertices as f64,
            thresholds.right_only_vertices as f64,
            &format!("right_only_vertices={}", metrics.right_only_vertices),
        ),
        check_ghost_policy(
            metrics,
            thresholds.ghost_policy_compliance,
        ),
    ]
}

fn check_numeric_le(key: &str, actual: f64, threshold: f64, detail: &str) -> VerifyCheckResult {
    let status = if actual <= threshold { "pass" } else { "fail" };
    VerifyCheckResult {
        key: key.into(),
        status: status.into(),
        expected: format!("<={threshold}"),
        actual: format!("{actual:.4}"),
        detail: detail.into(),
    }
}

fn check_ghost_policy(metrics: &VerifyMetrics, must_comply: bool) -> VerifyCheckResult {
    let total_ghost: usize = metrics.ghost_in_demand_count_by_language.values().sum();
    let detail = format!(
        "ghost_in_demand_by_language={:?}, total={}",
        metrics.ghost_in_demand_count_by_language, total_ghost
    );

    if !must_comply {
        return VerifyCheckResult {
            key: "ghost_policy_compliance".into(),
            status: "skip".into(),
            expected: "compliance not required".into(),
            actual: format!("{total_ghost} ghost nodes"),
            detail,
        };
    }

    // Ghost policy compliance: no ghost nodes with include_in_demand=true
    // This is a simplified check; full implementation requires config context
    let status = if total_ghost == 0 { "pass" } else { "fail" };
    VerifyCheckResult {
        key: "ghost_policy_compliance".into(),
        status: status.into(),
        expected: "0 ghost nodes".into(),
        actual: format!("{total_ghost} ghost nodes"),
        detail,
    }
}

// ── Main Entry Point ────────────────────────────────────────────────

/// Run topology verification on a parsed triad-map.
///
/// Reads nodes from the triad-map JSON, computes metrics, runs threshold
/// checks, and produces a `VerifyReport`.
pub fn run_topology_verify(
    map_file: &str,
    project_root: &str,
    nodes: &[TriadNodeDefinition],
    options: &VerifyOptions,
) -> VerifyReport {
    let mut thresholds = VerifyThresholds::default();

    if let Some(ratio) = options.max_execute_like_ratio {
        thresholds.execute_like_ratio = ratio;
    }
    if let Some(ratio) = options.max_ghost_ratio {
        thresholds.ghost_ratio = ratio;
    }

    let metrics = compute_verify_metrics(nodes);
    let checks = run_checks(&metrics, &thresholds);

    let passed = if options.strict {
        checks.iter().all(|c| c.status != "fail")
    } else {
        // Non-strict: only fail on critical checks
        checks
            .iter()
            .filter(|c| c.key == "empty_vertices" || c.key == "ghost_policy_compliance")
            .all(|c| c.status != "fail")
    };

    VerifyReport {
        generated_at: crate::sync::chrono_now(), /* reuse sync's chrono_now */
        project_root: project_root.into(),
        strict: options.strict,
        thresholds,
        metrics,
        checks,
        passed,
        map_file: map_file.into(),
    }
}

/// Convenience: load triad-map.json, compute metrics, run verify.
pub fn run_verify_from_file(
    map_path: &str,
    project_root: &str,
    options: &VerifyOptions,
) -> Result<VerifyReport, std::io::Error> {
    let content = std::fs::read_to_string(map_path)?;
    let trimmed = content.trim().trim_start_matches('\u{FEFF}');
    let nodes: Vec<TriadNodeDefinition> =
        serde_json::from_str(trimmed).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;

    Ok(run_topology_verify(map_path, project_root, &nodes, options))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::TriadFission;

    fn make_node(id: &str, problem: &str, demand: &[&str], answer: &[&str]) -> TriadNodeDefinition {
        TriadNodeDefinition {
            node_id: id.into(),
            category: Some("core".into()),
            source_path: Some("src/main.rs".into()),
            lifecycle: None,
            fission: Some(TriadFission {
                problem: problem.into(),
                demand: demand.iter().map(|s| s.to_string()).collect(),
                answer: answer.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }

    #[test]
    fn test_empty_nodes_metrics() {
        let metrics = compute_verify_metrics(&[]);
        assert_eq!(metrics.triad_nodes, 0);
        assert_eq!(metrics.triad_vertices, 0);
        assert_eq!(metrics.execute_like_ratio, 0.0);
        assert_eq!(metrics.ghost_ratio, 0.0);
    }

    #[test]
    fn test_execute_like_detection() {
        let nodes = vec![
            make_node("Service.execute", "run", &[], &["result"]),
            make_node("Service.handle", "handle", &["req"], &["res"]),
            make_node("Worker.executeTask", "work", &[], &[]),
        ];
        let metrics = compute_verify_metrics(&nodes);
        assert_eq!(metrics.execute_like_count, 2);
        assert!((metrics.execute_like_ratio - 2.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn test_ghost_detection() {
        let nodes = vec![
            make_node("A.run", "do", &["[Ghost:dep1]"], &[]),
            make_node("B.run", "do", &["normal_dep"], &["result"]),
            make_node("C.run", "do", &["[Ghost:x]", "[Ghost:y]"], &[]),
        ];
        let metrics = compute_verify_metrics(&nodes);
        assert_eq!(metrics.ghost_nodes, 2);
    }

    #[test]
    fn test_empty_and_branch_vertices() {
        let nodes = vec![
            // Empty fission
            TriadNodeDefinition {
                node_id: "Empty.one".into(),
                category: None,
                source_path: None,
                lifecycle: None,
                fission: None,
            },
            // Left-only: has answer (output) but no demand (input)
            make_node("LeftOnly.run", "problem", &[], &["output"]),
            // Right-only: has demand (input) but no answer (output)
            make_node("RightOnly.run", "problem", &["input"], &[]),
            // Complete: has both demand and answer
            make_node("Full.run", "problem", &["input"], &["output"]),
        ];
        let metrics = compute_verify_metrics(&nodes);
        assert_eq!(metrics.empty_vertices, 1); // Empty.one
        assert_eq!(metrics.left_only_vertices, 1); // LeftOnly.run
        assert_eq!(metrics.right_only_vertices, 1); // RightOnly.run
    }

    #[test]
    fn test_all_checks_pass_on_clean_nodes() {
        let nodes = vec![
            make_node("Service.handle", "handle request", &["req"], &["res"]),
            make_node("Repo.find", "find entity", &["id"], &["entity"]),
        ];
        let metrics = compute_verify_metrics(&nodes);
        let thresholds = VerifyThresholds::default();
        let checks = run_checks(&metrics, &thresholds);
        assert!(checks.iter().all(|c| c.status == "pass"));
    }

    #[test]
    fn test_report_generation() {
        let nodes = vec![
            make_node("Svc.run", "do work", &["in"], &["out"]),
        ];
        let report = run_topology_verify(
            "test-map.json",
            "/test/project",
            &nodes,
            &VerifyOptions::default(),
        );
        assert!(!report.generated_at.is_empty());
        assert_eq!(report.map_file, "test-map.json");
        assert_eq!(report.metrics.triad_nodes, 1);
    }
}
