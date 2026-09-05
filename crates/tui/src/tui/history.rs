//! TUI rendering helpers for chat history and tool output.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::deepseek_theme::active_theme;
use crate::localization::Locale;
use crate::models::{ContentBlock, Message};
use crate::palette;
use crate::tools::plan::PlanSnapshot;
use crate::tools::review::ReviewOutput;
use crate::tui::app::TranscriptSpacing;
use crate::tui::diff_render;
use crate::tui::motion::MotionMode;
use crate::tui::ui_text::CopyLineSeparator;

mod agent_activity;
mod archived_context;
mod automation;
mod checklist;
mod constants;
mod file_mutation;
mod latex_render;
mod message;
mod plan;
mod thinking;
mod tool_output;
mod tool_run;

use archived_context::{parse_archived_context, render_archived_context};
pub use automation::{AutomationCell, AutomationCellKind};
use checklist::{
    is_checklist_tool_name, parse_checklist_snapshot, parse_update_prefix, render_checklist_card,
    render_checklist_change_card,
};

#[cfg(test)]
use checklist::{ChecklistChange, ChecklistItemSnapshot, ChecklistSnapshot};
use constants::{
    ASSISTANT_GLYPH, FOREGROUND_SHELL_WAIT_HINT, TOOL_CARD_SUMMARY_LINES, TOOL_COMMAND_LINE_LIMIT,
    TOOL_DONE_SYMBOL, TOOL_FAILED_SYMBOL, TOOL_HEADER_SUMMARY_LIMIT, TOOL_OUTPUT_LINE_LIMIT,
    TOOL_SUCCESS_OUTPUT_PREVIEW_LINES, TOOL_SUMMARY_CARD_LINES, TRANSCRIPT_RAIL, USER_GLYPH,
};
#[cfg(test)]
use constants::{TOOL_RUNNING_SYMBOLS, TOOL_STATUS_SYMBOL_MS};
use message::{
    RenderedTranscriptLine, assistant_label_style_for, hard_break_copy_lines, message_body_style,
    render_message, render_message_with_copy_metadata_for_palette, render_plain_message,
    render_user_message, system_body_style, system_label_style, update_streaming_message_render,
    user_body_style, user_label_style,
};
#[cfg(test)]
pub(super) use thinking::render_thinking_with_analysis;
use thinking::{render_hidden_thinking_activity, render_thinking};
use tool_output::{render_exec_output_mode, render_tool_output_mode, wrap_plain_line, wrap_text};

