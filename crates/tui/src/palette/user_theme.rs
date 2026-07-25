//! User-authored theme overlays loaded from the Codewhale-owned themes directory.

use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use ratatui::style::Color;
use serde::Deserialize;

use super::{ThemeId, UiTheme, parse_hex_rgb_color};

pub const USER_THEME_PREFIX: &str = "custom:";
pub const USER_THEME_SCHEMA: &str = include_str!("../assets/user-theme.schema.json");
const MAX_USER_THEME_BYTES: u64 = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UserThemeFile {
    schema_version: u8,
    base: String,
    colors: UserThemeColors,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct UserThemeColors {
    surface_bg: Option<String>,
    panel_bg: Option<String>,
    elevated_bg: Option<String>,
    composer_bg: Option<String>,
    selection_bg: Option<String>,
    header_bg: Option<String>,
    footer_bg: Option<String>,
    text_dim: Option<String>,
    text_hint: Option<String>,
    text_muted: Option<String>,
    text_body: Option<String>,
    text_soft: Option<String>,
    border: Option<String>,
    accent_primary: Option<String>,
    accent_secondary: Option<String>,
    accent_action: Option<String>,
    error_fg: Option<String>,
    error_hover: Option<String>,
    error_surface: Option<String>,
    error_border: Option<String>,
    error_text: Option<String>,
    warning: Option<String>,
    success: Option<String>,
    info: Option<String>,
    mode_agent: Option<String>,
    mode_yolo: Option<String>,
    mode_plan: Option<String>,
    mode_operate: Option<String>,
    permission_ask: Option<String>,
    permission_auto_review: Option<String>,
    permission_full_access: Option<String>,
    status_ready: Option<String>,
    status_working: Option<String>,
    status_warning: Option<String>,
    diff_added_fg: Option<String>,
    diff_deleted_fg: Option<String>,
    diff_added_bg: Option<String>,
    diff_deleted_bg: Option<String>,
    tool_running: Option<String>,
    tool_success: Option<String>,
    tool_failed: Option<String>,
}

#[must_use]
pub fn user_theme_schema_json() -> &'static str {
    USER_THEME_SCHEMA
}

pub fn normalize_user_theme_selector(value: &str) -> Result<Option<String>, String> {
    let trimmed = value.trim();
    let Some(slug) = trimmed.strip_prefix(USER_THEME_PREFIX) else {
        return Ok(None);
    };
    let slug = slug.trim().to_ascii_lowercase();
    if slug.is_empty()
        || slug.len() > 64
        || !slug
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err(
            "custom theme names must be 1-64 ASCII letters, digits, '-' or '_'".to_string(),
        );
    }
    Ok(Some(format!("{USER_THEME_PREFIX}{slug}")))
}

pub fn normalize_theme_setting(value: &str) -> Result<String, String> {
    if let Some(id) = ThemeId::from_name(value) {
        return Ok(id.name().to_string());
    }
    normalize_user_theme_selector(value)?.ok_or_else(|| {
        format!("invalid theme '{value}'; use a compiled theme name or custom:<name>")
    })
}

pub fn resolve_theme_setting(
    value: &str,
    background_color: Option<&str>,
) -> Result<(String, ThemeId, UiTheme), String> {
    let normalized = normalize_theme_setting(value)?;
    let (id, mut theme) = if let Some(resolved) = resolve_user_theme(&normalized)? {
        resolved
    } else {
        let id = ThemeId::from_name(&normalized)
            .ok_or_else(|| format!("invalid compiled theme '{normalized}'"))?;
        (id, id.ui_theme())
    };
    if let Some(value) = background_color {
        theme = theme.with_background_color(color("background_color", value)?);
    }
    Ok((normalized, id, theme))
}

pub fn resolve_user_theme(value: &str) -> Result<Option<(ThemeId, UiTheme)>, String> {
    let Some(selector) = normalize_user_theme_selector(value)? else {
        return Ok(None);
    };
    let slug = selector.trim_start_matches(USER_THEME_PREFIX);
    let themes_dir = user_themes_dir()?;
    reject_symlink_directory(&themes_dir)?;
    let path = themes_dir.join(format!("{slug}.json"));
    let mut file = open_theme_file(&path)?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect user theme {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "user theme {} must be a regular file",
            path.display()
        ));
    }
    if metadata.len() > MAX_USER_THEME_BYTES {
        return Err(format!(
            "user theme {} is too large ({} bytes; max {MAX_USER_THEME_BYTES})",
            path.display(),
            metadata.len()
        ));
    }
    let mut raw = String::with_capacity(metadata.len() as usize);
    file.read_to_string(&mut raw)
        .map_err(|error| format!("failed to read user theme {}: {error}", path.display()))?;
    let parsed: UserThemeFile = serde_json::from_str(&raw)
        .map_err(|error| format!("invalid user theme {}: {error}", path.display()))?;
    if parsed.schema_version != 1 {
        return Err(format!(
            "unsupported user theme schema_version {} in {}; expected 1",
            parsed.schema_version,
            path.display()
        ));
    }
    let base = ThemeId::from_name(&parsed.base).ok_or_else(|| {
        format!(
            "invalid base theme '{}' in {}; use a compiled theme name",
            parsed.base,
            path.display()
        )
    })?;
    let mut theme = base.ui_theme();
    apply_colors(&mut theme, &parsed.colors)?;
    Ok(Some((base, theme)))
}

