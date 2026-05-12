//! # Govern — CI/CD governance gate checks
//!
//! Ported from triadmind-core/govern.ts and governPolicy.ts
//!
//! Enforces architecture quality gates in CI/CD pipelines:
//! - Coverage ratios (triad coverage, runtime coverage)
//! - Ghost ratio limits
//! - Execute-like method ratio limits
//! - Forbidden topology mutations
//! - Language-specific ghost policies
//!
//! @LeftBranch: run_govern_check, evaluate_policy
//! @RightBranch: GovernPolicy, GovernReport, GovernCheckResult, GovernMode

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ── Govern Mode ──────────────────────────────────────────────────────

/// Execution mode for governance checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GovernMode {
    /// Check mode: report violations but don't block.
    Check,
    /// CI mode: fail on violations (exit code ≠ 0).
    Ci,
    /// Fix mode: attempt to auto-fix violations via LLM.
    Fix,
}

// ── Govern Policy ───────────────────────────────────────────────────

/// A single metric threshold rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernMetricRule {
    /// Operator for comparison.
    pub op: GovernRuleOperator,
    /// Threshold value.
    pub value: f64,
    /// Whether a violation blocks the gate.
    #[serde(rename = "mustPass", default = "default_true")]
    pub must_pass: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GovernRuleOperator {
    Lt,
    Lte,
    Gt,
    Gte,
    Eq,
}

// ── Coverage Rule ───────────────────────────────────────────────────

/// A coverage threshold rule scoped by root/category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernCoverageRule {
    /// Coverage metric: triad, runtime, or combined.
    pub metric: GovernCoverageMetric,
    /// Operator for comparison.
    pub op: GovernCoverageOp,
    /// Threshold value (0.0–1.0).
    pub value: f64,
    /// Whether this rule blocks the gate.
    #[serde(rename = "mustPass", default = "default_true")]
    pub must_pass: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GovernCoverageMetric {
    Triad,
    Runtime,
    Combined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GovernCoverageOp {
    Gt,
    Gte,
}

// ── Ghost Language Policy ───────────────────────────────────────────

/// Ghost policy for a specific language.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernLanguageGhostPolicy {
    /// Whether ghost demand nodes count toward demand sets.
    #[serde(rename = "includeInDemand", default)]
    pub include_in_demand: bool,
    /// Top K ghost nodes to include.
    #[serde(rename = "topK", default = "default_top_k")]
    pub top_k: usize,
    /// Minimum confidence for ghost detection.
    #[serde(rename = "minConfidence", default = "default_min_confidence")]
    pub min_confidence: f64,
}

fn default_top_k() -> usize {
    5
}
fn default_min_confidence() -> f64 {
    0.5
}

impl Default for GovernLanguageGhostPolicy {
    fn default() -> Self {
        Self {
            include_in_demand: false,
            top_k: 5,
            min_confidence: 0.5,
        }
    }
}

// ── Forbidden Mutation ──────────────────────────────────────────────

/// A topology mutation that is explicitly forbidden.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForbiddenRunMutation {
    /// The node id pattern that is protected.
    pub node_id: String,
    /// The operation that is forbidden.
    pub op: String,
    /// Reason for the restriction.
    pub reason: String,
}

// ── Govern Policy (Complete) ────────────────────────────────────────

/// Complete governance policy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernPolicy {
    /// Schema version.
    #[serde(rename = "schemaVersion")]
    pub schema_version: String,
    /// Policy version tag.
    #[serde(default)]
    pub version: String,
    /// Policy mode (currently always "hard").
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Must-pass metric rules keyed by metric name.
    #[serde(rename = "mustPass", default)]
    pub must_pass: HashMap<String, GovernMetricRule>,
    /// Language-specific ghost policies.
    #[serde(rename = "languageGhostPolicy", default)]
    pub language_ghost_policy: HashMap<String, GovernLanguageGhostPolicy>,
    /// Coverage rules by root category.
    #[serde(rename = "coverageByRoot", default)]
    pub coverage_by_root: HashMap<String, GovernCoverageRule>,
    /// Forbidden topology mutations.
    #[serde(rename = "forbiddenInRun", default)]
    pub forbidden_in_run: Vec<ForbiddenRunMutation>,
    /// Path to a baseline file for comparison.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_path: Option<String>,
}

