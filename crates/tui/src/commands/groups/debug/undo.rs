//! Undo, retry, edit, and diff commands.

use crate::dependencies::{ExternalTool, Git};
use crate::models::ContentBlock;
use crate::tui::app::{App, AppAction};
use crate::tui::history::HistoryCell;

use super::CommandResult;

/// Remove last message pair (user + assistant).
///
/// This is the old `/undo` behaviour — it removes the most recent
/// user+assistant conversation pair from history and API messages.
/// The new `/undo` first tries to revert workspace files via
/// [`patch_undo`]; if no snapshots are available it falls back to
/// this function.
pub fn undo_conversation(app: &mut App) -> CommandResult {
    // Remove from display history (up to the last user message)
    let mut removed_count = 0;
    while !app.history.is_empty() {
        let last_is_user = matches!(app.history.last(), Some(HistoryCell::User { .. }));
        app.pop_history();
        removed_count += 1;
        if last_is_user {
            break;
        }
    }

    // Remove from API messages
    while let Some(last) = app.api_messages.last() {
        if last.role == "user" {
            app.api_messages.pop();
            break;
        }
        app.api_messages.pop();
    }

    if removed_count > 0 {
        // Keep tool/index mappings consistent after truncation.
        app.tool_cells.clear();
        app.tool_details_by_cell.clear();
        app.exploring_entries.clear();
        app.ignored_tool_calls.clear();
        app.mark_history_updated();
        CommandResult::message(format!("Removed {removed_count} message(s)"))
    } else {
        CommandResult::message("Nothing to undo")
    }
}

pub(crate) fn prune_undone_tool_context(app: &mut App, tool_id: &str) {
    if let Some(history_idx) = app.tool_cells.get(tool_id).copied() {
        app.truncate_history_to(history_idx);
    }

    let Some((msg_idx, block_idx)) =
        app.api_messages
            .iter()
            .enumerate()
            .find_map(|(msg_idx, msg)| {
                msg.content
                    .iter()
                    .position(
                        |block| matches!(block, ContentBlock::ToolUse { id, .. } if id == tool_id),
                    )
                    .map(|block_idx| (msg_idx, block_idx))
            })
    else {
        return;
    };

    let kept_blocks = app.api_messages[msg_idx].content[..block_idx].to_vec();
    let kept_tool_ids: std::collections::HashSet<String> = kept_blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();

    if kept_blocks.is_empty() {
        app.api_messages.truncate(msg_idx);
        return;
    }
    let preserved_tool_results: Vec<_> =
        app.api_messages
            .iter()
            .skip(msg_idx + 1)
            .take_while(|msg| {
                msg.role == "user"
                    && !msg.content.is_empty()
                    && msg
                        .content
                        .iter()
                        .all(|block| tool_result_id(block).is_some())
            })
            .filter(|msg| {
                msg.role == "user"
                    && !msg.content.is_empty()
                    && msg.content.iter().all(|block| {
                        tool_result_id(block).is_some_and(|id| kept_tool_ids.contains(id))
                    })
            })
            .cloned()
            .collect();
    app.api_messages.truncate(msg_idx + 1);
    app.api_messages[msg_idx].content = kept_blocks;
    app.api_messages.extend(preserved_tool_results);
}

fn prune_undone_turn_context(app: &mut App) {
    if let Some(history_idx) = app
        .history
        .iter()
        .rposition(|cell| matches!(cell, HistoryCell::User { .. }))
    {
        app.truncate_history_to(history_idx);
    }

    if let Some(api_idx) = app.api_messages.iter().rposition(|msg| msg.role == "user") {
        app.api_messages.truncate(api_idx);
    }
}

fn tool_result_id(block: &ContentBlock) -> Option<&String> {
    match block {
        ContentBlock::ToolResult { tool_use_id, .. }
        | ContentBlock::ToolSearchToolResult { tool_use_id, .. }
        | ContentBlock::CodeExecutionToolResult { tool_use_id, .. } => Some(tool_use_id),
        _ => None,
    }
}

