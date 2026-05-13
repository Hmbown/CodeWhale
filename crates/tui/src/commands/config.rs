//! Config commands: config, settings, mode switches, trust, logout

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::CommandResult;
use crate::client::DeepSeekClient;
use crate::config::{COMMON_DEEPSEEK_MODELS, clear_api_key, normalize_model_name};
use crate::config_ui::{ConfigUiMode, parse_mode};
use crate::llm_client::LlmClient;
use crate::localization::resolve_locale;
use crate::models::{ContentBlock, Message, MessageRequest, MessageResponse, SystemPrompt};
use crate::settings::Settings;
use crate::tui::app::{App, AppAction, AppMode, OnboardingState, ReasoningEffort, SidebarFocus};
use crate::tui::approval::ApprovalMode;
use anyhow::Result;

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
        "approval_mode" | "approval" => Some(app.approval_mode.label().to_string()),
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
        "show_thinking" | "thinking" => {
            Some(if app.show_thinking { "true" } else { "false" }.to_string())
        }
        "mode" | "default_mode" => Some(app.mode.as_setting().to_string()),
        "max_history" | "history" => Some(app.max_input_history.to_string()),
        "sidebar_width" | "sidebar" => Some(app.sidebar_width_percent.to_string()),
        "sidebar_focus" | "focus" => Some(app.sidebar_focus.as_setting().to_string()),
        "composer_density" | "composer" => Some(density_display(app.composer_density).to_string()),
        "composer_border" | "border" => {
            Some(if app.composer_border { "true" } else { "false" }.to_string())
        }
        "transcript_spacing" | "spacing" => {
            Some(spacing_display(app.transcript_spacing).to_string())
        }
        "status_indicator" | "indicator" => Some(app.status_indicator.clone()),
        "cost_currency" | "currency" => Some(
            match app.cost_currency {
                crate::pricing::CostCurrency::Usd => "usd",
                crate::pricing::CostCurrency::Cny => "cny",
            }
            .to_string(),
        ),
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

/// Persist `tui.status_items` to `~/.deepseek/config.toml` without disturbing
/// the rest of the file. We round-trip through `toml::Value` so any keys we
/// don't know about (provider blocks, MCP, etc.) survive the write
/// untouched.
///
/// Returns the path written so the caller can surface it in a status toast.
pub fn persist_status_items(items: &[crate::config::StatusItem]) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    use std::fs;

    let path = config_toml_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let mut doc: toml::Value = if path.exists() {
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        toml::from_str(&raw)
            .with_context(|| format!("failed to parse config at {}", path.display()))?
    } else {
        toml::Value::Table(toml::value::Table::new())
    };

    let table = doc
        .as_table_mut()
        .context("config.toml root must be a table")?;
    let tui_entry = table
        .entry("tui".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    let tui_table = tui_entry
        .as_table_mut()
        .context("`tui` section in config.toml must be a table")?;
    let array = items
        .iter()
        .map(|item| toml::Value::String(item.key().to_string()))
        .collect::<Vec<_>>();
    tui_table.insert("status_items".to_string(), toml::Value::Array(array));

    let body = toml::to_string_pretty(&doc).context("failed to serialize config.toml")?;
    fs::write(&path, body)
        .with_context(|| format!("failed to write config at {}", path.display()))?;
    Ok(path)
}

pub fn persist_root_string_key(key: &str, value: &str) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    use std::fs;

    let path = config_toml_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let mut doc: toml::Value = if path.exists() {
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        toml::from_str(&raw)
            .with_context(|| format!("failed to parse config at {}", path.display()))?
    } else {
        toml::Value::Table(toml::value::Table::new())
    };
    let table = doc
        .as_table_mut()
        .context("config.toml root must be a table")?;
    table.insert(key.to_string(), toml::Value::String(value.to_string()));
    let body = toml::to_string_pretty(&doc).context("failed to serialize config.toml")?;
    fs::write(&path, body)
        .with_context(|| format!("failed to write config at {}", path.display()))?;
    Ok(path)
}

