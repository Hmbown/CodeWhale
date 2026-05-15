#![allow(dead_code)]

//! Markdown stream collector for live micro-chunk rendering.
//!
//! This module implements the pattern from codex-rs where:
//! - Streaming text is split into small grapheme-aligned chunks
//! - Commit ticks drip chunks into the transcript between provider deltas
//! - Final content is emitted when the stream ends

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::time::Instant;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::palette;

pub mod chunking;
pub mod commit_tick;
pub mod line_buffer;

pub use chunking::{AdaptiveChunkingPolicy, ChunkingMode};
pub use commit_tick::{StreamChunker, run_commit_tick};
pub use line_buffer::LineBuffer;
/// Collects streaming text and commits complete lines.
#[derive(Debug, Clone)]
pub struct MarkdownStreamCollector {
    /// Buffer for incoming text
    buffer: String,
    /// Number of lines already committed
    committed_line_count: usize,
    /// Terminal width for wrapping
    width: Option<usize>,
    /// Whether the stream is still active
    is_streaming: bool,
    /// Whether this is a thinking block
    is_thinking: bool,
}

impl Default for MarkdownStreamCollector {
    fn default() -> Self {
        // `is_streaming: true` matches `MarkdownStreamCollector::new` so a
        // freshly-default block behaves like a freshly-started stream.
        Self::new(None, false)
    }
}

impl MarkdownStreamCollector {
    /// Create a new collector
    pub fn new(width: Option<usize>, is_thinking: bool) -> Self {
        Self {
            buffer: String::new(),
            committed_line_count: 0,
            width,
            is_streaming: true,
            is_thinking,
        }
    }

    /// Push new content to the buffer
    pub fn push(&mut self, content: &str) {
        self.buffer.push_str(content);
    }

    /// Get the current buffer content (for display during streaming)
    pub fn current_content(&self) -> &str {
        &self.buffer
    }

    /// Check if there are complete lines to commit
    pub fn has_complete_lines(&self) -> bool {
        self.buffer.contains('\n')
    }

    /// Commit complete lines and return them.
    /// Only lines ending with '\n' are committed.
    /// Returns the newly committed lines since last call.
    pub fn commit_complete_lines(&mut self) -> Vec<Line<'static>> {
        let committed = self.commit_complete_text();
        if committed.is_empty() {
            return Vec::new();
        }
        self.render_lines(&committed)
    }

    /// Commit complete text chunks ending in a newline.
    /// Returns the raw text that became visible since the last call.
    pub fn commit_complete_text(&mut self) -> String {
        if self.buffer.is_empty() {
            return String::new();
        }

        // Find the last newline - only process up to there
        let Some(last_newline_idx) = self.buffer.rfind('\n') else {
            return String::new(); // No complete lines yet
        };

        // Extract the complete portion (up to and including last newline)
        let complete_portion = self.buffer[..=last_newline_idx].to_string();

        // Remove the committed portion from the buffer so finalize only emits the remainder
        self.buffer = self.buffer[last_newline_idx + 1..].to_string();
        self.committed_line_count = 0;

        complete_portion
    }

    /// Finalize the stream and return any remaining content.
    /// Call this when the stream ends to emit the final incomplete line.
    pub fn finalize(&mut self) -> Vec<Line<'static>> {
        let remaining = self.finalize_text();
        if remaining.is_empty() {
            return Vec::new();
        }
        self.render_lines(&remaining)
    }

    /// Finalize the stream and return any remaining raw text.
    pub fn finalize_text(&mut self) -> String {
        self.is_streaming = false;

        if self.buffer.is_empty() {
            return String::new();
        }

        let remaining = self.buffer.clone();
        self.buffer.clear();
        self.committed_line_count = 0;
        remaining
    }

    /// Get all rendered lines (for final display after stream ends)
    pub fn all_lines(&self) -> Vec<Line<'static>> {
        self.render_lines(&self.buffer)
    }

    /// Render content into styled lines
    fn render_lines(&self, content: &str) -> Vec<Line<'static>> {
        let width = self.width.unwrap_or(80);
        let style = if self.is_thinking {
            Style::default()
                .fg(palette::STATUS_WARNING)
                .add_modifier(Modifier::DIM | Modifier::ITALIC)
        } else {
            Style::default()
        };

        let mut lines = Vec::new();

        for line in content.lines() {
            // Wrap long lines
            let wrapped = wrap_line(line, width);
            for wrapped_line in wrapped {
                lines.push(Line::from(Span::styled(wrapped_line, style)));
            }
        }

        // Handle trailing newline (add empty line)
        if content.ends_with('\n') {
            lines.push(Line::from(""));
        }

        lines
    }

    /// Check if the stream is still active
    pub fn is_streaming(&self) -> bool {
        self.is_streaming
    }

    /// Get the raw buffer length
    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    /// Clear the buffer
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.committed_line_count = 0;
    }
}

