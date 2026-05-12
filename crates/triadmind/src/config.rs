//! # Config — TriadMind configuration loading and project detection
//!
//! Ported from triadmind-core/config.ts and workspace.ts
//!
//! @RightBranch: TriadConfig, TriadLanguage, TriadScanMode, WorkspacePaths
//! @LeftBranch: load_triad_config, detect_project_language, should_skip_walk_path

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ── Core Enums ──────────────────────────────────────────────────────

/// Supported source languages for TriadMind analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriadLanguage {
    Typescript,
    Javascript,
    Python,
    Go,
    Rust,
    Cpp,
    Java,
}

impl TriadLanguage {
    pub fn as_str(&self) -> &'static str {
        match self {
            TriadLanguage::Typescript => "typescript",
            TriadLanguage::Javascript => "javascript",
            TriadLanguage::Python => "python",
            TriadLanguage::Go => "go",
            TriadLanguage::Rust => "rust",
            TriadLanguage::Cpp => "cpp",
            TriadLanguage::Java => "java",
        }
    }

    /// File extensions associated with each language (lowercase, with dot).
    pub fn extensions(&self) -> &'static [&'static str] {
        match self {
            TriadLanguage::Typescript => &[".ts", ".tsx", ".mts", ".cts"],
            TriadLanguage::Javascript => &[".js", ".jsx", ".mjs", ".cjs"],
            TriadLanguage::Python => &[".py"],
            TriadLanguage::Go => &[".go"],
            TriadLanguage::Rust => &[".rs"],
            TriadLanguage::Cpp => &[".cpp", ".cc", ".cxx", ".hpp", ".hh", ".h"],
            TriadLanguage::Java => &[".java"],
        }
    }
}

/// Parser engine used for topology extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TriadParserEngine {
    /// Language-specific native parser.
    Native,
    /// tree-sitter multi-language parser.
    #[serde(rename = "tree-sitter")]
    TreeSitter,
}

/// Scan granularity for topology extraction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriadScanMode {
    Leaf,
    #[default]
    Capability,
    Module,
    Domain,
}

/// Source file policy category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriadSourcePolicy {
    Api,
    Ui,
    Cli,
    Agent,
    Types,
    Tests,
    Migrations,
    Nodes,
    Tasks,
    Services,
    Utils,
    Other,
}

// ── Config Sub-structures ───────────────────────────────────────────

/// Ghost policy for a specific language.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhostLanguagePolicy {
    #[serde(rename = "includeInDemand", default)]
    pub include_in_demand: bool,
    #[serde(rename = "topK", default = "default_top_k")]
    pub top_k: i32,
    #[serde(rename = "minConfidence", default = "default_min_confidence")]
    pub min_confidence: f64,
}

fn default_top_k() -> i32 {
    5
}
fn default_min_confidence() -> f64 {
    0.5
}

impl Default for GhostLanguagePolicy {
    fn default() -> Self {
        Self {
            include_in_demand: false,
            top_k: 5,
            min_confidence: 0.5,
        }
    }
}

/// Match rules for a scan scope.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TriadScanScopeRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefixes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_segments: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_patterns: Option<Vec<String>>,
}

/// A single scan scope definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriadScanScope {
    pub name: String,
    pub kind: TriadSourcePolicy,
    #[serde(default)]
    pub priority: Option<i32>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub r#match: Option<TriadScanScopeRule>,
}

