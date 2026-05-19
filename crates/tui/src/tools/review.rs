//! Tool for structured code reviews of files, diffs, or pull requests.

use std::fs;
use std::path::Path;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::DeepSeekClient;
use crate::dependencies::ExternalTool;
use crate::llm_client::LlmClient;
use crate::models::{ContentBlock, Message, MessageRequest, SystemPrompt, Usage};
use crate::utils::truncate_with_ellipsis;

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_bool, optional_str, optional_u64, required_str,
};

const DEFAULT_MAX_CHARS: usize = 200_000;
const MAX_MAX_CHARS: usize = 1_000_000;
const REVIEW_MAX_TOKENS: u32 = 2048;
const FALLBACK_MAX_CHARS: usize = 4000;

const REVIEW_SYSTEM_PROMPT: &str = "You are a senior code reviewer. Return ONLY valid JSON with \
the following schema:\n\
{\n\
  \"summary\": \"short overview\",\n\
  \"issues\": [\n\
    {\n\
      \"severity\": \"error|warning|info\",\n\
      \"title\": \"issue title\",\n\
      \"description\": \"details and impact\",\n\
      \"path\": \"relative/file/path or null\",\n\
      \"line\": 123\n\
    }\n\
  ],\n\
  \"suggestions\": [\n\
    {\n\
      \"path\": \"relative/file/path or null\",\n\
      \"line\": 123,\n\
      \"suggestion\": \"actionable improvement\"\n\
    }\n\
  ],\n\
  \"overall_assessment\": \"final assessment\"\n\
}\n\
If a field is unknown, use an empty string or null. Prioritize correctness and missing tests.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewIssue {
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSuggestion {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub suggestion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewOutput {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub issues: Vec<ReviewIssue>,
    #[serde(default)]
    pub suggestions: Vec<ReviewSuggestion>,
    #[serde(default)]
    pub overall_assessment: String,
}

impl ReviewOutput {
    #[must_use]
    pub fn from_str(raw: &str) -> Self {
        if let Some(parsed) = parse_review_output_json(raw) {
            return parsed.normalize();
        }
        if let Some(json_block) = extract_json_block(raw)
            && let Some(parsed) = parse_review_output_json(json_block)
        {
            return parsed.normalize();
        }
        ReviewOutput::fallback(raw)
    }

    fn fallback(raw: &str) -> Self {
        let trimmed = raw.trim();
        let summary = if trimmed.is_empty() {
            "Review completed but no structured output was returned.".to_string()
        } else {
            truncate_with_ellipsis(trimmed, FALLBACK_MAX_CHARS, "\n...[truncated]\n")
        };
        Self {
            summary,
            issues: Vec::new(),
            suggestions: Vec::new(),
            overall_assessment: String::new(),
        }
    }

    fn normalize(mut self) -> Self {
        self.summary = self.summary.trim().to_string();
        self.overall_assessment = self.overall_assessment.trim().to_string();
        for issue in &mut self.issues {
            issue.severity = normalize_severity(&issue.severity);
            issue.title = issue.title.trim().to_string();
            issue.description = issue.description.trim().to_string();
            issue.path = normalize_optional(issue.path.take());
        }
        for suggestion in &mut self.suggestions {
            suggestion.suggestion = suggestion.suggestion.trim().to_string();
            suggestion.path = normalize_optional(suggestion.path.take());
        }
        self
    }
}

fn parse_review_output_json(raw: &str) -> Option<ReviewOutput> {
    if let Ok(parsed) = serde_json::from_str::<ReviewOutput>(raw) {
        return Some(parsed);
    }

    let Value::String(inner) = serde_json::from_str::<Value>(raw).ok()? else {
        return None;
    };
    if inner.trim().is_empty() || inner == raw {
        return None;
    }
    parse_review_output_json(&inner)
}

pub struct ReviewTool {
    client: Option<DeepSeekClient>,
    model: String,
}

impl ReviewTool {
    #[must_use]
    pub fn new(client: Option<DeepSeekClient>, model: String) -> Self {
        Self { client, model }
    }
}

#[async_trait]
impl ToolSpec for ReviewTool {
    fn name(&self) -> &'static str {
        "review"
    }

    fn description(&self) -> &'static str {
        "Run a structured code review for a file, git diff, or GitHub pull request."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "string",
                    "description": "File path, PR URL, or the literal 'diff'/'staged' for git diff review."
                },
                "kind": {
                    "type": "string",
                    "description": "Optional explicit target type: file, diff, or pr."
                },
                "base": {
                    "type": "string",
                    "description": "Optional git base ref when using diff target (e.g. origin/main)."
                },
                "staged": {
                    "type": "boolean",
                    "description": "Review staged changes when using diff target (default: false)."
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum characters to include from the source (default: 200000)."
                }
            },
            "required": ["target"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Network]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let Some(client) = self.client.clone() else {
            return Err(ToolError::not_available(
                "Review tool requires an active DeepSeek client".to_string(),
            ));
        };

        let target = required_str(&input, "target")?.trim();
        if target.is_empty() {
            return Err(ToolError::invalid_input("target cannot be empty"));
        }

        let kind = optional_str(&input, "kind").map(|s| s.trim().to_ascii_lowercase());
        let base = optional_str(&input, "base").map(|s| s.trim().to_string());
        let staged = optional_bool(&input, "staged", false);
        let max_chars =
            usize::try_from(optional_u64(&input, "max_chars", DEFAULT_MAX_CHARS as u64))
                .unwrap_or(DEFAULT_MAX_CHARS)
                .clamp(1, MAX_MAX_CHARS);

        let source =
            resolve_review_source(target, kind.as_deref(), staged, base.as_deref(), context)?;
        let prompt = build_review_prompt(&source, max_chars);

        let request = MessageRequest {
            model: self.model.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: prompt,
                    cache_control: None,
                }],
            }],
            max_tokens: REVIEW_MAX_TOKENS,
            system: Some(SystemPrompt::Text(REVIEW_SYSTEM_PROMPT.to_string())),
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: Some(false),
            temperature: Some(0.2),
            top_p: Some(0.9),
        };

        let response = client
            .create_message(request)
            .await
            .map_err(|e| ToolError::execution_failed(format!("Review request failed: {e}")))?;

        let response_text = extract_text(&response.content);
        let output = ReviewOutput::from_str(&response_text);
        let metadata = review_usage_metadata(&response.model, &response.usage);
        let result =
            ToolResult::json(&output).map_err(|e| ToolError::execution_failed(e.to_string()))?;
        Ok(result.with_metadata(metadata))
    }
}

