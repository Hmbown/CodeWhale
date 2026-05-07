//! DeepSeek color palette and semantic roles.

use ratatui::style::Color;

pub const DEEPSEEK_BLUE_RGB: (u8, u8, u8) = (53, 120, 229); // #3578E5
pub const DEEPSEEK_SKY_RGB: (u8, u8, u8) = (106, 174, 242);
#[allow(dead_code)]
pub const DEEPSEEK_AQUA_RGB: (u8, u8, u8) = (54, 187, 212);
#[allow(dead_code)]
pub const DEEPSEEK_NAVY_RGB: (u8, u8, u8) = (24, 63, 138);
pub const DEEPSEEK_INK_RGB: (u8, u8, u8) = (11, 21, 38);
pub const DEEPSEEK_SLATE_RGB: (u8, u8, u8) = (18, 28, 46);
pub const DEEPSEEK_RED_RGB: (u8, u8, u8) = (226, 80, 96);

pub const LIGHT_SURFACE_RGB: (u8, u8, u8) = (248, 250, 252); // #F8FAFC
pub const LIGHT_PANEL_RGB: (u8, u8, u8) = (241, 245, 249); // #F1F5F9
pub const LIGHT_ELEVATED_RGB: (u8, u8, u8) = (226, 232, 240); // #E2E8F0
pub const LIGHT_REASONING_RGB: (u8, u8, u8) = (254, 243, 199); // #FEF3C7
pub const LIGHT_SUCCESS_RGB: (u8, u8, u8) = (220, 252, 231); // #DCFCE7
pub const LIGHT_ERROR_RGB: (u8, u8, u8) = (254, 226, 226); // #FEE2E2
pub const LIGHT_TEXT_BODY_RGB: (u8, u8, u8) = (15, 23, 42); // #0F172A
pub const LIGHT_TEXT_MUTED_RGB: (u8, u8, u8) = (71, 85, 105); // #475569
pub const LIGHT_TEXT_HINT_RGB: (u8, u8, u8) = (100, 116, 139); // #64748B
pub const LIGHT_TEXT_SOFT_RGB: (u8, u8, u8) = (51, 65, 85); // #334155
pub const LIGHT_BORDER_RGB: (u8, u8, u8) = (148, 163, 184); // #94A3B8
pub const LIGHT_SELECTION_RGB: (u8, u8, u8) = (219, 234, 254); // #DBEAFE

// New semantic colors
pub const BORDER_COLOR_RGB: (u8, u8, u8) = (42, 74, 127); // #2A4A7F

pub const DEEPSEEK_BLUE: Color = Color::Rgb(
    DEEPSEEK_BLUE_RGB.0,
    DEEPSEEK_BLUE_RGB.1,
    DEEPSEEK_BLUE_RGB.2,
);
pub const DEEPSEEK_SKY: Color =
    Color::Rgb(DEEPSEEK_SKY_RGB.0, DEEPSEEK_SKY_RGB.1, DEEPSEEK_SKY_RGB.2);
#[allow(dead_code)]
pub const DEEPSEEK_AQUA: Color = Color::Rgb(
    DEEPSEEK_AQUA_RGB.0,
    DEEPSEEK_AQUA_RGB.1,
    DEEPSEEK_AQUA_RGB.2,
);
#[allow(dead_code)]
pub const DEEPSEEK_NAVY: Color = Color::Rgb(
    DEEPSEEK_NAVY_RGB.0,
    DEEPSEEK_NAVY_RGB.1,
    DEEPSEEK_NAVY_RGB.2,
);
pub const DEEPSEEK_INK: Color =
    Color::Rgb(DEEPSEEK_INK_RGB.0, DEEPSEEK_INK_RGB.1, DEEPSEEK_INK_RGB.2);
