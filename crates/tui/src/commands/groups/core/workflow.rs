//! `/workflow` command — the user's opt-in to workflow orchestration.
//!
//! The invocation carries authorization, not payload: bare `/workflow` asks
//! the model to synthesize the objective from the conversation context and
//! orchestrate it through the `workflow` tool (the same contract as goal-mode
//! `/goal`: context-dependent, no argument required). `/workflow <objective>`
//! narrows the run to an explicit objective, and `/workflow status` relays
//! typed run receipts without starting anything new.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "workflow",
    aliases: &["workflows", "wf"],
    usage: "/workflow [objective|status|cancel <run_id>]",
    description_id: MessageId::CmdWorkflowDescription,
};

pub(in crate::commands) struct WorkflowCmd;

impl RegisterCommand for WorkflowCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        workflow(app, arg)
    }
}

/// Shared orchestration contract appended to every start instruction. Mirrors
/// what makes opt-in orchestration work well: the user's invocation is the
/// authorization, fan-out scales to the ask, and receipts close the loop.
const ORCHESTRATION_CONTRACT: &str = "Author a workflow script for the `workflow` tool (task()/parallel()/pipeline()/phase()/log()); \
     you are the fan-in owner — fan out, wait for receipts, aggregate, verify, and synthesize one result. \
     scale the fan-out to the size of the ask — a quick check gets a few tasks, an audit gets a wider sweep. \
     Prefer pipeline() over barriers so items flow stage-to-stage without waiting. \
     Use responseSchema on task() when you need structured child output; schema mismatches fail loudly in the run receipt. \
     parallel() turns child failures into null — filter those slots and treat them as failures, not results. \
     Run it with the `workflow` tool (`run` to block, or `start` then `status` for long runs), \
     narrate phases as they complete, verify findings before reporting them as facts, \
     and end with a compact receipt summary: run_id, status, and per-leaf outcomes.";

pub fn workflow(_app: &mut App, arg: Option<&str>) -> CommandResult {
    let arg = arg.map(str::trim).filter(|value| !value.is_empty());

    if let Some(action) = parse_workflow_control_action(arg) {
        return action;
    }

    match arg {
        // Explicit objective: the argument narrows the run.
        Some(objective) => {
            let message = format!(
                "The user invoked /workflow with an explicit objective — this is authorization to \
                 orchestrate it with the `workflow` tool. Objective: {objective:?}. \
                 Use the conversation context to ground the work (files discussed, prior findings). \
                 {ORCHESTRATION_CONTRACT}"
            );
            CommandResult::with_message_and_action(
                format!("Orchestrating as a workflow: {objective}"),
                AppAction::SendMessage(message),
            )
        }
        // Bare invocation: context-dependent. The model derives the objective
        // from what the session is already doing — no restating required.
        None => {
            let message = format!(
                "The user invoked /workflow with no argument — this is authorization to orchestrate \
                 the CURRENT work as a workflow. Synthesize the objective from the conversation \
                 context: the task in flight, recent findings, and open items. Do not ask the user \
                 to restate it unless the conversation genuinely contains no work yet. \
                 {ORCHESTRATION_CONTRACT}"
            );
            CommandResult::with_message_and_action(
                "Orchestrating the current work as a workflow...",
                AppAction::SendMessage(message),
            )
        }
    }
}

/// Route `status`/`cancel` through the `workflow` tool without starting a run.
fn parse_workflow_control_action(arg: Option<&str>) -> Option<CommandResult> {
    let arg = arg?;
    let (verb, rest) = match arg.split_once(char::is_whitespace) {
        Some((verb, rest)) => (verb, rest.trim()),
        None => (arg, ""),
    };
    match verb {
        "status" | "runs" | "list" | "inspect" => {
            let target = if rest.is_empty() {
                "all runs".to_string()
            } else {
                format!("run_id `{rest}`")
            };
            let message = format!(
                "Call the `workflow` tool with action `status`{} and summarize the receipts for \
                 the user: run_id, status, phase progress, per-leaf outcomes, and any errors. \
                 Keep it compact. Do not start a new workflow.",
                if rest.is_empty() {
                    String::new()
                } else {
                    format!(" and run_id `{rest}`")
                }
            );
            Some(CommandResult::with_message_and_action(
                format!("Fetching workflow status for {target}..."),
                AppAction::SendMessage(message),
            ))
        }
        "cancel" | "stop" | "abort" => {
            if rest.is_empty() || rest.contains(char::is_whitespace) {
                return Some(CommandResult::error(
                    "Usage: /workflow cancel <run_id>\n\nUse /workflow status to list run ids.",
                ));
            }
            let message = format!(
                "Call the `workflow` tool with action `cancel` and run_id `{rest}`, then report \
                 the final run status to the user. Do not start a new workflow."
            );
            Some(CommandResult::with_message_and_action(
                format!("Cancelling workflow {rest}..."),
                AppAction::SendMessage(message),
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::tui::app::TuiOptions;

    fn test_app() -> App {
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
        App::new(options, &crate::config::Config::default())
    }

    #[test]
    fn bare_workflow_is_context_dependent_opt_in() {
        let mut app = test_app();
        let result = workflow(&mut app, None);
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        // The bare form must not demand an objective from the user.
        assert!(message.contains("Synthesize the objective from the conversation"));
        assert!(message.contains("authorization to orchestrate"));
        assert!(message.contains("`workflow` tool"));

        // Whitespace-only behaves like bare.
        let result = workflow(&mut app, Some("   "));
        assert!(matches!(result.action, Some(AppAction::SendMessage(_))));
    }

    #[test]
    fn workflow_with_objective_forwards_it() {
        let mut app = test_app();
        let result = workflow(&mut app, Some("audit provider error handling"));
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("audit provider error handling"));
        assert!(message.contains("authorization"));
    }

    #[test]
    fn workflow_status_and_cancel_route_to_tool_without_new_runs() {
        let mut app = test_app();
        let result = workflow(&mut app, Some("status"));
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("action `status`"));
        assert!(message.contains("Do not start a new workflow"));

        let result = workflow(&mut app, Some("status wf_run_1"));
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("run_id `wf_run_1`"));

        let result = workflow(&mut app, Some("cancel wf_run_1"));
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("action `cancel`"));
        assert!(message.contains("run_id `wf_run_1`"));

        let result = workflow(&mut app, Some("cancel"));
        assert!(result.is_error, "cancel without a run id is a usage error");
    }
}
