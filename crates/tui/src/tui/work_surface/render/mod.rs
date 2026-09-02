//! Painting the work surface, and the two files it leans on.
//!
//! - [`layout`] answers *where and how tall* — placement fallback, the height
//!   and cap arithmetic, and the side-rail split.
//! - [`rows`] answers *what one row says* — the sub-agent column layout, its
//!   degradation tiers, and row styling.
//!
//! What stays here is the paint itself: the Top strip, the side-rail panel,
//! the divider and scrollbar chrome, and the strip header content (goal title,
//! to-do receipt) that height and paint must both agree on.

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
    RailPanel, WorkHitbox, WorkRow, WorkSurfacePlacement, WorkTone, visible_rows_for_panel,
};

mod layout;
mod rows;

pub(crate) use layout::collapse_strip;
pub use layout::{height, split_chat};

use rows::{
    AGENT_ROLE_GUTTER, AgentRowTier, agent_identity, agent_identity_cap, agent_identity_column,
    agent_receipt, agent_row_styles, agent_status_column, layout_agent_row, row_style,
};

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        collapse_strip(app);
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
        collapse_strip(app);
        return;
    }
    let body_area = match placement {
        // Bottom mirrors Top's body/divider split; only the divider edge
        // differs (below-content for Top, above-content for Bottom).
        WorkSurfacePlacement::Top => Rect {
            height: area.height.saturating_sub(1),
            ..area
        },
        WorkSurfacePlacement::Bottom => Rect {
            y: area.y.saturating_add(1),
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

    // Context is the one panel that is not a work-row surface: session facts
    // render as a titled line list with nothing to click. Every other panel
    // (Tasks, Agents, Pinned) routes through the row machinery below, so its
    // rows keep hitboxes, selection, and primary actions — a work row is a
    // door in every panel, not only in Tasks.
    if app.work_surface.panel == RailPanel::Context {
        render_panel(frame, area, body_area, app);
        return;
    }

    let mut rows = visible_rows_for_panel(app);
    if placement.is_strip() {
        // Literal work list only: selectable to-dos/agents plus the
        // GrokBuild-style `▾ Subagents N` group header. Generic graph chrome
        // from the side/inspector projection stays out.
        rows.retain(|row| row.selectable || row.id.0.starts_with("section:"));
    }
    let todo_ordinals = if placement.is_strip() {
        todo_ordinals(&rows)
    } else {
        HashMap::new()
    };
    let ordinal_width = todo_ordinals.len().max(1).to_string().len();
    let goal_title = placement.is_strip().then(|| top_goal_title(app)).flatten();
    let todo_progress = placement
        .is_strip()
        .then(|| top_todo_progress(app, &rows))
        .flatten();
    // Pin goal title, then progress receipt, above the scrollable rows.
    // At the minimum two-row surface keep one usable content row + divider.
    let goal_height = u16::from(goal_title.is_some() && body_area.height >= 1);
    let fold_progress = progress_shares_goal_row(body_area.width, goal_height > 0);
    let progress_height = u16::from(
        todo_progress.is_some()
            && !fold_progress
            && body_area.height.saturating_sub(goal_height) >= 2,
    );
    let header_height = goal_height.saturating_add(progress_height);
    let list_height = body_area.height.saturating_sub(header_height);
    let body_height = usize::from(list_height);
    let overflow = rows.len() > body_height;
    // A capped list owes the reader the size of what it is hiding, so the
    // last painted row becomes `↓ N more`. The scrollbar shows position; only
    // this shows how much work is off-screen.
    let more_row = overflow && body_height >= 2;
    let list_rows = if more_row {
        body_height.saturating_sub(1)
    } else {
        body_height
    };
    let inset = u16::from(body_area.width >= 60);
    let rail_width = u16::from(overflow);
    let content_area = Rect {
        x: body_area.x.saturating_add(inset),
        y: body_area.y.saturating_add(header_height),
        width: body_area
            .width
            .saturating_sub(inset.saturating_mul(2))
            .saturating_sub(rail_width),
        height: list_height,
    };

    app.work_surface.visible_rows = list_rows;
    app.work_surface.total_rows = rows.len();
    // A redraw may clamp an obsolete offset, but it must not reveal the
    // remembered keyboard selection: doing so undoes mouse-wheel scrolling
    // whenever that selection is above the viewport (#4594).
    app.work_surface.clamp_viewport(&rows);
    let max_offset = rows.len().saturating_sub(list_rows.max(1));
    app.work_surface.scroll_offset = app.work_surface.scroll_offset.min(max_offset);

    Block::default()
        .style(Style::default().bg(app.ui_theme.surface_bg))
        .render(area, frame.buffer_mut());

    if let Some((goal_text, goal_style)) = goal_title.filter(|_| goal_height > 0) {
        let full_width = usize::from(content_area.width);
        // Wide strips carry the receipt right-aligned on the goal row rather
        // than spending a second row announcing a count.
        let receipt = todo_progress.as_deref().filter(|_| fold_progress);
        let reserved = receipt
            .map(|text| UnicodeWidthStr::width(text).saturating_add(2))
            .unwrap_or(0);
        let goal_text = truncate_line_to_width(&goal_text, full_width.saturating_sub(reserved));
        let mut spans = vec![Span::styled(
            goal_text.clone(),
            goal_style.bg(app.ui_theme.surface_bg),
        )];
        if let Some(receipt) = receipt {
            let gap = full_width
                .saturating_sub(UnicodeWidthStr::width(goal_text.as_str()))
                .saturating_sub(UnicodeWidthStr::width(receipt));
            spans.push(Span::styled(
                format!("{}{receipt}", " ".repeat(gap)),
                Style::default()
                    .fg(app.ui_theme.text_muted)
                    .bg(app.ui_theme.surface_bg),
            ));
        }
        Paragraph::new(Line::from(spans)).render(
            Rect {
                y: body_area.y,
                height: 1,
                ..content_area
            },
            frame.buffer_mut(),
        );
    }

    if let Some(progress) = todo_progress.filter(|_| progress_height > 0) {
        let progress = truncate_line_to_width(&progress, usize::from(content_area.width));
        // Muted, not accent: accent_primary means "selected" everywhere else
        // in the strip, and spending it on a static count makes the actual
        // selection hard to find.
        Paragraph::new(Line::from(Span::styled(
            progress,
            Style::default()
                .fg(app.ui_theme.text_muted)
                .bg(app.ui_theme.surface_bg),
        )))
        .render(
            Rect {
                y: body_area.y.saturating_add(goal_height),
                height: 1,
                ..content_area
            },
            frame.buffer_mut(),
        );
    }

    let start = app.work_surface.scroll_offset;
    let visible = rows.iter().skip(start).take(list_rows).collect::<Vec<_>>();
    let identity_cap = agent_identity_cap(usize::from(content_area.width));
    let identity_column = agent_identity_column(&visible, identity_cap);
    let status_column = agent_status_column(&visible);
    let mut lines = Vec::with_capacity(visible.len().saturating_add(1));
    let mut hover_rows = Vec::new();
    let mut hitboxes = Vec::new();
    for (visible_index, row) in visible.iter().enumerate() {
        let row_y = content_area.y.saturating_add(visible_index as u16);
        let selected =
            app.work_surface.focused && app.work_surface.selected.as_ref() == Some(&row.id);
        let hovered = app.work_surface.hovered.as_ref() == Some(&row.id);
        let opened = app.work_surface.opened.as_ref() == Some(&row.id);
        let style = row_style(app, row, selected, hovered, opened);
        let compact_owner = if placement.is_strip() {
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
        // Agent focus marker: while a worker is focused every row gains a
        // two-cell gutter and the focused worker's row shows the selection
        // glyph in it, so the addressed fork is visible at the left edge.
        let focus_gutter = if app.agent_focus.is_some() {
            let focused = row
                .id
                .0
                .strip_prefix("worker:")
                .is_some_and(|id| app.agent_focus.as_ref().is_some_and(|f| f.is(id)));
            if focused {
                "❯ ".to_string()
            } else {
                "  ".to_string()
            }
        } else {
            String::new()
        };
        let prefix = if row.tone == WorkTone::Heading {
            format!("{focus_gutter}{} ", mark)
        } else {
            format!("{focus_gutter}{compact_owner}{mark} ")
        };

        // Sub-agent rows own their own column layout: glyph, agent type,
        // objective, right-aligned elapsed and tokens. They stay ordinary
        // rows in every other respect — same hitbox, same selection, same
        // primary action.
        if let Some(facts) = row.agent.as_ref() {
            let queued = row
                .id
                .0
                .strip_prefix("worker:")
                .and_then(|id| crate::tui::agent_focus::queued_suffix(app, id))
                .map(|queued| format!(" · {queued}"));
            let queued_width = queued.as_deref().map(UnicodeWidthStr::width).unwrap_or(0);
            let laid_out = layout_agent_row(
                usize::from(content_area.width).saturating_sub(queued_width),
                UnicodeWidthStr::width(prefix.as_str()),
                agent_identity(row, identity_cap),
                identity_column,
                status_column,
                facts,
            );
            let (normal, muted) = agent_row_styles(app, selected, hovered, opened);
            let display = format!(
                "{prefix}{}{}{}{}{}{}{}",
                laid_out.role,
                if laid_out.role.is_empty() {
                    String::new()
                } else {
                    " ".repeat(AGENT_ROLE_GUTTER)
                },
                laid_out.status,
                if laid_out.status.is_empty() {
                    String::new()
                } else {
                    " ".repeat(AGENT_ROLE_GUTTER)
                },
                laid_out.objective,
                " ".repeat(laid_out.gap),
                laid_out.receipt,
            );
            let mut spans = vec![Span::styled(prefix.clone(), normal)];
            if !laid_out.role.is_empty() {
                spans.push(Span::styled(
                    format!("{}{}", laid_out.role, " ".repeat(AGENT_ROLE_GUTTER)),
                    muted,
                ));
            }
            if !laid_out.status.is_empty() {
                spans.push(Span::styled(
                    format!("{}{}", laid_out.status, " ".repeat(AGENT_ROLE_GUTTER)),
                    muted,
                ));
            }
            spans.push(Span::styled(laid_out.objective.clone(), normal));
            spans.push(Span::styled(
                format!("{}{}", " ".repeat(laid_out.gap), laid_out.receipt),
                muted,
            ));
            if let Some(queued) = queued.as_deref() {
                // Truthful `· N queued`: follow-ups the running child has not
                // yet folded into its next round. Accent so it reads as live
                // pending work, not as part of the receipt.
                spans.push(Span::styled(
                    queued.to_string(),
                    Style::default()
                        .fg(app.ui_theme.accent_action)
                        .bg(normal.bg.unwrap_or(app.ui_theme.surface_bg)),
                ));
            }
            lines.push(Line::from(spans));

            hitboxes.push(WorkHitbox {
                id: row.id.clone(),
                row_y,
            });
            hover_rows.push(SidebarHoverRow {
                row_y,
                display_text: display,
                full_text: format!("{} · {}", row.label, row.detail),
                detail: Some(row.detail.clone()),
                is_truncated: laid_out.objective != facts.objective
                    || laid_out.receipt != agent_receipt(facts, AgentRowTier::Full),
                click_action: row.primary_action.clone(),
                stop_action: None,
                stop_zone_start_col: None,
                stop_zone_end_col: None,
            });
            continue;
        }

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

    if more_row {
        // Right-aligned under the receipt column, muted like every other
        // secondary figure. Scrolled to the bottom there is nothing below, so
        // the reserved row stays blank rather than claiming a count of zero.
        let remaining = rows
            .len()
            .saturating_sub(start.saturating_add(visible.len()));
        let text = if remaining == 0 {
            String::new()
        } else {
            truncate_line_to_width(
                &format!("↓ {remaining} more"),
                usize::from(content_area.width),
            )
        };
        let pad = usize::from(content_area.width).saturating_sub(UnicodeWidthStr::width(&*text));
        lines.push(Line::from(Span::styled(
            format!("{}{text}", " ".repeat(pad)),
            Style::default()
                .fg(app.ui_theme.text_muted)
                .bg(app.ui_theme.surface_bg),
        )));
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
            list_rows,
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

/// Render the Context panel as a titled line list in the same body area and
/// with the same divider and scrollbar the row surface would use. Context is
/// the only panel that renders here: its lines are session facts, not work
/// rows, so there is nothing to click and no hitboxes to record. Every panel
/// that shows work rows (Tasks, Agents, Pinned) goes through the row/hitbox
/// machinery in [`render`] instead.
fn render_panel(frame: &mut Frame, area: Rect, body_area: Rect, app: &mut App) {
    let panel = app.work_surface.panel;
    let placement = app.work_surface.effective_placement;

    Block::default()
        .style(Style::default().bg(app.ui_theme.surface_bg))
        .render(area, frame.buffer_mut());

    // Title row policy:
    // - Top/Bottom: only an active goal (`Goal: …`). Never panel chrome ("Pinned").
    // - Left/Right: muted panel name — a full-height column needs naming.
    let goal = placement.is_strip().then(|| top_goal_title(app)).flatten();
    let side_panel_title = matches!(
        placement,
        WorkSurfacePlacement::Left | WorkSurfacePlacement::Right
    );

    let title_rows = if let Some((goal_text, goal_style)) = goal.as_ref() {
        let goal_text = truncate_line_to_width(goal_text, usize::from(body_area.width).max(1));
        Paragraph::new(Line::from(Span::styled(
            goal_text,
            goal_style.bg(app.ui_theme.surface_bg),
        )))
        .render(
            Rect {
                height: 1,
                ..body_area
            },
            frame.buffer_mut(),
        );
        1_u16
    } else if side_panel_title {
        Paragraph::new(Line::from(Span::styled(
            truncate_line_to_width(panel.title(), usize::from(body_area.width).max(1)),
            Style::default()
                .fg(app.ui_theme.text_muted)
                .bg(app.ui_theme.surface_bg),
        )))
        .render(
            Rect {
                height: 1,
                ..body_area
            },
            frame.buffer_mut(),
        );
        1_u16
    } else {
        0
    };

    let content_area = Rect {
        y: body_area.y.saturating_add(title_rows),
        height: body_area.height.saturating_sub(title_rows),
        ..body_area
    };
    let body_height = usize::from(content_area.height);
    let lines = super::panels::panel_lines(
        app,
        panel,
        usize::from(content_area.width),
        body_height.max(1),
        goal.is_some(),
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

/// Active goal as the Top strip's only title. Uses the same
/// paused/active/terminal resolution as the ocean header chip so a goal set
/// via `create_goal` is either visible everywhere or nowhere. Returns
/// `None` when no live goal exists — Top then paints no title row at all.
pub(super) fn top_goal_title(app: &App) -> Option<(String, Style)> {
    let (objective, paused) = crate::tui::footer_ui::active_goal_chip_state(app)?;
    let flat = objective.trim().replace(['\n', '\r'], " ");
    if flat.is_empty() {
        return None;
    }
    let text = if paused {
        format!("Goal (paused): {flat}")
    } else {
        format!("Goal: {flat}")
    };
    let style = if paused {
        Style::default()
            .fg(app.ui_theme.warning)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(app.ui_theme.status_working)
            .add_modifier(Modifier::BOLD)
    };
    Some((text, style))
}

fn todo_ordinals(rows: &[WorkRow]) -> HashMap<String, usize> {
    rows.iter()
        .filter(|row| row.id.0.starts_with("graph:"))
        .enumerate()
        .map(|(index, row)| (row.id.0.clone(), index.saturating_add(1)))
        .collect()
}

/// Below this width the goal title and the receipt cannot both stay readable
/// on one row, so the receipt keeps its own row.
const PROGRESS_FOLD_MIN_WIDTH: u16 = 72;

/// Whether the to-do receipt rides on the goal-title row instead of claiming
/// a row of its own.
///
/// [`height`] and [`render`] must agree on this or the strip paints into a row
/// it did not reserve, so the rule is a pure function of the strip width and
/// whether there is a goal title to share with.
pub(super) fn progress_shares_goal_row(width: u16, has_goal_title: bool) -> bool {
    has_goal_title && width >= PROGRESS_FOLD_MIN_WIDTH
}

pub(super) fn top_todo_progress(app: &App, rows: &[WorkRow]) -> Option<String> {
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
        WorkSurfacePlacement::Bottom => {
            let y = area.top();
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
