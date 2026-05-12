//! Note command: manage persistent workspace notes.

use crate::tui::app::App;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::CommandResult;

const USAGE: &str = "/note <text> | /note add <text> | /note list | /note show <n> | /note edit <n> <text> | /note remove <n> | /note clear | /note path";

/// Manage the persistent workspace notes file.
pub fn note(app: &mut App, content: Option<&str>) -> CommandResult {
    let input = match content {
        Some(c) => c.trim(),
        None => {
            return CommandResult::error(format!("Usage: {USAGE}"));
        }
    };

    if input.is_empty() {
        return CommandResult::error("Note content cannot be empty");
    }

    let notes_path = notes_path(app);
    let (command, rest) = split_command(input);

    match command.to_ascii_lowercase().as_str() {
        "add" => append_note_command(&notes_path, rest),
        "list" => list_notes_command(&notes_path),
        "show" => show_note_command(&notes_path, rest),
        "edit" => edit_note_command(&notes_path, rest),
        "remove" | "rm" | "delete" => remove_note_command(&notes_path, rest),
        "clear" => clear_notes_command(&notes_path),
        "path" => CommandResult::message(format!("Notes path: {}", notes_path.display())),
        "help" => CommandResult::message(format!("Usage: {USAGE}")),
        _ => append_note_command(&notes_path, Some(input)),
    }
}

fn notes_path(app: &App) -> PathBuf {
    app.workspace.join(".deepseek").join("notes.md")
}

fn split_command(input: &str) -> (&str, Option<&str>) {
    match input.find(char::is_whitespace) {
        Some(index) => (&input[..index], Some(input[index..].trim())),
        None => (input, None),
    }
}

fn append_note_command(notes_path: &Path, content: Option<&str>) -> CommandResult {
    let Some(note_content) = content.map(str::trim).filter(|content| !content.is_empty()) else {
        return CommandResult::error("Usage: /note add <text>");
    };

    match append_note(notes_path, note_content) {
        Ok(()) => CommandResult::message(format!("Note appended to {}", notes_path.display())),
        Err(e) => CommandResult::error(e),
    }
}

fn list_notes_command(notes_path: &Path) -> CommandResult {
    let notes = match read_notes(notes_path) {
        Ok(notes) => notes,
        Err(e) => return CommandResult::error(e),
    };

    if notes.is_empty() {
        return CommandResult::message(format!("No notes found at {}", notes_path.display()));
    }

    let mut output = format!("Notes in {}:", notes_path.display());
    for (index, note) in notes.iter().enumerate() {
        output.push_str(&format!("\n\n{}. {}", index + 1, note_preview(note)));
    }
    CommandResult::message(output)
}

fn show_note_command(notes_path: &Path, rest: Option<&str>) -> CommandResult {
    let notes = match read_notes(notes_path) {
        Ok(notes) => notes,
        Err(e) => return CommandResult::error(e),
    };
    let index = match parse_note_index(rest, notes.len(), "/note show <n>") {
        Ok(index) => index,
        Err(e) => return CommandResult::error(e),
    };

    CommandResult::message(format!("Note {}:\n\n{}", index + 1, notes[index]))
}

fn edit_note_command(notes_path: &Path, rest: Option<&str>) -> CommandResult {
    let Some(rest) = rest else {
        return CommandResult::error("Usage: /note edit <n> <text>");
    };
    let (index_text, new_content) = match split_command(rest) {
        (index_text, Some(new_content)) if !new_content.trim().is_empty() => {
            (index_text, new_content.trim())
        }
        _ => return CommandResult::error("Usage: /note edit <n> <text>"),
    };

    let mut notes = match read_notes(notes_path) {
        Ok(notes) => notes,
        Err(e) => return CommandResult::error(e),
    };
    let index = match parse_note_index(Some(index_text), notes.len(), "/note edit <n> <text>") {
        Ok(index) => index,
        Err(e) => return CommandResult::error(e),
    };

    notes[index] = new_content.to_string();
    match write_notes(notes_path, &notes) {
        Ok(()) => CommandResult::message(format!(
            "Note {} updated in {}",
            index + 1,
            notes_path.display()
        )),
        Err(e) => CommandResult::error(e),
    }
}

