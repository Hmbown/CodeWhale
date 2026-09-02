//! Palette audit tests to prevent color drift.
//!
//! These tests ensure that deprecated colors are not used directly in
//! user-visible code. Backward-compatible DeepSeek aliases should point
//! at the current Codewhale semantic tokens instead of stale brand RGBs.

use ratatui::style::Color;

// Local stand-in for the binary crate's `src/test_support.rs`.
//
// `palette_audit` `#[path]`-includes `src/palette/mod.rs`, which transitively
// compiles `src/palette/user_theme.rs`. That module's inline tests call
// `crate::test_support::{lock_test_env, EnvVarGuard}`. In the binary crate
// those resolve to the real helpers; in this separate integration-test crate
// there is no such module, so this minimal shim provides the same surface and
// keeps the transitive tests serialized within this process.
#[allow(dead_code)]
mod test_support {
    use std::ffi::{OsStr, OsString};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(Mutex::default)
    }

    pub(super) struct TestEnvLock {
        _guard: MutexGuard<'static, ()>,
    }

    pub(super) fn lock_test_env() -> TestEnvLock {
        let guard = match lock().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        TestEnvLock { _guard: guard }
    }

    pub(super) struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        pub(super) fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: callers hold the process-wide test env mutex.
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: callers hold the process-wide test env mutex until drop.
            unsafe {
                if let Some(value) = self.previous.take() {
                    std::env::set_var(self.key, value);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }
}

fn color_to_rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::White => (255, 255, 255),
        Color::Gray => (128, 128, 128),
        Color::DarkGray => (169, 169, 169),
        Color::Red => (255, 0, 0),
        Color::LightRed => (255, 102, 102),
        Color::Green => (0, 255, 0),
        Color::LightGreen => (102, 255, 102),
        Color::Yellow => (255, 255, 0),
        Color::LightYellow => (255, 255, 153),
        Color::Blue => (0, 0, 255),
        Color::LightBlue => (102, 153, 255),
        Color::Magenta => (255, 0, 255),
        Color::LightMagenta => (255, 153, 255),
        Color::Cyan => (0, 255, 255),
        Color::LightCyan => (153, 255, 255),
        _ => panic!("unsupported color variant for contrast test: {color:?}"),
    }
}

fn linearize_srgb(component: u8) -> f64 {
    let srgb = f64::from(component) / 255.0;
    if srgb <= 0.04045 {
        srgb / 12.92
    } else {
        ((srgb + 0.055) / 1.055).powf(2.4)
    }
}

fn relative_luminance(color: Color) -> f64 {
    let (r, g, b) = color_to_rgb(color);
    0.2126 * linearize_srgb(r) + 0.7152 * linearize_srgb(g) + 0.0722 * linearize_srgb(b)
}

fn contrast_ratio(foreground: Color, background: Color) -> f64 {
    let fg = relative_luminance(foreground);
    let bg = relative_luminance(background);
    if fg >= bg {
        (fg + 0.05) / (bg + 0.05)
    } else {
        (bg + 0.05) / (fg + 0.05)
    }
}

fn assert_min_contrast(label: &str, foreground: Color, background: Color, min_ratio: f64) {
    let ratio = contrast_ratio(foreground, background);
    assert!(
        ratio >= min_ratio,
        "{label} contrast {ratio:.2} is below minimum {min_ratio:.2}"
    );
}

// NOTE: The deprecated color audit (DEEPSEEK_AQUA) was removed because
// the deprecated constant no longer exists in the palette.

#[test]
fn verify_status_success_uses_success_token() {
    assert_eq!(
        crate::palette::STATUS_SUCCESS,
        Color::Rgb(
            crate::palette::WHALE_SUCCESS_RGB.0,
            crate::palette::WHALE_SUCCESS_RGB.1,
            crate::palette::WHALE_SUCCESS_RGB.2
        ),
        "STATUS_SUCCESS should use the current success token"
    );
    assert_ne!(
        crate::palette::STATUS_SUCCESS,
        crate::palette::WHALE_ACTION,
        "STATUS_SUCCESS should not regress to the primary accent"
    );
}

