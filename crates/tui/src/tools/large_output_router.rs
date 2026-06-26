//! Large-output routing for tool results (issue #548).
//!
//! Any tool result whose estimated token count exceeds the configured threshold
//! is intercepted here before it reaches the parent context. A lightweight
//! V4-Flash synthesis sub-agent condenses the raw output; only the synthesis
//! is returned to the parent. The raw content is stored in the workshop
//! variable `last_tool_result` so the parent agent can call
//! `promote_to_context` later if it needs the full text.
//!
//! Per-tool thresholds can override the global default. Individual tool calls
//! may pass `raw=true` to bypass routing entirely.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::tools::spec::ToolResult;

// ── Constants ──────────────────────────────────────────────────────────────────

/// Default token threshold above which a tool result is routed through the
/// workshop. Matches the issue spec of 4 096 tokens.
pub const DEFAULT_LARGE_OUTPUT_THRESHOLD_TOKENS: usize = 4_096;

/// Approximate characters-per-token ratio used for the heuristic estimate.
/// We intentionally choose a conservative value (3 chars/token) so we err
/// on the side of routing rather than dumping raw data into the parent.
const CHARS_PER_TOKEN_ESTIMATE: usize = 3;

/// Workshop variable name where the raw tool output is stored.
pub const WORKSHOP_LAST_TOOL_RESULT_VAR: &str = "last_tool_result";

// ── Configuration ─────────────────────────────────────────────────────────────

/// `[workshop]` section in `config.toml`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct WorkshopConfig {
    /// Token threshold above which tool results are routed through the workshop
    /// synthesis sub-agent. Default: [`DEFAULT_LARGE_OUTPUT_THRESHOLD_TOKENS`].
    #[serde(default)]
    pub large_output_threshold_tokens: Option<usize>,

    /// Per-tool threshold overrides (tool name → token limit). A tool whose
    /// name appears here uses this limit instead of
    /// `large_output_threshold_tokens`.
    #[serde(default)]
    pub per_tool_thresholds: Option<HashMap<String, usize>>,
}

impl WorkshopConfig {
    /// Resolve the effective threshold for the given tool name.
    #[must_use]
    pub fn threshold_for(&self, tool_name: &str) -> usize {
        if let Some(per_tool) = self.per_tool_thresholds.as_ref()
            && let Some(&limit) = per_tool.get(tool_name)
        {
            return limit;
        }
        self.large_output_threshold_tokens
            .unwrap_or(DEFAULT_LARGE_OUTPUT_THRESHOLD_TOKENS)
    }
}

// ── Token estimation ──────────────────────────────────────────────────────────

/// Estimate the number of tokens in `text` using a character-count heuristic.
///
/// This avoids a real tokeniser dependency; the estimate is deliberately
/// conservative (under-counts tokens) so we route aggressively rather than
/// letting a 5K-token blob slip through.
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    // Round up: partial last token still costs a token.
    chars.div_ceil(CHARS_PER_TOKEN_ESTIMATE)
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Decision returned by [`LargeOutputRouter::route`].
#[derive(Debug, Clone, PartialEq)]
pub enum RouteDecision {
    /// The output is small enough; pass it through unmodified.
    PassThrough,
    /// The output exceeded the threshold and was (or should be) synthesised.
    Synthesise {
        /// Estimated token count of the raw output.
        estimated_tokens: usize,
        /// The threshold that was breached.
        threshold: usize,
    },
}

/// Intercepts tool results and routes large ones through the workshop.
///
/// This type is intentionally `Clone` and `Default` so it can be embedded
/// cheaply in [`ToolContext`](crate::tools::spec::ToolContext) without
/// requiring `Arc` wrappers.
#[derive(Debug, Clone, Default)]
pub struct LargeOutputRouter {
    config: WorkshopConfig,
}

impl LargeOutputRouter {
    /// Construct a router from the resolved workshop config.
    #[must_use]
    pub fn new(config: WorkshopConfig) -> Self {
        Self { config }
    }

    /// Decide whether `result` for `tool_name` should be synthesised.
    ///
    /// Pass `raw_bypass = true` when the tool call included `raw = true`.
    #[must_use]
    pub fn route(&self, tool_name: &str, result: &ToolResult, raw_bypass: bool) -> RouteDecision {
        if raw_bypass || !result.success {
            return RouteDecision::PassThrough;
        }
        let threshold = self.config.threshold_for(tool_name);
        let estimated_tokens = estimate_tokens(&result.content);
        if estimated_tokens > threshold {
            RouteDecision::Synthesise {
                estimated_tokens,
                threshold,
            }
        } else {
            RouteDecision::PassThrough
        }
    }

    /// Build the synthesis prompt sent to the V4-Flash workshop sub-agent.
    ///
    /// The prompt is intentionally terse — Flash is a fast model and we just
    /// want a faithful summary, not deep reasoning.
    ///
    /// This is the building block for the live LLM synthesis call wired in
    /// the follow-up (once the async Flash client is safe to call from the
    /// registry layer). The method is public so callers outside this crate
    /// can unit-test the prompt shape.
    #[must_use]
    #[allow(dead_code)] // used by future Flash synthesis call; keep for API stability (see #3490)
    pub fn synthesis_prompt(tool_name: &str, raw_output: &str, estimated_tokens: usize) -> String {
        format!(
            "You are a synthesis assistant. The tool `{tool_name}` produced {estimated_tokens} tokens \
             of output that is too large to include directly in the parent context.\n\n\
             Summarise the output below into a concise, faithful synthesis of ≤ 800 words. \
             Preserve key facts, numbers, file paths, error messages, and any actionable \
             information. Do NOT add commentary or interpretation beyond what is in the source.\n\n\
             <raw_tool_output>\n{raw_output}\n</raw_tool_output>"
        )
    }

