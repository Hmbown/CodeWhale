//! Color palette — Claude Code style: warm-neutral dark, soft clay accents.

use ratatui::style::Color;

// === Base RGBs ===
pub const INK_RGB: (u8, u8, u8) = (16, 15, 13);
pub const SLATE_RGB: (u8, u8, u8) = (25, 23, 20);
pub const ELEVATED_RGB: (u8, u8, u8) = (35, 32, 28);
pub const BORDER_RGB: (u8, u8, u8) = (72, 64, 55);
pub const AMBER_RGB: (u8, u8, u8) = (219, 128, 91);
pub const AMBER_DIM_RGB: (u8, u8, u8) = (176, 113, 78);
pub const GREEN_RGB: (u8, u8, u8) = (91, 164, 108);
pub const RED_RGB: (u8, u8, u8) = (232, 93, 86);
pub const BLUE_RGB: (u8, u8, u8) = AMBER_RGB;
pub const SKY_RGB: (u8, u8, u8) = (238, 156, 112);
#[allow(dead_code)]
pub const PINK_RGB: (u8, u8, u8) = (219, 109, 165);

// Legacy re-exports — keep for minimal diff
pub const DEEPSEEK_BLUE_RGB: (u8, u8, u8) = BLUE_RGB;
pub const DEEPSEEK_SKY_RGB: (u8, u8, u8) = SKY_RGB;
pub const DEEPSEEK_INK_RGB: (u8, u8, u8) = INK_RGB;
pub const DEEPSEEK_SLATE_RGB: (u8, u8, u8) = SLATE_RGB;
pub const DEEPSEEK_RED_RGB: (u8, u8, u8) = RED_RGB;
#[allow(dead_code)]
pub const DEEPSEEK_AQUA_RGB: (u8, u8, u8) = (54, 187, 212);
#[allow(dead_code)]
pub const DEEPSEEK_NAVY_RGB: (u8, u8, u8) = (24, 63, 138);

// Light theme
pub const LIGHT_SURFACE_RGB: (u8, u8, u8) = (255, 255, 255);
pub const LIGHT_PANEL_RGB: (u8, u8, u8) = (250, 248, 244);
pub const LIGHT_ELEVATED_RGB: (u8, u8, u8) = (242, 238, 231);
pub const LIGHT_REASONING_RGB: (u8, u8, u8) = (255, 244, 232);
pub const LIGHT_SUCCESS_RGB: (u8, u8, u8) = (227, 245, 231);
pub const LIGHT_ERROR_RGB: (u8, u8, u8) = (255, 237, 233);
pub const LIGHT_TEXT_BODY_RGB: (u8, u8, u8) = (39, 35, 31);
pub const LIGHT_TEXT_MUTED_RGB: (u8, u8, u8) = (108, 99, 88);
pub const LIGHT_TEXT_HINT_RGB: (u8, u8, u8) = (139, 128, 113);
pub const LIGHT_TEXT_SOFT_RGB: (u8, u8, u8) = (67, 60, 52);
pub const LIGHT_BORDER_RGB: (u8, u8, u8) = (216, 204, 188);
pub const LIGHT_SELECTION_RGB: (u8, u8, u8) = (246, 216, 197);

#[allow(dead_code)]
pub const BORDER_COLOR_RGB: (u8, u8, u8) = BORDER_RGB;

// === Named Colors ===
pub const INK: Color = Color::Rgb(INK_RGB.0, INK_RGB.1, INK_RGB.2);
pub const SLATE: Color = Color::Rgb(SLATE_RGB.0, SLATE_RGB.1, SLATE_RGB.2);
pub const ELEVATED: Color = Color::Rgb(ELEVATED_RGB.0, ELEVATED_RGB.1, ELEVATED_RGB.2);
pub const BORDER_COLOR: Color = Color::Rgb(BORDER_RGB.0, BORDER_RGB.1, BORDER_RGB.2);
pub const AMBER: Color = Color::Rgb(AMBER_RGB.0, AMBER_RGB.1, AMBER_RGB.2);
pub const AMBER_DIM: Color = Color::Rgb(AMBER_DIM_RGB.0, AMBER_DIM_RGB.1, AMBER_DIM_RGB.2);
pub const GREEN: Color = Color::Rgb(GREEN_RGB.0, GREEN_RGB.1, GREEN_RGB.2);
pub const RED: Color = Color::Rgb(RED_RGB.0, RED_RGB.1, RED_RGB.2);
pub const BLUE: Color = Color::Rgb(BLUE_RGB.0, BLUE_RGB.1, BLUE_RGB.2);
pub const SKY: Color = Color::Rgb(SKY_RGB.0, SKY_RGB.1, SKY_RGB.2);
#[allow(dead_code)]
pub const PINK: Color = Color::Rgb(PINK_RGB.0, PINK_RGB.1, PINK_RGB.2);

