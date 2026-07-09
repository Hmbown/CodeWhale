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
pub mod workflow_panel;

pub use footer::{
    FooterProps, FooterToast, FooterWidget, footer_agents_chip, footer_shell_label_chip,
    footer_working_label,
};
pub use header::{HeaderData, HeaderWidget, header_status_indicator_frame};
pub use renderable::Renderable;

use std::borrow::Cow;
use std::collections::HashSet;
use std::time::Duration;

use crate::commands;
#[cfg(test)]
use crate::config::ApiProvider;
use crate::localization::{Locale, MessageId, tr};
use crate::palette;
#[cfg(test)]
use crate::provider_lake::all_catalog_models_for_provider;
use crate::tui::app::{App, AppMode, ComposerDensity, VimMode};
use crate::tui::approval::{
    ApprovalRequest, ApprovalView, ElevationOption, ElevationRequest, RiskLevel, ToolCategory,
};
use crate::tui::history::{GenericToolCell, HistoryCell, ToolCell, ToolRun, ToolStatus};
use crate::tui::scrolling::TranscriptLineMeta;
use crate::tui::ui_text::{char_display_width, text_display_width};
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
    ocean_field: Option<OceanField>,
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

#[derive(Debug, Clone, Copy)]
struct OceanField {
    elapsed_ms: u128,
    phase: crate::tui::motion::OceanPhase,
    ambient_fish: bool,
    scatter_seed: u64,
}

