//! Unified DeepSeek TUI theme tokens.
//!
//! Merges the former `palette::UiTheme` (chrome colors) and
//! `deepseek_theme::Theme` (section/tool/plan colors) into a single struct
//! that covers every visual site in the TUI.  Built-in presets inherit from
//! both sources and can be overridden field-by-field via `Settings`.

use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{BorderType, Borders, Padding};

use crate::palette;
use crate::palette::PaletteMode;
use crate::tui::history::ToolStatus;

// ── Unified Theme struct ────────────────────────────────────────────────────

/// Centralized visual tokens for every rendered surface in the TUI.
///
/// Includes the former `UiTheme` chrome colours and the former
/// `deepseek_theme::Theme` section/tool/plan colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    // ── Chrome colours (ex palette::UiTheme) ──
    pub name: &'static str,
    pub mode: PaletteMode,
    pub surface_bg: Color,
    pub panel_bg: Color,
    pub sidebar_bg: Color, // NEW — defaults to panel_bg
    pub elevated_bg: Color,
    pub composer_bg: Color,
    pub selection_bg: Color,
    pub header_bg: Color,
    pub footer_bg: Color,
    pub mode_agent: Color,
    pub mode_yolo: Color,
    pub mode_plan: Color,
    pub status_ready: Color,
    pub status_working: Color,
    pub status_warning: Color,
    pub text_dim: Color,
    pub text_hint: Color,
    pub text_muted: Color,
    pub text_body: Color,
    pub text_soft: Color,
    pub border_color: Color,
    pub border_type: BorderType,         // NEW — Plain | Rounded
    pub section_border_type: BorderType, // NEW — defaults to border_type

    // ── Section / tool / plan colours (ex deepseek_theme::Theme) ──
    pub section_borders: Borders,
    pub section_border_color: Color,
    pub section_title_color: Color,
    pub section_padding: Padding,
    pub tool_title_color: Color,
    pub tool_value_color: Color,
    pub tool_label_color: Color,
    pub tool_running_accent: Color,
    pub tool_success_accent: Color,
    pub tool_failed_accent: Color,
    pub plan_progress_color: Color,
    pub plan_summary_color: Color,
    pub plan_explanation_color: Color,
    pub plan_pending_color: Color,
    pub plan_in_progress_color: Color,
    pub plan_completed_color: Color,
    /// Reasoning / thinking block background tint.  `None` means "use the
    /// built-in hardcoded tint" (`palette::SURFACE_REASONING_TINT`).
    /// Set via theme file `reasoning_bg = "#RRGGBB"` or `"reset"`.
    pub reasoning_bg: Option<Color>,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Parse a border type from a settings string.
#[must_use]
pub fn border_type_from_setting(value: &str) -> BorderType {
    match value.trim().to_ascii_lowercase().as_str() {
        "rounded" => BorderType::Rounded,
        _ => BorderType::Plain,
    }
}

// ── Custom theme file support ───────────────────────────────────────────────

use serde::{Deserialize, Serialize};

/// On-disk representation of a custom theme file.
///
/// Lives under `~/.config/deepseek/themes/<name>.toml`.  Only `base` is
/// required; every other key is optional and overrides the corresponding
/// slot in the base theme.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CustomThemeFile {
    /// Built-in theme to inherit from (e.g. `"tokyo-night"`, `"dark"`).
    pub base: String,
    /// Main TUI background colour (`#RRGGBB`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_color: Option<String>,
    /// Sidebar panel background colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidebar_bg: Option<String>,
    /// Composer input-area background colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub composer_bg: Option<String>,
    /// Border style: `"plain"` or `"rounded"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border_type: Option<String>,
    /// Reasoning block background tint (`#RRGGBB` or `"reset"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_bg: Option<String>,
    // ── Chrome colours ──
    /// Panel background colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub panel_bg: Option<String>,
    /// Elevated surface background.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elevated_bg: Option<String>,
    /// Selection highlight background.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_bg: Option<String>,
    /// Header bar background.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_bg: Option<String>,
    /// Footer bar background.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footer_bg: Option<String>,
    /// Agent mode indicator colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_agent: Option<String>,
    /// YOLO mode indicator colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_yolo: Option<String>,
    /// Plan mode indicator colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_plan: Option<String>,
    /// Ready status colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_ready: Option<String>,
    /// Working status colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_working: Option<String>,
    /// Warning status colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_warning: Option<String>,
    // ── Text colours ──
    /// Dimmed text colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_dim: Option<String>,
    /// Hint text colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_hint: Option<String>,
    /// Muted text colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_muted: Option<String>,
    /// Body text colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_body: Option<String>,
    /// Soft text colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_soft: Option<String>,
    /// Border line colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border_color: Option<String>,
    // ── Section / tool colours ──
    /// Section border colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section_border_color: Option<String>,
    /// Section title colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section_title_color: Option<String>,
    /// Tool title colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_title_color: Option<String>,
    /// Tool value colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_value_color: Option<String>,
    /// Tool label colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_label_color: Option<String>,
    /// Running tool accent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_running_accent: Option<String>,
    /// Successful tool accent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_success_accent: Option<String>,
    /// Failed tool accent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_failed_accent: Option<String>,
    // ── Plan colours ──
    /// Plan progress bar colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_progress_color: Option<String>,
    /// Plan summary colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_summary_color: Option<String>,
    /// Plan explanation colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_explanation_color: Option<String>,
    /// Pending plan step colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_pending_color: Option<String>,
    /// In-progress plan step colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_in_progress_color: Option<String>,
    /// Completed plan step colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_completed_color: Option<String>,
    /// Extra keys that might be added in the future.
    #[serde(flatten)]
    #[serde(skip_serializing)]
    #[allow(dead_code)]
    pub extras: std::collections::BTreeMap<String, toml::Value>,
}

impl CustomThemeFile {
    /// Serialise to a TOML string for display or saving.
    #[allow(dead_code)]
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Write this theme to `~/.config/deepseek/themes/<name>.toml`.
    #[allow(dead_code)]
    pub fn save(&self, name: &str) -> std::io::Result<()> {
        let dir = themes_dir();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{name}.toml"));
        let content = self
            .to_toml_string()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        std::fs::write(&path, content)
    }
}

impl Default for CustomThemeFile {
    fn default() -> Self {
        Self {
            base: "dark".to_string(),
            background_color: None,
            sidebar_bg: None,
            composer_bg: None,
            border_type: None,
            reasoning_bg: None,
            panel_bg: None,
            elevated_bg: None,
            selection_bg: None,
            header_bg: None,
            footer_bg: None,
            mode_agent: None,
            mode_yolo: None,
            mode_plan: None,
            status_ready: None,
            status_working: None,
            status_warning: None,
            text_dim: None,
            text_hint: None,
            text_muted: None,
            text_body: None,
            text_soft: None,
            border_color: None,
            section_border_color: None,
            section_title_color: None,
            tool_title_color: None,
            tool_value_color: None,
            tool_label_color: None,
            tool_running_accent: None,
            tool_success_accent: None,
            tool_failed_accent: None,
            plan_progress_color: None,
            plan_summary_color: None,
            plan_explanation_color: None,
            plan_pending_color: None,
            plan_in_progress_color: None,
            plan_completed_color: None,
            extras: std::collections::BTreeMap::new(),
        }
    }
}

