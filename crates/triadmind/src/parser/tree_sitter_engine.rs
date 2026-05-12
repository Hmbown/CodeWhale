//! # Tree-Sitter Engine — Multi-language parsing backend
//!
//! Pattern-based source code parser that extracts triad leaf nodes.
//! Currently supports: Rust, TypeScript (via regex).
//!
//! When the `tree-sitter` feature is enabled, uses tree-sitter grammars
//! for precise AST-based extraction instead.
//!
//! @LeftBranch: parse_file, extract_nodes_for_language
//! @RightBranch: LanguageGrammar, TreeSitterConfig

use std::path::Path;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config::TriadLanguage;
use super::parser_core::{LeafNode, ParserOptions};

// ── Language Grammar Loading ────────────────────────────────────────

/// Represents a loaded language grammar (pattern-based or tree-sitter).
#[derive(Clone)]
pub struct LanguageGrammar {
    pub language: TriadLanguage,
    pub name: String,
}

impl LanguageGrammar {
    /// Check if a grammar is available for the given language.
    pub fn is_available(language: TriadLanguage) -> bool {
        matches!(language, TriadLanguage::Rust | TriadLanguage::Typescript)
    }

    /// Try to load a grammar for the given language.
    pub fn load(language: TriadLanguage) -> Option<Self> {
        if !Self::is_available(language) {
            return None;
        }
        Some(Self {
            language,
            name: language.as_str().to_string(),
        })
    }
}

// ── Tree-Sitter Configuration ───────────────────────────────────────

/// Configuration for parsing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeSitterConfig {
    /// Maximum file size in bytes to parse (skip larger files).
    #[serde(default = "default_max_file_size")]
    pub max_file_size_bytes: usize,
    /// Maximum parse time per file in milliseconds (not enforced).
    #[serde(default = "default_timeout_ms")]
    pub parse_timeout_ms: u64,
    /// Languages to enable (empty = all available).
    #[serde(default)]
    pub enabled_languages: Vec<TriadLanguage>,
}

fn default_max_file_size() -> usize {
    2 * 1024 * 1024 // 2 MB
}
fn default_timeout_ms() -> u64 {
    5000
}

impl Default for TreeSitterConfig {
    fn default() -> Self {
        Self {
            max_file_size_bytes: default_max_file_size(),
            parse_timeout_ms: default_timeout_ms(),
            enabled_languages: Vec::new(),
        }
    }
}

// ── Main Parse Entry Points ─────────────────────────────────────────

/// Parse a single source file into leaf nodes.
///
/// Detects the language from the file extension and delegates to
/// the appropriate language-specific parser.
pub fn parse_file(
    file_path: &Path,
    options: &ParserOptions,
) -> Result<Vec<LeafNode>, anyhow::Error> {
    let lang = detect_language(file_path)?;
    if !LanguageGrammar::is_available(lang) {
        return Ok(Vec::new());
    }
    let source = std::fs::read_to_string(file_path)?;

    if source.len() > TreeSitterConfig::default().max_file_size_bytes {
        return Ok(Vec::new());
    }

    let source_path = file_path.to_string_lossy().to_string();
    extract_nodes_for_language(&source, lang, &source_path, options)
}

/// Extract nodes for a specific language from source text.
pub fn extract_nodes_for_language(
    source: &str,
    language: TriadLanguage,
    source_path: &str,
    options: &ParserOptions,
) -> Result<Vec<LeafNode>, anyhow::Error> {
    match language {
        TriadLanguage::Rust => extract_rust_nodes(source, source_path, options),
        TriadLanguage::Typescript => extract_typescript_nodes(source, source_path, options),
        _ => Ok(Vec::new()),
    }
}

/// Detect language from file extension.
fn detect_language(file_path: &Path) -> Result<TriadLanguage, anyhow::Error> {
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "rs" => Ok(TriadLanguage::Rust),
        "ts" | "tsx" | "mts" | "cts" => Ok(TriadLanguage::Typescript),
        _ => Err(anyhow::anyhow!("Unsupported file extension: .{}", ext)),
    }
}

// ── Rust Parser (regex-based) ──────────────────────────────────────

