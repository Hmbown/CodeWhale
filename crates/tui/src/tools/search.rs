//! Search tools: `grep_files` for code search
//!
//! These tools provide powerful code search capabilities within the workspace,
//! similar to ripgrep/grep functionality.

use super::spec::{
    ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, optional_bool, optional_str,
    optional_u64, required_str,
};
use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// Maximum number of results to return to avoid overwhelming output
const MAX_RESULTS: usize = 100;

/// Maximum file size to search (skip large binaries)
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10MB

/// Result of a grep match
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepMatch {
    pub file: String,
    pub line_number: usize,
    pub line: String,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

/// Tool for searching files using regex patterns
pub struct GrepFilesTool;

#[async_trait]
impl ToolSpec for GrepFilesTool {
    fn name(&self) -> &'static str {
        "grep_files"
    }

    fn description(&self) -> &'static str {
        "Search for a regex pattern in workspace files. Use this instead of `grep -r`, `rg`, or `find ... -exec grep` in `exec_shell` — pure-Rust, faster, and respects `.gitignore`. Returns matching lines with context (default: 2 lines before/after each match)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search (relative to workspace, default: .)"
                },
                "include": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Glob patterns for files to include (e.g., ['*.rs', '*.ts'])"
                },
                "exclude": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Glob patterns for files to exclude (e.g., ['*.min.js', 'node_modules/*'])"
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Number of context lines before and after each match (default: 2)"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Whether to perform case-insensitive matching (default: false)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 100)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let pattern_str = required_str(&input, "pattern")?;
        let path_str = optional_str(&input, "path").unwrap_or(".");
        let context_lines =
            usize::try_from(optional_u64(&input, "context_lines", 2)).unwrap_or(usize::MAX);
        let case_insensitive = optional_bool(&input, "case_insensitive", false);
        let max_results = usize::try_from(optional_u64(&input, "max_results", MAX_RESULTS as u64))
            .unwrap_or(MAX_RESULTS);

        // Parse include patterns
        let include_patterns: Vec<String> = input
            .get("include")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // Parse exclude patterns
        let exclude_patterns: Vec<String> =
            input.get("exclude").and_then(|v| v.as_array()).map_or_else(
                || {
                    // Default exclusions for common non-code directories
                    vec![
                        "node_modules/*".to_string(),
                        ".git/*".to_string(),
                        "target/*".to_string(),
                        "*.min.js".to_string(),
                        "*.min.css".to_string(),
                        "dist/*".to_string(),
                        "build/*".to_string(),
                        "__pycache__/*".to_string(),
                        ".venv/*".to_string(),
                        "venv/*".to_string(),
                    ]
                },
                |arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                },
            );

        // Build regex
        let regex_pattern = if case_insensitive {
            format!("(?i){pattern_str}")
        } else {
            pattern_str.to_string()
        };

        let regex = Regex::new(&regex_pattern)
            .map_err(|e| ToolError::invalid_input(format!("Invalid regex pattern: {e}")))?;

        // Resolve search path
        let search_path = context.resolve_path(path_str)?;

        // Collect files to search
        let files = collect_files(&search_path, &include_patterns, &exclude_patterns)?;

        // Search files
        let mut results: Vec<GrepMatch> = Vec::new();
        let mut files_searched = 0;
        let mut total_matches = 0;

        for file_path in files {
            if results.len() >= max_results {
                break;
            }

            // Skip files that are too large
            if let Ok(metadata) = fs::metadata(&file_path)
                && metadata.len() > MAX_FILE_SIZE
            {
                continue;
            }

            // Read file content
            let Ok(file_content) = fs::read_to_string(&file_path) else {
                continue; // Skip binary or unreadable files
            };

            files_searched += 1;
            let lines: Vec<&str> = file_content.lines().collect();

            for (line_idx, line) in lines.iter().enumerate() {
                if regex.is_match(line) {
                    total_matches += 1;

                    // Get context lines
                    let context_before: Vec<String> = (line_idx.saturating_sub(context_lines)
                        ..line_idx)
                        .filter_map(|i| lines.get(i).map(|s| (*s).to_string()))
                        .collect();

                    let context_after: Vec<String> = ((line_idx + 1)
                        ..=(line_idx + context_lines).min(lines.len() - 1))
                        .filter_map(|i| lines.get(i).map(|s| (*s).to_string()))
                        .collect();

                    // Get relative path from workspace
                    let relative_path = file_path
                        .strip_prefix(&context.workspace)
                        .unwrap_or(&file_path)
                        .to_string_lossy()
                        .to_string();

                    results.push(GrepMatch {
                        file: relative_path,
                        line_number: line_idx + 1,
                        line: (*line).to_string(),
                        context_before,
                        context_after,
                    });

                    if results.len() >= max_results {
                        break;
                    }
                }
            }
        }

        let matches_json: Vec<Value> = results
            .iter()
            .map(|item| grep_match_to_json(item, context_lines))
            .collect();

        // Build result. When context_lines == 1, return the single context
        // line as a string instead of a one-item array. That keeps the common
        // "show just the adjacent line" case easy for model callers to read.
        let result = json!({
            "matches": matches_json,
            "total_matches": total_matches,
            "files_searched": files_searched,
            "truncated": total_matches > max_results,
        });

        ToolResult::json(&result).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

