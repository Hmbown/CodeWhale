use ratatui::{
    Frame,
    layout::Rect,
    prelude::Widget,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::tui::app::{App, SidebarHoverRow, SidebarHoverSection};
use crate::tui::ui_text::truncate_line_to_width;

use super::model::{WorkHitbox, WorkRow, WorkSurfacePlacement, WorkTone, project};

const SIDE_RAIL_MIN_HOST_WIDTH: u16 = 72;
const SIDE_RAIL_MIN_WIDTH: u16 = 26;
const SIDE_RAIL_MAX_WIDTH: u16 = 40;
const SIDE_RAIL_MIN_CHAT_WIDTH: u16 = 40;

fn effective_placement(
    configured: WorkSurfacePlacement,
    host_width: u16,
    classic_shell: bool,
) -> WorkSurfacePlacement {
    if classic_shell || host_width < SIDE_RAIL_MIN_HOST_WIDTH {
        WorkSurfacePlacement::Top
    } else {
        configured
    }
}

/// Responsive work-surface height. The component owns a bounded window; long
/// work lists scroll instead of consuming the transcript.
pub fn height(app: &mut App, width: u16, terminal_height: u16, classic_shell: bool) -> u16 {
    let rows = project(app);
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
        return 0;
    }
    app.work_surface.effective_placement =
        effective_placement(app.work_surface.placement, width, classic_shell);
    if app.work_surface.effective_placement != WorkSurfacePlacement::Top {
        return 0;
    }
    let cap = match terminal_height {
        0..=12 => 3,
        13..=16 => 5,
        17..=23 => 6,
        _ => 8,
    };
    // Reserve only the rows the projection can actually paint, plus the
    // panel-owned divider. The old fixed cap left three or four empty rows
    // behind a small/completed Fleet, taking transcript space without adding
    // any information (especially visible in an 89x50 Cursor terminal).
    let content_height = u16::try_from(rows.len()).unwrap_or(u16::MAX);
    content_height.saturating_add(1).min(cap)
}

/// Split the transcript slot for a side rail. Top placement consumes its own
/// vertical row before this point, so it returns the chat area unchanged.
/// Classic always resolves to Top and therefore preserves its existing layout.
pub fn split_chat(app: &mut App, area: Rect, classic_shell: bool) -> (Rect, Option<Rect>) {
    let placement = effective_placement(app.work_surface.placement, area.width, classic_shell);
    app.work_surface.effective_placement = placement;
    if app.work_surface.latest_rows.is_empty() || placement == WorkSurfacePlacement::Top {
        return (area, None);
    }

    let proportional = area.width.saturating_mul(30) / 100;
    let rail_width = proportional
        .clamp(SIDE_RAIL_MIN_WIDTH, SIDE_RAIL_MAX_WIDTH)
        .min(area.width.saturating_sub(SIDE_RAIL_MIN_CHAT_WIDTH));
    if rail_width < SIDE_RAIL_MIN_WIDTH {
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
        WorkSurfacePlacement::Top => (area, None),
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
    };

    let mut rows = project(app);
    if body_area.height <= 2 && rows.len() > usize::from(body_area.height) {
        // Compact fallback spends its two content rows on the first actionable
        // Task and To-do/worker objects instead of section chrome.
        let mut compact = Vec::new();
        for prefix in ["task:", "todo:", "worker:"] {
            if let Some(row) = rows.iter().find(|row| row.id.0.starts_with(prefix)) {
                compact.push(row.clone());
            }
        }
        for row in rows.iter().filter(|row| row.selectable) {
            if !compact.iter().any(|candidate| candidate.id == row.id) {
                compact.push(row.clone());
            }
        }
        rows = compact;
    }
    let body_height = usize::from(body_area.height);
    let overflow = rows.len() > body_height;
    let inset = u16::from(body_area.width >= 60);
    let rail_width = u16::from(overflow);
    let content_area = Rect {
        x: body_area.x.saturating_add(inset),
        y: body_area.y,
        width: body_area
            .width
            .saturating_sub(inset.saturating_mul(2))
            .saturating_sub(rail_width),
        height: body_area.height,
    };

    app.work_surface.visible_rows = body_height;
    app.work_surface.total_rows = rows.len();
    app.work_surface.clamp_selection(&rows);
    let max_offset = rows.len().saturating_sub(body_height.max(1));
    app.work_surface.scroll_offset = app.work_surface.scroll_offset.min(max_offset);

    Block::default()
        .style(Style::default().bg(app.ui_theme.surface_bg))
        .render(area, frame.buffer_mut());

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
        let compact_owner = if body_area.height <= 2 {
            row.id
                .0
                .split_once(':')
                .map(|(kind, _)| match kind {
                    "graph" => "Work · ".to_string(),
                    _ => String::new(),
                })
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
        let detail = if row.tone != WorkTone::Heading && content_area.width >= 44 {
            format!("  {}", row.detail)
        } else {
            String::new()
        };
        let detail_width = UnicodeWidthStr::width(detail.as_str());
        let label_width = usize::from(content_area.width)
            .saturating_sub(UnicodeWidthStr::width(prefix.as_str()) + detail_width)
            .max(1);
        let label = truncate_line_to_width(&row.label, label_width);
        let gap = usize::from(content_area.width).saturating_sub(
            UnicodeWidthStr::width(prefix.as_str())
                + UnicodeWidthStr::width(label.as_str())
                + detail_width,
        );
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
                is_truncated: label != row.label || detail.is_empty(),
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
            body_area,
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
    match placement {
        WorkSurfacePlacement::Top => {
            let y = area.bottom().saturating_sub(1);
            for x in area.left()..area.right() {
                frame.buffer_mut()[(x, y)]
                    .set_symbol("─")
                    .set_fg(app.ui_theme.border)
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
                    .set_symbol("│")
                    .set_fg(app.ui_theme.border)
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
