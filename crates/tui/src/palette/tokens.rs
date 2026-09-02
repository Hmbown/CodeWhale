//! Color token constants — RGB tuples and semantic `Color` roles.

use ratatui::style::Color;

// Codewhale Whale palette. Semantic colors own state, never decoration alone.
pub const WHALE_BG_RGB: (u8, u8, u8) = (7, 12, 29); // #070C1D Ink field
pub const WHALE_CHROME_RGB: (u8, u8, u8) = (12, 21, 49); // #0C1531 Navy chrome
pub const WHALE_PANEL_RGB: (u8, u8, u8) = (16, 28, 64); // #101C40 Panel surface
pub const WHALE_COMPOSER_RGB: (u8, u8, u8) = (20, 35, 82); // #142352 Stage plate (brand navy)
pub const WHALE_ELEVATED_RGB: (u8, u8, u8) = (26, 44, 99); // #1A2C63 Raised
pub const WHALE_SELECTION_RGB: (u8, u8, u8) = (30, 60, 143); // #1E3C8F Cobalt tint
pub const WHALE_TEXT_BODY_RGB: (u8, u8, u8) = (246, 242, 232); // #F6F2E8 Whale Ivory
pub const WHALE_TEXT_SOFT_RGB: (u8, u8, u8) = (182, 192, 212); // #B6C0D4
pub const WHALE_TEXT_MUTED_RGB: (u8, u8, u8) = (147, 160, 184); // #93A0B8
pub const WHALE_TEXT_HINT_RGB: (u8, u8, u8) = (138, 153, 179); // #8A99B3 — AA hint on the ink field
/// Blue Stage grammar: the interaction blue is the primary accent and the
/// Info ink; it has exactly one name. Signal Gold is reserved for the whale
/// mark and human-attention roles ([`WHALE_HUMAN_RGB`]), never for general
/// interaction.
pub const WHALE_ACTION_RGB: (u8, u8, u8) = (106, 166, 220); // #6AA6DC Ombre sky — owns interaction on dark
// No TUI consumer yet; the web reads these through scripts/export-design-tokens.py
// (`--whale-cobalt`, `--whale-ice`), so they are not dead.
#[expect(dead_code)]
pub const WHALE_COBALT_RGB: (u8, u8, u8) = (21, 53, 178); // #1535B2 Ombre cobalt — light-mode action
#[expect(dead_code)]
pub const WHALE_ICE_RGB: (u8, u8, u8) = (221, 238, 249); // #DDEEF9 Ice — structure on dark
pub const WHALE_CYAN_RGB: (u8, u8, u8) = (120, 188, 232); // #78BCE8 Cyan — bounded accents only
pub const WHALE_ACCENT_SECONDARY_RGB: (u8, u8, u8) = (79, 209, 197); // #4FD1C5 Seafoam
// Whale Teams (Signal Cut, CWC 2026-08-15) identity accents. These two brand
// palette entries have no semantic role in the TUI; they exist only so the
// Harbor and Echo whale marks carry their exact CWC accent instead of a
// borrowed state color. Never use them for status, mode, or permission.
pub const WHALE_BRAND_ORANGE_RGB: (u8, u8, u8) = (255, 138, 61); // #FF8A3D Harbor mooring loop
pub const WHALE_BRAND_MAGENTA_RGB: (u8, u8, u8) = (240, 78, 184); // #F04EB8 Echo sonar ticks
pub const WHALE_HUMAN_RGB: (u8, u8, u8) = (246, 196, 83); // #F6C453 Signal Gold
pub const WHALE_WORKING_GREEN_RGB: (u8, u8, u8) = (155, 214, 111); // #9BD66F Working Green
pub const WHALE_ERROR_RGB: (u8, u8, u8) = (255, 134, 178); // #FF86B2 Rose danger
pub const WHALE_ERROR_HOVER_RGB: (u8, u8, u8) = (255, 156, 194); // #FF9CC2
pub const WHALE_ERROR_SURFACE_RGB: (u8, u8, u8) = (43, 21, 34); // #2B1522
pub const WHALE_ERROR_BORDER_RGB: (u8, u8, u8) = WHALE_ERROR_RGB;
pub const WHALE_ERROR_TEXT_RGB: (u8, u8, u8) = (255, 219, 232); // #FFDBE8
pub const WHALE_WARNING_RGB: (u8, u8, u8) = (255, 122, 89); // #FF7A59 Coral warning
pub const WHALE_SUCCESS_RGB: (u8, u8, u8) = WHALE_WORKING_GREEN_RGB; // completed / verified
pub const WHALE_BORDER_RGB: (u8, u8, u8) = (42, 63, 114); // #2A3F72, sky at 25% on stage
pub const WHALE_REASONING_TEXT_RGB: (u8, u8, u8) = (224, 153, 72); // #E09948
pub const WHALE_REASONING_SURFACE_RGB: (u8, u8, u8) = (42, 34, 24); // #2A2218
pub const WHALE_REASONING_TINT_RGB: (u8, u8, u8) = (22, 36, 74); // #16244A