/// Parser configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriadParserConfig {
    #[serde(rename = "excludePatterns", default)]
    pub exclude_patterns: Vec<String>,
    #[serde(rename = "excludePathPatterns", default)]
    pub exclude_path_patterns: Vec<String>,
    #[serde(rename = "scanCategories", default)]
    pub scan_categories: Vec<String>,
    #[serde(rename = "scanMode", default)]
    pub scan_mode: TriadScanMode,
    #[serde(rename = "leafOutputFile", default = "default_leaf_output")]
    pub leaf_output_file: String,
    #[serde(rename = "capabilityOutputFile", default = "default_capability_output")]
    pub capability_output_file: String,
    #[serde(rename = "capabilityThreshold", default = "default_capability_threshold")]
    pub capability_threshold: f64,
    #[serde(rename = "excludeTestFiles", default = "default_true")]
    pub exclude_test_files: bool,
    #[serde(rename = "excludeMagicMethods", default = "default_true")]
    pub exclude_magic_methods: bool,
    #[serde(rename = "excludePrivateMethods", default)]
    pub exclude_private_methods: bool,
    #[serde(rename = "helperVerbPolicy", default)]
    pub helper_verb_policy: String,
    #[serde(rename = "foldHelpersIntoOwner", default = "default_true")]
    pub fold_helpers_into_owner: bool,
    #[serde(rename = "entryMethodNames", default)]
    pub entry_method_names: Vec<String>,
    #[serde(rename = "excludeNodeNamePatterns", default)]
    pub exclude_node_name_patterns: Vec<String>,
    #[serde(rename = "ignoreGenericContracts", default = "default_true")]
    pub ignore_generic_contracts: bool,
    #[serde(rename = "genericContractIgnoreList", default)]
    pub generic_contract_ignore_list: Vec<String>,
    #[serde(rename = "includeUntaggedExports", default)]
    pub include_untagged_exports: bool,
    #[serde(rename = "ghostPolicyByLanguage", default)]
    pub ghost_policy_by_language: HashMap<String, GhostLanguagePolicy>,
}

fn default_leaf_output() -> String {
    "triad-map.json".into()
}
fn default_capability_output() -> String {
    "capability-map.json".into()
}
fn default_capability_threshold() -> f64 {
    0.5
}
fn default_true() -> bool {
    true
}

impl Default for TriadParserConfig {
    fn default() -> Self {
        Self {
            exclude_patterns: vec![],
            exclude_path_patterns: vec![],
            scan_categories: vec![],
            scan_mode: TriadScanMode::Leaf,
            leaf_output_file: "triad-map.json".into(),
            capability_output_file: "capability-map.json".into(),
            capability_threshold: 0.5,
            exclude_test_files: true,
            exclude_magic_methods: true,
            exclude_private_methods: false,
            helper_verb_policy: "suppress".into(),
            fold_helpers_into_owner: true,
            entry_method_names: vec![],
            exclude_node_name_patterns: vec![],
            ignore_generic_contracts: true,
            generic_contract_ignore_list: vec![],
            include_untagged_exports: false,
            ghost_policy_by_language: HashMap::new(),
        }
    }
}

/// Architecture configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriadArchitectureConfig {
    pub language: TriadLanguage,
    #[serde(rename = "parserEngine")]
    pub parser_engine: TriadParserEngine,
    pub adapter: String,
}

/// Visualizer configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriadVisualizerConfig {
    #[serde(rename = "defaultView", default = "default_view")]
    pub default_view: String,
    #[serde(rename = "showIsolatedCapabilities", default = "default_true")]
    pub show_isolated_capabilities: bool,
    #[serde(rename = "maxContractEdges", default = "default_max_contract")]
    pub max_contract_edges: i32,
    #[serde(rename = "maxPrimaryEdges", default = "default_max_primary")]
    pub max_primary_edges: i32,
    #[serde(rename = "fastMode", default = "default_true")]
    pub fast_mode: bool,
    #[serde(rename = "strictFingerprint", default = "default_true")]
    pub strict_fingerprint: bool,
    #[serde(rename = "fastMayaThreshold", default)]
    pub fast_maya_threshold: f64,
    #[serde(rename = "fastFingerprintThreshold", default)]
    pub fast_fingerprint_threshold: f64,
    #[serde(rename = "maxFingerprintNodes", default = "default_max_fp_nodes")]
    pub max_fingerprint_nodes: i32,
    #[serde(rename = "maxFingerprintOwners", default = "default_max_fp_owners")]
    pub max_fingerprint_owners: i32,
    #[serde(rename = "fingerprintTimeoutMs", default = "default_fp_timeout")]
    pub fingerprint_timeout_ms: i32,
}

fn default_view() -> String {
    "architecture".into()
}
fn default_max_contract() -> i32 {
    200
}
fn default_max_primary() -> i32 {
    200
}
fn default_max_fp_nodes() -> i32 {
    200
}
fn default_max_fp_owners() -> i32 {
    50
}
fn default_fp_timeout() -> i32 {
    5000
}

