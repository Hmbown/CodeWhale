//! `/fleet` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "fleet",
    aliases: &["loadout", "party"],
    usage: "/fleet [setup|party|status]",
    description_id: MessageId::CmdFleetDescription,
};

pub(in crate::commands) struct FleetCmd;

impl RegisterCommand for FleetCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        match arg.map(str::trim).filter(|arg| !arg.is_empty()) {
            None | Some("setup" | "roles" | "role" | "profiles" | "profile" | "loadout") => {
                CommandResult::action(AppAction::OpenFleetSetup)
            }
            Some("party" | "team" | "roster") => party_roster(app),
            Some("status" | "workers" | "worker" | "agents" | "subagents" | "list") => {
                super::core::subagents(app)
            }
            Some("help" | "?") => CommandResult::message(
                "Usage: /fleet [setup|party|status]\n\n/fleet opens the setup flow (authors ONE worker profile per pass). /fleet party lists the saved party roster — every profile under .codewhale/agents/ with its per-slot role, model class, and pinned model. /fleet status shows Fleet worker status; /subagents is a compatibility shortcut for the same status view.",
            ),
            Some(other) => CommandResult::error(format!(
                "Unknown /fleet target '{other}'. Use `/fleet setup`, `/fleet party`, or `/fleet status`."
            )),
        }
    }
}

/// `/fleet party` — an honest roster of the saved party.
///
/// The party is the set of worker profiles under `.codewhale/agents/`: one
/// profile per slot, each carrying its own role, model class/loadout, and
/// optional pinned model. The orchestrator is the active session. This is a
/// read-only report; `/fleet setup` authors members and `/fleet status`
/// shows live workers.
fn party_roster(app: &App) -> CommandResult {
    let profiles = match crate::fleet::profile::load_workspace_agent_profiles(&app.workspace) {
        Ok(profiles) => profiles,
        Err(err) => {
            return CommandResult::error(format!("Fleet party: cannot read profiles: {err}"));
        }
    };
    CommandResult::message(party_roster_message(&profiles, app.api_provider))
}

