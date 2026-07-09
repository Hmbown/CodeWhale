//! Slash command registry and dispatch system
//!
//! This module provides a modular command system inspired by Codex-rs.
//! Commands are organized by category and dispatched through a central strategy
//! registry. Built-in handlers live in group-owned areas under [`groups`]; this
//! module keeps registry construction, user-command precedence, and the
//! fall-through behaviour.

mod groups;
pub mod traits;
pub mod user_commands;
pub mod user_registry;

use std::sync::OnceLock;

pub use traits::CommandInfo;

// Long-standing public paths that predate the group layout.
pub use groups::project::share;

// Voice capture plumbing shared with the hotbar and the UI event loop.
pub use groups::core::voice;

use crate::tui::app::{App, AppAction};

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
    /// Create an empty result (command succeeded with no output)
    pub fn ok() -> Self {
        Self {
            message: None,
            action: None,
            is_error: false,
        }
    }

    /// Create a result with just a message
    pub fn message(msg: impl Into<String>) -> Self {
        Self {
            message: Some(msg.into()),
            action: None,
            is_error: false,
        }
    }

    /// Create a result with an action
    pub fn action(action: AppAction) -> Self {
        Self {
            message: None,
            action: Some(action),
            is_error: false,
        }
    }

    /// Create a result with both message and action
    pub fn with_message_and_action(msg: impl Into<String>, action: AppAction) -> Self {
        Self {
            message: Some(msg.into()),
            action: Some(action),
            is_error: false,
        }
    }

    /// Create an error message result
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            message: Some(format!("Error: {}", msg.into())),
            action: None,
            is_error: true,
        }
    }
}

static REGISTRY: OnceLock<traits::CommandRegistry> = OnceLock::new();

fn build_registry() -> traits::CommandRegistry {
    let mut registry = traits::CommandRegistry::empty();
    for &group in groups::all_command_groups() {
        registry.register_group(group);
    }
    registry
}

pub fn registry() -> &'static traits::CommandRegistry {
    REGISTRY.get_or_init(build_registry)
}

pub fn command_infos() -> Vec<&'static CommandInfo> {
    registry().infos()
}

pub fn get_command_info(name: &str) -> Option<&'static CommandInfo> {
    registry().get_info(name)
}

/// Execute a slash command
pub fn execute(cmd: &str, app: &mut App) -> CommandResult {
    let trimmed = cmd.trim();

    // `$skillname` is a backward-compatible alias for `/skill skillname`.
    // Resolve it early so skills can be loaded with the `$` prefix.
    if let Some(skill_input) = trimmed.strip_prefix('$') {
        let skill_input = skill_input.trim_start();
        if skill_input.is_empty() {
            return CommandResult::error(
                "Type a skill name after $. For example: $getting-started",
            );
        }
        let parts: Vec<&str> = skill_input.splitn(2, char::is_whitespace).collect();
        let skill_name = parts.first().copied().unwrap_or("");
        let arg = parts
            .get(1)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty());
        if let Some(result) = groups::skills::run_skill_by_name(app, skill_name, arg) {
            return result;
        }
        return CommandResult::error(format!(
            "Unknown skill: ${skill_name}. Type /skills to see installed skills."
        ));
    }

    let parts: Vec<&str> = trimmed.splitn(2, char::is_whitespace).collect();
    let command = parts
        .first()
        .copied()
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_ascii_lowercase();
    let arg = parts
        .get(1)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());

    // Check user-defined commands FIRST so they can override built-ins.
    if let Some(result) = user_registry::try_dispatch(app, trimmed) {
        return result;
    }

    // Permanent backward-compatible aliases. They predate the group-owned
    // registry and remain documented in docs/architecture/command-dispatch.md.
    match command.as_str() {
        "jihua" => {
            return groups::config::dispatch(app, "jihua", arg).unwrap_or_else(|| {
                CommandResult::error("The /jihua alias could not be dispatched.")
            });
        }
        "zidong" => {
            return groups::config::dispatch(app, "zidong", arg).unwrap_or_else(|| {
                CommandResult::error("The /zidong alias could not be dispatched.")
            });
        }
        "slop" | "canzha" => {
            return groups::config::dispatch(app, "debt", arg).unwrap_or_else(|| {
                CommandResult::error("The /debt command could not be dispatched.")
            });
        }
        _ => {}
    }

    if let Some(command_object) = registry().get(command.as_str()) {
        return command_object.execute(app, arg);
    }

    match command.as_str() {
        // Permanent legacy migration hints. These are deliberately excluded
        // from registry/autocomplete and only appear when users type old names.
        "set" => CommandResult::error(
            "The /set command was retired. Use /config to edit settings and /settings to inspect current values.",
        ),
        "deepseek" => CommandResult::error(
            "The /deepseek command was renamed. Use /links (aliases: /dashboard, /api).",
        ),
        "doctor" => CommandResult::error(
            "The /doctor command is a CLI diagnostic. Run `codewhale doctor` or `codewhale doctor --json`; use `/setup` in the TUI for readiness and verification.",
        ),

        _ => {
            // Third source: skills (lowest precedence after native and user-config).
            // Try to run a skill whose name matches the command.
            if let Some(result) = groups::skills::run_skill_by_name(app, command.as_str(), arg) {
                return result;
            }
            let suggestions = suggest_command_names(command.as_str(), 3);
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
    }
}