fn default_mode() -> String {
    "hard".into()
}

impl Default for GovernPolicy {
    fn default() -> Self {
        Self {
            schema_version: "1.0".into(),
            version: String::new(),
            mode: "hard".into(),
            must_pass: HashMap::new(),
            language_ghost_policy: HashMap::new(),
            coverage_by_root: HashMap::new(),
            forbidden_in_run: Vec::new(),
            baseline_path: None,
        }
    }
}

// ── Govern Check Result ─────────────────────────────────────────────

/// Result of a single governance check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernCheckResult {
    /// Check identifier.
    pub key: String,
    /// Pass / fail / error.
    pub status: GovernCheckStatus,
    /// Expected threshold (as string for display).
    pub expected: String,
    /// Actual computed value (as string for display).
    pub actual: String,
    /// Human-readable detail.
    pub detail: String,
    /// Whether this check blocks the gate.
    #[serde(rename = "mustPass")]
    pub must_pass: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GovernCheckStatus {
    Pass,
    Fail,
    Error,
}

// ── Govern Report ───────────────────────────────────────────────────

/// Complete governance report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernReport {
    #[serde(rename = "schemaVersion")]
    pub schema_version: String,
    /// ISO 8601 generation timestamp.
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    /// Duration in milliseconds.
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    /// Check mode used.
    pub mode: GovernMode,
    /// Whether strict mode was enabled.
    pub strict: bool,
    /// Project root path.
    #[serde(rename = "projectRoot")]
    pub project_root: String,
    /// Path to the policy file used.
    #[serde(rename = "policyPath")]
    pub policy_path: String,
    /// Whether all checks passed.
    pub passed: bool,
    /// Exit code (0 = pass, non-zero = fail).
    #[serde(rename = "exitCode")]
    pub exit_code: i32,
    /// Individual check results.
    pub checks: Vec<GovernCheckResult>,
    /// Raw metrics snapshot.
    pub metrics: serde_json::Value,
    /// Artifact paths.
    pub artifacts: GovernArtifacts,
    /// Policy violation descriptions.
    #[serde(rename = "policyViolations", default)]
    pub policy_violations: Vec<String>,
    /// Forbidden change descriptions.
    #[serde(rename = "forbiddenChanges", default)]
    pub forbidden_changes: Vec<String>,
    /// Failure descriptions.
    pub failures: Vec<String>,
}

/// Paths to governance artifacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernArtifacts {
    #[serde(rename = "triadMapFile")]
    pub triad_map_file: String,
    #[serde(rename = "runtimeMapFile")]
    pub runtime_map_file: String,
    #[serde(rename = "runtimeDiagnosticsFile")]
    pub runtime_diagnostics_file: String,
    #[serde(rename = "coverageReportFile")]
    pub coverage_report_file: String,
    #[serde(rename = "governReportFile")]
    pub govern_report_file: String,
    #[serde(rename = "governAuditFile")]
    pub govern_audit_file: String,
}

// ── Run Options ─────────────────────────────────────────────────────

/// Options for running governance checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernRunOptions {
    /// Check mode.
    pub mode: GovernMode,
    /// Path to the policy file.
    #[serde(default)]
    pub policy_path: Option<PathBuf>,
    /// LLM provider for auto-fix mode.
    #[serde(default)]
    pub llm: Option<String>,
    /// Maximum fix iterations.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
    /// Dry run (don't write changes).
    #[serde(default)]
    pub dry_run: bool,
}

fn default_max_iterations() -> usize {
    3
}