/// Extract leaf nodes from Rust source code.
fn extract_rust_nodes(
    source: &str,
    source_path: &str,
    options: &ParserOptions,
) -> Result<Vec<LeafNode>, anyhow::Error> {
    let mut nodes = Vec::new();

    // Match Rust function declarations: `pub fn name(params) -> ReturnType`
    // Also handles `pub async fn`, `unsafe fn`, etc.
    let fn_re = Regex::new(
        r"(?m)^\s*(?:(?:pub(?:\s*\(\s*(?:crate|self|super)\s*\))?)\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+(\w+)\s*\(([^)]*)\)(?:\s*->\s*([^\n{]+?))?\s*\{"
    ).unwrap();

    let doc_re = Regex::new(r"(?m)^\s*///(.*)").unwrap();

    // Find current impl struct name
    let impl_re = Regex::new(r"(?m)^\s*impl\s+(?:(\w+)\s+for\s+)?(\w+)").unwrap();

    // Collect impl block ranges to associate methods with their struct
    let impl_ranges: Vec<(usize, String)> = impl_re
        .captures_iter(source)
        .map(|cap| {
            let pos = cap.get(0).unwrap().start();
            let type_name = cap.get(1).map_or_else(
                || cap[2].to_string(),
                |trait_name| format!("{} for {}", trait_name.as_str(), &cap[2]),
            );
            (pos, type_name)
        })
        .collect();

    // Collect doc comments with their line ranges
    let doc_lines: Vec<(usize, String)> = doc_re
        .captures_iter(source)
        .map(|cap| {
            let line_num = source[..cap.get(0).unwrap().start()]
                .lines()
                .count();
            (line_num, cap[1].trim().to_string())
        })
        .collect();

    for cap in fn_re.captures_iter(source) {
        let fn_name = cap[1].to_string();

        // Skip test functions
        if options.exclude_test_files
            && (fn_name.starts_with("test_") || fn_name.contains("_test"))
        {
            continue;
        }

        // Determine if this is a method (inside impl block)
        let fn_start = cap.get(0).unwrap().start();
        let fn_line = source[..fn_start].lines().count();
        let struct_name = impl_ranges
            .iter()
            .filter(|(start, _)| *start < fn_start)
            .last()
            .map(|(_, name)| name.as_str());

        // Check whether the function declaration itself contains "pub fn"
        let is_private = !cap.get(0).unwrap().as_str().contains("pub fn");

        if options.exclude_private_methods && is_private {
            continue;
        }

        // Extract doc comment preceding this function
        let problem = extract_preceding_doc(&doc_lines, fn_line);

        // Build node_id
        let node_id = if let Some(sname) = struct_name {
            format!("{}.{}", sname, fn_name)
        } else {
            fn_name.clone()
        };

        // Extract parameters (demand)
        let demand = extract_rust_param_types(&cap[2]);

        // Extract return type (answer)
        let answer = cap
            .get(3)
            .map(|m| vec![m.as_str().trim().to_string()])
            .unwrap_or_default();

        // Check if helper
        let is_helper = is_helper_function(&fn_name, options);

        nodes.push(LeafNode {
            node_id,
            source_path: source_path.to_string(),
            problem,
            demand,
            answer,
            is_public: !is_helper && !is_private,
            is_test: fn_name.contains("test"),
            is_helper,
            line: fn_line + 1,
        });
    }

    Ok(nodes)
}

/// Extract parameter type names from a Rust parameter string.
fn extract_rust_param_types(params: &str) -> Vec<String> {
    if params.trim().is_empty() {
        return Vec::new();
    }

    // Handle `&self` and `&mut self` by skipping
    let parts: Vec<&str> = params.split(',').collect();
    let mut types = Vec::new();

    for part in parts {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Skip `self`, `&self`, `&mut self`
        if trimmed == "self" || trimmed == "&self" || trimmed == "&mut self" {
            continue;
        }
        // Extract the type after the colon
        if let Some(colon_pos) = trimmed.rfind(':') {
            let type_part = trimmed[colon_pos + 1..].trim();
            // Remove reference, mut, and pattern wrappers
            let cleaned = type_part
                .trim_start_matches('&')
                .trim_start_matches("mut ")
                .trim();
            types.push(cleaned.to_string());
        } else {
            // Parameter without explicit type
            types.push("_".to_string());
        }
    }

    types
}

// ── TypeScript Parser (regex-based) ────────────────────────────────

