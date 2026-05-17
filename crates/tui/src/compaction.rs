//! Context compaction for long conversations.

use anyhow::Result;
use regex::Regex;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use crate::client::DeepSeekClient;
use crate::config::DEFAULT_TEXT_MODEL;
use crate::llm_client::LlmClient;
use crate::logging;
use crate::models::{
    CacheControl, ContentBlock, Message, MessageRequest, SystemBlock, SystemPrompt,
    context_window_for_model,
};

/// Configuration for conversation compaction behavior.
///
/// v0.8.11 simplified this from the prior token-OR-message-count trigger
/// to a token-only trigger gated by an absolute floor. The
/// `message_threshold` field was removed: its only purpose was to fire
/// compaction on long sessions of small messages, which is exactly the
/// case where rewriting the V4 prefix cache is least valuable. Token
/// budget is the right signal; message count was a 128K-era heuristic.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionConfig {
    pub enabled: bool,
    pub token_threshold: usize,
    pub model: String,
    pub cache_summary: bool,
    /// Hard floor — `should_compact` returns `false` when total session
    /// tokens fall below this number, regardless of `enabled` or
    /// `token_threshold`. Defaults to [`MINIMUM_AUTO_COMPACTION_TOKENS`]
    /// (500K) for v0.8.11+. Tests that want to exercise the threshold
    /// logic at small fixture sizes can set this to `0` to disable the
    /// floor.
    pub auto_floor_tokens: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            // ON BY DEFAULT since v0.8.6 (#402 P0 survivability) — but the
            // engine-level `auto_compact` setting was flipped OFF in v0.8.11
            // (#665) so this default is mostly a fallback for code paths
            // that build a `CompactionConfig` without going through
            // `compaction_threshold_for_model_and_effort`. Real per-model
            // values are still derived through that helper.
            enabled: true,
            // v0.8.11: 50K was a 128K-era leftover that biased every
            // unconfigured caller toward "compact almost immediately on V4."
            // Bumped to 800K (80% of V4's 1M window) so the dead-code
            // default matches the hard automatic compaction guardrail. This
            // is intentionally later than the model-visible 60% "suggest
            // /compact during sustained work" guidance; automatic replacement
            // compaction rewrites the cacheable prefix and remains opt-in.
            // Real call sites override this via
            // `compaction_threshold_for_model_and_effort`.
            token_threshold: 800_000,
            model: DEFAULT_TEXT_MODEL.to_string(),
            cache_summary: true,
            auto_floor_tokens: MINIMUM_AUTO_COMPACTION_TOKENS,
        }
    }
}

/// Hard floor for automatic compaction in v0.8.11+.
///
/// Below this token count, `should_compact` returns `false` regardless of
/// `enabled` or `token_threshold`. The point of the floor is V4 prefix-cache
/// economics: compaction rewrites the stable prefix, which destroys the KV
/// cache. At low token counts the prefix cache is healthy and compaction's
/// cost (full re-prefill at miss prices) dwarfs its benefit (a tiny budget
/// reclaim). Above the floor compaction can still be net-positive — cache
/// is already pressured, the prefix has drifted, and freeing budget matters.
///
/// Manual `/compact` slash command bypasses this floor with explicit user
/// agency.
///
/// Constant rather than configurable for v0.8.11. If anyone needs to dial
/// it (smaller models, opinionated workflows), we can add a setting later.
pub const MINIMUM_AUTO_COMPACTION_TOKENS: usize = 500_000;

pub const KEEP_RECENT_MESSAGES: usize = 4;
const RECENT_WORKING_SET_WINDOW: usize = 12;
const MAX_WORKING_SET_PATHS: usize = 24;
const MIN_SUMMARIZE_MESSAGES: usize = 6;
const SUMMARY_TEXT_SNIPPET_CHARS: usize = 800;
const SUMMARY_TOOL_RESULT_SNIPPET_CHARS: usize = 240;
const SUMMARY_INPUT_MAX_CHARS: usize = 24_000;
const SUMMARY_INPUT_HEAD_CHARS: usize = 14_000;
const SUMMARY_INPUT_TAIL_CHARS: usize = 6_000;
const LARGE_CONTEXT_SUMMARY_TEXT_SNIPPET_CHARS: usize = 2_000;
const LARGE_CONTEXT_SUMMARY_TOOL_RESULT_SNIPPET_CHARS: usize = 4_000;
const LARGE_CONTEXT_SUMMARY_INPUT_MAX_CHARS: usize = 120_000;
const LARGE_CONTEXT_SUMMARY_INPUT_HEAD_CHARS: usize = 72_000;
const LARGE_CONTEXT_SUMMARY_INPUT_TAIL_CHARS: usize = 36_000;
const TOOL_PRUNE_STOP_CHECK_BYTES: usize = 16 * 1024;
const LARGE_CONTEXT_SUMMARY_MAX_TOKENS: u32 = 2_048;
const LARGE_CONTEXT_WINDOW_TOKENS: u32 = 500_000;
const CACHE_ALIGNED_SUMMARY_CONTEXT_BUDGET_PERCENT: usize = 85;

#[derive(Debug, Clone, Copy)]
struct SummaryInputLimits {
    text_snippet_chars: usize,
    tool_result_snippet_chars: usize,
    input_max_chars: usize,
    input_head_chars: usize,
    input_tail_chars: usize,
    max_tokens: u32,
    word_limit: usize,
}

fn summary_input_limits_for_model(model: &str) -> SummaryInputLimits {
    let is_large_context =
        context_window_for_model(model).is_some_and(|window| window >= LARGE_CONTEXT_WINDOW_TOKENS);
    if is_large_context {
        SummaryInputLimits {
            text_snippet_chars: LARGE_CONTEXT_SUMMARY_TEXT_SNIPPET_CHARS,
            tool_result_snippet_chars: LARGE_CONTEXT_SUMMARY_TOOL_RESULT_SNIPPET_CHARS,
            input_max_chars: LARGE_CONTEXT_SUMMARY_INPUT_MAX_CHARS,
            input_head_chars: LARGE_CONTEXT_SUMMARY_INPUT_HEAD_CHARS,
            input_tail_chars: LARGE_CONTEXT_SUMMARY_INPUT_TAIL_CHARS,
            max_tokens: LARGE_CONTEXT_SUMMARY_MAX_TOKENS,
            word_limit: 900,
        }
    } else {
        SummaryInputLimits {
            text_snippet_chars: SUMMARY_TEXT_SNIPPET_CHARS,
            tool_result_snippet_chars: SUMMARY_TOOL_RESULT_SNIPPET_CHARS,
            input_max_chars: SUMMARY_INPUT_MAX_CHARS,
            input_head_chars: SUMMARY_INPUT_HEAD_CHARS,
            input_tail_chars: SUMMARY_INPUT_TAIL_CHARS,
            max_tokens: 1_024,
            word_limit: 500,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CompactionPlan {
    pub pinned_indices: BTreeSet<usize>,
    pub summarize_indices: Vec<usize>,
}

fn path_regex() -> &'static Regex {
    static PATH_RE: OnceLock<Regex> = OnceLock::new();
    PATH_RE.get_or_init(|| {
        Regex::new(
            r"(?x)
            (?:
                (?P<root>
                    Cargo\.toml|
                    Cargo\.lock|
                    README\.md|
                    CHANGELOG\.md|
                    AGENTS\.md|
                    config\.example\.toml
                )
            )
            |
            (?P<path>
                (?:[A-Za-z0-9._-]+/)+
                [A-Za-z0-9._-]+
                \.(?:rs|toml|md|json|ya?ml|txt|lock)
            )
        ",
        )
        .expect("path regex is valid")
    })
}

fn normalize_path_candidate(candidate: &str, workspace: Option<&Path>) -> Option<String> {
    if candidate.is_empty() {
        return None;
    }

    let cleaned = candidate.replace('\\', "/");
    let mut path = PathBuf::from(cleaned);

    if path.is_absolute() {
        let ws = workspace?;
        if let Ok(stripped) = path.strip_prefix(ws) {
            path = stripped.to_path_buf();
        } else {
            return None;
        }
    }

    let rel = path.to_string_lossy().trim_start_matches("./").to_string();
    if rel.is_empty() || rel.contains("..") {
        return None;
    }

    if let Some(ws) = workspace {
        let repo_path = ws.join(&rel);
        if repo_path.exists() || looks_repo_relative(&rel) {
            return Some(rel);
        }
        return None;
    }

    if looks_repo_relative(&rel) {
        return Some(rel);
    }

    None
}

fn looks_repo_relative(path: &str) -> bool {
    matches!(
        path,
        "Cargo.toml"
            | "Cargo.lock"
            | "README.md"
            | "CHANGELOG.md"
            | "AGENTS.md"
            | "config.example.toml"
    ) || path.starts_with("src/")
        || path.starts_with("tests/")
        || path.starts_with("docs/")
        || path.starts_with("examples/")
        || path.starts_with("benches/")
        || path.starts_with("crates/")
        || path.starts_with(".github/")
        || (path.contains('/') && path.rsplit('.').next().is_some())
}

fn extract_paths_from_text(text: &str, workspace: Option<&Path>) -> Vec<String> {
    path_regex()
        .captures_iter(text)
        .filter_map(|caps| {
            let candidate = caps
                .name("path")
                .or_else(|| caps.name("root"))
                .map(|m| m.as_str())?;
            normalize_path_candidate(candidate, workspace)
        })
        .collect()
}

fn extract_paths_from_tool_input(
    input: &serde_json::Value,
    workspace: Option<&Path>,
) -> Vec<String> {
    let mut out = Vec::new();
    let Some(obj) = input.as_object() else {
        return out;
    };

    for key in ["path", "file", "target", "cwd"] {
        if let Some(val) = obj.get(key).and_then(serde_json::Value::as_str)
            && let Some(path) = normalize_path_candidate(val, workspace)
        {
            out.push(path);
        }
    }

    for key in ["paths", "files", "targets"] {
        if let Some(vals) = obj.get(key).and_then(serde_json::Value::as_array) {
            for val in vals {
                if let Some(s) = val.as_str()
                    && let Some(path) = normalize_path_candidate(s, workspace)
                {
                    out.push(path);
                }
            }
        }
    }

    out
}

fn message_text(msg: &Message) -> String {
    let mut text = String::new();
    for block in &msg.content {
        match block {
            ContentBlock::Text { text: t, .. } => {
                let _ = writeln!(text, "{t}");
            }
            ContentBlock::Thinking { .. } => {}
            ContentBlock::ToolUse { name, input, .. } => {
                let _ = writeln!(text, "[tool_use:{name}] {input}");
            }
            ContentBlock::ToolResult { content, .. } => {
                let _ = writeln!(text, "{content}");
            }
            ContentBlock::ServerToolUse { .. }
            | ContentBlock::ToolSearchToolResult { .. }
            | ContentBlock::CodeExecutionToolResult { .. } => {}
        }
    }
    text
}

fn is_user_text_query(msg: &Message) -> bool {
    msg.role == "user"
        && msg
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::Text { .. }))
}

