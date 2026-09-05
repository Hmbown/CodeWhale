//! Config commands: config, settings, mode switches, trust, logout

use super::CommandResult;
use crate::config::{
    ApiProvider, CompletionSound, Config, DEFAULT_STREAM_CHUNK_TIMEOUT_SECS,
    DEFAULT_SUBAGENT_API_TIMEOUT_SECS, DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS,
    DEFAULT_XIAOMI_MIMO_BASE_URL, MAX_STREAM_CHUNK_TIMEOUT_SECS, MAX_SUBAGENT_API_TIMEOUT_SECS,
    MAX_SUBAGENT_HEARTBEAT_TIMEOUT_SECS, MAX_SUBAGENTS, MIN_STREAM_CHUNK_TIMEOUT_SECS,
    MIN_SUBAGENT_API_TIMEOUT_SECS, MIN_SUBAGENT_HEARTBEAT_TIMEOUT_SECS, NotificationConfigUpdate,
    NotificationMethod, NotificationsConfig, SearchProvider, SearchProviderSource,
    SubagentCompletionNotification, SubagentsConfig, XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL,
    clear_active_provider_api_key, normalize_custom_model_id, normalize_model_name_for_provider,
    validate_route,
};
use crate::config_persistence::{
    persist_provider_base_url_key, persist_root_bool_key, persist_root_string_key,
    persist_subagents_bool_key, persist_subagents_integer_key, persist_table_bool_key,
    persist_table_integer_key, persist_table_string_key, persist_tui_integer_key,
    persist_unset_root_key,
};
use crate::config_ui::{ConfigUiMode, parse_mode};
use crate::localization::{MessageId, resolve_locale, tr};
use crate::settings::Settings;
use crate::tui::app::{
    App, AppAction, AppMode, OnboardingState, ReasoningEffort, ScreenMode, SettingSelection,
    VimMode,
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
/// - `/config ask-rules` — compatibility entry for `/permissions`.
/// - `/config <key>` — shows the current value of a setting.
/// - `/config <key> <value>` — sets a runtime value (session only, add --save to persist).
pub fn config_command(app: &mut App, arg: Option<&str>) -> CommandResult {
    let raw = arg.map(str::trim).unwrap_or("");
    if raw.is_empty() {
        return show_config(app, None);
    }
    if matches!(
        raw.to_ascii_lowercase().as_str(),
        "audit" | "editability" | "editable" | "status"
    ) {
        return config_editability_audit(app);
    }
    let mut raw_words = raw.splitn(2, char::is_whitespace);
    let first_word = raw_words.next();
    if first_word.is_some_and(is_ask_rules_config_token) {
        let rest = raw_words.next().unwrap_or("").trim();
        return super::permissions::permissions_command(app, Some(rest));
    }
    if first_word.is_some_and(|token| {
        token.eq_ignore_ascii_case("workflow") || token.eq_ignore_ascii_case("goal")
    }) && raw_words
        .clone()
        .next()
        .is_none_or(|rest| rest.trim().is_empty())
    {
        return super::workflow_settings(app);
    }
    if first_word.is_some_and(|token| token.eq_ignore_ascii_case("subagents")) {
        let rest = raw_words.next().unwrap_or("").trim();
        return subagents_config_command(app, rest);
    }
    if first_word.is_some_and(|token| token.eq_ignore_ascii_case("search")) {
        let rest = raw_words.next().unwrap_or("").trim();
        return search_config_command(app, rest);
    }
    if first_word.is_some_and(|token| {
        token.eq_ignore_ascii_case("notifications") || token.eq_ignore_ascii_case("notification")
    }) {
        let rest = raw_words.next().unwrap_or("").trim();
        return notifications_config_command(app, rest);
    }
    // `/config preset <name> [--save|-s]` — apply a bundled settings preset (#3478).
    if first_word.is_some_and(|token| token.eq_ignore_ascii_case("preset")) {
        let rest = raw_words.next().unwrap_or("").trim();
        return config_preset_command(app, rest);
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

/// Reject a preset bundle *before* anything is written, returning the message
/// to show, or `None` when every field can be applied.
///
/// The bundle is persisted in one transaction and then mirrored field by field
/// into the live session. A per-field refusal during that mirror pass therefore
/// arrives *after* the file has already been rewritten — the user gets an error
/// and a saved file, which is the partial apply this preflight exists to make
/// impossible. Both refusals a field can raise are knowable up front:
///
/// 1. A live-route key while a turn is running (#2982).
/// 2. A value the setter would reject, checked against a throwaway `Settings`
///    so the real file is never touched by the check.
fn preset_preflight(app: &App, fields: &[(&str, &str)]) -> Option<String> {
    for (key, value) in fields {
        if app.is_loading
            && let Some(subject) = live_route_setting_subject(&key.to_lowercase())
        {
            return Some(app.setting_locked_message(subject));
        }
        if let Err(e) = Settings::default().set(key, value) {
            return Some(format!("Failed to apply preset field {key}={value}: {e}"));
        }
    }
    None
}

/// Apply a bundled settings preset, e.g. `/config preset calm [--save]` (#3478).
///
/// The preset is applied to the live session through the same per-key setter a
/// single `/config <key> <value>` uses, so app state mirroring and (with
/// `--save`) persistence stay consistent. The preset name is validated before
/// any field is touched.
fn config_preset_command(app: &mut App, rest: &str) -> CommandResult {
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let persist = matches!(tokens.last(), Some(&"--save") | Some(&"-s"));
    let name = tokens.first().copied().unwrap_or("");
    if name.is_empty() || name.starts_with('-') {
        return CommandResult::message(
            "Usage: /config preset <name> [--save]. Available presets: calm.",
        );
    }

    let Some(fields) = crate::settings::preset_fields(name) else {
        return CommandResult::error(format!("Unknown preset '{name}'. Available presets: calm."));
    };

    if let Some(refusal) = preset_preflight(app, fields) {
        return CommandResult::error(refusal);
    }

    // Persist the whole bundle atomically when requested (one load/apply/save),
    // now that every field is known to be applicable.
    if persist {
        // `Settings::transact` is what makes "one load/apply/save" true against
        // the *other* writers in this process, not just against a second preset
        // apply: an unsynchronized load/save pair here would write back a
        // pre-image that reverts a concurrent mode/thinking/posture write.
        if let Err(e) = Settings::transact(|settings| settings.apply_preset(name)) {
            return CommandResult::error(format!("Failed to save settings: {e}"));
        }
    }

    // Mirror the bundle into the live session via the per-key setter (the
    // persisted write, if any, already happened atomically above, so this pass
    // is session-only).
    let mut applied = Vec::with_capacity(fields.len());
    for (key, value) in fields {
        let result = set_config_value(app, key, value, false);
        if result.is_error {
            let message = result
                .message
                .unwrap_or_else(|| "unknown apply error".to_string());
            return CommandResult::error(format!(
                "Failed to apply preset field {key}={value}: {message}"
            ));
        }
        applied.push(format!("{key}={value}"));
    }

    let suffix = if persist {
        " (saved)"
    } else {
        " (session only — add --save to persist)"
    };
    CommandResult::message(format!(
        "Applied '{name}' transcript preset{suffix}: {}. Thinking stays visible and tool runs stay expandable.",
        applied.join(", ")
    ))
}

/// Show the current value of a single setting.
fn config_context_window_override(app: &App) -> Option<u32> {
    let mut config = Config::load(app.config_path.clone(), app.config_profile.as_deref()).ok()?;
    config.provider = Some(app.provider_identity_for_persistence().to_string());
    config.context_window_for_provider_config(app.api_provider)
}

fn show_single_setting(app: &App, key: &str) -> CommandResult {
    let key = key.to_lowercase();
    if let Some(subagent_key) = key.strip_prefix("subagents.") {
        return show_subagents_setting(app, subagent_key);
    }
    if let Some(notifications_key) = key.strip_prefix("notifications.") {
        return show_notifications_setting(app, notifications_key);
    }
    fn locale_display(l: crate::localization::Locale) -> &'static str {
        match l {
            crate::localization::Locale::En => "en",
            crate::localization::Locale::ZhHans => "zh-Hans",
            crate::localization::Locale::ZhHant => "zh-Hant",
            crate::localization::Locale::Ja => "ja",
            crate::localization::Locale::PtBr => "pt-BR",
            crate::localization::Locale::Es419 => "es-419",
            crate::localization::Locale::Vi => "vi",
            crate::localization::Locale::Ko => "ko",
            crate::localization::Locale::Ca => "ca",
            crate::localization::Locale::De => "de",
            crate::localization::Locale::Fr => "fr",
            crate::localization::Locale::Id => "id",
            crate::localization::Locale::Hi => "hi",
            crate::localization::Locale::Ru => "ru",
            crate::localization::Locale::Uk => "uk",
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
        "provider" => Some(app.provider_identity_for_persistence().to_string()),
        "approval_mode" | "approval" => Some(app.approval_mode.permission_chip_label().to_string()),
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
        // `/config title` reports the config-level default, not a session's
        // `/title` override. The latter is intentionally a separate setting
        // and is reported by bare `/title`.
        "title" | "window_title" | "tab_title" => Some(
            app.title_default
                .clone()
                .unwrap_or_else(|| "(unset)".to_string()),
        ),
        "provider_url" | "provider_base_url" | "endpoint" => {
            let config = match Config::load(app.config_path.clone(), app.config_profile.as_deref())
            {
                Ok(mut config) => {
                    config.provider = Some(app.provider_identity_for_persistence().to_string());
                    config
                }
                Err(err) => {
                    return CommandResult::error(format!("Failed to load config: {err}"));
                }
            };
            Some(config.deepseek_base_url())
        }
        "context_window" | "context_window_tokens" => Some(format!(
            "{} (effective {} from {})",
            config_context_window_override(app)
                .map_or_else(|| "not set".to_string(), |tokens| tokens.to_string()),
            crate::route_budget::route_context_window_tokens(
                app.api_provider,
                app.effective_model_for_budget(),
                app.active_route_limits,
            ),
            app.active_context_window_source.display_label(),
        )),
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
        "thinking_default_expanded" | "thinking_expanded" => Some(
            if app.thinking_default_expanded {
                "true"
            } else {
                "false"
            }
            .to_string(),
        ),
        "thinking_preview_lines" | "thinking_preview" => {
            Some(app.thinking_preview_lines.to_string())
        }
        "help_expand_groups" | "help_expanded" => Some(
            if app.help_expand_groups {
                "true"
            } else {
                "false"
            }
            .to_string(),
        ),
        "pin_last_prompt" | "pin_prompt" => {
            Some(if app.pin_last_prompt { "true" } else { "false" }.to_string())
        }
        "thinking_highlight" | "reasoning_highlight" => Some(
            if app.thinking_highlight {
                "true"
            } else {
                "false"
            }
            .to_string(),
        ),
        "show_tool_details" | "tool_details" => Some(
            if app.show_tool_details {
                "true"
            } else {
                "false"
            }
            .to_string(),
        ),
        "inline_diffs" | "inline_diff" | "diffs" => {
            Some(app.inline_diff_mode.as_setting().to_string())
        }
        "mode" | "default_mode" => Some(app.mode.as_setting().to_string()),
        "max_history" | "history" => Some(app.max_input_history.to_string()),
        "work_surface_placement" | "work_surface" | "work_rail" => {
            Some(app.work_surface.placement.as_setting().to_string())
        }
        "rail_panel" | "rail" => Some(app.work_surface.panel.as_setting().to_string()),
        "work_surface_top_height" | "work_top_height" => {
            Some(app.work_surface.top_height.to_string())
        }
        "work_surface_side_width" | "work_side_width" => {
            Some(app.work_surface.side_width.to_string())
        }
        "tool_collapse" | "tool_collapse_mode" | "collapse" => {
            Some(app.tool_collapse_mode.as_setting().to_string())
        }
        "context_panel" | "context" | "session_panel" => {
            Some(if app.context_panel { "true" } else { "false" }.to_string())
        }
        "sessions_rail" | "sessions_panel" | "session_rail" => {
            Some(if app.sessions_rail { "true" } else { "false" }.to_string())
        }
        // Read the persisted value rather than reporting a hard-coded default:
        // this setting is consumed at startup by `main`, so `App` has no live
        // copy, and printing "false" unconditionally would misreport a user who
        // has it on.
        "session_auto_resume" | "auto_resume" => Some(
            if crate::settings::Settings::load_persisted()
                .map(|settings| settings.session_auto_resume)
                .unwrap_or(false)
            {
                "true"
            } else {
                "false"
            }
            .to_string(),
        ),
        "composer_density" | "composer" => Some(density_display(app.composer_density).to_string()),
        "composer_border" | "border" => {
            Some(if app.composer_border { "true" } else { "false" }.to_string())
        }
        "composer_multiline_mode" | "multiline_mode" | "multiline" => Some(
            if app.composer_multiline_mode {
                "true"
            } else {
                "false"
            }
            .to_string(),
        ),
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
        "workspace_follow_symlinks" | "follow_symlinks" => Settings::load().ok().map(|settings| {
            format!(
                "{} (restart required for engine tools)",
                settings.workspace_follow_symlinks
            )
        }),
        "search" | "search.provider" | "search_provider" => load_command_config(app)
            .ok()
            .map(|config| search_provider_display(&config, app.ui_locale)),
        "telemetry" => load_command_config(app)
            .ok()
            .map(|config| crate::telemetry_notice::saved_preference_enabled(&config).to_string()),
        "prompt_suggestion" => load_command_config(app)
            .ok()
            .map(|config| prompt_suggestion_display(&config)),
        "notifications" => load_command_config(app)
            .ok()
            .map(|config| notifications_summary(&config)),
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

/// Open the typed settings editor. `text` preserves the legacy diagnostic
/// output for scripts and terminals that cannot render the modal.
pub fn settings_command(app: &mut App, arg: Option<&str>) -> CommandResult {
    match arg.map(str::trim).filter(|value| !value.is_empty()) {
        None => CommandResult::action(AppAction::OpenConfigView),
        Some("text" | "show" | "diagnostic" | "diagnostics") => show_settings(app),
        Some(_) => CommandResult::error("Usage: /settings [text]"),
    }
}

/// Show persistent settings as plain text (legacy compatibility path).
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

/// `/fullscreen` and `/inline`: move the TUI between the alternate screen and
/// a full-height inline viewport.
///
/// The terminal transition happens where the ratatui terminal lives — this
/// only emits the action, so a switch the terminal refuses can roll back and
/// explain itself there.
pub fn screen(app: &mut App, target: ScreenMode, arg: Option<&str>) -> CommandResult {
    if let Some(extra) = arg.map(str::trim).filter(|value| !value.is_empty()) {
        return CommandResult::error(format!(
            "/{} takes no argument (got {extra:?}). Use /fullscreen or /inline.",
            target.as_str()
        ));
    }
    if target == app.screen_mode {
        return CommandResult::message(match target {
            ScreenMode::Fullscreen => {
                "Already on the fullscreen screen (alternate screen). /inline keeps the terminal's own scrollback instead."
            }
            ScreenMode::Inline => {
                "Already inline: a full-height viewport with no alternate screen, so this terminal's scrollback survives the session. /fullscreen returns to the alternate screen."
            }
        });
    }
    CommandResult::action(AppAction::SetScreenMode(target))
}

/// Place the workbar or pick its panel.
///
/// `/workbar bottom|top|left|right|off` sets placement; `/workbar
/// tasks|agents|context|pinned` picks the panel. The two are orthogonal:
/// where the workbar sits and what it shows. `/rail` and `/sidebar` remain
/// registered as the aliases users know.
/// Bare `/workbar` reports the workbar's *actual* rendered state — never a
/// claim about a surface that cannot render.
pub fn sidebar(app: &mut App, arg: Option<&str>) -> CommandResult {
    const USAGE: &str =
        "Usage: /workbar [bottom|top|left|right|off|tasks|agents|context|pinned] [--save]";
    let raw = arg.map(str::trim).unwrap_or("");
    let mut tokens = raw.split_whitespace().collect::<Vec<_>>();
    let persist = matches!(tokens.last(), Some(&"--save" | &"-s"));
    if persist {
        tokens.pop();
    }

    match tokens.as_slice() {
        [] => return CommandResult::message(rail_status_message(app)),
        [value] => {
            let value = value.to_ascii_lowercase();
            // Legacy focus words map onto the closest workbar concept so muscle
            // memory keeps working: "on" restores the default bottom workbar,
            // "off" hides it, panel names select panels.
            let placement = match value.as_str() {
                "top" => Some(crate::tui::work_surface::WorkSurfacePlacement::Top),
                "bottom" | "on" | "show" | "visible" => {
                    Some(crate::tui::work_surface::WorkSurfacePlacement::Bottom)
                }
                "left" => Some(crate::tui::work_surface::WorkSurfacePlacement::Left),
                "right" => Some(crate::tui::work_surface::WorkSurfacePlacement::Right),
                "off" | "hide" | "hidden" | "closed" | "none" => {
                    Some(crate::tui::work_surface::WorkSurfacePlacement::Off)
                }
                _ => None,
            };
            let panel = match value.as_str() {
                "tasks" | "activity" | "live" | "running" | "pinned" | "work" | "plan"
                | "todos" => Some(crate::tui::work_surface::RailPanel::Tasks),
                "agents" | "subagents" | "sub-agents" => {
                    Some(crate::tui::work_surface::RailPanel::Agents)
                }
                "background" | "shells" | "jobs" => {
                    Some(crate::tui::work_surface::RailPanel::Background)
                }
                "files" | "changes" => Some(crate::tui::work_surface::RailPanel::Files),
                "notepad" | "notes" => Some(crate::tui::work_surface::RailPanel::Notepad),
                "context" | "session" => Some(crate::tui::work_surface::RailPanel::Context),
                "git" | "branch" => Some(crate::tui::work_surface::RailPanel::Git),
                "price" | "cost" => Some(crate::tui::work_surface::RailPanel::Price),
                _ => None,
            };
            match (placement, panel) {
                (Some(placement), None) => {
                    app.work_surface.placement = placement;
                    app.work_surface.focused = false;
                    if persist {
                        let result = set_config_value(
                            app,
                            "work_surface_placement",
                            placement.as_setting(),
                            true,
                        );
                        if result.is_error {
                            return result;
                        }
                    }
                }
                (None, Some(panel)) => {
                    crate::tui::work_surface::select_dock_panel(app, panel);
                    if persist {
                        let result = set_config_value(app, "rail_panel", panel.as_setting(), true);
                        if result.is_error {
                            return result;
                        }
                    }
                }
                _ => return CommandResult::error(USAGE),
            }
        }
        _ => return CommandResult::error(USAGE),
    }

    app.needs_redraw = true;
    CommandResult::message(rail_status_message(app))
}

/// Truthful workbar readout: the placement and panel that actually render,
/// with the narrow-terminal fallback and an empty-Tasks collapse spelled out.
/// Never claims a panel is visible when no workbar area was produced.
fn rail_status_message(app: &App) -> String {
    use crate::tui::work_surface::{RailPanel, WorkSurfacePlacement};

    let placement = app.work_surface.placement;
    if placement == WorkSurfacePlacement::Off {
        return "Workbar is off — no panel renders (/workbar bottom|top|left|right to show it)"
            .to_string();
    }
    let panel = app.work_surface.panel;
    let mut message = format!(
        "Workbar: {} placement, {} panel",
        placement.as_setting(),
        panel.title()
    );
    let effective = app.work_surface.effective_placement();
    if effective != placement && effective == WorkSurfacePlacement::Top {
        message.push_str(" — side placements need a wider terminal, showing top for now");
    }
    if app.work_surface.last_area.is_none() {
        if panel == RailPanel::Tasks {
            message.push_str(" (currently hidden — no work to show)");
        } else {
            message.push_str(" (renders next frame)");
        }
    }
    message
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

fn approval_mode_config_value(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Auto => "auto",
        ApprovalMode::Bypass => "bypass",
        ApprovalMode::Suggest => "on-request",
        ApprovalMode::Never => "never",
    }
}

fn is_ask_rules_config_token(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "ask-rules"
            | "ask_rules"
            | "askrules"
            | "rules"
            | "permission-rules"
            | "permission_rules"
            | "permissions"
    )
}

fn config_editability_audit(app: &App) -> CommandResult {
    let config = match load_command_config(app) {
        Ok(config) => config,
        Err(err) => return CommandResult::error(err),
    };
    let config_path = crate::config_persistence::config_toml_path(app.config_path.as_deref())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "(unresolved)".to_string());

    let mut provider_config = config.clone();
    provider_config.provider = Some(app.provider_identity_for_persistence().to_string());
    let model = if app.auto_model {
        "auto".to_string()
    } else {
        app.model.clone()
    };
    let saved_permission_posture = Settings::load()
        .ok()
        .and_then(|settings| settings.permission_posture)
        .unwrap_or_else(|| "(unset)".to_string());
    let configured_approval_policy = config
        .approval_policy
        .clone()
        .unwrap_or_else(|| "(unset)".to_string());
    let effective_permissions = if app.mode == AppMode::Plan {
        "Read Only"
    } else {
        app.approval_mode.permission_chip_label()
    };
    let search_audit_note = tr(app.ui_locale, MessageId::ConfigAuditSearchProvider);
    let prompt_audit_note = tr(app.ui_locale, MessageId::ConfigAuditPromptSuggestion);
    let notifications_audit_note = tr(app.ui_locale, MessageId::ConfigAuditNotifications);

    let rows = [
        (
            "provider",
            app.provider_identity_for_persistence().to_string(),
            "session",
            "/config provider <name>",
            "Switches the active provider now; edit provider in config.toml for startup default.",
        ),
        (
            "model",
            model,
            "session",
            "/config model <id|auto>",
            "Switches the active model now; use default_text_model in config.toml for startup default.",
        ),
        (
            "effective_permissions",
            effective_permissions.to_string(),
            "runtime",
            "Shift+Tab",
            "Shows the effective Act permission posture; Plan remains Read Only.",
        ),
        (
            "permission_posture",
            saved_permission_posture,
            "TUI settings",
            "Shift+Tab",
            "Saved in settings.toml and ignored when config/requirements manage approval policy.",
        ),
        (
            "approval_policy",
            configured_approval_policy,
            "persisted config",
            "/config approval_mode <auto|on-request|never> --save",
            "Top-level managed policy; Full Access is not a valid value here.",
        ),
        (
            "allow_shell",
            app.allow_shell.to_string(),
            "runtime+persisted",
            "/config allow_shell <true|false> --save",
            "Writes top-level allow_shell and applies to subsequent turns.",
        ),
        (
            "stream_chunk_timeout_secs",
            app.stream_chunk_timeout_secs.to_string(),
            "runtime+persisted",
            "/config stream_chunk_timeout_secs <0|1..3600> --save",
            "Writes [tui].stream_chunk_timeout_secs and updates the running stream timeout.",
        ),
        (
            "subagents.enabled",
            subagents_config_display_value(&config, "enabled"),
            "runtime+persisted",
            "/config subagents on|off --save",
            "Writes [subagents].enabled and updates subsequent sub-agent launches.",
        ),
        (
            "subagents.max_concurrent",
            subagents_config_display_value(&config, "max_concurrent"),
            "runtime+persisted",
            "/config subagents max_concurrent <n> --save",
            "Clamped with Config::max_subagents and written to [subagents].max_concurrent.",
        ),
        (
            "subagents.max_depth",
            subagents_config_display_value(&config, "max_depth"),
            "runtime+persisted",
            "/config subagents max_depth <n> --save",
            "Clamped to the configured spawn-depth ceiling.",
        ),
        (
            "subagents.launch_concurrency",
            subagents_config_display_value(&config, "launch_concurrency"),
            "runtime+persisted",
            "/config subagents launch_concurrency <n> --save",
            "Clamped to the resolved sub-agent concurrency cap.",
        ),
        (
            "subagents.api_timeout_secs",
            subagents_config_display_value(&config, "api_timeout_secs"),
            "runtime+persisted",
            "/config subagents api_timeout_secs <seconds> --save",
            "0 means the compiled default; non-zero values are clamped to the documented range.",
        ),
        (
            "subagents.heartbeat_timeout_secs",
            subagents_config_display_value(&config, "heartbeat_timeout_secs"),
            "runtime+persisted",
            "/config subagents heartbeat_timeout_secs <seconds> --save",
            "0 means the compiled default; non-zero values are clamped to the documented range.",
        ),
        (
            "base_url",
            config.deepseek_base_url(),
            "persisted restart",
            "/config base_url <url> --save",
            "Writes top-level base_url; model clients read it on startup.",
        ),
        (
            "providers.<active>.base_url",
            provider_config.deepseek_base_url(),
            "persisted restart",
            "/config provider_url <url> --save",
            "Writes the active provider table; model clients read it on startup.",
        ),
        (
            "providers.<active>.context_window",
            config_context_window_override(app)
                .map_or_else(|| "(unset)".to_string(), |tokens| tokens.to_string()),
            "persisted restart",
            "edit [providers.<active>] context_window = <tokens>",
            "Overrides compaction, context-pressure, header, and preflight input budgets; use 262144 to cap a 1M route to 256K.",
        ),
        (
            "effective_context_window",
            format!(
                "{} ({})",
                crate::route_budget::route_context_window_tokens(
                    app.api_provider,
                    app.effective_model_for_budget(),
                    app.active_route_limits,
                ),
                app.active_context_window_source.display_label(),
            ),
            "runtime",
            "/config context_window",
            "The shared resolved window used by every active-route budget surface.",
        ),
        (
            "mcp_config_path",
            app.mcp_config_path.display().to_string(),
            "persisted live reload",
            "/config mcp_config_path <path> --save",
            "Run /mcp reload to rebuild the live model-visible tool pool.",
        ),
        (
            "workspace_follow_symlinks",
            app.workspace_follow_symlinks.to_string(),
            "partial restart",
            "/config workspace_follow_symlinks <true|false> --save",
            "Updates TUI file completion now; engine tools require restart.",
        ),
        (
            "search.provider",
            search_provider_display(&config, app.ui_locale),
            "runtime+persisted",
            "/config search.provider <name> --save",
            search_audit_note.as_ref(),
        ),
        (
            "prompt_suggestion",
            prompt_suggestion_display(&config),
            "runtime+persisted",
            "/config prompt_suggestion <true|false> --save",
            prompt_audit_note.as_ref(),
        ),
        (
            "notifications",
            notifications_summary(&config),
            "runtime+persisted",
            "/config notifications <method|threshold_secs|quiet|completion_sound> <value> --save",
            notifications_audit_note.as_ref(),
        ),
        (
            "instructions",
            file_only_status(config.instructions.as_ref().map(|v| !v.is_empty())),
            "file-only restart",
            "edit config.toml",
            "Prompt layers are loaded before the first turn.",
        ),
        (
            "hooks",
            file_only_status(config.hooks.as_ref().map(|_| true)),
            "file-only",
            "edit config.toml",
            "Hook definitions are structured TOML, not a scalar runtime setting.",
        ),
        (
            "network",
            file_only_status(config.network.as_ref().map(|_| true)),
            "file-only",
            "edit config.toml",
            "Network policy is evaluated by tool dispatch and should be reviewed as TOML.",
        ),
        (
            "tools",
            file_only_status(config.tools.as_ref().map(|_| true)),
            "file-only restart",
            "edit config.toml",
            "Tool catalog policy is built before model/tool negotiation.",
        ),
        (
            "memory",
            file_only_status(config.memory.as_ref().map(|_| true)),
            "file-only restart",
            "edit config.toml",
            "Memory loading changes prompt context and is resolved at startup.",
        ),
        (
            "runtime_api",
            file_only_status(config.runtime_api.as_ref().map(|_| true)),
            "file-only restart",
            "edit config.toml",
            "Serve/API tuning belongs to the runtime server startup path.",
        ),
        (
            "vision_model",
            file_only_status(config.vision_model.as_ref().map(|_| true)),
            "file-only restart",
            "edit config.toml",
            "Image-analysis provider clients are configured outside the scalar /config editor.",
        ),
    ];

    let mut lines = Vec::new();
    lines.push("Config editability audit".to_string());
    lines.push(format!("Config path: {config_path}"));
    lines.push("Key | Current | Editability | Command / reason".to_string());
    for (key, current, editability, command, note) in rows {
        lines.push(format!("{key} | {current} | {editability} | {command}"));
        lines.push(format!("  {note}"));
    }
    CommandResult::message(lines.join("\n"))
}

fn file_only_status(configured: Option<bool>) -> String {
    match configured {
        Some(true) => "configured".to_string(),
        Some(false) => "empty".to_string(),
        None => "unset".to_string(),
    }
}

fn search_provider_display(config: &Config, locale: crate::localization::Locale) -> String {
    let resolved = config.search_provider_resolution();
    let source = match resolved.source {
        SearchProviderSource::Default => tr(locale, MessageId::ConfigDefaultValue)
            .trim_matches(&['(', ')'][..])
            .to_string(),
        SearchProviderSource::Config => "config.toml".to_string(),
        SearchProviderSource::EnvOverride => "CODEWHALE_SEARCH_PROVIDER".to_string(),
    };
    tr(locale, MessageId::ConfigCommandSource)
        .replace("{value}", resolved.provider.as_str())
        .replace("{source}", &source)
}

fn prompt_suggestion_display(config: &Config) -> String {
    config.prompt_suggestion_enabled().to_string()
}

fn notifications_for_edit(config: &Config) -> NotificationsConfig {
    config.notifications_config()
}

fn notifications_summary(config: &Config) -> String {
    let notifications = notifications_for_edit(config);
    format!(
        "method={} threshold={}s sound={} quiet={}",
        notifications.method.as_str(),
        notifications.threshold_secs,
        notifications.completion_sound.as_str(),
        notifications.quiet
    )
}

fn search_config_command(app: &mut App, raw: &str) -> CommandResult {
    let mut tokens = raw.split_whitespace().collect::<Vec<_>>();
    let persist = matches!(tokens.last(), Some(&"--save" | &"-s"));
    if persist {
        tokens.pop();
    }

    match tokens.as_slice() {
        [] | ["status"] | ["provider"] => show_single_setting(app, "search.provider"),
        ["provider", value] | [value] => set_search_provider(app, value, persist),
        _ => CommandResult::error(format!(
            "{} /config search.provider <{}> [--save]",
            tr(app.ui_locale, MessageId::HelpUsageLabel),
            SearchProvider::names_hint()
        )),
    }
}

fn set_search_provider(app: &mut App, value: &str, persist: bool) -> CommandResult {
    let Some(provider) = SearchProvider::parse(value) else {
        return CommandResult::error(
            tr(app.ui_locale, MessageId::ConfigCommandInvalidValue)
                .replace("{key}", "search.provider")
                .replace("{value}", value)
                .replace("{choices}", SearchProvider::names_hint()),
        );
    };

    let scope = if persist {
        match persist_table_string_key(
            app.config_path.as_deref(),
            "search",
            "provider",
            provider.as_str(),
        ) {
            Ok(path) => format!(
                "{} {}",
                tr(app.ui_locale, MessageId::ConfigScopeSaved),
                path.display()
            ),
            Err(err) => {
                return CommandResult::error(
                    tr(app.ui_locale, MessageId::StartupDefaultNotSaved)
                        .replace("{setting}", "search.provider")
                        .replace("{error}", &err.to_string()),
                );
            }
        }
    } else {
        tr(app.ui_locale, MessageId::ConfigScopeSession).into_owned()
    };

    CommandResult::with_message_and_action(
        tr(app.ui_locale, MessageId::ConfigSearchUpdated)
            .replace("{value}", provider.as_str())
            .replace("{scope}", &scope),
        AppAction::UpdateSearchProvider { provider },
    )
}

fn set_prompt_suggestion(app: &mut App, value: &str, persist: bool) -> CommandResult {
    let enabled = match parse_config_bool(value) {
        Ok(enabled) => enabled,
        Err(_) => {
            return CommandResult::error(
                tr(app.ui_locale, MessageId::ConfigCommandInvalidValue)
                    .replace("{key}", "prompt_suggestion")
                    .replace("{value}", value)
                    .replace("{choices}", "on, off, true, false, yes, no"),
            );
        }
    };
    let scope = if persist {
        match persist_root_bool_key(app.config_path.as_deref(), "prompt_suggestion", enabled) {
            Ok(path) => format!(
                "{} {}",
                tr(app.ui_locale, MessageId::ConfigScopeSaved),
                path.display()
            ),
            Err(err) => {
                return CommandResult::error(
                    tr(app.ui_locale, MessageId::StartupDefaultNotSaved)
                        .replace("{setting}", "prompt_suggestion")
                        .replace("{error}", &err.to_string()),
                );
            }
        }
    } else {
        tr(app.ui_locale, MessageId::ConfigScopeSession).into_owned()
    };
    CommandResult::with_message_and_action(
        tr(app.ui_locale, MessageId::ConfigPromptSuggestionUpdated)
            .replace("{value}", &enabled.to_string())
            .replace("{scope}", &scope),
        AppAction::UpdatePromptSuggestion { enabled },
    )
}

fn notifications_config_command(app: &mut App, raw: &str) -> CommandResult {
    let mut tokens = raw.split_whitespace().collect::<Vec<_>>();
    let persist = matches!(tokens.last(), Some(&"--save" | &"-s"));
    if persist {
        tokens.pop();
    }

    match tokens.as_slice() {
        [] | ["status"] => show_notifications_status(app),
        [key] => show_notifications_setting(app, key),
        [key, value] => set_notifications_value(app, key, value, persist),
        _ => CommandResult::error(format!(
            "{} /config notifications [status|method|threshold_secs|include_summary|quiet|completion_sound|subagent_completion <value>] [--save]",
            tr(app.ui_locale, MessageId::HelpUsageLabel)
        )),
    }
}

fn show_notifications_status(app: &App) -> CommandResult {
    let config = match load_command_config(app) {
        Ok(config) => config,
        Err(err) => return CommandResult::error(err),
    };
    let notifications = notifications_for_edit(&config);
    let lines = [
        "[notifications] — config.toml".to_string(),
        format!("method = {}", notifications.method.as_str()),
        format!("threshold_secs = {}", notifications.threshold_secs),
        format!("include_summary = {}", notifications.include_summary),
        format!("quiet = {}", notifications.quiet),
        format!(
            "completion_sound = {}",
            notifications.completion_sound.as_str()
        ),
        format!(
            "subagent_completion = {}",
            notifications.subagent_completion.as_str()
        ),
        String::new(),
        tr(app.ui_locale, MessageId::ConfigNotificationsSetHint).into_owned(),
    ];
    CommandResult::message(lines.join("\n"))
}

fn show_notifications_setting(app: &App, key: &str) -> CommandResult {
    let config = match load_command_config(app) {
        Ok(config) => config,
        Err(err) => return CommandResult::error(err),
    };
    let Some(key) = canonical_notifications_key(key) else {
        return CommandResult::error(
            tr(app.ui_locale, MessageId::ConfigCommandInvalidValue)
                .replace("{key}", "notifications")
                .replace("{value}", key)
                .replace("{choices}", "/config notifications status"),
        );
    };
    let notifications = notifications_for_edit(&config);
    let value = notifications_field_display(&notifications, key);
    CommandResult::message(format!("notifications.{key} = {value}"))
}

fn set_notifications_value(app: &mut App, key: &str, value: &str, persist: bool) -> CommandResult {
    let Some(key) = canonical_notifications_key(key) else {
        return CommandResult::error(
            tr(app.ui_locale, MessageId::ConfigCommandInvalidValue)
                .replace("{key}", "notifications")
                .replace("{value}", key)
                .replace("{choices}", "/config notifications status"),
        );
    };

    let (update, save_result) = match key {
        "method" => {
            let Some(method) = NotificationMethod::parse(value) else {
                return CommandResult::error(
                    tr(app.ui_locale, MessageId::ConfigCommandInvalidValue)
                        .replace("{key}", "notifications.method")
                        .replace("{value}", value)
                        .replace("{choices}", NotificationMethod::names_hint()),
                );
            };
            (
                NotificationConfigUpdate::Method(method),
                persist.then(|| {
                    persist_table_string_key(
                        app.config_path.as_deref(),
                        "notifications",
                        "method",
                        method.as_str(),
                    )
                }),
            )
        }
        "threshold_secs" => {
            let threshold = match value.trim().parse::<u64>() {
                Ok(threshold) => threshold,
                Err(_) => {
                    return CommandResult::error(
                        tr(app.ui_locale, MessageId::ConfigNotificationsWholeNumber).into_owned(),
                    );
                }
            };
            (
                NotificationConfigUpdate::ThresholdSecs(threshold),
                persist.then(|| {
                    persist_table_integer_key(
                        app.config_path.as_deref(),
                        "notifications",
                        "threshold_secs",
                        threshold,
                    )
                }),
            )
        }
        "include_summary" => {
            let enabled = match parse_config_bool(value) {
                Ok(enabled) => enabled,
                Err(_) => {
                    return CommandResult::error(
                        tr(app.ui_locale, MessageId::ConfigCommandInvalidValue)
                            .replace("{key}", "notifications.include_summary")
                            .replace("{value}", value)
                            .replace("{choices}", "on, off, true, false, yes, no"),
                    );
                }
            };
            (
                NotificationConfigUpdate::IncludeSummary(enabled),
                persist.then(|| {
                    persist_table_bool_key(
                        app.config_path.as_deref(),
                        "notifications",
                        "include_summary",
                        enabled,
                    )
                }),
            )
        }
        "quiet" => {
            let enabled = match parse_config_bool(value) {
                Ok(enabled) => enabled,
                Err(_) => {
                    return CommandResult::error(
                        tr(app.ui_locale, MessageId::ConfigCommandInvalidValue)
                            .replace("{key}", "notifications.quiet")
                            .replace("{value}", value)
                            .replace("{choices}", "on, off, true, false, yes, no"),
                    );
                }
            };
            (
                NotificationConfigUpdate::Quiet(enabled),
                persist.then(|| {
                    persist_table_bool_key(
                        app.config_path.as_deref(),
                        "notifications",
                        "quiet",
                        enabled,
                    )
                }),
            )
        }
        "completion_sound" => {
            let Some(sound) = CompletionSound::parse(value) else {
                return CommandResult::error(
                    tr(app.ui_locale, MessageId::ConfigCommandInvalidValue)
                        .replace("{key}", "notifications.completion_sound")
                        .replace("{value}", value)
                        .replace("{choices}", CompletionSound::names_hint()),
                );
            };
            (
                NotificationConfigUpdate::CompletionSound(sound),
                persist.then(|| {
                    persist_table_string_key(
                        app.config_path.as_deref(),
                        "notifications",
                        "completion_sound",
                        sound.as_str(),
                    )
                }),
            )
        }
        "subagent_completion" => {
            let Some(mode) = SubagentCompletionNotification::parse(value) else {
                return CommandResult::error(
                    tr(app.ui_locale, MessageId::ConfigCommandInvalidValue)
                        .replace("{key}", "notifications.subagent_completion")
                        .replace("{value}", value)
                        .replace("{choices}", SubagentCompletionNotification::names_hint()),
                );
            };
            (
                NotificationConfigUpdate::SubagentCompletion(mode),
                persist.then(|| {
                    persist_table_string_key(
                        app.config_path.as_deref(),
                        "notifications",
                        "subagent_completion",
                        mode.as_str(),
                    )
                }),
            )
        }
        _ => unreachable!("canonical notifications key"),
    };

    let scope = if let Some(result) = save_result {
        match result {
            Ok(path) => format!(
                "{} {}",
                tr(app.ui_locale, MessageId::ConfigScopeSaved),
                path.display()
            ),
            Err(err) => {
                return CommandResult::error(
                    tr(app.ui_locale, MessageId::StartupDefaultNotSaved)
                        .replace("{setting}", &format!("notifications.{key}"))
                        .replace("{error}", &err.to_string()),
                );
            }
        }
    } else {
        tr(app.ui_locale, MessageId::ConfigScopeSession).into_owned()
    };

    let display_value = notification_update_display(update);
    CommandResult::with_message_and_action(
        tr(app.ui_locale, MessageId::ConfigNotificationUpdated)
            .replace("{key}", key)
            .replace("{value}", &display_value)
            .replace("{scope}", &scope),
        AppAction::UpdateNotification { update },
    )
}

fn notification_update_display(update: NotificationConfigUpdate) -> String {
    match update {
        NotificationConfigUpdate::Method(value) => value.as_str().to_string(),
        NotificationConfigUpdate::ThresholdSecs(value) => value.to_string(),
        NotificationConfigUpdate::IncludeSummary(value) => value.to_string(),
        NotificationConfigUpdate::Quiet(value) => value.to_string(),
        NotificationConfigUpdate::CompletionSound(value) => value.as_str().to_string(),
        NotificationConfigUpdate::SubagentCompletion(value) => value.as_str().to_string(),
    }
}

fn canonical_notifications_key(key: &str) -> Option<&'static str> {
    match key.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "method" => Some("method"),
        "threshold_secs" | "threshold" => Some("threshold_secs"),
        "include_summary" | "summary" => Some("include_summary"),
        "quiet" => Some("quiet"),
        "completion_sound" | "sound" => Some("completion_sound"),
        "subagent_completion" => Some("subagent_completion"),
        _ => None,
    }
}

fn notifications_field_display(notifications: &NotificationsConfig, key: &str) -> String {
    match key {
        "method" => notifications.method.as_str().to_string(),
        "threshold_secs" => notifications.threshold_secs.to_string(),
        "include_summary" => notifications.include_summary.to_string(),
        "quiet" => notifications.quiet.to_string(),
        "completion_sound" => notifications.completion_sound.as_str().to_string(),
        "subagent_completion" => notifications.subagent_completion.as_str().to_string(),
        _ => unreachable!("canonical notifications key"),
    }
}

fn stream_chunk_timeout_value_label(raw: u64, resolved: u64) -> String {
    if raw == 0 {
        format!("0 (default {resolved})")
    } else {
        resolved.to_string()
    }
}

fn subagents_config_command(app: &mut App, raw: &str) -> CommandResult {
    let mut tokens = raw.split_whitespace().collect::<Vec<_>>();
    let persist = matches!(tokens.last(), Some(&"--save" | &"-s"));
    if persist {
        tokens.pop();
    }

    match tokens.as_slice() {
        [] | ["status"] => subagents_status(app),
        ["on"] | ["enable"] | ["enabled"] => {
            set_subagents_config_value(app, "enabled", "true", persist)
        }
        ["off"] | ["disable"] | ["disabled"] => {
            set_subagents_config_value(app, "enabled", "false", persist)
        }
        [key] => show_subagents_setting(app, key),
        [key, value] => set_subagents_config_value(app, key, value, persist),
        _ => CommandResult::error(
            "Usage: /config subagents [status|on|off|enabled|max_concurrent|max_depth|launch_concurrency|api_timeout_secs|heartbeat_timeout_secs <value>] [--save]",
        ),
    }
}

fn load_command_config(app: &App) -> Result<Config, String> {
    Config::load(app.config_path.clone(), app.config_profile.as_deref())
        .map_err(|err| format!("Failed to load config: {err}"))
}

fn subagents_status(app: &App) -> CommandResult {
    let config = match load_command_config(app) {
        Ok(config) => config,
        Err(err) => return CommandResult::error(err),
    };
    let path = crate::config_persistence::config_toml_path(app.config_path.as_deref())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "(unresolved)".to_string());
    let disabled_reason = config.subagents_disabled_reason();
    let active_provider = app.api_provider;
    let subagents = config.subagents.as_ref();
    let provider_subagents = config.subagent_provider_config(active_provider);
    let explicit_enabled = subagents.and_then(|cfg| cfg.enabled);
    let raw_max_concurrent = subagents.and_then(|cfg| cfg.max_concurrent);
    let raw_max_depth = subagents.and_then(|cfg| cfg.max_depth);
    let raw_launch = subagents.and_then(|cfg| cfg.launch_concurrency);
    let raw_api = subagents.and_then(|cfg| cfg.api_timeout_secs);
    let raw_heartbeat = subagents.and_then(|cfg| cfg.heartbeat_timeout_secs);
    let mut lines = Vec::new();
    lines.push(format!(
        "Sub-agents: {}",
        disabled_reason
            .map(|reason| format!("disabled ({reason})"))
            .unwrap_or_else(|| "enabled".to_string())
    ));
    lines.push(format!("Config path: {path}"));
    lines.push(format!(
        "Active provider: {} ({})",
        active_provider.as_str(),
        active_provider.display_name()
    ));
    lines.push(format!(
        "subagents.enabled = {}",
        explicit_enabled
            .map(|value| value.to_string())
            .unwrap_or_else(|| "default true".to_string())
    ));
    lines.push(format!(
        "subagents.max_concurrent = {} (resolved global {}; active provider {})",
        option_display(raw_max_concurrent),
        config.max_subagents(),
        config.max_subagents_for_provider(active_provider)
    ));
    lines.push(format!(
        "subagents.max_depth = {} (resolved global {}; active provider {})",
        option_display(raw_max_depth),
        config.subagent_max_spawn_depth(),
        config.subagent_max_spawn_depth_for_provider(active_provider)
    ));
    lines.push(format!(
        "subagents.launch_concurrency = {} (resolved global {}; active provider {})",
        option_display(raw_launch),
        config.launch_concurrency(),
        config.launch_concurrency_for_provider(active_provider)
    ));
    lines.push(format!(
        "subagents.api_timeout_secs = {} (resolved global {}; active provider {})",
        option_display(raw_api),
        config.subagent_api_timeout_secs(),
        config.subagent_api_timeout_secs_for_provider(active_provider)
    ));
    lines.push(format!(
        "subagents.heartbeat_timeout_secs = {} (resolved global {}; active provider {})",
        option_display(raw_heartbeat),
        config.subagent_heartbeat_timeout_secs(),
        config.subagent_heartbeat_timeout_secs_for_provider(active_provider)
    ));
    if let Some(provider_subagents) = provider_subagents {
        lines.push(format!(
            "subagents.providers.{}.enabled = {}",
            active_provider.as_str(),
            provider_subagents
                .enabled
                .map(|value| value.to_string())
                .unwrap_or_else(|| "inherits".to_string())
        ));
        lines.push(format!(
            "subagents.providers.{}.max_concurrent = {}",
            active_provider.as_str(),
            option_display(provider_subagents.max_concurrent)
        ));
        lines.push(format!(
            "subagents.providers.{}.max_depth = {}",
            active_provider.as_str(),
            option_display(provider_subagents.max_depth)
        ));
        lines.push(format!(
            "subagents.providers.{}.launch_concurrency = {}",
            active_provider.as_str(),
            option_display(provider_subagents.launch_concurrency)
        ));
        lines.push(format!(
            "subagents.providers.{}.max_admitted = {}",
            active_provider.as_str(),
            option_display(provider_subagents.max_admitted)
        ));
    } else {
        lines.push(format!(
            "subagents.providers.{} = inherits global",
            active_provider.as_str()
        ));
    }
    CommandResult::message(lines.join("\n"))
}

