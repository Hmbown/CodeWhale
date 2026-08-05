//! `/fleet` command.
//!
//! Fleet = who. `/fleet roster` and `/fleet setup` are the authoring views;
//! `/fleet list|status|interrupt|resume` are control-plane verbs that run
//! against the **durable** workspace ledger through the shared contract in
//! `codewhale-lane`, exactly as `codewhale fleet …` does (#1888, #4022).
//!
//! `/fleet status` used to show the current TUI session's sub-agents. That was
//! a different thing wearing the same name: session sub-agents are not the
//! durable Fleet ledger, and a run started by `codewhale fleet run` never
//! appeared. The session view is still reachable as `/fleet workers` (and
//! `/subagents`), now labelled as what it is.

use codewhale_lane::control::operations_for_domain;
use codewhale_lane::{ControlDomain, ControlOperation, ControlSurface};

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::fleet::control::execute_fleet_control;
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "fleet",
    aliases: &["loadout", "party"],
    usage: "/fleet [roster|setup|list|status|workers|interrupt <worker-id>|resume <run-id>]",
    description_id: MessageId::CmdFleetDescription,
};

pub(in crate::commands) struct FleetCmd;

fn help_text() -> String {
    let mut out = String::from(
        "Usage: /fleet [roster|setup|list|status|workers|interrupt <worker-id>|resume <run-id>]\n\n\
         Fleet is who. /fleet (or /fleet roster) opens Fleet workers and orchestration state — \
         each member's posture, routing, and origin. /fleet setup opens the authoring wizard.\n\n\
         /fleet list, status, interrupt, and resume act on the durable .codewhale/fleet.jsonl \
         ledger for this workspace — the same records `codewhale fleet` reads and writes. \
         /fleet workers (and /subagents) shows sub-agents in the current TUI session only, which \
         is a different set: it does not include durable Fleet runs.\n",
    );
    for descriptor in operations_for_domain(ControlDomain::Fleet) {
        out.push_str(&format!(
            "\n  {:<30} {:<6} {}\n      CLI: {}\n",
            descriptor.slash_invocation(),
            descriptor.authority.as_str(),
            descriptor.summary,
            descriptor.cli_invocation
        ));
    }
    out
}

/// Split `"<verb> <rest>"` into the verb and its raw target tail.
fn split_verb(arg: Option<&str>) -> Option<(&str, Option<&str>)> {
    let rest = arg.map(str::trim).filter(|value| !value.is_empty())?;
    Some(match rest.split_once(char::is_whitespace) {
        Some((verb, tail)) => (verb, Some(tail.trim())),
        None => (rest, None),
    })
}

fn run_control(app: &App, operation: ControlOperation, target: Option<&str>) -> CommandResult {
    let receipt = execute_fleet_control(ControlSurface::Slash, &app.workspace, operation, target);
    let rendered = receipt.render();
    if receipt.is_error() {
        CommandResult::error(rendered)
    } else {
        CommandResult::message(rendered)
    }
}

