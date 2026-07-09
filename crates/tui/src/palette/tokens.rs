//! Color token constants — RGB tuples and semantic `Color` roles.

use ratatui::style::Color;

// v0.8.46 Whale dark palette — improved contrast and layer separation.
pub const WHALE_BG_RGB: (u8, u8, u8) = (10, 17, 32); // #0A1120 Deep Navy
pub const WHALE_PANEL_RGB: (u8, u8, u8) = (22, 34, 56); // #162238
pub const WHALE_ELEVATED_RGB: (u8, u8, u8) = (36, 52, 78); // #24344E
pub const WHALE_SELECTION_RGB: (u8, u8, u8) = (40, 56, 84); // #283854 — darker to avoid bright pop on deep navy
pub const WHALE_TEXT_BODY_RGB: (u8, u8, u8) = (246, 242, 232); // #F6F2E8 Whale Ivory
pub const WHALE_TEXT_SOFT_RGB: (u8, u8, u8) = (217, 224, 234); // #D9E0EA
pub const WHALE_TEXT_MUTED_RGB: (u8, u8, u8) = (169, 180, 199); // #A9B4C7 Mist Gray
pub const WHALE_TEXT_HINT_RGB: (u8, u8, u8) = (138, 150, 174); // #8A96AE
#[allow(dead_code)]
pub const WHALE_TEXT_DIM_RGB: (u8, u8, u8) = (118, 130, 156); // #76829C
pub const WHALE_ACCENT_PRIMARY_RGB: (u8, u8, u8) = (246, 196, 83); // #F6C453 Signal Gold
pub const WHALE_ACCENT_SECONDARY_RGB: (u8, u8, u8) = (79, 209, 197); // #4FD1C5 Seafoam
pub const WHALE_ACCENT_ACTION_RGB: (u8, u8, u8) = (255, 122, 89); // #FF7A59 Coral Spark
pub const WHALE_ERROR_RGB: (u8, u8, u8) = (255, 92, 122); // #FF5C7A Rose Red
pub const WHALE_ERROR_HOVER_RGB: (u8, u8, u8) = (255, 120, 144); // #FF7890 Rose Hover
pub const WHALE_ERROR_SURFACE_RGB: (u8, u8, u8) = (42, 18, 26); // #2A121A Error Surface
pub const WHALE_ERROR_BORDER_RGB: (u8, u8, u8) = (255, 138, 160); // #FF8AA0 Error Border
pub const WHALE_ERROR_TEXT_RGB: (u8, u8, u8) = (255, 214, 222); // #FFD6DE Error Text
pub const WHALE_WARNING_RGB: (u8, u8, u8) = (240, 160, 48); // #F0A030
pub const WHALE_SUCCESS_RGB: (u8, u8, u8) = (79, 209, 197); // #4FD1C5 Seafoam
pub const WHALE_INFO_RGB: (u8, u8, u8) = (106, 174, 242); // #6AAEF2 Sky
pub const WHALE_BORDER_RGB: (u8, u8, u8) = (52, 88, 145); // #345891
pub const WHALE_REASONING_TEXT_RGB: (u8, u8, u8) = (224, 153, 72); // #E09948
pub const WHALE_REASONING_SURFACE_RGB: (u8, u8, u8) = (42, 34, 24); // #2A2218
pub const WHALE_REASONING_TINT_RGB: (u8, u8, u8) = (24, 36, 52); // #182434

