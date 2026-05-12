//! # Heal — Runtime error → topology matching → repair protocol
//!
//! Ported from triadmind-core/healing.ts
//!
//! When the agent encounters a runtime error, this module:
//! 1. Parses the error stack trace
//! 2. Matches trace frames to existing triad topology nodes
//! 3. Classifies the diagnosis (contract, missing dependency, etc.)
//! 4. Suggests a healing action (retry, modify, create_child)
//! 5. Generates a healing prompt for LLM-guided fix generation
//!
//! @LeftBranch: diagnose_runtime_failure, build_healing_prompt
//! @RightBranch: HealingDiagnosis, TraceFrame, HealingAction

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::protocol::TriadNodeDefinition;

// ── Trace Frame ─────────────────────────────────────────────────────

/// A single frame from a runtime error stack trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceFrame {
    /// Source file path (relative to project root).
    pub source_path: Option<String>,
    /// Function or method name.
    pub function_name: Option<String>,
    /// Line number.
    pub line: Option<u32>,
    /// Column number.
    pub column: Option<u32>,
    /// Raw frame text.
    pub raw: String,
}

// ── Blast Radius ────────────────────────────────────────────────────

/// Estimated impact radius of a proposed fix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlastRadius {
    /// Risk level.
    pub risk: String,
    /// Number of downstream nodes that would be affected.
    pub downstream_count: usize,
    /// Number of upstream nodes that would be affected.
    pub upstream_count: usize,
}

// ── Healing Diagnosis ───────────────────────────────────────────────

/// Diagnosis of a runtime failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealingDiagnosis {
    /// Project root path.
    #[serde(rename = "projectRoot")]
    pub project_root: String,
    /// Source language.
    #[serde(rename = "adapterLanguage")]
    pub adapter_language: String,
    /// Number of previous retry attempts.
    #[serde(rename = "retryCount")]
    pub retry_count: u32,
    /// ID of the matched topology node, if any.
    #[serde(rename = "matchedNodeId")]
    pub matched_node_id: Option<String>,
    /// Source path of the matched node.
    #[serde(rename = "matchedSourcePath")]
    pub matched_source_path: Option<String>,
    /// Diagnosis category.
    pub diagnosis: String,
    /// Suggested healing action.
    #[serde(rename = "suggestedAction")]
    pub suggested_action: String,
    /// Human-readable summary.
    pub summary: String,
    /// Estimated blast radius.
    #[serde(rename = "blastRadius")]
    pub blast_radius: BlastRadius,
    /// Parsed trace frames.
    #[serde(rename = "traceFrames")]
    pub trace_frames: Vec<TraceFrame>,
    /// Evidence supporting the diagnosis.
    pub evidence: Vec<HealingEvidence>,
    /// Whether human approval is required before applying the fix.
    #[serde(rename = "requiresHumanApproval")]
    pub requires_human_approval: bool,
}

/// Evidence supporting a healing diagnosis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealingEvidence {
    /// Evidence type.
    #[serde(rename = "type")]
    pub evidence_type: String,
    /// Evidence key.
    pub key: String,
    /// Evidence value.
    pub value: String,
}

// ── Core Entry Point ────────────────────────────────────────────────

/// Diagnose a runtime failure by parsing the error text, extracting trace
/// frames, and matching against existing triad topology nodes.
pub fn diagnose_runtime_failure(
    project_root: &Path,
    error_text: &str,
    retry_count: u32,
    nodes: &[TriadNodeDefinition],
) -> HealingDiagnosis {
    let trace_frames = extract_trace_frames(error_text, project_root);
    let matched = locate_best_node_match(&trace_frames, nodes);

    let diagnosis = classify_diagnosis(error_text);
    let blast_radius = estimate_blast_radius(matched.as_ref(), nodes, diagnosis.as_str());
    let suggested_action = choose_suggested_action(&diagnosis, retry_count);
    let summary = build_summary(
        matched.as_ref(),
        &diagnosis,
        &suggested_action,
        &blast_radius,
    );

    let evidence = build_evidence(error_text, &trace_frames, matched.as_ref(), &diagnosis);

    HealingDiagnosis {
        project_root: project_root.to_string_lossy().to_string(),
        adapter_language: "unknown".into(),
        retry_count,
        matched_node_id: matched.as_ref().map(|m| m.node.node_id.clone()),
        matched_source_path: matched.as_ref().and_then(|m| m.node.source_path.clone()),
        diagnosis,
        suggested_action,
        summary,
        blast_radius,
        trace_frames,
        evidence,
        requires_human_approval: false,
    }
}