#[cfg(test)]
use agent_activity::extract_agent_id;
pub use file_mutation::FileMutationReceipt;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReasoningAction {
    Expand,
    Collapse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TranscriptActionOwner {
    pub cell_index: usize,
    /// Rejects same-index replacements after destructive transcript changes.
    pub identity_epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReasoningActionTarget {
    pub owner: TranscriptActionOwner,
    pub action: ReasoningAction,
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
    /// Typed receipt for durable scheduled automations (fired / started /
    /// completed / failed / coalesced / missed / expired / mutated) — the
    /// one-line bulleted card that replaced bare-String `System` receipts
    /// (AUTOMATION-VISIBILITY-SPEC §2.2).
    Automation(AutomationCell),
    /// Live in-transcript card for sub-agent activity (issue #128). Owns
    /// either a single `DelegateCard` or a multi-worker `FanoutCard`; the
    /// UI re-binds it from the mailbox stream as envelopes arrive.
    SubAgent(SubAgentCell),
}

/// In-transcript sub-agent cell — either a single delegate or a fanout.
/// State mutates over the turn as mailbox envelopes are drained.
/// `Shelf` is a synthetic collapsed projector for concurrent live agents.
#[derive(Debug, Clone)]
pub enum SubAgentCell {
    Delegate(crate::tui::widgets::agent_card::DelegateCard),
    Fanout(crate::tui::widgets::agent_card::FanoutCard),
}

impl SubAgentCell {
    pub fn lines(&self, width: u16) -> Vec<Line<'static>> {
        match self {
            SubAgentCell::Delegate(card) => card.render_lines(width, &crate::palette::UI_THEME),
            SubAgentCell::Fanout(card) => card.render_lines(width, &crate::palette::UI_THEME),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptRenderOptions {
    pub locale: Locale,
    pub show_thinking: bool,
    pub thinking_default_expanded: bool,
    /// Collapsed completed-thought preview rows (settings.toml).
    pub thinking_preview_lines: usize,
    pub thinking_highlight: bool,
    pub verbose: bool,
    pub show_tool_details: bool,
    pub inline_diff_mode: crate::settings::InlineDiffMode,
    pub calm_mode: bool,
    pub low_motion: bool,
    pub motion_mode: MotionMode,
    pub spacing: TranscriptSpacing,
    /// Resolved application theme mode. This keeps cached markdown syntax
    /// colors aligned with an explicit theme selection.
    pub palette_mode: palette::PaletteMode,
    /// Wrap cap (columns) for prose cells — user messages, assistant answers,
    /// and reasoning/thinking blocks. `None` spends the full content width,
    /// matching tool/status cells (#5436); `Some(n)` caps prose at `n` columns
    /// for a bounded reading measure. Resolved once per render pass from
    /// `[transcript] prose_measure` so the main cache and the full-screen
    /// overlay agree on the same effective width.
    pub(crate) prose_measure: Option<u16>,
    /// Extra raw reasoning body rows available to the newest transcript cell.
    /// The transcript cache derives this from genuinely unused viewport rows;
    /// non-layout-aware renderers and historical cells retain the compact
    /// 10/12-row fallback.
    pub(crate) reasoning_preview_extra_lines: usize,
    /// Live transcript height used by the cache to derive the extra rows
    /// above. Kept separate from the render-level budget so cached geometry is
    /// stable and explicit reasoning summaries retain their four-row cap.
    pub(crate) reasoning_preview_viewport_lines: Option<usize>,
}

impl Default for TranscriptRenderOptions {
    fn default() -> Self {
        Self {
            locale: Locale::En,
            show_thinking: true,
            thinking_highlight: true,
            thinking_default_expanded: false,
            thinking_preview_lines: 2,
            verbose: false,
            show_tool_details: true,
            inline_diff_mode: crate::settings::InlineDiffMode::Full,
            calm_mode: false,
            low_motion: false,
            motion_mode: MotionMode::Full,
            spacing: TranscriptSpacing::Comfortable,
            palette_mode: palette::PaletteMode::detect(),
            prose_measure: None,
            reasoning_preview_extra_lines: 0,
            reasoning_preview_viewport_lines: None,
        }
    }
}

impl TranscriptRenderOptions {
    /// Effective wrap width for a prose cell at the given content width.
    ///
    /// `prose_measure` caps the measure; without a cap the prose cell uses
    /// the full content width, like tool/status cells. Applied only at the
    /// live-transcript render entry points so the main cache and the
    /// full-screen overlay agree on the same effective width.
    #[must_use]
    pub(crate) fn prose_width(self, width: u16) -> u16 {
        match self.prose_measure {
            Some(cap) => width.clamp(1, cap),
            None => width.max(1),
        }
    }
}

impl HistoryCell {
    #[must_use]
    pub(crate) fn has_live_motion(&self) -> bool {
        match self {
            HistoryCell::Assistant { streaming, .. } => *streaming,
            HistoryCell::Tool(ToolCell::Generic(tool))
                if tool.name == "agent" && !agent_activity::is_agent_inspection(tool) =>
            {
                false
            }
            HistoryCell::Tool(tool) => tool.is_running(),
            HistoryCell::User { .. }
            | HistoryCell::System { .. }
            | HistoryCell::Error { .. }
            | HistoryCell::Thinking { .. }
            | HistoryCell::ArchivedContext { .. }
            | HistoryCell::Automation(_)
            | HistoryCell::SubAgent(_) => false,
        }
    }

    /// Whether this cell is a completed (non-streaming) assistant answer —
    /// the only variant a "copy the answer" affordance may serialize.
    ///
    /// This is the typed "final answer only" projection for copy surfaces:
    /// everything else — user prompts, reasoning/thinking blocks, tool calls
    /// and results, sub-agent transcripts, runtime status/system notes, and
    /// archived-context seams — is excluded by construction, and a
    /// still-streaming partial answer never qualifies. Pair it with
    /// `history_cell_to_clipboard_text` for the canonical clean payload.
    #[must_use]
    pub fn is_completed_assistant_answer(&self) -> bool {
        matches!(
            self,
            HistoryCell::Assistant {
                streaming: false,
                ..
            }
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn update_incremental_streaming_render(
        &self,
        width: u16,
        options: TranscriptRenderOptions,
        verified_append: bool,
        cache: &mut crate::tui::markdown_render::IncrementalMarkdownRenderCache,
        lines: &mut Vec<Line<'static>>,
        links: &mut Vec<Vec<crate::tui::osc8::LineLink>>,
        copy_separators: &mut Vec<CopyLineSeparator>,
        copy_prefix_widths: &mut Vec<usize>,
    ) -> Option<usize> {
        let HistoryCell::Assistant {
            content,
            streaming: true,
        } = self
        else {
            return None;
        };
        if content.trim().is_empty() {
            lines.clear();
            links.clear();
            copy_separators.clear();
            copy_prefix_widths.clear();
            *cache = crate::tui::markdown_render::IncrementalMarkdownRenderCache::default();
            return Some(0);
        }
        let width = options.prose_width(width);
        Some(update_streaming_message_render(
            cache,
            content,
            width,
            assistant_label_style_for(true, options.low_motion),
            message_body_style(),
            options.palette_mode,
            verified_append,
            lines,
            links,
            copy_separators,
            copy_prefix_widths,
        ))
    }

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
                render_error_message(message, *severity, width, true)
            }
            HistoryCell::Thinking {
                content,
                streaming,
                duration_secs,
            } => render_thinking(content, width, *streaming, *duration_secs, false, false),
            HistoryCell::Tool(cell) => cell.lines_with_motion(width, false),
            HistoryCell::SubAgent(cell) => cell.lines(width),
            HistoryCell::Automation(cell) => cell.render(width),
            HistoryCell::ArchivedContext { .. } => render_archived_context(self, width, false),
        }
    }

    #[cfg(test)]
    pub fn lines_with_options(
        &self,
        width: u16,
        options: TranscriptRenderOptions,
    ) -> Vec<Line<'static>> {
        self.lines_with_options_folded(width, options, false).0
    }

    /// Render with an explicit per-cell fold override for thinking cells.
    ///
    /// Space toggles the collapsed state *relative* to the expanded
    /// baseline, which is on when either the session is verbose or the
    /// thinking default is expanded:
    /// - baseline off (default): thinking is collapsed; Space unfolds it
    /// - baseline on: thinking is expanded; Space folds it
    pub fn lines_with_options_folded(
        &self,
        width: u16,
        options: TranscriptRenderOptions,
        folded: bool,
    ) -> (Vec<Line<'static>>, Option<ReasoningAction>) {
        let mut reasoning_action = None;
        let mut lines = match self {
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
            } => {
                let collapsed = folded ^ !(options.verbose || options.thinking_default_expanded);
                let (lines, expandable) = thinking::render_thinking_with_preview_limit(
                    content,
                    width,
                    *streaming,
                    *duration_secs,
                    collapsed,
                    options.low_motion,
                    options.thinking_highlight,
                    options.reasoning_preview_extra_lines,
                    options.thinking_preview_lines,
                );
                reasoning_action = expandable.then_some(if collapsed {
                    ReasoningAction::Expand
                } else {
                    ReasoningAction::Collapse
                });
                lines
            }
            HistoryCell::Tool(ToolCell::PatchSummary(cell)) => cell.render(
                width,
                options.low_motion,
                RenderMode::Live,
                options.inline_diff_mode,
            ),
            HistoryCell::Tool(cell) if !options.show_tool_details && !cell.is_failed() => {
                let mut lines =
                    cell.lines_with_motion_and_locale(width, options.low_motion, options.locale);
                if lines.len() > TOOL_SUMMARY_CARD_LINES {
                    lines.truncate(TOOL_SUMMARY_CARD_LINES);
                    lines.push(details_affordance_line(
                        &crate::tui::key_shortcuts::tool_details_shortcut_action_hint("details"),
                        Style::default().fg(palette::TEXT_MUTED).italic(),
                    ));
                }
                lines
            }
            HistoryCell::Tool(cell) if options.calm_mode && !cell.is_failed() => {
                let mut lines =
                    cell.lines_with_motion_and_locale(width, options.low_motion, options.locale);
                if lines.len() > TOOL_CARD_SUMMARY_LINES {
                    lines.truncate(TOOL_CARD_SUMMARY_LINES);
                    lines.push(details_affordance_line(
                        &crate::tui::key_shortcuts::tool_details_shortcut_action_hint("details"),
                        Style::default().fg(palette::TEXT_MUTED).italic(),
                    ));
                }
                lines
            }
            HistoryCell::Tool(cell) => {
                cell.lines_with_motion_and_locale(width, options.low_motion, options.locale)
            }
            HistoryCell::User { content } => render_user_message(content, width),
            HistoryCell::Assistant { content, streaming } => {
                let mut lines: Vec<Line<'static>> = render_message_with_copy_metadata_for_palette(
                    ASSISTANT_GLYPH,
                    assistant_label_style_for(*streaming, options.low_motion),
                    message_body_style(),
                    content,
                    width,
                    options.palette_mode,
                )
                .into_iter()
                .map(|rendered| rendered.line)
                .collect();
                if *streaming {
                    apply_hot_tail_to_last_line(&mut lines, options.low_motion);
                }
                lines
            }
            HistoryCell::System { .. } => self.lines(width),
            HistoryCell::Error { message, severity } => {
                render_error_message(message, *severity, width, true)
            }
            HistoryCell::SubAgent(cell) => cell.lines(width),
            HistoryCell::Automation(cell) => cell.render(width),
            HistoryCell::ArchivedContext { .. } => {
                render_archived_context(self, width, options.low_motion)
            }
        };
        if matches!(self, HistoryCell::Tool(_)) {
            match options.motion_mode {
                MotionMode::Reduced => apply_static_tool_markers(
                    &mut lines,
                    crate::tui::spinner::BRAILLE_SPINNER_STILL_FRAME,
                ),
                MotionMode::Still => {
                    apply_static_tool_markers(&mut lines, crate::tui::spinner::LIVE_STATIC_MARKER)
                }
                MotionMode::Full => {}
            }
        }
        (lines, reasoning_action)
    }

    pub(crate) fn lines_with_copy_metadata(
        &self,
        width: u16,
        options: TranscriptRenderOptions,
    ) -> Vec<RenderedTranscriptLine> {
        self.lines_with_copy_metadata_folded(width, options, false)
            .0
    }

    pub(crate) fn lines_with_copy_metadata_folded(
        &self,
        width: u16,
        options: TranscriptRenderOptions,
        folded: bool,
    ) -> (Vec<RenderedTranscriptLine>, Option<ReasoningAction>) {
        if matches!(self, HistoryCell::Thinking { .. }) {
            let (lines, action) =
                self.lines_with_options_folded(options.prose_width(width), options, folded);
            return (hard_break_copy_lines(lines), action);
        }
        let lines = match self {
            HistoryCell::User { content } => {
                hard_break_copy_lines(render_user_message(content, options.prose_width(width)))
            }
            HistoryCell::Assistant { content, streaming } => {
                let width = options.prose_width(width);
                let mut rendered = render_message_with_copy_metadata_for_palette(
                    ASSISTANT_GLYPH,
                    assistant_label_style_for(*streaming, options.low_motion),
                    message_body_style(),
                    content,
                    width,
                    options.palette_mode,
                );
                if *streaming && let Some(last) = rendered.last_mut() {
                    apply_hot_tail_to_line(&mut last.line, options.low_motion);
                }
                rendered
            }
            HistoryCell::System { content } if !is_cycle_boundary(content) => {
                render_message_with_copy_metadata_for_palette(
                    "Note",
                    system_label_style(),
                    system_body_style(),
                    content,
                    width,
                    options.palette_mode,
                )
            }
            HistoryCell::Tool(_) => self
                .lines_with_options_folded(width, options, folded)
                .0
                .into_iter()
                .map(|line| {
                    let copy_prefix_width = tool_copy_prefix_width(&line);
                    RenderedTranscriptLine {
                        line,
                        links: Vec::new(),
                        copy_prefix_width,
                        copy_separator_after: CopyLineSeparator::Newline,
                    }
                })
                .collect(),
            HistoryCell::Thinking { .. } => unreachable!("reasoning handled above"),
            _ => hard_break_copy_lines(self.lines_with_options_folded(width, options, folded).0),
        };
        (lines, None)
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
            HistoryCell::System { .. } => self.lines(width),
            HistoryCell::Error { message, severity } => {
                render_error_message(message, *severity, width, false)
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
                /*collapsed*/ false,
                /*low_motion*/ false,
            ),
            HistoryCell::Tool(cell) => cell.transcript_lines(width),
            HistoryCell::SubAgent(cell) => cell.lines(width),
            HistoryCell::Automation(cell) => cell.render(width),
            HistoryCell::ArchivedContext { .. } => render_archived_context(self, width, true),
        }
    }
}

