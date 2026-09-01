//! Context compaction for long conversations.

use anyhow::Result;
use std::collections::HashMap;
use std::fmt::Write;
use std::time::Duration;

use crate::config::DEFAULT_TEXT_MODEL;
use crate::core::model_client::ModelClient;
use crate::logging;
use crate::models::Role;
use crate::models::{
    CacheControl, ContentBlock, Message, MessageRequest, SystemBlock, SystemPrompt,
};

#[path = "compaction/last_round.rs"]
mod last_round;
#[cfg(test)]
#[path = "compaction/survival_contract.rs"]
mod survival_contract;
pub(crate) use last_round::last_round_start;
pub use last_round::{
    CompactionCoverage, CompactionKeep, CompactionPath, LastCompactionSnapshot,
    inspect_compaction_keep, last_round_kept_count, pinned_anchors_text,
};

/// Configuration for conversation compaction behavior.
///
/// v0.8.11 simplified this from the prior token-OR-message-count trigger
/// to a token-only trigger. The
/// `message_threshold` field was removed: its only purpose was to fire
/// compaction on long sessions of small messages, which is exactly the
/// case where rewriting the prefix cache is least valuable. Token
/// budget is the right signal; message count was a 128K-era heuristic.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionConfig {
    pub enabled: bool,
    pub token_threshold: usize,
    pub model: String,
    /// Exact route image-input fact for the summarizer's outbound history.
    pub image_input: crate::model_profile::SupportState,
    /// Route-effective context window. `None` preserves compatibility for
    /// callers that have not resolved a provider route yet.
    pub effective_context_window: Option<u32>,
    pub cache_summary: bool,
    /// Optional user-supplied focus for a manual `/compact <focus>`: injected
    /// into the summary request so the checkpoint weights what the user
    /// said matters. `None` for automatic compaction.
    pub focus: Option<String>,
    /// Runtime turn that owns provider calls made by this compaction pass.
    /// `None` for the foreground TUI. This is accounting provenance only and
    /// is never included in a provider request.
    pub runtime_cost_owner: Option<String>,
    /// Workspace root, used only to re-state the user's `/anchor` file after
    /// the summary. `None` skips anchors.
    pub workspace: Option<std::path::PathBuf>,
}

/// Host-prepared configuration carried from compaction eligibility through
/// the replacement-history commit.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedCompactionEnvelope {
    pub config: CompactionConfig,
}

impl PreparedCompactionEnvelope {
    #[must_use]
    pub fn new(config: CompactionConfig) -> Self {
        Self { config }
    }
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            // ON BY DEFAULT since v0.8.6 (#402 P0 survivability). v0.8.64
            // resolves the user-facing default through the active model's
            // known context window, while explicit `auto_compact = false`
            // remains the opt-out. This fallback covers code paths that build
            // a `CompactionConfig` directly; real per-model values are still
            // derived through the threshold helpers.
            enabled: true,
            // v0.8.11: 50K was a 128K-era leftover that biased every
            // unconfigured caller toward "compact almost immediately on large-context routes."
            // Bumped to 800K (80% of a 1M window) so the fallback
            // default matches the hard automatic compaction guardrail. This
            // is intentionally later than the model-visible 60% "suggest
            // /compact during sustained work" guidance so automatic
            // replacement compaction stays a late continuity guardrail.
            // Real call sites override this via
            // `compaction_threshold_for_model_and_effort`.
            token_threshold: 800_000,
            model: DEFAULT_TEXT_MODEL.to_string(),
            image_input: crate::model_profile::SupportState::Unknown,
            effective_context_window: None,
            cache_summary: true,
            focus: None,
            runtime_cost_owner: None,
            workspace: None,
        }
    }
}

/// A provider can return HTTP success with an empty, non-text, or known
/// placeholder response. Committing that response would discard the useful
/// history while leaving only a placeholder checkpoint. Keep this deliberately
/// conservative: it is a corruption guard, not a prose-length or language
/// scorer.
const COMPACTION_LANGUAGE_CONTRACT: &str = "Use the natural language of the most recent \
substantive user message for reasoning and user-facing prose. Keep code, identifiers, paths, \
commands, logs, tool payloads, quotations, and the English structural labels verbatim. English \
scaffolding is not a request to switch languages.";

/// Failure kind for compaction LLM calls (deterministic vs transient).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionFailureKind {
    /// Same payload will fail again — do not sleep/retry unchanged.
    Deterministic,
    /// May resolve on retry (network, rate limit, timeout).
    Transient,
    /// Context overflow — drop the oldest history item and retry.
    ContextOverflow,
}

impl CompactionFailureKind {
    #[must_use]
    pub fn is_transient(self) -> bool {
        matches!(self, Self::Transient)
    }
}

pub const KEEP_RECENT_MESSAGES: usize = 4;
const MIN_SUMMARIZE_MESSAGES: usize = 6;
const SUMMARY_TOOL_RESULT_SNIPPET_CHARS: usize = 240;
const TOOL_PRUNE_STOP_CHECK_BYTES: usize = 16 * 1024;
const RETAINED_TOOL_RESULT_MAX_CHARS: usize = 64 * 1024;
const RETAINED_THINKING_MAX_CHARS: usize = 16 * 1024;
/// Token budget for the recent user messages retained verbatim in the
/// replacement history (Codex parity: COMPACT_USER_MESSAGE_MAX_TOKENS).
pub(crate) const COMPACT_RETAINED_USER_MESSAGE_MAX_TOKENS: usize = 20_000;
/// Handoff summarization prompt, appended to the live conversation as the
/// final user message (ported from Codex `templates/compact/prompt.md`).
const COMPACT_PROMPT: &str = "You are performing a context checkpoint compaction. Create a \
handoff summary for another LLM that will resume the task.\n\nInclude:\n\
- Current progress and key decisions made\n\
- Important context, constraints, or user preferences\n\
- What remains to be done (clear next steps)\n\
- Any critical data, examples, or references needed to continue (exact file paths, commands, and error text)\n\n\
Be concise, structured, and focused on helping the next LLM seamlessly continue the work. Do not call tools.\n\
Summarize the task, not the checkpoint machinery: do not mention compaction, checkpoints, or \
context management, and do not carry forward meta-commentary about them (e.g. \"context intact\") \
from earlier turns.";

/// Preamble for the one conversation-history checkpoint created by compaction.
/// This intentionally follows Codex's `templates/compact/summary_prefix.md`:
/// the checkpoint is a user-history item, never standing system-prompt prose.
const SUMMARY_HEADER: &str = "Another language model started to solve this problem and produced \
a summary of its thinking process. You also have access to the state of the tools that were used \
by that language model. Use this to build on the work that has already been done and avoid \
duplicating work. Here is the summary produced by the other language model, use the information \
in this summary to assist with your own analysis:";

/// Detection marker for committed compaction-summary text: the stable first
/// sentence of [`SUMMARY_HEADER`]. `engine/context.rs` restores summaries by
/// the same marker on session load.
pub const COMPACTION_SUMMARY_MARKER: &str = "Another language model started to solve this problem";
/// Marker written by pre-v0.9.6 compaction; sessions saved under the old
/// format must still be recognized so their summary is replaced, not stacked.
pub const LEGACY_COMPACTION_SUMMARY_MARKER: &str = "Conversation Summary (Auto-Generated)";
const COMPACTION_SUMMARY_BEGIN: &str = "<!-- compaction-summary:begin -->";
const COMPACTION_SUMMARY_END: &str = "<!-- compaction-summary:end -->";

/// Whether a system-prompt text block is a committed compaction summary.
#[must_use]
pub fn is_compaction_summary_text(text: &str) -> bool {
    text.contains(COMPACTION_SUMMARY_MARKER) || text.contains(LEGACY_COMPACTION_SUMMARY_MARKER)
}

fn summary_section(text: &str) -> Option<&str> {
    let begin = text.find(COMPACTION_SUMMARY_BEGIN)? + COMPACTION_SUMMARY_BEGIN.len();
    let remainder = &text[begin..];
    let end = remainder.find(COMPACTION_SUMMARY_END)?;
    let summary = remainder[..end].trim();
    (!summary.is_empty()).then_some(summary)
}

fn strip_summary_text(mut text: String) -> Option<String> {
    while let Some(begin) = text.find(COMPACTION_SUMMARY_BEGIN) {
        let after_begin = begin + COMPACTION_SUMMARY_BEGIN.len();
        let end = text[after_begin..]
            .find(COMPACTION_SUMMARY_END)
            .map_or(text.len(), |offset| {
                after_begin + offset + COMPACTION_SUMMARY_END.len()
            });
        text.replace_range(begin..end, "");
    }
    if let Some(marker) = text
        .find(COMPACTION_SUMMARY_MARKER)
        .or_else(|| text.find(LEGACY_COMPACTION_SUMMARY_MARKER))
    {
        text.truncate(marker);
    }
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Extract the persisted checkpoint payload from a legacy system-prompt
/// carrier. Runtime-thread storage used that carrier before checkpoints moved
/// into conversation history; the engine strips it before provider dispatch.
#[must_use]
pub fn extract_compaction_summary(prompt: Option<&SystemPrompt>) -> Option<SystemPrompt> {
    match prompt? {
        SystemPrompt::Text(text) => summary_section(text)
            .map(str::to_string)
            .or_else(|| {
                text.find(COMPACTION_SUMMARY_MARKER)
                    .or_else(|| text.find(LEGACY_COMPACTION_SUMMARY_MARKER))
                    .map(|start| text[start..].trim().to_string())
            })
            .map(SystemPrompt::Text),
        SystemPrompt::Blocks(blocks) => {
            let blocks = blocks
                .iter()
                .filter_map(|block| {
                    let text = summary_section(&block.text)
                        .map(str::to_string)
                        .or_else(|| {
                            block
                                .text
                                .find(COMPACTION_SUMMARY_MARKER)
                                .or_else(|| block.text.find(LEGACY_COMPACTION_SUMMARY_MARKER))
                                .map(|start| block.text[start..].trim().to_string())
                        })?;
                    let mut summary = block.clone();
                    summary.text = text;
                    Some(summary)
                })
                .collect::<Vec<_>>();
            (!blocks.is_empty()).then_some(SystemPrompt::Blocks(blocks))
        }
    }
}

/// Remove every committed compaction-summary block from a system prompt.
///
/// Compaction commits exactly one live summary: the newest one replaces its
/// predecessors. Before this existed, each compaction appended another
/// summary block to the successor system prompt, so the stable prefix grew by
/// up to a full summary per pass — which re-latched compaction pressure and
/// retriggered compaction on the next turn, forever.
#[must_use]
pub fn strip_compaction_summaries(prompt: Option<&SystemPrompt>) -> Option<SystemPrompt> {
    match prompt.cloned()? {
        SystemPrompt::Text(text) => strip_summary_text(text).map(SystemPrompt::Text),
        SystemPrompt::Blocks(blocks) => {
            let blocks = blocks
                .into_iter()
                .filter_map(|mut block| {
                    block.text = strip_summary_text(block.text)?;
                    Some(block)
                })
                .collect::<Vec<_>>();
            (!blocks.is_empty()).then_some(SystemPrompt::Blocks(blocks))
        }
    }
}

/// Flatten a committed summary prompt to the text stored in history.
#[must_use]
pub fn summary_prompt_text(prompt: &SystemPrompt) -> String {
    match prompt {
        SystemPrompt::Text(text) => text.clone(),
        SystemPrompt::Blocks(blocks) => blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n"),
    }
}

#[must_use]
pub(crate) fn compaction_checkpoint_message(prompt: &SystemPrompt) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: summary_prompt_text(prompt),
            cache_control: None,
        }],
    }
}

