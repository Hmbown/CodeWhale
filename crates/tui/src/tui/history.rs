//! TUI rendering helpers for chat history and tool output.

use std::path::{Path, PathBuf};
use std::time::Instant;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::deepseek_theme::active_theme;
use crate::models::{ContentBlock, Message};
use crate::palette;
use crate::tools::review::ReviewOutput;
use crate::tui::app::TranscriptSpacing;
use crate::tui::diff_render;
use crate::tui::markdown_render;

// === Constants ===

use std::process::Command;
const TOOL_COMMAND_LINE_LIMIT: usize = 3;
const TOOL_OUTPUT_LINE_LIMIT: usize = 6;
const TOOL_TEXT_LIMIT: usize = 180;
const TOOL_HEADER_SUMMARY_LIMIT: usize = 56;
const TOOL_OUTPUT_HEAD_LINES: usize = 2;
const TOOL_OUTPUT_TAIL_LINES: usize = 2;
const TOOL_RUNNING_SYMBOLS: [&str; 4] = ["·", "◦", "•", "◦"];
// Spinner cadence per glyph. The status-animation tick (UI_STATUS_ANIMATION_MS
// = 360 ms) fires every two glyphs, so a full 4-glyph "heartbeat" lands in
// ~2.88 s — fast enough that the user sees motion within a few hundred ms of
// starting a tool, slow enough to read as a pulse rather than a strobe.
const TOOL_STATUS_SYMBOL_MS: u64 = 720;
/// Visual marker for the user role at the start of their message line. Solid
/// vertical bar — no animation; user input is a finished thing.
const USER_GLYPH: &str = "\u{258E}"; // ▎
/// Visual marker for the assistant role. Solid bullet that pulses at 2s
/// cycle while the response is streaming, holds full brightness when idle.
const ASSISTANT_GLYPH: &str = "\u{25CF}"; // ●
/// Transcript body left rail. Solid 1/8 block (`▏`) followed by a space —
/// used as a visual left-margin anchor for continuation lines, tool-card
/// detail rows, and affordance lines. Dimmed so it guides the eye without
/// competing with content.
const TRANSCRIPT_RAIL: &str = "\u{258F} "; // ▏ + space
/// Reasoning header opener. Replaces the spinner glyph on thinking cells —
/// reasoning is a slow exhale, not a tool spin.
const REASONING_OPENER: &str = "\u{2026}"; // …
/// Reasoning body left rail. Dashed (`╎`) instead of the solid `▏` block to
/// visually separate reasoning from message body and tool output.
const REASONING_RAIL: &str = "\u{254E} "; // ╎ + space
/// Trailing-line cursor on streaming reasoning. Anchored to the live colour
/// so the user sees where new tokens land.
const REASONING_CURSOR: &str = "\u{258E}"; // ▎
const TOOL_CARD_SUMMARY_LINES: usize = 4;
const THINKING_SUMMARY_LINE_LIMIT: usize = 4;
const TOOL_DONE_SYMBOL: &str = "•";
const TOOL_FAILED_SYMBOL: &str = "•";

/// Render mode controlling whether tool/thinking cells render their compact
/// "live" form (with caps and collapsed reasoning) or their full transcript
/// form (uncapped, suitable for the pager / clipboard / message export).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// Live in-stream view: thinking is collapsed to a summary, tool output is
    /// truncated with a "Alt+V for details" affordance.
    Live,
    /// Full transcript view: every line of reasoning and tool output is
    /// emitted, no caps, no affordance.
    Transcript,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThinkingVisualState {
    Live,
    Done,
    Idle,
}

// === History Cells ===

/// Renderable history cell for user/assistant/system entries.
#[derive(Debug, Clone)]
pub enum HistoryCell {
    User {
        content: String,
    },
    Assistant {
        content: String,
        streaming: bool,
    },
    System {
        content: String,
    },
    /// Categorized engine-error cell. Severity drives the label glyph + color
    /// (red for `Error`/`Critical`, amber for `Warning`, dim for `Info`) so
    /// the user can prioritize at a glance.
    Error {
        message: String,
        severity: crate::error_taxonomy::ErrorSeverity,
    },
    Thinking {
        content: String,
        streaming: bool,
        duration_secs: Option<f32>,
    },
    /// An `<archived_context>` seam block produced by the Flash seam manager
    /// (issue #159). Rendered dimmed/italic with a level + range label so
    /// the user can see at a glance where context seams exist.
    ArchivedContext {
        /// Seam level (1, 2, 3, or 0 for cycle-level).
        level: u8,
        /// Message range covered (e.g. "msg 0-128").
        range: String,
        /// Token estimate string (e.g. "~2500").
        tokens: String,
        /// Density label (e.g. "~2,500 tokens").
        density: String,
        /// Model that produced the summary.
        model: String,
        /// RFC 3339 timestamp.
        timestamp: String,
        /// The summary text content.
        summary: String,
    },
    Tool(ToolCell),
    /// Live in-transcript card for sub-agent activity (issue #128). Owns
    /// either a single `DelegateCard` or a multi-worker `FanoutCard`; the
    /// UI re-binds it from the mailbox stream as envelopes arrive.
    SubAgent(SubAgentCell),
}

/// In-transcript sub-agent cell — either a single delegate or a fanout.
/// State mutates over the turn as mailbox envelopes are drained.
#[derive(Debug, Clone)]
pub enum SubAgentCell {
    Delegate(crate::tui::widgets::agent_card::DelegateCard),
    Fanout(crate::tui::widgets::agent_card::FanoutCard),
}

impl SubAgentCell {
    pub fn lines(&self, width: u16) -> Vec<Line<'static>> {
        match self {
            SubAgentCell::Delegate(card) => card.render_lines(width),
            SubAgentCell::Fanout(card) => card.render_lines(width),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptRenderOptions {
    pub show_thinking: bool,
    pub verbose: bool,
    pub show_tool_details: bool,
    pub calm_mode: bool,
    pub low_motion: bool,
    pub spacing: TranscriptSpacing,
}

impl Default for TranscriptRenderOptions {
    fn default() -> Self {
        Self {
            show_thinking: true,
            verbose: false,
            show_tool_details: true,
            calm_mode: false,
            low_motion: false,
            spacing: TranscriptSpacing::Comfortable,
        }
    }
}

impl HistoryCell {
    /// Render the cell into a set of terminal lines.
    ///
    /// This is the live-display path used by widgets that don't already pass
    /// `TranscriptRenderOptions`. Tool output is capped, but thinking is shown
    /// in full because callers using bare `lines()` historically expected the
    /// uncollapsed body. For the in-stream transcript view prefer
    /// `lines_with_options`; for the pager / clipboard prefer
    /// `transcript_lines`.
    pub fn lines(&self, width: u16) -> Vec<Line<'static>> {
        match self {
            HistoryCell::User { content } => render_message(
                USER_GLYPH,
                user_label_style(),
                user_body_style(),
                content,
                width,
            ),
            HistoryCell::Assistant { content, streaming } => render_message(
                ASSISTANT_GLYPH,
                assistant_label_style_for(*streaming, /*low_motion*/ false),
                message_body_style(),
                content,
                width,
            ),
            HistoryCell::System { content } => {
                if is_cycle_boundary(content) {
                    render_cycle_boundary(content, width)
                } else {
                    render_message(
                        "Note",
                        system_label_style(),
                        system_body_style(),
                        content,
                        width,
                    )
                }
            }
            HistoryCell::Error { message, severity } => {
                // Error messages are machine-generated and should not be run
                // through markdown rendering, which would mangle env-var names
                // containing underscores (e.g. DEEPSEEK_ALLOW_INSECURE_HTTP
                // would lose its underscores as italic markers).
                let label = error_label_text(*severity);
                let label_style = error_label_style(*severity);
                let body_style = error_body_style(*severity);
                let prefix_width = UnicodeWidthStr::width(label);
                let content_width = width.saturating_sub(2 + prefix_width as u16).max(1);
                let mut lines = wrap_plain_line(message, body_style, content_width);
                // Add the label prefix to the first line
                if let Some(first) = lines.get_mut(0) {
                    first.spans.insert(0, Span::raw(" "));
                    first.spans.insert(0, Span::styled(label, label_style));
                }
                // Continuation rail for subsequent lines
                let rail = format!("{}{}", '\u{258F}', " ".repeat(prefix_width));
                let rail_style = Style::default().fg(palette::TEXT_DIM);
                for line in lines.iter_mut().skip(1) {
                    line.spans.insert(0, Span::styled(rail.clone(), rail_style));
                }
                lines
            }
            HistoryCell::Thinking {
                content,
                streaming,
                duration_secs,
            } => render_thinking(content, width, *streaming, *duration_secs, false, false),
            HistoryCell::Tool(cell) => cell.lines_with_motion(width, false),
            HistoryCell::SubAgent(cell) => cell.lines(width),
            HistoryCell::ArchivedContext { .. } => render_archived_context(self, width, false),
        }
    }

    pub fn lines_with_options(
        &self,
        width: u16,
        options: TranscriptRenderOptions,
    ) -> Vec<Line<'static>> {
        match self {
            HistoryCell::Thinking { .. } if !options.show_thinking => Vec::new(),
            HistoryCell::Thinking {
                content,
                streaming,
                duration_secs,
            } => render_thinking(
                content,
                width,
                *streaming,
                *duration_secs,
                !options.verbose,
                options.low_motion,
            ),
            HistoryCell::Tool(cell) if !options.show_tool_details => {
                let mut lines = cell.lines_with_motion(width, options.low_motion);
                if lines.len() > 2 {
                    lines.truncate(2);
                    lines.push(details_affordance_line(
                        "details hidden",
                        Style::default().fg(palette::TEXT_MUTED).italic(),
                    ));
                }
                lines
            }
            HistoryCell::Tool(cell) if options.calm_mode => {
                let mut lines = cell.lines_with_motion(width, options.low_motion);
                if lines.len() > TOOL_CARD_SUMMARY_LINES {
                    lines.truncate(TOOL_CARD_SUMMARY_LINES);
                    lines.push(details_affordance_line(
                        "Alt+V for details",
                        Style::default().fg(palette::TEXT_MUTED).italic(),
                    ));
                }
                lines
            }
            HistoryCell::Tool(cell) => cell.lines_with_motion(width, options.low_motion),
            HistoryCell::User { content } => render_message(
                USER_GLYPH,
                user_label_style(),
                user_body_style(),
                content,
                width,
            ),
            HistoryCell::Assistant { content, streaming } => render_message(
                ASSISTANT_GLYPH,
                assistant_label_style_for(*streaming, options.low_motion),
                message_body_style(),
                content,
                width,
            ),
            HistoryCell::System { .. } | HistoryCell::Error { .. } => self.lines(width),
            HistoryCell::SubAgent(cell) => cell.lines(width),
            HistoryCell::ArchivedContext { .. } => {
                render_archived_context(self, width, options.low_motion)
            }
        }
    }

    /// Render the cell in transcript mode: full content, no caps, no
    /// "Alt+V for details" affordances.
    ///
    /// Use this for the pager (`v` / `Ctrl+O`), clipboard exports, and any
    /// surface that wants the complete body rather than the live summary.
    /// For most variants (User / Assistant / System) this matches `lines()`;
    /// `Thinking` and `Tool` are where the live and transcript surfaces
    /// diverge.
    pub fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        match self {
            HistoryCell::User { content } => render_message(
                USER_GLYPH,
                user_label_style(),
                user_body_style(),
                content,
                width,
            ),
            HistoryCell::Assistant { content, streaming } => render_message(
                ASSISTANT_GLYPH,
                // Pager / clipboard surface — pin the glyph at full
                // brightness so a screenshot reads the same as a live frame.
                assistant_label_style_for(*streaming, /*low_motion*/ true),
                message_body_style(),
                content,
                width,
            ),
            HistoryCell::System { .. } | HistoryCell::Error { .. } => self.lines(width),
            HistoryCell::Thinking {
                content,
                streaming,
                duration_secs,
            } => render_thinking(
                content,
                width,
                *streaming,
                *duration_secs,
                /*collapsed*/ false,
                /*low_motion*/ false,
            ),
            HistoryCell::Tool(cell) => cell.transcript_lines(width),
            HistoryCell::SubAgent(cell) => cell.lines(width),
            HistoryCell::ArchivedContext { .. } => render_archived_context(self, width, true),
        }
    }

    /// Whether this cell is the continuation of a streaming assistant message.
    #[must_use]
    pub fn is_stream_continuation(&self) -> bool {
        matches!(
            self,
            HistoryCell::Assistant {
                streaming: true,
                ..
            }
        )
    }

    #[must_use]
    pub fn is_conversational(&self) -> bool {
        matches!(
            self,
            HistoryCell::User { .. } | HistoryCell::Assistant { .. } | HistoryCell::Thinking { .. }
        )
    }
}

/// Parse an `<archived_context>` block from an assistant Text block.
///
/// Returns `Some(HistoryCell::ArchivedContext)` when the text contains a
/// well-formed `<archived_context>...</archived_context>` block, or `None`
/// if the text is regular assistant content.
fn parse_archived_context(text: &str) -> Option<HistoryCell> {
    let text = text.trim();
    if !text.starts_with("<archived_context") || !text.ends_with("</archived_context>") {
        return None;
    }

    let tag_end = text.find('>')?;
    let tag = &text[..tag_end];

    let level = archived_context_attr(tag, "level")
        .and_then(|v| v.parse::<u8>().ok())
        .unwrap_or(0);

    let range = archived_context_attr(tag, "range").unwrap_or_default();

    let tokens = archived_context_attr(tag, "tokens").unwrap_or_default();

    let density = archived_context_attr(tag, "density").unwrap_or_default();

    let model = archived_context_attr(tag, "model").unwrap_or_default();

    let timestamp = archived_context_attr(tag, "timestamp").unwrap_or_default();

    let close_tag = text.rfind("</archived_context>")?;
    let summary_start = tag_end + 1;
    let summary = text[summary_start..close_tag].trim().to_string();

    Some(HistoryCell::ArchivedContext {
        level,
        range,
        tokens,
        density,
        model,
        timestamp,
        summary,
    })
}

