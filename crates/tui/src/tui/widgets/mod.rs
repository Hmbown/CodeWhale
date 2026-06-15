mod footer;
mod header;
// Some helpers (`shift`, `ctrl_alt`, `is_press`, etc.) are part of the
// public surface for issue #93's help overlay and future call sites; allow
// dead code rather than scattering `#[allow]` across every constructor.
#[allow(dead_code)]
pub mod key_hint;
// Phase 1 of #85: widget lands without a wire-up site so reviewers can
// evaluate the rendering in isolation. The follow-up PR plumbs it through
// the composer area in `ui.rs`. `pub mod` (vs the usual `pub use` pattern)
// keeps the unused-imports lint quiet until then.
pub mod agent_card;
pub mod decision_card;
pub mod pending_input_preview;
mod renderable;
pub mod tool_card;

pub use footer::{
    FooterProps, FooterToast, FooterWidget, footer_agents_chip, footer_shell_chip,
    footer_working_label,
};
pub use header::{HeaderData, HeaderWidget, header_status_indicator_frame};
pub use renderable::Renderable;

use std::collections::HashSet;
use std::time::Duration;

use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::tui::app::{App, AppMode, ComposerDensity, VimMode};
use crate::tui::approval::{
    ApprovalRequest, ApprovalView, ElevationOption, ElevationRequest, RiskLevel, ToolCategory,
};
use crate::tui::history::{GenericToolCell, HistoryCell, ToolCell, ToolRun, ToolStatus};
use crate::tui::scrolling::TranscriptLineMeta;
use crate::{
    commands,
    config::{ApiProvider, model_completion_names_for_provider},
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, StatefulWidget, Widget, Wrap,
    },
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const SEND_FLASH_DURATION: Duration = Duration::from_millis(500);
const COMPOSER_PANEL_HEIGHT: u16 = 2;
const JUMP_TO_LATEST_BUTTON_WIDTH: u16 = 3;
const JUMP_TO_LATEST_BUTTON_HEIGHT: u16 = 3;

pub struct ChatWidget {
    content_area: Rect,
    lines: Vec<Line<'static>>,
    scrollbar: Option<TranscriptScrollbar>,
    jump_to_latest_button: Option<Rect>,
    background: Color,
    scroll_track: Color,
    scroll_thumb: Color,
    jump_border: Color,
    jump_arrow: Color,
}

#[derive(Debug, Clone, Copy)]
struct TranscriptScrollbar {
    top: usize,
    visible: usize,
    total: usize,
}

impl ChatWidget {
    pub fn new(app: &mut App, area: Rect) -> Self {
        let content_area = area;
        let background = app.ui_theme.surface_bg;
        let scroll_track = app.ui_theme.border;
        let scroll_thumb = app.ui_theme.status_working;
        let jump_border = app.ui_theme.border;
        let jump_arrow = app.ui_theme.status_working;
        let visible_lines = content_area.height as usize;
        let render_options = app.transcript_render_options();

        if should_render_empty_state(app) {
            let lines = build_empty_state_lines(app, content_area);
            app.viewport.last_transcript_area = Some(content_area);
            app.viewport.last_transcript_top = 0;
            app.viewport.last_transcript_visible = visible_lines;
            app.viewport.last_transcript_total = 0;
            app.viewport.last_transcript_padding_top = 0;
            app.viewport.jump_to_latest_button_area = None;
            return Self {
                content_area,
                lines,
                scrollbar: None,
                jump_to_latest_button: None,
                background,
                scroll_track,
                scroll_thumb,
                jump_border,
                jump_arrow,
            };
        }

        // Per-cell revision caching (fix for issue #78):
        //
        // Every committed history cell carries its own revision counter in
        // `app.history_revisions`. The transcript cache compares each cell's
        // current revision against the previously rendered one, so unchanged
        // cells reuse their cached wrapped lines instead of being re-wrapped
        // every frame. This is the difference between O(history.len()) and
        // O(changed_cells) per render — and was the root cause of scroll lag
        // on long transcripts.
        //
        // The active in-flight cell (if any) is appended as the last cell so
        // its mutations show up at the live tail. Each entry inside the
        // active cell becomes a virtual cell at index `history.len() + i`,
        // matching `App::cell_at_virtual_index`. Active-cell entries share
        // the same `active_cell_revision` salt so any mutation in the active
        // cell forces only those rows to re-render — committed history rows
        // are unaffected.
        app.resync_history_revisions();
        let active_entries: &[HistoryCell] = app
            .active_cell
            .as_ref()
            .map_or(&[], |active| active.entries());

        let history_len = app.history.len();
        let tool_runs = if app.tool_collapse_active() {
            crate::tui::history::detect_tool_runs_from_slices(
                &app.history,
                active_entries,
                app.tool_collapse_threshold,
            )
        } else {
            Vec::new()
        };
        let collapsed_run_starts: HashSet<usize> = tool_runs
            .iter()
            .filter_map(|run| (!app.expanded_tool_runs.contains(&run.start)).then_some(run.start))
            .collect();
        let mut collapsed_tool_indices: HashSet<usize> = HashSet::new();
        for run in &tool_runs {
            if !collapsed_run_starts.contains(&run.start) {
                continue;
            }
            for offset in 1..run.count {
                collapsed_tool_indices.insert(run.start + offset);
            }
        }
        let has_collapsed = !app.collapsed_cells.is_empty() || !collapsed_run_starts.is_empty();

        // Fast path: no collapsed cells — use original slices directly.
        if !has_collapsed {
            let mut cell_revisions: Vec<u64> =
                Vec::with_capacity(app.history.len() + active_entries.len());
            cell_revisions.extend_from_slice(&app.history_revisions);
            if !active_entries.is_empty() {
                let active_rev = app.active_cell_revision;
                for i in 0..active_entries.len() {
                    let salt = (i as u64).wrapping_add(1);
                    cell_revisions.push(
                        active_rev
                            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                            .wrapping_add(salt),
                    );
                }
            }
            // Build identity mapping: filtered index == original index.
            app.collapsed_cell_map = (0..app.history.len() + active_entries.len()).collect();

            let shards: [&[HistoryCell]; 2] = [&app.history, active_entries];
            app.viewport.transcript_cache.ensure_split(
                &shards,
                &cell_revisions,
                content_area.width.max(1),
                render_options,
                &app.folded_thinking,
                None,
            );
        } else {
            // Slow path: clone non-collapsed cells into filtered vecs so
            // collapsed cells are excluded from rendering. Build the
            // filtered→original index mapping.
            let mut filtered_cells: Vec<HistoryCell> =
                Vec::with_capacity(history_len + active_entries.len());
            let mut filtered_revs: Vec<u64> =
                Vec::with_capacity(history_len + active_entries.len());
            let mut filtered_to_original: Vec<usize> =
                Vec::with_capacity(history_len + active_entries.len());

            for (idx, cell) in app.history.iter().enumerate() {
                if app.collapsed_cells.contains(&idx) {
                    continue;
                }
                if collapsed_tool_indices.contains(&idx) {
                    continue;
                }
                if let Some(run) = tool_runs
                    .iter()
                    .find(|run| run.start == idx && collapsed_run_starts.contains(&idx))
                {
                    filtered_cells.push(tool_run_summary_cell(run));
                    filtered_revs.push(tool_run_summary_revision(
                        run,
                        &app.history_revisions,
                        history_len,
                        app.active_cell_revision,
                    ));
                    filtered_to_original.push(idx);
                    continue;
                }
                filtered_cells.push(cell.clone());
                filtered_revs.push(app.history_revisions[idx]);
                filtered_to_original.push(idx);
            }

            if !active_entries.is_empty() {
                let active_rev = app.active_cell_revision;
                for (i, cell) in active_entries.iter().enumerate() {
                    let original_idx = history_len + i;
                    if app.collapsed_cells.contains(&original_idx) {
                        continue;
                    }
                    if collapsed_tool_indices.contains(&original_idx) {
                        continue;
                    }
                    if let Some(run) = tool_runs.iter().find(|run| {
                        run.start == original_idx && collapsed_run_starts.contains(&original_idx)
                    }) {
                        filtered_cells.push(tool_run_summary_cell(run));
                        filtered_revs.push(tool_run_summary_revision(
                            run,
                            &app.history_revisions,
                            history_len,
                            active_rev,
                        ));
                        filtered_to_original.push(original_idx);
                        continue;
                    }
                    filtered_cells.push(cell.clone());
                    let salt = (i as u64).wrapping_add(1);
                    filtered_revs.push(active_entry_revision(active_rev, salt));
                    filtered_to_original.push(original_idx);
                }
            }

            app.collapsed_cell_map = filtered_to_original;

            let shards: [&[HistoryCell]; 1] = [&filtered_cells];
            app.viewport.transcript_cache.ensure_split(
                &shards,
                &filtered_revs,
                content_area.width.max(1),
                render_options,
                &app.folded_thinking,
                Some(&app.collapsed_cell_map),
            );
        }

        let total_lines = app.viewport.transcript_cache.total_lines();

        let line_meta = app.viewport.transcript_cache.line_meta();

        if app.viewport.pending_scroll_delta != 0 {
            app.viewport.transcript_scroll = app.viewport.transcript_scroll.scrolled_by(
                app.viewport.pending_scroll_delta,
                line_meta,
                visible_lines,
            );
            app.viewport.pending_scroll_delta = 0;
        }

        let max_start = total_lines.saturating_sub(visible_lines);
        // v0.8.11 hotfix: snapshot whether the user's prior scroll state
        // was *deliberately* tail BEFORE we resolve. `resolve_top` clamps
        // out-of-range `at_line(N)` to `to_bottom()` (e.g. when content
        // shrunk so `max_start < N`), and `scrolled_by` returns
        // `to_bottom()` when the whole transcript fits in one screen
        // even if the user just scrolled up. Either case would fool a
        // post-resolve `is_at_tail()` check into thinking the user is
        // tracking the tail and silently revoke `user_scrolled_during_
        // stream` — the next stream chunk would then yank them back to
        // bottom mid-read.
        let was_explicit_tail = app.viewport.transcript_scroll.is_at_tail();
        let (scroll_state, top) = app
            .viewport
            .transcript_scroll
            .resolve_top(line_meta, max_start);
        app.viewport.transcript_scroll = scroll_state;
        // If the user scrolled back to the live tail, the per-stream
        // "leave me alone" lock is over — new chunks should pin to bottom
        // again until they explicitly scroll up. Without this clear, content
        // piles up off-screen below the visible area and the view appears
        // frozen at the moment they returned to bottom.
        //
        // Only clear the lock when the user's INTENT was tail (their
        // stored state was already `to_bottom()` before resolve), AND
        // when the transcript actually has scrolling room to talk about
        // — if everything fits in one screen, "tail" is trivially true
        // and clearing here would yank the user back to bottom on the
        // next chunk even though they explicitly scrolled up.
        if was_explicit_tail && total_lines > visible_lines {
            app.user_scrolled_during_stream = false;
        }

        app.viewport.last_transcript_area = Some(content_area);
        app.viewport.last_transcript_top = top;
        app.viewport.last_transcript_visible = visible_lines;
        app.viewport.last_transcript_total = total_lines;
        app.viewport.last_transcript_padding_top = 0;
        let detail_target_cell = (!app.viewport.transcript_selection.is_active())
            .then(|| app.detail_cell_index_for_viewport(top, visible_lines, line_meta))
            .flatten();

        let end = (top + visible_lines).min(total_lines);
        let mut lines = if total_lines == 0 {
            vec![Line::from("")]
        } else {
            app.viewport.transcript_cache.lines()[top..end].to_vec()
        };

        // Brief flash highlight on the most recently sent user message.
        if !app.low_motion
            && let Some(send_at) = app.last_send_at
        {
            if send_at.elapsed() < SEND_FLASH_DURATION {
                apply_send_flash(
                    &mut lines,
                    top,
                    &app.history,
                    line_meta,
                    &app.collapsed_cell_map,
                );
            } else {
                app.last_send_at = None;
            }
        }

        if let Some(target_cell) = detail_target_cell {
            apply_detail_target_highlight(
                &mut lines,
                top,
                target_cell,
                line_meta,
                &app.collapsed_cell_map,
            );
        }

        apply_selection(&mut lines, top, app);

        if app.viewport.transcript_scroll.is_at_tail() {
            app.viewport.last_transcript_padding_top = visible_lines.saturating_sub(lines.len());
            pad_lines_to_bottom(&mut lines, visible_lines);
        }

        let scrollbar = (total_lines > visible_lines && content_area.width > 1).then_some(
            TranscriptScrollbar {
                top,
                visible: visible_lines,
                total: total_lines,
            },
        );
        let jump_to_latest_button =
            if app.use_mouse_capture && !app.viewport.transcript_scroll.is_at_tail() {
                jump_to_latest_button_rect(content_area, scrollbar.is_some())
            } else {
                None
            };
        app.viewport.jump_to_latest_button_area = jump_to_latest_button;

        Self {
            content_area,
            lines,
            scrollbar,
            jump_to_latest_button,
            background,
            scroll_track,
            scroll_thumb,
            jump_border,
            jump_arrow,
        }
    }
}

fn tool_run_summary_cell(run: &ToolRun) -> HistoryCell {
    HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
        name: "activity_group".to_string(),
        status: ToolStatus::Success,
        input_summary: Some(crate::tui::history::tool_run_summary(run)),
        output: None,
        prompts: None,
        spillover_path: None,
        output_summary: None,
        is_diff: false,
    }))
}

fn tool_run_summary_revision(
    run: &ToolRun,
    revisions: &[u64],
    history_len: usize,
    active_rev: u64,
) -> u64 {
    let mut revision = 0xA11C_EA5E_D00D_2692u64 ^ ((run.start as u64) << 32) ^ (run.count as u64);
    for idx in run.start..run.start.saturating_add(run.count) {
        let cell_revision = revisions.get(idx).copied().unwrap_or_else(|| {
            let active_idx = idx.saturating_sub(history_len);
            active_entry_revision(active_rev, (active_idx as u64).wrapping_add(1))
        });
        revision = revision.rotate_left(7) ^ cell_revision;
    }
    revision
}

fn active_entry_revision(active_rev: u64, salt: u64) -> u64 {
    active_rev
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(salt)
}

impl Renderable for ChatWidget {
    fn render(&self, _area: Rect, buf: &mut Buffer) {
        // Use the passed render area, not self.content_area — those can
        // drift when layout changes (e.g. file-tree pane toggle), and
        // using the stale self.content_area is the root cause of text
        // bleed-through (#400). In debug builds, assert the two match to
        // catch future drift early.
        debug_assert_eq!(
            _area, self.content_area,
            "ChatWidget content_area drifted from render area: \
             content_area={:?} render_area={:?}",
            self.content_area, _area
        );

        let area = _area;

        // Repaint the full chat area with the codewhale-ink background each
        // frame. Ratatui's `Paragraph` only writes cells that contain text,
        // so cells the current frame's paragraph doesn't touch would
        // otherwise hold the *previous* frame's contents (the `:24Z`
        // timestamp-tail bleed-through reported in v0.8.5 testing). Using
        // `Clear` reset cells to terminal default, which read as a brown-
        // gray on most user setups; an explicit ink fill keeps the chat
        // area on-brand.
        Block::default()
            .style(Style::default().bg(self.background))
            .render(area, buf);

        let paragraph =
            Paragraph::new(self.lines.clone()).style(Style::default().bg(self.background));
        paragraph.render(area, buf);

        if let Some(scrollbar) = self.scrollbar {
            let scrollable_range = scrollbar.total.saturating_sub(scrollbar.visible);
            let mut state = ScrollbarState::new(scrollable_range)
                .position(scrollbar.top.min(scrollable_range))
                .viewport_content_length(scrollbar.visible);
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .track_style(Style::default().fg(self.scroll_track))
                .thumb_symbol("┃")
                .thumb_style(Style::default().fg(self.scroll_thumb))
                .render(area, buf, &mut state);
        }

        if let Some(button_area) = self.jump_to_latest_button {
            render_jump_to_latest_button(
                button_area,
                buf,
                self.background,
                self.jump_border,
                self.jump_arrow,
            );
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

fn jump_to_latest_button_rect(area: Rect, has_scrollbar: bool) -> Option<Rect> {
    if area.width < JUMP_TO_LATEST_BUTTON_WIDTH + u16::from(has_scrollbar)
        || area.height < JUMP_TO_LATEST_BUTTON_HEIGHT
    {
        return None;
    }

    let scrollbar_gutter = u16::from(has_scrollbar);
    Some(Rect {
        x: area
            .x
            .saturating_add(area.width)
            .saturating_sub(scrollbar_gutter)
            .saturating_sub(JUMP_TO_LATEST_BUTTON_WIDTH),
        y: area
            .y
            .saturating_add(area.height)
            .saturating_sub(JUMP_TO_LATEST_BUTTON_HEIGHT),
        width: JUMP_TO_LATEST_BUTTON_WIDTH,
        height: JUMP_TO_LATEST_BUTTON_HEIGHT,
    })
}

fn render_jump_to_latest_button(
    area: Rect,
    buf: &mut Buffer,
    background: Color,
    border: Color,
    arrow: Color,
) {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(background))
        .render(area, buf);

    let arrow_x = area.x.saturating_add(1);
    let arrow_y = area.y.saturating_add(1);
    buf[(arrow_x, arrow_y)]
        .set_symbol("↓")
        .set_style(Style::default().fg(arrow).add_modifier(Modifier::BOLD));
}

pub struct ComposerWidget<'a> {
    app: &'a App,
    max_height: u16,
    slash_menu_entries: &'a [SlashMenuEntry],
    mention_menu_entries: &'a [String],
}

impl<'a> ComposerWidget<'a> {
    pub fn new(
        app: &'a App,
        max_height: u16,
        slash_menu_entries: &'a [SlashMenuEntry],
        mention_menu_entries: &'a [String],
    ) -> Self {
        Self {
            app,
            max_height,
            slash_menu_entries,
            mention_menu_entries,
        }
    }

