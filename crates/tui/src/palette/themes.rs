//! Named theme presets and theme resolution.

use ratatui::style::Color;

use super::detect::PaletteMode;
use super::tokens::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiTheme {
    pub name: &'static str,
    pub mode: PaletteMode,
    // Surface hierarchy
    pub surface_bg: Color,
    pub panel_bg: Color,
    pub elevated_bg: Color,
    pub composer_bg: Color,
    pub selection_bg: Color,
    pub header_bg: Color,
    pub footer_bg: Color,
    /// Text hierarchy
    pub text_dim: Color,
    pub text_hint: Color,
    pub text_muted: Color,
    pub text_body: Color,
    pub text_soft: Color,
    pub border: Color,
    // Accent roles
    pub accent_primary: Color,
    pub accent_secondary: Color,
    pub accent_action: Color,
    // Error / destructive
    pub error_fg: Color,
    pub error_hover: Color,
    pub error_surface: Color,
    pub error_border: Color,
    pub error_text: Color,
    // Status roles (warning / success / info)
    pub warning: Color,
    pub success: Color,
    pub info: Color,
    // Mode badge colors (act/plan/operate; mode_yolo kept for legacy theme data).
    // These must be distinct from one another and from the general accent,
    // human, warning, danger, and success lanes: the terminal backend receives
    // only the final `Color` and needs that identity to perform semantic ANSI
    // adaptation.
    pub mode_agent: Color,
    pub mode_yolo: Color,
    pub mode_plan: Color,
    pub mode_operate: Color,
    // Footer statusline colors
    pub status_ready: Color,
    pub status_working: Color,
    pub status_warning: Color,
    // Diff colors
    pub diff_added_fg: Color,
    pub diff_deleted_fg: Color,
    pub diff_added_bg: Color,
    pub diff_deleted_bg: Color,
    // Tool cell colors
    pub tool_running: Color,
    pub tool_success: Color,
    pub tool_failed: Color,
}

pub const UI_THEME: UiTheme = UiTheme {
    name: "whale",
    mode: PaletteMode::Dark,
    surface_bg: WHALE_BG,
    panel_bg: WHALE_PANEL,
    elevated_bg: SURFACE_ELEVATED,
    composer_bg: WHALE_COMPOSER,
    selection_bg: SELECTION_BG,
    header_bg: WHALE_CHROME,
    footer_bg: WHALE_CHROME,
    text_dim: TEXT_DIM,
    text_hint: TEXT_HINT,
    text_muted: TEXT_MUTED,
    text_body: TEXT_BODY,
    text_soft: TEXT_SOFT,
    border: BORDER_COLOR,
    accent_primary: WHALE_ACTION,
    accent_secondary: WHALE_LIVE,
    accent_action: WHALE_HUMAN,
    error_fg: WHALE_ERROR,
    error_hover: Color::Rgb(
        WHALE_ERROR_HOVER_RGB.0,
        WHALE_ERROR_HOVER_RGB.1,
        WHALE_ERROR_HOVER_RGB.2,
    ),
    error_surface: Color::Rgb(
        WHALE_ERROR_SURFACE_RGB.0,
        WHALE_ERROR_SURFACE_RGB.1,
        WHALE_ERROR_SURFACE_RGB.2,
    ),
    error_border: Color::Rgb(
        WHALE_ERROR_BORDER_RGB.0,
        WHALE_ERROR_BORDER_RGB.1,
        WHALE_ERROR_BORDER_RGB.2,
    ),
    error_text: Color::Rgb(
        WHALE_ERROR_TEXT_RGB.0,
        WHALE_ERROR_TEXT_RGB.1,
        WHALE_ERROR_TEXT_RGB.2,
    ),
    warning: STATUS_WARNING,
    success: Color::Rgb(
        WHALE_SUCCESS_RGB.0,
        WHALE_SUCCESS_RGB.1,
        WHALE_SUCCESS_RGB.2,
    ),
    info: WHALE_ACTION,
    mode_agent: MODE_AGENT,
    mode_yolo: MODE_YOLO,
    mode_plan: MODE_PLAN,
    mode_operate: MODE_OPERATE,
    status_ready: TEXT_MUTED,
    status_working: WHALE_LIVE,
    status_warning: STATUS_WARNING,
    diff_added_fg: DIFF_ADDED,
    diff_deleted_fg: WHALE_ERROR,
    diff_added_bg: DIFF_ADDED_BG,
    diff_deleted_bg: DIFF_DELETED_BG,
    tool_running: WHALE_LIVE,
    tool_success: Color::Rgb(
        WHALE_WORKING_GREEN_RGB.0,
        WHALE_WORKING_GREEN_RGB.1,
        WHALE_WORKING_GREEN_RGB.2,
    ),
    tool_failed: WHALE_ERROR,
};

pub const LIGHT_UI_THEME: UiTheme = UiTheme {
    name: "whale-light",
    mode: PaletteMode::Light,
    surface_bg: LIGHT_SURFACE,
    panel_bg: LIGHT_PANEL,
    elevated_bg: LIGHT_ELEVATED,
    composer_bg: LIGHT_PANEL,
    selection_bg: LIGHT_SELECTION_BG,
    header_bg: LIGHT_SURFACE,
    footer_bg: LIGHT_SURFACE,
    text_dim: LIGHT_TEXT_HINT,
    text_hint: LIGHT_TEXT_HINT,
    text_muted: LIGHT_TEXT_MUTED,
    text_body: LIGHT_TEXT_BODY,
    text_soft: LIGHT_TEXT_SOFT,
    border: LIGHT_BORDER,
    accent_primary: LIGHT_ACTION,
    accent_secondary: LIGHT_LIVE,
    accent_action: LIGHT_HUMAN,
    error_fg: LIGHT_DANGER,
    error_hover: Color::Rgb(201, 71, 120), // #C94778
    error_surface: LIGHT_ERROR,
    error_border: LIGHT_DANGER,
    error_text: Color::Rgb(109, 22, 56), // #6D1638
    warning: LIGHT_WARNING,
    success: LIGHT_SUCCESS_FG,
    info: LIGHT_ACTION,
    mode_agent: LIGHT_MODE_AGENT,
    mode_yolo: LIGHT_MODE_YOLO,
    mode_plan: LIGHT_MODE_PLAN,
    mode_operate: LIGHT_OPERATE,
    status_ready: LIGHT_TEXT_MUTED,
    status_working: LIGHT_LIVE,
    status_warning: LIGHT_WARNING,
    diff_added_fg: Color::Rgb(22, 101, 52), // green
    diff_deleted_fg: LIGHT_DANGER,
    diff_added_bg: Color::Rgb(223, 247, 231), // light green
    diff_deleted_bg: LIGHT_ERROR,
    tool_running: LIGHT_LIVE,
    tool_success: LIGHT_SUCCESS_FG,
    tool_failed: LIGHT_DANGER,
};