impl ChatWidget {
    pub fn new(app: &mut App, area: Rect) -> Self {
        let content_area = area;
        // Ocean completion surface: one-shot lighten of the transcript field
        // on working→done (gated inside completion_surface_field_color).
        let background = app.completion_surface_field_color(app.ui_theme.surface_bg);
        let render_empty_state = should_render_empty_state(app);
        let ocean_field = ocean_field_for_app(app, render_empty_state);
        let scroll_track = app.ui_theme.border;
        let scroll_thumb = app.ui_theme.status_working;
        let jump_border = app.ui_theme.border;
        let jump_arrow = app.ui_theme.status_working;
        let visible_lines = content_area.height as usize;
        let render_options = app.transcript_render_options();

        if render_empty_state {
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
                ocean_field,
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
            let mut settle_progress: Vec<f32> =
                Vec::with_capacity(app.history.len() + active_entries.len());
            for (idx, rev) in app.history_revisions.iter().copied().enumerate() {
                let settle_key = app.settle_frame_key_at(idx);
                // Bake settle frame into the revision so mid-settle cells
                // re-render each cadence tick without invalidating settled cells.
                cell_revisions.push(rev.wrapping_add(settle_key.wrapping_mul(1_000_003)));
                settle_progress.push(app.receipt_settle_progress_at(idx));
            }
            if !active_entries.is_empty() {
                let active_rev = app.active_cell_revision;
                for i in 0..active_entries.len() {
                    let salt = (i as u64).wrapping_add(1);
                    cell_revisions.push(
                        active_rev
                            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                            .wrapping_add(salt),
                    );
                    // Active (in-flight) cells are not settling receipts.
                    settle_progress.push(1.0);
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
                Some(&settle_progress),
            );
        } else {
            // Slow path: clone non-collapsed cells into filtered vecs so
            // collapsed cells are excluded from rendering. Build the
            // filtered→original index mapping.
            let mut filtered_cells: Vec<HistoryCell> =
                Vec::with_capacity(history_len + active_entries.len());
            let mut filtered_revs: Vec<u64> =
                Vec::with_capacity(history_len + active_entries.len());
            let mut filtered_settle: Vec<f32> =
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
                    filtered_settle.push(app.receipt_settle_progress_at(idx));
                    filtered_to_original.push(idx);
                    continue;
                }
                filtered_cells.push(cell.clone());
                let settle_key = app.settle_frame_key_at(idx);
                filtered_revs.push(
                    app.history_revisions[idx].wrapping_add(settle_key.wrapping_mul(1_000_003)),
                );
                filtered_settle.push(app.receipt_settle_progress_at(idx));
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
                        filtered_settle.push(1.0);
                        filtered_to_original.push(original_idx);
                        continue;
                    }
                    filtered_cells.push(cell.clone());
                    let salt = (i as u64).wrapping_add(1);
                    filtered_revs.push(active_entry_revision(active_rev, salt));
                    filtered_settle.push(1.0);
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
                Some(&filtered_settle),
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
            ocean_field,
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
        render_ocean_field_empty_cells(area, buf, self.background, self.ocean_field, &self.lines);

        // #3029: the transcript carries OSC 8 hyperlinks in-band inside span
        // content. Scan the rendered buffer for those payloads, blank the
        // payload cells (so no cell ever holds `\x1b`/`]8;;` — fixes the
        // column-drift corruption), and publish the recovered link regions
        // for ColorCompatBackend::draw to re-emit out-of-band. This is the
        // main transcript surface; the live-transcript overlay appends its
        // own regions separately. Replaces the frame buffer each render.
        let regions = crate::tui::osc8::extract_buffer_link_regions(buf, area);
        crate::tui::osc8::set_frame_links(regions);

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

fn ocean_field_for_app(app: &App, render_empty_state: bool) -> Option<OceanField> {
    if app.low_motion || !app.fancy_animations {
        return None;
    }
    let phase = if app.ocean_field_waiting_on_user() {
        crate::tui::motion::OceanPhase::Waiting
    } else if app.is_loading || app.is_compacting || app.is_purging || app.turn_started_at.is_some()
    {
        crate::tui::motion::OceanPhase::Working
    } else if app.completion_surface_started_at.is_some() {
        crate::tui::motion::OceanPhase::Done
    } else {
        crate::tui::motion::OceanPhase::Idle
    };
    Some(OceanField {
        elapsed_ms: app.ocean_field_started_at.elapsed().as_millis(),
        phase,
        ambient_fish: render_empty_state && matches!(phase, crate::tui::motion::OceanPhase::Idle),
        scatter_seed: app
            .history_version
            .wrapping_add(app.input.len() as u64)
            .wrapping_add(app.workspace.as_os_str().len() as u64),
    })
}

fn render_ocean_field_empty_cells(
    area: Rect,
    buf: &mut Buffer,
    base: Color,
    field: Option<OceanField>,
    lines: &[Line<'static>],
) {
    let Some(field) = field else {
        return;
    };
    for y in area.top()..area.bottom() {
        let local_y = y.saturating_sub(area.y);
        let occupied_width = lines
            .get(usize::from(local_y))
            .map(|line| line_display_width(line).min(usize::from(area.width)))
            .unwrap_or(0);
        let start_x = area.x.saturating_add(occupied_width as u16);
        let row_bg = crate::tui::motion::ocean_field_color(
            base,
            local_y,
            area.height,
            field.elapsed_ms,
            field.phase,
            false,
            true,
        );
        for x in start_x..area.right() {
            let local_x = x.saturating_sub(area.x);
            let cell = &mut buf[(x, y)];
            cell.set_symbol(" ")
                .set_style(Style::default().fg(palette::TEXT_DIM).bg(row_bg));
            if field.ambient_fish
                && let Some(symbol) = crate::tui::motion::ambient_fish_symbol(
                    local_x,
                    local_y,
                    area.width,
                    area.height,
                    field.elapsed_ms,
                    field.scatter_seed,
                    false,
                    true,
                )
            {
                cell.set_symbol(symbol)
                    .set_style(Style::default().fg(palette::WHALE_INFO).bg(row_bg));
            }
        }
    }
}

fn line_display_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
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
            AppMode::Agent | AppMode::Auto | AppMode::Yolo => palette::MODE_AGENT,
            AppMode::Plan => palette::MODE_PLAN,
            AppMode::Operate => palette::MODE_OPERATE,
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

        // Use the extended version that also returns character indices to avoid
        // redundant wrapping when rendering text selections (issue #3909).
        let (visible_lines, _cursor_row, _cursor_col, _scroll_offset, visible_char_indices) =
            layout_input_with_scroll_and_char_indices(
                input_text,
                input_cursor,
                content_width,
                input_rows_budget,
            );
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
                                palette::WHALE_INFO,
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
                                format!(
                                    "↵ queue ({} waiting, double-↵ to steer)",
                                    queue_count.saturating_add(1)
                                )
                            } else {
                                "↵ queue (double-↵ to steer)".to_string()
                            };
                            (Some(label), palette::TEXT_MUTED)
                        }
                    }
                    // Steer reached via double-tap Enter or Ctrl+Enter override.
                    SubmitDisposition::Steer => {
                        (Some("↵ steering".to_string()), palette::WHALE_INFO)
                    }
                    SubmitDisposition::QueueFollowUp => (
                        Some("↵ queued (double-↵ to steer)".to_string()),
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
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .style(background);
            if self.app.is_history_search_active() || is_draft_mode {
                block = if self.app.is_history_search_active() {
                    block.title(Line::from(Span::styled(
                        self.app
                            .tr(crate::localization::MessageId::HistorySearchTitle),
                        Style::default().fg(palette::TEXT_MUTED),
                    )))
                } else {
                    block.title(Line::from(Span::styled(
                        "Draft",
                        Style::default().fg(palette::TEXT_MUTED),
                    )))
                };
            }
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
            input_lines.push(Line::from(""));
            if input_rows_budget > 1 {
                let (placeholder, style): (Cow<'_, str>, Style) = if let Some(ref suggestion) =
                    self.app.prompt_suggestion
                    && !self.app.is_history_search_active()
                {
                    (
                        Cow::Borrowed(suggestion.as_str()),
                        Style::default().fg(palette::TEXT_HINT),
                    )
                } else {
                    (
                        composer_empty_hint_text(self.app),
                        Style::default().fg(palette::TEXT_MUTED).italic(),
                    )
                };
                input_lines.push(Line::from(Span::styled(placeholder, style)));
            }
        } else if let Some((sel_start, sel_end)) = self.app.selection_range() {
            // Use the character indices we already computed during layout
            // to avoid redundant wrapping (issue #3909).
            let line_ranges: Vec<(usize, usize)> = visible_char_indices
                .iter()
                .map(|(start, text)| (*start, *start + text.chars().count()))
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
        // layout_input. For empty input, keep the first row reserved for the
        // real terminal cursor so IME preedit text has a clean surface.
        let visual_rows = if input_text.is_empty() {
            let hint: Option<Cow<'_, str>> = if let Some(ref suggestion) =
                self.app.prompt_suggestion
                && !self.app.is_history_search_active()
            {
                Some(Cow::Borrowed(suggestion.as_str()))
            } else {
                Some(composer_empty_hint_text(self.app))
            };
            empty_composer_visual_rows(hint.as_deref(), content_width, input_rows_budget)
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
                    Style::default().fg(palette::WHALE_INFO)
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
            let hint: Option<Cow<'_, str>> = if let Some(ref suggestion) =
                self.app.prompt_suggestion
                && !self.app.is_history_search_active()
            {
                Some(Cow::Borrowed(suggestion.as_str()))
            } else {
                Some(composer_empty_hint_text(self.app))
            };
            empty_composer_visual_rows(hint.as_deref(), content_width, input_rows_budget)
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

    /// Build the inline approval content, split into the informational `body`
    /// (which may scroll/truncate within its region) and the interactive
    /// `controls` (which are always reserved and can never be clipped). Both
    /// `render` and `inline_region` use this so the painted band and the
    /// dimmed backdrop region always agree.
    fn build_inline_content(&self, area: Rect) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
        let risk = self.request.risk;
        let stakes = self.request.stakes();
        let locale = self.view.locale();
        let palette_colors = approval_palette(stakes);
        let critical = matches!(stakes, crate::tui::approval::ApprovalStakes::Critical);

        let mut body: Vec<Line<'static>> = Vec::with_capacity(16);
        // Header: stakes badge + tool identifier.
        body.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(" {} ", stakes_badge_text(stakes, locale)),
                Style::default()
                    .fg(palette::WHALE_BG)
                    .bg(palette_colors.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                self.request.tool_name.clone(),
                Style::default()
                    .fg(palette::WHALE_INFO)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        // Command / change preview FIRST — for an approval the thing being run
        // is the load-bearing content, so on a short terminal it is the
        // secondary context (about/impacts/category) that scrolls away, never
        // the command.
        let details = self.request.prominent_detail_items(locale);
        if details.is_empty() {
            push_params_detail_line(&mut body, self.request, locale, area.width);
        } else {
            let mut rendered_detail = false;
            for detail in details.iter().take(4) {
                let is_change_preview = matches!(detail.label.as_str(), "Preview" | "预览");
                if let Some(shell_lines) = detail.shell_lines.as_deref() {
                    let command_width = area.width.saturating_sub(10) as usize;
                    // Bound every multi-line preview so one huge command cannot
                    // grow the band without limit; the [v] pager shows the rest.
                    let max_rows = if is_change_preview {
                        if self.request.intent_summary.is_some() {
                            Some(3)
                        } else {
                            Some(5)
                        }
                    } else {
                        Some(8)
                    };
                    push_shell_command_lines(
                        &mut body,
                        &detail.label,
                        shell_lines,
                        command_width.max(20),
                        max_rows,
                    );
                } else {
                    push_detail_line(&mut body, &detail.label, &detail.value);
                }
                rendered_detail = true;
            }
            if !rendered_detail {
                push_params_detail_line(&mut body, self.request, locale, area.width);
            }
        }

        // Intent summary ("why this change is needed", #2381).
        if let Some(ref summary) = self.request.intent_summary {
            let max_width = area.width.saturating_sub(14) as usize;
            if max_width > 0 {
                let intent_label = tr(locale, MessageId::ApprovalIntentLabel);
                let summary_lines: Vec<&str> = summary.lines().collect();
                let intent_lines = 3usize;
                for (i, sline) in summary_lines.iter().take(intent_lines).enumerate() {
                    let prefix = if i == 0 {
                        intent_label.clone()
                    } else {
                        Cow::Borrowed("  ")
                    };
                    let truncated = crate::utils::truncate_with_ellipsis(sline, max_width, "...");
                    body.push(Line::from(vec![
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
                if summary_lines.len() > intent_lines {
                    let more = tr(locale, MessageId::ApprovalMoreLines)
                        .replace("{count}", &(summary_lines.len() - intent_lines).to_string());
                    body.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(more, Style::default().fg(palette::TEXT_HINT)),
                    ]));
                }
            }
        }

        // Destructive policy / cancel semantics — critical stakes only. For
        // routine and elevated work the controls speak for themselves; the
        // extra policy prose was noise that made every edit read like an
        // emergency.
        if critical {
            push_destructive_approval_semantics(&mut body, locale, false);
        }

        // Secondary context: what it is and what it touches. Only critical
        // prompts carry the full about/impact/category dossier by default —
        // everything stays one `v` away in the details pager. Keep a single
        // About line as fallback context when nothing else was rendered.
        if critical || details.is_empty() {
            body.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(label_about(locale), Style::default().fg(palette::TEXT_HINT)),
                Span::styled(
                    self.request.description_for_locale(locale),
                    Style::default().fg(palette::TEXT_BODY),
                ),
            ]));
        }
        if critical {
            for impact in self.request.impacts_for_locale(locale).into_iter().take(4) {
                body.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        label_impact(locale),
                        Style::default().fg(palette::TEXT_HINT),
                    ),
                    Span::styled(impact, Style::default().fg(palette::TEXT_BODY)),
                ]));
            }
            // Category line — localized risk category.
            let (cat_label, cat_color) = category_label_for(self.request.category, locale);
            body.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(label_type(locale), Style::default().fg(palette::TEXT_HINT)),
                Span::styled(
                    cat_label,
                    Style::default().fg(cat_color).add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        // Preview of the persistent ask-rule the `[s]` shortcut would save
        // (#3766). Informational, so it lives in the (scrollable) body.
        if let Some(preview) = self.request.ask_rule_save_preview() {
            push_ask_rule_save_preview(&mut body, &preview, palette_colors.shortcut, area.width);
        }

        let controls = build_approval_controls(
            self.request,
            self.view,
            risk,
            locale,
            palette_colors.accent,
            palette_colors.shortcut,
        );
        (body, controls)
    }

    /// Bottom-anchored band this inline prompt occupies within `area`. Must
    /// match what `render` paints so the backdrop dims exactly this strip.
    pub(crate) fn inline_region(&self, area: Rect) -> Rect {
        if area.width == 0 || area.height == 0 {
            return Rect {
                x: area.x,
                y: area.y.saturating_add(area.height),
                width: 0,
                height: 0,
            };
        }
        if self.view.collapsed {
            // Collapsed mode is a single banner row pinned to the bottom.
            let h = area.height.min(1);
            return Rect {
                x: area.x,
                y: area.y.saturating_add(area.height.saturating_sub(h)),
                width: area.width,
                height: h,
            };
        }
        let (body, controls) = self.build_inline_content(area);
        inline_region_for(area, &body, &controls)
    }
}

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

            let stakes = self.request.stakes();
            let palette_colors = approval_palette(stakes);
            let summary = format!(
                " {} — {}  [Tab to expand] ",
                self.request.tool_name,
                stakes_badge_text(stakes, self.view.locale()),
            );
            let line = Line::from(Span::styled(
                summary,
                Style::default()
                    .fg(palette::WHALE_BG)
                    .bg(palette_colors.accent)
                    .add_modifier(Modifier::BOLD),
            ));
            Paragraph::new(line).render(bar_area, buf);
            return;
        }

        // Compute stakes once for this render pass (it runs command_safety
        // analysis on shell commands); reuse it for the palette and the
        // left-rail gate instead of re-deriving per band.
        let stakes = self.request.stakes();
        let palette_colors = approval_palette(stakes);
        let (body, controls) = self.build_inline_content(area);
        let region = inline_region_for(area, &body, &controls);
        if region.width == 0 || region.height == 0 {
            return;
        }

        // Opaque inline panel anchored to the bottom of the frame. The
        // transcript above stays visible; only this band is painted — the
        // approval is no longer a full-screen takeover (#3799).
        Clear.render(region, buf);
        Block::default()
            .style(Style::default().bg(palette::WHALE_BG))
            .render(region, buf);

        // Top separator rule, risk-tinted, so the prompt reads as a distinct
        // panel without a heavy full border box.
        let rule: String = "─".repeat(region.width as usize);
        buf.set_string(
            region.x,
            region.y,
            &rule,
            Style::default().fg(palette_colors.border),
        );

        // Reserve the controls FIRST: they take their rows off the bottom of
        // the band and can never be clipped, no matter how long the body is.
        // The informational body takes whatever remains and shows a pager
        // affordance when it does not fit. This is the core #3799 fix — the
        // action row is no longer the last thing in a single clipping
        // Paragraph.
        let inner_top = region.y.saturating_add(1);
        let inner_height = region.height.saturating_sub(1);
        let control_rows = measure_wrapped_rows(&controls, region.width).min(inner_height);
        let body_height = inner_height.saturating_sub(control_rows);

        let body_rect = Rect {
            x: region.x,
            y: inner_top,
            width: region.width,
            height: body_height,
        };
        let control_rect = Rect {
            x: region.x,
            y: inner_top.saturating_add(body_height),
            width: region.width,
            height: control_rows,
        };

        let body_rows = measure_wrapped_rows(&body, region.width);
        if body_rows > body_height && body_height > 0 {
            // Body does not fit (short terminal): show as much as we can and
            // point at the params pager ([v]) for the full content.
            let shown = body_height.saturating_sub(1);
            if shown > 0 {
                Paragraph::new(body).wrap(Wrap { trim: false }).render(
                    Rect {
                        height: shown,
                        ..body_rect
                    },
                    buf,
                );
            }
            buf.set_string(
                region.x,
                body_rect.y.saturating_add(shown),
                approval_truncation_hint(self.view.locale()),
                Style::default().fg(palette::TEXT_HINT),
            );
        } else {
            Paragraph::new(body)
                .wrap(Wrap { trim: false })
                .render(body_rect, buf);
        }

        Paragraph::new(controls)
            .wrap(Wrap { trim: false })
            .render(control_rect, buf);

        if matches!(stakes, crate::tui::approval::ApprovalStakes::Critical) {
            paint_left_rail(region, buf, palette_colors.accent);
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

/// Bottom-anchored band the inline approval prompt occupies within `area`.
/// Sized to the measured content but never taller than the frame, and always
/// tall enough to show the reserved controls (#3799).
fn inline_region_for(area: Rect, body: &[Line<'static>], controls: &[Line<'static>]) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect {
            x: area.x,
            y: area.y.saturating_add(area.height),
            width: 0,
            height: 0,
        };
    }
    let width = area.width;
    let body_rows = measure_wrapped_rows(body, width);
    let control_rows = measure_wrapped_rows(controls, width);
    // +1 for the top separator rule.
    let desired = 1u16.saturating_add(body_rows).saturating_add(control_rows);
    // Never shrink below the rule + controls; never exceed the frame.
    let min_height = 1u16.saturating_add(control_rows).min(area.height);
    let height = desired.clamp(min_height, area.height);
    Rect {
        x: area.x,
        y: area.y.saturating_add(area.height.saturating_sub(height)),
        width,
        height,
    }
}