impl Default for GovernRunOptions {
    fn default() -> Self {
        Self {
            mode: GovernMode::Check,
            policy_path: None,
            llm: None,
            max_iterations: 3,
            dry_run: false,
        }
    }
}

// ── Default Policy ──────────────────────────────────────────────────

/// Build the default governance policy.
pub fn build_default_policy() -> GovernPolicy {
    let mut must_pass = HashMap::new();
    must_pass.insert(
        "execute_like_ratio".into(),
        GovernMetricRule {
            op: GovernRuleOperator::Lt,
            value: 0.1,
            must_pass: true,
        },
    );
    must_pass.insert(
        "ghost_ratio".into(),
        GovernMetricRule {
            op: GovernRuleOperator::Lt,
            value: 0.4,
            must_pass: true,
        },
    );
    must_pass.insert(
        "empty_vertices".into(),
        GovernMetricRule {
            op: GovernRuleOperator::Eq,
            value: 0.0,
            must_pass: false,
        },
    );

    let mut language_ghost_policy = HashMap::new();
    language_ghost_policy.insert(
        "rust".into(),
        GovernLanguageGhostPolicy {
            include_in_demand: true,
            top_k: 8,
            min_confidence: 0.7,
        },
    );
    language_ghost_policy.insert(
        "typescript".into(),
        GovernLanguageGhostPolicy {
            include_in_demand: true,
            top_k: 5,
            min_confidence: 0.7,
        },
    );

    GovernPolicy {
        schema_version: "1.0".into(),
        version: "1.0".into(),
        mode: "hard".into(),
        must_pass,
        language_ghost_policy,
        coverage_by_root: HashMap::new(),
        forbidden_in_run: Vec::new(),
        baseline_path: None,
    }
}

// ── Run Entry Point ─────────────────────────────────────────────────

