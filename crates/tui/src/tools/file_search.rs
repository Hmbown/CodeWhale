//! File search tool with fuzzy matching and scoring.

use std::cmp::Ordering;
use std::path::Path;

use async_trait::async_trait;
use ignore::WalkBuilder;
use serde::Serialize;
use serde_json::{Value, json};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_str, optional_u64, required_str,
};

#[derive(Debug, Clone, Serialize)]
struct FileSearchMatch {
    path: String,
    name: String,
    score: f64,
}

pub struct FileSearchTool;

#[async_trait]
impl ToolSpec for FileSearchTool {
    fn name(&self) -> &'static str {
        "file_search"
    }

    fn description(&self) -> &'static str {
        "Search for files using fuzzy matching with score-based ranking."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (file name or path fragment)."
                },
                "path": {
                    "type": "string",
                    "description": "Optional base path to search (relative to workspace)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 20)."
                },
                "extensions": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of file extensions to include (e.g. [\"rs\", \"md\"])."
                }
            },
            "required": ["query"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let query = required_str(&input, "query")?.trim();
        if query.is_empty() {
            return Err(ToolError::invalid_input("query cannot be empty"));
        }

        let limit = optional_u64(&input, "limit", 20).clamp(1, 200) as usize;
        let base_path = match optional_str(&input, "path") {
            Some(path) if !path.trim().is_empty() => context.resolve_path(path)?,
            _ => context.workspace.clone(),
        };

        let extensions = parse_extensions(&input);
        let matches = search_files(query, &base_path, extensions, limit)?;
        ToolResult::json(&matches).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

fn parse_extensions(input: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(values) = input.get("extensions").and_then(|v| v.as_array()) {
        for value in values {
            if let Some(ext) = value.as_str() {
                let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
                if !ext.is_empty() {
                    out.push(ext);
                }
            }
        }
    }
    if out.is_empty()
        && let Some(value) = input.get("extension").and_then(|v| v.as_str())
    {
        let ext = value.trim().trim_start_matches('.').to_ascii_lowercase();
        if !ext.is_empty() {
            out.push(ext);
        }
    }
    out
}

fn search_files(
    query: &str,
    base_path: &Path,
    extensions: Vec<String>,
    limit: usize,
) -> Result<Vec<FileSearchMatch>, ToolError> {
    if !base_path.exists() {
        return Err(ToolError::invalid_input(format!(
            "Base path does not exist: {}",
            base_path.display()
        )));
    }

    let query_norm = query.to_ascii_lowercase();
    let mut results: Vec<FileSearchMatch> = Vec::new();

    let mut builder = WalkBuilder::new(base_path);
    builder.hidden(false).follow_links(false).require_git(false);
    let walker = builder.build();

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }

        let path = entry.path();
        if !extensions.is_empty() && !extension_matches(path, &extensions) {
            continue;
        }

        let rel_path = path
            .strip_prefix(base_path)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        let name = file_name(path);

        let score = match score_match(&query_norm, &rel_path, &name) {
            Some(score) => score,
            None => continue,
        };

        results.push(FileSearchMatch {
            path: rel_path,
            name,
            score,
        });
    }

    results.sort_by(compare_match);
    if results.len() > limit {
        results.truncate(limit);
    }
    Ok(results)
}

fn extension_matches(path: &Path, extensions: &[String]) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    extensions.iter().any(|wanted| wanted == &ext)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn score_match(query: &str, rel_path: &str, name: &str) -> Option<f64> {
    let path_norm = rel_path.to_ascii_lowercase();
    let name_norm = name.to_ascii_lowercase();

    if name_norm == query {
        return Some(1.0);
    }
    if path_norm == query {
        return Some(0.98);
    }

    if name_norm.starts_with(query) {
        return Some(0.9 + length_bonus(query, &name_norm));
    }
    if path_norm.starts_with(query) {
        return Some(0.85 + length_bonus(query, &path_norm));
    }

    if name_norm.contains(query) {
        return Some(0.75 + length_bonus(query, &name_norm));
    }
    if path_norm.contains(query) {
        return Some(0.7 + length_bonus(query, &path_norm));
    }

    if let Some(score) = fuzzy_score(query, &name_norm) {
        return Some(0.6 + 0.4 * score);
    }
    if let Some(score) = fuzzy_score(query, &path_norm) {
        return Some(0.55 + 0.4 * score);
    }

    None
}

