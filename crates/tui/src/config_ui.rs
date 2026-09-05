use std::collections::BTreeMap;
#[cfg(feature = "web")]
use std::net::SocketAddr;
#[cfg(feature = "web")]
use std::time::Duration;

use anyhow::{Context, Result, bail};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands;
use crate::config::{
    Config, StatusItem, normalize_custom_model_id, normalize_model_name_for_provider,
    validate_route,
};
use crate::localization::{normalize_configured_locale, resolve_locale};
use crate::settings::Settings;
use crate::tui::app::{App, AppMode, ComposerDensity, ReasoningEffort, TranscriptSpacing};
use crate::tui::approval::ApprovalMode;

#[cfg(feature = "web")]
use schemaui::web::session::{ServeOptions, WebSessionBuilder, bind_session};
#[cfg(feature = "tui")]
use schemaui::{FrontendOptions, SchemaUI, UiOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigUiMode {
    Native,
    Tui,
    Web,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ConfigUiDocument {
    pub runtime: RuntimeSection,
    pub settings: SettingsSection,
    pub config: ConfigSection,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct RuntimeSection {
    #[schemars(
        title = "Active provider",
        description = "Route fact only. Switch providers with /provider, then return here to tune this session."
    )]
    pub provider: String,
    #[schemars(
        title = "Current model",
        description = "Model used by the active provider for this session. Save to apply the exact route choice."
    )]
    pub model: String,
    pub approval_mode: ApprovalModeValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SettingsSection {
    pub auto_compact: bool,
    pub calm_mode: bool,
    pub low_motion: bool,
    pub fancy_animations: bool,
    pub focus_texture: FocusTextureValue,
    pub work_surface_placement: WorkSurfacePlacementValue,
    #[schemars(range(min = 2, max = 16))]
    pub work_surface_top_height: u16,
    #[schemars(range(min = 26, max = 80))]
    pub work_surface_side_width: u16,
    pub paste_burst_detection: bool,
    pub show_thinking: bool,
    pub thinking_default_expanded: bool,
    #[schemars(range(min = 0, max = 40))]
    pub thinking_preview_lines: usize,
    pub thinking_highlight: bool,
    pub help_expand_groups: bool,
    pub pin_last_prompt: bool,
    pub show_tool_details: bool,
    pub inline_diffs: InlineDiffValue,
    #[schemars(
        title = "UI locale",
        description = "Locale used by the TUI. Every shipped locale pack holds full English parity; nothing falls back."
    )]
    pub locale: UiLocale,
    #[schemars(
        title = "Theme",
        description = "Compiled theme name, or custom:<name> for a theme from the Codewhale themes directory."
    )]
    pub theme: UiThemeValue,
    #[schemars(
        title = "Background color",
        description = "Optional Blue Stage background override as #RRGGBB. Leave empty to keep the named theme."
    )]
    pub background_color: Option<String>,
    pub bracketed_paste: bool,
    pub composer_density: ComposerDensityValue,
    pub composer_border: bool,
    #[schemars(
        title = "Multiline composer mode",
        description = "When enabled, Enter inserts a newline and Shift+Enter sends. Ctrl+J and Alt+Enter remain newline shortcuts."
    )]
    pub composer_multiline_mode: bool,
    pub composer_vim_mode: ComposerVimModeValue,
    #[schemars(range(min = 0))]
    pub mention_menu_limit: usize,
    pub mention_menu_behavior: MentionMenuBehaviorValue,
    #[schemars(range(min = 0))]
    pub mention_walk_depth: usize,
    pub transcript_spacing: TranscriptSpacingValue,
    pub status_indicator: StatusIndicatorValue,
    pub synchronized_output: SynchronizedOutputValue,
    pub default_mode: DefaultModeValue,
    pub context_panel: bool,
    #[schemars(range(min = 0))]
    pub max_history: usize,
    pub cost_currency: CostCurrencyValue,
    #[schemars(
        title = "Follow symlinks",
        description = "Allow workspace discovery to cross symbolic links. Enable only when linked projects are part of this workspace."
    )]
    pub workspace_follow_symlinks: bool,
    #[schemars(
        title = "Provider model overrides",
        description = "Durable model choices by provider identity. The active provider's entry follows Current model."
    )]
    pub provider_models: BTreeMap<String, String>,
    pub default_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ConfigSection {
    pub mcp_config_path: String,
    pub reasoning_effort: ReasoningEffortValue,
    #[schemars(title = "Status line items")]
    pub status_items: Vec<StatusItemValue>,
}

#[derive(Debug, Clone)]
pub struct ConfigUiApplyOutcome {
    pub changed: bool,
    pub final_message: String,
    pub requires_engine_sync: bool,
}

#[cfg(feature = "web")]
#[derive(Debug)]
pub struct WebConfigSession {
    #[allow(dead_code)]
    task: tokio::task::JoinHandle<()>,
    pub receiver: tokio::sync::mpsc::UnboundedReceiver<WebConfigSessionEvent>,
    pub addr: SocketAddr,
}

#[cfg(not(feature = "web"))]
#[derive(Debug)]
pub struct WebConfigSession {
    #[allow(dead_code)]
    pub receiver: tokio::sync::mpsc::UnboundedReceiver<WebConfigSessionEvent>,
}

#[cfg(test)]
impl WebConfigSession {
    pub(crate) fn for_test(
        receiver: tokio::sync::mpsc::UnboundedReceiver<WebConfigSessionEvent>,
    ) -> Self {
        #[cfg(feature = "web")]
        {
            Self {
                task: tokio::spawn(async {}),
                receiver,
                addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            }
        }
        #[cfg(not(feature = "web"))]
        {
            Self { receiver }
        }
    }
}

