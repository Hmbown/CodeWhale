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

use super::app::{App, looks_like_slash_command_input};
use super::widgets::SlashMenuEntry;
use super::widgets::slash_completion_hints;

/// Return the slash-menu entries the composer should display, honouring
/// `slash_menu_hidden` (set when the user dismisses the popup with Esc).
pub fn visible_slash_menu_entries(app: &App, limit: usize) -> Vec<SlashMenuEntry> {
    if app.slash_menu_hidden {
        return Vec::new();
    }
    if let Some((_byte_start, trigger, partial)) =
        partial_inline_skill_mention_at_cursor(&app.input, app.cursor_position)
    {
        return skill_mention_entries(trigger, &partial, limit, &app.cached_skills);
    }
    slash_completion_hints(
        &app.input,
        limit,
        &app.cached_skills,
        app.ui_locale,
        Some(&app.workspace),
        app.api_provider,
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
    let selected = &entries[selected_idx];

    if selected.is_skill
        && let Some((byte_start, trigger, partial)) =
            partial_inline_skill_mention_at_cursor(&app.input, app.cursor_position)
        && let Some(skill_name) = skill_name_from_menu_entry(selected)
    {
        replace_inline_skill_mention(app, byte_start, trigger, &partial, &skill_name);
        app.slash_menu_hidden = false;
        app.status_message = Some(format!("Skill selected: {trigger}{skill_name}"));
        return true;
    }

    let mut command = selected.name.clone();

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

/// Return the `/<skill>` token under the cursor when it is used as an inline
/// mention inside a normal message. A slash at the start of the composer, even
/// after leading whitespace, remains reserved for slash commands.
pub(crate) fn partial_inline_skill_mention_at_cursor(
    input: &str,
    cursor_chars: usize,
) -> Option<(usize, char, String)> {
    if looks_like_slash_command_input(input) {
        return None;
    }

    let chars: Vec<char> = input.chars().collect();
    if cursor_chars > chars.len() {
        return None;
    }

    let mut start_chars = cursor_chars;
    let mut trigger = None;
    while start_chars > 0 {
        let prev = chars[start_chars - 1];
        if matches!(prev, '/' | '$') {
            trigger = Some(prev);
            start_chars -= 1;
            break;
        }
        if prev.is_whitespace() {
            return None;
        }
        start_chars -= 1;
    }

    let trigger = trigger?;
    if start_chars == cursor_chars || chars.get(start_chars) != Some(&trigger) {
        return None;
    }
    if !is_inline_skill_mention_start(&chars, start_chars) {
        return None;
    }

    let byte_start: usize = chars[..start_chars].iter().map(|c| c.len_utf8()).sum();
    if input[..byte_start].trim().is_empty() {
        return None;
    }

    let mut end_chars = start_chars + 1;
    while end_chars < chars.len() && !chars[end_chars].is_whitespace() {
        end_chars += 1;
    }
    let partial: String = chars[start_chars + 1..end_chars].iter().collect();
    if partial.contains('/') || partial.contains('$') {
        return None;
    }

    Some((byte_start, trigger, partial))
}

fn is_inline_skill_mention_start(chars: &[char], idx: usize) -> bool {
    if idx == 0 {
        return false;
    }
    chars
        .get(idx.saturating_sub(1))
        .is_some_and(|ch| ch.is_whitespace() || matches!(ch, '(' | '[' | '{' | '<' | '"' | '\''))
}

fn skill_mention_entries(
    trigger: char,
    partial: &str,
    limit: usize,
    cached_skills: &[(String, String)],
) -> Vec<SlashMenuEntry> {
    if limit == 0 {
        return Vec::new();
    }
    let partial_lower = partial.to_ascii_lowercase();
    let mut entries = cached_skills
        .iter()
        .filter(|(skill_name, _)| skill_name.to_ascii_lowercase().starts_with(&partial_lower))
        .map(|(skill_name, skill_desc)| SlashMenuEntry {
            name: format!("{trigger}{skill_name}"),
            description: skill_desc.clone(),
            is_skill: true,
            alias_hint: None,
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries.dedup_by(|a, b| a.name == b.name);
    entries.into_iter().take(limit).collect()
}

fn skill_name_from_menu_entry(entry: &SlashMenuEntry) -> Option<String> {
    if !entry.is_skill {
        return None;
    }
    if let Some(name) = entry.name.strip_prefix("/skill ") {
        return Some(name.trim().to_string());
    }
    if let Some(name) = entry.name.strip_prefix('$') {
        return Some(name.trim().to_string());
    }
    entry
        .name
        .strip_prefix('/')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
}

fn replace_inline_skill_mention(
    app: &mut App,
    byte_start: usize,
    trigger: char,
    partial: &str,
    skill_name: &str,
) {
    let original_token_len = trigger.len_utf8() + partial.len();
    let original_token_end = byte_start + original_token_len;
    let mut new_input = String::with_capacity(
        app.input.len() - original_token_len + trigger.len_utf8() + skill_name.len(),
    );
    new_input.push_str(&app.input[..byte_start]);
    new_input.push(trigger);
    new_input.push_str(skill_name);
    if original_token_end < app.input.len() {
        new_input.push_str(&app.input[original_token_end..]);
    }
    let new_cursor_chars = app.input[..byte_start].chars().count() + 1 + skill_name.chars().count();
    app.input = new_input;
    app.cursor_position = new_cursor_chars;
}

/// Tab-completion for a slash-command-like input. Extends the input to the
/// longest unambiguous prefix; if exactly one command matches, completes it
/// fully (with trailing space). On ambiguity, posts a status hint listing
/// up to five candidates. Also considers skill names as completion candidates.
pub fn try_autocomplete_slash_command(app: &mut App) -> bool {
    if !looks_like_slash_command_input(&app.input) {
        return false;
    }

    let candidates = slash_completion_hints(
        &app.input,
        128,
        &app.cached_skills,
        app.ui_locale,
        Some(&app.workspace),
        app.api_provider,
    )
    .into_iter()
    .map(|entry| entry.name)
    .collect::<Vec<_>>();

    if candidates.is_empty() {
        return false;
    }

    let trimmed = app.input.trim_start();
    let trigger = if trimmed.starts_with('$') { '$' } else { '/' };
    let prefix = trimmed.strip_prefix(trigger).unwrap_or(trimmed);
    let refs: Vec<&str> = candidates
        .iter()
        .map(|name| name.trim_start_matches(trigger))
        .collect();
    let shared = crate::tui::file_mention::longest_common_prefix(&refs);

    if !shared.is_empty() && shared.len() > prefix.len() {
        app.input = format!("{trigger}{shared}");
        app.cursor_position = app.input.chars().count();
        app.slash_menu_hidden = false;
        app.status_message = Some(format!("Autocomplete: {trigger}{shared}"));
        return true;
    }

    if candidates.len() == 1 {
        let mut completed = candidates[0].clone();
        if !completed.starts_with('$') && !completed.ends_with(' ') {
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