/// Resolve the path to `~/.deepseek/config.toml` (or
/// `$DEEPSEEK_CONFIG_PATH`). Mirrors what `Config::load` accepts so we
/// never write to a different file than the one we read.
pub(super) fn config_toml_path() -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    if let Ok(env) = std::env::var("DEEPSEEK_CONFIG_PATH") {
        let trimmed = env.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    let home = dirs::home_dir().context("failed to resolve home directory for config.toml path")?;
    Ok(home.join(".deepseek").join("config.toml"))
}

/// Modify a setting at runtime
pub fn set_config_value(app: &mut App, key: &str, value: &str, persist: bool) -> CommandResult {
    let key = key.to_lowercase();

    match key.as_str() {
        "model" => {
            // Support "/model auto" — auto-select model based on request complexity
            if value.trim().eq_ignore_ascii_case("auto") {
                app.auto_model = true;
                app.model = "auto".to_string();
                app.last_effective_model = None;
                app.reasoning_effort = ReasoningEffort::Auto;
                app.last_effective_reasoning_effort = None;
                app.update_model_compaction_budget();
                app.session.last_prompt_tokens = None;
                app.session.last_completion_tokens = None;
                return CommandResult::with_message_and_action(
                    "model = auto (auto-select model and thinking per turn)".to_string(),
                    AppAction::UpdateCompaction(app.compaction_config()),
                );
            }
            // Clear auto mode when a specific model is set
            app.auto_model = false;
            app.last_effective_model = None;
            let Some(model) = normalize_model_name(value) else {
                return CommandResult::error(format!(
                    "Invalid model '{value}'. Expected a DeepSeek model ID. Common models: {}",
                    COMMON_DEEPSEEK_MODELS.join(", ")
                ));
            };
            app.model = model.clone();
            app.update_model_compaction_budget();
            app.session.last_prompt_tokens = None;
            app.session.last_completion_tokens = None;
            return CommandResult::with_message_and_action(
                format!("model = {model}"),
                AppAction::UpdateCompaction(app.compaction_config()),
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
        "mcp_config_path" | "mcp" => {
            if value.trim().is_empty() {
                return CommandResult::error("mcp_config_path cannot be empty");
            }
            app.mcp_config_path = PathBuf::from(expand_tilde(value));
            app.mcp_restart_required = true;
            let message = if persist {
                match persist_root_string_key("mcp_config_path", value) {
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

    let mut action = None;
    match key.as_str() {
        "auto_compact" | "compact" => {
            app.auto_compact = settings.auto_compact;
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
        "status_indicator" | "indicator" => {
            app.status_indicator = settings.status_indicator.clone();
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
            app.needs_redraw = true;
        }
        "theme" | "ui_theme" | "background_color" | "background" | "bg" => {
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
        "paste_burst_detection" | "paste_burst" => {
            app.use_paste_burst_detection = settings.paste_burst_detection;
            if !app.use_paste_burst_detection {
                app.paste_burst.clear_after_explicit_paste();
            }
        }
        "transcript_spacing" | "spacing" => {
            app.transcript_spacing =
                crate::tui::app::TranscriptSpacing::from_setting(&settings.transcript_spacing);
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
                app.auto_model = model.trim().eq_ignore_ascii_case("auto");
                app.model.clone_from(model);
                app.last_effective_model = None;
                if app.auto_model {
                    app.reasoning_effort = ReasoningEffort::Auto;
                    app.last_effective_reasoning_effort = None;
                }
                app.update_model_compaction_budget();
                app.session.last_prompt_tokens = None;
                app.session.last_completion_tokens = None;
                action = Some(AppAction::UpdateCompaction(app.compaction_config()));
            }
        }
        "sidebar_width" | "sidebar" => {
            app.sidebar_width_percent = settings.sidebar_width_percent;
            app.mark_history_updated();
        }
        "sidebar_focus" | "focus" => {
            app.set_sidebar_focus(SidebarFocus::from_setting(&settings.sidebar_focus));
        }
        _ => {}
    }

    let display_value = match key.as_str() {
        "default_mode" | "mode" => settings.default_mode.clone(),
        "cost_currency" | "currency" => settings.cost_currency.clone(),
        "theme" | "ui_theme" => settings.theme.clone(),
        "background_color" | "background" | "bg" => settings
            .background_color
            .clone()
            .unwrap_or_else(|| "default".to_string()),
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

/// Modify a setting at runtime
#[allow(dead_code)]
pub fn set_config(app: &mut App, args: Option<&str>) -> CommandResult {
    let Some(args) = args else {
        let available = Settings::available_settings()
            .iter()
            .map(|(k, d)| format!("  {k}: {d}"))
            .collect::<Vec<_>>()
            .join("\n");
        return CommandResult::message(format!(
            "Usage: /set <key> <value>\n\n\
             Available settings:\n{available}\n\n\
             Session-only settings:\n  \
             model: Current model\n  \
             approval_mode: auto | suggest | never\n\n\
             Add --save to persist to settings file."
        ));
    };

    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    if parts.len() < 2 {
        return CommandResult::error("Usage: /set <key> <value>");
    }

    let key = parts[0].to_lowercase();
    let (value, should_save) = if parts[1].ends_with(" --save") {
        (parts[1].trim_end_matches(" --save").trim(), true)
    } else {
        (parts[1].trim(), false)
    };

    set_config_value(app, &key, value, should_save)
}

/// Select the TUI operating mode.
pub fn mode(app: &mut App, arg: Option<&str>) -> CommandResult {
    let Some(arg) = arg.filter(|value| !value.trim().is_empty()) else {
        return CommandResult::action(AppAction::OpenModePicker);
    };
    match parse_mode_arg(arg) {
        Some(mode) => CommandResult::message(switch_mode(app, mode)),
        None => CommandResult::error("Usage: /mode [agent|plan|yolo|1|2|3]"),
    }
}

pub fn switch_mode(app: &mut App, mode: AppMode) -> String {
    if app.set_mode(mode) {
        format!("Switched to {} mode.", mode_display_name(mode))
    } else {
        format!("Already in {} mode.", mode_display_name(mode))
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

/// Switch the runtime theme. `/set theme <value> --save` persists it.
pub fn theme(app: &mut App, arg: Option<&str>) -> CommandResult {
    let requested = match arg.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => {
            let Some(theme) = crate::palette::normalize_theme_name(value) else {
                return CommandResult::error("Usage: /theme [dark|light|grayscale|system]");
            };
            theme
        }
        None => match app.ui_theme.mode {
            crate::palette::PaletteMode::Dark => "light",
            crate::palette::PaletteMode::Light => "grayscale",
            crate::palette::PaletteMode::Grayscale => "dark",
        },
    };

    let background = Settings::load()
        .ok()
        .and_then(|settings| settings.background_color);
    app.ui_theme = crate::palette::ui_theme_from_settings(requested, background.as_deref());
    app.needs_redraw = true;

    let label = crate::palette::theme_label_for_mode(app.ui_theme.mode);
    if requested == "system" {
        CommandResult::message(format!("Theme switched to system ({label})."))
    } else {
        CommandResult::message(format!("Theme switched to {label}."))
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

/// Auto-select a model based on request complexity.
///
/// Short messages (<100 chars) → Flash (fast & cheap).
/// Long messages (>500 chars) → Pro (powerful reasoning).
/// Messages with complex keywords → Pro.
/// Default → Flash (cost savings).
pub fn auto_model_heuristic(input: &str, _current_model: &str) -> String {
    let len = input.chars().count();
    let lower = input.to_lowercase();
    if COMPLEX_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
        return "deepseek-v4-pro".to_string();
    }
    // Short messages → Flash
    if len < 100 {
        return "deepseek-v4-flash".to_string();
    }
    // Long complex requests → Pro
    if len > 500 {
        return "deepseek-v4-pro".to_string();
    }
    // Default to Flash for cost savings
    "deepseek-v4-flash".to_string()
}

/// Keywords that escalate `auto`-mode model selection to
/// `deepseek-v4-pro`. The Latin entries are lowercase (the caller
/// lowercases the message); CJK has no case so the literal form
/// matches as-is.
///
/// Without the CJK entries, a Chinese-speaking user typing
/// "帮我重构这个模块" or "审计安全漏洞" silently fell through to the
/// short/long-message threshold and usually landed on Flash even
/// for tasks that obviously need Pro-grade reasoning.
const COMPLEX_KEYWORDS: &[&str] = &[
    // English (unchanged from the original list).
    "refactor",
    "architecture",
    "design",
    "debug",
    "security",
    "review",
    "audit",
    "migrate",
    "optimize",
    "rewrite",
    "implement",
    "analyze",
    // Simplified Chinese.
    "\u{91cd}\u{6784}", // 重构
    "\u{67b6}\u{6784}", // 架构
    "\u{8bbe}\u{8ba1}", // 设计
    "\u{8c03}\u{8bd5}", // 调试
    "\u{5b89}\u{5168}", // 安全
    "\u{5ba1}\u{67e5}", // 审查
    "\u{5ba1}\u{8ba1}", // 审计
    "\u{8fc1}\u{79fb}", // 迁移
    "\u{4f18}\u{5316}", // 优化
    "\u{91cd}\u{5199}", // 重写
    "\u{5b9e}\u{73b0}", // 实现
    "\u{5206}\u{6790}", // 分析
    // Traditional Chinese variants where they differ.
    "\u{91cd}\u{69cb}", // 重構
    "\u{67b6}\u{69cb}", // 架構
    "\u{8a2d}\u{8a08}", // 設計
    "\u{8abf}\u{8a66}", // 調試
    "\u{5be9}\u{67e5}", // 審查
    "\u{5be9}\u{8a08}", // 審計
    "\u{9077}\u{79fb}", // 遷移
    "\u{512a}\u{5316}", // 優化
    "\u{91cd}\u{5beb}", // 重寫
    "\u{5be6}\u{73fe}", // 實現
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoRouteRecommendation {
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoRouteSource {
    FlashRouter,
    Heuristic,
}

impl AutoRouteSource {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            AutoRouteSource::FlashRouter => "flash-router",
            AutoRouteSource::Heuristic => "heuristic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoRouteSelection {
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub source: AutoRouteSource,
}

pub const AUTO_MODEL_ROUTER_SYSTEM_PROMPT: &str = "\
You are the DeepSeek TUI auto-routing classifier. Return only compact JSON: \
{\"model\":\"deepseek-v4-flash|deepseek-v4-pro\",\"thinking\":\"off|high|max\"}. \
Use deepseek-v4-flash for trivial, conversational, status, or single-step work. \
Use deepseek-v4-pro for coding, debugging, release work, multi-step tasks, high-risk decisions, \
tool-heavy work, ambiguous requests, or anything that benefits from deeper reasoning. \
Use thinking off only for trivial no-tool answers, high for ordinary reasoning, and max for \
agentic, coding, multi-file, release, architecture, debugging, security, tool-heavy, or uncertain work.";

/// Parse the Flash router's JSON-only response.
///
/// The runtime treats classifier output as untrusted: only known V4 model IDs
/// and supported reasoning tiers are accepted. Anything else falls back to the
/// deterministic heuristic.
pub fn parse_auto_route_recommendation(raw: &str) -> Option<AutoRouteRecommendation> {
    let json = extract_first_json_object(raw)?;
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let model = value.get("model").and_then(serde_json::Value::as_str)?;
    let model = normalize_auto_route_model(model)?;
    let reasoning_effort = value
        .get("thinking")
        .or_else(|| value.get("reasoning_effort"))
        .or_else(|| value.get("effort"))
        .and_then(serde_json::Value::as_str)
        .and_then(parse_auto_route_reasoning_effort);

    Some(AutoRouteRecommendation {
        model: model.to_string(),
        reasoning_effort,
    })
}

fn extract_first_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    (end >= start).then_some(&raw[start..=end])
}

fn normalize_auto_route_model(model: &str) -> Option<&'static str> {
    match model.trim().to_ascii_lowercase().as_str() {
        "deepseek-v4-pro" | "v4-pro" | "pro" => Some("deepseek-v4-pro"),
        "deepseek-v4-flash" | "v4-flash" | "flash" => Some("deepseek-v4-flash"),
        _ => None,
    }
}

fn parse_auto_route_reasoning_effort(effort: &str) -> Option<ReasoningEffort> {
    match effort.trim().to_ascii_lowercase().as_str() {
        "off" | "disabled" | "none" | "false" => Some(ReasoningEffort::Off),
        "low" | "minimal" | "medium" | "mid" => Some(ReasoningEffort::High),
        "high" => Some(ReasoningEffort::High),
        "max" | "maximum" | "xhigh" => Some(ReasoningEffort::Max),
        _ => None,
    }
}

#[must_use]
pub fn normalize_auto_route_effort(effort: ReasoningEffort) -> ReasoningEffort {
    match effort {
        ReasoningEffort::Low | ReasoningEffort::Medium => ReasoningEffort::High,
        other => other,
    }
}

pub async fn resolve_auto_route_with_flash(
    config: &crate::config::Config,
    latest_request: &str,
    recent_context: &str,
    selected_model_mode: &str,
    selected_thinking_mode: &str,
) -> AutoRouteSelection {
    match auto_route_flash_recommendation(
        config,
        latest_request,
        recent_context,
        selected_model_mode,
        selected_thinking_mode,
    )
    .await
    {
        Ok(Some(recommendation)) => AutoRouteSelection {
            model: recommendation.model,
            reasoning_effort: recommendation.reasoning_effort,
            source: AutoRouteSource::FlashRouter,
        },
        Ok(None) | Err(_) => fallback_auto_route(latest_request, selected_model_mode),
    }
}

fn fallback_auto_route(latest_request: &str, selected_model_mode: &str) -> AutoRouteSelection {
    AutoRouteSelection {
        model: auto_model_heuristic(latest_request, selected_model_mode),
        reasoning_effort: Some(normalize_auto_route_effort(crate::auto_reasoning::select(
            false,
            latest_request,
        ))),
        source: AutoRouteSource::Heuristic,
    }
}

async fn auto_route_flash_recommendation(
    config: &crate::config::Config,
    latest_request: &str,
    recent_context: &str,
    selected_model_mode: &str,
    selected_thinking_mode: &str,
) -> Result<Option<AutoRouteRecommendation>> {
    if cfg!(test) {
        return Ok(None);
    }

    let client = DeepSeekClient::new(config)?;
    let request = MessageRequest {
        model: "deepseek-v4-flash".to_string(),
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: auto_route_prompt(
                    latest_request,
                    recent_context,
                    selected_model_mode,
                    selected_thinking_mode,
                ),
                cache_control: None,
            }],
        }],
        max_tokens: 96,
        system: Some(SystemPrompt::Text(
            AUTO_MODEL_ROUTER_SYSTEM_PROMPT.to_string(),
        )),
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort: Some("off".to_string()),
        stream: Some(false),
        temperature: Some(0.0),
        top_p: None,
    };

    let response =
        tokio::time::timeout(Duration::from_secs(4), client.create_message(request)).await??;
    Ok(parse_auto_route_recommendation(&message_response_text(
        &response,
    )))
}

fn auto_route_prompt(
    latest_request: &str,
    recent_context: &str,
    selected_model_mode: &str,
    selected_thinking_mode: &str,
) -> String {
    format!(
        "Session mode: agent\nSelected model mode: {}\nSelected thinking mode: {}\n\nRecent context:\n{}\n\nLatest user request:\n{}\n\nReturn JSON only.",
        selected_model_mode,
        selected_thinking_mode,
        if recent_context.trim().is_empty() {
            "No prior context."
        } else {
            recent_context
        },
        truncate_for_auto_router(latest_request, 4_000)
    )
}

fn message_response_text(response: &MessageResponse) -> String {
    let mut out = String::new();
    for block in &response.content {
        match block {
            ContentBlock::Text { text, .. } | ContentBlock::ToolResult { content: text, .. } => {
                append_router_text(&mut out, text);
            }
            ContentBlock::Thinking { thinking } => {
                append_router_text(&mut out, thinking);
            }
            ContentBlock::ToolUse { name, .. } => {
                append_router_text(&mut out, &format!("[tool call: {name}]"));
            }
            _ => {}
        }
    }
    out
}

fn append_router_text(out: &mut String, text: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(text);
}

fn truncate_for_auto_router(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
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

/// Logout - clear API key and return to onboarding
pub fn logout(app: &mut App) -> CommandResult {
    match clear_api_key() {
        Ok(()) => {
            app.onboarding = OnboardingState::ApiKey;
            app.onboarding_needs_api_key = true;
            app.api_key_input.clear();
            app.api_key_cursor = 0;
            CommandResult::message("Logged out. Enter a new API key to continue.")
        }
        Err(e) => CommandResult::error(format!("Failed to clear API key: {e}")),
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

    struct EnvGuard {
        home: Option<OsString>,
        userprofile: Option<OsString>,
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
            let deepseek_config_prev = env::var_os("DEEPSEEK_CONFIG_PATH");

            // Safety: test-only environment mutation guarded by process-wide mutex.
            unsafe {
                env::set_var("HOME", &home_str);
                env::set_var("USERPROFILE", &home_str);
                env::set_var("DEEPSEEK_CONFIG_PATH", &config_str);
            }

            Self {
                home: home_prev,
                userprofile: userprofile_prev,
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
        App::new(options, &Config::default())
    }

    #[test]
    fn test_mode_yolo_sets_all_flags() {
        let mut app = create_test_app();
        // Switch to Agent first to guarantee a clean starting state regardless of
        // user settings on the host machine.
        let _ = mode(&mut app, Some("agent"));
        let result = mode(&mut app, Some("yolo"));
        assert!(result.message.unwrap().contains("Switched to YOLO mode"));
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
        let _ = mode(&mut app, Some("2"));
        assert_eq!(app.mode, AppMode::Plan);
        let _ = mode(&mut app, Some("3"));
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
    fn test_set_without_args_shows_usage() {
        let mut app = create_test_app();
        let result = set_config(&mut app, None);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Usage: /set"));
        assert!(msg.contains("Available settings:"));
    }

    #[test]
    fn test_set_model_updates_app_state() {
        let mut app = create_test_app();
        let _old_model = app.model.clone();
        let result = set_config(&mut app, Some("model deepseek-v4-flash"));
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
    fn test_set_model_auto_enables_auto_thinking() {
        let mut app = create_test_app();
        app.reasoning_effort = ReasoningEffort::Off;

        let result = set_config(&mut app, Some("model auto"));

        assert!(result.message.is_some());
        assert!(app.auto_model);
        assert_eq!(app.model, "auto");
        assert_eq!(app.reasoning_effort, ReasoningEffort::Auto);
        assert!(app.last_effective_model.is_none());
        assert!(app.last_effective_reasoning_effort.is_none());
    }

    #[test]
    fn test_set_model_accepts_future_deepseek_model_id() {
        let mut app = create_test_app();
        let result = set_config(&mut app, Some("model deepseek-v4"));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("model = deepseek-v4"));
        assert_eq!(app.model, "deepseek-v4");
    }

    #[test]
    fn test_set_model_with_save_flag() {
        let mut app = create_test_app();
        let _result = set_config(&mut app, Some("model deepseek-v4-flash --save"));
        // Note: This test may fail in environments where settings can't be saved
        // The important thing is that the model is updated
        assert_eq!(app.model, "deepseek-v4-flash");
    }

    #[test]
    fn auto_model_heuristic_chinese_keywords_route_to_pro() {
        // Without these keywords, a Chinese user typing
        // "帮我重构这个模块" (37 chars in chars().count() terms after
        // the leading helper text) fell through to the short-message
        // Flash branch even though the intent is obviously Pro-tier.
        for msg in [
            "\u{5e2e}\u{6211}\u{91cd}\u{6784}\u{8fd9}\u{4e2a}\u{6a21}\u{5757}", // 帮我重构这个模块
            "\u{8bbe}\u{8ba1}\u{6570}\u{636e}\u{5e93}\u{67b6}\u{6784}",         // 设计数据库架构
            "\u{8c03}\u{8bd5}\u{5d29}\u{6e83}\u{95ee}\u{9898}",                 // 调试崩溃问题
            "\u{5ba1}\u{8ba1}\u{5b89}\u{5168}\u{6f0f}\u{6d1e}",                 // 审计安全漏洞
            "\u{8fc1}\u{79fb}\u{5230}\u{65b0}\u{6846}\u{67b6}",                 // 迁移到新框架
            "\u{4f18}\u{5316}\u{6027}\u{80fd}\u{74f6}\u{9888}",                 // 优化性能瓶颈
            "\u{5206}\u{6790}\u{8fd9}\u{6bb5}\u{4ee3}\u{7801}",                 // 分析这段代码
        ] {
            assert_eq!(
                auto_model_heuristic(msg, "auto"),
                "deepseek-v4-pro",
                "expected Pro for `{msg}`",
            );
        }
    }

    #[test]
    fn auto_model_heuristic_traditional_chinese_keywords_route_to_pro() {
        for msg in [
            "\u{8acb}\u{91cd}\u{69cb}\u{6b64}\u{6a21}\u{7d44}", // 請重構此模組
            "\u{67b6}\u{69cb}\u{8a2d}\u{8a08}",                 // 架構設計
            "\u{4ee3}\u{78bc}\u{8abf}\u{8a66}",                 // 代碼調試
            "\u{5be9}\u{8a08}\u{6f0f}\u{6d1e}",                 // 審計漏洞
            "\u{9077}\u{79fb}\u{5230}\u{65b0}\u{67b6}\u{69cb}", // 遷移到新架構
            "\u{512a}\u{5316}\u{6027}\u{80fd}",                 // 優化性能
            "\u{91cd}\u{5beb}\u{4ee3}\u{78bc}",                 // 重寫代碼
            "\u{5be6}\u{73fe}\u{65b0}\u{529f}\u{80fd}",         // 實現新功能
        ] {
            assert_eq!(
                auto_model_heuristic(msg, "auto"),
                "deepseek-v4-pro",
                "expected Pro for `{msg}`",
            );
        }
    }

    #[test]
    fn auto_model_heuristic_short_chinese_chat_stays_on_flash() {
        // Sanity: a short non-keyword Chinese message still falls
        // through to the cost-saving Flash branch.
        // "你好" (2 chars) — well under the 100-char Flash floor.
        assert_eq!(
            auto_model_heuristic("\u{4f60}\u{597d}", "auto"),
            "deepseek-v4-flash",
        );
    }

    #[test]
    fn auto_route_recommendation_parses_strict_json() {
        let rec =
            parse_auto_route_recommendation(r#"{"model":"deepseek-v4-pro","thinking":"max"}"#)
                .expect("valid router response should parse");

        assert_eq!(rec.model, "deepseek-v4-pro");
        assert_eq!(rec.reasoning_effort, Some(ReasoningEffort::Max));
    }

    #[test]
    fn auto_route_recommendation_accepts_wrapped_json_aliases() {
        let rec =
            parse_auto_route_recommendation(r#"route: {"model":"flash","reasoning_effort":"off"}"#)
                .expect("wrapped router response should parse");

        assert_eq!(rec.model, "deepseek-v4-flash");
        assert_eq!(rec.reasoning_effort, Some(ReasoningEffort::Off));
    }

    #[test]
    fn auto_route_recommendation_normalizes_legacy_low_medium_to_high() {
        let rec = parse_auto_route_recommendation(
            r#"{"model":"deepseek-v4-pro","reasoning_effort":"medium"}"#,
        )
        .expect("medium should parse for back-compat");

        assert_eq!(rec.model, "deepseek-v4-pro");
        assert_eq!(rec.reasoning_effort, Some(ReasoningEffort::High));
    }

    #[test]
    fn auto_route_recommendation_rejects_unknown_model() {
        assert!(
            parse_auto_route_recommendation(r#"{"model":"some-other-model","thinking":"max"}"#,)
                .is_none()
        );
    }

    #[test]
    fn test_set_default_mode_normal_save_reports_normalized_value() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "deepseek-tui-default-mode-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let mut app = create_test_app();
        let result = set_config(&mut app, Some("default_mode normal --save"));
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
            "deepseek-tui-cost-currency-test-{}-{}",
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
    fn theme_command_accepts_grayscale_arg() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "deepseek-tui-theme-command-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let mut app = create_test_app();
        let result = theme(&mut app, Some("grayscale"));

        assert_eq!(result.message.unwrap(), "Theme switched to grayscale.");
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
            "deepseek-tui-theme-save-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let mut app = create_test_app();
        let result = set_config(&mut app, Some("theme grayscale --save"));
        let msg = result.message.unwrap();

        assert_eq!(msg, "theme = grayscale (saved)");
        assert_eq!(app.ui_theme.mode, crate::palette::PaletteMode::Grayscale);

        let settings_path = Settings::path().unwrap();
        let saved = fs::read_to_string(settings_path).unwrap();
        assert!(saved.contains("theme = \"grayscale\""));
    }

    #[test]
    fn test_set_approval_mode_valid_values() {
        let mut app = create_test_app();
        // Test auto
        let result = set_config(&mut app, Some("approval_mode auto"));
        assert!(result.message.is_some());
        assert_eq!(app.approval_mode, ApprovalMode::Auto);

        // Test suggest
        let result = set_config(&mut app, Some("approval_mode suggest"));
        assert!(result.message.is_some());
        assert_eq!(app.approval_mode, ApprovalMode::Suggest);

        // Test never
        let result = set_config(&mut app, Some("approval_mode never"));
        assert!(result.message.is_some());
        assert_eq!(app.approval_mode, ApprovalMode::Never);
    }

    #[test]
    fn test_set_approval_mode_invalid_value() {
        let mut app = create_test_app();
        let result = set_config(&mut app, Some("approval_mode invalid"));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Invalid approval_mode"));
    }

    #[test]
    fn test_set_without_save_flag() {
        let _lock = lock_test_env();
        let mut app = create_test_app();
        let result = set_config(&mut app, Some("auto_compact true"));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("(session only"));
    }

    #[test]
    fn test_set_composer_border_updates_live_app() {
        let _lock = lock_test_env();
        let mut app = create_test_app();
        app.composer_border = true;

        let result = set_config(&mut app, Some("composer_border false"));

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
            "deepseek-tui-logout-test-{}-{}",
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

    #[test]
    fn test_set_invalid_setting() {
        let _lock = lock_test_env();
        let mut app = create_test_app();
        let _result = set_config(&mut app, Some("nonexistent value"));
        // Should either error or handle as session setting
        // The current implementation tries to set it in Settings
        // which may succeed or fail depending on Settings implementation
    }

    #[test]
    fn test_set_key_without_value() {
        let mut app = create_test_app();
        let result = set_config(&mut app, Some("model"));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Usage: /set"));
    }

    #[test]
    fn persist_status_items_writes_tui_section_to_config_toml() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "deepseek-statusline-persist-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let items = vec![
            crate::config::StatusItem::Mode,
            crate::config::StatusItem::Model,
            crate::config::StatusItem::Cost,
        ];

        let path = persist_status_items(&items).expect("persist should succeed");
        let body = fs::read_to_string(&path).expect("written file should be readable");
        assert!(body.contains("[tui]"), "expected [tui] section in {body}");
        assert!(
            body.contains("status_items"),
            "expected status_items key in {body}"
        );
        assert!(body.contains("\"mode\""), "expected mode key in {body}");
        assert!(body.contains("\"cost\""), "expected cost key in {body}");
    }

    #[test]
    fn persist_status_items_preserves_existing_unrelated_keys() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "deepseek-statusline-preserve-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let path = temp_root.join(".deepseek").join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Seed the config with a sentinel key the picker MUST NOT clobber.
        fs::write(
            &path,
            "api_key = \"sentinel-key\"\nmodel = \"deepseek-v4-pro\"\n",
        )
        .unwrap();

        let written = persist_status_items(&[crate::config::StatusItem::Mode])
            .expect("persist should succeed");
        let body = fs::read_to_string(&written).expect("written file should be readable");
        assert!(
            body.contains("api_key = \"sentinel-key\""),
            "round-trip lost api_key: {body}"
        );
        assert!(
            body.contains("model = \"deepseek-v4-pro\""),
            "round-trip lost model: {body}"
        );
        assert!(
            body.contains("status_items"),
            "expected status_items in {body}"
        );
    }
}