/// Build a healing prompt for LLM-guided fix generation.
pub fn build_healing_prompt(
    project_root: &Path,
    error_text: &str,
    diagnosis: &HealingDiagnosis,
    nodes: &[TriadNodeDefinition],
) -> String {
    let mut sections = Vec::new();

    sections.push(
        "[System]\n\
         You are TriadMind Runtime Self-Healing architect.\n\
         Your task is to match runtime errors to topology nodes and output a strict JSON UpgradeProtocol.\n\
         Prefer `modify` to fix the current node; only use `create_child` when retry budget is exhausted."
            .to_string(),
    );

    sections.push(format!("[Project Root]\n{}", project_root.display()));

    // Topology summary
    let topology_lines: Vec<String> = nodes
        .iter()
        .map(|n| {
            format!(
                "  {} @ {}",
                n.node_id,
                n.source_path.as_deref().unwrap_or("unknown")
            )
        })
        .collect();
    sections.push(format!(
        "[Triad Topology ({} nodes)]\n{}",
        nodes.len(),
        topology_lines.join("\n")
    ));

    sections.push(format!(
        "[Runtime Error]\n```\n{}\n```",
        error_text.trim()
    ));

    sections.push(format!(
        "[Healing Diagnosis]\n```json\n{}\n```",
        serde_json::to_string_pretty(diagnosis).unwrap_or_default()
    ));

    sections.push(
        "[Output Rules]\n\
         - Return UpgradeProtocol JSON only.\n\
         - Use modify for contract/implementation fixes.\n\
         - Use create_child when the node is overloaded.\n\
         - Include fission (problem/demand/answer) for new nodes."
            .to_string(),
    );

    sections.join("\n\n")
}

// ── Trace Parsing ───────────────────────────────────────────────────

/// Extract trace frames from error text.
fn extract_trace_frames(error_text: &str, project_root: &Path) -> Vec<TraceFrame> {
    let mut frames = Vec::new();
    let project_root_str = project_root.to_string_lossy().to_string();

    for line in error_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Match common stack trace patterns:
        //   at foo (src/main.rs:42:10)
        //   at bar (/path/to/file.ts:100:5)
        //   File "src/main.py", line 42, in foo
        //   src/main.rs:42:10
        let frame = parse_trace_line(trimmed, &project_root_str);
        if let Some(frame) = frame {
            frames.push(frame);
        }
    }

    frames
}

