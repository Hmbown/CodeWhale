//! Balance: query the active provider's account balance or credit status.
//!
//! Dispatches an async balance check via `AppAction::CheckBalance` for
//! providers with known billing endpoints. Unsupported providers return
//! a clear message immediately.

use crate::config::ApiProvider;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

/// Query provider account balance / credits.
///
/// `/balance` defaults to the active provider; `/balance <provider>` lets
/// the user target a specific provider by name (e.g. `deepseek`, `openrouter`).
pub fn balance(app: &mut App, arg: Option<&str>) -> CommandResult {
    let provider = if let Some(arg) = arg {
        match ApiProvider::parse(arg) {
            Some(p) => p,
            None => {
                return CommandResult::error(format!(
                    "Unknown provider '{}'. Supported: deepseek, openrouter, novita.",
                    arg
                ));
            }
        }
    } else {
        app.api_provider
    };

    match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::Openrouter => {
            CommandResult::action(AppAction::CheckBalance { provider })
        }
        ApiProvider::Novita => CommandResult::message(format!(
            "Balance check for {} is not yet implemented. Check the provider dashboard for account balance details.",
            provider.display_name()
        )),
        _ => CommandResult::message(format!(
            "Balance check is not supported for {}. Check the provider dashboard for account balance details.",
            provider.display_name()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use std::path::PathBuf;

    fn create_test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-flash".to_string(),
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
            start_in_agent_mode: true,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
    }

    #[test]
    fn test_balance_deepseek_returns_check_balance_action() {
        let mut app = create_test_app();
        app.api_provider = ApiProvider::Deepseek;
        let result = balance(&mut app, None);
        assert!(
            matches!(
                result.action,
                Some(AppAction::CheckBalance {
                    provider: ApiProvider::Deepseek
                })
            ),
            "expected CheckBalance action for DeepSeek, got {:?}",
            result.action
        );
    }

    #[test]
    fn test_balance_explicit_provider() {
        let mut app = create_test_app();
        app.api_provider = ApiProvider::Ollama;
        let result = balance(&mut app, Some("deepseek"));
        assert!(
            matches!(
                result.action,
                Some(AppAction::CheckBalance {
                    provider: ApiProvider::Deepseek
                })
            ),
            "expected CheckBalance for explicit deepseek, got {:?}",
            result.action
        );
    }

    #[test]
    fn test_balance_unknown_provider_returns_error() {
        let mut app = create_test_app();
        let result = balance(&mut app, Some("unknown_provider"));
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("Unknown provider"));
    }

    #[test]
    fn test_balance_unsupported_provider_returns_message() {
        let mut app = create_test_app();
        app.api_provider = ApiProvider::Ollama;
        let result = balance(&mut app, None);
        assert!(result.message.is_some());
        assert!(!result.is_error);
        let msg = result.message.unwrap();
        assert!(msg.contains("not supported"));
    }
}