fn extract_paths_from_message(message: &Message, workspace: Option<&Path>) -> Vec<String> {
    let mut paths = Vec::new();
    for block in &message.content {
        let candidates = match block {
            ContentBlock::Text { text, .. } => extract_paths_from_text(text, workspace),
            ContentBlock::ToolResult { content, .. } => extract_paths_from_text(content, workspace),
            ContentBlock::ToolUse { input, .. } => extract_paths_from_tool_input(input, workspace),
            ContentBlock::Thinking { .. } => Vec::new(),
            ContentBlock::ServerToolUse { .. }
            | ContentBlock::ToolSearchToolResult { .. }
            | ContentBlock::CodeExecutionToolResult { .. } => Vec::new(),
        };
        paths.extend(candidates);
    }
    paths
}

fn derive_working_set_paths(
    messages: &[Message],
    workspace: Option<&Path>,
    seed_indices: &[usize],
) -> HashSet<String> {
    let mut paths: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let mut seeds: Vec<usize> = seed_indices
        .iter()
        .copied()
        .filter(|idx| *idx < messages.len())
        .collect();
    seeds.sort_unstable_by(|a, b| b.cmp(a));

    for idx in seeds {
        for candidate in extract_paths_from_message(&messages[idx], workspace) {
            if seen.insert(candidate.clone()) {
                paths.push(candidate);
                if paths.len() >= MAX_WORKING_SET_PATHS {
                    return paths.into_iter().collect();
                }
            }
        }
    }

    for msg in messages.iter().rev().take(RECENT_WORKING_SET_WINDOW) {
        for candidate in extract_paths_from_message(msg, workspace) {
            if seen.insert(candidate.clone()) {
                paths.push(candidate);
                if paths.len() >= MAX_WORKING_SET_PATHS {
                    return paths.into_iter().collect();
                }
            }
        }
    }

    paths.into_iter().collect()
}

fn should_pin_message(text: &str, working_set_paths: &HashSet<String>) -> bool {
    let lower = text.to_lowercase();

    let mentions_working_set = working_set_paths.iter().any(|p| text.contains(p));
    if mentions_working_set {
        return true;
    }

    let error_markers = [
        "error:",
        "error ",
        "failed",
        "panic",
        "traceback",
        "stack trace",
        "assertion failed",
        "test failed",
    ];
    if error_markers.iter().any(|m| lower.contains(m)) {
        return true;
    }

    let patch_markers = [
        "diff --git",
        "+++ b/",
        "--- a/",
        "*** begin patch",
        "*** update file:",
        "*** add file:",
        "*** delete file:",
        "```diff",
        "apply_patch",
    ];
    patch_markers.iter().any(|m| lower.contains(m))
}

pub fn plan_compaction(
    messages: &[Message],
    workspace: Option<&Path>,
    keep_recent: usize,
    external_pins: Option<&[usize]>,
    external_working_set_paths: Option<&[String]>,
) -> CompactionPlan {
    let mut pinned_indices: BTreeSet<usize> = BTreeSet::new();
    let len = messages.len();
    if len == 0 {
        return CompactionPlan::default();
    }

    // Always pin the tail of the conversation to preserve immediate context.
    let recent_start = len.saturating_sub(keep_recent);
    pinned_indices.extend(recent_start..len);

    // Derive a repo-aware working set from recent messages/tool calls and
    // merge it with any externally provided working-set paths.
    let seed_indices = external_pins.unwrap_or(&[]);
    let mut working_set_paths = derive_working_set_paths(messages, workspace, seed_indices);
    if let Some(paths) = external_working_set_paths {
        for path in paths {
            if let Some(normalized) = normalize_path_candidate(path, workspace) {
                let _ = working_set_paths.insert(normalized);
            }
        }
    }

    for (idx, msg) in messages.iter().enumerate() {
        if pinned_indices.contains(&idx) {
            continue;
        }
        let text = message_text(msg);
        if should_pin_message(&text, &working_set_paths) {
            pinned_indices.insert(idx);
        }
    }

    // External pins are authoritative and should be preserved even if they
    // were not detected by the heuristics above.
    if let Some(pins) = external_pins {
        pinned_indices.extend(pins.iter().copied().filter(|idx| *idx < len));
    }

    // Ensure tool result messages are not kept without their corresponding tool call.
    enforce_tool_call_pairs(messages, &mut pinned_indices);

    // Some OpenAI-compatible chat templates require at least one user text
    // message. Tool-heavy tails can otherwise compact down to only tool calls
    // and tool results, which makes those backends reject the next request.
    if !pinned_indices
        .iter()
        .any(|&idx| is_user_text_query(&messages[idx]))
        && let Some(idx) = messages
            .iter()
            .enumerate()
            .rev()
            .find_map(|(idx, msg)| is_user_text_query(msg).then_some(idx))
    {
        pinned_indices.insert(idx);
    }

    let summarize_indices = (0..len)
        .filter(|idx| !pinned_indices.contains(idx))
        .collect();

    // `working_set_paths` was used only for pinning decisions above.
    drop(working_set_paths);

    CompactionPlan {
        pinned_indices,
        summarize_indices,
    }
}

fn enforce_tool_call_pairs(messages: &[Message], pinned_indices: &mut BTreeSet<usize>) {
    if pinned_indices.is_empty() {
        return;
    }

    // Build maps: tool_id → message index across ALL messages (not just pinned).
    let mut call_id_to_idx: HashMap<String, usize> = HashMap::new();
    let mut result_id_to_idx: HashMap<String, usize> = HashMap::new();

    for (idx, msg) in messages.iter().enumerate() {
        for block in &msg.content {
            match block {
                ContentBlock::ToolUse { id, .. } => {
                    call_id_to_idx.insert(id.clone(), idx);
                }
                ContentBlock::ToolResult { tool_use_id, .. } => {
                    result_id_to_idx.insert(tool_use_id.clone(), idx);
                }
                _ => {}
            }
        }
    }

    // Fixpoint loop: re-check until stable.
    // Newly pinned messages may introduce new pair requirements;
    // removed messages may orphan their counterparts.
    // Track permanently removed indices so they cannot be re-added
    // by a counterpart in a later iteration (prevents oscillation).
    let mut permanently_removed: HashSet<usize> = HashSet::new();

    let max_iters = messages.len().max(10);
    let mut converged = false;
    for _ in 0..max_iters {
        let mut to_add = Vec::new();
        let mut to_remove = Vec::new();

        let snapshot: Vec<usize> = pinned_indices.iter().copied().collect();

        for idx in snapshot {
            let msg = &messages[idx];
            for block in &msg.content {
                match block {
                    // Pinned result → its call must also be pinned (or remove result)
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        match call_id_to_idx.get(tool_use_id) {
                            Some(&call_idx) if !permanently_removed.contains(&call_idx) => {
                                to_add.push(call_idx);
                            }
                            _ => {
                                to_remove.push(idx);
                            }
                        }
                    }
                    // Pinned call → its result must also be pinned (or remove call)
                    ContentBlock::ToolUse { id, .. } => match result_id_to_idx.get(id) {
                        Some(&result_idx) if !permanently_removed.contains(&result_idx) => {
                            to_add.push(result_idx);
                        }
                        _ => {
                            to_remove.push(idx);
                        }
                    },
                    _ => {}
                }
            }
        }

        // Removals take priority: if a message is both needed and orphaned,
        // remove it now; the fixpoint loop will cascade the orphaning.
        let remove_set: HashSet<usize> = to_remove.iter().copied().collect();
        let mut changed = false;
        for idx in to_add {
            if !remove_set.contains(&idx) && pinned_indices.insert(idx) {
                changed = true;
            }
        }
        for idx in to_remove {
            if pinned_indices.remove(&idx) {
                permanently_removed.insert(idx);
                changed = true;
            }
        }

        if !changed {
            converged = true;
            break;
        }
    }
    if !converged {
        logging::warn(format!(
            "enforce_tool_call_pairs did not converge after {max_iters} iterations \
             ({} messages, {} pinned)",
            messages.len(),
            pinned_indices.len()
        ));
    }
}