fn archived_context_attr(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Render an `<archived_context>` block with dimmed/italic styling.
fn render_archived_context(
    cell: &HistoryCell,
    width: u16,
    _low_motion: bool,
) -> Vec<Line<'static>> {
    let HistoryCell::ArchivedContext {
        level,
        range,
        tokens,
        density,
        model,
        timestamp,
        summary,
    } = cell
    else {
        return Vec::new();
    };

    let body = if summary.is_empty() {
        "(no summary)".to_string()
    } else {
        summary.clone()
    };

    let label = format!("Context L{level}");
    let label_style = Style::default()
        .fg(palette::TEXT_DIM)
        .add_modifier(Modifier::BOLD);
    let body_style = Style::default().fg(palette::TEXT_DIM).italic();

    let content_width = width.saturating_sub(4).max(1);

    let mut lines = Vec::new();

    let range_display = if range.is_empty() {
        String::new()
    } else {
        range.to_string()
    };
    let mut header = format!("{label}  {range_display}");
    if !tokens.is_empty() {
        header.push_str(&format!("  {tokens}"));
    }
    if !density.is_empty() && density != tokens {
        header.push_str(&format!("  {density}"));
    }
    lines.push(Line::from(Span::styled(header, label_style)));

    let model_display = if model.is_empty() {
        String::new()
    } else {
        format!("via {model}")
    };
    let ts_display = if timestamp.is_empty() {
        String::new()
    } else {
        timestamp.clone()
    };
    let mut sub = String::new();
    if !model_display.is_empty() {
        sub.push_str(&model_display);
    }
    if !ts_display.is_empty() {
        if !sub.is_empty() {
            sub.push_str(" · ");
        }
        sub.push_str(&ts_display);
    }
    if !sub.is_empty() {
        lines.push(Line::from(Span::styled(
            sub,
            Style::default().fg(palette::TEXT_MUTED),
        )));
    }

    let rendered = crate::tui::markdown_render::render_markdown(&body, content_width, body_style);
    for (idx, line) in rendered.into_iter().enumerate() {
        if idx == 0 {
            let mut spans = vec![Span::styled(
                TRANSCRIPT_RAIL.to_string(),
                Style::default().fg(palette::TEXT_DIM),
            )];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        } else {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
    }

    lines.push(Line::from(""));

    lines
}

/// Convert a message into history cells for rendering.
#[must_use]
pub fn history_cells_from_message(msg: &Message) -> Vec<HistoryCell> {
    let mut cells = Vec::new();

    for block in &msg.content {
        match block {
            ContentBlock::Text { text, .. } => {
                // Check if this is an `<archived_context>` block.
                if msg.role == "assistant"
                    && let Some(archived) = parse_archived_context(text)
                {
                    cells.push(archived);
                    continue;
                }
                match msg.role.as_str() {
                    "user" => {
                        if let Some(HistoryCell::User { content }) = cells.last_mut() {
                            if !content.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(text);
                        } else {
                            cells.push(HistoryCell::User {
                                content: text.clone(),
                            });
                        }
                    }
                    "assistant" => {
                        if let Some(HistoryCell::Assistant { content, .. }) = cells.last_mut() {
                            if !content.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(text);
                        } else {
                            cells.push(HistoryCell::Assistant {
                                content: text.clone(),
                                streaming: false,
                            });
                        }
                    }
                    "system" => {
                        if let Some(HistoryCell::System { content }) = cells.last_mut() {
                            if !content.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(text);
                        } else {
                            cells.push(HistoryCell::System {
                                content: text.clone(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            ContentBlock::Thinking { thinking } => {
                if let Some(HistoryCell::Thinking { content, .. }) = cells.last_mut() {
                    if !content.is_empty() {
                        content.push('\n');
                    }
                    content.push_str(thinking);
                } else {
                    cells.push(HistoryCell::Thinking {
                        content: thinking.clone(),
                        streaming: false,
                        duration_secs: None,
                    });
                }
            }
            _ => {}
        }
    }

    cells
}

// === Tool Cells ===

/// Variants describing a tool result cell.
#[derive(Debug, Clone)]
pub enum ToolCell {
    Exec(ExecCell),
    Exploring(ExploringCell),
    PlanUpdate(PlanUpdateCell),
    PatchSummary(PatchSummaryCell),
    Review(ReviewCell),
    DiffPreview(DiffPreviewCell),
    Mcp(McpToolCell),
    ViewImage(ViewImageCell),
    WebSearch(WebSearchCell),
    Generic(GenericToolCell),
}

impl ToolCell {
    /// Render the tool cell into lines.
    pub fn lines(&self, width: u16) -> Vec<Line<'static>> {
        self.lines_with_motion(width, false)
    }

    pub fn lines_with_motion(&self, width: u16, low_motion: bool) -> Vec<Line<'static>> {
        self.render(width, low_motion, RenderMode::Live)
    }

    /// Full-content rendering for the pager / clipboard. Tool output that
    /// would be capped + suffixed with "Alt+V for details" in the live view
    /// is emitted in full here.
    pub fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.render(width, /*low_motion*/ false, RenderMode::Transcript)
    }

    fn render(&self, width: u16, low_motion: bool, mode: RenderMode) -> Vec<Line<'static>> {
        match self {
            ToolCell::Exec(cell) => cell.render(width, low_motion, mode),
            ToolCell::Exploring(cell) => cell.lines_with_motion(width, low_motion),
            ToolCell::PlanUpdate(cell) => cell.lines_with_motion(width, low_motion),
            ToolCell::PatchSummary(cell) => cell.render(width, low_motion, mode),
            ToolCell::Review(cell) => cell.render(width, low_motion, mode),
            ToolCell::DiffPreview(cell) => cell.lines_with_motion(width, low_motion),
            ToolCell::Mcp(cell) => cell.render(width, low_motion, mode),
            ToolCell::ViewImage(cell) => cell.lines_with_motion(width, low_motion),
            ToolCell::WebSearch(cell) => cell.lines_with_motion(width, low_motion),
            ToolCell::Generic(cell) => cell.lines_with_mode(width, low_motion, mode),
        }
    }
}

/// Overall status for a tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Success,
    Failed,
}

/// Shell command execution rendering data.
#[derive(Debug, Clone)]
pub struct ExecCell {
    pub command: String,
    pub status: ToolStatus,
    pub output: Option<String>,
    pub started_at: Option<Instant>,
    pub duration_ms: Option<u64>,
    pub source: ExecSource,
    pub interaction: Option<String>,
    /// Cached output summary — avoids re-parsing JSON every frame.
    pub output_summary: Option<String>,
}

impl ExecCell {
    /// Render the execution cell into lines (live view, capped output).
    #[cfg(test)]
    pub fn lines_with_motion(&self, width: u16, low_motion: bool) -> Vec<Line<'static>> {
        self.render(width, low_motion, RenderMode::Live)
    }

    pub(super) fn render(
        &self,
        width: u16,
        low_motion: bool,
        mode: RenderMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let command_summary = command_header_summary(&self.command);
        let header_summary = self
            .interaction
            .as_deref()
            .or(Some(command_summary.as_str()));
        lines.push(render_tool_header_with_summary(
            "Shell",
            header_summary,
            tool_status_label(self.status),
            self.status,
            self.started_at,
            low_motion,
        ));

        if self.status == ToolStatus::Success && self.source == ExecSource::User {
            lines.extend(render_compact_kv(
                "source",
                "started by you",
                Style::default().fg(palette::TEXT_MUTED),
                width,
            ));
        }

        if let Some(interaction) = self.interaction.as_ref() {
            lines.extend(wrap_plain_line(
                &format!("  {interaction}"),
                Style::default().fg(palette::TEXT_MUTED),
                width,
            ));
        } else {
            lines.extend(render_command_mode(&self.command, width, mode));
        }

        if self.interaction.is_none() {
            if let Some(output) = self.output.as_ref() {
                lines.extend(render_exec_output_mode(
                    output,
                    width,
                    TOOL_OUTPUT_LINE_LIMIT,
                    mode,
                ));
            } else if self.status == ToolStatus::Running && self.source == ExecSource::Assistant {
                lines.extend(wrap_plain_line(
                    "  Ctrl+B opens shell controls.",
                    Style::default().fg(palette::TEXT_MUTED),
                    width,
                ));
            } else if self.status != ToolStatus::Running {
                lines.push(Line::from(Span::styled(
                    "  (no output)",
                    Style::default().fg(palette::TEXT_MUTED).italic(),
                )));
            }
        }

        if let Some(duration_ms) = self.duration_ms {
            let seconds = f64::from(u32::try_from(duration_ms).unwrap_or(u32::MAX)) / 1000.0;
            lines.extend(render_compact_kv(
                "time",
                &format!("{seconds:.2}s"),
                Style::default().fg(palette::TEXT_DIM),
                width,
            ));
        }

        wrap_card_rail(lines)
    }
}

/// Source of a shell command execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecSource {
    User,
    Assistant,
}

/// Aggregate cell for tool exploration runs.
#[derive(Debug, Clone)]
pub struct ExploringCell {
    pub entries: Vec<ExploringEntry>,
}

impl ExploringCell {
    /// Render the exploring cell into lines.
    pub fn lines_with_motion(&self, width: u16, low_motion: bool) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let all_done = self
            .entries
            .iter()
            .all(|entry| entry.status != ToolStatus::Running);
        let status = if all_done {
            ToolStatus::Success
        } else {
            ToolStatus::Running
        };
        let header_summary = exploring_header_summary(&self.entries);
        lines.push(render_tool_header_with_summary(
            "Workspace",
            header_summary.as_deref(),
            if all_done { "done" } else { "running" },
            status,
            None,
            low_motion,
        ));

        for entry in &self.entries {
            let prefix = match entry.status {
                ToolStatus::Running => "live",
                ToolStatus::Success => "done",
                ToolStatus::Failed => "issue",
            };
            lines.extend(render_compact_kv(
                prefix,
                &entry.label,
                tool_value_style(),
                width,
            ));
        }
        lines
    }

    /// Insert a new entry and return its index.
    #[must_use]
    pub fn insert_entry(&mut self, entry: ExploringEntry) -> usize {
        self.entries.push(entry);
        self.entries.len().saturating_sub(1)
    }
}

/// Single entry for exploring tool output.
#[derive(Debug, Clone)]
pub struct ExploringEntry {
    pub label: String,
    pub status: ToolStatus,
}

/// Cell for plan updates emitted by the plan tool.
#[derive(Debug, Clone)]
pub struct PlanUpdateCell {
    pub explanation: Option<String>,
    pub steps: Vec<PlanStep>,
    pub status: ToolStatus,
}

impl PlanUpdateCell {
    /// Render the plan update cell into lines.
    pub fn lines_with_motion(&self, width: u16, low_motion: bool) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        lines.push(render_tool_header(
            "Plan",
            tool_status_label(self.status),
            self.status,
            None,
            low_motion,
        ));

        if let Some(explanation) = self.explanation.as_ref() {
            lines.extend(render_message(
                "",
                system_label_style(),
                system_body_style(),
                explanation,
                width,
            ));
        }

        for step in &self.steps {
            let marker = match step.status.as_str() {
                "completed" => "done",
                "in_progress" => "live",
                _ => "next",
            };
            lines.extend(render_compact_kv(
                marker,
                &step.step,
                tool_value_style(),
                width,
            ));
        }

        lines
    }
}

/// Single plan step rendered in the UI.
#[derive(Debug, Clone)]
pub struct PlanStep {
    pub step: String,
    pub status: String,
}

/// Cell for patch summaries emitted by the patch tool.
#[derive(Debug, Clone)]
pub struct PatchSummaryCell {
    pub path: String,
    pub summary: String,
    pub status: ToolStatus,
    pub error: Option<String>,
}

impl PatchSummaryCell {
    pub(super) fn render(
        &self,
        width: u16,
        low_motion: bool,
        mode: RenderMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        lines.push(render_tool_header_with_summary(
            "Patch",
            Some(&self.path),
            tool_status_label(self.status),
            self.status,
            None,
            low_motion,
        ));
        lines.extend(render_compact_kv(
            "file",
            &self.path,
            tool_value_style(),
            width,
        ));
        lines.extend(render_tool_output_mode(
            &self.summary,
            width,
            TOOL_COMMAND_LINE_LIMIT,
            mode,
        ));
        if let Some(error) = self.error.as_ref() {
            lines.extend(render_tool_output_mode(
                error,
                width,
                TOOL_COMMAND_LINE_LIMIT,
                mode,
            ));
        }
        lines
    }
}

/// Cell for structured review output.
#[derive(Debug, Clone)]
pub struct ReviewCell {
    pub target: String,
    pub status: ToolStatus,
    pub output: Option<ReviewOutput>,
    pub error: Option<String>,
}

impl ReviewCell {
    pub(super) fn render(
        &self,
        width: u16,
        low_motion: bool,
        mode: RenderMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        lines.push(render_tool_header(
            "Review",
            tool_status_label(self.status),
            self.status,
            None,
            low_motion,
        ));

        if !self.target.trim().is_empty() {
            lines.extend(render_compact_kv(
                "target",
                self.target.trim(),
                tool_value_style(),
                width,
            ));
        }

        if self.status == ToolStatus::Running {
            return lines;
        }

        if let Some(error) = self.error.as_ref() {
            lines.extend(render_tool_output_mode(
                error,
                width,
                TOOL_COMMAND_LINE_LIMIT,
                mode,
            ));
            return lines;
        }

        let Some(output) = self.output.as_ref() else {
            return lines;
        };

        if !output.summary.trim().is_empty() {
            lines.extend(wrap_plain_line(
                &format!("Summary: {}", output.summary.trim()),
                Style::default().fg(palette::TEXT_PRIMARY),
                width,
            ));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Issues",
            Style::default()
                .fg(palette::DEEPSEEK_BLUE)
                .add_modifier(Modifier::BOLD),
        )));
        if output.issues.is_empty() {
            lines.extend(wrap_plain_line(
                "  (none)",
                Style::default().fg(palette::TEXT_MUTED),
                width,
            ));
        } else {
            for issue in &output.issues {
                let severity = issue.severity.trim().to_ascii_lowercase();
                let color = review_severity_color(&severity);
                let location = format_review_location(issue.path.as_ref(), issue.line);
                let label = if location.is_empty() {
                    format!("  - [{}] {}", severity, issue.title.trim())
                } else {
                    format!("  - [{}] {} ({})", severity, issue.title.trim(), location)
                };
                lines.extend(wrap_plain_line(&label, Style::default().fg(color), width));
                if !issue.description.trim().is_empty() {
                    lines.extend(wrap_plain_line(
                        &format!("    {}", issue.description.trim()),
                        Style::default().fg(palette::TEXT_MUTED),
                        width,
                    ));
                }
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Suggestions",
            Style::default()
                .fg(palette::DEEPSEEK_BLUE)
                .add_modifier(Modifier::BOLD),
        )));
        if output.suggestions.is_empty() {
            lines.extend(wrap_plain_line(
                "  (none)",
                Style::default().fg(palette::TEXT_MUTED),
                width,
            ));
        } else {
            for suggestion in &output.suggestions {
                let location = format_review_location(suggestion.path.as_ref(), suggestion.line);
                let label = if location.is_empty() {
                    format!("  - {}", suggestion.suggestion.trim())
                } else {
                    format!("  - {} ({})", suggestion.suggestion.trim(), location)
                };
                lines.extend(wrap_plain_line(
                    &label,
                    Style::default().fg(palette::TEXT_PRIMARY),
                    width,
                ));
            }
        }

        if !output.overall_assessment.trim().is_empty() {
            lines.push(Line::from(""));
            lines.extend(wrap_plain_line(
                &format!("Overall: {}", output.overall_assessment.trim()),
                Style::default().fg(palette::TEXT_PRIMARY),
                width,
            ));
        }

        lines
    }
}

/// Cell for showing a diff preview before applying changes.
#[derive(Debug, Clone)]
pub struct DiffPreviewCell {
    pub title: String,
    pub diff: String,
}

impl DiffPreviewCell {
    pub fn lines_with_motion(&self, width: u16, low_motion: bool) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let diff_summary = diff_render::diff_summary_label(&self.diff);
        lines.push(render_tool_header_with_summary(
            "Diff",
            diff_summary.as_deref(),
            "done",
            ToolStatus::Success,
            None,
            low_motion,
        ));
        lines.extend(render_compact_kv(
            "title",
            &self.title,
            tool_value_style(),
            width,
        ));
        lines.extend(diff_render::render_diff(&self.diff, width));
        lines
    }
}

/// Cell representing an MCP tool execution.
#[derive(Debug, Clone)]
pub struct McpToolCell {
    pub tool: String,
    pub status: ToolStatus,
    pub content: Option<String>,
    pub is_image: bool,
}

impl McpToolCell {
    pub(super) fn render(
        &self,
        width: u16,
        low_motion: bool,
        mode: RenderMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        lines.push(render_tool_header_with_summary(
            "Tool",
            Some(&self.tool),
            tool_status_label(self.status),
            self.status,
            None,
            low_motion,
        ));
        lines.extend(render_compact_kv(
            "name",
            &self.tool,
            tool_value_style(),
            width,
        ));

        if self.is_image {
            lines.extend(render_compact_kv(
                "result",
                "image",
                tool_value_style(),
                width,
            ));
        }

        if let Some(content) = self.content.as_ref() {
            lines.extend(render_tool_output_mode(
                content,
                width,
                TOOL_COMMAND_LINE_LIMIT,
                mode,
            ));
        }
        lines
    }
}

/// Cell for image view actions.
#[derive(Debug, Clone)]
pub struct ViewImageCell {
    pub path: PathBuf,
}

impl ViewImageCell {
    /// Render the image view cell into lines.
    pub fn lines_with_motion(&self, width: u16, low_motion: bool) -> Vec<Line<'static>> {
        let path = self.path.display().to_string();
        let mut lines = vec![render_tool_header_with_summary(
            "Image",
            Some(&path),
            "done",
            ToolStatus::Success,
            None,
            low_motion,
        )];
        lines.extend(render_compact_kv("path", &path, tool_value_style(), width));
        lines
    }
}

/// Cell for web search tool output.
#[derive(Debug, Clone)]
pub struct WebSearchCell {
    pub query: String,
    pub status: ToolStatus,
    pub summary: Option<String>,
}

impl WebSearchCell {
    /// Render the web search cell into lines.
    pub fn lines_with_motion(&self, width: u16, low_motion: bool) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        lines.push(render_tool_header_with_summary(
            "Search",
            Some(&self.query),
            tool_status_label(self.status),
            self.status,
            None,
            low_motion,
        ));
        lines.extend(render_compact_kv(
            "query",
            &self.query,
            tool_value_style(),
            width,
        ));
        if let Some(summary) = self.summary.as_ref() {
            lines.extend(render_compact_kv(
                "result",
                summary,
                tool_value_style(),
                width,
            ));
        }
        lines
    }
}

/// Generic cell for tool output when no specialized rendering exists.
#[derive(Debug, Clone)]
pub struct GenericToolCell {
    pub name: String,
    pub status: ToolStatus,
    pub input_summary: Option<String>,
    pub output: Option<String>,
    /// Optional list of per-child prompts. When populated (by any future
    /// fan-out tool), each prompt is shown on its own indented row instead
    /// of the inline `args:` summary. `None` for ordinary tools.
    pub prompts: Option<Vec<String>>,
    /// Filesystem path to the full output's spillover file (#422/#423).
    /// Set by the tool-routing layer when `ToolResult.metadata` carried a
    /// `spillover_path` field. The truncation affordance includes the
    /// path so the user can `read_file` it (or Cmd+click in
    /// OSC 8-aware terminals — the path renders as a hyperlink when
    /// `tui.osc8_links` is enabled).
    pub spillover_path: Option<std::path::PathBuf>,
    // --- Pre-computed render cache (populated once at cell creation) ---
    /// Cached output summary — avoids re-parsing JSON every frame.
    pub output_summary: Option<String>,
    /// Whether the output looks like a unified diff (cached after first check).
    pub is_diff: bool,
}

impl GenericToolCell {
    /// Render the generic tool cell into lines.
    ///
    /// `mode` controls multi-line output handling: `Live` caps at
    /// `TOOL_OUTPUT_LINE_LIMIT` rows with a "+N more" affordance;
    /// `Transcript` emits the full output.
    pub fn lines_with_mode(
        &self,
        width: u16,
        low_motion: bool,
        mode: RenderMode,
    ) -> Vec<Line<'static>> {
        // Issue #241: when the underlying tool is a checklist/todo update and
        // the output is parseable, render a purpose-built progress card
        // instead of dumping the JSON into the generic tool block.
        if let Some(lines) = self.try_render_as_checklist(width, low_motion, mode) {
            return lines;
        }

