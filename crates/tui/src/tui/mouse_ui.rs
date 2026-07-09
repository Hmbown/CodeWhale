use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::localization::MessageId;
use crate::tui::app::{App, PendingInputAction, SidebarRowAction};
use crate::tui::command_palette::{
    CommandPaletteView, build_entries as build_command_palette_entries,
};
use crate::tui::context_menu::{ContextMenuEntry, ContextMenuView};
use crate::tui::history::HistoryCell;
use crate::tui::scrolling::{ScrollDirection, TranscriptScroll};
use crate::tui::selection::{SelectionAutoscroll, TranscriptSelectionPoint};
use crate::tui::ui_text::{
    history_cell_to_text, line_to_plain, slice_text, text_display_width, truncate_line_to_width,
};
use crate::tui::views::{ContextMenuAction, HelpView, ModalKind, ViewEvent};

// These functions will need to be imported from ui.rs or we can just import crate::tui::ui::*.
use crate::tui::ui::{
    copy_cell_to_clipboard, detail_target_label, open_context_inspector,
    open_details_pager_for_cell, open_pager_for_selection,
};

const COMPOSER_MOUSE_SCROLL_LINES: usize = 3;

pub(crate) fn should_drop_loading_mouse_motion(app: &App, mouse: MouseEvent) -> bool {
    if !app.is_loading {
        return false;
    }

    match mouse.kind {
        MouseEventKind::Moved => {
            let over_sidebar = mouse_hits_rect(mouse, app.viewport.last_sidebar_area);
            let over_pending_input = mouse_hits_rect(mouse, app.viewport.last_pending_input_area);
            let was_over_sidebar = app.last_mouse_pos.is_some_and(|(column, row)| {
                point_hits_rect(column, row, app.viewport.last_sidebar_area)
            });
            let was_over_pending_input = app.last_mouse_pos.is_some_and(|(column, row)| {
                point_hits_rect(column, row, app.viewport.last_pending_input_area)
            });
            !(over_sidebar
                || was_over_sidebar
                || over_pending_input
                || was_over_pending_input
                || app.sidebar_hover_tooltip.is_some())
        }
        MouseEventKind::Drag(_) => {
            // Sidebar drag-to-resize must stay live during active turns —
            // dropping these events wedges the resize state mid-drag (#3063).
            !app.viewport.transcript_selection.dragging
                && !app.viewport.transcript_scrollbar_dragging
                && !app.sidebar_resizing
        }
        _ => false,
    }
}

fn toggle_tool_run_expand(app: &mut App, mouse: MouseEvent) -> bool {
    if !app.tool_collapse_active() {
        return false;
    }
    let Some(rendered_idx) = transcript_cell_index_from_mouse(app, mouse) else {
        return false;
    };
    let original_idx = app.original_cell_index_for_rendered(rendered_idx);
    if app.tool_run_start_for_history_index(original_idx) != Some(original_idx) {
        return false;
    }
    app.toggle_tool_run_expansion_at(original_idx)
}

/// Handle mouse events on the sidebar resize handle (the 1-col vertical bar
/// between the chat area and the sidebar). Returns true when the event was
/// consumed so other handlers skip it.
fn handle_sidebar_resize_mouse(app: &mut App, mouse: MouseEvent) -> bool {
    let Some(handle) = app.last_sidebar_handle_area else {
        return false;
    };

    let hit = mouse.column == handle.x
        && mouse.row >= handle.y
        && mouse.row < handle.y.saturating_add(handle.height);

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) if hit => {
            app.sidebar_resizing = true;
            app.sidebar_resize_anchor_x = mouse.column;
            app.sidebar_resize_anchor_width = app.last_sidebar_area.map(|a| a.width).unwrap_or(28);
            app.needs_redraw = true;
            true
        }
        MouseEventKind::Drag(MouseButton::Left) if app.sidebar_resizing => {
            let delta = app.sidebar_resize_anchor_x as i32 - mouse.column as i32;
            let new_width = (app.sidebar_resize_anchor_width as i32 + delta).max(24) as u16;
            let total = app.sidebar_resize_total_width.max(1);
            let new_pct = ((new_width as u32 * 100) / total as u32).clamp(10, 50) as u16;
            if new_pct != app.sidebar_width_percent {
                app.sidebar_width_percent = new_pct;
                app.needs_redraw = true;
            }
            true
        }
        MouseEventKind::Up(MouseButton::Left) if app.sidebar_resizing => {
            app.sidebar_resizing = false;
            app.sidebar_width_dirty = true;
            app.needs_redraw = true;
            true
        }
        _ => false,
    }
}

fn pending_input_action_at(app: &App, mouse: MouseEvent) -> Option<PendingInputAction> {
    app.viewport
        .pending_input_action_regions
        .iter()
        .rev()
        .find_map(|region| {
            point_hits_rect(mouse.column, mouse.row, Some(region.rect)).then_some(region.action)
        })
}

fn pending_input_command_for_action(action: PendingInputAction) -> String {
    match action {
        PendingInputAction::SendQueued { index } => format!("/queue send {}", index + 1),
        PendingInputAction::CancelQueued { index } => format!("/queue drop {}", index + 1),
        PendingInputAction::SendNextQueued => "/queue send 1".to_string(),
    }
}