#[cfg_attr(not(feature = "web"), allow(dead_code))]
#[derive(Debug, Clone)]
pub enum WebConfigSessionEvent {
    Draft(ConfigUiDocument),
    Committed(ConfigUiDocument),
    Failed(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalModeValue {
    Auto,
    Bypass,
    Suggest,
    Never,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum UiLocale {
    #[serde(rename = "auto")]
    #[schemars(rename = "auto")]
    Auto,
    #[serde(rename = "en")]
    #[schemars(rename = "en")]
    En,
    #[serde(rename = "ja")]
    #[schemars(rename = "ja")]
    Ja,
    #[serde(rename = "zh-Hans")]
    #[schemars(rename = "zh-Hans")]
    ZhHans,
    #[serde(rename = "zh-Hant")]
    #[schemars(rename = "zh-Hant")]
    ZhHant,
    #[serde(rename = "pt-BR")]
    #[schemars(rename = "pt-BR")]
    PtBr,
    #[serde(rename = "es-419")]
    #[schemars(rename = "es-419")]
    Es419,
    #[serde(rename = "vi")]
    #[schemars(rename = "vi")]
    Vi,
    #[serde(rename = "ko")]
    #[schemars(rename = "ko")]
    Ko,
    #[serde(rename = "ca")]
    #[schemars(rename = "ca")]
    Ca,
    #[serde(rename = "de")]
    #[schemars(rename = "de")]
    De,
    #[serde(rename = "fr")]
    #[schemars(rename = "fr")]
    Fr,
    #[serde(rename = "id")]
    #[schemars(rename = "id")]
    Id,
    #[serde(rename = "hi")]
    #[schemars(rename = "hi")]
    Hi,
    #[serde(rename = "ru")]
    #[schemars(rename = "ru")]
    Ru,
    #[serde(rename = "uk")]
    #[schemars(rename = "uk")]
    Uk,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UiThemeValue {
    Terminal,
    System,
    Underwater,
    UnderwaterRetro,
    Dark,
    Light,
    Grayscale,
    CatppuccinMocha,
    TokyoNight,
    Dracula,
    GruvboxDark,
    Claude,
    Matrix,
    SolarizedLight,
    Uwu,
    /// User theme carried as its full `custom:<name>` selector — the same
    /// single string `/theme` and the persisted `theme` setting use.
    Custom(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FocusTextureValue {
    Off,
    Scrim,
    Grain,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComposerDensityValue {
    Compact,
    Comfortable,
    Spacious,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComposerVimModeValue {
    Normal,
    Vim,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MentionMenuBehaviorValue {
    Fuzzy,
    Browser,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptSpacingValue {
    Compact,
    Comfortable,
    Spacious,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InlineDiffValue {
    Full,
    Summary,
    Off,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkSurfacePlacementValue {
    Top,
    Bottom,
    Left,
    Right,
    Off,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[schemars(description = "Startup mode: Act (agent wire), Plan, or Operate")]
pub enum DefaultModeValue {
    Agent,
    Plan,
    Operate,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CostCurrencyValue {
    Usd,
    Cny,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffortValue {
    Off,
    Low,
    Medium,
    High,
    Auto,
    Max,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatusIndicatorValue {
    Cw,
    Whale,
    Dots,
    Off,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SynchronizedOutputValue {
    Auto,
    On,
    Off,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatusItemValue {
    Mode,
    Model,
    Cost,
    Status,
    Agents,
    ReasoningReplay,
    PrefixStability,
    Cache,
    ContextPercent,
    GitBranch,
    LastToolElapsed,
    RateLimit,
    Tokens,
    Balance,
    SessionMetrics,
}

pub fn parse_mode(arg: Option<&str>) -> Result<ConfigUiMode, String> {
    let raw = arg.unwrap_or("").trim();
    // Bare `/config` opens the legacy native modal — it matches the rest
    // of the codewhale-tui navy chrome out of the box. Power users can
    // opt into the schemaui-driven editor with `/config tui`, or the
    // browser surface with `/config web` (web feature only).
    if raw.is_empty() || raw.eq_ignore_ascii_case("native") {
        return Ok(ConfigUiMode::Native);
    }
    if raw.eq_ignore_ascii_case("tui") {
        return Ok(ConfigUiMode::Tui);
    }
    if raw.eq_ignore_ascii_case("web") {
        return Ok(ConfigUiMode::Web);
    }
    Err("Usage: /config [native|tui|web]".to_string())
}

pub fn build_document(app: &App, config: &Config) -> Result<ConfigUiDocument> {
    let settings = Settings::load_persisted().unwrap_or_default();
    let reasoning_effort = app
        .reasoning_effort_preference
        .map(Into::into)
        .or_else(|| {
            config
                .reasoning_effort()
                .map(ReasoningEffortValue::from_setting)
        })
        .unwrap_or_else(|| app.reasoning_effort.into());
    let default_model = settings.default_model.clone();
    let status_items = app.status_items.iter().copied().map(Into::into).collect();
    Ok(ConfigUiDocument {
        runtime: RuntimeSection {
            provider: app.provider_identity_for_persistence().to_string(),
            model: app.model.clone(),
            approval_mode: app.approval_mode.into(),
        },
        settings: SettingsSection {
            auto_compact: app.auto_compact,
            calm_mode: settings.calm_mode,
            low_motion: settings.low_motion,
            fancy_animations: settings.fancy_animations,
            focus_texture: settings.focus_texture.as_str().into(),
            work_surface_placement: settings.work_surface_placement.as_str().into(),
            work_surface_top_height: settings.work_surface_top_height,
            work_surface_side_width: settings.work_surface_side_width,
            paste_burst_detection: settings.paste_burst_detection,
            show_thinking: settings.show_thinking,
            thinking_default_expanded: settings.thinking_default_expanded,
            thinking_preview_lines: settings.thinking_preview_lines,
            thinking_highlight: settings.thinking_highlight,
            help_expand_groups: settings.help_expand_groups,
            pin_last_prompt: settings.pin_last_prompt,
            show_tool_details: settings.show_tool_details,
            inline_diffs: settings.inline_diffs.as_str().into(),
            locale: UiLocale::from_setting(&settings.locale)?,
            theme: UiThemeValue::from_setting(&settings.theme)?,
            background_color: settings.background_color.clone(),
            bracketed_paste: settings.bracketed_paste,
            composer_density: settings.composer_density.as_str().into(),
            composer_border: settings.composer_border,
            composer_multiline_mode: settings.composer_multiline_mode,
            composer_vim_mode: settings.composer_vim_mode.as_str().into(),
            mention_menu_limit: settings.mention_menu_limit,
            mention_menu_behavior: settings.mention_menu_behavior.as_str().into(),
            mention_walk_depth: settings.mention_walk_depth,
            transcript_spacing: settings.transcript_spacing.as_str().into(),
            status_indicator: settings.status_indicator.as_str().into(),
            synchronized_output: settings.synchronized_output.as_str().into(),
            default_mode: settings.default_mode.as_str().into(),
            context_panel: settings.context_panel,
            max_history: settings.max_input_history,
            cost_currency: CostCurrencyValue::from_setting(&settings.cost_currency)?,
            workspace_follow_symlinks: settings.workspace_follow_symlinks,
            provider_models: settings
                .provider_models
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            default_model,
        },
        config: ConfigSection {
            mcp_config_path: app.mcp_config_path.display().to_string(),
            reasoning_effort,
            status_items,
        },
    })
}

pub fn build_schema() -> Value {
    let mut schema = serde_json::to_value(schema_for!(ConfigUiDocument)).expect("config ui schema");
    schema["title"] = Value::String("Codewhale Config".to_string());
    schema["description"] = Value::String(
        "Tune live runtime choices and durable TUI defaults. Provider switching stays in /provider."
            .to_string(),
    );
    // Provider switching is asynchronous and owns client preflight, so this
    // editor shows the route identity only as validation context. `/provider`
    // remains the sole provider-switch surface.
    schema["$defs"]["RuntimeSection"]["properties"]["provider"]["readOnly"] = Value::Bool(true);
    schema
}

#[cfg(feature = "tui")]
pub fn run_tui_editor(app: &App, config: &Config) -> Result<ConfigUiDocument> {
    let document = build_document(app, config)?;
    let value = SchemaUI::new(serde_json::to_value(document.clone())?)
        .with_schema(build_schema())
        .with_title("Codewhale Config")
        .with_description("Review the live route, then save the settings you want to keep.")
        .run(FrontendOptions::Tui(
            UiOptions::default()
                .with_confirm_exit(true)
                .with_bool_labels("On", "Off")
                .with_integer_step(1)
                .with_integer_fast_step(5)
                .with_help(true),
        ))?;
    parse_document(value)
}

#[cfg(feature = "web")]
pub async fn start_web_editor(app: &App, config: &Config) -> Result<WebConfigSession> {
    let initial = serde_json::to_value(build_document(app, config)?)?;
    let session = WebSessionBuilder::new(build_schema())
        .with_initial_data(initial)
        .with_title("Codewhale Config")
        .with_description(
            "Save updates this browser draft. Exit returns the reviewed changes to the TUI.",
        )
        .build()?;
    let bound = bind_session(session, ServeOptions::default()).await?;
    let addr = bound.local_addr();
    let url = format!("http://{addr}");
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let app_snapshot = build_document(app, config)?;
    let task = tokio::spawn(async move {
        let poll_tx = tx.clone();
        let poll_url = format!("{url}/api/session");
        let poll_task = tokio::spawn(async move {
            let client = crate::tls::reqwest_client();
            let mut last: Option<ConfigUiDocument> = Some(app_snapshot);
            loop {
                tokio::time::sleep(Duration::from_millis(750)).await;
                let response = match client.get(&poll_url).send().await {
                    Ok(response) => response,
                    Err(err) => {
                        let _ = poll_tx.send(WebConfigSessionEvent::Failed(format!(
                            "config web poll failed: {err}"
                        )));
                        break;
                    }
                };
                if !response.status().is_success() {
                    continue;
                }
                let body: Value = match response.json().await {
                    Ok(body) => body,
                    Err(err) => {
                        let _ = poll_tx.send(WebConfigSessionEvent::Failed(format!(
                            "config web decode failed: {err}"
                        )));
                        break;
                    }
                };
                let Some(data) = body.get("data") else {
                    continue;
                };
                let doc = match parse_document(data.clone()) {
                    Ok(doc) => doc,
                    Err(_) => continue,
                };
                if last.as_ref() == Some(&doc) {
                    continue;
                }
                let _ = poll_tx.send(WebConfigSessionEvent::Draft(doc.clone()));
                last = Some(doc);
            }
        });

        let result = bound.run().await;
        poll_task.abort();
        match result {
            Ok(value) => match parse_document(value) {
                Ok(doc) => {
                    let _ = tx.send(WebConfigSessionEvent::Committed(doc));
                }
                Err(err) => {
                    let _ = tx.send(WebConfigSessionEvent::Failed(format!(
                        "config web result decode failed: {err}"
                    )));
                }
            },
            Err(err) => {
                let _ = tx.send(WebConfigSessionEvent::Failed(format!(
                    "config web session failed: {err}"
                )));
            }
        }
    });
    Ok(WebConfigSession {
        task,
        receiver: rx,
        addr,
    })
}

pub fn apply_document(
    doc: ConfigUiDocument,
    app: &mut App,
    config: &mut Config,
    persist: bool,
) -> Result<ConfigUiApplyOutcome> {
    validate_document(&doc, app, config)?;
    let theme_setting = theme_setting_for_document(&doc)?;
    let mut notes = Vec::new();
    let previous_compaction = app.compaction_config();
    let previous_reasoning_effort = app.reasoning_effort;

    for (key, value) in [
        ("model", doc.runtime.model.as_str()),
        ("approval_mode", doc.runtime.approval_mode.as_setting()),
        ("auto_compact", bool_str(doc.settings.auto_compact)),
        ("calm_mode", bool_str(doc.settings.calm_mode)),
        ("low_motion", bool_str(doc.settings.low_motion)),
        ("fancy_animations", bool_str(doc.settings.fancy_animations)),
        ("focus_texture", doc.settings.focus_texture.as_setting()),
        (
            "work_surface_placement",
            doc.settings.work_surface_placement.as_setting(),
        ),
        (
            "work_surface_top_height",
            &doc.settings.work_surface_top_height.to_string(),
        ),
        (
            "work_surface_side_width",
            &doc.settings.work_surface_side_width.to_string(),
        ),
        (
            "paste_burst_detection",
            bool_str(doc.settings.paste_burst_detection),
        ),
        ("show_thinking", bool_str(doc.settings.show_thinking)),
        (
            "thinking_default_expanded",
            bool_str(doc.settings.thinking_default_expanded),
        ),
        (
            "thinking_preview_lines",
            &doc.settings.thinking_preview_lines.to_string(),
        ),
        (
            "thinking_highlight",
            bool_str(doc.settings.thinking_highlight),
        ),
        (
            "help_expand_groups",
            bool_str(doc.settings.help_expand_groups),
        ),
        ("pin_last_prompt", bool_str(doc.settings.pin_last_prompt)),
        (
            "show_tool_details",
            bool_str(doc.settings.show_tool_details),
        ),
        ("inline_diffs", doc.settings.inline_diffs.as_setting()),
        ("locale", doc.settings.locale.as_setting()),
        ("theme", theme_setting.as_str()),
        (
            "background_color",
            doc.settings
                .background_color
                .as_deref()
                .unwrap_or("default"),
        ),
        ("bracketed_paste", bool_str(doc.settings.bracketed_paste)),
        (
            "composer_density",
            doc.settings.composer_density.as_setting(),
        ),
        ("composer_border", bool_str(doc.settings.composer_border)),
        (
            "composer_multiline_mode",
            bool_str(doc.settings.composer_multiline_mode),
        ),
        (
            "composer_vim_mode",
            doc.settings.composer_vim_mode.as_setting(),
        ),
        (
            "mention_menu_limit",
            &doc.settings.mention_menu_limit.to_string(),
        ),
        (
            "mention_menu_behavior",
            doc.settings.mention_menu_behavior.as_setting(),
        ),
        (
            "mention_walk_depth",
            &doc.settings.mention_walk_depth.to_string(),
        ),
        (
            "transcript_spacing",
            doc.settings.transcript_spacing.as_setting(),
        ),
        (
            "status_indicator",
            doc.settings.status_indicator.as_setting(),
        ),
        (
            "synchronized_output",
            doc.settings.synchronized_output.as_setting(),
        ),
        ("default_mode", doc.settings.default_mode.as_setting()),
        ("context_panel", bool_str(doc.settings.context_panel)),
        ("max_history", &doc.settings.max_history.to_string()),
        ("cost_currency", doc.settings.cost_currency.as_setting()),
        (
            "workspace_follow_symlinks",
            bool_str(doc.settings.workspace_follow_symlinks),
        ),
        ("mcp_config_path", doc.config.mcp_config_path.as_str()),
    ] {
        let result = commands::set_config_value(app, key, value, persist);
        if result.is_error {
            bail!(
                "{}",
                result
                    .message
                    .unwrap_or_else(|| "config update failed".to_string())
            );
        }
        if let Some(message) = result.message {
            notes.push(message);
        }
    }

    // default_model is only applied when persisting (it controls the model
    // for future sessions).  Processing it in the main loop would overwrite
    // the runtime model the user just chose when persist=false (#346-fix).
    if persist {
        let default_model_val = doc.settings.default_model.as_deref().unwrap_or("default");
        let result = commands::set_config_value(app, "default_model", default_model_val, true);
        if result.is_error {
            bail!(
                "{}",
                result
                    .message
                    .unwrap_or_else(|| "default_model update failed".to_string())
            );
        }
        if let Some(message) = result.message {
            notes.push(message);
        }

        // `/model` and the schema-driven config editor share one durable
        // provider-scoped model map. `set_config_value("model", ...)` owns the
        // live App mutation, while this block persists that selection without
        // rewriting the DeepSeek-only global fallback (#3227).
        let mut provider_models = doc
            .settings
            .provider_models
            .iter()
            .map(|(provider, model)| (provider.clone(), model.clone()))
            .collect::<std::collections::HashMap<_, _>>();
        provider_models.insert(
            app.provider_identity_for_persistence().to_string(),
            app.model_selection_for_persistence(),
        );
        Settings::transact(|settings| {
            settings.provider_models = (!provider_models.is_empty()).then_some(provider_models);
            Ok(())
        })?;
        notes.push(format!(
            "{} model saved for {}",
            app.model_display_label(),
            app.provider_identity_for_persistence()
        ));
    }

    apply_reasoning_effort(app, config, doc.config.reasoning_effort, persist)?;
    let requires_engine_sync = app.compaction_config() != previous_compaction
        || app.reasoning_effort != previous_reasoning_effort;

    let new_status_items = parse_status_items(&doc.config.status_items);
    if app.status_items != new_status_items {
        app.status_items = new_status_items.clone();
        app.needs_redraw = true;
        if persist {
            let path = crate::config_persistence::persist_status_items(&new_status_items)?;
            notes.push(format!("status_items saved to {}", path.display()));
        } else {
            notes.push("status_items updated for this session".to_string());
        }
    }

    if persist {
        reload_runtime_config(app, config)?;
        notes.extend(config_reload_notes(app, config));
    }
    let changed = !notes.is_empty();
    let final_message = if notes.is_empty() {
        if persist {
            "Config unchanged".to_string()
        } else {
            "Runtime config unchanged".to_string()
        }
    } else {
        notes.last().cloned().unwrap_or_default()
    };
    Ok(ConfigUiApplyOutcome {
        changed,
        final_message,
        requires_engine_sync,
    })
}

pub fn parse_document(value: Value) -> Result<ConfigUiDocument> {
    serde_json::from_value(value).context("failed to decode config ui document")
}

#[cfg(feature = "web")]
pub fn open_browser(url: &str) -> Result<()> {
    crate::utils::open_url(url)
}

fn validate_document(doc: &ConfigUiDocument, app: &App, config: &Config) -> Result<()> {
    let document_identity = config
        .resolve_provider_identity(&doc.runtime.provider)
        .map_err(anyhow::Error::msg)?;
    if document_identity.provider != app.api_provider
        || document_identity.key != app.provider_identity_for_persistence()
    {
        bail!(
            "provider changed from '{}' to '{}' while editing; switch providers with /provider, then reopen /config",
            app.provider_identity_for_persistence(),
            doc.runtime.provider
        );
    }
    validate_and_normalize_model(config, app.api_provider, &doc.runtime.model)?;
    for (provider_id, model) in &doc.settings.provider_models {
        let identity = config
            .resolve_provider_identity(provider_id)
            .map_err(anyhow::Error::msg)?;
        validate_and_normalize_model(config, identity.provider, model)
            .map_err(|err| anyhow::anyhow!("invalid provider_models.{provider_id} value: {err}"))?;
    }
    if doc.config.mcp_config_path.trim().is_empty() {
        bail!("mcp_config_path cannot be empty");
    }
    let _ = theme_setting_for_document(doc)?;
    Ok(())
}

fn theme_setting_for_document(doc: &ConfigUiDocument) -> Result<String> {
    crate::palette::resolve_theme_setting(&doc.settings.theme.as_setting(), None)
        .map(|(normalized, _, _)| normalized)
        .map_err(anyhow::Error::msg)
}

/// Validate and normalize a model against the provider's *effective* route.
///
/// A custom DeepSeek base URL owns its own model namespace, so non-DeepSeek ids
/// must be accepted here the same way request-time
/// `wire_model_for_provider_route` preserves them. The previous provider-only
/// gate rejected those saves even though the live session could use them.
fn validate_and_normalize_model(
    config: &Config,
    provider: crate::config::ApiProvider,
    model: &str,
) -> Result<String> {
    let model = model.trim();
    if model.is_empty() {
        bail!(
            "invalid model '{model}' for provider '{}'",
            provider.as_str()
        );
    }
    if model.eq_ignore_ascii_case("auto") {
        // Still run the provider gate so empty/unknown providers cannot sneak
        // an `auto` through without the same checks as `/model auto`.
        validate_route(provider, model).map_err(anyhow::Error::msg)?;
        return Ok("auto".to_string());
    }

    // OpenCode Go is protocol-bound (Chat Completions only) even when its base
    // URL is overridden — never open the custom-endpoint passthrough for it.
    if provider == crate::config::ApiProvider::OpencodeGo {
        validate_route(provider, model).map_err(anyhow::Error::msg)?;
        return normalize_model_name_for_provider(provider, model).ok_or_else(|| {
            anyhow::anyhow!(
                "invalid model '{model}' for provider '{}'",
                provider.as_str()
            )
        });
    }

    // Custom / self-hosted endpoints own their model id namespace. Match the
    // request path (`wire_model_for_provider_route`) so saving `/config` on a
    // custom deepseek gateway does not reject a non-DeepSeek wire id the
    // session is already running.
    if config.provider_uses_custom_endpoint(provider) {
        return normalize_custom_model_id(model).ok_or_else(|| {
            anyhow::anyhow!(
                "invalid model '{model}' for provider '{}'",
                provider.as_str()
            )
        });
    }

    validate_route(provider, model).map_err(anyhow::Error::msg)?;
    normalize_model_name_for_provider(provider, model).ok_or_else(|| {
        anyhow::anyhow!(
            "invalid model '{model}' for provider '{}'",
            provider.as_str()
        )
    })
}

fn reload_runtime_config(app: &mut App, config: &mut Config) -> Result<()> {
    let reloaded = Config::load(app.config_path.clone(), app.config_profile.as_deref())?;
    *config = reloaded.clone();
    // Match App startup precedence: an explicit config provider wins, while a
    // config with no provider keeps the saved TUI provider. Reloading an
    // unrelated setting must never pair a Z.ai model with DeepSeek merely
    // because `Config::default()` is DeepSeek.
    let settings = Settings::load_persisted().unwrap_or_default();
    let identity = if reloaded
        .provider
        .as_deref()
        .is_some_and(|provider| !provider.trim().is_empty())
    {
        reloaded.active_provider_identity(reloaded.api_provider())
    } else if let Some(provider) = settings.default_provider.as_deref() {
        reloaded.resolve_provider_identity(provider)
    } else {
        reloaded.active_provider_identity(reloaded.api_provider())
    }
    .map_err(anyhow::Error::msg)?;
    app.set_provider_identity_record(identity);
    let requested = reloaded
        .reasoning_effort()
        .map(ReasoningEffort::from_setting)
        .or(app.reasoning_effort_preference)
        .unwrap_or(app.reasoning_effort);
    if reloaded.reasoning_effort().is_some() {
        app.reasoning_effort_preference = Some(requested);
    }
    app.reasoning_effort = if app.auto_model {
        requested
    } else {
        requested.normalize_for_provider(app.api_provider)
    };
    app.invalidate_route_receipts_for_reasoning_change();
    app.update_model_compaction_budget();
    app.mcp_config_path = reloaded.mcp_config_path();
    app.skills_dir = reloaded.skills_dir();
    app.ui_locale = resolve_locale(&settings.locale);
    app.workflow_config = reloaded.workflow_config();
    crate::tools::workflow::set_session_workflow_config(
        &app.workspace,
        app.workflow_config.clone(),
    );
    app.goal_max_continuations = reloaded.goal_max_continuations();
    Ok(())
}

fn config_reload_notes(app: &App, config: &Config) -> Vec<String> {
    let mut notes = Vec::new();
    notes.push("Config saved and reloaded".to_string());
    if app.mcp_reload_required {
        notes.push(format!(
            "MCP tool pool still needs `/mcp reload` after {}",
            config.mcp_config_path().display()
        ));
    }
    notes
}

fn apply_reasoning_effort(
    app: &mut App,
    config: &mut Config,
    value: ReasoningEffortValue,
    persist: bool,
) -> Result<()> {
    let requested = ReasoningEffort::from(value);
    let effective = if app.auto_model {
        requested
    } else {
        requested.normalize_for_provider(app.api_provider)
    };
    app.reasoning_effort = effective;
    app.reasoning_effort_preference = Some(requested);
    app.invalidate_route_receipts_for_reasoning_change();
    app.update_model_compaction_budget();
    if persist {
        crate::config_persistence::persist_root_string_key(
            app.config_path.as_deref(),
            "reasoning_effort",
            requested.as_setting(),
        )?;
        // App startup gives settings.toml precedence over config.toml. Keep
        // the schema-driven TUI/web editor aligned with the picker so an older
        // saved startup value cannot silently undo this persisted choice on
        // the next launch.
        app.startup_defaults.apply_blocking(
            crate::tui::startup_defaults::StartupDefaults::reasoning_effort(requested.as_setting()),
        )?;
    }
    config.reasoning_effort = Some(requested.as_setting().to_string());
    Ok(())
}

fn parse_status_items(items: &[StatusItemValue]) -> Vec<StatusItem> {
    items.iter().copied().map(Into::into).collect()
}

impl ApprovalModeValue {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Bypass => "bypass",
            Self::Suggest => "suggest",
            Self::Never => "never",
        }
    }
}

impl UiLocale {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::En => "en",
            Self::Ja => "ja",
            Self::ZhHans => "zh-Hans",
            Self::ZhHant => "zh-Hant",
            Self::PtBr => "pt-BR",
            Self::Es419 => "es-419",
            Self::Vi => "vi",
            Self::Ko => "ko",
            Self::Ca => "ca",
            Self::De => "de",
            Self::Fr => "fr",
            Self::Id => "id",
            Self::Hi => "hi",
            Self::Ru => "ru",
            Self::Uk => "uk",
        }
    }

    fn from_setting(value: &str) -> Result<Self> {
        match normalize_configured_locale(value) {
            Some("auto") => Ok(Self::Auto),
            Some("en") => Ok(Self::En),
            Some("ja") => Ok(Self::Ja),
            Some("zh-Hans") => Ok(Self::ZhHans),
            Some("zh-Hant") => Ok(Self::ZhHant),
            Some("pt-BR") => Ok(Self::PtBr),
            Some("es-419") => Ok(Self::Es419),
            Some("vi") => Ok(Self::Vi),
            Some("ko") => Ok(Self::Ko),
            Some("ca") => Ok(Self::Ca),
            Some("de") => Ok(Self::De),
            Some("fr") => Ok(Self::Fr),
            Some("id") => Ok(Self::Id),
            Some("hi") => Ok(Self::Hi),
            Some("ru") => Ok(Self::Ru),
            Some("uk") => Ok(Self::Uk),
            Some(other) => bail!("unsupported locale '{other}'"),
            None => bail!("invalid locale '{value}'"),
        }
    }
}

impl UiThemeValue {
    /// Canonical settings string. `Custom` carries its own full
    /// `custom:<name>` selector, so it round-trips without a sibling field.
    fn as_setting(&self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Terminal => "terminal".into(),
            Self::System => "system".into(),
            Self::Underwater => "underwater".into(),
            Self::UnderwaterRetro => "underwater-retro".into(),
            Self::Dark => "dark".into(),
            Self::Light => "light".into(),
            Self::Grayscale => "grayscale".into(),
            Self::CatppuccinMocha => "catppuccin-mocha".into(),
            Self::TokyoNight => "tokyo-night".into(),
            Self::Dracula => "dracula".into(),
            Self::GruvboxDark => "gruvbox-dark".into(),
            Self::Claude => "claude".into(),
            Self::Matrix => "matrix".into(),
            Self::SolarizedLight => "solarized-light".into(),
            Self::Uwu => "uwu".into(),
            Self::Custom(selector) => std::borrow::Cow::Owned(selector.clone()),
        }
    }

    fn from_setting(value: &str) -> Result<Self> {
        if let Some(selector) =
            crate::palette::normalize_user_theme_selector(value).map_err(anyhow::Error::msg)?
        {
            return Ok(Self::Custom(selector));
        }
        match crate::palette::normalize_theme_name(value) {
            Some("terminal") => Ok(Self::Terminal),
            Some("system") => Ok(Self::System),
            Some("underwater") => Ok(Self::Underwater),
            Some("underwater-retro") => Ok(Self::UnderwaterRetro),
            Some("dark") => Ok(Self::Dark),
            Some("light") => Ok(Self::Light),
            Some("grayscale") => Ok(Self::Grayscale),
            Some("catppuccin-mocha") => Ok(Self::CatppuccinMocha),
            Some("tokyo-night") => Ok(Self::TokyoNight),
            Some("dracula") => Ok(Self::Dracula),
            Some("gruvbox-dark") => Ok(Self::GruvboxDark),
            Some("claude") => Ok(Self::Claude),
            Some("matrix") => Ok(Self::Matrix),
            Some("solarized-light") => Ok(Self::SolarizedLight),
            Some("uwu") => Ok(Self::Uwu),
            Some(other) => bail!("unsupported theme '{other}'"),
            None => bail!("invalid theme '{value}'"),
        }
    }
}

impl FocusTextureValue {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Scrim => "scrim",
            Self::Grain => "grain",
        }
    }
}

impl From<&str> for FocusTextureValue {
    fn from(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "scrim" => Self::Scrim,
            "grain" => Self::Grain,
            _ => Self::Off,
        }
    }
}

impl ComposerDensityValue {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Comfortable => "comfortable",
            Self::Spacious => "spacious",
        }
    }
}

impl ComposerVimModeValue {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Vim => "vim",
        }
    }
}

impl From<&str> for ComposerVimModeValue {
    fn from(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "vim" => Self::Vim,
            _ => Self::Normal,
        }
    }
}

impl MentionMenuBehaviorValue {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Fuzzy => "fuzzy",
            Self::Browser => "browser",
        }
    }
}

impl From<&str> for MentionMenuBehaviorValue {
    fn from(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "browser" => Self::Browser,
            _ => Self::Fuzzy,
        }
    }
}

impl TranscriptSpacingValue {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Comfortable => "comfortable",
            Self::Spacious => "spacious",
        }
    }
}