/// Convert a message into history cells for rendering.
#[must_use]
pub fn history_cells_from_message(msg: &Message) -> Vec<HistoryCell> {
    // Model-facing Operate contract; the live receipt is the goal line.
    if crate::runtime_handoff::is_operate_contract_message(msg) {
        return Vec::new();
    }
    if let Some(display) = crate::runtime_handoff::restored_subagent_checkpoint_display(msg) {
        return vec![HistoryCell::System {
            content: display.to_string(),
        }];
    }

    let mut cells = Vec::new();

    for (block_index, block) in msg.content.iter().enumerate() {
        match block {
            ContentBlock::Text { text, .. } => {
                if is_turn_metadata_block(msg, block_index, text) {
                    continue;
                }
                if text.starts_with("[tool_history_repair]") {
                    cells.push(HistoryCell::System {
                        content: text.clone(),
                    });
                    continue;
                }
                // Check if this is an `<archived_context>` block.
                if (msg.role == "assistant"
                    || msg.role == crate::models::INTERRUPTED_ASSISTANT_ROLE)
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
                // Older sessions may contain this transport-only fallback from
                // thinking-mode tool-call replay. It was never model output,
                // so do not surface it when restoring their transcripts.
                if thinking == "(reasoning omitted)" {
                    continue;
                }
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

/// Whether this text block is a runtime-owned `<turn_meta>` envelope that
/// must stay out of the visible transcript.
///
/// Current sessions persist the envelope as the trailing block of a
/// multi-block user message; sessions saved before the tail move
/// (pre-v0.8.54) carry it as the *leading* block instead, so a complete
/// envelope is hidden at any index. A single-block user message is never
/// hidden: its text is user-authored by construction, and a literal
/// `<turn_meta>` example the user typed must stay visible.
fn is_turn_metadata_block(msg: &Message, block_index: usize, text: &str) -> bool {
    if msg.role != "user" || msg.content.len() < 2 || !is_complete_turn_meta_envelope(text) {
        return false;
    }
    if block_index > 0 {
        return true;
    }
    // Leading envelope: hide it only when the trailing block is ordinary
    // text (the legacy `[turn_meta, prompt]` persisted shape). If the tail
    // block is itself an envelope, this message is the current shape and the
    // leading block is user-authored literal text that must stay visible.
    matches!(
        msg.content.last(),
        Some(ContentBlock::Text { text: tail, .. }) if !is_complete_turn_meta_envelope(tail)
    )
}

fn is_complete_turn_meta_envelope(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed
        .strip_prefix("<turn_meta>")
        .and_then(|body| body.strip_suffix("</turn_meta>"))
        .is_some()
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
    Mcp(McpToolCell),
    ViewImage(ViewImageCell),
    WebSearch(WebSearchCell),
    Generic(GenericToolCell),
}

impl ToolCell {
    /// Whether this tool cell projects durable Work state rather than a
    /// transient action receipt. Transcript rhythm uses this semantic split
    /// to keep plans, checklists, and workflows legible without teaching the
    /// renderer about individual tool payloads.
    #[must_use]
    pub(crate) fn is_durable_work_receipt(&self) -> bool {
        matches!(self, ToolCell::PlanUpdate(_))
            || matches!(
                self,
                ToolCell::Generic(cell)
                    if cell.name == "workflow" || is_checklist_tool_name(&cell.name)
            )
    }

    /// Status for cells that have a concrete lifecycle state.
    pub fn status(&self) -> Option<ToolStatus> {
        match self {
            ToolCell::Exec(cell) => Some(cell.status),
            ToolCell::Exploring(cell) => Some(cell.status()),
            ToolCell::PlanUpdate(cell) => Some(cell.status),
            ToolCell::PatchSummary(cell) => Some(cell.status),
            ToolCell::Review(cell) => Some(cell.status),
            ToolCell::Mcp(cell) => Some(cell.status),
            ToolCell::WebSearch(cell) => Some(cell.status),
            ToolCell::Generic(cell) => Some(cell.status),
            ToolCell::ViewImage(_) => Some(ToolStatus::Success),
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
                    | ToolCell::PlanUpdate(_)
            )
            || matches!(self, ToolCell::Generic(cell) if tool_run::generic_tool_name_is_collapse_guard(&cell.name) || cell.is_diff)
    }

    /// Render the tool cell into lines.
    pub fn lines(&self, width: u16) -> Vec<Line<'static>> {
        self.lines_with_motion(width, false)
    }

    pub fn lines_with_motion(&self, width: u16, low_motion: bool) -> Vec<Line<'static>> {
        self.lines_with_motion_and_locale(width, low_motion, Locale::En)
    }

    pub fn lines_with_motion_and_locale(
        &self,
        width: u16,
        low_motion: bool,
        locale: Locale,
    ) -> Vec<Line<'static>> {
        self.render_with_locale(width, low_motion, RenderMode::Live, locale)
    }

    /// Full-content rendering for the pager / clipboard. Tool output that
    /// would be capped + suffixed with a details-pager hint in the live view
    /// is emitted in full here.
    pub fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.transcript_lines_with_locale(width, Locale::En)
    }

    pub fn transcript_lines_with_locale(&self, width: u16, locale: Locale) -> Vec<Line<'static>> {
        self.render_with_locale(
            width,
            /*low_motion*/ false,
            RenderMode::Transcript,
            locale,
        )
    }

    fn render_with_locale(
        &self,
        width: u16,
        low_motion: bool,
        mode: RenderMode,
        locale: Locale,
    ) -> Vec<Line<'static>> {
        match self {
            ToolCell::Exec(cell) => cell.render_with_locale(width, low_motion, mode, locale),
            ToolCell::Exploring(cell) => {
                cell.lines_with_motion_and_locale(width, low_motion, locale)
            }
            ToolCell::PlanUpdate(cell) => cell.lines_with_motion(width, low_motion),
            ToolCell::PatchSummary(cell) => cell.render(
                width,
                low_motion,
                mode,
                crate::settings::InlineDiffMode::Full,
            ),
            ToolCell::Review(cell) => cell.render(width, low_motion, mode),
            ToolCell::Mcp(cell) => cell.render(width, low_motion, mode),
            ToolCell::ViewImage(cell) => cell.lines_with_motion(width, low_motion),
            ToolCell::WebSearch(cell) => cell.lines_with_motion(width, low_motion),
            ToolCell::Generic(cell) => {
                cell.lines_with_mode_and_locale(width, low_motion, mode, locale)
            }
        }
    }
}