fn handle_pending_input_preview_mouse(app: &mut App, mouse: MouseEvent) -> Option<Vec<ViewEvent>> {
    let over_preview = mouse_hits_rect(mouse, app.viewport.last_pending_input_area);
    let action = over_preview
        .then(|| pending_input_action_at(app, mouse))
        .flatten();

    match mouse.kind {
        MouseEventKind::Moved => {
            if app.viewport.hovered_pending_input_action != action {
                app.viewport.hovered_pending_input_action = action;
                app.needs_redraw = true;
            }
            over_preview.then(Vec::new)
        }
        MouseEventKind::Down(MouseButton::Left) if over_preview => {
            app.viewport.hovered_pending_input_action = action;
            app.needs_redraw = true;
            action.map_or_else(
                || Some(Vec::new()),
                |action| {
                    Some(vec![ViewEvent::CommandPaletteSelected {
                        action: crate::tui::views::CommandPaletteAction::ExecuteCommand {
                            command: pending_input_command_for_action(action),
                        },
                    }])
                },
            )
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if over_preview => Some(Vec::new()),
        _ if over_preview => Some(Vec::new()),
        _ => {
            if app.viewport.hovered_pending_input_action.is_some() {
                app.viewport.hovered_pending_input_action = None;
                app.needs_redraw = true;
            }
            None
        }
    }
}

/// Map a mouse (column, row) within the composer area to a char index
/// in the composer input string. Uses the inner content rect (border-aware)
/// for coordinate mapping, and accounts for vertical padding and scroll offset.
fn mouse_pos_to_char_index(app: &App, col: u16, row: u16, inner: Rect) -> Option<usize> {
    let rel_col = col.saturating_sub(inner.x) as usize;
    let rel_row = row.saturating_sub(inner.y) as usize;

    if app.input.is_empty() {
        return Some(0);
    }

    let width = inner.width.max(1) as usize;
    let wrapped = crate::tui::widgets::wrap_input_lines_for_mouse(&app.input, width);

    // Subtract the vertical top-padding (centering of short inputs).
    let text_row = rel_row.saturating_sub(app.viewport.last_composer_top_padding);

    // Add the scroll offset (lines scrolled out of view).
    let absolute_row = text_row + app.viewport.last_composer_scroll_offset;

    if absolute_row >= wrapped.len() {
        return Some(app.input.chars().count());
    }

    let (line_start, line_text) = &wrapped[absolute_row];

    let mut char_offset = 0usize;
    let mut col_used = 0usize;
    for g in line_text.graphemes(true) {
        let gw = g.width();
        if col_used + gw > rel_col {
            break;
        }
        col_used += gw;
        char_offset += g.chars().count();
    }
    Some(line_start + char_offset)
}

fn composer_wrapped_cursor_row_col(
    input: &str,
    cursor: usize,
    wrapped: &[(usize, String)],
) -> (usize, usize) {
    let total = input.chars().count();
    let cursor = cursor.min(total);

    for (idx, (line_start, line_text)) in wrapped.iter().enumerate() {
        let next_start = wrapped
            .get(idx + 1)
            .map(|(start, _)| *start)
            .unwrap_or_else(|| total.saturating_add(1));

        if cursor >= *line_start && cursor < next_start {
            let line_len = line_text.chars().count();
            return (idx, cursor.saturating_sub(*line_start).min(line_len));
        }
    }

    let row = wrapped.len().saturating_sub(1);
    let col = wrapped
        .get(row)
        .map(|(_, line_text)| line_text.chars().count())
        .unwrap_or(0);
    (row, col)
}

fn move_composer_cursor_by_wrapped_rows(app: &mut App, inner: Rect, rows: isize) {
    if app.input.is_empty() || rows == 0 {
        return;
    }

    let width = inner.width.max(1) as usize;
    let wrapped = crate::tui::widgets::wrap_input_lines_for_mouse(&app.input, width);
    if wrapped.len() <= 1 {
        return;
    }

    let (current_row, current_col) =
        composer_wrapped_cursor_row_col(&app.input, app.cursor_position, &wrapped);
    let max_row = wrapped.len().saturating_sub(1);
    let target_row = if rows.is_negative() {
        current_row.saturating_sub(rows.unsigned_abs())
    } else {
        current_row.saturating_add(rows as usize).min(max_row)
    };

    if target_row == current_row {
        return;
    }

    let (target_start, target_text) = &wrapped[target_row];
    let target_len = target_text.chars().count();
    let total = app.input.chars().count();
    app.clear_selection();
    app.cursor_position = target_start
        .saturating_add(current_col.min(target_len))
        .min(total);
    app.needs_redraw = true;
}

/// Delegate WorkflowPanel mouse controls (#4121 / COH-06): header toggles,
/// cancel emits, phase chips select, and body clicks grant panel focus without
/// leaking into transcript selection.
fn handle_workflow_panel_mouse(app: &mut App, mouse: MouseEvent) -> bool {
    let Some(area) = app.viewport.last_workflow_panel_area else {
        return false;
    };
    if !mouse_hits_rect(mouse, Some(area)) {
        return false;
    }
    let handled = app
        .workflow_panel
        .as_mut()
        .is_some_and(|panel| panel.handle_mouse(area, mouse));
    if handled {
        if let Some(run_id) = app
            .workflow_panel
            .as_mut()
            .and_then(|panel| panel.take_cancel_emit())
        {
            app.status_message = Some(format!(
                "Cancelling workflow {run_id}… (dispatch via /workflow cancel {run_id})"
            ));
        }
        app.needs_redraw = true;
    }
    handled
}

/// Handle mouse events within the composer area.
/// Returns true if the event was consumed.
pub(crate) fn handle_composer_mouse(app: &mut App, mouse: MouseEvent) -> bool {
    // Use outer area for hit-testing (includes border).
    let Some(area) = app.viewport.last_composer_area else {
        return false;
    };
    if mouse.column < area.x
        || mouse.column >= area.x + area.width
        || mouse.row < area.y
        || mouse.row >= area.y + area.height
    {
        return false;
    }
    // Use inner content rect for coordinate-to-char mapping (border-aware).
    let inner = app.viewport.last_composer_content.unwrap_or(area);

    match mouse.kind {
        MouseEventKind::ScrollUp => {
            move_composer_cursor_by_wrapped_rows(
                app,
                inner,
                -(COMPOSER_MOUSE_SCROLL_LINES as isize),
            );
            true
        }
        MouseEventKind::ScrollDown => {
            move_composer_cursor_by_wrapped_rows(app, inner, COMPOSER_MOUSE_SCROLL_LINES as isize);
            true
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(pos) = mouse_pos_to_char_index(app, mouse.column, mouse.row, inner) {
                app.cursor_position = pos;
                app.selection_anchor = None;
                app.needs_redraw = true;
            }
            true
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(pos) = mouse_pos_to_char_index(app, mouse.column, mouse.row, inner) {
                if app.selection_anchor.is_none() {
                    app.selection_anchor = Some(app.cursor_position);
                }
                app.cursor_position = pos;
                app.needs_redraw = true;
            }
            true
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if app.selection_anchor == Some(app.cursor_position) {
                app.selection_anchor = None;
            }
            true
        }
        _ => false,
    }
}

pub(crate) fn handle_mouse_event(app: &mut App, mouse: MouseEvent) -> Vec<ViewEvent> {
    if app.view_stack.top_kind() == Some(ModalKind::ContextMenu) {
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Right)) {
            app.view_stack.pop();
            open_context_menu(app, mouse);
            return Vec::new();
        }
        return app.view_stack.handle_mouse(mouse);
    }

    if !app.view_stack.is_empty() {
        app.needs_redraw = true;
        return app.view_stack.handle_mouse(mouse);
    }

    // Sidebar resize handle — check before composer so it doesn't compete
    // with text selection / scrolling.
    if handle_sidebar_resize_mouse(app, mouse) {
        return Vec::new();
    }

    // WorkflowPanel toggle / cancel (#4121) before composer so the strip
    // above the input remains clickable.
    if handle_workflow_panel_mouse(app, mouse) {
        return Vec::new();
    }

    if let Some(events) = handle_pending_input_preview_mouse(app, mouse) {
        return events;
    }

    // Composer mouse events take priority over transcript.
    if handle_composer_mouse(app, mouse) {
        return Vec::new();
    }

    // Scroll events while the cursor is over the right-hand sidebar must not
    // drive the transcript scroll. The sidebar is a fixed dashboard with no
    // scroll state of its own, so consume the wheel event instead of leaking
    // it into the transcript viewport behind it.
    if matches!(
        mouse.kind,
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
    ) && app.viewport.last_sidebar_area.is_some_and(|area| {
        mouse.column >= area.x
            && mouse.column < area.x.saturating_add(area.width)
            && mouse.row >= area.y
            && mouse.row < area.y.saturating_add(area.height)
    }) {
        return Vec::new();
    }

    match mouse.kind {
        MouseEventKind::Moved => {
            // Update last mouse position for tooltip rendering.
            app.last_mouse_pos = Some((mouse.column, mouse.row));

            // Check sidebar sections for hover popovers. Only surface a
            // popover when the hovered row lost information in the compact
            // sidebar view.
            let mut found = false;
            for section in &app.sidebar_hover.sections {
                if mouse.column >= section.content_area.x
                    && mouse.column
                        < section
                            .content_area
                            .x
                            .saturating_add(section.content_area.width)
                    && mouse.row >= section.content_area.y
                    && mouse.row
                        < section
                            .content_area
                            .y
                            .saturating_add(section.content_area.height)
                {
                    if let Some(row) = section.rows.iter().find(|row| row.row_y == mouse.row) {
                        let desired = row.is_truncated.then(|| {
                            if let Some(detail) = row.detail.as_deref()
                                && !detail.trim().is_empty()
                            {
                                format!("{}\n{detail}", row.full_text)
                            } else {
                                row.full_text.clone()
                            }
                        });
                        if app.sidebar_hover_tooltip != desired {
                            app.sidebar_hover_tooltip = desired;
                            app.needs_redraw = true;
                        }
                        found = true;
                        break;
                    } else if section.rows.is_empty() {
                        let line_idx = (mouse.row.saturating_sub(section.content_area.y)) as usize;
                        if let Some(full) = section.lines.get(line_idx) {
                            let truncated =
                                text_display_width(full) > section.content_area.width as usize;
                            let desired = truncated.then(|| full.clone());
                            if app.sidebar_hover_tooltip != desired {
                                app.sidebar_hover_tooltip = desired;
                                app.needs_redraw = true;
                            }
                            found = true;
                            break;
                        }
                    }
                }
            }
            if !found && app.sidebar_hover_tooltip.is_some() {
                app.sidebar_hover_tooltip = None;
                app.needs_redraw = true;
            }
        }
        MouseEventKind::ScrollUp => {
            let update = app.viewport.mouse_scroll.on_scroll(ScrollDirection::Up);
            app.viewport.pending_scroll_delta = app
                .viewport
                .pending_scroll_delta
                .saturating_add(update.delta_lines);
            if update.delta_lines != 0 {
                app.user_scrolled_during_stream = true;
                app.needs_redraw = true;
            }
        }
        MouseEventKind::ScrollDown => {
            let update = app.viewport.mouse_scroll.on_scroll(ScrollDirection::Down);
            app.viewport.pending_scroll_delta = app
                .viewport
                .pending_scroll_delta
                .saturating_add(update.delta_lines);
            if update.delta_lines != 0 {
                app.user_scrolled_during_stream = true;
                app.needs_redraw = true;
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            app.viewport.transcript_scrollbar_dragging = false;
            app.viewport.selection_autoscroll = None;

            // #3028/#4009: Check sidebar hover state for clickable rows before
            // falling through to transcript selection. Command rows still use
            // the command-palette pipeline; agent rows are direct UI actions.
            if let Some(action) = sidebar_click_action(app, mouse) {
                match action {
                    SidebarRowAction::Command(command) => {
                        use crate::tui::views::CommandPaletteAction;
                        return vec![ViewEvent::CommandPaletteSelected {
                            action: CommandPaletteAction::ExecuteCommand { command },
                        }];
                    }
                    SidebarRowAction::InsertText(text) => {
                        use crate::tui::views::CommandPaletteAction;
                        return vec![ViewEvent::CommandPaletteSelected {
                            action: CommandPaletteAction::InsertText { text },
                        }];
                    }
                    SidebarRowAction::ToggleAgentDetails { agent_id } => {
                        if !app.expanded_sidebar_agents.insert(agent_id.clone()) {
                            app.expanded_sidebar_agents.remove(&agent_id);
                            app.status_message = Some("Agent details collapsed".to_string());
                        } else {
                            app.status_message = Some("Agent details expanded".to_string());
                        }
                        app.needs_redraw = true;
                        return Vec::new();
                    }
                    SidebarRowAction::OpenAgentDetail { agent_id } => {
                        // #2889 slice / dogfood A3: drill from the expanded
                        // dossier into the child's transcript card (action
                        // tree, status, summary) in the detail pager.
                        let cell_index = app.history.iter().position(|cell| {
                            matches!(
                                cell,
                                HistoryCell::SubAgent(
                                    crate::tui::history::SubAgentCell::Delegate(card)
                                ) if card.agent_id == agent_id
                            )
                        });
                        match cell_index {
                            Some(cell_index) => {
                                open_details_pager_for_cell(app, cell_index);
                            }
                            None => {
                                app.status_message = Some(format!(
                                    "No transcript card for {agent_id} yet — use handle_read agent:{agent_id}/full_transcript"
                                ));
                            }
                        }
                        app.needs_redraw = true;
                        return Vec::new();
                    }
                    SidebarRowAction::CancelAgent { agent_id } => {
                        return vec![ViewEvent::SidebarAgentCancel { agent_id }];
                    }
                }
            }

            // Click on the transcript scrollbar gutter starts a scrollbar
            // drag so the visible thumb remains interactive for users who
            // prefer mouse-based navigation.
            if mouse_hits_transcript_scrollbar(app, mouse) {
                app.viewport.transcript_scrollbar_dragging = true;
                return Vec::new();
            }

            if mouse_hits_rect(mouse, app.viewport.jump_to_latest_button_area) {
                app.scroll_to_bottom();
                return Vec::new();
            }

            if toggle_tool_run_expand(app, mouse) {
                return Vec::new();
            }

            if let Some(point) = selection_point_from_mouse(app, mouse) {
                app.viewport.transcript_selection.anchor = Some(point);
                app.viewport.transcript_selection.head = Some(point);
                app.viewport.transcript_selection.dragging = true;

                if app.is_loading
                    && app.viewport.transcript_scroll.is_at_tail()
                    && let Some(anchor) = TranscriptScroll::anchor_for(
                        app.viewport.transcript_cache.line_meta(),
                        app.viewport.last_transcript_top,
                    )
                {
                    app.viewport.transcript_scroll = anchor;
                }
            } else if app.viewport.transcript_selection.is_active() {
                app.viewport.transcript_selection.clear();
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if app.viewport.transcript_scrollbar_dragging {
                scroll_transcript_to_mouse_row(app, mouse.row);
                return Vec::new();
            }

            if app.viewport.transcript_selection.dragging {
                update_selection_drag(app, mouse);
            }
        }
        MouseEventKind::Up(MouseButton::Left) if app.viewport.transcript_scrollbar_dragging => {
            app.viewport.transcript_scrollbar_dragging = false;
            app.viewport.selection_autoscroll = None;
            app.needs_redraw = true;
        }
        MouseEventKind::Up(MouseButton::Left) if app.viewport.transcript_selection.dragging => {
            app.viewport.transcript_selection.dragging = false;
            app.viewport.selection_autoscroll = None;
            if selection_has_content(app) {
                copy_active_selection(app);
            }
        }
        MouseEventKind::Down(MouseButton::Right) => {
            open_context_menu(app, mouse);
        }
        _ => {}
    }

    Vec::new()
}

/// Resolve a right-click in the sidebar to the hovered row's full copyable
/// text: the row's untruncated text plus its hover detail when present.
fn sidebar_row_copy_text(app: &App, mouse: MouseEvent) -> Option<String> {
    for section in &app.sidebar_hover.sections {
        if !mouse_hits_rect(mouse, Some(section.content_area)) {
            continue;
        }
        if let Some(row) = section.rows.iter().find(|row| row.row_y == mouse.row) {
            let mut text = row.full_text.clone();
            if let Some(detail) = row.detail.as_deref()
                && !detail.trim().is_empty()
            {
                text.push('\n');
                text.push_str(detail);
            }
            return Some(text).filter(|text| !text.trim().is_empty());
        }
        let line_idx = (mouse.row.saturating_sub(section.content_area.y)) as usize;
        if let Some(full) = section.lines.get(line_idx) {
            return Some(full.clone()).filter(|text| !text.trim().is_empty());
        }
    }
    None
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

/// Resolve a left-click in the sidebar to a typed row action, if the clicked
/// row has a click action assigned (#3028, #4009).
fn sidebar_click_action(app: &App, mouse: MouseEvent) -> Option<SidebarRowAction> {
    for section in &app.sidebar_hover.sections {
        if mouse.column >= section.content_area.x
            && mouse.column
                < section
                    .content_area
                    .x
                    .saturating_add(section.content_area.width)
            && mouse.row >= section.content_area.y
            && mouse.row
                < section
                    .content_area
                    .y
                    .saturating_add(section.content_area.height)
            && let Some(row) = section.rows.iter().find(|row| row.row_y == mouse.row)
        {
            if let (Some(action), Some(start), Some(end)) = (
                row.stop_action.as_ref(),
                row.stop_zone_start_col,
                row.stop_zone_end_col,
            ) && mouse.column >= start
                && mouse.column < end
            {
                return Some(action.clone());
            }
            return row.click_action.clone();
        }
    }
    None
}

pub(crate) fn mouse_hits_transcript_scrollbar(app: &App, mouse: MouseEvent) -> bool {
    let Some(area) = app.viewport.last_transcript_area else {
        return false;
    };
    if area.width <= 1 || app.viewport.last_transcript_total <= app.viewport.last_transcript_visible
    {
        return false;
    }

    let scrollbar_col = area.x.saturating_add(area.width.saturating_sub(1));
    mouse.column == scrollbar_col
        && mouse.row >= area.y
        && mouse.row < area.y.saturating_add(area.height)
}

pub(crate) fn scroll_transcript_to_mouse_row(app: &mut App, row: u16) -> bool {
    let Some(area) = app.viewport.last_transcript_area else {
        return false;
    };
    let total = app.viewport.last_transcript_total;
    let visible = app.viewport.last_transcript_visible;
    if area.height == 0 || total <= visible {
        return false;
    }

    let max_start = total.saturating_sub(visible);
    if max_start == 0 {
        app.scroll_to_bottom();
        return true;
    }

    let max_row = usize::from(area.height.saturating_sub(1));
    let relative_row = usize::from(row.saturating_sub(area.y)).min(max_row);
    let numerator = relative_row
        .saturating_mul(max_start)
        .saturating_add(max_row / 2);
    // Round to the nearest transcript offset so short thumbs still feel
    // responsive on compact terminals.
    let top = numerator.checked_div(max_row).unwrap_or(0);

    app.viewport.transcript_scroll = if top >= max_start {
        TranscriptScroll::to_bottom()
    } else {
        TranscriptScroll::at_line(top)
    };
    app.viewport.pending_scroll_delta = 0;
    app.user_scrolled_during_stream = !app.viewport.transcript_scroll.is_at_tail();
    app.needs_redraw = true;
    true
}

/// Cadence between auto-scroll ticks while drag-selecting past the
/// transcript edge (#1163). 30 ms ≈ 33 lines/sec, comparable to the feel
/// of a steady scroll-wheel drag.
const SELECTION_AUTOSCROLL_INTERVAL: Duration = Duration::from_millis(30);

/// Update the transcript selection while the left button is dragging.
/// When the mouse leaves the transcript rect vertically, arm
/// `selection_autoscroll` so the main loop can advance the viewport on a
/// fixed cadence; when the mouse returns inside, disarm it.
pub(crate) fn update_selection_drag(app: &mut App, mouse: MouseEvent) {
    if let Some(point) = selection_point_from_mouse(app, mouse) {
        app.viewport.transcript_selection.head = Some(point);
        app.viewport.selection_autoscroll = None;
        return;
    }

    let Some(area) = app.viewport.last_transcript_area else {
        return;
    };
    if area.height == 0 || area.width == 0 {
        return;
    }

    let direction = if mouse.row < area.y {
        -1
    } else if mouse.row >= area.y.saturating_add(area.height) {
        1
    } else {
        // Outside horizontally only — leave selection head where it is.
        return;
    };

    let max_col = area.x.saturating_add(area.width.saturating_sub(1));
    let column = mouse.column.clamp(area.x, max_col);

    // Fire on the next tick immediately by setting `next_tick` to now.
    app.viewport.selection_autoscroll = Some(SelectionAutoscroll {
        direction,
        column,
        next_tick: Instant::now(),
    });
    app.needs_redraw = true;
}

/// Advance the drag-edge auto-scroll one step if its cadence has elapsed.
/// Called once per main-loop iteration.
pub(crate) fn tick_selection_autoscroll(app: &mut App) {
    let Some(state) = app.viewport.selection_autoscroll else {
        return;
    };

    if !app.viewport.transcript_selection.dragging {
        app.viewport.selection_autoscroll = None;
        return;
    }

    let Some(area) = app.viewport.last_transcript_area else {
        return;
    };
    if area.height == 0 {
        return;
    }

    let now = Instant::now();
    if now < state.next_tick {
        return;
    }

    app.viewport.pending_scroll_delta = app
        .viewport
        .pending_scroll_delta
        .saturating_add(state.direction);
    app.user_scrolled_during_stream = true;

    let edge_row = if state.direction < 0 {
        area.y
    } else {
        area.y.saturating_add(area.height.saturating_sub(1))
    };
    if let Some(point) = selection_point_from_position(
        area,
        state.column,
        edge_row,
        app.viewport.last_transcript_top,
        app.viewport.last_transcript_total,
        app.viewport.last_transcript_padding_top,
    ) {
        app.viewport.transcript_selection.head = Some(point);
    }

    app.viewport.selection_autoscroll = Some(SelectionAutoscroll {
        next_tick: now + SELECTION_AUTOSCROLL_INTERVAL,
        ..state
    });
    app.needs_redraw = true;
}

pub(crate) fn mouse_hits_rect(mouse: MouseEvent, area: Option<Rect>) -> bool {
    point_hits_rect(mouse.column, mouse.row, area)
}

fn point_hits_rect(column: u16, row: u16, area: Option<Rect>) -> bool {
    let Some(area) = area else {
        return false;
    };

    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

pub(crate) fn open_context_menu(app: &mut App, mouse: MouseEvent) {
    let entries = build_context_menu_entries(app, mouse);
    if entries.is_empty() {
        return;
    }
    let title = app.tr(MessageId::CtxMenuTitle).to_string();
    app.view_stack.push(ContextMenuView::new(
        entries,
        mouse.column,
        mouse.row,
        title,
    ));
    app.needs_redraw = true;
}

pub(crate) fn build_context_menu_entries(app: &App, mouse: MouseEvent) -> Vec<ContextMenuEntry> {
    let mut entries = Vec::new();
    let on_sidebar = mouse_hits_rect(mouse, app.viewport.last_sidebar_area);

    if on_sidebar {
        if let Some(command) = sidebar_click_action(app, mouse)
            .and_then(|action| action.as_command().map(str::to_string))
        {
            entries.push(ContextMenuEntry {
                label: "Run".to_string(),
                description: command.clone(),
                action: ContextMenuAction::ExecuteCommand { command },
            });
        }
        // Copy the hovered row's full text (sidebar rows can't be
        // mouse-selected, so the menu is the only copy path).
        if let Some(text) = sidebar_row_copy_text(app, mouse) {
            entries.push(ContextMenuEntry {
                label: "Copy".to_string(),
                description: truncate_line_to_width(first_line(&text), 28),
                action: ContextMenuAction::CopyText { text },
            });
        }
    } else {
        // Paste first — the most common action when right-clicking in the
        // composer or transcript after copying text from the output area.
        entries.push(ContextMenuEntry {
            label: app.tr(MessageId::CtxMenuPaste).to_string(),
            description: app.tr(MessageId::CtxMenuPasteDesc).to_string(),
            action: ContextMenuAction::Paste,
        });
    }

    if selection_has_content(app) {
        entries.push(ContextMenuEntry {
            label: app.tr(MessageId::CtxMenuCopySelection).to_string(),
            description: app.tr(MessageId::CtxMenuCopySelectionDesc).to_string(),
            action: ContextMenuAction::CopySelection,
        });
        entries.push(ContextMenuEntry {
            label: app.tr(MessageId::CtxMenuOpenSelection).to_string(),
            description: app.tr(MessageId::CtxMenuOpenSelectionDesc).to_string(),
            action: ContextMenuAction::OpenSelection,
        });
        entries.push(ContextMenuEntry {
            label: app.tr(MessageId::CtxMenuClearSelection).to_string(),
            description: String::new(),
            action: ContextMenuAction::ClearSelection,
        });
    }

    if !on_sidebar && let Some(filtered_cell_index) = transcript_cell_index_from_mouse(app, mouse) {
        let cell_index = app.original_cell_index_for_rendered(filtered_cell_index);

        let target = detail_target_label(app, cell_index)
            .map(|label| truncate_line_to_width(label.as_str(), 28))
            .unwrap_or_else(|| "message".to_string());
        entries.push(ContextMenuEntry {
            label: app.tr(MessageId::CtxMenuOpenDetails).to_string(),
            description: target,
            action: ContextMenuAction::OpenDetails { cell_index },
        });
        entries.push(ContextMenuEntry {
            label: app.tr(MessageId::CtxMenuCopyMessage).to_string(),
            description: app.tr(MessageId::CtxMenuCopyMessageDesc).to_string(),
            action: ContextMenuAction::CopyCell { cell_index },
        });
        entries.push(ContextMenuEntry {
            label: app.tr(MessageId::CtxMenuOpenInEditor).to_string(),
            description: app.tr(MessageId::CtxMenuOpenInEditorDesc).to_string(),
            action: ContextMenuAction::OpenFileAtLine { cell_index },
        });
        // Hide/show cell toggle.
        if app.collapsed_cells.contains(&cell_index) {
            entries.push(ContextMenuEntry {
                label: app.tr(MessageId::CtxMenuShowCell).to_string(),
                description: app.tr(MessageId::CtxMenuShowCellDesc).to_string(),
                action: ContextMenuAction::ShowCell { cell_index },
            });
        } else {
            entries.push(ContextMenuEntry {
                label: app.tr(MessageId::CtxMenuHideCell).to_string(),
                description: app.tr(MessageId::CtxMenuHideCellDesc).to_string(),
                action: ContextMenuAction::HideCell { cell_index },
            });
        }
    }

    // When cells are hidden, offer a way to show them all.
    if !app.collapsed_cells.is_empty() {
        let count = app.collapsed_cells.len();
        let label = app.tr(MessageId::CtxMenuShowHidden).to_string();
        entries.push(ContextMenuEntry {
            label: format!("{label} ({count})"),
            description: app.tr(MessageId::CtxMenuShowHiddenDesc).to_string(),
            action: ContextMenuAction::ShowAllHidden,
        });
    }

    entries.push(ContextMenuEntry {
        label: app.tr(MessageId::CtxMenuCmdPalette).to_string(),
        description: app.tr(MessageId::CtxMenuCmdPaletteDesc).to_string(),
        action: ContextMenuAction::OpenCommandPalette,
    });
    entries.push(ContextMenuEntry {
        label: app.tr(MessageId::CtxMenuContextInspector).to_string(),
        description: app.tr(MessageId::CtxMenuContextInspectorDesc).to_string(),
        action: ContextMenuAction::OpenContextInspector,
    });
    entries.push(ContextMenuEntry {
        label: app.tr(MessageId::CtxMenuHelp).to_string(),
        description: app.tr(MessageId::CtxMenuHelpDesc).to_string(),
        action: ContextMenuAction::OpenHelp,
    });

    entries
}

pub(crate) fn transcript_cell_index_from_mouse(app: &App, mouse: MouseEvent) -> Option<usize> {
    let point = selection_point_from_mouse(app, mouse)?;
    app.viewport
        .transcript_cache
        .line_meta()
        .get(point.line_index)
        .and_then(|meta| meta.cell_line())
        .map(|(cell_index, _)| cell_index)
}

pub(crate) fn handle_context_menu_action(app: &mut App, action: ContextMenuAction) {
    match action {
        ContextMenuAction::CopySelection => {
            copy_active_selection(app);
        }
        ContextMenuAction::OpenSelection => {
            if !open_pager_for_selection(app) {
                app.status_message = Some("No selection to open".to_string());
            }
        }
        ContextMenuAction::ClearSelection => {
            app.viewport.transcript_selection.clear();
            app.status_message = Some("Selection cleared".to_string());
        }
        ContextMenuAction::CopyCell { cell_index } => {
            copy_cell_to_clipboard(app, cell_index);
        }
        ContextMenuAction::OpenDetails { cell_index } => {
            if !open_details_pager_for_cell(app, cell_index) {
                app.status_message = Some("No details available for that line".to_string());
            }
        }
        ContextMenuAction::Paste => {
            app.paste_from_clipboard();
        }
        ContextMenuAction::ExecuteCommand { command } => {
            app.input = command;
            app.status_message = Some("Command staged in composer".to_string());
            app.needs_redraw = true;
        }
        ContextMenuAction::CopyText { text } => {
            if app.clipboard.write_text(&text).is_ok() {
                app.status_message = Some("Copied".to_string());
            } else {
                app.status_message = Some("Copy failed".to_string());
            }
        }
        ContextMenuAction::OpenCommandPalette => {
            app.view_stack.push(CommandPaletteView::new_for_locale(
                app.ui_locale,
                build_command_palette_entries(
                    app.ui_locale,
                    &app.skills_dir,
                    app.skills_scan_codewhale_only,
                    &app.workspace,
                    &app.mcp_config_path,
                    app.mcp_snapshot.as_ref(),
                ),
            ));
        }
        ContextMenuAction::OpenContextInspector => {
            open_context_inspector(app);
        }
        ContextMenuAction::OpenHelp => {
            app.view_stack.push(HelpView::new_for_locale(app.ui_locale));
        }
        ContextMenuAction::OpenFileAtLine { cell_index } => {
            let width = app
                .viewport
                .last_transcript_area
                .map(|area| area.width)
                .unwrap_or(80);
            let text = history_cell_to_text(
                app.cell_at_virtual_index(cell_index)
                    .unwrap_or(&HistoryCell::System {
                        content: String::new(),
                    }),
                width,
            );
            if crate::tui::history::try_open_file_at_line(&text, &app.workspace) {
                app.status_message = Some("Opened file in editor".to_string());
            } else {
                app.status_message = Some("No file:line pattern found in selection".to_string());
            }
        }
        ContextMenuAction::HideCell { cell_index } => {
            app.collapsed_cells.insert(cell_index);
            app.status_message = Some("Cell hidden".to_string());
        }
        ContextMenuAction::ShowCell { cell_index } => {
            app.collapsed_cells.remove(&cell_index);
            app.status_message = Some("Cell shown".to_string());
        }
        ContextMenuAction::ShowAllHidden => {
            let count = app.collapsed_cells.len();
            app.collapsed_cells.clear();
            app.status_message = Some(format!("{count} hidden cell(s) restored"));
        }
    }
    app.needs_redraw = true;
}

pub(crate) fn selection_point_from_mouse(
    app: &App,
    mouse: MouseEvent,
) -> Option<TranscriptSelectionPoint> {
    selection_point_from_position(
        app.viewport.last_transcript_area?,
        mouse.column,
        mouse.row,
        app.viewport.last_transcript_top,
        app.viewport.last_transcript_total,
        app.viewport.last_transcript_padding_top,
    )
}

pub(crate) fn selection_point_from_position(
    area: Rect,
    column: u16,
    row: u16,
    transcript_top: usize,
    transcript_total: usize,
    padding_top: usize,
) -> Option<TranscriptSelectionPoint> {
    if column < area.x
        || column >= area.x + area.width
        || row < area.y
        || row >= area.y + area.height
    {
        return None;
    }

    if transcript_total == 0 {
        return None;
    }

    let row = row.saturating_sub(area.y) as usize;
    if row < padding_top {
        return None;
    }
    let row = row.saturating_sub(padding_top);

    let col = column.saturating_sub(area.x) as usize;
    let line_index = transcript_top
        .saturating_add(row)
        .min(transcript_total.saturating_sub(1));

    Some(TranscriptSelectionPoint {
        line_index,
        column: col,
    })
}

pub(crate) fn selection_has_content(app: &App) -> bool {
    // Composer selection takes priority (same as Cmd+C handler above).
    if !app.selected_text().is_empty() {
        return true;
    }
    selection_to_text(app).is_some_and(|text| !text.is_empty())
}

/// Branches taken by the Ctrl+C key handler. The order encodes priority and is
/// the unit-tested contract for #1337 / #1367: a transcript selection always
/// wins (so users learn that Ctrl+C copies when there's something to copy);
/// otherwise an active turn is interrupted; otherwise the quit-arm flow runs.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CtrlCDisposition {
    CopySelection,
    CancelTurn,
    ConfirmExit,
    ArmExit,
}

pub(crate) fn ctrl_c_disposition(app: &App) -> CtrlCDisposition {
    if selection_has_content(app) {
        CtrlCDisposition::CopySelection
    } else if app.is_loading {
        CtrlCDisposition::CancelTurn
    } else if app.quit_is_armed() {
        CtrlCDisposition::ConfirmExit
    } else {
        CtrlCDisposition::ArmExit
    }
}

/// Normalize the raw Ctrl+C control byte to canonical `Ctrl+C`.
///
/// In PTY/raw-mode the terminal driver delivers Ctrl+C as the literal byte
/// `0x03` (the ETX control character). crossterm usually decodes that to
/// `Char('c') + CONTROL`, but some terminal / kitty-keyboard-protocol
/// combinations surface it as `Char('\u{3}')` instead, where it slips past the
/// `Char('c') + CONTROL` arm of the key handler and never reaches the
/// quit-arm flow (#4090). Rewriting every encoding of Ctrl+C to the canonical
/// form here keeps the double-press-to-exit behavior consistent across PTY,
/// raw-mode, and kitty-enhanced terminals.
pub(crate) fn normalize_raw_ctrl_c(key: &mut KeyEvent) {
    if matches!(key.code, KeyCode::Char('\u{3}')) {
        key.code = KeyCode::Char('c');
        key.modifiers.insert(KeyModifiers::CONTROL);
    }
}

pub(crate) fn copy_active_selection(app: &mut App) {
    // Composer selection takes priority.
    let sel = app.selected_text();
    if !sel.is_empty() {
        if app.clipboard.write_text(&sel).is_ok() {
            app.status_message = Some("Selection copied".to_string());
            app.clear_selection();
        } else {
            app.status_message = Some("Copy failed".to_string());
        }
        return;
    }
    if !app.viewport.transcript_selection.is_active() {
        return;
    }
    if let Some(text) = selection_to_text(app).filter(|text| !text.is_empty()) {
        if app.clipboard.write_text(&text).is_ok() {
            app.status_message = Some("Selection copied".to_string());
        } else {
            app.status_message = Some("Copy failed".to_string());
        }
    } else {
        app.viewport.transcript_selection.clear();
        app.status_message = Some("No selection to copy".to_string());
    }
}

pub(crate) fn selection_to_text(app: &App) -> Option<String> {
    let (start, end) = app.viewport.transcript_selection.ordered_endpoints()?;
    let lines = app.viewport.transcript_cache.lines();
    if lines.is_empty() {
        return None;
    }
    let end_index = end.line_index.min(lines.len().saturating_sub(1));
    let start_index = start.line_index.min(end_index);

    let line_meta = app.viewport.transcript_cache.line_meta();
    let mut selected = String::new();
    let mut separator_before = None;
    #[allow(clippy::needless_range_loop)]
    for line_index in start_index..=end_index {
        if let Some(separator) = separator_before {
            selected.push_str(separator);
        }
        // Rail-prefix decorations are stored as cache metadata rather than
        // detected from glyphs, so new decoration types are covered without
        // changes to the copy path (#1163).
        let rail_width = app.viewport.transcript_cache.rail_prefix_width(line_index);
        // Convert the rendered line to plain text (strips OSC-8), then
        // slice off the rail prefix so subsequent column offsets operate
        // on content-only text.
        let full_text = line_to_plain(&lines[line_index]);
        let line_after_rail = if rail_width > 0 {
            slice_text(&full_text, rail_width, text_display_width(&full_text))
        } else {
            full_text
        };
        let line_after_rail_width = text_display_width(&line_after_rail);
        let copy_prefix_width = line_meta
            .get(line_index)
            .map(|meta| meta.copy_prefix_width())
            .unwrap_or(0)
            .min(line_after_rail_width);
        let line_text = if copy_prefix_width > 0 {
            slice_text(&line_after_rail, copy_prefix_width, line_after_rail_width)
        } else {
            line_after_rail
        };
        let line_width = text_display_width(&line_text);
        let visual_prefix_width = rail_width.saturating_add(copy_prefix_width);
        // Selection coordinates are recorded in rendered-column space, which
        // includes visual prefixes. Add them back so the column window maps
        // correctly into copy-only text.
        let (raw_col_start, raw_col_end) = if start_index == end_index {
            (start.column, end.column)
        } else if line_index == start_index {
            (start.column, line_width.saturating_add(visual_prefix_width))
        } else if line_index == end_index {
            (0, end.column)
        } else {
            (0, line_width.saturating_add(visual_prefix_width))
        };

        let col_start = raw_col_start
            .saturating_sub(visual_prefix_width)
            .min(line_width);
        let col_end = raw_col_end
            .saturating_sub(visual_prefix_width)
            .min(line_width);

        let slice = slice_text(&line_text, col_start, col_end);
        selected.push_str(&slice);
        separator_before = line_meta
            .get(line_index)
            .map(|meta| meta.copy_separator_after().as_str())
            .or(Some("\n"));
    }
    Some(selected)
}

#[cfg(test)]
mod tests {
    use super::{build_context_menu_entries, sidebar_click_action};
    use crate::config::Config;
    use crate::tui::app::{
        App, PendingInputAction, PendingInputActionRegion, SidebarHoverRow, SidebarHoverSection,
        SidebarRowAction, TuiOptions,
    };
    use crate::tui::views::{CommandPaletteAction, ContextMenuAction, ViewEvent};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;
    use std::path::PathBuf;

    fn create_test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
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
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
    }

    fn hover_row(row_y: u16, action: Option<&str>) -> SidebarHoverRow {
        SidebarHoverRow {
            row_y,
            display_text: "row".to_string(),
            full_text: "row".to_string(),
            detail: None,
            is_truncated: false,
            click_action: action.map(|action| SidebarRowAction::Command(action.to_string())),
            stop_action: None,
            stop_zone_start_col: None,
            stop_zone_end_col: None,
        }
    }

    fn hover_row_with_stop(row_y: u16, action: &str, stop_action: &str) -> SidebarHoverRow {
        SidebarHoverRow {
            row_y,
            display_text: "job row [x]".to_string(),
            full_text: "job row [x]".to_string(),
            detail: None,
            is_truncated: false,
            click_action: Some(SidebarRowAction::Command(action.to_string())),
            stop_action: Some(SidebarRowAction::Command(stop_action.to_string())),
            stop_zone_start_col: Some(68),
            stop_zone_end_col: Some(71),
        }
    }

    fn action_command(action: Option<SidebarRowAction>) -> Option<String> {
        action
            .as_ref()
            .and_then(SidebarRowAction::as_command)
            .map(str::to_string)
    }

    fn event_command(events: Vec<ViewEvent>) -> Option<String> {
        events.into_iter().find_map(|event| match event {
            ViewEvent::CommandPaletteSelected {
                action: CommandPaletteAction::ExecuteCommand { command },
            } => Some(command),
            _ => None,
        })
    }

    fn event_insert_text(events: Vec<ViewEvent>) -> Option<String> {
        events.into_iter().find_map(|event| match event {
            ViewEvent::CommandPaletteSelected {
                action: CommandPaletteAction::InsertText { text },
            } => Some(text),
            _ => None,
        })
    }

    fn left_click(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn right_click(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn pending_input_send_chip_click_emits_queue_send_command() {
        let mut app = create_test_app();
        app.viewport.last_pending_input_area = Some(Rect::new(0, 10, 80, 3));
        app.viewport
            .pending_input_action_regions
            .push(PendingInputActionRegion {
                action: PendingInputAction::SendQueued { index: 1 },
                rect: Rect::new(4, 11, 10, 1),
            });

        let events = super::handle_mouse_event(&mut app, left_click(5, 11));

        assert_eq!(event_command(events).as_deref(), Some("/queue send 2"));
        assert!(
            !app.viewport.transcript_selection.dragging,
            "pending-input chip click must not start transcript selection"
        );
    }

    #[test]
    fn pending_input_cancel_chip_click_emits_queue_drop_command() {
        let mut app = create_test_app();
        app.viewport.last_pending_input_area = Some(Rect::new(0, 10, 80, 3));
        app.viewport
            .pending_input_action_regions
            .push(PendingInputActionRegion {
                action: PendingInputAction::CancelQueued { index: 0 },
                rect: Rect::new(15, 11, 8, 1),
            });

        let events = super::handle_mouse_event(&mut app, left_click(16, 11));

        assert_eq!(event_command(events).as_deref(), Some("/queue drop 1"));
        assert_eq!(
            app.viewport.hovered_pending_input_action,
            Some(PendingInputAction::CancelQueued { index: 0 })
        );
    }

    #[test]
    fn sidebar_insert_text_row_click_prefills_composer() {
        let mut app = create_test_app();
        app.sidebar_hover.sections.push(SidebarHoverSection {
            content_area: Rect::new(60, 4, 20, 3),
            lines: vec!["todo row".to_string()],
            rows: vec![SidebarHoverRow {
                row_y: 5,
                display_text: "todo row".to_string(),
                full_text: "todo row".to_string(),
                detail: None,
                is_truncated: false,
                click_action: Some(SidebarRowAction::InsertText(
                    "Update To-do #2 to completed: Wire the thing".to_string(),
                )),
                stop_action: None,
                stop_zone_start_col: None,
                stop_zone_end_col: None,
            }],
        });

        let events = super::handle_mouse_event(&mut app, left_click(62, 5));

        assert_eq!(
            event_insert_text(events).as_deref(),
            Some("Update To-do #2 to completed: Wire the thing")
        );
    }

    #[test]
    fn context_menu_keeps_paste_first_outside_sidebar() {
        let mut app = create_test_app();
        app.viewport.last_sidebar_area = Some(Rect::new(60, 4, 20, 6));

        let entries = build_context_menu_entries(&app, right_click(10, 4));

        assert!(matches!(
            entries.first().map(|entry| &entry.action),
            Some(ContextMenuAction::Paste)
        ));
    }

    #[test]
    fn sidebar_context_menu_omits_paste_without_row_action() {
        let mut app = create_test_app();
        app.viewport.last_sidebar_area = Some(Rect::new(60, 4, 20, 6));
        app.sidebar_hover.sections.push(SidebarHoverSection {
            content_area: Rect::new(60, 4, 20, 6),
            lines: vec!["header".to_string()],
            rows: vec![hover_row(4, None)],
        });

        let entries = build_context_menu_entries(&app, right_click(65, 4));

        assert!(
            !entries
                .iter()
                .any(|entry| matches!(entry.action, ContextMenuAction::Paste)),
            "sidebar menu should not offer paste: {entries:?}"
        );
    }

    #[test]
    fn sidebar_context_menu_runs_clickable_row_action() {
        let mut app = create_test_app();
        app.viewport.last_sidebar_area = Some(Rect::new(60, 4, 20, 6));
        app.sidebar_hover.sections.push(SidebarHoverSection {
            content_area: Rect::new(60, 4, 20, 6),
            lines: vec!["job row".to_string()],
            rows: vec![hover_row(4, Some("/jobs show shell_x"))],
        });

        let entries = build_context_menu_entries(&app, right_click(65, 4));

        let first = entries.first().expect("sidebar row should have menu");
        assert_eq!(first.label, "Run");
        assert_eq!(first.description, "/jobs show shell_x");
        assert!(matches!(
            &first.action,
            ContextMenuAction::ExecuteCommand { command } if command == "/jobs show shell_x"
        ));
        assert!(
            !entries
                .iter()
                .any(|entry| matches!(entry.action, ContextMenuAction::Paste)),
            "clickable sidebar menu should not offer paste: {entries:?}"
        );
    }

    #[test]
    fn sidebar_click_resolves_row_actions_inside_section() {
        let mut app = create_test_app();
        app.sidebar_hover.sections.push(SidebarHoverSection {
            content_area: Rect::new(60, 4, 20, 6),
            lines: vec![
                "header".to_string(),
                "job row".to_string(),
                "job detail".to_string(),
                "agent row".to_string(),
            ],
            rows: vec![
                hover_row(4, None),
                hover_row(5, Some("/jobs show shell_x")),
                hover_row(6, Some("/jobs cancel shell_x")),
                SidebarHoverRow {
                    row_y: 7,
                    display_text: "agent row".to_string(),
                    full_text: "agent row".to_string(),
                    detail: None,
                    is_truncated: false,
                    click_action: Some(SidebarRowAction::ToggleAgentDetails {
                        agent_id: "agent_123".to_string(),
                    }),
                    stop_action: None,
                    stop_zone_start_col: None,
                    stop_zone_end_col: None,
                },
            ],
        });

        assert_eq!(
            action_command(sidebar_click_action(&app, left_click(65, 5))).as_deref(),
            Some("/jobs show shell_x"),
            "job label row resolves to its show action"
        );
        assert_eq!(
            action_command(sidebar_click_action(&app, left_click(79, 6))).as_deref(),
            Some("/jobs cancel shell_x"),
            "job detail row resolves to its cancel action"
        );
        assert!(matches!(
            sidebar_click_action(&app, left_click(60, 7)),
            Some(SidebarRowAction::ToggleAgentDetails { agent_id })
                if agent_id == "agent_123"
        ));
        assert_eq!(
            sidebar_click_action(&app, left_click(65, 4)),
            None,
            "header row has no action"
        );
    }

    #[test]
    fn sidebar_click_routes_inline_stop_zone_before_row_action() {
        let mut app = create_test_app();
        app.viewport.last_sidebar_area = Some(Rect::new(60, 4, 20, 4));
        app.sidebar_hover.sections.push(SidebarHoverSection {
            content_area: Rect::new(60, 4, 20, 4),
            lines: vec!["job row [x]".to_string()],
            rows: vec![hover_row_with_stop(
                4,
                "/jobs show shell_x",
                "/jobs cancel shell_x",
            )],
        });

        assert_eq!(
            action_command(sidebar_click_action(&app, left_click(62, 4))).as_deref(),
            Some("/jobs show shell_x"),
            "clicking the label opens the job"
        );
        assert_eq!(
            action_command(sidebar_click_action(&app, left_click(69, 4))).as_deref(),
            Some("/jobs cancel shell_x"),
            "clicking [x] cancels the job"
        );
    }

    #[test]
    fn sidebar_click_routes_agent_inline_stop_zone_before_peek_action() {
        let mut app = create_test_app();
        app.viewport.last_sidebar_area = Some(Rect::new(60, 4, 24, 4));
        app.sidebar_hover.sections.push(SidebarHoverSection {
            content_area: Rect::new(60, 4, 24, 4),
            lines: vec!["[~] worker Agent 1 [x]".to_string()],
            rows: vec![SidebarHoverRow {
                row_y: 4,
                display_text: "[~] Agent 1 is working [x]".to_string(),
                full_text: "[~] Agent 1 is working [x]".to_string(),
                detail: None,
                is_truncated: false,
                click_action: Some(SidebarRowAction::ToggleAgentDetails {
                    agent_id: "agent_123".to_string(),
                }),
                stop_action: Some(SidebarRowAction::CancelAgent {
                    agent_id: "agent_123".to_string(),
                }),
                stop_zone_start_col: Some(68),
                stop_zone_end_col: Some(71),
            }],
        });

        assert!(matches!(
            sidebar_click_action(&app, left_click(62, 4)),
            Some(SidebarRowAction::ToggleAgentDetails { agent_id })
                if agent_id == "agent_123"
        ));
        assert!(matches!(
            sidebar_click_action(&app, left_click(69, 4)),
            Some(SidebarRowAction::CancelAgent { agent_id }) if agent_id == "agent_123"
        ));
    }

    #[test]
    fn sidebar_context_menu_offers_copy_of_hovered_row() {
        let mut app = create_test_app();
        app.viewport.last_sidebar_area = Some(Rect::new(60, 4, 20, 6));
        app.sidebar_hover.sections.push(SidebarHoverSection {
            content_area: Rect::new(60, 4, 20, 6),
            lines: vec!["agent row".to_string()],
            rows: vec![SidebarHoverRow {
                row_y: 4,
                display_text: "[~] worker doc-che…".to_string(),
                full_text: "[~] worker doc-checker".to_string(),
                detail: Some("id: agent_123 · 2 step(s)".to_string()),
                is_truncated: true,
                click_action: None,
                stop_action: None,
                stop_zone_start_col: None,
                stop_zone_end_col: None,
            }],
        });

        let entries = build_context_menu_entries(&app, right_click(65, 4));

        let copy = entries
            .iter()
            .find(|entry| matches!(entry.action, ContextMenuAction::CopyText { .. }))
            .expect("sidebar row should offer Copy");
        assert_eq!(copy.label, "Copy");
        assert!(matches!(
            &copy.action,
            ContextMenuAction::CopyText { text }
                if text == "[~] worker doc-checker\nid: agent_123 · 2 step(s)"
        ));
    }

    #[test]
    fn sidebar_click_outside_section_resolves_to_none() {
        let mut app = create_test_app();
        app.sidebar_hover.sections.push(SidebarHoverSection {
            content_area: Rect::new(60, 4, 20, 6),
            lines: vec!["job row".to_string()],
            rows: vec![hover_row(4, Some("/jobs show shell_x"))],
        });

        // Left of the sidebar (transcript area).
        assert_eq!(sidebar_click_action(&app, left_click(10, 4)), None);
        // Below the section's content area.
        assert_eq!(sidebar_click_action(&app, left_click(65, 30)), None);
        // Inside the section but on an empty row without metadata.
        assert_eq!(sidebar_click_action(&app, left_click(65, 8)), None);
    }
}