pub const SOLARIZED_LIGHT_UI_THEME: UiTheme = UiTheme {
    name: "solarized-light",
    mode: PaletteMode::SolarizedLight,
    surface_bg: SOLARIZED_SURFACE,
    panel_bg: SOLARIZED_PANEL,
    elevated_bg: SOLARIZED_ELEVATED,
    composer_bg: SOLARIZED_COMPOSER,
    selection_bg: SOLARIZED_SELECT_BG,
    header_bg: SOLARIZED_SURFACE,
    footer_bg: SOLARIZED_SURFACE,
    text_dim: SOLARIZED_TEXT_DIM,
    text_hint: SOLARIZED_TEXT_HINT,
    text_muted: SOLARIZED_TEXT_MUTED,
    text_body: SOLARIZED_TEXT_BODY,
    text_soft: SOLARIZED_TEXT_SOFT,
    border: SOLARIZED_BORDER,
    accent_primary: SOLARIZED_BLUE,
    accent_secondary: SOLARIZED_CYAN,
    accent_action: SOLARIZED_ORANGE,
    error_fg: SOLARIZED_RED,
    error_hover: SOLARIZED_ERROR_HOVER,
    error_surface: SOLARIZED_ERROR_SURFACE,
    error_border: SOLARIZED_RED,
    error_text: SOLARIZED_ERROR_TEXT,
    warning: SOLARIZED_YELLOW,
    success: SOLARIZED_GREEN,
    info: SOLARIZED_BLUE,
    mode_agent: Color::Rgb(0x27, 0x8B, 0xD2),
    mode_yolo: Color::Rgb(0xDD, 0x32, 0x2F),
    mode_plan: Color::Rgb(0xCC, 0x4B, 0x16),
    mode_operate: Color::Rgb(0x6C, 0x71, 0xC4), // solarized violet
    status_ready: SOLARIZED_CYAN,
    status_working: SOLARIZED_CYAN,
    status_warning: SOLARIZED_YELLOW,
    diff_added_fg: SOLARIZED_GREEN,
    diff_deleted_fg: SOLARIZED_RED,
    diff_added_bg: SOLARIZED_DIFF_ADDED_BG,
    diff_deleted_bg: SOLARIZED_DIFF_DELETED_BG,
    tool_running: SOLARIZED_BLUE,
    tool_success: SOLARIZED_GREEN,
    tool_failed: SOLARIZED_RED,
};

pub const GRAYSCALE_UI_THEME: UiTheme = UiTheme {
    name: "grayscale",
    mode: PaletteMode::Grayscale,
    surface_bg: GRAYSCALE_SURFACE,
    panel_bg: GRAYSCALE_PANEL,
    elevated_bg: GRAYSCALE_ELEVATED,
    composer_bg: GRAYSCALE_PANEL,
    selection_bg: GRAYSCALE_SELECTION_BG,
    header_bg: GRAYSCALE_SURFACE,
    footer_bg: GRAYSCALE_SURFACE,
    text_dim: GRAYSCALE_TEXT_HINT,
    text_hint: GRAYSCALE_TEXT_HINT,
    text_muted: GRAYSCALE_TEXT_MUTED,
    text_body: GRAYSCALE_TEXT_BODY,
    text_soft: GRAYSCALE_TEXT_SOFT,
    border: GRAYSCALE_BORDER,
    accent_primary: GRAYSCALE_TEXT_SOFT,
    accent_secondary: GRAYSCALE_TEXT_MUTED,
    accent_action: Color::Rgb(210, 210, 210),
    error_fg: GRAYSCALE_TEXT_BODY,
    error_hover: GRAYSCALE_TEXT_SOFT,
    error_surface: GRAYSCALE_ERROR,
    error_border: GRAYSCALE_BORDER,
    error_text: GRAYSCALE_TEXT_SOFT,
    warning: GRAYSCALE_TEXT_MUTED,
    success: GRAYSCALE_TEXT_SOFT,
    info: GRAYSCALE_TEXT_MUTED,
    mode_agent: Color::Rgb(200, 200, 200),
    mode_yolo: Color::Rgb(237, 237, 237),
    mode_plan: Color::Rgb(181, 181, 181),
    // Near-white stays unmistakably distinct without claiming arbitrary pure
    // white content as an already-resolved mode slot.
    mode_operate: Color::Rgb(250, 250, 250),
    status_ready: GRAYSCALE_TEXT_MUTED,
    status_working: GRAYSCALE_TEXT_SOFT,
    status_warning: GRAYSCALE_TEXT_BODY,
    diff_added_fg: GRAYSCALE_TEXT_SOFT,
    diff_deleted_fg: GRAYSCALE_TEXT_BODY,
    diff_added_bg: GRAYSCALE_SUCCESS,
    diff_deleted_bg: GRAYSCALE_ERROR,
    tool_running: GRAYSCALE_TEXT_SOFT,
    tool_success: GRAYSCALE_TEXT_HINT,
    tool_failed: GRAYSCALE_TEXT_BODY,
};

