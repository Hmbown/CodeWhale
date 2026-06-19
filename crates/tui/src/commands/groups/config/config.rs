//! Config commands: config, settings, mode switches, trust, logout

use super::CommandResult;
use crate::config::{
    ApiProvider, COMMON_DEEPSEEK_MODELS, Config, DEFAULT_STREAM_CHUNK_TIMEOUT_SECS,
    DEFAULT_XIAOMI_MIMO_BASE_URL, MAX_STREAM_CHUNK_TIMEOUT_SECS, MIN_STREAM_CHUNK_TIMEOUT_SECS,
    XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL, clear_active_provider_api_key,
    normalize_model_name_for_provider,
};
use crate::config_persistence::{
    persist_feature_bool_key, persist_provider_base_url_key, persist_root_bool_key,
    persist_root_string_key, persist_tui_integer_key,
};
use crate::config_ui::{ConfigUiMode, parse_mode};
use crate::features::Feature;
use crate::localization::resolve_locale;
use crate::settings::Settings;
use crate::tui::app::{
    App, AppAction, AppMode, OnboardingState, ReasoningEffort, SidebarFocus, VimMode,
};
use crate::tui::approval::ApprovalMode;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Open the interactive config editor.
///
/// Bare `/config` opens the legacy Native modal (the `OpenConfigView` action),
/// preserving the v0.8.4 behaviour. `/config tui` opens the new
/// schemaui-driven TUI editor; `/config web` launches the web editor (only
/// available in builds compiled with the `web` feature).
pub fn show_config(_app: &mut App, arg: Option<&str>) -> CommandResult {
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

/// Dispatch `/config` with optional args.
///
/// - `/config` (no args) — opens the schemaui-driven TUI editor.
/// - `/config tui` / `/config web` / `/config native` — open a specific
///   editor mode (web requires the `web` build feature).
/// - `/config <key>` — shows the current value of a setting.
/// - `/config <key> <value>` — sets a runtime value (session only, add --save to persist).
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
        key if is_subagents_config_key(key) => {
            return CommandResult::message(subagents_status_message(
                app.features.enabled(Feature::Subagents),
            ));
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
        "stream_chunk_timeout_secs" => Some(app.stream_chunk_timeout_secs.to_string()),
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
        "tool_collapse" | "tool_collapse_mode" | "collapse" => {
            Some(app.tool_collapse_mode.as_setting().to_string())
        }
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
        "reasoning_effort" | "effort" => Some(
            app.reasoning_effort
                .as_setting_for_provider(app.api_provider)
                .to_string(),
        ),
        "prefer_external_pdftotext" | "external_pdftotext" | "pdftotext" => Settings::load()
            .ok()
            .map(|settings| settings.prefer_external_pdftotext.to_string()),
        "workspace_follow_symlinks" | "follow_symlinks" => Settings::load().ok().map(|settings| {
            format!(
                "{} (restart required for engine tools)",
                settings.workspace_follow_symlinks
            )
        }),
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

/// Show persistent settings
pub fn show_settings(app: &mut App) -> CommandResult {
    match Settings::load() {
        Ok(settings) => CommandResult::message(settings.display(app.ui_locale)),
        Err(e) => CommandResult::error(format!("Failed to load settings: {e}")),
    }
}

/// Open the `/statusline` multi-select picker for configuring footer items.
pub fn status_line(_app: &mut App) -> CommandResult {
    CommandResult::action(AppAction::OpenStatusPicker)
}

/// Toggle whether the live transcript renders full thinking detail.
pub fn verbose(app: &mut App, arg: Option<&str>) -> CommandResult {
    let next = match arg.map(str::trim).filter(|s| !s.is_empty()) {
        None => !app.verbose_transcript,
        Some(raw) => match raw.to_ascii_lowercase().as_str() {
            "on" | "true" | "1" | "yes" => true,
            "off" | "false" | "0" | "no" => false,
            "toggle" => !app.verbose_transcript,
            _ => {
                return CommandResult::error(
                    "Usage: /verbose [on|off]. Compact thinking remains available when verbose is off.",
                );
            }
        },
    };

    app.verbose_transcript = next;
    app.mark_history_updated();
    CommandResult::message(if next {
        "Verbose transcript on: live thinking renders in full."
    } else {
        "Verbose transcript off: live thinking stays compact."
    })
}

/// Toggle or focus the right sidebar.
///
/// Bare `/sidebar` toggles between hidden and auto. Explicit values mirror
/// `sidebar_focus` so users have a discoverable copy-friendly path that does
/// not depend on terminal-specific key translations.
pub fn sidebar(app: &mut App, arg: Option<&str>) -> CommandResult {
    let raw = arg.map(str::trim).unwrap_or("");
    let mut tokens = raw.split_whitespace().collect::<Vec<_>>();
    let persist = matches!(tokens.last(), Some(&"--save" | &"-s"));
    if persist {
        tokens.pop();
    }

    let target = match tokens.as_slice() {
        [] | ["toggle"] => {
            if app.sidebar_focus == SidebarFocus::Hidden {
                SidebarFocus::Auto
            } else {
                SidebarFocus::Hidden
            }
        }
        [value] => match value.to_ascii_lowercase().as_str() {
            "on" | "show" | "visible" => SidebarFocus::Auto,
            "off" | "hide" | "hidden" | "closed" | "none" => SidebarFocus::Hidden,
            "auto" => SidebarFocus::Auto,
            "work" | "plan" | "todos" => SidebarFocus::Work,
            "tasks" => SidebarFocus::Tasks,
            "agents" | "subagents" | "sub-agents" => SidebarFocus::Agents,
            "context" | "session" => SidebarFocus::Context,
            _ => {
                return CommandResult::error(
                    "Usage: /sidebar [on|off|auto|work|tasks|agents|context] [--save]",
                );
            }
        },
        _ => {
            return CommandResult::error(
                "Usage: /sidebar [on|off|auto|work|tasks|agents|context] [--save]",
            );
        }
    };

    if persist {
        let result = set_config_value(app, "sidebar_focus", target.as_setting(), true);
        if result.is_error {
            return result;
        }
    } else {
        app.set_sidebar_focus(target);
    }

    app.needs_redraw = true;
    let message = sidebar_status_message(target).to_string();
    CommandResult::message(message)
}

fn sidebar_status_message(focus: SidebarFocus) -> &'static str {
    if focus == SidebarFocus::Hidden {
        "Sidebar is hidden"
    } else {
        "Sidebar is visible"
    }
}

fn resolve_provider_url_value(provider: ApiProvider, value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("provider_url cannot be empty".to_string());
    }

    if provider == ApiProvider::XiaomiMimo {
        match trimmed.to_ascii_lowercase().as_str() {
            "token" | "token-plan" | "token_plan" | "token-plan-sgp" | "sgp" => {
                return Ok(DEFAULT_XIAOMI_MIMO_BASE_URL.to_string());
            }
            "payg" | "pay-go" | "paygo" | "pay-as-you-go" | "pay_as_you_go" | "api" => {
                return Ok(XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL.to_string());
            }
            _ => {}
        }
    }

    if trimmed.contains("://") {
        Ok(trimmed.to_string())
    } else if provider == ApiProvider::XiaomiMimo {
        Err("provider_url for Xiaomi MiMo must be token-plan, pay-as-you-go, or a URL".to_string())
    } else {
        Err("provider_url must be a URL".to_string())
    }
}

fn parse_config_bool(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" | "enabled" => Ok(true),
        "off" | "false" | "no" | "0" | "disabled" => Ok(false),
        _ => Err(format!(
            "Failed to parse boolean '{value}': expected on/off, true/false, yes/no."
        )),
    }
}

fn stream_chunk_timeout_value_label(raw: u64, resolved: u64) -> String {
    if raw == 0 {
        format!("0 (default {resolved})")
    } else {
        resolved.to_string()
    }
}