pub const DEEPSEEK_SLATE: Color = Color::Rgb(
    DEEPSEEK_SLATE_RGB.0,
    DEEPSEEK_SLATE_RGB.1,
    DEEPSEEK_SLATE_RGB.2,
);
pub const DEEPSEEK_RED: Color =
    Color::Rgb(DEEPSEEK_RED_RGB.0, DEEPSEEK_RED_RGB.1, DEEPSEEK_RED_RGB.2);

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

pub const TEXT_BODY: Color = Color::White;
pub const TEXT_SECONDARY: Color = Color::Rgb(192, 192, 192); // #C0C0C0
pub const TEXT_HINT: Color = Color::Rgb(160, 160, 160); // #A0A0A0
pub const TEXT_ACCENT: Color = DEEPSEEK_SKY;
pub const SELECTION_TEXT: Color = Color::White;
pub const TEXT_SOFT: Color = Color::Rgb(214, 223, 235); // #D6DFEB

// Compatibility aliases for existing call sites.
pub const TEXT_PRIMARY: Color = TEXT_BODY;
pub const TEXT_MUTED: Color = TEXT_SECONDARY;
pub const TEXT_DIM: Color = TEXT_HINT;

// New semantic colors for UI theming
pub const BORDER_COLOR: Color =
    Color::Rgb(BORDER_COLOR_RGB.0, BORDER_COLOR_RGB.1, BORDER_COLOR_RGB.2);
#[allow(dead_code)]
pub const ACCENT_PRIMARY: Color = DEEPSEEK_BLUE; // #3578E5
#[allow(dead_code)]
pub const ACCENT_SECONDARY: Color = TEXT_ACCENT; // #6AAEF2
#[allow(dead_code)]
pub const BACKGROUND_DARK: Color = Color::Rgb(13, 26, 48); // #0D1A30
#[allow(dead_code)]
pub const STATUS_NEUTRAL: Color = Color::Rgb(160, 160, 160); // #A0A0A0
#[allow(dead_code)]
pub const SURFACE_PANEL: Color = Color::Rgb(21, 33, 52); // #152134
#[allow(dead_code)]
pub const SURFACE_ELEVATED: Color = Color::Rgb(28, 42, 64); // #1C2A40
pub const SURFACE_REASONING: Color = Color::Rgb(54, 44, 26); // #362C1A
#[allow(dead_code)]
pub const SURFACE_REASONING_ACTIVE: Color = Color::Rgb(68, 53, 28); // #44351C
#[allow(dead_code)]
pub const SURFACE_TOOL: Color = Color::Rgb(24, 39, 60); // #18273C
#[allow(dead_code)]
pub const SURFACE_TOOL_ACTIVE: Color = Color::Rgb(29, 48, 73); // #1D3049
#[allow(dead_code)]
pub const SURFACE_SUCCESS: Color = Color::Rgb(22, 56, 63); // #16383F
#[allow(dead_code)]
pub const SURFACE_ERROR: Color = Color::Rgb(63, 27, 36); // #3F1B24
pub const DIFF_ADDED_BG: Color = Color::Rgb(18, 52, 38); // #123426 dark green tint
pub const DIFF_DELETED_BG: Color = Color::Rgb(52, 22, 28); // #34161C dark red tint
pub const DIFF_ADDED: Color = Color::Rgb(87, 199, 133); // #57C785
pub const ACCENT_REASONING_LIVE: Color = Color::Rgb(146, 198, 248); // #92C6F8
pub const ACCENT_TOOL_LIVE: Color = Color::Rgb(133, 184, 234); // #85B8EA
pub const ACCENT_TOOL_ISSUE: Color = Color::Rgb(192, 143, 153); // #C08F99
pub const TEXT_TOOL_OUTPUT: Color = Color::Rgb(205, 216, 228); // #CDD8E4

// Legacy status colors - keep for backward compatibility
pub const STATUS_SUCCESS: Color = DEEPSEEK_SKY;
pub const STATUS_WARNING: Color = Color::Rgb(255, 170, 60); // Amber
pub const STATUS_ERROR: Color = DEEPSEEK_RED;
#[allow(dead_code)]
pub const STATUS_INFO: Color = DEEPSEEK_BLUE;

