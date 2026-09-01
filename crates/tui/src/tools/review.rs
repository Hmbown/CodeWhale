//! Tool for structured code reviews of files, diffs, or pull requests.

use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
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
use crate::models::Role;

const DEFAULT_MAX_CHARS: usize = 200_000;
const MAX_MAX_CHARS: usize = 1_000_000;
const FALLBACK_MAX_CHARS: usize = 4000;
const REVIEW_RECEIPT_SCHEMA_VERSION: u32 = 1;

/// Budget for how many lines a committable suggestion may replace. A
/// mechanical fix is small; anything larger is judgement wearing a
/// suggestion fence, so it must degrade to prose.
pub const MAX_COMMITTABLE_SUGGESTION_LINES: u32 = 25;
const REVIEW_CLIENT_UNAVAILABLE: &str = "Review tool requires an active Codewhale model client";

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
      \"start_line\": 121,\n\
      \"end_line\": 123,\n\
      \"suggestion\": \"why this change is needed\",\n\
      \"replacement\": \"the exact literal lines that replace start_line..end_line\"\n\
    }\n\
  ],\n\
  \"overall_assessment\": \"final assessment\"\n\
}\n\
If a field is unknown, use an empty string or null. Prioritize correctness and missing tests.\n\
\n\
Rules for \"suggestions\":\n\
- \"suggestion\" is prose explaining the change.\n\
- \"replacement\" is NOT a description. It is the literal replacement source code, verbatim, with the exact indentation it must have in the file, and with no diff markers, no line numbers, and no fences. It replaces lines start_line..end_line (inclusive) of the NEW version of the file; when the change is a single line, set start_line == end_line == line.\n\
- Supply \"replacement\" ONLY for a mechanical, high-confidence fix you are certain compiles and is correct as written (a typo, a wrong comparison operator, a missing await/unwrap guard, a renamed symbol, a wrong constant). Anything requiring judgement, new imports, or edits elsewhere in the file must omit \"replacement\" and stay prose-only.\n\
- Anchor a suggestion only to lines that appear in the diff you were given, and never to a deleted line. If you are not sure of the exact line numbers, omit \"replacement\".\n\
- A wrong replacement is worse than no replacement: it is one click from being merged. When in doubt, omit it.";

/// The system prompt shared by every structured review path (`review`
/// tool and `codewhale review --pr`). Callers parse the reply with
/// [`ReviewOutput::from_str`], which falls back to freeform text when a
/// model ignores the JSON contract.
#[must_use]
pub fn review_system_prompt() -> &'static str {
    REVIEW_SYSTEM_PROMPT
}

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
    /// First line of the replaced span (inclusive). `None` means the
    /// suggestion covers a single line, `line`.
    #[serde(default)]
    pub start_line: Option<u32>,
    /// Last line of the replaced span (inclusive). Defaults to `line`.
    #[serde(default)]
    pub end_line: Option<u32>,
    /// Prose: why the change is wanted.
    #[serde(default)]
    pub suggestion: String,
    /// Literal replacement source for `start_line..=end_line`, indentation
    /// included. `Some` only for mechanical, high-confidence fixes; when it
    /// is `None` the reviewer posts prose instead of a committable
    /// GitHub suggestion block.
    #[serde(default)]
    pub replacement: Option<String>,
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
            // Leading whitespace in `replacement` is load-bearing indentation,
            // so only trailing newlines and all-whitespace payloads are
            // normalized away.
            suggestion.replacement = suggestion
                .replacement
                .take()
                .map(|replacement| replacement.trim_end_matches(['\n', '\r']).to_string())
                .filter(|replacement| !replacement.trim().is_empty());
        }
        self
    }
}

/// Resolve a model-supplied review path to the post-image form diff hunks
/// are keyed by: trimmed, with any `./` prefix removed. `None` means the
/// finding has no position at all.
#[must_use]
pub fn normalize_review_path(path: Option<&str>) -> Option<String> {
    let path = path?.trim().trim_start_matches("./");
    if path.is_empty() {
        return None;
    }
    Some(path.to_string())
}

/// Where a suggestion can anchor in a diff, and whether its replacement may
/// be emitted as a one-click committable block.
///
/// This is the single source of truth for "is this committable": the PR
/// inline-comment path and the review receipt both derive from it, so a
/// receipt can never claim a suggestion was committable while the posted
/// comment degraded it to prose (or the reverse).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestionAnchor {
    /// No path or no line at all: only the summary body can carry it.
    NoPosition,
    /// Path and line exist but no hunk contains the line — a model-estimated
    /// position that missed the diff.
    Unanchorable { path: String },
    /// Anchored to RIGHT-side `path:start..=end`. `committable` is true only
    /// when the literal replacement passed every safety gate: non-empty,
    /// within the span-size budget, an explicitly bounded (or single-line)
    /// span, and fully covered by RIGHT-side hunk lines.
    Anchored {
        path: String,
        start: u32,
        end: u32,
        committable: bool,
    },
}

