pub mod agent_card;
mod header;
pub mod key_hint;
pub mod pending_input_preview;
mod renderable;
pub mod tool_card;
pub mod workflow_panel;

pub use header::header_status_indicator_frame;
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
use crate::tui::app::{App, AppMode, ComposerDensity, ViewportState};
use crate::tui::approval::{
    ApprovalMode, ApprovalRequest, ApprovalView, ElevationOption, ElevationRequest, RiskLevel,
    ToolCategory,
};
use crate::tui::history::{GenericToolCell, HistoryCell, ToolCell, ToolRun, ToolStatus};
use crate::tui::menu_style;
use crate::tui::scrolling::TranscriptLineMeta;
use crate::tui::ui_text::{grapheme_display_width, text_display_width};
use crate::tui::underwater::ShellPhase;
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
#[cfg(test)]
const COMPOSER_PANEL_HEIGHT: u16 = 2;
const JUMP_TO_LATEST_BUTTON_WIDTH: u16 = 3;
const JUMP_TO_LATEST_BUTTON_HEIGHT: u16 = 3;
pub struct ChatWidget {
    content_area: Rect,
    /// Scrollable/selectable transcript geometry. When the last prompt is
    /// pinned, this starts one row below `content_area`; the pinned header is
    /// intentionally outside transcript hit-testing.
    transcript_area: Rect,
    lines: Vec<Line<'static>>,
    line_links: Vec<Vec<crate::tui::osc8::LineLink>>,
    scrollbar: Option<TranscriptScrollbar>,
    jump_to_latest_button: Option<Rect>,
    background: Color,
    ocean_column: Option<crate::tui::ocean::OceanColumn>,
    /// Live-activity shape of the ambient scene (thinking/tools/subagents).
    ocean_activity: crate::tui::ambient_life::AmbientActivity,
    /// Ink for the selected underwater scene's idle fish/bubbles.
    ambient_inks: Option<(Color, Color)>,
    ocean_elapsed_ms: u128,
    ocean_animated: bool,
    /// Fixed-point (0..=1000) life presence; see `ocean::life_presence`.
    life_presence_fixed: u16,
    fish_flee_elapsed_ms: Option<u128>,
    ambient_life: bool,
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

/// A `todo_write` result is a full replacement snapshot, not an incremental
/// transcript event. Keep only the newest successful snapshot in the visible
/// transcript while retaining every tool result in history for model context
/// and persistence.
fn superseded_todo_write_indices(
    history: &[HistoryCell],
    active_entries: &[HistoryCell],
) -> HashSet<usize> {
    let mut hidden = HashSet::new();
    let mut latest = None;

    for (index, cell) in history.iter().chain(active_entries).enumerate() {
        let HistoryCell::Tool(ToolCell::Generic(tool)) = cell else {
            continue;
        };
        if tool.name != "todo_write" || tool.status != ToolStatus::Success {
            continue;
        }

        if let Some(previous) = latest.replace(index) {
            hidden.insert(previous);
        }
    }

    if let Some(index) = latest {
        let cell = history
            .get(index)
            .or_else(|| active_entries.get(index.saturating_sub(history.len())));
        if cell.is_some_and(todo_write_snapshot_is_empty) {
            hidden.insert(index);
        }
    }

    hidden
}

fn todo_write_snapshot_is_empty(cell: &HistoryCell) -> bool {
    let HistoryCell::Tool(ToolCell::Generic(tool)) = cell else {
        return false;
    };
    let Some(output) = tool.output.as_deref() else {
        return false;
    };
    let Some(json_start) = output.find('{') else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&output[json_start..]) else {
        return false;
    };
    value
        .get("items")
        .and_then(serde_json::Value::as_array)
        .is_some_and(Vec::is_empty)
}

fn resolve_transcript_viewport_after_layout(
    viewport: &mut ViewportState,
    visible_lines: usize,
) -> (usize, usize, bool) {
    let total_lines = viewport.transcript_cache.total_lines();
    let line_meta = viewport.transcript_cache.line_meta();
    if viewport.pending_scroll_delta != 0 {
        viewport.transcript_scroll = viewport.transcript_scroll.scrolled_by(
            viewport.pending_scroll_delta,
            line_meta,
            visible_lines,
        );
        viewport.pending_scroll_delta = 0;
    }

    let max_start = total_lines.saturating_sub(visible_lines);
    // Snapshot tail intent before resolve: clamping an out-of-range fixed
    // offset can return `to_bottom()`, which must not masquerade as the user's
    // choice to resume following a streaming tail (v0.8.11).
    let was_explicit_tail = viewport.transcript_scroll.is_at_tail();
    let (scroll_state, top) = viewport.transcript_scroll.resolve_top(line_meta, max_start);
    viewport.transcript_scroll = scroll_state;
    viewport.last_transcript_top = top;
    viewport.last_transcript_visible = visible_lines;
    viewport.last_transcript_total = total_lines;
    (total_lines, top, was_explicit_tail)
}

impl ChatWidget {
    pub fn new(app: &mut App, area: Rect) -> Self {
        // The clamped ambient clock, not raw wall time: sparse draw schedules
        // advance the scene by at most one small step per frame, so creatures
        // drift instead of teleporting between distant samples.
        let ocean_elapsed_ms = app.sample_ambient_clock_ms();
        Self::new_with_ocean_elapsed(app, area, ocean_elapsed_ms)
    }

    /// Build one render snapshot from an already sampled ocean clock.
    ///
    /// Production samples the monotonic clock in [`Self::new`]. Keeping the
    /// sampled value as an explicit input here gives render tests a stable
    /// frame without adding a second clock or freezing the runtime animation.
    fn new_with_ocean_elapsed(app: &mut App, area: Rect, ocean_elapsed_ms: u128) -> Self {
        let content_area = area;
        let background = app.ui_theme.surface_bg;
        // The ordinary shell inherits its host/theme surface. Underwater life
        // is earned by the underwater theme, never painted over a user's
        // terminal simply because the app happens to be active.
        let underwater_atmosphere = app.theme_id == crate::palette::ThemeId::Underwater;
        let ocean_ramp = underwater_atmosphere
            .then(|| crate::tui::ocean::OceanRamp::for_theme(&app.ui_theme))
            .flatten();
        // Ink hue carries the live activity (reasoning deep-dim, tools bright,
        // sub-agents seafoam) so the marks read the state at a glance.
        let ocean_activity = crate::tui::ambient_life::AmbientActivity::from_kind(
            crate::tui::underwater::LiveActivity::from_app(app).kind(),
        );
        let ambient_inks = underwater_atmosphere
            .then(|| crate::tui::ocean::ambient_inks_for_activity(&app.ui_theme, ocean_activity));
        // The completion breath is authored decorative motion, so it rides the
        // same motion gate as everything else in the water. Both the column's
        // settle flourish and ambient life's presence read this one clock:
        // presence needs the settle tail past the breath, the column does not.
        let completion_life_clock = (underwater_atmosphere
            && app.motion_policy().allows_decorative())
        .then_some(())
        .and(app.ocean_completion_started_at)
        .map(|started| started.elapsed().as_millis());
        let completion_elapsed_ms = completion_life_clock
            .filter(|elapsed| *elapsed < crate::tui::ocean::COMPLETION_BREATH_MS);
        let completion_life_active = completion_life_clock
            .is_some_and(|elapsed| elapsed < crate::tui::ocean::COMPLETION_SETTLE_MS);
        let render_empty_state = should_render_empty_state(app);
        let phase = ShellPhase::from_app(app);
        // Keep the selected underwater scene alive while a turn is doing
        // work, even after the transcript exists. The ordinary Flat shell
        // remains entirely still and lets the host terminal lead.
        let underwater_motion_enabled =
            underwater_atmosphere && crate::tui::underwater::decorative_shell_motion_enabled(app);
        let browsing_history = !app.viewport.transcript_scroll.is_at_tail();
        let ocean_animated = underwater_motion_enabled
            && (render_empty_state
                || browsing_history
                || matches!(phase, ShellPhase::Working | ShellPhase::Verifying));
        // Life presence eases the animated/static boundary as a pure function
        // of the monotonic clocks (see ocean::life_presence): bursty fast
        // streams ramp in, quiet waits settle out, never a hard snap.
        //
        // This deliberately takes the *gated* completion clock. Reading
        // `app.ocean_completion_started_at` raw here let the completion branch
        // of `life_presence` short-circuit the `!animated` check, so a
        // reduced-motion session got ~1.4 s of full ambient life after every
        // successful turn — precisely while the user was reading the result.
        let life_presence = crate::tui::ocean::life_presence(
            completion_life_clock,
            app.turn_started_at
                .map(|started| started.elapsed().as_millis()),
            ocean_animated,
            browsing_history,
            render_empty_state,
        );
        let life_presence_fixed = (life_presence * 1000.0).round().clamp(0.0, 1000.0) as u16;
        let ocean_column = ocean_ramp.map(|ramp| {
            let context_percent = crate::tui::phase_strip::context_percent_from_app(app);
            crate::tui::ocean::OceanColumn::new(
                ramp,
                content_area,
                ocean_elapsed_ms,
                completion_elapsed_ms,
                phase,
                ocean_animated,
                life_presence_fixed,
                context_percent,
            )
        });
        let fish_flee_elapsed_ms = underwater_motion_enabled
            .then_some(())
            .and(app.turn_started_at)
            .map(|started| started.elapsed().as_millis())
            .filter(|elapsed| *elapsed < 800)
            .filter(|_| matches!(phase, ShellPhase::Working | ShellPhase::Verifying));
        let scroll_track = app.ui_theme.border;
        let scroll_thumb = app.ui_theme.status_working;
        let jump_border = app.ui_theme.border;
        let jump_arrow = app.ui_theme.status_working;
        let visible_lines = content_area.height as usize;
        let mut render_options = app.transcript_render_options();
        render_options.reasoning_preview_viewport_lines = Some(visible_lines);

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
                transcript_area: content_area,
                lines,
                line_links: Vec::new(),
                scrollbar: None,
                jump_to_latest_button: None,
                background,
                ocean_column,
                ocean_activity,
                ambient_inks,
                ocean_elapsed_ms,
                ocean_animated,
                life_presence_fixed,
                fish_flee_elapsed_ms,
                // Reduced-motion users still get a quiet, static Deepsea scene;
                // Flat remains a normal host-owned terminal either way.
                ambient_life: underwater_atmosphere
                    && !app.attention_hold_active()
                    && matches!(
                        phase,
                        ShellPhase::Idle
                            | ShellPhase::Typing
                            | ShellPhase::Working
                            | ShellPhase::Verifying
                    ),
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
        app.viewport.transcript_cache.set_streaming_source_receipt(
            app.streaming_source_receipt.map(|receipt| {
                crate::tui::transcript::StreamingSourceReceipt {
                    cell_index: receipt.cell_index,
                    from_revision: history_entry_revision(receipt.from_revision),
                    to_revision: history_entry_revision(receipt.to_revision),
                    content_len: receipt.content_len,
                }
            }),
        );
        let provisional_action_owner = app.transcript_action_owner();
        let active_entries: &[HistoryCell] = app
            .active_cell
            .as_ref()
            .map_or(&[], |active| active.entries());
        let superseded_todos = superseded_todo_write_indices(&app.history, active_entries);

        let history_len = app.history.len();
        let mut tool_runs = if app.tool_collapse_active() {
            let cache_key_matches = app.tool_run_cache.history_version == app.history_version
                && app.tool_run_cache.active_cell_revision == app.active_cell_revision
                && app.tool_run_cache.active_len == active_entries.len()
                && app.tool_run_cache.threshold == app.tool_collapse_threshold
                && app.tool_run_cache.mode == app.tool_collapse_mode
                && app.tool_run_cache.calm_mode == app.calm_mode;
            if !cache_key_matches {
                app.tool_run_cache.runs = crate::tui::history::detect_tool_runs_from_slices(
                    &app.history,
                    active_entries,
                    app.tool_collapse_threshold,
                );
                app.tool_run_cache.history_version = app.history_version;
                app.tool_run_cache.active_cell_revision = app.active_cell_revision;
                app.tool_run_cache.active_len = active_entries.len();
                app.tool_run_cache.threshold = app.tool_collapse_threshold;
                app.tool_run_cache.mode = app.tool_collapse_mode;
                app.tool_run_cache.calm_mode = app.calm_mode;
            }
            app.tool_run_cache.runs.clone()
        } else {
            Vec::new()
        };
        // A collapsed run that crosses a hidden replacement snapshot would
        // otherwise lose its summary start or count a row the user cannot
        // see. Leave only that run expanded; unrelated dense runs still use
        // the normal collapse path.
        tool_runs.retain(|run| {
            !(run.start..run.start.saturating_add(run.count))
                .any(|index| superseded_todos.contains(&index))
        });
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

        // v0.9.1: do not collapse concurrent sub-agent cards into an Enter-
        // expand shelf. Count lives in header chrome; full cards stay visible;
        // sidebar / SubAgents modal are the drill-in surface.
        let has_collapsed = !app.collapsed_cells.is_empty()
            || !collapsed_run_starts.is_empty()
            || !superseded_todos.is_empty();

        // Fast path: no collapsed cells — use original slices directly.
        if !has_collapsed {
            let mut cell_revisions: Vec<u64> =
                Vec::with_capacity(app.history.len() + active_entries.len());
            cell_revisions.extend(
                app.history_revisions
                    .iter()
                    .copied()
                    .map(history_entry_revision),
            );
            if !active_entries.is_empty() {
                let active_rev = app.active_cell_revision;
                for i in 0..active_entries.len() {
                    let salt = (i as u64).wrapping_add(1);
                    cell_revisions.push(active_entry_revision(active_rev, salt));
                }
            }
            // Build identity mapping: filtered index == original index.
            // Reused across frames; identity maps are rebuilt only when the
            // row count changes.
            let row_count = app.history.len() + active_entries.len();
            if app.collapsed_cell_map.len() != row_count {
                app.collapsed_cell_map = (0..row_count).collect();
            }

            let shards: [&[HistoryCell]; 2] = [&app.history, active_entries];
            app.viewport.transcript_cache.ensure_split(
                &shards,
                &cell_revisions,
                content_area.width.max(1),
                render_options,
                &app.folded_thinking,
                None,
                provisional_action_owner,
            );
        } else {
            // Slow path: borrow non-collapsed cells into a filtered ref list
            // so collapsed cells are excluded from rendering, and build the
            // filtered→original index mapping. Collapsed run starts render a
            // synthetic summary cell; those few summaries are materialized
            // up front so the ref list can borrow from a stable Vec —
            // avoiding the per-frame deep clone of every visible cell that
            // this path used to pay (#3896).
            let summary_cells: Vec<(usize, HistoryCell)> = tool_runs
                .iter()
                .filter(|run| collapsed_run_starts.contains(&run.start))
                .map(|run| (run.start, tool_run_summary_cell(run)))
                .collect();
            let summary_cell_for = |idx: usize| -> Option<&HistoryCell> {
                summary_cells
                    .iter()
                    .find(|(start, _)| *start == idx)
                    .map(|(_, cell)| cell)
            };

            let mut filtered_cells: Vec<&HistoryCell> =
                Vec::with_capacity(history_len + active_entries.len());
            let mut filtered_revs: Vec<u64> =
                Vec::with_capacity(history_len + active_entries.len());
            let mut filtered_to_original: Vec<usize> =
                Vec::with_capacity(history_len + active_entries.len());

            for (idx, cell) in app.history.iter().enumerate() {
                if superseded_todos.contains(&idx) {
                    continue;
                }
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
                    filtered_cells.push(summary_cell_for(idx).expect("summary cell materialized"));
                    filtered_revs.push(tool_run_summary_revision(
                        run,
                        &app.history_revisions,
                        history_len,
                        app.active_cell_revision,
                    ));
                    filtered_to_original.push(idx);
                    continue;
                }
                filtered_cells.push(cell);
                filtered_revs.push(history_entry_revision(app.history_revisions[idx]));
                filtered_to_original.push(idx);
            }

            if !active_entries.is_empty() {
                let active_rev = app.active_cell_revision;
                for (i, cell) in active_entries.iter().enumerate() {
                    let original_idx = history_len + i;
                    if superseded_todos.contains(&original_idx) {
                        continue;
                    }
                    if app.collapsed_cells.contains(&original_idx) {
                        continue;
                    }
                    if collapsed_tool_indices.contains(&original_idx) {
                        continue;
                    }
                    if let Some(run) = tool_runs.iter().find(|run| {
                        run.start == original_idx && collapsed_run_starts.contains(&original_idx)
                    }) {
                        filtered_cells
                            .push(summary_cell_for(original_idx).expect("summary materialized"));
                        filtered_revs.push(tool_run_summary_revision(
                            run,
                            &app.history_revisions,
                            history_len,
                            active_rev,
                        ));
                        filtered_to_original.push(original_idx);
                        continue;
                    }
                    filtered_cells.push(cell);
                    let salt = (i as u64).wrapping_add(1);
                    filtered_revs.push(active_entry_revision(active_rev, salt));
                    filtered_to_original.push(original_idx);
                }
            }

            app.collapsed_cell_map = filtered_to_original;

            app.viewport.transcript_cache.ensure_filtered(
                &filtered_cells,
                &filtered_revs,
                content_area.width.max(1),
                render_options,
                &app.folded_thinking,
                Some(&app.collapsed_cell_map),
                provisional_action_owner,
            );
        }

        let (mut total_lines, mut top, mut was_explicit_tail) =
            resolve_transcript_viewport_after_layout(&mut app.viewport, visible_lines);

        // A sticky prompt is layout chrome, not a synthetic transcript row.
        // First resolve against the full viewport, then reserve one real row
        // only when the prompt has actually scrolled above it. Resolving once
        // more with the smaller body keeps the newest tail line visible.
        let mut transcript_area = content_area;
        let pinned_prompt = (app.pin_last_prompt && content_area.height > 1)
            .then(|| {
                scrolled_user_prompt_pin(
                    &app.history,
                    app.viewport.transcript_cache.line_meta(),
                    &app.collapsed_cell_map,
                    top,
                    content_area.width,
                )
            })
            .flatten();
        let visible_lines = if pinned_prompt.is_some() {
            transcript_area.y = transcript_area.y.saturating_add(1);
            transcript_area.height = transcript_area.height.saturating_sub(1);
            let visible = usize::from(transcript_area.height);
            (total_lines, top, was_explicit_tail) =
                resolve_transcript_viewport_after_layout(&mut app.viewport, visible);
            visible
        } else {
            visible_lines
        };
        let owner = app.transcript_action_owner();
        let index_map = has_collapsed.then_some(app.collapsed_cell_map.as_slice());
        app.viewport.transcript_cache.retarget(owner, index_map);

        // The cache has now observed this revision (or the cell was filtered,
        // in which case a later reveal must cold-render). Start the next append
        // receipt from the current revision instead of chaining across an
        // already-consumed proof.
        if let Some(receipt) = app.streaming_source_receipt.as_mut() {
            receipt.from_revision = receipt.to_revision;
        }

        let line_meta = app.viewport.transcript_cache.line_meta();

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

        app.viewport.last_transcript_area = Some(transcript_area);
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
        let mut line_links = if total_lines == 0 {
            vec![Vec::new()]
        } else {
            app.viewport.transcript_cache.line_links()[top..end].to_vec()
        };

        if !app.low_motion
            && app.fancy_animations
            && let (Some(start), Some(started)) = (
                app.ocean_receipt_settle_start,
                app.ocean_completion_started_at,
            )
        {
            apply_receipt_settle_cascade(
                &mut lines,
                top,
                line_meta,
                &app.collapsed_cell_map,
                &app.history,
                start,
                started.elapsed().as_millis(),
            );
        }