fn remove_note_command(notes_path: &Path, rest: Option<&str>) -> CommandResult {
    let mut notes = match read_notes(notes_path) {
        Ok(notes) => notes,
        Err(e) => return CommandResult::error(e),
    };
    let index = match parse_note_index(rest, notes.len(), "/note remove <n>") {
        Ok(index) => index,
        Err(e) => return CommandResult::error(e),
    };

    notes.remove(index);
    match write_notes(notes_path, &notes) {
        Ok(()) => CommandResult::message(format!(
            "Note {} removed from {}",
            index + 1,
            notes_path.display()
        )),
        Err(e) => CommandResult::error(e),
    }
}

fn clear_notes_command(notes_path: &Path) -> CommandResult {
    match write_notes(notes_path, &[]) {
        Ok(()) => CommandResult::message(format!("Notes cleared in {}", notes_path.display())),
        Err(e) => CommandResult::error(e),
    }
}

fn append_note(notes_path: &Path, note_content: &str) -> Result<(), String> {
    ensure_notes_parent(notes_path)?;

    let mut file = match fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(notes_path)
    {
        Ok(f) => f,
        Err(e) => {
            return Err(format!("Failed to open notes file: {e}"));
        }
    };

    // Write separator and note content
    if let Err(e) = writeln!(file, "\n---\n{}", note_content) {
        return Err(format!("Failed to write note: {e}"));
    }

    Ok(())
}

fn read_notes(notes_path: &Path) -> Result<Vec<String>, String> {
    match fs::read_to_string(notes_path) {
        Ok(content) => Ok(parse_notes(&content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("Failed to read notes file: {e}")),
    }
}

fn write_notes(notes_path: &Path, notes: &[String]) -> Result<(), String> {
    ensure_notes_parent(notes_path)?;
    let content = notes
        .iter()
        .map(|note| format!("---\n{}", note.trim()))
        .collect::<Vec<_>>()
        .join("\n\n");
    fs::write(notes_path, content).map_err(|e| format!("Failed to write notes file: {e}"))
}

fn ensure_notes_parent(notes_path: &Path) -> Result<(), String> {
    if let Some(parent) = notes_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create notes directory: {e}"))?;
    }
    Ok(())
}

fn parse_notes(content: &str) -> Vec<String> {
    let mut notes = Vec::new();
    let mut current = Vec::new();
    let mut saw_separator = false;

    for line in content.lines() {
        if line.trim() == "---" {
            if saw_separator || !current.is_empty() {
                push_note(&mut notes, &current);
                current.clear();
            }
            saw_separator = true;
        } else if saw_separator || !line.trim().is_empty() {
            current.push(line);
        }
    }

    if saw_separator {
        push_note(&mut notes, &current);
    } else {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            notes.push(trimmed.to_string());
        }
    }

    notes
}

fn push_note(notes: &mut Vec<String>, lines: &[&str]) {
    let note = lines.join("\n").trim().to_string();
    if !note.is_empty() {
        notes.push(note);
    }
}

fn note_preview(note: &str) -> String {
    let first_line = note
        .lines()
        .find_map(|line| {
            let trimmed = line.trim();
            (!trimmed.is_empty()).then_some(trimmed)
        })
        .unwrap_or("(empty note)");
    if note.lines().filter(|line| !line.trim().is_empty()).count() > 1 {
        format!("{first_line} ...")
    } else {
        first_line.to_string()
    }
}