fn show_subagents_setting(app: &App, key: &str) -> CommandResult {
    let config = match load_command_config(app) {
        Ok(config) => config,
        Err(err) => return CommandResult::error(err),
    };
    let Some(key) = canonical_subagents_key(key) else {
        return CommandResult::error(format!(
            "Unknown subagents setting '{key}'. Use `/config subagents status`."
        ));
    };
    let active_provider = app.api_provider;
    let subagents = config.subagents.as_ref();
    let value = match key {
        "enabled" => subagents
            .and_then(|cfg| cfg.enabled)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "default true".to_string()),
        "max_concurrent" => format!(
            "{} (resolved global {}; active provider {})",
            option_display(subagents.and_then(|cfg| cfg.max_concurrent)),
            config.max_subagents(),
            config.max_subagents_for_provider(active_provider)
        ),
        "max_depth" => format!(
            "{} (resolved global {}; active provider {})",
            option_display(subagents.and_then(|cfg| cfg.max_depth)),
            config.subagent_max_spawn_depth(),
            config.subagent_max_spawn_depth_for_provider(active_provider)
        ),
        "launch_concurrency" => format!(
            "{} (resolved global {}; active provider {})",
            option_display(subagents.and_then(|cfg| cfg.launch_concurrency)),
            config.launch_concurrency(),
            config.launch_concurrency_for_provider(active_provider)
        ),
        "api_timeout_secs" => format!(
            "{} (resolved global {}; active provider {})",
            option_display(subagents.and_then(|cfg| cfg.api_timeout_secs)),
            config.subagent_api_timeout_secs(),
            config.subagent_api_timeout_secs_for_provider(active_provider)
        ),
        "heartbeat_timeout_secs" => format!(
            "{} (resolved global {}; active provider {})",
            option_display(subagents.and_then(|cfg| cfg.heartbeat_timeout_secs)),
            config.subagent_heartbeat_timeout_secs(),
            config.subagent_heartbeat_timeout_secs_for_provider(active_provider)
        ),
        _ => unreachable!("canonical subagent key"),
    };
    CommandResult::message(format!("subagents.{key} = {value}"))
}