// Solarized Light palette RGB tuples
pub const SOLARIZED_BASE03_RGB: (u8, u8, u8) = (0x00, 0x2B, 0x36);
pub const SOLARIZED_BASE02_RGB: (u8, u8, u8) = (0x07, 0x36, 0x42);
pub const SOLARIZED_BASE01_RGB: (u8, u8, u8) = (0x52, 0x66, 0x6D); // lifted for 4.5:1 muted text
pub const SOLARIZED_BASE00_RGB: (u8, u8, u8) = (0x65, 0x7B, 0x83);
pub const SOLARIZED_BASE0_RGB: (u8, u8, u8) = (0x72, 0x81, 0x82); // lifted for 3:1 hint text
pub const SOLARIZED_BASE1_RGB: (u8, u8, u8) = (0x93, 0xA1, 0xA1);
#[expect(dead_code)]
pub const SOLARIZED_BASE2_RGB: (u8, u8, u8) = (0xEE, 0xE8, 0xD5);
pub const SOLARIZED_BASE3_RGB: (u8, u8, u8) = (0xFD, 0xF6, 0xE3);
pub const SOLARIZED_YELLOW_RGB: (u8, u8, u8) = (0xB4, 0x88, 0x00); // lifted for 3:1 on ivory
pub const SOLARIZED_ORANGE_RGB: (u8, u8, u8) = (0xCB, 0x4B, 0x16);
pub const SOLARIZED_RED_RGB: (u8, u8, u8) = (0xDC, 0x32, 0x2F);
pub const SOLARIZED_BLUE_RGB: (u8, u8, u8) = (0x26, 0x8B, 0xD2);
pub const SOLARIZED_CYAN_RGB: (u8, u8, u8) = (0x29, 0x9E, 0x96); // lifted for 3:1 on ivory
pub const SOLARIZED_GREEN_RGB: (u8, u8, u8) = (0x7F, 0x92, 0x00); // lifted for 3:1 on ivory/diff bg
pub const SOLARIZED_PANEL_RGB: (u8, u8, u8) = (0xF0, 0xED, 0xE7);
pub const SOLARIZED_ELEVATED_RGB: (u8, u8, u8) = (0xE4, 0xDF, 0xCF);
pub const SOLARIZED_SELECT_RGB: (u8, u8, u8) = (0xD6, 0xD2, 0xC9);

pub const WHALE_DIFF_ADDED_RGB: (u8, u8, u8) = (87, 199, 133); // #57C785
pub const WHALE_DIFF_ADDED_BG_RGB: (u8, u8, u8) = (18, 42, 34); // #122A22
// Raw colors that are remapped by equality must remain distinct across roles.
// These stay in the same perceptual families as action, danger, and human asks
// while preserving mode identity for Terminal and community themes.
pub const WHALE_DIFF_DELETED_BG_RGB: (u8, u8, u8) = (52, 24, 39); // #341827
pub const WHALE_MODE_AGENT_RGB: (u8, u8, u8) = (126, 180, 232); // #7EB4E8
pub const WHALE_MODE_YOLO_RGB: (u8, u8, u8) = (255, 112, 160); // #FF70A0
pub const WHALE_MODE_PLAN_RGB: (u8, u8, u8) = (185, 220, 236); // #B9DCEC Structural Ice
pub const WHALE_MODE_OPERATE_RGB: (u8, u8, u8) = (173, 136, 255); // #AD88FF
pub const WHALE_TOOL_LIVE_RGB: (u8, u8, u8) = WHALE_ACCENT_SECONDARY_RGB;
pub const WHALE_TOOL_ISSUE_RGB: (u8, u8, u8) = WHALE_ERROR_RGB;
pub const WHALE_TOOL_OUTPUT_RGB: (u8, u8, u8) = WHALE_TEXT_SOFT_RGB;
pub const WHALE_TOOL_SURFACE_RGB: (u8, u8, u8) = (15, 26, 58); // #0F1A3A
pub const WHALE_TOOL_ACTIVE_RGB: (u8, u8, u8) = (24, 44, 94); // #182C5E

