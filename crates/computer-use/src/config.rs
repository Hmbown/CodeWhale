//! Server configuration: `~/.codewhale/computer-use.toml` plus CLI overrides.
//!
//! Plugin-spawned children run with a scrubbed environment
//! (`crates/tui/src/child_env.rs` keeps PATH/HOME and little else), so the
//! file is the durable way to pick a device target; environment variables are
//! honored only as a convenience when they happen to be present.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::consent::Policy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Target {
    #[default]
    Auto,
    Desktop,
    Android,
    Harmony,
}

impl Target {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Ok(Self::Auto),
            "desktop" | "host" | "native" | "macos" | "windows" | "linux" => Ok(Self::Desktop),
            "android" | "adb" => Ok(Self::Android),
            "harmony" | "harmonyos" | "ohos" | "openharmony" | "hdc" => Ok(Self::Harmony),
            other => Err(format!(
                "unknown target `{other}`; expected auto, desktop, android, or harmony"
            )),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Desktop => "desktop",
            Self::Android => "android",
            Self::Harmony => "harmony",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Screenshots and input.
    #[default]
    Act,
    /// Screenshots, zoom, info, UI tree, devices only.
    Observe,
}

impl Mode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "act" | "" | "full" => Ok(Self::Act),
            "observe" | "read-only" | "readonly" | "view" => Ok(Self::Observe),
            other => Err(format!("unknown mode `{other}`; expected act or observe")),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Act => "act",
            Self::Observe => "observe",
        }
    }
}