/// Render the roster text for the saved party against the active provider.
fn party_roster_message(
    profiles: &[crate::fleet::profile::AgentProfile],
    api_provider: crate::config::ApiProvider,
) -> String {
    let mut out = String::new();
    out.push_str("Fleet party — saved worker profiles in .codewhale/agents/\n\n");
    out.push_str("  orchestrator  this session — owns topology, dispatches slots, verifies returned claims\n");
    if profiles.is_empty() {
        out.push_str("  (no saved members yet)\n\n");
        out.push_str(
            "Each member is one worker slot with its own role, model class (fast/heavy/omni or a custom token), and optional pinned model. \
             /fleet setup authors one member per pass — run it again for more members.",
        );
        return out;
    }

    for (idx, profile) in profiles.iter().enumerate() {
        // Pre-flight the member's pins against the ACTIVE provider so a stale
        // pin (provider switched, model renamed) is visible here, not as an
        // opaque worker HTTP error mid-run.
        let selection = crate::fleet::worker_runtime::select_fleet_model(
            "auto",
            Some(api_provider),
            None,
            Some(profile),
        );
        let model = if !profile.profile.models.is_empty() {
            // Ranked preference list: best first, later entries are fallbacks.
            format!(
                "models {} (active: {})",
                profile.profile.models.join(" > "),
                selection.model
            )
        } else {
            format!(
                "model {}",
                profile
                    .profile
                    .model
                    .as_deref()
                    .filter(|model| !model.trim().is_empty())
                    .unwrap_or("inherit route")
            )
        };
        let class = match &profile.profile.loadout {
            codewhale_config::FleetLoadout::Custom(token) => {
                format!("{token} (custom token — routes auto)")
            }
            loadout => loadout.as_str().to_string(),
        };
        out.push_str(&format!(
            "  {}. {}  ·  role {}  ·  class {}  ·  {}\n",
            idx + 1,
            profile.id,
            profile.profile.role.name,
            class,
            model,
        ));
        if let Some(notice) = &selection.notice {
            out.push_str(&format!("     ⚠ {notice}\n"));
        }
    }
    out.push_str(&format!(
        "\n{count} member{plural}. Per-slot role/class/model come from each profile's role_hint, model_class_hint, and model fields. \
         /fleet setup adds a member (custom role and class supported); /fleet status shows live workers.",
        count = profiles.len(),
        plural = if profiles.len() == 1 { "" } else { "s" },
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::TuiOptions;
    use std::path::PathBuf;

    fn test_app() -> App {
        test_app_in(PathBuf::from("."))
    }

    fn test_app_in(workspace: PathBuf) -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace,
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
    fn fleet_command_opens_setup_view() {
        let mut app = test_app();

        let result = FleetCmd::execute(&mut app, None);

        assert_eq!(result.action, Some(AppAction::OpenFleetSetup));
        assert!(result.message.is_none());
    }

    #[test]
    fn fleet_status_arg_opens_worker_status_view() {
        for arg in ["status", "workers", "worker", "agents", "subagents", "list"] {
            let mut app = test_app();

            let result = FleetCmd::execute(&mut app, Some(arg));

            assert_eq!(result.action, Some(AppAction::ListSubAgents), "{arg}");
            assert!(result.message.is_none(), "{arg}");
        }
    }

    #[test]
    fn fleet_help_arg_returns_usage() {
        let mut app = test_app();

        let result = FleetCmd::execute(&mut app, Some("help"));

        assert!(!result.is_error);
        assert!(result.action.is_none());
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("/fleet status"))
        );
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
        assert!(FleetCmd::info().aliases.contains(&"party"));
    }

    #[test]
    fn fleet_party_reports_empty_roster_honestly() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = test_app_in(tmp.path().to_path_buf());

        let result = FleetCmd::execute(&mut app, Some("party"));

        assert!(!result.is_error);
        assert!(result.action.is_none(), "party must not open the wizard");
        let message = result.message.as_deref().unwrap();
        assert!(message.contains("no saved members yet"), "{message}");
        assert!(message.contains("orchestrator"), "{message}");
        assert!(message.contains("one member per pass"), "{message}");
    }

    #[test]
    fn fleet_party_lists_saved_profiles_with_per_slot_route() {
        let tmp = tempfile::TempDir::new().unwrap();
        let agents_dir = tmp.path().join(".codewhale/agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("reviewer.toml"),
            r#"
id = "reviewer"
role_hint = "reviewer"
model_class_hint = "heavy"
model = "deepseek-v4-pro"

[instructions]
text = "Read the diff. Report findings, then stop."
"#,
        )
        .unwrap();
        std::fs::write(
            agents_dir.join("scout.toml"),
            r#"
id = "scout"
role_hint = "scout"
model_class_hint = "fast"

[instructions]
text = "Read-first reconnaissance."
"#,
        )
        .unwrap();
        let mut app = test_app_in(tmp.path().to_path_buf());

        let result = FleetCmd::execute(&mut app, Some("party"));

        assert!(!result.is_error);
        assert!(result.action.is_none());
        let message = result.message.as_deref().unwrap();
        assert!(message.contains("orchestrator"), "{message}");
        assert!(
            message
                .contains("reviewer  ·  role reviewer  ·  class heavy  ·  model deepseek-v4-pro"),
            "{message}"
        );
        assert!(
            message.contains("scout  ·  role scout  ·  class fast  ·  model inherit route"),
            "{message}"
        );
        assert!(message.contains("2 members"), "{message}");
    }

    /// Load profiles from TOML in a temp workspace so roster tests can drive
    /// [`party_roster_message`] with an explicit provider (the App-based path
    /// inherits the ambient default provider, which varies per machine).
    fn profiles_from_toml(file_name: &str, toml: &str) -> Vec<crate::fleet::profile::AgentProfile> {
        let tmp = tempfile::TempDir::new().unwrap();
        let agents_dir = tmp.path().join(".codewhale/agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join(file_name), toml).unwrap();
        crate::fleet::profile::load_workspace_agent_profiles(tmp.path()).unwrap()
    }

    #[test]
    fn fleet_party_shows_ranked_model_preferences() {
        let profiles = profiles_from_toml(
            "builder.toml",
            r#"
id = "builder"
role_hint = "builder"
model_class_hint = "heavy"
models = ["deepseek-v4-pro", "glm-5.2"]

[instructions]
text = "Implement bounded changes."
"#,
        );

        let message = party_roster_message(&profiles, crate::config::ApiProvider::Deepseek);

        assert!(
            message.contains(
                "builder  ·  role builder  ·  class heavy  ·  models deepseek-v4-pro > glm-5.2"
            ),
            "{message}"
        );
        // On DeepSeek the top preference is served: active pick, no warning.
        assert!(message.contains("(active: deepseek-v4-pro)"), "{message}");
        assert!(!message.contains('⚠'), "{message}");
    }

    #[test]
    fn fleet_party_warns_when_no_pin_is_usable_on_active_provider() {
        let profiles = profiles_from_toml(
            "reviewer.toml",
            r#"
id = "reviewer"
role_hint = "reviewer"
model_class_hint = "heavy"
models = ["glm-5.2"]

[instructions]
text = "Read the diff."
"#,
        );

        let message = party_roster_message(&profiles, crate::config::ApiProvider::Deepseek);

        // glm-5.2 is foreign to the official DeepSeek provider: the roster must
        // surface the degradation instead of letting the spawn fail on the wire.
        assert!(message.contains("(active: auto)"), "{message}");
        assert!(message.contains('⚠'), "{message}");
        assert!(message.contains("class route"), "{message}");
    }

    #[test]
    fn fleet_party_labels_custom_class_tokens_honestly() {
        let profiles = profiles_from_toml(
            "scout.toml",
            r#"
id = "scout"
role_hint = "scout"
model_class_hint = "fastt"

[instructions]
text = "Scout."
"#,
        );

        let message = party_roster_message(&profiles, crate::config::ApiProvider::Deepseek);

        // A typo'd class token must not masquerade as a real class.
        assert!(
            message.contains("class fastt (custom token — routes auto)"),
            "{message}"
        );
    }

    #[test]
    fn fleet_party_lists_wizard_ratified_draft_member() {
        // JOURNEY: the exact TOML the wizard's ratify keypress persists
        // (FleetProfileDraft::render_toml at .codewhale/agents/<id>.toml)
        // must show up as a party member with its per-slot role/class/model.
        let crate::fleet::profile::UntrustedProfileParse::Drafted(draft) =
            crate::fleet::profile::FleetProfileDraft::from_untrusted_json(
                r#"{"id":"release-captain","display_name":"Release Captain","description":"Owns release verification.","role_hint":"verifier","model_class_hint":"heavy","model":"deepseek-v4-pro","instructions":"Verify receipts. Report. Stop."}"#,
            )
        else {
            panic!("draft should parse");
        };

        let tmp = tempfile::TempDir::new().unwrap();
        let agents_dir = tmp.path().join(".codewhale/agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join(draft.file_name()), draft.render_toml()).unwrap();
        let mut app = test_app_in(tmp.path().to_path_buf());

        let result = FleetCmd::execute(&mut app, Some("party"));

        assert!(!result.is_error);
        let message = result.message.as_deref().unwrap();
        assert!(
            message.contains(
                "release-captain  ·  role verifier  ·  class heavy  ·  model deepseek-v4-pro"
            ),
            "{message}"
        );
        assert!(message.contains("1 member"), "{message}");
    }

    #[test]
    fn fleet_party_surfaces_duplicate_id_load_errors() {
        // Two files claiming one id fail the loader; the roster must report
        // the reason instead of pretending the party is empty.
        let tmp = tempfile::TempDir::new().unwrap();
        let agents_dir = tmp.path().join(".codewhale/agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("a.toml"), "id = \"reviewer\"\n").unwrap();
        std::fs::write(agents_dir.join("b.toml"), "name = \"reviewer\"\n").unwrap();
        let mut app = test_app_in(tmp.path().to_path_buf());

        let result = FleetCmd::execute(&mut app, Some("party"));

        assert!(result.is_error);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("duplicate agent profile id reviewer")),
            "{:?}",
            result.message
        );
    }

    #[test]
    fn fleet_help_mentions_party() {
        let mut app = test_app();

        let result = FleetCmd::execute(&mut app, Some("help"));

        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("/fleet party"))
        );
    }
}