pub fn user_themes_dir() -> Result<PathBuf, String> {
    codewhale_config::codewhale_home()
        .map(|home| home.join("themes"))
        .map_err(|error| format!("failed to resolve Codewhale themes directory: {error}"))
}

fn reject_symlink_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "failed to inspect themes directory {}: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "themes directory {} must be a real directory, not a symlink",
            path.display()
        ));
    }
    Ok(())
}

fn open_theme_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path).map_err(|error| {
        format!(
            "failed to open user theme {} safely: {error}",
            path.display()
        )
    })
}

fn color(name: &str, value: &str) -> Result<Color, String> {
    parse_hex_rgb_color(value)
        .ok_or_else(|| format!("user theme color '{name}' must be #RRGGBB, got '{value}'"))
}

fn apply_colors(theme: &mut UiTheme, colors: &UserThemeColors) -> Result<(), String> {
    macro_rules! apply {
        ($($field:ident),+ $(,)?) => {$({
            if let Some(value) = colors.$field.as_deref() {
                theme.$field = color(stringify!($field), value)?;
            }
        })+};
    }
    apply!(
        surface_bg,
        panel_bg,
        elevated_bg,
        composer_bg,
        selection_bg,
        header_bg,
        footer_bg,
        text_dim,
        text_hint,
        text_muted,
        text_body,
        text_soft,
        border,
        accent_primary,
        accent_secondary,
        accent_action,
        error_fg,
        error_hover,
        error_surface,
        error_border,
        error_text,
        warning,
        success,
        info,
        mode_agent,
        mode_yolo,
        mode_plan,
        mode_operate,
        permission_ask,
        permission_auto_review,
        permission_full_access,
        status_ready,
        status_working,
        status_warning,
        diff_added_fg,
        diff_deleted_fg,
        diff_added_bg,
        diff_deleted_bg,
        tool_running,
        tool_success,
        tool_failed,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::EnvVarGuard;

    #[test]
    fn selector_rejects_paths_and_accepts_bounded_slugs() {
        assert_eq!(
            normalize_user_theme_selector("custom:My_Theme").unwrap(),
            Some("custom:my_theme".to_string())
        );
        assert!(normalize_user_theme_selector("custom:../secret").is_err());
        assert!(normalize_user_theme_selector("custom:").is_err());
        assert_eq!(normalize_user_theme_selector("dark").unwrap(), None);
    }

    #[test]
    fn user_theme_loads_fixed_file_and_rejects_unknown_fields() {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let _home = EnvVarGuard::set("CODEWHALE_HOME", temp.path());
        let themes = temp.path().join("themes");
        fs::create_dir(&themes).unwrap();
        fs::write(
            themes.join("ocean.json"),
            r##"{"schema_version":1,"base":"dark","colors":{"accent_primary":"#123456"}}"##,
        )
        .unwrap();
        let (base, theme) = resolve_user_theme("custom:ocean").unwrap().unwrap();
        assert_eq!(base, ThemeId::Whale);
        assert_eq!(theme.accent_primary, Color::Rgb(0x12, 0x34, 0x56));

        fs::write(
            themes.join("bad.json"),
            r##"{"schema_version":1,"base":"dark","colors":{"mystery":"#123456"}}"##,
        )
        .unwrap();
        assert!(resolve_user_theme("custom:bad").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn user_theme_refuses_symlink_files() {
        use std::os::unix::fs::symlink;
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let _home = EnvVarGuard::set("CODEWHALE_HOME", temp.path());
        let themes = temp.path().join("themes");
        fs::create_dir(&themes).unwrap();
        let outside = temp.path().join("outside.json");
        fs::write(&outside, "{}").unwrap();
        symlink(&outside, themes.join("linked.json")).unwrap();
        assert!(resolve_user_theme("custom:linked").is_err());
    }
}
