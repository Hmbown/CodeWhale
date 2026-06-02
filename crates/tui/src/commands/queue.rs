//! Queue commands: queue list/edit/drop/clear

use crate::tui::app::App;

use super::CommandResult;

const PREVIEW_LIMIT: usize = 120;

pub fn queue(app: &mut App, args: Option<&str>) -> CommandResult {
    let arg = args.unwrap_or("").trim();
    if arg.is_empty() || arg.eq_ignore_ascii_case("list") {
        return list_queue(app);
    }

    let mut parts = arg.split_whitespace();
    let action = parts.next().unwrap_or("").to_lowercase();

    match action.as_str() {
        "edit" => edit_queue(app, parts.next()),
        "drop" | "remove" | "rm" => drop_queue(app, parts.next()),
        "clear" => clear_queue(app),
        _ => CommandResult::error("Usage: /queue [list|edit <n>|drop <n>|clear]"),
    }
}

fn list_queue(app: &mut App) -> CommandResult {
    let mut lines = Vec::new();
    let queued = app.queued_message_count();

    if let Some(draft) = app.queued_draft.as_ref() {
        lines.push("Editing queued message:".to_string());
        lines.push(format!("- {}", truncate_preview(&draft.display)));
    }

    if queued == 0 {
        if lines.is_empty() {
            return CommandResult::message("No queued messages");
        }
        return CommandResult::message(lines.join("\n"));
    }

    lines.push(format!("Queued messages ({queued}):"));
    for (idx, message) in app.queued_messages.iter().enumerate() {
        lines.push(format!(
            "{}. {}",
            idx + 1,
            truncate_preview(&message.display)
        ));
    }

    lines.push("Tip: /queue edit <n> to edit, /queue drop <n> to remove".to_string());

    CommandResult::message(lines.join("\n"))
}

fn edit_queue(app: &mut App, index: Option<&str>) -> CommandResult {
    if app.queued_draft.is_some() {
        return CommandResult::error(
            "Already editing a queued message. Send it or /queue clear to discard.",
        );
    }
    let index = match parse_index(index) {
        Ok(index) => index,
        Err(err) => return CommandResult::error(err),
    };

    let Some(message) = app.remove_queued_message(index) else {
        return CommandResult::error("Queued message not found");
    };

    app.input = message.display.clone();
    app.cursor_position = app.input.len();
    app.queued_draft = Some(message);
    app.status_message = Some(format!("Editing queued message {}", index + 1));

    CommandResult::message(format!(
        "Editing queued message {} (press Enter to re-queue/send)",
        index + 1
    ))
}

fn drop_queue(app: &mut App, index: Option<&str>) -> CommandResult {
    let index = match parse_index(index) {
        Ok(index) => index,
        Err(err) => return CommandResult::error(err),
    };

    if app.remove_queued_message(index).is_none() {
        return CommandResult::error("Queued message not found");
    }

    CommandResult::message(format!("Dropped queued message {}", index + 1))
}

fn clear_queue(app: &mut App) -> CommandResult {
    let queued = app.queued_message_count();
    let had_draft = app.queued_draft.take().is_some();
    app.queued_messages.clear();
    if queued == 0 && !had_draft {
        return CommandResult::message("Queue already empty");
    }

    CommandResult::message("Queue cleared")
}

fn parse_index(input: Option<&str>) -> Result<usize, &'static str> {
    let Some(input) = input else {
        return Err("Missing index. Usage: /queue edit <n> or /queue drop <n>");
    };
    let raw = input
        .parse::<usize>()
        .map_err(|_| "Index must be a positive number")?;
    if raw == 0 {
        return Err("Index must be >= 1");
    }
    Ok(raw - 1)
}

fn truncate_preview(text: &str) -> String {
    if text.chars().count() <= PREVIEW_LIMIT {
        return text.to_string();
    }
    let mut out = String::new();
    for ch in text.chars().take(PREVIEW_LIMIT.saturating_sub(3)) {
        out.push(ch);
    }
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, QueuedMessage, TuiOptions};
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
    fn test_queue_list_empty() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = queue(&mut app, None);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("No queued messages"));
    }

    #[test]
    fn test_queue_list_with_messages() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.queued_messages
            .push_back(QueuedMessage::new("First message".to_string(), None));
        app.queued_messages
            .push_back(QueuedMessage::new("Second message".to_string(), None));
        let result = queue(&mut app, Some("list"));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Queued messages (2)"));
        assert!(msg.contains("1. First message"));
        assert!(msg.contains("2. Second message"));
    }

    #[test]
    fn test_queue_edit_missing_index() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.queued_messages
            .push_back(QueuedMessage::new("Test".to_string(), None));
        let result = queue(&mut app, Some("edit"));
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Missing index"));
    }

    #[test]
    fn test_queue_edit_invalid_index() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = queue(&mut app, Some("edit abc"));
        assert!(result.message.is_some());
        assert!(
            result
                .message
                .unwrap()
                .contains("must be a positive number")
        );
    }

    #[test]
    fn test_queue_edit_not_found() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = queue(&mut app, Some("edit 1"));
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("not found"));
    }

    #[test]
    fn test_queue_edit_already_editing() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.queued_messages
            .push_back(QueuedMessage::new("First".to_string(), None));
        app.queued_messages
            .push_back(QueuedMessage::new("Second".to_string(), None));
        // Start editing
        queue(&mut app, Some("edit 1"));
        // Try to edit another
        let result = queue(&mut app, Some("edit 2"));
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Already editing"));
    }

    #[test]
    fn test_queue_edit_success() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.queued_messages
            .push_back(QueuedMessage::new("Original message".to_string(), None));
        let result = queue(&mut app, Some("edit 1"));
        assert!(result.message.is_some());
        assert_eq!(app.input, "Original message");
        assert_eq!(app.cursor_position, app.input.len());
        assert!(app.queued_draft.is_some());
    }

    #[test]
    fn test_queue_drop_success() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.queued_messages
            .push_back(QueuedMessage::new("To drop".to_string(), None));
        let initial_count = app.queued_messages.len();
        let result = queue(&mut app, Some("drop 1"));
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Dropped queued message"));
        assert_eq!(app.queued_messages.len(), initial_count - 1);
    }

    #[test]
    fn test_queue_clear() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.queued_messages
            .push_back(QueuedMessage::new("Message 1".to_string(), None));
        app.queued_messages
            .push_back(QueuedMessage::new("Message 2".to_string(), None));
        let result = queue(&mut app, Some("clear"));
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Queue cleared"));
        assert!(app.queued_messages.is_empty());
    }

    #[test]
    fn test_queue_clear_already_empty() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = queue(&mut app, Some("clear"));
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Queue already empty"));
    }

    #[test]
    fn test_truncate_preview_short_text() {
        let result = truncate_preview("Short text");
        assert_eq!(result, "Short text");
    }

    #[test]
    fn test_truncate_preview_long_text() {
        let long_text = "x".repeat(200);
        let result = truncate_preview(&long_text);
        assert!(result.len() <= PREVIEW_LIMIT + 3);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_preview_unicode() {
        let text = "Hello 世界 🌍";
        let result = truncate_preview(text);
        assert_eq!(result, text);
    }
}
