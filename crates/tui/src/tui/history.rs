//! TUI rendering helpers for chat history and tool output.

use std::path::{Path, PathBuf};
use std::time::Instant;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::deepseek_theme::active_theme;
use crate::models::{ContentBlock, Message};
use crate::palette;
use crate::tools::plan::PlanSnapshot;
use crate::tools::review::ReviewOutput;
use crate::tui::app::TranscriptSpacing;
use crate::tui::diff_render;

mod agent_activity;
mod archived_context;
mod checklist;
mod constants;
mod message;
mod plan;
mod thinking;
mod tool_output;
mod tool_run;

use archived_context::{parse_archived_context, render_archived_context};
use checklist::{
    is_checklist_tool_name, parse_checklist_snapshot, parse_update_prefix, render_checklist_card,
    render_checklist_change_card,
};

#[cfg(test)]
use checklist::{ChecklistChange, ChecklistItemSnapshot, ChecklistSnapshot};
use constants::{
    ASSISTANT_GLYPH, FOREGROUND_SHELL_WAIT_HINT, TOOL_CARD_SUMMARY_LINES, TOOL_COMMAND_LINE_LIMIT,
    TOOL_DONE_SYMBOL, TOOL_FAILED_SYMBOL, TOOL_HEADER_SUMMARY_LIMIT, TOOL_OUTPUT_LINE_LIMIT,
    TRANSCRIPT_RAIL, USER_GLYPH,
};
#[cfg(test)]
use constants::{TOOL_RUNNING_SYMBOLS, TOOL_STATUS_SYMBOL_MS};
use message::{
    RenderedTranscriptLine, assistant_label_style_for, hard_break_copy_lines, message_body_style,
    render_message, render_message_with_copy_metadata, render_plain_message, render_user_message,
    system_body_style, system_label_style, user_body_style, user_label_style,
};
use thinking::{render_hidden_thinking_activity, render_thinking};
use tool_output::{render_exec_output_mode, render_tool_output_mode, wrap_plain_line, wrap_text};

#[cfg(test)]
use agent_activity::extract_agent_id;
pub use plan::PlanUpdateCell;
#[cfg(test)]
use thinking::extract_reasoning_summary;
#[cfg(test)]
use tool_run::ToolRunActivitySummary;
#[cfg(test)]
pub use tool_run::detect_tool_runs;
pub use tool_run::{ToolRun, detect_tool_runs_from_slices, tool_run_summary};

#[cfg(test)]
use thinking::{REASONING_CURSOR, REASONING_OPENER, REASONING_RAIL};
pub(crate) use tool_output::output_looks_like_diff;
pub use tool_output::{
    OutputRow, summarize_mcp_output, summarize_tool_args, summarize_tool_output,
};

use std::process::Command;

