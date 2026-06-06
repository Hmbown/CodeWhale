use crate::commands::CommandResult;
use crate::config::{
    ApiProvider, COMMON_DEEPSEEK_MODELS, normalize_custom_model_id,
    normalize_model_name_for_provider, provider_passes_model_through,
};
use crate::localization::{MessageId, tr};
use crate::tui::app::{App, AppAction, ReasoningEffort};

pub(crate) fn model(app: &mut App, model_name: Option<&str>) -> CommandResult {
    if let Some(name) = model_name {
        if name.trim().eq_ignore_ascii_case("auto") {
            return switch_to_auto_model(app);
        }

        let model_id = if app.accepts_custom_model_ids() {
            let Some(model_id) = normalize_custom_model_id(name) else {
                return CommandResult::error(format!(
                    "Invalid model '{name}'. Expected a non-empty model ID."
                ));
            };
            model_id
        } else {
            let Some(model_id) = normalize_model_name_for_provider(app.api_provider, name) else {
                if let Some((provider, model_id)) = saved_provider_model_match(app, name) {
                    return CommandResult::with_message_and_action(
                        format!(
                            "Switching provider to {} for model {model_id}.",
                            provider.as_str()
                        ),
                        AppAction::SwitchProvider {
                            provider,
                            model: Some(model_id),
                        },
                    );
                }
                return CommandResult::error(format!(
                    "Invalid model '{name}'. Expected auto, a model for the active provider, or a saved provider model. Common DeepSeek models: {}",
                    COMMON_DEEPSEEK_MODELS.join(", ")
                ));
            };
            model_id
        };

        switch_to_model(app, model_id)
    } else {
        CommandResult::action(AppAction::OpenModelPicker)
    }
}

fn switch_to_auto_model(app: &mut App) -> CommandResult {
    let old_model = app.model_display_label();
    let model_changed = !app.auto_model || app.model != "auto";
    app.auto_model = true;
    app.model = "auto".to_string();
    app.last_effective_model = None;
    app.reasoning_effort = ReasoningEffort::Auto;
    app.last_effective_reasoning_effort = None;
    app.update_model_compaction_budget();
    if model_changed {
        app.clear_model_scoped_telemetry();
    } else {
        app.session.last_prompt_tokens = None;
        app.session.last_completion_tokens = None;
    }
    app.provider_models
        .insert(app.api_provider.as_str().to_string(), "auto".to_string());
    let persist_warning = provider_model_selection_persist_warning(app.api_provider, "auto");
    let mut message = tr(app.ui_locale, MessageId::ModelChanged)
        .replace("{old}", &old_model)
        .replace("{new}", "auto");
    if let Some(warning) = persist_warning {
        message.push_str(&warning);
    }
    CommandResult::with_message_and_action(
        message,
        AppAction::UpdateCompaction(app.compaction_config()),
    )
}

fn switch_to_model(app: &mut App, model_id: String) -> CommandResult {
    let old_model = app.model_display_label();
    let model_changed = app.auto_model || app.model != model_id;
    app.auto_model = false;
    app.model = model_id.clone();
    app.last_effective_model = None;
    app.update_model_compaction_budget();
    if model_changed {
        app.clear_model_scoped_telemetry();
    } else {
        app.session.last_prompt_tokens = None;
        app.session.last_completion_tokens = None;
    }
    app.provider_models
        .insert(app.api_provider.as_str().to_string(), model_id.clone());
    let persist_warning = provider_model_selection_persist_warning(app.api_provider, &model_id);
    let mut message = tr(app.ui_locale, MessageId::ModelChanged)
        .replace("{old}", &old_model)
        .replace("{new}", &model_id);
    if let Some(warning) = persist_warning {
        message.push_str(&warning);
    }
    CommandResult::with_message_and_action(
        message,
        AppAction::UpdateCompaction(app.compaction_config()),
    )
}