        // Issue #409: `agent_spawn` already gets a dedicated `DelegateCard`
        // that owns the live action tree, status, and final summary. The
        // generic tool block for the same call duplicates that signal at
        // 3-4 lines per spawn — N parallel spawns multiply the noise. In
        // live mode, render one compact summary line and let the
        // DelegateCard be the source of truth. Transcript mode keeps the
        // full block so session replay remains complete.
        if matches!(mode, RenderMode::Live) && self.name == "agent_spawn" {
            return self.render_agent_spawn_compact(low_motion);
        }

        let mut lines = Vec::new();
        // Map the actual tool name (e.g. `agent_spawn`, `apply_patch`) to a
        // family rather than the catch-all `"Tool"` title — this is what
        // gives a `GenericToolCell` the right verb glyph (◐ delegate, ⋮⋮
        // fanout, etc.) instead of falling back to the neutral bullet.
        let family = crate::tui::widgets::tool_card::tool_family_for_name(&self.name);
        let header_summary = crate::tui::widgets::tool_card::tool_header_summary_for_name(
            &self.name,
            self.input_summary.as_deref(),
        );
        lines.push(render_tool_header_with_family_and_summary(
            family,
            header_summary.as_deref(),
            tool_status_label(self.status),
            self.status,
            None,
            low_motion,
        ));
        lines.extend(render_compact_kv(
            "name",
            &self.name,
            tool_value_style(),
            width,
        ));

        // Prefer per-prompt rows over the generic args summary when the tool
        // exposes a list of child prompts. One row per child with a `[i]`
        // index makes the fan-out legible without expanding JSON.
        let show_prompts = matches!(self.status, ToolStatus::Running) || self.output.is_none();
        if show_prompts
            && let Some(prompts) = self.prompts.as_ref()
            && !prompts.is_empty()
        {
            for (idx, prompt) in prompts.iter().enumerate() {
                let label = if idx == 0 { "prompts" } else { "" };
                let value = format!("[{idx}] {}", truncate_text(prompt.trim(), 200));
                lines.extend(render_card_detail_line(
                    if label.is_empty() { None } else { Some(label) },
                    &value,
                    tool_value_style(),
                    width,
                ));
            }
        } else {
            let show_args = matches!(self.status, ToolStatus::Running) || self.output.is_none();
            if show_args && let Some(summary) = self.input_summary.as_ref() {
                lines.extend(render_compact_kv(
                    "args",
                    summary,
                    tool_value_style(),
                    width,
                ));
            }
        }

        if let Some(output) = self.output.as_ref() {
            if self.is_diff {
                let diff_summary = diff_render::diff_summary_label(output);
                lines.push(render_tool_header_with_summary(
                    "Diff",
                    diff_summary.as_deref(),
                    tool_status_label(self.status),
                    self.status,
                    None,
                    low_motion,
                ));
                lines.extend(diff_render::render_diff(output, width));
            } else {
                lines.extend(render_tool_output_mode(
                    output,
                    width,
                    TOOL_OUTPUT_LINE_LIMIT,
                    mode,
                ));
            }

            if matches!(mode, RenderMode::Live)
                && let Some(path) = self.spillover_path.as_ref()
            {
                lines.push(render_spillover_annotation(path, width));
            }
        }
        wrap_card_rail(lines)
    }

    /// Render `agent_spawn` as a single compact summary line for live
    /// mode (#409). The companion `DelegateCard` already carries the
    /// live action tree, status, and final summary; this line is just
    /// the pointer that says "a spawn happened, here's the agent id".
    ///
    /// Output shape (header):
    ///   `◐ delegate · agent_spawn  agent-abc12  [running]`
    /// Falls back to a placeholder when the spawn is still pending and
    /// no agent id has been assigned yet.
    fn render_agent_spawn_compact(&self, low_motion: bool) -> Vec<Line<'static>> {
        let family = crate::tui::widgets::tool_card::ToolFamily::Delegate;
        let agent_id = self
            .output
            .as_deref()
            .and_then(extract_agent_id)
            .unwrap_or("…");
        vec![render_tool_header_with_family_and_summary(
            family,
            Some(agent_id),
            tool_status_label(self.status),
            self.status,
            None,
            low_motion,
        )]
    }

    /// If this cell is a checklist/todo write/add/update and the output is
    /// parseable as a checklist snapshot, render a purpose-built checklist
    /// card instead of the generic `name: ... { json }` block (issue #241).
    fn try_render_as_checklist(
        &self,
        width: u16,
        low_motion: bool,
        mode: RenderMode,
    ) -> Option<Vec<Line<'static>>> {
        if !is_checklist_tool_name(&self.name) {
            return None;
        }
        let output = self.output.as_ref()?;
        let snapshot = parse_checklist_snapshot(output)?;

        // Concise update rendering (#403). When the tool emits an
        // "Updated todo #N to STATUS" prefix line — which `todo_update` /
        // `checklist_update` always do on a successful match — render
        // only the changed item plus a `M/N · pct%` summary instead of
        // dumping the full list every time. The full list is still
        // reachable via Alt+V on the tool detail record. This keeps the
        // transcript scannable in long sessions.
        if matches!(mode, RenderMode::Live)
            && let Some(change) = parse_update_prefix(output)
        {
            return Some(render_checklist_change_card(
                &self.name,
                self.status,
                &snapshot,
                &change,
                width,
                low_motion,
            ));
        }

        Some(render_checklist_card(
            &self.name,
            self.status,
            &snapshot,
            width,
            low_motion,
            mode,
        ))
    }
}

/// Render the inline annotation for a tool cell whose full output was
/// spilled to disk (#422 + #423). Produces a one-line muted hint:
///
/// ```text
///   full output: /Users/you/.deepseek/tool_outputs/call-abc12.txt
/// ```
///
/// Path is plain text on this branch; the OSC 8 hyperlink-wrap that
/// makes it Cmd+click-openable lives on the OSC 8 branch (PR #515)
/// and merges in once both PRs land on `main`. The clipboard /
/// selection path already strips OSC 8 there, so a future enhancement
/// stays backward-compatible.
fn render_spillover_annotation(path: &std::path::Path, width: u16) -> Line<'static> {
    let display = path.display().to_string();
    let prefix = "  full output: ";
    let budget = usize::from(width).saturating_sub(prefix.len()).max(8);
    let truncated = truncate_text(&display, budget);
    Line::from(vec![
        Span::styled(prefix, Style::default().fg(palette::TEXT_MUTED)),
        Span::styled(truncated, Style::default().fg(palette::TEXT_MUTED).italic()),
    ])
}

/// Pull the `agent_id` field out of an `agent_spawn` tool output. The
/// tool emits structured JSON shaped like
/// `{"agent_id": "agent-abc12", "nickname": "...", "model": "..."}` so we
/// look for the `agent_id` key and return its string value.
///
/// Returns `None` for outputs we can't parse as JSON or that lack the
/// expected key — the caller falls back to a placeholder so a still-pending
/// spawn renders cleanly.
fn extract_agent_id(output: &str) -> Option<&str> {
    // Cheap, deterministic, no allocations: scan for the literal key.
    // Avoids dragging serde_json into a render hot path on every frame.
    let key = "\"agent_id\"";
    let key_idx = output.find(key)?;
    let rest = &output[key_idx + key.len()..];
    let colon = rest.find(':')?;
    let after_colon = rest[colon + 1..].trim_start();
    let after_colon = after_colon.strip_prefix('"')?;
    let end = after_colon.find('"')?;
    let id = &after_colon[..end];
    (!id.is_empty()).then_some(id)
}

fn is_checklist_tool_name(name: &str) -> bool {
    matches!(
        name,
        "checklist_write"
            | "checklist_add"
            | "checklist_update"
            | "todo_write"
            | "todo_add"
            | "todo_update"
    )
}

/// Heuristic: does the output look like a unified diff? Returns true when
/// the output contains at least one hunk header (`@@`) or a `diff --git`
/// line, which are reliable markers of unified diff content (#380).
pub(crate) fn output_looks_like_diff(output: &str) -> bool {
    let mut lines = output.lines();
    // Check first 5 lines for diff markers
    for _ in 0..5 {
        let Some(line) = lines.next() else { break };
        let trimmed = line.trim();
        if trimmed.starts_with("@@") || trimmed.starts_with("diff --git") {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone)]
struct ChecklistItemSnapshot {
    content: String,
    status: String,
}

#[derive(Debug, Clone, Default)]
struct ChecklistSnapshot {
    items: Vec<ChecklistItemSnapshot>,
    completion_pct: u8,
    completed: usize,
    total: usize,
}

/// Pull a structured checklist snapshot out of the tool's text output.
/// The tool emits a leading human-readable line followed by JSON, so we
/// scan for the first `{` and parse from there. Returns `None` if the
/// payload is missing the expected `items` array.
fn parse_checklist_snapshot(output: &str) -> Option<ChecklistSnapshot> {
    let json_start = output.find('{')?;
    let parsed: Value = serde_json::from_str(&output[json_start..]).ok()?;
    let items_value = parsed.get("items")?.as_array()?;

    let items: Vec<ChecklistItemSnapshot> = items_value
        .iter()
        .map(|item| ChecklistItemSnapshot {
            content: item
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            status: item
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending")
                .to_string(),
        })
        .collect();

    if items.is_empty() {
        return None;
    }

    let completed = items
        .iter()
        .filter(|item| item.status.eq_ignore_ascii_case("completed"))
        .count();
    let total = items.len();
    let completion_pct = parsed
        .get("completion_pct")
        .and_then(Value::as_u64)
        .map(|pct| u8::try_from(pct.min(100)).unwrap_or(100))
        .unwrap_or_else(|| {
            (completed * 100)
                .checked_div(total)
                .and_then(|pct| u8::try_from(pct).ok())
                .unwrap_or(0)
        });

    Some(ChecklistSnapshot {
        items,
        completion_pct,
        completed,
        total,
    })
}

/// One parsed "Updated todo #N to STATUS" prefix line emitted by
/// `todo_update` / `checklist_update`. Used by [`render_checklist_change_card`]
/// to show a compact state-change line instead of the full item list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChecklistChange {
    id: u32,
    status: String,
}

/// Parse the leading line of a checklist-update tool output. Returns
/// `None` for non-update outputs (e.g. `todo_write` snapshots, errors,
/// or an unexpected format) so the caller falls back to the full-list
/// renderer.
fn parse_update_prefix(output: &str) -> Option<ChecklistChange> {
    // The tool output shape is `Updated todo #3 to in_progress\n{ ... }`.
    // We tolerate `checklist` or `todo` as the noun and any reasonable
    // status word (the snapshot lookup in the renderer is the source of
    // truth for the title — we just need the id+status pair).
    let first = output.lines().next()?.trim();
    let rest = first
        .strip_prefix("Updated todo #")
        .or_else(|| first.strip_prefix("Updated checklist #"))?;
    let (id_str, after) = rest.split_once(' ')?;
    let id: u32 = id_str.parse().ok()?;
    let status = after.strip_prefix("to ")?.trim().to_string();
    if status.is_empty() {
        return None;
    }
    Some(ChecklistChange { id, status })
}

