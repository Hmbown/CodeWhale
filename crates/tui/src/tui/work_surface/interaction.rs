//! Typed work-surface interaction ownership (TUI-DOG-004 / 005 / 006).
//!
//! Selection, focus, and detail-open are distinct axes. Destructive lifecycle
//! actions live inside the inspector pager, not in compact rows.

use crate::tui::app::{App, SidebarRowAction};
use crate::tui::views::ModalKind;

use super::model::WorkRowId;

/// Claim work-surface focus and clear competing selection owners.
pub fn claim_focus(app: &mut App) {
    let was_focused = app.work_surface.focused;
    app.work_surface.focused = true;
    if app.viewport.transcript_selection.is_active() {
        app.viewport.transcript_selection.clear();
    }
    if !was_focused {
        app.needs_redraw = true;
    }
}

/// Release work-surface focus without clearing the remembered selection.
pub fn release_focus(app: &mut App) {
    if !app.work_surface.focused && app.work_surface.hovered.is_none() {
        return;
    }
    app.work_surface.focused = false;
    app.work_surface.hovered = None;
    app.needs_redraw = true;
}

/// Open or toggle-close the primary detail for a row.
///
/// Enter/click on an already-opened selected row closes it. Opening a different
/// row updates the inspector owner.
pub fn activate_primary(
    app: &mut App,
    row_id: &WorkRowId,
    primary: Option<SidebarRowAction>,
) -> Option<SidebarRowAction> {
    if app.work_surface.opened.as_ref() == Some(row_id) {
        close_opened(app);
        return None;
    }
    app.work_surface.selected = Some(row_id.clone());
    let action = primary?;
    app.work_surface.opened = Some(row_id.clone());
    Some(action)
}

/// Close the work-surface-owned detail (pager when we opened it).
pub fn close_opened(app: &mut App) {
    if app.work_surface.opened.take().is_none() {
        return;
    }
    if app.view_stack.top_kind() == Some(ModalKind::Pager) {
        app.view_stack.pop();
    }
    app.needs_redraw = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::TuiOptions;
    use std::path::PathBuf;

    fn app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: true,
            use_bracketed_paste: true,
            max_subagents: 4,
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

    #[test]
    fn primary_toggles_opened_closed() {
        let mut app = app();
        let row = WorkRowId("worker:a1".into());
        let open = SidebarRowAction::OpenAgentDetail {
            agent_id: "a1".into(),
        };
        assert!(activate_primary(&mut app, &row, Some(open.clone())).is_some());
        assert_eq!(app.work_surface.opened.as_ref(), Some(&row));
        assert!(activate_primary(&mut app, &row, Some(open)).is_none());
        assert!(app.work_surface.opened.is_none());
    }

    #[test]
    fn claim_focus_clears_transcript_selection() {
        use crate::tui::selection::TranscriptSelectionPoint;
        let mut app = app();
        app.viewport.transcript_selection.anchor = Some(TranscriptSelectionPoint {
            line_index: 0,
            column: 0,
        });
        app.viewport.transcript_selection.head = app.viewport.transcript_selection.anchor;
        claim_focus(&mut app);
        assert!(app.work_surface.focused);
        assert!(!app.viewport.transcript_selection.is_active());
    }
}
