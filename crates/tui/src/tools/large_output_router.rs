//! Adaptive evidence routing for tool results (#4619).
//!
//! Results are classified as inline, hybrid, or handle-only before they enter
//! model context. Non-inline results are published exactly once under their
//! origin session and remain available through bounded retrieval. The earlier
//! workshop preview behavior remains only behind the explicit classic-output
//! rollback switch.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::tools::spec::ToolResult;

// ── Constants ──────────────────────────────────────────────────────────────────

/// Default token threshold separating hybrid from handle-only evidence.
///
/// 32K tokens (≈96 KiB of text at the 3 chars/token estimate) keeps ordinary
/// tool results — file reads, test runs, build logs up to a few thousand
/// lines — fully inline. Only genuinely large outputs spill to evidence
/// artifacts, where the model-facing preview names the artifact path and how
/// to recover the omitted range.
pub const DEFAULT_LARGE_OUTPUT_THRESHOLD_TOKENS: usize = 32_768;

/// Approximate characters-per-token ratio used for the heuristic estimate.
/// We intentionally choose a conservative value (3 chars/token) so we err
/// on the side of routing rather than dumping raw data into the parent.
const CHARS_PER_TOKEN_ESTIMATE: usize = 3;

/// Workshop variable name where the raw tool output is stored.
pub const WORKSHOP_LAST_TOOL_RESULT_VAR: &str = "last_tool_result";

static ACTIVE_WORKSHOP: OnceLock<Mutex<WorkshopConfig>> = OnceLock::new();

#[cfg(test)]
static ACTIVE_WORKSHOP_TEST_SERIAL: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(test)]
std::thread_local! {
    static ACTIVE_WORKSHOP_TEST_SERIAL_HELD: std::cell::Cell<bool> = const {
        std::cell::Cell::new(false)
    };
}

/// Holds every test-side workshop activation behind one process-wide gate.
/// The thread-local marker lets the owning current-thread test call
/// `install_active` without trying to acquire its own non-reentrant lock.
#[cfg(test)]
pub(crate) struct ActiveWorkshopTestGuard {
    _serial: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for ActiveWorkshopTestGuard {
    fn drop(&mut self) {
        ACTIVE_WORKSHOP_TEST_SERIAL_HELD.with(|held| held.set(false));
    }
}

#[cfg(test)]
pub(crate) fn active_workshop_test_guard() -> ActiveWorkshopTestGuard {
    assert!(
        !ACTIVE_WORKSHOP_TEST_SERIAL_HELD.with(std::cell::Cell::get),
        "active workshop test guard is not reentrant"
    );
    let serial = ACTIVE_WORKSHOP_TEST_SERIAL
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    ACTIVE_WORKSHOP_TEST_SERIAL_HELD.with(|held| held.set(true));
    ActiveWorkshopTestGuard { _serial: serial }
}

fn active_workshop_slot() -> &'static Mutex<WorkshopConfig> {
    ACTIVE_WORKSHOP.get_or_init(|| Mutex::new(WorkshopConfig::default()))
}

// ── Configuration ─────────────────────────────────────────────────────────────

/// Existing `[workshop]` threshold configuration, retained for compatibility.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct WorkshopConfig {
    /// Token threshold above which results become handle-only evidence.
    #[serde(default)]
    pub large_output_threshold_tokens: Option<usize>,

    /// Per-tool threshold overrides (tool name → token limit). A tool whose
    /// name appears here uses this limit instead of
    /// `large_output_threshold_tokens`.
    #[serde(default)]
    pub per_tool_thresholds: Option<HashMap<String, usize>>,

    /// Optional model-visible byte budget for a single `read` / `read_file`
    /// result. Absent keeps the compile-time default (#5367).
    #[serde(default)]
    pub read_result_max_bytes: Option<usize>,

    /// Optional model-visible byte budget for a generic tool result after
    /// spillover. Absent keeps the compile-time default (#5367).
    #[serde(default)]
    pub tool_result_max_bytes: Option<usize>,
}