pub const CATPPUCCIN_MOCHA_UI_THEME: UiTheme = UiTheme {
    name: "catppuccin-mocha",
    mode: PaletteMode::Dark,
    surface_bg: Color::Rgb(0x1e, 0x1e, 0x2e),  // base
    panel_bg: Color::Rgb(0x18, 0x18, 0x25),    // mantle
    elevated_bg: Color::Rgb(0x31, 0x32, 0x44), // surface0
    composer_bg: Color::Rgb(0x18, 0x18, 0x25),
    selection_bg: Color::Rgb(0x45, 0x47, 0x5a), // surface1
    header_bg: Color::Rgb(0x11, 0x11, 0x1b),    // crust
    footer_bg: Color::Rgb(0x11, 0x11, 0x1b),
    text_dim: Color::Rgb(0x6c, 0x70, 0x86),         // overlay0
    text_hint: Color::Rgb(0x7f, 0x84, 0x9c),        // overlay1
    text_muted: Color::Rgb(0xa6, 0xad, 0xc8),       // subtext0
    text_body: Color::Rgb(0xcd, 0xd6, 0xf4),        // text
    text_soft: Color::Rgb(0xba, 0xc2, 0xde),        // subtext1
    border: Color::Rgb(0x45, 0x47, 0x5a),           // surface1
    accent_primary: Color::Rgb(0x89, 0xb4, 0xfa),   // blue
    accent_secondary: Color::Rgb(0x74, 0xc7, 0xec), // sapphire
    accent_action: Color::Rgb(0xfa, 0xb3, 0x87),    // peach
    error_fg: Color::Rgb(0xf3, 0x8b, 0xa8),         // red
    error_hover: Color::Rgb(0xf5, 0xa2, 0xbc),
    error_surface: Color::Rgb(0x3a, 0x1f, 0x2a),
    error_border: Color::Rgb(0xf3, 0x8b, 0xa8),
    error_text: Color::Rgb(0xf5, 0xc2, 0xd0),
    warning: Color::Rgb(0xf9, 0xe2, 0xaf),         // yellow
    success: Color::Rgb(0xa6, 0xe3, 0xa1),         // green
    info: Color::Rgb(0x89, 0xd9, 0xeb),            // sky
    mode_agent: Color::Rgb(0x8a, 0xb4, 0xfa),      // blue
    mode_yolo: Color::Rgb(0xf3, 0x8c, 0xa8),       // red
    mode_plan: Color::Rgb(0xfa, 0xb4, 0x87),       // peach
    mode_operate: Color::Rgb(0xcb, 0xa6, 0xf7),    // mauve
    status_ready: Color::Rgb(0x7f, 0x84, 0x9c),    // overlay1
    status_working: Color::Rgb(0x74, 0xc7, 0xec),  // sapphire
    status_warning: Color::Rgb(0xf9, 0xe2, 0xaf),  // yellow
    diff_added_fg: Color::Rgb(0xa6, 0xe3, 0xa1),   // green
    diff_deleted_fg: Color::Rgb(0xf3, 0x8b, 0xa8), // red
    diff_added_bg: Color::Rgb(0x1f, 0x33, 0x29),
    diff_deleted_bg: Color::Rgb(0x3a, 0x1f, 0x2a),
    tool_running: Color::Rgb(0x74, 0xc7, 0xec), // sapphire
    tool_success: Color::Rgb(0x7f, 0x84, 0x9c), // overlay1
    tool_failed: Color::Rgb(0xf3, 0x8b, 0xa8),  // red
};

pub const TOKYO_NIGHT_UI_THEME: UiTheme = UiTheme {
    name: "tokyo-night",
    mode: PaletteMode::Dark,
    surface_bg: Color::Rgb(0x1a, 0x1b, 0x26),  // bg
    panel_bg: Color::Rgb(0x16, 0x16, 0x1e),    // bg_dark
    elevated_bg: Color::Rgb(0x29, 0x2e, 0x42), // bg_highlight
    composer_bg: Color::Rgb(0x16, 0x16, 0x1e),
    selection_bg: Color::Rgb(0x28, 0x34, 0x57), // visual selection
    header_bg: Color::Rgb(0x16, 0x16, 0x1e),
    footer_bg: Color::Rgb(0x16, 0x16, 0x1e),
    text_dim: Color::Rgb(0x56, 0x5f, 0x89),   // comment
    text_hint: Color::Rgb(0x73, 0x7a, 0xa2),  // dark5
    text_muted: Color::Rgb(0xa9, 0xb1, 0xd6), // fg_dark
    text_body: Color::Rgb(0xc0, 0xca, 0xf5),  // fg
    text_soft: Color::Rgb(0xbb, 0xc2, 0xe0),
    border: Color::Rgb(0x41, 0x48, 0x68), // terminal_black
    accent_primary: Color::Rgb(0x7a, 0xa2, 0xf7), // blue
    accent_secondary: Color::Rgb(0x7d, 0xcf, 0xff), // cyan
    accent_action: Color::Rgb(0xff, 0x9e, 0x64), // orange
    error_fg: Color::Rgb(0xf7, 0x76, 0x8e), // red
    error_hover: Color::Rgb(0xf9, 0x92, 0xa4),
    error_surface: Color::Rgb(0x33, 0x1c, 0x24),
    error_border: Color::Rgb(0xf7, 0x76, 0x8e),
    error_text: Color::Rgb(0xfa, 0xcc, 0xd4),
    warning: Color::Rgb(0xe0, 0xaf, 0x68),         // yellow
    success: Color::Rgb(0x9e, 0xce, 0x6a),         // green
    info: Color::Rgb(0x7d, 0xcf, 0xff),            // cyan
    mode_agent: Color::Rgb(0x7b, 0xa2, 0xf7),      // blue
    mode_yolo: Color::Rgb(0xf7, 0x77, 0x8e),       // red
    mode_plan: Color::Rgb(0xff, 0x9f, 0x64),       // orange
    mode_operate: Color::Rgb(0xbb, 0x9a, 0xf7),    // purple
    status_ready: Color::Rgb(0x56, 0x5f, 0x89),    // comment
    status_working: Color::Rgb(0x7d, 0xcf, 0xff),  // cyan
    status_warning: Color::Rgb(0xe0, 0xaf, 0x68),  // yellow
    diff_added_fg: Color::Rgb(0x9e, 0xce, 0x6a),   // green
    diff_deleted_fg: Color::Rgb(0xf7, 0x76, 0x8e), // red
    diff_added_bg: Color::Rgb(0x1b, 0x2b, 0x1f),
    diff_deleted_bg: Color::Rgb(0x33, 0x1c, 0x24),
    tool_running: Color::Rgb(0x7d, 0xcf, 0xff), // cyan
    tool_success: Color::Rgb(0x56, 0x5f, 0x89), // comment
    tool_failed: Color::Rgb(0xf7, 0x76, 0x8e),  // red
};

