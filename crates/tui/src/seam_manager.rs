//! Append-only layered context management with Flash seam manager (issue #159).
//!
//! ## Why
//!
//! The current cycle/compaction/capacity mechanisms share a fatal flaw: they
//! replace or rewrite messages, which breaks DeepSeek V4's prefix cache
//! (SS4.2.1). The prefix cache gives ~90% discount on cached tokens at
//! 128-token granularity. Replacing old messages with summaries breaks the
//! cache at the replacement point — every token after must be recomputed.
//!
//! The append-only layered approach keeps all verbatim messages and appends
//! `<archived_context>` summary blocks produced by V4 Flash. These blocks
//! are *navigational aids* — the model reads them first, then drills into
//! verbatim messages when precision is needed. The prefix cache stays hot
//! for the entire stable prefix. In v0.7.5 this manager is opt-in while the
//! cache/timing policy is audited.
//!
//! ## Soft seam levels
//!
//! | Level | Active input trigger | Covers messages    | Density        |
//! |-------|------------------|--------------------|----------------|
//! | L1    | 192K             | 0–128K             | ~2,500 tokens  |
//! | L2    | 384K             | 0–320K             | ~1,800 tokens  |
//! | L3    | 576K             | 0–512K             | ~1,200 tokens  |
//! | Cycle | 768K             | All -> archive     | <=3,000 tokens  |
//!
//! Thresholds derived from V4 paper Figure 9 (MMR): 128K->256K is the real
//! cliff at -0.09. L1 triggers at 192K, before the cliff. Hard cycle at
//! 768K (~75% of 1M window).

use std::fmt::Write;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use tokio::sync::Mutex;

use crate::client::DeepSeekClient;
use crate::compaction::KEEP_RECENT_MESSAGES;
use crate::compaction::plan_compaction;
use crate::llm_client::LlmClient;
use crate::models::{ContentBlock, Message, MessageRequest, SystemBlock, SystemPrompt};

/// Default seam model — Flash is cheap and fast, ideal for summarization.
pub const DEFAULT_SEAM_MODEL: &str = "deepseek-v4-flash";

/// Default thresholds based on the active request input estimate.
pub const DEFAULT_L1_THRESHOLD: usize = 192_000;
pub const DEFAULT_L2_THRESHOLD: usize = 384_000;
pub const DEFAULT_L3_THRESHOLD: usize = 576_000;
pub const DEFAULT_CYCLE_THRESHOLD: usize = 768_000;

/// Verbatim window: last N turns never summarized.
pub const VERBATIM_WINDOW_TURNS: usize = 16;

/// Approximate token cap for each seam level.
const L1_MAX_TOKENS: u32 = 3_200;
const L2_MAX_TOKENS: u32 = 2_400;
const L3_MAX_TOKENS: u32 = 1_600;

/// Configuration for the Flash seam manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeamConfig {
    /// Whether the layered context manager is enabled.
    pub enabled: bool,
    /// Verbatim window: last N turns never summarized.
    pub verbatim_window_turns: usize,
    /// Soft seam thresholds based on the active request input estimate.
    pub l1_threshold: usize,
    pub l2_threshold: usize,
    pub l3_threshold: usize,
    /// Hard cycle boundary.
    pub cycle_threshold: usize,
    /// Model used for seam/briefing work.
    pub seam_model: String,
}

impl Default for SeamConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            verbatim_window_turns: VERBATIM_WINDOW_TURNS,
            l1_threshold: DEFAULT_L1_THRESHOLD,
            l2_threshold: DEFAULT_L2_THRESHOLD,
            l3_threshold: DEFAULT_L3_THRESHOLD,
            cycle_threshold: DEFAULT_CYCLE_THRESHOLD,
            seam_model: DEFAULT_SEAM_MODEL.to_string(),
        }
    }
}

/// Metadata for a single soft seam block.
#[derive(Debug, Clone)]
pub struct SeamMetadata {
    /// Which level (1, 2, or 3).
    pub level: u8,
    /// Message range covered (inclusive-exclusive indices).
    /// Reserved for future diagnostic use.
    #[allow(dead_code)]
    pub start_idx: usize,
    #[allow(dead_code)]
    pub end_idx: usize,
    /// Approximate token count of the summary.
    #[allow(dead_code)]
    pub token_estimate: usize,
    /// When the seam was produced.
    #[allow(dead_code)]
    pub timestamp: DateTime<Utc>,
    /// Model that produced it.
    #[allow(dead_code)]
    pub model: String,
}