pub const LIGHT_SURFACE: Color = Color::Rgb(
    LIGHT_SURFACE_RGB.0,
    LIGHT_SURFACE_RGB.1,
    LIGHT_SURFACE_RGB.2,
);
pub const LIGHT_PANEL: Color = Color::Rgb(LIGHT_PANEL_RGB.0, LIGHT_PANEL_RGB.1, LIGHT_PANEL_RGB.2);
pub const LIGHT_ELEVATED: Color = Color::Rgb(
    LIGHT_ELEVATED_RGB.0,
    LIGHT_ELEVATED_RGB.1,
    LIGHT_ELEVATED_RGB.2,
);
pub const LIGHT_REASONING: Color = Color::Rgb(
    LIGHT_REASONING_RGB.0,
    LIGHT_REASONING_RGB.1,
    LIGHT_REASONING_RGB.2,
);
pub const LIGHT_SUCCESS: Color = Color::Rgb(
    LIGHT_SUCCESS_RGB.0,
    LIGHT_SUCCESS_RGB.1,
    LIGHT_SUCCESS_RGB.2,
);
pub const LIGHT_ERROR: Color = Color::Rgb(LIGHT_ERROR_RGB.0, LIGHT_ERROR_RGB.1, LIGHT_ERROR_RGB.2);
pub const LIGHT_TEXT_BODY: Color = Color::Rgb(
    LIGHT_TEXT_BODY_RGB.0,
    LIGHT_TEXT_BODY_RGB.1,
    LIGHT_TEXT_BODY_RGB.2,
);
pub const LIGHT_TEXT_MUTED: Color = Color::Rgb(
    LIGHT_TEXT_MUTED_RGB.0,
    LIGHT_TEXT_MUTED_RGB.1,
    LIGHT_TEXT_MUTED_RGB.2,
);
pub const LIGHT_TEXT_HINT: Color = Color::Rgb(
    LIGHT_TEXT_HINT_RGB.0,
    LIGHT_TEXT_HINT_RGB.1,
    LIGHT_TEXT_HINT_RGB.2,
);
pub const LIGHT_TEXT_SOFT: Color = Color::Rgb(
    LIGHT_TEXT_SOFT_RGB.0,
    LIGHT_TEXT_SOFT_RGB.1,
    LIGHT_TEXT_SOFT_RGB.2,
);
pub const LIGHT_BORDER: Color =
    Color::Rgb(LIGHT_BORDER_RGB.0, LIGHT_BORDER_RGB.1, LIGHT_BORDER_RGB.2);
pub const LIGHT_SELECTION_BG: Color = Color::Rgb(
    LIGHT_SELECTION_RGB.0,
    LIGHT_SELECTION_RGB.1,
    LIGHT_SELECTION_RGB.2,
);

// Legacy re-exports — keep call sites minimal diff
pub const DEEPSEEK_BLUE: Color = BLUE;
pub const DEEPSEEK_SKY: Color = SKY;
pub const DEEPSEEK_INK: Color = INK;
pub const DEEPSEEK_SLATE: Color = SLATE;
pub const DEEPSEEK_RED: Color = RED;
#[allow(dead_code)]
pub const DEEPSEEK_AQUA: Color = Color::Rgb(54, 187, 212);
#[allow(dead_code)]
pub const DEEPSEEK_NAVY: Color = Color::Rgb(24, 63, 138);

// === Semantic text colors ===
pub const TEXT_BODY: Color = Color::Rgb(235, 230, 218);
pub const TEXT_SECONDARY: Color = Color::Rgb(166, 155, 140);
pub const TEXT_HINT: Color = Color::Rgb(119, 110, 98);
pub const TEXT_SOFT: Color = Color::Rgb(213, 204, 190);
pub const TEXT_ACCENT: Color = SKY;
pub const SELECTION_TEXT: Color = Color::White;
pub const TEXT_REASONING: Color = Color::Rgb(206, 158, 112);