pub const DRACULA_UI_THEME: UiTheme = UiTheme {
    name: "dracula",
    mode: PaletteMode::Dark,
    surface_bg: Color::Rgb(0x28, 0x2a, 0x36), // background
    panel_bg: Color::Rgb(0x21, 0x22, 0x2c),
    elevated_bg: Color::Rgb(0x34, 0x37, 0x46),
    composer_bg: Color::Rgb(0x21, 0x22, 0x2c),
    selection_bg: Color::Rgb(0x44, 0x47, 0x5a), // current line
    header_bg: Color::Rgb(0x21, 0x22, 0x2c),
    footer_bg: Color::Rgb(0x21, 0x22, 0x2c),
    text_dim: Color::Rgb(0x62, 0x72, 0xa4), // comment
    text_hint: Color::Rgb(0x8a, 0x8e, 0xaa),
    text_muted: Color::Rgb(0xc0, 0xc4, 0xd6),
    text_body: Color::Rgb(0xf8, 0xf8, 0xf2), // foreground
    text_soft: Color::Rgb(0xe2, 0xe2, 0xdc),
    border: Color::Rgb(0x44, 0x47, 0x5a),
    accent_primary: Color::Rgb(0xbd, 0x93, 0xf9), // purple
    accent_secondary: Color::Rgb(0x8b, 0xe9, 0xfd), // cyan
    accent_action: Color::Rgb(0xff, 0xb8, 0x6c),  // orange
    error_fg: Color::Rgb(0xff, 0x55, 0x55),       // red
    error_hover: Color::Rgb(0xff, 0x7c, 0x7c),
    error_surface: Color::Rgb(0x3a, 0x1f, 0x22),
    error_border: Color::Rgb(0xff, 0x55, 0x55),
    error_text: Color::Rgb(0xff, 0xbb, 0xbb),
    warning: Color::Rgb(0xf1, 0xfa, 0x8c),         // yellow
    success: Color::Rgb(0x50, 0xfa, 0x7b),         // green
    info: Color::Rgb(0x8b, 0xe9, 0xfd),            // cyan
    mode_agent: Color::Rgb(0xbe, 0x93, 0xf9),      // purple
    mode_yolo: Color::Rgb(0xff, 0x56, 0x55),       // red
    mode_plan: Color::Rgb(0xff, 0xb9, 0x6c),       // orange
    mode_operate: Color::Rgb(0x8c, 0xe9, 0xfd),    // cyan
    status_ready: Color::Rgb(0x62, 0x72, 0xa4),    // comment
    status_working: Color::Rgb(0x8b, 0xe9, 0xfd),  // cyan
    status_warning: Color::Rgb(0xf1, 0xfa, 0x8c),  // yellow
    diff_added_fg: Color::Rgb(0x50, 0xfa, 0x7b),   // green
    diff_deleted_fg: Color::Rgb(0xff, 0x55, 0x55), // red
    diff_added_bg: Color::Rgb(0x21, 0x3a, 0x2a),
    diff_deleted_bg: Color::Rgb(0x3a, 0x1f, 0x22),
    tool_running: Color::Rgb(0x8b, 0xe9, 0xfd), // cyan
    tool_success: Color::Rgb(0x62, 0x72, 0xa4), // comment
    tool_failed: Color::Rgb(0xff, 0x55, 0x55),  // red
};

/// "Terminal" theme: lets the host terminal's color scheme show through
/// instead of painting any RGB surface. Backgrounds use `Color::Reset`
/// (the terminal's own default bg) and most text uses `Color::Reset`
/// (terminal's own default fg). Accents are ANSI named colors so they
/// also inherit the user's terminal palette (Solarized, Nord, custom
/// schemes, etc.) rather than Codewhale brand RGB.
pub const TERMINAL_UI_THEME: UiTheme = UiTheme {
    name: "terminal",
    // Mode is reported as Dark to avoid the dark→light cell remap kicking
    // in; the terminal-theme cell remap already normalizes everything to
    // `Color::Reset`, and we never want a second pass overwriting that.
    mode: PaletteMode::Dark,
    surface_bg: Color::Reset,
    panel_bg: Color::Reset,
    elevated_bg: Color::Reset,
    composer_bg: Color::Reset,
    selection_bg: Color::Reset,
    header_bg: Color::Reset,
    footer_bg: Color::Reset,
    text_dim: Color::Reset,
    text_hint: Color::Reset,
    text_muted: Color::Reset,
    text_body: Color::Reset,
    text_soft: Color::Reset,
    border: Color::Reset,
    accent_primary: Color::Blue,
    accent_secondary: Color::Cyan,
    accent_action: Color::Yellow,
    error_fg: Color::Red,
    error_hover: Color::Red,
    error_surface: Color::Reset,
    error_border: Color::Red,
    error_text: Color::Red,
    warning: Color::Yellow,
    success: Color::Green,
    info: Color::Cyan,
    mode_agent: Color::LightBlue,
    mode_yolo: Color::LightRed,
    // Magenta keeps Plan visually distinct from `status_warning` (yellow)
    // so the mode indicator and warning chip don't collide on themes that
    // render both in the status row.
    mode_plan: Color::Magenta,
    mode_operate: Color::LightCyan,
    // DarkGray gives "Ready" a low-contrast but still distinguishable hue
    // versus default body text (which is `Color::Reset` on this theme).
    status_ready: Color::DarkGray,
    status_working: Color::Cyan,
    status_warning: Color::Yellow,
    diff_added_fg: Color::Green,
    diff_deleted_fg: Color::Red,
    diff_added_bg: Color::Reset,
    diff_deleted_bg: Color::Reset,
    tool_running: Color::Cyan,
    tool_success: Color::Green,
    tool_failed: Color::Red,
};