    /// Number of popup rows below the input. Mention and slash menus are
    /// mutually exclusive — the cursor can only sit inside an `@token` OR
    /// a `/cmd` token, not both at once. Mention takes precedence because
    /// the partial-mention check is positional and stricter than slash's
    /// "starts-with-/" check.
    fn active_menu_row_count(&self) -> usize {
        if self.app.is_history_search_active() {
            self.app.history_search_matches().len().max(1)
        } else if !self.mention_menu_entries.is_empty() {
            self.mention_menu_entries.len()
        } else {
            self.slash_menu_entries.len()
        }
    }

    /// Row reservation passed to `composer_height`. When the slash- or
    /// mention-menu is active we lock the composer to its worst-case
    /// envelope so the chat area above doesn't repaint every keystroke
    /// as the matched-entry count shrinks. Pure cosmetic: the menu
    /// itself still renders its actual entries — the extra rows are
    /// just panel padding inside the same Rect.
    ///
    /// Reported on Windows 10 PowerShell + WSL where the console
    /// backend's per-cell write cost makes the layout jitter visible
    /// even though the work is tiny on Unix terminals. See user
    /// feedback in v0.8.8 polish thread.
    pub fn active_menu_reserved_rows(&self) -> usize {
        let actual = self.active_menu_row_count();
        if actual == 0 {
            return 0;
        }
        if self.app.is_history_search_active() {
            return actual;
        }
        // Slash- and mention-menu are the cases that grow/shrink mid-typing.
        // Reserve the composer's panel-max so the layout stays stable
        // for the lifetime of the menu session.
        actual.max(usize::from(self.max_height_cap()))
    }

    fn has_panel(&self, area: Rect) -> bool {
        self.app.composer_border && area.height >= 3 && area.width >= 12
    }

    fn inner_area(&self, area: Rect) -> Rect {
        if self.has_panel(area) {
            Block::default().borders(Borders::ALL).inner(area)
        } else {
            area
        }
    }

    fn mode_color(&self) -> Color {
        match self.app.mode {
            AppMode::Agent => palette::MODE_AGENT,
            AppMode::Yolo => palette::MODE_YOLO,
            AppMode::Plan => palette::MODE_PLAN,
        }
    }

    fn max_height_cap(&self) -> u16 {
        composer_max_height(self.app.composer_density)
    }
}

impl Renderable for ComposerWidget<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let background = Style::default().bg(self.app.ui_theme.composer_bg);
        let has_panel = self.has_panel(area);
        let inner_area = self.inner_area(area);
        let input_text = self.app.composer_display_input();
        let input_cursor = self.app.composer_display_cursor();
        let history_search_matches = if self.app.is_history_search_active() {
            self.app.history_search_matches()
        } else {
            Vec::new()
        };
        let menu_lines = self.active_menu_row_count();
        // For the layout-budget calculation, treat the menu as if it were
        // already at its locked, worst-case height (see
        // `active_menu_reserved_rows`). Without this, when the matched-entry
        // count drops mid-typing, `top_padding` grows and the input visually
        // jumps down inside the panel even though the panel rect stayed put.
        let menu_lines_for_budget = self.active_menu_reserved_rows().max(menu_lines);
        let input_rows_budget =
            composer_input_rows_budget(inner_area.height, menu_lines_for_budget);
        let content_width = usize::from(inner_area.width.max(1));
        let (visible_lines, _cursor_row, _cursor_col, scroll_offset) =
            layout_input_with_scroll(input_text, input_cursor, content_width, input_rows_budget);
        let is_draft_mode = input_text.contains('\n') || visible_lines.len() > 1;
        if has_panel {
            let border_color = if input_text.trim().is_empty() {
                palette::BORDER_COLOR
            } else {
                self.mode_color()
            };
            let hint_line = if self.app.is_history_search_active() {
                Some(Line::from(vec![
                    Span::styled(
                        format!(
                            " {}  ",
                            self.app.tr(crate::localization::MessageId::HistoryHintMove)
                        ),
                        Style::default().fg(palette::TEXT_MUTED),
                    ),
                    Span::styled(
                        format!(
                            "{}  ",
                            self.app
                                .tr(crate::localization::MessageId::HistoryHintAccept)
                        ),
                        Style::default().fg(palette::TEXT_MUTED),
                    ),
                    Span::styled(
                        self.app
                            .tr(crate::localization::MessageId::HistoryHintRestore),
                        Style::default().fg(palette::TEXT_MUTED),
                    ),
                ]))
            } else if !self.slash_menu_entries.is_empty() {
                Some(Line::from(vec![
                    Span::styled(" Up/Down move  ", Style::default().fg(palette::TEXT_MUTED)),
                    Span::styled("Tab accept  ", Style::default().fg(palette::TEXT_MUTED)),
                    Span::styled("Esc close", Style::default().fg(palette::TEXT_MUTED)),
                ]))
            } else if !input_text.trim().is_empty() {
                // Live disambiguation for #345: when there's content in the
                // composer, show what `Enter` will do RIGHT NOW so the user
                // never has to guess between Immediate / Steer / QueueFollowUp /
                // Queue. The disposition flips with engine state so this hint
                // is the only reliable cue before pressing Enter.
                use crate::tui::app::SubmitDisposition;
                let queue_count = self.app.queued_message_count();
                let (label, color) = match self.app.decide_submit_disposition() {
                    SubmitDisposition::Immediate => {
                        if queue_count > 0 {
                            (
                                Some(format!("↵ send ({queue_count} queued)")),
                                palette::DEEPSEEK_SKY,
                            )
                        } else {
                            (None, palette::TEXT_MUTED)
                        }
                    }
                    SubmitDisposition::Queue => {
                        if self.app.offline_mode {
                            (Some("↵ offline queue".to_string()), palette::STATUS_WARNING)
                        } else {
                            let label = if queue_count > 0 {
                                format!("↵ queue ({} waiting)", queue_count.saturating_add(1))
                            } else {
                                "↵ queue for next turn".to_string()
                            };
                            (Some(label), palette::TEXT_MUTED)
                        }
                    }
                    // Steer and QueueFollowUp are now only reached via Ctrl+Enter override.
                    SubmitDisposition::Steer => (
                        Some("↵ steering (Ctrl+Enter)".to_string()),
                        palette::DEEPSEEK_SKY,
                    ),
                    SubmitDisposition::QueueFollowUp => (
                        Some("↵ queued (Ctrl+Enter to steer)".to_string()),
                        palette::TEXT_MUTED,
                    ),
                };
                label.map(|text| {
                    Line::from(vec![Span::styled(
                        format!(" {text} "),
                        Style::default().fg(color),
                    )])
                })
            } else {
                None
            };

            let mut block = Block::default()
                .title(Line::from(Span::styled(
                    if self.app.is_history_search_active() {
                        self.app
                            .tr(crate::localization::MessageId::HistorySearchTitle)
                    } else if is_draft_mode {
                        "Draft"
                    } else {
                        "Composer"
                    },
                    Style::default().fg(palette::TEXT_MUTED),
                )))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .style(background);
            // Top-right corner: editor state plus transient turn receipts.
            // Receipts are lifecycle chrome, not transcript content; they
            // should appear briefly without displacing conversation rows.
            if let Some(chrome) = composer_top_right_chrome(self.app, area.width) {
                block = block.title_top(chrome.right_aligned());
            }
            if let Some(hint_line) = hint_line {
                block = block.title_bottom(hint_line);
            }
            block.render(area, buf);
        } else {
            Block::default().style(background).render(area, buf);
        }

        let mut input_lines = Vec::new();
        if input_text.is_empty() {
            if let Some(ref suggestion) = self.app.prompt_suggestion
                && !self.app.is_history_search_active()
            {
                input_lines.push(Line::from(Span::styled(
                    suggestion.as_str(),
                    Style::default().fg(palette::TEXT_HINT),
                )));
            } else {
                let placeholder = if self.app.is_history_search_active() {
                    self.app
                        .tr(crate::localization::MessageId::HistorySearchPlaceholder)
                } else {
                    self.app
                        .tr(crate::localization::MessageId::ComposerPlaceholder)
                };
                input_lines.push(Line::from(Span::styled(
                    placeholder,
                    Style::default().fg(palette::TEXT_MUTED).italic(),
                )));
            }
        } else if let Some((sel_start, sel_end)) = self.app.selection_range() {
            let line_ranges: Vec<(usize, usize)> =
                wrap_input_lines_for_mouse(&self.app.input, content_width)
                    .into_iter()
                    .skip(scroll_offset)
                    .take(visible_lines.len())
                    .map(|(start, text)| (start, start + text.chars().count()))
                    .collect();
            for (line_text, (line_start, line_end)) in visible_lines.iter().zip(line_ranges.iter())
            {
                let spans = line_spans_with_selection(
                    line_text,
                    *line_start,
                    *line_end,
                    sel_start,
                    sel_end,
                    self.app.ui_theme.selection_bg,
                );
                input_lines.push(Line::from(spans));
            }
        } else {
            for line in &visible_lines {
                input_lines.push(Line::from(Span::styled(
                    line.clone(),
                    Style::default().fg(palette::TEXT_PRIMARY),
                )));
            }
        }

        // For non-empty input, input_lines.len() already reflects wrapping via
        // layout_input.  For the empty-input placeholder, Paragraph::wrap will
        // wrap the single Line at render time, so we must estimate the wrapped
        // row count ourselves to keep padding accurate on narrow widths.
        let visual_rows = if input_text.is_empty() {
            let placeholder: &str = if let Some(ref suggestion) = self.app.prompt_suggestion {
                suggestion.as_str()
            } else if self.app.is_history_search_active() {
                self.app
                    .tr(crate::localization::MessageId::HistorySearchPlaceholder)
            } else {
                self.app
                    .tr(crate::localization::MessageId::ComposerPlaceholder)
            };
            placeholder_visual_lines_for(placeholder, content_width)
        } else {
            input_lines.len()
        };
        let top_padding = composer_top_padding(visual_rows, input_rows_budget);
        let mut lines = Vec::new();
        for _ in 0..top_padding {
            lines.push(Line::from(""));
        }
        lines.extend(input_lines);

        if self.app.is_history_search_active() {
            if history_search_matches.is_empty() {
                lines.push(Line::from(Span::styled(
                    self.app
                        .tr(crate::localization::MessageId::HistoryNoMatches),
                    Style::default().fg(palette::TEXT_MUTED),
                )));
            } else {
                let selected = self
                    .app
                    .history_search_selected_index()
                    .min(history_search_matches.len().saturating_sub(1));
                let menu_visible_rows = inner_area
                    .height
                    .saturating_sub(visual_rows as u16)
                    .saturating_sub(top_padding as u16)
                    .saturating_sub(1)
                    .max(1) as usize;
                let menu_total = history_search_matches.len();
                let menu_top = if menu_total <= menu_visible_rows {
                    0
                } else {
                    let half = menu_visible_rows / 2;
                    if selected <= half {
                        0
                    } else if selected + half >= menu_total {
                        menu_total.saturating_sub(menu_visible_rows)
                    } else {
                        selected.saturating_sub(half)
                    }
                };
                let menu_bottom = (menu_top + menu_visible_rows).min(menu_total);

                for (idx, entry) in history_search_matches
                    .iter()
                    .enumerate()
                    .take(menu_bottom)
                    .skip(menu_top)
                {
                    let is_selected = idx == selected;
                    let style = if is_selected {
                        Style::default()
                            .fg(palette::SELECTION_TEXT)
                            .bg(palette::SELECTION_BG)
                    } else {
                        Style::default().fg(palette::TEXT_MUTED)
                    };
                    let marker = if is_selected { "▸" } else { " " };
                    lines.push(Line::from(vec![
                        Span::styled(" ", Style::default()),
                        Span::styled(marker, style),
                        Span::styled(" ", style),
                        Span::styled(entry.clone(), style),
                    ]));
                }
            }
        } else if !self.mention_menu_entries.is_empty() {
            let selected = self
                .app
                .mention_menu_selected
                .min(self.mention_menu_entries.len().saturating_sub(1));
            let menu_visible_rows = inner_area
                .height
                .saturating_sub(visual_rows as u16)
                .saturating_sub(top_padding as u16)
                .saturating_sub(1)
                .max(1) as usize;
            let menu_total = self.mention_menu_entries.len();
            let menu_top = if menu_total <= menu_visible_rows {
                0
            } else {
                let half = menu_visible_rows / 2;
                if selected <= half {
                    0
                } else if selected + half >= menu_total {
                    menu_total.saturating_sub(menu_visible_rows)
                } else {
                    selected.saturating_sub(half)
                }
            };
            let menu_bottom = (menu_top + menu_visible_rows).min(menu_total);

            for (idx, entry) in self
                .mention_menu_entries
                .iter()
                .enumerate()
                .take(menu_bottom)
                .skip(menu_top)
            {
                let is_selected = idx == selected;
                let style = if is_selected {
                    Style::default()
                        .fg(palette::SELECTION_TEXT)
                        .bg(palette::SELECTION_BG)
                } else {
                    Style::default().fg(palette::TEXT_MUTED)
                };
                let marker = if is_selected { "▸" } else { " " };
                lines.push(Line::from(vec![
                    Span::styled(" ", Style::default()),
                    Span::styled(marker, style),
                    Span::styled(" ", style),
                    Span::styled(format!("@{entry}"), style),
                ]));
            }
        } else if !self.slash_menu_entries.is_empty() {
            let selected = self
                .app
                .slash_menu_selected
                .min(self.slash_menu_entries.len().saturating_sub(1));
            let menu_visible_rows = inner_area
                .height
                .saturating_sub(visual_rows as u16)
                .saturating_sub(top_padding as u16)
                .saturating_sub(1)
                .max(1) as usize;
            let menu_total = self.slash_menu_entries.len();
            let menu_top = if menu_total <= menu_visible_rows {
                0
            } else {
                let half = menu_visible_rows / 2;
                if selected <= half {
                    0
                } else if selected + half >= menu_total {
                    menu_total.saturating_sub(menu_visible_rows)
                } else {
                    selected.saturating_sub(half)
                }
            };
            let menu_bottom = (menu_top + menu_visible_rows).min(menu_total);

            // Label column width — grows to fit the widest visible name
            // (including alias hint like " or /bangzhu") but stays bounded.
            let label_width = self
                .slash_menu_entries
                .iter()
                .take(menu_bottom)
                .skip(menu_top)
                .map(|e| {
                    if let Some(ref hint) = e.alias_hint {
                        format!("{} or /{}", e.name, hint).width()
                    } else {
                        e.name.width()
                    }
                })
                .max()
                .unwrap_or(22)
                .min(content_width.saturating_sub(4))
                .max(8);
            for (idx, entry) in self
                .slash_menu_entries
                .iter()
                .enumerate()
                .take(menu_bottom)
                .skip(menu_top)
            {
                let is_selected = idx == selected;
                let sel_style = if is_selected {
                    Style::default()
                        .fg(palette::SELECTION_TEXT)
                        .bg(palette::SELECTION_BG)
                } else {
                    Style::default().fg(palette::TEXT_MUTED)
                };
                let marker = if is_selected { "▸" } else { " " };

                // Name column
                let name_style = if entry.is_skill && !is_selected {
                    Style::default().fg(palette::DEEPSEEK_SKY)
                } else {
                    sel_style
                };

                // Description column (muted when not selected, secondary when selected)
                let desc_style = if is_selected {
                    Style::default()
                        .fg(palette::SELECTION_TEXT)
                        .bg(palette::SELECTION_BG)
                } else {
                    Style::default().fg(palette::TEXT_DIM)
                };

                // Build display name: canonical name, with "or /alias" hint
                // when the user typed via a pinyin alias.
                let display_name = if let Some(ref hint) = entry.alias_hint {
                    format!("{} or /{}", entry.name, hint)
                } else {
                    entry.name.clone()
                };

                let name_display = {
                    let display_width: usize = display_name.width();
                    if display_width > label_width {
                        let mut s = String::new();
                        let mut w = 0;
                        for ch in display_name.chars() {
                            let cw = ch.width().unwrap_or(0);
                            if w + cw + 1 > label_width {
                                break;
                            }
                            s.push(ch);
                            w += cw;
                        }
                        s.push('…');
                        // pad to label_width display cols
                        while s.width() < label_width {
                            s.push(' ');
                        }
                        s
                    } else {
                        // pad to label_width display cols
                        let mut s = display_name;
                        while s.width() < label_width {
                            s.push(' ');
                        }
                        s
                    }
                };

                // Skill marker prefix
                let skill_prefix = if entry.is_skill { "✦" } else { " " };

                // Compute exact prefix display width to avoid Paragraph wrap:
                // 1(" ") + 1(marker) + skill_prefix.width() + label_width + 2("  ")
                let prefix_display_width = 1 + 1 + skill_prefix.width() + label_width + 2;
                let desc_capacity = content_width.saturating_sub(prefix_display_width);
                let desc_display = {
                    let display_width: usize = entry.description.width();
                    if display_width > desc_capacity && desc_capacity > 0 {
                        let mut s = String::new();
                        let mut w = 0;
                        for ch in entry.description.chars() {
                            let cw = ch.width().unwrap_or(0);
                            if w + cw + 1 > desc_capacity {
                                break;
                            }
                            s.push(ch);
                            w += cw;
                        }
                        s.push('…');
                        s
                    } else {
                        entry.description.clone()
                    }
                };

                lines.push(Line::from(vec![
                    Span::styled(" ", Style::default()),
                    Span::styled(marker, sel_style),
                    Span::styled(skill_prefix, name_style),
                    Span::styled(name_display, name_style),
                    Span::styled("  ", desc_style),
                    Span::styled(desc_display, desc_style),
                ]));
            }
        }

