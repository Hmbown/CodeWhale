//! Slash command registry and dispatch system
//!
//! This module provides a modular command system built on the strategy pattern.
//! Commands are organized by logical group (Core, Session, Config, …), each
//! group lives in its own file, and the central registry collects them all.
//! `mod.rs` only registers groups and dispatches commands — it contains zero
//! command-specific code.

pub mod traits;
pub mod user_commands;

// Group modules — each registers its commands into the registry.
// Individual groups are declared in groups/mod.rs.
pub(crate) mod groups;

use std::sync::OnceLock;

use crate::tui::app::{App, AppAction};

#[allow(unused_imports)]
pub use traits::CommandInfo;

/// Result of executing a command
#[derive(Debug, Clone)]
pub struct CommandResult {
    /// Optional message to display to the user
    pub message: Option<String>,
    /// Optional action for the app to take
    pub action: Option<AppAction>,
    /// Whether the command failed.
    pub is_error: bool,
}

impl CommandResult {
    pub fn ok() -> Self {
        Self {
            message: None,
            action: None,
            is_error: false,
        }
    }
    pub fn message(msg: impl Into<String>) -> Self {
        Self {
            message: Some(msg.into()),
            action: None,
            is_error: false,
        }
    }
    pub fn action(action: AppAction) -> Self {
        Self {
            message: None,
            action: Some(action),
            is_error: false,
        }
    }
    pub fn with_message_and_action(msg: impl Into<String>, action: AppAction) -> Self {
        Self {
            message: Some(msg.into()),
            action: Some(action),
            is_error: false,
        }
    }
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            message: Some(format!("Error: {}", msg.into())),
            action: None,
            is_error: true,
        }
    }
}

// ── Registry access ────────────────────────────────────────────────────────

/// Access the global command registry (lazily initialised).
static REGISTRY: OnceLock<traits::CommandRegistry> = OnceLock::new();

fn build_registry() -> traits::CommandRegistry {
    let mut reg = traits::CommandRegistry::empty();
    for group in groups::all_command_groups() {
        reg.register_group(group);
    }
    reg
}

pub fn registry() -> &'static traits::CommandRegistry {
    REGISTRY.get_or_init(build_registry)
}

// ── Dispatch ───────────────────────────────────────────────────────────────

/// Execute a slash command.
///
/// Parses `cmd` (e.g. `/help` or `/help model`), looks up the command in
/// the registry, and runs it. User-defined commands are checked first so
/// they can shadow built-ins.
pub fn execute(cmd: &str, app: &mut App) -> CommandResult {
    let trimmed = cmd.trim();
    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let command = parts[0].to_lowercase();
    let command = command.strip_prefix('/').unwrap_or(&command);
    let arg = parts.get(1).map(|s| s.trim());

    // User-defined commands FIRST so they can override built-ins.
    if let Some(result) = user_commands::try_dispatch_user_command(app, trimmed) {
        return result;
    }

    // Registry lookup.
    if let Some(cmd_obj) = registry().get(command) {
        return cmd_obj.execute(app, arg);
    }

    // Skill fallback (lowest precedence).
    if let Some(result) = groups::skills::skill::skill_impl::run_skill_by_name(app, command, arg) {
        return result;
    }

    let suggestions = suggest_command_names(command, 3);
    if suggestions.is_empty() {
        CommandResult::error(format!(
            "Unknown command: /{command}. Type /help for available commands."
        ))
    } else {
        let list = suggestions
            .into_iter()
            .map(|name| format!("/{name}"))
            .collect::<Vec<_>>()
            .join(", ");
        CommandResult::error(format!(
            "Unknown command: /{command}. Did you mean: {list}? Type /help for available commands."
        ))
    }
}

// ── Suggestions ────────────────────────────────────────────────────────────

fn edit_distance(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    if a.is_empty() {
        return b.chars().count();
    }
    if b.is_empty() {
        return a.chars().count();
    }

    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    let mut curr = vec![0usize; b_chars.len() + 1];

    for (i, a_ch) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, b_ch) in b_chars.iter().enumerate() {
            let cost = if a_ch == *b_ch { 0 } else { 1 };
            let delete = prev[j + 1] + 1;
            let insert = curr[j] + 1;
            let substitute = prev[j] + cost;
            curr[j + 1] = delete.min(insert).min(substitute);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_chars.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use std::path::PathBuf;

    fn test_app() -> App {
        App::new(
            TuiOptions {
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
            },
            &Config::default(),
        )
    }

    #[test]
    fn registry_contains_commands() {
        let cmds = registry().infos();
        assert!(!cmds.is_empty(), "registry should contain commands");
        assert!(cmds.iter().any(|c| c.name == "help"));
        assert!(cmds.iter().any(|c| c.name == "clear"));
        assert!(cmds.iter().any(|c| c.name == "config"));
    }

    #[test]
    fn execute_help_command_succeeds() {
        let mut app = test_app();
        let result = execute("/help", &mut app);
        assert!(
            !result.is_error,
            "help should succeed: {:?}",
            result.message
        );
    }

    #[test]
    fn execute_unknown_command_returns_error() {
        let mut app = test_app();
        let result = execute("/nonexistent", &mut app);
        assert!(result.is_error);
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or("")
                .contains("Unknown command")
        );
    }

    #[test]
    fn execute_without_slash_still_works() {
        let mut app = test_app();
        let result = execute("help", &mut app);
        assert!(!result.is_error);
    }

    #[test]
    fn execute_dispatches_by_alias() {
        let mut app = test_app();
        let result = execute("/qingping", &mut app);
        assert!(!result.is_error, "alias /qingping should dispatch to clear");
    }

    #[test]
    fn unknown_command_suggests_similar() {
        let mut app = test_app();
        let result = execute("/hel", &mut app);
        let msg = result.message.as_deref().unwrap_or("");
        assert!(msg.contains("Did you mean"));
    }
}

fn suggest_command_names(input: &str, limit: usize) -> Vec<String> {
    let query = input.trim().to_ascii_lowercase();
    if query.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut scored: Vec<(u8, usize, String)> = Vec::new();
    for info in registry().infos() {
        let mut best: Option<(u8, usize)> = None;
        for candidate in std::iter::once(info.name).chain(info.aliases.iter().copied()) {
            let candidate = candidate.to_ascii_lowercase();
            let prefix_match = candidate.starts_with(&query) || query.starts_with(&candidate);
            let contains_match = candidate.contains(&query) || query.contains(&candidate);
            let distance = edit_distance(&candidate, &query);
            let close_typo = distance <= 2;
            if !(prefix_match || contains_match || close_typo) {
                continue;
            }

            let rank = if prefix_match {
                0
            } else if contains_match {
                1
            } else {
                2
            };

            match best {
                Some((best_rank, best_distance))
                    if rank > best_rank || (rank == best_rank && distance >= best_distance) => {}
                _ => best = Some((rank, distance)),
            }
        }

        if let Some((rank, distance)) = best {
            scored.push((rank, distance, info.name.to_string()));
        }
    }

    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, _, name)| name)
        .collect()
}
