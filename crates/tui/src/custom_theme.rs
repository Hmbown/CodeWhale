//! User-defined custom theme persistence layer.
//!
//! Custom themes live as JSON files under `~/.codewhale/themes/<name>.json`.
//! Each file is a complete `UiTheme` definition serialized with hex colour
//! strings for every field. The JSON format is human-readable and editable,
//! but the primary creation path is through the guided `/theme new` flow
//! which asks CodeWhale to generate a theme from a natural-language description.
//!
//! ## File format (example)
//!
//! ```json
//! {
//!   "name": "amber-library",
//!   "mode": "dark",
//!   "surface_bg": "#1a1410",
//!   "panel_bg": "#241e18",
//!   ...
//! }
//! ```
//!
//! ## Resilience
//!
//! - Missing colour keys in the JSON default to the Whale dark palette value,
//!   so a theme file written by an older version of CodeWhale remains valid
//!   after a new version adds fields to `UiTheme`.
//! - Invalid hex strings or unparseable JSON are rejected at load time with a
//!   warning logged to stderr; the offending file is skipped, not loaded.
//! - The themes directory is never touched by the updater or installer, so
//!   custom themes survive version upgrades without user intervention.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

use crate::palette::{PaletteMode, UI_THEME};

/// Directory name under the CodeWhale config root where custom theme
/// JSON files are stored.
const CUSTOM_THEMES_DIR_NAME: &str = "themes";

/// Default fallback values used when a JSON key is missing so that
/// older theme files remain loadable after new `UiTheme` fields are added.
fn default_surface_bg() -> String {
    color_to_hex(UI_THEME.surface_bg)
}
fn default_panel_bg() -> String {
    color_to_hex(UI_THEME.panel_bg)
}
fn default_elevated_bg() -> String {
    color_to_hex(UI_THEME.elevated_bg)
}
fn default_composer_bg() -> String {
    color_to_hex(UI_THEME.composer_bg)
}
fn default_selection_bg() -> String {
    color_to_hex(UI_THEME.selection_bg)
}
fn default_header_bg() -> String {
    color_to_hex(UI_THEME.header_bg)
}
fn default_footer_bg() -> String {
    color_to_hex(UI_THEME.footer_bg)
}
fn default_text_dim() -> String {
    color_to_hex(UI_THEME.text_dim)
}
fn default_text_hint() -> String {
    color_to_hex(UI_THEME.text_hint)
}
fn default_text_muted() -> String {
    color_to_hex(UI_THEME.text_muted)
}
fn default_text_body() -> String {
    color_to_hex(UI_THEME.text_body)
}
fn default_text_soft() -> String {
    color_to_hex(UI_THEME.text_soft)
}
fn default_border() -> String {
    color_to_hex(UI_THEME.border)
}
fn default_accent_primary() -> String {
    color_to_hex(UI_THEME.accent_primary)
}
fn default_accent_secondary() -> String {
    color_to_hex(UI_THEME.accent_secondary)
}
fn default_accent_action() -> String {
    color_to_hex(UI_THEME.accent_action)
}
fn default_error_fg() -> String {
    color_to_hex(UI_THEME.error_fg)
}
fn default_error_hover() -> String {
    color_to_hex(UI_THEME.error_hover)
}
fn default_error_surface() -> String {
    color_to_hex(UI_THEME.error_surface)
}
fn default_error_border() -> String {
    color_to_hex(UI_THEME.error_border)
}
fn default_error_text() -> String {
    color_to_hex(UI_THEME.error_text)
}
fn default_warning() -> String {
    color_to_hex(UI_THEME.warning)
}
fn default_success() -> String {
    color_to_hex(UI_THEME.success)
}
fn default_info() -> String {
    color_to_hex(UI_THEME.info)
}
fn default_mode_agent() -> String {
    color_to_hex(UI_THEME.mode_agent)
}
fn default_mode_yolo() -> String {
    color_to_hex(UI_THEME.mode_yolo)
}
fn default_mode_plan() -> String {
    color_to_hex(UI_THEME.mode_plan)
}
fn default_mode_goal() -> String {
    color_to_hex(UI_THEME.mode_goal)
}
fn default_status_ready() -> String {
    color_to_hex(UI_THEME.status_ready)
}
fn default_status_working() -> String {
    color_to_hex(UI_THEME.status_working)
}
fn default_status_warning() -> String {
    color_to_hex(UI_THEME.status_warning)
}
fn default_diff_added_fg() -> String {
    color_to_hex(UI_THEME.diff_added_fg)
}
fn default_diff_deleted_fg() -> String {
    color_to_hex(UI_THEME.diff_deleted_fg)
}
fn default_diff_added_bg() -> String {
    color_to_hex(UI_THEME.diff_added_bg)
}
fn default_diff_deleted_bg() -> String {
    color_to_hex(UI_THEME.diff_deleted_bg)
}
fn default_tool_running() -> String {
    color_to_hex(UI_THEME.tool_running)
}
fn default_tool_success() -> String {
    color_to_hex(UI_THEME.tool_success)
}
fn default_tool_failed() -> String {
    color_to_hex(UI_THEME.tool_failed)
}