pub const GRUVBOX_DARK_UI_THEME: UiTheme = UiTheme {
    name: "gruvbox-dark",
    mode: PaletteMode::Dark,
    surface_bg: Color::Rgb(0x28, 0x28, 0x28),  // bg0
    panel_bg: Color::Rgb(0x3c, 0x38, 0x36),    // bg1
    elevated_bg: Color::Rgb(0x50, 0x49, 0x45), // bg2
    composer_bg: Color::Rgb(0x3c, 0x38, 0x36),
    selection_bg: Color::Rgb(0x66, 0x5c, 0x54), // bg3
    header_bg: Color::Rgb(0x1d, 0x20, 0x21),    // bg0_h
    footer_bg: Color::Rgb(0x1d, 0x20, 0x21),
    text_dim: Color::Rgb(0x92, 0x83, 0x74),         // gray
    text_hint: Color::Rgb(0xa8, 0x99, 0x84),        // fg4
    text_muted: Color::Rgb(0xbd, 0xae, 0x93),       // fg3
    text_body: Color::Rgb(0xeb, 0xdb, 0xb2),        // fg1
    text_soft: Color::Rgb(0xd5, 0xc4, 0xa1),        // fg2
    border: Color::Rgb(0x66, 0x5c, 0x54),           // bg3
    accent_primary: Color::Rgb(0x83, 0xa5, 0x98),   // blue
    accent_secondary: Color::Rgb(0x8e, 0xc0, 0x7c), // aqua/green
    accent_action: Color::Rgb(0xfe, 0x80, 0x19),    // orange
    error_fg: Color::Rgb(0xfb, 0x49, 0x34),         // red
    error_hover: Color::Rgb(0xfc, 0x7c, 0x6b),
    error_surface: Color::Rgb(0x35, 0x1c, 0x18),
    error_border: Color::Rgb(0xfb, 0x49, 0x34),
    error_text: Color::Rgb(0xfc, 0xc4, 0xb8),
    warning: Color::Rgb(0xfa, 0xbd, 0x2f),         // yellow
    success: Color::Rgb(0x8e, 0xc0, 0x7c),         // green
    info: Color::Rgb(0x83, 0xa5, 0x98),            // blue
    mode_agent: Color::Rgb(0x84, 0xa5, 0x98),      // blue
    mode_yolo: Color::Rgb(0xfb, 0x4a, 0x34),       // red
    mode_plan: Color::Rgb(0xfe, 0x81, 0x19),       // orange
    mode_operate: Color::Rgb(0xd3, 0x86, 0x9b),    // purple
    status_ready: Color::Rgb(0x92, 0x83, 0x74),    // gray
    status_working: Color::Rgb(0x8e, 0xc0, 0x7c),  // aqua
    status_warning: Color::Rgb(0xfa, 0xbd, 0x2f),  // yellow
    diff_added_fg: Color::Rgb(0x8e, 0xc0, 0x7c),   // green
    diff_deleted_fg: Color::Rgb(0xfb, 0x49, 0x34), // red
    diff_added_bg: Color::Rgb(0x29, 0x32, 0x16),
    diff_deleted_bg: Color::Rgb(0x35, 0x1c, 0x18),
    tool_running: Color::Rgb(0x8e, 0xc0, 0x7c), // aqua
    tool_success: Color::Rgb(0x92, 0x83, 0x74), // gray
    tool_failed: Color::Rgb(0xfb, 0x49, 0x34),  // red
};

pub const CLAUDE_UI_THEME: UiTheme = UiTheme {
    name: "claude",
    mode: PaletteMode::Dark,
    // Claude Code product surfaces — dark navy with warm undertones
    surface_bg: Color::Rgb(0x18, 0x17, 0x15), // surface-dark
    panel_bg: Color::Rgb(0x25, 0x23, 0x20),   // surface-dark-elevated
    elevated_bg: Color::Rgb(0x1f, 0x1e, 0x1b), // surface-dark-soft (code blocks)
    composer_bg: Color::Rgb(0x25, 0x23, 0x20),
    selection_bg: Color::Rgb(0x30, 0x2d, 0x28),
    header_bg: Color::Rgb(0x18, 0x17, 0x15),
    footer_bg: Color::Rgb(0x18, 0x17, 0x15),
    // Cream-tinted text hierarchy on dark
    text_dim: Color::Rgb(0x72, 0x70, 0x6a),
    text_hint: Color::Rgb(0x7d, 0x7a, 0x73),
    text_muted: Color::Rgb(0xa0, 0x9d, 0x96), // on-dark-soft
    text_body: Color::Rgb(0xfa, 0xf9, 0xf5),  // on-dark (cream white)
    text_soft: Color::Rgb(0xd0, 0xcd, 0xc5),
    border: Color::Rgb(0x30, 0x2d, 0x28),
    // Coral primary (signature Anthropic accent), teal secondary
    accent_primary: Color::Rgb(0xcc, 0x78, 0x5c), // coral
    accent_secondary: Color::Rgb(0x5d, 0xb8, 0xa6), // accent-teal
    accent_action: Color::Rgb(0xe8, 0xa5, 0x5a),  // amber
    // Error / destructive — warm red
    error_fg: Color::Rgb(0xe0, 0x60, 0x60),
    error_hover: Color::Rgb(0xd9, 0x66, 0x66),
    error_surface: Color::Rgb(0x2a, 0x1c, 0x1c),
    error_border: Color::Rgb(0xe0, 0x60, 0x60),
    error_text: Color::Rgb(0xe8, 0xb8, 0xb8),
    // Status
    warning: Color::Rgb(0xd4, 0xa0, 0x17), // amber
    success: Color::Rgb(0x5d, 0xb8, 0x72), // green
    info: Color::Rgb(0x5d, 0xb8, 0xa6),    // teal
    // Mode badges
    mode_agent: Color::Rgb(0xcd, 0x78, 0x5c),   // coral
    mode_yolo: Color::Rgb(0xc7, 0x45, 0x45),    // red
    mode_plan: Color::Rgb(0xe8, 0xa6, 0x5a),    // amber
    mode_operate: Color::Rgb(0x8a, 0x63, 0xd2), // violet
    // Footer statusline
    status_ready: Color::Rgb(0xa0, 0x9d, 0x96),
    status_working: Color::Rgb(0x5d, 0xb8, 0xa6),
    status_warning: Color::Rgb(0xd4, 0xa0, 0x17),
    // Diff
    diff_added_fg: Color::Rgb(0x5d, 0xb8, 0x72),
    diff_deleted_fg: Color::Rgb(0xc6, 0x45, 0x45),
    diff_added_bg: Color::Rgb(0x1a, 0x24, 0x1d),
    diff_deleted_bg: Color::Rgb(0x24, 0x1a, 0x1a),
    // Tool cells
    tool_running: Color::Rgb(0x5d, 0xb8, 0xa6),
    tool_success: Color::Rgb(0xa0, 0x9d, 0x96),
    tool_failed: Color::Rgb(0xc6, 0x45, 0x45),
};