/// Extract leaf nodes from TypeScript source code.
fn extract_typescript_nodes(
    source: &str,
    source_path: &str,
    options: &ParserOptions,
) -> Result<Vec<LeafNode>, anyhow::Error> {
    let mut nodes = Vec::new();

    // Match standalone function declarations
    // `function name(params): ReturnType` or `export function name(params): ReturnType`
    let fn_re = Regex::new(
        r"(?m)^\s*(?:export\s+(?:default\s+)?)?(?:async\s+)?function\s+(\w+)\s*\(([^)]*)\)(?:\s*:\s*([^\n{]+?))?\s*\{"
    ).unwrap();

    // Match class method declarations
    // `methodName(params): ReturnType {`
    let method_re = Regex::new(
        r"(?m)^\s*(?:public\s+|private\s+|protected\s+)?(?:static\s+)?(?:async\s+)?(\w+)\s*\(([^)]*)\)(?:\s*:\s*([^\n{]+?))?\s*\{"
    ).unwrap();

    // Match arrow functions assigned to const/let
    // `export const name = (params): ReturnType =>`
    let arrow_re = Regex::new(
        r"(?m)^\s*(?:export\s+)?(?:const|let|var)\s+(\w+)\s*=\s*(?:async\s+)?\(([^)]*)\)(?:\s*:\s*([^\n=]+?))?\s*=>"
    ).unwrap();

    // Match class declarations to associate methods
    let class_re = Regex::new(r"(?m)^\s*(?:export\s+(?:default\s+)?)?class\s+(\w+)").unwrap();

    // JSDoc comments
    let jsdoc_re = Regex::new(r"/\*\*\s*\n?((?:[^*]|\*[^/])*)\*/").unwrap();

    // Collect class ranges
    let class_ranges: Vec<(usize, usize, String)> = {
        let mut ranges = Vec::new();
        for cap in class_re.captures_iter(source) {
            let start = cap.get(0).unwrap().start();
            let class_name = cap[1].to_string();
            // Find approximate end of class (matching braces is too complex; use next class or EOF)
            let end = source.len();
            ranges.push((start, end, class_name));
        }
        // Sort by start position
        ranges.sort_by_key(|r| r.0);
        ranges
    };

    // Collect JSDoc comments with positions
    let jsdoc_comments: Vec<(usize, String)> = jsdoc_re
        .captures_iter(source)
        .map(|cap| {
            let pos = cap.get(0).unwrap().start();
            let body = cap[1].trim();
            let cleaned = body
                .lines()
                .map(|l| l.trim().trim_start_matches('*').trim())
                .filter(|l| !l.is_empty() && !l.starts_with('@'))
                .collect::<Vec<_>>()
                .join(" ");
            (pos, cleaned)
        })
        .collect();

    // Process standalone functions
    for cap in fn_re.captures_iter(source) {
        let fn_name = cap[1].to_string();

        if options.exclude_test_files
            && (fn_name.starts_with("test") || fn_name.ends_with("Test"))
        {
            continue;
        }

        let fn_start = cap.get(0).unwrap().start();
        let fn_line = source[..fn_start].lines().count();

        let problem = extract_preceding_jsdoc(&jsdoc_comments, fn_start);

        let demand = extract_ts_param_types(&cap[2]);
        let answer = cap
            .get(3)
            .map(|m| vec![m.as_str().trim().to_string()])
            .unwrap_or_default();

        let is_helper = is_helper_function(&fn_name, options);

        nodes.push(LeafNode {
            node_id: fn_name.clone(),
            source_path: source_path.to_string(),
            problem,
            demand,
            answer,
            is_public: !is_helper,
            is_test: fn_name.contains("test") || fn_name.contains("Test"),
            is_helper,
            line: fn_line + 1,
        });
    }

    // Process class methods
    for cap in method_re.captures_iter(source) {
        let method_name = cap[1].to_string();

        // Skip keywords that look like method names
        if matches!(
            method_name.as_str(),
            "if" | "for" | "while" | "switch" | "return" | "throw" | "new" | "typeof"
                | "import" | "export" | "class" | "interface" | "type" | "enum" | "constructor"
        ) {
            continue;
        }

        // Skip constructor
        if method_name == "constructor" {
            continue;
        }

        if options.exclude_test_files
            && (method_name.starts_with("test") || method_name.ends_with("Test"))
        {
            continue;
        }

        if options.exclude_private_methods
            && (method_name.starts_with('_') || method_name.starts_with('#'))
        {
            continue;
        }

        let method_start = cap.get(0).unwrap().start();

        // Find which class this method belongs to
        let class_name = class_ranges
            .iter()
            .filter(|(start, end, _)| *start < method_start && method_start < *end)
            .last()
            .map(|(_, _, name)| name.as_str());

        let node_id = if let Some(cn) = class_name {
            format!("{}.{}", cn, method_name)
        } else {
            method_name.clone()
        };

        let fn_line = source[..method_start].lines().count();
        let problem = extract_preceding_jsdoc(&jsdoc_comments, method_start);
        let demand = extract_ts_param_types(&cap[2]);
        let answer = cap
            .get(3)
            .map(|m| vec![m.as_str().trim().to_string()])
            .unwrap_or_default();

        let is_helper = is_helper_function(&method_name, options);

        nodes.push(LeafNode {
            node_id,
            source_path: source_path.to_string(),
            problem,
            demand,
            answer,
            is_public: !is_helper,
            is_test: method_name.contains("test") || method_name.contains("Test"),
            is_helper,
            line: fn_line + 1,
        });
    }

    // Process arrow function exports
    for cap in arrow_re.captures_iter(source) {
        let fn_name = cap[1].to_string();

        if options.exclude_test_files
            && (fn_name.starts_with("test") || fn_name.ends_with("Test"))
        {
            continue;
        }

        let fn_start = cap.get(0).unwrap().start();
        let fn_line = source[..fn_start].lines().count();
        let problem = extract_preceding_jsdoc(&jsdoc_comments, fn_start);
        let demand = extract_ts_param_types(&cap[2]);
        let answer = cap
            .get(3)
            .map(|m| vec![m.as_str().trim().to_string()])
            .unwrap_or_default();

        let is_helper = is_helper_function(&fn_name, options);

        nodes.push(LeafNode {
            node_id: fn_name.clone(),
            source_path: source_path.to_string(),
            problem,
            demand,
            answer,
            is_public: !is_helper,
            is_test: fn_name.contains("test") || fn_name.contains("Test"),
            is_helper,
            line: fn_line + 1,
        });
    }

    Ok(nodes)
}