/// Wrap a single line to fit within the given width
fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    if width == 0 {
        return vec![line.to_string()];
    }

    let mut result = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0;

    for word in line.split_whitespace() {
        let word_width = word.width();

        if word_width > width {
            if !current_line.is_empty() {
                result.push(std::mem::take(&mut current_line));
                current_width = 0;
            }
            push_word_breaking_chars(
                word,
                width,
                &mut current_line,
                &mut current_width,
                &mut result,
            );
            continue;
        }

        if current_width == 0 {
            // First word on line
            current_line = word.to_string();
            current_width = word_width;
        } else if current_width + 1 + word_width <= width {
            // Word fits with space
            current_line.push(' ');
            current_line.push_str(word);
            current_width += 1 + word_width;
        } else {
            // Word doesn't fit, start new line
            result.push(current_line);
            current_line = word.to_string();
            current_width = word_width;
        }
    }

    if !current_line.is_empty() {
        result.push(current_line);
    }

    if result.is_empty() {
        vec![String::new()]
    } else {
        result
    }
}

fn push_word_breaking_chars(
    word: &str,
    width: usize,
    current_line: &mut String,
    current_width: &mut usize,
    result: &mut Vec<String>,
) {
    for ch in word.chars() {
        let ch_width = ch.width().unwrap_or(1);
        if *current_width + ch_width > width && *current_width > 0 {
            result.push(std::mem::take(current_line));
            *current_width = 0;
        }
        current_line.push(ch);
        *current_width += ch_width;
    }
}

/// Per-block streaming substate: optional line-buffer feeding a collector +
/// chunker/policy for two-gear pacing.
///
/// Pipeline:
/// ```text
/// raw delta -> LineBuffer.push -> take_committable -> collector + chunker -> commit tick
/// ```
///
/// The [`LineBuffer`] remains available for line-sensitive modes. Normal
/// assistant prose and thinking blocks bypass it so text can stream in live
/// micro-chunks instead of waiting for newline boundaries.
#[derive(Debug, Default)]
struct BlockState {
    /// Newline gate: holds back trailing partial-line text between deltas.
    /// Bypassed when `bypass_gate` is true (thinking blocks).
    line_buffer: LineBuffer,
    /// Whether to bypass the [`LineBuffer`] (thinking blocks stream live).
    bypass_gate: bool,
    collector: MarkdownStreamCollector,
    chunker: StreamChunker,
    policy: AdaptiveChunkingPolicy,
}

/// State for managing multiple stream collectors (one per content block)
#[derive(Debug, Default)]
pub struct StreamingState {
    /// Per-block state by index (collector + chunker + policy).
    blocks: Vec<Option<BlockState>>,
    /// Whether any stream is currently active
    pub is_active: bool,
    /// Accumulated text for display
    pub accumulated_text: String,
    /// Accumulated thinking for display
    pub accumulated_thinking: String,
}

impl StreamingState {
    /// Create a new streaming state
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a new text block. Assistant prose streams live in micro-chunks so
    /// users can visually track the answer as it forms instead of waiting for
    /// a newline-terminated line.
    pub fn start_text(&mut self, index: usize, width: Option<usize>) {
        self.ensure_capacity(index);
        self.blocks[index] = Some(BlockState {
            line_buffer: LineBuffer::new(),
            bypass_gate: true,
            collector: MarkdownStreamCollector::new(width, false),
            chunker: StreamChunker::new(),
            policy: AdaptiveChunkingPolicy::new(),
        });
        self.is_active = true;
    }

    /// Start a new thinking block. Thinking deltas bypass the newline gate so
    /// they remain visually live — long reasoning often arrives as a single
    /// paragraph without intermediate newlines, and gating it would create
    /// long pauses where the user sees nothing.
    pub fn start_thinking(&mut self, index: usize, width: Option<usize>) {
        self.ensure_capacity(index);
        self.blocks[index] = Some(BlockState {
            line_buffer: LineBuffer::new(),
            bypass_gate: true,
            collector: MarkdownStreamCollector::new(width, true),
            chunker: StreamChunker::new(),
            policy: AdaptiveChunkingPolicy::new(),
        });
        self.is_active = true;
    }