pub const LIGHT_SURFACE_RGB: (u8, u8, u8) = (244, 247, 251); // #F4F7FB
pub const LIGHT_PANEL_RGB: (u8, u8, u8) = (255, 253, 248); // #FFFDF8
pub const LIGHT_ELEVATED_RGB: (u8, u8, u8) = (232, 238, 248); // #E8EEF8
pub const LIGHT_REASONING_RGB: (u8, u8, u8) = (255, 246, 214); // #FFF6D6
pub const LIGHT_SUCCESS_RGB: (u8, u8, u8) = (223, 247, 231); // #DFF7E7
pub const LIGHT_SUCCESS_FG_RGB: (u8, u8, u8) = (20, 118, 61); // #14763D, readable on every light surface
pub const LIGHT_ERROR_RGB: (u8, u8, u8) = (252, 235, 242); // #FCEBF2
pub const LIGHT_TEXT_BODY_RGB: (u8, u8, u8) = (20, 33, 58); // #14213A
pub const LIGHT_TEXT_MUTED_RGB: (u8, u8, u8) = (91, 103, 128); // #5B6780
pub const LIGHT_TEXT_HINT_RGB: (u8, u8, u8) = (95, 107, 129); // #5F6B81
pub const LIGHT_TEXT_SOFT_RGB: (u8, u8, u8) = (69, 81, 104); // #455168
pub const LIGHT_ACTION_RGB: (u8, u8, u8) = (21, 53, 178); // #1535B2 Ombre cobalt
pub const LIGHT_LIVE_RGB: (u8, u8, u8) = (8, 118, 109); // #08766D
pub const LIGHT_HUMAN_RGB: (u8, u8, u8) = (122, 85, 0); // #7A5500
pub const LIGHT_WARNING_RGB: (u8, u8, u8) = (169, 71, 36); // #A94724
pub const LIGHT_DANGER_RGB: (u8, u8, u8) = (180, 35, 90); // #B4235A
// Mode shades stay in their parent semantic families while remaining distinct
// inputs to the render backend. A `Cell` carries only a `Color`, so reusing the
// exact action/human/danger value here would erase the mode role before ANSI
// adaptation can preserve it.
pub const LIGHT_MODE_AGENT_RGB: (u8, u8, u8) = (22, 54, 178); // #1636B2
pub const LIGHT_MODE_YOLO_RGB: (u8, u8, u8) = (181, 35, 90); // #B5235A
pub const LIGHT_MODE_PLAN_RGB: (u8, u8, u8) = (52, 92, 128); // #345C80 Structural steel-blue
pub const LIGHT_OPERATE_RGB: (u8, u8, u8) = (112, 71, 184); // #7047B8