fn estimate_tokens_for_message(message: &Message, include_thinking: bool) -> usize {
    message
        .content
        .iter()
        .map(|c| match c {
            ContentBlock::Text { text, .. } => text.len() / 4,
            // Historical reasoning blocks are UI/session metadata for DeepSeek.
            // Only current-turn tool-call reasoning is sent back to the API.
            ContentBlock::Thinking { thinking } if include_thinking => thinking.len() / 4,
            ContentBlock::Thinking { .. } => 0,
            ContentBlock::ToolUse { input, .. } => serde_json::to_string(input)
                .map(|s| s.len() / 4)
                .unwrap_or(100),
            ContentBlock::ToolResult { content, .. } => content.len() / 4,
            ContentBlock::ServerToolUse { .. }
            | ContentBlock::ToolSearchToolResult { .. }
            | ContentBlock::CodeExecutionToolResult { .. } => 0,
        })
        .sum::<usize>()
}

pub fn estimate_tokens(messages: &[Message]) -> usize {
    // Rough estimate: ~4 chars per token. DeepSeek thinking-mode rule: any
    // assistant message with tool_calls keeps its reasoning_content forever
    // (replayed in all subsequent requests). Final text-only answers drop it.
    messages
        .iter()
        .map(|message| estimate_tokens_for_message(message, message_has_tool_use(message)))
        .sum()
}

fn message_has_tool_use(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
}

fn estimate_text_tokens_conservative(text: &str) -> usize {
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

pub fn should_compact(
    messages: &[Message],
    config: &CompactionConfig,
    workspace: Option<&Path>,
    external_pins: Option<&[usize]>,
    external_working_set_paths: Option<&[String]>,
) -> bool {
    if !config.enabled {
        return false;
    }

    // v0.8.11: hard floor enforcement. Below the floor (default 500K tokens
    // — see `MINIMUM_AUTO_COMPACTION_TOKENS`), automatic compaction is
    // refused because rewriting the prefix kills V4's prefix cache for
    // little budget recovery. Manual `/compact` and the `compact_now` tool
    // bypass this floor by going through different code paths.
    if config.auto_floor_tokens > 0 {
        let total_session_tokens: usize = messages
            .iter()
            .map(|m| estimate_tokens_for_message(m, false))
            .sum();
        if total_session_tokens < config.auto_floor_tokens {
            return false;
        }
    }

    let plan = plan_compaction(
        messages,
        workspace,
        KEEP_RECENT_MESSAGES,
        external_pins,
        external_working_set_paths,
    );
    let pinned_tokens: usize = plan
        .pinned_indices
        .iter()
        .map(|&idx| estimate_tokens_for_message(&messages[idx], false))
        .sum();

    let token_estimate: usize = plan
        .summarize_indices
        .iter()
        .map(|&idx| estimate_tokens_for_message(&messages[idx], false))
        .sum();
    let message_count = plan.summarize_indices.len();

    // Pinned messages consume part of the budget, so compact earlier when needed.
    let effective_token_threshold = config.token_threshold.saturating_sub(pinned_tokens);

    // Token-only trigger (v0.8.11): the prior message-count branch was a
    // 128K-era heuristic that fired compaction on long chats of small
    // messages — exactly the case where rewriting the V4 prefix cache is
    // most wasteful. Token budget is the only signal that maps to actual
    // model context pressure.
    if effective_token_threshold == 0 {
        return message_count >= MIN_SUMMARIZE_MESSAGES;
    }
    if message_count < MIN_SUMMARIZE_MESSAGES {
        return false;
    }
    token_estimate > effective_token_threshold
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
    let raw = serde_json::to_string(input).unwrap_or_else(|_| input.to_string());
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
    let cutoff = messages.len().saturating_sub(protected_window);
    if cutoff == 0 {
        return 0;
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
                original_len: content.len(),
            });
        }
    }

    // The maps above are fully populated before pruning starts, so the order below
    // only changes which message bytes are rewritten first. Pruning from newest to
    // oldest lets callers stop as soon as enough bytes were saved, preserving the
    // earlier JSON request prefix for byte-level KV caches.
    candidates.reverse();

    let mut bytes_saved = 0usize;
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

        if let ContentBlock::ToolResult {
            content,
            content_blocks,
            ..
        } = &mut messages[candidate.message_idx].content[candidate.block_idx]
        {
            bytes_saved = bytes_saved.saturating_add(content.len().saturating_sub(summary.len()));
            *content = summary;
            *content_blocks = None;

            if should_stop(messages, bytes_saved) {
                break;
            }
        }
    }

    bytes_saved
}

/// Result of a compaction operation with metadata.
#[derive(Debug)]
pub struct CompactionResult {
    /// Compacted messages
    pub messages: Vec<Message>,
    /// Summary system prompt
    pub summary_prompt: Option<SystemPrompt>,
    /// Messages that were removed from the active window
    #[allow(dead_code)]
    pub removed_messages: Vec<Message>,
    /// Number of retries used before success
    pub retries_used: u32,
}