/// Directory where custom theme files are stored.
#[must_use]
pub fn themes_dir() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("deepseek")
        .join("themes")
}

// ── Theme picker entry ──────────────────────────────────────────────────────

/// A single row in the `/theme` picker.  Either a built-in preset or a
/// custom theme file from `~/.config/deepseek/themes/`.
#[derive(Debug, Clone)]
pub enum ThemePickerEntry {
    /// A built-in named theme (System, Dark, Light, Grayscale, Catppuccin, …).
    Builtin(ThemeId),
    /// A custom theme file loaded from disk.  The string is the file stem
    /// (e.g. `"midnight"` for `midnight.toml`).
    Custom {
        /// File stem used for `file:<stem>` references.
        stem: String,
        /// Display label in the picker (e.g. `"Midnight (custom)"`).
        label: String,
        /// Resolved theme at construction time.
        theme: Box<Theme>,
    },
}

impl ThemePickerEntry {
    /// Settings string for the `theme` key.  Built-ins return the canonical
    /// name (e.g. `"dark"`); custom entries return `"file:<stem>"`.
    #[must_use]
    pub fn setting_name(&self) -> String {
        match self {
            Self::Builtin(id) => id.name().to_string(),
            Self::Custom { stem, .. } => format!("file:{stem}"),
        }
    }

    /// Human-readable label for the picker row.
    #[must_use]
    pub fn display_label(&self) -> &str {
        match self {
            Self::Builtin(id) => id.display_name(),
            Self::Custom { label, .. } => label.as_str(),
        }
    }

    /// Short tagline / description.
    #[must_use]
    pub fn tagline(&self) -> &str {
        match self {
            Self::Builtin(id) => id.tagline(),
            Self::Custom { .. } => "Custom theme file",
        }
    }

    /// Resolve to a concrete `Theme` for live-preview rendering.
    #[must_use]
    pub fn to_theme(&self) -> Theme {
        match self {
            Self::Builtin(id) => id.ui_theme(),
            Self::Custom { theme, .. } => **theme,
        }
    }
}

/// Build the full picker list: built-in themes first, then any `.toml`
/// custom theme files found in `themes_dir()`.
#[must_use]
pub fn build_picker_list() -> Vec<ThemePickerEntry> {
    let mut entries: Vec<ThemePickerEntry> = SELECTABLE_THEMES
        .iter()
        .map(|&id| ThemePickerEntry::Builtin(id))
        .collect();

    let dir = themes_dir();
    if let Ok(read_dir) = std::fs::read_dir(&dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                // Try to load the file to get a representative Theme for
                // the live-preview swatch.  If parsing fails, skip the
                // entry (broken files shouldn't crash the picker).
                if let Ok(theme) = Theme::from_toml_file(&path) {
                    // Derive a readable label from the stem.
                    let label = stem_to_label(stem);
                    entries.push(ThemePickerEntry::Custom {
                        stem: stem.to_string(),
                        label,
                        theme: Box::new(theme),
                    });
                }
            }
        }
    }

    entries
}

/// Convert a file stem like `"warm-paper"` into a display label like
/// `"Warm Paper (custom)"`.
fn stem_to_label(stem: &str) -> String {
    let pretty: String = stem
        .split(['-', '_'])
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => {
                    let mut s = c.to_uppercase().collect::<String>();
                    s.push_str(&chars.as_str().to_lowercase());
                    s
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("{pretty} (custom)")
}

// ── ThemeId ──────────────────────────────────────────────────────────────────

/// Stable identifiers for the named themes the user can select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeId {
    System,
    Whale,
    WhaleLight,
    Grayscale,
    CatppuccinMocha,
    TokyoNight,
    Dracula,
    GruvboxDark,
}

impl ThemeId {
    /// Parse a settings string (`"system"`, `"dark"`, `"catppuccin-mocha"`, …).
    #[must_use]
    pub fn from_name(value: &str) -> Option<Self> {
        match crate::palette::normalize_theme_name(value)? {
            "system" => Some(Self::System),
            "dark" => Some(Self::Whale),
            "light" => Some(Self::WhaleLight),
            "grayscale" => Some(Self::Grayscale),
            "catppuccin-mocha" => Some(Self::CatppuccinMocha),
            "tokyo-night" => Some(Self::TokyoNight),
            "dracula" => Some(Self::Dracula),
            "gruvbox-dark" => Some(Self::GruvboxDark),
            _ => None,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Whale => "dark",
            Self::WhaleLight => "light",
            Self::Grayscale => "grayscale",
            Self::CatppuccinMocha => "catppuccin-mocha",
            Self::TokyoNight => "tokyo-night",
            Self::Dracula => "dracula",
            Self::GruvboxDark => "gruvbox-dark",
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Whale => "Whale (Dark)",
            Self::WhaleLight => "Whale Light",
            Self::Grayscale => "Grayscale",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::TokyoNight => "Tokyo Night",
            Self::Dracula => "Dracula",
            Self::GruvboxDark => "Gruvbox Dark",
        }
    }

    #[must_use]
    pub const fn tagline(self) -> &'static str {
        match self {
            Self::System => "Follow terminal background (COLORFGBG)",
            Self::Whale => "Default DeepSeek dark blue",
            Self::WhaleLight => "DeepSeek light, paper-ish",
            Self::Grayscale => "Color-minimal high contrast",
            Self::CatppuccinMocha => "Soft pastels on warm dark",
            Self::TokyoNight => "Deep blue/violet night palette",
            Self::Dracula => "Classic high-contrast purple",
            Self::GruvboxDark => "Vintage warm earth tones",
        }
    }

    /// Resolve to a concrete `Theme`. For `System` this consults
    /// `PaletteMode::detect()` exactly once.
    #[must_use]
    pub fn ui_theme(self) -> Theme {
        match self {
            Self::System => Theme::detect(),
            Self::Whale => DARK_THEME,
            Self::WhaleLight => LIGHT_THEME,
            Self::Grayscale => GRAYSCALE_THEME,
            Self::CatppuccinMocha => CATPPUCCIN_MOCHA_THEME,
            Self::TokyoNight => TOKYO_NIGHT_THEME,
            Self::Dracula => DRACULA_THEME,
            Self::GruvboxDark => GRUVBOX_DARK_THEME,
        }
    }
}