        // Brief flash highlight on the most recently sent user message. It is
        // a one-shot transition, so Reduced/Still clear the timestamp instead
        // of leaving a stale flash waiting for a later state-change redraw.
        if app.motion_policy().allows_decorative() {
            if let Some(send_at) = app.last_send_at {
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
        } else {
            app.last_send_at = None;
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

        if let Some(pin) = pinned_prompt {
            lines.insert(0, pin);
            line_links.insert(0, Vec::new());
        }

        // The HTML contract is a top-first ledger. Bottom-padding the short
        // transcript made every newly wrapped stream line shift all prior
        // rows upward, producing repeated thousand-cell repaints and the
        // visible "slab" motion recorded in live QA. Empty-state centering is
        // handled separately; active work starts at the top and appends in
        // place until scrolling is genuinely necessary.
        app.viewport.last_transcript_padding_top = 0;

        let scrollbar = (total_lines > visible_lines && transcript_area.width > 1).then_some(
            TranscriptScrollbar {
                top,
                visible: visible_lines,
                total: total_lines,
            },
        );
        let jump_to_latest_button =
            if app.use_mouse_capture && !app.viewport.transcript_scroll.is_at_tail() {
                jump_to_latest_button_rect(transcript_area, scrollbar.is_some())
            } else {
                None
            };
        app.viewport.jump_to_latest_button_area = jump_to_latest_button;

        Self {
            content_area,
            transcript_area,
            lines,
            line_links,
            scrollbar,
            jump_to_latest_button,
            background,
            ocean_column,
            ocean_activity,
            ambient_inks,
            ocean_elapsed_ms,
            ocean_animated,
            life_presence_fixed,
            fish_flee_elapsed_ms,
            // Fish also accompany intentional transcript browsing in the
            // selected underwater scene. They only occupy blank cells and are
            // collision-checked, so history stays legible while the ocean
            // remains playful when scrolling upward.
            ambient_life: underwater_atmosphere
                && !app.attention_hold_active()
                && (browsing_history
                    || matches!(phase, ShellPhase::Working | ShellPhase::Verifying)
                    || completion_life_active),
            scroll_track,
            scroll_thumb,
            jump_border,
            jump_arrow,
        }
    }

    /// Sample the water field against the full terminal instead of restarting
    /// it at the transcript's first row. Standalone widget callers keep the
    /// local column, which is useful for previews and focused tests.
    #[must_use]
    pub(crate) fn with_ocean_viewport(mut self, viewport: Rect) -> Self {
        self.ocean_column = self
            .ocean_column
            .map(|column| column.with_viewport(viewport));
        self
    }

    #[must_use]
    pub(crate) fn ocean_column(&self) -> Option<crate::tui::ocean::OceanColumn> {
        self.ocean_column
    }
}

fn apply_receipt_settle_cascade(
    lines: &mut [Line<'static>],
    top: usize,
    line_meta: &[TranscriptLineMeta],
    filtered_to_original: &[usize],
    history: &[HistoryCell],
    start: usize,
    elapsed_ms: u128,
) {
    for (visible_index, line) in lines.iter_mut().enumerate() {
        let Some((filtered_cell, _)) = line_meta
            .get(top + visible_index)
            .and_then(TranscriptLineMeta::cell_line)
        else {
            continue;
        };
        let original_cell = filtered_to_original
            .get(filtered_cell)
            .copied()
            .unwrap_or(filtered_cell);
        if original_cell < start
            || !matches!(
                history.get(original_cell),
                Some(HistoryCell::Tool(_) | HistoryCell::SubAgent(_))
            )
            || !receipt_is_settling(original_cell - start, elapsed_ms)
        {
            continue;
        }
        for span in &mut line.spans {
            span.style = span.style.add_modifier(Modifier::DIM);
        }
    }
}

#[must_use]
fn receipt_is_settling(receipt_order: usize, elapsed_ms: u128) -> bool {
    let delay = u128::try_from(receipt_order.min(6)).unwrap_or(6) * 70;
    elapsed_ms < delay + 140
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
        let cell_revision = revisions
            .get(idx)
            .copied()
            .map(history_entry_revision)
            .unwrap_or_else(|| {
                let active_idx = idx.saturating_sub(history_len);
                active_entry_revision(active_rev, (active_idx as u64).wrapping_add(1))
            });
        revision = revision.rotate_left(7) ^ cell_revision;
    }
    let extends_into_active = run.start.saturating_add(run.count) > history_len;
    revision_in_domain(revision, extends_into_active)
}

const ACTIVE_REVISION_DOMAIN: u64 = 1 << 63;

fn revision_in_domain(revision: u64, active: bool) -> u64 {
    // The top bit is exclusively a cache-domain tag. Clearing it means raw
    // counters that differ only by bit 63 can theoretically alias within one
    // domain after 2^63 updates; that lifetime is acceptable, while active and
    // committed-history keys must never alias each other.
    let payload = revision & !ACTIVE_REVISION_DOMAIN;
    if active {
        ACTIVE_REVISION_DOMAIN | payload
    } else {
        payload
    }
}

fn history_entry_revision(revision: u64) -> u64 {
    revision_in_domain(revision, false)
}

pub(crate) fn active_entry_revision(active_rev: u64, salt: u64) -> u64 {
    // Active entries and committed history cells can occupy the same
    // positional cache slot across `flush_active_cell`. Keep their revision
    // domains distinct so the first active entry (`active_rev = 0`,
    // `salt = 1`) cannot collide with the first history revision (`1`) and
    // reuse a stale `running` render after cancellation.
    let mixed = active_rev
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(salt);
    revision_in_domain(mixed, true)
}

/// Build the last-user-prompt header when that message is above the resolved
/// transcript viewport. The caller owns the one-row layout reservation so
/// the header never masquerades as `top` or displaces the newest tail line.
fn scrolled_user_prompt_pin(
    history: &[HistoryCell],
    line_meta: &[TranscriptLineMeta],
    collapsed_cell_map: &[usize],
    top: usize,
    width: u16,
) -> Option<Line<'static>> {
    if width == 0 {
        return None;
    }
    let (orig_idx, content) =
        history
            .iter()
            .enumerate()
            .rev()
            .find_map(|(idx, cell)| match cell {
                HistoryCell::User { content } if !content.trim().is_empty() => {
                    Some((idx, content.as_str()))
                }
                _ => None,
            })?;
    let first_line = line_meta.iter().position(|meta| match meta {
        TranscriptLineMeta::CellLine {
            cell_index,
            line_in_cell,
            ..
        } => {
            let original = collapsed_cell_map
                .get(*cell_index)
                .copied()
                .unwrap_or(*cell_index);
            original == orig_idx && *line_in_cell == 0
        }
        _ => false,
    });
    let first_line = first_line?;
    if first_line >= top {
        return None;
    }

    let first = content.lines().next().unwrap_or("").trim();
    if first.is_empty() {
        return None;
    }
    let budget = usize::from(width.saturating_sub(4)).max(1);
    let mut shown = String::new();
    let mut used = 0usize;
    for ch in first.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > budget {
            break;
        }
        shown.push(ch);
        used += w;
    }
    if used < UnicodeWidthStr::width(first) && !shown.is_empty() {
        shown.push('…');
    }

    Some(Line::from(vec![
        Span::styled(
            format!("{} ", crate::tui::glyphs::USER),
            Style::default()
                .fg(palette::WHALE_HUMAN)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(shown, Style::default().fg(palette::TEXT_PRIMARY)),
    ]))
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

        self.render_underwater_field(area, buf);

        // Link targets travel beside the wrapped lines, never inside Span
        // content. Convert relative line columns to absolute viewport regions
        // for the backend; clip the final column when a scrollbar owns it.
        let link_area = Rect {
            width: area
                .width
                .saturating_sub(u16::from(self.scrollbar.is_some())),
            ..area
        };
        let regions = crate::tui::osc8::link_regions_for_lines(link_area, &self.line_links);
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
                .render(self.transcript_area, buf, &mut state);
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

        // Hover: register OSC-8 link regions (copyable), then apply aura.
        let link_area = Rect {
            width: area
                .width
                .saturating_sub(u16::from(self.scrollbar.is_some())),
            ..area
        };
        for region in crate::tui::osc8::link_regions_for_lines(link_area, &self.line_links) {
            let width = region
                .col_end
                .saturating_sub(region.col_start)
                .saturating_add(1);
            let hit = Rect::new(region.col_start, region.row, width, 1);
            crate::tui::hover_layer::register_rect(
                crate::tui::hover_hit::HoverTargetKind::Link,
                hit,
                region.target,
                true,
            );
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

impl ChatWidget {
    /// Paint the explicitly selected underwater field. Flat keeps the theme
    /// surface, Solarized Light keeps canonical Base3, and Terminal keeps its
    /// inherited background without inherited aquarium decoration.
    fn render_underwater_field(&self, area: Rect, buf: &mut Buffer) {
        if let Some(column) = self.ocean_column {
            // Cache per-row ocean colors; invalidate only on phase/size/breath.
            let phase_tag = column.phase_tag();
            let fingerprint = column.ramp_fingerprint();
            let ramp = crate::tui::ambient_life::frame_ocean_ramp(
                &column,
                area.height,
                area.y,
                self.ocean_elapsed_ms,
                phase_tag,
                fingerprint,
            );
            for local_y in 0..area.height {
                let protected = self
                    .lines
                    .get(usize::from(local_y))
                    .and_then(occupied_text_bounds);
                let row_bg = ramp
                    .get(usize::from(local_y))
                    .copied()
                    .unwrap_or_else(|| column.color_at_y(area.y.saturating_add(local_y)));
                for local_x in 0..area.width {
                    let is_protected = protected.is_some_and(|(start, end)| {
                        usize::from(local_x) >= start && usize::from(local_x) < end
                    });
                    let cell = &mut buf[(area.x + local_x, area.y + local_y)];
                    // Plain transcript text participates in the water column;
                    // explicit semantic surfaces (selection, code, warnings)
                    // retain their own background.
                    if !is_protected || cell.bg == self.background {
                        cell.set_bg(row_bg);
                    }
                }
            }
        }

        if self.ambient_life
            && let Some(inks) = self.ambient_inks
        {
            // The scatter has a centre. It used to be column 0 with a row in
            // the middle of the field, which is neither where the school
            // swims nor anywhere the eye is: the flee proximity test
            // (|dy| < 6) could not even fire on a tall field, and when it did
            // every fish was to the right of the anchor so the whole school
            // slid the same way. Anchored on the composer's centre line and
            // the school's own band, a turn beginning reads as the shoal
            // parting around the thing that just happened.
            let cursor = crate::tui::ambient_life::AmbientCursor {
                column: area.x.saturating_add(area.width / 2),
                row: area
                    .y
                    .saturating_add(crate::tui::ambient_life::school_band_row(area)),
                flee_elapsed_ms: self.fish_flee_elapsed_ms,
            };
            // Whale cameo rides the completion breath clock when present.
            let whale = crate::tui::ambient_life::WhaleCameo {
                elapsed_ms: self.ocean_column.and_then(|c| c.completion_elapsed_ms()),
                anchor_x: area.x.saturating_add(area.width / 2),
                anchor_y: area.y.saturating_add(area.height.saturating_mul(2) / 3),
            };
            // Per-frame budget counters (built/painted/skipped/clipped);
            // consumed by ambient-life tests and debug tooling, not by the
            // widget itself.
            let _ambient_stats = crate::tui::ambient_life::render_ambient_life(
                area,
                buf,
                inks,
                &self.lines,
                self.ocean_elapsed_ms,
                self.ocean_presence_f32(),
                cursor,
                whale,
                self.ocean_activity,
            );
            if let Some(column) = self.ocean_column {
                crate::tui::ambient_life::apply_caustic_shimmer(
                    area,
                    buf,
                    &column,
                    self.ocean_elapsed_ms,
                    self.ocean_animated,
                    &self.lines,
                );
            }
        }
    }
}

impl ChatWidget {
    /// Life presence as a 0..=1 fraction; drives ambient-life ink fading.
    fn ocean_presence_f32(&self) -> f32 {
        (f32::from(self.life_presence_fixed) / 1000.0).clamp(0.0, 1.0)
    }
}

fn occupied_text_bounds(line: &Line<'_>) -> Option<(usize, usize)> {
    crate::tui::ambient_life::occupied_text_bounds(line)
}

#[cfg(test)]
fn fish_flee_offset(elapsed_ms: u128) -> u16 {
    crate::tui::ambient_life::fish_flee_offset(elapsed_ms)
}

#[cfg(test)]
fn fish_mark(facing_right: bool) -> &'static str {
    if facing_right { "><>" } else { "<><" }
}

#[cfg(test)]
fn fish_heading(previous_x: u16, current_x: u16, next_x: u16, fallback_right: bool) -> bool {
    if next_x != current_x {
        next_x > current_x
    } else if current_x != previous_x {
        current_x > previous_x
    } else {
        fallback_right
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

const COMPOSER_PROMPT_GUTTER_WIDTH: u16 = 2;
const COMPOSER_PANEL_MIN_WIDTH: u16 = 12;

/// Whether the active composer should use its full rounded enclosure.
///
/// `composer_border` is a legacy configuration name, but its compatibility
/// policy is deliberate: the default `true` means the Tideline enclosure;
/// `false` is an explicit compact/quiet opt-out. Keep every layout consumer
/// behind this helper so the reserved floor, measured height, and rendered
/// geometry cannot drift apart.
#[must_use]
pub(crate) fn composer_enclosure_enabled(app: &App) -> bool {
    app.composer_border
}

/// Shared `[↑]` submit rect for the live composer, or `None` when the
/// enclosure cannot host the three-cell affordance.
///
/// The gate is the same `enclosed_composer_panel_fits` predicate the painter
/// uses: a hitbox without the painted panel would be an invisible click
/// target (widths 6–11 rendered a borderless rule while still accepting
/// clicks).
#[must_use]
pub(crate) fn active_composer_submit_rect(app: &App, area: Rect) -> Option<Rect> {
    if !enclosed_composer_panel_fits(composer_enclosure_enabled(app), area.width, area.height) {
        return None;
    }
    Some(crate::tui::composer_chrome::tideline_composer_geometry(area).submit)
}

/// Restore the rounded corners after the semantic top/bottom passes.
///
/// Ratatui renders a `TOP`-only (or `BOTTOM`-only) block through the corner
/// cells as horizontal line glyphs. The live composer needs those passes for
/// its localized titles and independent permission/mode color ramps, so put
/// the four rounded joins back afterward rather than replacing its mature
/// input widget with the unfinished translation scaffold.
fn render_composer_panel_corners(
    area: Rect,
    buf: &mut Buffer,
    background: Style,
    permission_color: Color,
    mode_color: Color,
) {
    let top_style = background.fg(permission_color);
    let bottom_style = background.fg(mode_color);
    let left = area.left();
    let right = area.right().saturating_sub(1);
    let top = area.top();
    let bottom = area.bottom().saturating_sub(1);

    buf[(left, top)].set_symbol("╭").set_style(top_style);
    buf[(right, top)].set_symbol("╮").set_style(top_style);
    buf[(left, bottom)].set_symbol("╰").set_style(bottom_style);
    buf[(right, bottom)].set_symbol("╯").set_style(bottom_style);
}

/// Whether the outer composer rect can carry both semantic border rows.
///
/// Keep this policy in outer-area coordinates. Input wrapping subtracts the
/// prompt gutter later; using that narrower text width here made 12- and
/// 13-column composers render as panels after reserving only the quiet rule.
fn enclosed_composer_panel_fits(show_panel: bool, area_width: u16, area_height: u16) -> bool {
    show_panel && area_height >= 3 && area_width >= COMPOSER_PANEL_MIN_WIDTH
}

/// Border-aware input plane for the active composer.
///
/// The shared shell's `[↑]` control occupies three cells on the inner row.
/// Keep the text plane to its left, with one blank cell in between, so input
/// wrapping, cursor placement, and pointer mapping cannot claim painted send
/// cells. The outer block still owns the trailing breathing cell before its
/// right rail.
fn composer_inner_area(area: Rect, has_panel: bool) -> Rect {
    let inner = if has_panel {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .inner(area)
    } else if area.height >= 2 {
        Block::default().borders(Borders::TOP).inner(area)
    } else {
        area
    };
    if !has_panel {
        return inner;
    }

    let shell = crate::tui::composer_chrome::tideline_composer_geometry(area);
    Rect {
        width: shell.content.right().saturating_sub(inner.x),
        ..inner
    }
}

/// Canonical horizontal geometry for composer input text.
///
/// The prompt glyph occupies the first gutter column and the second column is
/// breathing room. Every consumer that wraps or maps input must use
/// `text_area`: rendering and cursor placement, viewport scroll bookkeeping,
/// and mouse hit-to-character conversion. Keeping the inset here prevents the
/// first typed character and exact wrap boundaries from using different
/// effective widths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ComposerContentGeometry {
    pub(crate) text_area: Rect,
    pub(crate) prompt_inset: u16,
}

impl ComposerContentGeometry {
    #[must_use]
    pub(crate) fn text_width(self) -> usize {
        usize::from(self.text_area.width.max(1))
    }

    #[must_use]
    fn prompt_padding(self) -> &'static str {
        if self.prompt_inset == COMPOSER_PROMPT_GUTTER_WIDTH {
            "  "
        } else {
            ""
        }
    }

    #[must_use]
    fn prompt_x(self) -> Option<u16> {
        (self.prompt_inset > 0).then(|| self.text_area.x.saturating_sub(self.prompt_inset))
    }
}

#[must_use]
pub(crate) fn composer_content_geometry(
    inner_area: Rect,
    history_search_active: bool,
) -> ComposerContentGeometry {
    let prompt_inset = if !history_search_active
        && inner_area.width >= COMPOSER_PROMPT_GUTTER_WIDTH.saturating_add(1)
    {
        COMPOSER_PROMPT_GUTTER_WIDTH
    } else {
        0
    };
    ComposerContentGeometry {
        text_area: Rect {
            x: inner_area.x.saturating_add(prompt_inset),
            y: inner_area.y,
            width: inner_area.width.saturating_sub(prompt_inset),
            height: inner_area.height,
        },
        prompt_inset,
    }
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

    fn wants_enclosed_panel(&self) -> bool {
        composer_enclosure_enabled(self.app)
    }

    pub(crate) fn has_panel(&self, area: Rect) -> bool {
        enclosed_composer_panel_fits(self.wants_enclosed_panel(), area.width, area.height)
    }

    /// The border- and submit-aware input rectangle shared by rendering,
    /// cursor mapping, and the frame's persistent mouse geometry.
    pub(crate) fn inner_area(&self, area: Rect) -> Rect {
        composer_inner_area(area, self.has_panel(area))
    }

    fn mode_color(&self) -> Color {
        match self.app.mode {
            AppMode::Agent => self.app.ui_theme.mode_agent,
            AppMode::Plan => self.app.ui_theme.mode_plan,
            AppMode::Operate => self.app.ui_theme.mode_operate,
        }
    }

    fn max_height_cap(&self) -> u16 {
        composer_max_height(self.app.composer_density)
    }
}

impl Renderable for ComposerWidget<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Slash rows are re-recorded below; clear first so a closed or
        // resized menu cannot keep stale hitboxes from the prior frame.
        self.app
            .viewport
            .last_slash_menu_hitboxes
            .borrow_mut()
            .clear();
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
        // Menu rows span the full inner panel. Input text alone uses the
        // prompt-adjusted geometry below.
        let content_width = usize::from(inner_area.width.max(1));
        let content_geometry =
            composer_content_geometry(inner_area, self.app.is_history_search_active());
        let input_content_width = content_geometry.text_width();

        // Use the extended version that also returns character indices to avoid
        // redundant wrapping when rendering text selections (issue #3909).
        let (visible_lines, _cursor_row, _cursor_col, _scroll_offset, visible_char_indices) =
            layout_input_with_scroll_and_char_indices(
                input_text,
                input_cursor,
                input_content_width,
                input_rows_budget,
            );
        if has_panel {
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
                Some(Line::from(Span::styled(
                    self.app
                        .tr(crate::localization::MessageId::ComposerSlashMenuHint),
                    Style::default().fg(self.app.ui_theme.text_hint),
                )))
            } else if !input_text.trim().is_empty() {
                composer_submit_hint(self.app).map(|hint| {
                    Line::from(vec![Span::styled(
                        format!(" {} ", hint.text),
                        Style::default().fg(hint.color),
                    )])
                })
            } else {
                None
            };

            // Warm permission ramp: Ask is amber, Auto-Review is Signal Gold,
            // and Full Access is coral. The bottom edge independently carries
            // the cool Plan -> Act -> Operate mode ramp.
            let permission_color = match self.app.approval_mode {
                ApprovalMode::Suggest | ApprovalMode::Never => self.app.ui_theme.permission_ask,
                ApprovalMode::Auto => self.app.ui_theme.permission_auto_review,
                ApprovalMode::Bypass => self.app.ui_theme.permission_full_access,
            };
            // Paint the enclosure first so the live composer gets actual
            // rounded side rails. The semantic top/bottom blocks below keep
            // their existing permission/mode color ramps and titles while the
            // neutral rails stay legible on every supported theme.
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(self.app.ui_theme.border))
                .style(background)
                .render(area, buf);
            let mut top_border = Block::default()
                .borders(Borders::TOP)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(permission_color))
                .style(background);
            if self.app.is_history_search_active() {
                top_border = top_border.title(Line::from(Span::styled(
                    format!(
                        " {} ",
                        self.app
                            .tr(crate::localization::MessageId::HistorySearchTitle)
                    ),
                    Style::default().fg(palette::TEXT_MUTED),
                )));
            }
            // Agent focus chip: the composer names the fork it addresses so
            // a message never goes to a worker by surprise.
            if let Some(chip) = crate::tui::agent_focus::composer_chip_text(self.app) {
                top_border = top_border.title_top(
                    Line::from(Span::styled(
                        format!(" {chip} "),
                        Style::default()
                            .fg(self.app.ui_theme.accent_action)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .right_aligned(),
                );
            }
            top_border.render(area, buf);

            let mut bottom_border = Block::default()
                .borders(Borders::BOTTOM)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(self.mode_color()))
                .style(background);
            if let Some(hint_line) = hint_line {
                bottom_border = bottom_border.title_bottom(hint_line);
            }
            bottom_border.render(area, buf);
            render_composer_panel_corners(
                area,
                buf,
                background,
                permission_color,
                self.mode_color(),
            );
        } else if area.height >= 2 {
            let mut block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(self.app.ui_theme.border))
                .style(background);
            if !input_text.trim().is_empty()
                && let Some(hint) = composer_submit_hint(self.app)
            {
                block = block.title(Line::from(Span::styled(
                    format!(" {} ", hint.text),
                    Style::default().fg(hint.color),
                )));
            }
            if let Some(chip) = crate::tui::agent_focus::composer_chip_text(self.app) {
                block = block.title_top(
                    Line::from(Span::styled(
                        format!(" {chip} "),
                        Style::default()
                            .fg(self.app.ui_theme.accent_action)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .right_aligned(),
                );
            }
            block.render(area, buf);
        } else {
            Block::default().style(background).render(area, buf);
        }

        let mut input_lines = Vec::new();
        if input_text.is_empty() {
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
                    Style::default().fg(self.app.ui_theme.text_soft),
                )
            };
            input_lines.push(Line::from(vec![
                Span::raw(content_geometry.prompt_padding()),
                Span::styled(placeholder, style),
            ]));
        } else if let Some((sel_start, sel_end)) = self.app.selection_range() {
            // Use the character indices we already computed during layout
            // to avoid redundant wrapping (issue #3909).
            let line_ranges: Vec<(usize, usize)> = visible_char_indices
                .iter()
                .map(|(start, text)| (*start, *start + text.chars().count()))
                .collect();
            for (line_text, (line_start, line_end)) in visible_lines.iter().zip(line_ranges.iter())
            {
                let mut spans = line_spans_with_selection(
                    line_text,
                    *line_start,
                    *line_end,
                    sel_start,
                    sel_end,
                    self.app.ui_theme.selection_bg,
                );
                if content_geometry.prompt_inset > 0 {
                    spans.insert(0, Span::raw(content_geometry.prompt_padding()));
                }
                input_lines.push(Line::from(spans));
            }
        } else {
            for line in &visible_lines {
                let mut spans = Vec::new();
                if content_geometry.prompt_inset > 0 {
                    spans.push(Span::raw(content_geometry.prompt_padding()));
                }
                spans.push(Span::styled(
                    line.clone(),
                    Style::default().fg(palette::TEXT_PRIMARY),
                ));
                input_lines.push(Line::from(spans));
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
            empty_composer_visual_rows(hint.as_deref(), input_content_width, input_rows_budget)
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
                        menu_style::selected_row_bg_style().fg(palette::SELECTION_TEXT)
                    } else {
                        Style::default().fg(palette::TEXT_MUTED)
                    };
                    let marker = crate::tui::glyphs::selection_marker(is_selected);
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
                    menu_style::selected_row_bg_style().fg(palette::SELECTION_TEXT)
                } else {
                    Style::default().fg(palette::TEXT_MUTED)
                };
                let marker = crate::tui::glyphs::selection_marker(is_selected);
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
                    menu_style::selected_row_bg_style().fg(palette::SELECTION_TEXT)
                } else {
                    Style::default().fg(palette::TEXT_MUTED)
                };
                let marker = crate::tui::glyphs::selection_marker(is_selected);

                // Name column
                let name_style = if entry.is_skill && !is_selected {
                    Style::default().fg(palette::WHALE_ACTION)
                } else {
                    sel_style
                };

                // Description column (muted when not selected, secondary when selected)
                let desc_style = if is_selected {
                    menu_style::selected_row_bg_style().fg(palette::SELECTION_TEXT)
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

                let name_was_truncated = display_name.width() > label_width;
                let mut name_display =
                    crate::tui::ui_text::truncate_line_to_width(&display_name, label_width);
                while name_display.width() < label_width {
                    name_display.push(' ');
                }

                // Skill marker prefix
                let skill_prefix = if entry.is_skill { "✦" } else { " " };

                // Compute exact prefix display width to avoid Paragraph wrap:
                // 1(" ") + 1(marker) + skill_prefix.width() + label_width + 2("  ")
                let prefix_display_width = 1 + 1 + skill_prefix.width() + label_width + 2;
                let desc_capacity = content_width.saturating_sub(prefix_display_width);
                let description_was_truncated = entry.description.width() > desc_capacity;
                let desc_display =
                    crate::tui::ui_text::truncate_line_to_width(&entry.description, desc_capacity);

                let row_line_index = lines.len();
                lines.push(Line::from(vec![
                    Span::styled(" ", Style::default()),
                    Span::styled(marker, sel_style),
                    Span::styled(skill_prefix, name_style),
                    Span::styled(name_display, name_style),
                    Span::styled("  ", desc_style),
                    Span::styled(desc_display, desc_style),
                ]));

                let row_y = inner_area
                    .y
                    .saturating_add(u16::try_from(row_line_index).unwrap_or(u16::MAX));
                if row_y < inner_area.bottom() && inner_area.width > 0 {
                    self.app
                        .viewport
                        .last_slash_menu_hitboxes
                        .borrow_mut()
                        .push((idx, Rect::new(inner_area.x, row_y, inner_area.width, 1)));
                }

                if name_was_truncated || description_was_truncated {
                    let full_text = if entry.description.trim().is_empty() {
                        display_name
                    } else {
                        format!("{display_name}  {}", entry.description)
                    };
                    if row_y < inner_area.bottom() {
                        crate::tui::hover_layer::register_rect(
                            crate::tui::hover_hit::HoverTargetKind::TruncatedText,
                            Rect::new(inner_area.x, row_y, inner_area.width, 1),
                            full_text,
                            false,
                        );
                    }
                }
            }
        }

        let paragraph = Paragraph::new(lines)
            .style(background)
            .wrap(Wrap { trim: false });
        paragraph.render(inner_area, buf);

        // The prompt is a persistent focus anchor, not empty-state chrome.
        // Rendering it on every input row keeps the first character from
        // causing a visible leftward jump.
        if let Some(prompt_x) = content_geometry.prompt_x()
            && let Some((cursor_x, cursor_y)) = self.cursor_pos(area)
        {
            debug_assert!(cursor_x >= content_geometry.text_area.x);
            buf[(prompt_x, cursor_y)]
                .set_symbol("❯")
                .set_style(Style::default().fg(self.app.ui_theme.accent_primary));
        }

        // Restore the shared `[↑]` after caller-owned input so a long draft
        // cannot erase the one cell target the mouse handler also uses.
        if has_panel {
            crate::tui::composer_chrome::render_tideline_composer_submit(
                area,
                buf,
                &self.app.ui_theme,
                true,
                crate::tui::color_compat::ascii_safe_enabled(),
            );
        }
    }

    fn desired_height(&self, width: u16) -> u16 {
        composer_height(
            self.app.composer_display_input(),
            width,
            self.max_height.min(self.max_height_cap()),
            self.active_menu_reserved_rows(),
            self.app.composer_density,
            self.wants_enclosed_panel(),
        )
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        let inner_area = self.inner_area(area);
        let input_text = self.app.composer_display_input();
        let input_cursor = self.app.composer_display_cursor();
        let content_geometry =
            composer_content_geometry(inner_area, self.app.is_history_search_active());
        let input_content_width = content_geometry.text_width();
        // Match the render path's locked-budget calculation so the cursor
        // lands on the same row the input is drawn on.
        let input_rows_budget =
            composer_input_rows_budget(inner_area.height, self.active_menu_reserved_rows());

        let (visible_lines, cursor_row, cursor_col) = layout_input(
            input_text,
            input_cursor,
            input_content_width,
            input_rows_budget,
        );
        let visual_rows = if input_text.is_empty() {
            let hint: Option<Cow<'_, str>> = if let Some(ref suggestion) =
                self.app.prompt_suggestion
                && !self.app.is_history_search_active()
            {
                Some(Cow::Borrowed(suggestion.as_str()))
            } else {
                Some(composer_empty_hint_text(self.app))
            };
            empty_composer_visual_rows(hint.as_deref(), input_content_width, input_rows_budget)
        } else {
            visible_lines.len()
        };
        let top_padding = composer_top_padding(visual_rows, input_rows_budget);

        let cursor_x = content_geometry
            .text_area
            .x
            .saturating_add(u16::try_from(cursor_col).unwrap_or(u16::MAX));
        let cursor_y = inner_area
            .y
            .saturating_add(u16::try_from(top_padding + cursor_row).unwrap_or(u16::MAX));
        if cursor_x < area.x + area.width && cursor_y < area.y + area.height {
            Some((cursor_x, cursor_y))
        } else {
            None
        }
    }
}