/// Revert the most recent write tool (apply_patch/edit_file/write_file) or turn.
///
/// Opens the side-git snapshot repo and finds the most recent snapshot,
/// preferring per-tool snapshots (`tool:*`) over pre-turn snapshots
/// (`pre-turn:*`). Restores files from that snapshot and shows a diff
/// summary. Falls back to conversation undo when no snapshots exist.
///
/// Posts a `HistoryCell::System` entry so the user can see what was
/// reverted in the transcript.
pub fn patch_undo(app: &mut App) -> CommandResult {
    let workspace = app.workspace.clone();

    let repo = match crate::snapshot::SnapshotRepo::open_or_init(&workspace) {
        Ok(r) => r,
        Err(e) => {
            return CommandResult::error(format!(
                "Snapshot repo unavailable for {}: {e}",
                workspace.display(),
            ));
        }
    };

    let snapshots = match repo.list(20) {
        Ok(s) => s,
        Err(e) => {
            return CommandResult::error(format!("Failed to list snapshots: {e}"));
        }
    };

    if snapshots.is_empty() {
        return CommandResult::message("No snapshots found to undo — nothing to revert.");
    }

    // Prefer the newest revertable `tool:` / `pre-turn:` snapshot whose
    // tracked content differs from the current workspace. This lets
    // repeated `/undo` walk back through older snapshots instead of
    // restoring the same no-op target forever.
    let target = snapshots
        .iter()
        .filter(|s| s.label.starts_with("tool:") || s.label.starts_with("pre-turn:"))
        .find(|s| match repo.work_tree_matches_snapshot(&s.id) {
            Ok(matches) => !matches,
            Err(_) => true,
        });

    let Some(target) = target else {
        return CommandResult::message(
            "No older tool or pre-turn snapshots differ from the current workspace — nothing to revert.",
        );
    };

    if let Err(e) = repo.restore(&target.id) {
        return CommandResult::error(format!("Restore failed: {e}"));
    }

    if let Some(tool_id) = target.label.strip_prefix("tool:") {
        prune_undone_tool_context(app, tool_id);
    } else if target.label.starts_with("pre-turn:") {
        prune_undone_turn_context(app);
    }

    // Show diff stat so the user knows what changed.
    let diff_stat = Git::command()
        .map(|mut git| {
            git.args(["diff", "--stat"])
                .current_dir(&workspace)
                .output()
                .ok()
                .and_then(|o| {
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if s.is_empty() { None } else { Some(s) }
                })
        })
        .unwrap_or(None);

    let short = &target.id.as_str()[..target.id.as_str().len().min(8)];
    let summary = match diff_stat {
        Some(ref stat) => {
            format!(
                "Restored snapshot '{}' ({}). Files affected:\n{stat}",
                target.label, short
            )
        }
        None => {
            format!(
                "Restored snapshot '{}' ({}). No diff changes detected.",
                target.label, short
            )
        }
    };

    // Post a system cell so the reverted state is visible in the transcript.
    app.push_history_cell(HistoryCell::System {
        content: format!(
            "/undo reverted workspace to snapshot '{}' ({})",
            target.label, short
        ),
    });

    CommandResult::with_message_and_action(
        summary,
        AppAction::SyncSession {
            session_id: app.current_session_id.clone(),
            messages: app.api_messages.clone(),
            system_prompt: app.system_prompt.clone(),
            model: app.model.clone(),
            workspace: app.workspace.clone(),
            mode: app.mode,
        },
    )
}

/// Load the last user message back into the composer for editing.
///
/// Searches `app.history` for the most recent `HistoryCell::User`, copies its
/// content into `app.input`, and positions the cursor at the end so the user
/// can edit and press Enter to resubmit. The original exchange stays visible
/// in the transcript.
pub fn edit(app: &mut App) -> CommandResult {
    let last_user = app.history.iter().rev().find_map(|cell| match cell {
        HistoryCell::User { content } => Some(content.clone()),
        _ => None,
    });

    match last_user {
        Some(content) => {
            app.input = content;
            app.cursor_position = app.input.chars().count();
            app.edit_in_progress = true;
            CommandResult::message(
                "Last message loaded into composer — edit and press Enter to resubmit",
            )
        }
        None => CommandResult::message("No previous message to edit"),
    }
}

/// Show git diff output since session start.
///
/// Runs `git diff --stat` and `git diff --name-only` in the workspace
/// directory. Displays which files have changed and a stat summary. If no
/// changes exist or git fails, returns an appropriate message.
pub fn diff(app: &mut App) -> CommandResult {
    let workspace = app.workspace.clone();

    let Some(mut name_only_cmd) = Git::command() else {
        return CommandResult::error("git not found on PATH");
    };
    let Some(mut stat_cmd) = Git::command() else {
        return CommandResult::error("git not found on PATH");
    };
    let name_only_output = name_only_cmd
        .args(["diff", "--name-only"])
        .current_dir(&workspace)
        .output();
    let stat_output = stat_cmd
        .args(["diff", "--stat"])
        .current_dir(&workspace)
        .output();

    match (name_only_output, stat_output) {
        (Ok(name_only), Ok(stat)) => {
            let name_stdout = String::from_utf8_lossy(&name_only.stdout);
            let stat_stdout = String::from_utf8_lossy(&stat.stdout);

            if name_stdout.trim().is_empty() {
                return CommandResult::message("No changes since session start");
            }

            let files: Vec<&str> = name_stdout.lines().filter(|l| !l.is_empty()).collect();
            let file_count = files.len();
            let file_list = files.join("\n");

            // Detect rename entries (e.g. "foo -> bar") and exclude them
            // from the file-count header so the user sees only actual
            // modifications.
            let renamed_count = files.iter().filter(|f| f.contains(" -> ")).count();
            let summary = if renamed_count > 0 {
                format!("Changed files ({file_count}, {renamed_count} renamed):\n{file_list}")
            } else {
                format!("Changed files ({file_count}):\n{file_list}")
            };

            let stat_str = stat_stdout.trim();
            let mut message = summary;
            if !stat_str.is_empty() {
                message.push_str("\n\n── Stat ──\n");
                message.push_str(stat_str);
            }
            CommandResult::message(message)
        }
        (Err(e), _) | (_, Err(e)) => {
            CommandResult::message(format!("Git diff failed — is this a git repository?\n{e}"))
        }
    }
}

/// Retry last request - remove last exchange and re-send the user's message
pub fn retry(app: &mut App) -> CommandResult {
    let last_user_input = app.history.iter().rev().find_map(|cell| match cell {
        HistoryCell::User { content } => Some(content.clone()),
        _ => None,
    });

    match last_user_input {
        Some(input) => {
            undo_conversation(app);
            let display_input = if input.len() > 50 {
                let truncate_at = input
                    .char_indices()
                    .take_while(|(i, _)| *i <= 50)
                    .last()
                    .map_or(0, |(i, _)| i);
                format!("{}...", &input[..truncate_at])
            } else {
                input.clone()
            };
            CommandResult::with_message_and_action(
                format!("Retrying: {display_input}"),
                AppAction::SendMessage(input),
            )
        }
        None => CommandResult::error("No previous request to retry"),
    }
}