/// Check if an error is transient and worth retrying. Categories that map to
/// transient retry: Network, RateLimit, Timeout. Anything else (auth, parse,
/// invalid request, etc.) is permanent and propagates.
fn is_transient_error(e: &anyhow::Error) -> bool {
    let category = crate::error_taxonomy::classify_error_message(&e.to_string());
    matches!(
        category,
        crate::error_taxonomy::ErrorCategory::Network
            | crate::error_taxonomy::ErrorCategory::RateLimit
            | crate::error_taxonomy::ErrorCategory::Timeout
    )
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
    client: &DeepSeekClient,
    messages: &[Message],
    config: &CompactionConfig,
    workspace: Option<&Path>,
    external_pins: Option<&[usize]>,
    external_working_set_paths: Option<&[String]>,
) -> Result<CompactionResult> {
    const MAX_RETRIES: u32 = 3;
    const BASE_DELAY_MS: u64 = 1000;

    let was_over_threshold = should_compact(
        messages,
        config,
        workspace,
        external_pins,
        external_working_set_paths,
    );
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
            now_under_threshold = !should_compact(
                candidate_messages,
                config,
                workspace,
                external_pins,
                external_working_set_paths,
            );
            now_under_threshold
        },
    );
    if was_over_threshold && pruned_bytes > 0 && !now_under_threshold {
        // The throttled in-loop check may skip the exact candidate that clears the
        // budget. Do one final pass so a successful local prune still avoids LLM compaction.
        now_under_threshold = !should_compact(
            &pruned_messages,
            config,
            workspace,
            external_pins,
            external_working_set_paths,
        );
    }

    let compaction_input: &[Message] = if pruned_bytes > 0 {
        logging::info(format!(
            "Local tool-result prune saved {pruned_bytes} bytes before LLM compaction"
        ));
        if was_over_threshold && now_under_threshold {
            return Ok(CompactionResult {
                messages: pruned_messages,
                summary_prompt: None,
                removed_messages: Vec::new(),
                retries_used: 0,
            });
        }
        &pruned_messages
    } else {
        messages
    };

    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            // Exponential backoff: 1s, 2s, 4s
            let delay = Duration::from_millis(BASE_DELAY_MS * (1 << (attempt - 1)));
            tokio::time::sleep(delay).await;
        }

        match compact_messages(
            client,
            compaction_input,
            config,
            workspace,
            external_pins,
            external_working_set_paths,
        )
        .await
        {
            Ok((msgs, prompt, removed)) => {
                return Ok(CompactionResult {
                    messages: msgs,
                    summary_prompt: prompt,
                    removed_messages: removed,
                    retries_used: attempt,
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

fn read_workspace_anchors(workspace: Option<&Path>) -> Vec<String> {
    let Some(ws) = workspace else {
        return Vec::new();
    };

    let anchors_path = ws.join(".deepseek").join("anchors.md");
    let Ok(content) = std::fs::read_to_string(anchors_path) else {
        return Vec::new();
    };

    content
        .split("\n---\n")
        .map(str::trim)
        .filter(|anchor| !anchor.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn anchor_summary_section(workspace: Option<&Path>) -> String {
    let anchors = read_workspace_anchors(workspace);
    if anchors.is_empty() {
        return String::new();
    }

    let mut section = String::from(
        "## Pinned Facts (User Anchors)\n\n\
         The following facts were explicitly anchored by the user with `/anchor`. \
         Preserve them across compaction cycles.\n\n",
    );

    for anchor in anchors {
        let _ = writeln!(section, "- {anchor}");
    }

    section.push_str("\n---\n\n");
    section
}

pub async fn compact_messages(
    client: &DeepSeekClient,
    messages: &[Message],
    config: &CompactionConfig,
    workspace: Option<&Path>,
    external_pins: Option<&[usize]>,
    external_working_set_paths: Option<&[String]>,
) -> Result<(Vec<Message>, Option<SystemPrompt>, Vec<Message>)> {
    if messages.is_empty() {
        return Ok((Vec::new(), None, Vec::new()));
    }

    let plan = plan_compaction(
        messages,
        workspace,
        KEEP_RECENT_MESSAGES,
        external_pins,
        external_working_set_paths,
    );
    if plan.summarize_indices.is_empty() {
        return Ok((messages.to_vec(), None, Vec::new()));
    }

    let to_summarize: Vec<Message> = plan
        .summarize_indices
        .iter()
        .map(|&idx| messages[idx].clone())
        .collect();

    // Create a summary of the unpinned portion of the conversation
    let summary = create_summary(client, &to_summarize, &config.model).await?;

    // Extract workflow context (files touched, tasks in progress, etc.)
    let workflow_context = extract_workflow_context(&to_summarize, workspace);

    let anchors_section = anchor_summary_section(workspace);

    // Build new message list with enhanced summary as system block
    let summary_block = SystemBlock {
        block_type: "text".to_string(),
        text: format!(
            "{anchors_section}\
             ## 📋 Conversation Summary (Auto-Generated)\n\n\
             {summary}\n\n\
             ---\n\n\
             ## 🔍 Workflow Context\n\n\
             {workflow_context}\n\n\
             ---\n\n\
             ## 💡 What to Do Next\n\n\
             You have just resumed from a context compaction. The conversation above was summarized to save space. \
             Review the summary and workflow context, then continue helping the user with their task. \
             If you need more details about the summarized portion, ask the user to clarify.\n\n\
             ---\n\n\
             Pinned messages follow:"
        ),
        cache_control: if config.cache_summary {
            Some(CacheControl {
                cache_type: "ephemeral".to_string(),
            })
        } else {
            None
        },
    };

    let pinned_messages = messages
        .iter()
        .enumerate()
        .filter_map(|(idx, msg)| plan.pinned_indices.contains(&idx).then_some(msg.clone()))
        .collect();

    Ok((
        pinned_messages,
        Some(SystemPrompt::Blocks(vec![summary_block])),
        to_summarize,
    ))
}

async fn create_summary(
    client: &DeepSeekClient,
    messages: &[Message],
    model: &str,
) -> Result<String> {
    let limits = summary_input_limits_for_model(model);
    let used_cache_aligned = should_use_cache_aligned_summary(model, messages);
    let request = if used_cache_aligned {
        build_cache_aligned_summary_request(model, messages, limits)
    } else {
        build_formatted_summary_request(model, messages, limits)
    };

    let response = client.create_message(request).await?;
    // Compaction summary calls are billed by DeepSeek; route the
    // tokens through the side-channel so the dashboard total
    // matches the website (#526).
    crate::cost_status::report(&response.model, &response.usage);

    // #584: emit one debug-level event per summary call so the
    // V4 cache-aligned win is observable post-deploy without
    // adding UI surface. The event is emitted with
    // `target = "compaction"`, so the filter is
    // `RUST_LOG=compaction=debug` (the module-path form
    // `deepseek_tui::compaction=debug` does NOT match — `EnvFilter`
    // matches the explicit target string when one is set).
    log_summary_cache_telemetry(used_cache_aligned, &response.usage);

    // Extract text from response
    let summary = response
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    Ok(summary)
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
fn summary_cache_hit_percent(cache_hit: u32, input_tokens: u32) -> f64 {
    if input_tokens > 0 {
        (f64::from(cache_hit) * 100.0) / f64::from(input_tokens)
    } else {
        0.0
    }
}

/// Emit one `tracing::debug!` event per compaction summary call so the
/// path choice (cache-aligned vs fallback) and the resulting cache-hit
/// rate are observable. Both raw token counts and the percentage are
/// included; on providers that don't return cache-token fields the
/// counts are reported as `0` and the percentage as `0.0`. (#584)
fn log_summary_cache_telemetry(used_cache_aligned: bool, usage: &crate::models::Usage) {
    let path = if used_cache_aligned {
        "cache_aligned"
    } else {
        "fallback"
    };
    let cache_hit = usage.prompt_cache_hit_tokens.unwrap_or(0);
    let cache_miss = usage.prompt_cache_miss_tokens.unwrap_or(0);
    let cache_hit_pct = summary_cache_hit_percent(cache_hit, usage.input_tokens);
    tracing::debug!(
        target: "compaction",
        "compaction summary call: path={} prompt_tokens={} cache_hit_tokens={} cache_miss_tokens={} cache_hit_pct={:.1}",
        path,
        usage.input_tokens,
        cache_hit,
        cache_miss,
        cache_hit_pct,
    );
}

/// Decide whether to use the cache-aligned summary path
/// ([`build_cache_aligned_summary_request`]) or the fallback
/// ([`build_formatted_summary_request`]). Returns `true` when both
/// gates hold:
///
/// 1. The model has a known large context window
///    (≥ `LARGE_CONTEXT_WINDOW_TOKENS`, currently V4-scale).
/// 2. Replaying the message prefix plus a ~512-token instruction
///    still fits within `CACHE_ALIGNED_SUMMARY_CONTEXT_BUDGET_PERCENT`
///    of that budget.
///
/// ## Why the two paths produce slightly different prompts (#584)
///
/// The two summary requests are *intentionally* framed differently:
///
/// - **Cache-aligned** replays the original `messages` verbatim
///   with `system: None` and appends the summary instruction as
///   the final `user` turn. The model sees the conversation as if
///   it were its own history. This is what lets the V4 prefix cache
///   hit on the bulk of the request (#572).
/// - **Fallback** reformats the conversation into a flat
///   `User:/Assistant:` transcript inside a single `user` message
///   and adds a "You are a helpful assistant that creates concise
///   conversation summaries." system prompt. The model sees a
///   transcript of someone else's conversation.
///
/// The empirical bar is that V4 produces equivalent summaries
/// either way; the post-#572 review noted this fork is worth
/// documenting but not yet worth unifying. The fallback's
/// external-transcript framing is also more conservative for the
/// older / smaller models the cache-aligned path explicitly
/// excludes, so dropping the system prompt would risk regressing
/// those models without a corresponding gain. If we ever want to
/// unify, land it in a separate PR backed by an A/B summary-quality
/// evaluation rather than as a drive-by cleanup.
///
/// `create_summary` emits a `tracing::debug!` event under
/// `target = "compaction"` after each call so the path choice and
/// cache-hit rate are observable post-deploy without UI surface.
fn should_use_cache_aligned_summary(model: &str, messages: &[Message]) -> bool {
    let Some(window) = context_window_for_model(model) else {
        return false;
    };
    if window < LARGE_CONTEXT_WINDOW_TOKENS {
        return false;
    }

    let budget = usize::try_from(window).unwrap_or(usize::MAX)
        * CACHE_ALIGNED_SUMMARY_CONTEXT_BUDGET_PERCENT
        / 100;
    let summary_prompt_tokens = 512usize;
    estimate_tokens(messages).saturating_add(summary_prompt_tokens) <= budget
}

fn summary_instruction(word_limit: usize) -> String {
    format!(
        "Summarize the conversation above in a concise but comprehensive way. \
         Preserve key information, decisions made, exact file paths, commands, \
         errors, and tool-result facts needed to continue the work. \
         Tool outputs may be abbreviated only when they are repetitive. \
         Keep it under {word_limit} words."
    )
}

fn build_cache_aligned_summary_request(
    model: &str,
    messages: &[Message],
    limits: SummaryInputLimits,
) -> MessageRequest {
    let mut request_messages = messages.to_vec();
    request_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: summary_instruction(limits.word_limit),
            cache_control: None,
        }],
    });

    MessageRequest {
        model: model.to_string(),
        messages: request_messages,
        max_tokens: limits.max_tokens,
        system: None,
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort: None,
        stream: Some(false),
        temperature: Some(0.3),
        top_p: None,
    }
}

fn build_formatted_summary_request(
    model: &str,
    messages: &[Message],
    limits: SummaryInputLimits,
) -> MessageRequest {
    // Format messages for summarization
    let mut conversation_text = String::new();
    for msg in messages {
        let role = if msg.role == "user" {
            "User"
        } else {
            "Assistant"
        };
        for block in &msg.content {
            match block {
                ContentBlock::Text { text, .. } => {
                    let snippet = truncate_chars(text, limits.text_snippet_chars);
                    let _ = write!(conversation_text, "{role}: {snippet}\n\n");
                }
                ContentBlock::ToolUse { name, .. } => {
                    let _ = write!(conversation_text, "{role}: [Used tool: {name}]\n\n");
                }
                ContentBlock::ToolResult { content, .. } => {
                    let snippet = truncate_chars(content, limits.tool_result_snippet_chars);
                    let _ = write!(conversation_text, "Tool result: {}\n\n", snippet);
                }
                ContentBlock::Thinking { .. } => {
                    // Skip thinking blocks in summary
                }
                ContentBlock::ServerToolUse { .. }
                | ContentBlock::ToolSearchToolResult { .. }
                | ContentBlock::CodeExecutionToolResult { .. } => {}
            }
        }
    }

    let conversation_chars = conversation_text.chars().count();
    if conversation_chars > limits.input_max_chars {
        let head = truncate_chars(&conversation_text, limits.input_head_chars).to_string();
        let tail = tail_chars(&conversation_text, limits.input_tail_chars);
        let omitted = conversation_chars
            .saturating_sub(head.chars().count())
            .saturating_sub(tail.chars().count());
        conversation_text =
            format!("{head}\n\n[... {omitted} characters omitted before summary ...]\n\n{tail}");
    }

    MessageRequest {
        model: model.to_string(),
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: format!(
                    "{}\n\n---\n\n{conversation_text}",
                    summary_instruction(limits.word_limit)
                ),
                cache_control: None,
            }],
        }],
        max_tokens: limits.max_tokens,
        system: Some(SystemPrompt::Text(
            "You are a helpful assistant that creates concise conversation summaries.".to_string(),
        )),
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort: None,
        stream: Some(false),
        temperature: Some(0.3),
        top_p: None,
    }
}