fn is_subagents_config_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "subagents"
            | "sub-agents"
            | "features.subagents"
            | "features.sub-agents"
            | "feature.subagents"
            | "feature.sub-agents"
    )
}

fn subagents_status_message(enabled: bool) -> String {
    let state = if enabled { "enabled" } else { "disabled" };
    format!("subagents = {enabled} ({state}; controls the model-facing agent tool)")
}

fn set_subagents_feature(app: &mut App, enabled: bool, persist: bool) -> CommandResult {
    let suffix = if persist {
        match persist_feature_bool_key(app.config_path.as_deref(), "subagents", enabled) {
            Ok(path) => format!(" (saved to {})", path.display()),
            Err(err) => return CommandResult::error(format!("Failed to save: {err}")),
        }
    } else {
        " (session only, add --save to persist)".to_string()
    };

    if enabled {
        app.features.enable(Feature::Subagents);
    } else {
        app.features.disable(Feature::Subagents);
    }

    let tool_hint = if enabled {
        " The agent tool will be available on the next turn."
    } else {
        " The agent tool will be hidden on the next turn."
    };

    CommandResult::with_message_and_action(
        format!("subagents = {enabled}{suffix}.{tool_hint}"),
        AppAction::UpdateFeatures(app.features.clone()),
    )
}

