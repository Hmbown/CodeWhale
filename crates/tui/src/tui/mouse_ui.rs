use std::time::{Duration, Instant};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::tui::app::App;
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

pub(crate) fn should_drop_loading_mouse_motion(app: &App, mouse: MouseEvent) -> bool {
    if !app.is_loading {
        return false;
    }

    match mouse.kind {
        MouseEventKind::Moved => true,
        MouseEventKind::Drag(_) => {
            !app.viewport.transcript_selection.dragging
                && !app.viewport.transcript_scrollbar_dragging
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

    match mouse.kind {
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
    let Some(area) = area else {
        return false;
    };

    mouse.column >= area.x
        && mouse.column < area.x.saturating_add(area.width)
        && mouse.row >= area.y
        && mouse.row < area.y.saturating_add(area.height)
}

pub(crate) fn open_context_menu(app: &mut App, mouse: MouseEvent) {
    let entries = build_context_menu_entries(app, mouse);
    if entries.is_empty() {
        return;
    }
    app.view_stack
        .push(ContextMenuView::new(entries, mouse.column, mouse.row));
    app.needs_redraw = true;
}

pub(crate) fn build_context_menu_entries(app: &App, mouse: MouseEvent) -> Vec<ContextMenuEntry> {
    let mut entries = Vec::new();

    if selection_has_content(app) {
        entries.push(ContextMenuEntry {
            label: "Copy selection".to_string(),
            description: "write selected transcript text".to_string(),
            action: ContextMenuAction::CopySelection,
        });
        entries.push(ContextMenuEntry {
            label: "Open selection".to_string(),
            description: "show selected text in pager".to_string(),
            action: ContextMenuAction::OpenSelection,
        });
        entries.push(ContextMenuEntry {
            label: "Clear selection".to_string(),
            description: String::new(),
            action: ContextMenuAction::ClearSelection,
        });
    }

    if let Some(filtered_cell_index) = transcript_cell_index_from_mouse(app, mouse) {
        // Convert filtered index → original virtual index using the
        // mapping built in ChatWidget::new. When no cells are collapsed
        // this is an identity mapping.
        let cell_index = app
            .collapsed_cell_map
            .get(filtered_cell_index)
            .copied()
            .unwrap_or(filtered_cell_index);

        let target = detail_target_label(app, cell_index)
            .map(|label| truncate_line_to_width(label.as_str(), 28))
            .unwrap_or_else(|| "message".to_string());
        entries.push(ContextMenuEntry {
            label: "Open details".to_string(),
            description: target,
            action: ContextMenuAction::OpenDetails { cell_index },
        });
        entries.push(ContextMenuEntry {
            label: "Copy message".to_string(),
            description: "write clicked transcript cell".to_string(),
            action: ContextMenuAction::CopyCell { cell_index },
        });
        entries.push(ContextMenuEntry {
            label: "Open in editor".to_string(),
            description: "open file:line in $EDITOR".to_string(),
            action: ContextMenuAction::OpenFileAtLine { cell_index },
        });
        // Hide/show cell toggle.
        if app.collapsed_cells.contains(&cell_index) {
            entries.push(ContextMenuEntry {
                label: "Show cell".to_string(),
                description: "unhide this transcript cell".to_string(),
                action: ContextMenuAction::ShowCell { cell_index },
            });
        } else {
            entries.push(ContextMenuEntry {
                label: "Hide cell".to_string(),
                description: "collapse this transcript cell".to_string(),
                action: ContextMenuAction::HideCell { cell_index },
            });
        }
    }

    // When cells are hidden, offer a way to show them all.
    if !app.collapsed_cells.is_empty() {
        let count = app.collapsed_cells.len();
        entries.push(ContextMenuEntry {
            label: format!("Show hidden ({count})"),
            description: "unhide all collapsed cells".to_string(),
            action: ContextMenuAction::ShowAllHidden,
        });
    }

    entries.push(ContextMenuEntry {
        label: "Paste".to_string(),
        description: "insert clipboard into composer".to_string(),
        action: ContextMenuAction::Paste,
    });
    entries.push(ContextMenuEntry {
        label: "Command palette".to_string(),
        description: "commands, skills, and tools".to_string(),
        action: ContextMenuAction::OpenCommandPalette,
    });
    entries.push(ContextMenuEntry {
        label: "Context inspector".to_string(),
        description: "active context and cache hints".to_string(),
        action: ContextMenuAction::OpenContextInspector,
    });
    entries.push(ContextMenuEntry {
        label: "Help".to_string(),
        description: "keybindings and commands".to_string(),
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
        ContextMenuAction::OpenCommandPalette => {
            app.view_stack
                .push(CommandPaletteView::new(build_command_palette_entries(
                    app.ui_locale,
                    &app.skills_dir,
                    &app.workspace,
                    &app.mcp_config_path,
                    app.mcp_snapshot.as_ref(),
                )));
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

pub(crate) fn copy_active_selection(app: &mut App) {
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

    let mut selected_lines = Vec::new();
    #[allow(clippy::needless_range_loop)]
    for line_index in start_index..=end_index {
        // Rail-prefix decorations are stored as cache metadata rather than
        // detected from glyphs, so new decoration types are covered without
        // changes to the copy path (#1163).
        let rail_width = app.viewport.transcript_cache.rail_prefix_width(line_index);
        // Convert the rendered line to plain text (strips OSC-8), then
        // slice off the rail prefix so subsequent column offsets operate
        // on content-only text.
        let full_text = line_to_plain(&lines[line_index]);
        let line_text = if rail_width > 0 {
            slice_text(&full_text, rail_width, text_display_width(&full_text))
        } else {
            full_text
        };
        let line_width = text_display_width(&line_text);
        // Selection coordinates are recorded in rendered-column space, which
        // includes the visual rail prefix. Add rail_width back so the column
        // window maps correctly into the rail-stripped text.
        let (raw_col_start, raw_col_end) = if start_index == end_index {
            (start.column, end.column)
        } else if line_index == start_index {
            (start.column, line_width.saturating_add(rail_width))
        } else if line_index == end_index {
            (0, end.column)
        } else {
            (0, line_width.saturating_add(rail_width))
        };

        let col_start = raw_col_start.saturating_sub(rail_width).min(line_width);
        let col_end = raw_col_end.saturating_sub(rail_width).min(line_width);

        let slice = slice_text(&line_text, col_start, col_end);
        selected_lines.push(slice);
    }
    Some(selected_lines.join("\n"))
}