fn length_bonus(query: &str, target: &str) -> f64 {
    let q_len = query.chars().count().max(1) as f64;
    let t_len = target.chars().count().max(1) as f64;
    (q_len / t_len).min(1.0) * 0.08
}

fn fuzzy_score(query: &str, target: &str) -> Option<f64> {
    let mut positions = Vec::new();
    let mut query_chars = query.chars();
    let mut current = query_chars.next()?;

    for (idx, ch) in target.chars().enumerate() {
        if ch == current {
            positions.push(idx);
            if let Some(next) = query_chars.next() {
                current = next;
            } else {
                break;
            }
        }
    }

    if positions.len() != query.chars().count() {
        return None;
    }

    let first = *positions.first().unwrap_or(&0) as f64;
    let last = *positions.last().unwrap_or(&0) as f64;
    let span = (last - first + 1.0).max(1.0);
    let query_len = query.chars().count().max(1) as f64;
    let target_len = target.chars().count().max(1) as f64;

    let density = (query_len / span).min(1.0);
    let coverage = (query_len / target_len).min(1.0);
    Some((density * 0.7 + coverage * 0.3).min(1.0))
}

fn compare_match(a: &FileSearchMatch, b: &FileSearchMatch) -> Ordering {
    b.score
        .partial_cmp(&a.score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| a.path.cmp(&b.path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_file_search_basic() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src")).expect("mkdir");
        std::fs::write(root.join("src").join("main.rs"), "fn main() {}\n").expect("write");
        std::fs::write(root.join("README.md"), "docs\n").expect("write");

        let ctx = ToolContext::new(root.to_path_buf());
        let tool = FileSearchTool;
        let result = tool
            .execute(json!({"query": "main", "limit": 5}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("main.rs"));
    }

    #[tokio::test]
    async fn test_file_search_respects_gitignore() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::write(root.join(".gitignore"), "ignored.txt\n").expect("write");
        std::fs::write(root.join("ignored.txt"), "nope\n").expect("write");
        std::fs::write(root.join("keep.txt"), "ok\n").expect("write");

        let ctx = ToolContext::new(root.to_path_buf());
        let tool = FileSearchTool;
        let result = tool
            .execute(json!({"query": "txt"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(!result.content.contains("ignored.txt"));
        assert!(result.content.contains("keep.txt"));
    }

    #[tokio::test]
    async fn test_file_search_extension_filter() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::write(root.join("main.rs"), "fn main() {}\n").expect("write");
        std::fs::write(root.join("notes.md"), "docs\n").expect("write");

        let ctx = ToolContext::new(root.to_path_buf());
        let tool = FileSearchTool;
        let result = tool
            .execute(json!({"query": "m", "extensions": ["rs"]}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("main.rs"));
        assert!(!result.content.contains("notes.md"));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_file_search_does_not_follow_symlinked_files() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().join("workspace");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root).expect("mkdir workspace");
        std::fs::create_dir_all(&outside).expect("mkdir outside");
        let outside_file = outside.join("secret.txt");
        std::fs::write(&outside_file, "outside\n").expect("write outside");
        std::os::unix::fs::symlink(&outside_file, root.join("secret.txt")).expect("symlink");

        let ctx = ToolContext::new(root);
        let tool = FileSearchTool;
        let result = tool
            .execute(json!({"query": "secret"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(!result.content.contains("secret.txt"));
    }
}
