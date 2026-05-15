//! Provider switching: flip between DeepSeek, hosted providers, and self-hosted
//! OpenAI-compatible DeepSeek V4 servers at runtime.
//!
//! `/provider` with no args opens the picker modal (#52). `/provider <name>`
//! keeps the v0.6.6 CLI form for muscle-memory + scripted use.

use crate::config::{ApiProvider, normalize_model_name};
use crate::tui::app::{App, AppAction};

use super::CommandResult;

/// Switch or view the current LLM backend.
///
/// With no args, opens the picker modal. With `<provider> [model]`, performs
/// the switch directly (e.g. `/provider nim flash` lands on
/// `deepseek-ai/deepseek-v4-flash`). The optional model accepts shorthand
/// (`flash`, `pro`, `v4-flash`, `v4-pro`) or any normal DeepSeek model ID.
pub fn provider(app: &mut App, args: Option<&str>) -> CommandResult {
    let trimmed = args.map(str::trim).filter(|s| !s.is_empty());
    let Some(args) = trimmed else {
        return CommandResult::action(AppAction::OpenProviderPicker);
    };

    let mut parts = args.split_whitespace();
    let name = parts.next().unwrap_or("");
    let model_arg = parts.next();

    let Some(target) = ApiProvider::parse(name) else {
        return CommandResult::error(format!(
            "Unknown provider '{name}'. Expected: deepseek, nvidia-nim, openai, atlascloud, openrouter, novita, fireworks, sglang, vllm, or ollama."
        ));
    };

    let model = match model_arg {
        None => None,
        Some(raw) if target == ApiProvider::Ollama => Some(raw.trim().to_string()),
        Some(raw) => match normalize_model_name(&expand_model_alias(raw)) {
            Some(normalized) => Some(normalized),
            None => {
                return CommandResult::error(format!(
                    "Invalid model '{raw}'. Try: flash, pro, deepseek-v4-flash, deepseek-v4-pro."
                ));
            }
        },
    };

    if target == app.api_provider && model.is_none() {
        return CommandResult::message(format!("Already on provider: {}", target.as_str()));
    }

    CommandResult::action(AppAction::SwitchProvider {
        provider: target,
        model,
    })
}