/// Serializable representation of a complete custom theme. Every colour
/// is a `"#RRGGBB"` hex string. `mode` is one of `"dark"`, `"light"`,
/// `"grayscale"`, or `"solarized-light"`.
///
/// All colour fields have a serde default of the Whale dark palette value
/// so that a partial JSON file (written by an older CodeWhale version)
/// still deserializes into a usable theme.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiThemeCustom {
    pub name: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    // Surface hierarchy
    #[serde(default = "default_surface_bg")]
    pub surface_bg: String,
    #[serde(default = "default_panel_bg")]
    pub panel_bg: String,
    #[serde(default = "default_elevated_bg")]
    pub elevated_bg: String,
    #[serde(default = "default_composer_bg")]
    pub composer_bg: String,
    #[serde(default = "default_selection_bg")]
    pub selection_bg: String,
    #[serde(default = "default_header_bg")]
    pub header_bg: String,
    #[serde(default = "default_footer_bg")]
    pub footer_bg: String,
    // Text hierarchy
    #[serde(default = "default_text_dim")]
    pub text_dim: String,
    #[serde(default = "default_text_hint")]
    pub text_hint: String,
    #[serde(default = "default_text_muted")]
    pub text_muted: String,
    #[serde(default = "default_text_body")]
    pub text_body: String,
    #[serde(default = "default_text_soft")]
    pub text_soft: String,
    #[serde(default = "default_border")]
    pub border: String,
    // Accent roles
    #[serde(default = "default_accent_primary")]
    pub accent_primary: String,
    #[serde(default = "default_accent_secondary")]
    pub accent_secondary: String,
    #[serde(default = "default_accent_action")]
    pub accent_action: String,
    // Error / destructive
    #[serde(default = "default_error_fg")]
    pub error_fg: String,
    #[serde(default = "default_error_hover")]
    pub error_hover: String,
    #[serde(default = "default_error_surface")]
    pub error_surface: String,
    #[serde(default = "default_error_border")]
    pub error_border: String,
    #[serde(default = "default_error_text")]
    pub error_text: String,
    // Status roles
    #[serde(default = "default_warning")]
    pub warning: String,
    #[serde(default = "default_success")]
    pub success: String,
    #[serde(default = "default_info")]
    pub info: String,
    // Mode badge colors
    #[serde(default = "default_mode_agent")]
    pub mode_agent: String,
    #[serde(default = "default_mode_yolo")]
    pub mode_yolo: String,
    #[serde(default = "default_mode_plan")]
    pub mode_plan: String,
    #[serde(default = "default_mode_goal")]
    pub mode_goal: String,
    // Footer statusline
    #[serde(default = "default_status_ready")]
    pub status_ready: String,
    #[serde(default = "default_status_working")]
    pub status_working: String,
    #[serde(default = "default_status_warning")]
    pub status_warning: String,
    // Diff colors
    #[serde(default = "default_diff_added_fg")]
    pub diff_added_fg: String,
    #[serde(default = "default_diff_deleted_fg")]
    pub diff_deleted_fg: String,
    #[serde(default = "default_diff_added_bg")]
    pub diff_added_bg: String,
    #[serde(default = "default_diff_deleted_bg")]
    pub diff_deleted_bg: String,
    // Tool cell colors
    #[serde(default = "default_tool_running")]
    pub tool_running: String,
    #[serde(default = "default_tool_success")]
    pub tool_success: String,
    #[serde(default = "default_tool_failed")]
    pub tool_failed: String,
}