#[must_use]
pub(crate) fn is_compaction_checkpoint_message(message: &Message) -> bool {
    user_text_of(message).is_some_and(|text| is_compaction_summary_text(&text))
}

pub(crate) fn estimate_tokens_for_message(message: &Message, include_thinking: bool) -> usize {
    message
        .content
        .iter()
        .map(|c| match c {
            ContentBlock::Text { text, .. } => text.len() / 4,
            // Historical reasoning blocks are UI/session metadata for DeepSeek.
            // Only current-turn tool-call reasoning is sent back to the API.
            ContentBlock::Thinking { thinking, .. } if include_thinking => thinking.len() / 4,
            ContentBlock::Thinking { .. } => 0,
            ContentBlock::ToolUse { input, .. } => serde_json::to_string(input)
                .map(|s| s.len() / 4)
                .unwrap_or(100),
            ContentBlock::ToolResult {
                content,
                content_blocks,
                ..
            } => {
                let images = content_blocks.as_ref().map_or(0, |blocks| {
                    blocks
                        .iter()
                        .filter(|block| {
                            block.get("type").and_then(serde_json::Value::as_str) == Some("image")
                        })
                        .count()
                });
                content.len() / 4 + images * IMAGE_TOKEN_ESTIMATE
            }
            // An inline image is real input the model pays for; estimating it
            // at 0 undercounts the budget and risks overflow in image-heavy
            // sessions. Use a conservative flat per-image estimate (vision
            // tiles are typically ~1k tokens); erring high compacts slightly
            // early rather than overflowing.
            ContentBlock::ImageUrl { .. } => IMAGE_TOKEN_ESTIMATE,
            ContentBlock::ServerToolUse { .. }
            | ContentBlock::ToolSearchToolResult { .. }
            | ContentBlock::CodeExecutionToolResult { .. } => 0,
        })
        .sum::<usize>()
}

/// Conservative flat token estimate for an inline image (`ContentBlock::ImageUrl`).
/// Vision models bill images by resized tile count; ~1k tokens is a safe
/// mid-range estimate that keeps the compaction trigger from under-reading an
/// image-heavy session.
const IMAGE_TOKEN_ESTIMATE: usize = 1000;

pub fn estimate_tokens(messages: &[Message]) -> usize {
    // Rough estimate: ~4 chars per token. DeepSeek thinking-mode rule: any
    // assistant message with tool_calls keeps its reasoning_content forever
    // (replayed in all subsequent requests). Final text-only answers drop it.
    messages
        .iter()
        .map(|message| estimate_tokens_for_message(message, message_has_tool_use(message)))
        .sum()
}

pub(crate) fn message_has_tool_use(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
}

pub(crate) fn estimate_text_tokens_conservative(text: &str) -> usize {
    text.chars().count().div_ceil(3)
}

fn estimate_system_tokens_conservative(system: Option<&SystemPrompt>) -> usize {
    match system {
        Some(SystemPrompt::Text(text)) => estimate_text_tokens_conservative(text),
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .iter()
            .map(|block| estimate_text_tokens_conservative(&block.text))
            .sum(),
        None => 0,
    }
}

/// Conservative estimate for full request input tokens (messages + system + framing).
#[must_use]
pub fn estimate_input_tokens_conservative(
    messages: &[Message],
    system: Option<&SystemPrompt>,
) -> usize {
    let message_tokens = estimate_tokens(messages).saturating_mul(3).div_ceil(2);
    let system_tokens = estimate_system_tokens_conservative(system);
    let framing_overhead = messages.len().saturating_mul(12).saturating_add(48);
    message_tokens
        .saturating_add(system_tokens)
        .saturating_add(framing_overhead)
}

/// Best-effort estimate of real request input tokens, without the 1.5×
/// safety inflation used by overflow math.
///
/// Compaction *pressure* compares against a threshold whose percentage means
/// "fraction of the context window" on the user-facing meter. Feeding the
/// inflated overflow estimate into that comparison made an 80% setting fire
/// at roughly half the real usage. Overflow protection keeps its inflated
/// estimator; the pressure trigger uses this one, preferring provider-billed
/// prompt tokens when the caller has them.
#[must_use]
pub fn estimate_input_tokens_for_pressure(
    messages: &[Message],
    system: Option<&SystemPrompt>,
) -> usize {
    let message_tokens = estimate_tokens(messages);
    let system_tokens = estimate_system_tokens_conservative(system);
    let framing_overhead = messages.len().saturating_mul(12).saturating_add(48);
    message_tokens
        .saturating_add(system_tokens)
        .saturating_add(framing_overhead)
}

fn estimate_retained_floor_conservative(
    messages: &[Message],
    system_prompt: Option<&SystemPrompt>,
    prepared: &PreparedCompactionEnvelope,
) -> usize {
    let config = &prepared.config;
    let retained = retained_user_messages(messages, COMPACT_RETAINED_USER_MESSAGE_MAX_TOKENS);
    let retained_tokens = estimate_tokens(&retained).saturating_mul(3).div_ceil(2);
    let framing = retained.len().saturating_mul(12).saturating_add(48);
    let anchors = user_anchors_section(config.workspace.as_deref());
    let summary_scaffolding_tokens =
        estimate_text_tokens_conservative(&build_compaction_summary_block_text("", &anchors));

    // Post-compaction the committed summary is REPLACED, not stacked, so prior
    // summary blocks must not inflate the floor. Count only the exact installed
    // scaffolding here; the model owns the concise summary length, just as it
    // owns the answer length on an ordinary turn.
    let retained_system_prompt = strip_compaction_summaries(system_prompt);
    retained_tokens
        .saturating_add(estimate_system_tokens_conservative(
            retained_system_prompt.as_ref(),
        ))
        .saturating_add(framing)
        .saturating_add(summary_scaffolding_tokens)
}

/// Whether the current canonical request has reached the configured automatic
/// compaction pressure. This deliberately excludes eligibility/reclaimability:
/// local tool-result pruning uses it to decide when pressure has actually
/// cleared, even if the remaining transcript cannot support an LLM summary.
#[must_use]
pub fn compaction_pressure_reached(
    messages: &[Message],
    system_prompt: Option<&SystemPrompt>,
    config: &CompactionConfig,
) -> bool {
    compaction_pressure_reached_with_billed(messages, system_prompt, config, None)
}

/// Pressure check that additionally honors provider-billed prompt tokens.
///
/// Billed usage is the ground truth for how large the context actually is;
/// the estimator undercounts non-ASCII text and cannot see server-side
/// framing. Whichever signal is higher decides, so an undercounting estimate
/// cannot hide pressure the provider already billed for. Callers must only
/// pass a billed count that describes the message list being checked —
/// post-prune re-checks pass `None` and fall back to the estimate.
#[must_use]
pub fn compaction_pressure_reached_with_billed(
    messages: &[Message],
    system_prompt: Option<&SystemPrompt>,
    config: &CompactionConfig,
    billed_input_tokens: Option<u64>,
) -> bool {
    if !config.enabled {
        return false;
    }
    let billed = billed_input_tokens
        .and_then(|tokens| usize::try_from(tokens).ok())
        .unwrap_or(0);
    // Billing alone proving pressure short-circuits the walk (#perf-r5):
    // `estimated.max(billed) >= threshold` is unconditionally true when
    // `billed >= threshold`, so estimating cannot change the answer and the
    // O(transcript) pass is skipped. Over-pressure sessions pay this check
    // multiple times per step (pressure gate + decision re-check).
    if billed >= config.token_threshold {
        return true;
    }
    let estimated = estimate_input_tokens_for_pressure(messages, system_prompt);
    estimated.max(billed) >= config.token_threshold
}

/// Estimate-only eligibility check ([`should_compact_with_billed`] with no
/// billed tokens): used by the request preview, where no provider bill exists.
pub fn should_compact(
    messages: &[Message],
    system_prompt: Option<&SystemPrompt>,
    prepared: &PreparedCompactionEnvelope,
) -> bool {
    should_compact_with_billed(messages, system_prompt, prepared, None)
}

/// Why an over-pressure context still did not start an automatic pass.
///
/// A refusal is not a bug by itself — each guard exists for a reason — but a
/// silent refusal is: the user watches a full context meter while
/// auto-compaction appears broken (#5577). Callers surface these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionRefusal {
    /// Too few messages for a summary pass to mean anything.
    TooFewMessages { count: usize },
    /// The conservative retained floor (system prompt + kept messages +
    /// summary allowance) cannot get below the trigger, so a pass would
    /// recur on every step without relieving pressure.
    RetainedFloor { floor: usize, threshold: usize },
}

/// Outcome of the automatic-compaction eligibility check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionDecision {
    /// Disabled, or pressure not reached: nothing to do, nothing to explain.
    NotNeeded,
    /// Start a pass.
    Compact,
    /// Pressure is real but a guard declined; the reason names the guard.
    Refused(CompactionRefusal),
}

/// Eligibility check that honors provider-billed prompt tokens for the
/// pressure gate, mirroring [`compaction_pressure_reached_with_billed`].
pub fn should_compact_with_billed(
    messages: &[Message],
    system_prompt: Option<&SystemPrompt>,
    prepared: &PreparedCompactionEnvelope,
    billed_input_tokens: Option<u64>,
) -> bool {
    matches!(
        compaction_decision_with_billed(messages, system_prompt, prepared, billed_input_tokens),
        CompactionDecision::Compact
    )
}

