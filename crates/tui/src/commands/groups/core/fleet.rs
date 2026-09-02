//! `/pod` command (`/fleet` remains a compatibility alias).
//!
//! Pod = who. Bare `/pod` (and `/pod roster`) opens the familiar roster
//! surface for the selected Pod; `/pod setup` opens the authoring wizard.
//! `/pod pods` (compatibility alias: `fleets`; other aliases: `saved`, `manage`)
//! opens the named-Pod picker
//! for switching between saved configurations — never the primary face.
//! `/pod list|status|interrupt|resume` are control-plane verbs that run
//! against the **durable** workspace ledger through the shared contract in
//! `codewhale-lane`, exactly as `codewhale pod …` does (#1888, #4022).
//!
//! `/pod status` used to show the current TUI session's sub-agents. That was
//! a different thing wearing the same name: session sub-agents are not the
//! durable Pod ledger, and a run started by `codewhale pod run` never
//! appeared. The session view is still reachable as `/pod workers` (and
//! `/subagents`), now labelled as what it is.

use codewhale_lane::control::operations_for_domain;
use codewhale_lane::{ControlDomain, ControlOperation, ControlSurface};

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::fleet::control::execute_fleet_control;
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "pod",
    aliases: &["fleet", "loadout", "party"],
    usage: "/pod [members|models|add <provider> <model> [role…]|remove <provider> <model>|setup|pods|workers|save|save-as|list|status|runs|interrupt <worker-id>|resume <run-id>]",
    description_id: MessageId::CmdFleetDescription,
};

pub(in crate::commands) struct FleetCmd;