fn review_usage_metadata(model: &str, usage: &Usage) -> Value {
    json!({
        "tool": "review",
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "child_model": model,
        "child_input_tokens": usage.input_tokens,
        "child_output_tokens": usage.output_tokens,
        "child_prompt_cache_hit_tokens": usage.prompt_cache_hit_tokens,
        "child_prompt_cache_miss_tokens": usage.prompt_cache_miss_tokens,
        "child_reasoning_tokens": usage.reasoning_tokens,
    })
}

enum ReviewSource {
    File { display: String, content: String },
    Diff { label: String, diff: String },
    PullRequest { label: String, diff: String },
}

fn resolve_review_source(
    target: &str,
    kind: Option<&str>,
    staged: bool,
    base: Option<&str>,
    context: &ToolContext,
) -> Result<ReviewSource, ToolError> {
    if let Some(kind) = kind {
        return match kind {
            "file" => resolve_file_target(target, context),
            "diff" => resolve_diff_target(context.workspace.as_path(), staged, base).map(|diff| {
                ReviewSource::Diff {
                    label: "git diff".to_string(),
                    diff,
                }
            }),
            "pr" | "pull" | "pull_request" => {
                let pr = parse_pr_url(target)
                    .ok_or_else(|| ToolError::invalid_input("Invalid pull request URL"))?;
                let diff = gh_pr_diff(&pr, &context.workspace)?;
                Ok(ReviewSource::PullRequest {
                    label: pr.label(),
                    diff,
                })
            }
            other => Err(ToolError::invalid_input(format!(
                "Unknown review kind '{other}'"
            ))),
        };
    }

    if let Some(pr) = parse_pr_url(target) {
        let diff = gh_pr_diff(&pr, &context.workspace)?;
        return Ok(ReviewSource::PullRequest {
            label: pr.label(),
            diff,
        });
    }

    if let Some(staged_override) = diff_mode_from_target(target) {
        let staged = staged || staged_override;
        let diff = resolve_diff_target(context.workspace.as_path(), staged, base)?;
        return Ok(ReviewSource::Diff {
            label: if staged {
                "git diff --cached"
            } else {
                "git diff"
            }
            .to_string(),
            diff,
        });
    }

    resolve_file_target(target, context)
}

fn resolve_file_target(target: &str, context: &ToolContext) -> Result<ReviewSource, ToolError> {
    let path = context.resolve_path(target)?;
    if !path.is_file() {
        return Err(ToolError::invalid_input(format!(
            "Target is not a file: {}",
            path.display()
        )));
    }
    let content = fs::read_to_string(&path).map_err(|e| {
        ToolError::execution_failed(format!("Failed to read file {}: {e}", path.display()))
    })?;
    let display = path
        .strip_prefix(&context.workspace)
        .unwrap_or(&path)
        .to_string_lossy()
        .to_string();
    Ok(ReviewSource::File { display, content })
}