/// Parse a single trace line into a TraceFrame.
fn parse_trace_line(line: &str, project_root: &str) -> Option<TraceFrame> {
    // Pattern 1: "at funcName (path:line:col)"
    if let Some(rest) = line.strip_prefix("at ") {
        if let Some((func, path_part)) = rest.split_once(" (") {
            let path_part = path_part.trim_end_matches(')');
            let (source_path, line_no, col_no) = parse_source_location(path_part, project_root);
            return Some(TraceFrame {
                source_path,
                function_name: Some(func.trim().to_string()),
                line: line_no,
                column: col_no,
                raw: line.to_string(),
            });
        }
        // "at path:line:col"
        let (source_path, line_no, col_no) = parse_source_location(rest, project_root);
        if source_path.is_some() {
            return Some(TraceFrame {
                source_path,
                function_name: None,
                line: line_no,
                column: col_no,
                raw: line.to_string(),
            });
        }
    }

    // Pattern 2: "File \"path\", line N, in funcName"
    if line.starts_with("File \"") {
        if let Some(path_end) = line.find('"') {
            let path = &line[6..path_end];
            let rest = &line[path_end + 1..];
            let line_no = rest
                .split(',')
                .find(|s| s.contains("line"))
                .and_then(|s| s.trim().strip_prefix("line "))
                .and_then(|s| s.trim().parse().ok());
            let func_name = line
                .split(',')
                .last()
                .and_then(|s| s.trim().strip_prefix("in "))
                .map(|s| s.trim().to_string());

            let rel_path = make_relative(path.trim(), project_root);
            return Some(TraceFrame {
                source_path: Some(rel_path),
                function_name: func_name,
                line: line_no,
                column: None,
                raw: line.to_string(),
            });
        }
    }

    // Pattern 3: "panicked at path:line:col" (Rust panic messages)
    if let Some(rest) = line.strip_prefix("panicked at ") {
        let (source_path, line_no, col_no) = parse_source_location(rest, project_root);
        if source_path.is_some() {
            return Some(TraceFrame {
                source_path,
                function_name: None,
                line: line_no,
                column: col_no,
                raw: line.to_string(),
            });
        }
    }

    // Pattern 4: "path:line:col" (bare source location)
    if line.contains(':') && !line.starts_with("at ") && !line.starts_with("File ") {
        let (source_path, line_no, col_no) = parse_source_location(line, project_root);
        if source_path.is_some() {
            return Some(TraceFrame {
                source_path,
                function_name: None,
                line: line_no,
                column: col_no,
                raw: line.to_string(),
            });
        }
    }

    None
}

/// Parse a "path:line:col" format string.
fn parse_source_location(location: &str, project_root: &str) -> (Option<String>, Option<u32>, Option<u32>) {
    let parts: Vec<&str> = location.rsplitn(3, ':').collect();
    if parts.len() < 2 {
        return (None, None, None);
    }

    let col = parts.first().and_then(|s| s.trim().parse().ok());
    let line = parts.get(1).and_then(|s| s.trim().parse().ok());
    let path = if parts.len() >= 3 {
        parts[2].trim().to_string()
    } else {
        String::new()
    };

    if path.is_empty() || !path.contains('.') {
        return (None, line, col);
    }

    let rel_path = make_relative(&path, project_root);
    (Some(rel_path), line, col)
}

/// Make a path relative to the project root if possible.
fn make_relative(path: &str, project_root: &str) -> String {
    let normalized_path = path.replace('\\', "/");
    let normalized_root = project_root.replace('\\', "/");
    if let Some(rel) = normalized_path.strip_prefix(&normalized_root) {
        rel.trim_start_matches('/').to_string()
    } else {
        path.to_string()
    }
}

// ── Node Matching ──────────────────────────────────────────────────

/// Best match between a trace frame and a triad node.
struct NodeMatch {
    node: TriadNodeDefinition,
    score: u32,
}

/// Find the best topology node matching the trace frames.
fn locate_best_node_match(frames: &[TraceFrame], nodes: &[TriadNodeDefinition]) -> Option<NodeMatch> {
    let mut best: Option<NodeMatch> = None;

    for frame in frames {
        for node in nodes {
            let score = score_node_match(frame, node);
            if score == 0 {
                continue;
            }
            if best.as_ref().map_or(true, |b| score > b.score) {
                best = Some(NodeMatch {
                    node: node.clone(),
                    score,
                });
            }
        }
    }

    best
}