/// Full eligibility decision, including *why* an over-pressure context was
/// refused, so hosts can tell the user instead of silently holding.
#[must_use]
pub fn compaction_decision_with_billed(
    messages: &[Message],
    system_prompt: Option<&SystemPrompt>,
    prepared: &PreparedCompactionEnvelope,
    billed_input_tokens: Option<u64>,
) -> CompactionDecision {
    let config = &prepared.config;
    if !config.enabled {
        return CompactionDecision::NotNeeded;
    }
    // Pressure gate + prune projection share one estimate (#perf-r5): both
    // consume `estimate_input_tokens_for_pressure` over the same
    // `(messages, system_prompt)`, a pure function, so it is computed at
    // most once. `billed >= threshold` proves pressure without estimating
    // (max is unconditionally >= threshold then); the estimate is deferred
    // until something actually needs it — the prune projection below — so
    // the billed-corner still reaches the TooFew and RetainedFloor guards
    // unchanged, and skips the walk entirely when no prune candidates exist.
    let billed = billed_input_tokens
        .and_then(|tokens| usize::try_from(tokens).ok())
        .unwrap_or(0);
    let estimated: Option<usize> = if billed < config.token_threshold {
        let estimate = estimate_input_tokens_for_pressure(messages, system_prompt);
        if estimate.max(billed) < config.token_threshold {
            return CompactionDecision::NotNeeded;
        }
        Some(estimate)
    } else {
        None
    };

    // The execution path mechanically prunes old verbose tool results before
    // asking the model for a summary. Local pruning alone may be enough to
    // clear pressure even when the transcript is too small for an LLM pass.
    // Project that outcome from the measured plan — per-block deltas use the
    // estimator's own arithmetic, so this equals re-estimating a pruned copy
    // without cloning a multi-megabyte transcript on every step.
    let prune_plan = plan_tool_result_prunes(messages, KEEP_RECENT_MESSAGES);
    if !prune_plan.is_empty() {
        let estimate = match estimated {
            Some(value) => value,
            None => estimate_input_tokens_for_pressure(messages, system_prompt),
        };
        let reclaimed_tokens: usize = prune_plan.iter().map(PlannedPrune::tokens_reclaimed).sum();
        let projected = estimate.saturating_sub(reclaimed_tokens);
        if projected < config.token_threshold {
            return CompactionDecision::Compact;
        }
    }

    if messages.len() < MIN_SUMMARIZE_MESSAGES {
        return CompactionDecision::Refused(CompactionRefusal::TooFewMessages {
            count: messages.len(),
        });
    }

    // Reclaimability guard: do not start a pass whose replacement request
    // (system prompt + retained user messages + summary allowance)
    // cannot get below the trigger, or a large stable prefix would cause
    // auto-compaction on every tool step.
    let floor = estimate_retained_floor_conservative(messages, system_prompt, prepared);
    if floor >= config.token_threshold {
        return CompactionDecision::Refused(CompactionRefusal::RetainedFloor {
            floor,
            threshold: config.token_threshold,
        });
    }
    CompactionDecision::Compact
}

fn truncate_chars(text: &str, max_chars: usize) -> &str {
    if max_chars == 0 {
        return "";
    }
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => &text[..idx],
        None => text,
    }
}

fn tail_chars(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        return text.to_string();
    }
    let start_char = total_chars.saturating_sub(max_chars);
    let start_idx = text
        .char_indices()
        .nth(start_char)
        .map_or(0, |(idx, _)| idx);
    text[start_idx..].to_string()
}

#[derive(Debug, Clone)]
struct ToolUseInfo {
    name: String,
    key: String,
    args_preview: String,
}

fn tool_use_key(name: &str, input: &serde_json::Value) -> String {
    format!(
        "{name}:{}",
        serde_json::to_string(input).unwrap_or_else(|_| input.to_string())
    )
}

fn tool_args_preview(input: &serde_json::Value) -> String {
    let redacted = codewhale_config::persistence::redact_json_secrets(input);
    let raw = serde_json::to_string(&redacted).unwrap_or_else(|_| redacted.to_string());
    truncate_chars(&raw, 120).to_string()
}

fn collect_tool_uses(messages: &[Message]) -> HashMap<String, ToolUseInfo> {
    let mut tool_uses = HashMap::new();
    for message in messages {
        for block in &message.content {
            if let ContentBlock::ToolUse {
                id, name, input, ..
            } = block
            {
                tool_uses.insert(
                    id.clone(),
                    ToolUseInfo {
                        name: name.clone(),
                        key: tool_use_key(name, input),
                        args_preview: tool_args_preview(input),
                    },
                );
            }
        }
    }
    tool_uses
}

struct ToolResultPruneCandidate {
    message_idx: usize,
    block_idx: usize,
    key: String,
    tool_name: String,
    args_preview: String,
    original_len: usize,
}

fn tool_result_content_blocks_len(content_blocks: Option<&[serde_json::Value]>) -> usize {
    content_blocks
        .and_then(|blocks| serde_json::to_vec(blocks).ok())
        .map_or(0, |bytes| bytes.len())
}

#[cfg(test)]
fn prune_tool_results(messages: &mut [Message], protected_window: usize) -> usize {
    prune_tool_results_until(messages, protected_window, |_, _| false)
}

/// Mechanically prune old verbose tool results before paying for an LLM summary.
///
/// The most recent `protected_window` messages stay byte-for-byte intact. Older
/// duplicate tool results keep the freshest full body and replace earlier
/// copies with one-line summaries; non-duplicate old results are summarized only
/// when they exceed the normal summary snippet size.
fn prune_tool_results_until<F>(
    messages: &mut [Message],
    protected_window: usize,
    mut should_stop: F,
) -> usize
where
    F: FnMut(&[Message], usize) -> bool,
{
    let plan = plan_tool_result_prunes(messages, protected_window);
    let mut bytes_saved = 0usize;
    for planned in plan {
        if let ContentBlock::ToolResult {
            content,
            content_blocks,
            ..
        } = &mut messages[planned.message_idx].content[planned.block_idx]
        {
            bytes_saved = bytes_saved.saturating_add(planned.bytes_reclaimed());
            *content = planned.summary;
            *content_blocks = None;

            if should_stop(messages, bytes_saved) {
                break;
            }
        }
    }
    bytes_saved
}

/// One tool-result replacement the pruner has decided on, measured up front
/// so eligibility checks can project the outcome without cloning a
/// multi-megabyte transcript ([`compaction_decision_with_billed`] used to
/// copy the entire message list every over-pressure step just to ask "would
/// pruning be enough?").
struct PlannedPrune {
    message_idx: usize,
    block_idx: usize,
    summary: String,
    content_len: usize,
    blocks_len: usize,
    image_count: usize,
}

impl PlannedPrune {
    /// Byte reduction this replacement realizes, matching the pruner's
    /// accounting exactly.
    fn bytes_reclaimed(&self) -> usize {
        self.content_len
            .saturating_sub(self.summary.len())
            .saturating_add(self.blocks_len)
    }

    /// Estimator-token reduction, using the same per-block arithmetic as
    /// [`estimate_tokens_for_message`] so a projection built from these
    /// deltas equals re-estimating the pruned transcript.
    fn tokens_reclaimed(&self) -> usize {
        let before = self.content_len / 4 + self.image_count * IMAGE_TOKEN_ESTIMATE;
        let after = self.summary.len() / 4;
        before.saturating_sub(after)
    }
}

/// Decide, without mutating anything, which old tool results pruning would
/// replace. The most recent `protected_window` messages stay untouched; older
/// duplicate results keep the freshest full body; non-duplicates are replaced
/// only when they exceed the summary snippet size.
fn plan_tool_result_prunes(messages: &[Message], protected_window: usize) -> Vec<PlannedPrune> {
    let cutoff = messages.len().saturating_sub(protected_window);
    if cutoff == 0 {
        return Vec::new();
    }

    let tool_uses = collect_tool_uses(messages);
    let mut candidates = Vec::new();
    let mut latest_by_key: HashMap<String, usize> = HashMap::new();
    let mut count_by_key: HashMap<String, usize> = HashMap::new();

    for (message_idx, message) in messages.iter().take(cutoff).enumerate() {
        for (block_idx, block) in message.content.iter().enumerate() {
            let ContentBlock::ToolResult {
                tool_use_id,
                content,
                content_blocks,
                ..
            } = block
            else {
                continue;
            };
            let Some(info) = tool_uses.get(tool_use_id) else {
                continue;
            };
            latest_by_key.insert(info.key.clone(), message_idx);
            *count_by_key.entry(info.key.clone()).or_insert(0) += 1;
            candidates.push(ToolResultPruneCandidate {
                message_idx,
                block_idx,
                key: info.key.clone(),
                tool_name: info.name.clone(),
                args_preview: info.args_preview.clone(),
                original_len: content
                    .len()
                    .saturating_add(tool_result_content_blocks_len(content_blocks.as_deref())),
            });
        }
    }

    // The maps above are fully populated before planning completes, so the order
    // below only changes which message bytes are rewritten first. Planning from
    // newest to oldest lets the pruner stop as soon as enough bytes were saved,
    // preserving the earlier JSON request prefix for byte-level KV caches.
    candidates.reverse();

    let mut plan = Vec::new();
    for candidate in candidates {
        let duplicate_count = count_by_key.get(&candidate.key).copied().unwrap_or(0);
        let is_latest_duplicate = duplicate_count > 1
            && latest_by_key.get(&candidate.key) == Some(&candidate.message_idx);
        if is_latest_duplicate {
            continue;
        }
        if duplicate_count <= 1 && candidate.original_len <= SUMMARY_TOOL_RESULT_SNIPPET_CHARS {
            continue;
        }

        let summary = format!(
            "[{}] tool result pruned ({} bytes; args: {})",
            candidate.tool_name, candidate.original_len, candidate.args_preview
        );
        if summary.len() >= candidate.original_len {
            continue;
        }

        let ContentBlock::ToolResult {
            content,
            content_blocks,
            ..
        } = &messages[candidate.message_idx].content[candidate.block_idx]
        else {
            continue;
        };
        plan.push(PlannedPrune {
            message_idx: candidate.message_idx,
            block_idx: candidate.block_idx,
            summary,
            content_len: content.len(),
            blocks_len: tool_result_content_blocks_len(content_blocks.as_deref()),
            image_count: content_blocks.as_ref().map_or(0, |blocks| {
                blocks
                    .iter()
                    .filter(|block| {
                        block.get("type").and_then(serde_json::Value::as_str) == Some("image")
                    })
                    .count()
            }),
        });
    }
    plan
}

fn truncate_retained_block(label: &str, content: &mut String, max_chars: usize) -> bool {
    let char_count = content.chars().count();
    if char_count <= max_chars {
        return false;
    }

    let snippet_budget = max_chars.saturating_sub(256).max(1024);
    let head_chars = snippet_budget / 2;
    let tail_chars_budget = snippet_budget.saturating_sub(head_chars);
    let head = truncate_chars(content, head_chars).to_string();
    let tail = tail_chars(content, tail_chars_budget);
    *content =
        format!("[{label} retained-history truncated from {char_count} chars]\n{head}\n…\n{tail}");
    true
}

// A match guard cannot mutably borrow `content`; keeping the mutation inside
// the arm updates both retained representations together without indirection.
#[allow(clippy::collapsible_match)]
fn sanitize_retained_messages(mut messages: Vec<Message>) -> Vec<Message> {
    for message in &mut messages {
        for block in &mut message.content {
            match block {
                ContentBlock::ToolResult {
                    content,
                    content_blocks,
                    ..
                } => {
                    if truncate_retained_block(
                        "tool result",
                        content,
                        RETAINED_TOOL_RESULT_MAX_CHARS,
                    ) {
                        *content_blocks = None;
                    }
                }
                // Signed thinking must stay byte-for-byte valid for providers that
                // verify replay signatures. Unsigned thinking is local memory pressure
                // and can be capped once compaction has summarized the old turn.
                ContentBlock::Thinking {
                    thinking,
                    signature,
                    ..
                } if signature.is_none() => {
                    truncate_retained_block(
                        "thinking block",
                        thinking,
                        RETAINED_THINKING_MAX_CHARS,
                    );
                }
                _ => {}
            }
        }
    }
    messages
}