/// Modify a setting at runtime
pub fn set_config_value(app: &mut App, key: &str, value: &str, persist: bool) -> CommandResult {
    let key = key.to_lowercase();

    match key.as_str() {
        key if is_subagents_config_key(key) => {
            let value = value.trim();
            if value.is_empty()
                || value.eq_ignore_ascii_case("status")
                || value.eq_ignore_ascii_case("show")
                || value.eq_ignore_ascii_case("get")
            {
                return CommandResult::message(subagents_status_message(
                    app.features.enabled(Feature::Subagents),
                ));
            }
            let enabled = match parse_config_bool(value) {
                Ok(enabled) => enabled,
                Err(err) => return CommandResult::error(err),
            };
            return set_subagents_feature(app, enabled, persist);
        }
        "model" => {
            // Support "/model auto" — auto-select model based on request complexity
            if value.trim().eq_ignore_ascii_case("auto") {
                app.set_model_selection("auto".to_string());
                app.reasoning_effort = ReasoningEffort::Auto;
                app.last_effective_reasoning_effort = None;
                app.update_model_compaction_budget();
                app.session.last_prompt_tokens = None;
                app.session.last_completion_tokens = None;
                app.session.last_output_throughput = None;
                return CommandResult::with_message_and_action(
                    "model = auto (auto-select model and thinking per turn)".to_string(),
                    AppAction::UpdateCompaction(app.compaction_config()),
                );
            }
            // Clear auto mode when a specific model is set
            let Some(model) = normalize_model_name_for_provider(app.api_provider, value) else {
                return CommandResult::error(format!(
                    "Invalid model '{value}'. Expected a DeepSeek model ID. Common models: {}",
                    COMMON_DEEPSEEK_MODELS.join(", ")
                ));
            };
            app.set_model_selection(model.clone());
            app.update_model_compaction_budget();
            app.session.last_prompt_tokens = None;
            app.session.last_completion_tokens = None;
            app.session.last_output_throughput = None;
            return CommandResult::with_message_and_action(
                format!("model = {model}"),
                AppAction::UpdateCompaction(app.compaction_config()),
            );
        }
        "provider" => {
            let value = value.trim();
            let Some(provider) = ApiProvider::parse(value) else {
                return CommandResult::error(format!(
                    "Unknown provider '{value}'. Use: {}.",
                    ApiProvider::names_hint()
                ));
            };
            if provider == app.api_provider {
                return CommandResult::message(format!("provider = {}", provider.as_str()));
            }
            return CommandResult::with_message_and_action(
                format!("provider = {}", provider.as_str()),
                AppAction::SwitchProvider {
                    provider,
                    model: None,
                },
            );
        }
        "approval_mode" | "approval" => {
            let mode = ApprovalMode::from_config_value(value);
            return match mode {
                Some(m) => {
                    app.approval_mode = m;
                    CommandResult::message(format!("approval_mode = {}", m.label()))
                }
                None => CommandResult::error(
                    "Invalid approval_mode. Use: auto, suggest/on-request/untrusted, never/deny",
                ),
            };
        }
        "allow_shell" | "shell" | "exec_shell" => {
            let enabled = match parse_config_bool(value) {
                Ok(enabled) => enabled,
                Err(err) => return CommandResult::error(err),
            };
            app.allow_shell = enabled;
            let suffix = if persist {
                match persist_root_bool_key(app.config_path.as_deref(), "allow_shell", enabled) {
                    Ok(path) => format!(" (saved to {})", path.display()),
                    Err(err) => return CommandResult::error(format!("Failed to save: {err}")),
                }
            } else {
                " (session only, add --save to persist)".to_string()
            };
            let mode_hint = if enabled {
                " Agent mode will expose shell on the next turn with approval gating. YOLO also enables shell and auto-approves."
            } else {
                " Shell tools will be hidden on the next turn. Re-enable with `/config allow_shell true`."
            };
            return CommandResult::message(format!("allow_shell = {enabled}{suffix}.{mode_hint}"));
        }
        "mcp_config_path" | "mcp" => {
            if value.trim().is_empty() {
                return CommandResult::error("mcp_config_path cannot be empty");
            }
            app.mcp_config_path = PathBuf::from(expand_tilde(value));
            app.mcp_restart_required = true;
            let message = if persist {
                match persist_root_string_key(app.config_path.as_deref(), "mcp_config_path", value)
                {
                    Ok(path) => format!(
                        "mcp_config_path = {} (saved to {}; restart required for MCP tool pool)",
                        app.mcp_config_path.display(),
                        path.display()
                    ),
                    Err(err) => return CommandResult::error(format!("Failed to save: {err}")),
                }
            } else {
                format!(
                    "mcp_config_path = {} (session only; restart required for MCP tool pool)",
                    app.mcp_config_path.display()
                )
            };
            return CommandResult::message(message);
        }
        "base_url" => {
            let value = value.trim();
            if value.is_empty() {
                return CommandResult::error("base_url cannot be empty");
            }
            if persist {
                match persist_root_string_key(app.config_path.as_deref(), "base_url", value) {
                    Ok(path) => {
                        return CommandResult::message(format!(
                            "base_url = {value} (saved to {})",
                            path.display()
                        ));
                    }
                    Err(err) => return CommandResult::error(format!("Failed to save: {err}")),
                }
            }
            return CommandResult::error(
                "base_url must be saved with --save; client base URL is loaded from config on startup. Restart and re-open your session after saving.",
            );
        }
        "provider_url" | "provider_base_url" | "endpoint" => {
            let value = match resolve_provider_url_value(app.api_provider, value) {
                Ok(value) => value,
                Err(err) => return CommandResult::error(err),
            };
            if matches!(
                app.api_provider,
                ApiProvider::Deepseek | ApiProvider::DeepseekCN
            ) {
                if persist {
                    match persist_root_string_key(app.config_path.as_deref(), "base_url", &value) {
                        Ok(path) => {
                            return CommandResult::message(format!(
                                "provider_url = {value} (saved to {}; restart required)",
                                path.display()
                            ));
                        }
                        Err(err) => return CommandResult::error(format!("Failed to save: {err}")),
                    }
                }
            } else if persist {
                match persist_provider_base_url_key(
                    app.config_path.as_deref(),
                    app.api_provider,
                    &value,
                ) {
                    Ok(path) => {
                        return CommandResult::message(format!(
                            "provider_url = {value} for {} (saved to {}; restart required)",
                            app.api_provider.as_str(),
                            path.display()
                        ));
                    }
                    Err(err) => return CommandResult::error(format!("Failed to save: {err}")),
                }
            }
            return CommandResult::error(
                "provider_url must be saved with --save; client base URL is loaded from config on startup. Restart and re-open your session after saving.",
            );
        }
        "stream_chunk_timeout_secs" => {
            let raw = match value.trim().parse::<u64>() {
                Ok(value) => value,
                Err(_) => {
                    return CommandResult::error(
                        "stream_chunk_timeout_secs must be a whole number",
                    );
                }
            };
            if raw != 0
                && !(MIN_STREAM_CHUNK_TIMEOUT_SECS..=MAX_STREAM_CHUNK_TIMEOUT_SECS).contains(&raw)
            {
                return CommandResult::error(format!(
                    "stream_chunk_timeout_secs must be 0 or {}..={}",
                    MIN_STREAM_CHUNK_TIMEOUT_SECS, MAX_STREAM_CHUNK_TIMEOUT_SECS
                ));
            }
            let resolved = if raw == 0 {
                DEFAULT_STREAM_CHUNK_TIMEOUT_SECS
            } else {
                raw
            };
            app.stream_chunk_timeout_secs = resolved;
            let value_label = stream_chunk_timeout_value_label(raw, resolved);
            if persist {
                match persist_tui_integer_key(
                    app.config_path.as_deref(),
                    "stream_chunk_timeout_secs",
                    raw,
                ) {
                    Ok(path) => {
                        return CommandResult::with_message_and_action(
                            format!(
                                "stream_chunk_timeout_secs = {value_label} (saved to {}; affects subsequent turns in this session)",
                                path.display()
                            ),
                            AppAction::UpdateStreamChunkTimeout(resolved),
                        );
                    }
                    Err(err) => return CommandResult::error(format!("Failed to save: {err}")),
                }
            }
            return CommandResult::with_message_and_action(
                format!(
                    "stream_chunk_timeout_secs = {value_label} (session only; affects subsequent turns in this session)"
                ),
                AppAction::UpdateStreamChunkTimeout(resolved),
            );
        }
        _ => {}
    }

    let mut settings = match Settings::load() {
        Ok(s) => s,
        Err(e) if !persist => {
            app.status_message = Some(format!(
                "Settings unavailable; applying session-only override ({e})"
            ));
            Settings::default()
        }
        Err(e) => return CommandResult::error(format!("Failed to load settings: {e}")),
    };

    if let Err(e) = settings.set(&key, value) {
        return CommandResult::error(format!("{e}"));
    }
    settings.apply_env_overrides();

    let mut action = None;
    match key.as_str() {
        "auto_compact" | "compact" => {
            app.auto_compact = settings.auto_compact;
            app.auto_compact_user_configured = true;
            action = Some(AppAction::UpdateCompaction(app.compaction_config()));
        }
        "calm_mode" | "calm" => {
            app.calm_mode = settings.calm_mode;
            app.mark_history_updated();
        }
        "low_motion" | "motion" => {
            app.low_motion = settings.low_motion;
            app.needs_redraw = true;
        }
        "fancy_animations" | "fancy" | "animations" => {
            app.fancy_animations = settings.fancy_animations;
            app.needs_redraw = true;
        }
        "bracketed_paste" | "paste" => {
            app.use_bracketed_paste = settings.bracketed_paste;
            app.needs_redraw = true;
        }
        "status_indicator" | "indicator" => {
            app.status_indicator = settings.status_indicator.clone();
            app.needs_redraw = true;
        }
        "synchronized_output" | "sync_output" | "sync" => {
            app.synchronized_output_enabled = settings.synchronized_output_enabled();
            app.needs_redraw = true;
        }
        "show_thinking" | "thinking" => {
            app.show_thinking = settings.show_thinking;
            app.mark_history_updated();
        }
        "show_tool_details" | "tool_details" => {
            app.show_tool_details = settings.show_tool_details;
            app.mark_history_updated();
        }
        "locale" | "language" => {
            app.ui_locale = resolve_locale(&settings.locale);
            app.mark_history_updated();
            app.needs_redraw = true;
        }
        "theme" | "ui_theme" | "background_color" | "background" | "bg" => {
            app.theme_id = crate::palette::ThemeId::from_name(&settings.theme)
                .unwrap_or(crate::palette::ThemeId::System);
            app.ui_theme = crate::palette::ui_theme_from_settings(
                &settings.theme,
                settings.background_color.as_deref(),
            );
            app.needs_redraw = true;
        }
        "cost_currency" | "currency" => {
            app.cost_currency = crate::pricing::CostCurrency::from_setting(&settings.cost_currency)
                .unwrap_or(crate::pricing::CostCurrency::Usd);
            app.needs_redraw = true;
        }
        "composer_density" | "composer" => {
            app.composer_density =
                crate::tui::app::ComposerDensity::from_setting(&settings.composer_density);
            app.needs_redraw = true;
        }
        "composer_border" | "border" => {
            app.composer_border = settings.composer_border;
            app.needs_redraw = true;
        }
        "composer_vim_mode" | "vim_mode" | "vim" => {
            app.composer.vim_enabled = settings.composer_vim_mode == "vim";
            app.composer.vim_mode = if app.composer.vim_enabled {
                VimMode::Normal
            } else {
                VimMode::Insert
            };
            app.composer.vim_pending_d = false;
            app.needs_redraw = true;
        }
        "paste_burst_detection" | "paste_burst" => {
            app.use_paste_burst_detection = settings.paste_burst_detection;
            if !app.use_paste_burst_detection {
                app.paste_burst.clear_after_explicit_paste();
            }
        }
        "mention_menu_limit" | "mention_limit" => {
            app.mention_menu_limit = settings.mention_menu_limit;
            app.composer.mention_completion_cache = None;
            app.needs_redraw = true;
        }
        "mention_menu_behavior" | "mention_behavior" | "mention_menu" => {
            app.mention_menu_behavior = settings.mention_menu_behavior.clone();
            app.composer.mention_completion_cache = None;
            app.needs_redraw = true;
        }
        "mention_walk_depth" | "mention_depth" | "completions_walk_depth" => {
            app.mention_walk_depth = settings.mention_walk_depth;
            app.composer.mention_completion_cache = None;
            app.needs_redraw = true;
        }
        "workspace_follow_symlinks" | "follow_symlinks" => {
            app.workspace_follow_symlinks = settings.workspace_follow_symlinks;
            app.composer.mention_completion_cache = None;
            app.needs_redraw = true;
            // Engine tools use EngineConfig which is fixed at startup
            return CommandResult::message(if persist {
                if let Err(e) = settings.save() {
                    return CommandResult::error(format!("Failed to save: {e}"));
                }
                format!(
                    "workspace_follow_symlinks = {} (saved; restart required for engine tools)",
                    settings.workspace_follow_symlinks
                )
            } else {
                format!(
                    "workspace_follow_symlinks = {} (session only for UI; restart required for engine tools)",
                    settings.workspace_follow_symlinks
                )
            });
        }
        "transcript_spacing" | "spacing" => {
            app.transcript_spacing =
                crate::tui::app::TranscriptSpacing::from_setting(&settings.transcript_spacing);
            app.mark_history_updated();
        }
        "tool_collapse" | "tool_collapse_mode" | "collapse" => {
            app.tool_collapse_mode =
                crate::tui::app::ToolCollapseMode::from_setting(&settings.tool_collapse_mode);
            app.expanded_tool_runs.clear();
            app.mark_history_updated();
        }
        "default_mode" | "mode" => {
            let mode = AppMode::from_setting(&settings.default_mode);
            app.set_mode(mode);
        }
        "max_history" | "history" => {
            app.max_input_history = settings.max_input_history;
        }
        "default_model" => {
            if let Some(ref model) = settings.default_model {
                app.set_model_selection(model.clone());
                if app.auto_model {
                    app.reasoning_effort = ReasoningEffort::Auto;
                    app.last_effective_reasoning_effort = None;
                }
                app.update_model_compaction_budget();
                app.session.last_prompt_tokens = None;
                app.session.last_completion_tokens = None;
                app.session.last_output_throughput = None;
                action = Some(AppAction::UpdateCompaction(app.compaction_config()));
            }
        }
        "reasoning_effort" | "effort" => {
            app.reasoning_effort = if app.auto_model {
                ReasoningEffort::Auto
            } else {
                settings
                    .reasoning_effort
                    .as_deref()
                    .map_or_else(ReasoningEffort::default, |value| {
                        ReasoningEffort::from_setting_for_provider(value, app.api_provider)
                    })
            };
            app.last_effective_reasoning_effort = None;
            app.update_model_compaction_budget();
            action = Some(AppAction::UpdateCompaction(app.compaction_config()));
        }
        "sidebar_width" | "sidebar" => {
            app.sidebar_width_percent = settings.sidebar_width_percent;
            app.mark_history_updated();
        }
        "sidebar_focus" | "focus" => {
            app.set_sidebar_focus(SidebarFocus::from_setting(&settings.sidebar_focus));
        }
        "context_panel" | "context" | "session_panel" => {
            app.context_panel = settings.context_panel;
            app.needs_redraw = true;
        }
        _ => {}
    }

    let display_value = match key.as_str() {
        "default_mode" | "mode" => settings.default_mode.clone(),
        "cost_currency" | "currency" => settings.cost_currency.clone(),
        "theme" | "ui_theme" => settings.theme.clone(),
        "synchronized_output" | "sync_output" | "sync" => settings.synchronized_output.clone(),
        "background_color" | "background" | "bg" => settings
            .background_color
            .clone()
            .unwrap_or_else(|| "default".to_string()),
        "reasoning_effort" | "effort" => settings.reasoning_effort.as_deref().map_or_else(
            || "config/default".to_string(),
            |value| {
                ReasoningEffort::from_setting_for_provider(value, app.api_provider)
                    .as_setting_for_provider(app.api_provider)
                    .to_string()
            },
        ),
        "composer_vim_mode" | "vim_mode" | "vim" => settings.composer_vim_mode.clone(),
        "low_motion" | "motion" => settings.low_motion.to_string(),
        "fancy_animations" | "fancy" | "animations" => settings.fancy_animations.to_string(),
        _ => value.to_string(),
    };

    let message = if persist {
        if let Err(e) = settings.save() {
            return CommandResult::error(format!("Failed to save: {e}"));
        }
        format!("{key} = {display_value} (saved)")
    } else {
        format!("{key} = {display_value} (session only, add --save to persist)")
    };

    CommandResult {
        message: Some(message),
        action,
        is_error: false,
    }
}