/// Extract workflow context from messages (files touched, tasks, etc.)
fn extract_workflow_context(messages: &[Message], workspace: Option<&Path>) -> String {
    let mut files_touched: Vec<String> = Vec::new();
    let mut tools_used: Vec<String> = Vec::new();
    let mut tasks_identified: Vec<String> = Vec::new();

    for msg in messages {
        for block in &msg.content {
            match block {
                ContentBlock::ToolUse { name, input, .. } => {
                    tools_used.push(name.clone());

                    // Extract file paths from tool inputs
                    if let Some(path) = extract_path_from_input(input)
                        && !files_touched.contains(&path)
                    {
                        files_touched.push(path);
                    }
                }
                ContentBlock::Text { text, .. }
                    // Look for task/todo mentions
                    if (text.contains("TODO") || text.contains("task") || text.contains("need to")) => {
                        let task = truncate_chars(text, 200).to_string();
                        if !tasks_identified.contains(&task) {
                            tasks_identified.push(task);
                        }
                    }
                _ => {}
            }
        }
    }

    let mut context = String::new();

    if !files_touched.is_empty() {
        context.push_str("**Files Modified/Read:**\n");
        for file in &files_touched {
            if let Some(ws) = workspace {
                let relative = Path::new(file)
                    .strip_prefix(ws)
                    .unwrap_or(Path::new(file))
                    .display();
                context.push_str(&format!("- `{}`\n", relative));
            } else {
                context.push_str(&format!("- `{}`\n", file));
            }
        }
        context.push('\n');
    }

    if !tools_used.is_empty() {
        context.push_str("**Tools Used:** ");
        context.push_str(&tools_used.join(", "));
        context.push_str("\n\n");
    }

    if !tasks_identified.is_empty() {
        context.push_str("**Tasks/TODOs Identified:**\n");
        for task in &tasks_identified {
            context.push_str(&format!("- {}\n", task));
        }
        context.push('\n');
    }

    if context.is_empty() {
        context.push_str("No specific workflow context detected. Continue assisting the user with their current task.\n");
    }

    context
}

/// Extract file path from tool input JSON
fn extract_path_from_input(input: &serde_json::Value) -> Option<String> {
    // Try common path field names
    for key in ["path", "file", "file_path", "filename"] {
        if let Some(path) = input.get(key).and_then(|v| v.as_str()) {
            return Some(path.to_string());
        }
    }

    // Try to find path in nested objects
    if let Some(obj) = input.as_object() {
        for (_, value) in obj {
            if let Some(path) = value.as_str()
                && (path.contains('/') || path.contains('\\') || path.contains('.'))
            {
                return Some(path.to_string());
            }
        }
    }

    None
}

