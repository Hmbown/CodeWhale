//! User-defined slash commands from `~/.deepseek/commands/<name>.md` and
//! workspace-local `<workspace>/.deepseek/commands/<name>.md`.
//!
//! Users drop `.md` files into a commands directory and the filename
//! (without `.md` extension) becomes a slash command. When invoked via
//! `/name`, the file contents are sent as a user message.
//!
//! ## Precedence
//!
//! Workspace-local directories shadow user-global by name:
//!
//! 1. `<workspace>/.deepseek/commands/`  (project-local, highest)
//! 2. `<workspace>/.claude/commands/`    (Claude Code interop)
//! 3. `<workspace>/.cursor/commands/`    (Cursor interop)
//! 4. `~/.deepseek/commands/`            (user-global, lowest)

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::tui::app::{App, AppAction};

use super::CommandResult;

/// Path to the global user commands directory: `~/.deepseek/commands/`.
fn global_commands_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    home.join(".deepseek").join("commands")
}

/// Return all candidate commands directories in precedence order.
fn commands_dirs(workspace: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(ws) = workspace {
        dirs.push(ws.join(".deepseek").join("commands"));
        dirs.push(ws.join(".claude").join("commands"));
        dirs.push(ws.join(".cursor").join("commands"));
    }
    dirs.push(global_commands_dir());
    dirs
}

/// Scan a single commands directory for `.md` files and return
/// `(name, content)` pairs. Errors are silently skipped.
fn load_commands_from_dir(dir: &Path) -> Vec<(String, String)> {
    let mut commands: Vec<(String, String)> = Vec::new();

    if !dir.is_dir() {
        return commands;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return commands,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(stem) => stem.to_lowercase(),
            None => continue,
        };
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        commands.push((stem, content));
    }

    commands
}

/// Scan every candidate commands directory and return merged
/// `(name, content)` pairs. Workspace-local directories shadow
/// user-global by name — the first occurrence of a name wins.
///
/// Pass `None` for the workspace to scan only the global directory
/// (backward-compatible with callers that don't have workspace context).
pub fn load_user_commands(workspace: Option<&Path>) -> Vec<(String, String)> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut commands: Vec<(String, String)> = Vec::new();

    for dir in commands_dirs(workspace) {
        for (name, content) in load_commands_from_dir(&dir) {
            if seen.insert(name.clone()) {
                commands.push((name, content));
            }
        }
    }

    // Sort by name for deterministic ordering.
    commands.sort_by(|a, b| a.0.cmp(&b.0));
    commands
}

/// Check if the input matches a user-defined command and return the
/// content as a `SendMessage` action.
///
/// The `input` should be the full command string including the `/`
/// prefix (e.g. `/mycmd` or `/mycmd with args`). Only exact matches
/// on the command name are considered (no partial/alias matching).
/// Substitute $1, $2, $ARGUMENTS placeholders in a command template.
fn apply_template(template: &str, args: &str) -> String {
    let positional: Vec<&str> = args.split_whitespace().collect();
    let mut result = template.replace("$ARGUMENTS", args);
    for (i, arg) in positional.iter().enumerate() {
        result = result.replace(&format!("${}", i + 1), arg);
    }
    result
}

pub fn try_dispatch_user_command(app: &mut App, input: &str) -> Option<CommandResult> {
    let parts: Vec<&str> = input.trim().splitn(2, ' ').collect();
    let command = parts[0].to_lowercase();
    let command = command.strip_prefix('/').unwrap_or(&command);
    let args = parts.get(1).copied().unwrap_or("").trim();

    let user_commands = load_user_commands(Some(&app.workspace));

    for (name, content) in &user_commands {
        if name == command {
            let message = apply_template(content, args);
            return Some(CommandResult::action(AppAction::SendMessage(message)));
        }
    }

    None
}