/// Result of a compaction operation with metadata.
#[derive(Debug)]
pub struct CompactionResult {
    /// Compacted messages
    pub messages: Vec<Message>,
    /// Host-persistence copy of the history checkpoint.
    pub summary_prompt: Option<SystemPrompt>,
    /// Number of retries used before success
    pub retries_used: u32,
    /// Last-round coverage for inspector receipts.
    pub coverage: CompactionCoverage,
}

/// Classify a compaction LLM failure for the retry / input-ladder policy.
fn classify_compaction_failure(e: &anyhow::Error) -> CompactionFailureKind {
    if let Some(error) = llm_error_in_chain(e) {
        return match error {
            crate::llm_client::LlmError::ContextLengthError(_) => {
                CompactionFailureKind::ContextOverflow
            }
            crate::llm_client::LlmError::QuotaExhausted(_) => CompactionFailureKind::Deterministic,
            error if error.is_retryable() => CompactionFailureKind::Transient,
            _ => CompactionFailureKind::Deterministic,
        };
    }

    let text = e.to_string();
    if is_context_window_error_message(&text) {
        return CompactionFailureKind::ContextOverflow;
    }
    let category = crate::error_taxonomy::classify_error_message(&text);
    match category {
        crate::error_taxonomy::ErrorCategory::Network
        | crate::error_taxonomy::ErrorCategory::RateLimit
        | crate::error_taxonomy::ErrorCategory::Timeout => CompactionFailureKind::Transient,
        _ => CompactionFailureKind::Deterministic,
    }
}

fn llm_error_in_chain(error: &anyhow::Error) -> Option<&crate::llm_client::LlmError> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<crate::llm_client::LlmError>())
}

/// Record and render a compaction failure as actionable, credential-safe text.
///
/// This classifies only the error supplied by the failed request; it never
/// infers a cause from later provider failures. Unknown diagnostics stay
/// visible after central secret/path redaction, and the same safe detail is
/// written to the runtime log so a transient status message remains auditable.
#[must_use]
pub fn report_compaction_failure(
    prefix: &str,
    id: &str,
    auto: bool,
    error: &anyhow::Error,
) -> String {
    let raw = error.to_string();
    let safe_raw = crate::safe_label::safe_error_text(&raw);
    tracing::warn!(
        compaction_id = %id,
        auto,
        error = %safe_raw,
        "context compaction failed"
    );
    let detail = match llm_error_in_chain(error) {
        Some(crate::llm_client::LlmError::QuotaExhausted(_)) => {
            "provider plan quota exhausted — switch provider/model or renew the provider plan"
                .to_string()
        }
        Some(crate::llm_client::LlmError::RateLimited { .. }) => {
            "provider rate limit blocked compaction — retry after the limit resets or switch provider/model"
                .to_string()
        }
        Some(crate::llm_client::LlmError::AuthenticationError(_)) => {
            "provider authentication failed — sign in or replace the credential, then retry"
                .to_string()
        }
        Some(crate::llm_client::LlmError::AuthorizationError(_)) => {
            "provider authorization rejected compaction — verify account access or switch provider/model"
                .to_string()
        }
        _ => match crate::error_taxonomy::classify_error_message(&raw) {
            crate::error_taxonomy::ErrorCategory::RateLimit => {
                "provider rate limit blocked compaction — retry after the limit resets or switch provider/model"
                    .to_string()
            }
            crate::error_taxonomy::ErrorCategory::Authentication => {
                "provider authentication failed — sign in or replace the credential, then retry"
                    .to_string()
            }
            crate::error_taxonomy::ErrorCategory::Authorization => {
                "provider authorization rejected compaction — verify account access or switch provider/model"
                    .to_string()
            }
            _ => safe_raw,
        },
    };

    format!("{prefix}: {detail}")
}

/// Check if an error is transient and worth retrying. Categories that map to
/// transient retry: Network, RateLimit, Timeout. Context overflow is *not*
/// transient — it needs a smaller input (ladder), not the same payload.
fn is_transient_error(e: &anyhow::Error) -> bool {
    classify_compaction_failure(e).is_transient()
}

fn is_context_window_error_message(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("too long for this model")
        || lower.contains("prompt is too long")
        || lower.contains("maximum prompt length")
        || lower.contains("maximum context length")
        || lower.contains("context_length_exceeded")
        || lower.contains("context window")
        || (lower.contains("context")
            && (lower.contains("token") || lower.contains("too long") || lower.contains("maximum")))
}

/// Compact messages with retry and backoff for transient errors.
///
/// This function wraps `compact_messages` with retry logic to handle
/// transient network errors and rate limits. It uses exponential backoff
/// with delays of 1s, 2s, 4s between retries.
///
/// # Safety
/// - Never panics
/// - Never corrupts the original messages (returns error instead)
/// - Only retries on transient errors (network, rate limit, etc.)
pub async fn compact_messages_safe(
    client: &dyn ModelClient,
    messages: &[Message],
    system_prompt: Option<&SystemPrompt>,
    prepared: &PreparedCompactionEnvelope,
) -> Result<CompactionResult> {
    const MAX_RETRIES: u32 = 3;
    const BASE_DELAY_MS: u64 = 1000;

    let config = &prepared.config;
    let was_over_threshold = compaction_pressure_reached(messages, system_prompt, config);
    let mut pruned_messages = messages.to_vec();
    let mut now_under_threshold = false;
    let mut next_stop_check_bytes = 0usize;
    let pruned_bytes = prune_tool_results_until(
        &mut pruned_messages,
        KEEP_RECENT_MESSAGES,
        |candidate_messages, bytes_saved| {
            if !was_over_threshold || bytes_saved < next_stop_check_bytes {
                return false;
            }

            // Stop at the first suffix-side prune check that clears the threshold.
            // The check itself is a full compaction-plan pass, so bound it by saved
            // bytes instead of running it after every candidate in huge sessions.
            next_stop_check_bytes = bytes_saved.saturating_add(TOOL_PRUNE_STOP_CHECK_BYTES);
            now_under_threshold =
                !compaction_pressure_reached(candidate_messages, system_prompt, config);
            now_under_threshold
        },
    );
    if was_over_threshold && pruned_bytes > 0 && !now_under_threshold {
        // The throttled in-loop check may skip the exact candidate that clears the
        // budget. Do one final pass so a successful local prune still avoids LLM compaction.
        now_under_threshold = !compaction_pressure_reached(&pruned_messages, system_prompt, config);
    }

    let compaction_input: &[Message] = if pruned_bytes > 0 {
        logging::info(format!(
            "Local tool-result prune saved {pruned_bytes} bytes before LLM compaction"
        ));
        if was_over_threshold && now_under_threshold {
            let kept = sanitize_retained_messages(pruned_messages);
            last_round::validate_last_round_coverage(messages, &kept)?;
            let coverage = last_round::measure_coverage(
                messages,
                &kept,
                CompactionPath::PruneOnly,
                pinned_anchors_text(config.workspace.as_deref())
                    .map(|text| text.chars().count())
                    .unwrap_or(0),
            );
            return Ok(CompactionResult {
                messages: kept,
                summary_prompt: None,
                retries_used: 0,
                coverage,
            });
        }
        &pruned_messages
    } else {
        messages
    };

    let mut last_error: Option<anyhow::Error> = None;
    let mut quality_retries = 0u32;

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            // Exponential backoff: 1s, 2s, 4s
            let delay = Duration::from_millis(BASE_DELAY_MS * (1 << (attempt - 1)));
            tokio::time::sleep(delay).await;
        }

        match compact_messages_with_metadata(client, compaction_input, config, &mut quality_retries)
            .await
        {
            Ok((msgs, prompt, mut coverage)) => {
                let kept = sanitize_retained_messages(msgs);
                last_round::validate_last_round_coverage(compaction_input, &kept)?;
                let keep: CompactionKeep = inspect_compaction_keep(&kept);
                coverage.last_round_messages = keep.last_round_messages;
                coverage.last_round_tool_results = keep.last_round_tool_results;
                coverage.last_round_assistant = keep.last_round_assistant;
                return Ok(CompactionResult {
                    messages: kept,
                    summary_prompt: prompt,
                    retries_used: attempt.saturating_add(quality_retries),
                    coverage,
                });
            }
            Err(e) => {
                // Only retry on transient errors
                if !is_transient_error(&e) {
                    return Err(e);
                }
                last_error = Some(e);
            }
        }
    }

    Err(last_error
        .unwrap_or_else(|| anyhow::anyhow!("Compaction failed after {MAX_RETRIES} retries")))
}

fn build_compaction_summary_block_text(summary: &str, anchors: &str) -> String {
    let summary = summary.trim();
    let summary = if summary.is_empty() {
        "(no summary available)"
    } else {
        summary
    };
    let mut text = format!("{SUMMARY_HEADER}\n\n{summary}");
    text.push_str(anchors);
    text
}

/// Codex-parity replacement history: the most recent plain user messages,
/// selected newest-first within a fixed token budget and restored to
/// transcript order. The oldest selected message is truncated to fit rather
/// than dropped whole.
pub(crate) fn retained_user_messages(messages: &[Message], max_tokens: usize) -> Vec<Message> {
    let mut selected: Vec<Message> = Vec::new();
    let mut remaining = max_tokens;
    for msg in messages.iter().rev() {
        if remaining == 0 {
            break;
        }
        let Some(text) = user_text_of(msg) else {
            continue;
        };
        if is_compaction_summary_text(&text) {
            continue;
        }
        let tokens = estimate_text_tokens_conservative(&text);
        let text = if tokens <= remaining {
            remaining -= tokens;
            text
        } else {
            let budget_chars = remaining.saturating_mul(3).max(1);
            remaining = 0;
            truncate_chars(&text, budget_chars).to_string()
        };
        selected.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text,
                cache_control: None,
            }],
        });
    }
    selected.reverse();
    selected
}

/// User-pinned facts from `/anchor` (`.codewhale/anchors.md`). These are the
/// user's own words, re-stated after the summary because the command promises
/// they survive compaction.
fn user_anchors_section(workspace: Option<&std::path::Path>) -> String {
    match pinned_anchors_text(workspace) {
        Some(contents) => format!("\n\nUser-pinned anchors (verbatim):\n{contents}"),
        None => String::new(),
    }
}

#[cfg(test)]
async fn compact_messages(
    client: &dyn ModelClient,
    messages: &[Message],
    config: &CompactionConfig,
) -> Result<(Vec<Message>, Option<SystemPrompt>, Vec<Message>)> {
    let mut quality_retries = 0;
    let (messages, summary_prompt, _coverage) =
        compact_messages_with_metadata(client, messages, config, &mut quality_retries).await?;
    Ok((messages, summary_prompt, Vec::new()))
}