// Mode-specific accent colors for mode badges
pub const MODE_AGENT: Color = Color::Rgb(80, 150, 255); // Bright blue
pub const MODE_YOLO: Color = Color::Rgb(255, 100, 100); // Warning red
pub const MODE_PLAN: Color = Color::Rgb(255, 170, 60); // Orange

pub const SELECTION_BG: Color = Color::Rgb(26, 44, 74);
#[allow(dead_code)]
pub const COMPOSER_BG: Color = DEEPSEEK_SLATE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    Dark,
    Light,
}

impl PaletteMode {
    /// Parse `COLORFGBG`, whose last numeric segment is the terminal
    /// background color. Values >= 8 conventionally indicate a light profile.
    #[must_use]
    pub fn from_colorfgbg(value: &str) -> Option<Self> {
        let bg = value
            .split(';')
            .rev()
            .find_map(|part| part.parse::<u16>().ok())?;
        Some(if bg >= 8 { Self::Light } else { Self::Dark })
    }

    /// Detect whether the terminal profile is light. Missing or unparsable
    /// values default to dark so existing terminal setups keep the tuned theme.
    #[must_use]
    pub fn detect() -> Self {
        std::env::var("COLORFGBG")
            .ok()
            .and_then(|value| Self::from_colorfgbg(&value))
            .unwrap_or(Self::Dark)
    }
}

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
    /// Statusline mode colors (agent/yolo/plan)
    pub mode_agent: Color,
    pub mode_yolo: Color,
    pub mode_plan: Color,
    /// Statusline status colors
    pub status_ready: Color,
    pub status_working: Color,
    pub status_warning: Color,
    /// Statusline text colors
    pub text_dim: Color,
    pub text_hint: Color,
    pub text_muted: Color,
    pub text_body: Color,
    pub text_soft: Color,
    pub border: Color,
}

pub const UI_THEME: UiTheme = UiTheme {
    name: "whale",
    mode: PaletteMode::Dark,
    surface_bg: DEEPSEEK_INK,
    panel_bg: DEEPSEEK_SLATE,
    elevated_bg: SURFACE_ELEVATED,
    composer_bg: DEEPSEEK_SLATE,
    selection_bg: SELECTION_BG,
    header_bg: DEEPSEEK_INK,
    footer_bg: DEEPSEEK_INK,
    mode_agent: MODE_AGENT,
    mode_yolo: MODE_YOLO,
    mode_plan: MODE_PLAN,
    status_ready: TEXT_MUTED,
    status_working: DEEPSEEK_SKY,
    status_warning: STATUS_WARNING,
    text_dim: TEXT_DIM,
    text_hint: TEXT_HINT,
    text_muted: TEXT_MUTED,
    text_body: TEXT_BODY,
    text_soft: TEXT_SOFT,
    border: BORDER_COLOR,
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
    mode_agent: DEEPSEEK_BLUE,
    mode_yolo: DEEPSEEK_RED,
    mode_plan: Color::Rgb(180, 83, 9),
    status_ready: LIGHT_TEXT_MUTED,
    status_working: DEEPSEEK_BLUE,
    status_warning: Color::Rgb(180, 83, 9),
    text_dim: LIGHT_TEXT_HINT,
    text_hint: LIGHT_TEXT_HINT,
    text_muted: LIGHT_TEXT_MUTED,
    text_body: LIGHT_TEXT_BODY,
    text_soft: LIGHT_TEXT_SOFT,
    border: LIGHT_BORDER,
};

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
    } else if color == ACCENT_REASONING_LIVE {
        Color::Rgb(146, 64, 14)
    } else if color == ACCENT_TOOL_ISSUE {
        Color::Rgb(159, 18, 57)
    } else if color == DIFF_ADDED {
        Color::Rgb(22, 101, 52)
    } else {
        color
    }
}

