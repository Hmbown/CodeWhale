use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::tui::app::{App, SidebarRowAction};

use super::interaction::{activate_primary, claim_focus, close_opened, release_focus};
use super::model::{
    SIDE_WIDTH_MAX, SIDE_WIDTH_MIN, TOP_HEIGHT_MAX, TOP_HEIGHT_MIN, WorkRow, WorkRowId,
    WorkSurfacePlacement, visible_rows_for_panel,
};

#[derive(Debug, Default)]
pub struct MouseOutcome {
    pub consumed: bool,
    pub action: Option<SidebarRowAction>,
}

/// `← for agents`: switch the rail to the Agents panel and give it keyboard
/// ownership so ↑/↓ + Enter select and focus a worker. Returns `false` when
/// the rail cannot show agents right now (rail off, or nothing to list); the
/// caller then opens the `/agents` register instead so the key still lands.
pub fn enter_agents(app: &mut App) -> bool {
    app.work_surface.panel = super::model::RailPanel::Agents;
    if app.work_surface.placement == WorkSurfacePlacement::Off
        || app.work_surface.effective_placement == WorkSurfacePlacement::Off
        || app.work_surface.last_area.is_none()
    {
        release_focus(app);
        return false;
    }
    let rows = visible_rows_for_panel(app);
    let first_agent = rows
        .iter()
        .find(|row| {
            row.selectable
                && row.id.0.starts_with("worker:")
                && app
                    .work_surface
                    .hitboxes
                    .iter()
                    .any(|hitbox| hitbox.id == row.id)
        })
        .map(|row| row.id.clone());
    let Some(first_agent) = first_agent else {
        return false;
    };
    claim_focus(app);
    let selected_agent_is_visible = app.work_surface.selected.as_ref().is_some_and(|selected| {
        rows.iter()
            .any(|row| row.selectable && row.id == *selected && row.id.0.starts_with("worker:"))
            && app
                .work_surface
                .hitboxes
                .iter()
                .any(|hitbox| hitbox.id == *selected)
    });
    if !selected_agent_is_visible {
        app.work_surface.selected = Some(first_agent);
    }
    app.work_surface.clamp_selection(&rows);
    app.needs_redraw = true;
    true
}

/// Handle the work surface's focused keyboard contract. `Alt+W` enters the
/// surface from the composer; Esc returns ownership to the composer (or clears
/// a local stop arm / open detail first). Plain printable input always returns
/// ownership to the composer instead of becoming a hidden panel shortcut.
pub fn handle_key(app: &mut App, key: KeyEvent) -> Option<Option<SidebarRowAction>> {
    // A starved, mini-window-hidden, or explicitly disabled rail owns no
    // cells, so it cannot own keyboard focus. `collapse_strip` normally
    // clears this at layout time; this guard also closes the pre-redraw race.
    if app.work_surface.last_area.is_none() {
        if app.work_surface.focused {
            release_focus(app);
        }
        return None;
    }
    // Keyboard and mouse share one row source per panel: Enter on the
    // selected row must open the same world a click would.
    let rows = visible_rows_for_panel(app);
    if rows.is_empty() {
        return None;
    }
    if !app.work_surface.focused {
        if key.code == KeyCode::Char('w') && key.modifiers.contains(KeyModifiers::ALT) {
            claim_focus(app);
            app.work_surface.clamp_selection(&rows);
            app.needs_redraw = true;
            return Some(None);
        }
        return None;
    }

    if matches!(key.code, KeyCode::Char(_))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        release_focus(app);
        return None;
    }

    // The details chord opens the selected row's own world; the transcript
    // pager owns ⌥V only when no work row is selected.
    if crate::tui::shell_key_routing::is_tool_details_shortcut(&key) {
        let action = selected_row(app, &rows)
            .and_then(|row| activate_primary(app, &row.id, row.primary_action.clone()));
        if action.is_some() {
            app.work_surface.clamp_selection(&rows);
            app.needs_redraw = true;
            return Some(action);
        }
        return None;
    }

    let action = match key.code {
        KeyCode::Esc => {
            if app.work_surface.opened.is_some() {
                close_opened(app);
            } else {
                release_focus(app);
            }
            return Some(None);
        }
        KeyCode::Up => {
            move_selection(app, &rows, -1);
            None
        }
        KeyCode::Down => {
            move_selection(app, &rows, 1);
            None
        }
        KeyCode::Home => {
            select_edge(app, &rows, false);
            None
        }
        KeyCode::End => {
            select_edge(app, &rows, true);
            None
        }
        KeyCode::PageUp => {
            move_selection(app, &rows, -(app.work_surface.visible_rows.max(1) as isize));
            None
        }
        KeyCode::PageDown => {
            move_selection(app, &rows, app.work_surface.visible_rows.max(1) as isize);
            None
        }
        KeyCode::Enter => selected_row(app, &rows)
            .and_then(|row| activate_primary(app, &row.id, row.primary_action.clone())),
        _ => return None,
    };
    app.work_surface.clamp_selection(&rows);
    app.needs_redraw = true;
    Some(action)
}