        let paragraph = Paragraph::new(lines)
            .style(background)
            .wrap(Wrap { trim: false });
        paragraph.render(inner_area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        composer_height(
            self.app.composer_display_input(),
            width,
            self.max_height.min(self.max_height_cap()),
            self.active_menu_reserved_rows(),
            self.app.composer_density,
            self.app.composer_border,
        )
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        let inner_area = self.inner_area(area);
        let input_text = self.app.composer_display_input();
        let input_cursor = self.app.composer_display_cursor();
        let content_width = usize::from(inner_area.width.max(1));
        // Match the render path's locked-budget calculation so the cursor
        // lands on the same row the input is drawn on.
        let input_rows_budget =
            composer_input_rows_budget(inner_area.height, self.active_menu_reserved_rows());

        let (visible_lines, cursor_row, cursor_col) =
            layout_input(input_text, input_cursor, content_width, input_rows_budget);
        let visual_rows = if input_text.is_empty() {
            let placeholder: &str = if let Some(ref suggestion) = self.app.prompt_suggestion {
                suggestion.as_str()
            } else if self.app.is_history_search_active() {
                self.app
                    .tr(crate::localization::MessageId::HistorySearchPlaceholder)
            } else {
                self.app
                    .tr(crate::localization::MessageId::ComposerPlaceholder)
            };
            placeholder_visual_lines_for(placeholder, content_width)
        } else {
            visible_lines.len()
        };
        let top_padding = composer_top_padding(visual_rows, input_rows_budget);

        let cursor_x = area
            .x
            .saturating_add(inner_area.x.saturating_sub(area.x))
            .saturating_add(u16::try_from(cursor_col).unwrap_or(u16::MAX));
        let cursor_y = area
            .y
            .saturating_add(inner_area.y.saturating_sub(area.y))
            .saturating_add(u16::try_from(top_padding + cursor_row).unwrap_or(u16::MAX));
        if cursor_x < area.x + area.width && cursor_y < area.y + area.height {
            Some((cursor_x, cursor_y))
        } else {
            None
        }
    }
}

/// Codex-style full-screen approval takeover (#129).
///
/// The widget reads its selected option and locale directly from the
/// [`ApprovalView`]. Rendering reflows to fill most of the transcript
/// area instead of a centered popup; on small terminals it falls back to
/// a 65×22 card so existing snapshot tests still see a coherent layout.
pub struct ApprovalWidget<'a> {
    request: &'a ApprovalRequest,
    view: &'a ApprovalView,
}

impl<'a> ApprovalWidget<'a> {
    pub fn new(request: &'a ApprovalRequest, view: &'a ApprovalView) -> Self {
        Self { request, view }
    }
}

/// Layout pad around the takeover card. Generous so the modal feels
/// like a takeover rather than a popup, but never larger than the
/// terminal can hold.
const APPROVAL_CARD_HORIZONTAL_PAD: u16 = 6;
const APPROVAL_CARD_VERTICAL_PAD: u16 = 2;
/// Minimum card height — anything tighter and the approval controls
/// overlap the option list.
const APPROVAL_CARD_MIN_HEIGHT: u16 = 18;
/// Minimum card width — anything tighter makes approval copy wrap too
/// aggressively on small terminals.
const APPROVAL_CARD_MIN_WIDTH: u16 = 40;
/// Maximum card height — taller cards stop reading like a focused
/// takeover and waste vertical space on large terminals.
const APPROVAL_CARD_MAX_HEIGHT: u16 = 28;
/// Maximum card width — readability craters past this on wide terminals.
const APPROVAL_CARD_MAX_WIDTH: u16 = 96;

impl Renderable for ApprovalWidget<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        // Collapsed mode: a single-line banner at the bottom of the area
        // so the user can still see the transcript behind it.
        if self.view.collapsed {
            let bar_y = area.y.saturating_add(area.height.saturating_sub(1));
            let bar_area = Rect::new(area.x, bar_y, area.width, 1);
            Clear.render(bar_area, buf);

            let risk = self.request.risk;
            let palette_colors = approval_palette(risk);
            let summary = format!(
                " {} — {}  [Tab to expand] ",
                self.request.tool_name,
                risk_badge_text(risk, self.view.locale()),
            );
            let line = Line::from(Span::styled(
                summary,
                Style::default()
                    .fg(palette::DEEPSEEK_INK)
                    .bg(palette_colors.accent)
                    .add_modifier(Modifier::BOLD),
            ));
            Paragraph::new(line).render(bar_area, buf);
            return;
        }

        let card_area = compute_takeover_area(area);
        Clear.render(card_area, buf);

        let risk = self.request.risk;
        let locale = self.view.locale();
        let palette_colors = approval_palette(risk);
        let mut lines: Vec<Line<'static>> = Vec::with_capacity(20);

        // Header: stakes badge + tool identifier. The badge is the
        // first thing the eye lands on.
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(" {} ", risk_badge_text(risk, locale)),
                Style::default()
                    .fg(palette::DEEPSEEK_INK)
                    .bg(palette_colors.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                self.request.tool_name.clone(),
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        // Category line — English remains the baseline while localized
        // sessions get the same risk category in their UI language.
        let (cat_label, cat_color) = category_label_for(self.request.category, locale);
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(label_type(locale), Style::default().fg(palette::TEXT_HINT)),
            Span::styled(
                cat_label,
                Style::default().fg(cat_color).add_modifier(Modifier::BOLD),
            ),
        ]));

        lines.push(Line::from(""));
        // About + impacts. Impact lines are the load-bearing content;
        // they tell the user what will happen.
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(label_about(locale), Style::default().fg(palette::TEXT_HINT)),
            Span::styled(
                self.request.description_for_locale(locale),
                Style::default().fg(palette::TEXT_BODY),
            ),
        ]));
        for impact in self.request.impacts_for_locale(locale).into_iter().take(4) {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    label_impact(locale),
                    Style::default().fg(palette::TEXT_HINT),
                ),
                Span::styled(impact, Style::default().fg(palette::TEXT_BODY)),
            ]));
        }

        // Intent summary — the model's explanation of why this change is needed (#2381).
        if let Some(ref summary) = self.request.intent_summary {
            let max_width = card_area.width.saturating_sub(14) as usize;
            if max_width > 0 {
                lines.push(Line::from(""));
                let intent_label = tr(locale, MessageId::ApprovalIntentLabel);
                let summary_lines: Vec<&str> = summary.lines().collect();
                for (i, sline) in summary_lines.iter().take(3).enumerate() {
                    let prefix = if i == 0 { intent_label } else { "  " };
                    let truncated = crate::utils::truncate_with_ellipsis(sline, max_width, "...");
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            prefix,
                            if i == 0 {
                                Style::default().fg(palette::TEXT_HINT)
                            } else {
                                Style::default()
                            },
                        ),
                        Span::styled(truncated, Style::default().fg(palette::TEXT_SECONDARY)),
                    ]));
                }
                if summary_lines.len() > 3 {
                    let more = tr(locale, MessageId::ApprovalMoreLines)
                        .replace("{count}", &(summary_lines.len() - 3).to_string());
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(more, Style::default().fg(palette::TEXT_HINT)),
                    ]));
                }
            }
        }

        lines.push(Line::from(""));
        let details = self.request.prominent_detail_items(locale);
        if details.is_empty() {
            let params_str = self.request.params_display();
            let params_width = card_area.width.saturating_sub(14) as usize;
            let params_truncated =
                crate::utils::truncate_with_ellipsis(&params_str, params_width.max(20), "...");
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    label_params(locale),
                    Style::default().fg(palette::TEXT_HINT),
                ),
                Span::styled(
                    params_truncated,
                    Style::default().fg(palette::TEXT_SECONDARY),
                ),
            ]));
        } else {
            for detail in details.iter().take(4) {
                if self.request.category == ToolCategory::Shell
                    && matches!(detail.label.as_str(), "Command" | "命令")
                    && let Some(shell_lines) = detail.shell_lines.as_deref()
                {
                    push_shell_command_lines(&mut lines, &detail.label, shell_lines);
                } else {
                    push_detail_line(&mut lines, &detail.label, &detail.value);
                }
            }
        }

        lines.push(Line::from(""));

        let options = approval_options_for(risk, locale);

        for (i, opt) in options.iter().enumerate() {
            let is_selected = i == self.view.selected();
            let label_color = if opt.dangerous {
                palette_colors.accent
            } else {
                palette::TEXT_BODY
            };

            let option_style = approval_option_style(is_selected, label_color);
            let shortcut_style = approval_option_style(is_selected, palette_colors.shortcut);

            let spans = vec![
                Span::raw("  "),
                Span::styled(
                    format!("[{}] ", opt.key_hint),
                    shortcut_style.add_modifier(Modifier::BOLD),
                ),
                Span::styled(opt.label.to_string(), option_style),
            ];
            lines.push(Line::from(spans));
        }

        // Footer: Enter commits the highlighted row; y/a/d remain direct
        // shortcuts for users who do not want to move the selection.
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                selection_hint_prefix(locale),
                Style::default().fg(palette::TEXT_HINT),
            ),
            Span::styled(
                selection_hint_value(locale),
                Style::default()
                    .fg(palette_colors.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                footer_controls(locale),
                Style::default().fg(palette::TEXT_HINT),
            ),
        ]));

        let title = format!(
            " {} {} — {} ",
            risk_badge_text(risk, locale),
            approval_word(locale),
            self.request.tool_name
        );
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette_colors.border))
            .style(Style::default().bg(palette::DEEPSEEK_INK))
            .padding(Padding::uniform(1));

        // Render the card body inside the block, then paint the warm
        // accent rail on the destructive variant. The rail uses a
        // single-cell column so it doesn't shift the body layout.
        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });
        paragraph.render(card_area, buf);

        if matches!(risk, RiskLevel::Destructive) {
            paint_left_rail(card_area, buf, palette_colors.accent);
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

/// Compute the card rect inside `area`. Always centered; pad on every
/// side so the takeover reads as a takeover but a small terminal still
/// stays inside the buffer. Very small terminals may truncate the card
/// content, but rendering must never address cells outside `area`.
fn compute_takeover_area(area: Rect) -> Rect {
    let avail_width = area.width.saturating_sub(APPROVAL_CARD_HORIZONTAL_PAD * 2);
    let avail_height = area.height.saturating_sub(APPROVAL_CARD_VERTICAL_PAD * 2);
    let card_width = APPROVAL_CARD_MAX_WIDTH
        .min(avail_width)
        .max(APPROVAL_CARD_MIN_WIDTH)
        .min(area.width);
    let card_height = APPROVAL_CARD_MIN_HEIGHT
        .max(avail_height.min(APPROVAL_CARD_MAX_HEIGHT))
        .min(area.height);
    let x = area.x + (area.width.saturating_sub(card_width)) / 2;
    let y = area.y + (area.height.saturating_sub(card_height)) / 2;
    Rect {
        x,
        y,
        width: card_width,
        height: card_height,
    }
}

/// Paint a single-column accent on the inside-left of the card. Only
/// touches cells that already exist in the buffer area.
fn paint_left_rail(card: Rect, buf: &mut Buffer, color: Color) {
    if card.width < 2 || card.height < 4 {
        return;
    }
    let rail_x = card.x + 1;
    let top = card.y + 1;
    let bot = card.y + card.height.saturating_sub(2);
    for y in top..=bot {
        if y >= buf.area.y + buf.area.height {
            break;
        }
        let cell = &mut buf[(rail_x, y)];
        cell.set_char('\u{2503}'); // ┃ — heavy bar so the warning reads at a glance
        cell.set_style(Style::default().fg(color).bg(palette::DEEPSEEK_INK));
    }
}

/// Approval palette per risk variant.
struct ApprovalColors {
    border: Color,
    accent: Color,
    shortcut: Color,
}

fn approval_palette(risk: RiskLevel) -> ApprovalColors {
    match risk {
        RiskLevel::Benign => ApprovalColors {
            border: palette::BORDER_COLOR,
            accent: palette::DEEPSEEK_SKY,
            shortcut: palette::DEEPSEEK_SKY,
        },
        RiskLevel::Destructive => ApprovalColors {
            border: palette::DEEPSEEK_RED,
            accent: palette::DEEPSEEK_RED,
            shortcut: palette::STATUS_WARNING,
        },
    }
}

fn approval_selected_style() -> Style {
    Style::default()
        .fg(palette::SELECTION_TEXT)
        .bg(palette::SELECTION_BG)
        .add_modifier(Modifier::BOLD)
}

fn approval_option_style(is_selected: bool, color: Color) -> Style {
    if is_selected {
        approval_selected_style()
    } else {
        Style::default().fg(color)
    }
}

fn risk_badge_text(risk: RiskLevel, locale: Locale) -> &'static str {
    match risk {
        RiskLevel::Benign => tr(locale, MessageId::ApprovalRiskReview),
        RiskLevel::Destructive => tr(locale, MessageId::ApprovalRiskDestructive),
    }
}

fn category_label_for(category: ToolCategory, locale: Locale) -> (&'static str, Color) {
    let label = match category {
        ToolCategory::Safe => tr(locale, MessageId::ApprovalCategorySafe),
        ToolCategory::FileWrite => tr(locale, MessageId::ApprovalCategoryFileWrite),
        ToolCategory::Shell => tr(locale, MessageId::ApprovalCategoryShell),
        ToolCategory::Network => tr(locale, MessageId::ApprovalCategoryNetwork),
        ToolCategory::McpRead => tr(locale, MessageId::ApprovalCategoryMcpRead),
        ToolCategory::McpAction => tr(locale, MessageId::ApprovalCategoryMcpAction),
        ToolCategory::Unknown => tr(locale, MessageId::ApprovalCategoryUnknown),
    };
    let color = match category {
        ToolCategory::Safe => palette::STATUS_SUCCESS,
        ToolCategory::FileWrite => palette::STATUS_WARNING,
        ToolCategory::Shell => palette::STATUS_ERROR,
        ToolCategory::Network => palette::STATUS_WARNING,
        ToolCategory::McpRead => palette::DEEPSEEK_SKY,
        ToolCategory::McpAction => palette::STATUS_WARNING,
        ToolCategory::Unknown => palette::STATUS_ERROR,
    };
    (label, color)
}

fn approval_word(locale: Locale) -> &'static str {
    tr(locale, MessageId::ApprovalBlockTitle)
}

fn label_type(locale: Locale) -> &'static str {
    tr(locale, MessageId::ApprovalFieldType)
}

fn label_about(locale: Locale) -> &'static str {
    tr(locale, MessageId::ApprovalFieldAbout)
}

fn label_impact(locale: Locale) -> &'static str {
    tr(locale, MessageId::ApprovalFieldImpact)
}

fn label_params(locale: Locale) -> &'static str {
    tr(locale, MessageId::ApprovalFieldParams)
}

fn push_detail_line(lines: &mut Vec<Line<'static>>, label: &str, value: &str) {
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{label:<7} "),
            Style::default()
                .fg(palette::DEEPSEEK_SKY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(value.to_string(), Style::default().fg(palette::TEXT_BODY)),
    ]));
}

fn push_shell_command_lines(lines: &mut Vec<Line<'static>>, label: &str, command_lines: &[String]) {
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{label}:"),
            Style::default()
                .fg(palette::DEEPSEEK_SKY)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    for line in command_lines {
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(
                line.clone(),
                Style::default()
                    .fg(palette::TEXT_BODY)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
}

fn footer_controls(locale: Locale) -> &'static str {
    tr(locale, MessageId::ApprovalControlsHint)
}

fn selection_hint_prefix(locale: Locale) -> &'static str {
    tr(locale, MessageId::ApprovalChooseHint)
}

fn selection_hint_value(locale: Locale) -> &'static str {
    tr(locale, MessageId::ApprovalChooseAction)
}

struct ApprovalOptionRow {
    label: &'static str,
    key_hint: &'static str,
    dangerous: bool,
}

fn approval_options_for(risk: RiskLevel, locale: Locale) -> [ApprovalOptionRow; 4] {
    let dangerous = matches!(risk, RiskLevel::Destructive);
    [
        ApprovalOptionRow {
            label: option_approve_once(locale),
            key_hint: "1 / y",
            dangerous,
        },
        ApprovalOptionRow {
            label: option_approve_always(locale),
            key_hint: "2 / a",
            dangerous,
        },
        ApprovalOptionRow {
            label: option_deny(locale),
            key_hint: "3 / d / n",
            dangerous: false,
        },
        ApprovalOptionRow {
            label: option_abort(locale),
            key_hint: "Esc",
            dangerous: false,
        },
    ]
}

fn option_approve_once(locale: Locale) -> &'static str {
    tr(locale, MessageId::ApprovalOptionApproveOnce)
}

fn option_approve_always(locale: Locale) -> &'static str {
    tr(locale, MessageId::ApprovalOptionApproveAlways)
}

fn option_deny(locale: Locale) -> &'static str {
    tr(locale, MessageId::ApprovalOptionDeny)
}

fn option_abort(locale: Locale) -> &'static str {
    tr(locale, MessageId::ApprovalOptionAbortTurn)
}

pub struct ElevationWidget<'a> {
    request: &'a ElevationRequest,
    selected: usize,
    locale: Locale,
}

impl<'a> ElevationWidget<'a> {
    pub fn new(request: &'a ElevationRequest, selected: usize, locale: Locale) -> Self {
        Self {
            request,
            selected,
            locale,
        }
    }
}

