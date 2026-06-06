use crate::commands::CommandResult;
use crate::config::Config;
use crate::config_actions::set_config_value;
use crate::config_ui::{ConfigUiMode, parse_mode};
use crate::settings::Settings;
use crate::tui::app::App;
use crate::tui::app::AppAction;

pub fn config_command(app: &mut App, arg: Option<&str>) -> CommandResult {
    let raw = arg.map(str::trim).unwrap_or("");
    if raw.is_empty() {
        return show_config(app, None);
    }
    let parts: Vec<&str> = raw.splitn(2, ' ').collect();
    if parts.len() == 1 {
        // Single arg: editor-mode shortcut OR show-value request.
        let token = parts[0];
        if matches!(
            token.to_ascii_lowercase().as_str(),
            "tui" | "web" | "native"
        ) {
            return show_config(app, Some(token));
        }
        // `/config <key>` — show current value
        show_single_setting(app, token)
    } else {
        // `/config <key> <value> [--save|-s]` — set value, optionally persist
        let raw_value = parts[1];
        let persist = raw_value.ends_with(" --save") || raw_value.ends_with(" -s");
        let value = if persist {
            raw_value
                .strip_suffix(" --save")
                .or_else(|| raw_value.strip_suffix(" -s"))
                .unwrap_or(raw_value)
        } else {
            raw_value
        };
        set_config_value(app, parts[0], value, persist)
    }
}

/// Open the interactive config editor.
///
/// Bare `/config` opens the legacy Native modal (the `OpenConfigView` action),
/// preserving the v0.8.4 behaviour. `/config tui` opens the schemaui-driven TUI
/// editor; `/config web` launches the web editor when the build enables it.
fn show_config(_app: &mut App, arg: Option<&str>) -> CommandResult {
    let mode = match parse_mode(arg) {
        Ok(mode) => mode,
        Err(err) => return CommandResult::error(err),
    };
    if mode == ConfigUiMode::Web && !cfg!(feature = "web") {
        return CommandResult::error(
            "This build does not include the web config UI. Rebuild with the `web` feature.",
        );
    }
    let action = match mode {
        ConfigUiMode::Native => AppAction::OpenConfigView,
        ConfigUiMode::Tui | ConfigUiMode::Web => AppAction::OpenConfigEditor(mode),
    };
    CommandResult::action(action)
}

