//! Linux desktop driver (also HarmonyOS PC with a glibc userspace and any
//! other X11/Wayland host). Everything is process-based: no X11/Wayland
//! client libraries, so the crate keeps building on every workspace target.
//!
//! X11: `xdotool` for input, one of `scrot`/`maim`/`import`/`gnome-screenshot`/
//! `spectacle` for capture. Wayland: `grim` (or `spectacle`/`gnome-screenshot`)
//! for capture, `ydotool` for input (`wtype` for text when present).

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::LinuxConfig;
use crate::driver::{
    AppAction, Button, Driver, DriverError, Point, RawFrame, ScrollDir, TargetInfo, TargetKind,
    UiNode,
};
use crate::keys::{Key, KeyCombo, NamedKey};
use crate::process;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionKind {
    X11,
    Wayland,
}

pub struct LinuxDriver {
    session: SessionKind,
    env: Vec<(String, String)>,
    screenshot_tool: Option<(PathBuf, &'static str)>,
    xdotool: Option<PathBuf>,
    ydotool: Option<PathBuf>,
    wtype: Option<PathBuf>,
    size: Option<(u32, u32)>,
    missing: Vec<String>,
}

fn runtime_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR")
        && !dir.trim().is_empty()
    {
        return Some(PathBuf::from(dir.trim()));
    }
    let home = process::home()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = std::fs::metadata(&home).ok()?.uid();
        let dir = PathBuf::from(format!("/run/user/{uid}"));
        if dir.is_dir() {
            return Some(dir);
        }
    }
    let _ = home;
    None
}

fn wayland_socket(runtime: &PathBuf) -> Option<String> {
    let entries = std::fs::read_dir(runtime).ok()?;
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("wayland-") && !n.ends_with(".lock"))
        .collect();
    names.sort();
    names.into_iter().next()
}