fn expand_model_alias(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "pro" | "v4-pro" => "deepseek-v4-pro".to_string(),
        "flash" | "v4-flash" => "deepseek-v4-flash".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::TuiOptions;
    use std::path::PathBuf;

    fn create_test_app() -> App {
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
        let mut app = App::new(options, &Config::default());
        app.ui_locale = crate::localization::Locale::En;
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app
    }

    #[test]
    fn no_args_opens_picker_modal() {
        let mut app = create_test_app();
        let result = provider(&mut app, None);
        assert!(result.message.is_none());
        assert_eq!(result.action, Some(AppAction::OpenProviderPicker));
    }

    #[test]
    fn unknown_provider_returns_error() {
        let mut app = create_test_app();
        let result = provider(&mut app, Some("anthropic"));
        let msg = result.message.expect("expected error message");
        assert!(msg.contains("Unknown provider"));
        assert!(msg.contains("openrouter"));
        assert!(msg.contains("novita"));
        assert!(result.action.is_none());
    }

    #[test]
    fn switch_to_openrouter_emits_action() {
        let mut app = create_test_app();
        let result = provider(&mut app, Some("openrouter"));
        match result.action {
            Some(AppAction::SwitchProvider { provider, model }) => {
                assert_eq!(provider, ApiProvider::Openrouter);
                assert_eq!(model, None);
            }
            other => panic!("expected SwitchProvider, got {other:?}"),
        }
    }

    #[test]
    fn switch_to_atlascloud_emits_action() {
        let mut app = create_test_app();
        let result = provider(&mut app, Some("atlascloud"));
        match result.action {
            Some(AppAction::SwitchProvider { provider, model }) => {
                assert_eq!(provider, ApiProvider::Atlascloud);
                assert_eq!(model, None);
            }
            other => panic!("expected SwitchProvider, got {other:?}"),
        }
    }

    #[test]
    fn switch_to_novita_emits_action() {
        let mut app = create_test_app();
        let result = provider(&mut app, Some("novita"));
        match result.action {
            Some(AppAction::SwitchProvider { provider, model }) => {
                assert_eq!(provider, ApiProvider::Novita);
                assert_eq!(model, None);
            }
            other => panic!("expected SwitchProvider, got {other:?}"),
        }
    }

    #[test]
    fn switch_to_fireworks_emits_action() {
        let mut app = create_test_app();
        let result = provider(&mut app, Some("fireworks pro"));
        match result.action {
            Some(AppAction::SwitchProvider { provider, model }) => {
                assert_eq!(provider, ApiProvider::Fireworks);
                assert_eq!(model.as_deref(), Some("deepseek-v4-pro"));
            }
            other => panic!("expected SwitchProvider, got {other:?}"),
        }
    }

    #[test]
    fn switch_to_sglang_flash_emits_action() {
        let mut app = create_test_app();
        let result = provider(&mut app, Some("sglang flash"));
        match result.action {
            Some(AppAction::SwitchProvider { provider, model }) => {
                assert_eq!(provider, ApiProvider::Sglang);
                assert_eq!(model.as_deref(), Some("deepseek-v4-flash"));
            }
            other => panic!("expected SwitchProvider, got {other:?}"),
        }
    }

    #[test]
    fn switch_to_vllm_flash_emits_action() {
        let mut app = create_test_app();
        let result = provider(&mut app, Some("vllm flash"));
        match result.action {
            Some(AppAction::SwitchProvider { provider, model }) => {
                assert_eq!(provider, ApiProvider::Vllm);
                assert_eq!(model.as_deref(), Some("deepseek-v4-flash"));
            }
            other => panic!("expected SwitchProvider, got {other:?}"),
        }
    }

    #[test]
    fn switch_to_ollama_preserves_model_tag() {
        let mut app = create_test_app();
        let result = provider(&mut app, Some("ollama qwen2.5-coder:7b"));
        match result.action {
            Some(AppAction::SwitchProvider { provider, model }) => {
                assert_eq!(provider, ApiProvider::Ollama);
                assert_eq!(model.as_deref(), Some("qwen2.5-coder:7b"));
            }
            other => panic!("expected SwitchProvider, got {other:?}"),
        }
    }

    #[test]
    fn switching_to_active_provider_without_model_is_a_noop() {
        let mut app = create_test_app();
        let result = provider(&mut app, Some("deepseek"));
        let msg = result.message.expect("expected message");
        assert!(msg.contains("Already on provider"));
        assert!(result.action.is_none());
    }

    #[test]
    fn switch_to_nim_emits_action_without_model_override() {
        let mut app = create_test_app();
        let result = provider(&mut app, Some("nvidia-nim"));
        assert!(result.message.is_none());
        match result.action {
            Some(AppAction::SwitchProvider { provider, model }) => {
                assert_eq!(provider, ApiProvider::NvidiaNim);
                assert_eq!(model, None);
            }
            other => panic!("expected SwitchProvider action, got {other:?}"),
        }
    }

    #[test]
    fn nim_flash_shorthand_emits_action_with_model_override() {
        let mut app = create_test_app();
        let result = provider(&mut app, Some("nim flash"));
        match result.action {
            Some(AppAction::SwitchProvider { provider, model }) => {
                assert_eq!(provider, ApiProvider::NvidiaNim);
                assert_eq!(model.as_deref(), Some("deepseek-v4-flash"));
            }
            other => panic!("expected SwitchProvider action, got {other:?}"),
        }
    }

    #[test]
    fn nim_pro_shorthand_emits_action_with_model_override() {
        let mut app = create_test_app();
        let result = provider(&mut app, Some("nim pro"));
        match result.action {
            Some(AppAction::SwitchProvider { provider, model }) => {
                assert_eq!(provider, ApiProvider::NvidiaNim);
                assert_eq!(model.as_deref(), Some("deepseek-v4-pro"));
            }
            other => panic!("expected SwitchProvider action, got {other:?}"),
        }
    }

    #[test]
    fn switch_to_active_provider_with_new_model_still_emits_action() {
        let mut app = create_test_app();
        let result = provider(&mut app, Some("deepseek flash"));
        match result.action {
            Some(AppAction::SwitchProvider { provider, model }) => {
                assert_eq!(provider, ApiProvider::Deepseek);
                assert_eq!(model.as_deref(), Some("deepseek-v4-flash"));
            }
            other => panic!("expected SwitchProvider action, got {other:?}"),
        }
    }

    #[test]
    fn invalid_model_returns_error() {
        let mut app = create_test_app();
        let result = provider(&mut app, Some("nim gpt-4"));
        let msg = result.message.expect("expected error message");
        assert!(msg.contains("Invalid model"));
        assert!(result.action.is_none());
    }
}
