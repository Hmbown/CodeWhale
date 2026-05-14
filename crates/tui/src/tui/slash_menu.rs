//! Slash-command autocomplete + popup-menu helpers.
//!
//! Extracted from `tui/ui.rs` (P1.2). The on-screen popup itself is rendered
//! by the composer widget; these helpers source the entries, apply a
//! selection, and handle Tab-completion when the popup isn't open.
//!
//! Intentionally separate from `tui::file_mention` even though both surface
//! a similar popup — the trigger characters, ranking, and post-selection
//! behaviour differ enough to keep them apart.

use crate::commands;

use super::app::App;
use super::widgets::SlashMenuEntry;
use super::widgets::slash_completion_hints;

/// Return the slash-menu entries the composer should display, honouring
/// `slash_menu_hidden` (set when the user dismisses the popup with Esc).
pub fn visible_slash_menu_entries(app: &App, limit: usize) -> Vec<SlashMenuEntry> {
    if app.slash_menu_hidden {
        return Vec::new();
    }
    slash_completion_hints(
        &app.input,
        limit,
        &app.cached_skills,
        app.ui_locale,
        Some(&app.workspace),
    )
}

/// Apply the currently-selected slash menu entry to the composer input.
/// Optionally appends a trailing space when the command takes arguments
/// so the user can type the rest without an extra keystroke.
pub fn apply_slash_menu_selection(
    app: &mut App,
    entries: &[SlashMenuEntry],
    append_space: bool,
) -> bool {
    if entries.is_empty() {
        return false;
    }

    let selected_idx = app.slash_menu_selected.min(entries.len().saturating_sub(1));
    let mut command = entries[selected_idx].name.clone();

    if append_space
        && !command.ends_with(' ')
        && !command.contains(char::is_whitespace)
        && let Some(info) = commands::get_command_info(command.trim_start_matches('/'))
        && info.name != "change"
        && (info.usage.contains('<') || info.usage.contains('['))
    {
        command.push(' ');
    }

    app.input = command;
    app.cursor_position = app.input.chars().count();
    app.slash_menu_hidden = false;
    app.status_message = Some(format!("Command selected: {}", app.input.trim_end()));
    true
}

/// Tab-completion for a slash-command-like input. Extends the input to the
/// longest unambiguous prefix; if exactly one command matches, completes it
/// fully (with trailing space). On ambiguity, posts a status hint listing
/// up to five candidates. Also considers skill names as completion candidates.
pub fn try_autocomplete_slash_command(app: &mut App) -> bool {
    if !app.input.starts_with('/') {
        return false;
    }

    let candidates = slash_completion_hints(
        &app.input,
        128,
        &app.cached_skills,
        app.ui_locale,
        Some(&app.workspace),
    )
    .into_iter()
    .map(|entry| entry.name)
    .collect::<Vec<_>>();

    if candidates.is_empty() {
        return false;
    }

    let prefix = app.input.trim_start_matches('/');
    let refs: Vec<&str> = candidates
        .iter()
        .map(|name| name.trim_start_matches('/'))
        .collect();
    let shared = crate::tui::file_mention::longest_common_prefix(&refs);

    if !shared.is_empty() && shared.len() > prefix.len() {
        app.input = format!("/{shared}");
        app.cursor_position = app.input.chars().count();
        app.slash_menu_hidden = false;
        app.status_message = Some(format!("Autocomplete: /{shared}"));
        return true;
    }

    if candidates.len() == 1 {
        let mut completed = candidates[0].clone();
        if !completed.ends_with(' ') {
            completed.push(' ');
        }
        app.input = completed.clone();
        app.cursor_position = completed.chars().count();
        app.slash_menu_hidden = false;
        app.status_message = Some(format!("Command completed: {}", completed.trim_end()));
        return true;
    }

    let preview = candidates
        .iter()
        .take(5)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    app.status_message = Some(format!("Suggestions: {preview}"));
    true
}