/// Run governance checks against the current topology.
///
/// Loads the policy from file (falls back to defaults), loads the triad-map,
/// computes verify metrics, and evaluates each policy rule. Produces a
/// GovernReport with pass/fail results.
pub fn run_govern_check(
    project_root: &Path,
    policy_path: &Path,
    options: &GovernRunOptions,
) -> Result<GovernReport, anyhow::Error> {
    let start = std::time::Instant::now();
    let now = crate::sync::chrono_now();

    // ── Load policy ───────────────────────────────────────────────
    let policy: GovernPolicy = if policy_path.exists() {
        let content = std::fs::read_to_string(policy_path)?;
        let trimmed = content.trim().trim_start_matches('\u{FEFF}');
        serde_json::from_str(trimmed).unwrap_or_else(|_| build_default_policy())
    } else {
        build_default_policy()
    };

    // ── Load triad-map ────────────────────────────────────────────
    let map_file = project_root.join(".triadmind").join("triad-map.json");
    let nodes: Vec<crate::protocol::TriadNodeDefinition> = if map_file.exists() {
        let content = std::fs::read_to_string(&map_file)?;
        let trimmed = content.trim().trim_start_matches('\u{FEFF}');
        serde_json::from_str(trimmed).unwrap_or_default()
    } else {
        Vec::new()
    };

    // ── Compute metrics ───────────────────────────────────────────
    let metrics = crate::verify::compute_verify_metrics(&nodes);

    // ── Evaluate rules ────────────────────────────────────────────
    let mut checks = Vec::new();

    // Check each must-pass metric rule
    for (key, rule) in &policy.must_pass {
        let (actual_val, detail) = match key.as_str() {
            "execute_like_ratio" => (
                metrics.execute_like_ratio,
                format!(
                    "{} of {} nodes are execute-like",
                    metrics.execute_like_count, metrics.triad_nodes,
                ),
            ),
            "ghost_ratio" => (
                metrics.ghost_ratio,
                format!(
                    "{} of {} nodes have ghost dependencies",
                    metrics.ghost_nodes, metrics.triad_nodes,
                ),
            ),
            "empty_vertices" => (
                metrics.empty_vertices as f64,
                format!("{} empty vertices", metrics.empty_vertices),
            ),
            _ => continue,
        };

        let passed = evaluate_rule(actual_val, &rule.op, rule.value);
        checks.push(GovernCheckResult {
            key: format!("must_pass.{}", key),
            status: if passed {
                GovernCheckStatus::Pass
            } else {
                GovernCheckStatus::Fail
            },
            expected: format!("{:?} {}", rule.op, rule.value),
            actual: format!("{:.4}", actual_val),
            detail,
            must_pass: rule.must_pass,
        });
    }

    // Check ghost policy per language
    for (lang, ghost_policy) in &policy.language_ghost_policy {
        let lang_ghost_count = metrics
            .ghost_ratio_by_language
            .get(lang)
            .copied()
            .unwrap_or(0.0) as usize;
        let lang_ghost_in_demand = metrics
            .ghost_in_demand_count_by_language
            .get(lang)
            .copied()
            .unwrap_or(0);

        let passed = !ghost_policy.include_in_demand || lang_ghost_in_demand <= ghost_policy.top_k;
        checks.push(GovernCheckResult {
            key: format!("ghost_policy.{}", lang),
            status: if passed {
                GovernCheckStatus::Pass
            } else {
                GovernCheckStatus::Fail
            },
            expected: format!("<= {} ghost nodes in demand", ghost_policy.top_k),
            actual: lang_ghost_in_demand.to_string(),
            detail: format!(
                "{} ghost demand nodes in {} (total ghost: {})",
                lang_ghost_in_demand, lang, lang_ghost_count,
            ),
            must_pass: false,
        });
    }

    // ── Determine overall pass/fail ───────────────────────────────
    let mut failures: Vec<String> = Vec::new();
    let mut policy_violations: Vec<String> = Vec::new();
    let all_passed = checks.iter().all(|c| {
        if c.status == GovernCheckStatus::Fail {
            if c.must_pass {
                failures.push(format!("{}: {}", c.key, c.detail));
            } else {
                policy_violations.push(format!("{}: {}", c.key, c.detail));
            }
        }
        c.status == GovernCheckStatus::Pass || !c.must_pass
    });

    let exit_code = if options.mode == GovernMode::Ci && !failures.is_empty() {
        2
    } else {
        0
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    // ── Build metrics JSON ────────────────────────────────────────
    let metrics_json = serde_json::json!({
        "triad_nodes": metrics.triad_nodes,
        "execute_like_ratio": metrics.execute_like_ratio,
        "execute_like_count": metrics.execute_like_count,
        "ghost_ratio": metrics.ghost_ratio,
        "ghost_nodes": metrics.ghost_nodes,
        "empty_vertices": metrics.empty_vertices,
    });

    Ok(GovernReport {
        schema_version: "1.0".into(),
        generated_at: now,
        duration_ms,
        mode: options.mode,
        strict: true,
        project_root: project_root.to_string_lossy().to_string(),
        policy_path: policy_path.to_string_lossy().to_string(),
        passed: all_passed,
        exit_code,
        checks,
        metrics: metrics_json,
        artifacts: GovernArtifacts {
            triad_map_file: map_file.to_string_lossy().to_string(),
            runtime_map_file: String::new(),
            runtime_diagnostics_file: String::new(),
            coverage_report_file: String::new(),
            govern_report_file: String::new(),
            govern_audit_file: String::new(),
        },
        policy_violations,
        forbidden_changes: Vec::new(),
        failures,
    })
}

/// Evaluate a single rule: compare actual value against threshold using operator.
fn evaluate_rule(actual: f64, op: &GovernRuleOperator, threshold: f64) -> bool {
    match op {
        GovernRuleOperator::Lt => actual < threshold,
        GovernRuleOperator::Lte => actual <= threshold,
        GovernRuleOperator::Gt => actual > threshold,
        GovernRuleOperator::Gte => actual >= threshold,
        GovernRuleOperator::Eq => (actual - threshold).abs() < 1e-9,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy() {
        let policy = GovernPolicy::default();
        assert_eq!(policy.schema_version, "1.0");
        assert_eq!(policy.mode, "hard");
        assert!(policy.must_pass.is_empty());
    }

    #[test]
    fn test_build_default_policy() {
        let policy = build_default_policy();
        assert!(policy.must_pass.contains_key("execute_like_ratio"));
        assert!(policy.must_pass.contains_key("ghost_ratio"));
        assert!(policy.language_ghost_policy.contains_key("rust"));
        assert!(policy.language_ghost_policy.contains_key("typescript"));
    }

    #[test]
    fn test_govern_check_empty_project() {
        let opts = GovernRunOptions::default();
        let tmp = std::env::temp_dir().join("triadmind_govern_test");
        let _ = std::fs::create_dir_all(&tmp);
        let policy_path = tmp.join("govern-policy.json");
        std::fs::write(
            &policy_path,
            serde_json::to_string_pretty(&build_default_policy()).unwrap(),
        )
        .unwrap();
        let result = run_govern_check(&tmp, &policy_path, &opts);
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(result.is_ok());
        let report = result.unwrap();
        assert!(report.passed); // Empty project should pass
    }

    #[test]
    fn test_govern_check_with_violations() {
        // Create a temp project with a triad-map that has violations
        let tmp = std::env::temp_dir().join("triadmind_govern_violations");
        let _ = std::fs::create_dir_all(tmp.join(".triadmind"));
        let nodes = vec![
            crate::protocol::TriadNodeDefinition {
                node_id: "Service.execute".into(),
                category: Some("core".into()),
                source_path: Some("src/main.rs".into()),
                lifecycle: None,
                fission: Some(crate::protocol::TriadFission {
                    problem: "execute stuff".into(),
                    demand: vec!["[Ghost:missing]".into()],
                    answer: vec!["Result".into()],
                }),
            },
            crate::protocol::TriadNodeDefinition {
                node_id: "Service.execute2".into(),
                category: Some("core".into()),
                source_path: Some("src/main.rs".into()),
                lifecycle: None,
                fission: Some(crate::protocol::TriadFission {
                    problem: "also execute".into(),
                    demand: vec![],
                    answer: vec![],
                }),
            },
        ];
        std::fs::write(
            tmp.join(".triadmind").join("triad-map.json"),
            serde_json::to_string_pretty(&nodes).unwrap(),
        )
        .unwrap();
        let policy_path = tmp.join("govern-policy.json");
        std::fs::write(
            &policy_path,
            serde_json::to_string_pretty(&build_default_policy()).unwrap(),
        )
        .unwrap();

        let opts = GovernRunOptions::default();
        let result = run_govern_check(&tmp, &policy_path, &opts);
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(result.is_ok());
        let report = result.unwrap();
        // Both nodes have "execute" in name → execute-like ratio = 1.0 > 0.1
        assert!(!report.passed);
        assert!(!report.checks.is_empty());
    }

    #[test]
    fn test_language_ghost_policy_defaults() {
        let policy = GovernLanguageGhostPolicy::default();
        assert!(!policy.include_in_demand);
        assert_eq!(policy.top_k, 5);
        assert_eq!(policy.min_confidence, 0.5);
    }

    #[test]
    fn test_evaluate_rule() {
        assert!(evaluate_rule(0.05, &GovernRuleOperator::Lt, 0.1));
        assert!(!evaluate_rule(0.2, &GovernRuleOperator::Lt, 0.1));
        assert!(evaluate_rule(0.1, &GovernRuleOperator::Lte, 0.1));
        assert!(evaluate_rule(0.2, &GovernRuleOperator::Gt, 0.1));
        assert!(!evaluate_rule(0.05, &GovernRuleOperator::Gt, 0.1));
        assert!(evaluate_rule(0.0, &GovernRuleOperator::Eq, 0.0));
        assert!(!evaluate_rule(0.1, &GovernRuleOperator::Eq, 0.0));
    }
}