impl Renderable for ElevationWidget<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        use crate::localization::MessageId;
        use crate::localization::tr;

        let popup_width = 70.min(area.width.saturating_sub(4));
        let popup_height = 22.min(area.height.saturating_sub(4));
        let popup_area = Rect {
            x: (area.width.saturating_sub(popup_width)) / 2,
            y: (area.height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };

        Clear.render(popup_area, buf);

        let mut lines = vec![
            Line::from(""),
            Line::from(vec![Span::styled(
                tr(self.locale, MessageId::ElevationTitleSandboxDenied),
                Style::default()
                    .fg(palette::STATUS_ERROR)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from(vec![
                Span::raw(tr(self.locale, MessageId::ElevationFieldTool)),
                Span::styled(
                    &self.request.tool_name,
                    Style::default()
                        .fg(palette::DEEPSEEK_SKY)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ];

        if let Some(ref command) = self.request.command {
            let cmd_display = crate::utils::truncate_with_ellipsis(command, 45, "...");
            lines.push(Line::from(vec![
                Span::raw(tr(self.locale, MessageId::ElevationFieldCmd)),
                Span::styled(cmd_display, Style::default().fg(palette::TEXT_MUTED)),
            ]));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw(tr(self.locale, MessageId::ElevationFieldReason)),
            Span::styled(
                &self.request.denial_reason,
                Style::default().fg(palette::STATUS_WARNING),
            ),
        ]));

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            tr(self.locale, MessageId::ElevationImpactHeader),
            Style::default().fg(palette::TEXT_MUTED),
        )));
        if self
            .request
            .options
            .iter()
            .any(|option| matches!(option, ElevationOption::WithNetwork))
        {
            lines.push(Line::from(Span::styled(
                tr(self.locale, MessageId::ElevationImpactNetwork),
                Style::default().fg(palette::TEXT_PRIMARY),
            )));
        }
        if self
            .request
            .options
            .iter()
            .any(|option| matches!(option, ElevationOption::WithWriteAccess(_)))
        {
            lines.push(Line::from(Span::styled(
                tr(self.locale, MessageId::ElevationImpactWrite),
                Style::default().fg(palette::TEXT_PRIMARY),
            )));
        }
        lines.push(Line::from(Span::styled(
            tr(self.locale, MessageId::ElevationImpactFullAccess),
            Style::default().fg(palette::TEXT_PRIMARY),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            tr(self.locale, MessageId::ElevationPromptProceed),
            Style::default().fg(palette::TEXT_MUTED),
        )));
        lines.push(Line::from(""));

        for (i, option) in self.request.options.iter().enumerate() {
            let is_selected = i == self.selected;
            let style = if is_selected {
                Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(palette::SELECTION_BG)
            } else {
                Style::default()
            };

            let (key, label_id, desc_id) = match option {
                ElevationOption::WithNetwork => (
                    "n",
                    MessageId::ElevationOptionNetwork,
                    MessageId::ElevationOptionNetworkDesc,
                ),
                ElevationOption::WithWriteAccess(_) => (
                    "w",
                    MessageId::ElevationOptionWrite,
                    MessageId::ElevationOptionWriteDesc,
                ),
                ElevationOption::FullAccess => (
                    "f",
                    MessageId::ElevationOptionFullAccess,
                    MessageId::ElevationOptionFullAccessDesc,
                ),
                ElevationOption::Abort => (
                    "a",
                    MessageId::ElevationOptionAbort,
                    MessageId::ElevationOptionAbortDesc,
                ),
            };

            let label_color = match option {
                ElevationOption::Abort => palette::TEXT_MUTED,
                ElevationOption::FullAccess => palette::STATUS_ERROR,
                _ => palette::TEXT_PRIMARY,
            };

            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("[{key}] "),
                    Style::default().fg(palette::STATUS_SUCCESS),
                ),
                Span::styled(tr(self.locale, label_id), style.fg(label_color)),
            ]));
            lines.push(Line::from(vec![
                Span::raw("      "),
                Span::styled(
                    tr(self.locale, desc_id),
                    Style::default().fg(palette::TEXT_MUTED),
                ),
            ]));
        }

        let title = tr(self.locale, MessageId::ElevationTitleRequired);
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::DEEPSEEK_INK))
            .padding(Padding::uniform(1));

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });

        paragraph.render(popup_area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

pub(crate) fn pad_lines_to_bottom(lines: &mut Vec<Line<'static>>, height: usize) {
    if lines.len() >= height {
        return;
    }
    let padding = height.saturating_sub(lines.len());
    if padding == 0 {
        return;
    }

    let mut padded = Vec::with_capacity(height);
    padded.extend(std::iter::repeat_n(Line::from(""), padding));
    padded.append(lines);
    *lines = padded;
}

fn apply_selection(lines: &mut [Line<'static>], top: usize, app: &App) {
    let Some((start, end)) = app.viewport.transcript_selection.ordered_endpoints() else {
        return;
    };

    let selection_style = Style::default()
        .bg(app.ui_theme.selection_bg)
        .fg(palette::SELECTION_TEXT);

    for (idx, line) in lines.iter_mut().enumerate() {
        let line_index = top + idx;
        if line_index < start.line_index || line_index > end.line_index {
            continue;
        }

        let (col_start, col_end) = if start.line_index == end.line_index {
            (start.column, end.column)
        } else if line_index == start.line_index {
            (start.column, usize::MAX)
        } else if line_index == end.line_index {
            (0, end.column)
        } else {
            (0, usize::MAX)
        };

        if col_start == 0 && col_end == usize::MAX {
            for span in &mut line.spans {
                span.style = span.style.patch(selection_style);
            }
            continue;
        }

        line.spans = apply_selection_to_line(line, col_start, col_end, selection_style);
    }
}

fn apply_detail_target_highlight(
    lines: &mut [Line<'static>],
    top: usize,
    target_cell: usize,
    line_meta: &[TranscriptLineMeta],
    original_index_map: &[usize],
) {
    let highlight_bg = Color::Reset;
    for (idx, line) in lines.iter_mut().enumerate() {
        let line_index = top + idx;
        if let Some(TranscriptLineMeta::CellLine { cell_index, .. }) = line_meta.get(line_index)
            && original_index_map
                .get(*cell_index)
                .copied()
                .unwrap_or(*cell_index)
                == target_cell
        {
            for span in &mut line.spans {
                span.style = span.style.bg(highlight_bg);
            }
        }
    }
}

/// Apply a brief background tint to the last user message's visible lines.
fn apply_send_flash(
    lines: &mut [Line<'static>],
    top: usize,
    history: &[HistoryCell],
    line_meta: &[TranscriptLineMeta],
    original_index_map: &[usize],
) {
    // Find the last User cell index.
    let last_user_cell = history
        .iter()
        .rposition(|cell| matches!(cell, HistoryCell::User { .. }));
    let Some(target_cell) = last_user_cell else {
        return;
    };

    let flash_bg = Color::Rgb(30, 40, 55); // subtle dark-blue tint

    for (idx, line) in lines.iter_mut().enumerate() {
        let line_index = top + idx;
        if let Some(TranscriptLineMeta::CellLine { cell_index, .. }) = line_meta.get(line_index)
            && original_index_map
                .get(*cell_index)
                .copied()
                .unwrap_or(*cell_index)
                == target_cell
        {
            for span in &mut line.spans {
                span.style = span.style.bg(flash_bg);
            }
        }
    }
}

fn apply_selection_to_line(
    line: &Line<'static>,
    col_start: usize,
    col_end: usize,
    selection_style: Style,
) -> Vec<Span<'static>> {
    let mut result = Vec::with_capacity(line.spans.len().saturating_add(2));
    let mut current_col = 0usize;

    for span in &line.spans {
        let span_text: &str = span.content.as_ref();
        let span_width = text_display_width(span_text);
        let span_end = current_col.saturating_add(span_width);

        if span_end <= col_start || current_col >= col_end {
            result.push(span.clone());
        } else if current_col >= col_start && span_end <= col_end {
            result.push(Span::styled(
                span.content.clone(),
                span.style.patch(selection_style),
            ));
        } else {
            let mut before = String::new();
            let mut selected = String::new();
            let mut after = String::new();
            let mut ch_col = current_col;

            for ch in span_text.chars() {
                let ch_width = char_display_width(ch);
                let ch_start = ch_col;
                let ch_end = ch_col.saturating_add(ch_width);
                if ch_end <= col_start {
                    before.push(ch);
                } else if ch_start >= col_end {
                    after.push(ch);
                } else {
                    selected.push(ch);
                }
                ch_col = ch_end;
            }

            if !before.is_empty() {
                result.push(Span::styled(before, span.style));
            }
            if !selected.is_empty() {
                result.push(Span::styled(selected, span.style.patch(selection_style)));
            }
            if !after.is_empty() {
                result.push(Span::styled(after, span.style));
            }
        }

        current_col = span_end;
    }

    result
}

fn text_display_width(text: &str) -> usize {
    text.chars().map(char_display_width).sum()
}

fn char_display_width(ch: char) -> usize {
    if ch == '\t' {
        4
    } else {
        UnicodeWidthChar::width(ch).unwrap_or(0).max(1)
    }
}

fn truncate_display_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    if max_width <= 3 {
        return text.chars().take(max_width).collect();
    }

    let mut out = String::new();
    let mut width = 0usize;
    let limit = max_width.saturating_sub(3);
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > limit {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push_str("...");
    out
}

fn vim_mode_style(mode: VimMode) -> Style {
    let color = match mode {
        VimMode::Normal => palette::TEXT_MUTED,
        VimMode::Insert => palette::DEEPSEEK_SKY,
        VimMode::Visual => palette::MODE_PLAN,
    };
    Style::default().fg(color).bold()
}

fn composer_top_right_chrome(app: &App, area_width: u16) -> Option<Line<'static>> {
    let receipt = app.active_receipt_text();
    let session_title = app.session_title.as_deref();
    if !app.composer.vim_enabled && receipt.is_none() && session_title.is_none() {
        return None;
    }

    // Leave room for the left title and both borders. On narrow panes, skip
    // extra chrome rather than letting status text collide with "Composer".
    let max_width = usize::from(area_width.saturating_sub(18));
    if max_width < 4 {
        return None;
    }

    let receipt_style = Style::default()
        .fg(palette::STATUS_SUCCESS)
        .add_modifier(Modifier::DIM);
    if let Some(receipt) = receipt {
        let receipt_text = receipt.trim();
        if app.composer.vim_enabled {
            let vim_label = app.composer.vim_mode.label();
            let vim_width = UnicodeWidthStr::width(vim_label);
            let sep_width = UnicodeWidthStr::width(" · ");
            if vim_width + sep_width + 4 <= max_width {
                let receipt_width = max_width.saturating_sub(vim_width + sep_width);
                return Some(Line::from(vec![
                    Span::styled(vim_label.to_string(), vim_mode_style(app.composer.vim_mode)),
                    Span::styled(" · ", Style::default().fg(palette::TEXT_MUTED)),
                    Span::styled(
                        truncate_display_width(receipt_text, receipt_width),
                        receipt_style,
                    ),
                ]));
            }
        }

        return Some(Line::from(Span::styled(
            truncate_display_width(receipt_text, max_width),
            receipt_style,
        )));
    }

    let mut spans: Vec<Span> = Vec::new();
    if app.composer.vim_enabled {
        spans.push(Span::styled(
            truncate_display_width(app.composer.vim_mode.label(), max_width),
            vim_mode_style(app.composer.vim_mode),
        ));
    }
    if let Some(title) = session_title {
        let used: usize = spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        let sep = if spans.is_empty() { 0 } else { 2 };
        let remaining = max_width.saturating_sub(used + sep);
        if remaining >= 4 {
            if !spans.is_empty() {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                truncate_display_width(title, remaining),
                Style::default().fg(palette::TEXT_MUTED),
            ));
        }
    }
    if spans.is_empty() {
        None
    } else {
        Some(Line::from(spans))
    }
}

fn should_render_empty_state(app: &App) -> bool {
    let active_is_empty = app
        .active_cell
        .as_ref()
        .is_none_or(crate::tui::active_cell::ActiveCell::is_empty);
    app.history.is_empty()
        && active_is_empty
        && !app.is_loading
        && !app.is_compacting
        && !app.is_purging
}

fn build_empty_state_lines(app: &App, area: Rect) -> Vec<Line<'static>> {
    if area.width == 0 || area.height == 0 {
        return Vec::new();
    }

    let workspace = crate::utils::display_path(&app.workspace);
    let body_width = usize::from(area.width.saturating_sub(8).clamp(24, 72));
    let left_padding = usize::from(area.width.saturating_sub(body_width as u16) / 2);
    let inset = " ".repeat(left_padding);

    let body = vec![
        Line::from(Span::styled(
            format!("{inset}>_ codewhale (v{})", env!("CARGO_PKG_VERSION")),
            Style::default().fg(palette::WHALE_ACCENT_PRIMARY).bold(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("{inset}model: {}  /model to switch", app.model),
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(Span::styled(
            format!("{inset}directory: {workspace}"),
            Style::default().fg(palette::TEXT_MUTED),
        )),
    ];

    // Keep the welcome block near the top of the chat pane (header is separate).
    let top_padding = 2usize;
    let mut lines = Vec::new();
    for _ in 0..top_padding {
        lines.push(Line::from(""));
    }
    lines.extend(body);
    lines
}

pub fn composer_input_rows_budget(inner_height: u16, extra_lines: usize) -> usize {
    usize::from(inner_height).saturating_sub(extra_lines).max(1)
}

fn composer_top_padding(content_lines: usize, rows_budget: usize) -> usize {
    rows_budget.saturating_sub(content_lines.clamp(1, rows_budget))
}

/// Placeholder text shown when the composer input is empty.
#[cfg(test)]
const COMPOSER_PLACEHOLDER: &str = "Write a task or use /.";

/// How many visual rows the empty-input placeholder occupies after wrapping.
#[cfg(test)]
fn placeholder_visual_lines(content_width: usize) -> usize {
    placeholder_visual_lines_for(COMPOSER_PLACEHOLDER, content_width)
}

fn placeholder_visual_lines_for(placeholder: &str, content_width: usize) -> usize {
    wrap_text(placeholder, content_width).len().max(1)
}

fn composer_min_input_rows(density: ComposerDensity) -> usize {
    match density {
        ComposerDensity::Compact => 2,
        ComposerDensity::Comfortable => 3,
        ComposerDensity::Spacious => 4,
    }
}

fn composer_max_height(density: ComposerDensity) -> u16 {
    match density {
        ComposerDensity::Compact => 7,
        ComposerDensity::Comfortable => 9,
        ComposerDensity::Spacious => 12,
    }
}

fn composer_height(
    input: &str,
    width: u16,
    available_height: u16,
    extra_lines: usize,
    density: ComposerDensity,
    show_panel: bool,
) -> u16 {
    let has_panel = show_panel && available_height >= 3 && width >= 12;
    let chrome_height = if has_panel {
        usize::from(COMPOSER_PANEL_HEIGHT)
    } else {
        0
    };
    let content_width = if has_panel {
        usize::from(width.saturating_sub(2).max(1))
    } else {
        usize::from(width.max(1))
    };
    let mut line_count = wrap_input_lines(input, content_width).len();
    if line_count == 0 {
        line_count = 1;
    }
    if has_panel {
        line_count = line_count.max(composer_min_input_rows(density));
    }
    line_count = line_count
        .saturating_add(extra_lines)
        .saturating_add(chrome_height);
    let max_height = usize::from(available_height.clamp(1, composer_max_height(density)));
    line_count.clamp(1, max_height).try_into().unwrap_or(1)
}

/// A single entry in the slash-command autocomplete popup.
pub(crate) struct SlashMenuEntry {
    pub name: String,
    pub description: String,
    pub is_skill: bool,
    /// Matching pinyin/alias prefix hint, e.g. when user types `/bang` and
    /// the command `/help` matches via alias `bangzhu`.
    pub alias_hint: Option<String>,
}

/// Check if all characters in `needle` appear in `haystack` in order
/// (subsequence matching — fuzzy filtering).
fn fuzzy_chars_in_order(needle: &str, haystack: &str) -> bool {
    let mut chars = needle.chars();
    let mut current = match chars.next() {
        Some(c) => c,
        None => return true,
    };
    for ch in haystack.chars() {
        if ch == current {
            if let Some(next) = chars.next() {
                current = next;
            } else {
                return true;
            }
        }
    }
    false
}

pub(crate) fn slash_completion_hints(
    input: &str,
    limit: usize,
    cached_skills: &[(String, String)],
    locale: crate::localization::Locale,
    workspace: Option<&std::path::Path>,
    api_provider: ApiProvider,
) -> Vec<SlashMenuEntry> {
    if !super::app::looks_like_slash_command_input(input) {
        return Vec::new();
    }

    if let Some(skill_prefix) = input.trim_start().strip_prefix('$') {
        return dollar_skill_completion_hints(skill_prefix, limit, cached_skills);
    }

    let prefix = input.trim_start_matches('/');
    let completing_skill_arg = prefix.strip_prefix("skill ").map(str::trim_start);
    if input.contains(char::is_whitespace) && completing_skill_arg.is_none() {
        return Vec::new();
    }
    let mut entries: Vec<SlashMenuEntry> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let prefix_lower = prefix.to_ascii_lowercase();
    let user_commands = if completing_skill_arg.is_none() {
        commands::user_commands::load_user_commands(workspace)
    } else {
        Vec::new()
    };

    // ── Phase 1: prefix (starts_with) matches ─────────────────────────
    // Highest priority — preserves existing exact-prefix completion.
    if completing_skill_arg.is_none() {
        for name in all_command_names_matching_loaded(prefix, &user_commands) {
            seen.insert(name.clone());
            let command_key = name.trim_start_matches('/');
            push_command_entry(
                &mut entries,
                &name,
                command_key,
                &prefix_lower,
                locale,
                &user_commands,
            );
        }
    }

    // ── Phase 2: contains (substring) matches ─────────────────────────
    // Medium priority — broader catching.
    if completing_skill_arg.is_none() {
        for cmd in commands::command_infos() {
            let name = format!("/{}", cmd.name);
            if seen.contains(&name) {
                continue;
            }
            let cmd_lower = cmd.name.to_ascii_lowercase();
            let alias_match = cmd
                .aliases
                .iter()
                .any(|a| a.to_ascii_lowercase().contains(&prefix_lower));
            if cmd_lower.contains(&prefix_lower) || alias_match {
                seen.insert(name.clone());
                push_command_entry(
                    &mut entries,
                    &name,
                    cmd.name,
                    &prefix_lower,
                    locale,
                    &user_commands,
                );
            }
        }
    }

    // ── Phase 3: fuzzy subsequence matches ────────────────────────────
    // Lowest priority — characters in order, not necessarily consecutive.
    if completing_skill_arg.is_none() {
        for cmd in commands::command_infos() {
            let name = format!("/{}", cmd.name);
            if seen.contains(&name) {
                continue;
            }
            let cmd_lower = cmd.name.to_ascii_lowercase();
            let alias_match = cmd
                .aliases
                .iter()
                .any(|a| fuzzy_chars_in_order(&prefix_lower, &a.to_ascii_lowercase()));
            if fuzzy_chars_in_order(&prefix_lower, &cmd_lower) || alias_match {
                seen.insert(name.clone());
                push_command_entry(
                    &mut entries,
                    &name,
                    cmd.name,
                    &prefix_lower,
                    locale,
                    &user_commands,
                );
            }
        }
    }

    // ── Skills (only after user has typed `/skill `) ──────────────────
    let skill_prefix = completing_skill_arg.unwrap_or(prefix).to_ascii_lowercase();
    if completing_skill_arg.is_some() {
        for (skill_name, skill_desc) in cached_skills {
            let skill_name_lower = skill_name.to_ascii_lowercase();
            if skill_name_lower.starts_with(&skill_prefix) {
                entries.push(SlashMenuEntry {
                    name: format!("/skill {skill_name}"),
                    description: skill_desc.clone(),
                    is_skill: true,
                    alias_hint: None,
                });
            }
        }
        // Skills: contains fuzzy fallback
        for (skill_name, skill_desc) in cached_skills {
            let skill_name_lower = skill_name.to_ascii_lowercase();
            if skill_name_lower.contains(&skill_prefix)
                && !entries
                    .iter()
                    .any(|e| e.name == format!("/skill {skill_name}"))
            {
                entries.push(SlashMenuEntry {
                    name: format!("/skill {skill_name}"),
                    description: skill_desc.clone(),
                    is_skill: true,
                    alias_hint: None,
                });
            }
        }
        for (skill_name, skill_desc) in cached_skills {
            let skill_name_lower = skill_name.to_ascii_lowercase();
            if !skill_name_lower.starts_with(&skill_prefix)
                && !skill_name_lower.contains(&skill_prefix)
                && fuzzy_chars_in_order(&skill_prefix, &skill_name_lower)
            {
                entries.push(SlashMenuEntry {
                    name: format!("/skill {skill_name}"),
                    description: skill_desc.clone(),
                    is_skill: true,
                    alias_hint: None,
                });
            }
        }
    }

    // Special: /model <name> completions when only /model matches
    if entries.iter().any(|e| e.name == "/model") && prefix_lower.eq_ignore_ascii_case("model") {
        for model_name in model_completion_names_for_provider(api_provider) {
            entries.push(SlashMenuEntry {
                name: format!("/model {model_name}"),
                description: String::from("Switch to this model"),
                is_skill: false,
                alias_hint: None,
            });
        }
    }

    // Rank exact-alias matches above prefix/alias matches so e.g. typing
    // `/q` ranks `/exit` (alias `q` is an exact hit) above `/clear` (alias
    // `qingping` only matches by prefix). Inside each tier, fall back to
    // alphabetical name order for deterministic display (#1811).
    let rank = |entry: &SlashMenuEntry| -> u8 {
        if entry.is_skill {
            return 3;
        }
        let command_key = entry.name.trim_start_matches('/');
        if command_key.eq_ignore_ascii_case(&prefix_lower) {
            return 0;
        }
        if let Some(info) = commands::get_command_info(command_key)
            && info
                .aliases
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&prefix_lower))
        {
            return 0;
        }
        if command_key.to_ascii_lowercase().starts_with(&prefix_lower) {
            return 1;
        }
        2
    };
    entries.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.name.cmp(&b.name)));
    entries.dedup_by(|a, b| a.name == b.name);
    entries.into_iter().take(limit).collect()
}

