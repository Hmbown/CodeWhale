//! Anchor command: keep critical facts across compaction.
//!
//! Unlike `/note` (active lookup), anchors are passive. They are automatically
//! re-injected into context after every compaction cycle. Use anchors to
//! preserve invariants like "This API's status field is unreliable" or
//! ".ssh/ must never be touched".

use crate::tui::app::App;
use std::fs;
use std::io::Write;

use crate::commands::CommandResult;

const USAGE: &str = "/anchor <text> | /anchor list | /anchor remove <n>";

/// Handle the `/anchor` command with subcommands:
/// - `/anchor <text>` — add a new anchor
/// - `/anchor list` — list all anchors
/// - `/anchor remove <n>` — remove anchor by 1-based index
pub fn anchor(app: &mut App, content: Option<&str>) -> CommandResult {
    let input = match content {
        Some(c) => c.trim(),
        None => {
            return CommandResult::error(format!("Usage: {USAGE}"));
        }
    };

    if input.is_empty() {
        return CommandResult::error(format!("Usage: {USAGE}"));
    }

    // Parse subcommands.
    if input.eq_ignore_ascii_case("list") {
        return list_anchors(app);
    }

    if let Some(rest) = input
        .strip_prefix("remove ")
        .or_else(|| input.strip_prefix("rm "))
        .or_else(|| input.strip_prefix("delete "))
    {
        return remove_anchor(app, rest.trim());
    }

    // Default: add a new anchor.
    add_anchor(app, input)
}

fn anchors_path(app: &App) -> std::path::PathBuf {
    let primary = app.workspace.join(".codewhale").join("anchors.md");
    if primary.exists() {
        return primary;
    }
    app.workspace.join(".deepseek").join("anchors.md")
}

/// Read and split anchors from the file. Each anchor is separated by "\n---\n".
fn read_anchors(app: &App) -> Vec<String> {
    let path = anchors_path(app);
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    content
        .split("\n---\n")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Write anchors back to the file, joined by "\n---\n".
fn write_anchors(app: &App, anchors: &[String]) -> Result<(), String> {
    let path = anchors_path(app);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create anchors directory: {e}"))?;
    }

    let content = anchors.join("\n---\n");
    fs::write(&path, content).map_err(|e| format!("Failed to write anchors file: {e}"))
}

fn add_anchor(app: &mut App, text: &str) -> CommandResult {
    let path = anchors_path(app);

    // Ensure parent directory exists.
    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        return CommandResult::error(format!("Failed to create anchors directory: {e}"));
    }

    // Append to anchors file.
    let mut file = match fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            return CommandResult::error(format!("Failed to open anchors file: {e}"));
        }
    };

    // Write separator and anchor content.
    if let Err(e) = writeln!(file, "\n---\n{text}") {
        return CommandResult::error(format!("Failed to write anchor: {e}"));
    }

    CommandResult::message(format!(
        "Anchor pinned. It will be auto-injected into context after each compaction.\n\
         Stored in: {}",
        path.display()
    ))
}

fn list_anchors(app: &App) -> CommandResult {
    let anchors = read_anchors(app);

    if anchors.is_empty() {
        return CommandResult::message(
            "No anchors set. Use /anchor <text> to pin a fact that survives compaction.",
        );
    }

    let mut output = format!("Pinned anchors ({} total):\n", anchors.len());
    for (i, anchor) in anchors.iter().enumerate() {
        output.push_str(&format!("\n  {}. {}", i + 1, anchor));
    }
    output.push_str("\n\nUse /anchor remove <n> to remove an anchor.");

    CommandResult::message(output)
}

fn remove_anchor(app: &mut App, index_str: &str) -> CommandResult {
    let index: usize = match index_str.parse() {
        Ok(n) if n >= 1 => n,
        _ => {
            return CommandResult::error(
                "Invalid index. Use /anchor list to see anchor numbers, then /anchor remove <n>.",
            );
        }
    };

    let mut anchors = read_anchors(app);

    if index > anchors.len() {
        return CommandResult::error(format!(
            "Anchor #{index} does not exist. You have {} anchor(s). Use /anchor list to see them.",
            anchors.len()
        ));
    }

    let removed = anchors.remove(index - 1);
    if let Err(e) = write_anchors(app, &anchors) {
        return CommandResult::error(e);
    }

    CommandResult::message(format!("Removed anchor #{index}: {removed}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
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

    #[test]
    fn test_anchor_without_content_returns_error() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = anchor(&mut app, None);
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("Usage:"));
    }

    #[test]
    fn test_anchor_with_empty_content_returns_error() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = anchor(&mut app, Some("   "));
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("Usage:"));
    }

    #[test]
    fn test_anchor_add() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = anchor(&mut app, Some("API status field is unreliable"));
        assert!(!result.is_error);
        assert!(result.message.unwrap().contains("Anchor pinned"));

        let path = tmpdir.path().join(".deepseek").join("anchors.md");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("API status field is unreliable"));
    }

    #[test]
    fn test_anchor_list_empty() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = anchor(&mut app, Some("list"));
        assert!(!result.is_error);
        assert!(result.message.unwrap().contains("No anchors set"));
    }

    #[test]
    fn test_anchor_list_with_items() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        anchor(&mut app, Some("First anchor"));
        anchor(&mut app, Some("Second anchor"));

        let result = anchor(&mut app, Some("list"));
        let msg = result.message.unwrap();
        assert!(msg.contains("2 total"));
        assert!(msg.contains("1. First anchor"));
        assert!(msg.contains("2. Second anchor"));
    }

    #[test]
    fn test_anchor_remove() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        anchor(&mut app, Some("First anchor"));
        anchor(&mut app, Some("Second anchor"));

        let result = anchor(&mut app, Some("remove 1"));
        assert!(!result.is_error);
        assert!(result.message.unwrap().contains("Removed anchor #1"));

        let result = anchor(&mut app, Some("list"));
        let msg = result.message.unwrap();
        assert!(msg.contains("1 total"));
        assert!(msg.contains("Second anchor"));
        assert!(!msg.contains("First anchor"));
    }

    #[test]
    fn test_anchor_remove_invalid_index() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        anchor(&mut app, Some("Only anchor"));

        let result = anchor(&mut app, Some("remove 5"));
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("does not exist"));
    }

    #[test]
    fn test_anchor_remove_non_numeric() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = anchor(&mut app, Some("remove abc"));
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("Invalid index"));
    }
}