#[must_use]
pub fn adapt_bg_for_palette_mode(color: Color, mode: PaletteMode) -> Color {
    if mode == PaletteMode::Dark {
        return color;
    }

    if color == DEEPSEEK_INK || color == BACKGROUND_DARK {
        LIGHT_SURFACE
    } else if color == DEEPSEEK_SLATE
        || color == COMPOSER_BG
        || color == SURFACE_PANEL
        || color == SURFACE_TOOL
    {
        LIGHT_PANEL
    } else if color == SURFACE_ELEVATED || color == SURFACE_TOOL_ACTIVE {
        LIGHT_ELEVATED
    } else if color == SURFACE_REASONING || color == SURFACE_REASONING_ACTIVE {
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
    /// 16-color terminals (macOS Terminal.app default, dumb tmux setups).
    /// Background tints distort the named-palette mapping, so we drop them.
    Ansi16,
    /// 256-color terminals — RGB→256 fallback is faithful enough.
    Ansi256,
    /// True-color (24-bit) — render the palette verbatim.
    TrueColor,
}

impl ColorDepth {
    /// Detect the active terminal's color depth. Honors `COLORTERM`
    /// (truecolor / 24bit) first, then falls back to `TERM`. Defaults to
    /// `TrueColor` because most modern terminals support it; the conservative
    /// fallback is `Ansi16` so background tints disappear safely.
    #[must_use]
    pub fn detect() -> Self {
        if let Ok(ct) = std::env::var("COLORTERM") {
            let ct = ct.to_ascii_lowercase();
            if ct.contains("truecolor") || ct.contains("24bit") {
                return Self::TrueColor;
            }
        }
        if std::env::var_os("WT_SESSION").is_some() {
            return Self::TrueColor;
        }
        if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
            let term_program = term_program.to_ascii_lowercase();
            if term_program.contains("iterm")
                || term_program.contains("wezterm")
                || term_program.contains("vscode")
                || term_program.contains("warp")
            {
                return Self::TrueColor;
            }
        }
        let term = std::env::var("TERM").unwrap_or_default();
        let term = term.to_ascii_lowercase();
        if term.contains("truecolor") || term.contains("24bit") {
            Self::TrueColor
        } else if term.contains("256") {
            Self::Ansi256
        } else if term.is_empty() || term == "dumb" {
            Self::Ansi16
        } else {
            // Unknown TERM strings should not receive 24-bit SGR by default.
            // Older macOS/remote terminals can render truecolor backgrounds as
            // bright cyan blocks; 256-color output is the safer compromise.
            Self::Ansi256
        }
    }
}

/// Adapt a foreground color to the terminal's color depth.
///
/// On TrueColor, `color` passes through. On Ansi256 we let ratatui's renderer
/// down-convert (it does this already). On Ansi16 we strip RGB to a near
/// named color so semantic intent survives even on legacy terminals.
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

/// Adapt a background color. On Ansi16 terminals background tints are noisy,
/// so we drop them to `Color::Reset` rather than attempt a coarse named-color
/// match — a quiet background reads cleaner than a wrong one.
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

/// Mix two RGB colors at `alpha` (0.0 = `bg`, 1.0 = `fg`). Anything that's not
/// RGB falls back to `fg` — there's no meaningful alpha blend on a named
/// palette entry.
#[must_use]
pub fn blend(fg: Color, bg: Color, alpha: f32) -> Color {
    let alpha = alpha.clamp(0.0, 1.0);
    match (fg, bg) {
        (Color::Rgb(fr, fg_, fb), Color::Rgb(br, bg_, bb)) => {
            let mix = |a: u8, b: u8| -> u8 {
                let a = f32::from(a);
                let b = f32::from(b);
                (b + (a - b) * alpha).round().clamp(0.0, 255.0) as u8
            };
            Color::Rgb(mix(fr, br), mix(fg_, bg_), mix(fb, bb))
        }
        _ => fg,
    }
}

