//! Keyboard event action handlers extracted from `ui.rs`.
//!
//! Each function handles a focused subset of keyboard input so the
//! main event loop stays lean.

use crossterm::event::{KeyCode, KeyEvent};

use super::app::App;

// ── File-tree key handling ───────────────────────────────────────

/// Handle keyboard input when the file-tree pane is visible.
///
/// Returns `true` when the key was consumed (caller should `continue`).
pub fn handle_file_tree_key(app: &mut App, key: &KeyEvent) -> bool {
    // Guard: do not intercept keys when the file-tree pane is not visible.
    if !app.file_tree_visible {
        return false;
    }

    // Esc closes the tree even when entries are still loading.
    if key.code == KeyCode::Esc && app.file_tree.is_some() {
        app.file_tree = None;
        app.status_message = Some("File tree closed".to_string());
        app.needs_redraw = true;
        return true;
    }

    let Some(file_tree) = app.file_tree.as_mut() else {
        return false;
    };

    match key.code {
        KeyCode::Up => {
            file_tree.cursor_up();
            app.needs_redraw = true;
            true
        }
        KeyCode::Down => {
            file_tree.cursor_down();
            app.needs_redraw = true;
            true
        }
        KeyCode::Enter => {
            if let Some(rel_path) = file_tree.activate() {
                let path_str = rel_path.to_string_lossy().to_string();
                app.status_message = Some(format!("Attached @{path_str}"));
                app.insert_str(&format!("@{path_str} "));
            } else {
                app.needs_redraw = true;
            }
            true
        }
        _ => false,
    }
}