/// Score how well a trace frame matches a triad node.
fn score_node_match(frame: &TraceFrame, node: &TriadNodeDefinition) -> u32 {
    let mut score = 0u32;

    // Match by source path
    if let (Some(trace_path), Some(node_path)) = (&frame.source_path, &node.source_path) {
        let tp = trace_path.replace('\\', "/").to_lowercase();
        let np = node_path.replace('\\', "/").to_lowercase();
        if tp == np {
            score += 10;
        } else if tp.ends_with(&np) || np.ends_with(&tp) {
            score += 5;
        }
    }

    // Match by function name against node_id
    if let Some(ref func_name) = frame.function_name {
        let fn_lower = func_name.to_lowercase();
        let node_id_lower = node.node_id.to_lowercase();
        if node_id_lower.contains(&fn_lower) {
            score += 8;
        } else if node_id_lower.split('.').any(|part| part == fn_lower) {
            score += 4;
        }
    }

    score
}

// ── Diagnosis Classification ────────────────────────────────────────

/// Classify the type of runtime failure from error text.
fn classify_diagnosis(error_text: &str) -> String {
    let lower = error_text.to_lowercase();

    if lower.contains("cannot find") || lower.contains("not found") || lower.contains("unresolved") {
        "missing_dependency".into()
    } else if lower.contains("type") && (lower.contains("mismatch") || lower.contains("expected")) {
        "type_mismatch".into()
    } else if lower.contains("contract") || lower.contains("interface") || lower.contains("trait") {
        "contract_violation".into()
    } else if lower.contains("timeout") || lower.contains("timed out") {
        "timeout".into()
    } else if lower.contains("panic") || lower.contains("unreachable") {
        "panic".into()
    } else if lower.contains("null") || lower.contains("undefined") || lower.contains("none") {
        "null_reference".into()
    } else {
        "unknown".into()
    }
}

/// Choose a suggested healing action based on diagnosis and retry count.
fn choose_suggested_action(diagnosis: &str, retry_count: u32) -> String {
    match diagnosis {
        "missing_dependency" if retry_count < 3 => "retry_with_corrected_dependency".into(),
        "type_mismatch" | "contract_violation" => "modify_node_contract".into(),
        _ if retry_count >= 3 => "create_child_node".into(),
        _ => "modify_node_implementation".into(),
    }
}

// ── Blast Radius ────────────────────────────────────────────────────