impl WorkshopConfig {
    /// Install the process-wide workshop budgets used by read/tool compactors.
    ///
    /// The returned immutable receipt is the snapshot written while the
    /// singleton lock was held. Callers that need evidence of their own
    /// activation can inspect it without racing a later process-wide update.
    pub fn install_active(config: Option<&Self>) -> Self {
        #[cfg(test)]
        let _test_serial = if ACTIVE_WORKSHOP_TEST_SERIAL_HELD.with(std::cell::Cell::get) {
            None
        } else {
            Some(
                ACTIVE_WORKSHOP_TEST_SERIAL
                    .get_or_init(|| Mutex::new(()))
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            )
        };
        let snapshot = config.cloned().unwrap_or_default();
        let mut slot = active_workshop_slot()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = snapshot;
        slot.clone()
    }

    /// Optional model-visible read budget, when the user opted in (#5367).
    #[must_use]
    pub fn active_read_result_max_bytes() -> Option<usize> {
        active_workshop_slot()
            .lock()
            .ok()
            .and_then(|cfg| cfg.read_result_max_bytes.filter(|n| *n > 0))
    }

    /// Optional model-visible tool-result budget, when the user opted in (#5367).
    #[must_use]
    pub fn active_tool_result_max_bytes() -> Option<usize> {
        active_workshop_slot()
            .lock()
            .ok()
            .and_then(|cfg| cfg.tool_result_max_bytes.filter(|n| *n > 0))
    }

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

    /// Decide whether classic routing would synthesize `result`.
    ///
    /// This is used only by the rollback implementation.
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

    #[must_use]
    pub fn evidence_routing(
        &self,
        tool_name: &str,
        result: &ToolResult,
        _raw_bypass: bool,
    ) -> (EvidenceRouting, usize, usize) {
        let threshold = self.config.threshold_for(tool_name);
        let estimated_tokens = estimate_tokens(&result.content);
        // `raw=true` no longer bypasses the context bound. Exact bytes remain
        // available through the artifact handle, so bypass is unnecessary.
        let routing = EvidenceRouting::from_token_estimate(estimated_tokens, threshold);
        (routing, estimated_tokens, threshold)
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
    #[allow(dead_code)] // consumed by promote_to_context tool in follow-up
    pub fn take_raw(&mut self) -> Option<(String, String)> {
        if self.last_tool_result.is_empty() {
            return None;
        }
        let content = std::mem::take(&mut self.last_tool_result);
        let name = std::mem::take(&mut self.last_tool_name);
        Some((name, content))
    }
}

// ── Adaptive evidence routing (#4619) ─────────────────────────────────────────

/// Routing policy for tool results: how much of the output stays inline in the
/// conversation vs. being stored as an external artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceRouting {
    /// Full result stays inline in the conversation context.
    Inline,
    /// A bounded observation (head/tail/summary) stays inline; the exact bytes
    /// are stored as an artifact recoverable via handle.
    Hybrid,
    /// Only a handle/reference stays inline; the full result is artifact-only.
    HandleOnly,
}

impl EvidenceRouting {
    /// Determine routing from estimated token count and threshold.
    #[must_use]
    pub fn from_token_estimate(estimated_tokens: usize, threshold: usize) -> Self {
        if estimated_tokens <= threshold / 4 {
            Self::Inline
        } else if estimated_tokens <= threshold {
            Self::Hybrid
        } else {
            Self::HandleOnly
        }
    }
}

/// Immutable metadata for a stored evidence artifact (#4619).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceArtifact {
    pub handle: String,
    pub digest: String,
    pub size_bytes: u64,
    pub content_type: String,
    pub tool_name: String,
    pub call_id: String,
    pub origin_session: String,
    pub generation: u32,
    pub redacted: bool,
    pub encoding: String,
    pub retention_state: EvidenceRetentionState,
    pub created_at_unix_ms: u64,
    pub retain_until_unix_ms: u64,
    pub storage_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceRetentionState {
    Live,
    Expired,
}

