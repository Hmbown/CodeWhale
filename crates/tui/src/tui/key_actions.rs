//! Keyboard event action handlers extracted from `ui.rs`.
//!
//! Each function handles a focused subset of keyboard input so the
//! main event loop stays lean. Functions that need to signal a
//! control-flow change (shutdown, return to caller) communicate via
//! [`KeyActionResult`].

use crossterm::event::{KeyCode, KeyEvent};

use super::app::App;

// ── File-tree key handling ───────────────────────────────────────

/// Handle keyboard input when the file-tree pane is visible.
///
/// Returns `true` when the key was consumed (caller should `continue`).
pub fn handle_file_tree_key(app: &mut App, key: &KeyEvent) -> bool {
    if app.file_tree.is_none() {
        return false;
    }
    match key.code {
        KeyCode::Up => {
            if let Some(state) = app.file_tree.as_mut() {
                state.cursor_up();
            }
            app.needs_redraw = true;
            true
        }
        KeyCode::Down => {
            if let Some(state) = app.file_tree.as_mut() {
                state.cursor_down();
            }
            app.needs_redraw = true;
            true
        }
        KeyCode::Enter => {
            if let Some(state) = app.file_tree.as_mut() {
                if let Some(rel_path) = state.activate() {
                    let path_str = rel_path.to_string_lossy().to_string();
                    app.status_message = Some(format!("Attached @{path_str}"));
                    app.insert_str(&format!("@{} ", path_str));
                } else {
                    app.needs_redraw = true;
                }
            }
            true
        }
        KeyCode::Esc => {
            app.file_tree = None;
            app.status_message = Some("File tree closed".to_string());
            app.needs_redraw = true;
            true
        }
        _ => false,
    }
}