/// Select the TUI operating mode.
pub fn mode(app: &mut App, arg: Option<&str>) -> CommandResult {
    let Some(arg) = arg.filter(|value| !value.trim().is_empty()) else {
        return CommandResult::action(AppAction::OpenModePicker);
    };
    match parse_mode_arg(arg) {
        Some(mode) => {
            let (message, changed) = switch_mode_with_status(app, mode);
            if changed {
                CommandResult::with_message_and_action(message, AppAction::ModeChanged(mode))
            } else {
                CommandResult::message(message)
            }
        }
        None => CommandResult::error("Usage: /mode [agent|plan|yolo|1|2|3]"),
    }
}

pub fn switch_mode(app: &mut App, mode: AppMode) -> String {
    switch_mode_with_status(app, mode).0
}

fn switch_mode_with_status(app: &mut App, mode: AppMode) -> (String, bool) {
    if app.set_mode(mode) {
        (
            format!("Switched to {} mode.", mode_display_name(mode)),
            true,
        )
    } else {
        (
            format!("Already in {} mode.", mode_display_name(mode)),
            false,
        )
    }
}

fn parse_mode_arg(arg: &str) -> Option<AppMode> {
    match arg.trim().to_ascii_lowercase().as_str() {
        "agent" | "1" => Some(AppMode::Agent),
        "plan" | "2" => Some(AppMode::Plan),
        "yolo" | "3" => Some(AppMode::Yolo),
        _ => None,
    }
}

fn mode_display_name(mode: AppMode) -> &'static str {
    match mode {
        AppMode::Agent => "Agent",
        AppMode::Plan => "Plan",
        AppMode::Yolo => "YOLO",
    }
}

/// `/theme [name]` — with no argument, open the interactive picker (arrow
/// keys, live preview, Enter to persist, Esc to revert). With an argument,
/// route through `set_config_value("theme", ...)` so the apply + save flow is
/// shared with `/config`.
pub fn theme(app: &mut App, arg: Option<&str>) -> CommandResult {
    match arg.map(str::trim).filter(|s| !s.is_empty()) {
        None => CommandResult::action(AppAction::OpenThemePicker),
        Some(name) => set_config_value(app, "theme", name, true),
    }
}

/// `/slop [query|export]` — inspect or export the slop ledger (#2127).
/// With no arguments, prints a summary. `query` shows filtered results;
/// `export` outputs the full ledger as Markdown.
pub fn slop(_app: &mut App, arg: Option<&str>) -> CommandResult {
    let arg = arg.map(str::trim).unwrap_or("");
    let ledger = match crate::slop_ledger::SlopLedger::load() {
        Ok(l) => l,
        Err(e) => return CommandResult::error(format!("Failed to load slop ledger: {e}")),
    };

    match arg {
        "" => CommandResult::message(ledger.summary()),
        "query" | "q" => {
            if ledger.is_empty() {
                return CommandResult::message("Slop ledger is empty.");
            }
            let mut out = String::new();
            for entry in &ledger.query(&Default::default()) {
                use std::fmt::Write;
                let _ = writeln!(
                    out,
                    "[{}] {} ({:?} | {:?}) — {}",
                    crate::slop_ledger::short_id(&entry.id),
                    entry.bucket.as_str(),
                    entry.severity,
                    entry.status,
                    entry.title
                );
            }
            CommandResult::message(out)
        }
        "export" | "e" => {
            let md = ledger.export_markdown(None, None);
            CommandResult::message(md)
        }
        _ => CommandResult::error(format!(
            "Unknown /slop action '{arg}'. Use /slop, /slop query, or /slop export."
        )),
    }
}

/// Manage workspace-level trust and the per-path allowlist.
///
/// Subcommands:
/// - `/trust`            – show current state and trusted external paths
/// - `/trust on`         – legacy: trust the entire workspace (turn off all path checks)
/// - `/trust off`        – disable workspace-level trust mode
/// - `/trust add <path>` – add a directory to the allowlist (#29)
/// - `/trust remove <path>` (alias `rm`) – remove a path from the allowlist
/// - `/trust list`       – list trusted external paths for this workspace
pub fn trust(app: &mut App, arg: Option<&str>) -> CommandResult {
    let raw = arg.map(str::trim).unwrap_or("");
    let mut parts = raw.splitn(2, char::is_whitespace);
    let sub = parts.next().unwrap_or("").to_lowercase();
    let rest = parts.next().map(str::trim).unwrap_or("");
    let workspace = app.workspace.clone();

    match sub.as_str() {
        "" | "status" | "list" => trust_status(&workspace, app, sub == "list"),
        "on" | "enable" | "yes" | "y" => {
            app.trust_mode = true;
            CommandResult::message(
                "Workspace trust mode enabled — agent file tools can now read/write any path. \
                 Use `/trust off` to revert; prefer `/trust add <path>` for a narrower opt-in.",
            )
        }
        "off" | "disable" | "no" | "n" => {
            app.trust_mode = false;
            CommandResult::message("Workspace trust mode disabled.")
        }
        "add" => trust_add(&workspace, rest),
        "remove" | "rm" | "del" | "delete" => trust_remove(&workspace, rest),
        other => CommandResult::error(format!(
            "Unknown /trust action `{other}`. Use `/trust`, `/trust on|off`, `/trust add <path>`, or `/trust remove <path>`."
        )),
    }
}