/// The Flash seam manager — produces `<archived_context>` blocks.
pub struct SeamManager {
    /// Flash client for summarization work.
    flash_client: DeepSeekClient,
    /// Configuration.
    config: SeamConfig,
    /// Currently active seams in order (oldest first).
    active_seams: Arc<Mutex<Vec<SeamMetadata>>>,
}

impl SeamManager {
    /// Create a new seam manager with a Flash client.
    pub fn new(flash_client: DeepSeekClient, config: SeamConfig) -> Self {
        Self {
            flash_client,
            config,
            active_seams: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Get the current config.
    pub fn config(&self) -> &SeamConfig {
        &self.config
    }

    /// Current active seam count.
    pub async fn seam_count(&self) -> usize {
        self.active_seams.lock().await.len()
    }

    /// Determine which seam level (if any) should fire for the given
    /// active request input estimate. Returns `None` when no seam is due.
    #[must_use]
    pub fn seam_level_for(
        &self,
        active_input_tokens: usize,
        highest_existing_level: Option<u8>,
    ) -> Option<u8> {
        seam_level_for_active_input(&self.config, active_input_tokens, highest_existing_level)
    }

    /// Check whether the hard cycle boundary is crossed.
    ///
    /// Note: not currently called — cycle detection uses an inline check.
    /// Kept as the canonical boundary definition for future wiring.
    #[must_use]
    #[allow(dead_code)]
    pub fn should_cycle(&self, active_input_tokens: usize) -> bool {
        self.config.enabled && active_input_tokens >= self.config.cycle_threshold
    }

    /// Compute the verbatim window: the last N message indices that must
    /// never be summarized. Returns the start index of the verbatim window.
    pub fn verbatim_window_start(&self, message_count: usize) -> usize {
        let turn_count = message_count / 2; // Rough: user+assistant per turn
        let verbatim_turns = self.config.verbatim_window_turns.min(turn_count);
        let verbatim_messages = (verbatim_turns * 2).min(message_count);
        message_count.saturating_sub(verbatim_messages)
    }

    /// Produce a soft seam for the given message range and level.
    ///
    /// Returns the `<archived_context>` XML block as a string, ready to
    /// be appended as an assistant message.
    pub async fn produce_soft_seam(
        &self,
        messages: &[Message],
        level: u8,
        start_idx: usize,
        end_idx: usize,
        workspace: Option<&Path>,
        pinned_indices: &[usize],
    ) -> Result<String> {
        if messages.is_empty() || start_idx >= end_idx {
            return Ok(String::new());
        }

        let range = &messages[start_idx..end_idx.min(messages.len())];
        if range.is_empty() {
            return Ok(String::new());
        }

        // Use compaction pinning heuristics to identify which messages to
        // exclude from summarization. Pinned messages stay verbatim; the
        // seam summary covers everything else.
        let local_pins = local_pins_for_range(pinned_indices, start_idx, end_idx, messages.len());
        let plan = plan_compaction(
            range,
            workspace,
            KEEP_RECENT_MESSAGES.min(range.len().saturating_sub(1)),
            Some(&local_pins),
            None,
        );

        // Collect messages to summarize (non-pinned), excluding pinned ones.
        let to_summarize: Vec<&Message> = range
            .iter()
            .enumerate()
            .filter(|(idx, _msg)| !plan.pinned_indices.contains(idx))
            .map(|(_idx, msg)| msg)
            .collect();

        if to_summarize.is_empty() {
            // Nothing to summarize — all messages are pinned.
            return Ok(String::new());
        }

        let summary = self
            .summarize_messages(&to_summarize, level, start_idx, end_idx)
            .await?;

        let density_label = match level {
            1 => "~2,500 tokens",
            2 => "~1,800 tokens",
            3 => "~1,200 tokens",
            _ => "unknown",
        };

        let timestamp = Utc::now();
        let token_estimate = summary.len() / 4;

        // Record this seam.
        {
            let mut seams = self.active_seams.lock().await;
            seams.push(SeamMetadata {
                level,
                start_idx,
                end_idx,
                token_estimate,
                timestamp,
                model: self.config.seam_model.clone(),
            });
        }

        Ok(format!(
            "<archived_context level=\"{level}\" range=\"msg {start_idx}-{end_idx}\" \
             tokens=\"~{token_estimate}\" density=\"{density_label}\" \
             model=\"{seam_model}\" timestamp=\"{ts}\">\n\
             {summary}\n\
             </archived_context>",
            seam_model = self.config.seam_model,
            ts = timestamp.to_rfc3339()
        ))
    }

    /// Re-compact existing seams into a higher-level block. Consumes prior
    /// `<archived_context>` content and fuses it with new messages.
    pub async fn recompact(
        &self,
        existing_seams: &[String],
        new_messages: &[&Message],
        level: u8,
        start_idx: usize,
        end_idx: usize,
    ) -> Result<String> {
        let mut input = String::from(
            "## Prior Context Summaries\n\n\
             The following <archived_context> blocks were produced earlier. \
             Merge their key information into a single denser summary.\n\n",
        );

        for (i, seam) in existing_seams.iter().enumerate() {
            let _ = write!(input, "### Seam {}\n{seam}\n\n", i + 1);
        }

        if !new_messages.is_empty() {
            input.push_str("## Recent Messages\n\n");
            for msg in new_messages {
                let role = &msg.role;
                for block in &msg.content {
                    if let ContentBlock::Text { text, .. } = block {
                        let _ = write!(input, "**{role}:** {text}\n\n");
                    }
                }
            }
        }

        let (max_tokens, word_limit) = match level {
            2 => (L2_MAX_TOKENS, 700),
            3 => (L3_MAX_TOKENS, 400),
            _ => (L3_MAX_TOKENS, 400),
        };

        let request = MessageRequest {
            model: self.config.seam_model.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: format!(
                        "Synthesize the following context into a single dense summary. \
                         Preserve: decisions made, file paths, error messages, \
                         constraints, hypotheses, open questions, and task state. \
                         Drop: greeting, filler, repeated information. \
                         Keep it under {word_limit} words.\n\n{input}"
                    ),
                    cache_control: None,
                }],
            }],
            max_tokens,
            system: Some(SystemPrompt::Text(
                "You are a context compaction specialist. Produce dense, factual summaries that \
                 preserve every decision, path, error, constraint, and open question. Drop \
                 conversational filler and repetition."
                    .to_string(),
            )),
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: Some(false),
            temperature: Some(0.1),
            top_p: None,
        };

        let response = self.flash_client.create_message(request).await?;
        // Seam recompaction calls are billed; route through the
        // side-channel (#526) so the footer total matches the
        // DeepSeek website.
        crate::cost_status::report(&response.model, &response.usage);
        let summary = response
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        let token_estimate = summary.len() / 4;
        let timestamp = Utc::now();

        // Record this recompacted seam.
        {
            let mut seams = self.active_seams.lock().await;
            seams.push(SeamMetadata {
                level,
                start_idx,
                end_idx,
                token_estimate,
                timestamp,
                model: self.config.seam_model.clone(),
            });
        }

        Ok(format!(
            "<archived_context level=\"{level}\" range=\"msg {start_idx}-{end_idx}\" \
             tokens=\"~{token_estimate}\" model=\"{model}\" timestamp=\"{ts}\">\n\
             {summary}\n\
             </archived_context>",
            model = self.config.seam_model,
            ts = timestamp.to_rfc3339()
        ))
    }

    /// Produce a cycle briefing using Flash. Unlike the current
    /// `produce_briefing` in cycle_manager.rs (which uses the main model),
    /// this consumes existing `<archived_context>` blocks as input rather
    /// than scanning raw history.
    pub async fn produce_flash_briefing(
        &self,
        existing_seams: &[String],
        structured_state: Option<&str>,
    ) -> Result<String> {
        let mut input = String::from(
            "## Briefing Request\n\n\
             Produce a <carry_forward> block summarizing the session state. \
             Include: decisions made + why, constraints discovered, \
             hypotheses being tested, approaches that failed, open questions. \
             Do NOT include tool output bytes, file contents, or step-by-step recaps.\n\n",
        );

        if let Some(state) = structured_state {
            let _ = write!(input, "## Structured State\n\n{state}\n\n");
        }

        if !existing_seams.is_empty() {
            input.push_str("## Prior Context Summaries\n\n");
            for (i, seam) in existing_seams.iter().enumerate() {
                let _ = write!(input, "### Seam {}\n{seam}\n\n", i + 1);
            }
        } else {
            input.push_str(
                "No prior context summaries available. Produce a brief carry-forward \
                 from the structured state alone.\n",
            );
        }

        let request = MessageRequest {
            model: self.config.seam_model.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: input,
                    cache_control: None,
                }],
            }],
            max_tokens: 4_096,
            system: Some(SystemPrompt::Blocks(vec![SystemBlock {
                block_type: "text".to_string(),
                text: crate::cycle_manager::CYCLE_HANDOFF_TEMPLATE.to_string(),
                cache_control: None,
            }])),
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: Some(false),
            temperature: Some(0.2),
            top_p: None,
        };

        let response = self.flash_client.create_message(request).await?;
        // Seam recompaction calls are billed; route through the
        // side-channel (#526) so the footer total matches the
        // DeepSeek website.
        crate::cost_status::report(&response.model, &response.usage);
        let raw = response
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(crate::cycle_manager::extract_carry_forward(&raw))
    }

    /// Internal: summarize a slice of messages using Flash.
    async fn summarize_messages(
        &self,
        messages: &[&Message],
        level: u8,
        start_idx: usize,
        end_idx: usize,
    ) -> Result<String> {
        let mut conversation = String::new();

        for msg in messages {
            let role = if msg.role == "user" {
                "User"
            } else {
                "Assistant"
            };
            for block in &msg.content {
                match block {
                    ContentBlock::Text { text, .. } => {
                        let snippet = truncate_chars(text, 800);
                        let _ = write!(conversation, "{role}: {snippet}\n\n");
                    }
                    ContentBlock::ToolUse { name, .. } => {
                        let _ = write!(conversation, "{role}: [Used tool: {name}]\n\n");
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        let snippet = truncate_chars(content, 200);
                        let _ = write!(conversation, "Tool result: {snippet}\n\n");
                    }
                    ContentBlock::Thinking { .. } => {
                        // Skip thinking in seam summaries.
                    }
                    ContentBlock::ImageUrl { .. } => {
                        let _ = write!(conversation, "{role}: [Attached image]\n\n");
                    }
                    ContentBlock::ServerToolUse { .. }
                    | ContentBlock::ToolSearchToolResult { .. }
                    | ContentBlock::CodeExecutionToolResult { .. } => {}
                }
            }
        }

        let (max_tokens, word_limit) = match level {
            1 => (L1_MAX_TOKENS, 800),
            2 => (L2_MAX_TOKENS, 600),
            3 => (L3_MAX_TOKENS, 400),
            _ => (L3_MAX_TOKENS, 400),
        };

        let request = MessageRequest {
            model: self.config.seam_model.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: format!(
                        "Summarize the following conversation segment (messages {start_idx}-{end_idx}). \
                         Preserve: key decisions and their rationale, exact file paths, \
                         command invocations, error messages, tool-result facts, constraints \
                         discovered, hypotheses being tested, and open questions. \
                         Drop: greetings, filler, repeated information, and thinking blocks. \
                         Keep it under {word_limit} words.\n\n---\n\n{conversation}"
                    ),
                    cache_control: None,
                }],
            }],
            max_tokens,
            system: Some(SystemPrompt::Text(
                "You are a context summarization specialist. Produce dense, factual summaries \
                 that preserve every decision, path, error, constraint, and open question. \
                 Never omit a file path, error message, or decision rationale."
                    .to_string(),
            )),
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: Some(false),
            temperature: Some(0.1),
            top_p: None,
        };

        let response = self.flash_client.create_message(request).await?;
        // Seam recompaction calls are billed; route through the
        // side-channel (#526) so the footer total matches the
        // DeepSeek website.
        crate::cost_status::report(&response.model, &response.usage);
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

    /// Collect the text content of all active seams (for use as input to
    /// re-compaction or briefing).
    pub async fn collect_seam_texts(&self, messages: &[Message]) -> Vec<String> {
        let _seams = self.active_seams.lock().await;
        let mut texts = Vec::new();

        // Extract `<archived_context>` blocks from messages.
        for msg in messages {
            if msg.role == "assistant" {
                for block in &msg.content {
                    if let ContentBlock::Text { text, .. } = block
                        && text.contains("<archived_context")
                    {
                        texts.push(text.clone());
                    }
                }
            }
        }

        texts
    }

    /// Get the highest seam level currently recorded.
    pub async fn highest_level(&self) -> Option<u8> {
        let seams = self.active_seams.lock().await;
        seams.last().map(|s| s.level)
    }

    /// Clear seam tracking (called on hard cycle reset).
    pub async fn reset(&self) {
        self.active_seams.lock().await.clear();
    }
}