fn dollar_skill_completion_hints(
    skill_prefix: &str,
    limit: usize,
    cached_skills: &[(String, String)],
) -> Vec<SlashMenuEntry> {
    if limit == 0 {
        return Vec::new();
    }

    let skill_prefix = skill_prefix.to_ascii_lowercase();
    let mut matches: Vec<(u8, &String, &String)> = Vec::new();

    for (skill_name, skill_desc) in cached_skills {
        let skill_name_lower = skill_name.to_ascii_lowercase();
        let rank = if skill_name_lower.starts_with(&skill_prefix) {
            0
        } else if skill_name_lower.contains(&skill_prefix) {
            1
        } else if fuzzy_chars_in_order(&skill_prefix, &skill_name_lower) {
            2
        } else {
            continue;
        };
        matches.push((rank, skill_name, skill_desc));
    }

    matches.sort_by(|(rank_a, name_a, _), (rank_b, name_b, _)| {
        rank_a.cmp(rank_b).then_with(|| name_a.cmp(name_b))
    });
    matches
        .into_iter()
        .take(limit)
        .map(|(_, skill_name, skill_desc)| SlashMenuEntry {
            name: format!("${skill_name}"),
            description: skill_desc.clone(),
            is_skill: true,
            alias_hint: None,
        })
        .collect()
}

fn all_command_names_matching_loaded(
    prefix: &str,
    user_commands: &[(String, String)],
) -> Vec<String> {
    let prefix = prefix.strip_prefix('/').unwrap_or(prefix).to_lowercase();
    let mut result: Vec<String> = commands::command_infos()
        .iter()
        .filter(|cmd| {
            cmd.name.starts_with(&prefix) || cmd.aliases.iter().any(|a| a.starts_with(&prefix))
        })
        .map(|cmd| format!("/{}", cmd.name))
        .collect();

    result.extend(
        user_commands
            .iter()
            .filter(|(name, _)| name.starts_with(&prefix))
            .map(|(name, _)| format!("/{name}")),
    );

    result.sort();
    result.dedup();
    result
}