#[test]
fn whale_roles_are_pinned_and_non_colliding() {
    assert_eq!(crate::palette::WHALE_BG_RGB, (7, 12, 29));
    assert_eq!(crate::palette::WHALE_PANEL_RGB, (16, 28, 64));
    assert_eq!(crate::palette::WHALE_ELEVATED_RGB, (26, 44, 99));
    assert_eq!(crate::palette::WHALE_ACTION_RGB, (106, 166, 220));
    assert_eq!(crate::palette::WHALE_ACCENT_SECONDARY_RGB, (79, 209, 197));
    assert_eq!(crate::palette::WHALE_HUMAN_RGB, (246, 196, 83));
    assert_eq!(crate::palette::WHALE_WARNING_RGB, (255, 122, 89));
    assert_eq!(crate::palette::WHALE_ERROR_RGB, (255, 134, 178));
    assert_eq!(crate::palette::WHALE_MODE_AGENT_RGB, (126, 180, 232));
    assert_eq!(crate::palette::WHALE_MODE_YOLO_RGB, (255, 112, 160));
    assert_eq!(crate::palette::WHALE_MODE_PLAN_RGB, (185, 220, 236));
    assert_eq!(crate::palette::WHALE_MODE_OPERATE_RGB, (173, 136, 255));
    assert_eq!(crate::palette::LIGHT_SUCCESS_FG_RGB, (20, 118, 61));
    assert_eq!(crate::palette::LIGHT_MODE_AGENT_RGB, (22, 54, 178));
    assert_eq!(crate::palette::LIGHT_MODE_PLAN_RGB, (52, 92, 128));
    assert_eq!(crate::palette::LIGHT_OPERATE_RGB, (112, 71, 184));
    assert_eq!(crate::palette::LIGHT_MODE_YOLO_RGB, (181, 35, 90));
    assert_eq!(
        crate::palette::LIGHT_USER_BODY,
        crate::palette::LIGHT_SUCCESS_FG
    );

    let ui = crate::palette::UI_THEME;
    assert_eq!(ui.accent_primary, crate::palette::WHALE_ACTION);
    assert_eq!(ui.info, crate::palette::WHALE_ACTION);
    assert_eq!(ui.status_working, crate::palette::WHALE_LIVE);
    assert_eq!(ui.accent_action, crate::palette::WHALE_HUMAN);
    assert_eq!(ui.warning, crate::palette::STATUS_WARNING);
    assert_eq!(ui.error_fg, crate::palette::WHALE_ERROR);
    assert_eq!(ui.mode_operate, crate::palette::MODE_OPERATE);
    assert_ne!(
        ui.mode_plan, ui.accent_action,
        "Plan is structural; Signal Gold is reserved for human attention"
    );
    assert_ne!(
        ui.status_working, ui.success,
        "live and done need separate ink"
    );
    assert_ne!(ui.accent_action, ui.warning, "human asks are not warnings");
    assert_ne!(
        ui.warning, ui.error_fg,
        "warning and danger must not collapse"
    );

    let foreground_domains = [
        ("action", crate::palette::WHALE_ACTION),
        ("live", crate::palette::WHALE_LIVE),
        ("human", crate::palette::WHALE_HUMAN),
        ("success", crate::palette::STATUS_SUCCESS),
        ("warning", crate::palette::STATUS_WARNING),
        ("danger", crate::palette::WHALE_ERROR),
        ("agent mode", crate::palette::MODE_AGENT),
        ("full-access mode", crate::palette::MODE_YOLO),
        ("plan mode", crate::palette::MODE_PLAN),
        ("operate mode", crate::palette::MODE_OPERATE),
        ("reasoning", crate::palette::TEXT_REASONING),
        ("diff added", crate::palette::DIFF_ADDED),
    ];
    for (index, (left_name, left)) in foreground_domains.iter().enumerate() {
        for (right_name, right) in foreground_domains.iter().skip(index + 1) {
            assert_ne!(
                left, right,
                "raw foreground adaptation domains '{left_name}' and '{right_name}' collide"
            );
        }
    }

    let background_domains = [
        ("base", crate::palette::WHALE_BG),
        ("panel", crate::palette::WHALE_PANEL),
        ("composer", crate::palette::WHALE_COMPOSER),
        ("elevated", crate::palette::SURFACE_ELEVATED),
        ("tool", crate::palette::SURFACE_TOOL),
        ("tool active", crate::palette::SURFACE_TOOL_ACTIVE),
        ("reasoning", crate::palette::SURFACE_REASONING),
        ("reasoning tint", crate::palette::SURFACE_REASONING_TINT),
        ("reasoning active", crate::palette::SURFACE_REASONING_ACTIVE),
        ("success", crate::palette::SURFACE_SUCCESS),
        ("error", crate::palette::SURFACE_ERROR),
        ("selection", crate::palette::SELECTION_BG),
        ("diff added", crate::palette::DIFF_ADDED_BG),
        ("diff deleted", crate::palette::DIFF_DELETED_BG),
    ];
    for (index, (left_name, left)) in background_domains.iter().enumerate() {
        for (right_name, right) in background_domains.iter().skip(index + 1) {
            assert_ne!(
                left, right,
                "raw background adaptation domains '{left_name}' and '{right_name}' collide"
            );
        }
    }
}