fn option_display<T: std::fmt::Display>(value: Option<T>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "default".to_string())
}

fn canonical_subagents_key(key: &str) -> Option<&'static str> {
    let normalized = key.trim().to_ascii_lowercase();
    let key = normalized
        .strip_prefix("subagents.")
        .unwrap_or(normalized.as_str());
    match key {
        "enabled" | "enable" => Some("enabled"),
        "max_concurrent" | "max_subagents" | "concurrency" | "cap" => Some("max_concurrent"),
        "max_depth" | "depth" | "spawn_depth" => Some("max_depth"),
        "launch_concurrency" | "launches" | "launch" => Some("launch_concurrency"),
        "api_timeout_secs" | "api_timeout" | "step_timeout_secs" => Some("api_timeout_secs"),
        "heartbeat_timeout_secs" | "heartbeat_timeout" | "heartbeat" => {
            Some("heartbeat_timeout_secs")
        }
        _ => None,
    }
}

fn set_subagents_config_value(
    app: &mut App,
    key: &str,
    value: &str,
    persist: bool,
) -> CommandResult {
    let Some(key) = canonical_subagents_key(key) else {
        return CommandResult::error(format!(
            "Unknown subagents setting '{key}'. Use `/config subagents status`."
        ));
    };
    let mut config = match load_command_config(app) {
        Ok(config) => config,
        Err(err) => return CommandResult::error(err),
    };
    let current_max_subagents = config.max_subagents() as u64;
    let subagents = config
        .subagents
        .get_or_insert_with(SubagentsConfig::default);

    let mut note = None;
    let save_result = match key {
        "enabled" => {
            let enabled = match parse_config_bool(value) {
                Ok(enabled) => enabled,
                Err(err) => return CommandResult::error(err),
            };
            subagents.enabled = Some(enabled);
            if persist {
                Some(persist_subagents_bool_key(
                    app.config_path.as_deref(),
                    "enabled",
                    enabled,
                ))
            } else {
                None
            }
        }
        "max_concurrent" => {
            let raw = match parse_subagents_u64(key, value) {
                Ok(raw) => raw,
                Err(err) => return CommandResult::error(err),
            };
            let clamped = raw.min(MAX_SUBAGENTS as u64);
            if clamped != raw {
                note = Some(format!("clamped from {raw} to {clamped}"));
            }
            subagents.max_concurrent = Some(clamped as usize);
            if persist {
                Some(persist_subagents_integer_key(
                    app.config_path.as_deref(),
                    "max_concurrent",
                    clamped,
                ))
            } else {
                None
            }
        }
        "max_depth" => {
            let raw = match parse_subagents_u64(key, value) {
                Ok(raw) => raw,
                Err(err) => return CommandResult::error(err),
            };
            let ceiling = u64::from(codewhale_config::MAX_SPAWN_DEPTH_CEILING);
            let clamped = raw.min(ceiling);
            if clamped != raw {
                note = Some(format!("clamped from {raw} to {clamped}"));
            }
            subagents.max_depth = Some(clamped as u32);
            if persist {
                Some(persist_subagents_integer_key(
                    app.config_path.as_deref(),
                    "max_depth",
                    clamped,
                ))
            } else {
                None
            }
        }
        "launch_concurrency" => {
            let raw = match parse_subagents_u64(key, value) {
                Ok(raw) => raw,
                Err(err) => return CommandResult::error(err),
            };
            let clamped = raw.clamp(1, current_max_subagents);
            if clamped != raw {
                note = Some(format!("clamped from {raw} to {clamped}"));
            }
            subagents.launch_concurrency = Some(clamped as usize);
            if persist {
                Some(persist_subagents_integer_key(
                    app.config_path.as_deref(),
                    "launch_concurrency",
                    clamped,
                ))
            } else {
                None
            }
        }
        "api_timeout_secs" => {
            let raw = match parse_subagents_u64(key, value) {
                Ok(raw) => raw,
                Err(err) => return CommandResult::error(err),
            };
            let stored = if raw == 0 {
                0
            } else {
                raw.clamp(MIN_SUBAGENT_API_TIMEOUT_SECS, MAX_SUBAGENT_API_TIMEOUT_SECS)
            };
            if stored != raw {
                note = Some(format!("clamped from {raw} to {stored}"));
            }
            subagents.api_timeout_secs = Some(stored);
            if persist {
                Some(persist_subagents_integer_key(
                    app.config_path.as_deref(),
                    "api_timeout_secs",
                    stored,
                ))
            } else {
                None
            }
        }
        "heartbeat_timeout_secs" => {
            let raw = match parse_subagents_u64(key, value) {
                Ok(raw) => raw,
                Err(err) => return CommandResult::error(err),
            };
            let stored = if raw == 0 {
                0
            } else {
                raw.clamp(
                    MIN_SUBAGENT_HEARTBEAT_TIMEOUT_SECS,
                    MAX_SUBAGENT_HEARTBEAT_TIMEOUT_SECS,
                )
            };
            if stored != raw {
                note = Some(format!("clamped from {raw} to {stored}"));
            }
            subagents.heartbeat_timeout_secs = Some(stored);
            if persist {
                Some(persist_subagents_integer_key(
                    app.config_path.as_deref(),
                    "heartbeat_timeout_secs",
                    stored,
                ))
            } else {
                None
            }
        }
        _ => unreachable!("canonical subagent key"),
    };

    let save_suffix = if let Some(result) = save_result {
        match result {
            Ok(path) => format!("saved to {}", path.display()),
            Err(err) => return CommandResult::error(format!("Failed to save: {err}")),
        }
    } else {
        "session only, add --save to persist".to_string()
    };

    if key == "max_concurrent" {
        app.max_subagents = config.max_subagents_for_provider(app.api_provider);
    }
    let display_value = subagents_config_display_value(&config, key);
    let note = note.map(|note| format!("; {note}")).unwrap_or_default();
    CommandResult::with_message_and_action(
        format!(
            "subagents.{key} = {display_value} ({save_suffix}; runtime updated for subsequent turns{note})"
        ),
        subagents_runtime_action(app, &config),
    )
}