fn trust_status(workspace: &Path, app: &App, force_paths: bool) -> CommandResult {
    let trust = crate::workspace_trust::WorkspaceTrust::load_for(workspace);
    let mut lines = Vec::new();
    lines.push(format!(
        "Workspace trust mode: {}",
        if app.trust_mode {
            "enabled"
        } else {
            "disabled"
        }
    ));
    if trust.paths().is_empty() {
        if force_paths {
            lines.push("No external paths trusted from this workspace.".to_string());
        } else {
            lines.push(
                "No external paths trusted yet. Use `/trust add <path>` to allow a directory."
                    .to_string(),
            );
        }
    } else {
        lines.push(format!("Trusted external paths ({}):", trust.paths().len()));
        for path in trust.paths() {
            lines.push(format!("  • {}", path.display()));
        }
    }
    CommandResult::message(lines.join("\n"))
}

fn trust_add(workspace: &Path, raw: &str) -> CommandResult {
    if raw.is_empty() {
        return CommandResult::error(
            "Usage: /trust add <path>. Supply an absolute path or a path relative to the workspace.",
        );
    }
    let path = PathBuf::from(expand_tilde(raw));
    if !path.exists() {
        return CommandResult::error(format!(
            "Path not found: {} — supply an existing directory or file.",
            path.display()
        ));
    }
    match crate::workspace_trust::add(workspace, &path) {
        Ok(stored) => CommandResult::message(format!(
            "Added to trust list for this workspace: {}",
            stored.display()
        )),
        Err(err) => CommandResult::error(format!("Failed to update trust list: {err}")),
    }
}

fn trust_remove(workspace: &Path, raw: &str) -> CommandResult {
    if raw.is_empty() {
        return CommandResult::error("Usage: /trust remove <path>");
    }
    let path = PathBuf::from(expand_tilde(raw));
    match crate::workspace_trust::remove(workspace, &path) {
        Ok(true) => CommandResult::message(format!("Removed from trust list: {}", path.display())),
        Ok(false) => CommandResult::message(format!("Not in trust list: {}", path.display())),
        Err(err) => CommandResult::error(format!("Failed to update trust list: {err}")),
    }
}

fn expand_tilde(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    } else if raw == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home.to_string_lossy().into_owned();
    }
    raw.to_string()
}

/// Toggle LSP diagnostics on/off or show status.
///
/// - `/lsp on` — enable inline LSP diagnostics
/// - `/lsp off` — disable inline LSP diagnostics
/// - `/lsp status` — show whether diagnostics are currently enabled
pub fn lsp_command(app: &mut App, arg: Option<&str>) -> CommandResult {
    let raw = arg.map(str::trim).unwrap_or("");
    // Access lsp_manager config through the App's engine handle
    let current_enabled = app.lsp_enabled;

    match raw {
        "" | "status" => {
            let status = if current_enabled { "on" } else { "off" };
            CommandResult::message(format!(
                "LSP diagnostics are currently **{status}**.\n\n\
                 Use `/lsp on` to enable or `/lsp off` to disable inline diagnostics after file edits."
            ))
        }
        "on" | "enable" | "1" | "true" => {
            app.lsp_enabled = true;
            CommandResult::message(
                "LSP diagnostics enabled — file edit results will include compiler errors and warnings when available.",
            )
        }
        "off" | "disable" | "0" | "false" => {
            app.lsp_enabled = false;
            CommandResult::message("LSP diagnostics disabled.")
        }
        other => CommandResult::error(format!(
            "Unknown /lsp argument `{other}`. Use `/lsp on`, `/lsp off`, or `/lsp status`."
        )),
    }
}