impl LinuxDriver {
    pub fn new(cfg: &LinuxConfig) -> Result<Self, DriverError> {
        let mut env: Vec<(String, String)> = Vec::new();
        let runtime = runtime_dir();
        if let Some(dir) = &runtime {
            env.push(("XDG_RUNTIME_DIR".into(), dir.to_string_lossy().into_owned()));
        }
        let wayland_display = std::env::var("WAYLAND_DISPLAY")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| runtime.as_ref().and_then(wayland_socket));
        let x_display = std::env::var("DISPLAY")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| (!cfg.display.trim().is_empty()).then(|| cfg.display.trim().to_string()))
            .unwrap_or_else(|| ":0".to_string());
        let session_type = std::env::var("XDG_SESSION_TYPE")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let session = if session_type == "x11" {
            SessionKind::X11
        } else if session_type == "wayland" || wayland_display.is_some() {
            SessionKind::Wayland
        } else {
            SessionKind::X11
        };
        env.push(("DISPLAY".into(), x_display));
        if let Some(wd) = &wayland_display {
            env.push(("WAYLAND_DISPLAY".into(), wd.clone()));
        }
        let mut missing = Vec::new();
        let xdotool = process::which("xdotool");
        let ydotool = process::which("ydotool");
        let wtype = process::which("wtype");
        let screenshot_candidates: &[(&str, &'static str)] = match session {
            SessionKind::Wayland => &[
                ("grim", "grim"),
                ("spectacle", "spectacle"),
                ("gnome-screenshot", "gnome-screenshot"),
                ("scrot", "scrot"),
                ("maim", "maim"),
                ("import", "import"),
            ],
            SessionKind::X11 => &[
                ("scrot", "scrot"),
                ("maim", "maim"),
                ("import", "import"),
                ("gnome-screenshot", "gnome-screenshot"),
                ("spectacle", "spectacle"),
                ("grim", "grim"),
            ],
        };
        let screenshot_tool = screenshot_candidates
            .iter()
            .find_map(|(bin, kind)| process::which(bin).map(|p| (p, *kind)));
        if screenshot_tool.is_none() {
            missing.push(match session {
                SessionKind::Wayland => {
                    "screenshot tool (install grim, spectacle, or gnome-screenshot)".to_string()
                }
                SessionKind::X11 => {
                    "screenshot tool (install scrot, maim, or ImageMagick `import`)".to_string()
                }
            });
        }
        match session {
            SessionKind::X11 if xdotool.is_none() => missing.push("xdotool (input)".to_string()),
            SessionKind::Wayland if ydotool.is_none() && xdotool.is_none() => {
                missing.push(
                    "ydotool (input; needs ydotoold running) or xdotool for XWayland apps"
                        .to_string(),
                );
            }
            _ => {}
        }
        Ok(Self {
            session,
            env,
            screenshot_tool,
            xdotool,
            ydotool,
            wtype,
            size: None,
            missing,
        })
    }

    fn env_refs(&self) -> Vec<(&str, &str)> {
        self.env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }

    fn run(&self, program: &Path, args: &[&str]) -> Result<process::Output, DriverError> {
        let out = process::run_with_env(program, args, &self.env_refs(), Duration::from_secs(30))?;
        if out.success() {
            Ok(out)
        } else {
            Err(DriverError::Failed(format!(
                "{} {} failed: {}",
                program
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("command"),
                args.join(" "),
                process::tail(&out.stderr, 300)
            )))
        }
    }

    fn require_missing(&self) -> Result<(), DriverError> {
        if self.missing.is_empty() {
            Ok(())
        } else {
            Err(DriverError::Unavailable(format!(
                "missing helpers: {}",
                self.missing.join("; ")
            )))
        }
    }

    fn input_tool(&self) -> Result<InputTool<'_>, DriverError> {
        match self.session {
            SessionKind::X11 => self.xdotool.as_ref().map(InputTool::Xdotool),
            SessionKind::Wayland => self
                .ydotool
                .as_ref()
                .map(InputTool::Ydotool)
                .or_else(|| self.xdotool.as_ref().map(InputTool::Xdotool)),
        }
        .ok_or_else(|| {
            DriverError::Unavailable(
                "no input tool: install xdotool (X11) or ydotool (Wayland)".to_string(),
            )
        })
    }

    fn screen_size(&mut self) -> Result<(u32, u32), DriverError> {
        if let Some(size) = self.size {
            return Ok(size);
        }
        let frame = self.screenshot()?;
        let img = crate::frame::decode(&frame.bytes).map_err(DriverError::Failed)?;
        use image::GenericImageView;
        self.size = Some(img.dimensions());
        Ok(img.dimensions())
    }

    fn mouse_button_x(button: Button) -> &'static str {
        match button {
            Button::Left => "1",
            Button::Middle => "2",
            Button::Right => "3",
        }
    }

    fn mouse_button_y(button: Button) -> &'static str {
        // ydotool click codes: 0x40 down, 0x80 up, low bits button.
        match button {
            Button::Left => "0xC0",
            Button::Right => "0xC1",
            Button::Middle => "0xC2",
        }
    }
}