/// Resolve one suggestion against a diff's hunks.
#[must_use]
pub fn resolve_suggestion_anchor(
    suggestion: &ReviewSuggestion,
    hunks: &super::review_hunks::DiffHunks,
) -> SuggestionAnchor {
    let Some(path) = normalize_review_path(suggestion.path.as_deref()) else {
        return SuggestionAnchor::NoPosition;
    };
    let Some(end) = suggestion.end_line.or(suggestion.line) else {
        return SuggestionAnchor::NoPosition;
    };
    if !hunks.contains_line(&path, end) {
        return SuggestionAnchor::Unanchorable { path };
    }
    let start = suggestion.start_line.unwrap_or(end);
    // A model that gives neither start_line nor end_line has told us nothing
    // about how much code it means to replace. GitHub would happily *insert*
    // a multi-line replacement at a single-line anchor, duplicating the lines
    // the model meant to replace, so that shape is not committable.
    let explicit_span = suggestion.start_line.is_some() || suggestion.end_line.is_some();
    let committable = suggestion
        .replacement
        .as_deref()
        .is_some_and(|replacement| {
            !replacement.trim().is_empty()
                && start <= end
                && end.saturating_sub(start).saturating_add(1) <= MAX_COMMITTABLE_SUGGESTION_LINES
                && (explicit_span || start < end || !replacement.contains('\n'))
                && hunks.contains_span(&path, start, end)
        });
    SuggestionAnchor::Anchored {
        path,
        start,
        end,
        committable,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewReceipt {
    pub schema_version: u32,
    pub mode: String,
    pub generated_at: String,
    pub target: String,
    pub diff_fingerprint: String,
    pub diff_bytes: usize,
    pub diff_lines: usize,
    pub provider: String,
    pub model: String,
    pub checks_run: Vec<ReviewReceiptCheck>,
    pub findings: ReviewReceiptFindings,
    pub unresolved_risk: ReviewReceiptRisk,
    pub review_content_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewReceiptCheck {
    pub name: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewReceiptFindings {
    pub summary: String,
    pub issue_count: usize,
    pub suggestion_count: usize,
    pub highest_severity: String,
    pub issues: Vec<ReviewReceiptIssue>,
    /// What the suggestion pipeline would do with each suggestion against
    /// this diff. `#[serde(default)]` keeps receipts written before the
    /// field existed readable, so the schema version does not move.
    #[serde(default)]
    pub suggestions: ReviewReceiptSuggestions,
}

/// One suggestion emitted as a one-click committable block: where it
/// anchored, nothing else. The replacement text is deliberately absent —
/// a receipt is an audit record, never a second channel for model-written
/// code.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewReceiptSuggestion {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// Receipt provenance for the suggestion pipeline. The three counters use
/// the same [`SuggestionAnchor`] resolution as the posted inline comments,
/// so the numbers a receipt records are exactly what the PR path would
/// emit for the same diff.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewReceiptSuggestions {
    /// Suggestions emitted as committable blocks (one entry each, below).
    pub committable_count: usize,
    /// Anchor spans of the committable suggestions: path + line range, no
    /// replacement text.
    pub committable: Vec<ReviewReceiptSuggestion>,
    /// Suggestions whose anchor was valid but whose replacement failed a
    /// safety gate, so they posted as prose instead.
    pub degraded_to_prose: usize,
    /// Suggestions whose line missed every hunk (or whose file is not in
    /// the diff), so nothing was posted inline. Suggestions with no
    /// position at all are not counted — they never had an anchor to lose.
    pub dropped_unanchorable: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewReceiptIssue {
    pub severity: String,
    pub title: String,
    pub path: Option<String>,
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewReceiptRisk {
    pub unresolved: bool,
    pub level: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewReceiptValidation {
    pub passed: bool,
    pub reason: String,
    pub diff_fingerprint: String,
    pub receipt_fingerprint: Option<String>,
    pub receipt_path: Option<PathBuf>,
    pub unresolved_risk: Option<ReviewReceiptRisk>,
}

/// Classify every suggestion in `output` against `diff` exactly as the PR
/// inline-comment path would, producing the receipt's provenance counts.
#[must_use]
pub fn suggestion_provenance(output: &ReviewOutput, diff: &str) -> ReviewReceiptSuggestions {
    let hunks = super::review_hunks::DiffHunks::parse(diff);
    let mut suggestions = ReviewReceiptSuggestions::default();
    for suggestion in &output.suggestions {
        match resolve_suggestion_anchor(suggestion, &hunks) {
            SuggestionAnchor::Anchored {
                path,
                start,
                end,
                committable: true,
            } => suggestions.committable.push(ReviewReceiptSuggestion {
                path,
                start_line: start,
                end_line: end,
            }),
            SuggestionAnchor::Anchored {
                committable: false, ..
            } => suggestions.degraded_to_prose += 1,
            SuggestionAnchor::Unanchorable { .. } => suggestions.dropped_unanchorable += 1,
            SuggestionAnchor::NoPosition => {}
        }
    }
    suggestions.committable_count = suggestions.committable.len();
    suggestions
}

#[must_use]
pub fn build_review_receipt(
    target: impl Into<String>,
    diff: &str,
    provider: impl Into<String>,
    model: impl Into<String>,
    output: &ReviewOutput,
    review_content: &str,
    checks_run: Vec<ReviewReceiptCheck>,
) -> ReviewReceipt {
    let highest_severity = highest_review_severity(output);
    let unresolved = !output.issues.is_empty();
    let risk_level = if unresolved {
        highest_severity.clone()
    } else {
        "none".to_string()
    };
    let risk_summary = if unresolved {
        format!(
            "{} unresolved review issue(s); highest severity: {highest_severity}",
            output.issues.len()
        )
    } else {
        "No structured unresolved issues reported by review output.".to_string()
    };

    ReviewReceipt {
        schema_version: REVIEW_RECEIPT_SCHEMA_VERSION,
        mode: "pre_push_review".to_string(),
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        target: target.into(),
        diff_fingerprint: diff_fingerprint(diff),
        diff_bytes: diff.len(),
        diff_lines: diff.lines().count(),
        provider: provider.into(),
        model: model.into(),
        checks_run,
        findings: ReviewReceiptFindings {
            summary: output.summary.clone(),
            issue_count: output.issues.len(),
            suggestion_count: output.suggestions.len(),
            highest_severity: highest_severity.clone(),
            suggestions: suggestion_provenance(output, diff),
            issues: output
                .issues
                .iter()
                .map(|issue| ReviewReceiptIssue {
                    severity: issue.severity.clone(),
                    title: issue.title.clone(),
                    path: issue.path.clone(),
                    line: issue.line,
                })
                .collect(),
        },
        unresolved_risk: ReviewReceiptRisk {
            unresolved,
            level: risk_level,
            summary: risk_summary,
        },
        review_content_sha256: sha256_hex(review_content.as_bytes()),
    }
}

pub fn write_review_receipt(
    receipt: &ReviewReceipt,
    path_override: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    let path = if let Some(path) = path_override {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        path.to_path_buf()
    } else {
        let dir = codewhale_config::ensure_state_dir("review-receipts")?;
        let digest = receipt
            .diff_fingerprint
            .strip_prefix("sha256:")
            .unwrap_or(receipt.diff_fingerprint.as_str());
        let short = digest.chars().take(12).collect::<String>();
        let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
        dir.join(format!("{stamp}-{short}.json"))
    };
    let encoded = serde_json::to_string_pretty(receipt)?;
    fs::write(&path, encoded)?;
    Ok(path)
}

pub fn read_review_receipt(path: &Path) -> anyhow::Result<ReviewReceipt> {
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

pub fn latest_review_receipt_for_diff(
    diff: &str,
) -> anyhow::Result<Option<(PathBuf, ReviewReceipt)>> {
    let dir = codewhale_config::resolve_state_dir("review-receipts")?;
    if !dir.is_dir() {
        return Ok(None);
    }

    let expected = diff_fingerprint(diff);
    let mut matches = Vec::new();
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(receipt) = read_review_receipt(&path) else {
            continue;
        };
        if receipt.diff_fingerprint != expected {
            continue;
        }
        let modified = entry.metadata().and_then(|meta| meta.modified()).ok();
        matches.push((modified, path, receipt));
    }
    matches.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    Ok(matches.pop().map(|(_, path, receipt)| (path, receipt)))
}

#[must_use]
pub fn validate_review_receipt_for_diff(
    diff: &str,
    receipt: &ReviewReceipt,
    receipt_path: Option<PathBuf>,
) -> ReviewReceiptValidation {
    let expected = diff_fingerprint(diff);
    let mut validation = ReviewReceiptValidation {
        passed: false,
        reason: String::new(),
        diff_fingerprint: expected.clone(),
        receipt_fingerprint: Some(receipt.diff_fingerprint.clone()),
        receipt_path,
        unresolved_risk: Some(receipt.unresolved_risk.clone()),
    };

    if receipt.schema_version != REVIEW_RECEIPT_SCHEMA_VERSION {
        validation.reason = format!(
            "unsupported review receipt schema version {}",
            receipt.schema_version
        );
        return validation;
    }
    if receipt.diff_fingerprint != expected {
        validation.reason = "current diff fingerprint does not match receipt".to_string();
        return validation;
    }
    if receipt.unresolved_risk.unresolved {
        validation.reason = receipt.unresolved_risk.summary.clone();
        return validation;
    }
    if let Some(check) = receipt
        .checks_run
        .iter()
        .find(|check| !review_receipt_check_status_passes(&check.status))
    {
        validation.reason = format!(
            "review receipt check '{}' did not pass: {}",
            check.name, check.status
        );
        return validation;
    }

    validation.passed = true;
    validation.reason = "receipt matches current diff and has no unresolved risk".to_string();
    validation
}

#[must_use]
pub fn diff_fingerprint(diff: &str) -> String {
    format!("sha256:{}", sha256_hex(diff.as_bytes()))
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

fn highest_review_severity(output: &ReviewOutput) -> String {
    let mut highest = "none";
    for issue in &output.issues {
        let severity = issue.severity.as_str();
        if severity_rank(severity) > severity_rank(highest) {
            highest = severity;
        }
    }
    highest.to_string()
}

fn severity_rank(severity: &str) -> u8 {
    match severity {
        "error" => 4,
        "warning" => 3,
        "info" => 2,
        "none" => 1,
        _ => 0,
    }
}

fn review_receipt_check_status_passes(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "passed" | "pass" | "success" | "ok"
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    crate::hashing::sha256_hex(bytes)
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
            return Err(ToolError::not_available(REVIEW_CLIENT_UNAVAILABLE));
        };

        let target = required_str(&input, "target")?.trim();
        if target.is_empty() {
            return Err(ToolError::invalid_input("target cannot be empty"));
        }

        let kind = optional_str(&input, "kind")?.map(|s| s.trim().to_ascii_lowercase());
        let base = optional_str(&input, "base")?.map(|s| s.trim().to_string());
        let staged = optional_bool(&input, "staged", false)?;
        let max_chars =
            usize::try_from(optional_u64(&input, "max_chars", DEFAULT_MAX_CHARS as u64)?)
                .unwrap_or(DEFAULT_MAX_CHARS)
                .clamp(1, MAX_MAX_CHARS);

        let source =
            resolve_review_source(target, kind.as_deref(), staged, base.as_deref(), context)
                .await?;
        let prompt = build_review_prompt(&source, max_chars);

        let route = client.effective_route_envelope(&self.model, chrono::Utc::now());
        let request = MessageRequest {
            model: self.model.clone(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: prompt,
                    cache_control: None,
                }],
            }],
            // A review is a deliberative call: on thinking-default routes the
            // reasoning stream shares this budget, so it gets the route's
            // normal output allowance, not a review-only ceiling.
            max_tokens: client.effective_max_output_tokens(&route.model),
            system: Some(SystemPrompt::Text(REVIEW_SYSTEM_PROMPT.to_string())),
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: Some(false),
            // Route parity with ordinary turns: no sampling params, so every
            // provider's own defaults apply and fixed-sampling routes don't
            // reject the request.
            temperature: None,
            top_p: None,
        };

        let response = client
            .create_message(request)
            .await
            .map_err(|e| ToolError::execution_failed(format!("Review request failed: {e}")))?;

        if crate::models::is_incomplete_stop_reason(response.stop_reason.as_deref()) {
            return Ok(ToolResult::error(format!(
                "Review model response incomplete: provider stop reason `{}`; the partial review was not accepted.",
                crate::models::stop_reason_detail(response.stop_reason.as_deref())
            ))
            .with_metadata(review_usage_metadata(&route, &response.usage)));
        }

        let response_text = extract_text(&response.content);
        let output = ReviewOutput::from_str(&response_text);
        let metadata = review_usage_metadata(&route, &response.usage);
        let result =
            ToolResult::json(&output).map_err(|e| ToolError::execution_failed(e.to_string()))?;
        Ok(result.with_metadata(metadata))
    }
}

fn review_usage_metadata(
    route: &crate::cost_status::EffectiveRouteEnvelope,
    usage: &Usage,
) -> Value {
    let mut metadata = json!({
        "tool": "review",
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
    });
    // Every billable class, from the one shared producer, so a child turn can be
    // priced with the same completeness as a parent turn (#4318).
    crate::cost_status::attach_child_usage_metadata(&mut metadata, route, usage);
    metadata
}

enum ReviewSource {
    File { display: String, content: String },
    Diff { label: String, diff: String },
    PullRequest { label: String, diff: String },
}

async fn resolve_review_source(
    target: &str,
    kind: Option<&str>,
    staged: bool,
    base: Option<&str>,
    context: &ToolContext,
) -> Result<ReviewSource, ToolError> {
    if let Some(kind) = kind {
        return match kind {
            "file" => resolve_file_target(target, context),
            "diff" => {
                let diff = resolve_diff_target(context.workspace.as_path(), staged, base).await?;
                Ok(ReviewSource::Diff {
                    label: "git diff".to_string(),
                    diff,
                })
            }
            "pr" | "pull" | "pull_request" => {
                let pr = parse_pr_url(target)
                    .ok_or_else(|| ToolError::invalid_input("Invalid pull request URL"))?;
                let diff = gh_pr_diff(&pr, &context.workspace).await?;
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
        let diff = gh_pr_diff(&pr, &context.workspace).await?;
        return Ok(ReviewSource::PullRequest {
            label: pr.label(),
            diff,
        });
    }

    if let Some(staged_override) = diff_mode_from_target(target) {
        let staged = staged || staged_override;
        let diff = resolve_diff_target(context.workspace.as_path(), staged, base).await?;
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

async fn resolve_diff_target(
    workspace: &Path,
    staged: bool,
    base: Option<&str>,
) -> Result<String, ToolError> {
    let base = base.map(str::trim).filter(|base| !base.is_empty());
    let base_commit = if let Some(base) = base {
        // Resolve the user-supplied ref before placing it in `git diff`. This
        // both rejects option-looking input and gives the staged path a real
        // commit from which it can compute the merge base.
        let revision = format!("{base}^{{commit}}");
        let output = run_review_git(
            workspace,
            vec![
                "rev-parse".to_string(),
                "--verify".to_string(),
                "--end-of-options".to_string(),
                revision,
            ],
            "resolve review base",
        )
        .await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ToolError::invalid_input(format!(
                "Invalid git base ref '{base}': {}",
                stderr.trim()
            )));
        }
        let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if commit.is_empty() || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ToolError::execution_failed(format!(
                "git resolved base ref '{base}' to an invalid commit id"
            )));
        }
        Some(commit)
    } else {
        None
    };

    let mut args = vec!["diff".to_string()];
    if staged {
        args.push("--cached".to_string());
        if let Some(base_commit) = base_commit {
            // `git diff --cached <base>...HEAD` is invalid because the index
            // is already one side of this diff. Preserve triple-dot semantics
            // by resolving the merge base first, then compare that tree with
            // the index (committed branch work plus the staged snapshot).
            let output = run_review_git(
                workspace,
                vec!["merge-base".to_string(), base_commit, "HEAD".to_string()],
                "resolve staged review merge base",
            )
            .await?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(ToolError::execution_failed(format!(
                    "git merge-base failed: {}",
                    stderr.trim()
                )));
            }
            let merge_base = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if merge_base.is_empty() || !merge_base.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(ToolError::execution_failed(
                    "git merge-base returned an invalid commit id",
                ));
            }
            args.push(merge_base);
        }
    } else if let Some(base_commit) = base_commit {
        args.push(format!("{base_commit}...HEAD"));
    }
    args.push("--".to_string());

    let output = run_review_git(workspace, args, "generate review diff").await?;
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

async fn run_review_git(
    workspace: &Path,
    args: Vec<String>,
    operation: &'static str,
) -> Result<std::process::Output, ToolError> {
    let workspace = workspace.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let Some(mut cmd) = crate::dependencies::Git::command() else {
            return Err(ToolError::execution_failed("git not found"));
        };
        cmd.args(args).current_dir(workspace).output().map_err(|e| {
            ToolError::execution_failed(format!("Failed to {operation} with git: {e}"))
        })
    })
    .await
    .map_err(|e| ToolError::execution_failed(format!("git {operation} task panicked: {e}")))?
}