fn default_mode() -> String {
    "dark".to_string()
}

// ---- Conversions -------------------------------------------------------

/// Convert a `ratatui::style::Color` to a `"#RRGGBB"` hex string.
/// `Color::Reset` and named ANSI colours are mapped to a neutral dark grey
/// (`"#222222"`) since custom themes use RGB exclusively.
pub fn color_to_hex(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => "#222222".to_string(),
    }
}

/// Parse a `"#RRGGBB"` or `"RRGGBB"` hex string into a `Color::Rgb`.
/// Returns `None` on malformed input.
pub fn hex_to_color(hex: &str) -> Option<Color> {
    let hex = hex.trim().strip_prefix('#').unwrap_or(hex.trim());
    if hex.len() != 6 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

/// Parse a mode string into `PaletteMode`. Unrecognised strings fall back
/// to `Dark`.
pub fn parse_mode(s: &str) -> PaletteMode {
    match s.trim().to_ascii_lowercase().as_str() {
        "light" | "whale-light" => PaletteMode::Light,
        "grayscale" | "greyscale" | "gray" | "grey" | "mono" => PaletteMode::Grayscale,
        "solarized-light" | "solarized" => PaletteMode::SolarizedLight,
        _ => PaletteMode::Dark,
    }
}

pub fn mode_to_string(mode: PaletteMode) -> &'static str {
    match mode {
        PaletteMode::Dark => "dark",
        PaletteMode::Light => "light",
        PaletteMode::Grayscale => "grayscale",
        PaletteMode::SolarizedLight => "solarized-light",
    }
}

impl UiThemeCustom {
    /// Build a `UiThemeCustom` by extracting every field from a `UiTheme`
    /// into its hex-string representation. The `name` field is set from the
    /// provided display name.
    #[allow(dead_code)]
    pub fn from_ui_theme(ui_theme: &crate::palette::UiTheme, name: String) -> Self {
        Self {
            name,
            mode: mode_to_string(ui_theme.mode).to_string(),
            surface_bg: color_to_hex(ui_theme.surface_bg),
            panel_bg: color_to_hex(ui_theme.panel_bg),
            elevated_bg: color_to_hex(ui_theme.elevated_bg),
            composer_bg: color_to_hex(ui_theme.composer_bg),
            selection_bg: color_to_hex(ui_theme.selection_bg),
            header_bg: color_to_hex(ui_theme.header_bg),
            footer_bg: color_to_hex(ui_theme.footer_bg),
            text_dim: color_to_hex(ui_theme.text_dim),
            text_hint: color_to_hex(ui_theme.text_hint),
            text_muted: color_to_hex(ui_theme.text_muted),
            text_body: color_to_hex(ui_theme.text_body),
            text_soft: color_to_hex(ui_theme.text_soft),
            border: color_to_hex(ui_theme.border),
            accent_primary: color_to_hex(ui_theme.accent_primary),
            accent_secondary: color_to_hex(ui_theme.accent_secondary),
            accent_action: color_to_hex(ui_theme.accent_action),
            error_fg: color_to_hex(ui_theme.error_fg),
            error_hover: color_to_hex(ui_theme.error_hover),
            error_surface: color_to_hex(ui_theme.error_surface),
            error_border: color_to_hex(ui_theme.error_border),
            error_text: color_to_hex(ui_theme.error_text),
            warning: color_to_hex(ui_theme.warning),
            success: color_to_hex(ui_theme.success),
            info: color_to_hex(ui_theme.info),
            mode_agent: color_to_hex(ui_theme.mode_agent),
            mode_yolo: color_to_hex(ui_theme.mode_yolo),
            mode_plan: color_to_hex(ui_theme.mode_plan),
            mode_goal: color_to_hex(ui_theme.mode_goal),
            status_ready: color_to_hex(ui_theme.status_ready),
            status_working: color_to_hex(ui_theme.status_working),
            status_warning: color_to_hex(ui_theme.status_warning),
            diff_added_fg: color_to_hex(ui_theme.diff_added_fg),
            diff_deleted_fg: color_to_hex(ui_theme.diff_deleted_fg),
            diff_added_bg: color_to_hex(ui_theme.diff_added_bg),
            diff_deleted_bg: color_to_hex(ui_theme.diff_deleted_bg),
            tool_running: color_to_hex(ui_theme.tool_running),
            tool_success: color_to_hex(ui_theme.tool_success),
            tool_failed: color_to_hex(ui_theme.tool_failed),
        }
    }

