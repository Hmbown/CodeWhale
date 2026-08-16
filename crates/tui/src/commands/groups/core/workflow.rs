//! `/workflow` command — the user's opt-in to workflow orchestration.
//!
//! The invocation carries authorization, not payload: bare `/workflow` asks
//! the model to synthesize the objective from the conversation context and
//! orchestrate it through the `workflow` tool (the same contract as goal-mode
//! `/goal`: context-dependent, no argument required). `/workflow <objective>`
//! narrows the run to an explicit objective. Control verbs (`status`,
//! `cancel`, `settings`, `help`) are answered by the host from the run
//! journal and live state — they never spend a model turn.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "workflow",
    aliases: &["workflows", "wf"],
    usage: "/workflow [objective|run <path>|status [run_id]|cancel [run_id]|settings]",
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

pub fn workflow(app: &mut App, arg: Option<&str>) -> CommandResult {
    let _app: &App = app;
    let arg = arg.map(str::trim).filter(|value| !value.is_empty());

    if let Some(action) = parse_workflow_control_action(_app, arg) {
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

/// Host-side `status` / `runs` / `cancel` / `settings`: read the run journal and
/// live run state directly and answer without a model turn, so a status
/// check is free and a cancel lands even while the model is busy.
fn parse_workflow_control_action(app: &App, arg: Option<&str>) -> Option<CommandResult> {
    let arg = arg?;
    let (verb, rest) = match arg.split_once(char::is_whitespace) {
        Some((verb, rest)) => (verb, rest.trim()),
        None => (arg, ""),
    };
    match verb {
        "status" | "runs" | "list" | "inspect" => Some(workflow_status(app, rest)),
        "cancel" | "stop" | "abort" => Some(workflow_cancel(app, rest)),
        "settings" | "config" => Some(super::super::config::workflow_settings(app)),
        "help" | "?" => Some(CommandResult::message(WORKFLOW_USAGE)),
        // `/workflow run <path>` — the form the checked-in examples document.
        // The run itself needs the tool's runtime, so the model is asked to
        // launch exactly this source path (no re-authoring, no new plan).
        "run" if !rest.is_empty() && !rest.contains(char::is_whitespace) => {
            let message = format!(
                "The user invoked /workflow run with the checked-in source path {rest:?} — this is \
                 authorization to launch it as-is. Call the `workflow` tool with `source_path` set \
                 to that path (action `run` to wait, or `start` then `status` if it is long), do not \
                 rewrite or replace the script, narrate phases as they complete, and end with a \
                 compact receipt: run_id, status, and per-leaf outcomes."
            );
            Some(CommandResult::with_message_and_action(
                format!("Running workflow {rest}..."),
                AppAction::SendMessage(message),
            ))
        }
        _ => None,
    }
}

const WORKFLOW_USAGE: &str =
    "/workflow <objective> — orchestrate the objective with the workflow tool
/workflow — orchestrate the current work
/workflow status [run_id] — runs known to this workspace (no model turn)
/workflow cancel [run_id] — stop a running workflow (no model turn)
/workflow settings — the effective [workflow] configuration";

fn describe_run(line: &crate::tools::workflow::HostWorkflowRunLine, now_ms: u64) -> String {
    let elapsed = line
        .completed_at_ms
        .unwrap_or(now_ms)
        .saturating_sub(line.started_at_ms)
        / 1000;
    let mut text = format!(
        "{}  {}  {}  {}  {} children",
        line.run_id,
        line.status,
        line.label,
        crate::elapsed::format_elapsed_secs(elapsed),
        line.child_count
    );
    if let Some(progress) = line.last_progress.as_deref() {
        text.push_str("  ·  ");
        text.push_str(progress);
    }
    if let Some(error) = line.error.as_deref() {
        text.push_str("  ·  ");
        text.push_str(error);
    }
    text
}

fn workflow_status(app: &App, run_id: &str) -> CommandResult {
    let runs = crate::tools::workflow::host_workflow_runs(&app.workspace);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default();
    if !run_id.is_empty() {
        return match runs.iter().find(|line| line.run_id == run_id) {
            Some(line) => CommandResult::message(describe_run(line, now_ms)),
            None => CommandResult::error(format!(
                "Unknown workflow run '{run_id}'. /workflow status lists the runs this workspace knows."
            )),
        };
    }
    if runs.is_empty() {
        return CommandResult::message(
            "No workflow runs in this workspace yet. /workflow <objective> starts one.",
        );
    }
    let running = runs.iter().filter(|line| line.status == "running").count();
    let mut lines = vec![format!(
        "{} workflow run{} · {running} running",
        runs.len(),
        if runs.len() == 1 { "" } else { "s" }
    )];
    // Newest first; the journal can hold every run the workspace ever made.
    for line in runs.iter().rev().take(20) {
        lines.push(describe_run(line, now_ms));
    }
    if runs.len() > 20 {
        lines.push(format!(
            "… {} older runs in .codewhale/workflow-runs.jsonl",
            runs.len() - 20
        ));
    }
    CommandResult::message(lines.join("\n"))
}

fn workflow_cancel(app: &App, run_id: &str) -> CommandResult {
    if run_id.contains(char::is_whitespace) {
        return CommandResult::error("Usage: /workflow cancel [run_id]");
    }
    let target = if run_id.is_empty() {
        let running: Vec<_> = crate::tools::workflow::host_workflow_runs(&app.workspace)
            .into_iter()
            .filter(|line| line.status == "running")
            .collect();
        match running.as_slice() {
            [] => return CommandResult::message("No workflow is running."),
            [only] => only.run_id.clone(),
            many => {
                let ids: Vec<&str> = many.iter().map(|line| line.run_id.as_str()).collect();
                return CommandResult::error(format!(
                    "{} workflows are running; name one: {}",
                    many.len(),
                    ids.join(", ")
                ));
            }
        }
    } else {
        run_id.to_string()
    };
    match crate::tools::workflow::host_cancel_workflow(&app.workspace, &target) {
        Ok(line) => CommandResult::message(format!(
            "Workflow {} {} · {}",
            line.run_id, line.status, line.label
        )),
        Err(reason) => CommandResult::error(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::tui::app::TuiOptions;

    fn test_app() -> App {
        let options = TuiOptions {
            ..crate::test_support::test_tui_options(PathBuf::from("."))
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
    fn workflow_status_and_cancel_answer_from_the_host_without_a_model_turn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app();
        app.workspace = dir.path().to_path_buf();

        // Nothing has run in this workspace: status is a plain answer, and it
        // must not create the run journal just to say so.
        let result = workflow(&mut app, Some("status"));
        assert!(!result.is_error);
        assert!(
            result.action.is_none(),
            "status must not send a model message"
        );
        assert!(
            result
                .message
                .as_deref()
                .unwrap()
                .contains("No workflow runs")
        );
        assert!(!dir.path().join(".codewhale/workflow-runs.jsonl").exists());

        let result = workflow(&mut app, Some("status wf_missing"));
        assert!(result.is_error);
        assert!(result.action.is_none());

        // A seeded run is listed and described from host state.
        crate::tools::workflow::structcopy_test_seed_run(dir.path(), "workflow_seed");
        let result = workflow(&mut app, Some("runs"));
        let text = result.message.unwrap();
        assert!(text.contains("workflow_seed"), "{text}");
        assert!(text.contains("running"), "{text}");
        assert!(result.action.is_none());

        // Cancel with one running run needs no id and never asks the model.
        // The seeded record has no live controller (no VM ran); cancel still
        // marks the journal cancelled with an honest nothing-live receipt.
        let result = workflow(&mut app, Some("cancel"));
        assert!(result.action.is_none());
        assert!(!result.is_error, "{:?}", result.message);
        let text = result.message.as_deref().unwrap();
        assert!(text.contains("workflow_seed"), "{text}");
        assert!(text.contains("cancelled"), "{text}");
        let after = crate::tools::workflow::host_workflow_runs(&app.workspace);
        assert_eq!(
            after
                .iter()
                .find(|line| line.run_id == "workflow_seed")
                .map(|line| line.status),
            Some("cancelled")
        );

        let result = workflow(&mut app, Some("cancel with spaces"));
        assert!(result.is_error);

        let result = workflow(&mut app, Some("help"));
        assert!(result.message.unwrap().contains("/workflow status"));

        // `/workflow run <path>` launches exactly that source through the tool.
        let result = workflow(&mut app, Some("run workflows/tiny.workflow.js"));
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("`source_path`"), "{message}");
        assert!(message.contains("workflows/tiny.workflow.js"), "{message}");
        assert!(message.contains("do not"), "{message}");
    }

    #[test]
    fn workflow_settings_explains_the_session_table() {
        let mut app = test_app();
        app.workflow_config.automatic = false;
        app.workflow_config.require_approval_for_writes = false;
        app.goal_max_continuations = 25;
        let result = workflow(&mut app, Some("settings"));
        assert!(result.action.is_none());
        let text = result.message.unwrap();
        assert!(text.contains("automatic = off"), "{text}");
        assert!(text.contains("require_approval_for_writes = off"), "{text}");
        assert!(text.contains("max_continuations = 25"), "{text}");
    }

    #[test]
    fn workflow_settings_and_tool_share_a_refreshed_session_table() {
        use crate::tools::spec::{ApprovalRequirement, ToolContext, ToolSpec};
        use crate::tools::subagent::{SubAgentRuntime, new_shared_subagent_manager};
        use crate::tools::workflow::WorkflowTool;
        use serde_json::json;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app();
        app.workspace = dir.path().to_path_buf();

        let mut table = app.workflow_config.clone();
        table.automatic = false;
        table.require_approval_for_writes = false;
        table.auto_start_read_only = false;
        crate::tools::workflow::set_session_workflow_config(&app.workspace, table.clone());
        app.workflow_config = table;

        let result = workflow(&mut app, Some("settings"));
        assert!(result.action.is_none());
        let text = result.message.unwrap();
        assert!(text.contains("automatic = off"), "{text}");
        assert!(text.contains("require_approval_for_writes = off"), "{text}");
        assert!(text.contains("auto_start_read_only = off"), "{text}");

        let ctx = ToolContext::new(dir.path().to_path_buf());
        let manager = new_shared_subagent_manager(dir.path().to_path_buf(), 2);
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = crate::client::DeepSeekClient::new(&crate::config::Config {
            api_key: Some("test-key".to_string()),
            ..crate::config::Config::default()
        })
        .expect("stub client");
        let mut runtime = SubAgentRuntime::new(
            client,
            "deepseek-v4-flash".to_string(),
            ctx,
            true,
            None,
            manager.clone(),
        );
        // Stale snapshot: product defaults still require write approval.
        runtime.api_config = Some(std::sync::Arc::new(crate::config::Config::default()));
        let tool = WorkflowTool::new(manager, runtime);

        let write_plan = json!({
            "action": "start",
            "plan": {
                "goal": "write freely",
                "risk": "writes",
                "children": [{ "prompt": "edit", "type": "implementer" }]
            }
        });
        let read_only = json!({
            "action": "start",
            "plan": {
                "goal": "scout crates",
                "risk": "read_only",
                "children": [{ "prompt": "look", "type": "explore" }]
            }
        });
        assert_eq!(
            tool.approval_requirement_for(&write_plan),
            ApprovalRequirement::Auto,
            "refreshed require_approval_for_writes = false must win over the stale runtime snapshot"
        );
        assert_eq!(
            tool.approval_requirement_for(&read_only),
            ApprovalRequirement::Required,
            "refreshed auto_start_read_only = false must still ask"
        );
    }
}
