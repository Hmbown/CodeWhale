//! `/lsp` command — enable/disable LSP integration.
//!
//! Bridges to the host LSP state through the portable project facet
//! (FEAT-021 D3): the handler composes byte-identical output from typed
//! status/set delegates; the TUI adapter owns all host-side LSP behavior.

use codewhale_command_contract::facets::CommandProjectContext;
use codewhale_command_contract::handler::{CommandContexts, CommandHandler};
use codewhale_command_contract::metadata::{CommandInfo, RegisterCommand};

use crate::commands::CommandResult;

pub(in crate::commands) const LSP_INFO: CommandInfo = CommandInfo {
    name: "lsp",
    aliases: &[],
    usage: "/lsp [on|off|status]",
    description_key: "cmd_lsp_description",
};

pub(in crate::commands) struct LspCmd;

impl RegisterCommand<CommandResult> for LspCmd {
    fn info() -> &'static CommandInfo {
        &LSP_INFO
    }

    fn handler() -> CommandHandler<CommandResult> {
        CommandHandler::Contextual {
            capabilities: codewhale_command_contract::handler::CommandCapabilities::PROJECT,
            handler: lsp_contextual,
        }
    }
}

/// Contextual `/lsp` dispatch (FEAT-021 Phase 4).
///
/// Destructures the declared `PROJECT` facet with a safe missing-facet error;
/// the portable [`lsp`] handler never panics on absent capabilities.
fn lsp_contextual(contexts: CommandContexts<'_>, arg: Option<&str>) -> CommandResult {
    let mut parts = contexts.into_parts();
    let Some(project) = parts.project.as_deref_mut() else {
        return CommandResult::error("Command capability unavailable: project");
    };
    lsp(project, arg)
}

/// Portable `/lsp` dispatch (FEAT-021 Phase 4).
///
/// The handler consumes only the typed project facet; all concrete host LSP
/// behavior lives in the TUI adapter (D3). Messages are byte-identical to the
/// baseline `config::config::lsp_command` output.
fn lsp(project: &mut dyn CommandProjectContext, arg: Option<&str>) -> CommandResult {
    let raw = arg.map(str::trim).unwrap_or("");

    match raw {
        "" | "status" => {
            let enabled = project.lsp_enabled();
            let status = if enabled { "on" } else { "off" };
            CommandResult::message(format!(
                "LSP diagnostics are currently **{status}**.\n\n\
                 Use `/lsp on` to enable or `/lsp off` to disable inline diagnostics after file edits."
            ))
        }
        "on" | "enable" | "1" | "true" => {
            if let Err(error) = project.lsp_set(true) {
                return CommandResult::error(format!("Failed to enable LSP diagnostics: {error}"));
            }
            CommandResult::message(
                "LSP diagnostics enabled — file edit results will include compiler errors and warnings when available.",
            )
        }
        "off" | "disable" | "0" | "false" => {
            if let Err(error) = project.lsp_set(false) {
                return CommandResult::error(format!("Failed to disable LSP diagnostics: {error}"));
            }
            CommandResult::message("LSP diagnostics disabled.")
        }
        other => CommandResult::error(format!(
            "Unknown /lsp argument `{other}`. Use `/lsp on`, `/lsp off`, or `/lsp status`."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codewhale_command_contract::facets::{
        ProjectGoalState, ProjectGoalStatus, ProjectShareProjection,
    };

    /// Deterministic fake project facet over portable values only.
    struct FakeProject {
        lsp_enabled: bool,
    }

    impl CommandProjectContext for FakeProject {
        fn lsp_enabled(&self) -> bool {
            self.lsp_enabled
        }

        fn lsp_set(&mut self, enabled: bool) -> Result<(), String> {
            self.lsp_enabled = enabled;
            Ok(())
        }

        fn share_projection(&self) -> ProjectShareProjection {
            ProjectShareProjection {
                history_is_empty: true,
                history_len: 0,
                model: String::new(),
                mode_label: String::new(),
            }
        }

        fn goal_state(&self) -> ProjectGoalState {
            ProjectGoalState {
                objective: None,
                status: ProjectGoalStatus::Active,
                pause_reason: None,
                started_at_elapsed_seconds: None,
                time_used_seconds: 0,
                token_budget: None,
                tokens_used: 0,
                session_total_tokens: 0,
                continuation_count: 0,
                pending_controls: false,
                last_known_objective: None,
                last_known_status: None,
                conversation_present: false,
                is_loading: false,
                goal_continuation_waiting: false,
            }
        }
    }

    fn run(arg: Option<&str>) -> (CommandResult, bool) {
        let mut project = FakeProject { lsp_enabled: false };
        let result = lsp(&mut project, arg);
        (result, project.lsp_enabled)
    }

    #[test]
    fn status_off_matches_baseline() {
        let (result, _) = run(Some("status"));
        let msg = result.message.expect("status must be a message");
        assert!(msg.contains("currently **off**"));
        assert!(msg.contains("Use `/lsp on` to enable or `/lsp off` to disable"));
    }

    #[test]
    fn bare_status_is_same_as_status() {
        let (result, _) = run(None);
        let msg = result.message.expect("bare must be a message");
        assert!(msg.contains("currently **off**"));
    }

    #[test]
    fn enable_synonyms_set_state_and_report() {
        for synonym in ["on", "enable", "1", "true"] {
            let (result, enabled) = run(Some(synonym));
            assert!(enabled, "{synonym} must enable");
            let msg = result.message.expect("enable must be a message");
            assert!(msg.contains("LSP diagnostics enabled"));
        }
    }

    #[test]
    fn disable_synonyms_clear_state_and_report() {
        let mut project = FakeProject { lsp_enabled: true };
        for synonym in ["off", "disable", "0", "false"] {
            let result = lsp(&mut project, Some(synonym));
            assert!(!project.lsp_enabled, "{synonym} must disable");
            let msg = result.message.expect("disable must be a message");
            assert_eq!(msg, "LSP diagnostics disabled.");
        }
    }

    #[test]
    fn status_reflects_current_state() {
        let mut project = FakeProject { lsp_enabled: true };
        let result = lsp(&mut project, Some("status"));
        assert!(result.message.unwrap().contains("currently **on**"));
    }

    #[test]
    fn unknown_argument_errors() {
        let (result, enabled) = run(Some("bogus"));
        assert!(!enabled);
        assert!(result.is_error, "unknown must error");
        let err = result.message.expect("unknown must carry a message");
        assert!(err.contains("Unknown /lsp argument `bogus`"));
        assert!(err.contains("/lsp on`, `/lsp off`, or `/lsp status`"));
    }

    #[test]
    fn missing_project_facet_fails_safely() {
        // An empty envelope must fail safely — never panic — with the exact
        // capability-unavailable error.
        let result = lsp_contextual(CommandContexts::empty(), Some("status"));
        assert!(result.is_error);
        assert!(
            result
                .message
                .unwrap()
                .contains("Command capability unavailable: project"),
            "missing project facet must fail safely"
        );
    }
}