impl InlineDiffValue {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Summary => "summary",
            Self::Off => "off",
        }
    }
}

impl From<&str> for InlineDiffValue {
    fn from(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "summary" => Self::Summary,
            "off" => Self::Off,
            _ => Self::Full,
        }
    }
}

impl WorkSurfacePlacementValue {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::Left => "left",
            Self::Right => "right",
            Self::Off => "off",
        }
    }
}

impl From<&str> for WorkSurfacePlacementValue {
    fn from(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "top" => Self::Top,
            "bottom" => Self::Bottom,
            "left" => Self::Left,
            "right" => Self::Right,
            "off" => Self::Off,
            // Mirror `normalize_work_surface_placement`: the bar's home is
            // under the composer.
            _ => Self::Bottom,
        }
    }
}

impl DefaultModeValue {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Plan => "plan",
            Self::Operate => "operate",
        }
    }

    /// User-facing label in config UI (wire values stay agent/plan/operate).
    #[allow(dead_code)] // reserved for config UI option rendering / chrome
    pub fn label(self) -> &'static str {
        match self {
            Self::Agent => "Act",
            Self::Plan => "Plan",
            Self::Operate => "Operate",
        }
    }

    #[allow(dead_code)]
    fn description(self) -> &'static str {
        match self {
            Self::Agent => "Do the work in this session.",
            Self::Plan => "Design first; implement after you approve.",
            Self::Operate => "Dispatch workers; keep the parent free for steers.",
        }
    }
}