// Solarized Light palette colors
pub const SOLARIZED_TEXT_DIM: Color = Color::Rgb(
    SOLARIZED_BASE00_RGB.0,
    SOLARIZED_BASE00_RGB.1,
    SOLARIZED_BASE00_RGB.2,
);
pub const SOLARIZED_TEXT_HINT: Color = Color::Rgb(
    SOLARIZED_BASE0_RGB.0,
    SOLARIZED_BASE0_RGB.1,
    SOLARIZED_BASE0_RGB.2,
);
pub const SOLARIZED_TEXT_MUTED: Color = Color::Rgb(
    SOLARIZED_BASE01_RGB.0,
    SOLARIZED_BASE01_RGB.1,
    SOLARIZED_BASE01_RGB.2,
);
pub const SOLARIZED_TEXT_BODY: Color = Color::Rgb(
    SOLARIZED_BASE03_RGB.0,
    SOLARIZED_BASE03_RGB.1,
    SOLARIZED_BASE03_RGB.2,
);
pub const SOLARIZED_TEXT_SOFT: Color = Color::Rgb(
    SOLARIZED_BASE02_RGB.0,
    SOLARIZED_BASE02_RGB.1,
    SOLARIZED_BASE02_RGB.2,
);
pub const SOLARIZED_BORDER: Color = Color::Rgb(
    SOLARIZED_BASE1_RGB.0,
    SOLARIZED_BASE1_RGB.1,
    SOLARIZED_BASE1_RGB.2,
);
pub const SOLARIZED_BLUE: Color = Color::Rgb(
    SOLARIZED_BLUE_RGB.0,
    SOLARIZED_BLUE_RGB.1,
    SOLARIZED_BLUE_RGB.2,
);
pub const SOLARIZED_CYAN: Color = Color::Rgb(
    SOLARIZED_CYAN_RGB.0,
    SOLARIZED_CYAN_RGB.1,
    SOLARIZED_CYAN_RGB.2,
);
pub const SOLARIZED_RED: Color = Color::Rgb(
    SOLARIZED_RED_RGB.0,
    SOLARIZED_RED_RGB.1,
    SOLARIZED_RED_RGB.2,
);
pub const SOLARIZED_ORANGE: Color = Color::Rgb(
    SOLARIZED_ORANGE_RGB.0,
    SOLARIZED_ORANGE_RGB.1,
    SOLARIZED_ORANGE_RGB.2,
);
pub const SOLARIZED_YELLOW: Color = Color::Rgb(
    SOLARIZED_YELLOW_RGB.0,
    SOLARIZED_YELLOW_RGB.1,
    SOLARIZED_YELLOW_RGB.2,
);
pub const SOLARIZED_GREEN: Color = Color::Rgb(
    SOLARIZED_GREEN_RGB.0,
    SOLARIZED_GREEN_RGB.1,
    SOLARIZED_GREEN_RGB.2,
);
pub const SOLARIZED_SURFACE: Color = Color::Rgb(
    SOLARIZED_BASE3_RGB.0,
    SOLARIZED_BASE3_RGB.1,
    SOLARIZED_BASE3_RGB.2,
);
pub const SOLARIZED_PANEL: Color = Color::Rgb(
    SOLARIZED_PANEL_RGB.0,
    SOLARIZED_PANEL_RGB.1,
    SOLARIZED_PANEL_RGB.2,
);
pub const SOLARIZED_ELEVATED: Color = Color::Rgb(
    SOLARIZED_ELEVATED_RGB.0,
    SOLARIZED_ELEVATED_RGB.1,
    SOLARIZED_ELEVATED_RGB.2,
);
pub const SOLARIZED_SELECT_BG: Color = Color::Rgb(
    SOLARIZED_SELECT_RGB.0,
    SOLARIZED_SELECT_RGB.1,
    SOLARIZED_SELECT_RGB.2,
);
pub const SOLARIZED_DIFF_ADDED_BG: Color = Color::Rgb(0xEA, 0xF2, 0xE0);
pub const SOLARIZED_ERROR_SURFACE: Color = Color::Rgb(0xFD, 0xEE, 0xEB);
/// Same tone as the error surface; kept as a distinct alias for diff context.
pub const SOLARIZED_DIFF_DELETED_BG: Color = SOLARIZED_ERROR_SURFACE;
pub const SOLARIZED_ERROR_TEXT: Color = Color::Rgb(0x8B, 0x00, 0x00);
pub const SOLARIZED_ERROR_HOVER: Color = Color::Rgb(0xE0, 0x55, 0x52);
pub const SOLARIZED_COMPOSER: Color = Color::Rgb(
    SOLARIZED_PANEL_RGB.0,
    SOLARIZED_PANEL_RGB.1,
    SOLARIZED_PANEL_RGB.2,
);

pub const LIGHT_BORDER_RGB: (u8, u8, u8) = (169, 184, 207); // #A9B8CF
pub const LIGHT_SELECTION_RGB: (u8, u8, u8) = (238, 246, 255); // #EEF6FF
pub const GRAYSCALE_SURFACE_RGB: (u8, u8, u8) = (10, 10, 10); // #0A0A0A
pub const GRAYSCALE_PANEL_RGB: (u8, u8, u8) = (18, 18, 18); // #121212
pub const GRAYSCALE_ELEVATED_RGB: (u8, u8, u8) = (31, 31, 31); // #1F1F1F
pub const GRAYSCALE_REASONING_RGB: (u8, u8, u8) = (38, 38, 38); // #262626
pub const GRAYSCALE_SUCCESS_RGB: (u8, u8, u8) = (34, 34, 34); // #222222
pub const GRAYSCALE_ERROR_RGB: (u8, u8, u8) = (42, 42, 42); // #2A2A2A
pub const GRAYSCALE_TEXT_BODY_RGB: (u8, u8, u8) = (236, 236, 236); // #ECECEC
pub const GRAYSCALE_TEXT_MUTED_RGB: (u8, u8, u8) = (180, 180, 180); // #B4B4B4
pub const GRAYSCALE_TEXT_HINT_RGB: (u8, u8, u8) = (138, 138, 138); // #8A8A8A
pub const GRAYSCALE_TEXT_SOFT_RGB: (u8, u8, u8) = (220, 220, 220); // #DCDCDC
pub const GRAYSCALE_BORDER_RGB: (u8, u8, u8) = (96, 96, 96); // #606060
pub const GRAYSCALE_SELECTION_RGB: (u8, u8, u8) = (62, 62, 62); // #3E3E3E

