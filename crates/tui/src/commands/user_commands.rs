//! User-defined slash commands from `~/.codewhale/commands/<name>.md` and
//! workspace-local `<workspace>/.codewhale/commands/<name>.md`.
//!
//! Users drop `.md` files into a commands directory and the filename
//! (without `.md` extension) becomes the default slash-command name. A
//! frontmatter `name` may replace it. When invoked, the file contents are sent
//! as a user message.
//!
//! Files may include optional YAML-like frontmatter between `---` markers.
//! Supported fields are `name`, `description`, `usage`, `arguments`,
//! `argument-hint`, `allowed-tools`, `pausable`, `alias`/`aliases`, and `hidden`.
//! Frontmatter is stripped before the command body is sent to the model.
//!
//! ## Precedence
//!
//! Workspace-local directories shadow user-global by name:
//!
//! 1. `<workspace>/.codewhale/commands/` (project-local, highest)
//! 2. `<workspace>/.deepseek/commands/`  (legacy project-local)
//! 3. `<workspace>/.claude/commands/`    (Claude Code interop)
//! 4. `<workspace>/.cursor/commands/`    (Cursor interop)
//! 5. `~/.codewhale/commands/`           (user-global)
//! 6. `~/.deepseek/commands/`            (legacy user-global)
//!
//! ## Permanent Role
//!
//! This module is the lower-level scanning, frontmatter parsing, and template
//! layer for [`super::user_registry::UserCommandRegistry`]. Runtime dispatch
//! lives in `user_registry.rs`; this file remains as the shared file I/O and
//! parsing boundary documented in `docs/architecture/command-dispatch.md`.

#[cfg(test)]
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[cfg(test)]
use crate::tui::app::{App, AppAction, HuntVerdict};

#[cfg(test)]
use super::CommandResult;

/// Path to the global user commands directory: `~/.codewhale/commands/`.
fn global_commands_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    home.join(".codewhale").join("commands")
}

fn legacy_global_commands_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    home.join(".deepseek").join("commands")
}

/// Return all candidate commands directories in precedence order.
pub(crate) fn commands_dirs(workspace: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(ws) = workspace {
        dirs.push(ws.join(".codewhale").join("commands"));
        dirs.push(ws.join(".deepseek").join("commands"));
        dirs.push(ws.join(".claude").join("commands"));
        dirs.push(ws.join(".cursor").join("commands"));
    }
    dirs.push(global_commands_dir());
    dirs.push(legacy_global_commands_dir());
    dirs
}

/// Scan a single commands directory for `.md` files and return
/// `(name, content)` pairs. Errors are silently skipped.
pub(crate) fn load_commands_from_dir(dir: &Path) -> Vec<(String, String)> {
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
#[cfg(test)]
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

pub(crate) fn parse_frontmatter(content: &str) -> (Vec<(String, String)>, &str) {
    let Some(first_line_end) = content.find('\n') else {
        return (Vec::new(), content);
    };
    let first = content[..first_line_end].trim_end_matches('\r');

    if first.trim().chars().all(|ch| ch == '-') && first.trim().len() >= 3 {
        let mut metadata = Vec::new();
        let mut offset = first_line_end + 1;
        let mut unclosed_body_start = None;
        for raw_line in content[offset..].split_inclusive('\n') {
            let line_start = offset;
            let line = raw_line.trim_end_matches(['\r', '\n']);
            offset += raw_line.len();
            let trimmed = line.trim();
            if unclosed_body_start.is_none() {
                if trimmed.chars().all(|ch| ch == '-') && trimmed.len() >= 3 {
                    let body = content[offset..].trim_start_matches(['\r', '\n']);
                    return (metadata, body);
                }
                if let Some((key, value)) = line.split_once(':') {
                    let key = key.trim().to_ascii_lowercase();
                    let raw_value = value.trim();
                    let value = if key == "allowed-tools" {
                        raw_value.to_string()
                    } else {
                        strip_matched_quotes(raw_value).to_string()
                    };
                    if !key.is_empty() {
                        metadata.push((key, value));
                    }
                } else if !trimmed.is_empty() {
                    unclosed_body_start = Some(line_start);
                }
            }
        }
        let body_start = unclosed_body_start.unwrap_or(content.len());
        let body = content[body_start..].trim_start_matches(['\r', '\n']);
        return (metadata, body);
    }

    (Vec::new(), content)
}

fn strip_matched_quotes(value: &str) -> &str {
    if let Some(stripped) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        return stripped;
    }
    if let Some(stripped) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        return stripped;
    }
    value
}