impl CostCurrencyValue {
    fn from_setting(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "usd" => Ok(Self::Usd),
            "cny" | "rmb" | "yuan" => Ok(Self::Cny),
            other => {
                anyhow::bail!("Invalid cost_currency '{other}': expected usd, cny, rmb, or yuan")
            }
        }
    }

    fn as_setting(self) -> &'static str {
        match self {
            Self::Usd => "usd",
            Self::Cny => "cny",
        }
    }
}

impl From<ApprovalMode> for ApprovalModeValue {
    fn from(value: ApprovalMode) -> Self {
        match value {
            ApprovalMode::Auto => Self::Auto,
            ApprovalMode::Bypass => Self::Bypass,
            ApprovalMode::Suggest => Self::Suggest,
            ApprovalMode::Never => Self::Never,
        }
    }
}

impl From<ReasoningEffort> for ReasoningEffortValue {
    fn from(value: ReasoningEffort) -> Self {
        match value {
            ReasoningEffort::Off => Self::Off,
            ReasoningEffort::Low => Self::Low,
            ReasoningEffort::Medium => Self::Medium,
            ReasoningEffort::High => Self::High,
            ReasoningEffort::Auto => Self::Auto,
            ReasoningEffort::Minimal => Self::Low,
            ReasoningEffort::XHigh => Self::Max,
            ReasoningEffort::Ultra => Self::Max,
            ReasoningEffort::Max => Self::Max,
        }
    }
}

