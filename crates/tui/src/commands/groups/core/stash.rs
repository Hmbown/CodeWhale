//! `/stash` slash command — list / pop parked composer drafts (#440).
//!
//! See `crates/tui/src/composer_stash.rs` for the on-disk format
//! and persistence rules. The slash command is the user-facing
//! surface; Ctrl+S in the composer is the corresponding push entry
//! point.

use crate::composer_stash;
use crate::tui::app::App;

use super::CommandResult;

/// Top-level dispatch for `/stash`. Subcommands:
///
/// * `/stash`        — same as `/stash list`.
/// * `/stash list`   — show parked drafts, oldest first.
/// * `/stash pop`    — restore the most recently parked draft into
///   the composer; the popped entry is removed from disk.
/// * `/stash clear`  — wipe the entire stash file. Reports how many
///   entries were dropped so the user knows what they deleted.
pub fn stash(app: &mut App, arg: Option<&str>) -> CommandResult {
    let sub = arg.map(str::trim).unwrap_or("list").to_ascii_lowercase();
    match sub.as_str() {
        "" | "list" | "ls" | "show" => list(),
        "pop" | "restore" => pop(app),
        "clear" | "wipe" | "drop" => clear(),
        other => CommandResult::error(format!(
            "unknown subcommand `{other}`. Try `/stash list`, `/stash pop`, or `/stash clear`."
        )),
    }
}

fn list() -> CommandResult {
    let entries = composer_stash::load_stash();
    if entries.is_empty() {
        return CommandResult::message(
            "Stash empty. Press Ctrl+S in the composer to park the current draft.",
        );
    }
    let mut out = String::new();
    out.push_str(&format!("{} parked draft(s):\n\n", entries.len()));
    for (idx, entry) in entries.iter().enumerate() {
        let preview = preview_first_line(&entry.text, 80);
        let ts = if entry.ts.is_empty() {
            "(no ts)".to_string()
        } else {
            entry.ts.clone()
        };
        out.push_str(&format!("  {idx}. [{ts}] {preview}\n"));
    }
    out.push_str("\nUse `/stash pop` to restore the most recent draft.");
    CommandResult::message(out)
}

fn clear() -> CommandResult {
    match composer_stash::clear_stash() {
        Ok(0) => CommandResult::message("Stash already empty — nothing to clear."),
        Ok(n) => CommandResult::message(format!("Cleared {n} parked draft(s) from the stash.")),
        Err(err) => CommandResult::error(format!("Failed to clear stash: {err}")),
    }
}

fn pop(app: &mut App) -> CommandResult {
    match composer_stash::pop_stash() {
        Some(entry) => {
            // Replace the current composer contents with the popped
            // draft. We don't merge — replacing is the predictable
            // behaviour and matches the "restore the parked draft"
            // mental model. Mirror the queue-edit pattern for the
            // cursor reset.
            app.input = entry.text.clone();
            app.cursor_position = app.input.len();
            let preview = preview_first_line(&entry.text, 60);
            // Tell the user how many drafts remain so they can plan
            // whether to keep popping or move on. Matches the
            // confirmation pattern used by the queue surface.
            let remaining = composer_stash::load_stash().len();
            let suffix = match remaining {
                0 => " (stash now empty)".to_string(),
                1 => " (1 more parked)".to_string(),
                n => format!(" ({n} more parked)"),
            };
            CommandResult::message(format!("Restored stashed draft: {preview}{suffix}"))
        }
        None => CommandResult::message("Stash empty — nothing to pop."),
    }
}

/// Take a one-line preview of `text`, capped at `max_chars`.
/// Multi-line drafts get a single-line summary so the listing
/// stays scannable.
fn preview_first_line(text: &str, max_chars: usize) -> String {
    let head = text.lines().next().unwrap_or("").trim();
    if head.chars().count() <= max_chars {
        return head.to_string();
    }
    let mut out: String = head.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_first_line_truncates_to_cap() {
        let body = "x".repeat(200);
        let p = preview_first_line(&body, 10);
        assert_eq!(p.chars().count(), 10);
        assert!(p.ends_with('…'));
    }

    #[test]
    fn preview_first_line_keeps_short_input_intact() {
        assert_eq!(preview_first_line("short", 50), "short");
    }

    #[test]
    fn preview_first_line_only_uses_first_line_of_multiline() {
        let body = "first line of the draft\nsecond line that's longer\nthird";
        assert_eq!(preview_first_line(body, 80), "first line of the draft");
    }

    #[test]
    fn preview_first_line_handles_empty_input() {
        assert_eq!(preview_first_line("", 50), "");
        assert_eq!(preview_first_line("   ", 50), "");
    }
}