pub const MATRIX_SURFACE_RGB: (u8, u8, u8) = (0, 10, 0); // #000A00
pub const MATRIX_ELEVATED_RGB: (u8, u8, u8) = (0, 51, 0); // #003300
pub const MATRIX_SELECTION_RGB: (u8, u8, u8) = (0, 51, 0); // #003300
pub const MATRIX_TEXT_BODY_RGB: (u8, u8, u8) = (136, 255, 136); // #88FF88
pub const MATRIX_TEXT_MUTED_RGB: (u8, u8, u8) = (0, 169, 0); // #00A900, lifted for 4.5:1
pub const MATRIX_TEXT_HINT_RGB: (u8, u8, u8) = (0, 135, 0); // #008700, lifted for 3:1
pub const MATRIX_TEXT_SOFT_RGB: (u8, u8, u8) = (221, 255, 221); // #DDFFDD
pub const MATRIX_TEXT_DIM_RGB: (u8, u8, u8) = (0, 108, 0); // #006C00, lifted for 3:1
pub const MATRIX_BORDER_RGB: (u8, u8, u8) = (0, 204, 0); // #00CC00

// Semantic colors
pub const BORDER_COLOR_RGB: (u8, u8, u8) = WHALE_BORDER_RGB;

pub const WHALE_ACTION: Color =
    Color::Rgb(WHALE_ACTION_RGB.0, WHALE_ACTION_RGB.1, WHALE_ACTION_RGB.2);
pub const WHALE_LIVE: Color = Color::Rgb(
    WHALE_ACCENT_SECONDARY_RGB.0,
    WHALE_ACCENT_SECONDARY_RGB.1,
    WHALE_ACCENT_SECONDARY_RGB.2,
);
pub const WHALE_HUMAN: Color = Color::Rgb(WHALE_HUMAN_RGB.0, WHALE_HUMAN_RGB.1, WHALE_HUMAN_RGB.2);
pub const WHALE_BG: Color = Color::Rgb(WHALE_BG_RGB.0, WHALE_BG_RGB.1, WHALE_BG_RGB.2);
pub const WHALE_CHROME: Color =
    Color::Rgb(WHALE_CHROME_RGB.0, WHALE_CHROME_RGB.1, WHALE_CHROME_RGB.2);