fn resolve_diff_target(
    workspace: &Path,
    staged: bool,
    base: Option<&str>,
) -> Result<String, ToolError> {
    let Some(mut cmd) = crate::dependencies::Git::command() else {
        return Err(ToolError::execution_failed("git not found"));
    };
    cmd.arg("diff");
    if staged {
        cmd.arg("--cached");
    }
    if let Some(base) = base
        && !base.trim().is_empty()
    {
        cmd.arg(format!("{base}...HEAD"));
    }
    cmd.current_dir(workspace);

    let output = cmd
        .output()
        .map_err(|e| ToolError::execution_failed(format!("Failed to run git diff: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ToolError::execution_failed(format!(
            "git diff failed: {}",
            stderr.trim()
        )));
    }
    let diff = String::from_utf8_lossy(&output.stdout).to_string();
    if diff.trim().is_empty() {
        return Err(ToolError::invalid_input("No diff to review"));
    }
    Ok(diff)
}

fn gh_pr_diff(pr: &PullRequestRef, workspace: &Path) -> Result<String, ToolError> {
    let Some(mut cmd) = crate::dependencies::Gh::command() else {
        return Err(ToolError::execution_failed("gh not found"));
    };
    cmd.arg("pr")
        .arg("diff")
        .arg(&pr.number)
        .arg("--repo")
        .arg(format!("{}/{}", pr.owner, pr.repo))
        .current_dir(workspace);

    let output = cmd.output().map_err(|e| {
        ToolError::execution_failed(format!("Failed to run gh pr diff (is gh installed?): {e}"))
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ToolError::execution_failed(format!(
            "gh pr diff failed: {}",
            stderr.trim()
        )));
    }
    let diff = String::from_utf8_lossy(&output.stdout).to_string();
    if diff.trim().is_empty() {
        return Err(ToolError::invalid_input("Pull request diff is empty."));
    }
    Ok(diff)
}

fn build_review_prompt(source: &ReviewSource, max_chars: usize) -> String {
    match source {
        ReviewSource::File {
            display, content, ..
        } => {
            let numbered = format_with_line_numbers(content);
            let truncated = truncate_with_ellipsis(&numbered, max_chars, "\n...[truncated]\n");
            format!(
                "Review the following file and provide feedback.\n\
Path: {display}\n\n{truncated}\n\nEnd of file."
            )
        }
        ReviewSource::Diff { label, diff } => {
            let truncated = truncate_with_ellipsis(diff, max_chars, "\n...[truncated]\n");
            format!(
                "Review the following {label} and provide feedback.\n\n{truncated}\n\nEnd of diff."
            )
        }
        ReviewSource::PullRequest { label, diff } => {
            let truncated = truncate_with_ellipsis(diff, max_chars, "\n...[truncated]\n");
            format!(
                "Review the following pull request diff ({label}) and provide feedback.\n\n{truncated}\n\nEnd of diff."
            )
        }
    }
}