/// Return the reasoning surface color tinted at 12% over the app background.
/// This is the headline reasoning treatment in v0.6.6; a 12% blend keeps the
/// warm bias subtle without competing with body text. Returns `None` when the
/// terminal can't render the bg faithfully.
#[must_use]
pub fn reasoning_surface_tint(depth: ColorDepth) -> Option<Color> {
    match depth {
        ColorDepth::Ansi16 => None,
        _ => Some(adapt_bg(
            blend(SURFACE_REASONING, DEEPSEEK_INK, 0.12),
            depth,
        )),
    }
}

/// Pulse `color` between 30% and 100% brightness on a 2s cycle keyed off
/// `now_ms` (epoch ms). The minimum keeps the glyph readable at trough; the
/// maximum is the source color verbatim. Linear interpolation between them
/// reads as a slow heartbeat.
#[must_use]
pub fn pulse_brightness(color: Color, now_ms: u64) -> Color {
    // 2 s = 2000 ms full cycle; sin gives a smooth 0..1..0 swing.
    let phase = (now_ms % 2000) as f32 / 2000.0;
    let t = (phase * std::f32::consts::TAU).sin() * 0.5 + 0.5; // 0..1
    let alpha = 0.30 + t * 0.70; // 30%..100%
    match color {
        Color::Rgb(r, g, b) => {
            let s = |c: u8| -> u8 { ((f32::from(c)) * alpha).round().clamp(0.0, 255.0) as u8 };
            Color::Rgb(s(r), s(g), s(b))
        }
        other => other,
    }
}

/// Map an RGB triple to its closest ANSI-16 named color. Only used by
/// `adapt_color` on Ansi16 terminals; we lean on hue dominance + lightness so
/// brand colors land on the obviously-related named entry (sky → cyan, blue →
/// blue, red → red, etc.) rather than dithering around grey.
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
            if bright {
                Color::LightYellow
            } else {
                Color::Yellow
            }
        } else if b > r.saturating_sub(24) {
            if bright {
                Color::LightMagenta
            } else {
                Color::Magenta
            }
        } else if bright {
            Color::LightRed
        } else {
            Color::Red
        }
    } else if g >= r && g >= b {
        if b > r + 24 {
            if bright {
                Color::LightCyan
            } else {
                Color::Cyan
            }
        } else if bright {
            Color::LightGreen
        } else {
            Color::Green
        }
    } else if r.saturating_add(48) >= b && r > g + 24 {
        if bright {
            Color::LightMagenta
        } else {
            Color::Magenta
        }
    } else if g.saturating_add(48) >= b && g > r + 24 {
        if bright {
            Color::LightCyan
        } else {
            Color::Cyan
        }
    } else if bright {
        Color::LightBlue
    } else {
        Color::Blue
    }
}