/// Logout - clear all saved API keys and return to onboarding.
/// This is NOT provider-scoped — it clears keys for every saved provider.
/// For single-provider key replacement, use
/// `codewhale auth clear --provider <id>` and
/// `codewhale auth set --provider <id>`.
pub fn logout(app: &mut App) -> CommandResult {
    let provider_name = app.api_provider.as_str();
    match clear_active_provider_api_key(provider_name) {
        Ok(()) => {
            app.onboarding = OnboardingState::ApiKey;
            app.onboarding_needs_api_key = true;
            app.api_key_input.clear();
            app.api_key_cursor = 0;
            CommandResult::message(format!(
                "Cleared API key for {provider_name}. \
                 Use `codewhale auth clear --provider <id>` to clear a different provider."
            ))
        }
        Err(e) => CommandResult::error(format!("Failed to clear API key for {provider_name}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::test_support::lock_test_env;
    use crate::tui::app::{App, TuiOptions};
    use crate::tui::approval::ApprovalMode;
    use std::env;
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::tempdir;

    struct EnvGuard {
        home: Option<OsString>,
        userprofile: Option<OsString>,
        codewhale_config_path: Option<OsString>,
        deepseek_config_path: Option<OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new(home: &Path) -> Self {
            let lock = crate::test_support::lock_test_env();
            let home_str = OsString::from(home.as_os_str());
            let config_path = home.join(".deepseek").join("config.toml");
            let config_str = OsString::from(config_path.as_os_str());
            let home_prev = env::var_os("HOME");
            let userprofile_prev = env::var_os("USERPROFILE");
            let codewhale_config_prev = env::var_os("CODEWHALE_CONFIG_PATH");
            let deepseek_config_prev = env::var_os("DEEPSEEK_CONFIG_PATH");

            // Safety: test-only environment mutation guarded by process-wide mutex.
            unsafe {
                env::set_var("HOME", &home_str);
                env::set_var("USERPROFILE", &home_str);
                env::remove_var("CODEWHALE_CONFIG_PATH");
                env::set_var("DEEPSEEK_CONFIG_PATH", &config_str);
            }

            Self {
                home: home_prev,
                userprofile: userprofile_prev,
                codewhale_config_path: codewhale_config_prev,
                deepseek_config_path: deepseek_config_prev,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = self.home.take() {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::set_var("HOME", value);
                }
            } else {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::remove_var("HOME");
                }
            }

            if let Some(value) = self.userprofile.take() {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::set_var("USERPROFILE", value);
                }
            } else {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::remove_var("USERPROFILE");
                }
            }

            if let Some(value) = self.codewhale_config_path.take() {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::set_var("CODEWHALE_CONFIG_PATH", value);
                }
            } else {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::remove_var("CODEWHALE_CONFIG_PATH");
                }
            }

            if let Some(value) = self.deepseek_config_path.take() {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::set_var("DEEPSEEK_CONFIG_PATH", value);
                }
            } else {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::remove_var("DEEPSEEK_CONFIG_PATH");
                }
            }
        }
    }

    fn create_test_app() -> App {
        let options = TuiOptions {
            model: "test-model".to_string(),
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
            skip_onboarding: false,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let mut app = App::new(options, &Config::default());
        // App::new folds in saved TUI settings from the developer machine.
        // Pin command tests back to DeepSeek semantics so model aliases are
        // not normalized through a provider selected in an interactive run.
        app.model = "test-model".to_string();
        app.auto_model = false;
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app.model_ids_passthrough = false;
        app
    }

    #[test]
    fn test_mode_yolo_sets_all_flags() {
        let mut app = create_test_app();
        // Switch to Agent first to guarantee a clean starting state regardless of
        // user settings on the host machine.
        let _ = mode(&mut app, Some("agent"));
        let result = mode(&mut app, Some("yolo"));
        assert!(result.message.unwrap().contains("Switched to YOLO mode"));
        assert_eq!(result.action, Some(AppAction::ModeChanged(AppMode::Yolo)));
        assert!(app.allow_shell);
        assert!(app.trust_mode);
        assert!(app.yolo);
        assert_eq!(app.approval_mode, ApprovalMode::Auto);
        assert_eq!(app.mode, AppMode::Yolo);
    }

    #[test]
    fn test_mode_switch_command_accepts_names_and_numbers() {
        let mut app = create_test_app();
        let _ = mode(&mut app, Some("agent"));
        assert_eq!(app.mode, AppMode::Agent);
        let result = mode(&mut app, Some("2"));
        assert_eq!(result.action, Some(AppAction::ModeChanged(AppMode::Plan)));
        assert_eq!(app.mode, AppMode::Plan);
        let result = mode(&mut app, Some("3"));
        assert_eq!(result.action, Some(AppAction::ModeChanged(AppMode::Yolo)));
        assert_eq!(app.mode, AppMode::Yolo);
    }

    #[test]
    fn test_mode_without_arg_opens_picker() {
        let mut app = create_test_app();
        let result = mode(&mut app, None);
        assert!(result.message.is_none());
        assert!(matches!(result.action, Some(AppAction::OpenModePicker)));
    }

    #[test]
    fn test_mode_rejects_unknown_value() {
        let mut app = create_test_app();
        let result = mode(&mut app, Some("fast"));
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("Usage: /mode"));
    }

    #[test]
    fn test_show_config_defaults_to_native() {
        let mut app = create_test_app();
        app.session.total_tokens = 1234;
        let result = show_config(&mut app, None);
        assert!(result.message.is_none());
        assert!(matches!(result.action, Some(AppAction::OpenConfigView)));
    }

    #[test]
    fn test_show_config_native_opens_legacy_editor() {
        let mut app = create_test_app();
        let result = show_config(&mut app, Some("native"));
        assert!(result.message.is_none());
        assert!(matches!(result.action, Some(AppAction::OpenConfigView)));
    }

    #[test]
    fn test_show_settings_loads_from_file() {
        let _lock = lock_test_env();
        let mut app = create_test_app();
        let result = show_settings(&mut app);
        // Settings should load (may use defaults if file doesn't exist)
        assert!(result.message.is_some());
    }

    #[test]
    fn config_model_updates_app_state() {
        let mut app = create_test_app();
        let _old_model = app.model.clone();
        let result = config_command(&mut app, Some("model deepseek-v4-flash"));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("model = deepseek-v4-flash"));
        assert_eq!(app.model, "deepseek-v4-flash");
        assert!(matches!(
            result.action,
            Some(AppAction::UpdateCompaction(_))
        ));
    }

    #[test]
    fn config_model_auto_enables_auto_thinking() {
        let mut app = create_test_app();
        app.reasoning_effort = ReasoningEffort::Off;

        let result = config_command(&mut app, Some("model auto"));

        assert!(result.message.is_some());
        assert!(app.auto_model);
        assert_eq!(app.model, "auto");
        assert_eq!(app.reasoning_effort, ReasoningEffort::Auto);
        assert!(app.last_effective_model.is_none());
        assert!(app.last_effective_reasoning_effort.is_none());
    }

    #[test]
    fn config_reasoning_effort_uses_codex_provider_labels() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-codex-effort-config-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let mut app = create_test_app();
        app.api_provider = ApiProvider::OpenaiCodex;
        app.reasoning_effort = ReasoningEffort::High;

        let result = set_config_value(&mut app, "reasoning_effort", "off", false);

        assert_eq!(app.reasoning_effort, ReasoningEffort::Low);
        assert_eq!(
            result.message.as_deref(),
            Some("reasoning_effort = low (session only, add --save to persist)")
        );

        let result = set_config_value(&mut app, "reasoning_effort", "xhigh", false);

        assert_eq!(app.reasoning_effort, ReasoningEffort::Max);
        assert_eq!(
            result.message.as_deref(),
            Some("reasoning_effort = xhigh (session only, add --save to persist)")
        );
    }

    #[test]
    fn config_fancy_animations_obeys_ghostty_override() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-ghostty-fancy-config-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let prev_term_program = env::var_os("TERM_PROGRAM");
        // Safety: test-only environment mutation guarded by EnvGuard's lock.
        unsafe {
            env::set_var("TERM_PROGRAM", "Ghostty");
        }

        let mut app = create_test_app();
        assert!(!app.fancy_animations);

        let result = set_config_value(&mut app, "fancy_animations", "true", false);

        assert!(!result.is_error);
        assert!(
            !app.fancy_animations,
            "Ghostty compatibility override must keep the water strip disabled"
        );
        assert_eq!(
            result.message.as_deref(),
            Some("fancy_animations = false (session only, add --save to persist)")
        );

        // Safety: cleanup under EnvGuard's lock.
        unsafe {
            match prev_term_program {
                Some(v) => env::set_var("TERM_PROGRAM", v),
                None => env::remove_var("TERM_PROGRAM"),
            }
        }
    }

    #[test]
    fn config_model_accepts_future_deepseek_model_id() {
        let mut app = create_test_app();
        let result = config_command(&mut app, Some("model deepseek-v4"));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("model = deepseek-v4"));
        assert_eq!(app.model, "deepseek-v4");
    }

    #[test]
    fn config_model_with_save_flag() {
        let mut app = create_test_app();
        let _result = config_command(&mut app, Some("model deepseek-v4-flash --save"));
        // Note: This test may fail in environments where settings can't be saved
        // The important thing is that the model is updated
        assert_eq!(app.model, "deepseek-v4-flash");
    }

    #[test]
    fn config_default_mode_normal_save_reports_normalized_value() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-default-mode-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let mut app = create_test_app();
        let result = config_command(&mut app, Some("default_mode normal --save"));
        let msg = result.message.unwrap();
        assert_eq!(msg, "default_mode = agent (saved)");
        assert_eq!(app.mode, AppMode::Agent);

        let settings_path = Settings::path().unwrap();
        let saved = fs::read_to_string(settings_path).unwrap();
        assert!(saved.contains("default_mode = \"agent\""));
    }

    #[test]
    fn config_command_cost_currency_save_persists_value() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-cost-currency-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let mut app = create_test_app();
        let result = config_command(&mut app, Some("cost_currency cny --save"));
        let msg = result.message.unwrap();

        assert_eq!(msg, "cost_currency = cny (saved)");
        assert_eq!(app.cost_currency, crate::pricing::CostCurrency::Cny);

        let settings_path = Settings::path().unwrap();
        let saved = fs::read_to_string(settings_path).unwrap();
        assert!(saved.contains("cost_currency = \"cny\""));
    }

    #[test]
    fn config_command_base_url_save_persists_value() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "deepseek-tui-base-url-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let mut app = create_test_app();
        let result = config_command(
            &mut app,
            Some("base_url https://example.internal.local/v1 --save"),
        );
        let msg = result.message.unwrap();
        let saved_path = crate::config_persistence::config_toml_path(None).unwrap();
        let saved = fs::read_to_string(&saved_path).unwrap();

        assert_eq!(
            msg,
            format!(
                "base_url = https://example.internal.local/v1 (saved to {})",
                saved_path.display()
            )
        );
        assert!(saved.contains("base_url = \"https://example.internal.local/v1\""));
    }

    #[test]
    fn config_command_provider_emits_switch_action() {
        let mut app = create_test_app();
        let result = config_command(&mut app, Some("provider openrouter"));

        assert!(!result.is_error);
        assert_eq!(result.message.as_deref(), Some("provider = openrouter"));
        match result.action {
            Some(AppAction::SwitchProvider { provider, model }) => {
                assert_eq!(provider, ApiProvider::Openrouter);
                assert_eq!(model, None);
            }
            other => panic!("expected SwitchProvider action, got {other:?}"),
        }
    }

    #[test]
    fn config_command_provider_rejects_unknown_provider() {
        let mut app = create_test_app();
        // "anthropic" became a real provider in #3014; probe with an id that
        // stays unknown.
        let result = config_command(&mut app, Some("provider not-a-provider"));
        assert!(result.is_error);
        let msg = result.message.unwrap();
        assert!(msg.contains("Unknown provider 'not-a-provider'"));
        assert!(msg.contains("openrouter"));
        assert!(msg.contains("xiaomi-mimo"));
    }

    #[test]
    fn config_command_allow_shell_enables_agent_shell_session_only() {
        let mut app = create_test_app();
        assert!(!app.allow_shell);

        let result = config_command(&mut app, Some("allow_shell true"));
        assert!(!result.is_error);
        assert!(app.allow_shell);
        let msg = result.message.unwrap();

        assert!(msg.contains("allow_shell = true"));
        assert!(msg.contains("session only"));
        assert!(msg.contains("Agent mode"));
        assert!(msg.contains("approval gating"));
        assert!(msg.contains("next turn"));
        assert!(msg.contains("YOLO also enables shell and auto-approves"));
    }

    #[test]
    fn config_command_allow_shell_save_persists_root_boolean() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-allow-shell-save-app-path-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();

        let config_path = temp_root.join("custom-config.toml");

        let mut app = create_test_app();
        app.config_path = Some(config_path.clone());
        let result = config_command(&mut app, Some("allow_shell true --save"));
        let msg = result.message.unwrap();
        let saved = fs::read_to_string(&config_path).unwrap();

        assert!(app.allow_shell);
        assert_eq!(
            msg,
            format!(
                "allow_shell = true (saved to {}). Agent mode will expose shell on the next turn with approval gating. YOLO also enables shell and auto-approves.",
                config_path.display()
            )
        );
        assert!(saved.contains("allow_shell = true"));
    }

    #[test]
    fn config_command_allow_shell_rejects_invalid_boolean() {
        let mut app = create_test_app();
        let result = config_command(&mut app, Some("allow_shell maybe"));
        assert!(result.is_error);
        assert!(!app.allow_shell);
        let msg = result.message.unwrap();
        assert!(msg.contains("Failed to parse boolean 'maybe'"));
    }

    #[test]
    fn config_command_subagents_off_updates_session_features() {
        let mut app = create_test_app();
        assert!(app.features.enabled(Feature::Subagents));

        let result = config_command(&mut app, Some("subagents off"));

        assert!(!result.is_error);
        assert!(!app.features.enabled(Feature::Subagents));
        assert!(matches!(
            result.action,
            Some(AppAction::UpdateFeatures(ref features))
                if !features.enabled(Feature::Subagents)
        ));
        let msg = result.message.unwrap();
        assert!(msg.contains("subagents = false"));
        assert!(msg.contains("session only"));
        assert!(msg.contains("agent tool"));
        assert!(msg.contains("next turn"));
    }

    #[test]
    fn config_command_subagents_status_reports_effective_state() {
        let mut app = create_test_app();
        app.features.disable(Feature::Subagents);

        let result = config_command(&mut app, Some("subagents status"));

        assert!(!result.is_error);
        assert!(result.action.is_none());
        let msg = result.message.unwrap();
        assert!(msg.contains("subagents = false"));
        assert!(msg.contains("disabled"));
    }

    #[test]
    fn config_command_subagents_save_persists_features_table() {
        let temp_root = tempdir().expect("tempdir");
        let config_path = temp_root.path().join("custom-config.toml");

        let mut app = create_test_app();
        app.config_path = Some(config_path.clone());

        let disabled = config_command(&mut app, Some("subagents off --save"));
        assert!(!disabled.is_error);
        assert!(!app.features.enabled(Feature::Subagents));
        let saved = fs::read_to_string(&config_path).unwrap();
        assert!(saved.contains("[features]"));
        assert!(saved.contains("subagents = false"));
        let reloaded = Config::load(Some(config_path.clone()), None).unwrap();
        assert!(!reloaded.features().enabled(Feature::Subagents));

        let enabled = config_command(&mut app, Some("subagents on --save"));
        assert!(!enabled.is_error);
        assert!(app.features.enabled(Feature::Subagents));
        let saved = fs::read_to_string(&config_path).unwrap();
        assert!(saved.contains("subagents = true"));
        let reloaded = Config::load(Some(config_path.clone()), None).unwrap();
        assert!(reloaded.features().enabled(Feature::Subagents));

        let direct_key = config_command(&mut app, Some("features.subagents false --save"));
        assert!(!direct_key.is_error);
        assert!(!app.features.enabled(Feature::Subagents));
        let reloaded = Config::load(Some(config_path), None).unwrap();
        assert!(!reloaded.features().enabled(Feature::Subagents));
    }

    #[test]
    fn config_command_base_url_without_save_requires_save() {
        let _lock = lock_test_env();
        let mut app = create_test_app();
        let result = config_command(&mut app, Some("base_url https://example.internal.local/v1"));
        assert!(result.is_error);
        let msg = result.message.unwrap();

        assert!(
            msg.contains("base_url must be saved with --save"),
            "got {msg}"
        );
    }

    #[test]
    fn config_command_base_url_reads_current_value_from_config() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "deepseek-tui-base-url-show-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            "base_url = \"https://api.from-config.local/v1\"\n",
        )
        .unwrap();

        let mut app = create_test_app();
        let result = config_command(&mut app, Some("base_url"));
        let msg = result.message.unwrap();

        assert_eq!(msg, "base_url = https://api.from-config.local/v1");
    }

    #[test]
    fn config_command_base_url_reads_current_value_from_app_config_path() {
        let temp_root = env::temp_dir().join(format!(
            "deepseek-tui-base-url-app-config-path-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();

        let config_path = temp_root.join("custom-config.toml");
        fs::write(
            &config_path,
            "base_url = \"https://api.from-app-path.local/v1\"\n",
        )
        .unwrap();

        let mut app = create_test_app();
        app.config_path = Some(config_path.clone());
        let result = config_command(&mut app, Some("base_url"));
        let msg = result.message.unwrap();

        assert_eq!(msg, "base_url = https://api.from-app-path.local/v1");
    }

    #[test]
    fn config_command_base_url_save_persists_to_app_config_path() {
        let temp_root = env::temp_dir().join(format!(
            "deepseek-tui-base-url-save-app-path-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();

        let config_path = temp_root.join("custom-config.toml");

        let mut app = create_test_app();
        app.config_path = Some(config_path.clone());
        let result = config_command(
            &mut app,
            Some("base_url https://example.session.local/v1 --save"),
        );
        let msg = result.message.unwrap();
        let saved = fs::read_to_string(&config_path).unwrap();

        assert_eq!(
            msg,
            format!(
                "base_url = https://example.session.local/v1 (saved to {})",
                config_path.display()
            )
        );
        assert!(saved.contains("base_url = \"https://example.session.local/v1\""));
    }

    #[test]
    fn config_command_stream_chunk_timeout_session_query_uses_live_value() {
        let _lock = lock_test_env();
        let mut app = create_test_app();

        let result = config_command(&mut app, Some("stream_chunk_timeout_secs 90"));
        assert!(!result.is_error);
        assert_eq!(app.stream_chunk_timeout_secs, 90);
        assert!(matches!(
            result.action,
            Some(AppAction::UpdateStreamChunkTimeout(90))
        ));

        let query = config_command(&mut app, Some("stream_chunk_timeout_secs"));
        assert_eq!(
            query.message.as_deref(),
            Some("stream_chunk_timeout_secs = 90")
        );
    }

    #[test]
    fn config_command_stream_chunk_timeout_save_persists_tui_key() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-stream-timeout-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join("custom-config.toml");
        let mut app = create_test_app();
        app.config_path = Some(config_path.clone());

        let result = config_command(&mut app, Some("stream_chunk_timeout_secs 120 --save"));
        let msg = result.message.unwrap();
        let saved = fs::read_to_string(&config_path).unwrap();

        assert_eq!(
            msg,
            format!(
                "stream_chunk_timeout_secs = 120 (saved to {}; affects subsequent turns in this session)",
                config_path.display()
            )
        );
        assert!(saved.contains("[tui]"));
        assert!(saved.contains("stream_chunk_timeout_secs = 120"));
        assert_eq!(app.stream_chunk_timeout_secs, 120);
        assert!(matches!(
            result.action,
            Some(AppAction::UpdateStreamChunkTimeout(120))
        ));
    }

    #[test]
    fn config_command_stream_chunk_timeout_rejects_invalid_input() {
        let _lock = lock_test_env();
        let mut app = create_test_app();

        let text = config_command(&mut app, Some("stream_chunk_timeout_secs abc"));
        assert!(text.is_error);
        assert!(
            text.message
                .unwrap()
                .contains("stream_chunk_timeout_secs must be a whole number")
        );

        let high = config_command(&mut app, Some("stream_chunk_timeout_secs 3601"));
        assert!(high.is_error);
        assert!(
            high.message
                .unwrap()
                .contains("stream_chunk_timeout_secs must be 0 or 1..=3600")
        );
    }

    #[test]
    fn config_command_stream_chunk_timeout_zero_reports_effective_default() {
        let _lock = lock_test_env();
        let mut app = create_test_app();

        let result = config_command(&mut app, Some("stream_chunk_timeout_secs 0"));

        assert!(!result.is_error);
        assert_eq!(
            app.stream_chunk_timeout_secs,
            DEFAULT_STREAM_CHUNK_TIMEOUT_SECS
        );
        assert_eq!(
            result.message.as_deref(),
            Some(
                "stream_chunk_timeout_secs = 0 (default 300) (session only; affects subsequent turns in this session)"
            )
        );
        assert!(matches!(
            result.action,
            Some(AppAction::UpdateStreamChunkTimeout(
                DEFAULT_STREAM_CHUNK_TIMEOUT_SECS
            ))
        ));
    }

    #[test]
    fn config_command_provider_url_token_plan_persists_provider_base_url() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-provider-url-save-app-path-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();

        let config_path = temp_root.join("custom-config.toml");

        let mut app = create_test_app();
        app.api_provider = ApiProvider::XiaomiMimo;
        app.config_path = Some(config_path.clone());
        let result = config_command(&mut app, Some("provider_url token-plan --save"));
        let msg = result.message.unwrap();
        let saved = fs::read_to_string(&config_path).unwrap();

        assert_eq!(
            msg,
            format!(
                "provider_url = {} for xiaomi-mimo (saved to {}; restart required)",
                DEFAULT_XIAOMI_MIMO_BASE_URL,
                config_path.display()
            )
        );
        assert!(saved.contains("[providers.xiaomi_mimo]"));
        assert!(saved.contains(&format!("base_url = \"{}\"", DEFAULT_XIAOMI_MIMO_BASE_URL)));
    }

    #[test]
    fn config_command_provider_url_without_save_requires_save() {
        let _lock = lock_test_env();
        let mut app = create_test_app();
        app.api_provider = ApiProvider::XiaomiMimo;
        let result = config_command(&mut app, Some("provider_url token-plan"));
        assert!(result.is_error);
        let msg = result.message.unwrap();

        assert!(
            msg.contains("provider_url must be saved with --save"),
            "got {msg}"
        );
    }

    #[test]
    fn theme_command_accepts_grayscale_arg() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-theme-command-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let mut app = create_test_app();
        let result = theme(&mut app, Some("grayscale"));

        assert_eq!(result.message.unwrap(), "theme = grayscale (saved)");
        assert_eq!(app.theme_id, crate::palette::ThemeId::Grayscale);
        assert_eq!(app.ui_theme.mode, crate::palette::PaletteMode::Grayscale);
        assert!(app.needs_redraw);
    }

    #[test]
    fn set_theme_save_updates_live_app_and_persists() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-theme-save-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let mut app = create_test_app();
        let result = config_command(&mut app, Some("theme grayscale --save"));
        let msg = result.message.unwrap();

        assert_eq!(msg, "theme = grayscale (saved)");
        assert_eq!(app.ui_theme.mode, crate::palette::PaletteMode::Grayscale);

        let settings_path = Settings::path().unwrap();
        let saved = fs::read_to_string(settings_path).unwrap();
        assert!(saved.contains("theme = \"grayscale\""));
    }

    #[test]
    fn config_approval_mode_valid_values() {
        let mut app = create_test_app();
        // Test auto
        let result = config_command(&mut app, Some("approval_mode auto"));
        assert!(result.message.is_some());
        assert_eq!(app.approval_mode, ApprovalMode::Auto);

        // Test suggest
        let result = config_command(&mut app, Some("approval_mode suggest"));
        assert!(result.message.is_some());
        assert_eq!(app.approval_mode, ApprovalMode::Suggest);

        // Test never
        let result = config_command(&mut app, Some("approval_mode never"));
        assert!(result.message.is_some());
        assert_eq!(app.approval_mode, ApprovalMode::Never);
    }

    #[test]
    fn config_approval_mode_invalid_value() {
        let mut app = create_test_app();
        let result = config_command(&mut app, Some("approval_mode invalid"));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Invalid approval_mode"));
    }

    #[test]
    fn config_without_save_flag() {
        let _lock = lock_test_env();
        let mut app = create_test_app();
        let result = config_command(&mut app, Some("auto_compact true"));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("(session only"));
    }

    #[test]
    fn config_composer_border_updates_live_app() {
        let _lock = lock_test_env();
        let mut app = create_test_app();
        app.composer_border = true;

        let result = config_command(&mut app, Some("composer_border false"));

        assert!(result.message.is_some());
        assert!(!app.composer_border);
        assert!(app.needs_redraw);
    }

    #[test]
    fn test_trust_on_enables_flag() {
        let mut app = create_test_app();
        // Normalize trust state regardless of user settings on the host machine.
        app.trust_mode = false;
        let result = trust(&mut app, Some("on"));
        let msg = result.message.expect("message");
        assert!(msg.contains("Workspace trust mode enabled"));
        assert!(app.trust_mode);
    }

    #[test]
    fn test_trust_status_default_lists_state() {
        let mut app = create_test_app();
        let result = trust(&mut app, None);
        let msg = result.message.expect("status message");
        assert!(msg.contains("Workspace trust mode"));
    }

    #[test]
    fn test_trust_add_requires_path() {
        let mut app = create_test_app();
        let result = trust(&mut app, Some("add"));
        let msg = result.message.expect("error message");
        assert!(msg.starts_with("Error:"), "got {msg:?}");
    }

    #[test]
    fn test_logout_clears_api_key_state() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-logout-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let config_path = temp_root.join(".deepseek").join("config.toml");
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(&config_path, "api_key = \"test-key\"\n").unwrap();

        let mut app = create_test_app();
        let result = logout(&mut app);
        assert!(result.message.is_some());
        assert_eq!(app.onboarding, OnboardingState::ApiKey);
        assert!(app.onboarding_needs_api_key);
        assert!(app.api_key_input.is_empty());
        assert_eq!(app.api_key_cursor, 0);

        let updated = fs::read_to_string(config_path).unwrap();
        assert!(!updated.contains("api_key"));
    }
}