pub const EVIDENCE_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;

#[must_use]
pub fn classic_output_routing_enabled() -> bool {
    std::env::var("CODEWHALE_CLASSIC_OUTPUT_ROUTING")
        .ok()
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes" | "on"))
}

#[must_use]
pub fn evidence_metadata_relative_path(handle: &str) -> PathBuf {
    PathBuf::from(crate::artifacts::ARTIFACTS_DIR_NAME).join(format!("{handle}.evidence.json"))
}

pub fn publish_evidence_metadata(
    session_id: &str,
    artifact: &EvidenceArtifact,
) -> io::Result<PathBuf> {
    let bytes = serde_json::to_vec_pretty(artifact)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    crate::artifacts::write_session_relative_immutable(
        session_id,
        &evidence_metadata_relative_path(&artifact.handle),
        &bytes,
    )
}

pub fn read_evidence_metadata(session_id: &str, handle: &str) -> io::Result<EvidenceArtifact> {
    let relative = evidence_metadata_relative_path(handle);
    let path = crate::artifacts::session_artifact_absolute_path(session_id, &relative)
        .ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "invalid evidence owner"))?;
    let raw = std::fs::read(path)?;
    serde_json::from_slice(&raw).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

#[must_use]
pub fn unix_millis_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[must_use]
pub fn evidence_is_expired(artifact: &EvidenceArtifact, now_ms: u64) -> bool {
    artifact.retention_state == EvidenceRetentionState::Expired
        || now_ms > artifact.retain_until_unix_ms
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
    fn default_threshold_is_32k_tokens() {
        assert_eq!(DEFAULT_LARGE_OUTPUT_THRESHOLD_TOKENS, 32_768);
    }

    #[test]
    fn synthesise_above_threshold() {
        let router = LargeOutputRouter::default();
        // DEFAULT threshold = 32768 tokens; 3 chars/token → 32768*3 = 98304 chars
        let big = "a".repeat(100_000);
        let result = make_result(&big);
        assert!(matches!(
            router.route("read_file", &result, false),
            RouteDecision::Synthesise { .. }
        ));
    }

    #[test]
    fn raw_bypass_skips_routing() {
        let router = LargeOutputRouter::default();
        let big = "a".repeat(100_000);
        let result = make_result(&big);
        // raw=true → always pass through regardless of size
        assert_eq!(
            router.route("exec_shell", &result, true),
            RouteDecision::PassThrough
        );
    }

    #[test]
    fn adaptive_evidence_cannot_bypass_context_bound_with_raw_flag() {
        let router = LargeOutputRouter::default();
        let big = make_result(&"a".repeat(100_000));
        let (routing, _, _) = router.evidence_routing("exec_shell", &big, true);
        assert_eq!(routing, EvidenceRouting::HandleOnly);
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
            read_result_max_bytes: None,
            tool_result_max_bytes: None,
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
    fn workshop_byte_budgets_raise_floor_only() {
        let _guard = active_workshop_test_guard();
        let installed = WorkshopConfig::install_active(Some(&WorkshopConfig {
            large_output_threshold_tokens: None,
            per_tool_thresholds: None,
            read_result_max_bytes: Some(102_400),
            tool_result_max_bytes: Some(80_000),
        }));
        assert_eq!(installed.read_result_max_bytes, Some(102_400));
        assert_eq!(installed.tool_result_max_bytes, Some(80_000));
        assert_eq!(
            WorkshopConfig::active_read_result_max_bytes(),
            Some(102_400)
        );
        assert_eq!(WorkshopConfig::active_tool_result_max_bytes(), Some(80_000));
        let cleared = WorkshopConfig::install_active(None);
        assert_eq!(cleared.read_result_max_bytes, None);
        assert_eq!(cleared.tool_result_max_bytes, None);
        assert_eq!(WorkshopConfig::active_read_result_max_bytes(), None);
        assert_eq!(WorkshopConfig::active_tool_result_max_bytes(), None);
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