/// Compact, bottom-anchored approval card.
///
/// The widget reads its selected option and locale directly from the
/// [`ApprovalView`]. Rendering preserves transcript context while reserving
/// the complete action set and at least one load-bearing command/preview row
/// on ordinary terminal sizes.
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
        let repo_law = self.request.is_repo_law_prompt();
        let palette_colors = if repo_law {
            repo_law_approval_palette()
        } else {
            approval_palette(stakes)
        };
        let critical = matches!(stakes, crate::tui::approval::ApprovalStakes::Critical);

        let mut body: Vec<Line<'static>> = Vec::with_capacity(16);
        // Header: stakes badge + tool identifier.
        body.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(
                    " {} ",
                    if repo_law {
                        tr(locale, MessageId::ApprovalRepoLawBadge)
                    } else {
                        stakes_badge_text(stakes, locale)
                    }
                ),
                Style::default()
                    .fg(palette::WHALE_BG)
                    .bg(palette_colors.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                if repo_law {
                    format!(
                        "{} · {}",
                        tr(locale, MessageId::ApprovalRepoLawTitle),
                        self.request.tool_name
                    )
                } else {
                    self.request.tool_name.clone()
                },
                Style::default()
                    .fg(palette::WHALE_ACTION)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        if repo_law {
            body.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    "◆ ",
                    Style::default()
                        .fg(palette::STATUS_WARNING)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    tr(locale, MessageId::ApprovalRepoLawWarning),
                    Style::default()
                        .fg(palette::WHALE_ERROR)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            body.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    tr(locale, MessageId::ApprovalRepoLawRuleLabel),
                    Style::default().fg(palette::TEXT_HINT),
                ),
                Span::styled(
                    self.request.description.clone(),
                    Style::default().fg(palette::TEXT_SECONDARY),
                ),
            ]));
        }

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
                    // A short approval band has room for only one detail row
                    // before its truncation hint. Project the most useful
                    // command/change into that row instead of spending it on
                    // setup (`cd`, `set`) or diff metadata. The complete,
                    // original-order value remains available in the details
                    // pager.
                    let inline_shell_lines = prioritize_inline_shell_lines(
                        shell_lines,
                        is_change_preview,
                        area.height <= 24,
                    );
                    // Bound every multi-line preview so one huge command cannot
                    // grow the band without limit; the details chord opens the rest.
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
                        &inline_shell_lines,
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
        // everything stays one details chord away in the pager. Keep a single
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

        // Preview the validated persistent-rule candidates. Informational, so
        // they live in the scrollable body rather than the action rows.
        if let Some(preview) = self.request.ask_rule_save_preview() {
            push_permission_rule_save_preview(
                &mut body,
                &preview,
                palette_colors.shortcut,
                area.width,
            );
        }
        if let Some(preview) = self.request.allow_rule_save_preview() {
            push_permission_rule_save_preview(
                &mut body,
                &preview,
                palette_colors.shortcut,
                area.width,
            );
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
            self.view.set_mouse_hitboxes(Vec::new());
            let bar_y = area.y.saturating_add(area.height.saturating_sub(1));
            let bar_area = Rect::new(area.x, bar_y, area.width, 1);
            Clear.render(bar_area, buf);

            let stakes = self.request.stakes();
            let repo_law = self.request.is_repo_law_prompt();
            let palette_colors = if repo_law {
                repo_law_approval_palette()
            } else {
                approval_palette(stakes)
            };
            let summary = format!(
                " {} — {}  [Tab to expand] ",
                if repo_law {
                    tr(self.view.locale(), MessageId::ApprovalRepoLawTitle)
                } else {
                    Cow::Borrowed(self.request.tool_name.as_str())
                },
                if repo_law {
                    tr(self.view.locale(), MessageId::ApprovalRepoLawBadge)
                } else {
                    stakes_badge_text(stakes, self.view.locale())
                },
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
        let repo_law = self.request.is_repo_law_prompt();
        let palette_colors = if repo_law {
            repo_law_approval_palette()
        } else {
            approval_palette(stakes)
        };
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
        let rule_glyph = if repo_law { "═" } else { "─" };
        let rule: String = rule_glyph.repeat(region.width as usize);
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

        let mut hitboxes = Vec::new();
        let option_count =
            approval_options_for_request(self.request, self.request.risk, self.view.locale()).len();
        for index in 0..option_count {
            let first_line = 1 + index;
            let y_offset = measure_wrapped_rows(&controls[..first_line], region.width);
            let next_offset = measure_wrapped_rows(&controls[..first_line + 1], region.width);
            let y = control_rect.y.saturating_add(y_offset);
            let height = next_offset.saturating_sub(y_offset).min(
                control_rect
                    .y
                    .saturating_add(control_rect.height)
                    .saturating_sub(y),
            );
            if height > 0 {
                hitboxes.push(Rect::new(control_rect.x, y, control_rect.width, height));
            }
        }
        self.view.set_mouse_hitboxes(hitboxes);

        let body_rows = measure_wrapped_rows(&body, region.width);
        if body_rows > body_height && body_height > 0 {
            // Body does not fit (short terminal): show as much as we can and
            // point at the params pager through the platform-aware details chord.
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
    }

    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

/// Bottom-anchored band the inline approval prompt occupies within `area`.
/// Sized to the measured content, capped to half the frame like the compact
/// permission surfaces in peer coding agents, and always tall enough to show
/// the reserved controls (#3799). Full details remain available through the
/// platform-aware details chord.
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
    // Never shrink below the rule + controls. At normal terminal heights,
    // reserve four body rows: header, detail label, at least one command or
    // preview row, and the truncation hint. Half a viewport is the preferred
    // cap; up to four fifths is allowed only when necessary to retain that
    // load-bearing preview on a short frame. The extra permanent-grant row
    // needs one more reserved line than the legacy four-action card. Truly
    // tiny frames prioritize the complete action set and details chord.
    let controls_floor = 1u16.saturating_add(control_rows).min(area.height);
    let preview_rows = if area.height >= 16 {
        body_rows.min(4)
    } else {
        0
    };
    let preview_floor = controls_floor.saturating_add(preview_rows).min(area.height);
    let preferred_cap = area.height.div_ceil(2);
    let short_frame_cap = area.height.saturating_mul(4).div_ceil(5);
    let max_height = preferred_cap
        .max(preview_floor.min(short_frame_cap))
        .max(controls_floor)
        .min(area.height);
    let min_height = controls_floor;
    let height = desired.clamp(min_height, max_height);
    Rect {
        x: area.x,
        y: area.y.saturating_add(area.height.saturating_sub(height)),
        width,
        height,
    }
}

/// Terminal rows `lines` occupy under the exact ratatui word-wrap used by the
/// renderer. Exact measurement keeps localized controls and their mouse
/// hitboxes aligned without padding the compact approval band.
fn measure_wrapped_rows(lines: &[Line<'static>], width: u16) -> u16 {
    if width == 0 {
        return lines.len() as u16;
    }
    let rows = Paragraph::new(lines.to_vec())
        .wrap(Wrap { trim: false })
        .line_count(width);
    u16::try_from(rows).unwrap_or(u16::MAX)
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
    let mut controls: Vec<Line<'static>> = Vec::with_capacity(6);
    controls.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            approval_proceed_question(locale),
            Style::default()
                .fg(palette::TEXT_BODY)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    let options = approval_options_for_request(request, risk, locale);
    for (i, opt) in options.iter().enumerate() {
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
    controls
}

fn approval_proceed_question(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => "是否继续？",
        _ => "Do you want to proceed?",
    }
}

fn approval_truncation_hint(locale: Locale) -> Cow<'static, str> {
    let details = crate::tui::shell_key_routing::tool_details_chord();
    Cow::Owned(tr(locale, MessageId::ApprovalTruncationHint).replace("{details}", details.as_ref()))
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
            accent: palette::WHALE_HUMAN,
            shortcut: palette::WHALE_ACTION,
        },
        // Ordinary state-touching work: a calm ask, not an alarm.
        ApprovalStakes::Elevated => ApprovalColors {
            border: palette::WHALE_HUMAN,
            accent: palette::WHALE_HUMAN,
            shortcut: palette::WHALE_ACTION,
        },
        ApprovalStakes::Critical => ApprovalColors {
            border: palette::WHALE_ERROR,
            accent: palette::WHALE_ERROR,
            shortcut: palette::STATUS_WARNING,
        },
    }
}

fn repo_law_approval_palette() -> ApprovalColors {
    ApprovalColors {
        border: palette::STATUS_WARNING,
        accent: palette::WHALE_ERROR,
        shortcut: palette::STATUS_WARNING,
    }
}

fn approval_selected_style() -> Style {
    menu_style::selected_row_style()
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
        ToolCategory::McpRead => palette::WHALE_ACTION,
        ToolCategory::McpAction => palette::STATUS_WARNING,
        ToolCategory::Agent => palette::WHALE_ACTION,
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
                .fg(palette::WHALE_ACTION)
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

fn push_permission_rule_save_preview(
    lines: &mut Vec<Line<'static>>,
    preview: &crate::tui::approval::PermissionRuleSavePreview,
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
                .fg(palette::WHALE_ACTION)
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

/// Put one representative command/change first for compact inline rendering.
/// This is a display-only projection: approval parameters and the details
/// pager retain the exact original order.
fn prioritize_inline_shell_lines(
    command_lines: &[String],
    is_change_preview: bool,
    compact: bool,
) -> Vec<String> {
    if !compact || command_lines.len() < 2 {
        return command_lines.to_vec();
    }

    let representative = if is_change_preview {
        command_lines
            .iter()
            .enumerate()
            .max_by_key(|(index, line)| (preview_line_priority(line), std::cmp::Reverse(*index)))
            .map(|(index, _)| index)
    } else {
        command_lines
            .iter()
            .enumerate()
            .max_by_key(|(index, line)| (command_line_priority(line), std::cmp::Reverse(*index)))
            .map(|(index, _)| index)
    };
    let Some(representative) = representative.filter(|index| *index > 0) else {
        return command_lines.to_vec();
    };

    let mut projected = Vec::with_capacity(command_lines.len());
    projected.push(command_lines[representative].clone());
    projected.extend(
        command_lines
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != representative)
            .map(|(_, line)| line.clone()),
    );
    projected
}

fn preview_line_priority(line: &str) -> u8 {
    let trimmed = line.trim_start();
    if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
        4
    } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
        3
    } else if trimmed.starts_with("@@") {
        2
    } else if trimmed.starts_with("diff ")
        || trimmed.starts_with("---")
        || trimmed.starts_with("+++")
    {
        0
    } else {
        1
    }
}