fn parse_note_index(rest: Option<&str>, note_count: usize, usage: &str) -> Result<usize, String> {
    let Some(index_text) = rest.map(str::trim).filter(|text| !text.is_empty()) else {
        return Err(format!("Usage: {usage}"));
    };
    let index = index_text
        .parse::<usize>()
        .map_err(|_| format!("Invalid note number: {index_text}"))?;
    if index == 0 || index > note_count {
        return Err(format!(
            "Note number {index} out of range; there are {note_count} note(s)"
        ));
    }
    Ok(index - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_test_app_with_tmpdir(tmpdir: &TempDir) -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: tmpdir.path().to_path_buf(),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: tmpdir.path().join("skills"),
            memory_path: tmpdir.path().join("memory.md"),
            notes_path: tmpdir.path().join("notes.txt"),
            mcp_config_path: tmpdir.path().join("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
    }

    fn notes_path(tmpdir: &TempDir) -> PathBuf {
        tmpdir.path().join(".deepseek").join("notes.md")
    }

    fn message(result: CommandResult) -> String {
        result.message.expect("command message")
    }

    #[test]
    fn test_note_without_content_returns_error() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = note(&mut app, None);
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Usage: /note"));
    }

    #[test]
    fn test_note_with_empty_content_returns_error() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = note(&mut app, Some("   "));
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("cannot be empty"));
    }

    #[test]
    fn test_note_appends_to_file() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = note(&mut app, Some("Test note content"));
        assert!(result.message.is_some());
        let msg = message(result);
        assert!(msg.contains("Note appended to"));

        let notes_path = notes_path(&tmpdir);
        assert!(notes_path.exists());
        let content = std::fs::read_to_string(&notes_path).unwrap();
        assert!(content.contains("Test note content"));
    }

    #[test]
    fn test_note_multiple_appends() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        note(&mut app, Some("First note"));
        note(&mut app, Some("Second note"));

        let notes_path = notes_path(&tmpdir);
        let content = std::fs::read_to_string(&notes_path).unwrap();
        assert!(content.contains("First note"));
        assert!(content.contains("Second note"));
        // Should have two separators
        assert_eq!(content.matches("---").count(), 2);
    }

    #[test]
    fn test_note_list_numbers_entries_without_storing_numbers() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        note(&mut app, Some("Alpha note"));
        note(&mut app, Some("Beta note"));

        let listed = message(note(&mut app, Some("list")));
        assert!(listed.contains("1. Alpha note"));
        assert!(listed.contains("2. Beta note"));

        let content = std::fs::read_to_string(notes_path(&tmpdir)).unwrap();
        assert!(content.contains("Alpha note"));
        assert!(!content.contains("1. Alpha note"));
    }

    #[test]
    fn test_note_show_displays_full_multiline_note() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        note(&mut app, Some("add first line\nsecond line"));

        let shown = message(note(&mut app, Some("show 1")));
        assert!(shown.contains("Note 1:"));
        assert!(shown.contains("first line\nsecond line"));
    }

    #[test]
    fn test_note_edit_updates_numbered_entry() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        note(&mut app, Some("First note"));
        note(&mut app, Some("Second note"));

        let edited = message(note(&mut app, Some("edit 2 Updated second note")));
        assert!(edited.contains("Note 2 updated"));

        let content = std::fs::read_to_string(notes_path(&tmpdir)).unwrap();
        assert!(content.contains("First note"));
        assert!(content.contains("Updated second note"));
        assert!(!content.contains("Second note"));
    }

    #[test]
    fn test_note_remove_renumbers_remaining_entries() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        note(&mut app, Some("First note"));
        note(&mut app, Some("Second note"));
        note(&mut app, Some("Third note"));

        let removed = message(note(&mut app, Some("remove 2")));
        assert!(removed.contains("Note 2 removed"));

        let listed = message(note(&mut app, Some("list")));
        assert!(listed.contains("1. First note"));
        assert!(listed.contains("2. Third note"));
        assert!(!listed.contains("Second note"));
    }

    #[test]
    fn test_note_clear_empties_file() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        note(&mut app, Some("First note"));

        let cleared = message(note(&mut app, Some("clear")));
        assert!(cleared.contains("Notes cleared"));
        assert_eq!(std::fs::read_to_string(notes_path(&tmpdir)).unwrap(), "");
    }

    #[test]
    fn test_note_path_prints_workspace_notes_file() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let path = message(note(&mut app, Some("path")));
        assert!(path.contains(".deepseek"));
        assert!(path.contains("notes.md"));
    }

    #[test]
    fn test_note_rejects_out_of_range_index() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        note(&mut app, Some("Only note"));

        let result = note(&mut app, Some("show 2"));
        assert!(result.message.unwrap().contains("out of range"));
    }

    #[test]
    fn test_parse_notes_handles_plain_text_before_separator() {
        let parsed = parse_notes("plain note\n---\nseparated note");
        assert_eq!(parsed, vec!["plain note", "separated note"]);
    }
}
