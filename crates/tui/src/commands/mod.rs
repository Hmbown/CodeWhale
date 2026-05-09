//! Slash command registry and dispatch system
//!
//! This module provides a modular command system inspired by Codex-rs.
//! Commands are organized by category and dispatched through a central registry.

mod anchor;
mod attachment;
mod config;
mod core;
mod cycle;
mod debug;
mod goal;
mod hooks;
mod init;
mod jobs;
mod mcp;
mod memory;
mod network;
mod note;
mod provider;
mod queue;
mod rename;
mod restore;
mod review;
mod session;
pub mod share;
mod skills;
mod stash;
mod task;
mod user_commands;

use crate::localization::{Locale, MessageId, tr};
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
    #[allow(dead_code)]
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

/// Command metadata for help and autocomplete.
///
/// The English description lives in `localization::english` (private), keyed
/// by `description_id`. Callers resolve a localized description through
/// [`CommandInfo::description_for`] which delegates to
/// [`crate::localization::tr`].
#[derive(Debug, Clone, Copy)]
pub struct CommandInfo {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub description_id: MessageId,
}

impl CommandInfo {
    pub fn requires_argument(&self) -> bool {
        self.usage.contains('<') || self.usage.contains('[')
    }

    pub fn palette_command(&self) -> String {
        if self.requires_argument() {
            format!("/{} ", self.name)
        } else {
            format!("/{}", self.name)
        }
    }

    pub fn description_for(&self, locale: Locale) -> &'static str {
        tr(locale, self.description_id)
    }

    pub fn palette_description_for(&self, locale: Locale) -> String {
        let desc = self.description_for(locale);
        if self.aliases.is_empty() {
            desc.to_string()
        } else {
            format!("{}  aliases: {}", desc, self.aliases.join(", "))
        }
    }
}