fn command_line_priority(line: &str) -> u8 {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return 0;
    }

    let tokens = trimmed
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ';' | '|' | '&' | '(' | ')'))
        .filter(|token| !token.is_empty())
        .map(|token| token.rsplit('/').next().unwrap_or(token))
        .collect::<Vec<_>>();
    if tokens.iter().any(|token| {
        matches!(
            *token,
            "rm" | "rmdir"
                | "unlink"
                | "mv"
                | "dd"
                | "chmod"
                | "chown"
                | "kill"
                | "pkill"
                | "shutdown"
                | "reboot"
                | "mkfs"
        )
    }) || tokens.windows(2).any(|pair| {
        matches!(
            pair,
            ["git", "push"] | ["cargo", "publish"] | ["npm", "publish"]
        )
    }) || trimmed.contains('>')
    {
        return 4;
    }

    let first = tokens.first().copied().unwrap_or_default();
    if matches!(
        first,
        "cd" | "pushd" | "popd" | "set" | "export" | "unset" | "pwd" | ":" | "true"
    ) {
        1
    } else if matches!(first, "echo" | "printf") {
        2
    } else {
        3
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
    // Platform-aware details chord (⌥V on macOS, Alt+V elsewhere). Bare `v`
    // is never advertised as a details shortcut (TUI-DOG-002).
    let details = crate::tui::shell_key_routing::tool_details_chord();
    Cow::Owned(tr(locale, MessageId::ApprovalControlsHint).replace("{details}", details.as_ref()))
}

fn save_ask_rule_hint(locale: Locale) -> Cow<'static, str> {
    tr(locale, MessageId::ApprovalSaveAskRuleHint)
}

#[derive(Clone)]
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

/// Workflow elevated-plan card options (#4126): Approve / Edit plan / Cancel.
fn workflow_approval_options(risk: RiskLevel, locale: Locale) -> [ApprovalOptionRow; 3] {
    let dangerous = matches!(risk, RiskLevel::Destructive);
    [
        ApprovalOptionRow {
            label: workflow_option_approve(locale),
            key_hint: "1 / y",
            dangerous,
        },
        ApprovalOptionRow {
            label: workflow_option_edit_plan(locale),
            key_hint: "2 / e",
            dangerous: false,
        },
        ApprovalOptionRow {
            label: workflow_option_cancel(locale),
            key_hint: "3 / Esc",
            dangerous: false,
        },
    ]
}

fn approval_options_for_request(
    request: &ApprovalRequest,
    risk: RiskLevel,
    locale: Locale,
) -> Vec<ApprovalOptionRow> {
    if request.tool_name == "workflow" {
        workflow_approval_options(risk, locale).to_vec()
    } else {
        let mut options = approval_options_for(risk, locale).to_vec();
        if request.can_save_allow_rule() {
            options.insert(
                2,
                ApprovalOptionRow {
                    label: tr(locale, MessageId::ApprovalOptionAllowExactRepo),
                    key_hint: "p",
                    dangerous: false,
                },
            );
        }
        options
    }
}

fn workflow_option_approve(locale: Locale) -> Cow<'static, str> {
    match locale {
        Locale::ZhHans => Cow::Borrowed("批准"),
        _ => Cow::Borrowed("Approve"),
    }
}

fn workflow_option_edit_plan(locale: Locale) -> Cow<'static, str> {
    match locale {
        Locale::ZhHans => Cow::Borrowed("编辑计划"),
        _ => Cow::Borrowed("Edit plan"),
    }
}

fn workflow_option_cancel(locale: Locale) -> Cow<'static, str> {
    match locale {
        Locale::ZhHans => Cow::Borrowed("取消"),
        _ => Cow::Borrowed("Cancel"),
    }
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
    hitboxes: Option<&'a std::cell::RefCell<Vec<Rect>>>,
}

impl<'a> ElevationWidget<'a> {
    #[allow(dead_code)]
    pub fn new(request: &'a ElevationRequest, selected: usize, locale: Locale) -> Self {
        Self {
            request,
            selected,
            locale,
            hitboxes: None,
        }
    }