    /// Push content to a block. Routing depends on the block kind:
    ///
    /// - Assistant text blocks: incoming bytes normally bypass [`LineBuffer`]
    ///   and are split into small display chunks downstream.
    /// - Thinking blocks: bytes bypass the gate and go straight to the
    ///   collector/chunker so reasoning stays visually live (long thoughts
    ///   often have no intermediate newlines).
    ///
    /// `accumulated_text` / `accumulated_thinking` always track the full raw
    /// stream so callers building API messages or doing retries see exactly
    /// what the model emitted, regardless of UI gating.
    pub fn push_content(&mut self, index: usize, content: &str) {
        if let Some(Some(block)) = self.blocks.get_mut(index) {
            // Always update the raw accumulator first — UI gating must not
            // affect what we send back to the model on retry/continuation.
            if block.collector.is_thinking {
                self.accumulated_thinking.push_str(content);
            } else {
                self.accumulated_text.push_str(content);
            }

            // Determine what bytes are safe to expose downstream on this push.
            let downstream: String = if block.bypass_gate {
                // Thinking: forward verbatim to collector + chunker.
                content.to_string()
            } else {
                // Assistant text: gate at the last-newline boundary.
                block.line_buffer.push(content);
                block.line_buffer.take_committable()
            };

            if downstream.is_empty() {
                return;
            }

            if block.bypass_gate {
                block.chunker.push_delta(&downstream);
            } else {
                block.collector.push(&downstream);
                let committed = block.collector.commit_complete_text();
                if !committed.is_empty() {
                    block.chunker.push_delta(&committed);
                }
            }
        }
    }