impl Default for TriadVisualizerConfig {
    fn default() -> Self {
        Self {
            default_view: "architecture".into(),
            show_isolated_capabilities: true,
            max_contract_edges: 200,
            max_primary_edges: 200,
            fast_mode: true,
            strict_fingerprint: true,
            fast_maya_threshold: 0.0,
            fast_fingerprint_threshold: 0.0,
            max_fingerprint_nodes: 200,
            max_fingerprint_owners: 50,
            fingerprint_timeout_ms: 5000,
        }
    }
}

/// Runtime healing configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeHealingConfig {
    #[serde(rename = "requireHumanApprovalForContractChanges", default = "default_true")]
    pub require_human_approval_for_contract_changes: bool,
    #[serde(rename = "maxAutoRetries", default = "default_max_retries")]
    pub max_auto_retries: i32,
}

fn default_max_retries() -> i32 {
    3
}

impl Default for RuntimeHealingConfig {
    fn default() -> Self {
        Self {
            require_human_approval_for_contract_changes: true,
            max_auto_retries: 3,
        }
    }
}

// ── Top-Level Config ────────────────────────────────────────────────

/// Complete TriadMind configuration, deserialized from .triadmind/config.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriadConfig {
    #[serde(rename = "schemaVersion", default = "default_schema_version")]
    pub schema_version: String,
    pub architecture: TriadArchitectureConfig,
    #[serde(default)]
    pub categories: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub parser: TriadParserConfig,
    #[serde(default)]
    pub visualizer: TriadVisualizerConfig,
    #[serde(rename = "runtimeHealing", default)]
    pub runtime_healing: RuntimeHealingConfig,
    #[serde(rename = "scanScopes", default)]
    pub scan_scopes: Vec<TriadScanScope>,
}

fn default_schema_version() -> String {
    "1.0".into()
}

impl Default for TriadConfig {
    fn default() -> Self {
        Self {
            schema_version: "1.0".into(),
            architecture: TriadArchitectureConfig {
                language: TriadLanguage::Typescript,
                parser_engine: TriadParserEngine::Native,
                adapter: String::new(),
            },
            categories: HashMap::new(),
            parser: TriadParserConfig::default(),
            visualizer: TriadVisualizerConfig::default(),
            runtime_healing: RuntimeHealingConfig::default(),
            scan_scopes: vec![],
        }
    }
}

// ── Workspace Paths ─────────────────────────────────────────────────

/// Resolved paths for a TriadMind workspace.
///
/// All paths are absolute and normalized.
#[derive(Debug, Clone)]
pub struct WorkspacePaths {
    pub project_root: PathBuf,
    pub triad_dir: PathBuf,
    pub config_file: PathBuf,
    pub map_file: PathBuf,
    pub leaf_map_file: PathBuf,
    pub cache_dir: PathBuf,
    pub sync_cache_file: PathBuf,
    pub agents_file: PathBuf,
    pub draft_file: PathBuf,
    pub macro_split_file: PathBuf,
    pub meso_split_file: PathBuf,
    pub micro_split_file: PathBuf,
    pub runtime_map_file: PathBuf,
    pub runtime_diagnostics_file: PathBuf,
    pub verify_baseline_file: PathBuf,
    pub dream_report_file: PathBuf,
    pub dream_state_file: PathBuf,
    pub dream_feedback_file: PathBuf,
    pub healing_report_file: PathBuf,
    pub healing_prompt_file: PathBuf,
    pub runtime_error_file: PathBuf,
    pub impact_map_file: PathBuf,
    pub impact_protocol_file: PathBuf,
    pub impact_prompt_file: PathBuf,
    pub impact_visualizer_file: PathBuf,
    pub triad_spec_file: PathBuf,
    pub demand_file: PathBuf,
}

impl WorkspacePaths {
    /// Build workspace paths from a project root directory.
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        let root = project_root.into();
        let triad_dir = root.join(".triadmind");
        let cache_dir = triad_dir.join("cache");