    /// Convert to an internal `UiTheme`. Colour strings that fail to parse
    /// fall back to the Whale dark palette value so a single malformed hex
    /// key does not break the whole theme.
    pub fn to_ui_theme(&self) -> crate::palette::UiTheme {
        crate::palette::UiTheme {
            name: Box::leak(self.name.clone().into_boxed_str()),
            mode: parse_mode(&self.mode),
            surface_bg: hex_to_color(&self.surface_bg).unwrap_or(UI_THEME.surface_bg),
            panel_bg: hex_to_color(&self.panel_bg).unwrap_or(UI_THEME.panel_bg),
            elevated_bg: hex_to_color(&self.elevated_bg).unwrap_or(UI_THEME.elevated_bg),
            composer_bg: hex_to_color(&self.composer_bg).unwrap_or(UI_THEME.composer_bg),
            selection_bg: hex_to_color(&self.selection_bg).unwrap_or(UI_THEME.selection_bg),
            header_bg: hex_to_color(&self.header_bg).unwrap_or(UI_THEME.header_bg),
            footer_bg: hex_to_color(&self.footer_bg).unwrap_or(UI_THEME.footer_bg),
            text_dim: hex_to_color(&self.text_dim).unwrap_or(UI_THEME.text_dim),
            text_hint: hex_to_color(&self.text_hint).unwrap_or(UI_THEME.text_hint),
            text_muted: hex_to_color(&self.text_muted).unwrap_or(UI_THEME.text_muted),
            text_body: hex_to_color(&self.text_body).unwrap_or(UI_THEME.text_body),
            text_soft: hex_to_color(&self.text_soft).unwrap_or(UI_THEME.text_soft),
            border: hex_to_color(&self.border).unwrap_or(UI_THEME.border),
            accent_primary: hex_to_color(&self.accent_primary).unwrap_or(UI_THEME.accent_primary),
            accent_secondary: hex_to_color(&self.accent_secondary)
                .unwrap_or(UI_THEME.accent_secondary),
            accent_action: hex_to_color(&self.accent_action).unwrap_or(UI_THEME.accent_action),
            error_fg: hex_to_color(&self.error_fg).unwrap_or(UI_THEME.error_fg),
            error_hover: hex_to_color(&self.error_hover).unwrap_or(UI_THEME.error_hover),
            error_surface: hex_to_color(&self.error_surface).unwrap_or(UI_THEME.error_surface),
            error_border: hex_to_color(&self.error_border).unwrap_or(UI_THEME.error_border),
            error_text: hex_to_color(&self.error_text).unwrap_or(UI_THEME.error_text),
            warning: hex_to_color(&self.warning).unwrap_or(UI_THEME.warning),
            success: hex_to_color(&self.success).unwrap_or(UI_THEME.success),
            info: hex_to_color(&self.info).unwrap_or(UI_THEME.info),
            mode_agent: hex_to_color(&self.mode_agent).unwrap_or(UI_THEME.mode_agent),
            mode_yolo: hex_to_color(&self.mode_yolo).unwrap_or(UI_THEME.mode_yolo),
            mode_plan: hex_to_color(&self.mode_plan).unwrap_or(UI_THEME.mode_plan),
            mode_goal: hex_to_color(&self.mode_goal).unwrap_or(UI_THEME.mode_goal),
            status_ready: hex_to_color(&self.status_ready).unwrap_or(UI_THEME.status_ready),
            status_working: hex_to_color(&self.status_working).unwrap_or(UI_THEME.status_working),
            status_warning: hex_to_color(&self.status_warning).unwrap_or(UI_THEME.status_warning),
            diff_added_fg: hex_to_color(&self.diff_added_fg).unwrap_or(UI_THEME.diff_added_fg),
            diff_deleted_fg: hex_to_color(&self.diff_deleted_fg)
                .unwrap_or(UI_THEME.diff_deleted_fg),
            diff_added_bg: hex_to_color(&self.diff_added_bg).unwrap_or(UI_THEME.diff_added_bg),
            diff_deleted_bg: hex_to_color(&self.diff_deleted_bg)
                .unwrap_or(UI_THEME.diff_deleted_bg),
            tool_running: hex_to_color(&self.tool_running).unwrap_or(UI_THEME.tool_running),
            tool_success: hex_to_color(&self.tool_success).unwrap_or(UI_THEME.tool_success),
            tool_failed: hex_to_color(&self.tool_failed).unwrap_or(UI_THEME.tool_failed),
        }
    }
}