fn grep_match_to_json(item: &GrepMatch, context_lines: usize) -> Value {
    if context_lines == 1 {
        json!({
            "file": item.file,
            "line_number": item.line_number,
            "line": item.line,
            "context_before": item.context_before.first().cloned().unwrap_or_default(),
            "context_after": item.context_after.first().cloned().unwrap_or_default(),
        })
    } else {
        json!(item)
    }
}

/// Collect files to search based on include/exclude patterns
fn collect_files(
    root: &Path,
    include_patterns: &[String],
    exclude_patterns: &[String],
) -> Result<Vec<PathBuf>, ToolError> {
    let mut files = Vec::new();

    if root.is_file() {
        files.push(root.to_path_buf());
        return Ok(files);
    }

    collect_files_recursive(root, root, include_patterns, exclude_patterns, &mut files)?;
    Ok(files)
}

fn collect_files_recursive(
    root: &Path,
    current: &Path,
    include_patterns: &[String],
    exclude_patterns: &[String],
    files: &mut Vec<PathBuf>,
) -> Result<(), ToolError> {
    let entries = fs::read_dir(current).map_err(|e| {
        ToolError::execution_failed(format!(
            "Failed to read directory {}: {}",
            current.display(),
            e
        ))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| ToolError::execution_failed(e.to_string()))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| {
            ToolError::execution_failed(format!(
                "Failed to inspect file type for {}: {}",
                path.display(),
                e
            ))
        })?;
        if file_type.is_symlink() {
            continue;
        }

        // Get relative path for pattern matching
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let relative_str = relative.to_string_lossy();

        // Check exclusions
        if should_exclude(&relative_str, exclude_patterns) {
            continue;
        }

        if file_type.is_dir() {
            collect_files_recursive(root, &path, include_patterns, exclude_patterns, files)?;
        } else if file_type.is_file() {
            // Check inclusions (if any specified)
            if include_patterns.is_empty() || should_include(&relative_str, include_patterns) {
                files.push(path);
            }
        }
    }

    Ok(())
}

/// Check if a path matches any of the exclude patterns
fn should_exclude(path: &str, patterns: &[String]) -> bool {
    for pattern in patterns {
        if matches_glob(path, pattern) {
            return true;
        }
    }
    false
}

/// Check if a path matches any of the include patterns
fn should_include(path: &str, patterns: &[String]) -> bool {
    for pattern in patterns {
        if matches_glob(path, pattern) {
            return true;
        }
    }
    false
}

/// Simple glob pattern matching
/// Supports: * (any chars), ** (any path), ? (single char)
pub(crate) fn matches_glob(path: &str, pattern: &str) -> bool {
    // Handle ** for any path
    if pattern.contains("**") {
        let parts: Vec<&str> = pattern.split("**").collect();
        if parts.len() == 2 {
            let prefix = parts[0].trim_end_matches('/');
            let suffix = parts[1].trim_start_matches('/');

            if !prefix.is_empty() && !path.starts_with(prefix) {
                return false;
            }
            if !suffix.is_empty() {
                return path.ends_with(suffix)
                    || path
                        .split('/')
                        .any(|part| matches_simple_glob(part, suffix));
            }
            return path.starts_with(prefix) || prefix.is_empty();
        }
    }

    // Handle patterns like "*.rs" - match against filename only
    if pattern.starts_with('*') && !pattern.contains('/') {
        let filename = path.rsplit('/').next().unwrap_or(path);
        return matches_simple_glob(filename, pattern);
    }

    // Handle patterns with path components
    if pattern.contains('/') {
        return matches_simple_glob(path, pattern);
    }

    // Match against filename
    let filename = path.rsplit('/').next().unwrap_or(path);
    matches_simple_glob(filename, pattern)
}

/// Simple glob matching for single path component
fn matches_simple_glob(text: &str, pattern: &str) -> bool {
    let mut text_chars = text.chars().peekable();
    let mut pattern_chars = pattern.chars().peekable();

    while let Some(p) = pattern_chars.next() {
        match p {
            '*' => {
                // Match zero or more characters
                let next_pattern: String = pattern_chars.collect();
                if next_pattern.is_empty() {
                    return true;
                }

                // Try matching at each position (use char-indices to stay on
                // UTF-8 boundaries — byte-index slicing panics on multi-byte
                // characters like 冰糖, see #249).
                let remaining: String = text_chars.collect();
                for (i, _) in remaining.char_indices() {
                    if matches_simple_glob(&remaining[i..], &next_pattern) {
                        return true;
                    }
                }
                // Also try the empty suffix at end of string
                if matches_simple_glob("", &next_pattern) {
                    return true;
                }
                return false;
            }
            '?' => {
                // Match exactly one character
                if text_chars.next().is_none() {
                    return false;
                }
            }
            c => {
                // Match literal character
                if text_chars.next() != Some(c) {
                    return false;
                }
            }
        }
    }

    text_chars.next().is_none()
}