pub const TEXT_PRIMARY: Color = TEXT_BODY;
pub const TEXT_MUTED: Color = TEXT_SECONDARY;
pub const TEXT_DIM: Color = TEXT_HINT;
pub const USER_BODY: Color = TEXT_BODY;
pub const LIGHT_USER_BODY: Color = Color::Rgb(21, 128, 61); // #15803D green

// === Surfaces ===
pub const SURFACE_ELEVATED: Color = ELEVATED;
pub const SURFACE_REASONING: Color = Color::Rgb(50, 38, 27);
pub const SURFACE_REASONING_TINT: Color = Color::Rgb(27, 23, 19);
#[allow(dead_code)]
pub const SURFACE_REASONING_ACTIVE: Color = Color::Rgb(60, 45, 31);
#[allow(dead_code)]
pub const SURFACE_TOOL: Color = Color::Rgb(30, 27, 23);
#[allow(dead_code)]
pub const SURFACE_TOOL_ACTIVE: Color = Color::Rgb(39, 35, 30);
#[allow(dead_code)]
pub const SURFACE_SUCCESS: Color = Color::Rgb(25, 48, 34);
#[allow(dead_code)]
pub const SURFACE_ERROR: Color = Color::Rgb(56, 27, 25);
#[allow(dead_code)]
pub const SURFACE_PANEL: Color = Color::Rgb(25, 23, 20);
#[allow(dead_code)]
pub const BACKGROUND_DARK: Color = INK;
const LEGACY_BACKGROUND_DARK: Color = Color::Rgb(11, 21, 38);
#[allow(dead_code)]
pub const COMPOSER_BG: Color = SLATE;

// === Diff ===
pub const DIFF_ADDED_BG: Color = Color::Rgb(22, 48, 33);
pub const DIFF_DELETED_BG: Color = Color::Rgb(55, 25, 24);
pub const DIFF_ADDED: Color = Color::Rgb(113, 195, 129);

// === Accent colors ===
pub const ACCENT_REASONING_LIVE: Color = AMBER_DIM;
pub const ACCENT_TOOL_LIVE: Color = AMBER;
pub const ACCENT_TOOL_ISSUE: Color = Color::Rgb(224, 126, 112);
pub const TEXT_TOOL_OUTPUT: Color = Color::Rgb(190, 181, 166);
#[allow(dead_code)]
pub const ACCENT_PRIMARY: Color = BLUE;
#[allow(dead_code)]
pub const ACCENT_SECONDARY: Color = SKY;

// === Status ===
pub const STATUS_SUCCESS: Color = GREEN;
pub const STATUS_WARNING: Color = AMBER;
pub const STATUS_ERROR: Color = RED;
#[allow(dead_code)]
pub const STATUS_INFO: Color = BLUE;
#[allow(dead_code)]
pub const STATUS_NEUTRAL: Color = Color::Rgb(160, 160, 160);

// === Mode badges ===
pub const MODE_AGENT: Color = SKY;
pub const MODE_YOLO: Color = RED;
pub const MODE_PLAN: Color = AMBER;

// === Selection ===
pub const SELECTION_BG: Color = Color::Rgb(89, 61, 45);

// === UiTheme ===
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiTheme {
    pub name: &'static str,
    pub mode: PaletteMode,
    pub surface_bg: Color,
    pub panel_bg: Color,
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
    pub border: Color,
}

pub const UI_THEME: UiTheme = UiTheme {
    name: "claude-code",
    mode: PaletteMode::Dark,
    surface_bg: Color::Reset,
    panel_bg: SLATE,
    elevated_bg: ELEVATED,
    composer_bg: Color::Reset,
    selection_bg: SELECTION_BG,
    header_bg: Color::Reset,
    footer_bg: Color::Reset,
    mode_agent: MODE_AGENT,
    mode_yolo: MODE_YOLO,
    mode_plan: MODE_PLAN,
    status_ready: TEXT_MUTED,
    status_working: SKY,
    status_warning: AMBER,
    text_dim: TEXT_DIM,
    text_hint: TEXT_HINT,
    text_muted: TEXT_MUTED,
    text_body: TEXT_BODY,
    text_soft: TEXT_SOFT,
    border: BORDER_COLOR,
};