/// Render a compact one-line state-change card for `todo_update` /
/// `checklist_update` calls (#403). Shows the changed item's marker,
/// title, and old → new status, with a `M/N · pct%` progress summary
/// in the header. The full list is still available via Alt+V on the
/// detail record.
fn render_checklist_change_card(
    name: &str,
    status: ToolStatus,
    snapshot: &ChecklistSnapshot,
    change: &ChecklistChange,
    width: u16,
    low_motion: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let header_summary = format!(
        "{}/{} \u{00B7} {}%",
        snapshot.completed, snapshot.total, snapshot.completion_pct
    );
    let family = crate::tui::widgets::tool_card::tool_family_for_name(name);
    lines.push(render_tool_header_with_family_and_summary(
        family,
        Some(&header_summary),
        tool_status_label(status),
        status,
        None,
        low_motion,
    ));

    // Look up the title from the snapshot. `id` in tool input is
    // 1-indexed; `items` is 0-indexed.
    let item = (change.id as usize)
        .checked_sub(1)
        .and_then(|idx| snapshot.items.get(idx));
    let title = item
        .map(|i| i.content.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(missing title)".to_string());

    let (marker, marker_color) = checklist_status_marker(&change.status);
    let prefix = format!("{marker} ");
    let prefix_width =
        UnicodeWidthStr::width(TRANSCRIPT_RAIL) + UnicodeWidthStr::width(prefix.as_str());
    let id_label = format!("Todo #{}", change.id);
    let arrow = " \u{2192} ";
    let status_label = change.status.clone();
    let title_budget = usize::from(width)
        .saturating_sub(prefix_width)
        .saturating_sub(UnicodeWidthStr::width(id_label.as_str()))
        .saturating_sub(UnicodeWidthStr::width(arrow))
        .saturating_sub(UnicodeWidthStr::width(status_label.as_str()))
        .saturating_sub(2)
        .max(8);
    let title_truncated = truncate_text(title.as_str(), title_budget);

    let spans = vec![
        Span::styled(
            "\u{258F} ".to_string(),
            Style::default().fg(palette::TEXT_DIM),
        ),
        Span::styled(prefix, Style::default().fg(marker_color)),
        Span::styled(id_label, Style::default().fg(palette::TEXT_DIM)),
        Span::styled(": ".to_string(), Style::default().fg(palette::TEXT_DIM)),
        Span::styled(title_truncated, tool_value_style()),
        Span::styled(arrow.to_string(), Style::default().fg(palette::TEXT_DIM)),
        Span::styled(status_label, Style::default().fg(marker_color)),
    ];
    lines.push(Line::from(spans));

    // Tease that the full list is still available without leaving the
    // transcript. Mirrors the same affordance used by other tool cells.
    lines.push(render_card_detail_line_single(
        None,
        &format!(
            "{} item{} (Alt+V for full list)",
            snapshot.total,
            if snapshot.total == 1 { "" } else { "s" }
        ),
        Style::default().fg(palette::TEXT_MUTED),
    ));
    lines
}

fn checklist_status_marker(status: &str) -> (&'static str, Color) {
    match status.to_ascii_lowercase().as_str() {
        "completed" | "done" => ("\u{2611}", palette::STATUS_SUCCESS), // ☑
        "in_progress" | "inprogress" | "running" => ("\u{25D0}", palette::DEEPSEEK_SKY), // ◐
        "blocked" | "failed" => ("\u{2717}", palette::STATUS_ERROR),   // ✗
        "cancelled" | "canceled" | "skipped" => ("\u{2298}", palette::TEXT_MUTED), // ⊘
        _ => ("\u{2610}", palette::TEXT_MUTED),                        // ☐ pending
    }
}

const CHECKLIST_LIVE_ITEM_LIMIT: usize = 8;

fn render_checklist_card(
    name: &str,
    status: ToolStatus,
    snapshot: &ChecklistSnapshot,
    width: u16,
    low_motion: bool,
    mode: RenderMode,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let header_summary = format!(
        "{}/{} \u{00B7} {}%",
        snapshot.completed, snapshot.total, snapshot.completion_pct
    );
    let family = crate::tui::widgets::tool_card::tool_family_for_name(name);
    lines.push(render_tool_header_with_family_and_summary(
        family,
        Some(&header_summary),
        tool_status_label(status),
        status,
        None,
        low_motion,
    ));
    lines.extend(render_compact_kv(
        "checklist",
        name,
        tool_value_style(),
        width,
    ));

    let cap = match mode {
        RenderMode::Live => CHECKLIST_LIVE_ITEM_LIMIT,
        RenderMode::Transcript => snapshot.items.len(),
    };
    let visible: Vec<&ChecklistItemSnapshot> = snapshot.items.iter().take(cap).collect();
    let omitted = snapshot.items.len().saturating_sub(visible.len());

    for item in visible {
        let (marker, color) = checklist_status_marker(&item.status);
        let prefix = format!("{marker} ");
        // Reserve room for the rail + marker prefix when wrapping content.
        let prefix_width =
            UnicodeWidthStr::width(TRANSCRIPT_RAIL) + UnicodeWidthStr::width(prefix.as_str());
        let content_width = usize::from(width).saturating_sub(prefix_width).max(1);
        for (idx, part) in wrap_text(item.content.trim(), content_width)
            .into_iter()
            .enumerate()
        {
            let mut spans = vec![Span::styled(
                "\u{258F} ".to_string(),
                Style::default().fg(palette::TEXT_DIM),
            )];
            if idx == 0 {
                spans.push(Span::styled(prefix.clone(), Style::default().fg(color)));
            } else {
                spans.push(Span::raw(
                    " ".repeat(UnicodeWidthStr::width(prefix.as_str())),
                ));
            }
            spans.push(Span::styled(part, tool_value_style()));
            lines.push(Line::from(spans));
        }
    }

    if omitted > 0 {
        lines.push(render_card_detail_line_single(
            None,
            &format!("+{omitted} more (Alt+V for full list)"),
            Style::default().fg(palette::TEXT_DIM),
        ));
    }

    lines
}

fn summarize_string_value(text: &str, max_len: usize, count_only: bool) -> String {
    let trimmed = text.trim();
    let len = trimmed.chars().count();
    if count_only || len > max_len {
        return format!("<{len} chars>");
    }
    truncate_text(trimmed, max_len)
}

fn summarize_inline_value(value: &Value, max_len: usize, count_only: bool) -> String {
    match value {
        Value::String(s) => summarize_string_value(s, max_len, count_only),
        Value::Array(items) => format!("<{} items>", items.len()),
        Value::Object(map) => format!("<{} keys>", map.len()),
        Value::Bool(b) => b.to_string(),
        Value::Number(num) => num.to_string(),
        Value::Null => "null".to_string(),
    }
}

#[must_use]
pub fn summarize_tool_args(input: &Value) -> Option<String> {
    let obj = input.as_object()?;
    if obj.is_empty() {
        return None;
    }

    let mut parts = Vec::new();

    if let Some(value) = obj.get("path") {
        parts.push(format!(
            "path: {}",
            summarize_inline_value(value, 80, false)
        ));
    }
    if let Some(value) = obj.get("command") {
        parts.push(format!(
            "command: {}",
            summarize_inline_value(value, 80, false)
        ));
    }
    if let Some(value) = obj.get("query") {
        parts.push(format!(
            "query: {}",
            summarize_inline_value(value, 80, false)
        ));
    }
    if let Some(value) = obj.get("prompt") {
        parts.push(format!(
            "prompt: {}",
            summarize_inline_value(value, 80, false)
        ));
    }
    if let Some(value) = obj.get("text") {
        parts.push(format!(
            "text: {}",
            summarize_inline_value(value, 80, false)
        ));
    }
    if let Some(value) = obj.get("pattern") {
        parts.push(format!(
            "pattern: {}",
            summarize_inline_value(value, 80, false)
        ));
    }
    if let Some(value) = obj.get("model") {
        parts.push(format!(
            "model: {}",
            summarize_inline_value(value, 40, false)
        ));
    }
    if let Some(value) = obj.get("file_id") {
        parts.push(format!(
            "file_id: {}",
            summarize_inline_value(value, 40, false)
        ));
    }
    if let Some(value) = obj.get("task_id") {
        parts.push(format!(
            "task_id: {}",
            summarize_inline_value(value, 40, false)
        ));
    }
    if let Some(value) = obj.get("voice_id") {
        parts.push(format!(
            "voice_id: {}",
            summarize_inline_value(value, 40, false)
        ));
    }
    if let Some(value) = obj.get("content") {
        parts.push(format!(
            "content: {}",
            summarize_inline_value(value, 0, true)
        ));
    }

    if parts.is_empty()
        && let Some((key, value)) = obj.iter().next()
    {
        return Some(format!(
            "{}: {}",
            key,
            summarize_inline_value(value, 80, false)
        ));
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

#[must_use]
pub fn summarize_tool_output(output: &str) -> String {
    if let Ok(json) = serde_json::from_str::<Value>(output) {
        if let Some(obj) = json.as_object() {
            if let Some(error) = obj.get("error").or(obj.get("status_msg")) {
                return format!("Error: {}", summarize_inline_value(error, 120, false));
            }

            let mut parts = Vec::new();

            if let Some(status) = obj.get("status").and_then(|v| v.as_str()) {
                parts.push(format!("status: {status}"));
            }
            if let Some(message) = obj.get("message").and_then(|v| v.as_str()) {
                parts.push(truncate_text(message, TOOL_TEXT_LIMIT));
            }
            if let Some(task_id) = obj.get("task_id").and_then(|v| v.as_str()) {
                parts.push(format!("task_id: {task_id}"));
            }
            if let Some(file_id) = obj.get("file_id").and_then(|v| v.as_str()) {
                parts.push(format!("file_id: {file_id}"));
            }
            if let Some(url) = obj
                .get("file_url")
                .or_else(|| obj.get("url"))
                .and_then(|v| v.as_str())
            {
                parts.push(format!("url: {}", truncate_text(url, 120)));
            }
            if let Some(data) = obj.get("data") {
                parts.push(format!("data: {}", summarize_inline_value(data, 80, true)));
            }

            if !parts.is_empty() {
                return parts.join(" | ");
            }

            if let Some(content) = obj
                .get("content")
                .or(obj.get("result"))
                .or(obj.get("output"))
            {
                return summarize_inline_value(content, TOOL_TEXT_LIMIT, false);
            }
        }

        return summarize_inline_value(&json, TOOL_TEXT_LIMIT, true);
    }

    truncate_text(output, TOOL_TEXT_LIMIT)
}

// === MCP Output Summaries ===

/// Summary information extracted from an MCP tool output payload.
pub struct McpOutputSummary {
    pub content: Option<String>,
    pub is_image: bool,
    pub is_error: Option<bool>,
}

/// Summarize raw MCP output into UI-friendly content.
#[must_use]
pub fn summarize_mcp_output(output: &str) -> McpOutputSummary {
    if let Ok(json) = serde_json::from_str::<Value>(output) {
        let is_error = json
            .get("isError")
            .and_then(serde_json::Value::as_bool)
            .or_else(|| json.get("is_error").and_then(serde_json::Value::as_bool));

        if let Some(blocks) = json.get("content").and_then(|v| v.as_array()) {
            let mut lines = Vec::new();
            let mut is_image = false;

            for block in blocks {
                let block_type = block
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                match block_type {
                    "text" => {
                        let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        if !text.is_empty() {
                            lines.push(format!("- text: {}", truncate_text(text, 200)));
                        }
                    }
                    "image" | "image_url" => {
                        is_image = true;
                        let url = block
                            .get("url")
                            .or_else(|| block.get("image_url"))
                            .and_then(|v| v.as_str());
                        if let Some(url) = url {
                            lines.push(format!("- image: {}", truncate_text(url, 200)));
                        } else {
                            lines.push("- image".to_string());
                        }
                    }
                    "resource" | "resource_link" => {
                        let uri = block
                            .get("uri")
                            .or_else(|| block.get("url"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("<resource>");
                        lines.push(format!("- resource: {}", truncate_text(uri, 200)));
                    }
                    other => {
                        lines.push(format!("- {other} content"));
                    }
                }
            }

            return McpOutputSummary {
                content: if lines.is_empty() {
                    None
                } else {
                    Some(lines.join("\n"))
                },
                is_image,
                is_error,
            };
        }
    }

    McpOutputSummary {
        content: Some(summarize_tool_output(output)),
        is_image: output_is_image(output),
        is_error: None,
    }
}

#[must_use]
pub fn output_is_image(output: &str) -> bool {
    let lower = output.to_lowercase();

    [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".tiff", ".ppm",
    ]
    .iter()
    .any(|ext| lower.contains(ext))
}

#[must_use]
pub fn extract_reasoning_summary(text: &str) -> Option<String> {
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.to_lowercase().starts_with("summary") {
            let mut summary = String::new();
            if let Some((_, rest)) = trimmed.split_once(':')
                && !rest.trim().is_empty()
            {
                summary.push_str(rest.trim());
                summary.push('\n');
            }
            while let Some(next) = lines.peek() {
                let next_trimmed = next.trim();
                if next_trimmed.is_empty() {
                    break;
                }
                if next_trimmed.starts_with('#') || next_trimmed.starts_with("**") {
                    break;
                }
                summary.push_str(next_trimmed);
                summary.push('\n');
                lines.next();
            }
            let summary = summary.trim().to_string();
            return if summary.is_empty() {
                None
            } else {
                Some(summary)
            };
        }
    }
    let fallback = text.trim();
    if fallback.is_empty() {
        None
    } else {
        Some(fallback.to_string())
    }
}

fn render_thinking(
    content: &str,
    width: u16,
    streaming: bool,
    duration_secs: Option<f32>,
    collapsed: bool,
    low_motion: bool,
) -> Vec<Line<'static>> {
    let state = thinking_visual_state(streaming, duration_secs);
    let style = thinking_style();
    // 12% reasoning surface tint over the app ink — the only deliberately
    // warm element in the transcript. Dropped on Ansi-16 terminals where the
    // tint would distort the named palette.
    let depth = cached_color_depth();
    let body_bg = palette::reasoning_surface_tint(depth);
    let body_style = match body_bg {
        Some(bg) => style.italic().bg(bg),
        None => style.italic(),
    };
    let mut lines = Vec::new();

    // Header: `…` opener (replaces the spinner; reasoning isn't a tool, it's
    // a slow exhale) followed by the `thinking` label and live status.
    let mut header_spans = vec![
        Span::styled(
            format!("{REASONING_OPENER} "),
            Style::default().fg(thinking_state_accent(state)),
        ),
        Span::styled("thinking", thinking_title_style()),
    ];
    header_spans.push(Span::styled(" ", Style::default()));
    header_spans.push(Span::styled(
        thinking_status_label(state),
        thinking_status_style(state),
    ));
    if let Some(dur) = duration_secs {
        header_spans.push(Span::styled(" · ", Style::default().fg(palette::TEXT_DIM)));
        header_spans.push(Span::styled(format!("{dur:.1}s"), thinking_meta_style()));
    }
    lines.push(Line::from(header_spans));

    let content_width = width.saturating_sub(3).max(1);
    let body_text = if collapsed {
        if streaming {
            // #861 RC4 / #1324: during streaming we don't yet have a
            // completed reasoning block, so `extract_reasoning_summary`
            // is meaningless. Show the raw content and let the
            // truncation logic below keep the *last* `LIMIT` lines so
            // the user sees the model's most recent thinking instead of
            // staring at an empty placeholder.
            content.to_string()
        } else {
            extract_reasoning_summary(content).unwrap_or_else(|| content.trim().to_string())
        }
    } else {
        content.to_string()
    };
    let mut rendered = if body_text.trim().is_empty() {
        Vec::new()
    } else {
        markdown_render::render_markdown(&body_text, content_width, body_style)
    };
    let mut truncated = false;
    if collapsed && rendered.len() > THINKING_SUMMARY_LINE_LIMIT {
        if streaming {
            // Drop the *head* during streaming so the visible window
            // tracks the live cursor at the bottom.
            let drop = rendered.len() - THINKING_SUMMARY_LINE_LIMIT;
            rendered.drain(0..drop);
        } else {
            rendered.truncate(THINKING_SUMMARY_LINE_LIMIT);
        }
        truncated = true;
    }

    let rail_style = Style::default().fg(thinking_state_accent(state));
    let cursor_style = Style::default().fg(palette::ACCENT_REASONING_LIVE);

    if rendered.is_empty() && streaming {
        let mut spans = vec![Span::styled(REASONING_RAIL.to_string(), rail_style)];
        spans.push(Span::styled("thinking...", body_style.italic()));
        if !low_motion {
            spans.push(Span::styled(format!(" {REASONING_CURSOR}"), cursor_style));
        }
        lines.push(Line::from(spans));
    }

    let last_idx = rendered.len().saturating_sub(1);
    for (idx, line) in rendered.into_iter().enumerate() {
        let mut spans = vec![Span::styled(REASONING_RAIL.to_string(), rail_style)];
        spans.extend(line.spans);
        // Trailing cursor on the very last body line while streaming —
        // signals "still generating" without churning every line.
        if streaming && !low_motion && idx == last_idx {
            spans.push(Span::styled(format!(" {REASONING_CURSOR}"), cursor_style));
        }
        lines.push(Line::from(spans));
    }

    let needs_affordance = collapsed
        && if streaming {
            // #861 RC4 / #1324: during streaming, surface the affordance
            // whenever any head lines have been clipped so the user
            // knows there's more above and how to reach it.
            truncated
        } else {
            truncated || body_text.trim() != content.trim()
        };
    if needs_affordance {
        let label = if streaming {
            "thinking continues; press Ctrl+O for full text"
        } else {
            "thinking collapsed; press Ctrl+O for full text"
        };
        lines.push(Line::from(vec![
            Span::styled(REASONING_RAIL.to_string(), rail_style),
            Span::styled(label, Style::default().fg(palette::TEXT_MUTED).italic()),
        ]));
    }

    lines
}

fn render_message(
    prefix: &str,
    label_style: Style,
    body_style: Style,
    content: &str,
    width: u16,
) -> Vec<Line<'static>> {
    let prefix_width = UnicodeWidthStr::width(prefix);
    let prefix_width_u16 = u16::try_from(prefix_width.saturating_add(2)).unwrap_or(u16::MAX);
    let content_width = usize::from(width.saturating_sub(prefix_width_u16).max(1));
    let mut lines = Vec::new();
    let rendered =
        markdown_render::render_markdown_tagged(content, content_width as u16, body_style);
    for (idx, rendered_line) in rendered.into_iter().enumerate() {
        if idx == 0 {
            let mut spans = Vec::new();
            if !prefix.is_empty() {
                spans.push(Span::styled(
                    prefix.to_string(),
                    label_style.add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::raw(" "));
            }
            spans.extend(rendered_line.line.spans);
            lines.push(Line::from(spans));
        } else {
            let indent = if prefix.is_empty() {
                String::new()
            } else if rendered_line.is_code {
                " ".repeat(prefix_width + 1)
            } else {
                let mut s = String::with_capacity(prefix_width + 1);
                s.push('\u{258F}');
                s.extend(std::iter::repeat_n(' ', prefix_width));
                s
            };
            let rail_style = Style::default().fg(palette::TEXT_DIM);
            let mut spans = vec![Span::styled(indent, rail_style)];
            spans.extend(rendered_line.line.spans);
            lines.push(Line::from(spans));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

fn render_command_mode(command: &str, width: u16, mode: RenderMode) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let cap = match mode {
        RenderMode::Live => TOOL_COMMAND_LINE_LIMIT,
        RenderMode::Transcript => usize::MAX,
    };
    for (count, chunk) in wrap_text(command, width.saturating_sub(4).max(1) as usize)
        .into_iter()
        .enumerate()
    {
        if count >= cap {
            lines.push(details_affordance_line(
                "command clipped; Alt+V for details",
                Style::default().fg(palette::TEXT_MUTED),
            ));
            break;
        }
        lines.extend(render_card_detail_line(
            if count == 0 { Some("command") } else { None },
            chunk.as_str(),
            tool_value_style(),
            width,
        ));
    }
    lines
}

fn command_header_summary(command: &str) -> String {
    command
        .lines()
        .next()
        .unwrap_or(command)
        .trim_start_matches("$ ")
        .trim()
        .to_string()
}

fn exploring_header_summary(entries: &[ExploringEntry]) -> Option<String> {
    match entries {
        [] => None,
        [entry] => Some(entry.label.clone()),
        entries => Some(format!("{} items", entries.len())),
    }
}

fn render_compact_kv(label: &str, value: &str, style: Style, width: u16) -> Vec<Line<'static>> {
    render_card_detail_line(Some(label.trim_end_matches(':')), value, style, width)
}

/// Wrap rendered tool-card lines with card-rail glyphs (╭ │ ╰).
/// First non-empty line gets `╭`, middle lines get `│`, last line gets `╰`.
/// Single-line cards get a single `─` prefix.
fn wrap_card_rail(mut lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let n = lines.len();
    if n == 0 {
        return lines;
    }
    if n == 1 {
        lines[0].spans.insert(0, Span::raw("─ "));
        return lines;
    }
    for (i, line) in lines.iter_mut().enumerate() {
        let rail = if i == 0 {
            "\u{256D} " // ╭
        } else if i == n - 1 {
            "\u{2570} " // ╰
        } else {
            "\u{2502} " // │
        };
        line.spans.insert(0, Span::raw(rail));
    }
    lines
}

fn render_tool_output_mode(
    output: &str,
    width: u16,
    line_limit: usize,
    mode: RenderMode,
) -> Vec<Line<'static>> {
    render_preserved_output_mode(output, width, line_limit, mode, "result")
}

fn review_severity_color(severity: &str) -> Color {
    match severity {
        "error" => palette::STATUS_ERROR,
        "warning" => palette::STATUS_WARNING,
        _ => palette::STATUS_INFO,
    }
}

fn format_review_location(path: Option<&String>, line: Option<u32>) -> String {
    let path = path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    match (path, line) {
        (Some(path), Some(line)) => format!("{path}:{line}"),
        (Some(path), None) => path,
        (None, Some(line)) => format!("line {line}"),
        (None, None) => String::new(),
    }
}

fn render_exec_output_mode(
    output: &str,
    width: u16,
    line_limit: usize,
    mode: RenderMode,
) -> Vec<Line<'static>> {
    render_preserved_output_mode(output, width, line_limit, mode, "output")
}

#[derive(Debug, Clone)]
struct OutputRow {
    text: String,
    intact: bool,
}

fn render_preserved_output_mode(
    output: &str,
    width: u16,
    line_limit: usize,
    mode: RenderMode,
    first_label: &str,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if output.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no output)",
            Style::default().fg(palette::TEXT_MUTED).italic(),
        )));
        return lines;
    }

    let all_lines = output_rows(output, width);

    if matches!(mode, RenderMode::Transcript) {
        // Full-content path: emit every wrapped line with no head/tail split,
        // no "+N more" affordance.
        for (idx, row) in all_lines.iter().enumerate() {
            render_output_row(
                &mut lines,
                if idx == 0 { Some(first_label) } else { None },
                row,
                width,
            );
        }
        return lines;
    }

    let selected = selected_output_indices(&all_lines, line_limit);
    let mut previous: Option<usize> = None;
    for (rendered_idx, idx) in selected.iter().copied().enumerate() {
        if let Some(prev) = previous {
            let omitted = idx.saturating_sub(prev + 1);
            if omitted > 0 {
                lines.push(details_affordance_line(
                    &format!("{omitted} lines omitted; Alt+V for details"),
                    Style::default().fg(palette::TEXT_MUTED),
                ));
            }
        }

        let row = &all_lines[idx];
        render_output_row(
            &mut lines,
            if rendered_idx == 0 {
                Some(first_label)
            } else {
                None
            },
            row,
            width,
        );
        previous = Some(idx);
    }

    lines
}