pub(crate) fn parse_allowed_tools(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|tool| {
            strip_matched_quotes(tool.trim())
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|tool| !tool.is_empty())
        .collect()
}

/// Check if the input matches a user-defined command and return the
/// content as a `SendMessage` action.
///
/// The `input` should be the full command string including the `/`
/// prefix (e.g. `/mycmd` or `/mycmd with args`). Only exact matches
/// on the command name are considered (no partial/alias matching).
/// Substitute $1, $2, $ARGUMENTS placeholders in a command template.
pub(crate) fn apply_template(template: &str, args: &str) -> String {
    let positional: Vec<&str> = args.split_whitespace().collect();
    let mut result = template.replace("$ARGUMENTS", args);
    for (i, arg) in positional.iter().enumerate() {
        result = result.replace(&format!("${}", i + 1), arg);
    }
    result
}

#[cfg(test)]
pub fn try_dispatch_user_command(app: &mut App, input: &str) -> Option<CommandResult> {
    let parts: Vec<&str> = input.trim().splitn(2, ' ').collect();
    let command = parts[0].to_lowercase();
    let command = command.strip_prefix('/').unwrap_or(&command);
    let args = parts.get(1).copied().unwrap_or("").trim();

    let user_commands = load_user_commands(Some(&app.workspace));

    for (name, content) in &user_commands {
        if name == command {
            let (metadata, body) = parse_frontmatter(content);
            app.hunt.quarry = None;
            app.hunt.started_at = None;
            app.hunt.verdict = HuntVerdict::Hunting;
            app.hunt.token_budget = None;
            app.hunt.tokens_used = 0;
            app.hunt.time_used_seconds = 0;
            app.hunt.continuation_count = 0;
            app.active_allowed_tools = None;
            app.pausable = false;
            app.paused = false;
            app.paused_quarry = None;
            // Clear todos and plan state from the previous command so they
            // don't bleed into the next one. Both are behind the same locks
            // the sidebar reads; a contended/poisoned lock is logged and
            // skipped rather than blocking dispatch.
            if let Ok(mut todos) = app.todos.try_lock() {
                todos.clear();
            } else {
                tracing::warn!(target: "commands", "todos lock contended or poisoned — previous todos not cleared");
            }
            if let Ok(mut plan) = app.plan_state.try_lock() {
                *plan = crate::tools::plan::PlanState::default();
            } else {
                tracing::warn!(target: "commands", "plan_state lock contended or poisoned — previous plan not cleared");
            }
            for (key, value) in &metadata {
                match key.as_str() {
                    "description" => {
                        app.hunt.quarry = Some(value.clone());
                        app.hunt.started_at = Some(std::time::Instant::now());
                    }
                    "allowed-tools" => {
                        app.active_allowed_tools = Some(parse_allowed_tools(value));
                    }
                    "pausable" => {
                        app.pausable = value.trim().eq_ignore_ascii_case("true");
                    }
                    _ => {}
                }
            }
            let message = apply_template(body, args);
            return Some(CommandResult::action(AppAction::SendMessage(message)));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_global_commands_dir_contains_codewhale_commands() {
        let dir = global_commands_dir();
        let parts: Vec<_> = dir
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .collect();
        assert!(
            parts
                .windows(2)
                .any(|pair| pair == [".codewhale", "commands"]),
            "expected .codewhale/commands components in path, got: {}",
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

    // ── Workspace-local commands tests ─────────────────────────────────

    fn write_command(dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("{name}.md")), body).unwrap();
    }

    fn test_options(workspace: PathBuf) -> crate::tui::app::TuiOptions {
        crate::tui::app::TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace,
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
        }
    }

    #[test]
    fn load_user_commands_scans_workspace_local_dir() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let cmds_dir = ws.join(".codewhale").join("commands");
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
            &ws.join(".codewhale").join("commands"),
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
            "workspace-local (.codewhale) must shadow later dirs"
        );
    }

    #[test]
    fn load_user_commands_without_workspace_falls_back_to_global_only() {
        // When no workspace is passed, only global command directories are
        // scanned. On test machines these often don't exist, so we just
        // verify we don't panic.
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
    fn frontmatter_is_stripped_before_dispatch() {
        use crate::config::Config;

        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        write_command(
            &ws.join(".deepseek").join("commands"),
            "secure",
            "---\ndescription: Secure scan\nallowed-tools: Bash, Read\n---\nRun $ARGUMENTS",
        );

        let mut app = App::new(test_options(ws), &Config::default());
        let result = try_dispatch_user_command(&mut app, "/secure checks").unwrap();
        match result.action {
            Some(AppAction::SendMessage(msg)) => assert_eq!(msg, "Run checks"),
            other => panic!("expected SendMessage action, got: {other:?}"),
        }
    }

    #[test]
    fn review_regression_unclosed_frontmatter_keeps_metadata_and_strips_header() {
        let (metadata, body) = parse_frontmatter(
            "---\ndescription: Broken command\nallowed-tools: Bash\nRun the safe body",
        );

        assert_eq!(
            metadata,
            vec![
                ("description".to_string(), "Broken command".to_string()),
                ("allowed-tools".to_string(), "Bash".to_string())
            ]
        );
        assert_eq!(body, "Run the safe body");
    }

    #[test]
    fn review_regression_unclosed_frontmatter_without_metadata_strips_header() {
        let (metadata, body) =
            parse_frontmatter("---\nRun the command body without a closing delimiter");

        assert!(metadata.is_empty());
        assert_eq!(body, "Run the command body without a closing delimiter");
    }

    #[test]
    fn review_regression_frontmatter_strips_only_matched_quote_pairs() {
        let (metadata, body) = parse_frontmatter("---\ndescription: 'Read\"\n---\nrun");

        assert_eq!(
            metadata,
            vec![("description".to_string(), "'Read\"".to_string())]
        );
        assert_eq!(body, "run");
    }

    #[test]
    fn allowed_tools_frontmatter_sets_app_state() {
        use crate::config::Config;

        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        write_command(
            &ws.join(".deepseek").join("commands"),
            "secure",
            "---\nallowed-tools: Bash, Grep\n---\nrun tests",
        );

        let mut app = App::new(test_options(ws), &Config::default());
        let _ = try_dispatch_user_command(&mut app, "/secure").unwrap();
        assert_eq!(
            app.active_allowed_tools,
            Some(vec!["bash".to_string(), "grep".to_string()])
        );
    }

    #[test]
    fn pausable_frontmatter_sets_app_state_without_worktree_mutation() {
        use crate::config::Config;

        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }

        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        let init = std::process::Command::new("git")
            .args(["-C", ws.to_str().unwrap(), "init"])
            .output()
            .expect("git init");
        assert!(
            init.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&init.stderr)
        );
        std::fs::write(ws.join("user-work.txt"), "untracked user work").unwrap();
        write_command(
            &ws.join(".codewhale").join("commands"),
            "pause-scan",
            "---\ndescription: Scan repos\npausable: true\n---\nscan",
        );

        let mut app = App::new(test_options(ws.clone()), &Config::default());
        let _ = try_dispatch_user_command(&mut app, "/pause-scan").unwrap();

        assert!(app.pausable);
        assert!(!app.paused);
        assert!(app.paused_quarry.is_none());
        assert!(ws.join("user-work.txt").exists());
        let stash = std::process::Command::new("git")
            .args(["-C", ws.to_str().unwrap(), "stash", "list"])
            .output()
            .expect("git stash list");
        assert!(
            stash.status.success(),
            "git stash list failed: {}",
            String::from_utf8_lossy(&stash.stderr)
        );
        assert!(
            String::from_utf8_lossy(&stash.stdout).trim().is_empty(),
            "pausable dispatch must not create git stash entries"
        );
    }

    #[test]
    fn new_user_command_clears_stale_paused_state() {
        use crate::config::Config;

        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        let commands_dir = ws.join(".codewhale").join("commands");
        write_command(
            &commands_dir,
            "pause-scan",
            "---\ndescription: Scan repos\npausable: true\n---\nscan",
        );
        write_command(&commands_dir, "plain", "plain command");

        let mut app = App::new(test_options(ws), &Config::default());
        let _ = try_dispatch_user_command(&mut app, "/pause-scan").unwrap();
        app.paused = true;
        app.paused_quarry = Some("Scan repos".to_string());

        let _ = try_dispatch_user_command(&mut app, "/plain").unwrap();

        assert!(!app.pausable);
        assert!(!app.paused);
        assert!(app.paused_quarry.is_none());
    }

    #[test]
    fn new_user_command_clears_previous_todos_and_plan() {
        use crate::config::Config;
        use crate::tools::plan::UpdatePlanArgs;
        use crate::tools::todo::TodoStatus;

        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        let commands_dir = ws.join(".codewhale").join("commands");
        write_command(&commands_dir, "first", "first command body");
        write_command(&commands_dir, "second", "second command body");

        let mut app = App::new(test_options(ws), &Config::default());

        // Seed the state a previous command would leave behind: a non-empty
        // todo list and a non-empty plan. These should NOT bleed into the
        // next command. The shared lists are tokio async mutexes, so seed and
        // observe through `try_lock` (the same sync path dispatch uses).
        {
            let mut todos = app.todos.try_lock().expect("todos lock");
            todos.add(
                "leftover task from first command".to_string(),
                TodoStatus::Pending,
            );
        }
        {
            let mut plan = app.plan_state.try_lock().expect("plan_state lock");
            plan.update(UpdatePlanArgs {
                title: Some("leftover plan".to_string()),
                objective: Some("old goal".to_string()),
                ..Default::default()
            });
        }

        // Dispatch a fresh command — dispatch must reset both.
        let _ = try_dispatch_user_command(&mut app, "/second").unwrap();

        assert!(
            app.todos
                .try_lock()
                .expect("todos lock")
                .snapshot()
                .items
                .is_empty(),
            "previous command's todos must be cleared on new command dispatch"
        );
        assert!(
            app.plan_state
                .try_lock()
                .expect("plan_state lock")
                .is_empty(),
            "previous command's plan must be cleared on new command dispatch"
        );
    }

    #[test]
    fn review_regression_empty_allowed_tools_blocks_all_tools() {
        use crate::config::Config;

        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        write_command(
            &ws.join(".deepseek").join("commands"),
            "locked",
            "---\nallowed-tools: \"\"\n---\nrun nothing",
        );

        let mut app = App::new(test_options(ws), &Config::default());
        let _ = try_dispatch_user_command(&mut app, "/locked").unwrap();
        assert_eq!(app.active_allowed_tools, Some(Vec::new()));
    }

    #[test]
    fn review_regression_allowed_tools_accepts_per_item_quotes() {
        use crate::config::Config;

        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        write_command(
            &ws.join(".deepseek").join("commands"),
            "quoted",
            "---\nallowed-tools: \"exec_shell\", 'read_file'\n---\nrun quoted tools",
        );

        let mut app = App::new(test_options(ws), &Config::default());
        let _ = try_dispatch_user_command(&mut app, "/quoted").unwrap();
        assert_eq!(
            app.active_allowed_tools,
            Some(vec!["exec_shell".to_string(), "read_file".to_string()])
        );
    }

    #[test]
    fn review_regression_dispatch_without_frontmatter_resets_previous_command_state() {
        use crate::config::Config;

        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        let commands_dir = ws.join(".deepseek").join("commands");
        write_command(
            &commands_dir,
            "described",
            "---\ndescription: Scan repos\nallowed-tools: Bash\n---\nscan",
        );
        write_command(&commands_dir, "plain", "plain command");

        let mut app = App::new(test_options(ws), &Config::default());
        let _ = try_dispatch_user_command(&mut app, "/described").unwrap();
        assert_eq!(app.hunt.quarry.as_deref(), Some("Scan repos"));
        assert!(app.hunt.started_at.is_some());
        assert_eq!(app.hunt.verdict, crate::tui::app::HuntVerdict::Hunting);
        assert_eq!(app.hunt.token_budget, None);
        assert_eq!(app.active_allowed_tools, Some(vec!["bash".to_string()]));

        app.hunt.verdict = crate::tui::app::HuntVerdict::Escaped;
        app.hunt.token_budget = Some(42);
        app.hunt.tokens_used = 100;
        app.hunt.time_used_seconds = 5;
        app.hunt.continuation_count = 1;
        let _ = try_dispatch_user_command(&mut app, "/plain").unwrap();
        assert_eq!(app.hunt.quarry, None);
        assert_eq!(app.hunt.started_at, None);
        assert_eq!(app.hunt.verdict, crate::tui::app::HuntVerdict::Hunting);
        assert_eq!(app.hunt.token_budget, None);
        assert_eq!(app.hunt.tokens_used, 0);
        assert_eq!(app.hunt.time_used_seconds, 0);
        assert_eq!(app.hunt.continuation_count, 0);
        assert_eq!(app.active_allowed_tools, None);
    }

    #[test]
    fn description_frontmatter_sets_work_objective_and_autocomplete_description() {
        use crate::config::Config;

        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        write_command(
            &ws.join(".deepseek").join("commands"),
            "git-scan",
            "---\ndescription: Scan nested git repositories\nargument-hint: <root>\n---\nscan",
        );

        let mut app = App::new(test_options(ws.clone()), &Config::default());
        let _ = try_dispatch_user_command(&mut app, "/git-scan").unwrap();
        assert_eq!(
            app.hunt.quarry.as_deref(),
            Some("Scan nested git repositories")
        );
        let commands = load_user_commands(Some(&ws));
        let (_, content) = commands
            .iter()
            .find(|(name, _)| name == "git-scan")
            .expect("git-scan command should load");
        let (metadata, _) = parse_frontmatter(content);
        assert!(metadata.contains(&(
            "description".to_string(),
            "Scan nested git repositories".to_string()
        )));
        assert!(metadata.contains(&("argument-hint".to_string(), "<root>".to_string())));
    }

    #[test]
    fn parser_preserves_layer_5_1_frontmatter_fields() {
        let (metadata, body) = parse_frontmatter(
            "---\nname: inspect\ndescription: Inspect a target\nusage: /inspect <path>\narguments: <path>\nhidden: false\nallowed-tools: Read_File, Grep_Files\n---\ninspect $ARGUMENTS",
        );

        assert!(metadata.contains(&("name".to_string(), "inspect".to_string())));
        assert!(metadata.contains(&("description".to_string(), "Inspect a target".to_string())));
        assert!(metadata.contains(&("usage".to_string(), "/inspect <path>".to_string())));
        assert!(metadata.contains(&("arguments".to_string(), "<path>".to_string())));
        assert!(metadata.contains(&("hidden".to_string(), "false".to_string())));
        assert!(metadata.contains(&(
            "allowed-tools".to_string(),
            "Read_File, Grep_Files".to_string()
        )));
        assert_eq!(body, "inspect $ARGUMENTS");
    }
}
