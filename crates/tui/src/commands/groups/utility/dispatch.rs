//! `/dispatch` — first-class Codewhale cloud-agent offload.

use codewhale_command_contract::facets::CommandWorkspaceContext;
use codewhale_command_contract::handler::{CommandContexts, CommandHandler};
use codewhale_command_contract::metadata::{CommandInfo, RegisterCommand};

use crate::cloud_dispatch::{
    CloudJobStore, DispatchOutcome, Forge, LiveDaytonaLauncher, cancel_job, confirm_job,
    discover_credentials, discover_machine_token, discover_remotes, execute_dispatch, format_job,
    format_job_list, format_status, plan_dispatch,
};
use crate::commands::CommandResult;
use crate::dispatch_runner::spawn_confirmed_runner;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "dispatch",
    aliases: &["cloud-agent", "cloud-dispatch"],
    usage: "/dispatch [list|show <id>|confirm <id>|cancel <id>|<prompt> [--remote github|cnb|gitee]]",
    description_key: "cmd_dispatch_description",
};

pub(in crate::commands) struct DispatchCmd;

impl RegisterCommand<CommandResult> for DispatchCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn handler() -> CommandHandler<CommandResult> {
        CommandHandler::Contextual {
            capabilities: codewhale_command_contract::handler::CommandCapabilities::WORKSPACE,
            handler: dispatch_contextual,
        }
    }
}

fn dispatch_contextual(contexts: CommandContexts<'_>, arg: Option<&str>) -> CommandResult {
    let mut parts = contexts.into_parts();
    let Some(workspace) = parts.workspace.as_deref_mut() else {
        return CommandResult::error("Command capability unavailable: workspace");
    };
    dispatch(workspace, arg)
}

fn dispatch(workspace: &mut dyn CommandWorkspaceContext, args: Option<&str>) -> CommandResult {
    let raw = args.unwrap_or("").trim();
    let store = match CloudJobStore::from_env() {
        Ok(store) => store,
        Err(error) => return CommandResult::error(error.to_string()),
    };
    if raw.is_empty() {
        let recent = store.list().unwrap_or_default();
        let recent: Vec<_> = recent.into_iter().take(5).collect();
        return CommandResult::message(format_status(
            &discover_remotes(&workspace.workspace()),
            &discover_credentials(),
            &recent,
        ));
    }

    let mut parts = raw.splitn(2, char::is_whitespace);
    let verb = parts.next().unwrap_or("").to_ascii_lowercase();
    let rest = parts.next().map(str::trim).unwrap_or("");

    match verb.as_str() {
        "list" => match store.list() {
            Ok(jobs) => CommandResult::message(format_job_list(&jobs)),
            Err(error) => CommandResult::error(error.to_string()),
        },
        "show" | "inspect" => {
            if rest.is_empty() {
                return CommandResult::error("Usage: /dispatch show <id>");
            }
            match store.load(rest) {
                Ok(job) => CommandResult::message(format_job(&job)),
                Err(error) => CommandResult::error(error.to_string()),
            }
        }
        "confirm" => {
            if rest.is_empty() {
                return CommandResult::error("Usage: /dispatch confirm <id>");
            }
            match confirm_job(
                &store,
                rest,
                &discover_credentials(),
                &discover_machine_token(),
            ) {
                Ok(outcome) => {
                    if let DispatchOutcome::Accepted(job) = &outcome {
                        // Detached: the TUI stays responsive and the job
                        // record streams progress; `/dispatch cancel` tears
                        // the sandbox down at any time.
                        spawn_confirmed_runner(store.clone(), job.id.clone());
                    }
                    CommandResult::message(outcome_message(&outcome))
                }
                Err(error) => CommandResult::error(error.to_string()),
            }
        }
        "cancel" | "kill" | "stop" => {
            if rest.is_empty() {
                return CommandResult::error("Usage: /dispatch cancel <id>");
            }
            match cancel_job(&store, rest, &LiveDaytonaLauncher) {
                Ok(job) => CommandResult::message(format_job(&job)),
                Err(error) => CommandResult::error(error.to_string()),
            }
        }
        _ => propose_or_run(workspace, &store, raw, false),
    }
}