/// Themes shown in the `/theme` picker, in display order.
pub const SELECTABLE_THEMES: &[ThemeId] = &[
    ThemeId::System,
    ThemeId::Whale,
    ThemeId::WhaleLight,
    ThemeId::Grayscale,
    ThemeId::CatppuccinMocha,
    ThemeId::TokyoNight,
    ThemeId::Dracula,
    ThemeId::GruvboxDark,
];

// ── Built-in Theme constants ─────────────────────────────────────────────────

// Common section defaults used by every built-in preset.
const SECT_BORDERS: Borders = Borders::ALL;
const SECT_PAD: Padding = Padding::horizontal(1);

/// Dark "Whale" theme — the original DeepSeek default.
pub const DARK_THEME: Theme = Theme {
    name: "whale",
    mode: PaletteMode::Dark,
    surface_bg: palette::DEEPSEEK_INK,
    panel_bg: palette::DEEPSEEK_SLATE,
    sidebar_bg: palette::DEEPSEEK_SLATE,
    elevated_bg: palette::SURFACE_ELEVATED,
    composer_bg: palette::DEEPSEEK_SLATE,
    selection_bg: palette::SELECTION_BG,
    header_bg: palette::DEEPSEEK_INK,
    footer_bg: palette::DEEPSEEK_INK,
    mode_agent: palette::MODE_AGENT,
    mode_yolo: palette::MODE_YOLO,
    mode_plan: palette::MODE_PLAN,
    status_ready: palette::TEXT_MUTED,
    status_working: palette::DEEPSEEK_SKY,
    status_warning: palette::STATUS_WARNING,
    text_dim: palette::TEXT_DIM,
    text_hint: palette::TEXT_HINT,
    text_muted: palette::TEXT_MUTED,
    text_body: palette::TEXT_BODY,
    text_soft: palette::TEXT_SOFT,
    border_color: palette::BORDER_COLOR,
    border_type: BorderType::Plain,
    section_border_type: BorderType::Plain,
    section_borders: SECT_BORDERS,
    section_border_color: palette::BORDER_COLOR,
    section_title_color: palette::DEEPSEEK_BLUE,
    section_padding: SECT_PAD,
    tool_title_color: palette::TEXT_SOFT,
    tool_value_color: palette::TEXT_MUTED,
    tool_label_color: palette::TEXT_DIM,
    tool_running_accent: palette::ACCENT_TOOL_LIVE,
    tool_success_accent: palette::TEXT_DIM,
    tool_failed_accent: palette::ACCENT_TOOL_ISSUE,
    plan_progress_color: palette::STATUS_SUCCESS,
    plan_summary_color: palette::TEXT_MUTED,
    plan_explanation_color: palette::TEXT_DIM,
    plan_pending_color: palette::TEXT_MUTED,
    plan_in_progress_color: palette::STATUS_WARNING,
    plan_completed_color: palette::STATUS_SUCCESS,
    reasoning_bg: None,
};

/// Light "Whale Light" theme.
pub const LIGHT_THEME: Theme = Theme {
    name: "whale-light",
    mode: PaletteMode::Light,
    surface_bg: palette::LIGHT_SURFACE,
    panel_bg: palette::LIGHT_PANEL,
    sidebar_bg: palette::LIGHT_PANEL,
    elevated_bg: palette::LIGHT_ELEVATED,
    composer_bg: palette::LIGHT_PANEL,
    selection_bg: palette::LIGHT_SELECTION_BG,
    header_bg: palette::LIGHT_SURFACE,
    footer_bg: palette::LIGHT_SURFACE,
    mode_agent: palette::DEEPSEEK_BLUE,
    mode_yolo: palette::DEEPSEEK_RED,
    mode_plan: Color::Rgb(180, 83, 9),
    status_ready: palette::LIGHT_TEXT_MUTED,
    status_working: palette::DEEPSEEK_BLUE,
    status_warning: Color::Rgb(180, 83, 9),
    text_dim: palette::LIGHT_TEXT_HINT,
    text_hint: palette::LIGHT_TEXT_HINT,
    text_muted: palette::LIGHT_TEXT_MUTED,
    text_body: palette::LIGHT_TEXT_BODY,
    text_soft: palette::LIGHT_TEXT_SOFT,
    border_color: palette::LIGHT_BORDER,
    border_type: BorderType::Plain,
    section_border_type: BorderType::Plain,
    section_borders: SECT_BORDERS,
    section_border_color: palette::LIGHT_BORDER,
    section_title_color: palette::DEEPSEEK_BLUE,
    section_padding: SECT_PAD,
    tool_title_color: palette::LIGHT_TEXT_SOFT,
    tool_value_color: palette::LIGHT_TEXT_MUTED,
    tool_label_color: palette::LIGHT_TEXT_HINT,
    tool_running_accent: palette::DEEPSEEK_BLUE,
    tool_success_accent: palette::LIGHT_TEXT_HINT,
    tool_failed_accent: palette::DEEPSEEK_RED,
    plan_progress_color: palette::DEEPSEEK_BLUE,
    plan_summary_color: palette::LIGHT_TEXT_MUTED,
    plan_explanation_color: palette::LIGHT_TEXT_HINT,
    plan_pending_color: palette::LIGHT_TEXT_MUTED,
    plan_in_progress_color: Color::Rgb(180, 83, 9),
    plan_completed_color: palette::DEEPSEEK_BLUE,
    reasoning_bg: None,
};

/// Grayscale / high-contrast neutral theme.
pub const GRAYSCALE_THEME: Theme = Theme {
    name: "grayscale",
    mode: PaletteMode::Grayscale,
    surface_bg: palette::GRAYSCALE_SURFACE,
    panel_bg: palette::GRAYSCALE_PANEL,
    sidebar_bg: palette::GRAYSCALE_PANEL,
    elevated_bg: palette::GRAYSCALE_ELEVATED,
    composer_bg: palette::GRAYSCALE_PANEL,
    selection_bg: palette::GRAYSCALE_SELECTION_BG,
    header_bg: palette::GRAYSCALE_SURFACE,
    footer_bg: palette::GRAYSCALE_SURFACE,
    mode_agent: palette::GRAYSCALE_TEXT_SOFT,
    mode_yolo: palette::GRAYSCALE_TEXT_BODY,
    mode_plan: palette::GRAYSCALE_TEXT_MUTED,
    status_ready: palette::GRAYSCALE_TEXT_MUTED,
    status_working: palette::GRAYSCALE_TEXT_SOFT,
    status_warning: palette::GRAYSCALE_TEXT_BODY,
    text_dim: palette::GRAYSCALE_TEXT_HINT,
    text_hint: palette::GRAYSCALE_TEXT_HINT,
    text_muted: palette::GRAYSCALE_TEXT_MUTED,
    text_body: palette::GRAYSCALE_TEXT_BODY,
    text_soft: palette::GRAYSCALE_TEXT_SOFT,
    border_color: palette::GRAYSCALE_BORDER,
    border_type: BorderType::Plain,
    section_border_type: BorderType::Plain,
    section_borders: SECT_BORDERS,
    section_border_color: palette::GRAYSCALE_BORDER,
    section_title_color: palette::GRAYSCALE_TEXT_SOFT,
    section_padding: SECT_PAD,
    tool_title_color: palette::GRAYSCALE_TEXT_SOFT,
    tool_value_color: palette::GRAYSCALE_TEXT_MUTED,
    tool_label_color: palette::GRAYSCALE_TEXT_HINT,
    tool_running_accent: palette::GRAYSCALE_TEXT_SOFT,
    tool_success_accent: palette::GRAYSCALE_TEXT_HINT,
    tool_failed_accent: palette::GRAYSCALE_TEXT_BODY,
    plan_progress_color: palette::GRAYSCALE_TEXT_SOFT,
    plan_summary_color: palette::GRAYSCALE_TEXT_MUTED,
    plan_explanation_color: palette::GRAYSCALE_TEXT_HINT,
    plan_pending_color: palette::GRAYSCALE_TEXT_MUTED,
    plan_in_progress_color: palette::GRAYSCALE_TEXT_BODY,
    plan_completed_color: palette::GRAYSCALE_TEXT_SOFT,
    reasoning_bg: None,
};