/// Map an RGB color to the nearest xterm 256-color palette index. We use only
/// the stable 6x6x6 cube and grayscale ramp (16..255), not the terminal's
/// user-configurable 0..15 colors.
#[allow(dead_code)]
fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

    fn nearest_cube_level(channel: u8) -> usize {
        CUBE_LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, level)| channel.abs_diff(**level))
            .map(|(idx, _)| idx)
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
    use super::{
        ACCENT_REASONING_LIVE, ColorDepth, DEEPSEEK_INK, DEEPSEEK_RED, DEEPSEEK_SKY,
        DEEPSEEK_SLATE, LIGHT_PANEL, LIGHT_SURFACE, LIGHT_TEXT_BODY, LIGHT_TEXT_HINT,
        LIGHT_UI_THEME, PaletteMode, SURFACE_REASONING, TEXT_HINT, UI_THEME, adapt_bg,
        adapt_bg_for_palette_mode, adapt_color, adapt_fg_for_palette_mode, blend, nearest_ansi16,
        normalize_hex_rgb_color, parse_hex_rgb_color, pulse_brightness, reasoning_surface_tint,
        rgb_to_ansi256,
    };
    use ratatui::style::Color;

    #[test]
    fn palette_mode_parses_colorfgbg_background_slot() {
        assert_eq!(
            PaletteMode::from_colorfgbg("0;15"),
            Some(PaletteMode::Light)
        );
        assert_eq!(PaletteMode::from_colorfgbg("15;0"), Some(PaletteMode::Dark));
        assert_eq!(
            PaletteMode::from_colorfgbg("7;default;15"),
            Some(PaletteMode::Light)
        );
        assert_eq!(PaletteMode::from_colorfgbg("not-a-color"), None);
    }

    #[test]
    fn ui_theme_selects_light_variant() {
        let theme = super::UiTheme::for_mode(PaletteMode::Light);
        assert_eq!(theme, LIGHT_UI_THEME);
        assert_eq!(theme.surface_bg, LIGHT_SURFACE);
        assert_eq!(theme.text_body, LIGHT_TEXT_BODY);
    }

    #[test]
    fn ui_theme_applies_custom_background_to_base_surfaces() {
        let custom = Color::Rgb(26, 27, 38);
        let theme = super::UiTheme::for_mode(PaletteMode::Dark).with_background_color(custom);

        assert_eq!(theme.surface_bg, custom);
        assert_eq!(theme.header_bg, custom);
        assert_eq!(theme.footer_bg, custom);
        assert_eq!(
            theme.composer_bg, UI_THEME.composer_bg,
            "custom background must not erase panel contrast"
        );
    }

    #[test]
    fn hex_rgb_color_parser_accepts_hashless_and_normalizes() {
        assert_eq!(parse_hex_rgb_color("#1a1B26"), Some(Color::Rgb(26, 27, 38)));
        assert_eq!(parse_hex_rgb_color("1a1b26"), Some(Color::Rgb(26, 27, 38)));
        assert_eq!(
            normalize_hex_rgb_color("#1A1B26").as_deref(),
            Some("#1a1b26")
        );
        assert_eq!(parse_hex_rgb_color("#123"), None);
        assert_eq!(parse_hex_rgb_color("#zzzzzz"), None);
    }

    #[test]
    fn light_palette_maps_dark_surfaces_and_text() {
        assert_eq!(
            adapt_bg_for_palette_mode(DEEPSEEK_INK, PaletteMode::Light),
            LIGHT_SURFACE
        );
        assert_eq!(
            adapt_bg_for_palette_mode(DEEPSEEK_SLATE, PaletteMode::Light),
            LIGHT_PANEL
        );
        assert_eq!(
            adapt_fg_for_palette_mode(Color::White, LIGHT_SURFACE, PaletteMode::Light),
            LIGHT_TEXT_BODY
        );
        assert_eq!(
            adapt_fg_for_palette_mode(TEXT_HINT, LIGHT_SURFACE, PaletteMode::Light),
            LIGHT_TEXT_HINT
        );
    }

    #[test]
    fn adapt_color_passes_through_truecolor() {
        let c = Color::Rgb(53, 120, 229);
        assert_eq!(adapt_color(c, ColorDepth::TrueColor), c);
    }

    #[test]
    fn adapt_color_maps_rgb_to_indexed_on_ansi256() {
        let c = Color::Rgb(53, 120, 229);
        assert!(matches!(
            adapt_color(c, ColorDepth::Ansi256),
            Color::Indexed(_)
        ));
    }

    #[test]
    fn adapt_bg_maps_rgb_to_indexed_on_ansi256() {
        assert!(matches!(
            adapt_bg(SURFACE_REASONING, ColorDepth::Ansi256),
            Color::Indexed(_)
        ));
    }

    #[test]
    fn adapt_color_drops_to_named_on_ansi16() {
        // Sky: blue-dominant and bright → LightBlue, not terminal cyan.
        assert_eq!(
            adapt_color(DEEPSEEK_SKY, ColorDepth::Ansi16),
            Color::LightBlue
        );
        // Red: red-dominant, mid lum → Red (not the bright variant).
        assert_eq!(adapt_color(DEEPSEEK_RED, ColorDepth::Ansi16), Color::Red);
    }

    #[test]
    fn adapt_bg_disables_tints_on_ansi16() {
        assert_eq!(
            adapt_bg(SURFACE_REASONING, ColorDepth::Ansi16),
            Color::Reset
        );
        assert_eq!(
            adapt_bg(SURFACE_REASONING, ColorDepth::TrueColor),
            SURFACE_REASONING
        );
    }

    #[test]
    fn reasoning_tint_is_none_on_ansi16() {
        assert!(reasoning_surface_tint(ColorDepth::Ansi16).is_none());
        assert!(reasoning_surface_tint(ColorDepth::TrueColor).is_some());
        assert!(matches!(
            reasoning_surface_tint(ColorDepth::Ansi256),
            Some(Color::Indexed(_))
        ));
    }

    #[test]
    fn blend_at_zero_returns_bg_at_one_returns_fg() {
        let fg = Color::Rgb(200, 100, 50);
        let bg = Color::Rgb(0, 0, 0);
        assert_eq!(blend(fg, bg, 0.0), bg);
        assert_eq!(blend(fg, bg, 1.0), fg);
    }

    #[test]
    fn blend_at_half_is_midpoint() {
        let mid = blend(Color::Rgb(200, 100, 0), Color::Rgb(0, 0, 0), 0.5);
        assert_eq!(mid, Color::Rgb(100, 50, 0));
    }

    #[test]
    fn pulse_brightness_swings_within_envelope() {
        // The pulse rides between 30%..100% — never below 30% of the source.
        let src = ACCENT_REASONING_LIVE;
        let mut min_r = u8::MAX;
        let mut max_r = 0u8;
        for ms in (0u64..2000).step_by(50) {
            if let Color::Rgb(r, _, _) = pulse_brightness(src, ms) {
                min_r = min_r.min(r);
                max_r = max_r.max(r);
            }
        }
        let Color::Rgb(src_r, _, _) = src else {
            panic!("expected RGB");
        };
        // Trough should land near 30% of source; crest near source itself.
        let lower = (f32::from(src_r) * 0.30).round() as u8;
        assert!(min_r <= lower + 2, "trough too high: {min_r}");
        assert!(max_r + 2 >= src_r, "crest too low: {max_r}");
    }

    #[test]
    fn pulse_passes_named_colors_unchanged() {
        // Named palette entries don't blend meaningfully — leave them alone.
        assert_eq!(pulse_brightness(Color::Reset, 0), Color::Reset);
        assert_eq!(pulse_brightness(Color::Cyan, 1234), Color::Cyan);
    }

    #[test]
    fn nearest_ansi16_routes_known_brand_colors() {
        // Blue-dominant brand colors should stay blue rather than collapsing
        // to the user's terminal cyan, which is often much louder.
        assert_eq!(nearest_ansi16(53, 120, 229), Color::Blue);
        assert_eq!(nearest_ansi16(106, 174, 242), Color::LightBlue);
        assert_eq!(nearest_ansi16(42, 74, 127), Color::Blue);
        assert_eq!(nearest_ansi16(54, 187, 212), Color::LightCyan);
        assert_eq!(nearest_ansi16(226, 80, 96), Color::Red);
        assert_eq!(nearest_ansi16(11, 21, 38), Color::Black);
    }

    #[test]
    fn rgb_to_ansi256_uses_stable_extended_palette() {
        assert!(rgb_to_ansi256(53, 120, 229) >= 16);
        assert!(rgb_to_ansi256(11, 21, 38) >= 16);
    }

    #[test]
    fn color_depth_detect_is_safe_without_env() {
        // Don't try to pin the result — env may be anything in CI. Just
        // exercise the path so a panic would surface.
        let _ = ColorDepth::detect();
        let _ = adapt_color(DEEPSEEK_INK, ColorDepth::detect());
    }
}