impl ReasoningEffortValue {
    fn from_setting(value: &str) -> Self {
        match ReasoningEffort::from_setting(value) {
            ReasoningEffort::Off => Self::Off,
            ReasoningEffort::Low => Self::Low,
            ReasoningEffort::Medium => Self::Medium,
            ReasoningEffort::High => Self::High,
            ReasoningEffort::Auto => Self::Auto,
            ReasoningEffort::Minimal => Self::Low,
            ReasoningEffort::XHigh => Self::Max,
            ReasoningEffort::Ultra => Self::Max,
            ReasoningEffort::Max => Self::Max,
        }
    }
}

impl From<ReasoningEffortValue> for ReasoningEffort {
    fn from(value: ReasoningEffortValue) -> Self {
        match value {
            ReasoningEffortValue::Off => Self::Off,
            ReasoningEffortValue::Low => Self::Low,
            ReasoningEffortValue::Medium => Self::Medium,
            ReasoningEffortValue::High => Self::High,
            ReasoningEffortValue::Auto => Self::Auto,
            ReasoningEffortValue::Max => Self::Max,
        }
    }
}

impl From<&str> for ComposerDensityValue {
    fn from(value: &str) -> Self {
        match ComposerDensity::from_setting(value) {
            ComposerDensity::Compact => Self::Compact,
            ComposerDensity::Comfortable => Self::Comfortable,
            ComposerDensity::Spacious => Self::Spacious,
        }
    }
}

impl From<&str> for TranscriptSpacingValue {
    fn from(value: &str) -> Self {
        match TranscriptSpacing::from_setting(value) {
            TranscriptSpacing::Compact => Self::Compact,
            TranscriptSpacing::Comfortable => Self::Comfortable,
            TranscriptSpacing::Spacious => Self::Spacious,
        }
    }
}

impl From<&str> for DefaultModeValue {
    fn from(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "operate" | "operation" | "ops" => Self::Operate,
            // yolo was a mode+permission bundle; startup mode becomes Act and
            // permission posture is migrated separately on load.
            other => match AppMode::from_setting(other) {
                AppMode::Plan => Self::Plan,
                AppMode::Operate => Self::Operate,
                AppMode::Agent => Self::Agent,
            },
        }
    }
}

impl StatusIndicatorValue {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Cw => "cw",
            Self::Whale => "whale",
            Self::Dots => "dots",
            Self::Off => "off",
        }
    }
}

impl SynchronizedOutputValue {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

impl From<&str> for SynchronizedOutputValue {
    fn from(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "on" | "true" | "yes" | "1" | "enabled" => Self::On,
            "off" | "false" | "no" | "0" | "disabled" => Self::Off,
            _ => Self::Auto,
        }
    }
}

impl From<&str> for StatusIndicatorValue {
    fn from(value: &str) -> Self {
        // Permissive aliases mirror `Settings::normalize_status_indicator`,
        // so a TOML file with `status_indicator = "🐳"` or `"none"`
        // resolves to the canonical enum variant.
        match value.trim().to_ascii_lowercase().as_str() {
            "cw" | "mark" | "text" => Self::Cw,
            "dots" | "dot" => Self::Dots,
            "off" | "none" | "hidden" | "false" => Self::Off,
            "whale" | "🐳" | "🐋" => Self::Whale,
            _ => Self::Cw,
        }
    }
}

impl From<StatusItem> for StatusItemValue {
    fn from(value: StatusItem) -> Self {
        match value {
            StatusItem::Mode => Self::Mode,
            StatusItem::Model => Self::Model,
            StatusItem::Cost => Self::Cost,
            StatusItem::Status => Self::Status,
            StatusItem::Agents => Self::Agents,
            StatusItem::ReasoningReplay => Self::ReasoningReplay,
            StatusItem::PrefixStability => Self::PrefixStability,
            StatusItem::Cache => Self::Cache,
            StatusItem::ContextPercent => Self::ContextPercent,
            StatusItem::GitBranch => Self::GitBranch,
            StatusItem::LastToolElapsed => Self::LastToolElapsed,
            StatusItem::RateLimit => Self::RateLimit,
            StatusItem::Tokens => Self::Tokens,
            StatusItem::Balance => Self::Balance,
            StatusItem::SessionMetrics => Self::SessionMetrics,
        }
    }
}