#[test]
fn contrast_guardrails_for_key_ui_pairs() {
    let min_readable = 4.5;

    assert_min_contrast(
        "TEXT_BODY on WHALE_BG",
        crate::palette::TEXT_BODY,
        crate::palette::WHALE_BG,
        min_readable,
    );
    assert_min_contrast(
        "TEXT_SECONDARY on WHALE_BG",
        crate::palette::TEXT_SECONDARY,
        crate::palette::WHALE_BG,
        min_readable,
    );
    assert_min_contrast(
        "TEXT_HINT on WHALE_BG",
        crate::palette::TEXT_HINT,
        crate::palette::WHALE_BG,
        min_readable,
    );
    assert_min_contrast(
        "STATUS_WARNING on WHALE_BG",
        crate::palette::STATUS_WARNING,
        crate::palette::WHALE_BG,
        min_readable,
    );
    assert_min_contrast(
        "STATUS_ERROR on WHALE_BG",
        crate::palette::STATUS_ERROR,
        crate::palette::WHALE_BG,
        min_readable,
    );
    assert_min_contrast(
        "SELECTION_TEXT on SELECTION_BG",
        crate::palette::SELECTION_TEXT,
        crate::palette::SELECTION_BG,
        min_readable,
    );
    assert_min_contrast(
        "TEXT_PRIMARY on SURFACE_ELEVATED",
        crate::palette::TEXT_PRIMARY,
        crate::palette::SURFACE_ELEVATED,
        min_readable,
    );
    for (label, foreground) in [
        ("action", crate::palette::UI_THEME.accent_primary),
        ("live", crate::palette::UI_THEME.status_working),
        ("human", crate::palette::UI_THEME.accent_action),
        ("warning", crate::palette::UI_THEME.warning),
        ("danger", crate::palette::UI_THEME.error_fg),
        ("act mode", crate::palette::UI_THEME.mode_agent),
        ("plan mode", crate::palette::UI_THEME.mode_plan),
        ("operate", crate::palette::UI_THEME.mode_operate),
        ("full-access mode", crate::palette::UI_THEME.mode_yolo),
        ("success", crate::palette::UI_THEME.success),
    ] {
        assert_min_contrast(
            label,
            foreground,
            crate::palette::SURFACE_ELEVATED,
            min_readable,
        );
    }
    let light_foregrounds = [
        ("body", crate::palette::LIGHT_UI_THEME.text_body),
        ("soft", crate::palette::LIGHT_UI_THEME.text_soft),
        ("muted", crate::palette::LIGHT_UI_THEME.text_muted),
        ("hint", crate::palette::LIGHT_UI_THEME.text_hint),
        ("action", crate::palette::LIGHT_UI_THEME.accent_primary),
        ("live", crate::palette::LIGHT_UI_THEME.status_working),
        ("human", crate::palette::LIGHT_UI_THEME.accent_action),
        ("warning", crate::palette::LIGHT_UI_THEME.warning),
        ("danger", crate::palette::LIGHT_UI_THEME.error_fg),
        ("act mode", crate::palette::LIGHT_UI_THEME.mode_agent),
        ("plan mode", crate::palette::LIGHT_UI_THEME.mode_plan),
        ("operate", crate::palette::LIGHT_UI_THEME.mode_operate),
        ("full-access mode", crate::palette::LIGHT_UI_THEME.mode_yolo),
        ("success", crate::palette::LIGHT_UI_THEME.success),
        ("user", crate::palette::LIGHT_USER_BODY),
    ];
    for (background_name, background) in [
        ("surface", crate::palette::LIGHT_SURFACE),
        ("panel", crate::palette::LIGHT_PANEL),
        ("raised", crate::palette::LIGHT_ELEVATED),
        ("selection", crate::palette::LIGHT_SELECTION_BG),
        ("reasoning", crate::palette::LIGHT_REASONING),
        ("success tint", crate::palette::LIGHT_SUCCESS),
        ("error tint", crate::palette::LIGHT_ERROR),
    ] {
        for (foreground_name, foreground) in light_foregrounds {
            assert_min_contrast(
                &format!("light {foreground_name} on {background_name}"),
                foreground,
                background,
                min_readable,
            );
        }
    }
    assert_min_contrast(
        "light user row on raised",
        crate::palette::LIGHT_USER_BODY,
        crate::palette::LIGHT_ELEVATED,
        min_readable,
    );
    assert_min_contrast(
        "light work-surface success hover on raised",
        crate::palette::LIGHT_UI_THEME.success,
        crate::palette::LIGHT_UI_THEME.elevated_bg,
        min_readable,
    );
}