/// All registered commands
pub const COMMANDS: &[CommandInfo] = &[
    // Core commands
    CommandInfo {
        name: "anchor",
        aliases: &[],
        usage: "/anchor <text> | /anchor list | /anchor remove <n>",
        description_id: MessageId::CmdAnchorDescription,
    },
    CommandInfo {
        name: "help",
        aliases: &["?"],
        usage: "/help [command]",
        description_id: MessageId::CmdHelpDescription,
    },
    CommandInfo {
        name: "clear",
        aliases: &[],
        usage: "/clear",
        description_id: MessageId::CmdClearDescription,
    },
    CommandInfo {
        name: "exit",
        aliases: &["quit", "q"],
        usage: "/exit",
        description_id: MessageId::CmdExitDescription,
    },
    CommandInfo {
        name: "model",
        aliases: &[],
        usage: "/model [name]",
        description_id: MessageId::CmdModelDescription,
    },
    CommandInfo {
        name: "models",
        aliases: &[],
        usage: "/models",
        description_id: MessageId::CmdModelsDescription,
    },
    CommandInfo {
        name: "provider",
        aliases: &[],
        usage: "/provider [name]",
        description_id: MessageId::CmdProviderDescription,
    },
    CommandInfo {
        name: "queue",
        aliases: &["queued"],
        usage: "/queue [list|edit <n>|drop <n>|clear]",
        description_id: MessageId::CmdQueueDescription,
    },
    CommandInfo {
        name: "stash",
        aliases: &["park"],
        usage: "/stash [list|pop|clear]",
        description_id: MessageId::CmdStashDescription,
    },
    CommandInfo {
        name: "hooks",
        aliases: &["hook"],
        usage: "/hooks [list|events]",
        description_id: MessageId::CmdHooksDescription,
    },
    CommandInfo {
        name: "subagents",
        aliases: &["agents"],
        usage: "/subagents",
        description_id: MessageId::CmdSubagentsDescription,
    },
    CommandInfo {
        name: "links",
        aliases: &["dashboard", "api"],
        usage: "/links",
        description_id: MessageId::CmdLinksDescription,
    },
    CommandInfo {
        name: "home",
        aliases: &["stats", "overview"],
        usage: "/home",
        description_id: MessageId::CmdHomeDescription,
    },
    CommandInfo {
        name: "note",
        aliases: &[],
        usage: "/note <text>",
        description_id: MessageId::CmdNoteDescription,
    },
    CommandInfo {
        name: "memory",
        aliases: &[],
        usage: "/memory [show|path|clear|edit|help]",
        description_id: MessageId::CmdMemoryDescription,
    },
    CommandInfo {
        name: "attach",
        aliases: &["image", "media"],
        usage: "/attach <path>",
        description_id: MessageId::CmdAttachDescription,
    },
    CommandInfo {
        name: "task",
        aliases: &["tasks"],
        usage: "/task [add <prompt>|list|show <id>|cancel <id>]",
        description_id: MessageId::CmdTaskDescription,
    },
    CommandInfo {
        name: "jobs",
        aliases: &["job"],
        usage: "/jobs [list|show <id>|poll <id>|wait <id>|stdin <id> <input>|cancel <id>]",
        description_id: MessageId::CmdJobsDescription,
    },
    CommandInfo {
        name: "mcp",
        aliases: &[],
        usage: "/mcp [init|add stdio <name> <command> [args...]|add http <name> <url>|enable <name>|disable <name>|remove <name>|validate|reload]",
        description_id: MessageId::CmdMcpDescription,
    },
    CommandInfo {
        name: "network",
        aliases: &[],
        usage: "/network [list|allow <host>|deny <host>|remove <host>|default <allow|deny|prompt>]",
        description_id: MessageId::CmdNetworkDescription,
    },
    // Session commands
    CommandInfo {
        name: "rename",
        aliases: &[],
        usage: "/rename <new title>",
        description_id: MessageId::CmdRenameDescription,
    },
    CommandInfo {
        name: "save",
        aliases: &[],
        usage: "/save [path]",
        description_id: MessageId::CmdSaveDescription,
    },
    CommandInfo {
        name: "sessions",
        aliases: &["resume"],
        usage: "/sessions [show|prune <days>]",
        description_id: MessageId::CmdSessionsDescription,
    },
    CommandInfo {
        name: "load",
        aliases: &[],
        usage: "/load [path]",
        description_id: MessageId::CmdLoadDescription,
    },
    CommandInfo {
        name: "compact",
        aliases: &[],
        usage: "/compact",
        description_id: MessageId::CmdCompactDescription,
    },
    CommandInfo {
        name: "context",
        aliases: &["ctx"],
        usage: "/context",
        description_id: MessageId::CmdContextDescription,
    },
    CommandInfo {
        name: "cycles",
        aliases: &[],
        usage: "/cycles",
        description_id: MessageId::CmdCyclesDescription,
    },
    CommandInfo {
        name: "cycle",
        aliases: &[],
        usage: "/cycle <n>",
        description_id: MessageId::CmdCycleDescription,
    },
    CommandInfo {
        name: "recall",
        aliases: &[],
        usage: "/recall <query>",
        description_id: MessageId::CmdRecallDescription,
    },
    CommandInfo {
        name: "export",
        aliases: &[],
        usage: "/export [path]",
        description_id: MessageId::CmdExportDescription,
    },
    // Config commands
    CommandInfo {
        name: "config",
        aliases: &[],
        usage: "/config",
        description_id: MessageId::CmdConfigDescription,
    },
    CommandInfo {
        name: "yolo",
        aliases: &[],
        usage: "/yolo",
        description_id: MessageId::CmdYoloDescription,
    },
    CommandInfo {
        name: "agent",
        aliases: &[],
        usage: "/agent",
        description_id: MessageId::CmdAgentDescription,
    },
    CommandInfo {
        name: "plan",
        aliases: &[],
        usage: "/plan",
        description_id: MessageId::CmdPlanDescription,
    },
    CommandInfo {
        name: "theme",
        aliases: &[],
        usage: "/theme",
        description_id: MessageId::CmdThemeDescription,
    },
    CommandInfo {
        name: "verbose",
        aliases: &[],
        usage: "/verbose [on|off]",
        description_id: MessageId::CmdVerboseDescription,
    },
    CommandInfo {
        name: "trust",
        aliases: &[],
        usage: "/trust [on|off|add <path>|remove <path>|list]",
        description_id: MessageId::CmdTrustDescription,
    },
    CommandInfo {
        name: "logout",
        aliases: &[],
        usage: "/logout",
        description_id: MessageId::CmdLogoutDescription,
    },
    // Debug commands
    CommandInfo {
        name: "tokens",
        aliases: &[],
        usage: "/tokens",
        description_id: MessageId::CmdTokensDescription,
    },
    CommandInfo {
        name: "system",
        aliases: &[],
        usage: "/system",
        description_id: MessageId::CmdSystemDescription,
    },
    CommandInfo {
        name: "edit",
        aliases: &[],
        usage: "/edit",
        description_id: MessageId::CmdEditDescription,
    },
    CommandInfo {
        name: "diff",
        aliases: &[],
        usage: "/diff",
        description_id: MessageId::CmdDiffDescription,
    },
    CommandInfo {
        name: "undo",
        aliases: &[],
        usage: "/undo",
        description_id: MessageId::CmdUndoDescription,
    },
    CommandInfo {
        name: "retry",
        aliases: &[],
        usage: "/retry",
        description_id: MessageId::CmdRetryDescription,
    },
    CommandInfo {
        name: "init",
        aliases: &[],
        usage: "/init",
        description_id: MessageId::CmdInitDescription,
    },
    CommandInfo {
        name: "lsp",
        aliases: &[],
        usage: "/lsp [on|off|status]",
        description_id: MessageId::CmdLspDescription,
    },
    CommandInfo {
        name: "share",
        aliases: &[],
        usage: "/share",
        description_id: MessageId::CmdShareDescription,
    },
    CommandInfo {
        name: "goal",
        aliases: &[],
        usage: "/goal [objective] [budget: N]",
        description_id: MessageId::CmdGoalDescription,
    },
    CommandInfo {
        name: "settings",
        aliases: &[],
        usage: "/settings",
        description_id: MessageId::CmdSettingsDescription,
    },
    CommandInfo {
        name: "statusline",
        aliases: &["status"],
        usage: "/statusline",
        description_id: MessageId::CmdStatuslineDescription,
    },
    // Skills commands
    CommandInfo {
        name: "skills",
        aliases: &[],
        usage: "/skills [--remote|sync]",
        description_id: MessageId::CmdSkillsDescription,
    },
    CommandInfo {
        name: "skill",
        aliases: &[],
        usage: "/skill <name|install <spec>|update <name>|uninstall <name>|trust <name>>",
        description_id: MessageId::CmdSkillDescription,
    },
    CommandInfo {
        name: "review",
        aliases: &[],
        usage: "/review <target>",
        description_id: MessageId::CmdReviewDescription,
    },
    CommandInfo {
        name: "restore",
        aliases: &[],
        usage: "/restore [N]",
        description_id: MessageId::CmdRestoreDescription,
    },
    // RLM command
    CommandInfo {
        name: "rlm",
        aliases: &["recursive"],
        usage: "/rlm <prompt>",
        description_id: MessageId::CmdRlmDescription,
    },
    // Debug/cost command
    CommandInfo {
        name: "cost",
        aliases: &[],
        usage: "/cost",
        description_id: MessageId::CmdCostDescription,
    },
    // Profile switching (#390)
    CommandInfo {
        name: "profile",
        aliases: &[],
        usage: "/profile <name>",
        description_id: MessageId::CmdHelpDescription, // reuse for now
    },
    // Cache telemetry (#263)
    CommandInfo {
        name: "cache",
        aliases: &[],
        usage: "/cache [count|inspect|warmup]",
        description_id: MessageId::CmdCacheDescription,
    },
];