impl From<StatusItemValue> for StatusItem {
    fn from(value: StatusItemValue) -> Self {
        match value {
            StatusItemValue::Mode => Self::Mode,
            StatusItemValue::Model => Self::Model,
            StatusItemValue::Cost => Self::Cost,
            StatusItemValue::Status => Self::Status,
            StatusItemValue::Agents => Self::Agents,
            StatusItemValue::ReasoningReplay => Self::ReasoningReplay,
            StatusItemValue::PrefixStability => Self::PrefixStability,
            StatusItemValue::Cache => Self::Cache,
            StatusItemValue::ContextPercent => Self::ContextPercent,
            StatusItemValue::GitBranch => Self::GitBranch,
            StatusItemValue::LastToolElapsed => Self::LastToolElapsed,
            StatusItemValue::RateLimit => Self::RateLimit,
            StatusItemValue::Tokens => Self::Tokens,
            StatusItemValue::Balance => Self::Balance,
            StatusItemValue::SessionMetrics => Self::SessionMetrics,
        }
    }
}

fn bool_str(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApiProvider, Config};
    use crate::test_support::{EnvVarGuard, lock_test_env};
    use crate::tui::app::{App, TuiOptions};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn app() -> App {
        let options = TuiOptions {
            screen_mode: crate::tui::app::ScreenMode::Inline,
            // Keep this fixture independent from the developer's saved
            // `default_mode` setting.
            start_in_agent_mode: true,
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        };
        let mut app = App::new(options, &Config::default());
        // App::new merges developer-local settings, which can include a saved
        // provider/model from the interactive TUI. Keep these config UI tests
        // pinned to DeepSeek defaults so they only exercise document apply
        // semantics.
        app.model = "deepseek-v4-pro".to_string();
        app.auto_model = false;
        app.api_provider = ApiProvider::Deepseek;
        app.model_ids_passthrough = false;
        app.active_route_limits = None;
        app.reasoning_effort_preference = None;
        app.update_model_compaction_budget();
        app
    }

    #[test]
    fn build_document_reflects_app_state() {
        let mut app = app();
        app.auto_model = false;
        app.model = "deepseek-v4-pro".to_string();
        app.reasoning_effort = ReasoningEffort::Max;
        let config = Config::default();
        let doc = build_document(&app, &config).expect("document");
        assert_eq!(doc.runtime.model, app.model);
        // The document must mirror the live app posture. The developer's saved
        // permission posture may legitimately be Bypass; this test must not
        // rewrite that product setting into an assumed Suggest default.
        assert_eq!(doc.runtime.approval_mode, app.approval_mode.into());
        assert_eq!(doc.config.reasoning_effort, ReasoningEffortValue::Max);
    }

    #[test]
    fn build_document_uses_raw_reasoning_preference() {
        let mut app = app();
        app.api_provider = ApiProvider::OpenaiCodex;
        app.reasoning_effort = ReasoningEffort::Low;
        app.reasoning_effort_preference = Some(ReasoningEffort::Off);

        let doc = build_document(&app, &Config::default()).expect("document");

        assert_eq!(doc.config.reasoning_effort, ReasoningEffortValue::Off);
    }

    #[test]
    fn config_ui_reasoning_keeps_raw_preference_for_fixed_route() {
        let mut app = app();
        app.api_provider = ApiProvider::OpenaiCodex;
        app.reasoning_effort = ReasoningEffort::High;
        app.reasoning_effort_preference = None;
        let mut config = Config::default();

        apply_reasoning_effort(&mut app, &mut config, ReasoningEffortValue::Off, false)
            .expect("apply reasoning");

        assert_eq!(app.reasoning_effort, ReasoningEffort::Low);
        assert_eq!(app.reasoning_effort_preference, Some(ReasoningEffort::Off));
        assert_eq!(config.reasoning_effort.as_deref(), Some("off"));
    }

    #[test]
    fn persisted_config_ui_reasoning_updates_the_startup_precedence_layer() {
        let _lock = lock_test_env();
        let temp_root = tempfile::tempdir().expect("isolated Codewhale home");
        let codewhale_home = temp_root.path().join(".codewhale");
        fs::create_dir_all(&codewhale_home).expect("settings dir");
        fs::write(
            codewhale_home.join("settings.toml"),
            "default_model = \"auto\"\nreasoning_effort = \"max\"\n",
        )
        .expect("seed settings");
        let config_path = temp_root.path().join("config.toml");
        fs::write(&config_path, "reasoning_effort = \"max\"\n").expect("seed config");
        let _home = EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let _codewhale_config = EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
        let _deepseek_config = EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");

        let mut app = app();
        app.config_path = Some(config_path.clone());
        let mut config = Config::load(Some(config_path.clone()), None).expect("load config");

        apply_reasoning_effort(&mut app, &mut config, ReasoningEffortValue::Low, true)
            .expect("persist reasoning");

        assert_eq!(
            Settings::load_persisted()
                .expect("reload settings")
                .reasoning_effort
                .as_deref(),
            Some("low")
        );
        let persisted_config =
            Config::load(Some(config_path), None).expect("reload persisted config");
        let restored = App::new(
            TuiOptions {
                screen_mode: crate::tui::app::ScreenMode::Inline,
                start_in_agent_mode: true,
                ..crate::test_support::test_tui_options(PathBuf::from("."))
            },
            &persisted_config,
        );
        assert!(restored.auto_model);
        assert_eq!(restored.reasoning_effort, ReasoningEffort::Low);
        assert_eq!(
            restored.reasoning_effort_preference,
            Some(ReasoningEffort::Low)
        );
    }

    #[test]
    fn legacy_startup_mode_values_project_to_agent_in_config_ui() {
        assert_eq!(DefaultModeValue::from("agent"), DefaultModeValue::Agent);
        assert_eq!(DefaultModeValue::from("operate"), DefaultModeValue::Operate);
        assert_eq!(DefaultModeValue::from("yolo"), DefaultModeValue::Agent);
        assert_eq!(DefaultModeValue::from("plan"), DefaultModeValue::Plan);
        assert_eq!(DefaultModeValue::Agent.as_setting(), "agent");
        assert_eq!(DefaultModeValue::Plan.as_setting(), "plan");
        assert_eq!(DefaultModeValue::Operate.as_setting(), "operate");
        assert_eq!(DefaultModeValue::Agent.label(), "Act");
        assert_eq!(DefaultModeValue::Operate.label(), "Operate");
    }

    #[test]
    fn build_document_reflects_cost_currency_from_settings() {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "codewhale-config-ui-cost-currency-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(temp_root.join(".deepseek")).expect("config dir");
        let config_path = temp_root.join(".deepseek").join("config.toml");
        fs::write(&config_path, "").expect("seed config");
        fs::write(
            temp_root.join(".deepseek").join("settings.toml"),
            r#"
cost_currency = "cny"
"#,
        )
        .expect("seed settings");

        let _config_path = EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &config_path);

        let app = app();
        let config = Config::default();
        let doc = build_document(&app, &config).expect("document");

        assert_eq!(doc.settings.cost_currency, CostCurrencyValue::Cny);
    }

    #[test]
    fn build_document_reflects_background_color_from_settings() {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "codewhale-config-ui-background-color-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(temp_root.join(".deepseek")).expect("config dir");
        let config_path = temp_root.join(".deepseek").join("config.toml");
        fs::write(&config_path, "").expect("seed config");
        fs::write(
            temp_root.join(".deepseek").join("settings.toml"),
            r##"
background_color = "#1A1B26"
"##,
        )
        .expect("seed settings");

        let _config_path = EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &config_path);

        let app = app();
        let config = Config::default();
        let doc = build_document(&app, &config).expect("document");

        assert_eq!(doc.settings.background_color.as_deref(), Some("#1a1b26"));
    }

    #[test]
    fn build_document_accepts_every_shipped_locale_from_settings() {
        let _lock = lock_test_env();
        let temp_root = tempfile::tempdir().expect("isolated Codewhale home");
        let codewhale_home = temp_root.path().join(".codewhale");
        fs::create_dir_all(&codewhale_home).expect("settings dir");
        let settings_path = codewhale_home.join("settings.toml");
        fs::write(&settings_path, "").expect("seed settings");
        let _home = EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let _codewhale_config = EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
        let _deepseek_config = EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");

        let app = app();
        let config = Config::default();
        for tag in std::iter::once("auto").chain(
            crate::localization::Locale::shipped()
                .iter()
                .map(|locale| locale.tag()),
        ) {
            fs::write(&settings_path, format!("locale = \"{tag}\"\n"))
                .unwrap_or_else(|err| panic!("persist locale {tag}: {err}"));
            let doc = build_document(&app, &config)
                .unwrap_or_else(|err| panic!("build config document for {tag}: {err}"));
            assert_eq!(
                doc.settings.locale.as_setting(),
                tag,
                "typed config document must preserve persisted locale {tag}"
            );
        }
    }

    #[test]
    fn custom_theme_round_trips_through_typed_config_document() {
        let _lock = lock_test_env();
        let temp_root = tempfile::tempdir().expect("isolated Codewhale home");
        let codewhale_home = temp_root.path().join(".codewhale");
        let themes_dir = codewhale_home.join("themes");
        fs::create_dir_all(&themes_dir).expect("themes dir");
        fs::write(
            themes_dir.join("ocean.json"),
            r##"{"schema_version":1,"base":"dark","colors":{"accent_primary":"#123456"}}"##,
        )
        .expect("custom theme");
        fs::write(
            codewhale_home.join("settings.toml"),
            r#"theme = "custom:ocean"
"#,
        )
        .expect("settings");
        let _home = EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let _codewhale_config = EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
        let _deepseek_config = EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");

        let mut app = app();
        let mut config = Config::default();
        let doc = build_document(&app, &config).expect("document");
        assert_eq!(
            doc.settings.theme,
            UiThemeValue::Custom("custom:ocean".to_string())
        );

        // The typed document must survive its wire form untouched: the custom
        // selector lives inside `theme` itself, with no sibling field and no
        // disk migration.
        let doc = parse_document(serde_json::to_value(&doc).expect("serialize document"))
            .expect("parse document");
        assert_eq!(
            doc.settings.theme,
            UiThemeValue::Custom("custom:ocean".to_string())
        );

        apply_document(doc, &mut app, &mut config, false).expect("apply custom theme");
        assert_eq!(
            app.ui_theme.accent_primary,
            ratatui::style::Color::Rgb(0x12, 0x34, 0x56)
        );
    }

    #[test]
    fn schema_contains_typed_enums() {
        let schema = build_schema();
        assert_eq!(schema["title"], serde_json::json!("Codewhale Config"));
        assert!(
            schema["description"]
                .as_str()
                .is_some_and(|copy| copy.contains("Provider switching stays in /provider"))
        );
        assert_eq!(
            schema["$defs"]["RuntimeSection"]["properties"]["provider"]["readOnly"],
            serde_json::json!(true)
        );
        assert!(
            schema["$defs"]["SettingsSection"]["properties"]["background_color"]["description"]
                .as_str()
                .is_some_and(|copy| copy.contains("Blue Stage"))
        );
        assert!(
            schema["$defs"]["SettingsSection"]["properties"]["locale"]["description"]
                .as_str()
                .is_some_and(|copy| {
                    copy.contains("Every shipped locale pack holds full English parity")
                        && copy.contains("nothing falls back")
                })
        );
        let approval_mode = &schema["$defs"]["ApprovalModeValue"]["enum"];
        assert_eq!(
            approval_mode,
            &serde_json::json!(["auto", "bypass", "suggest", "never"])
        );
        let default_mode_def = &schema["$defs"]["DefaultModeValue"];
        let default_mode = default_mode_def
            .get("enum")
            .cloned()
            .or_else(|| {
                default_mode_def.get("oneOf").and_then(|ones| {
                    let mut vals = Vec::new();
                    for item in ones.as_array()? {
                        if let Some(v) = item.get("const") {
                            vals.push(v.clone());
                        } else if let Some(arr) = item.get("enum").and_then(|e| e.as_array()) {
                            vals.extend(arr.iter().cloned());
                        }
                    }
                    Some(serde_json::Value::Array(vals))
                })
            })
            .unwrap_or(serde_json::Value::Null);
        assert_eq!(
            default_mode,
            serde_json::json!(["agent", "plan", "operate"]),
            "DefaultModeValue schema: {default_mode_def}"
        );
        let inline_diffs = &schema["$defs"]["InlineDiffValue"]["enum"];
        assert_eq!(inline_diffs, &serde_json::json!(["full", "summary", "off"]));
        let locale = &schema["$defs"]["UiLocale"]["enum"];
        let expected_locales = std::iter::once("auto")
            .chain(
                crate::localization::Locale::shipped()
                    .iter()
                    .map(|locale| locale.tag()),
            )
            .collect::<Vec<_>>();
        assert_eq!(
            locale,
            &serde_json::json!(expected_locales),
            "UiLocale schema must match Locale::shipped()"
        );
        let theme = &schema["$defs"]["UiThemeValue"];
        // `Custom` carries its `custom:<name>` selector inline, so schemars
        // renders oneOf: the named themes stay a string enum and the custom
        // variant becomes a single-key object.
        let theme_variants = theme
            .get("oneOf")
            .and_then(|ones| ones.as_array())
            .expect("UiThemeValue oneOf");
        let named = &theme_variants[0]["enum"];
        assert_eq!(
            named,
            &serde_json::json!([
                "terminal",
                "system",
                "underwater",
                "underwater-retro",
                "dark",
                "light",
                "grayscale",
                "catppuccin-mocha",
                "tokyo-night",
                "dracula",
                "gruvbox-dark",
                "claude",
                "matrix",
                "solarized-light",
                "uwu"
            ])
        );
        assert_eq!(
            theme_variants[1]["properties"]["custom"],
            serde_json::json!({"type": "string"}),
            "custom theme selector must ride inside the theme value"
        );
    }

    #[test]
    fn ui_theme_value_covers_every_selectable_theme() {
        // The typed /config document must round-trip every theme the /theme
        // picker can persist, so the typed value space tracks
        // `SELECTABLE_THEMES`. Drift here silently rewrote a saved theme on
        // save (claude and solarized-light used to fall out).
        for theme in crate::palette::SELECTABLE_THEMES {
            let name = theme.name();
            let value = UiThemeValue::from_setting(name)
                .unwrap_or_else(|err| panic!("UiThemeValue must accept theme {name}: {err}"));
            assert_eq!(
                value.as_setting(),
                name,
                "UiThemeValue must round-trip theme {name}"
            );
            let serialized = serde_json::to_value(&value)
                .unwrap_or_else(|err| panic!("serialize theme {name}: {err}"));
            assert_eq!(serialized, serde_json::json!(name));
        }
    }

    #[test]
    fn work_surface_placement_round_trips_bottom_and_off_through_typed_document() {
        // A persisted `bottom` used to deserialize as `Top` — saving the
        // typed document silently moved the work surface. Every placement
        // `Settings::set` accepts must survive the typed document.
        assert_eq!(
            WorkSurfacePlacementValue::from("bottom"),
            WorkSurfacePlacementValue::Bottom
        );
        assert_eq!(
            WorkSurfacePlacementValue::from("off"),
            WorkSurfacePlacementValue::Off
        );
        for placement in ["bottom", "top", "left", "right", "off"] {
            let value = WorkSurfacePlacementValue::from(placement);
            assert_eq!(value.as_setting(), placement);
            let serialized = serde_json::to_value(value)
                .unwrap_or_else(|err| panic!("serialize placement {placement}: {err}"));
            assert_eq!(
                serde_json::from_value::<WorkSurfacePlacementValue>(serialized)
                    .unwrap_or_else(|err| panic!("deserialize placement {placement}: {err}")),
                value
            );
        }

        let _lock = lock_test_env();
        let temp_root = tempfile::tempdir().expect("isolated Codewhale home");
        let codewhale_home = temp_root.path().join(".codewhale");
        fs::create_dir_all(&codewhale_home).expect("settings dir");
        let settings_path = codewhale_home.join("settings.toml");
        fs::write(&settings_path, "work_surface_placement = \"bottom\"\n").expect("settings");
        let _home = EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let _codewhale_config = EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
        let _deepseek_config = EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");

        let mut app = app();
        let mut config = Config::default();
        let doc = build_document(&app, &config).expect("document");
        assert_eq!(
            doc.settings.work_surface_placement,
            WorkSurfacePlacementValue::Bottom,
            "the live bottom default must not degrade to top in the typed document"
        );
        let doc = parse_document(serde_json::to_value(&doc).expect("serialize document"))
            .expect("parse document");
        assert_eq!(
            doc.settings.work_surface_placement,
            WorkSurfacePlacementValue::Bottom
        );
        // Applying session-only must validate: `Settings::set` accepts
        // bottom, so the typed document never corrupts it.
        apply_document(doc, &mut app, &mut config, false).expect("apply placement");
    }

    #[test]
    fn ui_locale_round_trips_every_shipped_locale() {
        for locale in crate::localization::Locale::shipped() {
            let tag = locale.tag();
            let ui_locale = UiLocale::from_setting(tag)
                .unwrap_or_else(|err| panic!("UiLocale must accept shipped locale {tag}: {err}"));
            assert_eq!(
                ui_locale.as_setting(),
                tag,
                "UiLocale must preserve shipped locale {tag}"
            );
            let serialized = serde_json::to_value(ui_locale)
                .unwrap_or_else(|err| panic!("serialize UiLocale {tag}: {err}"));
            assert_eq!(serialized, serde_json::json!(tag));
            assert_eq!(
                serde_json::from_value::<UiLocale>(serialized)
                    .unwrap_or_else(|err| panic!("deserialize UiLocale {tag}: {err}")),
                ui_locale
            );
        }
    }

    #[test]
    fn parse_document_roundtrip() {
        let _lock = lock_test_env();
        let app = app();
        let config = Config::default();
        let doc = build_document(&app, &config).expect("document");
        let value = serde_json::to_value(doc.clone()).expect("json");
        let parsed = parse_document(value).expect("parsed");
        assert_eq!(parsed, doc);
    }

    #[test]
    fn session_only_apply_keeps_runtime_overrides_and_skips_reload() {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "codewhale-config-ui-session-only-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(temp_root.join(".deepseek")).expect("config dir");
        let config_path = temp_root.join(".deepseek").join("config.toml");
        fs::write(
            &config_path,
            r#"
model = "deepseek-v4-pro"
reasoning_effort = "max"
mcp_config_path = "disk-mcp.json"
"#,
        )
        .expect("seed config");

        let mut app = app();
        app.config_path = Some(config_path.clone());
        app.model = "deepseek-v4-pro".to_string();
        app.mcp_config_path = PathBuf::from("disk-mcp.json");
        app.reasoning_effort = ReasoningEffort::Max;
        let mut config = Config::load(Some(config_path), None).expect("load config");

        let mut doc = build_document(&app, &config).expect("document");
        doc.runtime.model = "deepseek-v4-flash".to_string();
        doc.config.reasoning_effort = ReasoningEffortValue::Low;
        doc.config.mcp_config_path = "session-mcp.json".to_string();
        doc.settings.cost_currency = CostCurrencyValue::Cny;

        let outcome = apply_document(doc, &mut app, &mut config, false).expect("apply");

        assert!(outcome.changed);
        assert!(outcome.requires_engine_sync);
        assert_eq!(app.model, "deepseek-v4-flash");
        assert_eq!(app.reasoning_effort, ReasoningEffort::Low);
        assert_eq!(app.mcp_config_path, PathBuf::from("session-mcp.json"));
        assert_eq!(app.cost_currency, crate::pricing::CostCurrency::Cny);
        assert_eq!(
            config.reasoning_effort.as_deref(),
            Some(ReasoningEffort::Low.as_setting())
        );
        assert_eq!(
            config.mcp_config_path.as_deref(),
            Some("disk-mcp.json"),
            "session-only apply must not reload persisted config back into runtime state"
        );
    }

    #[test]
    fn zai_document_persists_provider_model_without_changing_deepseek_fallback() {
        let _lock = lock_test_env();
        let temp_root = tempfile::tempdir().expect("isolated config dir");
        let state_dir = temp_root.path().join(".deepseek");
        fs::create_dir_all(&state_dir).expect("state dir");
        let config_path = state_dir.join("config.toml");
        fs::write(
            &config_path,
            r#"default_text_model = "deepseek-v4-pro"
mcp_config_path = "mcp.json"

[providers.zai]
model = "GLM-5.2"
"#,
        )
        .expect("seed config");
        fs::write(
            state_dir.join("settings.toml"),
            r#"default_provider = "zai"
default_model = "deepseek-v4-pro"

[provider_models]
zai = "GLM-5.2"
"#,
        )
        .expect("seed settings");

        let _config_path_guard = EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &config_path);

        let mut config = Config::load(Some(config_path.clone()), None).expect("load config");
        let mut app = app();
        app.config_path = Some(config_path.clone());
        app.set_provider_identity_record(
            config
                .active_provider_identity(ApiProvider::Zai)
                .expect("Z.ai identity"),
        );
        app.set_model_selection("GLM-5.2".to_string());

        let mut doc = build_document(&app, &config).expect("document");
        assert_eq!(doc.runtime.provider, "zai");
        assert_eq!(doc.runtime.model, "GLM-5.2");
        assert_eq!(
            doc.settings.provider_models.get("zai").map(String::as_str),
            Some("GLM-5.2")
        );
        doc.runtime.model = "glm-5-turbo".to_string();

        let outcome = apply_document(doc, &mut app, &mut config, true).expect("apply Z.ai model");
        assert!(outcome.changed);
        assert!(outcome.requires_engine_sync);
        assert_eq!(app.api_provider, ApiProvider::Zai);
        assert_eq!(app.model, "GLM-5-Turbo");

        let settings = Settings::load_persisted().expect("persisted settings");
        assert_eq!(
            settings
                .provider_models
                .as_ref()
                .and_then(|models| models.get("zai"))
                .map(String::as_str),
            Some("GLM-5-Turbo")
        );
        assert_eq!(settings.default_model.as_deref(), Some("deepseek-v4-pro"));
        let persisted = fs::read_to_string(&config_path).expect("persisted config");
        let persisted: toml::Value = toml::from_str(&persisted).expect("parse persisted config");
        assert_eq!(
            persisted["default_text_model"].as_str(),
            Some("deepseek-v4-pro")
        );
    }

    #[test]
    fn zai_document_rejects_a_deepseek_model_before_mutating_app() {
        let mut config = Config {
            provider: Some("zai".to_string()),
            ..Config::default()
        };
        let mut app = app();
        app.set_provider_identity(ApiProvider::Zai, "zai");
        app.set_model_selection("GLM-5.2".to_string());
        let mut doc = build_document(&app, &config).expect("document");
        doc.settings.provider_models.clear();
        doc.runtime.model = "deepseek-v4-flash".to_string();

        let error = apply_document(doc, &mut app, &mut config, false)
            .expect_err("foreign model must be rejected");

        assert!(
            error
                .to_string()
                .contains("not compatible with provider 'zai'")
        );
        assert_eq!(app.api_provider, ApiProvider::Zai);
        assert_eq!(app.model, "GLM-5.2");
    }

    #[test]
    fn custom_deepseek_endpoint_accepts_non_deepseek_model_on_save() {
        // Provider-only validation used to reject any non-DeepSeek id even when
        // the active deepseek route pointed at a custom OpenAI-compatible
        // gateway that owns its own model namespace.
        let mut config = Config {
            provider: Some("deepseek".to_string()),
            base_url: Some("https://tenant-gateway.example.test/v1".to_string()),
            default_text_model: Some("anthropic/private-model".to_string()),
            ..Config::default()
        };
        let mut app = app();
        app.set_provider_identity(ApiProvider::Deepseek, "deepseek");
        app.set_model_selection("anthropic/private-model".to_string());
        // Mirror a live custom-route session: the endpoint owns model ids.
        app.active_route_base_url = "https://tenant-gateway.example.test/v1".to_string();
        app.model_ids_passthrough = true;
        let mut doc = build_document(&app, &config).expect("document");
        doc.settings.provider_models.clear();
        doc.runtime.model = "org/custom-router-id".to_string();

        let outcome = apply_document(doc, &mut app, &mut config, false)
            .expect("custom deepseek endpoint must accept non-DeepSeek wire ids");

        assert!(outcome.changed);
        assert_eq!(app.model, "org/custom-router-id");
    }

    #[test]
    fn official_deepseek_endpoint_still_rejects_non_deepseek_model_on_save() {
        let mut config = Config {
            provider: Some("deepseek".to_string()),
            ..Config::default()
        };
        let mut app = app();
        app.set_provider_identity(ApiProvider::Deepseek, "deepseek");
        app.set_model_selection("deepseek-v4-pro".to_string());
        let mut doc = build_document(&app, &config).expect("document");
        doc.settings.provider_models.clear();
        doc.runtime.model = "org/custom-router-id".to_string();

        let error = apply_document(doc, &mut app, &mut config, false)
            .expect_err("official deepseek endpoint must reject non-DeepSeek ids");

        assert!(
            error.to_string().contains("org/custom-router-id"),
            "error should name the rejected model, got: {error}"
        );
        assert_eq!(app.model, "deepseek-v4-pro");
    }

    #[test]
    fn status_item_only_apply_does_not_require_engine_sync() {
        let _lock = lock_test_env();
        let dir = tempfile::tempdir().expect("isolated config dir");
        let mut app = app();
        app.config_path = Some(dir.path().join("config.toml"));
        let mut config = Config::default();
        let mut doc = build_document(&app, &config).expect("document");
        doc.config.status_items = vec![StatusItemValue::Cost, StatusItemValue::Model];

        let outcome = apply_document(doc, &mut app, &mut config, false).expect("apply");

        assert!(outcome.changed);
        assert!(!outcome.requires_engine_sync);
    }
}