pub const LIGHT_UI_THEME: UiTheme = UiTheme {
    name: "claude-code-light",
    mode: PaletteMode::Light,
    surface_bg: LIGHT_SURFACE,
    panel_bg: LIGHT_PANEL,
    elevated_bg: LIGHT_ELEVATED,
    composer_bg: LIGHT_PANEL,
    selection_bg: LIGHT_SELECTION_BG,
    header_bg: LIGHT_SURFACE,
    footer_bg: LIGHT_SURFACE,
    mode_agent: BLUE,
    mode_yolo: RED,
    mode_plan: Color::Rgb(180, 83, 9),
    status_ready: LIGHT_TEXT_MUTED,
    status_working: BLUE,
    status_warning: Color::Rgb(180, 83, 9),
    text_dim: LIGHT_TEXT_HINT,
    text_hint: LIGHT_TEXT_HINT,
    text_muted: LIGHT_TEXT_MUTED,
    text_body: LIGHT_TEXT_BODY,
    text_soft: LIGHT_TEXT_SOFT,
    border: LIGHT_BORDER,
};

// === PaletteMode ===
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    Dark,
    Light,
}

impl PaletteMode {
    #[must_use]
    pub fn from_colorfgbg(value: &str) -> Option<Self> {
        let bg = value
            .split(';')
            .rev()
            .find_map(|part| part.parse::<u16>().ok())?;
        Some(if bg >= 8 { Self::Light } else { Self::Dark })
    }
    #[must_use]
    pub fn detect() -> Self {
        std::env::var("COLORFGBG")
            .ok()
            .and_then(|v| Self::from_colorfgbg(&v))
            .unwrap_or(Self::Dark)
    }
}

impl UiTheme {
    #[must_use]
    pub fn for_mode(mode: PaletteMode) -> Self {
        match mode {
            PaletteMode::Dark => UI_THEME,
            PaletteMode::Light => LIGHT_UI_THEME,
        }
    }
    #[must_use]
    pub fn detect() -> Self {
        Self::for_mode(PaletteMode::detect())
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

#[must_use]
pub fn adapt_fg_for_palette_mode(color: Color, _bg: Color, mode: PaletteMode) -> Color {
    if mode == PaletteMode::Dark {
        return color;
    }

    if color == TEXT_BODY || color == SELECTION_TEXT || color == Color::White {
        LIGHT_TEXT_BODY
    } else if color == TEXT_SECONDARY || color == TEXT_MUTED {
        LIGHT_TEXT_MUTED
    } else if color == TEXT_HINT || color == TEXT_DIM {
        LIGHT_TEXT_HINT
    } else if color == TEXT_SOFT || color == TEXT_TOOL_OUTPUT {
        LIGHT_TEXT_SOFT
    } else if color == BORDER_COLOR {
        LIGHT_BORDER
    } else if color == TEXT_ACCENT || color == DEEPSEEK_SKY || color == ACCENT_TOOL_LIVE {
        DEEPSEEK_BLUE
    } else if color == TEXT_REASONING || color == ACCENT_REASONING_LIVE {
        Color::Rgb(146, 64, 14)
    } else if color == ACCENT_TOOL_ISSUE {
        Color::Rgb(159, 18, 57)
    } else if color == DIFF_ADDED {
        Color::Rgb(22, 101, 52)
    } else if color == USER_BODY {
        LIGHT_USER_BODY
    } else {
        color
    }
}

#[must_use]
pub fn adapt_bg_for_palette_mode(color: Color, mode: PaletteMode) -> Color {
    if mode == PaletteMode::Dark {
        return color;
    }

    if color == DEEPSEEK_INK || color == BACKGROUND_DARK || color == LEGACY_BACKGROUND_DARK {
        LIGHT_SURFACE
    } else if color == DEEPSEEK_SLATE
        || color == COMPOSER_BG
        || color == SURFACE_PANEL
        || color == SURFACE_TOOL
    {
        LIGHT_PANEL
    } else if color == SURFACE_ELEVATED || color == SURFACE_TOOL_ACTIVE {
        LIGHT_ELEVATED
    } else if color == SURFACE_REASONING
        || color == SURFACE_REASONING_TINT
        || color == SURFACE_REASONING_ACTIVE
    {
        LIGHT_REASONING
    } else if color == SURFACE_SUCCESS {
        LIGHT_SUCCESS
    } else if color == SURFACE_ERROR {
        LIGHT_ERROR
    } else if color == DIFF_ADDED_BG {
        LIGHT_SUCCESS
    } else if color == DIFF_DELETED_BG {
        LIGHT_ERROR
    } else if color == SELECTION_BG {
        LIGHT_SELECTION_BG
    } else {
        color
    }
}

// === Color depth + brightness helpers (v0.6.6 UI redesign) ===

/// Terminal color depth, used to gate truecolor surfaces (e.g. reasoning bg
/// tints) on terminals that can't render them faithfully.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    Ansi16,
    Ansi256,
    TrueColor,
}

impl ColorDepth {
    #[must_use]
    pub fn detect() -> Self {
        if let Ok(ct) = std::env::var("COLORTERM") {
            if ct.to_ascii_lowercase().contains("truecolor")
                || ct.to_ascii_lowercase().contains("24bit")
            {
                return Self::TrueColor;
            }
        }
        if std::env::var_os("WT_SESSION").is_some() {
            return Self::TrueColor;
        }
        if let Ok(tp) = std::env::var("TERM_PROGRAM") {
            let tp = tp.to_ascii_lowercase();
            if tp.contains("iterm")
                || tp.contains("wezterm")
                || tp.contains("vscode")
                || tp.contains("warp")
            {
                return Self::TrueColor;
            }
        }
        match std::env::var("TERM")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            t if t.contains("truecolor") || t.contains("24bit") => Self::TrueColor,
            t if t.contains("256") => Self::Ansi256,
            "" | "dumb" => Self::Ansi16,
            _ => Self::Ansi256,
        }
    }
}