pub const MATRIX_UI_THEME: UiTheme = UiTheme {
    name: "matrix",
    mode: PaletteMode::Dark,
    surface_bg: Color::Rgb(
        MATRIX_SURFACE_RGB.0,
        MATRIX_SURFACE_RGB.1,
        MATRIX_SURFACE_RGB.2,
    ),
    panel_bg: Color::Rgb(
        MATRIX_SURFACE_RGB.0,
        MATRIX_SURFACE_RGB.1,
        MATRIX_SURFACE_RGB.2,
    ),
    elevated_bg: Color::Rgb(
        MATRIX_ELEVATED_RGB.0,
        MATRIX_ELEVATED_RGB.1,
        MATRIX_ELEVATED_RGB.2,
    ),
    composer_bg: Color::Rgb(
        MATRIX_SURFACE_RGB.0,
        MATRIX_SURFACE_RGB.1,
        MATRIX_SURFACE_RGB.2,
    ),
    selection_bg: Color::Rgb(
        MATRIX_SELECTION_RGB.0,
        MATRIX_SELECTION_RGB.1,
        MATRIX_SELECTION_RGB.2,
    ),
    header_bg: Color::Rgb(
        MATRIX_SURFACE_RGB.0,
        MATRIX_SURFACE_RGB.1,
        MATRIX_SURFACE_RGB.2,
    ),
    footer_bg: Color::Rgb(
        MATRIX_SURFACE_RGB.0,
        MATRIX_SURFACE_RGB.1,
        MATRIX_SURFACE_RGB.2,
    ),
    text_dim: Color::Rgb(
        MATRIX_TEXT_DIM_RGB.0,
        MATRIX_TEXT_DIM_RGB.1,
        MATRIX_TEXT_DIM_RGB.2,
    ),
    text_hint: Color::Rgb(
        MATRIX_TEXT_HINT_RGB.0,
        MATRIX_TEXT_HINT_RGB.1,
        MATRIX_TEXT_HINT_RGB.2,
    ),
    text_muted: Color::Rgb(
        MATRIX_TEXT_MUTED_RGB.0,
        MATRIX_TEXT_MUTED_RGB.1,
        MATRIX_TEXT_MUTED_RGB.2,
    ),
    text_body: Color::Rgb(
        MATRIX_TEXT_BODY_RGB.0,
        MATRIX_TEXT_BODY_RGB.1,
        MATRIX_TEXT_BODY_RGB.2,
    ),
    text_soft: Color::Rgb(
        MATRIX_TEXT_SOFT_RGB.0,
        MATRIX_TEXT_SOFT_RGB.1,
        MATRIX_TEXT_SOFT_RGB.2,
    ),
    border: Color::Rgb(
        MATRIX_BORDER_RGB.0,
        MATRIX_BORDER_RGB.1,
        MATRIX_BORDER_RGB.2,
    ),
    accent_primary: Color::Rgb(
        MATRIX_BORDER_RGB.0,
        MATRIX_BORDER_RGB.1,
        MATRIX_BORDER_RGB.2,
    ),
    accent_secondary: Color::Rgb(0, 153, 0),
    accent_action: Color::Rgb(0x88, 0xff, 0x88),
    error_fg: Color::Rgb(0xb4, 0, 0),
    error_hover: Color::Rgb(0xe0, 0, 0),
    error_surface: Color::Rgb(0x1a, 0x0d, 0x0d),
    error_border: Color::Rgb(0xb4, 0, 0),
    error_text: Color::Rgb(0xff, 0x44, 0x44),
    warning: Color::Rgb(204, 204, 0),
    success: Color::Rgb(0x88, 0xff, 0x88),
    info: Color::Rgb(0, 204, 0),
    mode_agent: Color::Rgb(0, 154, 0),
    mode_yolo: Color::Rgb(255, 100, 100),
    mode_plan: Color::Rgb(255, 170, 60),
    mode_operate: Color::Rgb(100, 255, 220),
    status_ready: Color::Rgb(0, 85, 0),
    status_working: Color::Rgb(
        MATRIX_TEXT_BODY_RGB.0,
        MATRIX_TEXT_BODY_RGB.1,
        MATRIX_TEXT_BODY_RGB.2,
    ),
    status_warning: Color::Rgb(204, 204, 0),
    diff_added_fg: Color::Rgb(0x88, 0xff, 0x88),
    diff_deleted_fg: Color::Rgb(0xb4, 0, 0),
    diff_added_bg: Color::Rgb(0x0d, 0x1a, 0x0d),
    diff_deleted_bg: Color::Rgb(0x1a, 0x0d, 0x0d),
    tool_running: Color::Rgb(0x88, 0xff, 0x88),
    tool_success: Color::Rgb(0, 102, 0),
    tool_failed: Color::Rgb(0xb4, 0, 0),
};