// ---- File I/O ----------------------------------------------------------

/// Resolve the `~/.codewhale/themes/` directory path. Creates it if absent.
pub fn ensure_themes_dir() -> std::io::Result<PathBuf> {
    let home = dirs_next().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "home directory not found")
    })?;
    let dir = home.join(".codewhale").join(CUSTOM_THEMES_DIR_NAME);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Resolve the directory path without creating it (used during app init
/// to check whether any custom themes exist without creating the dir).
pub fn themes_dir() -> Option<PathBuf> {
    let home = dirs_next()?;
    let dir = home.join(".codewhale").join(CUSTOM_THEMES_DIR_NAME);
    if dir.is_dir() { Some(dir) } else { None }
}

fn dirs_next() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| {
            std::env::var("USERPROFILE").or_else(|_| {
                std::env::var("HOMEDRIVE")
                    .and_then(|hd| std::env::var("HOMEPATH").map(|hp| format!("{hd}{hp}")))
            })
        })
        .ok()
        .map(PathBuf::from)
}

/// Scan `~/.codewhale/themes/` for `*.json` files and return a map of
/// theme-name → `UiTheme`. Files that fail to parse are skipped with a
/// warning printed to stderr.
pub fn load_custom_themes() -> HashMap<String, crate::palette::UiTheme> {
    let mut themes = HashMap::new();
    let dir = match themes_dir() {
        Some(d) => d,
        None => return themes,
    };
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return themes,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        match fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<UiThemeCustom>(&content) {
                Ok(ct) => {
                    let ui_theme = ct.to_ui_theme();
                    themes.insert(stem.to_string(), ui_theme);
                }
                Err(e) => {
                    eprintln!(
                        "[custom_theme] skipping {}: invalid JSON ({e})",
                        path.display()
                    );
                }
            },
            Err(e) => {
                eprintln!(
                    "[custom_theme] skipping {}: cannot read ({e})",
                    path.display()
                );
            }
        }
    }
    themes
}

