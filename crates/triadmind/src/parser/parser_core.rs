//! # Parser Core — Scan orchestration and node extraction
//!
//! Coordinates source file discovery, language-specific parsing,
//! and capability aggregation. Produces `TriadNodeDefinition` arrays
//! suitable for serialization into triad-map.json.
//!
//! @LeftBranch: scan_project, scan_file
//! @RightBranch: ParserOptions, ScanMode, LeafNode, CapabilityNode, ParseResult

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::TriadScanMode;
use crate::protocol::TriadNodeDefinition;

// ── Parser Configuration ────────────────────────────────────────────

/// Options controlling the topology scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParserOptions {
    /// Scan granularity: leaf, capability, module, or domain.
    #[serde(default = "default_scan_mode")]
    pub scan_mode: TriadScanMode,

    /// Minimum number of leaf nodes required to form a capability.
    #[serde(default = "default_capability_threshold")]
    pub capability_threshold: usize,

    /// Exclude test files from scan.
    #[serde(default = "default_true")]
    pub exclude_test_files: bool,

    /// Exclude magic methods (__init__, __repr__, etc.).
    #[serde(default = "default_true")]
    pub exclude_magic_methods: bool,

    /// Exclude private methods (prefixed with _).
    #[serde(default = "default_false")]
    pub exclude_private_methods: bool,

    /// How to handle helper verbs (build_, parse_, validate_, etc.).
    #[serde(default)]
    pub helper_verb_policy: HelperVerbPolicy,

    /// Fold helper methods into their owning capability.
    #[serde(default = "default_true")]
    pub fold_helpers_into_owner: bool,

    /// Method names that indicate an entry point / capability candidate.
    #[serde(default = "default_entry_methods")]
    pub entry_method_names: Vec<String>,

    /// Patterns for node names to exclude.
    #[serde(default)]
    pub exclude_node_name_patterns: Vec<String>,

    /// Ignore edges that involve only generic contracts (str, int, etc.).
    #[serde(default = "default_true")]
    pub ignore_generic_contracts: bool,

    /// Contract type names to treat as generic / low-value.
    #[serde(default = "default_generic_ignore_list")]
    pub generic_contract_ignore_list: Vec<String>,
}

fn default_scan_mode() -> TriadScanMode {
    TriadScanMode::Capability
}
fn default_capability_threshold() -> usize {
    4
}
fn default_true() -> bool {
    true
}
fn default_false() -> bool {
    false
}
fn default_entry_methods() -> Vec<String> {
    vec![
        "execute".into(),
        "run".into(),
        "handle".into(),
        "process".into(),
        "dispatch".into(),
        "apply".into(),
        "invoke".into(),
        "plan".into(),
    ]
}
fn default_generic_ignore_list() -> Vec<String> {
    vec![
        "str".into(),
        "string".into(),
        "int".into(),
        "number".into(),
        "bool".into(),
        "boolean".into(),
        "float".into(),
        "dict".into(),
        "object".into(),
        "list".into(),
        "array".into(),
        "Any".into(),
        "Option".into(),
        "Result".into(),
        "Vec".into(),
        "HashMap".into(),
        "JSON".into(),
        "Request".into(),
        "Response".into(),
        "void".into(),
        "()".into(),
    ]
}

impl Default for ParserOptions {
    fn default() -> Self {
        Self {
            scan_mode: default_scan_mode(),
            capability_threshold: default_capability_threshold(),
            exclude_test_files: default_true(),
            exclude_magic_methods: default_true(),
            exclude_private_methods: default_false(),
            helper_verb_policy: HelperVerbPolicy::default(),
            fold_helpers_into_owner: default_true(),
            entry_method_names: default_entry_methods(),
            exclude_node_name_patterns: Vec::new(),
            ignore_generic_contracts: default_true(),
            generic_contract_ignore_list: default_generic_ignore_list(),
        }
    }
}

// ── Helper Verb Policy ──────────────────────────────────────────────

/// Policy for handling helper-style function names
/// (build_*, parse_*, validate_*, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HelperVerbPolicy {
    /// Suppress helpers from appearing in the main graph.
    Suppress,
    /// Allow helpers to appear as independent nodes.
    Allow,
}