pub const CATPPUCCIN_MOCHA_THEME: Theme = {
    Theme {
        name: "catppuccin-mocha",
        mode: PaletteMode::Dark,
        surface_bg: Color::Rgb(0x1e, 0x1e, 0x2e),
        panel_bg: Color::Rgb(0x18, 0x18, 0x25),
        sidebar_bg: Color::Rgb(0x18, 0x18, 0x25),
        elevated_bg: Color::Rgb(0x31, 0x32, 0x44),
        composer_bg: Color::Rgb(0x18, 0x18, 0x25),
        selection_bg: Color::Rgb(0x45, 0x47, 0x5a),
        header_bg: Color::Rgb(0x11, 0x11, 0x1b),
        footer_bg: Color::Rgb(0x11, 0x11, 0x1b),
        mode_agent: Color::Rgb(0x89, 0xb4, 0xfa),
        mode_yolo: Color::Rgb(0xf3, 0x8b, 0xa8),
        mode_plan: Color::Rgb(0xfa, 0xb3, 0x87),
        status_ready: Color::Rgb(0x7f, 0x84, 0x9c),
        status_working: Color::Rgb(0x74, 0xc7, 0xec),
        status_warning: Color::Rgb(0xf9, 0xe2, 0xaf),
        text_dim: Color::Rgb(0x6c, 0x70, 0x86),
        text_hint: Color::Rgb(0x7f, 0x84, 0x9c),
        text_muted: Color::Rgb(0xa6, 0xad, 0xc8),
        text_body: Color::Rgb(0xcd, 0xd6, 0xf4),
        text_soft: Color::Rgb(0xba, 0xc2, 0xde),
        border_color: Color::Rgb(0x45, 0x47, 0x5a),
        border_type: BorderType::Plain,
        section_border_type: BorderType::Plain,
        section_borders: Borders::ALL,
        section_border_color: Color::Rgb(0x45, 0x47, 0x5a),
        section_title_color: Color::Rgb(0xba, 0xc2, 0xde),
        section_padding: Padding::horizontal(1),
        tool_title_color: Color::Rgb(0xba, 0xc2, 0xde),
        tool_value_color: Color::Rgb(0xa6, 0xad, 0xc8),
        tool_label_color: Color::Rgb(0x7f, 0x84, 0x9c),
        tool_running_accent: Color::Rgb(0x74, 0xc7, 0xec),
        tool_success_accent: Color::Rgb(0x7f, 0x84, 0x9c),
        tool_failed_accent: Color::Rgb(0xf3, 0x8b, 0xa8),
        plan_progress_color: Color::Rgb(0x74, 0xc7, 0xec),
        plan_summary_color: Color::Rgb(0xa6, 0xad, 0xc8),
        plan_explanation_color: Color::Rgb(0x7f, 0x84, 0x9c),
        plan_pending_color: Color::Rgb(0xa6, 0xad, 0xc8),
        plan_in_progress_color: Color::Rgb(0xf9, 0xe2, 0xaf),
        plan_completed_color: Color::Rgb(0x74, 0xc7, 0xec),
        reasoning_bg: None,
    }
};

pub const TOKYO_NIGHT_THEME: Theme = {
    Theme {
        name: "tokyo-night",
        mode: PaletteMode::Dark,
        surface_bg: Color::Rgb(0x1a, 0x1b, 0x26),
        panel_bg: Color::Rgb(0x16, 0x16, 0x1e),
        sidebar_bg: Color::Rgb(0x16, 0x16, 0x1e),
        elevated_bg: Color::Rgb(0x29, 0x2e, 0x42),
        composer_bg: Color::Rgb(0x16, 0x16, 0x1e),
        selection_bg: Color::Rgb(0x28, 0x34, 0x57),
        header_bg: Color::Rgb(0x16, 0x16, 0x1e),
        footer_bg: Color::Rgb(0x16, 0x16, 0x1e),
        mode_agent: Color::Rgb(0x7a, 0xa2, 0xf7),
        mode_yolo: Color::Rgb(0xf7, 0x76, 0x8e),
        mode_plan: Color::Rgb(0xff, 0x9e, 0x64),
        status_ready: Color::Rgb(0x56, 0x5f, 0x89),
        status_working: Color::Rgb(0x7d, 0xcf, 0xff),
        status_warning: Color::Rgb(0xe0, 0xaf, 0x68),
        text_dim: Color::Rgb(0x56, 0x5f, 0x89),
        text_hint: Color::Rgb(0x73, 0x7a, 0xa2),
        text_muted: Color::Rgb(0xa9, 0xb1, 0xd6),
        text_body: Color::Rgb(0xc0, 0xca, 0xf5),
        text_soft: Color::Rgb(0xbb, 0xc2, 0xe0),
        border_color: Color::Rgb(0x41, 0x48, 0x68),
        border_type: BorderType::Plain,
        section_border_type: BorderType::Plain,
        section_borders: Borders::ALL,
        section_border_color: Color::Rgb(0x41, 0x48, 0x68),
        section_title_color: Color::Rgb(0xbb, 0xc2, 0xe0),
        section_padding: Padding::horizontal(1),
        tool_title_color: Color::Rgb(0xbb, 0xc2, 0xe0),
        tool_value_color: Color::Rgb(0xa9, 0xb1, 0xd6),
        tool_label_color: Color::Rgb(0x73, 0x7a, 0xa2),
        tool_running_accent: Color::Rgb(0x7d, 0xcf, 0xff),
        tool_success_accent: Color::Rgb(0x73, 0x7a, 0xa2),
        tool_failed_accent: Color::Rgb(0xf7, 0x76, 0x8e),
        plan_progress_color: Color::Rgb(0x7d, 0xcf, 0xff),
        plan_summary_color: Color::Rgb(0xa9, 0xb1, 0xd6),
        plan_explanation_color: Color::Rgb(0x73, 0x7a, 0xa2),
        plan_pending_color: Color::Rgb(0xa9, 0xb1, 0xd6),
        plan_in_progress_color: Color::Rgb(0xe0, 0xaf, 0x68),
        plan_completed_color: Color::Rgb(0x7d, 0xcf, 0xff),
        reasoning_bg: None,
    }
};