/// Terminal rows `lines` occupy when wrapped to `width`, using display width
/// (CJK/emoji aware). The controls reserve a trailing blank row of headroom so
/// that even if ratatui word-wrap rounds up past this estimate, the action row
/// is never clipped.
fn measure_wrapped_rows(lines: &[Line<'static>], width: u16) -> u16 {
    if width == 0 {
        return lines.len() as u16;
    }
    let w = width as usize;
    lines
        .iter()
        .map(|line| {
            let dw: usize = line.spans.iter().map(|s| s.content.as_ref().width()).sum();
            dw.div_ceil(w).max(1) as u16
        })
        .fold(0u16, |acc, rows| acc.saturating_add(rows))
}

/// Build the always-visible approval controls: a "proceed?" prompt, the
/// numbered/selectable options, and the selection hint. Rendered into a region
/// reserved off the bottom of the band so it can never be clipped (#3799).
fn build_approval_controls(
    request: &ApprovalRequest,
    view: &ApprovalView,
    risk: RiskLevel,
    locale: Locale,
    accent: Color,
    shortcut: Color,
) -> Vec<Line<'static>> {
    let mut controls: Vec<Line<'static>> = Vec::with_capacity(8);
    controls.push(Line::from(""));
    controls.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            approval_proceed_question(locale),
            Style::default()
                .fg(palette::TEXT_BODY)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    for (i, opt) in approval_options_for(risk, locale).iter().enumerate() {
        let is_selected = i == view.selected();
        let label_color = if opt.dangerous {
            accent
        } else {
            palette::TEXT_BODY
        };
        let option_style = approval_option_style(is_selected, label_color);
        let shortcut_style = approval_option_style(is_selected, shortcut);
        // Leading caret marks the row Enter will fire — selection is not
        // signalled by background alone.
        let lead = if is_selected {
            Span::styled("\u{276f} ", approval_selected_style())
        } else {
            Span::raw("  ")
        };
        controls.push(Line::from(vec![
            lead,
            Span::styled(
                format!("[{}] ", opt.key_hint),
                shortcut_style.add_modifier(Modifier::BOLD),
            ),
            Span::styled(opt.label.to_string(), option_style),
        ]));
    }
    controls.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            footer_controls(locale),
            Style::default().fg(palette::TEXT_MUTED),
        ),
        if request.can_save_ask_rule() {
            Span::styled(save_ask_rule_hint(locale), Style::default().fg(shortcut))
        } else {
            Span::raw("")
        },
    ]));
    // Trailing blank: bottom padding plus a row of headroom so word-wrap of the
    // hint line can never push a control row out of the reserved region.
    controls.push(Line::from(""));
    controls
}

fn approval_proceed_question(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => "是否继续？",
        _ => "Do you want to proceed?",
    }
}

fn approval_truncation_hint(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => "  … 已截断 · 按 [v] 查看完整内容",
        _ => "  … truncated · press [v] for full details",
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
        cell.set_style(Style::default().fg(color).bg(palette::WHALE_BG));
    }
}

/// Approval palette per risk variant.
struct ApprovalColors {
    border: Color,
    accent: Color,
    shortcut: Color,
}

fn approval_palette(stakes: crate::tui::approval::ApprovalStakes) -> ApprovalColors {
    use crate::tui::approval::ApprovalStakes;
    match stakes {
        ApprovalStakes::Routine => ApprovalColors {
            border: palette::BORDER_COLOR,
            accent: palette::WHALE_INFO,
            shortcut: palette::WHALE_INFO,
        },
        // Ordinary state-touching work: a calm ask, not an alarm.
        ApprovalStakes::Elevated => ApprovalColors {
            border: palette::BORDER_COLOR,
            accent: palette::STATUS_WARNING,
            shortcut: palette::WHALE_INFO,
        },
        ApprovalStakes::Critical => ApprovalColors {
            border: palette::WHALE_ERROR,
            accent: palette::WHALE_ERROR,
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

fn stakes_badge_text(
    stakes: crate::tui::approval::ApprovalStakes,
    locale: Locale,
) -> Cow<'static, str> {
    use crate::tui::approval::ApprovalStakes;
    match stakes {
        ApprovalStakes::Routine => tr(locale, MessageId::ApprovalRiskReview),
        ApprovalStakes::Elevated => tr(locale, MessageId::ApprovalRiskElevated),
        ApprovalStakes::Critical => tr(locale, MessageId::ApprovalRiskDestructive),
    }
}

fn category_label_for(category: ToolCategory, locale: Locale) -> (Cow<'static, str>, Color) {
    let label = match category {
        ToolCategory::Safe => tr(locale, MessageId::ApprovalCategorySafe),
        ToolCategory::FileWrite => tr(locale, MessageId::ApprovalCategoryFileWrite),
        ToolCategory::Shell => tr(locale, MessageId::ApprovalCategoryShell),
        ToolCategory::Network => tr(locale, MessageId::ApprovalCategoryNetwork),
        ToolCategory::McpRead => tr(locale, MessageId::ApprovalCategoryMcpRead),
        ToolCategory::McpAction => tr(locale, MessageId::ApprovalCategoryMcpAction),
        ToolCategory::Agent => tr(locale, MessageId::ApprovalCategoryAgent),
        ToolCategory::Unknown => tr(locale, MessageId::ApprovalCategoryUnknown),
    };
    let color = match category {
        ToolCategory::Safe => palette::STATUS_SUCCESS,
        ToolCategory::FileWrite => palette::STATUS_WARNING,
        ToolCategory::Shell => palette::STATUS_ERROR,
        ToolCategory::Network => palette::STATUS_WARNING,
        ToolCategory::McpRead => palette::WHALE_INFO,
        ToolCategory::McpAction => palette::STATUS_WARNING,
        ToolCategory::Agent => palette::WHALE_INFO,
        ToolCategory::Unknown => palette::STATUS_ERROR,
    };
    (label, color)
}

fn label_type(locale: Locale) -> Cow<'static, str> {
    tr(locale, MessageId::ApprovalFieldType)
}

fn label_about(locale: Locale) -> Cow<'static, str> {
    tr(locale, MessageId::ApprovalFieldAbout)
}

fn label_impact(locale: Locale) -> Cow<'static, str> {
    tr(locale, MessageId::ApprovalFieldImpact)
}

fn label_params(locale: Locale) -> Cow<'static, str> {
    tr(locale, MessageId::ApprovalFieldParams)
}

fn push_detail_line(lines: &mut Vec<Line<'static>>, label: &str, value: &str) {
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{label:<7} "),
            Style::default()
                .fg(palette::WHALE_INFO)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(value.to_string(), Style::default().fg(palette::TEXT_BODY)),
    ]));
}

fn push_params_detail_line(
    lines: &mut Vec<Line<'static>>,
    request: &ApprovalRequest,
    locale: Locale,
    card_width: u16,
) {
    let params_str = request.params_display();
    let params_width = card_width.saturating_sub(14) as usize;
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
}

fn push_ask_rule_save_preview(
    lines: &mut Vec<Line<'static>>,
    preview: &crate::tui::approval::AskRuleSavePreview,
    shortcut: Color,
    card_width: u16,
) {
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "Save:   ",
            Style::default().fg(shortcut).add_modifier(Modifier::BOLD),
        ),
        Span::styled(preview.summary(), Style::default().fg(palette::TEXT_BODY)),
    ]));

    let entry_width = card_width.saturating_sub(10) as usize;
    let entries = preview.entries.join("; ");
    let truncated = crate::utils::truncate_with_ellipsis(&entries, entry_width.max(20), "...");
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(truncated, Style::default().fg(palette::TEXT_SECONDARY)),
    ]));
    if preview.omitted > 0 {
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(
                format!("... {} more", preview.omitted),
                Style::default().fg(palette::TEXT_HINT),
            ),
        ]));
    }
}