/// Update a configuration value programmatically (used by interactive UI views).
pub fn set_config_value(app: &mut App, key: &str, value: &str, persist: bool) -> CommandResult {
    groups::config::config::set_config_value(app, key, value, persist)
}

pub fn switch_mode(app: &mut App, mode: crate::tui::app::AppMode) -> String {
    groups::config::config::switch_mode(app, mode)
}

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
    let mut previous: Vec<usize> = (0..=b_chars.len()).collect();
    let mut current = vec![0usize; b_chars.len() + 1];

    for (i, a_ch) in a.chars().enumerate() {
        current[0] = i + 1;
        for (j, b_ch) in b_chars.iter().enumerate() {
            let cost = if a_ch == *b_ch { 0 } else { 1 };
            let delete = previous[j + 1] + 1;
            let insert = current[j] + 1;
            let substitute = previous[j] + cost;
            current[j + 1] = delete.min(insert).min(substitute);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[b_chars.len()]
}

fn suggest_command_names(input: &str, limit: usize) -> Vec<String> {
    let query = input.trim().to_ascii_lowercase();
    if query.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut scored: Vec<(u8, usize, String)> = Vec::new();
    for command in registry().infos() {
        let mut best: Option<(u8, usize)> = None;
        for candidate in std::iter::once(command.name).chain(command.aliases.iter().copied()) {
            let prefix_match = candidate.starts_with(&query) || query.starts_with(candidate);
            let contains_match = candidate.contains(&query) || query.contains(candidate);
            let distance = edit_distance(candidate, &query);
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
            scored.push((rank, distance, command.name.to_string()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApiProvider, Config};
    use crate::localization::{Locale, MessageId};
    use crate::tools::plan::{PlanItemArg, StepStatus, UpdatePlanArgs};
    use crate::tools::todo::TodoStatus;
    use crate::tui::app::{App, AppAction, SidebarFocus, TuiOptions};
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::MutexGuard;
    use tempfile::tempdir;

    fn create_test_app() -> App {
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
        App::new(options, &Config::default())
    }

    #[test]
    fn user_registry_module_is_compiled() {
        super::user_registry::reload(None);
        let registry = super::user_registry::current_registry();
        assert!(registry.is_valid());
    }

    #[test]
    fn user_command_shadows_builtin_before_group_dispatch() {
        let temp = tempdir().unwrap();
        let commands_dir = temp.path().join(".codewhale").join("commands");
        std::fs::create_dir_all(&commands_dir).unwrap();
        std::fs::write(
            commands_dir.join("help.md"),
            "---\ndescription: User help\n---\nuser help $ARGUMENTS",
        )
        .unwrap();

        let mut app = create_test_app();
        app.workspace = temp.path().to_path_buf();
        super::user_registry::reload(Some(temp.path()));

        let result = execute("/help now", &mut app);
        assert!(!result.is_error);
        match result.action {
            Some(AppAction::SendMessage(message)) => assert_eq!(message, "user help now"),
            other => panic!("expected user command SendMessage action, got {other:?}"),
        }
    }

    #[test]
    fn removed_user_command_reloads_and_falls_back_to_builtin() {
        let temp = tempdir().unwrap();
        let commands_dir = temp.path().join(".codewhale").join("commands");
        std::fs::create_dir_all(&commands_dir).unwrap();
        let command_path = commands_dir.join("help.md");
        std::fs::write(&command_path, "user help").unwrap();

        let mut app = create_test_app();
        app.workspace = temp.path().to_path_buf();
        super::user_registry::reload(Some(temp.path()));
        assert!(matches!(
            execute("/help config", &mut app).action,
            Some(AppAction::SendMessage(_))
        ));

        std::fs::remove_file(command_path).unwrap();
        super::user_registry::reload(Some(temp.path()));
        let result = execute("/help config", &mut app);
        assert!(!result.is_error);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("config")),
            "built-in /help should handle the command"
        );
        assert!(result.action.is_none());
    }

    #[test]
    fn command_registry_contains_config_and_links_but_not_set_or_deepseek() {
        assert!(command_infos().iter().any(|cmd| cmd.name == "config"));
        let sidebar = command_infos()
            .into_iter()
            .find(|cmd| cmd.name == "sidebar")
            .expect("sidebar command should exist");
        assert_eq!(sidebar.description_id, MessageId::CmdSidebarDescription);
        assert!(
            sidebar
                .description_for(Locale::En)
                .contains("right sidebar")
        );
        assert!(command_infos().iter().any(|cmd| cmd.name == "links"));
        let hf = command_infos()
            .into_iter()
            .find(|cmd| cmd.name == "hf")
            .expect("hf command should exist");
        assert_eq!(hf.aliases, &["huggingface"]);
        assert_eq!(hf.description_id, MessageId::CmdHfDescription);
        assert!(hf.description_for(Locale::En).contains("Hugging Face"));
        assert!(command_infos().iter().any(|cmd| cmd.name == "memory"));
        assert!(!command_infos().iter().any(|cmd| cmd.name == "set"));
        assert!(!command_infos().iter().any(|cmd| cmd.name == "deepseek"));
    }

    #[test]
    fn links_command_has_dashboard_and_api_aliases() {
        let links = command_infos()
            .into_iter()
            .find(|cmd| cmd.name == "links")
            .expect("links command should exist");
        assert_eq!(links.aliases, &["dashboard", "api", "lianjie"]);
    }

    #[test]
    fn hf_alias_dispatches_to_concepts_helper() {
        let mut app = create_test_app();
        let result = execute("/huggingface concepts", &mut app);
        assert!(!result.is_error);
        let message = result.message.expect("concepts message");
        assert!(message.contains("Hugging Face provider route"));
        assert!(message.contains("Hugging Face MCP"));
        assert!(message.contains("Hub workflows"));
    }

    #[test]
    fn rlm_slash_command_routes_to_persistent_tool_instruction() {
        let mut app = create_test_app();
        let result = execute("/rlm 2 inspect this long corpus", &mut app);
        assert!(!result.is_error);
        assert!(result.message.as_deref().unwrap_or("").contains("depth 2"));
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("rlm_open"));
        assert!(message.contains("rlm_configure"));
        assert!(message.contains("sub_rlm_max_depth: 2"));
    }

    #[test]
    fn agent_slash_command_routes_to_persistent_tool_instruction() {
        let mut app = create_test_app();
        let result = execute("/agent 0 inspect the parser", &mut app);
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("`agent`"));
        assert!(message.contains("max_depth: 0"));
    }

    #[test]
    fn relay_slash_command_routes_to_session_relay_instruction() {
        let mut app = create_test_app();
        app.hunt.quarry = Some("Unify the work surface".to_string());
        app.hunt.token_budget = Some(12_000);
        {
            let mut todos = app.todos.try_lock().expect("todo lock");
            todos.add("inspect workspace".to_string(), TodoStatus::Completed);
            todos.add("patch relay command".to_string(), TodoStatus::InProgress);
        }
        {
            let mut plan = app.plan_state.try_lock().expect("plan lock");
            plan.update(UpdatePlanArgs {
                objective: Some("Keep relays grounded".to_string()),
                explanation: Some("RLM-style strategy".to_string()),
                sources_used: vec!["transcript context".to_string()],
                critical_files: vec!["crates/tui/src/commands/mod.rs".to_string()],
                constraints: vec!["Do not invent verification".to_string()],
                verification_plan: Some("Check relay prompt assertions".to_string()),
                handoff_packet: Some("Next thread should read the To-do list".to_string()),
                plan: vec![PlanItemArg {
                    step: "keep To-do primary".to_string(),
                    status: StepStatus::InProgress,
                }],
                ..UpdatePlanArgs::default()
            });
        }

        let result = execute("/relay verify install", &mut app);
        assert!(!result.is_error);
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or_default()
                .contains(".deepseek/handoff.md")
        );
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("session relay"));
        assert!(message.contains("接力"));
        assert!(message.contains("Write or update `.deepseek/handoff.md`"));
        assert!(message.contains("# Session relay"));
        assert!(message.contains("Requested relay focus: verify install"));
        assert!(message.contains("Goal objective: Unify the work surface"));
        assert!(message.contains("Goal token budget: 12000"));
        assert!(message.contains("To-do (primary progress surface, 50% complete)"));
        assert!(message.contains("#1 [completed] inspect workspace"));
        assert!(message.contains("#2 [in_progress] patch relay command"));
        assert!(message.contains("Optional strategy metadata from update_plan"));
        assert!(message.contains("Objective: Keep relays grounded"));
        assert!(message.contains("Explanation: RLM-style strategy"));
        assert!(message.contains("Source: transcript context"));
        assert!(message.contains("Critical file: crates/tui/src/commands/mod.rs"));
        assert!(message.contains("Constraint: Do not invent verification"));
        assert!(message.contains("Verification plan: Check relay prompt assertions"));
        assert!(message.contains("Handoff packet: Next thread should read the To-do list"));
        assert!(message.contains("[in_progress] keep To-do primary"));
        assert!(
            !message.contains("Work checklist"),
            "relay copy should use To-do vocabulary: {message}"
        );
    }

    #[test]
    fn relay_command_has_bilingual_aliases() {
        let relay = command_infos()
            .into_iter()
            .find(|cmd| cmd.name == "relay")
            .expect("relay command should exist");
        assert_eq!(relay.aliases, &["batonpass", "接力"]);
        assert!(relay.description_for(Locale::ZhHans).contains("接力"));
        assert!(relay.description_for(Locale::ZhHant).contains("接力"));

        let mut app = create_test_app();
        let result = execute("/接力 next hand", &mut app);
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("Requested relay focus: next hand"));
    }

    /// AT-008: No built-in command name or alias is registered twice,
    /// and no built-in alias collides with another command's canonical name.
    /// This test iterates every command from `command_infos()` (all 9 groups)
    /// and asserts uniqueness across the full set of names and aliases.
    #[test]
    fn command_registry_has_unique_names_and_aliases() {
        let mut names = std::collections::BTreeSet::new();
        for command in command_infos() {
            assert!(
                names.insert(command.name),
                "duplicate command name /{}",
                command.name
            );
        }

        let mut aliases = std::collections::BTreeSet::new();
        for command in command_infos() {
            for alias in command.aliases {
                assert!(
                    !names.contains(alias),
                    "alias /{alias} collides with a command name"
                );
                assert!(aliases.insert(*alias), "duplicate command alias /{alias}");
            }
        }
    }

    /// AT-009: Command ownership contract — top-level `commands/mod.rs` only
    /// registers groups (`groups::all_command_groups()`), each group owns its
    /// `commands()` list, and every command has valid metadata.
    ///
    /// Config and debug groups are documented permanent exceptions: they keep
    /// group-local `CommandInfo` statics and `dispatch()` in `mod.rs` rather
    /// than extracting every command into a focused module. This is accepted
    /// final structure per FEAT-008 §3.2.
    ///
    /// Enforcement strategy:
    /// - Exactly 9 source-verified groups (from `groups/mod.rs`)
    /// - Each group owns its commands() list
    /// - Config and debug exceptions verified within their specific groups by
    ///   identifying the group through its first command ("config" and "tokens")
    /// - Not circular: the group-iterated command count is a consistency check;
    ///   the primary enforcement is exact group count + per-group non-empty + valid metadata
    #[test]
    fn command_ownership_contract_is_enforced() {
        let groups = groups::all_command_groups();

        // AT-009 primary: exactly 9 groups matching groups/mod.rs
        assert_eq!(
            groups.len(),
            9,
            "expected exactly 9 command groups (core, session, config, debug, \
             project, skills, memory, plugins, utility), got {}",
            groups.len()
        );

        let mut total_commands = 0;
        let mut has_config = false;
        let mut has_debug = false;
        for &group in groups {
            let commands = group.commands();
            assert!(
                !commands.is_empty(),
                "each group must have at least one command"
            );
            for cmd in commands {
                let info = cmd.info();
                assert!(!info.name.is_empty(), "command name must not be empty");
                assert!(
                    info.name
                        .chars()
                        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit()),
                    "/{} command names must be lowercase ASCII",
                    info.name
                );
                let usage_prefix = format!("/{}", info.name);
                assert!(
                    info.usage.starts_with(&usage_prefix),
                    "/{} usage must start with /{{name}}, got {:?}",
                    info.name,
                    info.usage
                );
            }
            total_commands += commands.len();

            // Identify config and debug groups by their command content to
            // verify permanent-exception counts within the correct group.
            if commands.iter().any(|c| c.info().name == "config") {
                has_config = true;
                assert_eq!(
                    commands.len(),
                    11,
                    "config group (group-local metadata exception) expected \
                     exactly 11 commands, got {}",
                    commands.len()
                );
            }
            if commands.iter().any(|c| c.info().name == "tokens") {
                has_debug = true;
                assert_eq!(
                    commands.len(),
                    11,
                    "debug group (group-local metadata exception) expected \
                     exactly 11 commands, got {}",
                    commands.len()
                );
            }
        }

        // Config and debug groups must be found and verified by content identity
        assert!(
            has_config,
            "config group not found (expected first command: /config)"
        );
        assert!(
            has_debug,
            "debug group not found (expected first command: /tokens)"
        );

        // Consistency: group-iterated command count must match registry
        assert_eq!(
            total_commands,
            command_infos().len(),
            "group-iterated command count must match registry infos count"
        );
    }

    #[test]
    fn command_groups_are_cached_once() {
        let first_groups = groups::all_command_groups();
        let second_groups = groups::all_command_groups();
        assert!(
            std::ptr::eq(first_groups.as_ptr(), second_groups.as_ptr()),
            "command group list should be cached"
        );

        for &group in first_groups {
            let first_commands = group.commands();
            let second_commands = group.commands();
            assert!(
                std::ptr::eq(first_commands.as_ptr(), second_commands.as_ptr()),
                "command list should be cached per group"
            );
        }
    }

    #[test]
    fn command_registry_metadata_is_complete_and_palette_safe() {
        for command in command_infos() {
            assert!(!command.name.is_empty(), "command name must not be empty");
            assert_eq!(
                command.name.trim(),
                command.name,
                "/{} command name must not need trimming",
                command.name
            );
            assert!(
                command
                    .name
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit()),
                "/{} command names must stay lowercase ASCII",
                command.name
            );

            let expected_usage_prefix = format!("/{}", command.name);
            assert!(
                command.usage.starts_with(&expected_usage_prefix),
                "/{} usage must start with its canonical slash command, got {:?}",
                command.name,
                command.usage
            );

            let description = command.description_for(Locale::En);
            assert!(
                !description.trim().is_empty(),
                "/{} must have non-empty English help text",
                command.name
            );

            let palette_command = command.palette_command();
            assert!(
                palette_command.starts_with(&expected_usage_prefix),
                "/{} palette command must use the canonical command, got {:?}",
                command.name,
                palette_command
            );
            assert_eq!(
                palette_command.ends_with(' '),
                command.requires_argument(),
                "/{} palette command spacing must match argument requirement",
                command.name
            );

            for &alias in command.aliases {
                assert!(
                    !alias.trim().is_empty(),
                    "/{} alias must not be empty",
                    command.name
                );
                assert_eq!(
                    alias.trim(),
                    alias,
                    "/{} alias /{alias} must not need trimming",
                    command.name
                );
                assert!(
                    !alias.starts_with('/'),
                    "/{} alias /{alias} must be stored without a slash",
                    command.name
                );
                assert!(
                    !alias.chars().any(char::is_whitespace),
                    "/{} alias /{alias} must not contain whitespace",
                    command.name
                );
                assert!(
                    !alias.chars().any(|ch| ch.is_ascii_uppercase()),
                    "/{} alias /{alias} must not contain uppercase ASCII",
                    command.name
                );
            }
        }
    }

    #[test]
    fn command_discovery_tier_lists_use_canonical_registered_names() {
        for (tier_name, names) in [
            ("advanced", traits::ADVANCED_DISCOVERY_COMMANDS),
            ("compatibility", traits::COMPATIBILITY_DISCOVERY_COMMANDS),
        ] {
            for &name in names {
                let info = registry()
                    .get_info(name)
                    .unwrap_or_else(|| panic!("{tier_name} discovery entry {name:?} must resolve"));
                assert_eq!(
                    info.name, name,
                    "{tier_name} discovery entry {name:?} must be canonical, not an alias for /{}",
                    info.name
                );
            }
        }
    }

    #[test]
    fn command_info_resolves_canonical_names_and_aliases() {
        for command in command_infos() {
            for lookup in [command.name.to_string(), format!("/{}", command.name)] {
                let resolved = get_command_info(&lookup)
                    .unwrap_or_else(|| panic!("{lookup:?} should resolve to /{}", command.name));
                assert_eq!(resolved.name, command.name);
            }

            for &alias in command.aliases {
                for lookup in [alias.to_string(), format!("/{alias}")] {
                    let resolved = get_command_info(&lookup).unwrap_or_else(|| {
                        panic!("{lookup:?} should resolve to /{}", command.name)
                    });
                    assert_eq!(resolved.name, command.name);
                }
            }
        }
    }

    #[test]
    fn every_registered_command_has_a_help_topic() {
        let mut app = create_test_app();
        for command in command_infos() {
            let result = execute(&format!("/help {}", command.name), &mut app);
            assert!(
                !result.is_error,
                "/help {} returned an error: {result:?}",
                command.name
            );
            let message = result
                .message
                .unwrap_or_else(|| panic!("/help {} should return text", command.name));
            assert!(
                message.contains(command.name),
                "/help {} should mention the command name, got {message:?}",
                command.name
            );
            assert!(
                message.contains(command.usage),
                "/help {} should include usage {:?}, got {message:?}",
                command.name,
                command.usage
            );
        }
    }

    #[test]
    fn context_command_opens_inspector_and_keeps_ctx_alias() {
        let context = command_infos()
            .into_iter()
            .find(|cmd| cmd.name == "context")
            .expect("context command should exist");
        assert_eq!(context.aliases, &["ctx"]);
        assert!(context.description_for(Locale::En).contains("inspector"));

        let mut app = create_test_app();
        let result = execute("/ctx", &mut app);
        assert!(matches!(
            result.action,
            Some(AppAction::OpenContextInspector)
        ));

        let report = execute("/context report", &mut app);
        let message = report.message.expect("context report should return text");
        assert!(message.contains("Context Source Map"));
    }

    #[test]
    fn cache_inspect_dispatches_through_cache_command() {
        let mut app = create_test_app();
        let result = execute("/cache inspect", &mut app);
        let msg = result.message.expect("cache inspect should return text");
        assert!(msg.contains("Cache Inspect"));
        assert!(msg.contains("Base static prefix hash:"));
        assert!(msg.contains("Full request prefix hash:"));
        assert!(result.action.is_none());
    }

    #[test]
    fn cache_warmup_dispatches_action() {
        let mut app = create_test_app();
        let result = execute("/cache warmup", &mut app);
        assert!(result.message.is_none());
        assert!(matches!(result.action, Some(AppAction::CacheWarmup)));
    }

    #[test]
    fn execute_config_opens_config_view_action() {
        let mut app = create_test_app();
        let result = execute("/config", &mut app);
        assert!(result.message.is_none());
        assert!(matches!(result.action, Some(AppAction::OpenConfigView)));
    }

    #[test]
    fn execute_verbose_toggles_live_transcript_detail() {
        let mut app = create_test_app();
        assert!(!app.verbose_transcript);

        let result = execute("/verbose on", &mut app);
        assert!(!result.is_error);
        assert!(app.verbose_transcript);
        assert!(result.message.unwrap().contains("on"));

        let result = execute("/verbose off", &mut app);
        assert!(!result.is_error);
        assert!(!app.verbose_transcript);
        assert!(result.message.unwrap().contains("off"));
    }

    #[test]
    fn voice_send_and_voice_control_commands_toggle_state() {
        let mut app = create_test_app();
        assert!(!app.voice_send_enabled);
        assert!(!app.voice_control_enabled);

        for invocation in ["/voicesend", "/voice-send", "/yuyinsend", "/语音发送"] {
            let result = execute(invocation, &mut app);
            assert!(!result.is_error, "{invocation} should toggle cleanly");
            assert!(result.action.is_none());
            assert!(result.message.is_some());
        }
        // Four toggles land back at disabled.
        assert!(!app.voice_send_enabled);

        let result = execute("/voicecontrol", &mut app);
        assert!(!result.is_error);
        assert!(app.voice_control_enabled);
        let result = execute("/voice-control", &mut app);
        assert!(!result.is_error);
        assert!(!app.voice_control_enabled);
    }

    /// `/voice` defers the actual capture to the UI event loop via
    /// `AppAction::VoiceCapture`, so executing it never records audio.
    /// On hosts without a recorder it must fail gracefully instead.
    #[test]
    fn voice_command_toggles_on_and_off_or_fails_gracefully() {
        let mut app = create_test_app();
        let result = execute("/voice", &mut app);
        if app.voice_enabled {
            assert!(!result.is_error);
            assert!(matches!(result.action, Some(AppAction::VoiceCapture)));
            let off = execute("/voice", &mut app);
            assert!(!off.is_error);
            assert!(off.action.is_none());
            assert!(!app.voice_enabled);
        } else {
            assert!(result.is_error);
            assert!(result.action.is_none());
        }
    }

    #[test]
    fn execute_sidebar_toggles_visibility() {
        let mut app = create_test_app();
        app.set_sidebar_focus(SidebarFocus::Pinned);
        app.last_sidebar_host_width = Some(120);

        let result = execute("/sidebar", &mut app);
        assert!(!result.is_error);
        assert_eq!(app.sidebar_focus, SidebarFocus::Hidden);
        assert!(app.status_message.is_none());
        assert_eq!(result.message.as_deref(), Some("Sidebar is hidden"));

        let result = execute("/sidebar", &mut app);
        assert!(!result.is_error);
        assert_eq!(app.sidebar_focus, SidebarFocus::Pinned);
        assert!(app.status_message.is_none());
        assert_eq!(result.message.as_deref(), Some("Sidebar is visible"));
    }

    #[test]
    fn execute_sidebar_accepts_explicit_focus_targets() {
        let mut app = create_test_app();
        app.last_sidebar_host_width = Some(120);

        let result = execute("/sidebar tasks", &mut app);
        assert!(!result.is_error);
        assert_eq!(app.sidebar_focus, SidebarFocus::Tasks);
        assert!(app.status_message.is_none());

        let result = execute("/sidebar activity", &mut app);
        assert!(!result.is_error);
        assert_eq!(
            app.sidebar_focus,
            SidebarFocus::Tasks,
            "activity is the user-facing alias for the Activity panel"
        );

        let result = execute("/sidebar off", &mut app);
        assert!(!result.is_error);
        assert_eq!(app.sidebar_focus, SidebarFocus::Hidden);
        assert!(app.status_message.is_none());

        let result = execute("/sidebar closed", &mut app);
        assert!(!result.is_error);
        assert_eq!(app.sidebar_focus, SidebarFocus::Hidden);
        assert!(app.status_message.is_none());

        let result = execute("/sidebar none", &mut app);
        assert!(!result.is_error);
        assert_eq!(app.sidebar_focus, SidebarFocus::Hidden);
        assert!(app.status_message.is_none());

        let result = execute("/sidebar on", &mut app);
        assert!(!result.is_error);
        assert_eq!(app.sidebar_focus, SidebarFocus::Pinned);
        assert!(app.status_message.is_none());
    }

    #[test]
    fn execute_sidebar_rejects_invalid_args() {
        let mut app = create_test_app();
        let result = execute("/sidebar maybe", &mut app);
        assert!(result.is_error);
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("Usage: /sidebar")
        );
    }

    #[test]
    fn execute_links_and_aliases_return_links_message() {
        let mut app = create_test_app();
        for cmd in ["/links", "/dashboard", "/api", "/lianjie"] {
            let result = execute(cmd, &mut app);
            let msg = result.message.expect("links commands should return text");
            assert!(msg.contains("https://platform.deepseek.com"));
            assert!(result.action.is_none());
        }
    }

    #[test]
    fn execute_workspace_alias_switches_workspace() {
        let dir = tempdir().expect("temp dir");
        let mut app = create_test_app();
        let result = execute(&format!("/cwd {}", dir.path().display()), &mut app);
        assert!(matches!(
            result.action,
            Some(AppAction::SwitchWorkspace { workspace }) if workspace == dir.path().canonicalize().unwrap()
        ));
    }

    #[test]
    fn removed_set_and_deepseek_commands_show_migration_hints() {
        let mut app = create_test_app();
        let set_result = execute("/set model deepseek-v4-pro", &mut app);
        let set_msg = set_result
            .message
            .expect("legacy command should return an error message");
        assert!(set_msg.contains("The /set command was retired"));
        assert!(set_msg.contains("/config"));
        assert!(set_msg.contains("/settings"));
        assert!(set_result.action.is_none());

        let deepseek_result = execute("/deepseek", &mut app);
        let deepseek_msg = deepseek_result
            .message
            .expect("legacy command should return an error message");
        assert!(deepseek_msg.contains("The /deepseek command was renamed"));
        assert!(deepseek_msg.contains("/links"));
        assert!(deepseek_msg.contains("/dashboard"));
        assert!(deepseek_msg.contains("/api"));
        assert!(deepseek_result.action.is_none());
    }

    struct ConfigPathGuard {
        previous: Option<OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl ConfigPathGuard {
        fn new(config_path: &Path) -> Self {
            let lock = crate::test_support::lock_test_env();
            let previous = std::env::var_os("DEEPSEEK_CONFIG_PATH");
            // Safety: test-only environment mutation guarded by a global mutex.
            unsafe {
                std::env::set_var("DEEPSEEK_CONFIG_PATH", config_path);
            }
            Self {
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for ConfigPathGuard {
        fn drop(&mut self) {
            // Safety: test-only environment mutation guarded by a global mutex.
            unsafe {
                if let Some(previous) = self.previous.take() {
                    std::env::set_var("DEEPSEEK_CONFIG_PATH", previous);
                } else {
                    std::env::remove_var("DEEPSEEK_CONFIG_PATH");
                }
            }
        }
    }

    /// Build an App scoped to an isolated tempdir so dispatch-side-effects
    /// (e.g. `/init` writing AGENTS.md, `/export` writing chat transcripts,
    /// `/logout` clearing credentials) don't pollute the repo working tree or
    /// the developer's real config when the smoke tests run.
    fn create_isolated_test_app() -> (App, tempfile::TempDir, ConfigPathGuard) {
        let tmpdir = tempfile::TempDir::new().expect("tempdir for smoke test");
        let workspace = tmpdir.path().to_path_buf();
        let config_path = workspace.join(".deepseek").join("config.toml");
        std::fs::create_dir_all(config_path.parent().expect("config parent")).expect("config dir");
        let guard = ConfigPathGuard::new(&config_path);
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: workspace.clone(),
            config_path: Some(config_path),
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: workspace.join("skills"),
            memory_path: workspace.join("memory.md"),
            notes_path: workspace.join("notes.txt"),
            mcp_config_path: workspace.join("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let app = App::new(options, &Config::default());
        (app, tmpdir, guard)
    }

    /// Smoke test: every entry in `command_infos()` must dispatch to a real handler.
    /// A dispatch miss surfaces as the fall-through `Unknown command:` error
    /// message in `execute`. This catches the case where a new command is
    /// added to `command_infos()` (so it shows up in `/help` and the palette) but
    /// the matching arm in `execute` is forgotten — the user would type the
    /// command, see it autocomplete, and then get an unhelpful "did you
    /// mean" suggestion. Also catches panics in handlers because the test
    /// runner unwinds the panic and reports the offending command.
    /// `/save` and `/export` default their output paths to `cwd`-relative
    /// filenames when no arg is supplied, which would scribble files into
    /// `crates/tui/` when CI runs from there. Pass an explicit tempdir-
    /// relative path for those two so the dispatch test stays sandboxed.
    fn invocation_for(command_name: &str, alias_or_name: &str, tmpdir: &std::path::Path) -> String {
        match command_name {
            "save" => format!("/{alias_or_name} {}", tmpdir.join("session.json").display()),
            "export" => format!("/{alias_or_name} {}", tmpdir.join("chat.md").display()),
            _ => format!("/{alias_or_name}"),
        }
    }

    /// `/restore` is covered by its own dedicated tests in
    /// `commands/restore.rs` that serialize on the global env mutex via
    /// `scoped_home` (snapshot repo init shells out to git, which races
    /// against parallel-running tests). Skip it here so this smoke test
    /// stays parallel-safe.
    fn skip_in_dispatch_smoke(name: &str) -> bool {
        name == "restore"
    }

    #[test]
    fn slash_parser_preserves_arguments_after_the_command_name() {
        let mut app = create_test_app();
        let result = execute("/agent 2 review   this   carefully", &mut app);
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected /agent to send a model instruction");
        };
        assert!(message.contains(r#"prompt: "review   this   carefully""#));
        assert!(message.contains("max_depth: 2"));

        let mut app = create_test_app();
        let result = execute("   /relay   ship   command   harness   ", &mut app);
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected /relay to send a model instruction");
        };
        assert!(message.contains("Requested relay focus: ship   command   harness"));

        let mut app = create_test_app();
        let result = execute("/rlm 3 inspect   this   corpus", &mut app);
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected /rlm to send a model instruction");
        };
        assert!(message.contains(r#"content: "inspect   this   corpus""#));
        assert!(message.contains("sub_rlm_max_depth: 3"));
    }

    #[test]
    fn representative_command_groups_keep_dispatch_surfaces() {
        let mut app = create_test_app();
        let help = execute("/help clear", &mut app)
            .message
            .expect("/help clear should return text");
        assert!(help.contains("clear"));
        assert!(help.contains("/clear"));

        let mut app = create_test_app();
        let result = execute("/config", &mut app);
        assert!(matches!(result.action, Some(AppAction::OpenConfigView)));

        let mut app = create_test_app();
        let result = execute("/relay command boundary", &mut app);
        assert!(!result.is_error);
        assert!(matches!(
            result.action,
            Some(AppAction::SendMessage(message))
                if message.contains("Requested relay focus: command boundary")
        ));

        let mut app = create_test_app();
        let note_help = execute("/note help", &mut app)
            .message
            .expect("/note help should return text");
        assert!(note_help.contains("Usage: /note"));

        let mut app = create_test_app();
        let result = execute("/hunt ship layer 2 | budget: 100", &mut app);
        assert!(!result.is_error);
        assert_eq!(app.hunt.quarry.as_deref(), Some("ship layer 2"));
        assert_eq!(app.hunt.token_budget, Some(100));

        let (mut app, _tmpdir, _guard) = create_isolated_test_app();
        let skills = execute("/skills", &mut app)
            .message
            .expect("/skills should return text");
        assert!(skills.contains("Skills location:"));

        let mut app = create_test_app();
        let result = execute("/task list", &mut app);
        assert!(matches!(result.action, Some(AppAction::TaskList)));

        let mut app = create_test_app();
        let tokens = execute("/tokens", &mut app)
            .message
            .expect("/tokens should return text");
        assert!(tokens.contains("deepseek-v4-pro"));
    }

    /// Smoke test: every entry in `command_infos()` must dispatch to a real handler.
    /// A dispatch miss surfaces as the fall-through `Unknown command:` error
    /// message in `execute`. This catches the case where a new command is
    /// added to `command_infos()` (so it shows up in `/help` and the palette) but
    /// the matching arm in `execute` is forgotten — the user would type the
    /// command, see it autocomplete, and then get an unhelpful "did you
    /// mean" suggestion. Also catches panics in handlers because the test
    /// runner unwinds the panic and reports the offending command.
    #[test]
    fn every_registered_command_dispatches_to_a_handler() {
        for command in command_infos() {
            if skip_in_dispatch_smoke(command.name) {
                continue;
            }
            let (mut app, tmpdir, _guard) = create_isolated_test_app();
            let invocation = invocation_for(command.name, command.name, tmpdir.path());
            let result = execute(&invocation, &mut app);
            if let Some(msg) = &result.message {
                assert!(
                    !msg.contains("Unknown command"),
                    "/{} fell through to the unknown-command branch: {msg}",
                    command.name,
                );
            }
        }
    }

    /// Same check, but for declared aliases — `/q` should not fall through
    /// just because the registry lists it as an alias of `/exit`.
    #[test]
    fn every_command_alias_dispatches_to_a_handler() {
        for command in command_infos() {
            if skip_in_dispatch_smoke(command.name) {
                continue;
            }
            for alias in command.aliases {
                let (mut app, tmpdir, _guard) = create_isolated_test_app();
                let invocation = invocation_for(command.name, alias, tmpdir.path());
                let result = execute(&invocation, &mut app);
                if let Some(msg) = &result.message {
                    assert!(
                        !msg.contains("Unknown command"),
                        "/{alias} (alias of /{}) fell through to unknown: {msg}",
                        command.name,
                    );
                }
            }
        }
    }

    #[test]
    fn balance_command_has_own_help_text() {
        let info = get_command_info("balance").expect("balance command should be registered");
        assert_eq!(info.description_id, MessageId::CmdBalanceDescription);
        assert!(
            info.description_for(Locale::En)
                .contains("provider account balance")
        );
    }

    #[test]
    fn balance_command_reports_scaffold_without_claiming_dispatch() {
        let mut app = create_test_app();
        app.api_provider = ApiProvider::Deepseek;

        let result = execute("/balance", &mut app);
        let msg = result
            .message
            .expect("balance scaffold should explain current state");

        assert!(!result.is_error);
        assert!(msg.contains("DeepSeek"));
        assert!(msg.contains("not wired"));
        assert!(!msg.contains("sent"));
    }

    #[test]
    fn balance_command_reports_unsupported_provider_clearly() {
        let mut app = create_test_app();
        app.api_provider = ApiProvider::Ollama;

        let result = execute("/balance", &mut app);
        let msg = result
            .message
            .expect("unsupported providers should return a clear message");

        assert!(!result.is_error);
        assert!(msg.contains("Ollama"));
        assert!(msg.contains("not supported"));
        assert!(msg.contains("dashboard"));
    }

    #[test]
    fn unknown_command_suggests_nearest_match() {
        let mut app = create_test_app();
        let result = execute("/modle", &mut app);
        let msg = result
            .message
            .expect("unknown command should return an error message");
        assert!(msg.contains("Unknown command: /modle"));
        assert!(msg.contains("Did you mean:"));
        assert!(msg.contains("/model"));
    }

    #[test]
    fn unknown_command_without_close_match_keeps_help_guidance() {
        let mut app = create_test_app();
        let result = execute("/zzzzzz", &mut app);
        let msg = result
            .message
            .expect("unknown command should return an error message");
        assert!(msg.contains("Unknown command: /zzzzzz"));
        assert!(msg.contains("Type /help for available commands."));
    }

    #[test]
    fn dollar_skill_prefix_with_no_name_shows_usage() {
        let mut app = create_test_app();
        let result = execute("$", &mut app);
        assert!(result.is_error);
        let msg = result.message.expect("should return error message");
        assert!(msg.contains("Type a skill name after $"));
    }

    #[test]
    fn dollar_skill_prefix_unknown_skill_reports_unknown_skill() {
        let mut app = create_test_app();
        let result = execute("$definitely-not-a-real-skill-12345", &mut app);
        assert!(result.is_error);
        let msg = result.message.expect("should return error message");
        assert!(msg.contains("Unknown skill: $definitely-not-a-real-skill-12345"));
        assert!(msg.contains("/skills"));
    }

    #[test]
    fn dollar_skill_prefix_does_not_break_existing_slash_dispatch() {
        let mut app = create_test_app();
        let result = execute("/help", &mut app);
        assert!(!result.is_error);
    }
}
