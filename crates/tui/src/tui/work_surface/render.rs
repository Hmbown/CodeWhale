use std::collections::HashMap;

use ratatui::{
    Frame,
    layout::Rect,
    prelude::Widget,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::localization::MessageId;
use crate::tui::app::{App, SidebarHoverRow, SidebarHoverSection};
use crate::tui::ui_text::truncate_line_to_width;

use super::model::{
    RailPanel, WorkHitbox, WorkRow, WorkSurfacePlacement, WorkTone, project_visible,
};

const SIDE_RAIL_MIN_HOST_WIDTH: u16 = 72;
const SIDE_RAIL_MIN_CHAT_WIDTH: u16 = 40;

fn effective_placement(configured: WorkSurfacePlacement, host_width: u16) -> WorkSurfacePlacement {
    if configured == WorkSurfacePlacement::Off {
        return WorkSurfacePlacement::Off;
    }
    if host_width < SIDE_RAIL_MIN_HOST_WIDTH {
        WorkSurfacePlacement::Top
    } else {
        configured
    }
}

/// Responsive work-surface height. The component owns a bounded window; long
/// work lists scroll instead of consuming the transcript.
pub fn height(app: &mut App, width: u16, terminal_height: u16) -> u16 {
    app.work_surface.effective_placement = effective_placement(app.work_surface.placement, width);
    // Off hides the rail outright: no strip, no side reservation, no stale
    // interaction state.
    if app.work_surface.effective_placement == WorkSurfacePlacement::Off {
        app.work_surface.last_area = None;
        app.work_surface.hitboxes.clear();
        return 0;
    }
    // Non-Tasks panels always own a strip once selected: the user asked for
    // the panel, so an empty panel collapses to a hint line, not a vanished
    // rail.
    if app.work_surface.panel != RailPanel::Tasks {
        if app.work_surface.effective_placement != WorkSurfacePlacement::Top {
            return 0;
        }
        let terminal_cap = terminal_height
            .saturating_div(2)
            .clamp(super::model::TOP_HEIGHT_MIN, super::model::TOP_HEIGHT_MAX);
        let cap = app.work_surface.top_height.min(terminal_cap);
        // Title row plus three content rows, bounded by the configured cap.
        return 4.clamp(super::model::TOP_HEIGHT_MIN, cap);
    }
    let rows = project_visible(app);
    if rows.is_empty() {
        app.work_surface.focused = false;
        app.work_surface.selected = None;
        app.work_surface.opened = None;
        app.work_surface.hovered = None;
        app.work_surface.last_area = None;
        app.work_surface.hitboxes.clear();
        app.work_surface.latest_rows.clear();
        app.work_surface.visible_rows = 0;
        app.work_surface.total_rows = 0;
        app.work_surface.scroll_offset = 0;
        app.work_surface.resizing = false;
        app.work_surface.divider_hovered = false;
        return 0;
    }
    if app.work_surface.effective_placement != WorkSurfacePlacement::Top {
        return 0;
    }
    // The strip auto-fits its content: the literal selectable list plus the
    // pinned progress receipt and the divider row. `top_height` (drag-resize
    // / settings) and half the terminal act as caps, so a two-step plan
    // takes two rows while an eight-step plan grows to show all eight —
    // never a fixed-height band of blank water.
    let terminal_cap = terminal_height
        .saturating_div(2)
        .clamp(super::model::TOP_HEIGHT_MIN, super::model::TOP_HEIGHT_MAX);
    let cap = app.work_surface.top_height.min(terminal_cap);
    let selectable = rows.iter().filter(|row| row.selectable).count();
    let progress = u16::from(top_todo_progress(app, &rows).is_some());
    let desired = u16::try_from(selectable)
        .unwrap_or(u16::MAX)
        .saturating_add(progress)
        .saturating_add(1);
    desired.clamp(super::model::TOP_HEIGHT_MIN, cap)
}

/// Split the transcript slot for a side rail. Top placement consumes its own
/// vertical row before this point, so it returns the chat area unchanged.
pub fn split_chat(app: &mut App, area: Rect) -> (Rect, Option<Rect>) {
    let placement = effective_placement(app.work_surface.placement, area.width);
    app.work_surface.effective_placement = placement;
    if placement == WorkSurfacePlacement::Top
        || placement == WorkSurfacePlacement::Off
        || (app.work_surface.panel == RailPanel::Tasks && app.work_surface.latest_rows.is_empty())
    {
        return (area, None);
    }

    let rail_width = app
        .work_surface
        .side_width
        .clamp(super::model::SIDE_WIDTH_MIN, super::model::SIDE_WIDTH_MAX)
        .min(area.width.saturating_sub(SIDE_RAIL_MIN_CHAT_WIDTH));
    if rail_width < super::model::SIDE_WIDTH_MIN {
        app.work_surface.effective_placement = WorkSurfacePlacement::Top;
        return (area, None);
    }

    let chat_width = area.width.saturating_sub(rail_width);
    match placement {
        WorkSurfacePlacement::Left => (
            Rect {
                x: area.x.saturating_add(rail_width),
                width: chat_width,
                ..area
            },
            Some(Rect {
                width: rail_width,
                ..area
            }),
        ),
        WorkSurfacePlacement::Right => (
            Rect {
                width: chat_width,
                ..area
            },
            Some(Rect {
                x: area.x.saturating_add(chat_width),
                width: rail_width,
                ..area
            }),
        ),
        WorkSurfacePlacement::Top | WorkSurfacePlacement::Off => (area, None),
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        app.work_surface.last_area = None;
        return;
    }

    if let Some(previous) = app.work_surface.last_area {
        app.sidebar_hover
            .sections
            .retain(|section| section.content_area != previous);
    }

    let placement = app.work_surface.effective_placement;
    // Off renders no rail; height()/split_chat() never hand us an area for it.
    if placement == WorkSurfacePlacement::Off {
        app.work_surface.last_area = None;
        return;
    }
    let body_area = match placement {
        WorkSurfacePlacement::Top => Rect {
            height: area.height.saturating_sub(1),
            ..area
        },
        WorkSurfacePlacement::Left => Rect {
            width: area.width.saturating_sub(1),
            ..area
        },
        WorkSurfacePlacement::Right => Rect {
            x: area.x.saturating_add(1),
            width: area.width.saturating_sub(1),
            ..area
        },
        WorkSurfacePlacement::Off => unreachable!("off placement returned above"),
    };

    // Non-Tasks panels render as a titled line list and skip the row
    // machinery (hitboxes, selection, todo ordinals) entirely.
    if app.work_surface.panel != RailPanel::Tasks {
        render_panel(frame, area, body_area, app);
        return;
    }

    let mut rows = project_visible(app);
    if placement == WorkSurfacePlacement::Top {
        // The top bar is the literal list: to-dos first, then sub-agents.
        // Section summaries belong to the optional side/detail surface and
        // must not spend scarce transcript rows on generic chrome.
        rows.retain(|row| row.selectable);
    }
    let todo_ordinals = if placement == WorkSurfacePlacement::Top {
        todo_ordinals(&rows)
    } else {
        HashMap::new()
    };
    let ordinal_width = todo_ordinals.len().max(1).to_string().len();
    let todo_progress = (placement == WorkSurfacePlacement::Top)
        .then(|| top_todo_progress(app, &rows))
        .flatten();
    // At the minimum two-row surface, preserve the one usable content row and
    // the divider. Taller surfaces pin the authoritative progress receipt
    // above the scrollable/selectable rows.
    let progress_height = u16::from(todo_progress.is_some() && body_area.height >= 2);
    let list_height = body_area.height.saturating_sub(progress_height);
    let body_height = usize::from(list_height);
    let overflow = rows.len() > body_height;
    let inset = u16::from(body_area.width >= 60);
    let rail_width = u16::from(overflow);
    let content_area = Rect {
        x: body_area.x.saturating_add(inset),
        y: body_area.y.saturating_add(progress_height),
        width: body_area
            .width
            .saturating_sub(inset.saturating_mul(2))
            .saturating_sub(rail_width),
        height: list_height,
    };

    app.work_surface.visible_rows = body_height;
    app.work_surface.total_rows = rows.len();
    // A redraw may clamp an obsolete offset, but it must not reveal the
    // remembered keyboard selection: doing so undoes mouse-wheel scrolling
    // whenever that selection is above the viewport (#4594).
    app.work_surface.clamp_viewport(&rows);
    let max_offset = rows.len().saturating_sub(body_height.max(1));
    app.work_surface.scroll_offset = app.work_surface.scroll_offset.min(max_offset);

    Block::default()
        .style(Style::default().bg(app.ui_theme.surface_bg))
        .render(area, frame.buffer_mut());

    if let Some(progress) = todo_progress.filter(|_| progress_height > 0) {
        let progress = truncate_line_to_width(&progress, usize::from(content_area.width));
        Paragraph::new(Line::from(Span::styled(
            progress,
            Style::default()
                .fg(app.ui_theme.accent_primary)
                .bg(app.ui_theme.surface_bg)
                .add_modifier(Modifier::BOLD),
        )))
        .render(
            Rect {
                y: body_area.y,
                height: 1,
                ..content_area
            },
            frame.buffer_mut(),
        );
    }

    let start = app.work_surface.scroll_offset;
    let visible = rows
        .iter()
        .skip(start)
        .take(body_height)
        .collect::<Vec<_>>();
    let mut lines = Vec::with_capacity(visible.len());
    let mut hover_rows = Vec::new();
    let mut hitboxes = Vec::new();
    for (visible_index, row) in visible.iter().enumerate() {
        let row_y = content_area.y.saturating_add(visible_index as u16);
        let selected =
            app.work_surface.focused && app.work_surface.selected.as_ref() == Some(&row.id);
        let hovered = app.work_surface.hovered.as_ref() == Some(&row.id);
        let opened = app.work_surface.opened.as_ref() == Some(&row.id);
        let style = row_style(app, row, selected, hovered, opened);
        let compact_owner = if placement == WorkSurfacePlacement::Top {
            todo_ordinals
                .get(&row.id.0)
                .map(|ordinal| format!("{ordinal:>ordinal_width$} · "))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let mark = if opened && row.selectable {
            "▾"
        } else {
            row.mark
        };
        let prefix = if row.tone == WorkTone::Heading {
            format!("{} ", mark)
        } else {
            format!("{compact_owner}{mark} ")
        };
        let detail_candidate = if row.tone != WorkTone::Heading && content_area.width >= 44 {
            format!("  {}", row.detail)
        } else {
            String::new()
        };
        let prefix_width = UnicodeWidthStr::width(prefix.as_str());
        let row_width = usize::from(content_area.width);
        let label_budget = row_width.saturating_sub(prefix_width).max(1);
        let label = truncate_line_to_width(&row.label, label_budget);
        let detail_budget =
            row_width.saturating_sub(prefix_width + UnicodeWidthStr::width(label.as_str()));
        let detail = if detail_budget >= 4 {
            truncate_line_to_width(&detail_candidate, detail_budget)
        } else {
            String::new()
        };
        let detail_width = UnicodeWidthStr::width(detail.as_str());
        let gap = usize::from(content_area.width)
            .saturating_sub(prefix_width + UnicodeWidthStr::width(label.as_str()) + detail_width);
        let display = format!("{prefix}{label}{}{detail}", " ".repeat(gap));
        lines.push(Line::from(Span::styled(display.clone(), style)));

        hitboxes.push(WorkHitbox {
            id: row.id.clone(),
            row_y,
        });

        if row.selectable {
            hover_rows.push(SidebarHoverRow {
                row_y,
                display_text: display,
                full_text: format!("{} · {}", row.label, row.detail),
                detail: Some(row.detail.clone()),
                is_truncated: label != row.label || detail != detail_candidate,
                click_action: row.primary_action.clone(),
                stop_action: None,
                stop_zone_start_col: None,
                stop_zone_end_col: None,
            });
        }
    }

    Paragraph::new(lines).render(content_area, frame.buffer_mut());
    render_divider(frame, area, placement, app);
    if overflow {
        render_scrollbar(
            frame,
            Rect {
                x: body_area.right().saturating_sub(1),
                y: content_area.y,
                width: 1,
                height: content_area.height,
            },
            app.work_surface.scroll_offset,
            body_height,
            rows.len(),
            app,
        );
    }

    app.work_surface.last_area = Some(area);
    app.work_surface.hitboxes = hitboxes;
    app.sidebar_hover.sections.push(SidebarHoverSection {
        content_area,
        lines: visible.iter().map(|row| row.label.clone()).collect(),
        rows: hover_rows,
    });
}

/// Render a non-Tasks rail panel (Agents / Context / Pinned) as a titled
/// line list in the same body area and with the same divider and scrollbar
/// the Tasks list would use. Row interactivity (hitboxes, selection,
/// click actions) is Tasks-only for now; panels scroll via the shared
/// `scroll_offset`.
fn render_panel(frame: &mut Frame, area: Rect, body_area: Rect, app: &mut App) {
    let panel = app.work_surface.panel;
    let placement = app.work_surface.effective_placement;

    Block::default()
        .style(Style::default().bg(app.ui_theme.surface_bg))
        .render(area, frame.buffer_mut());

    // Title row.
    Paragraph::new(Line::from(Span::styled(
        truncate_line_to_width(panel.title(), usize::from(body_area.width).max(1)),
        Style::default()
            .fg(app.ui_theme.accent_primary)
            .bg(app.ui_theme.surface_bg)
            .add_modifier(Modifier::BOLD),
    )))
    .render(
        Rect {
            height: 1,
            ..body_area
        },
        frame.buffer_mut(),
    );

    let content_area = Rect {
        y: body_area.y.saturating_add(1),
        height: body_area.height.saturating_sub(1),
        ..body_area
    };
    let body_height = usize::from(content_area.height);
    let lines = super::panels::panel_lines(
        app,
        panel,
        usize::from(content_area.width),
        body_height.max(1),
    )
    .unwrap_or_default();

    let max_offset = lines.len().saturating_sub(body_height.max(1));
    app.work_surface.scroll_offset = app.work_surface.scroll_offset.min(max_offset);
    let overflow = lines.len() > body_height;
    let visible: Vec<Line> = lines
        .iter()
        .skip(app.work_surface.scroll_offset)
        .take(body_height)
        .cloned()
        .collect();
    Paragraph::new(visible).render(content_area, frame.buffer_mut());

    render_divider(frame, area, placement, app);
    if overflow {
        render_scrollbar(
            frame,
            Rect {
                x: body_area.right().saturating_sub(1),
                y: content_area.y,
                width: 1,
                height: content_area.height,
            },
            app.work_surface.scroll_offset,
            body_height,
            lines.len(),
            app,
        );
    }

    app.work_surface.last_area = Some(area);
    app.work_surface.visible_rows = body_height;
    app.work_surface.total_rows = lines.len();
    app.work_surface.hitboxes.clear();
    app.work_surface.selected = None;
    app.work_surface.opened = None;
    app.work_surface.hovered = None;
}

fn todo_ordinals(rows: &[WorkRow]) -> HashMap<String, usize> {
    rows.iter()
        .filter(|row| row.id.0.starts_with("graph:"))
        .enumerate()
        .map(|(index, row)| (row.id.0.clone(), index.saturating_add(1)))
        .collect()
}

fn top_todo_progress(app: &App, rows: &[WorkRow]) -> Option<String> {
    let todos = rows
        .iter()
        .filter(|row| row.id.0.starts_with("graph:"))
        .collect::<Vec<_>>();
    let total = todos.len();
    if total == 0 {
        return None;
    }
    let completed = todos
        .iter()
        .filter(|row| row.tone == WorkTone::Success)
        .count();
    let remaining = total.saturating_sub(completed);
    let label = format!("{} ·", app.tr(MessageId::SidebarTodoLabel));
    Some(
        app.tr(MessageId::WorkSurfaceTodoProgress)
            .replace("{label}", &label)
            .replace("{completed}", &completed.to_string())
            .replace("{total}", &total.to_string())
            .replace("{remaining}", &remaining.to_string()),
    )
}

fn row_style(app: &App, row: &WorkRow, selected: bool, hovered: bool, opened: bool) -> Style {
    let fg = match row.tone {
        WorkTone::Heading => app.ui_theme.accent_primary,
        WorkTone::Live => app.ui_theme.status_working,
        WorkTone::Attention => app.ui_theme.error_fg,
        WorkTone::Success => app.ui_theme.success,
        WorkTone::Muted => app.ui_theme.text_muted,
    };
    let mut style = Style::default().fg(fg).bg(app.ui_theme.surface_bg);
    if row.tone == WorkTone::Heading {
        style = style.add_modifier(Modifier::BOLD);
    }
    if !row.selectable {
        return style;
    }
    if opened {
        style = style
            .fg(app.ui_theme.accent_primary)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    }
    if selected {
        style = style
            .bg(app.ui_theme.selection_bg)
            .add_modifier(Modifier::BOLD);
    } else if hovered {
        style = style.bg(app.ui_theme.elevated_bg);
    }
    style
}

fn render_divider(frame: &mut Frame, area: Rect, placement: WorkSurfacePlacement, app: &App) {
    let active = app.work_surface.resizing || app.work_surface.divider_hovered;
    let color = if active {
        app.ui_theme.accent_primary
    } else {
        app.ui_theme.border
    };
    match placement {
        WorkSurfacePlacement::Off => {}
        WorkSurfacePlacement::Top => {
            let y = area.bottom().saturating_sub(1);
            for x in area.left()..area.right() {
                frame.buffer_mut()[(x, y)]
                    .set_symbol(if active { "━" } else { "─" })
                    .set_fg(color)
                    .set_bg(app.ui_theme.surface_bg);
            }
        }
        WorkSurfacePlacement::Left | WorkSurfacePlacement::Right => {
            let x = if placement == WorkSurfacePlacement::Left {
                area.right().saturating_sub(1)
            } else {
                area.left()
            };
            for y in area.top()..area.bottom() {
                frame.buffer_mut()[(x, y)]
                    .set_symbol(if active { "┃" } else { "│" })
                    .set_fg(color)
                    .set_bg(app.ui_theme.surface_bg);
            }
        }
    }
}

fn render_scrollbar(
    frame: &mut Frame,
    area: Rect,
    offset: usize,
    visible: usize,
    total: usize,
    app: &App,
) {
    let rail_height = area.height;
    if rail_height == 0 || total == 0 {
        return;
    }
    let thumb_height = ((usize::from(rail_height) * visible) / total)
        .max(1)
        .min(usize::from(rail_height));
    let max_offset = total.saturating_sub(visible).max(1);
    let max_start = usize::from(rail_height).saturating_sub(thumb_height);
    let thumb_start = offset.saturating_mul(max_start) / max_offset;
    let x = area.right().saturating_sub(1);
    for row in 0..usize::from(rail_height) {
        let in_thumb = row >= thumb_start && row < thumb_start.saturating_add(thumb_height);
        frame.buffer_mut()[(x, area.y.saturating_add(row as u16))]
            // Match the transcript rail exactly: a fine border track with a
            // brighter, narrow thumb. The old solid block looked like a
            // separate native scrollbar bolted onto the work surface.
            .set_symbol(if in_thumb { "┃" } else { "│" })
            .set_fg(if in_thumb {
                app.ui_theme.status_working
            } else {
                app.ui_theme.border
            })
            .set_bg(app.ui_theme.surface_bg);
    }
}