/// Render mode controlling whether tool/thinking cells render their compact
/// "live" form (with caps and collapsed reasoning) or their full transcript
/// form (uncapped, suitable for the pager / clipboard / message export).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// Live in-stream view: thinking is collapsed to a summary, tool output is
    /// truncated with a visible details-pager affordance.
    Live,
    /// Full transcript view: every line of reasoning and tool output is
    /// emitted, no caps, no affordance.
    Transcript,
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
            HistoryCell::User { content } => render_user_message(content, width),
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
        self.lines_with_options_folded(width, options, false)
    }

    /// Render with an explicit per-cell fold override for thinking cells.
    ///
    /// Uses XOR with the `verbose` flag so that pressing Space toggles
    /// the collapsed state *relative* to the global setting:
    /// - verbose off (default): thinking is collapsed; Space unfolds it
    /// - verbose on: thinking is expanded; Space folds it
    pub fn lines_with_options_folded(
        &self,
        width: u16,
        options: TranscriptRenderOptions,
        folded: bool,
    ) -> Vec<Line<'static>> {
        match self {
            HistoryCell::Thinking {
                streaming,
                duration_secs,
                ..
            } if !options.show_thinking => {
                if *streaming {
                    render_hidden_thinking_activity(width, *duration_secs, options.low_motion)
                } else {
                    Vec::new()
                }
            }
            HistoryCell::Thinking {
                content,
                streaming,
                duration_secs,
            } => render_thinking(
                content,
                width,
                *streaming,
                *duration_secs,
                folded ^ !options.verbose,
                options.low_motion,
            ),
            HistoryCell::Tool(cell) if !options.show_tool_details && !cell.is_failed() => {
                let mut lines = cell.lines_with_motion(width, options.low_motion);
                if lines.len() > 2 {
                    lines.truncate(2);
                    lines.push(details_affordance_line(
                        &crate::tui::key_shortcuts::tool_details_shortcut_action_hint("details"),
                        Style::default().fg(palette::TEXT_MUTED).italic(),
                    ));
                }
                lines
            }
            HistoryCell::Tool(cell) if options.calm_mode && !cell.is_failed() => {
                let mut lines = cell.lines_with_motion(width, options.low_motion);
                if lines.len() > TOOL_CARD_SUMMARY_LINES {
                    lines.truncate(TOOL_CARD_SUMMARY_LINES);
                    lines.push(details_affordance_line(
                        &crate::tui::key_shortcuts::tool_details_shortcut_action_hint("details"),
                        Style::default().fg(palette::TEXT_MUTED).italic(),
                    ));
                }
                lines
            }
            HistoryCell::Tool(cell) => cell.lines_with_motion(width, options.low_motion),
            HistoryCell::User { content } => render_user_message(content, width),
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

    #[allow(dead_code)]
    pub(crate) fn lines_with_copy_metadata(
        &self,
        width: u16,
        options: TranscriptRenderOptions,
    ) -> Vec<RenderedTranscriptLine> {
        self.lines_with_copy_metadata_folded(width, options, false)
    }

    pub(crate) fn lines_with_copy_metadata_folded(
        &self,
        width: u16,
        options: TranscriptRenderOptions,
        folded: bool,
    ) -> Vec<RenderedTranscriptLine> {
        match self {
            HistoryCell::User { content } => {
                hard_break_copy_lines(render_user_message(content, width))
            }
            HistoryCell::Assistant { content, streaming } => render_message_with_copy_metadata(
                ASSISTANT_GLYPH,
                assistant_label_style_for(*streaming, options.low_motion),
                message_body_style(),
                content,
                width,
            ),
            HistoryCell::System { content } if !is_cycle_boundary(content) => {
                render_message_with_copy_metadata(
                    "Note",
                    system_label_style(),
                    system_body_style(),
                    content,
                    width,
                )
            }
            _ => hard_break_copy_lines(self.lines_with_options_folded(width, options, folded)),
        }
    }

    /// Render the cell in transcript mode: full content, no caps, no
    /// visible details-pager affordances.
    ///
    /// Use this for full-detail pagers, clipboard exports, and any
    /// surface that wants the complete body rather than the live summary.
    /// For most variants (User / Assistant / System) this matches `lines()`;
    /// `Thinking` and `Tool` are where the live and transcript surfaces
    /// diverge.
    pub fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        match self {
            HistoryCell::User { content } => render_plain_message(
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
            ContentBlock::Thinking { thinking, .. } => {
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
            ContentBlock::ToolUse { name, input, .. } if name == "update_plan" => {
                cells.push(HistoryCell::Tool(ToolCell::PlanUpdate(PlanUpdateCell {
                    snapshot: PlanSnapshot::from_tool_input(input),
                    status: ToolStatus::Success,
                })));
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
    /// Status for cells that have a concrete lifecycle state.
    pub fn status(&self) -> Option<ToolStatus> {
        match self {
            ToolCell::Exec(cell) => Some(cell.status),
            ToolCell::Exploring(cell) => {
                let has_running = cell
                    .entries
                    .iter()
                    .any(|entry| entry.status == ToolStatus::Running);
                let has_failed = cell
                    .entries
                    .iter()
                    .any(|entry| entry.status == ToolStatus::Failed);
                Some(if has_running {
                    ToolStatus::Running
                } else if has_failed {
                    ToolStatus::Failed
                } else {
                    ToolStatus::Success
                })
            }
            ToolCell::PlanUpdate(cell) => Some(cell.status),
            ToolCell::PatchSummary(cell) => Some(cell.status),
            ToolCell::Review(cell) => Some(cell.status),
            ToolCell::Mcp(cell) => Some(cell.status),
            ToolCell::WebSearch(cell) => Some(cell.status),
            ToolCell::Generic(cell) => Some(cell.status),
            ToolCell::DiffPreview(_) | ToolCell::ViewImage(_) => Some(ToolStatus::Success),
        }
    }

    #[must_use]
    pub fn is_success(&self) -> bool {
        self.status() == Some(ToolStatus::Success)
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.status() == Some(ToolStatus::Running)
    }

    #[must_use]
    pub fn is_failed(&self) -> bool {
        self.status() == Some(ToolStatus::Failed)
    }

    /// Whether this cell should stay visible even inside a dense tool run.
    #[must_use]
    pub fn is_collapsible_guard(&self) -> bool {
        self.is_running()
            || self.is_failed()
            || matches!(
                self,
                ToolCell::Exec(_)
                    | ToolCell::PatchSummary(_)
                    | ToolCell::Review(_)
                    | ToolCell::DiffPreview(_)
                    | ToolCell::PlanUpdate(_)
            )
            || matches!(self, ToolCell::Generic(cell) if tool_run::generic_tool_name_is_collapse_guard(&cell.name) || cell.is_diff)
    }

    /// Render the tool cell into lines.
    pub fn lines(&self, width: u16) -> Vec<Line<'static>> {
        self.lines_with_motion(width, false)
    }

    pub fn lines_with_motion(&self, width: u16, low_motion: bool) -> Vec<Line<'static>> {
        self.render(width, low_motion, RenderMode::Live)
    }

    /// Full-content rendering for the pager / clipboard. Tool output that
    /// would be capped + suffixed with a details-pager hint in the live view
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
    Hydrated,
    Failed,
}

/// Shell command execution rendering data.
#[derive(Debug, Clone)]
pub struct ExecCell {
    pub command: String,
    pub status: ToolStatus,
    pub output: Option<String>,
    pub live_output: Option<String>,
    pub shell_task_id: Option<String>,
    pub owner_agent_id: Option<String>,
    pub owner_agent_name: Option<String>,
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

    /// Foreground `exec_shell` blocking the turn — eligible for Ctrl+B detach.
    fn is_foreground_shell_wait(&self) -> bool {
        self.status == ToolStatus::Running
            && self.source == ExecSource::Assistant
            && self.interaction.is_none()
    }

    pub(super) fn render(
        &self,
        width: u16,
        low_motion: bool,
        mode: RenderMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let command_summary = command_header_summary(&self.command);
        let compact_foreground_wait = self.is_foreground_shell_wait();
        let header_summary = if compact_foreground_wait {
            Some(FOREGROUND_SHELL_WAIT_HINT)
        } else {
            self.interaction
                .as_deref()
                .or(Some(command_summary.as_str()))
        };
        lines.push(render_tool_header_with_summary(
            "Shell",
            header_summary,
            tool_status_label(self.status),
            self.status,
            self.started_at,
            low_motion,
        ));

        // Foreground shell waits block the turn but do not need a verbose
        // transcript card — spinner + running badge + Ctrl+B hint only.
        // Command, live output, and artifact paths belong in the Activity sidebar
        // and `/jobs` detail surfaces.
        if compact_foreground_wait {
            return wrap_card_rail(lines);
        }

        // A successful shell call is rarely worth its full body — collapse it
        // to the single header line in live mode. The bottom shell strip owns
        // live/background detail, failures stay fully verbose so errors remain
        // visible, and Transcript mode keeps everything for the pager/clipboard.
        if mode == RenderMode::Live && self.status == ToolStatus::Success {
            if let Some(duration_ms) = self.duration_ms
                && duration_ms >= 1000
            {
                let seconds = f64::from(u32::try_from(duration_ms).unwrap_or(u32::MAX)) / 1000.0;
                lines.extend(render_compact_kv(
                    "time",
                    &format!("{seconds:.2}s"),
                    Style::default().fg(palette::TEXT_DIM),
                    width,
                ));
            }
            return wrap_card_rail(lines);
        }

        if self.status == ToolStatus::Success && self.source == ExecSource::User {
            lines.extend(render_compact_kv(
                "source",
                "started by you",
                Style::default().fg(palette::TEXT_MUTED),
                width,
            ));
        }

        if let Some(owner) = self
            .owner_agent_name
            .as_deref()
            .or(self.owner_agent_id.as_deref())
        {
            lines.extend(render_compact_kv(
                "owner",
                owner,
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
            if let Some(output) = self.output.as_ref().or(self.live_output.as_ref()) {
                lines.extend(render_exec_output_mode(
                    output,
                    width,
                    TOOL_OUTPUT_LINE_LIMIT,
                    mode,
                ));
            } else if self.status == ToolStatus::Running && self.source == ExecSource::Assistant {
                lines.extend(wrap_plain_line(
                    "  Ctrl+B moves this shell wait to /jobs.",
                    Style::default().fg(palette::TEXT_MUTED),
                    width,
                ));
            } else if self.status != ToolStatus::Running && mode == RenderMode::Transcript {
                // #3031: Suppress "(no output)" in compact/Live mode;
                // the success header is enough signal. Transcript still
                // records it for exports/clipboard/pager.
                lines.push(Line::from(Span::styled(
                    "  (no output)",
                    Style::default().fg(palette::TEXT_MUTED).italic(),
                )));
            }
        }

        if let Some(duration_ms) = self.duration_ms {
            // #3031: Suppress sub-second timing in compact mode.
            // Transcript mode always shows exact timing.
            if mode == RenderMode::Transcript || duration_ms >= 1000 {
                let seconds = f64::from(u32::try_from(duration_ms).unwrap_or(u32::MAX)) / 1000.0;
                lines.extend(render_compact_kv(
                    "time",
                    &format!("{seconds:.2}s"),
                    Style::default().fg(palette::TEXT_DIM),
                    width,
                ));
            }
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
        let any_hydrated = self
            .entries
            .iter()
            .any(|entry| entry.status == ToolStatus::Hydrated);
        let status = if all_done {
            if any_hydrated {
                ToolStatus::Hydrated
            } else {
                ToolStatus::Success
            }
        } else {
            ToolStatus::Running
        };
        let header_summary = exploring_header_summary(&self.entries);
        let multi_entry = self.entries.len() > 1;
        let header_state = if multi_entry {
            ""
        } else if all_done {
            tool_status_label(status)
        } else {
            "running"
        };
        // Search-only exploration cards read with the `find` verb so a
        // completed grep renders `find done · Searching for …` instead of the
        // incoherent `read done · Searching …` (#4145). Read/list or mixed
        // cards keep the neutral `read` verb the Workspace card has always used.
        let family = exploring_card_family(&self.entries);
        lines.push(render_tool_header_with_family_and_summary(
            family,
            header_summary.as_deref(),
            header_state,
            status,
            None,
            low_motion,
        ));

        // Dot-grid status strip — one glyph per entry, showing parallel
        // fanout at a glance: ●=done ◐=running ✕=failed.
        if self.entries.len() > 1 {
            let (done, running, failed) =
                self.entries
                    .iter()
                    .fold((0usize, 0usize, 0usize), |(d, r, f), e| match e.status {
                        ToolStatus::Success | ToolStatus::Hydrated => (d + 1, r, f),
                        ToolStatus::Running => (d, r + 1, f),
                        ToolStatus::Failed => (d, r, f + 1),
                    });
            let dots: String = self
                .entries
                .iter()
                .map(|e| match e.status {
                    ToolStatus::Success | ToolStatus::Hydrated => "\u{25CF}",
                    ToolStatus::Running => "\u{25D0}",
                    ToolStatus::Failed => "\u{2715}",
                })
                .collect();
            let counts = format!(
                "{done} done, {running} running{}",
                if failed > 0 {
                    format!(", {failed} failed")
                } else {
                    String::new()
                },
            );
            lines.push(Line::styled(
                format!("  {dots}  {counts}"),
                Style::default().fg(palette::WHALE_INFO),
            ));
        }

        for entry in &self.entries {
            if multi_entry {
                lines.extend(render_card_detail_line(
                    None,
                    &entry.label,
                    tool_value_style(),
                    width,
                ));
            } else {
                let prefix = match entry.status {
                    ToolStatus::Running => "live",
                    ToolStatus::Success => "done",
                    ToolStatus::Hydrated => "loaded",
                    ToolStatus::Failed => "issue",
                };
                lines.extend(render_compact_kv(
                    prefix,
                    &entry.label,
                    tool_value_style(),
                    width,
                ));
            }
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
                .fg(palette::WHALE_ACCENT_PRIMARY)
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
                .fg(palette::WHALE_ACCENT_PRIMARY)
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

fn should_show_raw_tool_name(
    name: &str,
    family: crate::tui::widgets::tool_card::ToolFamily,
    mode: RenderMode,
) -> bool {
    matches!(mode, RenderMode::Transcript)
        || matches!(family, crate::tui::widgets::tool_card::ToolFamily::Generic)
        || name.starts_with("mcp_")
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
        if self.name == "activity_group" {
            return agent_activity::render_activity_group(self, width);
        }

        // Issue #241: when the underlying tool is a checklist/todo update and
        // the output is parseable, render a purpose-built progress card
        // instead of dumping the JSON into the generic tool block.
        if let Some(lines) = self.try_render_as_checklist(width, low_motion, mode) {
            return lines;
        }

        // #4038 / #4122: purpose-built workflow run card (compact in live,
        // expanded in transcript) shared with the WorkflowPanel state machine.
        if let Some(lines) = self.try_render_as_workflow(width, low_motion, mode) {
            return lines;
        }

        // Sub-agent launch already gets a dedicated `DelegateCard`
        // that owns the live action tree, status, and final summary (#4133).
        // Spawns therefore render nothing here in either mode — one visible
        // artifact per delegated unit. Inspection/join calls (peek/status/
        // wait) stay as a single compact line (#4112 dogfood A5).
        if self.name == "agent" {
            if agent_activity::is_agent_inspection(self) {
                return agent_activity::render_agent_compact(self, low_motion);
            }
            // Spawn / start / run: suppress the generic tool card entirely.
            return Vec::new();
        }

        // A call to a tool that doesn't exist carries exactly one useful
        // fact: the catalog error. The full name:/args:/result: block turns
        // each model slip into a four-line card (dogfood A5) — collapse it
        // to a single header line in both render modes.
        if self.status == ToolStatus::Failed
            && let Some(output) = self.output.as_deref()
            && output.contains("is not available in the current tool catalog")
        {
            let family = crate::tui::widgets::tool_card::tool_family_for_name(&self.name);
            let summary = truncate_text(output.trim(), 200);
            return wrap_card_rail(vec![render_tool_header_with_family_and_summary(
                family,
                Some(summary.as_str()),
                tool_status_label(self.status),
                self.status,
                None,
                low_motion,
            )]);
        }

        // Live mode stays calm: successful tool calls collapse to one header
        // line, and non-read in-flight tools do the same. Failures keep their
        // body visible because error output is the useful part.
        if matches!(mode, RenderMode::Live) {
            let family = crate::tui::widgets::tool_card::tool_family_for_name(&self.name);
            let is_read_family = matches!(
                family,
                crate::tui::widgets::tool_card::ToolFamily::Read
                    | crate::tui::widgets::tool_card::ToolFamily::Find
            );
            let should_collapse = self.status == ToolStatus::Success
                || (self.status != ToolStatus::Failed && !is_read_family);
            if should_collapse {
                let header_summary = crate::tui::widgets::tool_card::tool_header_summary_for_name(
                    &self.name,
                    self.input_summary.as_deref(),
                );
                return wrap_card_rail(vec![render_tool_header_with_family_and_summary(
                    family,
                    header_summary.as_deref(),
                    tool_status_label(self.status),
                    self.status,
                    None,
                    low_motion,
                )]);
            }
        }

        let mut lines = Vec::new();
        // Map the actual tool name (e.g. `agent`, `apply_patch`) to a
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
        if should_show_raw_tool_name(&self.name, family, mode) {
            lines.extend(render_compact_kv(
                "name",
                &self.name,
                tool_value_style(),
                width,
            ));
        }

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
            let show_args = matches!(self.status, ToolStatus::Running | ToolStatus::Failed)
                || self.output.is_none();
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
                let output_mode =
                    if matches!(mode, RenderMode::Live) && self.status == ToolStatus::Failed {
                        RenderMode::Transcript
                    } else {
                        mode
                    };
                lines.extend(render_tool_output_mode(
                    output,
                    width,
                    TOOL_OUTPUT_LINE_LIMIT,
                    output_mode,
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
        // reachable via `v` on the tool detail record. This keeps the
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

    /// Render the `workflow` tool via the shared WorkflowPanel history-card
    /// renderer (#4122). Live mode stays compact (lifecycle, children, phases,
    /// failures, elapsed); transcript mode expands phase/child summaries,
    /// artifact/transcript links, final result, and failure details.
    /// Status-list payloads keep a multi-run summary card.
    fn try_render_as_workflow(
        &self,
        width: u16,
        low_motion: bool,
        mode: RenderMode,
    ) -> Option<Vec<Line<'static>>> {
        if self.name != "workflow" {
            return None;
        }
        let output = self.output.as_ref()?;
        let value: serde_json::Value = serde_json::from_str(output).ok()?;
        let is_status_list =
            value.get("action").and_then(serde_json::Value::as_str) == Some("status");
        if value.get("run_id").is_none() && !is_status_list {
            return None;
        }
        let family = crate::tui::widgets::tool_card::tool_family_for_name("workflow");
        let mut lines = Vec::new();

        if is_status_list {
            let runs = value.get("runs").and_then(serde_json::Value::as_array);
            let count = value
                .get("count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_else(|| runs.map(|r| r.len() as u64).unwrap_or(0));
            let header = format!("{count} run(s)");
            lines.push(render_tool_header_with_family_and_summary(
                family,
                Some(header.as_str()),
                tool_status_label(self.status),
                self.status,
                None,
                low_motion,
            ));
            if let Some(runs) = runs {
                for run in runs {
                    let run_id = run
                        .get("run_id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("?");
                    let status = run
                        .get("status")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("?");
                    let children = run
                        .get("child_count")
                        .and_then(serde_json::Value::as_u64)
                        .or_else(|| {
                            run.get("child_ids")
                                .and_then(serde_json::Value::as_array)
                                .map(|a| a.len() as u64)
                        })
                        .unwrap_or(0);
                    lines.extend(render_card_detail_line(
                        None,
                        &format!("{run_id} · {status} · {children} child(ren)"),
                        tool_value_style(),
                        width,
                    ));
                }
            }
            return Some(wrap_card_rail(lines));
        }

        use crate::tui::widgets::workflow_panel::{WorkflowHistoryExtras, WorkflowPanel};
        let panel = WorkflowPanel::from_run_json(&value)?;
        // Prefer the panel's lifecycle-aware status label when the tool cell
        // is still marked running but the snapshot already terminal (or vice
        // versa during live streaming).
        let header_status = match panel.lifecycle {
            crate::tui::widgets::workflow_panel::WorkflowPanelLifecycle::Failed
            | crate::tui::widgets::workflow_panel::WorkflowPanelLifecycle::Cancelled => {
                ToolStatus::Failed
            }
            crate::tui::widgets::workflow_panel::WorkflowPanelLifecycle::Succeeded => {
                ToolStatus::Success
            }
            crate::tui::widgets::workflow_panel::WorkflowPanelLifecycle::Pending
            | crate::tui::widgets::workflow_panel::WorkflowPanelLifecycle::Running => {
                if self.status == ToolStatus::Failed {
                    ToolStatus::Failed
                } else if self.status == ToolStatus::Success {
                    ToolStatus::Success
                } else {
                    ToolStatus::Running
                }
            }
        };
        let summary = panel.history_header_summary(usize::from(width).saturating_sub(18));
        lines.push(render_tool_header_with_family_and_summary(
            family,
            Some(summary.as_str()),
            tool_status_label(header_status),
            header_status,
            None,
            low_motion,
        ));
        let expanded = matches!(mode, RenderMode::Transcript);
        if expanded {
            let extras = WorkflowHistoryExtras {
                result_summary: panel.result_summary.clone(),
                source_path: panel.source_path.clone().or_else(|| {
                    value
                        .get("source_path")
                        .and_then(serde_json::Value::as_str)
                        .map(std::path::PathBuf::from)
                }),
                spillover_path: self.spillover_path.clone(),
                verification_summary: value
                    .get("verification")
                    .and_then(|v| v.get("summary"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            };
            for detail in panel.history_expanded_lines(width, &extras) {
                // history_expanded_lines omit the leading indent; re-use the
                // card detail path so rails and spacing stay consistent.
                let text: String = detail.spans.iter().map(|s| s.content.as_ref()).collect();
                lines.extend(render_card_detail_line(
                    None,
                    &text,
                    tool_value_style(),
                    width,
                ));
            }
        }
        Some(wrap_card_rail(lines))
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
                &crate::tui::key_shortcuts::tool_details_shortcut_action_hint("full command"),
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

/// Choose the verb family for an exploring card's header. A card whose entries
/// are all searches reads with the `find` verb so the completed action agrees
/// with its `Searching for …` labels (#4145); every other exploration mix keeps
/// the neutral `read` verb the Workspace card uses. The search signal is the
/// English label prefix produced by `exploring_label` in `tool_routing`.
fn exploring_card_family(entries: &[ExploringEntry]) -> crate::tui::widgets::tool_card::ToolFamily {
    use crate::tui::widgets::tool_card::ToolFamily;
    let all_search = !entries.is_empty()
        && entries
            .iter()
            .all(|entry| entry.label.starts_with("Searching"));
    if all_search {
        ToolFamily::Find
    } else {
        ToolFamily::Read
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

/// Detect whether a system message is a cycle-boundary announcement
/// (e.g. `─── cycle 0 → 1  (briefing: 2500 tokens) ───`).
fn is_cycle_boundary(content: &str) -> bool {
    content.contains("cycle")
}

/// Render a cycle-boundary system message with distinct visual styling (#395):
/// full-width line with primary accent text and bold weight, plus a thin
/// horizontal rule above for visual separation.
fn render_cycle_boundary(content: &str, width: u16) -> Vec<Line<'static>> {
    let style = Style::default()
        .fg(palette::WHALE_ACCENT_PRIMARY)
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

fn status_symbol(started_at: Option<Instant>, status: ToolStatus, low_motion: bool) -> String {
    match status {
        ToolStatus::Running => {
            crate::tui::spinner::live_bubble_frame(started_at, low_motion).to_string()
        }
        ToolStatus::Success | ToolStatus::Hydrated => TOOL_DONE_SYMBOL.to_string(),
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

    // #4148: don't let the summary echo the verb it sits next to — an
    // identity/summary that resolves to the family word itself would render a
    // duplicate like "delegate · delegate". When the summary collapses to the
    // verb, the verb already carries the signal, so drop the redundant tail.
    if let Some(summary) = summary
        .and_then(normalize_header_summary)
        .filter(|summary| !summary.eq_ignore_ascii_case(verb))
    {
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
        ToolStatus::Hydrated => "tool loaded - retry required",
        ToolStatus::Failed => "issue",
    }
}

fn tool_value_style() -> Style {
    active_theme().tool_value_style()
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
mod tests;