pub const DEFAULT_MAX_EDGE: u32 = 1024;
pub const MIN_MAX_EDGE: u32 = 256;
pub const MAX_MAX_EDGE: u32 = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub target: Target,
    pub mode: Mode,
    /// Longest edge of screenshots sent to the model, in pixels.
    pub max_edge: u32,
    pub grid_default: bool,
    pub screenshot_after_action: bool,
    /// Settle delay before the post-action screenshot (desktop / device).
    pub settle_ms_desktop: u64,
    pub settle_ms_device: u64,
    pub android: AndroidConfig,
    pub harmony: HarmonyConfig,
    pub linux: LinuxConfig,
    /// Per-app consent (phase 2).
    pub apps: Policy,
    /// Where this config came from, for approval messages.
    pub config_hint: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AndroidConfig {
    /// `adb -s <serial>`; empty = the only connected device.
    pub serial: String,
    /// Explicit adb path; empty = search PATH and SDK locations.
    pub adb: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HarmonyConfig {
    /// `hdc -t <key>`; empty = the only connected target.
    pub target: String,
    pub hdc: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LinuxConfig {
    /// X11 display used when the environment does not provide one.
    pub display: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            target: Target::Auto,
            mode: Mode::Act,
            max_edge: DEFAULT_MAX_EDGE,
            grid_default: false,
            screenshot_after_action: true,
            settle_ms_desktop: 300,
            settle_ms_device: 700,
            android: AndroidConfig::default(),
            harmony: HarmonyConfig::default(),
            linux: LinuxConfig::default(),
            apps: Policy::default(),
            config_hint: None,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    target: Option<String>,
    mode: Option<String>,
    max_edge: Option<u32>,
    grid: Option<bool>,
    screenshot_after_action: Option<bool>,
    settle_ms_desktop: Option<u64>,
    settle_ms_device: Option<u64>,
    #[serde(default)]
    android: FileAndroid,
    #[serde(default)]
    harmony: FileHarmony,
    #[serde(default)]
    linux: FileLinux,
    #[serde(default)]
    apps: FileApps,
}

/// `[apps]` consent lists: bundle ids, package names, or app names.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileApps {
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileAndroid {
    serial: Option<String>,
    adb: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileHarmony {
    target: Option<String>,
    hdc: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileLinux {
    display: Option<String>,
}

/// Where the config file lives: `$CODEWHALE_HOME/computer-use.toml` or
/// `~/.codewhale/computer-use.toml`.
pub fn default_config_path() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("CODEWHALE_HOME")
        && !home.trim().is_empty()
    {
        return Some(PathBuf::from(home).join("computer-use.toml"));
    }
    home_dir().map(|home| home.join(".codewhale").join("computer-use.toml"))
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

impl Config {
    /// Defaults, then the file (if present), then environment hints.
    pub fn load(path: Option<&Path>) -> Result<Self, String> {
        let mut cfg = Self::default();
        let path = match path {
            Some(p) => Some(p.to_path_buf()),
            None => default_config_path(),
        };
        cfg.config_hint = path.clone().or_else(default_config_path);
        if let Some(path) = path
            && path.is_file()
        {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
            cfg.apply_toml(&text)
                .map_err(|e| format!("invalid {}: {e}", path.display()))?;
        }
        cfg.apply_env();
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn apply_toml(&mut self, text: &str) -> Result<(), String> {
        let file: FileConfig = toml::from_str(text).map_err(|e| e.to_string())?;
        if let Some(target) = file.target {
            self.target = Target::parse(&target)?;
        }
        if let Some(mode) = file.mode {
            self.mode = Mode::parse(&mode)?;
        }
        if let Some(max_edge) = file.max_edge {
            self.max_edge = max_edge;
        }
        if let Some(grid) = file.grid {
            self.grid_default = grid;
        }
        if let Some(v) = file.screenshot_after_action {
            self.screenshot_after_action = v;
        }
        if let Some(v) = file.settle_ms_desktop {
            self.settle_ms_desktop = v;
        }
        if let Some(v) = file.settle_ms_device {
            self.settle_ms_device = v;
        }
        if let Some(v) = file.android.serial {
            self.android.serial = v;
        }
        if let Some(v) = file.android.adb {
            self.android.adb = v;
        }
        if let Some(v) = file.harmony.target {
            self.harmony.target = v;
        }
        if let Some(v) = file.harmony.hdc {
            self.harmony.hdc = v;
        }
        if let Some(v) = file.linux.display {
            self.linux.display = v;
        }
        if !file.apps.allow.is_empty() {
            self.apps.allow = file.apps.allow;
        }
        if !file.apps.deny.is_empty() {
            self.apps.deny = file.apps.deny;
        }
        Ok(())
    }

    /// Environment hints (only consulted when the value is present).
    fn apply_env(&mut self) {
        if self.android.serial.is_empty()
            && let Ok(serial) = std::env::var("ANDROID_SERIAL")
            && !serial.trim().is_empty()
        {
            self.android.serial = serial.trim().to_string();
        }
        if let Ok(mode) = std::env::var("CODEWHALE_COMPUTER_USE_MODE")
            && let Ok(mode) = Mode::parse(&mode)
        {
            self.mode = mode;
        }
        if let Ok(target) = std::env::var("CODEWHALE_COMPUTER_USE_TARGET")
            && let Ok(target) = Target::parse(&target)
        {
            self.target = target;
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(MIN_MAX_EDGE..=MAX_MAX_EDGE).contains(&self.max_edge) {
            return Err(format!(
                "max_edge must be between {MIN_MAX_EDGE} and {MAX_MAX_EDGE}, got {}",
                self.max_edge
            ));
        }
        if self.settle_ms_desktop > 10_000 || self.settle_ms_device > 10_000 {
            return Err("settle delays must be at most 10000 ms".to_string());
        }
        Ok(())
    }

    /// Display path for approval messages.
    pub fn path_hint(&self) -> String {
        self.config_hint
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~/.codewhale/computer-use.toml".to_string())
    }

    /// Resolve `auto` using compile-time target and configured devices.
    pub fn effective_target(&self) -> Target {
        match self.target {
            Target::Auto => {
                if cfg!(target_os = "android") {
                    Target::Android
                } else if cfg!(target_env = "ohos") {
                    Target::Harmony
                } else if !self.android.serial.is_empty() {
                    Target::Android
                } else if !self.harmony.target.is_empty() {
                    Target::Harmony
                } else {
                    Target::Desktop
                }
            }
            explicit => explicit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = Config::default();
        assert_eq!(cfg.max_edge, DEFAULT_MAX_EDGE);
        assert_eq!(cfg.mode, Mode::Act);
        assert!(cfg.screenshot_after_action);
        cfg.validate().unwrap();
    }

    #[test]
    fn toml_overrides_apply() {
        let mut cfg = Config::default();
        cfg.apply_toml(
            r#"
target = "android"
mode = "observe"
max_edge = 800
grid = true

[android]
serial = "emulator-5554"

[apps]
allow = ["com.apple.Notes"]
deny = ["SketchyApp"]
"#,
        )
        .unwrap();
        assert_eq!(cfg.target, Target::Android);
        assert_eq!(cfg.mode, Mode::Observe);
        assert_eq!(cfg.max_edge, 800);
        assert!(cfg.grid_default);
        assert_eq!(cfg.android.serial, "emulator-5554");
        assert_eq!(cfg.apps.allow, vec!["com.apple.Notes"]);
        assert_eq!(cfg.apps.deny, vec!["SketchyApp"]);
        assert_eq!(cfg.effective_target(), Target::Android);
    }

    #[test]
    fn unknown_keys_and_bad_values_are_errors() {
        let mut cfg = Config::default();
        assert!(cfg.apply_toml("bogus = 1").is_err());
        assert!(cfg.apply_toml("target = \"ios\"").is_err());
        cfg.max_edge = 10;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn auto_target_prefers_configured_device() {
        let mut cfg = Config::default();
        cfg.harmony.target = "abc".into();
        if !cfg!(any(target_os = "android", target_env = "ohos")) {
            assert_eq!(cfg.effective_target(), Target::Harmony);
        }
    }
}