pub const DRACULA_THEME: Theme = {
    Theme {
        name: "dracula",
        mode: PaletteMode::Dark,
        surface_bg: Color::Rgb(0x28, 0x2a, 0x36),
        panel_bg: Color::Rgb(0x21, 0x22, 0x2c),
        sidebar_bg: Color::Rgb(0x21, 0x22, 0x2c),
        elevated_bg: Color::Rgb(0x34, 0x37, 0x46),
        composer_bg: Color::Rgb(0x21, 0x22, 0x2c),
        selection_bg: Color::Rgb(0x44, 0x47, 0x5a),
        header_bg: Color::Rgb(0x21, 0x22, 0x2c),
        footer_bg: Color::Rgb(0x21, 0x22, 0x2c),
        mode_agent: Color::Rgb(0xbd, 0x93, 0xf9),
        mode_yolo: Color::Rgb(0xff, 0x55, 0x55),
        mode_plan: Color::Rgb(0xff, 0xb8, 0x6c),
        status_ready: Color::Rgb(0x62, 0x72, 0xa4),
        status_working: Color::Rgb(0x8b, 0xe9, 0xfd),
        status_warning: Color::Rgb(0xf1, 0xfa, 0x8c),
        text_dim: Color::Rgb(0x62, 0x72, 0xa4),
        text_hint: Color::Rgb(0x8a, 0x8e, 0xaa),
        text_muted: Color::Rgb(0xc0, 0xc4, 0xd6),
        text_body: Color::Rgb(0xf8, 0xf8, 0xf2),
        text_soft: Color::Rgb(0xe2, 0xe2, 0xdc),
        border_color: Color::Rgb(0x44, 0x47, 0x5a),
        border_type: BorderType::Plain,
        section_border_type: BorderType::Plain,
        section_borders: Borders::ALL,
        section_border_color: Color::Rgb(0x44, 0x47, 0x5a),
        section_title_color: Color::Rgb(0xe2, 0xe2, 0xdc),
        section_padding: Padding::horizontal(1),
        tool_title_color: Color::Rgb(0xe2, 0xe2, 0xdc),
        tool_value_color: Color::Rgb(0xc0, 0xc4, 0xd6),
        tool_label_color: Color::Rgb(0x8a, 0x8e, 0xaa),
        tool_running_accent: Color::Rgb(0x8b, 0xe9, 0xfd),
        tool_success_accent: Color::Rgb(0x8a, 0x8e, 0xaa),
        tool_failed_accent: Color::Rgb(0xff, 0x55, 0x55),
        plan_progress_color: Color::Rgb(0x8b, 0xe9, 0xfd),
        plan_summary_color: Color::Rgb(0xc0, 0xc4, 0xd6),
        plan_explanation_color: Color::Rgb(0x8a, 0x8e, 0xaa),
        plan_pending_color: Color::Rgb(0xc0, 0xc4, 0xd6),
        plan_in_progress_color: Color::Rgb(0xf1, 0xfa, 0x8c),
        plan_completed_color: Color::Rgb(0x8b, 0xe9, 0xfd),
        reasoning_bg: None,
    }
};

pub const GRUVBOX_DARK_THEME: Theme = {
    Theme {
        name: "gruvbox-dark",
        mode: PaletteMode::Dark,
        surface_bg: Color::Rgb(0x28, 0x28, 0x28),
        panel_bg: Color::Rgb(0x3c, 0x38, 0x36),
        sidebar_bg: Color::Rgb(0x3c, 0x38, 0x36),
        elevated_bg: Color::Rgb(0x50, 0x49, 0x45),
        composer_bg: Color::Rgb(0x3c, 0x38, 0x36),
        selection_bg: Color::Rgb(0x66, 0x5c, 0x54),
        header_bg: Color::Rgb(0x1d, 0x20, 0x21),
        footer_bg: Color::Rgb(0x1d, 0x20, 0x21),
        mode_agent: Color::Rgb(0x83, 0xa5, 0x98),
        mode_yolo: Color::Rgb(0xfb, 0x49, 0x34),
        mode_plan: Color::Rgb(0xfe, 0x80, 0x19),
        status_ready: Color::Rgb(0x92, 0x83, 0x74),
        status_working: Color::Rgb(0x8e, 0xc0, 0x7c),
        status_warning: Color::Rgb(0xfa, 0xbd, 0x2f),
        text_dim: Color::Rgb(0x92, 0x83, 0x74),
        text_hint: Color::Rgb(0xa8, 0x99, 0x84),
        text_muted: Color::Rgb(0xbd, 0xae, 0x93),
        text_body: Color::Rgb(0xeb, 0xdb, 0xb2),
        text_soft: Color::Rgb(0xd5, 0xc4, 0xa1),
        border_color: Color::Rgb(0x66, 0x5c, 0x54),
        border_type: BorderType::Plain,
        section_border_type: BorderType::Plain,
        section_borders: Borders::ALL,
        section_border_color: Color::Rgb(0x66, 0x5c, 0x54),
        section_title_color: Color::Rgb(0xd5, 0xc4, 0xa1),
        section_padding: Padding::horizontal(1),
        tool_title_color: Color::Rgb(0xd5, 0xc4, 0xa1),
        tool_value_color: Color::Rgb(0xbd, 0xae, 0x93),
        tool_label_color: Color::Rgb(0xa8, 0x99, 0x84),
        tool_running_accent: Color::Rgb(0x8e, 0xc0, 0x7c),
        tool_success_accent: Color::Rgb(0xa8, 0x99, 0x84),
        tool_failed_accent: Color::Rgb(0xfb, 0x49, 0x34),
        plan_progress_color: Color::Rgb(0x8e, 0xc0, 0x7c),
        plan_summary_color: Color::Rgb(0xbd, 0xae, 0x93),
        plan_explanation_color: Color::Rgb(0xa8, 0x99, 0x84),
        plan_pending_color: Color::Rgb(0xbd, 0xae, 0x93),
        plan_in_progress_color: Color::Rgb(0xfa, 0xbd, 0x2f),
        plan_completed_color: Color::Rgb(0x8e, 0xc0, 0x7c),
        reasoning_bg: None,
    }
};

/// Apply a single hex-colour override from a `CustomThemeFile` field.
/// If the field is `Some` and parses to a valid colour, calls the
/// corresponding `with_<field>` builder method.
macro_rules! try_override {
    ($theme:ident, $custom:expr, $field:ident, $with:ident) => {
        if let Some(ref c) = $custom.$field {
            if let Some(color) = $crate::palette::parse_hex_rgb_color(c) {
                $theme = $theme.$with(color);
            }
        }
    };
}