/// Push a built-in command entry to the slash menu, resolving description
/// and alias hints.
fn push_command_entry(
    entries: &mut Vec<SlashMenuEntry>,
    name: &str,
    command_key: &str,
    prefix_lower: &str,
    locale: crate::localization::Locale,
    user_commands: &[(String, String)],
) {
    let (description, alias_hint) = if let Some(info) = commands::get_command_info(command_key) {
        let hint = if !command_key.to_ascii_lowercase().starts_with(prefix_lower) {
            info.aliases
                .iter()
                .find(|a| {
                    a.to_ascii_lowercase().starts_with(prefix_lower)
                        || a.to_ascii_lowercase().contains(prefix_lower)
                        || fuzzy_chars_in_order(prefix_lower, &a.to_ascii_lowercase())
                })
                .map(|a| a.to_string())
        } else {
            None
        };
        let desc = if info.aliases.is_empty() {
            info.description_for(locale).to_string()
        } else {
            format!(
                "{}  (aliases: {})",
                info.description_for(locale),
                info.aliases
                    .iter()
                    .map(|a| format!("/{a}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        (desc, hint)
    } else {
        let mut description = String::from("User-defined command");
        let mut argument_hint = None;
        if let Some((_, content)) = user_commands.iter().find(|(key, _)| key == command_key) {
            let (metadata, _) = commands::user_commands::parse_frontmatter(content);
            for (key, value) in metadata {
                match key.as_str() {
                    "description" => description = value,
                    "argument-hint" => argument_hint = Some(value),
                    _ => {}
                }
            }
        }
        if let Some(hint) = argument_hint
            && !hint.trim().is_empty()
        {
            description.push_str("  ");
            description.push_str(hint.trim());
        }
        (description, None)
    };
    entries.push(SlashMenuEntry {
        name: name.to_string(),
        description,
        is_skill: false,
        alias_hint,
    });
}

fn layout_input(
    input: &str,
    cursor: usize,
    width: usize,
    max_height: usize,
) -> (Vec<String>, usize, usize) {
    let (visible, visible_cursor_row, visible_cursor_col, _) =
        layout_input_with_scroll(input, cursor, width, max_height);
    (visible, visible_cursor_row, visible_cursor_col)
}

pub fn layout_input_with_scroll(
    input: &str,
    cursor: usize,
    width: usize,
    max_height: usize,
) -> (Vec<String>, usize, usize, usize) {
    let mut lines = wrap_input_lines(input, width);
    if lines.is_empty() {
        lines.push(String::new());
    }
    let (cursor_row, cursor_col) = cursor_row_col(input, cursor, width.max(1));

    let max_height = max_height.max(1);
    let mut start = 0usize;
    if cursor_row >= max_height {
        start = cursor_row + 1 - max_height;
    }
    if start + max_height > lines.len() {
        start = lines.len().saturating_sub(max_height);
    }
    let visible = lines
        .into_iter()
        .skip(start)
        .take(max_height)
        .collect::<Vec<_>>();
    let visible_cursor_row = cursor_row.saturating_sub(start);

    (
        visible,
        visible_cursor_row,
        cursor_col.min(width.saturating_sub(1)),
        start,
    )
}

fn cursor_row_col(input: &str, cursor: usize, width: usize) -> (usize, usize) {
    let mut row = 0usize;
    let mut col = 0usize;
    let mut char_idx = 0usize;

    for grapheme in input.graphemes(true) {
        if char_idx >= cursor {
            break;
        }
        let grapheme_chars = grapheme.chars().count();
        let next_char_idx = char_idx.saturating_add(grapheme_chars);
        let cursor_inside = cursor < next_char_idx;

        if grapheme == "\n" {
            row += 1;
            col = 0;
            char_idx = next_char_idx;
            if cursor_inside {
                break;
            }
            continue;
        }

        let grapheme_width = grapheme.width();
        if col + grapheme_width > width && col != 0 {
            row += 1;
            col = 0;
        }
        col += grapheme_width;
        if col >= width {
            row += 1;
            col = 0;
        }
        if cursor_inside {
            break;
        }
        char_idx = next_char_idx;
    }

    (row, col)
}

fn wrap_input_lines(input: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    if input.is_empty() {
        return lines;
    }

    for raw in input.split('\n') {
        let wrapped = wrap_text(raw, width);
        if wrapped.is_empty() {
            lines.push(String::new());
        } else {
            lines.extend(wrapped);
        }
    }

    // Note: No need for ends_with('\n') check - split('\n') already includes
    // the trailing empty string for inputs ending with newline.

    lines
}

/// For mouse coordinate mapping: returns (char_start_of_line, line_text) pairs
/// matching the wrapping produced by `wrap_input_lines`.
pub fn wrap_input_lines_for_mouse(input: &str, width: usize) -> Vec<(usize, String)> {
    if input.is_empty() || width == 0 {
        return vec![(0, String::new())];
    }

    let mut result = Vec::new();
    let mut char_idx = 0usize;

    for raw_line in input.split('\n') {
        if raw_line.is_empty() {
            result.push((char_idx, String::new()));
            char_idx += 1; // the '\n'
            continue;
        }
        let wrapped = wrap_text(raw_line, width);
        for wrapped_line in &wrapped {
            let line_char_len: usize = wrapped_line.chars().count();
            result.push((char_idx, wrapped_line.clone()));
            char_idx += line_char_len;
        }
        char_idx += 1; // the '\n'
    }

    result
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
    let mut current_width = 0;

    for grapheme in text.graphemes(true) {
        if grapheme == "\n" {
            lines.push(current);
            current = String::new();
            current_width = 0;
            continue;
        }

        let grapheme_width = grapheme.width();
        if current_width + grapheme_width > width && current_width != 0 {
            lines.push(current);
            current = String::new();
            current_width = 0;
        }

        current.push_str(grapheme);
        current_width += grapheme_width;

        if current_width >= width {
            lines.push(current);
            current = String::new();
            current_width = 0;
        }
    }

    lines.push(current);
    lines
}

fn line_spans_with_selection<'a>(
    line: &'a str,
    line_start: usize,
    line_end: usize,
    sel_start: usize,
    sel_end: usize,
    highlight_bg: Color,
) -> Vec<Span<'a>> {
    let normal_style = Style::default().fg(palette::TEXT_PRIMARY);
    let sel_style = Style::default().fg(palette::TEXT_PRIMARY).bg(highlight_bg);

    // No overlap between this line and the selection
    if line_end <= sel_start || line_start >= sel_end {
        return vec![Span::styled(line, normal_style)];
    }

    let local_sel_start = sel_start.saturating_sub(line_start);
    let local_sel_end = sel_end.min(line_end).saturating_sub(line_start);

    // Build a Vec of byte offsets for each char boundary, plus one past the end.
    let mut byte_offsets: Vec<usize> = line.char_indices().map(|(i, _)| i).collect();
    byte_offsets.push(line.len());

    let b0 = byte_offsets
        .get(local_sel_start)
        .copied()
        .unwrap_or(line.len());
    let b1 = byte_offsets
        .get(local_sel_end)
        .copied()
        .unwrap_or(line.len());

    let mut spans = Vec::with_capacity(3);

    // Text before selection
    if b0 > 0 {
        spans.push(Span::styled(&line[..b0], normal_style));
    }
    // Selected text
    if b1 > b0 {
        spans.push(Span::styled(&line[b0..b1], sel_style));
    }
    // Text after selection
    if b1 < line.len() {
        spans.push(Span::styled(&line[b1..], normal_style));
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::{
        ApprovalWidget, COMPOSER_PANEL_HEIGHT, ChatWidget, ComposerWidget, Renderable,
        SlashMenuEntry, apply_detail_target_highlight, apply_selection_to_line, apply_send_flash,
        build_empty_state_lines, composer_height, composer_max_height, composer_min_input_rows,
        composer_top_padding, compute_takeover_area, cursor_row_col, layout_input,
        pad_lines_to_bottom, placeholder_visual_lines, push_command_entry,
        should_render_empty_state, slash_completion_hints, wrap_input_lines, wrap_text,
    };
    use crate::config::{ApiProvider, Config};
    use crate::localization::Locale;
    use crate::palette;
    use crate::tui::active_cell::ActiveCell;
    use crate::tui::app::{App, ComposerDensity, ToolCollapseMode, TuiOptions};
    use crate::tui::history::{GenericToolCell, HistoryCell, ToolCell, ToolStatus};
    use crate::tui::scrolling::{TranscriptLineMeta, TranscriptScroll};
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::{Color, Style},
        text::{Line, Span},
    };
    use std::path::PathBuf;
    use unicode_width::UnicodeWidthStr;

    fn create_test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-flash".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: true,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
    }

    fn buffer_text(buf: &Buffer, area: Rect) -> String {
        let mut text = String::new();
        for y in area.y..area.y.saturating_add(area.height) {
            for x in area.x..area.x.saturating_add(area.width) {
                text.push_str(buf[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    fn success_tool_cell(name: &str) -> HistoryCell {
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: name.to_string(),
            status: ToolStatus::Success,
            input_summary: Some(format!("path: {name}.txt")),
            output: Some(format!("full output from {name}")),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        }))
    }

    fn add_dense_tool_run(app: &mut App) {
        app.add_message(success_tool_cell("read_file"));
        app.add_message(success_tool_cell("list_dir"));
        app.add_message(success_tool_cell("web_search"));
    }

    #[test]
    fn send_flash_uses_original_index_map_for_collapsed_rows() {
        let history = vec![
            success_tool_cell("read_file"),
            success_tool_cell("list_dir"),
            HistoryCell::User {
                content: "sent".to_string(),
            },
        ];
        let mut lines = vec![Line::from("sent")];
        let line_meta = vec![TranscriptLineMeta::CellLine {
            cell_index: 0,
            line_in_cell: 0,
            copy_prefix_width: 0,
            copy_separator_after: crate::tui::ui_text::CopyLineSeparator::Newline,
        }];
        let original_index_map = vec![2];

        apply_send_flash(&mut lines, 0, &history, &line_meta, &original_index_map);

        assert_eq!(lines[0].spans[0].style.bg, Some(Color::Rgb(30, 40, 55)));
    }

    #[test]
    fn detail_highlight_uses_original_index_map_for_collapsed_rows() {
        let mut lines = vec![Line::from("tool group")];
        let line_meta = vec![TranscriptLineMeta::CellLine {
            cell_index: 0,
            line_in_cell: 0,
            copy_prefix_width: 0,
            copy_separator_after: crate::tui::ui_text::CopyLineSeparator::Newline,
        }];
        let original_index_map = vec![4];

        apply_detail_target_highlight(&mut lines, 0, 4, &line_meta, &original_index_map);

        assert_eq!(lines[0].spans[0].style.bg, Some(Color::Reset));
    }

    #[test]
    fn chat_widget_collapses_dense_tool_runs_by_default() {
        let mut app = create_test_app();
        app.tool_collapse_mode = ToolCollapseMode::Compact;
        app.tool_collapse_threshold = 3;
        add_dense_tool_run(&mut app);

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 8,
        };
        let mut buf = Buffer::empty(area);
        let widget = ChatWidget::new(&mut app, area);
        widget.render(area, &mut buf);
        let rendered = buffer_text(&buf, area);

        assert_eq!(app.collapsed_cell_map, vec![0]);
        assert!(
            rendered.contains("Explored 2 files, 1 search"),
            "{rendered}"
        );
        assert!(!rendered.contains("activity_group"), "{rendered}");
        assert!(
            !rendered.contains("full output from list_dir"),
            "{rendered}"
        );
    }

    #[test]
    fn chat_widget_collapses_dense_active_tool_runs_by_default() {
        let mut app = create_test_app();
        app.tool_collapse_mode = ToolCollapseMode::Compact;
        app.tool_collapse_threshold = 3;
        let active = app.active_cell.get_or_insert_with(ActiveCell::new);
        active.push_untracked(success_tool_cell("read_file"));
        active.push_untracked(success_tool_cell("list_dir"));
        active.push_untracked(success_tool_cell("web_search"));
        app.bump_active_cell_revision();

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 8,
        };
        let mut buf = Buffer::empty(area);
        let widget = ChatWidget::new(&mut app, area);
        widget.render(area, &mut buf);
        let rendered = buffer_text(&buf, area);

        assert_eq!(app.collapsed_cell_map, vec![0]);
        assert!(
            rendered.contains("Explored 2 files, 1 search"),
            "{rendered}"
        );
        assert!(!rendered.contains("activity_group"), "{rendered}");
        assert!(
            !rendered.contains("full output from list_dir"),
            "{rendered}"
        );
    }

    #[test]
    fn chat_widget_expands_dense_tool_runs_on_demand() {
        let mut app = create_test_app();
        app.tool_collapse_mode = ToolCollapseMode::Compact;
        app.tool_collapse_threshold = 3;
        add_dense_tool_run(&mut app);
        app.expanded_tool_runs.insert(0);

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 12,
        };
        let mut buf = Buffer::empty(area);
        let widget = ChatWidget::new(&mut app, area);
        widget.render(area, &mut buf);
        let rendered = buffer_text(&buf, area);

        assert_eq!(app.collapsed_cell_map, vec![0, 1, 2]);
        assert!(rendered.contains("full output from list_dir"), "{rendered}");
    }

    #[test]
    fn chat_widget_expanded_mode_leaves_dense_tool_runs_visible() {
        let mut app = create_test_app();
        app.tool_collapse_mode = ToolCollapseMode::Expanded;
        app.tool_collapse_threshold = 3;
        add_dense_tool_run(&mut app);

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 12,
        };
        let _widget = ChatWidget::new(&mut app, area);

        assert_eq!(app.collapsed_cell_map, vec![0, 1, 2]);
    }

    #[test]
    fn pad_lines_to_bottom_noop_when_already_filled() {
        let mut lines = vec![Line::from("one"), Line::from("two")];
        pad_lines_to_bottom(&mut lines, 2);
        assert_eq!(lines, vec![Line::from("one"), Line::from("two")]);
    }

    #[test]
    fn pad_lines_to_bottom_prepends_empty_lines() {
        let mut lines = vec![Line::from("one"), Line::from("two")];
        pad_lines_to_bottom(&mut lines, 5);

        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], Line::from(""));
        assert_eq!(lines[1], Line::from(""));
        assert_eq!(lines[2], Line::from(""));
        assert_eq!(lines[3], Line::from("one"));
        assert_eq!(lines[4], Line::from("two"));
    }

    #[test]
    fn pad_lines_to_bottom_noop_when_height_is_zero() {
        let mut lines = vec![Line::from("one")];
        pad_lines_to_bottom(&mut lines, 0);
        assert_eq!(lines, vec![Line::from("one")]);
    }

    // Cursor alignment tests

    #[test]
    fn cursor_basic_ascii() {
        // "hello" with cursor at various positions, width=10
        assert_eq!(cursor_row_col("hello", 0, 10), (0, 0));
        assert_eq!(cursor_row_col("hello", 3, 10), (0, 3));
        assert_eq!(cursor_row_col("hello", 5, 10), (0, 5));
    }

    #[test]
    fn cursor_at_wrap_boundary() {
        // "abcde" exactly fills width=5
        // Cursor at position 5 (after last char) should wrap to next line
        let (row, col) = cursor_row_col("abcde", 5, 5);
        assert_eq!(row, 1, "cursor at end of full line should wrap");
        assert_eq!(col, 0, "cursor should be at start of next line");
    }

    #[test]
    fn cursor_with_cjk_characters() {
        // "中" is a CJK character with width 2
        // "a中b" = 1 + 2 + 1 = 4 display width
        assert_eq!(cursor_row_col("a中b", 0, 10), (0, 0)); // before 'a'
        assert_eq!(cursor_row_col("a中b", 1, 10), (0, 1)); // after 'a', before '中'
        assert_eq!(cursor_row_col("a中b", 2, 10), (0, 3)); // after '中', before 'b'
        assert_eq!(cursor_row_col("a中b", 3, 10), (0, 4)); // after 'b'
    }

    #[test]
    fn cursor_cjk_at_wrap_boundary() {
        // width=5, input "abcd中" (4 + 2 = 6, CJK doesn't fit on line 1)
        // CJK should wrap to next line
        let lines = wrap_text("abcd中", 5);
        assert_eq!(lines, vec!["abcd", "中"]);

        // Cursor after CJK should be on row 1, col 2
        let (row, col) = cursor_row_col("abcd中", 5, 5);
        assert_eq!(row, 1);
        assert_eq!(col, 2);
    }

    #[test]
    fn cursor_with_combining_marks() {
        // "e\u0301" is 'e' with combining acute accent (é)
        // Display width is 1 (combining mark has width 0)
        let input = "e\u{0301}"; // é as e + combining acute
        assert_eq!(input.chars().count(), 2);

        // Cursor positions:
        // 0 = before 'e'
        // 1 = after 'e', before combining mark
        // 2 = after combining mark
        assert_eq!(cursor_row_col(input, 0, 10), (0, 0));
        assert_eq!(cursor_row_col(input, 1, 10), (0, 1));
        assert_eq!(cursor_row_col(input, 2, 10), (0, 1)); // combining mark has width 0
    }

    #[test]
    fn cursor_with_emoji() {
        // Many emojis are double-width
        let input = "a😀b";
        // Cursor at 2 (after emoji) should account for emoji width
        let (_row, col) = cursor_row_col(input, 2, 10);
        // Emoji width varies by system, but should be either 1 or 2
        assert!((2..=3).contains(&col), "col = {col}, expected 2 or 3");
    }

    #[test]
    fn cursor_with_emoji_zwj_sequence() {
        let input = "👨‍👩‍👧‍👦";
        let cursor = input.chars().count();
        let (row, col) = cursor_row_col(input, cursor, 10);
        assert_eq!(row, 0);
        assert_eq!(col, input.width());
    }

    #[test]
    fn cursor_with_newlines() {
        // "ab\ncd" with cursor moving through
        assert_eq!(cursor_row_col("ab\ncd", 0, 10), (0, 0)); // before 'a'
        assert_eq!(cursor_row_col("ab\ncd", 2, 10), (0, 2)); // after 'b', before '\n'
        assert_eq!(cursor_row_col("ab\ncd", 3, 10), (1, 0)); // after '\n', before 'c'
        assert_eq!(cursor_row_col("ab\ncd", 5, 10), (1, 2)); // after 'd'
    }

    #[test]
    fn wrap_input_lines_preserves_empty_lines() {
        let lines = wrap_input_lines("a\n\nb", 10);
        assert_eq!(lines, vec!["a", "", "b"]);
    }

    #[test]
    fn wrap_input_lines_trailing_newline() {
        let lines = wrap_input_lines("a\n", 10);
        assert_eq!(lines, vec!["a", ""]);
    }

    #[test]
    fn cursor_and_wrap_consistency() {
        // Ensure cursor_row_col is consistent with wrap_text
        // for various inputs
        let test_cases = vec![
            ("hello world", 5),
            ("abcdefghij", 3),
            ("中文测试", 6),
            ("a\nb\nc", 10),
        ];

        for (input, width) in test_cases {
            let lines = wrap_input_lines(input, width);
            let (cursor_row, _) = cursor_row_col(input, input.chars().count(), width);

            // Cursor at end should be on the last line (or wrapped past it)
            assert!(
                cursor_row <= lines.len(),
                "cursor_row={cursor_row} should be <= lines.len()={} for input={input:?}",
                lines.len()
            );
        }
    }

    #[test]
    fn slash_completion_hints_include_links_and_config() {
        let hints = slash_completion_hints("/", 128, &[], Locale::En, None, ApiProvider::Deepseek);
        assert!(hints.iter().any(|hint| hint.name == "/config"));
        assert!(hints.iter().any(|hint| hint.name == "/links"));
    }

    #[test]
    fn slash_completion_hints_rank_exact_alias_above_prefix_alias() {
        // `/q` should rank `/exit` (exact alias `q`) above `/clear` (alias
        // `qingping` only matches by prefix). Before #1811 the entries were
        // sorted alphabetically, so `/clear` shadowed `/exit` even though
        // the user typed the exact alias for `/exit`.
        let hints = slash_completion_hints("/q", 128, &[], Locale::En, None, ApiProvider::Deepseek);
        let names: Vec<&str> = hints.iter().map(|h| h.name.as_str()).collect();
        let exit_pos = names
            .iter()
            .position(|n| *n == "/exit")
            .expect("/exit should appear when typing /q (alias `q`)");
        let clear_pos = names
            .iter()
            .position(|n| *n == "/clear")
            .expect("/clear should still appear when typing /q (alias `qingping`)");
        assert!(
            exit_pos < clear_pos,
            "expected /exit to rank above /clear for prefix /q, got {names:?}"
        );
    }

    #[test]
    fn slash_completion_hints_keep_prefix_match_alphabetical_within_tier() {
        // Within the same rank tier (no exact-alias match), entries fall
        // back to alphabetical name order, same as the prior behavior.
        let hints =
            slash_completion_hints("/co", 128, &[], Locale::En, None, ApiProvider::Deepseek);
        let names: Vec<&str> = hints
            .iter()
            .map(|h| h.name.as_str())
            .filter(|n| n.starts_with("/co"))
            .collect();
        let sorted = {
            let mut copy = names.clone();
            copy.sort();
            copy
        };
        assert_eq!(
            names, sorted,
            "tied entries (no exact-alias match) should stay alphabetical"
        );
    }

    #[test]
    fn slash_completion_hints_exclude_set_and_deepseek_commands() {
        let hints = slash_completion_hints("/", 128, &[], Locale::En, None, ApiProvider::Deepseek);
        assert!(!hints.iter().any(|hint| hint.name == "/set"));
        assert!(!hints.iter().any(|hint| hint.name == "/codewhale"));
    }

    #[test]
    fn slash_completion_hints_use_user_command_frontmatter_description() {
        let tmp = tempfile::TempDir::new().unwrap();
        let commands_dir = tmp.path().join(".deepseek").join("commands");
        std::fs::create_dir_all(&commands_dir).unwrap();
        std::fs::write(
            commands_dir.join("git-scan.md"),
            "---\ndescription: Scan nested git repositories\n---\nscan",
        )
        .unwrap();

        let hints = slash_completion_hints(
            "/git",
            128,
            &[],
            Locale::En,
            Some(tmp.path()),
            ApiProvider::Deepseek,
        );
        let entry = hints
            .iter()
            .find(|hint| hint.name == "/git-scan")
            .expect("custom command should be present");
        assert_eq!(entry.description, "Scan nested git repositories");
    }

    #[test]
    fn slash_completion_hints_use_user_command_argument_hint() {
        let tmp = tempfile::TempDir::new().unwrap();
        let commands_dir = tmp.path().join(".deepseek").join("commands");
        std::fs::create_dir_all(&commands_dir).unwrap();
        std::fs::write(
            commands_dir.join("deploy.md"),
            "---\ndescription: Deploy target\nargument-hint: <env>\n---\ndeploy",
        )
        .unwrap();

        let hints = slash_completion_hints(
            "/deploy",
            128,
            &[],
            Locale::En,
            Some(tmp.path()),
            ApiProvider::Deepseek,
        );
        let entry = hints
            .iter()
            .find(|hint| hint.name == "/deploy")
            .expect("custom command should be present");
        assert_eq!(entry.description, "Deploy target  <env>");
    }

    #[test]
    fn review_regression_push_command_entry_uses_preloaded_user_command_frontmatter() {
        let user_commands = vec![(
            "deploy".to_string(),
            "---\ndescription: Deploy target\nargument-hint: <env>\n---\ndeploy".to_string(),
        )];
        let mut entries = Vec::new();

        push_command_entry(
            &mut entries,
            "/deploy",
            "deploy",
            "deploy",
            Locale::En,
            &user_commands,
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "/deploy");
        assert_eq!(entries[0].description, "Deploy target  <env>");
    }

    #[test]
    fn slash_completion_hints_hide_skills_from_top_level_menu() {
        let cached_skills = vec![
            ("search-files".to_string(), "Search files".to_string()),
            ("my-review".to_string(), "Review code".to_string()),
        ];
        let hints = slash_completion_hints(
            "/",
            128,
            &cached_skills,
            Locale::En,
            None,
            ApiProvider::Deepseek,
        );
        assert!(hints.iter().any(|hint| hint.name == "/skill"));
        assert!(hints.iter().any(|hint| hint.name == "/skills"));
        assert!(!hints.iter().any(|hint| hint.is_skill));
    }

    #[test]
    fn slash_completion_hints_hide_skills_from_top_level_prefix() {
        let cached_skills = vec![
            ("search-files".to_string(), "Search files".to_string()),
            ("my-review".to_string(), "Review code".to_string()),
        ];
        let hints = slash_completion_hints(
            "/se",
            128,
            &cached_skills,
            Locale::En,
            None,
            ApiProvider::Deepseek,
        );
        assert!(!hints.iter().any(|hint| hint.name == "/skill search-files"));
        assert!(!hints.iter().any(|hint| hint.name == "/skill my-review"));
    }

    #[test]
    fn slash_completion_hints_complete_skill_argument_all() {
        let cached_skills = vec![
            ("search-files".to_string(), "Search files".to_string()),
            ("my-review".to_string(), "Review code".to_string()),
        ];
        let hints = slash_completion_hints(
            "/skill ",
            128,
            &cached_skills,
            Locale::En,
            None,
            ApiProvider::Deepseek,
        );
        assert_eq!(hints.len(), 2);
        assert!(hints.iter().any(|hint| hint.name == "/skill search-files"));
        assert!(hints.iter().any(|hint| hint.name == "/skill my-review"));
        assert!(hints.iter().all(|hint| hint.is_skill));
    }

    #[test]
    fn slash_completion_hints_complete_skill_argument_prefix() {
        let cached_skills = vec![
            ("search-files".to_string(), "Search files".to_string()),
            ("my-review".to_string(), "Review code".to_string()),
        ];
        let hints = slash_completion_hints(
            "/skill my",
            128,
            &cached_skills,
            Locale::En,
            None,
            ApiProvider::Deepseek,
        );
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].name, "/skill my-review");
        assert!(hints[0].is_skill);
    }

    #[test]
    fn slash_completion_hints_complete_dollar_skill_alias_prefix() {
        let cached_skills = vec![
            ("search-files".to_string(), "Search files".to_string()),
            ("my-review".to_string(), "Review code".to_string()),
        ];
        let hints = slash_completion_hints(
            "$my",
            128,
            &cached_skills,
            Locale::En,
            None,
            ApiProvider::Deepseek,
        );
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].name, "$my-review");
        assert!(hints[0].is_skill);
    }

    #[test]
    fn slash_completion_hints_rank_dollar_skill_alias_matches() {
        let cached_skills = vec![
            ("a-my".to_string(), "Contains match".to_string()),
            ("my-review".to_string(), "Prefix match".to_string()),
            ("z-my".to_string(), "Contains match".to_string()),
        ];
        let hints = slash_completion_hints(
            "$my",
            128,
            &cached_skills,
            Locale::En,
            None,
            ApiProvider::Deepseek,
        );
        let names = hints
            .iter()
            .map(|hint| hint.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["$my-review", "$a-my", "$z-my"]);
    }

    #[test]
    fn slash_completion_hints_model_deepseek_provider_uses_bare_ids() {
        let hints =
            slash_completion_hints("/model", 128, &[], Locale::En, None, ApiProvider::Deepseek);
        let names = hints
            .iter()
            .map(|hint| hint.name.as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&"/model deepseek-v4-pro"));
        assert!(names.contains(&"/model deepseek-v4-flash"));
        assert!(!names.contains(&"/model deepseek-ai/deepseek-v4-pro"));
        assert!(!names.contains(&"/model deepseek/deepseek-v4-pro"));
    }

    #[test]
    fn slash_completion_hints_model_provider_uses_provider_specific_ids() {
        let hints =
            slash_completion_hints("/model", 128, &[], Locale::En, None, ApiProvider::NvidiaNim);
        let names = hints
            .iter()
            .map(|hint| hint.name.as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&"/model deepseek-ai/deepseek-v4-pro"));
        assert!(!names.contains(&"/model deepseek/deepseek-v4-pro"));
    }

    #[test]
    fn slash_completion_hints_model_ollama_has_no_static_remote_models() {
        let hints =
            slash_completion_hints("/model", 128, &[], Locale::En, None, ApiProvider::Ollama);
        let names = hints
            .iter()
            .map(|hint| hint.name.as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&"/model"));
        assert!(!names.contains(&"/model deepseek-v4-pro"));
        assert!(!names.contains(&"/model deepseek-v4-flash"));
        assert!(!names.contains(&"/model deepseek-coder:1.3b"));
    }

    #[test]
    fn selection_style_uses_explicit_selection_text_role() {
        let line = Line::from(Span::styled(
            "hello world",
            Style::default().fg(palette::TEXT_PRIMARY),
        ));
        let selection_style = Style::default()
            .bg(palette::SELECTION_BG)
            .fg(palette::SELECTION_TEXT);

        let styled = apply_selection_to_line(&line, 0, 5, selection_style);
        assert_eq!(styled.len(), 2);
        assert_eq!(styled[0].content.as_ref(), "hello");
        assert_eq!(styled[0].style.fg, Some(palette::SELECTION_TEXT));
        assert_eq!(styled[0].style.bg, Some(palette::SELECTION_BG));
        assert_eq!(styled[1].content.as_ref(), " world");
    }

    #[test]
    fn composer_layout_helpers_stay_consistent() {
        let input = "line one wraps nicely\nline two wraps as well";
        let width = 16;
        let available_height = 6;
        let menu_lines = 2;

        let height = composer_height(
            input,
            width,
            available_height,
            menu_lines,
            ComposerDensity::Comfortable,
            true,
        );
        let has_panel = available_height >= 3 && width >= 12;
        let chrome_height = if has_panel {
            usize::from(COMPOSER_PANEL_HEIGHT)
        } else {
            0
        };
        let content_width = if has_panel {
            usize::from(width.saturating_sub(2).max(1))
        } else {
            usize::from(width.max(1))
        };
        let input_height_budget = usize::from(height)
            .saturating_sub(menu_lines)
            .saturating_sub(chrome_height)
            .max(1);
        let (visible, cursor_row, cursor_col) = layout_input(
            input,
            input.chars().count(),
            content_width,
            input_height_budget,
        );

        assert!(visible.len().saturating_add(menu_lines) <= usize::from(height));
        assert!(!visible.is_empty());
        assert!(cursor_row < visible.len());
        assert!(cursor_col < content_width.max(1));
        assert!(height >= 5);
    }

    #[test]
    fn composer_height_prefers_panel_shape_when_space_allows() {
        let height = composer_height("", 40, 8, 0, ComposerDensity::Comfortable, true);
        assert_eq!(height, 5);
    }

    #[test]
    fn composer_height_skips_panel_chrome_when_border_disabled() {
        let with_border = composer_height("", 40, 8, 0, ComposerDensity::Comfortable, true);
        let without_border = composer_height("", 40, 8, 0, ComposerDensity::Comfortable, false);

        assert_eq!(with_border, 5);
        assert_eq!(without_border, 1);
        assert!(without_border < with_border);
    }

    #[test]
    fn composer_density_changes_min_rows_and_height_cap() {
        assert_eq!(composer_min_input_rows(ComposerDensity::Compact), 2);
        assert_eq!(composer_min_input_rows(ComposerDensity::Spacious), 4);
        assert!(
            composer_max_height(ComposerDensity::Spacious)
                > composer_max_height(ComposerDensity::Compact)
        );
    }

    #[test]
    fn empty_composer_cursor_matches_placeholder_padding() {
        let mut app = create_test_app();
        // Pin density so the test is independent of any loaded user settings.
        app.composer_density = ComposerDensity::Comfortable;
        let slash_menu_entries = Vec::<SlashMenuEntry>::new();
        let mention_menu_entries = Vec::<String>::new();
        let widget = ComposerWidget::new(&app, 5, &slash_menu_entries, &mention_menu_entries);

        // Use a wide area so the placeholder fits on one line (no wrapping).
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 5,
        };

        // inner_area: {x:1, y:1, w:38, h:3}  (borders shrink by 1 each side)
        // input_rows_budget = 3
        // placeholder_visual_lines(38) = 1  (placeholder is 22 chars, fits in 38)
        // top_padding = 3 - clamp(1, 1, 3) = 2
        // cursor_x = 0 + (1-0) + 0 = 1
        // cursor_y = 0 + (1-0) + (2+0) = 3
        assert_eq!(widget.cursor_pos(area), Some((1, 3)));
    }

    #[test]
    fn empty_composer_cursor_accounts_for_placeholder_wrapping() {
        let mut app = create_test_app();
        app.composer_density = ComposerDensity::Comfortable;
        let slash_menu_entries = Vec::<SlashMenuEntry>::new();
        let mention_menu_entries = Vec::<String>::new();
        let widget = ComposerWidget::new(&app, 5, &slash_menu_entries, &mention_menu_entries);

        // Narrow area forces the placeholder to wrap.
        let area = Rect {
            x: 0,
            y: 0,
            width: 14,
            height: 5,
        };

        // inner_area: {x:1, y:1, w:12, h:3}
        // input_rows_budget = 3
        // placeholder_visual_lines(12) = 2  ("Write a task" / " or use /.")
        // top_padding = 3 - clamp(2, 1, 3) = 1
        // cursor_x = 0 + (1-0) + 0 = 1
        // cursor_y = 0 + (1-0) + (1+0) = 2
        assert_eq!(placeholder_visual_lines(12), 2);
        assert_eq!(widget.cursor_pos(area), Some((1, 2)));
    }

    #[test]
    fn composer_border_renders_session_title() {
        let mut app = create_test_app();
        app.composer_density = ComposerDensity::Comfortable;
        app.session_title = Some("my-session".to_string());
        let slash_menu_entries = Vec::<SlashMenuEntry>::new();
        let mention_menu_entries = Vec::<String>::new();
        let widget = ComposerWidget::new(&app, 5, &slash_menu_entries, &mention_menu_entries);
        let area = Rect {
            x: 0,
            y: 0,
            width: 96,
            height: 5,
        };
        let mut buf = Buffer::empty(area);

        widget.render(area, &mut buf);
        let rendered = buffer_text(&buf, area);

        assert!(rendered.contains("Composer"));
        assert!(rendered.contains("my-session"));
    }

    #[test]
    fn composer_border_renders_active_turn_receipt() {
        let mut app = create_test_app();
        app.composer_density = ComposerDensity::Comfortable;
        app.set_receipt_text("✓ turn completed · 2 tool(s) used");
        let slash_menu_entries = Vec::<SlashMenuEntry>::new();
        let mention_menu_entries = Vec::<String>::new();
        let widget = ComposerWidget::new(&app, 5, &slash_menu_entries, &mention_menu_entries);
        let area = Rect {
            x: 0,
            y: 0,
            width: 96,
            height: 5,
        };
        let mut buf = Buffer::empty(area);

        widget.render(area, &mut buf);
        let rendered = buffer_text(&buf, area);

        assert!(rendered.contains("Composer"));
        assert!(rendered.contains("turn completed"));
        assert!(rendered.contains("tool(s) used"));
    }

    #[test]
    fn slash_menu_open_locks_composer_height_against_match_count_changes() {
        // Repro for the Windows 10 PowerShell + WSL feedback: typing
        // through a slash command shrinks the matched-entry list, which
        // used to shrink the composer height — and shrinking the
        // composer forces the chat area above to repaint every
        // keystroke.  With the height lock, the desired height returned
        // for a 5-match menu and a 1-match menu must be identical so
        // the layout stays stable for the lifetime of the slash session.
        let mut app = create_test_app();
        app.composer_density = ComposerDensity::Comfortable;
        app.input = "/skill".to_string();

        let many_matches: Vec<SlashMenuEntry> = (0..5)
            .map(|i| SlashMenuEntry {
                name: format!("/skill{i}"),
                description: String::new(),
                is_skill: false,
                alias_hint: None,
            })
            .collect();
        let one_match = vec![SlashMenuEntry {
            name: "/skill".to_string(),
            description: String::new(),
            is_skill: false,
            alias_hint: None,
        }];
        let no_matches = Vec::<SlashMenuEntry>::new();

        let widget_many = ComposerWidget::new(&app, 9, &many_matches, &[]);
        let widget_one = ComposerWidget::new(&app, 9, &one_match, &[]);
        let widget_none = ComposerWidget::new(&app, 9, &no_matches, &[]);

        // Fixed worst-case envelope while the slash menu is open.
        let height_many = widget_many.desired_height(40);
        let height_one = widget_one.desired_height(40);
        assert_eq!(
            height_many, height_one,
            "slash menu height must not jitter as the matched-entry count changes"
        );

        // Sanity: closing the slash menu (no matches) lets the panel
        // collapse back to a tight composer — we only want to lock
        // height *while* the menu is open.
        let height_none = widget_none.desired_height(40);
        assert!(
            height_none < height_many,
            "with the menu closed the composer should release the reserved rows; got {height_none} vs locked {height_many}"
        );
    }

    #[test]
    fn empty_composer_cursor_uses_full_area_when_border_disabled() {
        let mut app = create_test_app();
        app.composer_density = ComposerDensity::Comfortable;
        app.composer_border = false;
        let slash_menu_entries = Vec::<SlashMenuEntry>::new();
        let mention_menu_entries = Vec::<String>::new();
        let widget = ComposerWidget::new(&app, 3, &slash_menu_entries, &mention_menu_entries);

        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 3,
        };

        assert_eq!(widget.cursor_pos(area), Some((0, 2)));
    }

    #[test]
    fn localized_composer_placeholders_render_at_narrow_widths() {
        for locale in [Locale::Ja, Locale::ZhHans, Locale::PtBr] {
            let mut app = create_test_app();
            app.ui_locale = locale;
            app.composer_density = ComposerDensity::Comfortable;
            let slash_menu_entries = Vec::<SlashMenuEntry>::new();
            let mention_menu_entries = Vec::<String>::new();
            let widget = ComposerWidget::new(&app, 5, &slash_menu_entries, &mention_menu_entries);
            let area = Rect {
                x: 0,
                y: 0,
                width: 18,
                height: 5,
            };
            let mut buf = Buffer::empty(area);

            widget.render(area, &mut buf);
            let Some((cursor_x, cursor_y)) = widget.cursor_pos(area) else {
                panic!("localized composer should expose cursor position");
            };

            assert!(cursor_x < area.width, "{locale:?} cursor x overflow");
            assert!(cursor_y < area.height, "{locale:?} cursor y overflow");
        }
    }

    #[test]
    fn composer_top_padding_uses_clamp() {
        // content_lines=0 is clamped to 1
        assert_eq!(composer_top_padding(0, 3), 2);
        // content_lines=1
        assert_eq!(composer_top_padding(1, 3), 2);
        // content_lines=3 fills the budget
        assert_eq!(composer_top_padding(3, 3), 0);
        // content_lines > budget is clamped
        assert_eq!(composer_top_padding(5, 3), 0);
    }

    #[test]
    fn empty_state_renders_only_without_transcript_activity() {
        let mut app = create_test_app();
        assert!(should_render_empty_state(&app));
        app.add_message(crate::tui::history::HistoryCell::User {
            content: "hello".to_string(),
        });
        assert!(!should_render_empty_state(&app));
    }

    #[test]
    fn empty_state_shows_startup_context() {
        let mut app = create_test_app();
        app.workspace = PathBuf::from("/tmp/codewhale-test-workspace");
        app.model = "deepseek-v4-pro".to_string();

        let lines = build_empty_state_lines(&app, Rect::new(0, 0, 100, 20));
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains(&format!(">_ codewhale (v{})", env!("CARGO_PKG_VERSION"))));
        assert!(rendered.contains("model: deepseek-v4-pro  /model to switch"));
        assert!(rendered.contains("directory: /tmp/codewhale-test-workspace"));
    }

    /// Probe: confirm `cell.lines_with_motion` returns no Line whose total
    /// visual width exceeds the requested area width, even for pathological
    /// long single-line tool results.
    #[test]
    fn long_tool_result_lines_fit_requested_width() {
        let cell = HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "todo_write".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("items: <2 items>".to_string()),
            output: Some("hello world ".repeat(420)),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        }));
        for width in [40u16, 80, 111, 165] {
            let lines = cell.lines(width);
            for (idx, line) in lines.iter().enumerate() {
                let visual: usize = line
                    .spans
                    .iter()
                    .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                    .sum();
                // Card-rail prefix (╭/│/╰ + space) adds 2 chars.
                let rail_adjust = if line.spans.first().is_some_and(|s| {
                    let c = s.content.as_ref();
                    c == "\u{256D} " || c == "\u{2502} " || c == "\u{2570} "
                }) {
                    2usize
                } else {
                    0
                };
                assert!(
                    visual.saturating_sub(rail_adjust) <= usize::from(width),
                    "line {idx} at width {width} has visual width {visual} > {width}"
                );
            }
        }
    }

    /// Regression: a long single-line tool result must not write any cells
    /// outside the chat content area (issue #36 — sidebar gutter bleed).
    ///
    /// We render `ChatWidget` into a buffer that is wider than the chat area
    /// (simulating the sidebar split) and assert every cell to the right of
    /// `chat_area` is still the default empty cell.
    #[test]
    fn chat_widget_does_not_bleed_into_sidebar_for_long_tool_result() {
        // Reproduces the actual `todo_write` output shape: a status line,
        // a newline, then a pretty-printed JSON payload with long string
        // values. Run at several widths since the leak in the issue was
        // observed at ~165 cols.
        let cases: Vec<(u16, u16)> = vec![(80, 50), (120, 80), (165, 111), (200, 140)];
        for (total_width, chat_width) in cases {
            let mut app = create_test_app();
            let long_value: String = "hello world ".repeat(420);
            let json_payload = format!(
                "{{\n  \"items\": [\n    {{ \"id\": 1, \"content\": \"{long_value}\", \"status\": \"pending\" }}\n  ]\n}}"
            );
            let output = format!("Todo list updated (1 items, 0% complete)\n{json_payload}");
            app.add_message(HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: "todo_write".to_string(),
                status: ToolStatus::Success,
                input_summary: Some("todos: <1 items>".to_string()),
                output: Some(output),
                prompts: None,
                spillover_path: None,
                output_summary: None,
                is_diff: false,
            })));

            let height: u16 = 30;
            let chat_area = Rect {
                x: 0,
                y: 0,
                width: chat_width,
                height,
            };
            let full_area = Rect {
                x: 0,
                y: 0,
                width: total_width,
                height,
            };
            let mut buf = Buffer::empty(full_area);

            let widget = ChatWidget::new(&mut app, chat_area);
            widget.render(chat_area, &mut buf);

            // Every cell outside chat_area should remain at default. If the
            // widget bled, we'll see leftover symbols.
            let default_symbol = " ";
            for y in 0..height {
                for x in chat_width..total_width {
                    let cell = &buf[(x, y)];
                    let sym = cell.symbol();
                    assert!(
                        sym == default_symbol || sym.is_empty(),
                        "[{total_width}x{height}, chat={chat_width}] cell ({x},{y}) leaked content {sym:?} outside chat_area"
                    );
                }
            }
        }
    }

    #[test]
    fn chat_widget_uses_configured_surface_background() {
        let mut app = create_test_app();
        let custom = ratatui::style::Color::Rgb(26, 27, 38);
        app.ui_theme = app.ui_theme.with_background_color(custom);
        app.add_message(HistoryCell::Assistant {
            content: "ready".to_string(),
            streaming: false,
        });

        let area = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 5,
        };
        let mut buf = Buffer::empty(area);
        let widget = ChatWidget::new(&mut app, area);
        widget.render(area, &mut buf);

        assert_eq!(buf[(area.x, area.y)].bg, custom);
        assert_eq!(
            buf[(area.x + area.width - 1, area.y + area.height - 1)].bg,
            custom
        );
    }

    #[test]
    fn chat_widget_does_not_render_turn_receipt_as_transcript_content() {
        let mut app = create_test_app();
        for i in 0..8 {
            app.add_message(HistoryCell::Assistant {
                content: format!("assistant line {i}"),
                streaming: false,
            });
        }
        app.set_receipt_text("✓ turn completed · 2 tool(s) used");

        let area = Rect {
            x: 0,
            y: 0,
            width: 48,
            height: 6,
        };
        let mut buf = Buffer::empty(area);
        let widget = ChatWidget::new(&mut app, area);
        widget.render(area, &mut buf);
        let rendered = buffer_text(&buf, area);

        assert!(!rendered.contains("turn completed"));
        assert!(
            rendered.contains("assistant line 7"),
            "receipt should not displace the latest transcript line: {rendered:?}"
        );
    }

    /// Regression: when the transcript scrollbar is visible, the rightmost
    /// content column must remain readable (the scrollbar gets its own
    /// 1-column gutter rather than overdrawing chat content).
    #[test]
    fn chat_widget_reserves_scrollbar_gutter_when_scrollbar_visible() {
        let mut app = create_test_app();
        // Many short messages → forces the scrollbar to be visible.
        for i in 0..200 {
            app.add_message(HistoryCell::User {
                content: format!("user message {i}"),
            });
        }

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 8,
        };
        let mut buf = Buffer::empty(area);
        let widget = ChatWidget::new(&mut app, area);
        widget.render(area, &mut buf);

        // The rightmost column should host the scrollbar track/thumb.
        // The penultimate column should still hold normal content (a digit,
        // letter, or space — never the scrollbar glyph).
        let scrollbar_track = "│";
        let scrollbar_thumb = "┃";
        let mut scrollbar_seen = false;
        for y in 0..area.height {
            let last = buf[(area.width - 1, y)].symbol();
            let penult = buf[(area.width - 2, y)].symbol();
            if last == scrollbar_track || last == scrollbar_thumb {
                scrollbar_seen = true;
            }
            assert!(
                penult != scrollbar_track && penult != scrollbar_thumb,
                "scrollbar leaked into column {} (cell {:?}) at row {y}",
                area.width - 2,
                penult
            );
        }
        assert!(
            scrollbar_seen,
            "scrollbar should be visible for a long history"
        );
    }

    #[test]
    fn chat_widget_shows_jump_to_latest_button_when_scrolled_up() {
        let mut app = create_test_app();
        app.use_mouse_capture = true;
        for i in 0..80 {
            app.add_message(HistoryCell::User {
                content: format!("user message {i}"),
            });
        }
        app.viewport.transcript_scroll = TranscriptScroll::at_line(0);

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 8,
        };
        let mut buf = Buffer::empty(area);
        let widget = ChatWidget::new(&mut app, area);
        widget.render(area, &mut buf);

        let button = app
            .viewport
            .jump_to_latest_button_area
            .expect("button appears when transcript is not at tail");
        assert_eq!(button.width, 3);
        assert_eq!(button.height, 3);
        assert_eq!(buf[(button.x + 1, button.y + 1)].symbol(), "↓");
    }

    #[test]
    fn chat_widget_uses_light_theme_scroll_chrome() {
        let mut app = create_test_app();
        app.ui_theme = palette::LIGHT_UI_THEME;
        app.use_mouse_capture = true;
        for i in 0..120 {
            app.add_message(HistoryCell::User {
                content: format!("user message {i}"),
            });
        }
        app.viewport.transcript_scroll = TranscriptScroll::at_line(0);

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 8,
        };
        let mut buf = Buffer::empty(area);
        let widget = ChatWidget::new(&mut app, area);
        widget.render(area, &mut buf);

        let mut saw_track = false;
        let mut saw_thumb = false;
        for y in 0..area.height {
            let cell = &buf[(area.width - 1, y)];
            match cell.symbol() {
                "│" => {
                    saw_track = true;
                    assert_eq!(cell.fg, palette::LIGHT_UI_THEME.border);
                }
                "┃" => {
                    saw_thumb = true;
                    assert_eq!(cell.fg, palette::LIGHT_UI_THEME.status_working);
                }
                _ => {}
            }
        }
        assert!(saw_track, "scrollbar track should render");
        assert!(saw_thumb, "scrollbar thumb should render");

        let button = app
            .viewport
            .jump_to_latest_button_area
            .expect("button appears when transcript is not at tail");
        assert_eq!(
            buf[(button.x + 1, button.y + 1)].fg,
            palette::LIGHT_UI_THEME.status_working
        );
    }

    #[test]
    fn chat_widget_hides_jump_to_latest_button_at_tail() {
        let mut app = create_test_app();
        app.use_mouse_capture = true;
        for i in 0..80 {
            app.add_message(HistoryCell::User {
                content: format!("user message {i}"),
            });
        }
        app.viewport.transcript_scroll = TranscriptScroll::to_bottom();

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 8,
        };
        let _widget = ChatWidget::new(&mut app, area);
        assert!(
            app.viewport.jump_to_latest_button_area.is_none(),
            "button should hide while following the live tail"
        );
        assert!(app.viewport.transcript_scroll.is_at_tail());
    }

    /// Regression for issue #582: a resize event arriving while the
    /// engine is in `CoherenceState::RefreshingContext` (i.e. running
    /// a compaction summary call) must NOT leave the chat widget with
    /// an empty viewport. The user-reported symptom on Windows
    /// PowerShell is that the screen turns black on the maximize→
    /// windowed transition during a long task; the post-resize render
    /// must produce a populated frame regardless of the active
    /// coherence intervention. Pins the invariant from the renderer
    /// side; the actual ConHost size-stale fix lives in
    /// `tui::ui::run_tui` (the `Event::Resize` handler now forwards
    /// the event-reported dimensions to ratatui's viewport before the
    /// redraw).
    #[test]
    fn chat_widget_renders_cleanly_after_resize_during_refreshing_context() {
        use crate::core::coherence::CoherenceState;

        let mut app = create_test_app();
        for i in 0..30 {
            app.add_message(HistoryCell::User {
                content: format!("user message {i} during a long-running task"),
            });
        }

        // Pretend the engine is mid-compaction when the resize arrives.
        app.coherence_state = CoherenceState::RefreshingContext;

        // Drive the same shrink-then-grow cycle that maximize→windowed
        // transitions produce on Windows.
        for (width, height) in [(140u16, 40u16), (90, 28), (60, 20), (140, 40)] {
            app.handle_resize(width, height);
            let area = Rect {
                x: 0,
                y: 0,
                width,
                height,
            };
            let mut buf = Buffer::empty(area);
            let widget = ChatWidget::new(&mut app, area);
            widget.render(area, &mut buf);

            let mut non_empty = 0usize;
            for y in 0..height {
                for x in 0..width {
                    let sym = buf[(x, y)].symbol();
                    if sym != " " && !sym.is_empty() {
                        non_empty += 1;
                    }
                }
            }
            assert!(
                non_empty > 0,
                "resize-during-RefreshingContext at {width}x{height} produced an empty buffer; \
                 render path must not gate on coherence state (#582)"
            );
        }

        // The engine's coherence_state must survive a resize — it is
        // the engine's runtime decision, not a render-loop concern.
        // A future regression that bounced the state to `Healthy` on
        // resize would silently drop the "refreshing context" footer
        // chip while compaction is still in flight.
        assert_eq!(
            app.coherence_state,
            CoherenceState::RefreshingContext,
            "resize must not mutate engine-owned coherence_state"
        );
    }

    #[test]
    fn approval_takeover_clamps_to_short_terminal_height() {
        let request = crate::tui::approval::ApprovalRequest::new(
            "approval-1",
            "exec_shell",
            "Run git commit",
            &serde_json::json!({ "command": "git commit -m fix" }),
            "exec_shell:git commit",
        );
        let view = crate::tui::approval::ApprovalView::new(request.clone());
        let widget = ApprovalWidget::new(&request, &view);

        for area in [Rect::new(0, 0, 162, 17), Rect::new(0, 0, 39, 17)] {
            let card_area = compute_takeover_area(area);
            assert!(card_area.x >= area.x);
            assert!(card_area.y >= area.y);
            assert!(card_area.right() <= area.right());
            assert!(card_area.bottom() <= area.bottom());

            let mut buf = Buffer::empty(area);
            widget.render(area, &mut buf);
        }
    }

    #[test]
    fn approval_selected_destructive_option_uses_contrasting_highlight() {
        let request = crate::tui::approval::ApprovalRequest::new(
            "approval-1",
            "exec_shell",
            "Run git commit",
            &serde_json::json!({ "command": "git commit -m fix" }),
            "exec_shell:git commit",
        );
        let view = crate::tui::approval::ApprovalView::new(request.clone());
        let widget = ApprovalWidget::new(&request, &view);
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);

        widget.render(area, &mut buf);

        let selected_row = (area.y..area.y.saturating_add(area.height))
            .find(|&y| {
                (area.x..area.x.saturating_add(area.width))
                    .any(|x| buf[(x, y)].bg == palette::SELECTION_BG)
            })
            .expect("selected approval row should use selection background");
        let highlighted_cells = (area.x..area.x.saturating_add(area.width))
            .filter(|&x| {
                let cell = &buf[(x, selected_row)];
                !cell.symbol().trim().is_empty()
                    && cell.bg == palette::SELECTION_BG
                    && cell.fg == palette::SELECTION_TEXT
            })
            .count();

        assert!(
            highlighted_cells >= 4,
            "selected destructive option should render visible selection text"
        );
    }

    #[test]
    fn approval_shell_command_detects_printf_write_file_preview() {
        let request = crate::tui::approval::ApprovalRequest::new(
            "approval-1",
            "exec_shell",
            "Run shell command",
            &serde_json::json!({
                "command": "printf '%s\\n' 'alpha' 'beta' > src/generated.txt",
                "cwd": "/tmp/project",
            }),
            "exec_shell:printf",
        );
        let view = crate::tui::approval::ApprovalView::new(request.clone());
        let widget = ApprovalWidget::new(&request, &view);
        let area = Rect::new(0, 0, 110, 32);
        let mut buf = Buffer::empty(area);

        widget.render(area, &mut buf);
        let rendered = buffer_text(&buf, area);

        assert!(rendered.contains("Command:"), "{rendered}");
        assert!(
            rendered.contains("printf > src/generated.txt"),
            "{rendered}"
        );
        assert!(rendered.contains("alpha"), "{rendered}");
        assert!(rendered.contains("beta"), "{rendered}");
        assert!(rendered.contains("Dir"), "{rendered}");
        assert!(rendered.contains("/tmp/project"), "{rendered}");
    }

    #[test]
    fn approval_intent_summary_still_renders_with_shell_details() {
        let request = crate::tui::approval::ApprovalRequest::new_with_intent(
            "approval-1",
            "exec_shell",
            "Run shell command",
            &serde_json::json!({
                "command": "cargo build || echo fallback",
                "cwd": "/tmp/project",
            }),
            "exec_shell:cargo",
            Some("Need to verify the fallback build path before editing files."),
        );
        let view = crate::tui::approval::ApprovalView::new(request.clone());
        let widget = ApprovalWidget::new(&request, &view);
        let area = Rect::new(0, 0, 120, 34);
        let mut buf = Buffer::empty(area);

        widget.render(area, &mut buf);
        let rendered = buffer_text(&buf, area);

        assert!(rendered.contains("Intent:"), "{rendered}");
        assert!(rendered.contains("fallback build path"), "{rendered}");
        assert!(rendered.contains("Command:"), "{rendered}");
        assert!(rendered.contains("cargo build ||"), "{rendered}");
        assert!(rendered.contains("echo fallback"), "{rendered}");
    }

    /// Regression for issue #65: after `App::handle_resize`, the chat widget
    /// must produce a clean render at the new width — no stale wrapping,
    /// no panic, no content exceeding the requested width. Cycling through
    /// several widths (shrinks and grows) flushes any cached layout that
    /// fails to invalidate on resize.
    #[test]
    fn chat_widget_renders_cleanly_after_resize_cycle() {
        let mut app = create_test_app();
        // Add some long content that wraps differently at different widths.
        for i in 0..40 {
            app.add_message(HistoryCell::User {
                content: format!("user message {i} with enough text to wrap at 30 columns easily"),
            });
        }

        let widths_to_cycle = [120u16, 80, 40, 60, 100, 30];
        let height: u16 = 20;
        for width in widths_to_cycle {
            // Caller-side: simulate the resize handler invalidating caches.
            app.handle_resize(width, height);
            let area = Rect {
                x: 0,
                y: 0,
                width,
                height,
            };
            let mut buf = Buffer::empty(area);
            let widget = ChatWidget::new(&mut app, area);
            widget.render(area, &mut buf);

            // The render must produce at least some non-empty content for a
            // populated history at any reasonable width. This catches a class
            // of resize regressions where stale layout state leaves a blank
            // viewport after a width change.
            let mut non_empty = 0usize;
            for y in 0..height {
                for x in 0..width {
                    let sym = buf[(x, y)].symbol();
                    if sym != " " && !sym.is_empty() {
                        non_empty += 1;
                    }
                }
            }
            assert!(
                non_empty > 0,
                "render at {width}x{height} produced an empty buffer after resize"
            );
        }
    }

    /// Regression for issue #65: the transcript view cache must invalidate
    /// when width changes, so the same `App.history` re-wraps to the new
    /// width on the very next `ChatWidget::new` call.
    #[test]
    fn transcript_cache_invalidates_on_width_change() {
        let mut app = create_test_app();
        for i in 0..10 {
            app.add_message(HistoryCell::User {
                content: format!("a fairly long user message number {i} that needs to wrap"),
            });
        }

        let area_wide = Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 20,
        };
        let area_narrow = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 20,
        };
        let mut buf_wide = Buffer::empty(area_wide);
        let widget_wide = ChatWidget::new(&mut app, area_wide);
        widget_wide.render(area_wide, &mut buf_wide);
        let wide_total_lines = app.viewport.transcript_cache.total_lines();

        // Without an explicit resize call, just shrinking the render area
        // should still trigger a cache rebuild because the cache keys on width.
        let mut buf_narrow = Buffer::empty(area_narrow);
        let widget_narrow = ChatWidget::new(&mut app, area_narrow);
        widget_narrow.render(area_narrow, &mut buf_narrow);
        let narrow_total_lines = app.viewport.transcript_cache.total_lines();

        assert!(
            narrow_total_lines > wide_total_lines,
            "narrow render should produce more wrapped lines (got {narrow_total_lines}, wide={wide_total_lines})"
        );
    }

    /// Issue #78 — perf bench for transcript scroll lag.
    ///
    /// Builds a 5000-entry history (mix of user / assistant / a few tool
    /// cells), then times `ChatWidget::new` at scroll offsets 0, 100, 500,
    /// and 2000 lines from the tail. The first call after history mutation
    /// pays the wrap cost; subsequent calls at different offsets should hit
    /// the per-cell cache and be ~constant time regardless of offset.
    ///
    /// Run with: `cargo test -p codewhale-tui --release bench_transcript_scroll
    /// -- --ignored --nocapture`
    // Perf bench prints timing rows to stdout — runs in `cargo test`,
    // never inside the TUI alt-screen.
    #[allow(clippy::print_stdout)]
    #[test]
    #[ignore = "perf bench; run with --release"]
    fn bench_transcript_scroll_5000_messages() {
        use std::time::Instant;

        let mut app = create_test_app();
        // 5000 cells: alternating user / assistant with realistic-ish bodies
        // so wrapping cost is non-trivial. Every 50th cell is a (small)
        // generic tool cell, mirroring real transcripts.
        for i in 0..5000usize {
            let cell = if i % 50 == 49 {
                HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                    name: "grep_files".to_string(),
                    status: ToolStatus::Success,
                    input_summary: Some(format!("query: hit-{i}")),
                    output: Some(format!("found 12 matches in cell-{i}")),
                    prompts: None,
                    spillover_path: None,
                    output_summary: None,
                    is_diff: false,
                }))
            } else if i % 2 == 0 {
                HistoryCell::User {
                    content: format!(
                        "user message {i}: please review the changes in src/foo/bar.rs and \
                         tell me whether the new error handling looks reasonable"
                    ),
                }
            } else {
                HistoryCell::Assistant {
                    content: format!(
                        "Sure — looking at src/foo/bar.rs in cell {i}, the new error \
                         handling wraps each fallible call in `?` and propagates a \
                         typed `FooError`. That looks fine, but consider whether the \
                         `Display` impl needs to redact the inner path."
                    ),
                    streaming: false,
                }
            };
            app.add_message(cell);
        }

        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 30,
        };

        // Warm-up: first call after a full history build pays the wrap cost
        // for every cell. We don't time this — it's amortized across the
        // session and is not the user-visible problem.
        let _ = ChatWidget::new(&mut app, area);

        let visible = area.height as usize;
        // For each scroll target, snap the scroll position there and measure
        // a fresh ChatWidget::new(). The cache should hit for all unchanged
        // cells, so the time should be roughly constant regardless of
        // offset.
        for offset_from_tail in [0usize, 100, 500, 2000] {
            let total = app.viewport.transcript_cache.total_lines();
            let max_start = total.saturating_sub(visible);
            let target = max_start.saturating_sub(offset_from_tail);
            app.viewport.transcript_scroll =
                crate::tui::scrolling::TranscriptScroll::at_line(target);

            let iters: u32 = 10;
            let start = Instant::now();
            for _ in 0..iters {
                let _ = ChatWidget::new(&mut app, area);
            }
            let elapsed = start.elapsed();
            let per_call_us = elapsed.as_micros() / u128::from(iters);
            println!(
                "[bench_transcript_scroll] offset={offset_from_tail:>5} \
                 per_render={per_call_us:>6} \u{3bc}s  ({:>3} ms / {iters} iters)",
                elapsed.as_millis()
            );
        }

        // Streaming-delta scenario: append one assistant cell at the tail
        // and time a render. The cache should re-render only the new cell,
        // NOT every cell — even at deep scroll.
        for offset_from_tail in [0usize, 2000] {
            let total = app.viewport.transcript_cache.total_lines();
            let max_start = total.saturating_sub(visible);
            let target = max_start.saturating_sub(offset_from_tail);
            app.viewport.transcript_scroll =
                crate::tui::scrolling::TranscriptScroll::at_line(target);

            let iters: u32 = 10;
            let start = Instant::now();
            for i in 0..iters {
                app.add_message(HistoryCell::Assistant {
                    content: format!("delta {i}"),
                    streaming: false,
                });
                let _ = ChatWidget::new(&mut app, area);
            }
            let elapsed = start.elapsed();
            let per_call_us = elapsed.as_micros() / u128::from(iters);
            println!(
                "[bench_transcript_scroll] streaming offset={offset_from_tail:>5} \
                 per_render={per_call_us:>6} \u{3bc}s  ({:>3} ms / {iters} iters)",
                elapsed.as_millis()
            );
        }
    }

    // ── Ghost-text prompt suggestion rendering ────────────────────────

    #[test]
    fn ghost_text_renders_when_suggestion_set_and_input_empty() {
        let mut app = create_test_app();
        app.prompt_suggestion = Some("What about error handling?".to_string());
        let slash_menu_entries = Vec::<SlashMenuEntry>::new();
        let mention_menu_entries = Vec::<String>::new();
        let widget = ComposerWidget::new(&app, 5, &slash_menu_entries, &mention_menu_entries);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        };
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let rendered: String = buf
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(
            rendered.contains("What about error handling?"),
            "ghost text should render the suggestion. Got: {rendered}"
        );
    }

    #[test]
    fn ghost_text_hidden_when_input_not_empty() {
        let mut app = create_test_app();
        app.prompt_suggestion = Some("A suggestion".to_string());
        app.input = "hello".to_string();
        app.cursor_position = 5;
        let slash_menu_entries = Vec::<SlashMenuEntry>::new();
        let mention_menu_entries = Vec::<String>::new();
        let widget = ComposerWidget::new(&app, 5, &slash_menu_entries, &mention_menu_entries);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        };
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let has_suggestion = buf
            .content
            .iter()
            .any(|c| c.symbol().contains("A suggestion"));
        assert!(
            !has_suggestion,
            "suggestion should not render when input is non-empty"
        );
    }

    #[test]
    fn ghost_text_hidden_when_no_suggestion() {
        let mut app = create_test_app();
        app.prompt_suggestion = None;
        let slash_menu_entries = Vec::<SlashMenuEntry>::new();
        let mention_menu_entries = Vec::<String>::new();
        let widget = ComposerWidget::new(&app, 5, &slash_menu_entries, &mention_menu_entries);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        };
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        // When no suggestion and input is empty, placeholder text should appear
        // instead. The exact placeholder text is locale-dependent, so we check
        // that the suggestion text is NOT present.
        let has_placeholder_like_text = buf.content.iter().any(|c| !c.symbol().trim().is_empty());
        assert!(
            has_placeholder_like_text,
            "some non-empty text should render as placeholder"
        );
    }
}