/// Overall status for a tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Success,
    Hydrated,
    /// Terminal result with usable output that still needs attention.
    Warning,
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
    pub stale_elapsed_since_output_ms: Option<u64>,
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

    #[cfg(test)]
    pub(super) fn render(
        &self,
        width: u16,
        low_motion: bool,
        mode: RenderMode,
    ) -> Vec<Line<'static>> {
        self.render_with_locale(width, low_motion, mode, Locale::En)
    }

    pub(super) fn render_with_locale(
        &self,
        width: u16,
        low_motion: bool,
        mode: RenderMode,
        locale: Locale,
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
        let stale_status = self
            .stale_elapsed_since_output_ms
            .map(stale_shell_status_label);
        let receipt = tool_receipt_label(
            crate::tui::widgets::tool_card::ToolFamily::Run,
            self.status,
            self.output.as_deref(),
            locale,
        );
        let status_text = stale_status
            .as_deref()
            .map(Cow::Borrowed)
            .unwrap_or(receipt);
        lines.push(render_tool_header_with_summary(
            "Shell",
            header_summary,
            status_text.as_ref(),
            self.status,
            self.started_at,
            low_motion || stale_status.is_some(),
        ));

        // Foreground shell waits block the turn but do not need a verbose
        // transcript card — spinner + running badge + Ctrl+B hint only.
        // Command, live output, and artifact paths belong in the Activity sidebar
        // and `/jobs` detail surfaces.
        if compact_foreground_wait {
            return wrap_card_rail(lines, self.status);
        }

        // A successful shell call does not earn its full body in live mode —
        // failures stay fully verbose so errors remain visible, and Transcript
        // mode keeps everything for the pager/clipboard. But it does earn a
        // glimpse: collapsing success to the bare header meant a `run` card
        // showed literally nothing of what the command produced, and you had
        // to expand every single one to find out whether anything happened.
        // `TOOL_SUCCESS_OUTPUT_PREVIEW_LINES` rows show roughly half of real
        // successful runs in full and the opening of the rest.
        if mode == RenderMode::Live
            && self
                .output
                .as_deref()
                .is_some_and(is_truncated_output_preview)
        {
            lines.push(render_spillover_annotation(width));
            return wrap_card_rail(lines, self.status);
        }
        if mode == RenderMode::Live && self.status == ToolStatus::Success {
            if self.interaction.is_none()
                && let Some(output) = self.output.as_ref().or(self.live_output.as_ref())
            {
                lines.extend(render_exec_output_mode(
                    output,
                    width,
                    TOOL_SUCCESS_OUTPUT_PREVIEW_LINES,
                    mode,
                ));
            }
            if let Some(duration_ms) = self.duration_ms
                && duration_ms >= 1000
            {
                lines.extend(render_compact_kv(
                    "time",
                    &crate::elapsed::format_elapsed_ms(duration_ms),
                    Style::default().fg(palette::TEXT_DIM),
                    width,
                ));
            }
            return wrap_card_rail(lines, self.status);
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
            // Transcript mode always shows timing.
            if mode == RenderMode::Transcript || duration_ms >= 1000 {
                lines.extend(render_compact_kv(
                    "time",
                    &crate::elapsed::format_elapsed_ms(duration_ms),
                    Style::default().fg(palette::TEXT_DIM),
                    width,
                ));
            }
        }

        wrap_card_rail(lines, self.status)
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
    /// The cell's own lifecycle state, rolled up from its parallel entries.
    ///
    /// One derivation, used by the card header, by [`ToolCell::status`], and by
    /// the Activity surfaces — three copies of this fold had drifted apart, and
    /// the header's copy could not produce `Failed` at all, so a fan-out with a
    /// failed read painted itself `done` in the success colour.
    ///
    /// Precedence: any entry still running keeps the whole cell running; after
    /// that the loudest terminal state wins, so a single failure is never
    /// averaged away by its successful siblings.
    #[must_use]
    pub fn status(&self) -> ToolStatus {
        let any = |wanted: ToolStatus| self.entries.iter().any(|entry| entry.status == wanted);
        if any(ToolStatus::Running) {
            ToolStatus::Running
        } else if any(ToolStatus::Failed) {
            ToolStatus::Failed
        } else if any(ToolStatus::Warning) {
            ToolStatus::Warning
        } else if any(ToolStatus::Hydrated) {
            ToolStatus::Hydrated
        } else {
            ToolStatus::Success
        }
    }

    /// Render the exploring cell into lines.
    #[cfg(test)]
    pub fn lines_with_motion(&self, width: u16, low_motion: bool) -> Vec<Line<'static>> {
        self.lines_with_motion_and_locale(width, low_motion, Locale::En)
    }

    pub fn lines_with_motion_and_locale(
        &self,
        width: u16,
        low_motion: bool,
        locale: Locale,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let status = self.status();
        let all_done = status != ToolStatus::Running;
        let header_summary = exploring_header_summary(&self.entries);
        let multi_entry = self.entries.len() > 1;
        let header_state: Cow<'static, str> = if multi_entry {
            Cow::Borrowed("")
        } else if all_done {
            if status == ToolStatus::Success {
                crate::localization::tr(locale, crate::localization::MessageId::ToolReceiptDone)
            } else {
                Cow::Borrowed(tool_status_label(status))
            }
        } else {
            Cow::Borrowed("running")
        };
        // Search-only exploration cards read with the `find` verb so a
        // completed grep renders `find done · Searching for …` instead of the
        // incoherent `read done · Searching …` (#4145). Read/list or mixed
        // cards keep the neutral `read` verb the Workspace card has always used.
        let family = exploring_card_family(&self.entries);
        lines.push(render_tool_header_with_family_and_summary(
            family,
            header_summary.as_deref(),
            header_state.as_ref(),
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
                        ToolStatus::Warning => (d, r, f + 1),
                        ToolStatus::Running => (d, r + 1, f),
                        ToolStatus::Failed => (d, r, f + 1),
                    });
            // Each dot carries its own entry's state. Painting the strip one
            // flat colour made a failed parallel read indistinguishable from a
            // finished one, which is exactly the narration the cell is
            // supposed to make unnecessary.
            let mut dot_spans: Vec<Span<'static>> = vec![Span::raw("  ")];
            dot_spans.extend(self.entries.iter().map(|e| {
                let glyph = match e.status {
                    ToolStatus::Success | ToolStatus::Hydrated => "\u{25CF}",
                    ToolStatus::Warning => "!",
                    ToolStatus::Running => "\u{25D0}",
                    ToolStatus::Failed => "\u{2715}",
                };
                Span::styled(
                    glyph,
                    Style::default().fg(tool_glyph_color(e.status, family)),
                )
            }));
            let counts = format!(
                "  {done} done, {running} running{}",
                if failed > 0 {
                    format!(", {failed} failed")
                } else {
                    String::new()
                },
            );
            dot_spans.push(Span::styled(
                counts,
                Style::default().fg(tool_glyph_color(status, family)),
            ));
            lines.push(Line::from(dot_spans));
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
                    ToolStatus::Warning => "issue",
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