async fn compact_messages_with_metadata(
    client: &dyn ModelClient,
    messages: &[Message],
    config: &CompactionConfig,
    quality_retries: &mut u32,
) -> Result<(Vec<Message>, Option<SystemPrompt>, CompactionCoverage)> {
    if messages.is_empty() {
        return Ok((Vec::new(), None, CompactionCoverage::default()));
    }

    let summary = create_summary(client, messages, config, quality_retries).await?;
    let anchors = user_anchors_section(config.workspace.as_deref());
    let checkpoint_text = build_compaction_summary_block_text(&summary, &anchors);
    let summary_block = SystemBlock {
        block_type: "text".to_string(),
        text: checkpoint_text.clone(),
        cache_control: config.cache_summary.then(|| CacheControl {
            cache_type: "ephemeral".to_string(),
        }),
    };

    let retained = last_round::build_replacement_history(
        messages,
        &checkpoint_text,
        pinned_anchors_text(config.workspace.as_deref()).as_deref(),
    )?;
    let coverage = last_round::measure_coverage(
        messages,
        &retained,
        CompactionPath::Summary,
        pinned_anchors_text(config.workspace.as_deref())
            .map(|text| text.chars().count())
            .unwrap_or(0),
    );
    Ok((
        retained,
        Some(SystemPrompt::Blocks(vec![summary_block])),
        coverage,
    ))
}

fn compact_prompt(focus: Option<&str>) -> String {
    let mut prompt = format!("{COMPACT_PROMPT} {COMPACTION_LANGUAGE_CONTRACT}");
    if let Some(focus) = focus.map(str::trim).filter(|focus| !focus.is_empty()) {
        let _ = write!(
            prompt,
            "\n\nThe user asked this compaction to focus on: {focus}"
        );
    }
    prompt
}

fn compact_quality_retry_prompt(focus: Option<&str>) -> String {
    let mut prompt = format!(
        "The previous handoff response was empty or a placeholder. Return a substantive factual \
continuation handoff. State the user objective, completed and current work, hard constraints, verified \
evidence, unresolved failures, and the single next action. Do not refuse, call tools, discuss \
checkpoint machinery, or return a placeholder. {COMPACTION_LANGUAGE_CONTRACT}"
    );
    if let Some(focus) = focus.map(str::trim).filter(|focus| !focus.is_empty()) {
        let _ = write!(
            prompt,
            "\n\nThe user asked this compaction to focus on: {focus}"
        );
    }
    prompt
}

fn validate_compaction_summary(summary: &str) -> Result<()> {
    let trimmed = summary.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Compaction summary response was unusable: no text was returned.");
    }

    // Strip every non-word edge, not just ASCII punctuation. Providers can
    // return visually non-empty Unicode punctuation or emoji-only payloads;
    // neither is a usable continuation checkpoint. `is_alphanumeric` keeps
    // this language-neutral for CJK and other scripts without imposing a
    // prose-length heuristic.
    let normalized = trimmed
        .trim_matches(|ch: char| !ch.is_alphanumeric())
        .to_ascii_lowercase();
    if normalized.is_empty() {
        anyhow::bail!(
            "Compaction summary response was unusable: only whitespace or punctuation was returned."
        );
    }
    if matches!(
        normalized.as_str(),
        "no summary available"
            | "summary unavailable"
            | "no summary"
            | "n/a"
            | "na"
            | "not available"
            | "i cannot provide a summary"
            | "i can't provide a summary"
            | "unable to provide a summary"
    ) {
        anyhow::bail!("Compaction summary response was unusable: a placeholder was returned.");
    }
    Ok(())
}

/// Drop the oldest history message before retrying an over-window summary
/// request (Codex parity: `history.remove_first_item()`), plus any tool
/// results the removal orphans — strict providers reject unpaired results.
fn drop_oldest_history_messages(messages: &mut Vec<Message>) {
    if messages.len() <= 1 {
        return;
    }
    messages.remove(0);
    while messages.len() > 1
        && messages[0]
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
    {
        messages.remove(0);
    }
}

async fn create_summary(
    client: &dyn ModelClient,
    messages: &[Message],
    config: &CompactionConfig,
    quality_retries: &mut u32,
) -> Result<String> {
    // The summarization request IS the live conversation plus one final user
    // message asking for the handoff summary, so the provider's prefix cache
    // covers everything already sent this session.
    let mut request_messages = messages.to_vec();
    let stripped_images = crate::image_attach::strip_images_when_unsupported(
        &mut request_messages,
        config.image_input,
        &config.model,
    );
    if stripped_images > 0 {
        logging::warn(format!(
            "Compaction omitted {stripped_images} image block(s) unsupported by its route"
        ));
    }
    request_messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: compact_prompt(config.focus.as_deref()),
            cache_control: None,
        }],
    });

    let mut quality_retry_used = false;
    loop {
        // Codex compaction is a normal model generation over the existing
        // cached prefix. Do the same here: the resolved route decides how
        // much output the model may need instead of imposing a smaller,
        // compaction-only ceiling that can be consumed by hidden reasoning.
        let cost_route = client.effective_route_envelope(&config.model, chrono::Utc::now());
        let request = MessageRequest {
            model: config.model.clone(),
            messages: request_messages.clone(),
            max_tokens: client.effective_max_output_tokens(&cost_route.model),
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: Some(false),
            // Route parity with ordinary turns: turns send no sampling
            // params, so every provider's own normalization/defaults apply.
            // A hard-coded 0.3 leaked to the wire on routes that pass
            // temperature through (e.g. Kimi Code membership), where the
            // fixed-sampling contract rejects it and the whole compaction
            // pass fails.
            temperature: None,
            top_p: None,
        };

        // Capture the session scope before awaiting so a late response cannot
        // accrue into a subsequently loaded/new session.
        let cost_scope = crate::cost_status::scope_token();
        let response = match client.create_message(request).await {
            Ok(response) => response,
            Err(err) if is_context_window_error(&err) && request_messages.len() > 2 => {
                logging::warn(format!(
                    "Compaction summary input over the context window ({err}); \
                     dropping the oldest history item and retrying"
                ));
                drop_oldest_history_messages(&mut request_messages);
                continue;
            }
            Err(err) => return Err(err),
        };

        // Compaction summary calls are billed; route the tokens through the
        // side-channel so the dashboard total matches the website (#526).
        crate::cost_status::report_effective_route_for_runtime(
            cost_scope,
            config.runtime_cost_owner.as_deref(),
            &format!(
                "compaction:dispatch:{}:response:{}",
                cost_route
                    .dispatched_at
                    .timestamp_nanos_opt()
                    .unwrap_or_default(),
                response.id
            ),
            &cost_route,
            &response.usage,
        );

        // Usage above is already billed; a provider-declared incomplete
        // summary must still fail rather than replace the session history
        // with a fragment.
        if crate::models::is_incomplete_stop_reason(response.stop_reason.as_deref()) {
            anyhow::bail!(
                "Compaction summary response incomplete: provider stop reason `{}`; the partial summary was not accepted.",
                crate::models::stop_reason_detail(response.stop_reason.as_deref())
            );
        }

        let summary = response
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        if let Err(error) = validate_compaction_summary(&summary) {
            if quality_retry_used {
                return Err(error.context(
                    "Compaction summary remained unusable after one conservative retry; \
no replacement checkpoint was committed",
                ));
            }

            quality_retry_used = true;
            *quality_retries = (*quality_retries).saturating_add(1);
            logging::warn(
                "Compaction provider returned an unusable successful response; retrying once with the conservative handoff prompt",
            );
            let Some(instruction) = request_messages.last_mut() else {
                return Err(error.context(
                    "Compaction summary validation failed and the retry instruction was missing",
                ));
            };
            instruction.content = vec![ContentBlock::Text {
                text: compact_quality_retry_prompt(config.focus.as_deref()),
                cache_control: None,
            }];
            continue;
        }

        return Ok(summary);
    }
}

fn is_context_window_error(e: &anyhow::Error) -> bool {
    let text = e.to_string();
    if crate::error_taxonomy::classify_error_message(&text)
        != crate::error_taxonomy::ErrorCategory::InvalidInput
    {
        return false;
    }

    let lower = text.to_lowercase();
    lower.contains("context")
        || lower.contains("token")
        || lower.contains("prompt is too long")
        || lower.contains("requested")
        || lower.contains("maximum")
}

/// Cache-hit percentage for a compaction summary call.
///
/// Denominator is `input_tokens` (the total prompt size), not
/// `cache_hit + cache_miss`. Some providers populate
/// `prompt_cache_hit_tokens` but not `prompt_cache_miss_tokens` — using
/// the sum as the denominator there reports an inflated 100% even when
/// most of the prompt was uncached. Anchoring on `input_tokens` matches
/// how the rest of the codebase (cost reporting, `/cache`) infers
/// missing miss counts. (#584)
fn user_text_of(msg: &Message) -> Option<String> {
    if msg.role != "user" {
        return None;
    }
    let text = msg
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(test)]
#[path = "compaction/tests.rs"]
mod quota_tests;

#[cfg(test)]
mod tests {
    use crate::models::{ImageUrlContent, Message};

    #[test]
    fn inline_image_estimates_nonzero_tokens() {
        let msg = Message {
            role: Role::User,
            content: vec![ContentBlock::ImageUrl {
                image_url: ImageUrlContent {
                    url: "data:image/png;base64,AAAA".to_string(),
                },
            }],
        };
        assert!(
            estimate_tokens_for_message(&msg, false) >= IMAGE_TOKEN_ESTIMATE,
            "an inline image must not estimate to 0 tokens"
        );
    }

    use super::*;
    use serde_json::json;