fn propose_or_run(
    workspace: &mut dyn CommandWorkspaceContext,
    store: &CloudJobStore,
    raw: &str,
    confirm: bool,
) -> CommandResult {
    let (prompt, requested) = match split_prompt_and_remote(raw) {
        Ok(parsed) => parsed,
        Err(error) => return CommandResult::error(error),
    };
    let remotes = discover_remotes(&workspace.workspace());
    let plan = match plan_dispatch(&remotes, &prompt, requested, None) {
        Ok(plan) => plan,
        Err(error) => return CommandResult::error(error.to_string()),
    };
    match execute_dispatch(
        store,
        plan,
        confirm,
        &discover_credentials(),
        &discover_machine_token(),
    ) {
        Ok(outcome) => {
            if let DispatchOutcome::Accepted(job) = &outcome {
                spawn_confirmed_runner(store.clone(), job.id.clone());
            }
            CommandResult::message(outcome_message(&outcome))
        }
        Err(error) => CommandResult::error(error.to_string()),
    }
}

fn split_prompt_and_remote(raw: &str) -> Result<(String, Option<Forge>), String> {
    let mut remote = None;
    let mut prompt_parts = Vec::new();
    let mut tokens = raw.split_whitespace().peekable();
    while let Some(token) = tokens.next() {
        if token == "--remote" {
            let Some(value) = tokens.next() else {
                return Err("Usage: /dispatch <prompt> --remote github|cnb|gitee".to_string());
            };
            remote = Some(
                Forge::parse(value)
                    .ok_or_else(|| "Remote must be github, cnb, or gitee.".to_string())?,
            );
            continue;
        }
        if let Some(value) = token.strip_prefix("--remote=") {
            remote = Some(
                Forge::parse(value)
                    .ok_or_else(|| "Remote must be github, cnb, or gitee.".to_string())?,
            );
            continue;
        }
        prompt_parts.push(token);
    }
    let prompt = prompt_parts.join(" ");
    if prompt.is_empty() {
        return Err("Usage: /dispatch <prompt> [--remote github|cnb|gitee]".to_string());
    }
    Ok((prompt, remote))
}

fn outcome_message(outcome: &DispatchOutcome) -> String {
    match outcome {
        DispatchOutcome::Proposal(job)
        | DispatchOutcome::Refused(job)
        | DispatchOutcome::Accepted(job) => format_job(job),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct FakeWorkspace(PathBuf);
    impl CommandWorkspaceContext for FakeWorkspace {
        fn workspace(&self) -> PathBuf {
            self.0.clone()
        }
        fn work_state_snapshot(&self) -> Result<Option<String>, String> {
            Ok(None)
        }
        fn operation_digest(&mut self) -> Result<String, String> {
            Ok("digest".to_string())
        }
    }

    #[test]
    fn parses_prompt_and_explicit_remote() {
        let (prompt, remote) = split_prompt_and_remote("fix the flake --remote cnb").unwrap();
        assert_eq!(prompt, "fix the flake");
        assert_eq!(remote, Some(Forge::Cnb));
        assert!(split_prompt_and_remote("--remote gitee").is_err());
        assert_eq!(
            split_prompt_and_remote("open pr --remote=github")
                .unwrap()
                .1,
            Some(Forge::Github)
        );
    }

    #[test]
    fn handler_is_contextual_and_argument_aware() {
        assert!(matches!(
            DispatchCmd::handler(),
            CommandHandler::Contextual { .. }
        ));
        assert_eq!(
            DispatchCmd::info().description_key,
            "cmd_dispatch_description"
        );
        assert_eq!(
            DispatchCmd::info().aliases,
            &["cloud-agent", "cloud-dispatch"]
        );
        assert!(DispatchCmd::info().usage.starts_with("/dispatch"));
    }

    #[test]
    fn missing_workspace_facet_fails_safely() {
        let result = dispatch_contextual(CommandContexts::empty(), None);
        assert!(result.is_error, "{result:?}");
        assert_eq!(
            result.message.as_deref(),
            Some("Error: Command capability unavailable: workspace")
        );
        assert!(result.action.is_none());
    }

    #[test]
    fn bare_dispatch_is_a_status_card_not_a_silent_launch() {
        let mut workspace = FakeWorkspace(PathBuf::from("."));
        let result = dispatch(&mut workspace, None);
        assert!(!result.is_error, "{result:?}");
        let message = result.message.expect("status message");
        assert!(message.contains("Cloud agents"));
        assert!(message.contains("fails closed") || message.contains("ready"));
        assert!(result.action.is_none());
    }
}