/// Calm outcome and exact evidence for a structured File mutation.
#[derive(Debug, Clone)]
pub struct PatchSummaryCell {
    pub path: String,
    pub summary: String,
    pub status: ToolStatus,
    pub error: Option<String>,
    pub receipt: Option<FileMutationReceipt>,
}

impl PatchSummaryCell {
    pub(super) fn render(
        &self,
        width: u16,
        low_motion: bool,
        mode: RenderMode,
        inline_diff_mode: crate::settings::InlineDiffMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let header_summary = self
            .receipt
            .as_ref()
            .map(FileMutationReceipt::outcome_label)
            .unwrap_or_else(|| self.path.clone());
        lines.push(render_tool_header_with_summary(
            "File",
            Some(&header_summary),
            tool_status_label(self.status),
            self.status,
            None,
            low_motion,
        ));
        if self.status == ToolStatus::Success
            && let Some(receipt) = self.receipt.as_ref()
        {
            lines.extend(receipt.render_inline(width, inline_diff_mode));
        } else {
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
        }
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
                .fg(palette::WHALE_ACTION)
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
                .fg(palette::WHALE_ACTION)
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
            if mode == RenderMode::Live && is_truncated_output_preview(content) {
                lines.push(render_spillover_annotation(width));
                return lines;
            }
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
    pub source: Option<String>,
    pub degraded: Option<String>,
    pub ref_count: usize,
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
        if let Some(source) = self.source.as_ref() {
            lines.extend(render_compact_kv(
                "source",
                source,
                tool_value_style(),
                width,
            ));
        }
        if let Some(degraded) = self.degraded.as_ref() {
            lines.extend(render_compact_kv(
                "degraded",
                degraded,
                tool_value_style(),
                width,
            ));
        }
        if self.ref_count > 0 {
            lines.extend(render_compact_kv(
                "citations",
                &self.ref_count.to_string(),
                tool_value_style(),
                width,
            ));
        }
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
    #[cfg(test)]
    pub fn lines_with_mode(
        &self,
        width: u16,
        low_motion: bool,
        mode: RenderMode,
    ) -> Vec<Line<'static>> {
        self.lines_with_mode_and_locale(width, low_motion, mode, Locale::En)
    }