    fn msg(role: &str, text: &str) -> Message {
        Message {
            role: Role::from(role),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    fn prepared(config: &CompactionConfig) -> PreparedCompactionEnvelope {
        PreparedCompactionEnvelope::new(config.clone())
    }

    fn tool_use(id: &str, name: &str, input: serde_json::Value) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input,
                caller: None,
                thought_signature: None,
            }],
        }
    }

    fn tool_result(id: &str, content: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: content.to_string(),
                is_error: None,
                content_blocks: None,
            }],
        }
    }

    #[test]
    fn truncate_chars_respects_unicode_boundaries() {
        let text = "abc😀é";
        assert_eq!(truncate_chars(text, 0), "");
        assert_eq!(truncate_chars(text, 1), "a");
        assert_eq!(truncate_chars(text, 3), "abc");
        assert_eq!(truncate_chars(text, 4), "abc😀");
        assert_eq!(truncate_chars(text, 5), "abc😀é");
    }

    #[test]
    fn prune_tool_results_summarizes_old_verbose_outputs() {
        let verbose = "x".repeat(SUMMARY_TOOL_RESULT_SNIPPET_CHARS + 80);
        let mut messages = vec![
            tool_use("call-1", "read_file", json!({"path": "Cargo.toml"})),
            tool_result("call-1", &verbose),
            msg("user", "recent question"),
            msg("assistant", "recent answer"),
        ];

        let saved = prune_tool_results(&mut messages, 2);

        assert!(saved > 0);
        let ContentBlock::ToolResult { content, .. } = &messages[1].content[0] else {
            panic!("expected tool result");
        };
        assert!(content.contains("[read_file] tool result pruned"));
        assert!(content.contains("Cargo.toml"));
        assert!(content.len() < verbose.len());
    }

    #[test]
    fn prune_tool_results_preserves_protected_tail() {
        let verbose = "x".repeat(SUMMARY_TOOL_RESULT_SNIPPET_CHARS + 80);
        let mut messages = vec![
            msg("user", "older context"),
            tool_use("call-1", "read_file", json!({"path": "Cargo.toml"})),
            tool_result("call-1", &verbose),
        ];

        let saved = prune_tool_results(&mut messages, 2);

        assert_eq!(saved, 0);
        let ContentBlock::ToolResult { content, .. } = &messages[2].content[0] else {
            panic!("expected tool result");
        };
        assert_eq!(content, &verbose);
    }

    #[test]
    fn prune_tool_results_preserves_prefix_bytes_when_reverse_prune_is_enough() {
        let older_verbose = "old ".repeat(SUMMARY_TOOL_RESULT_SNIPPET_CHARS + 40);
        let newer_verbose = "new ".repeat(SUMMARY_TOOL_RESULT_SNIPPET_CHARS + 40);
        let mut messages = vec![
            tool_use("call-old", "read_file", json!({"path": "old.txt"})),
            tool_result("call-old", &older_verbose),
            tool_use("call-new", "read_file", json!({"path": "new.txt"})),
            tool_result("call-new", &newer_verbose),
            msg("user", "protected tail"),
        ];
        let original = messages.clone();

        // Simulate the caller clearing its token budget after one suffix prune.
        let saved = prune_tool_results_until(&mut messages, 1, |_, saved| saved > 0);

        assert!(saved > 0);
        assert_eq!(&messages[..3], &original[..3]);
        assert_eq!(&messages[4..], &original[4..]);
        let ContentBlock::ToolResult { content, .. } = &messages[3].content[0] else {
            panic!("expected pruned tool result");
        };
        assert!(content.contains("[read_file] tool result pruned"));
        assert!(content.contains("new.txt"));
        assert!(content.len() < newer_verbose.len());
    }

    #[test]
    fn prune_tool_results_stops_after_newest_duplicate_prune() {
        let oldest = "oldest ".repeat(80);
        let middle = "middle ".repeat(80);
        let latest = "latest ".repeat(80);
        let mut messages = vec![
            tool_use("call-1", "read_file", json!({"path": "Cargo.toml"})),
            tool_result("call-1", &oldest),
            tool_use("call-2", "read_file", json!({"path": "Cargo.toml"})),
            tool_result("call-2", &middle),
            tool_use("call-3", "read_file", json!({"path": "Cargo.toml"})),
            tool_result("call-3", &latest),
            msg("user", "protected tail"),
        ];
        let original = messages.clone();

        let saved = prune_tool_results_until(&mut messages, 1, |_, saved| saved > 0);

        assert!(saved > 0);
        assert_eq!(&messages[..3], &original[..3]);
        assert_eq!(&messages[4..], &original[4..]);
        let ContentBlock::ToolResult { content, .. } = &messages[3].content[0] else {
            panic!("expected middle duplicate to be pruned");
        };
        assert!(content.contains("[read_file] tool result pruned"));
    }

    #[test]
    fn prune_tool_results_dedupes_identical_reads_but_keeps_latest_full_body() {
        let first = "first ".repeat(80);
        let second = "second ".repeat(80);
        let mut messages = vec![
            tool_use("call-1", "read_file", json!({"path": "Cargo.toml"})),
            tool_result("call-1", &first),
            tool_use("call-2", "read_file", json!({"path": "Cargo.toml"})),
            tool_result("call-2", &second),
            msg("user", "tail"),
        ];

        let saved = prune_tool_results(&mut messages, 1);

        assert!(saved > 0);
        let ContentBlock::ToolResult { content: older, .. } = &messages[1].content[0] else {
            panic!("expected older tool result");
        };
        assert!(older.contains("tool result pruned"));
        let ContentBlock::ToolResult {
            content: latest, ..
        } = &messages[3].content[0]
        else {
            panic!("expected latest tool result");
        };
        assert_eq!(latest, &second);
    }

    #[test]
    fn context_window_errors_are_detected_for_summary_fallback() {
        for msg in [
            "HTTP 400 Bad Request: maximum context length is 1000000 tokens",
            "invalid_request_error: prompt is too long for the current model",
            "You requested 1000001 tokens but the maximum is 1000000",
            "request exceeds context window",
        ] {
            assert!(
                is_context_window_error(&anyhow::anyhow!(msg)),
                "expected context-window detection for `{msg}`",
            );
        }

        assert!(!is_context_window_error(&anyhow::anyhow!(
            "Invalid request: missing required field"
        )));
        assert!(!is_context_window_error(&anyhow::anyhow!(
            "503 Service Unavailable"
        )));
    }

    #[test]
    fn tool_args_preview_redacts_sensitive_first_without_dropping_siblings() {
        let input: serde_json::Value = serde_json::from_str(
            r#"{"api_key":"sk-tool-secret-value","command":"cargo test -p auth"}"#,
        )
        .unwrap();

        let preview: serde_json::Value = serde_json::from_str(&tool_args_preview(&input)).unwrap();

        assert_eq!(preview["api_key"], codewhale_config::persistence::REDACTED);
        assert_eq!(preview["command"], "cargo test -p auth");
    }

    #[test]
    fn tool_args_preview_redacts_sensitive_later_without_touching_earlier_fields() {
        let input: serde_json::Value =
            serde_json::from_str(r#"{"command":"cargo test","api_key":"plain-secret-value"}"#)
                .unwrap();

        let preview: serde_json::Value = serde_json::from_str(&tool_args_preview(&input)).unwrap();

        assert_eq!(preview["command"], "cargo test");
        assert_eq!(preview["api_key"], codewhale_config::persistence::REDACTED);
    }

    #[test]
    fn tool_args_preview_redacts_nested_sensitive_values_recursively() {
        let input: serde_json::Value = serde_json::from_str(
            r#"{"meta":{"token":"nested-secret","keep":"yes"},"steps":[{"password":"pw","name":"a"}]}"#,
        )
        .unwrap();

        let preview: serde_json::Value = serde_json::from_str(&tool_args_preview(&input)).unwrap();

        assert_eq!(
            preview["meta"]["token"],
            codewhale_config::persistence::REDACTED
        );
        assert_eq!(preview["meta"]["keep"], "yes");
        assert_eq!(
            preview["steps"][0]["password"],
            codewhale_config::persistence::REDACTED
        );
        assert_eq!(preview["steps"][0]["name"], "a");
    }

    #[test]
    fn tool_args_preview_redacts_complete_multi_word_secret_value() {
        let input: serde_json::Value =
            serde_json::from_str(r#"{"command":"run this","password":"hunter two words"}"#)
                .unwrap();

        let serialized = tool_args_preview(&input);
        let preview: serde_json::Value = serde_json::from_str(&serialized).unwrap();

        assert_eq!(preview["command"], "run this");
        assert_eq!(preview["password"], codewhale_config::persistence::REDACTED);
        assert!(!serialized.contains("hunter"));
        assert!(!serialized.contains("two words"));
    }

    struct FixedSummaryClient {
        request: std::sync::Mutex<Option<MessageRequest>>,
        provider: &'static str,
        model: &'static str,
    }

    impl Default for FixedSummaryClient {
        fn default() -> Self {
            Self {
                request: std::sync::Mutex::new(None),
                provider: "test",
                model: "test-model",
            }
        }
    }

    impl FixedSummaryClient {
        fn for_route(provider: &'static str, model: &'static str) -> Self {
            Self {
                request: std::sync::Mutex::new(None),
                provider,
                model,
            }
        }
    }

    const FIXED_SUMMARY: &str = "1. Primary request and intent — migrate the session store. \
        2. Key technical concepts — sqlite. 7. Pending tasks — finish the fixed clock. \
        8. Current work — rerunning the session tests.";

    struct ScriptedSummaryClient {
        responses: std::sync::Mutex<std::collections::VecDeque<anyhow::Result<Vec<ContentBlock>>>>,
        requests: std::sync::Mutex<Vec<MessageRequest>>,
    }

    impl ScriptedSummaryClient {
        fn new(responses: Vec<Vec<ContentBlock>>) -> Self {
            Self::with_outcomes(responses.into_iter().map(Ok).collect())
        }

        fn with_outcomes(responses: Vec<anyhow::Result<Vec<ContentBlock>>>) -> Self {
            Self {
                responses: std::sync::Mutex::new(responses.into()),
                requests: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::core::model_client::ModelClient for ScriptedSummaryClient {
        fn provider_name(&self) -> &str {
            "test"
        }

        fn model(&self) -> &str {
            "test-model"
        }

        async fn create_message(
            &self,
            request: MessageRequest,
        ) -> anyhow::Result<crate::models::MessageResponse> {
            self.requests
                .lock()
                .expect("capture scripted summary request")
                .push(request);
            let outcome = self
                .responses
                .lock()
                .expect("read scripted summary response")
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("scripted summary responses exhausted"))?;
            let content = outcome?;
            Ok(crate::models::MessageResponse {
                id: "summary-scripted".to_string(),
                r#type: "message".to_string(),
                role: "assistant".to_string(),
                content,
                model: "test-model".to_string(),
                stop_reason: None,
                stop_sequence: None,
                container: None,
                usage: crate::models::Usage::default(),
            })
        }

        async fn create_message_stream(
            &self,
            _request: MessageRequest,
        ) -> anyhow::Result<crate::llm_client::StreamEventBox> {
            anyhow::bail!("streaming is unused by compaction")
        }

        async fn health_check(&self) -> anyhow::Result<bool> {
            Ok(true)
        }
    }

    #[async_trait::async_trait]
    impl crate::core::model_client::ModelClient for FixedSummaryClient {
        fn provider_name(&self) -> &str {
            self.provider
        }

        fn model(&self) -> &str {
            self.model
        }

        async fn create_message(
            &self,
            request: MessageRequest,
        ) -> anyhow::Result<crate::models::MessageResponse> {
            *self.request.lock().expect("capture summary request") = Some(request);
            Ok(crate::models::MessageResponse {
                id: "summary-fixture".to_string(),
                r#type: "message".to_string(),
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: FIXED_SUMMARY.to_string(),
                    cache_control: None,
                }],
                model: self.model.to_string(),
                stop_reason: None,
                stop_sequence: None,
                container: None,
                usage: crate::models::Usage::default(),
            })
        }

        async fn create_message_stream(
            &self,
            _request: MessageRequest,
        ) -> anyhow::Result<crate::llm_client::StreamEventBox> {
            anyhow::bail!("streaming is unused by compaction")
        }

        async fn health_check(&self) -> anyhow::Result<bool> {
            Ok(true)
        }
    }

    #[tokio::test]
    async fn compaction_commits_summary_and_retains_recent_user_messages() {
        let messages = vec![
            msg(
                "user",
                "Objective: migrate the session store to sqlite without breaking existing logins",
            ),
            msg("assistant", "Working on it."),
            tool_use(
                "t1",
                "Bash",
                json!({"command": "cargo test -p session-store"}),
            ),
            tool_result("t1", "test session_store::roundtrip ... ok\nexit code 0"),
            msg("user", "Sounds good, do it"),
            msg("assistant", "Nearly done, rerunning the suite."),
        ];
        let config = CompactionConfig {
            model: "test-model".to_string(),
            cache_summary: false,
            ..Default::default()
        };
        let client = FixedSummaryClient::default();

        let (retained, summary_prompt, _) =
            compact_messages(&client, &messages, &config).await.unwrap();

        let request = client
            .request
            .lock()
            .expect("read summary request")
            .clone()
            .expect("summary request was captured");
        assert_eq!(&request.messages[..messages.len()], messages.as_slice());
        assert_eq!(request.messages.len(), messages.len() + 1);
        let ContentBlock::Text { text, .. } = &request.messages.last().unwrap().content[0] else {
            panic!("final compaction instruction must be text");
        };
        assert!(!text.contains(COMPACTION_SUMMARY_MARKER));
        assert_eq!(request.temperature, None);
        assert_eq!(request.top_p, None);
        assert_eq!(
            request.max_tokens,
            crate::route_budget::effective_max_output_tokens_for_route(
                crate::config::ApiProvider::Custom,
                "test-model",
                None,
            )
        );

        let Some(SystemPrompt::Blocks(blocks)) = summary_prompt else {
            panic!("compaction must produce a summary system block");
        };
        let text = &blocks[0].text;
        assert!(text.contains(FIXED_SUMMARY));
        assert!(text.contains("Another language model"));

        // Replacement history keeps older user turns, then the open round
        // verbatim (user + assistant + tools), then one checkpoint.
        assert!(retained.iter().any(|message| {
            user_text_of(message).is_some_and(|text| text.contains("Objective: migrate"))
        }));
        assert!(
            retained
                .iter()
                .any(|message| { user_text_of(message).as_deref() == Some("Sounds good, do it") })
        );
        assert!(retained.iter().any(|message| {
            message.role.is_assistant_like()
                && message.content.iter().any(|block| {
                    matches!(
                        block,
                        ContentBlock::Text { text, .. }
                            if text.contains("Nearly done, rerunning the suite.")
                    )
                })
        }));
        assert!(retained.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::ToolResult { content, .. }
                        if content.contains("session_store::roundtrip")
                )
            })
        }));
        assert!(is_compaction_checkpoint_message(retained.last().unwrap()));
        assert_eq!(
            user_text_of(retained.last().unwrap()).as_deref(),
            Some(text.as_str())
        );
        last_round::validate_last_round_coverage(&messages, &retained[..retained.len() - 1])
            .unwrap();
    }

    #[test]
    fn coverage_floor_rejects_a_replacement_that_drops_last_round_assistant() {
        let original = vec![
            msg("user", "What failed?"),
            msg("assistant", "session_store::roundtrip panics on reload."),
        ];
        let gutting = vec![msg("user", "What failed?")];
        let error = last_round::validate_last_round_coverage(&original, &gutting)
            .expect_err("dropping last-round assistant text must fail closed");
        assert!(error.to_string().contains("assistant"), "{error}");
    }

    #[test]
    fn summary_quality_gate_rejects_empty_and_known_placeholder_text() {
        for summary in [
            "",
            " \n\t ",
            "...",
            "。。。",
            "🫧",
            "N/A",
            "(no summary available)",
            "I cannot provide a summary.",
        ] {
            let error = validate_compaction_summary(summary)
                .expect_err("degenerate summary must fail closed");
            assert!(error.to_string().contains("unusable"), "{error}");
        }
        validate_compaction_summary(FIXED_SUMMARY)
            .expect("a substantive continuation handoff must be accepted");
        validate_compaction_summary(
            "目的: #4394の空要約を防止。完了: 検証と実装。制約: 履歴を変更しない。次: テスト実行。",
        )
        .expect("a concise multilingual handoff must not be rejected by prose length");
    }

    #[tokio::test]
    async fn empty_successful_summary_retries_once_without_replacing_history() {
        let original = vec![
            msg(
                "user",
                "Keep the migration transactional and preserve existing sessions.",
            ),
            msg(
                "assistant",
                "I am updating the session store and its fixtures.",
            ),
        ];
        let client = ScriptedSummaryClient::new(vec![
            vec![ContentBlock::Text {
                text: " \n\t ".to_string(),
                cache_control: None,
            }],
            vec![ContentBlock::Text {
                text: FIXED_SUMMARY.to_string(),
                cache_control: None,
            }],
        ]);
        let config = CompactionConfig {
            model: "test-model".to_string(),
            cache_summary: false,
            ..Default::default()
        };

        let result = compact_messages_safe(&client, &original, None, &prepared(&config))
            .await
            .expect("the conservative retry should recover a usable summary");

        let requests = client
            .requests
            .lock()
            .expect("read scripted summary requests");
        assert_eq!(requests.len(), 2, "quality failure retries exactly once");
        let ContentBlock::Text { text, .. } = &requests[1]
            .messages
            .last()
            .expect("retry instruction")
            .content[0]
        else {
            panic!("retry instruction must be text");
        };
        assert!(text.contains("previous handoff response was empty"));
        drop(requests);

        assert_eq!(
            result.retries_used, 1,
            "quality retry must reach diagnostics"
        );
        assert_eq!(original[0].role, "user", "source history remains untouched");
        assert!(result.messages.iter().any(is_compaction_checkpoint_message));
        let Some(SystemPrompt::Blocks(blocks)) = result.summary_prompt else {
            panic!("recovered summary must be committed");
        };
        assert!(blocks[0].text.contains(FIXED_SUMMARY));
        assert!(!blocks[0].text.contains("(no summary available)"));
    }

    #[tokio::test]
    async fn quality_retry_count_survives_a_later_transient_failure() {
        let client = ScriptedSummaryClient::with_outcomes(vec![
            Ok(vec![ContentBlock::Text {
                text: "...".to_string(),
                cache_control: None,
            }]),
            Err(anyhow::anyhow!("request timed out")),
            Ok(vec![ContentBlock::Text {
                text: FIXED_SUMMARY.to_string(),
                cache_control: None,
            }]),
        ]);
        let config = CompactionConfig {
            model: "test-model".to_string(),
            cache_summary: false,
            ..Default::default()
        };

        let result = compact_messages_safe(
            &client,
            &[msg("user", "Preserve the current migration state.")],
            None,
            &prepared(&config),
        )
        .await
        .expect("the outer retry should recover after the transient failure");

        assert_eq!(
            result.retries_used, 2,
            "one quality retry plus one outer transient retry must be reported"
        );
        assert_eq!(
            client
                .requests
                .lock()
                .expect("read scripted summary requests")
                .len(),
            3,
            "the diagnostic count must match the two calls after the initial request"
        );
    }

    #[tokio::test]
    async fn non_text_summary_failure_preserves_history_after_one_retry() {
        let original = vec![
            msg(
                "user",
                "Do not lose the current branch or the failing test name.",
            ),
            msg("assistant", "The failing test is session_store::roundtrip."),
        ];
        let client = ScriptedSummaryClient::new(vec![
            vec![ContentBlock::thinking("internal-only response")],
            vec![ContentBlock::thinking("still no user-visible handoff")],
        ]);
        let config = CompactionConfig {
            model: "test-model".to_string(),
            cache_summary: false,
            ..Default::default()
        };

        let error = compact_messages_safe(&client, &original, None, &prepared(&config))
            .await
            .expect_err("two non-text responses must not replace history");

        assert!(
            error
                .to_string()
                .contains("remained unusable after one conservative retry"),
            "{error}"
        );
        assert_eq!(
            client
                .requests
                .lock()
                .expect("read scripted summary requests")
                .len(),
            2,
            "quality failure gets one retry, not the transient retry ladder"
        );
        assert_eq!(
            original,
            vec![
                msg(
                    "user",
                    "Do not lose the current branch or the failing test name."
                ),
                msg("assistant", "The failing test is session_store::roundtrip."),
            ],
            "borrowed source history must remain byte-for-byte unchanged"
        );
    }

    #[tokio::test]
    async fn compaction_uses_the_resolved_route_output_allowance() {
        for (route_label, provider, model) in [
            (
                "thinking-default route",
                crate::config::ApiProvider::Deepseek,
                "deepseek-v4-flash",
            ),
            (
                "fixed-sampling route",
                crate::config::ApiProvider::Moonshot,
                "k3",
            ),
        ] {
            let client = FixedSummaryClient::for_route(provider.as_str(), model);
            let config = CompactionConfig {
                model: model.to_string(),
                cache_summary: false,
                ..Default::default()
            };
            compact_messages(&client, &[msg("user", "summarize this task")], &config)
                .await
                .expect("route compaction should complete");

            let request = client
                .request
                .lock()
                .expect("read summary request")
                .clone()
                .expect("summary request was captured");
            assert_eq!(
                request.max_tokens,
                crate::route_budget::effective_max_output_tokens_for_route(provider, model, None),
                "{route_label} must use the ordinary route output policy"
            );
            assert_eq!(request.temperature, None);
            assert_eq!(request.top_p, None);
        }
    }

    struct TruncatedSummaryClient;

    #[async_trait::async_trait]
    impl crate::core::model_client::ModelClient for TruncatedSummaryClient {
        fn provider_name(&self) -> &str {
            "test"
        }

        fn model(&self) -> &str {
            "test-model"
        }

        async fn create_message(
            &self,
            _request: MessageRequest,
        ) -> anyhow::Result<crate::models::MessageResponse> {
            Ok(crate::models::MessageResponse {
                id: "summary-truncated".to_string(),
                r#type: "message".to_string(),
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "1. Primary request and intent — mig".to_string(),
                    cache_control: None,
                }],
                model: "test-model".to_string(),
                stop_reason: Some("max_tokens".to_string()),
                stop_sequence: None,
                container: None,
                usage: crate::models::Usage::default(),
            })
        }

        async fn create_message_stream(
            &self,
            _request: MessageRequest,
        ) -> anyhow::Result<crate::llm_client::StreamEventBox> {
            anyhow::bail!("streaming is unused by compaction")
        }

        async fn health_check(&self) -> anyhow::Result<bool> {
            Ok(true)
        }
    }

    /// A provider-truncated summary must fail compaction instead of replacing
    /// session history with a fragment.
    #[tokio::test]
    async fn truncated_summary_response_fails_compaction() {
        let messages: Vec<Message> = (0..40)
            .map(|index| {
                msg(
                    if index % 2 == 0 { "user" } else { "assistant" },
                    &format!("padding message {index} with enough text to compact"),
                )
            })
            .collect();
        let config = CompactionConfig {
            model: "test-model".to_string(),
            cache_summary: false,
            ..Default::default()
        };

        let error = compact_messages(&TruncatedSummaryClient, &messages, &config)
            .await
            .expect_err("a truncated summary must not be committed");
        let text = error.to_string();
        assert!(text.contains("incomplete"), "{text}");
        assert!(text.contains("max_tokens"), "{text}");
    }

    #[test]
    fn estimate_tokens_empty_messages() {
        let messages: Vec<Message> = vec![];
        assert_eq!(estimate_tokens(&messages), 0);
    }

    #[test]
    fn estimate_tokens_with_text() {
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello, world!".to_string(), // 13 chars = ~3 tokens
                cache_control: None,
            }],
        }];
        let tokens = estimate_tokens(&messages);
        assert!(tokens > 0 && tokens < 10);
    }

    #[test]
    fn estimate_tokens_counts_tool_round_thinking_across_turns() {
        // Per DeepSeek thinking-mode rules, any assistant message that
        // performed a tool call keeps its reasoning_content in the request
        // forever, including across new user turns. Token estimates must
        // count those bytes.
        let thinking = "reasoning ".repeat(800);
        let current_messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Use a tool".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        signature: None,
                        state: None,
                        thinking: thinking.clone(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path": "Cargo.toml"}),
                        caller: None,
                        thought_signature: None,
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: "manifest".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];
        let historical_messages = {
            let mut messages = current_messages.clone();
            messages.push(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "Done.".to_string(),
                    cache_control: None,
                }],
            });
            messages.push(Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Next question.".to_string(),
                    cache_control: None,
                }],
            });
            messages
        };
        let completed_messages = {
            let mut messages = current_messages.clone();
            messages.push(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "Done.".to_string(),
                    cache_control: None,
                }],
            });
            messages
        };

        let lower_bound = thinking.len() / 5;
        assert!(estimate_tokens(&current_messages) > lower_bound);
        assert!(estimate_tokens(&completed_messages) > lower_bound);
        assert!(estimate_tokens(&historical_messages) > lower_bound);
    }

    #[test]
    fn should_compact_respects_enabled_flag() {
        let config = CompactionConfig {
            enabled: false,
            ..Default::default()
        };
        // Even with many messages, disabled compaction should return false
        let messages: Vec<Message> = (0..100)
            .map(|_| Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "test".to_string(),
                    cache_control: None,
                }],
            })
            .collect();
        assert!(!should_compact(&messages, None, &prepared(&config)));
    }

    /// The #5577 acceptance case: a session whose provider bills 842K prompt
    /// tokens on a 1M window (threshold 800K) MUST compact even when the
    /// local estimate is far lower — the bounded working list undercounts
    /// what the provider actually saw, and billed truth wins.
    #[test]
    fn billed_842k_on_a_1m_window_compacts_despite_a_small_estimate() {
        let config = CompactionConfig {
            enabled: true,
            token_threshold: 800_000,
            ..Default::default()
        };
        let messages: Vec<Message> = (0..40)
            .map(|i| Message {
                role: if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                content: vec![ContentBlock::Text {
                    text: format!("short message {i}"),
                    cache_control: None,
                }],
            })
            .collect();
        // Estimate alone stays far under the trigger…
        assert!(!should_compact(&messages, None, &prepared(&config)));
        // …but the provider's billed prompt total decides.
        assert_eq!(
            compaction_decision_with_billed(&messages, None, &prepared(&config), Some(842_000)),
            CompactionDecision::Compact
        );
    }

    /// A refusal under real pressure must name its guard so the host can
    /// tell the user, instead of the silent hold that reads as a broken
    /// auto-compactor (#5577).
    #[test]
    fn refusals_under_pressure_name_their_guard() {
        let config = CompactionConfig {
            enabled: true,
            token_threshold: 100,
            ..Default::default()
        };
        // Too few messages to summarize: over-pressure, short transcript.
        let few: Vec<Message> = (0..3)
            .map(|i| Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: format!("message {i} {}", "x".repeat(300)),
                    cache_control: None,
                }],
            })
            .collect();
        assert_eq!(
            compaction_decision_with_billed(&few, None, &prepared(&config), None),
            CompactionDecision::Refused(CompactionRefusal::TooFewMessages { count: few.len() })
        );

        // Retained floor above the trigger: a giant system prompt no pass
        // can reclaim. The refusal carries the numbers the user needs.
        let many: Vec<Message> = (0..12)
            .map(|i| Message {
                role: if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                content: vec![ContentBlock::Text {
                    text: format!("message {i} {}", "y".repeat(200)),
                    cache_control: None,
                }],
            })
            .collect();
        let system = SystemPrompt::Text("s".repeat(4_000));
        match compaction_decision_with_billed(&many, Some(&system), &prepared(&config), None) {
            CompactionDecision::Refused(CompactionRefusal::RetainedFloor { floor, threshold }) => {
                assert_eq!(threshold, 100);
                assert!(floor >= threshold, "floor {floor} must be over {threshold}");
            }
            other => panic!("expected a retained-floor refusal, got {other:?}"),
        }
    }

    /// v0.8.11: message-count is no longer a compaction trigger. Long
    /// chats of small messages stay uncompacted because rewriting the
    /// prefix cache for a tiny budget reclaim is net-negative. Only token
    /// pressure (and the explicit `/compact` slash command) trigger
    /// compaction.
    #[test]
    fn message_count_no_longer_triggers_compaction() {
        let config = CompactionConfig {
            enabled: true,
            token_threshold: 1_000_000,
            ..Default::default()
        };

        // 200 tiny messages, well above the prior message threshold.
        let many_messages: Vec<Message> = (0..200)
            .map(|_| Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "x".to_string(),
                    cache_control: None,
                }],
            })
            .collect();
        // Token total stays minuscule so the token threshold is not hit;
        // without the prior message-count trigger, no compaction.
        assert!(!should_compact(&many_messages, None, &prepared(&config)));
    }

    // ========================================================================
    // Additional Compaction Trigger Tests
    // ========================================================================

    #[test]
    fn full_request_pressure_crosses_token_threshold() {
        let config = CompactionConfig {
            enabled: true,
            token_threshold: 20_000,
            ..Default::default()
        };

        // Create messages that exceed token threshold
        let messages: Vec<Message> = (0..20).map(|_| msg("user", &"x".repeat(5_000))).collect();

        assert!(compaction_pressure_reached(&messages, None, &config));
    }

    #[test]
    fn auto_compaction_uses_full_request_pressure_across_context_sizes() {
        for (window, output_reserve) in [
            (128_000_u64, 4_096_u64),
            (272_000, 4_096),
            // Large windows use the same ordinary request reservation; there
            // is no second, non-wire reasoning allowance.
            (1_000_000, 65_536),
        ] {
            let budget = crate::context_budget::ContextBudget::new(window, 0, output_reserve);
            let threshold = usize::try_from(budget.compaction_trigger_for_percent(80.0))
                .expect("test threshold fits usize");
            let raw_target = threshold.saturating_mul(7) / 10;
            let chars_per_message = raw_target.saturating_mul(4) / 14;
            let messages: Vec<Message> = (0..14)
                .map(|index| {
                    msg(
                        if index % 2 == 0 { "user" } else { "assistant" },
                        &"x".repeat(chars_per_message),
                    )
                })
                .collect();
            let raw = estimate_tokens(&messages);
            let full = estimate_input_tokens_for_pressure(&messages, None);
            let config = CompactionConfig {
                enabled: true,
                token_threshold: threshold,
                ..Default::default()
            };

            assert!(
                raw < threshold,
                "raw message estimator alone must not cross {window}"
            );
            // The pressure estimate adds per-message framing on top of the
            // raw message tokens; billed usage from the provider can also
            // cross the trigger on its own.
            assert!(
                full < threshold,
                "70%-filled fixture must stay under the {window} trigger: {full} >= {threshold}"
            );
            assert!(
                crate::compaction::compaction_pressure_reached_with_billed(
                    &messages,
                    None,
                    &config,
                    Some(threshold as u64),
                ),
                "billed prompt tokens at the trigger must reach pressure for {window}"
            );
            assert!(
                crate::compaction::should_compact_with_billed(
                    &messages,
                    None,
                    &prepared(&config),
                    Some(threshold as u64),
                ),
                "billed pressure must trigger eligibility for a {window}-token route"
            );
        }
    }

    #[test]
    fn auto_compaction_skips_pressure_that_cannot_be_reclaimed_below_trigger() {
        let messages: Vec<Message> = (0..20)
            .map(|index| {
                msg(
                    if index % 2 == 0 { "user" } else { "assistant" },
                    &"x".repeat(500),
                )
            })
            .collect();
        let system = SystemPrompt::Text("s".repeat(24_000));
        let config = CompactionConfig {
            enabled: true,
            token_threshold: 10_000,
            ..Default::default()
        };

        assert!(
            estimate_input_tokens_conservative(&messages, Some(&system)) >= config.token_threshold,
            "fixture must be under full-request pressure"
        );
        assert!(
            !should_compact(&messages, Some(&system), &prepared(&config)),
            "a pinned/system floor above the trigger would loop every tool step"
        );
    }

    #[test]
    fn full_request_threshold_is_inclusive() {
        let messages: Vec<Message> = (0..10)
            .map(|index| msg(if index % 2 == 0 { "user" } else { "assistant" }, "payload"))
            .collect();
        let threshold = estimate_input_tokens_for_pressure(&messages, None);
        let config = CompactionConfig {
            enabled: true,
            token_threshold: threshold,
            ..Default::default()
        };

        assert!(compaction_pressure_reached(&messages, None, &config));
    }

    #[test]
    fn test_should_compact_below_token_threshold() {
        let config = CompactionConfig {
            enabled: true,
            token_threshold: 1000,
            ..Default::default()
        };

        // Create short messages
        let messages: Vec<Message> = (0..5).map(|_| msg("user", "short")).collect();

        assert!(!should_compact(&messages, None, &prepared(&config)));
    }

    #[test]
    fn auto_compaction_uses_token_threshold_without_fixed_floor() {
        let config = CompactionConfig {
            enabled: true,
            token_threshold: 20_000,
            ..Default::default()
        };

        // Long sessions are dominated by assistant/tool output; the retained
        // user tail stays small, so the pass is reclaimable.
        let messages: Vec<Message> = (0..20)
            .map(|index| {
                if index % 2 == 0 {
                    msg("user", &"x".repeat(100))
                } else {
                    msg("assistant", &"x".repeat(10_000))
                }
            })
            .collect();
        assert!(should_compact(&messages, None, &prepared(&config)));
    }

    #[test]
    fn test_compaction_result_retries_used() {
        // This test verifies the CompactionResult structure
        let result = CompactionResult {
            messages: vec![],
            summary_prompt: None,
            retries_used: 2,
            coverage: CompactionCoverage::default(),
        };

        assert_eq!(result.retries_used, 2);
        assert!(result.messages.is_empty());
    }
}