/// Show the current value of a single setting.
fn show_single_setting(app: &App, key: &str) -> CommandResult {
    let key = key.to_lowercase();
    fn locale_display(l: crate::localization::Locale) -> &'static str {
        match l {
            crate::localization::Locale::En => "en",
            crate::localization::Locale::ZhHans => "zh-Hans",
            crate::localization::Locale::ZhHant => "zh-Hant",
            crate::localization::Locale::Ja => "ja",
            crate::localization::Locale::PtBr => "pt-BR",
            crate::localization::Locale::Es419 => "es-419",
            crate::localization::Locale::Vi => "vi",
        }
    }
    fn density_display(d: crate::tui::app::ComposerDensity) -> &'static str {
        match d {
            crate::tui::app::ComposerDensity::Compact => "compact",
            crate::tui::app::ComposerDensity::Comfortable => "comfortable",
            crate::tui::app::ComposerDensity::Spacious => "spacious",
        }
    }
    fn spacing_display(s: crate::tui::app::TranscriptSpacing) -> &'static str {
        match s {
            crate::tui::app::TranscriptSpacing::Compact => "compact",
            crate::tui::app::TranscriptSpacing::Comfortable => "comfortable",
            crate::tui::app::TranscriptSpacing::Spacious => "spacious",
        }
    }
    let value = match key.as_str() {
        "model" => {
            if app.auto_model {
                let mut label = "auto (auto-select model per turn)".to_string();
                if let Some(effective) = app.last_effective_model.as_deref()
                    && effective != "auto"
                {
                    label.push_str(&format!("; last: {effective}"));
                }
                Some(label)
            } else {
                Some(app.model.clone())
            }
        }
        "provider" => Some(app.api_provider.as_str().to_string()),
        "approval_mode" | "approval" => Some(app.approval_mode.label().to_string()),
        "allow_shell" | "shell" | "exec_shell" => Some(app.allow_shell.to_string()),
        "base_url" => {
            let config = match Config::load(app.config_path.clone(), app.config_profile.as_deref())
            {
                Ok(config) => config,
                Err(err) => {
                    return CommandResult::error(format!("Failed to load config: {err}"));
                }
            };
            Some(config.deepseek_base_url())
        }
        "provider_url" | "provider_base_url" | "endpoint" => {
            let config = match Config::load(app.config_path.clone(), app.config_profile.as_deref())
            {
                Ok(mut config) => {
                    config.provider = Some(app.api_provider.as_str().to_string());
                    config
                }
                Err(err) => {
                    return CommandResult::error(format!("Failed to load config: {err}"));
                }
            };
            Some(config.deepseek_base_url())
        }
        "locale" | "language" => Some(locale_display(app.ui_locale).to_string()),
        "theme" | "ui_theme" => {
            Some(crate::palette::theme_label_for_mode(app.ui_theme.mode).to_string())
        }
        "background_color" | "background" | "bg" => {
            crate::palette::hex_rgb_string(app.ui_theme.surface_bg)
                .or_else(|| Some("(default)".to_string()))
        }
        "auto_compact" | "compact" => {
            Some(if app.auto_compact { "true" } else { "false" }.to_string())
        }
        "calm_mode" | "calm" => Some(if app.calm_mode { "true" } else { "false" }.to_string()),
        "low_motion" | "motion" => Some(if app.low_motion { "true" } else { "false" }.to_string()),
        "fancy_animations" | "fancy" | "animations" => Some(
            if app.fancy_animations {
                "true"
            } else {
                "false"
            }
            .to_string(),
        ),
        "bracketed_paste" | "paste" => Some(
            if app.use_bracketed_paste {
                "true"
            } else {
                "false"
            }
            .to_string(),
        ),
        "paste_burst_detection" | "paste_burst" => Some(
            if app.use_paste_burst_detection {
                "true"
            } else {
                "false"
            }
            .to_string(),
        ),
        "show_thinking" | "thinking" => {
            Some(if app.show_thinking { "true" } else { "false" }.to_string())
        }
        "show_tool_details" | "tool_details" => Some(
            if app.show_tool_details {
                "true"
            } else {
                "false"
            }
            .to_string(),
        ),
        "mode" | "default_mode" => Some(app.mode.as_setting().to_string()),
        "max_history" | "history" => Some(app.max_input_history.to_string()),
        "sidebar_width" | "sidebar" => Some(app.sidebar_width_percent.to_string()),
        "sidebar_focus" | "focus" => Some(app.sidebar_focus.as_setting().to_string()),
        "context_panel" | "context" | "session_panel" => {
            Some(if app.context_panel { "true" } else { "false" }.to_string())
        }
        "composer_density" | "composer" => Some(density_display(app.composer_density).to_string()),
        "composer_border" | "border" => {
            Some(if app.composer_border { "true" } else { "false" }.to_string())
        }
        "composer_vim_mode" | "vim_mode" | "vim" => Some(
            if app.composer.vim_enabled {
                "vim"
            } else {
                "normal"
            }
            .to_string(),
        ),
        "transcript_spacing" | "spacing" => {
            Some(spacing_display(app.transcript_spacing).to_string())
        }
        "status_indicator" | "indicator" => Some(app.status_indicator.clone()),
        "synchronized_output" | "sync_output" | "sync" => Some(
            if app.synchronized_output_enabled {
                "on"
            } else {
                "off"
            }
            .to_string(),
        ),
        "cost_currency" | "currency" => Some(
            match app.cost_currency {
                crate::pricing::CostCurrency::Usd => "usd",
                crate::pricing::CostCurrency::Cny => "cny",
            }
            .to_string(),
        ),
        "default_model" => Settings::load().ok().map(|settings| {
            settings
                .default_model
                .unwrap_or_else(|| "(default)".to_string())
        }),
        "reasoning_effort" | "effort" => Some(app.reasoning_effort.as_setting().to_string()),
        "prefer_external_pdftotext" | "external_pdftotext" | "pdftotext" => Settings::load()
            .ok()
            .map(|settings| settings.prefer_external_pdftotext.to_string()),
        _ => {
            let known = Settings::available_settings()
                .iter()
                .any(|(k, _)| k == &key);
            if known {
                Some("(see /settings for current value)".to_string())
            } else {
                None
            }
        }
    };
    match value {
        Some(v) => CommandResult::message(format!("{key} = {v}")),
        None => CommandResult::error(format!(
            "Unknown setting '{key}'. See `/help config` for available settings."
        )),
    }
}