// ── Theme methods ────────────────────────────────────────────────────────────

impl Theme {
    #[must_use]
    pub fn for_mode(mode: PaletteMode) -> Self {
        match mode {
            PaletteMode::Dark => DARK_THEME,
            PaletteMode::Light => LIGHT_THEME,
            PaletteMode::Grayscale => GRAYSCALE_THEME,
        }
    }

    #[must_use]
    pub fn detect() -> Self {
        Self::for_mode(PaletteMode::detect())
    }

    #[must_use]
    #[allow(dead_code)] // kept for backward compatibility with palette.rs callers
    pub fn from_setting(value: &str) -> Option<Self> {
        ThemeId::from_name(value).map(ThemeId::ui_theme)
    }

    /// Returns `true` for community presets (Catppuccin, Tokyo Night,
    /// Dracula, Gruvbox) that need per-cell colour remapping at the
    /// backend layer.  Replaces the former `palette::theme_remap_active`.
    #[must_use]
    pub fn is_community_preset(&self) -> bool {
        matches!(
            self.name,
            "catppuccin-mocha" | "tokyo-night" | "dracula" | "gruvbox-dark"
        )
    }

    /// Derive the `ThemeId` this instance corresponds to.
    /// For `System` the result is not meaningful — callers should use
    /// `is_community_preset()` first.
    #[must_use]
    pub fn theme_id(&self) -> ThemeId {
        ThemeId::from_name(self.name).unwrap_or(ThemeId::System)
    }

    #[must_use]
    pub fn with_background_color(mut self, color: Color) -> Self {
        self.surface_bg = color;
        self.header_bg = color;
        self.footer_bg = color;
        self
    }

    #[must_use]
    pub fn with_sidebar_bg(mut self, color: Color) -> Self {
        self.sidebar_bg = color;
        self
    }

    #[must_use]
    pub fn with_composer_bg(mut self, color: Color) -> Self {
        self.composer_bg = color;
        self
    }

    #[must_use]
    pub fn with_border_type(mut self, bt: BorderType) -> Self {
        self.border_type = bt;
        self
    }

    #[must_use]
    pub fn with_section_border_type(mut self, bt: BorderType) -> Self {
        self.section_border_type = bt;
        self
    }

    #[must_use]
    pub fn with_reasoning_bg(mut self, color: Option<Color>) -> Self {
        self.reasoning_bg = color;
        self
    }

    // ── Chrome colour overrides ──

    #[must_use]
    pub fn with_panel_bg(mut self, color: Color) -> Self {
        self.panel_bg = color;
        self
    }
    #[must_use]
    pub fn with_elevated_bg(mut self, color: Color) -> Self {
        self.elevated_bg = color;
        self
    }
    #[must_use]
    pub fn with_selection_bg(mut self, color: Color) -> Self {
        self.selection_bg = color;
        self
    }
    #[must_use]
    pub fn with_header_bg(mut self, color: Color) -> Self {
        self.header_bg = color;
        self
    }
    #[must_use]
    pub fn with_footer_bg(mut self, color: Color) -> Self {
        self.footer_bg = color;
        self
    }
    #[must_use]
    pub fn with_mode_agent(mut self, color: Color) -> Self {
        self.mode_agent = color;
        self
    }
    #[must_use]
    pub fn with_mode_yolo(mut self, color: Color) -> Self {
        self.mode_yolo = color;
        self
    }
    #[must_use]
    pub fn with_mode_plan(mut self, color: Color) -> Self {
        self.mode_plan = color;
        self
    }
    #[must_use]
    pub fn with_status_ready(mut self, color: Color) -> Self {
        self.status_ready = color;
        self
    }
    #[must_use]
    pub fn with_status_working(mut self, color: Color) -> Self {
        self.status_working = color;
        self
    }
    #[must_use]
    pub fn with_status_warning(mut self, color: Color) -> Self {
        self.status_warning = color;
        self
    }

    // ── Text colour overrides ──

    #[must_use]
    pub fn with_text_dim(mut self, color: Color) -> Self {
        self.text_dim = color;
        self
    }
    #[must_use]
    pub fn with_text_hint(mut self, color: Color) -> Self {
        self.text_hint = color;
        self
    }
    #[must_use]
    pub fn with_text_muted(mut self, color: Color) -> Self {
        self.text_muted = color;
        self
    }
    #[must_use]
    pub fn with_text_body(mut self, color: Color) -> Self {
        self.text_body = color;
        self
    }
    #[must_use]
    pub fn with_text_soft(mut self, color: Color) -> Self {
        self.text_soft = color;
        self
    }

    // ── Border colour override ──

    #[must_use]
    pub fn with_border_color(mut self, color: Color) -> Self {
        self.border_color = color;
        self
    }

    // ── Section / tool colour overrides ──

    #[must_use]
    pub fn with_section_border_color(mut self, color: Color) -> Self {
        self.section_border_color = color;
        self
    }
    #[must_use]
    pub fn with_section_title_color(mut self, color: Color) -> Self {
        self.section_title_color = color;
        self
    }
    #[must_use]
    pub fn with_tool_title_color(mut self, color: Color) -> Self {
        self.tool_title_color = color;
        self
    }
    #[must_use]
    pub fn with_tool_value_color(mut self, color: Color) -> Self {
        self.tool_value_color = color;
        self
    }
    #[must_use]
    pub fn with_tool_label_color(mut self, color: Color) -> Self {
        self.tool_label_color = color;
        self
    }
    #[must_use]
    pub fn with_tool_running_accent(mut self, color: Color) -> Self {
        self.tool_running_accent = color;
        self
    }
    #[must_use]
    pub fn with_tool_success_accent(mut self, color: Color) -> Self {
        self.tool_success_accent = color;
        self
    }
    #[must_use]
    pub fn with_tool_failed_accent(mut self, color: Color) -> Self {
        self.tool_failed_accent = color;
        self
    }

    // ── Plan colour overrides ──

    #[must_use]
    pub fn with_plan_progress_color(mut self, color: Color) -> Self {
        self.plan_progress_color = color;
        self
    }
    #[must_use]
    pub fn with_plan_summary_color(mut self, color: Color) -> Self {
        self.plan_summary_color = color;
        self
    }
    #[must_use]
    pub fn with_plan_explanation_color(mut self, color: Color) -> Self {
        self.plan_explanation_color = color;
        self
    }
    #[must_use]
    pub fn with_plan_pending_color(mut self, color: Color) -> Self {
        self.plan_pending_color = color;
        self
    }
    #[must_use]
    pub fn with_plan_in_progress_color(mut self, color: Color) -> Self {
        self.plan_in_progress_color = color;
        self
    }
    #[must_use]
    pub fn with_plan_completed_color(mut self, color: Color) -> Self {
        self.plan_completed_color = color;
        self
    }