pub fn merge_system_prompts(
    original: Option<&SystemPrompt>,
    summary: Option<SystemPrompt>,
) -> Option<SystemPrompt> {
    match (original, summary) {
        (None, None) => None,
        (Some(orig), None) => Some(orig.clone()),
        (None, Some(sum)) => Some(sum),
        (Some(SystemPrompt::Text(orig_text)), Some(SystemPrompt::Blocks(mut sum_blocks))) => {
            // Prepend original system prompt
            sum_blocks.insert(
                0,
                SystemBlock {
                    block_type: "text".to_string(),
                    text: orig_text.clone(),
                    cache_control: None,
                },
            );
            Some(SystemPrompt::Blocks(sum_blocks))
        }
        (Some(SystemPrompt::Blocks(orig_blocks)), Some(SystemPrompt::Blocks(mut sum_blocks))) => {
            // Prepend original blocks
            for (i, block) in orig_blocks.iter().enumerate() {
                sum_blocks.insert(i, block.clone());
            }
            Some(SystemPrompt::Blocks(sum_blocks))
        }
        (Some(orig), Some(SystemPrompt::Text(sum_text))) => {
            let mut blocks = match orig {
                SystemPrompt::Text(t) => vec![SystemBlock {
                    block_type: "text".to_string(),
                    text: t.clone(),
                    cache_control: None,
                }],
                SystemPrompt::Blocks(b) => b.clone(),
            };
            blocks.push(SystemBlock {
                block_type: "text".to_string(),
                text: sum_text,
                cache_control: None,
            });
            Some(SystemPrompt::Blocks(blocks))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(role: &str, text: &str) -> Message {
        Message {
            role: role.to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    fn tool_use(id: &str, name: &str, input: serde_json::Value) -> Message {
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input,
                caller: None,
            }],
        }
    }

    fn tool_result(id: &str, content: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: content.to_string(),
                is_error: None,
                content_blocks: None,
            }],
        }
    }

    #[test]
    fn anchor_summary_section_is_empty_without_workspace_or_file() {
        assert!(anchor_summary_section(None).is_empty());

        let tmpdir = tempfile::TempDir::new().unwrap();
        assert!(anchor_summary_section(Some(tmpdir.path())).is_empty());
    }

    #[test]
    fn anchor_summary_section_parses_anchor_file_into_bullets() {
        let tmpdir = tempfile::TempDir::new().unwrap();
        let deepseek_dir = tmpdir.path().join(".deepseek");
        std::fs::create_dir_all(&deepseek_dir).unwrap();
        std::fs::write(
            deepseek_dir.join("anchors.md"),
            "\n---\nDo not touch .ssh\n---\nStatus field is unreliable\n",
        )
        .unwrap();

        let section = anchor_summary_section(Some(tmpdir.path()));

        assert!(section.contains("## Pinned Facts (User Anchors)"));
        assert!(section.contains("- Do not touch .ssh\n"));
        assert!(section.contains("- Status field is unreliable\n"));
        assert!(!section.contains("\n---\nDo not touch"));
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
    fn is_transient_error_detects_network_issues() {
        let timeout_err = anyhow::anyhow!("Connection timeout");
        assert!(is_transient_error(&timeout_err));

        let rate_limit_err = anyhow::anyhow!("429 Too Many Requests");
        assert!(is_transient_error(&rate_limit_err));

        let service_err = anyhow::anyhow!("503 Service Unavailable");
        assert!(is_transient_error(&service_err));

        let network_err = anyhow::anyhow!("network error: connection refused");
        assert!(is_transient_error(&network_err));
    }

    #[test]
    fn is_transient_error_rejects_permanent_errors() {
        let auth_err = anyhow::anyhow!("401 Unauthorized: Invalid API key");
        assert!(!is_transient_error(&auth_err));

        let parse_err = anyhow::anyhow!("Failed to parse JSON response");
        assert!(!is_transient_error(&parse_err));

        let validation_err = anyhow::anyhow!("Invalid request: missing required field");
        assert!(!is_transient_error(&validation_err));
    }

    #[test]
    fn summary_limits_expand_for_v4_context() {
        let legacy = summary_input_limits_for_model("deepseek-v3.2-128k");
        let v4 = summary_input_limits_for_model("deepseek-v4-pro");

        assert!(v4.input_max_chars > legacy.input_max_chars);
        assert!(v4.tool_result_snippet_chars > legacy.tool_result_snippet_chars);
        assert!(v4.max_tokens > legacy.max_tokens);
    }

    #[test]
    fn cache_aligned_summary_is_used_for_v4_scale_contexts() {
        let messages = vec![msg("user", "Please edit crates/tui/src/compaction.rs")];

        assert!(should_use_cache_aligned_summary(
            "deepseek-v4-flash",
            &messages
        ));
        assert!(!should_use_cache_aligned_summary(
            "deepseek-v3.2-128k",
            &messages
        ));
    }

    /// #584: the summary cache-hit percentage must be computed against
    /// `input_tokens`, not `cache_hit + cache_miss`. Providers that
    /// only populate `prompt_cache_hit_tokens` (and leave the miss
    /// field at `None`) would otherwise be reported as a flat 100%
    /// hit rate even when most of the prompt was uncached.
    #[test]
    fn summary_cache_hit_percent_uses_input_tokens_as_denominator() {
        // Both fields populated and consistent.
        assert!((summary_cache_hit_percent(800, 1000) - 80.0).abs() < f64::EPSILON);
        // No cache hit at all.
        assert!((summary_cache_hit_percent(0, 1000) - 0.0).abs() < f64::EPSILON);
        // Full cache hit.
        assert!((summary_cache_hit_percent(1000, 1000) - 100.0).abs() < f64::EPSILON);
        // Partial-telemetry guard: provider reports `cache_hit` only,
        // miss is unknown (treated as 0 by the caller). Naive
        // `hit / (hit + miss)` would have reported 100%; against
        // `input_tokens` the answer is the real share.
        assert!((summary_cache_hit_percent(200, 1000) - 20.0).abs() < f64::EPSILON);
        // Defensive: zero `input_tokens` short-circuits without a
        // divide-by-zero.
        assert!((summary_cache_hit_percent(0, 0) - 0.0).abs() < f64::EPSILON);
        assert!((summary_cache_hit_percent(50, 0) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn cache_aligned_summary_request_preserves_message_prefix() {
        let messages = vec![
            msg("user", "Please edit crates/tui/src/compaction.rs"),
            msg("assistant", "I will inspect the file."),
        ];
        let limits = summary_input_limits_for_model("deepseek-v4-pro");
        let request = build_cache_aligned_summary_request("deepseek-v4-pro", &messages, limits);

        assert_eq!(request.system, None);
        assert_eq!(&request.messages[..messages.len()], &messages[..]);
        assert_eq!(request.messages.len(), messages.len() + 1);
        let last = request.messages.last().expect("summary instruction");
        assert_eq!(last.role, "user");
        assert!(matches!(
            &last.content[..],
            [ContentBlock::Text { text, .. }] if text.contains("conversation above")
        ));
    }

    #[test]
    fn estimate_tokens_empty_messages() {
        let messages: Vec<Message> = vec![];
        assert_eq!(estimate_tokens(&messages), 0);
    }

    #[test]
    fn estimate_tokens_with_text() {
        let messages = vec![Message {
            role: "user".to_string(),
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
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Use a tool".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::Thinking {
                        thinking: thinking.clone(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path": "Cargo.toml"}),
                        caller: None,
                    },
                ],
            },
            Message {
                role: "user".to_string(),
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
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Done.".to_string(),
                    cache_control: None,
                }],
            });
            messages.push(Message {
                role: "user".to_string(),
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
                role: "assistant".to_string(),
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
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "test".to_string(),
                    cache_control: None,
                }],
            })
            .collect();
        assert!(!should_compact(&messages, &config, None, None, None));
    }

    /// v0.8.11: message-count is no longer a compaction trigger. Long
    /// chats of small messages stay uncompacted because rewriting the V4
    /// prefix cache for a tiny budget reclaim is net-negative. Only token
    /// pressure (and the explicit `/compact` slash command) trigger
    /// compaction.
    #[test]
    fn message_count_no_longer_triggers_compaction() {
        let config = CompactionConfig {
            enabled: true,
            token_threshold: 1_000_000,
            auto_floor_tokens: 0,
            ..Default::default()
        };

        // 200 tiny messages, well above the prior message threshold.
        let many_messages: Vec<Message> = (0..200)
            .map(|_| Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "x".to_string(),
                    cache_control: None,
                }],
            })
            .collect();
        // Token total stays minuscule so the token threshold is not hit;
        // without the prior message-count trigger, no compaction.
        assert!(!should_compact(&many_messages, &config, None, None, None));
    }

    #[test]
    fn plan_compaction_pins_recent_and_working_set_paths() {
        let messages = vec![
            msg("user", "General discussion"),
            msg("assistant", "Unrelated note"),
            msg("user", "Earlier we touched src/core/engine.rs"),
            msg("assistant", "More unrelated chatter"),
            msg("user", "Let's keep working on src/core/engine.rs"),
            msg("assistant", "Tool output mentions src/core/engine.rs too"),
            msg("assistant", "Recent reasoning"),
            msg("user", "Final recent instruction"),
        ];

        let plan = plan_compaction(&messages, None, KEEP_RECENT_MESSAGES, None, None);

        assert!(plan.pinned_indices.contains(&2));
        for idx in 4..messages.len() {
            assert!(plan.pinned_indices.contains(&idx));
        }
        assert!(plan.summarize_indices.contains(&0));
        assert!(plan.summarize_indices.contains(&1));
        assert!(plan.summarize_indices.contains(&3));
    }

    #[test]
    fn plan_compaction_respects_external_pins() {
        let messages = vec![
            msg("user", "noise 0"),
            msg("assistant", "noise 1"),
            msg("user", "noise 2"),
            msg("assistant", "noise 3"),
            msg("user", "recent 4"),
            msg("assistant", "recent 5"),
            msg("assistant", "recent 6"),
            msg("user", "recent 7"),
        ];

        let pins = vec![1usize];
        let plan = plan_compaction(&messages, None, KEEP_RECENT_MESSAGES, Some(&pins), None);

        assert!(plan.pinned_indices.contains(&1));
        assert!(!plan.summarize_indices.contains(&1));
    }

    #[test]
    fn plan_compaction_uses_external_working_set_paths() {
        let mut messages = vec![msg("user", "edit src/core/engine.rs now")];
        messages.extend((1..20).map(|i| msg("assistant", &format!("noise {i}"))));

        let working_set_paths = vec!["src/core/engine.rs".to_string()];
        let plan = plan_compaction(
            &messages,
            None,
            KEEP_RECENT_MESSAGES,
            None,
            Some(&working_set_paths),
        );

        assert!(plan.pinned_indices.contains(&0));
    }

    #[test]
    fn plan_compaction_pins_tool_calls_for_tool_results() {
        let messages = vec![
            msg("user", "noise"),
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "read_file".to_string(),
                    input: json!({"path": "src/main.rs"}),
                    caller: None,
                }],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: "ok src/main.rs".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];

        let plan = plan_compaction(&messages, None, 1, None, None);
        assert!(plan.pinned_indices.contains(&2));
        assert!(plan.pinned_indices.contains(&1));
    }

    #[test]
    fn should_compact_ignores_fully_pinned_context() {
        let config = CompactionConfig {
            enabled: true,
            token_threshold: 10,
            ..Default::default()
        };

        let messages: Vec<Message> = (0..12)
            .map(|_| msg("user", "Work on src/compaction.rs right now"))
            .collect();

        assert!(!should_compact(&messages, &config, None, None, None));
    }

    // v0.8.11: removed `should_compact_counts_only_unpinned_messages` and
    // `should_compact_when_pins_consume_budget` — both tested the
    // message-count compaction trigger that v0.8.11 deleted. The
    // pinned-tokens accounting they exercised is still tested by
    // `should_compact_ignores_fully_pinned_context` below; the rest of
    // their setup has no contemporary contract to pin.

    #[test]
    fn enforce_tool_call_pairs_removes_orphaned_tool_call() {
        // An assistant message with a tool call but no matching result anywhere
        // in the history should be removed from the pinned set.
        let messages = vec![
            msg("user", "noise"),
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "orphan-call".to_string(),
                    name: "read_file".to_string(),
                    input: json!({"path": "src/main.rs"}),
                    caller: None,
                }],
            },
            msg("assistant", "recent"),
        ];

        let mut pinned = BTreeSet::from([0, 1, 2]);
        enforce_tool_call_pairs(&messages, &mut pinned);

        // The orphaned tool call message (index 1) should be removed.
        assert!(
            !pinned.contains(&1),
            "orphaned tool call should be removed from pinned set"
        );
        // Other messages stay.
        assert!(pinned.contains(&0));
        assert!(pinned.contains(&2));
    }

    #[test]
    fn enforce_tool_call_pairs_removes_orphaned_tool_result() {
        // A tool result whose call doesn't exist anywhere should be removed.
        let messages = vec![
            msg("user", "noise"),
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "orphan-result".to_string(),
                    content: "ok".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            msg("assistant", "recent"),
        ];

        let mut pinned = BTreeSet::from([0, 1, 2]);
        enforce_tool_call_pairs(&messages, &mut pinned);

        assert!(
            !pinned.contains(&1),
            "orphaned tool result should be removed from pinned set"
        );
        assert!(pinned.contains(&0));
        assert!(pinned.contains(&2));
    }

    #[test]
    fn enforce_tool_call_pairs_preserves_valid_pairs() {
        // A complete call+result pair should remain intact.
        let messages = vec![
            msg("user", "do something"),
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "tool-ok".to_string(),
                    name: "list_dir".to_string(),
                    input: json!({}),
                    caller: None,
                }],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool-ok".to_string(),
                    content: "files here".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            msg("assistant", "done"),
        ];

        let mut pinned = BTreeSet::from([1, 2, 3]);
        enforce_tool_call_pairs(&messages, &mut pinned);

        assert!(pinned.contains(&1), "tool call should stay pinned");
        assert!(pinned.contains(&2), "tool result should stay pinned");
        assert!(pinned.contains(&3));
    }

    #[test]
    fn enforce_tool_call_pairs_pins_transitive_pairs() {
        // If only the result is initially pinned, the call should be pulled in.
        // The call message may also contain another tool call whose result should
        // then be pulled in transitively.
        let messages = vec![
            msg("user", "start"),
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::ToolUse {
                        id: "t1".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "a.rs"}),
                        caller: None,
                    },
                    ContentBlock::ToolUse {
                        id: "t2".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "b.rs"}),
                        caller: None,
                    },
                ],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    content: "content of a.rs".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "t2".to_string(),
                    content: "content of b.rs".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            msg("assistant", "done"),
        ];

        // Only pin the result for t1 initially.
        let mut pinned = BTreeSet::from([2, 4]);
        enforce_tool_call_pairs(&messages, &mut pinned);

        // The call message (index 1) should be pulled in because t1's result is pinned.
        assert!(
            pinned.contains(&1),
            "call message should be transitively pinned"
        );
        // Since the call message also contains t2, t2's result (index 3) should also be pinned.
        assert!(
            pinned.contains(&3),
            "t2 result should be transitively pinned via the call message"
        );
    }

    #[test]
    fn enforce_tool_call_pairs_cascading_removal() {
        // Removing an orphaned call should cascade to remove its result.
        // Message 1: assistant with t1 (call) — t1 has a result at index 2
        // Message 2: user with t1 (result)
        // Message 3: assistant with t2 (call) — t2 has NO result
        // Message 4: user with t2 result referencing the call
        //
        // If t2 has no result in history, message 3 is removed. That's straightforward.
        // Here we test: if a call message is removed because ONE of its calls is orphaned,
        // the result for the other call also gets removed in subsequent iterations.
        let messages = vec![
            msg("user", "start"),
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::ToolUse {
                        id: "good".to_string(),
                        name: "read_file".to_string(),
                        input: json!({}),
                        caller: None,
                    },
                    ContentBlock::ToolUse {
                        id: "orphan".to_string(),
                        name: "shell".to_string(),
                        input: json!({}),
                        caller: None,
                    },
                ],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "good".to_string(),
                    content: "ok".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            // Note: NO result for "orphan" exists anywhere
            msg("assistant", "done"),
        ];

        let mut pinned = BTreeSet::from([1, 2, 3]);
        enforce_tool_call_pairs(&messages, &mut pinned);

        // Message 1 has an orphaned tool call ("orphan"), so it's removed.
        assert!(
            !pinned.contains(&1),
            "message with orphaned call should be removed"
        );
        // Message 2 (result for "good") now has no matching call pinned, so it's also removed.
        assert!(
            !pinned.contains(&2),
            "result whose call was removed should cascade-remove"
        );
        // Message 3 (plain text) stays.
        assert!(pinned.contains(&3));
    }

    #[test]
    fn enforce_tool_call_pairs_converges_long_chain() {
        let mut messages = vec![msg("user", "start")];
        for i in 0..15 {
            messages.push(Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: format!("t{i}"),
                    name: "read_file".to_string(),
                    input: json!({}),
                    caller: None,
                }],
            });
            messages.push(Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: format!("t{i}"),
                    content: format!("result {i}"),
                    is_error: None,
                    content_blocks: None,
                }],
            });
        }
        messages.push(msg("assistant", "done"));

        let mut pinned: BTreeSet<usize> = (0..messages.len()).collect();
        enforce_tool_call_pairs(&messages, &mut pinned);

        // All pairs should remain intact (no orphans)
        assert_eq!(pinned.len(), messages.len());
    }

    #[test]
    fn plan_compaction_keeps_at_least_one_user_text_query() {
        let mut messages = vec![msg(
            "user",
            "This is the original query that started the chain.",
        )];

        for i in 0..10 {
            messages.push(Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: format!("call-{i}"),
                    name: "test_tool".to_string(),
                    input: json!({}),
                    caller: None,
                }],
            });
            messages.push(Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: format!("call-{i}"),
                    content: "tool output".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            });
        }

        let plan = plan_compaction(&messages, None, KEEP_RECENT_MESSAGES, None, None);

        assert!(plan.pinned_indices.contains(&0));
    }

    // ========================================================================
    // Additional Compaction Trigger Tests
    // ========================================================================

    #[test]
    fn test_should_compact_token_threshold_triggers() {
        let config = CompactionConfig {
            enabled: true,
            token_threshold: 100, // Low threshold for testing
            auto_floor_tokens: 0,
            ..Default::default()
        };

        // Create messages that exceed token threshold
        let messages: Vec<Message> = (0..10)
            .map(|_| msg("user", &"x".repeat(50))) // 50 chars = ~12 tokens each
            .collect();

        // Total tokens: ~120, which exceeds 100
        assert!(should_compact(&messages, &config, None, None, None));
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

        assert!(!should_compact(&messages, &config, None, None, None));
    }

    /// v0.8.11: the 500K hard floor blocks auto-compaction even when the
    /// token-percentage threshold would otherwise fire. This is the V4
    /// prefix-cache protection — below 500K total tokens, rewriting the
    /// prefix loses cache for tiny budget gains.
    #[test]
    fn auto_compaction_floor_blocks_below_500k_even_when_threshold_says_yes() {
        let config = CompactionConfig {
            enabled: true,
            token_threshold: 100, // would normally fire instantly
            // Use the production default explicitly so this test pins the
            // floor's contract rather than relying on `Default`.
            auto_floor_tokens: MINIMUM_AUTO_COMPACTION_TOKENS,
            ..Default::default()
        };

        let messages: Vec<Message> = (0..10).map(|_| msg("user", &"x".repeat(50))).collect();
        // Total tokens way under 500K, so floor blocks compaction.
        assert!(!should_compact(&messages, &config, None, None, None));
    }

    /// v0.8.11: when total tokens cross the 500K floor, the existing
    /// threshold/message-count logic takes over again.
    #[test]
    fn auto_compaction_floor_yields_to_threshold_logic_above_500k() {
        let config = CompactionConfig {
            enabled: true,
            token_threshold: 2_000_000,
            auto_floor_tokens: MINIMUM_AUTO_COMPACTION_TOKENS,
            ..Default::default()
        };

        // Each message ~500 tokens; 1100 messages → ~550K total tokens.
        // That's above the floor (500K) AND below the deliberately high
        // token_threshold, so auto-compaction stays off — by threshold,
        // not floor.
        let messages: Vec<Message> = (0..1100).map(|_| msg("user", &"x".repeat(2000))).collect();
        assert!(!should_compact(&messages, &config, None, None, None));

        // Crank threshold below total → compaction fires now that we're
        // past the floor.
        let config_lower = CompactionConfig {
            token_threshold: 100_000,
            ..config
        };
        assert!(should_compact(&messages, &config_lower, None, None, None));
    }

    /// `CompactionConfig::default()` ships with the 500K floor on by
    /// default — production callers via `..Default::default()` get the
    /// safety guarantee automatically.
    #[test]
    fn compaction_config_default_carries_500k_floor() {
        let config = CompactionConfig::default();
        assert_eq!(config.auto_floor_tokens, MINIMUM_AUTO_COMPACTION_TOKENS);
        assert_eq!(config.auto_floor_tokens, 500_000);
    }

    #[test]
    fn test_plan_compaction_pins_error_messages() {
        let messages = vec![
            msg("user", "normal message"),
            msg("assistant", "error: compilation failed"),
            msg("user", "another message"),
            msg("assistant", "panic at src/main.rs:42"),
            msg("user", "more chat"),
            msg("assistant", "Traceback (most recent call last):"),
            msg("user", "recent 1"),
            msg("assistant", "recent 2"),
        ];

        let plan = plan_compaction(&messages, None, KEEP_RECENT_MESSAGES, None, None);

        // Error messages should be pinned
        assert!(plan.pinned_indices.contains(&1)); // error:
        assert!(plan.pinned_indices.contains(&3)); // panic
        assert!(plan.pinned_indices.contains(&5)); // traceback
    }

    #[test]
    fn test_plan_compaction_pins_patch_messages() {
        let messages = vec![
            msg("user", "normal chat"),
            msg("assistant", "diff --git a/src/main.rs b/src/main.rs"),
            msg("user", "more chat"),
            msg("assistant", "+++ b/src/core.rs"),
            msg("user", "chat"),
            msg("assistant", "```diff\n-some code\n+new code\n```"),
            msg("user", "recent 1"),
            msg("assistant", "recent 2"),
        ];

        let plan = plan_compaction(&messages, None, KEEP_RECENT_MESSAGES, None, None);

        // Patch/diff messages should be pinned
        assert!(plan.pinned_indices.contains(&1)); // diff --git
        assert!(plan.pinned_indices.contains(&3)); // +++ b/
        assert!(plan.pinned_indices.contains(&5)); // ```diff
    }

    #[test]
    fn test_plan_compaction_pins_apply_patch_tool_calls() {
        let messages = vec![
            msg("user", "normal chat"),
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "patch-1".to_string(),
                    name: "apply_patch".to_string(),
                    input: json!({"patch": "diff content"}),
                    caller: None,
                }],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "patch-1".to_string(),
                    content: "Patch applied successfully".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            msg("assistant", "more chat"),
            msg("user", "even more"),
            msg("assistant", "recent 1"),
            msg("user", "recent 2"),
            msg("assistant", "recent 3"),
        ];

        let plan = plan_compaction(&messages, None, KEEP_RECENT_MESSAGES, None, None);

        // Message 1 contains apply_patch tool call with matching result (message 2)
        // Both should be pinned due to tool call pairing
        // Messages 5, 6, 7, 8 are recent (last 4 messages)
        eprintln!("Pinned indices: {:?}", plan.pinned_indices);

        // apply_patch tool call and its result should be pinned
        assert!(
            plan.pinned_indices.contains(&1),
            "apply_patch tool call should be pinned"
        );
        assert!(
            plan.pinned_indices.contains(&2),
            "apply_patch tool result should be pinned"
        );
    }

    #[test]
    fn test_extract_paths_from_text_finds_various_formats() {
        let text = r#"
            I'm working on src/main.rs
            Also check Cargo.toml
            The error is in src/core/engine.rs:42
            See docs/API.md for details
            Config at config.example.toml
        "#;

        let paths = extract_paths_from_text(text, None);

        assert!(paths.iter().any(|p| p == "src/main.rs"));
        assert!(paths.iter().any(|p| p == "Cargo.toml"));
        assert!(paths.iter().any(|p| p == "src/core/engine.rs"));
        assert!(paths.iter().any(|p| p == "docs/API.md"));
        assert!(paths.iter().any(|p| p == "config.example.toml"));
    }

    #[test]
    fn test_extract_paths_from_tool_input_finds_path_field() {
        let input = json!({
            "path": "src/main.rs",
            "content": "test"
        });

        let paths = extract_paths_from_tool_input(&input, None);
        assert!(paths.iter().any(|p| p == "src/main.rs"));
    }

    #[test]
    fn test_extract_paths_from_tool_input_finds_paths_array() {
        let input = json!({
            "paths": ["src/main.rs", "src/core.rs", "tests/test.rs"]
        });

        let paths = extract_paths_from_tool_input(&input, None);
        assert_eq!(paths.len(), 3);
        assert!(paths.iter().any(|p| p == "src/main.rs"));
        assert!(paths.iter().any(|p| p == "src/core.rs"));
        assert!(paths.iter().any(|p| p == "tests/test.rs"));
    }

    #[test]
    fn test_extract_paths_from_tool_input_finds_cwd() {
        let input = json!({
            "cwd": "src/core",
            "command": "cargo build"
        });

        let paths = extract_paths_from_tool_input(&input, None);
        assert!(paths.iter().any(|p| p == "src/core"));
    }

    #[test]
    fn test_normalize_path_candidate_handles_absolute_paths() {
        use std::env;
        let current_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        // Create an absolute path
        let absolute_path = current_dir.join("src/main.rs");
        let absolute_path_str = absolute_path.to_string_lossy();

        let normalized = normalize_path_candidate(&absolute_path_str, Some(&current_dir));

        assert_eq!(normalized, Some("src/main.rs".to_string()));
    }

    #[test]
    fn test_normalize_path_candidate_rejects_parent_refs() {
        let normalized = normalize_path_candidate("../outside/file.rs", Some(&PathBuf::from(".")));
        assert_eq!(normalized, None);
    }

    #[test]
    fn test_normalize_path_candidate_cleans_backslashes() {
        let normalized = normalize_path_candidate("src\\main.rs", Some(&PathBuf::from(".")));
        assert_eq!(normalized, Some("src/main.rs".to_string()));
    }

    #[test]
    fn test_merge_system_prompts_none_none() {
        let result = merge_system_prompts(None, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_merge_system_prompts_some_text_none() {
        let original = Some(SystemPrompt::Text("original".to_string()));
        let result = merge_system_prompts(original.as_ref(), None);
        assert!(matches!(result, Some(SystemPrompt::Text(s)) if s == "original"));
    }

    #[test]
    fn test_merge_system_prompts_none_some_blocks() {
        let summary = Some(SystemPrompt::Blocks(vec![SystemBlock {
            block_type: "text".to_string(),
            text: "summary".to_string(),
            cache_control: None,
        }]));
        let result = merge_system_prompts(None, summary);
        assert!(matches!(result, Some(SystemPrompt::Blocks(b)) if b.len() == 1));
    }

    #[test]
    fn test_merge_system_prompts_text_plus_blocks() {
        let original = Some(SystemPrompt::Text("original".to_string()));
        let summary = Some(SystemPrompt::Blocks(vec![SystemBlock {
            block_type: "text".to_string(),
            text: "summary".to_string(),
            cache_control: None,
        }]));

        let result = merge_system_prompts(original.as_ref(), summary);

        match result {
            Some(SystemPrompt::Blocks(blocks)) => {
                assert_eq!(blocks.len(), 2);
                assert!(matches!(&blocks[0], SystemBlock { text, .. } if text == "original"));
                assert!(matches!(&blocks[1], SystemBlock { text, .. } if text == "summary"));
            }
            _ => panic!("Expected Blocks"),
        }
    }

    #[test]
    fn test_merge_system_prompts_blocks_plus_blocks() {
        let original = Some(SystemPrompt::Blocks(vec![
            SystemBlock {
                block_type: "text".to_string(),
                text: "orig1".to_string(),
                cache_control: None,
            },
            SystemBlock {
                block_type: "text".to_string(),
                text: "orig2".to_string(),
                cache_control: None,
            },
        ]));

        let summary = Some(SystemPrompt::Blocks(vec![SystemBlock {
            block_type: "text".to_string(),
            text: "summary".to_string(),
            cache_control: None,
        }]));

        let result = merge_system_prompts(original.as_ref(), summary);

        match result {
            Some(SystemPrompt::Blocks(blocks)) => {
                assert_eq!(blocks.len(), 3);
                assert!(matches!(&blocks[0], SystemBlock { text, .. } if text == "orig1"));
                assert!(matches!(&blocks[1], SystemBlock { text, .. } if text == "orig2"));
                assert!(matches!(&blocks[2], SystemBlock { text, .. } if text == "summary"));
            }
            _ => panic!("Expected Blocks"),
        }
    }

    #[test]
    fn test_merge_system_prompts_blocks_plus_text() {
        let original = Some(SystemPrompt::Blocks(vec![SystemBlock {
            block_type: "text".to_string(),
            text: "original".to_string(),
            cache_control: None,
        }]));

        let summary = Some(SystemPrompt::Text("summary".to_string()));

        let result = merge_system_prompts(original.as_ref(), summary);

        match result {
            Some(SystemPrompt::Blocks(blocks)) => {
                assert_eq!(blocks.len(), 2);
                assert!(matches!(&blocks[0], SystemBlock { text, .. } if text == "original"));
                assert!(matches!(&blocks[1], SystemBlock { text, .. } if text == "summary"));
            }
            _ => panic!("Expected Blocks"),
        }
    }

    #[test]
    fn test_compaction_result_retries_used() {
        // This test verifies the CompactionResult structure
        let result = CompactionResult {
            messages: vec![],
            summary_prompt: None,
            removed_messages: vec![],
            retries_used: 2,
        };

        assert_eq!(result.retries_used, 2);
        assert!(result.messages.is_empty());
        assert!(result.removed_messages.is_empty());
    }

    #[test]
    fn test_should_compact_with_workspace_path_detection() {
        use std::env;
        let workspace = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let _config = CompactionConfig {
            enabled: true,
            token_threshold: 1000,
            ..Default::default()
        };

        // Create messages mentioning workspace paths
        let messages = vec![
            msg("user", "working on src/main.rs"),
            msg("assistant", "noise 1"),
            msg("user", "noise 2"),
            msg("assistant", "noise 3"),
            msg("user", "noise 4"),
            msg("assistant", "noise 5"),
            msg("user", "recent 1"),
            msg("assistant", "recent 2"),
        ];

        // src/main.rs mention should pin message 0 in the plan.
        let plan = plan_compaction(
            &messages,
            Some(&workspace),
            KEEP_RECENT_MESSAGES,
            None,
            None,
        );
        assert!(plan.pinned_indices.contains(&0)); // src/main.rs mention
    }
}