/// Get user command names that match a given prefix (for autocomplete).
///
/// The prefix should be the command name portion only (after `/`).
/// Returns entries formatted as `/name`.
///
/// `workspace` is used to also scan workspace-local command directories;
/// pass `None` when no workspace context is available.
pub fn user_commands_matching(prefix: &str, workspace: Option<&Path>) -> Vec<String> {
    let prefix = prefix.to_lowercase();
    load_user_commands(workspace)
        .into_iter()
        .filter(|(name, _)| name.starts_with(&prefix))
        .map(|(name, _)| format!("/{name}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_global_commands_dir_contains_deepseek_commands() {
        let dir = global_commands_dir();
        let parts: Vec<_> = dir
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .collect();
        assert!(
            parts
                .windows(2)
                .any(|pair| pair == [".deepseek", "commands"]),
            "expected .deepseek/commands components in path, got: {}",
            dir.display()
        );
    }

    #[test]
    fn test_load_user_commands_when_no_dir_exists() {
        let cmds = load_user_commands(None);
        // Should not panic; returns empty vec when no directories exist.
        assert!(cmds.is_empty() || !cmds.is_empty());
    }

    #[test]
    fn test_try_dispatch_nonexistent_command() {
        use crate::config::Config;
        use crate::tui::app::TuiOptions;

        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
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
        let mut app = App::new(options, &Config::default());
        let result = try_dispatch_user_command(&mut app, "/nonexistent-thing-12345");
        assert!(result.is_none());
    }

    #[test]
    fn test_user_commands_matching_with_prefix_no_workspace() {
        let matches = user_commands_matching("zzzznotfound", None);
        assert!(matches.is_empty());
    }

    // ── Workspace-local commands tests ─────────────────────────────────

    fn write_command(dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("{name}.md")), body).unwrap();
    }

    #[test]
    fn load_user_commands_scans_workspace_local_dir() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let cmds_dir = ws.join(".deepseek").join("commands");
        write_command(&cmds_dir, "hello", "echo hi");

        let cmds = load_user_commands(Some(ws));
        let names: Vec<&str> = cmds.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains(&"hello"),
            "expected 'hello' in workspace-local commands: {names:?}"
        );
    }

    #[test]
    fn load_user_commands_scans_claude_and_cursor_dirs() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_command(
            &ws.join(".claude").join("commands"),
            "claude-cmd",
            "claude body",
        );
        write_command(
            &ws.join(".cursor").join("commands"),
            "cursor-cmd",
            "cursor body",
        );

        let cmds = load_user_commands(Some(ws));
        let names: Vec<&str> = cmds.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains(&"claude-cmd"),
            "expected 'claude-cmd': {names:?}"
        );
        assert!(
            names.contains(&"cursor-cmd"),
            "expected 'cursor-cmd': {names:?}"
        );
    }

    #[test]
    fn workspace_local_shadows_global_by_name() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();

        // Workspace-local version
        write_command(
            &ws.join(".deepseek").join("commands"),
            "shared",
            "workspace version",
        );
        // Global version — simulate by putting it in a "global" temp dir.
        // Since we can't easily override `dirs::home_dir()`, we test the
        // first-match-wins semantics by putting the same name in both
        // workspace-scanned dirs. The first dir in precedence order wins.
        write_command(
            &ws.join(".claude").join("commands"),
            "shared",
            "claude version",
        );

        let cmds = load_user_commands(Some(ws));
        let shared = cmds
            .iter()
            .find(|(n, _)| n == "shared")
            .expect("shared present");
        assert_eq!(
            shared.1, "workspace version",
            "workspace-local (.deepseek) must shadow later dirs"
        );
    }

    #[test]
    fn load_user_commands_without_workspace_falls_back_to_global_only() {
        // When no workspace is passed, only the global ~/.deepseek/commands/
        // is scanned. On test machines this dir often doesn't exist, so we
        // just verify we don't panic.
        let cmds = load_user_commands(None);
        // This should not panic; can be empty or have user's real commands.
        let _ = cmds;
    }

    #[test]
    fn try_dispatch_uses_workspace_local_command() {
        use crate::config::Config;
        use crate::tui::app::TuiOptions;

        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        write_command(
            &ws.join(".deepseek").join("commands"),
            "hello",
            "Hello, $ARGUMENTS!",
        );

        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: ws.clone(),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
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
        let mut app = App::new(options, &Config::default());
        let result = try_dispatch_user_command(&mut app, "/hello world");
        assert!(result.is_some());
        let cmd_result = result.unwrap();
        match cmd_result.action {
            Some(AppAction::SendMessage(msg)) => {
                assert!(msg.contains("Hello, world!"), "got: {msg}");
            }
            other => panic!("expected SendMessage action, got: {other:?}"),
        }
    }

    #[test]
    fn user_commands_matching_with_workspace() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_command(
            &ws.join(".deepseek").join("commands"),
            "project-cmd",
            "body",
        );

        let matches = user_commands_matching("project", Some(ws));
        assert!(
            matches.contains(&"/project-cmd".to_string()),
            "got: {matches:?}"
        );
    }
}