    /// Apply overrides from `Settings`-style optional fields on top of a
    /// resolved base theme.  This is the canonical entry point for building
    /// the runtime `Theme` from `Settings`.
    #[must_use]
    pub fn from_settings(
        theme_setting: &str,
        border_type_setting: Option<&str>,
        section_border_type: Option<&str>,
    ) -> Self {
        let mut t =
            Self::from_setting_or_file(theme_setting).unwrap_or_else(|| ThemeId::System.ui_theme());

        // Only override border_type from settings when the user explicitly
        // configured it.  Custom theme files already set their own.
        if let Some(bt_str) = border_type_setting {
            t = t.with_border_type(border_type_from_setting(bt_str));
        }
        // section_border_type falls back to the (possibly overridden) border_type
        let sbt = section_border_type.map_or(t.border_type, border_type_from_setting);
        t.section_borders = Borders::ALL;
        t = t.with_section_border_type(sbt);
        t
    }

    /// Load a custom theme from a TOML file.
    ///
    /// The file MUST contain a `base` key naming one of the built-in themes.
    /// Any additional keys override the corresponding field of the base theme.
    ///
    /// # Example `~/.config/deepseek/themes/midnight.toml`
    ///
    /// ```toml
    /// base = "tokyo-night"
    /// background_color = "#000000"
    /// sidebar_bg = "#111111"
    /// border_type = "rounded"
    /// ```
    pub fn from_toml_file(path: &std::path::Path) -> std::io::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let custom: CustomThemeFile = toml::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        Ok(Self::from_custom_theme(&custom))
    }

    /// Build a `Theme` from a deserialized `CustomThemeFile`.
    #[must_use]
    pub fn from_custom_theme(custom: &CustomThemeFile) -> Self {
        let base_id = ThemeId::from_name(&custom.base).unwrap_or(ThemeId::Whale);
        let mut t = base_id.ui_theme();

        try_override!(t, custom, background_color, with_background_color);
        try_override!(t, custom, sidebar_bg, with_sidebar_bg);
        try_override!(t, custom, composer_bg, with_composer_bg);

        if let Some(ref bt) = custom.border_type {
            t = t.with_border_type(border_type_from_setting(bt));
        }
        if let Some(ref c) = custom.reasoning_bg {
            let color = palette::parse_hex_rgb_color(c);
            t = t.with_reasoning_bg(color);
        }

        // ── Apply chrome colour overrides ──
        try_override!(t, custom, panel_bg, with_panel_bg);
        try_override!(t, custom, elevated_bg, with_elevated_bg);
        try_override!(t, custom, selection_bg, with_selection_bg);
        try_override!(t, custom, header_bg, with_header_bg);
        try_override!(t, custom, footer_bg, with_footer_bg);
        try_override!(t, custom, mode_agent, with_mode_agent);
        try_override!(t, custom, mode_yolo, with_mode_yolo);
        try_override!(t, custom, mode_plan, with_mode_plan);
        try_override!(t, custom, status_ready, with_status_ready);
        try_override!(t, custom, status_working, with_status_working);
        try_override!(t, custom, status_warning, with_status_warning);

        // ── Apply text colour overrides ──
        try_override!(t, custom, text_dim, with_text_dim);
        try_override!(t, custom, text_hint, with_text_hint);
        try_override!(t, custom, text_muted, with_text_muted);
        try_override!(t, custom, text_body, with_text_body);
        try_override!(t, custom, text_soft, with_text_soft);

        // ── Apply border colour override ──
        try_override!(t, custom, border_color, with_border_color);

        // ── Apply section/tool colour overrides ──
        try_override!(t, custom, section_border_color, with_section_border_color);
        try_override!(t, custom, section_title_color, with_section_title_color);
        try_override!(t, custom, tool_title_color, with_tool_title_color);
        try_override!(t, custom, tool_value_color, with_tool_value_color);
        try_override!(t, custom, tool_label_color, with_tool_label_color);
        try_override!(t, custom, tool_running_accent, with_tool_running_accent);
        try_override!(t, custom, tool_success_accent, with_tool_success_accent);
        try_override!(t, custom, tool_failed_accent, with_tool_failed_accent);

        // ── Apply plan colour overrides ──
        try_override!(t, custom, plan_progress_color, with_plan_progress_color);
        try_override!(t, custom, plan_summary_color, with_plan_summary_color);
        try_override!(
            t,
            custom,
            plan_explanation_color,
            with_plan_explanation_color
        );
        try_override!(t, custom, plan_pending_color, with_plan_pending_color);
        try_override!(
            t,
            custom,
            plan_in_progress_color,
            with_plan_in_progress_color
        );
        try_override!(t, custom, plan_completed_color, with_plan_completed_color);

        t
    }

    /// Resolve a theme setting string that may be a built-in name or
    /// a `file:<name>` reference to a custom theme file in the themes
    /// directory.
    #[must_use]
    pub fn from_setting_or_file(theme_setting: &str) -> Option<Self> {
        if let Some(file_name) = theme_setting.strip_prefix("file:") {
            return Self::load_custom_theme(file_name);
        }
        // Try built-in
        let base = Self::from_setting(theme_setting);
        if base.is_some() {
            return base;
        }
        // Fallback: try loading as a custom theme file name
        Self::load_custom_theme(theme_setting)
    }

    /// Load a custom theme by file name (without directory path).
    /// Searches `~/.config/deepseek/themes/<name>.toml`.
    #[must_use]
    pub fn load_custom_theme(name: &str) -> Option<Self> {
        let path = themes_dir().join(format!("{name}.toml"));
        if path.exists() {
            Self::from_toml_file(&path).ok()
        } else {
            // also try bare name without extension
            let path = themes_dir().join(name);
            if path.exists() {
                Self::from_toml_file(&path).ok()
            } else {
                None
            }
        }
    }

    // ── Tool/plan style helpers (ex deepseek_theme::Theme) ─────────────────

    #[must_use]
    pub const fn tool_status_color(self, status: ToolStatus) -> Color {
        match status {
            ToolStatus::Running => self.tool_running_accent,
            ToolStatus::Success => self.tool_success_accent,
            ToolStatus::Failed => self.tool_failed_accent,
        }
    }

    #[must_use]
    pub fn tool_title_style(self) -> Style {
        Style::default()
            .fg(self.tool_title_color)
            .add_modifier(Modifier::BOLD)
    }

    #[must_use]
    pub fn tool_status_style(self, status: ToolStatus) -> Style {
        Style::default().fg(self.tool_status_color(status))
    }

    #[must_use]
    pub fn tool_label_style(self) -> Style {
        Style::default().fg(self.tool_label_color)
    }

    #[must_use]
    pub fn tool_value_style(self) -> Style {
        Style::default().fg(self.tool_value_color)
    }
}