fn parse_subagents_u64(key: &str, value: &str) -> Result<u64, String> {
    value
        .trim()
        .parse::<u64>()
        .map_err(|_| format!("subagents.{key} must be a whole number"))
}

fn subagents_config_display_value(config: &Config, key: &str) -> String {
    let subagents = config.subagents.as_ref();
    match key {
        "enabled" => subagents
            .and_then(|cfg| cfg.enabled)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "default true".to_string()),
        "max_concurrent" => {
            if subagents.and_then(|cfg| cfg.max_concurrent) == Some(0) {
                "0 (disabled)".to_string()
            } else {
                config.max_subagents().to_string()
            }
        }
        "max_depth" => {
            if subagents.and_then(|cfg| cfg.max_depth) == Some(0) {
                "0 (agent tool disabled)".to_string()
            } else {
                config.subagent_max_spawn_depth().to_string()
            }
        }
        "launch_concurrency" => config.launch_concurrency().to_string(),
        "api_timeout_secs" => {
            let raw = subagents.and_then(|cfg| cfg.api_timeout_secs);
            if raw == Some(0) {
                format!("0 (default {DEFAULT_SUBAGENT_API_TIMEOUT_SECS})")
            } else {
                config.subagent_api_timeout_secs().to_string()
            }
        }
        "heartbeat_timeout_secs" => {
            let raw = subagents.and_then(|cfg| cfg.heartbeat_timeout_secs);
            if raw == Some(0) {
                format!("0 (default {DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS})")
            } else {
                config.subagent_heartbeat_timeout_secs().to_string()
            }
        }
        _ => unreachable!("canonical subagent key"),
    }
}

fn subagents_runtime_action(app: &App, config: &Config) -> AppAction {
    let provider = app.api_provider;
    let max_subagents = config
        .max_subagents_for_provider(provider)
        .clamp(1, MAX_SUBAGENTS);
    AppAction::UpdateSubagentRuntimeConfig {
        enabled: config.subagents_enabled_for_provider(provider),
        max_subagents,
        launch_concurrency: config.launch_concurrency_for_provider(provider),
        max_spawn_depth: config.subagent_max_spawn_depth_for_provider(provider),
        api_timeout_secs: config.subagent_api_timeout_secs_for_provider(provider),
        heartbeat_timeout_secs: config.subagent_heartbeat_timeout_secs_for_provider(provider),
    }
}

/// The subject a live-route key belongs to, or `None` if the key does not touch
/// the route the engine is currently acting on.
///
/// This is the single list the #2982 turn lock is enforced from. It exists
/// because the lock used to live in the *selectors* — the Tab cycle, the
/// pickers, the hotbar — while `/set <key> <value>` and `/config <key> <value>`
/// reached the same live state through a different door. A slash command is
/// reachable mid-turn (the composer accepts Shift+Enter and the slash menu while
/// `is_loading`), so during a running turn `/set model …` could swap the route
/// out from under the engine and persist it.
///
/// `default_mode` is deliberately absent: it is a restart default that
/// `set_config_value` explicitly does *not* apply to the live session, so
/// refusing it would lock a key that cannot affect the turn.
fn live_route_setting_subject(key: &str) -> Option<MessageId> {
    match key {
        "mode" => Some(MessageId::SettingSubjectMode),
        // `default_model` is not merely a startup default: for the DeepSeek
        // routes `set_config_value` installs it as the live model.
        "model" | "default_model" => Some(MessageId::SettingSubjectModel),
        "reasoning_effort" | "effort" => Some(MessageId::SettingSubjectThinking),
        "provider" => Some(MessageId::SettingSubjectProvider),
        "approval_mode" | "approval_policy" | "approval" => {
            Some(MessageId::SettingSubjectPermissions)
        }
        _ => None,
    }
}