fn output_rows(output: &str, width: u16) -> Vec<OutputRow> {
    let wrap_width = width.saturating_sub(4).max(1) as usize;
    let mut rows = Vec::new();
    let mut sanitized = String::with_capacity(output.len());
    for line in output.lines() {
        sanitized.clear();
        crate::tui::osc8::strip_ansi_into(line, &mut sanitized);
        let intact = is_path_or_url_like(&sanitized);
        if intact {
            rows.push(OutputRow {
                text: sanitized.clone(),
                intact: true,
            });
        } else {
            for wrapped in wrap_text(&sanitized, wrap_width) {
                rows.push(OutputRow {
                    text: wrapped,
                    intact: false,
                });
            }
        }
    }
    if rows.is_empty() {
        rows.push(OutputRow {
            text: String::new(),
            intact: false,
        });
    }
    rows
}

fn selected_output_indices(rows: &[OutputRow], line_limit: usize) -> Vec<usize> {
    let total = rows.len();
    if total <= line_limit || line_limit == 0 {
        return (0..total).collect();
    }

    let head = TOOL_OUTPUT_HEAD_LINES.min(line_limit).min(total);
    let tail = TOOL_OUTPUT_TAIL_LINES
        .min(line_limit.saturating_sub(head))
        .min(total.saturating_sub(head));
    let mut selected = std::collections::BTreeSet::new();
    selected.extend(0..head);
    selected.extend(total.saturating_sub(tail)..total);

    let budget = line_limit.saturating_sub(selected.len());
    if budget > 0 {
        let mut important: Vec<(usize, usize)> = rows
            .iter()
            .enumerate()
            .skip(head)
            .take(total.saturating_sub(head + tail))
            .filter_map(|(idx, row)| output_importance_rank(&row.text).map(|rank| (idx, rank)))
            .collect();
        important.sort_by_key(|(idx, rank)| (*rank, *idx));
        for (idx, _) in important.into_iter().take(budget) {
            selected.insert(idx);
        }
    }

    selected.into_iter().collect()
}

fn output_importance_rank(line: &str) -> Option<usize> {
    let lower = line.to_ascii_lowercase();
    if [
        "error",
        "failed",
        "failure",
        "fatal",
        "panic",
        "exception",
        "traceback",
        "denied",
        "not found",
        "no such file",
        "cannot",
        "can't",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return Some(0);
    }
    if lower.contains("warning") || lower.contains("warn") {
        return Some(1);
    }
    if is_path_or_url_like(line) {
        return Some(2);
    }
    None
}

fn is_path_or_url_like(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.contains("://") || trimmed.starts_with("file:") {
        return true;
    }
    let has_separator = trimmed.contains('/') || trimmed.contains('\\');
    let has_extension = trimmed
        .split_whitespace()
        .any(|part| part.rsplit_once('.').is_some_and(|(_, ext)| ext.len() <= 8));
    has_separator && has_extension
}

/// Detect whether a system message is a cycle-boundary announcement
/// (e.g. `─── cycle 0 → 1  (briefing: 2500 tokens) ───`).
fn is_cycle_boundary(content: &str) -> bool {
    content.contains("cycle")
}

/// Render a cycle-boundary system message with distinct visual styling (#395):
/// full-width line with DEEPSEEK_BLUE text and bold weight, plus a thin
/// horizontal rule above for visual separation.
fn render_cycle_boundary(content: &str, width: u16) -> Vec<Line<'static>> {
    let style = Style::default()
        .fg(palette::DEEPSEEK_BLUE)
        .add_modifier(Modifier::BOLD);
    let rule_style = Style::default().fg(palette::TEXT_DIM);
    let content_width = usize::from(width.saturating_sub(2).max(1));
    let mut lines = Vec::new();
    // Thin horizontal rule above for visual separation
    if width >= 4 {
        let rule = "\u{2500}".repeat(content_width);
        lines.push(Line::from(Span::styled(format!("  {rule}"), rule_style)));
    }
    // Cycle boundary text — just the content, full-width
    let rendered =
        crate::tui::markdown_render::render_markdown(content, content_width as u16, style);
    for line in rendered {
        let mut spans = vec![Span::raw("  ")];
        spans.extend(line.spans);
        lines.push(Line::from(spans));
    }
    if lines.len() == 1 && width >= 4 {
        // Only the rule was added (unlikely), but add at least a spacer
        lines.push(Line::from(""));
    }
    lines
}

/// Detect whether a line contains a `path:line` pattern that could be
/// opened by `try_open_file_at_line`. Returns a distinctive style
/// (underline + blue) when the pattern matches, or `None` otherwise.
/// The style is applied over the existing value style so the line
/// remains readable.
fn file_line_style(text: &str) -> Option<Style> {
    let trimmed = text.trim();
    if let Some((before, after)) = trimmed.rsplit_once(':')
        && !before.is_empty()
        && after.chars().all(|c| c.is_ascii_digit())
        && looks_like_file_path(before)
    {
        Some(
            Style::default()
                .fg(palette::DEEPSEEK_SKY)
                .add_modifier(Modifier::UNDERLINED),
        )
    } else {
        None
    }
}

/// Apply inline diff highlighting to a single text line.
///
/// Returns the appropriate style for the line based on its prefix:
/// - Lines starting with `+` (after trimming) => `palette::DIFF_ADDED` (green)
/// - Lines starting with `-` (after trimming) => `palette::STATUS_ERROR` (red)
/// - Lines starting with `@@` => `palette::DEEPSEEK_SKY` (cyan/blue)
/// - All other lines => None (use default style)
fn diff_line_style(text: &str) -> Option<Style> {
    let trimmed = text.trim_start();
    if trimmed.starts_with("@@") {
        Some(Style::default().fg(palette::DEEPSEEK_BLUE))
    } else if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
        Some(Style::default().fg(palette::DIFF_ADDED))
    } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
        Some(Style::default().fg(palette::STATUS_ERROR))
    } else {
        None
    }
}

fn render_output_row(
    lines: &mut Vec<Line<'static>>,
    label: Option<&str>,
    row: &OutputRow,
    width: u16,
) {
    // #374: apply file:line highlighting when the row text contains
    // a `path:line` pattern. Diff style takes precedence (colored
    // prefix lines should stay colored), but if no diff style matched,
    // check for a file:line pattern and highlight it distinctively.
    let diff_style = diff_line_style(&row.text);
    let file_style = file_line_style(&row.text);
    let value_style = diff_style.or(file_style).unwrap_or_else(tool_value_style);
    if row.intact {
        lines.push(render_card_detail_line_single(
            label,
            &row.text,
            value_style,
        ));
    } else {
        lines.extend(render_card_detail_line(
            label,
            &row.text,
            value_style,
            width,
        ));
    }
}

fn wrap_plain_line(line: &str, style: Style, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for part in wrap_text(line, width.max(1) as usize) {
        lines.push(Line::from(Span::styled(part, style)));
    }
    lines
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        let tentative = if current.is_empty() {
            ch.to_string()
        } else {
            let mut t = current.clone();
            t.push(ch);
            t
        };

        if UnicodeWidthStr::width(tentative.as_str()) > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }

        current.push(ch);
    }

    lines.push(current);

    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn status_symbol(started_at: Option<Instant>, status: ToolStatus, low_motion: bool) -> String {
    match status {
        ToolStatus::Running => {
            if low_motion {
                return TOOL_RUNNING_SYMBOLS[0].to_string();
            }
            let elapsed_ms = started_at.map_or_else(
                || {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |duration| duration.as_millis())
                },
                |t| t.elapsed().as_millis(),
            );
            let cycle = u128::from(TOOL_STATUS_SYMBOL_MS);
            let idx = elapsed_ms
                .checked_div(cycle)
                .map_or(0, |d| d % (TOOL_RUNNING_SYMBOLS.len() as u128));
            TOOL_RUNNING_SYMBOLS[usize::try_from(idx).unwrap_or_default()].to_string()
        }
        ToolStatus::Success => TOOL_DONE_SYMBOL.to_string(),
        ToolStatus::Failed => TOOL_FAILED_SYMBOL.to_string(),
    }
}

fn details_affordance_line(text: &str, style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            TRANSCRIPT_RAIL.to_string(),
            Style::default().fg(palette::TEXT_DIM),
        ),
        Span::styled(text.to_string(), style),
    ])
}

fn truncate_text(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        return text.to_string();
    }
    let mut out = String::new();
    for ch in text.chars().take(max_len.saturating_sub(3)) {
        out.push(ch);
    }
    out.push_str("...");
    out
}

fn user_label_style() -> Style {
    Style::default().fg(palette::TEXT_MUTED)
}

fn user_body_style() -> Style {
    Style::default().fg(palette::USER_BODY)
}

/// Style for the assistant glyph (`●`). When the cell is streaming and
/// motion is allowed, the foreground pulses on a 2s cycle between 30% and
/// 100% brightness — the only deliberately animated element in a calm
/// transcript. When idle (or low_motion is on) it sits at the full DeepSeek
/// sky color so finished turns read as solid rather than dim.
fn assistant_label_style_for(streaming: bool, low_motion: bool) -> Style {
    let color = if streaming && !low_motion {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        palette::pulse_brightness(palette::DEEPSEEK_SKY, now_ms)
    } else {
        palette::DEEPSEEK_SKY
    };
    Style::default().fg(color)
}

fn system_label_style() -> Style {
    Style::default().fg(palette::TEXT_DIM)
}

fn message_body_style() -> Style {
    Style::default().fg(palette::TEXT_PRIMARY)
}

fn system_body_style() -> Style {
    Style::default().fg(palette::TEXT_MUTED).italic()
}

/// Label glyph for an error cell. `Critical`/`Error` get the loudest marker;
/// `Warning` is softer; `Info` is neutral. Kept as ASCII so it survives any
/// terminal font fallback.
fn error_label_text(severity: crate::error_taxonomy::ErrorSeverity) -> &'static str {
    match severity {
        crate::error_taxonomy::ErrorSeverity::Critical
        | crate::error_taxonomy::ErrorSeverity::Error => "Error",
        crate::error_taxonomy::ErrorSeverity::Warning => "Warn",
        crate::error_taxonomy::ErrorSeverity::Info => "Info",
    }
}

/// Label color for an error cell — drives the leading rail glyph.
fn error_label_style(severity: crate::error_taxonomy::ErrorSeverity) -> Style {
    let color = match severity {
        crate::error_taxonomy::ErrorSeverity::Critical
        | crate::error_taxonomy::ErrorSeverity::Error => palette::STATUS_ERROR,
        crate::error_taxonomy::ErrorSeverity::Warning => palette::STATUS_WARNING,
        crate::error_taxonomy::ErrorSeverity::Info => palette::TEXT_DIM,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

/// Body color for an error cell — softer than the label so the rail draws
/// the eye but the prose stays readable.
fn error_body_style(severity: crate::error_taxonomy::ErrorSeverity) -> Style {
    let color = match severity {
        crate::error_taxonomy::ErrorSeverity::Critical
        | crate::error_taxonomy::ErrorSeverity::Error => palette::STATUS_ERROR,
        crate::error_taxonomy::ErrorSeverity::Warning => palette::STATUS_WARNING,
        crate::error_taxonomy::ErrorSeverity::Info => palette::TEXT_MUTED,
    };
    Style::default().fg(color)
}

fn thinking_style() -> Style {
    Style::default().fg(palette::TEXT_REASONING)
}

fn render_tool_header(
    title: &str,
    state: &str,
    status: ToolStatus,
    started_at: Option<Instant>,
    low_motion: bool,
) -> Line<'static> {
    let family = crate::tui::widgets::tool_card::tool_family_for_title(title);
    render_tool_header_with_family(family, state, status, started_at, low_motion)
}

fn render_tool_header_with_summary(
    title: &str,
    summary: Option<&str>,
    state: &str,
    status: ToolStatus,
    started_at: Option<Instant>,
    low_motion: bool,
) -> Line<'static> {
    let family = crate::tui::widgets::tool_card::tool_family_for_title(title);
    render_tool_header_with_family_and_summary(
        family, summary, state, status, started_at, low_motion,
    )
}

/// Render a tool-card header with an explicit verb family. Lets callers
/// (e.g. `GenericToolCell`) bypass the legacy title→family mapping when
/// they already know the actual tool name.
fn render_tool_header_with_family(
    family: crate::tui::widgets::tool_card::ToolFamily,
    state: &str,
    status: ToolStatus,
    started_at: Option<Instant>,
    low_motion: bool,
) -> Line<'static> {
    render_tool_header_with_family_and_summary(family, None, state, status, started_at, low_motion)
}

fn render_tool_header_with_family_and_summary(
    family: crate::tui::widgets::tool_card::ToolFamily,
    summary: Option<&str>,
    state: &str,
    status: ToolStatus,
    started_at: Option<Instant>,
    low_motion: bool,
) -> Line<'static> {
    // For long-running tools, append elapsed seconds so the user can see the
    // call isn't stuck. Threshold matches the eye's "did this hang?" reflex
    // — under 3s we stay quiet so quick reads/greps don't visually churn.
    let state_owned: String = if state == "running"
        && status == ToolStatus::Running
        && let Some(started) = started_at
    {
        running_status_label_with_elapsed(started.elapsed().as_secs())
    } else {
        state.to_string()
    };

    let glyph = crate::tui::widgets::tool_card::family_glyph(family);
    let verb = crate::tui::widgets::tool_card::family_label(family);

    let mut spans = vec![
        Span::styled(
            format!("{} ", status_symbol(started_at, status, low_motion)),
            Style::default().fg(tool_state_color(status)),
        ),
        Span::styled(
            format!("{glyph} "),
            Style::default().fg(tool_state_color(status)),
        ),
        Span::styled(verb.to_string(), tool_title_style()),
        Span::styled(" ", Style::default()),
        Span::styled(state_owned, tool_status_style(status)),
    ];

    if let Some(summary) = summary.and_then(normalize_header_summary) {
        spans.push(Span::styled(" · ", Style::default().fg(palette::TEXT_DIM)));
        spans.push(Span::styled(
            truncate_text(&summary, TOOL_HEADER_SUMMARY_LIMIT),
            Style::default().fg(palette::TEXT_MUTED),
        ));
    }

    Line::from(spans)
}

fn normalize_header_summary(summary: &str) -> Option<String> {
    let normalized = summary
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

/// Build the "running" label with an elapsed-seconds badge for long-running
/// tools. Below 3s the badge is suppressed to avoid visual churn for tools
/// that resolve in milliseconds; at 3s and beyond the badge appears and ticks
/// every second the tool stays in flight.
pub(crate) fn running_status_label_with_elapsed(elapsed_secs: u64) -> String {
    if elapsed_secs < 3 {
        "running".to_string()
    } else {
        format!("running ({elapsed_secs}s)")
    }
}

fn render_card_detail_line(
    label: Option<&str>,
    value: &str,
    value_style: Style,
    width: u16,
) -> Vec<Line<'static>> {
    let label_text = label.map(|text| format!("{text}:"));
    let prefix_width = UnicodeWidthStr::width(TRANSCRIPT_RAIL)
        + label_text.as_deref().map_or(0, UnicodeWidthStr::width)
        + usize::from(label.is_some());
    let content_width = usize::from(width).saturating_sub(prefix_width).max(1);

    let mut lines = Vec::new();
    for (idx, part) in wrap_text(value, content_width).into_iter().enumerate() {
        let mut spans = vec![Span::styled(
            TRANSCRIPT_RAIL.to_string(),
            Style::default().fg(palette::TEXT_DIM),
        )];
        if idx == 0 {
            if let Some(label_text) = label_text.as_deref() {
                spans.push(Span::styled(
                    label_text.to_string(),
                    tool_detail_label_style(),
                ));
                spans.push(Span::raw(" "));
            }
        } else if let Some(label_text) = label_text.as_deref() {
            spans.push(Span::raw(
                " ".repeat(UnicodeWidthStr::width(label_text) + 1),
            ));
        }
        spans.push(Span::styled(part, value_style));
        lines.push(Line::from(spans));
    }
    lines
}

fn render_card_detail_line_single(
    label: Option<&str>,
    value: &str,
    value_style: Style,
) -> Line<'static> {
    let label_text = label.map(|text| format!("{text}:"));
    let mut spans = vec![Span::styled(
        TRANSCRIPT_RAIL.to_string(),
        Style::default().fg(palette::TEXT_DIM),
    )];
    if let Some(label_text) = label_text {
        spans.push(Span::styled(label_text, tool_detail_label_style()));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(value.to_string(), value_style));
    Line::from(spans)
}

fn tool_title_style() -> Style {
    active_theme().tool_title_style()
}

fn tool_status_style(status: ToolStatus) -> Style {
    active_theme().tool_status_style(status)
}

fn tool_detail_label_style() -> Style {
    active_theme().tool_label_style()
}

fn tool_state_color(status: ToolStatus) -> Color {
    active_theme().tool_status_color(status)
}

fn tool_status_label(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Running => "running",
        ToolStatus::Success => "done",
        ToolStatus::Failed => "issue",
    }
}

fn tool_value_style() -> Style {
    active_theme().tool_value_style()
}