/// Estimate how many nodes would be affected by a change to the matched node.
fn estimate_blast_radius(
    matched: Option<&NodeMatch>,
    nodes: &[TriadNodeDefinition],
    _diagnosis: &str,
) -> BlastRadius {
    let target_id = matched.map(|m| &m.node.node_id);

    let downstream = nodes
        .iter()
        .filter(|n| {
            n.fission
                .as_ref()
                .map(|f| {
                    f.demand
                        .iter()
                        .any(|d| target_id.map_or(false, |tid| d.contains(tid.as_str())))
                })
                .unwrap_or(false)
        })
        .count();

    let upstream = target_id
        .map(|tid| {
            nodes
                .iter()
                .filter(|n| {
                    n.node_id != *tid
                        && n.fission
                            .as_ref()
                            .map(|f| f.answer.iter().any(|a| a.contains(tid.as_str())))
                            .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);

    let risk = if downstream > 10 || upstream > 10 {
        "high"
    } else if downstream > 3 || upstream > 3 {
        "medium"
    } else {
        "low"
    };

    BlastRadius {
        risk: risk.into(),
        downstream_count: downstream,
        upstream_count: upstream,
    }
}

// ── Summary & Evidence ──────────────────────────────────────────────

fn build_summary(
    matched: Option<&NodeMatch>,
    diagnosis: &str,
    action: &str,
    blast: &BlastRadius,
) -> String {
    if let Some(m) = matched {
        format!(
            "{}: {} (score={}, blast={}, {} downstream, action={})",
            diagnosis,
            m.node.node_id,
            m.score,
            blast.risk,
            blast.downstream_count,
            action,
        )
    } else {
        format!(
            "{}: no topology match, action={}, blast={}",
            diagnosis, action, blast.risk,
        )
    }
}

fn build_evidence(
    error_text: &str,
    frames: &[TraceFrame],
    matched: Option<&NodeMatch>,
    diagnosis: &str,
) -> Vec<HealingEvidence> {
    let mut evidence = Vec::new();
    evidence.push(HealingEvidence {
        evidence_type: "error".into(),
        key: "error_text".into(),
        value: error_text.chars().take(200).collect(),
    });
    evidence.push(HealingEvidence {
        evidence_type: "trace".into(),
        key: "frame_count".into(),
        value: frames.len().to_string(),
    });
    if let Some(m) = matched {
        evidence.push(HealingEvidence {
            evidence_type: "topology".into(),
            key: "matched_node".into(),
            value: format!("{} (score={})", m.node.node_id, m.score),
        });
    }
    evidence.push(HealingEvidence {
        evidence_type: "diagnosis".into(),
        key: "classification".into(),
        value: diagnosis.into(),
    });
    evidence
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
                demand: vec![],
                answer: vec![],
            }),
        }
    }

    #[test]
    fn test_extract_trace_frames_rust() {
        let error = "panicked at src/main.rs:42:10";
        let frames = extract_trace_frames(error, Path::new("/project"));
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].source_path.as_deref(), Some("src/main.rs"));
        assert_eq!(frames[0].line, Some(42));
        assert_eq!(frames[0].column, Some(10));
    }

    #[test]
    fn test_extract_trace_frames_typescript() {
        let error = "at UserService.createUser (src/services.ts:100:5)";
        let frames = extract_trace_frames(error, Path::new("/project"));
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].function_name.as_deref(), Some("UserService.createUser"));
        assert_eq!(frames[0].source_path.as_deref(), Some("src/services.ts"));
    }

    #[test]
    fn test_score_node_match() {
        let frame = TraceFrame {
            source_path: Some("src/main.rs".into()),
            function_name: Some("execute".into()),
            line: None,
            column: None,
            raw: String::new(),
        };
        let node = make_node("Service.execute", "src/main.rs");
        let score = score_node_match(&frame, &node);
        assert!(score >= 18); // 10 (path) + 8 (function name in node_id)
    }

    #[test]
    fn test_classify_diagnosis() {
        assert_eq!(classify_diagnosis("cannot find module 'foo'"), "missing_dependency");
        assert_eq!(classify_diagnosis("type mismatch: expected String"), "type_mismatch");
        assert_eq!(classify_diagnosis("contract violation in trait Foo"), "contract_violation");
        assert_eq!(classify_diagnosis("request timed out after 30s"), "timeout");
        assert_eq!(classify_diagnosis("panic: index out of bounds"), "panic");
        assert_eq!(classify_diagnosis("something weird happened"), "unknown");
    }

    #[test]
    fn test_healing_diagnosis_integration() {
        let nodes = vec![
            make_node("Service.handle", "src/service.rs"),
            make_node("Repo.find", "src/repo.rs"),
        ];
        let diagnosis = diagnose_runtime_failure(
            Path::new("/test"),
            "at Service.handle (src/service.rs:42:10)\nTypeError: cannot read property",
            0,
            &nodes,
        );
        assert_eq!(diagnosis.matched_node_id.as_deref(), Some("Service.handle"));
        assert_eq!(diagnosis.trace_frames.len(), 1);
        assert!(!diagnosis.summary.is_empty());
    }

    #[test]
    fn test_build_healing_prompt() {
        let nodes = vec![make_node("Svc.run", "src/svc.rs")];
        let diag = diagnose_runtime_failure(
            Path::new("/test"),
            "panic: boom",
            0,
            &nodes,
        );
        let prompt = build_healing_prompt(Path::new("/test"), "panic: boom", &diag, &nodes);
        assert!(prompt.contains("panic: boom"));
        assert!(prompt.contains("Self-Healing"));
        assert!(prompt.contains("UpgradeProtocol"));
    }
}
