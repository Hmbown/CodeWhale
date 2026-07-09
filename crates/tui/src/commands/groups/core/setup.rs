//! `/setup` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::config::ApiProvider;
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

use super::CommandResult;
use codewhale_config::SetupStep;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "setup",
    aliases: &[],
    usage: "/setup",
    description_id: MessageId::CmdSetupDescription,
};

pub(in crate::commands) struct SetupCmd;

impl RegisterCommand for SetupCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(_app: &mut App, arg: Option<&str>) -> CommandResult {
        let target = arg.map(str::trim).filter(|arg| !arg.is_empty());
        if let Some(arg) = target {
            let mut parts = arg.split_whitespace();
            if matches!(parts.next(), Some("provider" | "providers"))
                && let Some(raw_provider) = parts.next()
            {
                if parts.next().is_some() {
                    return CommandResult::error(
                        "Usage: /setup provider [provider-name]".to_string(),
                    );
                }
                let Some(provider) = ApiProvider::parse(raw_provider) else {
                    return CommandResult::error(format!(
                        "Unknown provider '{raw_provider}'. Expected: {}.",
                        ApiProvider::names_hint()
                    ));
                };
                return CommandResult::action(AppAction::OpenProviderSetup {
                    provider: Some(provider),
                });
            }
        }

        match target {
            None | Some("open" | "wizard" | "checkpoint") => {
                CommandResult::action(AppAction::OpenSetupWizard)
            }
            Some("provider" | "providers" | "model" | "models" | "route") => {
                CommandResult::action(AppAction::OpenSetupWizardAt {
                    step: SetupStep::ProviderModel,
                })
            }
            Some("runtime" | "posture" | "trust" | "sandbox") => {
                CommandResult::action(AppAction::OpenSetupWizardAt {
                    step: SetupStep::TrustSandbox,
                })
            }
            Some("constitution" | "law") => CommandResult::action(AppAction::OpenSetupWizardAt {
                step: SetupStep::Constitution,
            }),
            Some("status" | "report" | "verification" | "verify") => {
                CommandResult::action(AppAction::OpenSetupWizardAt {
                    step: SetupStep::Verification,
                })
            }
            Some("operate" | "fleet" | "operate-fleet" | "operate_fleet") => {
                CommandResult::action(AppAction::OpenSetupWizardAt {
                    step: SetupStep::OperateFleet,
                })
            }
            Some("hotbar" | "hotkeys" | "shortcuts" | "keys") => {
                CommandResult::action(AppAction::OpenSetupWizardAt {
                    step: SetupStep::Hotbar,
                })
            }
            Some("tools" | "tool" | "mcp" | "tools-mcp" | "tools_mcp" | "skills" | "plugins") => {
                CommandResult::action(AppAction::OpenSetupWizardAt {
                    step: SetupStep::ToolsMcp,
                })
            }
            Some(
                "remote" | "remote-runtime" | "remote_runtime" | "cloud" | "bridge" | "mobile"
                | "phone",
            ) => CommandResult::action(AppAction::OpenSetupWizardAt {
                step: SetupStep::RemoteRuntime,
            }),
            Some("persistence" | "persist" | "storage") => {
                CommandResult::action(AppAction::OpenSetupWizardAt {
                    step: SetupStep::Persistence,
                })
            }
            Some(other) => CommandResult::error(format!(
                "Unknown /setup target '{other}'. Try `/setup` to open the setup wizard."
            )),
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
    fn setup_command_opens_wizard() {
        let mut app = test_app();

        let result = SetupCmd::execute(&mut app, None);

        assert_eq!(result.action, Some(AppAction::OpenSetupWizard));
        assert!(result.message.is_none());
    }

    #[test]
    fn setup_checkpoint_alias_opens_wizard() {
        let mut app = test_app();

        let result = SetupCmd::execute(&mut app, Some("checkpoint"));

        assert_eq!(result.action, Some(AppAction::OpenSetupWizard));
        assert!(result.message.is_none());
    }

    #[test]
    fn setup_report_opens_verification_step() {
        let mut app = test_app();

        let result = SetupCmd::execute(&mut app, Some("report"));

        assert_eq!(
            result.action,
            Some(AppAction::OpenSetupWizardAt {
                step: SetupStep::Verification
            })
        );
        assert!(result.message.is_none());
    }

    #[test]
    fn setup_named_steps_open_matching_wizard_cards() {
        let cases = [
            ("provider", SetupStep::ProviderModel),
            ("model", SetupStep::ProviderModel),
            ("runtime", SetupStep::TrustSandbox),
            ("posture", SetupStep::TrustSandbox),
            ("constitution", SetupStep::Constitution),
            ("hotbar", SetupStep::Hotbar),
            ("shortcuts", SetupStep::Hotbar),
            ("tools", SetupStep::ToolsMcp),
            ("tool", SetupStep::ToolsMcp),
            ("tools-mcp", SetupStep::ToolsMcp),
            ("tools_mcp", SetupStep::ToolsMcp),
            ("mcp", SetupStep::ToolsMcp),
            ("skills", SetupStep::ToolsMcp),
            ("plugins", SetupStep::ToolsMcp),
            ("remote", SetupStep::RemoteRuntime),
            ("cloud", SetupStep::RemoteRuntime),
            ("persistence", SetupStep::Persistence),
            ("persist", SetupStep::Persistence),
            ("storage", SetupStep::Persistence),
        ];

        for (arg, step) in cases {
            let mut app = test_app();
            let result = SetupCmd::execute(&mut app, Some(arg));
            assert_eq!(
                result.action,
                Some(AppAction::OpenSetupWizardAt { step }),
                "{arg}"
            );
            assert!(result.message.is_none(), "{arg}");
        }
    }

    #[test]
    fn setup_fleet_opens_operate_readiness_step() {
        let mut app = test_app();

        let result = SetupCmd::execute(&mut app, Some("fleet"));

        assert_eq!(
            result.action,
            Some(AppAction::OpenSetupWizardAt {
                step: SetupStep::OperateFleet
            })
        );
        assert!(result.message.is_none());
    }

    #[test]
    fn setup_provider_named_opens_provider_setup_catalog() {
        let mut app = test_app();

        let result = SetupCmd::execute(&mut app, Some("provider anthropic"));

        assert_eq!(
            result.action,
            Some(AppAction::OpenProviderSetup {
                provider: Some(ApiProvider::Anthropic)
            })
        );
        assert!(result.message.is_none());
    }

    #[test]
    fn setup_provider_named_rejects_unknown_provider() {
        let mut app = test_app();

        let result = SetupCmd::execute(&mut app, Some("provider imaginary"));

        assert!(result.action.is_none());
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("Unknown provider 'imaginary'"))
        );
    }
}