    pub fn lines_with_mode_and_locale(
        &self,
        width: u16,
        low_motion: bool,
        mode: RenderMode,
        locale: Locale,
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
            return wrap_card_rail(
                vec![render_tool_header_with_family_and_summary(
                    family,
                    Some(summary.as_str()),
                    tool_status_label(self.status),
                    self.status,
                    None,
                    low_motion,
                )],
                self.status,
            );
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
            if should_collapse || self.spillover_path.is_some() {
                let header_summary = crate::tui::widgets::tool_card::tool_header_summary_for_name(
                    &self.name,
                    self.input_summary.as_deref(),
                );
                let mut collapsed = vec![render_tool_header_with_family_and_summary(
                    family,
                    header_summary.as_deref(),
                    &tool_receipt_label(family, self.status, self.output.as_deref(), locale),
                    self.status,
                    None,
                    low_motion,
                )];
                if self.spillover_path.is_some() {
                    collapsed.push(render_spillover_annotation(width));
                }
                return wrap_card_rail(collapsed, self.status);
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
            &tool_receipt_label(family, self.status, self.output.as_deref(), locale),
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
                if matches!(mode, RenderMode::Live) {
                    let rendered =
                        diff_render::render_diff_bounded(output, width, TOOL_OUTPUT_LINE_LIMIT);
                    lines.extend(rendered.lines);
                    if rendered.omitted_rows > 0 {
                        let detail_hint =
                            crate::tui::key_shortcuts::tool_details_shortcut_action_hint("diff");
                        lines.push(details_affordance_line(
                            &format!("+{} diff lines · {detail_hint}", rendered.omitted_rows),
                            Style::default().fg(palette::TEXT_MUTED).italic(),
                        ));
                    }
                } else {
                    // Transcript/detail mode remains the exact-evidence path;
                    // only the live frame is budgeted.
                    lines.extend(diff_render::render_diff(output, width));
                }
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

            if matches!(mode, RenderMode::Live) && self.spillover_path.is_some() {
                lines.push(render_spillover_annotation(width));
            }
        }
        wrap_card_rail(lines, self.status)
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
            return Some(wrap_card_rail(lines, self.status));
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
            crate::tui::widgets::workflow_panel::WorkflowPanelLifecycle::Degraded => {
                ToolStatus::Warning
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
        Some(wrap_card_rail(lines, self.status))
    }
}

/// Render the inline annotation for a tool cell whose full output was
/// retained internally and replaced by a bounded preview. The annotation
/// stays calm and path-free: it only says the output was shortened and that
/// the details shortcut opens the full retained output.
fn render_spillover_annotation(width: u16) -> Line<'static> {
    // Matches the model-facing preview footer (truncate.rs) and the existing
    // "Alt+V opens …" hint style (#3256): one quiet line, no handles or paths.
    let affordance = format!(
        "Output shortened — {}",
        crate::tui::key_shortcuts::tool_details_shortcut_action_hint("output")
    );
    Line::from(Span::styled(
        truncate_text(&affordance, usize::from(width).max(8)),
        Style::default().fg(palette::TEXT_MUTED).italic(),
    ))
}