// Solarized Light palette RGB tuples
pub const SOLARIZED_BASE03_RGB: (u8, u8, u8) = (0x00, 0x2B, 0x36);
pub const SOLARIZED_BASE02_RGB: (u8, u8, u8) = (0x07, 0x36, 0x42);
pub const SOLARIZED_BASE01_RGB: (u8, u8, u8) = (0x58, 0x6E, 0x75);
pub const SOLARIZED_BASE00_RGB: (u8, u8, u8) = (0x65, 0x7B, 0x83);
pub const SOLARIZED_BASE0_RGB: (u8, u8, u8) = (0x83, 0x94, 0x96);
pub const SOLARIZED_BASE1_RGB: (u8, u8, u8) = (0x93, 0xA1, 0xA1);
#[allow(dead_code)]
pub const SOLARIZED_BASE2_RGB: (u8, u8, u8) = (0xEE, 0xE8, 0xD5);
pub const SOLARIZED_BASE3_RGB: (u8, u8, u8) = (0xFD, 0xF6, 0xE3);
pub const SOLARIZED_YELLOW_RGB: (u8, u8, u8) = (0xB5, 0x89, 0x00);
pub const SOLARIZED_ORANGE_RGB: (u8, u8, u8) = (0xCB, 0x4B, 0x16);
pub const SOLARIZED_RED_RGB: (u8, u8, u8) = (0xDC, 0x32, 0x2F);
pub const SOLARIZED_BLUE_RGB: (u8, u8, u8) = (0x26, 0x8B, 0xD2);
pub const SOLARIZED_CYAN_RGB: (u8, u8, u8) = (0x2A, 0xA1, 0x98);
pub const SOLARIZED_GREEN_RGB: (u8, u8, u8) = (0x85, 0x99, 0x00);
pub const SOLARIZED_PANEL_RGB: (u8, u8, u8) = (0xF0, 0xED, 0xE7);
pub const SOLARIZED_ELEVATED_RGB: (u8, u8, u8) = (0xE4, 0xDF, 0xCF);
pub const SOLARIZED_SELECT_RGB: (u8, u8, u8) = (0xD6, 0xD2, 0xC9);

pub const WHALE_DIFF_ADDED_RGB: (u8, u8, u8) = (87, 199, 133); // #57C785
#[allow(dead_code)]
pub const WHALE_DIFF_DELETED_RGB: (u8, u8, u8) = (255, 92, 122); // #FF5C7A Rose Red
pub const WHALE_DIFF_ADDED_BG_RGB: (u8, u8, u8) = (18, 42, 34); // #122A22
pub const WHALE_DIFF_DELETED_BG_RGB: (u8, u8, u8) = (42, 18, 26); // #2A121A
pub const WHALE_MODE_AGENT_RGB: (u8, u8, u8) = (80, 150, 255); // #5096FF
pub const WHALE_MODE_YOLO_RGB: (u8, u8, u8) = (255, 100, 100); // #FF6464
pub const WHALE_MODE_PLAN_RGB: (u8, u8, u8) = (246, 196, 83); // #F6C453 Signal Gold
pub const WHALE_MODE_OPERATE_RGB: (u8, u8, u8) = (178, 132, 255); // #B284FF
pub const WHALE_TOOL_LIVE_RGB: (u8, u8, u8) = (140, 190, 238); // #8CBEEE
pub const WHALE_TOOL_ISSUE_RGB: (u8, u8, u8) = (198, 150, 160); // #C696A0
pub const WHALE_TOOL_OUTPUT_RGB: (u8, u8, u8) = (194, 208, 224); // #C2D0E0
pub const WHALE_TOOL_SURFACE_RGB: (u8, u8, u8) = (28, 40, 62); // #1C283E
pub const WHALE_TOOL_ACTIVE_RGB: (u8, u8, u8) = (38, 54, 80); // #263650

pub const LIGHT_SURFACE_RGB: (u8, u8, u8) = (246, 248, 251); // #F6F8FB
pub const LIGHT_PANEL_RGB: (u8, u8, u8) = (236, 242, 248); // #ECF2F8
pub const LIGHT_ELEVATED_RGB: (u8, u8, u8) = (219, 229, 240); // #DBE5F0
pub const LIGHT_REASONING_RGB: (u8, u8, u8) = (255, 246, 214); // #FFF6D6
pub const LIGHT_SUCCESS_RGB: (u8, u8, u8) = (223, 247, 231); // #DFF7E7
pub const LIGHT_ERROR_RGB: (u8, u8, u8) = (254, 229, 229); // #FEE5E5
pub const LIGHT_TEXT_BODY_RGB: (u8, u8, u8) = (15, 23, 42); // #0F172A
pub const LIGHT_TEXT_MUTED_RGB: (u8, u8, u8) = (51, 65, 85); // #334155
pub const LIGHT_TEXT_HINT_RGB: (u8, u8, u8) = (100, 116, 139); // #64748B
pub const LIGHT_TEXT_SOFT_RGB: (u8, u8, u8) = (30, 41, 59); // #1E293B

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

