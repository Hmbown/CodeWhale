//! Slash command registry and dispatch system
//!
//! This module provides a modular command system inspired by Codex-rs.
//! Commands are organized by category and dispatched through a central registry.

mod anchor;
mod attachment;
mod balance;
mod change;
mod config;
mod core;
mod debug;
mod feedback;
mod goal;
mod hf;
mod hooks;
mod init;
mod jobs;
mod mcp;
mod memory;
mod network;
mod note;
mod parse;
mod provider;
mod queue;
mod registry;
mod rename;
mod restore;
mod review;
mod session;
pub mod share;
mod skills;
mod stash;
mod status;
mod task;
pub mod user_commands;

use std::fmt::Write as _;

use parse::parse_slash_command;
use registry::suggest_command_names;
pub use registry::{COMMANDS, get_command_info};

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

/// Execute a slash command
pub fn execute(cmd: &str, app: &mut App) -> CommandResult {
    let parsed = parse_slash_command(cmd);
    let command = parsed.name.as_str();
    let arg = parsed.arg;

    // Check user-defined commands FIRST so they can override built-ins.
    if let Some(result) = user_commands::try_dispatch_user_command(app, cmd.trim()) {
        return result;
    }

    // Match command or alias
    match command {
        // Core commands
        "anchor" | "maodian" => anchor::anchor(app, arg),
        "help" | "?" | "bangzhu" | "帮助" => core::help(app, arg),
        "clear" | "qingping" => core::clear(app),
        "exit" | "quit" | "q" | "tuichu" => core::exit(),
        "model" | "moxing" => core::model(app, arg),
        "models" | "moxingliebiao" => core::models(app),
        "provider" => provider::provider(app, arg),
        "queue" | "queued" => queue::queue(app, arg),
        "stash" | "park" => stash::stash(app, arg),
        "hooks" | "hook" | "gouzi" => hooks::hooks(app, arg),
        "subagents" | "agents" | "zhinengti" => core::subagents(app),
        "agent" | "daili" => agent(app, arg),
        "links" | "dashboard" | "api" | "lianjie" => core::deepseek_links(app),
        "feedback" => feedback::feedback(app, arg),
        "hf" | "huggingface" => hf::hf(app, arg),
        "home" | "stats" | "overview" | "zhuye" | "shouye" => core::home_dashboard(app),
        "workspace" | "cwd" => core::workspace_switch(app, arg),
        "note" => note::note(app, arg),
        "memory" => memory::memory(app, arg),
        "attach" | "image" | "media" | "fujian" => attachment::attach(app, arg),
        "task" | "tasks" => task::task(app, arg),
        "jobs" | "job" | "zuoye" => jobs::jobs(app, arg),
        "mcp" => mcp::mcp(app, arg),
        "network" => network::network(app, arg),

        // Session commands
        "rename" | "gaiming" | "chongmingming" => rename::rename(app, arg),
        "save" => session::save(app, arg),
        "fork" | "branch" => session::fork(app),
        "new" => session::new_session(app, arg),
        "sessions" | "resume" => session::sessions(app, arg),
        "relay" | "batonpass" | "接力" => relay(app, arg),
        "load" | "jiazai" => session::load(app, arg),
        "compact" | "yasuo" => session::compact(app),
        "purge" | "qingchu" => session::purge(app),
        "export" | "daochu" => session::export(app, arg),

        // Config commands
        "config" => config::config_command(app, arg),
        "sidebar" => config::sidebar(app, arg),
        "settings" => config::show_settings(app),
        "status" => status::status(app),
        "statusline" => config::status_line(app),
        "mode" => config::mode(app, arg),
        "jihua" => config::mode(app, Some("plan")),
        "zidong" => config::mode(app, Some("yolo")),
        "theme" => config::theme(app, arg),
        "locale" | "language" | "lang" => config::locale(app, arg),
        "verbose" => config::verbose(app, arg),
        "trust" | "xinren" => config::trust(app, arg),
        "logout" => config::logout(app),

        // Debug commands
        "translate" | "translation" | "transale" => core::translate(app),
        "tokens" => debug::tokens(app),
        "cost" => debug::cost(app),
        "balance" => balance::balance(app),
        "cache" => debug::cache(app, arg),

        // Slop ledger (#2127)
        "slop" | "canzha" => config::slop(app, arg),

        // ChangeLog command
        "change" => change::change(app, arg),
        "system" | "xitong" => debug::system_prompt(app),
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
        "retry" | "chongshi" => debug::retry(app),

        // Project commands
        "init" => init::init(app),
        "lsp" => config::lsp_command(app, arg),
        "share" => share::share(app, arg),
        "goal" | "hunt" | "mubiao" | "狩猎" => goal::hunt(app, arg),

        // Skills commands
        "skills" | "jinengliebiao" => skills::list_skills(app, arg),
        "skill" | "jineng" => skills::run_skill(app, arg),
        "review" | "shencha" => review::review(app, arg),
        "restore" => restore::restore(app, arg),

        // Profile switch (#390)
        "profile" | "dangan" => core::profile_switch(app, arg),

        // RLM command
        "rlm" | "recursive" | "digui" => rlm(app, arg),

        // Legacy command migrations (kept out of registry/autocomplete intentionally).
        "set" => CommandResult::error(
            "The /set command was retired. Use /config to edit settings and /settings to inspect current values.",
        ),
        "deepseek" => CommandResult::error(
            "The /deepseek command was renamed. Use /links (aliases: /dashboard, /api).",
        ),

        _ => {
            // Third source: skills (lowest precedence after native and user-config).
            // Try to run a skill whose name matches the command.
            if let Some(result) = skills::run_skill_by_name(app, command, arg) {
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
    }
}

/// Update a configuration value programmatically (used by interactive UI views).
pub fn set_config_value(app: &mut App, key: &str, value: &str, persist: bool) -> CommandResult {
    config::set_config_value(app, key, value, persist)
}

pub fn switch_mode(app: &mut App, mode: crate::tui::app::AppMode) -> String {
    config::switch_mode(app, mode)
}
/// Execute a Recursive Language Model (RLM) turn — Algorithm 1 from
/// Zhang et al. (arXiv:2512.24601).
///
/// The user's prompt text is passed as the argument. It will be stored
/// in the REPL as the `PROMPT` variable. The root LLM will only see
/// metadata about the REPL state, never the prompt text directly.
pub fn rlm(app: &mut App, arg: Option<&str>) -> CommandResult {
    let (max_depth, target) = match parse_depth_prefixed_arg(arg, 1) {
        Ok(parsed) => parsed,
        Err(message) => return CommandResult::error(message),
    };
    let target = match target {
        Some(p) if !p.trim().is_empty() => p.trim().to_string(),
        _ => {
            return CommandResult::error(
                "Usage: /rlm [N] <file_or_text>\n\n\
                 Opens a persistent RLM context with sub_rlm depth N (0-3, default 1)."
                    .to_string(),
            );
        }
    };

    let source_arg = if resolves_to_existing_file(app, &target) {
        format!(r#"file_path: "{target}""#)
    } else {
        format!("content: {target:?}")
    };
    let message = format!(
        "Open and use a persistent RLM session for this request. Call `rlm_open` with name `slash_rlm` and {source_arg}. Then call `rlm_configure` with `sub_rlm_max_depth: {max_depth}`. Use `rlm_eval` to inspect the context through `peek`, `search`, and `chunk`, and call `finalize(...)` from the REPL when ready. If a `var_handle` is returned, use `handle_read` for bounded slices or projections before answering."
    );

    CommandResult::with_message_and_action(
        format!("Opening persistent RLM context at depth {max_depth}..."),
        AppAction::SendMessage(message),
    )
}

/// Open a persistent sub-agent session from a slash command.
pub fn agent(_app: &mut App, arg: Option<&str>) -> CommandResult {
    let (max_depth, task) = match parse_depth_prefixed_arg(arg, 1) {
        Ok(parsed) => parsed,
        Err(message) => return CommandResult::error(message),
    };
    let task = match task {
        Some(task) if !task.trim().is_empty() => task.trim().to_string(),
        _ => {
            return CommandResult::error(
                "Usage: /agent [N] <task>\n\n\
                 Opens a persistent sub-agent session with recursive agent depth N (0-3, default 1).",
            );
        }
    };
    let message = format!(
        "Open a persistent sub-agent session for this task. Call `agent_open` with name `slash_agent`, `prompt: {task:?}`, and `max_depth: {max_depth}`. Use `agent_eval` to wait for the next terminal/current projection and `handle_read` on the returned transcript_handle if you need more detail. Verify any claimed side effects before reporting success."
    );
    CommandResult::with_message_and_action(
        format!("Opening persistent sub-agent at depth {max_depth}..."),
        AppAction::SendMessage(message),
    )
}

/// Ask the active model to write a compact relay artifact for the next thread.
///
/// The visible command is `/relay` (with `/接力` for Chinese users), but the
/// durable file path remains `.deepseek/handoff.md` for compatibility with
/// existing sessions and startup prompt loading.
pub fn relay(app: &mut App, arg: Option<&str>) -> CommandResult {
    let focus = arg.map(str::trim).filter(|value| !value.is_empty());
    let message = build_relay_instruction(app, focus);
    CommandResult::with_message_and_action(
        "Preparing session relay at .deepseek/handoff.md...",
        AppAction::SendMessage(message),
    )
}

fn build_relay_instruction(app: &App, focus: Option<&str>) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Create a compact session relay (接力) for a future CodeWhale thread."
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "Write or update `.deepseek/handoff.md`.");
    let _ = writeln!(
        out,
        "Keep the existing file path for compatibility, but title the artifact `# Session relay`."
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "Current session snapshot:");
    let _ = writeln!(out, "- Workspace: {}", app.workspace.display());
    let _ = writeln!(out, "- Mode: {}", app.mode.label());
    let _ = writeln!(out, "- Model: {}", app.model_display_label());
    if let Some(focus) = focus {
        let _ = writeln!(out, "- Requested relay focus: {focus}");
    }
    if let Some(quarry) = app.hunt.quarry.as_deref() {
        let _ = writeln!(out, "- Hunt quarry: {quarry}");
    }
    if let Some(budget) = app.hunt.token_budget {
        let _ = writeln!(out, "- Hunt token budget: {budget}");
    }
    if let Ok(todos) = app.todos.try_lock() {
        let snapshot = todos.snapshot();
        if !snapshot.items.is_empty() {
            let _ = writeln!(
                out,
                "\nWork checklist (primary progress surface, {}% complete):",
                snapshot.completion_pct
            );
            for item in snapshot.items {
                let _ = writeln!(
                    out,
                    "- #{} [{}] {}",
                    item.id,
                    item.status.as_str(),
                    item.content
                );
            }
        }
    } else {
        let _ = writeln!(
            out,
            "\nWork checklist: unavailable because the checklist is busy."
        );
    }

    if let Ok(plan) = app.plan_state.try_lock() {
        let snapshot = plan.snapshot();
        if !snapshot.is_empty() {
            let _ = writeln!(out, "\nOptional strategy metadata from update_plan:");
            write_plan_field(&mut out, "Title", snapshot.title.as_deref());
            write_plan_field(&mut out, "Objective", snapshot.objective.as_deref());
            write_plan_field(&mut out, "Context", snapshot.context_summary.as_deref());
            write_plan_field(&mut out, "Explanation", snapshot.explanation.as_deref());
            write_plan_list(&mut out, "Source", &snapshot.sources_used);
            write_plan_list(&mut out, "Critical file", &snapshot.critical_files);
            write_plan_list(&mut out, "Constraint", &snapshot.constraints);
            write_plan_field(
                &mut out,
                "Recommended approach",
                snapshot.recommended_approach.as_deref(),
            );
            write_plan_field(
                &mut out,
                "Verification plan",
                snapshot.verification_plan.as_deref(),
            );
            write_plan_field(
                &mut out,
                "Risks and unknowns",
                snapshot.risks_and_unknowns.as_deref(),
            );
            write_plan_field(
                &mut out,
                "Handoff packet",
                snapshot.handoff_packet.as_deref(),
            );
            for item in snapshot.items {
                let _ = writeln!(out, "- [{}] {}", plan_status_label(&item.status), item.step);
            }
        }
    } else {
        let _ = writeln!(
            out,
            "\nStrategy metadata: unavailable because plan state is busy."
        );
    }

    let _ = writeln!(
        out,
        "\nBefore writing, inspect the current transcript context and any live tool evidence you need. Do not invent test results, file changes, blockers, or decisions."
    );
    let _ = writeln!(
        out,
        "\nUse this compact structure:\n\
         # Session relay\n\
         \n\
         ## Goal\n\
         [the user's objective and any explicit constraints]\n\
         \n\
         ## Current work\n\
         [the active Work checklist item, progress, and what is mid-flight]\n\
         \n\
         ## Files and state\n\
         [changed files, important paths, sub-agents/RLM sessions, commands run]\n\
         \n\
         ## Decisions\n\
         [why key choices were made]\n\
         \n\
         ## Verification\n\
         [what passed, what failed, what was not run]\n\
         \n\
         ## Next action\n\
         [one concrete action for the next thread]"
    );
    let _ = writeln!(
        out,
        "\nKeep it under about 900 words unless the session genuinely needs more. After writing, report the path and the single next action."
    );
    out
}

fn write_plan_field(out: &mut String, label: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        let _ = writeln!(out, "- {label}: {value}");
    }
}