/// Detect a truncated-output preview: the current model-facing footer (which
/// names the artifact path and recovery instruction), the previous plain
/// footer, or the legacy receipt header still present in older saved
/// sessions. Live cards collapse to the expand affordance for all of them.
fn is_truncated_output_preview(content: &str) -> bool {
    content.contains(crate::tools::truncate::SPILLOVER_RECOVERY_HINT)
        || content.contains(crate::tools::truncate::SPILLOVER_PREVIEW_HINT)
        || content.trim_start().starts_with("[Exact evidence retained")
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
                &crate::tui::key_shortcuts::tool_details_shortcut_action_hint("command"),
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
///
/// The rail is the card's border, so it carries the cell's own state: it is
/// painted with [`tool_rail_color`], OMP's border rule. A running card is
/// bordered in the action accent, a failed one in the error colour, and a
/// settled one recedes to muted — which is what makes a tool cell legible
/// without a neighbouring row narrating what the card already shows. The
/// header glyph inside it follows [`tool_glyph_color`] instead, so a finished
/// card still says what it was.
fn wrap_card_rail(mut lines: Vec<Line<'static>>, status: ToolStatus) -> Vec<Line<'static>> {
    let n = lines.len();
    if n == 0 {
        return lines;
    }
    let rail_style = Style::default().fg(tool_rail_color(status));
    if n == 1 {
        lines[0].spans.insert(0, Span::styled("─ ", rail_style));
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
        line.spans.insert(0, Span::styled(rail, rail_style));
    }
    lines
}

/// The legacy tool renderers accept only a low-motion boolean, which gives
/// both Reduced and Still a mix of legacy static frames. Preserve that stable
/// rendering path, then apply the central mode's exact fallback at the typed
/// header span (never by rewriting user/tool output text).
fn apply_static_tool_markers(lines: &mut [Line<'static>], marker: &'static str) {
    for line in lines {
        let mut index = 0;
        if line
            .spans
            .first()
            .is_some_and(|span| matches!(span.content.as_ref(), "─ " | "╭ " | "│ " | "╰ "))
        {
            index = 1;
        }
        let Some(status) = line.spans.get(index).map(|span| span.content.as_ref()) else {
            continue;
        };
        let Some(family) = line.spans.get(index + 1).map(|span| span.content.as_ref()) else {
            continue;
        };
        if !status.ends_with(' ')
            || !is_tool_status_glyph(status.trim_end())
            || !family.ends_with(' ')
            || !is_tool_family_glyph(family.trim_end())
        {
            continue;
        }
        let mut chars = status.trim_end().chars();
        if matches!(chars.next(), Some('\u{2800}'..='\u{28FF}')) && chars.next().is_none() {
            line.spans[index].content = format!("{marker} ").into();
        }
    }
}

/// Return the width of tool-cell chrome that remains after the transcript
/// cache removes the cell-local card rail. Tool headers have two additional
/// visual tokens (`✓`/spinner and the family glyph); detail rows have the
/// thin transcript rail. Keeping this width with the rendered line avoids
/// making selection copy infer chrome from glyph ranges, which can consume
/// real code or CJK text that happens to begin with a box-drawing character.
fn tool_copy_prefix_width(line: &Line<'static>) -> usize {
    let spans = line.spans.as_slice();
    let mut index = 0;

    // The cache removes these exact local card rails before flattening.
    if spans
        .first()
        .is_some_and(|span| matches!(span.content.as_ref(), "─ " | "╭ " | "│ " | "╰ "))
    {
        index = 1;
    }

    // Detail rows and pager affordances use the transcript rail as their
    // first span. The live transcript's general rail accounting removes it;
    // do not report it again as cell-local copy chrome.
    if spans
        .get(index)
        .is_some_and(|span| span.content.as_ref() == TRANSCRIPT_RAIL)
    {
        return 0;
    }

    // A tool header starts with `<status> <family> `. Only consume this
    // pair when both tokens are present, so output beginning with `✓` or a
    // braille character remains copyable content.
    let Some(status) = spans.get(index).map(|span| span.content.as_ref()) else {
        return 0;
    };
    let Some(family) = spans.get(index + 1).map(|span| span.content.as_ref()) else {
        return 0;
    };
    if !status.ends_with(' ')
        || !is_tool_status_glyph(status.trim_end())
        || !family.ends_with(' ')
        || !is_tool_family_glyph(family.trim_end())
    {
        return 0;
    }

    UnicodeWidthStr::width(status) + UnicodeWidthStr::width(family)
}

fn is_tool_status_glyph(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(ch) = chars.next() else {
        return false;
    };
    chars.next().is_none()
        && matches!(
            ch,
            '\u{2713}' // ✓
                | '\u{2715}' // ✕
                | '\u{00B7}' // ·
                | '!' // terminal warning
                | '\u{203A}' // › static still-mode marker
                | '\u{2800}'..='\u{28FF}' // braille spinner frames
        )
}

fn is_tool_family_glyph(text: &str) -> bool {
    use crate::tui::widgets::tool_card::{ToolFamily, family_glyph};

    [
        ToolFamily::Read,
        ToolFamily::Patch,
        ToolFamily::Run,
        ToolFamily::Find,
        ToolFamily::Delegate,
        ToolFamily::Fanout,
        ToolFamily::Rlm,
        ToolFamily::Verify,
        ToolFamily::Think,
        ToolFamily::Generic,
    ]
    .into_iter()
    .any(|family| family_glyph(family) == text)
}

fn review_severity_color(severity: &str) -> Color {
    match severity {
        "error" => palette::STATUS_ERROR,
        "warning" => palette::STATUS_WARNING,
        _ => palette::WHALE_ACTION,
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
        .fg(palette::WHALE_ACTION)
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

fn status_symbol(
    started_at: Option<Instant>,
    status: ToolStatus,
    low_motion: bool,
    family: crate::tui::widgets::tool_card::ToolFamily,
) -> String {
    match status {
        ToolStatus::Running if family == crate::tui::widgets::tool_card::ToolFamily::Verify => {
            crate::tui::spinner::verification_tick_frame(started_at, low_motion).to_string()
        }
        ToolStatus::Running => {
            crate::tui::spinner::braille_spinner_frame(started_at, low_motion).to_string()
        }
        ToolStatus::Success | ToolStatus::Hydrated => TOOL_DONE_SYMBOL.to_string(),
        ToolStatus::Warning => "!".to_string(),
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

/// Render an engine error without markdown interpretation. The live transcript
/// always advertises the dedicated full-error pager: terminal height, scroll
/// position, and adjacent tool cards can otherwise make a multiline recovery
/// instruction look like a clipped one-line failure. Transcript/pager mode
/// omits the recursive affordance while preserving every character.
fn render_error_message(
    message: &str,
    severity: crate::error_taxonomy::ErrorSeverity,
    width: u16,
    show_full_error_affordance: bool,
) -> Vec<Line<'static>> {
    // Error messages are machine-generated and should not be run through
    // markdown rendering, which would mangle env-var names containing
    // underscores (e.g. CODEWHALE_ALLOW_INSECURE_HTTP would lose them as
    // italic markers).
    let label = error_label_text(severity);
    let label_style = error_label_style(severity);
    let body_style = error_body_style(severity);
    let prefix_width = UnicodeWidthStr::width(label);
    let content_width = width.saturating_sub(2 + prefix_width as u16).max(1);
    let mut lines = wrap_plain_line(message, body_style, content_width);
    if let Some(first) = lines.get_mut(0) {
        first.spans.insert(0, Span::raw(" "));
        first.spans.insert(0, Span::styled(label, label_style));
    }
    let rail = format!("{}{}", '\u{258F}', " ".repeat(prefix_width));
    let rail_style = Style::default().fg(palette::TEXT_DIM);
    for line in lines.iter_mut().skip(1) {
        line.spans.insert(0, Span::styled(rail.clone(), rail_style));
    }
    if show_full_error_affordance {
        lines.push(details_affordance_line(
            &crate::tui::key_shortcuts::tool_details_shortcut_action_hint("full error"),
            Style::default().fg(palette::TEXT_MUTED).italic(),
        ));
    }
    lines
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

    let glyph_style = Style::default().fg(tool_glyph_color(status, family));
    let mut spans = vec![
        Span::styled(
            format!("{} ", status_symbol(started_at, status, low_motion, family)),
            glyph_style,
        ),
        Span::styled(format!("{glyph} "), glyph_style),
        Span::styled(verb.to_string(), tool_title_style()),
        Span::styled(" ", Style::default()),
        Span::styled(state_owned, tool_status_style(status, family)),
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

pub(crate) fn stale_shell_status_label(elapsed_since_output_ms: u64) -> String {
    format!(
        "running · stale · no output {}",
        crate::elapsed::format_elapsed_ms(elapsed_since_output_ms)
    )
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

/// `render_card_detail_line` for a row whose text carries its own SGR
/// colours: every segment's style is patched over `value_style`, so the
/// tool's colour wins where it set one and the cell's own ink (dim, state,
/// file:line emphasis) shows through where it did not. `text` is the row's
/// plain text (what the segments concatenate to); wrapping follows the same
/// boundaries as the plain path.
fn render_card_detail_line_styled(
    label: Option<&str>,
    text: &str,
    segments: &[tool_output::StyledSegment],
    value_style: Style,
    width: u16,
) -> Vec<Line<'static>> {
    let label_text = label.map(|text| format!("{text}:"));
    let prefix_width = UnicodeWidthStr::width(TRANSCRIPT_RAIL)
        + label_text.as_deref().map_or(0, UnicodeWidthStr::width)
        + usize::from(label.is_some());
    let content_width = usize::from(width).saturating_sub(prefix_width).max(1);

    let parts = wrap_text(text, content_width);
    let split = tool_output::split_segments(segments, &parts);

    let mut lines = Vec::new();
    for (idx, part) in split.into_iter().enumerate() {
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
        for (text, style) in part {
            spans.push(Span::styled(text, value_style.patch(style)));
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// `render_card_detail_line_single` for a coloured row: one line, never
/// wrapped, so an intact path or URL keeps the same hitbox with or without
/// colour.
fn render_card_detail_line_single_styled(
    label: Option<&str>,
    segments: &[tool_output::StyledSegment],
    value_style: Style,
) -> Line<'static> {
    let mut line = render_card_detail_line_single(label, "", value_style);
    line.spans.pop();
    for (text, style) in segments {
        line.spans
            .push(Span::styled(text.clone(), value_style.patch(*style)));
    }
    line
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

fn tool_status_style(
    status: ToolStatus,
    family: crate::tui::widgets::tool_card::ToolFamily,
) -> Style {
    active_theme().tool_status_style(status, family)
}

fn tool_detail_label_style() -> Style {
    active_theme().tool_label_style()
}

/// Card border ink — OMP's border rule. See [`Theme::tool_rail_color`].
fn tool_rail_color(status: ToolStatus) -> Color {
    active_theme().tool_rail_color(status)
}

/// Status-glyph ink — the mockup's reading. See [`Theme::tool_glyph_color`].
fn tool_glyph_color(
    status: ToolStatus,
    family: crate::tui::widgets::tool_card::ToolFamily,
) -> Color {
    active_theme().tool_glyph_color(status, family)
}

fn tool_status_label(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Running => "running",
        ToolStatus::Success => "done",
        ToolStatus::Hydrated => "tool loaded - retry required",
        ToolStatus::Warning => "issue",
        ToolStatus::Failed => "issue",
    }
}

/// A finished read/find card can truthfully name how many rendered lines came
/// back. Generic command output does not preserve typed stdout/stderr streams,
/// so Run receipts stay at localized `done` instead of inventing per-stream
/// counts from display text.
pub(crate) fn tool_receipt_label(
    family: crate::tui::widgets::tool_card::ToolFamily,
    status: ToolStatus,
    output: Option<&str>,
    locale: Locale,
) -> Cow<'static, str> {
    if status != ToolStatus::Success {
        return Cow::Borrowed(tool_status_label(status));
    }
    use crate::tui::widgets::tool_card::ToolFamily;
    match family {
        ToolFamily::Read | ToolFamily::Find => {
            let lines = output.map(count_output_lines).unwrap_or(0);
            if lines == 0 {
                crate::localization::tr(locale, crate::localization::MessageId::ToolReceiptDone)
            } else if lines == 1 {
                crate::localization::tr(
                    locale,
                    crate::localization::MessageId::ToolReceiptLinesSingular,
                )
            } else {
                Cow::Owned(
                    crate::localization::tr(
                        locale,
                        crate::localization::MessageId::ToolReceiptLinesPlural,
                    )
                    .replace("{count}", &lines.to_string()),
                )
            }
        }
        ToolFamily::Run => {
            crate::localization::tr(locale, crate::localization::MessageId::ToolReceiptDone)
        }
        _ => crate::localization::tr(locale, crate::localization::MessageId::ToolReceiptDone),
    }
}

fn count_output_lines(output: &str) -> usize {
    if output.is_empty() {
        0
    } else {
        output.lines().count()
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

/// Aggregated file activity for compact Work panel display (#4636).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileActivitySummary {
    pub files_read: u32,
    pub dirs_listed: u32,
    pub patterns_searched: u32,
    pub files_written: u32,
}

impl FileActivitySummary {
    pub fn is_empty(&self) -> bool {
        self.files_read == 0
            && self.dirs_listed == 0
            && self.patterns_searched == 0
            && self.files_written == 0
    }

    pub fn compact_display(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if self.files_read > 0 {
            parts.push(format!("Read {} files", self.files_read));
        }
        if self.dirs_listed > 0 {
            parts.push(format!("Listed {} directories", self.dirs_listed));
        }
        if self.patterns_searched > 0 {
            parts.push(format!("Searched {} patterns", self.patterns_searched));
        }
        if self.files_written > 0 {
            parts.push(format!("Wrote {} files", self.files_written));
        }
        parts
    }

    pub fn from_tool_name(name: &str) -> Option<FileActivityKind> {
        match name {
            "read_file" | "Read" | "read" => Some(FileActivityKind::Read),
            "list_dir" | "list_directory" | "Glob" | "glob" => Some(FileActivityKind::List),
            "search" | "grep" | "Grep" | "grep_files" | "file_search" | "codebase_search" => {
                Some(FileActivityKind::Search)
            }
            "write_file" | "Write" | "apply_patch" | "Edit" | "edit_file" | "fim_edit" => {
                Some(FileActivityKind::Write)
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileActivityKind {
    Read,
    List,
    Search,
    Write,
}

impl FileActivitySummary {
    pub fn record(&mut self, kind: FileActivityKind) {
        match kind {
            FileActivityKind::Read => self.files_read += 1,
            FileActivityKind::List => self.dirs_listed += 1,
            FileActivityKind::Search => self.patterns_searched += 1,
            FileActivityKind::Write => self.files_written += 1,
        }
    }
}

/// Illuminate the newest graphemes of an actively streaming assistant line.
fn apply_hot_tail_to_last_line(lines: &mut [Line<'static>], low_motion: bool) {
    if let Some(last) = lines.last_mut() {
        apply_hot_tail_to_line(last, low_motion);
    }
}

pub(crate) fn apply_hot_tail_to_line(line: &mut Line<'static>, low_motion: bool) {
    if line.spans.is_empty() {
        return;
    }
    // Reconstruct plain text from spans, split hot tail, re-style.
    let plain: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    if plain.trim().is_empty() {
        return;
    }
    let (_settled, hot) = crate::tui::hot_tail::split_hot_tail(
        &plain,
        true,
        crate::tui::hot_tail::HOT_TAIL_GRAPHEMES,
    );
    if hot.is_empty() {
        return;
    }
    let hot_start = plain.len().saturating_sub(hot.len());
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let base_fg = palette::TEXT_PRIMARY;
    let hot_style = crate::tui::hot_tail::hot_tail_style(base_fg, elapsed, low_motion);

    // Walk spans and re-style the trailing hot graphemes.
    let mut cursor = 0usize;
    let mut new_spans = Vec::with_capacity(line.spans.len() + 2);
    for span in line.spans.drain(..) {
        let content = span.content.to_string();
        let len = content.len();
        let span_end = cursor + len;
        if span_end <= hot_start {
            new_spans.push(span);
        } else if cursor >= hot_start {
            new_spans.push(Span::styled(content, hot_style));
        } else {
            // Split this span across the boundary.
            let local = hot_start - cursor;
            let (left, right) = content.split_at(local.min(content.len()));
            if !left.is_empty() {
                new_spans.push(Span::styled(left.to_string(), span.style));
            }
            if !right.is_empty() {
                new_spans.push(Span::styled(right.to_string(), hot_style));
            }
        }
        cursor = span_end;
    }
    line.spans = new_spans;
}

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// Tideline receipt stream (spec §5a "Receipt stream", §1 work screen): turn
// rows, the pod-formation `├──/└──` tree, state-marked receipt rows with
// timestamps and receipt counts, an indented conclusion block, and the
// legend row that teaches the marks in place. Translation scaffolding in
// the topbar mold: pure, deterministic, injected events — the transcript
// click path (`work_surface` row rects) is reused at the landing slice;
// not wired into `ui/frame.rs` (#5698 gate).

pub use tideline_stream::{TidelineStream, render_tideline_stream};

/// Full export alias for the Tideline components that compose the stream
/// into their stages (work surface, settings preview).
pub(crate) mod tideline_exports {
    #![allow(unused_imports)] // consumed by the work-surface/settings test suites and landing slice

    pub use super::tideline_stream::{
        TidelineReceiptState, TidelineStream, TidelineStreamEvent, render_tideline_stream,
        tideline_stream_hitboxes,
    };
}

mod tideline_stream;

#[cfg(test)]
mod tideline_stream_tests;