pub const LIGHT_BORDER_RGB: (u8, u8, u8) = (139, 161, 184); // #8BA1B8
pub const LIGHT_SELECTION_RGB: (u8, u8, u8) = (207, 224, 247); // #CFE0F7
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
pub const MATRIX_TEXT_MUTED_RGB: (u8, u8, u8) = (0, 85, 0); // #005500
pub const MATRIX_TEXT_HINT_RGB: (u8, u8, u8) = (0, 102, 0); // #006600
pub const MATRIX_TEXT_SOFT_RGB: (u8, u8, u8) = (221, 255, 221); // #DDFFDD
pub const MATRIX_TEXT_DIM_RGB: (u8, u8, u8) = (0, 68, 0); // #004400
pub const MATRIX_BORDER_RGB: (u8, u8, u8) = (0, 204, 0); // #00CC00

// New semantic colors
pub const BORDER_COLOR_RGB: (u8, u8, u8) = WHALE_BORDER_RGB; // #2A4A7F

pub const WHALE_ACCENT_PRIMARY: Color = Color::Rgb(
    WHALE_ACCENT_PRIMARY_RGB.0,
    WHALE_ACCENT_PRIMARY_RGB.1,
    WHALE_ACCENT_PRIMARY_RGB.2,
);
pub const WHALE_INFO: Color = Color::Rgb(WHALE_INFO_RGB.0, WHALE_INFO_RGB.1, WHALE_INFO_RGB.2);
pub const WHALE_BG: Color = Color::Rgb(WHALE_BG_RGB.0, WHALE_BG_RGB.1, WHALE_BG_RGB.2);
pub const WHALE_PANEL: Color = Color::Rgb(WHALE_PANEL_RGB.0, WHALE_PANEL_RGB.1, WHALE_PANEL_RGB.2);
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
pub const LIGHT_USER_BODY: Color = Color::Rgb(21, 128, 61); // #15803D green

// New semantic colors for UI theming
pub const BORDER_COLOR: Color =
    Color::Rgb(BORDER_COLOR_RGB.0, BORDER_COLOR_RGB.1, BORDER_COLOR_RGB.2);
#[allow(dead_code)]
pub const ACCENT_PRIMARY: Color = Color::Rgb(
    WHALE_ACCENT_PRIMARY_RGB.0,
    WHALE_ACCENT_PRIMARY_RGB.1,
    WHALE_ACCENT_PRIMARY_RGB.2,
);
#[allow(dead_code)]
pub const ACCENT_SECONDARY: Color = Color::Rgb(
    WHALE_ACCENT_SECONDARY_RGB.0,
    WHALE_ACCENT_SECONDARY_RGB.1,
    WHALE_ACCENT_SECONDARY_RGB.2,
);
#[allow(dead_code)]
pub const BACKGROUND_DARK: Color = Color::Rgb(WHALE_BG_RGB.0, WHALE_BG_RGB.1, WHALE_BG_RGB.2);
#[allow(dead_code)]
pub const STATUS_NEUTRAL: Color = TEXT_MUTED;
#[allow(dead_code)]
pub const SURFACE_PANEL: Color =
    Color::Rgb(WHALE_PANEL_RGB.0, WHALE_PANEL_RGB.1, WHALE_PANEL_RGB.2);
#[allow(dead_code)]
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
#[allow(dead_code)]
pub const SURFACE_REASONING_ACTIVE: Color = Color::Rgb(58, 46, 32);
#[allow(dead_code)]
pub const SURFACE_TOOL: Color = Color::Rgb(
    WHALE_TOOL_SURFACE_RGB.0,
    WHALE_TOOL_SURFACE_RGB.1,
    WHALE_TOOL_SURFACE_RGB.2,
);
#[allow(dead_code)]
pub const SURFACE_TOOL_ACTIVE: Color = Color::Rgb(
    WHALE_TOOL_ACTIVE_RGB.0,
    WHALE_TOOL_ACTIVE_RGB.1,
    WHALE_TOOL_ACTIVE_RGB.2,
);
#[allow(dead_code)]
pub const SURFACE_SUCCESS: Color = Color::Rgb(18, 42, 37); // dark teal tint
#[allow(dead_code)]
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
#[allow(dead_code)]
pub const STATUS_INFO: Color = Color::Rgb(WHALE_INFO_RGB.0, WHALE_INFO_RGB.1, WHALE_INFO_RGB.2);

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
#[allow(dead_code)]
pub const COMPOSER_BG: Color = WHALE_PANEL;