enum InputTool<'a> {
    Xdotool(&'a PathBuf),
    Ydotool(&'a PathBuf),
}

fn xdotool_keysym(key: &Key) -> Result<String, DriverError> {
    Ok(match key {
        Key::Named(named) => match named {
            NamedKey::Enter => "Return",
            NamedKey::Tab => "Tab",
            NamedKey::Escape => "Escape",
            NamedKey::Backspace => "BackSpace",
            NamedKey::Delete => "Delete",
            NamedKey::Space => "space",
            NamedKey::Up => "Up",
            NamedKey::Down => "Down",
            NamedKey::Left => "Left",
            NamedKey::Right => "Right",
            NamedKey::Home => "Home",
            NamedKey::End => "End",
            NamedKey::PageUp => "Prior",
            NamedKey::PageDown => "Next",
            NamedKey::Insert => "Insert",
            NamedKey::CapsLock => "Caps_Lock",
            NamedKey::F(n) => return Ok(format!("F{n}")),
            NamedKey::Back => "XF86Back",
            NamedKey::AppHome => "XF86HomePage",
            NamedKey::Recents => "super",
            NamedKey::Power => "XF86PowerOff",
            NamedKey::VolumeUp => "XF86AudioRaiseVolume",
            NamedKey::VolumeDown => "XF86AudioLowerVolume",
            NamedKey::Menu => "Menu",
        }
        .to_string(),
        Key::Char(c) => match c {
            ' ' => "space".to_string(),
            '+' => "plus".to_string(),
            '-' => "minus".to_string(),
            '=' => "equal".to_string(),
            ',' => "comma".to_string(),
            '.' => "period".to_string(),
            '/' => "slash".to_string(),
            ';' => "semicolon".to_string(),
            '\'' => "apostrophe".to_string(),
            '[' => "bracketleft".to_string(),
            ']' => "bracketright".to_string(),
            '\\' => "backslash".to_string(),
            '`' => "grave".to_string(),
            '\n' => "Return".to_string(),
            '\t' => "Tab".to_string(),
            other => other.to_string(),
        },
    })
}

/// Linux evdev key codes for ydotool.
fn evdev_code(key: &Key) -> Result<u16, DriverError> {
    Ok(match key {
        Key::Named(named) => match named {
            NamedKey::Enter => 28,
            NamedKey::Tab => 15,
            NamedKey::Escape => 1,
            NamedKey::Backspace => 14,
            NamedKey::Delete => 111,
            NamedKey::Space => 57,
            NamedKey::Up => 103,
            NamedKey::Down => 108,
            NamedKey::Left => 105,
            NamedKey::Right => 106,
            NamedKey::Home => 102,
            NamedKey::End => 107,
            NamedKey::PageUp => 104,
            NamedKey::PageDown => 109,
            NamedKey::Insert => 110,
            NamedKey::CapsLock => 58,
            NamedKey::F(n) => match n {
                1..=10 => 58 + u16::from(*n),
                11 => 87,
                12 => 88,
                _ => {
                    return Err(DriverError::Failed(format!(
                        "F{n} is not mapped for ydotool"
                    )));
                }
            },
            NamedKey::Back => 158,
            NamedKey::AppHome => 172,
            NamedKey::Recents => 125,
            NamedKey::Power => 116,
            NamedKey::VolumeUp => 115,
            NamedKey::VolumeDown => 114,
            NamedKey::Menu => 139,
        },
        Key::Char(c) => {
            let c = c.to_ascii_lowercase();
            match c {
                'a' => 30,
                'b' => 48,
                'c' => 46,
                'd' => 32,
                'e' => 18,
                'f' => 33,
                'g' => 34,
                'h' => 35,
                'i' => 23,
                'j' => 36,
                'k' => 37,
                'l' => 38,
                'm' => 50,
                'n' => 49,
                'o' => 24,
                'p' => 25,
                'q' => 16,
                'r' => 19,
                's' => 31,
                't' => 20,
                'u' => 22,
                'v' => 47,
                'w' => 17,
                'x' => 45,
                'y' => 21,
                'z' => 44,
                '1' => 2,
                '2' => 3,
                '3' => 4,
                '4' => 5,
                '5' => 6,
                '6' => 7,
                '7' => 8,
                '8' => 9,
                '9' => 10,
                '0' => 11,
                '-' => 12,
                '=' => 13,
                '[' => 26,
                ']' => 27,
                ';' => 39,
                '\'' => 40,
                '`' => 41,
                '\\' => 43,
                ',' => 51,
                '.' => 52,
                '/' => 53,
                ' ' => 57,
                '\n' => 28,
                '\t' => 15,
                other => {
                    return Err(DriverError::Failed(format!(
                        "no evdev code for `{other}`; use computer_type"
                    )));
                }
            }
        }
    })
}

impl Driver for LinuxDriver {
    fn info(&mut self) -> Result<TargetInfo, DriverError> {
        let mut notes = vec![format!(
            "session: {}",
            match self.session {
                SessionKind::X11 => "x11",
                SessionKind::Wayland => "wayland",
            }
        )];
        for (k, v) in &self.env {
            notes.push(format!("{k}={v}"));
        }
        if let Some((tool, _)) = &self.screenshot_tool {
            notes.push(format!("screenshot: {}", tool.display()));
        }
        if let Some(p) = &self.xdotool {
            notes.push(format!("xdotool: {}", p.display()));
        }
        if let Some(p) = &self.ydotool {
            notes.push(format!("ydotool: {}", p.display()));
        }
        if let Some(p) = &self.wtype {
            notes.push(format!("wtype: {}", p.display()));
        }
        for m in &self.missing {
            notes.push(format!("missing: {m}"));
        }
        let (w, h) = match self.screen_size() {
            Ok(size) => size,
            Err(e) => {
                notes.push(format!("screenshot failed: {e}"));
                (0, 0)
            }
        };
        Ok(TargetInfo {
            kind: TargetKind::Desktop,
            driver: match self.session {
                SessionKind::X11 => "linux-x11".into(),
                SessionKind::Wayland => "linux-wayland".into(),
            },
            device_w: w,
            device_h: h,
            notes,
            supports_ui_tree: false,
            supports_apps: true,
        })
    }

    fn screenshot(&mut self) -> Result<RawFrame, DriverError> {
        let (tool, kind) = self
            .screenshot_tool
            .clone()
            .ok_or_else(|| DriverError::Unavailable(self.missing.join("; ")))?;
        let path = std::env::temp_dir().join(format!("codewhale-cu-{}.png", std::process::id()));
        let path_str = path.to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&path);
        let args: Vec<&str> = match kind {
            "grim" => vec![&path_str],
            "scrot" => vec!["-o", &path_str],
            "maim" => vec![&path_str],
            "import" => vec!["-window", "root", &path_str],
            "gnome-screenshot" => vec!["-f", &path_str],
            "spectacle" => vec!["-b", "-n", "-o", &path_str],
            _ => vec![&path_str],
        };
        self.run(&tool, &args)?;
        let bytes = std::fs::read(&path)
            .map_err(|e| DriverError::Failed(format!("screenshot file missing: {e}")))?;
        let _ = std::fs::remove_file(&path);
        let img = crate::frame::decode(&bytes).map_err(DriverError::Failed)?;
        use image::GenericImageView;
        self.size = Some(img.dimensions());
        Ok(RawFrame { bytes })
    }

    fn move_to(&mut self, p: Point) -> Result<(), DriverError> {
        self.require_missing()?;
        let (x, y) = (p.x.round().to_string(), p.y.round().to_string());
        match self.input_tool()? {
            InputTool::Xdotool(t) => self.run(t, &["mousemove", "--sync", &x, &y]).map(|_| ()),
            InputTool::Ydotool(t) => self
                .run(t, &["mousemove", "--absolute", "-x", &x, "-y", &y])
                .map(|_| ()),
        }
    }

    fn click(
        &mut self,
        p: Point,
        button: Button,
        clicks: u32,
        hold_ms: u64,
    ) -> Result<(), DriverError> {
        self.move_to(p)?;
        match self.input_tool()? {
            InputTool::Xdotool(t) => {
                let b = Self::mouse_button_x(button);
                if hold_ms > 0 {
                    self.run(t, &["mousedown", b])?;
                    std::thread::sleep(Duration::from_millis(hold_ms));
                    self.run(t, &["mouseup", b]).map(|_| ())
                } else {
                    let repeat = clicks.max(1).to_string();
                    self.run(t, &["click", "--repeat", &repeat, "--delay", "80", b])
                        .map(|_| ())
                }
            }
            InputTool::Ydotool(t) => {
                let code = Self::mouse_button_y(button);
                if hold_ms > 0 {
                    let down = format!("0x{:02X}", 0x40 | (button as u8));
                    let up = format!("0x{:02X}", 0x80 | (button as u8));
                    self.run(t, &["click", &down])?;
                    std::thread::sleep(Duration::from_millis(hold_ms));
                    self.run(t, &["click", &up]).map(|_| ())
                } else {
                    let repeat = clicks.max(1).to_string();
                    self.run(
                        t,
                        &["click", "--repeat", &repeat, "--next-delay", "80", code],
                    )
                    .map(|_| ())
                }
            }
        }
    }

    fn drag(&mut self, from: Point, to: Point, duration_ms: u64) -> Result<(), DriverError> {
        self.move_to(from)?;
        let steps = 12u32;
        match self.input_tool()? {
            InputTool::Xdotool(t) => {
                self.run(t, &["mousedown", "1"])?;
                for i in 1..=steps {
                    let f = f64::from(i) / f64::from(steps);
                    let x = (from.x + (to.x - from.x) * f).round().to_string();
                    let y = (from.y + (to.y - from.y) * f).round().to_string();
                    self.run(t, &["mousemove", "--sync", &x, &y])?;
                    std::thread::sleep(Duration::from_millis(duration_ms / u64::from(steps)));
                }
                self.run(t, &["mouseup", "1"]).map(|_| ())
            }
            InputTool::Ydotool(t) => {
                self.run(t, &["click", "0x40"])?;
                for i in 1..=steps {
                    let f = f64::from(i) / f64::from(steps);
                    let x = (from.x + (to.x - from.x) * f).round().to_string();
                    let y = (from.y + (to.y - from.y) * f).round().to_string();
                    self.run(t, &["mousemove", "--absolute", "-x", &x, "-y", &y])?;
                    std::thread::sleep(Duration::from_millis(duration_ms / u64::from(steps)));
                }
                self.run(t, &["click", "0x80"]).map(|_| ())
            }
        }
    }

    fn scroll(&mut self, p: Point, dir: ScrollDir, amount: u32) -> Result<(), DriverError> {
        self.move_to(p)?;
        let repeat = amount.max(1).to_string();
        match self.input_tool()? {
            InputTool::Xdotool(t) => {
                let b = match dir {
                    ScrollDir::Up => "4",
                    ScrollDir::Down => "5",
                    ScrollDir::Left => "6",
                    ScrollDir::Right => "7",
                };
                self.run(t, &["click", "--repeat", &repeat, "--delay", "30", b])
                    .map(|_| ())
            }
            InputTool::Ydotool(t) => {
                let n = i64::from(amount.max(1));
                let (x, y) = match dir {
                    ScrollDir::Up => (0, n),
                    ScrollDir::Down => (0, -n),
                    ScrollDir::Left => (n, 0),
                    ScrollDir::Right => (-n, 0),
                };
                let (x, y) = (x.to_string(), y.to_string());
                self.run(t, &["mousemove", "--wheel", "-x", &x, "-y", &y])
                    .map(|_| ())
            }
        }
    }

    fn type_text(&mut self, text: &str) -> Result<(), DriverError> {
        self.require_missing()?;
        if let Some(wtype) = &self.wtype
            && self.session == SessionKind::Wayland
        {
            return self.run(wtype, &["--", text]).map(|_| ());
        }
        match self.input_tool()? {
            InputTool::Xdotool(t) => self
                .run(t, &["type", "--delay", "12", "--", text])
                .map(|_| ()),
            InputTool::Ydotool(t) => self
                .run(t, &["type", "--key-delay", "12", "--", text])
                .map(|_| ()),
        }
    }

    fn key(&mut self, combo: &KeyCombo) -> Result<(), DriverError> {
        self.require_missing()?;
        match self.input_tool()? {
            InputTool::Xdotool(t) => {
                let mut parts: Vec<String> = Vec::new();
                if combo.modifiers.ctrl {
                    parts.push("ctrl".into());
                }
                if combo.modifiers.alt {
                    parts.push("alt".into());
                }
                if combo.modifiers.shift {
                    parts.push("shift".into());
                }
                if combo.modifiers.meta {
                    parts.push("super".into());
                }
                parts.push(xdotool_keysym(&combo.key)?);
                let spec = parts.join("+");
                self.run(t, &["key", "--clearmodifiers", &spec]).map(|_| ())
            }
            InputTool::Ydotool(t) => {
                let mut codes: Vec<u16> = Vec::new();
                if combo.modifiers.ctrl {
                    codes.push(29);
                }
                if combo.modifiers.alt {
                    codes.push(56);
                }
                if combo.modifiers.shift
                    || matches!(combo.key, Key::Char(c) if c.is_ascii_uppercase())
                {
                    codes.push(42);
                }
                if combo.modifiers.meta {
                    codes.push(125);
                }
                codes.push(evdev_code(&combo.key)?);
                let mut args: Vec<String> = vec!["key".into()];
                for c in &codes {
                    args.push(format!("{c}:1"));
                }
                for c in codes.iter().rev() {
                    args.push(format!("{c}:0"));
                }
                let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                self.run(t, &refs).map(|_| ())
            }
        }
    }

    fn ui_tree(&mut self) -> Result<Vec<UiNode>, DriverError> {
        Err(DriverError::Unsupported("UI tree dumps are only available on Android and HarmonyOS; use computer_zoom on desktop".to_string()))
    }

    fn app(&mut self, action: AppAction<'_>) -> Result<String, DriverError> {
        match action {
            AppAction::Launch(name) => {
                if let Some(gtk) = process::which("gtk-launch")
                    && self.run(&gtk, &[name]).is_ok()
                {
                    return Ok(format!("launched {name} via gtk-launch"));
                }
                let sh = process::which("sh")
                    .ok_or_else(|| DriverError::Unavailable("sh not found".to_string()))?;
                let script = format!("nohup {} >/dev/null 2>&1 &", process::sh_quote(name));
                self.run(&sh, &["-c", &script])?;
                Ok(format!("started `{name}` in the background"))
            }
            AppAction::List => {
                let mut names = Vec::new();
                let mut dirs = vec![
                    PathBuf::from("/usr/share/applications"),
                    PathBuf::from("/usr/local/share/applications"),
                ];
                if let Some(home) = process::home() {
                    dirs.push(home.join(".local/share/applications"));
                }
                for dir in dirs {
                    if let Ok(entries) = std::fs::read_dir(dir) {
                        for entry in entries.flatten() {
                            if let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str())
                                && entry.path().extension().and_then(|e| e.to_str())
                                    == Some("desktop")
                            {
                                names.push(stem.to_string());
                            }
                        }
                    }
                }
                names.sort_unstable();
                names.dedup();
                Ok(format!(
                    "{} desktop entries (launch by id):\n{}",
                    names.len(),
                    names.join("\n")
                ))
            }
            AppAction::Current => {
                let xdotool = self.xdotool.clone().ok_or_else(|| {
                    DriverError::Unavailable(
                        "xdotool needed for the active window name".to_string(),
                    )
                })?;
                let out = self.run(&xdotool, &["getactivewindow", "getwindowname"])?;
                Ok(format!("active window: {}", out.stdout_text().trim()))
            }
        }
    }

    fn devices(&mut self) -> Result<String, DriverError> {
        Ok("desktop target (no adb/hdc device selected); set [android] or [harmony] in ~/.codewhale/computer-use.toml to drive a phone".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysyms_and_evdev_codes() {
        assert_eq!(
            xdotool_keysym(&Key::Named(NamedKey::Enter)).unwrap(),
            "Return"
        );
        assert_eq!(xdotool_keysym(&Key::Char('+')).unwrap(), "plus");
        assert_eq!(xdotool_keysym(&Key::Char('A')).unwrap(), "A");
        assert_eq!(evdev_code(&Key::Char('a')).unwrap(), 30);
        assert_eq!(evdev_code(&Key::Named(NamedKey::F(12))).unwrap(), 88);
        assert!(evdev_code(&Key::Char('é')).is_err());
    }
}