// ── Backward-compat re-exports from palette ──────────────────────────────────

/// Returns the active theme — kept for `history.rs` tests.
#[must_use]
pub fn active_theme() -> Theme {
    DARK_THEME
}

/// Build a `UiTheme`-equivalent from settings strings (palette.rs compat).
#[must_use]
#[allow(dead_code)] // compat wrapper for external callers
pub fn ui_theme_from_settings(theme: &str, background_color: Option<&str>) -> Theme {
    let mut t = Theme::from_setting(theme).unwrap_or_else(Theme::detect);
    if let Some(background) = background_color.and_then(palette::parse_hex_rgb_color) {
        t = t.with_background_color(background);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette;
    use crate::tui::history::ToolStatus;

    #[test]
    fn active_theme_returns_dark() {
        assert_eq!(active_theme(), DARK_THEME);
    }

    #[test]
    fn dark_theme_matches_existing_palette_choices() {
        let theme = DARK_THEME;
        assert_eq!(theme.mode, PaletteMode::Dark);
        assert_eq!(theme.border_color, palette::BORDER_COLOR);
        assert_eq!(theme.surface_bg, palette::DEEPSEEK_INK);
        assert_eq!(theme.section_title_color, palette::DEEPSEEK_BLUE);
        assert_eq!(theme.tool_title_color, palette::TEXT_SOFT);
        assert_eq!(theme.tool_value_color, palette::TEXT_MUTED);
        assert_eq!(theme.tool_label_color, palette::TEXT_DIM);
        assert_eq!(theme.tool_running_accent, palette::ACCENT_TOOL_LIVE);
        assert_eq!(theme.tool_success_accent, palette::TEXT_DIM);
        assert_eq!(theme.tool_failed_accent, palette::ACCENT_TOOL_ISSUE);
    }

    #[test]
    fn light_theme_uses_light_panel_tokens() {
        let theme = Theme::for_mode(PaletteMode::Light);
        assert_eq!(theme.mode, PaletteMode::Light);
        assert_eq!(theme.panel_bg, palette::LIGHT_PANEL);
        assert_eq!(theme.sidebar_bg, palette::LIGHT_PANEL);
        assert_eq!(theme.border_color, palette::LIGHT_BORDER);
        assert_eq!(theme.tool_title_color, palette::LIGHT_TEXT_SOFT);
        assert_eq!(theme.tool_value_color, palette::LIGHT_TEXT_MUTED);
        assert_eq!(theme.plan_summary_color, palette::LIGHT_TEXT_MUTED);
    }

    #[test]
    fn grayscale_theme_uses_neutral_tokens() {
        let theme = Theme::for_mode(PaletteMode::Grayscale);
        assert_eq!(theme.mode, PaletteMode::Grayscale);
        assert_eq!(theme.panel_bg, palette::GRAYSCALE_PANEL);
        assert_eq!(theme.sidebar_bg, palette::GRAYSCALE_PANEL);
        assert_eq!(theme.border_color, palette::GRAYSCALE_BORDER);
        assert_eq!(theme.tool_running_accent, palette::GRAYSCALE_TEXT_SOFT);
        assert_eq!(theme.tool_failed_accent, palette::GRAYSCALE_TEXT_BODY);
        assert_eq!(theme.plan_summary_color, palette::GRAYSCALE_TEXT_MUTED);
    }

    #[test]
    fn tool_status_color_maps_each_status() {
        let theme = DARK_THEME;
        assert_eq!(
            theme.tool_status_color(ToolStatus::Running),
            theme.tool_running_accent
        );
        assert_eq!(
            theme.tool_status_color(ToolStatus::Success),
            theme.tool_success_accent
        );
        assert_eq!(
            theme.tool_status_color(ToolStatus::Failed),
            theme.tool_failed_accent
        );
    }

    #[test]
    fn ui_theme_from_settings_applies_theme_and_background() {
        let theme = ui_theme_from_settings("grayscale", Some("#111111"));
        assert_eq!(theme.mode, PaletteMode::Grayscale);
        assert_eq!(theme.surface_bg, Color::Rgb(17, 17, 17));
    }

    #[test]
    fn from_settings_with_overrides() {
        let theme = Theme::from_settings("tokyo-night", Some("rounded"), None);
        assert_eq!(theme.border_type, BorderType::Rounded);
    }

    #[test]
    fn from_settings_with_section_border_fallback() {
        let theme = Theme::from_settings("dark", Some("rounded"), None);
        assert_eq!(theme.border_type, BorderType::Rounded);
        assert_eq!(theme.section_border_type, BorderType::Rounded);

        let theme2 = Theme::from_settings("dark", Some("rounded"), Some("plain"));
        assert_eq!(theme2.border_type, BorderType::Rounded);
        assert_eq!(theme2.section_border_type, BorderType::Plain);
    }

    #[test]
    fn custom_theme_file_parses_reasoning_bg() {
        let toml_str = "base = \"tokyo-night\"\nreasoning_bg = \"#362C1A\"\n";
        let custom: crate::tui::theme::CustomThemeFile =
            toml::from_str(toml_str).expect("parse custom theme");
        assert_eq!(custom.base, "tokyo-night");
        assert_eq!(custom.reasoning_bg, Some("#362C1A".to_string()));

        let theme = Theme::from_custom_theme(&custom);
        assert_eq!(theme.reasoning_bg, Some(Color::Rgb(0x36, 0x2C, 0x1A)));
    }

    #[test]
    fn custom_theme_reasoning_bg_reset() {
        let toml_str = "base = \"dark\"\nreasoning_bg = \"reset\"\n";
        let custom: crate::tui::theme::CustomThemeFile =
            toml::from_str(toml_str).expect("parse custom theme");
        let theme = Theme::from_custom_theme(&custom);
        assert_eq!(theme.reasoning_bg, Some(Color::Reset));
    }

    #[test]
    fn border_type_from_setting_roundtrips() {
        assert_eq!(border_type_from_setting("rounded"), BorderType::Rounded);
        assert_eq!(border_type_from_setting("plain"), BorderType::Plain);
        assert_eq!(border_type_from_setting(""), BorderType::Plain);
        assert_eq!(border_type_from_setting("unknown"), BorderType::Plain);
    }

    #[test]
    fn theme_id_from_name_aliases() {
        assert_eq!(ThemeId::from_name("dark"), Some(ThemeId::Whale));
        assert_eq!(ThemeId::from_name("whale"), Some(ThemeId::Whale));
        assert_eq!(ThemeId::from_name("light"), Some(ThemeId::WhaleLight));
        assert_eq!(ThemeId::from_name("system"), Some(ThemeId::System));
        assert_eq!(
            ThemeId::from_name("catppuccin-mocha"),
            Some(ThemeId::CatppuccinMocha)
        );
        assert_eq!(ThemeId::from_name("nonexistent"), None);
    }
}