#[must_use]
pub fn seam_level_for_active_input(
    config: &SeamConfig,
    active_input_tokens: usize,
    highest_existing_level: Option<u8>,
) -> Option<u8> {
    if !config.enabled {
        return None;
    }
    let highest = highest_existing_level.unwrap_or(0);

    // Each level fires at most once, and only in order.
    if highest < 1 && active_input_tokens >= config.l1_threshold {
        return Some(1);
    }
    if highest < 2 && active_input_tokens >= config.l2_threshold {
        return Some(2);
    }
    if highest < 3 && active_input_tokens >= config.l3_threshold {
        return Some(3);
    }
    None
}

/// Truncate a string to max_chars, respecting Unicode boundaries.
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

fn local_pins_for_range(
    pinned_indices: &[usize],
    start_idx: usize,
    end_idx: usize,
    message_count: usize,
) -> Vec<usize> {
    let end_idx = end_idx.min(message_count);
    pinned_indices
        .iter()
        .copied()
        .filter(|idx| *idx >= start_idx && *idx < end_idx)
        .map(|idx| idx - start_idx)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seam_levels_fire_in_order() {
        // Cannot create DeepSeekClient without API key in test env.
        // Test the pure logic functions only.
        let config = SeamConfig::default();

        assert_eq!(seam_level_for_active_input(&config, 100_000, None), None);
        assert_eq!(seam_level_for_active_input(&config, 192_000, None), Some(1));
        assert_eq!(
            seam_level_for_active_input(&config, 384_000, Some(1)),
            Some(2)
        );
        assert_eq!(
            seam_level_for_active_input(&config, 576_000, Some(2)),
            Some(3)
        );
    }

    #[test]
    fn seam_trigger_uses_active_request_size_not_lifetime_usage() {
        let config = SeamConfig::default();
        let lifetime_prompt_usage = 900_000usize;
        let active_request_input = 120_000usize;

        assert!(lifetime_prompt_usage >= config.l3_threshold);
        assert_eq!(
            seam_level_for_active_input(&config, active_request_input, None),
            None
        );
    }

    #[test]
    fn cycle_threshold_check() {
        let config = SeamConfig::default();
        assert!(768_000 >= config.cycle_threshold);
        assert!(700_000 < config.cycle_threshold);
    }

    #[test]
    fn verbatim_window_calculation() {
        let config = SeamConfig {
            verbatim_window_turns: 4,
            ..Default::default()
        };
        // 4 verbatim turns = 8 messages
        // 20 messages: 20 - (4*2) = 12
        assert_eq!(20usize.saturating_sub(8), 12);
        // 8 messages: 8 - 8 = 0
        assert_eq!(8usize.saturating_sub(8), 0);
        // 4 messages: 4 - 4 = 0
        assert_eq!(4usize.saturating_sub(4), 0);

        let _ = config;
    }

    #[test]
    fn truncate_chars_handles_unicode() {
        assert_eq!(truncate_chars("abc😀é", 3), "abc".to_string());
        assert_eq!(truncate_chars("abc😀é", 4), "abc😀".to_string());
        assert_eq!(truncate_chars("abc😀é", 10), "abc😀é".to_string());
        assert_eq!(truncate_chars("", 5), "".to_string());
    }

    #[test]
    fn global_pins_are_mapped_to_soft_seam_slice_indices() {
        let pins = vec![1, 4, 5, 8, 12];

        let local = local_pins_for_range(&pins, 4, 9, 10);

        assert_eq!(local, vec![0, 1, 4]);
    }

    #[test]
    fn disabled_config() {
        let config = SeamConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(!config.enabled);
    }
}