/// Persist a custom theme to `~/.codewhale/themes/<name>.json`. Uses
/// atomic write (temp file + rename) to prevent corruption on crash.
#[allow(dead_code)]
pub fn save_custom_theme(name: &str, custom: &UiThemeCustom) -> std::io::Result<PathBuf> {
    let dir = ensure_themes_dir()?;
    let path = dir.join(format!("{name}.json"));
    let tmp = dir.join(format!(".{name}.json.tmp"));
    let json = serde_json::to_string_pretty(custom)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Delete a custom theme file from disk.
pub fn delete_custom_theme(name: &str) -> std::io::Result<()> {
    let dir = ensure_themes_dir()?;
    let path = dir.join(format!("{name}.json"));
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

/// List the base file names (without `.json` extension) of all custom
/// themes on disk.
#[allow(dead_code)]
pub fn list_custom_theme_names() -> Vec<String> {
    let dir = match themes_dir() {
        Some(d) => d,
        None => return Vec::new(),
    };
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_to_color_round_trips() {
        let cases = [
            ("#0A1120", "#0a1120"),
            ("#F6F2E8", "#f6f2e8"),
            ("#aabbcc", "#aabbcc"),
            ("AABBCC", "#aabbcc"),
            ("#000000", "#000000"),
            ("#FFFFFF", "#ffffff"),
        ];
        for (input, expected) in cases {
            let color = hex_to_color(input).expect("valid hex");
            let back = color_to_hex(color);
            assert_eq!(back.to_ascii_lowercase(), expected);
        }
    }

    #[test]
    fn hex_to_color_rejects_bad_input() {
        assert!(hex_to_color("").is_none());
        assert!(hex_to_color("#ZZZZZZ").is_none());
        assert!(hex_to_color("#12345").is_none());
        assert!(hex_to_color("#1234567").is_none());
        assert!(hex_to_color("not-a-colour").is_none());
    }

    #[test]
    fn parse_mode_recognises_all_variants() {
        assert_eq!(parse_mode("dark"), PaletteMode::Dark);
        assert_eq!(parse_mode("light"), PaletteMode::Light);
        assert_eq!(parse_mode("grayscale"), PaletteMode::Grayscale);
        assert_eq!(parse_mode("grey"), PaletteMode::Grayscale);
        assert_eq!(parse_mode("solarized-light"), PaletteMode::SolarizedLight);
        assert_eq!(parse_mode("solarized"), PaletteMode::SolarizedLight);
    }

    #[test]
    fn parse_mode_unknown_falls_back_to_dark() {
        assert_eq!(parse_mode("mystery"), PaletteMode::Dark);
        assert_eq!(parse_mode(""), PaletteMode::Dark);
    }

    #[test]
    fn ui_theme_custom_round_trips() {
        use crate::palette::DRACULA_UI_THEME;
        let custom = UiThemeCustom::from_ui_theme(&DRACULA_UI_THEME, "test-dracula".to_string());
        let round_tripped = custom.to_ui_theme();
        // The name::&'static str is leaked so it won't match == exactly;
        // check the mode and a few key colours instead.
        assert_eq!(round_tripped.mode, DRACULA_UI_THEME.mode);
        assert_eq!(round_tripped.surface_bg, DRACULA_UI_THEME.surface_bg);
        assert_eq!(round_tripped.text_body, DRACULA_UI_THEME.text_body);
        assert_eq!(
            round_tripped.accent_primary,
            DRACULA_UI_THEME.accent_primary
        );
    }

    #[test]
    fn partial_json_fills_defaults() {
        let json = r##"{"name": "minimal", "surface_bg": "#ff0000"}"##;
        let ct: UiThemeCustom = serde_json::from_str(json).unwrap();
        assert_eq!(ct.name, "minimal");
        assert_eq!(ct.surface_bg, "#ff0000");
        // Unspecified fields get Whale dark defaults
        assert_eq!(ct.text_body, color_to_hex(UI_THEME.text_body));
        assert_eq!(ct.accent_primary, color_to_hex(UI_THEME.accent_primary));
    }
}