fn write_plan_list(out: &mut String, label: &str, values: &[String]) {
    for value in values {
        let value = value.trim();
        if !value.is_empty() {
            let _ = writeln!(out, "- {label}: {value}");
        }
    }
}

fn plan_status_label(status: &crate::tools::plan::StepStatus) -> &'static str {
    match status {
        crate::tools::plan::StepStatus::Pending => "pending",
        crate::tools::plan::StepStatus::InProgress => "in_progress",
        crate::tools::plan::StepStatus::Completed => "completed",
    }
}

fn parse_depth_prefixed_arg(
    arg: Option<&str>,
    default_depth: u32,
) -> Result<(u32, Option<&str>), String> {
    let Some(raw) = arg.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok((default_depth, None));
    };
    let mut parts = raw.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or_default();
    if first.chars().all(|ch| ch.is_ascii_digit()) {
        let depth: u32 = first
            .parse()
            .map_err(|_| "Depth must be an integer from 0 to 3".to_string())?;
        if depth > 3 {
            return Err("Depth must be between 0 and 3".to_string());
        }
        Ok((depth, parts.next().map(str::trim)))
    } else {
        Ok((default_depth, Some(raw)))
    }
}

fn resolves_to_existing_file(app: &App, input: &str) -> bool {
    let path = std::path::Path::new(input);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        app.workspace.join(path)
    };
    candidate.is_file()
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
    fn command_registry_contains_config_and_links_but_not_set_or_deepseek() {
        assert!(COMMANDS.iter().any(|cmd| cmd.name == "config"));
        let sidebar = COMMANDS
            .iter()
            .find(|cmd| cmd.name == "sidebar")
            .expect("sidebar command should exist");
        assert_eq!(sidebar.description_id, MessageId::CmdSidebarDescription);
        assert!(
            sidebar
                .description_for(Locale::En)
                .contains("right sidebar")
        );
        assert!(COMMANDS.iter().any(|cmd| cmd.name == "links"));
        let hf = COMMANDS
            .iter()
            .find(|cmd| cmd.name == "hf")
            .expect("hf command should exist");
        assert_eq!(hf.aliases, &["huggingface"]);
        assert_eq!(hf.description_id, MessageId::CmdHfDescription);
        assert!(hf.description_for(Locale::En).contains("Hugging Face"));
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
        assert!(message.contains("agent_open"));
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
                handoff_packet: Some("Next thread should read the Work checklist".to_string()),
                plan: vec![PlanItemArg {
                    step: "keep checklist primary".to_string(),
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
        assert!(message.contains("Hunt quarry: Unify the work surface"));
        assert!(message.contains("Hunt token budget: 12000"));
        assert!(message.contains("Work checklist (primary progress surface, 50% complete)"));
        assert!(message.contains("#1 [completed] inspect workspace"));
        assert!(message.contains("#2 [in_progress] patch relay command"));
        assert!(message.contains("Optional strategy metadata from update_plan"));
        assert!(message.contains("Objective: Keep relays grounded"));
        assert!(message.contains("Explanation: RLM-style strategy"));
        assert!(message.contains("Source: transcript context"));
        assert!(message.contains("Critical file: crates/tui/src/commands/mod.rs"));
        assert!(message.contains("Constraint: Do not invent verification"));
        assert!(message.contains("Verification plan: Check relay prompt assertions"));
        assert!(message.contains("Handoff packet: Next thread should read the Work checklist"));
        assert!(message.contains("[in_progress] keep checklist primary"));
    }

    #[test]
    fn relay_command_has_bilingual_aliases() {
        let relay = COMMANDS
            .iter()
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
                    "alias /{alias} collides with a command name"
                );
                assert!(aliases.insert(*alias), "duplicate command alias /{alias}");
            }
        }
    }

    #[test]
    fn command_registry_metadata_is_complete_and_palette_safe() {
        for command in COMMANDS {
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
    fn command_info_resolves_canonical_names_and_aliases() {
        for command in COMMANDS {
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
        for command in COMMANDS {
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
    fn execute_sidebar_toggles_visibility() {
        let mut app = create_test_app();
        app.set_sidebar_focus(SidebarFocus::Auto);

        let result = execute("/sidebar", &mut app);
        assert!(!result.is_error);
        assert_eq!(app.sidebar_focus, SidebarFocus::Hidden);
        assert!(app.status_message.is_none());
        assert_eq!(result.message.as_deref(), Some("Sidebar is hidden"));

        let result = execute("/sidebar", &mut app);
        assert!(!result.is_error);
        assert_eq!(app.sidebar_focus, SidebarFocus::Auto);
        assert!(app.status_message.is_none());
        assert_eq!(result.message.as_deref(), Some("Sidebar is visible"));
    }

    #[test]
    fn execute_sidebar_accepts_explicit_focus_targets() {
        let mut app = create_test_app();

        let result = execute("/sidebar tasks", &mut app);
        assert!(!result.is_error);
        assert_eq!(app.sidebar_focus, SidebarFocus::Tasks);
        assert!(app.status_message.is_none());

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
        assert_eq!(app.sidebar_focus, SidebarFocus::Auto);
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
        for command in COMMANDS {
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
}