pub fn handle_mouse(app: &mut App, mouse: MouseEvent) -> MouseOutcome {
    let Some(area) = app.work_surface.last_area else {
        return MouseOutcome::default();
    };
    let placement = app.work_surface.effective_placement;
    let on_divider = match placement {
        WorkSurfacePlacement::Off => false,
        WorkSurfacePlacement::Top => {
            mouse.row == area.bottom().saturating_sub(1)
                && mouse.column >= area.x
                && mouse.column < area.right()
        }
        WorkSurfacePlacement::Bottom => {
            mouse.row == area.y && mouse.column >= area.x && mouse.column < area.right()
        }
        WorkSurfacePlacement::Left => {
            mouse.column == area.right().saturating_sub(1)
                && mouse.row >= area.y
                && mouse.row < area.bottom()
        }
        WorkSurfacePlacement::Right => {
            mouse.column == area.x && mouse.row >= area.y && mouse.row < area.bottom()
        }
    };

    if matches!(mouse.kind, MouseEventKind::Moved) && app.work_surface.divider_hovered != on_divider
    {
        app.work_surface.divider_hovered = on_divider;
        app.needs_redraw = true;
    }

    match mouse.kind {
        MouseEventKind::Moved if on_divider => {
            return MouseOutcome {
                consumed: true,
                action: None,
            };
        }
        MouseEventKind::Down(MouseButton::Left) if on_divider => {
            app.work_surface.resizing = true;
            app.work_surface.divider_hovered = true;
            app.work_surface.resize_anchor_column = mouse.column;
            app.work_surface.resize_anchor_row = mouse.row;
            app.work_surface.resize_anchor_size = match placement {
                WorkSurfacePlacement::Top | WorkSurfacePlacement::Bottom => area.height,
                WorkSurfacePlacement::Left | WorkSurfacePlacement::Right => area.width,
                WorkSurfacePlacement::Off => area.width,
            };
            app.needs_redraw = true;
            return MouseOutcome {
                consumed: true,
                action: None,
            };
        }
        MouseEventKind::Drag(MouseButton::Left) if app.work_surface.resizing => {
            let anchor = i32::from(app.work_surface.resize_anchor_size);
            match placement {
                WorkSurfacePlacement::Top => {
                    let delta =
                        i32::from(mouse.row) - i32::from(app.work_surface.resize_anchor_row);
                    app.work_surface.top_height = (anchor + delta)
                        .clamp(i32::from(TOP_HEIGHT_MIN), i32::from(TOP_HEIGHT_MAX))
                        as u16;
                }
                // The bottom strip's divider is its TOP edge: dragging up
                // grows the strip, so the delta is inverted from Top.
                WorkSurfacePlacement::Bottom => {
                    let delta =
                        i32::from(app.work_surface.resize_anchor_row) - i32::from(mouse.row);
                    app.work_surface.top_height = (anchor + delta)
                        .clamp(i32::from(TOP_HEIGHT_MIN), i32::from(TOP_HEIGHT_MAX))
                        as u16;
                }
                WorkSurfacePlacement::Left => {
                    let delta =
                        i32::from(mouse.column) - i32::from(app.work_surface.resize_anchor_column);
                    app.work_surface.side_width = (anchor + delta)
                        .clamp(i32::from(SIDE_WIDTH_MIN), i32::from(SIDE_WIDTH_MAX))
                        as u16;
                }
                WorkSurfacePlacement::Right => {
                    let delta =
                        i32::from(app.work_surface.resize_anchor_column) - i32::from(mouse.column);
                    app.work_surface.side_width = (anchor + delta)
                        .clamp(i32::from(SIDE_WIDTH_MIN), i32::from(SIDE_WIDTH_MAX))
                        as u16;
                }
                WorkSurfacePlacement::Off => {}
            }
            app.needs_redraw = true;
            return MouseOutcome {
                consumed: true,
                action: None,
            };
        }
        MouseEventKind::Up(MouseButton::Left) if app.work_surface.resizing => {
            app.work_surface.resizing = false;
            app.work_surface.divider_hovered = on_divider;
            let top_height = app.work_surface.top_height;
            let side_width = app.work_surface.side_width;
            if let Err(error) = crate::settings::Settings::transact(|settings| {
                settings.work_surface_top_height = top_height;
                settings.work_surface_side_width = side_width;
                Ok(())
            }) {
                app.status_message =
                    Some(format!("Failed to save To-do/Sub-agent bar size: {error}"));
            }
            app.needs_redraw = true;
            return MouseOutcome {
                consumed: true,
                action: None,
            };
        }
        _ => {}
    }
    let inside = mouse.column >= area.x
        && mouse.column < area.right()
        && mouse.row >= area.y
        && mouse.row < area.bottom();
    if !inside {
        if matches!(
            mouse.kind,
            MouseEventKind::Down(MouseButton::Left)
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollDown
        ) && app.work_surface.focused
        {
            // Another region is taking the pointer — release strip focus so
            // only one owner shows selection.
            release_focus(app);
        }
        if matches!(mouse.kind, MouseEventKind::Moved) && app.work_surface.hovered.take().is_some()
        {
            app.needs_redraw = true;
        }
        return MouseOutcome::default();
    }

    match mouse.kind {
        MouseEventKind::ScrollUp => {
            claim_focus(app);
            app.work_surface.scroll_offset = app.work_surface.scroll_offset.saturating_sub(2);
            app.needs_redraw = true;
            MouseOutcome {
                consumed: true,
                action: None,
            }
        }
        MouseEventKind::ScrollDown => {
            claim_focus(app);
            let max = app
                .work_surface
                .total_rows
                .saturating_sub(app.work_surface.visible_rows.max(1));
            app.work_surface.scroll_offset =
                app.work_surface.scroll_offset.saturating_add(2).min(max);
            app.needs_redraw = true;
            MouseOutcome {
                consumed: true,
                action: None,
            }
        }
        MouseEventKind::Moved => {
            let hovered = hit_row(app, mouse.row).map(|row| row.id.clone());
            if app.work_surface.hovered != hovered {
                app.work_surface.hovered = hovered;
                app.needs_redraw = true;
            }
            MouseOutcome {
                consumed: true,
                action: None,
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let row = hit_row(app, mouse.row).cloned();
            let Some(row) = row else {
                claim_focus(app);
                return MouseOutcome {
                    consumed: true,
                    action: None,
                };
            };
            claim_focus(app);
            app.work_surface.selected = Some(row.id.clone());
            app.needs_redraw = true;

            let action = activate_primary(app, &row.id, row.primary_action.clone());
            MouseOutcome {
                consumed: true,
                action,
            }
        }
        _ => MouseOutcome {
            consumed: true,
            action: None,
        },
    }
}

fn hit_row(app: &App, row_y: u16) -> Option<&WorkRow> {
    let id = app
        .work_surface
        .hitboxes
        .iter()
        .find(|hitbox| hitbox.row_y == row_y)
        .map(|hitbox| &hitbox.id)?;
    app.work_surface
        .latest_rows
        .iter()
        .find(|row| &row.id == id)
}

fn selected_row<'a>(app: &App, rows: &'a [WorkRow]) -> Option<&'a WorkRow> {
    let selected = app.work_surface.selected.as_ref()?;
    rows.iter().find(|row| &row.id == selected)
}

fn selectable_ids(rows: &[WorkRow]) -> Vec<WorkRowId> {
    rows.iter()
        .filter(|row| row.selectable)
        .map(|row| row.id.clone())
        .collect()
}

fn move_selection(app: &mut App, rows: &[WorkRow], delta: isize) {
    let ids = selectable_ids(rows);
    if ids.is_empty() {
        return;
    }
    let current = app
        .work_surface
        .selected
        .as_ref()
        .and_then(|selected| ids.iter().position(|id| id == selected))
        .unwrap_or_default();
    let next = if delta.is_negative() {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        current
            .saturating_add(delta as usize)
            .min(ids.len().saturating_sub(1))
    };
    app.work_surface.selected = Some(ids[next].clone());
}

fn select_edge(app: &mut App, rows: &[WorkRow], end: bool) {
    let ids = selectable_ids(rows);
    app.work_surface.selected = if end {
        ids.last().cloned()
    } else {
        ids.first().cloned()
    };
}