/// Stable identifiers for the named themes the user can select. `System`
/// defers to `PaletteMode::detect()` (terminal-driven dark/light). Each
/// dark/light id resolves to a single fixed `UiTheme`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeId {
    System,
    Terminal,
    Whale,
    WhaleLight,
    Grayscale,
    CatppuccinMocha,
    TokyoNight,
    Dracula,
    GruvboxDark,
    Claude,
    Matrix,
    SolarizedLight,
}

impl ThemeId {
    /// Parse a settings string (`"system"`, `"dark"`, `"catppuccin-mocha"`, …).
    /// Accepts a few aliases (`"whale"` for dark, `"light"` for whale-light)
    /// so existing config files keep working. Case-insensitive.
    #[must_use]
    pub fn from_name(value: &str) -> Option<Self> {
        match normalize_theme_name(value)? {
            "system" => Some(Self::System),
            "terminal" => Some(Self::Terminal),
            "dark" => Some(Self::Whale),
            "light" => Some(Self::WhaleLight),
            "grayscale" => Some(Self::Grayscale),
            "catppuccin-mocha" => Some(Self::CatppuccinMocha),
            "tokyo-night" => Some(Self::TokyoNight),
            "dracula" => Some(Self::Dracula),
            "gruvbox-dark" => Some(Self::GruvboxDark),
            "claude" => Some(Self::Claude),
            "matrix" => Some(Self::Matrix),
            "solarized-light" => Some(Self::SolarizedLight),
            _ => None,
        }
    }

    /// Canonical settings string (lowercase, dash-separated). Round-trips
    /// through `from_name`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Terminal => "terminal",
            Self::Whale => "dark",
            Self::WhaleLight => "light",
            Self::Grayscale => "grayscale",
            Self::CatppuccinMocha => "catppuccin-mocha",
            Self::TokyoNight => "tokyo-night",
            Self::Dracula => "dracula",
            Self::GruvboxDark => "gruvbox-dark",
            Self::Claude => "claude",
            Self::Matrix => "matrix",
            Self::SolarizedLight => "solarized-light",
        }
    }

    /// Human-readable label for picker rows.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Terminal => "Terminal",
            Self::Whale => "Blue Stage",
            Self::WhaleLight => "Blue Stage Light",
            Self::Grayscale => "Grayscale",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::TokyoNight => "Tokyo Night",
            Self::Dracula => "Dracula",
            Self::GruvboxDark => "Gruvbox Dark",
            Self::Claude => "Claude",
            Self::Matrix => "Matrix",
            Self::SolarizedLight => "Solarized Light",
        }
    }

    /// Short tagline for picker rows.
    #[must_use]
    pub const fn tagline(self) -> &'static str {
        match self {
            Self::System => "Follow terminal background (COLORFGBG / macOS appearance)",
            Self::Terminal => "Inherit terminal colors fully (transparent surfaces, ANSI accents)",
            Self::Whale => "Stage black, action blue, and one Signal Gold human beacon",
            Self::WhaleLight => "Paper, cobalt action, and one Signal Gold human beacon",
            Self::Grayscale => "Color-minimal high contrast",
            Self::CatppuccinMocha => "Soft pastels on warm dark",
            Self::TokyoNight => "Deep blue/violet night palette",
            Self::Dracula => "Classic high-contrast purple",
            Self::GruvboxDark => "Vintage warm earth tones",
            Self::Claude => "Warm navy & coral",
            Self::Matrix => "The Matrix films inspired theme",
            Self::SolarizedLight => {
                "Solarized light — Light, calming palette on warm ivory — easy on the eyes"
            }
        }
    }

    /// Resolve to a concrete `UiTheme`. For `System` this consults
    /// `PaletteMode::detect()` exactly once and returns the corresponding
    /// dark/light theme — callers that want to live-track terminal background
    /// changes need to re-invoke this.
    #[must_use]
    pub fn ui_theme(self) -> UiTheme {
        match self {
            Self::System => UiTheme::detect(),
            Self::Terminal => TERMINAL_UI_THEME,
            Self::Whale => UI_THEME,
            Self::WhaleLight => LIGHT_UI_THEME,
            Self::Grayscale => GRAYSCALE_UI_THEME,
            Self::CatppuccinMocha => CATPPUCCIN_MOCHA_UI_THEME,
            Self::TokyoNight => TOKYO_NIGHT_UI_THEME,
            Self::Dracula => DRACULA_UI_THEME,
            Self::GruvboxDark => GRUVBOX_DARK_UI_THEME,
            Self::Claude => CLAUDE_UI_THEME,
            Self::Matrix => MATRIX_UI_THEME,
            Self::SolarizedLight => SOLARIZED_LIGHT_UI_THEME,
        }
    }
}

/// Themes shown in the `/theme` picker, in display order.
pub const SELECTABLE_THEMES: &[ThemeId] = &[
    ThemeId::System,
    ThemeId::Terminal,
    ThemeId::Whale,
    ThemeId::WhaleLight,
    ThemeId::Grayscale,
    ThemeId::CatppuccinMocha,
    ThemeId::TokyoNight,
    ThemeId::Dracula,
    ThemeId::GruvboxDark,
    ThemeId::Claude,
    ThemeId::Matrix,
    ThemeId::SolarizedLight,
];

impl UiTheme {
    #[must_use]
    pub fn for_mode(mode: PaletteMode) -> Self {
        match mode {
            PaletteMode::Dark => UI_THEME,
            PaletteMode::Light => LIGHT_UI_THEME,
            PaletteMode::Grayscale => GRAYSCALE_UI_THEME,
            PaletteMode::SolarizedLight => SOLARIZED_LIGHT_UI_THEME,
        }
    }

    #[must_use]
    pub fn detect() -> Self {
        Self::for_mode(PaletteMode::detect())
    }