impl RegisterCommand for FleetCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        let Some((verb, target)) = split_verb(arg) else {
            // The primary Fleet surface is the saved-Fleet list. The legacy
            // roster remains one subcommand away.
            return CommandResult::action(AppAction::OpenFleetList);
        };
        match verb {
            "roster" | "party" | "loadout" | "roles" | "role" | "profiles" | "profile" => {
                // The old per-role roster; the saved-Fleet list is /fleet.
                CommandResult::action(AppAction::OpenFleetRoster)
            }
            "setup" | "edit" | "new" => CommandResult::action(AppAction::OpenFleetSetup),
            // The current-session sub-agent projection, named for what it is.
            "workers" | "worker" | "agents" | "subagents" => super::core::subagents(app),
            "help" | "?" => CommandResult::message(help_text()),
            other => match ControlOperation::parse_verb(ControlDomain::Fleet, other) {
                Some(operation) => run_control(app, operation, target),
                None => CommandResult::error(format!(
                    "Unknown /fleet target '{other}'. Use roster, setup, list, status, workers, \
                     interrupt <worker-id>, or resume <run-id>."
                )),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::TuiOptions;
    use std::path::PathBuf;

    fn test_app() -> App {
        let options = TuiOptions {
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        };
        App::new(options, &Config::default())
    }

    fn app_in(workspace: PathBuf) -> App {
        let options = TuiOptions {
            ..crate::test_support::test_tui_options(workspace.clone())
        };
        let mut app = App::new(options, &Config::default());
        app.workspace = workspace;
        app
    }

    #[test]
    fn fleet_command_opens_fleet_list_view() {
        let mut app = test_app();

        let result = FleetCmd::execute(&mut app, None);

        assert_eq!(result.action, Some(AppAction::OpenFleetList));
        assert!(result.message.is_none());
    }

    #[test]
    fn fleet_roster_aliases_open_roster_view() {
        for arg in [
            "roster", "party", "loadout", "roles", "role", "profiles", "profile",
        ] {
            let mut app = test_app();

            let result = FleetCmd::execute(&mut app, Some(arg));

            assert_eq!(result.action, Some(AppAction::OpenFleetRoster), "{arg}");
            assert!(result.message.is_none(), "{arg}");
        }
    }

    #[test]
    fn fleet_setup_args_open_setup_wizard() {
        for arg in ["setup", "edit", "new"] {
            let mut app = test_app();

            let result = FleetCmd::execute(&mut app, Some(arg));

            assert_eq!(result.action, Some(AppAction::OpenFleetSetup), "{arg}");
            assert!(result.message.is_none(), "{arg}");
        }
    }

    /// #4022: the session sub-agent projection keeps its own name. It is no
    /// longer allowed to answer for the durable Fleet ledger.
    #[test]
    fn fleet_workers_arg_opens_the_session_subagent_view() {
        for arg in ["workers", "worker", "agents", "subagents"] {
            let mut app = test_app();

            let result = FleetCmd::execute(&mut app, Some(arg));

            assert_eq!(result.action, Some(AppAction::ListSubAgents), "{arg}");
            assert!(result.message.is_none(), "{arg}");
        }
    }

    /// #4022: `/fleet status` must read the durable ledger, not substitute the
    /// current session's sub-agents for it.
    #[test]
    fn fleet_status_reads_the_durable_ledger_not_session_subagents() {
        let workspace = tempfile::tempdir().unwrap();
        let mut app = app_in(workspace.path().to_path_buf());

        let result = FleetCmd::execute(&mut app, Some("status"));

        assert_eq!(
            result.action, None,
            "/fleet status must not open the session sub-agent view"
        );
        let message = result.message.as_deref().unwrap_or_default();
        assert!(message.contains("fleet.status"), "got: {message}");
        // This workspace has no ledger, so the truthful answer is a typed
        // unavailability — never an empty-looking "all clear".
        assert!(message.contains("no_fleet_ledger"), "got: {message}");
        assert!(
            !workspace
                .path()
                .join(".codewhale")
                .join("fleet.jsonl")
                .exists(),
            "a read verb must not create the durable ledger"
        );
    }

    #[test]
    fn fleet_control_verbs_route_through_the_shared_contract() {
        let workspace = tempfile::tempdir().unwrap();
        for (arg, expected_id) in [
            ("list", "fleet.list"),
            ("status", "fleet.status"),
            ("interrupt worker-1", "fleet.interrupt"),
            ("resume run-1", "fleet.resume"),
            ("restart worker-1", "fleet.restart"),
        ] {
            let mut app = app_in(workspace.path().to_path_buf());
            let result = FleetCmd::execute(&mut app, Some(arg));
            let message = result.message.as_deref().unwrap_or_default();
            assert!(
                message.contains(expected_id),
                "/fleet {arg} must report {expected_id}, got: {message}"
            );
            assert_eq!(result.action, None, "/fleet {arg}");
        }
    }

    #[test]
    fn fleet_help_arg_distinguishes_durable_from_session_state() {
        let mut app = test_app();

        let result = FleetCmd::execute(&mut app, Some("help"));

        assert!(!result.is_error);
        assert!(result.action.is_none());
        let message = result.message.as_deref().unwrap_or_default();
        for surface in ["/fleet roster", "/fleet setup", "/fleet status"] {
            assert!(message.contains(surface), "help must describe {surface}");
        }
        for truth in [
            "current TUI session",
            "codewhale fleet status",
            ".codewhale/fleet.jsonl",
        ] {
            assert!(message.contains(truth), "help must distinguish {truth}");
        }
        for descriptor in operations_for_domain(ControlDomain::Fleet) {
            assert!(
                message.contains(descriptor.cli_invocation),
                "help must name the CLI twin of {}",
                descriptor.id
            );
        }
    }

    #[test]
    fn fleet_unknown_arg_reports_error() {
        let mut app = test_app();

        let result = FleetCmd::execute(&mut app, Some("bogus"));

        assert!(result.is_error);
        assert!(result.action.is_none());
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("Unknown /fleet target 'bogus'"))
        );
    }

    #[test]
    fn fleet_aliases_are_registered_on_command_info() {
        assert!(FleetCmd::info().aliases.contains(&"loadout"));
    }

    #[test]
    fn slash_command_and_cli_agree_on_fleet_verb_ids() {
        for descriptor in operations_for_domain(ControlDomain::Fleet) {
            assert_eq!(descriptor.slash_command, COMMAND_INFO.name);
            assert_eq!(descriptor.hotbar_action_id(), "slash.fleet");
            assert!(
                COMMAND_INFO.usage.contains(descriptor.verb) || descriptor.verb == "restart",
                "/fleet usage must document {} or declare it CLI-only",
                descriptor.verb
            );
            assert!(descriptor.offers(ControlSurface::Cli));
        }
        assert!(
            !COMMAND_INFO.requires_required_argument(),
            "/fleet must stay directly runnable from the palette and hotbar"
        );
    }
}