#[allow(dead_code)]
#[must_use]
pub fn adapt_color(color: Color, depth: ColorDepth) -> Color {
    match (color, depth) {
        (_, ColorDepth::TrueColor) => color,
        (Color::Rgb(r, g, b), ColorDepth::Ansi256) => Color::Indexed(rgb_to_ansi256(r, g, b)),
        (Color::Rgb(r, g, b), ColorDepth::Ansi16) => nearest_ansi16(r, g, b),
        _ => color,
    }
}

#[allow(dead_code)]
#[must_use]
pub fn adapt_bg(color: Color, depth: ColorDepth) -> Color {
    match (color, depth) {
        (_, ColorDepth::TrueColor) => color,
        (Color::Rgb(r, g, b), ColorDepth::Ansi256) => Color::Indexed(rgb_to_ansi256(r, g, b)),
        (_, ColorDepth::Ansi256) => color,
        (_, ColorDepth::Ansi16) => Color::Reset,
    }
}

#[must_use]
pub fn reasoning_surface_tint(depth: ColorDepth) -> Option<Color> {
    match depth {
        ColorDepth::Ansi16 => None,
        _ => Some(adapt_bg(SURFACE_REASONING_TINT, depth)),
    }
}

#[allow(dead_code)]
#[must_use]
pub fn blend(fg: Color, bg: Color, alpha: f32) -> Color {
    let alpha = alpha.clamp(0.0, 1.0);
    match (fg, bg) {
        (Color::Rgb(fr, fg_, fb), Color::Rgb(br, bg_, bb)) => {
            let mix = |a: u8, b: u8| -> u8 {
                (f32::from(b) + (f32::from(a) - f32::from(b)) * alpha)
                    .round()
                    .clamp(0.0, 255.0) as u8
            };
            Color::Rgb(mix(fr, br), mix(fg_, bg_), mix(fb, bb))
        }
        _ => fg,
    }
}

#[must_use]
pub fn pulse_brightness(color: Color, now_ms: u64) -> Color {
    let phase = (now_ms % 2000) as f32 / 2000.0;
    let t = (phase * std::f32::consts::TAU).sin() * 0.5 + 0.5;
    let alpha = 0.30 + t * 0.70;
    match color {
        Color::Rgb(r, g, b) => {
            let s = |c: u8| -> u8 { ((f32::from(c)) * alpha).round().clamp(0.0, 255.0) as u8 };
            Color::Rgb(s(r), s(g), s(b))
        }
        other => other,
    }
}