/// Extract parameter type names from a TypeScript parameter string.
fn extract_ts_param_types(params: &str) -> Vec<String> {
    if params.trim().is_empty() {
        return Vec::new();
    }

    let parts: Vec<&str> = params.split(',').collect();
    let mut types = Vec::new();

    for part in parts {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Extract the type after the colon
        if let Some(colon_pos) = trimmed.rfind(':') {
            let type_part = trimmed[colon_pos + 1..].trim();
            // Handle `= defaultValue` after type
            let cleaned = if let Some(eq_pos) = type_part.find('=') {
                type_part[..eq_pos].trim().to_string()
            } else {
                type_part.to_string()
            };
            types.push(cleaned);
        } else {
            types.push("any".to_string());
        }
    }

    types
}

// ── Shared Helpers ──────────────────────────────────────────────────

/// Extract doc comment from lines preceding a function (Rust style).
fn extract_preceding_doc(doc_lines: &[(usize, String)], fn_line: usize) -> String {
    let mut comment_lines: Vec<&str> = Vec::new();
    for (line_num, text) in doc_lines.iter().rev() {
        if *line_num < fn_line && fn_line - line_num <= 20 {
            comment_lines.push(text.as_str());
        }
    }
    comment_lines.reverse();
    let result = comment_lines.join(" ");
    result
}

/// Extract JSDoc comment preceding a position.
fn extract_preceding_jsdoc(jsdocs: &[(usize, String)], pos: usize) -> String {
    // Find the closest JSDoc that ends just before this position
    let source_char_pos = pos;
    jsdocs
        .iter()
        .filter(|(doc_pos, _)| *doc_pos < source_char_pos)
        .last()
        .map(|(_, text)| text.clone())
        .unwrap_or_default()
}