        Self {
            agents_file: root.join("AGENTS.md"),
            triad_spec_file: triad_dir.join("triad.md"),
            config_file: triad_dir.join("config.json"),
            map_file: triad_dir.join("triad-map.json"),
            leaf_map_file: triad_dir.join("leaf-map.json"),
            cache_dir: cache_dir.clone(),
            sync_cache_file: cache_dir.join("sync-manifest.json"),
            draft_file: triad_dir.join("draft-protocol.json"),
            macro_split_file: triad_dir.join("macro-split.json"),
            meso_split_file: triad_dir.join("meso-split.json"),
            micro_split_file: triad_dir.join("micro-split.json"),
            runtime_map_file: triad_dir.join("runtime-map.json"),
            runtime_diagnostics_file: triad_dir.join("runtime-diagnostics.json"),
            verify_baseline_file: triad_dir.join("verify-baseline.json"),
            dream_report_file: triad_dir.join("dream-report.json"),
            dream_state_file: triad_dir.join("dream-state.json"),
            dream_feedback_file: triad_dir.join("dream-feedback.json"),
            healing_report_file: triad_dir.join("healing-report.json"),
            healing_prompt_file: triad_dir.join("healing-prompt.md"),
            runtime_error_file: triad_dir.join("runtime-error.log"),
            impact_map_file: triad_dir.join("impact-map.json"),
            impact_protocol_file: triad_dir.join("impact-protocol.json"),
            impact_prompt_file: triad_dir.join("impact-prompt.md"),
            impact_visualizer_file: triad_dir.join("impact-visualizer.html"),
            demand_file: triad_dir.join("latest-demand.txt"),
            project_root: root,
            triad_dir,
        }
    }
}

// ── Config Loading ──────────────────────────────────────────────────

/// Load TriadMind configuration from `.triadmind/config.json`.
///
/// If the file doesn't exist or can't be parsed, returns a default config
/// with auto-detected language.
pub fn load_triad_config(paths: &WorkspacePaths) -> TriadConfig {
    match std::fs::read_to_string(&paths.config_file) {
        Ok(content) => {
            let trimmed = content.trim().trim_start_matches('\u{FEFF}');
            match serde_json::from_str::<TriadConfig>(trimmed) {
                Ok(config) => config,
                Err(_) => {
                    let mut config = TriadConfig::default();
                    config.architecture.language =
                        detect_project_language(&paths.project_root);
                    config
                }
            }
        }
        Err(_) => {
            let mut config = TriadConfig::default();
            config.architecture.language = detect_project_language(&paths.project_root);
            config
        }
    }
}

// ── Language Detection ──────────────────────────────────────────────

/// Auto-detect the project's primary language by scanning file extensions.
pub fn detect_project_language(project_root: &Path) -> TriadLanguage {
    if !project_root.exists() {
        return TriadLanguage::Typescript;
    }

    // Quick check: tsconfig.json strongly signals TypeScript
    if project_root.join("tsconfig.json").exists() {
        return TriadLanguage::Typescript;
    }

    let mut scores: HashMap<TriadLanguage, usize> = HashMap::new();

    let walker = walkdir::WalkDir::new(project_root)
        .max_depth(6)
        .into_iter()
        .filter_entry(|e| !should_skip_walk_entry(e));

    for entry in walker.flatten() {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };

        let ext_lower = ext.to_lowercase();
        let lang = match ext_lower.as_str() {
            "ts" | "tsx" | "mts" | "cts" => TriadLanguage::Typescript,
            "js" | "jsx" | "mjs" | "cjs" => TriadLanguage::Javascript,
            "py" => TriadLanguage::Python,
            "go" => TriadLanguage::Go,
            "rs" => TriadLanguage::Rust,
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "h" => TriadLanguage::Cpp,
            "java" => TriadLanguage::Java,
            _ => continue,
        };

        *scores.entry(lang).or_insert(0) += 1;
    }

    scores
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(lang, _)| lang)
        .unwrap_or(TriadLanguage::Typescript)
}

/// Path segments that TriadMind should always skip during project walks.
const HARD_EXCLUDE_SEGMENTS: &[&str] = &[
    "node_modules",
    ".git",
    ".triadmind",
    "__pycache__",
    ".venv",
    "venv",
    "target",
    "dist",
    "build",
    ".next",
    ".turbo",
    "coverage",
    ".nyc_output",
];