    pub fn new_with_hitboxes(
        request: &'a ElevationRequest,
        selected: usize,
        locale: Locale,
        hitboxes: &'a std::cell::RefCell<Vec<Rect>>,
    ) -> Self {
        Self {
            request,
            selected,
            locale,
            hitboxes: Some(hitboxes),
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
                        .fg(palette::WHALE_ACTION)
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

        let option_start = lines.len();
        for (i, option) in self.request.options.iter().enumerate() {
            let is_selected = i == self.selected;
            let style = if is_selected {
                menu_style::selected_row_bg_style().fg(palette::SELECTION_TEXT)
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

        if let Some(hitboxes) = self.hitboxes {
            hitboxes.borrow_mut().clear();
            let content = block.inner(popup_area);
            for i in 0..self.request.options.len() {
                let y = content
                    .y
                    .saturating_add(u16::try_from(option_start + i * 2).unwrap_or(u16::MAX));
                let height = 2u16.min(content.y.saturating_add(content.height).saturating_sub(y));
                if height > 0 {
                    hitboxes
                        .borrow_mut()
                        .push(Rect::new(content.x, y, content.width, height));
                }
            }
        }

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });

        paragraph.render(popup_area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
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

    let flash_bg = palette::SURFACE_TOOL_ACTIVE; // subtle dark-blue tint

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
            let mut grapheme_col = current_col;

            for grapheme in span_text.graphemes(true) {
                let grapheme_width = grapheme_display_width(grapheme);
                let grapheme_start = grapheme_col;
                let grapheme_end = grapheme_col.saturating_add(grapheme_width);
                if grapheme_end <= col_start {
                    before.push_str(grapheme);
                } else if grapheme_start >= col_end {
                    after.push_str(grapheme);
                } else {
                    selected.push_str(grapheme);
                }
                grapheme_col = grapheme_end;
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

/// The "fully idle" predicate: nothing in the transcript, nothing running,
/// nothing pending. It gates the idle ocean, and — because the idle ocean has
/// a row floor the layout has to respect — it also gates how many rows the
/// work rail is allowed to take. Evaluate it *once* per frame in
/// [`crate::tui::ui::render`] and thread the result, so the reservation and
/// the render can never disagree inside a single frame.
pub(crate) fn should_render_empty_state(app: &App) -> bool {
    let active_is_empty = app
        .active_cell
        .as_ref()
        .is_none_or(crate::tui::active_cell::ActiveCell::is_empty);
    app.history.is_empty()
        && active_is_empty
        && !app.is_loading
        && !app.is_compacting
        && !app.is_purging
        && !app.attention_hold_active()
        && !app
            .task_panel
            .iter()
            .any(|task| task.kind == crate::tui::app::TaskPanelEntryKind::Background)
        // Live work suppresses the empty state. On lock contention, treat
        // the todo store as non-empty rather than flash the empty ocean.
        && !app
            .todos
            .try_lock()
            .map(|todos| !todos.snapshot().is_empty())
            .unwrap_or(true)
        && app.goal.objective.is_none()
        && app.paused_goal_objective.is_none()
}

fn build_empty_state_lines(app: &App, area: Rect) -> Vec<Line<'static>> {
    crate::tui::underwater::empty_state_lines(app, area)
}

pub fn composer_input_rows_budget(inner_height: u16, extra_lines: usize) -> usize {
    usize::from(inner_height).saturating_sub(extra_lines).max(1)
}

fn composer_top_padding(content_lines: usize, rows_budget: usize) -> usize {
    crate::tui::composer_chrome::top_padding(content_lines, rows_budget)
}

/// Placeholder text shown when the composer input is empty.
#[cfg(test)]
const COMPOSER_PLACEHOLDER: &str = "Write a task or use /.";

/// How many visual rows the empty-input placeholder occupies after wrapping.
#[cfg(test)]
fn placeholder_visual_lines(content_width: usize) -> usize {
    placeholder_visual_lines_for(COMPOSER_PLACEHOLDER, content_width)
}

#[cfg(test)]
fn placeholder_visual_lines_for(placeholder: &str, content_width: usize) -> usize {
    wrap_text(placeholder, content_width).len().max(1)
}

pub(crate) fn composer_empty_hint_text(app: &App) -> Cow<'static, str> {
    if let Some(placeholder) = crate::tui::agent_focus::composer_placeholder(app) {
        Cow::Owned(placeholder)
    } else if app.is_history_search_active() {
        app.tr(MessageId::HistorySearchPlaceholder)
    } else if app.is_loading
        && !app.offline_mode
        && app.queued_draft.is_none()
        && !app.queued_messages.is_empty()
    {
        app.tr(MessageId::ComposerPlaceholderSendNow)
    } else if app.is_loading {
        app.tr(MessageId::ComposerPlaceholderFollowUp)
    } else {
        app.tr(MessageId::ComposerPlaceholder)
    }
}

/// Live label for what portable bare Enter will do with the current draft.
///
/// The quiet composer and the enclosed panel share this so the action is
/// visible before submit (#4703) without teaching internal "steer" vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerSubmitHint {
    pub text: String,
    pub color: Color,
}

pub(crate) fn composer_submit_hint(app: &App) -> Option<ComposerSubmitHint> {
    use crate::tui::app::{ComposerSubmitAction, ComposerSubmitChord, SubmitDisposition};

    let queue_count = app.queued_message_count();
    let (text, color) = match app.decide_composer_submit(ComposerSubmitChord::Enter) {
        ComposerSubmitAction::Submit(SubmitDisposition::Immediate) => {
            if queue_count == 0 {
                return None;
            }
            (
                app.tr(MessageId::ComposerHintSendWithQueue)
                    .replace("{count}", &queue_count.to_string()),
                palette::WHALE_ACTION,
            )
        }
        ComposerSubmitAction::Submit(SubmitDisposition::Queue)
        | ComposerSubmitAction::Submit(SubmitDisposition::QueueFollowUp) => {
            if app.offline_mode {
                let id = if app.onboarding_explore_offline {
                    MessageId::ComposerHintOfflineConnect
                } else {
                    MessageId::ComposerHintOfflineQueue
                };
                (app.tr(id).into_owned(), palette::STATUS_WARNING)
            } else if queue_count > 0 {
                (
                    app.tr(MessageId::ComposerHintQueueWithCount)
                        .replace("{count}", &queue_count.saturating_add(1).to_string()),
                    palette::WHALE_ACTION,
                )
            } else {
                (
                    app.tr(MessageId::ComposerHintQueue).into_owned(),
                    palette::WHALE_ACTION,
                )
            }
        }
        ComposerSubmitAction::Submit(SubmitDisposition::Steer) => (
            app.tr(MessageId::ComposerHintSendIntoTurn).into_owned(),
            palette::WHALE_ACTION,
        ),
        ComposerSubmitAction::SendQueuedNow => (
            app.tr(MessageId::ComposerHintSendNow).into_owned(),
            palette::WHALE_ACTION,
        ),
        ComposerSubmitAction::Noop => return None,
    };
    Some(ComposerSubmitHint { text, color })
}

pub(crate) fn empty_composer_visual_rows(
    _hint: Option<&str>,
    _content_width: usize,
    _rows_budget: usize,
) -> usize {
    1
}

fn composer_max_height(density: ComposerDensity) -> u16 {
    crate::tui::composer_chrome::ComposerChrome::for_density(density, false).max_total_rows
}

fn composer_height(
    input: &str,
    area_width: u16,
    available_height: u16,
    extra_lines: usize,
    density: ComposerDensity,
    show_panel: bool,
) -> u16 {
    let has_panel = enclosed_composer_panel_fits(show_panel, area_width, available_height);
    // Measure through the same border- and submit-aware plane that rendering,
    // cursor placement, the frame viewport, and mouse mapping use. A draft
    // that wraps here therefore cannot consume the painted `[↑]` cells later.
    let measurement_area = Rect::new(0, 0, area_width, if has_panel { 3 } else { 1 });
    let content_width =
        composer_content_geometry(composer_inner_area(measurement_area, has_panel), false)
            .text_width();
    let mut line_count = wrap_input_lines(input, content_width).len();
    if line_count == 0 {
        line_count = 1;
    }
    crate::tui::composer_chrome::desired_height(
        line_count,
        extra_lines,
        available_height,
        density,
        has_panel,
    )
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
    let completing_model_arg = prefix.strip_prefix("model ").map(str::trim_start);
    if input.contains(char::is_whitespace)
        && completing_skill_arg.is_none()
        && completing_model_arg.is_none()
    {
        return Vec::new();
    }
    let mut entries: Vec<SlashMenuEntry> = Vec::new();
    let prefix_lower = prefix.to_ascii_lowercase();

    // ── Phase 1: prefix (starts_with) matches ─────────────────────────
    // Highest priority — preserves existing exact-prefix completion.
    if completing_skill_arg.is_none() && completing_model_arg.is_none() {
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
                    &all_user_commands,
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
                        &all_user_commands,
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
                        &all_user_commands,
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
                        &all_user_commands,
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
                        &all_user_commands,
                    );
                }
            }
        });
    }

    // ── Skills (only after user has typed `/skill `) ──────────────────
    // `/model <prefix>` is the only slash-argument path that needs the
    // provider inventory. Filter it here instead of rebuilding that inventory
    // for every generic slash-menu keystroke.
    if let Some(model_prefix) = completing_model_arg {
        let model_prefix = model_prefix.to_ascii_lowercase();
        for model_name in model_candidates {
            let lower = model_name.to_ascii_lowercase();
            if lower.starts_with(&model_prefix)
                || lower.contains(&model_prefix)
                || fuzzy_chars_in_order(&model_prefix, &lower)
            {
                entries.push(SlashMenuEntry {
                    name: format!("/model {model_name}"),
                    description: String::from("Switch to this model"),
                    is_skill: false,
                    alias_hint: None,
                });
            }
        }
    }

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

    // A bare slash is an invitation, not a manual. Keep the root list to the
    // small task-oriented set while leaving every command available once the
    // user types a prefix or searches Help. User commands retain token
    // ownership, but do not flood the empty root.
    if prefix_lower.is_empty() {
        entries.retain(|entry| {
            let key = entry.name.trim_start_matches('/');
            commands::traits::BARE_SLASH_DISCOVERY_COMMANDS
                .iter()
                .any(|name| {
                    key == *name
                        || commands::get_command_info(key)
                            .is_some_and(|info| info.aliases.contains(name))
                })
        });
        for entry in &mut entries {
            if entry.name == "/subagents" {
                entry.name = "/agents".to_string();
                entry.alias_hint = None;
            }
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
    // Bare `/` follows the deliberately short task sequence. Typed prefixes
    // keep the existing rank/alpha order.
    let root_rank = |entry: &SlashMenuEntry| -> usize {
        if !prefix_lower.is_empty() {
            return 0;
        }
        let command_key = entry.name.trim_start_matches('/');
        commands::traits::bare_slash_discovery_rank(command_key).unwrap_or(usize::MAX)
    };
    entries.sort_by(|a, b| {
        root_rank(a)
            .cmp(&root_rank(b))
            .then_with(|| rank(a).cmp(&rank(b)))
            .then_with(|| a.name.cmp(&b.name))
    });
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

    if commands::discovery::user_command_shadows_builtin_canonical(builtin, user_commands) {
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
        alias_matches(alias)
            && !commands::discovery::user_command_shadows_builtin_alias(alias, user_commands)
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
        if let Some(hint) = command.display_usage() {
            description.push_str("  ");
            description.push_str(hint);
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
        let unshadowed_aliases = info
            .aliases
            .iter()
            .copied()
            .filter(|alias| {
                !commands::discovery::user_command_shadows_builtin_alias(alias, user_commands)
            })
            .collect::<Vec<_>>();
        let hint = if !command_key.to_ascii_lowercase().starts_with(prefix_lower) {
            unshadowed_aliases
                .iter()
                .copied()
                .find(|a| {
                    a.to_ascii_lowercase().starts_with(prefix_lower)
                        || a.to_ascii_lowercase().contains(prefix_lower)
                        || fuzzy_chars_in_order(prefix_lower, &a.to_ascii_lowercase())
                })
                .map(str::to_string)
        } else {
            None
        };
        // Omit aliases already shown in the label (`/clear or /qingping`) so
        // the description does not repeat them (#3990).
        let remaining_aliases: Vec<&str> = unshadowed_aliases
            .into_iter()
            .filter(|alias| hint.as_deref() != Some(*alias))
            .collect();
        let desc = if prefix_lower.is_empty() || remaining_aliases.is_empty() {
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
    // Derive the cursor's row/col from the SAME wrapped lines the renderer
    // draws. An earlier version recomputed wrapping here with hard margin
    // breaks while wrap_text broke on word boundaries, so the two disagreed on
    // row count: a long paste landed one row short of its marker, and the
    // caret drifted behind fast typing. Walking the actual wrapped lines makes
    // a desync impossible by construction (regression introduced in ff97641b7).
    let (_, lines_with_indices) = wrap_input_lines_internal(input, width.max(1));
    cursor_row_col_in_lines(&lines_with_indices, cursor)
}

/// Map a char-index cursor onto wrapped lines tagged with their starting char
/// index, as produced by wrap_input_lines_internal. The row is the line whose
/// char range contains the cursor; the column is the display width of that
/// line up to the cursor. Because wrap_text emits a trailing empty line when a
/// line fills exactly to the width, a cursor at the end of a full line lands
/// on that empty line (row+1, col 0), the display convention callers rely on,
/// without any special case here.
fn cursor_row_col_in_lines(
    lines_with_indices: &[(usize, String)],
    cursor: usize,
) -> (usize, usize) {
    let mut row = 0usize;
    let mut line_start = 0usize;
    let mut line: &str = "";
    let mut found = false;
    for (i, (start, l)) in lines_with_indices.iter().enumerate() {
        if *start <= cursor {
            row = i;
            line_start = *start;
            line = l.as_str();
            found = true;
        } else {
            break;
        }
    }
    if !found {
        return (0, 0);
    }
    let offset = cursor.saturating_sub(line_start);
    let byte_end = line
        .char_indices()
        .nth(offset)
        .map(|(b, _)| b)
        .unwrap_or(line.len());
    let col = line[..byte_end].width();
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

/// Wrap composer text to `width` display columns, breaking at word boundaries
/// where one is available.
///
/// This used to break strictly on the grapheme that crossed the margin, so a
/// wrapped sentence split mid-word — `…Write the file onl` / `y after the…`.
/// The text was never lost, but a line ending in a severed word reads exactly
/// like content that was cut off, which is what it was reported as.
///
/// Two invariants the callers depend on and this must not break:
///
/// * **Nothing is added or removed.** Concatenating the returned lines
///   reproduces `text` exactly. `wrap_input_lines_internal` walks the wrapped
///   lines accumulating `chars().count()` to map cursor and mouse positions
///   back into the raw buffer, so a dropped break character would silently
///   desynchronise the caret. The space a line breaks on therefore stays at
///   the end of the preceding line rather than being swallowed.
/// * **Every line fits.** A word longer than `width` — a URL, a path, a
///   base64 blob — has no usable break point and still breaks hard.
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
    // Byte offset in `current` just past the most recent space, and the
    // display width up to that point. `None` while the line holds no usable
    // break point — a leading space is not one, since breaking there would
    // emit an empty line and make no progress.
    let mut break_at: Option<(usize, usize)> = None;

    // Flush `current` up to its break point (if any), carrying the remainder
    // onto the next line.
    macro_rules! flush {
        () => {{
            match break_at.take() {
                Some((byte, _)) if byte < current.len() => {
                    let remainder = current.split_off(byte);
                    lines.push(std::mem::replace(&mut current, remainder));
                    current_width = current.width();
                }
                _ => {
                    lines.push(std::mem::take(&mut current));
                    current_width = 0;
                }
            }
        }};
    }

    for grapheme in text.graphemes(true) {
        if grapheme == "\n" {
            break_at = None;
            lines.push(std::mem::take(&mut current));
            current_width = 0;
            continue;
        }

        let grapheme_width = grapheme.width();
        if current_width + grapheme_width > width && current_width != 0 {
            flush!();
        }

        current.push_str(grapheme);
        current_width += grapheme_width;
        if grapheme == " " && !current.trim_start().is_empty() {
            break_at = Some((current.len(), current_width));
        }

        if current_width >= width {
            flush!();
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
        ACTIVE_REVISION_DOMAIN, ApprovalMode, ApprovalWidget, COMPOSER_PANEL_HEIGHT,
        COMPOSER_PLACEHOLDER, ChatWidget, ComposerWidget, Renderable, SlashMenuEntry,
        active_composer_submit_rect, active_entry_revision, apply_detail_target_highlight,
        apply_selection_to_line, apply_send_flash, approval_palette, approval_truncation_hint,
        build_empty_state_lines, composer_content_geometry, composer_empty_hint_text,
        composer_height, composer_inner_area, composer_max_height, composer_submit_hint,
        composer_top_padding, cursor_row_col, empty_composer_visual_rows,
        enclosed_composer_panel_fits, fish_flee_offset, fish_heading, fish_mark,
        history_entry_revision, layout_input, layout_input_with_scroll, placeholder_visual_lines,
        push_command_entry, receipt_is_settling, revision_in_domain, should_render_empty_state,
        slash_completion_hints, tool_run_summary_revision, wrap_input_lines,
        wrap_input_lines_for_mouse, wrap_text,
    };
    use crate::config::{ApiProvider, Config};
    use crate::localization::Locale;
    use crate::palette;
    use crate::tui::active_cell::ActiveCell;
    use crate::tui::app::{
        App, AppMode, ComposerDensity, QueuedMessage, TaskPanelEntry, TaskPanelEntryKind,
        ToolCollapseMode, TranscriptSpacing, TuiOptions,
    };
    use crate::tui::history::{
        ExecCell, ExecSource, GenericToolCell, HistoryCell, ToolCell, ToolRun, ToolStatus,
    };
    use crate::tui::scrolling::{TranscriptLineMeta, TranscriptScroll};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::{Color, Modifier, Style},
        text::{Line, Span},
    };
    use std::{path::PathBuf, time::Instant};
    use unicode_width::UnicodeWidthStr;

    fn create_test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-flash".to_string(),
            start_in_agent_mode: true,
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        };
        let mut app = App::new(options, &Config::default());
        // Widget contracts below exercise the post-Startup conversation
        // surface. Startup rendering has its own explicit fixture/tests.
        app.launch.visible = false;
        app.ui_locale = Locale::En;
        app.composer.vim_enabled = false;
        // Most widget fixtures exercise the underwater theme's field. Other
        // themes keep the terminal-owned shell; keep tests that inspect fish
        // and caustics intentional rather than coupled to that choice.
        app.theme_id = crate::palette::ThemeId::Underwater;
        app.ui_theme = palette::UNDERWATER_UI_THEME;
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

    #[test]
    fn approval_palette_reserves_signal_gold_for_human_decisions() {
        use crate::tui::approval::ApprovalStakes;

        let routine = approval_palette(ApprovalStakes::Routine);
        let elevated = approval_palette(ApprovalStakes::Elevated);
        let critical = approval_palette(ApprovalStakes::Critical);

        assert_eq!(routine.accent, palette::WHALE_HUMAN);
        assert_eq!(routine.shortcut, palette::WHALE_ACTION);
        assert_eq!(elevated.border, palette::WHALE_HUMAN);
        assert_eq!(elevated.accent, palette::WHALE_HUMAN);
        assert_eq!(critical.accent, palette::WHALE_ERROR);
    }

    #[test]
    fn first_active_tool_settles_when_flushed_to_history() {
        let mut app = create_test_app();
        app.clear_history();
        app.next_history_revision = 1;
        app.active_cell_revision = 0;

        let mut active = ActiveCell::new();
        active.push_tool("user_shell_1", running_user_shell_cell());
        app.active_cell = Some(active);

        let area = Rect::new(0, 0, 100, 20);
        let mut running_buf = Buffer::empty(area);
        ChatWidget::new(&mut app, area).render(area, &mut running_buf);
        let running = buffer_text(&running_buf, area);
        assert!(running.contains("run running"), "{running}");

        app.finalize_active_cell_as_interrupted();
        let HistoryCell::Tool(ToolCell::Exec(exec)) = &app.history[0] else {
            panic!("expected settled exec history cell")
        };
        assert_eq!(exec.status, ToolStatus::Failed);

        let mut settled_buf = Buffer::empty(area);
        ChatWidget::new(&mut app, area).render(area, &mut settled_buf);
        let settled = buffer_text(&settled_buf, area);
        assert!(
            !settled.contains("run running"),
            "flushed terminal state reused the active cache entry:\n{settled}"
        );
        assert!(settled.contains("run issue"), "{settled}");
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

    fn running_user_shell_cell() -> HistoryCell {
        HistoryCell::Tool(ToolCell::Exec(ExecCell {
            command: "sleep 30".to_string(),
            status: ToolStatus::Running,
            output: None,
            live_output: None,
            shell_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
            started_at: None,
            duration_ms: None,
            stale_elapsed_since_output_ms: None,
            source: ExecSource::User,
            interaction: None,
            output_summary: None,
        }))
    }

    fn add_dense_tool_run(app: &mut App) {
        app.add_message(success_tool_cell("read_file"));
        app.add_message(success_tool_cell("list_dir"));
        app.add_message(success_tool_cell("web_search"));
    }

    fn spacer_rows_after_transcript_cell(app: &App, target_cell: usize) -> usize {
        let mut saw_target = false;
        let mut spacer_rows = 0;
        for meta in app.viewport.transcript_cache.line_meta() {
            match meta {
                TranscriptLineMeta::CellLine { cell_index, .. } if *cell_index == target_cell => {
                    saw_target = true;
                    spacer_rows = 0;
                }
                TranscriptLineMeta::Spacer { .. } if saw_target => spacer_rows += 1,
                TranscriptLineMeta::CellLine { .. } if saw_target => break,
                TranscriptLineMeta::Spacer { .. } | TranscriptLineMeta::CellLine { .. } => {}
            }
        }
        spacer_rows
    }

    #[test]
    fn chat_widget_breathes_between_groups_without_padding_tool_rows_at_any_width() {
        for (width, height) in [(40, 8), (120, 12)] {
            let mut app = create_test_app();
            app.low_motion = true;
            app.fancy_animations = false;
            app.transcript_spacing = TranscriptSpacing::Comfortable;

            for turn in 0..4 {
                app.add_message(HistoryCell::User {
                    content: format!("turn {turn}: inspect the release receipts"),
                });
                app.add_message(HistoryCell::Assistant {
                    content: format!("I will inspect receipt group {turn}."),
                    streaming: false,
                });
                app.add_message(success_tool_cell(&format!("read_{turn}")));
                app.add_message(success_tool_cell(&format!("verify_{turn}")));
                app.add_message(HistoryCell::Assistant {
                    content: format!("receipt group {turn} is complete"),
                    streaming: false,
                });
            }

            let area = Rect::new(0, 0, width, height);
            app.viewport.transcript_scroll = TranscriptScroll::at_line(0);
            let mut top_buf = Buffer::empty(area);
            ChatWidget::new(&mut app, area).render(area, &mut top_buf);

            assert_eq!(app.viewport.last_transcript_top, 0, "width={width}");
            assert!(
                app.viewport.last_transcript_total > usize::from(height),
                "fixture must scroll at width={width}"
            );
            assert_eq!(
                spacer_rows_after_transcript_cell(&app, 0),
                1,
                "the top-level user turn needs a breathing row at width={width}"
            );
            assert_eq!(
                spacer_rows_after_transcript_cell(&app, 1),
                1,
                "answer to tool-group transition needs a breathing row at width={width}"
            );
            assert_eq!(
                spacer_rows_after_transcript_cell(&app, 2),
                0,
                "calls inside one tool group must stay compact at width={width}"
            );
            assert_eq!(
                spacer_rows_after_transcript_cell(&app, 3),
                1,
                "the completed tool group needs a breathing row at width={width}"
            );
            assert!(
                buffer_text(&top_buf, area).contains("turn 0"),
                "top scroll source drifted at width={width}"
            );

            let total = app.viewport.last_transcript_total;
            app.viewport.transcript_scroll = TranscriptScroll::to_bottom();
            let mut tail_buf = Buffer::empty(area);
            ChatWidget::new(&mut app, area).render(area, &mut tail_buf);

            assert_eq!(app.viewport.last_transcript_total, total, "width={width}");
            assert!(app.viewport.last_transcript_top > 0, "width={width}");
            assert!(
                buffer_text(&tail_buf, area).contains("receipt group 3 is complete"),
                "tail scroll lost the final source-backed cell at width={width}"
            );
            assert!(
                app.viewport
                    .transcript_cache
                    .line_meta()
                    .iter()
                    .all(|meta| match meta {
                        TranscriptLineMeta::CellLine { cell_index, .. } => {
                            *cell_index < app.history.len()
                        }
                        TranscriptLineMeta::Spacer { .. } => true,
                    }),
                "spacing rows must not invent source-cell ownership at width={width}"
            );
        }
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

        assert_eq!(
            lines[0].spans[0].style.bg,
            Some(palette::SURFACE_TOOL_ACTIVE)
        );
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
    fn tool_run_summary_revision_separates_128_entry_history_and_active_alias() {
        let active_rev = 17;
        let run = ToolRun {
            start: 0,
            count: 128,
            tool_families: Vec::new(),
            activity: Default::default(),
        };
        let history_revisions = (1..=run.count)
            .map(|salt| active_entry_revision(active_rev, salt as u64))
            .collect::<Vec<_>>();

        let history_key =
            tool_run_summary_revision(&run, &history_revisions, run.count, active_rev);
        let active_key = tool_run_summary_revision(&run, &[], 0, active_rev);

        // Rotating by seven over 128 entries cancels the 128 identical domain
        // bits, reproducing the old untagged hash alias. The final domain tag
        // must still keep the cache keys distinct.
        assert_eq!(
            history_key & !ACTIVE_REVISION_DOMAIN,
            active_key & !ACTIVE_REVISION_DOMAIN,
            "fixture must exercise the 128-entry payload alias"
        );
        assert_eq!(history_key & ACTIVE_REVISION_DOMAIN, 0);
        assert_eq!(active_key & ACTIVE_REVISION_DOMAIN, ACTIVE_REVISION_DOMAIN);
        assert_ne!(history_key, active_key);
    }

    #[test]
    fn high_bit_raw_revision_remains_distinct_across_history_and_active_domains() {
        let raw = ACTIVE_REVISION_DOMAIN | 0x2692;
        let history_key = history_entry_revision(raw);
        let active_key = revision_in_domain(raw, true);

        assert_eq!(history_key, 0x2692);
        assert_eq!(active_key, ACTIVE_REVISION_DOMAIN | 0x2692);
        assert_ne!(history_key, active_key);
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
    fn collapsed_slow_path_does_not_reuse_running_active_cache_after_flush() {
        let mut app = create_test_app();
        app.tool_collapse_mode = ToolCollapseMode::Compact;
        app.tool_collapse_threshold = 3;
        add_dense_tool_run(&mut app);

        // Force the next committed history revision to have the same raw key
        // as active revision 0, salt 1. The prior collapsed run keeps both
        // renders on the filtered slow path.
        app.next_history_revision = ACTIVE_REVISION_DOMAIN | 1;
        app.active_cell_revision = 0;
        let mut active = ActiveCell::new();
        active.push_tool("user_shell_slow_path", running_user_shell_cell());
        app.active_cell = Some(active);

        let area = Rect::new(0, 0, 100, 20);
        let mut running_buf = Buffer::empty(area);
        ChatWidget::new(&mut app, area).render(area, &mut running_buf);
        let running = buffer_text(&running_buf, area);
        assert!(running.contains("run running"), "{running}");
        assert_eq!(app.collapsed_cell_map, vec![0, 3]);

        app.finalize_active_cell_as_interrupted();
        assert_eq!(
            app.history_revisions[3],
            ACTIVE_REVISION_DOMAIN | 1,
            "fixture must force the old raw-revision collision"
        );

        let mut settled_buf = Buffer::empty(area);
        ChatWidget::new(&mut app, area).render(area, &mut settled_buf);
        let settled = buffer_text(&settled_buf, area);
        assert!(
            !settled.contains("run running"),
            "history cell reused the active slow-path cache entry:\n{settled}"
        );
        assert!(settled.contains("run issue"), "{settled}");
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
    fn chat_widget_collapse_path_stable_across_frames() {
        let mut app = create_test_app();
        app.tool_collapse_mode = ToolCollapseMode::Compact;
        app.tool_collapse_threshold = 3;
        add_dense_tool_run(&mut app);
        app.add_message(HistoryCell::User {
            content: "trailing prompt".to_string(),
        });

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 10,
        };

        let mut first_buf = Buffer::empty(area);
        ChatWidget::new(&mut app, area).render(area, &mut first_buf);
        let first = buffer_text(&first_buf, area);
        let first_map = app.collapsed_cell_map.clone();
        let first_total = app.viewport.last_transcript_total;

        // Second frame without any app mutation: the borrowed filtered path
        // must reproduce the identical output and index map.
        let mut second_buf = Buffer::empty(area);
        ChatWidget::new(&mut app, area).render(area, &mut second_buf);
        let second = buffer_text(&second_buf, area);

        assert_eq!(first, second, "collapse path is frame-stable");
        assert_eq!(first_map, app.collapsed_cell_map);
        assert_eq!(first_total, app.viewport.last_transcript_total);
        assert!(first.contains("Explored 2 files, 1 search"), "{first}");
        assert!(first.contains("trailing prompt"), "{first}");
    }

    #[test]
    fn chat_widget_collapses_run_spanning_history_and_active_entries() {
        let mut app = create_test_app();
        app.tool_collapse_mode = ToolCollapseMode::Compact;
        app.tool_collapse_threshold = 3;
        app.add_message(success_tool_cell("read_file"));
        app.add_message(success_tool_cell("list_dir"));
        let active = app.active_cell.get_or_insert_with(ActiveCell::new);
        active.push_untracked(success_tool_cell("web_search"));
        app.bump_active_cell_revision();

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 8,
        };
        let mut buf = Buffer::empty(area);
        ChatWidget::new(&mut app, area).render(area, &mut buf);
        let rendered = buffer_text(&buf, area);

        assert_eq!(app.collapsed_cell_map, vec![0]);
        assert!(
            rendered.contains("Explored 2 files, 1 search"),
            "run spanning the history/active boundary renders one summary: {rendered}"
        );

        // Mutating the active tail must re-render the summary (its revision
        // folds in the covered active entries).
        let rev_before = app.active_cell_revision;
        app.bump_active_cell_revision();
        assert_ne!(rev_before, app.active_cell_revision);
        let mut second_buf = Buffer::empty(area);
        ChatWidget::new(&mut app, area).render(area, &mut second_buf);
        let second = buffer_text(&second_buf, area);
        assert!(second.contains("Explored 2 files, 1 search"), "{second}");
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

    /// Composer wrapping breaks between words, not through them. A line
    /// ending in a severed word (`…Write the file onl`) reads exactly like
    /// content that was cut off, which is how it was reported.
    #[test]
    fn composer_wraps_on_word_boundaries_without_losing_a_character() {
        let text = "Mark inferences as inferences. A short PRD where each \
                    section decides something beats a long one.";
        for width in [20usize, 33, 47, 60, 79] {
            let lines = wrap_text(text, width);
            assert_eq!(
                lines.concat(),
                text,
                "wrapping must be lossless at width={width}: {lines:?}"
            );
            for line in &lines {
                assert!(
                    line.width() <= width,
                    "line exceeds width={width}: {line:?}"
                );
            }
            // No line may end in the middle of a word: either it ends the
            // text, or it ends on whitespace.
            for line in lines.iter().take(lines.len().saturating_sub(1)) {
                assert!(
                    line.is_empty() || line.ends_with(' '),
                    "wrapped line broke mid-word at width={width}: {line:?}"
                );
            }
        }
    }

    /// A token with no break point in it still has to fit the terminal, so it
    /// breaks hard. Losslessness holds there too.
    #[test]
    fn composer_hard_breaks_words_longer_than_the_line() {
        let text = "see https://example.com/a/very/long/path/that/never/breaks?x=1 now";
        let lines = wrap_text(text, 24);
        assert_eq!(lines.concat(), text, "{lines:?}");
        for line in &lines {
            assert!(line.width() <= 24, "line exceeds width: {line:?}");
        }
        assert!(
            lines.len() > 2,
            "an unbreakable token must still be split across lines: {lines:?}"
        );
    }

    /// Wide characters have no spaces to break on; the width accounting must
    /// still hold. This repo patches `unicode-width` for CJK, so measure the
    /// wrapped output rather than trusting char counts.
    #[test]
    fn composer_wrapping_respects_wide_character_width() {
        let text = "中文字符串没有空格可以换行";
        let lines = wrap_text(text, 7);
        assert_eq!(lines.concat(), text, "{lines:?}");
        for line in &lines {
            assert!(line.width() <= 7, "line exceeds width: {line:?}");
        }
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
    fn bare_slash_menu_is_bounded_and_the_long_tail_remains_searchable() {
        let hints = slash_completion_hints("/", 128, &[], Locale::En, None, ApiProvider::Deepseek);
        let names: Vec<&str> = hints.iter().map(|hint| hint.name.as_str()).collect();
        assert_eq!(
            names,
            ["/help", "/setup", "/model", "/settings", "/resume", "/rc",],
            "bare / should offer the small starting set: {names:?}"
        );
        assert!(
            slash_completion_hints("/wor", 128, &[], Locale::En, None, ApiProvider::Deepseek)
                .iter()
                .any(|hint| hint.name == "/workflow")
        );
        assert!(
            slash_completion_hints("/conf", 128, &[], Locale::En, None, ApiProvider::Deepseek)
                .iter()
                .any(|hint| hint.name == "/config")
        );
        assert!(
            slash_completion_hints("/age", 128, &[], Locale::En, None, ApiProvider::Deepseek)
                .iter()
                .any(|hint| hint.name == "/subagents")
        );
        assert!(
            slash_completion_hints("/comp", 128, &[], Locale::En, None, ApiProvider::Deepseek)
                .iter()
                .any(|hint| hint.name == "/compact")
        );
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
        assert!(root.iter().any(|hint| hint.name == "/model"));
        assert!(!root.iter().any(|hint| hint.name == "/provider"));
        assert!(!root.iter().any(|hint| hint.name == "/fleet"));
        assert!(!root.iter().any(|hint| hint.name == "/fleet"));
        assert!(!root.iter().any(|hint| hint.name == "/config"));
        assert!(!root.iter().any(|hint| hint.name == "/statusline"));
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
    fn slash_completion_uses_frontmatter_name_and_usage() {
        let tmp = tempfile::TempDir::new().unwrap();
        let commands_dir = tmp.path().join(".codewhale").join("commands");
        std::fs::create_dir_all(&commands_dir).unwrap();
        std::fs::write(
            commands_dir.join("workflow-file.md"),
            "---\nname: inspect\ndescription: Inspect target\nusage: /inspect <path>\narguments: <path>\n---\ninspect",
        )
        .unwrap();

        let hints = slash_completion_hints(
            "/ins",
            128,
            &[],
            Locale::En,
            Some(tmp.path()),
            ApiProvider::Deepseek,
        );
        let entry = hints
            .iter()
            .find(|hint| hint.name == "/inspect")
            .expect("frontmatter name should complete");

        assert_eq!(entry.description, "Inspect target  /inspect <path>");
        assert!(!hints.iter().any(|hint| hint.name == "/workflow-file"));
    }

    #[test]
    fn slash_completion_uses_arguments_when_usage_and_legacy_hint_are_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let commands_dir = tmp.path().join(".codewhale").join("commands");
        std::fs::create_dir_all(&commands_dir).unwrap();
        std::fs::write(
            commands_dir.join("deploy.md"),
            "---\ndescription: Deploy target\narguments: <environment>\n---\ndeploy",
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

        assert_eq!(entry.description, "Deploy target  <environment>");
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
    fn hidden_name_override_filters_shadowed_builtin_from_slash_completion() {
        let tmp = tempfile::TempDir::new().unwrap();
        let commands_dir = tmp.path().join(".codewhale").join("commands");
        std::fs::create_dir_all(&commands_dir).unwrap();
        std::fs::write(
            commands_dir.join("private-help.md"),
            "---\nname: help\nhidden: true\n---\nprivate help",
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

        assert!(!hints.iter().any(|hint| hint.name == "/help"));
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
    fn slash_completion_offers_no_retired_pod_entry() {
        let hints =
            slash_completion_hints("/pod", 128, &[], Locale::En, None, ApiProvider::Deepseek);
        assert!(
            !hints.iter().any(|hint| hint.name == "/pod"),
            "the retired /pod spelling must not complete"
        );
        for entry in hints.iter().filter(|hint| hint.name == "/fleet") {
            assert_eq!(
                entry.alias_hint, None,
                "no alias may point at the retired spelling"
            );
        }
    }

    #[test]
    fn slash_completion_omits_rejected_user_alias_collisions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let commands_dir = tmp.path().join(".codewhale").join("commands");
        std::fs::create_dir_all(&commands_dir).unwrap();
        std::fs::write(
            commands_dir.join("alpha.md"),
            "---\ndescription: Alpha command\nalias: beta\n---\nalpha",
        )
        .unwrap();
        std::fs::write(
            commands_dir.join("beta.md"),
            "---\ndescription: Beta command\n---\nbeta",
        )
        .unwrap();

        let hints = slash_completion_hints(
            "/bet",
            128,
            &[],
            Locale::En,
            Some(tmp.path()),
            ApiProvider::Deepseek,
        );

        assert!(hints.iter().any(|hint| hint.name == "/beta"));
        assert!(
            !hints.iter().any(|hint| hint.name == "/alpha"),
            "a command must not match through an alias rejected by the registry"
        );
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

        let attach = canonical_hints
            .iter()
            .find(|hint| hint.name == "/attach")
            .expect(
                "canonical /attach should remain visible when only its /image alias is shadowed",
            );
        assert!(
            !attach.description.contains("/image"),
            "canonical completion must not advertise a user-shadowed alias"
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
    fn slash_completion_accepted_user_alias_claims_builtin_canonical_token() {
        // A visible user command whose accepted alias equals a built-in
        // canonical token must own that token in completion: the built-in
        // suggestion is absent and the user command appears for the alias.
        let tmp = tempfile::TempDir::new().unwrap();
        let commands_dir = tmp.path().join(".codewhale").join("commands");
        std::fs::create_dir_all(&commands_dir).unwrap();
        std::fs::write(
            commands_dir.join("assistant.md"),
            "---\ndescription: My assistant\nalias: help\n---\nassistant",
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

        assert!(
            !hints.iter().any(|hint| hint.name == "/help"),
            "built-in /help must be absent when a user alias claims the token"
        );
        assert!(
            hints.iter().any(|hint| hint.name == "/assistant"),
            "the user command must appear for the claimed token"
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
        assert!(!hints.iter().any(|hint| hint.name == "/skill"));
        assert!(!hints.iter().any(|hint| hint.name == "/skills"));
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
    fn truncated_slash_row_registers_its_full_localized_copy_for_hover() {
        let mut app = create_test_app();
        app.input = "/model".to_string();
        app.cursor_position = app.input.len();
        let full_name = "/model provider/very-long-model-identifier";
        let full_description = "切换到这个模型并保留完整的本地化说明";
        let entries = vec![SlashMenuEntry {
            name: full_name.to_string(),
            description: full_description.to_string(),
            is_skill: false,
            alias_hint: None,
        }];
        let area = Rect::new(0, 0, 36, 7);
        let mut buf = Buffer::empty(area);

        crate::tui::hover_layer::begin_frame();
        ComposerWidget::new(&app, area.height, &entries, &[]).render(area, &mut buf);

        let targets = crate::tui::hover_layer::registered_targets();
        assert_eq!(targets.len(), 1, "targets: {targets:?}");
        assert_eq!(
            targets[0].kind,
            crate::tui::hover_hit::HoverTargetKind::TruncatedText
        );
        assert!(targets[0].label.contains(full_name));
        assert!(targets[0].label.contains(full_description));
    }

    #[test]
    fn complete_slash_row_does_not_register_a_hover_popover() {
        let mut app = create_test_app();
        app.input = "/help".to_string();
        app.cursor_position = app.input.len();
        let entries = vec![SlashMenuEntry {
            name: "/help".to_string(),
            description: "Show help".to_string(),
            is_skill: false,
            alias_hint: None,
        }];
        let area = Rect::new(0, 0, 80, 7);
        let mut buf = Buffer::empty(area);

        crate::tui::hover_layer::begin_frame();
        ComposerWidget::new(&app, area.height, &entries, &[]).render(area, &mut buf);

        assert!(crate::tui::hover_layer::registered_targets().is_empty());
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
    fn selection_keeps_keycap_grapheme_intact() {
        let line = Line::from(Span::raw("A1\u{fe0f}\u{20e3}B"));
        let selection_style = Style::default().bg(palette::SELECTION_BG);

        // Selecting the second display column of the two-column keycap must
        // style the complete grapheme, never only FE0F/U+20E3.
        let styled = apply_selection_to_line(&line, 2, 3, selection_style);
        assert_eq!(styled.len(), 3);
        assert_eq!(styled[0].content.as_ref(), "A");
        assert_eq!(styled[1].content.as_ref(), "1\u{fe0f}\u{20e3}");
        assert_eq!(styled[1].style.bg, Some(palette::SELECTION_BG));
        assert_eq!(styled[2].content.as_ref(), "B");
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
        let has_panel = enclosed_composer_panel_fits(true, width, available_height);
        let chrome_height = if has_panel {
            usize::from(COMPOSER_PANEL_HEIGHT)
        } else {
            1
        };
        let measurement_area = Rect::new(0, 0, width, if has_panel { 3 } else { 1 });
        let content_width =
            composer_content_geometry(composer_inner_area(measurement_area, has_panel), false)
                .text_width();
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
        assert_eq!(height, 3);
    }

    #[test]
    fn composer_panel_height_and_render_policy_agree_at_width_boundary() {
        let mut app = create_test_app();
        app.composer_border = true;
        app.composer_density = ComposerDensity::Comfortable;
        let slash_menu_entries = Vec::<SlashMenuEntry>::new();
        let mention_menu_entries = Vec::<String>::new();
        let widget = ComposerWidget::new(&app, 8, &slash_menu_entries, &mention_menu_entries);

        for (width, expected_panel, expected_height) in
            [(11, false, 2), (12, true, 3), (13, true, 3), (14, true, 3)]
        {
            let height = widget.desired_height(width);
            let area = Rect::new(0, 0, width, height);

            assert_eq!(height, expected_height, "width={width}");
            assert_eq!(widget.has_panel(area), expected_panel, "width={width}");
            assert_eq!(
                widget.inner_area(area).height,
                1,
                "width={width} auto-fit composer reserves one input row plus \
                 every rendered border row"
            );

            let mut buf = Buffer::empty(area);
            widget.render(area, &mut buf);
            assert_eq!(
                buf[(1, area.bottom().saturating_sub(1))].symbol() == "\u{2500}",
                expected_panel,
                "width={width} bottom border disagrees with height policy"
            );
            if expected_panel {
                let shell = crate::tui::composer_chrome::tideline_composer_geometry(area);
                assert_eq!(
                    widget.inner_area(area),
                    Rect::new(1, 1, shell.content.right().saturating_sub(1), 1,),
                    "width={width} panel input area must reserve the send control and breathing cell"
                );
                assert_eq!(buf[(area.left(), area.top())].symbol(), "\u{256d}");
                assert_eq!(
                    buf[(area.right().saturating_sub(1), area.top())].symbol(),
                    "\u{256e}"
                );
                assert_eq!(
                    buf[(area.left(), area.bottom().saturating_sub(1))].symbol(),
                    "\u{2570}"
                );
                assert_eq!(
                    buf[(
                        area.right().saturating_sub(1),
                        area.bottom().saturating_sub(1)
                    )]
                        .symbol(),
                    "\u{256f}"
                );
                assert_eq!(
                    buf[(area.left(), area.y.saturating_add(1))].symbol(),
                    "\u{2502}"
                );
                assert_eq!(
                    buf[(area.right().saturating_sub(1), area.y.saturating_add(1))].symbol(),
                    "\u{2502}"
                );
            } else {
                assert_eq!(
                    widget.inner_area(area),
                    Rect::new(area.x, area.y.saturating_add(1), area.width, 1),
                    "width={width} compact fallback must keep its full input width"
                );
                assert_ne!(buf[(area.left(), area.top())].symbol(), "\u{256d}");
            }
        }
    }

    #[test]
    fn composer_height_wraps_to_the_rounded_panel_content_width() {
        // At the minimum viable panel width, the side rails, prompt gutter,
        // shared `[↑]` control, and its breathing cell leave three text
        // columns. Measuring against the old width would render extra lines
        // without allocating their rows.
        let height = composer_height(
            "123456789",
            super::COMPOSER_PANEL_MIN_WIDTH,
            8,
            0,
            ComposerDensity::Comfortable,
            true,
        );
        assert_eq!(height, 6);
    }

    #[test]
    fn composer_expands_for_multiline_input_and_collapses_again() {
        let height_for =
            |input| composer_height(input, 40, 12, 0, ComposerDensity::Comfortable, true);

        let collapsed = height_for("short");
        let expanded = height_for("one\ntwo\nthree\nfour\nfive\nsix");
        let collapsed_again = height_for("short");

        // Auto-fit: one input row + top/bottom panel borders.
        assert_eq!(collapsed, 3);
        // Six content rows + two borders, still under the Comfortable cap of 9.
        assert_eq!(expanded, 8);
        assert!(expanded > collapsed);
        assert_eq!(collapsed_again, collapsed);
    }

    /// Issue #4809 acceptance: the composer auto-fits its content through the
    /// real widget path — typed input, `submit_input`, `clear_input` — not just
    /// through the pure height helper.
    #[test]
    fn composer_auto_fits_typed_lines_and_returns_to_one_row_on_submit_or_clear() {
        const WIDTH: u16 = 40;
        const AVAILABLE: u16 = 24;

        fn measure(app: &App) -> (u16, u16) {
            let slash_menu_entries = Vec::<SlashMenuEntry>::new();
            let mention_menu_entries = Vec::<String>::new();
            let widget =
                ComposerWidget::new(app, AVAILABLE, &slash_menu_entries, &mention_menu_entries);
            let total = widget.desired_height(WIDTH);
            let inner = widget.inner_area(Rect::new(0, 0, WIDTH, total)).height;
            (total, inner)
        }

        let mut app = create_test_app();
        app.composer_border = true;
        app.composer_density = ComposerDensity::Comfortable;

        // Empty composer: one input row inside the panel borders.
        assert_eq!(measure(&app), (3, 1), "empty composer");

        app.insert_str("one line");
        assert_eq!(measure(&app), (3, 1), "single-line composer");

        // Typing N lines grows the composer to N input rows while N is under
        // the Comfortable cap of 9 total rows (7 input rows + 2 borders).
        for n in 2..=7u16 {
            app.clear_input();
            let text = (1..=n)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n");
            app.insert_str(&text);
            assert_eq!(measure(&app), (n + 2, n), "{n} typed lines");
        }

        // Past the cap the density setting wins, not the content.
        app.clear_input();
        app.insert_str(&vec!["over"; 40].join("\n"));
        let cap = composer_max_height(ComposerDensity::Comfortable);
        assert_eq!(measure(&app), (cap, cap - 2), "content beyond the cap");

        // Submitting returns the composer to a single input row.
        assert!(app.submit_input().is_some());
        assert_eq!(measure(&app), (3, 1), "after submit");

        // So does clearing a fresh multi-line draft.
        app.insert_str("a\nb\nc\nd");
        assert_eq!(measure(&app), (6, 4), "four-line draft");
        app.clear_input();
        assert_eq!(measure(&app), (3, 1), "after clear");
    }

    #[test]
    fn composer_height_uses_quiet_rule_when_panel_is_not_needed() {
        let with_border = composer_height("", 40, 8, 0, ComposerDensity::Comfortable, true);
        let without_border = composer_height("", 40, 8, 0, ComposerDensity::Comfortable, false);

        // Quiet composer keeps a single top rule over the one auto-fit
        // input row; the panel shape adds its bottom border.
        assert_eq!(with_border, 3);
        assert_eq!(without_border, 2);
        assert!(without_border < with_border);
    }

    #[test]
    fn composer_density_changes_height_cap() {
        assert!(
            composer_max_height(ComposerDensity::Spacious)
                > composer_max_height(ComposerDensity::Compact)
        );
    }

    #[test]
    fn composer_content_geometry_is_the_single_prompt_adjusted_text_rect() {
        let inner = Rect::new(10, 4, 7, 3);
        let normal = composer_content_geometry(inner, false);
        assert_eq!(normal.prompt_inset, 2);
        assert_eq!(normal.text_area, Rect::new(12, 4, 5, 3));
        assert_eq!(normal.text_width(), 5);

        let history = composer_content_geometry(inner, true);
        assert_eq!(history.prompt_inset, 0);
        assert_eq!(history.text_area, inner);

        let narrow = composer_content_geometry(Rect::new(3, 2, 2, 1), false);
        assert_eq!(narrow.prompt_inset, 0);
        assert_eq!(narrow.text_area, Rect::new(3, 2, 2, 1));
    }

    #[test]
    fn composer_wrap_boundary_cursor_scroll_and_mouse_lines_share_text_width() {
        let geometry = composer_content_geometry(Rect::new(0, 0, 7, 2), false);
        let input = "abcde";
        let cursor = input.chars().count();
        let width = geometry.text_width();

        let (absolute_row, absolute_col) = cursor_row_col(input, cursor, width);
        let (visible, visible_row, visible_col, scroll_offset) =
            layout_input_with_scroll(input, cursor, width, 1);
        let mouse_lines = wrap_input_lines_for_mouse(input, width);

        assert_eq!((absolute_row, absolute_col), (1, 0));
        assert_eq!(scroll_offset, 1);
        assert_eq!((visible_row, visible_col), (0, 0));
        assert_eq!(visible, vec![String::new()]);
        assert_eq!(mouse_lines[scroll_offset], (cursor, String::new()));
    }

    #[test]
    fn empty_composer_keeps_prompt_and_hint_on_one_row() {
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

        // The two border rows carry independent permission/mode signals.
        // inner_area: {x:1, y:1, w:38, h:3}
        // input_rows_budget = 3
        // The prompt and hint share one quiet row.
        assert_eq!(
            empty_composer_visual_rows(Some(COMPOSER_PLACEHOLDER), 40, 3),
            1
        );
        assert_eq!(widget.cursor_pos(area), Some((3, 2)));
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
        // placeholder_visual_lines(12) = 3
        // The narrow fallback still reserves one composer row; Paragraph
        // clipping keeps it from growing the shell.
        assert_eq!(placeholder_visual_lines(12), 3);
        assert_eq!(
            empty_composer_visual_rows(Some(COMPOSER_PLACEHOLDER), 14, 3),
            1
        );
        assert_eq!(widget.cursor_pos(area), Some((3, 2)));
    }

    #[test]
    fn empty_composer_renders_prompt_and_hint_on_cursor_row() {
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
        let placeholder = composer_empty_hint_text(&app).into_owned();
        let first_placeholder_cell = placeholder
            .chars()
            .next()
            .expect("composer placeholder should not be empty")
            .to_string();

        assert_eq!(buf[(cursor_x, cursor_y)].symbol(), first_placeholder_cell);
        assert_eq!(
            buf[(cursor_x, cursor_y)].fg,
            app.ui_theme.text_soft,
            "the idle prompt should use the readable soft-text role"
        );
        assert!(
            !buf[(cursor_x, cursor_y)]
                .modifier
                .contains(Modifier::ITALIC),
            "the idle prompt should remain upright at distance"
        );
        assert!(
            rendered.contains(&placeholder),
            "placeholder hint should render on the prompt row: {rendered}"
        );
        assert!(
            row_text(&buf, area, cursor_y).contains(&placeholder),
            "prompt and hint should share one row: {rendered}"
        );
        let inner = widget.inner_area(area);
        let quiet_row = cursor_y.saturating_add(1);
        // The quiet row hosts exactly one thing: the shared `[↑]` affordance
        // on its recorded hitbox cells. Every other cell stays blank.
        let submit = active_composer_submit_rect(&app, area).expect("enclosed composer submit");
        assert!(
            quiet_row < inner.bottom()
                && (inner.x..inner.right()).all(|x| {
                    let on_submit =
                        submit.y == quiet_row && x >= submit.x && x < submit.x + submit.width;
                    on_submit || buf[(x, quiet_row)].symbol() == " "
                }),
            "comfortable composer should keep a quiet content row before the footer, hosting only the shared [↑]: {rendered}"
        );
        let painted: String = (submit.x..submit.x + submit.width)
            .map(|x| buf[(x, submit.y)].symbol().to_string())
            .collect();
        assert_eq!(painted, "[↑]", "the quiet row hosts the shared send cells");
    }

    #[test]
    fn composer_keeps_prompt_anchored_after_first_keystroke() {
        let mut app = create_test_app();
        app.composer_density = ComposerDensity::Comfortable;
        app.input = "hello".to_string();
        app.cursor_position = app.input.len();
        let slash_menu_entries = Vec::<SlashMenuEntry>::new();
        let mention_menu_entries = Vec::<String>::new();
        let widget = ComposerWidget::new(&app, 5, &slash_menu_entries, &mention_menu_entries);
        let area = Rect::new(0, 0, 40, 5);
        let mut buf = Buffer::empty(area);

        widget.render(area, &mut buf);
        let (cursor_x, cursor_y) = widget
            .cursor_pos(area)
            .expect("composer with input should expose a cursor");

        assert_eq!(buf[(1, cursor_y)].symbol(), "❯");
        assert_eq!(buf[(3, cursor_y)].symbol(), "h");
        assert_eq!(cursor_x, 8, "cursor keeps the prompt gutter reserved");
    }

    fn render_composer(app: &App, width: u16, height: u16) -> String {
        let slash_menu_entries = Vec::<SlashMenuEntry>::new();
        let mention_menu_entries = Vec::<String>::new();
        let widget = ComposerWidget::new(app, height, &slash_menu_entries, &mention_menu_entries);
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        buffer_text(&buf, area)
    }

    #[test]
    fn composer_empty_hint_names_a_follow_up_while_a_turn_is_running() {
        let mut app = create_test_app();
        assert_eq!(composer_empty_hint_text(&app).as_ref(), "Type a message…");

        app.is_loading = true;
        assert_eq!(composer_empty_hint_text(&app).as_ref(), "Type a follow-up…");

        app.queue_message(QueuedMessage::new("later".to_string(), None));
        assert_eq!(
            composer_empty_hint_text(&app).as_ref(),
            "Enter send now · type another"
        );
    }

    #[test]
    fn composer_submit_hint_names_send_after_this_turn_without_steer() {
        let mut app = create_test_app();
        app.input = "keep going".to_string();
        app.cursor_position = app.input.chars().count();
        assert!(composer_submit_hint(&app).is_none());

        app.is_loading = true;
        let hint = composer_submit_hint(&app).expect("busy draft should name Enter");
        assert_eq!(hint.text, "↵ send after this turn");
        assert!(
            !hint.text.to_ascii_lowercase().contains("steer"),
            "composer hint leaked internal vocabulary: {}",
            hint.text
        );

        app.queue_message(QueuedMessage::new("first".to_string(), None));
        let hint = composer_submit_hint(&app).expect("queued count should stay visible");
        assert_eq!(hint.text, "↵ send after this turn (2 waiting)");
    }

    #[test]
    fn composer_submit_hint_renders_at_release_floor_widths() {
        let mut app = create_test_app();
        app.composer_border = true;
        app.is_loading = true;
        app.input = "keep going".to_string();
        app.cursor_position = app.input.chars().count();

        for (width, height) in [(40_u16, 12), (60, 16), (80, 24), (100, 32), (140, 40)] {
            let rendered = render_composer(&app, width, height);
            assert!(
                rendered.contains("send after this turn"),
                "missing queue hint at {width}x{height}:\n{rendered}"
            );
            assert!(
                !rendered.to_ascii_lowercase().contains("steer"),
                "steer vocabulary at {width}x{height}:\n{rendered}"
            );
        }
    }

    #[test]
    fn quiet_composer_still_shows_the_submit_hint() {
        let mut app = create_test_app();
        app.composer_border = false;
        app.is_loading = true;
        app.input = "keep going".to_string();
        app.cursor_position = app.input.chars().count();
        let rendered = render_composer(&app, 80, 4);
        assert!(
            rendered.contains("send after this turn"),
            "quiet composer hid the Enter action:\n{rendered}"
        );
    }

    #[test]
    fn composer_border_omits_session_title_chrome() {
        // The top-right composer chrome (session title / receipts / vim mode)
        // was classic-shell-only; with the classic shell removed the composer
        // border never carries it. Session identity lives in the header.
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
        assert!(!rendered.contains("my-session"));
    }

    #[test]
    fn composer_border_omits_active_turn_receipt_chrome() {
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
        assert!(!rendered.contains("turn completed"));
        assert!(!rendered.contains("tool(s) used"));
    }

    #[test]
    fn composer_border_edges_encode_warm_permission_and_cool_mode_ramps() {
        let slash_menu_entries = Vec::<SlashMenuEntry>::new();
        let mention_menu_entries = Vec::<String>::new();
        let area = Rect::new(0, 0, 40, 5);

        for theme_id in palette::SELECTABLE_THEMES {
            let theme = theme_id.ui_theme();
            for (approval_mode, expected) in [
                (ApprovalMode::Suggest, theme.permission_ask),
                (ApprovalMode::Never, theme.permission_ask),
                (ApprovalMode::Auto, theme.permission_auto_review),
                (ApprovalMode::Bypass, theme.permission_full_access),
            ] {
                let mut app = create_test_app();
                app.ui_theme = theme;
                app.approval_mode = approval_mode;
                let widget =
                    ComposerWidget::new(&app, 5, &slash_menu_entries, &mention_menu_entries);
                let mut buf = Buffer::empty(area);
                widget.render(area, &mut buf);
                assert_eq!(
                    buf[(1, area.top())].fg,
                    expected,
                    "{} {approval_mode:?}",
                    theme_id.name()
                );
            }

            for (mode, expected) in [
                (AppMode::Plan, theme.mode_plan),
                (AppMode::Agent, theme.mode_agent),
                (AppMode::Operate, theme.mode_operate),
            ] {
                let mut app = create_test_app();
                app.ui_theme = theme;
                app.mode = mode;
                let widget =
                    ComposerWidget::new(&app, 5, &slash_menu_entries, &mention_menu_entries);
                let mut buf = Buffer::empty(area);
                widget.render(area, &mut buf);
                assert_eq!(
                    buf[(1, area.bottom().saturating_sub(1))].fg,
                    expected,
                    "{} {mode:?}",
                    theme_id.name()
                );
            }
        }
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
        // Multi-line drafts no longer announce themselves with a block title;
        // the user can see the draft. Only history search keeps its title.
        assert!(!buffer_text(&draft_buf, area).contains("Draft"));

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
    fn enclosed_composer_paints_the_shared_send_hitbox() {
        let mut app = create_test_app();
        app.composer_border = true;
        app.input = "ship it".to_string();
        app.cursor_position = app.input.chars().count();
        for (width, height) in [(40_u16, 12), (60, 16), (80, 24), (100, 32), (120, 32)] {
            let rendered = render_composer(&app, width, height);
            assert!(
                rendered.contains("[↑]"),
                "missing send affordance at {width}x{height}:\n{rendered}"
            );
            assert!(
                !rendered.contains("▚△▞"),
                "retired crown must stay gone at {width}x{height}:\n{rendered}"
            );
        }
    }

    #[test]
    fn enclosed_composer_send_hitbox_matches_painted_cells() {
        let mut app = create_test_app();
        app.composer_border = true;
        app.input = "x".repeat(240);
        app.cursor_position = app.input.chars().count();
        let slash_menu_entries = Vec::<SlashMenuEntry>::new();
        let mention_menu_entries = Vec::<String>::new();
        let widget = ComposerWidget::new(&app, 8, &slash_menu_entries, &mention_menu_entries);
        let area = Rect::new(0, 0, 80, 8);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        let submit = active_composer_submit_rect(&app, area).expect("enclosed composer submit");
        let painted: String = (submit.x..submit.x + submit.width)
            .map(|x| buf[(x, submit.y)].symbol().to_string())
            .collect();
        assert_eq!(painted, "[↑]", "geometry must cover the painted send cells");
    }

    #[test]
    fn enclosed_composer_reserves_submit_cells_for_a_74_character_draft() {
        let mut app = create_test_app();
        app.composer_border = true;
        let draft = "x".repeat(74);
        app.input = draft.clone();
        app.cursor_position = app.input.chars().count();
        let slash_menu_entries = Vec::<SlashMenuEntry>::new();
        let mention_menu_entries = Vec::<String>::new();
        let widget = ComposerWidget::new(&app, 8, &slash_menu_entries, &mention_menu_entries);
        let area = Rect::new(0, 0, 80, 8);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let submit = active_composer_submit_rect(&app, area).expect("enclosed composer submit");
        let input_plane = widget.inner_area(area);
        let text_area = composer_content_geometry(input_plane, false).text_area;
        assert_eq!(
            text_area.right(),
            submit.x.saturating_sub(1),
            "one blank cell must remain between draft text and submit"
        );

        let (cursor_x, cursor_y) = widget.cursor_pos(area).expect("draft cursor");
        assert!(
            cursor_x < submit.x || cursor_x >= submit.right() || cursor_y != submit.y,
            "cursor {cursor_x},{cursor_y} must not land in submit {submit:?}"
        );
        assert_eq!(app.input, draft, "rendering must retain the full draft");

        let first_line: String = (text_area.x..text_area.right())
            .map(|x| buf[(x, cursor_y.saturating_sub(1))].symbol().to_string())
            .collect();
        let continuation: String = (text_area.x..text_area.x.saturating_add(3))
            .map(|x| buf[(x, cursor_y)].symbol().to_string())
            .collect();
        assert_eq!(first_line, "x".repeat(71), "first wrapped draft row");
        assert_eq!(
            continuation, "xxx",
            "draft continuation must remain visible"
        );
        let painted: String = (submit.x..submit.right())
            .map(|x| buf[(x, submit.y)].symbol().to_string())
            .collect();
        assert_eq!(painted, "[↑]", "submit stays intact beside the draft");
    }

    #[test]
    fn composer_send_hitbox_only_exists_where_the_panel_paints() {
        let mut app = create_test_app();
        app.composer_border = true;
        // Widths 6–11 fail COMPOSER_PANEL_MIN_WIDTH: the painter sheds the
        // enclosure there, so no invisible hit target may remain.
        for width in 6..12_u16 {
            let area = Rect::new(0, 0, width, 4);
            assert!(
                active_composer_submit_rect(&app, area).is_none(),
                "no hitbox without the painted panel at width {width}"
            );
        }
        let area = Rect::new(0, 0, 12, 4);
        assert!(
            active_composer_submit_rect(&app, area).is_some(),
            "the minimum panel width hosts the hitbox"
        );
        // Short composer rows and the quiet opt-out shed the hitbox too.
        assert!(active_composer_submit_rect(&app, Rect::new(0, 0, 80, 2)).is_none());
        app.composer_border = false;
        assert!(active_composer_submit_rect(&app, Rect::new(0, 0, 80, 4)).is_none());
    }

    #[test]
    fn quiet_composer_does_not_paint_a_fake_send_control() {
        let mut app = create_test_app();
        app.composer_border = false;
        app.input = "ship it".to_string();
        app.cursor_position = app.input.chars().count();
        let rendered = render_composer(&app, 80, 4);
        assert!(
            !rendered.contains("[↑]"),
            "compact composer must shed the send chrome:\n{rendered}"
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
    fn empty_composer_cursor_follows_idle_prompt_when_border_disabled() {
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

        assert_eq!(widget.cursor_pos(area), Some((2, 2)));
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
        assert_eq!(composer_top_padding(0, 3), 1);
        // content_lines=1
        assert_eq!(composer_top_padding(1, 3), 1);
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
    fn durable_tasks_suppress_the_launch_tableau() {
        let mut app = create_test_app();
        app.task_panel.push(TaskPanelEntry {
            id: "shell_1".to_string(),
            status: "running".to_string(),
            prompt_summary: "cargo test".to_string(),
            duration_ms: Some(100),
            kind: TaskPanelEntryKind::Background,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: None,
            owner_agent_name: None,
            current_tool: None,
            role: None,
            files_touched: 0,
        });

        assert!(!should_render_empty_state(&app));
    }

    #[test]
    fn chat_widget_publishes_wrapped_url_regions_without_touching_cells() {
        let mut app = create_test_app();
        app.low_motion = true;
        let target = "https://example.test/a/very/long/path/that/wraps/across/chat/rows";
        app.add_message(HistoryCell::Assistant {
            content: target.to_string(),
            streaming: false,
        });

        let area = Rect::new(4, 2, 20, 10);
        let mut buf = Buffer::empty(area);
        let _ = crate::tui::osc8::take_frame_links();
        ChatWidget::new(&mut app, area).render(area, &mut buf);
        let regions = crate::tui::osc8::take_frame_links();

        assert!(regions.len() > 1, "narrow chat should wrap: {regions:?}");
        assert!(regions.iter().all(|region| region.target == target));
        assert!(regions.iter().all(|region| {
            area.contains(ratatui::layout::Position {
                x: region.col_start,
                y: region.row,
            }) && area.contains(ratatui::layout::Position {
                x: region.col_end,
                y: region.row,
            })
        }));
        assert!((area.y..area.bottom()).all(|y| {
            (area.x..area.right()).all(|x| {
                let symbol = buf[(x, y)].symbol();
                !symbol.contains('\x1b') && !symbol.contains("]8;;")
            })
        }));
    }

    #[test]
    fn waiting_state_freezes_the_whole_ocean_field() {
        let mut app = create_test_app();
        app.low_motion = false;
        app.fancy_animations = true;
        app.view_stack
            .push(crate::tui::views::HelpView::new_for_locale(app.ui_locale));

        let widget = ChatWidget::new(&mut app, Rect::new(0, 0, 100, 20));

        assert!(!widget.ocean_animated);
        assert!(!widget.ambient_life);
        assert!(!should_render_empty_state(&app));
    }

    #[test]
    fn reduced_motion_gets_no_ambient_life_through_the_completion_breath() {
        // The completion branch of `life_presence` runs before its `!animated`
        // check, so feeding it an ungated clock flashed a full field of fish
        // and jellyfish for ~1.4 s after every successful turn even with
        // `low_motion = true`. Reduced motion means reduced motion.
        for (low_motion, fancy_animations) in [(true, true), (false, false)] {
            let mut app = create_test_app();
            app.low_motion = low_motion;
            app.fancy_animations = fancy_animations;
            app.ocean_completion_started_at = Some(Instant::now());

            let widget = ChatWidget::new(&mut app, Rect::new(0, 0, 100, 20));

            assert_eq!(
                widget.life_presence_fixed, 0,
                "low_motion={low_motion} fancy={fancy_animations} leaked ambient life"
            );
        }

        let mut full = create_test_app();
        full.low_motion = false;
        full.fancy_animations = true;
        full.ocean_completion_started_at = Some(Instant::now());
        let widget = ChatWidget::new(&mut full, Rect::new(0, 0, 100, 20));
        assert!(
            widget.life_presence_fixed > 0,
            "full motion should still get the completion breath"
        );
    }

    #[test]
    fn reduced_and_still_modes_clear_the_one_shot_send_flash() {
        for (low_motion, fancy_animations) in [(true, true), (false, false)] {
            let mut app = create_test_app();
            app.low_motion = low_motion;
            app.fancy_animations = fancy_animations;
            app.last_send_at = Some(Instant::now());
            app.add_message(HistoryCell::User {
                content: "semantic receipt".to_string(),
            });

            let _widget = ChatWidget::new(&mut app, Rect::new(0, 0, 100, 20));
            assert!(
                app.last_send_at.is_none(),
                "non-full motion must not retain a time-based flash"
            );
        }

        let mut full = create_test_app();
        full.low_motion = false;
        full.fancy_animations = true;
        full.last_send_at = Some(Instant::now());
        full.add_message(HistoryCell::User {
            content: "animated receipt".to_string(),
        });
        let _widget = ChatWidget::new(&mut full, Rect::new(0, 0, 100, 20));
        assert!(
            full.last_send_at.is_some(),
            "full motion should retain the active send-flash window"
        );
    }

    #[test]
    fn empty_state_shows_startup_context() {
        let mut app = create_test_app();
        app.onboarding_needs_api_key = false;
        app.workspace = PathBuf::from("/tmp/codewhale-test-workspace");
        app.mcp_configured_count = 2;

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

        assert!(rendered.contains("codewhale"));
        assert!(rendered.contains("/tmp/codewhale-test-workspace · no git · mcp 2"));
        assert!(rendered.contains("What do you want to accomplish?"));
        assert!(!rendered.contains("/workflow /goal /auto"));
    }

    #[test]
    fn empty_state_centers_startup_block_by_actual_text_width() {
        let mut app = create_test_app();
        app.workspace = PathBuf::from("/tmp/codewhale-test-workspace");

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
        let context = "/tmp/codewhale-test-workspace · no git · mcp 0";
        let context_line = text_lines
            .iter()
            .find(|line| line.trim_start() == context)
            .expect("context line");
        let expected_padding = (100usize - UnicodeWidthStr::width(context)) / 2;
        let actual_padding = context_line.chars().take_while(|ch| *ch == ' ').count();

        assert_eq!(actual_padding, expected_padding);
    }

    #[test]
    fn underwater_launch_is_visibly_deep_and_preserves_text_cells() {
        let mut app = create_test_app();
        // App::new reads persisted presentation settings. Other tests swap the
        // isolated settings home in parallel, so this visual contract must pin
        // the theme it is actually asserting instead of inheriting a transient
        // non-underwater choice from the process.
        app.theme_id = crate::palette::ThemeId::Underwater;
        app.ui_theme = palette::UNDERWATER_UI_THEME;
        app.low_motion = false;
        app.fancy_animations = true;
        app.workspace = PathBuf::from("codewhale-test-workspace");
        app.model = "deepseek-v4-pro".to_string();

        let area = Rect::new(0, 0, 100, 20);
        let base = app.ui_theme.surface_bg;
        let context = format!("{} · no git · mcp 0", app.workspace.display());
        let mut buf = Buffer::empty(area);
        // Sample one known point in the live motion path. The old test raced
        // the scheduler between App construction and rendering, which could
        // move the school off-screen on slower Windows runners.
        ChatWidget::new_with_ocean_elapsed(&mut app, area, 0).render(area, &mut buf);

        assert_ne!(buf[(0, 0)].bg, buf[(0, 19)].bg);
        let rendered = buffer_text(&buf, area);
        // One loose wedge school: an eyed lead plus plain members, all
        // facing the same way (facing equals travel by construction).
        let rightward = rendered.matches("><>").count() + rendered.matches("><o>").count();
        let leftward = rendered.matches("<><").count() + rendered.matches("<o><").count();
        assert!(
            rightward == 0 || leftward == 0,
            "one school shares one direction:\n{rendered}"
        );
        let fish_count = rightward + leftward;
        assert!(
            (4..=7).contains(&fish_count),
            "wide idle water should show one cohesive wedge school (got {fish_count}):\n{rendered}"
        );
        let leads = rendered.matches("><o>").count() + rendered.matches("<o><").count();
        assert_eq!(leads, 1, "exactly one eyed lead fish:\n{rendered}");

        let context_x = ((100usize - UnicodeWidthStr::width(context.as_str())) / 2) as u16;
        let context_cell = (0..area.height)
            .find_map(|y| (buf[(context_x, y)].symbol() == "c").then_some((context_x, y)))
            .expect("context line");
        assert_eq!(
            buf[context_cell].bg,
            buf[(0, context_cell.1)].bg,
            "ordinary transcript text must share its row's water color"
        );
        assert_ne!(
            buf[context_cell].bg, base,
            "the water column should continue behind ordinary text"
        );
    }

    #[test]
    fn terminal_owned_theme_keeps_theme_surface_without_ambient_life() {
        let mut app = create_test_app();
        app.theme_id = crate::palette::ThemeId::Whale;
        app.ui_theme = palette::UI_THEME;
        app.low_motion = false;
        app.fancy_animations = true;
        let area = Rect::new(0, 0, 100, 20);
        let base = app.ui_theme.surface_bg;
        let mut buf = Buffer::empty(area);
        let widget = ChatWidget::new(&mut app, area);
        assert!(!widget.ambient_life);
        widget.render(area, &mut buf);

        assert_eq!(buf[(0, 0)].bg, base);
        assert_eq!(buf[(0, 19)].bg, base, "flat keeps the plain theme surface");
        let rendered = buffer_text(&buf, area);
        assert!(
            !rendered.contains("><>") && !rendered.contains("<><"),
            "terminal-owned themes must keep a normal shell without decorative fish:\n{rendered}"
        );
    }

    #[test]
    fn solarized_light_keeps_canonical_surface_without_a_field() {
        let mut app = create_test_app();
        app.theme_id = crate::palette::ThemeId::SolarizedLight;
        app.ui_theme = crate::palette::SOLARIZED_LIGHT_UI_THEME;
        app.low_motion = false;
        app.fancy_animations = true;
        // The old cyan-tinted ramp produced the reported #e1e9da at row 16
        // of a common 30-row viewport.
        let area = Rect::new(0, 0, 100, 30);
        let canonical_base3 = Color::Rgb(0xfd, 0xf6, 0xe3);
        let mut buf = Buffer::empty(area);
        ChatWidget::new(&mut app, area).render(area, &mut buf);

        assert_eq!(buf[(0, 0)].bg, canonical_base3);
        assert_eq!(
            buf[(0, 16)].bg,
            canonical_base3,
            "Solarized Light must not regress to the reported #e1e9da tint"
        );
        assert_eq!(
            buf[(0, 29)].bg,
            canonical_base3,
            "Solarized Light must keep canonical Base3 through the viewport"
        );
        let rendered = buffer_text(&buf, area);
        assert!(
            !rendered.contains("><>") && !rendered.contains("<><"),
            "a theme with no painted field earns no ambient life:\n{rendered}"
        );
    }

    #[test]
    fn underwater_custom_background_keeps_field_depth() {
        let mut app = create_test_app();
        let custom = Color::Rgb(0x1a, 0x1b, 0x26);
        app.theme_id = crate::palette::ThemeId::Underwater;
        app.ui_theme = palette::UNDERWATER_UI_THEME.with_background_color(custom);

        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        ChatWidget::new(&mut app, area).render(area, &mut buf);

        assert_ne!(buf[(0, 0)].bg, custom);
        assert_ne!(
            buf[(0, 0)].bg,
            buf[(0, 29)].bg,
            "custom backgrounds must not flatten the underwater field"
        );
    }

    #[test]
    fn terminal_owned_background_stays_visually_quiet_without_deepsea() {
        let mut app = create_test_app();
        app.theme_id = crate::palette::ThemeId::Terminal;
        app.ui_theme = crate::palette::TERMINAL_UI_THEME;
        app.low_motion = false;
        app.fancy_animations = true;
        let area = Rect::new(0, 0, 100, 20);
        let mut buf = Buffer::empty(area);
        let widget = ChatWidget::new(&mut app, area);
        assert!(!widget.ambient_life);
        widget.render(area, &mut buf);

        assert!(
            (0..area.height).all(|y| (0..area.width).all(|x| buf[(x, y)].bg == Color::Reset)),
            "the Terminal treatment must never paint a background"
        );
        let rendered = buffer_text(&buf, area);
        assert!(
            !rendered.contains("><>") && !rendered.contains("<><"),
            "Terminal must remain a quiet host-owned shell without the selected Deepsea scene:\n{rendered}"
        );
    }

    /// #4208: `CODEWHALE_ASCII_SAFE=1` must narrow every CodeWhale-authored
    /// decorative glyph — whale mark, fish, bubble, context meter, borders,
    /// braille state markers — across real rendered surfaces, not a
    /// hand-picked symbol list.
    #[test]
    fn ascii_safe_tier_covers_whole_rendered_surfaces() {
        let mut app = create_test_app();
        app.low_motion = false;
        app.fancy_animations = true;

        // Idle empty water at a size that earns the whale, fish, and bubble.
        let transcript_area = Rect::new(0, 0, 100, 32);
        let mut transcript = Buffer::empty(transcript_area);
        ChatWidget::new(&mut app, transcript_area).render(transcript_area, &mut transcript);

        // Pre-session launch stage (the Tideline startup surface).
        app.launch.visible = true;
        let launch_area = Rect::new(0, 0, 100, 32);
        let mut launch = Buffer::empty(launch_area);
        {
            let startup = crate::tui::underwater::tideline_startup_from_app(&app).ascii_safe(true);
            crate::tui::underwater::render_tideline_startup(launch_area, &mut launch, &startup);
        }
        app.launch.visible = false;

        // The info line (the shell's bottom row since the placement move)
        // owns the route facts and the block context meter.
        let info_area = Rect::new(0, 0, 100, 1);
        let mut info_buf = Buffer::empty(info_area);
        {
            let segments = crate::tui::ui::frame::info_segments(&app, info_area.width);
            let help_hint = crate::tui::shell_key_routing::info_help_hint(app.ui_locale);
            let info = crate::tui::infoline::InfoLine::new(&app.ui_theme, &help_hint, &segments)
                .ascii_safe(true);
            use ratatui::widgets::Widget;
            Widget::render(info, info_area, &mut info_buf);
        }

        for (surface, buf, rect) in [
            ("idle transcript", &transcript, transcript_area),
            ("launch", &launch, launch_area),
            ("info line", &info_buf, info_area),
        ] {
            for y in rect.y..rect.bottom() {
                for x in rect.x..rect.right() {
                    let mut cell = buf[(x, y)].clone();
                    crate::tui::color_compat::adapt_cell_symbol_for_ascii(&mut cell);
                    assert!(
                        cell.symbol().is_ascii(),
                        "{surface} cell ({x},{y}) {:?} lacks an ASCII-safe alternative",
                        buf[(x, y)].symbol()
                    );
                }
            }
        }
    }

    #[test]
    fn reduced_motion_freezes_the_ocean_without_removing_depth() {
        let mut app = create_test_app();
        app.theme_id = crate::palette::ThemeId::Underwater;
        app.ui_theme = palette::UNDERWATER_UI_THEME;
        app.low_motion = true;
        app.fancy_animations = true;
        let area = Rect::new(0, 0, 100, 20);
        // Drive the sampled clock directly: the freeze must hold even across
        // a 9-second animation-clock jump.
        let mut first = Buffer::empty(area);
        ChatWidget::new_with_ocean_elapsed(&mut app, area, 2_000).render(area, &mut first);

        let mut second = Buffer::empty(area);
        ChatWidget::new_with_ocean_elapsed(&mut app, area, 11_000).render(area, &mut second);

        assert_ne!(first[(0, 0)].bg, first[(0, 19)].bg);
        assert_eq!(first[(0, 0)].bg, second[(0, 0)].bg);
        assert_eq!(first[(11, 14)].symbol(), second[(11, 14)].symbol());
    }

    #[test]
    fn pin_helper_returns_header_when_user_line_is_above_viewport() {
        let history = vec![
            HistoryCell::User {
                content: "remember this prompt".into(),
            },
            HistoryCell::Assistant {
                content: "ok".into(),
                streaming: false,
            },
        ];
        let meta = vec![
            TranscriptLineMeta::CellLine {
                cell_index: 0,
                line_in_cell: 0,
                copy_prefix_width: 0,
                copy_separator_after: crate::tui::ui_text::CopyLineSeparator::None,
            },
            TranscriptLineMeta::CellLine {
                cell_index: 1,
                line_in_cell: 0,
                copy_prefix_width: 0,
                copy_separator_after: crate::tui::ui_text::CopyLineSeparator::None,
            },
        ];
        let map = vec![0, 1];
        let pin = super::scrolled_user_prompt_pin(&history, &meta, &map, 1, 40)
            .expect("scrolled user prompt should yield a pinned header");
        let text: String = pin.spans.iter().map(|span| span.content.as_ref()).collect();
        assert!(
            text.contains("remember this prompt"),
            "expected pinned user text, got {text:?}"
        );
    }

    #[test]
    fn pin_helper_is_idle_when_user_line_is_visible() {
        let history = vec![HistoryCell::User {
            content: "still on screen".into(),
        }];
        let meta = vec![TranscriptLineMeta::CellLine {
            cell_index: 0,
            line_in_cell: 0,
            copy_prefix_width: 0,
            copy_separator_after: crate::tui::ui_text::CopyLineSeparator::None,
        }];
        let map = vec![0];
        assert!(super::scrolled_user_prompt_pin(&history, &meta, &map, 0, 40).is_none());
    }

    #[test]
    fn pinned_prompt_reserves_header_without_hiding_tail_or_shifting_mouse_mapping() {
        let mut app = create_test_app();
        app.pin_last_prompt = true;
        app.add_message(HistoryCell::User {
            content: "keep this goal visible".into(),
        });
        for index in 0..8 {
            app.add_message(HistoryCell::Assistant {
                content: format!("answer {index}"),
                streaming: false,
            });
        }

        let area = Rect::new(2, 5, 48, 5);
        let widget = ChatWidget::new_with_ocean_elapsed(&mut app, area, 0);
        let transcript_area = app
            .viewport
            .last_transcript_area
            .expect("transcript geometry recorded");
        assert_eq!(transcript_area, Rect::new(2, 6, 48, 4));
        assert_eq!(widget.transcript_area, transcript_area);
        assert_eq!(app.viewport.last_transcript_visible, 4);
        assert_eq!(
            app.viewport.last_transcript_top + app.viewport.last_transcript_visible,
            app.viewport.last_transcript_total,
            "reserving the header must still resolve the real transcript to its newest tail"
        );
        let last_rendered: String = widget
            .lines
            .last()
            .expect("tail line rendered")
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        let last_cached: String = app
            .viewport
            .transcript_cache
            .lines()
            .last()
            .expect("tail line cached")
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(last_rendered, last_cached);

        let pinned_row = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: area.x,
            row: area.y,
            modifiers: KeyModifiers::NONE,
        };
        assert!(
            crate::tui::mouse_ui::selection_point_from_mouse(&app, pinned_row).is_none(),
            "the sticky header must not impersonate transcript line `top`"
        );

        let meta = app.viewport.transcript_cache.line_meta();
        let (line_offset, expected_cell) = meta[app.viewport.last_transcript_top..]
            .iter()
            .take(app.viewport.last_transcript_visible)
            .enumerate()
            .find_map(|(offset, meta)| meta.cell_line().map(|(cell, _)| (offset, cell)))
            .expect("visible transcript contains a cell row");
        let body_row = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: transcript_area.x,
            row: transcript_area.y + u16::try_from(line_offset).unwrap(),
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            crate::tui::mouse_ui::transcript_cell_index_from_mouse(&app, body_row),
            Some(expected_cell),
            "click, drag, selection, and right-click must share the actual body geometry"
        );
    }

    #[test]
    fn fish_glyph_always_matches_screen_direction() {
        assert_eq!(fish_mark(true), "><>");
        assert_eq!(fish_mark(false), "<><");
        assert!(fish_heading(8, 9, 10, false));
        assert!(!fish_heading(10, 9, 8, true));
        assert!(fish_heading(8, 9, 9, false));
        assert!(!fish_heading(10, 9, 9, true));

        // Mirrored tracks are the regression case: a forward path flag can
        // correspond to decreasing screen x. Heading follows x, not the flag.
        assert!(!fish_heading(74, 73, 72, true));
    }

    /// Render a chat field carrying `rows` of history and return its rows.
    fn history_field_rows(rows: usize) -> Vec<String> {
        let mut app = create_test_app();
        app.low_motion = false;
        app.fancy_animations = true;
        for index in 0..rows {
            app.add_message(HistoryCell::Assistant {
                content: format!("history row {index}"),
                streaming: false,
            });
        }
        app.viewport.transcript_scroll = TranscriptScroll::at_line(0);
        let area = Rect::new(0, 0, 100, 20);
        let widget = ChatWidget::new(&mut app, area);
        assert!(widget.ambient_life);
        assert!(widget.ocean_animated);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        buffer_text(&buf, area)
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn browsing_history_keeps_fish_in_available_water() {
        // Short transcript rows own their text plus a quiet gutter, not the
        // entire width. Browsing still holds the school in the clear water.
        let rows = history_field_rows(4);
        let rendered = rows.join("\n");
        assert!(
            rendered.contains("><>") || rendered.contains("<><"),
            "open water below the transcript should hold fish:\n{rendered}"
        );
        for index in 0..4 {
            assert!(
                rendered.contains(&format!("history row {index}")),
                "ambient life damaged history row {index}:\n{rendered}"
            );
        }
    }

    #[test]
    fn active_tail_keeps_fish_after_message_submit() {
        let mut app = create_test_app();
        app.low_motion = false;
        app.fancy_animations = true;
        for index in 0..18 {
            app.add_message(HistoryCell::Assistant {
                content: format!("release check {index:02}"),
                streaming: false,
            });
        }
        app.is_loading = true;
        app.runtime_turn_status = Some("in_progress".to_string());
        app.turn_started_at = Some(
            Instant::now()
                .checked_sub(std::time::Duration::from_millis(900))
                .expect("recent turn start"),
        );
        let area = Rect::new(0, 0, 80, 24);
        let widget = ChatWidget::new_with_ocean_elapsed(&mut app, area, 0);
        assert!(widget.ambient_life);
        assert!(widget.ocean_animated);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        let rendered = buffer_text(&buf, area);
        assert!(
            rendered.contains("><") || rendered.contains("<o"),
            "submitting a message must not empty the ocean:\n{rendered}"
        );
        assert!(rendered.contains("release check 17"), "{rendered}");
    }

    #[test]
    fn completed_turn_keeps_bounded_ocean_settle() {
        let mut app = create_test_app();
        app.low_motion = false;
        app.fancy_animations = true;
        app.add_message(HistoryCell::Assistant {
            content: "release receipt".to_string(),
            streaming: false,
        });
        app.runtime_turn_status = Some("completed".to_string());
        app.ocean_completion_started_at = Some(Instant::now());
        let area = Rect::new(0, 0, 80, 24);
        let widget = ChatWidget::new_with_ocean_elapsed(&mut app, area, 0);
        assert!(widget.ambient_life);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        let rendered = buffer_text(&buf, area);
        assert!(
            rendered.contains("><") || rendered.contains("<o"),
            "the completion settle must not snap the ocean empty:\n{rendered}"
        );
        assert!(rendered.contains("release receipt"), "{rendered}");
    }

    #[test]
    fn a_field_full_of_transcript_holds_no_fish() {
        // Full-width prose really does claim the whole field; short status
        // lines no longer impersonate this fixture.
        let mut app = create_test_app();
        app.low_motion = false;
        app.fancy_animations = true;
        for _ in 0..30 {
            app.add_message(HistoryCell::Assistant {
                content: "X".repeat(100),
                streaming: false,
            });
        }
        app.viewport.transcript_scroll = TranscriptScroll::at_line(0);
        let area = Rect::new(0, 0, 100, 20);
        let widget = ChatWidget::new_with_ocean_elapsed(&mut app, area, 0);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        let rendered = buffer_text(&buf, area);
        assert!(
            !rendered.contains("><>") && !rendered.contains("<><"),
            "a full transcript is not an aquarium:\n{rendered}"
        );
    }

    fn todo_write_cell(item: Option<&str>) -> HistoryCell {
        let items = item.map_or_else(
            || "[]".to_string(),
            |content| format!(r#"[{{"id":1,"content":"{content}","status":"pending"}}]"#),
        );
        let count = usize::from(item.is_some());
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "todo_write".to_string(),
            status: ToolStatus::Success,
            input_summary: Some(format!("todos: <{count} items>")),
            output: Some(format!(
                "Todo list updated ({count} items, 0% settled)\n{{\"items\":{items},\"completion_pct\":0}}"
            )),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        }))
    }

    #[test]
    fn todo_write_renders_only_the_latest_successful_snapshot() {
        let mut app = create_test_app();
        app.add_message(todo_write_cell(Some("stale task")));
        app.add_message(HistoryCell::Assistant {
            content: "working".to_string(),
            streaming: false,
        });
        app.add_message(todo_write_cell(Some("current task")));

        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        ChatWidget::new(&mut app, area).render(area, &mut buf);
        let rendered = buffer_text(&buf, area);

        assert!(!rendered.contains("stale task"), "{rendered}");
        assert!(rendered.contains("current task"), "{rendered}");
        assert_eq!(app.collapsed_cell_map, vec![1, 2]);
    }

    #[test]
    fn empty_todo_write_hides_the_previous_snapshot() {
        let mut app = create_test_app();
        app.add_message(todo_write_cell(Some("finished task")));
        let active = app.active_cell.get_or_insert_with(ActiveCell::new);
        active.push_untracked(todo_write_cell(None));
        app.bump_active_cell_revision();

        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        ChatWidget::new(&mut app, area).render(area, &mut buf);
        let rendered = buffer_text(&buf, area);

        assert!(!rendered.contains("finished task"), "{rendered}");
        assert!(!rendered.contains("todo_write"), "{rendered}");
        assert!(app.collapsed_cell_map.is_empty());
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
        app.theme_id = crate::palette::ThemeId::Whale;
        app.ui_theme = palette::UI_THEME.with_background_color(custom);
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
    fn approval_inline_band_caps_at_half_the_viewport_and_keeps_actions_visible() {
        let command = (0..24)
            .map(|index| format!("printf command-{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let request = crate::tui::approval::ApprovalRequest::new(
            "approval-long",
            "exec_shell",
            "Run a long shell command",
            &serde_json::json!({ "command": command }),
            "exec_shell:long",
        );
        let view = crate::tui::approval::ApprovalView::new(request.clone());
        let widget = ApprovalWidget::new(&request, &view);
        let area = Rect::new(0, 0, 100, 30);
        let region = widget.inline_region(area);

        assert_eq!(region.bottom(), area.bottom());
        assert!(region.height <= area.height.div_ceil(2), "{region:?}");

        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        let rendered = buffer_text(&buf, area);
        assert!(rendered.contains("[1 / y]"), "{rendered}");
        assert!(rendered.contains("[Esc]"), "{rendered}");
        assert!(rendered.contains("truncated"), "{rendered}");
    }

    #[test]
    fn approval_compact_tiers_preserve_command_before_falling_back_to_details() {
        let request = crate::tui::approval::ApprovalRequest::new(
            "approval-tiers",
            "exec_shell",
            "Print a localized verification marker",
            &serde_json::json!({ "command": "printf '安全確認'" }),
            "exec_shell:printf",
        );
        let view = crate::tui::approval::ApprovalView::new(request.clone());
        let widget = ApprovalWidget::new(&request, &view);

        for area in [Rect::new(0, 0, 80, 24), Rect::new(0, 0, 60, 16)] {
            let region = widget.inline_region(area);
            assert_eq!(region.bottom(), area.bottom());
            assert!(region.height < area.height, "{area:?}: {region:?}");

            let mut buf = Buffer::empty(area);
            widget.render(area, &mut buf);
            let rendered = buffer_text(&buf, area);
            assert!(rendered.contains("Command:"), "{area:?}: {rendered}");
            for marker in ['安', '全', '確', '認'] {
                assert!(rendered.contains(marker), "{area:?}: {rendered}");
            }
            assert!(rendered.contains("[1 / y]"), "{area:?}: {rendered}");
            assert!(rendered.contains("[Esc]"), "{area:?}: {rendered}");
        }

        let tiny = Rect::new(0, 0, 40, 12);
        let mut buf = Buffer::empty(tiny);
        widget.render(tiny, &mut buf);
        let rendered = buffer_text(&buf, tiny);
        assert!(rendered.contains("[1 / y]"), "{rendered}");
        assert!(rendered.contains("[Esc]"), "{rendered}");
        assert!(
            rendered.contains(crate::tui::shell_key_routing::tool_details_chord().as_ref()),
            "{rendered}"
        );
    }

    #[test]
    fn approval_truncation_hint_uses_platform_details_chord_in_every_locale() {
        let details = crate::tui::shell_key_routing::tool_details_chord();
        for locale in Locale::shipped() {
            let hint = approval_truncation_hint(*locale);
            assert!(hint.contains(details.as_ref()), "{locale:?}: {hint}");
            assert!(!hint.contains("[v]"), "{locale:?}: {hint}");
        }
    }

    #[test]
    fn repo_law_approval_has_distinct_authority_grammar() {
        let request = crate::tui::approval::ApprovalRequest::new(
            "approval-law",
            "edit_file",
            "Repo law holds this write: \"manifest review\" protects Cargo.toml (matched Cargo.toml, .codewhale/constitution.json)",
            &serde_json::json!({ "path": "Cargo.toml", "old": "a", "new": "b" }),
            "edit_file:Cargo.toml",
        );
        assert!(request.is_repo_law_prompt());
        let view = crate::tui::approval::ApprovalView::new(request.clone());
        let widget = ApprovalWidget::new(&request, &view);
        let area = Rect::new(0, 0, 120, 30);
        let mut buf = Buffer::empty(area);

        widget.render(area, &mut buf);
        let rendered = buffer_text(&buf, area);
        assert!(rendered.contains("REPO LAW"), "{rendered}");
        assert!(rendered.contains("Repository constitution"), "{rendered}");
        assert!(rendered.contains("approval-gated postures"), "{rendered}");
        assert!(rendered.contains("Cargo.toml"), "{rendered}");
        assert!((0..area.height).any(|y| {
            let cell = &buf[(1, y)];
            cell.symbol() == "═" && cell.fg == palette::STATUS_WARNING
        }));
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
        // user that particular option is "always"/permanent. The distinct
        // `[p]` row may use that word for an exact repo-scoped grant.
        let request = crate::tui::approval::ApprovalRequest::new(
            "approval-1",
            "exec_shell",
            "Run git commit",
            &serde_json::json!({ "command": "git commit -m fix" }),
            "exec_shell:git commit",
        );

        // Full card (tall): full option rows render the session-scoped label.
        let full = render_approval_request(&request, Rect::new(0, 0, 100, 30));
        let full_session_option = full
            .lines()
            .find(|line| line.contains("[2 / a]"))
            .expect("full approval card should render the session option");
        assert!(
            full_session_option.to_lowercase().contains("this session")
                && !full_session_option.to_lowercase().contains("always"),
            "full approval option must state session scope without saying always:\n{full}"
        );

        // Short terminal: the reserved controls still render the session-scoped
        // option `[2 / a]` without calling that option "always".
        let compact = render_approval_request(&request, Rect::new(0, 0, 60, 17));
        let compact_session_option = compact
            .lines()
            .find(|line| line.contains("[2 / a]"))
            .expect("short approval card should render the session option");
        assert!(
            compact_session_option.to_lowercase().contains("session")
                && !compact_session_option.to_lowercase().contains("always"),
            "short-terminal controls must label [2 / a] as session-scoped:\n{compact}"
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

        assert!(
            rendered.contains("s allow once + always ask exact rule"),
            "{rendered}"
        );
        assert!(
            rendered.contains("Always allow this exact rule in this repo"),
            "{rendered}"
        );
        assert!(rendered.contains("Save:"), "{rendered}");
        assert!(rendered.contains("1 ask rule"), "{rendered}");
        assert!(rendered.contains("1 allow rule"), "{rendered}");
        assert!(
            rendered.contains("tool=exec_shell command=cargo test --workspace"),
            "{rendered}"
        );
        assert!(rendered.contains("command_exact=true"), "{rendered}");
        assert!(rendered.contains("workspace=/workspace"), "{rendered}");
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
                rendered.contains("1 allow rule"),
                "{tool_name}:\n{rendered}"
            );
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
        assert!(rendered.contains("2 allow rules"), "{rendered}");
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
                "replace": [
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
                !rendered.contains("s allow once + always ask exact rule"),
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

        // At 20 rows the compact band preserves both a load-bearing preview
        // row and the complete action set.
        assert!(rendered.contains("Preview:"), "{rendered}");
        assert!(rendered.contains("+new"), "{rendered}");
        assert!(rendered.contains("truncated"), "{rendered}");
        assert!(
            rendered.contains(crate::tui::shell_key_routing::tool_details_chord().as_ref()),
            "{rendered}"
        );
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
        // The compact band keeps the transcript visible without hiding the
        // load-bearing command; full content remains one details chord away.
        assert!(rendered.contains("Command:"), "{rendered}");
        assert!(rendered.contains("cargo clippy"), "{rendered}");
        assert!(rendered.contains("truncated"), "{rendered}");
        assert!(
            rendered.contains(crate::tui::shell_key_routing::tool_details_chord().as_ref()),
            "{rendered}"
        );
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

    #[test]
    fn receipt_settle_cascade_is_bounded_and_ordered() {
        assert!(receipt_is_settling(0, 0));
        assert!(!receipt_is_settling(0, 140));
        assert!(receipt_is_settling(1, 140));
        assert!(!receipt_is_settling(6, 560));
        assert!(!receipt_is_settling(60, 560));
    }

    #[test]
    fn fish_flee_is_one_shot_and_returns_to_ambient_origin() {
        assert_eq!(fish_flee_offset(0), 0);
        assert!(fish_flee_offset(400) >= 8);
        assert_eq!(fish_flee_offset(800), 0);
        assert_eq!(fish_flee_offset(8_000), 0);
    }
}