fn push_shell_command_lines(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    command_lines: &[String],
    command_width: usize,
    max_rows: Option<usize>,
) {
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{label}:"),
            Style::default()
                .fg(palette::WHALE_INFO)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    let mut rendered = 0usize;
    for line in command_lines {
        for wrapped in wrap_text(line, command_width) {
            if max_rows.is_some_and(|limit| rendered >= limit) {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(
                        "...",
                        Style::default()
                            .fg(palette::TEXT_HINT)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
                return;
            }
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(
                    wrapped,
                    Style::default()
                        .fg(palette::TEXT_BODY)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            rendered += 1;
        }
    }
}

fn push_destructive_approval_semantics(
    lines: &mut Vec<Line<'static>>,
    locale: Locale,
    compact: bool,
) {
    if compact {
        let (label, value) = destructive_approval_compact_semantics(locale);
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(label, Style::default().fg(palette::TEXT_HINT)),
            Span::styled(value, Style::default().fg(palette::TEXT_SECONDARY)),
        ]));
        return;
    }

    for (label, value) in destructive_approval_semantics(locale) {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(label, Style::default().fg(palette::TEXT_HINT)),
            Span::styled(value, Style::default().fg(palette::TEXT_SECONDARY)),
        ]));
    }
}

fn destructive_approval_compact_semantics(locale: Locale) -> (&'static str, &'static str) {
    match locale {
        Locale::ZhHans => ("规则: ", "批准策略要求确认；拒绝跳过本次，Esc 中止整轮。"),
        _ => (
            "Policy: ",
            "Approval policy requires review; d denies, Esc aborts.",
        ),
    }
}

fn destructive_approval_semantics(locale: Locale) -> [(&'static str, &'static str); 2] {
    match locale {
        Locale::ZhHans => [
            (
                "规则: ",
                "当前批准策略、审查规则或显式询问规则要求用户确认。",
            ),
            ("取消: ", "拒绝只跳过本次工具调用；Esc 会中止整轮。"),
        ],
        _ => [
            (
                "Policy: ",
                "The active approval policy, a review rule, or an explicit ask-rule requires confirmation.",
            ),
            (
                "Cancel: ",
                "Deny rejects only this tool call; Esc aborts the whole turn.",
            ),
        ],
    }
}

fn footer_controls(locale: Locale) -> Cow<'static, str> {
    tr(locale, MessageId::ApprovalControlsHint)
}

fn save_ask_rule_hint(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => "  s 批准并保存询问规则",
        _ => "  s approve + save ask rule",
    }
}

struct ApprovalOptionRow {
    label: Cow<'static, str>,
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

fn option_approve_once(locale: Locale) -> Cow<'static, str> {
    tr(locale, MessageId::ApprovalOptionApproveOnce)
}

fn option_approve_always(locale: Locale) -> Cow<'static, str> {
    tr(locale, MessageId::ApprovalOptionApproveAlways)
}

fn option_deny(locale: Locale) -> Cow<'static, str> {
    tr(locale, MessageId::ApprovalOptionDeny)
}

