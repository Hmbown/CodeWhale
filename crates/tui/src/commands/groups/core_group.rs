//! Core commands group — help, clear, exit, model, models, provider, links,
//! workspace, home/stats, profile, subagents, agent, relay, feedback

use crate::tui::app::{App, AppAction};

use crate::commands::traits::{Command, CommandGroup, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;

// ---------------------------------------------------------------------------
// Help
// ---------------------------------------------------------------------------

pub struct Help;
impl Command for Help {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "help",
            aliases: &["?", "bangzhu", "帮助"],
            usage: "/help [command]",
            description_id: MessageId::CmdHelpDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::core::help(app, args)
    }
}

// ---------------------------------------------------------------------------
// Clear
// ---------------------------------------------------------------------------

pub struct Clear;
impl Command for Clear {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "clear",
            aliases: &["qingping"],
            usage: "/clear",
            description_id: MessageId::CmdClearDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::core::clear(app)
    }
}

// ---------------------------------------------------------------------------
// Exit
// ---------------------------------------------------------------------------

pub struct Exit;
impl Command for Exit {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "exit",
            aliases: &["quit", "q", "tuichu"],
            usage: "/exit",
            description_id: MessageId::CmdExitDescription,
        }
    }
    fn execute(&self, _app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::core::exit()
    }
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

pub struct Model;
impl Command for Model {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "model",
            aliases: &["moxing"],
            usage: "/model [name]",
            description_id: MessageId::CmdModelDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::core::model(app, args)
    }
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

pub struct Models;
impl Command for Models {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "models",
            aliases: &["moxingliebiao"],
            usage: "/models",
            description_id: MessageId::CmdModelsDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::core::models(app)
    }
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

pub struct Provider;
impl Command for Provider {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "provider",
            aliases: &[],
            usage: "/provider [name] [model]",
            description_id: MessageId::CmdProviderDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::provider::provider(app, args)
    }
}

// ---------------------------------------------------------------------------
// Links / Dashboard / API
// ---------------------------------------------------------------------------

pub struct Links;
impl Command for Links {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "links",
            aliases: &["dashboard", "api", "lianjie"],
            usage: "/links",
            description_id: MessageId::CmdLinksDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::core::deepseek_links(app)
    }
}

// ---------------------------------------------------------------------------
// Feedback
// ---------------------------------------------------------------------------

pub struct Feedback;
impl Command for Feedback {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "feedback",
            aliases: &[],
            usage: "/feedback [bug|feature|security]",
            description_id: MessageId::CmdFeedbackDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::feedback::feedback(app, args)
    }
}

// ---------------------------------------------------------------------------
// Home / Stats / Overview
// ---------------------------------------------------------------------------

pub struct Home;
impl Command for Home {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "home",
            aliases: &["stats", "overview", "zhuye", "shouye"],
            usage: "/home",
            description_id: MessageId::CmdHomeDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::core::home_dashboard(app)
    }
}

// ---------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------

pub struct Workspace;
impl Command for Workspace {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "workspace",
            aliases: &["cwd"],
            usage: "/workspace [path]",
            description_id: MessageId::CmdWorkspaceDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::core::workspace_switch(app, args)
    }
}

// ---------------------------------------------------------------------------
// Subagents
// ---------------------------------------------------------------------------

pub struct Subagents;
impl Command for Subagents {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "subagents",
            aliases: &["agents", "zhinengti"],
            usage: "/subagents",
            description_id: MessageId::CmdSubagentsDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::core::subagents(app)
    }
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

pub struct Agent;
impl Command for Agent {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "agent",
            aliases: &["daili"],
            usage: "/agent [N] <task>",
            description_id: MessageId::CmdAgentDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        agent(app, args)
    }
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

pub struct Profile;
impl Command for Profile {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "profile",
            aliases: &["dangan"],
            usage: "/profile <name>",
            description_id: MessageId::CmdHelpDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::core::profile_switch(app, args)
    }
}

// ---------------------------------------------------------------------------
// Relay
// ---------------------------------------------------------------------------

pub struct Relay;
impl Command for Relay {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "relay",
            aliases: &["batonpass", "接力"],
            usage: "/relay [focus]",
            description_id: MessageId::CmdRelayDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        relay(app, args)
    }
}

// ---------------------------------------------------------------------------
// Group
// ---------------------------------------------------------------------------

pub struct CoreCommands;
impl CommandGroup for CoreCommands {

    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(Help),
            Box::new(Clear),
            Box::new(Exit),
            Box::new(Model),
            Box::new(Models),
            Box::new(Provider),
            Box::new(Links),
            Box::new(Feedback),
            Box::new(Home),
            Box::new(Workspace),
            Box::new(Subagents),
            Box::new(Agent),
            Box::new(Profile),
            Box::new(Relay),
        ]
    }
}


// ── Helper functions ───────────────────────────────────────────────────────

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

fn build_relay_instruction(app: &App, focus: Option<&str>) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "Create a compact session relay for a future CodeWhale thread.");
    let _ = writeln!(out);
    let _ = writeln!(out, "Write or update `.deepseek/handoff.md`.");
    let _ = writeln!(out, "Keep the existing file path for compatibility, but title the artifact `# Session relay`.");
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
            let _ = writeln!(out, "\nWork checklist (primary progress surface, {}% complete):", snapshot.completion_pct);
            for item in snapshot.items {
                let _ = writeln!(out, "- #{} [{}] {}", item.id, item.status.as_str(), item.content);
            }
        }
    } else {
        let _ = writeln!(out, "\nWork checklist: unavailable because the checklist is busy.");
    }
    if let Ok(plan) = app.plan_state.try_lock() {
        let snapshot = plan.snapshot();
        if snapshot.explanation.is_some() || !snapshot.items.is_empty() {
            let _ = writeln!(out, "\nOptional strategy metadata from update_plan:");
            if let Some(explanation) = snapshot.explanation.as_deref() {
                let _ = writeln!(out, "- Explanation: {explanation}");
            }
            for item in snapshot.items {
                let _ = writeln!(out, "- [{}] {}", plan_status_label(&item.status), item.step);
            }
        }
    } else {
        let _ = writeln!(out, "\nStrategy metadata: unavailable because plan state is busy.");
    }
    let _ = writeln!(out, "\nBefore writing, inspect the current transcript context and any live tool evidence you need.");
    let _ = writeln!(out, "\nKeep it under about 900 words. After writing, report the path and the single next action.");
    out
}

fn relay(app: &mut App, arg: Option<&str>) -> CommandResult {
    let focus = arg.map(str::trim).filter(|value| !value.is_empty());
    let message = build_relay_instruction(app, focus);
    CommandResult::with_message_and_action(
        "Preparing session relay at .deepseek/handoff.md...",
        AppAction::SendMessage(message),
    )
}

fn agent(_app: &mut App, arg: Option<&str>) -> CommandResult {
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