    #[must_use]
    pub fn from_setting(value: &str) -> Option<Self> {
        ThemeId::from_name(value).map(ThemeId::ui_theme)
    }

    #[must_use]
    pub fn with_background_color(mut self, color: Color) -> Self {
        self.surface_bg = color;
        self.header_bg = color;
        self.footer_bg = color;
        self
    }
}

#[must_use]
pub fn normalize_theme_name(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "auto" | "system" | "default" => Some("system"),
        "terminal" | "term" | "transparent" | "follow-terminal" | "inherit" => Some("terminal"),
        "dark" | "whale" | "whale-dark" => Some("dark"),
        "light" | "whale-light" => Some("light"),
        "grayscale" | "greyscale" | "gray" | "grey" | "mono" | "monochrome" | "black-white"
        | "black_and_white" | "blackwhite" | "bw" | "b&w" => Some("grayscale"),
        "catppuccin-mocha" | "catppuccin" | "mocha" => Some("catppuccin-mocha"),
        "tokyo-night" | "tokyonight" | "tokyo" => Some("tokyo-night"),
        "dracula" => Some("dracula"),
        "gruvbox-dark" | "gruvbox" => Some("gruvbox-dark"),
        "claude" => Some("claude"),
        "matrix" | "hacker" => Some("matrix"),
        "solarized-light" | "solarized" => Some("solarized-light"),
        _ => None,
    }
}

#[must_use]
pub fn theme_label_for_mode(mode: PaletteMode) -> &'static str {
    match mode {
        PaletteMode::Dark => "dark",
        PaletteMode::Light => "light",
        PaletteMode::Grayscale => "grayscale",
        PaletteMode::SolarizedLight => "solarized-light",
    }
}

#[must_use]
pub fn ui_theme_from_settings(theme: &str, background_color: Option<&str>) -> UiTheme {
    let mut ui_theme = UiTheme::from_setting(theme).unwrap_or_else(UiTheme::detect);
    if let Some(background) = background_color.and_then(parse_hex_rgb_color) {
        ui_theme = ui_theme.with_background_color(background);
    }
    ui_theme
}

#[must_use]
pub fn parse_hex_rgb_color(value: &str) -> Option<Color> {
    let hex = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if hex.len() != 6 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }

    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

#[must_use]
pub fn normalize_hex_rgb_color(value: &str) -> Option<String> {
    hex_rgb_string(parse_hex_rgb_color(value)?)
}

#[must_use]
pub fn hex_rgb_string(color: Color) -> Option<String> {
    let Color::Rgb(r, g, b) = color else {
        return None;
    };
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whale_refresh_keeps_theme_ids_and_default_compatibility() {
        let names = SELECTABLE_THEMES
            .iter()
            .map(|theme| theme.name())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "system",
                "terminal",
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
            ]
        );
        assert_eq!(normalize_theme_name("default"), Some("system"));
        assert_eq!(normalize_theme_name("whale"), Some("dark"));
    }

    #[test]
    fn whale_pair_uses_codewhale_semantic_grammar() {
        assert_eq!(UI_THEME.accent_primary, WHALE_ACTION);
        assert_eq!(UI_THEME.status_working, WHALE_LIVE);
        assert_eq!(UI_THEME.accent_action, WHALE_HUMAN);
        assert_eq!(UI_THEME.warning, STATUS_WARNING);
        assert_eq!(UI_THEME.error_fg, WHALE_ERROR);
        assert_eq!(UI_THEME.mode_operate, MODE_OPERATE);
        assert_eq!(LIGHT_UI_THEME.accent_primary, LIGHT_ACTION);
        assert_eq!(LIGHT_UI_THEME.status_working, LIGHT_LIVE);
        assert_eq!(LIGHT_UI_THEME.accent_action, LIGHT_HUMAN);
        assert_eq!(LIGHT_UI_THEME.warning, LIGHT_WARNING);
        assert_eq!(LIGHT_UI_THEME.error_fg, LIGHT_DANGER);
        assert_eq!(LIGHT_UI_THEME.mode_operate, LIGHT_OPERATE);
    }

    /// Dogfood A7 (#4092): every mode must be tellable apart from the footer
    /// badge alone — Operate must never wear the full-access red again. Mode
    /// slots also stay distinct from general semantic lanes so limited-color
    /// adaptation can identify direct `UiTheme` call sites without guessing.
    #[test]
    fn every_selectable_theme_keeps_mode_badges_distinct() {
        for theme_id in SELECTABLE_THEMES {
            let ui = theme_id.ui_theme();
            let badges = [
                ("act", ui.mode_agent),
                ("plan", ui.mode_plan),
                ("operate", ui.mode_operate),
                ("full access", ui.mode_yolo),
            ];
            for (i, (name_a, color_a)) in badges.iter().enumerate() {
                for (name_b, color_b) in badges.iter().skip(i + 1) {
                    assert_ne!(
                        color_a,
                        color_b,
                        "theme '{}' renders modes '{name_a}' and '{name_b}' with the same badge color",
                        theme_id.name(),
                    );
                }
            }
            let semantic_lanes = [
                ("action", ui.accent_primary),
                ("live", ui.status_working),
                ("human", ui.accent_action),
                ("warning", ui.warning),
                ("danger", ui.error_fg),
                ("success", ui.success),
            ];
            for (mode_name, mode_color) in badges {
                for (lane_name, lane_color) in semantic_lanes {
                    assert_ne!(
                        mode_color,
                        lane_color,
                        "theme '{}' reuses the {lane_name} color for mode '{mode_name}', erasing render-stage identity",
                        theme_id.name(),
                    );
                }
            }
        }
    }

    #[test]
    fn every_selectable_theme_separates_live_workers_from_completed_work() {
        for theme_id in SELECTABLE_THEMES {
            let ui = theme_id.ui_theme();
            assert_ne!(
                ui.info,
                ui.success,
                "theme '{}' makes live workers indistinguishable from completed work",
                theme_id.name(),
            );
        }
    }
}