fn help_text() -> String {
    let mut out = String::from(
        "Usage: /pod [members|setup|pods|workers|save|save-as|list|status|runs|interrupt <worker-id>|resume <run-id>]\n\n\
         Pod is who. /pod (or /pod members) opens the Pod member list and orchestration state — \
         each member's role, model, and access. /pod setup opens the authoring wizard. \
         /pod pods (or saved/manage) switches between named saved Pods; /pod fleets remains \
         accepted as a compatibility alias.\n\n\
         /pod list, status, interrupt, and resume act on the durable .codewhale/fleet.jsonl \
         ledger for this workspace — the same records `codewhale pod` reads and writes. \
         /pod workers (and /subagents) shows sub-agents in the current TUI session only, which \
         is a different set: it does not include durable Pod runs. /fleet and `codewhale fleet` \
         remain accepted as compatibility aliases; the ledger file, saved rosters, and config \
         tables keep the Fleet name.\n",
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

fn fleet_models_text(app: &App) -> String {
    use crate::fleet::members::fleet_models;
    let models = fleet_models(&app.workspace);
    if models.is_empty() {
        return "Your fleet is the session model only. Add one: /pod add <provider> <model> [role…] (or ⇧F on a row in /model).".to_string();
    }
    let mut lines = vec![format!(
        "Your fleet `{}` ({} models)",
        models[0].fleet,
        models.len()
    )];
    for member in &models {
        let provider = crate::config::ApiProvider::parse(&member.provider);
        let facts = provider
            .and_then(|p| crate::provider_lake::catalog_offering_for_model(p, &member.model))
            .map(|row| {
                let mut parts = Vec::new();
                if let Some(cost) = row.cost.as_ref()
                    && let (Some(input), Some(output)) = (cost.input, cost.output)
                {
                    parts.push(format!("${input:.2}/{output:.2} per M"));
                }
                if let Some(limit) = row.limit.as_ref()
                    && let Some(context) = limit.context
                {
                    parts.push(format!("{}k ctx", context / 1000));
                }
                if row.tool_call == Some(true) {
                    parts.push("tools".to_string());
                }
                parts.join(" · ")
            })
            .filter(|facts| !facts.is_empty())
            .map(|facts| format!(" · {facts}"))
            .unwrap_or_default();
        lines.push(format!(
            "  {}/{} · {}{facts}",
            member.provider,
            member.model,
            member.roles_label()
        ));
    }
    lines.push(
        "Add: /pod add <provider> <model> [role…] · Remove: /pod remove <provider> <model>"
            .to_string(),
    );
    lines.join("\n")
}

/// Whether `provider_id` names a provider the user has configured — active
/// route, explicit `[providers.<id>]` table, or usable credentials. Reuses
/// the same predicate as the `/provider` and `/model` pickers.
fn provider_id_is_configured(app: &App, provider_id: &str) -> bool {
    let provider_id = provider_id.trim();
    if provider_id.is_empty() {
        return false;
    }
    if let Some(provider) = crate::config::ApiProvider::parse(provider_id) {
        return crate::config::provider_is_configured_for_active(
            &app.config,
            provider,
            app.api_provider,
        );
    }
    // Named custom provider: allow the active custom route, or any explicit
    // `[providers.<name>]` table.
    if app.api_provider == crate::config::ApiProvider::Custom
        && app
            .provider_identity_for_persistence()
            .eq_ignore_ascii_case(provider_id)
    {
        return true;
    }
    app.config.providers.as_ref().is_some_and(|providers| {
        providers
            .custom
            .keys()
            .any(|name| name.eq_ignore_ascii_case(provider_id))
    })
}

fn fleet_add(app: &App, target: Option<&str>) -> CommandResult {
    let mut words = target.unwrap_or_default().split_whitespace();
    let (Some(provider), Some(model)) = (words.next(), words.next()) else {
        return CommandResult::error(
            "Usage: /pod add <provider> <model> [role…] — e.g. /pod add openrouter z-ai/glm-5.3-flash explore",
        );
    };
    let roles: Vec<String> = words.map(str::to_string).collect();
    if !provider_id_is_configured(app, provider) {
        return CommandResult::error(format!(
            "`{provider}` is not a configured provider. Configure it in ~/.codewhale/config.toml or switch to it with `/provider` before adding it to a Pod."
        ));
    }
    if let Some(known) = crate::config::ApiProvider::parse(provider) {
        let served = crate::provider_lake::all_catalog_models_for_provider(known);
        if !served.is_empty() && !served.iter().any(|id| id.eq_ignore_ascii_case(model)) {
            return CommandResult::error(format!(
                "{provider} does not serve `{model}` in the current catalog; run /models to see what it serves, or /pod add with the exact id it lists."
            ));
        }
    }
    match crate::fleet::members::add_fleet_model(&app.workspace, provider, model, &roles) {
        Ok(change) => CommandResult::message(crate::fleet::members::change_receipt(
            provider, model, &change,
        )),
        Err(error) => CommandResult::error(format!("Could not add to the fleet: {error}")),
    }
}

fn fleet_remove(app: &App, target: Option<&str>) -> CommandResult {
    let mut words = target.unwrap_or_default().split_whitespace();
    let (Some(provider), Some(model)) = (words.next(), words.next()) else {
        return CommandResult::error("Usage: /pod remove <provider> <model>");
    };
    match crate::fleet::members::remove_fleet_model(&app.workspace, provider, model) {
        Ok(change) => CommandResult::message(crate::fleet::members::change_receipt(
            provider, model, &change,
        )),
        Err(error) => CommandResult::error(format!("Could not remove from the fleet: {error}")),
    }
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
            // Primary face: the familiar roster for the selected Pod.
            // Named-Pod switching lives under /pod pods — never between
            // the operator and their Pod.
            return CommandResult::action(AppAction::OpenFleetRoster);
        };
        match verb {
            "save" | "update" => {
                // Explicit persistence of the pending session route into the
                // selected Pod's operator. Only an explicit command can
                // write a saved Pod after an in-session route change.
                let message = app.apply_route_save_choice(
                    crate::tui::views::route_save_prompt::RouteSaveChoice::UpdateFleet,
                );
                return CommandResult::message(message);
            }
            "save-as" | "saveas" => {
                let message = app.apply_route_save_choice(
                    crate::tui::views::route_save_prompt::RouteSaveChoice::SaveAsNewFleet,
                );
                return CommandResult::message(message);
            }
            _ => {}
        }
        match verb {
            // The fleet as models (design §10 F1): what the person added,
            // provider-exact, with the roles each model fills.
            "models" | "model" => CommandResult::message(fleet_models_text(app)),
            "add" => fleet_add(app, target),
            "remove" | "rm" | "drop" => fleet_remove(app, target),
            "members" | "member" | "roster" | "party" | "loadout" | "roles" | "role"
            | "profiles" | "profile" => CommandResult::action(AppAction::OpenFleetRoster),
            "setup" | "edit" | "new" => CommandResult::action(AppAction::OpenFleetSetup),
            // Named saved Pods — secondary surface for multi-Pod pick/switch.
            // Deliberately not "list": that verb is the durable ledger (#4022).
            "pods" | "fleets" | "saved" | "manage" => {
                CommandResult::action(AppAction::OpenFleetList)
            }
            // The current-session sub-agent projection, named for what it is.
            "workers" | "worker" | "agents" | "subagents" => super::core::subagents(app),
            "help" | "?" => CommandResult::message(help_text()),
            other => match ControlOperation::parse_verb(ControlDomain::Fleet, other) {
                Some(operation) => run_control(app, operation, target),
                None => CommandResult::error(format!(
                    "Unknown /pod target '{other}'. Use members, setup, pods, list, status, \
                     workers, interrupt <worker-id>, or resume <run-id>. /pod fleets remains \
                     accepted for compatibility."
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
    fn pod_models_add_and_remove_round_trip_through_the_selected_pod() {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());
        let workspace = temp.path().join("repo");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut app = app_in(workspace.clone());
        app.config
            .provider_config_for_mut(crate::config::ApiProvider::Openrouter)
            .api_key = Some("test-key".to_string());

        let empty = FleetCmd::execute(&mut app, Some("models"));
        assert!(
            empty
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("session model only"),
            "got: {empty:?}"
        );

        let added = FleetCmd::execute(&mut app, Some("add openrouter z-ai/glm-5.3-flash explore"));
        let text = added.message.clone().unwrap_or_default();
        assert!(
            text.contains("Added openrouter/z-ai/glm-5.3-flash as explore"),
            "got: {text}"
        );
        assert!(text.contains("new user-global Pod"), "got: {text}");

        let listed = FleetCmd::execute(&mut app, Some("models"))
            .message
            .unwrap_or_default();
        assert!(
            listed.contains("openrouter/z-ai/glm-5.3-flash · explore"),
            "got: {listed}"
        );

        let removed = FleetCmd::execute(&mut app, Some("remove openrouter z-ai/glm-5.3-flash"));
        assert!(
            removed
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("Removed openrouter/z-ai/glm-5.3-flash (explore)"),
            "got: {removed:?}"
        );
        assert!(crate::fleet::members::fleet_models(&workspace).is_empty());
    }

    #[test]
    fn pod_add_rejects_a_model_the_provider_does_not_serve() {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());
        let workspace = temp.path().join("repo");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut app = app_in(workspace.clone());
        app.config
            .provider_config_for_mut(crate::config::ApiProvider::Anthropic)
            .api_key = Some("test-key".to_string());
        app.config
            .provider_config_for_mut(crate::config::ApiProvider::Openrouter)
            .api_key = Some("test-key".to_string());
        let result = FleetCmd::execute(&mut app, Some("add anthropic not-a-real-model"));
        assert!(result.is_error, "got: {result:?}");
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("does not serve"),
            "got: {result:?}"
        );
        assert!(crate::fleet::members::fleet_models(&workspace).is_empty());

        let usage = FleetCmd::execute(&mut app, Some("add openrouter"));
        assert!(
            usage.is_error
                && usage
                    .message
                    .as_deref()
                    .unwrap_or_default()
                    .contains("Usage"),
            "got: {usage:?}"
        );
    }

    #[test]
    fn pod_add_rejects_an_unconfigured_provider() {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());
        let workspace = temp.path().join("repo");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut app = app_in(workspace.clone());
        let result = FleetCmd::execute(&mut app, Some("add unknown-provider some-model"));
        assert!(result.is_error, "got: {result:?}");
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("not a configured provider"),
            "got: {result:?}"
        );
        assert!(crate::fleet::members::fleet_models(&workspace).is_empty());
    }

    #[test]
    fn pod_command_opens_roster_view() {
        let mut app = test_app();

        let result = FleetCmd::execute(&mut app, None);

        assert_eq!(result.action, Some(AppAction::OpenFleetRoster));
        assert!(result.message.is_none());
    }

    #[test]
    fn pod_pods_is_canonical_and_fleets_remains_a_compatibility_alias() {
        for arg in ["pods", "fleets", "saved", "manage"] {
            let mut app = test_app();

            let result = FleetCmd::execute(&mut app, Some(arg));

            assert_eq!(result.action, Some(AppAction::OpenFleetList), "{arg}");
            assert!(result.message.is_none(), "{arg}");
        }
    }

    #[test]
    fn pod_pods_and_legacy_fleets_invocations_dispatch_identically() {
        let mut pod_app = test_app();
        let mut fleet_app = test_app();

        let pod = crate::commands::execute("/pod pods", &mut pod_app);
        let fleet = crate::commands::execute("/pod fleets", &mut fleet_app);

        assert_eq!(pod.action, Some(AppAction::OpenFleetList));
        assert_eq!(pod.action, fleet.action);
        assert_eq!(pod.message, fleet.message);
        assert_eq!(pod.is_error, fleet.is_error);
    }

    #[test]
    fn pod_members_and_roster_aliases_open_roster_view() {
        for arg in [
            "members", "member", "roster", "party", "loadout", "roles", "role", "profiles",
            "profile",
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
            "/pod status must not open the session sub-agent view"
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
                "/pod {arg} must report {expected_id}, got: {message}"
            );
            assert_eq!(result.action, None, "/pod {arg}");
        }
    }

    #[test]
    fn fleet_help_arg_distinguishes_durable_from_session_state() {
        let mut app = test_app();

        let result = FleetCmd::execute(&mut app, Some("help"));

        assert!(!result.is_error);
        assert!(result.action.is_none());
        let message = result.message.as_deref().unwrap_or_default();
        for surface in ["/pod members", "/pod setup", "/pod pods", "/pod status"] {
            assert!(message.contains(surface), "help must describe {surface}");
        }
        assert!(
            message
                .contains("/fleet and `codewhale fleet` remain accepted as compatibility aliases"),
            "help must document the one-way compatibility boundary"
        );
        assert!(
            message.contains("/pod fleets remains accepted as a compatibility alias"),
            "help must disclose the saved-Pod compatibility alias"
        );
        assert!(
            message.contains("config tables keep the Fleet name"),
            "help must name what keeps the Fleet serialization spelling"
        );
        for truth in [
            "current TUI session",
            "codewhale pod status",
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
        assert!(result
            .message
            .as_deref()
            .is_some_and(|message| message.contains("Unknown /pod target 'bogus'")));
        assert!(result
            .message
            .as_deref()
            .is_some_and(|message| message.contains("Use members, setup, pods")));
    }

    #[test]
    fn fleet_aliases_are_registered_on_command_info() {
        assert_eq!(FleetCmd::info().name, "pod");
        assert!(FleetCmd::info().aliases.contains(&"fleet"));
        assert!(FleetCmd::info().aliases.contains(&"loadout"));
        assert!(FleetCmd::info().usage.contains("pods"));
        assert!(FleetCmd::info().usage.contains("workers"));
        assert!(FleetCmd::info().usage.contains("save-as"));
        assert!(!FleetCmd::info().usage.contains("fleets"));
    }

    #[test]
    fn pod_and_legacy_fleet_invocations_dispatch_identically() {
        for invocation in ["/pod", "/fleet"] {
            let mut app = test_app();
            let result = crate::commands::execute(invocation, &mut app);
            assert_eq!(
                result.action,
                Some(AppAction::OpenFleetRoster),
                "{invocation}"
            );
            assert!(!result.is_error, "{invocation}");
        }

        let canonical = crate::commands::get_command_info("pod").expect("canonical /pod");
        let compatibility =
            crate::commands::get_command_info("fleet").expect("compatibility /fleet");
        assert!(std::ptr::eq(canonical, compatibility));
        assert_eq!(compatibility.name, "pod");

        let workspace = tempfile::tempdir().expect("workspace");
        let mut pod_app = app_in(workspace.path().to_path_buf());
        let mut fleet_app = app_in(workspace.path().to_path_buf());
        let pod_status = crate::commands::execute("/pod status", &mut pod_app);
        let fleet_status = crate::commands::execute("/fleet status", &mut fleet_app);
        assert_eq!(pod_status.action, fleet_status.action);
        assert_eq!(pod_status.message, fleet_status.message);
        assert_eq!(pod_status.is_error, fleet_status.is_error);
    }

    #[test]
    fn slash_command_and_cli_agree_on_fleet_verb_ids() {
        for descriptor in operations_for_domain(ControlDomain::Fleet) {
            assert_eq!(descriptor.slash_command, COMMAND_INFO.name);
            assert_eq!(descriptor.hotbar_action_id(), "slash.pod");
            assert!(
                COMMAND_INFO.usage.contains(descriptor.verb) || descriptor.verb == "restart",
                "/pod usage must document {} or declare it CLI-only",
                descriptor.verb
            );
            assert!(descriptor.offers(ControlSurface::Cli));
        }
        assert!(
            !COMMAND_INFO.requires_required_argument(),
            "/pod must stay directly runnable from the palette and hotbar"
        );
    }
}