    /// Wrap a synthesis result with a workshop provenance header and a hint
    /// about the stored raw output.
    #[must_use]
    pub fn wrap_synthesis(
        tool_name: &str,
        synthesis: &str,
        estimated_tokens: usize,
        threshold: usize,
    ) -> String {
        format!(
            "[workshop-synthesis: tool={tool_name}, raw_tokens≈{estimated_tokens}, \
             threshold={threshold}, raw_stored_in={WORKSHOP_LAST_TOOL_RESULT_VAR}]\n\n{synthesis}"
        )
    }
}

// ── Workshop variable store ───────────────────────────────────────────────────

/// In-process store for workshop variables that persist across tool calls
/// within a session. The only variable exposed today is `last_tool_result`
/// which holds the most recent raw large-tool output for `promote_to_context`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkshopVariables {
    /// Raw content of the most recent large tool output that was routed
    /// through the workshop. Empty string when no routing has occurred.
    #[serde(default)]
    pub last_tool_result: String,

    /// Name of the tool that produced `last_tool_result`.
    #[serde(default)]
    pub last_tool_name: String,
}

impl WorkshopVariables {
    /// Store the raw output from a large-tool routing event.
    pub fn store_raw(&mut self, tool_name: &str, raw: &str) {
        self.last_tool_result = raw.to_string();
        self.last_tool_name = tool_name.to_string();
    }

    /// Retrieve and clear the stored raw output (consume semantics so the
    /// variable is not accidentally promoted twice).
    ///
    /// Called by the `promote_to_context` tool (not yet wired in this PR).
    #[must_use]
    #[allow(dead_code)] // consumed by promote_to_context tool in follow-up (see #3490)
    pub fn take_raw(&mut self) -> Option<(String, String)> {
        if self.last_tool_result.is_empty() {
            return None;
        }
        let content = std::mem::take(&mut self.last_tool_result);
        let name = std::mem::take(&mut self.last_tool_name);
        Some((name, content))
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_result(content: &str) -> ToolResult {
        ToolResult::success(content.to_string())
    }

    #[test]
    fn pass_through_below_threshold() {
        let router = LargeOutputRouter::default();
        let small = "x".repeat(100);
        let result = make_result(&small);
        assert_eq!(
            router.route("read_file", &result, false),
            RouteDecision::PassThrough
        );
    }

    #[test]
    fn synthesise_above_threshold() {
        let router = LargeOutputRouter::default();
        // DEFAULT threshold = 4096 tokens; 3 chars/token → 4096*3 = 12288 chars
        let big = "a".repeat(13_000);
        let result = make_result(&big);
        assert!(matches!(
            router.route("read_file", &result, false),
            RouteDecision::Synthesise { .. }
        ));
    }

    #[test]
    fn raw_bypass_skips_routing() {
        let router = LargeOutputRouter::default();
        let big = "a".repeat(13_000);
        let result = make_result(&big);
        // raw=true → always pass through regardless of size
        assert_eq!(
            router.route("exec_shell", &result, true),
            RouteDecision::PassThrough
        );
    }

    #[test]
    fn error_results_always_pass_through() {
        let router = LargeOutputRouter::default();
        let big = "error: ".repeat(2_000);
        let result = ToolResult::error(big);
        assert_eq!(
            router.route("exec_shell", &result, false),
            RouteDecision::PassThrough
        );
    }

    #[test]
    fn per_tool_threshold_override() {
        let mut per_tool = HashMap::new();
        per_tool.insert("grep_files".to_string(), 100); // very low
        let config = WorkshopConfig {
            large_output_threshold_tokens: Some(4096),
            per_tool_thresholds: Some(per_tool),
        };
        let router = LargeOutputRouter::new(config);
        // 100 tokens * 3 = 300 chars → trigger with 400 chars
        let medium = "b".repeat(400);
        let result = make_result(&medium);
        assert!(matches!(
            router.route("grep_files", &result, false),
            RouteDecision::Synthesise { .. }
        ));
        // Other tools still use the global threshold
        assert_eq!(
            router.route("read_file", &result, false),
            RouteDecision::PassThrough
        );
    }

    #[test]
    fn estimate_tokens_conservative() {
        // 9 chars → ceil(9/3) = 3 tokens
        assert_eq!(estimate_tokens("123456789"), 3);
        // 10 chars → ceil(10/3) = 4 tokens
        assert_eq!(estimate_tokens("1234567890"), 4);
        // Empty string
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn workshop_variables_store_and_take() {
        let mut vars = WorkshopVariables::default();
        assert!(vars.take_raw().is_none());

        vars.store_raw("read_file", "raw content here");
        let taken = vars.take_raw().expect("should have content");
        assert_eq!(taken.0, "read_file");
        assert_eq!(taken.1, "raw content here");

        // Second take is empty — consume semantics
        assert!(vars.take_raw().is_none());
    }

    #[test]
    fn wrap_synthesis_includes_provenance_header() {
        let wrapped = LargeOutputRouter::wrap_synthesis("web_search", "key facts here", 5000, 4096);
        assert!(wrapped.contains("workshop-synthesis"));
        assert!(wrapped.contains("web_search"));
        assert!(wrapped.contains("5000"));
        assert!(wrapped.contains("key facts here"));
    }
}