fn provider_model_selection_persist_warning(provider: ApiProvider, model: &str) -> Option<String> {
    crate::settings::Settings::persist_provider_model_selection(provider, model)
        .err()
        .map(|err| format!(" (not persisted: {err})"))
}

fn saved_provider_model_match(app: &App, name: &str) -> Option<(ApiProvider, String)> {
    let requested = normalize_custom_model_id(name)?;
    let mut saved = app
        .provider_models
        .iter()
        .filter_map(|(provider_name, model)| {
            let provider = ApiProvider::parse(provider_name)?;
            (provider != app.api_provider).then_some((provider, model.as_str()))
        })
        .collect::<Vec<_>>();
    saved.sort_by_key(|(provider, _)| provider.as_str());

    for (provider, saved_model) in saved {
        let Some(saved_model) = normalize_model_for_provider_selection(provider, saved_model)
        else {
            continue;
        };
        let requested_model = normalize_model_for_provider_selection(provider, &requested)
            .unwrap_or_else(|| requested.clone());
        if saved_model.eq_ignore_ascii_case(&requested_model)
            || saved_model.eq_ignore_ascii_case(&requested)
        {
            return Some((provider, saved_model));
        }
    }

    None
}

fn normalize_model_for_provider_selection(provider: ApiProvider, model: &str) -> Option<String> {
    if provider_passes_model_through(provider) {
        normalize_custom_model_id(model)
    } else {
        normalize_model_name_for_provider(provider, model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::groups::core::test_support::{SettingsPathGuard, create_test_app};
    use crate::tui::app::TurnCacheRecord;
    use std::time::Instant;

    #[test]
    fn test_model_change_updates_state() {
        let _settings = SettingsPathGuard::new();
        let mut app = create_test_app();
        let old_model = app.model.clone();

        let result = model(&mut app, Some("deepseek-v4-flash"));

        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains(&old_model));
        assert!(msg.contains("deepseek-v4-flash"));
        assert!(matches!(
            result.action,
            Some(AppAction::UpdateCompaction(_))
        ));
        assert_eq!(app.model, "deepseek-v4-flash");
        assert_eq!(app.session.last_prompt_tokens, None);
        assert_eq!(app.session.last_completion_tokens, None);
    }

    #[test]
    fn model_command_persists_active_provider_model() {
        let _settings = SettingsPathGuard::new();
        let mut app = create_test_app();

        let result = model(&mut app, Some("deepseek-v4-flash"));

        assert!(result.message.is_some());
        assert_eq!(
            app.provider_models.get("deepseek").map(String::as_str),
            Some("deepseek-v4-flash")
        );
        let settings = crate::settings::Settings::load().expect("load settings");
        assert_eq!(settings.default_provider.as_deref(), Some("deepseek"));
        assert_eq!(settings.default_model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(
            settings
                .provider_models
                .as_ref()
                .and_then(|models| models.get("deepseek"))
                .map(String::as_str),
            Some("deepseek-v4-flash")
        );
    }

    #[test]
    fn model_switch_clears_turn_cache_history() {
        let _settings = SettingsPathGuard::new();
        let mut app = create_test_app();
        app.auto_model = false;
        app.model = "deepseek-v4-pro".to_string();
        app.push_turn_cache_record(TurnCacheRecord {
            input_tokens: 100,
            output_tokens: 25,
            cache_hit_tokens: Some(70),
            cache_miss_tokens: Some(30),
            reasoning_replay_tokens: Some(12),
            recorded_at: Instant::now(),
        });

        let result = model(&mut app, Some("deepseek-v4-flash"));

        assert!(result.message.is_some());
        assert!(app.session.turn_cache_history.is_empty());
    }

    #[test]
    fn model_reset_same_model_keeps_turn_cache_history() {
        let _settings = SettingsPathGuard::new();
        let mut app = create_test_app();
        app.auto_model = false;
        app.model = "deepseek-v4-pro".to_string();
        app.push_turn_cache_record(TurnCacheRecord {
            input_tokens: 100,
            output_tokens: 25,
            cache_hit_tokens: Some(70),
            cache_miss_tokens: Some(30),
            reasoning_replay_tokens: Some(12),
            recorded_at: Instant::now(),
        });

        let result = model(&mut app, Some("deepseek-v4-pro"));

        assert!(result.message.is_some());
        assert_eq!(app.session.turn_cache_history.len(), 1);
    }

    #[test]
    fn test_model_auto_enables_auto_thinking() {
        let _settings = SettingsPathGuard::new();
        let mut app = create_test_app();
        app.reasoning_effort = ReasoningEffort::Off;

        let result = model(&mut app, Some("auto"));

        assert!(result.message.is_some());
        assert!(app.auto_model);
        assert_eq!(app.model, "auto");
        assert_eq!(app.reasoning_effort, ReasoningEffort::Auto);
        assert!(app.last_effective_model.is_none());
        assert!(app.last_effective_reasoning_effort.is_none());
    }

    #[test]
    fn test_model_change_accepts_future_deepseek_model() {
        let _settings = SettingsPathGuard::new();
        let mut app = create_test_app();

        let result = model(&mut app, Some("deepseek-v4"));

        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("deepseek-v4"));
        assert_eq!(app.model, "deepseek-v4");
        assert!(matches!(
            result.action,
            Some(AppAction::UpdateCompaction(_))
        ));
    }

    #[test]
    fn test_model_change_accepts_custom_id_for_openai_compatible_provider() {
        let _settings = SettingsPathGuard::new();
        let mut app = create_test_app();
        app.api_provider = crate::config::ApiProvider::Openai;
        app.model_ids_passthrough = true;

        let result = model(&mut app, Some("opencode-go/glm-5.1"));

        assert!(result.message.is_some());
        assert_eq!(app.model, "opencode-go/glm-5.1");
        assert!(!app.auto_model);
        assert!(matches!(
            result.action,
            Some(AppAction::UpdateCompaction(_))
        ));
    }

    #[test]
    fn test_model_change_accepts_custom_id_for_custom_base_url() {
        let _settings = SettingsPathGuard::new();
        let mut app = create_test_app();
        app.model_ids_passthrough = true;

        let result = model(&mut app, Some("opencode-go/kimi-k2.6"));

        assert!(result.message.is_some());
        assert_eq!(app.model, "opencode-go/kimi-k2.6");
        assert!(matches!(
            result.action,
            Some(AppAction::UpdateCompaction(_))
        ));
    }

    #[test]
    fn test_model_change_rejects_invalid_model() {
        let mut app = create_test_app();

        let result = model(&mut app, Some("gpt-4"));

        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Invalid model"));
        assert!(msg.contains("active provider"));
        assert!(msg.contains("deepseek-v4-pro"));
        assert!(msg.contains("deepseek-v4-flash"));
        assert!(result.action.is_none());
    }

    #[test]
    fn model_command_switches_to_saved_provider_model() {
        let mut app = create_test_app();
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app.provider_models
            .insert("moonshot".to_string(), "kimi-k2.6".to_string());

        let result = model(&mut app, Some("kimi-k2.6"));

        match result.action {
            Some(AppAction::SwitchProvider { provider, model }) => {
                assert_eq!(provider, crate::config::ApiProvider::Moonshot);
                assert_eq!(model.as_deref(), Some("kimi-k2.6"));
            }
            other => panic!("expected SwitchProvider action, got {other:?}"),
        }
        assert_eq!(app.api_provider, crate::config::ApiProvider::Deepseek);
        assert_eq!(app.model, "deepseek-v4-pro");
    }

    #[test]
    fn test_model_without_args_opens_picker() {
        let mut app = create_test_app();

        let result = model(&mut app, None);

        assert_eq!(result.message, None);
        assert_eq!(result.action, Some(AppAction::OpenModelPicker));
    }
}