async fn gh_pr_diff(pr: &PullRequestRef, workspace: &Path) -> Result<String, ToolError> {
    let Some(mut cmd) = crate::dependencies::Gh::command() else {
        return Err(ToolError::execution_failed("gh not found"));
    };
    cmd.arg("pr")
        .arg("diff")
        .arg(&pr.number)
        .arg("--repo")
        .arg(format!("{}/{}", pr.owner, pr.repo))
        .current_dir(workspace);

    let output = tokio::task::spawn_blocking(move || cmd.output())
        .await
        .map_err(|e| ToolError::execution_failed(format!("gh pr diff task panicked: {e}")))?
        .map_err(|e| {
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

    #[tokio::test]
    async fn missing_review_client_uses_codewhale_provider_neutral_language() {
        let tool = ReviewTool::new(None, "unused".to_string());
        let context = ToolContext::new(PathBuf::from("."));

        let error = tool
            .execute(json!({}), &context)
            .await
            .expect_err("review requires a configured model client")
            .to_string();

        assert_eq!(
            error,
            "Failed to locate tool: Review tool requires an active Codewhale model client"
        );
        assert!(!error.contains("DeepSeek"));
    }

    fn fixture_git(workspace: &Path, args: &[&str]) -> std::process::Output {
        let mut command = crate::dependencies::Git::command().expect("git test dependency");
        let output = command
            .args(args)
            .current_dir(workspace)
            .output()
            .expect("run git fixture command");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    #[tokio::test]
    async fn staged_diff_with_base_compares_merge_base_to_index() {
        let repo = tempfile::TempDir::new().expect("temp git repository");
        fixture_git(repo.path(), &["init"]);
        fixture_git(repo.path(), &["config", "user.name", "Codewhale Test"]);
        fixture_git(
            repo.path(),
            &["config", "user.email", "codewhale-test@example.invalid"],
        );

        let tracked = repo.path().join("tracked.txt");
        fs::write(&tracked, "base\n").expect("write base fixture");
        fixture_git(repo.path(), &["add", "tracked.txt"]);
        fixture_git(repo.path(), &["commit", "-m", "base"]);
        let base =
            String::from_utf8_lossy(&fixture_git(repo.path(), &["rev-parse", "HEAD"]).stdout)
                .trim()
                .to_string();

        fs::write(&tracked, "base\ncommitted\n").expect("write committed fixture");
        fixture_git(repo.path(), &["add", "tracked.txt"]);
        fixture_git(repo.path(), &["commit", "-m", "branch change"]);
        fs::write(&tracked, "base\ncommitted\nstaged\n").expect("write staged fixture");
        fixture_git(repo.path(), &["add", "tracked.txt"]);
        fs::write(&tracked, "base\ncommitted\nstaged\nunstaged\n").expect("write unstaged fixture");

        let diff = resolve_diff_target(repo.path(), true, Some(&base))
            .await
            .expect("staged review diff from base");
        assert!(diff.contains("+committed"), "{diff}");
        assert!(diff.contains("+staged"), "{diff}");
        assert!(!diff.contains("unstaged"), "{diff}");
    }

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
        let route = crate::cost_status::EffectiveRouteEnvelope::capture(
            None,
            crate::config::ApiProvider::Deepseek,
            "deepseek",
            "deepseek-v4-flash",
            Some("https://api.deepseek.com/v1"),
            chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).expect("epoch"),
        );
        let metadata = review_usage_metadata(
            &route,
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

    #[test]
    fn pre_push_diff_review_receipt_includes_fingerprint_and_risk() {
        let diff = "diff --git a/src/lib.rs b/src/lib.rs\n+let risky = true;\n";
        let output = ReviewOutput {
            summary: "Found one issue".to_string(),
            issues: vec![ReviewIssue {
                severity: "warning".to_string(),
                title: "Missing test".to_string(),
                description: "Add coverage".to_string(),
                path: Some("src/lib.rs".to_string()),
                line: Some(12),
            }],
            suggestions: vec![ReviewSuggestion {
                path: Some("src/lib.rs".to_string()),
                line: Some(12),
                start_line: None,
                end_line: None,
                suggestion: "Add a regression test".to_string(),
                replacement: None,
            }],
            overall_assessment: "Needs a test".to_string(),
        };

        let receipt = build_review_receipt(
            "working-tree",
            diff,
            "deepseek",
            "deepseek-v4-pro",
            &output,
            "review body",
            vec![ReviewReceiptCheck {
                name: "cargo test -p codewhale-tui".to_string(),
                status: "passed".to_string(),
            }],
        );

        assert_eq!(receipt.schema_version, REVIEW_RECEIPT_SCHEMA_VERSION);
        assert_eq!(receipt.mode, "pre_push_review");
        assert_eq!(receipt.target, "working-tree");
        assert_eq!(receipt.diff_fingerprint, diff_fingerprint(diff));
        assert_eq!(receipt.diff_lines, 2);
        assert_eq!(receipt.provider, "deepseek");
        assert_eq!(receipt.model, "deepseek-v4-pro");
        assert_eq!(receipt.checks_run.len(), 1);
        assert_eq!(receipt.findings.issue_count, 1);
        assert_eq!(receipt.findings.suggestion_count, 1);
        assert_eq!(receipt.findings.highest_severity, "warning");
        assert!(receipt.unresolved_risk.unresolved);
        assert_eq!(receipt.unresolved_risk.level, "warning");
        assert_eq!(
            receipt.review_content_sha256,
            sha256_hex("review body".as_bytes())
        );
    }

    #[test]
    fn review_receipt_records_committable_suggestion_provenance() {
        // Built from a slice, not one string literal: a `\` continuation
        // strips the leading space that marks a context line.
        let diff = [
            "diff --git a/src/lib.rs b/src/lib.rs",
            "--- a/src/lib.rs",
            "+++ b/src/lib.rs",
            "@@ -10,2 +10,3 @@ fn head() {",
            " let a = 1;",
            "+let b = a.unwrap();",
            " let c = 2;",
            "",
        ]
        .join("\n");
        let output = ReviewOutput {
            summary: "One fix, one judgement call, one miss".to_string(),
            issues: Vec::new(),
            suggestions: vec![
                // Valid anchor, explicit in-hunk span, literal replacement:
                // the only shape that may become a committable block.
                ReviewSuggestion {
                    path: Some("src/lib.rs".to_string()),
                    line: Some(12),
                    start_line: Some(11),
                    end_line: Some(12),
                    suggestion: "Use the checked variant".to_string(),
                    replacement: Some("let b = a.unwrap_or_default();\nlet c = 2;".to_string()),
                },
                // Valid anchor, no literal replacement: degrades to prose.
                ReviewSuggestion {
                    path: Some("src/lib.rs".to_string()),
                    line: Some(11),
                    start_line: None,
                    end_line: None,
                    suggestion: "Add a test".to_string(),
                    replacement: None,
                },
                // Line 25 sits between hunks: unanchorable.
                ReviewSuggestion {
                    path: Some("src/lib.rs".to_string()),
                    line: Some(25),
                    start_line: None,
                    end_line: None,
                    suggestion: "Wrong line".to_string(),
                    replacement: Some("x = 1;".to_string()),
                },
                // No position at all: summary-body only, counted nowhere here.
                ReviewSuggestion {
                    path: None,
                    line: None,
                    start_line: None,
                    end_line: None,
                    suggestion: "Consider renaming".to_string(),
                    replacement: None,
                },
            ],
            overall_assessment: "Fix the unwrap".to_string(),
        };

        let receipt = build_review_receipt(
            "working-tree",
            &diff,
            "deepseek",
            "deepseek-v4-pro",
            &output,
            "review body",
            Vec::new(),
        );

        let provenance = &receipt.findings.suggestions;
        assert_eq!(provenance.committable_count, 1);
        assert_eq!(
            provenance.committable,
            vec![ReviewReceiptSuggestion {
                path: "src/lib.rs".to_string(),
                start_line: 11,
                end_line: 12,
            }]
        );
        assert_eq!(provenance.degraded_to_prose, 1);
        assert_eq!(provenance.dropped_unanchorable, 1);
        assert_eq!(receipt.findings.suggestion_count, 4);

        // Provenance records anchors, never code: the replacement text must
        // not leak into the receipt.
        let serialized = serde_json::to_string(&receipt).expect("serialize receipt");
        assert!(!serialized.contains("unwrap_or_default"), "{serialized}");
        assert!(!serialized.contains("x = 1;"), "{serialized}");
    }

    #[test]
    fn review_receipt_without_suggestion_provenance_still_decodes() {
        // Receipts written before the provenance field existed are schema v1;
        // the field is additive and serde-defaulted, so they must keep
        // decoding and the schema version must not move.
        let output = ReviewOutput::from_str("Looks good");
        let receipt = build_review_receipt(
            "working-tree",
            "diff --git a/a b/a\n",
            "deepseek",
            "deepseek-v4-flash",
            &output,
            "Looks good",
            Vec::new(),
        );
        let mut value = serde_json::to_value(&receipt).expect("serialize");
        value
            .get_mut("findings")
            .expect("findings")
            .as_object_mut()
            .expect("findings object")
            .remove("suggestions");
        let legacy: ReviewReceipt = serde_json::from_value(value).expect("legacy receipt decodes");
        assert_eq!(legacy.schema_version, REVIEW_RECEIPT_SCHEMA_VERSION);
        assert_eq!(
            legacy.findings.suggestions,
            ReviewReceiptSuggestions::default()
        );
    }

    #[test]
    fn write_review_receipt_accepts_override_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("receipt.json");
        let output = ReviewOutput::from_str("Looks good");
        let receipt = build_review_receipt(
            "staged",
            "diff --git a/a b/a\n",
            "deepseek",
            "deepseek-v4-flash",
            &output,
            "Looks good",
            Vec::new(),
        );

        let written = write_review_receipt(&receipt, Some(&path)).expect("write receipt");

        assert_eq!(written, path);
        let raw = fs::read_to_string(&written).expect("read receipt");
        let decoded: ReviewReceipt = serde_json::from_str(&raw).expect("decode receipt");
        assert_eq!(decoded.diff_fingerprint, receipt.diff_fingerprint);
        assert_eq!(decoded.unresolved_risk.level, "none");
    }

    #[test]
    fn review_receipt_validation_passes_matching_clean_receipt() {
        let diff = "diff --git a/a b/a\n+ok\n";
        let output = ReviewOutput::from_str("Looks good");
        let receipt = build_review_receipt(
            "working-tree",
            diff,
            "deepseek",
            "deepseek-v4-flash",
            &output,
            "Looks good",
            vec![ReviewReceiptCheck {
                name: "cargo test".to_string(),
                status: "passed".to_string(),
            }],
        );

        let validation = validate_review_receipt_for_diff(diff, &receipt, None);

        assert!(validation.passed);
        assert_eq!(validation.diff_fingerprint, diff_fingerprint(diff));
        assert_eq!(
            validation.reason,
            "receipt matches current diff and has no unresolved risk"
        );
    }

    #[test]
    fn review_receipt_validation_rejects_changed_diff() {
        let output = ReviewOutput::from_str("Looks good");
        let receipt = build_review_receipt(
            "working-tree",
            "diff --git a/a b/a\n+old\n",
            "deepseek",
            "deepseek-v4-flash",
            &output,
            "Looks good",
            Vec::new(),
        );

        let validation =
            validate_review_receipt_for_diff("diff --git a/a b/a\n+new\n", &receipt, None);

        assert!(!validation.passed);
        assert_eq!(
            validation.reason,
            "current diff fingerprint does not match receipt"
        );
    }

    #[test]
    fn review_receipt_validation_rejects_unresolved_risk() {
        let diff = "diff --git a/a b/a\n+risk\n";
        let output = ReviewOutput {
            summary: "Risk found".to_string(),
            issues: vec![ReviewIssue {
                severity: "error".to_string(),
                title: "Unsafe change".to_string(),
                description: "Needs work".to_string(),
                path: Some("a".to_string()),
                line: Some(1),
            }],
            suggestions: Vec::new(),
            overall_assessment: String::new(),
        };
        let receipt = build_review_receipt(
            "working-tree",
            diff,
            "deepseek",
            "deepseek-v4-flash",
            &output,
            "Risk found",
            Vec::new(),
        );

        let validation = validate_review_receipt_for_diff(diff, &receipt, None);

        assert!(!validation.passed);
        assert_eq!(validation.unresolved_risk.as_ref().unwrap().level, "error");
        assert!(validation.reason.contains("unresolved review issue"));
    }

    #[test]
    fn review_receipt_validation_rejects_failed_check() {
        let diff = "diff --git a/a b/a\n+ok\n";
        let output = ReviewOutput::from_str("Looks good");
        let receipt = build_review_receipt(
            "working-tree",
            diff,
            "deepseek",
            "deepseek-v4-flash",
            &output,
            "Looks good",
            vec![ReviewReceiptCheck {
                name: "cargo test".to_string(),
                status: "failed".to_string(),
            }],
        );

        let validation = validate_review_receipt_for_diff(diff, &receipt, None);

        assert!(!validation.passed);
        assert!(
            validation
                .reason
                .contains("review receipt check 'cargo test' did not pass")
        );
    }

    #[test]
    fn review_receipt_validation_rejects_attached_not_run_check() {
        let diff = "diff --git a/a b/a\n+ok\n";
        let output = ReviewOutput::from_str("Looks good");
        let receipt = build_review_receipt(
            "working-tree",
            diff,
            "deepseek",
            "deepseek-v4-flash",
            &output,
            "Looks good",
            vec![ReviewReceiptCheck {
                name: "cargo test".to_string(),
                status: "not_run".to_string(),
            }],
        );

        let validation = validate_review_receipt_for_diff(diff, &receipt, None);

        assert!(!validation.passed);
        assert!(
            validation
                .reason
                .contains("review receipt check 'cargo test' did not pass: not_run")
        );
    }
}