fn format_with_line_numbers(content: &str) -> String {
    content
        .lines()
        .enumerate()
        .map(|(idx, line)| format!("{:>4} | {}", idx + 1, line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_text(blocks: &[ContentBlock]) -> String {
    let mut output = String::new();
    for block in blocks {
        if let ContentBlock::Text { text, .. } = block {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(text);
        }
    }
    output.trim().to_string()
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn normalize_severity(value: &str) -> String {
    let lower = value.trim().to_ascii_lowercase();
    if lower.starts_with("err") || lower == "critical" || lower == "high" {
        "error".to_string()
    } else if lower.starts_with("warn") || lower == "medium" {
        "warning".to_string()
    } else {
        "info".to_string()
    }
}

fn extract_json_block(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        None
    } else {
        Some(&raw[start..=end])
    }
}

fn diff_mode_from_target(target: &str) -> Option<bool> {
    match target.trim().to_ascii_lowercase().as_str() {
        "diff" | "git diff" | "changes" | "working tree" | "working-tree" => Some(false),
        "staged" | "cached" | "git diff --cached" | "git diff --staged" => Some(true),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct PullRequestRef {
    owner: String,
    repo: String,
    number: String,
}

impl PullRequestRef {
    fn label(&self) -> String {
        format!("{}/{}#{}", self.owner, self.repo, self.number)
    }
}

fn parse_pr_url(url: &str) -> Option<PullRequestRef> {
    let trimmed = url.trim().trim_end_matches('/');
    if !trimmed.starts_with("http") {
        return None;
    }
    let parts: Vec<&str> = trimmed.split('/').collect();
    let pull_idx = parts.iter().position(|part| *part == "pull")?;
    if pull_idx < 2 || pull_idx + 1 >= parts.len() {
        return None;
    }
    let owner = parts.get(pull_idx.saturating_sub(2))?;
    let repo = parts.get(pull_idx.saturating_sub(1))?;
    let number = parts.get(pull_idx + 1)?;
    if owner.is_empty() || repo.is_empty() || number.is_empty() {
        return None;
    }
    Some(PullRequestRef {
        owner: (*owner).to_string(),
        repo: (*repo).to_string(),
        number: (*number).to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pr_url() {
        let pr =
            parse_pr_url("https://github.com/deepseek-ai/deepseek-cli/pull/123").expect("parse pr");
        assert_eq!(pr.owner, "deepseek-ai");
        assert_eq!(pr.repo, "deepseek-cli");
        assert_eq!(pr.number, "123");
    }

    #[test]
    fn ignores_non_pr_url() {
        assert!(parse_pr_url("https://github.com/deepseek-ai/deepseek-cli").is_none());
        assert!(parse_pr_url("not-a-url").is_none());
    }

    #[test]
    fn extracts_json_block() {
        let raw = "prefix {\"summary\":\"ok\"} suffix";
        let block = extract_json_block(raw).expect("block");
        assert!(block.contains("\"summary\""));
    }

    #[test]
    fn review_output_parses_structured_json() {
        let raw = r#"{
            "summary": " Looks good overall ",
            "issues": [{
                "severity": "high",
                "title": " Missing test ",
                "description": " Add coverage ",
                "path": " src/lib.rs ",
                "line": 42
            }],
            "suggestions": [{
                "path": "",
                "line": 7,
                "suggestion": " Keep the helper small "
            }],
            "overall_assessment": " Safe after test "
        }"#;

        let output = ReviewOutput::from_str(raw);

        assert_eq!(output.summary, "Looks good overall");
        assert_eq!(output.issues.len(), 1);
        assert_eq!(output.issues[0].severity, "error");
        assert_eq!(output.issues[0].title, "Missing test");
        assert_eq!(output.issues[0].path.as_deref(), Some("src/lib.rs"));
        assert_eq!(output.issues[0].line, Some(42));
        assert_eq!(output.suggestions.len(), 1);
        assert_eq!(output.suggestions[0].path, None);
        assert_eq!(output.suggestions[0].line, Some(7));
        assert_eq!(output.suggestions[0].suggestion, "Keep the helper small");
        assert_eq!(output.overall_assessment, "Safe after test");
    }

    #[test]
    fn review_output_parses_double_encoded_json_string() {
        let inner = serde_json::json!({
            "summary": "structured",
            "issues": [{
                "severity": "warning",
                "title": "Risk",
                "description": "The parser should not fall back to a raw JSON string.",
                "path": "src/main.rs",
                "line": 3
            }],
            "suggestions": [],
            "overall_assessment": "usable"
        })
        .to_string();
        let double_encoded = serde_json::to_string(&inner).expect("encode string");

        let output = ReviewOutput::from_str(&double_encoded);

        assert_eq!(output.summary, "structured");
        assert_eq!(output.issues.len(), 1);
        assert_eq!(output.issues[0].severity, "warning");
        assert_eq!(output.issues[0].path.as_deref(), Some("src/main.rs"));
        assert_eq!(output.overall_assessment, "usable");
    }

    #[test]
    fn review_output_fallback_keeps_summary() {
        let output = ReviewOutput::from_str("Not JSON");
        assert!(!output.summary.is_empty());
        assert!(output.issues.is_empty());
    }

    #[test]
    fn review_usage_metadata_reports_child_tokens_for_cost_accrual() {
        let metadata = review_usage_metadata(
            "deepseek-v4-flash",
            &Usage {
                input_tokens: 123,
                output_tokens: 45,
                prompt_cache_hit_tokens: Some(100),
                prompt_cache_miss_tokens: Some(23),
                reasoning_tokens: Some(7),
                ..Default::default()
            },
        );

        assert_eq!(metadata["tool"], "review");
        assert_eq!(metadata["child_model"], "deepseek-v4-flash");
        assert_eq!(metadata["child_input_tokens"], 123);
        assert_eq!(metadata["child_output_tokens"], 45);
        assert_eq!(metadata["child_prompt_cache_hit_tokens"], 100);
        assert_eq!(metadata["child_prompt_cache_miss_tokens"], 23);
        assert_eq!(metadata["child_reasoning_tokens"], 7);
    }
}