/// Check if a function name indicates a helper/utility function.
fn is_helper_function(name: &str, options: &ParserOptions) -> bool {
    if options.exclude_private_methods && name.starts_with('_') {
        return true;
    }

    let helper_prefixes = [
        "build_",
        "parse_",
        "format_",
        "normalize_",
        "sanitize_",
        "validate_",
        "load_",
        "save_",
        "resolve_",
        "collect_",
        "compute_",
        "convert_",
    ];

    if helper_prefixes
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        return matches!(
            options.helper_verb_policy,
            super::parser_core::HelperVerbPolicy::Suppress
        );
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_language_rust() {
        assert_eq!(
            detect_language(Path::new("src/main.rs")).unwrap(),
            TriadLanguage::Rust
        );
    }

    #[test]
    fn test_detect_language_typescript() {
        assert_eq!(
            detect_language(Path::new("src/index.ts")).unwrap(),
            TriadLanguage::Typescript
        );
    }

    #[test]
    fn test_parse_rust_simple_function() {
        let source = r#"
/// Calculates the sum of two numbers.
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}
"#;
        let opts = ParserOptions::default();
        let nodes = extract_nodes_for_language(
            source,
            TriadLanguage::Rust,
            "test.rs",
            &opts,
        )
        .unwrap();

        assert_eq!(nodes.len(), 1);
        let node = &nodes[0];
        assert_eq!(node.node_id, "add");
        assert_eq!(node.demand, vec!["i32", "i32"]);
        assert_eq!(node.answer, vec!["i32"]);
        assert!(node.problem.contains("Calculates"));
    }

    #[test]
    fn test_parse_rust_impl_method() {
        let source = r#"
struct Calculator;

impl Calculator {
    /// Multiplies two values.
    pub fn multiply(&self, x: i32, y: i32) -> i32 {
        x * y
    }

    fn internal_help(&self) {}
}
"#;
        let opts = ParserOptions::default();
        let nodes = extract_nodes_for_language(
            source,
            TriadLanguage::Rust,
            "lib.rs",
            &opts,
        )
        .unwrap();

        assert!(nodes.len() >= 2);
        let multiply = nodes.iter().find(|n| n.node_id == "Calculator.multiply").unwrap();
        assert_eq!(multiply.demand, vec!["i32", "i32"]);
        assert_eq!(multiply.answer, vec!["i32"]);
    }

    #[test]
    fn test_parse_rust_private_function_excluded() {
        let source = r#"
fn secret_helper(data: String) -> bool {
    true
}

pub fn public_api(data: String) -> bool {
    secret_helper(data)
}
"#;
        let mut opts = ParserOptions::default();
        opts.exclude_private_methods = true;
        let nodes = extract_nodes_for_language(
            source,
            TriadLanguage::Rust,
            "mod.rs",
            &opts,
        )
        .unwrap();

        // secret_helper should be excluded (no `pub`)
        let names: Vec<&str> = nodes.iter().map(|n| n.node_id.as_str()).collect();
        assert!(!names.contains(&"secret_helper"));
        assert!(names.contains(&"public_api"));
    }

    #[test]
    fn test_parse_typescript_function() {
        let source = r#"
/**
 * Processes a user request.
 */
function handleRequest(req: Request): Response {
    return new Response();
}
"#;
        let opts = ParserOptions::default();
        let nodes = extract_nodes_for_language(
            source,
            TriadLanguage::Typescript,
            "handler.ts",
            &opts,
        )
        .unwrap();

        assert_eq!(nodes.len(), 1);
        let node = &nodes[0];
        assert_eq!(node.node_id, "handleRequest");
        assert_eq!(node.demand, vec!["Request"]);
        assert_eq!(node.answer, vec!["Response"]);
        assert!(node.problem.contains("Processes"));
    }

    #[test]
    fn test_parse_typescript_class_method() {
        let source = r#"
class UserService {
    /** Creates a new user. */
    createUser(name: string, email: string): User {
        return new User();
    }

    private _validateEmail(email: string): boolean {
        return true;
    }
}
"#;
        let opts = ParserOptions::default();
        let nodes = extract_nodes_for_language(
            source,
            TriadLanguage::Typescript,
            "service.ts",
            &opts,
        )
        .unwrap();

        let create_user = nodes
            .iter()
            .find(|n| n.node_id == "UserService.createUser")
            .unwrap();
        assert_eq!(create_user.demand, vec!["string", "string"]);
        assert_eq!(create_user.answer, vec!["User"]);
    }

    #[test]
    fn test_parse_typescript_arrow_export() {
        let source = r#"
/** Fetches data from API. */
export const fetchData = async (id: string): Promise<Data> => {
    return {} as Data;
};
"#;
        let opts = ParserOptions::default();
        let nodes = extract_nodes_for_language(
            source,
            TriadLanguage::Typescript,
            "api.ts",
            &opts,
        )
        .unwrap();

        let fetch = nodes.iter().find(|n| n.node_id == "fetchData").unwrap();
        assert!(fetch.problem.contains("Fetches"));
        assert_eq!(fetch.demand, vec!["string"]);
    }

    #[test]
    fn test_is_helper_function() {
        let opts = ParserOptions::default();
        assert!(is_helper_function("build_url", &opts));
        assert!(is_helper_function("parse_input", &opts));
        assert!(!is_helper_function("execute", &opts));
        assert!(!is_helper_function("handleRequest", &opts));
    }

    #[test]
    fn test_extract_rust_param_types() {
        assert_eq!(
            extract_rust_param_types("a: i32, b: String"),
            vec!["i32", "String"]
        );
        assert_eq!(
            extract_rust_param_types("&self, x: i32"),
            vec!["i32"]
        );
        assert_eq!(
            extract_rust_param_types("&mut self, config: &Config"),
            vec!["Config"]
        );
        assert!(extract_rust_param_types("").is_empty());
    }

    #[test]
    fn test_extract_ts_param_types() {
        assert_eq!(
            extract_ts_param_types("name: string, age: number"),
            vec!["string", "number"]
        );
        assert_eq!(
            extract_ts_param_types("id: string = 'default'"),
            vec!["string"]
        );
        assert!(extract_ts_param_types("").is_empty());
    }
}