/// Execute a slash command
pub fn execute(cmd: &str, app: &mut App) -> CommandResult {
    let parts: Vec<&str> = cmd.trim().splitn(2, ' ').collect();
    let command = parts[0].to_lowercase();
    let command = command.strip_prefix('/').unwrap_or(&command);
    let arg = parts.get(1).map(|s| s.trim());

    // Check user-defined commands FIRST so they can override built-ins.
    if let Some(result) = user_commands::try_dispatch_user_command(app, cmd.trim()) {
        return result;
    }

    // Match command or alias
    match command {
        // Core commands
        "anchor" => anchor::anchor(app, arg),
        "help" | "?" => core::help(app, arg),
        "clear" => core::clear(app),
        "exit" | "quit" | "q" => core::exit(),
        "model" => core::model(app, arg),
        "models" => core::models(app),
        "provider" => provider::provider(app, arg),
        "queue" | "queued" => queue::queue(app, arg),
        "stash" | "park" => stash::stash(app, arg),
        "hooks" | "hook" => hooks::hooks(app, arg),
        "subagents" | "agents" => core::subagents(app),
        "links" | "dashboard" | "api" => core::deepseek_links(app),
        "home" | "stats" | "overview" => core::home_dashboard(app),
        "note" => note::note(app, arg),
        "memory" => memory::memory(app, arg),
        "attach" | "image" | "media" => attachment::attach(app, arg),
        "task" | "tasks" => task::task(app, arg),
        "jobs" | "job" => jobs::jobs(app, arg),
        "mcp" => mcp::mcp(app, arg),
        "network" => network::network(app, arg),

        // Session commands
        "rename" => rename::rename(app, arg),
        "save" => session::save(app, arg),
        "sessions" | "resume" => session::sessions(app, arg),
        "load" => session::load(app, arg),
        "compact" => session::compact(app),
        "cycles" => cycle::list_cycles(app),
        "cycle" => cycle::show_cycle(app, arg),
        "recall" => cycle::recall_archive(app, arg),
        "export" => session::export(app, arg),

        // Config commands
        "config" => config::config_command(app, arg),
        "settings" => config::show_settings(app),
        "statusline" | "status" => config::status_line(app),
        "yolo" => config::yolo(app),
        "agent" => config::agent_mode(app),
        "plan" => config::plan_mode(app),
        "theme" => config::theme(app),
        "verbose" => config::verbose(app, arg),
        "trust" => config::trust(app, arg),
        "logout" => config::logout(app),

        // Debug commands
        "tokens" => debug::tokens(app),
        "cost" => debug::cost(app),
        "cache" => debug::cache(app, arg),
        "system" => debug::system_prompt(app),
        "context" | "ctx" => debug::context(app),
        "edit" => debug::edit(app),
        "diff" => debug::diff(app),
        "undo" => {
            // Try surgical patch-undo first; fall back to conversation undo
            // if no snapshots are available or if the snapshot undo couldn't
            // find anything useful.
            let result = debug::patch_undo(app);
            if result.message.as_deref().is_none_or(|m| {
                m.starts_with("No snapshots found")
                    || m.starts_with("No tool or pre-turn")
                    || m.starts_with("Snapshot repo")
            }) {
                debug::undo_conversation(app)
            } else {
                result
            }
        }
        "retry" => debug::retry(app),

        // Project commands
        "init" => init::init(app),
        "lsp" => config::lsp_command(app, arg),
        "share" => share::share(app, arg),
        "goal" => goal::goal(app, arg),

        // Skills commands
        "skills" => skills::list_skills(app, arg),
        "skill" => skills::run_skill(app, arg),
        "review" => review::review(app, arg),
        "restore" => restore::restore(app, arg),

        // Profile switch (#390)
        "profile" => core::profile_switch(app, arg),

        // RLM command
        "rlm" | "recursive" => rlm(app, arg),

        // Legacy command migrations (kept out of registry/autocomplete intentionally).
        "set" => CommandResult::error(
            "The /set command was retired. Use /config to edit settings and /settings to inspect current values.",
        ),
        "normal" => config::normal_mode(app),
        "deepseek" => CommandResult::error(
            "The /deepseek command was renamed. Use /links (aliases: /dashboard, /api).",
        ),

        _ => {
            // Third source: skills (lowest precedence after native and user-config).
            // Try to run a skill whose name matches the command.
            if skills::run_skill_by_name(app, command, arg).is_some() {
                return skills::run_skill_by_name(app, command, arg).unwrap();
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
    }
}

/// Update a configuration value programmatically (used by interactive UI views).
pub fn set_config_value(app: &mut App, key: &str, value: &str, persist: bool) -> CommandResult {
    config::set_config_value(app, key, value, persist)
}

/// Persist the user's chosen footer items to `~/.deepseek/config.toml` under
/// `tui.status_items`. See [`config::persist_status_items`] for details.
pub fn persist_status_items(
    items: &[crate::config::StatusItem],
) -> anyhow::Result<std::path::PathBuf> {
    config::persist_status_items(items)
}

/// Persist a root-level string key in `config.toml`.
pub fn persist_root_string_key(key: &str, value: &str) -> anyhow::Result<std::path::PathBuf> {
    config::persist_root_string_key(key, value)
}

/// Auto-select a model based on request complexity.
pub fn auto_model_heuristic(input: &str, current_model: &str) -> String {
    config::auto_model_heuristic(input, current_model)
}

pub use config::{
    AutoRouteRecommendation, AutoRouteSelection, normalize_auto_route_effort,
    parse_auto_route_recommendation, resolve_auto_route_with_flash,
};

/// Execute a Recursive Language Model (RLM) turn — Algorithm 1 from
/// Zhang et al. (arXiv:2512.24601).
///
/// The user's prompt text is passed as the argument. It will be stored
/// in the REPL as the `PROMPT` variable. The root LLM will only see
/// metadata about the REPL state, never the prompt text directly.
pub fn rlm(app: &mut App, arg: Option<&str>) -> CommandResult {
    let prompt = match arg {
        Some(p) if !p.trim().is_empty() => p.trim().to_string(),
        _ => {
            return CommandResult::error(
                "Usage: /rlm <prompt>\n\n\
                 Process a prompt using a Recursive Language Model (RLM).\n\
                 The prompt is stored in a REPL and the model writes code\n\
                 to decompose and process it recursively."
                    .to_string(),
            );
        }
    };

    // Sanity-check: RLM is most useful for longer prompts.
    if prompt.len() < 50 {
        return CommandResult::message(
            "Tip: RLM is designed for processing LONG prompts (>100 chars). \
             For short queries, just type the message directly."
                .to_string(),
        );
    }

    let model = app.model.clone();
    let child_model = "deepseek-v4-flash".to_string();
    // Paper experiments use depth=1 (one level of `sub_rlm`); we default to
    // depth=2 so the model can recurse twice if it chooses to.
    let max_depth: u32 = 2;

    CommandResult::with_message_and_action(
        format!(
            "Starting RLM turn for {} chars of prompt using {} (child={}, depth={})...",
            prompt.len(),
            model,
            child_model,
            max_depth,
        ),
        AppAction::Rlm {
            prompt,
            model,
            child_model,
            max_depth,
        },
    )
}

/// Get command info by name or alias
pub fn get_command_info(name: &str) -> Option<&'static CommandInfo> {
    let name = name.strip_prefix('/').unwrap_or(name);
    COMMANDS
        .iter()
        .find(|cmd| cmd.name == name || cmd.aliases.contains(&name))
}

/// Get all command names matching a prefix, including both built-in
/// static commands and user-defined commands, formatted as `/name`.
///
/// `workspace` is used to also scan workspace-local command directories;
/// pass `None` when no workspace context is available.
pub fn all_command_names_matching(
    prefix: &str,
    workspace: Option<&std::path::Path>,
) -> Vec<String> {
    let prefix = prefix.strip_prefix('/').unwrap_or(prefix).to_lowercase();
    let mut result: Vec<String> = COMMANDS
        .iter()
        .filter(|cmd| {
            cmd.name.starts_with(&prefix) || cmd.aliases.iter().any(|a| a.starts_with(&prefix))
        })
        .map(|cmd| format!("/{}", cmd.name))
        .collect();

    // Add user-defined commands
    result.extend(user_commands::user_commands_matching(&prefix, workspace));

    result.sort();
    result.dedup();
    result
}

/// Get all commands matching a prefix (for autocomplete)
#[allow(dead_code)]
pub fn commands_matching(prefix: &str) -> Vec<&'static CommandInfo> {
    let prefix = prefix.strip_prefix('/').unwrap_or(prefix).to_lowercase();
    COMMANDS
        .iter()
        .filter(|cmd| {
            cmd.name.starts_with(&prefix) || cmd.aliases.iter().any(|a| a.starts_with(&prefix))
        })
        .collect()
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

fn suggest_command_names(input: &str, limit: usize) -> Vec<String> {
    let query = input.trim().to_ascii_lowercase();
    if query.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut scored: Vec<(u8, usize, String)> = Vec::new();
    for command in COMMANDS {
        let mut best: Option<(u8, usize)> = None;
        for candidate in std::iter::once(command.name).chain(command.aliases.iter().copied()) {
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
    use crate::config::Config;
    use crate::tui::app::{App, AppAction, TuiOptions};
    use std::path::PathBuf;

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
    fn command_registry_contains_config_and_links_but_not_set_or_deepseek() {
        assert!(COMMANDS.iter().any(|cmd| cmd.name == "config"));
        assert!(COMMANDS.iter().any(|cmd| cmd.name == "links"));
        assert!(COMMANDS.iter().any(|cmd| cmd.name == "memory"));
        assert!(!COMMANDS.iter().any(|cmd| cmd.name == "set"));
        assert!(!COMMANDS.iter().any(|cmd| cmd.name == "deepseek"));
    }

    #[test]
    fn links_command_has_dashboard_and_api_aliases() {
        let links = COMMANDS
            .iter()
            .find(|cmd| cmd.name == "links")
            .expect("links command should exist");
        assert_eq!(links.aliases, &["dashboard", "api"]);
    }

    #[test]
    fn command_registry_has_unique_names_and_aliases() {
        let mut names = std::collections::BTreeSet::new();
        for command in COMMANDS {
            assert!(
                names.insert(command.name),
                "duplicate command name /{}",
                command.name
            );
        }

        let mut aliases = std::collections::BTreeSet::new();
        for command in COMMANDS {
            for alias in command.aliases {
                assert!(
                    !names.contains(alias),
                    "alias /{} collides with a command name",
                    alias
                );
                assert!(aliases.insert(*alias), "duplicate command alias /{alias}");
            }
        }
    }

    #[test]
    fn context_command_opens_inspector_and_keeps_ctx_alias() {
        let context = COMMANDS
            .iter()
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
    fn execute_links_and_aliases_return_links_message() {
        let mut app = create_test_app();
        for cmd in ["/links", "/dashboard", "/api"] {
            let result = execute(cmd, &mut app);
            let msg = result.message.expect("links commands should return text");
            assert!(msg.contains("https://platform.deepseek.com"));
            assert!(result.action.is_none());
        }
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

    /// Build an App scoped to an isolated tempdir so dispatch-side-effects
    /// (e.g. `/init` writing AGENTS.md, `/export` writing chat transcripts)
    /// don't pollute the repo working tree when the smoke tests run.
    fn create_isolated_test_app() -> (App, tempfile::TempDir) {
        let tmpdir = tempfile::TempDir::new().expect("tempdir for smoke test");
        let workspace = tmpdir.path().to_path_buf();
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: workspace.clone(),
            config_path: None,
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
        (app, tmpdir)
    }

    /// Smoke test: every entry in `COMMANDS` must dispatch to a real handler.
    /// A dispatch miss surfaces as the fall-through `Unknown command:` error
    /// message in `execute`. This catches the case where a new command is
    /// added to `COMMANDS` (so it shows up in `/help` and the palette) but
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

    /// Smoke test: every entry in `COMMANDS` must dispatch to a real handler.
    /// A dispatch miss surfaces as the fall-through `Unknown command:` error
    /// message in `execute`. This catches the case where a new command is
    /// added to `COMMANDS` (so it shows up in `/help` and the palette) but
    /// the matching arm in `execute` is forgotten — the user would type the
    /// command, see it autocomplete, and then get an unhelpful "did you
    /// mean" suggestion. Also catches panics in handlers because the test
    /// runner unwinds the panic and reports the offending command.
    #[test]
    fn every_registered_command_dispatches_to_a_handler() {
        for command in COMMANDS {
            if skip_in_dispatch_smoke(command.name) {
                continue;
            }
            let (mut app, tmpdir) = create_isolated_test_app();
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
        for command in COMMANDS {
            if skip_in_dispatch_smoke(command.name) {
                continue;
            }
            for alias in command.aliases {
                let (mut app, tmpdir) = create_isolated_test_app();
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
}