impl Default for HelperVerbPolicy {
    fn default() -> Self {
        Self::Suppress
    }
}

// ── Leaf Node (before capability aggregation) ──────────────────────

/// A raw leaf node extracted from a source file, before capability aggregation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeafNode {
    /// Fully qualified node id, e.g. "ClassName.methodName".
    pub node_id: String,
    /// Relative source file path.
    pub source_path: String,
    /// The detected problem / responsibility statement.
    pub problem: String,
    /// Parameter types or dependency names.
    pub demand: Vec<String>,
    /// Return type or output names.
    pub answer: Vec<String>,
    /// Whether this is a public method/function.
    pub is_public: bool,
    /// Whether this is a test function.
    pub is_test: bool,
    /// Whether this is a private/helper method.
    pub is_helper: bool,
    /// Line number in the source file.
    pub line: usize,
}

// ── Capability Node (after aggregation) ─────────────────────────────

/// A capability node aggregated from one or more leaf nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityNode {
    /// Capability node id.
    pub node_id: String,
    /// Category inferred from path/context.
    pub category: String,
    /// Leaf node ids that contribute to this capability.
    pub leaf_ids: Vec<String>,
    /// Aggregated problem statement.
    pub problem: String,
    /// Aggregated demands (deduplicated).
    pub demand: Vec<String>,
    /// Aggregated answers (deduplicated).
    pub answer: Vec<String>,
    /// Whether this node represents an external entry point.
    pub is_entry_point: bool,
}

// ── Parse Result ────────────────────────────────────────────────────

/// Result of a full project scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseResult {
    /// Project root directory.
    pub project_root: String,
    /// Scan mode used.
    pub scan_mode: TriadScanMode,
    /// Total source files scanned.
    pub files_scanned: usize,
    /// Raw leaf nodes extracted.
    pub leaf_nodes: Vec<LeafNode>,
    /// Capability nodes (when scan_mode is capability/module/domain).
    pub capability_nodes: Vec<CapabilityNode>,
    /// Leaf nodes suppressed by filtering rules.
    pub suppressed_leaves: Vec<LeafNode>,
    /// Final triad node definitions for serialization.
    pub triad_nodes: Vec<TriadNodeDefinition>,
}

// ── Scan Entry Point (stub) ─────────────────────────────────────────

