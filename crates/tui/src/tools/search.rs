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
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Maximum number of results to return to avoid overwhelming output
const MAX_RESULTS: usize = 100;

/// Maximum file size to search (skip large binaries)
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10MB

/// Hard cap on a single grep_files run. The directory walk plus per-file regex
/// is synchronous blocking work; without this it can run for minutes on a large
/// tree. Mirrors the file_search tool so both blocking searches behave the same.
const GREP_FILES_TIMEOUT: Duration = Duration::from_secs(30);

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
        let context_lines = usize::try_from(optional_u64(&input, "context_lines", 2))
            .unwrap_or(usize::MAX)
            .min(1000);
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
                    // Default exclusions for common non-code directories.
                    // Bare directory names skip the directory traversal entirely;
                    // `dir/*` filters files inside if the directory is already
                    // being walked (belt-and-suspenders — see #2200).
                    vec![
                        "node_modules".to_string(),
                        "node_modules/*".to_string(),
                        ".git".to_string(),
                        ".git/*".to_string(),
                        "target".to_string(),
                        "target/*".to_string(),
                        "*.min.js".to_string(),
                        "*.min.css".to_string(),
                        "dist".to_string(),
                        "dist/*".to_string(),
                        "build".to_string(),
                        "build/*".to_string(),
                        "__pycache__".to_string(),
                        "__pycache__/*".to_string(),
                        ".venv".to_string(),
                        ".venv/*".to_string(),
                        "venv".to_string(),
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

        let workspace = context.workspace.clone();
        let cancel_token = context.cancel_token.clone();
        let follow_symlinks = context.follow_symlinks;

        // The directory walk and per-file regex are synchronous blocking work.
        // Run them on a blocking worker bounded by a hard timeout so a huge tree
        // can't pin the async runtime and leave the stop button unresponsive.
        let result = run_blocking_grep(GREP_FILES_TIMEOUT, cancel_token.clone(), move || {
            let cancel_token = cancel_token.as_ref();

            // Stream the walk: each file is searched as it is discovered and
            // the traversal stops as soon as the match budget is exhausted.
            // Files are never materialized in a big Vec and file contents are
            // read line-by-line, so memory stays bounded by the result set.
            let mut results: Vec<GrepMatch> = Vec::new();
            let mut files_searched = 0;
            let mut total_matches = 0;

            visit_files(
                &search_path,
                &include_patterns,
                &exclude_patterns,
                cancel_token,
                follow_symlinks,
                &mut |file_path| {
                    if results.len() >= max_results {
                        return Ok(WalkControl::Stop);
                    }
                    check_cancelled(cancel_token)?;

                    // Skip files that are too large
                    if let Ok(metadata) = fs::metadata(file_path)
                        && metadata.len() > MAX_FILE_SIZE
                    {
                        return Ok(WalkControl::Continue);
                    }

                    // Get relative path from workspace
                    let relative_path = file_path
                        .strip_prefix(&workspace)
                        .unwrap_or(file_path)
                        .to_string_lossy()
                        .to_string();

                    let budget = max_results - results.len();
                    let Some(file_matches) = search_file_streaming(
                        file_path,
                        &relative_path,
                        &regex,
                        context_lines,
                        budget,
                        cancel_token,
                    )?
                    else {
                        return Ok(WalkControl::Continue); // Skip binary or unreadable files
                    };

                    files_searched += 1;
                    total_matches += file_matches.len();
                    results.extend(file_matches);
                    Ok(WalkControl::Continue)
                },
            )?;

            let matches_json: Vec<Value> = results
                .iter()
                .map(|item| grep_match_to_json(item, context_lines))
                .collect();

            // Build result. When context_lines == 1, return the single context
            // line as a string instead of a one-item array. That keeps the common
            // "show just the adjacent line" case easy for model callers to read.
            Ok(json!({
                "matches": matches_json,
                "total_matches": total_matches,
                "files_searched": files_searched,
                "truncated": total_matches > max_results,
            }))
        })
        .await?;

        ToolResult::json(&result).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

/// Run the synchronous grep walk on a blocking worker, cancellable via the
/// token and bounded by `timeout`. Mirrors `run_blocking_file_search`.
async fn run_blocking_grep<F>(
    timeout: Duration,
    cancel_token: Option<CancellationToken>,
    search: F,
) -> Result<Value, ToolError>
where
    F: FnOnce() -> Result<Value, ToolError> + Send + 'static,
{
    if cancel_token
        .as_ref()
        .is_some_and(CancellationToken::is_cancelled)
    {
        return Err(grep_cancelled());
    }

    let task = tokio::task::spawn_blocking(search);
    let result = match cancel_token {
        Some(token) => {
            tokio::select! {
                biased;
                () = token.cancelled() => return Err(grep_cancelled()),
                result = tokio::time::timeout(timeout, task) => result,
            }
        }
        None => tokio::time::timeout(timeout, task).await,
    };

    let joined = result.map_err(|_| grep_timeout(timeout))?;
    joined.map_err(|err| {
        ToolError::execution_failed(format!("grep_files worker failed before completion: {err}"))
    })?
}

fn grep_cancelled() -> ToolError {
    ToolError::execution_failed("grep_files cancelled before completion")
}

fn grep_timeout(timeout: Duration) -> ToolError {
    ToolError::Timeout {
        seconds: timeout.as_secs().max(1),
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

/// Search a single file line-by-line with a small ring buffer for
/// before-context, so file contents are never fully materialized.
///
/// Returns `Ok(None)` when the file is unreadable or contains invalid UTF-8
/// anywhere — the same "skip binary or unreadable files" semantics as the
/// previous `read_to_string` implementation, which required the whole file to
/// be valid before contributing any match. At most `budget` matches are
/// recorded; the scan still runs to EOF so late invalid bytes disqualify the
/// file and pending after-context is completed.
fn search_file_streaming(
    path: &Path,
    relative_path: &str,
    regex: &Regex,
    context_lines: usize,
    budget: usize,
    cancel_token: Option<&CancellationToken>,
) -> Result<Option<Vec<GrepMatch>>, ToolError> {
    let Ok(file) = fs::File::open(path) else {
        return Ok(None);
    };
    let mut reader = std::io::BufReader::new(file);
    let mut raw: Vec<u8> = Vec::new();
    let mut before: VecDeque<String> = VecDeque::new();
    let mut matches: Vec<GrepMatch> = Vec::new();
    // Matches still waiting for after-context lines: (index into `matches`,
    // lines still needed). Entries complete in FIFO order.
    let mut pending: VecDeque<(usize, usize)> = VecDeque::new();
    let mut line_idx = 0usize;

    loop {
        raw.clear();
        let n = match reader.read_until(b'\n', &mut raw) {
            Ok(n) => n,
            Err(_) => return Ok(None),
        };
        if n == 0 {
            break;
        }
        check_cancelled(cancel_token)?;

        // Mirror `str::lines`: strip the trailing '\n', and a '\r' only when
        // it directly precedes that '\n'.
        let mut end = raw.len();
        if raw[..end].ends_with(b"\n") {
            end -= 1;
            if raw[..end].ends_with(b"\r") {
                end -= 1;
            }
        }
        let Ok(line) = std::str::from_utf8(&raw[..end]) else {
            return Ok(None);
        };

        for (idx, remaining) in &mut pending {
            matches[*idx].context_after.push(line.to_string());
            *remaining -= 1;
        }
        while pending
            .front()
            .is_some_and(|(_, remaining)| *remaining == 0)
        {
            pending.pop_front();
        }

        if matches.len() < budget && regex.is_match(line) {
            matches.push(GrepMatch {
                file: relative_path.to_string(),
                line_number: line_idx + 1,
                line: line.to_string(),
                context_before: before.iter().cloned().collect(),
                context_after: Vec::new(),
            });
            if context_lines > 0 {
                pending.push_back((matches.len() - 1, context_lines));
            }
        }

        if context_lines > 0 {
            if before.len() == context_lines {
                before.pop_front();
            }
            before.push_back(line.to_string());
        }
        line_idx += 1;
    }

    Ok(Some(matches))
}

/// Flow control for the streaming file walk.
enum WalkControl {
    Continue,
    Stop,
}

/// Walk files matching the include/exclude patterns, invoking `visit` for
/// each one in traversal order. The walk stops early when `visit` returns
/// [`WalkControl::Stop`].
fn visit_files(
    root: &Path,
    include_patterns: &[String],
    exclude_patterns: &[String],
    cancel_token: Option<&CancellationToken>,
    follow_symlinks: bool,
    visit: &mut dyn FnMut(&Path) -> Result<WalkControl, ToolError>,
) -> Result<(), ToolError> {
    let mut visited_dirs: HashSet<PathBuf> = HashSet::new();
    check_cancelled(cancel_token)?;

    if root.is_file() {
        visit(root)?;
        return Ok(());
    }

    if follow_symlinks && let Ok(canonical_root) = root.canonicalize() {
        visited_dirs.insert(canonical_root);
    }

    visit_files_recursive(
        root,
        root,
        include_patterns,
        exclude_patterns,
        cancel_token,
        &mut visited_dirs,
        follow_symlinks,
        visit,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn visit_files_recursive(
    root: &Path,
    current: &Path,
    include_patterns: &[String],
    exclude_patterns: &[String],
    cancel_token: Option<&CancellationToken>,
    visited_dirs: &mut HashSet<PathBuf>,
    follow_symlinks: bool,
    visit: &mut dyn FnMut(&Path) -> Result<WalkControl, ToolError>,
) -> Result<WalkControl, ToolError> {
    check_cancelled(cancel_token)?;

    let entries = fs::read_dir(current).map_err(|e| {
        ToolError::execution_failed(format!(
            "Failed to read directory {}: {}",
            current.display(),
            e
        ))
    })?;

    for entry in entries {
        check_cancelled(cancel_token)?;

        let entry = entry.map_err(|e| ToolError::execution_failed(e.to_string()))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| {
            ToolError::execution_failed(format!(
                "Failed to inspect file type for {}: {}",
                path.display(),
                e
            ))
        })?;
        if file_type.is_symlink() && !follow_symlinks {
            continue;
        }

        // Get relative path for pattern matching
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let relative_str = relative.to_string_lossy();

        // Check exclusions
        if should_exclude(&relative_str, exclude_patterns) {
            continue;
        }

        // When following symlinks, resolve the target type for directories
        // and files so symlinked dirs are traversed and symlinked files are
        // included.
        let effective_type = if file_type.is_symlink() && follow_symlinks {
            match fs::metadata(&path) {
                Ok(meta) => meta.file_type(),
                Err(_) => continue,
            }
        } else {
            file_type
        };

        if effective_type.is_dir() {
            if follow_symlinks {
                let canonical_dir = match path.canonicalize() {
                    Ok(canonical) => canonical,
                    Err(_) => continue,
                };
                if !visited_dirs.insert(canonical_dir) {
                    continue;
                }
            }
            if let WalkControl::Stop = visit_files_recursive(
                root,
                &path,
                include_patterns,
                exclude_patterns,
                cancel_token,
                visited_dirs,
                follow_symlinks,
                visit,
            )? {
                return Ok(WalkControl::Stop);
            }
        } else if effective_type.is_file() {
            // Check inclusions (if any specified)
            if (include_patterns.is_empty() || should_include(&relative_str, include_patterns))
                && let WalkControl::Stop = visit(&path)?
            {
                return Ok(WalkControl::Stop);
            }
        }
    }

    Ok(WalkControl::Continue)
}

fn check_cancelled(cancel_token: Option<&CancellationToken>) -> Result<(), ToolError> {
    if cancel_token.is_some_and(CancellationToken::is_cancelled) {
        return Err(ToolError::execution_failed(
            "search cancelled before completion",
        ));
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
    use tokio_util::sync::CancellationToken;

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
    #[cfg(unix)]
    async fn test_grep_files_default_mode_skips_symlinked_directories_but_keeps_real_files() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        let real_dir = workspace.join("real");
        std::fs::create_dir_all(&real_dir).expect("mkdir workspace");
        fs::write(real_dir.join("needle.txt"), "NEEDLE\n").expect("write real file");
        std::os::unix::fs::symlink(&workspace, real_dir.join("loop")).expect("symlink loop");

        let ctx = ToolContext::new(workspace);
        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "NEEDLE"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 1);
        assert_eq!(parsed["files_searched"].as_u64().unwrap(), 1);
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert!(
            matches[0]["file"]
                .as_str()
                .unwrap()
                .ends_with("real/needle.txt")
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_grep_files_follow_symlinks_avoids_directory_cycles() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        let real_dir = workspace.join("real");
        fs::create_dir_all(&real_dir).expect("mkdir");
        fs::write(real_dir.join("needle.txt"), "NEEDLE\n").expect("write");
        std::os::unix::fs::symlink(&workspace, real_dir.join("loop")).expect("symlink loop");

        let ctx = ToolContext::new(workspace).with_follow_symlinks(true);
        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "NEEDLE"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 1);
        assert_eq!(parsed["files_searched"].as_u64().unwrap(), 1);
        let matches = parsed["matches"].as_array().unwrap();
        assert!(matches[0]["file"].as_str().unwrap().ends_with("needle.txt"));
    }

    #[tokio::test]
    async fn test_grep_files_invalid_regex() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let tool = GrepFilesTool;
        let result = tool.execute(json!({"pattern": "[invalid"}), &ctx).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_grep_files_respects_cancel_token() {
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("test.txt"), "needle\n").expect("write");
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();
        let ctx = ToolContext::new(tmp.path().to_path_buf()).with_cancel_token(cancel_token);

        let tool = GrepFilesTool;
        let err = tool
            .execute(json!({"pattern": "needle"}), &ctx)
            .await
            .expect_err("cancelled grep should return an error");

        assert!(
            format!("{err:?}").contains("cancelled"),
            "unexpected error: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_grep_files_streaming_stops_at_max_results() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // Two files with many matches each; the walk must stop once the
        // budget is exhausted without dropping context for the last match.
        for name in ["a.txt", "b.txt"] {
            let body: String = (1..=20).map(|n| format!("needle {n}\n")).collect();
            fs::write(tmp.path().join(name), body).expect("write");
        }

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "needle", "max_results": 5}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 5);
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 5);
        // All five matches must come from the first file walked, in file
        // order (streaming preserves walk order).
        let first_file = matches[0]["file"].as_str().unwrap().to_string();
        for m in matches {
            assert_eq!(m["file"].as_str().unwrap(), first_file);
        }
        // The final in-budget match still gets its full after-context even
        // though the match budget was exhausted on it.
        assert_eq!(
            matches[4]["context_after"],
            json!(["needle 6", "needle 7"]),
            "last match must keep after-context lines"
        );
    }

    #[tokio::test]
    async fn test_grep_files_ring_buffer_context_matches_full_read() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // Matches at the start, middle, and end of the file exercise the
        // partial before-context (ring not yet full) and truncated
        // after-context (EOF) paths.
        fs::write(
            tmp.path().join("ctx.txt"),
            "MATCH first\nb1\nb2\nb3\nMATCH mid\na1\na2\na3\nMATCH last\n",
        )
        .expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "MATCH", "context_lines": 2}), &ctx)
            .await
            .expect("execute");

        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0]["context_before"], json!([]));
        assert_eq!(matches[0]["context_after"], json!(["b1", "b2"]));
        assert_eq!(matches[1]["context_before"], json!(["b2", "b3"]));
        assert_eq!(matches[1]["context_after"], json!(["a1", "a2"]));
        assert_eq!(matches[2]["context_before"], json!(["a2", "a3"]));
        assert_eq!(matches[2]["context_after"], json!([]));
        assert_eq!(matches[2]["line_number"].as_u64().unwrap(), 9);
    }

    #[tokio::test]
    async fn test_grep_files_streaming_skips_invalid_utf8_files() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // Invalid UTF-8 after a matching line: the whole file must be
        // skipped, matching the historical read_to_string behavior.
        fs::write(
            tmp.path().join("binary.txt"),
            [b"needle\n".as_slice(), &[0xFF, 0xFE, 0x00]].concat(),
        )
        .expect("write");
        fs::write(tmp.path().join("clean.txt"), "needle\n").expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "needle"}), &ctx)
            .await
            .expect("execute");

        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 1);
        assert_eq!(parsed["files_searched"].as_u64().unwrap(), 1);
        let matches = parsed["matches"].as_array().unwrap();
        assert!(matches[0]["file"].as_str().unwrap().ends_with("clean.txt"));
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