#[must_use]
#[allow(dead_code)]
fn nearest_ansi16(r: u8, g: u8, b: u8) -> Color {
    let lum = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    if lum < 24 {
        return Color::Black;
    }
    if r > 220 && g > 220 && b > 220 {
        return Color::White;
    }
    let bright = lum > 144;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max.saturating_sub(min) < 16 {
        return if bright { Color::Gray } else { Color::DarkGray };
    }
    if r >= g && r >= b {
        if g > b + 24 {
            return if bright {
                Color::LightYellow
            } else {
                Color::Yellow
            };
        } else if b > r.saturating_sub(24) {
            return if bright {
                Color::LightMagenta
            } else {
                Color::Magenta
            };
        } else {
            return if bright { Color::LightRed } else { Color::Red };
        }
    } else if g >= r && g >= b {
        if b > r + 24 {
            return if bright {
                Color::LightCyan
            } else {
                Color::Cyan
            };
        } else {
            return if bright {
                Color::LightGreen
            } else {
                Color::Green
            };
        }
    } else if r.saturating_add(48) >= b && r > g + 24 {
        return if bright {
            Color::LightMagenta
        } else {
            Color::Magenta
        };
    } else if g.saturating_add(48) >= b && g > r + 24 {
        return if bright {
            Color::LightCyan
        } else {
            Color::Cyan
        };
    } else {
        return if bright {
            Color::LightBlue
        } else {
            Color::Blue
        };
    }
}

#[allow(dead_code)]
fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    fn nearest_cube_level(channel: u8) -> usize {
        CUBE_LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, lv)| channel.abs_diff(**lv))
            .map(|(i, _)| i)
            .unwrap_or(0)
    }
    fn dist_sq(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
        let dr = i32::from(a.0) - i32::from(b.0);
        let dg = i32::from(a.1) - i32::from(b.1);
        let db = i32::from(a.2) - i32::from(b.2);
        (dr * dr + dg * dg + db * db) as u32
    }
    let ri = nearest_cube_level(r);
    let gi = nearest_cube_level(g);
    let bi = nearest_cube_level(b);
    let cube_rgb = (CUBE_LEVELS[ri], CUBE_LEVELS[gi], CUBE_LEVELS[bi]);
    let cube_index = 16 + (36 * ri) as u8 + (6 * gi) as u8 + bi as u8;
    let avg = ((u16::from(r) + u16::from(g) + u16::from(b)) / 3) as u8;
    let gray_i = if avg <= 8 {
        0
    } else if avg >= 238 {
        23
    } else {
        ((u16::from(avg) - 8 + 5) / 10).min(23) as u8
    };
    let gray = 8 + 10 * gray_i;
    let gray_index = 232 + gray_i;
    if dist_sq((r, g, b), (gray, gray, gray)) < dist_sq((r, g, b), cube_rgb) {
        gray_index
    } else {
        cube_index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_mode_parses_colorfgbg_background_slot() {
        assert_eq!(
            PaletteMode::from_colorfgbg("0;15"),
            Some(PaletteMode::Light)
        );
        assert_eq!(PaletteMode::from_colorfgbg("15;0"), Some(PaletteMode::Dark));
    }

    #[test]
    fn ui_theme_selects_light_variant() {
        let theme = UiTheme::for_mode(PaletteMode::Light);
        assert_eq!(theme.surface_bg, LIGHT_SURFACE);
        assert_eq!(theme.text_body, LIGHT_TEXT_BODY);
    }

    #[test]
    fn dark_palette_uses_correct_tokens() {
        assert_eq!(INK, Color::Rgb(16, 15, 13));
        assert_eq!(AMBER, Color::Rgb(219, 128, 91));
        assert_eq!(GREEN, Color::Rgb(91, 164, 108));
        assert_eq!(RED, Color::Rgb(232, 93, 86));
    }

    #[test]
    fn legacy_re_exports_match() {
        assert_eq!(DEEPSEEK_BLUE, BLUE);
        assert_eq!(DEEPSEEK_SKY, SKY);
        assert_eq!(DEEPSEEK_INK, INK);
        assert_eq!(DEEPSEEK_RED, RED);
    }

    #[test]
    fn parse_hex_rgb_works() {
        assert_eq!(parse_hex_rgb_color("#100f0d"), Some(INK));
        assert_eq!(parse_hex_rgb_color("#zzz"), None);
    }
}