/// Modify a setting at runtime
pub fn set_config_value(app: &mut App, key: &str, value: &str, persist: bool) -> CommandResult {
    let key = key.to_lowercase();
    if let Some(subagent_key) = key.strip_prefix("subagents.") {
        return set_subagents_config_value(app, subagent_key, value, persist);
    }
    if let Some(notifications_key) = key.strip_prefix("notifications.") {
        return set_notifications_value(app, notifications_key, value, persist);
    }

    // Refuse before *anything* — before the disk write, and before the live
    // `App` mutation each arm performs. Placing the check at the top is what
    // makes it central: every caller of this function (`/set`, `/config k v`,
    // the preset mirror, the schema-driven config editor, the runtime
    // `ConfigUpdated` event) inherits it, and none of them can half-apply.
    if let Some(subject) = live_route_setting_subject(key.as_str())
        && app.is_loading
    {
        return CommandResult::error(app.setting_locked_message(subject));
    }

    match key.as_str() {
        "telemetry" => {
            if !persist {
                return CommandResult::error(
                    "Telemetry is a durable privacy preference. Change it in /settings or add --save.",
                );
            }
            let enabled = match parse_config_bool(value) {
                Ok(enabled) => enabled,
                Err(err) => return CommandResult::error(err),
            };
            let applied = crate::telemetry_notice::apply_persistent_preference(
                app.config_path.clone(),
                enabled,
            );
            let message = applied.message(app.ui_locale);
            return if applied.is_error() {
                CommandResult {
                    message: Some(message),
                    action: None,
                    is_error: true,
                }
            } else {
                CommandResult::message(message)
            };
        }
        "model" => {
            // Support "/model auto" — auto-select model based on request complexity
            if value.trim().eq_ignore_ascii_case("auto") {
                app.set_model_selection("auto".to_string());
                app.update_model_compaction_budget();
                app.session.last_prompt_tokens = None;
                app.session.last_completion_tokens = None;
                app.session.last_output_throughput = None;
                return CommandResult::with_message_and_action(
                    format!(
                        "model = auto (auto-select model per turn; thinking = {})",
                        app.reasoning_effort_display_label()
                    ),
                    AppAction::UpdateCompaction(app.compaction_config()),
                );
            }
            // Route-aware: a custom DeepSeek (or other) endpoint owns its model
            // namespace. Provider-only normalization would reject a non-DeepSeek
            // id that the live session is already allowed to use via `/model`.
            // OpenCode Go stays protocol-strict even on a custom host.
            let model = if app.api_provider == ApiProvider::OpencodeGo {
                let Some(model) = normalize_model_name_for_provider(app.api_provider, value) else {
                    return CommandResult::error(format!(
                        "Invalid model '{value}' for provider {}.",
                        app.api_provider.as_str()
                    ));
                };
                if let Err(reason) = validate_route(app.api_provider, &model) {
                    return CommandResult::error(reason);
                }
                model
            } else if app.accepts_custom_model_ids() {
                let Some(model) = normalize_custom_model_id(value) else {
                    return CommandResult::error(format!(
                        "Invalid model '{value}' for provider {}.",
                        app.api_provider.as_str()
                    ));
                };
                model
            } else {
                let Some(model) = normalize_model_name_for_provider(app.api_provider, value) else {
                    return CommandResult::error(format!(
                        "Invalid model '{value}' for provider {}.",
                        app.api_provider.as_str()
                    ));
                };
                if let Err(reason) = validate_route(app.api_provider, &model) {
                    return CommandResult::error(reason);
                }
                model
            };
            app.set_model_selection(model.clone());
            app.fleet_roster_stale |= crate::fleet::members::auto_enroll_fleet_model(
                &app.workspace,
                app.provider_identity_for_persistence(),
                &model,
            );
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
        "approval_mode" | "approval_policy" | "approval" => {
            let use_tui_default = matches!(
                value
                    .trim()
                    .to_ascii_lowercase()
                    .replace([' ', '_'], "-")
                    .as_str(),
                "default" | "tui-default" | "use-tui-default"
            );
            if use_tui_default {
                if !persist {
                    return CommandResult::error(
                        "Removing the config approval override requires --save.",
                    );
                }
                let control = match load_command_config(app) {
                    Ok(config) => config.approval_policy_control(
                        app.config_path.as_deref(),
                        app.config_profile.as_deref(),
                        &app.workspace,
                    ),
                    Err(err) => return CommandResult::error(err),
                };
                if !matches!(
                    control,
                    crate::config::ApprovalPolicyControl::RootConfig
                        | crate::config::ApprovalPolicyControl::Unset
                ) {
                    return CommandResult::error(format!(
                        "Approval posture is controlled by {}; change that source first.",
                        control.label()
                    ));
                }
                return match persist_unset_root_key(app.config_path.as_deref(), "approval_policy") {
                    Ok(path) => {
                        let saved_mode = Settings::load_persisted()
                            .ok()
                            .and_then(|settings| settings.permission_posture)
                            .as_deref()
                            .and_then(ApprovalMode::from_config_value)
                            .unwrap_or(ApprovalMode::Suggest);
                        app.set_agent_approval_posture(saved_mode);
                        app.clear_saved_approval_policy_lock();
                        CommandResult::with_message_and_action(
                            format!(
                                "approval_policy removed from {}; new sessions use the TUI {} default",
                                path.display(),
                                saved_mode.permission_chip_label()
                            ),
                            AppAction::ApprovalPolicyPersisted { policy: None },
                        )
                    }
                    Err(err) => CommandResult::error(format!("Failed to save: {err}")),
                };
            }
            let control = match load_command_config(app) {
                Ok(config) => config.approval_policy_control(
                    app.config_path.as_deref(),
                    app.config_profile.as_deref(),
                    &app.workspace,
                ),
                Err(err) => return CommandResult::error(err),
            };
            let control_allows_change = if persist {
                control.editable_root()
            } else {
                matches!(control, crate::config::ApprovalPolicyControl::Unset)
            };
            if !control_allows_change {
                return CommandResult::error(format!(
                    "Approval posture is controlled by {}; {}.",
                    control.label(),
                    if matches!(control, crate::config::ApprovalPolicyControl::RootConfig) {
                        "save a new config value or choose Use TUI permission default"
                    } else {
                        "change that source first"
                    }
                ));
            }
            let mode = ApprovalMode::from_config_value(value);
            return match mode {
                Some(ApprovalMode::Bypass)
                    if persist
                        && matches!(control, crate::config::ApprovalPolicyControl::RootConfig) =>
                {
                    match app.adopt_root_approval_posture(ApprovalMode::Bypass) {
                        Ok(()) => CommandResult::with_message_and_action(
                            "approval_mode = Full Access (saved as the TUI permission posture; removed the root approval_policy override)",
                            AppAction::ApprovalPolicyPersisted { policy: None },
                        ),
                        Err(reason) => {
                            CommandResult::error(format!("Failed to save Full Access: {reason}"))
                        }
                    }
                }
                Some(ApprovalMode::Bypass) if persist => CommandResult::error(
                    "Full Access is saved as the TUI permission posture, not as a top-level approval_policy. Remove the controlling policy first.",
                ),
                Some(m) => {
                    if persist {
                        let saved = approval_mode_config_value(m);
                        match persist_root_string_key(
                            app.config_path.as_deref(),
                            "approval_policy",
                            saved,
                        ) {
                            Ok(path) => {
                                app.set_agent_approval_posture(m);
                                app.mark_approval_policy_locked();
                                CommandResult::with_message_and_action(
                                    format!(
                                        "approval_mode = {} (saved to {} as approval_policy = \"{}\")",
                                        m.permission_chip_label(),
                                        path.display(),
                                        saved
                                    ),
                                    AppAction::ApprovalPolicyPersisted {
                                        policy: Some(saved.to_string()),
                                    },
                                )
                            }
                            Err(err) => CommandResult::error(format!("Failed to save: {err}")),
                        }
                    } else {
                        app.set_agent_approval_posture(m);
                        CommandResult::with_message_and_action(
                            format!(
                                "approval_mode = {} (session only, add --save to persist)",
                                m.permission_chip_label()
                            ),
                            AppAction::ModeChanged(app.mode),
                        )
                    }
                }
                None => CommandResult::error(
                    "Invalid approval_mode. Use: auto-review/auto, ask/suggest/on-request, full-access, never/deny",
                ),
            };
        }
        "allow_shell" | "shell" | "exec_shell" => {
            let control = match load_command_config(app) {
                Ok(config) => config.allow_shell_control(
                    app.config_path.as_deref(),
                    app.config_profile.as_deref(),
                    &app.workspace,
                ),
                Err(err) => return CommandResult::error(err),
            };
            if !control.editable_root() {
                return CommandResult::error(format!(
                    "Shell access is controlled by {}; change that source first.",
                    control.label()
                ));
            }
            let enabled = match parse_config_bool(value) {
                Ok(enabled) => enabled,
                Err(err) => return CommandResult::error(err),
            };
            let suffix = if persist {
                match persist_root_bool_key(app.config_path.as_deref(), "allow_shell", enabled) {
                    Ok(path) => format!(" (saved to {})", path.display()),
                    Err(err) => return CommandResult::error(format!("Failed to save: {err}")),
                }
            } else {
                " (session only, add --save to persist)".to_string()
            };
            app.set_agent_shell_access(enabled);
            let mode_hint = if enabled {
                " Act mode will expose shell on the next turn with approval gating. Full Access (Shift+Tab) also enables shell and auto-approves."
            } else {
                " Shell tools will be hidden on the next turn. Re-enable with `/config allow_shell true`."
            };
            return CommandResult::message(format!("allow_shell = {enabled}{suffix}.{mode_hint}"));
        }
        "mcp_config_path" | "mcp" => {
            if value.trim().is_empty() {
                return CommandResult::error("mcp_config_path cannot be empty");
            }
            let next_path = PathBuf::from(expand_tilde(value));
            let path_changed = next_path != app.mcp_config_path;
            app.mcp_config_path = next_path;
            if path_changed {
                app.mcp_reload_required = true;
            }
            let reload_note = if path_changed {
                "; run /mcp reload to rebuild the live tool pool"
            } else {
                ""
            };
            let message = if persist {
                match persist_root_string_key(app.config_path.as_deref(), "mcp_config_path", value)
                {
                    Ok(path) => format!(
                        "mcp_config_path = {} (saved to {}){}",
                        app.mcp_config_path.display(),
                        path.display(),
                        reload_note
                    ),
                    Err(err) => return CommandResult::error(format!("Failed to save: {err}")),
                }
            } else {
                format!(
                    "mcp_config_path = {} (session only){}",
                    app.mcp_config_path.display(),
                    reload_note
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
        "title" | "window_title" | "tab_title" => {
            // Keep the config setter under the same terminal-control and
            // bidi/zero-width policy as `/title` and `/rename`. Persist the
            // normalized value too, so a restart cannot reintroduce bytes the
            // live session already discarded.
            let sanitized = crate::session_manager::sanitize_session_title(value);
            let value = sanitized.trim();
            if value.is_empty() {
                return CommandResult::error(
                    "title cannot be empty; use /title off to clear a session title",
                );
            }
            if value.chars().count() > 100 {
                return CommandResult::error("Title too long (max 100 characters)");
            }
            let suffix = if persist {
                match persist_root_string_key(app.config_path.as_deref(), "title", value) {
                    Ok(path) => format!(" (saved to {})", path.display()),
                    Err(err) => return CommandResult::error(format!("Failed to save: {err}")),
                }
            } else {
                " (session only, add --save to persist)".to_string()
            };
            app.title_default = Some(value.to_string());
            app.needs_redraw = true;
            return CommandResult::message(format!(
                "title = {value}{suffix} — terminal window titles now read [\"{value}\"] … until /title overrides this session"
            ));
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
                    "stream_chunk_timeout_secs must be 0 or {MIN_STREAM_CHUNK_TIMEOUT_SECS}..={MAX_STREAM_CHUNK_TIMEOUT_SECS}"
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
        "search" | "search.provider" | "search_provider" => {
            return set_search_provider(app, value, persist);
        }
        "prompt_suggestion" => return set_prompt_suggestion(app, value, persist),
        "notifications" => return notifications_config_command(app, value),
        _ => {}
    }

    // This copy exists to validate the value and to project it onto live `App`
    // state. It is deliberately *not* what gets saved: see
    // [`persist_single_setting`].
    let mut settings = match Settings::load_persisted() {
        Ok(s) => s,
        Err(e) if !persist => {
            app.status_message = Some(format!(
                "Settings unavailable; applying session-only override ({e})"
            ));
            Settings::default()
        }
        Err(e) => return CommandResult::error(format!("Failed to load settings: {e}")),
    };

    if key == "default_model"
        && !matches!(
            app.api_provider,
            ApiProvider::Deepseek | ApiProvider::DeepseekCN
        )
        && !persist
    {
        return CommandResult::error(format!(
            "default_model is the DeepSeek startup fallback and cannot change the active {} session. Use /model for the current provider, or add --save to change only future DeepSeek sessions.",
            app.api_provider.as_str()
        ));
    }

    if let Err(e) = settings.set(&key, value) {
        return CommandResult::error(format!("{e}"));
    }
    // Runtime/environment constraints are an effective projection, not saved
    // preferences. Keep the persisted copy pristine so NO_ANIMATIONS or a
    // terminal quirk cannot become permanent during an unrelated edit.
    let mut effective_settings = settings.clone();
    effective_settings.apply_env_overrides();

    let mut action = None;
    match key.as_str() {
        "auto_compact" | "compact" => {
            app.auto_compact = settings.auto_compact;
            app.auto_compact_user_configured = true;
            action = Some(AppAction::UpdateCompaction(app.compaction_config()));
        }
        "auto_compact_threshold" | "auto_compact_threshold_percent" => {
            app.auto_compact = true;
            app.auto_compact_user_configured = true;
            app.auto_compact_threshold_percent = settings.auto_compact_threshold_percent;
            app.update_model_compaction_budget();
            action = Some(AppAction::UpdateCompaction(app.compaction_config()));
        }
        "calm_mode" | "calm" => {
            app.calm_mode = settings.calm_mode;
            app.mark_history_updated();
        }
        "low_motion" | "motion" => {
            app.low_motion = effective_settings.low_motion;
            app.needs_redraw = true;
        }
        "fancy_animations" | "fancy" | "animations" => {
            app.fancy_animations = effective_settings.fancy_animations;
            app.needs_redraw = true;
        }
        "focus_texture" | "texture" => {
            app.focus_texture =
                crate::tui::focus_texture::FocusTextureMode::parse(&settings.focus_texture)
                    .unwrap_or_default();
            app.needs_redraw = true;
        }
        "work_surface_placement" | "work_surface" | "work_rail" => {
            app.work_surface.placement = crate::tui::work_surface::WorkSurfacePlacement::parse(
                &settings.work_surface_placement,
            );
            app.work_surface.focused = false;
            app.work_surface.last_area = None;
            app.needs_redraw = true;
        }
        "rail_panel" | "rail" => {
            app.work_surface.panel =
                crate::tui::work_surface::RailPanel::parse(&settings.rail_panel);
            app.needs_redraw = true;
        }
        "work_surface_top_height" | "work_top_height" => {
            app.work_surface.top_height = settings.work_surface_top_height;
            app.needs_redraw = true;
        }
        "work_surface_side_width" | "work_side_width" => {
            app.work_surface.side_width = settings.work_surface_side_width;
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
            app.synchronized_output_enabled = effective_settings.synchronized_output_enabled();
            app.needs_redraw = true;
        }
        "show_thinking" | "thinking" => {
            app.show_thinking = settings.show_thinking;
            app.mark_history_updated();
        }
        "thinking_default_expanded" | "thinking_expanded" => {
            app.thinking_default_expanded = settings.thinking_default_expanded;
            app.mark_history_updated();
        }
        "thinking_preview_lines" | "thinking_preview" => {
            app.thinking_preview_lines = settings.thinking_preview_lines;
            app.mark_history_updated();
        }
        "help_expand_groups" | "help_expanded" => {
            app.help_expand_groups = settings.help_expand_groups;
            app.needs_redraw = true;
        }
        "pin_last_prompt" | "pin_prompt" => {
            app.pin_last_prompt = settings.pin_last_prompt;
            app.needs_redraw = true;
        }
        "thinking_highlight" | "reasoning_highlight" => {
            app.thinking_highlight = settings.thinking_highlight;
            app.mark_history_updated();
        }
        "show_tool_details" | "tool_details" => {
            app.show_tool_details = settings.show_tool_details;
            app.mark_history_updated();
        }
        "inline_diffs" | "inline_diff" | "diffs" => {
            app.inline_diff_mode = crate::settings::InlineDiffMode::parse(&settings.inline_diffs);
            app.mark_history_updated();
            app.needs_redraw = true;
        }
        "locale" | "language" => {
            app.ui_locale = resolve_locale(&settings.locale);
            app.mark_history_updated();
            app.needs_redraw = true;
        }
        "theme" | "ui_theme" | "background_color" | "background" | "bg" => {
            // Theme previews reload persisted settings for each cursor move.
            // Keep a session-only background overlay live unless this command
            // is itself updating (or clearing) the background.
            let background_color_override = if matches!(key.as_str(), "theme" | "ui_theme") {
                app.background_color_override
            } else {
                settings
                    .background_color
                    .as_deref()
                    .and_then(crate::palette::parse_hex_rgb_color)
            };
            let background_setting =
                background_color_override.and_then(crate::palette::hex_rgb_string);
            let (_, theme_id, ui_theme) = match crate::palette::resolve_theme_setting(
                &settings.theme,
                background_setting.as_deref(),
            ) {
                Ok(resolved) => resolved,
                Err(error) => {
                    return CommandResult::error(format!("Failed to apply theme: {error}"));
                }
            };
            app.background_color_override = background_color_override;
            app.theme_id = theme_id;
            app.ui_theme = ui_theme;
            app.needs_redraw = true;
        }
        "cost_currency" | "currency" => {
            app.cost_currency = crate::pricing::CostCurrency::from_setting(&settings.cost_currency)
                .unwrap_or(crate::pricing::CostCurrency::Usd);
            app.needs_redraw = true;
        }
        key @ ("mini_window.keep_header"
        | "mini_window.keep_input"
        | "mini_window.keep_todo"
        | "mini_window.keep_sidebar"
        | "mini_window.keep_footer") => {
            let field = key.strip_prefix("mini_window.").unwrap_or(key);
            let value = match parse_config_bool(value) {
                Ok(value) => value,
                Err(err) => return CommandResult::error(err),
            };
            match field {
                "keep_header" => app.mini_window.keep_header = value,
                "keep_input" => app.mini_window.keep_input = value,
                "keep_todo" => app.mini_window.keep_todo = value,
                "keep_sidebar" => app.mini_window.keep_sidebar = value,
                "keep_footer" => app.mini_window.keep_footer = value,
                _ => unreachable!("mini_window field matched above"),
            }
            if persist
                && let Err(err) = crate::config_persistence::persist_mini_window_bool_key(
                    app.config_path.as_deref(),
                    field,
                    value,
                )
            {
                return CommandResult::error(format!("Failed to persist: {err}"));
            }
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
        "composer_multiline_mode" | "multiline_mode" | "multiline" => {
            app.composer_multiline_mode = settings.composer_multiline_mode;
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
            app.composer.mention_discovery.invalidate();
            app.needs_redraw = true;
        }
        "mention_menu_behavior" | "mention_behavior" | "mention_menu" => {
            app.mention_menu_behavior = settings.mention_menu_behavior.clone();
            app.composer.mention_completion_cache = None;
            app.composer.mention_discovery.invalidate();
            app.needs_redraw = true;
        }
        "mention_walk_depth" | "mention_depth" | "completions_walk_depth" => {
            app.mention_walk_depth = settings.mention_walk_depth;
            app.composer.mention_completion_cache = None;
            app.composer.mention_discovery.invalidate();
            app.needs_redraw = true;
        }
        "workspace_follow_symlinks" | "follow_symlinks" => {
            app.workspace_follow_symlinks = settings.workspace_follow_symlinks;
            app.composer.mention_completion_cache = None;
            app.composer.mention_discovery.invalidate();
            app.needs_redraw = true;
            // Engine tools use EngineConfig which is fixed at startup
            return CommandResult::message(if persist {
                if let Err(e) = persist_single_setting(&key, value) {
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
        // `default_mode` is a restart default, not a live mode switch. The
        // `/mode` command owns synchronized session transitions.
        "default_mode" => {}
        "mode" => {
            let mode = AppMode::from_setting(&settings.default_mode);
            app.set_mode(mode);
            action = Some(AppAction::ModeChanged(mode));
        }
        "max_history" | "history" => {
            app.max_input_history = settings.max_input_history;
        }
        "default_model" => {
            if matches!(
                app.api_provider,
                ApiProvider::Deepseek | ApiProvider::DeepseekCN
            ) && let Some(ref model) = settings.default_model
            {
                app.set_model_selection(model.clone());
                app.fleet_roster_stale |= crate::fleet::members::auto_enroll_fleet_model(
                    &app.workspace,
                    app.provider_identity_for_persistence(),
                    model,
                );
                app.update_model_compaction_budget();
                app.session.last_prompt_tokens = None;
                app.session.last_completion_tokens = None;
                app.session.last_output_throughput = None;
                action = Some(AppAction::UpdateCompaction(app.compaction_config()));
            }
        }
        "reasoning_effort" | "effort" => {
            app.reasoning_effort_preference = settings
                .reasoning_effort
                .as_deref()
                .map(ReasoningEffort::from_setting);
            app.reasoning_effort = app.reasoning_effort_preference.map_or_else(
                || {
                    if app.auto_model {
                        ReasoningEffort::Auto
                    } else {
                        ReasoningEffort::default()
                    }
                },
                |requested| {
                    if app.auto_model {
                        requested
                    } else {
                        requested.normalize_for_provider(app.api_provider)
                    }
                },
            );
            app.invalidate_route_receipts_for_reasoning_change();
            app.update_model_compaction_budget();
            action = Some(AppAction::UpdateCompaction(app.compaction_config()));
        }
        "context_panel" | "context" | "session_panel" => {
            app.context_panel = settings.context_panel;
            app.needs_redraw = true;
        }
        "sessions_rail" | "sessions_panel" | "session_rail" => {
            app.sessions_rail = settings.sessions_rail;
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
        "composer_multiline_mode" | "multiline_mode" | "multiline" => {
            settings.composer_multiline_mode.to_string()
        }
        "low_motion" | "motion" => settings.low_motion.to_string(),
        "fancy_animations" | "fancy" | "animations" => settings.fancy_animations.to_string(),
        _ => value.to_string(),
    };

    let mut message = if persist {
        if let Err(e) = persist_single_setting(&key, value) {
            return CommandResult::error(format!("Failed to save: {e}"));
        }
        format!("{key} = {display_value} (saved)")
    } else {
        format!("{key} = {display_value} (session only, add --save to persist)")
    };
    if key == "default_model"
        && !matches!(
            app.api_provider,
            ApiProvider::Deepseek | ApiProvider::DeepseekCN
        )
    {
        message.push_str(&format!(
            "; DeepSeek fallback only — active {}/{} is unchanged",
            app.api_provider.as_str(),
            app.model_display_label()
        ));
    }

    CommandResult {
        message: Some(message),
        action,
        is_error: false,
    }
}

/// Persist exactly the one key `/set --save` changed.
///
/// `/set` loads a `Settings` copy up front to validate the value and to project
/// it onto live `App` state, and a lot of `App` mutation happens in between. That
/// copy is a stale snapshot by the time we get here, so saving *it* would write
/// back every other field as it looked before — reverting any mode, thinking,
/// model, or permission write that landed in the meantime. Re-applying the single
/// key inside [`Settings::transact`] persists the user's actual edit and nothing
/// else. `Settings::set` is the same normalizer the copy above already accepted
/// the value through, so this cannot fail for a value that validated.
fn persist_single_setting(key: &str, value: &str) -> anyhow::Result<()> {
    Settings::transact(|settings| settings.set(key, value))
}

/// Select the TUI operating mode.
pub fn mode(app: &mut App, arg: Option<&str>) -> CommandResult {
    let Some(arg) = arg.filter(|value| !value.trim().is_empty()) else {
        return CommandResult::action(AppAction::OpenModePicker);
    };
    // The legacy YOLO spellings are a one-way permission shorthand, not a
    // mode: route them to the full-access compat path before parse folds
    // them to Act.
    if matches!(
        arg.trim().to_ascii_lowercase().as_str(),
        "yolo" | "4" | "bypass" | "bypass-permissions" | "bypasspermissions"
    ) {
        let (message, changed) = switch_yolo_compat_with_status(app);
        if changed {
            CommandResult::with_message_and_action(message, AppAction::ModeChanged(app.mode))
        } else {
            CommandResult::message(message)
        }
    } else {
        mode_selection(app, arg)
    }
}

/// `/mode <mode>` for the real modes (Plan/Act/Operate).
fn mode_selection(app: &mut App, arg: &str) -> CommandResult {
    match AppMode::parse(arg) {
        Some(mode) => {
            let (message, changed) = switch_mode_with_status(app, mode);
            if changed {
                CommandResult::with_message_and_action(message, AppAction::ModeChanged(mode))
            } else {
                CommandResult::message(message)
            }
        }
        None => CommandResult::error("Usage: /mode [act|agent|plan|operate|1|2|3]"),
    }
}

pub fn switch_mode(app: &mut App, mode: AppMode) -> String {
    switch_mode_with_status(app, mode).0
}

/// Returns the user-facing sentence and whether live mode moved (the caller
/// emits `AppAction::ModeChanged` only for the latter).
///
/// The three outcomes read differently on purpose. Before the typed
/// [`SettingSelection`], a refusal and a same-mode selection that *did* persist
/// the startup default both came back as "Already in X mode." — so the one case
/// where `/mode` had written something looked exactly like the case where it had
/// written nothing.
fn switch_mode_with_status(app: &mut App, mode: AppMode) -> (String, bool) {
    match app.select_mode(mode) {
        SettingSelection::Changed => (format!("Switched to {} mode.", mode.display_name()), true),
        SettingSelection::PersistedSame => (app.mode_startup_default_receipt(mode), false),
        SettingSelection::Refused => (
            app.setting_locked_message(MessageId::SettingSubjectMode),
            false,
        ),
    }
}

/// Status for the legacy YOLO alias: user-facing copy says Act, because the
/// alias is invisible Act + Full Access.
fn switch_yolo_compat_with_status(app: &mut App) -> (String, bool) {
    match app.select_yolo_compat() {
        SettingSelection::Changed => (
            format!("Switched to {} mode.", AppMode::Agent.display_name()),
            true,
        ),
        SettingSelection::PersistedSame => {
            (app.mode_startup_default_receipt(AppMode::Agent), false)
        }
        SettingSelection::Refused => (
            app.setting_locked_message(MessageId::SettingSubjectMode),
            false,
        ),
    }
}

/// `/theme [name]` — with no argument, open the interactive picker (arrow
/// keys, live preview, Enter to persist, Esc to revert). With an argument,
/// route through `set_config_value("theme", ...)` so the apply + save flow is
/// shared with `/config`.
pub fn theme(app: &mut App, arg: Option<&str>) -> CommandResult {
    match arg.map(str::trim).filter(|s| !s.is_empty()) {
        None => CommandResult::action(AppAction::OpenThemePicker),
        Some("schema") => CommandResult::message(crate::palette::user_theme_schema_json()),
        Some("path") => match crate::palette::user_themes_dir() {
            Ok(path) => CommandResult::message(format!(
                "User themes: {}\nSelect with: /theme custom:<name>",
                path.display()
            )),
            Err(error) => CommandResult::error(error),
        },
        // `underwater` is an ordinary theme (aliases `deepsea`/`deep-sea`/
        // `ombre` fold through the same normalizer); the painted ocean field
        // is the theme itself, not a treatment beside it.
        Some(name) => set_config_value(app, "theme", name, true),
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
        && let Some(home) = crate::config::effective_home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    } else if raw == "~"
        && let Some(home) = crate::config::effective_home_dir()
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

/// Unified login status. Account device flow stays on the CLI so this
/// command never freezes the TUI and never invents a second OAuth broker.
/// The internal cloud-agent credential is not user surface: membership
/// (`codewhale login`) is the only door, never a provider key.
pub fn login(app: &mut App, arg: Option<&str>) -> CommandResult {
    let raw = arg.map(str::trim).unwrap_or("");
    let token = raw.split_whitespace().next().unwrap_or("");
    match token {
        "" | "status" => CommandResult::message(login_status_text(app)),
        "key" | "provider" => CommandResult::with_message_and_action(
            "Open the provider picker to store an API key. Account sign-in is `codewhale login`.",
            AppAction::OpenProviderPicker,
        ),
        "account" => CommandResult::message(
            "TUI cannot start the browser device flow without freezing the session.\n\
             Run `codewhale login` (same as `codewhale account login`) in a terminal.\n\
             Then `/login` to confirm the session landed."
                .to_string(),
        ),
        other => CommandResult::error(format!(
            "Usage: /login [status|account|key]\nUnknown argument: {other}"
        )),
    }
}

fn login_status_text(app: &App) -> String {
    use codewhale_secrets::account::{
        ACCOUNT_API_BASE_ENV, AccountSessionState, AccountSessionStore, DEFAULT_ACCOUNT_API_BASE,
        secure_account_session_secrets,
    };

    let api_base = std::env::var(ACCOUNT_API_BASE_ENV)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_ACCOUNT_API_BASE.to_string());
    let account = match secure_account_session_secrets() {
        Ok(secrets) => {
            match AccountSessionStore::new(secrets, None, &api_base)
                .runtime_info_at(chrono::Utc::now())
            {
                Ok(info) => match info.state {
                    AccountSessionState::SignedOut => format!("not signed in (api {api_base})"),
                    AccountSessionState::Authenticated => format!("signed in (api {api_base})"),
                    AccountSessionState::OfflineCached => {
                        format!("offline cached (api {api_base})")
                    }
                    AccountSessionState::Expired => format!("expired (api {api_base})"),
                    AccountSessionState::Revoked => format!("revoked (api {api_base})"),
                },
                Err(error) => format!("unavailable ({error})"),
            }
        }
        Err(error) => format!("unavailable ({error})"),
    };
    let provider = app.provider_identity_for_persistence();
    format!(
        "Codewhale login\n\
         Account: {account}\n\
         Active provider: {provider}\n\
         \n\
         Sign in: `codewhale login`\n\
         Provider key: `codewhale auth set --provider <id>` or `/login key`\n\
         Sign out: `/logout` or `codewhale logout`"
    )
}

fn clear_local_account_session() -> Result<bool, String> {
    use codewhale_secrets::account::{
        ACCOUNT_API_BASE_ENV, AccountSessionStore, DEFAULT_ACCOUNT_API_BASE,
        secure_account_session_secrets,
    };
    let secrets = secure_account_session_secrets().map_err(|error| error.to_string())?;
    let api_base = std::env::var(ACCOUNT_API_BASE_ENV)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_ACCOUNT_API_BASE.to_string());
    let store = AccountSessionStore::new(secrets, None, &api_base);
    let had = store.load().map_err(|error| error.to_string())?.is_some();
    store.clear().map_err(|error| error.to_string())?;
    Ok(had)
}

fn clear_daytona_slot() -> Result<bool, String> {
    let secrets = codewhale_secrets::Secrets::auto_detect();
    let had = secrets
        .get(codewhale_secrets::DAYTONA_TOKEN_SLOT)
        .map_err(|error| error.to_string())?
        .is_some_and(|value| !value.trim().is_empty());
    if had {
        secrets
            .delete(codewhale_secrets::DAYTONA_TOKEN_SLOT)
            .map_err(|error| error.to_string())?;
    }
    Ok(had)
}

/// Logout — clear the active provider key, the Codewhale account session,
/// and the Daytona slot. Named custom providers still clear only their own
/// table. For a full every-provider wipe, use `codewhale logout`.
pub fn logout(app: &mut App) -> CommandResult {
    let provider_name = app.provider_identity_for_persistence().to_string();
    match clear_active_provider_api_key(&provider_name) {
        Ok(()) => {
            app.onboarding = OnboardingState::Provider;
            app.onboarding_needs_api_key = true;
            app.onboarding_provider = app.api_provider;
            app.onboarding_missing_key_recovery = true;
            app.api_key_env_only = false;
            let mut cleared = vec![format!("provider key ({provider_name})")];
            match clear_local_account_session() {
                Ok(true) => cleared.push("Codewhale account session".to_string()),
                Ok(false) => {}
                Err(error) => cleared.push(format!("account session not cleared ({error})")),
            }
            match clear_daytona_slot() {
                Ok(true) => cleared.push("internal cloud-agent token".to_string()),
                Ok(false) => {}
                Err(error) => {
                    cleared.push(format!("internal cloud-agent token not cleared ({error})"))
                }
            }
            CommandResult::with_message_and_action(
                format!(
                    "Cleared {}. \
                     Use `codewhale login` to sign in again, or `codewhale auth set --provider <id>` to store a provider key.",
                    cleared.join(", ")
                ),
                AppAction::OpenProviderPicker,
            )
        }
        Err(e) => CommandResult::error(format!("Failed to clear API key for {provider_name}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::test_support::{EnvVarGuard, TestEnvLock, lock_test_env};
    use crate::tui::app::{App, TuiOptions};
    use crate::tui::approval::ApprovalMode;
    use std::env;
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct EnvGuard {
        _vars: Vec<EnvVarGuard>,
        _lock: TestEnvLock,
    }

    impl EnvGuard {
        fn new(home: &Path) -> Self {
            let lock = lock_test_env();
            let config_path = home.join(".deepseek").join("config.toml");
            let vars = vec![
                EnvVarGuard::set("HOME", home),
                EnvVarGuard::set("USERPROFILE", home),
                EnvVarGuard::set("CODEWHALE_HOME", home.join(".codewhale")),
                EnvVarGuard::remove("CODEWHALE_CONFIG_PATH"),
                EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", config_path),
                EnvVarGuard::remove("CODEWHALE_ALLOW_SHELL"),
                EnvVarGuard::remove("DEEPSEEK_ALLOW_SHELL"),
                EnvVarGuard::remove("DEEPSEEK_APPROVAL_POLICY"),
                EnvVarGuard::remove("NO_ANIMATIONS"),
                EnvVarGuard::remove("TERM_PROGRAM"),
                EnvVarGuard::remove("PTYXIS_VERSION"),
                EnvVarGuard::remove("CODEWHALE_SEARCH_PROVIDER"),
                EnvVarGuard::remove("DEEPSEEK_SEARCH_PROVIDER"),
            ];
            Self {
                _vars: vars,
                _lock: lock,
            }
        }
    }

    fn create_test_app_with_config(config: &Config) -> App {
        let options = TuiOptions {
            model: "test-model".to_string(),
            // Keep command tests independent from the developer's saved
            // `default_mode` setting: with `false`, App::new starts in the
            // saved mode, so a machine with `default_mode = "yolo"` flips
            // `allow_shell` on and breaks the allow_shell assertions.
            start_in_agent_mode: true,
            skip_onboarding: false,
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        };
        let mut app = App::new(options, config);
        // App::new folds in saved TUI settings from the developer machine.
        // Pin command tests back to DeepSeek semantics so model aliases are
        // not normalized through a provider selected in an interactive run.
        app.model = "test-model".to_string();
        app.auto_model = false;
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app.model_ids_passthrough = false;
        app
    }

    fn create_test_app() -> App {
        create_test_app_with_config(&Config::default())
    }

    #[test]
    fn screen_commands_dispatch_to_the_matching_screen_mode() {
        let mut app = create_test_app();
        assert_eq!(app.screen_mode, ScreenMode::Fullscreen);

        let switched = crate::commands::execute("/inline", &mut app);
        assert_eq!(
            switched.action,
            Some(AppAction::SetScreenMode(ScreenMode::Inline)),
            "/inline must ask for the inline screen"
        );
        assert!(!switched.is_error, "/inline must not be an error");

        // The action is applied where the terminal lives, so the app is still
        // fullscreen here; asking for the current mode reports, it does not
        // emit a second action.
        let repeated = crate::commands::execute("/fullscreen", &mut app);
        assert!(
            repeated.action.is_none(),
            "already-current mode must not re-switch"
        );
        assert!(
            repeated
                .message
                .as_deref()
                .is_some_and(|msg| msg.contains("Already on the fullscreen screen")),
            "{:?}",
            repeated.message
        );

        let rejected = crate::commands::execute("/inline sideways", &mut app);
        assert!(rejected.is_error, "/inline takes no argument");
    }

    #[test]
    fn config_workflow_and_goal_explain_the_effective_tables() {
        let mut app = create_test_app();
        for token in ["workflow", "goal"] {
            let result = config_command(&mut app, Some(token));
            assert!(
                result.action.is_none(),
                "{token} must not spend a model turn"
            );
            let text = result.message.as_deref().unwrap_or_default();
            assert!(
                text.contains("require_approval_for_writes"),
                "{token}: {text}"
            );
            assert!(text.contains("max_continuations"), "{token}: {text}");
        }
    }

    #[test]
    fn title_config_reports_the_default_not_the_session_override() {
        let config = Config {
            title: Some(" workspace\u{1b}]0;ignored\u{7}\u{202e}-default ".to_string()),
            ..Config::default()
        };
        let mut app = create_test_app_with_config(&config);
        assert_eq!(
            app.title_default.as_deref(),
            Some("workspace]0;ignored-default")
        );
        app.window_title = Some("session-override".to_string());

        let shown = show_single_setting(&app, "title");

        assert_eq!(
            shown.message.as_deref(),
            Some("title = workspace]0;ignored-default")
        );
    }

    #[test]
    fn title_config_normalizes_the_live_and_persisted_default() {
        let dir = tempfile::tempdir().expect("isolated config dir");
        let config_path = dir.path().join("config.toml");
        let mut app = create_test_app();
        app.config_path = Some(config_path.clone());

        let result = set_config_value(
            &mut app,
            "title",
            " Ev\u{1b}]0;PWNED\u{7}il\u{202e} Beta ",
            true,
        );

        assert!(!result.is_error, "{:?}", result.message);
        assert_eq!(app.title_default.as_deref(), Some("Ev]0;PWNEDil Beta"));
        assert!(app.needs_redraw);
        let loaded = Config::load(Some(config_path), None).expect("reload saved config");
        assert_eq!(loaded.title.as_deref(), Some("Ev]0;PWNEDil Beta"));
    }

    /// The shipped preset must survive its own preflight, or `/config preset
    /// calm` would be refused for a reason the user cannot act on.
    #[test]
    fn the_shipped_preset_passes_its_own_preflight() {
        let app = create_test_app();
        let fields = crate::settings::preset_fields("calm").expect("the calm preset exists");
        assert_eq!(preset_preflight(&app, fields), None);
    }

    /// A field the setter would reject must be caught *before* the transaction
    /// opens. Previously the bundle was saved first and the per-field mirror
    /// pass then failed, leaving the user with an error message and a rewritten
    /// settings file.
    #[test]
    fn preset_preflight_refuses_an_invalid_field_before_any_write() {
        let app = create_test_app();
        let refusal = preset_preflight(&app, &[("calm_mode", "true"), ("low_motion", "banana")])
            .expect("an invalid value must be refused");
        assert!(
            refusal.contains("low_motion"),
            "the refusal must name the offending field, got {refusal:?}"
        );
    }

    /// A preset carrying a live-route key is refused whole while a turn runs,
    /// rather than saving the bundle and then failing on that one field.
    #[test]
    fn preset_preflight_refuses_a_live_route_field_while_a_turn_runs() {
        let mut app = create_test_app();
        app.is_loading = true;
        let bundle = [("calm_mode", "true"), ("reasoning_effort", "high")];
        let refusal =
            preset_preflight(&app, &bundle).expect("a live-route field must be refused mid-turn");
        assert!(
            refusal.contains("locked while a turn is running"),
            "got {refusal:?}"
        );

        app.is_loading = false;
        assert_eq!(
            preset_preflight(&app, &bundle),
            None,
            "the same bundle must apply once the turn ends"
        );
    }

    /// The refusal list is the contract for #2982 on the slash surfaces. Keep
    /// restart-only `default_mode` out of it: `set_config_value` deliberately
    /// does not apply that key to the live session.
    #[test]
    fn live_route_key_list_covers_every_route_mutating_alias() {
        for key in [
            "mode",
            "model",
            "default_model",
            "reasoning_effort",
            "effort",
            "provider",
            "approval_mode",
            "approval_policy",
            "approval",
        ] {
            assert!(
                live_route_setting_subject(key).is_some(),
                "{key} mutates the active route and must be locked mid-turn"
            );
        }
        for key in ["default_mode", "theme", "calm_mode", "rail_panel"] {
            assert!(
                live_route_setting_subject(key).is_none(),
                "{key} does not mutate the active route and must stay settable"
            );
        }
    }

    #[test]
    fn approval_aliases_are_inert_while_a_turn_is_running() {
        let mut app = create_test_app();
        app.approval_mode = ApprovalMode::Suggest;
        app.is_loading = true;

        for key in ["approval_mode", "approval_policy", "approval"] {
            let result = set_config_value(&mut app, key, "never", false);
            assert!(result.is_error, "{key} must be refused mid-turn");
            assert!(
                result
                    .message
                    .as_deref()
                    .is_some_and(|message| message.contains("locked while a turn is running")),
                "unexpected refusal for {key}: {:?}",
                result.message
            );
            assert_eq!(app.approval_mode, ApprovalMode::Suggest);
        }
    }

    #[test]
    fn config_preset_calm_applies_bundle_to_session_and_keeps_evidence() {
        let mut app = create_test_app();
        app.calm_mode = false;
        app.show_thinking = true;
        app.show_tool_details = true;
        app.fancy_animations = true;

        let result = config_command(&mut app, Some("preset calm"));
        let message = result.message.unwrap_or_default();
        assert!(
            message.contains("calm"),
            "summary should name the preset: {message}"
        );

        assert!(app.calm_mode);
        assert!(!app.show_tool_details);
        assert!(app.low_motion);
        assert!(!app.fancy_animations);
        assert_eq!(
            app.tool_collapse_mode,
            crate::tui::app::ToolCollapseMode::Calm
        );
        assert_eq!(
            app.transcript_spacing,
            crate::tui::app::TranscriptSpacing::Compact
        );
        // Evidence preserved: thinking is not hidden by the preset.
        assert!(app.show_thinking, "calm preset must not hide thinking");
    }

    #[test]
    fn config_preset_unknown_name_reports_error() {
        let mut app = create_test_app();
        let result = config_command(&mut app, Some("preset turbo"));
        let message = result.message.unwrap_or_default();
        assert!(
            message.to_lowercase().contains("unknown preset"),
            "expected unknown-preset error, got: {message}"
        );
    }

    #[test]
    fn config_preset_save_without_name_reports_usage() {
        let mut app = create_test_app();
        let result = config_command(&mut app, Some("preset --save"));
        let message = result.message.unwrap_or_default();
        assert!(
            message.contains("Usage: /config preset"),
            "expected usage hint, got: {message}"
        );
        assert!(!result.is_error);
    }

    #[test]
    fn work_surface_config_applies_live_and_accepts_bottom() {
        let mut app = create_test_app();

        let result = set_config_value(&mut app, "work_surface_placement", "left", false);
        assert!(!result.is_error, "{:?}", result.message);
        assert_eq!(
            app.work_surface.placement,
            crate::tui::work_surface::WorkSurfacePlacement::Left
        );
        let shown = show_single_setting(&app, "work_surface_placement");
        assert_eq!(
            shown.message.as_deref(),
            Some("work_surface_placement = left")
        );

        let result = set_config_value(&mut app, "work_surface_placement", "bottom", false);
        assert!(!result.is_error, "{:?}", result.message);
        assert_eq!(
            app.work_surface.placement,
            crate::tui::work_surface::WorkSurfacePlacement::Bottom
        );
    }

    #[test]
    fn rail_command_on_restores_default_bottom_placement() {
        let mut app = create_test_app();
        app.work_surface.placement = crate::tui::work_surface::WorkSurfacePlacement::Off;

        let result = sidebar(&mut app, Some("on"));

        assert!(!result.is_error);
        assert_eq!(
            app.work_surface.placement,
            crate::tui::work_surface::WorkSurfacePlacement::Bottom
        );
        let message = result.message.unwrap_or_default();
        assert!(message.contains("bottom placement"), "got: {message}");
    }

    #[test]
    fn rail_command_reports_narrow_terminal_top_fallback() {
        let mut app = create_test_app();
        app.work_surface.placement = crate::tui::work_surface::WorkSurfacePlacement::Left;
        // A 60-column host is below the side-rail floor, so the effective
        // placement falls back to top; the status must say so rather than
        // claim a left workbar renders.
        let _ = crate::tui::work_surface::height(&mut app, 60, 24, u16::MAX);

        let result = sidebar(&mut app, None);

        assert!(!result.is_error);
        let message = result.message.unwrap_or_default();
        assert!(message.contains("left placement"), "got: {message}");
        assert!(message.contains("showing top for now"), "got: {message}");
    }

    #[test]
    fn rail_command_off_never_claims_visibility() {
        let mut app = create_test_app();

        let result = sidebar(&mut app, Some("off"));

        assert!(!result.is_error);
        assert_eq!(
            app.work_surface.placement,
            crate::tui::work_surface::WorkSurfacePlacement::Off
        );
        let message = result.message.unwrap_or_default();
        assert!(message.contains("Workbar is off"), "got: {message}");
        assert!(
            !message.contains("Workbar is visible"),
            "the readout must never claim a hidden surface renders: {message}"
        );
    }

    #[test]
    fn rail_command_rejects_retired_auto_mode() {
        let mut app = create_test_app();

        let result = sidebar(&mut app, Some("auto"));

        assert!(result.is_error);
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("Usage: /workbar")
        );
    }

    #[test]
    fn test_mode_yolo_sets_all_flags() {
        let mut app = create_test_app();
        // Switch to Agent first to guarantee a clean starting state regardless of
        // user settings on the host machine.
        let _ = mode(&mut app, Some("agent"));
        let result = mode(&mut app, Some("yolo"));
        // YOLO is invisible Act+Bypass shorthand — user-facing copy says Act.
        assert!(result.message.unwrap().contains("Switched to Act mode"));
        assert_eq!(result.action, Some(AppAction::ModeChanged(AppMode::Agent)));
        assert!(app.allow_shell);
        assert!(app.trust_mode);
        assert!(app.yolo);
        assert_eq!(app.approval_mode, ApprovalMode::Bypass);
        // The deprecated YOLO alias remaps to Agent mode (M6 compat shim).
        assert_eq!(app.mode, AppMode::Agent);
    }

    #[test]
    fn test_mode_switch_command_accepts_names_and_numbers() {
        let mut app = create_test_app();
        let _ = mode(&mut app, Some("agent"));
        assert_eq!(app.mode, AppMode::Agent);
        let result = mode(&mut app, Some("2"));
        assert_eq!(result.action, Some(AppAction::ModeChanged(AppMode::Plan)));
        assert_eq!(app.mode, AppMode::Plan);
        let result = mode(&mut app, Some("act"));
        assert_eq!(result.action, Some(AppAction::ModeChanged(AppMode::Agent)));
        assert_eq!(app.mode, AppMode::Agent);
        let _ = mode(&mut app, Some("plan"));
        assert_eq!(app.mode, AppMode::Plan);
        let result = mode(&mut app, Some("3"));
        assert_eq!(
            result.action,
            Some(AppAction::ModeChanged(AppMode::Operate))
        );
        assert_eq!(app.mode, AppMode::Operate);
        let result = mode(&mut app, Some("5"));
        assert!(result.is_error);
        assert_eq!(app.mode, AppMode::Operate);
        let result = mode(&mut app, Some("9"));
        assert!(result.is_error);
        assert_eq!(app.mode, AppMode::Operate);
        let result = mode(&mut app, Some("4"));
        // "4" still routes to the deprecated YOLO alias, which lands in Agent
        // mode with bypass approvals (M6 compat shim).
        assert_eq!(result.action, Some(AppAction::ModeChanged(AppMode::Agent)));
        assert_eq!(app.mode, AppMode::Agent);
        assert!(app.yolo);
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
    fn settings_command_opens_typed_editor_and_preserves_text_mode() {
        let _lock = lock_test_env();
        let mut app = create_test_app();

        let modal = settings_command(&mut app, None);
        assert!(modal.message.is_none());
        assert!(matches!(modal.action, Some(AppAction::OpenConfigView)));

        let text = settings_command(&mut app, Some("text"));
        let message = text.message.as_deref().expect("settings diagnostic text");
        assert!(message.contains("Settings:"), "{message}");
        assert!(message.contains("provider_models:"), "{message}");
        assert!(message.contains("Config file:"), "{message}");
        assert!(text.action.is_none());
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
    fn config_model_rejects_foreign_model_for_direct_provider() {
        let mut app = create_test_app();
        app.api_provider = ApiProvider::Zai;
        app.model = crate::config::ZAI_GLM_5_2_MODEL.to_string();

        let result = set_config_value(&mut app, "model", "deepseek-v4-pro", false);

        assert!(result.is_error);
        assert_eq!(app.model, crate::config::ZAI_GLM_5_2_MODEL);
        assert!(result.action.is_none());
        let message = result.message.as_deref().expect("rejection message");
        assert!(
            message.contains("not compatible with provider 'zai'")
                || message.contains("not served by direct provider zai"),
            "unexpected rejection message: {message}"
        );
        assert!(message.contains("deepseek-v4-pro"), "{message}");
    }

    #[test]
    fn config_model_auto_preserves_explicit_thinking() {
        let mut app = create_test_app();
        app.reasoning_effort = ReasoningEffort::Off;
        app.reasoning_effort_preference = Some(ReasoningEffort::Off);

        let result = config_command(&mut app, Some("model auto"));

        assert!(result.message.is_some());
        assert!(app.auto_model);
        assert_eq!(app.model, "auto");
        assert_eq!(app.reasoning_effort, ReasoningEffort::Off);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("thinking = off"))
        );
        assert!(app.last_effective_model.is_none());
        assert!(app.last_effective_reasoning_effort.is_none());
    }

    #[test]
    fn config_model_auto_releases_implicit_fixed_model_thinking() {
        let mut app = create_test_app();
        app.reasoning_effort = ReasoningEffort::Max;
        app.reasoning_effort_preference = None;

        let result = config_command(&mut app, Some("model auto"));

        assert!(result.message.is_some());
        assert!(app.auto_model);
        assert_eq!(app.reasoning_effort, ReasoningEffort::Auto);
        assert_eq!(app.reasoning_effort_preference, None);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("thinking = auto"))
        );
    }

    #[test]
    fn config_reasoning_effort_applies_while_model_routing_is_auto() {
        let mut app = create_test_app();
        app.set_model_selection("auto".to_string());
        app.reasoning_effort = ReasoningEffort::Auto;
        app.reasoning_effort_preference = None;

        let result = set_config_value(&mut app, "reasoning_effort", "low", false);

        assert!(!result.is_error);
        assert_eq!(app.reasoning_effort, ReasoningEffort::Low);
        assert_eq!(app.reasoning_effort_preference, Some(ReasoningEffort::Low));
        assert!(matches!(
            result.action,
            Some(AppAction::UpdateCompaction(_))
        ));
    }

    #[test]
    fn config_default_model_cannot_replace_a_non_deepseek_live_route() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-provider-scoped-default-model-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let mut app = create_test_app();
        app.api_provider = ApiProvider::Zai;
        app.model = crate::config::ZAI_GLM_5_2_MODEL.to_string();
        app.auto_model = false;

        let session_only = set_config_value(&mut app, "default_model", "deepseek-v4-flash", false);

        assert!(session_only.is_error);
        assert_eq!(app.model, crate::config::ZAI_GLM_5_2_MODEL);
        assert!(session_only.action.is_none());
        assert!(
            session_only
                .message
                .as_deref()
                .is_some_and(|message| message.contains("DeepSeek startup fallback"))
        );

        let saved = set_config_value(&mut app, "default_model", "deepseek-v4-flash", true);

        assert!(!saved.is_error);
        assert_eq!(app.model, crate::config::ZAI_GLM_5_2_MODEL);
        assert!(saved.action.is_none());
        assert!(
            saved
                .message
                .as_deref()
                .is_some_and(|message| message.contains("active zai/GLM-5.2 is unchanged"))
        );
        let persisted = Settings::load_persisted().expect("saved settings");
        assert_eq!(
            persisted.default_model.as_deref(),
            Some("deepseek-v4-flash")
        );
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
        assert_eq!(app.reasoning_effort_preference, Some(ReasoningEffort::Off));
        assert_eq!(
            result.message.as_deref(),
            Some("reasoning_effort = low (session only, add --save to persist)")
        );

        let result = set_config_value(&mut app, "reasoning_effort", "xhigh", false);

        // `xhigh` stopped collapsing into `Max` when the ladder gave it a rung.
        assert_eq!(app.reasoning_effort, ReasoningEffort::XHigh);
        assert_eq!(
            result.message.as_deref(),
            Some("reasoning_effort = xhigh (session only, add --save to persist)")
        );
    }

    #[test]
    fn config_fancy_animations_keeps_ghostty_full_motion() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-ghostty-fancy-config-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        // Neutralize the SSH markers: production intentionally caps motion
        // over SSH, and the suite routinely runs inside one.
        let _ssh_client = EnvVarGuard::remove("SSH_CLIENT");
        let _ssh_connection = EnvVarGuard::remove("SSH_CONNECTION");
        let _ssh_tty = EnvVarGuard::remove("SSH_TTY");
        let prev_term_program = env::var_os("TERM_PROGRAM");
        // Safety: test-only environment mutation guarded by EnvGuard's lock.
        unsafe {
            env::set_var("TERM_PROGRAM", "Ghostty");
        }

        let mut app = create_test_app();
        assert!(app.fancy_animations);
        assert!(!app.constrained_frame_rate);

        let result = set_config_value(&mut app, "fancy_animations", "true", false);

        assert!(!result.is_error);
        assert!(
            app.fancy_animations,
            "Ghostty must keep authored motion enabled"
        );
        assert_eq!(
            result.message.as_deref(),
            Some("fancy_animations = true (session only, add --save to persist)")
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
        let temp_root = tempfile::tempdir().expect("isolated settings dir");
        let _guard = EnvGuard::new(temp_root.path());
        let mut app = create_test_app();
        let _result = config_command(&mut app, Some("model deepseek-v4-flash --save"));
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
        assert!(msg.contains("Act mode"));
        assert!(msg.contains("approval gating"));
        assert!(msg.contains("next turn"));
        assert!(msg.contains("Full Access (Shift+Tab) also enables shell and auto-approves"));
    }

    #[test]
    fn config_command_allow_shell_save_persists_root_boolean() {
        let temp_root = tempfile::tempdir().expect("isolated config dir");
        let _guard = EnvGuard::new(temp_root.path());

        let config_path = temp_root.path().join("custom-config.toml");

        let mut app = create_test_app();
        app.config_path = Some(config_path.clone());
        let result = config_command(&mut app, Some("allow_shell true --save"));
        let msg = result.message.unwrap();
        let saved = fs::read_to_string(&config_path).unwrap();

        assert!(app.allow_shell);
        assert_eq!(
            msg,
            format!(
                "allow_shell = true (saved to {}). Act mode will expose shell on the next turn with approval gating. Full Access (Shift+Tab) also enables shell and auto-approves.",
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
    fn config_command_cannot_bypass_project_shell_constraint() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-project-shell-control-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(temp_root.join(".deepseek")).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let root_config = temp_root.join(".deepseek").join("config.toml");
        fs::write(&root_config, "# user root\n").unwrap();
        let workspace = temp_root.join("workspace");
        fs::create_dir_all(workspace.join(codewhale_config::CODEWHALE_APP_DIR)).unwrap();
        fs::write(
            workspace
                .join(codewhale_config::CODEWHALE_APP_DIR)
                .join("config.toml"),
            "allow_shell = false\n",
        )
        .unwrap();
        let mut app = create_test_app();
        app.config_path = Some(root_config.clone());
        app.workspace = workspace;
        app.set_agent_shell_access(false);

        let result = config_command(&mut app, Some("allow_shell true --save"));

        assert!(result.is_error, "{:?}", result.message);
        assert!(!app.allow_shell);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("project configuration"))
        );
        assert!(
            !fs::read_to_string(root_config)
                .unwrap()
                .contains("allow_shell")
        );
    }

    #[test]
    fn config_command_cannot_bypass_environment_shell_constraint() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-env-shell-control-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(temp_root.join(".deepseek")).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let config_path = temp_root.join(".deepseek").join("config.toml");
        fs::write(&config_path, "# root\n").unwrap();
        // Safety: EnvGuard holds the process-wide environment lock and restores
        // this variable on drop.
        unsafe { env::set_var("DEEPSEEK_ALLOW_SHELL", "false") };
        let config = Config::load(Some(config_path.clone()), None).unwrap();
        let mut app = create_test_app_with_config(&config);
        app.config_path = Some(config_path);
        app.set_agent_shell_access(false);

        let result = config_command(&mut app, Some("allow_shell true"));

        assert!(result.is_error, "{:?}", result.message);
        assert!(!app.allow_shell);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("DEEPSEEK_ALLOW_SHELL"))
        );
    }

    #[test]
    fn config_command_cannot_bypass_project_or_environment_approval() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-external-approval-control-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(temp_root.join(".deepseek")).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let root_config = temp_root.join(".deepseek").join("config.toml");
        fs::write(&root_config, "# root\n").unwrap();
        let workspace = temp_root.join("workspace");
        fs::create_dir_all(workspace.join(codewhale_config::CODEWHALE_APP_DIR)).unwrap();
        fs::write(
            workspace
                .join(codewhale_config::CODEWHALE_APP_DIR)
                .join("config.toml"),
            "approval_policy = \"never\"\n",
        )
        .unwrap();
        let mut app = create_test_app();
        app.config_path = Some(root_config.clone());
        app.workspace = workspace;
        app.set_agent_approval_posture(ApprovalMode::Never);

        let project_result = config_command(&mut app, Some("approval_mode full-access"));
        assert!(project_result.is_error, "{:?}", project_result.message);
        assert_eq!(app.approval_mode, ApprovalMode::Never);

        // Move outside the project and make the environment the controlling
        // source for the second half of the regression.
        app.workspace = temp_root.join("clean-workspace");
        fs::create_dir_all(&app.workspace).unwrap();
        // Safety: EnvGuard holds the process-wide environment lock and restores
        // this variable on drop.
        unsafe { env::set_var("DEEPSEEK_APPROVAL_POLICY", "never") };
        let env_result = config_command(&mut app, Some("approval_mode auto"));
        assert!(env_result.is_error, "{:?}", env_result.message);
        assert_eq!(app.approval_mode, ApprovalMode::Never);
        assert!(
            env_result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("DEEPSEEK_APPROVAL_POLICY"))
        );
    }

    #[test]
    fn config_command_shell_choice_survives_plan_round_trip() {
        let mut app = create_test_app();
        app.set_agent_approval_posture(ApprovalMode::Bypass);

        let result = config_command(&mut app, Some("allow_shell true"));

        assert!(!result.is_error, "{:?}", result.message);
        app.set_mode(AppMode::Plan);
        assert!(!app.allow_shell);
        app.set_mode(AppMode::Agent);
        assert!(app.allow_shell);
        assert_eq!(app.approval_mode, ApprovalMode::Bypass);
    }

    #[test]
    fn config_command_subagents_off_save_persists_and_updates_runtime() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-subagents-off-save-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let config_path = temp_root.join("custom-config.toml");

        let mut app = create_test_app();
        app.config_path = Some(config_path.clone());
        let result = config_command(&mut app, Some("subagents off --save"));
        let msg = result.message.unwrap();
        let saved = fs::read_to_string(&config_path).unwrap();

        assert!(!result.is_error);
        assert!(msg.contains("subagents.enabled = false"));
        assert!(msg.contains("saved to"));
        assert!(saved.contains("[subagents]"));
        assert!(saved.contains("enabled = false"));
        match result.action {
            Some(AppAction::UpdateSubagentRuntimeConfig { enabled, .. }) => {
                assert!(!enabled);
            }
            other => panic!("expected subagent runtime update, got {other:?}"),
        }
    }

    #[test]
    fn config_command_subagents_depth_save_clamps_to_ceiling() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-subagents-depth-save-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let config_path = temp_root.join("custom-config.toml");

        let mut app = create_test_app();
        app.config_path = Some(config_path.clone());
        let result = config_command(&mut app, Some("subagents max_depth 99 --save"));
        let msg = result.message.unwrap();
        let saved = fs::read_to_string(&config_path).unwrap();
        let ceiling = codewhale_config::MAX_SPAWN_DEPTH_CEILING;

        assert!(!result.is_error);
        assert!(msg.contains(&format!("subagents.max_depth = {ceiling}")));
        assert!(msg.contains(&format!("clamped from 99 to {ceiling}")));
        assert!(saved.contains(&format!("max_depth = {ceiling}")));
        match result.action {
            Some(AppAction::UpdateSubagentRuntimeConfig {
                max_spawn_depth, ..
            }) => {
                assert_eq!(max_spawn_depth, ceiling);
            }
            other => panic!("expected subagent runtime update, got {other:?}"),
        }
    }

    #[test]
    fn config_command_subagents_status_shows_raw_and_resolved_values() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-subagents-status-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let config_path = temp_root.join("custom-config.toml");
        fs::write(
            &config_path,
            r#"
[subagents]
enabled = true
max_concurrent = 2
max_depth = 0
launch_concurrency = 5
api_timeout_secs = 0
heartbeat_timeout_secs = 1
"#,
        )
        .unwrap();

        let mut app = create_test_app();
        app.config_path = Some(config_path);
        let result = config_command(&mut app, Some("subagents status"));
        let msg = result.message.unwrap();

        assert!(!result.is_error);
        assert!(msg.contains("Sub-agents: disabled (subagents.max_depth=0)"));
        assert!(msg.contains("Active provider: deepseek"));
        assert!(
            msg.contains("subagents.max_concurrent = 2 (resolved global 2; active provider 2)")
        );
        assert!(
            msg.contains("subagents.launch_concurrency = 5 (resolved global 2; active provider 2)")
        );
        assert!(
            msg.contains(
                "subagents.api_timeout_secs = 0 (resolved global 600; active provider 600)"
            )
        );
        assert!(msg.contains(
            "subagents.heartbeat_timeout_secs = 1 (resolved global 630; active provider 630)"
        ));
        assert!(msg.contains("subagents.providers.deepseek = inherits global"));
    }

    #[test]
    fn config_command_audit_lists_editability_and_current_values() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-config-audit-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();
        // Hermetic: the audit reads Settings::load(); without this guard the
        // developer's real saved permission_posture leaks in and the
        // "(unset)" assertion below becomes machine-dependent.
        let _guard = EnvGuard::new(&temp_root);
        let config_path = temp_root.join("custom-config.toml");
        fs::write(
            &config_path,
            r#"
base_url = "https://api.from-config.local/v1"
instructions = ["~/global.md"]
prompt_suggestion = true

[subagents]
enabled = false
max_concurrent = 4

[search]
provider = "bing"

[notifications]
method = "osc9"
threshold_secs = 45
quiet = true
completion_sound = "off"
"#,
        )
        .unwrap();

        let mut app = create_test_app();
        app.config_path = Some(config_path.clone());
        app.approval_mode = ApprovalMode::Never;
        app.stream_chunk_timeout_secs = 45;

        let result = config_command(&mut app, Some("audit"));
        let msg = result.message.unwrap();

        assert!(!result.is_error);
        assert!(msg.contains("Config editability audit"));
        assert!(msg.contains(&format!("Config path: {}", config_path.display())));
        assert!(msg.contains("effective_permissions | Never | runtime"));
        assert!(msg.contains("permission_posture | (unset) | TUI settings"));
        assert!(msg.contains("approval_policy | (unset) | persisted config"));
        assert!(msg.contains("stream_chunk_timeout_secs | 45 | runtime+persisted"));
        assert!(msg.contains("subagents.enabled | false | runtime+persisted"));
        assert!(msg.contains("subagents.max_concurrent | 4 | runtime+persisted"));
        assert!(msg.contains("base_url | https://api.from-config.local/v1 | persisted restart"));
        assert!(msg.contains("providers.<active>.context_window | (unset) | persisted restart"));
        assert!(msg.contains("effective_context_window |"), "{msg}");
        assert!(msg.contains("| runtime | /config context_window"), "{msg}");
        assert!(msg.contains("instructions | configured | file-only restart"));
        assert!(msg.contains("network | unset | file-only"));
        assert!(
            msg.contains("search.provider | bing (source: config.toml) | runtime+persisted"),
            "{msg}"
        );
        assert!(
            msg.contains("prompt_suggestion | true | runtime+persisted"),
            "{msg}"
        );
        assert!(
            msg.contains(
                "notifications | method=osc9 threshold=45s sound=off quiet=true | runtime+persisted"
            ),
            "{msg}"
        );

        app.mode = AppMode::Plan;
        let plan_msg = config_command(&mut app, Some("audit"))
            .message
            .expect("Plan audit message");
        assert!(
            plan_msg.contains("effective_permissions | Read Only | runtime"),
            "{plan_msg}"
        );
    }

    #[test]
    fn config_command_shows_search_prompt_suggestion_and_notifications() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-config-discovery-show-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let config_path = temp_root.join("custom-config.toml");
        fs::write(
            &config_path,
            r#"
prompt_suggestion = true

[search]
provider = "tavily"

[notifications]
method = "bel"
threshold_secs = 12
quiet = false
completion_sound = "bell"
"#,
        )
        .unwrap();

        let mut app = create_test_app();
        app.config_path = Some(config_path);

        let search = config_command(&mut app, Some("search.provider"));
        assert!(!search.is_error, "{:?}", search.message);
        assert_eq!(
            search.message.as_deref(),
            Some("search.provider = tavily (source: config.toml)")
        );

        let suggestion = config_command(&mut app, Some("prompt_suggestion"));
        assert!(!suggestion.is_error, "{:?}", suggestion.message);
        assert_eq!(
            suggestion.message.as_deref(),
            Some("prompt_suggestion = true")
        );

        let notifications = config_command(&mut app, Some("notifications"));
        let notifications_msg = notifications.message.expect("notifications status");
        assert!(!notifications.is_error, "{notifications_msg}");
        assert!(
            notifications_msg.contains("method = bel"),
            "{notifications_msg}"
        );
        assert!(
            notifications_msg.contains("threshold_secs = 12"),
            "{notifications_msg}"
        );
        assert!(
            notifications_msg.contains("completion_sound = bell"),
            "{notifications_msg}"
        );
    }

    #[test]
    fn config_command_sets_search_prompt_suggestion_and_notifications() {
        let temp_root = tempfile::tempdir().expect("isolated config dir");
        let _guard = EnvGuard::new(temp_root.path());
        let config_path = temp_root.path().join("custom-config.toml");

        let mut app = create_test_app();
        app.config_path = Some(config_path.clone());

        let search = config_command(&mut app, Some("search.provider duckduckgo --save"));
        assert!(!search.is_error, "{:?}", search.message);
        match search.action {
            Some(AppAction::UpdateSearchProvider { provider }) => {
                assert_eq!(provider, SearchProvider::DuckDuckGo);
            }
            other => panic!("expected UpdateSearchProvider, got {other:?}"),
        }

        let suggestion = config_command(&mut app, Some("prompt_suggestion true --save"));
        assert!(!suggestion.is_error, "{:?}", suggestion.message);
        match suggestion.action {
            Some(AppAction::UpdatePromptSuggestion { enabled }) => assert!(enabled),
            other => panic!("expected UpdatePromptSuggestion, got {other:?}"),
        }

        let notifications = config_command(&mut app, Some("notifications method osc9 --save"));
        assert!(!notifications.is_error, "{:?}", notifications.message);
        match notifications.action {
            Some(AppAction::UpdateNotification {
                update: NotificationConfigUpdate::Method(method),
            }) => assert_eq!(method, NotificationMethod::Osc9),
            other => panic!("expected UpdateNotification method, got {other:?}"),
        }

        let saved = fs::read_to_string(&config_path).unwrap();
        assert!(saved.contains("provider = \"duckduckgo\""), "{saved}");
        assert!(saved.contains("prompt_suggestion = true"), "{saved}");
        assert!(saved.contains("method = \"osc9\""), "{saved}");

        let loaded = Config::load(Some(config_path), None).expect("reloaded config");
        assert_eq!(loaded.search_provider(), SearchProvider::DuckDuckGo);
        assert!(loaded.prompt_suggestion_enabled());
        assert_eq!(
            loaded.notifications_config().method,
            NotificationMethod::Osc9
        );
    }

    #[test]
    fn session_only_notification_commands_emit_composable_field_deltas() {
        let temp_root = tempfile::tempdir().expect("isolated config dir");
        let _guard = EnvGuard::new(temp_root.path());
        let config_path = temp_root.path().join("custom-config.toml");
        fs::write(
            &config_path,
            "[notifications]\nmethod = \"bel\"\nthreshold_secs = 12\nquiet = false\n",
        )
        .expect("persisted notification config");

        let mut app = create_test_app();
        app.config_path = Some(config_path);
        let mut live = NotificationsConfig {
            threshold_secs: 12,
            ..NotificationsConfig::default()
        };

        for command in ["notifications method osc9", "notifications quiet true"] {
            let result = config_command(&mut app, Some(command));
            assert!(!result.is_error, "{:?}", result.message);
            let Some(AppAction::UpdateNotification { update }) = result.action else {
                panic!("expected notification field delta for {command}");
            };
            live.apply_update(update);
        }

        assert_eq!(live.method, NotificationMethod::Osc9);
        assert!(live.quiet);
        assert_eq!(live.threshold_secs, 12);
    }

    #[test]
    fn config_command_rejects_invalid_search_and_notification_values() {
        let mut app = create_test_app();
        let search = config_command(&mut app, Some("search.provider not-a-backend"));
        assert!(search.is_error);
        let search_msg = search.message.unwrap();
        assert!(
            search_msg.contains("Invalid search.provider"),
            "{search_msg}"
        );
        assert!(search_msg.contains("firecrawl"), "{search_msg}");

        let notifications = config_command(&mut app, Some("notifications method semaphore"));
        assert!(notifications.is_error);
        let notifications_msg = notifications.message.unwrap();
        assert!(
            notifications_msg.contains("Invalid notifications.method"),
            "{notifications_msg}"
        );
        assert!(notifications_msg.contains("osc9"), "{notifications_msg}");
    }

    #[test]
    fn config_context_window_query_shows_override_and_effective_source() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-context-window-query-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let config_path = temp_root.join("custom-config.toml");
        fs::write(
            &config_path,
            r#"
provider = "moonshot"
[providers.moonshot]
model = "kimi-k3"
context_window = 262144
"#,
        )
        .unwrap();
        let mut app = create_test_app();
        app.config_path = Some(config_path);
        app.api_provider = ApiProvider::Moonshot;
        app.model = "kimi-k3".to_string();
        app.active_route_limits = Some(codewhale_config::route::RouteLimits {
            context_tokens: Some(262_144),
            ..Default::default()
        });
        app.active_context_window_source = crate::route_runtime::ContextWindowSource::Configured;

        let result = config_command(&mut app, Some("context_window"));
        let message = result.message.expect("context window message");

        assert!(!result.is_error, "{message}");
        assert!(
            message.contains("262144 (effective 262144 from configured)"),
            "{message}"
        );
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
                "stream_chunk_timeout_secs = 0 (default 900) (session only; affects subsequent turns in this session)"
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
        assert!(saved.contains(&format!("base_url = \"{DEFAULT_XIAOMI_MIMO_BASE_URL}\"")));
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
    fn theme_command_underwater_alias_selects_the_underwater_theme() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-theme-underwater-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let mut app = create_test_app();
        for alias in ["underwater", "Deepsea", "deep-sea", "ombre"] {
            let result = theme(&mut app, Some(alias));
            assert!(!result.is_error, "{alias}: {:?}", result.message);
            assert_eq!(
                result.message.as_deref(),
                Some("theme = underwater (saved)"),
                "{alias}"
            );
            assert_eq!(app.theme_id, crate::palette::ThemeId::Underwater, "{alias}");
            assert_eq!(app.ui_theme.name, "underwater", "{alias}");
            assert!(
                crate::tui::ocean::OceanRamp::for_theme(&app.ui_theme).is_some(),
                "{alias}: the underwater theme owns the painted field"
            );
        }
    }

    #[test]
    fn underwater_theme_selection_updates_live_state_and_persists_one_field() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-underwater-selection-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join(".deepseek")).expect("settings dir");
        let _guard = EnvGuard::new(&temp_root);
        fs::write(
            temp_root.join(".deepseek").join("settings.toml"),
            "theme = \"light\"\nmax_input_history = 77\n",
        )
        .expect("seed settings");

        let mut app = create_test_app();
        let result = set_config_value(&mut app, "theme", "underwater", true);

        assert!(!result.is_error, "{:?}", result.message);
        assert_eq!(app.theme_id, crate::palette::ThemeId::Underwater);
        let persisted = Settings::load_persisted().expect("persisted selection");
        assert_eq!(persisted.theme, "underwater");
        assert_eq!(
            persisted.max_input_history, 77,
            "the theme save must not overwrite unrelated settings"
        );
    }

    #[test]
    fn invalid_theme_name_changes_nothing() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-theme-preflight-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join(".deepseek")).expect("settings dir");
        let _guard = EnvGuard::new(&temp_root);
        fs::write(
            temp_root.join(".deepseek").join("settings.toml"),
            "theme = \"light\"\n",
        )
        .expect("seed settings");

        let mut app = create_test_app();
        let original_theme = app.theme_id;
        let result = set_config_value(&mut app, "theme", "kelp", true);

        assert!(result.is_error);
        assert_eq!(app.theme_id, original_theme);
        let persisted = Settings::load_persisted().expect("unchanged persisted settings");
        assert_eq!(persisted.theme, "light");
    }

    #[test]
    fn explicit_default_background_override_survives_theme_preview() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-background-override-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join(".deepseek")).expect("settings dir");
        let _guard = EnvGuard::new(&temp_root);
        fs::write(
            temp_root.join(".deepseek").join("settings.toml"),
            "theme = \"solarized-light\"\nbackground_color = \"#fdf6e3\"\n",
        )
        .expect("seed settings");

        let mut app = create_test_app();
        let explicit_base3 = ratatui::style::Color::Rgb(0xfd, 0xf6, 0xe3);
        assert_eq!(app.background_color_override, Some(explicit_base3));

        let result = set_config_value(&mut app, "theme", "dark", false);

        assert!(!result.is_error, "{:?}", result.message);
        assert_eq!(app.theme_id, crate::palette::ThemeId::Whale);
        assert_eq!(app.background_color_override, Some(explicit_base3));
        assert_eq!(app.ui_theme.surface_bg, explicit_base3);
        assert!(
            crate::tui::ocean::OceanRamp::for_theme(&app.ui_theme).is_none(),
            "only the underwater theme owns a painted field"
        );
    }

    #[test]
    fn underwater_theme_keeps_its_field_under_a_background_override() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-underwater-override-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join(".deepseek")).expect("settings dir");
        let _guard = EnvGuard::new(&temp_root);

        let mut app = create_test_app();
        let custom = ratatui::style::Color::Rgb(0x1a, 0x1b, 0x26);
        let background = set_config_value(&mut app, "background_color", "#1a1b26", false);
        assert!(!background.is_error, "{:?}", background.message);

        let preview = set_config_value(&mut app, "theme", "underwater", false);
        assert!(!preview.is_error, "{:?}", preview.message);
        assert_eq!(app.theme_id, crate::palette::ThemeId::Underwater);
        assert_eq!(app.background_color_override, Some(custom));
        assert_eq!(app.ui_theme.surface_bg, custom);
        assert!(
            crate::tui::ocean::OceanRamp::for_theme(&app.ui_theme).is_some(),
            "the underwater theme's field survives a background override"
        );
    }

    #[test]
    fn session_only_background_override_survives_theme_preview() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-session-background-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join(".deepseek")).expect("settings dir");
        let _guard = EnvGuard::new(&temp_root);
        fs::write(
            temp_root.join(".deepseek").join("settings.toml"),
            "theme = \"solarized-light\"\n",
        )
        .expect("seed settings");

        let mut app = create_test_app();
        let custom = ratatui::style::Color::Rgb(0x1a, 0x1b, 0x26);
        let background = set_config_value(&mut app, "background_color", "#1a1b26", false);
        assert!(!background.is_error, "{:?}", background.message);
        assert_eq!(app.background_color_override, Some(custom));

        let preview = set_config_value(&mut app, "theme", "dark", false);
        assert!(!preview.is_error, "{:?}", preview.message);
        assert_eq!(app.background_color_override, Some(custom));
        assert_eq!(app.ui_theme.surface_bg, custom);

        let solarized_preview = set_config_value(&mut app, "theme", "solarized-light", false);
        assert!(
            !solarized_preview.is_error,
            "{:?}",
            solarized_preview.message
        );
        assert_eq!(app.background_color_override, Some(custom));
        assert_eq!(app.ui_theme.surface_bg, custom);
        assert!(crate::tui::ocean::OceanRamp::for_theme(&app.ui_theme).is_none());

        let saved_theme = set_config_value(&mut app, "theme", "dark", true);
        assert!(!saved_theme.is_error, "{:?}", saved_theme.message);
        assert_eq!(app.background_color_override, Some(custom));
        assert_eq!(app.ui_theme.surface_bg, custom);
        let persisted = Settings::load_persisted().expect("persisted settings");
        assert_eq!(persisted.theme, "dark");
        assert_eq!(
            persisted.background_color, None,
            "saving a theme must not persist the session-only background"
        );
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
    fn unrelated_save_does_not_persist_no_animations_runtime_overlay() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-no-animations-save-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join(".deepseek")).expect("settings dir");
        let _guard = EnvGuard::new(&temp_root);
        fs::write(
            temp_root.join(".deepseek").join("settings.toml"),
            "low_motion = false\nfancy_animations = true\ntheme = \"system\"\n",
        )
        .expect("seed settings");
        // Safety: test-only environment mutation is serialized by EnvGuard.
        unsafe {
            env::set_var("NO_ANIMATIONS", "1");
        }

        let mut app = create_test_app();
        assert!(app.low_motion, "runtime overlay should reduce motion");
        assert!(
            !app.fancy_animations,
            "runtime overlay should disable ocean animations"
        );

        let result = set_config_value(&mut app, "theme", "grayscale", true);
        assert!(!result.is_error, "{:?}", result.message);
        let saved = Settings::load_persisted().expect("persisted settings");
        assert_eq!(saved.theme, "grayscale");
        assert!(
            !saved.low_motion,
            "NO_ANIMATIONS must not become a saved preference"
        );
        assert!(
            saved.fancy_animations,
            "NO_ANIMATIONS must not overwrite the saved animation preference"
        );
        assert!(app.low_motion);
        assert!(!app.fancy_animations);
    }

    #[test]
    fn preset_save_does_not_persist_runtime_environment_overlays() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-preset-env-overlay-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join(".deepseek")).expect("settings dir");
        let _guard = EnvGuard::new(&temp_root);
        fs::write(
            temp_root.join(".deepseek").join("settings.toml"),
            "low_motion = false\nfancy_animations = true\nsynchronized_output = \"auto\"\n",
        )
        .expect("seed settings");
        // NO_ANIMATIONS exercises the reported path. Ptyxis supplies an
        // unrelated effective-only field, making an accidental
        // apply_env_overrides()+save observable even though the calm preset
        // intentionally selects reduced motion itself.
        unsafe {
            env::set_var("NO_ANIMATIONS", "1");
            env::set_var("PTYXIS_VERSION", "50.0");
        }

        let mut app = create_test_app();
        let result = config_command(&mut app, Some("preset calm --save"));
        assert!(!result.is_error, "{:?}", result.message);

        let saved = Settings::load_persisted().expect("persisted settings");
        assert!(saved.low_motion, "calm preset should save reduced motion");
        assert!(
            !saved.fancy_animations,
            "calm preset should save static ocean chrome"
        );
        assert_eq!(
            saved.synchronized_output, "auto",
            "Ptyxis runtime override must not leak into a preset save"
        );
    }

    #[test]
    fn config_approval_mode_valid_values() {
        let dir = tempfile::tempdir().expect("isolated config dir");
        let mut app = create_test_app();
        app.config_path = Some(dir.path().join("config.toml"));
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
    fn config_approval_mode_save_persists_top_level_policy() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-approval-policy-save-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let config_path = temp_root.join("custom-config.toml");

        let mut app = create_test_app();
        app.config_path = Some(config_path.clone());
        let result = config_command(&mut app, Some("approval_mode suggest --save"));
        let msg = result.message.unwrap();
        let saved = fs::read_to_string(&config_path).unwrap();

        assert!(!result.is_error);
        assert_eq!(app.approval_mode, ApprovalMode::Suggest);
        assert_eq!(
            msg,
            format!(
                "approval_mode = Ask (saved to {} as approval_policy = \"on-request\")",
                config_path.display()
            )
        );
        assert!(saved.contains("approval_policy = \"on-request\""));

        let loaded = Config::load(Some(config_path.clone()), None).unwrap();
        assert_eq!(loaded.approval_policy.as_deref(), Some("on-request"));

        let mut restarted = create_test_app_with_config(&loaded);
        restarted.config_path = Some(config_path.clone());
        assert!(restarted.approval_policy_locked());
        assert!(!restarted.approval_policy_requirements_managed());
        let changed = config_command(&mut restarted, Some("approval_mode auto --save"));
        assert!(!changed.is_error, "{:?}", changed.message);
        assert_eq!(
            changed.action,
            Some(AppAction::ApprovalPolicyPersisted {
                policy: Some("auto".to_string())
            })
        );
        assert_eq!(restarted.approval_mode, ApprovalMode::Auto);
        let reloaded = Config::load(Some(config_path), None).unwrap();
        assert_eq!(reloaded.approval_policy.as_deref(), Some("auto"));
    }

    #[test]
    fn config_approval_policy_can_return_to_saved_tui_permission_default() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-approval-policy-tui-default-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(temp_root.join(".deepseek")).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let config_path = temp_root.join("custom-config.toml");
        fs::write(&config_path, "# keep\napproval_policy = \"auto\"\n").unwrap();
        fs::write(
            temp_root.join(".deepseek").join("settings.toml"),
            "permission_posture = \"full-access\"\n",
        )
        .unwrap();
        let loaded = Config::load(Some(config_path.clone()), None).unwrap();
        let mut app = create_test_app_with_config(&loaded);
        app.config_path = Some(config_path.clone());

        let result = set_config_value(&mut app, "approval_policy", "use-tui-default", true);

        assert!(!result.is_error, "{:?}", result.message);
        assert_eq!(app.approval_mode, ApprovalMode::Bypass);
        assert!(!app.approval_policy_locked());
        assert_eq!(
            result.action,
            Some(AppAction::ApprovalPolicyPersisted { policy: None })
        );
        let saved = fs::read_to_string(config_path).unwrap();
        assert!(saved.contains("# keep"));
        assert!(!saved.contains("approval_policy"));
    }

    #[test]
    fn config_approval_policy_full_access_adopts_tui_posture_and_releases_root_override() {
        let temp_root = env::temp_dir().join(format!(
            "codewhale-approval-policy-full-access-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(temp_root.join(".deepseek")).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let config_path = temp_root.join("custom-config.toml");
        fs::write(&config_path, "# keep\napproval_policy = \"on-request\"\n").unwrap();
        fs::write(
            temp_root.join(".deepseek").join("settings.toml"),
            "permission_posture = \"ask\"\n",
        )
        .unwrap();
        let loaded = Config::load(Some(config_path.clone()), None).unwrap();
        let mut app = create_test_app_with_config(&loaded);
        app.config_path = Some(config_path.clone());
        // The production constructor receives the path up front and marks a
        // user-owned root policy editable. This focused fixture attaches the
        // path after construction, so mirror that resolved ownership here.
        app.mark_approval_policy_locked();
        assert!(app.approval_policy_locked());

        let result = set_config_value(&mut app, "approval_policy", "full-access", true);

        assert!(!result.is_error, "{:?}", result.message);
        assert_eq!(app.approval_mode, ApprovalMode::Bypass);
        assert!(!app.approval_policy_locked());
        assert_eq!(
            result.action,
            Some(AppAction::ApprovalPolicyPersisted { policy: None })
        );
        let saved_config = fs::read_to_string(config_path).unwrap();
        assert!(saved_config.contains("# keep"));
        assert!(!saved_config.contains("approval_policy"));
        let saved_settings = Settings::load_persisted().expect("saved TUI settings");
        assert_eq!(
            saved_settings.permission_posture.as_deref(),
            Some("full-access")
        );
    }

    #[test]
    fn config_approval_mode_invalid_value() {
        let dir = tempfile::tempdir().expect("isolated config dir");
        let mut app = create_test_app();
        app.config_path = Some(dir.path().join("config.toml"));
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
    fn config_threshold_enables_and_updates_live_auto_compaction() {
        let _lock = lock_test_env();
        let mut app = create_test_app();
        app.auto_compact = false;
        app.auto_compact_user_configured = false;

        let result = config_command(&mut app, Some("auto_compact_threshold_percent 65"));

        assert!(!result.is_error, "{:?}", result.message);
        assert!(app.auto_compact);
        assert!(app.auto_compact_user_configured);
        assert_eq!(app.auto_compact_threshold_percent, 65.0);
        assert_eq!(
            app.compact_threshold,
            crate::route_budget::compaction_threshold_for_route_at_percent(
                app.api_provider,
                app.effective_model_for_budget(),
                app.active_route_limits,
                65.0,
            )
        );
        assert!(matches!(
            result.action,
            Some(AppAction::UpdateCompaction(_))
        ));
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
    fn config_composer_multiline_mode_updates_live_app() {
        let _lock = lock_test_env();
        let mut app = create_test_app();
        app.composer_multiline_mode = false;

        let result = config_command(&mut app, Some("composer_multiline_mode true"));

        assert!(!result.is_error, "{:?}", result.message);
        assert!(app.composer_multiline_mode);
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
        assert_eq!(app.onboarding, OnboardingState::Provider);
        assert!(app.onboarding_needs_api_key);
        assert!(app.onboarding_missing_key_recovery);
        assert_eq!(result.action, Some(AppAction::OpenProviderPicker));

        let updated = fs::read_to_string(config_path).unwrap();
        assert!(!updated.contains("api_key"));
    }

    #[test]
    fn logout_clears_only_exact_named_custom_provider_key() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-custom-logout-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let config_path = temp_root.join(".deepseek").join("config.toml");
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            "[providers.custom-a]\napi_key = \"a-key\"\n\n[providers.custom-b]\napi_key = \"b-key\"\n",
        )
        .unwrap();
        let mut app = create_test_app();
        app.set_provider_identity(ApiProvider::Custom, "custom-a");

        let result = logout(&mut app);

        assert!(result.message.is_some());
        let updated = fs::read_to_string(config_path).unwrap();
        assert!(!updated.contains("a-key"), "{updated}");
        assert!(updated.contains("b-key"), "{updated}");
    }

    #[test]
    fn named_custom_provider_url_write_fails_closed() {
        let mut app = create_test_app();
        app.set_provider_identity(ApiProvider::Custom, "custom-a");

        let result = config_command(
            &mut app,
            Some("provider_url http://127.0.0.1:18181/v1 --save"),
        );
        let message = result.message.expect("error message");

        assert!(
            message.contains("named [providers.<name>] table"),
            "{message}"
        );
    }
}