fn thinking_visual_state(streaming: bool, duration_secs: Option<f32>) -> ThinkingVisualState {
    if streaming {
        ThinkingVisualState::Live
    } else if duration_secs.is_some() {
        ThinkingVisualState::Done
    } else {
        ThinkingVisualState::Idle
    }
}

fn thinking_status_label(state: ThinkingVisualState) -> &'static str {
    match state {
        ThinkingVisualState::Live => "live",
        ThinkingVisualState::Done => "done",
        ThinkingVisualState::Idle => "idle",
    }
}

fn thinking_title_style() -> Style {
    Style::default()
        .fg(palette::TEXT_SOFT)
        .add_modifier(Modifier::BOLD)
}

fn thinking_status_style(state: ThinkingVisualState) -> Style {
    Style::default().fg(match state {
        ThinkingVisualState::Live => palette::ACCENT_REASONING_LIVE,
        ThinkingVisualState::Done => palette::TEXT_DIM,
        ThinkingVisualState::Idle => palette::TEXT_DIM,
    })
}

fn thinking_meta_style() -> Style {
    Style::default().fg(palette::TEXT_DIM)
}

fn thinking_state_accent(state: ThinkingVisualState) -> Color {
    match state {
        ThinkingVisualState::Live => palette::ACCENT_REASONING_LIVE,
        ThinkingVisualState::Done => palette::TEXT_DIM,
        ThinkingVisualState::Idle => palette::TEXT_DIM,
    }
}

// === Cached colour depth ===

/// Once-initialised colour depth for the terminal session. Avoids re-reading
/// `COLORTERM` / `TERM` env vars on every frame.
static COLOR_DEPTH: std::sync::OnceLock<palette::ColorDepth> = std::sync::OnceLock::new();

fn cached_color_depth() -> palette::ColorDepth {
    *COLOR_DEPTH.get_or_init(palette::ColorDepth::detect)
}

/// Parse `path:line` patterns from `text` and open the file at the given line
/// in the user's preferred editor (`$VISUAL` / `$EDITOR` / `vim`).
///
/// Scans lines of `text` for patterns like `src/main.rs:42`. Resolves the path
/// relative to `workspace` (if not absolute) and opens the editor. Returns
/// `true` if at least one file was opened successfully.
pub fn try_open_file_at_line(text: &str, workspace: &Path) -> bool {
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_else(|| "vim".to_string());

    let mut any_opened = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some((before, after)) = trimmed.rsplit_once(':')
            && after.chars().all(|c| c.is_ascii_digit())
        {
            let line_num: u32 = after.parse().unwrap_or(1);
            let path_str = before.trim();
            if !path_str.is_empty() && looks_like_file_path(path_str) {
                let abs_path = if Path::new(path_str).is_absolute() {
                    PathBuf::from(path_str)
                } else {
                    workspace.join(path_str)
                };
                if abs_path.is_file()
                    && Command::new(&editor)
                        .arg(format!("+{line_num}"))
                        .arg(&abs_path)
                        .spawn()
                        .is_ok()
                {
                    any_opened = true;
                }
            }
        }
    }
    any_opened
}