    /// Get newly committed lines from a block. (Legacy entry point that maps
    /// onto the chunker.)
    pub fn commit_lines(&mut self, index: usize) -> Vec<Line<'static>> {
        let text = self.commit_text(index);
        if text.is_empty() {
            return Vec::new();
        }
        // Re-render the text through the same path the collector used.
        let style = if self
            .blocks
            .get(index)
            .and_then(|b| b.as_ref())
            .is_some_and(|b| b.collector.is_thinking)
        {
            Style::default()
                .fg(palette::STATUS_WARNING)
                .add_modifier(Modifier::DIM | Modifier::ITALIC)
        } else {
            Style::default()
        };
        let mut lines = Vec::new();
        for line in text.lines() {
            lines.push(Line::from(Span::styled(line.to_string(), style)));
        }
        if text.ends_with('\n') {
            lines.push(Line::from(""));
        }
        lines
    }

    /// Run one commit-tick of the chunker policy and return any text safe to
    /// flush to the transcript on this tick. May be empty (Smooth-mode tick
    /// against an empty queue) or contain anywhere from one line up to the
    /// full backlog (CatchUp-mode burst drain).
    pub fn commit_text(&mut self, index: usize) -> String {
        if let Some(Some(block)) = self.blocks.get_mut(index) {
            let now = Instant::now();
            let out = run_commit_tick(&mut block.policy, &mut block.chunker, now);
            out.committed_text
        } else {
            String::new()
        }
    }

    /// Inspect the current chunking mode for a block (testing/observability).
    pub fn chunking_mode(&self, index: usize) -> Option<ChunkingMode> {
        self.blocks
            .get(index)
            .and_then(|b| b.as_ref())
            .map(|b| b.policy.mode())
    }

    /// Whether the chunker has queued content waiting to be flushed by the
    /// next commit tick. Useful for callers that want to drive an extra tick
    /// while the queue drains under Smooth-mode pacing.
    pub fn has_pending_chunker_lines(&self, index: usize) -> bool {
        self.blocks
            .get(index)
            .and_then(|b| b.as_ref())
            .is_some_and(|b| b.chunker.queued_lines() > 0)
    }

    /// Finalize a block and get remaining lines
    pub fn finalize_block(&mut self, index: usize) -> Vec<Line<'static>> {
        let text = self.finalize_block_text(index);
        if text.is_empty() {
            return Vec::new();
        }
        let style = if self
            .blocks
            .get(index)
            .and_then(|b| b.as_ref())
            .is_some_and(|b| b.collector.is_thinking)
        {
            Style::default()
                .fg(palette::STATUS_WARNING)
                .add_modifier(Modifier::DIM | Modifier::ITALIC)
        } else {
            Style::default()
        };
        let mut lines = Vec::new();
        for line in text.lines() {
            lines.push(Line::from(Span::styled(line.to_string(), style)));
        }
        if text.ends_with('\n') {
            lines.push(Line::from(""));
        }
        lines
    }

    /// Finalize a block and get remaining raw text. Drains the full pipeline
    /// in upstream-to-downstream order:
    ///
    /// 1. [`LineBuffer::flush`] returns any post-newline tail held by the gate.
    ///    For gated blocks this is critical — without it, a final partial
    ///    line (e.g. text the model emitted without a trailing newline before
    ///    the turn ended) would otherwise be stranded in the gate.
    /// 2. The collector's `finalize_text` releases any partial line it still
    ///    holds (relevant for the bypass path where the collector receives
    ///    raw deltas directly).
    /// 3. The chunker's `drain_remaining` releases queued whole-line text
    ///    that the policy hadn't yet committed.
    pub fn finalize_block_text(&mut self, index: usize) -> String {
        if let Some(Some(block)) = self.blocks.get_mut(index) {
            // Flush the gate first so any held tail rejoins the stream
            // before the collector/chunker drain. For thinking blocks the
            // gate is unused, so this is a no-op.
            let gate_tail = block.line_buffer.flush();
            if !gate_tail.is_empty() {
                block.collector.push(&gate_tail);
            }
            // Any newly committable text after the gate flush feeds the
            // chunker so drain order remains "queued-lines, then partial-tail".
            let post_flush = block.collector.commit_complete_text();
            if !post_flush.is_empty() {
                block.chunker.push_delta(&post_flush);
            }
            // Any unterminated tail still in the collector is returned raw.
            let tail = block.collector.finalize_text();
            // Any whole-line text held by the chunker is safe to emit now.
            let mut out = block.chunker.drain_remaining();
            if !tail.is_empty() {
                out.push_str(&tail);
            }
            self.check_active();
            out
        } else {
            String::new()
        }
    }

    /// Finalize all blocks
    pub fn finalize_all(&mut self) -> Vec<(usize, Vec<Line<'static>>)> {
        let mut result = Vec::new();
        let len = self.blocks.len();
        for i in 0..len {
            let lines = self.finalize_block(i);
            if !lines.is_empty() {
                result.push((i, lines));
            }
        }
        self.is_active = false;
        result
    }

    /// Propagate the low-motion flag to every block's chunking policy.
    /// When true, all policies stay in `Smooth` regardless of queue pressure,
    /// preventing CatchUp burst drains that would create sudden visual jumps.
    pub fn set_low_motion(&mut self, low_motion: bool) {
        for block in self.blocks.iter_mut().flatten() {
            block.policy.set_low_motion(low_motion);
        }
    }

    /// Check if any stream is still active
    fn check_active(&mut self) {
        self.is_active = self.blocks.iter().any(|b| {
            b.as_ref()
                .is_some_and(|state| state.collector.is_streaming())
        });
    }

    /// Ensure capacity for the given index
    fn ensure_capacity(&mut self, index: usize) {
        while self.blocks.len() <= index {
            self.blocks.push(None);
        }
    }

    /// Reset the streaming state
    pub fn reset(&mut self) {
        self.blocks.clear();
        self.is_active = false;
        self.accumulated_text.clear();
        self.accumulated_thinking.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn test_commit_complete_lines() {
        let mut collector = MarkdownStreamCollector::new(Some(80), false);

        // Push incomplete line
        collector.push("Hello ");
        let lines = collector.commit_complete_lines();
        assert!(lines.is_empty()); // No complete lines yet

        // Complete the line
        collector.push("World\n");
        let lines = collector.commit_complete_lines();
        assert_eq!(lines.len(), 2); // "Hello World" + empty line from trailing \n

        // Push more content
        collector.push("Second line");
        let lines = collector.commit_complete_lines();
        assert!(lines.is_empty()); // No new complete lines

        // Finalize
        let lines = collector.finalize();
        assert_eq!(lines.len(), 1); // "Second line"
    }

    #[test]
    fn test_wrap_line() {
        let result = wrap_line("This is a long line that should be wrapped", 20);
        assert!(result.len() > 1);
    }

    #[test]
    fn wrap_line_breaks_no_whitespace_cjk_text() {
        let text = "这是一个没有任何空格的中文长段落".repeat(12);
        let result = wrap_line(&text, 40);

        assert!(result.len() > 1);
        assert!(result.iter().all(|line| line.width() <= 40));
        assert_eq!(result.join(""), text);
    }

    #[test]
    fn wrap_line_breaks_first_overlong_word() {
        let text = "x".repeat(120);
        let result = wrap_line(&text, 40);

        assert_eq!(result.len(), 3);
        assert!(result.iter().all(|line| line.width() <= 40));
        assert_eq!(result.join(""), text);
    }

    #[test]
    fn assistant_text_streams_before_newline() {
        let mut state = StreamingState::new();
        state.start_text(0, None);
        state.push_content(0, "hello world");

        assert_eq!(state.commit_text(0), "hello world");
        assert!(!state.has_pending_chunker_lines(0));
    }

    #[test]
    fn thinking_text_streams_before_newline() {
        let mut state = StreamingState::new();
        state.start_thinking(0, None);
        state.push_content(0, "thinking deeply");

        assert_eq!(state.commit_text(0), "thinking deeply");
        assert!(!state.has_pending_chunker_lines(0));
    }

    #[test]
    fn finalize_preserves_uncommitted_micro_chunks() {
        let mut state = StreamingState::new();
        state.start_text(0, None);
        state.set_low_motion(true);
        state.push_content(0, "abc");
        assert_eq!(state.commit_text(0), "a");

        assert_eq!(state.finalize_block_text(0), "bc");
    }
}
