#[cfg(feature = "web")]
use std::net::SocketAddr;
#[cfg(feature = "web")]
use std::process::Command;
#[cfg(feature = "web")]
use std::time::Duration;

use anyhow::{Context, Result, bail};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands;
use crate::config::{Config, StatusItem, normalize_model_name};
use crate::localization::{normalize_configured_locale, resolve_locale};
use crate::settings::Settings;
use crate::tui::app::{
    App, AppMode, ComposerDensity, ReasoningEffort, SidebarFocus, TranscriptSpacing,
};
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
    #[schemars(title = "Current model")]
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
    pub paste_burst_detection: bool,
    pub show_thinking: bool,
    pub show_tool_details: bool,
    pub locale: UiLocale,
    pub theme: UiThemeValue,
    #[schemars(
        title = "Background color",
        description = "Main TUI background color as #RRGGBB"
    )]
    pub background_color: Option<String>,
    pub bracketed_paste: bool,
    pub composer_density: ComposerDensityValue,
    pub composer_border: bool,
    pub composer_vim_mode: ComposerVimModeValue,
    pub transcript_spacing: TranscriptSpacingValue,
    pub status_indicator: StatusIndicatorValue,
    pub synchronized_output: SynchronizedOutputValue,
    pub default_mode: DefaultModeValue,
    #[schemars(range(min = 10, max = 50))]
    pub sidebar_width: u16,
    pub sidebar_focus: SidebarFocusValue,
    pub context_panel: bool,
    #[schemars(range(min = 0))]
    pub max_history: usize,
    pub cost_currency: CostCurrencyValue,
    pub prefer_external_pdftotext: bool,
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
    #[serde(rename = "pt-BR")]
    #[schemars(rename = "pt-BR")]
    PtBr,
    #[serde(rename = "es-419")]
    #[schemars(rename = "es-419")]
    Es419,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UiThemeValue {
    System,
    Dark,
    Light,
    Grayscale,
    CatppuccinMocha,
    TokyoNight,
    Dracula,
    GruvboxDark,
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
pub enum TranscriptSpacingValue {
    Compact,
    Comfortable,
    Spacious,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DefaultModeValue {
    Agent,
    Plan,
    Yolo,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CostCurrencyValue {
    Usd,
    Cny,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SidebarFocusValue {
    Auto,
    Work,
    Tasks,
    Agents,
    Context,
    Hidden,
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
    Coherence,
    Agents,
    ReasoningReplay,
    PrefixStability,
    Cache,
    ContextPercent,
    GitBranch,
    LastToolElapsed,
    RateLimit,
}

pub fn parse_mode(arg: Option<&str>) -> Result<ConfigUiMode, String> {
    let raw = arg.unwrap_or("").trim();
    // Bare `/config` opens the legacy native modal — it matches the rest
    // of the deepseek-tui navy chrome out of the box. Power users can
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
    let settings = Settings::load().unwrap_or_default();
    let reasoning_effort = config
        .reasoning_effort()
        .map(ReasoningEffortValue::from_setting)
        .unwrap_or_else(|| app.reasoning_effort.into());
    let default_model = settings.default_model.clone();
    let status_items = app.status_items.iter().copied().map(Into::into).collect();
    Ok(ConfigUiDocument {
        runtime: RuntimeSection {
            model: app.model.clone(),
            approval_mode: app.approval_mode.into(),
        },
        settings: SettingsSection {
            auto_compact: settings.auto_compact,
            calm_mode: settings.calm_mode,
            low_motion: settings.low_motion,
            fancy_animations: settings.fancy_animations,
            paste_burst_detection: settings.paste_burst_detection,
            show_thinking: settings.show_thinking,
            show_tool_details: settings.show_tool_details,
            locale: UiLocale::from_setting(&settings.locale)?,
            theme: UiThemeValue::from_setting(&settings.theme)?,
            background_color: settings.background_color.clone(),
            bracketed_paste: settings.bracketed_paste,
            composer_density: settings.composer_density.as_str().into(),
            composer_border: settings.composer_border,
            composer_vim_mode: settings.composer_vim_mode.as_str().into(),
            transcript_spacing: settings.transcript_spacing.as_str().into(),
            status_indicator: settings.status_indicator.as_str().into(),
            synchronized_output: settings.synchronized_output.as_str().into(),
            default_mode: settings.default_mode.as_str().into(),
            sidebar_width: settings.sidebar_width_percent,
            sidebar_focus: settings.sidebar_focus.as_str().into(),
            context_panel: settings.context_panel,
            max_history: settings.max_input_history,
            cost_currency: CostCurrencyValue::from_setting(&settings.cost_currency)?,
            prefer_external_pdftotext: settings.prefer_external_pdftotext,
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
    schema["title"] = Value::String("DeepSeek TUI Config".to_string());
    schema["description"] =
        Value::String("Edit runtime and persisted TUI configuration.".to_string());
    schema
}

#[cfg(feature = "tui")]
pub fn run_tui_editor(app: &App, config: &Config) -> Result<ConfigUiDocument> {
    let document = build_document(app, config)?;
    let value = SchemaUI::new(serde_json::to_value(document.clone())?)
        .with_schema(build_schema())
        .with_title("DeepSeek TUI Config")
        .with_description("Edit persisted settings and live runtime knobs.")
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
        .with_title("DeepSeek TUI Config")
        .with_description("Save updates the browser draft. Exit commits changes back to the TUI.")
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
            let client = reqwest::Client::new();
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
    validate_document(&doc)?;
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
        (
            "paste_burst_detection",
            bool_str(doc.settings.paste_burst_detection),
        ),
        ("show_thinking", bool_str(doc.settings.show_thinking)),
        (
            "show_tool_details",
            bool_str(doc.settings.show_tool_details),
        ),
        ("locale", doc.settings.locale.as_setting()),
        ("theme", doc.settings.theme.as_setting()),
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
            "composer_vim_mode",
            doc.settings.composer_vim_mode.as_setting(),
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
        ("sidebar_width", &doc.settings.sidebar_width.to_string()),
        ("sidebar_focus", doc.settings.sidebar_focus.as_setting()),
        ("context_panel", bool_str(doc.settings.context_panel)),
        ("max_history", &doc.settings.max_history.to_string()),
        ("cost_currency", doc.settings.cost_currency.as_setting()),
        (
            "prefer_external_pdftotext",
            bool_str(doc.settings.prefer_external_pdftotext),
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
    }

    apply_reasoning_effort(app, config, doc.config.reasoning_effort, persist)?;
    let requires_engine_sync = app.compaction_config() != previous_compaction
        || app.reasoning_effort != previous_reasoning_effort;

    let new_status_items = parse_status_items(&doc.config.status_items);
    if app.status_items != new_status_items {
        app.status_items = new_status_items.clone();
        app.needs_redraw = true;
        if persist {
            let path = commands::persist_status_items(&new_status_items)?;
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
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return Err(anyhow::anyhow!(
        "browser opening is unsupported on this platform"
    ));

    let status = command
        .status()
        .context("failed to launch browser command")?;
    if !status.success() {
        bail!("browser command exited with status {status}");
    }
    Ok(())
}

fn validate_document(doc: &ConfigUiDocument) -> Result<()> {
    if !doc.runtime.model.trim().eq_ignore_ascii_case("auto")
        && normalize_model_name(&doc.runtime.model).is_none()
    {
        bail!("invalid model '{}'", doc.runtime.model);
    }
    if doc.config.mcp_config_path.trim().is_empty() {
        bail!("mcp_config_path cannot be empty");
    }
    Ok(())
}

fn reload_runtime_config(app: &mut App, config: &mut Config) -> Result<()> {
    let reloaded = Config::load(app.config_path.clone(), app.config_profile.as_deref())?;
    *config = reloaded.clone();
    app.api_provider = reloaded.api_provider();
    app.reasoning_effort = ReasoningEffort::from_setting(
        reloaded
            .reasoning_effort()
            .unwrap_or_else(|| app.reasoning_effort.as_setting()),
    );
    app.last_effective_reasoning_effort = None;
    app.update_model_compaction_budget();
    app.mcp_config_path = reloaded.mcp_config_path();
    app.skills_dir = reloaded.skills_dir();
    app.ui_locale = resolve_locale(&Settings::load().unwrap_or_default().locale);
    Ok(())
}

fn config_reload_notes(app: &App, config: &Config) -> Vec<String> {
    let mut notes = Vec::new();
    notes.push("Config saved and reloaded".to_string());
    if app.mcp_restart_required {
        notes.push(format!(
            "MCP tool pool still requires restart after {}",
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
    let effort: ReasoningEffort = value.into();
    app.reasoning_effort = effort;
    app.last_effective_reasoning_effort = None;
    app.update_model_compaction_budget();
    if persist {
        commands::persist_root_string_key("reasoning_effort", effort.as_setting())?;
    }
    config.reasoning_effort = Some(effort.as_setting().to_string());
    Ok(())
}

fn parse_status_items(items: &[StatusItemValue]) -> Vec<StatusItem> {
    items.iter().copied().map(Into::into).collect()
}

impl ApprovalModeValue {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Auto => "auto",
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
            Self::PtBr => "pt-BR",
            Self::Es419 => "es-419",
        }
    }

    fn from_setting(value: &str) -> Result<Self> {
        match normalize_configured_locale(value) {
            Some("auto") => Ok(Self::Auto),
            Some("en") => Ok(Self::En),
            Some("ja") => Ok(Self::Ja),
            Some("zh-Hans") => Ok(Self::ZhHans),
            Some("pt-BR") => Ok(Self::PtBr),
            Some("es-419") => Ok(Self::Es419),
            Some(other) => bail!("unsupported locale '{other}'"),
            None => bail!("invalid locale '{value}'"),
        }
    }
}

impl UiThemeValue {
    fn as_setting(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Grayscale => "grayscale",
            Self::CatppuccinMocha => "catppuccin-mocha",
            Self::TokyoNight => "tokyo-night",
            Self::Dracula => "dracula",
            Self::GruvboxDark => "gruvbox-dark",
        }
    }

    fn from_setting(value: &str) -> Result<Self> {
        match crate::palette::normalize_theme_name(value) {
            Some("system") => Ok(Self::System),
            Some("dark") => Ok(Self::Dark),
            Some("light") => Ok(Self::Light),
            Some("grayscale") => Ok(Self::Grayscale),
            Some("catppuccin-mocha") => Ok(Self::CatppuccinMocha),
            Some("tokyo-night") => Ok(Self::TokyoNight),
            Some("dracula") => Ok(Self::Dracula),
            Some("gruvbox-dark") => Ok(Self::GruvboxDark),
            Some(other) => bail!("unsupported theme '{other}'"),
            None => bail!("invalid theme '{value}'"),
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

impl TranscriptSpacingValue {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Comfortable => "comfortable",
            Self::Spacious => "spacious",
        }
    }
}

impl DefaultModeValue {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Plan => "plan",
            Self::Yolo => "yolo",
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

impl SidebarFocusValue {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Work => "work",
            Self::Tasks => "tasks",
            Self::Agents => "agents",
            Self::Context => "context",
            Self::Hidden => "hidden",
        }
    }
}

impl From<ApprovalMode> for ApprovalModeValue {
    fn from(value: ApprovalMode) -> Self {
        match value {
            ApprovalMode::Auto => Self::Auto,
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
        match AppMode::from_setting(value) {
            AppMode::Agent => Self::Agent,
            AppMode::Plan => Self::Plan,
            AppMode::Yolo => Self::Yolo,
        }
    }
}

impl StatusIndicatorValue {
    fn as_setting(self) -> &'static str {
        match self {
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
            "dots" | "dot" => Self::Dots,
            "off" | "none" | "hidden" | "false" => Self::Off,
            // Default to whale for "whale", aliases, and anything unknown
            // (we'd rather restore the historic indicator than silently
            // hide it on a typo).
            _ => Self::Whale,
        }
    }
}

impl From<&str> for SidebarFocusValue {
    fn from(value: &str) -> Self {
        match SidebarFocus::from_setting(value) {
            SidebarFocus::Auto => Self::Auto,
            SidebarFocus::Work => Self::Work,
            SidebarFocus::Tasks => Self::Tasks,
            SidebarFocus::Agents => Self::Agents,
            SidebarFocus::Context => Self::Context,
            SidebarFocus::Hidden => Self::Hidden,
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
            StatusItem::Coherence => Self::Coherence,
            StatusItem::Agents => Self::Agents,
            StatusItem::ReasoningReplay => Self::ReasoningReplay,
            StatusItem::PrefixStability => Self::PrefixStability,
            StatusItem::Cache => Self::Cache,
            StatusItem::ContextPercent => Self::ContextPercent,
            StatusItem::GitBranch => Self::GitBranch,
            StatusItem::LastToolElapsed => Self::LastToolElapsed,
            StatusItem::RateLimit => Self::RateLimit,
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
            StatusItemValue::Coherence => Self::Coherence,
            StatusItemValue::Agents => Self::Agents,
            StatusItemValue::ReasoningReplay => Self::ReasoningReplay,
            StatusItemValue::PrefixStability => Self::PrefixStability,
            StatusItemValue::Cache => Self::Cache,
            StatusItemValue::ContextPercent => Self::ContextPercent,
            StatusItemValue::GitBranch => Self::GitBranch,
            StatusItemValue::LastToolElapsed => Self::LastToolElapsed,
            StatusItemValue::RateLimit => Self::RateLimit,
        }
    }
}

fn bool_str(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::test_support::lock_test_env;
    use crate::tui::app::{App, TuiOptions};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: false,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            // Keep this fixture independent from the developer's saved
            // `default_mode` setting.
            start_in_agent_mode: true,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
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
        assert_eq!(doc.runtime.approval_mode, ApprovalModeValue::Suggest);
        assert_eq!(doc.config.reasoning_effort, ReasoningEffortValue::Max);
    }

    #[test]
    fn build_document_reflects_cost_currency_from_settings() {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "deepseek-config-ui-cost-currency-{}-{}",
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

        let old_config_path = std::env::var_os("DEEPSEEK_CONFIG_PATH");
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            std::env::set_var("DEEPSEEK_CONFIG_PATH", &config_path);
        }

        let app = app();
        let config = Config::default();
        let doc = build_document(&app, &config).expect("document");

        assert_eq!(doc.settings.cost_currency, CostCurrencyValue::Cny);
        // Safety: restore the guarded test-only environment mutation above.
        unsafe {
            if let Some(value) = old_config_path {
                std::env::set_var("DEEPSEEK_CONFIG_PATH", value);
            } else {
                std::env::remove_var("DEEPSEEK_CONFIG_PATH");
            }
        }
    }

    #[test]
    fn build_document_reflects_background_color_from_settings() {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "deepseek-config-ui-background-color-{}-{}",
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

        let old_config_path = std::env::var_os("DEEPSEEK_CONFIG_PATH");
        unsafe {
            std::env::set_var("DEEPSEEK_CONFIG_PATH", &config_path);
        }

        let app = app();
        let config = Config::default();
        let doc = build_document(&app, &config).expect("document");

        assert_eq!(doc.settings.background_color.as_deref(), Some("#1a1b26"));
        unsafe {
            if let Some(value) = old_config_path {
                std::env::set_var("DEEPSEEK_CONFIG_PATH", value);
            } else {
                std::env::remove_var("DEEPSEEK_CONFIG_PATH");
            }
        }
    }

    #[test]
    fn schema_contains_typed_enums() {
        let schema = build_schema();
        let approval_mode = &schema["$defs"]["ApprovalModeValue"]["enum"];
        assert_eq!(
            approval_mode,
            &serde_json::json!(["auto", "suggest", "never"])
        );
        let locale = &schema["$defs"]["UiLocale"]["enum"];
        assert_eq!(
            locale,
            &serde_json::json!(["auto", "en", "ja", "zh-Hans", "pt-BR", "es-419"])
        );
        let theme = &schema["$defs"]["UiThemeValue"]["enum"];
        assert_eq!(
            theme,
            &serde_json::json!([
                "system",
                "dark",
                "light",
                "grayscale",
                "catppuccin-mocha",
                "tokyo-night",
                "dracula",
                "gruvbox-dark"
            ])
        );
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
            "deepseek-config-ui-session-only-{}-{}",
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
    fn status_item_only_apply_does_not_require_engine_sync() {
        let _lock = lock_test_env();
        let mut app = app();
        let mut config = Config::default();
        let mut doc = build_document(&app, &config).expect("document");
        doc.config.status_items = vec![StatusItemValue::Cost, StatusItemValue::Model];

        let outcome = apply_document(doc, &mut app, &mut config, false).expect("apply");

        assert!(outcome.changed);
        assert!(!outcome.requires_engine_sync);
    }
}