pub const WHALE_PANEL: Color = Color::Rgb(WHALE_PANEL_RGB.0, WHALE_PANEL_RGB.1, WHALE_PANEL_RGB.2);
pub const WHALE_COMPOSER: Color = Color::Rgb(
    WHALE_COMPOSER_RGB.0,
    WHALE_COMPOSER_RGB.1,
    WHALE_COMPOSER_RGB.2,
);
pub const WHALE_ERROR: Color = Color::Rgb(WHALE_ERROR_RGB.0, WHALE_ERROR_RGB.1, WHALE_ERROR_RGB.2);

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
pub const LIGHT_SUCCESS_FG: Color = Color::Rgb(
    LIGHT_SUCCESS_FG_RGB.0,
    LIGHT_SUCCESS_FG_RGB.1,
    LIGHT_SUCCESS_FG_RGB.2,
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
pub const LIGHT_ACTION: Color =
    Color::Rgb(LIGHT_ACTION_RGB.0, LIGHT_ACTION_RGB.1, LIGHT_ACTION_RGB.2);
pub const LIGHT_LIVE: Color = Color::Rgb(LIGHT_LIVE_RGB.0, LIGHT_LIVE_RGB.1, LIGHT_LIVE_RGB.2);
pub const LIGHT_HUMAN: Color = Color::Rgb(LIGHT_HUMAN_RGB.0, LIGHT_HUMAN_RGB.1, LIGHT_HUMAN_RGB.2);
pub const LIGHT_WARNING: Color = Color::Rgb(
    LIGHT_WARNING_RGB.0,
    LIGHT_WARNING_RGB.1,
    LIGHT_WARNING_RGB.2,
);
pub const LIGHT_DANGER: Color =
    Color::Rgb(LIGHT_DANGER_RGB.0, LIGHT_DANGER_RGB.1, LIGHT_DANGER_RGB.2);
pub const LIGHT_MODE_AGENT: Color = Color::Rgb(
    LIGHT_MODE_AGENT_RGB.0,
    LIGHT_MODE_AGENT_RGB.1,
    LIGHT_MODE_AGENT_RGB.2,
);
pub const LIGHT_MODE_YOLO: Color = Color::Rgb(
    LIGHT_MODE_YOLO_RGB.0,
    LIGHT_MODE_YOLO_RGB.1,
    LIGHT_MODE_YOLO_RGB.2,
);
pub const LIGHT_MODE_PLAN: Color = Color::Rgb(
    LIGHT_MODE_PLAN_RGB.0,
    LIGHT_MODE_PLAN_RGB.1,
    LIGHT_MODE_PLAN_RGB.2,
);
pub const LIGHT_OPERATE: Color = Color::Rgb(
    LIGHT_OPERATE_RGB.0,
    LIGHT_OPERATE_RGB.1,
    LIGHT_OPERATE_RGB.2,
);
pub const LIGHT_BORDER: Color =
    Color::Rgb(LIGHT_BORDER_RGB.0, LIGHT_BORDER_RGB.1, LIGHT_BORDER_RGB.2);
pub const LIGHT_SELECTION_BG: Color = Color::Rgb(
    LIGHT_SELECTION_RGB.0,
    LIGHT_SELECTION_RGB.1,
    LIGHT_SELECTION_RGB.2,
);
pub const GRAYSCALE_SURFACE: Color = Color::Rgb(
    GRAYSCALE_SURFACE_RGB.0,
    GRAYSCALE_SURFACE_RGB.1,
    GRAYSCALE_SURFACE_RGB.2,
);
pub const GRAYSCALE_PANEL: Color = Color::Rgb(
    GRAYSCALE_PANEL_RGB.0,
    GRAYSCALE_PANEL_RGB.1,
    GRAYSCALE_PANEL_RGB.2,
);
pub const GRAYSCALE_ELEVATED: Color = Color::Rgb(
    GRAYSCALE_ELEVATED_RGB.0,
    GRAYSCALE_ELEVATED_RGB.1,
    GRAYSCALE_ELEVATED_RGB.2,
);
pub const GRAYSCALE_REASONING: Color = Color::Rgb(
    GRAYSCALE_REASONING_RGB.0,
    GRAYSCALE_REASONING_RGB.1,
    GRAYSCALE_REASONING_RGB.2,
);
pub const GRAYSCALE_SUCCESS: Color = Color::Rgb(
    GRAYSCALE_SUCCESS_RGB.0,
    GRAYSCALE_SUCCESS_RGB.1,
    GRAYSCALE_SUCCESS_RGB.2,
);
pub const GRAYSCALE_ERROR: Color = Color::Rgb(
    GRAYSCALE_ERROR_RGB.0,
    GRAYSCALE_ERROR_RGB.1,
    GRAYSCALE_ERROR_RGB.2,
);
pub const GRAYSCALE_TEXT_BODY: Color = Color::Rgb(
    GRAYSCALE_TEXT_BODY_RGB.0,
    GRAYSCALE_TEXT_BODY_RGB.1,
    GRAYSCALE_TEXT_BODY_RGB.2,
);
pub const GRAYSCALE_TEXT_MUTED: Color = Color::Rgb(
    GRAYSCALE_TEXT_MUTED_RGB.0,
    GRAYSCALE_TEXT_MUTED_RGB.1,
    GRAYSCALE_TEXT_MUTED_RGB.2,
);
pub const GRAYSCALE_TEXT_HINT: Color = Color::Rgb(
    GRAYSCALE_TEXT_HINT_RGB.0,
    GRAYSCALE_TEXT_HINT_RGB.1,
    GRAYSCALE_TEXT_HINT_RGB.2,
);
pub const GRAYSCALE_TEXT_SOFT: Color = Color::Rgb(
    GRAYSCALE_TEXT_SOFT_RGB.0,
    GRAYSCALE_TEXT_SOFT_RGB.1,
    GRAYSCALE_TEXT_SOFT_RGB.2,
);
pub const GRAYSCALE_BORDER: Color = Color::Rgb(
    GRAYSCALE_BORDER_RGB.0,
    GRAYSCALE_BORDER_RGB.1,
    GRAYSCALE_BORDER_RGB.2,
);
pub const GRAYSCALE_SELECTION_BG: Color = Color::Rgb(
    GRAYSCALE_SELECTION_RGB.0,
    GRAYSCALE_SELECTION_RGB.1,
    GRAYSCALE_SELECTION_RGB.2,
);

pub const TEXT_BODY: Color = Color::Rgb(
    WHALE_TEXT_BODY_RGB.0,
    WHALE_TEXT_BODY_RGB.1,
    WHALE_TEXT_BODY_RGB.2,
);
pub const TEXT_SECONDARY: Color = Color::Rgb(
    WHALE_TEXT_MUTED_RGB.0,
    WHALE_TEXT_MUTED_RGB.1,
    WHALE_TEXT_MUTED_RGB.2,
);
pub const TEXT_HINT: Color = Color::Rgb(
    WHALE_TEXT_HINT_RGB.0,
    WHALE_TEXT_HINT_RGB.1,
    WHALE_TEXT_HINT_RGB.2,
);
pub const TEXT_ACCENT: Color = Color::Rgb(
    WHALE_ACCENT_SECONDARY_RGB.0,
    WHALE_ACCENT_SECONDARY_RGB.1,
    WHALE_ACCENT_SECONDARY_RGB.2,
);
pub const SELECTION_TEXT: Color = Color::Rgb(
    WHALE_TEXT_BODY_RGB.0,
    WHALE_TEXT_BODY_RGB.1,
    WHALE_TEXT_BODY_RGB.2,
); // Ivory — softer than pure white
pub const TEXT_SOFT: Color = Color::Rgb(
    WHALE_TEXT_SOFT_RGB.0,
    WHALE_TEXT_SOFT_RGB.1,
    WHALE_TEXT_SOFT_RGB.2,
);
pub const TEXT_REASONING: Color = Color::Rgb(
    WHALE_REASONING_TEXT_RGB.0,
    WHALE_REASONING_TEXT_RGB.1,
    WHALE_REASONING_TEXT_RGB.2,
);

// Compatibility aliases for existing call sites.
pub const TEXT_PRIMARY: Color = TEXT_BODY;
pub const TEXT_MUTED: Color = TEXT_SECONDARY;
pub const TEXT_DIM: Color = TEXT_HINT;
pub const USER_BODY: Color = Color::Rgb(74, 222, 128); // #4ADE80 green
pub const LIGHT_USER_BODY: Color = LIGHT_SUCCESS_FG;

// Compatibility semantic colors for UI theming
pub const BORDER_COLOR: Color =
    Color::Rgb(BORDER_COLOR_RGB.0, BORDER_COLOR_RGB.1, BORDER_COLOR_RGB.2);
pub const BACKGROUND_DARK: Color = Color::Rgb(WHALE_BG_RGB.0, WHALE_BG_RGB.1, WHALE_BG_RGB.2);
pub const SURFACE_PANEL: Color =
    Color::Rgb(WHALE_PANEL_RGB.0, WHALE_PANEL_RGB.1, WHALE_PANEL_RGB.2);
pub const SURFACE_ELEVATED: Color = Color::Rgb(
    WHALE_ELEVATED_RGB.0,
    WHALE_ELEVATED_RGB.1,
    WHALE_ELEVATED_RGB.2,
);
pub const SURFACE_REASONING: Color = Color::Rgb(
    WHALE_REASONING_SURFACE_RGB.0,
    WHALE_REASONING_SURFACE_RGB.1,
    WHALE_REASONING_SURFACE_RGB.2,
);
pub const SURFACE_REASONING_TINT: Color = Color::Rgb(
    WHALE_REASONING_TINT_RGB.0,
    WHALE_REASONING_TINT_RGB.1,
    WHALE_REASONING_TINT_RGB.2,
);
pub const SURFACE_REASONING_ACTIVE: Color = Color::Rgb(58, 46, 32);
pub const SURFACE_TOOL: Color = Color::Rgb(
    WHALE_TOOL_SURFACE_RGB.0,
    WHALE_TOOL_SURFACE_RGB.1,
    WHALE_TOOL_SURFACE_RGB.2,
);
pub const SURFACE_TOOL_ACTIVE: Color = Color::Rgb(
    WHALE_TOOL_ACTIVE_RGB.0,
    WHALE_TOOL_ACTIVE_RGB.1,
    WHALE_TOOL_ACTIVE_RGB.2,
);
pub const SURFACE_SUCCESS: Color = Color::Rgb(18, 42, 37); // dark teal tint
pub const SURFACE_ERROR: Color = Color::Rgb(
    WHALE_ERROR_SURFACE_RGB.0,
    WHALE_ERROR_SURFACE_RGB.1,
    WHALE_ERROR_SURFACE_RGB.2,
);
pub const DIFF_ADDED_BG: Color = Color::Rgb(
    WHALE_DIFF_ADDED_BG_RGB.0,
    WHALE_DIFF_ADDED_BG_RGB.1,
    WHALE_DIFF_ADDED_BG_RGB.2,
);
pub const DIFF_DELETED_BG: Color = Color::Rgb(
    WHALE_DIFF_DELETED_BG_RGB.0,
    WHALE_DIFF_DELETED_BG_RGB.1,
    WHALE_DIFF_DELETED_BG_RGB.2,
);
pub const DIFF_ADDED: Color = Color::Rgb(
    WHALE_DIFF_ADDED_RGB.0,
    WHALE_DIFF_ADDED_RGB.1,
    WHALE_DIFF_ADDED_RGB.2,
);
pub const ACCENT_REASONING_LIVE: Color = Color::Rgb(
    WHALE_REASONING_TEXT_RGB.0,
    WHALE_REASONING_TEXT_RGB.1,
    WHALE_REASONING_TEXT_RGB.2,
);
pub const ACCENT_TOOL_LIVE: Color = Color::Rgb(
    WHALE_TOOL_LIVE_RGB.0,
    WHALE_TOOL_LIVE_RGB.1,
    WHALE_TOOL_LIVE_RGB.2,
);
pub const ACCENT_TOOL_ISSUE: Color = Color::Rgb(
    WHALE_TOOL_ISSUE_RGB.0,
    WHALE_TOOL_ISSUE_RGB.1,
    WHALE_TOOL_ISSUE_RGB.2,
);
pub const TEXT_TOOL_OUTPUT: Color = Color::Rgb(
    WHALE_TOOL_OUTPUT_RGB.0,
    WHALE_TOOL_OUTPUT_RGB.1,
    WHALE_TOOL_OUTPUT_RGB.2,
);

// Legacy status colors - keep for backward compatibility
pub const STATUS_SUCCESS: Color = Color::Rgb(
    WHALE_SUCCESS_RGB.0,
    WHALE_SUCCESS_RGB.1,
    WHALE_SUCCESS_RGB.2,
);
pub const STATUS_WARNING: Color = Color::Rgb(
    WHALE_WARNING_RGB.0,
    WHALE_WARNING_RGB.1,
    WHALE_WARNING_RGB.2,
);
pub const STATUS_ERROR: Color = Color::Rgb(WHALE_ERROR_RGB.0, WHALE_ERROR_RGB.1, WHALE_ERROR_RGB.2);

// Mode-specific accent colors for mode badges
pub const MODE_AGENT: Color = Color::Rgb(
    WHALE_MODE_AGENT_RGB.0,
    WHALE_MODE_AGENT_RGB.1,
    WHALE_MODE_AGENT_RGB.2,
);
pub const MODE_YOLO: Color = Color::Rgb(
    WHALE_MODE_YOLO_RGB.0,
    WHALE_MODE_YOLO_RGB.1,
    WHALE_MODE_YOLO_RGB.2,
);
pub const MODE_PLAN: Color = Color::Rgb(
    WHALE_MODE_PLAN_RGB.0,
    WHALE_MODE_PLAN_RGB.1,
    WHALE_MODE_PLAN_RGB.2,
);
pub const MODE_OPERATE: Color = Color::Rgb(
    WHALE_MODE_OPERATE_RGB.0,
    WHALE_MODE_OPERATE_RGB.1,
    WHALE_MODE_OPERATE_RGB.2,
);

pub const SELECTION_BG: Color = Color::Rgb(
    WHALE_SELECTION_RGB.0,
    WHALE_SELECTION_RGB.1,
    WHALE_SELECTION_RGB.2,
);
pub const COMPOSER_BG: Color = WHALE_COMPOSER;