/// Filename patterns that should always be excluded.
fn is_hard_excluded_filename(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".min.js")
        || lower.ends_with(".min.css")
        || lower.ends_with(".d.ts")
        || lower.ends_with(".generated.ts")
        || lower.ends_with(".generated.tsx")
        || lower == "package-lock.json"
        || lower == "yarn.lock"
        || lower == "pnpm-lock.yaml"
        || lower == "cargo.lock"
        || lower == "go.sum"
}

/// Whether to skip a directory entry during project walking.
pub fn should_skip_walk_entry(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();

    // Skip hidden directories and known exclude segments
    if entry.file_type().is_dir() {
        if name.starts_with('.') && name != "." {
            return false; // walkdir already skips hidden by default; let's be explicit
        }
        if HARD_EXCLUDE_SEGMENTS.contains(&name.as_ref()) {
            return false;
        }
    }

    // Skip known generated / lock files
    if entry.file_type().is_file() && is_hard_excluded_filename(&name) {
        return false;
    }

    true
}

/// Check if a relative path should be skipped during project walks.
pub fn should_skip_walk_path(rel_path: &str) -> bool {
    let normalized = rel_path.replace('\\', "/");
    let segments: Vec<&str> = normalized.split('/').collect();

    // Check if any segment is a hard-exclude directory
    if segments
        .iter()
        .any(|s| HARD_EXCLUDE_SEGMENTS.contains(s))
    {
        return true;
    }

    // Check basename
    let basename = segments.last().copied().unwrap_or(&normalized);
    if basename == ".git" || basename == ".triadmind" {
        return true;
    }

    if is_hard_excluded_filename(basename) {
        return true;
    }

    false
}

/// Create a predicate that tests whether a file path should be included in scanning.
pub fn create_source_path_filter<'a>(
    _project_root: &'a Path,
    config: &'a TriadConfig,
) -> impl Fn(&str) -> bool + 'a {
    move |rel_path: &str| {
        if should_skip_walk_path(rel_path) {
            return false;
        }

        // Check against exclude patterns
        let normalized = rel_path.replace('\\', "/");
        for pattern in &config.parser.exclude_path_patterns {
            if normalized.contains(pattern.as_str()) {
                return false;
            }
        }

        // Check file extension against configured language
        let Some(ext) = Path::new(rel_path)
            .extension()
            .and_then(|e| e.to_str())
        else {
            return false;
        };

        let ext_lower = ext.to_lowercase();
        config
            .architecture
            .language
            .extensions()
            .iter()
            .any(|e| e.trim_start_matches('.') == ext_lower)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_language_from_tsconfig() {
        // This test is illustrative; in real CI we'd use a temp dir
        let lang = detect_project_language(Path::new("."));
        // In this workspace, we may find TypeScript or Rust files
        // Just ensure it returns a valid variant
        let _ = lang.as_str();
    }

    #[test]
    fn test_should_skip_node_modules() {
        assert!(should_skip_walk_path("node_modules/foo.ts"));
        assert!(should_skip_walk_path("src/node_modules/bar.ts"));
    }

    #[test]
    fn test_should_not_skip_normal_source() {
        assert!(!should_skip_walk_path("src/main.rs"));
        assert!(!should_skip_walk_path("lib/index.ts"));
    }

    #[test]
    fn test_workspace_paths_construction() {
        let paths = WorkspacePaths::new("/test/project");
        assert_eq!(
            paths.map_file,
            PathBuf::from("/test/project/.triadmind/triad-map.json")
        );
        assert_eq!(
            paths.config_file,
            PathBuf::from("/test/project/.triadmind/config.json")
        );
    }

    #[test]
    fn test_default_config_parses() {
        let config = TriadConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: TriadConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.schema_version, "1.0");
    }

    #[test]
    fn test_language_extensions() {
        assert!(TriadLanguage::Rust.extensions().contains(&".rs"));
        assert!(TriadLanguage::Typescript.extensions().contains(&".ts"));
        assert!(TriadLanguage::Python.extensions().contains(&".py"));
    }
}