/// Heuristic check whether a string looks like a file path (contains a
/// directory separator or a known source file extension).
fn looks_like_file_path(s: &str) -> bool {
    if s.contains('/') || s.contains('\\') {
        return true;
    }
    // Check for a known file extension
    if let Some((_, ext)) = s.rsplit_once('.') {
        let ext = ext.trim();
        matches!(
            ext,
            "rs" | "toml"
                | "md"
                | "sh"
                | "py"
                | "js"
                | "ts"
                | "json"
                | "yaml"
                | "yml"
                | "css"
                | "html"
                | "go"
                | "c"
                | "h"
                | "cpp"
                | "hpp"
                | "java"
                | "kt"
                | "swift"
                | "rb"
                | "php"
                | "lua"
                | "zig"
                | "mod"
                | "sum"
                | "lock"
                | "txt"
                | "ini"
                | "cfg"
                | "conf"
                | "env"
                | "gitignore"
                | "dockerfile"
                | "sql"
                | "r"
                | "ex"
                | "exs"
                | "vue"
                | "svelte"
                | "tsx"
                | "jsx"
                | "scss"
                | "sass"
                | "less"
                | "gradle"
                | "properties"
                | "xml"
                | "proto"
                | "nix"
        )
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ASSISTANT_GLYPH, ExecCell, ExecSource, GenericToolCell, HistoryCell, PlanStep,
        PlanUpdateCell, REASONING_CURSOR, REASONING_OPENER, REASONING_RAIL, TOOL_RUNNING_SYMBOLS,
        TOOL_STATUS_SYMBOL_MS, ToolCell, ToolStatus, TranscriptRenderOptions, USER_GLYPH,
        assistant_label_style_for, extract_reasoning_summary, render_thinking,
        running_status_label_with_elapsed,
    };
    use crate::deepseek_theme::Theme;
    use crate::models::{ContentBlock, Message};
    use crate::palette;
    use ratatui::style::Modifier;
    use std::time::{Duration, Instant};

    // ---- elapsed-seconds badge for long-running tools ----
    //
    // Below 3s the label stays "running" — quick reads/greps shouldn't
    // visually churn. From 3s onward the badge appears and ticks each
    // second so the user can tell the call hasn't hung.
    // ---- #423 spillover-path UI annotation ----
    //
    // When a tool result carries a `spillover_path` (set by the
    // tool-routing layer when the tool's `metadata.spillover_path` is
    // populated), the live render appends a one-line muted hint
    // pointing at the file. Transcript-mode replay leaves the hint
    // off because the full output is already inline.

    #[test]
    fn render_spillover_annotation_shows_path() {
        use std::path::PathBuf;
        let cell = GenericToolCell {
            name: "exec_shell".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("cmd: cargo build --release".to_string()),
            output: Some("very large output...".to_string()),
            prompts: None,
            spillover_path: Some(PathBuf::from(
                "/Users/dev/.deepseek/tool_outputs/call-abc12.txt",
            )),
            output_summary: None,
            is_diff: false,
        };
        let lines = cell.lines_with_mode(120, true, super::RenderMode::Live);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            joined.contains("full output:"),
            "expected annotation prefix: {joined:?}"
        );
        assert!(
            joined.contains("/Users/dev/.deepseek/tool_outputs/call-abc12.txt"),
            "expected the spillover path: {joined:?}"
        );
    }

    #[test]
    fn render_spillover_annotation_omitted_in_transcript_mode() {
        use std::path::PathBuf;
        // Transcript mode is for replay; the full output is already
        // inline so the annotation would just be redundant.
        let cell = GenericToolCell {
            name: "exec_shell".to_string(),
            status: ToolStatus::Success,
            input_summary: None,
            output: Some("output".to_string()),
            prompts: None,
            spillover_path: Some(PathBuf::from("/tmp/spill.txt")),
            output_summary: None,
            is_diff: false,
        };
        let lines = cell.lines_with_mode(120, true, super::RenderMode::Transcript);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            !joined.contains("full output:"),
            "annotation should be omitted in transcript mode: {joined:?}"
        );
    }

    #[test]
    fn render_spillover_annotation_omitted_when_no_path_set() {
        // The common case: most tool results don't trigger spillover.
        let cell = GenericToolCell {
            name: "read_file".to_string(),
            status: ToolStatus::Success,
            input_summary: None,
            output: Some("contents".to_string()),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        };
        let lines = cell.lines_with_mode(80, true, super::RenderMode::Live);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(!joined.contains("full output:"), "{joined:?}");
    }

    #[test]
    fn render_spillover_annotation_truncates_to_width() {
        use std::path::PathBuf;
        let long_path = "/Users/dev/.deepseek/tool_outputs/this-is-a-very-long-tool-call-id-that-will-not-fit-in-narrow-widths.txt";
        let cell = GenericToolCell {
            name: "exec_shell".to_string(),
            status: ToolStatus::Success,
            input_summary: None,
            output: Some("output".to_string()),
            prompts: None,
            spillover_path: Some(PathBuf::from(long_path)),
            output_summary: None,
            is_diff: false,
        };
        let lines = cell.lines_with_mode(40, true, super::RenderMode::Live);
        let annotation_line = lines
            .iter()
            .find(|l| {
                l.spans
                    .iter()
                    .any(|s| s.content.as_ref().contains("full output:"))
            })
            .expect("annotation line present");
        let rendered: String = annotation_line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        // Width budget is 40; annotation line should be at most ~40 chars.
        // (Some slack for the prefix; the truncate_text ellipsis costs
        // 3 cols.)
        assert!(
            rendered.chars().count() <= 60,
            "annotation overflowed at width 40: {} chars: {rendered:?}",
            rendered.chars().count()
        );
    }

    // ---- #409 compact agent_spawn rendering ----
    //
    // The DelegateCard owns live state for spawned sub-agents; the
    // generic tool block previously duplicated that signal at 3-4 lines
    // per spawn. In live mode we now render a single compact line that
    // points at the spawned agent id; transcript-mode replay keeps the
    // full block so debug history is intact.

    #[test]
    fn extract_agent_id_pulls_id_from_json_output() {
        let output =
            r#"{"agent_id": "agent-abc12", "nickname": "Beluga", "model": "deepseek-v4-flash"}"#;
        assert_eq!(super::extract_agent_id(output), Some("agent-abc12"));
    }

    #[test]
    fn extract_agent_id_handles_extra_whitespace() {
        let output = r#"{
            "agent_id"   :    "agent-xyz",
            "model": "x"
        }"#;
        assert_eq!(super::extract_agent_id(output), Some("agent-xyz"));
    }

    #[test]
    fn extract_agent_id_returns_none_when_missing() {
        let output = r#"{"nickname": "Orca", "model": "x"}"#;
        assert!(super::extract_agent_id(output).is_none());
        assert!(super::extract_agent_id("(not json)").is_none());
        assert!(super::extract_agent_id("").is_none());
    }

    #[test]
    fn extract_agent_id_returns_none_for_empty_id() {
        let output = r#"{"agent_id": "", "model": "x"}"#;
        assert!(super::extract_agent_id(output).is_none());
    }

    #[test]
    fn agent_spawn_renders_single_compact_line_in_live_mode() {
        let cell = GenericToolCell {
            name: "agent_spawn".to_string(),
            status: ToolStatus::Running,
            input_summary: Some("prompt: do thing".to_string()),
            output: Some(
                r#"{"agent_id": "agent-abc12", "nickname": "Beluga", "model": "deepseek-v4-flash"}"#
                    .to_string(),
            ),
            prompts: None,
            spillover_path: None,
                output_summary: None,
                is_diff: false,
        };
        let lines = cell.lines_with_mode(80, true, super::RenderMode::Live);
        // One header line, no details/args/output expansion.
        assert_eq!(lines.len(), 1, "expected exactly 1 line, got {:?}", lines);
        let rendered: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        // Header carries the agent id and the running status.
        assert!(
            rendered.contains("agent-abc12"),
            "expected agent id in header: {rendered:?}"
        );
        assert!(
            rendered.contains("running"),
            "expected status in header: {rendered:?}"
        );
        // No verbose `args:` / `name:` rows.
        assert!(
            !rendered.contains("args"),
            "args should be hidden: {rendered:?}"
        );
    }

    #[test]
    fn agent_spawn_pending_render_uses_placeholder_id() {
        // No output yet → use the … placeholder so the user still sees a
        // header line during the brief gap between tool-call-started and
        // the spawn returning the agent_id.
        let cell = GenericToolCell {
            name: "agent_spawn".to_string(),
            status: ToolStatus::Running,
            input_summary: Some("prompt: do thing".to_string()),
            output: None,
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        };
        let lines = cell.lines_with_mode(80, true, super::RenderMode::Live);
        assert_eq!(lines.len(), 1);
        let rendered: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(rendered.contains('\u{2026}'), "{rendered:?}"); // …
    }

    #[test]
    fn agent_spawn_transcript_mode_keeps_full_block() {
        // Transcript mode is for replay/debug — preserve the full block
        // so session export still carries the args/output verbatim.
        let cell = GenericToolCell {
            name: "agent_spawn".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("prompt: do thing".to_string()),
            output: Some(
                r#"{"agent_id": "agent-abc12", "model": "deepseek-v4-flash"}"#.to_string(),
            ),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        };
        let lines = cell.lines_with_mode(80, true, super::RenderMode::Transcript);
        // Transcript mode emits header + name kv + (no args, output present)
        // + output rows. At minimum more than the live one-liner.
        assert!(lines.len() > 1, "expected verbose transcript render");
    }

    #[test]
    fn other_tools_are_unaffected_by_agent_spawn_compact_path() {
        // Only `agent_spawn` is collapsed — `read_file` and friends
        // continue to render their normal multi-line block in live mode.
        let cell = GenericToolCell {
            name: "read_file".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("path: foo.rs".to_string()),
            output: Some("first line\nsecond line\nthird line".to_string()),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        };
        let lines = cell.lines_with_mode(80, true, super::RenderMode::Live);
        assert!(
            lines.len() > 1,
            "non-spawn tools should keep their full block"
        );
    }

    // ---- #403 concise todo / checklist update rendering ----
    //
    // The tool emits an "Updated todo #N to STATUS" leading line plus a
    // JSON snapshot. The renderer should detect the prefix and produce
    // a compact one-line state-change card instead of dumping the full
    // item list every time.

    #[test]
    fn parse_update_prefix_recognises_todo_form() {
        let parsed =
            super::parse_update_prefix("Updated todo #3 to in_progress\n{ \"items\": [...] }");
        assert_eq!(
            parsed,
            Some(super::ChecklistChange {
                id: 3,
                status: "in_progress".to_string(),
            }),
        );
    }

    #[test]
    fn parse_update_prefix_recognises_checklist_form() {
        let parsed =
            super::parse_update_prefix("Updated checklist #7 to completed\n{ \"items\": [] }");
        assert_eq!(
            parsed,
            Some(super::ChecklistChange {
                id: 7,
                status: "completed".to_string(),
            }),
        );
    }

    #[test]
    fn parse_update_prefix_returns_none_for_writes() {
        // `todo_write` / `checklist_write` outputs don't start with
        // "Updated …" — they should fall through to the full-card path.
        assert!(super::parse_update_prefix("{ \"items\": [] }").is_none());
        assert!(super::parse_update_prefix("Wrote 5 todos\n{}").is_none());
    }

    #[test]
    fn parse_update_prefix_returns_none_for_malformed() {
        // Missing arrow/status → fall through.
        assert!(super::parse_update_prefix("Updated todo #3\n").is_none());
        // Non-numeric id → fall through.
        assert!(super::parse_update_prefix("Updated todo #foo to done\n").is_none());
    }

    #[test]
    fn render_checklist_change_card_shows_only_changed_item() {
        // Build a snapshot with three items; render the change for #2.
        let snapshot = super::ChecklistSnapshot {
            items: vec![
                super::ChecklistItemSnapshot {
                    content: "Read the spec".to_string(),
                    status: "completed".to_string(),
                },
                super::ChecklistItemSnapshot {
                    content: "Write the test".to_string(),
                    status: "in_progress".to_string(),
                },
                super::ChecklistItemSnapshot {
                    content: "Land the PR".to_string(),
                    status: "pending".to_string(),
                },
            ],
            completion_pct: 33,
            completed: 1,
            total: 3,
        };
        let change = super::ChecklistChange {
            id: 2,
            status: "in_progress".to_string(),
        };
        let lines = super::render_checklist_change_card(
            "todo_update",
            ToolStatus::Success,
            &snapshot,
            &change,
            80,
            true,
        );
        // Header + change line + summary affordance = 3 lines.
        assert!(lines.len() >= 3, "expected ≥3 lines, got {}", lines.len());

        // The change line should mention the title and the new status,
        // and should NOT include the other two item titles (that's the
        // whole point — concise rendering).
        let change_line: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(change_line.contains("#2"), "missing id: {change_line:?}");
        assert!(
            change_line.contains("Write the test"),
            "missing title: {change_line:?}"
        );
        assert!(
            change_line.contains("in_progress"),
            "missing status: {change_line:?}"
        );
        assert!(
            !change_line.contains("Land the PR"),
            "should not show other items: {change_line:?}"
        );
        assert!(
            !change_line.contains("Read the spec"),
            "should not show other items: {change_line:?}"
        );

        // The summary line carries the count + Alt+V hint.
        let summary_line: String = lines
            .last()
            .unwrap()
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(summary_line.contains("3 items"), "{summary_line:?}");
        assert!(summary_line.contains("Alt+V"), "{summary_line:?}");
    }

    #[test]
    fn render_checklist_change_card_handles_missing_title_gracefully() {
        // If the change targets an out-of-range id, the title falls
        // back to a placeholder rather than crashing.
        let snapshot = super::ChecklistSnapshot {
            items: vec![super::ChecklistItemSnapshot {
                content: "only item".to_string(),
                status: "pending".to_string(),
            }],
            completion_pct: 0,
            completed: 0,
            total: 1,
        };
        let change = super::ChecklistChange {
            id: 99,
            status: "completed".to_string(),
        };
        let lines = super::render_checklist_change_card(
            "todo_update",
            ToolStatus::Success,
            &snapshot,
            &change,
            80,
            true,
        );
        let change_line: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(change_line.contains("#99"));
        assert!(change_line.contains("(missing title)"));
    }

    #[test]
    fn running_status_label_omits_elapsed_below_threshold() {
        assert_eq!(running_status_label_with_elapsed(0), "running");
        assert_eq!(running_status_label_with_elapsed(1), "running");
        assert_eq!(running_status_label_with_elapsed(2), "running");
    }

    #[test]
    fn running_status_label_appends_elapsed_at_three_seconds() {
        assert_eq!(running_status_label_with_elapsed(3), "running (3s)");
        assert_eq!(running_status_label_with_elapsed(7), "running (7s)");
        assert_eq!(running_status_label_with_elapsed(120), "running (120s)");
    }

    #[test]
    fn extract_reasoning_summary_prefers_summary_block() {
        let text = "Thinking...\nSummary: First line\nSecond line\n\nTail";
        let summary = extract_reasoning_summary(text).expect("summary should exist");
        assert_eq!(summary, "First line\nSecond line");
    }

    #[test]
    fn extract_reasoning_summary_falls_back_to_full_text() {
        let text = "Line one\nLine two";
        let summary = extract_reasoning_summary(text).expect("summary should exist");
        assert_eq!(summary, "Line one\nLine two");
    }

    #[test]
    fn archived_context_metadata_preserves_spaces_in_attributes() {
        let msg = Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "<archived_context level=\"1\" range=\"msg 0-128\" tokens=\"2499\" density=\"~2,500 tokens\" model=\"deepseek-v4-flash\" timestamp=\"2026-04-28T00:00:00Z\">\nSummary body\n</archived_context>".to_string(),
                cache_control: None,
            }],
        };

        let cells = super::history_cells_from_message(&msg);
        assert_eq!(cells.len(), 1);
        let HistoryCell::ArchivedContext {
            level,
            range,
            tokens,
            density,
            model,
            timestamp,
            summary,
        } = &cells[0]
        else {
            panic!("expected archived context cell");
        };

        assert_eq!(*level, 1);
        assert_eq!(range, "msg 0-128");
        assert_eq!(tokens, "2499");
        assert_eq!(density, "~2,500 tokens");
        assert_eq!(model, "deepseek-v4-flash");
        assert_eq!(timestamp, "2026-04-28T00:00:00Z");
        assert_eq!(summary, "Summary body");
    }

    #[test]
    fn render_thinking_collapsed_shows_details_affordance() {
        let lines = render_thinking(
            "Summary: First line\nSecond line\nThird line\nFourth line\nFifth line",
            80,
            false,
            Some(2.0),
            true,
            false,
        );
        let text = lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<String>();
        assert!(text.contains("thinking collapsed; press Ctrl+O for full text"));
        assert!(text.contains("thinking"));
    }

    #[test]
    fn render_thinking_streaming_collapsed_shows_live_content() {
        // #861 RC4 / #1324: during a live thinking block in collapsed view,
        // the body must NOT be blanked out. Users want to watch the model
        // think; the previous behaviour stalled on a "thinking..." spinner
        // until ThinkingComplete fired.
        let lines = render_thinking(
            "Step 1: read the code\nStep 2: trace the call\nStep 3: form a hypothesis",
            80,
            true, // streaming
            None, // no duration yet
            true, // collapsed
            true, // low_motion (no cursor noise to grep)
        );
        let text = lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<String>();
        assert!(
            text.contains("Step 3: form a hypothesis"),
            "the most recent thinking line must be visible during streaming, got: {text}"
        );
        // "thinking..." placeholder must not be the only thing rendered.
        assert!(
            !text.contains("thinking..."),
            "raw content present means the placeholder line should not be drawn, got: {text}"
        );
    }

    #[test]
    fn render_thinking_streaming_truncated_shows_continues_affordance() {
        // #861 RC4: when a streaming thinking block exceeds the line cap,
        // surface a live affordance pointing at Ctrl+O. The earlier code
        // suppressed the affordance unless `!streaming`.
        let long = (1..=10)
            .map(|i| format!("Reasoning line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = render_thinking(&long, 80, true, None, true, true);
        let text = lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<String>();
        assert!(
            text.contains("thinking continues; press Ctrl+O for full text"),
            "streaming-truncation affordance missing, got: {text}"
        );
        // The most recent line must be the visible tail (head dropped).
        assert!(
            text.contains("Reasoning line 10"),
            "tail line missing, got: {text}"
        );
        assert!(
            !text.contains("Reasoning line 1\n"),
            "head should be clipped, got: {text}"
        );
    }

    #[test]
    fn tool_lines_with_options_respects_low_motion_in_default_path() {
        // Use a 2× cycle offset so the animated frame lands on index 2,
        // which is maximally far from index 0. This avoids flaky failures on
        // platforms with coarse timer resolution (Windows ≈ 15.6 ms) and
        // gives 3600 ms of headroom before the index could wrap back to 0
        // (indices 2 → 3 → 0 requires two more full cycles).
        let started_at = Some(Instant::now() - Duration::from_millis(TOOL_STATUS_SYMBOL_MS * 2));
        let cell = HistoryCell::Tool(ToolCell::Exec(ExecCell {
            command: "echo hi".to_string(),
            status: ToolStatus::Running,
            output: None,
            started_at,
            duration_ms: None,
            source: ExecSource::Assistant,
            interaction: None,
            output_summary: None,
        }));

        let animated = cell.lines_with_options(80, TranscriptRenderOptions::default());
        let low_motion = cell.lines_with_options(
            80,
            TranscriptRenderOptions {
                low_motion: true,
                ..TranscriptRenderOptions::default()
            },
        );

        // Index 0 is card-rail glyph (╭); the animated symbol is at index 1.
        let animated_symbol = animated[0].spans[1].content.trim();
        let low_motion_symbol = low_motion[0].spans[1].content.trim();

        // low_motion always pins to the first (static) frame.
        assert_eq!(low_motion_symbol, TOOL_RUNNING_SYMBOLS[0]);
        // The animated path should be on a different frame (index 2).
        assert_ne!(animated_symbol, TOOL_RUNNING_SYMBOLS[0]);
    }

    // === Speaker glyph tests (v0.6.6 UI redesign) ===
    //
    // The literal "Assistant" / "You" labels are replaced by the calmer
    // bullet/bar glyphs (`●` / `▎`). Only the assistant glyph pulses, and
    // only while the cell is streaming — finished turns sit at the source
    // sky color so the transcript reads as solid history.

    #[test]
    fn user_cell_renders_with_bar_glyph_not_literal_label() {
        let cell = HistoryCell::User {
            content: "hello".to_string(),
        };
        let lines = cell.lines(80);
        let head = &lines[0];
        assert_eq!(head.spans[0].content.as_ref(), USER_GLYPH);
        // No "You" literal anywhere in the rendered head line.
        let visible: String = head
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(!visible.contains("You"), "user label dropped: {visible:?}");
        assert!(visible.contains("hello"));
    }

    #[test]
    fn assistant_cell_renders_with_bullet_glyph_not_literal_label() {
        let cell = HistoryCell::Assistant {
            content: "ready".to_string(),
            streaming: false,
        };
        let lines = cell.lines(80);
        let head = &lines[0];
        assert_eq!(head.spans[0].content.as_ref(), ASSISTANT_GLYPH);
        let visible: String = head
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(
            !visible.contains("Assistant"),
            "assistant label dropped: {visible:?}"
        );
        assert!(visible.contains("ready"));
    }

    #[test]
    fn assistant_code_block_lines_do_not_get_transcript_rail() {
        let cell = HistoryCell::Assistant {
            content: "SQL:\n```sql\nSELECT\nFROM customers\n```".to_string(),
            streaming: false,
        };
        let visible: Vec<String> = cell
            .lines(80)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();

        assert_eq!(visible[0], format!("{ASSISTANT_GLYPH} SQL:"));
        for line in visible
            .iter()
            .filter(|line| line.contains("SELECT") || line.contains("FROM customers"))
        {
            assert!(
                !line.contains('\u{258F}'),
                "code block line should not inherit the transcript rail: {line:?}"
            );
        }
    }

    /// Issue #1212 repro: a multi-line SQL fence rendered after a short
    /// intro paragraph. Every code-block line — not just the first or last —
    /// must avoid the `▏` rail.
    #[test]
    fn assistant_long_code_block_keeps_every_line_rail_free() {
        let cell = HistoryCell::Assistant {
            content: "Here's the query:\n```sql\nSELECT\n  c.customer_id,\n  c.name,\n  COUNT(o.order_id) AS order_count\nFROM customers c\nJOIN orders o ON c.customer_id = o.customer_id;\n```".to_string(),
            streaming: false,
        };
        let visible: Vec<String> = cell
            .lines(80)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();

        let code_markers = ["SELECT", "customer_id", "name,", "COUNT", "FROM", "JOIN"];
        for marker in code_markers {
            let line = visible
                .iter()
                .find(|line| line.contains(marker))
                .unwrap_or_else(|| panic!("expected code line containing {marker:?}"));
            assert!(
                !line.contains('\u{258F}'),
                "code block line containing {marker:?} must not have the transcript rail: {line:?}"
            );
        }
    }

    /// Edge case: a blank line inside a fence is still a code line; it must
    /// not regress to the rail because the empty body falls through a
    /// different wrap branch.
    #[test]
    fn assistant_code_block_blank_line_keeps_no_rail() {
        let cell = HistoryCell::Assistant {
            content: "```\nfn one() {}\n\nfn two() {}\n```".to_string(),
            streaming: false,
        };
        for line in cell.lines(80).iter().skip(1) {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(
                !text.contains('\u{258F}'),
                "fence body line must stay rail-free: {text:?}"
            );
        }
    }

    /// Wrapped code lines (a single source line longer than the viewport)
    /// emit multiple rendered lines from one `Block::Code`. None of them
    /// should leak the rail.
    #[test]
    fn assistant_wrapped_code_lines_keep_no_rail() {
        let long = "let x = ".to_string() + &"abcdef ".repeat(40);
        let content = format!("```\n{long}\n```");
        let cell = HistoryCell::Assistant {
            content,
            streaming: false,
        };
        for line in cell.lines(40).iter().skip(1) {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(
                !text.contains('\u{258F}'),
                "wrapped code line must stay rail-free: {text:?}"
            );
        }
    }

    #[test]
    fn assistant_glyph_holds_full_brightness_when_idle() {
        // Idle (streaming=false) and low_motion both pin the colour to the
        // source sky — pulse only fires when actively streaming.
        let idle = assistant_label_style_for(false, false);
        let low_motion = assistant_label_style_for(true, true);
        assert_eq!(idle.fg, Some(palette::DEEPSEEK_SKY));
        assert_eq!(low_motion.fg, Some(palette::DEEPSEEK_SKY));
    }

    #[test]
    fn assistant_glyph_pulses_when_streaming_and_motion_allowed() {
        // The streaming path runs through `pulse_brightness`, which yields
        // an RGB colour scaled within 30%..100% of the source. Sample twice
        // — at least one of the samples must fall below 100% brightness, or
        // the test wouldn't be exercising the pulse at all. (We can't pin
        // the value because the function reads SystemTime::now().)
        use ratatui::style::Color;
        let mut saw_dimmed = false;
        for _ in 0..50 {
            if let Some(Color::Rgb(_, _, b)) = assistant_label_style_for(true, false).fg {
                let Color::Rgb(_, _, src_b) = palette::DEEPSEEK_SKY else {
                    panic!("DEEPSEEK_SKY must be RGB");
                };
                if b < src_b {
                    saw_dimmed = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            saw_dimmed,
            "expected the streaming pulse to dip below source brightness at least once",
        );
    }

    // === Tool-card verb-glyph tests (v0.6.6 UI redesign) ===

    #[test]
    fn exec_cell_header_uses_run_verb_glyph_and_label() {
        let cell = ExecCell {
            command: "ls".to_string(),
            status: ToolStatus::Success,
            output: Some("a\nb\n".to_string()),
            started_at: None,
            duration_ms: Some(10),
            source: ExecSource::Assistant,
            interaction: None,
            output_summary: None,
        };
        let header = &cell.lines_with_motion(80, true)[0];
        let visible: String = header
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(
            visible.contains('\u{25B6}'),
            "Run glyph `▶` present: {visible:?}"
        );
        assert!(visible.contains(" run "), "verb label `run`: {visible:?}");
        // Old literal title must be gone.
        assert!(
            !visible.contains("Shell"),
            "old `Shell` literal is gone: {visible:?}"
        );
    }

    #[test]
    fn exec_cell_header_includes_compact_command_summary() {
        let cell = ExecCell {
            command: "cargo test --workspace --all-features".to_string(),
            status: ToolStatus::Running,
            output: None,
            started_at: None,
            duration_ms: None,
            source: ExecSource::Assistant,
            interaction: None,
            output_summary: None,
        };

        let header = &cell.lines_with_motion(80, true)[0];
        let visible: String = header
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(visible.contains("run running"));
        assert!(
            visible.contains("cargo test --workspace --all-features"),
            "header should expose command target: {visible:?}"
        );
    }

    #[test]
    fn generic_tool_cell_picks_family_from_tool_name() {
        let cell = GenericToolCell {
            name: "agent_spawn".to_string(),
            status: ToolStatus::Running,
            input_summary: Some("foo".to_string()),
            output: None,
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        };
        let lines = cell.lines_with_mode(80, true, super::RenderMode::Live);
        let header_visible: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        // agent_spawn → Delegate family (◐ delegate).
        assert!(
            header_visible.contains('\u{25D0}'),
            "Delegate glyph `◐`: {header_visible:?}"
        );
        assert!(
            header_visible.contains(" delegate "),
            "verb label `delegate`: {header_visible:?}"
        );
    }

    #[test]
    fn generic_tool_cell_renders_rlm_with_rlm_label_not_swarm() {
        let cell = GenericToolCell {
            name: "rlm".to_string(),
            status: ToolStatus::Running,
            input_summary: Some("task: compare source trees".to_string()),
            output: None,
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        };
        let lines = cell.lines_with_mode(80, true, super::RenderMode::Live);
        let header_visible: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();

        assert!(
            header_visible.contains(" rlm "),
            "RLM card should identify RLM work: {header_visible:?}"
        );
        assert!(
            !header_visible.contains("swarm"),
            "RLM card must not use removed swarm wording: {header_visible:?}"
        );
    }

    // === Reasoning treatment tests (v0.6.6 UI redesign) ===

    #[test]
    fn render_thinking_uses_dotted_opener_in_header() {
        let lines = render_thinking("Step one\nStep two", 80, false, Some(2.0), false, true);
        let header = &lines[0];
        // First span carries `…` followed by a space.
        assert!(
            header.spans[0].content.starts_with(REASONING_OPENER),
            "header opener: {:?}",
            header.spans[0].content
        );
    }

    #[test]
    fn render_thinking_body_lines_use_dashed_rail_and_italic() {
        let lines = render_thinking(
            "concrete reasoning content",
            80,
            /*streaming*/ false,
            Some(1.0),
            /*collapsed*/ false,
            /*low_motion*/ true,
        );
        // Header is index 0; first body line is index 1.
        assert!(lines.len() >= 2, "expected at least one body line");
        let body = &lines[1];
        assert_eq!(
            body.spans[0].content.as_ref(),
            REASONING_RAIL,
            "body rail must be the dashed `╎ ` glyph"
        );
        // The body span should carry italic.
        let italic_seen = body
            .spans
            .iter()
            .skip(1)
            .any(|span| span.style.add_modifier.contains(Modifier::ITALIC));
        assert!(italic_seen, "body content should carry italic modifier");
    }

    #[test]
    fn render_thinking_streaming_appends_cursor_when_motion_allowed() {
        let lines = render_thinking(
            "ongoing reasoning...",
            80,
            /*streaming*/ true,
            None,
            /*collapsed*/ false,
            /*low_motion*/ false,
        );
        // Last line is the most recent body line — cursor lives there.
        let last = lines.last().expect("body line present");
        let last_span = last.spans.last().expect("trailing span present");
        assert!(
            last_span.content.contains(REASONING_CURSOR),
            "expected trailing cursor `▎` on last streaming body line, got {:?}",
            last_span.content
        );
    }

    #[test]
    fn render_thinking_streaming_omits_cursor_when_low_motion() {
        let lines = render_thinking(
            "ongoing reasoning...",
            80,
            /*streaming*/ true,
            None,
            /*collapsed*/ false,
            /*low_motion*/ true,
        );
        let last = lines.last().expect("body line present");
        let visible: String = last
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(
            !visible.contains(REASONING_CURSOR),
            "low_motion must suppress the streaming cursor: {visible:?}"
        );
    }

    // === Theme parity tests ===
    //
    // These lock the visible color/style choices for one plan cell and one
    // tool cell against `deepseek_theme::Theme::dark()`. The render path is
    // unchanged in shape; the assertions just guarantee a future skin swap
    // (or accidental drift) is caught here instead of at runtime.

    #[test]
    fn plan_update_cell_renders_with_dark_theme_tokens() {
        let theme = Theme::dark();
        let cell = PlanUpdateCell {
            explanation: None,
            steps: vec![
                PlanStep {
                    step: "scan repo".to_string(),
                    status: "completed".to_string(),
                },
                PlanStep {
                    step: "extract theme".to_string(),
                    status: "in_progress".to_string(),
                },
                PlanStep {
                    step: "land tests".to_string(),
                    status: "pending".to_string(),
                },
            ],
            status: ToolStatus::Running,
        };

        let lines = cell.lines_with_motion(80, true);

        // Header: "<spinner> <family-glyph> <verb> <state>" (v0.6.6 layout).
        // PlanUpdate has no canonical family yet, so it falls into the
        // Generic bullet glyph + "tool" verb. The shape and colour wiring
        // is what matters for the theme parity; the verb text moves with
        // the redesign.
        // PlanUpdate does NOT use card-rail wrapping (separate render path).
        let header = &lines[0];
        let symbol_span = &header.spans[0];
        let glyph_span = &header.spans[1];
        let title_span = &header.spans[2];
        let state_span = &header.spans[4];

        assert_eq!(
            symbol_span.style.fg,
            Some(theme.tool_running_accent),
            "running header symbol should use the dark theme running accent"
        );
        assert_eq!(
            glyph_span.style.fg,
            Some(theme.tool_running_accent),
            "family glyph rides the same status colour as the spinner"
        );
        assert_eq!(
            title_span.content.as_ref(),
            "tool",
            "PlanUpdate routes to Generic family → 'tool' verb",
        );
        assert_eq!(title_span.style.fg, Some(theme.tool_title_color));
        assert!(
            title_span.style.add_modifier.contains(Modifier::BOLD),
            "tool title should be bold"
        );
        assert_eq!(
            state_span.content.as_ref(),
            "running",
            "running PlanUpdate should label state as 'running'"
        );
        assert_eq!(state_span.style.fg, Some(theme.tool_running_accent));

        // Each step row: ["▏ ", "<marker>:", " ", "<step>"]
        let step_line = &lines[1];
        let label_span = &step_line.spans[1];
        let value_span = &step_line.spans[3];
        assert_eq!(
            label_span.style.fg,
            Some(theme.tool_label_color),
            "step label should use theme.tool_label_color"
        );
        assert_eq!(
            value_span.style.fg,
            Some(theme.tool_value_color),
            "step value should use theme.tool_value_color"
        );

        // Plain content stays identical so visible output does not move.
        let visible = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(visible[1].trim_end(), "▏ done: scan repo");
        assert_eq!(visible[2].trim_end(), "▏ live: extract theme");
        assert_eq!(visible[3].trim_end(), "▏ next: land tests");
    }

    #[test]
    fn exec_cell_failed_status_renders_with_dark_theme_tokens() {
        let theme = Theme::dark();
        let cell = ExecCell {
            command: "false".to_string(),
            status: ToolStatus::Failed,
            output: Some("boom".to_string()),
            started_at: None,
            duration_ms: Some(42),
            source: ExecSource::Assistant,
            interaction: None,
            output_summary: None,
        };

        let lines = cell.lines_with_motion(80, true);

        let header = &lines[0];
        let symbol_span = &header.spans[1];
        let glyph_span = &header.spans[2];
        let title_span = &header.spans[3];
        let state_span = &header.spans[5];

        assert_eq!(
            symbol_span.style.fg,
            Some(theme.tool_failed_accent),
            "failed exec header symbol should use the dark theme failed accent"
        );
        // ExecCell is family Run → glyph `▶ ` and verb `run`.
        assert!(
            glyph_span.content.starts_with('\u{25B6}'),
            "Run family glyph: {:?}",
            glyph_span.content
        );
        assert_eq!(
            title_span.content.as_ref(),
            "run",
            "ExecCell routes to Run family → 'run' verb",
        );
        assert_eq!(title_span.style.fg, Some(theme.tool_title_color));
        assert!(title_span.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(state_span.content.as_ref(), "issue");
        assert_eq!(state_span.style.fg, Some(theme.tool_failed_accent));
    }

    // === display_lines (lines_with_options) vs transcript_lines parity ===
    //
    // These lock the contract for CX#8: live view compresses thinking and
    // caps tool output, transcript view shows the full body. Both surfaces
    // must contain the first paragraph / first line of the underlying
    // content so users never lose the lede.

    fn line_text(line: &ratatui::text::Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn lines_text(lines: &[ratatui::text::Line<'static>]) -> String {
        lines.iter().map(line_text).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn long_thinking_display_is_shorter_than_transcript() {
        // Build a multi-paragraph thinking body so the live view has
        // something to compress. The first paragraph is the lede; both
        // surfaces must keep it.
        let body = "First paragraph lede.\n\
                    Second sentence of the first paragraph.\n\n\
                    Second paragraph: deeper analysis follows.\n\
                    More detail in paragraph two.\n\n\
                    Third paragraph: even more reasoning.\n\
                    With another line.\n\n\
                    Fourth paragraph: the conclusion.\n\
                    And one more line for good measure.";
        let cell = HistoryCell::Thinking {
            content: body.to_string(),
            streaming: false,
            duration_secs: Some(3.2),
        };

        let live = cell.lines_with_options(
            80,
            TranscriptRenderOptions {
                low_motion: true,
                ..TranscriptRenderOptions::default()
            },
        );
        let transcript = cell.transcript_lines(80);

        assert!(
            live.len() < transcript.len(),
            "live thinking should compress (live = {} lines, transcript = {} lines)",
            live.len(),
            transcript.len()
        );

        let live_text = lines_text(&live);
        let transcript_text = lines_text(&transcript);

        assert!(
            live_text.contains("First paragraph lede"),
            "live thinking must keep the lede: {live_text}"
        );
        assert!(
            transcript_text.contains("First paragraph lede"),
            "transcript thinking must keep the lede"
        );
        assert!(
            transcript_text.contains("Fourth paragraph"),
            "transcript thinking must keep the full body"
        );
        assert!(
            !live_text.contains("Fourth paragraph"),
            "live thinking must drop the tail when collapsed"
        );
        assert!(
            live_text.contains("press Ctrl+O for full text"),
            "live thinking must offer the pager affordance"
        );
        assert!(
            !transcript_text.contains("press Ctrl+O for full text"),
            "transcript thinking must not include the live affordance"
        );
    }

    #[test]
    fn short_thinking_display_equals_transcript() {
        // A single-line thinking body has nothing to compress; live and
        // transcript surfaces should agree.
        let cell = HistoryCell::Thinking {
            content: "One brief reasoning step.".to_string(),
            streaming: false,
            duration_secs: Some(0.4),
        };

        let live = cell.lines_with_options(
            80,
            TranscriptRenderOptions {
                low_motion: true,
                ..TranscriptRenderOptions::default()
            },
        );
        let transcript = cell.transcript_lines(80);

        let live_text = lines_text(&live);
        let transcript_text = lines_text(&transcript);

        assert_eq!(
            live_text, transcript_text,
            "short thinking must render identically on both surfaces"
        );
        assert!(
            !live_text.contains("press Ctrl+O for full text"),
            "short thinking must not show the collapse affordance"
        );
    }

    #[test]
    fn tool_exec_live_caps_output_transcript_does_not() {
        // Live mode renders head+tail with card-rail wrapping and "Alt+V" affordance.
        // Transcript mode emits the full output uncapped.
        let total_output_lines = 30usize;
        let output = (0..total_output_lines)
            .map(|i| format!("output line {i:02}"))
            .collect::<Vec<_>>()
            .join("\n");

        let cell = HistoryCell::Tool(ToolCell::Exec(ExecCell {
            command: "noisy_script.sh".to_string(),
            status: ToolStatus::Success,
            output: Some(output),
            started_at: None,
            duration_ms: Some(120),
            source: ExecSource::Assistant,
            interaction: None,
            output_summary: None,
        }));

        let live = cell.lines_with_options(
            80,
            TranscriptRenderOptions {
                low_motion: true,
                ..TranscriptRenderOptions::default()
            },
        );
        let transcript = cell.transcript_lines(80);

        let live_text = lines_text(&live);
        let transcript_text = lines_text(&transcript);

        assert!(
            live.len() < transcript.len(),
            "live exec output must be shorter than transcript exec output (live={}, transcript={})",
            live.len(),
            transcript.len()
        );
        assert!(
            live_text.contains("Alt+V for details"),
            "live exec output must surface the expand affordance: {live_text}"
        );
        assert!(
            !transcript_text.contains("Alt+V for details"),
            "transcript exec output must not include the expand affordance"
        );
        assert!(transcript_text.contains("output line 00"));
        // The middle should only appear in the transcript, since the live
        // view truncates the head/tail around the cap.
        assert!(
            transcript_text.contains("output line 15"),
            "transcript must include the middle of the exec output"
        );
        // Last line should appear in both because the live view shows
        // head + tail around an omission marker.
        let last = format!("output line {:02}", total_output_lines - 1);
        assert!(transcript_text.contains(&last));
    }

    #[test]
    fn generic_tool_cell_renders_prompts_as_indexed_rows() {
        // When prompts are populated by a fan-out tool, each child shows on
        // its own row instead of the inline `args:` summary so the user can
        // read what each child was asked.
        let cell = HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "future_fanout_tool".to_string(),
            status: ToolStatus::Running,
            input_summary: Some("prompts: <3 items>".to_string()),
            output: None,
            prompts: Some(vec![
                "Summarize the README".to_string(),
                "List the public types in client.rs".to_string(),
                "Diff this commit against main".to_string(),
            ]),
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        }));
        let text = lines_text(&cell.lines(80));

        assert!(text.contains("[0] Summarize the README"));
        assert!(text.contains("[1] List the public types in client.rs"));
        assert!(text.contains("[2] Diff this commit against main"));
        // The inline args summary must not also be emitted — we replaced it
        // with the per-child rows.
        assert!(
            !text.contains("args: prompts:"),
            "inline `args:` summary must be suppressed when per-prompt rows render"
        );
    }

    #[test]
    fn generic_tool_cell_falls_back_to_args_when_prompts_none() {
        // Non-fan-out tools keep the existing `args:` summary so behavior
        // doesn't drift for everything else.
        let cell = HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "file_search".to_string(),
            status: ToolStatus::Running,
            input_summary: Some("query: foo".to_string()),
            output: None,
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        }));
        let text = lines_text(&cell.lines(80));
        assert!(text.contains("query: foo"));
    }

    #[test]
    fn generic_tool_cell_preserves_multi_line_output_in_transcript() {
        // Repro for #80: a `git diff --stat`-shaped tool result should keep
        // its newlines on the transcript surface — one file per row, not
        // squashed into a single line.
        let diff_stat = "Cargo.lock                |  1 +\n\
                         crates/cli/Cargo.toml     |  1 +\n\
                         crates/cli/src/main.rs    | 47 ++++++\n\
                         crates/config/src/lib.rs  | 27 ++++\n\
                         crates/tui/src/mcp.rs     | 384 +++++";

        let cell = HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "exec_shell".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("command: git diff --stat".to_string()),
            output: Some(diff_stat.to_string()),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        }));

        let transcript_text = lines_text(&cell.transcript_lines(80));

        // Each file path must appear on its own row in the transcript.
        for needle in [
            "Cargo.lock",
            "crates/cli/Cargo.toml",
            "crates/cli/src/main.rs",
            "crates/config/src/lib.rs",
            "crates/tui/src/mcp.rs",
        ] {
            assert!(
                transcript_text.contains(needle),
                "transcript missing '{needle}': {transcript_text}"
            );
        }
        // The pre-fix bug: result line containing
        // "Cargo.lock | 1 + crates/cli/Cargo.toml" — joined into one row.
        // With the fix, the diff-stat pipes are still present per-line, but
        // adjacent file paths are on separate rendered rows. Assert that the
        // first file's line ends before the second begins.
        let lines: Vec<&str> = transcript_text.lines().collect();
        let cargo_lock_line = lines
            .iter()
            .find(|l| l.contains("Cargo.lock"))
            .expect("Cargo.lock row must exist");
        assert!(
            !cargo_lock_line.contains("crates/cli/Cargo.toml"),
            "Cargo.lock row must not also contain the second file: {cargo_lock_line}"
        );
    }

    #[test]
    fn generic_tool_cell_caps_multi_line_output_in_live_with_affordance() {
        // Live (in-progress / active-cell) view caps long output at
        // TOOL_OUTPUT_LINE_LIMIT (=6) and shows a "+N more lines" affordance.
        let total = 30usize;
        let output = (0..total)
            .map(|i| format!("row {i:02}: payload"))
            .collect::<Vec<_>>()
            .join("\n");

        let cell = HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "exec_shell".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("command: ls".to_string()),
            output: Some(output),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        }));

        let live = cell.lines_with_options(80, TranscriptRenderOptions::default());
        let transcript = cell.transcript_lines(80);

        assert!(
            live.len() < transcript.len(),
            "live generic-tool output must be shorter than transcript (live={}, transcript={})",
            live.len(),
            transcript.len(),
        );
        let live_text = lines_text(&live);
        assert!(
            live_text.contains("Alt+V for details"),
            "live view must show pager affordance: {live_text}"
        );
        let transcript_text = lines_text(&transcript);
        assert!(transcript_text.contains("row 29"));
    }

    #[test]
    fn generic_tool_output_live_renders_card_rail() {
        let output = (0..24usize)
            .map(|i| format!("line {i:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let cell = HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "exec_shell".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("command: noisy".to_string()),
            output: Some(output),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        }));

        let live_text =
            lines_text(&cell.lines_with_options(80, TranscriptRenderOptions::default()));

        // Card-rail wrapping: first line starts with ╭, last with ╰.
        assert!(
            live_text.starts_with('\u{256D}'),
            "live view must start with card-rail top glyph ╭: {live_text}"
        );
        assert!(live_text.contains("Alt+V for details"));
        assert!(live_text.contains("line 00"));
        assert!(live_text.contains("line 23"));
    }

    #[test]
    fn tool_output_live_preserves_error_card_rail() {
        let output = [
            "start",
            "still starting",
            "middle noise 1",
            "fatal: failed to read /tmp/deepseek/config.toml",
            "middle noise 2",
            "see https://example.test/build/log for details",
            "middle noise 3",
            "almost done",
            "final line",
        ]
        .join("\n");
        let cell = HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "exec_shell".to_string(),
            status: ToolStatus::Failed,
            input_summary: Some("command: tool".to_string()),
            output: Some(output),
            prompts: None,
            spillover_path: None,
            output_summary: Some("Error: failed to read config".to_string()),
            is_diff: false,
        }));

        let live_text =
            lines_text(&cell.lines_with_options(80, TranscriptRenderOptions::default()));

        // Live mode: one-line summary + expand affordance.
        assert!(
            live_text.contains("Alt+V for details"),
            "live view must show expand affordance: {live_text}"
        );
        // The pre-computed summary captures the first meaningful content.
        assert!(
            live_text.contains("Error:") || live_text.contains("fatal:"),
            "live summary should capture error text: {live_text}"
        );
    }

    // === ErrorEnvelope severity → cell color tests (#66) ===

    /// Snapshot: an `Error`-severity cell uses the red status palette token
    /// for both the leading "Error" label glyph and the body. This is the
    /// load-bearing visual signal that distinguishes an error cell from a
    /// neutral system note.
    #[test]
    fn error_severity_cell_renders_in_red() {
        let cell = HistoryCell::Error {
            message: "Authentication failed: invalid API key".to_string(),
            severity: crate::error_taxonomy::ErrorSeverity::Error,
        };
        let lines = cell.lines(80);
        assert!(
            !lines.is_empty(),
            "error cell must render at least one line"
        );

        let head = &lines[0];
        let label_span = &head.spans[0];
        assert_eq!(label_span.content.as_ref(), "Error");
        assert_eq!(label_span.style.fg, Some(palette::STATUS_ERROR));
        assert!(label_span.style.add_modifier.contains(Modifier::BOLD));

        // The body carries the error message and is rendered in the same red.
        let body_text = lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<String>();
        assert!(body_text.contains("Authentication failed"));
        // Find a span whose text contains "Authentication" and verify its color.
        let body_span = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.contains("Authentication"))
            .expect("error body span must exist");
        assert_eq!(body_span.style.fg, Some(palette::STATUS_ERROR));
    }

    /// `Warning`-severity uses amber, not red — distinguishes a transient
    /// retry hiccup from a hard failure.
    #[test]
    fn warning_severity_cell_renders_in_amber() {
        let cell = HistoryCell::Error {
            message: "Stream stalled: no data received for 60s, closing stream".to_string(),
            severity: crate::error_taxonomy::ErrorSeverity::Warning,
        };
        let lines = cell.lines(80);
        let label_span = &lines[0].spans[0];
        assert_eq!(label_span.content.as_ref(), "Warn");
        assert_eq!(label_span.style.fg, Some(palette::STATUS_WARNING));
    }

    /// `Critical` severity collapses to the same red as `Error` — both flip
    /// offline mode and both should read as the loudest signal in the
    /// transcript.
    #[test]
    fn critical_severity_cell_renders_in_red() {
        let cell = HistoryCell::Error {
            message: "API key expired".to_string(),
            severity: crate::error_taxonomy::ErrorSeverity::Critical,
        };
        let lines = cell.lines(80);
        let label_span = &lines[0].spans[0];
        assert_eq!(label_span.content.as_ref(), "Error");
        assert_eq!(label_span.style.fg, Some(palette::STATUS_ERROR));
    }

    /// `Info` severity stays neutral / dim so it doesn't draw the eye away
    /// from real failures sitting alongside it in the transcript.
    #[test]
    fn info_severity_cell_renders_in_dim() {
        let cell = HistoryCell::Error {
            message: "Reconnected".to_string(),
            severity: crate::error_taxonomy::ErrorSeverity::Info,
        };
        let lines = cell.lines(80);
        let label_span = &lines[0].spans[0];
        assert_eq!(label_span.content.as_ref(), "Info");
        assert_eq!(label_span.style.fg, Some(palette::TEXT_DIM));
    }
}
