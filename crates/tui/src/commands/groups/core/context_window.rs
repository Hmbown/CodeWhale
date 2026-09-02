//! Per-model context-window configuration.

use crate::commands::{CommandResult, traits::CommandInfo};
use crate::config::{
    Config, ProviderIdentity, clear_model_context_window_for_identity,
    save_model_context_window_for_identity,
};
use crate::localization::{MessageId, tr};
use crate::route_runtime::ContextWindowSource;
use crate::tui::app::App;

pub const CONTEXT_WINDOW_INFO: CommandInfo = CommandInfo {
    name: "context-window",
    aliases: &["context_window", "ctx"],
    usage: "/context-window [<tokens>|clear]",
    description_id: MessageId::CmdContextWindowDescription,
};

pub fn context_window(app: &mut App, args: Option<&str>) -> CommandResult {
    let model = app.effective_model_for_budget().to_string();
    let argument = args.unwrap_or("").trim();
    if argument.is_empty() {
        return CommandResult::message(
            tr(app.ui_locale, MessageId::ContextWindowCurrent)
                .replace("{model}", &model)
                .replace(
                    "{tokens}",
                    &crate::tui::model_picker::format_picker_context_window(u64::from(
                        crate::route_budget::route_context_window_tokens(
                            app.api_provider,
                            &model,
                            app.active_route_limits,
                        ),
                    )),
                )
                .replace(
                    "{source}",
                    &app.active_context_window_source.display_label(),
                ),
        );
    }

    if app.auto_model {
        return CommandResult::error(tr(
            app.ui_locale,
            MessageId::ContextWindowNeedsConcreteModel,
        ));
    }

    let identity = match persistence_identity(app) {
        Ok(identity) => identity,
        Err(error) => return CommandResult::error(error),
    };
    if argument.eq_ignore_ascii_case("clear") {
        if let Err(error) = clear_model_context_window_for_identity(&identity, &model) {
            return CommandResult::error(error.to_string());
        }
        let fallback = Config::load(app.config_path.clone(), app.config_profile.as_deref())
            .ok()
            .and_then(|config| config.context_window_for_provider_config(app.api_provider));
        apply_live_context_window(app, fallback);
        return CommandResult::message(
            tr(app.ui_locale, MessageId::ContextWindowCleared).replace("{model}", &model),
        );
    }

    let Some(context_window) = parse_context_window(argument) else {
        return CommandResult::error(
            tr(app.ui_locale, MessageId::ContextWindowInvalid).replace("{value}", argument),
        );
    };
    let path = match save_model_context_window_for_identity(&identity, &model, context_window) {
        Ok(path) => path,
        Err(error) => return CommandResult::error(error.to_string()),
    };
    apply_live_context_window(app, Some(context_window));
    CommandResult::message(
        tr(app.ui_locale, MessageId::ContextWindowSaved)
            .replace("{model}", &model)
            .replace(
                "{tokens}",
                &crate::tui::model_picker::format_picker_context_window(u64::from(context_window)),
            )
            .replace("{path}", &path.display().to_string()),
    )
}

fn parse_context_window(value: &str) -> Option<u32> {
    let (digits, multiplier) = match value.chars().last()? {
        'k' | 'K' => (&value[..value.len() - 1], 1_000_u64),
        'm' | 'M' => (&value[..value.len() - 1], 1_000_000_u64),
        _ => (value, 1),
    };
    let parsed = digits.parse::<u64>().ok()?;
    let tokens = parsed.checked_mul(multiplier)?;
    u32::try_from(tokens).ok().filter(|tokens| *tokens > 0)
}

fn persistence_identity(app: &App) -> Result<ProviderIdentity, String> {
    let config = Config::load(app.config_path.clone(), app.config_profile.as_deref())
        .map_err(|error| error.to_string())?;
    Ok(config
        .resolve_persisted_provider_identity(
            Some(app.api_provider.as_str()),
            app.provider_id_for_persistence(),
        )
        .unwrap_or_else(|_| ProviderIdentity {
            provider: app.api_provider,
            key: app.provider_identity_for_persistence().to_string(),
            exact_id: app.provider_id_for_persistence().map(str::to_string),
            migrated_legacy_ollama_cloud_route: false,
        }))
}

fn apply_live_context_window(app: &mut App, context_window: Option<u32>) {
    app.set_active_context_window_override(context_window);
    let mut limits = app.active_route_limits.unwrap_or_default();
    limits.context_tokens = context_window.map(u64::from);
    app.active_route_limits = Some(limits);
    app.active_context_window_source = if context_window.is_some() {
        ContextWindowSource::Configured
    } else {
        ContextWindowSource::Fallback
    };
    app.update_model_compaction_budget();
}