fn option_abort(locale: Locale) -> Cow<'static, str> {
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
                        .fg(palette::WHALE_INFO)
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
            .style(Style::default().bg(palette::WHALE_BG))
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
        VimMode::Insert => palette::WHALE_INFO,
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
            let vim_label = app.composer.vim_mode.label_localized(app.ui_locale);
            let vim_width = UnicodeWidthStr::width(&*vim_label);
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
            truncate_display_width(
                &app.composer.vim_mode.label_localized(app.ui_locale),
                max_width,
            ),
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
    let title = format!(">_ codewhale (v{})", env!("CARGO_PKG_VERSION"));
    let model = format!("model: {}  /model to switch", app.model);
    let directory = format!("directory: {workspace}");
    let block_width = [&title, &model, &directory]
        .into_iter()
        .map(|line| UnicodeWidthStr::width(line.as_str()))
        .max()
        .unwrap_or(0);
    let left_padding = usize::from(area.width).saturating_sub(block_width) / 2;
    let inset = " ".repeat(left_padding);

    let body = vec![
        Line::from(Span::styled(
            format!("{inset}{title}"),
            Style::default().fg(palette::WHALE_ACCENT_PRIMARY).bold(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("{inset}{model}"),
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(Span::styled(
            format!("{inset}{directory}"),
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

pub(crate) fn composer_empty_hint_text(app: &App) -> Cow<'static, str> {
    if app.is_history_search_active() {
        app.tr(crate::localization::MessageId::HistorySearchPlaceholder)
    } else {
        app.tr(crate::localization::MessageId::ComposerPlaceholder)
    }
}

pub(crate) fn empty_composer_visual_rows(
    hint: Option<&str>,
    content_width: usize,
    rows_budget: usize,
) -> usize {
    if rows_budget <= 1 {
        1
    } else {
        1 + hint
            .map(|text| placeholder_visual_lines_for(text, content_width))
            .unwrap_or(0)
    }
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

#[cfg(test)]
pub(crate) fn slash_completion_hints(
    input: &str,
    limit: usize,
    cached_skills: &[(String, String)],
    locale: crate::localization::Locale,
    workspace: Option<&std::path::Path>,
    api_provider: ApiProvider,
) -> Vec<SlashMenuEntry> {
    let model_candidates = all_catalog_models_for_provider(api_provider);
    slash_completion_hints_with_model_candidates(
        input,
        limit,
        cached_skills,
        locale,
        workspace,
        &model_candidates,
    )
}

pub(crate) fn slash_completion_hints_with_model_candidates(
    input: &str,
    limit: usize,
    cached_skills: &[(String, String)],
    locale: crate::localization::Locale,
    workspace: Option<&std::path::Path>,
    model_candidates: &[String],
) -> Vec<SlashMenuEntry> {
    if !super::app::looks_like_slash_command_input(input) {
        return Vec::new();
    }

    let trimmed = input.trim_start();
    // `$skillname` mode: only skill completions, prefixed with `$`.
    if trimmed.starts_with('$') {
        let prefix = trimmed.trim_start_matches('$').to_ascii_lowercase();
        let mut entries: Vec<SlashMenuEntry> = Vec::new();
        for (skill_name, skill_desc) in cached_skills {
            let skill_name_lower = skill_name.to_ascii_lowercase();
            if skill_name_lower.starts_with(&prefix)
                || skill_name_lower.contains(&prefix)
                || fuzzy_chars_in_order(&prefix, &skill_name_lower)
            {
                entries.push(SlashMenuEntry {
                    name: format!("${skill_name}"),
                    description: skill_desc.clone(),
                    is_skill: true,
                    alias_hint: None,
                });
            }
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries.dedup_by(|a, b| a.name == b.name);
        return entries.into_iter().take(limit).collect();
    }

    let prefix = input.trim_start_matches('/');
    let completing_skill_arg = prefix.strip_prefix("skill ").map(str::trim_start);
    if input.contains(char::is_whitespace) && completing_skill_arg.is_none() {
        return Vec::new();
    }
    let mut entries: Vec<SlashMenuEntry> = Vec::new();
    let prefix_lower = prefix.to_ascii_lowercase();

    // ── Phase 1: prefix (starts_with) matches ─────────────────────────
    // Highest priority — preserves existing exact-prefix completion.
    if completing_skill_arg.is_none() {
        commands::user_registry::with_registry_for_workspace(workspace, |registry| {
            let all_user_commands = registry.iter().collect::<Vec<_>>();
            let user_commands = all_user_commands
                .iter()
                .copied()
                .filter(|cmd| !cmd.hidden)
                .collect::<Vec<_>>();
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

            for name in
                all_command_names_matching_loaded(prefix, &user_commands, &all_user_commands)
            {
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

            // ── Phase 2: contains (substring) matches ─────────────────────────
            // Medium priority — broader catching.
            for cmd in commands::command_infos() {
                let name = format!("/{}", cmd.name);
                if seen.contains(&name) {
                    continue;
                }
                let cmd_lower = cmd.name.to_ascii_lowercase();
                let name_match = cmd_lower.contains(&prefix_lower);
                let alias_matches =
                    |alias: &str| alias.to_ascii_lowercase().contains(&prefix_lower);
                if builtin_visible_for_completion_match(
                    cmd,
                    &all_user_commands,
                    &prefix_lower,
                    name_match,
                    alias_matches,
                ) {
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
            for cmd in &user_commands {
                let name = format!("/{}", cmd.name);
                if seen.contains(&name) {
                    continue;
                }
                let alias_match = cmd.aliases.iter().any(|a| a.contains(&prefix_lower));
                if cmd.name.contains(&prefix_lower) || alias_match {
                    seen.insert(name.clone());
                    push_command_entry(
                        &mut entries,
                        &name,
                        &cmd.name,
                        &prefix_lower,
                        locale,
                        &user_commands,
                    );
                }
            }

            // ── Phase 3: fuzzy subsequence matches ────────────────────────────
            // Lowest priority — characters in order, not necessarily consecutive.
            for cmd in commands::command_infos() {
                let name = format!("/{}", cmd.name);
                if seen.contains(&name) {
                    continue;
                }
                let cmd_lower = cmd.name.to_ascii_lowercase();
                let name_match = fuzzy_chars_in_order(&prefix_lower, &cmd_lower);
                let alias_matches = |alias: &str| fuzzy_chars_in_order(&prefix_lower, alias);
                if builtin_visible_for_completion_match(
                    cmd,
                    &all_user_commands,
                    &prefix_lower,
                    name_match,
                    alias_matches,
                ) {
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
            for cmd in &user_commands {
                let name = format!("/{}", cmd.name);
                if seen.contains(&name) {
                    continue;
                }
                let alias_match = cmd
                    .aliases
                    .iter()
                    .any(|a| fuzzy_chars_in_order(&prefix_lower, a));
                if fuzzy_chars_in_order(&prefix_lower, &cmd.name) || alias_match {
                    seen.insert(name.clone());
                    push_command_entry(
                        &mut entries,
                        &name,
                        &cmd.name,
                        &prefix_lower,
                        locale,
                        &user_commands,
                    );
                }
            }
        });
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
        for model_name in model_candidates {
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

fn all_command_names_matching_loaded(
    prefix: &str,
    user_commands: &[&commands::user_registry::UserCommandMetadata],
    all_user_commands: &[&commands::user_registry::UserCommandMetadata],
) -> Vec<String> {
    let prefix = prefix.strip_prefix('/').unwrap_or(prefix).to_lowercase();
    let mut result: Vec<String> = commands::command_infos()
        .iter()
        .filter(|cmd| {
            builtin_visible_for_completion_match(
                cmd,
                all_user_commands,
                &prefix,
                cmd.name.starts_with(&prefix),
                |alias| alias.starts_with(&prefix),
            )
        })
        .map(|cmd| format!("/{}", cmd.name))
        .collect();

    result.extend(user_commands.iter().filter_map(|command| {
        let name_matches = command.name.starts_with(&prefix);
        let alias_matches = command
            .aliases
            .iter()
            .any(|alias| alias.starts_with(&prefix));
        (name_matches || alias_matches).then(|| format!("/{}", command.name))
    }));

    result.sort();
    result.dedup();
    result
}

fn builtin_visible_for_completion_match(
    builtin: &commands::CommandInfo,
    user_commands: &[&commands::user_registry::UserCommandMetadata],
    prefix: &str,
    canonical_name_matches: bool,
    alias_matches: impl Fn(&str) -> bool,
) -> bool {
    if !builtin.show_in_slash_completion(prefix) {
        return false;
    }

    if user_command_shadows_builtin_canonical(builtin, user_commands) {
        return false;
    }

    // Keep the canonical built-in visible when the typed text matches the
    // canonical name, even if a user command shadows one of the built-in's
    // aliases. Example: a user command with alias `/image` must not hide
    // canonical `/attach` for `/att`.
    if canonical_name_matches {
        return true;
    }

    // If the built-in is visible only through an alias, hide it when that
    // specific alias is shadowed by a user command. Example: `/image` should
    // complete to the user command, not built-in `/attach` via its `/image`
    // alias.
    builtin.aliases.iter().any(|alias| {
        alias_matches(alias) && !user_command_shadows_builtin_alias(alias, user_commands)
    })
}

fn user_command_shadows_builtin_canonical(
    builtin: &commands::CommandInfo,
    user_commands: &[&commands::user_registry::UserCommandMetadata],
) -> bool {
    user_commands.iter().any(|user| {
        user.name == builtin.name || user.aliases.iter().any(|alias| alias == builtin.name)
    })
}

fn user_command_shadows_builtin_alias(
    builtin_alias: &str,
    user_commands: &[&commands::user_registry::UserCommandMetadata],
) -> bool {
    user_commands.iter().any(|user| {
        user.name == builtin_alias || user.aliases.iter().any(|alias| alias == builtin_alias)
    })
}

/// Push a built-in command entry to the slash menu, resolving description
/// and alias hints.
fn push_command_entry(
    entries: &mut Vec<SlashMenuEntry>,
    name: &str,
    command_key: &str,
    prefix_lower: &str,
    locale: crate::localization::Locale,
    user_commands: &[&commands::user_registry::UserCommandMetadata],
) {
    let user_command = user_commands
        .iter()
        .find(|command| command.name == command_key);

    let (description, alias_hint) = if let Some(command) = user_command {
        // User command shadows any built-in — use user metadata.
        let mut description = command
            .description
            .clone()
            .unwrap_or_else(|| String::from("User-defined command"));
        if let Some(hint) = &command.argument_hint
            && !hint.trim().is_empty()
        {
            description.push_str("  ");
            description.push_str(hint.trim());
        }
        let alias_hint = if !command_key.to_ascii_lowercase().starts_with(prefix_lower) {
            command
                .aliases
                .iter()
                .find(|alias| {
                    alias.starts_with(prefix_lower)
                        || alias.contains(prefix_lower)
                        || fuzzy_chars_in_order(prefix_lower, alias)
                })
                .cloned()
        } else {
            None
        };
        (description, alias_hint)
    } else if let Some(info) = commands::get_command_info(command_key) {
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
        // Omit aliases already shown in the label (`/clear or /qingping`) so
        // the description does not repeat them (#3990).
        let remaining_aliases: Vec<&str> = info
            .aliases
            .iter()
            .copied()
            .filter(|alias| hint.as_deref() != Some(*alias))
            .collect();
        let desc = if remaining_aliases.is_empty() {
            info.description_for(locale).to_string()
        } else {
            format!(
                "{}  (aliases: {})",
                info.description_for(locale),
                remaining_aliases
                    .iter()
                    .map(|a| format!("/{a}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        (desc, hint)
    } else {
        (String::from("User-defined command"), None)
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

/// Extended version of `layout_input_with_scroll` that also returns character
/// indices for each wrapped line. Used by ComposerWidget to avoid redundant
/// wrapping when rendering text selections.
fn layout_input_with_scroll_and_char_indices(
    input: &str,
    cursor: usize,
    width: usize,
    max_height: usize,
) -> (Vec<String>, usize, usize, usize, Vec<(usize, String)>) {
    let (all_lines, all_with_indices) = wrap_input_lines_internal(input, width);

    let lines = if all_lines.is_empty() {
        vec![String::new()]
    } else {
        all_lines
    };

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

    // Also slice the char indices to match visible lines
    let visible_with_indices = all_with_indices
        .into_iter()
        .skip(start)
        .take(max_height)
        .collect();

    (
        visible,
        visible_cursor_row,
        cursor_col.min(width.saturating_sub(1)),
        start,
        visible_with_indices,
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

/// Internal helper that returns both wrapped lines and character indices.
/// Used by `wrap_input_lines`, `wrap_input_lines_for_mouse`, and
/// `layout_input_with_scroll` to avoid redundant wrapping computations.
fn wrap_input_lines_internal(input: &str, width: usize) -> (Vec<String>, Vec<(usize, String)>) {
    let mut lines = Vec::new();
    let mut lines_with_indices = Vec::new();
    let mut char_idx = 0usize;

    if input.is_empty() {
        lines_with_indices.push((0, String::new()));
        return (lines, lines_with_indices);
    }

    for raw_line in input.split('\n') {
        if raw_line.is_empty() {
            lines.push(String::new());
            if width != 0 {
                lines_with_indices.push((char_idx, String::new()));
            }
            char_idx += 1; // the '\n'
            continue;
        }

        let wrapped = wrap_text(raw_line, width);
        if wrapped.is_empty() {
            lines.push(String::new());
            if width != 0 {
                lines_with_indices.push((char_idx, String::new()));
            }
        } else {
            for wrapped_line in &wrapped {
                let line_char_len: usize = wrapped_line.chars().count();
                lines.push(wrapped_line.clone());
                if width != 0 {
                    lines_with_indices.push((char_idx, wrapped_line.clone()));
                }
                char_idx += line_char_len;
            }
        }
        char_idx += 1; // the '\n'
    }

    (lines, lines_with_indices)
}

fn wrap_input_lines(input: &str, width: usize) -> Vec<String> {
    let (lines, _) = wrap_input_lines_internal(input, width);
    lines
}

/// For mouse coordinate mapping: returns (char_start_of_line, line_text) pairs
/// matching the wrapping produced by `wrap_input_lines`.
pub fn wrap_input_lines_for_mouse(input: &str, width: usize) -> Vec<(usize, String)> {
    if input.is_empty() || width == 0 {
        return vec![(0, String::new())];
    }

    let (_, lines_with_indices) = wrap_input_lines_internal(input, width);
    lines_with_indices
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
        ApprovalWidget, COMPOSER_PANEL_HEIGHT, COMPOSER_PLACEHOLDER, ChatWidget, ComposerWidget,
        Renderable, SlashMenuEntry, apply_detail_target_highlight, apply_selection_to_line,
        apply_send_flash, build_empty_state_lines, composer_height, composer_max_height,
        composer_min_input_rows, composer_top_padding, cursor_row_col, empty_composer_visual_rows,
        layout_input, pad_lines_to_bottom, placeholder_visual_lines, push_command_entry,
        should_render_empty_state, slash_completion_hints, wrap_input_lines,
        wrap_input_lines_for_mouse, wrap_text,
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
    use std::time::{Duration, Instant};
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
        let mut app = App::new(options, &Config::default());
        app.ui_locale = Locale::En;
        app.composer.vim_enabled = false;
        app
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

    fn render_approval_request(
        request: &crate::tui::approval::ApprovalRequest,
        area: Rect,
    ) -> String {
        let view = crate::tui::approval::ApprovalView::new(request.clone());
        let widget = ApprovalWidget::new(request, &view);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        buffer_text(&buf, area)
    }

    fn row_text(buf: &Buffer, area: Rect, row: u16) -> String {
        let mut text = String::new();
        for x in area.x..area.x.saturating_add(area.width) {
            text.push_str(buf[(x, row)].symbol());
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
        assert!(rendered.contains("read_file.txt"), "{rendered}");
        assert!(rendered.contains("list_dir.txt"), "{rendered}");
        assert!(rendered.contains("web_search.txt"), "{rendered}");
        assert!(
            !rendered.contains("full output from list_dir"),
            "{rendered}"
        );
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
    fn wrap_input_lines_for_mouse_empty_input() {
        // Empty input should return a single empty line at position 0.
        // This ensures empty composer mouse selection works correctly (issue #3909).
        let result = wrap_input_lines_for_mouse("", 10);
        assert_eq!(result, vec![(0, String::new())]);

        // Also verify with width=0 edge case
        let result_zero = wrap_input_lines_for_mouse("", 0);
        assert_eq!(result_zero, vec![(0, String::new())]);
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
    fn slash_completion_does_not_repeat_alias_already_in_label() {
        // Typing `/p` matches `/clear` via alias `qingping`, so the label
        // shows `/clear or /qingping`. The description must not also append
        // `(aliases: /qingping)` (#3990).
        let hints = slash_completion_hints("/p", 128, &[], Locale::En, None, ApiProvider::Deepseek);
        let clear = hints
            .iter()
            .find(|h| h.name == "/clear")
            .expect("/clear should appear for /p via qingping");
        assert_eq!(
            clear.alias_hint.as_deref(),
            Some("qingping"),
            "label should surface the matching alias"
        );
        assert!(
            !clear.description.contains("(aliases:"),
            "description should omit alias list when the only alias is already in the label: {}",
            clear.description
        );
        assert!(
            !clear.description.contains("/qingping"),
            "description must not repeat /qingping: {}",
            clear.description
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
    fn slash_completion_hints_hide_toolbox_commands_until_typed() {
        let root = slash_completion_hints("/", 128, &[], Locale::En, None, ApiProvider::Deepseek);
        assert!(root.iter().any(|hint| hint.name == "/provider"));
        assert!(root.iter().any(|hint| hint.name == "/model"));
        assert!(root.iter().any(|hint| hint.name == "/fleet"));
        assert!(root.iter().any(|hint| hint.name == "/config"));
        assert!(root.iter().any(|hint| hint.name == "/statusline"));
        assert!(!root.iter().any(|hint| hint.name == "/rlm"));
        assert!(!root.iter().any(|hint| hint.name == "/modeldb"));
        assert!(!root.iter().any(|hint| hint.name == "/models"));
        assert!(!root.iter().any(|hint| hint.name == "/plugin"));
        assert!(!root.iter().any(|hint| hint.name == "/subagents"));

        let rlm = slash_completion_hints("/rl", 128, &[], Locale::En, None, ApiProvider::Deepseek);
        assert!(rlm.iter().any(|hint| hint.name == "/rlm"));

        let modeldb =
            slash_completion_hints("/modeld", 128, &[], Locale::En, None, ApiProvider::Deepseek);
        assert!(modeldb.iter().any(|hint| hint.name == "/modeldb"));

        let plugin =
            slash_completion_hints("/pl", 128, &[], Locale::En, None, ApiProvider::Deepseek);
        assert!(plugin.iter().any(|hint| hint.name == "/plugin"));

        let subagents =
            slash_completion_hints("/sub", 128, &[], Locale::En, None, ApiProvider::Deepseek);
        assert!(subagents.iter().any(|hint| hint.name == "/subagents"));
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
    fn slash_completion_hints_exclude_hidden_user_commands() {
        let tmp = tempfile::TempDir::new().unwrap();
        let commands_dir = tmp.path().join(".codewhale").join("commands");
        std::fs::create_dir_all(&commands_dir).unwrap();
        std::fs::write(
            commands_dir.join("secret.md"),
            "---\ndescription: Internal command\nhidden: true\n---\nsecret",
        )
        .unwrap();

        let hints = slash_completion_hints(
            "/secret",
            128,
            &[],
            Locale::En,
            Some(tmp.path()),
            ApiProvider::Deepseek,
        );

        assert!(!hints.iter().any(|hint| hint.name == "/secret"));
    }

    #[test]
    fn slash_completion_hints_match_user_command_aliases() {
        let tmp = tempfile::TempDir::new().unwrap();
        let commands_dir = tmp.path().join(".codewhale").join("commands");
        std::fs::create_dir_all(&commands_dir).unwrap();
        std::fs::write(
            commands_dir.join("deploy-target.md"),
            "---\ndescription: Deploy target\nalias: ship\n---\ndeploy",
        )
        .unwrap();

        let hints = slash_completion_hints(
            "/ship",
            128,
            &[],
            Locale::En,
            Some(tmp.path()),
            ApiProvider::Deepseek,
        );
        let entry = hints
            .iter()
            .find(|hint| hint.name == "/deploy-target")
            .expect("user command should be matched by alias");

        assert_eq!(entry.alias_hint.as_deref(), Some("ship"));
        assert_eq!(entry.description, "Deploy target");
    }

    #[test]
    fn slash_completion_hints_keep_builtin_canonical_when_only_builtin_alias_is_shadowed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let commands_dir = tmp.path().join(".codewhale").join("commands");
        std::fs::create_dir_all(&commands_dir).unwrap();
        std::fs::write(
            commands_dir.join("attach-review.md"),
            "---\ndescription: Review image\nalias: image\n---\nreview image",
        )
        .unwrap();

        let canonical_hints = slash_completion_hints(
            "/att",
            128,
            &[],
            Locale::En,
            Some(tmp.path()),
            ApiProvider::Deepseek,
        );

        assert!(
            canonical_hints.iter().any(|hint| hint.name == "/attach"),
            "canonical /attach should remain visible when only its /image alias is shadowed"
        );

        let alias_hints = slash_completion_hints(
            "/image",
            128,
            &[],
            Locale::En,
            Some(tmp.path()),
            ApiProvider::Deepseek,
        );

        assert!(
            alias_hints.iter().any(|hint| hint.name == "/attach-review"),
            "user command should complete through its /image alias"
        );
        assert!(
            !alias_hints.iter().any(|hint| hint.name == "/attach"),
            "built-in /attach should not complete through shadowed /image alias"
        );
    }

    #[test]
    fn slash_completion_hints_prefer_user_metadata_for_shadowed_builtin() {
        let tmp = tempfile::TempDir::new().unwrap();
        let commands_dir = tmp.path().join(".codewhale").join("commands");
        std::fs::create_dir_all(&commands_dir).unwrap();
        std::fs::write(
            commands_dir.join("help.md"),
            "---\ndescription: Custom help workflow\nargument-hint: <topic>\n---\nhelp",
        )
        .unwrap();

        let hints = slash_completion_hints(
            "/help",
            128,
            &[],
            Locale::En,
            Some(tmp.path()),
            ApiProvider::Deepseek,
        );
        let help_entries: Vec<_> = hints.iter().filter(|hint| hint.name == "/help").collect();

        assert_eq!(help_entries.len(), 1);
        assert_eq!(help_entries[0].description, "Custom help workflow  <topic>");
    }

    #[test]
    fn review_regression_push_command_entry_uses_preloaded_user_command_frontmatter() {
        let registry = crate::commands::user_registry::UserCommandRegistry::from_loaded(vec![(
            "deploy".to_string(),
            "---\ndescription: Deploy target\nargument-hint: <env>\n---\ndeploy".to_string(),
        )]);
        let user_commands: Vec<_> = registry.iter().collect();
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
    fn empty_composer_cursor_sits_above_placeholder_hint() {
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
        // empty_composer_visual_rows = cursor row + one hint row = 2
        // top_padding = 3 - clamp(2, 1, 3) = 1
        // cursor_x = 0 + (1-0) + 0 = 1
        // cursor_y = 0 + (1-0) + (1+0) = 2
        assert_eq!(
            empty_composer_visual_rows(Some(COMPOSER_PLACEHOLDER), 38, 3),
            2
        );
        assert_eq!(widget.cursor_pos(area), Some((1, 2)));
    }

    #[test]
    fn empty_composer_cursor_accounts_for_wrapped_placeholder_hint() {
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
        // empty_composer_visual_rows = cursor row + two hint rows = 3
        // top_padding = 3 - clamp(3, 1, 3) = 0
        // cursor_x = 0 + (1-0) + 0 = 1
        // cursor_y = 0 + (1-0) + (0+0) = 1
        assert_eq!(placeholder_visual_lines(12), 2);
        assert_eq!(
            empty_composer_visual_rows(Some(COMPOSER_PLACEHOLDER), 12, 3),
            3
        );
        assert_eq!(widget.cursor_pos(area), Some((1, 1)));
    }

    #[test]
    fn empty_composer_keeps_cursor_row_clear_for_ime_preedit() {
        let mut app = create_test_app();
        app.composer_density = ComposerDensity::Comfortable;
        let slash_menu_entries = Vec::<SlashMenuEntry>::new();
        let mention_menu_entries = Vec::<String>::new();
        let widget = ComposerWidget::new(&app, 5, &slash_menu_entries, &mention_menu_entries);
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 5,
        };
        let mut buf = Buffer::empty(area);

        widget.render(area, &mut buf);
        let Some((cursor_x, cursor_y)) = widget.cursor_pos(area) else {
            panic!("empty composer should expose cursor position");
        };
        let rendered = buffer_text(&buf, area);

        assert_eq!(buf[(cursor_x, cursor_y)].symbol(), " ");
        assert!(
            rendered.contains(COMPOSER_PLACEHOLDER),
            "placeholder hint should still render below the cursor row: {rendered}"
        );
        assert!(
            !row_text(&buf, area, cursor_y).contains(COMPOSER_PLACEHOLDER),
            "cursor row must remain clear for terminal IME preedit text: {rendered}"
        );
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

        assert!(!rendered.contains("Composer"));
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

        assert!(!rendered.contains("Composer"));
        assert!(rendered.contains("turn completed"));
        assert!(rendered.contains("tool(s) used"));
    }

    #[test]
    fn composer_border_keeps_mode_titles_contextual() {
        let slash_menu_entries = Vec::<SlashMenuEntry>::new();
        let mention_menu_entries = Vec::<String>::new();
        let area = Rect {
            x: 0,
            y: 0,
            width: 96,
            height: 5,
        };

        let mut normal_app = create_test_app();
        normal_app.composer_density = ComposerDensity::Comfortable;
        let normal_widget =
            ComposerWidget::new(&normal_app, 5, &slash_menu_entries, &mention_menu_entries);
        let mut normal_buf = Buffer::empty(area);
        normal_widget.render(area, &mut normal_buf);
        let normal_rendered = buffer_text(&normal_buf, area);
        assert!(!normal_rendered.contains("Composer"));
        assert!(!normal_rendered.contains("Draft"));
        assert!(
            !normal_rendered
                .contains(&*normal_app.tr(crate::localization::MessageId::HistorySearchTitle))
        );

        let mut draft_app = create_test_app();
        draft_app.composer_density = ComposerDensity::Comfortable;
        draft_app.insert_str("first line\nsecond line");
        let draft_widget =
            ComposerWidget::new(&draft_app, 5, &slash_menu_entries, &mention_menu_entries);
        let mut draft_buf = Buffer::empty(area);
        draft_widget.render(area, &mut draft_buf);
        assert!(buffer_text(&draft_buf, area).contains("Draft"));

        let mut search_app = create_test_app();
        search_app.composer_density = ComposerDensity::Comfortable;
        search_app.start_history_search();
        let search_widget =
            ComposerWidget::new(&search_app, 5, &slash_menu_entries, &mention_menu_entries);
        let mut search_buf = Buffer::empty(area);
        search_widget.render(area, &mut search_buf);
        assert!(
            buffer_text(&search_buf, area)
                .contains(&*search_app.tr(crate::localization::MessageId::HistorySearchTitle))
        );
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

        assert_eq!(widget.cursor_pos(area), Some((0, 1)));
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

    #[test]
    fn empty_state_centers_startup_block_by_actual_text_width() {
        let mut app = create_test_app();
        app.workspace = PathBuf::from("/tmp/codewhale-test-workspace");
        app.model = "deepseek-v4-pro".to_string();

        let lines = build_empty_state_lines(&app, Rect::new(0, 0, 100, 20));
        let text_lines = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let title = format!(">_ codewhale (v{})", env!("CARGO_PKG_VERSION"));
        let model = "model: deepseek-v4-pro  /model to switch";
        let directory = "directory: /tmp/codewhale-test-workspace";
        let block_width = [title.as_str(), model, directory]
            .into_iter()
            .map(UnicodeWidthStr::width)
            .max()
            .expect("startup block has lines");
        let expected_padding = (100usize - block_width) / 2;
        let actual_padding = text_lines[2].chars().take_while(|ch| *ch == ' ').count();

        assert_eq!(actual_padding, expected_padding);
        assert_eq!(text_lines[2].trim_start(), title);
        assert_eq!(text_lines[4].trim_start(), model);
        assert_eq!(text_lines[5].trim_start(), directory);
    }

    #[test]
    fn ocean_field_paints_empty_water_but_not_occupied_empty_state_text() {
        let mut app = create_test_app();
        app.low_motion = false;
        app.fancy_animations = true;
        app.ocean_field_started_at = Instant::now() - Duration::from_millis(1_200);
        app.workspace = PathBuf::from("/tmp/codewhale-test-workspace");
        app.model = "deepseek-v4-pro".to_string();

        let area = Rect::new(0, 0, 100, 20);
        let base = app.ui_theme.surface_bg;
        let widget = ChatWidget::new(&mut app, area);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        assert_ne!(
            buf[(0, 12)].bg,
            base,
            "lower blank empty water should show ombre depth"
        );

        let title = format!(">_ codewhale (v{})", env!("CARGO_PKG_VERSION"));
        let model = "model: deepseek-v4-pro  /model to switch";
        let directory = "directory: /tmp/codewhale-test-workspace";
        let block_width = [title.as_str(), model, directory]
            .into_iter()
            .map(UnicodeWidthStr::width)
            .max()
            .expect("startup block has lines");
        let title_x = ((100usize - block_width) / 2) as u16;
        let title_cell = &buf[(title_x, 2)];
        assert_eq!(title_cell.symbol(), ">");
        assert_eq!(
            title_cell.bg, base,
            "occupied startup text must overwrite water field"
        );
    }

    #[test]
    fn ocean_field_is_flat_when_reduced_motion_is_enabled() {
        let mut app = create_test_app();
        app.low_motion = true;
        app.fancy_animations = true;
        app.ocean_field_started_at = Instant::now() - Duration::from_millis(1_200);

        let area = Rect::new(0, 0, 80, 12);
        let base = app.ui_theme.surface_bg;
        let widget = ChatWidget::new(&mut app, area);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        assert_eq!(buf[(0, 0)].bg, base);
        assert_eq!(buf[(20, 8)].bg, base);
    }

    #[test]
    fn ocean_field_waiting_surfaces_are_time_still() {
        let request = crate::tui::approval::ApprovalRequest::new(
            "approval-1",
            "exec_shell",
            "Run git status",
            &serde_json::json!({ "command": "git status" }),
            "exec_shell:git status",
        );
        let mut early = create_test_app();
        early.low_motion = false;
        early.fancy_animations = true;
        early
            .view_stack
            .push(crate::tui::approval::ApprovalView::new(request.clone()));
        early.ocean_field_started_at = Instant::now();

        let mut late = create_test_app();
        late.low_motion = false;
        late.fancy_animations = true;
        late.view_stack
            .push(crate::tui::approval::ApprovalView::new(request));
        late.ocean_field_started_at =
            Instant::now() - Duration::from_millis(crate::tui::motion::OCEAN_FIELD_CYCLE_MS * 3);

        let area = Rect::new(0, 0, 80, 12);
        let mut early_buf = Buffer::empty(area);
        ChatWidget::new(&mut early, area).render(area, &mut early_buf);
        let mut late_buf = Buffer::empty(area);
        ChatWidget::new(&mut late, area).render(area, &mut late_buf);

        assert_eq!(early_buf[(4, 8)].bg, late_buf[(4, 8)].bg);

        let mut plan_prompt = create_test_app();
        plan_prompt.plan_prompt_pending = true;
        assert!(plan_prompt.ocean_field_waiting_on_user());
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

    /// Regression for issue #582: a resize event during a long task must not
    /// leave the chat widget with an empty viewport. The actual ConHost
    /// size-stale fix lives in `tui::ui::run_tui`.
    #[test]
    fn chat_widget_renders_cleanly_after_resize_during_long_task() {
        let mut app = create_test_app();
        for i in 0..30 {
            app.add_message(HistoryCell::User {
                content: format!("user message {i} during a long-running task"),
            });
        }

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
                "resize at {width}x{height} produced an empty buffer (#582)"
            );
        }
    }

    #[test]
    fn approval_inline_band_stays_within_short_terminal() {
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
            let region = widget.inline_region(area);
            // Band never addresses cells outside the frame.
            assert!(region.x >= area.x);
            assert!(region.right() <= area.right());
            assert!(region.bottom() <= area.bottom());
            // Inline prompt is anchored to the bottom of the frame.
            assert_eq!(
                region.bottom(),
                area.bottom(),
                "approval band must be bottom-anchored at {area:?}"
            );

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
    fn approval_inline_marks_selected_row_and_separator_rule() {
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
        let rendered = buffer_text(&buf, area);

        assert!(
            rendered.contains('\u{276f}'),
            "selected option row should show a caret:\n{rendered}"
        );
        assert!(
            rendered.contains('\u{2500}'),
            "inline prompt should show a top separator rule:\n{rendered}"
        );
    }

    #[test]
    fn approval_inline_keeps_action_row_and_leaves_transcript_visible() {
        // The #3799 repro: a destructive approval with a long multi-line command
        // and long intent text. Across narrow, normal, and short terminals the
        // action row must stay visible, the band must never address cells
        // outside the frame, and on a tall terminal the band must not fill the
        // whole frame (transcript stays visible — no full-screen takeover).
        let request = crate::tui::approval::ApprovalRequest::new_with_intent(
            "approval-1",
            "exec_shell",
            "Run shell command",
            &serde_json::json!({
                "command": "rm -rf ./build && find . -name '*.tmp' -delete && cargo clean && echo done",
            }),
            "exec_shell:cleanup",
            Some(
                "Clearing stale build artifacts and temp files before a fresh run so the next build is reproducible.",
            ),
            std::path::Path::new("/tmp/project"),
        );
        let view = crate::tui::approval::ApprovalView::new(request.clone());
        let widget = ApprovalWidget::new(&request, &view);

        for (w, h) in [(40u16, 14u16), (80, 24), (100, 50), (60, 10)] {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            widget.render(area, &mut buf);
            let rendered = buffer_text(&buf, area);

            // Action row is always present (reserved off the bottom of the band).
            assert!(
                rendered.contains("[1 / y]") && rendered.contains("[3 / d / n]"),
                "action row must stay visible at {w}x{h}:\n{rendered}"
            );

            // Band stays inside the frame and is anchored to the bottom.
            let region = widget.inline_region(area);
            assert!(region.right() <= area.right() && region.bottom() <= area.bottom());
            assert_eq!(
                region.bottom(),
                area.bottom(),
                "band must be bottom-anchored at {w}x{h}"
            );

            // Tall terminal with content that fits: transcript above stays
            // visible — the prompt is not a full-screen takeover.
            if h >= 40 {
                assert!(
                    region.y > area.y,
                    "tall frame must leave transcript visible above the band at {w}x{h}"
                );
            }
        }
    }

    #[test]
    fn approval_option_two_reads_as_session_scoped_not_always() {
        // #3766: option 2 / `a` maps to ReviewDecision::ApprovedForSession, so
        // neither the full option rows nor the compact controls may tell the
        // user the approval is "always"/permanent. Persisting is the separate
        // `s` save-rule action.
        let request = crate::tui::approval::ApprovalRequest::new(
            "approval-1",
            "exec_shell",
            "Run git commit",
            &serde_json::json!({ "command": "git commit -m fix" }),
            "exec_shell:git commit",
        );

        // Full card (tall): full option rows render the session-scoped label.
        let full = render_approval_request(&request, Rect::new(0, 0, 100, 30));
        assert!(
            full.to_lowercase().contains("this session"),
            "full approval option must state session scope:\n{full}"
        );
        assert!(
            !full.to_lowercase().contains("always"),
            "full approval card must not call the session option 'always':\n{full}"
        );

        // Short terminal: the reserved controls still render the session-scoped
        // option `[2 / a]` and never call it "always".
        let compact = render_approval_request(&request, Rect::new(0, 0, 60, 17));
        assert!(
            compact.contains("[2 / a]") && compact.to_lowercase().contains("session"),
            "short-terminal controls must label [2 / a] as session-scoped:\n{compact}"
        );
        assert!(
            !compact.to_lowercase().contains("always"),
            "short-terminal controls must not label the session option 'always':\n{compact}"
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
    fn approval_card_renders_shell_ask_rule_save_preview() {
        let request = crate::tui::approval::ApprovalRequest::new(
            "approval-1",
            "exec_shell",
            "Run shell command",
            &serde_json::json!({ "command": "cargo test --workspace" }),
            "exec_shell:cargo-test",
        );

        let rendered = render_approval_request(&request, Rect::new(0, 0, 120, 40));

        assert!(rendered.contains("s approve + save ask rule"), "{rendered}");
        assert!(rendered.contains("Save:"), "{rendered}");
        assert!(rendered.contains("1 ask rule"), "{rendered}");
        assert!(
            rendered.contains("tool=exec_shell command=cargo test --workspace"),
            "{rendered}"
        );
    }

    #[test]
    fn approval_card_renders_file_ask_rule_save_previews() {
        let cases = [
            (
                "write_file",
                serde_json::json!({
                    "path": "src/main.rs",
                    "content": "fn main() {}\n",
                }),
                "tool=write_file path=src/main.rs",
            ),
            (
                "edit_file",
                serde_json::json!({
                    "path": "/workspace/src/lib.rs",
                    "old_string": "old",
                    "new_string": "new",
                }),
                "tool=edit_file path=src/lib.rs",
            ),
        ];

        for (tool_name, params, expected_rule) in cases {
            let request = crate::tui::approval::ApprovalRequest::new(
                "approval-1",
                tool_name,
                "Modify a file",
                &params,
                &format!("{tool_name}:src"),
            );

            let rendered = render_approval_request(&request, Rect::new(0, 0, 120, 40));

            assert!(rendered.contains("Save:"), "{tool_name}:\n{rendered}");
            assert!(rendered.contains("1 ask rule"), "{tool_name}:\n{rendered}");
            assert!(
                rendered.contains(expected_rule),
                "{tool_name} should preview {expected_rule}:\n{rendered}"
            );
        }
    }

    #[test]
    fn approval_card_renders_apply_patch_multi_rule_save_preview() {
        let patch = "diff --git a/src/a.rs b/src/a.rs\n\
--- a/src/a.rs\n\
+++ b/src/a.rs\n\
@@ -1,1 +1,1 @@\n\
-old\n\
+new\n\
diff --git a/src/b.rs b/src/b.rs\n\
--- a/src/b.rs\n\
+++ b/src/b.rs\n\
@@ -1,1 +1,1 @@\n\
-old\n\
+new\n";
        let request = crate::tui::approval::ApprovalRequest::new(
            "approval-1",
            "apply_patch",
            "Apply a patch",
            &serde_json::json!({ "patch": patch }),
            "apply_patch:multi",
        );

        let rendered = render_approval_request(&request, Rect::new(0, 0, 120, 40));

        assert!(rendered.contains("Save:"), "{rendered}");
        assert!(rendered.contains("2 ask rules"), "{rendered}");
        assert!(
            rendered.contains("tool=apply_patch path=src/a.rs"),
            "{rendered}"
        );
        assert!(
            rendered.contains("tool=apply_patch path=src/b.rs"),
            "{rendered}"
        );
    }

    #[test]
    fn approval_card_truncates_apply_patch_ask_rule_save_preview() {
        let request = crate::tui::approval::ApprovalRequest::new(
            "approval-1",
            "apply_patch",
            "Apply a patch",
            &serde_json::json!({
                "changes": [
                    { "path": "src/a.rs", "content": "a" },
                    { "path": "src/b.rs", "content": "b" },
                    { "path": "src/c.rs", "content": "c" },
                    { "path": "src/d.rs", "content": "d" },
                    { "path": "src/e.rs", "content": "e" }
                ]
            }),
            "apply_patch:many",
        );

        let rendered = render_approval_request(&request, Rect::new(0, 0, 120, 40));

        assert!(rendered.contains("5 ask rules"), "{rendered}");
        assert!(
            rendered.contains("tool=apply_patch path=src/a.rs"),
            "{rendered}"
        );
        assert!(rendered.contains("... 1 more"), "{rendered}");
        assert!(
            !rendered.contains("tool=apply_patch path=src/e.rs"),
            "truncated rule should not render directly:\n{rendered}"
        );
    }

    #[test]
    fn approval_card_omits_ask_rule_save_preview_when_rule_is_unavailable() {
        let unsafe_path = crate::tui::approval::ApprovalRequest::new(
            "approval-1",
            "write_file",
            "Write a file",
            &serde_json::json!({
                "path": "../escape.rs",
                "content": "unsafe\n",
            }),
            "write_file:escape",
        );
        let preflight_failed = crate::tui::approval::ApprovalRequest::new(
            "approval-2",
            "apply_patch",
            "Apply a patch",
            &serde_json::json!({ "patch": "@@ -1 +1 @@\n-old\n+new\n" }),
            "apply_patch:invalid",
        );

        for request in [unsafe_path, preflight_failed] {
            let rendered = render_approval_request(&request, Rect::new(0, 0, 120, 40));

            assert!(
                !rendered.contains("s approve + save ask rule"),
                "S shortcut should stay hidden:\n{rendered}"
            );
            assert!(
                !rendered.contains("Save:"),
                "save preview should stay hidden:\n{rendered}"
            );
            assert!(
                !rendered.contains("ask rule"),
                "ask-rule details should stay hidden:\n{rendered}"
            );
        }
    }

    #[test]
    fn approval_file_write_modal_renders_proposed_change_preview() {
        let request = crate::tui::approval::ApprovalRequest::new(
            "approval-1",
            "write_file",
            "Write a file",
            &serde_json::json!({
                "path": "src/main.rs",
                "content": "fn main() {\n    println!(\"visible before approval\");\n}\n",
            }),
            "write_file:src/main.rs",
        );
        let view = crate::tui::approval::ApprovalView::new(request.clone());
        let widget = ApprovalWidget::new(&request, &view);
        let area = Rect::new(0, 0, 120, 34);
        let mut buf = Buffer::empty(area);

        widget.render(area, &mut buf);
        let rendered = buffer_text(&buf, area);

        assert!(rendered.contains("Preview:"), "{rendered}");
        assert!(rendered.contains("+ fn main() {"), "{rendered}");
        assert!(
            rendered.contains("visible before approval"),
            "approval modal should show proposed file content before approval:\n{rendered}"
        );
    }

    #[test]
    fn apply_patch_approval_shows_preview_and_reserved_controls_on_short_terminal() {
        let request = crate::tui::approval::ApprovalRequest::new(
            "approval-1",
            "apply_patch",
            "Apply a patch",
            &serde_json::json!({
                "patch": "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n",
            }),
            "apply_patch:src/lib.rs",
        );
        let view = crate::tui::approval::ApprovalView::new(request.clone());
        let widget = ApprovalWidget::new(&request, &view);
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);

        widget.render(area, &mut buf);
        let rendered = buffer_text(&buf, area);

        // The change preview renders (bounded), and the action row is reserved
        // off the bottom of the band so it can never be clipped (#3799).
        assert!(rendered.contains("Preview:"), "{rendered}");
        assert!(rendered.contains("[1 / y]"), "{rendered}");
        assert!(rendered.contains("[3 / d / n]"), "{rendered}");
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
            std::path::Path::new("/tmp/project"),
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

    #[test]
    fn approval_shell_modal_stays_useful_on_short_terminals() {
        let request = crate::tui::approval::ApprovalRequest::new_with_intent(
            "approval-1",
            "exec_shell",
            "Built-in safety gate requires approval: destructive background/headless actions cannot auto-approve",
            &serde_json::json!({
                "command": "cd /Volumes/VIXinSSD/codewhale; cargo clippy -p codewhale-tui --all-targets --locked -- -D warnings 2>&1 | tee /tmp/codewhale-clippy.log",
                "cwd": "/Volumes/VIXinSSD/codewhale",
            }),
            "exec_shell:cargo-clippy",
            Some("Confirmed - passes in isolation, so this is the documentation gate."),
            std::path::Path::new("/Volumes/VIXinSSD/codewhale"),
        );
        let view = crate::tui::approval::ApprovalView::new(request.clone());
        let widget = ApprovalWidget::new(&request, &view);
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);

        widget.render(area, &mut buf);
        let rendered = buffer_text(&buf, area);

        assert!(
            !rendered.contains("Built-in safety gate requires approval"),
            "policy internals should not be the modal summary:\n{rendered}"
        );
        assert!(
            !rendered.contains("Impact: Command"),
            "command should only render in the command block:\n{rendered}"
        );
        // The command is the prioritized body content, so it stays visible even
        // when the band is short and secondary context scrolls away.
        assert!(rendered.contains("Command:"), "{rendered}");
        assert!(rendered.contains("cargo clippy"), "{rendered}");
        // Action row is reserved off the bottom and always visible (#3799).
        assert!(rendered.contains("[1 / y]"), "{rendered}");
        assert!(rendered.contains("[2 / a]"), "{rendered}");
        assert!(rendered.contains("[3 / d / n]"), "{rendered}");
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