/// Scan a project directory and produce a `ParseResult`.
///
/// Walks the project directory for source files, parses each with tree-sitter,
/// and collects leaf nodes. Capability aggregation is deferred to a later phase.
pub fn scan_project(project_root: &Path, options: &ParserOptions) -> Result<ParseResult, anyhow::Error> {
    let mut leaf_nodes = Vec::new();
    let mut suppressed_leaves = Vec::new();
    let mut files_scanned = 0usize;

    let walker = walkdir::WalkDir::new(project_root)
        .max_depth(12)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            // Skip hidden dirs and common excludes
            if e.file_type().is_dir() {
                if name.starts_with('.') && name != "." {
                    return false;
                }
                if matches!(
                    name.as_ref(),
                    "node_modules"
                        | "target"
                        | "dist"
                        | "build"
                        | ".git"
                        | "__pycache__"
                        | "venv"
                        | ".venv"
                        | ".next"
                        | "vendor"
                ) {
                    return false;
                }
            }
            true
        });

    for entry in walker.flatten() {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();

        // Check if it's a source file we can parse
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let is_source = matches!(
            ext.as_str(),
            "rs" | "ts" | "tsx" | "mts" | "cts"
        );

        if !is_source {
            continue;
        }

        // Skip test files if configured
        if options.exclude_test_files {
            let file_name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if file_name.contains("test") || file_name.ends_with("_test") {
                continue;
            }
            let path_str = path.to_string_lossy();
            if path_str.contains("\\tests\\") || path_str.contains("/tests/")
                || path_str.contains("\\test\\") || path_str.contains("/test/")
            {
                continue;
            }
        }

        files_scanned += 1;

        match super::tree_sitter_engine::parse_file(path, options) {
            Ok(nodes) => {
                for node in nodes {
                    if node.is_helper || node.is_test {
                        suppressed_leaves.push(node);
                    } else {
                        leaf_nodes.push(node);
                    }
                }
            }
            Err(_e) => {
                // Silently skip files that can't be parsed
            }
        }
    }

    // Sort leaf nodes by node_id for deterministic output
    leaf_nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    suppressed_leaves.sort_by(|a, b| a.node_id.cmp(&b.node_id));

    // Build triad node definitions (stub: 1:1 mapping from leaf nodes)
    let triad_nodes: Vec<_> = leaf_nodes
        .iter()
        .map(|leaf| crate::protocol::TriadNodeDefinition {
            node_id: leaf.node_id.clone(),
            category: None,
            source_path: Some(leaf.source_path.clone()),
            lifecycle: None,
            fission: Some(crate::protocol::TriadFission {
                problem: leaf.problem.clone(),
                demand: leaf.demand.clone(),
                answer: leaf.answer.clone(),
            }),
        })
        .collect();

    Ok(ParseResult {
        project_root: project_root.to_string_lossy().to_string(),
        scan_mode: options.scan_mode,
        files_scanned,
        leaf_nodes,
        capability_nodes: Vec::new(), // Deferred to capability aggregation phase
        suppressed_leaves,
        triad_nodes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_options() {
        let opts = ParserOptions::default();
        assert_eq!(opts.scan_mode, TriadScanMode::Capability);
        assert!(opts.exclude_test_files);
        assert!(opts.ignore_generic_contracts);
        assert!(!opts.entry_method_names.is_empty());
    }

    #[test]
    fn test_scan_project_handles_empty_dir() {
        let tmp = std::env::temp_dir().join("triadmind_test_empty");
        let _ = std::fs::create_dir_all(&tmp);
        let opts = ParserOptions::default();
        let result = scan_project(&tmp, &opts);
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(result.is_ok());
        let pr = result.unwrap();
        assert_eq!(pr.files_scanned, 0);
        assert!(pr.leaf_nodes.is_empty());
    }

    // ── Golden-file tests ──────────────────────────────────────────

    /// Helper: write files to a temp dir and scan.
    fn scan_temp_project(files: &[(&str, &str)], opts: &ParserOptions) -> ParseResult {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("triadmind_golden_{}_{}", std::process::id(), n));
        let _ = std::fs::create_dir_all(&tmp);
        for (name, content) in files {
            let path = tmp.join(name);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&path, content).unwrap();
        }
        let result = scan_project(&tmp, opts).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
        result
    }

    #[test]
    fn golden_rust_simple_project() {
        let files = [
            (
                "src/main.rs",
                r#"
/// Application entry point.
pub fn main() {
    println!("Hello");
}
"#,
            ),
            (
                "src/lib.rs",
                r#"
/// Adds two numbers.
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

/// Subtracts b from a.
pub fn subtract(a: i32, b: i32) -> i32 {
    a - b
}

fn internal_helper(x: i32) -> i32 {
    x * 2
}
"#,
            ),
            (
                "src/service.rs",
                r#"
pub struct UserService;

impl UserService {
    /// Creates a new user in the system.
    pub fn create_user(name: String, email: String) -> bool {
        true
    }

    /// Finds a user by their unique identifier.
    pub fn find_user(id: u64) -> Option<String> {
        None
    }
}
"#,
            ),
        ];

        let mut opts = ParserOptions::default();
        opts.exclude_private_methods = true;
        let result = scan_temp_project(&files, &opts);

        assert_eq!(result.files_scanned, 3);
        // main, add, subtract, UserService.create_user, UserService.find_user = 5 public nodes
        assert_eq!(result.leaf_nodes.len(), 5);

        let ids: Vec<&str> = result.leaf_nodes.iter().map(|n| n.node_id.as_str()).collect();
        assert!(ids.contains(&"add"));
        assert!(ids.contains(&"subtract"));
        assert!(ids.contains(&"UserService.create_user"));
        assert!(ids.contains(&"UserService.find_user"));
        // internal_helper should be excluded (private)
        assert!(!ids.contains(&"internal_helper"));

        // Verify add node details
        let add = result.leaf_nodes.iter().find(|n| n.node_id == "add").unwrap();
        assert_eq!(add.demand, vec!["i32", "i32"]);
        assert_eq!(add.answer, vec!["i32"]);
        assert!(add.problem.contains("Adds"));
    }

    #[test]
    fn golden_typescript_simple_project() {
        let files = [
            (
                "src/index.ts",
                r#"
/**
 * Application bootstrap.
 */
function bootstrap(): void {
    console.log('starting');
}
"#,
            ),
            (
                "src/handlers.ts",
                r#"
/**
 * Handles incoming HTTP request.
 */
export function handleRequest(req: Request): Response {
    return new Response();
}

/**
 * Validates request payload.
 */
function validatePayload(data: unknown): boolean {
    return true;
}
"#,
            ),
            (
                "src/services.ts",
                r#"
export class UserService {
    /** Creates a new user account. */
    createUser(name: string, email: string): User {
        return {} as User;
    }

    /** Deletes a user by ID. */
    deleteUser(id: string): boolean {
        return true;
    }

    private _hashPassword(pw: string): string {
        return pw;
    }
}
"#,
            ),
            (
                "src/api.ts",
                r#"
/** Fetches data from the remote API. */
export const fetchRemote = async (url: string): Promise<Data> => {
    return {} as Data;
};
"#,
            ),
        ];

        let opts = ParserOptions::default();
        let result = scan_temp_project(&files, &opts);

        assert_eq!(result.files_scanned, 4);

        let ids: Vec<&str> = result.leaf_nodes.iter().map(|n| n.node_id.as_str()).collect();
        // Should include: handleRequest, UserService.createUser, UserService.deleteUser, fetchRemote
        // bootstrap is non-exported; validatePayload is helper; _hashPassword is private
        assert!(ids.contains(&"handleRequest"));
        assert!(ids.contains(&"UserService.createUser"));
        assert!(ids.contains(&"UserService.deleteUser"));
        assert!(ids.contains(&"fetchRemote"));

        let create = result
            .leaf_nodes
            .iter()
            .find(|n| n.node_id == "UserService.createUser")
            .unwrap();
        assert_eq!(create.demand, vec!["string", "string"]);
        assert_eq!(create.answer, vec!["User"]);
        assert!(create.problem.contains("Creates"));

        let fetch = result
            .leaf_nodes
            .iter()
            .find(|n| n.node_id == "fetchRemote")
            .unwrap();
        assert_eq!(fetch.demand, vec!["string"]);
        assert!(fetch.problem.contains("Fetches"));
    }

    #[test]
    fn golden_exclude_tests() {
        let files = [
            (
                "src/lib.rs",
                r#"
pub fn real_logic(data: String) -> bool {
    true
}
"#,
            ),
            (
                "tests/test_lib.rs",
                r#"
fn test_real_logic() {
    assert!(real_logic("hello"));
}
"#,
            ),
        ];

        let mut opts = ParserOptions::default();
        opts.exclude_test_files = true;
        let result = scan_temp_project(&files, &opts);

        // tests/ directory should be skipped
        assert_eq!(result.files_scanned, 1);
        assert_eq!(result.leaf_nodes.len(), 1);
        assert_eq!(result.leaf_nodes[0].node_id, "real_logic");
    }

    #[test]
    fn golden_triad_nodes_output() {
        let files = [(
            "src/lib.rs",
            r#"
/// Core processing function.
pub fn process(input: String) -> String {
    input
}
"#,
        )];

        let opts = ParserOptions::default();
        let result = scan_temp_project(&files, &opts);

        assert_eq!(result.triad_nodes.len(), 1);
        let node = &result.triad_nodes[0];
        assert_eq!(node.node_id, "process");
        let fission = node.fission.as_ref().unwrap();
        assert_eq!(fission.problem, "Core processing function.");
        assert_eq!(fission.demand, vec!["String"]);
        assert_eq!(fission.answer, vec!["String"]);
    }
}