// === Unit Tests ===

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{Value, json};
    use tempfile::tempdir;

    use crate::tools::spec::{ApprovalRequirement, ToolContext, ToolSpec};

    use super::{GrepFilesTool, matches_glob};

    #[test]
    fn test_matches_glob_star() {
        assert!(matches_glob("test.rs", "*.rs"));
        assert!(matches_glob("foo.rs", "*.rs"));
        assert!(!matches_glob("test.ts", "*.rs"));
        assert!(!matches_glob("test.rs.bak", "*.rs"));
    }

    #[test]
    fn test_matches_glob_question() {
        assert!(matches_glob("test.rs", "test.??"));
        assert!(!matches_glob("test.rs", "test.?"));
    }

    #[test]
    fn test_matches_glob_double_star() {
        assert!(matches_glob("src/main.rs", "src/**"));
        assert!(matches_glob("src/lib/mod.rs", "src/**"));
        assert!(matches_glob("node_modules/pkg/index.js", "node_modules/*"));
    }

    #[test]
    fn test_matches_glob_path() {
        assert!(matches_glob("src/main.rs", "src/*.rs"));
        assert!(!matches_glob("lib/main.rs", "src/*.rs"));
    }

    /// Regression for #249: byte-index slicing panics on multi-byte
    /// characters inside filenames like `dialogue_line__冰糖.mp3`.
    #[test]
    fn test_matches_glob_unicode_filename() {
        let filename = "dialogue_line__冰糖.mp3";
        // The filename should match *.mp3 without panicking.
        assert!(matches_glob(filename, "*.mp3"));
        // Asterisk matching against multi-byte characters must succeed.
        assert!(matches_glob(filename, "dialogue_line__*"));
        // Literal multi-byte characters inside the pattern must match.
        assert!(matches_glob(filename, "*冰糖*"));
        // Non-matching pattern must not panic either.
        assert!(!matches_glob(filename, "nonexistent*"));
    }

    #[tokio::test]
    async fn test_grep_files_basic() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // Create test files
        fs::write(
            tmp.path().join("test.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .expect("write");
        fs::write(
            tmp.path().join("lib.rs"),
            "pub fn hello() {}\npub fn world() {}\n",
        )
        .expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "fn"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("main"));
        assert!(result.content.contains("hello"));
    }

    #[tokio::test]
    async fn test_grep_files_with_context() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::write(
            tmp.path().join("test.txt"),
            "line1\nline2\nMATCH\nline4\nline5\n",
        )
        .expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "MATCH", "context_lines": 1}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("line2")); // context before
        assert!(result.content.contains("line4")); // context after

        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["context_before"], "line2");
        assert_eq!(matches[0]["context_after"], "line4");
        assert!(matches[0]["context_before"].is_string());
        assert!(matches[0]["context_after"].is_string());
    }

    #[tokio::test]
    async fn test_grep_files_multi_line_context_remains_arrays() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::write(tmp.path().join("test.txt"), "a\nb\nMATCH\nd\ne\n").expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "MATCH", "context_lines": 2}), &ctx)
            .await
            .expect("execute");

        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["context_before"], json!(["a", "b"]));
        assert_eq!(matches[0]["context_after"], json!(["d", "e"]));
    }

    #[tokio::test]
    async fn test_grep_files_case_insensitive() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::write(
            tmp.path().join("test.txt"),
            "Hello World\nHELLO WORLD\nhello world\n",
        )
        .expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "hello", "case_insensitive": true}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        // Should find all 3 lines
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 3);
    }

    #[tokio::test]
    async fn test_grep_files_include_filter() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::write(tmp.path().join("test.rs"), "fn test() {}\n").expect("write");
        fs::write(tmp.path().join("test.js"), "function test() {}\n").expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "test", "include": ["*.rs"]}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        // Should only match .rs file
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        let file = matches[0]["file"].as_str().unwrap();
        assert!(
            file.rsplit('.')
                .next()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"))
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_grep_files_does_not_follow_symlinked_files() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().join("workspace");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root).expect("mkdir workspace");
        std::fs::create_dir_all(&outside).expect("mkdir outside");
        let outside_file = outside.join("secret.txt");
        fs::write(&outside_file, "NEEDLE\n").expect("write outside");
        std::os::unix::fs::symlink(&outside_file, root.join("secret.txt")).expect("symlink");

        let ctx = ToolContext::new(root);
        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "NEEDLE"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 0);
        assert_eq!(parsed["files_searched"].as_u64().unwrap(), 0);
    }

    #[tokio::test]
    async fn test_grep_files_invalid_regex() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let tool = GrepFilesTool;
        let result = tool.execute(json!({"pattern": "[invalid"}), &ctx).await;

        assert!(result.is_err());
    }

    #[test]
    fn test_grep_files_tool_properties() {
        let tool = GrepFilesTool;
        assert_eq!(tool.name(), "grep_files");
        assert!(tool.is_read_only());
        assert!(tool.is_sandboxable());
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Auto);
    }

    #[test]
    fn test_parallel_support_flags() {
        let tool = GrepFilesTool;
        assert!(tool.supports_parallel());
    }
}
